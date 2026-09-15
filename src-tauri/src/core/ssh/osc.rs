//! Shared OSC parsing, shell detection types, and injection script generation.
//!
//! Used by both SSH (`core::ssh::io`) and local PTY (`core::terminal_session::local`) to avoid duplication.

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
const READY_FAILED_MARKER_PREFIX: &str = "7777;NyaTermReadyFailed:";
const COMMAND_MARKER_PREFIX: &str = "7777;NyaTermCommand:";
const LEGACY_READY_MARKER_PREFIX: &str = "7777;DflyReady:";
const LEGACY_COMMAND_MARKER_PREFIX: &str = "7777;DflyCommand:";
const BASH_DIRECT_INJECTION_MAX_BYTES: usize = 3 * 1024;

/// Build a session-unique ready marker: `\x1b]7777;NyaTermReady:<id>\x07`.
pub fn build_ready_marker(session_id: &str) -> String {
    format!("\x1b]{}{}\x07", READY_MARKER_PREFIX, session_id)
}

/// Build the session-bound inner marker used for shell command confirmation.
/// The marker intentionally contains no escape terminator; shell scripts add
/// the OSC framing around the base64 payload.
pub fn build_command_marker(session_id: &str) -> String {
    format!("{COMMAND_MARKER_PREFIX}{session_id}:")
}

fn command_marker_for_ready(ready_marker: &str) -> String {
    let inner = marker_inner(ready_marker);
    inner
        .strip_prefix(READY_MARKER_PREFIX)
        .map(build_command_marker)
        .unwrap_or_else(|| COMMAND_MARKER_PREFIX.to_string())
}

fn ready_failed_marker_for_ready(ready_marker: &str) -> String {
    let inner = marker_inner(ready_marker);
    inner
        .strip_prefix(READY_MARKER_PREFIX)
        .map(|session_id| format!("\x1b]{READY_FAILED_MARKER_PREFIX}{session_id}\x07"))
        .unwrap_or_default()
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
    let ready_failed_osc = ready_failed_marker_for_ready(ready_marker)
        .replace('\x1b', "\\033")
        .replace('\x07', "\\007");
    let command_marker = command_marker_for_ready(ready_marker);

    match shell {
        ShellKind::Bash => {
            let script = format!(
                concat!(
                    // Keep the entire direct-injection command below the Linux
                    // N_TTY canonical queue limit. RcFile mode retains the full
                    // self-repairing prompt implementation from bash_persistent.sh.
                    " NYATERM_PRUNE_HISTORY=1;",
                    " NYATERM_READY_PENDING=1;",
                    " NYATERM_SKIP_COMMAND_ONCE=1;",
                    " export NYATERM_INJ=1;",
                    " NYATERM_COMMAND_MARKER=\"{command_marker}\";",
                    " NYATERM_READY_FAILED_MARKER=\"$(printf '{ready_failed_osc}')\";",
                    " NYATERM_LAST_HISTCMD=\"${{HISTCMD-}}\";",
                    " __nyaterm_host(){{ hostname 2>/dev/null || printf localhost; }};",
                    " __nyaterm_ready_failed(){{ [ -n \"${{__nyaterm_failure_reported:-}}\" ] || {{ __nyaterm_failure_reported=1; printf '%s' \"${{NYATERM_READY_FAILED_MARKER-}}\"; }}; }};",
                    " __nyaterm_prune_history(){{ [ -n \"${{NYATERM_PRUNE_HISTORY:-}}\" ] || return 0; unset NYATERM_PRUNE_HISTORY; local hline history_number; hline=\"$(HISTTIMEFORMAT= history 1 2>/dev/null || true)\"; case \"$hline\" in *NYATERM_PRUNE_HISTORY*|*NYATERM_INJ*|*__nyaterm_prompt*|*NyaTermReady*) history_number=${{hline#\"${{hline%%[![:space:]]*}}\"}}; history_number=${{history_number%%[!0-9]*}}; [ -z \"$history_number\" ] || history -d \"$history_number\" 2>/dev/null || true;; esac; NYATERM_LAST_HISTCMD=\"${{HISTCMD-}}\"; }};",
                    " __nyaterm_emit_command(){{ local histcmd=\"${{HISTCMD-}}\"; if [ -n \"${{NYATERM_SKIP_COMMAND_ONCE:-}}\" ]; then unset NYATERM_SKIP_COMMAND_ONCE; NYATERM_LAST_HISTCMD=\"$histcmd\"; return 0; fi; if [ -n \"$histcmd\" ] && [ \"${{NYATERM_LAST_HISTCMD-}}\" != \"$histcmd\" ]; then NYATERM_LAST_HISTCMD=\"$histcmd\"; local cmd; cmd=\"$(fc -ln -1 2>/dev/null)\"; if [ -n \"$cmd\" ] && command -v base64 >/dev/null 2>&1; then local b64; b64=\"$(printf '%s' \"$cmd\" | base64 | tr -d '\\r\\n')\"; printf '\\033]%s%s\\007' \"$NYATERM_COMMAND_MARKER\" \"$b64\"; fi; fi; }};",
                    " __nyaterm_prompt(){{ local status=$?; __nyaterm_prune_history; __nyaterm_emit_command; local cwd=\"${{PWD//%/%25}}\"; printf '\\033]7;file://%s%s\\007' \"$(__nyaterm_host)\" \"$cwd\"; return \"$status\"; }};",
                    " __nyaterm_install_prompt(){{ local decl f; decl=\"$(declare -p PROMPT_COMMAND 2>/dev/null || true)\"; [[ ! \"$decl\" =~ ^declare\\ -[^[:space:]]*r ]] || return 1; if [[ \"$decl\" =~ ^declare\\ -[^[:space:]]*a[^[:space:]]*\\ PROMPT_COMMAND= ]]; then for f in \"${{PROMPT_COMMAND[@]}}\"; do [ \"$f\" = __nyaterm_prompt ] && return 0; done; PROMPT_COMMAND=(__nyaterm_prompt \"${{PROMPT_COMMAND[@]}}\") || return 1; else case \"${{PROMPT_COMMAND-}}\" in *__nyaterm_prompt*) ;; *) PROMPT_COMMAND=\"__nyaterm_prompt${{PROMPT_COMMAND:+; $PROMPT_COMMAND}}\" || return 1;; esac; fi; }};",
                    " if __nyaterm_install_prompt; then __nyaterm_install_ok=1; else __nyaterm_install_ok=0; fi;",
                    " __nyaterm_prune_history;",
                    " if [ \"$__nyaterm_install_ok\" = 1 ]; then if [ -n \"${{NYATERM_READY_PENDING:-}}\" ]; then unset NYATERM_READY_PENDING; printf '{ready_osc}'; fi; else unset NYATERM_READY_PENDING; __nyaterm_ready_failed; fi;",
                    " unset __nyaterm_install_ok\n",
                ),
                command_marker = command_marker,
                ready_failed_osc = ready_failed_osc,
                ready_osc = ready_osc,
            );
            (script.len() <= BASH_DIRECT_INJECTION_MAX_BYTES).then_some(script)
        }

        ShellKind::Zsh => Some(format!(
            concat!(
                " fc -p /dev/null 2>/dev/null\n",
                " NYATERM_READY_PENDING=1;",
                " export NYATERM_INJ=1;",
                " NYATERM_COMMAND_MARKER=\"{}\";",
                " NYATERM_READY_FAILED_MARKER=\"$(printf '{}')\";",
                " __nyaterm_host(){{ hostname 2>/dev/null || printf localhost; }};",
                " __nyaterm_ready_failed(){{ [ -n \"${{__nyaterm_failure_reported:-}}\" ] || {{ __nyaterm_failure_reported=1; printf '%s' \"${{NYATERM_READY_FAILED_MARKER-}}\"; }}; }};",
                " __nyaterm_emit(){{",
                " builtin typeset saved_status=$?; builtin typeset cwd=\"${{PWD//\\%/%25}}\"; printf '\\033]7;file://%s%s\\007' \"$(__nyaterm_host)\" \"$cwd\"; return \"$saved_status\";",
                " }};",
                " __nyaterm_preexec(){{",
                " builtin typeset saved_status=$?; if [ -n \"$1\" ]; then",
                " if command -v base64 >/dev/null 2>&1; then",
                " builtin typeset b64; b64=\"$(printf '%s' \"$1\" | base64 | tr -d '\\r\\n')\";",
                " printf '\\033]%s%s\\007' \"$NYATERM_COMMAND_MARKER\" \"$b64\";",
                " fi;",
                " fi; return \"$saved_status\";",
                " }};",
                " __nyaterm_install_prompt(){{",
                " [[ ${{parameters[precmd_functions]-}} == *readonly* ]] && return 1;",
                " [[ ${{parameters[preexec_functions]-}} == *readonly* ]] && return 1;",
                " builtin typeset -ga precmd_functions preexec_functions || return 1;",
                " builtin typeset -a retained || return 1; builtin typeset f || return 1; retained=(); for f in \"${{precmd_functions[@]}}\"; do case \"$f\" in (__nyaterm_emit|__nyaterm_repair_prompt) ;; (*) retained+=(\"$f\");; esac; done;",
                " precmd_functions=(\"${{retained[@]}}\" __nyaterm_emit) || return 1;",
                " retained=(); for f in \"${{preexec_functions[@]}}\"; do [ \"$f\" = __nyaterm_preexec ] || retained+=(\"$f\"); done;",
                " preexec_functions=(\"${{retained[@]}}\" __nyaterm_preexec) || return 1; return 0;",
                " }};",
                " fc -P 2>/dev/null\n",
                " if __nyaterm_install_prompt; then if [ -n \"${{NYATERM_READY_PENDING:-}}\" ]; then unset NYATERM_READY_PENDING; printf '{}'; fi; else unset NYATERM_READY_PENDING; __nyaterm_ready_failed; fi\n",
            ),
            command_marker, ready_failed_osc, ready_osc,
        )),

        ShellKind::Fish => Some(format!(
            concat!(
                " set fish_private_mode 1 2>/dev/null\n",
                " set -g NYATERM_READY_PENDING 1;",
                " set -gx NYATERM_INJ 1;",
                " set -g NYATERM_COMMAND_MARKER \"{}\";",
                " set -g NYATERM_READY_FAILED_MARKER (printf '{}');",
                " function __nyaterm_emit --on-event fish_prompt;",
                " set -l saved_status $status; set -l cwd (string replace -a '%' '%25' -- $PWD); printf '\\033]7;file://%s%s\\007' (hostname) $cwd; return $saved_status;",
                " end;",
                " function __nyaterm_preexec --on-event fish_preexec;",
                " set -l saved_status $status; if test -n \"$argv[1]\";",
                " if command -sq base64;",
                " set -l b64 (printf '%s' \"$argv[1]\" | base64 | tr -d '\\r\\n');",
                " if test -n \"$b64\";",
                " printf '\\033]%s%s\\007' \"$NYATERM_COMMAND_MARKER\" \"$b64\";",
                " end;",
                " end;",
                " end; return $saved_status;",
                " end;",
                " set -e fish_private_mode 2>/dev/null\n",
                " if functions -q __nyaterm_emit; and functions -q __nyaterm_preexec;",
                " set -e NYATERM_READY_PENDING; printf '{}';",
                " else; set -e NYATERM_READY_PENDING; printf '%s' \"$NYATERM_READY_FAILED_MARKER\"; end\n",
            ),
            command_marker, ready_failed_osc, ready_osc,
        )),

        ShellKind::PosixSh | ShellKind::Unknown => None,
    }
}

