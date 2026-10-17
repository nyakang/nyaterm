use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

const MAX_LINES: usize = 500;
const MAX_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone)]
struct StoredLine {
    text: String,
    start_cursor: u64,
    end_cursor: u64,
}

#[derive(Debug)]
struct SessionOutput {
    lines: VecDeque<StoredLine>,
    partial: String,
    partial_start_cursor: u64,
    line_bytes: usize,
    tail_cursor: u64,
    retained_start_cursor: u64,
    closed: bool,
    notify: Arc<Notify>,
}

impl Default for SessionOutput {
    fn default() -> Self {
        Self {
            lines: VecDeque::new(),
            partial: String::new(),
            partial_start_cursor: 0,
            line_bytes: 0,
            tail_cursor: 0,
            retained_start_cursor: 0,
            closed: false,
            notify: Arc::new(Notify::new()),
        }
    }
}

impl SessionOutput {
    fn append(&mut self, text: &str) {
        if text.is_empty() || self.closed {
            return;
        }

        let clean = strip_ansi_escapes::strip_str(text).replace('\r', "");
        if clean.is_empty() {
            return;
        }

        let mut rest = clean.as_str();
        while let Some(index) = rest.find('\n') {
            let before = &rest[..index];
            self.partial.push_str(before);
            self.tail_cursor = self
                .tail_cursor
                .saturating_add(before.len() as u64)
                .saturating_add(1);
            let text = std::mem::take(&mut self.partial);
            let start_cursor = self.partial_start_cursor;
            let end_cursor = self.tail_cursor;
            self.line_bytes = self.line_bytes.saturating_add(text.len() + 1);
            self.lines.push_back(StoredLine {
                text,
                start_cursor,
                end_cursor,
            });
            self.partial_start_cursor = end_cursor;
            rest = &rest[index + 1..];
        }

        if !rest.is_empty() {
            self.partial.push_str(rest);
            self.tail_cursor = self.tail_cursor.saturating_add(rest.len() as u64);
        }

        self.trim();
    }

    fn trim(&mut self) {
        while self.lines.len() > MAX_LINES {
            self.pop_front_line();
        }

        while self.line_bytes.saturating_add(self.partial.len()) > MAX_BYTES {
            if let Some(front) = self.lines.front() {
                let serialized_len = front.text.len() + 1;
                if self.lines.len() > 1 || serialized_len <= MAX_BYTES {
                    self.pop_front_line();
                    continue;
                }

                let keep_text_bytes = MAX_BYTES.saturating_sub(1);
                let drop_bytes = front.text.len().saturating_sub(keep_text_bytes);
                if drop_bytes > 0 {
                    let front = self.lines.front_mut().expect("front line");
                    let split_at = byte_boundary_at_or_after(&front.text, drop_bytes);
                    front.text.drain(..split_at);
                    front.start_cursor = front.start_cursor.saturating_add(split_at as u64);
                    self.line_bytes = front.text.len() + 1;
                    self.retained_start_cursor = front.start_cursor;
                    continue;
                }
            }

            let overflow = self
                .line_bytes
                .saturating_add(self.partial.len())
                .saturating_sub(MAX_BYTES);
            if overflow == 0 || self.partial.is_empty() {
                break;
            }
            let split_at = byte_boundary_at_or_after(&self.partial, overflow);
            self.partial.drain(..split_at);
            self.partial_start_cursor = self.partial_start_cursor.saturating_add(split_at as u64);
            self.retained_start_cursor = self.partial_start_cursor;
        }

        self.retained_start_cursor = self
            .lines
            .front()
            .map(|line| line.start_cursor)
            .unwrap_or(self.partial_start_cursor);
    }

    fn pop_front_line(&mut self) {
        if let Some(line) = self.lines.pop_front() {
            self.line_bytes = self.line_bytes.saturating_sub(line.text.len() + 1);
            self.retained_start_cursor = line.end_cursor;
        }
    }

