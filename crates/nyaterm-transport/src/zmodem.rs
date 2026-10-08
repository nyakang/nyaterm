//! ZMODEM (lrzsz) file transfer detection and protocol handling.
//!
//! Ported from the Tauri `core/zmodem` module for the native session stack.

macro_rules! zmodem_log {
    ($($t:tt)*) => {{}};
}

use serde::Serialize;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

// ZMODEM protocol constants for header detection.
const ZPAD: u8 = 0x2A; // '*'
const ZDLE: u8 = 0x18; // CAN / Ctrl-X
const ZHEX: u8 = 0x42; // 'B'
const ZBIN: u8 = 0x41; // 'A'
const ZBIN32: u8 = 0x43; // 'C'

/// Minimum header bytes: ZPAD ZPAD ZDLE (ZHEX|ZBIN|ZBIN32)
const ZMODEM_HEADER_LEN: usize = 4;

// Bare asterisks can be shell echoes; prefixes containing ZDLE get more time
// for fragmented delivery, but neither may hide terminal text indefinitely.
const ZMODEM_ASTERISK_IDLE_TIMEOUT: Duration = Duration::from_millis(50);
const ZMODEM_HEADER_IDLE_TIMEOUT: Duration = Duration::from_secs(1);

/// Five consecutive CAN (0x18) bytes abort a ZMODEM session.
const CANCEL_SEQ_LEN: usize = 5;

const ZMODEM_PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const ZMODEM_PROGRESS_BYTES: u64 = 256 * 1024;

/// Direction of the ZMODEM transfer from the **local** perspective.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ZmodemDirection {
    /// Remote `sz` → we **download** (receive) files.
    Download,
    /// Remote `rz` → we **upload** (send) files.
    Upload,
}

/// Events emitted to the frontend via Tauri events.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ZmodemEvent {
    /// A ZMODEM session was detected — frontend should show a file dialog.
    Detected { direction: ZmodemDirection },
    /// Progress update for an active transfer.
    Progress {
        #[serde(rename = "fileName")]
        file_name: String,
        #[serde(rename = "bytesTransferred")]
        bytes_transferred: u64,
        #[serde(rename = "totalSize")]
        total_size: u64,
        direction: ZmodemDirection,
    },
    /// The ZMODEM session completed successfully.
    Complete {
        direction: ZmodemDirection,
        #[serde(rename = "fileCount")]
        file_count: u32,
    },
    /// The ZMODEM session failed.
    Failed { reason: String },
}

/// Creates a transfer and optionally auto-accepts a prepared upload.
pub fn start_zmodem_transfer(
    direction: ZmodemDirection,
    initial_bytes: &[u8],
    prepared_upload_files: Option<Vec<PathBuf>>,
) -> (ZmodemTransfer, Vec<ZmodemAction>) {
    let mut transfer = ZmodemTransfer::new(direction, initial_bytes);
    let bootstrap_actions = match (direction, prepared_upload_files) {
        (ZmodemDirection::Upload, Some(files)) => transfer.accept_upload(files),
        _ => Vec::new(),
    };
    (transfer, bootstrap_actions)
}

/// Actions returned to the I/O loop after feeding bytes.
pub enum ZmodemAction {
    /// Send these bytes back to the remote (protocol responses).
    SendToRemote(Vec<u8>),
    /// Emit a Tauri event to the frontend.
    EmitEvent(ZmodemEvent),
}

/// Result of scanning a raw byte chunk for ZMODEM startup.
pub enum ZmodemDetectResult {
    /// No complete header was detected. `passthrough` is known-safe terminal text.
    NoMatch { passthrough: Vec<u8> },
    /// A ZMODEM header was detected.
    Detected {
        direction: ZmodemDirection,
        passthrough: Vec<u8>,
        initial_bytes: Vec<u8>,
    },
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// Scans the raw byte stream for a ZMODEM init header.
///
/// Handles the pattern being split across multiple reads by keeping
/// a small state machine.
pub struct ZmodemDetector {
    /// Bytes withheld until they are known not to be a split ZMODEM header.
    pending: Vec<u8>,
    pending_updated_at: Option<Instant>,
    /// Whether `pending[0]` is at the beginning of the stream or directly after a newline.
    pending_starts_at_line_start: bool,
}

impl Default for ZmodemDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl ZmodemDetector {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            pending_updated_at: None,
            pending_starts_at_line_start: true,
        }
    }

    pub fn is_idle(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn output_may_contain_trigger(data: &[u8]) -> bool {
        data.contains(&ZDLE)
            || data.ends_with(&[ZPAD])
            || data.ends_with(&[ZPAD, ZPAD])
            || data.ends_with(&[ZPAD, ZPAD, ZDLE])
    }

    /// Feed raw bytes and return whether a ZMODEM header was found.
    ///
    /// The direction is inferred from the frame type byte that follows the
    /// header prefix:
    /// - ZRQINIT (0x00) → remote wants to **send** → we **download**
    /// - ZRINIT  (0x01) → remote wants to **receive** → we **upload**
    ///
    /// `passthrough` contains bytes that can be shown in the terminal. Split
    /// header prefixes are retained internally until enough bytes arrive.
    /// When an upload is detected, the "rz -y" shell echo is stripped from
    /// passthrough so the user doesn't see the command.
    pub fn feed(&mut self, data: &[u8]) -> ZmodemDetectResult {
        self.feed_at(data, Instant::now())
    }

    fn feed_at(&mut self, data: &[u8], now: Instant) -> ZmodemDetectResult {
        self.pending.extend_from_slice(data);
        if !data.is_empty() {
            self.pending_updated_at = Some(now);
        }

        if let Some((direction, header_start)) = detect_zmodem_start(&self.pending) {
            let mut passthrough = self.pending[..header_start].to_vec();
            let initial_bytes = self.pending[header_start..].to_vec();
            self.reset();
            if direction == ZmodemDirection::Upload {
                strip_rz_echo(&mut passthrough);
            }
            return ZmodemDetectResult::Detected {
                direction,
                passthrough,
                initial_bytes,
            };
        }

        let keep_from = retained_prefix_start(&self.pending, self.pending_starts_at_line_start);
        let passthrough = self.pending[..keep_from].to_vec();
        if keep_from > 0 {
            self.pending.drain(..keep_from);
            self.pending_starts_at_line_start = ends_at_line_start(&passthrough);
        }
        if self.pending.is_empty() {
            self.pending_updated_at = None;
        }

        ZmodemDetectResult::NoMatch { passthrough }
    }

    /// Release an incomplete header without waiting for another output chunk.
    /// Callers must route these bytes through downstream protocol detectors
    /// before displaying them, preserving their order relative to later output.
    pub fn flush_pending_if_idle(&mut self, now: Instant) -> Vec<u8> {
        let Some(updated_at) = self.pending_updated_at else {
            return Vec::new();
        };
        let timeout = if suffix_contains_zdle(&self.pending) {
            ZMODEM_HEADER_IDLE_TIMEOUT
        } else {
            ZMODEM_ASTERISK_IDLE_TIMEOUT
        };
        if now.saturating_duration_since(updated_at) < timeout {
            return Vec::new();
        }
        let bytes = std::mem::take(&mut self.pending);
        self.pending_updated_at = None;
        self.pending_starts_at_line_start = ends_at_line_start(&bytes);
        bytes
    }

    pub fn reset(&mut self) {
        self.pending.clear();
        self.pending_updated_at = None;
        self.pending_starts_at_line_start = true;
    }
}

