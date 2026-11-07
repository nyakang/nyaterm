use std::collections::{HashMap, VecDeque};
use std::time::Instant;

pub const MAX_CAPTURE_BYTES: usize = 8 * 1024 * 1024;

const MARKER_PREFIX: &str = "__DF_CMD_";

/// After observation stops, let ordinary output through while removing only this
/// request's late markers. This never retains an unbounded server-controlled tail.
pub struct RetiredCaptureFilter {
    marker_id: String,
    exit_code: Option<i32>,
    start: String,
    end: String,
    pending: String,
    control_filter: CaptureControlFilter,
    finished: bool,
    observer: Option<AgentOutputCaptureProcessor>,
    observed_output: String,
}

impl RetiredCaptureFilter {
    pub fn new(id: &str) -> Self {
        Self {
            marker_id: id.into(),
            exit_code: None,
            start: format!("{MARKER_PREFIX}START_{id}__"),
            end: format!("{MARKER_PREFIX}END_{id}_"),
            pending: String::new(),
            control_filter: CaptureControlFilter::default(),
            finished: false,
            observer: None,
            observed_output: String::new(),
        }
    }

    pub fn disable_observation(&mut self) {
        self.observer = None;
        self.observed_output.clear();
    }
    pub fn marker_id(&self) -> &str {
        &self.marker_id
    }
    pub fn take_observed_output(&mut self) -> String {
        std::mem::take(&mut self.observed_output)
    }
    pub fn completed_marker(&self) -> Option<(&str, i32)> {
        self.exit_code.map(|code| (self.marker_id.as_str(), code))
    }

