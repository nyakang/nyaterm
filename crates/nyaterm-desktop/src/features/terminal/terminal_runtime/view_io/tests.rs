use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{AppContext as _, KeyDownEvent, MouseButton, TestAppContext};
use nyaterm_terminal::TerminalScreen;

use crate::features::terminal::terminal_surface_entity::terminal_snapshot_anchor_row_for_display_offset;
use crate::features::test_support::app_with_visible_local_session;
use crate::models::{
    TerminalBufferCellPos, TerminalFrameActionLinks, TerminalSelection, TerminalViewState,
};
use crate::terminal::{TerminalBufferMatch, TerminalKeyMode};
use crate::test_support::TestConfigDir;

use super::{
    TERMINAL_INPUT_LATENCY_WINDOW, TERMINAL_USER_SCROLL_ACTIVE_WINDOW,
    lost_mouse_report_release_button, mouse_report_allowed_by_alt,
    terminal_cursor_position_changed, terminal_cursor_visible_for_display_offset,
    terminal_input_latency_active, terminal_key_bytes_for_mode_and_settings,
    terminal_keyword_highlight_updates_allowed, terminal_live_action_link_enrichment_allowed,
    terminal_mouse_report_button, terminal_paint_snapshot_for_view,
    terminal_paint_window_snapshot_for_view, terminal_retained_snapshot_matches_view,
    terminal_scroll_retained_window_extra_rows, terminal_scroll_snapshot_request_offset,
    terminal_scroll_text_first_decorations, terminal_selection_for_session,
    terminal_session_write_failure_log, terminal_should_defer_key_text_to_input_handler_for_state,
    terminal_should_track_command_suggestion_input, terminal_snapshot_covers_display_offset,
    terminal_snapshot_with_newer_edge_row, terminal_snapshot_with_retained_scroll_window,
    terminal_status_changed, terminal_user_scroll_active, terminal_visual_display_offset,
};

#[test]
fn terminal_mouse_report_buttons_map_to_their_gpui_capture_button() {
    assert_eq!(terminal_mouse_report_button(0), Some(MouseButton::Left));
    assert_eq!(terminal_mouse_report_button(1), Some(MouseButton::Middle));
    assert_eq!(terminal_mouse_report_button(2), Some(MouseButton::Right));
    assert_eq!(terminal_mouse_report_button(3), None);
}

#[test]
fn alt_mouse_gate_allows_captured_release_after_alt_is_released() {
    assert!(!mouse_report_allowed_by_alt(
        true, false, true, false, false
    ));
    assert!(!mouse_report_allowed_by_alt(true, false, false, true, true));
    assert!(mouse_report_allowed_by_alt(true, false, false, false, true));
    assert!(!mouse_report_allowed_by_alt(
        true, false, false, false, false
    ));
    assert!(mouse_report_allowed_by_alt(true, true, true, false, false));
    assert!(mouse_report_allowed_by_alt(
        false, false, true, false, false
    ));
}

#[test]
fn lost_mouse_report_capture_releases_the_button_that_is_no_longer_pressed() {
    for (captured, button) in [
        (0, MouseButton::Left),
        (1, MouseButton::Middle),
        (2, MouseButton::Right),
    ] {
        assert_eq!(
            lost_mouse_report_release_button(captured, Some(button)),
            None
        );
        assert_eq!(
            lost_mouse_report_release_button(captured, None),
            Some(button)
        );
        assert_eq!(
            lost_mouse_report_release_button(
                captured,
                Some(MouseButton::Navigate(gpui::NavigationDirection::Back))
            ),
            Some(button)
        );
    }
    assert_eq!(lost_mouse_report_release_button(3, None), None);
}

#[test]
fn terminal_selection_visual_is_isolated_to_its_owner_session() {
    let selection = TerminalSelection::from_range(
        TerminalBufferCellPos::new(10, 2),
        TerminalBufferCellPos::new(11, 4),
    );

    assert_eq!(
        terminal_selection_for_session(Some(selection), Some("left"), Some("right"), "left"),
        Some(selection)
    );
    assert_eq!(
        terminal_selection_for_session(Some(selection), Some("left"), Some("right"), "right"),
        None
    );
    assert_eq!(
        terminal_selection_for_session(Some(selection), None, Some("right"), "right"),
        Some(selection)
    );
}

fn key_event(key: &str, key_char: Option<&str>, modifiers: gpui::Modifiers) -> KeyDownEvent {
    KeyDownEvent {
        keystroke: gpui::Keystroke {
            modifiers,
            key: key.to_string(),
            key_char: key_char.map(str::to_string),
        },
        is_held: false,
        prefer_character_input: false,
    }
}

fn terminal_output_lines(count: usize) -> String {
    (0..count)
        .map(|index| format!("line {index:03}\n"))
        .collect::<String>()
}

const CURSOR_POSITION_SESSION_ID: &str = "cursor-position-session";

#[test]
fn terminal_paint_snapshot_waits_without_authoritative_scrollback_snapshot() {
    let view = TerminalViewState::from_output(terminal_output_lines(40));

    assert!(terminal_paint_snapshot_for_view(Some(&view), 4, None).is_none());
}

#[test]
fn terminal_paint_snapshot_can_retain_matching_surface_snapshot() {
    let view = TerminalViewState::from_output(terminal_output_lines(40));
    let retained = std::sync::Arc::new(view.screen.viewport_snapshot(4));

    let snapshot = terminal_paint_snapshot_for_view(Some(&view), 4, Some(retained))
        .expect("matching retained surface snapshot should be usable");

    assert_eq!(snapshot.display_offset, 4);
}

#[test]
fn terminal_paint_snapshot_does_not_use_ui_screen_fallback() {
    let view = TerminalViewState::from_output(terminal_output_lines(40));

    assert!(terminal_paint_snapshot_for_view(Some(&view), 4, None).is_none());
}

#[test]
fn terminal_paint_window_snapshot_prefers_latest_live_frame_over_retained_surface() {
    let mut old_view = TerminalViewState::from_output(terminal_output_lines(40));
    old_view.frame_snapshot = Some(std::sync::Arc::new(old_view.screen.viewport_snapshot(0)));
    let mut view = TerminalViewState::from_output(terminal_output_lines(42));
    view.frame_snapshot = Some(std::sync::Arc::new(view.screen.viewport_snapshot(0)));
    let retained = old_view.frame_snapshot.clone();
    let latest = view
        .frame_snapshot
        .clone()
        .expect("view should have live frame snapshot");

    let snapshot = terminal_paint_window_snapshot_for_view(
        Some(&view),
        0,
        view.viewport_rows_for_ui(),
        retained,
    )
    .expect("live frame snapshot should be available");

    assert!(std::sync::Arc::ptr_eq(&snapshot, &latest));
}

#[test]
fn live_surface_frame_waits_for_matching_worker_geometry_and_a_live_snapshot() {
    let root = TestConfigDir::new("nyaterm-live-scroll-geometry");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "live-scroll");
    cx.update_entity(&app, |app, _| {
        assert!(app.terminal_live_surface_frame("live-scroll").is_none());
        let snapshot = Arc::new(TerminalScreen::default().viewport_snapshot(0));
        let view = app.terminal.view.views.get_mut("live-scroll").unwrap();
        view.frame_snapshot = Some(snapshot.clone());
        assert!(app.terminal_live_surface_frame("live-scroll").is_some());

        let view = app.terminal.view.views.get_mut("live-scroll").unwrap();
        view.grid_resize_pending = true;
        assert!(app.terminal_live_surface_frame("live-scroll").is_none());

        let view = app.terminal.view.views.get_mut("live-scroll").unwrap();
        view.grid_resize_pending = false;
        view.screen.resize(100, 24);
        assert!(app.terminal_live_surface_frame("live-scroll").is_none());
    });
}

#[test]
fn live_surface_frame_uses_current_cursor_policy_without_resetting_blink_phase() {
    let root = TestConfigDir::new("nyaterm-live-scroll-cursor");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "live-scroll");
    cx.update_entity(&app, |app, _| {
        let mut screen = TerminalScreen::default();
        screen.advance(b"prompt\x1b[?25h");
        app.terminal
            .view
            .views
            .get_mut("live-scroll")
            .unwrap()
            .frame_snapshot = Some(Arc::new(screen.viewport_snapshot(0)));
        app.shell.set_cursor_blink_on(true);
        let (frame, basis) = app.terminal_live_surface_frame("live-scroll").unwrap();
        assert!(frame.show_cursor);
        assert!(basis.cursor_visible_when_lit);

        app.shell.set_cursor_blink_on(false);
        assert!(
            !app.terminal_live_surface_frame("live-scroll")
                .unwrap()
                .0
                .show_cursor
        );
        assert!(!app.shell.cursor_blink_on());

        app.shell.set_cursor_blink_on(true);
        app.session.select_active_session("other-session");
        assert!(
            !app.terminal_live_surface_frame("live-scroll")
                .unwrap()
                .0
                .show_cursor
        );

        app.session.select_active_session("live-scroll");
        app.session.mark_session_disconnected("live-scroll");
        assert!(
            !app.terminal_live_surface_frame("live-scroll")
                .unwrap()
                .0
                .show_cursor
        );
        app.session
            .metadata_mut("live-scroll")
            .unwrap()
            .disconnected = false;
        screen.advance(b"\x1b[?25l");
        app.terminal
            .view
            .views
            .get_mut("live-scroll")
            .unwrap()
            .frame_snapshot = Some(Arc::new(screen.viewport_snapshot(0)));
        assert!(
            !app.terminal_live_surface_frame("live-scroll")
                .unwrap()
                .0
                .show_cursor
        );
    });
}

