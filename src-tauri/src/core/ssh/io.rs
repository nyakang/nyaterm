use super::client::{
    SshDiagnosticContext, SshDiagnosticStage, SshHandle, SshPostLoginConfig, SshStartupCommand,
};
use crate::config::SftpCwdFollowMode;
use crate::core::capture::OutputCaptureProcessor;
use crate::core::input::remap_del_to_bs;
use crate::core::ssh::osc::{self, OscStripper, ShellKind};
use crate::core::terminal_session::local::split_startup_passthrough;
use crate::core::terminal_session::{TerminalOutputDecoder, encode_terminal_input};
use crate::core::zmodem::{
    ZmodemAction, ZmodemDetectResult, ZmodemDetector, ZmodemDirection, ZmodemDownloadOoDrain,
    ZmodemEvent, ZmodemTransfer, ZmodemUploadDrain, start_zmodem_transfer,
};
use crate::core::{
    RecordingManager, SessionCommand, SessionCommandReceiver, SessionCommandSender,
    SessionCwdReplacement, SessionManager, SessionOutputCoalescer, SharedCwd, replace_cwd_state,
    update_cwd_if_changed,
};
use crate::error::{AppError, AppResult};
use russh::{ChannelMsg, client};
use std::{pin::Pin, sync::Arc, time::Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::time::{Duration, Sleep, timeout};

const INJECT_TIMEOUT_SECS: u64 = 30;
const INITIAL_INJECT_DELAY_MS: u64 = 500;
const SUPPRESSED_VISIBLE_FALLBACK_MAX_BYTES: usize = 64 * 1024;

#[derive(Debug)]
enum ShellDetectionResult {
    Detected { shell: ShellKind },
    NoSupportedShell,
    TimedOut,
    ChannelOpenFailed,
    ExecFailed,
}

/// Tries to detect the remote shell via an exec channel with a timeout.
///
/// Produces the same runtime outcome as the previous `Option<ShellKind>`
/// contract: only `Detected` enables integration, all other outcomes skip it.
async fn detect_shell_type<H: client::Handler>(
    handle: &mut client::Handle<H>,
    session_id: &str,
    timeout_ms: u64,
) -> ShellDetectionResult {
    tracing::info!(
        session_id = %session_id,
        timeout_ms,
        "SSH shell detection starting"
    );

    let started = Instant::now();
    let fut = async {
        let mut ch = match handle.channel_open_session().await {
            Ok(channel) => channel,
            Err(error) => {
                return Err(("channel_open", error.to_string(), 0usize));
            }
        };
        tracing::info!(
            session_id = %session_id,
            "SSH shell detection channel opened"
        );

        if let Err(error) = ch
            .exec(
                true,
                r#"printf '%s\n' "$SHELL"; ps -p $$ -o comm= 2>/dev/null || true"#,
            )
            .await
        {
            return Err(("exec", error.to_string(), 0usize));
        }
        tracing::info!(
            session_id = %session_id,
            "SSH shell detection exec request sent"
        );

        let mut output = String::new();
        let mut output_bytes = 0usize;
        while let Some(msg) = ch.wait().await {
            if let ChannelMsg::Data { ref data } = msg {
                output_bytes += data.len();
                output.push_str(&String::from_utf8_lossy(data));
            }
        }

        Ok((ShellKind::from_name(output.trim()), output_bytes))
    };

    match timeout(Duration::from_millis(timeout_ms), fut).await {
        Ok(Ok((shell, output_bytes))) if shell != ShellKind::Unknown => {
            tracing::info!(
                session_id = %session_id,
                shell = ?shell,
                elapsed_ms = started.elapsed().as_millis() as u64,
                output_bytes,
                "SSH shell detection completed"
            );
            ShellDetectionResult::Detected { shell }
        }
        Ok(Ok((_shell, output_bytes))) => {
            tracing::info!(
                session_id = %session_id,
                elapsed_ms = started.elapsed().as_millis() as u64,
                output_bytes,
                "SSH shell detection returned no supported shell"
            );
            ShellDetectionResult::NoSupportedShell
        }
        Ok(Err(("channel_open", error, _output_bytes))) => {
            tracing::warn!(
                session_id = %session_id,
                elapsed_ms = started.elapsed().as_millis() as u64,
                %error,
                "SSH shell detection channel open failed"
            );
            ShellDetectionResult::ChannelOpenFailed
        }
        Ok(Err(("exec", error, _output_bytes))) => {
            tracing::warn!(
                session_id = %session_id,
                elapsed_ms = started.elapsed().as_millis() as u64,
                %error,
                "SSH shell detection exec failed"
            );
            ShellDetectionResult::ExecFailed
        }
        Ok(Err((_stage, error, _output_bytes))) => {
            tracing::warn!(
                session_id = %session_id,
                elapsed_ms = started.elapsed().as_millis() as u64,
                %error,
                "SSH shell detection failed"
            );
            ShellDetectionResult::NoSupportedShell
        }
        Err(_) => {
            tracing::warn!(
                session_id = %session_id,
                timeout_ms,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "SSH shell detection timed out"
            );
            ShellDetectionResult::TimedOut
        }
    }
}

async fn exec_remote_command<H: client::Handler>(
    handle: &mut client::Handle<H>,
    command: &str,
    timeout_ms: u64,
) -> AppResult<String> {
    let fut = async {
        let mut ch = handle.channel_open_session().await.map_err(|error| {
            AppError::Channel(format!("Failed to open exec channel: {}", error))
        })?;
        ch.exec(true, command).await.map_err(|error| {
            AppError::Channel(format!("Failed to execute remote command: {}", error))
        })?;

        let mut output = String::new();
        let mut exit_status = None;
        while let Some(msg) = ch.wait().await {
            match msg {
                ChannelMsg::Data { ref data } | ChannelMsg::ExtendedData { ref data, .. } => {
                    output.push_str(&String::from_utf8_lossy(data));
                }
                ChannelMsg::ExitStatus {
                    exit_status: status,
                } => {
                    exit_status = Some(status);
                }
                _ => {}
            }
        }

        match exit_status.unwrap_or(0) {
            0 => Ok(output),
            status => Err(AppError::Channel(format!(
                "Remote command exited with status {status}: {output}"
            ))),
        }
    };

    timeout(Duration::from_millis(timeout_ms), fut)
        .await
        .map_err(|_| AppError::Channel("Remote command timed out".to_string()))?
}

fn sh_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn remote_install_command(shell: ShellKind) -> Option<String> {
    let script = osc::persistent_script(shell)?;
    let block = osc::rc_managed_block(shell)?;
    let script_path = osc::persistent_script_path(shell)?;
    let rc_path = osc::rc_file_path(shell)?;

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
        start = sh_single_quote(osc::MANAGED_BLOCK_START),
        end = sh_single_quote(osc::MANAGED_BLOCK_END),
    ))
}

async fn install_remote_shell_integration<H: client::Handler>(
    handle: &mut client::Handle<H>,
    shell: ShellKind,
) -> AppResult<()> {
    let Some(command) = remote_install_command(shell) else {
        return Err(AppError::Config(format!(
            "No persistent shell integration available for {shell:?}"
        )));
    };
    exec_remote_command(handle, &command, 3000)
        .await
        .map(|_| ())
}

