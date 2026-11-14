use nyaterm_terminal::output::TERMINAL_OUTPUT_VISIBLE_BACKLOG_CAP;

use super::TerminalFrameOutputEvent;

use super::{
    TERMINAL_PERFORMANCE_RECOVERY_NOTICE, TERMINAL_RENDER_DEGRADATION_RECOVERY_CALM,
    TERMINAL_SCROLLBACK_SNAPSHOT_CACHE_LIMIT, TERMINAL_UI_OUTPUT_TAIL_CAP,
    TerminalFrameActionLinks, TerminalFrameParts, TerminalPerformanceMode,
    TerminalPerformanceOverlay, TerminalProtocolState, TerminalViewState,
    append_terminal_ui_output_tail, terminal_expensive_interactions_enabled,
    terminal_frame_scroll_window_extra_rows, terminal_snapshot_matches_grid_geometry,
};

use crate::terminal::terminal_screen_from_output;
use nyaterm_core::ActionLinksMatcherSettings;
use nyaterm_terminal::action_links::prepare_terminal_frame_action_links;
use nyaterm_terminal::{TerminalEffects, TerminalScreen, TerminalSnapshot};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[test]
fn line_number_gutter_keeps_high_water_width_after_scrollback_prunes() {
    let mut view = TerminalViewState::new();
    let mut snapshot = TerminalScreen::default().viewport_snapshot(0);
    snapshot.total_rows = 1000;
    view.observe_line_number_digits(&snapshot);
    assert_eq!(view.max_line_number_digits, 4);
    snapshot.total_rows = 9;
    view.observe_line_number_digits(&snapshot);
    assert_eq!(view.max_line_number_digits, 4);
}

#[test]
fn clear_all_resets_line_number_gutter_high_water_width() {
    let mut view = TerminalViewState::new();
    let mut snapshot = TerminalScreen::default().viewport_snapshot(0);
    snapshot.total_rows = 100_000;
    view.observe_line_number_digits(&snapshot);
    assert_eq!(view.max_line_number_digits, 6);

    let cleared = TerminalScreen::default().viewport_snapshot(0);
    let expected = cleared.total_rows.max(1).to_string().len();
    view.clear_presentation_except_input(2, &cleared);
    assert_eq!(view.max_line_number_digits, expected);

    // Output after the clear keeps numbering small; the width stays restored.
    view.observe_line_number_digits(&cleared);
    assert_eq!(view.max_line_number_digits, expected);
}

fn output_frame_with_sizes(
    accepted_bytes: usize,
    skipped_output_bytes: usize,
) -> TerminalFrameOutputEvent {
    TerminalFrameOutputEvent {
        session_id: "s1".to_string(),
        visible_text: "x".to_string(),
        recording_text_bytes: 1,
        snapshot: Some(Arc::new(TerminalScreen::default().viewport_snapshot(0))),
        action_links: None,
        protocol_state: TerminalProtocolState::default(),
        effects: TerminalEffects::default(),
        command_running: false,
        accepted_bytes,
        skipped_output_bytes,
        revision: 1,
        snapshot_duration: Duration::ZERO,
        snapshot_stats: Default::default(),
        process_duration: Duration::ZERO,
    }
}

fn apply_output_frame_to_view(view: &mut TerminalViewState, frame: TerminalFrameOutputEvent) {
    let TerminalFrameOutputEvent {
        visible_text,
        snapshot,
        action_links,
        protocol_state,
        accepted_bytes,
        skipped_output_bytes,
        revision,
        ..
    } = frame;
    let Some(snapshot) = snapshot else {
        view.apply_terminal_background_frame_parts(
            &visible_text,
            None,
            None,
            protocol_state,
            skipped_output_bytes,
            revision,
        );
        return;
    };
    view.apply_terminal_frame_parts(TerminalFrameParts {
        visible_text: &visible_text,
        snapshot,
        action_links,
        protocol_state,
        accepted_bytes,
        skipped_output_bytes,
        revision,
    });
}

fn terminal_output_lines(count: usize) -> String {
    (0..count)
        .map(|index| format!("line {index:03}\n"))
        .collect::<String>()
}

fn screen_from_line_count(count: usize) -> TerminalScreen {
    terminal_screen_from_output(&terminal_output_lines(count))
}

fn snapshot_covers_offset(
    snapshot: &TerminalSnapshot,
    display_offset: usize,
    viewport_rows: usize,
) -> bool {
    let viewport_rows = viewport_rows.max(1);
    let real_total_rows = snapshot.scrollback_len.saturating_add(viewport_rows);
    let snapshot_end = snapshot.total_rows.saturating_sub(snapshot.display_offset);
    let snapshot_start = snapshot_end.saturating_sub(snapshot.row_count());
    let desired_end = real_total_rows.saturating_sub(display_offset);
    let desired_start = desired_end.saturating_sub(viewport_rows);
    snapshot_start <= desired_start && desired_end <= snapshot_end
}

