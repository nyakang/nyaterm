use gpui::{Context, Window};
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::settings::theme_designer::ThemeIntent;
use crate::models::{NavItem, WorkspaceTabId};

/// Open settings as a regular workspace tab.
///
/// The tab is a layout key like an editor tab: it lives in the same terminal
/// window tree as sessions, so switching away leaves the draft and the running
/// sessions untouched. `begin_settings_draft` must already have run.
pub(in crate::features) fn key() -> String {
    WorkspaceTabId::settings_key()
}

impl NyaTermApp {
    pub(in crate::features) fn settings_tab_is_open(&self) -> bool {
        self.workspace_tab_ids()
            .iter()
            .any(|key| WorkspaceTabId::from_layout_key(key).is_settings())
    }

    pub(in crate::features) fn open_settings_workspace_tab(&mut self, cx: &mut Context<Self>) {
        let key = key();
        self.ensure_settings_workspace_tab_layout(&key);
        self.activate_workspace_tab(&key, cx);
    }

    fn ensure_settings_workspace_tab_layout(&mut self, key: &str) {
        // Only place a tab that is being opened; activating an existing
        // settings tab must not move it around the strip.
        if self.settings_tab_is_open() {
            return;
        }
        self.ensure_terminal_windows_root();
        let Some(mut tree) = self.shell.terminal_window_tree() else {
            return;
        };
        let after = self.active_workspace_tab();
        let group = after
            .as_deref()
            .and_then(|active| tree.leaf_for_tab(active))
            .map(str::to_string)
            .or_else(|| self.shell.workspace.focused_group.clone())
            .or_else(|| tree.first_leaf_id());
        tree.ensure_tab(key, group.as_deref());
        // Put the settings tab next to the tab it was opened from; `ensure_tab`
        // appends, which would otherwise leave it after every split leaf.
        if let Some(after) = after.as_deref().filter(|active| tree.contains_tab(active))
            && let Some(before) = tree.leaf_tab_ids_for_tab(after).and_then(|tabs| {
                let index = tabs.iter().position(|tab| tab == after)?;
                tabs.get(index + 1).cloned()
            })
        {
            tree.place_tab_before(key, &before);
        }
        let _ = tree.set_active_tab(key);
        self.shell.restore_terminal_window_tree(Some(tree));
    }