/// Opens a PTY shell channel and detects the remote shell type.
///
/// Returns `(channel, Option<injection_script>, ready_marker)`.
/// The injection script is **not** sent here — the IO loop defers it until
/// the shell has produced its initial output (banner / MOTD) so that the
/// welcome text is not swallowed.
pub(super) async fn open_shell_channel<H: client::Handler>(
    handle: &mut client::Handle<H>,
    session_id: &str,
    x11_fake_cookie_hex: Option<&str>,
    agent_forwarding: bool,
    terminal_type: &str,
    sftp_enabled: bool,
    network_device_profile: bool,
    cwd_follow_mode: SftpCwdFollowMode,
    shell_detection_timeout_ms: u64,
    diagnostics: Option<SshDiagnosticContext>,
) -> AppResult<(
    russh::Channel<client::Msg>,
    Option<String>,
    String,
    Option<ShellKind>,
    Option<String>,
)> {
    if let Some(diagnostics) = diagnostics.as_ref() {
        diagnostics.set_stage(SshDiagnosticStage::OpeningInteractiveChannel);
    }
    tracing::info!(
        session_id = %session_id,
        "SSH interactive channel opening"
    );
    let channel = match handle.channel_open_session().await {
        Ok(channel) => {
            tracing::info!(
                session_id = %session_id,
                "SSH interactive channel opened"
            );
            channel
        }
        Err(error) => {
            tracing::warn!(
                session_id = %session_id,
                %error,
                "SSH interactive channel open failed"
            );
            return Err(AppError::Channel(format!(
                "Failed to open channel: {}",
                error
            )));
        }
    };

    let mut local_notice = None;
    if agent_forwarding {
        if let Err(error) = channel.agent_forward(false).await {
            tracing::warn!(
                session_id = %session_id,
                %error,
                "Could not enable SSH Agent forwarding"
            );
        }
    }
    if let Some(fake_cookie_hex) = x11_fake_cookie_hex {
        if let Err(error) = channel
            .request_x11(true, false, "MIT-MAGIC-COOKIE-1", fake_cookie_hex, 0)
            .await
        {
            tracing::warn!(
                session_id = %session_id,
                %error,
                "Could not enable X11 forwarding"
            );
            local_notice = Some(super::x11_forwarding::enable_failed_message());
        }
    }

    if let Some(diagnostics) = diagnostics.as_ref() {
        diagnostics.set_stage(SshDiagnosticStage::RequestingPty);
    }
    tracing::info!(
        session_id = %session_id,
        terminal = %terminal_type,
        cols = 80,
        rows = 24,
        "SSH PTY request sending"
    );
    if let Err(error) = channel
        .request_pty(false, terminal_type, 80, 24, 0, 0, &[])
        .await
    {
        tracing::warn!(
            session_id = %session_id,
            %error,
            "SSH PTY request failed"
        );
        return Err(AppError::Channel(format!("PTY request failed: {}", error)));
    }
    tracing::info!(
        session_id = %session_id,
        "SSH PTY request completed"
    );

    if let Some(diagnostics) = diagnostics.as_ref() {
        diagnostics.set_stage(SshDiagnosticStage::RequestingShell);
    }
    tracing::info!(
        session_id = %session_id,
        "SSH shell request sending"
    );
    if let Err(error) = channel.request_shell(false).await {
        tracing::warn!(
            session_id = %session_id,
            %error,
            "SSH shell request failed"
        );
        return Err(AppError::Channel(format!(
            "Shell request failed: {}",
            error
        )));
    }
    tracing::info!(
        session_id = %session_id,
        "SSH shell request completed"
    );

    if let Some(diagnostics) = diagnostics.as_ref() {
        diagnostics.set_stage(SshDiagnosticStage::PreparingIntegration);
    }
    let ready_marker = osc::build_ready_marker(session_id);

    let mut detected_shell = None;
    let injection_script = match cwd_follow_mode {
        SftpCwdFollowMode::Off => {
            let reason = if network_device_profile {
                "network_device_profile"
            } else if sftp_enabled {
                "cwd_follow_disabled"
            } else {
                "sftp_disabled"
            };
            tracing::info!(
                session_id = %session_id,
                mode = ?cwd_follow_mode,
                detected_shell = ?detected_shell,
                has_injection_script = false,
                has_ready_marker = false,
                reason,
                "SSH shell integration skipped"
            );
            None
        }
        SftpCwdFollowMode::ShellIntegration | SftpCwdFollowMode::RcFile => {
            if let Some(diagnostics) = diagnostics.as_ref() {
                diagnostics.set_stage(SshDiagnosticStage::DetectingShell);
            }
            let detection_result =
                detect_shell_type(handle, session_id, shell_detection_timeout_ms).await;
            if let Some(diagnostics) = diagnostics.as_ref() {
                diagnostics.set_stage(SshDiagnosticStage::PreparingIntegration);
            }
            match detection_result {
                ShellDetectionResult::Detected { shell: shell_kind } => {
                    detected_shell = Some(shell_kind);
                    let script = if cwd_follow_mode == SftpCwdFollowMode::RcFile {
                        match install_remote_shell_integration(handle, shell_kind).await {
                            Ok(()) => {
                                tracing::debug!(
                                    session_id = %session_id,
                                    shell = ?shell_kind,
                                    "Remote shell integration files are installed"
                                );
                                osc::activation_script(shell_kind, &ready_marker)
                            }
                            Err(error) => {
                                tracing::warn!(
                                    session_id = %session_id,
                                    shell = ?shell_kind,
                                    %error,
                                    "Failed to install remote shell integration files; falling back to session injection"
                                );
                                osc::injection_script(shell_kind, &ready_marker)
                            }
                        }
                    } else {
                        osc::injection_script(shell_kind, &ready_marker)
                    };
                    if script.is_some() {
                        tracing::info!(
                            session_id = %session_id,
                            mode = ?cwd_follow_mode,
                            detected_shell = ?detected_shell,
                            has_injection_script = true,
                            has_ready_marker = true,
                            "SSH shell integration prepared"
                        );
                    } else {
                        tracing::info!(
                            session_id = %session_id,
                            mode = ?cwd_follow_mode,
                            detected_shell = ?detected_shell,
                            has_injection_script = false,
                            has_ready_marker = false,
                            reason = "no_script_available",
                            "SSH shell integration skipped"
                        );
                    }
                    script
                }
                ShellDetectionResult::TimedOut => {
                    tracing::info!(
                        session_id = %session_id,
                        mode = ?cwd_follow_mode,
                        detected_shell = ?detected_shell,
                        has_injection_script = false,
                        has_ready_marker = false,
                        reason = "shell_detection_timeout",
                        "SSH shell integration skipped"
                    );
                    None
                }
                ShellDetectionResult::NoSupportedShell => {
                    tracing::info!(
                        session_id = %session_id,
                        mode = ?cwd_follow_mode,
                        detected_shell = ?detected_shell,
                        has_injection_script = false,
                        has_ready_marker = false,
                        reason = "shell_unknown",
                        "SSH shell integration skipped"
                    );
                    None
                }
                ShellDetectionResult::ChannelOpenFailed | ShellDetectionResult::ExecFailed => {
                    tracing::info!(
                        session_id = %session_id,
                        mode = ?cwd_follow_mode,
                        detected_shell = ?detected_shell,
                        has_injection_script = false,
                        has_ready_marker = false,
                        reason = "shell_unknown",
                        "SSH shell integration skipped"
                    );
                    None
                }
            }
        }
    };

    Ok((
        channel,
        injection_script,
        ready_marker,
        detected_shell,
        local_notice,
    ))
}

