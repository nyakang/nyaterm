use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nyaterm_core::ActionLinksMatcherSettings;
use nyaterm_terminal::{TerminalEffects, TerminalOutputDecoder, TerminalScreen, TerminalSnapshot};

use crate::terminal::{TerminalLineDecorations, terminal_screen_from_output};

use super::{
    SELECTED_OCCURRENCE_SEARCH_CHUNK_ROWS, SelectedOccurrenceSearchJob,
    TERMINAL_FRAME_COMMAND_QUEUE_CAP, TERMINAL_FRAME_EVENT_WAKE_ALL,
    TERMINAL_FRAME_EVENT_WAKE_OUTPUT, TERMINAL_FRAME_EVENT_WAKE_SEARCH,
    TERMINAL_FRAME_EVENT_WAKE_SNAPSHOT, TERMINAL_FRAME_OUTPUT_BURST_BYTE_LIMIT,
    TERMINAL_FRAME_OUTPUT_CHUNK_SIZE, TERMINAL_FRAME_OUTPUT_COALESCE_BYTE_LIMIT,
    TERMINAL_FRAME_VISIBLE_TEXT_TAIL_CAP, TERMINAL_OUTPUT_VISIBLE_BACKLOG_CAP,
    TERMINAL_PERFORMANCE_RECOVERY_NOTICE, TERMINAL_RENDER_DEGRADATION_RECOVERY_CALM,
    TERMINAL_SCROLLBACK_SNAPSHOT_CACHE_LIMIT, TERMINAL_UI_OUTPUT_TAIL_CAP,
    TerminalFrameActionLinks, TerminalFrameCommand, TerminalFrameEvent, TerminalFrameEventQueue,
    TerminalFrameOutputBatch, TerminalFrameOutputEvent, TerminalFrameOutputSubmission,
    TerminalFrameParts, TerminalFrameSearchEvent, TerminalFrameSearchKey,
    TerminalFrameSearchPurpose, TerminalFrameSearchResult, TerminalFrameSession,
    TerminalFrameSnapshotEvent, TerminalFrameSnapshotPurpose, TerminalPerformanceMode,
    TerminalPerformanceOverlay, TerminalProtocolState, TerminalRenderCache, TerminalViewState,
    append_terminal_ui_output_tail, coalesce_terminal_frame_output_command,
    compact_stale_terminal_frame_commands, next_terminal_frame_command,
    prepare_terminal_frame_action_links, prepare_terminal_frame_action_links_reusing,
    process_next_selected_occurrence_search_chunk, process_terminal_frame_output_burst,
    protect_terminal_output_burst, replace_selected_occurrence_search_job,
    terminal_expensive_interactions_enabled, terminal_frame_command_channel,
    terminal_frame_output_commands, terminal_frame_scroll_window_extra_rows,
    terminal_frame_search_result_is_current, terminal_snapshot_matches_grid_geometry,
    try_next_terminal_frame_command,
};

fn selected_occurrence_test_key(
    query: &str,
    limit: usize,
    generation: u64,
) -> TerminalFrameSearchKey {
    TerminalFrameSearchKey {
        query: query.to_string(),
        case_sensitive: true,
        regex: false,
        whole_word: false,
        limit,
        request_generation: generation,
    }
}

fn selected_occurrence_test_session(cols: u16, lines: usize, text: &str) -> TerminalFrameSession {
    let mut screen = TerminalScreen::new(cols, 24);
    screen.set_scrollback_limit(lines.saturating_mul(2).max(1000));
    let mut output = String::with_capacity(lines.saturating_mul(text.len().saturating_add(2)));
    for _ in 0..lines {
        output.push_str(text);
        output.push_str("\r\n");
    }
    screen.advance(output.as_bytes());
    let mut session = TerminalFrameSession::new("UTF-8", lines.saturating_mul(2).max(1000));
    session.screen = screen;
    session.revision = 1;
    session
}

/// A submission is handed over whole. Slicing it here would only be undone
/// by the enqueue-side merge and the worker's burst coalescing.
#[test]
fn terminal_frame_output_submission_is_a_single_command() {
    let data = vec![b'x'; TERMINAL_FRAME_OUTPUT_CHUNK_SIZE * 2 + 5];
    let command = terminal_frame_output_commands(TerminalFrameOutputSubmission {
        session_id: "s1".to_string(),
        data: data.clone(),
        encoding: "UTF-8".to_string(),
        scrollback_limit: 1000,
    });

    match command {
        Some(TerminalFrameCommand::Output {
            data: submitted, ..
        }) => assert_eq!(submitted, data),
        other => panic!("submission should produce one output command, got {other:?}"),
    }
}

#[test]
fn terminal_frame_empty_output_submission_produces_no_command() {
    let command = terminal_frame_output_commands(TerminalFrameOutputSubmission {
        session_id: "s1".to_string(),
        data: Vec::new(),
        encoding: "UTF-8".to_string(),
        scrollback_limit: 1000,
    });
    assert!(command.is_none());
}