fn detect_zmodem_start(data: &[u8]) -> Option<(ZmodemDirection, usize)> {
    for start in 0..data.len() {
        if data.len().saturating_sub(start) < ZMODEM_HEADER_LEN {
            break;
        }

        let header = &data[start..start + ZMODEM_HEADER_LEN];
        if header[0] != ZPAD
            || header[1] != ZPAD
            || header[2] != ZDLE
            || !matches!(header[3], ZHEX | ZBIN | ZBIN32)
        {
            continue;
        }

        let remaining = &data[start + ZMODEM_HEADER_LEN..];
        let frame_type = if header[3] == ZHEX {
            parse_hex_frame_type(remaining)
        } else {
            remaining.first().copied()
        };

        let direction = match frame_type {
            Some(0x00) => Some(ZmodemDirection::Download),
            Some(0x01) => Some(ZmodemDirection::Upload),
            _ => None,
        };

        if let Some(direction) = direction {
            return Some((direction, start));
        }
    }

    None
}

fn retained_prefix_start(data: &[u8], data_starts_at_line_start: bool) -> usize {
    let max_suffix = data.len().min(ZMODEM_HEADER_LEN + 1);
    for len in (1..=max_suffix).rev() {
        let start = data.len() - len;
        let suffix = &data[start..];
        if !is_possible_zmodem_prefix(suffix) {
            continue;
        }
        if suffix_contains_zdle(suffix)
            || is_line_start(data, start, data_starts_at_line_start)
            || has_rz_receive_prompt_before(data, start)
        {
            return start;
        }
    }
    data.len()
}

fn is_line_start(data: &[u8], start: usize, data_starts_at_line_start: bool) -> bool {
    if start == 0 {
        return data_starts_at_line_start;
    }
    matches!(data.get(start - 1), Some(b'\n' | b'\r'))
}

fn ends_at_line_start(data: &[u8]) -> bool {
    matches!(data.last(), Some(b'\n' | b'\r'))
}

fn suffix_contains_zdle(data: &[u8]) -> bool {
    data.contains(&ZDLE)
}

fn has_rz_receive_prompt_before(data: &[u8], start: usize) -> bool {
    const RZ_RECEIVE_PROMPT: &[u8] = b"z waiting to receive.";
    data[..start]
        .windows(RZ_RECEIVE_PROMPT.len())
        .any(|window| window == RZ_RECEIVE_PROMPT)
}

/// Strip the "rz" shell echo from the end of the passthrough data
/// so that the user doesn't see the upload command in the terminal.
/// Handles common echo variants: "rz\r\n", "rz\r", "rz".
fn strip_rz_echo(data: &mut Vec<u8>) {
    // Try to strip from the end: \r\n, \r, or just the command text.
    // The command is always at the end of the passthrough because the
    // ZMODEM header follows immediately.
    let patterns: &[&[u8]] = &[
        b"rz -e -y\r\n",
        b"rz -e -y\r",
        b"rz -e -y",
        b"rz -e\r\n",
        b"rz -e\r",
        b"rz -e",
        b"rz\r\n",
        b"rz\r",
        b"rz",
    ];
    for &pat in patterns {
        if data.ends_with(pat) {
            data.truncate(data.len() - pat.len());
            return;
        }
    }
}

fn is_possible_zmodem_prefix(data: &[u8]) -> bool {
    match data {
        [] => true,
        [ZPAD] => true,
        [ZPAD, ZPAD] => true,
        [ZPAD, ZPAD, ZDLE] => true,
        [ZPAD, ZPAD, ZDLE, kind] => matches!(*kind, ZHEX | ZBIN | ZBIN32),
        [ZPAD, ZPAD, ZDLE, ZHEX, first_hex] => hex_digit(*first_hex).is_some(),
        _ => false,
    }
}

