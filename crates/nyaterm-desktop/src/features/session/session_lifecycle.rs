use std::path::PathBuf;

use gpui::{Context, Window};

use crate::features::formatting::short_id;
use crate::features::{NyaTermApp, session::SavedConnectionStartOptions};
use crate::models::{SessionLaunchConfig, StartupCommandRequest};

use super::session_runtime::{MultiplexSshStartRequest, SessionStartFocusTarget};

const RECONNECTING_BANNER: &str = "\r\n\x1b[36m[Reconnecting…]\x1b[0m\r\n";

impl NyaTermApp {
    pub(in crate::features) fn duplicate_active_session(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.duplicate_active_session_with_startup(None, window, cx);
    }

    pub(in crate::features) fn duplicate_active_session_with_startup(
        &mut self,
        startup_command: Option<StartupCommandRequest>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.duplicate_active_session_with_options(startup_command, None, window, cx);
    }

    pub(in crate::features) fn duplicate_active_local_session_in_directory(
        &mut self,
        working_dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.duplicate_active_session_with_options(None, Some(working_dir), window, cx);
    }

    fn duplicate_active_session_with_options(
        &mut self,
        startup_command: Option<StartupCommandRequest>,
        working_dir: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session.start_has_active_pending() || self.session.start_has_active_failed() {
            self.shell
                .set_status("select a connected session before duplicating".to_string());
            cx.notify();
            return;
        }
        let Some(source_session_id) = self.session.active_id_owned() else {
            self.shell
                .set_status("no active session to duplicate".to_string());
            cx.notify();
            return;
        };
        let Some(metadata) = self.session.metadata(&source_session_id).cloned() else {
            self.shell
                .set_status("active session cannot be duplicated".to_string());
            cx.notify();
            return;
        };
        let custom_name = self
            .session
            .custom_name(&source_session_id)
            .map(str::to_string);
        let custom_color = self.session.tab_color(&source_session_id);
        let workspace_split = self.session.start_take_pending_workspace_split();

        match metadata.launch_config.clone() {
            SessionLaunchConfig::Local(mut config) => {
                if let Some(working_dir) = working_dir {
                    config.working_dir = Some(working_dir);
                }
                self.apply_desired_geometry_to_local_config(&mut config);
                self.begin_background_session_start(
                    format!("{} duplicate", config.name),
                    SessionLaunchConfig::Local(config),
                    metadata.source_connection_id.clone(),
                    metadata.ai_execution_profile,
                    SavedConnectionStartOptions {
                        custom_name,
                        tab_color: custom_color,
                        after_session_id: Some(source_session_id),
                        startup_command,
                        workspace_split: workspace_split.clone(),
                        ..Default::default()
                    },
                    cx,
                );
            }
            SessionLaunchConfig::Telnet(config) => {
                self.begin_background_session_start(
                    format!("{} duplicate", config.name),
                    SessionLaunchConfig::Telnet(config),
                    metadata.source_connection_id.clone(),
                    metadata.ai_execution_profile,
                    SavedConnectionStartOptions {
                        custom_name,
                        tab_color: custom_color,
                        after_session_id: Some(source_session_id),
                        startup_command,
                        workspace_split: workspace_split.clone(),
                        ..Default::default()
                    },
                    cx,
                );
            }
            SessionLaunchConfig::Serial(config) => {
                self.begin_background_session_start(
                    format!("{} duplicate", config.name),
                    SessionLaunchConfig::Serial(config),
                    metadata.source_connection_id.clone(),
                    metadata.ai_execution_profile,
                    SavedConnectionStartOptions {
                        custom_name,
                        tab_color: custom_color,
                        after_session_id: Some(source_session_id),
                        startup_command,
                        workspace_split: workspace_split.clone(),
                        ..Default::default()
                    },
                    cx,
                );
            }
            SessionLaunchConfig::Ssh(config) => {
                self.begin_background_ssh_start(
                    format!("{} duplicate", config.name),
                    *config,
                    metadata.source_connection_id.clone(),
                    metadata.ai_execution_profile,
                    SavedConnectionStartOptions {
                        custom_name,
                        tab_color: custom_color,
                        after_session_id: Some(source_session_id),
                        startup_command,
                        workspace_split: workspace_split.clone(),
                        ..Default::default()
                    },
                    cx,
                );
            }
            SessionLaunchConfig::Rdp(_) | SessionLaunchConfig::Vnc(_) => {
                self.shell
                    .set_status("remote desktop duplication is not available".to_string());
                self.shell.show_workspace();
                cx.notify();
                return;
            }
        }
        self.shell.show_workspace();
        if self.session.start_has_active_pending() {
            self.focus_user_started_session_surface(SessionStartFocusTarget::Terminal, window, cx);
        }
        cx.notify();
    }

