use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use buddyclip_core::PROTOCOL_VERSION;
use buddyclip_core::clipboard::{ClipItem, ClipRingBuffer, compute_content_hash};
use buddyclip_core::client::{ClientConfig, ClientEvent, SyncClientHandle, spawn_sync_client};
use buddyclip_core::crypto::{MasterKey, server_handshake};
use buddyclip_core::now_millis;
use buddyclip_core::protocol::{PeerIdentity, WireMessage};
use buddyclip_core::tailnet::{TailnetPolicy, detect_tailscale_status, validate_ip};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, error, info, warn};

use crate::backend::{
    ClipboardBackendKind, LocalClip, detect_backend, notify_desktop, spawn_clipboard_watcher,
    write_system_clipboard,
};
use crate::config::AppConfig;
use crate::ipc::{
    ConnectedPeerInfo, DaemonStatus, HistoryEntry, IpcRequest, IpcResponse, decode_payload,
    encode_payload,
};

struct ActivePeer {
    info: ConnectedPeerInfo,
    tx: mpsc::Sender<WireMessage>,
}

pub struct DaemonState {
    config: AppConfig,
    key: MasterKey,
    listen_addr: SocketAddr,
    backend: ClipboardBackendKind,
    policy: TailnetPolicy,
    history: Mutex<ClipRingBuffer>,
    peers: Mutex<HashMap<u64, ActivePeer>>,
    outbound_clients: Mutex<Vec<SyncClientHandle>>,
    /// Content hash of the most recently applied remote clip (prevents clipboard echo loops).
    last_remote_hash: Mutex<Option<String>>,
    /// Content hash of the most recently seen local clip.
    last_local_hash: Mutex<Option<String>>,
    next_conn_id: AtomicU64,
    clips_sent: AtomicU64,
    clips_received: AtomicU64,
}

impl DaemonState {
    async fn handle_local_clip(self: &Arc<Self>, clip: LocalClip) {
        let hash = compute_content_hash(&clip.mime_type, &clip.payload);
        {
            let remote = self.last_remote_hash.lock().await;
            if remote.as_deref() == Some(hash.as_str()) {
                debug!("ignoring local clipboard event matching last received remote clip");
                return;
            }
        }
        {
            let mut last_local = self.last_local_hash.lock().await;
            if last_local.as_deref() == Some(hash.as_str()) {
                return;
            }
            *last_local = Some(hash);
        }

        let item = match ClipItem::new(
            self.config.device_id.clone(),
            self.config.device_name.clone(),
            clip.mime_type,
            clip.payload,
        ) {
            Ok(i) => i,
            Err(e) => {
                debug!(error = %e, "ignoring invalid local clip");
                return;
            }
        };

        let is_new = {
            let mut h = self.history.lock().await;
            h.insert(item.clone())
        };
        if !is_new {
            return;
        }

        info!(
            clip_id = %item.meta.id,
            mime = %item.meta.mime_type,
            bytes = item.meta.byte_len,
            "broadcasting local clipboard update to tailnet peers"
        );
        self.clips_sent.fetch_add(1, Ordering::Relaxed);
        self.broadcast_clip(item, None).await;
    }

    async fn handle_remote_clip(self: &Arc<Self>, item: ClipItem, source_conn_id: Option<u64>) {
        if item.meta.origin_device_id == self.config.device_id {
            return;
        }
        let is_new = {
            let mut h = self.history.lock().await;
            h.insert(item.clone())
        };
        if !is_new {
            return;
        }

        info!(
            from = %item.meta.origin_device_name,
            mime = %item.meta.mime_type,
            bytes = item.meta.byte_len,
            "received encrypted clip from tailnet peer"
        );
        self.clips_received.fetch_add(1, Ordering::Relaxed);

        // Set echo-guard BEFORE writing to local clipboard so wl-paste --watch ignores it.
        {
            let mut guard = self.last_remote_hash.lock().await;
            *guard = Some(item.meta.content_hash.clone());
        }
        {
            let mut local = self.last_local_hash.lock().await;
            *local = Some(item.meta.content_hash.clone());
        }

        if let Err(e) =
            write_system_clipboard(self.backend, &item.meta.mime_type, &item.payload).await
        {
            warn!(error = %e, "failed to write remote clip to system clipboard");
        } else if self.config.notifications {
            let title = format!("Synced from {}", item.meta.origin_device_name);
            let preview = item.preview(90);
            notify_desktop(&title, &preview);
        }

        // Relay to all other connected devices (e.g. Android -> PC1 -> PC2).
        self.broadcast_clip(item, source_conn_id).await;
    }

