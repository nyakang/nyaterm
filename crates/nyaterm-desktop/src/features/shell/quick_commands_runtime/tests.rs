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
    ActivityBarEntry, ActivityBarZone, BottomPanelMode, NavItem, QuickCommandSortMode,
    QuickCommandViewMode, WorkspaceTabId,
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

#[test]
fn command_bar_coexists_below_command_send_and_keeps_terminal_focus() {
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let root = TestTempDir::new("nyaterm-command-bar-coexist");
    let app = app_with_visible_local_session(&mut cx, root.path(), "local");
    app.update(&mut cx, |app, cx| {
        app.shell.bottom_panel.quick_commands_height = 224.;
        app.set_bottom_panel_mode(BottomPanelMode::CommandSend);
        app.set_quick_command_view_mode(QuickCommandViewMode::CommandBar, cx);
        app.shell.set_command_bar_visible(false);
    });
    let cx = window(&mut cx, &app, false, true, HashMap::new());

    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.toggle_quick_commands_panel(window, cx);
        });
    });
    draw(cx);

    cx.update(|window, cx| {
        let app_ref = app.read(cx);
        assert_eq!(
            app_ref.shell.bottom_panel_mode(),
            BottomPanelMode::CommandSend
        );
        assert!(app_ref.shell.command_bar_visible());
        assert!(app_ref.quick_commands_is_visible());
        assert_eq!(app_ref.shell.quick_commands_height(), 224.);
        assert!(app_ref.settings.summary().ui_serial_send_visible);
        assert!(app_ref.settings.summary().ui_quick_cmd_visible);
        // Command Bar opening preserves terminal focus rather than stealing it.
        assert!(app_ref.terminal.input_focus().is_focused(window));
    });

    let bar_bounds = cx
        .debug_bounds("quick-command-bar")
        .expect("command bar rendered");
    assert_eq!(f32::from(bar_bounds.size.height), 36.);

    // Closing via the Command Bar close button leaves Command Send open above it.
    let close_bounds = cx
        .debug_bounds("quick-command-bar-close")
        .expect("command bar close button");
    cx.simulate_click(close_bounds.center(), Modifiers::default());
    draw(cx);

    app.read_with(cx, |app, _| {
        assert!(!app.shell.command_bar_visible());
        assert!(!app.quick_commands_is_visible());
        assert_eq!(app.shell.bottom_panel_mode(), BottomPanelMode::CommandSend);
        assert!(app.settings.summary().ui_serial_send_visible);
        assert!(!app.settings.summary().ui_quick_cmd_visible);
    });
}

#[test]
fn switching_across_all_five_modes_preserves_state_and_prevents_duplicate_mounts() {
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let root = TestTempDir::new("nyaterm-quick-five-modes");
    let app = app_with_visible_local_session(&mut cx, root.path(), "local");
    app.update(&mut cx, |app, _| {
        app.shell.bottom_panel.quick_commands_height = 212.;
        app.commands.set_quick_search_draft("docker".into());
        app.commands.select_quick_category("uncategorized".into());
        app.commands.set_quick_sort_mode(QuickCommandSortMode::Name);
        app.set_bottom_panel_mode(BottomPanelMode::QuickCommands);
    });
    let cx = window(&mut cx, &app, false, true, HashMap::new());

    // Switch List/Tile -> CommandBar -> Sidebar -> Compact while visible.
    for mode in [
        QuickCommandViewMode::CommandBar,
        QuickCommandViewMode::Sidebar,
        QuickCommandViewMode::Compact,
    ] {
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.set_quick_command_view_mode_in_window(mode, window, cx);
            });
        });
        draw(cx);
        app.read_with(cx, |app, _| {
            assert_eq!(app.commands.quick_view_mode(), mode);
            assert!(app.quick_commands_is_visible());
            assert_eq!(app.commands.quick_search_draft(), "docker");
            assert_eq!(app.commands.quick_selected_category(), "uncategorized");
            assert_eq!(app.commands.quick_sort_mode(), QuickCommandSortMode::Name);
            assert_eq!(app.shell.quick_commands_height(), 212.);

            // Exactly one Quick Commands layout is mounted at any time.
            let in_bottom = app.shell.bottom_panel_mode() == BottomPanelMode::QuickCommands
                && app.commands.quick_view_mode().is_bottom_panel();
            let in_bar =
                app.commands.quick_view_mode().is_command_bar() && app.shell.command_bar_visible();
            let in_sidebar = app.panel_entry_selected(NavItem::QuickCommands);
            assert_eq!(
                usize::from(in_bottom) + usize::from(in_bar) + usize::from(in_sidebar),
                1
            );
        });
    }

    // Sidebar follows quickCmdBar between right and left rails and never closes Command Send.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.set_bottom_panel_mode(BottomPanelMode::CommandSend);
            app.set_quick_command_view_mode_in_window(QuickCommandViewMode::Sidebar, window, cx);
            app.open_quick_commands_panel(window, cx);
            assert_eq!(app.current_right_panel(), Some(NavItem::QuickCommands));
            assert_eq!(app.shell.bottom_panel_mode(), BottomPanelMode::CommandSend);

            app.move_activity_entry("quickCmdBar".into(), ActivityBarZone::LeftBottom, None, cx);
            app.open_quick_commands_panel(window, cx);
            assert_eq!(app.current_left_panel(), Some(NavItem::QuickCommands));
            assert_eq!(app.shell.bottom_panel_mode(), BottomPanelMode::CommandSend);
        });
    });
}

