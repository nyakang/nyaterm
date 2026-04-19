use crate::error::{AppError, AppResult};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::mem;
use std::path::PathBuf;
use std::sync::Mutex;
use time::OffsetDateTime;

struct RecordingState {
    writer: BufWriter<File>,
    file_path: PathBuf,
    input_buffer: String,
    output_buffer: String,
    live_echo_buffer: String,
    submitted_line_echo: Option<String>,
    suppress_next_newline: bool,
}

impl RecordingState {
    fn new(file: File, file_path: PathBuf) -> Self {
        Self {
            writer: BufWriter::new(file),
            file_path,
            input_buffer: String::new(),
            output_buffer: String::new(),
            live_echo_buffer: String::new(),
            submitted_line_echo: None,
            suppress_next_newline: false,
        }
    }

    fn write_record(&mut self, label: &str, data: &str) {
        if data.is_empty() {
            return;
        }
        let timestamp = chrono_timestamp();
        let _ = writeln!(self.writer, "[{timestamp}] [{label}] {data}");
    }

    fn write_input(&mut self, data: &[u8]) {
        let text = String::from_utf8_lossy(data);

        for ch in text.chars() {
            match ch {
                '\r' | '\n' => self.commit_input_line(),
                '\u{8}' | '\u{7f}' => self.handle_backspace(),
                '\t' => {
                    self.input_buffer.push('\t');
                    self.live_echo_buffer.push('\t');
                }
                c if !c.is_control() => {
                    self.input_buffer.push(c);
                    self.live_echo_buffer.push(c);
                }
                _ => {}
            }
        }
    }

    fn write_output(&mut self, data: &str) {
        let mut sanitized = strip_terminal_control_sequences(data);
        if sanitized.is_empty() {
            return;
        }

        if self.suppress_next_newline {
            sanitized = strip_one_leading_newline(&sanitized).to_string();
            self.suppress_next_newline = false;
            if sanitized.is_empty() {
                return;
            }
        }

        sanitized = self.consume_live_echo(&sanitized);
        if sanitized.is_empty() {
            return;
        }

        let (mut sanitized, consumed_submitted_echo) = self.consume_submitted_echo(&sanitized);
        if sanitized.is_empty() {
            return;
        }

        if !consumed_submitted_echo && self.submitted_line_echo.is_some() {
            sanitized = strip_one_leading_newline(&sanitized).to_string();
            self.submitted_line_echo = None;
            if sanitized.is_empty() {
                return;
            }
        }

        self.output_buffer.push_str(&sanitized);
        self.flush_output_lines(false);
    }

    fn finish(&mut self) {
        self.commit_partial_input();
        self.flush_output_lines(true);
        let _ = self.writer.flush();
    }

    fn handle_backspace(&mut self) {
        if let Some(removed) = self.input_buffer.pop() {
            if self.live_echo_buffer.ends_with(removed) {
                self.live_echo_buffer.pop();
            }
        }
    }

    fn commit_input_line(&mut self) {
        self.flush_output_lines(true);
        let line = mem::take(&mut self.input_buffer);
        self.live_echo_buffer.clear();

        if line.trim().is_empty() {
            self.submitted_line_echo = None;
            return;
        }

        self.write_record("INPUT", &line);
        self.submitted_line_echo = Some(line);
    }

    fn commit_partial_input(&mut self) {
        self.flush_output_lines(true);
        let line = mem::take(&mut self.input_buffer);
        self.live_echo_buffer.clear();
        self.submitted_line_echo = None;

        if line.trim().is_empty() {
            return;
        }

        self.write_record("INPUT", &line);
    }

    fn consume_live_echo(&mut self, text: &str) -> String {
        let consumed = consume_matching_prefix(&mut self.live_echo_buffer, text);
        text[consumed..].to_string()
    }

    fn consume_submitted_echo(&mut self, text: &str) -> (String, bool) {
        let Some(line) = self.submitted_line_echo.as_ref() else {
            return (text.to_string(), false);
        };

        if !text.starts_with(line) {
            return (text.to_string(), false);
        }

        let mut remaining = text[line.len()..].to_string();
        self.submitted_line_echo = None;

        let stripped = strip_one_leading_newline(&remaining);
        if stripped.len() != remaining.len() {
            remaining = stripped.to_string();
        } else {
            self.suppress_next_newline = true;
        }

        (remaining, true)
    }

    fn flush_output_lines(&mut self, flush_partial: bool) {
        while let Some(pos) = self.output_buffer.find('\n') {
            let line = self.output_buffer[..pos].to_string();
            self.output_buffer.drain(..=pos);
            self.write_record("OUTPUT", &line);
        }

        if flush_partial && !self.output_buffer.is_empty() {
            let tail = mem::take(&mut self.output_buffer);
            self.write_record("OUTPUT", &tail);
        }
    }
}

pub struct RecordingManager {
    recordings: Mutex<HashMap<String, RecordingState>>,
}

impl RecordingManager {
    pub fn new() -> Self {
        Self {
            recordings: Mutex::new(HashMap::new()),
        }
    }