#[test]
fn terminal_paint_window_keeps_worker_snapshot_when_ui_screen_is_stale() {
    let view = TerminalViewState::from_output(terminal_output_lines(8));
    let mut worker_screen = TerminalScreen::default();
    worker_screen.advance_decoded_text(&terminal_output_lines(160));
    let worker_snapshot = std::sync::Arc::new(worker_screen.snapshot());
    let mut view = view;
    view.frame_snapshot = Some(worker_snapshot.clone());

    let snapshot =
        terminal_paint_window_snapshot_for_view(Some(&view), 0, view.viewport_rows_for_ui(), None)
            .expect("authoritative worker snapshot should be paintable");

    assert!(std::sync::Arc::ptr_eq(&snapshot, &worker_snapshot));
    assert!(
        snapshot
            .rows()
            .iter()
            .any(|row| row.text.contains("line 159"))
    );
}

#[test]
fn terminal_retained_snapshot_uses_worker_scrollback_length() {
    let view = TerminalViewState::from_output(terminal_output_lines(8));
    let mut worker_screen = TerminalScreen::default();
    worker_screen.advance_decoded_text(&terminal_output_lines(160));
    let worker_snapshot = std::sync::Arc::new(worker_screen.snapshot());
    let mut view = view;
    view.frame_snapshot = Some(worker_snapshot.clone());

    assert!(terminal_retained_snapshot_matches_view(
        worker_snapshot.as_ref(),
        &view,
        0,
        view.viewport_rows_for_ui(),
    ));
}

#[test]
fn terminal_paint_window_waits_for_worker_after_height_resize() {
    let mut view = TerminalViewState::from_output(terminal_output_lines(80));
    let old_snapshot = std::sync::Arc::new(view.screen.viewport_snapshot(0));
    let old_rows = old_snapshot.row_count();
    view.frame_snapshot = Some(old_snapshot.clone());

    view.screen
        .resize(view.screen.cols() as u16, (old_rows + 16) as u16);
    view.grid_resize_pending = true;
    let viewport_rows = view.viewport_rows_for_ui();
    assert!(viewport_rows > old_rows);

    let snapshot = terminal_paint_window_snapshot_for_view(Some(&view), 0, viewport_rows, None);

    assert!(snapshot.is_none());
    assert!(std::sync::Arc::ptr_eq(
        view.frame_snapshot.as_ref().unwrap(),
        &old_snapshot,
    ));
}

#[test]
fn terminal_paint_window_waits_for_worker_after_width_resize() {
    let mut view = TerminalViewState::from_output(terminal_output_lines(80));
    let old_snapshot = std::sync::Arc::new(view.screen.viewport_snapshot(0));
    let old_cols = old_snapshot.cols;
    view.frame_snapshot = Some(old_snapshot.clone());

    view.screen
        .resize((old_cols + 24) as u16, view.screen.rows() as u16);
    view.grid_resize_pending = true;
    let viewport_rows = view.viewport_rows_for_ui();

    let snapshot = terminal_paint_window_snapshot_for_view(Some(&view), 0, viewport_rows, None);

    assert!(snapshot.is_none());
    assert_eq!(view.frame_snapshot.as_ref().unwrap().cols, old_cols);
}

#[test]
fn terminal_retained_snapshot_accepts_current_scroll_window_geometry() {
    let view = TerminalViewState::from_output(terminal_output_lines(80));
    let display_offset = 4;
    let snapshot = view.snapshot_with_scroll_window(display_offset);
    let viewport_rows = view.viewport_rows_for_ui();

    assert!(terminal_retained_snapshot_matches_view(
        snapshot.as_ref(),
        &view,
        display_offset,
        viewport_rows,
    ));
}

#[test]
fn terminal_retained_snapshot_rejects_pre_resize_geometry() {
    let mut view = TerminalViewState::from_output(terminal_output_lines(80));
    let display_offset = 4;
    let snapshot = view.snapshot_with_scroll_window(display_offset);
    let viewport_rows = view.viewport_rows_for_ui();
    let resized_cols = view.screen.cols().saturating_add(8) as u16;
    let resized_rows = view.screen.rows().saturating_add(6) as u16;

    view.screen.resize(resized_cols, resized_rows);

    assert!(!terminal_retained_snapshot_matches_view(
        snapshot.as_ref(),
        &view,
        display_offset,
        viewport_rows,
    ));
}

#[test]
fn terminal_visual_display_offset_keeps_text_window_stable_for_fractional_scroll() {
    assert_eq!(terminal_visual_display_offset(0, 0.0, 10), 0);
    assert_eq!(terminal_visual_display_offset(0, 0.25, 10), 0);
    assert_eq!(terminal_visual_display_offset(0, 0.5, 10), 0);
    assert_eq!(terminal_visual_display_offset(0, 0.95, 10), 0);
    assert_eq!(terminal_visual_display_offset(4, -0.25, 10), 4);
    assert_eq!(terminal_visual_display_offset(4, -0.6, 10), 4);
    assert_eq!(terminal_visual_display_offset(10, 0.5, 10), 10);
}

#[test]
fn terminal_scroll_snapshot_request_offset_waits_for_stable_text_offset() {
    assert_eq!(terminal_scroll_snapshot_request_offset(0, 0.0, 10), None);
    assert_eq!(terminal_scroll_snapshot_request_offset(0, 0.49, 10), None);
    assert_eq!(terminal_scroll_snapshot_request_offset(0, 0.5, 10), None);
    assert_eq!(
        terminal_scroll_snapshot_request_offset(4, -0.25, 10),
        Some(4)
    );
    assert_eq!(
        terminal_scroll_snapshot_request_offset(10, 0.5, 10),
        Some(10)
    );
}

#[test]
fn terminal_cursor_visibility_uses_display_offset_not_raw_scroll_offset() {
    assert!(terminal_cursor_visible_for_display_offset(
        true, false, 0, true, false, false
    ));
    assert!(!terminal_cursor_visible_for_display_offset(
        true, false, 1, true, false, false
    ));
    assert!(!terminal_cursor_visible_for_display_offset(
        true, false, 0, true, true, false
    ));
}

#[test]
fn terminal_cursor_position_change_is_detected_for_a_new_live_position() {
    assert!(terminal_cursor_position_changed(Some((4, 7)), Some((4, 8)),));
}

#[test]
fn moving_the_live_cursor_resets_a_hidden_blink_phase_before_paint() {
    let root = TestConfigDir::new("nyaterm-cursor-position");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), CURSOR_POSITION_SESSION_ID);

    cx.update_entity(&app, |app, cx| {
        let mut screen = TerminalScreen::default();
        screen.advance_decoded_text("prompt");
        let old_snapshot = Arc::new(screen.snapshot());
        screen.advance(b"\x1b[1C");
        let new_snapshot = Arc::new(screen.snapshot());

        app.terminal
            .view
            .views
            .get_mut(CURSOR_POSITION_SESSION_ID)
            .expect("fixture session view should exist")
            .frame_snapshot = Some(old_snapshot);
        app.sync_terminal_surface_paint(CURSOR_POSITION_SESSION_ID, cx);

        app.shell.set_cursor_blink_on(false);
        app.terminal
            .view
            .views
            .get_mut(CURSOR_POSITION_SESSION_ID)
            .expect("fixture session view should exist")
            .frame_snapshot = Some(new_snapshot);
        app.sync_terminal_surface_paint(CURSOR_POSITION_SESSION_ID, cx);

        assert!(app.shell.cursor_blink_on());
        let surface = app
            .terminal
            .view
            .surfaces
            .get(CURSOR_POSITION_SESSION_ID)
            .expect("fixture terminal surface should exist");
        assert!(surface.read(cx).cursor_is_shown());
    });
}

#[test]
fn terminal_snapshot_edge_row_extends_fractional_scroll_window() {
    let view = TerminalViewState::from_output(terminal_output_lines(40));
    let base = std::sync::Arc::new(view.screen.viewport_snapshot(1));
    let newer = std::sync::Arc::new(view.screen.viewport_snapshot(0));
    let base_rows = base.row_count();
    let newer_tail = newer.rows().last().map(|row| row.text.clone());

    let snapshot = terminal_snapshot_with_newer_edge_row(base, newer);

    assert_eq!(snapshot.row_count(), base_rows + 1);
    assert_eq!(
        snapshot.rows().last().map(|row| row.text.clone()),
        newer_tail
    );
    assert!(
        snapshot
            .rows()
            .iter()
            .all(|row| row.cells.len() == snapshot.cols)
    );
}

