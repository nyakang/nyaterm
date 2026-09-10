use rust_i18n::t;

use std::time::Duration;

use gpui::{Context, Window};

use crate::features::formatting::{normalize_startup_command, short_id};
use crate::features::{AppLifecycleEvent, NyaTermApp};
use crate::models::StartupCommandRequest;
use crate::terminal::INITIAL_TERMINAL_BANNER;
use crate::terminal::initial_terminal_screen;

impl NyaTermApp {
    pub(in crate::features) fn schedule_startup_command(
        &mut self,
        session_id: String,
        startup_command: StartupCommandRequest,
        cx: &mut Context<Self>,
    ) {
        let command = normalize_startup_command(&startup_command.command);
        if command.trim().is_empty() {
            return;
        }
        let delay_ms = startup_command.delay_ms.min(60_000);
        self.shell.set_status(format!(
            "scheduled startup command for {}",
            short_id(&session_id)
        ));
        cx.spawn(async move |this, cx| {
            if delay_ms > 0 {
                cx.background_executor()
                    .timer(Duration::from_millis(delay_ms))
                    .await;
            }
            let _ = this.update(cx, |this, cx| {
                if this.send_terminal_input_to_session(session_id, command.into_bytes(), cx) {
                    this.shell.set_status("startup command sent".to_string());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(in crate::features) fn close_active_session(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.session.active_id_owned() else {
            self.shell.set_status("no active session".to_string());
            cx.notify();
            return;
        };
        self.close_session(self.tab_root_for_session(&session_id), cx);
    }

    pub(in crate::features) fn close_session(
        &mut self,
        session_id: String,
        cx: &mut Context<Self>,
    ) {
        if self.tab_tree_is_locked(&session_id) {
            self.notify_locked_tab_close_blocked(cx);
            return;
        }
        // Tauri: closing a strip tab closes the whole tab tree; closing a secondary leaf
        // only removes that pane. Strip close uses the tab-root id.
        let close_ids = if !self.is_secondary_pane_session(&session_id) {
            if let Some(root) = self.shell.workspace_pane_root(&session_id) {
                root.session_ids()
            } else {
                vec![session_id.clone()]
            }
        } else {
            vec![session_id.clone()]
        };
        let was_active = self
            .session
            .active_id()
            .is_some_and(|active_id| close_ids.iter().any(|id| id == active_id));
        for close_id in &close_ids {
            let disconnected = self.session.is_disconnected(close_id);
            let close_result = if self.remote_desktop.is_session(close_id) {
                self.close_remote_desktop_runtime(close_id)
            } else {
                self.session
                    .manager()
                    .close(close_id)
                    .map_err(anyhow::Error::from)
            };
            match close_result {
                Ok(()) => {}
                Err(_) if disconnected => {}
                Err(error) if !disconnected && !self.session.has_session(close_id) => {
                    self.shell.set_status(format!("close failed: {error}"));
                    cx.notify();
                    return;
                }
                Err(_) => {}
            }
            self.cleanup_recording_for_session(close_id);
            self.remove_session_state(close_id, cx);
        }
        self.prune_workspace_split();
        if was_active {
            self.ai.reset_agent_runtime();
            self.sync_session_event_bridge_policy();
            if let Some(next_session_id) = self.session.next_session_after(&session_id) {
                self.activate_session_id(&next_session_id, cx);
                self.shell.set_status(format!(
                    "session closed; active {}",
                    short_id(&next_session_id)
                ));
            } else {
                self.session.clear_active_session();
                self.terminal.view.output = String::from(INITIAL_TERMINAL_BANNER);
                self.terminal.view.output_decoder.reset_decoder();
                self.terminal.view.screen = initial_terminal_screen();
                self.terminal
                    .view
                    .screen
                    .set_encoding(&self.settings.summary().interaction_default_encoding);
                self.shell.set_status("session closed".to_string());
            }
        } else {
            self.shell
                .set_status(format!("closed {}", short_id(&session_id)));
        }
        cx.notify();
    }

    pub(in crate::features) fn close_tab_active_pane(
        &mut self,
        tab_root_id: &str,
        cx: &mut Context<Self>,
    ) {
        if self.tab_tree_is_locked(tab_root_id) {
            self.notify_locked_tab_close_blocked(cx);
            return;
        }
        let pane_id = self.active_pane_for_tab_root(tab_root_id);
        if self.tab_tree_session_ids(tab_root_id).len() <= 1 {
            self.close_session(tab_root_id.to_string(), cx);
            return;
        }

        if pane_id == tab_root_id
            && let Some(survivor_id) = self
                .tab_tree_session_ids(tab_root_id)
                .into_iter()
                .find(|id| id != &pane_id)
        {
            self.session
                .migrate_tab_root_presentation(tab_root_id, &survivor_id);
            self.shell
                .rekey_workspace_pane_root(tab_root_id, survivor_id);
        }

        let was_active = self.session.active_id() == Some(pane_id.as_str());
        let disconnected = self.session.is_disconnected(&pane_id);
        let close_result = if self.remote_desktop.is_session(&pane_id) {
            self.close_remote_desktop_runtime(&pane_id)
        } else {
            self.session
                .manager()
                .close(&pane_id)
                .map_err(anyhow::Error::from)
        };
        match close_result {
            Ok(()) => {}
            Err(_) if disconnected => {}
            Err(error) if !disconnected && !self.session.has_session(&pane_id) => {
                self.shell.set_status(format!("close failed: {error}"));
                cx.notify();
                return;
            }
            Err(_) => {}
        }
        self.cleanup_recording_for_session(&pane_id);
        self.remove_session_state(&pane_id, cx);
        self.prune_workspace_split();
        if was_active {
            self.ai.reset_agent_runtime();
            self.sync_session_event_bridge_policy();
            let next = self
                .tab_tree_session_ids(tab_root_id)
                .into_iter()
                .find(|id| self.session.has_session(id))
                .or_else(|| self.session.next_session_after(&pane_id));
            if let Some(next_session_id) = next {
                self.activate_session_id(&next_session_id, cx);
            } else {
                self.session.clear_active_session();
            }
        }
        self.shell
            .set_status(format!("closed {}", short_id(&pane_id)));
        cx.notify();
    }

    pub(in crate::features) fn close_session_batch(
        &mut self,
        session_ids: Vec<String>,
        label: &'static str,
        cx: &mut Context<Self>,
    ) {
        if session_ids.is_empty() {
            self.shell
                .set_status(format!("no {label} sessions to close"));
            return;
        }

        let active_before = self.session.active_id_owned();
        let mut closed = 0usize;
        let mut failed = 0usize;
        for session_id in session_ids {
            let close_result = if self.remote_desktop.is_session(&session_id) {
                self.close_remote_desktop_runtime(&session_id)
            } else {
                self.session
                    .manager()
                    .close(&session_id)
                    .map_err(anyhow::Error::from)
            };
            match close_result {
                Ok(()) => {
                    self.cleanup_recording_for_session(&session_id);
                    self.remove_session_state(&session_id, cx);
                    closed += 1;
                }
                Err(_) => {
                    failed += 1;
                }
            }
        }
        self.prune_workspace_split();

        // After close_session_batch, local metadata is the source of truth for
        // remaining tabs (includes disconnected). Avoid transport map lock.
        let active_is_live = active_before
            .as_deref()
            .is_some_and(|session_id| self.session.has_session(session_id));

        if !active_is_live {
            self.ai.reset_agent_runtime();
            self.sync_session_event_bridge_policy();
            if let Some(next_session_id) = self.session.next_live_session() {
                self.activate_session_id(&next_session_id, cx);
            } else {
                self.session.clear_active_session();
                self.terminal.view.output = String::from(INITIAL_TERMINAL_BANNER);
                self.terminal.view.output_decoder.reset_decoder();
                self.terminal.view.screen = initial_terminal_screen();
                self.terminal
                    .view
                    .screen
                    .set_encoding(&self.settings.summary().interaction_default_encoding);
            }
        }

        self.shell.set_status(if failed == 0 {
            format!("closed {closed} {label} session(s)")
        } else {
            format!("closed {closed} {label} session(s), {failed} failed")
        });
    }

    pub(in crate::features) fn handle_window_minimize(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.summary().minimize_to_tray {
            if self.minimize_to_system_tray(window, cx) {
                return;
            }
            self.notify_operation(
                "tray-unavailable",
                nyaterm_ui::notification::NyaNotificationKind::Warning,
                rust_i18n::t!("tray.unavailable").to_string(),
                cx,
            );
        }
        window.minimize_window();
    }

    pub(crate) fn handle_window_close_request(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open_sessions = self.session.ordered_sessions().len();
        if self.settings.summary().confirm_on_close && open_sessions > 0 {
            // Reuse the close-all confirmation as the quit-with-sessions gate.
            // A title-bar close control can also produce the native window-close
            // callback, so retain the existing modal rather than stacking a
            // second confirmation for the same close request.
            if self.session.dialog_should_quit_after_close_all() {
                return;
            }
            self.session.dialog_request_quit_after_close_all();
            self.open_close_all_sessions_confirm(window, cx);
            self.shell.set_status(format!(
                "confirm close: {open_sessions} session(s) still open"
            ));
            cx.notify();
            return;
        }
        // Persist workspace before exit when startup restore is enabled.
        if self.settings.summary().startup_restore {
            self.flush_open_tabs_now(cx);
        }
        if self.prepare_native_update_exit(cx) {
            cx.emit(AppLifecycleEvent::ShutdownRequested);
        }
    }

    pub(in crate::features) fn open_close_all_sessions_confirm(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session.ordered_sessions().is_empty() {
            self.shell.set_status("no sessions to close".to_string());
            cx.notify();
            return;
        }
        self.session.dialog_open_close_all_sessions_confirm();
        self.shell
            .set_status("close all sessions confirmation opened".to_string());
        let title_key = if self.session.dialog_should_quit_after_close_all() {
            "dialog.confirmClose"
        } else {
            "tabCtx.closeAll"
        };
        let description_key = if self.session.dialog_should_quit_after_close_all() {
            "dialog.confirmCloseDesc"
        } else {
            "tabCtx.closeAllConfirm"
        };
        let action_key = if self.session.dialog_should_quit_after_close_all() {
            "dialog.confirmCloseAction"
        } else {
            "tabCtx.closeAll"
        };
        self.open_confirm_dialog_with_cancel(
            (
                t!(title_key).to_string(),
                t!(description_key).to_string(),
                t!(action_key).to_string(),
                true,
                |app, window, cx| app.confirm_close_all_sessions(window, cx),
                |app, cx| app.cancel_close_all_sessions_confirm(cx),
            ),
            window,
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn cancel_close_all_sessions_confirm(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.update.install_requested = false;
        self.session.dialog_cancel_close_all_sessions_confirm();
        self.shell
            .set_status("close all sessions cancelled".to_string());
        cx.notify();
    }

    pub(in crate::features) fn confirm_close_all_sessions(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let quit_after = self.session.dialog_take_close_all_sessions_confirm();
        if quit_after {
            if self.settings.summary().startup_restore {
                self.flush_open_tabs_now(cx);
            }
            self.shell.set_status("closing window".to_string());
            if self.prepare_native_update_exit(cx) {
                cx.emit(AppLifecycleEvent::ShutdownRequested);
            }
            return true;
        }
        self.close_all_sessions(cx);
        cx.notify();
        true
    }

    pub(in crate::features) fn close_all_sessions(&mut self, cx: &mut Context<Self>) {
        let roots = self
            .ordered_tab_sessions()
            .into_iter()
            .map(|session| session.id)
            .collect::<Vec<_>>();
        self.close_tab_roots_batch(roots, "active", cx);
        cx.notify();
    }

    pub(in crate::features) fn close_inactive_sessions(
        &mut self,
        keep_session_id: String,
        cx: &mut Context<Self>,
    ) {
        let roots = self
            .ordered_tab_sessions()
            .into_iter()
            .filter_map(|session| (session.id != keep_session_id).then_some(session.id))
            .collect::<Vec<_>>();
        self.activate_session_id_with_surface_sync(&keep_session_id, cx);
        self.close_tab_roots_batch(roots, "inactive", cx);
        cx.notify();
    }

    pub(in crate::features) fn close_sessions_to_right(
        &mut self,
        anchor_session_id: String,
        cx: &mut Context<Self>,
    ) {
        let sessions = self.ordered_tab_sessions();
        let Some(anchor_index) = sessions
            .iter()
            .position(|session| session.id == anchor_session_id)
        else {
            self.shell
                .set_status("session no longer exists".to_string());
            cx.notify();
            return;
        };
        let roots = sessions
            .into_iter()
            .skip(anchor_index + 1)
            .map(|session| session.id)
            .collect::<Vec<_>>();
        self.close_tab_roots_batch(roots, "right-side", cx);
        cx.notify();
    }

    fn close_tab_roots_batch(
        &mut self,
        tab_roots: Vec<String>,
        label: &'static str,
        cx: &mut Context<Self>,
    ) {
        let mut skipped = 0usize;
        let mut close_ids = Vec::new();
        for tab_root in tab_roots {
            if self.tab_tree_is_locked(&tab_root) {
                skipped += 1;
            } else {
                close_ids.extend(self.tab_tree_session_ids(&tab_root));
            }
        }
        close_ids.sort();
        close_ids.dedup();
        self.close_session_batch(close_ids, label, cx);
        if skipped > 0 {
            self.shell
                .set_status(format!("{} ({skipped})", t!("tabCtx.lockedTabsSkipped")));
        }
    }

    pub(in crate::features) fn clear_terminal(&mut self, cx: &mut Context<Self>) {
        self.clear_terminal_selection(cx);
        if let Some(session_id) = self.session.active_id()
            && let Some(view) = self.terminal.view.views.get_mut(session_id)
        {
            view.clear();
        }
        self.terminal.view.output.clear();
        self.terminal.view.output_decoder.reset_decoder();
        self.terminal.view.screen.clear();
        self.shell.set_status("terminal cleared".to_string());
        cx.notify();
    }

    pub(in crate::features) fn append_terminal_log(&mut self, text: impl AsRef<str>) {
        let session_id = self.session.active_id_owned();
        self.append_terminal_log_for_session(session_id.as_deref(), text.as_ref(), false);
    }
}
