//! Interactive SSH session and shell integration workers.
use crate::X11ForwardingConfig;
use crate::session::ManagedSession;
use crate::session_event_queue::SessionEventQueue;
use crate::session_notice::SessionNotice;
use crate::ssh_connection::{PrimarySessionClaim, SharedSshHandle};
use crate::ssh_connection::{
    SshClientHandler, effective_agent_forwarding_config,
    open_authenticated_ssh_handle_with_channel_senders,
};
use crate::ssh_services::SshSessionServices;
use crate::ssh_shell_integration::{
    PreparedSshShellIntegration, ShellKind, SshIntegrationOutput, SshShellIntegrationState,
    build_legacy_ssh_ready_marker, build_ssh_ready_marker, build_ssh_shell_integration_script,
    detect_ssh_shell_type,
};
use crate::x11::X11ChannelOpen;
use crate::x11::{MIT_MAGIC_COOKIE, X11Forwarder, spawn_x11_forwarder};
use crate::{
    SessionEvent, SftpCwdFollowMode, ShellEnvironmentCache, SshMultiplexHandle, SshSessionConfig,
    connection_attempt, prepare_x11_forwarding, ssh_mailbox, ssh_shell_integration, tmux,
};
use russh::{ChannelMsg, Disconnect, client};
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use tokio::sync::mpsc as tokio_mpsc;

pub(crate) enum SshCommand {
    Write(ssh_mailbox::PendingWrite),
    Resize {
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    },
    /// tmux control-channel command line from the UI (`select-window -t @2`,
    /// `new-window`, ...). Only meaningful once the channel runs control mode.
    TmuxCommand {
        line: String,
    },
    /// Leave tmux control mode and return the channel to the shell.
    TmuxDetach,
    /// Input bytes for one projected tmux pane (routed via `send-keys`).
    TmuxPaneInput {
        pane: String,
        data: Vec<u8>,
    },
    /// Renderer geometry for one projected tmux pane. Cell dimensions only:
    /// `resize-pane` takes cells and the pixel fields would be dead weight.
    TmuxPaneResize {
        pane: String,
        cols: u16,
        rows: u16,
    },
    /// Close one projected tmux pane (`kill-pane`).
    TmuxPaneClose {
        pane: String,
    },
    Close,
}

/// One channel carrying every tmux control-mode command — UI commands,
/// detach, and projected-pane input/resize/close — so the FIFO that pairs
/// `%begin`/`%end` responses stays strictly ordered.
pub(crate) type TmuxCommandSender = tmux::mailbox::Sender;

pub(crate) type TmuxCommandReceiver = tmux::mailbox::Receiver;

pub(crate) struct OpenSshShellSession {
    pub(crate) handle: SshShellHandle,
    pub(crate) attempt: connection_attempt::ConnectionAttempt,
    pub(crate) post_login: Option<nyaterm_core::models::sessions::ConnectionPostLogin>,
    pub(crate) channel: russh::Channel<client::Msg>,
    pub(crate) jump_handles: Vec<client::Handle<SshClientHandler>>,
    pub(crate) disconnect_on_close: bool,
    pub(crate) x11_forwarder: Option<X11Forwarder>,
    pub(crate) x11_multiplex_registration: Option<SshMultiplexHandle>,
    pub(crate) local_notice: Option<SessionNotice>,
    pub(crate) early_output: Vec<u8>,
    pub(crate) prepared_shell_integration: Option<PreparedSshShellIntegration>,
    pub(crate) ready_marker: String,
    pub(crate) legacy_ready_marker: Option<String>,
    pub(crate) shell_kind: Option<ShellKind>,
    pub(crate) command_context: nyaterm_core::completion::types::ShellCommandContext,
}

pub(crate) enum SshShellHandle {
    Dedicated(client::Handle<SshClientHandler>),
    Multiplexed(SharedSshHandle),
}

pub(crate) struct PendingOpenSshShellSession {
    pub(crate) handle: SshShellHandle,
    pub(crate) jump_handles: Vec<client::Handle<SshClientHandler>>,
    pub(crate) disconnect_on_close: bool,
    pub(crate) multiplex: Option<SshMultiplexHandle>,
    /// Held from before the deferred-PTY wait until the shell request lands, so
    /// no shared-connection feature can open a channel ahead of the terminal.
    pub(crate) primary_claim: Option<PrimarySessionClaim>,
    pub(crate) x11_config: Option<X11ForwardingConfig>,
    pub(crate) x11_tx: Option<tokio_mpsc::UnboundedSender<X11ChannelOpen>>,
    pub(crate) x11_rx: Option<tokio_mpsc::UnboundedReceiver<X11ChannelOpen>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SshPtyDimensions {
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    pub(crate) pixel_width: u16,
    pub(crate) pixel_height: u16,
}

pub(crate) struct PendingSshIntegrationUpload {
    pub(crate) integration: PreparedSshShellIntegration,
    pub(crate) cancel: Option<tokio::sync::oneshot::Sender<()>>,
    pub(crate) join: tokio::task::JoinHandle<ssh_shell_integration::ScriptUploadOutcome>,
}

/// Worker readiness must not let tmux publish projections before its host is registered.
async fn confirm_ssh_registration(
    ready: &mpsc::Sender<Result<(), String>>,
    registered: tokio::sync::oneshot::Receiver<()>,
) -> bool {
    ready.send(Ok(())).is_ok() && registered.await.is_ok()
}

async fn discard_open_ssh_shell(session_id: &str, session: OpenSshShellSession) {
    let _ = tokio::time::timeout(Duration::from_millis(250), async {
        let _ = session.channel.eof().await;
        let _ = session.channel.close().await;
    })
    .await;
    disconnect_open_ssh_shell(
        session_id,
        &session.handle,
        session.jump_handles,
        session.disconnect_on_close,
        session.x11_multiplex_registration,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_ssh_worker(
    session_id: String,
    config: SshSessionConfig,
    command_rx: ssh_mailbox::Receiver,
    ready_tx: mpsc::Sender<Result<(), String>>,
    registered_rx: tokio::sync::oneshot::Receiver<()>,
    event_queue: SessionEventQueue,
    multiplex: Option<SshMultiplexHandle>,
    shell_environment: Arc<ShellEnvironmentCache>,
    ssh_services: SshSessionServices,
    tmux_cmd_rx: TmuxCommandReceiver,
    tmux_tx: TmuxCommandSender,
    tmux_active: Arc<AtomicBool>,
    sessions: Arc<Mutex<HashMap<String, ManagedSession>>>,
) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("nyaterm-ssh")
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = ready_tx.send(Err(format!("failed to start SSH runtime: {error}")));
            return;
        }
    };