#[test]
fn terminal_view_output_decodes_session_charset() {
    let mut view = TerminalViewState::new();
    view.set_encoding("GBK");

    view.append_bytes_unprotected(&[0xb2]);
    assert!(view.output.is_empty());

    view.append_bytes_unprotected(&[0xe2, 0xca, 0xd4]);
    assert_eq!(view.output, "测试");
    let joined = view.screen.lines().join("");
    let compact = joined.replace(' ', "");
    assert!(compact.contains("测试"), "grid={joined:?}");
}

#[test]
fn terminal_view_local_text_bypasses_session_charset() {
    let mut view = TerminalViewState::new();
    view.set_encoding("GBK");

    view.append_text("本地提示");

    assert_eq!(view.output, "本地提示");
    let joined = view.screen.lines().join("");
    let compact = joined.replace(' ', "");
    assert!(compact.contains("本地提示"), "grid={joined:?}");
    assert!(!joined.contains('\u{fffd}'), "grid={joined:?}");
}

#[test]
fn terminal_view_local_text_live_snapshot_keeps_scroll_window() {
    let mut view = TerminalViewState::from_output(terminal_output_lines(80));
    let viewport_rows = view.screen.viewport_snapshot(0).row_count();

    view.append_text("local status\n");

    let snapshot = view
        .frame_snapshot
        .as_ref()
        .expect("local append should publish a live snapshot");
    assert!(snapshot.row_count() > viewport_rows);
    assert!(snapshot_covers_offset(snapshot.as_ref(), 0, viewport_rows));
    assert!(snapshot_covers_offset(snapshot.as_ref(), 1, viewport_rows));
}

#[test]
fn terminal_view_ui_viewport_rows_ignore_retained_snapshot_rows() {
    let mut view = TerminalViewState::from_output(terminal_output_lines(80));
    let viewport_rows = view.screen.viewport_snapshot(0).row_count();

    view.append_text("local status\n");

    let snapshot_rows = view
        .frame_snapshot
        .as_ref()
        .expect("local append should publish a live snapshot")
        .row_count();
    assert!(snapshot_rows > viewport_rows);
    assert_eq!(view.viewport_rows_for_ui(), viewport_rows);
}

#[test]
fn terminal_view_live_snapshot_uses_normal_scroll_window() {
    let view = TerminalViewState::from_output(terminal_output_lines(320));
    let viewport_rows = view.screen.viewport_snapshot(0).row_count();
    let normal_offset = viewport_rows.saturating_mul(2);

    let snapshot = view.live_snapshot_with_scroll_window();

    assert!(
        snapshot.row_count()
            >= viewport_rows.saturating_add(terminal_frame_scroll_window_extra_rows(
                viewport_rows,
                false,
            ))
    );
    assert!(
        snapshot.row_count()
            < viewport_rows
                .saturating_add(terminal_frame_scroll_window_extra_rows(viewport_rows, true,))
    );
    assert!(snapshot_covers_offset(snapshot.as_ref(), 0, viewport_rows));
    assert!(snapshot_covers_offset(
        snapshot.as_ref(),
        normal_offset,
        viewport_rows
    ));
}

#[test]
fn terminal_view_scroll_snapshot_uses_resized_grid_geometry() {
    let mut view = TerminalViewState::from_output(terminal_output_lines(320));
    let old_cols = view.screen.cols();
    let old_rows = view.screen.rows();
    let resized_cols = old_cols.saturating_add(12) as u16;
    let resized_rows = old_rows.saturating_add(6) as u16;

    view.screen.resize(resized_cols, resized_rows);
    let display_offset = 8.min(view.screen.scrollback_len());
    let viewport_rows = view.viewport_rows_for_ui();
    let snapshot = view.snapshot_with_scroll_window(display_offset);

    assert_eq!(snapshot.cols, resized_cols as usize);
    assert_eq!(snapshot.scrollback_len, view.screen.scrollback_len());
    assert!(terminal_snapshot_matches_grid_geometry(
        snapshot.as_ref(),
        view.screen.cols(),
        view.screen.rows(),
    ));
    assert!(snapshot_covers_offset(
        snapshot.as_ref(),
        display_offset,
        viewport_rows,
    ));
}