pub fn activation_script(shell: ShellKind, ready_marker: &str) -> Option<String> {
    let ready_printf = ready_marker
        .replace('\\', "\\\\")
        .replace('\x1b', "\\033")
        .replace('\x07', "\\007")
        .replace('\'', "'\\''");
    let ready_failed_printf = ready_failed_marker_for_ready(ready_marker)
        .replace('\\', "\\\\")
        .replace('\x1b', "\\033")
        .replace('\x07', "\\007")
        .replace('\'', "'\\''");
    let command_marker = command_marker_for_ready(ready_marker);

    match shell {
        ShellKind::Bash => Some(format!(
            " NYATERM_PRUNE_HISTORY=1; NYATERM_READY_PENDING=1; export NYATERM_INJ=1; export NYATERM_READY_MARKER=\"$(printf '{}')\"; export NYATERM_READY_FAILED_MARKER=\"$(printf '{}')\"; export NYATERM_COMMAND_MARKER=\"{}\"; if [ -r \"$HOME/.config/nyaterm/shell-integration.bash\" ] && . \"$HOME/.config/nyaterm/shell-integration.bash\" && __nyaterm_install_prompt 2>/dev/null; then unset NYATERM_READY_PENDING; printf '%s' \"${{NYATERM_READY_MARKER-}}\"; else unset NYATERM_READY_PENDING; printf '%s' \"${{NYATERM_READY_FAILED_MARKER-}}\"; fi\n",
            ready_printf, ready_failed_printf, command_marker
        )),
        ShellKind::Zsh => Some(format!(
            " fc -p /dev/null 2>/dev/null\n NYATERM_READY_PENDING=1; export NYATERM_INJ=1; export NYATERM_READY_MARKER=\"$(printf '{}')\"; export NYATERM_READY_FAILED_MARKER=\"$(printf '{}')\"; export NYATERM_COMMAND_MARKER=\"{}\"; if [ -r \"$HOME/.config/nyaterm/shell-integration.zsh\" ] && . \"$HOME/.config/nyaterm/shell-integration.zsh\" && __nyaterm_install_prompt 2>/dev/null; then __nyaterm_install_ok=1; else __nyaterm_install_ok=0; fi; fc -P 2>/dev/null\n if [ \"$__nyaterm_install_ok\" = 1 ]; then unset NYATERM_READY_PENDING; printf '%s' \"${{NYATERM_READY_MARKER-}}\"; else unset NYATERM_READY_PENDING; printf '%s' \"${{NYATERM_READY_FAILED_MARKER-}}\"; fi; unset __nyaterm_install_ok\n",
            ready_printf, ready_failed_printf, command_marker
        )),
        ShellKind::Fish => Some(format!(
            " set fish_private_mode 1 2>/dev/null\n set -g NYATERM_READY_PENDING 1; set -gx NYATERM_INJ 1; set -gx NYATERM_READY_MARKER (printf '{}'); set -gx NYATERM_READY_FAILED_MARKER (printf '{}'); set -gx NYATERM_COMMAND_MARKER \"{}\"; if test -r \"$HOME/.config/nyaterm/shell-integration.fish\"; and source \"$HOME/.config/nyaterm/shell-integration.fish\"; and __nyaterm_install_prompt 2>/dev/null; set -e NYATERM_READY_PENDING; printf '%s' \"$NYATERM_READY_MARKER\"; else; set -e NYATERM_READY_PENDING; printf '%s' \"$NYATERM_READY_FAILED_MARKER\"; end; set -e fish_private_mode 2>/dev/null\n",
            ready_printf, ready_failed_printf, command_marker
        )),
        ShellKind::PosixSh | ShellKind::Unknown => None,
    }
}

pub fn persistent_script(shell: ShellKind) -> Option<&'static str> {
    match shell {
        ShellKind::Bash => Some(BASH_PERSISTENT_SCRIPT),
        ShellKind::Zsh => Some(ZSH_PERSISTENT_SCRIPT),
        ShellKind::Fish => Some(FISH_PERSISTENT_SCRIPT),
        ShellKind::PosixSh | ShellKind::Unknown => None,
    }
}

pub fn persistent_script_path(shell: ShellKind) -> Option<&'static str> {
    match shell {
        ShellKind::Bash => Some("$HOME/.config/nyaterm/shell-integration.bash"),
        ShellKind::Zsh => Some("$HOME/.config/nyaterm/shell-integration.zsh"),
        ShellKind::Fish => Some("$HOME/.config/nyaterm/shell-integration.fish"),
        ShellKind::PosixSh | ShellKind::Unknown => None,
    }
}

pub fn rc_file_path(shell: ShellKind) -> Option<&'static str> {
    match shell {
        ShellKind::Bash => Some("$HOME/.bashrc"),
        ShellKind::Zsh => Some("$HOME/.zshrc"),
        ShellKind::Fish => Some("$HOME/.config/fish/conf.d/nyaterm-shell-integration.fish"),
        ShellKind::PosixSh | ShellKind::Unknown => None,
    }
}

pub fn rc_managed_block(shell: ShellKind) -> Option<String> {
    let source_path = persistent_script_path(shell)?;
    let body = match shell {
        ShellKind::Bash | ShellKind::Zsh => format!(
            "if [ -r \"{}\" ]; then\n  . \"{}\"\n  __nyaterm_install_cwd_prompt 2>/dev/null || true\nfi",
            source_path, source_path
        ),
        ShellKind::Fish => format!(
            "if test -r \"{}\"\n  source \"{}\"\n  __nyaterm_install_cwd_prompt 2>/dev/null; or true\nend",
            source_path, source_path
        ),
        ShellKind::PosixSh | ShellKind::Unknown => return None,
    };

    Some(format!(
        "{MANAGED_BLOCK_START}\n{body}\n{MANAGED_BLOCK_END}"
    ))
}

#[cfg(test)]
pub fn replace_managed_block(existing: &str, block: &str) -> String {
    let mut output = Vec::new();
    let mut lines = existing.lines();
    let mut replaced = false;

    while let Some(line) = lines.next() {
        if line == MANAGED_BLOCK_START {
            output.extend(block.lines().map(str::to_string));
            replaced = true;
            for skipped in lines.by_ref() {
                if skipped == MANAGED_BLOCK_END {
                    break;
                }
            }
        } else {
            output.push(line.to_string());
        }
    }

    if !replaced {
        if !output.is_empty() {
            output.push(String::new());
        }
        output.extend(block.lines().map(str::to_string));
    }

    let mut result = output.join("\n");
    result.push('\n');
    result
}