    runtime.block_on(async move {
        if config.runtime_mode == nyaterm_core::models::ssh_runtime::SshRuntimeMode::Sftp {
            let result = async {
                let handle = multiplex.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("SFTP-only sessions require a shared SSH connection")
                })?;
                ssh_services.probe_files(&config, handle).await?;
                Ok::<_, anyhow::Error>(())
            }
            .await;
            match result {
                Ok(()) => {
                    if !confirm_ssh_registration(&ready_tx, registered_rx).await {
                        return;
                    }
                    run_sftp_only_session(
                        &session_id,
                        command_rx,
                        &event_queue,
                        multiplex.as_ref(),
                    )
                    .await;
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error.to_string()));
                }
            }
            return;
        }
        if config.deferred_pty {
            run_deferred_ssh_worker(
                session_id,
                config,
                command_rx,
                ready_tx,
                registered_rx,
                event_queue,
                multiplex,
                shell_environment,
                ssh_services,
                tmux_cmd_rx,
                tmux_tx,
                tmux_active,
                sessions.clone(),
            )
            .await;
            return;
        }

        let open_session =
            match open_ssh_shell(&session_id, &config, multiplex.as_ref(), shell_environment).await
            {
                Ok(session) => {
                    if !confirm_ssh_registration(&ready_tx, registered_rx).await {
                        discard_open_ssh_shell(&session_id, session).await;
                        return;
                    }
                    session
                }
                Err(_)
                    if config.runtime_mode
                        == nyaterm_core::models::ssh_runtime::SshRuntimeMode::Standard
                        && config.remote_file_browser_enabled()
                        && multiplex.as_ref().is_some_and(|handle| {
                            handle.shell_availability()
                                == connection_attempt::ShellAvailability::Unavailable
                        }) =>
                {
                    if let Err(error) = ssh_services
                        .probe_files(
                            &config,
                            multiplex.as_ref().expect("fallback has shared connection"),
                        )
                        .await
                    {
                        let _ = ready_tx.send(Err(error.to_string()));
                        return;
                    }
                    config.attempt.record_shell_reply(false);
                    if !confirm_ssh_registration(&ready_tx, registered_rx).await {
                        return;
                    }
                    run_sftp_only_session(
                        &session_id,
                        command_rx,
                        &event_queue,
                        multiplex.as_ref(),
                    )
                    .await;
                    return;
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error.to_string()));
                    return;
                }
            };
        run_open_ssh_shell_session(
            session_id,
            open_session,
            command_rx,
            event_queue,
            VecDeque::new(),
            tmux_cmd_rx,
            tmux_tx,
            tmux_active,
            sessions,
        )
        .await;
    });
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_deferred_ssh_worker(
    session_id: String,
    config: SshSessionConfig,
    mut command_rx: ssh_mailbox::Receiver,
    ready_tx: mpsc::Sender<Result<(), String>>,
    registered_rx: tokio::sync::oneshot::Receiver<()>,
    event_queue: SessionEventQueue,
    multiplex: Option<SshMultiplexHandle>,
    shell_environment: Arc<ShellEnvironmentCache>,
    ssh_services: SshSessionServices,
    tmux_cmd_rx: TmuxCommandReceiver,
    tmux_tx: TmuxCommandSender,
    tmux_active: Arc<AtomicBool>,
    sessions: Arc<Mutex<HashMap<String, ManagedSession>>>,
) {
    let pending_session =
        match open_pending_ssh_shell(&config, multiplex.as_ref(), shell_environment).await {
            Ok(session) => {
                if !confirm_ssh_registration(&ready_tx, registered_rx).await {
                    disconnect_pending_ssh_shell(session).await;
                    return;
                }
                session
            }
            Err(error) => {
                let _ = ready_tx.send(Err(error.to_string()));
                return;
            }
        };
    let mut pending_session = Some(pending_session);
    let mut dimensions = SshPtyDimensions::from_config(&config);
    let mut pending_writes = VecDeque::new();
    let mut fallback = Box::pin(tokio::time::sleep(Duration::from_millis(750)));

    loop {
        tokio::select! {
            command = command_rx.recv() => {
                match command {
                    Some(SshCommand::Write(data)) => {
                        pending_writes.push_back(data);
                    }
                    Some(SshCommand::Resize {
                        cols,
                        rows,
                        pixel_width,
                        pixel_height,
                    }) => {
                        dimensions = SshPtyDimensions::new(cols, rows, pixel_width, pixel_height);
                        break;
                    }
                    Some(
                        SshCommand::TmuxCommand { .. }
                        | SshCommand::TmuxDetach
                        | SshCommand::TmuxPaneInput { .. }
                        | SshCommand::TmuxPaneResize { .. }
                        | SshCommand::TmuxPaneClose { .. },
                    ) => {}
                    Some(SshCommand::Close) | None => {
                        if let Some(session) = pending_session.take() {
                            disconnect_pending_ssh_shell(session).await;
                        }
                        return;
                    }
                }
            }
            _ = &mut fallback => {
                break;
            }
        }
    }

    let Some(pending_session) = pending_session.take() else {
        return;
    };
    if drain_deferred_ssh_open_commands(&mut command_rx, &mut dimensions, &mut pending_writes) {
        disconnect_pending_ssh_shell(pending_session).await;
        return;
    }
    match open_ssh_shell_from_pending(&session_id, &config, pending_session, dimensions).await {
        Ok(open_session) => {
            run_open_ssh_shell_session(
                session_id,
                open_session,
                command_rx,
                event_queue,
                pending_writes,
                tmux_cmd_rx,
                tmux_tx,
                tmux_active,
                sessions,
            )
            .await;
        }
        Err(_)
            if config.runtime_mode
                == nyaterm_core::models::ssh_runtime::SshRuntimeMode::Standard
                && config.remote_file_browser_enabled()
                && multiplex.as_ref().is_some_and(|handle| {
                    handle.shell_availability()
                        == connection_attempt::ShellAvailability::Unavailable
                }) =>
        {
            if let Err(error) = ssh_services
                .probe_files(
                    &config,
                    multiplex.as_ref().expect("fallback has shared connection"),
                )
                .await
            {
                send_session_error(&event_queue, &session_id, error);
                return;
            }
            config.attempt.record_shell_reply(false);
            run_sftp_only_session(&session_id, command_rx, &event_queue, multiplex.as_ref()).await;
        }
        Err(error) => {
            send_session_error(&event_queue, &session_id, error);
        }
    }
}

