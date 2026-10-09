use gpui::{
    AppContext as _, Context, Entity, IntoElement, Modifiers, MouseButton, MouseDownEvent,
    MouseUpEvent, Render, TestAppContext, VisualTestContext, Window, div, point, prelude::*, px,
};
use nyaterm_core::{
    AppRuntime, QuickCommand, QuickCommandCategory, RuntimeMode, test_support::TestTempDir,
};
use nyaterm_ui::nya_root;

use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
use crate::features::NyaTermApp;
use crate::models::QuickCommandViewMode;

struct QuickCommandsFixture {
    app: Entity<NyaTermApp>,
}

impl Render for QuickCommandsFixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let panel = self.app.update(cx, |app, cx| {
            app.quick_commands_panel(cx).into_any_element()
        });
        div().w(px(800.)).h(px(500.)).child(panel)
    }
}

fn command(id: &str, text: &str) -> QuickCommand {
    serde_json::from_value(serde_json::json!({
        "id": id, "label": id, "command": text,
    }))
    .unwrap()
}

fn app(cx: &mut TestAppContext, root: &TestTempDir) -> Entity<NyaTermApp> {
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
    app.update(cx, |app, cx| app.sync_component_theme(cx));
    app
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        _ = window.draw(cx);
    });
}

fn right_click(cx: &mut VisualTestContext, position: gpui::Point<gpui::Pixels>) {
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Right,
        position,
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    cx.simulate_event(MouseUpEvent {
        button: MouseButton::Right,
        position,
        modifiers: Modifiers::default(),
        click_count: 1,
    });
    draw(cx);
}

#[test]
fn quick_command_context_menu_copies_each_row_and_resets_to_blank_in_every_view() {
    for mode in [
        QuickCommandViewMode::List,
        QuickCommandViewMode::Compact,
        QuickCommandViewMode::Tile,
    ] {
        let root = TestTempDir::new("nyaterm-quick-command-context");
        let mut cx = TestAppContext::single();
        let app = app(&mut cx, &root);
        app.update(&mut cx, |app, _| {
            app.commands.replace_quick_command_catalog(
                vec![
                    command("first", "  printf 'first'\n"),
                    command("second", "pwd\nprintf 'second'"),
                ],
                vec![QuickCommandCategory {
                    id: "docker".into(),
                    name: "Docker".into(),
                    parent_id: None,
                    sort_order: 0,
                }],
            );
            app.commands.set_quick_view_mode(mode);
        });
        let fixture_app = app.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
            let fixture = cx.new(|_| QuickCommandsFixture { app: fixture_app });
            nya_root(fixture, window, cx)
        });
        draw(cx);

        for (selector, text) in [
            ("quick-command-row-first", "  printf 'first'\n"),
            ("quick-command-row-second", "pwd\nprintf 'second'"),
        ] {
            let row = cx.debug_bounds(selector).unwrap();
            right_click(cx, row.center());
            cx.simulate_keystrokes("down down enter");
            draw(cx);
            cx.update(|_, cx| {
                assert_eq!(
                    cx.read_from_clipboard()
                        .and_then(|item| item.text())
                        .as_deref(),
                    Some(text),
                    "{mode:?}: copied the wrong row"
                );
            });
        }

        // Select without rendering first: the action must use current state, and
        // the blank-area capture must clear the previous row's menu target.
        app.update(cx, |app, _| {
            app.commands.select_quick_category("docker".into())
        });
        let pane = cx.debug_bounds("quick-command-pane").unwrap();
        right_click(cx, point(pane.center().x, pane.bottom() - px(20.)));
        cx.simulate_keystrokes("down enter");
        draw(cx);
        app.read_with(cx, |app, _| {
            let editor = app
                .commands
                .quick_editor()
                .expect("blank menu should create a command");
            assert_eq!(editor.category_id.as_deref(), Some("docker"));
            assert!(editor.command.is_empty());
        });
    }
}

