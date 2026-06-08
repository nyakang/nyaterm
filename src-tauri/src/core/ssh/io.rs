use super::client::{SshHandle, SshHandler, SshPostLoginConfig};
use crate::core::capture::OutputCaptureProcessor;
use crate::core::ssh::osc::{self, OscStripper, ShellKind};
use crate::core::zmodem::{
    ZmodemAction, ZmodemDetectResult, ZmodemDetector, ZmodemEvent, ZmodemTransfer,
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

const INJECT_TIMEOUT_SECS: u64 = 5;
const INITIAL_INJECT_DELAY_MS: u64 = 500;

/// Tries to detect the remote shell via an exec channel with a timeout.
///
/// Returns the detected [`ShellKind`], or `None` when the exec channel
/// fails / returns empty output — which is the normal behaviour of
/// non-standard "shells" such as JumpServer (koko).
async fn detect_shell_type(handle: &mut client::Handle<SshHandler>) -> Option<ShellKind> {
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

    timeout(Duration::from_millis(1200), fut)
        .await
        .ok()
        .flatten()
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
) -> AppResult<(russh::Channel<client::Msg>, Option<String>, String)> {
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|error| AppError::Channel(format!("Failed to open channel: {}", error)))?;

    channel
        .request_pty(false, "xterm-256color", 80, 24, 0, 0, &[])
        .await
        .map_err(|error| AppError::Channel(format!("PTY request failed: {}", error)))?;

    channel
        .request_shell(false)
        .await
        .map_err(|error| AppError::Channel(format!("Shell request failed: {}", error)))?;

    let ready_marker = osc::build_ready_marker(session_id);

    let injection_script = match detect_shell_type(handle).await {
        Some(shell_kind) => {
            let script = osc::injection_script(shell_kind, &ready_marker);
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
    };

    Ok((channel, injection_script, ready_marker))
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

struct PendingPostLogin {
    input: Vec<u8>,
    delay_ms: u64,
}

pub(super) fn build_post_login_input(command: &str) -> Option<Vec<u8>> {
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
    pending_post_login: &Option<PendingPostLogin>,
    post_login_deadline: &mut Option<Pin<Box<Sleep>>>,
) {
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
) {
    match handle_injection_result(phase, result) {
        InjectionEvent::Inject => {
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
    cwd: SharedCwd,
    connection_id: Option<String>,
    injection_script: Option<String>,
    ready_marker: String,
    post_login: Option<SshPostLoginConfig>,
) {
    let output_event = format!("terminal-output-{}", session_id);
    let cwd_event = format!("cwd-changed-{}", session_id);
    let closed_event = format!("session-closed-{}", session_id);

    let recording_mgr: Option<Arc<RecordingManager>> = app
        .try_state::<Arc<RecordingManager>>()
        .map(|state| state.inner().clone());
    let output = SessionOutputCoalescer::for_app(app.clone(), output_event.clone());
    let mut stripper = OscStripper::new(&ready_marker);

    let mut capture_processor = OutputCaptureProcessor::new();

    let mut phase = if injection_script.is_some() {
        IoPhase::WaitInitial
    } else {
        IoPhase::Normal
    };
    let mut pending_script = injection_script;
    let mut pending_post_login = post_login.and_then(|config| {
        build_post_login_input(&config.command).map(|input| PendingPostLogin {
            input,
            delay_ms: config.delay_ms,
        })
    });
    let mut post_login_deadline: Option<Pin<Box<Sleep>>> = None;
    arm_post_login_timer(&pending_post_login, &mut post_login_deadline);
    let mut remote_exit_status: Option<u32> = None;
    let mut remote_exit_signal: Option<String> = None;

    let mut zmodem_detector = ZmodemDetector::new();
    let mut zmodem_transfer: Option<ZmodemTransfer> = None;
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
                    Some(SessionCommand::Write(data)) => {
                        if zmodem_transfer.is_some() {
                            continue;
                        }
                        if let Some(ref recorder) = recording_mgr {
                            recorder.write_input(&session_id, &data);
                        }
                        let _ = channel.data(&data[..]).await;
                    }
                    Some(SessionCommand::Resize { cols, rows }) => {
                        let _ = channel.window_change(cols, rows, 0, 0).await;
                    }
                    Some(SessionCommand::CaptureExec { marker_id, wrapped_command, result_tx }) => {
                        capture_processor.register(marker_id, result_tx);
                        let _ = channel.data(&wrapped_command[..]).await;
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
                    Some(SessionCommand::ZmodemAcceptUpload { files }) => {
                        if let Some(ref mut transfer) = zmodem_transfer {
                            let actions = transfer.accept_upload(files);
                            handle_zmodem_actions(&app, &zmodem_event_name, &mut channel, actions).await;
                            if transfer.is_done() {
                                zmodem_transfer = None;
                            }
                        }
                    }
                    Some(SessionCommand::ZmodemCancel) => {
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
            msg = channel.wait() => {
                match msg {
                    Some(ChannelMsg::Data { ref data }) => {
                        // ZMODEM: if a transfer is active, route raw bytes to it.
                        if let Some(ref mut transfer) = zmodem_transfer {
                            let actions = transfer.feed_incoming(data);
                            handle_zmodem_actions(&app, &zmodem_event_name, &mut channel, actions).await;
                            if transfer.is_done() {
                                zmodem_transfer = None;
                                zmodem_detector.reset();
                            }
                            continue;
                        }

                        // ZMODEM: detect header in raw bytes before lossy UTF-8 conversion.
                        if phase == IoPhase::Normal {
                            match zmodem_detector.feed(data) {
                                ZmodemDetectResult::Detected { direction, passthrough, initial_bytes } => {
                                    // Forward any pre-header bytes to the terminal.
                                    if !passthrough.is_empty() {
                                        let pre = String::from_utf8_lossy(&passthrough).to_string();
                                        if !pre.is_empty() {
                                            output.push_owned(pre);
                                        }
                                    }
                                    let transfer = ZmodemTransfer::new(direction, &initial_bytes);
                                    zmodem_transfer = Some(transfer);
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
                                    let text = String::from_utf8_lossy(&passthrough).to_string();
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
                                    ).await;
                                    continue;
                                }
                            }
                        }

                        let text = String::from_utf8_lossy(data).to_string();
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
                        ).await;
                    }
                    Some(ChannelMsg::ExtendedData { ref data, .. }) => {
                        let text = String::from_utf8_lossy(data).to_string();
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
                tracing::debug!(
                    session_id = %session_id,
                    buffered_bytes = flushed.len(),
                    "Injection timeout — falling back to passthrough mode"
                );
                if timeout_event == InjectionTimeoutEvent::FallbackToNormal && !flushed.is_empty() {
                    emit_visible_text(&output, &recording_mgr, &session_id, &flushed);
                }
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
        IoPhase, PendingPostLogin, build_post_login_input, handle_injection_result,
        handle_injection_timeout, should_send_initial_injection,
    };
    use crate::core::ssh::osc::OscResult;
    use std::pin::Pin;
    use tokio::time::Sleep;

    #[test]
    fn post_login_input_normalizes_line_endings_and_adds_enter() {
        let input = build_post_login_input("cd /opt/app\nclear").expect("input");

        assert_eq!(input, b"cd /opt/app\rclear\r");
    }

    #[test]
    fn post_login_input_preserves_existing_trailing_enter() {
        let input = build_post_login_input("uptime\r").expect("input");

        assert_eq!(input, b"uptime\r");
    }

    #[test]
    fn post_login_input_ignores_blank_commands() {
        assert!(build_post_login_input(" \n\t ").is_none());
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
    fn injection_timeout_is_5s_and_falls_back_to_normal() {
        assert_eq!(INJECT_TIMEOUT_SECS, 5);

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

    #[tokio::test]
    async fn post_login_timer_can_arm_while_injection_is_waiting_for_ready() {
        let pending_post_login = Some(PendingPostLogin {
            input: b"uptime\r".to_vec(),
            delay_ms: 1,
        });
        let mut post_login_deadline: Option<Pin<Box<Sleep>>> = None;

        super::arm_post_login_timer(&pending_post_login, &mut post_login_deadline);

        assert!(post_login_deadline.is_some());
        post_login_deadline.as_mut().unwrap().as_mut().await;
    }
}
