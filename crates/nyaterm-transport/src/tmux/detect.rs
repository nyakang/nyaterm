//! Bounded passive confirmation of tmux startup in a normal SSH stream.

use super::handshake::{CONFIRM_MAX_BYTES, CONFIRM_TIMEOUT, CONTROL_PREFIX, Startup};
use std::time::Duration;
use tokio::time::Instant;

/// How long after a `tmux -CC` command line a control notification is still
/// expected. tmux announces control mode essentially immediately; a longer
/// window would only widen the false-positive surface for `%` lines printed
/// by unrelated programs.
const ARM_WINDOW: Duration = Duration::from_secs(30);

/// Shorter arm window for a *submitted but invisible* command line: shell
/// history recall (arrow keys, Ctrl+R) sends only editing escapes — the
/// recalled command text arrives as output redraw, so input-side matching
/// can never see it. The gesture only enables passive startup detection;
/// framing and the nonce query must still confirm the control protocol. Gated on having consumed an escape sequence so a bare
/// Enter at an empty prompt never arms detection.
const BLIND_ARM_WINDOW: Duration = Duration::from_secs(5);

/// Upper bound for the tracked input line; longer lines cannot be matched
/// reliably anyway, so tracking resets instead of growing without bound.
const MAX_INPUT_LINE_BYTES: usize = 4096;

/// Grace period after a submitted `tmux -CC` for *any* channel output at
/// all. A tmux too old for the control protocol (pre-2.x `%session-changed`,
/// `%layout-change`) enters `-CC` yet emits no confirmable startup framing —
/// the channel just stays silent while the remote process owns the tty.
/// After this delay we surface an advisory hint; the detector itself keeps
/// working so a merely slow attach can still take over later.
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