#[test]
fn quick_command_toolbar_and_empty_state_create_in_an_empty_selected_category() {
    for selector in ["quick-command-add", "quick-command-empty-add"] {
        let root = TestTempDir::new("nyaterm-quick-command-toolbar-category");
        let mut cx = TestAppContext::single();
        let app = app(&mut cx, &root);
        app.update(&mut cx, |app, _| {
            app.commands.replace_quick_command_catalog(
                vec![command("elsewhere", "pwd")],
                vec![QuickCommandCategory {
                    id: "docker".into(),
                    name: "Docker".into(),
                    parent_id: None,
                    sort_order: 0,
                }],
            );
            app.commands.select_quick_category("docker".into());
        });
        let fixture_app = app.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let fixture = cx.new(|_| QuickCommandsFixture { app: fixture_app });
            nya_root(fixture, window, cx)
        });
        draw(cx);
        let button = cx
            .debug_bounds(selector)
            .expect("empty category should offer creation even with commands elsewhere");
        cx.simulate_click(button.center(), Modifiers::default());
        draw(cx);
        app.read_with(cx, |app, _| {
            assert_eq!(
                app.commands.quick_editor().unwrap().category_id.as_deref(),
                Some("docker")
            );
        });
    }
}

#[test]
fn quick_command_category_blank_menu_creates_a_root_category() {
    let root = TestTempDir::new("nyaterm-quick-command-category-blank");
    let mut cx = TestAppContext::single();
    let app = app(&mut cx, &root);
    app.update(&mut cx, |app, _| {
        app.commands.replace_quick_command_catalog(
            Vec::new(),
            vec![QuickCommandCategory {
                id: "docker".into(),
                name: "Docker".into(),
                parent_id: None,
                sort_order: 0,
            }],
        );
        app.commands.select_quick_category("docker".into());
    });
    let fixture_app = app.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let fixture = cx.new(|_| QuickCommandsFixture { app: fixture_app });
        nya_root(fixture, window, cx)
    });
    draw(cx);
    let blank = cx.debug_bounds("quick-command-category-blank").unwrap();
    right_click(cx, blank.center());
    cx.simulate_keystrokes("down enter");
    draw(cx);
    app.read_with(cx, |app, _| {
        let create = app
            .commands
            .quick_category_create()
            .expect("blank sidebar should open category creation");
        assert_eq!(create.parent_id, None);
    });
}

fn category(id: &str, parent: Option<&str>, order: i32) -> QuickCommandCategory {
    QuickCommandCategory {
        id: id.into(),
        name: id.into(),
        parent_id: parent.map(str::to_string),
        sort_order: order,
    }
}

fn host<'a>(cx: &'a mut TestAppContext, app: &Entity<NyaTermApp>) -> &'a mut VisualTestContext {
    let fixture_app = app.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
        let fixture = cx.new(|_| QuickCommandsFixture { app: fixture_app });
        nya_root(fixture, window, cx)
    });
    draw(cx);
    cx
}

