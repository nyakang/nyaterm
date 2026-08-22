use std::time::{Duration, Instant};

use futures::StreamExt as _;
use gpui::Context;
use nyaterm_core::{AiExecutionProfile, uuid};
use nyaterm_transport::{
    LocalSessionConfig, SerialSessionConfig, SessionInfo, SessionKind, SessionManager,
    SshSessionConfig, TelnetSessionConfig, open_ssh_multiplex_handle_with_environment,
};

use super::super::state::{failed_session_start_display_name, pending_session_start_display_name};
use super::{MultiplexSshStartRequest, PendingSessionStartRegistration};
use crate::features::formatting::{session_kind_label, short_id, ssh_multiplex_key};
use crate::features::{
    NyaTermApp, runtime_jobs::SessionStartResult, runtime_jobs::SessionStartSuccess,
    session::PendingSessionStart, session::SavedConnectionStartOptions,
    session::SessionStartEventRequest,
};
use crate::models::{NavItem, SessionLaunchConfig, SessionRuntimeMetadata};

impl NyaTermApp {
    pub(in crate::features) fn prepare_session_start_options(
        &mut self,
        mut options: SavedConnectionStartOptions,
    ) -> SavedConnectionStartOptions {
        if options.reconnect_session_id.is_some() || options.tab_placement.is_some() {
            return options;
        }

        let insert_index = options
            .insert_index
            .or_else(|| {
                options.after_session_id.as_ref().and_then(|after_id| {
                    self.ordered_tab_sessions()
                        .iter()
                        .position(|session| session.id == *after_id)
                        .map(|index| index + 1)
                })
            })
            .unwrap_or_else(|| {
                self.ordered_tab_session_count()
                    .saturating_add(self.session.start_visible_tab_reservation_count())
            });
        options.tab_placement = Some(self.session.start.allocate_tab_placement(insert_index));
        options
    }

    pub(in crate::features) fn select_pending_session_start(
        &mut self,
        request_id: String,
        cx: &mut Context<Self>,
    ) {
        if !self.session.start.select_pending(&request_id) {
            return;
        }
        self.shell.close_open_tabs_menu();
        self.shell.close_new_session_menu();
        self.shell.show_workspace();
        cx.notify();
    }

    pub(in crate::features) fn close_pending_session_start(
        &mut self,
        request_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.session.start.close_pending(&request_id) else {
            return;
        };
        self.shell.set_status(format!(
            "cancelled connection {}",
            pending_session_start_display_name(&pending)
        ));
        self.settle_session_start_tab_placements_if_idle();
        cx.notify();
    }

    pub(in crate::features) fn select_failed_session_start(
        &mut self,
        request_id: String,
        cx: &mut Context<Self>,
    ) {
        if !self.session.start.select_failed(&request_id) {
            return;
        }
        self.shell.close_open_tabs_menu();
        self.shell.close_new_session_menu();
        self.shell.show_workspace();
        cx.notify();
    }

    pub(in crate::features) fn close_failed_session_start(
        &mut self,
        request_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(failed) = self.session.start.close_failed(&request_id) else {
            return;
        };
        if !self.session.start.has_failed() {
            self.shell.clear_last_connect_failure();
        }
        self.shell.set_status(format!(
            "closed failed connection {}",
            failed_session_start_display_name(&failed)
        ));
        self.settle_session_start_tab_placements_if_idle();
        cx.notify();
    }

