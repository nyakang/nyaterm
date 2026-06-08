//! Shared OSC parsing, shell detection types, and injection script generation.
//!
//! Used by both SSH (`core::ssh::io`) and local PTY (`core::pty`) to avoid duplication.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;

/// Remote shell flavour detected via exec channel or local shell path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    Bash,
    Zsh,
    Fish,
    PosixSh,
    Unknown,
}

impl ShellKind {
    /// Classify a shell name / path string (case-insensitive).
    pub fn from_name(name: &str) -> Self {
        let s = name.to_ascii_lowercase();
        if s.contains("fish") {
            Self::Fish
        } else if s.contains("zsh") {
            Self::Zsh
        } else if s.contains("bash") {
            Self::Bash
        } else if s.contains("sh") {
            Self::PosixSh
        } else {
            Self::Unknown
        }
    }
}

// ---------------------------------------------------------------------------
// Ready marker
// ---------------------------------------------------------------------------

const READY_MARKER_PREFIX: &str = "7777;NyaTermReady:";
const COMMAND_MARKER_PREFIX: &str = "7777;NyaTermCommand:";
const LEGACY_READY_MARKER_PREFIX: &str = "7777;DflyReady:";
const LEGACY_COMMAND_MARKER_PREFIX: &str = "7777;DflyCommand:";

/// Build a session-unique ready marker: `\x1b]7777;NyaTermReady:<id>\x07`.
pub fn build_ready_marker(session_id: &str) -> String {
    format!("\x1b]{}{}\x07", READY_MARKER_PREFIX, session_id)
}

// ---------------------------------------------------------------------------
// Injection scripts (per shell)
// ---------------------------------------------------------------------------

/// Generate the shell-specific injection script that installs an OSC 7 hook
/// and emits the ready marker.  Returns `None` for shells we cannot inject
/// (plain POSIX sh, unknown).
pub fn injection_script(shell: ShellKind, ready_marker: &str) -> Option<String> {
    let ready_osc = ready_marker
        .replace('\x1b', "\\033")
        .replace('\x07', "\\007");

    match shell {
        ShellKind::Bash => Some(format!(
            concat!(
                " NYATERM_PRUNE_HISTORY=1;",
                " if [ -z \"${{NYATERM_INJ:-}}\" ]; then export NYATERM_INJ=1;",
                " NYATERM_LAST_HISTCMD=\"${{HISTCMD-}}\";",
                " __nyaterm_host(){{ hostname 2>/dev/null || printf localhost; }};",
                " __nyaterm_emit_command(){{",
                " local histcmd=\"${{HISTCMD-}}\";",
                " if [ -n \"$histcmd\" ] && [ \"${{NYATERM_LAST_HISTCMD-}}\" != \"$histcmd\" ]; then",
                " NYATERM_LAST_HISTCMD=\"$histcmd\";",
                " local cmd; cmd=\"$(fc -ln -1 2>/dev/null)\";",
                " if [ -n \"$cmd\" ] && command -v base64 >/dev/null 2>&1; then",
                " local b64; b64=\"$(printf '%s' \"$cmd\" | base64 | tr -d '\\r\\n')\";",
                " printf '\\033]7777;NyaTermCommand:%s\\007' \"$b64\";",
                " fi;",
                " fi;",
                " }};",
                " __nyaterm_prompt(){{",
                " if [ -n \"${{NYATERM_PRUNE_HISTORY:-}}\" ]; then",
                " history -d $((HISTCMD-1)) 2>/dev/null || true;",
                " unset NYATERM_PRUNE_HISTORY;",
                " NYATERM_LAST_HISTCMD=\"${{HISTCMD-}}\";",
                " fi;",
                " __nyaterm_emit_command;",
                " printf '\\033]7;file://%s%s\\007' \"$(__nyaterm_host)\" \"$PWD\";",
                " }};",
                " case \"${{PROMPT_COMMAND-}}\" in (*__nyaterm_prompt*) ;; (*)",
                " PROMPT_COMMAND=\"__nyaterm_prompt${{PROMPT_COMMAND:+; $PROMPT_COMMAND}}\" ;; esac;",
                " fi;",
                " printf '{}' 2>/dev/null\n",
            ),
            ready_osc,
        )),

        ShellKind::Zsh => Some(format!(
            concat!(
                " fc -p /dev/null 2>/dev/null\n",
                " if [ -z \"${{NYATERM_INJ:-}}\" ]; then export NYATERM_INJ=1;",
                " __nyaterm_host(){{ hostname 2>/dev/null || printf localhost; }};",
                " __nyaterm_emit(){{ printf '\\033]7;file://%s%s\\007' \"$(__nyaterm_host)\" \"$PWD\"; }};",
                " __nyaterm_preexec(){{",
                " if [ -n \"$1\" ] && command -v base64 >/dev/null 2>&1; then",
                " local b64; b64=\"$(printf '%s' \"$1\" | base64 | tr -d '\\r\\n')\";",
                " printf '\\033]7777;NyaTermCommand:%s\\007' \"$b64\";",
                " fi;",
                " }};",
                " autoload -Uz add-zsh-hook 2>/dev/null || true;",
                " typeset -ga precmd_functions preexec_functions;",
                " [[ \" ${{precmd_functions[*]}} \" == *\" __nyaterm_emit \"* ]] || precmd_functions+=(__nyaterm_emit);",
                " [[ \" ${{preexec_functions[*]}} \" == *\" __nyaterm_preexec \"* ]] || preexec_functions+=(__nyaterm_preexec);",
                " fi;",
                " fc -P 2>/dev/null\n",
                " printf '{}' 2>/dev/null\n",
            ),
            ready_osc,
        )),

        ShellKind::Fish => Some(format!(
            concat!(
                " set fish_private_mode 1 2>/dev/null\n",
                " if not set -q NYATERM_INJ;",
                " set -gx NYATERM_INJ 1;",
                " function __nyaterm_emit --on-event fish_prompt;",
                " printf '\\033]7;file://%s%s\\007' (hostname) $PWD;",
                " end;",
                " function __nyaterm_preexec --on-event fish_preexec;",
                " if test -n \"$argv[1]\"; and command -sq base64;",
                " set -l b64 (printf '%s' \"$argv[1]\" | base64 | tr -d '\\r\\n');",
                " if test -n \"$b64\";",
                " printf '\\033]7777;NyaTermCommand:%s\\007' \"$b64\";",
                " end;",
                " end;",
                " end;",
                " end;",
                " set -e fish_private_mode 2>/dev/null\n",
                " printf '{}' 2>/dev/null\n",
            ),
            ready_osc,
        )),

        ShellKind::PosixSh | ShellKind::Unknown => None,
    }
}