fn drag(
    cx: &mut VisualTestContext,
    source: gpui::Point<gpui::Pixels>,
    target: gpui::Point<gpui::Pixels>,
) {
    cx.simulate_mouse_down(source, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        source + point(px(8.), px(0.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_move(target, MouseButton::Left, Modifiers::none());
    draw(cx);
}

#[test]
fn category_drag_tracks_only_the_hovered_row_and_shows_before_after_inside() {
    use crate::features::commands::QuickCommandDropPosition;
    for (fraction, position, expected_parent) in [
        (0.1, QuickCommandDropPosition::Before, None),
        (0.9, QuickCommandDropPosition::After, None),
        (0.5, QuickCommandDropPosition::Inside, Some("peer")),
    ] {
        let root = TestTempDir::new("nyaterm-category-drag");
        let mut cx = TestAppContext::single();
        let app = app(&mut cx, &root);
        app.update(&mut cx, |app, _| {
            app.commands.replace_quick_command_catalog(
                vec![command("cmd", "pwd")],
                vec![
                    category("root", None, 0),
                    category("child", Some("root"), 0),
                    category("peer", None, 1),
                    category("last", None, 2),
                ],
            )
        });
        let cx = host(&mut cx, &app);
        let source = cx
            .debug_bounds("quick-command-category-child")
            .unwrap()
            .center();
        let bounds = cx.debug_bounds("quick-command-category-peer").unwrap();
        let target = point(source.x, bounds.top() + bounds.size.height * fraction);
        drag(cx, source, target);
        app.read_with(cx, |app, _| {
            let target = app.commands.quick_drop_target().unwrap();
            assert_eq!(target.id, "peer");
            assert_eq!(target.position, position);
        });
        if position != QuickCommandDropPosition::Inside {
            assert!(
                cx.debug_bounds("quick-command-category-insertion-peer")
                    .is_some()
            );
            assert!(
                cx.debug_bounds("quick-command-category-insertion-last")
                    .is_none()
            );
        }
        cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::none());
        draw(cx);
        app.read_with(cx, |app, _| {
            let child = app
                .commands
                .quick_command_categories()
                .iter()
                .find(|item| item.id == "child")
                .unwrap();
            assert_eq!(child.parent_id.as_deref(), expected_parent);
            assert_eq!(
                child.sort_order,
                match position {
                    QuickCommandDropPosition::Before => 1,
                    QuickCommandDropPosition::After => 2,
                    QuickCommandDropPosition::Inside => 0,
                }
            );
            assert!(app.commands.quick_drop_target().is_none());
        });
    }
}

#[test]
fn category_drag_to_all_moves_to_root_and_keeps_children() {
    let root = TestTempDir::new("nyaterm-category-root-drag");
    let mut cx = TestAppContext::single();
    let app = app(&mut cx, &root);
    app.update(&mut cx, |app, _| {
        app.commands.replace_quick_command_catalog(
            Vec::new(),
            vec![
                category("root", None, 0),
                category("child", Some("root"), 0),
                category("leaf", Some("child"), 0),
            ],
        )
    });
    let cx = host(&mut cx, &app);
    let source = cx
        .debug_bounds("quick-command-category-child")
        .unwrap()
        .center();
    let target = cx
        .debug_bounds("quick-command-category-all")
        .unwrap()
        .center();
    drag(cx, source, target);
    app.read_with(cx, |app, _| {
        assert_eq!(app.commands.quick_drop_target().unwrap().id, "all")
    });
    assert!(
        cx.debug_bounds("quick-command-category-drop-hint")
            .is_some()
    );
    cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::none());
    draw(cx);
    app.read_with(cx, |app, _| {
        let categories = app.commands.quick_command_categories();
        assert!(app.commands.quick_drop_target().is_none());
        assert_eq!(
            categories
                .iter()
                .find(|item| item.id == "child")
                .unwrap()
                .parent_id,
            None
        );
        assert_eq!(
            categories
                .iter()
                .find(|item| item.id == "leaf")
                .unwrap()
                .parent_id
                .as_deref(),
            Some("child")
        );
    });
    assert!(
        cx.debug_bounds("quick-command-category-drop-hint")
            .is_none()
    );
    let stored = app.update(cx, |app, _| {
        app.store_blocking_client()
            .request_fn(nyaterm_store::StoreDomain::Commands, |store| {
                store.load_quick_commands()
            })
            .unwrap()
    });
    assert_eq!(
        stored
            .categories
            .iter()
            .find(|item| item.id == "child")
            .unwrap()
            .parent_id,
        None
    );
}

#[test]
fn category_drag_rejects_descendants_and_clears_marks_on_leave_and_cancel() {
    let root = TestTempDir::new("nyaterm-category-drag-cancel");
    let mut cx = TestAppContext::single();
    let app = app(&mut cx, &root);
    app.update(&mut cx, |app, _| {
        app.commands.replace_quick_command_catalog(
            Vec::new(),
            vec![
                category("root", None, 0),
                category("child", Some("root"), 0),
                category("peer", None, 1),
            ],
        )
    });
    let cx = host(&mut cx, &app);
    let source = cx
        .debug_bounds("quick-command-category-root")
        .unwrap()
        .center();
    let invalid = cx
        .debug_bounds("quick-command-category-child")
        .unwrap()
        .center();
    drag(cx, source, invalid);
    app.read_with(cx, |app, _| {
        assert!(app.commands.quick_drop_target().is_none())
    });
    let target = cx
        .debug_bounds("quick-command-category-peer")
        .unwrap()
        .center();
    cx.simulate_mouse_move(target, MouseButton::Left, Modifiers::none());
    draw(cx);
    app.read_with(cx, |app, _| {
        assert!(app.commands.quick_drop_target().is_some())
    });
    cx.simulate_mouse_move(
        point(px(790.), px(490.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    draw(cx);
    app.read_with(cx, |app, _| {
        assert!(app.commands.quick_drop_target().is_none())
    });
    cx.simulate_mouse_move(target, MouseButton::Left, Modifiers::none());
    cx.update(|window, cx| {
        assert!(cx.stop_active_drag(window));
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(32));
    draw(cx);
    app.read_with(cx, |app, _| {
        assert!(app.commands.quick_drop_target().is_none())
    });
}

#[test]
fn category_rename_resets_cached_input_and_retires_it_on_cancel_and_confirm() {
    use crate::features::text_inputs::TextInputSetup;
    let root = TestTempDir::new("nyaterm-category-rename");
    let mut cx = TestAppContext::single();
    let app = app(&mut cx, &root);
    app.update(&mut cx, |app, _| {
        app.commands.replace_quick_command_catalog(
            Vec::new(),
            vec![category("first", None, 0), category("second", None, 1)],
        );
        let config = app.commands.quick_command_config();
        app.store_blocking_client()
            .request_fn(nyaterm_store::StoreDomain::Commands, move |store| {
                store.save_quick_commands(config)
            })
            .unwrap();
    });
    let cx = host(&mut cx, &app);
    let input = app.update(cx, |app, cx| {
        app.text_input(
            "quick-command.category-rename",
            "stale",
            TextInputSetup::default(),
            cx,
        )
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_rename_quick_command_category("first".into(), window, cx)
        })
    });
    draw(cx);
    cx.read(|cx| assert_eq!(input.read(cx).value(cx), "first"));
    // Reopen directly to verify reset even if a previous lifecycle left a cache.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_rename_quick_command_category("second".into(), window, cx)
        })
    });
    draw(cx);
    cx.read(|cx| assert_eq!(input.read(cx).value(cx), "second"));
    app.update(cx, |app, cx| app.cancel_rename_quick_command_category(cx));
    let fresh = app.update(cx, |app, cx| {
        app.text_input(
            "quick-command.category-rename",
            "fresh",
            TextInputSetup::default(),
            cx,
        )
    });
    assert_ne!(fresh.entity_id(), input.entity_id());
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_rename_quick_command_category("first".into(), window, cx)
        })
    });
    draw(cx);
    app.update(cx, |app, cx| {
        app.commands.apply_quick_category_rename(String::new());
        assert!(!app.confirm_rename_quick_command_category(cx));
        let retained = app.text_input(
            "quick-command.category-rename",
            "ignored",
            TextInputSetup::default(),
            cx,
        );
        assert_eq!(retained.entity_id(), fresh.entity_id());
        app.commands.apply_quick_category_rename("renamed".into());
        assert!(app.confirm_rename_quick_command_category(cx));
        assert!(app.commands.quick_category_rename().is_none());
        let next = app.text_input(
            "quick-command.category-rename",
            "next",
            TextInputSetup::default(),
            cx,
        );
        assert_ne!(next.entity_id(), fresh.entity_id());
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_rename_quick_command_category("second".into(), window, cx)
        })
    });
    // Wait for the previous save, then deliver its completion to the newly opened form.
    app.update(cx, |app, _| {
        app.store_blocking_client()
            .request_fn(nyaterm_store::StoreDomain::Commands, |store| {
                store.load_quick_commands()
            })
            .unwrap()
    });
    draw(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.commands.quick_category_rename().unwrap().id, "second")
    });
}