fn is_control_flag(token: &str) -> bool {
    // `-C` enters control mode, `-CC` additionally silences echo.
    matches!(token, "-C" | "-CC")
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
/// indirect launches) degrade to plain passthrough — never to a hijack.
pub(crate) fn is_tmux_control_command(line: &str) -> bool {
    // Expansions, escapes, redirections and comments need a full shell parser.
    // Keep them eligible for passive discovery, never an explicit advisory.
    if line.contains(['\\', '$', '`', '<', '>', '#', '(', ')', '{', '}']) {
        return false;
    }
    let tokens = shell_tokens(line);
    if tokens.iter().any(|raw| {
        let token = strip_quotes(raw);
        token.contains(['\'', '"'])
            || (raw.starts_with('\'') && !raw.ends_with('\''))
            || (raw.starts_with('"') && !raw.ends_with('"'))
    }) {
        return false;
    }
    let mut expect_command = true;
    let mut expect_ssh_dest = false;
    let mut ssh_flag_arg = false;
    let mut tmux_seen = false;
    let mut option_arg = false;
    let mut wrapper: Option<&str> = None;

    for raw in tokens {
        let token = strip_quotes(raw);
        if token.is_empty() {
            return false;
        }
        if raw == token && is_shell_separator(token) {
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
                    return false;
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
                    return false;
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
                    _ => return false,
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
            if is_control_flag(token) {
                return true;
            }
            match token {
                "-L" | "-S" | "-f" | "-T" | "-c" => {
                    option_arg = true;
                }
                "-2" | "-u" | "-v" | "-vv" | "-D" | "-l" | "-N" => {}
                _ => {
                    tmux_seen = false;
                }
            }
        }
    }
    false
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

/// Escape-sequence state while tracking user input bytes. Cursor/editing
/// keys emit multi-byte sequences (CSI, SS3, OSC) that must be skipped as
/// a unit: ending CSI at the `[` byte itself would leak the parameter tail
/// (e.g. `A` from `ESC [ A` arrow-up) into the tracked line, and a
/// history-recalled `tmux -CC` would then fail to match.
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

/// How forwarded output is framed for line tracking. Rejected candidates
/// are flushed back to the terminal verbatim, but their trailing bytes are
/// still part of the same escape sequence or `%`-line — they must not
/// corrupt `line_start` or count as shell output.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum OutTrack {
    #[default]
    Normal,
    /// Inside a terminal escape sequence whose `\x1bX` lead was flushed as
    /// a rejected candidate.
    Csi,
    Ss3,
    Osc,
    OscEsc,
    Dcs,
    DcsEsc,
    /// Inside a `%`-line that failed the startup prefix gate.
    PercentLine,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Evidence {
    ExplicitCommand,
    PassiveHint,
}

struct Attempt {
    evidence: Evidence,
    deadline: Instant,
    hint_at: Instant,
    hint_sent: bool,
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
    /// The next byte begins a new line.
    line_start: bool,
    /// Escape/`%`-line framing of the forwarded output stream.
    out_track: OutTrack,
    /// Bytes of a not-yet-decided line that started with `%`.
    candidate: Option<Vec<u8>>,
    /// Set once control mode was detected; a dead detector passes through.
    done: bool,
    startup: Startup,
    startup_offset: usize,
    candidate_deadline: Option<Instant>,
    /// Input line being composed on the user side; completed on Enter.
    input_line: Vec<u8>,
    /// Escape-sequence tracking state for [`Self::feed_input`].
    input_track: InputTrack,
    attempt: Option<Attempt>,
    /// Candidate bytes invalidated by a new submission still belong to output.
    released: Vec<u8>,
    /// The submitted command's own PTY echo is expected to end with one
    /// `\n`; ordinary line-start bytes are not counted as shell output
    /// until that echo tail has passed.
    echo_tail: bool,
    /// A printable line-start byte arrived after the echo tail — the shell
    /// is talking (e.g. `zsh: command not found: tmux`), so the attempt
    /// already failed instead of leaving a silent control client behind.
    saw_shell_output: bool,
    /// An escape sequence was consumed since the current input line began —
    /// evidence of a recall/editing gesture that gates the blind arm.
    input_saw_escape: bool,
    input_last_cr: bool,
}

impl TmuxControlDetector {
    pub(crate) fn new() -> Self {
        Self {
            line_start: true,
            out_track: OutTrack::Normal,
            candidate: None,
            done: false,
            startup: Startup::default(),
            startup_offset: 0,
            candidate_deadline: None,
            input_line: Vec::new(),
            input_track: InputTrack::Normal,
            attempt: None,
            released: Vec::new(),
            echo_tail: false,
            saw_shell_output: false,
            input_saw_escape: false,
            input_last_cr: false,
        }
    }

    fn expiry_is_attempt(&self) -> bool {
        !self.saw_shell_output
            && self.attempt.as_ref().is_some_and(|attempt| {
                attempt.evidence == Evidence::ExplicitCommand && !attempt.hint_sent
            })
    }

    pub(crate) fn is_armed(&self) -> bool {
        self.attempt
            .as_ref()
            .is_some_and(|attempt| Instant::now() < attempt.deadline)
    }

    fn silence_deadline(&self) -> Option<Instant> {
        if self.done || self.candidate.is_some() || !self.expiry_is_attempt() {
            return None;
        }
        self.attempt.as_ref().map(|attempt| attempt.hint_at)
    }

    fn expire_silence(&mut self) -> bool {
        if !self
            .silence_deadline()
            .is_some_and(|at| Instant::now() >= at)
        {
            return false;
        }
        if let Some(attempt) = &mut self.attempt {
            attempt.hint_sent = true;
        }
        true
    }

    fn reset_attempt(&mut self) {
        if self.candidate.is_some() {
            let bytes = self.take_candidate();
            self.released.extend(bytes);
        }
        self.attempt = None;
        self.echo_tail = false;
        self.saw_shell_output = false;
        self.out_track = OutTrack::Normal;
    }

    fn arm(&mut self, evidence: Evidence) {
        let now = Instant::now();
        self.attempt = Some(Attempt {
            evidence,
            deadline: now
                + match evidence {
                    Evidence::ExplicitCommand => ARM_WINDOW,
                    Evidence::PassiveHint => BLIND_ARM_WINDOW,
                },
            hint_at: now + SILENT_HINT_DELAY,
            hint_sent: false,
        });
        self.echo_tail = true;
        self.saw_shell_output = false;
        self.out_track = OutTrack::Normal;
    }

    /// Both the timer and incoming data use this exact expiry path.
    pub(crate) fn expire(&mut self) -> Expiration {
        let mut result = Expiration {
            bytes: std::mem::take(&mut self.released),
            notice: false,
        };
        if self
            .candidate_deadline
            .is_some_and(|at| Instant::now() >= at)
        {
            result.notice = self.expiry_is_attempt();
            result.bytes.extend(self.expire_candidate());
        } else {
            result.notice = self.expire_silence();
        }
        result
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

    /// stderr may explain a failed launch, but can never establish stdout's
    /// protocol. Suppress the silence advisory without disabling discovery.
    pub(crate) fn observe_extended_output(&mut self, data: &[u8]) {
        if !data.is_empty() {
            self.saw_shell_output = true;
        }
    }

    /// Track bytes written to the channel (user input, AI capture commands)
    /// so a `tmux -CC` command line arms output detection.
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
                        b'[' => InputTrack::Csi,
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
                    if (0x40..=0x7e).contains(&byte) {
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
                b'\r' | b'\n' => {
                    if self.input_line.len() < MAX_INPUT_LINE_BYTES
                        && input_continues(&self.input_line)
                    {
                        self.input_line.push(b'\n');
                        self.input_saw_escape = true;
                        self.reset_attempt();
                        continue;
                    }
                    let line = std::mem::take(&mut self.input_line);
                    let saw_escape = std::mem::take(&mut self.input_saw_escape);
                    self.reset_attempt();
                    if saw_escape {
                        self.arm(Evidence::PassiveHint);
                    } else {
                        self.evaluate_input_line(&line);
                    }
                }

                0x12 | 0x10 | 0x0e => {
                    self.input_saw_escape = true;
                    self.input_line.clear();
                }
                0x03 | 0x1c | 0x1a => {
                    // Ctrl+C / Ctrl+\ / Ctrl+Z kill or suspend the pending
                    // line: the buffered text never reaches the shell, so
                    // keeping it would poison the next tracked command (a
                    // leading ETX makes `tmux -CC` unrecognizable). Clear
                    // the recall flag too — a bare Enter afterwards is a
                    // real empty submit, not a recalled command.
                    self.input_saw_escape = false;
                    self.input_line.clear();
                    self.reset_attempt();
                }
                0x7f | 0x08 => {
                    self.input_line.pop();
                    self.input_saw_escape = true;
                }
                0x1b => {
                    self.input_track = InputTrack::Esc;
                    self.input_saw_escape = true;
                }
                _ => {
                    if self.input_line.len() < MAX_INPUT_LINE_BYTES {
                        self.input_line.push(byte);
                    } else {
                        self.input_line.clear();
                        self.input_saw_escape = true;
                    }
                }
            }
        }
    }

    /// True when a `tmux` occurrence outside shell quoting starts a suffix
    /// that parses as a control-mode command. Escaped bytes and quote
    /// interior are skipped so `echo 'x; tmux -CC'` cannot match.
    fn tmux_suffix_matches(text: &str) -> bool {
        let bytes = text.as_bytes();
        let mut single = false;
        let mut double = false;
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\'' if !double => single = !single,
                b'"' if !single => double = !double,
                b'\\' if !single => i += 1,
                _ if !single
                    && !double
                    && bytes[i..].starts_with(b"tmux")
                    && is_tmux_control_command(
                        text[i..].split([';', '|', '&', '}']).next().unwrap_or(""),
                    ) =>
                {
                    return true;
                }
                _ => {}
            }
            i += 1;
        }
        false
    }

    fn evaluate_input_line(&mut self, line: &[u8]) {
        let text = std::str::from_utf8(line).unwrap_or("");
        if is_tmux_control_command(text) {
            self.arm(Evidence::ExplicitCommand);
        } else if Self::tmux_suffix_matches(text) {
            // A suffix may be stale TUI input or merely an argument to echo.
            self.arm(Evidence::PassiveHint);
        }
    }

    pub(crate) fn deadline(&self) -> Option<Instant> {
        if !self.released.is_empty() {
            return Some(Instant::now());
        }
        self.candidate_deadline.or_else(|| self.silence_deadline())
    }

    fn expire_candidate(&mut self) -> Vec<u8> {
        self.attempt = None;
        self.take_candidate()
    }

    fn take_candidate(&mut self) -> Vec<u8> {
        self.candidate_deadline = None;
        self.startup = Startup::default();
        self.startup_offset = 0;
        let bytes = self.candidate.take().unwrap_or_default();
        self.line_start = bytes
            .last()
            .is_some_and(|last| matches!(last, b'\n' | b'\r'));
        bytes
    }

    fn feed(&mut self, data: &[u8]) -> TmuxFeed {
        if self.done {
            return TmuxFeed::Passthrough(data.to_vec());
        }
        let mut out = Vec::new();
        if !self.is_armed() && self.candidate.is_none() {
            if self.candidate.is_some() {
                out.extend(self.take_candidate());
            }
            out.extend_from_slice(data);
            if let Some(last) = data.last() {
                self.line_start = matches!(last, b'\n' | b'\r');
            }
            return TmuxFeed::Passthrough(out);
        }
        for (i, &byte) in data.iter().enumerate() {
            if self.candidate.is_none() && !self.is_armed() {
                out.extend_from_slice(&data[i..]);
                self.line_start = data
                    .last()
                    .is_some_and(|byte| matches!(byte, b'\n' | b'\r'));
                break;
            }
            if self.candidate.is_none() {
                match self.out_track {
                    OutTrack::Normal => {}
                    OutTrack::PercentLine => {
                        out.push(byte);
                        if byte == b'\n' {
                            self.out_track = OutTrack::Normal;
                            self.line_start = true;
                        }
                        continue;
                    }
                    OutTrack::Csi => {
                        out.push(byte);
                        if (0x40..=0x7e).contains(&byte) {
                            self.out_track = OutTrack::Normal;
                        }
                        continue;
                    }
                    OutTrack::Ss3 => {
                        out.push(byte);
                        self.out_track = OutTrack::Normal;
                        continue;
                    }
                    OutTrack::Osc => {
                        out.push(byte);
                        self.out_track = match byte {
                            0x07 => OutTrack::Normal,
                            0x1b => OutTrack::OscEsc,
                            _ => OutTrack::Osc,
                        };
                        continue;
                    }
                    OutTrack::OscEsc => {
                        out.push(byte);
                        self.out_track = if byte == b'\\' {
                            OutTrack::Normal
                        } else {
                            OutTrack::Osc
                        };
                        continue;
                    }
                    OutTrack::Dcs => {
                        out.push(byte);
                        if byte == 0x1b {
                            self.out_track = OutTrack::DcsEsc;
                        }
                        continue;
                    }
                    OutTrack::DcsEsc => {
                        out.push(byte);
                        self.out_track = if byte == b'\\' {
                            OutTrack::Normal
                        } else {
                            OutTrack::Dcs
                        };
                        continue;
                    }
                }
                if byte == 0x1b || (self.line_start && byte == b'%') {
                    self.candidate = Some(Vec::new());
                    self.candidate_deadline = Some(Instant::now() + CONFIRM_TIMEOUT);
                } else {
                    if byte == b'\n' {
                        self.echo_tail = false;
                    } else if self.line_start && !byte.is_ascii_control() && !self.echo_tail {
                        // Ordinary text at a line start after the echo is the
                        // shell responding — the submitted command already
                        // failed or finished, so no silent control client is
                        // holding the tty.
                        self.saw_shell_output = true;
                        tracing::info!("tmux detector: shell output seen; attempt resolved");
                    }
                    out.push(byte);
                    // A bare `\r` also returns the cursor to column 0 —
                    // readline wraps prompt redraws as `\x1b[?2004l\r`, so
                    // error lines can start after CR without an LF.
                    self.line_start = byte == b'\n' || byte == b'\r';
                    continue;
                }
            }
            let candidate = self.candidate.as_mut().unwrap();
            candidate.push(byte);
            if candidate.len() > CONFIRM_MAX_BYTES {
                out.extend(self.expire_candidate());
                out.extend_from_slice(&data[i + 1..]);
                return TmuxFeed::Passthrough(out);
            }
            if self.startup_offset == 0 {
                // -CC's DCS may start mid-prompt; unrelated terminal escapes
                // and ordinary '%' notifications are forwarded immediately.
                let prefix = if candidate[0] == 0x1b {
                    CONTROL_PREFIX
                } else {
                    b"%begin "
                };
                if candidate.len() <= prefix.len() && !prefix.starts_with(candidate) {
                    let line_start_before = self.line_start;
                    let flushed = self.take_candidate();
                    self.out_track = match flushed.first() {
                        Some(&0x1b) => {
                            // The flush put escape bytes in `out`; they do
                            // not print, so the incoming line position stays.
                            self.line_start = line_start_before;
                            match flushed.get(1) {
                                Some(&b'[') => OutTrack::Csi,
                                Some(&b'O') => OutTrack::Ss3,
                                Some(&b']') => OutTrack::Osc,
                                Some(&b'P') => OutTrack::Dcs,
                                _ => OutTrack::Normal,
                            }
                        }
                        Some(&b'%') => OutTrack::PercentLine,
                        _ => OutTrack::Normal,
                    };
                    out.extend(flushed);
                    continue;
                }
            }
            if byte == b'\n' {
                let mut line = &candidate[self.startup_offset..candidate.len() - 1];
                if self.startup_offset == 0 {
                    line = line.strip_prefix(CONTROL_PREFIX).unwrap_or(line);
                }
                let result = self.startup.line(line);
                self.startup_offset = candidate.len();
                match result {
                    Some(true) => {
                        let startup = self.take_candidate();
                        self.done = true;
                        return TmuxFeed::Detected {
                            passthrough: out,
                            startup,
                            rest: data[i + 1..].to_vec(),
                        };
                    }
                    Some(false) => {}
                    None => out.extend(self.take_candidate()),
                }
            }
        }
        TmuxFeed::Passthrough(out)
    }
}