    pub fn process(&mut self, text: &str) -> (String, bool) {
        if let Some(observer) = self.observer.as_mut() {
            self.observed_output
                .push_str(&observer.process(text).recording_text);
        }
        let mut output = String::with_capacity(text.len());
        let filtered = self.control_filter.process(text);
        for ch in filtered.text.chars() {
            self.pending.push(ch);
            while !self.pending.is_empty() {
                if self.pending == self.start {
                    self.pending.clear();
                    break;
                }
                if self.start.starts_with(&self.pending) || self.end.starts_with(&self.pending) {
                    break;
                }
                if let Some(code) = self.pending.strip_prefix(&self.end) {
                    if let Some(code) = code
                        .strip_suffix("__")
                        .and_then(|code| code.parse::<i32>().ok())
                    {
                        self.exit_code = Some(code);
                        self.pending.clear();
                        self.finished = true;
                        break;
                    }
                    let partial = code.strip_suffix('_').unwrap_or(code);
                    if code.len() <= 12
                        && partial.chars().all(|ch| ch.is_ascii_digit() || ch == '-')
                    {
                        break;
                    }
                }
                let ch = self.pending.remove(0);
                output.push(ch);
            }
        }
        (output, self.finished && !self.control_filter.has_pending())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCapturedOutput {
    pub marker_id: String,
    pub output: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub source_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCaptureProcessResult {
    pub visible_text: String,
    /// Command output before the bounded RPC result is truncated. Never includes
    /// wrapper echoes, markers, or the post-command prompt.
    pub recording_text: String,
    pub completed: Vec<AgentCapturedOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCaptureCancelResult {
    pub marker_id: String,
    pub output: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub source_truncated: bool,
    pub recording_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapturePhase {
    WaitingForStart,
    Capturing,
    PostCapture,
}

#[derive(Debug)]
struct ActiveCapture {
    buffer: String,
    retain_output: bool,
    phase: CapturePhase,
    start_time: Instant,
    source_truncated: bool,
    start_newline: bool,
    recording_tail: String,
    end_newline: bool,
}

/// Control strings are opaque to marker matching. CMD titles can contain copies
/// of the wrapper's markers, including an END marker with a numeric exit status.
#[derive(Debug, Default, Clone)]
struct CaptureControlFilter {
    pending: String,
    // OSC accepts BEL as well as ST; the other control strings accept only ST.
    in_string: Option<bool>,
    string_escape: bool,
}

struct CaptureControlOutput {
    text: String,
    queries: String,
}

impl CaptureControlFilter {
    fn has_pending(&self) -> bool {
        self.in_string.is_some() || !self.pending.is_empty()
    }

    fn process(&mut self, text: &str) -> CaptureControlOutput {
        let mut queries = String::new();
        let mut output = String::with_capacity(text.len());
        for ch in text.chars() {
            if let Some(osc) = self.in_string {
                if (osc && ch == '\x07') || ch == '\u{009c}' || (self.string_escape && ch == '\\') {
                    self.in_string = None;
                }
                self.string_escape = ch == '\x1b';
                continue;
            }
            if ch == '\x1b' {
                self.pending = ch.into();
                continue;
            }
            if self.pending == "\x1b" {
                match ch {
                    '[' => self.pending.push(ch),
                    ']' | 'P' | 'X' | '^' | '_' => {
                        self.pending.clear();
                        self.in_string = Some(ch == ']');
                        self.string_escape = false;
                    }
                    'Z' => {
                        queries.push_str("\x1bZ");
                        output.push_str("\x1bZ");
                        self.pending.clear();
                    }
                    _ => {
                        output.push_str(&self.pending);
                        output.push(ch);
                        self.pending.clear();
                    }
                }
            } else if self.pending.starts_with("\x1b[") {
                self.pending.push(ch);
                if ('\x40'..='\x7e').contains(&ch) {
                    let body = &self.pending[2..];
                    if matches!(
                        body,
                        "5n" | "6n"
                            | "?6n"
                            | "c"
                            | "0c"
                            | ">c"
                            | ">0c"
                            | "14t"
                            | "16t"
                            | "18t"
                            | "19t"
                    ) {
                        queries.push_str(&self.pending);
                    }
                    output.push_str(&self.pending);
                    self.pending.clear();
                } else if self.pending.len() > 64 {
                    self.pending.clear();
                }
            } else {
                match ch {
                    '\u{009b}' => self.pending.push_str("\x1b["),
                    '\u{009d}' | '\u{0090}' | '\u{0098}' | '\u{009e}' | '\u{009f}' => {
                        self.in_string = Some(ch == '\u{009d}');
                        self.string_escape = false;
                    }
                    _ => output.push(ch),
                }
            }
        }
        CaptureControlOutput {
            text: output,
            queries,
        }
    }
}

#[derive(Debug, Default)]
pub struct AgentOutputCaptureProcessor {
    active: HashMap<String, ActiveCapture>,
    pending_marker_tail: String,
    completed: VecDeque<AgentCapturedOutput>,
    control_filter: CaptureControlFilter,
}

impl AgentOutputCaptureProcessor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, marker_id: String) {
        self.register_with_end_newline(marker_id, true);
    }

    /// POSIX and PowerShell insert a newline before END; CMD only echoes END.
    pub fn register_with_end_newline(&mut self, marker_id: String, end_newline: bool) {
        self.active.insert(
            marker_id,
            ActiveCapture {
                buffer: String::new(),
                retain_output: true,
                phase: CapturePhase::WaitingForStart,
                start_time: Instant::now(),
                source_truncated: false,
                start_newline: true,
                recording_tail: String::new(),
                end_newline,
            },
        );
    }

    pub fn register_streaming(&mut self, id: String, end_newline: bool) {
        self.register_with_end_newline(id.clone(), end_newline);
        self.active
            .get_mut(&id)
            .expect("registered capture")
            .retain_output = false;
    }

    pub fn marker_started(&self, id: &str) -> bool {
        self.active
            .get(id)
            .is_some_and(|c| c.phase != CapturePhase::WaitingForStart)
    }

    pub fn has_active(&self) -> bool {
        !self.active.is_empty() || self.has_pending_control()
    }

    pub fn has_pending_control(&self) -> bool {
        self.control_filter.has_pending()
    }

    /// A new execution boundary may arrive in the middle of the previous title.
    pub fn continue_control_sequences_from(&mut self, previous: Self) {
        self.control_filter = previous.control_filter;
    }

    /// Keep partial markers and control strings opaque after observation stops.
    pub fn retire(&mut self, marker_id: &str) -> RetiredCaptureFilter {
        let mut filter = RetiredCaptureFilter::new(marker_id);
        if let Some(active) = self.active.get(marker_id) {
            let mut observer = AgentOutputCaptureProcessor::new();
            observer.active.insert(
                marker_id.into(),
                ActiveCapture {
                    buffer: String::new(),
                    retain_output: false,
                    phase: active.phase,
                    start_time: active.start_time,
                    source_truncated: active.source_truncated,
                    start_newline: active.start_newline,
                    recording_tail: String::new(),
                    end_newline: active.end_newline,
                },
            );
            observer.pending_marker_tail = self.pending_marker_tail.clone();
            observer.control_filter = self.control_filter.clone();
            filter.observer = Some(observer);
        }
        let observer = filter.observer.take();
        filter.process(&std::mem::take(&mut self.pending_marker_tail));
        filter.observer = observer;
        filter.control_filter = std::mem::take(&mut self.control_filter);
        filter
    }

    pub fn cancel(&mut self, marker_id: &str) -> Option<AgentCaptureCancelResult> {
        let capture = self.active.remove(marker_id)?;
        if self.active.is_empty() {
            self.pending_marker_tail.clear();
        }
        Some(AgentCaptureCancelResult {
            marker_id: marker_id.to_string(),
            output: capture.buffer,
            exit_code: None,
            duration_ms: capture.start_time.elapsed().as_millis() as u64,
            source_truncated: capture.source_truncated,
            recording_text: capture.recording_tail,
        })
    }

    pub fn process(&mut self, text: &str) -> AgentCaptureProcessResult {
        if !self.has_active() {
            return AgentCaptureProcessResult {
                visible_text: text.to_string(),
                recording_text: String::new(),
                completed: self.drain_completed(),
            };
        }

        let filtered = self.control_filter.process(text);
        if self.active.is_empty() {
            return AgentCaptureProcessResult {
                visible_text: filtered.text,
                recording_text: String::new(),
                completed: self.drain_completed(),
            };
        }
        let text = filtered.text.as_str();

        let combined;
        let mut remaining = if self.pending_marker_tail.is_empty() {
            text
        } else {
            combined = format!("{}{}", self.pending_marker_tail, text);
            self.pending_marker_tail.clear();
            combined.as_str()
        };
        // Forward only terminal queries during capture so shell startup can finish.
        let mut visible_text = filtered.queries;
        let mut recording_text = String::new();

        while !remaining.is_empty() {
            if let Some(result) = self.try_match_start(remaining) {
                remaining = result.after;
                continue;
            }

            if let Some(result) = self.try_match_end(remaining, &mut recording_text) {
                visible_text.push_str(result.before);
                remaining = result.after;
                continue;
            }

            if let Some(capture_id) = self.any_in_phase(CapturePhase::Capturing) {
                if let Some(pos) = remaining.find(MARKER_PREFIX) {
                    if let Some(cap) = self.active.get_mut(&capture_id) {
                        append_capture_output(cap, &remaining[..pos], &mut recording_text);
                    }
                    let candidate = &remaining[pos..];
                    if self.is_possible_marker_prefix(candidate) {
                        self.pending_marker_tail.push_str(candidate);
                        remaining = "";
                    } else if pos == 0 {
                        if let Some(cap) = self.active.get_mut(&capture_id) {
                            append_capture_output(cap, MARKER_PREFIX, &mut recording_text);
                        }
                        remaining = &remaining[MARKER_PREFIX.len()..];
                    } else {
                        remaining = &remaining[pos..];
                    }
                } else {
                    let tail_start = self
                        .possible_marker_tail_start(remaining)
                        .unwrap_or(remaining.len());
                    if let Some(cap) = self.active.get_mut(&capture_id) {
                        append_capture_output(cap, &remaining[..tail_start], &mut recording_text);
                    }
                    self.pending_marker_tail.push_str(&remaining[tail_start..]);
                    remaining = "";
                }
            } else if let Some(capture_id) = self.any_in_phase(CapturePhase::PostCapture) {
                self.active.remove(&capture_id);
                if self.active.is_empty() {
                    self.pending_marker_tail.clear();
                }
                remaining = "";
            } else if self.any_in_phase(CapturePhase::WaitingForStart).is_some() {
                if let Some(tail_start) = self.possible_marker_tail_start(remaining) {
                    self.pending_marker_tail.push_str(&remaining[tail_start..]);
                }
                remaining = "";
            } else if let Some(pos) = remaining.find(MARKER_PREFIX) {
                visible_text.push_str(&remaining[..pos]);
                if pos == 0 {
                    visible_text.push_str(MARKER_PREFIX);
                    remaining = &remaining[MARKER_PREFIX.len()..];
                } else {
                    remaining = &remaining[pos..];
                }
            } else {
                visible_text.push_str(remaining);
                remaining = "";
            }
        }

        AgentCaptureProcessResult {
            visible_text,
            recording_text,
            completed: self.drain_completed(),
        }
    }

    fn drain_completed(&mut self) -> Vec<AgentCapturedOutput> {
        self.completed.drain(..).collect()
    }

    fn any_in_phase(&self, target: CapturePhase) -> Option<String> {
        self.active
            .iter()
            .find(|(_, cap)| cap.phase == target)
            .map(|(id, _)| id.clone())
    }

    fn try_match_start<'a>(&mut self, text: &'a str) -> Option<MatchResult<'a>> {
        let (marker_id, marker_end) = self
            .active
            .iter()
            .filter(|(_, capture)| capture.phase == CapturePhase::WaitingForStart)
            .filter_map(|(id, _)| {
                let marker = format!("{MARKER_PREFIX}START_{id}__");
                text.find(&marker)
                    .map(|start| (id.clone(), start + marker.len()))
            })
            .min_by_key(|(_, end)| *end)?;
        self.active.get_mut(&marker_id)?.phase = CapturePhase::Capturing;
        Some(MatchResult {
            before: "",
            after: &text[marker_end..],
        })
    }

    fn try_match_end<'a>(
        &mut self,
        text: &'a str,
        recording_text: &mut String,
    ) -> Option<MatchResult<'a>> {
        let (marker_id, start, marker_end, exit_code) = self
            .active
            .iter()
            .filter(|(_, capture)| capture.phase == CapturePhase::Capturing)
            .flat_map(|(id, _)| {
                let prefix = format!("{MARKER_PREFIX}END_{id}_");
                text.match_indices(&prefix)
                    .filter_map(move |(start, matched)| {
                        let after = start + matched.len();
                        let suffix = text[after..].find("__")?;
                        let code = text[after..after + suffix].parse::<i32>().ok()?;
                        Some((id.clone(), start, after + suffix + 2, code))
                    })
                    .collect::<Vec<_>>()
            })
            .min_by_key(|(_, start, _, _)| *start)?;
        let capture = self.active.get_mut(&marker_id)?;
        append_capture_output(capture, &text[..start], recording_text);
        // Delay at most one newline so the wrapper's separator is excluded,
        // even when it and the END marker arrive in different chunks.
        let tail = std::mem::take(&mut capture.recording_tail);
        if !capture.end_newline {
            recording_text.push_str(&tail);
        }
        self.completed.push_back(AgentCapturedOutput {
            marker_id: marker_id.clone(),
            output: std::mem::take(&mut capture.buffer).trim().to_string(),
            exit_code: Some(exit_code),
            duration_ms: capture.start_time.elapsed().as_millis() as u64,
            source_truncated: capture.source_truncated,
        });
        capture.phase = CapturePhase::PostCapture;
        // The shell prompt may share this chunk, or arrive in the next chunk.
        if !text[marker_end..].trim().is_empty() {
            self.active.remove(&marker_id);
        }
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
                && value.len() <= end_prefix.len() + 12
                && value[end_prefix.len()..]
                    .strip_suffix('_')
                    .unwrap_or(&value[end_prefix.len()..])
                    .chars()
                    .all(|ch| ch.is_ascii_digit() || ch == '-')
        })
    }
}