#[test]
fn category_menu_outdents_one_level_at_a_time_and_persists() {
    let root = TestTempDir::new("nyaterm-category-menu-outdent");
    let mut cx = TestAppContext::single();
    let app = app(&mut cx, &root);
    app.update(&mut cx, |app, _| {
        app.commands.replace_quick_command_catalog(
            Vec::new(),
            vec![
                category("a", None, 0),
                category("b", Some("a"), 0),
                category("c", Some("b"), 0),
                category("leaf", Some("c"), 0),
            ],
        )
    });
    let cx = host(&mut cx, &app);
    for (keys, parent) in [
        ("down down down enter", Some("a")),
        ("down down down down enter", None),
    ] {
        let row = cx.debug_bounds("quick-command-category-c").unwrap();
        right_click(cx, row.center());
        cx.simulate_keystrokes(keys);
        draw(cx);
        app.read_with(cx, |app, _| {
            let categories = app.commands.quick_command_categories();
            assert_eq!(
                categories
                    .iter()
                    .find(|item| item.id == "c")
                    .unwrap()
                    .parent_id
                    .as_deref(),
                parent
            );
            assert_eq!(
                categories
                    .iter()
                    .find(|item| item.id == "leaf")
                    .unwrap()
                    .parent_id
                    .as_deref(),
                Some("c")
            );
        });
    }
    let stored = app.update(cx, |app, _| {
        app.store_blocking_client()
            .request_fn(nyaterm_store::StoreDomain::Commands, |store| {
                store.load_quick_commands()
            })
            .unwrap()
    });
    assert_eq!(
        stored
            .categories
            .iter()
            .find(|item| item.id == "c")
            .unwrap()
            .parent_id,
        None
    );
}

