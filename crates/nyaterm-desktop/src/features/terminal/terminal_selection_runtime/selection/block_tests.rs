use std::io::Read;
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AppContext as _, ClipboardItem, Context, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Point, TestAppContext, px,
};
use nyaterm_terminal::TerminalScreen;
use nyaterm_terminal::selection::{
    TerminalBufferCellPos, TerminalSelection, TerminalSelectionMode,
};

use super::terminal_selected_text_for_view;
use super::tests::{click_terminal_cell, seed_click_selection_fixture};
use crate::features::NyaTermApp;
use crate::features::test_support::app_with_visible_local_session;
use crate::models::TerminalViewState;
use crate::test_support::{TestAppCleanup, TestConfigDir};

struct BlockCanvasHost {
    app: gpui::Entity<NyaTermApp>,
}

impl gpui::Render for BlockCanvasHost {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        use gpui::{InteractiveElement as _, IntoElement as _, ParentElement as _, Styled as _};
        let canvas = self.app.update(cx, |app, cx| {
            app.terminal_canvas_for("s1".into(), cx).into_any_element()
        });
        let move_app = self.app.clone();
        let up_app = self.app.clone();
        gpui::div()
            .size_full()
            .flex()
            .child(canvas)
            .on_mouse_move(move |event, _, cx| {
                move_app.update(cx, |app, cx| app.update_terminal_selection_drag(event, cx));
            })
            .on_mouse_up(MouseButton::Left, move |event, _, cx| {
                up_app.update(cx, |app, cx| app.finish_terminal_selection(event, cx));
            })
    }
}