#[test]
fn terminal_snapshot_geometry_rejects_height_only_resize() {
    let mut view = TerminalViewState::from_output(terminal_output_lines(80));
    let snapshot = view.live_snapshot_with_scroll_window();
    let old_cols = view.screen.cols();
    let old_rows = view.screen.rows();

    assert!(terminal_snapshot_matches_grid_geometry(
        snapshot.as_ref(),
        old_cols,
        old_rows,
    ));

    view.screen
        .resize(old_cols as u16, old_rows.saturating_add(5) as u16);

    assert!(!terminal_snapshot_matches_grid_geometry(
        snapshot.as_ref(),
        view.screen.cols(),
        view.screen.rows(),
    ));
}

#[test]
fn terminal_view_unprotected_bytes_live_snapshot_keeps_scroll_window() {
    let mut view = TerminalViewState::from_output(terminal_output_lines(80));
    let viewport_rows = view.screen.viewport_snapshot(0).row_count();

    view.append_bytes_unprotected(b"byte status\n");

    let snapshot = view
        .frame_snapshot
        .as_ref()
        .expect("byte append should publish a live snapshot");
    assert!(snapshot.row_count() > viewport_rows);
    assert!(snapshot_covers_offset(snapshot.as_ref(), 0, viewport_rows));
    assert!(snapshot_covers_offset(snapshot.as_ref(), 1, viewport_rows));
}

#[test]
fn terminal_view_append_anchors_scrollback_when_output_adds_history() {
    let mut view = TerminalViewState::from_output(terminal_output_lines(120));
    let old_len = view.scrollback_len_for_ui();
    assert!(old_len > 32);
    view.scroll_offset = 32;
    view.scrollback_snapshots
        .insert(32, Arc::new(view.screen.viewport_snapshot(32)));
    view.scrollback_action_links
        .insert(32, TerminalFrameActionLinks::default());
    view.pending_snapshot_offsets.insert(32);
    view.priority_pending_snapshot_offsets.insert(32);

    view.append_text("new anchored line\n");

    let new_len = view.scrollback_len_for_ui();
    let delta = new_len.saturating_sub(old_len);
    assert!(delta > 0);
    assert_eq!(view.scroll_offset, 32 + delta);
    assert!(view.has_new_while_scrolled);
    assert!(!view.scrollback_snapshots.contains_key(&32));
    let remapped = view
        .scrollback_snapshots
        .get(&(32 + delta))
        .expect("cached scrolled snapshot should be rekeyed");
    assert_eq!(remapped.display_offset, 32 + delta);
    assert_eq!(remapped.scrollback_len, new_len);
    assert!(view.scrollback_action_links.contains_key(&(32 + delta)));
    assert!(view.pending_snapshot_offsets.is_empty());
    assert!(view.priority_pending_snapshot_offsets.is_empty());
}

#[test]
fn terminal_view_frame_apply_anchors_scrolled_offset() {
    let old_screen = screen_from_line_count(120);
    let mut view = TerminalViewState::new();
    view.frame_snapshot = Some(Arc::new(old_screen.viewport_snapshot(0)));
    let old_len = view.scrollback_len_for_ui();
    assert!(old_len > 32);
    view.scroll_offset = 32;
    view.scrollback_snapshots
        .insert(32, Arc::new(old_screen.viewport_snapshot(32)));

    let new_screen = screen_from_line_count(123);
    view.apply_terminal_frame_parts(TerminalFrameParts {
        visible_text: "",
        snapshot: Arc::new(new_screen.viewport_snapshot(0)),
        action_links: None,
        protocol_state: TerminalProtocolState::default(),
        accepted_bytes: 1,
        skipped_output_bytes: 0,
        revision: 2,
    });

    let new_len = view.scrollback_len_for_ui();
    let delta = new_len.saturating_sub(old_len);
    assert!(delta > 0);
    assert_eq!(view.scroll_offset, 32 + delta);
    assert!(view.scrollback_snapshots.contains_key(&(32 + delta)));
}

#[test]
fn terminal_view_live_snapshot_preserves_scrolled_cache() {
    let old_screen = screen_from_line_count(120);
    let mut view = TerminalViewState::new();
    view.frame_snapshot = Some(Arc::new(old_screen.viewport_snapshot(0)));
    let old_len = view.scrollback_len_for_ui();
    assert!(old_len > 32);
    view.scroll_offset = 32;
    view.scrollback_snapshots
        .insert(32, Arc::new(old_screen.viewport_snapshot(32)));
    view.pending_snapshot_offsets.insert(32);
    view.priority_pending_snapshot_offsets.insert(32);

    let new_screen = screen_from_line_count(123);
    view.apply_terminal_live_snapshot_frame(Arc::new(new_screen.viewport_snapshot(0)), None, 2);

    let new_len = view.scrollback_len_for_ui();
    let delta = new_len.saturating_sub(old_len);
    assert!(delta > 0);
    assert_eq!(view.scroll_offset, 32 + delta);
    assert!(view.has_new_while_scrolled);
    assert!(!view.scrollback_snapshots.contains_key(&32));
    assert!(view.scrollback_snapshots.contains_key(&(32 + delta)));
    assert!(view.pending_snapshot_offsets.is_empty());
    assert!(view.priority_pending_snapshot_offsets.is_empty());
}