pub const MANAGED_BLOCK_START: &str = "# >>> nyaterm shell integration >>>";
pub const MANAGED_BLOCK_END: &str = "# <<< nyaterm shell integration <<<";

const BASH_PERSISTENT_SCRIPT: &str = include_str!("bash_persistent.sh");

const ZSH_PERSISTENT_SCRIPT: &str = concat!(
    "# nyaterm shell integration v1\n",
    "__nyaterm_host(){ hostname 2>/dev/null || printf localhost; }\n",
    "__nyaterm_ready_failed(){ [ -n \"${__nyaterm_failure_reported:-}\" ] || { __nyaterm_failure_reported=1; printf '%s' \"${NYATERM_READY_FAILED_MARKER-}\"; }; }\n",
    "__nyaterm_emit(){\n",
    "  builtin typeset saved_status=$?\n",
    "  if [ -n \"${NYATERM_READY_PENDING:-}\" ]; then unset NYATERM_READY_PENDING; printf '%s' \"${NYATERM_READY_MARKER-}\"; fi\n",
    "  builtin typeset cwd=\"${PWD//\\%/%25}\"\n",
    "  printf '\\033]7;file://%s%s\\007' \"$(__nyaterm_host)\" \"$cwd\"\n",
    "  return \"$saved_status\"\n",
    "}\n",
    "__nyaterm_cwd_prompt(){\n",
    "  builtin typeset saved_status=$?\n",
    "  builtin typeset cwd=\"${PWD//\\%/%25}\"\n",
    "  printf '\\033]7;file://%s%s\\007' \"$(__nyaterm_host)\" \"$cwd\"\n",
    "  return \"$saved_status\"\n",
    "}\n",
    "__nyaterm_install_cwd_prompt(){\n",
    "  [[ ${parameters[precmd_functions]-} == *readonly* ]] && return 1\n",
    "  builtin typeset -ga precmd_functions || return 1\n",
    "  builtin typeset f\n",
    "  for f in \"${precmd_functions[@]}\"; do\n",
    "    case \"$f\" in (__nyaterm_emit|__nyaterm_cwd_prompt) return 0;; esac\n",
    "  done\n",
    "  precmd_functions+=(__nyaterm_cwd_prompt) || return 1\n",
    "}\n",
    "__nyaterm_preexec(){\n",
    "  builtin typeset saved_status=$?\n",
    "  if [ -n \"$1\" ] && command -v base64 >/dev/null 2>&1; then\n",
    "    builtin typeset b64; b64=\"$(printf '%s' \"$1\" | base64 | tr -d '\\r\\n')\"\n",
    "    printf '\\033]7777;NyaTermCommand:%s\\007' \"$b64\"\n",
    "  fi\n",
    "  return \"$saved_status\"\n",
    "}\n",
    "__nyaterm_install_prompt(){\n",
    "  [[ ${parameters[precmd_functions]-} == *readonly* ]] && return 1\n",
    "  [[ ${parameters[preexec_functions]-} == *readonly* ]] && return 1\n",
    "  builtin typeset -ga precmd_functions preexec_functions || return 1\n",
    "  builtin typeset -a retained || return 1\n",
    "  builtin typeset f || return 1\n",
    "  retained=()\n",
    "  for f in \"${precmd_functions[@]}\"; do case \"$f\" in (__nyaterm_emit|__nyaterm_cwd_prompt|__nyaterm_repair_prompt) ;; (*) retained+=(\"$f\");; esac; done\n",
    "  precmd_functions=(\"${retained[@]}\" __nyaterm_emit) || return 1\n",
    "  retained=()\n",
    "  for f in \"${preexec_functions[@]}\"; do [ \"$f\" = __nyaterm_preexec ] || retained+=(\"$f\"); done\n",
    "  preexec_functions=(\"${retained[@]}\" __nyaterm_preexec) || return 1\n",
    "}\n"
);

const FISH_PERSISTENT_SCRIPT: &str = concat!(
    "# nyaterm shell integration v1\n",
    "function __nyaterm_emit\n",
    "  set -l saved_status $status\n",
    "  if set -q NYATERM_READY_PENDING\n",
    "    set -e NYATERM_READY_PENDING\n",
    "    printf '%s' \"$NYATERM_READY_MARKER\"\n",
    "  end\n",
    "  set -l cwd (string replace -a '%' '%25' -- $PWD)\n",
    "  printf '\\033]7;file://%s%s\\007' (hostname) $cwd\n",
    "  return $saved_status\n",
    "end\n",
    "function __nyaterm_cwd_prompt\n",
    "  set -l saved_status $status\n",
    "  set -l cwd (string replace -a '%' '%25' -- $PWD)\n",
    "  printf '\\033]7;file://%s%s\\007' (hostname) $cwd\n",
    "  return $saved_status\n",
    "end\n",
    "function __nyaterm_install_cwd_prompt\n",
    "  functions -e __nyaterm_cwd_prompt_event 2>/dev/null\n",
    "  function __nyaterm_cwd_prompt_event --on-event fish_prompt\n",
    "    __nyaterm_cwd_prompt\n",
    "  end\n",
    "  functions -q __nyaterm_cwd_prompt_event\n",
    "end\n",
    "function __nyaterm_preexec\n",
    "  set -l saved_status $status\n",
    "  if test -n \"$argv[1]\"; and command -sq base64\n",
    "    set -l b64 (printf '%s' \"$argv[1]\" | base64 | tr -d '\\r\\n')\n",
    "    if test -n \"$b64\"\n",
    "      printf '\\033]7777;NyaTermCommand:%s\\007' \"$b64\"\n",
    "    end\n",
    "  end\n",
    "  return $saved_status\n",
    "end\n",
    "function __nyaterm_install_prompt\n",
    "  functions -e __nyaterm_cwd_prompt_event 2>/dev/null\n",
    "  functions -e __nyaterm_emit_event 2>/dev/null\n",
    "  functions -e __nyaterm_preexec_event 2>/dev/null\n",
    "  function __nyaterm_emit_event --on-event fish_prompt\n",
    "    __nyaterm_emit\n",
    "  end\n",
    "  function __nyaterm_preexec_event --on-event fish_preexec\n",
    "    __nyaterm_preexec $argv\n",
    "  end\n",
    "  functions -q __nyaterm_emit_event; and functions -q __nyaterm_preexec_event\n",
    "end\n"
);

// ---------------------------------------------------------------------------
// Streaming OSC stripper
// ---------------------------------------------------------------------------

const MAX_OSC_BUF: usize = 64 * 1024;
const MAX_CWD_OSC_PAYLOAD: usize = MAX_OSC_BUF;
const MAX_COMMAND_OSC_PAYLOAD: usize = 64 * 1024;

/// Control-string families understood by the startup transport filter.
///
/// xterm.js accepts both the traditional 7-bit `ESC` introducers and the
/// equivalent 8-bit C1 controls. Rust does not interpret application OSC 0/2
/// payloads; it only finds complete frames so they can be forwarded intact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ControlSequenceKind {
    Csi,
    Osc,
    Dcs,
}

/// Find the next recognized 7-bit or C1 control-sequence introducer.
/// Returns the byte offset, family, and byte length of the introducer.
pub(crate) fn find_control_sequence_start(
    value: &str,
    from: usize,
) -> Option<(usize, ControlSequenceKind, usize)> {
    let tail = value.get(from..)?;
    let mut chars = tail.char_indices().peekable();
    while let Some((offset, character)) = chars.next() {
        let index = from + offset;
        match character {
            '\x1b' => match chars.peek().map(|(_, next)| *next) {
                Some('[') => return Some((index, ControlSequenceKind::Csi, 2)),
                Some(']') => return Some((index, ControlSequenceKind::Osc, 2)),
                Some('P') => return Some((index, ControlSequenceKind::Dcs, 2)),
                _ => {}
            },
            '\u{009b}' => return Some((index, ControlSequenceKind::Csi, character.len_utf8())),
            '\u{009d}' => return Some((index, ControlSequenceKind::Osc, character.len_utf8())),
            '\u{0090}' => return Some((index, ControlSequenceKind::Dcs, character.len_utf8())),
            _ => {}
        }
    }
    None
}

/// Find the next OSC introducer, ignoring unrelated CSI/DCS sequences.
pub(crate) fn find_osc_start(value: &str, from: usize) -> Option<(usize, usize)> {
    let mut cursor = from;
    while let Some((index, kind, opener_len)) = find_control_sequence_start(value, cursor) {
        if kind == ControlSequenceKind::Osc {
            return Some((index, opener_len));
        }
        cursor = index.saturating_add(opener_len.max(1));
    }
    None
}