    fn read(&self, lines: usize) -> String {
        let count = lines.clamp(1, MAX_LINES);
        let mut all: Vec<&str> = self.lines.iter().map(|line| line.text.as_str()).collect();
        if !self.partial.is_empty() {
            all.push(&self.partial);
        }
        let start = all.len().saturating_sub(count);
        all[start..].join("\n")
    }

    fn read_since(&self, cursor: u64) -> Result<RecentOutputSnapshot, RecentOutputReadError> {
        if self.closed {
            return Ok(RecentOutputSnapshot {
                text: String::new(),
                tail_cursor: self.tail_cursor,
                retained_start_cursor: self.retained_start_cursor,
                closed: true,
            });
        }
        if cursor < self.retained_start_cursor {
            return Err(RecentOutputReadError::Overrun {
                requested_cursor: cursor,
                retained_start_cursor: self.retained_start_cursor,
            });
        }
        if cursor > self.tail_cursor {
            return Err(RecentOutputReadError::InvalidCursor {
                requested_cursor: cursor,
                tail_cursor: self.tail_cursor,
            });
        }

        let mut text = String::new();
        for line in &self.lines {
            if line.end_cursor <= cursor {
                continue;
            }
            let mut serialized = line.text.clone();
            serialized.push('\n');
            let offset = cursor.saturating_sub(line.start_cursor) as usize;
            if offset < serialized.len() {
                text.push_str(&serialized[offset..]);
            }
        }
        if self.tail_cursor > cursor && !self.partial.is_empty() {
            let offset = cursor.saturating_sub(self.partial_start_cursor) as usize;
            if offset < self.partial.len() {
                text.push_str(&self.partial[offset..]);
            }
        }

        Ok(RecentOutputSnapshot {
            text,
            tail_cursor: self.tail_cursor,
            retained_start_cursor: self.retained_start_cursor,
            closed: self.closed,
        })
    }
}

fn byte_boundary_at_or_after(text: &str, requested: usize) -> usize {
    let mut index = requested.min(text.len());
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentOutputSnapshot {
    pub text: String,
    pub tail_cursor: u64,
    pub retained_start_cursor: u64,
    pub closed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecentOutputReadError {
    SessionNotFound(String),
    Overrun {
        requested_cursor: u64,
        retained_start_cursor: u64,
    },
    InvalidCursor {
        requested_cursor: u64,
        tail_cursor: u64,
    },
}

impl fmt::Display for RecentOutputReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SessionNotFound(id) => write!(f, "Recent output session '{id}' not found"),
            Self::Overrun {
                requested_cursor,
                retained_start_cursor,
            } => write!(
                f,
                "Recent output cursor overrun: requested {requested_cursor}, retained output starts at {retained_start_cursor}"
            ),
            Self::InvalidCursor {
                requested_cursor,
                tail_cursor,
            } => write!(
                f,
                "Recent output cursor {requested_cursor} is beyond tail cursor {tail_cursor}"
            ),
        }
    }
}

impl std::error::Error for RecentOutputReadError {}

#[derive(Default)]
pub struct RecentOutputStore {
    sessions: Mutex<HashMap<String, SessionOutput>>,
}

impl RecentOutputStore {
    pub fn open(&self, session_id: &str) {
        let mut sessions = self.sessions.lock().unwrap();
        sessions.insert(session_id.to_string(), SessionOutput::default());
    }

    pub fn append(&self, session_id: &str, text: &str) {
        let notify = {
            let mut sessions = self.sessions.lock().unwrap();
            let output = sessions.entry(session_id.to_string()).or_default();
            output.append(text);
            output.notify.clone()
        };
        notify.notify_waiters();
    }

    pub fn read(&self, session_id: &str, lines: usize) -> String {
        self.sessions
            .lock()
            .unwrap()
            .get(session_id)
            .map(|output| output.read(lines))
            .unwrap_or_default()
    }