    pub(in crate::features) fn multiplex_active_ssh_session(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.multiplex_active_ssh_session_with_startup(None, window, cx);
    }

    pub(in crate::features) fn multiplex_active_ssh_session_with_startup(
        &mut self,
        startup_command: Option<StartupCommandRequest>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session.start_has_active_pending() || self.session.start_has_active_failed() {
            self.shell
                .set_status("select a connected session before multiplexing".to_string());
            cx.notify();
            return;
        }
        let Some(source_session_id) = self.session.active_id_owned() else {
            self.shell
                .set_status("no active SSH session to multiplex".to_string());
            cx.notify();
            return;
        };
        let Some(metadata) = self.session.metadata(&source_session_id).cloned() else {
            self.shell
                .set_status("active session cannot be multiplexed".to_string());
            cx.notify();
            return;
        };
        let SessionLaunchConfig::Ssh(config) = metadata.launch_config.clone() else {
            self.shell
                .set_status("active session is not SSH".to_string());
            cx.notify();
            return;
        };
        let existing_multiplex_key = metadata.ssh_multiplex_key.clone();
        let existing_multiplex = self.session.ssh_connection_for_session(&source_session_id);
        let custom_name = self
            .session
            .custom_name(&source_session_id)
            .map(str::to_string);
        let custom_color = self.session.tab_color(&source_session_id);
        self.begin_background_multiplex_ssh_start(
            MultiplexSshStartRequest {
                connection_name: format!("{} multiplex", config.name),
                config: *config,
                source_connection_id: metadata.source_connection_id.clone(),
                ai_execution_profile: metadata.ai_execution_profile,
                options: SavedConnectionStartOptions {
                    custom_name,
                    tab_color: custom_color,
                    after_session_id: Some(source_session_id),
                    startup_command,
                    ..Default::default()
                },
                existing_multiplex,
                existing_multiplex_key,
            },
            cx,
        );
        self.shell.show_workspace();
        if self.session.start_has_active_pending() {
            self.focus_user_started_session_surface(SessionStartFocusTarget::Terminal, window, cx);
        }
        cx.notify();
    }

    pub(in crate::features) fn reconnect_active_session(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(source_session_id) = self.session.active_id_owned() else {
            self.shell
                .set_status("no active session to reconnect".to_string());
            cx.notify();
            return;
        };
        self.reconnect_session(source_session_id, window, cx);
    }

    /// Close the backend session but keep the tab for reconnect (Tauri Disconnect).
    pub(in crate::features) fn disconnect_session(
        &mut self,
        session_id: String,
        cx: &mut Context<Self>,
    ) {
        if self.session.session_is_busy(&session_id) {
            self.shell
                .set_status("session action already in progress".to_string());
            cx.notify();
            return;
        }
        if self.session.is_disconnected(&session_id) {
            self.shell
                .set_status("session already disconnected".to_string());
            cx.notify();
            return;
        }
        if !self.session.has_session(&session_id) {
            self.shell
                .set_status("session no longer exists".to_string());
            cx.notify();
            return;
        }

        self.session.begin_disconnect_action(session_id.clone());
        // Backend may already be gone (race with Exited); still mark disconnected.
        if self.remote_desktop.is_session(&session_id) {
            let _ = self.close_remote_desktop_runtime(&session_id);
        } else {
            let _ = self.session.manager().close(&session_id);
        }
        self.disconnect_recording_for_session(&session_id);
        self.mark_session_disconnected(&session_id, cx);
        self.session.finish_busy_action(&session_id);
        self.shell
            .set_status(format!("disconnected {}", short_id(&session_id)));
        cx.notify();
    }

