#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapturePhase {
    /// Just registered — suppress all output (the echoed command text)
    /// until the real START marker appears in execution output.
    WaitingForStart,
    /// Between START and END markers — buffer output for the AI.
    Capturing,
    /// The caller stopped waiting, but the shell may still be running.
    Abandoned,
}

/// Tracks one in-flight capture request.
struct ActiveCapture {
    buffer: String,
    phase: CapturePhase,
    start_time: Instant,
    result_tx: Option<oneshot::Sender<CapturedOutput>>,
    source_truncated: bool,
    execution: Option<crate::core::session::TerminalExecutionGuard>,
}

/// Shared processor that all IO loops (SSH, PTY, Telnet, Serial) can use to
/// intercept marker sequences in the output stream.
pub struct OutputCaptureProcessor {
    active: HashMap<String, ActiveCapture>,
    pending_marker_tail: String,
    pending_query: String,
}

impl OutputCaptureProcessor {
    pub fn new() -> Self {
        Self {
            active: HashMap::new(),
            pending_marker_tail: String::new(),
            pending_query: String::new(),
        }
    }

    /// Register a new capture. The caller should then write the
    /// `build_capture_command()` output into the PTY.
    ///
    /// From this point, all output is suppressed until the START marker
    /// appears (hiding the echoed command text).
    #[cfg(test)]
    pub fn register(&mut self, marker_id: String, result_tx: oneshot::Sender<CapturedOutput>) {
        self.register_execution(marker_id, result_tx, None);
    }

    pub fn register_execution(
        &mut self,
        marker_id: String,
        result_tx: oneshot::Sender<CapturedOutput>,
        execution: Option<crate::core::session::TerminalExecutionGuard>,
    ) -> bool {
        if self.has_active() {
            return false;
        }
        self.pending_marker_tail.clear();
        self.pending_query.clear();
        self.active.insert(
            marker_id,
            ActiveCapture {
                buffer: String::new(),
                phase: CapturePhase::WaitingForStart,
                start_time: Instant::now(),
                result_tx: Some(result_tx),
                source_truncated: false,
                execution,
            },
        );
        true
    }

    /// Returns true when at least one capture is in progress.
    pub fn has_active(&self) -> bool {
        !self.active.is_empty()
    }

    /// Cancel a capture by marker id (e.g. on timeout from the caller side).
    #[allow(dead_code)]
    pub fn cancel(&mut self, marker_id: &str) {
        if let Some(capture) = self.active.get_mut(marker_id) {
            capture.result_tx.take();
            capture.buffer.clear();
            capture.phase = CapturePhase::Abandoned;
            if let Some(execution) = &capture.execution {
                execution.awaiting_end();
            }
        }
        self.pending_query.clear();
    }

    /// Only use when dispatch failed before execution, not on caller timeout.
    pub fn abort(&mut self, marker_id: &str) {
        self.active.remove(marker_id);
        self.pending_marker_tail.clear();
        self.pending_query.clear();
    }

    /// A trusted, session-bound prompt hook can prove that an interrupted shell
    /// has returned to its prompt even when Ctrl+C prevented an END marker.
    pub fn finish_abandoned_at_prompt(&mut self) {
        if self.any_in_phase(CapturePhase::Abandoned).is_some() {
            self.active.clear();
            self.pending_marker_tail.clear();
            self.pending_query.clear();
        }
    }

    /// Process a chunk of visible terminal output. Returns the portion of
    /// text that should be forwarded to the terminal (i.e. everything
    /// **not** consumed by an active capture).
    ///
    /// - **WaitingForStart**: all text is suppressed (command echo).
    /// - **Capturing**: text is buffered for the AI result.
    /// - **Abandoned**: normal output is forwarded while late markers are removed.
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
            if self.active.is_empty() {
                passthrough.push_str(remaining);
                break;
            }
            if let Some(result) = self.try_match_start(remaining) {
                if self.any_in_phase(CapturePhase::Abandoned).is_some() {
                    passthrough.push_str(&result.before);
                } else {
                    passthrough.push_str(&self.terminal_queries(&result.before));
                }
                remaining = result.after;
                continue;
            }

            if let Some(result) = self.try_match_end(remaining) {
                passthrough.push_str(&result.before);
                remaining = result.after;
                continue;
            }