/// Return `(content_end, terminator_len)` for a complete control sequence.
/// For CSI, `content_end` is already exclusive and `terminator_len` is zero.
pub(crate) fn find_control_sequence_end(
    value: &str,
    start: usize,
    kind: ControlSequenceKind,
    opener_len: usize,
) -> Option<(usize, usize)> {
    let payload_start = start.checked_add(opener_len)?;
    match kind {
        ControlSequenceKind::Csi => {
            value
                .get(payload_start..)?
                .char_indices()
                .find_map(|(offset, character)| {
                    (('\x40'..='\x7e').contains(&character))
                        .then_some((payload_start + offset + character.len_utf8(), 0))
                })
        }
        ControlSequenceKind::Osc | ControlSequenceKind::Dcs => {
            find_control_string_terminator(value, payload_start)
        }
    }
}

fn find_control_string_terminator(value: &str, from: usize) -> Option<(usize, usize)> {
    let tail = value.get(from..)?;
    let bel = tail.find('\x07').map(|index| (from + index, 1));
    let st_7bit = tail.find("\x1b\\").map(|index| (from + index, 2));
    let st_c1 = tail
        .find('\u{009c}')
        .map(|index| (from + index, '\u{009c}'.len_utf8()));
    [bel, st_7bit, st_c1]
        .into_iter()
        .flatten()
        .min_by_key(|(index, _)| *index)
}

pub(crate) fn control_sequence_payload<'a>(
    value: &'a str,
    start: usize,
    opener_len: usize,
    content_end: usize,
    terminator_len: usize,
) -> Option<&'a str> {
    let payload_start = start.checked_add(opener_len)?;
    // `content_end` is the terminator start for OSC/DCS and the sequence end
    // for CSI; the terminator length is only needed by the caller to advance.
    let _ = terminator_len;
    value.get(payload_start..content_end)
}

/// Ordered Local cwd transport event. A valid payload and an oversized
/// invalidation must be applied in source order; otherwise a cwd update can
/// be erased by an event processed out of order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CwdPayloadEvent {
    Payload(String),
    Invalidated,
}

/// Result returned by [`OscStripper::push`].
pub struct OscResult {
    /// Text safe to display in the terminal (all recognised OSC sequences removed).
    pub visible: String,
    /// Visible text that appeared after the ready marker in this chunk.
    pub visible_after_ready: String,
    /// Released cwd path projection extracted from valid `file://` OSC 7.
    pub cwd_paths: Vec<String>,
    /// Bounded raw OSC 7 payloads used by Local compatibility tests. Runtime
    /// consumers use `cwd_payload_events` to preserve invalidation order.
    #[allow(dead_code)]
    pub cwd_payloads: Vec<String>,
    /// Compatibility summary used by parser tests; ordered runtime handling is
    /// represented by `cwd_payload_events`.
    #[allow(dead_code)]
    pub cwd_payload_invalidated: bool,
    /// Ordered Local-only payload/invalidation events for projection updates.
    pub(crate) cwd_payload_events: Vec<CwdPayloadEvent>,
    /// Whether the ready marker was detected in this chunk.
    pub ready: bool,
    /// Whether this session's explicit integration-failure marker was detected.
    pub ready_failed: bool,
    /// Shell-confirmed commands extracted from private NyaTerm OSC markers.
    pub accepted_commands: Vec<String>,
}

/// Streaming parser that strips OSC 7 and NyaTermReady sequences from terminal
/// output, handling split packets and extracting CWD paths.
pub struct OscStripper {
    buf: String,
    ready_inner: String,
    ready_failed_inner: Option<String>,
    legacy_ready_inner: Option<String>,
    command_marker_inner: Option<String>,
    collect_cwd_payloads: bool,
    discard_oversized_metadata: bool,
    discard_oversized_metadata_pending_st: bool,
}

impl OscStripper {
    pub fn new(ready_marker: &str) -> Self {
        Self::with_cwd_payloads(ready_marker, false)
    }

    pub fn with_cwd_payloads(ready_marker: &str, collect_cwd_payloads: bool) -> Self {
        let ready_inner = marker_inner(ready_marker);
        let ready_failed_inner = ready_inner
            .strip_prefix(READY_MARKER_PREFIX)
            .map(|session_id| format!("{READY_FAILED_MARKER_PREFIX}{session_id}"));
        let legacy_ready_inner = ready_inner
            .strip_prefix(READY_MARKER_PREFIX)
            .map(|session_id| format!("{LEGACY_READY_MARKER_PREFIX}{session_id}"));
        let command_marker_inner = ready_inner
            .strip_prefix(READY_MARKER_PREFIX)
            .map(build_command_marker);

        Self {
            buf: String::new(),
            ready_inner,
            ready_failed_inner,
            legacy_ready_inner,
            command_marker_inner,
            collect_cwd_payloads,
            discard_oversized_metadata: false,
            discard_oversized_metadata_pending_st: false,
        }
    }