#[test]
fn terminal_snapshot_edge_row_preserves_absolute_range_start() {
    let view = TerminalViewState::from_output(terminal_output_lines(40));
    let base = std::sync::Arc::new(view.screen.viewport_snapshot(1));
    let newer = std::sync::Arc::new(view.screen.viewport_snapshot(0));
    let (base_start, _) =
        crate::features::terminal::terminal_surface::terminal_snapshot_absolute_range(
            base.as_ref(),
        );

    let snapshot = terminal_snapshot_with_newer_edge_row(base, newer);
    let (start, end) =
        crate::features::terminal::terminal_surface::terminal_snapshot_absolute_range(
            snapshot.as_ref(),
        );

    assert_eq!(start, base_start);
    assert_eq!(
        end,
        snapshot.total_rows.saturating_sub(snapshot.display_offset)
    );
}

#[test]
fn terminal_paint_window_snapshot_reuses_cached_retained_window_for_neighbor_offsets() {
    let mut view = TerminalViewState::from_output(terminal_output_lines(80));
    let display_offset = 6;
    let viewport_rows = view.viewport_rows_for_ui();
    let scrollback_len = view.scrollback_len_for_ui();
    let retained = terminal_snapshot_with_retained_scroll_window(
        &view,
        std::sync::Arc::new(view.screen.viewport_snapshot(display_offset)),
        display_offset,
        viewport_rows,
        scrollback_len,
    );
    view.scrollback_snapshots.insert(display_offset, retained);

    let snapshot =
        terminal_paint_window_snapshot_for_view(Some(&view), display_offset, viewport_rows, None)
            .expect("window snapshot should be available");

    assert!(snapshot.row_count() > viewport_rows);
    assert!(terminal_snapshot_covers_display_offset(
        snapshot.as_ref(),
        display_offset,
        viewport_rows,
        scrollback_len
    ));
    assert!(terminal_snapshot_covers_display_offset(
        snapshot.as_ref(),
        display_offset - 1,
        viewport_rows,
        scrollback_len
    ));
    assert!(terminal_snapshot_covers_display_offset(
        snapshot.as_ref(),
        display_offset + 1,
        viewport_rows,
        scrollback_len
    ));
    assert!(
        terminal_snapshot_anchor_row_for_display_offset(
            snapshot.as_ref(),
            display_offset,
            viewport_rows,
            scrollback_len
        ) > 0
    );
}

#[test]
fn terminal_paint_window_snapshot_reuses_covering_cached_retained_window() {
    let mut view = TerminalViewState::from_output(terminal_output_lines(160));
    let cached_offset = 40;
    let target_offset = cached_offset + 2;
    let viewport_rows = view.viewport_rows_for_ui();
    let scrollback_len = view.scrollback_len_for_ui();
    let retained = terminal_snapshot_with_retained_scroll_window(
        &view,
        std::sync::Arc::new(view.screen.viewport_snapshot(cached_offset)),
        cached_offset,
        viewport_rows,
        scrollback_len,
    );
    assert!(terminal_snapshot_covers_display_offset(
        retained.as_ref(),
        target_offset,
        viewport_rows,
        scrollback_len
    ));
    view.scrollback_snapshots
        .insert(cached_offset, retained.clone());

    let snapshot =
        terminal_paint_window_snapshot_for_view(Some(&view), target_offset, viewport_rows, None)
            .expect("covering retained window should be reused");

    assert!(std::sync::Arc::ptr_eq(&snapshot, &retained));
    let anchor = terminal_snapshot_anchor_row_for_display_offset(
        snapshot.as_ref(),
        target_offset,
        viewport_rows,
        scrollback_len,
    );
    assert_eq!(
        snapshot.line(anchor),
        view.screen.viewport_snapshot(target_offset).line(0)
    );
}

#[test]
fn terminal_paint_window_snapshot_reuses_cached_retained_window_without_rewrapping() {
    let mut view = TerminalViewState::from_output(terminal_output_lines(120));
    let display_offset = 12;
    let viewport_rows = view.viewport_rows_for_ui();
    let scrollback_len = view.scrollback_len_for_ui();
    let base = std::sync::Arc::new(view.screen.viewport_snapshot(display_offset));
    let retained = terminal_snapshot_with_retained_scroll_window(
        &view,
        base,
        display_offset,
        viewport_rows,
        scrollback_len,
    );
    let retained_rows = retained.row_count();
    view.scrollback_snapshots
        .insert(display_offset, retained.clone());

    let snapshot =
        terminal_paint_window_snapshot_for_view(Some(&view), display_offset, viewport_rows, None)
            .expect("cached retained window should be reusable");

    assert_eq!(snapshot.row_count(), retained_rows);
    assert!(std::sync::Arc::ptr_eq(&snapshot, &retained));
    assert!(terminal_snapshot_covers_display_offset(
        snapshot.as_ref(),
        display_offset,
        viewport_rows,
        scrollback_len
    ));
}

#[test]
fn terminal_paint_window_snapshot_covers_viewport_sized_scroll_runs() {
    let mut view = TerminalViewState::from_output(terminal_output_lines(160));
    let display_offset = 40;
    let viewport_rows = view.viewport_rows_for_ui();
    let scrollback_len = view.scrollback_len_for_ui();
    let base = std::sync::Arc::new(view.screen.viewport_snapshot(display_offset));
    let retained = terminal_snapshot_with_retained_scroll_window(
        &view,
        base,
        display_offset,
        viewport_rows,
        scrollback_len,
    );
    view.scrollback_snapshots.insert(display_offset, retained);

    let snapshot =
        terminal_paint_window_snapshot_for_view(Some(&view), display_offset, viewport_rows, None)
            .expect("cached window snapshot should cover direct scroll runs");

    assert!(viewport_rows >= 16);
    assert!(terminal_snapshot_covers_display_offset(
        snapshot.as_ref(),
        display_offset.saturating_sub(viewport_rows),
        viewport_rows,
        scrollback_len
    ));
    assert!(terminal_snapshot_covers_display_offset(
        snapshot.as_ref(),
        display_offset,
        viewport_rows,
        scrollback_len
    ));
    assert!(terminal_snapshot_covers_display_offset(
        snapshot.as_ref(),
        display_offset + viewport_rows,
        viewport_rows,
        scrollback_len
    ));
}

#[test]
fn terminal_paint_window_snapshot_covers_multi_viewport_fast_scroll_runs() {
    let mut view = TerminalViewState::from_output(terminal_output_lines(240));
    let display_offset = 80;
    let viewport_rows = view.viewport_rows_for_ui();
    let scrollback_len = view.scrollback_len_for_ui();
    let fast_delta = viewport_rows.saturating_mul(2);
    let base = std::sync::Arc::new(view.screen.viewport_snapshot(display_offset));
    let retained = terminal_snapshot_with_retained_scroll_window(
        &view,
        base,
        display_offset,
        viewport_rows,
        scrollback_len,
    );
    view.scrollback_snapshots.insert(display_offset, retained);

    let snapshot =
        terminal_paint_window_snapshot_for_view(Some(&view), display_offset, viewport_rows, None)
            .expect("cached window snapshot should cover multi-viewport fast scroll runs");

    assert!(
        snapshot
            .rows()
            .iter()
            .all(|row| row.cells.len() == snapshot.cols)
    );
    assert!(terminal_snapshot_covers_display_offset(
        snapshot.as_ref(),
        display_offset.saturating_sub(fast_delta),
        viewport_rows,
        scrollback_len
    ));
    assert!(terminal_snapshot_covers_display_offset(
        snapshot.as_ref(),
        display_offset,
        viewport_rows,
        scrollback_len
    ));
    assert!(terminal_snapshot_covers_display_offset(
        snapshot.as_ref(),
        display_offset + fast_delta,
        viewport_rows,
        scrollback_len
    ));

    for offset in [
        display_offset.saturating_sub(fast_delta),
        display_offset,
        display_offset + fast_delta,
    ] {
        let anchor = terminal_snapshot_anchor_row_for_display_offset(
            snapshot.as_ref(),
            offset,
            viewport_rows,
            scrollback_len,
        );
        assert_eq!(
            snapshot.line(anchor),
            view.screen.viewport_snapshot(offset).line(0)
        );
    }
}

#[test]
fn terminal_scroll_retained_window_extra_rows_covers_fast_scroll_runs() {
    assert_eq!(terminal_scroll_retained_window_extra_rows(12), 32);
    assert_eq!(terminal_scroll_retained_window_extra_rows(40), 80);
    assert_eq!(terminal_scroll_retained_window_extra_rows(120), 192);
}

#[test]
fn terminal_paint_window_snapshot_waits_without_cached_snapshot() {
    let view = TerminalViewState::from_output(terminal_output_lines(80));
    let display_offset = 6;
    let viewport_rows = view.viewport_rows_for_ui();

    assert!(
        terminal_paint_window_snapshot_for_view(Some(&view), display_offset, viewport_rows, None,)
            .is_none()
    );
}