pub(crate) async fn run_sftp_only_session(
    session_id: &str,
    mut commands: ssh_mailbox::Receiver,
    events: &SessionEventQueue,
    multiplex: Option<&SshMultiplexHandle>,
) {
    // The caller retains the multiplexed SSH connection. Writes queued before
    // capability detection are discarded, never replayed as shell commands.
    loop {
        tokio::select! {
            command = commands.recv() => {
                if command.is_none() || matches!(command, Some(SshCommand::Close)) { break; }
            },
            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                if let Some(handle) = multiplex
                    && (handle.is_closed() || handle.inner.target.lock().await.is_closed()) {
                    events.push(SessionEvent::Exited { session_id: session_id.to_owned(), reason: "SSH connection closed".into() });
                    break;
                }
            }
        }
    }
}

pub(crate) fn drain_deferred_ssh_open_commands(
    command_rx: &mut ssh_mailbox::Receiver,
    dimensions: &mut SshPtyDimensions,
    pending_writes: &mut VecDeque<ssh_mailbox::PendingWrite>,
) -> bool {
    loop {
        match command_rx.try_recv() {
            Ok(SshCommand::Write(data)) => {
                pending_writes.push_back(data);
            }
            Ok(SshCommand::Resize {
                cols,
                rows,
                pixel_width,
                pixel_height,
            }) => {
                *dimensions = SshPtyDimensions::new(cols, rows, pixel_width, pixel_height);
            }
            Ok(SshCommand::Close) => return true,
            Ok(
                SshCommand::TmuxCommand { .. }
                | SshCommand::TmuxDetach
                | SshCommand::TmuxPaneInput { .. }
                | SshCommand::TmuxPaneResize { .. }
                | SshCommand::TmuxPaneClose { .. },
            ) => {}
            Err(tokio_mpsc::error::TryRecvError::Empty) => return false,
            Err(tokio_mpsc::error::TryRecvError::Disconnected) => return true,
        }
    }
}