    pub fn tail_cursor(&self, session_id: &str) -> Result<u64, RecentOutputReadError> {
        self.sessions
            .lock()
            .unwrap()
            .get(session_id)
            .map(|output| output.tail_cursor)
            .ok_or_else(|| RecentOutputReadError::SessionNotFound(session_id.to_string()))
    }

    pub fn read_since(
        &self,
        session_id: &str,
        cursor: u64,
    ) -> Result<RecentOutputSnapshot, RecentOutputReadError> {
        self.sessions
            .lock()
            .unwrap()
            .get(session_id)
            .ok_or_else(|| RecentOutputReadError::SessionNotFound(session_id.to_string()))?
            .read_since(cursor)
    }

    pub fn notification(&self, session_id: &str) -> Result<Arc<Notify>, RecentOutputReadError> {
        self.sessions
            .lock()
            .unwrap()
            .get(session_id)
            .map(|output| output.notify.clone())
            .ok_or_else(|| RecentOutputReadError::SessionNotFound(session_id.to_string()))
    }

    pub fn remove(&self, session_id: &str) {
        let notify = {
            let mut sessions = self.sessions.lock().unwrap();
            let Some(output) = sessions.get_mut(session_id) else {
                return;
            };
            output.closed = true;
            output.lines.clear();
            output.partial.clear();
            output.line_bytes = 0;
            output.partial_start_cursor = output.tail_cursor;
            output.retained_start_cursor = output.tail_cursor;
            output.notify.clone()
        };
        notify.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_BYTES, MAX_LINES, RecentOutputReadError, RecentOutputStore};

    #[test]
    fn strips_ansi_and_bounds_lines() {
        let store = RecentOutputStore::default();
        store.open("s");
        for i in 0..(MAX_LINES + 20) {
            store.append("s", &format!("\x1b[31mline-{i}\x1b[0m\n"));
        }
        let output = store.read("s", MAX_LINES + 20);
        assert!(!output.contains("\x1b"));
        assert_eq!(output.lines().count(), MAX_LINES);
        assert!(output.contains("line-519"));
        assert!(!output.contains("line-0\n"));
    }

    #[test]
    fn bounds_single_oversized_line() {
        let store = RecentOutputStore::default();
        store.open("s");
        store.append("s", &format!("{}TAIL\n", "x".repeat(MAX_BYTES + 1024)));
        let output = store.read("s", 10);
        assert!(output.len() <= MAX_BYTES);
        assert!(output.ends_with("TAIL"));
    }

    #[test]
    fn cursor_reads_only_unread_output_and_advances_by_match_end() {
        let store = RecentOutputStore::default();
        store.open("s");
        store.append("s", "old prompt\n");
        let cursor = store.tail_cursor("s").unwrap();
        store.append("s", "fast response\nnext");

        let unread = store.read_since("s", cursor).unwrap();
        assert_eq!(unread.text, "fast response\nnext");
        let match_end = unread.text.find("response").unwrap() + "response".len();
        let after = store.read_since("s", cursor + match_end as u64).unwrap();
        assert_eq!(after.text, "\nnext");
    }

    #[test]
    fn reports_cursor_overrun_after_retention_trim() {
        let store = RecentOutputStore::default();
        store.open("s");
        let cursor = store.tail_cursor("s").unwrap();
        store.append("s", &format!("{}TAIL", "x".repeat(MAX_BYTES + 128)));
        let error = store.read_since("s", cursor).unwrap_err();
        assert!(matches!(error, RecentOutputReadError::Overrun { .. }));
    }

    #[tokio::test]
    async fn remove_marks_closed_and_wakes_waiter() {
        let store = RecentOutputStore::default();
        store.open("s");
        let notify = store.notification("s").unwrap();
        let waiting = notify.notified();
        store.remove("s");
        waiting.await;
        assert!(store.read_since("s", 0).unwrap().closed);
    }
}