    pub fn start(&self, session_id: &str, file_path: &str) -> AppResult<()> {
        let path = PathBuf::from(file_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| AppError::Config(format!("Failed to create directory: {e}")))?;
        }
        let file = File::create(&path)
            .map_err(|e| AppError::Config(format!("Failed to create recording file: {e}")))?;
        let state = RecordingState::new(file, path);
        self.recordings
            .lock()
            .unwrap()
            .insert(session_id.to_string(), state);
        Ok(())
    }

    pub fn stop(&self, session_id: &str) -> AppResult<String> {
        let mut state = {
            let mut recordings = self.recordings.lock().unwrap();
            recordings
                .remove(session_id)
                .ok_or_else(|| AppError::Config("No active recording".to_string()))?
        };
        // finish() flushes to disk -- run outside the mutex to minimize lock hold time
        state.finish();
        Ok(state.file_path.to_string_lossy().to_string())
    }

    pub fn is_recording(&self, session_id: &str) -> bool {
        self.recordings.lock().unwrap().contains_key(session_id)
    }

    pub fn write_output(&self, session_id: &str, data: &str) {
        let mut recordings = self.recordings.lock().unwrap();
        if let Some(state) = recordings.get_mut(session_id) {
            state.write_output(data);
        }
    }

    pub fn write_input(&self, session_id: &str, data: &[u8]) {
        let mut recordings = self.recordings.lock().unwrap();
        if let Some(state) = recordings.get_mut(session_id) {
            state.write_input(data);
        }
    }

    pub fn cleanup_session(&self, session_id: &str) {
        let removed = {
            let mut recordings = self.recordings.lock().unwrap();
            recordings.remove(session_id)
        };
        if let Some(mut state) = removed {
            state.finish();
        }
    }
}

fn chrono_timestamp() -> String {
    let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
    now.format(time::macros::format_description!(
        "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3]"
    ))
    .unwrap_or_else(|_| "1970-01-01 00:00:00.000".to_string())
}

fn consume_matching_prefix(prefix_buffer: &mut String, text: &str) -> usize {
    let mut prefix_idx = 0;
    let mut text_idx = 0;

    while prefix_idx < prefix_buffer.len() && text_idx < text.len() {
        let prefix_char = prefix_buffer[prefix_idx..].chars().next();
        let text_char = text[text_idx..].chars().next();

        match (prefix_char, text_char) {
            (Some(left), Some(right)) if left == right => {
                prefix_idx += left.len_utf8();
                text_idx += right.len_utf8();
            }
            _ => break,
        }
    }

    if prefix_idx > 0 {
        prefix_buffer.drain(..prefix_idx);
    }

    text_idx
}

fn strip_one_leading_newline(text: &str) -> &str {
    text.strip_prefix('\n').unwrap_or(text)
}

fn strip_terminal_control_sequences(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'\x1b' => {
                i += 1;
                if i >= bytes.len() {
                    break;
                }
                match bytes[i] {
                    b'[' => {
                        i += 1;
                        while i < bytes.len() {
                            let b = bytes[i];
                            i += 1;
                            if (0x40..=0x7e).contains(&b) {
                                break;
                            }
                        }
                    }
                    b']' => {
                        i += 1;
                        while i < bytes.len() {
                            if bytes[i] == b'\x07' {
                                i += 1;
                                break;
                            }
                            if bytes[i] == b'\x1b' && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                                i += 2;
                                break;
                            }
                            i += 1;
                        }
                    }
                    b'P' | b'X' | b'^' | b'_' => {
                        i += 1;
                        while i < bytes.len() {
                            if bytes[i] == b'\x1b' && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                                i += 2;
                                break;
                            }
                            i += 1;
                        }
                    }
                    _ => {
                        i += 1;
                    }
                }
            }
            b'\r' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    out.push('\n');
                    i += 2;
                } else {
                    i += 1;
                }
            }
            b'\n' | b'\t' => {
                out.push(bytes[i] as char);
                i += 1;
            }
            b if b.is_ascii_control() => {
                i += 1;
            }
            b if b.is_ascii() => {
                out.push(b as char);
                i += 1;
            }
            _ => {
                let ch = text[i..]
                    .chars()
                    .next()
                    .expect("UTF-8 string must decode to at least one char");
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::{
        consume_matching_prefix, strip_one_leading_newline, strip_terminal_control_sequences,
    };

    #[test]
    fn strips_terminal_escape_sequences_from_output() {
        let raw = concat!(
            "\x1b[?2004l",
            "app.log  \x1b[0m\x1b[01;34mgo\x1b[0m\n",
            "\x1b]7;file://ubuntu/root\x07",
            "\x1b[?2004h\x1b[0m\x1b[1;33m[root\x1b[1;37m@\x1b[1;36mubuntu ",
            "\x1b[1;32m~\x1b[1;35m]\x1b[1;31m\n\n# \x1b[0m"
        );

        let cleaned = strip_terminal_control_sequences(raw);
        assert_eq!(cleaned, "app.log  go\n[root@ubuntu ~]\n\n# ");
    }

    #[test]
    fn consumes_matching_echo_prefix() {
        let mut prefix = "ps -ef".to_string();
        let consumed = consume_matching_prefix(&mut prefix, "ps -ef\nUID");
        assert_eq!(consumed, "ps -ef".len());
        assert!(prefix.is_empty());
    }

    #[test]
    fn strips_only_one_leading_newline() {
        assert_eq!(strip_one_leading_newline("\nhello"), "hello");
        assert_eq!(strip_one_leading_newline("hello"), "hello");
        assert_eq!(strip_one_leading_newline("\n\nhello"), "\nhello");
    }
}