#[test]
fn terminal_paint_window_snapshot_reuses_cached_authoritative_snapshot() {
    let mut view = TerminalViewState::from_output(terminal_output_lines(80));
    let display_offset = 6;
    let viewport_rows = view.viewport_rows_for_ui();
    let scrollback_len = view.scrollback_len_for_ui();
    let retained = terminal_snapshot_with_retained_scroll_window(
        &view,
        std::sync::Arc::new(view.screen.viewport_snapshot(display_offset)),
        display_offset,
        viewport_rows,
        scrollback_len,
    );
    view.scrollback_snapshots.insert(display_offset, retained);

    let snapshot =
        terminal_paint_window_snapshot_for_view(Some(&view), display_offset, viewport_rows, None)
            .expect("cached scrolled paint window should be used");

    assert!(terminal_snapshot_covers_display_offset(
        snapshot.as_ref(),
        display_offset,
        viewport_rows,
        scrollback_len
    ));
    assert_eq!(
        snapshot.line(terminal_snapshot_anchor_row_for_display_offset(
            snapshot.as_ref(),
            display_offset,
            viewport_rows,
            scrollback_len
        )),
        view.screen.viewport_snapshot(display_offset).line(0)
    );
}

#[test]
fn terminal_paint_window_snapshot_preserves_view_absolute_start() {
    let mut view = TerminalViewState::from_output(terminal_output_lines(80));
    let display_offset = 4;
    let viewport_rows = view.viewport_rows_for_ui();
    let base = std::sync::Arc::new(view.screen.viewport_snapshot(display_offset));
    view.scrollback_snapshots
        .insert(display_offset, base.clone());
    let (base_start, _) =
        crate::features::terminal::terminal_surface::terminal_snapshot_absolute_range(
            base.as_ref(),
        );

    let snapshot =
        terminal_paint_window_snapshot_for_view(Some(&view), display_offset, viewport_rows, None)
            .expect("window snapshot should be available");
    let anchor = terminal_snapshot_anchor_row_for_display_offset(
        snapshot.as_ref(),
        display_offset,
        viewport_rows,
        view.scrollback_len_for_ui(),
    );
    let (window_start, _) =
        crate::features::terminal::terminal_surface::terminal_snapshot_absolute_range(
            snapshot.as_ref(),
        );

    assert_eq!(window_start + anchor, base_start);
}

#[test]
fn terminal_scroll_text_first_decorations_keep_search_overlay_without_links() {
    let view = TerminalViewState::from_output(terminal_output_lines(80));
    let snapshot = view.screen.viewport_snapshot(6);
    let (abs_start, _) =
        crate::features::terminal::terminal_surface::terminal_snapshot_absolute_range(&snapshot);
    let matches = vec![TerminalBufferMatch {
        line_index: abs_start + 1,
        start_col: 2,
        end_col: 6,
    }];

    let decorations = terminal_scroll_text_first_decorations(
        &snapshot,
        Some(matches.as_slice()),
        &[],
        true,
        true,
    );

    assert_eq!(decorations.len(), snapshot.row_count());
    assert_eq!(decorations[1].search_ranges, vec![(2, 6)]);
    assert!(decorations[1].active_search_ranges.is_empty());
    assert!(decorations[1].link_ranges.is_empty());
}

#[test]
fn terminal_scroll_text_first_decorations_include_links_for_current_snapshot() {
    let mut screen = TerminalScreen::new(40, 3);
    screen.advance(b"\x1b]8;;https://example.com\x07click\x1b]8;;\x07 plain");
    let snapshot = screen.viewport_snapshot(0);
    let (absolute_start_row, absolute_end_row) =
        crate::features::terminal::terminal_surface::terminal_snapshot_absolute_range(&snapshot);
    let mut links = TerminalFrameActionLinks {
        matcher_key: 1,
        absolute_start_row,
        absolute_end_row,
        row_signatures: snapshot.rows().iter().map(|row| row.signature).collect(),
        matches_by_line: vec![Vec::new(); snapshot.row_count()],
        cell_ranges_by_line: vec![Vec::new(); snapshot.row_count()],
    };
    links.cell_ranges_by_line[0].push((6, 11));

    let decorations = terminal_scroll_text_first_decorations(
        &snapshot,
        None,
        std::slice::from_ref(&links),
        true,
        true,
    );

    assert_eq!(decorations.len(), snapshot.row_count());
    assert_eq!(decorations[0].link_ranges, vec![(6, 11), (0, 5)]);
    assert!(decorations[0].search_ranges.is_empty());
}

#[test]
fn terminal_scroll_text_first_decorations_map_covering_action_link_window() {
    let view = TerminalViewState::from_output(terminal_output_lines(80));
    let snapshot = view.screen.viewport_snapshot(6);
    let (snapshot_start, snapshot_end) =
        crate::features::terminal::terminal_surface::terminal_snapshot_absolute_range(&snapshot);
    let mut links = TerminalFrameActionLinks {
        matcher_key: 1,
        absolute_start_row: snapshot_start.saturating_sub(2),
        absolute_end_row: snapshot_end.saturating_add(2),
        row_signatures: vec![0; snapshot.row_count() + 4],
        matches_by_line: vec![Vec::new(); snapshot.row_count() + 4],
        cell_ranges_by_line: vec![Vec::new(); snapshot.row_count() + 4],
    };
    let relative_row = snapshot_start + 1 - links.absolute_start_row;
    links.row_signatures[relative_row] = snapshot.rows()[1].signature;
    links.cell_ranges_by_line[relative_row].push((3, 8));

    let decorations = terminal_scroll_text_first_decorations(
        &snapshot,
        None,
        std::slice::from_ref(&links),
        true,
        true,
    );

    assert_eq!(decorations.len(), snapshot.row_count());
    assert!(decorations[0].link_ranges.is_empty());
    assert_eq!(decorations[1].link_ranges, vec![(3, 8)]);
}

#[test]
fn terminal_scroll_text_first_decorations_keep_partially_covered_bottom_links() {
    let view = TerminalViewState::from_output(terminal_output_lines(80));
    let snapshot = view.screen.viewport_snapshot(6);
    let (snapshot_start, snapshot_end) =
        crate::features::terminal::terminal_surface::terminal_snapshot_absolute_range(&snapshot);
    let top_links = TerminalFrameActionLinks {
        matcher_key: 1,
        absolute_start_row: snapshot_start,
        absolute_end_row: snapshot_start + 1,
        row_signatures: vec![snapshot.rows()[0].signature],
        matches_by_line: vec![Vec::new()],
        cell_ranges_by_line: vec![vec![(1, 4)]],
    };
    let bottom_links = TerminalFrameActionLinks {
        matcher_key: 1,
        absolute_start_row: snapshot_end - 1,
        absolute_end_row: snapshot_end,
        row_signatures: vec![snapshot.rows()[snapshot.row_count() - 1].signature],
        matches_by_line: vec![Vec::new()],
        cell_ranges_by_line: vec![vec![(3, 8)]],
    };

    let decorations = terminal_scroll_text_first_decorations(
        &snapshot,
        None,
        &[top_links, bottom_links],
        true,
        true,
    );

    assert_eq!(decorations.len(), snapshot.row_count());
    assert_eq!(decorations[0].link_ranges, vec![(1, 4)]);
    assert_eq!(
        decorations[snapshot.row_count() - 1].link_ranges,
        vec![(3, 8)]
    );
}

#[test]
fn terminal_keyword_highlight_updates_stay_enabled_for_active_surface() {
    assert!(terminal_keyword_highlight_updates_allowed(true, false));
    assert!(!terminal_keyword_highlight_updates_allowed(true, true));
}

#[test]
fn terminal_keyword_highlight_updates_pause_for_inactive_surface() {
    assert!(!terminal_keyword_highlight_updates_allowed(false, false));
}

#[test]
fn live_action_link_enrichment_waits_for_input_and_output_to_calm() {
    assert!(terminal_live_action_link_enrichment_allowed(
        0, true, false, false
    ));
    assert!(!terminal_live_action_link_enrichment_allowed(
        0, true, true, false
    ));
    assert!(!terminal_live_action_link_enrichment_allowed(
        0, true, false, true
    ));
    assert!(!terminal_live_action_link_enrichment_allowed(
        1, true, false, false
    ));
}

#[test]
fn terminal_user_scroll_active_requires_scrolled_surface_and_recent_input() {
    let now = Instant::now();

    assert!(!terminal_user_scroll_active(0, true, Some(now), now));
    assert!(terminal_user_scroll_active(4, true, Some(now), now));
    assert!(!terminal_user_scroll_active(4, false, Some(now), now));
    assert!(!terminal_user_scroll_active(
        4,
        true,
        Some(now - TERMINAL_USER_SCROLL_ACTIVE_WINDOW - Duration::from_millis(1)),
        now,
    ));
    assert!(!terminal_user_scroll_active(4, true, None, now));
}