#[test]
fn overflow_popup_hover_card_copy_button_copies_script_without_closing_popup() {
    let mut test_cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&test_cx);
    let root = TestTempDir::new("nyaterm-command-bar-hover-copy");
    let app = app_with_visible_local_session(&mut test_cx, root.path(), "local");

    let commands: Vec<nyaterm_core::QuickCommand> = (0..12)
        .map(|index| nyaterm_core::QuickCommand {
            id: format!("cmd-{index}"),
            label: format!("移除文件中的 CR 字符与空白-{index}"),
            command: format!("sed -i \"s/\\r//g\" file-{index}.txt"),
            category_id: None,
            description: Some("用于去掉 Windows CR 字符".into()),
            color_tag: None,
            icon_tag: None,
            pinned: (index < 2).then_some(true),
            execution_mode: Some("append".into()),
            segmented_send: None,
            raw_send: None,
            source: None,
            risk_level: None,
            updated_at: None,
            created_at: None,
            use_count: None,
            sort_order: Some(index),
            extra: Default::default(),
        })
        .collect();

    app.update(&mut test_cx, |app, cx| {
        app.replace_quick_command_catalog(commands, Vec::new(), cx);
        app.commands
            .set_quick_sort_mode(QuickCommandSortMode::Custom);
        app.set_quick_command_view_mode(QuickCommandViewMode::CommandBar, cx);
        app.shell.set_command_bar_visible(true);
    });

    let cx = window(&mut test_cx, &app, false, true, HashMap::new());
    cx.update(|window, _| window.activate_window());
    draw(cx);

    let overflow_bounds = cx
        .debug_bounds("quick-command-bar-overflow")
        .expect("overflow trigger visible");
    cx.simulate_click(overflow_bounds.center(), Modifiers::default());
    draw(cx);
    assert!(app.read_with(cx, |app, _| app.commands.quick_command_bar_popup_is_open()));

    let popup_bounds = cx
        .debug_bounds("quick-command-bar-popup")
        .expect("overflow popup rendered");
    const ROW_SELECTORS: [&str; 12] = [
        "quick-command-bar-popup-row-cmd-0",
        "quick-command-bar-popup-row-cmd-1",
        "quick-command-bar-popup-row-cmd-2",
        "quick-command-bar-popup-row-cmd-3",
        "quick-command-bar-popup-row-cmd-4",
        "quick-command-bar-popup-row-cmd-5",
        "quick-command-bar-popup-row-cmd-6",
        "quick-command-bar-popup-row-cmd-7",
        "quick-command-bar-popup-row-cmd-8",
        "quick-command-bar-popup-row-cmd-9",
        "quick-command-bar-popup-row-cmd-10",
        "quick-command-bar-popup-row-cmd-11",
    ];
    let (row_bounds, expected_script) = ROW_SELECTORS
        .iter()
        .enumerate()
        .find_map(|(index, selector)| {
            cx.debug_bounds(selector)
                .map(|bounds| (bounds, format!("sed -i \"s/\\r//g\" file-{index}.txt")))
        })
        .expect("at least one overflow row rendered");

    cx.simulate_mouse_move(row_bounds.center(), None, Modifiers::default());
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(360));
    cx.run_until_parked();
    draw(cx);
    draw(cx);

    let hover_bounds = cx
        .debug_bounds("quick-command-hover-surface")
        .expect("hover card opened beside overflow popup");
    assert!(
        hover_bounds.right() <= popup_bounds.left(),
        "hover card sits outside the popup bounds on the left"
    );

    let copy_bounds = cx
        .debug_bounds("quick-command-tile-copy")
        .expect("copy button inside hover card");
    cx.simulate_mouse_move(copy_bounds.center(), None, Modifiers::default());
    cx.simulate_click(copy_bounds.center(), Modifiers::default());
    draw(cx);

    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some(expected_script)
    );
    assert!(
        app.read_with(cx, |app, _| app.commands.quick_command_bar_popup_is_open()),
        "clicking copy inside the hover card must not close the overflow popup"
    );

    // Clicking genuinely outside both surfaces dismisses the popup.
    cx.simulate_click(gpui::point(px(24.), px(24.)), Modifiers::default());
    draw(cx);
    assert!(!app.read_with(cx, |app, _| app.commands.quick_command_bar_popup_is_open()));
}