pub(crate) async fn cancel_pending_upload(
    pending_upload: &mut Option<PendingSshIntegrationUpload>,
) {
    let Some(mut pending) = pending_upload.take() else {
        return;
    };

    if let Some(cancel) = pending.cancel.take() {
        let _ = cancel.send(());
    }

    let _ = tokio::time::timeout(Duration::from_secs(1), &mut pending.join).await;
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_open_ssh_shell_session(
    session_id: String,
    open_session: OpenSshShellSession,
    mut command_rx: ssh_mailbox::Receiver,
    event_queue: SessionEventQueue,
    mut pending_writes: VecDeque<ssh_mailbox::PendingWrite>,
    mut tmux_cmd_rx: TmuxCommandReceiver,
    tmux_tx: TmuxCommandSender,
    tmux_active: Arc<AtomicBool>,
    sessions: Arc<Mutex<HashMap<String, ManagedSession>>>,
) {
    let OpenSshShellSession {
        attempt,
        mut post_login,
        handle,
        channel,
        jump_handles,
        disconnect_on_close,
        x11_forwarder,
        x11_multiplex_registration,
        local_notice,
        early_output,
        prepared_shell_integration,
        ready_marker,
        legacy_ready_marker,
        shell_kind: _shell_kind,
        command_context,
    } = open_session;
    event_queue
        .push_async(SessionEvent::CommandContext {
            session_id: session_id.clone(),
            context: command_context,
        })
        .await;
    let handle = Arc::new(handle);
    let (mut channel_rx, channel) = channel.split();
    let channel = Arc::new(channel);
    type ChannelOperation = Pin<Box<dyn Future<Output = Result<(), russh::Error>> + Send>>;
    let mut pending_send: Option<ChannelOperation> = None;
    let mut pending_resize: Option<ChannelOperation> = None;
    let mut pending_output: Option<Pin<Box<dyn Future<Output = ()> + Send>>> = None;
    let mut shell_integration = SshShellIntegrationState::new(
        prepared_shell_integration,
        ready_marker,
        legacy_ready_marker,
    );
    emit_ssh_startup_events(&event_queue, &session_id, local_notice, early_output).await;
    if let Some(forwarder) = x11_forwarder {
        spawn_x11_forwarder(event_queue.clone(), session_id.clone(), forwarder);
    }

    let initial_inject_delay = tokio::time::sleep(Duration::from_millis(500));
    tokio::pin!(initial_inject_delay);
    let inject_timeout = tokio::time::sleep(ssh_shell_integration::SSH_INTEGRATION_TIMEOUT);
    tokio::pin!(inject_timeout);

    let mut pending_upload: Option<PendingSshIntegrationUpload> = None;

    // Double-C discovery continuously follows stdout framing. Input only
    // enables launch advisories and explicit single-C compatibility.
    let mut tmux_detector = tmux::TmuxControlDetector::new();

    post_login = post_login.filter(|command| command.enabled && !command.command.trim().is_empty());
    let post_login_timer = tokio::time::sleep(Duration::ZERO);
    tokio::pin!(post_login_timer);
    let mut post_login_armed = false;

    loop {
        if shell_integration.is_normal()
            && pending_send.is_none()
            && let Some(mut write) = pending_writes.pop_front()
        {
            let channel = channel.clone();
            pending_send = Some(Box::pin(async move {
                let result = channel.data_bytes(std::mem::take(&mut write.bytes)).await;
                drop(write); // Release the capacity permit only after SSH accepted the bytes.
                result
            }));
        }
        if !post_login_armed
            && shell_integration.is_normal()
            && let Some(command) = post_login.as_ref()
        {
            post_login_timer.as_mut().reset(
                tokio::time::Instant::now() + Duration::from_millis(command.delay_ms.min(60_000)),
            );
            post_login_armed = true;
        }
        tokio::select! {
            result = async { pending_send.as_mut().expect("guarded send").await }, if pending_send.is_some() => {
                pending_send = None;
                if let Err(error) = result { event_queue.push_async(SessionEvent::Error { session_id: session_id.clone(), message: error.to_string() }).await; break; }
            }
            result = async { pending_resize.as_mut().expect("guarded resize").await }, if pending_resize.is_some() => {
                pending_resize = None;
                if result.is_err() { break; }
            }
            _ = async { pending_output.as_mut().expect("guarded output").await }, if pending_output.is_some() => {
                pending_output = None;
            }
            _ = async { while !attempt.is_cancelled() { tokio::time::sleep(Duration::from_millis(25)).await; } } => break,
            _ = &mut post_login_timer, if post_login_armed && post_login.is_some() && pending_send.is_none() => {
                if let Some(command) = post_login.take() {
                    let mut text = command.command.replace("\r\n", "\r").replace('\n', "\r");
                    if !text.ends_with('\r') { text.push('\r'); }
                    // Post-login commands provide the same explicit launch
                    // evidence as user input, including single-C compatibility.
                    tmux_detector.feed_input(text.as_bytes());
                    let channel = channel.clone();
                    pending_send = Some(Box::pin(async move { channel.data_bytes(text.into_bytes()).await }));
                }
            }
            _ = &mut initial_inject_delay, if shell_integration.should_inject_on_initial_delay() => {
                if let Some(integration) = shell_integration.take_pending_integration() {
                    shell_integration.begin_suppression();
                    inject_timeout
                        .as_mut()
                        .reset(
                            tokio::time::Instant::now()
                                + ssh_shell_integration::SSH_INTEGRATION_TIMEOUT,
                    );
                    let task_handle = Arc::clone(&handle);
                    let upload_payload = integration.upload_payload();
                    let (cancel, cancel_rx) = tokio::sync::oneshot::channel();
                    let join = tokio::spawn(ssh_shell_integration::upload_integration_script(
                        task_handle,
                        upload_payload,
                        cancel_rx,
                    ));
                    pending_upload = Some(PendingSshIntegrationUpload {
                        integration,
                        cancel: Some(cancel),
                        join,
                    });
                }
            }
            outcome = async {
                match pending_upload.as_mut() {
                    Some(pending) => (&mut pending.join).await,
                    None => std::future::pending().await,
                }
            }, if pending_upload.is_some() && pending_send.is_none() => {
                let pending = pending_upload
                    .take()
                    .expect("pending_upload checked Some by the select guard");
                let outcome = outcome.unwrap_or_else(|_join_error| {
                    // The upload task panicked or was cancelled; treat it as
                    // a failed upload so we fall back to the direct-write
                    // path below instead of hanging the injection forever.
                    ssh_shell_integration::ScriptUploadOutcome::failed()
                });
                let integration = pending.integration;
                let script = shell_integration.apply_upload_outcome(outcome, integration);
                let channel = channel.clone();
                pending_send = Some(Box::pin(async move { channel.data_bytes(script).await }));
                inject_timeout
                    .as_mut()
                    .reset(
                        tokio::time::Instant::now()
                            + ssh_shell_integration::SSH_INTEGRATION_TIMEOUT,
                    );
            }
            _ = &mut inject_timeout, if shell_integration.is_suppressing() && pending_output.is_none() => {
                let output = shell_integration.force_normal_after_timeout();
                pending_output = Some(ssh_output_future(event_queue.clone(), session_id.clone(), output));
            }
            () = async {
                if let Some(deadline) = tmux_detector.deadline() {
                    tokio::time::sleep_until(deadline).await;
                }
            }, if tmux_detector.deadline().is_some() && pending_output.is_none() => {
                let expired = tmux_detector.expire();
                if expired.notice {
                    event_queue.push_async(SessionEvent::Notice {
                        session_id: session_id.clone(),
                        notice: SessionNotice::TmuxControlUnconfirmed,
                    }).await;
                }
                if !expired.bytes.is_empty() {
                    let output = shell_integration.filter_output(&expired.bytes);
                    pending_output = Some(ssh_output_future(event_queue.clone(), session_id.clone(), output));
                }
            }
            command = command_rx.recv() => {
                match command {
                    Some(SshCommand::Write(data)) => {
                        // Track user-typed lines so a `tmux -CC` command arms
                        // output-side detection before the write lands.
                        tmux_detector.feed_input(&data.bytes);
                        pending_writes.push_back(data);
                    }
                    Some(SshCommand::Resize {
                        cols,
                        rows,
                        pixel_width,
                        pixel_height,
                    }) => {
                        let channel = channel.clone();
                        pending_resize = Some(Box::pin(async move {
                            channel.window_change(cols.into(), rows.into(), pixel_width.into(), pixel_height.into()).await
                        }));
                    }
                    Some(
                        SshCommand::TmuxCommand { .. }
                        | SshCommand::TmuxDetach
                        | SshCommand::TmuxPaneInput { .. }
                        | SshCommand::TmuxPaneResize { .. }
                        | SshCommand::TmuxPaneClose { .. },
                    ) => {}
                    Some(SshCommand::Close) | None => {
                        break;
                    }
                }
            }
            message = channel_rx.wait(), if pending_output.is_none() => {
                match message {
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        // stderr is not part of tmux's stdout framing.
                        tmux_detector.observe_extended_output(&data);
                        pending_output = Some(ssh_output_future(event_queue.clone(), session_id.clone(), SshIntegrationOutput { visible: data.to_vec(), ..Default::default() }));
                    }
                    Some(ChannelMsg::Data { data }) => {
                        let detection = tmux_detector.feed_output(&data);
                        if detection.notice {
                            event_queue.push_async(SessionEvent::Notice {
                                session_id: session_id.clone(),
                                notice: SessionNotice::TmuxControlUnconfirmed,
                            }).await;
                        }
                        let data = match detection.output {
                            tmux::TmuxFeed::Passthrough(data) => data,
                            tmux::TmuxFeed::Detected {
                                passthrough,
                                startup,
                                rest,
                            } => {
                                // Verification gate, not proof of an active control session.
                                // Disable binary transfers before any awaited
                                // output flush, upload cancellation or input drain.
                                tmux_active.store(true, Ordering::Release);
                                pending_writes.clear();
                                // The confirmed startup frame and everything
                                // after it belong to the control channel, not
                                // the terminal. Bytes before it (the echoed
                                // command line) still go through normally.
                                if !passthrough.is_empty() {
                                    let output = shell_integration.filter_output(&passthrough);
                                    ssh_output_future(event_queue.clone(), session_id.clone(), output).await;
                                }
                                // An in-flight integration upload would keep
                                // writing its script into the control stream;
                                // cancel it before the takeover.
                                cancel_pending_upload(&mut pending_upload).await;
                                let settled = tokio::time::timeout(tmux::handshake::CONFIRM_TIMEOUT, async {
                                    if let Some(send) = pending_send.take() { send.await?; }
                                    if let Some(resize) = pending_resize.take() { resize.await?; }
                                    Ok::<(), russh::Error>(())
                                }).await;
                                if !matches!(settled, Ok(Ok(()))) {
                                    event_queue.push_async(SessionEvent::Exited {
                                        session_id: session_id.clone(),
                                        reason: "SSH input did not settle before tmux takeover; reconnect to restore the terminal".into(),
                                    }).await;
                                    break;
                                }
                                match tmux::handshake::verify(&channel, &mut channel_rx, &startup, &rest).await {
                                    tmux::handshake::Verification::Confirmed(initial) => {
                                        tracing::info!(
                                            session_id = %session_id,
                                            "tmux control mode confirmed; taking over SSH channel"
                                        );
                                        tmux_active.store(true, Ordering::Release);
                                        let exit = tmux::run_control_session(
                                            &event_queue,
                                            &sessions,
                                            &session_id,
                                            &channel,
                                            &mut channel_rx,
                                            &mut command_rx,
                                            &mut tmux_cmd_rx,
                                            tmux_tx.clone(),
                                            &initial,
                                        )
                                        .await;
                                        tmux_active.store(false, Ordering::Release);
                                        match exit {
                                            tmux::ControlExit::ReturnToShell { leftover } => {
                                                // Replay the bytes tmux left
                                                // after %exit and reset
                                                // detection for a possible
                                                // next `tmux -CC`.
                                                if !leftover.is_empty() {
                                                    let output = shell_integration.filter_output(&leftover);
                                                    ssh_output_future(event_queue.clone(), session_id.clone(), output).await;
                                                }
                                                tmux_detector = tmux::TmuxControlDetector::new();
                                            }
                                            tmux::ControlExit::Closed => {
                                                // The control channel died on
                                                // its own (idle drop, network
                                                // loss, server close). Mirror
                                                // the plain-SSH EOF path so the
                                                // session renders disconnected
                                                // instead of swallowing input
                                                // into a dead channel.
                                                event_queue.push_async(SessionEvent::Exited {
                                                    session_id: session_id.clone(),
                                                    reason: "tmux control channel closed".to_string(),
                                                }).await;
                                                break;
                                            }
                                        }
                                    }
                                    tmux::handshake::Verification::Exited(leftover) => {
                                        command_rx.discard_writes();
                                        tmux_cmd_rx.discard_pending();
                                        tmux_active.store(false, Ordering::Release);
                                        let output = shell_integration.filter_output(&leftover);
                                        ssh_output_future(event_queue.clone(), session_id.clone(), output).await;
                                        tmux_detector = tmux::TmuxControlDetector::new();
                                    }
                                    tmux::handshake::Verification::Failed(reason) => {
                                        // Without nonce confirmation neither shell nor tmux
                                        // commands are safe. Close only this terminal channel.
                                        tracing::warn!(session_id = %session_id, ?reason,
                                            "tmux verification failed; closing terminal channel");
                                        event_queue.push_async(SessionEvent::Exited {
                                            session_id: session_id.clone(),
                                            reason: format!("tmux control-mode verification failed ({reason:?}); reconnect to restore the terminal"),
                                        }).await;
                                        break;
                                    }
                                }
                                continue;
                            }
                        };
                        let was_waiting_initial = shell_integration.is_waiting_initial();
                        let output = shell_integration.filter_output(&data);
                        pending_output = Some(ssh_output_future(event_queue.clone(), session_id.clone(), output));
                        if was_waiting_initial
                            && shell_integration.is_waiting_initial()
                            && let Some(integration) = shell_integration.take_pending_integration() {
                            shell_integration.begin_suppression();
                            inject_timeout
                                .as_mut()
                                .reset(
                                    tokio::time::Instant::now()
                                        + ssh_shell_integration::SSH_INTEGRATION_TIMEOUT,
                            );
                            let task_handle = Arc::clone(&handle);
                            let upload_payload = integration.upload_payload();
                            let (cancel, cancel_rx) = tokio::sync::oneshot::channel();
                            let join = tokio::spawn(ssh_shell_integration::upload_integration_script(
                                task_handle,
                                upload_payload,
                                cancel_rx,
                            ));
                            pending_upload = Some(PendingSshIntegrationUpload {
                                integration,
                                cancel: Some(cancel),
                                join,
                            });
                        }
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        event_queue.push_async(SessionEvent::Exited {
                            session_id: session_id.clone(),
                            reason: format!("SSH channel exit status {exit_status}"),
                        }).await;
                        break;
                    }
                    Some(ChannelMsg::Eof) => {
                        event_queue.push_async(SessionEvent::Exited {
                            session_id: session_id.clone(),
                            reason: "SSH channel EOF".to_string(),
                        }).await;
                        break;
                    }
                    Some(ChannelMsg::Close) => {
                        event_queue.push_async(SessionEvent::Exited {
                            session_id: session_id.clone(),
                            reason: "SSH channel closed by remote".to_string(),
                        }).await;
                        break;
                    }
                    None => {
                        event_queue.push_async(SessionEvent::Exited {
                            session_id: session_id.clone(),
                            reason: "SSH connection task ended".to_string(),
                        }).await;
                        break;
                    }
                    Some(_) => {}
                }
            }
        }
    }
    tmux_active.store(false, Ordering::Release);
    drop(pending_send);
    drop(pending_resize);
    drop(pending_output);
    // russh may be parked delivering to its bounded channel queue. Release
    // that receiver before waiting for EOF/disconnect to reach the SSH task.
    drop(channel_rx);
    let _ = tokio::time::timeout(Duration::from_millis(250), async {
        let _ = channel.eof().await;
        let _ = channel.close().await;
    })
    .await;
    cancel_pending_upload(&mut pending_upload).await;

    disconnect_open_ssh_shell(
        &session_id,
        &handle,
        jump_handles,
        disconnect_on_close,
        x11_multiplex_registration,
    )
    .await;
}