    /// Close request from the tab strip.
    ///
    /// Mirrors the old child-window close policy: an unsaved draft and an
    /// unsaved theme draft are confirmed first, and closing discards them.
    pub(in crate::features) fn request_settings_tab_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.theme_designer().submitting {
            return;
        }
        if self.settings.theme_designer().dirty() {
            self.theme_designer_action(ThemeIntent::CloseSettings, cx);
            return;
        }
        if !self.settings_draft_dirty() {
            self.cancel_settings(cx);
            return;
        }
        self.open_confirm_dialog(
            (
                t!("settings.discardChangesTitle").to_string(),
                t!("settings.discardChangesDesc").to_string(),
                t!("settings.discardChanges").to_string(),
                true,
                |app: &mut NyaTermApp, _window, cx| {
                    app.cancel_settings(cx);
                    true
                },
            ),
            window,
            cx,
        );
    }

    /// Remove the settings tab after the draft has been restored or applied.
    pub(in crate::features) fn close_settings_workspace_tab(&mut self, cx: &mut Context<Self>) {
        if let Some(controller) = self
            .desktop_controller
            .as_ref()
            .and_then(|controller| controller.upgrade())
        {
            let workspace_id = self.workspace_id;
            controller.update(cx, |controller, cx| {
                controller.release_settings_owner(workspace_id, cx)
            });
        }
        let key = key();
        let Some(mut tree) = self.shell.terminal_window_tree() else {
            return;
        };
        if !tree.contains_tab(&key) {
            return;
        }
        let current = self.active_workspace_tab();
        let preferred = tree.leaf_tab_ids_for_tab(&key).and_then(|tabs| {
            let index = tabs.iter().position(|tab| tab == &key)?;
            tabs[..index]
                .iter()
                .rev()
                .chain(tabs[index + 1..].iter())
                .find(|tab| tab.as_str() != key)
                .cloned()
        });
        let next = tree.remove_tab(&key);
        self.shell.restore_terminal_window_tree(next);
        if current.as_deref() == Some(key.as_str()) {
            if let Some(next) = preferred.or_else(|| self.workspace_tab_ids().first().cloned()) {
                self.activate_workspace_tab(&next, cx);
            } else {
                self.shell.workspace.focused_group = None;
            }
        }
        if self.shell.navigation.selected_nav == NavItem::Settings {
            self.shell.navigation.selected_nav = NavItem::Workspace;
        }
        self.persist_open_tabs();
        cx.notify();
    }

    /// Activation side of a settings tab click.
    pub(in crate::features) fn activate_settings_workspace_tab(&mut self, cx: &mut Context<Self>) {
        self.session.clear_active_start_selection();
        self.shell.workspace.focused_group = self.shell.sync_terminal_windows_active_tab(&key());
        self.shell.set_pane_focus_mode(false);
        self.shell.navigation.selected_nav = NavItem::Settings;
        let tab = self.shell.settings_active_tab();
        self.ensure_settings_tab_inputs(tab, cx);
        self.request_settings_panel_refresh(cx);
        self.shell.request_session_tab_scroll_into_view();
        self.close_action_link_menu(cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext as _, TestAppContext};
    use nyaterm_ui::NyaDialogWindowExt as _;

    use crate::features::test_support::app_with_visible_local_session;
    use crate::models::{NavItem, WorkspaceTabId};
    use crate::test_support::{TestAppCleanup, TestConfigDir};

    #[test]
    fn opening_settings_adds_a_tab_and_leaves_the_session_running() {
        let dir = TestConfigDir::new("nyaterm-settings-tab-open");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
        let key = WorkspaceTabId::settings_key();

        cx.update_entity(&app, |app, cx| {
            app.open_page(NavItem::Settings, cx);

            assert!(app.shell.has_settings_draft());
            assert!(
                app.workspace_tab_ids().iter().any(|tab| tab == &key),
                "settings must be an ordinary workspace tab"
            );
            assert_eq!(app.active_workspace_tab().as_deref(), Some(key.as_str()));
            assert!(
                app.session.has_session("local"),
                "opening settings must not touch the running session"
            );
            assert_eq!(
                app.serialize_open_tabs().len(),
                1,
                "the transient settings tab must not be persisted"
            );

            // Switching back to the session keeps the settings tab and draft.
            app.activate_workspace_tab("local", cx);
            assert_eq!(app.active_workspace_tab().as_deref(), Some("local"));
            assert!(app.shell.has_settings_draft());
            app.activate_workspace_tab(&key, cx);
            assert_eq!(app.active_workspace_tab().as_deref(), Some(key.as_str()));

            // Closing settings returns to the session tab and keeps it alive.
            app.cancel_settings(cx);
            assert!(!app.shell.has_settings_draft());
            assert!(!app.workspace_tab_ids().iter().any(|tab| tab == &key));
            assert_eq!(app.active_workspace_tab().as_deref(), Some("local"));
            assert!(app.session.has_session("local"));
        });
    }

    #[test]
    fn settings_tab_renders_in_the_workspace_strip_and_switches_back() {
        let dir = TestConfigDir::new("nyaterm-settings-tab-render");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
        let key = WorkspaceTabId::settings_key();
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.open_page(NavItem::Settings, cx);
        });
        let host_app = app.clone();
        let (_, vcx) = cx
            .add_window_view(move |window, cx| nyaterm_ui::nya_root(host_app.clone(), window, cx));
        let vcx: &mut gpui::VisualTestContext = vcx;
        let draw = |vcx: &mut gpui::VisualTestContext| {
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
        };
        for _ in 0..3 {
            draw(vcx);
        }

        assert!(
            vcx.debug_bounds("settings-tab").is_some(),
            "the settings tab must be painted in the workspace tab strip"
        );
        assert!(
            vcx.debug_bounds("settings-tab-active").is_some(),
            "the newly opened settings tab is the active tab"
        );
        let session_tab = vcx
            .debug_bounds("session-tab-local")
            .expect("the session tab keeps its place next to settings");
        vcx.simulate_click(session_tab.center(), gpui::Modifiers::default());
        draw(vcx);
        vcx.update(|_, cx| {
            assert_eq!(
                app.read(cx).active_workspace_tab(),
                Some("local".to_string())
            );
        });
        assert!(
            vcx.debug_bounds("settings-tab").is_some(),
            "switching away must not close the settings tab"
        );

        let settings_tab = vcx
            .debug_bounds("settings-tab")
            .expect("settings tab is still painted");
        vcx.simulate_click(settings_tab.center(), gpui::Modifiers::default());
        draw(vcx);
        vcx.update(|_, cx| {
            assert_eq!(
                app.read(cx).active_workspace_tab(),
                Some(WorkspaceTabId::settings_key())
            );
        });
        let settings_close = vcx
            .debug_bounds("settings-close")
            .expect("the settings surface paints its own close control");
        vcx.simulate_click(settings_close.center(), gpui::Modifiers::default());
        draw(vcx);
        vcx.update(|_, cx| {
            assert!(
                !app.read(cx)
                    .workspace_tab_ids()
                    .iter()
                    .any(|tab| tab == &key),
                "the in-panel close control must close the settings tab"
            );
            assert_eq!(
                app.read(cx).active_workspace_tab(),
                Some("local".to_string())
            );
        });
    }

    #[test]
    fn settings_tab_opens_next_to_the_current_tab_when_the_tree_is_new() {
        use nyaterm_core::AiExecutionProfile;
        use nyaterm_transport::LocalSessionConfig;

        use crate::models::{SessionLaunchConfig, SessionRuntimeMetadata};

        let dir = TestConfigDir::new("nyaterm-settings-tab-order");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
        cx.update_entity(&app, |app, cx| {
            app.session.register_session_metadata(
                "second",
                SessionRuntimeMetadata {
                    ssh_config: None,
                    ssh_multiplex_key: None,
                    source_connection_id: None,
                    ai_execution_profile: AiExecutionProfile::Posix,
                    launch_config: SessionLaunchConfig::Local(LocalSessionConfig {
                        name: "Second".to_string(),
                        ..LocalSessionConfig::default()
                    }),
                    disconnected: false,
                },
            );
            app.terminal
                .seed_session_view("second".to_string(), String::new(), "UTF-8");
            app.session.select_active_session("local".to_string());
            app.open_page(NavItem::Settings, cx);

            assert_eq!(
                app.workspace_tab_ids(),
                vec![
                    "local".to_string(),
                    WorkspaceTabId::settings_key(),
                    "second".to_string()
                ],
                "a newly created tree must place settings after the current tab"
            );
        });
    }

    #[test]
    fn theme_only_draft_gates_quit_and_closes_through_the_theme_prompt() {
        use crate::features::settings::theme_designer::ThemeIntent;

        let dir = TestConfigDir::new("nyaterm-settings-tab-theme-only");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
        let key = WorkspaceTabId::settings_key();
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.open_page(NavItem::Settings, cx);
            app.theme_designer_action(ThemeIntent::Copy("github-dark".into()), cx);
            app.edit_theme_designer("primary", "#123456".into(), cx);
            assert!(app.has_unconfirmed_theme_changes());
            assert!(
                !app.settings_draft_dirty(),
                "fixture: only the theme draft is dirty"
            );
        });
        let host_app = app.clone();
        let (_, vcx) = cx
            .add_window_view(move |window, cx| nyaterm_ui::nya_root(host_app.clone(), window, cx));
        let vcx: &mut gpui::VisualTestContext = vcx;
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.request_settings_tab_close(window, cx);
                assert_eq!(
                    app.settings.theme_designer().pending,
                    Some(ThemeIntent::CloseSettings),
                    "a theme-only close is deferred to the theme prompt, not recursed"
                );
                assert!(app.workspace_tab_ids().iter().any(|tab| tab == &key));

                app.resolve_theme_designer_pending(Some(false), cx);
                assert!(!app.has_unconfirmed_theme_changes());
                assert!(
                    !app.workspace_tab_ids().iter().any(|tab| tab == &key),
                    "discarding the theme draft closes the settings tab"
                );
                assert!(!app.shell.has_settings_draft());
                assert!(app.session.has_session("local"));
            });
        });
    }

    #[test]
    fn dirty_panel_close_opens_the_confirmation_from_the_rendered_surface() {
        let dir = TestConfigDir::new("nyaterm-settings-tab-panel-close");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
        let key = WorkspaceTabId::settings_key();
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.open_page(NavItem::Settings, cx);
            app.ensure_settings_tab_inputs(crate::models::SettingsTab::Appearance, cx);
            app.update_ui_font_family("Noto Sans", cx);
        });
        let host_app = app.clone();
        let (_, vcx) = cx
            .add_window_view(move |window, cx| nyaterm_ui::nya_root(host_app.clone(), window, cx));
        let vcx: &mut gpui::VisualTestContext = vcx;
        let draw = |vcx: &mut gpui::VisualTestContext| {
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
        };
        for _ in 0..3 {
            draw(vcx);
        }
        let close = vcx
            .debug_bounds("settings-close")
            .expect("the settings surface paints its own close control");
        vcx.simulate_click(close.center(), gpui::Modifiers::default());
        draw(vcx);
        vcx.update(|window, cx| {
            assert!(
                window.has_active_nya_dialog(cx),
                "the panel close control must ask before discarding edits"
            );
            assert!(
                app.read(cx)
                    .workspace_tab_ids()
                    .iter()
                    .any(|tab| tab == &key),
                "the tab stays while the confirmation is open"
            );
            assert!(app.read(cx).session.has_session("local"));
        });
    }

    #[test]
    fn dirty_settings_tab_close_asks_before_discarding() {
        let dir = TestConfigDir::new("nyaterm-settings-tab-close");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
        let key = WorkspaceTabId::settings_key();
        cx.update_entity(&app, |app, cx| app.sync_component_theme(cx));
        let host_app = app.clone();
        let (_, vcx) = cx
            .add_window_view(move |window, cx| nyaterm_ui::nya_root(host_app.clone(), window, cx));
        let vcx: &mut gpui::VisualTestContext = vcx;

        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_page(NavItem::Settings, cx);
                app.ensure_settings_tab_inputs(crate::models::SettingsTab::Appearance, cx);
                app.update_ui_font_family("Noto Sans", cx);
                assert!(app.settings_draft_dirty());

                app.request_settings_tab_close(window, cx);

                assert!(
                    window.has_active_nya_dialog(cx),
                    "a dirty settings tab must ask before discarding the draft"
                );
                assert!(
                    app.workspace_tab_ids().iter().any(|tab| tab == &key),
                    "the tab survives while the confirmation is open"
                );
                assert!(app.shell.has_settings_draft());
                assert!(
                    app.session.has_session("local"),
                    "a settings dialog must not tear down the running session"
                );
            });
        });
    }
}
