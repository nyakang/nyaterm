use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use russh::{ChannelMsg, client};

use super::{SshClientHandler, SshShellHandle};

const READY_MARKER_PREFIX: &str = "7777;NyaTermReady:";
const COMMAND_MARKER_PREFIX: &str = "7777;NyaTermCommand:";
const LEGACY_READY_MARKER_PREFIX: &str = "7777;DflyReady:";
const LEGACY_COMMAND_MARKER_PREFIX: &str = "7777;DflyCommand:";
const MAX_OSC_BUF: usize = 64 * 1024;
const INITIAL_PROMPT_TAIL_LIMIT: usize = 1024;
const SUPPRESSED_OUTPUT_LIMIT: usize = 64 * 1024;
// Keep each PTY write below the smallest canonical input queue observed on
// macOS. Newline-aware chunking preserves complete shell statements while
// avoiding dropped bytes when the remote shell is briefly not scheduled.
const SHELL_INJECTION_CHUNK_SIZE: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ShellKind {
    Bash,
    Zsh,
    Fish,
    PosixSh,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ShellIntegrationMode {
    Full,
    CwdOnly,
}

impl ShellIntegrationMode {
    fn install_arg(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::CwdOnly => "cwd",
        }
    }
}

impl ShellKind {
    fn from_name(name: &str) -> Self {
        let value = name.to_ascii_lowercase();
        if value.contains("fish") {
            Self::Fish
        } else if value.contains("zsh") {
            Self::Zsh
        } else if value.contains("bash") {
            Self::Bash
        } else if value.contains("sh") {
            Self::PosixSh
        } else {
            Self::Unknown
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct SshIntegrationOutput {
    pub(super) visible: Vec<u8>,
    pub(super) cwd_paths: Vec<String>,
    pub(super) accepted_commands: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OscResult {
    pub(super) visible: Vec<u8>,
    pub(super) visible_after_ready: Vec<u8>,
    pub(super) cwd_paths: Vec<String>,
    pub(super) ready: bool,
    pub(super) accepted_commands: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SshShellIntegrationPhase {
    Normal,
    WaitInitial,
    Suppressing,
}

pub(super) struct SshShellIntegrationState {
    phase: SshShellIntegrationPhase,
    pending_script: Option<Vec<u8>>,
    stripper: OscStripper,
    suppress_started_at: Option<Instant>,
    suppressed_visible_bytes: usize,
    initial_prompt_seen: bool,
    initial_prompt_tail: Vec<u8>,
    initial_prompt_pattern: Vec<u8>,
    suppress_initial_prompt_after_ready: bool,
    post_ready_prompt_buffer: Vec<u8>,
}

impl SshShellIntegrationState {
    pub(super) fn new(
        pending_script: Option<Vec<u8>>,
        ready_marker: String,
        legacy_ready_marker: Option<String>,
    ) -> Self {
        let phase = if pending_script.is_some() {
            SshShellIntegrationPhase::WaitInitial
        } else {
            SshShellIntegrationPhase::Normal
        };
        Self {
            phase,
            pending_script,
            stripper: OscStripper::new(&ready_marker, legacy_ready_marker.as_deref()),
            suppress_started_at: None,
            suppressed_visible_bytes: 0,
            initial_prompt_seen: false,
            initial_prompt_tail: Vec::new(),
            initial_prompt_pattern: Vec::new(),
            suppress_initial_prompt_after_ready: false,
            post_ready_prompt_buffer: Vec::new(),
        }
    }

    /// Creates the initial phase used while shell detection runs in parallel
    /// with the interactive PTY reader.
    pub(super) fn waiting_for_integration(
        ready_marker: String,
        legacy_ready_marker: Option<String>,
    ) -> Self {
        let mut state = Self::new(None, ready_marker, legacy_ready_marker);
        state.phase = SshShellIntegrationPhase::WaitInitial;
        state
    }

    /// Completes asynchronous shell integration preparation without hiding
    /// bytes that were already forwarded from the login banner.
    pub(super) fn set_integration_script(&mut self, script: Option<Vec<u8>>) {
        self.pending_script = script;
        self.phase = if self.pending_script.is_some() {
            SshShellIntegrationPhase::WaitInitial
        } else {
            SshShellIntegrationPhase::Normal
        };
    }

    pub(super) fn is_normal(&self) -> bool {
        self.phase == SshShellIntegrationPhase::Normal
    }

    pub(super) fn is_suppressing(&self) -> bool {
        self.phase == SshShellIntegrationPhase::Suppressing
    }

    pub(super) fn is_waiting_initial(&self) -> bool {
        self.phase == SshShellIntegrationPhase::WaitInitial
    }

    /// Returns whether the shell has rendered the prompt that terminates the
    /// initial login/banner burst. Do not start the integration write before
    /// this point: a slow PAM/MOTD response can arrive after the first PTY
    /// packet, and entering suppression early would hide that response.
    pub(super) fn initial_prompt_seen(&self) -> bool {
        self.initial_prompt_seen
    }

    pub(super) fn should_inject_on_initial_delay(&self) -> bool {
        self.is_waiting_initial() && self.pending_script.is_some()
    }

    pub(super) async fn inject(&mut self, channel: &mut russh::Channel<client::Msg>) {
        let Some(script) = self.pending_script.take() else {
            return;
        };
        let mut sent = 0;
        let mut success = true;
        while sent < script.len() {
            let mut end = (sent + SHELL_INJECTION_CHUNK_SIZE).min(script.len());
            if end < script.len()
                && let Some(newline) = script[sent..end].iter().rposition(|byte| *byte == b'\n')
            {
                end = sent + newline + 1;
            }
            if channel
                .data_bytes(script[sent..end].to_vec())
                .await
                .is_err()
            {
                success = false;
                break;
            }
            sent = end;
        }
        if success {
            self.phase = SshShellIntegrationPhase::Suppressing;
            self.suppress_started_at = Some(Instant::now());
        } else {
            self.force_normal();
        }
    }

    pub(super) fn force_normal_after_timeout(&mut self) -> SshIntegrationOutput {
        let flushed = self.stripper.flush();
        self.force_normal();
        let _ = flushed;
        SshIntegrationOutput::default()
    }

    fn timeout_expired(&self) -> bool {
        self.phase == SshShellIntegrationPhase::Suppressing
            && self
                .suppress_started_at
                .is_some_and(|started_at| started_at.elapsed() > Duration::from_secs(30))
    }

    fn force_normal(&mut self) {
        self.phase = SshShellIntegrationPhase::Normal;
        self.suppress_started_at = None;
        self.pending_script = None;
        self.suppressed_visible_bytes = 0;
        self.initial_prompt_seen = false;
        self.initial_prompt_tail.clear();
        self.initial_prompt_pattern.clear();
        self.suppress_initial_prompt_after_ready = false;
        self.post_ready_prompt_buffer.clear();
    }

    pub(super) fn filter_output(&mut self, bytes: &[u8]) -> SshIntegrationOutput {
        match self.phase {
            SshShellIntegrationPhase::Normal => {
                let result = self.stripper.push(bytes);
                if self.suppress_initial_prompt_after_ready {
                    let visible = self.filter_post_ready_prompt(result.visible);
                    return SshIntegrationOutput {
                        visible,
                        cwd_paths: result.cwd_paths,
                        accepted_commands: result.accepted_commands,
                    };
                }
                result.into_output()
            }
            SshShellIntegrationPhase::WaitInitial => {
                let result = self.stripper.push(bytes);
                // Match the Tauri transport by exposing initial remote output immediately. The
                // caller waits for the initial burst to become quiet before injecting. Remember
                // a prompt that was already rendered; the prompt emitted after our ready marker
                // is then suppressed instead of dropping the banner or the original prompt.
                self.remember_initial_visible(&result.visible);
                result.into_output()
            }
            SshShellIntegrationPhase::Suppressing => {
                if self.timeout_expired() {
                    return self.force_normal_after_timeout();
                }
                let result = self.stripper.push(bytes);
                let cwd_paths = result.cwd_paths;
                let accepted_commands = result.accepted_commands;
                if result.ready {
                    let suppress_prompt_after_ready = self.initial_prompt_seen;
                    let initial_prompt_pattern = self.initial_prompt_pattern.clone();
                    self.force_normal();
                    self.suppress_initial_prompt_after_ready = suppress_prompt_after_ready;
                    self.initial_prompt_pattern = initial_prompt_pattern;
                    let visible = if suppress_prompt_after_ready {
                        self.filter_post_ready_prompt(result.visible_after_ready)
                    } else {
                        result.visible_after_ready
                    };
                    SshIntegrationOutput {
                        visible,
                        cwd_paths,
                        accepted_commands,
                    }
                } else {
                    self.suppressed_visible_bytes = self
                        .suppressed_visible_bytes
                        .saturating_add(result.visible.len())
                        .min(SUPPRESSED_OUTPUT_LIMIT);
                    SshIntegrationOutput {
                        visible: Vec::new(),
                        cwd_paths,
                        accepted_commands,
                    }
                }
            }
        }
    }

    fn remember_initial_visible(&mut self, visible: &[u8]) {
        if visible.is_empty() {
            return;
        }
        self.initial_prompt_tail.extend_from_slice(visible);
        if self.initial_prompt_tail.len() > INITIAL_PROMPT_TAIL_LIMIT {
            let trim = self
                .initial_prompt_tail
                .len()
                .saturating_sub(INITIAL_PROMPT_TAIL_LIMIT);
            self.initial_prompt_tail.drain(..trim);
        }
        if let Some(prompt) = trailing_shell_prompt(&self.initial_prompt_tail) {
            self.initial_prompt_seen = true;
            self.initial_prompt_pattern = strip_terminal_prompt_controls(prompt);
        }
    }

    fn filter_post_ready_prompt(&mut self, visible: Vec<u8>) -> Vec<u8> {
        self.post_ready_prompt_buffer.extend_from_slice(&visible);
        if self.post_ready_prompt_buffer.len() > INITIAL_PROMPT_TAIL_LIMIT {
            self.suppress_initial_prompt_after_ready = false;
            return std::mem::take(&mut self.post_ready_prompt_buffer);
        }

        if let Some(line_start) = trailing_shell_prompt_start(&self.post_ready_prompt_buffer)
            && (self.initial_prompt_pattern.is_empty()
                || self.is_initial_prompt_candidate(&self.post_ready_prompt_buffer[line_start..]))
        {
            self.post_ready_prompt_buffer.truncate(line_start);
            self.suppress_initial_prompt_after_ready = false;
            return std::mem::take(&mut self.post_ready_prompt_buffer);
        }

        let line_start = self
            .post_ready_prompt_buffer
            .iter()
            .rposition(|byte| *byte == b'\n' || *byte == b'\r')
            .map_or(0, |index| index.saturating_add(1));
        let candidate = &self.post_ready_prompt_buffer[line_start..];
        if !self.is_initial_prompt_candidate(candidate) {
            self.suppress_initial_prompt_after_ready = false;
            return std::mem::take(&mut self.post_ready_prompt_buffer);
        }

        let mut prefix = self.post_ready_prompt_buffer.split_off(line_start);
        std::mem::swap(&mut prefix, &mut self.post_ready_prompt_buffer);
        prefix
    }

    fn is_initial_prompt_candidate(&self, candidate: &[u8]) -> bool {
        let clean = strip_terminal_prompt_controls(candidate);
        if self.initial_prompt_pattern.is_empty() {
            return is_shell_prompt_candidate(candidate);
        }
        self.initial_prompt_pattern.starts_with(&clean)
    }
}

fn trailing_shell_prompt_start(visible: &[u8]) -> Option<usize> {
    let line_start = visible
        .iter()
        .rposition(|byte| *byte == b'\n' || *byte == b'\r')
        .map_or(0, |index| index.saturating_add(1));
    looks_like_shell_prompt(&visible[line_start..]).then_some(line_start)
}

fn trailing_shell_prompt(visible: &[u8]) -> Option<&[u8]> {
    let line_start = trailing_shell_prompt_start(visible)?;
    Some(&visible[line_start..])
}

fn looks_like_shell_prompt(bytes: &[u8]) -> bool {
    if bytes.is_empty() || bytes.len() > INITIAL_PROMPT_TAIL_LIMIT {
        return false;
    }
    let clean = strip_terminal_prompt_controls(bytes);
    let text = String::from_utf8_lossy(&clean);
    let trimmed = text.trim();
    let Some(marker) = trimmed.chars().last() else {
        return false;
    };
    if !matches!(marker, '$' | '#' | '%' | '>' | '❯' | '➜' | 'λ') {
        return false;
    }
    let prefix_with_spacing = &trimmed[..trimmed.len().saturating_sub(marker.len_utf8())];
    let prefix = prefix_with_spacing.trim_end();
    let has_prompt_separator = prefix.contains(['@', ':', '/', '\\', '~']);
    // A space before a bare marker is much more likely to be ordinary output
    // (for example, "price $") than a shell prompt. Keep path/host-shaped
    // prompts such as "user@host:~$" and Windows-style "PS C:\\>".
    if prefix_with_spacing.chars().any(char::is_whitespace) && !has_prompt_separator {
        return false;
    }
    // A single-token suffix such as "100%", "done#" or "status$" is not
    // distinctive enough to identify a shell prompt. Bare markers remain
    // valid for shells configured with a minimal PS1.
    prefix.is_empty() || has_prompt_separator
}

fn is_shell_prompt_candidate(bytes: &[u8]) -> bool {
    if bytes.len() > INITIAL_PROMPT_TAIL_LIMIT {
        return false;
    }
    let clean = strip_terminal_prompt_controls(bytes);
    !clean
        .iter()
        .any(|byte| byte.is_ascii_control() && *byte != b'\t')
}

fn strip_terminal_prompt_controls(bytes: &[u8]) -> Vec<u8> {
    let mut clean = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != 0x1b {
            if !bytes[index].is_ascii_control() || bytes[index] == b'\t' {
                clean.push(bytes[index]);
            }
            index += 1;
            continue;
        }

        index += 1;
        match bytes.get(index).copied() {
            Some(b'[') => {
                index += 1;
                while let Some(byte) = bytes.get(index).copied() {
                    index += 1;
                    if (0x40..=0x7e).contains(&byte) {
                        break;
                    }
                }
            }
            Some(b']') => {
                index += 1;
                while index < bytes.len() {
                    if bytes[index] == b'\x07' {
                        index += 1;
                        break;
                    }
                    if bytes[index] == 0x1b && bytes.get(index + 1) == Some(&b'\\') {
                        index += 2;
                        break;
                    }
                    index += 1;
                }
            }
            Some(_) | None => {}
        }
    }
    clean
}

impl OscResult {
    fn into_output(self) -> SshIntegrationOutput {
        SshIntegrationOutput {
            visible: self.visible,
            cwd_paths: self.cwd_paths,
            accepted_commands: self.accepted_commands,
        }
    }
}

pub(super) async fn detect_ssh_shell_type(
    handle: &SshShellHandle,
    timeout_ms: u64,
) -> Option<ShellKind> {
    let timeout_ms = timeout_ms.clamp(100, 60_000);
    let command = r#"printf '%s\n' "$SHELL"; ps -p $$ -o comm= 2>/dev/null || true"#;
    tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        let channel = open_ssh_exec_channel(handle, command).await.ok()?;
        let output = collect_ssh_exec_stdout(channel).await;
        let kind = ShellKind::from_name(output.trim());
        (kind != ShellKind::Unknown).then_some(kind)
    })
    .await
    .ok()
    .flatten()
}

pub(super) async fn build_ssh_shell_integration_script(
    handle: &SshShellHandle,
    shell: ShellKind,
    ready_marker: &str,
    terminal_shell_integration: bool,
    cwd_follow_mode: super::SftpCwdFollowMode,
    timeout_ms: u64,
) -> Option<Vec<u8>> {
    let mode = if terminal_shell_integration {
        ShellIntegrationMode::Full
    } else {
        ShellIntegrationMode::CwdOnly
    };
    match cwd_follow_mode {
        super::SftpCwdFollowMode::Off => terminal_shell_integration
            .then(|| ssh_shell_injection_script(shell, ready_marker, mode))
            .flatten()
            .map(String::into_bytes),
        super::SftpCwdFollowMode::ShellIntegration => {
            ssh_shell_injection_script(shell, ready_marker, mode).map(String::into_bytes)
        }
        super::SftpCwdFollowMode::RcFile => {
            match install_remote_shell_integration(handle, shell, timeout_ms).await {
                Ok(()) => activation_script(shell, ready_marker, mode).map(String::into_bytes),
                Err(_error) => {
                    ssh_shell_injection_script(shell, ready_marker, mode).map(String::into_bytes)
                }
            }
        }
    }
}

async fn open_ssh_exec_channel(
    handle: &SshShellHandle,
    command: &str,
) -> anyhow::Result<russh::Channel<client::Msg>> {
    match handle {
        SshShellHandle::Dedicated(handle) | SshShellHandle::Multiplexed(handle) => {
            let handle = handle.lock().await;
            open_ssh_exec_channel_on_handle(&handle, command).await
        }
    }
}

async fn open_ssh_exec_channel_on_handle(
    handle: &client::Handle<SshClientHandler>,
    command: &str,
) -> anyhow::Result<russh::Channel<client::Msg>> {
    let channel = handle.channel_open_session().await?;
    channel.exec(true, command.as_bytes().to_vec()).await?;
    Ok(channel)
}

async fn collect_ssh_exec_stdout(mut channel: russh::Channel<client::Msg>) -> String {
    let mut output = String::new();
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                output.push_str(&String::from_utf8_lossy(&data));
            }
            ChannelMsg::Close | ChannelMsg::Eof => break,
            _ => {}
        }
    }
    let _ = channel.close().await;
    output
}

