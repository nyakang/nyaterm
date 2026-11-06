use std::collections::HashMap;

use gpui::{
    AppContext as _, Context, Entity, Focusable as _, IntoElement, Modifiers, Render,
    TestAppContext, VisualTestContext, Window, div, prelude::*, px,
};
use nyaterm_core::{AppRuntime, RuntimeMode, StartWorkspaceMode, test_support::TestTempDir};
use nyaterm_ui::nya_root;

use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
use crate::features::NyaTermApp;
use crate::features::test_support::app_with_visible_local_session;
use crate::models::{
    ActivityBarEntry, BottomPanelMode, NavItem, QuickCommandSortMode, WorkspaceTabId,
};
use crate::shortcuts::{ShortcutId, WORKSPACE_KEY_CONTEXT, rebuild_keymap};
use crate::test_support::TestAppCleanup;

struct QuickCommandsHost {
    app: Entity<NyaTermApp>,
    welcome: bool,
}

impl Render for QuickCommandsHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.app.update(cx, |app, cx| {
            let workspace = if self.welcome {
                app.empty_workspace_state(cx)
            } else if app.active_workspace_tab().as_deref() == Some(&WorkspaceTabId::settings_key())
            {
                div()
                    .flex_1()
                    .child(app.settings_view(cx))
                    .into_any_element()
            } else {
                div()
                    .track_focus(app.terminal.input_focus())
                    .flex_1()
                    .into_any_element()
            };
            let root = div()
                .id("quick-commands-shortcut-host")
                .key_context(WORKSPACE_KEY_CONTEXT)
                .w(px(900.))
                .h(px(700.))
                .flex()
                .flex_col()
                .child(workspace)
                .child(app.bottom_panel_view(cx));
            app.with_shortcut_action_handlers(root, cx)
        })
    }
}

fn window<'a>(
    cx: &'a mut TestAppContext,
    app: &Entity<NyaTermApp>,
    welcome: bool,
    intercept: bool,
    overrides: HashMap<String, String>,
) -> &'a mut VisualTestContext {
    app.update(cx, |app, cx| {
        app.sync_component_theme(cx);
        app.settings.set_keybindings(overrides.clone());
        rebuild_keymap(&overrides, cx);
    });
    let fixture_app = app.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
        let host = cx.new(|_| QuickCommandsHost {
            app: fixture_app,
            welcome,
        });
        nya_root(host, window, cx)
    });
    let handle = cx.update(|window, _| window.window_handle().downcast().unwrap());
    app.update(cx, |app, cx| {
        app.shell.remember_main_window(handle);
        if intercept {
            app.ensure_shortcut_interceptor(cx);
        }
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.focus_active_workspace_surface(window, cx));
    });
    draw(cx);
    cx
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}

#[test]
fn quick_commands_shortcut_toggles_once_from_search_and_preserves_filters_and_focus() {
    for intercept in [false, true] {
        for custom in [false, true] {
            let mut cx = TestAppContext::single();
            let _cleanup = TestAppCleanup::new(&cx);
            let root = TestTempDir::new("nyaterm-quick-commands-shortcut");
            let app = app_with_visible_local_session(&mut cx, root.path(), "local");
            app.update(&mut cx, |app, _| {
                app.set_bottom_panel_mode(BottomPanelMode::Hidden);
                app.shell.bottom_panel.quick_commands_height = 200.;
                app.commands.set_quick_search_draft("ps".into());
                app.commands.select_quick_category("uncategorized".into());
                app.commands.set_quick_sort_mode(QuickCommandSortMode::Name);
            });
            let shortcut = if custom {
                "ctrl-alt-u"
            } else if cfg!(target_os = "macos") {
                "cmd-shift-p"
            } else {
                "ctrl-shift-p"
            };
            let overrides = if custom {
                HashMap::from([(
                    ShortcutId::ShowAllCommands.as_str().into(),
                    "ctrl+alt+u".into(),
                )])
            } else {
                HashMap::new()
            };
            let cx = window(&mut cx, &app, false, intercept, overrides);

            cx.simulate_keystrokes(shortcut);
            draw(cx);
            let search = app.read_with(cx, |app, _| {
                assert_eq!(
                    app.shell.bottom_panel_mode(),
                    BottomPanelMode::QuickCommands
                );
                assert!(app.settings.summary().ui_quick_cmd_visible);
                app.existing_text_input("quick-command.search").unwrap()
            });
            cx.update(|window, cx| {
                assert!(
                    search
                        .read(cx)
                        .component_focus_handle(cx)
                        .is_focused(window)
                );
            });
            assert_eq!(search.read_with(cx, |input, cx| input.value(cx)), "ps");

            cx.simulate_input(" -ef");
            draw(cx);
            let query = app.read_with(cx, |app, _| app.commands.quick_search_draft().to_string());
            assert!(query.contains(" -ef"));
            cx.simulate_keystrokes(shortcut);
            draw(cx);
            app.read_with(cx, |app, _| {
                assert_eq!(app.shell.bottom_panel_mode(), BottomPanelMode::Hidden);
                assert!(!app.settings.summary().ui_quick_cmd_visible);
                assert_eq!(app.commands.quick_search_draft(), query);
                assert_eq!(app.commands.quick_selected_category(), "uncategorized");
                assert_eq!(app.commands.quick_sort_mode(), QuickCommandSortMode::Name);
                assert_eq!(app.shell.quick_commands_height(), 200.);
                assert_eq!(app.settings.summary().ui_quick_cmd_height, 200);
                assert_eq!(app.capture_workspace_ui_state().bottom_panel_mode, "hidden");
            });
            cx.update(|window, cx| {
                assert!(app.read(cx).terminal.input_focus().is_focused(window));
            });

            cx.simulate_keystrokes(shortcut);
            draw(cx);
            app.read_with(cx, |app, _| {
                assert_eq!(
                    app.shell.bottom_panel_mode(),
                    BottomPanelMode::QuickCommands
                );
                assert_eq!(app.commands.quick_search_draft(), query);
                assert_eq!(app.commands.quick_selected_category(), "uncategorized");
                assert_eq!(app.commands.quick_sort_mode(), QuickCommandSortMode::Name);
                assert_eq!(app.shell.quick_commands_height(), 200.);
                assert_eq!(
                    app.existing_text_input("quick-command.search").unwrap(),
                    search
                );
            });
            assert_eq!(search.read_with(cx, |input, cx| input.value(cx)), query);

            cx.simulate_keystrokes("escape");
            draw(cx);
            app.read_with(cx, |app, _| {
                assert_eq!(
                    app.shell.bottom_panel_mode(),
                    BottomPanelMode::QuickCommands
                );
                assert_eq!(app.commands.quick_search_draft(), "");
                assert_eq!(app.commands.quick_selected_category(), "all");
            });
        }
    }
}