#[test]
fn terminal_view_bottom_output_clears_stale_scrollback_cache() {
    let mut view = TerminalViewState::from_output(terminal_output_lines(40));
    view.scrollback_snapshots
        .insert(2, Arc::new(view.screen.viewport_snapshot(2)));
    view.pending_snapshot_offsets.insert(2);
    view.priority_pending_snapshot_offsets.insert(2);

    view.append_text("bottom line\n");

    assert_eq!(view.scroll_offset, 0);
    assert!(view.scrollback_snapshots.is_empty());
    assert!(view.pending_snapshot_offsets.is_empty());
    assert!(view.priority_pending_snapshot_offsets.is_empty());
}

#[test]
fn terminal_view_remember_scrollback_snapshot_prunes_to_recent_window_budget() {
    let mut view = TerminalViewState::from_output(terminal_output_lines(80));

    for offset in 1..=20 {
        view.remember_scrollback_snapshot(offset, Arc::new(view.screen.viewport_snapshot(offset)));
    }

    assert_eq!(view.scrollback_snapshots.len(), 16);
    assert!(view.scrollback_snapshots.contains_key(&20));
    assert!((5..=20).all(|offset| view.scrollback_snapshots.contains_key(&offset)));
    assert!((1..5).all(|offset| !view.scrollback_snapshots.contains_key(&offset)));
}

#[test]
fn terminal_view_prunes_scrollback_snapshots_around_keep_offset() {
    let mut view = TerminalViewState::from_output(terminal_output_lines(160));

    for offset in [
        1usize, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60, 64,
    ] {
        view.scrollback_snapshots
            .insert(offset, Arc::new(view.screen.viewport_snapshot(offset)));
        view.scrollback_action_links.insert(
            offset,
            TerminalFrameActionLinks {
                matcher_key: 0,
                absolute_start_row: 0,
                absolute_end_row: 0,
                row_signatures: Vec::new(),
                matches_by_line: Vec::new(),
                cell_ranges_by_line: Vec::new(),
            },
        );
    }

    view.prune_scrollback_snapshot_cache(32);

    assert_eq!(
        view.scrollback_snapshots.len(),
        TERMINAL_SCROLLBACK_SNAPSHOT_CACHE_LIMIT
    );
    assert!(view.scrollback_snapshots.contains_key(&32));
    assert!(!view.scrollback_snapshots.contains_key(&64));
    assert!(!view.scrollback_action_links.contains_key(&64));
    assert!(view.scrollback_snapshots.contains_key(&28));
    assert!(view.scrollback_snapshots.contains_key(&36));
}

#[test]
fn terminal_view_seed_output_applies_session_encoding() {
    let view = TerminalViewState::from_output_with_encoding("seed".to_string(), "GBK");

    assert_eq!(view.screen.encoding_label(), "GBK");
    assert_eq!(view.output_decoder.encoding_label(), "GBK");
    assert_eq!(view.recording_decoder.encoding_label(), "GBK");
    assert_eq!(
        view.screen.encode_outgoing("测试".as_bytes()).unwrap(),
        [0xb2, 0xe2, 0xca, 0xd4]
    );
}

#[test]
fn terminal_view_output_decodes_split_utf8() {
    let mut view = TerminalViewState::new();
    let bytes = "测".as_bytes();

    view.append_bytes_unprotected(&bytes[..1]);
    assert!(view.output.is_empty());

    view.append_bytes_unprotected(&bytes[1..]);
    assert_eq!(view.output, "测");
    assert!(view.screen.lines().join("").contains('测'));
}

#[test]
fn terminal_view_output_burst_drop_resets_stream_decoders() {
    let mut view = TerminalViewState::new();
    view.set_encoding("GBK");

    // First byte of "测" in GBK, intentionally left incomplete.
    view.append_bytes_unprotected(&[0xb2]);
    assert!(view.output.is_empty());

    let mut burst = vec![b'a'; TERMINAL_OUTPUT_VISIBLE_BACKLOG_CAP + 2];
    // The first byte retained after the forced drop is the second byte of
    // "测". It must not combine with the skipped/pending 0xb2 above.
    burst[2] = 0xe2;

    let feed = view.protect_output_burst(&burst);
    view.append_bytes_unprotected(feed);

    assert!(!view.output.contains('测'), "output={:?}", view.output);
    let grid = view.screen.lines().join("");
    assert!(!grid.contains('测'), "grid={grid:?}");
    assert_eq!(view.skipped_output_chars, 2);
}