// ---------------------------------------------------------------------------
// Streaming OSC stripper
// ---------------------------------------------------------------------------

const MAX_OSC_BUF: usize = 64 * 1024;

/// Result returned by [`OscStripper::push`].
pub struct OscResult {
    /// Text safe to display in the terminal (all recognised OSC sequences removed).
    pub visible: String,
    /// Visible text that appeared after the ready marker in this chunk.
    pub visible_after_ready: String,
    /// CWD paths extracted from OSC 7 sequences in this chunk.
    pub cwd_paths: Vec<String>,
    /// Whether the ready marker was detected in this chunk.
    pub ready: bool,
    /// Shell-confirmed commands extracted from private NyaTerm OSC markers.
    pub accepted_commands: Vec<String>,
}

/// Streaming parser that strips OSC 7 and NyaTermReady sequences from terminal
/// output, handling split packets and extracting CWD paths.
pub struct OscStripper {
    buf: String,
    ready_inner: String,
    legacy_ready_inner: Option<String>,
}

impl OscStripper {
    pub fn new(ready_marker: &str) -> Self {
        let ready_inner = marker_inner(ready_marker);
        let legacy_ready_inner = ready_inner
            .strip_prefix(READY_MARKER_PREFIX)
            .map(|session_id| format!("{LEGACY_READY_MARKER_PREFIX}{session_id}"));

        Self {
            buf: String::new(),
            ready_inner,
            legacy_ready_inner,
        }
    }