async fn exec_remote_command(
    handle: &SshShellHandle,
    command: &str,
    timeout_ms: u64,
) -> anyhow::Result<String> {
    let timeout_ms = timeout_ms.clamp(100, 60_000);
    tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        let mut channel = open_ssh_exec_channel(handle, command).await?;
        let mut output = String::new();
        let mut exit_status = None;
        while let Some(message) = channel.wait().await {
            match message {
                ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                    output.push_str(&String::from_utf8_lossy(&data));
                }
                ChannelMsg::ExitStatus {
                    exit_status: status,
                } => exit_status = Some(status),
                ChannelMsg::Close | ChannelMsg::Eof => break,
                _ => {}
            }
        }
        let _ = channel.close().await;
        match exit_status.unwrap_or(0) {
            0 => Ok(output),
            status => anyhow::bail!("remote command exited with status {status}: {output}"),
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("remote command timed out"))?
}

pub(super) fn build_ssh_ready_marker(session_id: &str) -> String {
    format!("\x1b]7777;NyaTermReady:{session_id}\x07")
}

pub(super) fn build_legacy_ssh_ready_marker(ready_marker: &str) -> Option<String> {
    let inner = marker_inner(ready_marker);
    let session_id = inner.strip_prefix(READY_MARKER_PREFIX)?;
    Some(format!("\x1b]{LEGACY_READY_MARKER_PREFIX}{session_id}\x07"))
}