/// Parse a hex-encoded frame type byte from two ASCII hex chars.
fn parse_hex_frame_type(data: &[u8]) -> Option<u8> {
    if data.len() < 2 {
        return None;
    }
    let hi = hex_digit(data[0])?;
    let lo = hex_digit(data[1])?;
    Some((hi << 4) | lo)
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Transfer state machine
// ---------------------------------------------------------------------------

/// Active ZMODEM transfer state, created when a ZMODEM header is detected
/// and the user accepts the transfer via the frontend dialog.
pub struct ZmodemTransfer {
    #[allow(dead_code)]
    direction: ZmodemDirection,
    state: TransferState,
    /// Count of consecutive CAN bytes seen — 5 in a row means abort.
    cancel_count: usize,
    file_count: u32,
    progress_throttle: ProgressThrottle,
    /// Scratch buffer for file bytes handed to a `zmodem2::Sender`.
    send_buf: Vec<u8>,
    /// Scratch buffer for received file bytes staged out of a `zmodem2::Receiver`
    /// poll before they are written to disk.
    file_buf: Vec<u8>,
}

enum TransferState {
    /// Waiting for the frontend to provide save path / file paths.
    WaitingForUser {
        /// Raw bytes buffered while waiting for the user to pick files.
        buffered: Vec<u8>,
    },
    /// Actively receiving files (download / remote `sz`).
    Receiving {
        receiver: zmodem2::Receiver,
        save_dir: PathBuf,
        current_file: Option<ReceiveFile>,
    },
    /// Actively sending files (upload / remote `rz`).
    Sending {
        sender: zmodem2::Sender,
        files: Vec<PathBuf>,
        file_index: usize,
        current_file: Option<SendFile>,
    },
    /// Transfer finished or aborted.
    Done,
}

struct ReceiveFile {
    name: String,
    size: u64,
    file: std::fs::File,
    written: u64,
}

struct SendFile {
    name: String,
    size: u64,
    file: std::fs::File,
    sent: u64,
    position: u64,
}

struct ProgressThrottle {
    last_emit_at: Option<Instant>,
    last_emit_bytes: u64,
}

impl ProgressThrottle {
    fn new() -> Self {
        Self {
            last_emit_at: None,
            last_emit_bytes: 0,
        }
    }

    fn reset(&mut self) {
        self.last_emit_at = None;
        self.last_emit_bytes = 0;
    }

    fn should_emit(&mut self, bytes_transferred: u64, force: bool) -> bool {
        self.should_emit_at(bytes_transferred, force, Instant::now())
    }

    fn should_emit_at(&mut self, bytes_transferred: u64, force: bool, now: Instant) -> bool {
        if force
            || self.last_emit_at.is_none()
            || bytes_transferred.saturating_sub(self.last_emit_bytes) >= ZMODEM_PROGRESS_BYTES
            || self
                .last_emit_at
                .is_some_and(|last| now.duration_since(last) >= ZMODEM_PROGRESS_INTERVAL)
        {
            self.last_emit_at = Some(now);
            self.last_emit_bytes = bytes_transferred;
            return true;
        }

        false
    }
}

impl ZmodemTransfer {
    pub fn new(direction: ZmodemDirection, initial_bytes: &[u8]) -> Self {
        Self {
            direction,
            state: TransferState::WaitingForUser {
                buffered: initial_bytes.to_vec(),
            },
            cancel_count: 0,
            file_count: 0,
            progress_throttle: ProgressThrottle::new(),
            send_buf: Vec::new(),
            file_buf: Vec::new(),
        }
    }

    #[allow(dead_code)]
    pub fn direction(&self) -> ZmodemDirection {
        self.direction
    }

    pub fn is_done(&self) -> bool {
        matches!(self.state, TransferState::Done)
    }

    /// Called when the user cancels the transfer from the frontend.
    pub fn cancel(&mut self) -> Vec<ZmodemAction> {
        self.cancel_with_reason("cancelled")
    }

    pub fn cancel_with_reason(&mut self, reason: impl Into<String>) -> Vec<ZmodemAction> {
        self.state = TransferState::Done;
        vec![
            ZmodemAction::SendToRemote(cancel_sequence()),
            ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                reason: reason.into(),
            }),
        ]
    }

    /// Called when the user accepts a **download** and provides a save directory.
    pub fn accept_download(&mut self, save_dir: PathBuf) -> Vec<ZmodemAction> {
        let buffered = match &mut self.state {
            TransferState::WaitingForUser { buffered } => std::mem::take(buffered),
            _ => return vec![],
        };

        let receiver = match zmodem2::Receiver::new() {
            Ok(r) => r,
            Err(e) => {
                self.state = TransferState::Done;
                return vec![ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                    reason: format!("Failed to create ZMODEM receiver: {e}"),
                })];
            }
        };

        self.state = TransferState::Receiving {
            receiver,
            save_dir,
            current_file: None,
        };

        let mut actions = self.flush_outgoing();
        // Process the buffered bytes that arrived before the user accepted.
        actions.extend(self.feed_incoming(&buffered));
        actions
    }

    /// Called when the user accepts an **upload** and provides file paths.
    pub fn accept_upload(&mut self, files: Vec<PathBuf>) -> Vec<ZmodemAction> {
        let buffered = match &mut self.state {
            TransferState::WaitingForUser { buffered } => std::mem::take(buffered),
            _ => return vec![],
        };

        let sender = match zmodem2::Sender::new() {
            Ok(s) => s,
            Err(e) => {
                self.state = TransferState::Done;
                return vec![ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                    reason: format!("Failed to create ZMODEM sender: {e}"),
                })];
            }
        };

        self.state = TransferState::Sending {
            sender,
            files,
            file_index: 0,
            current_file: None,
        };

        let mut actions = Vec::new();
        // 1. Push out the Sender's initial bytes (ZRQINIT).
        actions.extend(self.flush_outgoing());
        // 2. Feed the buffered remote ZRINIT so the Sender knows the
        //    receiver's capabilities.
        actions.extend(self.feed_incoming(&buffered));
        // 3. Start the first file (queues the ZFILE frame).
        actions.extend(self.start_next_send_file());
        // 4. Push out that ZFILE frame.
        actions.extend(self.flush_outgoing());
        actions
    }

    /// Feed raw bytes from the remote into the transfer state machine.
    pub fn feed_incoming(&mut self, data: &[u8]) -> Vec<ZmodemAction> {
        // Check for cancel sequence (5+ consecutive CAN/ZDLE bytes).
        for &b in data {
            if b == ZDLE {
                self.cancel_count += 1;
                if self.cancel_count >= CANCEL_SEQ_LEN {
                    self.state = TransferState::Done;
                    return vec![ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                        reason: "Remote cancelled transfer".to_string(),
                    })];
                }
            } else {
                self.cancel_count = 0;
            }
        }

        match &mut self.state {
            TransferState::WaitingForUser { buffered } => {
                buffered.extend_from_slice(data);
                vec![]
            }
            TransferState::Receiving { .. } => self.feed_receiver(data),
            TransferState::Sending { .. } => self.feed_sender(data),
            TransferState::Done => vec![],
        }
    }

    fn feed_receiver(&mut self, data: &[u8]) -> Vec<ZmodemAction> {
        let mut actions = Vec::new();
        let mut offset = 0;

        loop {
            if !self.pump_receiver(&mut actions) {
                return actions;
            }

            if offset >= data.len() {
                break;
            }

            let TransferState::Receiving { receiver, .. } = &mut self.state else {
                break;
            };

            match receiver.submit_wire(&data[offset..]) {
                // Nothing more can be consumed until further input arrives.
                Ok(0) => break,
                Ok(consumed) => offset += consumed,
                Err(e) => {
                    zmodem_log!("ZMODEM receive error: {e}");
                    if matches!(
                        e,
                        zmodem2::Error::UnexpectedCrc16 | zmodem2::Error::UnexpectedCrc32
                    ) {
                        // The state machine has already queued a NAK; resynchronise
                        // on the next byte.
                        offset += 1;
                        continue;
                    }
                    self.state = TransferState::Done;
                    actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                        reason: format!("ZMODEM protocol error: {e}"),
                    }));
                    return actions;
                }
            }
        }

        actions
    }

    /// Drains everything the receiver has queued — wire responses, file data and
    /// events — until it reports [`zmodem2::Action::Idle`].
    ///
    /// `poll` borrows the state machine for as long as the action it returns is
    /// alive, so each action is copied out before it is acted on. Returns `false`
    /// once the transfer has reached a terminal state, in which case the closing
    /// event has already been pushed.
    fn pump_receiver(&mut self, actions: &mut Vec<ZmodemAction>) -> bool {
        loop {
            let TransferState::Receiving {
                receiver,
                save_dir,
                current_file,
            } = &mut self.state
            else {
                return false;
            };

            let step = match receiver.poll() {
                zmodem2::Action::WriteWire(bytes) => ReceiverStep::WriteWire(bytes.to_vec()),
                zmodem2::Action::WriteFile(bytes) => {
                    self.file_buf.clear();
                    self.file_buf.extend_from_slice(bytes);
                    ReceiverStep::WriteFile
                }
                zmodem2::Action::Event(zmodem2::Event::FileStarted(info)) => {
                    ReceiverStep::FileStarted {
                        name: sanitize_filename(&String::from_utf8_lossy(info.name)),
                        size: info.size.map_or(0, |size| u64::from(size.get())),
                    }
                }
                zmodem2::Action::Event(zmodem2::Event::FileCompleted) => {
                    ReceiverStep::FileCompleted
                }
                zmodem2::Action::Event(zmodem2::Event::SessionCompleted) => {
                    ReceiverStep::SessionCompleted
                }
                zmodem2::Action::Event(zmodem2::Event::Aborted) => ReceiverStep::Aborted,
                // `Idle`, the sender-only `ReadFile`, and anything a later zmodem2
                // adds to these non-exhaustive enums: no work for this side.
                _ => return true,
            };

            match step {
                ReceiverStep::WriteWire(bytes) => {
                    let len = bytes.len();
                    actions.push(ZmodemAction::SendToRemote(bytes));
                    receiver.wire_written(len);
                }
                ReceiverStep::WriteFile => {
                    let Some(rf) = current_file.as_mut() else {
                        // File data before any file header: the remote is not
                        // following the protocol.
                        self.state = TransferState::Done;
                        actions.push(ZmodemAction::SendToRemote(cancel_sequence()));
                        actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                            reason: "ZMODEM file data arrived before any file header".to_string(),
                        }));
                        return false;
                    };
                    if let Err(e) = rf.file.write_all(&self.file_buf) {
                        zmodem_log!("Failed to write file data: {e}");
                        self.state = TransferState::Done;
                        actions.push(ZmodemAction::SendToRemote(cancel_sequence()));
                        actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                            reason: format!("File write error: {e}"),
                        }));
                        return false;
                    }
                    let written = self.file_buf.len();
                    rf.written += written as u64;
                    if self.progress_throttle.should_emit(rf.written, false) {
                        actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Progress {
                            file_name: rf.name.clone(),
                            bytes_transferred: rf.written,
                            total_size: rf.size,
                            direction: ZmodemDirection::Download,
                        }));
                    }
                    if let Err(_e) = receiver.file_written(written) {
                        zmodem_log!("file_written error: {e}");
                    }
                }

                ReceiverStep::FileStarted { name, size } => {
                    let file_path = save_dir.join(&name);
                    zmodem_log!(
                        file = %file_path.display(),
                        size,
                        "ZMODEM receiving file"
                    );
                    match std::fs::File::create(&file_path) {
                        Ok(file) => {
                            self.progress_throttle.reset();
                            *current_file = Some(ReceiveFile {
                                name: name.clone(),
                                size,
                                file,
                                written: 0,
                            });
                            if self.progress_throttle.should_emit(0, true) {
                                actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Progress {
                                    file_name: name,
                                    bytes_transferred: 0,
                                    total_size: size,
                                    direction: ZmodemDirection::Download,
                                }));
                            }
                        }
                        Err(e) => {
                            zmodem_log!("Failed to create file {}: {e}", file_path.display());
                            self.state = TransferState::Done;
                            actions.push(ZmodemAction::SendToRemote(cancel_sequence()));
                            actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                                reason: format!("Failed to create file: {e}"),
                            }));
                            return false;
                        }
                    }
                }

                ReceiverStep::FileCompleted => {
                    if let Some(rf) = current_file.as_mut() {
                        if self.progress_throttle.should_emit(rf.written, true) {
                            actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Progress {
                                file_name: rf.name.clone(),
                                bytes_transferred: rf.written,
                                total_size: rf.size,
                                direction: ZmodemDirection::Download,
                            }));
                        }
                        let _ = rf.file.flush();
                    }
                    self.file_count += 1;
                    *current_file = None;
                }
                ReceiverStep::SessionCompleted => {
                    // The state machine queues its closing ZFIN before it pushes
                    // the event, and `poll` reports events first, so flush what is
                    // still queued: the remote is waiting for that ZFIN.
                    flush_receiver_wire(receiver, actions);
                    self.state = TransferState::Done;
                    actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Complete {
                        direction: ZmodemDirection::Download,
                        file_count: self.file_count,
                    }));
                    return false;
                }
                ReceiverStep::Aborted => {
                    flush_receiver_wire(receiver, actions);
                    self.state = TransferState::Done;
                    actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                        reason: "Remote aborted the ZMODEM session".to_string(),
                    }));
                    return false;
                }
            }
        }
    }

    fn feed_sender(&mut self, data: &[u8]) -> Vec<ZmodemAction> {
        let mut actions = Vec::new();
        let mut offset = 0;

        loop {
            if !self.pump_sender(&mut actions) {
                return actions;
            }

            if offset >= data.len() {
                break;
            }

            let TransferState::Sending { sender, .. } = &mut self.state else {
                break;
            };

            match sender.submit_wire(&data[offset..]) {
                Ok(0) => break,
                Ok(consumed) => offset += consumed,
                Err(e) => {
                    zmodem_log!("ZMODEM send error: {e}");
                    if matches!(
                        e,
                        zmodem2::Error::UnexpectedCrc16 | zmodem2::Error::UnexpectedCrc32
                    ) {
                        offset += 1;
                        continue;
                    }
                    self.state = TransferState::Done;
                    actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                        reason: format!("ZMODEM send error: {e}"),
                    }));
                    return actions;
                }
            }
        }

        actions
    }

    /// The sending counterpart of [`Self::pump_receiver`]: wire output, file read
    /// requests, and events, until the sender goes idle.
    fn pump_sender(&mut self, actions: &mut Vec<ZmodemAction>) -> bool {
        loop {
            let TransferState::Sending {
                sender,
                files,
                file_index,
                current_file,
            } = &mut self.state
            else {
                return false;
            };

            let step = match sender.poll() {
                zmodem2::Action::WriteWire(bytes) => SenderStep::WriteWire(bytes.to_vec()),
                zmodem2::Action::ReadFile { offset, max_len } => SenderStep::ReadFile {
                    offset: u64::from(offset.get()),
                    max_len,
                },
                zmodem2::Action::Event(zmodem2::Event::FileCompleted) => SenderStep::FileCompleted,
                zmodem2::Action::Event(zmodem2::Event::SessionCompleted) => {
                    SenderStep::SessionCompleted
                }
                zmodem2::Action::Event(zmodem2::Event::Aborted) => SenderStep::Aborted,
                _ => return true,
            };

            match step {
                SenderStep::WriteWire(bytes) => {
                    let len = bytes.len();
                    actions.push(ZmodemAction::SendToRemote(bytes));
                    sender.wire_written(len);
                }

                SenderStep::ReadFile { offset, max_len } => {
                    let Some(sf) = current_file.as_mut() else {
                        // Nothing to answer with. Stopping here rather than
                        // ignoring the request keeps the pump from spinning on a
                        // request it can never satisfy.
                        self.state = TransferState::Done;
                        actions.push(ZmodemAction::SendToRemote(cancel_sequence()));
                        actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                            reason: "ZMODEM sender requested data with no file open".to_string(),
                        }));
                        return false;
                    };

                    if sf.position != offset {
                        if let Err(e) = sf.file.seek(SeekFrom::Start(offset)) {
                            zmodem_log!("File seek error: {e}");
                            self.state = TransferState::Done;
                            actions.push(ZmodemAction::SendToRemote(cancel_sequence()));
                            actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                                reason: format!("File seek error: {e}"),
                            }));
                            return false;
                        }
                        sf.position = offset;
                    }

                    if self.send_buf.len() < max_len {
                        self.send_buf.resize(max_len, 0);
                    }
                    let read = match sf.file.read(&mut self.send_buf[..max_len]) {
                        Ok(0) => {
                            // The advertised size promised more than the file
                            // holds, and an empty chunk is not a legal answer.
                            self.state = TransferState::Done;
                            actions.push(ZmodemAction::SendToRemote(cancel_sequence()));
                            actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                                reason: "File ended before the advertised size".to_string(),
                            }));
                            return false;
                        }
                        Ok(read) => read,
                        Err(e) => {
                            zmodem_log!("File read error: {e}");
                            self.state = TransferState::Done;
                            actions.push(ZmodemAction::SendToRemote(cancel_sequence()));
                            actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                                reason: format!("File read error: {e}"),
                            }));
                            return false;
                        }
                    };

                    sf.sent = sf.sent.max(offset + read as u64);
                    sf.position += read as u64;
                    let sent = sf.sent;
                    let file_name = sf.name.clone();
                    let total_size = sf.size;
                    if self.progress_throttle.should_emit(sent, false) {
                        actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Progress {
                            file_name,
                            bytes_transferred: sent,
                            total_size,
                            direction: ZmodemDirection::Upload,
                        }));
                    }
                    if let Err(e) = sender.submit_file(&self.send_buf[..read]) {
                        zmodem_log!("submit_file error: {e}");
                        self.state = TransferState::Done;
                        actions.push(ZmodemAction::SendToRemote(cancel_sequence()));
                        actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                            reason: format!("ZMODEM send error: {e}"),
                        }));
                        return false;
                    }
                }

                SenderStep::FileCompleted => {
                    if let Some(sf) = current_file.as_ref()
                        && self.progress_throttle.should_emit(sf.sent, true)
                    {
                        actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Progress {
                            file_name: sf.name.clone(),
                            bytes_transferred: sf.sent,
                            total_size: sf.size,
                            direction: ZmodemDirection::Upload,
                        }));
                    }
                    self.file_count += 1;
                    *current_file = None;
                    *file_index += 1;
                    if *file_index < files.len() {
                        self.progress_throttle.reset();
                        actions.extend(Self::start_file_for_sender(
                            sender,
                            files,
                            *file_index,
                            current_file,
                        ));
                        if let Some(sf) = current_file.as_ref()
                            && self.progress_throttle.should_emit(0, true)
                        {
                            actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Progress {
                                file_name: sf.name.clone(),
                                bytes_transferred: 0,
                                total_size: sf.size,
                                direction: ZmodemDirection::Upload,
                            }));
                        }
                    } else if let Err(_e) = sender.finish() {
                        zmodem_log!("finish error: {e}");
                    }
                }

                SenderStep::SessionCompleted => {
                    flush_sender_wire(sender, actions);
                    self.state = TransferState::Done;
                    actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Complete {
                        direction: ZmodemDirection::Upload,
                        file_count: self.file_count,
                    }));
                    return false;
                }
                SenderStep::Aborted => {
                    flush_sender_wire(sender, actions);
                    self.state = TransferState::Done;
                    actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                        reason: "Remote aborted the ZMODEM session".to_string(),
                    }));
                    return false;
                }
            }
        }
    }

    fn start_next_send_file(&mut self) -> Vec<ZmodemAction> {
        let TransferState::Sending {
            sender,
            files,
            file_index,
            current_file,
        } = &mut self.state
        else {
            return vec![];
        };

        if *file_index >= files.len() {
            return vec![];
        }

        self.progress_throttle.reset();
        let mut actions = Self::start_file_for_sender(sender, files, *file_index, current_file);
        if let Some(sf) = current_file
            && self.progress_throttle.should_emit(0, true)
        {
            actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Progress {
                file_name: sf.name.clone(),
                bytes_transferred: 0,
                total_size: sf.size,
                direction: ZmodemDirection::Upload,
            }));
        }
        actions
    }

    fn start_file_for_sender(
        sender: &mut zmodem2::Sender,
        files: &[PathBuf],
        index: usize,
        current_file: &mut Option<SendFile>,
    ) -> Vec<ZmodemAction> {
        let path = &files[index];
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();

        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) => {
                return vec![ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                    reason: format!("Failed to open {file_name}: {e}"),
                })];
            }
        };

        let metadata = match file.metadata() {
            Ok(m) => m,
            Err(e) => {
                return vec![ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                    reason: format!("Failed to read metadata for {file_name}: {e}"),
                })];
            }
        };

        let size = metadata.len();
        // zmodem2 carries the file size as a u32.
        let size_u32 = u32::try_from(size).unwrap_or(u32::MAX);
        let info =
            zmodem2::FileInfo::new(file_name.as_bytes(), Some(zmodem2::Position::new(size_u32)));

        if let Err(e) = sender.start_file(info) {
            return vec![ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                reason: format!("start_file error: {e}"),
            })];
        }

        *current_file = Some(SendFile {
            name: file_name,
            size,
            file,
            sent: 0,
            position: 0,
        });

        vec![]
    }

    /// Pushes out whatever the active state machine has already queued, without
    /// feeding it any new input.
    fn flush_outgoing(&mut self) -> Vec<ZmodemAction> {
        let mut actions = Vec::new();
        if matches!(self.state, TransferState::Receiving { .. }) {
            self.pump_receiver(&mut actions);
        } else if matches!(self.state, TransferState::Sending { .. }) {
            self.pump_sender(&mut actions);
        }
        actions
    }
}