    async fn broadcast_clip(&self, item: ClipItem, exclude_conn_id: Option<u64>) {
        let msg = WireMessage::ClipPush(item.clone());
        let senders: Vec<(u64, mpsc::Sender<WireMessage>)> = {
            let peers = self.peers.lock().await;
            peers
                .iter()
                .filter(|(id, _)| Some(**id) != exclude_conn_id)
                .map(|(id, peer)| (*id, peer.tx.clone()))
                .collect()
        };

        for (conn_id, tx) in senders {
            if tx.try_send(msg.clone()).is_err() {
                debug!(conn_id, "dropping clip send to slow/closed peer");
            }
        }

        let outbound = self.outbound_clients.lock().await;
        for client in outbound.iter() {
            let _ = client.try_push_clip(item.clone());
        }
    }

    async fn handle_history_clear(&self, cleared_at_ms: u64, exclude_conn_id: Option<u64>) {
        {
            let mut h = self.history.lock().await;
            h.clear_up_to(cleared_at_ms);
        }
        info!(cleared_at_ms, "cleared in-memory clipboard history across peers");

        let msg = WireMessage::HistoryClear { cleared_at_ms };
        let senders: Vec<(u64, mpsc::Sender<WireMessage>)> = {
            let peers = self.peers.lock().await;
            peers
                .iter()
                .filter(|(id, _)| Some(**id) != exclude_conn_id)
                .map(|(id, peer)| (*id, peer.tx.clone()))
                .collect()
        };
        for (_, tx) in senders {
            let _ = tx.try_send(msg.clone());
        }

        let outbound = self.outbound_clients.lock().await;
        for client in outbound.iter() {
            client.try_clear_history(cleared_at_ms);
        }
    }

    async fn status_snapshot(&self) -> DaemonStatus {
        let connected_peers = {
            let peers = self.peers.lock().await;
            peers.values().map(|p| p.info.clone()).collect()
        };
        let history_count = self.history.lock().await.len();
        DaemonStatus {
            device_id: self.config.device_id.clone(),
            device_name: self.config.device_name.clone(),
            listen_addr: self.listen_addr.to_string(),
            key_fingerprint: self.key.fingerprint(),
            backend: self.backend.as_str().to_string(),
            connected_peers,
            history_count,
            clips_sent: self.clips_sent.load(Ordering::Relaxed),
            clips_received: self.clips_received.load(Ordering::Relaxed),
        }
    }
}

