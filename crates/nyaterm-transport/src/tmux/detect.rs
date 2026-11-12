//! Bounded passive confirmation of tmux startup in a normal SSH stream.

use super::handshake::{CONFIRM_MAX_BYTES, CONFIRM_TIMEOUT, CONTROL_PREFIX, Startup};
use std::time::Duration;
use tokio::time::Instant;

/// Only an explicitly submitted single-C command permits bare control lines.
const SINGLE_C_WINDOW: Duration = Duration::from_secs(30);

/// Upper bound for the tracked input line; longer lines cannot be matched
/// reliably anyway, so tracking resets instead of growing without bound.
const MAX_INPUT_LINE_BYTES: usize = 4096;

/// Delay before advising that an explicit launch has not been confirmed.
/// Command echo and protocol-looking notifications do not count as a shell
/// response. The advisory never stops discovery of a delayed startup.
const SILENT_HINT_DELAY: Duration = Duration::from_secs(10);

/// Commands whose following tokens are still "command position": prefix
/// wrappers (`sudo nice tmux -CC`) and environment assignments
/// (`FOO=1 tmux -CC` — handled via the `=` token rule).
const COMMAND_WRAPPERS: &[&str] = &[
    "sudo", "doas", "pkexec", "command", "builtin", "nice", "stdbuf", "noglob", "time", "exec",
    "nohup", "env",
];

/// `ssh` flags that consume the next token as their argument, so the
/// destination host is found while skipping them (`ssh -p 22 host tmux -CC`).
const SSH_FLAGS_WITH_ARG: &[&str] = &[
    "-B", "-b", "-c", "-D", "-E", "-F", "-I", "-i", "-J", "-L", "-l", "-m", "-O", "-o", "-p", "-Q",
    "-R", "-S", "-W", "-w",
];

fn strip_quotes(token: &str) -> &str {
    token.trim_matches(|c| c == '"' || c == '\'')
}

fn is_shell_separator(token: &str) -> bool {
    !token.is_empty()
        && token
            .chars()
            .all(|c| matches!(c, ';' | '|' | '&' | '<' | '>' | '(' | ')' | '{' | '}'))
}

/// Split `line` into words and separator tokens. Separator characters count
/// even when glued to a word (`foo; tmux -CC`, `} ; tmux`), but quotes keep
/// them inside their word so `echo 'x; tmux -CC'` cannot arm the detector.
fn shell_tokens(line: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let bytes = line.as_bytes();
    let mut start = 0;
    let mut i = 0;
    let mut quote = 0u8;
    while i < bytes.len() {
        let byte = bytes[i];
        if quote != 0 {
            if byte == quote {
                quote = 0;
            }
        } else if byte == b'\'' || byte == b'"' {
            quote = byte;
        } else if byte.is_ascii_whitespace() {
            if start < i {
                tokens.push(&line[start..i]);
            }
            start = i + 1;
        } else if matches!(
            byte,
            b';' | b'|' | b'&' | b'<' | b'>' | b'(' | b')' | b'{' | b'}'
        ) {
            if start < i {
                tokens.push(&line[start..i]);
            }
            tokens.push(&line[i..i + 1]);
            start = i + 1;
        }
        i += 1;
    }
    if start < bytes.len() {
        tokens.push(&line[start..]);
    }
    tokens
}

fn is_tmux_binary(token: &str) -> bool {
    token == "tmux" || token.ends_with("/tmux")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlMode {
    SingleC,
    DoubleC,
}

impl ControlMode {
    fn prefix(self) -> &'static [u8] {
        match self {
            Self::SingleC => b"%begin ",
            Self::DoubleC => CONTROL_PREFIX,
        }
    }
}

/// A newline inside shell quoting (or after a backslash) does not submit a
/// new command. Retain the bounded input so its next line cannot gain stronger
/// evidence than the incomplete command it belongs to.
fn input_continues(line: &[u8]) -> bool {
    let mut quote = None;
    let mut escaped = false;
    for &byte in line {
        if escaped {
            escaped = false;
        } else if byte == b'\\' && quote != Some(b'\'') {
            escaped = true;
        } else if quote == Some(byte) {
            quote = None;
        } else if quote.is_none() && matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
        }
    }
    quote.is_some() || escaped
}

/// Whether `line` invokes tmux in control mode. Evaluated on the input side:
/// the command that spawned the protocol is visible before its output is.
///
/// Token-level analysis, not a full shell parser: it tracks command position
/// across `; | &` separators and past prefix wrappers (`sudo`, `env`,
/// `VAR=val`), understands `ssh host tmux -CC`, and matches `tmux`/`*/tmux`
/// followed by a `-C`/`-CC` flag. Misses (aliases, `sh -c 'tmux -CC'`,
/// indirect launches) rely on output-side DCS discovery instead.
fn tmux_control_command(line: &str) -> Option<ControlMode> {
    // Expansions, escapes, redirections and comments need a full shell parser.
    // Keep them eligible for passive discovery, never an explicit advisory.
    if line.contains(['\\', '$', '`', '<', '>', '#', '(', ')', '{', '}']) {
        return None;
    }
    let tokens = shell_tokens(line);
    if tokens.iter().any(|raw| {
        let token = strip_quotes(raw);
        token.contains(['\'', '"'])
            || (raw.starts_with('\'') && !raw.ends_with('\''))
            || (raw.starts_with('"') && !raw.ends_with('"'))
    }) {
        return None;
    }
    let mut mode = None;
    let mut expect_command = true;
    let mut expect_ssh_dest = false;
    let mut ssh_flag_arg = false;
    let mut tmux_seen = false;
    let mut option_arg = false;
    let mut wrapper: Option<&str> = None;

    for raw in tokens {
        let token = strip_quotes(raw);
        if token.is_empty() {
            return None;
        }
        if raw == token && is_shell_separator(token) {
            if mode.is_some() {
                return mode;
            }
            expect_command = true;
            expect_ssh_dest = false;
            ssh_flag_arg = false;
            tmux_seen = false;
            option_arg = false;
            wrapper = None;
            continue;
        }
        if option_arg {
            option_arg = false;
            continue;
        }
        if expect_ssh_dest {
            // Skip ssh options and their arguments; the first bare token is
            // the destination, and what follows it is the remote command.
            if ssh_flag_arg {
                ssh_flag_arg = false;
                continue;
            }
            if token.starts_with('-') {
                if matches!(token, "-N" | "-G" | "-V" | "-Q" | "-f") {
                    return None;
                }
                if SSH_FLAGS_WITH_ARG.contains(&token) {
                    ssh_flag_arg = true;
                } else if !matches!(
                    token,
                    "-t" | "-tt"
                        | "-T"
                        | "-4"
                        | "-6"
                        | "-A"
                        | "-a"
                        | "-C"
                        | "-v"
                        | "-vv"
                        | "-vvv"
                        | "-q"
                        | "-x"
                        | "-X"
                        | "-Y"
                ) {
                    return None;
                }
                continue;
            }
            expect_ssh_dest = false;
            expect_command = true;
            continue;
        }
        if expect_command {
            if token.starts_with('-') {
                match (wrapper, token) {
                    (_, "--") if wrapper.is_some() => {
                        wrapper = None;
                    }
                    (Some("sudo" | "doas"), "-u" | "-g")
                    | (Some("nice"), "-n")
                    | (Some("env"), "-u" | "-C")
                    | (Some("stdbuf"), "-i" | "-o" | "-e") => {
                        option_arg = true;
                    }
                    (Some("sudo"), "-k" | "-n" | "-E" | "-H") | (Some("env"), "-i") => {}
                    _ => return None,
                }
                continue;
            }
            if COMMAND_WRAPPERS.contains(&token) {
                wrapper = Some(token);
                continue;
            }
            if !raw.starts_with(['\'', '"'])
                && let Some((name, _)) = token.split_once('=')
                && !name.is_empty()
                && name.bytes().enumerate().all(|(i, b)| {
                    b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit())
                })
            {
                continue;
            }
            if token == "ssh" {
                expect_ssh_dest = true;
                continue;
            }
            if is_tmux_binary(token) {
                tmux_seen = true;
            }
            expect_command = false;
            continue;
        }
        if tmux_seen {
            if matches!(token, "-C" | "-CC") {
                mode = Some(if token == "-CC" || mode.is_some() {
                    ControlMode::DoubleC
                } else {
                    ControlMode::SingleC
                });
                continue;
            }
            match token {
                "-L" | "-S" | "-f" | "-T" | "-c" => {
                    option_arg = true;
                }
                "-2" | "-u" | "-v" | "-vv" | "-D" | "-l" | "-N" => {}
                _ => {
                    if mode.is_some() {
                        return mode;
                    }
                    tmux_seen = false;
                }
            }
        }
    }
    mode
}