    pub(in crate::features) fn mark_session_disconnected(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        self.terminal.invalidate_shell_editing_session(session_id);
        self.clear_terminal_mouse_report_for_session(session_id);
        self.transfer.clear_file_clipboard_for_session(session_id);
        self.session.remove_remote_file_service(session_id);
        let Some(update) = self.session.mark_session_disconnected(session_id) else {
            return;
        };
        if update.already_disconnected {
            return;
        }
        self.close_terminal_executions_for_session(session_id, cx);
        let banner = "\r\n\x1b[31m[Session disconnected]\x1b[0m\r\n\x1b[33m[Press Enter to reconnect]\x1b[0m\r\n";
        let encoding = self
            .session
            .metadata(session_id)
            .and_then(|metadata| metadata.launch_config.encoding())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| self.settings.summary().interaction_default_encoding.clone());
        let scrollback_limit = self.terminal_scrollback_line_limit();
        self.terminal.append_session_text_or_create(
            session_id,
            &encoding,
            scrollback_limit,
            banner,
        );

        if self.session.active_id() == Some(session_id) {
            self.terminal.clear_active_session_assist();
        }
        self.prune_workspace_split();
        cx.notify();
    }

    /// Reconnect a disconnected tab (or force-recreate a live one) by id.
    pub(in crate::features) fn reconnect_session(
        &mut self,
        session_id: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session.session_is_busy(&session_id) {
            self.shell
                .set_status("session action already in progress".to_string());
            cx.notify();
            return;
        }
        if self.session.start_reconnect_is_pending(&session_id) {
            self.shell
                .set_status("session is already reconnecting".to_string());
            cx.notify();
            return;
        }
        if !self.session.has_session(&session_id) {
            self.shell
                .set_status("session cannot be reconnected".to_string());
            cx.notify();
            return;
        }
        self.remote_ops.clear_stats_sample(&session_id);
        self.session.begin_reconnect_action(session_id.clone());
        self.session.start.clear_active_selection();
        let old_id = session_id;
        self.session.start.clear_reconnect_failure(&old_id);
        let source_index = self
            .session
            .session_index(&old_id)
            .unwrap_or(self.session.session_order_len());
        let custom_name = self.session.custom_name(&old_id).map(str::to_string);
        let custom_color = self.session.tab_color(&old_id);
        let reconnect_cwd_command = self
            .settings
            .summary()
            .terminal_reconnect_restore_cwd
            .then(|| self.session.cwd(&old_id))
            .flatten()
            .and_then(nyaterm_transport::build_ssh_reconnect_cwd_command);
        // Tauri: write cyan reconnecting line into the buffer before recreating.
        self.terminal
            .append_existing_session_text(&old_id, RECONNECTING_BANNER);

        // Close live backend if still present.
        if self.remote_desktop.is_session(&old_id) {
            let _ = self.close_remote_desktop_runtime(&old_id);
        } else {
            let _ = self.session.manager().close(&old_id);
        }
        self.disconnect_recording_for_session(&old_id);
        self.clear_terminal_mouse_report_for_session(&old_id);
        self.transfer.clear_file_clipboard_for_session(&old_id);
        let Some(metadata) = self.session.metadata(&old_id).cloned() else {
            self.session.finish_busy_action(&old_id);
            self.shell
                .set_status("session cannot be reconnected".to_string());
            cx.notify();
            return;
        };
        self.session.mark_session_disconnected(&old_id);
        self.close_terminal_executions_for_session(&old_id, cx);
        let launch_config = metadata.launch_config;
        let source_connection_id = metadata.source_connection_id;
        let ai_execution_profile = metadata.ai_execution_profile;
        match launch_config {
            SessionLaunchConfig::Local(mut config) => {
                self.apply_desired_geometry_to_local_config(&mut config);
                self.begin_background_session_start(
                    format!("{} reconnect", config.name),
                    SessionLaunchConfig::Local(config),
                    source_connection_id,
                    ai_execution_profile,
                    SavedConnectionStartOptions {
                        custom_name,
                        tab_color: custom_color,
                        insert_index: Some(source_index),
                        reconnect_session_id: Some(old_id.clone()),
                        ..Default::default()
                    },
                    cx,
                );
            }
            SessionLaunchConfig::Telnet(config) => {
                self.begin_background_session_start(
                    format!("{} reconnect", config.name),
                    SessionLaunchConfig::Telnet(config),
                    source_connection_id,
                    ai_execution_profile,
                    SavedConnectionStartOptions {
                        custom_name,
                        tab_color: custom_color,
                        insert_index: Some(source_index),
                        startup_command: reconnect_cwd_command.map(|command| {
                            crate::models::StartupCommandRequest {
                                command,
                                delay_ms: 0,
                            }
                        }),
                        reconnect_session_id: Some(old_id.clone()),
                        ..Default::default()
                    },
                    cx,
                );
            }
            SessionLaunchConfig::Serial(config) => {
                self.begin_background_session_start(
                    format!("{} reconnect", config.name),
                    SessionLaunchConfig::Serial(config),
                    source_connection_id,
                    ai_execution_profile,
                    SavedConnectionStartOptions {
                        custom_name,
                        tab_color: custom_color,
                        insert_index: Some(source_index),
                        reconnect_session_id: Some(old_id.clone()),
                        ..Default::default()
                    },
                    cx,
                );
            }
            SessionLaunchConfig::Ssh(config) => {
                self.begin_background_ssh_start(
                    format!("{} reconnect", config.name),
                    *config,
                    source_connection_id,
                    ai_execution_profile,
                    SavedConnectionStartOptions {
                        custom_name,
                        tab_color: custom_color,
                        insert_index: Some(source_index),
                        reconnect_session_id: Some(old_id.clone()),
                        ..Default::default()
                    },
                    cx,
                );
            }
            SessionLaunchConfig::Rdp(_) | SessionLaunchConfig::Vnc(_) => {
                self.session.finish_busy_action(&old_id);
                self.shell
                    .set_status("Use Retry in the remote desktop view to reconnect".to_string());
            }
        }
        // Tauri clears busy when reconnect action returns (even if SSH still connecting).
        self.session.finish_busy_action(&old_id);
        self.session.retain_busy_actions_for_live_sessions();
        self.shell.show_workspace();
        cx.notify();
    }

    pub(in crate::features) fn migrate_reconnected_session_state(
        &mut self,
        old_id: &str,
        new_id: &str,
        cx: &mut Context<Self>,
    ) {
        self.session.start.clear_reconnect_failure(old_id);
        let encoding = self
            .session
            .metadata(new_id)
            .and_then(|metadata| metadata.launch_config.encoding())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| self.settings.summary().interaction_default_encoding.clone());
        self.terminal.rekey_session_view(old_id, new_id, &encoding);
        self.terminal
            .move_session_surface_bounds(old_id, new_id.to_string());
        self.terminal.move_search_session_state(old_id, new_id);
        self.session.replace_session_order_id(old_id, new_id);
        self.session.migrate_session_presentation(old_id, new_id);

        self.shell.replace_workspace_session_id(old_id, new_id);
        self.terminal.replace_terminal_window_tab_id(old_id, new_id);
        self.sync_input.replace_session_id(old_id, new_id);
        if self.session.active_id() == Some(old_id) {
            self.activate_session_id(new_id, cx);
        }
        self.transfer.replace_session_id(old_id, new_id);
        if self.session.active_id() == Some(new_id)
            && self.transfer.has_browser_session_cache(new_id)
        {
            self.restore_transfer_browser_session_cache(new_id, cx);
        }
        self.sync_workspace_split_from_active_tab();
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use nyaterm_terminal::TerminalScreen;
    use nyaterm_transport::SshSessionConfig;

    use super::RECONNECTING_BANNER;
    use crate::features::test_support::{
        app_with_visible_local_session, session_focus_test_window,
    };
    use crate::models::SessionLaunchConfig;
    use crate::test_support::TestConfigDir;

    #[test]
    fn duplicate_session_focus_handoff_survives_menu_dismissal() {
        let root = TestConfigDir::new("nyaterm-duplicate-focus");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "source");
        let cx = session_focus_test_window(&mut cx, &app);

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                // Exercise the real start path without launching an interactive shell.
                let mut metadata = app.session.metadata("source").unwrap().clone();
                let SessionLaunchConfig::Local(config) = &mut metadata.launch_config else {
                    panic!("expected local fixture");
                };
                config.shell_path = Some(
                    root.path()
                        .join("missing-test-shell")
                        .to_string_lossy()
                        .into_owned(),
                );
                app.session.register_session_metadata("source", metadata);
                window.focus(app.session.dialog_tab_actions_focus(), cx);

                app.duplicate_active_session(window, cx);
                assert!(app.session.start_has_active_pending());
                assert!(app.terminal.input_focus().is_focused(window));
                // Menus restore their previous focus after the action returns.
                window.focus(app.session.dialog_tab_actions_focus(), cx);
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(app.read(cx).terminal.input_focus().is_focused(window));
        });
    }

    #[test]
    fn duplicate_without_a_valid_start_preserves_existing_input_focus() {
        for missing_source in [true, false] {
            let root = TestConfigDir::new("nyaterm-duplicate-invalid-start-focus");
            let mut cx = TestAppContext::single();
            let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
            let app = app_with_visible_local_session(&mut cx, root.path(), "source");
            let cx = session_focus_test_window(&mut cx, &app);
            cx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    if missing_source {
                        app.session.clear_active_session();
                    } else {
                        let mut metadata = app.session.metadata("source").unwrap().clone();
                        let SessionLaunchConfig::Local(config) = &mut metadata.launch_config else {
                            panic!("expected local fixture");
                        };
                        config.encoding = "invalid-test-encoding".into();
                        app.session.register_session_metadata("source", metadata);
                    }
                    window.focus(app.session.prompt_credential_focus(), cx);
                    app.duplicate_active_session(window, cx);
                    assert!(!app.session.start_has_active_pending());
                });
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                assert!(
                    app.read(cx)
                        .session
                        .prompt_credential_focus()
                        .is_focused(window)
                );
            });
        }
    }

    #[test]
    fn multiplex_session_focus_handoff_survives_menu_dismissal() {
        let root = TestConfigDir::new("nyaterm-multiplex-focus");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "source");
        let cx = session_focus_test_window(&mut cx, &app);

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let mut metadata = app.session.metadata("source").unwrap().clone();
                // Multiplexing needs SSH metadata. A closed loopback port keeps the
                // worker from reaching a real server while still exercising the
                // user-initiated start path.
                metadata.launch_config = SessionLaunchConfig::Ssh(Box::new(SshSessionConfig {
                    name: "source".to_string(),
                    host: "127.0.0.1".to_string(),
                    port: 1,
                    username: "nya".to_string(),
                    ..SshSessionConfig::default()
                }));
                app.session.register_session_metadata("source", metadata);
                window.focus(app.session.dialog_tab_actions_focus(), cx);

                app.multiplex_active_ssh_session(window, cx);
                assert!(app.session.start_has_active_pending());
                assert!(app.terminal.input_focus().is_focused(window));
                // Menus restore their previous focus after the action returns.
                window.focus(app.session.dialog_tab_actions_focus(), cx);
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(app.read(cx).terminal.input_focus().is_focused(window));
        });
    }

    #[test]
    fn reconnect_banner_returns_cursor_to_first_column_before_server_output() {
        let mut screen = TerminalScreen::new(80, 24);
        screen.advance(b"previous output");
        screen.advance_decoded_text(RECONNECTING_BANNER);
        screen.advance(b"Welcome to Ubuntu\r\n");

        assert!(
            screen
                .lines()
                .iter()
                .any(|line| line.starts_with("Welcome to Ubuntu"))
        );
    }
}
