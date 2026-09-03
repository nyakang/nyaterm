use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::error::{AppError, AppResult};

const ENTRY_LIMIT: usize = 4 * 1024 * 1024;
const TOTAL_LIMIT: usize = 16 * 1024 * 1024;
const TTL: Duration = Duration::from_secs(10 * 60);

struct Entry {
    bytes: Vec<u8>,
    total_bytes: usize,
    created: Instant,
}

#[derive(Default)]
pub struct OutputStore {
    entries: HashMap<String, Entry>,
    lru: VecDeque<String>,
    bytes: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtectedOutput {
    pub preview: String,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_id: Option<String>,
    pub total_bytes: usize,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub source_truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputChunk {
    pub data: String,
    pub offset: usize,
    pub next_offset: usize,
    pub total_bytes: usize,
    pub eof: bool,
}

impl OutputStore {
    pub fn protect(&mut self, text: String, inline_limit: usize) -> ProtectedOutput {
        self.cleanup();
        let total_bytes = text.len();
        if total_bytes <= inline_limit {
            return ProtectedOutput {
                preview: text,
                truncated: false,
                output_id: None,
                total_bytes,
                source_truncated: false,
            };
        }
        let preview_end = boundary_at_or_before(&text, inline_limit);
        let preview = text[..preview_end].to_string();
        let keep_end = boundary_at_or_before(&text, ENTRY_LIMIT.min(text.len()));
        let output_id = format!("out_{}", uuid::Uuid::new_v4());
        self.insert(
            output_id.clone(),
            text.as_bytes()[..keep_end].to_vec(),
            total_bytes,
        );
        ProtectedOutput {
            preview,
            truncated: true,
            output_id: Some(output_id),
            total_bytes,
            source_truncated: keep_end < total_bytes,
        }
    }

    pub fn read(
        &mut self,
        output_id: &str,
        offset: usize,
        max_bytes: usize,
    ) -> AppResult<OutputChunk> {
        self.cleanup();
        let entry = self
            .entries
            .get(output_id)
            .ok_or_else(|| AppError::Config("Output is unavailable or has expired.".to_string()))?;
        if offset > entry.bytes.len() || !std::str::from_utf8(&entry.bytes).is_ok() {
            return Err(AppError::Config("Invalid output offset.".to_string()));
        }
        let text = std::str::from_utf8(&entry.bytes).unwrap();
        if !text.is_char_boundary(offset) {
            return Err(AppError::Config("Invalid output offset.".to_string()));
        }
        let end = boundary_at_or_before(
            text,
            offset
                .saturating_add(max_bytes.clamp(1, 64 * 1024))
                .min(text.len()),
        );
        let chunk = OutputChunk {
            data: text[offset..end].to_string(),
            offset,
            next_offset: end,
            total_bytes: entry.total_bytes,
            eof: end == entry.bytes.len(),
        };
        self.lru.retain(|id| id != output_id);
        self.lru.push_back(output_id.to_string());
        Ok(chunk)
    }

    fn insert(&mut self, id: String, bytes: Vec<u8>, total_bytes: usize) {
        while self.bytes + bytes.len() > TOTAL_LIMIT {
            let Some(oldest) = self.lru.pop_front() else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(entry.bytes.len());
            }
        }
        self.bytes += bytes.len();
        self.lru.push_back(id.clone());
        self.entries.insert(
            id,
            Entry {
                bytes,
                total_bytes,
                created: Instant::now(),
            },
        );
    }

    pub fn cleanup(&mut self) {
        let expired = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.created.elapsed() >= TTL)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in expired {
            if let Some(entry) = self.entries.remove(&id) {
                self.bytes = self.bytes.saturating_sub(entry.bytes.len());
            }
        }
        self.lru.retain(|id| self.entries.contains_key(id));
    }
}

fn boundary_at_or_before(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protects_and_reads_utf8_chunks() {
        let mut store = OutputStore::default();
        let protected = store.protect("猫".repeat(30_000), 1024);
        assert!(protected.truncated);
        let chunk = store
            .read(protected.output_id.as_deref().unwrap(), 0, 1024)
            .unwrap();
        assert!(!chunk.data.is_empty());
        assert!(chunk.next_offset <= 1024);
    }

    #[test]
    fn keeps_the_inline_boundary_and_isolates_store_ids() {
        let mut first = OutputStore::default();
        let inline = first.protect("x".repeat(1024), 1024);
        assert!(!inline.truncated);
        assert!(inline.output_id.is_none());

        let protected = first.protect("x".repeat(1025), 1024);
        let id = protected.output_id.unwrap();
        let mut second = OutputStore::default();
        assert!(second.read(&id, 0, 128).is_err());
        assert_eq!(first.read(&id, 0, 128).unwrap().next_offset, 128);
    }

    #[test]
    fn evicts_the_least_recently_used_entry_at_the_connection_limit() {
        let mut store = OutputStore::default();
        let mut ids = Vec::new();
        for marker in ['a', 'b', 'c', 'd', 'e'] {
            ids.push(
                store
                    .protect(marker.to_string().repeat(ENTRY_LIMIT + 1), 1)
                    .output_id
                    .unwrap(),
            );
        }
        assert!(store.read(&ids[0], 0, 1).is_err());
        assert_eq!(store.read(&ids[4], 0, 1).unwrap().data, "e");
    }
}