pub(crate) enum TmuxFeed {
    /// Bytes to forward through the regular output pipeline.
    Passthrough(Vec<u8>),
    /// A complete startup response and session notification were confirmed.
    ///
    /// `startup` preserves the bounded candidate for protocol verification.
    /// `rest` follows it; `passthrough` precedes it in the normal stream.
    Detected {
        passthrough: Vec<u8>,
        startup: Vec<u8>,
        rest: Vec<u8>,
    },
}

/// Consume input escape sequences as units so their tails cannot be mistaken
/// for command text. Such editing input marks the entire submission ambiguous.
#[derive(Default, PartialEq, Clone, Copy)]
enum InputTrack {
    #[default]
    Normal,
    /// Saw `ESC`; the next byte selects the sequence class.
    Esc,
    /// Inside `ESC [` — ends on a final byte in `0x40..=0x7e`.
    Csi,
    /// Inside `ESC O` (SS3) — exactly one more byte follows.
    Ss3,
    /// Inside `ESC ]` (OSC) — ends on BEL or ST (`ESC \`).
    Osc,
    /// Inside OSC after an `ESC` — `\` closes ST, anything else stays OSC.
    OscEsc,
}

/// Minimal output framing, updated even while bytes are held in a candidate.
/// Releasing a candidate must not reset this state: its tail may still be DCS.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum OutTrack {
    #[default]
    Normal,
    Esc,
    EscIntermediate,
    Csi,
    Ss3,
    Osc,
    OscEsc,
    Dcs,
    DcsEsc,
}

struct OutputContext {
    track: OutTrack,
    line_start: bool,
    percent_line: bool,
}

impl Default for OutputContext {
    fn default() -> Self {
        Self {
            track: OutTrack::Normal,
            line_start: true,
            percent_line: false,
        }
    }
}

impl OutputContext {
    fn can_start_escape(&self) -> bool {
        matches!(
            self.track,
            OutTrack::Normal
                | OutTrack::Esc
                | OutTrack::EscIntermediate
                | OutTrack::Csi
                | OutTrack::Ss3
        )
    }

    fn advance(&mut self, byte: u8) {
        // CAN/SUB cancel an unfinished terminal sequence, including strings.
        if matches!(byte, 0x18 | 0x1a) {
            self.track = OutTrack::Normal;
            return;
        }
        if byte == 0x1b && self.can_start_escape() {
            self.track = OutTrack::Esc;
            return;
        }
        self.track = match self.track {
            OutTrack::Normal => {
                if matches!(byte, b'\r' | b'\n') {
                    self.line_start = true;
                    self.percent_line = false;
                } else if !byte.is_ascii_control() {
                    self.percent_line |= self.line_start && byte == b'%';
                    self.line_start = false;
                }
                OutTrack::Normal
            }
            OutTrack::Esc => match byte {
                b'[' => OutTrack::Csi,
                b'O' => OutTrack::Ss3,
                b']' => OutTrack::Osc,
                // DCS, SOS, PM and APC strings all terminate at ST.
                b'P' | b'X' | b'^' | b'_' => OutTrack::Dcs,
                0x20..=0x2f => OutTrack::EscIntermediate,
                _ if byte.is_ascii_control() => OutTrack::Esc,
                _ => OutTrack::Normal,
            },
            OutTrack::EscIntermediate => {
                if (0x30..=0x7e).contains(&byte) {
                    OutTrack::Normal
                } else {
                    self.track
                }
            }
            OutTrack::Csi => {
                if (0x40..=0x7e).contains(&byte) {
                    OutTrack::Normal
                } else {
                    self.track
                }
            }
            OutTrack::Ss3 => OutTrack::Normal,
            OutTrack::Osc => match byte {
                0x07 => OutTrack::Normal,
                0x1b => OutTrack::OscEsc,
                _ => OutTrack::Osc,
            },
            OutTrack::OscEsc => match byte {
                b'\\' | 0x07 => OutTrack::Normal,
                0x1b => OutTrack::OscEsc,
                _ => OutTrack::Osc,
            },
            OutTrack::Dcs => {
                if byte == 0x1b {
                    OutTrack::DcsEsc
                } else {
                    OutTrack::Dcs
                }
            }
            OutTrack::DcsEsc => match byte {
                b'\\' => OutTrack::Normal,
                0x1b => OutTrack::DcsEsc,
                _ => OutTrack::Dcs,
            },
        };
    }
}

#[derive(PartialEq, Eq)]
enum HintStatus {
    Waiting,
    SuppressedByOutput,
    Notified,
}

struct LaunchHint {
    id: u64,
    mode: ControlMode,
    hint_at: Instant,
    status: HintStatus,
    // Do not count the submitted line's PTY echo as a shell response.
    echo_tail: bool,
}

struct SingleCWindow {
    deadline: Instant,
}

