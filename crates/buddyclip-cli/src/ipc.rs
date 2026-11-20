use std::path::Path;

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use buddyclip_core::clipboard::ClipMeta;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum IpcRequest {
    Status,
    History { limit: usize },
    Latest,
    Push { mime_type: String, payload_b64: String },
    Clear,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectedPeerInfo {
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
    pub peer_addr: String,
    pub connected_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub device_id: String,
    pub device_name: String,
    pub listen_addr: String,
    pub key_fingerprint: String,
    pub backend: String,
    pub connected_peers: Vec<ConnectedPeerInfo>,
    pub history_count: usize,
    pub clips_sent: u64,
    pub clips_received: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub meta: ClipMeta,
    pub preview: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcResponse {
    Status(DaemonStatus),
    History { items: Vec<HistoryEntry> },
    Latest { meta: Option<ClipMeta>, payload_b64: Option<String> },
    Ok { message: String },
    Error { message: String },
}

pub fn encode_payload(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

pub fn decode_payload(b64: &str) -> Result<Vec<u8>> {
    STANDARD.decode(b64).context("invalid base64 payload")
}

pub async fn send_ipc_request(socket_path: &Path, req: &IpcRequest) -> Result<IpcResponse> {
    let stream = UnixStream::connect(socket_path).await.with_context(|| {
        format!(
            "buddyclip daemon is not running (could not connect to {}). Start it with `buddyclip daemon`.",
            socket_path.display()
        )
    })?;

    let (read_half, mut write_half) = stream.into_split();
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    write_half.write_all(line.as_bytes()).await?;
    write_half.flush().await?;

    let mut reader = BufReader::new(read_half);
    let mut resp_line = String::new();
    reader.read_line(&mut resp_line).await?;
    if resp_line.is_empty() {
        anyhow::bail!("daemon closed IPC connection without response");
    }
    let resp: IpcResponse = serde_json::from_str(resp_line.trim())?;
    Ok(resp)
}
