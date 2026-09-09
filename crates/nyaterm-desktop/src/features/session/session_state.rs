use rust_i18n::t;

use std::borrow::Cow;

use gpui::{ClipboardItem, Context, Window};

use crate::features::NyaTermApp;
use crate::features::formatting::{session_kind_label, short_id};
use crate::models::{NavItem, SessionLaunchConfig};

impl NyaTermApp {
    fn active_session_info_line(&self) -> Option<String> {
        let session_id = self.session.active_id()?;
        let name = self.session.display_name(session_id)?;
        let session = self.session.session_info(session_id)?;
        let endpoint = self
            .session
            .endpoint(session_id)
            .unwrap_or_else(|| "unknown endpoint".to_string());
        Some(format!(
            "{} · {} · {} · {}x{} · {}",
            name,
            session_kind_label(session.kind),
            short_id(session_id),
            session.cols,
            session.rows,
            endpoint
        ))
    }

    pub(in crate::features) fn active_session_info_details(
        &self,
    ) -> Option<Vec<(Cow<'static, str>, String)>> {
        let session_id = self.session.active_id()?;
        let name = self.session.display_name(session_id)?;
        let session = self.session.session_info(session_id)?;
        let metadata = self.session.metadata(session_id)?;
        let endpoint = self
            .session
            .endpoint(session_id)
            .unwrap_or_else(|| "unknown endpoint".to_string());

        let mut details = vec![
            (t!("sessionInfo.name"), name),
            (
                t!("sessionInfo.kind"),
                session_kind_label(session.kind).to_string(),
            ),
            (t!("sessionInfo.sessionId"), session_id.to_string()),
            (
                t!("sessionInfo.size"),
                format!("{} x {}", session.cols, session.rows),
            ),
            (t!("sessionInfo.endpoint"), endpoint),
            (
                t!("sessionInfo.aiProfile"),
                format!("{:?}", metadata.ai_execution_profile),
            ),
        ];
        if let Some(cwd) = self.session.cwd(session_id) {
            details.push((t!("sessionInfo.cwd"), cwd.to_string()));
        }

        match &metadata.launch_config {
            SessionLaunchConfig::Local(config) => {
                details.push((
                    t!("sessionInfo.launch"),
                    t!("sessionInfo.localShell").to_string(),
                ));
                if let Some(shell) = config
                    .shell_path
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                {
                    details.push((t!("sessionInfo.shell"), shell.to_string()));
                }
                if let Some(dir) = config.working_dir.as_ref() {
                    details.push((t!("sessionInfo.workingDir"), dir.display().to_string()));
                }
            }
            SessionLaunchConfig::Ssh(config) => {
                details.push((t!("sessionInfo.launch"), t!("sessionInfo.ssh").to_string()));
                details.push((t!("sessionInfo.host"), config.host.clone()));
                details.push((t!("sessionInfo.port"), config.port.to_string()));
                details.push((t!("sessionInfo.username"), config.username.clone()));
                if let Some(address) = self.session.ssh_address(session_id) {
                    details.push((t!("sessionInfo.sshAddress"), address));
                }
                if let Some(proxy_jump) = config.proxy_jump.as_ref() {
                    details.push((
                        t!("sessionInfo.proxyJump"),
                        format!(
                            "{}@{}:{}",
                            proxy_jump.username, proxy_jump.host, proxy_jump.port
                        ),
                    ));
                }
            }
            SessionLaunchConfig::Telnet(config) => {
                details.push((
                    t!("sessionInfo.launch"),
                    t!("sessionInfo.telnet").to_string(),
                ));
                details.push((t!("sessionInfo.host"), config.host.clone()));
                details.push((t!("sessionInfo.port"), config.port.to_string()));
            }
            SessionLaunchConfig::Serial(config) => {
                details.push((
                    t!("sessionInfo.launch"),
                    t!("sessionInfo.serial").to_string(),
                ));
                details.push((t!("sessionInfo.port"), config.port_name.clone()));
                details.push((t!("sessionInfo.baud"), config.baud_rate.to_string()));
                details.push((
                    t!("sessionInfo.frame"),
                    format!("{}{}{}", config.data_bits, config.parity, config.stop_bits),
                ));
            }
            SessionLaunchConfig::Rdp(config) => {
                details.push((t!("sessionInfo.launch"), "RDP".to_string()));
                details.push((t!("sessionInfo.host"), config.host.clone()));
                details.push((t!("sessionInfo.port"), config.port.to_string()));
                details.push((t!("sessionInfo.username"), config.username.clone()));
            }
            SessionLaunchConfig::Vnc(config) => {
                details.push((t!("sessionInfo.launch"), "VNC".to_string()));
                details.push((t!("sessionInfo.host"), config.host.clone()));
                details.push((t!("sessionInfo.port"), config.port.to_string()));
            }
        }

        Some(details)
    }