            if let Some(capture_id) = self.any_in_phase(CapturePhase::Capturing) {
                if let Some(pos) = find_marker_start(remaining) {
                    passthrough.push_str(&self.terminal_queries(&remaining[..pos]));
                    if let Some(cap) = self.active.get_mut(&capture_id) {
                        append_capture_output(cap, &remaining[..pos]);
                    }
                    let candidate = &remaining[pos..];
                    if self.is_possible_marker_prefix(candidate) {
                        self.pending_marker_tail.push_str(candidate);
                        remaining = "";
                    } else if pos == 0 {
                        let end = remaining.chars().next().unwrap().len_utf8();
                        if let Some(cap) = self.active.get_mut(&capture_id) {
                            append_capture_output(cap, &remaining[..end]);
                        }
                        remaining = &remaining[end..];
                    } else {
                        remaining = &remaining[pos..];
                    }
                } else {
                    // A marker can be split anywhere, including inside __DF_CMD_.
                    let end = self
                        .possible_marker_tail_start(remaining)
                        .unwrap_or(remaining.len());
                    passthrough.push_str(&self.terminal_queries(&remaining[..end]));
                    if let Some(cap) = self.active.get_mut(&capture_id) {
                        append_capture_output(cap, &remaining[..end]);
                    }
                    self.pending_marker_tail.push_str(&remaining[end..]);
                    remaining = "";
                }
            } else if self.any_in_phase(CapturePhase::WaitingForStart).is_some() {
                // Suppress everything — this is the echoed command text.
                // try_match_start above handles START marker detection.
                let end = self
                    .possible_marker_tail_start(remaining)
                    .unwrap_or(remaining.len());
                passthrough.push_str(&self.terminal_queries(&remaining[..end]));
                self.pending_marker_tail.push_str(&remaining[end..]);
                remaining = "";
            } else if self.any_in_phase(CapturePhase::Abandoned).is_some() {
                let end = self
                    .possible_marker_tail_start(remaining)
                    .unwrap_or(remaining.len());
                passthrough.push_str(&remaining[..end]);
                self.pending_marker_tail.push_str(&remaining[end..]);
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
        let (start_pos, prefix) = find_boundary_marker(text, true, &self.active)?;

        let after_prefix = &text[start_pos + prefix.len()..];
        let end_suffix = "__";
        let suffix_pos = after_prefix.find(end_suffix)?;

        let marker_id = &after_prefix[..suffix_pos];

        if !self.active.get(marker_id).is_some_and(|cap| {
            matches!(
                cap.phase,
                CapturePhase::WaitingForStart | CapturePhase::Abandoned
            )
        }) {
            return None;
        }

        let marker_end = start_pos + prefix.len() + suffix_pos + end_suffix.len();
        if let Some(cap) = self.active.get_mut(marker_id) {
            if cap.phase == CapturePhase::WaitingForStart {
                cap.phase = CapturePhase::Capturing;
            }
        }

        let after_marker = &text[marker_end..];
        let after = after_marker
            .strip_prefix("\r\n")
            .or_else(|| after_marker.strip_prefix('\n'))
            .unwrap_or(after_marker);

        Some(MatchResult {
            before: text[..start_pos].to_string(),
            after,
        })
    }

