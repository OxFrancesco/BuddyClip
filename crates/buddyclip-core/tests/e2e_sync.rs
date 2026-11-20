use std::time::Duration;

use buddyclip_core::PROTOCOL_VERSION;
use buddyclip_core::clipboard::{ClipItem, ClipRingBuffer};
use buddyclip_core::client::{ClientConfig, ClientEvent, spawn_sync_client};
use buddyclip_core::crypto::{MasterKey, server_handshake};
use buddyclip_core::protocol::{PeerIdentity, WireMessage};
use buddyclip_core::tailnet::TailnetPolicy;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

#[tokio::test]
async fn end_to_end_encrypted_client_server_sync_and_replay() {
    let key = MasterKey::generate();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Seed server with an existing clip (simulating PC copied text before phone connected)
    let initial_pc_clip = ClipItem::new_text("pc-dev-1", "linux-pc", "Copied on PC before connect").unwrap();
    let initial_clip_clone = initial_pc_clip.clone();
    let server_key = key.clone();

    let (server_rx_tx, mut server_rx_rx) = mpsc::unbounded_channel::<ClipItem>();

    let server_task = tokio::spawn(async move {
        let mut ring = ClipRingBuffer::default();
        ring.insert(initial_clip_clone);

        let (stream, _) = listener.accept().await.unwrap();
        let (mut reader, mut writer) = server_handshake(stream, &server_key).await.unwrap();

        // Send server identity
        let srv_id = WireMessage::Identify(PeerIdentity {
            device_id: "pc-dev-1".into(),
            device_name: "linux-pc".into(),
            platform: "linux".into(),
            protocol_version: PROTOCOL_VERSION,
            last_seen_clip_id: ring.latest().map(|c| c.meta.id.clone()),
        });
        writer.write_frame(&srv_id.encode().unwrap()).await.unwrap();

        // Read client identity
        let frame = reader.read_frame().await.unwrap();
        match WireMessage::decode(&frame).unwrap() {
            WireMessage::Identify(id) => assert_eq!(id.device_name, "Android Phone"),
            other => panic!("expected Identify, got {other:?}"),
        }

        // Handle SyncRequest and incoming phone ClipPush
        for _ in 0..4 {
            let f = match tokio::time::timeout(Duration::from_secs(3), reader.read_frame()).await {
                Ok(Ok(bytes)) => bytes,
                _ => break,
            };
            match WireMessage::decode(&f).unwrap() {
                WireMessage::SyncRequest(_) => {
                    for item in ring.items().cloned().collect::<Vec<_>>().into_iter().rev() {
                        writer
                            .write_frame(&WireMessage::ClipPush(item).encode().unwrap())
                            .await
                            .unwrap();
                    }
                }
                WireMessage::ClipPush(item) => {
                    let ack = WireMessage::ClipAck {
                        clip_id: item.meta.id.clone(),
                    };
                    writer.write_frame(&ack.encode().unwrap()).await.unwrap();
                    let _ = server_rx_tx.send(item);
                }
                WireMessage::ClipAck { .. } => {}
                other => panic!("unexpected wire message: {other:?}"),
            }
        }
    });

    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    let handle = spawn_sync_client(
        ClientConfig {
            host: "127.0.0.1".into(),
            port: addr.port(),
            key: key.clone(),
            device_id: "phone-dev-1".into(),
            device_name: "Android Phone".into(),
            platform: "android".into(),
            policy: TailnetPolicy::AllowLoopbackForTests,
        },
        ev_tx,
    );

    // 1. Expect Connected event + replayed PC clip
    let mut got_connected = false;
    let mut got_pc_clip = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while (!got_connected || !got_pc_clip) && tokio::time::Instant::now() < deadline {
        if let Ok(Some(ev)) = tokio::time::timeout(Duration::from_secs(2), ev_rx.recv()).await {
            match ev {
                ClientEvent::Connected {
                    peer_name,
                    key_fingerprint,
                    ..
                } => {
                    assert_eq!(peer_name, "linux-pc");
                    assert_eq!(key_fingerprint, key.fingerprint());
                    got_connected = true;
                }
                ClientEvent::ClipReceived(item) => {
                    assert_eq!(item.as_text(), Some("Copied on PC before connect"));
                    assert_eq!(item.meta.origin_device_name, "linux-pc");
                    got_pc_clip = true;
                }
                _ => {}
            }
        }
    }
    assert!(got_connected, "client did not connect");
    assert!(got_pc_clip, "client did not receive initial PC clip on sync");

    // 2. Push a binary PNG image from Android phone -> PC
    let png_bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 42, 99, 128, 255];
    let phone_clip = ClipItem::new("phone-dev-1", "Android Phone", "image/png", png_bytes.clone()).unwrap();
    handle.push_clip(phone_clip).await.unwrap();

    let received_by_pc = tokio::time::timeout(Duration::from_secs(3), server_rx_rx.recv())
        .await
        .expect("timed out waiting for server to receive phone clip")
        .expect("server channel closed");
    assert_eq!(received_by_pc.meta.mime_type, "image/png");
    assert_eq!(received_by_pc.payload, png_bytes);
    assert_eq!(received_by_pc.meta.origin_device_name, "Android Phone");

    handle.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), server_task).await;
}

#[tokio::test]
async fn strict_tailnet_client_refuses_non_tailnet_destination() {
    let key = MasterKey::generate();
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    let handle = spawn_sync_client(
        ClientConfig {
            host: "192.168.1.50".into(),
            port: 9455,
            key,
            device_id: "phone-1".into(),
            device_name: "Android Phone".into(),
            platform: "android".into(),
            policy: TailnetPolicy::Strict,
        },
        ev_tx,
    );

    let mut rejected = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while !rejected && tokio::time::Instant::now() < deadline {
        if let Ok(Some(ClientEvent::Disconnected { reason })) =
            tokio::time::timeout(Duration::from_secs(2), ev_rx.recv()).await
            && reason.contains("outside the Tailscale tailnet")
        {
            rejected = true;
        }
    }
    handle.shutdown();
    assert!(rejected, "strict policy must refuse non-tailnet LAN IP");
}