    /// The canonical session-switch boundary.
    ///
    /// Takes `cx` so the switch and everything that must follow it land in one outer
    /// GPUI update transaction. That is what stops a future caller switching sessions
    /// while forgetting the Remote-panel resync: there is one place that knows, and it
    /// is the place every switch already goes through.
    pub(in crate::features) fn activate_session_id(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        self.session.start.clear_active_selection();
        self.shell.prepare_session_switch();
        // Session switch resets terminal-output credential autofill (Tauri XTerminal remount).
        self.terminal.reset_assist_for_session_switch();
        let previous_session_id = self.session.active_id_owned();
        let switching_sessions = previous_session_id.as_deref() != Some(session_id);
        let target_is_rdp = self.remote_desktop.is_session(session_id);
        if switching_sessions && let Some(previous_session_id) = previous_session_id.as_deref() {
            self.release_remote_keys(previous_session_id);
        }
        if switching_sessions
            && let Some(previous_session_id) = previous_session_id.as_deref()
            && !self.remote_desktop.is_session(previous_session_id)
        {
            self.clear_terminal_selection_state_for_session(previous_session_id);
        }
        if previous_session_id.as_deref() != Some(session_id)
            && let Some(previous_session_id) = previous_session_id.as_deref()
            && !self.remote_desktop.is_session(previous_session_id)
        {
            self.cache_transfer_browser_session(previous_session_id);
        }

        self.session.select_active_session(session_id);
        if switching_sessions {
            self.transfer.reset_transfer_queue_interaction();
            // Reset stays `cx`-free: it is pure remote state. The GPUI-facing resync is
            // the next line, so the ordering is reset -> resync -> (caller's surface
            // sync) -> paint, all inside this transaction.
            self.reset_remote_runtime_for_session_switch(session_id);
            self.sync_remote_panels_after_activation(cx);
        }
        // Keep workspace_split mirrored to the active tab's per-tab pane root.
        self.sync_workspace_split_from_active_tab();
        self.transfer.reset_browser_auto_sync_cwd();
        // Transfer browser state is only needed when the transfers panel is open
        // or we already have cached browser state for this session. Skipping the
        // full reset on every activate keeps connect/switch chrome responsive.
        let transfers_panel_visible = self.shell.active_left_panel() == Some(NavItem::Transfers)
            || self.shell.active_right_panel() == Some(NavItem::Transfers)
            || self.shell.selected_nav() == NavItem::Transfers;
        if transfers_panel_visible
            || self.transfer.has_browser_session_cache(session_id)
            || !self.transfer.browser_entries_are_empty()
        {
            self.sync_transfer_browser_favorites_for_active_session();
            if !self.restore_transfer_browser_session_cache(session_id) {
                self.reset_transfer_browser_for_active_session();
            }
        } else {
            // Keep favorites map coherent for the active connection without wiping UI.
            self.sync_transfer_browser_favorites_for_active_session();
        }
        let live_snapshot_missing = if target_is_rdp {
            false
        } else {
            self.terminal.activate_session_view(session_id)
        };
        if !target_is_rdp && switching_sessions && self.terminal.input_focus_is_active() {
            if let Some(previous_session_id) = previous_session_id.as_deref() {
                self.write_terminal_focus_report_to_session(previous_session_id, false);
            }
            self.write_terminal_focus_report_to_session(session_id, true);
        }
        self.sync_terminal_windows_active_tab(session_id);
        // Priority was refreshed via sync_workspace_split_from_active_tab.
        // Recover paint immediately if this tab was backgrounded without grids.
        if !target_is_rdp && live_snapshot_missing {
            self.request_terminal_live_snapshot(session_id);
        }
        // Transfer browser state is rendered by a separate entity, so an App notify
        // alone cannot publish session-switch changes to its snapshot.
        self.defer_transfer_panel_snapshot_flush(cx);
        previous_session_id
    }

