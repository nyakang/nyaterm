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

    pub(in crate::features) fn close_active_session(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(key) = self.active_workspace_tab() {
            match crate::models::WorkspaceTabId::from_layout_key(&key) {
                crate::models::WorkspaceTabId::Settings => {
                    self.request_settings_tab_close(window, cx);
                    return;
                }
                id if !id.is_session() => {
                    self.request_workspace_tabs_close(vec![key], None, cx);
                    return;
                }
                _ => {}
            }
        }
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
        if self.hide_referenced_session_tab(&session_id, cx) {
            return;
        }
        if self.tab_tree_is_locked(&session_id) {
            self.notify_locked_tab_close_blocked(cx);
            return;
        }
        let preferred_next = self.shell.terminal_window_tree().and_then(|mut root| {
            let group = root.leaf_for_tab(&session_id).map(str::to_string)?;
            let groups = root.leaf_ids();
            let index = groups.iter().position(|id| id == &group).unwrap_or(0);
            let next = root.remove_tab(&session_id)?;
            let surviving = next.leaf_ids();
            let group = if next.leaf_tabs(&group).is_some() {
                group
            } else {
                surviving
                    .get(index.min(surviving.len().saturating_sub(1)))?
                    .clone()
            };
            next.leaf_tabs(&group)?.1.map(str::to_string)
        });
        // Tauri: closing a strip tab closes the whole tab tree; closing a secondary leaf
        // only removes that pane. Strip close uses the tab-root id.
        let mut close_ids = if !self.is_secondary_pane_session(&session_id) {
            if let Some(root) = self.shell.workspace_pane_root(&session_id) {
                root.session_ids()
            } else {
                vec![session_id.clone()]
            }
        } else {
            vec![session_id.clone()]
        };
        // A tmux tab projects pane leaves, but the host SSH worker still owns
        // the control client. Closing the tab releases that host as well.
        if self.tmux.has_host(&session_id) && !close_ids.contains(&session_id) {
            close_ids.insert(0, session_id.clone());
        }
        let was_active = self
            .session
            .active_id()
            .is_some_and(|active_id| close_ids.iter().any(|id| id == active_id));
        // A pane that fails to close must not abandon the rest of the tab tree half
        // torn down: the remaining panes still close and the layout is still pruned.
        let mut close_error = None;
        let mut closed_any = false;
        for close_id in &close_ids {
            let disconnected = self.session.is_disconnected(close_id);
            let close_result = if self.remote_desktop.is_session(close_id) {
                self.close_remote_desktop_runtime(close_id)
            } else {
                self.session
                    .manager()
                    .close_detached(close_id)
                    .map_err(anyhow::Error::from)
            };
            match close_result {
                Ok(()) => {}
                Err(_) if disconnected => {}
                Err(error) if !disconnected && !self.session.has_session(close_id) => {
                    close_error.get_or_insert(error);
                    continue;
                }
                Err(error) => {
                    close_error.get_or_insert(error);
                    continue;
                }
            }
            self.cleanup_recording_for_session(close_id);
            self.remove_session_state(close_id, cx);
            closed_any = true;
        }
        if !closed_any && let Some(error) = close_error.take() {
            self.shell.set_status(format!("close failed: {error}"));
            cx.notify();
            return;
        }
        self.prune_workspace_split();
        if was_active {
            self.ai.reset_agent_runtime();
            self.sync_session_event_bridge_policy();
            if let Some(next_tab) = preferred_next
                .filter(|id| self.workspace_tab_ids().contains(id))
                .or_else(|| {
                    self.session
                        .next_session_after(&session_id)
                        .filter(|id| !self.shell.session_tab_is_hidden(id))
                })
                .or_else(|| self.workspace_tab_ids().first().cloned())
            {
                self.activate_workspace_tab(&next_tab, cx);
                self.shell.set_status("session closed".to_string());
            } else {
                self.session.clear_active_session();
                self.sync_ai_active_scope(cx);
                self.terminal.view.output = String::from(INITIAL_TERMINAL_BANNER);
                self.terminal.view.output_decoder.reset_decoder();
                self.terminal.view.screen = initial_terminal_screen();
                if let Err(error) = self
                    .terminal
                    .view
                    .screen
                    .set_encoding(&self.settings.summary().interaction_default_encoding)
                {
                    self.shell
                        .set_status(crate::features::terminal::encoding_error_text(error));
                }
                self.shell.set_status("session closed".to_string());
            }
        } else {
            self.shell
                .set_status(format!("closed {}", short_id(&session_id)));
        }
        if let Some(error) = close_error {
            self.shell.set_status(format!("close failed: {error}"));
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
        if self.tmux.pane_for_session(&pane_id).is_some() {
            // Pane-close is destructive; tab-close, detach and teardown are not.
            // Let the confirmed remote layout remove the local terminal state.
            if let Err(error) = self.session.manager().tmux_close_pane(&pane_id) {
                self.shell.set_status(format!("close failed: {error}"));
            }
            cx.notify();
            return;
        }
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

        if self.workspace_references_session(&pane_id, cx) {
            let next = self
                .tab_tree_session_ids(&pane_id)
                .into_iter()
                .find(|id| id != &pane_id)
                .or_else(|| {
                    self.tab_tree_session_ids(tab_root_id)
                        .into_iter()
                        .find(|id| id != &pane_id)
                });
            self.shell.hide_workspace_session(&pane_id);
            self.prune_workspace_split();
            self.reconcile_terminal_windows();
            if let Some(next) = next {
                self.activate_session_id(&next, cx);
            }
            self.persist_open_tabs();
            cx.notify();
            return;
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
                self.sync_ai_active_scope(cx);
            }
        }
        self.shell
            .set_status(format!("closed {}", short_id(&pane_id)));
        cx.notify();
    }

    pub(in crate::features) fn handle_window_minimize(
        &mut self,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        window.minimize_window();
    }

    pub(crate) fn handle_window_close_request(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace_count = self
            .desktop_controller
            .as_ref()
            .and_then(gpui::WeakEntity::upgrade)
            .map_or(1, |controller| controller.read(cx).workspace_count());
        if self.settings.summary().minimize_to_tray && workspace_count == 1 {
            if self.minimize_to_system_tray(window, cx) {
                return;
            }
            self.notify_operation(
                "tray-unavailable",
                nyaterm_ui::notification::NyaNotificationKind::Warning,
                rust_i18n::t!("tray.unavailable").to_string(),
                cx,
            );
            // Keep the window recoverable from the taskbar when the tray cannot
            // hide it. Explicit Quit still uses the normal shutdown path.
            // GPUI's test window does not implement native minimization.
            #[cfg(not(test))]
            window.minimize_window();
            return;
        }
        self.handle_window_close_request_with_count(
            self.session.ordered_sessions().len(),
            window,
            cx,
        );
    }

    pub(crate) fn handle_window_close_request_with_count(
        &mut self,
        open_sessions: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.theme_designer().submitting {
            cx.emit(AppLifecycleEvent::ShutdownCancelled);
            return;
        }
        if self.settings.theme_designer().dirty() {
            self.begin_settings_draft(cx);
            self.open_settings_workspace_tab(cx);
            self.theme_designer_action(
                crate::features::settings::theme_designer::ThemeIntent::Quit(open_sessions),
                cx,
            );
            return;
        }
        if self.request_settings_quit_confirmation(open_sessions, window, cx) {
            return;
        }
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
        if self.has_unconfirmed_editor_changes() {
            let targets = self
                .transfer
                .editor_documents()
                .iter()
                .map(|doc| crate::models::WorkspaceTabId::Editor(doc.id.clone()).layout_key())
                .collect();
            self.request_workspace_tabs_close(targets, Some(open_sessions), cx);
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
        if self.session.dialog_should_quit_after_close_all() {
            self.cancel_editor_exit_approval();
            cx.emit(AppLifecycleEvent::ShutdownCancelled);
        }
        self.update
            .update(cx, |update, _| update.install_requested = false);
        self.session.dialog_cancel_close_all_sessions_confirm();
        self.shell
            .set_status("close all sessions cancelled".to_string());
        cx.notify();
    }

    pub(in crate::features) fn confirm_close_all_sessions(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let quit_after = self.session.dialog_take_close_all_sessions_confirm();
        if quit_after {
            self.handle_window_close_request_with_count(0, window, cx);
            return true;
        }
        self.close_all_sessions(cx);
        cx.notify();
        true
    }

    pub(in crate::features) fn close_all_sessions(&mut self, cx: &mut Context<Self>) {
        self.request_workspace_tabs_close(self.workspace_tab_ids(), None, cx);
    }

    pub(in crate::features) fn close_inactive_sessions(
        &mut self,
        keep_session_id: String,
        cx: &mut Context<Self>,
    ) {
        let targets = self
            .workspace_tab_ids()
            .into_iter()
            .filter(|key| key != &keep_session_id)
            .collect();
        self.activate_workspace_tab(&keep_session_id, cx);
        self.request_workspace_tabs_close(targets, None, cx);
    }

    pub(in crate::features) fn close_sessions_to_right(
        &mut self,
        anchor_session_id: String,
        cx: &mut Context<Self>,
    ) {
        let tabs = self
            .shell
            .terminal_group_for_tab(&anchor_session_id)
            .as_deref()
            .and_then(|group| self.shell.terminal_group_tabs(group))
            .map(|(tabs, _)| tabs)
            .unwrap_or_else(|| self.workspace_tab_ids());
        let Some(index) = tabs.iter().position(|key| key == &anchor_session_id) else {
            return;
        };
        self.request_workspace_tabs_close(tabs.into_iter().skip(index + 1).collect(), None, cx);
    }

    pub(in crate::features) fn navigate_terminal_command(
        &mut self,
        action: nyaterm_terminal::command_navigation::CommandNavigationAction,
        cx: &mut Context<Self>,
    ) {
        if let Some(session_id) = self.session.active_id_owned() {
            self.terminal.view.frame_pipeline.navigate_command(
                session_id.clone(),
                action,
                self.terminal_display_offset_for_session(Some(&session_id)),
                self.terminal
                    .view
                    .views
                    .get(&session_id)
                    .map_or_else(Default::default, |view| view.history_state()),
            );
            self.terminal.view.frame_pipeline.arm_event_wakes();
        }
        cx.notify();
    }

    pub(in crate::features) fn clear_terminal(&mut self, cx: &mut Context<Self>) {
        self.clear_terminal_selection(cx);
        self.terminal.menus.actions_open = false;
        if let Some(session_id) = self.session.active_id() {
            self.terminal
                .view
                .frame_pipeline
                .clear_session_except_input(session_id.to_string());
            self.terminal.view.frame_pipeline.arm_event_wakes();
            self.shell
                .set_status("terminal clear requested".to_string());
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{AppContext as _, EmptyView, TestAppContext};
    use nyaterm_core::test_support::TestTempDir;

    use crate::features::AppLifecycleEvent;
    use crate::features::test_support::app_with_visible_local_session;

    #[derive(Clone, Copy)]
    enum CloseRequest {
        Window,
        ExplicitQuit,
    }

    fn close_request_outcome(
        workspace_count: usize,
        minimize_to_tray: bool,
        confirm_on_close: bool,
        request: CloseRequest,
    ) -> (usize, bool) {
        let root = TestTempDir::new("nyaterm-close-to-tray");
        let controller_root = TestTempDir::new("nyaterm-close-workspaces");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "session");
        app.update(&mut cx, |app, cx| {
            let mut summary = app.settings.summary().clone();
            summary.minimize_to_tray = minimize_to_tray;
            summary.confirm_on_close = confirm_on_close;
            summary.startup_restore = false;
            app.settings.replace_summary(summary);
            app.sync_component_theme(cx);
        });

        let runtime = nyaterm_core::AppRuntime::from_parts_for_test(
            nyaterm_core::RuntimeMode::Portable,
            controller_root.path().into(),
            controller_root.join("config"),
            controller_root.join("logs"),
            controller_root.join("cache"),
            None,
        );
        let startup = crate::app_shell::AppShellStartup::prepare(&runtime);
        let controller =
            cx.new(|cx| crate::app_shell::DesktopController::new(runtime, startup, cx));
        controller.update(&mut cx, |controller, cx| {
            for _ in 0..workspace_count {
                controller
                    .open_workspace(nyaterm_core::OpenWorkspaceRequest::default(), cx)
                    .unwrap();
            }
        });
        if workspace_count > 0 {
            app.update(&mut cx, |app, _| {
                app.desktop_controller = Some(controller.downgrade())
            });
        }

        let shutdown_requests = Rc::new(Cell::new(0));
        let observed = Rc::clone(&shutdown_requests);
        let _subscription = cx.update(|cx| {
            cx.subscribe(&app, move |_, event, _| {
                if matches!(event, AppLifecycleEvent::ShutdownRequested) {
                    observed.set(observed.get() + 1);
                }
            })
        });

        let (_, window_cx) = cx.add_window_view(|window, cx| {
            let content = cx.new(|_| EmptyView);
            nyaterm_ui::nya_root(content, window, cx)
        });
        window_cx.update(|window, cx| {
            app.update(cx, |app, cx| match request {
                CloseRequest::Window => app.handle_window_close_request(window, cx),
                CloseRequest::ExplicitQuit => {
                    app.handle_window_close_request_with_count(0, window, cx)
                }
            });
        });
        cx.run_until_parked();
        let confirming = cx.read(|cx| app.read(cx).session.dialog_should_quit_after_close_all());
        (shutdown_requests.get(), confirming)
    }

    #[test]
    fn last_workspace_close_keeps_running_when_minimize_to_tray_is_enabled() {
        for count in [0, 1] {
            assert_eq!(
                close_request_outcome(count, true, true, CloseRequest::Window),
                (0, false)
            );
        }
    }

    #[test]
    fn close_request_shuts_down_when_minimize_to_tray_is_disabled() {
        assert_eq!(
            close_request_outcome(1, false, false, CloseRequest::Window),
            (1, false)
        );
    }

    #[test]
    fn close_request_preserves_session_confirmation_when_minimize_to_tray_is_disabled() {
        assert_eq!(
            close_request_outcome(1, false, true, CloseRequest::Window),
            (0, true)
        );
    }

    #[test]
    fn multiple_workspaces_close_normally_even_when_minimize_to_tray_is_enabled() {
        for count in [2, 3] {
            assert_eq!(
                close_request_outcome(count, true, false, CloseRequest::Window),
                (1, false)
            );
            assert_eq!(
                close_request_outcome(count, true, true, CloseRequest::Window),
                (0, true)
            );
        }
    }

    #[test]
    fn explicit_quit_bypasses_minimize_to_tray() {
        for count in [1, 2] {
            assert_eq!(
                close_request_outcome(count, true, true, CloseRequest::ExplicitQuit),
                (1, false)
            );
        }
    }
}