#[test]
fn terminal_decoration_cache_reuses_shared_lines() {
    let cache = TerminalRenderCache::default();
    let first = cache.line_decorations(7, || {
        vec![TerminalLineDecorations {
            link_ranges: vec![(1, 3)],
            ..TerminalLineDecorations::default()
        }]
    });
    let second = cache.line_decorations(7, || panic!("cache hit should not rebuild"));

    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(cache.decoration_stats(), (1, 1));
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
            None,
            None,
            &visible_text,
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

fn snapshot_anchor_row_for_offset(
    snapshot: &TerminalSnapshot,
    display_offset: usize,
    viewport_rows: usize,
) -> usize {
    let viewport_rows = viewport_rows.max(1);
    let real_total_rows = snapshot.scrollback_len.saturating_add(viewport_rows);
    let snapshot_end = snapshot.total_rows.saturating_sub(snapshot.display_offset);
    let snapshot_start = snapshot_end.saturating_sub(snapshot.row_count());
    let desired_end = real_total_rows.saturating_sub(display_offset);
    let desired_start = desired_end.saturating_sub(viewport_rows);
    desired_start.saturating_sub(snapshot_start)
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
    let mut view = TerminalViewState::from_output(terminal_output_lines(40));
    let old_len = view.scrollback_len_for_ui();
    assert!(old_len > 3);
    view.scroll_offset = 3;
    view.scrollback_snapshots
        .insert(3, Arc::new(view.screen.viewport_snapshot(3)));
    view.scrollback_action_links
        .insert(3, TerminalFrameActionLinks::default());
    view.pending_snapshot_offsets.insert(3);
    view.priority_pending_snapshot_offsets.insert(3);

    view.append_text("new anchored line\n");

    let new_len = view.scrollback_len_for_ui();
    let delta = new_len.saturating_sub(old_len);
    assert!(delta > 0);
    assert_eq!(view.scroll_offset, 3 + delta);
    assert!(view.has_new_while_scrolled);
    assert!(!view.scrollback_snapshots.contains_key(&3));
    let remapped = view
        .scrollback_snapshots
        .get(&(3 + delta))
        .expect("cached scrolled snapshot should be rekeyed");
    assert_eq!(remapped.display_offset, 3 + delta);
    assert_eq!(remapped.scrollback_len, new_len);
    assert!(view.scrollback_action_links.contains_key(&(3 + delta)));
    assert!(view.pending_snapshot_offsets.is_empty());
    assert!(view.priority_pending_snapshot_offsets.is_empty());
}

#[test]
fn terminal_view_frame_apply_anchors_scrolled_offset() {
    let old_screen = screen_from_line_count(40);
    let mut view = TerminalViewState::new();
    view.frame_snapshot = Some(Arc::new(old_screen.viewport_snapshot(0)));
    let old_len = view.scrollback_len_for_ui();
    assert!(old_len > 4);
    view.scroll_offset = 4;
    view.scrollback_snapshots
        .insert(4, Arc::new(old_screen.viewport_snapshot(4)));

    let new_screen = screen_from_line_count(43);
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
    assert_eq!(view.scroll_offset, 4 + delta);
    assert!(view.scrollback_snapshots.contains_key(&(4 + delta)));
}

#[test]
fn terminal_view_live_snapshot_preserves_scrolled_cache() {
    let old_screen = screen_from_line_count(40);
    let mut view = TerminalViewState::new();
    view.frame_snapshot = Some(Arc::new(old_screen.viewport_snapshot(0)));
    let old_len = view.scrollback_len_for_ui();
    assert!(old_len > 4);
    view.scroll_offset = 4;
    view.scrollback_snapshots
        .insert(4, Arc::new(old_screen.viewport_snapshot(4)));
    view.pending_snapshot_offsets.insert(4);
    view.priority_pending_snapshot_offsets.insert(4);

    let new_screen = screen_from_line_count(43);
    view.apply_terminal_live_snapshot_frame(Arc::new(new_screen.viewport_snapshot(0)), None, 2);

    let new_len = view.scrollback_len_for_ui();
    let delta = new_len.saturating_sub(old_len);
    assert!(delta > 0);
    assert_eq!(view.scroll_offset, 4 + delta);
    assert!(view.has_new_while_scrolled);
    assert!(!view.scrollback_snapshots.contains_key(&4));
    assert!(view.scrollback_snapshots.contains_key(&(4 + delta)));
    assert!(view.pending_snapshot_offsets.is_empty());
    assert!(view.priority_pending_snapshot_offsets.is_empty());
}

#[test]
fn terminal_frame_snapshot_event_returns_scroll_window() {
    let mut session = TerminalFrameSession::new("UTF-8", 1000);
    session.screen = screen_from_line_count(120);
    let offset = 20;
    let viewport_rows = session.screen.viewport_snapshot(offset).row_count();

    let event = session.snapshot_event(
        "s1".to_string(),
        offset,
        false,
        ActionLinksMatcherSettings::default(),
        false,
    );

    assert_eq!(event.offset, offset);
    assert!(event.snapshot.row_count() > viewport_rows);
    assert!(snapshot_covers_offset(
        event.snapshot.as_ref(),
        offset,
        viewport_rows
    ));
    assert!(snapshot_covers_offset(
        event.snapshot.as_ref(),
        offset - 1,
        viewport_rows
    ));
    assert!(snapshot_covers_offset(
        event.snapshot.as_ref(),
        offset + 1,
        viewport_rows
    ));
}

#[test]
fn terminal_frame_resize_snapshot_preserves_worker_content_and_geometry() {
    let mut session = TerminalFrameSession::new("UTF-8", 1000);
    session.screen = screen_from_line_count(120);

    session.resize(100, 30);
    let event = session.resized_live_snapshot_event("s1".to_string(), Instant::now());

    assert_eq!(event.offset, 0);
    assert_eq!(event.snapshot.cols, 100);
    assert_eq!(event.snapshot.viewport_rows, 30);
    assert!(
        event
            .snapshot
            .rows()
            .iter()
            .any(|row| row.text.contains("line 119"))
    );
    assert!(terminal_snapshot_matches_grid_geometry(
        event.snapshot.as_ref(),
        100,
        30,
    ));
}

#[test]
fn terminal_frame_snapshot_event_covers_multi_viewport_fast_scroll_runs() {
    let mut session = TerminalFrameSession::new("UTF-8", 1000);
    session.screen = screen_from_line_count(240);
    let offset = 80;
    let viewport_rows = session.screen.viewport_snapshot(offset).row_count();
    let fast_delta = viewport_rows.saturating_mul(2);

    let event = session.snapshot_event(
        "s1".to_string(),
        offset,
        false,
        ActionLinksMatcherSettings::default(),
        false,
    );

    assert_eq!(event.offset, offset);
    assert!(
        event
            .snapshot
            .rows()
            .iter()
            .all(|row| row.cells.len() == event.snapshot.cols)
    );
    assert!(snapshot_covers_offset(
        event.snapshot.as_ref(),
        offset.saturating_sub(fast_delta),
        viewport_rows
    ));
    assert!(snapshot_covers_offset(
        event.snapshot.as_ref(),
        offset,
        viewport_rows
    ));
    assert!(snapshot_covers_offset(
        event.snapshot.as_ref(),
        offset + fast_delta,
        viewport_rows
    ));
}

#[test]
fn terminal_priority_snapshot_event_covers_predictive_user_scroll_runs() {
    let mut session = TerminalFrameSession::new("UTF-8", 2000);
    session.screen = screen_from_line_count(900);
    let offset = 300;
    let viewport_rows = session.screen.viewport_snapshot(offset).row_count();
    let fast_delta = viewport_rows.saturating_mul(3);
    let event = session.snapshot_event(
        "s1".to_string(),
        offset,
        false,
        ActionLinksMatcherSettings::default(),
        true,
    );

    assert_eq!(event.offset, offset);
    assert!(snapshot_covers_offset(
        event.snapshot.as_ref(),
        offset.saturating_sub(fast_delta),
        viewport_rows
    ));
    assert!(snapshot_covers_offset(
        event.snapshot.as_ref(),
        offset + fast_delta,
        viewport_rows
    ));
}

#[test]
fn terminal_frame_scroll_window_extra_rows_covers_fast_scroll_runs() {
    assert_eq!(terminal_frame_scroll_window_extra_rows(12, false), 32);
    assert_eq!(terminal_frame_scroll_window_extra_rows(40, false), 80);
    assert_eq!(terminal_frame_scroll_window_extra_rows(120, false), 192);
    assert_eq!(terminal_frame_scroll_window_extra_rows(12, true), 64);
    assert_eq!(terminal_frame_scroll_window_extra_rows(40, true), 120);
    assert_eq!(terminal_frame_scroll_window_extra_rows(160, true), 256);
}

#[test]
fn terminal_live_frame_snapshot_covers_first_scrollback_step() {
    let mut session = TerminalFrameSession::new("UTF-8", 1000);
    session.screen = screen_from_line_count(80);
    let offset = 0;
    let viewport_rows = session.screen.viewport_snapshot(offset).row_count();
    assert!(session.screen.scrollback_len() > 0);

    let event = session.snapshot_event(
        "s1".to_string(),
        offset,
        false,
        ActionLinksMatcherSettings::default(),
        false,
    );

    assert_eq!(event.offset, offset);
    assert!(event.snapshot.row_count() > viewport_rows);
    assert!(snapshot_covers_offset(
        event.snapshot.as_ref(),
        offset,
        viewport_rows
    ));
    assert!(snapshot_covers_offset(
        event.snapshot.as_ref(),
        1,
        viewport_rows
    ));
}

#[test]
fn terminal_scroll_window_offsets_live_cursor_by_prepended_rows() {
    let mut session = TerminalFrameSession::new("UTF-8", 1000);
    session.screen = screen_from_line_count(80);
    let base = session.screen.viewport_snapshot(0);
    assert!(session.screen.scrollback_len() > 0);

    let event = session.snapshot_event(
        "s1".to_string(),
        0,
        false,
        ActionLinksMatcherSettings::default(),
        false,
    );

    let prepended_rows = event.snapshot.row_count().saturating_sub(base.row_count());
    assert!(prepended_rows > 0);
    assert_eq!(
        event.snapshot.cursor.row,
        base.cursor.row.saturating_add(prepended_rows)
    );
    assert_eq!(
        event.snapshot.cursor.row,
        base.cursor.row.saturating_add(prepended_rows)
    );
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
        view.screen.encode_outgoing("测试".as_bytes()),
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
fn terminal_output_burst_helper_resets_screen_and_decoder() {
    let mut screen = TerminalScreen::default();
    let mut decoder = TerminalOutputDecoder::default();
    screen.set_encoding("GBK");
    decoder.set_encoding("GBK");

    // First byte of "测" in GBK, intentionally left incomplete in both
    // streaming consumers.
    screen.advance(&[0xb2]);
    assert!(decoder.decode_output_text(&[0xb2]).is_empty());

    let mut burst = vec![b'a'; TERMINAL_OUTPUT_VISIBLE_BACKLOG_CAP + 2];
    // The first retained byte is the second half of "测". It must not pair
    // with the skipped/pending first byte after protection resets state.
    burst[2] = 0xe2;

    let (feed, skipped) = protect_terminal_output_burst(&mut screen, &mut decoder, &burst);
    screen.advance(feed);
    let output = decoder.decode_output_text(feed);

    assert_eq!(skipped, 2);
    assert!(!output.contains('测'), "output={output:?}");
    let grid = screen.lines().join("");
    assert!(!grid.contains('测'), "grid={grid:?}");
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
fn terminal_background_frame_apply_skips_render_work() {
    let mut view = TerminalViewState::new();
    view.render_degraded = false;
    let frame = output_frame_with_sizes((32 * 1024) + 1, 0);

    view.apply_terminal_background_frame_parts(
        frame.snapshot.clone(),
        frame.action_links.clone(),
        &frame.visible_text,
        frame.protocol_state,
        frame.skipped_output_bytes,
        frame.revision,
    );

    assert_eq!(view.output, "x");
    assert_eq!(view.screen_revision, frame.revision);
    assert!(view.frame_snapshot.is_some());
    assert_eq!(view.output_burst_bytes, 0);
    assert!(!view.render_degraded);
    assert_eq!(view.performance_mode, TerminalPerformanceMode::Normal);
    assert_eq!(view.performance_overlay, None);
}

#[test]
fn terminal_frame_output_skips_snapshot_when_low_priority() {
    let mut session = TerminalFrameSession::new("UTF-8", 1000);
    session.include_live_snapshot = false;
    session.screen.advance_decoded_text("hidden-output");
    let batch = TerminalFrameOutputBatch {
        visible_text: "hidden-output".to_string(),
        accepted_bytes: 13,
        ..TerminalFrameOutputBatch::default()
    };
    let event = session.output_event_from_batch("hidden".to_string(), batch, Instant::now());
    assert!(event.snapshot.is_none());
    assert_eq!(event.visible_text, "hidden-output");
    assert!(event.revision >= 1 || event.accepted_bytes == 13);
}

#[test]
fn terminal_frame_output_includes_snapshot_when_high_priority() {
    let mut session = TerminalFrameSession::new("UTF-8", 1000);
    session.include_live_snapshot = true;
    session.screen.advance_decoded_text("visible-output");
    session.revision = 3;
    let batch = TerminalFrameOutputBatch {
        visible_text: "visible-output".to_string(),
        accepted_bytes: 14,
        ..TerminalFrameOutputBatch::default()
    };
    let event = session.output_event_from_batch("visible".to_string(), batch, Instant::now());
    assert!(event.snapshot.is_some());
    let snap = event.snapshot.unwrap();
    assert!(
        snap.rows()
            .iter()
            .any(|row| row.text.contains("visible-output") || !row.text.is_empty())
    );
}

#[test]
fn terminal_frame_output_reports_single_line_snapshot_reuse() {
    let mut session = TerminalFrameSession::new("UTF-8", 1000);
    session.include_live_snapshot = true;
    session.screen.advance(b"prompt> ");
    let first = session.output_event_from_batch(
        "visible".to_string(),
        TerminalFrameOutputBatch::default(),
        Instant::now(),
    );
    let viewport_rows = first.snapshot.as_ref().unwrap().row_count();

    session.screen.advance(b"x");
    let second = session.output_event_from_batch(
        "visible".to_string(),
        TerminalFrameOutputBatch::default(),
        Instant::now(),
    );

    assert_eq!(
        second.snapshot_stats.reused_rows + second.snapshot_stats.rebuilt_rows,
        viewport_rows
    );
    assert!(second.snapshot_stats.reused_rows >= viewport_rows.saturating_sub(1));
    assert!(second.snapshot_stats.rebuilt_rows <= 1);
    drop(first);
}

#[test]
fn terminal_frame_output_live_snapshot_stays_viewport_only() {
    let mut session = TerminalFrameSession::new("UTF-8", 1000);
    session.include_live_snapshot = true;
    session.screen = screen_from_line_count(80);
    let viewport_rows = session.screen.viewport_snapshot(0).row_count();
    let event = session.output_event_from_batch(
        "visible".to_string(),
        TerminalFrameOutputBatch::default(),
        Instant::now(),
    );
    let snapshot = event
        .snapshot
        .expect("visible output should include a live snapshot");

    assert_eq!(snapshot.row_count(), viewport_rows);
    assert!(snapshot_covers_offset(snapshot.as_ref(), 0, viewport_rows));
    assert!(!snapshot_covers_offset(snapshot.as_ref(), 1, viewport_rows));
}

#[test]
fn terminal_frame_output_live_snapshot_bounds_scroll_prefetch() {
    let mut session = TerminalFrameSession::new("UTF-8", 1000);
    session.include_live_snapshot = true;
    session.screen = screen_from_line_count(160);
    let viewport_rows = session.screen.viewport_snapshot(0).row_count();
    let event = session.output_event_from_batch(
        "visible".to_string(),
        TerminalFrameOutputBatch::default(),
        Instant::now(),
    );
    let snapshot = event
        .snapshot
        .expect("visible output should include a live snapshot");

    assert_eq!(snapshot.row_count(), viewport_rows);
    assert!(snapshot_covers_offset(snapshot.as_ref(), 0, viewport_rows));
    assert!(!snapshot_covers_offset(snapshot.as_ref(), 1, viewport_rows));
}

#[test]
fn terminal_frame_scroll_request_keeps_normal_scroll_window() {
    let mut session = TerminalFrameSession::new("UTF-8", 1000);
    session.include_live_snapshot = true;
    session.screen = screen_from_line_count(320);
    let viewport_rows = session.screen.viewport_snapshot(0).row_count();
    let normal_offset = viewport_rows.saturating_mul(2);
    let live_snapshot = session
        .output_event_from_batch(
            "visible".to_string(),
            TerminalFrameOutputBatch::default(),
            Instant::now(),
        )
        .snapshot
        .expect("visible output should include a live snapshot");
    let snapshot = session
        .snapshot_event(
            "visible".to_string(),
            0,
            false,
            ActionLinksMatcherSettings::default(),
            false,
        )
        .snapshot;

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
    assert!(snapshot.row_count() > live_snapshot.row_count());
    assert!(snapshot_covers_offset(
        snapshot.as_ref(),
        normal_offset,
        viewport_rows
    ));
    let anchor = snapshot_anchor_row_for_offset(snapshot.as_ref(), normal_offset, viewport_rows);
    assert_eq!(
        snapshot.line(anchor),
        session.screen.viewport_snapshot(normal_offset).line(0)
    );
}

#[test]
fn terminal_frame_set_snapshot_priority_compacts_to_latest() {
    let mut commands = VecDeque::new();
    commands.push_back(TerminalFrameCommand::SetSnapshotPriority {
        session_ids: vec!["a".to_string()],
    });
    commands.push_back(TerminalFrameCommand::Output {
        session_id: "a".to_string(),
        data: b"x".to_vec(),
        encoding: "UTF-8".to_string(),
        scrollback_limit: 1000,
    });
    commands.push_back(TerminalFrameCommand::SetSnapshotPriority {
        session_ids: vec!["b".to_string(), "c".to_string()],
    });
    compact_stale_terminal_frame_commands(&mut commands);
    let priorities: Vec<_> = commands
        .iter()
        .filter_map(|command| match command {
            TerminalFrameCommand::SetSnapshotPriority { session_ids } => Some(session_ids.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(priorities.len(), 1);
    assert_eq!(priorities[0], vec!["b".to_string(), "c".to_string()]);
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
    let visible_bytes = view.screen.encode_outgoing_str(visible_text);

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
fn terminal_frame_visible_text_event_keeps_only_tail_for_ui() {
    let mut session = TerminalFrameSession::new("UTF-8", 1000);
    let recording_manager = Arc::new(nyaterm_transport::RecordingManager::new());
    let recording_pipeline =
        super::super::RecordingWritePipeline::spawn(Arc::clone(&recording_manager));
    let input = format!(
        "{}tail",
        "x".repeat(TERMINAL_FRAME_VISIBLE_TEXT_TAIL_CAP + 1024)
    );

    let event = session.process_output(
        "s1".to_string(),
        input.into_bytes(),
        "UTF-8".to_string(),
        1000,
        &recording_pipeline.writer(),
    );

    assert!(event.visible_text.len() <= TERMINAL_FRAME_VISIBLE_TEXT_TAIL_CAP);
    assert!(event.visible_text.ends_with("tail"));
    assert_eq!(
        event.recording_text_bytes,
        TERMINAL_FRAME_VISIBLE_TEXT_TAIL_CAP + 1028
    );
    recording_pipeline.writer().flush();
    let recorded = recording_manager
        .search_history(nyaterm_transport::TerminalHistorySearchRequest {
            session_id: "s1".to_string(),
            query: "tail".to_string(),
            case_sensitive: false,
            regex: false,
            whole_word: false,
            limit: Some(10),
            context_before: Some(0),
            context_after: Some(0),
            max_lines: None,
        })
        .expect("recording history search should succeed");
    assert_eq!(recorded.total, 1);
}

#[test]
fn terminal_frame_seed_does_not_replace_live_output() {
    let mut session = TerminalFrameSession::new("UTF-8", 1000);
    let recording_manager = Arc::new(nyaterm_transport::RecordingManager::new());
    let recording_pipeline =
        super::super::RecordingWritePipeline::spawn(Arc::clone(&recording_manager));

    let _ = session.process_output(
        "s1".to_string(),
        b"new banner\r\n".to_vec(),
        "UTF-8".to_string(),
        1000,
        &recording_pipeline.writer(),
    );

    session.seed("old reconnect seed\r\n".to_string(), "UTF-8", 1000);

    let snapshot = session.screen.viewport_snapshot(0);
    assert!(
        snapshot
            .rows()
            .iter()
            .any(|row| row.text.contains("new banner")),
        "live output should survive a late reconnect seed"
    );
    assert!(
        snapshot
            .rows()
            .iter()
            .all(|row| !row.text.contains("old reconnect seed")),
        "late seed must not replace the live screen"
    );
}

#[test]
fn terminal_frame_control_output_does_not_block_reconnect_seed() {
    let mut session = TerminalFrameSession::new("UTF-8", 1000);
    let recording_manager = Arc::new(nyaterm_transport::RecordingManager::new());
    let recording_pipeline =
        super::super::RecordingWritePipeline::spawn(Arc::clone(&recording_manager));

    let _ = session.process_output(
        "s1".to_string(),
        b"\x1b]0;remote".to_vec(),
        "UTF-8".to_string(),
        1000,
        &recording_pipeline.writer(),
    );
    assert!(!session.output_seen);
    let _ = session.process_output(
        "s1".to_string(),
        b" title\x07".to_vec(),
        "UTF-8".to_string(),
        1000,
        &recording_pipeline.writer(),
    );
    assert!(!session.output_seen);

    session.seed("reconnect seed\r\n".to_string(), "UTF-8", 1000);

    let snapshot = session.screen.viewport_snapshot(0);
    assert!(
        snapshot
            .rows()
            .iter()
            .any(|row| row.text.contains("reconnect seed")),
        "control-only output must not block the initial reconnect seed"
    );
}

#[test]
fn terminal_frame_search_result_current_requires_matching_revision() {
    let key = TerminalFrameSearchKey {
        query: "alpha".to_string(),
        case_sensitive: false,
        regex: false,
        whole_word: false,
        limit: 100,
        request_generation: 0,
    };
    let result = TerminalFrameSearchResult::new(key.clone(), 7, Ok(Vec::new()));
    let other_key = TerminalFrameSearchKey {
        query: "beta".to_string(),
        ..key.clone()
    };
    let newer_generation = TerminalFrameSearchKey {
        request_generation: 1,
        ..key.clone()
    };

    assert!(terminal_frame_search_result_is_current(&result, &key, 7));
    assert!(!terminal_frame_search_result_is_current(&result, &key, 8));
    assert!(!terminal_frame_search_result_is_current(
        &result, &other_key, 7
    ));
    assert!(!terminal_frame_search_result_is_current(
        &result,
        &newer_generation,
        7,
    ));
}

#[test]
fn terminal_frame_search_event_carries_current_session_revision() {
    let mut session = TerminalFrameSession::new("UTF-8", 1000);
    session.screen.advance_decoded_text("alpha\nbeta");
    session.revision = 3;
    let key = TerminalFrameSearchKey {
        query: "alpha".to_string(),
        case_sensitive: false,
        regex: false,
        whole_word: false,
        limit: 100,
        request_generation: 0,
    };

    let event = session.search_event(
        "s1".to_string(),
        TerminalFrameSearchPurpose::Find,
        key.clone(),
    );

    assert_eq!(event.session_id, "s1");
    assert_eq!(event.result.key, key);
    assert_eq!(event.result.revision, 3);
    assert_eq!(event.result.matches.unwrap().len(), 1);
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
fn terminal_protocol_state_encodes_sgr_mouse_report() {
    let protocol = TerminalProtocolState {
        mouse_reporting: true,
        mouse_sgr: true,
        ..TerminalProtocolState::default()
    };

    assert_eq!(
        protocol.encode_mouse_report(0, 1, 2, true, false, false, false, false),
        b"\x1b[<0;2;3M".to_vec()
    );
    assert_eq!(
        protocol.encode_mouse_report(0, 1, 2, false, false, false, false, false),
        b"\x1b[<0;2;3m".to_vec()
    );
}

#[test]
fn terminal_protocol_state_blocks_alternate_scroll_when_mouse_reporting() {
    let protocol = TerminalProtocolState {
        alternate_screen: true,
        alternate_scroll: true,
        mouse_reporting: true,
        application_cursor_keys: true,
        ..TerminalProtocolState::default()
    };

    assert_eq!(protocol.alternate_scroll_payload(1), None);
}

#[test]
fn terminal_protocol_state_emits_alternate_scroll_payload() {
    let protocol = TerminalProtocolState {
        alternate_screen: true,
        alternate_scroll: true,
        application_cursor_keys: true,
        ..TerminalProtocolState::default()
    };

    assert_eq!(
        protocol.alternate_scroll_payload(1),
        Some(b"\x1bOA".to_vec())
    );
}

#[test]
fn terminal_frame_event_queue_coalesces_pure_output_to_latest() {
    let queue = TerminalFrameEventQueue::new(8);
    let mut first = output_frame_with_sizes(1, 0);
    first.revision = 1;
    first.visible_text = "a".to_string();
    let mut second = output_frame_with_sizes(2, 0);
    second.revision = 2;
    second.visible_text = "bc".to_string();

    queue.push(TerminalFrameEvent::Output(first));
    queue.push(TerminalFrameEvent::Output(second));

    assert!(matches!(
        queue.try_recv(),
        Some(TerminalFrameEvent::Output(frame))
            if frame.revision == 2
                && frame.visible_text == "abc"
                && frame.accepted_bytes == 3
    ));
    assert!(queue.try_recv().is_none());
}

#[test]
fn terminal_frame_event_queue_wakes_once_after_interest_is_armed() {
    let (queue, mut wake_rx) = TerminalFrameEventQueue::new_with_wake(8);

    queue.push(TerminalFrameEvent::Output(output_frame_with_sizes(1, 0)));
    assert!(wake_rx.try_recv().is_err());

    queue.arm_wake(TERMINAL_FRAME_EVENT_WAKE_OUTPUT);
    queue
        .clone()
        .push(TerminalFrameEvent::Output(output_frame_with_sizes(1, 0)));
    assert!(matches!(wake_rx.try_recv(), Ok(())));
    assert_eq!(queue.wake_count(), 1);

    queue.push(TerminalFrameEvent::Output(output_frame_with_sizes(1, 0)));
    assert!(wake_rx.try_recv().is_err());
    assert_eq!(queue.wake_count(), 1);
}

/// The mask `arm_event_wakes` installs must leave every reply kind deliverable.
///
/// Armed with the production mask, an output wake must not disarm the snapshot
/// interest -- narrowing `TERMINAL_FRAME_EVENT_WAKE_ALL` strands snapshot replies,
/// which on a live terminal is a paint that never happens.
#[test]
fn the_drain_tasks_frame_interest_mask_covers_every_reply_kind() {
    let (queue, mut wake_rx) = TerminalFrameEventQueue::new_with_wake(8);
    queue.arm_wake(TERMINAL_FRAME_EVENT_WAKE_ALL);

    queue.push(TerminalFrameEvent::Output(output_frame_with_sizes(1, 0)));
    assert!(matches!(wake_rx.try_recv(), Ok(())));
    assert!(
        wake_rx.try_recv().is_err(),
        "output should consume only its own interest"
    );

    let screen = TerminalScreen::default();
    queue.push(TerminalFrameEvent::Snapshot(TerminalFrameSnapshotEvent {
        session_id: "s1".to_string(),
        offset: 1,
        snapshot: Arc::new(screen.snapshot()),
        action_links: None,
        revision: 1,
        snapshot_duration: Duration::ZERO,
        snapshot_stats: Default::default(),
        action_link_stats: Default::default(),
        process_duration: Duration::ZERO,
    }));
    assert!(
        matches!(wake_rx.try_recv(), Ok(())),
        "the snapshot interest must survive the output wake"
    );
    assert_eq!(queue.wake_count(), 2);
}

#[test]
fn terminal_frame_event_queue_keeps_snapshot_wake_armed_across_output() {
    let (queue, mut wake_rx) = TerminalFrameEventQueue::new_with_wake(8);
    queue.arm_wake(TERMINAL_FRAME_EVENT_WAKE_SNAPSHOT);

    queue.push(TerminalFrameEvent::Output(output_frame_with_sizes(1, 0)));
    assert!(wake_rx.try_recv().is_err());

    let screen = TerminalScreen::default();
    queue.push(TerminalFrameEvent::Snapshot(TerminalFrameSnapshotEvent {
        session_id: "s1".to_string(),
        offset: 1,
        snapshot: Arc::new(screen.snapshot()),
        action_links: None,
        revision: 1,
        snapshot_duration: Duration::ZERO,
        snapshot_stats: Default::default(),
        action_link_stats: Default::default(),
        process_duration: Duration::ZERO,
    }));
    assert!(matches!(wake_rx.try_recv(), Ok(())));
}

#[test]
fn terminal_frame_event_queue_wakes_search_results_when_terminal_is_quiet() {
    let (queue, mut wake_rx) = TerminalFrameEventQueue::new_with_wake(8);
    queue.arm_wake(TERMINAL_FRAME_EVENT_WAKE_SEARCH);

    queue.push(TerminalFrameEvent::Search(TerminalFrameSearchEvent {
        session_id: "s1".to_string(),
        purpose: TerminalFrameSearchPurpose::SelectedOccurrence,
        result: TerminalFrameSearchResult::new(
            TerminalFrameSearchKey {
                query: "term".to_string(),
                case_sensitive: true,
                regex: false,
                whole_word: false,
                limit: 8,
                request_generation: 1,
            },
            1,
            Ok(Vec::new()),
        ),
        process_duration: Duration::ZERO,
    }));

    assert!(matches!(wake_rx.try_recv(), Ok(())));
    assert_eq!(queue.wake_count(), 1);
    assert!(matches!(
        queue.try_recv(),
        Some(TerminalFrameEvent::Search(_))
    ));
}

#[test]
fn terminal_frame_event_queue_preserves_output_effects() {
    let queue = TerminalFrameEventQueue::new(8);
    let mut effect_frame = output_frame_with_sizes(1, 0);
    effect_frame.revision = 1;
    effect_frame.effects.bell = true;
    let mut latest = output_frame_with_sizes(2, 0);
    latest.revision = 2;

    queue.push(TerminalFrameEvent::Output(effect_frame));
    queue.push(TerminalFrameEvent::Output(latest));

    assert!(matches!(
        queue.try_recv(),
        Some(TerminalFrameEvent::Output(frame)) if frame.revision == 1 && frame.effects.bell
    ));
    assert!(matches!(
        queue.try_recv(),
        Some(TerminalFrameEvent::Output(frame)) if frame.revision == 2
    ));
    assert!(queue.try_recv().is_none());
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
fn terminal_frame_event_queue_drains_batch_with_limit() {
    let queue = TerminalFrameEventQueue::new(8);
    let mut first = output_frame_with_sizes(1, 0);
    first.revision = 1;
    first.effects.bell = true;
    let mut second = output_frame_with_sizes(2, 0);
    second.revision = 2;
    second.effects.bell = true;
    let mut third = output_frame_with_sizes(3, 0);
    third.revision = 3;
    third.effects.bell = true;

    queue.push(TerminalFrameEvent::Output(first));
    queue.push(TerminalFrameEvent::Output(second));
    queue.push(TerminalFrameEvent::Output(third));

    let mut drained = VecDeque::new();
    assert_eq!(queue.drain_into(&mut drained, 2), 2);
    assert_eq!(drained.len(), 2);
    assert!(matches!(
        drained.pop_front(),
        Some(TerminalFrameEvent::Output(frame)) if frame.revision == 1
    ));
    assert!(matches!(
        drained.pop_front(),
        Some(TerminalFrameEvent::Output(frame)) if frame.revision == 2
    ));
    assert!(matches!(
        queue.try_recv(),
        Some(TerminalFrameEvent::Output(frame)) if frame.revision == 3
    ));
    assert_eq!(queue.len(), 0);
}

#[test]
fn terminal_frame_action_links_align_with_snapshot_lines() {
    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text("visit http://example.com\nping 10.0.0.1");
    let snapshot = screen.viewport_snapshot(0);
    let matchers = ActionLinksMatcherSettings::default();

    let links = prepare_terminal_frame_action_links(&snapshot, true, &matchers).unwrap();
    let absolute_end_row = snapshot.total_rows.saturating_sub(snapshot.display_offset);
    let absolute_start_row = absolute_end_row.saturating_sub(snapshot.row_count());

    assert_eq!(links.absolute_start_row, absolute_start_row);
    assert_eq!(links.absolute_end_row, absolute_end_row);
    assert_eq!(links.matches_by_line.len(), snapshot.row_count());
    assert_eq!(links.cell_ranges_by_line.len(), snapshot.row_count());
    assert!(
        links
            .matches_by_line
            .iter()
            .flatten()
            .any(|item| item.value == "http://example.com")
    );
    assert!(
        links
            .cell_ranges_by_line
            .iter()
            .flatten()
            .any(|range| *range == (6, 24))
    );
    assert!(
        links
            .matches_by_line
            .iter()
            .flatten()
            .any(|item| item.value == "10.0.0.1")
    );

    let disabled = prepare_terminal_frame_action_links(&snapshot, false, &matchers).unwrap();
    assert!(disabled.matches_by_line.iter().all(Vec::is_empty));
    assert!(disabled.cell_ranges_by_line.iter().all(Vec::is_empty));
    assert_ne!(links.matcher_key, disabled.matcher_key);
}

#[test]
fn terminal_frame_action_links_reuse_unchanged_rows() {
    let mut screen = TerminalScreen::new(80, 8);
    screen.advance_decoded_text("visit http://example.com");
    let first_snapshot = screen.viewport_snapshot(0);
    let matchers = ActionLinksMatcherSettings::default();
    let first = prepare_terminal_frame_action_links(&first_snapshot, true, &matchers).unwrap();

    let (_, unchanged_stats) =
        prepare_terminal_frame_action_links_reusing(&first_snapshot, true, &matchers, Some(&first));
    assert_eq!(unchanged_stats.reused_rows, first_snapshot.row_count());
    assert_eq!(unchanged_stats.rebuilt_rows, 0);

    screen.advance_decoded_text(" updated");
    let changed_snapshot = screen.viewport_snapshot(0);
    let (_, changed_stats) = prepare_terminal_frame_action_links_reusing(
        &changed_snapshot,
        true,
        &matchers,
        Some(&first),
    );
    assert!(changed_stats.reused_rows > 0);
    assert!(changed_stats.rebuilt_rows > 0);
    assert_eq!(
        changed_stats.reused_rows + changed_stats.rebuilt_rows,
        changed_snapshot.row_count()
    );
}

#[test]
fn terminal_frame_action_link_matcher_change_rebuilds_all_rows() {
    let mut screen = TerminalScreen::new(80, 8);
    screen.advance_decoded_text("visit http://example.com and 10.0.0.1");
    let snapshot = screen.viewport_snapshot(0);
    let matchers = ActionLinksMatcherSettings::default();
    let first = prepare_terminal_frame_action_links(&snapshot, true, &matchers).unwrap();
    let mut changed_matchers = matchers.clone();
    changed_matchers.ipv4 = false;

    let (_, stats) = prepare_terminal_frame_action_links_reusing(
        &snapshot,
        true,
        &changed_matchers,
        Some(&first),
    );
    assert_eq!(stats.reused_rows, 0);
    assert_eq!(stats.rebuilt_rows, snapshot.row_count());
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

#[test]
fn terminal_frame_worker_coalesces_adjacent_matching_output() {
    let (tx, rx) = terminal_frame_command_channel();
    assert!(tx.send(TerminalFrameCommand::Output {
        session_id: "s1".to_string(),
        data: b"bc".to_vec(),
        encoding: "UTF-8".to_string(),
        scrollback_limit: 1000,
    }));
    drop(tx);

    let mut pending = VecDeque::new();
    let (_, data, _, _) = coalesce_terminal_frame_output_command(
        &rx,
        &mut pending,
        "s1".to_string(),
        b"a".to_vec(),
        "UTF-8".to_string(),
        1000,
        TERMINAL_FRAME_OUTPUT_COALESCE_BYTE_LIMIT,
    );

    assert_eq!(data, b"abc");
    assert!(pending.is_empty());
}

#[test]
fn terminal_frame_worker_caps_coalesced_output_batch() {
    let (tx, rx) = terminal_frame_command_channel();
    assert!(tx.send(TerminalFrameCommand::Output {
        session_id: "s1".to_string(),
        data: vec![b'b'; 2],
        encoding: "UTF-8".to_string(),
        scrollback_limit: 1000,
    }));
    drop(tx);

    let mut pending = VecDeque::new();
    let (_, data, _, _) = coalesce_terminal_frame_output_command(
        &rx,
        &mut pending,
        "s1".to_string(),
        vec![b'a'; TERMINAL_FRAME_OUTPUT_COALESCE_BYTE_LIMIT - 1],
        "UTF-8".to_string(),
        1000,
        TERMINAL_FRAME_OUTPUT_COALESCE_BYTE_LIMIT,
    );

    assert_eq!(data.len(), TERMINAL_FRAME_OUTPUT_COALESCE_BYTE_LIMIT - 1);
    assert!(matches!(
        next_terminal_frame_command(&rx, &mut pending),
        Some(TerminalFrameCommand::Output { data, .. }) if data == vec![b'b'; 2]
    ));
}

#[test]
fn terminal_frame_worker_does_not_coalesce_across_resize() {
    let (tx, rx) = terminal_frame_command_channel();
    assert!(tx.send(TerminalFrameCommand::ResizeSession {
        session_id: "s1".to_string(),
        cols: 100,
        rows: 30,
    }));
    assert!(tx.send(TerminalFrameCommand::Output {
        session_id: "s1".to_string(),
        data: b"bc".to_vec(),
        encoding: "UTF-8".to_string(),
        scrollback_limit: 1000,
    }));
    drop(tx);

    let mut pending = VecDeque::new();
    let (_, data, _, _) = coalesce_terminal_frame_output_command(
        &rx,
        &mut pending,
        "s1".to_string(),
        b"a".to_vec(),
        "UTF-8".to_string(),
        1000,
        TERMINAL_FRAME_OUTPUT_COALESCE_BYTE_LIMIT,
    );

    assert_eq!(data, b"a");
    assert!(matches!(
        next_terminal_frame_command(&rx, &mut pending),
        Some(TerminalFrameCommand::ResizeSession { .. })
    ));
    assert!(matches!(
        next_terminal_frame_command(&rx, &mut pending),
        Some(TerminalFrameCommand::Output { .. })
    ));
}

#[test]
fn terminal_frame_worker_batches_output_burst_into_single_frame() {
    let (tx, rx) = terminal_frame_command_channel();
    assert!(tx.send(TerminalFrameCommand::Output {
        session_id: "s1".to_string(),
        data: b"bc".to_vec(),
        encoding: "UTF-8".to_string(),
        scrollback_limit: 1000,
    }));
    drop(tx);

    let recording_manager = Arc::new(nyaterm_transport::RecordingManager::new());
    let recording_pipeline =
        super::super::RecordingWritePipeline::spawn(Arc::clone(&recording_manager));
    let mut pending = VecDeque::new();
    let mut sessions = HashMap::new();
    let event_queue = TerminalFrameEventQueue::new(8);
    process_terminal_frame_output_burst(
        &rx,
        &mut pending,
        &mut sessions,
        &recording_pipeline.writer(),
        "s1".to_string(),
        b"a".to_vec(),
        "UTF-8".to_string(),
        1000,
        |event| event_queue.push(TerminalFrameEvent::Output(event)),
    );
    let Some(TerminalFrameEvent::Output(event)) = event_queue.try_recv() else {
        panic!("worker should emit one coalesced output frame");
    };

    assert_eq!(event.visible_text, "abc");
    assert_eq!(event.recording_text_bytes, 3);
    assert_eq!(event.accepted_bytes, 3);
    assert_eq!(event.revision, 2);
    assert!(
        event
            .snapshot
            .as_ref()
            .unwrap()
            .rows()
            .iter()
            .map(|row| row.text.as_str())
            .collect::<String>()
            .contains("abc")
    );
    assert!(pending.is_empty());
    assert!(event_queue.try_recv().is_none());
}

#[test]
fn terminal_frame_worker_collects_trailing_output_before_emitting() {
    let (tx, rx) = terminal_frame_command_channel();
    assert!(tx.send(TerminalFrameCommand::Output {
        session_id: "s1".to_string(),
        data: b"bc".to_vec(),
        encoding: "UTF-8".to_string(),
        scrollback_limit: 1000,
    }));
    drop(tx);
    let recording_manager = Arc::new(nyaterm_transport::RecordingManager::new());
    let recording_pipeline =
        super::super::RecordingWritePipeline::spawn(Arc::clone(&recording_manager));
    let mut pending = VecDeque::new();
    let mut sessions = HashMap::new();
    let mut events = Vec::new();

    process_terminal_frame_output_burst(
        &rx,
        &mut pending,
        &mut sessions,
        &recording_pipeline.writer(),
        "s1".to_string(),
        b"a".to_vec(),
        "UTF-8".to_string(),
        1000,
        |event| events.push(event),
    );

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].visible_text, "abc");
    assert_eq!(events[0].revision, 2);
}

#[test]
fn terminal_frame_worker_amortizes_snapshot_across_sixteen_pty_chunks() {
    let (tx, rx) = terminal_frame_command_channel();
    for _ in 0..16 {
        assert!(tx.send(TerminalFrameCommand::Output {
            session_id: "s1".to_string(),
            data: vec![b'x'; TERMINAL_FRAME_OUTPUT_CHUNK_SIZE],
            encoding: "UTF-8".to_string(),
            scrollback_limit: 1000,
        }));
    }
    drop(tx);

    let recording_manager = Arc::new(nyaterm_transport::RecordingManager::new());
    let recording_pipeline =
        super::super::RecordingWritePipeline::spawn(Arc::clone(&recording_manager));
    let mut pending = VecDeque::new();
    let mut sessions = HashMap::new();
    let mut events = Vec::new();
    process_terminal_frame_output_burst(
        &rx,
        &mut pending,
        &mut sessions,
        &recording_pipeline.writer(),
        "s1".to_string(),
        vec![b'x'; TERMINAL_FRAME_OUTPUT_CHUNK_SIZE],
        "UTF-8".to_string(),
        1000,
        |event| events.push(event),
    );

    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].accepted_bytes,
        TERMINAL_FRAME_OUTPUT_BURST_BYTE_LIMIT
    );
    assert!(pending.is_empty());
    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::Output { data, .. })
            if data.len() == TERMINAL_FRAME_OUTPUT_CHUNK_SIZE
    ));
}

#[test]
fn terminal_frame_worker_batch_stops_at_resize_boundary() {
    let (tx, rx) = terminal_frame_command_channel();
    assert!(tx.send(TerminalFrameCommand::ResizeSession {
        session_id: "s1".to_string(),
        cols: 100,
        rows: 30,
    }));
    assert!(tx.send(TerminalFrameCommand::Output {
        session_id: "s1".to_string(),
        data: b"bc".to_vec(),
        encoding: "UTF-8".to_string(),
        scrollback_limit: 1000,
    }));
    drop(tx);

    let recording_manager = Arc::new(nyaterm_transport::RecordingManager::new());
    let recording_pipeline =
        super::super::RecordingWritePipeline::spawn(Arc::clone(&recording_manager));
    let mut pending = VecDeque::new();
    let mut sessions = HashMap::new();
    let mut events = Vec::new();
    process_terminal_frame_output_burst(
        &rx,
        &mut pending,
        &mut sessions,
        &recording_pipeline.writer(),
        "s1".to_string(),
        b"a".to_vec(),
        "UTF-8".to_string(),
        1000,
        |event| events.push(event),
    );

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].visible_text, "a");
    assert!(matches!(
        next_terminal_frame_command(&rx, &mut pending),
        Some(TerminalFrameCommand::ResizeSession { .. })
    ));
    assert!(matches!(
        next_terminal_frame_command(&rx, &mut pending),
        Some(TerminalFrameCommand::Output { .. })
    ));
}

/// The burst coalesces what has already arrived and stops there. It must
/// not block hoping for more: the sender below is still very much alive,
/// and holding finished bytes back for it would sit on the echo path.
#[test]
fn terminal_frame_output_burst_does_not_wait_for_a_live_sender() {
    let (tx, rx) = terminal_frame_command_channel();

    let recording_manager = Arc::new(nyaterm_transport::RecordingManager::new());
    let recording_pipeline =
        super::super::RecordingWritePipeline::spawn(Arc::clone(&recording_manager));
    let mut pending = VecDeque::new();
    let mut sessions = HashMap::new();
    let mut events = Vec::new();
    process_terminal_frame_output_burst(
        &rx,
        &mut pending,
        &mut sessions,
        &recording_pipeline.writer(),
        "s1".to_string(),
        b"echo".to_vec(),
        "UTF-8".to_string(),
        1000,
        |event| events.push(event),
    );

    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].accepted_bytes, 4,
        "only the bytes already in hand should have been folded in"
    );

    // Whatever the sender produces next is the *next* burst's work.
    assert!(tx.send(TerminalFrameCommand::Output {
        session_id: "s1".to_string(),
        data: b"later".to_vec(),
        encoding: "UTF-8".to_string(),
        scrollback_limit: 1000,
    }));
    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::Output { data, .. }) if data == b"later"
    ));
}