#[test]
fn terminal_input_latency_active_uses_short_idle_window() {
    let now = Instant::now();

    assert!(terminal_input_latency_active(Some(now), now));
    assert!(!terminal_input_latency_active(
        Some(now - TERMINAL_INPUT_LATENCY_WINDOW - Duration::from_millis(1)),
        now,
    ));
    assert!(!terminal_input_latency_active(None, now));
}

#[test]
fn legacy_low_latency_setting_does_not_override_individual_feature_switches() {
    let root = TestConfigDir::new("nyaterm-retired-low-latency");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "s1");
    cx.update_entity(&app, |app, cx| {
        let mut summary = app.settings.summary().clone();
        summary.terminal_low_latency_mode = true;
        summary.interaction_command_suggestions_enabled = true;
        summary.terminal_action_links_enabled = true;
        app.settings.replace_summary(summary);
        let mut keywords = app.settings.keyword_config().clone();
        keywords.enabled = true;
        app.settings.replace_keyword_config(keywords);
        app.invalidate_paint_theme_caches();

        app.note_shell_editing_input(b"echo abc", cx);
        app.note_command_suggestion_input(b"echo abc", cx);
        assert!(
            app.terminal
                .assist
                .command_suggestion_refresh_task
                .is_some()
        );
        assert!(!app.resolved_keyword_highlight_rules().is_empty());
        app.ensure_paint_theme_caches();
        assert!(!app.resolved_keyword_highlight_rules().is_empty());
        let snapshot = app.terminal.view.views["s1"].screen.viewport_snapshot(0);
        assert!(app.request_terminal_live_action_link_enrichment("s1", &snapshot));

        let mut summary = app.settings.summary().clone();
        summary.interaction_command_suggestions_enabled = false;
        summary.terminal_action_links_enabled = false;
        app.settings.replace_summary(summary);
        let mut keywords = app.settings.keyword_config().clone();
        keywords.enabled = false;
        app.settings.replace_keyword_config(keywords);
        app.invalidate_paint_theme_caches();

        app.note_command_suggestion_input(b"echo abc", cx);
        assert!(
            app.terminal
                .assist
                .command_suggestion_refresh_task
                .is_none()
        );
        assert!(app.resolved_keyword_highlight_rules().is_empty());
        app.ensure_paint_theme_caches();
        assert!(app.resolved_keyword_highlight_rules().is_empty());
        assert!(!app.request_terminal_live_action_link_enrichment("s1", &snapshot));
        assert!(app.settings.summary().terminal_low_latency_mode);
    });
}

#[test]
fn terminal_command_suggestion_input_tracking_requires_tracking_opt_in() {
    assert!(terminal_should_track_command_suggestion_input(true, true));
    assert!(!terminal_should_track_command_suggestion_input(false, true));
}

#[test]
fn terminal_command_suggestion_input_tracking_skips_disabled_suggestions() {
    assert!(!terminal_should_track_command_suggestion_input(true, false));
}

#[test]
fn terminal_session_write_failure_log_escapes_control_text() {
    let log = terminal_session_write_failure_log("input", "closed\r\n\x1b[31m");

    assert_eq!(
        log,
        "\n# session write failed (input): closed\\r\\n\\x1b[31m\n"
    );
}

#[test]
fn terminal_key_encoding_uses_target_session_mode() {
    let event = key_event("up", None, gpui::Modifiers::default());
    let normal =
        terminal_key_bytes_for_mode_and_settings(&event, TerminalKeyMode::default(), false)
            .unwrap();
    let application = terminal_key_bytes_for_mode_and_settings(
        &event,
        TerminalKeyMode {
            application_cursor: true,
            ..TerminalKeyMode::default()
        },
        false,
    )
    .unwrap();

    assert_eq!(normal, b"\x1b[A".to_vec());
    assert_eq!(application, b"\x1bOA".to_vec());
}

#[test]
fn terminal_key_encoding_sends_plain_tab_to_pty() {
    let event = key_event("tab", None, gpui::Modifiers::default());

    assert_eq!(
        terminal_key_bytes_for_mode_and_settings(&event, TerminalKeyMode::default(), false)
            .unwrap(),
        b"\t".to_vec()
    );
}

#[test]
fn terminal_key_encoding_sends_shift_tab_to_pty() {
    let event = key_event(
        "tab",
        None,
        gpui::Modifiers {
            shift: true,
            ..gpui::Modifiers::default()
        },
    );

    assert_eq!(
        terminal_key_bytes_for_mode_and_settings(&event, TerminalKeyMode::default(), false)
            .unwrap(),
        b"\x1b[Z".to_vec()
    );
}

#[test]
fn terminal_key_encoding_keeps_alt_meta_setting_outside_mode() {
    let event = key_event(
        "x",
        Some("x"),
        gpui::Modifiers {
            alt: true,
            ..gpui::Modifiers::default()
        },
    );

    assert_eq!(
        terminal_key_bytes_for_mode_and_settings(&event, TerminalKeyMode::default(), true,)
            .unwrap(),
        b"\x1bx".to_vec()
    );
    assert!(
        terminal_key_bytes_for_mode_and_settings(&event, TerminalKeyMode::default(), false,)
            .is_none()
    );
}

#[test]
fn terminal_key_encoding_uses_layout_key_for_option_meta_shortcuts() {
    for (key, key_char, expected) in [
        (".", Some("≥"), "\x1b."),
        ("b", Some("∫"), "\x1bb"),
        ("f", Some("ƒ"), "\x1bf"),
        ("d", Some("∂"), "\x1bd"),
        ("b", Some("��"), "\x1bb"),
        ("e", None, "\x1be"),
        ("space", Some("\u{a0}"), "\x1b "),
    ] {
        let event = key_event(key, key_char, gpui::Modifiers::alt());

        assert_eq!(
            terminal_key_bytes_for_mode_and_settings(&event, TerminalKeyMode::default(), true),
            Some(expected.as_bytes().to_vec()),
            "Option+{key}"
        );
    }
}

#[test]
fn terminal_key_encoding_preserves_shift_and_layout_characters_for_meta() {
    for (key, key_char, shift, expected) in [
        ("b", "ı", true, "\x1bB"),
        (">", "˘", false, "\x1b>"),
        ("é", "´", false, "\x1bé"),
        ("Ö", "Ø", false, "\x1bÖ"),
    ] {
        let event = key_event(
            key,
            Some(key_char),
            gpui::Modifiers {
                alt: true,
                shift,
                ..gpui::Modifiers::default()
            },
        );

        assert_eq!(
            terminal_key_bytes_for_mode_and_settings(&event, TerminalKeyMode::default(), true),
            Some(expected.as_bytes().to_vec())
        );
    }
}

#[test]
fn terminal_key_encoding_preserves_protocol_sequences_with_meta_enabled() {
    let event = key_event("b", Some("∫"), gpui::Modifiers::alt());
    let mode = TerminalKeyMode {
        kitty_keyboard_disambiguate: true,
        ..TerminalKeyMode::default()
    };
    assert_eq!(
        terminal_key_bytes_for_mode_and_settings(&event, mode, true),
        Some(b"\x1b[98;3u".to_vec())
    );

    for (key, key_char, expected) in [
        ("left", None, b"\x1b[1;3D".as_slice()),
        ("backspace", None, b"\x1b\x7f".as_slice()),
        ("tab", Some("\t"), b"\x1b\t".as_slice()),
    ] {
        let event = key_event(key, key_char, gpui::Modifiers::alt());
        assert_eq!(
            terminal_key_bytes_for_mode_and_settings(&event, TerminalKeyMode::default(), true),
            Some(expected.to_vec())
        );
    }
}

#[test]
fn terminal_key_encoding_preserves_word_shortcuts_with_meta_disabled() {
    for (key, key_char) in [("b", "∫"), ("f", "ƒ"), ("d", "∂")] {
        let event = key_event(key, Some(key_char), gpui::Modifiers::alt());
        assert_eq!(
            terminal_key_bytes_for_mode_and_settings(&event, TerminalKeyMode::default(), false),
            Some(vec![0x1b, key.as_bytes()[0]])
        );
    }
    let event = key_event(".", Some("≥"), gpui::Modifiers::alt());
    assert!(
        terminal_key_bytes_for_mode_and_settings(&event, TerminalKeyMode::default(), false)
            .is_none()
    );
}

#[test]
fn ime_defer_leaves_option_shortcuts_for_terminal_encoding() {
    let event = key_event("b", Some("∫"), gpui::Modifiers::alt());
    for marked_text in ["", "五笔"] {
        assert!(!terminal_should_defer_key_text_to_input_handler_for_state(
            marked_text,
            &event
        ));
    }
}

#[test]
fn ime_defer_does_not_swallow_plain_space_without_marked_text() {
    let event = key_event("space", None, gpui::Modifiers::default());

    assert!(!terminal_should_defer_key_text_to_input_handler_for_state(
        "", &event
    ));
}

#[test]
fn ime_defer_keeps_space_for_active_marked_text() {
    let event = key_event("space", None, gpui::Modifiers::default());

    assert!(terminal_should_defer_key_text_to_input_handler_for_state(
        "ni", &event
    ));
}