    fn reset_remote_runtime_for_session_switch(&mut self, session_id: &str) {
        self.remote_ops.reset_for_session_switch();
        self.remote_ops.activate_stats_session(session_id);
    }

    pub(in crate::features) fn activate_session_id_with_surface_sync(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        let previous_session_id = self.activate_session_id(session_id, cx);
        self.load_transfer_browser_for_active_session_if_needed(cx);
        self.sync_terminal_activation_surfaces(previous_session_id, session_id, cx);
    }

    fn sync_terminal_activation_surfaces(
        &mut self,
        previous_session_id: Option<String>,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        if self.remote_desktop.is_session(session_id) {
            cx.notify();
            return;
        }
        let notify_session_ids =
            terminal_activation_surface_notify_ids(previous_session_id.as_deref(), session_id);
        if notify_session_ids.is_empty() {
            return;
        }
        let chrome_changed = self.clear_terminal_activation_interaction_state();
        for session_id in notify_session_ids {
            self.notify_terminal_surface_only(Some(session_id.as_str()), cx);
        }
        if chrome_changed {
            cx.notify();
        }
    }

    fn clear_terminal_activation_interaction_state(&mut self) -> bool {
        self.terminal.clear_activation_interaction()
    }

    pub(in crate::features) fn select_session(
        &mut self,
        session_id: String,
        cx: &mut Context<Self>,
    ) {
        // Local metadata is authoritative for tab existence; transport lock not needed.
        let known = self.session.has_session(&session_id);
        let disconnected = self.session.is_disconnected(&session_id);
        if !known && !disconnected {
            self.shell
                .set_status("session no longer exists".to_string());
            self.remove_session_state(&session_id, cx);
            cx.notify();
            return;
        }
        // Strip selection targets tab roots; focus the preferred leaf under that tab.
        let focus_id = if !self.is_secondary_pane_session(&session_id) {
            self.active_pane_for_tab_root(&session_id)
        } else {
            session_id.clone()
        };
        let disconnected = self.session.is_disconnected(&focus_id) || disconnected;
        self.activate_session_id_with_surface_sync(&focus_id, cx);
        self.shell.set_status(if disconnected {
            format!("disconnected {}", short_id(&focus_id))
        } else {
            format!("active {}", short_id(&focus_id))
        });
        self.shell.select_nav(NavItem::Workspace);
        cx.notify();
    }

    pub(in crate::features) fn select_relative_session(
        &mut self,
        offset: isize,
        cx: &mut Context<Self>,
    ) {
        let sessions = self.session.ordered_sessions();
        if sessions.is_empty() {
            self.shell.set_status("no sessions to switch".to_string());
            cx.notify();
            return;
        }
        let active_index = self
            .session
            .active_id()
            .and_then(|active_id| {
                sessions
                    .iter()
                    .position(|session| session.id.as_str() == active_id)
            })
            .unwrap_or(0);
        let len = sessions.len() as isize;
        let next_index = (active_index as isize + offset).rem_euclid(len) as usize;
        let session_id = sessions[next_index].id.clone();
        self.activate_session_id_with_surface_sync(&session_id, cx);
        self.shell.show_workspace();
        self.shell
            .set_status(format!("active {}", short_id(&session_id)));
        cx.notify();
    }

