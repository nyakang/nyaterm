//! Bounded append-only output with absolute UTF-8 byte cursors.
use std::collections::VecDeque;

use super::OutputStoreError;

pub struct StreamOutput {
    chunks: VecDeque<String>,
    bytes: usize,
    base_offset: usize,
    total_bytes: usize,
    limit: usize,
    source_truncated: bool,
}

pub struct StreamOutputChunk {
    pub data: String,
    pub offset: usize,
    pub next_offset: usize,
    pub base_offset: usize,
    pub total_bytes: usize,
    pub eof: bool,
    pub source_truncated: bool,
    pub dropped_before_offset: bool,
}

impl StreamOutput {
    pub fn new(limit: usize) -> Self {
        Self {
            chunks: VecDeque::new(),
            bytes: 0,
            base_offset: 0,
            total_bytes: 0,
            limit,
            source_truncated: false,
        }
    }
    pub fn append(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.total_bytes = self.total_bytes.saturating_add(text.len());
        self.bytes = self.bytes.saturating_add(text.len());
        // Bound both per-event allocation and deque metadata for tiny writes.
        let mut remaining = text;
        while !remaining.is_empty() {
            let mut end = remaining.len().min(8192);
            while !remaining.is_char_boundary(end) {
                end -= 1;
            }
            let part = &remaining[..end];
            if let Some(last) = self
                .chunks
                .back_mut()
                .filter(|chunk| chunk.len() + part.len() <= 8192)
            {
                last.push_str(part);
            } else {
                self.chunks.push_back(part.into());
            }
            remaining = &remaining[end..];
        }
        self.trim_to(self.limit);
    }
    pub fn retained_bytes(&self) -> usize {
        self.bytes
    }
    pub fn note_source_loss(&mut self) {
        self.source_truncated = true;
    }
    pub fn trim_to(&mut self, keep: usize) {
        while self.bytes > keep {
            let Some(mut front) = self.chunks.pop_front() else {
                break;
            };
            let mut drop = (self.bytes - keep).min(front.len());
            while drop < front.len() && !front.is_char_boundary(drop) {
                drop += 1;
            }
            self.bytes -= drop;
            self.base_offset += drop;
            self.source_truncated = true;
            if drop < front.len() {
                front.drain(..drop);
                self.chunks.push_front(front);
            }
        }
    }
    pub fn read(
        &self,
        offset: usize,
        max_bytes: usize,
    ) -> Result<StreamOutputChunk, OutputStoreError> {
        if offset > self.total_bytes {
            return Err(OutputStoreError::InvalidOffset);
        }
        let actual = offset.max(self.base_offset);
        let mut skip = actual - self.base_offset;
        let mut data = String::new();
        let budget = max_bytes.clamp(4, super::MAX_OUTPUT_CHUNK_BYTES);
        for chunk in &self.chunks {
            if skip >= chunk.len() {
                skip -= chunk.len();
                continue;
            }
            if !chunk.is_char_boundary(skip) {
                return Err(OutputStoreError::InvalidOffset);
            }
            let text = &chunk[skip..];
            skip = 0;
            let mut end = text.len().min(budget.saturating_sub(data.len()));
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            data.push_str(&text[..end]);
            if end < text.len() || data.len() == budget {
                break;
            }
        }
        let next_offset = actual + data.len();
        Ok(StreamOutputChunk {
            data,
            offset: actual,
            next_offset,
            base_offset: self.base_offset,
            total_bytes: self.total_bytes,
            eof: next_offset == self.total_bytes,
            source_truncated: self.source_truncated,
            dropped_before_offset: offset < self.base_offset,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::StreamOutput;
    #[test]
    fn absolute_cursors_survive_utf8_eviction_and_source_loss_is_separate_from_pagination() {
        let mut output = StreamOutput::new(9);
        output.append("猫猫");
        let first = output.read(0, 4).unwrap();
        assert_eq!(first.data, "猫");
        assert!(!first.eof && !first.source_truncated);
        output.append("猫猫");
        let lost = output.read(0, 4).unwrap();
        assert_eq!(
            (
                lost.offset,
                lost.next_offset,
                lost.base_offset,
                lost.total_bytes
            ),
            (3, 6, 3, 12)
        );
        assert!(lost.dropped_before_offset && lost.source_truncated);
        assert!(output.read(4, 4).is_err());
        assert!(output.read(13, 4).is_err());
        assert!(output.read(12, 4).unwrap().eof);
        output.trim_to(0);
        assert_eq!(output.retained_bytes(), 0);
        assert_eq!(output.read(0, 4).unwrap().offset, 12);
    }
}