    /// Feed a chunk of terminal output.  Returns visible text with OSC
    /// sequences stripped, any CWD paths found, and whether the ready
    /// marker appeared.
    pub fn push(&mut self, chunk: &str) -> OscResult {
        let chunk = if self.discard_oversized_metadata {
            if self.discard_oversized_metadata_pending_st && chunk.starts_with('\\') {
                self.discard_oversized_metadata = false;
                self.discard_oversized_metadata_pending_st = false;
                &chunk[1..]
            } else {
                self.discard_oversized_metadata_pending_st = false;
                let Some((end_idx, term_len)) = find_control_string_terminator(chunk, 0) else {
                    self.discard_oversized_metadata_pending_st = chunk.ends_with('\x1b');
                    return empty_osc_result();
                };
                self.discard_oversized_metadata = false;
                &chunk[end_idx + term_len..]
            }
        } else {
            chunk
        };
        self.buf.push_str(chunk);

        // Safety valve: if the buffer is enormous without any ESC, just
        // flush everything as visible to avoid unbounded memory growth.
        if self.buf.len() > MAX_OSC_BUF
            && !self.buf.contains('\x1b')
            && !self.buf.contains('\u{009d}')
        {
            return OscResult {
                visible: std::mem::take(&mut self.buf),
                visible_after_ready: String::new(),
                cwd_paths: Vec::new(),
                cwd_payloads: Vec::new(),
                cwd_payload_invalidated: false,
                cwd_payload_events: Vec::new(),
                ready: false,
                ready_failed: false,
                accepted_commands: Vec::new(),
            };
        }

        let mut visible = String::new();
        let mut visible_after_ready = String::new();
        let mut paths = Vec::new();
        let mut cwd_payloads = Vec::new();
        let mut cwd_payload_invalidated = false;
        let mut cwd_payload_events = Vec::new();
        let mut ready = false;
        let mut ready_failed = false;
        let mut after_ready = false;
        let mut commands = Vec::new();

        loop {
            let Some((osc_pos, opener_len)) = find_osc_start(&self.buf, 0) else {
                // Retain a trailing ESC or C1 OSC opener so a split opener is
                // recognized on the next push. C1 controls are represented as
                // two UTF-8 bytes in this decoded String.
                let keep = if self.buf.ends_with('\x1b') {
                    1
                } else if self.buf.ends_with('\u{009d}') {
                    '\u{009d}'.len_utf8()
                } else {
                    0
                };
                let visible_end = self.buf.len().saturating_sub(keep);
                if after_ready {
                    visible_after_ready.push_str(&self.buf[..visible_end]);
                }
                visible.push_str(&self.buf[..visible_end]);
                if keep == 0 {
                    self.buf.clear();
                } else {
                    self.buf = self.buf[visible_end..].to_string();
                }
                break;
            };

            // Text before the OSC introducer is always visible.
            if after_ready {
                visible_after_ready.push_str(&self.buf[..osc_pos]);
            }
            visible.push_str(&self.buf[..osc_pos]);
            let rest = self.buf[osc_pos..].to_string();

            // Find the terminator: BEL (\x07), 7-bit ST (ESC \\), or C1 ST.
            let Some((end_idx, term_len)) = find_control_string_terminator(&rest, opener_len)
            else {
                // Incomplete sequence — keep in buffer for next chunk.
                self.buf = rest;

                if let Some(limit) = recognized_metadata_limit(&self.buf) {
                    if self.buf.len() > limit {
                        if self
                            .buf
                            .get(opener_len..)
                            .is_some_and(|value| value.starts_with("7;"))
                        {
                            cwd_payload_invalidated = true;
                            cwd_payload_events.push(CwdPayloadEvent::Invalidated);
                        }
                        self.discard_oversized_metadata_pending_st = self.buf.ends_with('\x1b');
                        self.buf.clear();
                        self.discard_oversized_metadata = true;
                    }
                } else if self.buf.len() > MAX_OSC_BUF {
                    // Ordinary application OSC remains xterm.js-owned.
                    visible.push_str(&self.buf);
                    self.buf.clear();
                }
                break;
            };

            let seq = &rest[..end_idx + term_len];
            let inner = &rest[opener_len..end_idx]; // between opener and terminator

            if inner.starts_with("7;") {
                // Preserve released `file://` path extraction while exposing a
                // bounded raw payload to the stricter Local-only parser.
                let payload = &inner[2..];
                if payload.len() <= MAX_CWD_OSC_PAYLOAD {
                    if self.collect_cwd_payloads {
                        let payload_value = payload.to_string();
                        cwd_payloads.push(payload_value.clone());
                        cwd_payload_events.push(CwdPayloadEvent::Payload(payload_value));
                    }
                    if let Some(path) = parse_legacy_osc7_payload(payload) {
                        paths.push(path);
                    }
                } else {
                    cwd_payload_invalidated = true;
                    cwd_payload_events.push(CwdPayloadEvent::Invalidated);
                }
            } else if self.is_current_ready_marker(inner) {
                ready = true;
                after_ready = true;
            } else if self.is_current_ready_failed_marker(inner) {
                ready_failed = true;
                after_ready = true;
            } else if inner.starts_with(READY_MARKER_PREFIX)
                || inner.starts_with(READY_FAILED_MARKER_PREFIX)
                || inner.starts_with(LEGACY_READY_MARKER_PREFIX)
            {
                // Private marker for another session; strip it without
                // treating this session as ready.
            } else if inner.starts_with(COMMAND_MARKER_PREFIX)
                || inner.starts_with(LEGACY_COMMAND_MARKER_PREFIX)
            {
                if inner.len() <= MAX_COMMAND_OSC_PAYLOAD {
                    if let Some(command) =
                        parse_command_marker(inner, self.command_marker_inner.as_deref())
                    {
                        commands.push(command);
                    }
                }
                // Recognized private marker: always strip it, even when the
                // session nonce/payload is invalid.
            } else {
                // OSC 0/1/2 and all other application sequences belong to the
                // terminal emulator. Keep them byte-for-byte visible to the
                // downstream xterm.js parser.
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
            cwd_payloads,
            cwd_payload_invalidated,
            cwd_payload_events,
            ready,
            ready_failed,
            accepted_commands: commands,
        }
    }

    /// Drain any buffered bytes as visible text (used on timeout / teardown).
    pub fn flush(&mut self) -> String {
        std::mem::take(&mut self.buf)
    }

    pub(crate) fn buffered_len(&self) -> usize {
        self.buf.len()
    }

    fn is_current_ready_marker(&self, inner: &str) -> bool {
        inner == self.ready_inner || self.legacy_ready_inner.as_deref() == Some(inner)
    }

    fn is_current_ready_failed_marker(&self, inner: &str) -> bool {
        self.ready_failed_inner.as_deref() == Some(inner)
    }
}

fn empty_osc_result() -> OscResult {
    OscResult {
        visible: String::new(),
        visible_after_ready: String::new(),
        cwd_paths: Vec::new(),
        cwd_payloads: Vec::new(),
        cwd_payload_invalidated: false,
        cwd_payload_events: Vec::new(),
        ready: false,
        ready_failed: false,
        accepted_commands: Vec::new(),
    }
}

fn recognized_metadata_limit(value: &str) -> Option<usize> {
    let opener_len = if value.starts_with("\x1b]") {
        2
    } else if value.starts_with('\u{009d}') {
        '\u{009d}'.len_utf8()
    } else {
        return None;
    };
    let inner = value.get(opener_len..)?;
    if inner.starts_with("7;") {
        Some(MAX_CWD_OSC_PAYLOAD + opener_len + 2)
    } else if inner.starts_with("7777;NyaTermCommand:")
        || inner.starts_with("7777;DflyCommand:")
        || inner.starts_with("7777;NyaTermReady:")
        || inner.starts_with("7777;NyaTermReadyFailed:")
        || inner.starts_with("7777;DflyReady:")
    {
        Some(MAX_COMMAND_OSC_PAYLOAD + opener_len)
    } else {
        None
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

/// Released OSC 7 path projection. Keep this byte-for-byte compatible for
/// existing SSH, File Explorer and `cwd-changed-*` consumers.
pub(crate) fn parse_legacy_osc7_payload(payload: &str) -> Option<String> {
    let after_scheme = payload.strip_prefix("file://")?;
    let path = if after_scheme.starts_with('/') {
        after_scheme
    } else {
        let slash = after_scheme.find('/')?;
        &after_scheme[slash..]
    };
    (!path.is_empty()).then(|| path.to_string())
}

fn parse_command_marker(inner: &str, expected_marker: Option<&str>) -> Option<String> {
    if let Some(expected) = expected_marker {
        if let Some(rest) = inner.strip_prefix(COMMAND_MARKER_PREFIX) {
            // Legacy markers have base64 immediately after the prefix. Only
            // reject a mismatched marker when the nonce separator is present.
            if rest.contains(':') && !inner.starts_with(expected) {
                return None;
            }
        }
    }

    let payload = if let Some(marker) = expected_marker {
        inner.strip_prefix(marker)
    } else {
        None
    }
    .or_else(|| inner.strip_prefix(COMMAND_MARKER_PREFIX))
    .or_else(|| inner.strip_prefix(LEGACY_COMMAND_MARKER_PREFIX))?;

    // A new-format marker for another session contains a nonce separator and
    // is never accepted by the legacy fallback. This prevents cross-session
    // marker confusion while preserving old scripts until they are refreshed.
    if payload.contains(':') {
        return None;
    }

    let decoded = BASE64_STANDARD.decode(payload).ok()?;
    let command = String::from_utf8(decoded).ok()?;
    (!command.is_empty()).then_some(command)
}

#[cfg(test)]
mod tests {
    use super::{
        BASH_DIRECT_INJECTION_MAX_BYTES, CwdPayloadEvent, MANAGED_BLOCK_END, MANAGED_BLOCK_START,
        OscStripper, ShellKind, activation_script, build_ready_marker, injection_script,
        persistent_script, rc_managed_block, replace_managed_block,
    };
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;

    #[test]
    fn compact_bash_injection_prunes_its_single_history_entry() {
        let script = injection_script(ShellKind::Bash, &build_ready_marker("session-1"))
            .expect("bash injection script");

        assert!(script.contains("NYATERM_PRUNE_HISTORY=1;"));
        assert!(script.contains("history_number=${hline#"));
        assert!(script.contains("history -d \"$history_number\" 2>/dev/null || true;"));
        assert!(script.contains("NYATERM_SKIP_COMMAND_ONCE=1;"));
        assert!(script.contains("unset NYATERM_SKIP_COMMAND_ONCE"));
        assert!(!script.contains("NyaTermHistory:"));
        assert!(!script.contains("NYATERM_HISTORY_BEGIN"));
        assert!(!script.contains("history_window="));
        assert!(!script.contains("BASH_REMATCH"));
        assert!(!script.contains("__nyaterm_saved_prompt_command"));
        assert!(!script.contains("__nyaterm_extra_prompt_commands"));
        assert!(!script.contains("__nyaterm_repair_prompt"));
        assert!(!script.contains("set +o history"));
        assert!(!script.contains("set -o history"));
    }

    #[test]
    fn direct_injection_lines_fit_linux_pty_canonical_input() {
        // Linux N_TTY reserves one byte of its 4096-byte input buffer, so a
        // canonical input line must not exceed 4095 bytes.
        const LINUX_MAX_CANON_BYTES: usize = 4095;
        let ready = build_ready_marker("00000000-0000-0000-0000-000000000000");

        for shell in [ShellKind::Bash, ShellKind::Zsh, ShellKind::Fish] {
            let script = injection_script(shell, &ready).expect("direct injection script");
            let longest_line = script
                .lines()
                .map(|line| line.len())
                .max()
                .unwrap_or_default();

            assert!(
                longest_line <= LINUX_MAX_CANON_BYTES,
                "{shell:?} injection line is {longest_line} bytes"
            );
        }

        let bash = injection_script(ShellKind::Bash, &ready).expect("Bash injection script");
        assert!(bash.starts_with(" NYATERM_PRUNE_HISTORY=1;"));
        assert!(bash.ends_with('\n'));
        assert_eq!(bash.lines().count(), 1);
        assert!(
            bash.len() <= BASH_DIRECT_INJECTION_MAX_BYTES,
            "Bash direct injection is {} bytes; limit is {BASH_DIRECT_INJECTION_MAX_BYTES}",
            bash.len()
        );
        assert!(!bash.contains("{\n"));
    }

    #[test]
    fn oversized_bash_direct_injection_fails_open_to_passive_mode() {
        let oversized_ready = build_ready_marker(&"x".repeat(BASH_DIRECT_INJECTION_MAX_BYTES));

        assert!(injection_script(ShellKind::Bash, &oversized_ready).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn native_bash_injection_prunes_history_with_cmdhist_enabled_or_disabled() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        for cmdhist in ["shopt -s cmdhist", "shopt -u cmdhist"] {
            let integration = injection_script(ShellKind::Bash, &build_ready_marker("session-1"))
                .expect("Bash injection script");
            let input = format!(
                concat!(
                    "HISTFILE=/dev/null\n",
                    "HISTCONTROL=\n",
                    "history -c\n",
                    "{cmdhist}\n",
                    "PS1=\n",
                    "PS2=\n",
                    "{integration}",
                    "history\n",
                    "exit\n",
                ),
                cmdhist = cmdhist,
                integration = integration,
            );
            let mut child = Command::new("/bin/bash")
                .args(["--noprofile", "--norc", "-i"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn interactive Bash");
            child
                .stdin
                .take()
                .expect("Bash stdin")
                .write_all(input.as_bytes())
                .expect("write Bash injection");
            let output = child.wait_with_output().expect("wait for Bash");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            assert!(output.status.success(), "{cmdhist}\n{stdout}\n{stderr}");
            assert!(
                stdout.contains("NyaTermReady:session-1"),
                "{cmdhist}\n{stdout}"
            );
            assert!(stdout.contains(cmdhist), "{cmdhist}\n{stdout}");
            assert!(
                !stdout.contains("NYATERM_PRUNE_HISTORY=1;")
                    && !stdout.contains("__nyaterm_host(){")
                    && !stdout.contains("NyaTermHistory:"),
                "injection leaked into Bash history with {cmdhist}\n{stdout}"
            );
        }
    }

    #[test]
    fn bash_scripts_keep_legacy_scalar_semantics_without_extglob_syntax() {
        let ready = build_ready_marker("session-1");
        let direct = injection_script(ShellKind::Bash, &ready).expect("Bash injection");
        let persistent = persistent_script(ShellKind::Bash).expect("Bash persistent script");
        let activation = activation_script(ShellKind::Bash, &ready).expect("Bash activation");

        for script in [&direct, persistent, &activation] {
            assert!(!script.contains(" in ("));
            assert!(!script.contains("BASH_REMATCH"));
        }
        assert!(!direct.contains("array_prompt_supported"));
        assert!(!direct.contains("capture_prompt_string"));
        assert!(!direct.contains("__nyaterm_repair_prompt"));
        assert!(persistent.contains("array_prompt_supported"));
        assert!(persistent.contains("capture_prompt_string"));
        assert!(persistent.contains("BASH_VERSINFO[1]"));
        assert!(persistent.contains("__nyaterm_prompt_guard(){ return $?; }"));
        assert!(persistent.contains("__nyaterm_repair_prompt; __nyaterm_prompt_guard"));
    }

    #[test]
    fn managed_block_is_added_replaced_and_deduplicated() {
        let block = rc_managed_block(ShellKind::Bash).expect("bash block");
        let added = replace_managed_block("alias ll='ls -la'\n", &block);

        assert!(added.contains("alias ll='ls -la'"));
        assert!(block.contains("__nyaterm_install_cwd_prompt"));
        assert_eq!(added.matches(MANAGED_BLOCK_START).count(), 1);
        assert_eq!(added.matches(MANAGED_BLOCK_END).count(), 1);

        let replacement = format!("{MANAGED_BLOCK_START}\nnew body\n{MANAGED_BLOCK_END}");
        let replaced = replace_managed_block(&added, &replacement);

        assert!(replaced.contains("new body"));
        assert!(!replaced.contains("shell-integration.bash"));
        assert_eq!(replaced.matches(MANAGED_BLOCK_START).count(), 1);
    }

    #[test]
    fn fish_persistent_script_keeps_full_activation_explicit() {
        let script = persistent_script(ShellKind::Fish).expect("fish persistent script");
        let cwd_install_pos = script
            .find("function __nyaterm_install_cwd_prompt")
            .expect("cwd install function");
        let install_pos = script
            .find("function __nyaterm_install_prompt")
            .expect("install function");

        assert!(cwd_install_pos < install_pos);
        assert!(script[cwd_install_pos..install_pos].contains("--on-event fish_prompt"));
        assert!(script[install_pos..].contains("--on-event fish_prompt"));
        assert!(script[install_pos..].contains("--on-event fish_preexec"));
    }

    #[test]
    fn persistent_hooks_preserve_status_and_fail_open_on_incompatible_containers() {
        let bash = persistent_script(ShellKind::Bash).expect("bash persistent script");
        assert!(bash.contains("local status=$?"));
        assert!(bash.contains("declare -p __nyaterm_extra_prompt_commands"));
        assert!(bash.contains("__nyaterm_prompt_state_writable"));
        assert!(bash.contains("__nyaterm_state_token"));
        assert!(bash.contains("__nyaterm_prompt_state_nonce"));
        assert!(bash.contains("direct_exported"));
        assert!(bash.contains("${__nyaterm_integration_active-}"));
        assert!(bash.contains("__nyaterm_prompt_state_mirror_valid"));
        assert!(bash.contains("__nyaterm_restore_prompt_state"));
        assert!(bash.contains("__nyaterm_run_saved_prompt_command"));
        assert!(bash.contains("__nyaterm_repair_prompt_container"));
        assert!(bash.contains("NYATERM_READY_FAILED_MARKER"));
        assert!(bash.contains("__nyaterm_failure_reported"));

        let zsh = persistent_script(ShellKind::Zsh).expect("zsh persistent script");
        assert!(zsh.contains("builtin typeset saved_status=$?"));
        assert!(zsh.contains("builtin typeset -ga precmd_functions preexec_functions"));
        assert!(zsh.contains("precmd_functions=(\"${retained[@]}\" __nyaterm_emit)"));
        assert!(!zsh.contains("__nyaterm_repair_prompt(){"));
        assert!(!zsh.contains("__nyaterm_emit __nyaterm_repair_prompt)"));
        assert!(zsh.contains("NYATERM_READY_FAILED_MARKER"));
        assert!(zsh.contains("__nyaterm_failure_reported"));

        let fish = persistent_script(ShellKind::Fish).expect("fish persistent script");
        assert!(fish.contains("set -l saved_status $status"));
        assert!(fish.contains("return $saved_status"));
        assert!(fish.contains("functions -q __nyaterm_emit_event"));
    }

    #[test]
    fn zsh_hooks_are_installed_once_without_bare_local_or_runtime_repair_hook() {
        let ready = build_ready_marker("session-1");
        let direct = injection_script(ShellKind::Zsh, &ready).expect("zsh direct injection");
        let persistent = persistent_script(ShellKind::Zsh).expect("zsh persistent script");

        for script in [&direct, persistent] {
            assert!(script.contains("builtin typeset saved_status=$?"));
            assert!(script.contains("builtin typeset b64"));
            assert!(script.contains("builtin typeset -ga precmd_functions preexec_functions"));
            assert!(script.contains("precmd_functions=(\"${retained[@]}\" __nyaterm_emit)"));
            assert!(script.contains("preexec_functions=(\"${retained[@]}\" __nyaterm_preexec)"));
            assert!(script.contains("(__nyaterm_emit|__nyaterm_repair_prompt)"));
            assert!(!script.contains(" local saved_status=$?"));
            assert!(!script.contains(" local b64"));
            assert!(!script.contains("__nyaterm_repair_prompt(){"));
            assert!(!script.contains("__nyaterm_emit __nyaterm_repair_prompt)"));
        }

        assert!(direct.contains("builtin typeset -a retained"));
        assert!(persistent.contains("builtin typeset -a retained"));
    }

    #[cfg(unix)]
    #[test]
    fn native_bash_persistent_resource_preserves_captured_prompt_mutations() {
        use std::process::Command;

        let integration = persistent_script(ShellKind::Bash).expect("bash persistent script");
        let script = format!(
            concat!(
                "NYATERM_COMMAND_MARKER='7777;NyaTermCommand:test-session:'\n",
                "PROMPT_COMMAND=':'; hook=0\n",
                "{0}\n",
                "__nyaterm_install_prompt\n",
                "PROMPT_COMMAND+='; hook=$((hook+1))'\n",
                "eval \"$PROMPT_COMMAND\"\n",
                "eval \"$PROMPT_COMMAND\"\n",
                "printf 'before=%s\\n' \"$hook\"\n",
                "{0}\n",
                "__nyaterm_install_prompt\n",
                "eval \"$PROMPT_COMMAND\"\n",
                "printf 'after=%s\\n' \"$hook\"\n",
                "collision=0\n",
                "__nyaterm_saved_prompt_command='collision=$((collision+1))'\n",
                "__nyaterm_extra_prompt_commands=('collision=$((collision+10))')\n",
                "{0}\n",
                "__nyaterm_install_prompt\n",
                "eval \"$PROMPT_COMMAND\"\n",
                "printf 'restored=%s collision=%s\\n' \"$hook\" \"$collision\"\n",
                "readonly __nyaterm_saved_prompt_command='collision=$((collision+1))'\n",
                "{0}\n",
                "if __nyaterm_install_prompt; then readonly_install=unexpected; else readonly_install=failed; fi\n",
                "eval \"$PROMPT_COMMAND\"\n",
                "printf 'readonly_install=%s collision=%s\\n' \"$readonly_install\" \"$collision\"\n"
            ),
            integration
        );
        let output = Command::new("/bin/bash")
            .args(["--noprofile", "--norc", "-c", &script])
            .output()
            .expect("run Bash persistent re-source smoke");
        let mut combined = output.stdout;
        combined.extend_from_slice(&output.stderr);

        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&combined)
        );
        assert!(
            combined
                .windows(b"before=2".len())
                .any(|value| value == b"before=2"),
            "{}",
            String::from_utf8_lossy(&combined)
        );
        assert!(
            combined
                .windows(b"after=3".len())
                .any(|value| value == b"after=3"),
            "{}",
            String::from_utf8_lossy(&combined)
        );
        assert!(
            combined
                .windows(b"restored=4 collision=0".len())
                .any(|value| value == b"restored=4 collision=0"),
            "{}",
            String::from_utf8_lossy(&combined)
        );
        assert!(
            combined
                .windows(b"readonly_install=failed collision=0".len())
                .any(|value| value == b"readonly_install=failed collision=0"),
            "{}",
            String::from_utf8_lossy(&combined)
        );
    }

    #[cfg(unix)]
    #[test]
    fn direct_bash_rejects_readonly_prompt_state_before_ready() {
        use std::process::Command;

        let integration = injection_script(ShellKind::Bash, &build_ready_marker("session-1"))
            .expect("bash injection script");
        let script = format!(
            r#"readonly PROMPT_COMMAND=collision
{0}
"#,
            integration
        );
        let output = Command::new("/bin/bash")
            .args(["--noprofile", "--norc", "-c", &script])
            .output()
            .expect("run readonly direct-injection smoke");
        let mut combined = output.stdout;
        combined.extend_from_slice(&output.stderr);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&combined)
        );
        assert!(
            combined
                .windows(b"NyaTermReadyFailed:session-1".len())
                .any(|value| value == b"NyaTermReadyFailed:session-1")
        );
        assert!(
            !combined
                .windows(b"NyaTermReady:session-1".len())
                .any(|value| value == b"NyaTermReady:session-1")
        );
    }

    #[cfg(unix)]
    #[test]
    fn persistent_bash_rejects_preseeded_prompt_state() {
        use std::process::Command;

        let integration = persistent_script(ShellKind::Bash).expect("bash persistent script");
        let script = format!(
            r#"NYATERM_COMMAND_MARKER='7777;NyaTermCommand:test-session:'
PROMPT_COMMAND='__nyaterm_prompt; __nyaterm_run_saved_prompt_command; __nyaterm_repair_prompt; __nyaterm_prompt_guard'
__nyaterm_prompt_state_owner=nyaterm-shell-integration-v1
readonly __nyaterm_prompt_state_owner
__nyaterm_saved_prompt_command_mirror='printf __SPOOFED__'
declare -a __nyaterm_extra_prompt_commands_mirror=()
{0}
if __nyaterm_install_prompt; then printf 'UNEXPECTED\n'; else printf 'FAILED\n'; fi
eval "$PROMPT_COMMAND" 2>/dev/null || true
"#,
            integration
        );
        let output = Command::new("/bin/bash")
            .args(["--noprofile", "--norc", "-c", &script])
            .output()
            .expect("run prompt-state provenance smoke");
        let mut combined = output.stdout;
        combined.extend_from_slice(&output.stderr);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&combined)
        );
        assert!(
            combined
                .windows(b"FAILED".len())
                .any(|value| value == b"FAILED")
        );
        assert!(
            !combined
                .windows(b"__SPOOFED__".len())
                .any(|value| value == b"__SPOOFED__")
        );
    }