struct OutputCandidate {
    mode: ControlMode,
    bytes: Vec<u8>,
    startup: Startup,
    consumed: usize,
    deadline: Instant,
    // Input can replace a launch without invalidating a double-C candidate.
    launch_id: Option<u64>,
}

impl OutputCandidate {
    fn has_prefix(&self) -> bool {
        self.bytes.len() >= self.mode.prefix().len()
    }
}

#[derive(Default)]
pub(crate) struct Expiration {
    pub(crate) bytes: Vec<u8>,
    pub(crate) notice: bool,
}

pub(crate) struct Detection {
    pub(crate) output: TmuxFeed,
    pub(crate) notice: bool,
}

#[derive(Default)]
pub(crate) struct TmuxControlDetector {
    output: OutputContext,
    candidate: Option<OutputCandidate>,
    done: bool,
    launch: Option<LaunchHint>,
    next_launch_id: u64,
    single_c: Option<SingleCWindow>,
    released: Vec<u8>,
    input_line: Vec<u8>,
    input_track: InputTrack,
    input_csi: Vec<u8>,
    input_pasting: bool,
    input_ambiguous: bool,
    input_last_cr: bool,
}

impl TmuxControlDetector {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn single_c_enabled(&self) -> bool {
        self.single_c
            .as_ref()
            .is_some_and(|window| Instant::now() < window.deadline)
    }

    fn silence_deadline(&self) -> Option<Instant> {
        if self.done || self.candidate.is_some() {
            return None;
        }
        self.launch
            .as_ref()
            .filter(|hint| hint.status == HintStatus::Waiting)
            .map(|hint| hint.hint_at)
    }

    fn expire_silence(&mut self) -> bool {
        if self
            .silence_deadline()
            .is_some_and(|at| Instant::now() >= at)
        {
            self.launch.as_mut().unwrap().status = HintStatus::Notified;
            return true;
        }
        false
    }

    fn reset_launch(&mut self) {
        if self
            .candidate
            .as_ref()
            .is_some_and(|candidate| candidate.mode == ControlMode::SingleC)
        {
            let candidate = self.candidate.take().unwrap();
            self.released.extend(candidate.bytes);
        }
        self.launch = None;
        self.single_c = None;
    }

    fn start_launch(&mut self, mode: ControlMode) {
        self.next_launch_id = self.next_launch_id.wrapping_add(1);
        self.launch = Some(LaunchHint {
            id: self.next_launch_id,
            mode,
            hint_at: Instant::now() + SILENT_HINT_DELAY,
            status: HintStatus::Waiting,
            echo_tail: true,
        });
        self.single_c = (mode == ControlMode::SingleC).then(|| SingleCWindow {
            deadline: Instant::now() + SINGLE_C_WINDOW,
        });
    }

    fn suppress_advisory(&mut self) {
        if let Some(hint) = &mut self.launch
            && hint.status == HintStatus::Waiting
        {
            hint.status = HintStatus::SuppressedByOutput;
            tracing::debug!(mode = ?hint.mode, "tmux detector: launch advisory suppressed by output");
        }
    }

    /// Returns held bytes without rewinding their already-consumed framing.
    /// Mere terminal escape prefix mismatches are not failed tmux launches.
    fn reject_candidate(&mut self, reason: &'static str, timeout: bool) -> Expiration {
        let candidate = self.candidate.take().unwrap();
        let mut notice = false;
        if candidate.has_prefix() {
            tracing::debug!(mode = ?candidate.mode, reason, bytes = candidate.bytes.len(), "tmux detector: startup candidate rejected");
            if self
                .launch
                .as_ref()
                .is_some_and(|hint| Some(hint.id) == candidate.launch_id)
            {
                notice = timeout && self.launch.as_ref().unwrap().status == HintStatus::Waiting;
                self.launch = None;
                self.single_c = None;
            }
        }
        Expiration {
            bytes: candidate.bytes,
            notice,
        }
    }

    /// Both timer wakeups and incoming output use the same expiry path.
    pub(crate) fn expire(&mut self) -> Expiration {
        let mut result = Expiration {
            bytes: std::mem::take(&mut self.released),
            notice: false,
        };
        if self
            .single_c
            .as_ref()
            .is_some_and(|window| Instant::now() >= window.deadline)
        {
            self.single_c = None;
        }
        if self
            .candidate
            .as_ref()
            .is_some_and(|candidate| Instant::now() >= candidate.deadline)
        {
            let expired = self.reject_candidate("timeout", true);
            result.bytes.extend(expired.bytes);
            result.notice = expired.notice;
        }
        result.notice |= self.expire_silence();
        result
    }