#[test]
fn terminal_frame_large_accepted_output_does_not_show_protection_overlay() {
    let mut view = TerminalViewState::new();
    let frame = output_frame_with_sizes((32 * 1024) + 1, 0);

    apply_output_frame_to_view(&mut view, frame);

    assert_eq!(view.performance_mode, TerminalPerformanceMode::Normal);
    assert_eq!(view.performance_overlay, None);
    assert_eq!(view.skipped_output_chars, 0);
}

#[test]
fn terminal_degraded_frame_apply_keeps_output_tail() {
    let mut view = TerminalViewState::new();
    view.render_degraded = true;

    apply_output_frame_to_view(&mut view, output_frame_with_sizes(1, 0));

    assert_eq!(view.output, "x");
}

#[test]
fn terminal_background_frame_apply_skips_render_work_but_keeps_output_tail() {
    let mut view = TerminalViewState::new();
    view.render_degraded = false;
    let frame = output_frame_with_sizes((32 * 1024) + 1, 0);

    view.apply_terminal_background_frame_parts(
        &frame.visible_text,
        frame.snapshot.clone(),
        frame.action_links.clone(),
        frame.protocol_state,
        frame.skipped_output_bytes,
        frame.revision,
    );

    // AI context and MCP read this tail for sessions that are not on screen.
    assert_eq!(view.output, "x");
    assert_eq!(view.screen_revision, frame.revision);
    assert!(view.frame_snapshot.is_some());
    assert_eq!(view.output_burst_bytes, 0);
    assert!(!view.render_degraded);
    assert_eq!(view.performance_mode, TerminalPerformanceMode::Normal);
    assert_eq!(view.performance_overlay, None);
}

#[test]
fn clear_presentation_keeps_terminal_protocol_and_stream_state() {
    let mut view = TerminalViewState::new();
    view.screen.advance(b"\x1b[?2004hprompt> draft");
    view.protocol_state = TerminalProtocolState::from_screen(&view.screen);
    view.output = "old output".to_string();
    view.scroll_offset = 3;
    let before = view.protocol_state;
    let snapshot = view.screen.snapshot();

    view.clear_presentation_except_input(42, &snapshot);

    assert_eq!(view.protocol_state, before);
    assert!(view.screen.bracketed_paste());
    assert_eq!(view.output, "prompt> draft");
    assert_eq!(view.scroll_offset, 0);
    assert_eq!(view.screen_revision, 42);
}

#[test]
fn terminal_frame_skipped_output_shows_protection_overlay() {
    let mut view = TerminalViewState::new();
    let frame = output_frame_with_sizes(1, 7);

    apply_output_frame_to_view(&mut view, frame);

    assert_eq!(view.performance_mode, TerminalPerformanceMode::Overloaded);
    assert_eq!(
        view.performance_overlay,
        Some(TerminalPerformanceOverlay::Overloaded)
    );
    assert_eq!(view.skipped_output_chars, 7);
}

#[test]
fn terminal_view_output_discontinuity_resets_all_stream_decoders() {
    let mut view = TerminalViewState::new();
    view.set_encoding("GBK");

    // First byte of "测" in GBK, intentionally left incomplete in all
    // three streaming consumers: screen, visible output, and recording.
    view.append_bytes_unprotected(&[0xb2]);
    assert!(view.output.is_empty());
    assert!(
        view.recording_decoder
            .decode_output_text(&[0xb2])
            .is_empty()
    );

    view.note_output_discontinuity(7);

    view.append_bytes_unprotected(&[0xe2]);
    let recorded = view.recording_decoder.decode_output_text(&[0xe2]);

    assert!(!view.output.contains('测'), "output={:?}", view.output);
    assert!(!recorded.contains('测'), "recorded={recorded:?}");
    let grid = view.screen.lines().join("");
    assert!(!grid.contains('测'), "grid={grid:?}");
    assert_eq!(view.skipped_output_chars, 7);
}

#[test]
fn terminal_view_output_skips_graphics_payload() {
    let mut view = TerminalViewState::new();

    view.append_bytes_unprotected(b"pre\x1b_Ga=T,i=1,c=1,r=1;QUI=\x1b\\post");

    assert_eq!(view.output, "prepost");
    assert!(view.screen.lines().join("").contains("prepost"));
}

#[test]
fn terminal_view_filtered_visible_text_can_reenter_byte_parser() {
    let mut view = TerminalViewState::new();
    let visible_text = "plain \x1b[31mred\x1b[0m";
    let visible_bytes = view.screen.encode_outgoing_str(visible_text).unwrap();

    view.append_bytes_unprotected(&visible_bytes);

    let snapshot = view.screen.snapshot();
    let red_cell = snapshot
        .rows()
        .iter()
        .flat_map(|row| row.cells.iter())
        .find(|cell| cell.text() == "r")
        .expect("styled red cell");
    assert_eq!(red_cell.style.fg, Some(1));
}