async fn emit_ssh_startup_events(
    queue: &SessionEventQueue,
    session_id: &str,
    notice: Option<SessionNotice>,
    early_output: Vec<u8>,
) {
    if let Some(notice) = notice {
        queue
            .push_async(SessionEvent::Notice {
                session_id: session_id.to_string(),
                notice,
            })
            .await;
    }
    // Request-reply waits consume real server bytes before starting the reader.
    // Preserve their charset and ANSI framing separately from local notices.
    if !early_output.is_empty() {
        queue
            .push_async(SessionEvent::Output {
                session_id: session_id.to_string(),
                data: early_output,
            })
            .await;
    }
}

pub(crate) fn ssh_output_future(
    queue: SessionEventQueue,
    session_id: String,
    output: SshIntegrationOutput,
) -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(async move {
        push_ssh_integration_output(&queue, &session_id, output).await;
    })
}

pub(crate) async fn push_ssh_integration_output(
    event_queue: &SessionEventQueue,
    session_id: &str,
    output: SshIntegrationOutput,
) {
    for cwd in output.cwd_paths {
        event_queue
            .push_async(SessionEvent::CwdChanged {
                session_id: session_id.to_string(),
                cwd,
            })
            .await;
    }
    for command in output.accepted_commands {
        event_queue
            .push_async(SessionEvent::CommandAccepted {
                session_id: session_id.to_string(),
                command,
            })
            .await;
    }
    if !output.visible.is_empty() {
        event_queue
            .push_async(SessionEvent::Output {
                session_id: session_id.to_string(),
                data: output.visible,
            })
            .await;
    }
}

