use gpui::{Context, Window};
use nyaterm_core::{ConnectionType, uuid};
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::shell::tab_mouse::TabActionTarget;
use crate::models::SessionLaunchConfig;

use super::state::StartTabActionSnapshot;
use super::{PendingSessionStart, SavedConnectionStartOptions};

pub(in crate::features) struct StartTabActionContext {
    pub connecting: bool,
    pub can_restart: bool,
    pub ssh_host: Option<String>,
}

impl StartTabActionContext {
    pub(in crate::features) fn enabled(&self, action: &str) -> bool {
        match action {
            "rename_tab" | "copy_tab_name" | "close_tab" => true,
            "copy_server_ip" => self.ssh_host.is_some(),
            "duplicate_session" | "reconnect_session" => !self.connecting && self.can_restart,
            "disconnect_session" => self.connecting,
            _ => false,
        }
    }
}

impl NyaTermApp {
    pub(in crate::features) fn start_tab_action_context(
        &self,
        target: &TabActionTarget,
    ) -> Option<StartTabActionContext> {
        let (_, pending, ended) = self.session.start_action_request(target)?;
        let connection = pending.source_connection_id.as_ref().and_then(|id| {
            self.connection_state
                .connections()
                .iter()
                .find(|connection| &connection.id == id)
        });
        let source_busy = pending.source_connection_id.as_ref().is_some_and(|id| {
            self.session.start.source_connection_is_pending(id)
                || self.session.start.saved_connection_is_preparing(id)
        });
        let ssh_host = match pending.launch_config.as_ref() {
            Some(SessionLaunchConfig::Ssh(config)) => Some(config.host.clone()),
            _ => connection.and_then(|connection| match &connection.config {
                ConnectionType::Ssh { host, .. } => Some(host.clone()),
                _ => None,
            }),
        }
        .filter(|host| !host.trim().is_empty());
        Some(StartTabActionContext {
            connecting: ended.is_none(),
            can_restart: !source_busy
                && (connection.is_some()
                    || pending
                        .launch_config
                        .as_ref()
                        .is_some_and(|config| config.encoding().is_some())),
            ssh_host,
        })
    }

    pub(in crate::features) fn start_tab_action_snapshot(
        &self,
        target: &TabActionTarget,
    ) -> Option<StartTabActionSnapshot> {
        let (id, _, reason) = self.session.start_action_request(target)?;
        Some(StartTabActionSnapshot {
            request_id: id.to_string(),
            reason,
        })
    }

    pub(in crate::features) fn open_start_tab_actions_at(
        &mut self,
        target: TabActionTarget,
        anchor: (f32, f32),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = self.start_tab_action_snapshot(&target) else {
            return;
        };
        let restore_focus = window.focused(cx);
        self.select_tab_action_target(&target, window, cx);
        self.session
            .dialogs
            .open_start_tab_actions(target, snapshot, anchor, restore_focus);
        window.focus(self.session.dialogs.tab_actions_focus(), cx);
        cx.notify();
    }

    pub(in crate::features) fn run_start_tab_mouse_action(
        &mut self,
        target: TabActionTarget,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .start_tab_action_context(&target)
            .is_some_and(|context| context.enabled(action))
        {
            return;
        }
        let Some((request_id, _, ended)) = self.session.start_action_request(&target) else {
            return;
        };
        let request_id = request_id.to_string();
        match action {
            "close_tab" if ended.is_some() => self.close_failed_session_start(request_id, cx),
            "close_tab" => self.close_pending_session_start(request_id, cx),
            "disconnect_session" => {
                let message = t!("terminal.connectionCancelled").to_string();
                self.session
                    .start
                    .cancel_pending(&request_id, message.clone());
                self.shell.set_status(message);
                self.clear_session_tab_drag(cx);
                cx.notify();
            }
            "rename_tab" => self.open_rename_tab_target(target, window, cx),
            "copy_tab_name" => {
                if let Some(name) = self.session.tab_action_display_name(&target) {
                    self.copy_text_with_feedback("copy-session-name", name, cx);
                }
            }
            "copy_server_ip" => {
                if let Some(host) = self
                    .start_tab_action_context(&target)
                    .and_then(|context| context.ssh_host)
                {
                    self.copy_text_with_feedback("copy-session-ip", host, cx);
                }
            }
            "reconnect_session" => self.retry_failed_session_start(request_id, window, cx),
            "duplicate_session" => self.duplicate_start_tab(&target, window, cx),
            _ => {}
        }
    }

