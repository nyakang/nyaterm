use gpui::{
    AppContext as _, Context, Entity, FocusHandle, IntoElement, Modifiers, Render, ScrollDelta,
    ScrollEvent, TestAppContext, VisualTestContext, Window, div, point, prelude::*, px, size,
};
use nyaterm_core::{AiExecutionProfile, test_support::TestTempDir};
use nyaterm_transport::LocalSessionConfig;
use nyaterm_ui::{NyaDialogWindowExt as _, NyaWindowHandle, nya_root};

use crate::features::NyaTermApp;
use crate::features::test_support::app_with_visible_local_session;
use crate::features::view_widgets::init_child_window_key_bindings;
use crate::models::{SessionLaunchConfig, SessionRuntimeMetadata};

struct ParentHost(FocusHandle);

impl Render for ParentHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().track_focus(&self.0)
    }
}

fn parent_window(cx: &mut TestAppContext, app: &Entity<NyaTermApp>) -> NyaWindowHandle {
    cx.update(init_child_window_key_bindings);
    app.update(cx, |app, cx| app.sync_component_theme(cx));
    let focus = app.read_with(cx, |app, _| app.terminal.input_focus().clone());
    let handle = cx.add_window(move |window, cx| {
        window.focus(&focus, cx);
        let host = cx.new(|_| ParentHost(focus));
        nya_root(host, window, cx)
    });
    app.update(cx, |app, _| app.shell.remember_main_window(handle));
    handle
}

fn open_groups(
    cx: &mut TestAppContext,
    app: &Entity<NyaTermApp>,
    parent: NyaWindowHandle,
) -> VisualTestContext {
    parent
        .update(cx, |_, window, cx| {
            app.update(cx, |app, cx| app.open_sync_groups(window, cx));
        })
        .unwrap();
    cx.run_until_parked();
    let handle = app.read_with(cx, |app, _| app.sync_input.window.handle().unwrap());
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    draw(&mut visual);
    visual
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    });
    cx.run_until_parked();
}

fn scroll_to_end(cx: &mut VisualTestContext, selector: &'static str) {
    let viewport = cx.debug_bounds(selector).unwrap();
    cx.simulate_mouse_move(viewport.center(), None, Modifiers::default());
    cx.simulate_event(ScrollEvent {
        position: viewport.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-10000.))),
        ..Default::default()
    });
    draw(cx);
}