#[test]
fn terminal_frame_command_queue_merges_small_pty_reads_for_worker() {
    let (tx, rx) = terminal_frame_command_channel();
    assert!(tx.send(TerminalFrameCommand::Output {
        session_id: "s1".to_string(),
        data: b"a".to_vec(),
        encoding: "UTF-8".to_string(),
        scrollback_limit: 1000,
    }));
    assert!(tx.send(TerminalFrameCommand::Output {
        session_id: "s1".to_string(),
        data: b"bc".to_vec(),
        encoding: "UTF-8".to_string(),
        scrollback_limit: 1000,
    }));

    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::Output { data, .. }) if data == b"abc"
    ));
    assert!(rx.try_recv().is_none());
}

#[test]
fn terminal_frame_command_queue_caps_merged_output_at_worker_chunk_size() {
    let (tx, rx) = terminal_frame_command_channel();
    assert!(tx.send(TerminalFrameCommand::Output {
        session_id: "s1".to_string(),
        data: vec![b'a'; TERMINAL_FRAME_OUTPUT_CHUNK_SIZE - 1],
        encoding: "UTF-8".to_string(),
        scrollback_limit: 1000,
    }));
    assert!(tx.send(TerminalFrameCommand::Output {
        session_id: "s1".to_string(),
        data: b"bc".to_vec(),
        encoding: "UTF-8".to_string(),
        scrollback_limit: 1000,
    }));

    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::Output { data, .. })
            if data.len() == TERMINAL_FRAME_OUTPUT_CHUNK_SIZE - 1
    ));
    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::Output { data, .. }) if data == b"bc"
    ));
    assert!(rx.try_recv().is_none());
}