    fn duplicate_start_tab(
        &mut self,
        target: &TabActionTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, source, Some(_))) = self.session.start_action_request(target) else {
            return;
        };
        let connection = source
            .source_connection_id
            .as_ref()
            .and_then(|id| {
                self.connection_state
                    .connections()
                    .iter()
                    .find(|connection| &connection.id == id)
            })
            .cloned();
        let placement = source.tab_placement.expect("visible start has a placement");
        let group = self
            .shell
            .terminal_start_display_group(placement.request_sequence)
            .or_else(|| self.shell.first_terminal_group());
        let mut order = self
            .session_strip_tabs(&group)
            .iter()
            .map(|tab| tab.id.layout_key())
            .collect::<Vec<_>>();
        let source_key = crate::features::shell::session_tabs::SessionStripTabId::Start(
            placement.request_sequence,
        )
        .layout_key();
        let Some(index) = order
            .iter()
            .position(|key| key == &source_key)
            .map(|index| index + 1)
        else {
            return;
        };
        let mut pending = PendingSessionStart {
            requested_ssh_runtime_mode: source.requested_ssh_runtime_mode,
            attempt: Default::default(),
            connection_name: source.connection_name.clone(),
            launch_config: source.launch_config.clone(),
            requested_at: std::time::Instant::now(),
            kind: source.kind,
            ai_execution_profile: source.ai_execution_profile,
            custom_name: source.custom_name.clone(),
            tab_color: source.tab_color,
            locked: false,
            after_session_id: None,
            insert_index: Some(index),
            seed_output: None,
            startup_command: None,
            multiplex_key: None,
            source_connection_id: source.source_connection_id.clone(),
            workspace_split: None,
            tab_placement: None,
            reconnect_session_id: None,
        };
        let placement = self.session.start.allocate_tab_placement(index);
        pending.tab_placement = Some(placement);
        if let Some(group) = group.clone() {
            self.shell
                .reserve_terminal_start_group(placement.request_sequence, group);
        }
        let request_id = uuid();
        let options = SavedConnectionStartOptions {
            ssh_runtime_mode: pending.ssh_runtime_mode(),
            custom_name: pending.custom_name.clone(),
            tab_color: pending.tab_color,
            insert_index: Some(index),
            tab_placement: Some(placement),
            retry_request_id: Some(request_id.clone()),
            ..Default::default()
        };
        let launch_config = pending.launch_config.clone();
        let name = pending.connection_name.clone();
        let source_id = pending.source_connection_id.clone();
        let profile = pending.ai_execution_profile;
        self.session
            .start
            .begin_retry_preparation(request_id, pending);
        let new_key = crate::features::shell::session_tabs::SessionStripTabId::Start(
            placement.request_sequence,
        )
        .layout_key();
        order.insert(index, new_key.clone());
        self.commit_session_strip_order(&group, &order, &new_key, cx);
        if let Some(connection) = connection {
            self.start_saved_connection_with_options(connection, options, window, cx);
        } else if let Some(config) = launch_config {
            match config {
                SessionLaunchConfig::Ssh(config) => {
                    self.begin_background_ssh_start(name, *config, source_id, profile, options, cx)
                }
                config => self
                    .begin_background_session_start(name, config, source_id, profile, options, cx),
            }
            self.focus_user_started_session_surface(
                super::session_runtime::SessionStartFocusTarget::Terminal,
                window,
                cx,
            );
        }
        cx.notify();
    }
}
