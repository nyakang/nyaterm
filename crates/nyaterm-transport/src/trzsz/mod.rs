//! trzsz transfer trigger detection.
//!
//! This identifies the terminal-side trigger marker. The transfer engine is
//! separate; callers that reserve the pre-parser trzsz slot can filter trigger
//! markers out of terminal-visible bytes while surfacing an unsupported state.

use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Write},
    path::PathBuf,
    time::{Duration, Instant},
};

use base64::Engine as _;
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use serde::{Deserialize, Serialize};

const TRZSZ_PREFIX: &[u8] = b"::TRZSZ:TRANSFER:";
// Bare colons are common shell echoes. A recognizable marker gets more time
// for fragmented delivery, but neither prefix may hide terminal text forever.
const TRZSZ_COLON_IDLE_TIMEOUT: Duration = Duration::from_millis(50);
const TRZSZ_TRIGGER_IDLE_TIMEOUT: Duration = Duration::from_secs(1);
const TRZSZ_MAX_TRIGGER_LEN: usize = 96;
const TRZSZ_MAX_PROTOCOL_LINE_LEN: usize = 1024 * 1024;
const TRZSZ_MAX_DECODED_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;
const TRZSZ_UPLOAD_DATA_CHUNK_SIZE: usize = 256 * 1024;
// Protocol 3 switched NAME and its SUCC reply to sourceFile/targetFile JSON.
// The negotiated value comes from CFG; a server that omits `protocol` keeps
// the legacy plain-text semantics, matching trzsz-go's default of 0.
const TRZSZ_JSON_NAME_PROTOCOL: i64 = 3;
const TRZSZ_STALE_TRIGGER_MARKERS: [&[u8]; 5] =
    [b"#CFG:", b"Saved", b"Cancelled", b"Stopped", b"Interrupted"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrzszMode {
    Send,
    Receive,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrzszTrigger {
    pub mode: TrzszMode,
    pub version: String,
    pub unique_id: Option<String>,
    /// Official trzsz clients treat unique id `1` or a 13-digit id ending in
    /// `10` as a Windows server marker, which changes newline/binary handling.
    pub remote_is_windows: bool,
    pub tunnel_port: Option<u16>,
    pub raw: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrzszProtocolPayload {
    /// `#TYPE:<integer>` frames such as `#NUM`, `#SIZE`, and binary `#DATA`.
    Integer(i64),
    /// `#TYPE:<zlib+base64>` frames such as `#ACT`, `#CFG`, `#SUCC`, `#fail`, and
    /// text-mode `#DATA`.
    EncodedBytes(Vec<u8>),
    /// A syntactically valid trzsz protocol line whose payload cannot be
    /// classified without more transfer state.
    Raw(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrzszProtocolFrame {
    pub frame_type: String,
    pub payload: TrzszProtocolPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrzszAction {
    pub lang: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub confirm: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newline: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<i64>,
    #[serde(default, rename = "binary", skip_serializing_if = "is_false")]
    pub support_binary: bool,
    #[serde(default, rename = "support_dir", skip_serializing_if = "is_false")]
    pub support_directory: bool,
    #[serde(default, rename = "tunnel", skip_serializing_if = "is_false")]
    pub tunnel_connected: bool,
    #[serde(default, rename = "fork", skip_serializing_if = "is_false")]
    pub support_fork: bool,
    #[serde(default, rename = "tmuxcc", skip_serializing_if = "is_false")]
    pub tmux_integration: bool,
}

impl TrzszAction {
    pub fn local_default(remote_is_windows: bool) -> Self {
        Self {
            lang: "rust".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            confirm: true,
            newline: remote_is_windows.then(|| "!\n".to_string()),
            protocol: Some(4),
            support_binary: true,
            support_directory: true,
            tunnel_connected: false,
            support_fork: false,
            tmux_integration: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrzszConfig {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lang: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub quiet: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub binary: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub directory: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub overwrite: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newline: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<i64>,
    #[serde(default, rename = "bufsize", skip_serializing_if = "Option::is_none")]
    pub max_buf_size: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compress: Option<i64>,
    #[serde(default, rename = "fork", skip_serializing_if = "is_false")]
    pub fork: bool,
}

impl TrzszConfig {
    pub fn local_default(action: Option<&TrzszAction>, directory: bool) -> Self {
        Self {
            lang: "rust".to_string(),
            quiet: false,
            binary: action.is_some_and(|action| action.support_binary),
            directory,
            overwrite: false,
            timeout: Some(20),
            newline: action.and_then(|action| action.newline.clone()),
            protocol: action
                .and_then(|action| action.protocol)
                .map(|protocol| protocol.min(4)),
            max_buf_size: Some(10 * 1024 * 1024),
            compress: None,
            fork: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrzszTransferPhase {
    Idle,
    Triggered,
    ActionNegotiated,
    Configured,
    Transferring,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrzszTransferEvent {
    Started {
        mode: TrzszMode,
        remote_is_windows: bool,
    },
    Action {
        action: TrzszAction,
    },
    Config {
        config: TrzszConfig,
    },
    Metadata {
        frame_type: String,
        payload: TrzszProtocolPayload,
    },
    Data {
        payload: TrzszProtocolPayload,
    },
    Success {
        payload: TrzszProtocolPayload,
    },
    Failure {
        message: String,
    },
    Exit {
        message: String,
    },
    Unknown {
        frame: TrzszProtocolFrame,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrzszTransferState {
    pub phase: TrzszTransferPhase,
    pub mode: Option<TrzszMode>,
    pub remote_is_windows: bool,
    pub action: Option<TrzszAction>,
    pub config: Option<TrzszConfig>,
}

impl Default for TrzszTransferState {
    fn default() -> Self {
        Self {
            phase: TrzszTransferPhase::Idle,
            mode: None,
            remote_is_windows: false,
            action: None,
            config: None,
        }
    }
}

impl TrzszTransferState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe_trigger(&mut self, trigger: &TrzszTrigger) -> TrzszTransferEvent {
        self.phase = TrzszTransferPhase::Triggered;
        self.mode = Some(trigger.mode);
        self.remote_is_windows = trigger.remote_is_windows;
        self.action = None;
        self.config = None;
        TrzszTransferEvent::Started {
            mode: trigger.mode,
            remote_is_windows: trigger.remote_is_windows,
        }
    }

    pub fn observe_frame(&mut self, frame: TrzszProtocolFrame) -> TrzszTransferEvent {
        match frame.frame_type.to_ascii_uppercase().as_str() {
            "ACT" => {
                if let Some(action) = parse_trzsz_action_frame(&frame) {
                    self.phase = TrzszTransferPhase::ActionNegotiated;
                    self.action = Some(action.clone());
                    TrzszTransferEvent::Action { action }
                } else {
                    TrzszTransferEvent::Unknown { frame }
                }
            }
            "CFG" => {
                if let Some(config) = parse_trzsz_config_frame(&frame) {
                    self.phase = TrzszTransferPhase::Configured;
                    self.config = Some(config.clone());
                    TrzszTransferEvent::Config { config }
                } else {
                    TrzszTransferEvent::Unknown { frame }
                }
            }
            "NUM" | "NAME" | "SIZE" => {
                self.phase = TrzszTransferPhase::Transferring;
                TrzszTransferEvent::Metadata {
                    frame_type: frame.frame_type,
                    payload: frame.payload,
                }
            }
            "DATA" => {
                self.phase = TrzszTransferPhase::Transferring;
                TrzszTransferEvent::Data {
                    payload: frame.payload,
                }
            }
            "SUCC" => {
                self.phase = TrzszTransferPhase::Completed;
                TrzszTransferEvent::Success {
                    payload: frame.payload,
                }
            }
            "FAIL" => {
                self.phase = TrzszTransferPhase::Failed;
                TrzszTransferEvent::Failure {
                    message: trzsz_payload_message(&frame.payload),
                }
            }
            "EXIT" => {
                self.phase = TrzszTransferPhase::Failed;
                TrzszTransferEvent::Exit {
                    message: trzsz_payload_message(&frame.payload),
                }
            }
            _ => TrzszTransferEvent::Unknown { frame },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrzszDetectResult {
    /// No complete trigger was detected. `passthrough` is known-safe terminal text.
    NoMatch { passthrough: Vec<u8> },
    /// A trzsz trigger marker was detected in the byte stream.
    Detected {
        trigger: TrzszTrigger,
        passthrough: Vec<u8>,
        /// Bytes after the trigger in the same input chunk.
        remaining: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrzszFilteredOutput {
    /// Bytes that are safe to pass to terminal parsing/recording.
    pub passthrough: Vec<u8>,
    /// Trigger markers consumed from the terminal-visible stream.
    pub triggers: Vec<TrzszTrigger>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrzszOutputEvent {
    Passthrough(Vec<u8>),
    Trigger(TrzszTrigger),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrzszOutputScan {
    pub events: Vec<TrzszOutputEvent>,
}

#[derive(Debug, Default)]
pub struct TrzszDetector {
    pending: Vec<u8>,
    pending_updated_at: Option<Instant>,
    seen_unique_ids: HashMap<String, usize>,
}

impl TrzszDetector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_idle(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn output_may_contain_trigger(data: &[u8]) -> bool {
        data.windows(TRZSZ_PREFIX.len())
            .any(|window| window == TRZSZ_PREFIX)
            || (1..TRZSZ_PREFIX.len())
                .any(|len| data.len() >= len && TRZSZ_PREFIX.starts_with(&data[data.len() - len..]))
    }

    pub fn feed(&mut self, data: &[u8]) -> TrzszDetectResult {
        self.feed_at(data, Instant::now())
    }

    fn feed_at(&mut self, data: &[u8], now: Instant) -> TrzszDetectResult {
        self.pending.extend_from_slice(data);
        if !data.is_empty() {
            self.pending_updated_at = Some(now);
        }

        let mut search_from = 0;
        while let Some((trigger, start, end)) = detect_trzsz_trigger(&self.pending, search_from) {
            if self.is_repeated_unique_id(&trigger) {
                search_from = end;
                continue;
            }

            let passthrough = self.pending[..start].to_vec();
            let remaining = self.pending[end..].to_vec();
            self.pending.clear();
            self.pending_updated_at = None;
            return TrzszDetectResult::Detected {
                trigger,
                passthrough,
                remaining,
            };
        }

        let keep_from = retained_prefix_start(&self.pending);
        let passthrough = self.pending[..keep_from].to_vec();
        if keep_from > 0 {
            self.pending.drain(..keep_from);
        }
        if self.pending.is_empty() {
            self.pending_updated_at = None;
        }

        TrzszDetectResult::NoMatch { passthrough }
    }

    /// Release an incomplete marker after input goes idle. Callers must poll
    /// this even when no more output arrives, and route these bytes through the
    /// same protocol/terminal path as ordinary passthrough output.
    pub fn flush_pending_if_idle(&mut self, now: Instant) -> Vec<u8> {
        let Some(updated_at) = self.pending_updated_at else {
            return Vec::new();
        };
        let timeout = if matches!(self.pending.as_slice(), b":" | b"::") {
            TRZSZ_COLON_IDLE_TIMEOUT
        } else {
            TRZSZ_TRIGGER_IDLE_TIMEOUT
        };
        if now.saturating_duration_since(updated_at) < timeout {
            return Vec::new();
        }
        self.pending_updated_at = None;
        std::mem::take(&mut self.pending)
    }

    pub fn filter_terminal_output(&mut self, data: &[u8]) -> TrzszFilteredOutput {
        let scan = self.scan_terminal_output(data);
        let mut output = TrzszFilteredOutput::default();
        for event in scan.events {
            match event {
                TrzszOutputEvent::Passthrough(bytes) => output.passthrough.extend(bytes),
                TrzszOutputEvent::Trigger(trigger) => output.triggers.push(trigger),
            }
        }
        output
    }

    pub fn scan_terminal_output(&mut self, data: &[u8]) -> TrzszOutputScan {
        let mut output = TrzszOutputScan::default();
        let mut feed = data.to_vec();

        loop {
            let result = self.feed(&feed);
            match result {
                TrzszDetectResult::NoMatch { passthrough } => {
                    if !passthrough.is_empty() {
                        output
                            .events
                            .push(TrzszOutputEvent::Passthrough(passthrough));
                    }
                    break;
                }
                TrzszDetectResult::Detected {
                    trigger,
                    passthrough,
                    remaining,
                } => {
                    if !passthrough.is_empty() {
                        output
                            .events
                            .push(TrzszOutputEvent::Passthrough(passthrough));
                    }
                    output.events.push(TrzszOutputEvent::Trigger(trigger));
                    if remaining.is_empty() {
                        break;
                    }
                    feed = remaining;
                }
            }
        }

        output
    }

    pub fn reset(&mut self) {
        self.pending.clear();
        self.pending_updated_at = None;
    }

    fn is_repeated_unique_id(&mut self, trigger: &TrzszTrigger) -> bool {
        let Some(unique_id) = trigger.unique_id.as_deref() else {
            return false;
        };
        if !should_track_unique_id(unique_id) {
            return false;
        }
        if self.seen_unique_ids.contains_key(unique_id) {
            return true;
        }

        if self.seen_unique_ids.len() > 100 {
            self.seen_unique_ids.retain(|_, order| {
                if *order >= 50 {
                    *order -= 50;
                    true
                } else {
                    false
                }
            });
        }
        self.seen_unique_ids
            .insert(unique_id.to_string(), self.seen_unique_ids.len());
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrzszProtocolFilteredOutput {
    pub passthrough: Vec<u8>,
    pub frames: Vec<TrzszProtocolFrame>,
    pub consumed_binary_bytes: usize,
}

#[derive(Debug, Default)]
pub struct TrzszProtocolStream {
    pending_line: Vec<u8>,
    binary_bytes_remaining: usize,
    binary_data: Vec<u8>,
}

impl TrzszProtocolStream {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn filter_terminal_output(&mut self, data: &[u8]) -> TrzszProtocolFilteredOutput {
        let mut output = TrzszProtocolFilteredOutput::default();

        for byte in data {
            if self.binary_bytes_remaining > 0 {
                self.binary_bytes_remaining -= 1;
                output.consumed_binary_bytes += 1;
                self.binary_data.push(*byte);
                if self.binary_bytes_remaining == 0 {
                    output.frames.push(TrzszProtocolFrame {
                        frame_type: "DATA".to_string(),
                        payload: TrzszProtocolPayload::EncodedBytes(std::mem::take(
                            &mut self.binary_data,
                        )),
                    });
                }
                continue;
            }

            if self.pending_line.is_empty() && *byte != b'#' {
                output.passthrough.push(*byte);
                continue;
            }

            self.pending_line.push(*byte);
            if self.pending_line.len() > TRZSZ_MAX_PROTOCOL_LINE_LEN {
                output.passthrough.append(&mut self.pending_line);
                continue;
            }
            if *byte != b'\n' {
                continue;
            }

            let line = std::mem::take(&mut self.pending_line);
            if let Some(frame) = parse_trzsz_protocol_frame(&line) {
                if let TrzszProtocolPayload::Integer(length) = &frame.payload
                    && frame.frame_type.eq_ignore_ascii_case("DATA")
                    && *length > 0
                {
                    self.binary_bytes_remaining =
                        usize::try_from(*length).unwrap_or(usize::MAX / 2);
                }
                output.frames.push(frame);
            } else {
                output.passthrough.extend(line);
            }
        }

        output
    }

    pub fn reset(&mut self) {
        self.pending_line.clear();
        self.binary_bytes_remaining = 0;
        self.binary_data.clear();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrzszDownloadEvent {
    FileCount {
        count: i64,
    },
    FileName {
        name: String,
    },
    FilePath {
        name: String,
        path_id: i64,
        components: Vec<String>,
    },
    Directory {
        name: String,
        path_id: i64,
        components: Vec<String>,
    },
    FileSize {
        name: String,
        size: i64,
    },
    Data {
        name: String,
        bytes: Vec<u8>,
        received: i64,
        size: i64,
    },
    FileFinished {
        name: String,
        digest: Vec<u8>,
    },
    Completed {
        names: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrzszDownloadStep {
    pub events: Vec<TrzszDownloadEvent>,
    pub responses: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrzszDownloadError {
    UnexpectedFrame {
        expected: &'static str,
        frame_type: String,
    },
    InvalidPayload {
        frame_type: String,
    },
    DataLengthMismatch {
        expected: i64,
        actual: i64,
    },
    DataOverflow {
        size: i64,
        received: i64,
        chunk: i64,
    },
    DigestMismatch {
        expected: Vec<u8>,
        actual: Vec<u8>,
    },
}

pub struct TrzszDownloadEngine {
    phase: TrzszDownloadPhase,
    remote_is_windows: bool,
    directory_mode: bool,
    protocol_version: i64,
}

enum TrzszDownloadPhase {
    AwaitNum,
    AwaitName {
        remaining: i64,
        names: Vec<String>,
    },
    AwaitSize {
        remaining: i64,
        names: Vec<String>,
        name: String,
    },
    AwaitData {
        remaining: i64,
        names: Vec<String>,
        name: String,
        size: i64,
        received: i64,
        md5: md5::Context,
        expected_binary_chunk: Option<i64>,
    },
    AwaitMd5 {
        remaining: i64,
        names: Vec<String>,
        name: String,
        digest: Vec<u8>,
    },
    Completed {
        names: Vec<String>,
    },
}

impl TrzszDownloadEngine {
    pub fn new(remote_is_windows: bool) -> Self {
        Self {
            phase: TrzszDownloadPhase::AwaitNum,
            remote_is_windows,
            directory_mode: false,
            protocol_version: 0,
        }
    }

    pub fn set_directory_mode(&mut self, directory_mode: bool) {
        self.directory_mode = directory_mode;
    }

    pub fn set_protocol_version(&mut self, protocol_version: i64) {
        self.protocol_version = protocol_version;
    }

    pub fn observe_frame(
        &mut self,
        frame: TrzszProtocolFrame,
    ) -> Result<TrzszDownloadStep, TrzszDownloadError> {
        match std::mem::replace(&mut self.phase, TrzszDownloadPhase::AwaitNum) {
            TrzszDownloadPhase::AwaitNum => self.observe_num(frame),
            TrzszDownloadPhase::AwaitName { remaining, names } => {
                self.observe_name(frame, remaining, names)
            }
            TrzszDownloadPhase::AwaitSize {
                remaining,
                names,
                name,
            } => self.observe_size(frame, remaining, names, name),
            TrzszDownloadPhase::AwaitData {
                remaining,
                names,
                name,
                size,
                received,
                md5,
                expected_binary_chunk,
            } => self.observe_data(
                frame,
                TrzszDownloadDataState {
                    remaining,
                    names,
                    name,
                    size,
                    received,
                    md5,
                    expected_binary_chunk,
                },
            ),
            TrzszDownloadPhase::AwaitMd5 {
                remaining,
                names,
                name,
                digest,
            } => self.observe_md5(frame, remaining, names, name, digest),
            TrzszDownloadPhase::Completed { names } => {
                self.phase = TrzszDownloadPhase::Completed { names };
                Err(TrzszDownloadError::UnexpectedFrame {
                    expected: "no more frames after completion",
                    frame_type: frame.frame_type,
                })
            }
        }
    }

    pub fn is_completed(&self) -> bool {
        matches!(self.phase, TrzszDownloadPhase::Completed { .. })
    }

    fn observe_num(
        &mut self,
        frame: TrzszProtocolFrame,
    ) -> Result<TrzszDownloadStep, TrzszDownloadError> {
        let frame_type = frame.frame_type.clone();
        if !frame_type.eq_ignore_ascii_case("NUM") {
            self.phase = TrzszDownloadPhase::AwaitNum;
            return Err(unexpected_trzsz_frame("NUM", frame));
        }
        let count = integer_payload(frame)?;
        let response = build_trzsz_integer_frame("SUCC", count, self.newline());
        let mut step = TrzszDownloadStep {
            events: vec![TrzszDownloadEvent::FileCount { count }],
            responses: vec![response],
        };
        if count <= 0 {
            self.phase = TrzszDownloadPhase::Completed { names: Vec::new() };
            step.events
                .push(TrzszDownloadEvent::Completed { names: Vec::new() });
        } else {
            self.phase = TrzszDownloadPhase::AwaitName {
                remaining: count,
                names: Vec::new(),
            };
        }
        Ok(step)
    }

    fn observe_name(
        &mut self,
        frame: TrzszProtocolFrame,
        remaining: i64,
        names: Vec<String>,
    ) -> Result<TrzszDownloadStep, TrzszDownloadError> {
        if !frame.frame_type.eq_ignore_ascii_case("NAME") {
            self.phase = TrzszDownloadPhase::AwaitName { remaining, names };
            return Err(unexpected_trzsz_frame("NAME", frame));
        }
        let raw_name = string_payload(&frame)?;
        let mut events = Vec::new();
        let (name, response, names) = if self.protocol_version >= TRZSZ_JSON_NAME_PROTOCOL {
            let entry = parse_trzsz_source_file(&raw_name).ok_or_else(|| {
                TrzszDownloadError::InvalidPayload {
                    frame_type: frame.frame_type.clone(),
                }
            })?;
            let name = entry.file_name().to_string();
            let response_name = entry.top_level_name().to_string();
            let names = push_unique_name(names, response_name.clone());
            // A fresh local download always reports size 0 so the sender skips
            // the prefix-hash resume exchange this engine does not implement.
            let target = build_trzsz_target_file(&response_name, 0);
            let response = build_trzsz_string_frame("SUCC", target.as_bytes(), self.newline());
            if entry.is_dir {
                events.push(TrzszDownloadEvent::Directory {
                    name: name.clone(),
                    path_id: entry.path_id,
                    components: entry.path_name.clone(),
                });
                let remaining = remaining.saturating_sub(1);
                if remaining <= 0 {
                    self.phase = TrzszDownloadPhase::Completed {
                        names: names.clone(),
                    };
                    events.push(TrzszDownloadEvent::Completed { names });
                } else {
                    self.phase = TrzszDownloadPhase::AwaitName { remaining, names };
                }
                return Ok(TrzszDownloadStep {
                    events,
                    responses: vec![response],
                });
            }
            if self.directory_mode {
                events.push(TrzszDownloadEvent::FilePath {
                    name: name.clone(),
                    path_id: entry.path_id,
                    components: entry.path_name,
                });
            }
            (name, response, names)
        } else {
            let response = build_trzsz_string_frame("SUCC", raw_name.as_bytes(), self.newline());
            (raw_name.clone(), response, names)
        };
        events.push(TrzszDownloadEvent::FileName { name: name.clone() });
        self.phase = TrzszDownloadPhase::AwaitSize {
            remaining,
            names,
            name: name.clone(),
        };
        Ok(TrzszDownloadStep {
            events,
            responses: vec![response],
        })
    }

    fn observe_size(
        &mut self,
        frame: TrzszProtocolFrame,
        remaining: i64,
        names: Vec<String>,
        name: String,
    ) -> Result<TrzszDownloadStep, TrzszDownloadError> {
        if !frame.frame_type.eq_ignore_ascii_case("SIZE") {
            self.phase = TrzszDownloadPhase::AwaitSize {
                remaining,
                names,
                name,
            };
            return Err(unexpected_trzsz_frame("SIZE", frame));
        }
        let size = integer_payload(frame)?;
        let response = build_trzsz_integer_frame("SUCC", size, self.newline());
        self.phase = if size == 0 {
            TrzszDownloadPhase::AwaitMd5 {
                remaining,
                names,
                name: name.clone(),
                digest: md5::Context::new().finalize().0.to_vec(),
            }
        } else {
            TrzszDownloadPhase::AwaitData {
                remaining,
                names,
                name: name.clone(),
                size,
                received: 0,
                md5: md5::Context::new(),
                expected_binary_chunk: None,
            }
        };
        Ok(TrzszDownloadStep {
            events: vec![TrzszDownloadEvent::FileSize { name, size }],
            responses: vec![response],
        })
    }

    fn observe_data(
        &mut self,
        frame: TrzszProtocolFrame,
        mut state: TrzszDownloadDataState,
    ) -> Result<TrzszDownloadStep, TrzszDownloadError> {
        if !frame.frame_type.eq_ignore_ascii_case("DATA") {
            self.phase = state.into_phase();
            return Err(unexpected_trzsz_frame("DATA", frame));
        }
        if let TrzszProtocolPayload::Integer(length) = frame.payload {
            state.expected_binary_chunk = Some(length);
            self.phase = state.into_phase();
            return Ok(TrzszDownloadStep::default());
        }

        let bytes = bytes_payload(&frame)?;
        if let Some(expected) = state.expected_binary_chunk.take() {
            let actual = bytes.len() as i64;
            if expected != actual {
                self.phase = state.into_phase();
                return Err(TrzszDownloadError::DataLengthMismatch { expected, actual });
            }
        }
        let chunk = bytes.len() as i64;
        if state.received.saturating_add(chunk) > state.size {
            let error = TrzszDownloadError::DataOverflow {
                size: state.size,
                received: state.received,
                chunk,
            };
            self.phase = state.into_phase();
            return Err(error);
        }

        state.md5.consume(&bytes);
        state.received += chunk;
        let response = build_trzsz_integer_frame("SUCC", chunk, self.newline());
        let event = TrzszDownloadEvent::Data {
            name: state.name.clone(),
            bytes,
            received: state.received,
            size: state.size,
        };
        if state.received == state.size {
            let digest = state.md5.finalize().0.to_vec();
            self.phase = TrzszDownloadPhase::AwaitMd5 {
                remaining: state.remaining,
                names: state.names,
                name: state.name,
                digest,
            };
        } else {
            self.phase = state.into_phase();
        }
        Ok(TrzszDownloadStep {
            events: vec![event],
            responses: vec![response],
        })
    }

    fn observe_md5(
        &mut self,
        frame: TrzszProtocolFrame,
        remaining: i64,
        mut names: Vec<String>,
        name: String,
        digest: Vec<u8>,
    ) -> Result<TrzszDownloadStep, TrzszDownloadError> {
        if !frame.frame_type.eq_ignore_ascii_case("MD5") {
            self.phase = TrzszDownloadPhase::AwaitMd5 {
                remaining,
                names,
                name,
                digest,
            };
            return Err(unexpected_trzsz_frame("MD5", frame));
        }
        let expected = bytes_payload(&frame)?;
        if digest != expected {
            self.phase = TrzszDownloadPhase::AwaitMd5 {
                remaining,
                names,
                name,
                digest: digest.clone(),
            };
            return Err(TrzszDownloadError::DigestMismatch {
                expected,
                actual: digest,
            });
        }
        if !self.directory_mode {
            names = push_unique_name(names, name.clone());
        }
        let response = build_trzsz_string_frame("SUCC", &digest, self.newline());
        let mut events = vec![TrzszDownloadEvent::FileFinished {
            name,
            digest: digest.clone(),
        }];
        let remaining = remaining.saturating_sub(1);
        if remaining <= 0 {
            self.phase = TrzszDownloadPhase::Completed {
                names: names.clone(),
            };
            events.push(TrzszDownloadEvent::Completed { names });
        } else {
            self.phase = TrzszDownloadPhase::AwaitName { remaining, names };
        }
        Ok(TrzszDownloadStep {
            events,
            responses: vec![response],
        })
    }

    fn newline(&self) -> &str {
        if self.remote_is_windows { "!\n" } else { "\n" }
    }
}

struct TrzszDownloadDataState {
    remaining: i64,
    names: Vec<String>,
    name: String,
    size: i64,
    received: i64,
    md5: md5::Context,
    expected_binary_chunk: Option<i64>,
}

impl TrzszDownloadDataState {
    fn into_phase(self) -> TrzszDownloadPhase {
        TrzszDownloadPhase::AwaitData {
            remaining: self.remaining,
            names: self.names,
            name: self.name,
            size: self.size,
            received: self.received,
            md5: self.md5,
            expected_binary_chunk: self.expected_binary_chunk,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrzszUploadEntry {
    pub name: String,
    pub size: i64,
    pub payload: TrzszUploadPayload,
    pub source: Option<TrzszUploadSource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrzszUploadPayload {
    Memory(Vec<u8>),
    File(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrzszUploadSource {
    pub path_id: i64,
    pub path_name: Vec<String>,
    pub is_dir: bool,
    #[serde(default)]
    pub size: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perm: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrzszUploadEvent {
    Started {
        count: i64,
    },
    FileStarted {
        name: String,
        remote_name: String,
        size: i64,
    },
    Data {
        name: String,
        sent: i64,
        size: i64,
    },
    FileFinished {
        name: String,
        digest: Vec<u8>,
    },
    Directory {
        name: String,
        remote_name: String,
    },
    Completed {
        names: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrzszUploadStep {
    pub events: Vec<TrzszUploadEvent>,
    pub responses: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrzszUploadError {
    NoFiles,
    AlreadyStarted,
    UnexpectedFrame {
        expected: &'static str,
        frame_type: String,
    },
    InvalidPayload {
        frame_type: String,
    },
    InvalidSource {
        name: String,
    },
    PayloadRead {
        name: String,
        message: String,
    },
    AckMismatch {
        expected: TrzszProtocolPayload,
        actual: TrzszProtocolPayload,
    },
}

pub struct TrzszUploadEngine {
    phase: TrzszUploadPhase,
    remote_is_windows: bool,
    entries: Vec<TrzszUploadEntry>,
    protocol_version: i64,
}

enum TrzszUploadPhase {
    Ready,
    AwaitNumAck,
    AwaitNameAck {
        index: usize,
        remote_names: Vec<String>,
    },
    AwaitSizeAck {
        index: usize,
        remote_names: Vec<String>,
        remote_name: String,
    },
    AwaitDataAck {
        index: usize,
        remote_names: Vec<String>,
        remote_name: String,
        sent: i64,
        chunk: i64,
        md5: md5::Context,
        reader: TrzszUploadReader,
    },
    AwaitMd5Ack {
        index: usize,
        remote_names: Vec<String>,
        digest: Vec<u8>,
    },
    Completed {
        names: Vec<String>,
    },
}

enum TrzszUploadReader {
    Memory,
    File(File),
}

impl TrzszUploadEngine {
    pub fn new(remote_is_windows: bool, entries: Vec<TrzszUploadEntry>) -> Self {
        Self {
            phase: TrzszUploadPhase::Ready,
            remote_is_windows,
            entries,
            protocol_version: 0,
        }
    }

    pub fn set_protocol_version(&mut self, protocol_version: i64) {
        self.protocol_version = protocol_version;
    }

    pub fn begin(&mut self) -> Result<TrzszUploadStep, TrzszUploadError> {
        if !matches!(self.phase, TrzszUploadPhase::Ready) {
            return Err(TrzszUploadError::AlreadyStarted);
        }
        if self.entries.is_empty() {
            return Err(TrzszUploadError::NoFiles);
        }
        let count = self.entries.len() as i64;
        self.phase = TrzszUploadPhase::AwaitNumAck;
        Ok(TrzszUploadStep {
            events: vec![TrzszUploadEvent::Started { count }],
            responses: vec![build_trzsz_integer_frame("NUM", count, self.newline())],
        })
    }

    pub fn observe_frame(
        &mut self,
        frame: TrzszProtocolFrame,
    ) -> Result<TrzszUploadStep, TrzszUploadError> {
        match std::mem::replace(&mut self.phase, TrzszUploadPhase::Ready) {
            TrzszUploadPhase::Ready => {
                self.phase = TrzszUploadPhase::Ready;
                Err(TrzszUploadError::UnexpectedFrame {
                    expected: "begin before observing remote acknowledgements",
                    frame_type: frame.frame_type,
                })
            }
            TrzszUploadPhase::AwaitNumAck => self.observe_num_ack(frame),
            TrzszUploadPhase::AwaitNameAck {
                index,
                remote_names,
            } => self.observe_name_ack(frame, index, remote_names),
            TrzszUploadPhase::AwaitSizeAck {
                index,
                remote_names,
                remote_name,
            } => self.observe_size_ack(frame, index, remote_names, remote_name),
            TrzszUploadPhase::AwaitDataAck {
                index,
                remote_names,
                remote_name,
                sent,
                chunk,
                md5,
                reader,
            } => self.observe_data_ack(
                frame,
                index,
                remote_names,
                remote_name,
                sent,
                chunk,
                md5,
                reader,
            ),
            TrzszUploadPhase::AwaitMd5Ack {
                index,
                remote_names,
                digest,
            } => self.observe_md5_ack(frame, index, remote_names, digest),
            TrzszUploadPhase::Completed { names } => {
                self.phase = TrzszUploadPhase::Completed { names };
                Err(TrzszUploadError::UnexpectedFrame {
                    expected: "no more frames after completion",
                    frame_type: frame.frame_type,
                })
            }
        }
    }

    pub fn is_completed(&self) -> bool {
        matches!(self.phase, TrzszUploadPhase::Completed { .. })
    }

    fn observe_num_ack(
        &mut self,
        frame: TrzszProtocolFrame,
    ) -> Result<TrzszUploadStep, TrzszUploadError> {
        let expected = TrzszProtocolPayload::Integer(self.entries.len() as i64);
        expect_upload_ack(&frame, &expected)?;
        self.send_name(0, Vec::new())
    }

    fn observe_name_ack(
        &mut self,
        frame: TrzszProtocolFrame,
        index: usize,
        mut remote_names: Vec<String>,
    ) -> Result<TrzszUploadStep, TrzszUploadError> {
        if !frame.frame_type.eq_ignore_ascii_case("SUCC") {
            self.phase = TrzszUploadPhase::AwaitNameAck {
                index,
                remote_names,
            };
            return Err(unexpected_trzsz_upload_frame("SUCC", frame));
        }
        let remote_name = if self.protocol_version >= TRZSZ_JSON_NAME_PROTOCOL {
            let target = upload_string_payload(&frame)?;
            parse_trzsz_target_file_name(&target).ok_or_else(|| {
                TrzszUploadError::InvalidPayload {
                    frame_type: frame.frame_type.clone(),
                }
            })?
        } else {
            upload_string_payload(&frame)?
        };
        if !remote_names.contains(&remote_name) {
            remote_names.push(remote_name.clone());
        }
        let entry = &self.entries[index];
        if entry.is_dir() {
            let mut events = vec![TrzszUploadEvent::Directory {
                name: entry.display_name().to_string(),
                remote_name,
            }];
            let next_index = index + 1;
            if next_index >= self.entries.len() {
                self.phase = TrzszUploadPhase::Completed {
                    names: remote_names.clone(),
                };
                events.push(TrzszUploadEvent::Completed {
                    names: remote_names,
                });
                return Ok(TrzszUploadStep {
                    events,
                    responses: Vec::new(),
                });
            }

            let mut step = self.send_name(next_index, remote_names)?;
            step.events.splice(0..0, events);
            return Ok(step);
        }

        let size = entry.size;
        self.phase = TrzszUploadPhase::AwaitSizeAck {
            index,
            remote_names,
            remote_name: remote_name.clone(),
        };
        Ok(TrzszUploadStep {
            events: vec![TrzszUploadEvent::FileStarted {
                name: entry.display_name().to_string(),
                remote_name,
                size,
            }],
            responses: vec![build_trzsz_integer_frame("SIZE", size, self.newline())],
        })
    }

    fn observe_size_ack(
        &mut self,
        frame: TrzszProtocolFrame,
        index: usize,
        remote_names: Vec<String>,
        remote_name: String,
    ) -> Result<TrzszUploadStep, TrzszUploadError> {
        let entry = &self.entries[index];
        let size = entry.size;
        let expected = TrzszProtocolPayload::Integer(size);
        expect_upload_ack(&frame, &expected)?;

        if size <= 0 {
            let digest = md5::compute([]).0.to_vec();
            self.phase = TrzszUploadPhase::AwaitMd5Ack {
                index,
                remote_names,
                digest: digest.clone(),
            };
            return Ok(TrzszUploadStep {
                events: Vec::new(),
                responses: vec![build_trzsz_string_frame("MD5", &digest, self.newline())],
            });
        }

        let reader = self.open_upload_reader(index)?;
        self.send_upload_data_chunk(
            index,
            remote_names,
            remote_name,
            0,
            md5::Context::new(),
            reader,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn observe_data_ack(
        &mut self,
        frame: TrzszProtocolFrame,
        index: usize,
        remote_names: Vec<String>,
        remote_name: String,
        sent: i64,
        chunk: i64,
        md5: md5::Context,
        reader: TrzszUploadReader,
    ) -> Result<TrzszUploadStep, TrzszUploadError> {
        let expected = TrzszProtocolPayload::Integer(chunk);
        expect_upload_ack(&frame, &expected)?;
        if sent < self.entries[index].size {
            return self.send_upload_data_chunk(
                index,
                remote_names,
                remote_name,
                sent,
                md5,
                reader,
            );
        }
        let digest = md5.finalize().0.to_vec();
        self.phase = TrzszUploadPhase::AwaitMd5Ack {
            index,
            remote_names,
            digest: digest.clone(),
        };
        Ok(TrzszUploadStep {
            events: Vec::new(),
            responses: vec![build_trzsz_string_frame("MD5", &digest, self.newline())],
        })
    }

    fn observe_md5_ack(
        &mut self,
        frame: TrzszProtocolFrame,
        index: usize,
        remote_names: Vec<String>,
        digest: Vec<u8>,
    ) -> Result<TrzszUploadStep, TrzszUploadError> {
        let expected = TrzszProtocolPayload::EncodedBytes(digest.clone());
        expect_upload_ack(&frame, &expected)?;
        let entry = &self.entries[index];
        let mut events = vec![TrzszUploadEvent::FileFinished {
            name: entry.display_name().to_string(),
            digest,
        }];
        let next_index = index + 1;
        if next_index >= self.entries.len() {
            self.phase = TrzszUploadPhase::Completed {
                names: remote_names.clone(),
            };
            events.push(TrzszUploadEvent::Completed {
                names: remote_names,
            });
            return Ok(TrzszUploadStep {
                events,
                responses: Vec::new(),
            });
        }

        let mut step = self.send_name(next_index, remote_names)?;
        step.events.splice(0..0, events);
        Ok(step)
    }

    fn open_upload_reader(&self, index: usize) -> Result<TrzszUploadReader, TrzszUploadError> {
        let entry = self.entries.get(index).ok_or(TrzszUploadError::NoFiles)?;
        match &entry.payload {
            TrzszUploadPayload::Memory(_) => Ok(TrzszUploadReader::Memory),
            TrzszUploadPayload::File(path) => File::open(path)
                .map(TrzszUploadReader::File)
                .map_err(|error| TrzszUploadError::PayloadRead {
                    name: entry.display_name().to_string(),
                    message: error.to_string(),
                }),
        }
    }

    fn send_upload_data_chunk(
        &mut self,
        index: usize,
        remote_names: Vec<String>,
        remote_name: String,
        sent: i64,
        mut md5: md5::Context,
        mut reader: TrzszUploadReader,
    ) -> Result<TrzszUploadStep, TrzszUploadError> {
        let entry = self.entries.get(index).ok_or(TrzszUploadError::NoFiles)?;
        let size = entry.size;
        let chunk = read_upload_chunk(entry, sent, &mut reader)?;
        let chunk_len = chunk.len() as i64;
        if chunk_len <= 0 || sent + chunk_len > size {
            return Err(TrzszUploadError::PayloadRead {
                name: entry.display_name().to_string(),
                message: format!(
                    "unexpected upload payload length: sent {sent}, chunk {chunk_len}, size {size}"
                ),
            });
        }
        md5.consume(&chunk);
        let sent = sent + chunk_len;
        let display_name = entry.display_name().to_string();
        self.phase = TrzszUploadPhase::AwaitDataAck {
            index,
            remote_names,
            remote_name,
            sent,
            chunk: chunk_len,
            md5,
            reader,
        };
        Ok(TrzszUploadStep {
            events: vec![TrzszUploadEvent::Data {
                name: display_name,
                sent,
                size,
            }],
            responses: vec![build_trzsz_string_frame("DATA", &chunk, self.newline())],
        })
    }

    fn send_name(
        &mut self,
        index: usize,
        remote_names: Vec<String>,
    ) -> Result<TrzszUploadStep, TrzszUploadError> {
        let entry = self.entries.get(index).ok_or(TrzszUploadError::NoFiles)?;
        let name = entry.protocol_name(self.protocol_version)?;
        self.phase = TrzszUploadPhase::AwaitNameAck {
            index,
            remote_names,
        };
        Ok(TrzszUploadStep {
            events: Vec::new(),
            responses: vec![build_trzsz_string_frame(
                "NAME",
                name.as_bytes(),
                self.newline(),
            )],
        })
    }

    fn newline(&self) -> &str {
        if self.remote_is_windows { "!\n" } else { "\n" }
    }
}

impl TrzszUploadEntry {
    pub fn from_bytes(name: impl Into<String>, data: Vec<u8>) -> Self {
        Self {
            name: name.into(),
            size: data.len() as i64,
            payload: TrzszUploadPayload::Memory(data),
            source: None,
        }
    }

    pub fn from_file(name: impl Into<String>, path: PathBuf, size: i64) -> Self {
        Self {
            name: name.into(),
            size,
            payload: TrzszUploadPayload::File(path),
            source: None,
        }
    }

    fn display_name(&self) -> &str {
        self.source
            .as_ref()
            .and_then(|source| source.path_name.last())
            .map(String::as_str)
            .unwrap_or(&self.name)
    }

    fn is_dir(&self) -> bool {
        self.source.as_ref().is_some_and(|source| source.is_dir)
    }

    fn protocol_name(&self, protocol_version: i64) -> Result<String, TrzszUploadError> {
        if protocol_version >= TRZSZ_JSON_NAME_PROTOCOL {
            let source = self.source.clone().unwrap_or(TrzszUploadSource {
                path_id: 0,
                path_name: vec![self.name.clone()],
                is_dir: false,
                size: self.size.max(0),
                perm: None,
            });
            return serde_json::to_string(&source).map_err(|_| TrzszUploadError::InvalidSource {
                name: self.display_name().to_string(),
            });
        }
        Ok(self.name.clone())
    }
}

fn read_upload_chunk(
    entry: &TrzszUploadEntry,
    sent: i64,
    reader: &mut TrzszUploadReader,
) -> Result<Vec<u8>, TrzszUploadError> {
    let remaining = entry.size.saturating_sub(sent) as usize;
    let chunk_size = remaining.min(TRZSZ_UPLOAD_DATA_CHUNK_SIZE);
    match (&entry.payload, reader) {
        (TrzszUploadPayload::Memory(data), TrzszUploadReader::Memory) => {
            let start = sent.max(0) as usize;
            let end = start.saturating_add(chunk_size).min(data.len());
            Ok(data.get(start..end).unwrap_or_default().to_vec())
        }
        (TrzszUploadPayload::File(_), TrzszUploadReader::File(file)) => {
            let mut chunk = vec![0; chunk_size];
            let read = file
                .read(&mut chunk)
                .map_err(|error| TrzszUploadError::PayloadRead {
                    name: entry.display_name().to_string(),
                    message: error.to_string(),
                })?;
            chunk.truncate(read);
            Ok(chunk)
        }
        _ => Err(TrzszUploadError::PayloadRead {
            name: entry.display_name().to_string(),
            message: "upload payload reader mismatch".to_string(),
        }),
    }
}

#[derive(Debug, Clone, Deserialize)]
struct TrzszSourceFile {
    #[serde(default)]
    path_id: i64,
    #[serde(default)]
    path_name: Vec<String>,
    #[serde(default)]
    is_dir: bool,
}

impl TrzszSourceFile {
    fn top_level_name(&self) -> &str {
        self.path_name.first().map(String::as_str).unwrap_or("")
    }

    fn file_name(&self) -> &str {
        self.path_name.last().map(String::as_str).unwrap_or("")
    }
}

fn parse_trzsz_source_file(source: &str) -> Option<TrzszSourceFile> {
    let source: TrzszSourceFile = serde_json::from_str(source).ok()?;
    (!source.path_name.is_empty()
        && !source.top_level_name().is_empty()
        && !source.file_name().is_empty())
    .then_some(source)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrzszTargetFile {
    name: String,
    size: i64,
}

fn build_trzsz_target_file(name: &str, size: i64) -> String {
    serde_json::to_string(&TrzszTargetFile {
        name: name.to_string(),
        size,
    })
    .expect("target file payload is always serializable")
}

fn parse_trzsz_target_file_name(target: &str) -> Option<String> {
    let target: TrzszTargetFile = serde_json::from_str(target).ok()?;
    (target.size >= 0 && !target.name.is_empty()).then_some(target.name)
}

fn push_unique_name(mut names: Vec<String>, name: String) -> Vec<String> {
    if !names.contains(&name) {
        names.push(name);
    }
    names
}

fn unexpected_trzsz_upload_frame(
    expected: &'static str,
    frame: TrzszProtocolFrame,
) -> TrzszUploadError {
    TrzszUploadError::UnexpectedFrame {
        expected,
        frame_type: frame.frame_type,
    }
}

fn expect_upload_ack(
    frame: &TrzszProtocolFrame,
    expected: &TrzszProtocolPayload,
) -> Result<(), TrzszUploadError> {
    if !frame.frame_type.eq_ignore_ascii_case("SUCC") {
        return Err(TrzszUploadError::UnexpectedFrame {
            expected: "SUCC",
            frame_type: frame.frame_type.clone(),
        });
    }
    if &frame.payload == expected {
        return Ok(());
    }
    Err(TrzszUploadError::AckMismatch {
        expected: expected.clone(),
        actual: frame.payload.clone(),
    })
}

fn upload_string_payload(frame: &TrzszProtocolFrame) -> Result<String, TrzszUploadError> {
    match &frame.payload {
        TrzszProtocolPayload::EncodedBytes(bytes) => {
            String::from_utf8(bytes.clone()).map_err(|_| TrzszUploadError::InvalidPayload {
                frame_type: frame.frame_type.clone(),
            })
        }
        _ => Err(TrzszUploadError::InvalidPayload {
            frame_type: frame.frame_type.clone(),
        }),
    }
}

fn unexpected_trzsz_frame(expected: &'static str, frame: TrzszProtocolFrame) -> TrzszDownloadError {
    TrzszDownloadError::UnexpectedFrame {
        expected,
        frame_type: frame.frame_type,
    }
}

fn integer_payload(frame: TrzszProtocolFrame) -> Result<i64, TrzszDownloadError> {
    match frame.payload {
        TrzszProtocolPayload::Integer(value) => Ok(value),
        _ => Err(TrzszDownloadError::InvalidPayload {
            frame_type: frame.frame_type,
        }),
    }
}

fn bytes_payload(frame: &TrzszProtocolFrame) -> Result<Vec<u8>, TrzszDownloadError> {
    match &frame.payload {
        TrzszProtocolPayload::EncodedBytes(bytes) => Ok(bytes.clone()),
        _ => Err(TrzszDownloadError::InvalidPayload {
            frame_type: frame.frame_type.clone(),
        }),
    }
}

fn string_payload(frame: &TrzszProtocolFrame) -> Result<String, TrzszDownloadError> {
    let bytes = bytes_payload(frame)?;
    String::from_utf8(bytes).map_err(|_| TrzszDownloadError::InvalidPayload {
        frame_type: frame.frame_type.clone(),
    })
}

fn detect_trzsz_trigger(data: &[u8], search_from: usize) -> Option<(TrzszTrigger, usize, usize)> {
    for start in search_from..data.len() {
        if !data[start..].starts_with(TRZSZ_PREFIX) {
            continue;
        }
        if let ParseTrigger::Detected(trigger, end) = parse_trzsz_trigger(&data[start..]) {
            return Some((trigger, start, start + end));
        }
    }
    None
}

enum ParseTrigger {
    Detected(TrzszTrigger, usize),
    Incomplete,
    NoMatch,
}

fn parse_trzsz_trigger(data: &[u8]) -> ParseTrigger {
    if data.len() < TRZSZ_PREFIX.len() {
        return if TRZSZ_PREFIX.starts_with(data) {
            ParseTrigger::Incomplete
        } else {
            ParseTrigger::NoMatch
        };
    }
    if !data.starts_with(TRZSZ_PREFIX) {
        return ParseTrigger::NoMatch;
    }
    if is_stale_trzsz_trigger_text(data) {
        return ParseTrigger::NoMatch;
    }

    let mut pos = TRZSZ_PREFIX.len();
    let Some(mode_byte) = data.get(pos).copied() else {
        return ParseTrigger::Incomplete;
    };
    let mode = match mode_byte {
        b'S' => TrzszMode::Send,
        b'R' => TrzszMode::Receive,
        b'D' => TrzszMode::Directory,
        _ => return ParseTrigger::NoMatch,
    };
    pos += 1;

    if !consume_byte(data, &mut pos, b':') {
        return if pos >= data.len() {
            ParseTrigger::Incomplete
        } else {
            ParseTrigger::NoMatch
        };
    }

    let version_start = pos;
    for part in 0..3 {
        if !consume_digits(data, &mut pos) {
            return if pos >= data.len() {
                ParseTrigger::Incomplete
            } else {
                ParseTrigger::NoMatch
            };
        }
        if part < 2 && !consume_byte(data, &mut pos, b'.') {
            return if pos >= data.len() {
                ParseTrigger::Incomplete
            } else {
                ParseTrigger::NoMatch
            };
        }
    }
    let version = String::from_utf8_lossy(&data[version_start..pos]).to_string();

    let unique_id = match parse_optional_number_field(data, &mut pos) {
        OptionalField::Some(value) => Some(value),
        OptionalField::None => None,
        OptionalField::Incomplete => return ParseTrigger::Incomplete,
    };
    let remote_is_windows = unique_id
        .as_deref()
        .is_some_and(is_windows_server_unique_id);
    let tunnel_port = match parse_optional_number_field(data, &mut pos) {
        OptionalField::Some(value) => value.parse::<u16>().ok(),
        OptionalField::None => None,
        OptionalField::Incomplete => return ParseTrigger::Incomplete,
    };

    let raw = data[..pos].to_vec();
    ParseTrigger::Detected(
        TrzszTrigger {
            mode,
            version,
            unique_id,
            remote_is_windows,
            tunnel_port,
            raw,
        },
        pos,
    )
}

fn consume_byte(data: &[u8], pos: &mut usize, expected: u8) -> bool {
    if data.get(*pos) == Some(&expected) {
        *pos += 1;
        true
    } else {
        false
    }
}

fn consume_digits(data: &[u8], pos: &mut usize) -> bool {
    let start = *pos;
    while data.get(*pos).is_some_and(u8::is_ascii_digit) {
        *pos += 1;
    }
    *pos > start
}

enum OptionalField {
    Some(String),
    None,
    Incomplete,
}

fn parse_optional_number_field(data: &[u8], pos: &mut usize) -> OptionalField {
    if data.get(*pos) != Some(&b':') {
        return OptionalField::None;
    }
    let value_start = (*pos).saturating_add(1);
    let Some(next) = data.get(value_start).copied() else {
        return OptionalField::Incomplete;
    };
    if !next.is_ascii_digit() {
        return OptionalField::None;
    }
    *pos = value_start;
    let start = *pos;
    while data.get(*pos).is_some_and(u8::is_ascii_digit) {
        *pos += 1;
    }
    OptionalField::Some(String::from_utf8_lossy(&data[start..*pos]).to_string())
}

fn is_windows_server_unique_id(unique_id: &str) -> bool {
    unique_id == "1" || (unique_id.len() == 13 && unique_id.ends_with("10"))
}

fn should_track_unique_id(unique_id: &str) -> bool {
    unique_id.len() > 6 && (cfg!(windows) || unique_id.len() != 13 || !unique_id.ends_with("00"))
}

fn is_stale_trzsz_trigger_text(data: &[u8]) -> bool {
    data.len() > 40
        && TRZSZ_STALE_TRIGGER_MARKERS.iter().any(|marker| {
            data[40..]
                .windows(marker.len())
                .any(|window| window == *marker)
        })
}

pub fn trzsz_fail_response(message: &str, remote_is_windows: bool) -> Vec<u8> {
    let newline = if remote_is_windows { "!\n" } else { "\n" };
    build_trzsz_string_frame("fail", message.as_bytes(), newline)
}

pub fn build_trzsz_integer_frame(frame_type: &str, value: i64, newline: &str) -> Vec<u8> {
    format!("#{frame_type}:{value}{newline}").into_bytes()
}

pub fn build_trzsz_string_frame(frame_type: &str, data: &[u8], newline: &str) -> Vec<u8> {
    let encoded = encode_trzsz_string(data);
    format!("#{frame_type}:{encoded}{newline}").into_bytes()
}

pub fn build_trzsz_action_frame(action: &TrzszAction, remote_is_windows: bool) -> Vec<u8> {
    let json = serde_json::to_vec(action).expect("serializing trzsz action should not fail");
    let newline = if remote_is_windows { "!\n" } else { "\n" };
    build_trzsz_string_frame("ACT", &json, newline)
}

pub fn build_trzsz_config_frame(config: &TrzszConfig, remote_is_windows: bool) -> Vec<u8> {
    let json = serde_json::to_vec(config).expect("serializing trzsz config should not fail");
    let newline = if remote_is_windows { "!\n" } else { "\n" };
    build_trzsz_string_frame("CFG", &json, newline)
}

pub fn parse_trzsz_protocol_frame(line: &[u8]) -> Option<TrzszProtocolFrame> {
    let line = trim_trzsz_protocol_line_ending(line);
    if !line.starts_with(b"#") {
        return None;
    }
    let colon = line.iter().position(|byte| *byte == b':')?;
    if colon <= 1 {
        return None;
    }
    let frame_type = std::str::from_utf8(&line[1..colon]).ok()?.to_string();
    let payload_bytes = &line[colon + 1..];
    let payload_text = std::str::from_utf8(payload_bytes).ok()?.to_string();
    let payload = if should_parse_integer_frame(&frame_type)
        && payload_bytes.iter().all(u8::is_ascii_digit)
        && let Ok(value) = payload_text.parse::<i64>()
    {
        TrzszProtocolPayload::Integer(value)
    } else if let Some(decoded) = decode_trzsz_string(payload_bytes) {
        TrzszProtocolPayload::EncodedBytes(decoded)
    } else {
        TrzszProtocolPayload::Raw(payload_text)
    };

    Some(TrzszProtocolFrame {
        frame_type,
        payload,
    })
}

pub fn parse_trzsz_json_frame(frame: &TrzszProtocolFrame) -> Option<serde_json::Value> {
    let TrzszProtocolPayload::EncodedBytes(bytes) = &frame.payload else {
        return None;
    };
    serde_json::from_slice(bytes).ok()
}

pub fn parse_trzsz_action_frame(frame: &TrzszProtocolFrame) -> Option<TrzszAction> {
    let TrzszProtocolPayload::EncodedBytes(bytes) = &frame.payload else {
        return None;
    };
    serde_json::from_slice(bytes).ok()
}

pub fn parse_trzsz_config_frame(frame: &TrzszProtocolFrame) -> Option<TrzszConfig> {
    let TrzszProtocolPayload::EncodedBytes(bytes) = &frame.payload else {
        return None;
    };
    serde_json::from_slice(bytes).ok()
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn encode_trzsz_string(data: &[u8]) -> String {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(data)
        .expect("zlib encoder write to Vec should not fail");
    let compressed = encoder
        .finish()
        .expect("zlib encoder finish should not fail");
    base64::engine::general_purpose::STANDARD.encode(compressed)
}

fn decode_trzsz_string(encoded: &[u8]) -> Option<Vec<u8>> {
    let compressed = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let decoder = ZlibDecoder::new(compressed.as_slice());
    let mut decoded = Vec::new();
    decoder
        .take((TRZSZ_MAX_DECODED_PAYLOAD_BYTES + 1) as u64)
        .read_to_end(&mut decoded)
        .ok()?;
    if decoded.len() > TRZSZ_MAX_DECODED_PAYLOAD_BYTES {
        return None;
    }
    Some(decoded)
}

fn trim_trzsz_protocol_line_ending(mut line: &[u8]) -> &[u8] {
    if line.ends_with(b"\n") {
        line = &line[..line.len() - 1];
    }
    if line.ends_with(b"\r") || line.ends_with(b"!") {
        line = &line[..line.len() - 1];
    }
    line
}

fn should_parse_integer_frame(frame_type: &str) -> bool {
    matches!(
        frame_type.to_ascii_uppercase().as_str(),
        "NUM" | "SIZE" | "DATA" | "SUCC"
    )
}

fn trzsz_payload_message(payload: &TrzszProtocolPayload) -> String {
    match payload {
        TrzszProtocolPayload::EncodedBytes(bytes) => String::from_utf8_lossy(bytes).to_string(),
        TrzszProtocolPayload::Raw(text) => text.clone(),
        TrzszProtocolPayload::Integer(value) => value.to_string(),
    }
}

fn retained_prefix_start(data: &[u8]) -> usize {
    let max_suffix = data.len().min(TRZSZ_MAX_TRIGGER_LEN);
    for len in (1..=max_suffix).rev() {
        let start = data.len() - len;
        let suffix = &data[start..];
        if TRZSZ_PREFIX.starts_with(suffix) {
            return start;
        }
        if suffix.starts_with(TRZSZ_PREFIX)
            && matches!(parse_trzsz_trigger(suffix), ParseTrigger::Incomplete)
        {
            return start;
        }
    }
    data.len()
}

#[cfg(test)]
mod tests;