#[gpui::test]
fn block_canvas_click_bubbles_release_and_keeps_stationary_alt_link_menu(cx: &mut TestAppContext) {
    let dir = TestConfigDir::new("nyaterm-block-canvas");
    let _cleanup = TestAppCleanup::new(cx);
    cx.update(nyaterm_ui::init_components);
    let app = app_with_visible_local_session(cx, dir.path(), "s1");
    app.update(cx, |app, _| {
        seed_click_selection_fixture(app);
        app.terminal
            .seed_session_view("s1".into(), "192.0.2.10\r\n192.0.2.11".into(), "UTF-8");
        let pipeline = &app.terminal.view.frame_pipeline;
        pipeline.seed_session("s1", "192.0.2.10\r\n192.0.2.11", "UTF-8", 1000);
        pipeline.request_snapshot("s1", 0, false, Default::default());
        pipeline.flush_for_test();
        let mut events = std::collections::VecDeque::new();
        pipeline.drain_events_into(&mut events, usize::MAX);
        let snapshot = events
            .into_iter()
            .find_map(|event| match event {
                crate::models::TerminalFrameEvent::Snapshot(frame) => Some(frame.snapshot),
                _ => None,
            })
            .unwrap();
        let view = app.terminal.view.views.get_mut("s1").unwrap();
        view.frame_snapshot = Some(snapshot);
        view.render_degraded = false;
        view.output_burst_bytes = 0;
        let mut settings = app.settings.summary().clone();
        settings.terminal_action_links_enabled = true;
        settings.interaction_copy_on_select = true;
        app.settings.replace_summary(settings);
    });
    let (_, vcx) = cx.add_window_view({
        let app = app.clone();
        move |window, cx| {
            let host = cx.new(|_| BlockCanvasHost { app });
            nyaterm_ui::nya_root(host, window, cx)
        }
    });
    vcx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    let (origin, head) = vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            (
                block_pointer(app, 0.5, 2.25, cx),
                block_pointer(app, 1.5, 10.25, cx),
            )
        })
    });
    let alt = Modifiers {
        alt: true,
        ..Modifiers::default()
    };
    vcx.simulate_mouse_move(origin, None, alt);
    vcx.simulate_mouse_down(origin, MouseButton::Left, alt);
    vcx.update(|_, cx| {
        app.update(cx, |app, _| {
            assert!(
                app.terminal.selection.block_gesture.is_some(),
                "canvas Alt press must enter selection; pointer={origin:?}, bounds={:?}",
                app.terminal.layout.session_surface_bounds
            );
        })
    });
    vcx.simulate_mouse_move(head, Some(MouseButton::Left), alt);
    vcx.update(|_, cx| {
        app.update(cx, |app, _| {
            assert!(
                !app.terminal.selection.selection.unwrap().is_empty(),
                "canvas drag must update the head"
            );
        })
    });
    vcx.simulate_mouse_up(head, MouseButton::Left, alt);
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            assert!(
                !app.terminal.selection.dragging,
                "on_click must let the root MouseUp finish the block"
            );
            assert!(app.terminal.menus.action_link_menu.is_none());
            let selected = app.selected_terminal_text().unwrap();
            assert_eq!(selected, "2.0.2.10\n2.0.2.11");
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some(selected)
            );
        })
    });
    vcx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    let origin = vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            // Initial canvas sizing queues output work; link detection requires a calm view.
            app.terminal.view.frame_pipeline.flush_for_test();
            while app.terminal.view.frame_pipeline.queued_event_count() > 0
                || !app.terminal.view.pending_frame_events.is_empty()
            {
                app.drain_terminal_frame_events(cx);
            }
            let view = app.terminal.view.views.get_mut("s1").unwrap();
            view.render_degraded = false;
            view.output_burst_bytes = 0;
            let origin = block_pointer(app, 0.5, 2.25, cx);
            assert!(!app.runtime_output_pressure_active());
            assert!(
                app.action_link_at_point(origin, cx).is_some(),
                "the calm fixture must expose an actionable link"
            );
            origin
        })
    });
    vcx.simulate_mouse_move(origin, None, alt);
    vcx.simulate_mouse_down(origin, MouseButton::Left, alt);
    vcx.simulate_mouse_up(origin, MouseButton::Left, alt);
    vcx.update(|_, cx| {
        app.update(cx, |app, _| {
            assert!(
                app.terminal.menus.action_link_menu.is_some(),
                "Alt click must open the link menu: suppressed={}, status={}",
                app.terminal.selection.suppress_action_link_click,
                app.shell.status()
            );
            assert!(app.terminal.selection.block_gesture.is_none());
            assert!(!app.terminal.selection.dragging);
        })
    });
}

fn block_pointer(
    app: &NyaTermApp,
    row: f32,
    col: f32,
    cx: &Context<NyaTermApp>,
) -> Point<gpui::Pixels> {
    let geometry = app
        .terminal_hit_test_geometry_for_session(Some("s1"), cx)
        .unwrap();
    Point {
        x: geometry.bounds.origin.x
            + px(geometry.padding_left + geometry.gutter + col * geometry.cell_w),
        y: geometry.bounds.origin.y
            + px(geometry.padding_top + geometry.visual_y_offset + row * geometry.cell_h),
    }
}

fn start_block_gesture(
    app: &mut NyaTermApp,
    position: Point<gpui::Pixels>,
    cx: &mut Context<NyaTermApp>,
) {
    app.start_terminal_selection_for_session(
        Some("s1"),
        &MouseDownEvent {
            position,
            button: MouseButton::Left,
            click_count: 3,
            modifiers: Modifiers {
                alt: true,
                shift: true,
                ..Modifiers::default()
            },
            ..MouseDownEvent::default()
        },
        cx,
    );
}