/// One unit of work taken out of a [`zmodem2::Receiver`] poll, with the payload
/// copied out: `poll` borrows the state machine for as long as the action it
/// returns is alive, and every handler needs to call back into it.
enum ReceiverStep {
    WriteWire(Vec<u8>),
    /// File bytes staged in `ZmodemTransfer::file_buf`.
    WriteFile,
    FileStarted {
        name: String,
        size: u64,
    },
    FileCompleted,
    SessionCompleted,
    Aborted,
}

/// The sending counterpart of [`ReceiverStep`].
enum SenderStep {
    WriteWire(Vec<u8>),
    ReadFile { offset: u64, max_len: usize },
    FileCompleted,
    SessionCompleted,
    Aborted,
}

/// Pushes out the wire bytes a receiver has already queued, stopping at anything
/// else. Needed on the terminal events, which `poll` reports before the closing
/// frame queued alongside them.
fn flush_receiver_wire(receiver: &mut zmodem2::Receiver, actions: &mut Vec<ZmodemAction>) {
    loop {
        let payload = match receiver.poll() {
            zmodem2::Action::WriteWire(bytes) => bytes.to_vec(),
            _ => return,
        };
        let len = payload.len();
        actions.push(ZmodemAction::SendToRemote(payload));
        receiver.wire_written(len);
    }
}