#[test]
fn terminal_frame_command_queue_does_not_coalesce_across_resize() {
    let (tx, rx) = terminal_frame_command_channel();
    assert!(tx.send(TerminalFrameCommand::Output {
        session_id: "s1".to_string(),
        data: b"a".to_vec(),
        encoding: "UTF-8".to_string(),
        scrollback_limit: 1000,
    }));
    assert!(tx.send(TerminalFrameCommand::ResizeSession {
        session_id: "s1".to_string(),
        cols: 100,
        rows: 30,
    }));
    assert!(tx.send(TerminalFrameCommand::Output {
        session_id: "s1".to_string(),
        data: b"bc".to_vec(),
        encoding: "UTF-8".to_string(),
        scrollback_limit: 1000,
    }));

    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::Output { data, .. }) if data == b"a"
    ));
    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::ResizeSession { .. })
    ));
    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::Output { data, .. }) if data == b"bc"
    ));
}

#[test]
fn terminal_frame_command_queue_keeps_latest_search_per_session() {
    let (tx, rx) = terminal_frame_command_channel();
    assert!(tx.send(TerminalFrameCommand::RequestSearch {
        session_id: "s1".to_string(),
        purpose: TerminalFrameSearchPurpose::Find,
        key: TerminalFrameSearchKey {
            query: "old".to_string(),
            case_sensitive: false,
            regex: false,
            whole_word: false,
            limit: 100,
            request_generation: 0,
        },
    }));
    assert!(tx.send(TerminalFrameCommand::RequestSearch {
        session_id: "s1".to_string(),
        purpose: TerminalFrameSearchPurpose::SelectedOccurrence,
        key: TerminalFrameSearchKey {
            query: "selected".to_string(),
            case_sensitive: true,
            regex: false,
            whole_word: false,
            limit: 2000,
            request_generation: 1,
        },
    }));
    assert!(tx.send(TerminalFrameCommand::RequestSearch {
        session_id: "s1".to_string(),
        purpose: TerminalFrameSearchPurpose::Find,
        key: TerminalFrameSearchKey {
            query: "new".to_string(),
            case_sensitive: false,
            regex: false,
            whole_word: false,
            limit: 100,
            request_generation: 0,
        },
    }));

    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::RequestSearch { key, purpose, .. })
            if key.query == "selected" && purpose == TerminalFrameSearchPurpose::SelectedOccurrence
    ));
    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::RequestSearch { key, purpose, .. })
            if key.query == "new" && purpose == TerminalFrameSearchPurpose::Find
    ));
    assert!(rx.try_recv().is_none());
}