#[test]
fn block_drag_latches_mode_copies_on_release_and_shift_extends_the_block() {
    let dir = TestConfigDir::new("nyaterm-block-drag");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
    cx.update_entity(&app, |app, cx| {
        seed_click_selection_fixture(app);
        let mut settings = app.settings.summary().clone();
        settings.terminal_wrap_mode = nyaterm_core::models::settings::TerminalWrapMode::Character;
        settings.interaction_copy_on_select = true;
        settings.terminal_action_links_enabled = true;
        app.settings.replace_summary(settings);
        let origin = block_pointer(app, 0.5, 1.25, cx);
        let head = block_pointer(app, 2.5, 4.75, cx);
        start_block_gesture(app, origin, cx);
        assert!(app.selected_terminal_text().is_none());
        app.update_terminal_selection_drag(
            &MouseMoveEvent {
                position: Point {
                    x: origin.x + px(1.),
                    ..origin
                },
                pressed_button: Some(MouseButton::Left),
                ..MouseMoveEvent::default()
            },
            cx,
        );
        assert!(app.selected_terminal_text().is_none());
        assert!(!app.terminal.selection.suppress_action_link_click);
        // Alt is released before the first real drag event.
        app.update_terminal_selection_drag(
            &MouseMoveEvent {
                position: head,
                pressed_button: Some(MouseButton::Left),
                ..MouseMoveEvent::default()
            },
            cx,
        );
        assert_eq!(
            app.selected_terminal_text().as_deref(),
            Some("irst\necon\nhird")
        );
        app.finish_terminal_selection(
            &MouseUpEvent {
                position: head,
                button: MouseButton::Left,
                ..MouseUpEvent::default()
            },
            cx,
        );
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("irst\necon\nhird")
        );
        assert!(app.terminal.selection.block_gesture.is_none());
        assert!(app.terminal.selection.suppress_action_link_click);
        assert!(
            app.terminal
                .editing
                .state()
                .is_none_or(|state| state.selection.is_none())
        );
        assert!(app.terminal.selection.selected_occurrence.query.is_none());
        click_terminal_cell(app, "s1", 1, 2, true, cx);
        assert_eq!(
            app.terminal.selection.selection.unwrap().mode,
            TerminalSelectionMode::Block
        );
        assert_eq!(app.selected_terminal_text().as_deref(), Some("ir\nec"));
    });
}

#[test]
fn block_click_jitter_and_drag_back_have_different_link_decisions() {
    let dir = TestConfigDir::new("nyaterm-block-click");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
    cx.update_entity(&app, |app, cx| {
        seed_click_selection_fixture(app);
        let origin = block_pointer(app, 0.5, 1.25, cx);
        start_block_gesture(app, origin, cx);
        app.finish_terminal_selection(
            &MouseUpEvent {
                position: Point {
                    x: origin.x + px(1.),
                    ..origin
                },
                button: MouseButton::Left,
                ..MouseUpEvent::default()
            },
            cx,
        );
        assert!(app.terminal.selection.selection.is_none());
        assert!(!app.terminal.selection.suppress_action_link_click);
        start_block_gesture(app, origin, cx);
        app.update_terminal_selection_drag(
            &MouseMoveEvent {
                position: block_pointer(app, 1.5, 5.25, cx),
                pressed_button: Some(MouseButton::Left),
                ..MouseMoveEvent::default()
            },
            cx,
        );
        app.finish_terminal_selection(
            &MouseUpEvent {
                position: origin,
                button: MouseButton::Left,
                ..MouseUpEvent::default()
            },
            cx,
        );
        assert!(app.terminal.selection.selection.is_none());
        assert!(app.terminal.selection.suppress_action_link_click);
        // A release can establish a drag without an intervening move event.
        start_block_gesture(app, origin, cx);
        assert!(!app.terminal.selection.suppress_action_link_click);
        app.finish_terminal_selection(
            &MouseUpEvent {
                position: block_pointer(app, 1.5, 4.25, cx),
                button: MouseButton::Left,
                ..MouseUpEvent::default()
            },
            cx,
        );
        assert_eq!(app.selected_terminal_text().as_deref(), Some("irs\neco"));
        assert!(app.terminal.selection.suppress_action_link_click);
    });
}

