use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::sync::{Mutex, Notify, mpsc, watch};
use tracing::{debug, info, warn};

use crate::PROTOCOL_VERSION;
use crate::clipboard::{ClipItem, ClipRingBuffer};
use crate::crypto::{MasterKey, client_handshake};
use crate::now_millis;
use crate::protocol::{PeerIdentity, SyncRequest, WireMessage};
use crate::tailnet::{TailnetPolicy, resolve_tailnet_addr};

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub host: String,
    pub port: u16,
    pub key: MasterKey,
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
    pub policy: TailnetPolicy,
    /// Ignore/skip any server clips with `created_at_ms <= initial_cleared_before_ms`.
    pub initial_cleared_before_ms: u64,
}

#[derive(Debug, Clone)]
pub enum ClientEvent {
    Connecting {
        target: String,
    },
    Connected {
        peer_name: String,
        peer_addr: String,
        key_fingerprint: String,
    },
    Disconnected {
        reason: String,
    },
    ClipReceived(ClipItem),
    ClipSent {
        clip_id: String,
    },
    HistoryCleared {
        cleared_at_ms: u64,
    },
    Error(String),
}

enum ControlCommand {
    SendClip(ClipItem),
    ClearHistory(u64),
    RequestSync,
    ReconnectNow,
}

#[derive(Clone)]
pub struct SyncClientHandle {
    cmd_tx: mpsc::Sender<ControlCommand>,
    shutdown_tx: watch::Sender<bool>,
    reconnect_notify: Arc<Notify>,
    history: Arc<Mutex<ClipRingBuffer>>,
}

impl SyncClientHandle {
    pub async fn push_clip(&self, clip: ClipItem) -> Result<(), String> {
        {
            let mut h = self.history.lock().await;
            h.insert(clip.clone());
        }
        self.cmd_tx
            .send(ControlCommand::SendClip(clip))
            .await
            .map_err(|_| "sync client is stopped".to_string())
    }

    pub fn try_push_clip(&self, clip: ClipItem) -> Result<(), String> {
        self.cmd_tx
            .try_send(ControlCommand::SendClip(clip))
            .map_err(|e| format!("failed to enqueue clip: {e}"))
    }

    pub fn request_sync(&self) {
        let _ = self.cmd_tx.try_send(ControlCommand::RequestSync);
    }

    pub fn trigger_reconnect(&self) {
        self.reconnect_notify.notify_waiters();
        let _ = self.cmd_tx.try_send(ControlCommand::ReconnectNow);
    }

    pub async fn history_snapshot(&self) -> Vec<ClipItem> {
        let h = self.history.lock().await;
        h.items().cloned().collect()
    }

    pub async fn clear_history(&self) {
        let ts = now_millis();
        {
            let mut h = self.history.lock().await;
            h.clear_up_to(ts);
        }
        let _ = self.cmd_tx.send(ControlCommand::ClearHistory(ts)).await;
    }

    pub fn try_clear_history(&self, cleared_at_ms: u64) {
        let _ = self.cmd_tx.try_send(ControlCommand::ClearHistory(cleared_at_ms));
    }

    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
        self.reconnect_notify.notify_waiters();
    }
}

/// Spawns a persistent, auto-reconnecting, tailnet-enforced E2EE sync client task.
pub fn spawn_sync_client(
    config: ClientConfig,
    event_tx: mpsc::UnboundedSender<ClientEvent>,
) -> SyncClientHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel::<ControlCommand>(64);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let reconnect_notify = Arc::new(Notify::new());
    let history = Arc::new(Mutex::new(
        ClipRingBuffer::default().with_cleared_before(config.initial_cleared_before_ms),
    ));

    let handle = SyncClientHandle {
        cmd_tx,
        shutdown_tx,
        reconnect_notify: reconnect_notify.clone(),
        history: history.clone(),
    };

    tokio::spawn(run_client_loop(
        config,
        cmd_rx,
        shutdown_rx,
        reconnect_notify,
        history,
        event_tx,
    ));

    handle
}