/// Injection phase state machine.
///
/// ```text
/// ┌─────────────┐  first data   ┌─────────────┐  ready marker  ┌────────┐
/// │ WaitInitial  │──────────────▶│ Suppressing  │──────────────▶│ Normal │
/// └─────────────┘  (inject sent) └─────────────┘               └────────┘
///                                       │ timeout
///                                       └──────────────────────▶│ Normal │
/// ```
///
/// When no injection script is provided we start directly in `Normal`.
#[derive(Debug, PartialEq)]
enum IoPhase {
    /// Passing through all output; waiting for the first data chunk so we
    /// can display the banner / MOTD before injecting.
    WaitInitial,
    /// Injection script sent; discarding visible output (echo) until the
    /// ready marker appears.
    Suppressing,
    /// Normal operation — strip our OSC sequences, forward everything else.
    Normal,
}

impl std::fmt::Display for IoPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WaitInitial => f.write_str("WaitInitial"),
            Self::Suppressing => f.write_str("Suppressing"),
            Self::Normal => f.write_str("Normal"),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum InjectionEvent {
    None,
    Inject,
    Ready { visible_after_ready: String },
    Failed { visible_after_ready: String },
    RuntimeFailed { visible: String },
}

#[derive(Debug, PartialEq, Eq)]
enum InjectionTimeoutEvent {
    None,
    FallbackToNormal,
}

struct PendingStartupCommand {
    input: Vec<u8>,
    delay_ms: u64,
}

/// Build renderer-supplied startup input without allowing terminal controls.
pub(super) fn build_startup_command_input(command: &str) -> Option<Vec<u8>> {
    if command.trim().is_empty() || contains_terminal_control(command) {
        return None;
    }

    let mut input = command.as_bytes().to_vec();
    if !input.ends_with(b"\r") {
        input.push(b'\r');
    }
    Some(input)
}

fn build_post_login_command_input(command: &str) -> Option<Vec<u8>> {
    if command.trim().is_empty() {
        return None;
    }

    let normalized = command.replace("\r\n", "\r").replace('\n', "\r");
    let mut input = normalized.into_bytes();
    if !input.ends_with(b"\r") {
        input.push(b'\r');
    }
    Some(input)
}

fn contains_terminal_control(value: &str) -> bool {
    value
        .chars()
        .any(|ch| matches!(ch, '\u{0000}'..='\u{001F}' | '\u{007F}'..='\u{009F}'))
}

fn arm_post_login_timer(
    phase: &IoPhase,
    pending_post_login: &Option<PendingStartupCommand>,
    post_login_deadline: &mut Option<Pin<Box<Sleep>>>,
) {
    if *phase != IoPhase::Normal {
        return;
    }

    if post_login_deadline.is_none() {
        if let Some(pending) = pending_post_login.as_ref() {
            *post_login_deadline = Some(Box::pin(tokio::time::sleep(Duration::from_millis(
                pending.delay_ms,
            ))));
        }
    }
}

fn should_send_initial_injection(phase: &IoPhase, has_pending_script: bool) -> bool {
    *phase == IoPhase::WaitInitial && has_pending_script
}

fn on_initial_injection_sent(phase: &mut IoPhase) {
    if *phase == IoPhase::WaitInitial {
        *phase = IoPhase::Suppressing;
    }
}

fn handle_injection_result(phase: &mut IoPhase, result: &osc::OscResult) -> InjectionEvent {
    match phase {
        IoPhase::WaitInitial => {
            *phase = IoPhase::Suppressing;
            InjectionEvent::Inject
        }
        IoPhase::Suppressing if result.ready_failed => {
            *phase = IoPhase::Normal;
            InjectionEvent::Failed {
                visible_after_ready: result.visible_after_ready.clone(),
            }
        }
        IoPhase::Suppressing if result.ready => {
            *phase = IoPhase::Normal;
            InjectionEvent::Ready {
                visible_after_ready: result.visible_after_ready.clone(),
            }
        }
        IoPhase::Suppressing => InjectionEvent::None,
        IoPhase::Normal if result.ready_failed => InjectionEvent::RuntimeFailed {
            visible: result.visible.clone(),
        },
        IoPhase::Normal => InjectionEvent::None,
    }
}

fn handle_injection_timeout(phase: &mut IoPhase) -> InjectionTimeoutEvent {
    match phase {
        IoPhase::WaitInitial | IoPhase::Suppressing => {
            *phase = IoPhase::Normal;
            InjectionTimeoutEvent::FallbackToNormal
        }
        IoPhase::Normal => InjectionTimeoutEvent::None,
    }
}

fn append_suppressed_visible_and_take_passthrough(buffer: &mut String, visible: &str) -> String {
    buffer.push_str(visible);
    let (mut suppressed, passthrough) = split_startup_passthrough(buffer);
    while suppressed.len() > SUPPRESSED_VISIBLE_FALLBACK_MAX_BYTES {
        let Some(first_char) = suppressed.chars().next() else {
            break;
        };
        suppressed.drain(..first_char.len_utf8());
    }
    *buffer = suppressed;
    passthrough
}

fn discard_suppressed_output(buffer: &mut String, flushed_osc_buffer: String) -> usize {
    let discarded_len = buffer.len() + flushed_osc_buffer.len();
    buffer.clear();
    discarded_len
}