    pub(in crate::features) fn select_session_index(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        let sessions = self.session.ordered_sessions();
        if sessions.is_empty() {
            self.shell.set_status("no sessions to switch".to_string());
            cx.notify();
            return;
        }
        let index = index.min(sessions.len().saturating_sub(1));
        let session_id = sessions[index].id.clone();
        self.activate_session_id_with_surface_sync(&session_id, cx);
        self.shell.show_workspace();
        self.shell
            .set_status(format!("active {}", short_id(&session_id)));
        cx.notify();
    }

    pub(in crate::features) fn toggle_open_tabs_menu(&mut self, cx: &mut Context<Self>) {
        self.shell.toggle_open_tabs_menu();
        cx.notify();
    }

    pub(in crate::features) fn close_open_tabs_menu(&mut self, cx: &mut Context<Self>) {
        if self.shell.close_open_tabs_menu() {
            cx.notify();
        }
    }

    pub(in crate::features) fn open_new_session_menu(
        &mut self,
        anchor: crate::features::shell::NewSessionMenuAnchor,
        cx: &mut Context<Self>,
    ) {
        if self.shell.open_new_session_menu(anchor) {
            cx.notify();
        }
    }

    pub(in crate::features) fn close_new_session_menu(&mut self, cx: &mut Context<Self>) {
        if self.shell.close_new_session_menu() {
            cx.notify();
        }
    }

    pub(in crate::features) fn open_tab_actions_at(
        &mut self,
        session_id: String,
        anchor: Option<(f32, f32)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab_root = self.tab_root_for_session(&session_id);
        self.select_session(tab_root.clone(), cx);
        if !self
            .session
            .active_id()
            .is_some_and(|active| self.tab_root_for_session(active) == tab_root)
        {
            return;
        }
        self.session.dialogs.open_tab_actions(tab_root, anchor);
        self.shell.set_status("tab actions opened".to_string());
        window.focus(self.session.dialogs.tab_actions_focus(), cx);
        cx.notify();
    }

    pub(in crate::features) fn close_tab_actions(&mut self, cx: &mut Context<Self>) {
        self.session.dialogs.close_tab_actions();
        self.shell.set_status("tab actions closed".to_string());
        cx.notify();
    }

    pub(in crate::features) fn copy_session_name(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self.session.display_name(session_id) else {
            self.shell
                .set_status("active session name is unavailable".to_string());
            cx.notify();
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(name.clone()));
        self.shell.set_status(format!("copied tab name '{name}'"));
        cx.notify();
    }

    pub(in crate::features) fn copy_active_session_ssh_host(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.session.active_id() else {
            self.shell
                .set_status("no active SSH host to copy".to_string());
            cx.notify();
            return;
        };
        let Some(host) = self.session.ssh_host(session_id) else {
            self.shell
                .set_status("active session is not SSH".to_string());
            cx.notify();
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(host.clone()));
        self.shell.set_status(format!("copied SSH host '{host}'"));
        cx.notify();
    }

    pub(in crate::features) fn open_active_session_info(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session.active_id().is_none() {
            self.shell.set_status("no active session info".to_string());
            cx.notify();
            return;
        }
        self.session.dialogs.open_session_info();
        self.shell.set_status(
            self.active_session_info_line()
                .unwrap_or_else(|| "session info opened".to_string()),
        );
        window.focus(self.session.dialogs.session_info_focus(), cx);
        cx.notify();
    }

    pub(in crate::features) fn close_active_session_info(&mut self, cx: &mut Context<Self>) {
        self.session.dialogs.close_session_info();
        self.shell.set_status("session info closed".to_string());
        cx.notify();
    }