async fn run_client_loop(
    config: ClientConfig,
    mut cmd_rx: mpsc::Receiver<ControlCommand>,
    mut shutdown_rx: watch::Receiver<bool>,
    reconnect_notify: Arc<Notify>,
    history: Arc<Mutex<ClipRingBuffer>>,
    event_tx: mpsc::UnboundedSender<ClientEvent>,
) {
    let mut backoff_secs: u64 = 1;
    let mut pending_outbox: Option<ClipItem> = None;
    let mut pending_clear_ms: Option<u64> = None;

    loop {
        if *shutdown_rx.borrow() {
            break;
        }

        // Drain any queued commands while offline.
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                ControlCommand::SendClip(item) => {
                    let mut h = history.lock().await;
                    h.insert(item.clone());
                    pending_outbox = Some(item);
                }
                ControlCommand::ClearHistory(ts) => {
                    let mut h = history.lock().await;
                    h.clear_up_to(ts);
                    if pending_outbox
                        .as_ref()
                        .is_some_and(|i| i.meta.created_at_ms <= ts)
                    {
                        pending_outbox = None;
                    }
                    pending_clear_ms = Some(pending_clear_ms.map_or(ts, |prev| prev.max(ts)));
                }
                ControlCommand::RequestSync | ControlCommand::ReconnectNow => {}
            }
        }

        let target_label = format!("{}:{}", config.host, config.port);
        let _ = event_tx.send(ClientEvent::Connecting {
            target: target_label.clone(),
        });

        match connect_and_run_session(
            &config,
            &mut cmd_rx,
            &mut shutdown_rx,
            &history,
            &event_tx,
            &mut pending_outbox,
            &mut pending_clear_ms,
        )
        .await
        {
            Ok(()) => {
                // Clean shutdown or explicit reconnect request
                backoff_secs = 1;
            }
            Err(reason) => {
                warn!(target = %target_label, error = %reason, "tailnet sync session ended");
                let _ = event_tx.send(ClientEvent::Disconnected { reason });
            }
        }

        if *shutdown_rx.borrow() {
            break;
        }

        // Wait for backoff or immediate wake (e.g., network came back or user tapped Send).
        let sleep = tokio::time::sleep(Duration::from_secs(backoff_secs));
        tokio::pin!(sleep);
        tokio::select! {
            _ = &mut sleep => {
                backoff_secs = (backoff_secs * 2).min(15);
            }
            _ = reconnect_notify.notified() => {
                backoff_secs = 1;
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    ControlCommand::SendClip(item) => {
                        let mut h = history.lock().await;
                        h.insert(item.clone());
                        pending_outbox = Some(item);
                        backoff_secs = 1;
                    }
                    ControlCommand::ClearHistory(ts) => {
                        let mut h = history.lock().await;
                        h.clear_up_to(ts);
                        if pending_outbox
                            .as_ref()
                            .is_some_and(|i| i.meta.created_at_ms <= ts)
                        {
                            pending_outbox = None;
                        }
                        pending_clear_ms = Some(pending_clear_ms.map_or(ts, |prev| prev.max(ts)));
                        backoff_secs = 1;
                    }
                    ControlCommand::ReconnectNow | ControlCommand::RequestSync => {
                        backoff_secs = 1;
                    }
                }
            }
        }
    }
}