#[test]
fn block_gestures_recover_lost_release_and_are_cleared_in_no_wrap_mode() {
    let dir = TestConfigDir::new("nyaterm-block-lifecycle");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
    cx.update_entity(&app, |app, cx| {
        seed_click_selection_fixture(app);
        let origin = block_pointer(app, 0.5, 1.25, cx);
        start_block_gesture(app, origin, cx);
        app.recover_terminal_selection_after_lost_mouse_up(cx);
        assert!(app.terminal.selection.block_gesture.is_none());
        assert!(app.terminal.selection.selection.is_none());
        start_block_gesture(app, origin, cx);
        app.update_terminal_selection_drag(
            &MouseMoveEvent {
                position: block_pointer(app, 1.5, 4.25, cx),
                pressed_button: Some(MouseButton::Left),
                ..MouseMoveEvent::default()
            },
            cx,
        );
        app.recover_terminal_selection_after_lost_mouse_up(cx);
        assert_eq!(app.selected_terminal_text().as_deref(), Some("irs\neco"));
        app.set_terminal_wrap_mode(nyaterm_core::models::settings::TerminalWrapMode::None, cx);
        assert!(app.terminal.selection.selection.is_none());
        assert!(app.terminal.selection.block_gesture.is_none());
        start_block_gesture(app, origin, cx);
        assert!(app.terminal.selection.block_gesture.is_none());
        assert_eq!(
            app.terminal.selection.selection.unwrap().mode,
            TerminalSelectionMode::Linear
        );
    });
}

#[test]
fn block_snapshot_copy_matches_live_grid_in_history_with_wide_edges_and_wraps() {
    let mut screen = TerminalScreen::new(5, 3);
    screen.advance("abc界x\r\n\r\na😀z\r\nlast".as_bytes());
    let snapshot = screen.viewport_snapshot(2);
    let mut selection = TerminalSelection::with_anchor_and_mode(
        TerminalBufferCellPos::new(0, 2),
        TerminalSelectionMode::Block,
    );
    selection.head = TerminalBufferCellPos::new(2, 5).into();
    let expected = screen.text_for_selection(selection).unwrap();
    let mut view = TerminalViewState::new();
    view.frame_snapshot = Some(Arc::new(snapshot));
    assert_eq!(terminal_selected_text_for_view(&view, selection), expected);
}

#[test]
fn block_scroll_rehit_preserves_anchor_columns_and_mode() {
    let dir = TestConfigDir::new("nyaterm-block-scroll");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
    cx.update_entity(&app, |app, cx| {
        seed_click_selection_fixture(app);
        let origin = block_pointer(app, 0.5, 1.25, cx);
        let head = block_pointer(app, 2.5, 4.25, cx);
        start_block_gesture(app, origin, cx);
        app.update_terminal_selection_drag(
            &MouseMoveEvent {
                position: head,
                pressed_button: Some(MouseButton::Left),
                ..MouseMoveEvent::default()
            },
            cx,
        );
        let anchor = app.terminal.selection.selection.unwrap().anchor;
        app.reconcile_terminal_selection_after_scroll("s1", head, 0, 1, cx);
        let selection = app.terminal.selection.selection.unwrap();
        assert_eq!(selection.head.position.line, 1);
        assert_eq!(selection.cols_for_absolute_line(1), Some((1, 4)));
        assert_eq!(selection.anchor, anchor);
        let rehit = block_pointer(app, 1.5, 4.75, cx);
        assert!(app.refresh_terminal_selection_head_from_painted_geometry("s1", rehit, cx));
        let selection = app.terminal.selection.selection.unwrap();
        assert_eq!(selection.cols_for_absolute_line(1), Some((1, 5)));
        assert_eq!(selection.anchor, anchor);
        assert_eq!(selection.mode, TerminalSelectionMode::Block);
        let top = app
            .terminal_hit_test_geometry_for_session(Some("s1"), cx)
            .unwrap()
            .bounds
            .origin
            .y;
        app.update_terminal_selection_drag(
            &MouseMoveEvent {
                position: Point {
                    y: top - px(10.),
                    ..head
                },
                pressed_button: Some(MouseButton::Left),
                ..MouseMoveEvent::default()
            },
            cx,
        );
        assert!(app.terminal.selection.autoscroll.is_some());
        cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
        app.cancel_terminal_selection_drag(cx);
        assert!(app.terminal.selection.autoscroll.is_none());
        assert!(app.terminal.selection.block_gesture.is_none());
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("unchanged")
        );
    });
}