/// The sending counterpart of [`flush_receiver_wire`].
fn flush_sender_wire(sender: &mut zmodem2::Sender, actions: &mut Vec<ZmodemAction>) {
    loop {
        let payload = match sender.poll() {
            zmodem2::Action::WriteWire(bytes) => bytes.to_vec(),
            _ => return,
        };
        let len = payload.len();
        actions.push(ZmodemAction::SendToRemote(payload));
        sender.wire_written(len);
    }
}

/// Remove path separators and invalid characters from a filename.
fn sanitize_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let sanitized: String = base
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();

    if sanitized.is_empty() {
        "zmodem_file".to_string()
    } else {
        sanitized
    }
}

/// Build a 5×CAN + 5×BS abort/cancel sequence per ZMODEM spec.
fn cancel_sequence() -> Vec<u8> {
    let mut seq = vec![ZDLE; CANCEL_SEQ_LEN];
    seq.extend([0x08; CANCEL_SEQ_LEN]); // backspace to clean up display
    seq
}

#[cfg(test)]
mod tests {
    use super::{
        ProgressThrottle, ZMODEM_PROGRESS_BYTES, ZMODEM_PROGRESS_INTERVAL, ZmodemAction,
        ZmodemDetectResult, ZmodemDetector, ZmodemDirection, ZmodemEvent, ZmodemTransfer,
        cancel_sequence,
    };
    use std::time::{Duration, Instant};

