use std::time::Instant;

use gpui::{Context, Window};
use nyaterm_core::uuid;
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
        if failed
            .pending
            .source_connection_id
            .as_ref()
            .is_some_and(|connection_id| {
                self.session
                    .start
                    .source_connection_is_pending(connection_id)
                    || self
                        .session
                        .start
                        .saved_connection_is_preparing(connection_id)
            })
        {
            self.session
                .start
                .set_failed_error(&request_id, t!("tabCtx.reconnecting").to_string());
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
            retry_request_id: Some(new_id.clone()),
        };
        pending.attempt = Default::default();
        pending.requested_at = Instant::now();
        pending.reconnect_session_id = None;
        self.session.start.begin_retry_preparation(new_id, pending);
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

    pub(in crate::features::session) fn fail_retry_preparation(
        &mut self,
        options: &SavedConnectionStartOptions,
        error: String,
        cx: &mut Context<Self>,
    ) {
        let Some(request_id) = options.retry_request_id.as_ref() else {
            return;
        };
        if !self.session.start.retry_is_pending(options) {
            return;
        }
        if let SessionStartEventRequest::Pending {
            pending,
            was_active,
        } = self.session.start.take_event_request(request_id)
        {
            self.session.start.record_failure(
                request_id.clone(),
                pending,
                error,
                was_active,
                false,
            );
        }
        cx.notify();
    }
}