#[test]
fn selected_occurrence_search_advances_in_256_row_chunks() {
    let session = selected_occurrence_test_session(80, 600, "needle");
    let mut job = SelectedOccurrenceSearchJob::new(
        "s1".to_string(),
        selected_occurrence_test_key("needle", 2000, 1),
        &session,
    );

    assert!(!job.process_chunk(&session).unwrap());
    assert_eq!(job.next_absolute_row, SELECTED_OCCURRENCE_SEARCH_CHUNK_ROWS);
    assert!(!job.process_chunk(&session).unwrap());
    assert_eq!(
        job.next_absolute_row,
        SELECTED_OCCURRENCE_SEARCH_CHUNK_ROWS * 2
    );
    assert!(job.process_chunk(&session).unwrap());
    assert_eq!(job.next_absolute_row, job.total_rows);
}

#[test]
fn selected_occurrence_search_deduplicates_soft_wrap_matches_across_chunks() {
    let mut session = selected_occurrence_test_session(4, 255, "x");
    session.screen.advance(b"xxaddress");
    session.revision = session.revision.saturating_add(1);
    let key = selected_occurrence_test_key("address", 2000, 2);
    let expected = session
        .screen
        .search_grid(&nyaterm_terminal::TerminalSearchQuery {
            pattern: key.query.clone(),
            regex: false,
            case_sensitive: true,
            whole_word: false,
            direction: nyaterm_terminal::TerminalSearchDirection::Forward,
            limit: key.limit,
        })
        .unwrap();
    let mut jobs = VecDeque::from([SelectedOccurrenceSearchJob::new(
        "s1".to_string(),
        key,
        &session,
    )]);
    let sessions = HashMap::from([("s1".to_string(), session)]);

    let event = loop {
        if let Some(event) = process_next_selected_occurrence_search_chunk(&mut jobs, &sessions) {
            break event;
        }
    };
    let matches = event.result.matches.unwrap();
    let actual = matches
        .iter()
        .map(|m| (m.line_index, m.start_col, m.end_col))
        .collect::<Vec<_>>();
    let expected = expected
        .iter()
        .map(|m| (m.line_index, m.start_col, m.end_col))
        .collect::<Vec<_>>();

    assert_eq!(actual, expected);
}

