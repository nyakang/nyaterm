use gpui::{Context, div, prelude::*};

use super::super::terminal_action_prompt_text;
use super::{
    CompactTabActionsMenuState, TabActionPolicy, TabActionPolicyInput, TabSessionCapability,
};
use crate::features::NyaTermApp;
use crate::features::formatting::recent_terminal_output;

impl NyaTermApp {
    fn tab_action_session_capability(&self, session_id: &str) -> Option<TabSessionCapability> {
        self.session
            .metadata(session_id)
            .map(|metadata| TabSessionCapability::from_launch_config(&metadata.launch_config))
    }

    pub(in crate::features) fn tab_action_can_spawn_session(&self, session_id: &str) -> bool {
        self.tab_action_session_capability(session_id)
            .is_some_and(TabSessionCapability::supports_terminal_actions)
    }

    pub(super) fn tab_action_source_connection_id(&self, session_id: &str) -> Option<&str> {
        self.session
            .metadata(session_id)
            .and_then(|metadata| metadata.source_connection_id.as_deref())
            .map(str::trim)
            .filter(|id| !id.is_empty())
    }

    pub(super) fn tab_action_can_show_session_info(&self, session_id: &str) -> bool {
        self.tab_action_source_connection_id(session_id).is_some()
    }

    pub(in crate::features) fn tab_action_policy_for(
        &self,
        session_id: &str,
        tab_root_id: &str,
    ) -> Option<TabActionPolicy> {
        let session = self.tab_action_session_capability(session_id)?;
        let is_busy = self.session.session_is_busy(session_id);
        let is_disconnected = self.session.is_disconnected(session_id);
        let tab_sessions = self.ordered_tab_sessions();
        let tab_index = tab_sessions.iter().position(|tab| tab.id == tab_root_id);
        let terminal_available = session.supports_terminal_actions()
            && self.terminal.session_output(session_id).is_some();

        Some(TabActionPolicy::from_input(TabActionPolicyInput {
            session,
            has_copyable_ssh_host: self.session.ssh_host(session_id).is_some(),
            has_source_connection: self.tab_action_can_show_session_info(session_id),
            is_busy,
            is_disconnected,
            reconnect_pending: self.session.start_reconnect_is_pending(session_id),
            // Session registration creates a terminal frame for launch configs with an
            // encoding. Check the in-memory view so a transiently unavailable terminal keeps
            // the AI submenu visible while its actions remain disabled.
            terminal_available,
            rdp_secure_attention_available: self.rdp_secure_attention_available(session_id),
            locked: self.tab_tree_is_locked(tab_root_id),
            tab_count: tab_sessions.len(),
            tab_index,
        }))
    }

    pub(in crate::features) fn tab_actions_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if let Some(target @ crate::features::shell::tab_mouse::TabActionTarget::Start { .. }) =
            self.session.dialog_tab_actions_target().cloned()
        {
            return self.start_tab_actions_menu(target, cx);
        }
        let palette = self.theme_palette();
        let Some(tab_root_id) = self
            .session
            .dialog_tab_actions_session_id()
            .map(str::to_string)
        else {
            return div().into_any_element();
        };
        let sessions = self.session.ordered_sessions();
        if !sessions.iter().any(|session| session.id == tab_root_id) {
            self.session.dialog_close_tab_actions();
            return div().into_any_element();
        }

        let session_id = self.active_pane_for_tab_root(&tab_root_id);
        if !sessions.iter().any(|session| session.id == session_id) {
            self.session.dialog_close_tab_actions();
            return div().into_any_element();
        }
        let Some(policy) = self.tab_action_policy_for(&session_id, &tab_root_id) else {
            self.session.dialog_close_tab_actions();
            return div().into_any_element();
        };
        let active_color = self.session.tab_color(&tab_root_id);
        let locked = self.tab_tree_is_locked(&tab_root_id);

        let (visible_for_ai, buffer_for_ai) = if policy.availability.use_ai {
            let scroll_offset = self.terminal.session_scroll_offset(&session_id);
            let visible = terminal_action_prompt_text(
                &self
                    .terminal_snapshot_for_session(Some(session_id.as_str()), scroll_offset)
                    .rows()
                    .iter()
                    .map(|row| row.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
                2_800,
            );
            let buffer = terminal_action_prompt_text(
                &recent_terminal_output(
                    self.terminal_buffer_tail_for_session(&session_id),
                    usize::MAX,
                ),
                4_000,
            );
            (visible, buffer)
        } else {
            (String::new(), String::new())
        };

        self.compact_tab_actions_menu(
            palette,
            CompactTabActionsMenuState {
                session_id,
                tab_root_id,
                active_color,
                locked,
                policy,
                visible_for_ai,
                buffer_for_ai,
            },
            cx,
        )
    }
}