#[test]
fn block_gesture_defers_to_remote_alt_mouse_reports_until_release() {
    let dir = TestConfigDir::new("nyaterm-block-mouse-report");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
    let manager = cx.update_entity(&app, |app, _| app.session.manager_handle());
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let info = manager
        .create_telnet_session(nyaterm_transport::TelnetSessionConfig {
            host: "127.0.0.1".into(),
            port: listener.local_addr().unwrap().port(),
            raw_tcp: true,
            ..nyaterm_transport::TelnetSessionConfig::default()
        })
        .unwrap();
    let (mut peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut expected = Vec::new();
    cx.update_entity(&app, |app, cx| {
        seed_click_selection_fixture(app);
        let metadata = app.session.metadata("s1").unwrap().clone();
        app.session.register_session_metadata(&info.id, metadata);
        app.session.select_active_session(&info.id);
        app.terminal
            .seed_session_view(info.id.clone(), String::new(), "UTF-8");
        let geometry = app
            .terminal_hit_test_geometry_for_session(Some(&info.id), cx)
            .unwrap();
        let origin = Point {
            x: geometry.bounds.origin.x
                + px(geometry.padding_left + geometry.gutter + 1.25 * geometry.cell_w),
            y: geometry.bounds.origin.y
                + px(geometry.padding_top + geometry.visual_y_offset + 0.5 * geometry.cell_h),
        };
        let alt = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        for require_alt in [false, true] {
            for links in [false, true] {
                let mut settings = app.settings.summary().clone();
                settings.interaction_mouse_events_require_alt = require_alt;
                settings.terminal_action_links_enabled = links;
                app.settings.replace_summary(settings);
                let protocol = &mut app
                    .terminal
                    .view
                    .views
                    .get_mut(&info.id)
                    .unwrap()
                    .protocol_state;
                protocol.mouse_reporting = true;
                protocol.mouse_drag_reporting = true;
                protocol.mouse_sgr = true;
                app.start_terminal_selection_for_session(
                    Some(&info.id),
                    &MouseDownEvent {
                        position: origin,
                        button: MouseButton::Left,
                        modifiers: alt,
                        click_count: 1,
                        ..MouseDownEvent::default()
                    },
                    cx,
                );
                assert!(app.terminal.selection.selection.is_none());
                assert!(app.terminal.selection.block_gesture.is_none());
                assert_eq!(
                    app.terminal.selection.mouse_report_session_id.as_deref(),
                    Some(info.id.as_str())
                );
                assert!(app.terminal.selection.suppress_action_link_click);
                app.update_terminal_selection_drag(
                    &MouseMoveEvent {
                        position: origin,
                        modifiers: alt,
                        pressed_button: Some(MouseButton::Left),
                    },
                    cx,
                );
                app.finish_terminal_selection(
                    &MouseUpEvent {
                        position: origin,
                        button: MouseButton::Left,
                        // Existing protocol policy permits the captured release after Alt is lifted.
                        ..MouseUpEvent::default()
                    },
                    cx,
                );
                assert!(app.terminal.selection.mouse_report_button.is_none());
                expected.extend_from_slice(b"\x1b[<8;2;1M\x1b[<40;2;1M\x1b[<0;2;1m");
            }
        }
    });
    let mut actual = vec![0; expected.len()];
    peer.read_exact(&mut actual).unwrap();
    assert_eq!(actual, expected);
    manager.close(&info.id).unwrap();
}