    pub(crate) fn deadline(&self) -> Option<Instant> {
        if !self.released.is_empty() {
            return Some(Instant::now());
        }
        [
            self.candidate.as_ref().map(|candidate| candidate.deadline),
            self.single_c.as_ref().map(|window| window.deadline),
            self.silence_deadline(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    pub(crate) fn feed_output(&mut self, data: &[u8]) -> Detection {
        let expired = self.expire();
        let output = match self.feed(data) {
            TmuxFeed::Passthrough(bytes) if expired.bytes.is_empty() => {
                TmuxFeed::Passthrough(bytes)
            }
            TmuxFeed::Passthrough(bytes) => TmuxFeed::Passthrough([expired.bytes, bytes].concat()),
            TmuxFeed::Detected {
                passthrough,
                startup,
                rest,
            } => TmuxFeed::Detected {
                passthrough: [expired.bytes, passthrough].concat(),
                startup,
                rest,
            },
        };
        Detection {
            output,
            notice: expired.notice,
        }
    }

    /// stderr can suppress an advisory, but cannot establish stdout framing.
    pub(crate) fn observe_extended_output(&mut self, data: &[u8]) {
        if !data.is_empty() {
            self.suppress_advisory();
        }
    }

    /// Input is only evidence for an advisory and the single-C compatibility
    /// window. Double-C discovery is independent of this bounded tracker.
    pub(crate) fn feed_input(&mut self, data: &[u8]) {
        if self.done {
            return;
        }
        for &byte in data {
            let after_cr = self.input_last_cr;
            self.input_last_cr = byte == b'\r';
            if byte == b'\n' && after_cr {
                continue;
            }
            match self.input_track {
                InputTrack::Normal => {}
                InputTrack::Esc => {
                    self.input_track = match byte {
                        b'[' => {
                            self.input_csi.clear();
                            InputTrack::Csi
                        }
                        b'O' => InputTrack::Ss3,
                        b']' => InputTrack::Osc,
                        0x1b => InputTrack::Esc,
                        // Alt+key and other two-byte escapes produce no text.
                        _ => InputTrack::Normal,
                    };
                    continue;
                }
                InputTrack::Csi => {
                    // Parameter/intermediate bytes are 0x20..=0x3f; the
                    // first byte in 0x40..=0x7e terminates the sequence.
                    if self.input_csi.len() < 16 {
                        self.input_csi.push(byte);
                    }
                    if (0x40..=0x7e).contains(&byte) {
                        match self.input_csi.as_slice() {
                            b"200~" => self.input_pasting = true,
                            b"201~" => self.input_pasting = false,
                            _ => {}
                        }
                        self.input_track = InputTrack::Normal;
                    }
                    continue;
                }
                InputTrack::Ss3 => {
                    self.input_track = InputTrack::Normal;
                    continue;
                }
                InputTrack::Osc => {
                    self.input_track = match byte {
                        0x07 => InputTrack::Normal,
                        0x1b => InputTrack::OscEsc,
                        _ => InputTrack::Osc,
                    };
                    continue;
                }
                InputTrack::OscEsc => {
                    self.input_track = if byte == b'\\' {
                        InputTrack::Normal
                    } else {
                        InputTrack::Osc
                    };
                    continue;
                }
            }
            match byte {
                b'\r' | b'\n' if self.input_pasting => {
                    // Newlines inside a paste cannot turn its next line into
                    // a reliably observed shell submission.
                    self.input_ambiguous = true;
                    self.input_line.clear();
                }
                b'\r' | b'\n' => {
                    if self.input_line.len() < MAX_INPUT_LINE_BYTES
                        && input_continues(&self.input_line)
                    {
                        self.input_line.push(b'\n');
                        self.input_ambiguous = true;
                        self.reset_launch();
                        continue;
                    }
                    let line = std::mem::take(&mut self.input_line);
                    let ambiguous = std::mem::take(&mut self.input_ambiguous);
                    self.reset_launch();
                    if !ambiguous
                        && let Ok(text) = std::str::from_utf8(&line)
                        && let Some(mode) = tmux_control_command(text)
                    {
                        self.start_launch(mode);
                    }
                }

                0x12 | 0x10 | 0x0e => {
                    self.input_ambiguous = true;
                    self.input_line.clear();
                }
                0x03 | 0x1c | 0x1a => {
                    // Ctrl+C / Ctrl+\ / Ctrl+Z kill or suspend the pending
                    // line: the buffered text never reaches the shell, so
                    // keeping it would poison the next tracked command (a
                    // leading ETX makes `tmux -CC` unrecognizable). Clear
                    // the recall flag too — a bare Enter afterwards is a
                    // real empty submit, not a recalled command.
                    self.input_ambiguous = false;
                    self.input_line.clear();
                    self.reset_launch();
                }
                0x7f | 0x08 => {
                    self.input_line.pop();
                    self.input_ambiguous = true;
                }
                0x1b => {
                    self.input_track = InputTrack::Esc;
                    self.input_ambiguous = true;
                }
                _ if byte.is_ascii_control() && byte != b'\t' => {
                    self.input_ambiguous = true;
                }
                _ => {
                    if self.input_line.len() < MAX_INPUT_LINE_BYTES {
                        self.input_line.push(byte);
                    } else {
                        self.input_line.clear();
                        self.input_ambiguous = true;
                    }
                }
            }
        }
    }

    fn feed(&mut self, data: &[u8]) -> TmuxFeed {
        if self.done {
            return TmuxFeed::Passthrough(data.to_vec());
        }
        let mut out = Vec::with_capacity(data.len());
        for (i, &byte) in data.iter().enumerate() {
            if byte == 0x1b
                && self.output.can_start_escape()
                && self
                    .candidate
                    .as_ref()
                    .is_some_and(|candidate| candidate.mode == ControlMode::SingleC)
            {
                // Bare control-looking text must not hide a subsequent DCS.
                out.extend(self.reject_candidate("interrupted", false).bytes);
            }
            // Reconsider the mismatching byte after releasing only the old
            // prefix. In ESC ESC P1000p, the second ESC starts a new sequence.
            if self.candidate.as_ref().is_some_and(|candidate| {
                !candidate.has_prefix() && candidate.mode.prefix()[candidate.bytes.len()] != byte
            }) {
                out.extend(self.reject_candidate("prefix", false).bytes);
            }
            // Release at the cap before considering the current byte, which
            // might itself start a new DCS after a single-C candidate.
            if self
                .candidate
                .as_ref()
                .is_some_and(|candidate| candidate.bytes.len() == CONFIRM_MAX_BYTES)
            {
                out.extend(self.reject_candidate("limit", false).bytes);
            }
            if self.candidate.is_none() {
                let mode = if byte == 0x1b && self.output.can_start_escape() {
                    Some(ControlMode::DoubleC)
                } else if byte == b'%'
                    && self.output.track == OutTrack::Normal
                    && self.output.line_start
                    && self.single_c_enabled()
                {
                    Some(ControlMode::SingleC)
                } else {
                    None
                };
                if let Some(mode) = mode {
                    self.candidate = Some(OutputCandidate {
                        mode,
                        bytes: Vec::new(),
                        startup: Startup::default(),
                        consumed: 0,
                        deadline: Instant::now() + CONFIRM_TIMEOUT,
                        launch_id: self
                            .launch
                            .as_ref()
                            .filter(|hint| hint.mode == mode)
                            .map(|hint| hint.id),
                    });
                } else {
                    if self.output.track == OutTrack::Normal {
                        if byte == b'\n' {
                            if let Some(hint) = &mut self.launch {
                                hint.echo_tail = false;
                            }
                        } else if self.output.line_start
                            && !self.output.percent_line
                            && byte != b'%'
                            && !byte.is_ascii_control()
                            && self.launch.as_ref().is_some_and(|hint| !hint.echo_tail)
                        {
                            self.suppress_advisory();
                        }
                    }
                    self.output.advance(byte);
                    out.push(byte);
                    continue;
                }
            }
            self.output.advance(byte);
            let candidate = self.candidate.as_mut().unwrap();
            candidate.bytes.push(byte);
            if candidate.mode == ControlMode::DoubleC
                && candidate.has_prefix()
                && self.output.track == OutTrack::Normal
            {
                // ST or CAN/SUB ended the string before startup completed.
                // Do not swallow the next independent DCS while waiting for LF.
                out.extend(self.reject_candidate("terminated", false).bytes);
                continue;
            }
            if byte == b'\n' {
                let mut line = &candidate.bytes[candidate.consumed..candidate.bytes.len() - 1];
                if candidate.consumed == 0 {
                    line = line.strip_prefix(CONTROL_PREFIX).unwrap_or(line);
                }
                let result = candidate.startup.line(line);
                candidate.consumed = candidate.bytes.len();
                match result {
                    Some(true) => {
                        let startup = self.candidate.take().unwrap().bytes;
                        self.done = true;
                        self.launch = None;
                        self.single_c = None;
                        return TmuxFeed::Detected {
                            passthrough: out,
                            startup,
                            rest: data[i + 1..].to_vec(),
                        };
                    }
                    Some(false) => {}
                    None => out.extend(self.reject_candidate("invalid_startup", false).bytes),
                }
            }
        }
        TmuxFeed::Passthrough(out)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CONFIRM_MAX_BYTES, CONFIRM_TIMEOUT, CONTROL_PREFIX, SILENT_HINT_DELAY, SINGLE_C_WINDOW,
    };
    use super::{ControlMode, HintStatus, TmuxControlDetector, TmuxFeed, tmux_control_command};
    use std::time::Duration;

    const STARTUP: &[u8] = b"%begin 1 2 0\r\n%end 1 2 0\r\n%window-add @0\r\n%sessions-changed\r\n%session-changed $0 0\r\n";

    fn cc_startup() -> Vec<u8> {
        [CONTROL_PREFIX, STARTUP].concat()
    }

    fn passthrough(feed: TmuxFeed) -> Vec<u8> {
        match feed {
            TmuxFeed::Passthrough(bytes) => bytes,
            TmuxFeed::Detected { .. } => panic!("unexpected takeover"),
        }
    }

    #[test]
    fn only_unambiguous_startup_options_enable_advisories() {
        for line in [
            "tmux new-session echo -CC",
            "tmux display-message -C",
            "tmux -L -CC",
            "tmux -- -CC",
            "echo ';' tmux -CC",
            "sudo -u tmux -CC",
            "echo x\\; tmux -CC",
            "echo \"x; tmux -CC\"",
            "tmux '-CC",
            "tmux -CC\\",
            "cat > tmux -CC",
            "echo foo=bar tmux -CC",
            "tmux -f -CC new-session",
            "tmux -CCC",
            "ssh -N host tmux -CC",
            "\"\" tmux -CC",
            "\"FOO=x\" tmux -CC",
        ] {
            let mut detector = TmuxControlDetector::new();
            detector.feed_input(format!("{line}\r").as_bytes());
            assert!(detector.silence_deadline().is_none(), "{line}");
        }
        for line in [
            "tmux -L test -CC",
            "sudo -u root tmux -C",
            "nice -n 5 tmux -CC",
            "env -u TEST tmux -C",
            "'tmux' '-CC'",
            "ssh -p 2222 host tmux -CC",
        ] {
            let mut detector = TmuxControlDetector::new();
            detector.feed_input(format!("{line}\r\n").as_bytes());
            assert!(detector.silence_deadline().is_some(), "{line}");
        }
    }

    #[test]
    fn tmux_control_command_matching() {
        // Positive: direct, wrapped, chained, nested-ssh invocations.
        for line in [
            "tmux -CC",
            "tmux -C",
            "tmux -CC new -As0",
            "tmux -CC attach -t 0",
            "  tmux   -CC  ",
            "sudo tmux -CC",
            "sudo -k nice tmux -CC",
            "env FOO=1 tmux -CC",
            "FOO=1 tmux -C",
            "cd ~ && tmux -CC attach",
            "ssh host tmux -CC",
            "ssh -p 2222 user@host tmux -CC",
            "/usr/bin/tmux -CC",
            "tmux \t-CC",
        ] {
            assert!(tmux_control_command(line).is_some(), "should match: {line}");
        }
        // Negative: tmux as an argument, missing flag, lookalike names.
        for line in [
            "echo tmux -CC",
            "printf 'tmux -CC\\n'",
            "cat tmux -CC",
            "tmux new",
            "tmux attach -t 0",
            "tmux -CCx",
            "tmuxer -CC",
            "mytmux -CC",
            "ssh host",
            "",
            "ls -la",
        ] {
            assert!(
                !tmux_control_command(line).is_some(),
                "should not match: {line}"
            );
        }
    }

    #[test]
    fn command_modes_are_distinct_including_repeated_c() {
        for (line, expected) in [
            ("tmux -C", ControlMode::SingleC),
            ("tmux -CC", ControlMode::DoubleC),
            ("tmux -C -C attach", ControlMode::DoubleC),
            ("tmux -C -L test -C", ControlMode::DoubleC),
            ("sudo tmux -C", ControlMode::SingleC),
            ("env -C somewhere tmux -CC", ControlMode::DoubleC),
        ] {
            assert_eq!(tmux_control_command(line), Some(expected), "{line}");
            let mut detector = TmuxControlDetector::new();
            detector.feed_input(format!("{line}\r\n").as_bytes());
            assert_eq!(
                detector.single_c_enabled(),
                expected == ControlMode::SingleC
            );
        }
    }

    #[test]
    fn only_explicit_single_c_permits_bare_startup() {
        for input in [
            b"".as_slice(),
            b"ls\r",
            b"\r",
            b"tmux -CC\r",
            b"\x1b[A\r",
            b"\x12\r",
            b"\x10\r",
            b"\x0e\r",
            b"\x1bOA\r",
            b"tmux -C\x1b[D\r",
            b"tmux -Cx\x7f\r",
            b"tmux -Cx\x08\r",
            b"\x1b[200~tmux -C\x1b[201~\r",
            b"\x02dtmux -C\r",
            b"echo tmux -C\r",
            b"echo 'x; tmux -C'\r",
            b"\x1bOA\x1bbtmux -C\r",
            b"tmux -C\x1b[1;5D\x1b[1;5C\r",
        ] {
            let mut detector = TmuxControlDetector::new();
            detector.feed_input(input);
            assert!(!detector.single_c_enabled(), "{input:?}");
            assert_eq!(passthrough(detector.feed_output(STARTUP).output), STARTUP);
            assert!(detector.candidate.is_none());
        }
        for input in [b"tmux -C\r".as_slice(), b"sudo -u root tmux -C\r\n"] {
            let mut detector = TmuxControlDetector::new();
            // Includes split writes, as used by interactive input and login commands.
            for byte in input {
                detector.feed_input(&[*byte]);
            }
            assert!(matches!(
                detector.feed_output(STARTUP).output,
                TmuxFeed::Detected { .. }
            ));
        }
    }

    #[test]
    fn double_c_needs_no_visible_command() {
        for input in [
            b"".as_slice(),
            b"alias_name\r",
            b"./script\r",
            b"\x1b[A\r",
            b"\x12\r",
            b"tmux -CCx\x7f\r",
            b"\x1b[200~tmux -CC\x1b[201~\r",
            b"\x02dtmux -CC\r",
            b"echo tmux -CC\r",
            b"printf '\\nmarker\\n'; { tmux -CC; }; x=$?\r",
        ] {
            let mut detector = TmuxControlDetector::new();
            detector.feed_input(input);
            assert!(detector.deadline().is_none(), "no blind arm for {input:?}");
            assert!(matches!(
                detector.feed_output(&cc_startup()).output,
                TmuxFeed::Detected { .. }
            ));
        }
    }

    #[test]
    fn quotes_paste_and_overflow_do_not_promote_command_suffixes() {
        for input in [
            b"echo '\rtmux -C\r'\r".as_slice(),
            b"echo \\\rtmux -C\r",
            b"\x1b[200~first\rtmux -C\r\x1b[201~\r",
            b"\x1b]ignored\x07tmux -C\r",
        ] {
            let mut detector = TmuxControlDetector::new();
            for byte in input {
                detector.feed_input(&[*byte]);
                assert!(detector.launch.is_none());
                assert!(detector.single_c.is_none());
            }
            detector.feed_input(b"tmux -C\r");
            assert!(detector.single_c_enabled());
        }
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(&vec![b'x'; super::MAX_INPUT_LINE_BYTES + 1]);
        detector.feed_input(b"tmux -C\r");
        assert!(detector.deadline().is_none());
        assert!(detector.input_line.len() <= super::MAX_INPUT_LINE_BYTES);
        detector.feed_input(b"tmux -C\r");
        assert!(detector.single_c_enabled());
    }

    #[test]
    fn cancel_then_retype_keeps_explicit_commands_recognizable() {
        for &cancel in b"\x03\x1c\x1a" {
            let mut detector = TmuxControlDetector::new();
            detector.feed_input(b"tmux -C\r");
            detector.feed_input(&[cancel]);
            assert!(detector.deadline().is_none());
            detector.feed_input(b"tmux -C\r");
            assert!(matches!(
                detector.feed_output(STARTUP).output,
                TmuxFeed::Detected { .. }
            ));
        }
    }

    #[test]
    fn startup_and_output_are_separated_at_every_chunk_boundary() {
        for single_c in [false, true] {
            let startup = if single_c {
                STARTUP.to_vec()
            } else {
                cc_startup()
            };
            let before = if single_c {
                b"prompt\r\n".as_slice()
            } else {
                b"prompt%text"
            };
            let after = b"%output %0 hello\n";
            let bytes = [before, &startup, after].concat();
            for split in 0..=bytes.len() {
                let mut detector = TmuxControlDetector::new();
                if single_c {
                    detector.feed_input(b"tmux -C\r");
                }
                let mut visible = Vec::new();
                let mut control = Vec::new();
                let mut detected = false;
                for chunk in [&bytes[..split], &bytes[split..]] {
                    match detector.feed_output(chunk).output {
                        TmuxFeed::Passthrough(bytes) => {
                            if detected {
                                control.extend(bytes);
                            } else {
                                visible.extend(bytes);
                            }
                        }
                        TmuxFeed::Detected {
                            passthrough,
                            startup: raw,
                            rest,
                        } => {
                            assert!(!detected);
                            assert_eq!(raw, startup);
                            visible.extend(passthrough);
                            control.extend(rest);
                            detected = true;
                        }
                    }
                }
                assert!(detected, "split {split}");
                assert_eq!(visible, before);
                assert_eq!(control, after);
            }
            let mut detector = TmuxControlDetector::new();
            if single_c {
                detector.feed_input(b"tmux -C\r");
            }
            for (i, byte) in startup.iter().enumerate() {
                let feed = detector.feed_output(&[*byte]).output;
                if i + 1 == startup.len() {
                    assert!(matches!(feed, TmuxFeed::Detected { .. }));
                } else {
                    assert!(passthrough(feed).is_empty());
                }
            }
        }
    }

    #[test]
    fn unrelated_terminal_sequences_preserve_framing_and_future_discovery() {
        let cc = cc_startup();
        for bytes in [
            b"\x1b[?1049h\x1b[2J\x1bOA\x1b(B%window-add @1\n".to_vec(),
            [b"\x1b]title".as_slice(), &cc, b"\x1b\x1b\\"].concat(),
            [b"\x1b]title".as_slice(), &cc, b"\x07"].concat(),
            [b"\x1bPother".as_slice(), &cc, b"\x1b\\"].concat(),
            [b"\x1b_".as_slice(), &cc, b"\x1b\\"].concat(),
            [b"\x1b^".as_slice(), &cc, b"\x1b\\"].concat(),
            [b"\x1bX".as_slice(), &cc, b"\x1b\\"].concat(),
        ] {
            for split in 0..=bytes.len() {
                let mut detector = TmuxControlDetector::new();
                let mut visible = passthrough(detector.feed_output(&bytes[..split]).output);
                // Starting an explicit launch must not reset OSC/DCS context.
                detector.feed_input(b"tmux -C\r");
                visible.extend(passthrough(detector.feed_output(&bytes[split..]).output));
                assert_eq!(visible, bytes, "split {split}");
                assert!(matches!(
                    detector.feed_output(&cc).output,
                    TmuxFeed::Detected { .. }
                ));
            }
        }
    }

    #[test]
    fn escape_restart_and_string_cancellation_allow_new_dcs() {
        for prefix in [
            b"\x1b".as_slice(),
            b"\x1b[12;",
            b"\x1bPother\x18",
            b"\x1b]title\x1a",
        ] {
            let mut detector = TmuxControlDetector::new();
            let TmuxFeed::Detected { passthrough, .. } = detector
                .feed_output(&[prefix, &cc_startup()].concat())
                .output
            else {
                panic!("DCS not detected after {prefix:?}")
            };
            assert_eq!(passthrough, prefix);
        }
    }

    #[test]
    fn terminated_or_interrupted_candidate_cannot_hide_a_new_dcs() {
        for prefix in [
            b"\x1bP1000p%begin 1 2 0\n\x1b\\".as_slice(),
            b"\x1bP1000p%begin 1 2 0\n\x18",
            b"%begin ",
            b"%begin 1 2 0\n",
        ] {
            let mut detector = TmuxControlDetector::new();
            detector.feed_input(b"tmux -C\r");
            let bytes = [prefix, &cc_startup()].concat();
            let TmuxFeed::Detected {
                passthrough,
                startup,
                ..
            } = detector.feed_output(&bytes).output
            else {
                panic!("candidate hid the next DCS")
            };
            assert_eq!(passthrough, prefix);
            assert_eq!(startup, cc_startup());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn partial_prefix_expiry_is_transparent_and_does_not_announce_tmux() {
        for prefix in [b"\x1b".as_slice(), b"\x1bP", b"\x1bP1000", b"%", b"%beg"] {
            let mut detector = TmuxControlDetector::new();
            detector.feed_input(b"tmux -C\r");
            assert!(passthrough(detector.feed_output(prefix).output).is_empty());
            tokio::time::advance(CONFIRM_TIMEOUT).await;
            let expired = detector.expire();
            assert_eq!(expired.bytes, prefix);
            assert!(!expired.notice);
            assert!(!detector.expire().notice);
            tokio::time::advance(SILENT_HINT_DELAY - CONFIRM_TIMEOUT).await;
            assert!(detector.expire().notice);
            assert!(!detector.expire().notice);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn timer_and_incoming_data_have_identical_expiry_results() {
        for (input, prefix, notice) in [
            (b"tmux -C\r".as_slice(), b"%begin 1 2 0\n".as_slice(), true),
            (b"tmux -CC\r", CONTROL_PREFIX, true),
            (b"tmux -CC\r", b"\x1b", false),
            (b"\x1b[A\r", CONTROL_PREFIX, false),
        ] {
            let mut timer = TmuxControlDetector::new();
            let mut incoming = TmuxControlDetector::new();
            for detector in [&mut timer, &mut incoming] {
                detector.feed_input(input);
                assert!(passthrough(detector.feed_output(prefix).output).is_empty());
            }
            tokio::time::advance(CONFIRM_TIMEOUT).await;
            let expired = timer.expire();
            let later = timer.feed_output(b"tail");
            let together = incoming.feed_output(b"tail");
            assert_eq!(expired.notice, notice);
            assert_eq!(expired.notice, together.notice);
            assert_eq!(
                [expired.bytes, passthrough(later.output)].concat(),
                passthrough(together.output)
            );
            assert!(!timer.expire().notice);
            assert!(!incoming.expire().notice);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_single_c_candidate_is_released_before_later_output() {
        for input in [b"\x03".as_slice(), b"ls\r", b"\r", b"\x1b[A\r"] {
            let mut detector = TmuxControlDetector::new();
            detector.feed_input(b"tmux -C\r");
            detector.feed_output(b"%begin ");
            detector.feed_input(input);
            assert!(detector.deadline().is_some());
            let output = detector.feed_output(b"tail\n");
            assert_eq!(passthrough(output.output), b"%begin tail\n");
            assert!(!output.notice);
            tokio::time::advance(SILENT_HINT_DELAY).await;
            assert!(!detector.expire().notice);
            assert!(detector.deadline().is_none());
        }
    }

    #[test]
    fn input_cannot_cancel_any_fragment_of_double_c_startup() {
        let startup = cc_startup();
        for split in 1..startup.len() {
            for input in [b"\x03".as_slice(), b"ls\r", b"tmux -C\r", b"\x1b[A\r"] {
                let mut detector = TmuxControlDetector::new();
                assert!(passthrough(detector.feed_output(&startup[..split]).output).is_empty());
                detector.feed_input(input);
                let TmuxFeed::Detected {
                    startup: raw,
                    passthrough,
                    ..
                } = detector.feed_output(&startup[split..]).output
                else {
                    panic!("input cancelled double-C at {split}")
                };
                assert_eq!(raw, startup);
                assert!(passthrough.is_empty());
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn old_candidate_failure_does_not_clear_new_launch() {
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -CC\r");
        detector.feed_output(CONTROL_PREFIX);
        detector.feed_input(b"tmux -C\r");
        tokio::time::advance(CONFIRM_TIMEOUT).await;
        assert!(!detector.expire().notice);
        assert!(detector.single_c_enabled());
        tokio::time::advance(SILENT_HINT_DELAY - CONFIRM_TIMEOUT).await;
        assert!(detector.expire().notice);
    }

    #[tokio::test(start_paused = true)]
    async fn candidate_deadline_is_fixed_and_independent_of_single_c_window() {
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -C\r");
        tokio::time::advance(SINGLE_C_WINDOW - Duration::from_secs(1)).await;
        detector.feed_output(b"%begin ");
        let deadline = detector.candidate.as_ref().unwrap().deadline;
        tokio::time::advance(Duration::from_secs(2)).await;
        detector.feed_output(b"1 2 0\n");
        assert!(!detector.single_c_enabled());
        assert_eq!(detector.candidate.as_ref().unwrap().deadline, deadline);
        assert!(matches!(
            detector
                .feed_output(b"%end 1 2 0\n%session-changed $0 0\n")
                .output,
            TmuxFeed::Detected { .. }
        ));

        let mut detector = TmuxControlDetector::new();
        detector.feed_output(CONTROL_PREFIX);
        let deadline = detector.deadline().unwrap();
        tokio::time::advance(Duration::from_secs(4)).await;
        detector.feed_output(b"%begin 1 2 0\n");
        assert_eq!(detector.deadline(), Some(deadline));
        tokio::time::advance(Duration::from_secs(1)).await;
        let expired = detector.expire();
        assert_eq!(expired.bytes, [CONTROL_PREFIX, b"%begin 1 2 0\n"].concat());
        assert!(!expired.notice);
    }

    #[tokio::test(start_paused = true)]
    async fn expired_single_c_window_cannot_start_new_candidates() {
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -C\r");
        tokio::time::advance(SINGLE_C_WINDOW).await;
        assert_eq!(passthrough(detector.feed_output(STARTUP).output), STARTUP);
        assert!(detector.deadline().is_none());
        assert!(matches!(
            detector.feed_output(&cc_startup()).output,
            TmuxFeed::Detected { .. }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn rejected_or_expired_dcs_keeps_its_string_context() {
        for timeout in [false, true] {
            let mut detector = TmuxControlDetector::new();
            detector.feed_input(b"tmux -CC\r");
            let bytes = if timeout {
                b"\x1bP1000p%begin 1 2 0\n\xffpartial".as_slice()
            } else {
                b"\x1bP1000p%begin 1 2 0\n%end 1 3 0\n"
            };
            let mut visible = passthrough(detector.feed_output(bytes).output);
            if timeout {
                tokio::time::advance(CONFIRM_TIMEOUT).await;
                visible.extend(detector.expire().bytes);
            }
            assert_eq!(visible, bytes);
            assert!(detector.launch.is_none());
            // A nested signature inside the rejected string remains ordinary data.
            assert_eq!(
                passthrough(detector.feed_output(&cc_startup()).output),
                cc_startup()
            );
            assert_eq!(
                passthrough(detector.feed_output(b"\x1b\\").output),
                b"\x1b\\"
            );
            assert!(matches!(
                detector.feed_output(&cc_startup()).output,
                TmuxFeed::Detected { .. }
            ));
        }
    }

    #[test]
    fn startup_requires_matching_block_and_valid_session() {
        for body in [
            b"%begin 1 2 0\n%end 1 3 0\n%session-changed $0 x\n".as_slice(),
            b"%begin 1 2 0\n%error 1 2 0\n%session-changed $0 x\n",
            b"%begin 1 2 0\n%end 1 2 0\n%session-changed nope x\n",
            b"%begin 1 2 0\n%end 1 2 0\n%exit\n",
        ] {
            for mode in [ControlMode::SingleC, ControlMode::DoubleC] {
                let mut detector = TmuxControlDetector::new();
                let bytes = if mode == ControlMode::SingleC {
                    detector.feed_input(b"tmux -C\r");
                    body.to_vec()
                } else {
                    [CONTROL_PREFIX, body].concat()
                };
                assert_eq!(passthrough(detector.feed_output(&bytes).output), bytes);
                assert!(detector.candidate.is_none());
            }
        }
    }

    #[test]
    fn startup_buffer_is_bounded_and_overflow_does_not_lose_bytes() {
        for single_c in [false, true] {
            let mut detector = TmuxControlDetector::new();
            if single_c {
                detector.feed_input(b"tmux -C\r");
            }
            let prefix = if single_c {
                b"%begin 1 2 0\n".as_slice()
            } else {
                b"\x1bP1000p%begin 1 2 0\n"
            };
            let bytes = [prefix, &vec![b'x'; CONFIRM_MAX_BYTES - prefix.len()]].concat();
            assert!(passthrough(detector.feed_output(&bytes).output).is_empty());
            assert_eq!(
                detector.candidate.as_ref().unwrap().bytes.len(),
                CONFIRM_MAX_BYTES
            );
            assert_eq!(
                passthrough(detector.feed_output(b"overflow\n").output),
                [bytes, b"overflow\n".to_vec()].concat()
            );
            assert!(detector.candidate.is_none());
            if !single_c {
                detector.feed_output(b"\x1b\\");
            }
            assert!(matches!(
                detector.feed_output(&cc_startup()).output,
                TmuxFeed::Detected { .. }
            ));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn only_explicit_launches_get_one_silence_advisory() {
        for input in [
            b"\x1b[A\r".as_slice(),
            b"\x12\r",
            b"\x10\r",
            b"\x1bOA\r",
            b"ls\r",
        ] {
            let mut detector = TmuxControlDetector::new();
            detector.feed_input(input);
            assert!(detector.deadline().is_none());
            tokio::time::advance(SINGLE_C_WINDOW).await;
            assert!(!detector.expire().notice);
        }
        for input in [b"tmux -C\r".as_slice(), b"tmux -CC\r"] {
            let mut detector = TmuxControlDetector::new();
            detector.feed_input(input);
            tokio::time::advance(SILENT_HINT_DELAY).await;
            assert!(detector.expire().notice);
            assert!(!detector.expire().notice);
            // An advisory does not prevent a delayed control-mode launch.
            assert!(matches!(
                detector.feed_output(&cc_startup()).output,
                TmuxFeed::Detected { .. }
            ));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn shell_output_suppresses_hint_without_resolving_discovery() {
        for response in [
            b"zsh: command not found: tmux\r\nprompt> ".as_slice(),
            b"\x1b[?2004l\r-bash: tmux: command not found\r\n",
            b"\x1b[?2004l-bash: tmux: command not found\r\n",
            b"script: preparing session\r\n",
        ] {
            for single_c in [false, true] {
                let mut detector = TmuxControlDetector::new();
                detector.feed_input(if single_c {
                    b"tmux -C\r"
                } else {
                    b"tmux -CC\r"
                });
                detector.feed_output(b"command echo\r\n");
                assert!(detector.silence_deadline().is_some());
                assert_eq!(passthrough(detector.feed_output(response).output), response);
                assert!(detector.silence_deadline().is_none());
                assert_eq!(detector.single_c_enabled(), single_c);
                tokio::time::advance(SILENT_HINT_DELAY).await;
                assert!(!detector.expire().notice);
                // A bare single-C response must start on its own line.
                detector.feed_output(b"\r\n");
                let bytes = if single_c {
                    STARTUP.to_vec()
                } else {
                    cc_startup()
                };
                assert!(matches!(
                    detector.feed_output(&bytes).output,
                    TmuxFeed::Detected { .. }
                ));
            }
        }
    }

    #[test]
    fn echo_and_protocol_noise_do_not_count_as_shell_response() {
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -CC\r");
        let bytes = b"tmux -CC\r\n%unknown 1 2 3\n%sessions-changed\n\x1b[?1049h\x1b[2J";
        assert_eq!(passthrough(detector.feed_output(bytes).output), bytes);
        assert!(detector.silence_deadline().is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn stderr_suppresses_hint_but_never_contributes_to_startup() {
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -CC\r");
        detector.observe_extended_output(&cc_startup());
        assert!(detector.candidate.is_none());
        assert!(!detector.done);
        assert!(passthrough(detector.feed_output(CONTROL_PREFIX).output).is_empty());
        detector.observe_extended_output(STARTUP);
        assert!(!detector.done);
        tokio::time::advance(CONFIRM_TIMEOUT).await;
        assert!(!detector.expire().notice);
        detector.feed_output(b"\x1b\\");
        assert!(matches!(
            detector.feed_output(&cc_startup()).output,
            TmuxFeed::Detected { .. }
        ));
    }

    #[test]
    fn high_volume_output_logs_only_the_first_advisory_transition() {
        // Callsite interest is process-global. Other tests exercise this same
        // callsite without a subscriber, so count events in an isolated test
        // process rather than depending on parallel test registration order.
        const ISOLATED: &str = "NYATERM_TMUX_LOG_TEST_ISOLATED";
        if std::env::var_os(ISOLATED).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tmux::detect::tests::high_volume_output_logs_only_the_first_advisory_transition",
                    "--nocapture",
                ])
                .env(ISOLATED, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout).contains("1 passed;"),
                "isolated log regression failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        use std::sync::{Arc, Mutex};
        use tracing::{Event, Level, Metadata, Subscriber, span};
        #[derive(Clone)]
        struct Events(Arc<Mutex<Vec<Level>>>);
        impl Subscriber for Events {
            fn enabled(&self, metadata: &Metadata<'_>) -> bool {
                metadata.target().ends_with("tmux::detect")
            }
            fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
                span::Id::from_u64(1)
            }
            fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
            fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
            fn event(&self, event: &Event<'_>) {
                self.0.lock().unwrap().push(*event.metadata().level());
            }
            fn enter(&self, _: &span::Id) {}
            fn exit(&self, _: &span::Id) {}
        }
        let events = Events(Arc::default());
        tracing::subscriber::with_default(events.clone(), || {
            let output = b"ordinary shell output\r\n".repeat(10_000);
            let mut detector = TmuxControlDetector::new();
            detector.feed_input(b"\x1b[A\r");
            assert_eq!(passthrough(detector.feed_output(&output).output), output);
            assert!(events.0.lock().unwrap().is_empty());
            for _ in 0..2 {
                detector.feed_input(b"tmux -CC\r");
                detector.feed_output(b"tmux -CC\r\n");
                detector.feed_output(&output);
                detector.observe_extended_output(b"stderr");
                detector.feed_output(&output);
                assert!(
                    detector
                        .launch
                        .as_ref()
                        .is_some_and(|hint| hint.status == HintStatus::SuppressedByOutput)
                );
            }
        });
        assert_eq!(*events.0.lock().unwrap(), vec![Level::DEBUG, Level::DEBUG]);
    }
}
