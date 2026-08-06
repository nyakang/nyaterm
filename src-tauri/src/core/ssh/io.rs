use super::client::{SshHandle, SshHandler, SshPostLoginConfig, SshStartupCommand};
use crate::config::SftpCwdFollowMode;
use crate::core::capture::OutputCaptureProcessor;
use crate::core::input::remap_del_to_bs;
use crate::core::ssh::osc::{self, OscStripper, ShellKind};
use crate::core::terminal_session::{TerminalOutputDecoder, encode_terminal_input};
use crate::core::zmodem::{
    ZmodemAction, ZmodemDetectResult, ZmodemDetector, ZmodemDirection, ZmodemDownloadOoDrain,
    ZmodemEvent, ZmodemTransfer, ZmodemUploadDrain, start_zmodem_transfer,
};
use crate::core::{
    RecordingManager, SessionCommand, SessionManager, SessionOutputCoalescer, SharedCwd,
    update_cwd_if_changed,
};
use crate::error::{AppError, AppResult};
use russh::{ChannelMsg, client};
use std::{pin::Pin, sync::Arc};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;
use tokio::time::{Duration, Sleep, timeout};

const INJECT_TIMEOUT_SECS: u64 = 30;
const INITIAL_INJECT_DELAY_MS: u64 = 500;
const SUPPRESSED_VISIBLE_FALLBACK_MAX_BYTES: usize = 64 * 1024;

/// Tries to detect the remote shell via an exec channel with a timeout.
///
/// Returns the detected [`ShellKind`], or `None` when the exec channel
/// fails / returns empty output — which is the normal behaviour of
/// non-standard "shells" such as JumpServer (koko).
async fn detect_shell_type(
    handle: &mut client::Handle<SshHandler>,
    timeout_ms: u64,
) -> Option<ShellKind> {
    let fut = async {
        let mut ch = handle.channel_open_session().await.ok()?;
        ch.exec(
            true,
            r#"printf '%s\n' "$SHELL"; ps -p $$ -o comm= 2>/dev/null || true"#,
        )
        .await
        .ok()?;

        let mut output = String::new();
        while let Some(msg) = ch.wait().await {
            if let ChannelMsg::Data { ref data } = msg {
                output.push_str(&String::from_utf8_lossy(data));
            }
        }

        let kind = ShellKind::from_name(output.trim());
        if kind == ShellKind::Unknown {
            None
        } else {
            Some(kind)
        }
    };

    timeout(Duration::from_millis(timeout_ms), fut)
        .await
        .ok()
        .flatten()
}