#[test]
fn terminal_ui_output_tail_is_bounded_and_utf8_safe() {
    let mut output = format!("{}界", "好".repeat(TERMINAL_UI_OUTPUT_TAIL_CAP));

    append_terminal_ui_output_tail(&mut output, "done");

    assert!(output.len() <= TERMINAL_UI_OUTPUT_TAIL_CAP);
    assert!(output.ends_with("done"));
    assert!(std::str::from_utf8(output.as_bytes()).is_ok());
}

#[test]
fn terminal_view_backend_resize_detects_pixel_only_changes() {
    let mut view = TerminalViewState::new();

    assert!(view.backend_resize_changed(80, 24, 800, 432));
    view.remember_backend_resize(80, 24, 800, 432);

    assert!(!view.backend_resize_changed(80, 24, 800, 432));
    assert!(view.backend_resize_changed(80, 24, 960, 432));
    assert!(view.backend_resize_changed(80, 25, 800, 450));
}

#[test]
fn reconnect_view_resets_worker_protocol_cache_before_any_new_frame() {
    let mut view = TerminalViewState::new();
    // Production UI screens mirror geometry rather than the worker's modes.
    let mut worker_screen = TerminalScreen::default();
    worker_screen.advance(b"\x1b[>31u\x1b[?1049h\x1b[>31u\x1b[?1;1003;1004;1006;2004h\x1b=");
    view.protocol_state = TerminalProtocolState::from_screen(&worker_screen);
    view.output = "retained output".into();
    view.frame_snapshot = Some(Arc::new(worker_screen.snapshot()));
    let snapshot = view.frame_snapshot.clone().unwrap();
    view.pressed_keys.insert("enter".into());

    view.reset_reconnect_stream("UTF-8");

    assert_eq!(
        view.protocol_state,
        TerminalProtocolState {
            alternate_screen: true,
            alternate_scroll: true,
            ..TerminalProtocolState::default()
        }
    );
    assert!(view.pressed_keys.is_empty());
    assert_eq!(view.output, "retained output");
    assert!(Arc::ptr_eq(
        view.frame_snapshot.as_ref().unwrap(),
        &snapshot
    ));
}

#[test]
fn snapshot_reply_clears_both_pending_request_sets() {
    let mut view = TerminalViewState::new();
    view.pending_snapshot_offsets.insert(0);
    view.priority_pending_snapshot_offsets.insert(0);
    view.complete_snapshot_request(0);
    assert!(view.pending_snapshot_offsets.is_empty());
    assert!(view.priority_pending_snapshot_offsets.is_empty());
}

#[test]
fn expensive_interactions_require_active_calm_terminal() {
    assert!(terminal_expensive_interactions_enabled(
        true,
        true,
        false,
        false,
        0,
        TerminalPerformanceMode::Normal,
    ));
    assert!(!terminal_expensive_interactions_enabled(
        false,
        true,
        false,
        false,
        0,
        TerminalPerformanceMode::Normal,
    ));
    assert!(!terminal_expensive_interactions_enabled(
        true,
        false,
        false,
        false,
        0,
        TerminalPerformanceMode::Normal,
    ));
}

#[test]
fn expensive_interactions_yield_under_render_pressure() {
    assert!(!terminal_expensive_interactions_enabled(
        true,
        true,
        true,
        false,
        0,
        TerminalPerformanceMode::Normal,
    ));
    assert!(!terminal_expensive_interactions_enabled(
        true,
        true,
        false,
        true,
        0,
        TerminalPerformanceMode::Normal,
    ));
    assert!(!terminal_expensive_interactions_enabled(
        true,
        true,
        false,
        false,
        1,
        TerminalPerformanceMode::Normal,
    ));
    assert!(!terminal_expensive_interactions_enabled(
        true,
        true,
        false,
        false,
        0,
        TerminalPerformanceMode::Overloaded,
    ));
}

#[test]
fn live_output_frame_without_action_links_preserves_matching_links() {
    let mut screen = TerminalScreen::new(40, 3);
    screen.advance_decoded_text("visit http://example.com");
    let snapshot = Arc::new(screen.viewport_snapshot(0));
    let matchers = ActionLinksMatcherSettings::default();
    let links = prepare_terminal_frame_action_links(&snapshot, true, &matchers).unwrap();
    let mut view = TerminalViewState::new();

    view.apply_terminal_live_snapshot_frame(snapshot.clone(), Some(links), 1);
    view.apply_terminal_live_snapshot_frame(snapshot, None, 2);

    assert!(view.frame_action_links.as_ref().is_some_and(|links| {
        links
            .matches_by_line
            .iter()
            .flatten()
            .any(|item| item.value == "http://example.com")
    }));
}