pub(crate) async fn open_ssh_shell(
    session_id: &str,
    config: &SshSessionConfig,
    multiplex: Option<&SshMultiplexHandle>,
    shell_environment: Arc<ShellEnvironmentCache>,
) -> anyhow::Result<OpenSshShellSession> {
    let pending = open_pending_ssh_shell(config, multiplex, shell_environment).await?;
    open_ssh_shell_from_pending(
        session_id,
        config,
        pending,
        SshPtyDimensions::from_config(config),
    )
    .await
}

pub(crate) async fn open_pending_ssh_shell(
    config: &SshSessionConfig,
    multiplex: Option<&SshMultiplexHandle>,
    shell_environment: Arc<ShellEnvironmentCache>,
) -> anyhow::Result<PendingOpenSshShellSession> {
    tracing::debug!(
        stage = "connection",
        host = %config.host,
        port = config.port,
        profile = ?config.profile,
        multiplexed = multiplex.is_some(),
        "opening SSH transport"
    );
    let x11_config = if config.x11_forwarding {
        Some(prepare_x11_forwarding(&config.x11_display).await)
    } else {
        None
    };
    let (x11_tx, x11_rx) = if x11_config.is_some() {
        let (tx, rx) = tokio_mpsc::unbounded_channel();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };
    let mut pending_multiplex = None;
    let mut primary_claim = None;
    let (handle, jump_handles, disconnect_on_close) = if let Some(multiplex) = multiplex {
        multiplex.ensure_matches_config(config)?;
        pending_multiplex = Some(multiplex.clone());
        // Claimed here rather than next to the channel open: the desktop reports
        // the session ready as soon as this returns, and with a deferred PTY the
        // channel is not opened until the first resize arrives.
        primary_claim = multiplex.claim_primary_session();
        let handle = multiplex.target_handle();
        (SshShellHandle::Multiplexed(handle), Vec::new(), false)
    } else {
        let (handle, jump_handles) = open_authenticated_ssh_handle_with_channel_senders(
            config,
            None,
            x11_tx.clone(),
            shell_environment,
        )
        .await?;
        (SshShellHandle::Dedicated(handle), jump_handles, true)
    };

    Ok(PendingOpenSshShellSession {
        handle,
        jump_handles,
        disconnect_on_close,
        multiplex: pending_multiplex,
        primary_claim,
        x11_config,
        x11_tx,
        x11_rx,
    })
}

