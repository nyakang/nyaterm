use nyaterm_core::terminal::input_tracker::{TerminalInputState, resync_from_terminal_line};
use nyaterm_terminal::recording_sanitizer::RecordingSanitizer;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as FmtWrite;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::mem;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;
use thiserror::Error;
use time::OffsetDateTime;

pub const DEFAULT_MEMORY_LIMIT_BYTES: usize = 5 * 1024 * 1024;

#[cfg(test)]
mod audit_tests;
pub const DEFAULT_HISTORY_SEARCH_LINES: usize = 30_000;
pub const MAX_HISTORY_SEARCH_LINES: usize = 100_000;
pub const DEFAULT_HISTORY_SEARCH_LIMIT: usize = 100;
pub const DEFAULT_RECORDING_PATH_TEMPLATE: &str =
    "{group}/{session}/{yyyy}-{MM}-{dd}/{HH}-{mm}-{ss}-{SSS}-{session_short_id}.log";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RecordingMode {
    #[default]
    Transcript,
    Raw,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ExistingFileBehavior {
    #[default]
    Unique,
    Append,
    Overwrite,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum RecordingRotationPolicy {
    #[default]
    Session,
    Daily,
    Size {
        max_bytes: u64,
    },
}

#[derive(Clone, Debug)]
pub struct RecordingContext {
    pub session_id: String,
    pub session_name: String,
    pub connection_id: Option<String>,
    pub connection_name: Option<String>,
    pub group_path: Option<String>,
    pub protocol: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub started_at: OffsetDateTime,
}

#[derive(Clone, Debug)]
pub struct RecordingProfile {
    pub mode: RecordingMode,
    pub base_path: PathBuf,
    pub path_template: String,
    pub include_timestamps: bool,
    pub include_io_labels: bool,
    pub include_session_metadata: bool,
    pub rotation: RecordingRotationPolicy,
    pub existing_file_behavior: ExistingFileBehavior,
    pub include_binary_transfer_payloads: bool,
    pub include_input: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RecordingCapturePolicy {
    pub mode: RecordingMode,
    pub include_input: bool,
    pub include_binary_transfer_payloads: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RecordingStatusState {
    Starting,
    #[default]
    Recording,
    Degraded,
    Failed,
    Stopping,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingStatus {
    pub session_id: String,
    pub state: RecordingStatusState,
    pub mode: RecordingMode,
    pub file_path: Option<PathBuf>,
    pub input_file_path: Option<PathBuf>,
    pub started_at: Option<OffsetDateTime>,
    pub written_bytes: u64,
    pub queued_bytes: u64,
    pub dropped_bytes: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Error)]
pub enum RecordingError {
    #[error("{0}")]
    Config(String),
    #[error("invalid regular expression: {0}")]
    Regex(#[from] regex::Error),
    #[error("recording runtime failure: {0}")]
    Runtime(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug)]
struct TranscriptRecord {
    line_id: u64,
    timestamp: String,
    label: &'static str,
    data: String,
    size_bytes: usize,
}

impl TranscriptRecord {
    fn new(line_id: u64, label: &'static str, data: String) -> Self {
        Self::at_timestamp(line_id, label, data, chrono_timestamp())
    }

    fn at_timestamp(line_id: u64, label: &'static str, data: String, timestamp: String) -> Self {
        let size_bytes = timestamp.len() + label.len() + data.len() + 7;
        Self {
            line_id,
            timestamp,
            label,
            data,
            size_bytes,
        }
    }

    fn format(&self, include_io_labels: bool, include_timestamps: bool) -> String {
        format_record_parts(
            &self.timestamp,
            self.label,
            &self.data,
            include_io_labels,
            include_timestamps,
        )
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalHistorySearchRequest {
    pub session_id: String,
    pub query: String,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub whole_word: bool,
    pub limit: Option<usize>,
    pub context_before: Option<usize>,
    pub context_after: Option<usize>,
    pub max_lines: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalHistorySearchResponse {
    pub total: usize,
    pub elapsed_ms: u128,
    pub truncated: bool,
    pub results: Vec<TerminalHistorySearchResult>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalHistorySearchResult {
    pub line_id: u64,
    pub line_number: usize,
    pub column_start: usize,
    pub column_end: usize,
    pub preview: String,
    pub before: Vec<String>,
    pub after: Vec<String>,
    pub source: String,
}

/// Final paths remain available even when writing or flushing failed.
#[derive(Clone, Debug)]
pub struct RecordingCompletion {
    pub file_path: PathBuf,
    pub input_file_path: Option<PathBuf>,
    pub error: Option<String>,
}

struct FileRecording {
    writer: BufWriter<File>,
    input_writer: Option<BufWriter<File>>,
    input_file_path: Option<PathBuf>,
    file_path: PathBuf,
    original_path: PathBuf,
    explicit_path: bool,
    used_paths: HashSet<PathBuf>,
    profile: RecordingProfile,
    context: RecordingContext,
    written_bytes: u64,
    segment_bytes: u64,
    size_rotation_index: u64,
    daily_key: String,
    finished: bool,
    active_paths: Arc<Mutex<HashSet<PathBuf>>>,
}

impl FileRecording {
    fn new(
        file: File,
        input_file: Option<File>,
        file_path: PathBuf,
        explicit_path: bool,
        profile: RecordingProfile,
        context: RecordingContext,
        active_paths: Arc<Mutex<HashSet<PathBuf>>>,
    ) -> Result<Self, RecordingError> {
        let daily_key = recording_day_key(context.started_at);
        let segment_bytes = file.metadata()?.len()
            + input_file
                .as_ref()
                .map(|file| file.metadata().map(|meta| meta.len()))
                .transpose()?
                .unwrap_or(0);
        let input_file_path = input_file
            .as_ref()
            .map(|_| input_recording_path(&file_path));
        let used_paths = std::iter::once(file_path.clone())
            .chain(input_file_path.iter().cloned())
            .collect();
        let mut recording = Self {
            writer: BufWriter::new(file),
            input_writer: input_file.map(BufWriter::new),
            input_file_path,
            original_path: file_path.clone(),
            explicit_path,
            used_paths,
            file_path,
            profile,
            context,
            written_bytes: 0,
            segment_bytes,
            size_rotation_index: 0,
            daily_key,
            finished: false,
            active_paths,
        };
        recording.write_header()?;
        lock_recover(&recording.active_paths).extend(
            std::iter::once(recording.file_path.clone()).chain(recording.input_file_path.clone()),
        );
        Ok(recording)
    }

    fn write_header(&mut self) -> Result<(), RecordingError> {
        if self.profile.mode == RecordingMode::Transcript && self.profile.include_session_metadata {
            let header = format_session_header(&self.context);
            self.writer.write_all(header.as_bytes())?;
            self.note_written(header.len());
        }
        Ok(())
    }

    fn note_written(&mut self, bytes: usize) {
        self.written_bytes = self.written_bytes.saturating_add(bytes as u64);
        self.segment_bytes = self.segment_bytes.saturating_add(bytes as u64);
    }

    fn write_input_bytes(&mut self, data: &[u8]) -> Result<(), RecordingError> {
        if self.input_writer.is_none() || data.is_empty() {
            return Ok(());
        }
        self.maybe_rotate(data.len() as u64)?;
        if let Some(writer) = self.input_writer.as_mut() {
            writer.write_all(data)?;
            self.note_written(data.len());
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), RecordingError> {
        let output = self.writer.flush();
        let input = self.input_writer.as_mut().map(Write::flush).transpose();
        output?;
        input?;
        Ok(())
    }

    fn write_record(&mut self, record: &TranscriptRecord) -> Result<(), RecordingError> {
        if self.profile.mode != RecordingMode::Transcript {
            return Ok(());
        }
        let data = record
            .format(
                self.profile.include_io_labels,
                self.profile.include_timestamps,
            )
            .into_bytes();
        self.write_bytes(&data)
    }

    fn write_raw(&mut self, data: &[u8]) -> Result<(), RecordingError> {
        if self.profile.mode != RecordingMode::Raw || data.is_empty() {
            return Ok(());
        }
        self.write_bytes(data)
    }

    fn write_bytes(&mut self, data: &[u8]) -> Result<(), RecordingError> {
        self.maybe_rotate(data.len() as u64)?;
        self.writer.write_all(data)?;
        self.note_written(data.len());
        Ok(())
    }

    fn finish(&mut self) -> Result<(), RecordingError> {
        let mut footer_result = Ok(());
        if !self.finished
            && self.profile.mode == RecordingMode::Transcript
            && self.profile.include_session_metadata
        {
            self.finished = true;
            let footer = format_session_footer(&self.context, "Stopped").into_bytes();
            footer_result = self.writer.write_all(&footer);
            if footer_result.is_ok() {
                self.note_written(footer.len());
            }
        }
        let flush_result = self.flush();
        footer_result?;
        flush_result?;
        Ok(())
    }

    fn maybe_rotate(&mut self, incoming_bytes: u64) -> Result<(), RecordingError> {
        match self.profile.rotation {
            RecordingRotationPolicy::Session => return Ok(()),
            RecordingRotationPolicy::Size { max_bytes }
                if max_bytes == 0
                    || self.segment_bytes == 0
                    || self.segment_bytes.saturating_add(incoming_bytes) <= max_bytes =>
            {
                return Ok(());
            }
            _ => {}
        }
        let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
        let next_key = recording_day_key(now);
        let rotate = match self.profile.rotation {
            RecordingRotationPolicy::Session => false,
            RecordingRotationPolicy::Daily => next_key != self.daily_key,
            RecordingRotationPolicy::Size { max_bytes } => {
                max_bytes > 0
                    && self.segment_bytes > 0
                    && self.segment_bytes.saturating_add(incoming_bytes) > max_bytes
            }
        };
        if !rotate {
            return Ok(());
        }

        self.size_rotation_index = self.size_rotation_index.saturating_add(1);
        let suffix = matches!(self.profile.rotation, RecordingRotationPolicy::Size { .. })
            .then_some(self.size_rotation_index);
        let mut rotation_context = self.context.clone();
        rotation_context.started_at = now;
        let mut requested = if self.explicit_path {
            match self.profile.rotation {
                RecordingRotationPolicy::Daily => {
                    append_named_suffix(&self.original_path, &next_key)
                }
                _ => append_numbered_suffix(self.original_path.clone(), self.size_rotation_index),
            }
        } else {
            resolve_recording_path(&self.profile, &rotation_context, suffix)?
        };
        while self.used_paths.contains(&requested) {
            requested = append_numbered_suffix(requested, self.size_rotation_index);
        }
        // Refuse aliases of active files and earlier segments before any truncation.
        let protected = lock_recover(&self.active_paths);
        for path in std::iter::once(&requested).chain(
            (self.profile.mode == RecordingMode::Raw && self.profile.include_input)
                .then(|| input_recording_path(&requested))
                .as_ref(),
        ) {
            for active in protected.iter().chain(self.used_paths.iter()) {
                if path == active
                    || (path.exists() && active.exists() && same_file::is_same_file(path, active)?)
                {
                    return Err(RecordingError::Config(
                        "rotation destination aliases a recording segment".into(),
                    ));
                }
            }
        }
        drop(protected);
        self.flush()?;
        let (path, file, input_file) = open_recording_pair(
            &requested,
            self.profile.existing_file_behavior,
            self.profile.mode == RecordingMode::Raw && self.profile.include_input,
        )?;
        if self.profile.mode == RecordingMode::Transcript && self.profile.include_session_metadata {
            let footer = format_session_footer(&self.context, "Rotated");
            self.writer.write_all(footer.as_bytes())?;
            self.note_written(footer.len());
        }
        self.flush()?;
        let mut next_writer = BufWriter::new(file);
        let mut segment_bytes = next_writer.get_ref().metadata()?.len()
            + input_file
                .as_ref()
                .map(|file| file.metadata().map(|meta| meta.len()))
                .transpose()?
                .unwrap_or(0);
        if self.profile.mode == RecordingMode::Transcript && self.profile.include_session_metadata {
            let header = format_session_header(&self.context).into_bytes();
            next_writer.write_all(&header)?;
            self.written_bytes += header.len() as u64;
            segment_bytes += header.len() as u64;
        }
        let mut active = lock_recover(&self.active_paths);
        active.remove(&self.file_path);
        if let Some(input) = &self.input_file_path {
            active.remove(input);
        }
        active.insert(path.clone());
        if input_file.is_some() {
            active.insert(input_recording_path(&path));
        }
        drop(active);
        self.writer = next_writer;
        self.input_writer = input_file.map(BufWriter::new);
        self.input_file_path = self
            .input_writer
            .as_ref()
            .map(|_| input_recording_path(&path));
        self.used_paths.insert(path.clone());
        self.used_paths.extend(self.input_file_path.iter().cloned());
        self.file_path = path;
        self.segment_bytes = segment_bytes;
        self.daily_key = next_key;
        Ok(())
    }
}

impl Drop for FileRecording {
    fn drop(&mut self) {
        let mut active = lock_recover(&self.active_paths);
        active.remove(&self.file_path);
        if let Some(input) = &self.input_file_path {
            active.remove(input);
        }
    }
}

struct SessionCaptureState {
    recording: Option<FileRecording>,
    recording_state: RecordingStatusState,
    disconnected: bool,
    dropped_bytes: u64,
    last_error: Option<String>,
    records: VecDeque<TranscriptRecord>,
    record_bytes: usize,
    memory_limit_bytes: usize,
    pending_input_escape: Option<TerminalInputEscapeState>,
    input_buffer: String,
    input_needs_terminal_resync: bool,
    input_before_tab: Option<String>,
    output_buffer: String,
    /// Byte offset in `output_buffer` where the next character overwrites,
    /// after a cursor-back; `None` appends at the end.
    output_cursor: Option<usize>,
    output_scan_from: usize,
    provisional_raw_output: VecDeque<u8>,
    live_echo_buffer: String,
    submitted_line_echo: Option<String>,
    suppress_next_newline: bool,
    next_line_id: u64,
    sanitizer: RecordingSanitizer,
    execution_sanitizer: RecordingSanitizer,
    history_truncated: bool,
    history_include_input: bool,
    batch_timestamp: String,
}

impl SessionCaptureState {
    fn new(memory_limit_bytes: usize) -> Self {
        Self {
            recording: None,
            recording_state: RecordingStatusState::Recording,
            disconnected: false,
            dropped_bytes: 0,
            last_error: None,
            records: VecDeque::new(),
            record_bytes: 0,
            memory_limit_bytes,
            pending_input_escape: None,
            input_buffer: String::new(),
            input_needs_terminal_resync: false,
            input_before_tab: None,
            output_buffer: String::new(),
            output_cursor: None,
            output_scan_from: 0,
            provisional_raw_output: VecDeque::new(),
            live_echo_buffer: String::new(),
            submitted_line_echo: None,
            suppress_next_newline: false,
            next_line_id: 1,
            sanitizer: RecordingSanitizer::with_cursor_back(),
            execution_sanitizer: RecordingSanitizer::default(),
            history_truncated: false,
            history_include_input: false,
            batch_timestamp: chrono_timestamp(),
        }
    }

    fn set_memory_limit(&mut self, memory_limit_bytes: usize) {
        self.memory_limit_bytes = memory_limit_bytes.max(1);
        self.trim_records();
    }

    fn start_recording(&mut self, recording: FileRecording) -> Result<(), RecordingError> {
        if self.recording.is_some() {
            return Err(RecordingError::Config(
                "recording is already active".to_string(),
            ));
        }
        self.recording_state = RecordingStatusState::Starting;
        self.last_error = None;
        self.dropped_bytes = 0;
        self.flush_output_lines(true);
        self.provisional_raw_output.clear();
        self.commit_partial_input();
        self.recording = Some(recording);
        self.recording_state = RecordingStatusState::Recording;
        Ok(())
    }

    fn stop_recording(&mut self) -> Result<String, RecordingError> {
        let completion = self.stop_complete()?;
        match completion.error {
            Some(error) => Err(RecordingError::Runtime(error)),
            None => Ok(completion.file_path.to_string_lossy().into_owned()),
        }
    }

    fn stop_complete(&mut self) -> Result<RecordingCompletion, RecordingError> {
        if self.recording.is_none() {
            return Err(RecordingError::Config("no active recording".to_string()));
        }
        if self.recording_state != RecordingStatusState::Failed {
            self.recording_state = RecordingStatusState::Stopping;
        }
        self.commit_partial_input();
        self.flush_output_lines(true);
        let mut recording = self
            .recording
            .take()
            .ok_or_else(|| RecordingError::Config("no active recording".to_string()))?;
        let file_path = recording.file_path.clone();
        let input_file_path = recording.input_file_path.clone();
        if let Err(error) = recording.finish() {
            self.mark_failed(error.to_string());
        }
        let error = self.last_error.clone();
        if error.is_none() {
            self.recording_state = RecordingStatusState::Recording;
        }
        Ok(RecordingCompletion {
            file_path,
            input_file_path,
            error,
        })
    }

    fn mark_failed(&mut self, error: String) {
        self.recording_state = RecordingStatusState::Failed;
        self.last_error = Some(error);
    }

    fn report_dropped(&mut self, bytes: usize) {
        self.commit_partial_input();
        self.flush_output_lines(true);
        self.dropped_bytes = self.dropped_bytes.saturating_add(bytes as u64);
        self.sanitizer.reset();
        self.execution_sanitizer.reset();
        self.pending_input_escape = None;
        self.live_echo_buffer.clear();
        self.submitted_line_echo = None;
        self.history_truncated = true;
        if self.recording_state != RecordingStatusState::Failed {
            self.recording_state = RecordingStatusState::Degraded;
            self.last_error = Some("recording writer queue overflowed".to_string());
        }
    }

    fn write_input(&mut self, data: &[u8]) {
        self.batch_timestamp = chrono_timestamp();
        if !self.includes_input() {
            return;
        }
        if self
            .recording
            .as_ref()
            .is_some_and(|file| file.profile.mode == RecordingMode::Raw)
        {
            return;
        }
        let mut index = 0;

        while index < data.len() {
            if let Some(state) = self.pending_input_escape.take() {
                let (next_index, next_state) =
                    consume_terminal_input_escape_state(data, index, state);
                self.pending_input_escape = next_state;
                index = next_index;
                continue;
            }

            match data[index] {
                b'\r' | b'\n' => {
                    self.commit_input_line();
                    index += 1;
                }
                b'\x08' | b'\x7f' => {
                    self.handle_backspace();
                    index += 1;
                }
                b'\t' => {
                    if !self.input_needs_terminal_resync {
                        self.input_before_tab = Some(self.input_buffer.clone());
                    }
                    self.input_needs_terminal_resync = true;
                    index += 1;
                }
                b'\x1b' => {
                    let (next_index, next_state) = consume_terminal_input_escape_state(
                        data,
                        index + 1,
                        TerminalInputEscapeState::Esc,
                    );
                    self.pending_input_escape = next_state;
                    index = next_index;
                }
                byte if byte.is_ascii_control() => {
                    index += 1;
                }
                byte if byte.is_ascii() => {
                    let ch = byte as char;
                    self.input_buffer.push(ch);
                    self.live_echo_buffer.push(ch);
                    index += 1;
                }
                _ => match next_utf8_char(data, index) {
                    Some((ch, next_index)) if !ch.is_control() => {
                        self.input_buffer.push(ch);
                        self.live_echo_buffer.push(ch);
                        index = next_index;
                    }
                    Some((_ch, next_index)) => {
                        index = next_index;
                    }
                    None => {
                        index += 1;
                    }
                },
            }
            if self.input_buffer.len() >= self.fragment_limit() {
                self.commit_partial_input();
            }
            self.trim_records();
        }
    }

    fn write_raw_input(&mut self, data: &[u8]) {
        self.batch_timestamp = chrono_timestamp();
        if !self.includes_input() {
            return;
        }
        if data.is_empty() {
            return;
        }
        if self
            .recording
            .as_ref()
            .is_some_and(|recording| recording.profile.mode == RecordingMode::Raw)
        {
            let result = self
                .recording
                .as_mut()
                .expect("recording checked above")
                .write_input_bytes(data);
            if let Err(error) = result {
                self.mark_failed(error.to_string());
            }
            return;
        }

        self.commit_partial_input();
        self.flush_output_lines(true);
        self.append_record("RAW_INPUT", format_raw_input_bytes(data));
    }

    fn resync_input_line(&mut self, terminal_line: &str) {
        if !self.input_needs_terminal_resync {
            return;
        }
        let current = TerminalInputState {
            value: self
                .input_before_tab
                .as_ref()
                .unwrap_or(&self.input_buffer)
                .clone(),
            ..TerminalInputState::new()
        };
        if let Some(recovered) = resync_from_terminal_line(&current, terminal_line) {
            self.input_buffer = recovered.value;
            self.live_echo_buffer.clear();
            self.output_buffer.clear();
            self.output_cursor = None;
            self.output_scan_from = 0;
            self.input_needs_terminal_resync = false;
            self.input_before_tab = None;
            if self.input_buffer.len() >= self.fragment_limit() {
                self.commit_partial_input();
            }
            self.trim_records();
        }
    }

    fn includes_input(&self) -> bool {
        self.recording
            .as_ref()
            .map_or(self.history_include_input, |file| {
                file.profile.include_input
            })
    }

    fn write_raw_output(&mut self, data: &[u8]) {
        if let Some(recording) = self.recording.as_mut() {
            if self.recording_state != RecordingStatusState::Failed
                && let Err(error) = recording.write_raw(data)
            {
                self.mark_failed(error.to_string());
            }
        } else {
            let keep = self.memory_limit_bytes.min(data.len());
            self.provisional_raw_output
                .extend(&data[data.len() - keep..]);
            self.trim_records();
        }
    }

    fn write_output(&mut self, data: &str) {
        self.batch_timestamp = chrono_timestamp();
        let mut sanitized = self.sanitizer.feed(data);
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

        self.append_output(&sanitized);
    }

    fn append_output(&mut self, text: &str) {
        for chunk in utf8_fragments(text, self.fragment_limit()) {
            self.push_output(chunk);
            self.flush_output_lines(false);
            self.trim_records();
        }
    }

    fn execution_boundary(&mut self, command: Option<&str>) {
        self.flush_output_lines(true);
        self.execution_sanitizer.reset();
        if self.includes_input()
            && !self
                .recording
                .as_ref()
                .is_some_and(|file| file.profile.mode == RecordingMode::Raw)
            && let Some(command) = command
        {
            self.batch_timestamp = chrono_timestamp();
            let text = RecordingSanitizer::default().feed(command);
            for line in text.lines() {
                self.append_record("INPUT", line.to_string());
            }
        }
    }

    fn write_execution_output(&mut self, text: &str) {
        self.batch_timestamp = chrono_timestamp();
        let text = self.execution_sanitizer.feed(text);
        // Background execution can overlap manual typing and output. Its output
        // must neither consume keyboard echo nor share partial control strings.
        self.append_output(&text);
    }

    fn finish(&mut self) {
        self.commit_partial_input();
        self.flush_output_lines(true);
        if let Some(recording) = self.recording.as_mut()
            && let Err(error) = recording.finish()
        {
            self.recording_state = RecordingStatusState::Failed;
            self.last_error = Some(error.to_string());
        }
        self.recording = None;
    }

    fn disconnect(&mut self) {
        if self.disconnected {
            return;
        }
        self.disconnected = true;
        self.commit_partial_input();
        self.flush_output_lines(true);
        self.pending_input_escape = None;
        self.live_echo_buffer.clear();
        self.submitted_line_echo = None;
        self.suppress_next_newline = false;
        self.append_record("SYSTEM", "Session disconnected".to_string());
        if let Some(recording) = self.recording.as_mut()
            && let Err(error) = recording.flush()
        {
            self.mark_failed(error.to_string());
        }
    }

    fn absorb_early_state(&mut self, mut early: Self) {
        early.commit_partial_input();
        early.flush_output_lines(true);
        if let Some(recording) = self.recording.as_mut()
            && recording.profile.mode == RecordingMode::Raw
            && let Err(error) = recording.write_raw(early.provisional_raw_output.make_contiguous())
        {
            self.mark_failed(error.to_string());
        }
        if let Some(mut recording) = early.recording.take()
            && let Err(error) = recording.finish()
        {
            self.mark_failed(error.to_string());
        }
        for mut record in early.records {
            record.line_id = self.next_line_id;
            self.next_line_id = self.next_line_id.saturating_add(1);
            if let Some(recording) = self.recording.as_mut()
                && let Err(error) = recording.write_record(&record)
            {
                self.mark_failed(error.to_string());
            }
            self.record_bytes += record.size_bytes;
            self.records.push_back(record);
            self.trim_records();
        }
    }

    fn snapshot_records(&self) -> Vec<TranscriptRecord> {
        let mut records: Vec<_> = self.records.iter().cloned().collect();
        if !self.output_buffer.is_empty() {
            records.push(TranscriptRecord::new(
                self.next_line_id,
                "OUTPUT",
                self.output_buffer.clone(),
            ));
        }
        if self.includes_input() && !self.input_buffer.trim().is_empty() {
            records.push(TranscriptRecord::new(
                self.next_line_id + 1,
                "INPUT",
                self.input_buffer.clone(),
            ));
        }
        records
    }

    fn append_record(&mut self, label: &'static str, data: String) {
        if data.is_empty() && label != "OUTPUT" {
            return;
        }
        if data.len() > self.fragment_limit() {
            for fragment in utf8_fragments(&data, self.fragment_limit()) {
                self.append_record(label, fragment.to_string());
            }
            return;
        }

        let line_id = self.next_line_id;
        self.next_line_id = self.next_line_id.saturating_add(1);
        let record =
            TranscriptRecord::at_timestamp(line_id, label, data, self.batch_timestamp.clone());
        if self.recording_state != RecordingStatusState::Failed
            && let Some(recording) = self.recording.as_mut()
            && let Err(error) = recording.write_record(&record)
        {
            self.recording_state = RecordingStatusState::Failed;
            self.last_error = Some(error.to_string());
        }

        self.record_bytes += record.size_bytes;
        self.records.push_back(record);
        self.trim_records();
    }

    fn fragment_limit(&self) -> usize {
        (self.memory_limit_bytes / 4).clamp(4, 64 * 1024)
    }

    fn buffered_bytes(&self) -> usize {
        self.input_buffer.len()
            + self.output_buffer.len()
            + self.live_echo_buffer.len()
            + self.input_before_tab.as_ref().map_or(0, String::len)
            + self.submitted_line_echo.as_ref().map_or(0, String::len)
            + self.provisional_raw_output.len()
    }

    fn trim_records(&mut self) {
        while self.record_bytes + self.buffered_bytes() > self.memory_limit_bytes {
            if let Some(record) = self.records.pop_front() {
                self.record_bytes = self.record_bytes.saturating_sub(record.size_bytes);
                self.history_truncated = true;
                continue;
            }
            if !self.provisional_raw_output.is_empty() {
                let excess = (self.buffered_bytes() - self.memory_limit_bytes)
                    .min(self.provisional_raw_output.len());
                self.provisional_raw_output.drain(..excess);
            } else if !self.live_echo_buffer.is_empty()
                || self.input_before_tab.is_some()
                || self.submitted_line_echo.is_some()
            {
                self.live_echo_buffer.clear();
                self.input_before_tab = None;
                self.submitted_line_echo = None;
            } else if !self.output_buffer.is_empty() {
                self.flush_output_lines(true);
            } else if !self.input_buffer.is_empty() {
                self.commit_partial_input();
            } else {
                break;
            }
            self.history_truncated = true;
        }
    }

    fn handle_backspace(&mut self) {
        if let Some(removed) = self.input_buffer.pop()
            && self.live_echo_buffer.ends_with(removed)
        {
            self.live_echo_buffer.pop();
        }
    }

    fn commit_input_line(&mut self) {
        self.flush_output_lines(true);
        let line = mem::take(&mut self.input_buffer);
        self.input_needs_terminal_resync = false;
        self.input_before_tab = None;
        self.live_echo_buffer.clear();

        if line.trim().is_empty() {
            self.submitted_line_echo = None;
            return;
        }

        self.append_record("INPUT", line.clone());
        self.submitted_line_echo = Some(line);
    }

    fn commit_partial_input(&mut self) {
        self.flush_output_lines(true);
        let line = mem::take(&mut self.input_buffer);
        self.input_needs_terminal_resync = false;
        self.input_before_tab = None;
        self.live_echo_buffer.clear();
        self.submitted_line_echo = None;

        if line.trim().is_empty() {
            return;
        }

        self.append_record("INPUT", line);
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

    /// Append output, applying `\x08` (cursor back) to the line still in the
    /// buffer so redrawn text overwrites what it erased on screen.
    fn push_output(&mut self, text: &str) {
        for ch in text.chars() {
            match ch {
                '\x08' => {
                    let line_start = self.output_buffer.rfind('\n').map_or(0, |index| index + 1);
                    let cursor = self.output_cursor.unwrap_or(self.output_buffer.len());
                    if cursor > line_start {
                        let previous = self.output_buffer[line_start..cursor]
                            .char_indices()
                            .next_back()
                            .map_or(line_start, |(index, _)| line_start + index);
                        self.output_cursor = Some(previous);
                    }
                }
                '\n' => {
                    self.output_cursor = None;
                    self.output_buffer.push('\n');
                }
                ch => match self.output_cursor {
                    Some(cursor) if cursor < self.output_buffer.len() => {
                        let end = self.output_buffer[cursor..]
                            .chars()
                            .next()
                            .map_or(cursor, |current| cursor + current.len_utf8());
                        self.output_buffer
                            .replace_range(cursor..end, ch.encode_utf8(&mut [0; 4]));
                        let next = cursor + ch.len_utf8();
                        self.output_cursor = (next < self.output_buffer.len()).then_some(next);
                        // An overwrite may shorten the buffer below the scan mark.
                        self.output_scan_from = self.output_scan_from.min(cursor);
                    }
                    _ => {
                        self.output_cursor = None;
                        self.output_buffer.push(ch);
                    }
                },
            }
        }
    }

    fn flush_output_lines(&mut self, flush_partial: bool) {
        let limit = self.fragment_limit();
        if !flush_partial && self.output_buffer.len() < limit {
            let start = self.output_scan_from.min(self.output_buffer.len());
            let complete_line = self.output_buffer[start..].contains('\n');
            self.output_scan_from = self.output_buffer.len();
            if !complete_line {
                return;
            }
        }
        let buffer = mem::take(&mut self.output_buffer);
        let cursor = self.output_cursor.take();
        self.output_scan_from = 0;
        let mut start = 0;
        while start < buffer.len() {
            let tail = &buffer[start..];
            if let Some(pos) = tail[..utf8_prefix_len(tail, limit)].find('\n') {
                self.append_record("OUTPUT", tail[..pos].to_string());
                start += pos + 1;
            } else if tail.len() >= limit {
                let end = utf8_prefix_len(tail, limit);
                self.append_record("OUTPUT", tail[..end].to_string());
                start += end;
            } else if flush_partial {
                self.append_record("OUTPUT", tail.to_string());
                break;
            } else {
                self.output_buffer.push_str(tail);
                self.output_cursor = cursor
                    .and_then(|cursor| cursor.checked_sub(start))
                    .filter(|cursor| *cursor < tail.len());
                self.output_scan_from = tail.len();
                break;
            }
        }
    }
}

pub struct RecordingManager {
    sessions: Mutex<HashMap<String, SessionCaptureState>>,
    memory_limit_bytes: Mutex<usize>,
    history_include_input: Mutex<bool>,
    active_paths: Arc<Mutex<HashSet<PathBuf>>>,
}

impl RecordingManager {
    pub fn set_history_include_input(&self, enabled: bool) {
        *lock_recover(&self.history_include_input) = enabled;
        for state in lock_recover(&self.sessions).values_mut() {
            state.history_include_input = enabled;
            if !state.includes_input() {
                state.input_buffer.clear();
                state.input_before_tab = None;
                state.live_echo_buffer.clear();
                state.submitted_line_echo = None;
                state.pending_input_escape = None;
            }
        }
    }

    pub fn capture_policy(&self, session_id: &str) -> RecordingCapturePolicy {
        let sessions = lock_recover(&self.sessions);
        if let Some(file) = sessions
            .get(session_id)
            .and_then(|state| state.recording.as_ref())
        {
            RecordingCapturePolicy {
                mode: file.profile.mode,
                include_input: file.profile.include_input,
                include_binary_transfer_payloads: file.profile.include_binary_transfer_payloads,
            }
        } else {
            RecordingCapturePolicy {
                include_input: *lock_recover(&self.history_include_input),
                ..Default::default()
            }
        }
    }

    pub fn write_raw_output(&self, session_id: &str, data: &[u8]) {
        let memory_limit = *lock_recover(&self.memory_limit_bytes);
        let mut sessions = lock_recover(&self.sessions);
        let state = sessions
            .entry(session_id.to_string())
            .or_insert_with(|| SessionCaptureState::new(memory_limit));
        state.write_raw_output(data);
    }

    pub fn flush_recordings(&self) {
        for state in lock_recover(&self.sessions).values_mut() {
            if let Some(file) = state.recording.as_mut()
                && let Err(error) = file.flush()
            {
                state.mark_failed(error.to_string());
            }
        }
    }

    pub fn finish_all(&self) -> Vec<RecordingError> {
        let mut errors = Vec::new();
        for state in lock_recover(&self.sessions).values_mut() {
            if state.recording.is_some()
                && let Err(error) = state.stop_recording()
            {
                errors.push(error);
            }
        }
        errors
    }
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            memory_limit_bytes: Mutex::new(DEFAULT_MEMORY_LIMIT_BYTES),
            history_include_input: Mutex::new(false),
            active_paths: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn start(
        &self,
        session_id: &str,
        file_path: &str,
        include_io_labels: bool,
        include_timestamps: bool,
    ) -> Result<(), RecordingError> {
        let path = PathBuf::from(file_path);
        let base_path = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("recording.log")
            .to_string();
        let profile = RecordingProfile {
            mode: RecordingMode::Transcript,
            base_path,
            path_template: file_name,
            include_timestamps,
            include_io_labels,
            include_session_metadata: false,
            rotation: RecordingRotationPolicy::Session,
            existing_file_behavior: ExistingFileBehavior::Overwrite,
            include_binary_transfer_payloads: false,
            include_input: *lock_recover(&self.history_include_input),
        };
        let context = RecordingContext {
            session_id: session_id.to_string(),
            session_name: safe_recording_name(session_id),
            connection_id: None,
            connection_name: None,
            group_path: None,
            protocol: "terminal".to_string(),
            host: None,
            port: None,
            username: None,
            started_at: OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc()),
        };
        self.start_with_profile(session_id, context, profile, Some(path))
            .map(|_| ())
    }

    pub fn start_with_profile(
        &self,
        session_id: &str,
        context: RecordingContext,
        profile: RecordingProfile,
        explicit_path: Option<PathBuf>,
    ) -> Result<String, RecordingError> {
        let mut sessions = lock_recover(&self.sessions);
        if sessions
            .get(session_id)
            .is_some_and(|state| state.recording.is_some())
        {
            return Err(RecordingError::Config(
                "recording is already active".to_string(),
            ));
        }
        let is_explicit = explicit_path.is_some();
        let requested_path = match explicit_path {
            Some(path) => path,
            None => resolve_recording_path(&profile, &context, None)?,
        };
        if profile.existing_file_behavior != ExistingFileBehavior::Unique {
            reject_active_path(&sessions, &requested_path)?;
        }
        if profile.existing_file_behavior != ExistingFileBehavior::Unique
            && profile.mode == RecordingMode::Raw
            && profile.include_input
        {
            reject_active_path(&sessions, &input_recording_path(&requested_path))?;
        }
        let (path, file, input_file) = open_recording_pair(
            &requested_path,
            profile.existing_file_behavior,
            profile.mode == RecordingMode::Raw && profile.include_input,
        )?;
        let memory_limit_bytes = *lock_recover(&self.memory_limit_bytes);
        let state = sessions
            .entry(session_id.to_string())
            .or_insert_with(|| SessionCaptureState::new(memory_limit_bytes));
        state.set_memory_limit(memory_limit_bytes);
        state.start_recording(FileRecording::new(
            file,
            input_file,
            path.clone(),
            is_explicit,
            profile,
            context,
            Arc::clone(&self.active_paths),
        )?)?;
        Ok(path.to_string_lossy().to_string())
    }

    pub fn stop(&self, session_id: &str) -> Result<String, RecordingError> {
        let mut sessions = lock_recover(&self.sessions);
        let state = sessions
            .get_mut(session_id)
            .ok_or_else(|| RecordingError::Config("no active recording".to_string()))?;
        state.stop_recording()
    }

    pub fn stop_complete(&self, session_id: &str) -> Result<RecordingCompletion, RecordingError> {
        lock_recover(&self.sessions)
            .get_mut(session_id)
            .ok_or_else(|| RecordingError::Config("no active recording".into()))?
            .stop_complete()
    }

    pub fn status(&self, session_id: &str) -> Option<RecordingStatus> {
        let sessions = lock_recover(&self.sessions);
        recording_status_for_state(session_id, sessions.get(session_id)?)
    }

    pub fn list_recording_statuses(&self) -> Vec<RecordingStatus> {
        let sessions = lock_recover(&self.sessions);
        sessions
            .iter()
            .filter_map(|(session_id, state)| recording_status_for_state(session_id, state))
            .collect()
    }

    pub fn save_transcript(
        &self,
        session_id: &str,
        file_path: &str,
        include_io_labels: bool,
        include_timestamps: bool,
    ) -> Result<String, RecordingError> {
        self.save_transcript_impl(
            session_id,
            file_path,
            include_io_labels,
            include_timestamps,
            false,
        )
    }

    pub fn save_transcript_unique(
        &self,
        session_id: &str,
        file_path: &str,
        include_io_labels: bool,
        include_timestamps: bool,
    ) -> Result<String, RecordingError> {
        self.save_transcript_impl(
            session_id,
            file_path,
            include_io_labels,
            include_timestamps,
            true,
        )
    }

    fn save_transcript_impl(
        &self,
        session_id: &str,
        file_path: &str,
        include_io_labels: bool,
        include_timestamps: bool,
        unique: bool,
    ) -> Result<String, RecordingError> {
        let sessions = lock_recover(&self.sessions);
        let path = prepare_output_file_path(file_path)?;
        reject_active_path(&sessions, &path)?;
        let records = sessions
            .get(session_id)
            .map(SessionCaptureState::snapshot_records)
            .unwrap_or_default();
        let parent = path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        for record in &records {
            temporary.write_all(
                record
                    .format(include_io_labels, include_timestamps)
                    .as_bytes(),
            )?;
        }
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        let mut candidate = path.clone();
        for index in 0..10_000 {
            if unique {
                match temporary.persist_noclobber(&candidate) {
                    Ok(_) => return Ok(candidate.to_string_lossy().into_owned()),
                    Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                        temporary = error.file;
                        candidate = append_numbered_suffix(path.clone(), index + 1);
                    }
                    Err(error) => return Err(error.error.into()),
                }
            } else {
                temporary.persist(&candidate).map_err(|error| error.error)?;
                return Ok(candidate.to_string_lossy().into_owned());
            }
        }
        Err(RecordingError::Config(
            "too many transcript file collisions".into(),
        ))
    }

    pub fn search_history(
        &self,
        request: TerminalHistorySearchRequest,
    ) -> Result<TerminalHistorySearchResponse, RecordingError> {
        let started = Instant::now();
        let query = request.query;
        if query.is_empty() {
            return Ok(TerminalHistorySearchResponse {
                total: 0,
                elapsed_ms: started.elapsed().as_millis(),
                truncated: false,
                results: Vec::new(),
            });
        }

        let limit = request.limit.unwrap_or(DEFAULT_HISTORY_SEARCH_LIMIT).max(1);
        let context_before = request.context_before.unwrap_or(0).min(20);
        let context_after = request.context_after.unwrap_or(0).min(20);
        let max_lines = request
            .max_lines
            .unwrap_or(DEFAULT_HISTORY_SEARCH_LINES)
            .clamp(1, MAX_HISTORY_SEARCH_LINES);
        let records = {
            let sessions = lock_recover(&self.sessions);
            sessions
                .get(&request.session_id)
                .map(SessionCaptureState::snapshot_records)
                .unwrap_or_default()
        };
        let start_index = records.len().saturating_sub(max_lines);
        let searched_records = &records[start_index..];
        let matcher = HistoryMatcher::new(
            &query,
            request.case_sensitive,
            request.regex,
            request.whole_word,
        )?;
        let mut total = 0usize;
        let mut results = Vec::new();

        for (relative_index, record) in searched_records.iter().enumerate() {
            if let Some((column_start, column_end)) = matcher.find(&record.data) {
                total += 1;
                if results.len() < limit {
                    let absolute_index = start_index + relative_index;
                    results.push(TerminalHistorySearchResult {
                        line_id: record.line_id,
                        line_number: absolute_index + 1,
                        column_start,
                        column_end,
                        preview: record.data.clone(),
                        before: context_records(&records, absolute_index, context_before, true),
                        after: context_records(&records, absolute_index, context_after, false),
                        source: record.label.to_ascii_lowercase(),
                    });
                }
            }
        }

        Ok(TerminalHistorySearchResponse {
            total,
            elapsed_ms: started.elapsed().as_millis(),
            truncated: total > results.len()
                || records.len() > max_lines
                || lock_recover(&self.sessions)
                    .get(&request.session_id)
                    .is_some_and(|state| state.history_truncated),
            results,
        })
    }

    pub fn set_memory_limit(&self, max_bytes: usize) {
        let bounded = max_bytes.max(1);
        *lock_recover(&self.memory_limit_bytes) = bounded;

        let mut sessions = lock_recover(&self.sessions);
        for state in sessions.values_mut() {
            state.set_memory_limit(bounded);
        }
    }

    pub fn is_recording(&self, session_id: &str) -> bool {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(session_id)
            .is_some_and(|state| state.recording.is_some())
    }

    pub fn list_recording_sessions(&self) -> Vec<String> {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter_map(|(id, state)| state.recording.as_ref().map(|_| id.clone()))
            .collect()
    }

    pub fn write_output(&self, session_id: &str, data: &str) {
        let memory_limit_bytes = *lock_recover(&self.memory_limit_bytes);
        let mut sessions = lock_recover(&self.sessions);
        let state = sessions
            .entry(session_id.to_string())
            .or_insert_with(|| SessionCaptureState::new(memory_limit_bytes));
        state.history_include_input = *lock_recover(&self.history_include_input);
        state.set_memory_limit(memory_limit_bytes);
        state.write_output(data);
    }

    pub fn write_local_message(&self, session_id: &str, text: &str) {
        let budget = *lock_recover(&self.memory_limit_bytes);
        let mut sessions = lock_recover(&self.sessions);
        let state = sessions
            .entry(session_id.into())
            .or_insert_with(|| SessionCaptureState::new(budget));
        state.batch_timestamp = chrono_timestamp();
        state.append_record("SYSTEM", text.to_string());
    }

    pub fn write_input(&self, session_id: &str, data: &[u8]) {
        let memory_limit_bytes = *lock_recover(&self.memory_limit_bytes);
        let mut sessions = lock_recover(&self.sessions);
        let state = sessions
            .entry(session_id.to_string())
            .or_insert_with(|| SessionCaptureState::new(memory_limit_bytes));
        state.history_include_input = *lock_recover(&self.history_include_input);
        state.set_memory_limit(memory_limit_bytes);
        state.write_input(data);
    }

    /// Delimit automatic execution without treating its command as typed input.
    /// `None` also finishes an execution and flushes an unterminated output line.
    pub fn execution_boundary(&self, session_id: &str, command: Option<&str>) {
        let budget = *lock_recover(&self.memory_limit_bytes);
        let mut sessions = lock_recover(&self.sessions);
        let state = sessions
            .entry(session_id.into())
            .or_insert_with(|| SessionCaptureState::new(budget));
        state.history_include_input = *lock_recover(&self.history_include_input);
        state.set_memory_limit(budget);
        state.execution_boundary(command);
    }

    pub fn write_execution_output(&self, session_id: &str, text: &str) {
        let budget = *lock_recover(&self.memory_limit_bytes);
        let mut sessions = lock_recover(&self.sessions);
        let state = sessions
            .entry(session_id.into())
            .or_insert_with(|| SessionCaptureState::new(budget));
        state.set_memory_limit(budget);
        state.write_execution_output(text);
    }

    pub fn resync_input_line(&self, session_id: &str, terminal_line: &str) {
        let mut sessions = lock_recover(&self.sessions);
        if let Some(state) = sessions.get_mut(session_id) {
            state.resync_input_line(terminal_line);
        }
    }

    pub fn write_raw_input(&self, session_id: &str, data: &[u8]) {
        let memory_limit_bytes = *lock_recover(&self.memory_limit_bytes);
        let mut sessions = lock_recover(&self.sessions);
        let state = sessions
            .entry(session_id.to_string())
            .or_insert_with(|| SessionCaptureState::new(memory_limit_bytes));
        state.history_include_input = *lock_recover(&self.history_include_input);
        state.set_memory_limit(memory_limit_bytes);
        state.write_raw_input(data);
    }

    pub fn report_dropped(&self, session_id: &str, bytes: usize) {
        let mut sessions = lock_recover(&self.sessions);
        if let Some(state) = sessions.get_mut(session_id) {
            state.report_dropped(bytes);
        }
    }

    pub fn cleanup_session(&self, session_id: &str) {
        let removed = {
            let mut sessions = lock_recover(&self.sessions);
            sessions.remove(session_id)
        };
        if let Some(mut state) = removed {
            state.finish();
        }
    }

    pub fn disconnect_session(&self, session_id: &str) {
        let mut sessions = lock_recover(&self.sessions);
        if let Some(state) = sessions.get_mut(session_id) {
            state.disconnect();
        }
    }

    pub fn rekey_session(&self, old_session_id: &str, new_session_id: &str) {
        if old_session_id == new_session_id {
            return;
        }
        let mut sessions = lock_recover(&self.sessions);
        let Some(mut previous) = sessions.remove(old_session_id) else {
            return;
        };
        if let Some(early) = sessions.remove(new_session_id) {
            previous.absorb_early_state(early);
        }
        previous.disconnected = false;
        sessions.insert(new_session_id.to_string(), previous);
    }
}

fn recording_status_for_state(
    session_id: &str,
    state: &SessionCaptureState,
) -> Option<RecordingStatus> {
    let recording = state.recording.as_ref()?;
    Some(RecordingStatus {
        session_id: session_id.to_string(),
        state: state.recording_state,
        mode: recording.profile.mode,
        file_path: Some(recording.file_path.clone()),
        input_file_path: recording.input_file_path.clone(),
        started_at: Some(recording.context.started_at),
        written_bytes: recording.written_bytes,
        queued_bytes: 0,
        dropped_bytes: state.dropped_bytes,
        last_error: state.last_error.clone(),
    })
}

impl Default for RecordingManager {
    fn default() -> Self {
        Self::new()
    }
}

fn utf8_prefix_len(text: &str, limit: usize) -> usize {
    let mut end = limit.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    if end == 0 {
        text.chars().next().map_or(0, char::len_utf8)
    } else {
        end
    }
}

fn utf8_fragments(text: &str, limit: usize) -> impl Iterator<Item = &str> {
    let mut remaining = text;
    std::iter::from_fn(move || {
        if remaining.is_empty() {
            return None;
        }
        let end = utf8_prefix_len(remaining, limit);
        let (fragment, tail) = remaining.split_at(end);
        remaining = tail;
        Some(fragment)
    })
}

fn input_recording_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".input.bin");
    PathBuf::from(name)
}

fn append_named_suffix(path: &Path, suffix: &str) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let name = match path.extension() {
        Some(extension) => format!("{stem}-{suffix}.{}", extension.to_string_lossy()),
        None => format!("{stem}-{suffix}"),
    };
    path.with_file_name(name)
}

fn open_recording_pair(
    requested: &Path,
    behavior: ExistingFileBehavior,
    include_input: bool,
) -> Result<(PathBuf, File, Option<File>), RecordingError> {
    prepare_output_file_path(&requested.to_string_lossy())?;
    for index in 0..10_000 {
        let path = if index == 0 {
            requested.to_path_buf()
        } else {
            append_numbered_suffix(requested.to_path_buf(), index)
        };
        let input_path = input_recording_path(&path);
        if behavior == ExistingFileBehavior::Unique && include_input && input_path.exists() {
            continue;
        }
        let open = |path: &Path| {
            if behavior == ExistingFileBehavior::Overwrite {
                OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(path)
                    .map_err(RecordingError::Io)
            } else {
                open_recording_file(path, behavior)
            }
        };
        let file = match open(&path) {
            Err(RecordingError::Io(error))
                if behavior == ExistingFileBehavior::Unique
                    && error.kind() == std::io::ErrorKind::AlreadyExists =>
            {
                continue;
            }
            result => result?,
        };
        let input_file = if include_input {
            match open(&input_path) {
                Ok(file) => Some(file),
                Err(error) => {
                    drop(file);
                    if behavior == ExistingFileBehavior::Unique {
                        let _ = fs::remove_file(&path);
                        if matches!(&error, RecordingError::Io(error) if error.kind() == std::io::ErrorKind::AlreadyExists)
                        {
                            continue;
                        }
                    }
                    return Err(error);
                }
            }
        } else {
            None
        };
        if behavior == ExistingFileBehavior::Overwrite {
            if let Some(input) = input_file.as_ref() {
                if same_file::is_same_file(&path, &input_path)? {
                    return Err(RecordingError::Config(
                        "output and input destinations alias the same file".into(),
                    ));
                }
                input.metadata()?;
            }
            file.metadata()?;
            // Opening the companion may fail (permissions/directory); no existing bytes
            // are removed until both handles have been obtained successfully.
            file.set_len(0)?;
            if let Some(input) = input_file.as_ref() {
                input.set_len(0)?;
            }
        }
        return Ok((path, file, input_file));
    }
    Err(RecordingError::Config(
        "failed to reserve a unique recording file".into(),
    ))
}

fn reject_active_path(
    sessions: &HashMap<String, SessionCaptureState>,
    path: &Path,
) -> Result<(), RecordingError> {
    for file in sessions
        .values()
        .filter_map(|state| state.recording.as_ref())
    {
        for active in std::iter::once(&file.file_path).chain(file.input_file_path.iter()) {
            if path == active || (path.exists() && same_file::is_same_file(path, active)?) {
                return Err(RecordingError::Config(
                    "destination is an active recording file".into(),
                ));
            }
        }
    }
    Ok(())
}

pub fn safe_recording_name(name: &str) -> String {
    let mut safe = String::new();
    let mut last_was_separator = false;

    for ch in name.chars() {
        if ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            safe.push(ch);
            last_was_separator = false;
        } else if !last_was_separator {
            safe.push('_');
            last_was_separator = true;
        }
    }

    let safe = safe.trim_matches('_');
    if safe.is_empty() {
        "session".to_string()
    } else {
        safe.to_string()
    }
}

fn prepare_output_file_path(file_path: &str) -> Result<PathBuf, RecordingError> {
    let path = PathBuf::from(file_path);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    Ok(path)
}

fn resolve_recording_path(
    profile: &RecordingProfile,
    context: &RecordingContext,
    size_suffix: Option<u64>,
) -> Result<PathBuf, RecordingError> {
    let expanded = expand_recording_template(&profile.path_template, profile.mode, context);
    let mut path = PathBuf::new();
    for part in PathBuf::from(expanded).components() {
        match part {
            Component::Normal(segment) => {
                let segment = sanitize_path_segment(&segment.to_string_lossy());
                if !segment.is_empty() {
                    path.push(segment);
                }
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                path.push("safe");
            }
        }
    }
    if path.as_os_str().is_empty() {
        path.push(default_template_for_mode(profile.mode));
    }
    if let Some(index) = size_suffix {
        path = append_numbered_suffix(path, index);
    }
    Ok(profile.base_path.join(path))
}

fn expand_recording_template(
    template: &str,
    mode: RecordingMode,
    context: &RecordingContext,
) -> String {
    let mut expanded = if template.trim().is_empty() {
        default_template_for_mode(mode)
    } else {
        template.to_string()
    };
    let replacements = [
        ("session", context.session_name.clone()),
        ("session_id", context.session_id.clone()),
        ("session_short_id", short_session_id(&context.session_id)),
        (
            "connection_id",
            context.connection_id.clone().unwrap_or_default(),
        ),
        (
            "connection",
            context.connection_name.clone().unwrap_or_default(),
        ),
        ("group", context.group_path.clone().unwrap_or_default()),
        ("protocol", context.protocol.clone()),
        ("host", context.host.clone().unwrap_or_default()),
        (
            "port",
            context
                .port
                .map_or_else(String::new, |port| port.to_string()),
        ),
        ("username", context.username.clone().unwrap_or_default()),
        ("yyyy", format_time(context.started_at, "yyyy")),
        ("MM", format_time(context.started_at, "MM")),
        ("dd", format_time(context.started_at, "dd")),
        ("HH", format_time(context.started_at, "HH")),
        ("mm", format_time(context.started_at, "mm")),
        ("ss", format_time(context.started_at, "ss")),
        ("SSS", format_time(context.started_at, "SSS")),
    ];
    for (key, value) in replacements {
        expanded = expanded.replace(&format!("{{{key}}}"), &value);
    }
    expanded = expanded.replace(
        "{session_id:8}",
        short_session_id(&context.session_id).as_str(),
    );
    if mode == RecordingMode::Raw && expanded.ends_with(".log") && !expanded.ends_with(".raw.log") {
        expanded.truncate(expanded.len() - ".log".len());
        expanded.push_str(".raw.log");
    }
    expanded
}

fn default_template_for_mode(mode: RecordingMode) -> String {
    match mode {
        RecordingMode::Transcript => DEFAULT_RECORDING_PATH_TEMPLATE.to_string(),
        RecordingMode::Raw => {
            "{group}/{session}/{yyyy}-{MM}-{dd}/{HH}-{mm}-{ss}-{SSS}-{session_short_id}.raw.log"
                .to_string()
        }
    }
}

fn sanitize_path_segment(segment: &str) -> String {
    let trimmed = segment.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return String::new();
    }
    let mut safe = String::new();
    let mut last_was_replacement = false;
    for ch in trimmed.chars() {
        let invalid = matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\0')
            || matches!(ch, '/' | '\\')
            || ch.is_control();
        if invalid {
            if !last_was_replacement {
                safe.push('_');
                last_was_replacement = true;
            }
        } else {
            safe.push(ch);
            last_was_replacement = false;
        }
    }
    let safe = safe.trim_matches([' ', '.']).to_string();
    if safe.is_empty() {
        "session".to_string()
    } else {
        safe
    }
}

fn open_recording_file(
    path: &Path,
    behavior: ExistingFileBehavior,
) -> Result<File, RecordingError> {
    let mut options = OpenOptions::new();
    options.write(true).create(true);
    match behavior {
        ExistingFileBehavior::Unique => {
            options.create_new(true);
        }
        ExistingFileBehavior::Append => {
            options.append(true);
        }
        ExistingFileBehavior::Overwrite => {
            options.truncate(true);
        }
    }
    options.open(path).map_err(RecordingError::Io)
}

fn append_numbered_suffix(mut path: PathBuf, index: u64) -> PathBuf {
    let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "recording".to_string());
    let extension = path
        .extension()
        .map(|ext| ext.to_string_lossy().to_string());
    let mut filename = format!("{stem}-{index}");
    if let Some(extension) = extension {
        filename.push('.');
        filename.push_str(&extension);
    }
    path = parent;
    path.push(filename);
    path
}

fn format_record_parts(
    timestamp: &str,
    label: &str,
    data: &str,
    include_io_labels: bool,
    include_timestamps: bool,
) -> String {
    match (include_timestamps, include_io_labels) {
        (true, true) => format!("[{timestamp}] [{label}] {data}\n"),
        (true, false) => format!("[{timestamp}] {data}\n"),
        (false, true) => format!("[{label}] {data}\n"),
        (false, false) => format!("{data}\n"),
    }
}

fn chrono_timestamp() -> String {
    let now = OffsetDateTime::now_utc();
    now.format(time::macros::format_description!(
        "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3]"
    ))
    .unwrap_or_else(|_| "1970-01-01 00:00:00.000".to_string())
}

fn format_time(time: OffsetDateTime, token: &str) -> String {
    match token {
        "yyyy" => time.year().to_string(),
        "MM" => format!("{:02}", u8::from(time.month())),
        "dd" => format!("{:02}", time.day()),
        "HH" => format!("{:02}", time.hour()),
        "mm" => format!("{:02}", time.minute()),
        "ss" => format!("{:02}", time.second()),
        "SSS" => format!("{:03}", time.millisecond()),
        _ => String::new(),
    }
}

fn recording_day_key(time: OffsetDateTime) -> String {
    format!(
        "{}-{:02}-{:02}",
        time.year(),
        u8::from(time.month()),
        time.day()
    )
}

fn short_session_id(session_id: &str) -> String {
    session_id.chars().take(8).collect()
}

fn format_session_header(context: &RecordingContext) -> String {
    let mut lines = vec![
        "========== NyaTerm Session ==========".to_string(),
        format!("Session: {}", context.session_name),
        format!("Protocol: {}", context.protocol),
    ];
    if let Some(connection_id) = context
        .connection_id
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        lines.push(format!("Connection ID: {connection_id}"));
    }
    if let Some(host) = context.host.as_ref() {
        let host_line = context
            .port
            .map_or_else(|| host.clone(), |port| format!("{host}:{port}"));
        lines.push(format!("Host: {host_line}"));
    }
    if let Some(username) = context.username.as_ref().filter(|value| !value.is_empty()) {
        lines.push(format!("User: {username}"));
    }
    lines.push(format!(
        "Started: {}",
        context
            .started_at
            .format(time::macros::format_description!(
                "[year]-[month]-[day] [hour]:[minute]:[second]"
            ))
            .unwrap_or_else(|_| "unknown".to_string())
    ));
    lines.push("======================================".to_string());
    lines.push(String::new());
    lines.join("\n")
}

fn format_session_footer(context: &RecordingContext, reason: &str) -> String {
    let stopped_at = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
    let duration = stopped_at - context.started_at;
    format!(
        "\n========== NyaTerm Session End ==========\nStopped: {}\nReason: {reason}\nDuration: {}s\n=========================================\n",
        stopped_at
            .format(time::macros::format_description!(
                "[year]-[month]-[day] [hour]:[minute]:[second]"
            ))
            .unwrap_or_else(|_| "unknown".to_string()),
        duration.whole_seconds().max(0)
    )
}

fn format_raw_input_bytes(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().saturating_mul(3).saturating_sub(1));
    for (index, byte) in data.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalInputEscapeState {
    Esc,
    Csi,
    Ss3,
    String,
    StringEsc,
}

fn consume_terminal_input_escape_state(
    data: &[u8],
    mut index: usize,
    mut state: TerminalInputEscapeState,
) -> (usize, Option<TerminalInputEscapeState>) {
    loop {
        match state {
            TerminalInputEscapeState::Esc => {
                let Some(byte) = data.get(index).copied() else {
                    return (index, Some(TerminalInputEscapeState::Esc));
                };
                index += 1;
                match byte {
                    b'[' => state = TerminalInputEscapeState::Csi,
                    b'O' => state = TerminalInputEscapeState::Ss3,
                    b']' | b'P' | b'X' | b'^' | b'_' => state = TerminalInputEscapeState::String,
                    _ => return (index, None),
                }
            }
            TerminalInputEscapeState::Csi => {
                while let Some(byte) = data.get(index).copied() {
                    index += 1;
                    if (0x40..=0x7e).contains(&byte) {
                        return (index, None);
                    }
                }
                return (index, Some(TerminalInputEscapeState::Csi));
            }
            TerminalInputEscapeState::Ss3 => {
                if index < data.len() {
                    return (index + 1, None);
                }
                return (index, Some(TerminalInputEscapeState::Ss3));
            }
            TerminalInputEscapeState::String => {
                while let Some(byte) = data.get(index).copied() {
                    index += 1;
                    if byte == b'\x07' {
                        return (index, None);
                    }
                    if byte == b'\x1b' {
                        state = TerminalInputEscapeState::StringEsc;
                        break;
                    }
                }
                if index >= data.len() && state == TerminalInputEscapeState::String {
                    return (index, Some(TerminalInputEscapeState::String));
                }
            }
            TerminalInputEscapeState::StringEsc => {
                let Some(byte) = data.get(index).copied() else {
                    return (index, Some(TerminalInputEscapeState::StringEsc));
                };
                index += 1;
                if byte == b'\\' || byte == b'\x07' {
                    return (index, None);
                }
                state = TerminalInputEscapeState::String;
            }
        }
    }
}

fn next_utf8_char(data: &[u8], index: usize) -> Option<(char, usize)> {
    let suffix = &data[index..];
    let text = match std::str::from_utf8(suffix) {
        Ok(text) => text,
        Err(error) if error.valid_up_to() > 0 => {
            std::str::from_utf8(&suffix[..error.valid_up_to()]).ok()?
        }
        Err(_) => return None,
    };
    let ch = text.chars().next()?;
    Some((ch, index + ch.len_utf8()))
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

fn lock_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

enum HistoryMatcher {
    Literal {
        needle: String,
        case_sensitive: bool,
        whole_word: bool,
    },
    Regex(regex::Regex),
}

impl HistoryMatcher {
    fn new(
        query: &str,
        case_sensitive: bool,
        regex: bool,
        whole_word: bool,
    ) -> Result<Self, RecordingError> {
        if regex {
            let pattern = if whole_word {
                format!(r"\b(?:{query})\b")
            } else {
                query.to_string()
            };
            let compiled = RegexBuilder::new(&pattern)
                .case_insensitive(!case_sensitive)
                .build()?;
            return Ok(Self::Regex(compiled));
        }

        let needle = if case_sensitive {
            query.to_string()
        } else {
            query.to_lowercase()
        };

        Ok(Self::Literal {
            needle,
            case_sensitive,
            whole_word,
        })
    }

    fn find(&self, haystack: &str) -> Option<(usize, usize)> {
        match self {
            Self::Literal {
                needle,
                case_sensitive,
                whole_word,
            } => {
                let searchable = if *case_sensitive {
                    haystack.to_string()
                } else {
                    haystack.to_lowercase()
                };
                find_literal_match(&searchable, needle, *whole_word)
            }
            Self::Regex(regex) => regex
                .find(haystack)
                .map(|found| (found.start(), found.end())),
        }
    }
}

fn find_literal_match(haystack: &str, needle: &str, whole_word: bool) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }

    let mut offset = 0;
    while offset <= haystack.len() {
        let relative = haystack[offset..].find(needle)?;
        let start = offset + relative;
        let end = start + needle.len();

        if !whole_word || is_word_boundary_match(haystack, start, end) {
            return Some((start, end));
        }

        offset = end;
    }

    None
}

fn is_word_boundary_match(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();

    before.is_none_or(|ch| !is_word_char(ch)) && after.is_none_or(|ch| !is_word_char(ch))
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn context_records(
    records: &[TranscriptRecord],
    index: usize,
    count: usize,
    before: bool,
) -> Vec<String> {
    if count == 0 {
        return Vec::new();
    }

    if before {
        let start = index.saturating_sub(count);
        return records[start..index]
            .iter()
            .map(|record| record.data.clone())
            .collect();
    }

    let start = index.saturating_add(1);
    let end = start.saturating_add(count).min(records.len());
    records[start..end]
        .iter()
        .map(|record| record.data.clone())
        .collect()
}

#[cfg(test)]
fn strip_terminal_control_sequences(text: &str) -> String {
    RecordingSanitizer::default().feed(text)
}

#[cfg(test)]
mod tests {
    use super::{
        RecordingManager, consume_matching_prefix, safe_recording_name, strip_one_leading_newline,
        strip_terminal_control_sequences,
    };
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn input_enabled_manager() -> RecordingManager {
        let manager = RecordingManager::new();
        manager.set_history_include_input(true);
        manager
    }

    fn unique_path(name: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir()
            .join(format!("nyaterm-recording-{name}-{nanos}.log"))
            .to_string_lossy()
            .to_string()
    }

    #[test]
    fn transcript_survives_disconnect_and_multiple_rekeys() {
        let manager = input_enabled_manager();
        let path = unique_path("rekey-transcript");
        manager.write_output("s1", "first\n");
        manager.disconnect_session("s1");
        manager.disconnect_session("s1");
        manager.save_transcript("s1", &path, true, false).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("first"));
        manager.rekey_session("s1", "s2");
        manager.write_output("s2", "second\n");
        manager.disconnect_session("s2");
        manager.rekey_session("s2", "s3");
        manager.write_output("s3", "third\n");
        manager.save_transcript("s3", &path, true, false).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.find("first").unwrap() < text.find("second").unwrap());
        assert!(text.find("second").unwrap() < text.find("third").unwrap());
        assert_eq!(text.matches("[SYSTEM] Session disconnected").count(), 2);
        manager.cleanup_session("s3");
        manager.save_transcript("s3", &path, true, false).unwrap();
        assert!(fs::read_to_string(&path).unwrap().is_empty());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn recording_rekey_keeps_file_and_merges_early_output_without_crossing_sessions() {
        let manager = input_enabled_manager();
        let path = unique_path("rekey-file");
        let other_path = unique_path("rekey-other");
        let transcript_path = unique_path("rekey-merged");
        manager.start("s1", &path, true, false).unwrap();
        manager.write_output("s1", "before\n");
        manager.write_output("other", "other pane\n");
        manager.disconnect_session("s1");
        manager.write_output("s2", "early banner\n");
        manager.rekey_session("s1", "s2");
        assert!(manager.status("s1").is_none());
        assert!(manager.is_recording("s2"));
        manager.write_output("s2", "after\n");
        manager.stop("s2").unwrap();
        manager
            .save_transcript("s2", &transcript_path, true, false)
            .unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.find("before").unwrap() < text.find("early banner").unwrap());
        assert!(text.find("early banner").unwrap() < text.find("after").unwrap());
        assert!(!text.contains("other pane"));
        assert_eq!(fs::read_to_string(&transcript_path).unwrap(), text);
        manager
            .save_transcript("other", &other_path, true, false)
            .unwrap();
        assert!(
            fs::read_to_string(&other_path)
                .unwrap()
                .contains("other pane")
        );
        manager.cleanup_session("s2");
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(other_path);
        let _ = fs::remove_file(transcript_path);
    }

    #[test]
    fn raw_recording_rekey_preserves_early_wire_output() {
        let manager = input_enabled_manager();
        let path = std::path::PathBuf::from(unique_path("raw-rekey"));
        let profile = super::RecordingProfile {
            mode: super::RecordingMode::Raw,
            base_path: path.parent().unwrap().to_path_buf(),
            path_template: path.file_name().unwrap().to_string_lossy().to_string(),
            include_timestamps: false,
            include_io_labels: false,
            include_session_metadata: false,
            rotation: super::RecordingRotationPolicy::Session,
            existing_file_behavior: super::ExistingFileBehavior::Overwrite,
            include_binary_transfer_payloads: false,
            include_input: true,
        };
        let context = super::RecordingContext {
            session_id: "s1".to_string(),
            session_name: "session".to_string(),
            connection_id: None,
            connection_name: None,
            group_path: None,
            protocol: "terminal".to_string(),
            host: None,
            port: None,
            username: None,
            started_at: time::OffsetDateTime::now_utc(),
        };
        manager
            .start_with_profile("s1", context, profile, Some(path.clone()))
            .unwrap();
        manager.write_raw_output("s1", b"before\n");
        manager.disconnect_session("s1");
        manager.write_raw_output("s2", b"\x1b[32mearly\x1b[0m\n");
        manager.rekey_session("s1", "s2");
        manager.write_raw_output("s2", b"after\n");
        manager.stop("s2").unwrap();
        assert_eq!(
            fs::read(&path).unwrap(),
            b"before\n\x1b[32mearly\x1b[0m\nafter\n"
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn safe_recording_name_preserves_readable_parts() {
        assert_eq!(safe_recording_name(""), "session");
        assert_eq!(safe_recording_name("my session!/prod"), "my_session_prod");
        assert_eq!(safe_recording_name("ssh.host_01-prod"), "ssh.host_01-prod");
    }

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
    fn strips_unknown_escape_with_multibyte_replacement_without_panicking() {
        let raw = format!("before\x1b{}after\n", char::REPLACEMENT_CHARACTER);
        let cleaned = strip_terminal_control_sequences(&raw);
        assert_eq!(cleaned, "beforeafter\n");
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

    #[test]
    fn writes_recording_with_and_without_io_labels() {
        let manager = input_enabled_manager();
        let labeled_path = unique_path("labels");
        manager.start("s1", &labeled_path, true, true).unwrap();
        manager.write_input("s1", b"echo hi\r");
        manager.write_output("s1", "echo hi\r\nhi\n");
        manager.stop("s1").unwrap();

        let labeled = fs::read_to_string(&labeled_path).unwrap();
        assert!(labeled.contains("[INPUT] echo hi"));
        assert!(labeled.contains("[OUTPUT] hi"));

        let plain_path = unique_path("plain");
        manager.start("s1", &plain_path, false, true).unwrap();
        manager.write_output("s1", "done\n");
        manager.stop("s1").unwrap();

        let plain = fs::read_to_string(&plain_path).unwrap();
        assert!(!plain.contains("[INPUT]"));
        assert!(!plain.contains("[OUTPUT]"));
        assert!(plain.contains("done"));

        let _ = fs::remove_file(labeled_path);
        let _ = fs::remove_file(plain_path);
    }

    #[test]
    fn writes_recording_without_timestamps() {
        let manager = input_enabled_manager();

        let labeled_path = unique_path("no-timestamp-labels");
        manager.start("s1", &labeled_path, true, false).unwrap();
        manager.write_output("s1", "done\n");
        manager.stop("s1").unwrap();

        let labeled = fs::read_to_string(&labeled_path).unwrap();
        assert_eq!(labeled, "[OUTPUT] done\n");

        let plain_path = unique_path("no-timestamp-plain");
        manager.start("s1", &plain_path, false, false).unwrap();
        manager.write_output("s1", "plain\n");
        manager.stop("s1").unwrap();

        let plain = fs::read_to_string(&plain_path).unwrap();
        assert_eq!(plain, "plain\n");

        let _ = fs::remove_file(labeled_path);
        let _ = fs::remove_file(plain_path);
    }

    #[test]
    fn text_input_records_logical_utf8_text() {
        let manager = input_enabled_manager();
        let path = unique_path("logical-input");

        manager.start("s1", &path, true, false).unwrap();
        manager.write_input("s1", "echo 测试\r".as_bytes());
        manager.stop("s1").unwrap();

        let recorded = fs::read_to_string(&path).unwrap();
        assert_eq!(recorded, "[INPUT] echo 测试\n");
        assert!(!recorded.contains(char::REPLACEMENT_CHARACTER));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn tab_completion_uses_terminal_line_and_drops_partial_repaint() {
        let manager = input_enabled_manager();
        let path = unique_path("tab-completion");
        manager.write_input("s1", b"vi ins");
        manager.write_input("s1", b"\t");
        manager.write_output("s1", "\r\x1b[2Kvi install-node-exporter.sh");
        manager.resync_input_line("s1", "[root@rocky9 ~]# vi install-node-exporter.sh");
        manager.write_input("s1", b"\r");
        manager.save_transcript("s1", &path, true, false).unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[INPUT] vi install-node-exporter.sh\n"
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn tab_completion_keeps_complete_candidate_lines() {
        let manager = input_enabled_manager();
        let path = unique_path("tab-candidates");
        manager.write_input("s1", b"vi ins");
        manager.write_input("s1", b"\t\t");
        manager.write_output("s1", "install-a\ninstall-b\n[root@rocky9 ~]# vi install-");
        manager.resync_input_line("s1", "[root@rocky9 ~]# vi install-");
        manager.write_input("s1", b"node-exporter.sh\r");
        manager.save_transcript("s1", &path, true, false).unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[OUTPUT] install-a\n[OUTPUT] install-b\n[INPUT] vi install-node-exporter.sh\n"
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn tab_completion_recovers_when_more_text_is_typed_before_enter() {
        let manager = input_enabled_manager();
        let path = unique_path("tab-continued-input");
        manager.write_input("s1", b"vi ins\t");
        manager.write_output("s1", "\r\x1b[2Kvi install-node-exporter.sh");
        manager.write_input("s1", b".bak");
        manager.resync_input_line("s1", "[root@rocky9 ~]# vi install-node-exporter.sh.bak");
        manager.write_input("s1", b"\r");
        manager.save_transcript("s1", &path, true, false).unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[INPUT] vi install-node-exporter.sh.bak\n"
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn resync_without_tab_preserves_manual_input_and_backspace() {
        let manager = input_enabled_manager();
        let path = unique_path("manual-input");
        manager.write_input("s1", b"vi nginx_cp.confx\x7f");
        manager.write_input("s1", b"\x1b[D");
        manager.resync_input_line("s1", "unrelated screen line");
        manager.write_input("s1", b"\r");
        manager.save_transcript("s1", &path, true, false).unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[INPUT] vi nginx_cp.conf\n"
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn raw_input_records_exact_bytes_as_hex() {
        let manager = input_enabled_manager();
        let path = unique_path("raw-input");
        let bytes = [0x00, 0xff, 0x1b, b'[', b'A'];

        manager.start("s1", &path, true, false).unwrap();
        manager.write_raw_input("s1", &bytes);
        manager.stop("s1").unwrap();

        let recorded = fs::read_to_string(&path).unwrap();
        assert_eq!(recorded, "[RAW_INPUT] 00 ff 1b 5b 41\n");
        assert!(!recorded.contains(char::REPLACEMENT_CHARACTER));

        manager.write_raw_input("s2", &bytes);
        let transcript_path = unique_path("raw-input-transcript");
        manager
            .save_transcript("s2", &transcript_path, true, false)
            .unwrap();
        let transcript = fs::read_to_string(&transcript_path).unwrap();
        assert_eq!(transcript, "[RAW_INPUT] 00 ff 1b 5b 41\n");
        assert!(!transcript.contains(char::REPLACEMENT_CHARACTER));

        let _ = fs::remove_file(path);
        let _ = fs::remove_file(transcript_path);
    }

    #[test]
    fn raw_recording_keeps_tab_and_terminal_bytes_unchanged() {
        let manager = input_enabled_manager();
        let path = std::path::PathBuf::from(unique_path("raw-tab"));
        let profile = super::RecordingProfile {
            mode: super::RecordingMode::Raw,
            base_path: path.parent().unwrap().to_path_buf(),
            path_template: path.file_name().unwrap().to_string_lossy().to_string(),
            include_timestamps: false,
            include_io_labels: false,
            include_session_metadata: false,
            rotation: super::RecordingRotationPolicy::Session,
            existing_file_behavior: super::ExistingFileBehavior::Overwrite,
            include_binary_transfer_payloads: false,
            include_input: true,
        };
        let context = super::RecordingContext {
            session_id: "s1".to_string(),
            session_name: "session".to_string(),
            connection_id: None,
            connection_name: None,
            group_path: None,
            protocol: "terminal".to_string(),
            host: None,
            port: None,
            username: None,
            started_at: time::OffsetDateTime::now_utc(),
        };
        manager
            .start_with_profile("s1", context, profile, Some(path.clone()))
            .unwrap();
        manager.write_raw_input("s1", b"vi ins\t");
        manager.write_raw_output("s1", b"\r\x1b[2Kvi install-node-exporter.sh");
        manager.resync_input_line("s1", "[root@rocky9 ~]# vi install-node-exporter.sh");
        manager.write_raw_input("s1", b"\r");
        manager.stop("s1").unwrap();

        assert_eq!(
            fs::read(&path).unwrap(),
            b"\r\x1b[2Kvi install-node-exporter.sh"
        );
        assert_eq!(
            fs::read(super::input_recording_path(&path)).unwrap(),
            b"vi ins\t\r"
        );
        let _ = fs::remove_file(super::input_recording_path(&path));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn terminal_protocol_input_sequences_do_not_pollute_recorded_text() {
        let manager = input_enabled_manager();
        let path = unique_path("protocol-input");

        manager.write_input("s1", b"\x1b[A");
        manager.write_input("s1", b"echo ");
        manager.write_input("s1", b"\x1b[<0;12;5M");
        manager.write_input("s1", "界".as_bytes());
        manager.write_input("s1", b"\r");
        manager.save_transcript("s1", &path, true, false).unwrap();

        let transcript = fs::read_to_string(&path).unwrap();
        assert_eq!(transcript, "[INPUT] echo 界\n");
        assert!(!transcript.contains("[A"));
        assert!(!transcript.contains("<0;12;5M"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn split_terminal_protocol_input_sequences_do_not_pollute_recorded_text() {
        let manager = input_enabled_manager();
        let path = unique_path("split-protocol-input");

        manager.write_input("s1", b"echo ");
        manager.write_input("s1", b"\x1b[<0;");
        manager.write_input("s1", b"12;5M");
        manager.write_input("s1", b"\x1b]");
        manager.write_input("s1", b"52;c;clipboard");
        manager.write_input("s1", b"\x1b");
        manager.write_input("s1", b"\\");
        manager.write_input("s1", b"\x1bO");
        manager.write_input("s1", b"A");
        manager.write_input("s1", b"done\r");
        manager.save_transcript("s1", &path, true, false).unwrap();

        let transcript = fs::read_to_string(&path).unwrap();
        assert_eq!(transcript, "[INPUT] echo done\n");
        assert!(!transcript.contains("<0;"));
        assert!(!transcript.contains("clipboard"));
        assert!(!transcript.contains("\x1b"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn saves_memory_transcript_and_trims_old_records() {
        let manager = input_enabled_manager();
        manager.set_memory_limit(90);
        manager.write_output("s1", "first line\n");
        manager.write_output("s1", "second line\n");
        manager.write_output("s1", "third line\n");

        let path = unique_path("memory");
        manager.save_transcript("s1", &path, true, true).unwrap();
        let saved = fs::read_to_string(&path).unwrap();

        assert!(!saved.contains("first line"));
        assert!(saved.contains("third line"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn terminal_history_search_finds_literal_matches() {
        let manager = input_enabled_manager();
        manager.write_output("s1", "alpha\nbeta install\nbeta done\n");

        let result = manager
            .search_history(super::TerminalHistorySearchRequest {
                session_id: "s1".to_string(),
                query: "beta".to_string(),
                case_sensitive: false,
                regex: false,
                whole_word: false,
                limit: Some(100),
                context_before: Some(1),
                context_after: Some(1),
                max_lines: None,
            })
            .unwrap();

        assert_eq!(result.total, 2);
        assert_eq!(result.results.len(), 2);
        assert_eq!(result.results[0].line_number, 2);
        assert_eq!(result.results[0].before, vec!["alpha"]);
        assert_eq!(result.results[0].after, vec!["beta done"]);
        assert_eq!(result.results[0].source, "output");
    }

    #[test]
    fn terminal_history_search_honors_case_and_whole_word() {
        let manager = input_enabled_manager();
        manager.write_output("s1", "install\nInstall\ninstaller\n");

        let case_sensitive = manager
            .search_history(super::TerminalHistorySearchRequest {
                session_id: "s1".to_string(),
                query: "Install".to_string(),
                case_sensitive: true,
                regex: false,
                whole_word: false,
                limit: Some(100),
                context_before: Some(0),
                context_after: Some(0),
                max_lines: None,
            })
            .unwrap();
        assert_eq!(case_sensitive.total, 1);
        assert_eq!(case_sensitive.results[0].preview, "Install");

        let whole_word = manager
            .search_history(super::TerminalHistorySearchRequest {
                session_id: "s1".to_string(),
                query: "install".to_string(),
                case_sensitive: false,
                regex: false,
                whole_word: true,
                limit: Some(100),
                context_before: Some(0),
                context_after: Some(0),
                max_lines: None,
            })
            .unwrap();
        assert_eq!(whole_word.total, 2);
    }

    #[test]
    fn terminal_history_search_supports_regex_limit_and_truncation() {
        let manager = input_enabled_manager();
        manager.write_output("s1", "error 100\nerror 200\nok\n");

        let result = manager
            .search_history(super::TerminalHistorySearchRequest {
                session_id: "s1".to_string(),
                query: r"error \d+".to_string(),
                case_sensitive: false,
                regex: true,
                whole_word: false,
                limit: Some(1),
                context_before: Some(0),
                context_after: Some(0),
                max_lines: None,
            })
            .unwrap();

        assert_eq!(result.total, 2);
        assert_eq!(result.results.len(), 1);
        assert!(result.truncated);
        assert_eq!(result.results[0].preview, "error 100");
    }

    #[test]
    fn recording_does_not_backfill_existing_memory() {
        let manager = input_enabled_manager();
        manager.write_output("s1", "before\n");

        let path = unique_path("no-backfill");
        manager.start("s1", &path, true, true).unwrap();
        manager.write_output("s1", "after\n");
        manager.stop("s1").unwrap();

        let recorded = fs::read_to_string(&path).unwrap();
        assert!(!recorded.contains("before"));
        assert!(recorded.contains("after"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn recording_does_not_backfill_partial_output_buffer() {
        let manager = input_enabled_manager();
        manager.write_output("s1", "prompt without newline");

        let path = unique_path("no-partial-backfill");
        manager.start("s1", &path, true, true).unwrap();
        manager.write_output("s1", "\nafter\n");
        manager.stop("s1").unwrap();

        let recorded = fs::read_to_string(&path).unwrap();
        assert!(!recorded.contains("prompt without newline"));
        assert!(recorded.contains("after"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn rotation_write_failure_marks_recording_failed_and_surfaces_on_stop() {
        let manager = input_enabled_manager();
        let initial_path = std::path::PathBuf::from(unique_path("rotation-initial"));
        let blocked_base_path = std::path::PathBuf::from(unique_path("rotation-blocked-base"));
        fs::create_dir(super::append_numbered_suffix(initial_path.clone(), 1)).unwrap();
        let context = super::RecordingContext {
            session_id: "s1".to_string(),
            session_name: "session".to_string(),
            connection_id: None,
            connection_name: None,
            group_path: None,
            protocol: "local".to_string(),
            host: None,
            port: None,
            username: None,
            started_at: time::OffsetDateTime::now_utc(),
        };
        let profile = super::RecordingProfile {
            mode: super::RecordingMode::Transcript,
            base_path: blocked_base_path.clone(),
            path_template: "rotated.log".to_string(),
            include_timestamps: false,
            include_io_labels: true,
            include_session_metadata: false,
            rotation: super::RecordingRotationPolicy::Size { max_bytes: 1 },
            existing_file_behavior: super::ExistingFileBehavior::Overwrite,
            include_binary_transfer_payloads: false,
            include_input: true,
        };

        manager
            .start_with_profile("s1", context, profile, Some(initial_path.clone()))
            .unwrap();
        manager.write_output("s1", "first\n");
        manager.write_output("s1", "rotation must fail\n");

        let status = manager
            .status("s1")
            .expect("recording should remain observable");
        assert_eq!(status.state, super::RecordingStatusState::Failed);
        assert!(status.last_error.is_some());
        let error = manager
            .stop("s1")
            .expect_err("stop must surface the failure");
        assert!(matches!(error, super::RecordingError::Runtime(_)));

        let _ = fs::remove_file(&initial_path);
        let _ = fs::remove_file(blocked_base_path);
        let _ = fs::remove_dir(super::append_numbered_suffix(initial_path, 1));
    }

    #[test]
    fn transcript_applies_cursor_back_so_pager_prompts_are_erased() {
        let manager = input_enabled_manager();
        let path = unique_path("pager-cursor-back");
        manager.start("s1", &path, true, true).unwrap();
        // Huawei VRP: "  ---- More ----", then on space ESC[16D, 16 spaces, ESC[16D.
        manager.write_output("s1", "device board 1 board-type LSW\n#\n  ---- More ----");
        manager.write_output(
            "s1",
            "\x1b[16D                \x1b[16Dauthentication-profile name\n",
        );
        // Backspace and CSI D in one chunk, with output after the newline.
        manager.write_output("s1", "tpyo\x08\x08\x08ypo\nnext\x1b[2Dxt\n");
        manager.stop("s1").unwrap();

        let recorded = fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = recorded
            .lines()
            .filter_map(|line| line.split_once("[OUTPUT] ").map(|(_, text)| text))
            .collect();
        assert_eq!(
            lines,
            [
                "device board 1 board-type LSW",
                "#",
                "authentication-profile name",
                "typo",
                "next",
            ],
            "{recorded}"
        );

        let _ = fs::remove_file(path);
    }
}