#[test]
fn command_drop_near_category_edges_always_means_inside() {
    use crate::features::commands::QuickCommandDropPosition;
    for fraction in [0.1, 0.9] {
        let root = TestTempDir::new("nyaterm-command-category-drag");
        let mut cx = TestAppContext::single();
        let app = app(&mut cx, &root);
        app.update(&mut cx, |app, _| {
            app.commands.replace_quick_command_catalog(
                vec![command("cmd", "pwd")],
                vec![category("root", None, 0), category("peer", None, 1)],
            )
        });
        let cx = host(&mut cx, &app);
        let source = cx.debug_bounds("quick-command-row-cmd").unwrap().center();
        let bounds = cx.debug_bounds("quick-command-category-root").unwrap();
        let target = point(
            bounds.center().x,
            bounds.top() + bounds.size.height * fraction,
        );
        drag(cx, source, target);
        app.read_with(cx, |app, _| {
            let drop = app.commands.quick_drop_target().unwrap();
            assert_eq!(drop.id, "root");
            assert_eq!(drop.position, QuickCommandDropPosition::Inside);
        });
        cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::none());
        draw(cx);
        app.read_with(cx, |app, _| {
            assert_eq!(
                app.commands.quick_commands()[0].category_id.as_deref(),
                Some("root")
            )
        });
    }
}