    pub(in crate::features) fn register_pending_session_start(
        &mut self,
        registration: PendingSessionStartRegistration,
        cx: &mut Context<Self>,
    ) -> String {
        let request_id = uuid();
        let PendingSessionStartRegistration {
            connection_name,
            launch_config,
            kind,
            ai_execution_profile,
            custom_name,
            tab_color,
            locked,
            after_session_id,
            insert_index,
            seed_output,
            startup_command,
            multiplex_key,
            source_connection_id,
            reconnect_session_id,
            workspace_split,
            tab_placement,
            status_message,
            append_start_log,
        } = registration;
        let requested_at = tab_placement
            .map(|placement| placement.requested_at)
            .unwrap_or_else(Instant::now);

        let reconnecting = self.session.start.register_pending(
            request_id.clone(),
            PendingSessionStart {
                connection_name: connection_name.clone(),
                launch_config,
                requested_at,
                kind,
                ai_execution_profile,
                custom_name,
                tab_color,
                locked,
                after_session_id,
                insert_index,
                seed_output,
                startup_command,
                multiplex_key,
                source_connection_id,
                workspace_split,
                tab_placement,
                reconnect_session_id,
            },
        );
        if !reconnecting {
            self.shell.clear_last_connect_failure();
        }
        self.shell.set_status(status_message);
        // Status + connecting tab already show progress; avoid full terminal decode
        // work on the click path before the worker even starts.
        let _ = append_start_log;
        self.shell.show_workspace();
        cx.notify();
        request_id
    }

    pub(in crate::features) fn begin_background_session_start(
        &mut self,
        connection_name: String,
        launch_config: SessionLaunchConfig,
        source_connection_id: Option<String>,
        ai_execution_profile: AiExecutionProfile,
        options: SavedConnectionStartOptions,
        cx: &mut Context<Self>,
    ) {
        let options = self.prepare_session_start_options(options);
        let SavedConnectionStartOptions {
            custom_name,
            tab_color,
            locked,
            after_session_id,
            insert_index,
            seed_output,
            startup_command,
            reconnect_session_id,
            workspace_split,
            tab_placement,
        } = options;
        let kind = session_kind_for_launch_config(&launch_config);
        let multiplex_key = match &launch_config {
            SessionLaunchConfig::Ssh(config) => Some(ssh_multiplex_key(config)),
            _ => None,
        };
        let request_id = self.register_pending_session_start(
            PendingSessionStartRegistration {
                connection_name: connection_name.clone(),
                launch_config: Some(launch_config.clone()),
                kind,
                ai_execution_profile,
                custom_name,
                tab_color,
                locked,
                after_session_id,
                insert_index,
                seed_output,
                startup_command,
                multiplex_key,
                source_connection_id,
                reconnect_session_id,
                workspace_split,
                tab_placement,
                status_message: format!("connecting to {connection_name}"),
                append_start_log: true,
            },
            cx,
        );

        let session_manager = self.session.manager_handle();
        let session_start_tx = self.session.start.sender();
        let request_id_for_worker = request_id.clone();
        std::thread::spawn(move || {
            let worker_started_at = Instant::now();
            let result = create_session_from_launch_config(&session_manager, launch_config.clone())
                .map(
                    |(session_info, multiplex_handle, ssh_start_handle)| SessionStartSuccess {
                        session_info,
                        multiplex_handle,
                        ssh_start_handle,
                        launch_config: Some(launch_config),
                    },
                )
                .map_err(|error| error.to_string());
            let worker_finished_at = Instant::now();
            let _ = session_start_tx.unbounded_send(SessionStartResult {
                request_id: request_id_for_worker,
                connection_name,
                kind,
                worker_started_at,
                worker_finished_at,
                result,
            });
        });
    }