fn ready_printf(marker: &str) -> String {
    marker
        .replace('\\', "\\\\")
        .replace('\x1b', "\\033")
        .replace('\x07', "\\007")
        .replace('\'', "'\\''")
}

pub(super) fn ssh_shell_injection_script(
    shell: ShellKind,
    ready_marker: &str,
    mode: ShellIntegrationMode,
) -> Option<String> {
    let script = match mode {
        ShellIntegrationMode::Full => persistent_script(shell)?,
        ShellIntegrationMode::CwdOnly => cwd_only_script(shell)?,
    };
    let ready = ready_printf(ready_marker);
    let install_arg = mode.install_arg();
    // Bash records each top-level definition from a multiline PTY write separately. Keep the
    // guard start and finish on single physical lines so history is disabled between them.
    let prefix = match shell {
        ShellKind::Bash => {
            " case $- in *h*) NYATERM_INJ_HISTORY_WAS_ENABLED=1; NYATERM_PRUNE_HISTORY=1 ;; *) unset NYATERM_INJ_HISTORY_WAS_ENABLED NYATERM_PRUNE_HISTORY ;; esac; NYATERM_LAST_HISTCMD=\"${HISTCMD-}\"; export NYATERM_INJ=1; set +o history\n"
        }
        ShellKind::Zsh => " fc -p /dev/null 2>/dev/null\n export NYATERM_INJ=1;\n",
        ShellKind::Fish => " set fish_private_mode 1 2>/dev/null\n set -gx NYATERM_INJ 1\n",
        ShellKind::PosixSh | ShellKind::Unknown => return None,
    };
    let suffix = match shell {
        ShellKind::Bash => {
            format!(
                "\nif [ -n \"${{NYATERM_INJ_HISTORY_WAS_ENABLED:-}}\" ]; then set -o history; __nyaterm_prune_history; else unset NYATERM_PRUNE_HISTORY; fi; unset NYATERM_INJ_HISTORY_WAS_ENABLED; __nyaterm_install_prompt {install_arg} 2>/dev/null || true; printf '{ready}'\n"
            )
        }
        ShellKind::Zsh => format!(
            "\n__nyaterm_install_prompt {install_arg} 2>/dev/null || true; fc -P 2>/dev/null\nprintf '{ready}'\n"
        ),
        ShellKind::Fish => format!(
            "\n__nyaterm_install_prompt {install_arg} 2>/dev/null; or true\nset -e fish_private_mode 2>/dev/null\nprintf '{ready}'\n"
        ),
        ShellKind::PosixSh | ShellKind::Unknown => return None,
    };
    // Parse the complete injection as one Bash/Zsh command. Interactive shells may
    // execute PROMPT_COMMAND between physical PTY writes; a here-document keeps the
    // helper definitions ahead of the DEBUG trap and prevents partially parsed
    // function bodies from reaching the prompt. It also avoids the PTY line-buffer
    // limit that truncates a single quoted eval payload on some macOS shells.
    let body = format!("{script}{suffix}");
    match shell {
        ShellKind::Bash | ShellKind::Zsh => Some(format!(
            "{prefix}eval \"$(cat <<'NYATERM_INJECTION_EOF'\n{body}\nNYATERM_INJECTION_EOF\n)\"\n"
        )),
        ShellKind::Fish => Some(format!("{prefix}{body}")),
        ShellKind::PosixSh | ShellKind::Unknown => None,
    }
}

pub(super) fn activation_script(
    shell: ShellKind,
    ready_marker: &str,
    mode: ShellIntegrationMode,
) -> Option<String> {
    let ready = ready_printf(ready_marker);
    let install_arg = mode.install_arg();
    match shell {
        ShellKind::Bash => Some(format!(
            " NYATERM_PRUNE_HISTORY=1; NYATERM_READY_PENDING=1; export NYATERM_INJ=1; export NYATERM_READY_MARKER=\"$(printf '{}')\"; [ -r \"$HOME/.config/nyaterm/shell-integration.bash\" ] && . \"$HOME/.config/nyaterm/shell-integration.bash\"; __nyaterm_install_prompt {install_arg} 2>/dev/null; if [ -n \"${{NYATERM_READY_PENDING:-}}\" ]; then unset NYATERM_READY_PENDING; printf '%s' \"${{NYATERM_READY_MARKER-}}\"; fi\n",
            ready
        )),
        ShellKind::Zsh => Some(format!(
            " fc -p /dev/null 2>/dev/null\n NYATERM_READY_PENDING=1; export NYATERM_INJ=1; export NYATERM_READY_MARKER=\"$(printf '{}')\"; [ -r \"$HOME/.config/nyaterm/shell-integration.zsh\" ] && . \"$HOME/.config/nyaterm/shell-integration.zsh\"; __nyaterm_install_prompt {install_arg} 2>/dev/null; fc -P 2>/dev/null\n if [ -n \"${{NYATERM_READY_PENDING:-}}\" ]; then unset NYATERM_READY_PENDING; printf '%s' \"${{NYATERM_READY_MARKER-}}\"; fi\n",
            ready
        )),
        ShellKind::Fish => Some(format!(
            " set fish_private_mode 1 2>/dev/null\n set -g NYATERM_READY_PENDING 1; set -gx NYATERM_INJ 1; set -gx NYATERM_READY_MARKER (printf '{}'); if test -r \"$HOME/.config/nyaterm/shell-integration.fish\"; source \"$HOME/.config/nyaterm/shell-integration.fish\"; end; __nyaterm_install_prompt {install_arg} 2>/dev/null; set -e fish_private_mode 2>/dev/null\n if set -q NYATERM_READY_PENDING; set -e NYATERM_READY_PENDING; printf '%s' \"$NYATERM_READY_MARKER\"; end\n",
            ready
        )),
        ShellKind::PosixSh | ShellKind::Unknown => None,
    }
}

pub(super) fn persistent_script(shell: ShellKind) -> Option<&'static str> {
    match shell {
        ShellKind::Bash => Some(BASH_PERSISTENT_SCRIPT),
        ShellKind::Zsh => Some(ZSH_PERSISTENT_SCRIPT),
        ShellKind::Fish => Some(FISH_PERSISTENT_SCRIPT),
        ShellKind::PosixSh | ShellKind::Unknown => None,
    }
}

fn cwd_only_script(shell: ShellKind) -> Option<&'static str> {
    match shell {
        ShellKind::Bash => Some(BASH_CWD_ONLY_SCRIPT),
        ShellKind::Zsh => Some(ZSH_CWD_ONLY_SCRIPT),
        ShellKind::Fish => Some(FISH_CWD_ONLY_SCRIPT),
        ShellKind::PosixSh | ShellKind::Unknown => None,
    }
}

fn persistent_script_path(shell: ShellKind) -> Option<&'static str> {
    match shell {
        ShellKind::Bash => Some("$HOME/.config/nyaterm/shell-integration.bash"),
        ShellKind::Zsh => Some("$HOME/.config/nyaterm/shell-integration.zsh"),
        ShellKind::Fish => Some("$HOME/.config/nyaterm/shell-integration.fish"),
        ShellKind::PosixSh | ShellKind::Unknown => None,
    }
}

fn rc_file_path(shell: ShellKind) -> Option<&'static str> {
    match shell {
        ShellKind::Bash => Some("$HOME/.bashrc"),
        ShellKind::Zsh => Some("$HOME/.zshrc"),
        ShellKind::Fish => Some("$HOME/.config/fish/conf.d/nyaterm-shell-integration.fish"),
        ShellKind::PosixSh | ShellKind::Unknown => None,
    }
}

pub(super) const MANAGED_BLOCK_START: &str = "# >>> nyaterm shell integration >>>";
pub(super) const MANAGED_BLOCK_END: &str = "# <<< nyaterm shell integration <<<";