#[test]
fn category_after_marker_follows_the_whole_subtree_and_root_target_stays_visible() {
    let root = TestTempDir::new("nyaterm-category-tree-indicator");
    let mut cx = TestAppContext::single();
    let app = app(&mut cx, &root);
    app.update(&mut cx, |app, _| {
        let mut categories = vec![
            category("root", None, 0),
            category("child", Some("root"), 0),
            category("peer", None, 1),
        ];
        categories.extend((2..24).map(|index| category(&format!("extra-{index}"), None, index)));
        app.commands
            .replace_quick_command_catalog(Vec::new(), categories);
    });
    let cx = host(&mut cx, &app);
    let all = cx.debug_bounds("quick-command-category-all").unwrap();
    assert!(
        all.top() > px(0.) && all.bottom() <= px(500.),
        "root target must stay in the visible viewport"
    );
    let source = cx
        .debug_bounds("quick-command-category-peer")
        .unwrap()
        .center();
    let row = cx.debug_bounds("quick-command-category-root").unwrap();
    let target = point(row.center().x, row.bottom() - px(2.));
    drag(cx, source, target);
    let line = cx
        .debug_bounds("quick-command-category-insertion-child")
        .unwrap();
    let child = cx.debug_bounds("quick-command-category-child").unwrap();
    assert_eq!(line.bottom(), child.bottom());
    assert!(
        cx.debug_bounds("quick-command-category-insertion-root")
            .is_none()
    );
    // "All" remains pinned when the tree is scrolled during a drag.
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: child.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-200.))),
        ..Default::default()
    });
    draw(cx);
    assert_eq!(cx.debug_bounds("quick-command-category-all").unwrap(), all);
    assert!(
        cx.debug_bounds("quick-command-category-root")
            .unwrap()
            .top()
            < row.top()
    );
    let target = all.center();
    cx.simulate_mouse_move(target, MouseButton::Left, Modifiers::none());
    draw(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.commands.quick_drop_target().unwrap().id, "all")
    });
    cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::none());
    draw(cx);
}

#[test]
fn all_category_drop_target_clears_on_leave_and_cancel() {
    let root = TestTempDir::new("nyaterm-category-all-cancel");
    let mut cx = TestAppContext::single();
    let app = app(&mut cx, &root);
    app.update(&mut cx, |app, _| {
        app.commands.replace_quick_command_catalog(
            Vec::new(),
            vec![
                category("root", None, 0),
                category("child", Some("root"), 0),
            ],
        );
    });
    let cx = host(&mut cx, &app);
    let source = cx
        .debug_bounds("quick-command-category-child")
        .unwrap()
        .center();
    let target = cx
        .debug_bounds("quick-command-category-all")
        .unwrap()
        .center();
    drag(cx, source, target);
    cx.simulate_mouse_move(
        point(px(790.), px(490.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    draw(cx);
    app.read_with(cx, |app, _| {
        assert!(app.commands.quick_drop_target().is_none())
    });
    assert!(
        cx.debug_bounds("quick-command-category-drop-hint")
            .is_none()
    );
    cx.simulate_mouse_move(target, MouseButton::Left, Modifiers::none());
    draw(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.commands.quick_drop_target().unwrap().id, "all")
    });
    cx.update(|window, cx| assert!(cx.stop_active_drag(window)));
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(32));
    draw(cx);
    app.read_with(cx, |app, _| {
        assert!(app.commands.quick_drop_target().is_none());
        assert_eq!(
            app.commands.quick_command_categories()[1]
                .parent_id
                .as_deref(),
            Some("root")
        );
    });
    assert!(
        cx.debug_bounds("quick-command-category-drop-hint")
            .is_none()
    );
}

#[test]
fn command_drag_to_all_keeps_category_but_uncategorized_removes_it() {
    let root = TestTempDir::new("nyaterm-command-all-drag");
    let mut cx = TestAppContext::single();
    let app = app(&mut cx, &root);
    app.update(&mut cx, |app, _| {
        let mut cmd = command("cmd", "pwd");
        cmd.category_id = Some("root".into());
        app.commands
            .replace_quick_command_catalog(vec![cmd], vec![category("root", None, 0)]);
    });
    let cx = host(&mut cx, &app);
    for (selector, expected_category) in [
        ("quick-command-category-all", Some("root")),
        ("quick-command-category-uncategorized", None),
    ] {
        let source = cx.debug_bounds("quick-command-row-cmd").unwrap().center();
        let target = cx.debug_bounds(selector).unwrap().center();
        drag(cx, source, target);
        app.read_with(cx, |app, _| {
            assert!(app.commands.quick_drop_target().is_none())
        });
        assert!(
            cx.debug_bounds("quick-command-category-drop-hint")
                .is_none()
        );
        cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::none());
        draw(cx);
        app.read_with(cx, |app, _| {
            assert_eq!(
                app.commands.quick_commands()[0].category_id.as_deref(),
                expected_category
            );
        });
    }
}