    pub(in crate::features) fn begin_background_ssh_start(
        &mut self,
        connection_name: String,
        mut config: SshSessionConfig,
        source_connection_id: Option<String>,
        ai_execution_profile: AiExecutionProfile,
        options: SavedConnectionStartOptions,
        cx: &mut Context<Self>,
    ) {
        let options = self.prepare_session_start_options(options);
        let SavedConnectionStartOptions {
            custom_name,
            tab_color,
            locked,
            after_session_id,
            insert_index,
            seed_output,
            startup_command,
            reconnect_session_id,
            workspace_split,
            tab_placement,
        } = options;
        // Open the PTY as soon as the registration barrier is released. The terminal
        // surface sends its actual viewport resize after layout; waiting for that command
        // here adds a fixed startup delay before the first remote bytes can be read.
        config.deferred_pty = false;
        let geometry_session_hint = after_session_id
            .as_deref()
            .or(reconnect_session_id.as_deref());
        if let Some(geometry) =
            self.desired_terminal_resize_geometry_for_session_hint(geometry_session_hint)
        {
            config.cols = geometry.cols;
            config.rows = geometry.rows;
            config.pixel_width = geometry.pixel_width;
            config.pixel_height = geometry.pixel_height;
        }
        let multiplex_key = ssh_multiplex_key(&config);
        let request_id = self.register_pending_session_start(
            PendingSessionStartRegistration {
                connection_name: connection_name.clone(),
                launch_config: Some(SessionLaunchConfig::Ssh(Box::new(config.clone()))),
                kind: SessionKind::Ssh,
                ai_execution_profile,
                custom_name,
                tab_color,
                locked,
                after_session_id,
                insert_index,
                seed_output,
                startup_command,
                multiplex_key: Some(multiplex_key),
                source_connection_id,
                reconnect_session_id,
                workspace_split,
                tab_placement,
                status_message: format!("connecting to {connection_name}"),
                append_start_log: true,
            },
            cx,
        );

        let session_manager = self.session.manager_handle();
        let session_start_tx = self.session.start.sender();
        let request_id_for_worker = request_id.clone();
        std::thread::spawn(move || {
            let worker_started_at = Instant::now();
            let result = (|| {
                let (session_info, multiplex, ssh_start_handle) = session_manager
                    .create_ssh_session_with_shared_handle_deferred(config.clone())
                    .map_err(|error| error.to_string())?;
                Ok(SessionStartSuccess {
                    session_info,
                    multiplex_handle: Some(multiplex),
                    ssh_start_handle: Some(ssh_start_handle),
                    launch_config: Some(SessionLaunchConfig::Ssh(Box::new(config))),
                })
            })();
            let worker_finished_at = Instant::now();
            let _ = session_start_tx.unbounded_send(SessionStartResult {
                request_id: request_id_for_worker,
                connection_name,
                kind: SessionKind::Ssh,
                worker_started_at,
                worker_finished_at,
                result,
            });
        });
    }

    pub(in crate::features) fn begin_background_multiplex_ssh_start(
        &mut self,
        request: MultiplexSshStartRequest,
        cx: &mut Context<Self>,
    ) {
        let MultiplexSshStartRequest {
            connection_name,
            mut config,
            source_connection_id,
            ai_execution_profile,
            options,
            existing_multiplex,
        } = request;
        let options = self.prepare_session_start_options(options);
        let SavedConnectionStartOptions {
            custom_name,
            tab_color,
            locked,
            after_session_id,
            insert_index,
            seed_output,
            startup_command,
            reconnect_session_id,
            workspace_split,
            tab_placement,
        } = options;
        // Reused SSH channels follow the same immediate PTY-open path as fresh connections.
        config.deferred_pty = false;
        let geometry_session_hint = after_session_id
            .as_deref()
            .or(reconnect_session_id.as_deref());
        if let Some(geometry) =
            self.desired_terminal_resize_geometry_for_session_hint(geometry_session_hint)
        {
            config.cols = geometry.cols;
            config.rows = geometry.rows;
            config.pixel_width = geometry.pixel_width;
            config.pixel_height = geometry.pixel_height;
        }
        let multiplex_key = ssh_multiplex_key(&config);
        let request_id = self.register_pending_session_start(
            PendingSessionStartRegistration {
                connection_name: connection_name.clone(),
                launch_config: Some(SessionLaunchConfig::Ssh(Box::new(config.clone()))),
                kind: SessionKind::Ssh,
                ai_execution_profile,
                custom_name,
                tab_color,
                locked,
                after_session_id,
                insert_index,
                seed_output,
                startup_command,
                multiplex_key: Some(multiplex_key.clone()),
                source_connection_id,
                reconnect_session_id,
                workspace_split,
                tab_placement,
                status_message: format!("multiplexing SSH session {connection_name}"),
                append_start_log: false,
            },
            cx,
        );

        let session_manager = self.session.manager_handle();
        let session_start_tx = self.session.start.sender();
        let request_id_for_worker = request_id.clone();
        std::thread::spawn(move || {
            let worker_started_at = Instant::now();
            let result = (|| {
                let multiplex = match existing_multiplex {
                    Some(handle) if !handle.is_closed() => handle,
                    _ => open_ssh_multiplex_handle_with_environment(
                        config.clone(),
                        session_manager.shell_environment(),
                    )
                    .map_err(|error| error.to_string())?,
                };
                let (info, ssh_start_handle) = session_manager
                    .create_ssh_session_with_multiplex_deferred(config.clone(), multiplex.clone())
                    .map_err(|error| error.to_string())?;
                Ok(SessionStartSuccess {
                    session_info: info,
                    multiplex_handle: Some(multiplex),
                    ssh_start_handle: Some(ssh_start_handle),
                    launch_config: Some(SessionLaunchConfig::Ssh(Box::new(config))),
                })
            })();
            let worker_finished_at = Instant::now();
            let _ = session_start_tx.unbounded_send(SessionStartResult {
                request_id: request_id_for_worker,
                connection_name,
                kind: SessionKind::Ssh,
                worker_started_at,
                worker_finished_at,
                result,
            });
        });
    }