#[test]
fn ime_defer_keeps_non_ascii_text_for_input_handler() {
    let event = key_event("あ", Some("あ"), gpui::Modifiers::default());

    assert!(terminal_should_defer_key_text_to_input_handler_for_state(
        "", &event
    ));
}

#[test]
fn terminal_status_changed_detects_identical_text() {
    assert!(!terminal_status_changed("sent 1 byte(s)", "sent 1 byte(s)"));
    assert!(terminal_status_changed("idle", "sent 1 byte(s)"));
}

#[test]
fn rejected_input_and_paste_leave_recording_empty_and_preserve_session_encoding() {
    use crate::models::SessionLaunchConfig;
    let root = TestConfigDir::new("nyaterm-rejected-encoding-input");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "encoded");
    cx.update_entity(&app, |app, cx| {
        let mut metadata = app
            .session
            .metadata_entries()
            .find(|(id, _)| *id == "encoded")
            .unwrap()
            .1
            .clone();
        if let SessionLaunchConfig::Local(config) = &mut metadata.launch_config {
            config.encoding = "CP936".into();
        }
        app.session.register_session_metadata("encoded", metadata);
        assert_eq!(app.effective_session_encoding("encoded"), "CP936");
        assert_eq!(
            app.encode_session_outgoing("encoded", "测试".as_bytes())
                .unwrap(),
            [0xb2, 0xe2, 0xca, 0xd4]
        );
        let mut summary = app.settings.summary().clone();
        summary.interaction_default_encoding = "UTF-8".into();
        app.settings.replace_summary(summary);
        app.sync_terminal_encodings_from_settings();
        assert_eq!(
            app.terminal.view.views["encoded"].screen.encoding_label(),
            "GBK"
        );
        assert!(
            app.write_session_input_recorded("encoded", "secret😀".as_bytes())
                .is_err()
        );
        assert!(
            app.write_session_sensitive_input("encoded", "secret😀".as_bytes())
                .is_err()
        );
        assert!(
            app.wrap_terminal_paste_bytes_for_session("encoded", "secret😀")
                .is_err()
        );
        app.send_terminal_paste_input("secret😀", cx);
        let path = root.path().join("transcript.txt");
        let result = app.recording.writer().save_transcript(
            "encoded".into(),
            path.to_string_lossy().into_owned(),
            false,
            false,
            1024 * 1024,
        );
        result.unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "");
        assert_eq!(
            app.terminal.view.views["encoded"].screen.encoding_label(),
            "GBK"
        );
    });
}

#[test]
fn paste_targets_use_their_own_charset_and_bracketed_mode() {
    use crate::models::SessionLaunchConfig;
    let root = TestConfigDir::new("nyaterm-paste-charset-targets");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "gbk");
    cx.update_entity(&app, |app, _| {
        let mut metadata = app
            .session
            .metadata_entries()
            .find(|(id, _)| *id == "gbk")
            .unwrap()
            .1
            .clone();
        if let SessionLaunchConfig::Local(config) = &mut metadata.launch_config {
            config.encoding = "GBK".into();
        }
        app.session
            .register_session_metadata("gbk", metadata.clone());
        if let SessionLaunchConfig::Local(config) = &mut metadata.launch_config {
            config.encoding = "UTF-8".into();
        }
        app.session.register_session_metadata("utf8", metadata);
        app.terminal
            .seed_session_view("utf8".into(), String::new(), "UTF-8");
        app.terminal
            .view
            .views
            .get_mut("gbk")
            .unwrap()
            .protocol_state
            .bracketed_paste = true;
        assert_eq!(
            app.wrap_terminal_paste_bytes_for_session("gbk", "测试")
                .unwrap(),
            b"\x1b[200~\xb2\xe2\xca\xd4\x1b[201~"
        );
        assert_eq!(
            app.wrap_terminal_paste_bytes_for_session("utf8", "测试😀")
                .unwrap(),
            "测试😀".as_bytes()
        );
        assert!(
            app.wrap_terminal_paste_bytes_for_session("gbk", "😀")
                .is_err()
        );
    });
}

#[test]
fn telnet_gpui_enter_submits_locally_edited_text_in_the_session_charset() {
    use std::io::Read;
    use std::net::TcpListener;

    use nyaterm_core::character_encoding::CharacterEncoding;
    use nyaterm_transport::{TelnetEnterMode, TelnetSessionConfig};

    use crate::models::SessionLaunchConfig;

    let root = TestConfigDir::new("nyaterm-telnet-local-input");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "fixture");
    let enter = terminal_key_bytes_for_mode_and_settings(
        &key_event("enter", None, gpui::Modifiers::default()),
        TerminalKeyMode::default(),
        false,
    )
    .unwrap();
    let backspace = terminal_key_bytes_for_mode_and_settings(
        &key_event("backspace", None, gpui::Modifiers::default()),
        TerminalKeyMode::default(),
        false,
    )
    .unwrap();
    assert_eq!(enter, b"\r");

    for raw_tcp in [false, true] {
        for encoding in [CharacterEncoding::Utf8, CharacterEncoding::Gbk] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let config = TelnetSessionConfig {
                host: "127.0.0.1".into(),
                port: listener.local_addr().unwrap().port(),
                raw_tcp,
                local_line_edit: true,
                local_echo: true,
                enter_mode: TelnetEnterMode::Crlf,
                encoding: encoding.label().into(),
                send_naws: false,
                keep_alive_interval_secs: 0,
                auto_login: nyaterm_transport::TelnetAutoLoginConfig {
                    enabled: false,
                    ..Default::default()
                },
                ..Default::default()
            };
            let (manager, id) = cx.update_entity(&app, |app, _| {
                let manager = app.session.manager_handle();
                let info = manager.create_telnet_session(config.clone()).unwrap();
                let mut metadata = app.session.metadata("fixture").unwrap().clone();
                metadata.launch_config = SessionLaunchConfig::Telnet(config);
                app.session.register_session_metadata(&info.id, metadata);
                app.terminal
                    .seed_session_view(info.id.clone(), String::new(), encoding.label());
                (manager, info.id)
            });
            let (mut peer, _) = listener.accept().unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            cx.update_entity(&app, |app, _| {
                app.write_session_input_recorded(&id, "中中".as_bytes())
                    .unwrap();
                app.write_session_input_recorded(&id, &backspace).unwrap();
                app.write_session_input_recorded(&id, &enter).unwrap();
            });
            let mut expected = encoding.encode("中").unwrap();
            expected.extend_from_slice(b"\r\n");
            let mut actual = vec![0; expected.len()];
            peer.read_exact(&mut actual).unwrap();
            assert_eq!(actual, expected);
            manager.close(&id).unwrap();
        }
    }
}

#[test]
fn reconnect_drops_old_enter_release_and_repeat_even_after_new_kitty_negotiation() {
    use std::io::Read;
    use std::net::TcpListener;

    use gpui::KeyUpEvent;
    use nyaterm_transport::TelnetSessionConfig;

    use crate::features::terminal::terminal_runtime::view_io::TerminalMouseReportRequest;
    use crate::models::{SessionLaunchConfig, TerminalProtocolState};

    let root = TestConfigDir::new("nyaterm-reconnect-key-wire");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "fixture");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (manager, id) = cx.update_entity(&app, |app, _| {
        let config = TelnetSessionConfig {
            host: "127.0.0.1".into(),
            port: listener.local_addr().unwrap().port(),
            raw_tcp: true,
            ..TelnetSessionConfig::default()
        };
        let manager = app.session.manager_handle();
        let info = manager.create_telnet_session(config.clone()).unwrap();
        let mut metadata = app.session.metadata("fixture").unwrap().clone();
        metadata.launch_config = SessionLaunchConfig::Telnet(config);
        app.session.register_session_metadata(&info.id, metadata);
        app.terminal
            .seed_session_view("old".into(), "\x1b[>3u\x1b[?1003;1006hOLD".into(), "UTF-8");
        app.terminal
            .view
            .views
            .get_mut("old")
            .unwrap()
            .pressed_keys
            .insert("enter".into());
        app.terminal.rekey_session_view("old", &info.id, "UTF-8");
        app.session.select_active_session(&info.id);
        (manager, info.id)
    });
    let (mut peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    cx.update_entity(&app, |app, cx| {
        let press = key_event("enter", None, gpui::Modifiers::default());
        let release = KeyUpEvent {
            keystroke: press.keystroke.clone(),
        };
        assert_eq!(
            app.terminal_key_bytes_for_event_for_session(Some(&id), &press),
            Some(b"\r".to_vec())
        );
        assert!(!app.send_terminal_key_release_event(&release, cx));
        for require_alt in [false, true] {
            let mut settings = app.settings.summary().clone();
            settings.interaction_mouse_events_require_alt = require_alt;
            app.settings.replace_summary(settings);
            for modifiers in [gpui::Modifiers::default(), gpui::Modifiers::alt()] {
                assert!(!app.maybe_send_mouse_report_for_session(
                    TerminalMouseReportRequest {
                        session_id: &id,
                        button: 3,
                        col: 1,
                        row: 1,
                        press: true,
                        motion: true,
                        modifiers,
                    },
                    cx,
                ));
            }
        }

        // A fresh TUI can enable Kitty before the reconnecting Enter is released.
        let view = app.terminal.view.views.get_mut(&id).unwrap();
        view.screen.advance(b"\x1b[>3u");
        view.protocol_state = TerminalProtocolState::from_screen(&view.screen);
        let mut repeat = press.clone();
        repeat.is_held = true;
        assert!(!app.send_terminal_key_event(&repeat, false, cx));
        assert!(!app.send_terminal_key_release_event(&release, cx));
        assert!(app.send_terminal_key_event(&press, false, cx));
        assert!(app.send_terminal_key_release_event(&release, cx));
        // A duplicate or unrelated release must not be written either.
        assert!(!app.send_terminal_key_release_event(&release, cx));
        app.write_session_protocol_response(&id, b"END").unwrap();
    });
    let expected = b"\x1b[13;1:1u\x1b[13;1:3uEND";
    let mut actual = vec![0; expected.len()];
    peer.read_exact(&mut actual).unwrap();
    assert_eq!(actual, expected);
    manager.close(&id).unwrap();
}