fn append_capture_output(capture: &mut ActiveCapture, text: &str, recording_text: &mut String) {
    let text = if capture.start_newline {
        let text = text.trim_start_matches('\r');
        if text.is_empty() {
            return;
        }
        capture.start_newline = false;
        text.strip_prefix('\n').unwrap_or(text)
    } else {
        text
    };
    if !text.is_empty() {
        recording_text.push_str(&std::mem::take(&mut capture.recording_tail));
        recording_text.push_str(text);
        let tail_len = if recording_text.ends_with("\r\n") {
            2
        } else if recording_text.ends_with(['\r', '\n']) {
            1
        } else {
            0
        };
        capture.recording_tail = recording_text.split_off(recording_text.len() - tail_len);
    }
    if !capture.retain_output {
        return;
    }
    let available = MAX_CAPTURE_BYTES.saturating_sub(capture.buffer.len());
    let mut end = available.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    capture.buffer.push_str(&text[..end]);
    capture.source_truncated |= end < text.len();
}

struct MatchResult<'a> {
    before: &'a str,
    after: &'a str,
}

#[cfg(test)]
mod tests {
    use super::AgentOutputCaptureProcessor;
    use nyaterm_core::agent_capture::build_agent_capture_command;
    use nyaterm_core::models::connection::AiExecutionProfile;