async fn connect_and_run_session(
    config: &ClientConfig,
    cmd_rx: &mut mpsc::Receiver<ControlCommand>,
    shutdown_rx: &mut watch::Receiver<bool>,
    history: &Arc<Mutex<ClipRingBuffer>>,
    event_tx: &mpsc::UnboundedSender<ClientEvent>,
    pending_outbox: &mut Option<ClipItem>,
    pending_clear_ms: &mut Option<u64>,
) -> Result<(), String> {
    let addr = resolve_tailnet_addr(&config.host, config.port, config.policy)
        .await
        .map_err(|e| e.to_string())?;

    let stream = tokio::time::timeout(Duration::from_secs(8), TcpStream::connect(addr))
        .await
        .map_err(|_| format!("timed out connecting to {addr} (check Tailscale VPN & ACL)"))?
        .map_err(|e| format!("failed to connect to {addr}: {e}"))?;

    let _ = stream.set_nodelay(true);

    let (mut reader, mut writer) = tokio::time::timeout(
        Duration::from_secs(8),
        client_handshake(stream, &config.key),
    )
    .await
    .map_err(|_| "E2EE handshake timed out".to_string())?
    .map_err(|e| format!("E2EE handshake failed: {e}"))?;

    // Send our encrypted identity
    let last_seen_clip_id = {
        let h = history.lock().await;
        h.latest().map(|c| c.meta.id.clone())
    };
    let my_id = WireMessage::Identify(PeerIdentity {
        device_id: config.device_id.clone(),
        device_name: config.device_name.clone(),
        platform: config.platform.clone(),
        protocol_version: PROTOCOL_VERSION,
        last_seen_clip_id,
    });
    writer
        .write_frame(&my_id.encode().map_err(|e| e.to_string())?)
        .await
        .map_err(|e| e.to_string())?;

    // Expect server's encrypted identity
    let peer_frame = tokio::time::timeout(Duration::from_secs(8), reader.read_frame())
        .await
        .map_err(|_| "timed out awaiting peer identity".to_string())?
        .map_err(|e| format!("failed reading peer identity: {e}"))?;
    let peer_msg = WireMessage::decode(&peer_frame).map_err(|e| e.to_string())?;
    let peer_name = match peer_msg {
        WireMessage::Identify(id) => id.device_name,
        _ => return Err("unexpected first message from server".into()),
    };

    info!(peer = %peer_name, addr = %addr, "connected over encrypted tailnet channel");
    let _ = event_tx.send(ClientEvent::Connected {
        peer_name: peer_name.clone(),
        peer_addr: addr.to_string(),
        key_fingerprint: config.key.fingerprint(),
    });

    // Flush any pending history clear before requesting sync so cleared items are not replayed
    if let Some(ts) = pending_clear_ms.take() {
        let clear_msg = WireMessage::HistoryClear { cleared_at_ms: ts }
            .encode()
            .map_err(|e| e.to_string())?;
        if let Err(e) = writer.write_frame(&clear_msg).await {
            *pending_clear_ms = Some(ts);
            return Err(e.to_string());
        }
    }

    // Request missed recent clips newer than our clear watermark from server
    let (known_ids, since_ms): (Vec<String>, u64) = {
        let h = history.lock().await;
        (
            h.items().map(|c| c.meta.id.clone()).collect(),
            h.cleared_before_ms(),
        )
    };
    let sync_req = WireMessage::SyncRequest(SyncRequest {
        known_clip_ids: known_ids,
        limit: 10,
        since_ms,
    });
    writer
        .write_frame(&sync_req.encode().map_err(|e| e.to_string())?)
        .await
        .map_err(|e| e.to_string())?;

    // Flush any clip queued while offline
    if let Some(item) = pending_outbox.take() {
        let clip_id = item.meta.id.clone();
        let encoded = WireMessage::ClipPush(item).encode().map_err(|e| e.to_string())?;
        writer.write_frame(&encoded).await.map_err(|e| e.to_string())?;
        let _ = event_tx.send(ClientEvent::ClipSent { clip_id });
    }

    let mut heartbeat = tokio::time::interval(Duration::from_secs(25));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await; // consume immediate first tick

    let mut awaiting_pong = false;

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    return Ok(());
                }
            }
            _ = heartbeat.tick() => {
                if awaiting_pong {
                    return Err("heartbeat timeout (connection stale)".into());
                }
                let ping = WireMessage::Ping { ts_ms: now_millis() }
                    .encode()
                    .map_err(|e| e.to_string())?;
                writer.write_frame(&ping).await.map_err(|e| e.to_string())?;
                awaiting_pong = true;
            }
            maybe_cmd = cmd_rx.recv() => {
                match maybe_cmd {
                    Some(ControlCommand::SendClip(item)) => {
                        let clip_id = item.meta.id.clone();
                        let encoded = WireMessage::ClipPush(item.clone())
                            .encode()
                            .map_err(|e| e.to_string())?;
                        if let Err(e) = writer.write_frame(&encoded).await {
                            *pending_outbox = Some(item);
                            return Err(format!("send failed: {e}"));
                        }
                        let _ = event_tx.send(ClientEvent::ClipSent { clip_id });
                    }
                    Some(ControlCommand::ClearHistory(ts)) => {
                        {
                            let mut h = history.lock().await;
                            h.clear_up_to(ts);
                        }
                        let encoded = WireMessage::HistoryClear { cleared_at_ms: ts }
                            .encode()
                            .map_err(|e| e.to_string())?;
                        if let Err(e) = writer.write_frame(&encoded).await {
                            *pending_clear_ms = Some(ts);
                            return Err(format!("clear history send failed: {e}"));
                        }
                    }
                    Some(ControlCommand::RequestSync) => {
                        let (known_ids, since_ms): (Vec<String>, u64) = {
                            let h = history.lock().await;
                            (
                                h.items().map(|c| c.meta.id.clone()).collect(),
                                h.cleared_before_ms(),
                            )
                        };
                        let req = WireMessage::SyncRequest(SyncRequest {
                            known_clip_ids: known_ids,
                            limit: 10,
                            since_ms,
                        }).encode().map_err(|e| e.to_string())?;
                        writer.write_frame(&req).await.map_err(|e| e.to_string())?;
                    }
                    Some(ControlCommand::ReconnectNow) => {
                        return Ok(());
                    }
                    None => return Ok(()),
                }
            }
            frame_res = reader.read_frame() => {
                let frame = frame_res.map_err(|e| format!("connection closed: {e}"))?;
                let msg = WireMessage::decode(&frame).map_err(|e| e.to_string())?;
                match msg {
                    WireMessage::ClipPush(item) => {
                        let clip_id = item.meta.id.clone();
                        let is_new = {
                            let mut h = history.lock().await;
                            h.insert(item.clone())
                        };
                        let ack = WireMessage::ClipAck { clip_id }
                            .encode()
                            .map_err(|e| e.to_string())?;
                        let _ = writer.write_frame(&ack).await;
                        if is_new && item.meta.origin_device_id != config.device_id {
                            let _ = event_tx.send(ClientEvent::ClipReceived(item));
                        }
                    }
                    WireMessage::ClipAck { clip_id } => {
                        debug!(%clip_id, "peer acknowledged clip");
                    }
                    WireMessage::Ping { ts_ms } => {
                        let pong = WireMessage::Pong { ts_ms }
                            .encode()
                            .map_err(|e| e.to_string())?;
                        writer.write_frame(&pong).await.map_err(|e| e.to_string())?;
                    }
                    WireMessage::Pong { .. } => {
                        awaiting_pong = false;
                    }
                    WireMessage::HistoryClear { cleared_at_ms } => {
                        {
                            let mut h = history.lock().await;
                            h.clear_up_to(cleared_at_ms);
                        }
                        let _ = event_tx.send(ClientEvent::HistoryCleared { cleared_at_ms });
                    }
                    WireMessage::Error { message } => {
                        let _ = event_tx.send(ClientEvent::Error(message));
                    }
                    WireMessage::Identify(_) | WireMessage::SyncRequest(_) => {}
                }
            }
        }
    }
}