#[allow(clippy::too_many_arguments)]
async fn handle_osc_result(
    app: &AppHandle,
    output: &Arc<SessionOutputCoalescer>,
    cwd_event: &str,
    cwd: &SharedCwd,
    recording_mgr: &Option<Arc<RecordingManager>>,
    session_id: &str,
    manager: &Arc<SessionManager>,
    channel: &mut russh::Channel<client::Msg>,
    pending_script: &mut Option<String>,
    inject_deadline: &mut Pin<&mut Sleep>,
    phase: &mut IoPhase,
    result: &osc::OscResult,
    suppressed_visible_fallback: &mut String,
    shell_kind: Option<ShellKind>,
    injection_sent_at: &mut Option<Instant>,
) {
    let was_suppressing = *phase == IoPhase::Suppressing;
    let injection_event = handle_injection_result(phase, result);
    let suppressed_passthrough = if was_suppressing {
        let before_ready_len = result
            .visible
            .len()
            .saturating_sub(result.visible_after_ready.len());
        append_suppressed_visible_and_take_passthrough(
            suppressed_visible_fallback,
            &result.visible[..before_ready_len],
        )
    } else {
        String::new()
    };
    match injection_event {
        InjectionEvent::Inject => {
            tracing::info!(
                session_id = %session_id,
                shell = ?shell_kind,
                phase = "WaitInitial",
                "SSH shell integration injection sending"
            );
            emit_output(
                app,
                output,
                cwd_event,
                cwd,
                recording_mgr,
                session_id,
                manager,
                result,
            )
            .await;

            if let Some(script) = pending_script.take() {
                let _ = channel.data(script.as_bytes()).await;
                *injection_sent_at = Some(Instant::now());
                tracing::info!(
                    session_id = %session_id,
                    shell = ?shell_kind,
                    phase_transition = "WaitInitial -> Suppressing",
                    "SSH shell integration injection sent"
                );
            }
            inject_deadline
                .as_mut()
                .reset(tokio::time::Instant::now() + Duration::from_secs(INJECT_TIMEOUT_SECS));
        }
        InjectionEvent::Ready {
            visible_after_ready,
        } => {
            let suppressed_visible_bytes = result
                .visible
                .len()
                .saturating_sub(visible_after_ready.len())
                + suppressed_visible_fallback.len();
            suppressed_visible_fallback.clear();
            tracing::info!(
                session_id = %session_id,
                shell = ?shell_kind,
                elapsed_ms = injection_sent_at
                    .as_ref()
                    .map(|started| started.elapsed().as_millis() as u64)
                    .unwrap_or(0),
                suppressed_visible_bytes,
                ready_after_visible_bytes = visible_after_ready.len(),
                phase_transition = "Suppressing -> Normal",
                "SSH shell integration ready marker received"
            );
            // Exact OSC 0/2 and terminal queries retain their original order
            // while source-command echo remains suppressed.
            let mut release_output = suppressed_passthrough;
            release_output.push_str(&visible_after_ready);
            emit_metadata(app, cwd_event, cwd, manager, session_id, result).await;
            emit_visible_text(output, recording_mgr, session_id, &release_output);
        }
        InjectionEvent::Failed {
            visible_after_ready,
        } => {
            let suppressed_visible_bytes = result
                .visible
                .len()
                .saturating_sub(visible_after_ready.len())
                + suppressed_visible_fallback.len();
            suppressed_visible_fallback.clear();
            let mut release_output = suppressed_passthrough;
            release_output.push_str(&visible_after_ready);
            tracing::warn!(
                session_id = %session_id,
                shell = ?shell_kind,
                elapsed_ms = injection_sent_at
                    .as_ref()
                    .map(|started| started.elapsed().as_millis() as u64)
                    .unwrap_or(0),
                suppressed_visible_bytes,
                phase_transition = "Suppressing -> Normal",
                "SSH shell integration reported installation failure; continuing in passive mode"
            );
            emit_metadata(app, cwd_event, cwd, manager, session_id, result).await;
            emit_visible_text(output, recording_mgr, session_id, &release_output);
        }
        InjectionEvent::RuntimeFailed { visible } => {
            tracing::warn!(
                session_id = %session_id,
                shell = ?shell_kind,
                "SSH shell integration failed after startup; clearing stale cwd"
            );
            emit_metadata(app, cwd_event, cwd, manager, session_id, result).await;
            let changes = replace_cwd_state(
                cwd,
                SessionCwdReplacement {
                    legacy_path: None,
                    operational_path: None,
                    presentation: None,
                },
            )
            .await;
            if changes.operational_changed {
                let _ = app.emit(cwd_event, "");
            }
            emit_visible_text(output, recording_mgr, session_id, &visible);
        }
        InjectionEvent::None if *phase == IoPhase::Normal => {
            emit_output(
                app,
                output,
                cwd_event,
                cwd,
                recording_mgr,
                session_id,
                manager,
                result,
            )
            .await;
        }
        InjectionEvent::None => {
            // Transport terminal queries and bounded OSC 0/2 in their exact
            // source order while all remaining startup text stays hidden.
            emit_visible_text(output, recording_mgr, session_id, &suppressed_passthrough);
            emit_metadata(app, cwd_event, cwd, manager, session_id, result).await;
        }
    }
}