    #[test]
    fn capture_forwards_only_terminal_queries_even_when_split_or_inside_control_strings() {
        let mut processor = AgentOutputCaptureProcessor::new();
        processor.register("query".into());
        assert!(processor.process("wrapper\x1b[").visible_text.is_empty());
        assert_eq!(
            processor
                .process("6n\x1b[2J__DF_CMD_START_query__\noutput")
                .visible_text,
            "\x1b[6n"
        );
        assert_eq!(
            processor
                .process("\x1b]title\x1b[6n\x07\x1b[0c")
                .visible_text,
            "\x1b[0c"
        );
        assert_eq!(
            processor
                .process("\n__DF_CMD_END_query_0__\nPS>")
                .completed
                .len(),
            1
        );
    }

    #[test]
    fn markers_inside_control_strings_cannot_start_or_finish_a_capture() {
        for (open, close) in [
            ("\x1b]0;", "\x07"),
            ("\x1b]0;", "\x1b\\"),
            ("\x1bP", "\x1b\\"),
            ("\x1bX", "\x1b\\"),
            ("\x1b^", "\x1b\\"),
            ("\x1b_", "\x1b\\"),
            ("\u{009d}0;", "\u{009c}"),
        ] {
            let stream = format!(
                "{open}__DF_CMD_START_title__\nwrong\n__DF_CMD_END_title_9__{close}\
                 __DF_CMD_START_title__\nexpected\n\
                 {open}__DF_CMD_END_title_8__{close}\
                 __DF_CMD_END_title_0__\nPS>"
            );
            for split in stream.char_indices().map(|(i, _)| i) {
                let mut processor = AgentOutputCaptureProcessor::new();
                processor.register("title".into());
                let mut completed = processor.process(&stream[..split]).completed;
                completed.extend(processor.process(&stream[split..]).completed);
                assert_eq!(completed.len(), 1, "{open:?}, split {split}");
                assert_eq!(completed[0].exit_code, Some(0), "{open:?}, split {split}");
                assert_eq!(completed[0].output, "expected", "{open:?}, split {split}");
            }
        }
    }