    /// Feed a chunk of terminal output.  Returns visible text with OSC
    /// sequences stripped, any CWD paths found, and whether the ready
    /// marker appeared.
    pub fn push(&mut self, chunk: &str) -> OscResult {
        self.buf.push_str(chunk);

        // Safety valve: if the buffer is enormous without any ESC, just
        // flush everything as visible to avoid unbounded memory growth.
        if self.buf.len() > MAX_OSC_BUF && !self.buf.contains('\x1b') {
            return OscResult {
                visible: std::mem::take(&mut self.buf),
                visible_after_ready: String::new(),
                cwd_paths: Vec::new(),
                ready: false,
                accepted_commands: Vec::new(),
            };
        }

        let mut visible = String::new();
        let mut visible_after_ready = String::new();
        let mut paths = Vec::new();
        let mut ready = false;
        let mut after_ready = false;
        let mut commands = Vec::new();

        loop {
            let esc_pos = match self.buf.find("\x1b]") {
                Some(i) => i,
                None => {
                    // No ESC] left — everything is visible text.
                    if after_ready {
                        visible_after_ready.push_str(&self.buf);
                    }
                    visible.push_str(&self.buf);
                    self.buf.clear();
                    break;
                }
            };

            // Text before the ESC is always visible.
            if after_ready {
                visible_after_ready.push_str(&self.buf[..esc_pos]);
            }
            visible.push_str(&self.buf[..esc_pos]);
            let rest = self.buf[esc_pos..].to_string();

            // Find the terminator: BEL (\x07) or ST (\x1b\\).
            let end = rest.find('\x07').map(|i| (i, 1)).or_else(|| {
                // Make sure we don't match the opening \x1b] as \x1b\\.
                rest[2..].find("\x1b\\").map(|i| (i + 2, 2))
            });

            let Some((end_idx, term_len)) = end else {
                // Incomplete sequence — keep in buffer for next chunk.
                self.buf = rest;

                // But if buffer is already huge, give up and flush.
                if self.buf.len() > MAX_OSC_BUF {
                    visible.push_str(&self.buf);
                    self.buf.clear();
                }
                break;
            };

            let seq = &rest[..end_idx + term_len];
            let inner = &rest[2..end_idx]; // between \x1b] and terminator

            if inner.starts_with("7;") {
                // OSC 7 — extract CWD path.
                if let Some(path) = parse_osc7_payload(&inner[2..]) {
                    paths.push(path);
                }
            } else if self.is_current_ready_marker(inner) {
                ready = true;
                after_ready = true;
            } else if inner.starts_with(READY_MARKER_PREFIX)
                || inner.starts_with(LEGACY_READY_MARKER_PREFIX)
            {
                // Private marker for another session; strip it without
                // treating this session as ready.
            } else if let Some(command) = inner
                .strip_prefix(COMMAND_MARKER_PREFIX)
                .or_else(|| inner.strip_prefix(LEGACY_COMMAND_MARKER_PREFIX))
                .and_then(parse_command_payload)
            {
                commands.push(command);
            } else {
                // Not ours — pass through to the terminal.
                if after_ready {
                    visible_after_ready.push_str(seq);
                }
                visible.push_str(seq);
            }

            self.buf = rest[end_idx + term_len..].to_string();
        }

        OscResult {
            visible,
            visible_after_ready,
            cwd_paths: paths,
            ready,
            accepted_commands: commands,
        }
    }

    /// Drain any buffered bytes as visible text (used on timeout / teardown).
    pub fn flush(&mut self) -> String {
        std::mem::take(&mut self.buf)
    }

    fn is_current_ready_marker(&self, inner: &str) -> bool {
        inner == self.ready_inner || self.legacy_ready_inner.as_deref() == Some(inner)
    }
}

fn marker_inner(marker: &str) -> String {
    let Some(rest) = marker.strip_prefix("\x1b]") else {
        return marker.to_string();
    };

    if let Some(inner) = rest.strip_suffix('\x07') {
        inner.to_string()
    } else if let Some(inner) = rest.strip_suffix("\x1b\\") {
        inner.to_string()
    } else {
        rest.to_string()
    }
}

/// Parse the payload of an OSC 7 sequence (`file://host/path`).
fn parse_osc7_payload(payload: &str) -> Option<String> {
    let after_scheme = payload.strip_prefix("file://")?;
    let path = if after_scheme.starts_with('/') {
        after_scheme.to_string()
    } else {
        let slash = after_scheme.find('/')?;
        after_scheme[slash..].to_string()
    };
    if path.is_empty() { None } else { Some(path) }
}

fn parse_command_payload(payload: &str) -> Option<String> {
    let decoded = BASE64_STANDARD.decode(payload).ok()?;
    let command = String::from_utf8(decoded).ok()?;
    if command.is_empty() {
        None
    } else {
        Some(command)
    }
}

#[cfg(test)]
mod tests {
    use super::{OscStripper, ShellKind, build_ready_marker, injection_script};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;