pub async fn run_daemon(
    config: AppConfig,
    socket_path: PathBuf,
    policy: TailnetPolicy,
) -> Result<()> {
    let key = config.master_key()?;
    let bind_ip = resolve_bind_ip(&config, policy).await?;
    let listen_addr = SocketAddr::new(bind_ip, config.port);

    let listener = TcpListener::bind(listen_addr).await.with_context(|| {
        format!(
            "failed to bind strictly to Tailscale address {listen_addr} (is Tailscale running?)"
        )
    })?;

    let backend = detect_backend();
    let state = Arc::new(DaemonState {
        config: config.clone(),
        key: key.clone(),
        listen_addr,
        backend,
        policy,
        history: Mutex::new(ClipRingBuffer::default()),
        peers: Mutex::new(HashMap::new()),
        outbound_clients: Mutex::new(Vec::new()),
        last_remote_hash: Mutex::new(None),
        last_local_hash: Mutex::new(None),
        next_conn_id: AtomicU64::new(1),
        clips_sent: AtomicU64::new(0),
        clips_received: AtomicU64::new(0),
    });

    info!(
        device = %config.device_name,
        listen = %listen_addr,
        fingerprint = %key.fingerprint(),
        backend = %backend.as_str(),
        "buddyclip daemon listening exclusively on tailnet"
    );

    // 1. Start local OS clipboard watcher
    let (clip_tx, mut clip_rx) = mpsc::channel::<LocalClip>(32);
    spawn_clipboard_watcher(backend, config.sync_images, config.skip_sensitive, clip_tx);
    {
        let st = state.clone();
        tokio::spawn(async move {
            while let Some(clip) = clip_rx.recv().await {
                st.handle_local_clip(clip).await;
            }
        });
    }

    // 2. Start outbound PC-to-PC peer mesh connections (if configured)
    for peer_spec in &config.peers {
        if let Some((host, port)) = parse_peer_spec(peer_spec, config.port) {
            let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
            let handle = spawn_sync_client(
                ClientConfig {
                    host: host.clone(),
                    port,
                    key: key.clone(),
                    device_id: config.device_id.clone(),
                    device_name: config.device_name.clone(),
                    platform: "linux".into(),
                    policy,
                    initial_cleared_before_ms: 0,
                },
                ev_tx,
            );
            state.outbound_clients.lock().await.push(handle);
            let st = state.clone();
            tokio::spawn(async move {
                while let Some(ev) = ev_rx.recv().await {
                    match ev {
                        ClientEvent::ClipReceived(item) => {
                            st.handle_remote_clip(item, None).await;
                        }
                        ClientEvent::HistoryCleared { cleared_at_ms } => {
                            st.handle_history_clear(cleared_at_ms, None).await;
                        }
                        ClientEvent::Connected { peer_name, peer_addr, .. } => {
                            info!(%peer_name, %peer_addr, "connected to outbound peer daemon");
                        }
                        ClientEvent::Disconnected { reason } => {
                            debug!(%reason, "outbound peer disconnected");
                        }
                        _ => {}
                    }
                }
            });
        }
    }

    // 3. Start local Unix IPC control socket
    let _ipc_guard = spawn_ipc_server(socket_path.clone(), state.clone())?;

    // 4. Main accept loop (tailnet-only!)
    let mut sigterm =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("received SIGINT; shutting down cleanly");
                break;
            }
            _ = sigterm.recv() => {
                info!("received SIGTERM; shutting down cleanly");
                break;
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, peer_addr)) => {
                        if let Err(e) = validate_ip(peer_addr.ip(), state.policy) {
                            warn!(%peer_addr, error = %e, "REJECTED non-tailnet connection");
                            drop(stream);
                            continue;
                        }
                        let st = state.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_client_connection(st, stream, peer_addr).await {
                                debug!(%peer_addr, error = %e, "client connection ended");
                            }
                        });
                    }
                    Err(e) => {
                        error!(error = %e, "TCP accept error");
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                }
            }
        }
    }

    let _ = std::fs::remove_file(&socket_path);
    Ok(())
}

async fn resolve_bind_ip(config: &AppConfig, policy: TailnetPolicy) -> Result<IpAddr> {
    if let Some(ref explicit) = config.bind_ip {
        let ip: IpAddr = explicit
            .parse()
            .with_context(|| format!("invalid bind_ip '{explicit}' in config.toml"))?;
        validate_ip(ip, policy).context("configured bind_ip is not a Tailscale tailnet address")?;
        return Ok(ip);
    }

    let ts = detect_tailscale_status().await.context(
        "could not auto-detect Tailscale IP (is `tailscaled` running and connected?)",
    )?;
    if let Some(v4) = ts.self_ipv4() {
        return Ok(IpAddr::V4(v4));
    }
    if let Some(v6) = ts.self_ipv6() {
        return Ok(IpAddr::V6(v6));
    }
    anyhow::bail!("Tailscale is running, but no 100.64.0.0/10 or fd7a:115c:a1e0::/48 IP was found")
}