pub(super) async fn ssh_io_loop(
    app: AppHandle,
    session_id: String,
    manager: Arc<SessionManager>,
    mut channel: russh::Channel<client::Msg>,
    _handle: SshHandle,
    mut cmd_rx: SessionCommandReceiver,
    output_control_tx: SessionCommandSender,
    cwd: SharedCwd,
    connection_id: Option<String>,
    injection_script: Option<String>,
    ready_marker: String,
    shell_kind: Option<ShellKind>,
    post_login: Option<SshPostLoginConfig>,
    startup_command: Option<SshStartupCommand>,
    backspace_mode: String,
    initial_notice: Option<String>,
    encoding: String,
    diagnostics: Option<SshDiagnosticContext>,
) {
    let backspace_as_bs = backspace_mode == "ctrl_h";
    let output_event = format!("terminal-output-{}", session_id);
    let cwd_event = format!("cwd-changed-{}", session_id);
    let closed_event = format!("session-closed-{}", session_id);

    let recording_mgr: Option<Arc<RecordingManager>> = app
        .try_state::<Arc<RecordingManager>>()
        .map(|state| state.inner().clone());
    let output =
        SessionOutputCoalescer::for_app(app.clone(), output_event.clone(), output_control_tx);
    let mut output_decoder = TerminalOutputDecoder::new(&encoding);
    if let Some(notice) = initial_notice {
        output.push_owned(notice);
    }
    let mut stripper = OscStripper::new(&ready_marker);

    let mut capture_processor = OutputCaptureProcessor::new();

    let has_injection_script = injection_script.is_some();
    let mut phase = if has_injection_script {
        IoPhase::WaitInitial
    } else {
        IoPhase::Normal
    };
    if let Some(diagnostics) = diagnostics.as_ref() {
        diagnostics.set_stage(SshDiagnosticStage::IoRunning);
    }
    tracing::info!(
        session_id = %session_id,
        initial_phase = %phase,
        has_injection_script,
        shell_kind = ?shell_kind,
        "SSH IO loop started"
    );
    let mut suppressed_visible_fallback = String::new();
    let mut pending_script = injection_script;
    let mut initial_remote_data_logged = false;
    let mut injection_sent_at: Option<Instant> = None;
    let mut pending_post_login = post_login.and_then(|config| {
        build_post_login_command_input(&config.command).map(|input| PendingStartupCommand {
            input,
            delay_ms: config.delay_ms,
        })
    });
    let mut pending_startup_command = startup_command.and_then(|config| {
        build_startup_command_input(&config.command).map(|input| PendingStartupCommand {
            input,
            delay_ms: config.delay_ms,
        })
    });
    if pending_post_login.is_none() {
        pending_post_login = pending_startup_command.take();
    }
    let mut post_login_deadline: Option<Pin<Box<Sleep>>> = None;
    arm_post_login_timer(&phase, &pending_post_login, &mut post_login_deadline);
    let mut remote_exit_status: Option<u32> = None;
    let mut remote_exit_signal: Option<String> = None;
    let mut output_paused = false;

    let mut zmodem_detector = ZmodemDetector::new();
    let mut zmodem_transfer: Option<ZmodemTransfer> = None;
    let mut zmodem_upload_drain = ZmodemUploadDrain::new();
    let mut zmodem_download_oo_drain = ZmodemDownloadOoDrain::new();
    let zmodem_event_name = format!("zmodem-event-{session_id}");

    let initial_inject_deadline =
        tokio::time::sleep(std::time::Duration::from_millis(INITIAL_INJECT_DELAY_MS));
    tokio::pin!(initial_inject_deadline);

    let inject_deadline = tokio::time::sleep(std::time::Duration::from_secs(INJECT_TIMEOUT_SECS));
    tokio::pin!(inject_deadline);

    let close_reason = loop {
        tokio::select! {
            biased;

            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(SessionCommand::AttachConfirmed { ack }) => {
                        output.attach_confirmed(ack);
                    }
                    Some(SessionCommand::DetachRenderer) => {
                        output.detach();
                    }
                    Some(SessionCommand::Write { mut data, .. }) => {
                        if zmodem_transfer.is_some()
                            || zmodem_upload_drain.should_suppress(std::time::Instant::now())
                        {
                            continue;
                        }
                        if backspace_as_bs {
                            remap_del_to_bs(&mut data);
                        }
                        let send_data = encode_terminal_input(&data, &encoding);
                        let _ = channel.data(&send_data[..]).await;
                    }
                    Some(SessionCommand::Resize { cols, rows }) => {
                        let _ = channel.window_change(cols, rows, 0, 0).await;
                    }
                    Some(SessionCommand::PauseOutput) => {
                        output_paused = true;
                    }
                    Some(SessionCommand::ResumeOutput) => {
                        output_paused = false;
                    }
                    Some(SessionCommand::AckOutput { bytes }) => {
                        output.ack(bytes);
                    }
                    Some(SessionCommand::CaptureExec { marker_id, wrapped_command, result_tx }) => {
                        capture_processor.register(marker_id, result_tx);
                        let send_command = encode_terminal_input(&wrapped_command, &encoding);
                        let _ = channel.data(&send_command[..]).await;
                    }
                    Some(SessionCommand::CancelCapture { marker_id }) => {
                        capture_processor.cancel(&marker_id);
                    }
                    Some(SessionCommand::Close) => {
                        let _ = channel.close().await;
                        break "local-close-request";
                    }
                    Some(SessionCommand::ZmodemAcceptDownload { save_dir }) => {
                        if let Some(ref mut transfer) = zmodem_transfer {
                            let actions = transfer.accept_download(save_dir);
                            handle_zmodem_actions(&app, &zmodem_event_name, &mut channel, actions).await;
                            if transfer.is_done() {
                                zmodem_transfer = None;
                            }
                        }
                    }
                    Some(SessionCommand::ZmodemAcceptUpload {
                        files,
                        conflict_mode,
                        preserve_timestamps,
                    }) => {
                        if let Some(ref mut transfer) = zmodem_transfer {
                            let actions =
                                transfer.accept_upload(files, conflict_mode, preserve_timestamps);
                            handle_zmodem_actions(&app, &zmodem_event_name, &mut channel, actions).await;
                            if transfer.is_done() {
                                zmodem_transfer = None;
                            }
                        } else {
                            tracing::warn!(
                                session_id = %session_id,
                                "Received ZmodemAcceptUpload without an active transfer"
                            );
                        }
                    }
                    Some(SessionCommand::ZmodemCancel) => {
                        manager.clear_pending_zmodem_upload(&session_id).await;
                        if let Some(ref mut transfer) = zmodem_transfer {
                            let actions = transfer.cancel();
                            handle_zmodem_actions(&app, &zmodem_event_name, &mut channel, actions).await;
                        }
                        zmodem_transfer = None;
                    }
                    None => {
                        let _ = channel.close().await;
                        break "session-command-channel-closed";
                    }
                }
            }
            msg = channel.wait(), if !output_paused => {
                match msg {
                    Some(ChannelMsg::Data { ref data }) => {
                        if !initial_remote_data_logged {
                            initial_remote_data_logged = true;
                            tracing::info!(
                                session_id = %session_id,
                                bytes = data.len(),
                                phase = %phase,
                                "SSH terminal received initial remote data"
                            );
                        }
                        // ZMODEM: if a transfer is active, route raw bytes to it.
                        if let Some(ref mut transfer) = zmodem_transfer {
                            let direction = transfer.direction();
                            let actions = transfer.feed_incoming(data);
                            handle_zmodem_actions(&app, &zmodem_event_name, &mut channel, actions).await;
                            if transfer.is_done() {
                                zmodem_transfer = None;
                                zmodem_detector.reset();
                                if direction == ZmodemDirection::Upload {
                                    zmodem_upload_drain.start(std::time::Instant::now());
                                } else if direction == ZmodemDirection::Download {
                                    zmodem_download_oo_drain.start(std::time::Instant::now());
                                }
                            }
                            continue;
                        }

                        let data = zmodem_upload_drain.filter(data, std::time::Instant::now());
                        if data.is_empty() {
                            continue;
                        }
                        let data = zmodem_download_oo_drain
                            .filter(data, std::time::Instant::now());
                        if data.is_empty() {
                            continue;
                        }

                        // ZMODEM: detect header in raw bytes before lossy UTF-8 conversion.
                        // Detection must run in every phase so an early `rz` is not missed
                        // while shell-integration output is still being suppressed.
                        match zmodem_detector.feed(data) {
                                ZmodemDetectResult::Detected { direction, passthrough, initial_bytes } => {
                                    // Forward any pre-header bytes to the terminal.
                                    if !passthrough.is_empty() {
                                        if let Some(ref recorder) = recording_mgr {
                                            recorder.write_raw_output(&session_id, &passthrough);
                                        }
                                        let pre = output_decoder.decode(&passthrough);
                                        if !pre.is_empty() {
                                            output.push_owned(pre);
                                        }
                                    }
                                    let prepared_upload = if direction == ZmodemDirection::Upload {
                                        manager.take_pending_zmodem_upload(&session_id).await
                                    } else {
                                        None
                                    };
                                    let (transfer, bootstrap_actions) =
                                        start_zmodem_transfer(direction, &initial_bytes, prepared_upload);
                                    zmodem_transfer = Some(transfer);
                                    handle_zmodem_actions(
                                        &app,
                                        &zmodem_event_name,
                                        &mut channel,
                                        bootstrap_actions,
                                    )
                                    .await;
                                    let _ = app.emit(&zmodem_event_name, &ZmodemEvent::Detected { direction });
                                    tracing::info!(
                                        session_id = %session_id,
                                        ?direction,
                                        "ZMODEM transfer detected"
                                    );
                                    continue;
                                }
                                ZmodemDetectResult::NoMatch { passthrough } if passthrough.is_empty() => {
                                    continue;
                                }
                                ZmodemDetectResult::NoMatch { passthrough } => {
                                    if let Some(ref recorder) = recording_mgr {
                                        recorder.write_raw_output(&session_id, &passthrough);
                                    }
                                    let text = output_decoder.decode(&passthrough);
                                    let mut result = stripper.push(&text);

                                    if capture_processor.has_active() {
                                        result.visible = capture_processor.process(&result.visible);
                                    }

                                    handle_osc_result(
                                        &app,
                                        &output,
                                        &cwd_event,
                                        &cwd,
                                        &recording_mgr,
                                        &session_id,
                                        &manager,
                                        &mut channel,
                                        &mut pending_script,
                                        &mut inject_deadline,
                                        &mut phase,
                                        &result,
                                        &mut suppressed_visible_fallback,
                                        shell_kind,
                                        &mut injection_sent_at,
                                    ).await;
                                    arm_post_login_timer(
                                        &phase,
                                        &pending_post_login,
                                        &mut post_login_deadline,
                                    );
                                    continue;
                                }
                            }
                    }
                    Some(ChannelMsg::ExtendedData { ref data, .. }) => {
                        if let Some(ref recorder) = recording_mgr {
                            recorder.write_raw_output(&session_id, data);
                        }
                        let text = output_decoder.decode(data);
                        if let Some(ref recorder) = recording_mgr {
                            recorder.write_output(&session_id, &text);
                        }
                        output.push_owned(text);
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        remote_exit_status = Some(exit_status);
                        tracing::info!(
                            session_id = %session_id,
                            exit_status,
                            "SSH remote process reported exit status"
                        );
                    }
                    Some(ChannelMsg::ExitSignal {
                        signal_name,
                        core_dumped,
                        error_message,
                        lang_tag,
                    }) => {
                        remote_exit_signal = Some(format!("{signal_name:?}"));
                        tracing::warn!(
                            session_id = %session_id,
                            signal = ?signal_name,
                            core_dumped,
                            error_message = %error_message,
                            lang_tag = %lang_tag,
                            "SSH remote process exited on signal"
                        );
                    }
                    Some(ChannelMsg::Eof) => {
                        tracing::info!(
                            session_id = %session_id,
                            "SSH channel received EOF from remote"
                        );
                        break "remote-channel-eof";
                    }
                    Some(ChannelMsg::Close) => {
                        tracing::info!(
                            session_id = %session_id,
                            "SSH channel received close from remote"
                        );
                        break "remote-channel-close";
                    }
                    None => {
                        tracing::info!(
                            session_id = %session_id,
                            "SSH channel stream ended"
                        );
                        break "channel-stream-ended";
                    }
                    _ => {}
                }
            }
            _ = &mut initial_inject_deadline, if should_send_initial_injection(&phase, pending_script.is_some()) => {
                if let Some(script) = pending_script.take() {
                    tracing::info!(
                        session_id = %session_id,
                        shell = ?shell_kind,
                        phase = %phase,
                        "SSH shell integration injection sending"
                    );
                    let _ = channel.data(script.as_bytes()).await;
                    injection_sent_at = Some(Instant::now());
                    tracing::info!(
                        session_id = %session_id,
                        shell = ?shell_kind,
                        phase_transition = "WaitInitial -> Suppressing",
                        "SSH shell integration injection sent"
                    );
                    on_initial_injection_sent(&mut phase);
                    inject_deadline.as_mut().reset(
                        tokio::time::Instant::now()
                            + std::time::Duration::from_secs(INJECT_TIMEOUT_SECS),
                    );
                }
            }
            _ = &mut inject_deadline, if phase != IoPhase::Normal => {
                let previous_phase = phase.to_string();
                let timeout_event = handle_injection_timeout(&mut phase);
                let flushed = stripper.flush();
                let osc_buffer_bytes = flushed.len();
                let suppressed_visible_bytes = suppressed_visible_fallback.len();
                let discarded_visible_bytes = discard_suppressed_output(
                    &mut suppressed_visible_fallback,
                    flushed,
                );
                let phase_transition = format!("{previous_phase} -> {phase}");
                tracing::warn!(
                    session_id = %session_id,
                    shell = ?shell_kind,
                    elapsed_ms = injection_sent_at
                        .as_ref()
                        .map(|started| started.elapsed().as_millis() as u64)
                        .unwrap_or(0),
                    suppressed_visible_bytes,
                    osc_buffer_bytes,
                    discarded_visible_bytes,
                    phase_transition = %phase_transition,
                    "SSH shell integration injection timed out"
                );
                let _ = timeout_event;
                arm_post_login_timer(&phase, &pending_post_login, &mut post_login_deadline);
            }
            _ = async {
                if let Some(deadline) = post_login_deadline.as_mut() {
                    deadline.as_mut().await;
                }
            }, if post_login_deadline.is_some() => {
                post_login_deadline = None;
                if zmodem_transfer.is_some() {
                    post_login_deadline = Some(Box::pin(tokio::time::sleep(
                        Duration::from_millis(250),
                    )));
                    continue;
                }
                if let Some(pending) = pending_post_login.take() {
                    let _ = channel.data(&pending.input[..]).await;
                    tracing::info!(
                        session_id = %session_id,
                        delay_ms = pending.delay_ms,
                        "Sent SSH post-login command"
                    );
                    arm_post_login_timer(
                        &phase,
                        &pending_startup_command,
                        &mut post_login_deadline,
                    );
                    continue;
                }
                if let Some(pending) = pending_startup_command.take() {
                    let _ = channel.data(&pending.input[..]).await;
                    tracing::info!(
                        session_id = %session_id,
                        delay_ms = pending.delay_ms,
                        "Sent SSH startup command"
                    );
                }
            }
        }
    };

    output.close();

    if let Some(ref recorder) = recording_mgr {
        recorder.cleanup_session(&session_id);
    }

    manager.remove_session(&session_id).await;

    if let Some(ref conn_id) = connection_id {
        if let Some(tunnel_mgr) = app.try_state::<Arc<super::TunnelManager>>() {
            tunnel_mgr
                .close_auto_tunnels_for_connection(&app, conn_id)
                .await;
        }
    }

    tracing::info!(
        session_id = %session_id,
        close_reason,
        remote_exit_status,
        remote_exit_signal = remote_exit_signal.as_deref(),
        "SSH session closed"
    );
    let _ = app.emit(&closed_event, ());
}