    #[test]
    fn bash_injection_prunes_its_history_entry() {
        let script = injection_script(ShellKind::Bash, &build_ready_marker("session-1"))
            .expect("bash injection script");

        assert!(script.contains("NYATERM_PRUNE_HISTORY=1;"));
        assert!(script.contains("history -d $((HISTCMD-1)) 2>/dev/null || true;"));
        assert!(
            script.contains(
                "PROMPT_COMMAND=\"__nyaterm_prompt${PROMPT_COMMAND:+; $PROMPT_COMMAND}\""
            )
        );
        assert!(!script.contains("set +o history"));
        assert!(!script.contains("set -o history"));
    }

    #[test]
    fn interactive_shell_ready_marker_is_emitted_after_cleanup() {
        let ready_marker = build_ready_marker("session-1");
        let ready_pos = |script: &str| script.find("NyaTermReady:session-1").expect("ready marker");

        let zsh = injection_script(ShellKind::Zsh, &ready_marker).expect("zsh injection script");
        assert!(
            zsh.find(" fc -P 2>/dev/null\n")
                .expect("zsh history restore")
                < ready_pos(&zsh)
        );

        let fish = injection_script(ShellKind::Fish, &ready_marker).expect("fish injection script");
        assert!(
            fish.find(" set -e fish_private_mode 2>/dev/null\n")
                .expect("fish private mode cleanup")
                < ready_pos(&fish)
        );

        assert!(zsh.contains("NyaTermCommand:%s"));
        assert!(fish.contains("NyaTermCommand:%s"));
    }

    #[test]
    fn strips_private_command_osc_without_leaking_visible_text() {
        let command = BASE64_STANDARD.encode("docker ps");
        let payload = format!(
            "before\x1b]7777;NyaTermCommand:{command}\x07after\x1b]7777;NyaTermReady:session-1\x07"
        );

        let result = OscStripper::new(&build_ready_marker("session-1")).push(&payload);
        assert_eq!(result.visible, "beforeafter");
        assert_eq!(result.accepted_commands, vec!["docker ps".to_string()]);
        assert!(result.ready);
    }

    #[test]
    fn ready_marker_with_prompt_in_same_chunk_preserves_prompt_after_ready() {
        let payload = "echoed injection\x1b]7777;NyaTermReady:session-1\x07[user@host ~]$ ";

        let result = OscStripper::new(&build_ready_marker("session-1")).push(payload);

        assert!(result.ready);
        assert_eq!(result.visible, "echoed injection[user@host ~]$ ");
        assert_eq!(result.visible_after_ready, "[user@host ~]$ ");
    }

    #[test]
    fn ready_marker_for_other_session_does_not_mark_ready() {
        let payload = "before\x1b]7777;NyaTermReady:session-2\x07after";

        let result = OscStripper::new(&build_ready_marker("session-1")).push(payload);

        assert!(!result.ready);
        assert_eq!(result.visible, "beforeafter");
        assert!(result.visible_after_ready.is_empty());
    }

    #[test]
    fn legacy_ready_marker_must_match_current_session() {
        let mut stripper = OscStripper::new(&build_ready_marker("session-1"));

        let other = stripper.push("x\x1b]7777;DflyReady:session-2\x07y");
        assert!(!other.ready);
        assert_eq!(other.visible, "xy");

        let current = stripper.push("x\x1b]7777;DflyReady:session-1\x07y");
        assert!(current.ready);
        assert_eq!(current.visible_after_ready, "y");
    }

    #[test]
    fn parses_split_command_markers_across_chunks() {
        let command = BASE64_STANDARD.encode("kubectl get pods");
        let mut stripper = OscStripper::new(&build_ready_marker("session-1"));

        let first = stripper.push(&format!("x\x1b]7777;NyaTermCommand:{}", &command[..8]));
        assert_eq!(first.visible, "x");
        assert!(first.accepted_commands.is_empty());

        let second = stripper.push(&format!("{}\x07y", &command[8..]));
        assert_eq!(second.visible, "y");
        assert_eq!(
            second.accepted_commands,
            vec!["kubectl get pods".to_string()]
        );
    }

    #[test]
    fn accepts_legacy_private_command_markers() {
        let command = BASE64_STANDARD.encode("docker ps");
        let payload = format!(
            "before\x1b]7777;DflyCommand:{command}\x07after\x1b]7777;DflyReady:session-1\x07"
        );

        let result = OscStripper::new(&build_ready_marker("session-1")).push(&payload);
        assert_eq!(result.visible, "beforeafter");
        assert_eq!(result.accepted_commands, vec!["docker ps".to_string()]);
        assert!(result.ready);
    }
}