#[gpui::test]
fn session_lists_scroll_to_last_row_without_moving_search_or_footer(cx: &mut TestAppContext) {
    let _cleanup = crate::test_support::TestAppCleanup::new(cx);
    let dir = TestTempDir::new("nyaterm-sync-groups-scrolling");
    let app = app_with_visible_local_session(cx, dir.path(), "session-0");
    app.update(cx, |app, _| {
        for index in 1..24 {
            app.session.register_session_metadata(
                &format!("session-{index}"),
                SessionRuntimeMetadata {
                    ssh_config: None,
                    ssh_multiplex_key: None,
                    source_connection_id: None,
                    ai_execution_profile: AiExecutionProfile::Posix,
                    launch_config: SessionLaunchConfig::Local(LocalSessionConfig {
                        name: format!("Session {index}"),
                        ..Default::default()
                    }),
                    disconnected: false,
                },
            );
        }
    });
    let parent = parent_window(cx, &app);
    let mut visual = open_groups(cx, &app, parent);
    let handle = app.read_with(cx, |app, _| app.sync_input.window.handle().unwrap());

    for dimensions in [size(px(960.), px(640.)), size(px(760.), px(480.))] {
        visual.simulate_window_resize(handle.into(), dimensions);
        draw(&mut visual);
        assert!(visual.debug_bounds("sync-group-header").is_none());
        let search = visual.debug_bounds("sync-sessions-search").unwrap();
        let heading = visual.debug_bounds("sync-groups-heading").unwrap();
        assert_eq!(
            search.top(),
            heading.top(),
            "empty group header reserves no row"
        );
        let footer = visual.debug_bounds("sync-sessions-footer").unwrap();
        let viewport = visual.debug_bounds("sync-sessions-viewport").unwrap();
        assert!(viewport.size.height > px(100.));
        assert!(viewport.top() >= search.bottom());
        assert!(viewport.bottom() <= footer.top());
        assert!(footer.bottom() <= dimensions.height);
        scroll_to_end(&mut visual, "sync-sessions-viewport");
        let last = visual.debug_bounds("sync-session-session-23").unwrap();
        assert!(last.top() >= viewport.top());
        assert!(last.bottom() <= viewport.bottom() + px(1.));
        assert_eq!(visual.debug_bounds("sync-sessions-search").unwrap(), search);
        assert_eq!(visual.debug_bounds("sync-sessions-footer").unwrap(), footer);
    }

    app.update(cx, |app, cx| {
        for _ in 0..24 {
            app.create_sync_group(cx);
        }
    });
    draw(&mut visual);
    assert!(visual.debug_bounds("sync-group-header").is_some());
    let viewport = visual.debug_bounds("sync-groups-viewport").unwrap();
    assert!(
        visual
            .debug_bounds("sync-group-selected-row")
            .unwrap()
            .bottom()
            > viewport.bottom()
    );
    scroll_to_end(&mut visual, "sync-groups-viewport");
    let viewport = visual.debug_bounds("sync-groups-viewport").unwrap();
    let last = visual.debug_bounds("sync-group-selected-row").unwrap();
    assert!(last.top() >= viewport.top());
    assert!(last.bottom() <= viewport.bottom() + px(1.));
    let last_id = app.read_with(cx, |app, _| {
        app.sync_input.selected_group().unwrap().id.clone()
    });
    // Clicking the last visible group also proves the scrolled row stays interactive.
    visual.simulate_click(
        point(viewport.center().x, viewport.bottom() - px(20.)),
        Modifiers::default(),
    );
    assert_eq!(
        app.read_with(cx, |app, _| app
            .sync_input
            .selected_group()
            .unwrap()
            .id
            .clone()),
        last_id
    );
}

#[gpui::test]
fn repeated_open_and_close_reuse_one_window_and_restore_parent_focus(cx: &mut TestAppContext) {
    let _cleanup = crate::test_support::TestAppCleanup::new(cx);
    let dir = TestTempDir::new("nyaterm-sync-groups-lifecycle");
    let app = app_with_visible_local_session(cx, dir.path(), "session-0");
    let parent = parent_window(cx, &app);
    parent
        .update(cx, |_, window, cx| {
            let menu_focus = cx.focus_handle();
            window.focus(&menu_focus, cx);
            app.update(cx, |app, cx| {
                app.open_sync_groups(window, cx);
                app.open_sync_groups(window, cx);
            });
            let workspace_focus = app.read(cx).terminal.input_focus().clone();
            window.focus(&workspace_focus, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.windows().len(), 2);
    let handle = app.read_with(cx, |app, _| app.sync_input.window.handle().unwrap());
    let mut visual = open_groups(cx, &app, parent);
    assert_eq!(
        app.read_with(cx, |app, _| app.sync_input.window.handle()),
        Some(handle)
    );
    app.update(cx, |app, cx| {
        app.create_sync_group(cx);
        app.toggle_session_in_selected_sync_group("session-0".into(), cx);
    });
    visual.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!app.read_with(cx, |app, _| app.sync_input.window_is_open_or_pending()));
    assert_eq!(cx.windows().len(), 1);
    parent
        .update(cx, |_, window, cx| {
            assert!(app.read(cx).terminal.input_focus().is_focused(window));
        })
        .unwrap();
    visual = open_groups(cx, &app, parent);
    assert_ne!(
        app.read_with(cx, |app, _| app.sync_input.window.handle()),
        Some(handle)
    );
    assert_eq!(
        app.read_with(cx, |app, _| app
            .sync_input
            .selected_group()
            .unwrap()
            .session_ids
            .clone()),
        ["session-0"]
    );
    visual.simulate_keystrokes("ctrl-w");
    cx.run_until_parked();
    assert!(!app.read_with(cx, |app, _| app.sync_input.window_is_open_or_pending()));
}

#[gpui::test]
fn escape_dismisses_delete_confirmation_before_closing_groups_window(cx: &mut TestAppContext) {
    let _cleanup = crate::test_support::TestAppCleanup::new(cx);
    let dir = TestTempDir::new("nyaterm-sync-groups-delete");
    let app = app_with_visible_local_session(cx, dir.path(), "session-0");
    let parent = parent_window(cx, &app);
    let mut visual = open_groups(cx, &app, parent);
    visual.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.create_sync_group(cx);
            app.request_delete_selected_sync_group(window, cx);
        });
    });
    draw(&mut visual);
    visual.update(|window, cx| assert!(window.has_active_nya_dialog(cx)));
    visual.simulate_keystrokes("escape");
    draw(&mut visual);
    visual.update(|window, cx| assert!(!window.has_active_nya_dialog(cx)));
    assert!(app.read_with(cx, |app, _| app.sync_input.window_is_open_or_pending()));
    assert_eq!(app.read_with(cx, |app, _| app.sync_input.groups().len()), 1);
    visual.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.request_delete_selected_sync_group(window, cx)
        });
    });
    draw(&mut visual);
    visual.simulate_keystrokes("enter");
    draw(&mut visual);
    assert!(app.read_with(cx, |app, _| app.sync_input.groups().is_empty()));
}