pub(super) fn rc_managed_block(shell: ShellKind) -> Option<String> {
    let source_path = persistent_script_path(shell)?;
    let body = match shell {
        ShellKind::Bash | ShellKind::Zsh => format!(
            "if [ -r \"{}\" ]; then\n  . \"{}\"\nfi",
            source_path, source_path
        ),
        ShellKind::Fish => format!(
            "if test -r \"{}\"\n  source \"{}\"\nend",
            source_path, source_path
        ),
        ShellKind::PosixSh | ShellKind::Unknown => return None,
    };
    Some(format!(
        "{MANAGED_BLOCK_START}\n{body}\n{MANAGED_BLOCK_END}"
    ))
}

fn sh_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn remote_install_command(shell: ShellKind) -> Option<String> {
    let script = persistent_script(shell)?;
    let block = rc_managed_block(shell)?;
    let script_path = persistent_script_path(shell)?;
    let rc_path = rc_file_path(shell)?;
    Some(format!(
        r#"set -eu
script_path={script_path}
rc_path={rc_path}
mkdir -p "$HOME/.config/nyaterm"
case "$rc_path" in */*) mkdir -p "${{rc_path%/*}}" ;; esac
script_tmp="${{script_path}}.tmp.$$"
cat > "$script_tmp" <<'NYATERM_SCRIPT_EOF'
{script}
NYATERM_SCRIPT_EOF
if [ ! -f "$script_path" ] || ! cmp -s "$script_tmp" "$script_path"; then
  mv "$script_tmp" "$script_path"
else
  rm -f "$script_tmp"
fi
block_tmp="${{script_path}}.block.$$"
cat > "$block_tmp" <<'NYATERM_BLOCK_EOF'
{block}
NYATERM_BLOCK_EOF
rc_tmp="${{rc_path}}.tmp.$$"
start={start}
end={end}
if [ -f "$rc_path" ] && grep -F "$start" "$rc_path" >/dev/null 2>&1 && grep -F "$end" "$rc_path" >/dev/null 2>&1; then
  NYATERM_BLOCK_FILE="$block_tmp" awk -v start="$start" -v end="$end" '
    $0 == start {{
      if (!done) {{
        while ((getline line < ENVIRON["NYATERM_BLOCK_FILE"]) > 0) print line
        close(ENVIRON["NYATERM_BLOCK_FILE"])
        done=1
      }}
      skip=1
      next
    }}
    $0 == end {{ skip=0; next }}
    !skip {{ print }}
    END {{
      if (!done) {{
        if (NR > 0) print ""
        while ((getline line < ENVIRON["NYATERM_BLOCK_FILE"]) > 0) print line
      }}
    }}
  ' "$rc_path" > "$rc_tmp"
else
  if [ -f "$rc_path" ]; then
    cat "$rc_path" > "$rc_tmp"
    if [ -s "$rc_tmp" ]; then printf '\n' >> "$rc_tmp"; fi
  else
    : > "$rc_tmp"
  fi
  cat "$block_tmp" >> "$rc_tmp"
fi
if [ ! -f "$rc_path" ] || ! cmp -s "$rc_tmp" "$rc_path"; then
  if [ -f "$rc_path" ] && [ ! -f "$rc_path.nyaterm.bak" ]; then
    cp "$rc_path" "$rc_path.nyaterm.bak" 2>/dev/null || true
  fi
  mv "$rc_tmp" "$rc_path"
else
  rm -f "$rc_tmp"
fi
rm -f "$block_tmp"
"#,
        script_path = script_path,
        rc_path = rc_path,
        script = script,
        block = block,
        start = sh_single_quote(MANAGED_BLOCK_START),
        end = sh_single_quote(MANAGED_BLOCK_END),
    ))
}

async fn install_remote_shell_integration(
    handle: &SshShellHandle,
    shell: ShellKind,
    timeout_ms: u64,
) -> anyhow::Result<()> {
    let Some(command) = remote_install_command(shell) else {
        anyhow::bail!("no persistent shell integration available for {shell:?}");
    };
    exec_remote_command(handle, &command, timeout_ms)
        .await
        .map(|_| ())
}

// Precmd/preexec plumbing is adapted from bash-preexec
// (https://github.com/rcaloras/bash-preexec, MIT), using the vendored subset
// validated by temp/tty7. NyaTerm-prefixed symbols avoid replacing an existing
// bash-preexec installation.
const BASH_PERSISTENT_SCRIPT: &str = r#"# nyaterm shell integration v2
__nyaterm_host(){ hostname 2>/dev/null || printf localhost; }
__nyaterm_prune_history(){
  [ -n "${NYATERM_PRUNE_HISTORY:-}" ] || return 0
  unset NYATERM_PRUNE_HISTORY
  local hline
  hline="$(HISTTIMEFORMAT= history 1 2>/dev/null || true)"
  case "$hline" in
    (*NYATERM_PRUNE_HISTORY*|*NYATERM_INJ*|*__nyaterm_install_prompt*|*NyaTermReady*)
      if [[ "$hline" =~ ^[[:space:]]*([0-9]+) ]]; then
        history -d "${BASH_REMATCH[1]}" 2>/dev/null || true
      fi
      ;;
  esac
  NYATERM_LAST_HISTCMD="${HISTCMD-}"
}
__nyaterm_emit_command(){
  local cmd="$1"
  [ -n "$cmd" ] || return 0
  if command -v base64 >/dev/null 2>&1; then
    local b64; b64="$(printf '%s' "$cmd" | base64 | tr -d '\r\n')"
    [ -n "$b64" ] && printf '\033]7777;NyaTermCommand:%s\007' "$b64"
  fi
}
__nyaterm_emit_history_command(){
  local histcmd="${HISTCMD-}"
  if [ -n "$histcmd" ] && [ "${NYATERM_LAST_HISTCMD-}" != "$histcmd" ]; then
    NYATERM_LAST_HISTCMD="$histcmd"
    local cmd; cmd="$(fc -ln -1 2>/dev/null)"
    __nyaterm_emit_command "$cmd"
  fi
}
__nyaterm_cwd_prompt(){
  local ret=$?
  [ -n "${NYATERM_BASH_CWD_READY:-}" ] || return "$ret"
  __nyaterm_prune_history
  __nyaterm_emit_history_command
  printf '\033]7;file://%s%s\007' "$(__nyaterm_host)" "$PWD"
  return "$ret"
}
__nyaterm_precmd_d(){
  local ret=$?
  if [ -n "${NYATERM_BASH_HOOKS_READY:-}" ] && [ -n "${__nyaterm_cmd_active:-}" ]; then
    printf '\033]133;D;%s\007' "$ret"
    unset __nyaterm_cmd_active
  fi
  return "$ret"
}
__nyaterm_prompt(){
  local ret=$?
  [ -n "${NYATERM_BASH_HOOKS_READY:-}" ] || return "$ret"
  __nyaterm_prune_history
  printf '\033]7;file://%s%s\007' "$(__nyaterm_host)" "$PWD"
  printf '\033]133;A\007'
  case "$PS1" in (*133\;B*) ;; (*) PS1="$PS1"'\[\033]133;B\007\]' ;; esac
  return "$ret"
}
__nyaterm_preexec(){
  [ -n "${NYATERM_BASH_HOOKS_READY:-}" ] || return 0
  [ -z "${__nyaterm_cmd_active:-}" ] || return 0
  local cmd="$1"
  [ -n "$cmd" ] || return 0
  __nyaterm_cmd_active=1
  __nyaterm_emit_command "$cmd"
  printf '\033]133;C\007'
}
__nyaterm_array_contains(){
  local needle="$1" candidate
  shift
  for candidate; do [ "$candidate" = "$needle" ] && return 0; done
  return 1
}
__nyaterm_hook_arrays_writable(){
  local var decl
  for var; do
    decl="$(declare -p "$var" 2>/dev/null)" || return 1
    [[ "$decl" =~ ^declare\ -[^[:space:]]*a[^[:space:]]*\  ]] || return 1
    [[ ! "$decl" =~ ^declare\ -[^[:space:]]*r[^[:space:]]*\  ]] || return 1
  done
}
__nyaterm_register_full_hooks(){
  __nyaterm_array_contains __nyaterm_precmd_d "${precmd_functions[@]}" \
    || precmd_functions=(__nyaterm_precmd_d "${precmd_functions[@]}")
  __nyaterm_array_contains __nyaterm_prompt "${precmd_functions[@]}" \
    || precmd_functions+=(__nyaterm_prompt)
  __nyaterm_array_contains __nyaterm_preexec "${preexec_functions[@]}" \
    || preexec_functions+=(__nyaterm_preexec)
}
__nya_bp_require_not_readonly(){
  local var
  for var; do
    if ! ( unset "$var" 2>/dev/null ); then return 1; fi
  done
}
__nya_bp_trim_whitespace(){
  local var=${1:?} text=${2:-}
  text="${text#"${text%%[![:space:]]*}"}"
  text="${text%"${text##*[![:space:]]}"}"
  printf -v "$var" '%s' "$text"
}
__nya_bp_sanitize_string(){
  local var=${1:?} text=${2:-} sanitized
  __nya_bp_trim_whitespace sanitized "$text"
  sanitized=${sanitized%;}; sanitized=${sanitized#;}
  __nya_bp_trim_whitespace sanitized "$sanitized"
  printf -v "$var" '%s' "$sanitized"
}
__nya_bp_set_ret_value(){ return ${1:+"$1"}; }
__nya_bp_interactive_mode(){ __nya_bp_preexec_interactive_mode=on; }
__nya_bp_precmd_invoke_cmd(){
  __nya_bp_last_ret_value="$?" NYATERM_BP_PIPESTATUS=("${PIPESTATUS[@]}")
  if (( __nya_bp_inside_precmd > 0 )); then return; fi
  local __nya_bp_inside_precmd=1 precmd_function
  for precmd_function in "${precmd_functions[@]}"; do
    if type -t "$precmd_function" >/dev/null; then
      __nya_bp_set_ret_value "$__nya_bp_last_ret_value" "$__nya_bp_last_argument"
      "$precmd_function"
    fi
  done
  __nya_bp_set_ret_value "$__nya_bp_last_ret_value"
}
__nya_bp_in_prompt_command(){
  local prompt_command_array IFS=$'\n;'
  read -rd '' -a prompt_command_array <<< "${PROMPT_COMMAND[*]:-}"
  local trimmed_arg command trimmed_command
  __nya_bp_trim_whitespace trimmed_arg "${1:-}"
  for command in "${prompt_command_array[@]:-}"; do
    __nya_bp_trim_whitespace trimmed_command "$command"
    [ "$trimmed_command" = "$trimmed_arg" ] && return 0
  done
  return 1
}
__nya_bp_preexec_invoke_exec(){
  __nya_bp_last_argument="${1:-}"
  if (( __nya_bp_inside_preexec > 0 )); then return; fi
  local __nya_bp_inside_preexec=1
  [ -t 1 ] || return
  [ -z "${COMP_LINE:-}" ] || return
  [ -z "${READLINE_LINE+x}" ] || return
  if [ -z "${__nya_bp_preexec_interactive_mode:-}" ]; then
    return
  elif [ "${BASH_SUBSHELL:-0}" -eq 0 ]; then
    __nya_bp_preexec_interactive_mode=
  fi
  if __nya_bp_in_prompt_command "${BASH_COMMAND:-}"; then
    __nya_bp_preexec_interactive_mode=
    return
  fi
  local this_command
  this_command=$(LC_ALL=C HISTTIMEFORMAT='' builtin history 1 | sed '1 s/^ *[0-9][0-9]*[* ] //')
  [ -n "$this_command" ] || return
  local preexec_function preexec_ret=0 function_ret
  for preexec_function in "${preexec_functions[@]:-}"; do
    if type -t "$preexec_function" >/dev/null; then
      __nya_bp_set_ret_value "${__nya_bp_last_ret_value:-}"
      "$preexec_function" "$this_command"
      function_ret=$?
      [ "$function_ret" -eq 0 ] || preexec_ret=$function_ret
    fi
  done
  __nya_bp_set_ret_value "$preexec_ret" "$__nya_bp_last_argument"
}
__nya_bp_install(){
  case "${PROMPT_COMMAND[*]:-}" in (*__nya_bp_precmd_invoke_cmd*) return 1 ;; esac
  trap '__nya_bp_preexec_invoke_exec "$_"' DEBUG || return 1
  local prior_trap
  prior_trap=$(sed "s/[^']*'\(.*\)'[^']*/\1/" <<<"${__nya_bp_trap_string:-}")
  unset __nya_bp_trap_string
  if [ -n "$prior_trap" ]; then
    eval '__nya_bp_original_debug_trap(){ '"$prior_trap"'; }'
    preexec_functions+=(__nya_bp_original_debug_trap)
  fi
  local existing_prompt_command="${PROMPT_COMMAND:-}"
  existing_prompt_command="${existing_prompt_command//$__nya_bp_install_string/:}"
  existing_prompt_command="${existing_prompt_command//$'\n':$'\n'/$'\n'}"
  existing_prompt_command="${existing_prompt_command//$'\n':;/$'\n'}"
  __nya_bp_sanitize_string existing_prompt_command "$existing_prompt_command"
  [ "${existing_prompt_command:-:}" = : ] && existing_prompt_command=
  PROMPT_COMMAND=__nya_bp_precmd_invoke_cmd
  PROMPT_COMMAND+=${existing_prompt_command:+$'\n'$existing_prompt_command}
  if (( BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 1) )); then
    PROMPT_COMMAND+=(__nya_bp_interactive_mode)
  else
    PROMPT_COMMAND+=$'\n__nya_bp_interactive_mode'
  fi
  NYATERM_BASH_HOOKS_READY=1
  unset NYATERM_BASH_INSTALL_PENDING
  __nya_bp_precmd_invoke_cmd
  __nya_bp_interactive_mode
}
__nya_bp_install_after_session_init(){
  __nya_bp_require_not_readonly PROMPT_COMMAND HISTCONTROL HISTTIMEFORMAT || return 1
  local sanitized_prompt_command
  __nya_bp_sanitize_string sanitized_prompt_command "${PROMPT_COMMAND:-}"
  [ -z "$sanitized_prompt_command" ] || PROMPT_COMMAND=${sanitized_prompt_command}$'\n'
  PROMPT_COMMAND+=${__nya_bp_install_string}
}
__nyaterm_install_cwd(){
  [ -n "${NYATERM_BASH_CWD_READY:-}" ] && return 0
  local decl hook
  decl="$(declare -p PROMPT_COMMAND 2>/dev/null || true)"
  if [[ "$decl" =~ ^declare\ -[^[:space:]]*a[^[:space:]]*\ PROMPT_COMMAND= ]]; then
    for hook in "${PROMPT_COMMAND[@]}"; do
      [ "$hook" = __nyaterm_cwd_prompt ] && { NYATERM_BASH_CWD_READY=1; return 0; }
    done
    PROMPT_COMMAND=(__nyaterm_cwd_prompt "${PROMPT_COMMAND[@]}")
  else
    case "${PROMPT_COMMAND-}" in
      (*__nyaterm_cwd_prompt*) ;;
      (*) PROMPT_COMMAND="__nyaterm_cwd_prompt${PROMPT_COMMAND:+; $PROMPT_COMMAND}" ;;
    esac
  fi
  NYATERM_BASH_CWD_READY=1
}
__nyaterm_install_full(){
  [ -n "${NYATERM_BASH_HOOKS_READY:-}" ] && return 0
  [ -n "${NYATERM_BASH_INSTALL_PENDING:-}" ] && return 0
  if [ -n "${bash_preexec_imported:-}" ]; then
    __nyaterm_hook_arrays_writable precmd_functions preexec_functions || return 1
    __nyaterm_register_full_hooks
    NYATERM_BASH_HOOKS_READY=1
    return 0
  fi
  command -v sed >/dev/null 2>&1 || return 1
  __nya_bp_require_not_readonly \
    PROMPT_COMMAND HISTCONTROL HISTTIMEFORMAT precmd_functions preexec_functions || return 1
  bash_preexec_imported=defined
  declare -ga precmd_functions preexec_functions
  __nya_bp_last_ret_value="$?"
  __nya_bp_last_argument="$_"
  __nya_bp_inside_precmd=0
  __nya_bp_inside_preexec=0
  __nya_bp_preexec_interactive_mode=
  __nya_bp_install_string=$'__nya_bp_trap_string="$(trap -p DEBUG)"\ntrap - DEBUG\n__nya_bp_install'
  __nyaterm_register_full_hooks
  NYATERM_BASH_INSTALL_PENDING=1
  __nya_bp_install_after_session_init || { unset NYATERM_BASH_INSTALL_PENDING; return 1; }
}
__nyaterm_install_prompt(){
  NYATERM_LAST_HISTCMD="${HISTCMD-}"
  case "${1:-full}" in
    (full) __nyaterm_install_full ;;
    (cwd) __nyaterm_install_cwd ;;
    (*) return 1 ;;
  esac
}
"#;

const ZSH_PERSISTENT_SCRIPT: &str = r#"# nyaterm shell integration v2
__nyaterm_host(){ hostname 2>/dev/null || printf localhost; }
__nyaterm_emit_command(){
  if [ -n "$1" ] && command -v base64 >/dev/null 2>&1; then
    local b64; b64="$(printf '%s' "$1" | base64 | tr -d '\r\n')"
    [ -n "$b64" ] && printf '\033]7777;NyaTermCommand:%s\007' "$b64"
  fi
}
__nyaterm_cwd_prompt(){
  [ -n "${NYATERM_ZSH_CWD_READY:-}" ] || return 0
  printf '\033]7;file://%s%s\007' "$(__nyaterm_host)" "$PWD"
}
__nyaterm_cwd_preexec(){
  [ -n "${NYATERM_ZSH_CWD_READY:-}" ] || return 0
  __nyaterm_emit_command "$1"
}
__nyaterm_precmd_d(){
  local ret=$?
  [ -n "${NYATERM_ZSH_HOOKS_READY:-}" ] || return "$ret"
  if [ -n "${__nyaterm_cmd_active:-}" ]; then
    printf '\033]133;D;%s\007' "$ret"
    unset __nyaterm_cmd_active
  fi
  return "$ret"
}
__nyaterm_prompt(){
  [ -n "${NYATERM_ZSH_HOOKS_READY:-}" ] || return 0
  printf '\033]7;file://%s%s\007' "$(__nyaterm_host)" "$PWD"
  printf '\033]133;A\007'
  [[ "$PS1" == *$'\033]133;B\007'* ]] || PS1="$PS1"$'%{\033]133;B\007%}'
}
__nyaterm_preexec(){
  [ -n "${NYATERM_ZSH_HOOKS_READY:-}" ] || return 0
  __nyaterm_cmd_active=1
  __nyaterm_emit_command "$1"
  printf '\033]133;C\007'
}
__nyaterm_install_cwd(){
  [ -n "${NYATERM_ZSH_CWD_READY:-}" ] && return 0
  autoload -Uz add-zsh-hook 2>/dev/null || return 1
  add-zsh-hook -d precmd __nyaterm_cwd_prompt 2>/dev/null || true
  add-zsh-hook -d preexec __nyaterm_cwd_preexec 2>/dev/null || true
  add-zsh-hook precmd __nyaterm_cwd_prompt || return 1
  add-zsh-hook preexec __nyaterm_cwd_preexec || return 1
  typeset -g NYATERM_ZSH_CWD_READY=1
}
__nyaterm_install_full(){
  [ -n "${NYATERM_ZSH_HOOKS_READY:-}" ] && return 0
  autoload -Uz add-zsh-hook 2>/dev/null || return 1
  typeset -ga precmd_functions preexec_functions
  precmd_functions=(__nyaterm_precmd_d ${precmd_functions:#__nyaterm_precmd_d})
  add-zsh-hook -d precmd __nyaterm_prompt 2>/dev/null || true
  add-zsh-hook -d preexec __nyaterm_preexec 2>/dev/null || true
  add-zsh-hook precmd __nyaterm_prompt || return 1
  add-zsh-hook preexec __nyaterm_preexec || return 1
  typeset -g NYATERM_ZSH_HOOKS_READY=1
}
__nyaterm_install_prompt(){
  case "${1:-full}" in
    (full) __nyaterm_install_full ;;
    (cwd) __nyaterm_install_cwd ;;
    (*) return 1 ;;
  esac
}
"#;

const FISH_PERSISTENT_SCRIPT: &str = r#"# nyaterm shell integration v2
function __nyaterm_emit_command
  if test -n "$argv[1]"; and command -sq base64
    set -l b64 (printf '%s' "$argv[1]" | base64 | tr -d '\r\n')
    test -n "$b64"; and printf '\033]7777;NyaTermCommand:%s\007' "$b64"
  end
end
function __nyaterm_cwd_prompt
  set -q NYATERM_FISH_CWD_READY; or return 0
  printf '\033]7;file://%s%s\007' (hostname) $PWD
end
function __nyaterm_cwd_preexec
  set -q NYATERM_FISH_CWD_READY; or return 0
  __nyaterm_emit_command "$argv[1]"
end
function __nyaterm_prompt_end
  printf '\033]133;B\007'
end
function __nyaterm_ensure_prompt_wrapper
  if not functions fish_prompt | string match -q '*__nyaterm_prompt_end*'
    functions -e __nyaterm_original_fish_prompt 2>/dev/null
    functions -c fish_prompt __nyaterm_original_fish_prompt; or return 1
    function fish_prompt
      __nyaterm_original_fish_prompt
      __nyaterm_prompt_end
    end
  end
end
function __nyaterm_precmd
  set -q NYATERM_FISH_HOOKS_READY; or return 0
  set -l ret $status
  if set -q __nyaterm_cmd_active
    printf '\033]133;D;%s\007' "$ret"
    set -e __nyaterm_cmd_active
  end
  printf '\033]7;file://%s%s\007' (hostname) $PWD
  printf '\033]133;A\007'
  __nyaterm_ensure_prompt_wrapper
end
function __nyaterm_preexec
  set -q NYATERM_FISH_HOOKS_READY; or return 0
  set -g __nyaterm_cmd_active 1
  __nyaterm_emit_command "$argv[1]"
  printf '\033]133;C\007'
end
function __nyaterm_install_cwd
  set -q NYATERM_FISH_CWD_READY; and return 0
  functions -e __nyaterm_cwd_prompt_event __nyaterm_cwd_preexec_event 2>/dev/null
  function __nyaterm_cwd_prompt_event --on-event fish_prompt
    __nyaterm_cwd_prompt
  end
  function __nyaterm_cwd_preexec_event --on-event fish_preexec
    __nyaterm_cwd_preexec $argv
  end
  set -g NYATERM_FISH_CWD_READY 1
end
function __nyaterm_install_full
  set -q NYATERM_FISH_HOOKS_READY; and return 0
  functions -e __nyaterm_precmd_event __nyaterm_preexec_event 2>/dev/null
  function __nyaterm_precmd_event --on-event fish_prompt
    __nyaterm_precmd
  end
  function __nyaterm_preexec_event --on-event fish_preexec
    __nyaterm_preexec $argv
  end
  set -g NYATERM_FISH_HOOKS_READY 1
end
function __nyaterm_install_prompt
  switch "$argv[1]"
    case full ''
      __nyaterm_install_full
    case cwd
      __nyaterm_install_cwd
    case '*'
      return 1
  end
end
"#;

const BASH_CWD_ONLY_SCRIPT: &str = r#"# nyaterm cwd integration v2
__nyaterm_host(){ hostname 2>/dev/null || printf localhost; }
__nyaterm_prune_history(){
  [ -n "${NYATERM_PRUNE_HISTORY:-}" ] || return 0
  unset NYATERM_PRUNE_HISTORY
  local hline
  hline="$(HISTTIMEFORMAT= history 1 2>/dev/null || true)"
  case "$hline" in
    (*NYATERM_PRUNE_HISTORY*|*NYATERM_INJ*|*NyaTermReady*)
      if [[ "$hline" =~ ^[[:space:]]*([0-9]+) ]]; then
        history -d "${BASH_REMATCH[1]}" 2>/dev/null || true
      fi
      ;;
  esac
}
__nyaterm_emit_command(){
  local cmd="$1"
  [ -n "$cmd" ] && command -v base64 >/dev/null 2>&1 || return 0
  local b64; b64="$(printf '%s' "$cmd" | base64 | tr -d '\r\n')"
  [ -n "$b64" ] && printf '\033]7777;NyaTermCommand:%s\007' "$b64"
}
__nyaterm_cwd_prompt(){
  local ret=$? histcmd="${HISTCMD-}"
  [ -n "${NYATERM_BASH_CWD_READY:-}" ] || return "$ret"
  __nyaterm_prune_history
  histcmd="${HISTCMD-}"
  if [ -n "$histcmd" ] && [ "${NYATERM_LAST_HISTCMD-}" != "$histcmd" ]; then
    NYATERM_LAST_HISTCMD="$histcmd"
    __nyaterm_emit_command "$(fc -ln -1 2>/dev/null)"
  fi
  printf '\033]7;file://%s%s\007' "$(__nyaterm_host)" "$PWD"
  return "$ret"
}
__nyaterm_install_prompt(){
  [ "${1:-cwd}" = cwd ] || return 1
  [ -n "${NYATERM_BASH_CWD_READY:-}" ] && return 0
  local decl hook
  decl="$(declare -p PROMPT_COMMAND 2>/dev/null || true)"
  if [[ "$decl" =~ ^declare\ -[^[:space:]]*a[^[:space:]]*\ PROMPT_COMMAND= ]]; then
    for hook in "${PROMPT_COMMAND[@]}"; do
      [ "$hook" = __nyaterm_cwd_prompt ] && { NYATERM_BASH_CWD_READY=1; return 0; }
    done
    PROMPT_COMMAND=(__nyaterm_cwd_prompt "${PROMPT_COMMAND[@]}")
  else
    case "${PROMPT_COMMAND-}" in
      (*__nyaterm_cwd_prompt*) ;;
      (*) PROMPT_COMMAND="__nyaterm_cwd_prompt${PROMPT_COMMAND:+; $PROMPT_COMMAND}" ;;
    esac
  fi
  NYATERM_BASH_CWD_READY=1
}
"#;

const ZSH_CWD_ONLY_SCRIPT: &str = r#"# nyaterm cwd integration v2
__nyaterm_host(){ hostname 2>/dev/null || printf localhost; }
__nyaterm_emit_command(){
  [ -n "$1" ] && command -v base64 >/dev/null 2>&1 || return 0
  local b64; b64="$(printf '%s' "$1" | base64 | tr -d '\r\n')"
  [ -n "$b64" ] && printf '\033]7777;NyaTermCommand:%s\007' "$b64"
}
__nyaterm_cwd_prompt(){
  [ -n "${NYATERM_ZSH_CWD_READY:-}" ] || return 0
  printf '\033]7;file://%s%s\007' "$(__nyaterm_host)" "$PWD"
}
__nyaterm_cwd_preexec(){
  [ -n "${NYATERM_ZSH_CWD_READY:-}" ] || return 0
  __nyaterm_emit_command "$1"
}
__nyaterm_install_prompt(){
  [ "${1:-cwd}" = cwd ] || return 1
  [ -n "${NYATERM_ZSH_CWD_READY:-}" ] && return 0
  autoload -Uz add-zsh-hook 2>/dev/null || return 1
  add-zsh-hook -d precmd __nyaterm_cwd_prompt 2>/dev/null || true
  add-zsh-hook -d preexec __nyaterm_cwd_preexec 2>/dev/null || true
  add-zsh-hook precmd __nyaterm_cwd_prompt || return 1
  add-zsh-hook preexec __nyaterm_cwd_preexec || return 1
  typeset -g NYATERM_ZSH_CWD_READY=1
}
"#;

const FISH_CWD_ONLY_SCRIPT: &str = r#"# nyaterm cwd integration v2
function __nyaterm_emit_command
  if test -n "$argv[1]"; and command -sq base64
    set -l b64 (printf '%s' "$argv[1]" | base64 | tr -d '\r\n')
    test -n "$b64"; and printf '\033]7777;NyaTermCommand:%s\007' "$b64"
  end
end
function __nyaterm_cwd_prompt
  set -q NYATERM_FISH_CWD_READY; or return 0
  printf '\033]7;file://%s%s\007' (hostname) $PWD
end
function __nyaterm_cwd_preexec
  set -q NYATERM_FISH_CWD_READY; or return 0
  __nyaterm_emit_command "$argv[1]"
end
function __nyaterm_install_prompt
  test "$argv[1]" = cwd; or return 1
  set -q NYATERM_FISH_CWD_READY; and return 0
  functions -e __nyaterm_cwd_prompt_event __nyaterm_cwd_preexec_event 2>/dev/null
  function __nyaterm_cwd_prompt_event --on-event fish_prompt
    __nyaterm_cwd_prompt
  end
  function __nyaterm_cwd_preexec_event --on-event fish_preexec
    __nyaterm_cwd_preexec $argv
  end
  set -g NYATERM_FISH_CWD_READY 1
end
"#;

pub(super) struct OscStripper {
    buf: Vec<u8>,
    ready_inner: Vec<u8>,
    legacy_ready_inner: Option<Vec<u8>>,
}

impl OscStripper {
    pub(super) fn new(ready_marker: &str, legacy_ready_marker: Option<&str>) -> Self {
        Self {
            buf: Vec::new(),
            ready_inner: marker_inner(ready_marker).into_bytes(),
            legacy_ready_inner: legacy_ready_marker.map(|marker| marker_inner(marker).into_bytes()),
        }
    }

    pub(super) fn push(&mut self, chunk: &[u8]) -> OscResult {
        self.buf.extend_from_slice(chunk);
        if self.buf.len() > MAX_OSC_BUF && find_subsequence(&self.buf, b"\x1b]").is_none() {
            return OscResult {
                visible: std::mem::take(&mut self.buf),
                visible_after_ready: Vec::new(),
                cwd_paths: Vec::new(),
                ready: false,
                accepted_commands: Vec::new(),
            };
        }

        let mut visible = Vec::new();
        let mut visible_after_ready = Vec::new();
        let mut cwd_paths = Vec::new();
        let mut ready = false;
        let mut after_ready = false;
        let mut accepted_commands = Vec::new();

        loop {
            let Some(esc_pos) = find_subsequence(&self.buf, b"\x1b]") else {
                if after_ready {
                    visible_after_ready.extend_from_slice(&self.buf);
                }
                visible.extend_from_slice(&self.buf);
                self.buf.clear();
                break;
            };

            if after_ready {
                visible_after_ready.extend_from_slice(&self.buf[..esc_pos]);
            }
            visible.extend_from_slice(&self.buf[..esc_pos]);
            let rest = self.buf[esc_pos..].to_vec();
            let Some((end_idx, term_len)) = find_osc_terminator(&rest) else {
                self.buf = rest;
                if self.buf.len() > MAX_OSC_BUF {
                    visible.extend_from_slice(&self.buf);
                    self.buf.clear();
                }
                break;
            };

            let seq_end = end_idx + term_len;
            let seq = &rest[..seq_end];
            let inner = &rest[2..end_idx];
            if let Some(payload) = inner.strip_prefix(b"7;") {
                if let Some(path) = parse_osc7_payload(payload) {
                    cwd_paths.push(path);
                }
            } else if self.is_current_ready_marker(inner) {
                ready = true;
                after_ready = true;
            } else if inner.starts_with(READY_MARKER_PREFIX.as_bytes())
                || inner.starts_with(LEGACY_READY_MARKER_PREFIX.as_bytes())
            {
                // Private marker for another session. Strip it but do not mark ready.
            } else if let Some(command) = parse_command_marker(inner) {
                accepted_commands.push(command);
            } else {
                if after_ready {
                    visible_after_ready.extend_from_slice(seq);
                }
                visible.extend_from_slice(seq);
            }
            self.buf = rest[seq_end..].to_vec();
        }

        OscResult {
            visible,
            visible_after_ready,
            cwd_paths,
            ready,
            accepted_commands,
        }
    }

    pub(super) fn flush(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buf)
    }

    fn is_current_ready_marker(&self, inner: &[u8]) -> bool {
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

fn find_osc_terminator(bytes: &[u8]) -> Option<(usize, usize)> {
    bytes
        .iter()
        .position(|byte| *byte == b'\x07')
        .map(|index| (index, 1))
        .or_else(|| find_subsequence(&bytes[2..], b"\x1b\\").map(|index| (index + 2, 2)))
}

fn parse_osc7_payload(payload: &[u8]) -> Option<String> {
    let payload = std::str::from_utf8(payload).ok()?;
    let after_scheme = payload.strip_prefix("file://")?;
    let path = if after_scheme.starts_with('/') {
        after_scheme.to_string()
    } else {
        let slash = after_scheme.find('/')?;
        after_scheme[slash..].to_string()
    };
    if path.is_empty() { None } else { Some(path) }
}

fn parse_command_marker(inner: &[u8]) -> Option<String> {
    let payload = inner
        .strip_prefix(COMMAND_MARKER_PREFIX.as_bytes())
        .or_else(|| inner.strip_prefix(LEGACY_COMMAND_MARKER_PREFIX.as_bytes()))?;
    let decoded = BASE64_STANDARD.decode(payload).ok()?;
    let command = String::from_utf8(decoded).ok()?;
    if command.is_empty() {
        None
    } else {
        Some(command)
    }
}

#[cfg(test)]
pub(super) fn bytes_after_ssh_ready_marker<'a>(
    bytes: &'a [u8],
    ready_marker: &[u8],
    legacy_ready_marker: Option<&[u8]>,
) -> Option<&'a [u8]> {
    find_subsequence(bytes, ready_marker)
        .map(|index| &bytes[index + ready_marker.len()..])
        .or_else(|| {
            legacy_ready_marker.and_then(|marker| {
                find_subsequence(bytes, marker).map(|index| &bytes[index + marker.len()..])
            })
        })
}

#[cfg(test)]
pub(super) fn strip_ssh_ready_markers(
    bytes: &[u8],
    ready_marker: &[u8],
    legacy_ready_marker: Option<&[u8]>,
) -> Vec<u8> {
    let mut output = strip_one_marker(bytes, ready_marker);
    if let Some(marker) = legacy_ready_marker {
        output = strip_one_marker(&output, marker);
    }
    output
}

#[cfg(test)]
fn strip_one_marker(bytes: &[u8], marker: &[u8]) -> Vec<u8> {
    if marker.is_empty() {
        return bytes.to_vec();
    }
    let mut output = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while let Some(index) = find_subsequence(rest, marker) {
        output.extend_from_slice(&rest[..index]);
        rest = &rest[index + marker.len()..];
    }
    output.extend_from_slice(rest);
    output
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;

    use super::{
        SshShellIntegrationPhase, SshShellIntegrationState, build_legacy_ssh_ready_marker,
        build_ssh_ready_marker,
    };

    fn shell_integration_state(session_id: &str) -> SshShellIntegrationState {
        let ready_marker = build_ssh_ready_marker(session_id);
        let legacy_ready_marker = build_legacy_ssh_ready_marker(&ready_marker);
        SshShellIntegrationState::new(
            Some(b"inject-script\n".to_vec()),
            ready_marker,
            legacy_ready_marker,
        )
    }

    #[test]
    fn prompt_heuristic_rejects_sentence_like_output() {
        assert!(!super::looks_like_shell_prompt(b"price $"));
        assert!(!super::looks_like_shell_prompt(b"build finished >"));
        assert!(!super::looks_like_shell_prompt(b"100%"));
        assert!(!super::looks_like_shell_prompt(b"done#"));
        assert!(!super::looks_like_shell_prompt(b"status$"));
        assert!(super::looks_like_shell_prompt(b"user@host:~$ "));
        assert!(super::looks_like_shell_prompt(b"PS C:\\> "));
    }

    fn mark_injection_sent(state: &mut SshShellIntegrationState) {
        state.phase = SshShellIntegrationPhase::Suppressing;
        state.pending_script = None;
        state.suppress_started_at = Some(Instant::now());
    }

    #[test]
    fn wait_initial_forwards_first_output_and_keeps_injection_pending() {
        let mut state = shell_integration_state("session-1");

        let output = state.filter_output(b"Welcome to Ubuntu\r\nLast login: today\r\n");

        assert_eq!(
            output.visible,
            b"Welcome to Ubuntu\r\nLast login: today\r\n"
        );
        assert!(output.cwd_paths.is_empty());
        assert!(output.accepted_commands.is_empty());
        assert!(state.is_waiting_initial());
        assert!(state.should_inject_on_initial_delay());
    }

    #[test]
    fn async_preparation_keeps_banner_visible_before_script_is_available() {
        let ready_marker = build_ssh_ready_marker("session-1");
        let legacy_ready_marker = build_legacy_ssh_ready_marker(&ready_marker);
        let mut state =
            SshShellIntegrationState::waiting_for_integration(ready_marker, legacy_ready_marker);

        let output = state.filter_output(b"Welcome\r\nuser@host:~$ ");

        assert_eq!(output.visible, b"Welcome\r\nuser@host:~$ ");
        assert!(state.is_waiting_initial());
        assert!(!state.should_inject_on_initial_delay());

        state.set_integration_script(Some(b"integration\n".to_vec()));

        assert!(state.is_waiting_initial());
        assert!(state.should_inject_on_initial_delay());
    }

    #[test]
    fn suppressing_discards_visible_output_until_ready_marker() {
        let mut state = shell_integration_state("session-1");
        mark_injection_sent(&mut state);

        let before_ready = state.filter_output(b"echoed injection and stale prompt# ");

        assert!(before_ready.visible.is_empty());
        assert!(before_ready.cwd_paths.is_empty());
        assert!(before_ready.accepted_commands.is_empty());
        assert!(state.is_suppressing());
    }

    #[test]
    fn ready_marker_enters_normal_and_preserves_only_visible_after_ready() {
        let mut state = shell_integration_state("session-1");
        mark_injection_sent(&mut state);
        let command = BASE64_STANDARD.encode("git status");

        let output = state.filter_output(
            format!(
                "old prompt# \x1b]7;file://host/home/user\x07\
                 \x1b]7777;NyaTermCommand:{command}\x07\
                 \x1b]7777;NyaTermReady:session-1\x07prompt# "
            )
            .as_bytes(),
        );

        assert_eq!(output.visible, b"prompt# ");
        assert_eq!(output.cwd_paths, vec!["/home/user".to_string()]);
        assert_eq!(output.accepted_commands, vec!["git status".to_string()]);
        assert!(state.is_normal());
    }

    #[test]
    fn tauri_parity_keeps_motd_discards_injection_noise_and_shows_final_prompt() {
        let mut state = shell_integration_state("session-1");
        let mut visible = Vec::new();

        let initial = state.filter_output(b"Welcome to Ubuntu\r\nLast login: today\r\n");
        visible.extend_from_slice(&initial.visible);
        assert!(state.is_waiting_initial());

        mark_injection_sent(&mut state);
        let stale_prompt = state.filter_output(b"root@host:~# ");
        visible.extend_from_slice(&stale_prompt.visible);

        let ready = state.filter_output(b"\x1b]7777;NyaTermReady:session-1\x07root@host:~# ");
        visible.extend_from_slice(&ready.visible);

        assert_eq!(
            visible,
            b"Welcome to Ubuntu\r\nLast login: today\r\nroot@host:~# "
        );
        assert_eq!(count_subsequence(&visible, b"root@host:~# "), 1);
        assert!(state.is_normal());
    }

    #[test]
    fn initial_prompt_is_not_repeated_after_immediate_injection() {
        let mut state = shell_integration_state("session-1");
        let mut visible = Vec::new();

        let initial = state.filter_output(b"Welcome to Debian\r\nuser@host:~$ ");
        visible.extend_from_slice(&initial.visible);

        mark_injection_sent(&mut state);
        let ready = state.filter_output(b"\x1b]7777;NyaTermReady:session-1\x07user@host:~$ ");
        visible.extend_from_slice(&ready.visible);

        assert_eq!(count_subsequence(&visible, b"user@host:~$ "), 1);
    }

    #[test]
    fn initial_colored_prompt_is_not_repeated_after_ready_marker() {
        let mut state = shell_integration_state("session-1");
        let initial = state
            .filter_output(b"Debian banner\r\n\x1b[01;32muser@host\x1b[00m:\x1b[01;34m~\x1b[00m$ ");
        assert!(!initial.visible.is_empty());

        mark_injection_sent(&mut state);
        let ready = state.filter_output(
            b"\x1b]7777;NyaTermReady:session-1\x07\x1b[01;32muser@host\x1b[00m:\x1b[01;34m~\x1b[00m$ ",
        );

        assert!(ready.visible.is_empty());
        assert_eq!(count_subsequence(&initial.visible, b"user@host"), 1);
    }

    #[test]
    fn initial_prompt_seen_survives_fragmented_banner_output() {
        let mut state = shell_integration_state("session-1");
        let first = state.filter_output(b"Debian banner\r\nuser@host:~$ ");
        assert!(first.visible.ends_with(b"user@host:~$ "));

        // A later banner chunk must not clear the fact that the prompt was already rendered.
        let second = state.filter_output(b"\r\n");
        assert_eq!(second.visible, b"\r\n");

        mark_injection_sent(&mut state);
        let ready = state.filter_output(b"\x1b]7777;NyaTermReady:session-1\x07user@host:~$ ");
        assert!(ready.visible.is_empty());
    }

    #[test]
    fn initial_prompt_is_not_repeated_when_ready_prompt_is_split() {
        let mut state = shell_integration_state("session-1");
        let initial = state.filter_output(b"Debian banner\r\nuser@host:~$ ");
        assert!(initial.visible.ends_with(b"user@host:~$ "));

        mark_injection_sent(&mut state);
        let ready = state.filter_output(b"\x1b]7777;NyaTermReady:session-1\x07");
        assert!(ready.visible.is_empty());
        assert!(state.is_normal());

        let prompt = state.filter_output(b"user@host:~$ ");
        assert!(prompt.visible.is_empty());
    }

    #[test]
    fn initial_prompt_is_not_repeated_when_ready_prompt_is_split_into_multiple_chunks() {
        let mut state = shell_integration_state("session-1");
        let initial = state.filter_output(b"Debian banner\r\nuser@host:~$ ");
        assert!(initial.visible.ends_with(b"user@host:~$ "));

        mark_injection_sent(&mut state);
        let ready_prefix = state.filter_output(b"\x1b]7777;NyaTermReady:session-1\x07user@host:");
        let ready_middle = state.filter_output(b"~");
        let ready_suffix = state.filter_output(b"$ ");

        assert!(ready_prefix.visible.is_empty());
        assert!(ready_middle.visible.is_empty());
        assert!(ready_suffix.visible.is_empty());
    }

    #[test]
    fn normal_output_after_ready_is_not_buffered_as_a_prompt_prefix() {
        let mut state = shell_integration_state("session-1");
        let initial = state.filter_output(b"Debian banner\r\nuser@host:~$ ");
        assert!(initial.visible.ends_with(b"user@host:~$ "));

        mark_injection_sent(&mut state);
        let ready = state.filter_output(b"\x1b]7777;NyaTermReady:session-1\x07\r\n");
        assert_eq!(ready.visible, b"\r\n");

        let output = state.filter_output(b"status$");
        assert_eq!(output.visible, b"status$");
    }

    #[test]
    fn initial_delay_can_still_inject_when_remote_outputs_nothing() {
        let state = shell_integration_state("session-1");

        assert!(state.should_inject_on_initial_delay());
        assert!(state.is_waiting_initial());
    }

    #[test]
    fn suppressing_timeout_discards_buffered_output_and_enters_normal() {
        let mut state = shell_integration_state("session-1");
        mark_injection_sent(&mut state);
        state.suppress_started_at = Some(Instant::now() - Duration::from_secs(31));

        let output = state.filter_output(b"stale prompt# ");

        assert!(output.visible.is_empty());
        assert!(output.cwd_paths.is_empty());
        assert!(output.accepted_commands.is_empty());
        assert!(state.is_normal());
    }

    fn count_subsequence(haystack: &[u8], needle: &[u8]) -> usize {
        haystack
            .windows(needle.len())
            .filter(|window| *window == needle)
            .count()
    }
}