#[test]
fn encoding_failures_write_nothing_and_mouse_and_protocol_bytes_bypass_charset() {
    use crate::features::terminal::terminal_runtime::view_io::TerminalMouseReportRequest;
    use crate::models::SessionLaunchConfig;
    use nyaterm_core::terminal::editing::{EditCapability, EditPhase};
    use nyaterm_core::terminal::input_tracker::InputSelectionRange;
    use nyaterm_terminal::editing::InputMapping;
    use nyaterm_transport::TelnetSessionConfig;
    use std::io::Read;
    use std::net::TcpListener;

    let root = TestConfigDir::new("nyaterm-encoding-wire");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "fixture");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (manager, id) = cx.update_entity(&app, |app, _| {
        let config = TelnetSessionConfig {
            host: "127.0.0.1".into(),
            port,
            raw_tcp: true,
            encoding: "GBK".into(),
            ..TelnetSessionConfig::default()
        };
        let manager = app.session.manager_handle();
        let info = manager.create_telnet_session(config.clone()).unwrap();
        let mut metadata = app
            .session
            .metadata_entries()
            .find(|(id, _)| *id == "fixture")
            .unwrap()
            .1
            .clone();
        metadata.launch_config = SessionLaunchConfig::Telnet(config);
        app.session.register_session_metadata(&info.id, metadata);
        app.session.select_active_session(&info.id);
        app.terminal
            .seed_session_view(info.id.clone(), String::new(), "GBK");
        app.terminal.editing.activate(&info.id);
        (manager, info.id)
    });
    let (mut stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let expected = cx.update_entity(&app, |app, cx| {
        let state = app.terminal.editing.state_mut();
        state.model.input.value = "abcdef".into();
        state.model.input.cursor = 6;
        state.model.phase = EditPhase::Ready;
        state.capability = EditCapability::MarkedShell;
        state.awaiting_snapshot = false;
        state.mapping = Some(InputMapping {
            cells: vec![],
            end: (0, 0),
            row_revisions: vec![],
            anchor: None,
        });
        let version = state.model.version;
        let mapping = state.mapping.clone();
        assert!(app.can_use_smart_cursor_selection());
        assert!(!app.send_terminal_input("secret😀".as_bytes().to_vec(), cx));
        assert!(
            app.write_session_sensitive_input(&id, "secret😀".as_bytes())
                .is_err()
        );
        assert!(app.write_session_input_recorded(&id, &[0xff]).is_err());
        app.send_terminal_paste_input("secret😀", cx);
        for paste in [false, true] {
            let range = InputSelectionRange::new(1, 4).unwrap();
            assert!(if paste {
                app.replace_smart_input_selection_with_paste(range, "😀", cx)
            } else {
                app.replace_smart_input_selection(range, "😀", cx)
            });
            let state = app.terminal.editing.state().unwrap();
            assert_eq!(state.model.input.value, "abcdef");
            assert_eq!(state.model.input.cursor, 6);
            assert_eq!(state.model.phase, EditPhase::Ready);
            assert_eq!(state.model.version, version);
            assert_eq!(state.mapping, mapping);
            assert!(state.pending_evidence.is_none());
        }
        app.recording.write_output(id.clone(), "baseline");
        let recording_path = root.path().join("rejected.txt");
        app.recording
            .writer()
            .save_transcript(
                id.clone(),
                recording_path.to_string_lossy().into_owned(),
                false,
                false,
                1024 * 1024,
            )
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(recording_path).unwrap(),
            "baseline\n"
        );
        let mut settings = app.settings.summary().clone();
        settings.interaction_mouse_events_require_alt = false;
        app.settings.replace_summary(settings);
        let mut expected = Vec::new();
        // Exercise the actual mouse UI adapter, including invalid UTF-8 X10 bytes.
        for (sgr, utf8, col, row, wire) in [
            (true, false, 100, 110, b"\x1b[<0;101;111M".as_slice()),
            (false, false, 100, 110, b"\x1b[M \x85\x8f".as_slice()),
            (false, true, 100, 110, b"\x1b[M \xc2\x85\xc2\x8f".as_slice()),
        ] {
            let protocol = &mut app.terminal.view.views.get_mut(&id).unwrap().protocol_state;
            protocol.mouse_reporting = true;
            protocol.mouse_sgr = sgr;
            protocol.mouse_utf8 = utf8;
            assert!(app.maybe_send_mouse_report_for_session(
                TerminalMouseReportRequest {
                    session_id: &id,
                    button: 0,
                    col,
                    row,
                    press: true,
                    motion: false,
                    modifiers: gpui::Modifiers::default(),
                },
                cx
            ));
            expected.extend_from_slice(wire);
        }
        for reply in [
            b"\x1b[2;3R".as_slice(),
            b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\".as_slice(),
            b"\x1b_Gi=7;OK\x1b\\".as_slice(),
        ] {
            app.write_session_protocol_response(&id, reply).unwrap();
            expected.extend_from_slice(reply);
        }
        app.terminal
            .view
            .views
            .get_mut(&id)
            .unwrap()
            .protocol_state
            .bracketed_paste = true;
        app.send_terminal_paste_input("测试", cx);
        expected.extend_from_slice(b"\x1b[200~\xb2\xe2\xca\xd4\x1b[201~");
        app.write_session_protocol_response(&id, b"END").unwrap();
        expected.extend_from_slice(b"END");
        expected
    });
    let mut actual = vec![0; expected.len()];
    stream.read_exact(&mut actual).unwrap();
    assert_eq!(actual, expected);
    manager.close(&id).unwrap();
}

#[test]
fn sync_paste_encodes_each_live_target_and_stops_when_primary_cannot_encode() {
    use crate::models::SessionLaunchConfig;
    use nyaterm_transport::TelnetSessionConfig;
    use std::io::Read;
    use std::net::TcpListener;

    let root = TestConfigDir::new("nyaterm-sync-paste-wire");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "fixture");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (manager, ids) = cx.update_entity(&app, |app, _| {
        let manager = app.session.manager_handle();
        let metadata = app
            .session
            .metadata_entries()
            .find(|(id, _)| *id == "fixture")
            .unwrap()
            .1
            .clone();
        let mut ids = Vec::new();
        app.sync_input.create_group();
        for encoding in ["GB18030", "UTF-8", "GBK"] {
            let config = TelnetSessionConfig {
                host: "127.0.0.1".into(),
                port,
                raw_tcp: true,
                encoding: encoding.into(),
                ..TelnetSessionConfig::default()
            };
            let info = manager.create_telnet_session(config.clone()).unwrap();
            let mut metadata = metadata.clone();
            metadata.launch_config = SessionLaunchConfig::Telnet(config);
            app.session.register_session_metadata(&info.id, metadata);
            app.terminal
                .seed_session_view(info.id.clone(), String::new(), encoding);
            app.sync_input.toggle_selected_session(info.id.clone());
            ids.push(info.id);
        }
        app.terminal
            .view
            .views
            .get_mut(&ids[1])
            .unwrap()
            .protocol_state
            .bracketed_paste = true;
        app.session.select_active_session(&ids[0]);
        (manager, ids)
    });
    let mut streams = (0..3)
        .map(|_| {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            stream
        })
        .collect::<Vec<_>>();
    cx.update_entity(&app, |app, cx| {
        assert_eq!(app.sync_peer_session_ids(&ids[0]).len(), 2);
        app.send_terminal_paste_input("测试😀", cx);
        assert_eq!(
            app.shell.status(),
            nyaterm_core::terminal_input_fanout_status("pasted", 8, 1, 1)
        );
        // A failed primary must stop fanout before either successful peer sees text.
        app.session.select_active_session(&ids[2]);
        app.send_terminal_paste_input("😀", cx);
        for id in &ids {
            app.write_session_protocol_response(id, b"END").unwrap();
        }
        let path = root.path().join("failed-peer.txt");
        app.recording
            .writer()
            .save_transcript(
                ids[2].clone(),
                path.to_string_lossy().into_owned(),
                false,
                false,
                1024 * 1024,
            )
            .unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "");
    });
    let mut gb18030 = nyaterm_core::character_encoding::CharacterEncoding::Gb18030
        .encode("测试😀")
        .unwrap();
    gb18030.extend_from_slice(b"END");
    let mut utf8 = b"\x1b[200~".to_vec();
    utf8.extend_from_slice("测试😀".as_bytes());
    utf8.extend_from_slice(b"\x1b[201~END");
    for (stream, expected) in streams.iter_mut().zip([gb18030, utf8, b"END".to_vec()]) {
        let mut actual = vec![0; expected.len()];
        stream.read_exact(&mut actual).unwrap();
        assert_eq!(actual, expected);
    }
    for id in ids {
        manager.close(&id).unwrap();
    }
}