#[test]
fn selected_occurrence_search_stops_at_match_limit() {
    let session = selected_occurrence_test_session(80, 2500, "needle");
    let mut jobs = VecDeque::from([SelectedOccurrenceSearchJob::new(
        "s1".to_string(),
        selected_occurrence_test_key("needle", 2000, 1),
        &session,
    )]);
    let sessions = HashMap::from([("s1".to_string(), session)]);

    let event = loop {
        if let Some(event) = process_next_selected_occurrence_search_chunk(&mut jobs, &sessions) {
            break event;
        }
    };

    assert_eq!(event.result.matches.unwrap().len(), 2000);
}

#[test]
fn selected_occurrence_search_cancels_for_revision_and_session_removal() {
    let mut session = selected_occurrence_test_session(80, 600, "needle");
    let revision_job = SelectedOccurrenceSearchJob::new(
        "s1".to_string(),
        selected_occurrence_test_key("needle", 2000, 1),
        &session,
    );
    session.revision = session.revision.saturating_add(1);
    let mut revision_jobs = VecDeque::from([revision_job]);
    let sessions = HashMap::from([("s1".to_string(), session)]);

    let revision_event =
        process_next_selected_occurrence_search_chunk(&mut revision_jobs, &sessions).unwrap();
    assert!(revision_event.result.matches.is_err());

    let session = selected_occurrence_test_session(80, 600, "needle");
    let mut removed_jobs = VecDeque::from([SelectedOccurrenceSearchJob::new(
        "removed".to_string(),
        selected_occurrence_test_key("needle", 2000, 2),
        &session,
    )]);
    let removed_event =
        process_next_selected_occurrence_search_chunk(&mut removed_jobs, &HashMap::new()).unwrap();
    assert!(removed_event.result.matches.is_err());
}

