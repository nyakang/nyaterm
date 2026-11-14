use gpui::{
    Bounds, Context, Entity, IntoElement, Modifiers, MouseButton, Pixels, Point, Render,
    TestAppContext, VisualTestContext, Window, div, point, prelude::*, px,
};
use nyaterm_core::{AiExecutionProfile, test_support::TestTempDir};
use nyaterm_transport::LocalSessionConfig;

use crate::features::{NyaTermApp, test_support::app_with_visible_local_session};
use crate::models::{SessionLaunchConfig, SessionRuntimeMetadata, TabDockEdge, TabDockZone};

struct DockingFixture {
    app: Entity<NyaTermApp>,
}

impl Render for DockingFixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let workspace = self
            .app
            .update(cx, |app, cx| app.workspace_view(cx).into_any_element());
        div().size_full().flex().flex_col().child(workspace)
    }
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.run_until_parked();
}

fn setup(cx: &mut TestAppContext, root: &TestTempDir, edge: TabDockEdge) -> Entity<NyaTermApp> {
    let app = app_with_visible_local_session(cx, root.path(), "a");
    app.update(cx, |app, cx| {
        app.sync_component_theme(cx);
        app.session.mark_restore_complete();
        app.shell.set_workspace_pane_layout_restored(true);
        app.shell.complete_terminal_windows_restore();
        for id in ["b", "c"] {
            app.session.register_session_metadata(
                id,
                SessionRuntimeMetadata {
                    confirmed_ssh_runtime_mode: None,
                    pending_ssh_startup_command: None,
                    ssh_config: None,
                    ssh_multiplex_key: None,
                    source_connection_id: None,
                    ai_execution_profile: AiExecutionProfile::Posix,
                    launch_config: SessionLaunchConfig::Local(LocalSessionConfig::default()),
                    disconnected: false,
                },
            );
            app.terminal
                .seed_session_view(id.into(), String::new(), "UTF-8");
        }
        app.reconcile_terminal_windows();
        let first = app.shell.terminal_group_for_tab("a").unwrap();
        app.dock_tab_on_terminal_window_leaf("b".into(), first, TabDockZone::Edge(edge), cx);
        let second = app.shell.terminal_group_for_tab("b").unwrap();
        app.dock_tab_on_terminal_window_leaf("c".into(), second, TabDockZone::Center, cx);
    });
    app
}

fn content_bounds(
    app: &Entity<NyaTermApp>,
    session: &str,
    cx: &VisualTestContext,
) -> Bounds<Pixels> {
    app.read_with(cx, |app, _| {
        let group = app.shell.terminal_group_for_tab(session).unwrap();
        let mut bounds = app.shell.terminal_split_bounds(&group).unwrap();
        bounds.origin.y += px(36.);
        bounds.size.height -= px(36.);
        bounds
    })
}