    #[test]
    fn interactive_shell_ready_marker_is_emitted_after_hook_installation() {
        let ready_marker = build_ready_marker("session-1");
        let ready_pos = |script: &str| script.find("NyaTermReady:session-1").expect("ready marker");
        let assert_no_empty_tail_printf = |script: &str| {
            assert!(!script.contains("printf '' 2>/dev/null"));
        };

        let bash = injection_script(ShellKind::Bash, &ready_marker).expect("bash injection script");
        assert!(bash.find("__nyaterm_prompt(){").expect("bash prompt hook") < ready_pos(&bash));
        assert!(
            bash.find(" __nyaterm_install_prompt;")
                .expect("bash prompt install")
                < ready_pos(&bash)
        );
        assert!(bash.contains("printf '\\033]7;file://%s%s\\007'"));
        assert_no_empty_tail_printf(&bash);

        let zsh = injection_script(ShellKind::Zsh, &ready_marker).expect("zsh injection script");
        assert!(zsh.find("__nyaterm_emit(){").expect("zsh prompt hook") < ready_pos(&zsh));
        assert!(
            zsh.find(" fc -P 2>/dev/null\n")
                .expect("zsh history restore")
                < ready_pos(&zsh)
        );
        assert!(zsh.contains("printf '\\033]7;file://%s%s\\007'"));
        assert_no_empty_tail_printf(&zsh);

        let fish = injection_script(ShellKind::Fish, &ready_marker).expect("fish injection script");
        assert!(
            fish.find("function __nyaterm_emit --on-event fish_prompt;")
                .expect("fish prompt hook")
                < ready_pos(&fish)
        );
        assert!(
            fish.find(" set -e fish_private_mode 2>/dev/null\n")
                .expect("fish private mode cleanup")
                < ready_pos(&fish)
        );
        assert!(fish.contains("printf '\\033]7;file://%s%s\\007'"));
        assert_no_empty_tail_printf(&fish);
        assert!(bash.contains("${PWD//%/%25}"));
        assert!(zsh.contains("${PWD//\\%/%25}"));
        assert!(fish.contains("string replace -a '%' '%25' -- $PWD"));
        assert!(
            persistent_script(ShellKind::Bash)
                .expect("bash persistent script")
                .contains("${PWD//%/%25}")
        );
        assert!(
            persistent_script(ShellKind::Zsh)
                .expect("zsh persistent script")
                .contains("${PWD//\\%/%25}")
        );
        assert!(
            persistent_script(ShellKind::Fish)
                .expect("fish persistent script")
                .contains("string replace -a '%' '%25' -- $PWD")
        );

        assert!(bash.contains("NYATERM_COMMAND_MARKER"));
        assert!(zsh.contains("NYATERM_COMMAND_MARKER"));
        assert!(fish.contains("NYATERM_COMMAND_MARKER"));
        for script in [&bash, &zsh, &fish] {
            assert!(!script.contains("\\033]2;"));
        }
        for script in [
            persistent_script(ShellKind::Bash).unwrap(),
            persistent_script(ShellKind::Zsh).unwrap(),
            persistent_script(ShellKind::Fish).unwrap(),
        ] {
            assert!(!script.contains("\\033]2;"));
        }
        assert!(bash.contains("NyaTermCommand:session-1:"));
        assert!(zsh.contains("NyaTermCommand:session-1:"));
        assert!(fish.contains("NyaTermCommand:session-1:"));
        for script in [&bash, &zsh, &fish] {
            assert!(script.contains("NyaTermReadyFailed:session-1"));
        }
        assert!(bash.contains("local status=$?"));
        assert!(bash.contains("__nyaterm_install_prompt"));
        assert!(!bash.contains("__nyaterm_run_saved_prompt_command"));
        assert!(!bash.contains("__nyaterm_repair_prompt"));
        assert!(zsh.contains("builtin typeset -ga precmd_functions preexec_functions"));
        assert!(!zsh.contains("__nyaterm_repair_prompt(){"));
        assert!(fish.contains("return $saved_status"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_zsh_percent_escape_does_not_append_to_plain_cwd() {
        use std::process::Command;

        let output = Command::new("/bin/zsh")
            .args([
                "-fc",
                concat!(
                    "plain='/opt/tomcat/bin'; ",
                    "percent='/opt/100%'; ",
                    "printf '%s\\n%s\\n' \"${plain//\\%/%25}\" \"${percent//\\%/%25}\"",
                ),
            ])
            .output()
            .expect("run native zsh percent escaping");

        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "/opt/tomcat/bin\n/opt/100%25\n"
        );
    }

    #[test]
    fn activation_scripts_emit_ready_without_empty_tail_printf() {
        let ready_marker = build_ready_marker("session-1");

        for shell in [ShellKind::Bash, ShellKind::Zsh, ShellKind::Fish] {
            let script = activation_script(shell, &ready_marker).expect("activation script");

            assert!(script.contains("NyaTermReady:session-1"));
            assert!(script.contains("NyaTermReadyFailed:session-1"));
            assert!(!script.contains("printf '' 2>/dev/null"));
        }
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
    fn ready_marker_before_cwd_osc_preserves_prompt_after_ready() {
        let payload = concat!(
            "echoed injection",
            "\x1b]7777;NyaTermReady:session-1\x07",
            "\x1b]7;file://host/home/user\x07",
            "[user@host ~]$ "
        );

        let result = OscStripper::new(&build_ready_marker("session-1")).push(payload);

        assert!(result.ready);
        assert_eq!(result.cwd_paths, vec!["/home/user".to_string()]);
        assert_eq!(result.visible, "echoed injection[user@host ~]$ ");
        assert_eq!(result.visible_after_ready, "[user@host ~]$ ");
    }

    #[test]
    fn session_bound_failure_marker_is_stripped_and_reported() {
        let payload = concat!(
            "before",
            "\x1b]7777;NyaTermReadyFailed:session-1\x07",
            "after",
            "\x1b]7777;NyaTermReadyFailed:session-2\x07",
        );

        let result = OscStripper::new(&build_ready_marker("session-1")).push(payload);

        assert!(!result.ready);
        assert!(result.ready_failed);
        assert_eq!(result.visible, "beforeafter");
        assert_eq!(result.visible_after_ready, "after");
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

    #[test]
    fn parses_osc_when_the_opener_is_split_after_escape() {
        let mut stripper = OscStripper::with_cwd_payloads(&build_ready_marker("s"), true);
        let first = stripper.push("before\x1b");
        assert_eq!(first.visible, "before");
        let second = stripper.push("]7;file://localhost/home/user\x07after");
        assert_eq!(second.visible, "after");
        assert_eq!(second.cwd_paths, vec!["/home/user".to_string()]);
        assert_eq!(
            second.cwd_payloads,
            vec!["file://localhost/home/user".to_string()]
        );
    }

    #[test]
    fn passthrough_mode_keeps_osc_title_sequences_visible() {
        let mut stripper = OscStripper::new(&build_ready_marker("session-1"));
        let payload = "before\x1b]2;My Title\x07after";

        let result = stripper.push(payload);
        assert_eq!(result.visible, payload);
    }

    #[test]
    fn c1_osc_title_and_st_are_kept_for_xterm() {
        let mut stripper = OscStripper::new(&build_ready_marker("session-1"));
        let payload = "before\u{009d}2;C1 title\u{009c}after";

        let result = stripper.push(payload);
        assert_eq!(result.visible, payload);
    }

    #[test]
    fn c1_osc_cwd_payload_is_extracted_without_interpreting_title() {
        let mut stripper = OscStripper::with_cwd_payloads(&build_ready_marker("s"), true);
        let first = stripper.push("before\u{009d}");
        assert_eq!(first.visible, "before");

        let second = stripper.push("7;file://localhost/home/user\u{009c}after");
        assert_eq!(second.visible, "after");
        assert_eq!(second.cwd_paths, vec!["/home/user".to_string()]);
        assert_eq!(
            second.cwd_payloads,
            vec!["file://localhost/home/user".to_string()]
        );
    }

    #[test]
    fn title_osc_stays_exact_and_ordered_around_ready() {
        let before = "\x1b]2;Remote title\x07";
        let after = "\x1b]0;Prompt title\x1b\\";
        let payload =
            format!("hidden{before}\x1b]7777;NyaTermReady:session-1\x07{after}[user@host]$ ");

        let result = OscStripper::new(&build_ready_marker("session-1")).push(&payload);

        assert!(result.ready);
        assert_eq!(
            result.visible,
            format!("hidden{before}{after}[user@host]$ ")
        );
        assert_eq!(result.visible_after_ready, format!("{after}[user@host]$ "));
    }

    #[test]
    fn split_title_osc_is_reassembled_once_without_interpreting_payload() {
        let mut stripper = OscStripper::new(&build_ready_marker("session-1"));
        let first = stripper.push("hidden\x1b]2;split");
        assert_eq!(first.visible, "hidden");

        let second = stripper.push(" title\x1b\\tail");
        assert_eq!(second.visible, "\x1b]2;split title\x1b\\tail");

        let ordinary_icon = stripper.push("\x1b]1;icon only\x07");
        assert_eq!(ordinary_icon.visible, "\x1b]1;icon only\x07");
    }

    #[test]
    fn oversized_title_osc_remains_normal_visible_output() {
        let payload = format!("\x1b]2;{}\x07", "x".repeat(17 * 1024));
        let result = OscStripper::new(&build_ready_marker("session-1")).push(&payload);

        assert_eq!(result.visible, payload);
    }

    #[test]
    fn session_bound_command_marker_is_parsed_and_wrong_session_is_ignored() {
        let command = BASE64_STANDARD.encode("docker ps");
        let marker = crate::core::ssh::osc::build_command_marker("session-1");
        let payload = format!(
            "before\x1b]{marker}{command}\x07wrong\x1b]7777;NyaTermCommand:session-2:{command}\x07after"
        );

        let result = OscStripper::new(&build_ready_marker("session-1")).push(&payload);
        assert_eq!(result.visible, "beforewrongafter");
        assert_eq!(result.accepted_commands, vec!["docker ps".to_string()]);
    }

    #[test]
    fn osc7_keeps_released_path_projection_and_exposes_raw_payload() {
        let mut stripper = OscStripper::with_cwd_payloads(&build_ready_marker("s"), true);
        let result = stripper.push(
            "\x1b]7;file://remote.example.com/opt/a%20b\x07\x1b]7;kitty-shell-cwd://host/home/a%20b\x07",
        );
        assert_eq!(result.cwd_paths, vec!["/opt/a%20b".to_string()]);
        assert_eq!(
            result.cwd_payloads,
            vec![
                "file://remote.example.com/opt/a%20b".to_string(),
                "kitty-shell-cwd://host/home/a%20b".to_string(),
            ]
        );
        assert!(result.visible.is_empty());
    }

    #[test]
    fn local_cwd_events_preserve_valid_and_invalid_source_order() {
        let huge = "x".repeat(70 * 1024);
        let mut stripper = OscStripper::with_cwd_payloads(&build_ready_marker("s"), true);
        let result = stripper.push(&format!(
            "\x1b]7;file://localhost/{huge}\x07\x1b]7;file://localhost/final\x07"
        ));
        assert_eq!(
            result.cwd_payload_events,
            vec![
                CwdPayloadEvent::Invalidated,
                CwdPayloadEvent::Payload("file://localhost/final".to_string()),
            ]
        );

        let mut stripper = OscStripper::with_cwd_payloads(&build_ready_marker("s"), true);
        let result = stripper.push(&format!(
            "\x1b]7;file://localhost/final\x07\x1b]7;file://localhost/{huge}\x07"
        ));
        assert_eq!(
            result.cwd_payload_events,
            vec![
                CwdPayloadEvent::Payload("file://localhost/final".to_string()),
                CwdPayloadEvent::Invalidated,
            ]
        );
    }

    #[test]
    fn oversized_recognized_metadata_is_stripped_without_decoding() {
        let huge_cwd = format!("\x1b]7;file://localhost/{}\x07", "x".repeat(70 * 1024));
        let result = OscStripper::new(&build_ready_marker("s")).push(&huge_cwd);
        assert!(result.visible.is_empty());
        assert!(result.cwd_paths.is_empty());
        assert!(result.cwd_payloads.is_empty());
        assert!(result.cwd_payload_invalidated);

        let huge_command = format!("\x1b]7777;NyaTermCommand:s:{}\x07", "A".repeat(70 * 1024));
        let result = OscStripper::new(&build_ready_marker("s")).push(&huge_command);
        assert!(result.visible.is_empty());
        assert!(result.accepted_commands.is_empty());
    }

    #[test]
    fn split_oversized_metadata_discards_through_its_terminator() {
        let mut stripper = OscStripper::new(&build_ready_marker("s"));
        let first = stripper.push(&format!(
            "before\x1b]7;file://localhost/{}",
            "x".repeat(70 * 1024)
        ));
        assert_eq!(first.visible, "before");
        assert!(first.cwd_payloads.is_empty());
        assert!(first.cwd_payload_invalidated);

        let second = stripper.push("discarded-tail\x07after");
        assert_eq!(second.visible, "after");
        assert!(second.cwd_paths.is_empty());
        assert!(second.cwd_payloads.is_empty());
        assert!(!second.cwd_payload_invalidated);

        let mut stripper = OscStripper::new(&build_ready_marker("s"));
        let first = stripper.push(&format!(
            "\x1b]7;file://localhost/{}\x1b",
            "x".repeat(70 * 1024)
        ));
        assert!(first.visible.is_empty());
        assert!(first.cwd_payload_invalidated);
        let second = stripper.push("\\after-st");
        assert_eq!(second.visible, "after-st");
    }

    /// Guard against unbalanced shell control structures in the generated
    /// bash/zsh scripts. Every `if ...; then` must be closed by a matching
    /// `fi` — a missing `fi` turns the whole injected script into a syntax
    /// error that breaks shell integration silently.
    #[test]
    fn generated_bash_zsh_scripts_are_if_fi_balanced() {
        fn count_token(script: &str, needle: &str) -> usize {
            script
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .filter(|word| *word == needle)
                .count()
        }

        let marker = build_ready_marker("session-1");
        for shell in [ShellKind::Bash, ShellKind::Zsh] {
            let script = injection_script(shell, &marker).expect("injection script");
            assert_eq!(
                count_token(&script, "then"),
                count_token(&script, "fi"),
                "unbalanced if/fi in {shell:?} injection script"
            );

            if let Some(persist) = persistent_script(shell) {
                assert_eq!(
                    count_token(persist, "then"),
                    count_token(persist, "fi"),
                    "unbalanced if/fi in {shell:?} persistent script"
                );
            }
        }
    }
}