#[test]
fn selected_occurrence_new_generation_replaces_pending_job() {
    let session = selected_occurrence_test_session(80, 600, "needle");
    let mut jobs = VecDeque::from([SelectedOccurrenceSearchJob::new(
        "s1".to_string(),
        selected_occurrence_test_key("needle", 2000, 1),
        &session,
    )]);
    let replacement = SelectedOccurrenceSearchJob::new(
        "s1".to_string(),
        selected_occurrence_test_key("needle", 2000, 2),
        &session,
    );

    let stale = replace_selected_occurrence_search_job(&mut jobs, replacement).unwrap();

    assert_eq!(stale.result.key.request_generation, 1);
    assert!(stale.result.matches.is_err());
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].key.request_generation, 2);
}

#[test]
fn terminal_output_command_is_taken_before_next_search_chunk() {
    let session = selected_occurrence_test_session(80, 600, "needle");
    let mut job = SelectedOccurrenceSearchJob::new(
        "s1".to_string(),
        selected_occurrence_test_key("needle", 2000, 1),
        &session,
    );
    assert!(!job.process_chunk(&session).unwrap());
    let next_row = job.next_absolute_row;
    let (tx, rx) = terminal_frame_command_channel();
    assert!(tx.send(TerminalFrameCommand::Output {
        session_id: "s1".to_string(),
        data: b"new output".to_vec(),
        encoding: "UTF-8".to_string(),
        scrollback_limit: 1000,
    }));

    let command = try_next_terminal_frame_command(&rx, &mut VecDeque::new());

    assert!(matches!(command, Some(TerminalFrameCommand::Output { .. })));
    assert_eq!(job.next_absolute_row, next_row);
}

#[test]
#[ignore = "performance benchmark; run manually with --ignored --nocapture"]
fn selected_occurrence_search_large_scrollback_benchmark() {
    for lines in [5000, 100_000] {
        let session = selected_occurrence_test_session(80, lines, "unique-line");
        let mut job = SelectedOccurrenceSearchJob::new(
            "bench".to_string(),
            selected_occurrence_test_key("missing-needle", 2000, 1),
            &session,
        );
        let started = Instant::now();
        let mut max_chunk = Duration::ZERO;
        let mut chunks = 0usize;
        loop {
            let chunk_started = Instant::now();
            let done = job.process_chunk(&session).unwrap();
            max_chunk = max_chunk.max(chunk_started.elapsed());
            chunks += 1;
            if done {
                break;
            }
        }
        eprintln!(
            "selected occurrence benchmark: lines={lines} chunks={chunks} total={:?} max_chunk={max_chunk:?}",
            started.elapsed()
        );
    }
}

#[test]
fn terminal_frame_command_queue_keeps_latest_resize_per_session() {
    let (tx, rx) = terminal_frame_command_channel();
    assert!(tx.send(TerminalFrameCommand::ResizeSession {
        session_id: "s1".to_string(),
        cols: 80,
        rows: 24,
    }));
    assert!(tx.send(TerminalFrameCommand::ResizeSession {
        session_id: "s1".to_string(),
        cols: 120,
        rows: 40,
    }));

    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::ResizeSession {
            cols: 120,
            rows: 40,
            ..
        })
    ));
    assert!(rx.try_recv().is_none());
}

#[test]
fn terminal_frame_command_queue_keeps_resize_when_output_intervenes() {
    let (tx, rx) = terminal_frame_command_channel();
    assert!(tx.send(TerminalFrameCommand::ResizeSession {
        session_id: "s1".to_string(),
        cols: 80,
        rows: 24,
    }));
    assert!(tx.send(TerminalFrameCommand::Output {
        session_id: "s1".to_string(),
        data: b"a".to_vec(),
        encoding: "UTF-8".to_string(),
        scrollback_limit: 1000,
    }));
    assert!(tx.send(TerminalFrameCommand::ResizeSession {
        session_id: "s1".to_string(),
        cols: 120,
        rows: 40,
    }));

    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::ResizeSession {
            cols: 80,
            rows: 24,
            ..
        })
    ));
    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::Output { data, .. }) if data == b"a"
    ));
    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::ResizeSession {
            cols: 120,
            rows: 40,
            ..
        })
    ));
    assert!(rx.try_recv().is_none());
}