    fn detected_direction(result: ZmodemDetectResult) -> ZmodemDirection {
        match result {
            ZmodemDetectResult::Detected { direction, .. } => direction,
            ZmodemDetectResult::NoMatch { .. } => panic!("expected ZMODEM detection"),
        }
    }

    #[test]
    fn fast_scan_predicate_skips_ordinary_output() {
        assert!(!ZmodemDetector::output_may_contain_trigger(
            b"Last login: Wed Jul 15\r\n$ "
        ));
        assert!(ZmodemDetector::new().is_idle());
    }

    #[test]
    fn fast_scan_predicate_keeps_split_header_prefixes() {
        assert!(ZmodemDetector::output_may_contain_trigger(b"*"));
        assert!(ZmodemDetector::output_may_contain_trigger(b"**"));
        assert!(ZmodemDetector::output_may_contain_trigger(b"**\x18"));
    }

    #[test]
    fn detects_complete_zhex_download_header() {
        let mut detector = ZmodemDetector::new();
        let result = detector.feed(b"ready\r\n**\x18B00");

        match result {
            ZmodemDetectResult::Detected {
                direction,
                passthrough,
                initial_bytes,
            } => {
                assert_eq!(direction, ZmodemDirection::Download);
                assert_eq!(passthrough, b"ready\r\n");
                assert_eq!(initial_bytes, b"**\x18B00");
            }
            ZmodemDetectResult::NoMatch { .. } => panic!("expected ZMODEM detection"),
        }
    }

    #[test]
    fn detects_zbin_upload_header_across_chunks() {
        let mut detector = ZmodemDetector::new();
        match detector.feed(b"prefix\r\n**\x18") {
            ZmodemDetectResult::NoMatch { passthrough } => {
                assert_eq!(passthrough, b"prefix\r\n")
            }
            ZmodemDetectResult::Detected { .. } => panic!("unexpected early detection"),
        }

        let result = detector.feed(b"A\x01payload");
        match result {
            ZmodemDetectResult::Detected {
                direction,
                passthrough,
                initial_bytes,
            } => {
                assert_eq!(direction, ZmodemDirection::Upload);
                assert!(passthrough.is_empty());
                assert_eq!(initial_bytes, b"**\x18A\x01payload");
            }
            ZmodemDetectResult::NoMatch { .. } => panic!("expected ZMODEM detection"),
        }
    }

    #[test]
    fn detects_rz_zhex_upload_header_after_prompt_text() {
        let mut detector = ZmodemDetector::new();
        let result = detector.feed(b"\x18z waiting to receive.**\x18B01");

        match result {
            ZmodemDetectResult::Detected {
                direction,
                passthrough,
                initial_bytes,
            } => {
                assert_eq!(direction, ZmodemDirection::Upload);
                assert_eq!(passthrough, b"\x18z waiting to receive.");
                assert_eq!(initial_bytes, b"**\x18B01");
            }
            ZmodemDetectResult::NoMatch { .. } => panic!("expected ZMODEM detection"),
        }
    }

    #[test]
    fn detects_rz_zhex_upload_header_split_after_prompt_text() {
        let mut detector = ZmodemDetector::new();
        match detector.feed(b"\x18z waiting to receive.**") {
            ZmodemDetectResult::NoMatch { passthrough } => {
                assert_eq!(passthrough, b"\x18z waiting to receive.")
            }
            ZmodemDetectResult::Detected { .. } => panic!("unexpected early detection"),
        }

        assert_eq!(
            detected_direction(detector.feed(b"\x18B01")),
            ZmodemDirection::Upload
        );
    }

