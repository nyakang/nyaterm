#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapturePhase {
    /// Just registered — suppress all output (the echoed command text)
    /// until the real START marker appears in execution output.
    WaitingForStart,
    /// Between START and END markers — buffer output for the AI.
    Capturing,
    /// After the END marker — suppress the shell prompt that follows,
    /// then remove the capture.
    PostCapture,
}

/// Tracks one in-flight capture request.
struct ActiveCapture {
    buffer: String,
    phase: CapturePhase,
    start_time: Instant,
    result_tx: Option<oneshot::Sender<CapturedOutput>>,
    source_truncated: bool,
}

/// Shared processor that all IO loops (SSH, PTY, Telnet, Serial) can use to
/// intercept marker sequences in the output stream.
pub struct OutputCaptureProcessor {
    active: HashMap<String, ActiveCapture>,
    pending_marker_tail: String,
}

impl OutputCaptureProcessor {
    pub fn new() -> Self {
        Self {
            active: HashMap::new(),
            pending_marker_tail: String::new(),
        }
    }

    /// Register a new capture. The caller should then write the
    /// `build_capture_command()` output into the PTY.
    ///
    /// From this point, all output is suppressed until the START marker
    /// appears (hiding the echoed command text).
    pub fn register(&mut self, marker_id: String, result_tx: oneshot::Sender<CapturedOutput>) {
        self.active.insert(
            marker_id,
            ActiveCapture {
                buffer: String::new(),
                phase: CapturePhase::WaitingForStart,
                start_time: Instant::now(),
                result_tx: Some(result_tx),
                source_truncated: false,
            },
        );
    }

    /// Returns true when at least one capture is in progress.
    pub fn has_active(&self) -> bool {
        !self.active.is_empty()
    }

    /// Cancel a capture by marker id (e.g. on timeout from the caller side).
    #[allow(dead_code)]
    pub fn cancel(&mut self, marker_id: &str) {
        self.active.remove(marker_id);
        if self.active.is_empty() {
            self.pending_marker_tail.clear();
        }
    }

    /// Process a chunk of visible terminal output. Returns the portion of
    /// text that should be forwarded to the terminal (i.e. everything
    /// **not** consumed by an active capture).
    ///
    /// - **WaitingForStart**: all text is suppressed (command echo).
    /// - **Capturing**: text is buffered for the AI result.
    /// - **PostCapture**: text is suppressed (shell prompt after command).
    /// - When the END marker is found, captured output is sent through
    ///   the `oneshot` channel automatically.
    pub fn process(&mut self, text: &str) -> String {
        if self.active.is_empty() {
            return text.to_string();
        }

        let combined;
        let mut remaining = if self.pending_marker_tail.is_empty() {
            text
        } else {
            combined = format!("{}{}", self.pending_marker_tail, text);
            self.pending_marker_tail.clear();
            combined.as_str()
        };
        let mut passthrough = String::with_capacity(text.len());

        while !remaining.is_empty() {
            if let Some(result) = self.try_match_start(remaining) {
                remaining = result.after;
                continue;
            }

            if let Some(result) = self.try_match_end(remaining) {
                passthrough.push_str(result.before);
                remaining = result.after;
                continue;
            }

            if let Some(capture_id) = self.any_in_phase(CapturePhase::Capturing) {
                if let Some(pos) = remaining.find(MARKER_PREFIX) {
                    if let Some(cap) = self.active.get_mut(&capture_id) {
                        append_capture_output(cap, &remaining[..pos]);
                    }
                    let candidate = &remaining[pos..];
                    if self.is_possible_marker_prefix(candidate) {
                        self.pending_marker_tail.push_str(candidate);
                        remaining = "";
                    } else if pos == 0 {
                        if let Some(cap) = self.active.get_mut(&capture_id) {
                            append_capture_output(cap, MARKER_PREFIX);
                        }
                        remaining = &remaining[MARKER_PREFIX.len()..];
                    } else {
                        remaining = &remaining[pos..];
                    }
                } else {
                    if let Some(cap) = self.active.get_mut(&capture_id) {
                        append_capture_output(cap, remaining);
                    }
                    remaining = "";
                }
            } else if let Some(capture_id) = self.any_in_phase(CapturePhase::PostCapture) {
                // Suppress the shell prompt that appears after the command.
                // Remove the capture so the next chunk passes through normally.
                self.active.remove(&capture_id);
                if self.active.is_empty() {
                    self.pending_marker_tail.clear();
                }
                remaining = "";
            } else if self.any_in_phase(CapturePhase::WaitingForStart).is_some() {
                // Suppress everything — this is the echoed command text.
                // try_match_start above handles START marker detection.
                if let Some(tail_start) = self.possible_marker_tail_start(remaining) {
                    self.pending_marker_tail.push_str(&remaining[tail_start..]);
                }
                remaining = "";
            } else if let Some(pos) = remaining.find(MARKER_PREFIX) {
                passthrough.push_str(&remaining[..pos]);
                if pos == 0 {
                    passthrough.push_str(MARKER_PREFIX);
                    remaining = &remaining[MARKER_PREFIX.len()..];
                } else {
                    remaining = &remaining[pos..];
                }
            } else {
                passthrough.push_str(remaining);
                remaining = "";
            }
        }

        passthrough
    }