#[test]
fn live_output_frame_without_action_links_drops_signature_mismatch() {
    let mut first_screen = TerminalScreen::new(40, 3);
    first_screen.advance_decoded_text("visit http://example.com");
    let first_snapshot = Arc::new(first_screen.viewport_snapshot(0));
    let matchers = ActionLinksMatcherSettings::default();
    let links = prepare_terminal_frame_action_links(&first_snapshot, true, &matchers).unwrap();
    let mut second_screen = TerminalScreen::new(40, 3);
    second_screen.advance_decoded_text("plain output");
    let second_snapshot = Arc::new(second_screen.viewport_snapshot(0));
    let mut view = TerminalViewState::new();

    view.apply_terminal_live_snapshot_frame(first_snapshot, Some(links), 1);
    view.apply_terminal_live_snapshot_frame(second_snapshot, None, 2);

    assert!(view.frame_action_links.is_none());
}

/// Two observations spanning the calm window, which is the minimum the accounting
/// can honestly claim: output between ticks shows up as `output_burst_bytes`, so a
/// span it never observed cannot count as calm.
fn tick_through_calm_window(view: &mut TerminalViewState, start: Instant) {
    view.tick_performance_overlay(false, start);
    view.tick_performance_overlay(false, start + TERMINAL_RENDER_DEGRADATION_RECOVERY_CALM);
}

#[test]
fn render_degradation_stays_active_while_output_pressure_is_present() {
    let mut view = TerminalViewState::new();
    let start = Instant::now();

    assert!(view.render_degraded);

    view.tick_performance_overlay(true, start);

    assert!(view.render_degraded);
    assert_eq!(view.render_degraded_calm_since, None);
    // However long pressure lasts, no calm window ever opens.
    view.tick_performance_overlay(true, start + TERMINAL_RENDER_DEGRADATION_RECOVERY_CALM * 4);
    assert!(view.render_degraded);
    assert_eq!(view.render_degraded_calm_since, None);
}

#[test]
fn render_degradation_is_initial_view_profile() {
    let mut view = TerminalViewState::new();

    assert!(view.render_degraded);
    tick_through_calm_window(&mut view, Instant::now());

    assert!(!view.render_degraded);
}

#[test]
fn render_degradation_starts_after_output_frame_applies() {
    let mut view = TerminalViewState::new();
    let start = Instant::now();
    tick_through_calm_window(&mut view, start);
    assert!(!view.render_degraded);
    let frame = output_frame_with_sizes(1, 0);

    apply_output_frame_to_view(&mut view, frame);
    view.tick_performance_overlay(false, start + TERMINAL_RENDER_DEGRADATION_RECOVERY_CALM * 2);

    assert!(view.render_degraded);
    assert_eq!(view.render_degraded_calm_since, None);
}

/// Recovery is a wall-clock window, not a tick count.
///
/// The counter this replaced was 8 ticks, described as a "short calm window" at the
/// 50ms cadence. The event pump also runs at 500ms when the app is calm -- which is
/// precisely the state after a flood ends -- so those 8 ticks were 4s of degraded
/// rendering, and 0.13s under pressure. Ticking the clock in one step here fails if
/// the accounting goes back to counting calls.
#[test]
fn render_degradation_recovers_on_a_wall_clock_calm_window() {
    let mut view = TerminalViewState::new();
    view.enter_render_degraded_mode();
    let start = Instant::now();

    // Many calls inside the window must not recover: it is time, not calls.
    for step in 0..8 {
        view.tick_performance_overlay(
            false,
            start + TERMINAL_RENDER_DEGRADATION_RECOVERY_CALM / 16 * step,
        );
        assert!(
            view.render_degraded,
            "recovery must wait for the calm window, not a call count"
        );
    }

    view.tick_performance_overlay(false, start + TERMINAL_RENDER_DEGRADATION_RECOVERY_CALM);

    assert!(!view.render_degraded);
    assert_eq!(view.render_degraded_calm_since, None);
}