async fn exec_remote_command(
    handle: &mut client::Handle<SshHandler>,
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

async fn install_remote_shell_integration(
    handle: &mut client::Handle<SshHandler>,
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
pub(super) async fn open_shell_channel(
    handle: &mut client::Handle<SshHandler>,
    session_id: &str,
    x11_fake_cookie_hex: Option<&str>,
    cwd_follow_mode: SftpCwdFollowMode,
    shell_detection_timeout_ms: u64,
) -> AppResult<(
    russh::Channel<client::Msg>,
    Option<String>,
    String,
    Option<ShellKind>,
    Option<String>,
)> {
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|error| AppError::Channel(format!("Failed to open channel: {}", error)))?;

    let mut local_notice = None;
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

    channel
        .request_pty(false, "xterm-256color", 80, 24, 0, 0, &[])
        .await
        .map_err(|error| AppError::Channel(format!("PTY request failed: {}", error)))?;

    channel
        .request_shell(false)
        .await
        .map_err(|error| AppError::Channel(format!("Shell request failed: {}", error)))?;

    let ready_marker = osc::build_ready_marker(session_id);

    let mut detected_shell = None;
    let injection_script = match cwd_follow_mode {
        SftpCwdFollowMode::Off => {
            tracing::debug!(
                session_id = %session_id,
                "SSH shell integration disabled by connection settings"
            );
            None
        }
        SftpCwdFollowMode::ShellIntegration | SftpCwdFollowMode::RcFile => {
            match detect_shell_type(handle, shell_detection_timeout_ms).await {
                Some(shell_kind) => {
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
                        tracing::debug!(
                            session_id = %session_id,
                            shell = ?shell_kind,
                            "Will inject OSC 7 hook after initial output"
                        );
                    } else {
                        tracing::debug!(
                            session_id = %session_id,
                            shell = ?shell_kind,
                            "Shell detected but no injection script available — skipping"
                        );
                    }
                    script
                }
                None => {
                    tracing::debug!(
                        session_id = %session_id,
                        "Shell detection returned no output — skipping OSC 7 injection"
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

#[derive(Debug, PartialEq, Eq)]
enum InjectionEvent {
    None,
    Inject,
    Ready { visible_after_ready: String },
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

pub(super) fn build_startup_command_input(command: &str) -> Option<Vec<u8>> {
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
        IoPhase::Suppressing if result.ready => {
            *phase = IoPhase::Normal;
            InjectionEvent::Ready {
                visible_after_ready: result.visible_after_ready.clone(),
            }
        }
        IoPhase::Suppressing => InjectionEvent::None,
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

fn append_suppressed_visible(buffer: &mut String, visible: &str) {
    if visible.is_empty() {
        return;
    }

    buffer.push_str(visible);
    while buffer.len() > SUPPRESSED_VISIBLE_FALLBACK_MAX_BYTES {
        let Some(first_char) = buffer.chars().next() else {
            break;
        };
        buffer.drain(..first_char.len_utf8());
    }
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
) {
    match handle_injection_result(phase, result) {
        InjectionEvent::Inject => {
            tracing::debug!(
                session_id = %session_id,
                shell = ?shell_kind,
                visible_bytes = result.visible.len(),
                "Sending SSH shell integration injection"
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
            tracing::debug!(
                session_id = %session_id,
                shell = ?shell_kind,
                suppressed_visible_bytes,
                ready_after_visible_bytes = visible_after_ready.len(),
                "SSH shell integration ready marker received"
            );
            emit_metadata(app, cwd_event, cwd, manager, session_id, result).await;
            if !visible_after_ready.is_empty() {
                emit_visible_text(output, recording_mgr, session_id, &visible_after_ready);
            }
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
            append_suppressed_visible(suppressed_visible_fallback, &result.visible);
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
    mut cmd_rx: mpsc::UnboundedReceiver<SessionCommand>,
    output_control_tx: mpsc::UnboundedSender<SessionCommand>,
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

    let mut phase = if injection_script.is_some() {
        IoPhase::WaitInitial
    } else {
        IoPhase::Normal
    };
    let mut suppressed_visible_fallback = String::new();
    let mut pending_script = injection_script;
    let mut pending_post_login = post_login.and_then(|config| {
        build_startup_command_input(&config.command).map(|input| PendingStartupCommand {
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
                    Some(SessionCommand::Attach) => {
                        output.attach();
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
                        if let Some(ref recorder) = recording_mgr {
                            recorder.write_input(&session_id, &data);
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
                    let _ = channel.data(script.as_bytes()).await;
                    tracing::debug!(
                        session_id = %session_id,
                        shell = ?shell_kind,
                        "Sending SSH shell integration injection after initial delay"
                    );
                    on_initial_injection_sent(&mut phase);
                    inject_deadline.as_mut().reset(
                        tokio::time::Instant::now()
                            + std::time::Duration::from_secs(INJECT_TIMEOUT_SECS),
                    );
                }
            }
            _ = &mut inject_deadline, if phase != IoPhase::Normal => {
                let timeout_event = handle_injection_timeout(&mut phase);
                let flushed = stripper.flush();
                let discarded_visible_bytes = discard_suppressed_output(
                    &mut suppressed_visible_fallback,
                    flushed,
                );
                tracing::debug!(
                    session_id = %session_id,
                    shell = ?shell_kind,
                    discarded_visible_bytes,
                    "Injection timeout — discarding suppressed shell integration output"
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
                    if let Some(ref recorder) = recording_mgr {
                        recorder.write_input(&session_id, &pending.input);
                    }
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
                    if let Some(ref recorder) = recording_mgr {
                        recorder.write_input(&session_id, &pending.input);
                    }
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
        manager
            .confirm_command_submission(session_id, command.clone())
            .await;
        let _ = app.emit(
            "session-command-accepted",
            serde_json::json!({
                "sessionId": session_id,
                "command": command,
            }),
        );
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
        append_suppressed_visible, build_startup_command_input, discard_suppressed_output,
        handle_injection_result, handle_injection_timeout, should_send_initial_injection,
    };
    use crate::core::ssh::osc::OscResult;
    use std::pin::Pin;
    use tokio::time::Sleep;

    #[test]
    fn post_login_input_normalizes_line_endings_and_adds_enter() {
        let input = build_startup_command_input("cd /opt/app\nclear").expect("input");

        assert_eq!(input, b"cd /opt/app\rclear\r");
    }

    #[test]
    fn post_login_input_preserves_existing_trailing_enter() {
        let input = build_startup_command_input("uptime\r").expect("input");

        assert_eq!(input, b"uptime\r");
    }

    #[test]
    fn post_login_input_ignores_blank_commands() {
        assert!(build_startup_command_input(" \n\t ").is_none());
    }

    fn osc_result(ready: bool, visible_after_ready: &str) -> OscResult {
        OscResult {
            visible: "suppressed output".to_string(),
            visible_after_ready: visible_after_ready.to_string(),
            cwd_paths: Vec::new(),
            ready,
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
    fn suppressed_visible_fallback_keeps_recent_visible_text() {
        let mut buffer = String::new();

        append_suppressed_visible(&mut buffer, "prompt-before-ready");

        assert_eq!(buffer, "prompt-before-ready");
    }

    #[test]
    fn suppressed_visible_fallback_is_bounded() {
        let mut buffer = String::new();

        append_suppressed_visible(
            &mut buffer,
            &"x".repeat(SUPPRESSED_VISIBLE_FALLBACK_MAX_BYTES + 16),
        );

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