    fn any_in_phase(&self, target: CapturePhase) -> Option<String> {
        self.active
            .iter()
            .find(|(_, cap)| cap.phase == target)
            .map(|(id, _)| id.clone())
    }

    fn try_match_start<'a>(&mut self, text: &'a str) -> Option<MatchResult<'a>> {
        let prefix = format!("{MARKER_PREFIX}START_");
        let start_pos = text.find(&prefix)?;

        let after_prefix = &text[start_pos + prefix.len()..];
        let end_suffix = "__";
        let suffix_pos = after_prefix.find(end_suffix)?;

        let marker_id = &after_prefix[..suffix_pos];

        if !self.active.contains_key(marker_id) {
            return None;
        }

        if let Some(cap) = self.active.get_mut(marker_id) {
            cap.phase = CapturePhase::Capturing;
        }

        let marker_end = start_pos + prefix.len() + suffix_pos + end_suffix.len();
        let after_marker = &text[marker_end..];
        let after = after_marker
            .strip_prefix("\r\n")
            .or_else(|| after_marker.strip_prefix('\n'))
            .unwrap_or(after_marker);

        Some(MatchResult { before: "", after })
    }

    fn try_match_end<'a>(&mut self, text: &'a str) -> Option<MatchResult<'a>> {
        let prefix = format!("{MARKER_PREFIX}END_");
        let start_pos = text.find(&prefix)?;

        let after_prefix = &text[start_pos + prefix.len()..];
        let end_suffix = "__";
        let suffix_pos = after_prefix.find(end_suffix)?;

        let inner = &after_prefix[..suffix_pos];

        let last_underscore = inner.rfind('_')?;
        let marker_id = &inner[..last_underscore];
        let code_str = &inner[last_underscore + 1..];
        let exit_code = code_str.parse::<i32>().ok();

        let capture = self.active.get_mut(marker_id)?;

        let before = &text[..start_pos];
        let marker_end = start_pos + prefix.len() + suffix_pos + end_suffix.len();
        let after_marker = &text[marker_end..];
        let _ = after_marker;

        let mut output = std::mem::take(&mut capture.buffer);
        output.push_str(before);
        let output = output.trim().to_string();

        if let Some(tx) = capture.result_tx.take() {
            let _ = tx.send(CapturedOutput {
                output,
                exit_code,
                duration_ms: capture.start_time.elapsed().as_millis() as u64,
                source_truncated: capture.source_truncated,
            });
        }

        // Transition to PostCapture to suppress the shell prompt that follows.
        // Also discard any text after the END marker in this chunk.
        capture.phase = CapturePhase::PostCapture;

        Some(MatchResult {
            before: "",
            after: "",
        })
    }

    fn possible_marker_tail_start(&self, text: &str) -> Option<usize> {
        text.char_indices()
            .filter_map(|(idx, _)| self.is_possible_marker_prefix(&text[idx..]).then_some(idx))
            .min_by_key(|idx| *idx)
    }

    fn is_possible_marker_prefix(&self, value: &str) -> bool {
        if value.is_empty() {
            return false;
        }
        if MARKER_PREFIX.starts_with(value) {
            return true;
        }

        self.active.keys().any(|marker_id| {
            let start_marker = format!("{MARKER_PREFIX}START_{marker_id}__");
            if start_marker.starts_with(value) {
                return true;
            }

            let end_prefix = format!("{MARKER_PREFIX}END_{marker_id}_");
            if end_prefix.starts_with(value) {
                return true;
            }
            value.starts_with(&end_prefix)
                && value[end_prefix.len()..]
                    .chars()
                    .all(|ch| ch.is_ascii_digit() || ch == '-')
        })
    }
}

fn append_capture_output(capture: &mut ActiveCapture, text: &str) {
    let available = MAX_CAPTURE_BYTES.saturating_sub(capture.buffer.len());
    if text.len() <= available {
        capture.buffer.push_str(text);
        return;
    }
    let mut end = available.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    capture.buffer.push_str(&text[..end]);
    capture.source_truncated = true;
}

struct MatchResult<'a> {
    before: &'a str,
    after: &'a str,
}

impl Default for OutputCaptureProcessor {
    fn default() -> Self {
        Self::new()
    }
}