    fn try_match_end<'a>(&mut self, text: &'a str) -> Option<MatchResult<'a>> {
        let (start_pos, prefix) = find_boundary_marker(text, false, &self.active)?;

        let after_prefix = &text[start_pos + prefix.len()..];
        let end_suffix = "__";
        let suffix_pos = after_prefix.find(end_suffix)?;

        let inner = &after_prefix[..suffix_pos];

        let last_underscore = inner.rfind('_')?;
        let marker_id = &inner[..last_underscore];
        let code_str = &inner[last_underscore + 1..];
        let exit_code = Some(code_str.parse::<i32>().ok()?);

        let capture = self.active.get(marker_id)?;
        if capture.phase == CapturePhase::WaitingForStart {
            return None;
        }
        let abandoned = capture.phase == CapturePhase::Abandoned;
        let marker_end = start_pos + prefix.len() + suffix_pos + end_suffix.len();
        let mut capture = self.active.remove(marker_id)?;

        let before = &text[..start_pos];
        let after_marker = &text[marker_end..];
        append_capture_output(&mut capture, before);
        let output = capture.buffer.trim().to_string();

        let captured = CapturedOutput {
            output,
            exit_code,
            duration_ms: capture.start_time.elapsed().as_millis() as u64,
            source_truncated: capture.source_truncated,
        };
        // Emit presentation before returning the real prompt to the I/O loop.
        if !abandoned {
            if let Some(execution) = &mut capture.execution {
                execution.complete(&captured);
            }
        }
        // Release ownership before waking the caller, so its next sequential
        // execution cannot race the processor's guard destructor.
        drop(capture.execution.take());
        if let Some(tx) = capture.result_tx.take() {
            let _ = tx.send(captured);
        }
        let visible_before = if abandoned {
            before.to_string()
        } else {
            self.terminal_queries(before)
        };
        self.pending_marker_tail.clear();
        self.pending_query.clear();

        Some(MatchResult {
            before: visible_before,
            after: after_marker,
        })
    }

    fn possible_marker_tail_start(&self, text: &str) -> Option<usize> {
        let max_tail = self
            .active
            .keys()
            .map(|id| id.len() + MARKER_PREFIX.len() + 20)
            .max()
            .unwrap_or(0);
        let mut start = text.len().saturating_sub(max_tail);
        while !text.is_char_boundary(start) {
            start += 1;
        }
        text[start..]
            .char_indices()
            .map(|(idx, ch)| (idx + start, ch))
            .filter_map(|(idx, _)| self.is_possible_marker_prefix(&text[idx..]).then_some(idx))
            .min_by_key(|idx| *idx)
    }

    fn is_possible_marker_prefix(&self, value: &str) -> bool {
        if value.is_empty() {
            return false;
        }
        [MARKER_PREFIX, POWERSHELL_MARKER_PREFIX]
            .iter()
            .any(|prefix| prefix.starts_with(value))
            || self.active.keys().any(|marker_id| {
                [
                    (MARKER_PREFIX, "START", "END"),
                    (POWERSHELL_MARKER_PREFIX, "S", "E"),
                ]
                .iter()
                .any(|(prefix, start, end)| {
                    let start_marker = format!("{prefix}{start}_{marker_id}__");
                    let end_prefix = format!("{prefix}{end}_{marker_id}_");
                    start_marker.starts_with(value)
                        || end_prefix.starts_with(value)
                        || (value.starts_with(&end_prefix)
                            && value[end_prefix.len()..].len() <= 12
                            && value[end_prefix.len()..]
                                .trim_end_matches('_')
                                .chars()
                                .enumerate()
                                .all(|(idx, ch)| ch.is_ascii_digit() || (idx == 0 && ch == '-'))
                            && !value.ends_with("__"))
                })
            })
    }

    /// PSReadLine/ConPTY can query the frontend cursor even while command echo
    /// is hidden. Swallowing the query hangs or corrupts its redraw state.
    fn terminal_queries(&mut self, text: &str) -> String {
        let mut visible = String::new();
        for ch in text.chars() {
            if ch == '\x1b' {
                self.pending_query.clear();
                self.pending_query.push(ch);
            } else if !self.pending_query.is_empty() {
                self.pending_query.push(ch);
                if self.pending_query.len() == 2 && ch != '[' {
                    self.pending_query.clear();
                } else if self.pending_query.len() > 2 && ('@'..='~').contains(&ch) {
                    if matches!(
                        self.pending_query.as_str(),
                        "\x1b[6n" | "\x1b[?6n" | "\x1b[c" | "\x1b[0c" | "\x1b[>c" | "\x1b[>0c"
                    ) {
                        visible.push_str(&self.pending_query);
                    }
                    self.pending_query.clear();
                } else if self.pending_query.len() > 32 {
                    self.pending_query.clear();
                }
            }
        }
        visible
    }
}

fn find_marker_start(text: &str) -> Option<usize> {
    [MARKER_PREFIX, POWERSHELL_MARKER_PREFIX]
        .iter()
        .filter_map(|prefix| text.find(prefix))
        .min()
}

fn find_boundary_marker(
    text: &str,
    start: bool,
    active: &HashMap<String, ActiveCapture>,
) -> Option<(usize, String)> {
    [
        (MARKER_PREFIX, if start { "START" } else { "END" }),
        (POWERSHELL_MARKER_PREFIX, if start { "S" } else { "E" }),
    ]
    .iter()
    .filter_map(|(prefix, boundary)| {
        let prefix = format!("{prefix}{boundary}_");
        text.match_indices(&prefix).find_map(|(pos, _)| {
            let after = &text[pos + prefix.len()..];
            let end = after.find("__")?;
            let inner = &after[..end];
            let id = if start {
                inner
            } else {
                let split = inner.rfind('_')?;
                inner[split + 1..].parse::<i32>().ok()?;
                &inner[..split]
            };
            let cap = active.get(id)?;
            let valid = if start {
                matches!(
                    cap.phase,
                    CapturePhase::WaitingForStart | CapturePhase::Abandoned
                )
            } else {
                cap.phase != CapturePhase::WaitingForStart
            };
            valid.then(|| (pos, prefix.clone()))
        })
    })
    .min_by_key(|(pos, _)| *pos)
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
    before: String,
    after: &'a str,
}

impl Default for OutputCaptureProcessor {
    fn default() -> Self {
        Self::new()
    }
}
