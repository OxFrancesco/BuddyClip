use std::collections::{HashSet, VecDeque};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{DEFAULT_HISTORY_COUNT, DEFAULT_HISTORY_MAX_BYTES, MAX_PAYLOAD_BYTES, now_millis};

pub const MIME_TEXT_PLAIN: &str = "text/plain;charset=utf-8";
pub const MIME_IMAGE_PNG: &str = "image/png";
pub const MIME_IMAGE_JPEG: &str = "image/jpeg";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClipMeta {
    pub id: String,
    pub created_at_ms: u64,
    pub origin_device_id: String,
    pub origin_device_name: String,
    pub mime_type: String,
    pub content_hash: String,
    pub byte_len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipItem {
    pub meta: ClipMeta,
    pub payload: Vec<u8>,
}

impl ClipItem {
    pub fn new(
        origin_device_id: impl Into<String>,
        origin_device_name: impl Into<String>,
        mime_type: impl Into<String>,
        payload: Vec<u8>,
    ) -> Result<Self, String> {
        if payload.is_empty() {
            return Err("empty clipboard payload".into());
        }
        if payload.len() > MAX_PAYLOAD_BYTES {
            return Err(format!(
                "clipboard payload exceeds limit ({} > {} bytes)",
                payload.len(),
                MAX_PAYLOAD_BYTES
            ));
        }

        let mime_type = normalize_mime(mime_type.into());
        let content_hash = compute_content_hash(&mime_type, &payload);
        let byte_len = payload.len();

        Ok(Self {
            meta: ClipMeta {
                id: Uuid::new_v4().to_string(),
                created_at_ms: now_millis(),
                origin_device_id: origin_device_id.into(),
                origin_device_name: origin_device_name.into(),
                mime_type,
                content_hash,
                byte_len,
            },
            payload,
        })
    }

    pub fn new_text(
        origin_device_id: impl Into<String>,
        origin_device_name: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<Self, String> {
        Self::new(
            origin_device_id,
            origin_device_name,
            MIME_TEXT_PLAIN,
            text.into().into_bytes(),
        )
    }

    pub fn is_text(&self) -> bool {
        self.meta.mime_type.starts_with("text/")
    }

    pub fn is_image(&self) -> bool {
        self.meta.mime_type.starts_with("image/")
    }

    pub fn as_text(&self) -> Option<&str> {
        if self.is_text() {
            std::str::from_utf8(&self.payload).ok()
        } else {
            None
        }
    }

    pub fn preview(&self, max_chars: usize) -> String {
        if let Some(text) = self.as_text() {
            let single_line: String = text
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            let trimmed = single_line.trim();
            if trimmed.chars().count() > max_chars {
                let mut out: String = trimmed.chars().take(max_chars).collect();
                out.push('…');
                out
            } else {
                trimmed.to_string()
            }
        } else if self.is_image() {
            format!("[{} image, {:.1} KB]", self.meta.mime_type, self.payload.len() as f64 / 1024.0)
        } else {
            format!("[{}, {} bytes]", self.meta.mime_type, self.payload.len())
        }
    }

    pub fn verify_integrity(&self) -> bool {
        self.meta.byte_len == self.payload.len()
            && self.meta.content_hash == compute_content_hash(&self.meta.mime_type, &self.payload)
    }
}

pub fn normalize_mime(mime: String) -> String {
    let lower = mime.trim().to_ascii_lowercase();
    if lower == "text/plain" || lower == "utf8_string" || lower == "text/plain; charset=utf-8" {
        MIME_TEXT_PLAIN.to_string()
    } else if lower == "image/jpg" {
        MIME_IMAGE_JPEG.to_string()
    } else {
        lower
    }
}

pub fn compute_content_hash(mime_type: &str, payload: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"buddyclip-v1-content:");
    hasher.update(mime_type.as_bytes());
    hasher.update(&[0u8]);
    hasher.update(payload);
    hasher.finalize().to_hex().to_string()
}

/// RAM-only clipboard ring buffer with deduplication by clip ID and bounded memory usage.
#[derive(Debug)]
pub struct ClipRingBuffer {
    items: VecDeque<ClipItem>,
    seen_ids: HashSet<String>,
    total_bytes: usize,
    max_items: usize,
    max_bytes: usize,
    cleared_before_ms: u64,
}

impl Default for ClipRingBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_HISTORY_COUNT, DEFAULT_HISTORY_MAX_BYTES)
    }
}

impl ClipRingBuffer {
    pub fn new(max_items: usize, max_bytes: usize) -> Self {
        Self {
            items: VecDeque::with_capacity(max_items),
            seen_ids: HashSet::new(),
            total_bytes: 0,
            max_items: max_items.max(1),
            max_bytes: max_bytes.max(1024),
            cleared_before_ms: 0,
        }
    }

    pub fn with_cleared_before(mut self, cleared_before_ms: u64) -> Self {
        self.cleared_before_ms = cleared_before_ms;
        self
    }

    pub fn cleared_before_ms(&self) -> u64 {
        self.cleared_before_ms
    }

    /// Inserts a clip if its ID has not been seen yet and it is newer than the clear watermark.
    /// Returns `true` if inserted (new clip), `false` if duplicate or cleared.
    pub fn insert(&mut self, item: ClipItem) -> bool {
        if item.meta.created_at_ms <= self.cleared_before_ms {
            return false;
        }
        if self.seen_ids.contains(&item.meta.id) {
            return false;
        }
        // Also collapse consecutive identical content from the latest entry so we don't spam history
        if let Some(latest) = self.items.front()
            && latest.meta.content_hash == item.meta.content_hash
        {
            self.seen_ids.insert(item.meta.id);
            return false;
        }

        self.total_bytes += item.payload.len();
        self.seen_ids.insert(item.meta.id.clone());
        self.items.push_front(item);

        while self.items.len() > self.max_items
            || (self.items.len() > 1 && self.total_bytes > self.max_bytes)
        {
            if let Some(evicted) = self.items.pop_back() {
                self.total_bytes = self.total_bytes.saturating_sub(evicted.payload.len());
                self.seen_ids.remove(&evicted.meta.id);
            }
        }
        true
    }

    pub fn latest(&self) -> Option<&ClipItem> {
        self.items.front()
    }

    pub fn has_id(&self, id: &str) -> bool {
        self.seen_ids.contains(id)
    }

    pub fn items(&self) -> impl Iterator<Item = &ClipItem> {
        self.items.iter()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn clear(&mut self) {
        self.clear_up_to(now_millis());
    }

    pub fn clear_up_to(&mut self, cleared_at_ms: u64) {
        if cleared_at_ms > self.cleared_before_ms {
            self.cleared_before_ms = cleared_at_ms;
        }
        self.items
            .retain(|item| item.meta.created_at_ms > self.cleared_before_ms);
        self.seen_ids = self.items.iter().map(|i| i.meta.id.clone()).collect();
        self.total_bytes = self.items.iter().map(|i| i.payload.len()).sum();
    }
}