fn parse_peer_spec(spec: &str, default_port: u16) -> Option<(String, u16)> {
    let s = spec.trim();
    if s.is_empty() {
        return None;
    }
    if let Some((host, port_str)) = s.rsplit_once(':')
        && let Ok(port) = port_str.parse::<u16>()
    {
        return Some((host.trim_matches(['[', ']']).to_string(), port));
    }
    Some((s.to_string(), default_port))
}

async fn handle_client_connection(
    state: Arc<DaemonState>,
    stream: TcpStream,
    peer_addr: SocketAddr,
) -> Result<()> {
    let _ = stream.set_nodelay(true);

    let (mut reader, mut writer) =
        tokio::time::timeout(Duration::from_secs(8), server_handshake(stream, &state.key))
            .await
            .context("handshake timeout")?
            .context("handshake authentication failed")?;

    // Send server identity
    let last_seen = state.history.lock().await.latest().map(|c| c.meta.id.clone());
    let server_id = WireMessage::Identify(PeerIdentity {
        device_id: state.config.device_id.clone(),
        device_name: state.config.device_name.clone(),
        platform: "linux".into(),
        protocol_version: PROTOCOL_VERSION,
        last_seen_clip_id: last_seen,
    });
    writer.write_frame(&server_id.encode()?).await?;

    // Read client identity
    let first_frame = tokio::time::timeout(Duration::from_secs(8), reader.read_frame())
        .await
        .context("client identify timeout")??;
    let client_id = match WireMessage::decode(&first_frame)? {
        WireMessage::Identify(id) => id,
        _ => anyhow::bail!("expected Identify as first message"),
    };

    let conn_id = state.next_conn_id.fetch_add(1, Ordering::Relaxed);
    let (out_tx, mut out_rx) = mpsc::channel::<WireMessage>(64);
    let peer_info = ConnectedPeerInfo {
        device_id: client_id.device_id.clone(),
        device_name: client_id.device_name.clone(),
        platform: client_id.platform.clone(),
        peer_addr: peer_addr.to_string(),
        connected_at_ms: now_millis(),
    };

    {
        let mut peers = state.peers.lock().await;
        // Replace any stale connection from the exact same device_id so reconnects stay clean.
        peers.retain(|_, p| p.info.device_id != client_id.device_id);
        peers.insert(
            conn_id,
            ActivePeer {
                info: peer_info,
                tx: out_tx.clone(),
            },
        );
    }

    info!(
        conn_id,
        device = %client_id.device_name,
        platform = %client_id.platform,
        %peer_addr,
        "authenticated tailnet peer connected"
    );

    let write_task = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let encoded = match msg.encode() {
                Ok(b) => b,
                Err(e) => {
                    warn!(error = %e, "failed encoding outbound wire message");
                    continue;
                }
            };
            if writer.write_frame(&encoded).await.is_err() {
                break;
            }
        }
    });

    let read_result: Result<()> = async {
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(90), reader.read_frame())
                .await
                .context("peer read idle timeout")??;
            let msg = WireMessage::decode(&frame)?;
            match msg {
                WireMessage::ClipPush(item) => {
                    let clip_id = item.meta.id.clone();
                    let _ = out_tx.try_send(WireMessage::ClipAck { clip_id });
                    state.handle_remote_clip(item, Some(conn_id)).await;
                }
                WireMessage::SyncRequest(req) => {
                    let known: HashSet<String> = req.known_clip_ids.into_iter().collect();
                    let limit = req.limit.clamp(1, 25);
                    let to_send: Vec<ClipItem> = {
                        let history = state.history.lock().await;
                        history
                            .items()
                            .filter(|c| {
                                c.meta.created_at_ms > req.since_ms && !known.contains(&c.meta.id)
                            })
                            .take(limit)
                            .cloned()
                            .collect()
                    };
                    // Replay oldest-to-newest so the latest clip ends up on top of the client's clipboard/history.
                    for item in to_send.into_iter().rev() {
                        if out_tx.send(WireMessage::ClipPush(item)).await.is_err() {
                            break;
                        }
                    }
                }
                WireMessage::HistoryClear { cleared_at_ms } => {
                    state.handle_history_clear(cleared_at_ms, Some(conn_id)).await;
                }
                WireMessage::Ping { ts_ms } => {
                    let _ = out_tx.try_send(WireMessage::Pong { ts_ms });
                }
                WireMessage::Pong { .. } | WireMessage::ClipAck { .. } => {}
                WireMessage::Error { message } => {
                    warn!(device = %client_id.device_name, %message, "peer reported error");
                }
                WireMessage::Identify(_) => {}
            }
        }
    }
    .await;

    {
        let mut peers = state.peers.lock().await;
        peers.remove(&conn_id);
    }
    write_task.abort();
    info!(
        conn_id,
        device = %client_id.device_name,
        %peer_addr,
        "tailnet peer disconnected"
    );

    read_result
}