fn start_drag(cx: &mut VisualTestContext) {
    let tab = cx.debug_bounds("session-tab-c").expect("visible c tab");
    let grab = tab.origin + point(px(20.), px(16.));
    cx.simulate_mouse_down(grab, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(
        grab + point(px(0.), px(12.)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    cx.read(|cx| assert!(cx.has_active_drag()));
    draw(cx);
}

fn hover(
    app: &Entity<NyaTermApp>,
    session: &str,
    position: Point<Pixels>,
    zone: TabDockZone,
    cx: &mut VisualTestContext,
) {
    cx.simulate_mouse_move(position, Some(MouseButton::Left), Modifiers::default());
    draw(cx);
    app.read_with(cx, |app, _| {
        let target = app.shell.terminal_group_for_tab(session).unwrap();
        for leaf in app.shell.terminal_window_tree().unwrap().leaf_ids() {
            assert_eq!(
                app.shell.terminal_window_drop_for_leaf(&leaf),
                (leaf == target).then_some(zone),
                "only the hovered group may own the drop preview"
            );
        }
    });
    assert_eq!(
        cx.debug_bounds("tab-dock-drop-overlay"),
        Some(content_bounds(app, session, cx)),
        "the preview must be rendered over the hovered content"
    );
}

#[test]
fn tab_drag_back_to_first_group_previews_and_splits_in_horizontal_vertical_and_nested_layouts() {
    for initial_edge in [TabDockEdge::Right, TabDockEdge::Bottom] {
        let root = TestTempDir::new("nyaterm-tab-dock-back");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = setup(&mut cx, &root, initial_edge);
        let fixture_app = app.clone();
        let (_, cx) = cx.add_window_view(move |_, cx| {
            cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
            DockingFixture { app: fixture_app }
        });
        draw(cx);

        // First exercise siblings, then repeat with c starting in a nested group.
        for nested in [false, true] {
            if nested {
                app.update(cx, |app, cx| {
                    let group = app.shell.terminal_group_for_tab("b").unwrap();
                    app.dock_tab_on_terminal_window_leaf(
                        "c".into(),
                        group,
                        TabDockZone::Edge(TabDockEdge::Top),
                        cx,
                    );
                });
                draw(cx);
            }
            start_drag(cx);
            let source = content_bounds(&app, "c", cx);
            hover(&app, "c", source.center(), TabDockZone::Center, cx);
            let target = content_bounds(&app, "a", cx);
            let position = point(target.center().x, target.origin.y + px(8.));
            hover(&app, "a", position, TabDockZone::Edge(TabDockEdge::Top), cx);
            cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
            draw(cx);
            app.read_with(cx, |app, _| {
                let tree = app.shell.terminal_window_tree().unwrap();
                assert_eq!(tree.leaf_ids().len(), 3);
                assert_ne!(tree.leaf_for_tab("a"), tree.leaf_for_tab("c"));
                let mut tabs = tree.collect_tab_ids();
                tabs.sort();
                assert_eq!(tabs, ["a", "b", "c"]);
            });

            // Returning to b's center must still merge and remove c's empty group.
            start_drag(cx);
            let position = content_bounds(&app, "b", cx).center();
            hover(&app, "b", position, TabDockZone::Center, cx);
            cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
            draw(cx);
            app.read_with(cx, |app, _| {
                let tree = app.shell.terminal_window_tree().unwrap();
                assert_eq!(tree.leaf_ids().len(), 2);
                assert_eq!(tree.leaf_for_tab("b"), tree.leaf_for_tab("c"));
            });
        }
    }
}

#[test]
fn tab_drag_clears_preview_outside_content_and_on_cancellation_then_can_merge_into_first_group() {
    let root = TestTempDir::new("nyaterm-tab-dock-clear");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = setup(&mut cx, &root, TabDockEdge::Right);
    let fixture_app = app.clone();
    let (_, cx) = cx.add_window_view(move |_, cx| {
        cx.observe(&fixture_app, |_, _, cx| cx.notify()).detach();
        DockingFixture { app: fixture_app }
    });
    draw(cx);
    start_drag(cx);
    let position = content_bounds(&app, "a", cx).center();
    hover(&app, "a", position, TabDockZone::Center, cx);
    let tab = cx.debug_bounds("session-tab-c").unwrap().center();
    cx.simulate_mouse_move(tab, Some(MouseButton::Left), Modifiers::default());
    draw(cx);
    app.read_with(cx, |app, _| {
        for leaf in app.shell.terminal_window_tree().unwrap().leaf_ids() {
            assert!(app.shell.terminal_window_drop_for_leaf(&leaf).is_none());
        }
    });
    assert!(cx.debug_bounds("tab-dock-drop-overlay").is_none());
    hover(&app, "a", position, TabDockZone::Center, cx);
    cx.update(|window, cx| {
        cx.stop_active_drag(window);
        app.update(cx, |app, cx| app.clear_drop_hover(cx));
    });
    draw(cx);
    app.read_with(cx, |app, _| {
        let tree = app.shell.terminal_window_tree().unwrap();
        assert_eq!(tree.leaf_ids().len(), 2);
        for leaf in tree.leaf_ids() {
            assert!(app.shell.terminal_window_drop_for_leaf(&leaf).is_none());
        }
    });
    assert!(cx.debug_bounds("tab-dock-drop-overlay").is_none());
    cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
    start_drag(cx);
    hover(&app, "a", position, TabDockZone::Center, cx);
    cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
    draw(cx);
    app.read_with(cx, |app, _| {
        let tree = app.shell.terminal_window_tree().unwrap();
        assert_eq!(tree.leaf_ids().len(), 2);
        assert_eq!(tree.leaf_for_tab("a"), tree.leaf_for_tab("c"));
    });
}