#[test]
fn terminal_frame_command_queue_runs_output_before_idle_snapshot() {
    let (tx, rx) = terminal_frame_command_channel();
    assert!(tx.send(TerminalFrameCommand::RequestSnapshot {
        session_id: "s1".to_string(),
        offset: 0,
        action_links_enabled: false,
        action_link_matchers: ActionLinksMatcherSettings::default(),
        priority: false,
        purpose: TerminalFrameSnapshotPurpose::ActionLinkEnrichment,
    }));
    assert!(tx.send(TerminalFrameCommand::Output {
        session_id: "s1".to_string(),
        data: b"echo".to_vec(),
        encoding: "UTF-8".to_string(),
        scrollback_limit: 1000,
    }));

    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::Output { data, .. }) if data == b"echo"
    ));
    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::RequestSnapshot {
            offset: 0,
            priority: false,
            ..
        })
    ));
    assert!(rx.try_recv().is_none());
}

#[test]
fn terminal_frame_command_queue_caps_rebuildable_render_requests() {
    let (tx, rx) = terminal_frame_command_channel();
    for offset in 0..TERMINAL_FRAME_COMMAND_QUEUE_CAP + 32 {
        assert!(tx.send(TerminalFrameCommand::RequestSnapshot {
            session_id: format!("s{offset}"),
            offset,
            action_links_enabled: false,
            action_link_matchers: ActionLinksMatcherSettings::default(),
            priority: false,
            purpose: TerminalFrameSnapshotPurpose::Paint,
        }));
    }

    assert_eq!(tx.len(), TERMINAL_FRAME_COMMAND_QUEUE_CAP);
    let mut drained = 0usize;
    while let Some(command) = rx.try_recv() {
        assert!(matches!(
            command,
            TerminalFrameCommand::RequestSnapshot { .. }
        ));
        drained += 1;
    }
    assert_eq!(drained, TERMINAL_FRAME_COMMAND_QUEUE_CAP);
}

#[test]
fn terminal_frame_command_queue_prioritizes_user_scroll_snapshot() {
    let (tx, rx) = terminal_frame_command_channel();
    assert!(tx.send(TerminalFrameCommand::Output {
        session_id: "s1".to_string(),
        data: b"abc".to_vec(),
        encoding: "UTF-8".to_string(),
        scrollback_limit: 1000,
    }));
    assert!(tx.send(TerminalFrameCommand::RequestSearch {
        session_id: "s1".to_string(),
        purpose: TerminalFrameSearchPurpose::Find,
        key: TerminalFrameSearchKey {
            query: "needle".to_string(),
            case_sensitive: false,
            regex: false,
            whole_word: false,
            limit: 100,
            request_generation: 0,
        },
    }));
    assert!(tx.send(TerminalFrameCommand::RequestSnapshot {
        session_id: "s1".to_string(),
        offset: 12,
        action_links_enabled: false,
        action_link_matchers: ActionLinksMatcherSettings::default(),
        priority: true,
        purpose: TerminalFrameSnapshotPurpose::Paint,
    }));

    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::RequestSnapshot {
            offset: 12,
            priority: true,
            ..
        })
    ));
    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::Output { .. })
    ));
    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::RequestSearch { .. })
    ));
}

#[test]
fn terminal_frame_command_queue_keeps_latest_user_scroll_target_per_session() {
    let (tx, rx) = terminal_frame_command_channel();
    for offset in [12, 24, 48] {
        assert!(tx.send(TerminalFrameCommand::RequestSnapshot {
            session_id: "s1".to_string(),
            offset,
            action_links_enabled: false,
            action_link_matchers: ActionLinksMatcherSettings::default(),
            priority: true,
            purpose: TerminalFrameSnapshotPurpose::Paint,
        }));
    }

    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::RequestSnapshot {
            session_id,
            offset: 48,
            priority: true,
            ..
        }) if session_id == "s1"
    ));
    assert!(rx.try_recv().is_none());
}

#[test]
fn terminal_frame_command_queue_keeps_priority_snapshot_under_cap() {
    let (tx, rx) = terminal_frame_command_channel();
    assert!(tx.send(TerminalFrameCommand::RequestSnapshot {
        session_id: "active".to_string(),
        offset: 9,
        action_links_enabled: false,
        action_link_matchers: ActionLinksMatcherSettings::default(),
        priority: true,
        purpose: TerminalFrameSnapshotPurpose::Paint,
    }));
    for offset in 0..TERMINAL_FRAME_COMMAND_QUEUE_CAP + 32 {
        assert!(tx.send(TerminalFrameCommand::RequestSnapshot {
            session_id: format!("s{offset}"),
            offset,
            action_links_enabled: false,
            action_link_matchers: ActionLinksMatcherSettings::default(),
            priority: false,
            purpose: TerminalFrameSnapshotPurpose::Paint,
        }));
    }

    let mut saw_priority = false;
    while let Some(command) = rx.try_recv() {
        if matches!(
            command,
            TerminalFrameCommand::RequestSnapshot {
                session_id,
                offset: 9,
                priority: true,
                ..
            } if session_id == "active"
        ) {
            saw_priority = true;
        }
    }
    assert!(saw_priority);
}

#[test]
fn terminal_frame_command_queue_reports_queued_output_bytes() {
    let (tx, rx) = terminal_frame_command_channel();
    assert!(tx.send(TerminalFrameCommand::Output {
        session_id: "s1".to_string(),
        data: b"abc".to_vec(),
        encoding: "UTF-8".to_string(),
        scrollback_limit: 1000,
    }));
    assert!(tx.send(TerminalFrameCommand::ResizeSession {
        session_id: "s1".to_string(),
        cols: 100,
        rows: 30,
    }));
    assert!(tx.send(TerminalFrameCommand::Output {
        session_id: "s1".to_string(),
        data: b"de".to_vec(),
        encoding: "UTF-8".to_string(),
        scrollback_limit: 1000,
    }));

    assert_eq!(tx.queued_output_bytes(), 5);
    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::Output { data, .. }) if data == b"abc"
    ));
    assert_eq!(tx.queued_output_bytes(), 2);
    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::ResizeSession { .. })
    ));
    assert_eq!(tx.queued_output_bytes(), 2);
    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::Output { data, .. }) if data == b"de"
    ));
    assert_eq!(tx.queued_output_bytes(), 0);
}

#[test]
fn terminal_frame_command_queue_sends_many_in_order() {
    let (tx, rx) = terminal_frame_command_channel();
    assert!(!tx.send_many(Vec::<TerminalFrameCommand>::new()));
    assert!(tx.send_many(vec![
        TerminalFrameCommand::Output {
            session_id: "s1".to_string(),
            data: b"abc".to_vec(),
            encoding: "UTF-8".to_string(),
            scrollback_limit: 1000,
        },
        TerminalFrameCommand::ResizeSession {
            session_id: "s1".to_string(),
            cols: 100,
            rows: 30,
        },
        TerminalFrameCommand::Output {
            session_id: "s1".to_string(),
            data: b"de".to_vec(),
            encoding: "UTF-8".to_string(),
            scrollback_limit: 1000,
        },
    ]));

    assert_eq!(tx.queued_output_bytes(), 5);
    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::Output { data, .. }) if data == b"abc"
    ));
    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::ResizeSession {
            cols: 100,
            rows: 30,
            ..
        })
    ));
    assert!(matches!(
        rx.try_recv(),
        Some(TerminalFrameCommand::Output { data, .. }) if data == b"de"
    ));
    assert!(rx.try_recv().is_none());
}

#[test]
fn terminal_frame_command_queue_stops_after_sender_drop() {
    let (tx, rx) = terminal_frame_command_channel();
    drop(tx);

    assert!(rx.recv().is_none());
}

/// Two observations spanning the calm window, which is the minimum the accounting
/// can honestly claim: output between ticks shows up as `output_burst_bytes`, so a
/// span it never observed cannot count as calm.
fn tick_through_calm_window(view: &mut TerminalViewState, start: Instant) {
    view.tick_performance_overlay(false, start);
    view.tick_performance_overlay(false, start + TERMINAL_RENDER_DEGRADATION_RECOVERY_CALM);
}

#[test]
fn terminal_frame_keeps_output_tail_while_render_is_degraded() {
    let mut view = TerminalViewState::new();
    assert!(view.render_degraded);

    apply_output_frame_to_view(
        &mut view,
        TerminalFrameOutputEvent {
            session_id: "s1".to_string(),
            visible_text: "Debian banner\r\nuser@host:~$ ".to_string(),
            recording_text_bytes: 0,
            snapshot: Some(Arc::new(TerminalScreen::default().viewport_snapshot(0))),
            action_links: None,
            protocol_state: TerminalProtocolState::default(),
            effects: TerminalEffects::default(),
            command_running: false,
            accepted_bytes: 1,
            skipped_output_bytes: 0,
            revision: 1,
            snapshot_duration: Duration::ZERO,
            snapshot_stats: Default::default(),
            process_duration: Duration::ZERO,
        },
    );

    assert_eq!(view.output, "Debian banner\r\nuser@host:~$ ");
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