#[test]
fn welcome_quick_commands_action_keeps_the_panel_open_on_repeated_clicks() {
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let root = TestTempDir::new("nyaterm-welcome-quick-commands");
    let runtime = AppRuntime::from_parts_for_test(
        RuntimeMode::Portable,
        root.path().to_path_buf(),
        root.path().join("config"),
        root.path().join("logs"),
        root.path().join("cache"),
        None,
    );
    let stores = UiStoreHandles {
        startup_restore: cx.new(|_| StartupRestoreStore::default()),
        overlays: cx.new(|_| OverlayStore::default()),
    };
    let app = cx.new(|cx| NyaTermApp::new(runtime, stores, cx));
    app.update(&mut cx, |app, _| {
        app.start_workspace.set_mode(StartWorkspaceMode::Workbench);
        app.set_bottom_panel_mode(BottomPanelMode::Hidden);
    });
    let cx = window(&mut cx, &app, true, true, HashMap::new());
    for _ in 0..2 {
        let action = cx.debug_bounds("empty-workspace-quick-commands").unwrap();
        cx.simulate_click(action.center(), Modifiers::default());
        draw(cx);
        app.read_with(cx, |app, _| {
            assert_eq!(
                app.shell.bottom_panel_mode(),
                BottomPanelMode::QuickCommands
            );
        });
        cx.update(|window, cx| {
            let search = app
                .read(cx)
                .existing_text_input("quick-command.search")
                .unwrap();
            assert!(
                search
                    .read(cx)
                    .component_focus_handle(cx)
                    .is_focused(window)
            );
        });
    }
    let shortcut = if cfg!(target_os = "macos") {
        "cmd-shift-p"
    } else {
        "ctrl-shift-p"
    };
    cx.simulate_keystrokes(shortcut);
    draw(cx);
    cx.update(|window, cx| {
        let app = app.read(cx);
        assert_eq!(app.shell.bottom_panel_mode(), BottomPanelMode::Hidden);
        assert!(app.terminal.input_focus().is_focused(window));
    });
    cx.simulate_keystrokes(shortcut);
    draw(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(
            app.shell.bottom_panel_mode(),
            BottomPanelMode::QuickCommands
        );
    });
}

#[test]
fn activity_bar_quick_commands_toggle_replaces_command_send_then_hides() {
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let root = TestTempDir::new("nyaterm-activity-quick-commands");
    let app = app_with_visible_local_session(&mut cx, root.path(), "local");
    app.update(&mut cx, |app, _| {
        app.set_bottom_panel_mode(BottomPanelMode::CommandSend)
    });
    let cx = window(&mut cx, &app, false, true, HashMap::new());
    for expected in [BottomPanelMode::QuickCommands, BottomPanelMode::Hidden] {
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.activate_activity_entry(ActivityBarEntry::QuickCommands, window, cx);
            });
        });
        draw(cx);
        app.read_with(cx, |app, _| {
            assert_eq!(app.shell.bottom_panel_mode(), expected);
            assert!(!app.settings.summary().ui_serial_send_visible);
        });
    }
    cx.update(|window, cx| {
        assert!(app.read(cx).terminal.input_focus().is_focused(window));
    });
}

#[test]
fn quick_commands_hide_restores_settings_workspace_focus_and_allows_reopening() {
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let root = TestTempDir::new("nyaterm-settings-quick-commands");
    let app = app_with_visible_local_session(&mut cx, root.path(), "local");
    app.update(&mut cx, |app, cx| {
        app.open_page(NavItem::Settings, cx);
        app.set_bottom_panel_mode(BottomPanelMode::Hidden);
    });
    let cx = window(&mut cx, &app, false, true, HashMap::new());
    let shortcut = if cfg!(target_os = "macos") {
        "cmd-shift-p"
    } else {
        "ctrl-shift-p"
    };
    cx.simulate_keystrokes(shortcut);
    draw(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(
            app.shell.bottom_panel_mode(),
            BottomPanelMode::QuickCommands
        );
    });
    cx.simulate_keystrokes(shortcut);
    draw(cx);
    cx.update(|window, cx| {
        let app = app.read(cx);
        assert_eq!(app.shell.bottom_panel_mode(), BottomPanelMode::Hidden);
        assert!(
            app.settings_panel
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );
    });
    cx.simulate_keystrokes(shortcut);
    draw(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(
            app.shell.bottom_panel_mode(),
            BottomPanelMode::QuickCommands
        );
    });
}
