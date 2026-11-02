use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use gpui::Context;
use nyaterm_core::{AiExecutionProfile, uuid};
use nyaterm_transport::{
    LocalSessionConfig, SerialSessionConfig, SessionInfo, SessionKind, SessionManager,
    SshSessionConfig, TelnetSessionConfig, open_ssh_multiplex_handle,
};

use crate::app_shell::session_hub::ssh_connections::SshConnectionPool;

use super::super::state::{
    PendingReconnectCompletion, failed_session_start_display_name,
    pending_session_start_display_name,
};
use super::{MultiplexSshStartRequest, PendingSessionStartRegistration};
use crate::features::formatting::{session_kind_label, short_id};
use crate::features::{
    NyaTermApp, runtime_jobs::SessionStartResult, runtime_jobs::SessionStartSuccess,
    runtime_jobs::submit_session_start_job, session::PendingSessionStart,
    session::SavedConnectionStartOptions, session::SessionStartEventRequest,
};
use crate::models::{NavItem, SessionLaunchConfig, SessionRuntimeMetadata};

impl NyaTermApp {
    pub(in crate::features) fn prepare_session_start_options(
        &mut self,
        mut options: SavedConnectionStartOptions,
    ) -> SavedConnectionStartOptions {
        if let Some(request_id) = options.retry_request_id.as_ref()
            && let Some((_, pending)) = self
                .session
                .start
                .pending_entries()
                .find(|(id, _)| *id == request_id)
        {
            options.custom_name = pending.custom_name.clone();
        }
        options.tab_placement = options
            .tab_placement
            .map(|p| self.session.start.resolve_placement(p));
        if options.reconnect_session_id.is_some() {
            return options;
        }
        if let Some(placement) = options.tab_placement {
            if self
                .shell
                .terminal_start_group(placement.request_sequence)
                .is_none()
            {
                self.ensure_terminal_windows_root();
                if let Some(group) = self.current_terminal_group() {
                    self.shell
                        .reserve_terminal_start_group(placement.request_sequence, group);
                }
            }
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
        let placement = self.session.start.allocate_tab_placement(insert_index);
        self.ensure_terminal_windows_root();
        if let Some(group) = self.current_terminal_group() {
            self.shell
                .reserve_terminal_start_group(placement.request_sequence, group);
        }
        options.tab_placement = Some(placement);
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
        let request_id = self
            .session
            .start
            .preparing_retry_request(registration.tab_placement)
            .map(str::to_string)
            .unwrap_or_else(uuid);
        let PendingSessionStartRegistration {
            requested_ssh_runtime_mode,
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
        self.session.start.finish_retry_preparation(&request_id);
        let requested_at = self
            .session
            .start
            .pending_entries()
            .find(|(id, _)| *id == &request_id)
            .map(|(_, pending)| pending.requested_at)
            .or_else(|| tab_placement.map(|placement| placement.requested_at))
            .unwrap_or_else(Instant::now);

        let reconnecting = self.session.start.register_pending(
            request_id.clone(),
            PendingSessionStart {
                requested_ssh_runtime_mode,
                attempt: Default::default(),
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
        // A pending start is the only state the "still connecting" status means
        // anything in, so its clock is armed here rather than polled for.
        self.ensure_pending_session_status_clock(cx);
        cx.notify();
        request_id
    }

    pub(in crate::features) fn begin_background_session_start(
        &mut self,
        connection_name: String,
        mut launch_config: SessionLaunchConfig,
        source_connection_id: Option<String>,
        ai_execution_profile: AiExecutionProfile,
        options: SavedConnectionStartOptions,
        cx: &mut Context<Self>,
    ) {
        if let Some(label) = launch_config.encoding()
            && let Err(error) = nyaterm_core::character_encoding::CharacterEncoding::parse(label)
        {
            let error = crate::features::terminal::encoding_error_text(error);
            self.fail_retry_preparation(&options, error.clone(), cx);
            self.shell.set_status(error);
            cx.notify();
            return;
        }

        if !self.session.start.retry_is_pending(&options) {
            return;
        }
        let options = self.prepare_session_start_options(options);
        let SavedConnectionStartOptions {
            ssh_runtime_mode: _,
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
            retry_request_id: _,
        } = options;
        let kind = session_kind_for_launch_config(&launch_config);
        let multiplex_key = matches!(launch_config, SessionLaunchConfig::Ssh(_))
            .then(|| format!("ssh-session:{}", uuid()));
        let request_id = self.register_pending_session_start(
            PendingSessionStartRegistration {
                requested_ssh_runtime_mode: None,
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
                multiplex_key: multiplex_key.clone(),
                source_connection_id,
                reconnect_session_id,
                workspace_split,
                tab_placement,
                status_message: format!("connecting to {connection_name}"),
                append_start_log: true,
            },
            cx,
        );

        let attempt = self.session.start.attempt(&request_id);
        if let SessionLaunchConfig::Ssh(config) = &mut launch_config {
            config.bind_attempt(attempt.clone());
        }
        let session_manager = self.session.manager_handle();
        let ssh_connections = self.session.ssh_connection_pool();
        let session_start_tx = self.session.start.sender();
        let request_id_for_worker = request_id.clone();
        submit_session_start_job(
            &self.blocking_jobs,
            "session-start",
            request_id_for_worker,
            connection_name,
            kind,
            session_start_tx,
            move || {
                attempt.check()?;
                let result = if let SessionLaunchConfig::Telnet(config) = &launch_config {
                    session_manager
                        .create_telnet_session_with_attempt(config.clone(), attempt.clone())
                        .map(|session_info| SessionStartSuccess {
                            session_info,
                            multiplex_handle: None,
                            cleanup: None,
                            launch_config: None,
                        })
                        .map_err(|error| error.to_string())
                } else {
                    create_session_from_launch_config(
                        &session_manager,
                        &ssh_connections,
                        multiplex_key,
                        launch_config.clone(),
                    )
                };
                result
                    .map(|success| SessionStartSuccess {
                        launch_config: Some(launch_config),
                        ..success
                    })
                    .map_err(|error| error.to_string())
            },
        );
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
        if !self.session.start.retry_is_pending(&options) {
            return;
        }
        config.runtime_mode = config.effective_runtime_mode();
        if let Some(mode) = options.ssh_runtime_mode {
            config.runtime_mode = mode;
        }
        let reload_saved = options
            .reconnect_session_id
            .as_ref()
            .and(source_connection_id.clone());
        let mut build_context = self.ssh_session_config_build_context();
        let mut options = options;
        if options.ssh_runtime_mode == Some(nyaterm_core::models::ssh_runtime::SshRuntimeMode::Sftp)
            || config.runtime_mode == nyaterm_core::models::ssh_runtime::SshRuntimeMode::Sftp
        {
            options.startup_command = None;
        }
        let options = self.prepare_session_start_options(options);
        let SavedConnectionStartOptions {
            ssh_runtime_mode,
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
            retry_request_id: _,
        } = options;
        config.deferred_pty = true;
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
        let multiplex_key = format!("ssh-session:{}", uuid());
        let request_id = self.register_pending_session_start(
            PendingSessionStartRegistration {
                requested_ssh_runtime_mode: None,
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
                status_message: format!("connecting to {connection_name}"),
                append_start_log: true,
            },
            cx,
        );

        config.bind_attempt(self.session.start.attempt(&request_id));
        build_context.attempt = config.attempt.clone();
        let session_manager = self.session.manager_handle();
        let ssh_connections = self.session.ssh_connection_pool();
        let session_start_tx = self.session.start.sender();
        let request_id_for_worker = request_id.clone();
        submit_session_start_job(
            &self.blocking_jobs,
            "ssh-session-start",
            request_id_for_worker,
            connection_name,
            SessionKind::Ssh,
            session_start_tx,
            move || {
                if let Some(id) = reload_saved {
                    let saved = build_context
                        .store
                        .request_fn(nyaterm_store::StoreDomain::Connections, move |store| {
                            store.get_connection(&id)
                        })
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| "saved SSH connection was deleted".to_string())?;
                    let mut fresh = build_context.build_config(&saved)?;
                    fresh.runtime_mode = config.runtime_mode;
                    fresh.cols = config.cols;
                    fresh.rows = config.rows;
                    fresh.pixel_width = config.pixel_width;
                    fresh.pixel_height = config.pixel_height;
                    fresh.bind_attempt(config.attempt.clone());
                    config = fresh;
                }
                if let Some(mode) = ssh_runtime_mode {
                    config.runtime_mode = mode;
                }
                config.apply_runtime_policy()?;
                let multiplex =
                    open_ssh_multiplex_handle(config.clone()).map_err(|error| error.to_string())?;
                let connection = ssh_connections.register(multiplex_key, multiplex);
                let session_info = session_manager
                    .create_ssh_session_with_multiplex(config.clone(), connection.handle())
                    .map_err(|error| error.to_string())?;
                Ok(SessionStartSuccess::ssh(
                    Arc::clone(&session_manager),
                    session_info,
                    connection,
                    Some(SessionLaunchConfig::Ssh(Box::new(config))),
                ))
            },
        );
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
            existing_multiplex_key,
        } = request;
        if !self.session.start.retry_is_pending(&options) {
            return;
        }
        config.runtime_mode = config.effective_runtime_mode();
        let mut options = options;
        if let Some(mode) = options.ssh_runtime_mode {
            config.runtime_mode = mode;
        }
        if config.runtime_mode == nyaterm_core::models::ssh_runtime::SshRuntimeMode::Sftp {
            options.startup_command = None;
        }
        let options = self.prepare_session_start_options(options);
        let SavedConnectionStartOptions {
            ssh_runtime_mode: _,
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
            retry_request_id: _,
        } = options;
        config.deferred_pty = true;
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
        let multiplex_key = existing_multiplex_key
            .filter(|_| existing_multiplex.is_some())
            .unwrap_or_else(|| format!("ssh-session:{}", uuid()));
        let request_id = self.register_pending_session_start(
            PendingSessionStartRegistration {
                requested_ssh_runtime_mode: None,
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

        config.bind_attempt(self.session.start.attempt(&request_id));
        let session_manager = self.session.manager_handle();
        let ssh_connections = self.session.ssh_connection_pool();
        let session_start_tx = self.session.start.sender();
        let request_id_for_worker = request_id.clone();
        submit_session_start_job(
            &self.blocking_jobs,
            "ssh-multiplex-session-start",
            request_id_for_worker,
            connection_name,
            SessionKind::Ssh,
            session_start_tx,
            move || {
                config.apply_runtime_policy()?;
                let connection = match existing_multiplex {
                    Some(connection)
                        if !connection.is_closed()
                            && connection.handle().matches_config(&config) =>
                    {
                        connection
                    }
                    _ => {
                        let handle = open_ssh_multiplex_handle(config.clone())
                            .map_err(|error| error.to_string())?;
                        ssh_connections.register(multiplex_key, handle)
                    }
                };
                let session_info = session_manager
                    .create_ssh_session_with_multiplex(config.clone(), connection.handle())
                    .map_err(|error| error.to_string())?;
                Ok(SessionStartSuccess::ssh(
                    Arc::clone(&session_manager),
                    session_info,
                    connection,
                    Some(SessionLaunchConfig::Ssh(Box::new(config))),
                ))
            },
        );
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
                // `update_in`: pumping the restore queue opens sessions, which needs
                // the window.
                if this
                    .update_in(cx, |this, window, cx| {
                        this.apply_session_start_event(event, cx);
                        // A start finishing is precisely when the next queued restore
                        // may go; the idle plane used to poll for that.
                        this.pump_startup_restore_queue_if_ready(window, cx);
                        // A start settling is also when the layout restore and any
                        // auto-recording become eligible -- though both still have to
                        // wait out connect settle, which the clock handles.
                        this.ensure_post_start_work_clock(cx);
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

    pub(in crate::features::session) fn apply_session_start_event(
        &mut self,
        event: SessionStartResult,
        cx: &mut Context<Self>,
    ) {
        let request_id = event.request_id.clone();
        let (mut pending, was_active_pending) =
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
            Ok(mut success) => {
                let ui_register_started_at = Instant::now();
                self.shell.clear_last_connect_failure();
                let session_id = success.session_info.id.clone();
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
                success.accept();
                let session_info = success.session_info;
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
                if let Some(connection) = success.multiplex_handle {
                    self.session
                        .register_ssh_connection(session_id.clone(), connection);
                }
                let source_connection_id = pending
                    .as_ref()
                    .and_then(|pending| pending.source_connection_id.clone());
                let tab_placement = pending
                    .as_ref()
                    .and_then(|pending| pending.tab_placement)
                    .map(|p| self.session.start.resolve_placement(p));
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
                let ai_execution_profile = if ssh_config.as_ref().is_some_and(|config| {
                    config.effective_runtime_mode()
                        == nyaterm_core::models::ssh_runtime::SshRuntimeMode::Sftp
                }) {
                    AiExecutionProfile::Disabled
                } else {
                    ai_execution_profile
                };
                if ssh_config.as_ref().is_some_and(|config| {
                    config.effective_runtime_mode()
                        == nyaterm_core::models::ssh_runtime::SshRuntimeMode::Sftp
                }) && let Some(pending) = pending.as_mut()
                {
                    pending.startup_command = None;
                }
                let metadata = SessionRuntimeMetadata {
                    ssh_config,
                    ssh_multiplex_key,
                    source_connection_id: source_connection_id.clone(),
                    ai_execution_profile,
                    launch_config,
                    disconnected: false,
                };
                let rekey_queued = if let Some(old_id) = reconnect_session_id.as_deref()
                    && old_id != session_id
                {
                    let encoding = metadata
                        .launch_config
                        .encoding()
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| {
                            self.settings.summary().interaction_default_encoding.clone()
                        });
                    self.session
                        .start
                        .record_reconnect_completion(PendingReconnectCompletion {
                            old_id: old_id.to_string(),
                            new_id: session_id.clone(),
                            connection_name: connection_name.clone(),
                            kind,
                            source_connection_id: source_connection_id.clone(),
                            workspace_split: workspace_split.clone(),
                            startup_command: pending
                                .as_ref()
                                .and_then(|pending| pending.startup_command.clone()),
                        });
                    self.terminal.request_session_rekey(
                        old_id,
                        &session_id,
                        &encoding,
                        self.terminal_scrollback_line_limit(),
                    )
                } else {
                    true
                };
                if let Some(old_id) = reconnect_session_id.as_deref() {
                    self.recording.rekey_session(old_id, &session_id);
                    self.register_session_for_reconnect(&session_id, metadata);
                } else {
                    self.register_session_for_start(
                        &session_id,
                        metadata,
                        tab_placement,
                        fallback_insert_index,
                    );
                }
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
                    .filter(|_| reconnect_session_id.is_none())
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
                if let Some(old_id) = reconnect_session_id.as_deref()
                    && old_id != session_id
                {
                    if !rekey_queued {
                        self.complete_reconnected_session_frame(old_id, &session_id, false, cx);
                    }
                    self.settle_session_start_tab_placements_if_idle();
                    return;
                }
                if let Some(connection_id) = source_connection_id.as_deref() {
                    self.complete_mcp_session_open_success(connection_id, session_id.clone());
                    if kind == nyaterm_transport::SessionKind::Ssh {
                        self.start_auto_tunnels_for_connection(connection_id, cx);
                    }
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
                let should_activate = self
                    .session
                    .start
                    .complete_success(was_active_pending, self.session.active_id().is_none());
                if should_activate {
                    self.activate_session_id(&session_id, cx);
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
                self.apply_workspace_split_for_duplicate(cx, workspace_split, &session_id);
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
                let source_connection_id = pending
                    .as_ref()
                    .and_then(|pending| pending.source_connection_id.clone());
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
                if let Some(connection_id) = source_connection_id.as_deref() {
                    self.complete_mcp_session_open_failure(connection_id, &error);
                }
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

    pub(in crate::features) fn complete_reconnected_session_frame(
        &mut self,
        old_id: &str,
        new_id: &str,
        success: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(completion) = self.session.start.take_reconnect_completion(new_id) else {
            return false;
        };
        if completion.old_id != old_id
            || !success
            || !self.session.has_session(old_id)
            || !self.session.has_session(new_id)
        {
            let error = "terminal history could not be transferred".to_string();
            let _ = self.session.manager().close(new_id);
            if self.session.has_session(new_id) {
                self.remove_session_state(new_id, cx);
            }
            if self.session.has_session(old_id) {
                self.session
                    .start
                    .set_reconnect_failure(old_id.to_string(), error.clone());
            }
            if let Some(connection_id) = completion.source_connection_id.as_deref() {
                self.complete_mcp_session_open_failure(connection_id, &error);
            }
            self.shell.set_status(format!("reconnect failed: {error}"));
            cx.notify();
            return true;
        }

        self.migrate_reconnected_session_state(old_id, new_id, cx);
        self.remove_session_state(old_id, cx);
        self.persist_workspace_pane_layout();
        self.persist_terminal_window_layout();
        if let Some(connection_id) = completion.source_connection_id.as_deref() {
            self.complete_mcp_session_open_success(connection_id, new_id.to_string());
            if completion.kind == SessionKind::Ssh {
                self.start_auto_tunnels_for_connection(connection_id, cx);
            }
        }
        if self
            .session
            .start
            .complete_success(false, self.session.active_id().is_none())
        {
            self.activate_session_id(new_id, cx);
            self.load_transfer_browser_for_active_session_if_needed(cx);
        }
        self.enter_connect_settle();
        self.terminal.enter_session_render_degraded(new_id);
        self.shell.set_status(format!(
            "running {} · {}",
            short_id(new_id),
            completion.connection_name
        ));
        self.apply_workspace_split_for_duplicate(cx, completion.workspace_split, new_id);
        if let Some(startup_command) = completion.startup_command {
            self.schedule_startup_command(new_id.to_string(), startup_command, cx);
        }
        self.shell.select_nav(NavItem::Workspace);
        cx.notify();
        true
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
    session_manager: &Arc<SessionManager>,
    ssh_connections: &SshConnectionPool,
    multiplex_key: Option<String>,
    launch_config: SessionLaunchConfig,
) -> Result<SessionStartSuccess, String> {
    match launch_config {
        SessionLaunchConfig::Local(config) => session_manager
            .create_local_session(config)
            .map(|session_info| SessionStartSuccess {
                session_info,
                multiplex_handle: None,
                cleanup: None,
                launch_config: None,
            })
            .map_err(|error| error.to_string()),
        SessionLaunchConfig::Ssh(config) => {
            let config = *config;
            let multiplex =
                open_ssh_multiplex_handle(config.clone()).map_err(|error| error.to_string())?;
            let connection =
                ssh_connections.register(multiplex_key.expect("SSH connection key"), multiplex);
            let session_info = session_manager
                .create_ssh_session_with_multiplex(config, connection.handle())
                .map_err(|error| error.to_string())?;
            Ok(SessionStartSuccess::ssh(
                Arc::clone(session_manager),
                session_info,
                connection,
                None,
            ))
        }
        SessionLaunchConfig::Telnet(config) => session_manager
            .create_telnet_session(config)
            .map(|session_info| SessionStartSuccess {
                session_info,
                multiplex_handle: None,
                cleanup: None,
                launch_config: None,
            })
            .map_err(|error| error.to_string()),
        SessionLaunchConfig::Serial(config) => session_manager
            .create_serial_session(config)
            .map(|session_info| SessionStartSuccess {
                session_info,
                multiplex_handle: None,
                cleanup: None,
                launch_config: None,
            })
            .map_err(|error| error.to_string()),
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
                keep_alive_interval_secs:
                    nyaterm_core::models::connection::DEFAULT_TELNET_KEEP_ALIVE_INTERVAL_SECS,
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
            flow_control: Default::default(),
            stop_bits: "1".to_string(),
            backspace_mode: "delete".to_string(),
            encoding: "UTF-8".to_string(),
        }),
        SessionKind::Rdp | SessionKind::Vnc => {
            unreachable!("remote desktop session metadata does not originate from SessionManager")
        }
    }
}

#[cfg(test)]
mod focus_tests {
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::time::Instant;

    use gpui::TestAppContext;
    use nyaterm_core::AiExecutionProfile;
    use nyaterm_transport::{SessionInfo, SessionKind};

    use super::super::PendingSessionStartRegistration;
    use crate::features::runtime_jobs::{SessionStartResult, SessionStartSuccess};
    use crate::features::test_support::{
        app_with_visible_local_session, session_focus_test_window,
    };
    use crate::models::{SessionLaunchConfig, TransferBrowserSessionCacheState};
    use crate::test_support::TestConfigDir;

    #[test]
    fn background_ssh_completion_preserves_input_focus_and_user_tab_selection() {
        for switch_tab in [false, true] {
            let root = TestConfigDir::new("nyaterm-background-ssh-focus");
            let mut cx = TestAppContext::single();
            let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
            let app = app_with_visible_local_session(&mut cx, root.path(), "source");
            let cx = session_focus_test_window(&mut cx, &app);
            cx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    // The completion is synthetic: retain a browser cache so
                    // activation cannot start file operations against a fake SSH peer.
                    app.transfer.store_browser_session_cache(
                        "new-ssh".into(),
                        TransferBrowserSessionCacheState {
                            entries: Arc::new(Vec::new()),
                            current_path: "/".into(),
                            current_raw_path_token: None,
                            home_dir: "/".into(),
                            history: VecDeque::from(["/".into()]),
                            history_index: 0,
                            visited_history: VecDeque::new(),
                        },
                    );
                    let request_id = app.register_pending_session_start(
                        PendingSessionStartRegistration {
                            requested_ssh_runtime_mode: None,
                            connection_name: "SSH".into(),
                            launch_config: Some(SessionLaunchConfig::Ssh(Box::default())),
                            kind: SessionKind::Ssh,
                            ai_execution_profile: AiExecutionProfile::Posix,
                            custom_name: None,
                            tab_color: None,
                            locked: false,
                            after_session_id: Some("source".into()),
                            insert_index: None,
                            seed_output: None,
                            startup_command: None,
                            multiplex_key: None,
                            source_connection_id: None,
                            reconnect_session_id: None,
                            workspace_split: None,
                            tab_placement: None,
                            status_message: "connecting".into(),
                            append_start_log: false,
                        },
                        cx,
                    );
                    if switch_tab {
                        app.activate_session_id("source", cx);
                    }
                    window.focus(app.session.prompt_credential_focus(), cx);
                    let now = Instant::now();
                    // Drive the real completion callback without a network connection.
                    app.apply_session_start_event(
                        SessionStartResult {
                            request_id,
                            connection_name: "SSH".into(),
                            kind: SessionKind::Ssh,
                            worker_started_at: now,
                            worker_finished_at: now,
                            result: Ok(SessionStartSuccess {
                                session_info: SessionInfo {
                                    id: "new-ssh".into(),
                                    name: "SSH".into(),
                                    kind: SessionKind::Ssh,
                                    working_dir: None,
                                    cols: 80,
                                    rows: 24,
                                },
                                multiplex_handle: None,
                                launch_config: Some(SessionLaunchConfig::Ssh(Box::default())),
                                cleanup: None,
                            }),
                        },
                        cx,
                    );
                });
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let app = app.read(cx);
                assert!(app.session.prompt_credential_focus().is_focused(window));
                assert_eq!(
                    app.session.active_id(),
                    Some(if switch_tab { "source" } else { "new-ssh" }),
                );
            });
        }
    }
}