#[test]
fn recording_sensitive_prompts_are_session_local_for_sync_keyboard_paste_and_raw_input() {
    use crate::models::SessionLaunchConfig;
    use nyaterm_transport::{
        ExistingFileBehavior, RecordingContext, RecordingMode, RecordingProfile,
        RecordingRotationPolicy, TelnetSessionConfig,
    };
    use std::io::Read;
    use std::net::TcpListener;

    let root = TestConfigDir::new("nyaterm-sensitive-recording-wire");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "fixture");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (manager, ids) = cx.update_entity(&app, |app, _| {
        let manager = app.session.manager_handle();
        let metadata = app.session.metadata("fixture").unwrap().clone();
        let mut ids = Vec::new();
        app.sync_input.create_group();
        for index in 0..2 {
            let config = TelnetSessionConfig {
                host: "127.0.0.1".into(),
                port,
                raw_tcp: true,
                encoding: "GBK".into(),
                ..Default::default()
            };
            let info = manager.create_telnet_session(config.clone()).unwrap();
            let mut metadata = metadata.clone();
            metadata.launch_config = SessionLaunchConfig::Telnet(config);
            app.session.register_session_metadata(&info.id, metadata);
            app.terminal
                .seed_session_view(info.id.clone(), String::new(), "GBK");
            app.sync_input.toggle_selected_session(info.id.clone());
            let path = root.path().join(format!("{index}.bin"));
            app.recording
                .writer()
                .start(
                    info.id.clone(),
                    RecordingContext {
                        session_id: info.id.clone(),
                        session_name: "synthetic".into(),
                        connection_id: None,
                        connection_name: None,
                        group_path: None,
                        protocol: "telnet".into(),
                        host: None,
                        port: None,
                        username: None,
                        started_at: time::OffsetDateTime::now_utc(),
                    },
                    RecordingProfile {
                        mode: RecordingMode::Raw,
                        base_path: root.path().into(),
                        path_template: "unused.log".into(),
                        include_timestamps: false,
                        include_io_labels: false,
                        include_session_metadata: false,
                        rotation: RecordingRotationPolicy::Session,
                        existing_file_behavior: ExistingFileBehavior::Unique,
                        include_binary_transfer_payloads: false,
                        include_input: true,
                    },
                    Some(path),
                    4096,
                )
                .unwrap();
            ids.push(info.id);
        }
        app.session.select_active_session(&ids[0]);
        (manager, ids)
    });
    let mut streams = (0..2)
        .map(|_| listener.accept().unwrap().0)
        .collect::<Vec<_>>();
    for stream in &streams {
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
    }
    cx.update_entity(&app, |app, cx| {
        app.observe_recording_prompt_output(&ids[0], "Password: ");
        app.observe_recording_prompt_output(&ids[1], "$ ");
        assert!(app.send_terminal_input_without_suggestion_track(b"a".to_vec(), cx));
        app.send_terminal_paste_input("测试", cx);
        app.write_session_raw_input_recorded(&ids[0], b"r").unwrap();
        app.write_session_raw_input_recorded(&ids[1], b"r").unwrap();
        app.observe_recording_prompt_output(&ids[0], "\n$ ");
        app.observe_recording_prompt_output(&ids[1], "\nOTP: ");
        assert!(app.send_terminal_input_without_suggestion_track(b"b".to_vec(), cx));
        for id in &ids {
            app.write_session_sensitive_input(id, b"dedicated").unwrap();
        }
        for (index, id) in ids.iter().enumerate() {
            let completion = app.recording.writer().stop_complete(id.clone()).unwrap();
            assert!(completion.error.is_none());
            let data = std::fs::read(completion.input_file_path.unwrap()).unwrap();
            assert_eq!(
                data,
                if index == 0 {
                    b"b".as_slice()
                } else {
                    b"a\xb2\xe2\xca\xd4r".as_slice()
                }
            );
        }
    });
    for stream in &mut streams {
        let mut data = vec![0; 1 + 4 + 1 + 1 + 9];
        stream.read_exact(&mut data).unwrap();
        assert_eq!(data, b"a\xb2\xe2\xca\xd4rbdedicated");
    }
    for id in ids {
        manager.close(&id).unwrap();
    }
}

#[test]
fn retained_snapshot_from_before_history_rotation_is_not_painted() {
    let mut view = TerminalViewState::new();
    view.screen = TerminalScreen::new(16, 3);
    view.screen.set_scrollback_limit(8);
    view.append_text("0\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8\r\n9\r\n10\r\n");
    let old = Arc::new(view.screen.viewport_snapshot(3));
    view.append_text("11\r\n");
    assert!(!terminal_retained_snapshot_matches_view(&old, &view, 3, 3));
    assert!(terminal_paint_window_snapshot_for_view(Some(&view), 3, 3, Some(old)).is_none());
}

#[test]
fn cached_mouse_drag_targets_cannot_bypass_target_exclusion() {
    use super::TerminalMouseReportRequest;
    use crate::models::SessionLaunchConfig;
    use nyaterm_transport::TelnetSessionConfig;
    use std::io::Read;
    use std::net::TcpListener;

    let root = TestConfigDir::new("nyaterm-sync-mouse-exclusion");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "fixture");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (manager, ids) = cx.update_entity(&app, |app, _| {
        let manager = app.session.manager_handle();
        let metadata = app.session.metadata("fixture").unwrap().clone();
        let mut ids = Vec::new();
        for _ in 0..2 {
            let config = TelnetSessionConfig {
                host: "127.0.0.1".into(),
                port: listener.local_addr().unwrap().port(),
                raw_tcp: true,
                ..Default::default()
            };
            let info = manager.create_telnet_session(config.clone()).unwrap();
            let mut metadata = metadata.clone();
            metadata.launch_config = SessionLaunchConfig::Telnet(config);
            app.session.register_session_metadata(&info.id, metadata);
            app.terminal
                .seed_session_view(info.id.clone(), String::new(), "UTF-8");
            ids.push(info.id);
        }
        app.session.metadata_mut("fixture").unwrap().disconnected = true;
        app.session.select_active_session(&ids[0]);
        (manager, ids)
    });
    let mut streams = (0..2)
        .map(|_| {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            stream
        })
        .collect::<Vec<_>>();
    cx.update_entity(&app, |app, cx| {
        app.sync_input.toggle_broadcast_to_all();
        let mut settings = app.settings.summary().clone();
        settings.interaction_mouse_events_require_alt = false;
        app.settings.replace_summary(settings);
        for id in &ids {
            let protocol = &mut app.terminal.view.views.get_mut(id).unwrap().protocol_state;
            protocol.mouse_reporting = true;
            protocol.mouse_sgr = true;
            protocol.mouse_drag_reporting = true;
        }
        let source = &ids[0];
        assert!(app.maybe_send_mouse_report_for_session(
            TerminalMouseReportRequest {
                session_id: source,
                button: 0,
                col: 1,
                row: 1,
                press: true,
                motion: false,
                modifiers: Default::default()
            },
            cx
        ));
        assert_eq!(
            app.terminal.selection.mouse_report_peer_session_ids,
            [ids[1].clone()]
        );
        app.set_sessions_receive_sync_input(&[ids[1].clone()], false, cx);
        assert!(app.maybe_send_mouse_report_for_session(
            TerminalMouseReportRequest {
                session_id: source,
                button: 0,
                col: 2,
                row: 2,
                press: true,
                motion: true,
                modifiers: Default::default()
            },
            cx
        ));
        assert!(app.maybe_send_mouse_report_for_session(
            TerminalMouseReportRequest {
                session_id: source,
                button: 0,
                col: 2,
                row: 2,
                press: false,
                motion: false,
                modifiers: Default::default()
            },
            cx
        ));
        app.write_session_protocol_response(&ids[1], b"END")
            .unwrap();
    });
    let expected = b"\x1b[<0;2;2MEND";
    let mut actual = vec![0; expected.len()];
    streams[1].read_exact(&mut actual).unwrap();
    assert_eq!(actual, expected);
    for id in ids {
        let _ = manager.close(&id);
    }
}