    #[test]
    fn a_split_cmd_title_after_completion_never_becomes_visible_text() {
        let title = "\x1b]0;cmd - call echo __DF_CMD_END_cmd_%ERRORLEVEL%__\x07";
        for split in 1..title.len() {
            let mut processor = AgentOutputCaptureProcessor::new();
            processor.register("cmd".into());
            let first = processor.process(&format!(
                "__DF_CMD_START_cmd__\r\nexpected\r\n__DF_CMD_END_cmd_0__\r\n{}",
                &title[..split]
            ));
            assert_eq!(first.completed.len(), 1, "split {split}");
            assert_eq!(first.completed[0].output, "expected", "split {split}");
            assert!(processor.has_active(), "unfinished title at split {split}");
            let tail = processor.process(&format!("{}\r\nC:\\>", &title[split..]));
            assert!(!tail.visible_text.contains("__DF_CMD_"), "split {split}");
            assert!(tail.completed.is_empty(), "split {split}");
            assert!(!processor.has_active(), "finished title at split {split}");
        }
    }

    #[test]
    fn late_marker_filter_preserves_unicode_and_ordinary_text_at_every_split() {
        use super::RetiredCaptureFilter;
        let stream = "猫\n__DF_CMD_START_old__\nlate output🐱\n__DF_CMD_END_old_-3__\nready>";
        for split in stream.char_indices().map(|(i, _)| i) {
            let mut filter = RetiredCaptureFilter::new("old");
            let (mut visible, _) = filter.process(&stream[..split]);
            let (tail, _) = filter.process(&stream[split..]);
            visible.push_str(&tail);
            assert_eq!(visible, "猫\n\nlate output🐱\n\nready>", "split {split}");
        }
    }

