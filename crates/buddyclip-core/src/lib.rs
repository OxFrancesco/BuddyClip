pub mod clipboard;
pub mod client;
pub mod crypto;
pub mod pairing;
pub mod protocol;
pub mod tailnet;

pub const DEFAULT_PORT: u16 = 9455;
pub const PROTOCOL_VERSION: u8 = 1;
/// Maximum payload size per clipboard item (16 MiB, plenty for PNG screenshots).
pub const MAX_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;
/// Maximum encrypted wire frame size (payload + metadata + AEAD overhead).
pub const MAX_FRAME_BYTES: usize = MAX_PAYLOAD_BYTES + 64 * 1024;
/// Default number of recent clips retained in RAM.
pub const DEFAULT_HISTORY_COUNT: usize = 25;
/// Default RAM budget for the in-memory clipboard ring buffer (64 MiB).
pub const DEFAULT_HISTORY_MAX_BYTES: usize = 64 * 1024 * 1024;

pub fn now_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