    #[test]
    fn detects_zhex_frame_type_split_after_first_hex_digit() {
        let mut detector = ZmodemDetector::new();
        match detector.feed(b"**\x18B0") {
            ZmodemDetectResult::NoMatch { passthrough } => assert!(passthrough.is_empty()),
            ZmodemDetectResult::Detected { .. } => panic!("unexpected early detection"),
        }

        assert_eq!(
            detected_direction(detector.feed(b"1rest")),
            ZmodemDirection::Upload
        );
    }

    #[test]
    fn isolated_asterisks_are_released_without_another_output_chunk() {
        for text in [b"*".as_slice(), b"**", b"\r\n*", b"\r\n**"] {
            let mut detector = ZmodemDetector::new();
            let now = Instant::now();
            let ZmodemDetectResult::NoMatch { mut passthrough } = detector.feed_at(text, now)
            else {
                panic!("ordinary asterisks must not trigger a transfer");
            };
            assert!(
                detector
                    .flush_pending_if_idle(now + Duration::from_millis(25))
                    .is_empty()
            );
            passthrough.extend(detector.flush_pending_if_idle(now + Duration::from_millis(100)));
            assert_eq!(passthrough, text);
            assert!(detector.is_idle());
            assert!(
                detector
                    .flush_pending_if_idle(now + Duration::from_secs(2))
                    .is_empty()
            );
            let ZmodemDetectResult::NoMatch { passthrough } = detector.feed(b"*") else {
                panic!("ordinary asterisk must not trigger a transfer");
            };
            assert_eq!(passthrough, b"*", "idle flush must preserve line position");
        }
    }

    #[test]
    fn repeated_asterisk_echoes_and_backspace_preserve_wire_order() {
        let mut detector = ZmodemDetector::new();
        let started_at = Instant::now();
        let mut visible = Vec::new();
        for press in 0..7 {
            let now = started_at + Duration::from_millis(press * 300);
            let ZmodemDetectResult::NoMatch { passthrough } = detector.feed_at(b"*", now) else {
                panic!("asterisk echo must not trigger a transfer");
            };
            visible.extend(passthrough);
            visible.extend(detector.flush_pending_if_idle(now + Duration::from_millis(100)));
            assert_eq!(visible, vec![b'*'; press as usize + 1]);
        }
        let ZmodemDetectResult::NoMatch { passthrough } = detector.feed(b"\x08\x1b[K") else {
            panic!("backspace must not trigger a transfer");
        };
        assert_eq!(passthrough, b"\x08\x1b[K");

        detector.reset();
        detector.feed_at(b"**", started_at);
        let ZmodemDetectResult::NoMatch { passthrough } =
            detector.feed_at(b"\x08\x1b[K", started_at + Duration::from_millis(20))
        else {
            panic!("backspace must release held asterisks");
        };
        assert_eq!(passthrough, b"**\x08\x1b[K");
        assert!(
            detector
                .flush_pending_if_idle(started_at + Duration::from_secs(2))
                .is_empty()
        );
    }

    #[test]
    fn idle_flush_preserves_split_zmodem_headers() {
        for (header, direction) in [
            (b"**\x18B00".as_slice(), ZmodemDirection::Download),
            (b"**\x18B01".as_slice(), ZmodemDirection::Upload),
            (b"**\x18A\x01".as_slice(), ZmodemDirection::Upload),
            (b"**\x18C\x00".as_slice(), ZmodemDirection::Download),
        ] {
            for split in 1..header.len() {
                let mut detector = ZmodemDetector::new();
                let now = Instant::now();
                let ZmodemDetectResult::NoMatch { passthrough } =
                    detector.feed_at(&header[..split], now)
                else {
                    panic!("incomplete header must wait for remaining bytes");
                };
                assert!(passthrough.is_empty());
                assert!(
                    detector
                        .flush_pending_if_idle(now + Duration::from_millis(25))
                        .is_empty()
                );
                let ZmodemDetectResult::Detected {
                    direction: actual_direction,
                    passthrough,
                    initial_bytes,
                } = detector.feed_at(&header[split..], now + Duration::from_millis(30))
                else {
                    panic!("split header must still be detected at boundary {split}");
                };
                assert_eq!(actual_direction, direction);
                assert_eq!(initial_bytes, header);
                assert!(passthrough.is_empty());
                assert!(
                    detector
                        .flush_pending_if_idle(now + Duration::from_secs(2))
                        .is_empty()
                );
            }
        }
    }

    #[test]
    fn recognizable_header_gets_more_time_but_eventually_releases() {
        let mut detector = ZmodemDetector::new();
        let now = Instant::now();
        detector.feed_at(b"**\x18B0", now);
        assert!(
            detector
                .flush_pending_if_idle(now + Duration::from_millis(100))
                .is_empty()
        );
        // Empty polls must not postpone the idle deadline.
        detector.feed_at(b"", now + Duration::from_millis(900));
        assert_eq!(
            detector.flush_pending_if_idle(now + Duration::from_secs(1)),
            b"**\x18B0"
        );
        assert!(detector.is_idle());
    }

    #[test]
    fn reset_discards_pending_header_before_idle_flush() {
        let mut detector = ZmodemDetector::new();
        let now = Instant::now();
        detector.feed_at(b"**", now);
        detector.reset();
        assert!(
            detector
                .flush_pending_if_idle(now + Duration::from_secs(2))
                .is_empty()
        );
        detector.feed_at(b"*", now);
        assert_eq!(
            detector.flush_pending_if_idle(now + Duration::from_millis(100)),
            b"*"
        );
    }

    #[test]
    fn passthroughs_interactive_asterisks_immediately() {
        let mut detector = ZmodemDetector::new();
        let chunks: [&[u8]; 4] = [b"docker", b"*", b"*", b"*"];
        let mut visible = Vec::new();

        for chunk in chunks {
            match detector.feed(chunk) {
                ZmodemDetectResult::NoMatch { passthrough } => {
                    visible.extend_from_slice(&passthrough);
                }
                ZmodemDetectResult::Detected { .. } => panic!("unexpected ZMODEM detection"),
            }
        }

        assert_eq!(visible, b"docker***");
    }

    #[test]
    fn waits_for_zmodem_header_after_zdle_even_inside_text() {
        let mut detector = ZmodemDetector::new();
        match detector.feed(b"prefix**\x18") {
            ZmodemDetectResult::NoMatch { passthrough } => {
                assert_eq!(passthrough, b"prefix")
            }
            ZmodemDetectResult::Detected { .. } => panic!("unexpected ZMODEM detection"),
        }

        assert_eq!(
            detected_direction(detector.feed(b"A\x01payload")),
            ZmodemDirection::Upload
        );
    }