/// Requests sharing a channel must consume their own acknowledgement, otherwise
/// an X11 reply can be mistaken for acceptance of the shell request.
pub(crate) async fn wait_ssh_request_reply(
    channel: &mut russh::Channel<client::Msg>,
    early_output: &mut Vec<u8>,
) -> Result<(), russh::Error> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match channel.wait().await {
                Some(ChannelMsg::Success) => return Ok(()),
                Some(ChannelMsg::Failure) => return Err(russh::Error::RequestDenied),
                Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                    if early_output.len().saturating_add(data.len()) > 1024 * 1024 {
                        return Err(russh::Error::Disconnect);
                    }
                    early_output.extend_from_slice(&data);
                }
                Some(ChannelMsg::Close | ChannelMsg::Eof) | None => {
                    return Err(russh::Error::Disconnect);
                }
                _ => {}
            }
        }
    })
    .await
    .map_err(|_| russh::Error::ConnectionTimeout)?
}

pub(crate) async fn open_ssh_shell_from_pending(
    session_id: &str,
    config: &SshSessionConfig,
    pending: PendingOpenSshShellSession,
    dimensions: SshPtyDimensions,
) -> anyhow::Result<OpenSshShellSession> {
    let PendingOpenSshShellSession {
        mut handle,
        jump_handles,
        disconnect_on_close,
        multiplex,
        primary_claim,
        x11_config,
        x11_tx,
        x11_rx,
    } = pending;
    let ready_marker = build_ssh_ready_marker(session_id);
    let legacy_ready_marker = build_legacy_ssh_ready_marker(&ready_marker);
    let attempt = multiplex
        .as_ref()
        .map(|m| &m.inner.attempt)
        .unwrap_or(&config.attempt);
    attempt.ensure_shell_available()?;
    let mut early_output = Vec::new();
    let mut channel = match &mut handle {
        SshShellHandle::Dedicated(handle) => handle.channel_open_session().await?,
        SshShellHandle::Multiplexed(handle) => handle.lock().await.channel_open_session().await?,
    };
    if effective_agent_forwarding_config(config).is_some_and(|forwarding| forwarding.enabled)
        && let Err(error) = channel.agent_forward(false).await
    {
        let _ = channel.close().await;
        return Err(error.into());
    }
    tracing::debug!(
        stage = "interactive-channel",
        host = %config.host,
        port = config.port,
        profile = ?config.profile,
        "opened SSH session channel"
    );
    let (x11_forwarder, x11_multiplex_registration, local_notice) =
        if let (Some(config), Some(rx)) = (x11_config, x11_rx) {
            if let Some(multiplex) = multiplex.as_ref() {
                let Some(tx) = x11_tx.clone() else {
                    let _ = channel.close().await;
                    anyhow::bail!("X11 forwarding sender is unavailable");
                };
                if let Err(error) = multiplex.register_x11_sender(session_id, tx).await {
                    let _ = channel.close().await;
                    return Err(error);
                }
            }
            let request = async {
                channel
                    .request_x11(true, false, MIT_MAGIC_COOKIE, &config.fake_cookie_hex, 0)
                    .await?;
                wait_ssh_request_reply(&mut channel, &mut early_output).await
            }
            .await;
            match request {
                Ok(()) => (Some(X11Forwarder { rx, config }), multiplex.clone(), None),
                Err(error) => {
                    if let Some(multiplex) = multiplex.as_ref() {
                        multiplex.unregister_x11_sender(session_id).await;
                    }
                    // Only an explicit denial consumes a definite request reply.
                    // A timed-out reply could otherwise be mistaken for shell acceptance.
                    if !matches!(error, russh::Error::RequestDenied) {
                        let _ = channel.close().await;
                        return Err(error.into());
                    }
                    (None, None, Some(SessionNotice::X11ForwardingDenied))
                }
            }
        } else {
            (None, None, None)
        };
    if let Err(error) = channel
        .request_pty(
            false,
            &config.term,
            dimensions.cols.into(),
            dimensions.rows.into(),
            dimensions.pixel_width.into(),
            dimensions.pixel_height.into(),
            &[],
        )
        .await
    {
        if let Some(multiplex) = x11_multiplex_registration.as_ref() {
            multiplex.unregister_x11_sender(session_id).await;
        }
        let _ = channel.close().await;
        return Err(error.into());
    }
    tracing::debug!(
        stage = "pty",
        host = %config.host,
        port = config.port,
        profile = ?config.profile,
        term = %config.term,
        cols = dimensions.cols,
        rows = dimensions.rows,
        "SSH PTY accepted"
    );
    let shell_reply = async {
        channel.request_shell(true).await?;
        wait_ssh_request_reply(&mut channel, &mut early_output).await
    }
    .await;
    if let Err(error) = shell_reply {
        if matches!(error, russh::Error::RequestDenied) {
            attempt.record_shell_reply(false);
            config.attempt.record_shell_reply(false);
        }
        if let Some(multiplex) = x11_multiplex_registration.as_ref() {
            multiplex.unregister_x11_sender(session_id).await;
        }
        let _ = channel.close().await;
        return Err(error.into());
    }
    attempt.record_shell_reply(true);
    config.attempt.record_shell_reply(true);
    // The explicit reply wait completed, so `sshd` has already run
    // `do_exec` for this channel and flushed the login banner into it. Anything
    // else on this connection is free to open its own channel now.
    drop(primary_claim);
    tracing::debug!(
        stage = "shell",
        host = %config.host,
        port = config.port,
        profile = ?config.profile,
        "SSH shell accepted"
    );
    let cwd_follow_mode = config.effective_cwd_follow_mode();
    let terminal_shell_integration = config.effective_terminal_shell_integration();
    let shell_kind =
        if terminal_shell_integration || !matches!(cwd_follow_mode, SftpCwdFollowMode::Off) {
            detect_ssh_shell_type(
                &handle,
                config.shell_integration_detection_timeout_ms(cwd_follow_mode),
            )
            .await
        } else {
            None
        };
    let prepared_shell_integration = match shell_kind {
        Some(kind) => {
            build_ssh_shell_integration_script(
                &handle,
                kind,
                &ready_marker,
                terminal_shell_integration,
                cwd_follow_mode,
                config.sftp.shell_detection_timeout_ms,
            )
            .await
        }
        None => None,
    };
    tracing::debug!(
        stage = "integration",
        host = %config.host,
        port = config.port,
        profile = ?config.profile,
        cwd_follow_mode = ?cwd_follow_mode,
        shell_detected = shell_kind.is_some(),
        integration_enabled = prepared_shell_integration.is_some(),
        "resolved SSH shell integration"
    );
    Ok(OpenSshShellSession {
        attempt: config.attempt.clone(),
        post_login: config.post_login.clone(),
        handle,
        channel,
        jump_handles,
        disconnect_on_close,
        x11_forwarder,
        x11_multiplex_registration,
        local_notice,
        early_output,
        prepared_shell_integration,
        ready_marker,
        legacy_ready_marker,
        command_context: {
            let dialect = match shell_kind {
                Some(ShellKind::Bash) => nyaterm_core::completion::types::ShellDialect::Bash,
                Some(ShellKind::Zsh) => nyaterm_core::completion::types::ShellDialect::Zsh,
                Some(ShellKind::Fish) => nyaterm_core::completion::types::ShellDialect::Fish,
                _ => nyaterm_core::completion::types::ShellDialect::Unknown,
            };
            nyaterm_core::completion::types::ShellCommandContext {
                scope: nyaterm_core::completion::types::CompletionScope::new(&[
                    "ssh",
                    &config.host.to_ascii_lowercase(),
                    &config.port.to_string(),
                    &config.username,
                    &format!("{dialect:?}"),
                ]),
                dialect,
                command_events: terminal_shell_integration
                    && matches!(
                        dialect,
                        nyaterm_core::completion::types::ShellDialect::Bash
                            | nyaterm_core::completion::types::ShellDialect::Zsh
                            | nyaterm_core::completion::types::ShellDialect::Fish
                    ),
            }
        },
        shell_kind,
    })
}