#[cfg(test)]
mod tests {
    use super::{BLIND_ARM_WINDOW, TmuxControlDetector, TmuxFeed, is_tmux_control_command};
    const STARTUP: &[u8] = b"%begin 1 2 0\r\n%end 1 2 0\r\n%window-add @0\r\n%sessions-changed\r\n%session-changed $0 0\r\n";

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
    fn edited_input_only_enables_passive_detection() {
        for input in [b"tmux -CC\x1b[D\r".as_slice(), b"tmux -CCx\x7f\r"] {
            let mut detector = TmuxControlDetector::new();
            detector.feed_input(input);
            assert!(detector.silence_deadline().is_none());
            assert!(matches!(
                detector.feed_output(STARTUP).output,
                TmuxFeed::Detected { .. }
            ));
        }
    }

    #[test]
    fn multiline_quoted_text_never_becomes_an_explicit_command() {
        let mut detector = TmuxControlDetector::new();
        for input in [b"echo '\r".as_slice(), b"tmux -CC\r", b"'\r"] {
            detector.feed_input(input);
            assert!(detector.silence_deadline().is_none());
        }
        detector.feed_input(b"tmux -CC\r");
        assert!(detector.silence_deadline().is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn stderr_suppresses_advisories_without_contributing_protocol_bytes() {
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -CC\r");
        detector.observe_extended_output(STARTUP);
        assert!(detector.candidate.is_none());
        assert!(!detector.done);
        tokio::time::advance(super::SILENT_HINT_DELAY).await;
        assert!(!detector.expire().notice);
        assert!(matches!(
            detector.feed_output(STARTUP).output,
            TmuxFeed::Detected { .. }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn timer_and_incoming_data_have_identical_expiry_results() {
        for input in [b"tmux -CC\r".as_slice(), b"\x1b[A\r"] {
            for prefix in [b"\x1b".as_slice(), b"%begin 1 2 0\n", b"\x1bP1000p"] {
                let mut timer = TmuxControlDetector::new();
                let mut incoming = TmuxControlDetector::new();
                for detector in [&mut timer, &mut incoming] {
                    detector.feed_input(input);
                    assert!(
                        matches!(detector.feed_output(prefix).output, TmuxFeed::Passthrough(bytes) if bytes.is_empty())
                    );
                }
                tokio::time::advance(super::CONFIRM_TIMEOUT).await;
                let expired = timer.expire();
                let later = timer.feed_output(b"tail");
                let together = incoming.feed_output(b"tail");
                assert_eq!(expired.notice, input.starts_with(b"tmux"));
                assert_eq!(expired.notice, together.notice);
                let TmuxFeed::Passthrough(later) = later.output else {
                    panic!("passthrough")
                };
                let TmuxFeed::Passthrough(together) = together.output else {
                    panic!("passthrough")
                };
                assert_eq!([expired.bytes, later].concat(), together);
                assert_eq!(together, [prefix, b"tail"].concat());
                assert!(!timer.expire().notice);
                assert!(!incoming.expire().notice);
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_or_new_submission_releases_candidate_without_stale_notice() {
        for input in [b"\x03".as_slice(), b"ls\r", b"\r", b"\x1b[A\r"] {
            let mut detector = TmuxControlDetector::new();
            detector.feed_input(b"tmux -CC\r");
            detector.feed_output(b"%begin ");
            detector.feed_input(input);
            let expired = detector.expire();
            assert_eq!(expired.bytes, b"%begin ");
            assert!(!expired.notice);
            tokio::time::advance(super::SILENT_HINT_DELAY).await;
            assert!(!detector.expire().notice);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn silence_hint_does_not_repeat_or_prevent_a_slow_valid_startup() {
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -CC\r");
        tokio::time::advance(super::SILENT_HINT_DELAY).await;
        assert!(detector.expire().notice);
        assert!(!detector.expire().notice);
        assert!(matches!(
            detector.feed_output(STARTUP).output,
            TmuxFeed::Detected { .. }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn candidate_deadline_is_not_extended_by_fragments() {
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"\x1b[A\r");
        detector.feed_output(b"%begin ");
        let deadline = detector.deadline().unwrap();
        tokio::time::advance(std::time::Duration::from_secs(4)).await;
        detector.feed_output(b"1 2 0\n");
        assert_eq!(detector.deadline(), Some(deadline));
        tokio::time::sleep_until(deadline).await;
        let expired = detector.expire();
        assert_eq!(expired.bytes, b"%begin 1 2 0\n");
        assert!(!expired.notice);
    }

    #[test]
    fn detects_control_line_and_splits_stream() {
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -CC new -As0\r");
        let chunk =
            b"user@host:~$ tmux -CC new -As0\r\n\x1bP1000p%begin 1 2 0\r\n%end 1 2 0\r\n%session-changed $0 0\r\n%window-add @1\r\n";
        match detector.feed(chunk) {
            TmuxFeed::Detected {
                passthrough, rest, ..
            } => {
                assert_eq!(passthrough, b"user@host:~$ tmux -CC new -As0\r\n");
                assert_eq!(rest, b"%window-add @1\r\n");
            }
            TmuxFeed::Passthrough(_) => panic!("expected detection"),
        }
    }

    #[test]
    fn ctrl_c_before_retype_keeps_the_command_recognizable() {
        // An unsupported `tmux -CC` leaves the remote client owning the tty;
        // the user presses Ctrl+C and retries. The ETX byte must not stay in
        // the tracked line — `\x03tmux -CC` is not a control command and the
        // detector would never arm again.
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -CC new -As0\r");
        detector.feed_input(b"\x03");
        detector.feed_input(b"tmux -CC new -As0\r");
        match detector.feed(&[STARTUP, b"rest"].concat()) {
            TmuxFeed::Detected { rest, .. } => assert_eq!(rest, b"rest"),
            TmuxFeed::Passthrough(_) => panic!("re-armed detector must detect"),
        }
    }

    #[test]
    fn unarmed_output_never_triggers_takeover() {
        // A normal shell that prints tmux-looking lines must not be
        // hijacked — the review's `printf '%window-add @1\n'` case.
        let mut detector = TmuxControlDetector::new();
        match detector.feed(b"%window-add @1\n%session-changed $0 x\n") {
            TmuxFeed::Passthrough(bytes) => {
                assert_eq!(bytes, b"%window-add @1\n%session-changed $0 x\n");
            }
            TmuxFeed::Detected { .. } => panic!("unarmed detector must pass through"),
        }
    }

    #[test]
    fn non_tmux_command_disarms() {
        // `tmux -CC` failed (`command not found`); the next command closes
        // the arming window so later `%` output is harmless.
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -CC\r");
        detector.feed_input(b"ls\r");
        match detector.feed(b"%window-add @1\n") {
            TmuxFeed::Passthrough(bytes) => assert_eq!(bytes, b"%window-add @1\n"),
            TmuxFeed::Detected { .. } => panic!("disarmed detector must pass through"),
        }
    }

    #[test]
    fn startup_command_can_pre_arm() {
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -CC new -As0\r");
        match detector.feed(&[STARTUP, b"rest"].concat()) {
            TmuxFeed::Detected { rest, .. } => assert_eq!(rest, b"rest"),
            TmuxFeed::Passthrough(_) => panic!("pre-armed detector must detect"),
        }
    }

    #[test]
    fn ai_wrapped_command_arms() {
        // Marker-wrapped capture commands still contain the tmux invocation
        // (`{ cmd; }` braces and glued `;` must not hide command position).
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(
            b" printf '\\n__DF_CMD_''START_x__\\n'; { tmux -CC attach -t 0; }; _dfec=$?; printf '\\n__DF_CMD_''END_x__\\n'\r",
        );
        assert!(matches!(detector.feed(STARTUP), TmuxFeed::Detected { .. }));
    }

    #[test]
    fn quoted_separator_never_arms() {
        // `;` inside quotes stays inside the word — echoing a tmux-looking
        // string must not arm, or `echo 'x; tmux -CC'` would hijack.
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"echo 'x; tmux -CC'\r");
        match detector.feed(STARTUP) {
            TmuxFeed::Passthrough(bytes) => {
                assert_eq!(bytes, STARTUP);
            }
            TmuxFeed::Detected { .. } => panic!("quoted tmux text must not arm"),
        }
    }

    #[test]
    fn ignores_percent_text_mid_line() {
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -CC\r");
        match detector.feed(b"value is %window-add @1\n") {
            TmuxFeed::Passthrough(bytes) => {
                assert_eq!(bytes, b"value is %window-add @1\n");
            }
            TmuxFeed::Detected { .. } => panic!("must not detect mid-line"),
        }
    }

    #[test]
    fn ignores_unknown_percent_lines_at_start() {
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -CC\r");
        match detector.feed(b"%percent-sign example\nnext") {
            TmuxFeed::Passthrough(bytes) => {
                assert_eq!(bytes, b"%percent-sign example\nnext");
            }
            TmuxFeed::Detected { .. } => panic!("must not detect unknown keyword"),
        }
    }

    #[test]
    fn candidate_line_flushed_when_keyword_breaks() {
        let mut detector = TmuxControlDetector::new();
        match detector.feed(b"\n%foo_bar more\n") {
            TmuxFeed::Passthrough(bytes) => assert_eq!(bytes, b"\n%foo_bar more\n"),
            TmuxFeed::Detected { .. } => panic!("unexpected detection"),
        }
    }

    #[test]
    fn detection_spans_every_chunk_boundary() {
        let bytes = [
            b"prompt\x1bP1000p".as_slice(),
            STARTUP,
            b"%output %0 hello\n",
        ]
        .concat();
        for split in 0..bytes.len() {
            let mut detector = TmuxControlDetector::new();
            detector.feed_input(b"\x1b[A\r");
            let mut visible = Vec::new();
            let mut detected = false;
            for chunk in [&bytes[..split], &bytes[split..]] {
                match detector.feed(chunk) {
                    TmuxFeed::Passthrough(data) => {
                        if !detected {
                            visible.extend(data);
                        }
                    }
                    TmuxFeed::Detected {
                        passthrough,
                        startup,
                        ..
                    } => {
                        visible.extend(passthrough);
                        assert_eq!(startup, [b"\x1bP1000p".as_slice(), STARTUP].concat());
                        detected = true;
                    }
                }
            }
            assert!(detected, "split {split}");
            assert_eq!(visible, b"prompt");
        }
    }

    #[test]
    fn recalled_non_tmux_commands_never_take_over() {
        for input in [b"\x1b[A\r".as_slice(), b"\x12\r", b"\x1bOA\r"] {
            let mut detector = TmuxControlDetector::new();
            detector.feed_input(input);
            let output = b"printf '%%window-add @1\\n'\r\n%window-add @1\r\nuser@host$ ";
            assert!(
                matches!(detector.feed(output), TmuxFeed::Passthrough(bytes) if bytes == output)
            );
        }
    }

    #[test]
    fn startup_needs_matching_response_and_session() {
        for bytes in [
            b"%begin 1 2 0\n%end 1 3 0\n%session-changed $0 x\n".as_slice(),
            b"%begin 1 2 0\n%error 1 2 0\n%session-changed $0 x\n",
        ] {
            let mut detector = TmuxControlDetector::new();
            detector.feed_input(b"tmux -CC new -As0\r");
            assert!(matches!(detector.feed(bytes), TmuxFeed::Passthrough(data) if data == bytes));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn candidate_timeout_replays_raw_bytes_without_new_data() {
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -CC new -As0\r");
        let bytes = b"\x1bP1000p%begin 1 2 0\r\n\xffincomplete";
        assert!(matches!(detector.feed(bytes), TmuxFeed::Passthrough(data) if data.is_empty()));
        tokio::time::sleep_until(detector.deadline().unwrap()).await;
        assert_eq!(detector.expire_candidate(), bytes);
        assert!(detector.deadline().is_none());
    }

    #[test]
    fn startup_buffer_is_bounded_and_preserves_overflow() {
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -CC new -As0\r");
        let bytes = [
            b"%begin 1 2 0\n".as_slice(),
            &vec![b'x'; super::CONFIRM_MAX_BYTES],
        ]
        .concat();
        assert!(matches!(detector.feed(&bytes), TmuxFeed::Passthrough(data) if data == bytes));
        assert!(detector.candidate.is_none());
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
            assert!(is_tmux_control_command(line), "should match: {line}");
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
            assert!(!is_tmux_control_command(line), "should not match: {line}");
        }
    }

    #[test]
    fn arrow_key_history_recall_still_arms() {
        // Re-attaching usually means recalling `tmux -CC` with arrow-up —
        // the shell redraws the recalled text, so the input side only sees
        // the CSI sequence and Enter. The sequence must be skipped whole:
        // leaking `A` into the line made re-attach detection impossible.
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"\x1b[A\r");
        assert!(matches!(detector.feed(STARTUP), TmuxFeed::Detected { .. }));
    }

    #[test]
    fn csi_parameters_do_not_leak_into_input_line() {
        // `ESC [ 1 ; 5 D` (Ctrl+Left) — the `[` itself is in the
        // final-byte range, so a naive tracker ends CSI early and pushes
        // `1;5D` into the line, corrupting the command match.
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -CC new -As0");
        // word-jump left then more editing noise before submitting
        detector.feed_input(b"\x1b[1;5D\x1b[1;5C\r");
        assert!(matches!(detector.feed(STARTUP), TmuxFeed::Detected { .. }));
    }

    #[test]
    fn ss3_and_alt_keys_do_not_corrupt_input_line() {
        // Application-mode arrow (`ESC O A`) and Alt+key two-byte escapes
        // are editing input, not command text.
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"\x1bOA\x1bb"); // SS3 arrow-up, Alt+b
        detector.feed_input(b"tmux -CC new -As0\r");
        assert!(matches!(detector.feed(STARTUP), TmuxFeed::Detected { .. }));
    }

    #[test]
    fn bracketed_paste_markers_do_not_corrupt_input_line() {
        // `ESC [ 200 ~` … `ESC [ 201 ~` wrap a paste; the markers are CSI
        // sequences and must not push `200~`/`201~` into the line.
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"\x1b[200~tmux -CC new -As0\x1b[201~\r");
        assert!(matches!(detector.feed(STARTUP), TmuxFeed::Detected { .. }));
    }

    #[tokio::test(start_paused = true)]
    async fn recalled_silent_commands_never_emit_tmux_advisories() {
        for input in [b"\x1b[A\r".as_slice(), b"\x12\r", b"\x10\r", b"\x1bOA\r"] {
            let mut detector = TmuxControlDetector::new();
            detector.feed_input(input);
            assert!(detector.is_armed());
            assert!(detector.silence_deadline().is_none());
            assert!(!detector.expiry_is_attempt());
            tokio::time::advance(super::SILENT_HINT_DELAY).await;
            assert!(!detector.expire_silence());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn explicit_silent_tmux_command_emits_one_advisory() {
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -CC\r");
        tokio::time::sleep_until(detector.silence_deadline().unwrap()).await;
        assert!(detector.expire_silence());
        assert!(!detector.expire_silence());
    }

    #[test]
    fn recall_clears_a_previous_explicit_silence_deadline() {
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -CC\r");
        detector.feed_input(b"\x03\x1b[A\r");
        assert!(detector.silence_deadline().is_none());
    }

    #[test]
    fn tmux_suffix_alone_does_not_enable_silence_advisories() {
        for input in [b"echo tmux -CC\r".as_slice(), b"\x02dtmux -CC\r"] {
            let mut detector = TmuxControlDetector::new();
            detector.feed_input(input);
            assert!(detector.silence_deadline().is_none());
            assert!(matches!(detector.feed(STARTUP), TmuxFeed::Detected { .. }));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn recalled_program_partial_prefixes_expire_without_advisories() {
        for bytes in [b"\x1b".as_slice(), b"\x1bP", b"%", b"%beg"] {
            let mut detector = TmuxControlDetector::new();
            detector.feed_input(b"\x1b[A\r");
            assert!(matches!(detector.feed(bytes), TmuxFeed::Passthrough(data) if data.is_empty()));
            assert!(!detector.expiry_is_attempt());
            tokio::time::sleep_until(detector.deadline().unwrap()).await;
            assert!(!detector.expiry_is_attempt());
            assert_eq!(detector.expire_candidate(), bytes);
            assert!(!detector.expire_silence());
        }
    }

    #[tokio::test]
    async fn blind_armed_protocol_candidate_expires_without_advisory() {
        // A protocol-looking fragment is insufficient evidence for an advisory.
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"\x1b[A\r");
        let feed = detector.feed(b"%begin 1 2 0\n%output %0 x\n");
        assert!(matches!(feed, TmuxFeed::Passthrough(ref b) if b.is_empty()));
        tokio::time::sleep(BLIND_ARM_WINDOW + std::time::Duration::from_millis(50)).await;
        assert!(!detector.is_armed());
        assert!(!detector.expiry_is_attempt());
    }

    #[test]
    fn command_not_found_disarms_the_silence_hint() {
        // `zsh: command not found: tmux` means no control client ever
        // started — the shell already has the tty back, so neither the
        // hint nor the `detach-client` abort may fire.
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -CC\r");
        let feed = detector.feed(b"tmux -CC\r\nzsh: command not found: tmux\r\nprompt> ");
        let TmuxFeed::Passthrough(bytes) = feed else {
            panic!("expected passthrough");
        };
        assert_eq!(
            bytes,
            b"tmux -CC\r\nzsh: command not found: tmux\r\nprompt> "
        );
        assert!(detector.silence_deadline().is_none());
        assert!(!detector.expiry_is_attempt());
    }

    #[test]
    fn echo_alone_keeps_the_silence_deadline() {
        // Just the command's own PTY echo is still silence: the arm must
        // not treat its own reflection as shell output.
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -CC new -As0\r");
        let feed = detector.feed(b"tmux -CC new -As0\r\n");
        assert!(matches!(feed, TmuxFeed::Passthrough(ref b) if b == b"tmux -CC new -As0\r\n"));
        assert!(detector.silence_deadline().is_some());
    }

    #[test]
    fn shell_output_after_bare_cr_disarms_the_hint() {
        // readline emits `\x1b[?2004l\r` before printing a command's error —
        // the error line starts after a bare CR with no LF, so `\r` must
        // count as line start for the shell-output heuristic.
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -CC new -As0\r");
        let feed =
            detector.feed(b"tmux -CC new -As0\r\n\x1b[?2004l\r-bash: tmux: command not found\r\n");
        assert!(matches!(feed, TmuxFeed::Passthrough(_)));
        assert!(detector.silence_deadline().is_none());
    }

    #[test]
    fn keys_from_a_nested_tmux_do_not_block_the_next_arm() {
        // Detaching a plain `tmux attach` types `C-b d` inside the remote
        // program; those bytes land in our tracked line as a garbage prefix
        // even though the next shell command runs clean. The `tmux -CC`
        // tail must still arm — otherwise the control client's output
        // passes through invisibly and the tab hangs.
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"\x02dtmux -CC new -As0\r");
        assert!(detector.is_armed());
    }

    #[test]
    fn shell_output_after_flushed_csi_disarms_the_hint() {
        // Some readline builds emit `\x1b[?2004l` and print the error right
        // after it — no CR in between. The flushed CSI params must not touch
        // `line_start`, otherwise the error text would look mid-line.
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -CC new -As0\r");
        let feed =
            detector.feed(b"tmux -CC new -As0\r\n\x1b[?2004l-bash: tmux: command not found\r\n");
        assert!(matches!(feed, TmuxFeed::Passthrough(_)));
        assert!(detector.silence_deadline().is_none());
    }

    #[test]
    fn rejected_percent_line_keeps_silence_armed() {
        // An old control client prints `%`-lines that fail the prefix gate;
        // the line tail passes through but must not read as shell output.
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -CC new -As0\r");
        detector.feed(b"tmux -CC new -As0\r\n%unknown 1 2 3\n%also-old\n");
        assert!(detector.silence_deadline().is_some());
    }

    #[test]
    fn old_protocol_noise_is_not_shell_output() {
        // Pre-2.x tmux emits `%`-lines that fail the startup prefix gate
        // and pass through — still a control client holding the tty, so
        // the silence hint must stay armed.
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"tmux -CC new -As0\r");
        detector.feed(b"tmux -CC new -As0\r\n%sessions-changed\n");
        assert!(detector.silence_deadline().is_some());
    }

    #[tokio::test]
    async fn blind_armed_escape_candidate_is_not_an_attempt() {
        // A recalled TUI (vim, less, …) emits escapes that fail the
        // `%begin `/DCS prefix gate, so they pass through immediately and
        // never become a candidate — nothing to abort afterwards.
        let mut detector = TmuxControlDetector::new();
        detector.feed_input(b"\x1b[A\r");
        let feed = detector.feed(b"\x1b[?1049h\x1b[2J");
        let TmuxFeed::Passthrough(bytes) = feed else {
            panic!("expected passthrough");
        };
        assert_eq!(bytes, b"\x1b[?1049h\x1b[2J");
        tokio::time::sleep(BLIND_ARM_WINDOW + std::time::Duration::from_millis(50)).await;
        assert!(!detector.expiry_is_attempt());
    }
}