#[gpui::test]
fn name_and_search_focus_rings_remain_visible_at_panel_edges(cx: &mut TestAppContext) {
    let _cleanup = crate::test_support::TestAppCleanup::new(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    let dir = TestTempDir::new("nyaterm-sync-groups-input-rings");
    let app = app_with_visible_local_session(cx, dir.path(), "session-0");
    let parent = parent_window(cx, &app);
    let mut visual = open_groups(cx, &app, parent);
    let handle = app.read_with(cx, |app, _| app.sync_input.window.handle().unwrap());
    let mut input_ids = vec!["sync.groups.search".to_string()];

    for has_group in [false, true] {
        if has_group {
            app.update(cx, |app, cx| app.create_sync_group(cx));
            let group_id = app.read_with(cx, |app, _| {
                app.sync_input.selected_group().unwrap().id.clone()
            });
            input_ids.push(format!("sync.group-name.{group_id}"));
        }
        for dimensions in [size(px(960.), px(640.)), size(px(760.), px(480.))] {
            visual.simulate_window_resize(handle.into(), dimensions);
            for scale in [1., 1.25] {
                visual.simulate_scale_factor_change(scale);
                draw(&mut visual);
                for id in &input_ids {
                    visual.update(|window, cx| {
                        let field = app.read(cx).existing_text_input(id).unwrap();
                        let focus = field.read(cx).component_focus_handle(cx);
                        window.focus(&focus, cx);
                    });
                    draw(&mut visual);
                    visual.update(|window, cx| {
                        let color: gpui::Hsla =
                            gpui::rgb(app.read(cx).theme_palette().focus_ring).into();
                        let quads = window.painted_quads();
                        let ring: Vec<_> = quads
                            .iter()
                            .filter(|quad| quad.border_color == color.alpha(0.5))
                            .collect();
                        let bounds = ring.first().expect("focused input paints a ring").bounds;
                        // Border strips share a ring's bounds, but their masks can
                        // clip individual edges. Inspect the combined visible paint.
                        let visible = ring
                            .iter()
                            .map(|quad| quad.content_mask.bounds.intersect(&quad.bounds))
                            .reduce(|left, right| left.union(&right))
                            .unwrap();
                        assert_eq!(
                            visible, bounds,
                            "focus ring clipped for {id} at scale {scale}"
                        );
                    });
                }
            }
        }
    }
}
