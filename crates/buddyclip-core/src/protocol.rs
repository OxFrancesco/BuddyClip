use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::MAX_PAYLOAD_BYTES;
use crate::clipboard::{ClipItem, ClipMeta};

const TAG_IDENTIFY: u8 = 0x01;
const TAG_CLIP_PUSH: u8 = 0x02;
const TAG_CLIP_ACK: u8 = 0x03;
const TAG_SYNC_REQUEST: u8 = 0x04;
const TAG_PING: u8 = 0x05;
const TAG_PONG: u8 = 0x06;
const TAG_ERROR: u8 = 0x07;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("empty wire message")]
    EmptyMessage,
    #[error("unknown wire message tag: 0x{0:02x}")]
    UnknownTag(u8),
    #[error("truncated binary message")]
    Truncated,
    #[error("clip payload integrity mismatch")]
    IntegrityMismatch,
    #[error("clip payload exceeds max size ({0} bytes)")]
    PayloadTooLarge(usize),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerIdentity {
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
    pub protocol_version: u8,
    #[serde(default)]
    pub last_seen_clip_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncRequest {
    pub known_clip_ids: Vec<String>,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireMessage {
    Identify(PeerIdentity),
    ClipPush(ClipItem),
    ClipAck { clip_id: String },
    SyncRequest(SyncRequest),
    Ping { ts_ms: u64 },
    Pong { ts_ms: u64 },
    Error { message: String },
}

impl WireMessage {
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        match self {
            WireMessage::Identify(id) => encode_json(TAG_IDENTIFY, id),
            WireMessage::ClipPush(item) => {
                if item.payload.len() > MAX_PAYLOAD_BYTES {
                    return Err(ProtocolError::PayloadTooLarge(item.payload.len()));
                }
                let meta_bytes = serde_json::to_vec(&item.meta)?;
                let mut out = Vec::with_capacity(1 + 4 + meta_bytes.len() + item.payload.len());
                out.push(TAG_CLIP_PUSH);
                out.extend_from_slice(&(meta_bytes.len() as u32).to_be_bytes());
                out.extend_from_slice(&meta_bytes);
                out.extend_from_slice(&item.payload);
                Ok(out)
            }
            WireMessage::ClipAck { clip_id } => {
                #[derive(Serialize)]
                struct Ack<'a> {
                    clip_id: &'a str,
                }
                encode_json(TAG_CLIP_ACK, &Ack { clip_id })
            }
            WireMessage::SyncRequest(req) => encode_json(TAG_SYNC_REQUEST, req),
            WireMessage::Ping { ts_ms } => {
                let mut out = Vec::with_capacity(9);
                out.push(TAG_PING);
                out.extend_from_slice(&ts_ms.to_be_bytes());
                Ok(out)
            }
            WireMessage::Pong { ts_ms } => {
                let mut out = Vec::with_capacity(9);
                out.push(TAG_PONG);
                out.extend_from_slice(&ts_ms.to_be_bytes());
                Ok(out)
            }
            WireMessage::Error { message } => {
                #[derive(Serialize)]
                struct ErrBody<'a> {
                    message: &'a str,
                }
                encode_json(TAG_ERROR, &ErrBody { message })
            }
        }
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let (&tag, rest) = bytes.split_first().ok_or(ProtocolError::EmptyMessage)?;
        match tag {
            TAG_IDENTIFY => Ok(WireMessage::Identify(serde_json::from_slice(rest)?)),
            TAG_CLIP_PUSH => {
                if rest.len() < 4 {
                    return Err(ProtocolError::Truncated);
                }
                let meta_len = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
                let after_len = &rest[4..];
                if after_len.len() < meta_len {
                    return Err(ProtocolError::Truncated);
                }
                let meta: ClipMeta = serde_json::from_slice(&after_len[..meta_len])?;
                let payload = after_len[meta_len..].to_vec();
                if payload.len() > MAX_PAYLOAD_BYTES {
                    return Err(ProtocolError::PayloadTooLarge(payload.len()));
                }
                let item = ClipItem { meta, payload };
                if !item.verify_integrity() {
                    return Err(ProtocolError::IntegrityMismatch);
                }
                Ok(WireMessage::ClipPush(item))
            }
            TAG_CLIP_ACK => {
                #[derive(Deserialize)]
                struct Ack {
                    clip_id: String,
                }
                let a: Ack = serde_json::from_slice(rest)?;
                Ok(WireMessage::ClipAck { clip_id: a.clip_id })
            }
            TAG_SYNC_REQUEST => Ok(WireMessage::SyncRequest(serde_json::from_slice(rest)?)),
            TAG_PING => {
                if rest.len() != 8 {
                    return Err(ProtocolError::Truncated);
                }
                let ts_ms = u64::from_be_bytes(rest.try_into().unwrap());
                Ok(WireMessage::Ping { ts_ms })
            }
            TAG_PONG => {
                if rest.len() != 8 {
                    return Err(ProtocolError::Truncated);
                }
                let ts_ms = u64::from_be_bytes(rest.try_into().unwrap());
                Ok(WireMessage::Pong { ts_ms })
            }
            TAG_ERROR => {
                #[derive(Deserialize)]
                struct ErrBody {
                    message: String,
                }
                let e: ErrBody = serde_json::from_slice(rest)?;
                Ok(WireMessage::Error { message: e.message })
            }
            other => Err(ProtocolError::UnknownTag(other)),
        }
    }
}

fn encode_json<T: Serialize>(tag: u8, value: &T) -> Result<Vec<u8>, ProtocolError> {
    let json = serde_json::to_vec(value)?;
    let mut out = Vec::with_capacity(1 + json.len());
    out.push(tag);
    out.extend_from_slice(&json);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_clip_push_binary_image() {
        let fake_png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3, 4];
        let item = ClipItem::new("dev-1", "linux-pc", "image/png", fake_png.clone()).unwrap();
        let encoded = WireMessage::ClipPush(item.clone()).encode().unwrap();
        let decoded = WireMessage::decode(&encoded).unwrap();
        assert_eq!(decoded, WireMessage::ClipPush(item));
    }

    #[test]
    fn tampered_clip_payload_rejected() {
        let item = ClipItem::new_text("dev-1", "linux-pc", "secret clip").unwrap();
        let mut encoded = WireMessage::ClipPush(item).encode().unwrap();
        let last = encoded.len() - 1;
        encoded[last] ^= 0xFF;
        assert!(matches!(
            WireMessage::decode(&encoded),
            Err(ProtocolError::IntegrityMismatch)
        ));
    }
}