    #[test]
    fn cancellation_transfers_an_unfinished_title_to_the_late_marker_filter() {
        let mut processor = AgentOutputCaptureProcessor::new();
        processor.register("old".into());
        processor.process("__DF_CMD_START_old__\npartial\n\x1b]0;");
        let mut filter = processor.retire("old");
        processor.cancel("old").unwrap();
        let (visible, finished) = filter.process("__DF_CMD_END_old_0__\x07normal");
        assert_eq!(visible, "normal");
        assert!(!finished, "the title's marker cannot finish the filter");
        let (visible, finished) = filter.process("__DF_CMD_END_old_0__\x1b]0;");
        assert!(visible.is_empty());
        assert!(
            !finished,
            "the title must finish before dropping the filter"
        );
        let (visible, finished) = filter.process("__DF_CMD_END_old_0__\x07ready>");
        assert_eq!(visible, "ready>");
        assert!(finished);
    }

    #[test]
    fn every_utf8_chunk_boundary_preserves_output_and_exit_status() {
        let stream =
            "echo wrapper\r\n__DF_CMD_START_split__\r\n猫🐱\r\n__DF_CMD_END_split_-7__\r\nPS>";
        for split in stream.char_indices().map(|(i, _)| i) {
            let mut processor = AgentOutputCaptureProcessor::new();
            processor.register("split".into());
            let mut completed = processor.process(&stream[..split]).completed;
            completed.extend(processor.process(&stream[split..]).completed);
            assert_eq!(completed.len(), 1, "split {split}");
            assert_eq!(completed[0].output, "猫🐱", "split {split}");
            assert_eq!(completed[0].exit_code, Some(-7));
        }
    }

    #[test]
    fn recording_stream_preserves_whitespace_and_removes_split_wrapper_framing() {
        let stream =
            "wrapper\r\n__DF_CMD_START_log__\r\n  猫🐱  \r\n\r\n__DF_CMD_END_log_0__\r\nprompt>";
        for split in stream.char_indices().map(|(i, _)| i) {
            let mut processor = AgentOutputCaptureProcessor::new();
            processor.register("log".into());
            let first = processor.process(&stream[..split]);
            let second = processor.process(&stream[split..]);
            let recorded = first.recording_text + &second.recording_text;
            assert_eq!(recorded, "  猫🐱  \r\n", "split {split}");
        }
    }

    #[test]
    fn cancellation_flushes_only_the_unrecorded_tail_and_late_markers_stay_hidden() {
        let mut processor = AgentOutputCaptureProcessor::new();
        processor.register("log".into());
        let first = processor.process("__DF_CMD_START_log__\npartial\n__DF_CMD_EN");
        let mut retired = processor.retire("log");
        let cancelled = processor.cancel("log").unwrap();
        assert_eq!(
            first.recording_text + &cancelled.recording_text,
            "partial\n"
        );
        let (late, done) = retired.process("D_log_0__\nlate output\n");
        assert!(done);
        assert_eq!(late, "\nlate output\n");
    }