async fn handle_zmodem_actions(
    app: &AppHandle,
    event_name: &str,
    channel: &mut russh::Channel<client::Msg>,
    actions: Vec<ZmodemAction>,
) {
    for action in actions {
        match action {
            ZmodemAction::SendToRemote(data) => {
                let _ = channel.data(&data[..]).await;
            }
            ZmodemAction::EmitEvent(event) => {
                let _ = app.emit(event_name, &event);
            }
        }
    }
}

/// Helper: emit visible text + CWD updates from an [`OscResult`].
async fn emit_output(
    app: &AppHandle,
    output: &Arc<SessionOutputCoalescer>,
    cwd_event: &str,
    cwd: &SharedCwd,
    recording_mgr: &Option<Arc<RecordingManager>>,
    session_id: &str,
    manager: &Arc<SessionManager>,
    result: &osc::OscResult,
) {
    emit_metadata(app, cwd_event, cwd, manager, session_id, result).await;
    emit_visible_text(output, recording_mgr, session_id, &result.visible);
}

async fn emit_metadata(
    app: &AppHandle,
    cwd_event: &str,
    cwd: &SharedCwd,
    manager: &Arc<SessionManager>,
    session_id: &str,
    result: &osc::OscResult,
) {
    for path in &result.cwd_paths {
        if let Some(next_cwd) = update_cwd_if_changed(cwd, path).await {
            let _ = app.emit(cwd_event, &next_cwd);
        }
    }

    for command in &result.accepted_commands {
        let accepted = manager
            .confirm_command_submission(session_id, command.clone())
            .await;
        if accepted {
            let _ = app.emit(
                "session-command-accepted",
                serde_json::json!({
                    "sessionId": session_id,
                    "command": command,
                }),
            );
        }
    }
}

fn emit_visible_text(
    output: &Arc<SessionOutputCoalescer>,
    recording_mgr: &Option<Arc<RecordingManager>>,
    session_id: &str,
    visible: &str,
) {
    if visible.is_empty() {
        return;
    }

    if let Some(recorder) = recording_mgr {
        recorder.write_output(session_id, visible);
    }

    output.push(visible);
}

#[cfg(test)]
mod tests {
    use super::{
        INITIAL_INJECT_DELAY_MS, INJECT_TIMEOUT_SECS, InjectionEvent, InjectionTimeoutEvent,
        IoPhase, PendingStartupCommand, SUPPRESSED_VISIBLE_FALLBACK_MAX_BYTES,
        append_suppressed_visible_and_take_passthrough, build_post_login_command_input,
        build_startup_command_input, discard_suppressed_output, handle_injection_result,
        handle_injection_timeout, open_shell_channel, should_send_initial_injection,
    };
    use crate::config::SftpCwdFollowMode;
    use crate::core::ssh::osc::{OscResult, OscStripper, build_ready_marker};
    use russh::{Channel, ChannelId, Disconnect, client, server};
    use std::pin::Pin;
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use tokio::time::{Duration, Sleep, timeout};

