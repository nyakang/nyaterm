use std::time::Instant;

use gpui::{Context, Window};
use nyaterm_core::{ConnectionType, uuid};
use nyaterm_transport::SessionKind;
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::session::{SavedConnectionStartOptions, SessionStartEventRequest};
use crate::models::SessionLaunchConfig;

impl NyaTermApp {
    pub(in crate::features) fn retry_failed_session_start(
        &mut self,
        request_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(failed) = self.session.start.failed(&request_id) else {
            return;
        };
        let remote = failed.pending.kind != SessionKind::LocalPty;
        if remote && self.remote_start_blocked_by_configuration_replacement(cx) {
            return;
        }
        let Some(failed) = self.session.start.failed(&request_id) else {
            return;
        };
        let connection = failed
            .pending
            .source_connection_id
            .as_ref()
            .and_then(|id| {
                self.connection_state
                    .connections()
                    .iter()
                    .find(|connection| &connection.id == id)
            })
            .cloned();
        if failed.pending.source_connection_id.is_some() && connection.is_none() {
            self.session.start.set_failed_error(
                &request_id,
                t!("terminal.savedConnectionDeleted").to_string(),
            );
            cx.notify();
            return;
        }
        let same_protocol = connection.as_ref().is_none_or(|connection| {
            matches!(
                (&connection.config, failed.pending.kind),
                (ConnectionType::LocalTerminal { .. }, SessionKind::LocalPty)
                    | (ConnectionType::Ssh { .. }, SessionKind::Ssh)
                    | (
                        ConnectionType::Telnet { .. },
                        SessionKind::Telnet | SessionKind::RawTcp
                    )
                    | (ConnectionType::Serial { .. }, SessionKind::Serial)
                    | (ConnectionType::Rdp { .. }, SessionKind::Rdp)
                    | (ConnectionType::Vnc { .. }, SessionKind::Vnc)
                    | (
                        ConnectionType::Adb { .. },
                        SessionKind::AdbShell | SessionKind::AdbFiles | SessionKind::AdbScreen
                    )
            )
        });
        if !same_protocol {
            self.session.start.set_failed_error(
                &request_id,
                t!("terminal.savedConnectionProtocolChanged").to_string(),
            );
            cx.notify();
            return;
        }
        if connection.is_none()
            && !failed
                .pending
                .launch_config
                .as_ref()
                .is_some_and(|config| config.encoding().is_some())
        {
            self.session
                .start
                .set_failed_error(&request_id, t!("terminal.retryUnavailable").to_string());
            cx.notify();
            return;
        }
        let Some(failed) = self.session.start.take_failed_for_retry(&request_id) else {
            return;
        };
        let mut pending = failed.pending;
        let launch_config = pending.launch_config.clone();
        let connection_name = pending.connection_name.clone();
        let source_connection_id = pending.source_connection_id.clone();
        let profile = pending.ai_execution_profile;
        let new_id = uuid();
        let options = SavedConnectionStartOptions {
            android_view: pending.android_view_kind(),
            mcp_open_operation_id: None,
            ssh_runtime_mode: pending.ssh_runtime_mode(),
            custom_name: pending.custom_name.clone(),
            tab_color: pending.tab_color,
            locked: pending.locked,
            after_session_id: pending.after_session_id.clone(),
            insert_index: pending.insert_index,
            seed_output: pending.seed_output.clone(),
            startup_command: pending.startup_command.clone(),
            reconnect_session_id: None,
            workspace_split: pending.workspace_split.clone(),
            tab_placement: pending.tab_placement,
            request_id: Some(new_id.clone()),
        };
        pending.attempt = Default::default();
        pending.requested_at = Instant::now();
        pending.reconnect_session_id = None;
        self.session.start.begin_preparation(new_id, pending);
        self.shell.clear_last_connect_failure();
        self.clear_session_tab_drag(cx);
        if let Some(connection) = connection {
            self.start_saved_connection_with_options(connection, options, window, cx);
        } else if let Some(config) = launch_config {
            match config {
                SessionLaunchConfig::Ssh(config) => self.begin_background_ssh_start(
                    connection_name,
                    *config,
                    source_connection_id,
                    profile,
                    options,
                    cx,
                ),
                config => self.begin_background_session_start(
                    connection_name,
                    config,
                    source_connection_id,
                    profile,
                    options,
                    cx,
                ),
            }
            self.focus_user_started_session_surface(
                super::SessionStartFocusTarget::Terminal,
                window,
                cx,
            );
        }
        cx.notify();
    }

    pub(in crate::features) fn fail_start_preparation(
        &mut self,
        options: &SavedConnectionStartOptions,
        error: String,
        cx: &mut Context<Self>,
    ) {
        let Some(request_id) = options.request_id.as_ref() else {
            return;
        };
        if !self.session.start.request_can_start(options) {
            return;
        }
        if let SessionStartEventRequest::Pending {
            pending,
            was_active,
        } = self.session.start.take_event_request(request_id)
        {
            let reconnect_exists = pending
                .as_ref()
                .and_then(|pending| pending.reconnect_session_id.as_deref())
                .is_some_and(|id| self.session.has_session(id));
            self.session.start.record_failure(
                request_id.clone(),
                pending,
                error,
                was_active,
                reconnect_exists,
            );
        }
        self.wake_startup_restore_after_preparation(cx);
        cx.notify();
    }
}