    #[test]
    fn cmd_recording_preserves_empty_output_lines_without_an_end_separator() {
        let mut processor = AgentOutputCaptureProcessor::new();
        processor.register_with_end_newline("cmd".into(), false);
        let first = processor.process("__DF_CMD_START_cmd__\r\n\r");
        let second = processor.process("\n__DF_CMD_END_cmd_0__\r\nC:\\>");
        assert_eq!(first.recording_text + &second.recording_text, "\r\n");
    }

    #[test]
    fn unrelated_and_malformed_markers_do_not_finish_or_restart_capture() {
        let mut processor = AgentOutputCaptureProcessor::new();
        processor.register("live".into());
        let result = processor.process("__DF_CMD_START_other__\n__DF_CMD_START_live__\none\n__DF_CMD_START_live__\n__DF_CMD_END_live_invalid__\n__DF_CMD_END_other_0__\ntwo\n__DF_CMD_END_live_3__\nPS>");
        assert_eq!(result.completed.len(), 1);
        assert!(result.completed[0].output.starts_with("one"));
        assert!(result.completed[0].output.ends_with("two"));
        assert_eq!(result.completed[0].exit_code, Some(3));
        assert_eq!(
            processor.process("normal output").visible_text,
            "normal output"
        );
    }

    #[test]
    fn capture_limit_keeps_scanning_and_does_not_split_utf8() {
        use super::MAX_CAPTURE_BYTES;
        let mut processor = AgentOutputCaptureProcessor::new();
        processor.register("limit".into());
        processor.process("__DF_CMD_START_limit__\n");
        let body = "猫".repeat(MAX_CAPTURE_BYTES / 3 + 5);
        let streamed = processor.process(&body);
        assert_eq!(streamed.recording_text, body);
        let result = processor.process("\n__DF_CMD_END_limit_0__\nPS>");
        let output = &result.completed[0];
        assert!(output.source_truncated);
        assert!(output.output.len() <= MAX_CAPTURE_BYTES);
        assert_eq!(output.exit_code, Some(0));
        assert_eq!(processor.process("next").visible_text, "next");
    }