    struct TestClient;

    impl client::Handler for TestClient {
        type Error = russh::Error;

        async fn check_server_key(
            &mut self,
            _server_public_key: &russh::keys::PublicKey,
        ) -> Result<bool, Self::Error> {
            Ok(true)
        }
    }

    struct TestServer {
        agent_request_tx: mpsc::UnboundedSender<()>,
    }

    impl server::Handler for TestServer {
        type Error = russh::Error;

        async fn auth_none(&mut self, _user: &str) -> Result<server::Auth, Self::Error> {
            Ok(server::Auth::Accept)
        }

        async fn channel_open_session(
            &mut self,
            _channel: Channel<server::Msg>,
            reply: server::ChannelOpenHandle,
            _session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            reply.accept().await;
            Ok(())
        }

        async fn agent_request(
            &mut self,
            _channel: ChannelId,
            _session: &mut server::Session,
        ) -> Result<bool, Self::Error> {
            let _ = self.agent_request_tx.send(());
            Ok(true)
        }
    }

    async fn assert_agent_forwarding_request(enabled: bool) {
        let (client_stream, server_stream) = tokio::io::duplex(1024 * 1024);
        let (agent_request_tx, mut agent_request_rx) = mpsc::unbounded_channel();
        let mut rng = russh::keys::key::safe_rng();
        let server_config = Arc::new(server::Config {
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            keys: vec![
                russh::keys::PrivateKey::random(&mut rng, russh::keys::Algorithm::Ed25519)
                    .expect("test server key"),
            ],
            ..server::Config::default()
        });
        let server_task = tokio::spawn(async move {
            let session = server::run_stream(
                server_config,
                server_stream,
                TestServer { agent_request_tx },
            )
            .await
            .expect("fake SSH server handshake");
            session.await.expect("fake SSH server session");
        });

        let mut handle = client::connect_stream(
            Arc::new(client::Config::default()),
            client_stream,
            TestClient,
        )
        .await
        .expect("fake SSH client handshake");
        assert!(
            handle
                .authenticate_none("test")
                .await
                .expect("none authentication")
                .success()
        );

        let (channel, ..) = open_shell_channel(
            &mut handle,
            "agent-forwarding-test",
            None,
            enabled,
            "xterm-256color",
            false,
            false,
            SftpCwdFollowMode::Off,
            100,
            None,
        )
        .await
        .expect("interactive shell channel");

        if enabled {
            timeout(Duration::from_secs(1), agent_request_rx.recv())
                .await
                .expect("fake server should receive Agent forwarding request")
                .expect("Agent forwarding request channel");
        } else {
            assert!(
                timeout(Duration::from_millis(100), agent_request_rx.recv())
                    .await
                    .is_err(),
                "disabled forwarding must not send auth-agent-req@openssh.com"
            );
        }

        drop(channel);
        handle
            .disconnect(Disconnect::ByApplication, "test complete", "")
            .await
            .expect("disconnect fake SSH session");
        timeout(Duration::from_secs(1), server_task)
            .await
            .expect("fake SSH server should stop")
            .expect("fake SSH server task");
    }

    #[tokio::test]
    async fn interactive_shell_requests_agent_forwarding_only_when_enabled() {
        assert_agent_forwarding_request(false).await;
        assert_agent_forwarding_request(true).await;
    }

    #[test]
    fn post_login_input_normalizes_line_endings_and_adds_enter() {
        let input = build_post_login_command_input("cd /opt/app\nclear").expect("input");

        assert_eq!(input, b"cd /opt/app\rclear\r");
    }

    #[test]
    fn post_login_input_preserves_existing_trailing_enter() {
        let input = build_post_login_command_input("uptime\r").expect("input");

        assert_eq!(input, b"uptime\r");
    }

    #[test]
    fn post_login_input_ignores_blank_commands() {
        assert!(build_post_login_command_input(" \n\t ").is_none());
    }

    #[test]
    fn startup_input_rejects_c0_del_and_c1_controls() {
        for code in 0x00..=0x1f {
            let command = format!("cd /tmp/a{}b", char::from_u32(code).expect("C0 code point"));
            assert!(
                build_startup_command_input(&command).is_none(),
                "C0 control U+{code:04X} must be rejected"
            );
        }
        for code in 0x7f..=0x9f {
            let command = format!("cd /tmp/a{}b", char::from_u32(code).expect("C1 code point"));
            assert!(
                build_startup_command_input(&command).is_none(),
                "DEL/C1 control U+{code:04X} must be rejected"
            );
        }
    }

    #[test]
    fn startup_input_adds_enter_to_a_safe_command() {
        let input = build_startup_command_input("cd '/opt/app'").expect("input");

        assert_eq!(input, b"cd '/opt/app'\r");
    }

    fn osc_result(ready: bool, visible_after_ready: &str) -> OscResult {
        OscResult {
            visible: "suppressed output".to_string(),
            visible_after_ready: visible_after_ready.to_string(),
            cwd_paths: Vec::new(),
            cwd_payloads: Vec::new(),
            cwd_payload_invalidated: false,
            cwd_payload_events: Vec::new(),
            ready,
            ready_failed: false,
            accepted_commands: Vec::new(),
        }
    }

    #[test]
    fn initial_inject_delay_is_500ms_and_wait_initial_can_inject_without_output() {
        assert_eq!(INITIAL_INJECT_DELAY_MS, 500);
        assert!(should_send_initial_injection(&IoPhase::WaitInitial, true));
        assert!(!should_send_initial_injection(&IoPhase::WaitInitial, false));
        assert!(!should_send_initial_injection(&IoPhase::Suppressing, true));
        assert!(!should_send_initial_injection(&IoPhase::Normal, true));
    }

    #[test]
    fn ready_marker_in_suppressing_enters_normal_and_preserves_prompt_after_ready() {
        let mut phase = IoPhase::Suppressing;
        let result = osc_result(true, "[user@host ~]$ ");

        let event = handle_injection_result(&mut phase, &result);

        assert_eq!(phase, IoPhase::Normal);
        assert_eq!(
            event,
            InjectionEvent::Ready {
                visible_after_ready: "[user@host ~]$ ".to_string()
            }
        );
    }

    #[test]
    fn failure_marker_in_suppressing_enters_normal_and_preserves_prompt() {
        let mut phase = IoPhase::Suppressing;
        let mut result = osc_result(false, "prompt-after-failure");
        result.ready_failed = true;

        let event = handle_injection_result(&mut phase, &result);

        assert_eq!(phase, IoPhase::Normal);
        assert_eq!(
            event,
            InjectionEvent::Failed {
                visible_after_ready: "prompt-after-failure".to_string()
            }
        );
    }

    #[test]
    fn failure_marker_after_startup_preserves_visible_output_and_reports_runtime_failure() {
        let mut phase = IoPhase::Normal;
        let mut result = osc_result(false, "prompt-after-failure");
        result.visible = "beforeprompt-after-failure".to_string();
        result.ready_failed = true;

        let event = handle_injection_result(&mut phase, &result);

        assert_eq!(phase, IoPhase::Normal);
        assert_eq!(
            event,
            InjectionEvent::RuntimeFailed {
                visible: "beforeprompt-after-failure".to_string()
            }
        );
    }