/// The recovered banner dismisses on a deadline, not after 60 pump ticks.
///
/// At the 500ms quiet cadence -- where an app lands once a flood stops and the user
/// is left looking at the notice -- 60 ticks was 30 seconds instead of the ~3s the
/// constant's comment claimed. Two ticks are enough here because the dismissal is a
/// deadline; a call-counting implementation needs sixty and fails.
#[test]
fn the_recovered_notice_dismisses_on_a_deadline_not_a_tick_count() {
    let mut view = TerminalViewState::new();
    let start = Instant::now();
    view.enter_overloaded_mode();
    assert_eq!(
        view.performance_overlay,
        Some(TerminalPerformanceOverlay::Overloaded)
    );

    // First calm tick leaves overloaded mode and raises the notice.
    view.tick_performance_overlay(false, start);
    assert_eq!(
        view.performance_overlay,
        Some(TerminalPerformanceOverlay::Recovered)
    );

    // Still up just before the deadline.
    view.tick_performance_overlay(
        false,
        start + TERMINAL_PERFORMANCE_RECOVERY_NOTICE - Duration::from_millis(1),
    );
    assert_eq!(
        view.performance_overlay,
        Some(TerminalPerformanceOverlay::Recovered),
        "the notice must stay up for its full duration"
    );

    view.tick_performance_overlay(false, start + TERMINAL_PERFORMANCE_RECOVERY_NOTICE);

    assert_eq!(view.performance_overlay, None);
    assert_eq!(view.performance_overlay_until, None);
}

#[test]
fn full_history_output_reanchors_reading_and_reuses_retained_query_rows() {
    let mut view = TerminalViewState::new();
    view.screen = TerminalScreen::new(16, 3);
    view.screen.set_scrollback_limit(8);
    view.append_text("0\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8\r\n9\r\n10\r\n");
    view.scroll_offset = 3;
    let old_snapshot = Arc::new(view.screen.viewport_snapshot(3));
    let reading = old_snapshot.line(0).unwrap().to_string();
    view.remember_scrollback_snapshot(3, old_snapshot.clone());
    view.pending_snapshot_offsets.insert(3);
    view.priority_pending_snapshot_offsets.insert(3);
    view.append_text("11\r\n12\r\n");
    assert_eq!(view.scroll_offset, 5);
    assert!(view.has_new_while_scrolled);
    assert_eq!(
        view.screen.viewport_snapshot(5).line(0),
        Some(reading.as_str())
    );
    assert_eq!(view.scrollback_snapshots.len(), 1);
    assert!(Arc::ptr_eq(
        &view.scrollback_snapshots[&5].row_data,
        &old_snapshot.row_data
    ));
    assert!(view.pending_snapshot_offsets.is_empty());
    assert!(view.priority_pending_snapshot_offsets.is_empty());
    view.remember_scrollback_snapshot(3, old_snapshot);
    assert_eq!(view.scrollback_snapshots.len(), 1);
    assert!(view.scrollback_snapshots.contains_key(&5));
}

#[test]
fn repeated_full_history_frames_keep_anchor_until_the_read_line_is_evicted() {
    let mut view = TerminalViewState::new();
    let mut worker = TerminalScreen::new(16, 3);
    worker.set_scrollback_limit(16);
    for index in 0..20 {
        worker.advance_decoded_text(&format!("{index}\r\n"));
    }
    view.screen.resize(16, 3);
    view.apply_terminal_live_snapshot_frame(Arc::new(worker.snapshot()), None, 1);
    view.scroll_offset = 3;
    let reading = worker.viewport_snapshot(3).line(0).unwrap().to_string();
    for index in 20..30 {
        worker.advance_decoded_text(&format!("{index}\r\n"));
        view.apply_terminal_live_snapshot_frame(Arc::new(worker.snapshot()), None, index);
        assert_eq!(view.scroll_offset, 3 + (index as usize - 19));
        assert_eq!(
            worker.viewport_snapshot(view.scroll_offset).line(0),
            Some(reading.as_str())
        );
    }
    view.scroll_offset = 0;
    worker.advance(b"30\r\n");
    view.apply_terminal_live_snapshot_frame(Arc::new(worker.snapshot()), None, 31);
    assert_eq!(view.scroll_offset, 0);
}

#[test]
fn hidden_session_keeps_history_anchor_when_releasing_its_snapshot() {
    let mut view = TerminalViewState::new();
    let mut worker = TerminalScreen::new(16, 3);
    worker.set_scrollback_limit(8);
    for index in 0..12 {
        worker.advance_decoded_text(&format!("{index}\r\n"));
    }
    view.screen.resize(16, 3);
    view.apply_terminal_live_snapshot_frame(Arc::new(worker.snapshot()), None, 1);
    view.scroll_offset = 3;
    let baseline = view.history_state();
    view.apply_terminal_background_frame_parts(
        "",
        None,
        None,
        TerminalProtocolState::default(),
        0,
        2,
    );
    assert!(view.frame_snapshot.is_none());
    assert_eq!(view.history_state(), baseline);
    worker.advance(b"12\r\n13\r\n");
    view.apply_terminal_live_snapshot_frame(Arc::new(worker.snapshot()), None, 3);
    assert_eq!(view.scroll_offset, 5);
}