    pub(in crate::features) fn send_probe_command(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.session.active_id_owned() else {
            self.shell.set_status("start a session first".to_string());
            cx.notify();
            return;
        };
        if self.session.is_disconnected(&session_id) {
            self.shell
                .set_status("session disconnected — reconnect before probing".to_string());
            cx.notify();
            return;
        }

        let command = if cfg!(target_os = "windows") {
            "echo nyaterm-app-ready\r\n"
        } else {
            "printf 'nyaterm-app-ready\\n'\n"
        };
        match self.write_session_input_recorded(&session_id, command.as_bytes()) {
            Ok(()) => {
                self.shell.set_status("probe command sent".to_string());
            }
            Err(error) => {
                self.shell.set_status(format!("write failed: {error}"));
            }
        }
        cx.notify();
    }

    /// Deliver session-start results as they arrive.
    ///
    /// Started once at window open. Before this the runtime tick polled
    /// `try_recv`, so a connect result -- success or failure -- waited for the
    /// next tick. `runtime_quiet_tick_allowed` still carries
    /// `session.start_has_pending()`, but now only because a pending start also
    /// gates the prompt activation and tab-placement work the control plane does,
    /// not to keep this delivery fast.
    pub(in crate::features) fn start_session_start_event_drain(&mut self, cx: &mut Context<Self>) {
        let Some(mut rx) = self.session.start.take_event_receiver() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                if this
                    .update(cx, |this, cx| {
                        this.apply_session_start_event(event, cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply_session_start_event(&mut self, event: SessionStartResult, cx: &mut Context<Self>) {
        let request_id = event.request_id.clone();
        let (pending, was_active_pending) =
            match self.session.start.take_event_request(&event.request_id) {
                SessionStartEventRequest::Cancelled => {
                    if let Ok(success) = event.result {
                        let session_id = success.session_info.id;
                        if let Err(error) = self.session.manager().close(&session_id) {
                            tracing::warn!(
                                request_id = %request_id,
                                session_id = %session_id,
                                error = %error,
                                "failed to close cancelled session start result"
                            );
                        }
                    }
                    tracing::debug!(
                        request_id = %request_id,
                        "discarded cancelled session start result"
                    );
                    return;
                }
                SessionStartEventRequest::Pending {
                    pending,
                    was_active,
                } => (pending, was_active),
            };
        let connection_name = event.connection_name.clone();
        let kind = pending
            .as_ref()
            .map(|pending| pending.kind)
            .unwrap_or(event.kind);
        let requested_at = pending.as_ref().map(|pending| pending.requested_at);
        let worker_duration = event
            .worker_finished_at
            .saturating_duration_since(event.worker_started_at);
        let worker_to_ui_duration =
            Instant::now().saturating_duration_since(event.worker_finished_at);
        match event.result {
            Ok(success) => {
                let ui_register_started_at = Instant::now();
                self.shell.clear_last_connect_failure();
                let session_info = success.session_info;
                let session_id = session_info.id.clone();
                let ssh_start_handle = success.ssh_start_handle;
                let reconnect_session_id = pending
                    .as_ref()
                    .and_then(|pending| pending.reconnect_session_id.clone());
                let workspace_split = pending
                    .as_ref()
                    .and_then(|pending| pending.workspace_split.clone());
                if reconnect_session_id
                    .as_deref()
                    .is_some_and(|stale_id| !self.session.has_session(stale_id))
                {
                    if let Err(error) = self.session.manager().close(&session_id) {
                        tracing::warn!(
                            request_id = %request_id,
                            session_id = %session_id,
                            error = %error,
                            "failed to close stale reconnect result"
                        );
                    }
                    return;
                }
                let launch_config = success
                    .launch_config
                    .or_else(|| {
                        pending
                            .as_ref()
                            .and_then(|pending| pending.launch_config.clone())
                    })
                    .unwrap_or_else(|| launch_config_for_session_info(&session_info));
                let ssh_config = match &launch_config {
                    SessionLaunchConfig::Ssh(config) => Some(config.as_ref().clone()),
                    _ => None,
                };
                let ssh_multiplex_key = pending
                    .as_ref()
                    .and_then(|pending| pending.multiplex_key.clone());
                if let (Some(key), Some(handle)) =
                    (ssh_multiplex_key.clone(), success.multiplex_handle)
                {
                    self.session.register_multiplex_handle(key, handle);
                }
                let source_connection_id = pending
                    .as_ref()
                    .and_then(|pending| pending.source_connection_id.clone());
                let tab_placement = pending.as_ref().and_then(|pending| pending.tab_placement);
                let fallback_insert_index = pending
                    .as_ref()
                    .and_then(|pending| pending.insert_index)
                    .filter(|_| tab_placement.is_none());
                if let Some(connection_id) = source_connection_id.as_ref() {
                    self.persist_connection_used(connection_id.clone(), cx);
                }
                let ai_execution_profile = pending
                    .as_ref()
                    .map(|pending| pending.ai_execution_profile)
                    .unwrap_or(AiExecutionProfile::SendOnly);
                self.register_session_for_start(
                    &session_id,
                    SessionRuntimeMetadata {
                        ssh_config,
                        ssh_multiplex_key,
                        source_connection_id,
                        ai_execution_profile,
                        launch_config,
                        disconnected: false,
                    },
                    tab_placement,
                    fallback_insert_index,
                );
                if let Some(custom_name) = pending
                    .as_ref()
                    .and_then(|pending| pending.custom_name.clone())
                {
                    self.session
                        .set_custom_name(session_id.clone(), custom_name);
                }
                if let Some(tab_color) = pending.as_ref().and_then(|pending| pending.tab_color) {
                    self.session.set_tab_color(&session_id, Some(tab_color));
                }
                if pending.as_ref().is_some_and(|pending| pending.locked) {
                    self.session.set_tab_locked(&session_id, true);
                }
                if let Some(seed_output) = pending
                    .as_ref()
                    .and_then(|pending| pending.seed_output.clone())
                {
                    let encoding = self
                        .session
                        .metadata(&session_id)
                        .and_then(|metadata| metadata.launch_config.encoding())
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| {
                            self.settings.summary().interaction_default_encoding.clone()
                        });
                    self.seed_terminal_frame_session(&session_id, seed_output.clone(), &encoding);
                    self.terminal
                        .seed_session_view(session_id.clone(), seed_output, &encoding);
                }
                // Keep the authenticated transport paused until the session metadata, frame
                // pipeline, and reconnect seed are installed, so the first banner cannot race
                // the UI registration path.
                if let Some(ssh_start_handle) = ssh_start_handle {
                    ssh_start_handle.start();
                }
                if tab_placement.is_none()
                    && fallback_insert_index.is_none()
                    && let Some(after_session_id) = pending
                        .as_ref()
                        .and_then(|pending| pending.after_session_id.clone())
                {
                    self.session
                        .move_session_after(&session_id, &after_session_id);
                }
                if let Some(stale_id) = reconnect_session_id
                    && stale_id != session_id
                {
                    self.migrate_reconnected_session_state(&stale_id, &session_id);
                    self.remove_session_state(&stale_id);
                    self.persist_workspace_pane_layout();
                    self.persist_terminal_window_layout();
                }
                let should_activate = self
                    .session
                    .start
                    .complete_success(was_active_pending, self.session.active_id().is_none());
                if should_activate {
                    self.activate_session_id(&session_id);
                    self.load_transfer_browser_for_active_session_if_needed(cx);
                }
                // First connected frames often land with a login banner burst.
                // Enter degraded paint immediately so tab-strip/status repaint
                // does not stack full terminal decorations on connect.
                self.enter_connect_settle();
                self.terminal.enter_session_render_degraded(&session_id);
                self.shell.set_status(format!(
                    "running {} · {}",
                    short_id(&session_id),
                    event.connection_name
                ));
                // Do not append local log text through the full terminal decode path
                // on connect success — that competes with the first SSH/PTY frames.
                // Auto-recording file open is deferred to the idle plane.
                if self.settings.summary().recording_auto_start {
                    self.recording
                        .schedule_auto_start(session_id.clone(), session_info.name.clone());
                }
                self.apply_workspace_split_for_duplicate(workspace_split, &session_id);
                if let Some(startup_command) = pending.and_then(|pending| pending.startup_command) {
                    self.schedule_startup_command(session_id.clone(), startup_command, cx);
                }
                self.shell.select_nav(NavItem::Workspace);
                let ui_register_duration = ui_register_started_at.elapsed();
                let request_to_ui_duration = requested_at
                    .map(|requested_at| requested_at.elapsed())
                    .unwrap_or(worker_duration + worker_to_ui_duration + ui_register_duration);
                tracing::debug!(
                    diagnostic = "session_start",
                    request_id = %request_id,
                    connection_name = %connection_name,
                    kind = session_kind_label(kind),
                    session_id = %session_id,
                    worker_duration_ms = worker_duration.as_millis(),
                    worker_to_ui_ms = worker_to_ui_duration.as_millis(),
                    ui_register_ms = ui_register_duration.as_millis(),
                    request_to_ui_ms = request_to_ui_duration.as_millis(),
                    "session start completed"
                );
                if (worker_duration >= SESSION_START_SLOW_THRESHOLD
                    || request_to_ui_duration >= SESSION_START_SLOW_THRESHOLD
                    || ui_register_duration >= SESSION_START_SLOW_THRESHOLD)
                    && self.should_log_slow_diagnostic("session_start", Instant::now())
                {
                    tracing::warn!(
                        diagnostic = "session_start",
                        request_id = %request_id,
                        connection_name = %connection_name,
                        kind = session_kind_label(kind),
                        session_id = %session_id,
                        worker_duration_ms = worker_duration.as_millis(),
                        worker_to_ui_ms = worker_to_ui_duration.as_millis(),
                        ui_register_ms = ui_register_duration.as_millis(),
                        request_to_ui_ms = request_to_ui_duration.as_millis(),
                        "slow session start"
                    );
                }
            }
            Err(error) => {
                let reconnect_session_id = pending
                    .as_ref()
                    .and_then(|pending| pending.reconnect_session_id.clone());
                let reconnect_session_exists = reconnect_session_id
                    .as_deref()
                    .is_some_and(|session_id| self.session.has_session(session_id));
                let reconnect_failure = self.session.start.record_failure(
                    request_id.clone(),
                    pending,
                    error.clone(),
                    was_active_pending,
                    reconnect_session_exists,
                );
                if !reconnect_failure {
                    self.shell
                        .set_last_connect_failure(connection_name.clone(), error.clone());
                }
                self.shell
                    .set_status(format!("failed to start {connection_name}: {error}"));
                if self.session.active_id().is_none() {
                    self.append_terminal_log(format!(
                        "\n# failed to start {}: {error}\n",
                        connection_name
                    ));
                }
                self.shell.select_nav(NavItem::Workspace);
                let request_to_ui_duration = requested_at
                    .map(|requested_at| requested_at.elapsed())
                    .unwrap_or(worker_duration + worker_to_ui_duration);
                tracing::warn!(
                    diagnostic = "session_start",
                    request_id = %request_id,
                    connection_name = %connection_name,
                    kind = session_kind_label(kind),
                    worker_duration_ms = worker_duration.as_millis(),
                    worker_to_ui_ms = worker_to_ui_duration.as_millis(),
                    request_to_ui_ms = request_to_ui_duration.as_millis(),
                    error = %error,
                    "session start failed"
                );
            }
        }

        self.settle_session_start_tab_placements_if_idle();
    }
}

fn session_kind_for_launch_config(config: &SessionLaunchConfig) -> SessionKind {
    match config {
        SessionLaunchConfig::Local(_) => SessionKind::LocalPty,
        SessionLaunchConfig::Ssh(_) => SessionKind::Ssh,
        SessionLaunchConfig::Telnet(config) if config.raw_tcp => SessionKind::RawTcp,
        SessionLaunchConfig::Telnet(_) => SessionKind::Telnet,
        SessionLaunchConfig::Serial(_) => SessionKind::Serial,
        SessionLaunchConfig::Rdp(_) => SessionKind::Rdp,
        SessionLaunchConfig::Vnc(_) => SessionKind::Vnc,
    }
}

const SESSION_START_SLOW_THRESHOLD: Duration = Duration::from_millis(500);

fn create_session_from_launch_config(
    session_manager: &SessionManager,
    launch_config: SessionLaunchConfig,
) -> Result<
    (
        SessionInfo,
        Option<nyaterm_transport::SshMultiplexHandle>,
        Option<nyaterm_transport::SshSessionStartHandle>,
    ),
    nyaterm_transport::SessionError,
> {
    match launch_config {
        SessionLaunchConfig::Local(config) => session_manager
            .create_local_session(config)
            .map(|info| (info, None, None)),
        SessionLaunchConfig::Ssh(config) => session_manager
            .create_ssh_session_with_shared_handle_deferred(*config)
            .map(|(info, handle, start)| (info, Some(handle), Some(start))),
        SessionLaunchConfig::Telnet(config) => session_manager
            .create_telnet_session(config)
            .map(|info| (info, None, None)),
        SessionLaunchConfig::Serial(config) => session_manager
            .create_serial_session(config)
            .map(|info| (info, None, None)),
        SessionLaunchConfig::Rdp(_) | SessionLaunchConfig::Vnc(_) => {
            unreachable!("remote desktop sessions are created by RemoteDesktopFeatureState")
        }
    }
}

fn launch_config_for_session_info(info: &SessionInfo) -> SessionLaunchConfig {
    match info.kind {
        SessionKind::LocalPty => SessionLaunchConfig::Local(LocalSessionConfig {
            name: info.name.clone(),
            shell_path: None,
            shell_args: Vec::new(),
            working_dir: info.working_dir.clone(),
            encoding: "UTF-8".to_string(),
            cols: info.cols,
            rows: info.rows,
            pixel_width: 0,
            pixel_height: 0,
        }),
        SessionKind::Ssh => SessionLaunchConfig::Ssh(Box::default()),
        SessionKind::Telnet | SessionKind::RawTcp => {
            SessionLaunchConfig::Telnet(TelnetSessionConfig {
                name: info.name.clone(),
                host: String::new(),
                port: 23,
                username: String::new(),
                password: None,
                backspace_mode: "del".to_string(),
                raw_tcp: info.kind == SessionKind::RawTcp,
                enter_mode: nyaterm_transport::TelnetEnterMode::default(),
                local_echo: false,
                local_line_edit: false,
                force_character_at_a_time: false,
                send_naws: false,
                send_sga: false,
                auto_login: nyaterm_transport::TelnetAutoLoginConfig::default(),
                encoding: "UTF-8".to_string(),
                cols: info.cols,
                rows: info.rows,
            })
        }
        SessionKind::Serial => SessionLaunchConfig::Serial(SerialSessionConfig {
            name: info.name.clone(),
            port_name: String::new(),
            baud_rate: 9600,
            data_bits: 8,
            parity: "none".to_string(),
            stop_bits: "1".to_string(),
            backspace_mode: "delete".to_string(),
            encoding: "UTF-8".to_string(),
        }),
        SessionKind::Rdp | SessionKind::Vnc => {
            unreachable!("remote desktop session metadata does not originate from SessionManager")
        }
    }
}