#[test]
fn sidebar_categories_default_to_collapsed_and_persist_expanded_ids_across_restart() {
    let shared_root = TestTempDir::new("nyaterm-sidebar-shared-config");
    let data_initial = TestTempDir::new("nyaterm-sidebar-data-initial");
    let data_restarted = TestTempDir::new("nyaterm-sidebar-data-restarted");
    let store_runtime = nyaterm_store::StoreRuntime::spawn(nyaterm_store::StoreConfig {
        config_dir: shared_root.path().join("config"),
        portable_key_path: None,
    })
    .expect("spawn initial store");

    {
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            data_initial.path().to_path_buf(),
            shared_root.path().join("config"),
            shared_root.path().join("logs"),
            shared_root.path().join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        let app = cx.new(|cx| NyaTermApp::new_with_test_store(runtime, stores, &store_runtime, cx));
        app.update(&mut cx, |app, cx| {
            let categories = vec![
                nyaterm_core::QuickCommandCategory {
                    id: "docker".into(),
                    name: "Docker".into(),
                    parent_id: None,
                    sort_order: 0,
                },
                nyaterm_core::QuickCommandCategory {
                    id: "k8s".into(),
                    name: "Kubernetes".into(),
                    parent_id: None,
                    sort_order: 1,
                },
            ];
            let commands = vec![nyaterm_core::QuickCommand {
                id: "c1".into(),
                label: "列出镜像".into(),
                command: "docker images".into(),
                category_id: Some("docker".into()),
                description: None,
                color_tag: None,
                icon_tag: None,
                pinned: Some(true),
                execution_mode: None,
                segmented_send: None,
                raw_send: None,
                source: None,
                risk_level: None,
                updated_at: None,
                created_at: None,
                use_count: None,
                sort_order: Some(0),
                extra: Default::default(),
            }];
            let seed_commands = commands.clone();
            let seed_categories = categories.clone();
            app.store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Commands, move |store| {
                    store.save_quick_commands(nyaterm_core::QuickCommandsConfig {
                        commands: seed_commands,
                        categories: seed_categories,
                    })
                })
                .expect("seed quick commands");
            app.replace_quick_command_catalog(commands, categories, cx);

            // Fresh default: all categories start collapsed rather than expanding everything.
            assert!(app.commands.quick_sidebar_expanded_categories().is_empty());
            app.set_quick_command_view_mode(QuickCommandViewMode::Sidebar, cx);
            app.toggle_quick_command_sidebar_category("docker".into(), cx);
            assert!(
                app.commands
                    .quick_sidebar_expanded_categories()
                    .contains("docker")
            );
            assert!(
                !app.commands
                    .quick_sidebar_expanded_categories()
                    .contains("k8s")
            );
        });
        // Drain both in-flight and coalesced follow-up settings saves on the FIFO store worker.
        for _ in 0..2 {
            cx.run_until_parked();
            let _ = cx.update_entity(&app, |app, _| {
                app.store_blocking_client()
                    .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                        store.load_app_settings_summary()
                    })
                    .expect("drain store queue")
            });
        }
        cx.run_until_parked();
        let flushed = cx.update_entity(&app, |app, _| {
            app.store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    store.load_app_settings_summary()
                })
                .expect("flush settings")
        });
        assert_eq!(flushed.ui_quick_cmd_view_mode, "sidebar");
        assert_eq!(
            flushed.ui_quick_cmd_sidebar_expanded_category_ids,
            vec!["docker".to_string()]
        );
    }

    // As in application shutdown, wait for the worker to release redb before reopening it.
    store_runtime.close_blocking().expect("close initial store");

    // Simulate application restart over the same config directory.
    let mut restart_cx = TestAppContext::single();
    let _restart_cleanup = TestAppCleanup::new(&restart_cx);
    let runtime = AppRuntime::from_parts_for_test(
        RuntimeMode::Portable,
        data_restarted.path().to_path_buf(),
        shared_root.path().join("config"),
        shared_root.path().join("logs"),
        shared_root.path().join("cache"),
        None,
    );
    let stores = UiStoreHandles {
        startup_restore: restart_cx.new(|_| StartupRestoreStore::default()),
        overlays: restart_cx.new(|_| OverlayStore::default()),
    };
    let restarted_app = restart_cx.new(|cx| NyaTermApp::new(runtime, stores, cx));
    restart_cx.run_until_parked();

    restarted_app.read_with(&restart_cx, |app, _| {
        assert_eq!(
            app.commands.quick_view_mode(),
            QuickCommandViewMode::Sidebar
        );
        assert_eq!(
            app.commands.quick_sidebar_expanded_category_ids_sorted(),
            vec!["docker".to_string()]
        );
    });
}
