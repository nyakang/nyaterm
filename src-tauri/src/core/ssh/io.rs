use super::client::{SshHandle, SshHandler};
use crate::core::capture::OutputCaptureProcessor;
use crate::core::ssh::osc::{self, OscStripper, ShellKind};
use crate::core::zmodem::{ZmodemAction, ZmodemDetector, ZmodemEvent, ZmodemTransfer};
use crate::core::{
    update_cwd_if_changed, RecordingManager, SessionCommand, SessionManager,
    SessionOutputCoalescer, SharedCwd,
};
use crate::error::{AppError, AppResult};
use russh::{client, ChannelMsg};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};

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
#[derive(PartialEq)]
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
) {
    const INJECT_TIMEOUT_SECS: u64 = 30;

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
    let mut remote_exit_status: Option<u32> = None;
    let mut remote_exit_signal: Option<String> = None;

    let mut zmodem_detector = ZmodemDetector::new();
    let mut zmodem_transfer: Option<ZmodemTransfer> = None;
    let zmodem_event_name = format!("zmodem-event-{session_id}");

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
                            if let Some((direction, header_offset)) = zmodem_detector.feed(data) {
                                // Forward any pre-header bytes to the terminal.
                                if header_offset > 0 {
                                    let pre = String::from_utf8_lossy(&data[..header_offset]).to_string();
                                    if !pre.is_empty() {
                                        output.push_owned(pre);
                                    }
                                }
                                let transfer = ZmodemTransfer::new(direction, &data[header_offset..]);
                                zmodem_transfer = Some(transfer);
                                let _ = app.emit(&zmodem_event_name, &ZmodemEvent::Detected { direction });
                                tracing::info!(
                                    session_id = %session_id,
                                    ?direction,
                                    "ZMODEM transfer detected"
                                );
                                continue;
                            }
                        }

                        let text = String::from_utf8_lossy(data).to_string();
                        let mut result = stripper.push(&text);

                        if capture_processor.has_active() {
                            result.visible = capture_processor.process(&result.visible);
                        }

                        match phase {
                            IoPhase::WaitInitial => {
                                emit_output(
                                    &app, &output, &cwd_event, &cwd,
                                    &recording_mgr, &session_id, &manager,
                                    &result,
                                ).await;

                                if let Some(script) = pending_script.take() {
                                    let _ = channel.data(script.as_bytes()).await;
                                }
                                phase = IoPhase::Suppressing;
                                inject_deadline.as_mut().reset(
                                    tokio::time::Instant::now()
                                        + std::time::Duration::from_secs(INJECT_TIMEOUT_SECS),
                                );
                            }
                            IoPhase::Suppressing => {
                                for path in &result.cwd_paths {
                                    if let Some(next_cwd) = update_cwd_if_changed(&cwd, path).await {
                                        let _ = app.emit(&cwd_event, &next_cwd);
                                    }
                                }
                                for command in &result.accepted_commands {
                                    manager
                                        .confirm_command_submission(&session_id, command.clone())
                                        .await;
                                }
                                if result.ready {
                                    phase = IoPhase::Normal;
                                }
                            }
                            IoPhase::Normal => {
                                emit_output(
                                    &app, &output, &cwd_event, &cwd,
                                    &recording_mgr, &session_id, &manager,
                                    &result,
                                ).await;
                            }
                        }
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
            _ = &mut inject_deadline, if phase == IoPhase::Suppressing => {
                phase = IoPhase::Normal;
                let flushed = stripper.flush();
                tracing::debug!(
                    session_id = %session_id,
                    buffered_bytes = flushed.len(),
                    "Injection timeout — falling back to passthrough mode"
                );
                if !flushed.is_empty() {
                    if let Some(ref recorder) = recording_mgr {
                        recorder.write_output(&session_id, &flushed);
                    }
                    output.push_owned(flushed);
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

    if result.visible.is_empty() {
        return;
    }

    if let Some(recorder) = recording_mgr {
        recorder.write_output(session_id, &result.visible);
    }

    output.push(&result.visible);
}