    #[test]
    fn suppression_transport_preserves_interleaved_title_query_order() {
        let mut buffer = String::new();
        let controls = concat!(
            "\x1b]2;first\x07",
            "\x1b[c",
            "\x1b]0;final\x1b\\",
            "\x1b]11;?\x07"
        );

        let passthrough = append_suppressed_visible_and_take_passthrough(
            &mut buffer,
            &format!("hidden{controls}tail"),
        );

        assert_eq!(passthrough, controls);
        assert_eq!(buffer, "hiddentail");
    }

    #[test]
    fn oversized_startup_queries_and_titles_remain_suppressed() {
        let mut buffer = String::new();
        let huge_dcs = format!("\x1bP$q{}\x1b\\", "x".repeat(17 * 1024));
        let huge_title = format!("\x1b]2;{}\x07", "x".repeat(17 * 1024));

        let passthrough = append_suppressed_visible_and_take_passthrough(
            &mut buffer,
            &format!("{huge_dcs}{huge_title}"),
        );

        assert!(passthrough.is_empty());
        assert!(buffer.len() <= SUPPRESSED_VISIBLE_FALLBACK_MAX_BYTES);
    }

    #[test]
    fn suppression_title_transport_is_exactly_once_across_split_ready_and_normal_phases() {
        let ready_marker = build_ready_marker("session-1");
        let mut stripper = OscStripper::new(&ready_marker);
        let mut phase = IoPhase::Suppressing;
        let mut buffer = String::new();
        let mut emitted = String::new();

        let incomplete = stripper.push("hidden source\x1b]2;split");
        let _ = handle_injection_result(&mut phase, &incomplete);
        emitted.push_str(&append_suppressed_visible_and_take_passthrough(
            &mut buffer,
            &incomplete.visible,
        ));

        let completed = stripper.push(" title\x07hidden tail");
        let _ = handle_injection_result(&mut phase, &completed);
        emitted.push_str(&append_suppressed_visible_and_take_passthrough(
            &mut buffer,
            &completed.visible,
        ));

        let ready = stripper.push(&format!("{ready_marker}[user@host]$ "));
        let event = handle_injection_result(&mut phase, &ready);
        let before_ready_len = ready.visible.len() - ready.visible_after_ready.len();
        emitted.push_str(&append_suppressed_visible_and_take_passthrough(
            &mut buffer,
            &ready.visible[..before_ready_len],
        ));
        assert!(matches!(event, InjectionEvent::Ready { .. }));
        emitted.push_str(&ready.visible_after_ready);
        buffer.clear();

        let normal = stripper.push("\x1b]2;normal title\x07prompt");
        let event = handle_injection_result(&mut phase, &normal);
        assert_eq!(event, InjectionEvent::None);
        emitted.push_str(&normal.visible);

        assert_eq!(phase, IoPhase::Normal);
        assert_eq!(emitted.matches("\x1b]2;split title\x07").count(), 1);
        assert_eq!(emitted.matches("\x1b]2;normal title\x07").count(), 1);
        assert!(!emitted.contains("hidden source"));
        assert!(!emitted.contains("hidden tail"));
        assert!(
            emitted.find("\x1b]2;split title\x07").unwrap()
                < emitted.find("[user@host]$ ").unwrap()
        );
        assert!(
            emitted.find("[user@host]$ ").unwrap()
                < emitted.find("\x1b]2;normal title\x07").unwrap()
        );
    }

    #[test]
    fn title_transport_cap_is_per_sequence_and_keeps_final_semantics() {
        let first = format!("\x1b]2;{}\x07", "x".repeat(16_395));
        let final_title = "\x1b]2;final\x07";
        let mut buffer = String::new();

        let passthrough = append_suppressed_visible_and_take_passthrough(
            &mut buffer,
            &format!("{first}{final_title}"),
        );

        assert_eq!(passthrough, format!("{first}{final_title}"));
        assert!(passthrough.ends_with(final_title));
        assert!(buffer.is_empty());
    }

    #[test]
    fn delivered_suppression_title_survives_timeout_fallback() {
        let mut buffer = String::new();
        let delivered = append_suppressed_visible_and_take_passthrough(
            &mut buffer,
            "\x1b]2;delivered\x07hidden",
        );
        let mut phase = IoPhase::Suppressing;

        assert_eq!(
            handle_injection_timeout(&mut phase),
            InjectionTimeoutEvent::FallbackToNormal
        );
        assert_eq!(delivered, "\x1b]2;delivered\x07");
        assert_eq!(buffer, "hidden");
        assert_eq!(phase, IoPhase::Normal);
    }

    #[test]
    fn injection_timeout_is_30s_and_falls_back_to_normal() {
        assert_eq!(INJECT_TIMEOUT_SECS, 30);

        let mut wait_initial = IoPhase::WaitInitial;
        assert_eq!(
            handle_injection_timeout(&mut wait_initial),
            InjectionTimeoutEvent::FallbackToNormal
        );
        assert_eq!(wait_initial, IoPhase::Normal);

        let mut suppressing = IoPhase::Suppressing;
        assert_eq!(
            handle_injection_timeout(&mut suppressing),
            InjectionTimeoutEvent::FallbackToNormal
        );
        assert_eq!(suppressing, IoPhase::Normal);
    }

    #[test]
    fn suppressed_visible_fallback_keeps_recent_text_and_extracts_split_passthrough() {
        let mut buffer = String::new();

        assert!(
            append_suppressed_visible_and_take_passthrough(&mut buffer, "hidden\x1b[").is_empty()
        );
        let passthrough =
            append_suppressed_visible_and_take_passthrough(&mut buffer, "cstill-hidden");

        assert_eq!(passthrough, "\x1b[c");
        assert_eq!(buffer, "hiddenstill-hidden");
    }

    #[test]
    fn suppressed_visible_fallback_is_bounded_after_passthrough_extraction() {
        let mut buffer = String::new();

        let passthrough = append_suppressed_visible_and_take_passthrough(
            &mut buffer,
            &format!(
                "{}\x1b]11;?\x07",
                "x".repeat(SUPPRESSED_VISIBLE_FALLBACK_MAX_BYTES + 16)
            ),
        );

        assert_eq!(passthrough, "\x1b]11;?\x07");
        assert_eq!(buffer.len(), SUPPRESSED_VISIBLE_FALLBACK_MAX_BYTES);
    }

    #[test]
    fn timeout_discard_clears_suppressed_visible_and_buffered_osc_text() {
        let mut buffer = "prompt".to_string();

        let discarded_len = discard_suppressed_output(&mut buffer, "tail".to_string());

        assert_eq!(discarded_len, "prompttail".len());
        assert!(buffer.is_empty());
    }

    #[tokio::test]
    async fn post_login_timer_only_arms_after_injection_is_normal() {
        let pending_post_login = Some(PendingStartupCommand {
            input: b"uptime\r".to_vec(),
            delay_ms: 1,
        });
        let mut post_login_deadline: Option<Pin<Box<Sleep>>> = None;

        super::arm_post_login_timer(
            &IoPhase::WaitInitial,
            &pending_post_login,
            &mut post_login_deadline,
        );
        assert!(post_login_deadline.is_none());

        super::arm_post_login_timer(
            &IoPhase::Suppressing,
            &pending_post_login,
            &mut post_login_deadline,
        );
        assert!(post_login_deadline.is_none());

        super::arm_post_login_timer(
            &IoPhase::Normal,
            &pending_post_login,
            &mut post_login_deadline,
        );

        assert!(post_login_deadline.is_some());
        post_login_deadline.as_mut().unwrap().as_mut().await;
    }
}