pub(crate) async fn disconnect_pending_ssh_shell(session: PendingOpenSshShellSession) {
    if session.disconnect_on_close {
        if let SshShellHandle::Dedicated(handle) = session.handle {
            let _ = handle
                .disconnect(Disconnect::ByApplication, "session closed", "en")
                .await;
        }
        for jump_handle in session.jump_handles {
            let _ = jump_handle
                .disconnect(Disconnect::ByApplication, "session closed", "en")
                .await;
        }
    }
}

pub(crate) async fn disconnect_open_ssh_shell(
    session_id: &str,
    handle: &SshShellHandle,
    jump_handles: Vec<client::Handle<SshClientHandler>>,
    disconnect_on_close: bool,
    x11_multiplex_registration: Option<SshMultiplexHandle>,
) {
    if let Some(multiplex) = x11_multiplex_registration {
        multiplex.unregister_x11_sender(session_id).await;
    }
    if disconnect_on_close {
        if let SshShellHandle::Dedicated(handle) = handle {
            let _ = handle
                .disconnect(Disconnect::ByApplication, "session closed", "en")
                .await;
        }
        for jump_handle in jump_handles {
            let _ = jump_handle
                .disconnect(Disconnect::ByApplication, "session closed", "en")
                .await;
        }
    }
}

pub(crate) fn send_session_error(
    event_queue: &SessionEventQueue,
    session_id: &str,
    error: impl std::fmt::Display,
) {
    event_queue.push(SessionEvent::Error {
        session_id: session_id.to_string(),
        message: error.to_string(),
    });
}

impl SshPtyDimensions {
    pub(crate) fn new(cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) -> Self {
        Self {
            cols: cols.max(1),
            rows: rows.max(1),
            pixel_width,
            pixel_height,
        }
    }

    pub(crate) fn from_config(config: &SshSessionConfig) -> Self {
        Self::new(
            config.cols,
            config.rows,
            config.pixel_width,
            config.pixel_height,
        )
    }
}

#[cfg(test)]
mod notice_tests {
    use super::{emit_ssh_startup_events, run_sftp_only_session};
    use crate::session_event_queue::SessionEventQueue;
    use crate::session_notice::SessionNotice;
    use crate::{SessionEvent, ssh_mailbox};

    #[tokio::test]
    async fn startup_notice_preserves_server_bytes_and_does_not_end_the_session() {
        let queue = SessionEventQueue::new();
        let bytes = b"Welcome\r\n\x1b[31mserver\xff\x1b[0m".to_vec();
        emit_ssh_startup_events(
            &queue,
            "ssh",
            Some(SessionNotice::X11ForwardingDenied),
            bytes.clone(),
        )
        .await;
        assert!(!queue.session_ended("ssh"));
        let drain = queue.drain(10);
        assert_eq!(drain.stats.drained_output_bytes, bytes.len());
        assert!(matches!(
            &drain.events[0],
            SessionEvent::Notice {
                notice: SessionNotice::X11ForwardingDenied,
                ..
            }
        ));
        assert!(matches!(&drain.events[1], SessionEvent::Output { data, .. } if data == &bytes));
    }

    #[tokio::test]
    async fn sftp_only_session_does_not_emit_a_terminal_ready_banner() {
        let queue = SessionEventQueue::new();
        let (sender, receiver) = ssh_mailbox::channel();
        drop(sender);
        run_sftp_only_session("sftp", receiver, &queue, None).await;
        assert!(queue.drain(10).events.is_empty());
    }
}