    pub(in crate::features) fn copy_active_session_info(&mut self, cx: &mut Context<Self>) {
        let Some(details) = self.active_session_info_details() else {
            self.shell
                .set_status("no active session info to copy".to_string());
            cx.notify();
            return;
        };
        let text = details
            .into_iter()
            .map(|(label, value)| format!("{label}: {value}"))
            .collect::<Vec<_>>()
            .join("\n");
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.shell.set_status("copied session info".to_string());
        cx.notify();
    }

    pub(in crate::features) fn close_tab_color_picker(&mut self, cx: &mut Context<Self>) {
        self.session.dialogs.close_color_picker();
        self.shell.set_status("tab color picker closed".to_string());
        cx.notify();
    }

    pub(in crate::features) fn set_active_session_tab_color(
        &mut self,
        color: Option<u32>,
        cx: &mut Context<Self>,
    ) {
        let Some(session_id) = self.session.active_id_owned() else {
            self.shell
                .set_status("no active session color to set".to_string());
            cx.notify();
            return;
        };
        let tab_root = self.tab_root_for_session(&session_id);
        self.session.set_tab_color(&tab_root, color);
        self.shell.set_status(if color.is_some() {
            "tab color updated".to_string()
        } else {
            "tab color reset".to_string()
        });
        self.session.dialogs.close_color_picker();
        self.persist_open_tabs();
        cx.notify();
    }

    pub(in crate::features) fn set_tab_tree_locked(
        &mut self,
        session_id: &str,
        locked: bool,
        cx: &mut Context<Self>,
    ) {
        let tab_root = self.tab_root_for_session(session_id);
        let mut ids = self.tab_tree_session_ids(&tab_root);
        if !ids.iter().any(|id| id == &tab_root) {
            ids.push(tab_root);
        }
        for id in ids {
            self.session.set_tab_locked(&id, locked);
        }
        self.shell.set_status(
            t!(if locked {
                "tabCtx.locked"
            } else {
                "tabCtx.unlockTab"
            })
            .to_string(),
        );
        self.persist_open_tabs();
        cx.notify();
    }

    pub(in crate::features) fn toggle_tab_tree_locked(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        let locked = !self.tab_tree_is_locked(session_id);
        self.set_tab_tree_locked(session_id, locked, cx);
    }

    pub(in crate::features) fn notify_locked_tab_close_blocked(&mut self, cx: &mut Context<Self>) {
        self.shell
            .set_status(t!("tabCtx.lockedCloseBlocked").to_string());
        cx.notify();
    }
}

fn terminal_activation_surface_notify_ids(
    previous_session_id: Option<&str>,
    session_id: &str,
) -> Vec<String> {
    if previous_session_id == Some(session_id) {
        return Vec::new();
    }
    let mut ids = Vec::with_capacity(2);
    if let Some(previous_session_id) = previous_session_id.filter(|id| !id.is_empty()) {
        ids.push(previous_session_id.to_string());
    }
    if !session_id.is_empty() {
        ids.push(session_id.to_string());
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::terminal_activation_surface_notify_ids;

    #[test]
    fn activation_surface_notify_skips_unchanged_session() {
        assert!(terminal_activation_surface_notify_ids(Some("a"), "a").is_empty());
    }

    #[test]
    fn activation_surface_notify_targets_previous_and_current_sessions() {
        assert_eq!(
            terminal_activation_surface_notify_ids(Some("old"), "new"),
            vec!["old".to_string(), "new".to_string()]
        );
    }

    #[test]
    fn activation_surface_notify_ignores_empty_session_ids() {
        assert_eq!(
            terminal_activation_surface_notify_ids(None, "new"),
            vec!["new".to_string()]
        );
        assert_eq!(
            terminal_activation_surface_notify_ids(Some("old"), ""),
            vec!["old".to_string()]
        );
    }
}