struct IpcGuard {
    path: PathBuf,
}

impl Drop for IpcGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn spawn_ipc_server(socket_path: PathBuf, state: Arc<DaemonState>) -> Result<IpcGuard> {
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }
    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("failed to bind IPC socket {}", socket_path.display()))?;
    let _ = std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600));

    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let st = state.clone();
                    tokio::spawn(async move {
                        let _ = handle_ipc_client(st, stream).await;
                    });
                }
                Err(e) => {
                    warn!(error = %e, "IPC accept error");
                    break;
                }
            }
        }
    });

    Ok(IpcGuard { path: socket_path })
}

async fn handle_ipc_client(state: Arc<DaemonState>, stream: UnixStream) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    if line.trim().is_empty() {
        return Ok(());
    }

    let req: IpcRequest = match serde_json::from_str(line.trim()) {
        Ok(r) => r,
        Err(e) => {
            let resp = IpcResponse::Error {
                message: format!("invalid request: {e}"),
            };
            let mut out = serde_json::to_string(&resp)?;
            out.push('\n');
            write_half.write_all(out.as_bytes()).await?;
            return Ok(());
        }
    };

    let resp = match req {
        IpcRequest::Status => IpcResponse::Status(state.status_snapshot().await),
        IpcRequest::History { limit } => {
            let history = state.history.lock().await;
            let items = history
                .items()
                .take(limit.max(1))
                .map(|c| HistoryEntry {
                    meta: c.meta.clone(),
                    preview: c.preview(100),
                })
                .collect();
            IpcResponse::History { items }
        }
        IpcRequest::Latest => {
            let history = state.history.lock().await;
            match history.latest() {
                Some(clip) => IpcResponse::Latest {
                    meta: Some(clip.meta.clone()),
                    payload_b64: Some(encode_payload(&clip.payload)),
                },
                None => IpcResponse::Latest {
                    meta: None,
                    payload_b64: None,
                },
            }
        }
        IpcRequest::Push {
            mime_type,
            payload_b64,
        } => match decode_payload(&payload_b64) {
            Ok(payload) => match ClipItem::new(
                state.config.device_id.clone(),
                state.config.device_name.clone(),
                mime_type.clone(),
                payload.clone(),
            ) {
                Ok(item) => {
                    {
                        let mut h = state.history.lock().await;
                        h.insert(item.clone());
                    }
                    {
                        let mut guard = state.last_remote_hash.lock().await;
                        *guard = Some(item.meta.content_hash.clone());
                    }
                    let _ = write_system_clipboard(state.backend, &mime_type, &payload).await;
                    state.clips_sent.fetch_add(1, Ordering::Relaxed);
                    state.broadcast_clip(item.clone(), None).await;
                    IpcResponse::Ok {
                        message: format!("pushed {} ({} bytes)", item.meta.mime_type, item.meta.byte_len),
                    }
                }
                Err(e) => IpcResponse::Error { message: e },
            },
            Err(e) => IpcResponse::Error {
                message: e.to_string(),
            },
        },
        IpcRequest::Clear => {
            state.handle_history_clear(now_millis(), None).await;
            IpcResponse::Ok {
                message: "cleared in-memory clipboard history".into(),
            }
        }
    };

    let mut out = serde_json::to_string(&resp)?;
    out.push('\n');
    write_half.write_all(out.as_bytes()).await?;
    write_half.flush().await?;
    Ok(())
}
