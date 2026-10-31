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
    token.len() >= 2 && token.starts_with('-') && token[1..].chars().all(|c| c == 'C')
}

/// Whether `line` invokes tmux in control mode. Evaluated on the input side:
/// the command that spawned the protocol is visible before its output is.
///
/// Token-level analysis, not a full shell parser: it tracks command position
/// across `; | & >` separators and past prefix wrappers (`sudo`, `env`,
/// `VAR=val`), understands `ssh host tmux -CC`, and matches `tmux`/`*/tmux`
/// followed by a `-C`/`-CC` flag. Misses (aliases, `sh -c 'tmux -CC'`,
/// indirect launches) degrade to plain passthrough — never to a hijack.
pub(crate) fn is_tmux_control_command(line: &str) -> bool {
    let mut expect_command = true;
    let mut expect_ssh_dest = false;
    let mut ssh_flag_arg = false;
    let mut tmux_seen = false;

    for raw in shell_tokens(line) {
        let token = strip_quotes(raw);
        if token.is_empty() {
            continue;
        }
        if is_shell_separator(token) {
            expect_command = true;
            expect_ssh_dest = false;
            ssh_flag_arg = false;
            tmux_seen = false;
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
                if SSH_FLAGS_WITH_ARG.contains(&token) {
                    ssh_flag_arg = true;
                }
                continue;
            }
            expect_ssh_dest = false;
            expect_command = true;
            continue;
        }
        if expect_command {
            if token.starts_with('-') {
                // Option of a pending wrapper (`sudo -k tmux -CC`); options
                // that take arguments may still swallow the command name —
                // a documented miss, never a false positive.
                continue;
            }
            if COMMAND_WRAPPERS.contains(&token) || token.contains('=') {
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
        if tmux_seen && is_control_flag(token) {
            return true;
        }
    }
    false
}

pub(crate) enum TmuxFeed {
    /// Bytes to forward through the regular output pipeline.
    Passthrough(Vec<u8>),
    /// A complete startup response and session notification were confirmed.
    ///
    /// `startup` preserves the full candidate for replay on probe failure.
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

#[derive(Default)]
pub(crate) struct TmuxControlDetector {
    /// The next byte begins a new line.
    line_start: bool,
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
    /// Set when a `tmux -CC` command line was just submitted; notifications
    /// only count while this is armed and fresh.
    armed_at: Option<Instant>,
    /// Short window armed by a submitted-but-invisible command line — see
    /// [`BLIND_ARM_WINDOW`]. Set only when editing escapes preceded Enter.
    blind_arm_at: Option<Instant>,
    /// An escape sequence was consumed since the current input line began —
    /// evidence of a recall/editing gesture that gates the blind arm.
    input_saw_escape: bool,
}

impl TmuxControlDetector {
    pub(crate) fn new() -> Self {
        Self {
            line_start: true,
            candidate: None,
            done: false,
            startup: Startup::default(),
            startup_offset: 0,
            candidate_deadline: None,
            input_line: Vec::new(),
            input_track: InputTrack::Normal,
            armed_at: None,
            blind_arm_at: None,
            input_saw_escape: false,
        }
    }

    /// Pre-arm detection for a configured startup/post-login command that
    /// already matched `tmux -CC`.
    pub(crate) fn arm(&mut self) {
        self.armed_at = Some(Instant::now());
    }

    fn is_armed(&self) -> bool {
        self.armed_at
            .is_some_and(|armed_at| armed_at.elapsed() <= ARM_WINDOW)
            || self
                .blind_arm_at
                .is_some_and(|armed_at| armed_at.elapsed() <= BLIND_ARM_WINDOW)
    }

    /// Track bytes written to the channel (user input, AI capture commands)
    /// so a `tmux -CC` command line arms output detection.
    pub(crate) fn feed_input(&mut self, data: &[u8]) {
        if self.done {
            return;
        }
        for &byte in data {
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
                    let line = std::mem::take(&mut self.input_line);
                    let saw_escape = std::mem::take(&mut self.input_saw_escape);
                    if line.iter().all(|b| b.is_ascii_whitespace()) {
                        // The submit carried no visible text — a recalled
                        // command is only visible to the shell. Arm the
                        // short blind window only when an editing escape
                        // (arrow key, etc.) actually preceded the Enter.
                        if saw_escape {
                            self.blind_arm_at = Some(Instant::now());
                        }
                    } else {
                        self.blind_arm_at = None;
                        self.evaluate_input_line(&line);
                    }
                }
                0x12 | 0x10 | 0x0e => {
                    self.input_saw_escape = true;
                    self.input_line.clear();
                }
                0x7f | 0x08 => {
                    self.input_line.pop();
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
                    }
                }
            }
        }
    }

    fn evaluate_input_line(&mut self, line: &[u8]) {
        match std::str::from_utf8(line) {
            Ok(text) if is_tmux_control_command(text) => {
                self.armed_at = Some(Instant::now());
            }
            // A different submitted command resolves the `tmux -CC` attempt
            // either way — close the window rather than staying primed.
            _ if line.iter().any(|b| !b.is_ascii_whitespace()) => {
                self.armed_at = None;
            }
            _ => {}
        }
    }

    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.candidate_deadline
    }

    /// Called by the SSH select loop even if the peer sends no more bytes.
    pub(crate) fn expire_candidate(&mut self) -> Vec<u8> {
        self.armed_at = None;
        self.blind_arm_at = None;
        self.take_candidate()
    }

    fn take_candidate(&mut self) -> Vec<u8> {
        self.candidate_deadline = None;
        self.startup = Startup::default();
        self.startup_offset = 0;
        let bytes = self.candidate.take().unwrap_or_default();
        self.line_start = bytes.last() == Some(&b'\n');
        bytes
    }

    pub(crate) fn feed(&mut self, data: &[u8]) -> TmuxFeed {
        if self.done {
            return TmuxFeed::Passthrough(data.to_vec());
        }
        let mut out = Vec::new();
        if self
            .candidate_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            out.extend(self.expire_candidate());
        }
        if !self.is_armed() {
            if self.candidate.is_some() {
                out.extend(self.take_candidate());
            }
            out.extend_from_slice(data);
            if let Some(last) = data.last() {
                self.line_start = *last == b'\n';
            }
            return TmuxFeed::Passthrough(out);
        }
        for (i, &byte) in data.iter().enumerate() {
            if self.candidate.is_none() {
                if byte == 0x1b || (self.line_start && byte == b'%') {
                    self.candidate = Some(Vec::new());
                    self.candidate_deadline = Some(Instant::now() + CONFIRM_TIMEOUT);
                } else {
                    out.push(byte);
                    self.line_start = byte == b'\n';
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
                    out.extend(self.take_candidate());
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
    use super::{TmuxControlDetector, TmuxFeed, is_tmux_control_command};
    const STARTUP: &[u8] = b"%begin 1 2 0\r\n%end 1 2 0\r\n%window-add @0\r\n%sessions-changed\r\n%session-changed $0 0\r\n";

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
        detector.arm();
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
            detector.arm();
            assert!(matches!(detector.feed(bytes), TmuxFeed::Passthrough(data) if data == bytes));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn candidate_timeout_replays_raw_bytes_without_new_data() {
        let mut detector = TmuxControlDetector::new();
        detector.arm();
        let bytes = b"\x1bP1000p%begin 1 2 0\r\n\xffincomplete";
        assert!(matches!(detector.feed(bytes), TmuxFeed::Passthrough(data) if data.is_empty()));
        tokio::time::sleep_until(detector.deadline().unwrap()).await;
        assert_eq!(detector.expire_candidate(), bytes);
        assert!(detector.deadline().is_none());
    }

    #[test]
    fn startup_buffer_is_bounded_and_preserves_overflow() {
        let mut detector = TmuxControlDetector::new();
        detector.arm();
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
}