    #[test]
    fn cancel_with_reason_sends_abort_and_reports_reason() {
        let mut transfer = ZmodemTransfer::new(ZmodemDirection::Download, b"**\x18B00");

        let actions = transfer.cancel_with_reason("terminal output dropped 8 byte(s)");

        assert!(transfer.is_done());
        assert!(matches!(
            actions.first(),
            Some(ZmodemAction::SendToRemote(bytes)) if bytes == &cancel_sequence()
        ));
        assert!(matches!(
            actions.get(1),
            Some(ZmodemAction::EmitEvent(ZmodemEvent::Failed { reason }))
                if reason == "terminal output dropped 8 byte(s)"
        ));
    }

    #[test]
    fn progress_throttle_emits_first_time() {
        let mut throttle = ProgressThrottle::new();
        assert!(throttle.should_emit_at(0, false, Instant::now()));
    }

    #[test]
    fn progress_throttle_respects_time_and_byte_thresholds() {
        let mut throttle = ProgressThrottle::new();
        let start = Instant::now();
        assert!(throttle.should_emit_at(0, false, start));
        assert!(!throttle.should_emit_at(1, false, start + Duration::from_millis(10)));
        assert!(throttle.should_emit_at(
            ZMODEM_PROGRESS_BYTES,
            false,
            start + Duration::from_millis(20)
        ));
        assert!(throttle.should_emit_at(
            ZMODEM_PROGRESS_BYTES + 1,
            false,
            start + ZMODEM_PROGRESS_INTERVAL + Duration::from_millis(30)
        ));
    }

    #[test]
    fn progress_throttle_force_emits_completion() {
        let mut throttle = ProgressThrottle::new();
        let start = Instant::now();
        assert!(throttle.should_emit_at(128, false, start));
        assert!(throttle.should_emit_at(129, true, start + Duration::from_millis(1)));
    }

    /// Wires an uploading `ZmodemTransfer` to a downloading one and shuttles the
    /// protocol bytes between them. This is the only test that drives the
    /// `zmodem2` poll/submit pump, so it is what covers `pump_sender` and
    /// `pump_receiver` rather than the header detection above.
    #[test]
    fn loopback_transfer_reproduces_the_file_on_the_receiving_side() {
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-zmodem");
        let src_dir = root.join("src");
        let dst_dir = root.join("dst");
        std::fs::create_dir_all(&src_dir).expect("create source dir");
        std::fs::create_dir_all(&dst_dir).expect("create destination dir");

        // Larger than one subpacket and not a multiple of it, so the transfer has
        // to span frames and end on a partial one.
        let payload: Vec<u8> = (0..40_003u32).map(|i| (i % 251) as u8).collect();
        let src_file = src_dir.join("payload.bin");
        std::fs::write(&src_file, &payload).expect("write source file");

        let mut upload = ZmodemTransfer::new(ZmodemDirection::Upload, &[]);
        let mut download = ZmodemTransfer::new(ZmodemDirection::Download, &[]);

        // The receiving side speaks first with its ZRINIT, exactly as a remote
        // `rz` would.
        let mut to_upload = collect_wire(download.accept_download(dst_dir.clone()));
        let mut to_download = collect_wire(upload.accept_upload(vec![src_file]));

        let mut upload_complete = None;
        let mut download_complete = None;
        let mut failures = Vec::new();

        for _ in 0..10_000 {
            if to_upload.is_empty() && to_download.is_empty() {
                break;
            }

            let inbound_upload = std::mem::take(&mut to_upload);
            let inbound_download = std::mem::take(&mut to_download);

            for action in upload.feed_incoming(&inbound_upload) {
                match action {
                    ZmodemAction::SendToRemote(bytes) => to_download.extend_from_slice(&bytes),
                    ZmodemAction::EmitEvent(ZmodemEvent::Complete { file_count, .. }) => {
                        upload_complete = Some(file_count);
                    }
                    ZmodemAction::EmitEvent(ZmodemEvent::Failed { reason }) => {
                        failures.push(format!("upload: {reason}"));
                    }
                    ZmodemAction::EmitEvent(_) => {}
                }
            }

            for action in download.feed_incoming(&inbound_download) {
                match action {
                    ZmodemAction::SendToRemote(bytes) => to_upload.extend_from_slice(&bytes),
                    ZmodemAction::EmitEvent(ZmodemEvent::Complete { file_count, .. }) => {
                        download_complete = Some(file_count);
                    }
                    ZmodemAction::EmitEvent(ZmodemEvent::Failed { reason }) => {
                        failures.push(format!("download: {reason}"));
                    }
                    ZmodemAction::EmitEvent(_) => {}
                }
            }
        }

        assert!(
            failures.is_empty(),
            "transfer reported failures: {failures:?}"
        );
        assert_eq!(
            upload_complete,
            Some(1),
            "sender did not finish its session"
        );
        assert_eq!(
            download_complete,
            Some(1),
            "receiver did not finish its session"
        );
        assert!(upload.is_done() && download.is_done());

        let received = std::fs::read(dst_dir.join("payload.bin")).expect("read received file");
        assert_eq!(received.len(), payload.len());
        assert_eq!(received, payload);
    }

    fn collect_wire(actions: Vec<ZmodemAction>) -> Vec<u8> {
        let mut wire = Vec::new();
        for action in actions {
            if let ZmodemAction::SendToRemote(bytes) = action {
                wire.extend_from_slice(&bytes);
            }
        }
        wire
    }
}

/// Ask the peer to escape controls; overwrite only after conflict resolution.
pub fn receive_command(overwrite: bool) -> Vec<u8> {
    if overwrite {
        b"rz -e -y\r".to_vec()
    } else {
        b"rz -e\r".to_vec()
    }
}

#[cfg(test)]
mod receive_command_tests {
    use super::{receive_command, strip_rz_echo};
    #[test]
    fn receive_commands_request_control_escaping_and_strip_their_echo() {
        for overwrite in [false, true] {
            let command = receive_command(overwrite);
            assert_eq!(
                command,
                if overwrite {
                    b"rz -e -y\r".to_vec()
                } else {
                    b"rz -e\r".to_vec()
                }
            );
            let mut echoed = b"prompt> ".to_vec();
            echoed.extend(command);
            echoed.push(b'\n');
            strip_rz_echo(&mut echoed);
            assert_eq!(echoed, b"prompt> ");
        }
    }
}