    #[cfg(windows)]
    #[test]
    fn powershell_wrapper_executes_in_the_current_shell_scope() {
        let wrapped = build_agent_capture_command(
            AiExecutionProfile::Powershell,
            "windows",
            "Write-Output $nyatermTestValue",
        )
        .unwrap();
        let script = format!("$nyatermTestValue = 'current-shell'; {wrapped}");
        let output = std::process::Command::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &script,
            ])
            .output()
            .expect("Windows PowerShell");
        assert!(output.status.success());
        let mut processor = AgentOutputCaptureProcessor::new();
        processor.register("windows".into());
        let result = processor.process(&String::from_utf8_lossy(&output.stdout));
        assert_eq!(result.completed[0].output, "current-shell");
        assert_eq!(result.completed[0].exit_code, Some(0));
    }

    #[cfg(windows)]
    #[test]
    fn cmd_wrapper_captures_output_and_exit_status() {
        let wrapped =
            build_agent_capture_command(AiExecutionProfile::Cmd, "windows", "echo current-shell")
                .unwrap();
        let output = std::process::Command::new("cmd.exe")
            .args(["/D", "/C", &wrapped])
            .output()
            .expect("Windows CMD");
        assert!(output.status.success());
        let mut processor = AgentOutputCaptureProcessor::new();
        processor.register("windows".into());
        let result = processor.process(&String::from_utf8_lossy(&output.stdout));
        assert_eq!(result.completed[0].output, "current-shell");
        assert_eq!(result.completed[0].exit_code, Some(0));
    }

    #[test]
    fn builders_do_not_embed_matchable_markers_in_input_text() {
        for profile in [
            AiExecutionProfile::Posix,
            AiExecutionProfile::Powershell,
            AiExecutionProfile::Cmd,
        ] {
            let command = build_agent_capture_command(profile, "marker-1", "echo ok").unwrap();
            assert!(!command.contains("__DF_CMD_START_marker-1__"));
            assert!(!command.contains("__DF_CMD_END_marker-1_0__"));
        }
    }

    #[test]
    fn powershell_builder_is_single_logical_input_line() {
        let command = build_agent_capture_command(
            AiExecutionProfile::Powershell,
            "marker-1",
            "Write-Output 'ok'\r\n# comment",
        )
        .unwrap();
        let command = command.strip_suffix("\r\n").unwrap();

        assert!(!command.contains('\r'));
        assert!(!command.contains('\n'));
        assert!(command.contains("[scriptblock]::Create($nyaiScript)"));
        assert!(!command.contains("Write-Output 'ok'"));
    }

    #[test]
    fn cmd_builder_is_single_logical_input_line() {
        let command = build_agent_capture_command(
            AiExecutionProfile::Cmd,
            "marker-1",
            "echo one\r\necho two",
        )
        .unwrap();
        let command = command.strip_suffix("\r\n").unwrap();

        assert!(!command.contains('\r'));
        assert!(!command.contains('\n'));
        assert!(command.contains("echo one & echo two"));
        assert!(command.contains("call echo"));
        assert!(command.contains("^%ERRORLEVEL^%"));
    }

    #[test]
    fn unsupported_profiles_do_not_build_capture_commands() {
        for profile in [
            AiExecutionProfile::Auto,
            AiExecutionProfile::SendOnly,
            AiExecutionProfile::Disabled,
        ] {
            assert!(build_agent_capture_command(profile, "marker-1", "echo ok").is_none());
        }
    }

    #[test]
    fn captures_crlf_output_with_prompt_before_markers() {
        let mut processor = AgentOutputCaptureProcessor::new();
        processor.register("m1".to_string());

        let result = processor.process(
            "C:\\>echo marker\r\n__DF_CMD_START_m1__\r\nok\r\n__DF_CMD_END_m1_7__\r\nC:\\>",
        );

        assert!(result.visible_text.is_empty());
        assert_eq!(result.completed.len(), 1);
        assert_eq!(result.completed[0].output, "ok");
        assert_eq!(result.completed[0].exit_code, Some(7));
    }

    #[test]
    fn captures_start_marker_split_across_chunks() {
        let mut processor = AgentOutputCaptureProcessor::new();
        processor.register("m2".to_string());

        assert!(processor.process("__DF_CMD_STA").visible_text.is_empty());
        assert!(
            processor
                .process("RT_m2__\nhello\n")
                .visible_text
                .is_empty()
        );
        let result = processor.process("__DF_CMD_END_m2_0__\n");

        assert!(result.visible_text.is_empty());
        assert_eq!(result.completed.len(), 1);
        assert_eq!(result.completed[0].output, "hello");
        assert_eq!(result.completed[0].exit_code, Some(0));
    }

    #[test]
    fn captures_end_marker_split_across_chunks() {
        let mut processor = AgentOutputCaptureProcessor::new();
        processor.register("m3".to_string());

        assert!(
            processor
                .process("__DF_CMD_START_m3__\nhello\n__DF_CMD_EN")
                .visible_text
                .is_empty()
        );
        let result = processor.process("D_m3_9__\n");

        assert!(result.visible_text.is_empty());
        assert_eq!(result.completed.len(), 1);
        assert_eq!(result.completed[0].output, "hello");
        assert_eq!(result.completed[0].exit_code, Some(9));
    }

    #[test]
    fn cancel_clears_pending_marker_tail() {
        let mut processor = AgentOutputCaptureProcessor::new();
        processor.register("m4".to_string());

        assert!(processor.process("__DF_CMD_STA").visible_text.is_empty());
        assert!(!processor.pending_marker_tail.is_empty());

        let cancelled = processor.cancel("m4").unwrap();
        assert_eq!(cancelled.marker_id, "m4");
        assert!(!processor.has_active());
        assert!(processor.pending_marker_tail.is_empty());
        let result = processor.process("plain output");

        assert_eq!(result.visible_text, "plain output");
        assert!(result.completed.is_empty());
    }
}
