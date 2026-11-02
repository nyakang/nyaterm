use std::sync::{Arc, Weak};

use super::{
    CursorShape, GraphicsProtocol, ScreenLineState, ShellCommandMark, ShellInputLineKind,
    TERMINAL_SNAPSHOT_ROW_CACHE_LIMIT, TerminalOutputDecoder, TerminalScreen,
    TerminalSearchDirection, TerminalSearchQuery, TerminalSearchRangeMode, TerminalSnapshot,
    TerminalSnapshotRowCache, TerminalSnapshotRowCacheEntry, TerminalSnapshotRowCacheKey,
    alternate_scroll_key_bytes, encode_mouse_report, encode_mouse_report_with_modifiers,
    render_row_signature,
};

fn snapshot_text(snapshot: &TerminalSnapshot) -> String {
    snapshot
        .rows()
        .iter()
        .map(|row| row.text.as_str())
        .collect()
}

#[test]
fn snapshot_distinguishes_delayed_soft_wraps_from_hard_breaks() {
    let mut screen = TerminalScreen::new(4, 3);
    screen.advance(b"ab  ");
    let pending = screen.snapshot();
    assert!(!pending.row(0).unwrap().wraps_to_next);

    screen.advance(b"X");
    let wrapped = screen.snapshot();
    assert!(wrapped.row(0).unwrap().wraps_to_next);
    assert!(!wrapped.row(0).unwrap().wrapped);
    assert!(wrapped.row(1).unwrap().wrapped);
    assert!(!wrapped.row(1).unwrap().wraps_to_next);
    assert_eq!(pending.row(0).unwrap().cells, wrapped.row(0).unwrap().cells);
    assert_eq!(
        pending.row(0).unwrap().revision,
        wrapped.row(0).unwrap().revision
    );
    assert!(!Arc::ptr_eq(&pending.rows()[0], &wrapped.rows()[0]));

    // Clearing only WRAPLINE must invalidate the row even though rendered cells
    // and their signature stay unchanged.
    screen.advance(b"\x1b[1;4H\x1b[K");
    let hard = screen.snapshot();
    assert!(!hard.row(0).unwrap().wraps_to_next);
    assert_eq!(hard.row(0).unwrap().cells, wrapped.row(0).unwrap().cells);
    assert_eq!(
        hard.row(0).unwrap().signature,
        wrapped.row(0).unwrap().signature
    );
    assert_eq!(
        hard.row(0).unwrap().revision,
        wrapped.row(0).unwrap().revision
    );
    assert!(!Arc::ptr_eq(&hard.rows()[0], &wrapped.rows()[0]));

    let mut screen = TerminalScreen::new(4, 3);
    screen.advance(b"ab  \r\nX");
    let hard = screen.snapshot();
    assert!(!hard.row(0).unwrap().wraps_to_next);
    assert!(!hard.row(1).unwrap().wrapped);
}

#[test]
fn snapshot_keeps_outgoing_wrap_at_scrolled_window_boundary() {
    let mut screen = TerminalScreen::new(4, 2);
    screen.advance(b"zero\r\nab  X\r\nlast");
    let window = screen.viewport_snapshot(2);
    assert_eq!(window.row(1).unwrap().text, "ab");
    assert!(window.row(1).unwrap().wraps_to_next);
    let full = screen.viewport_snapshot_with_window(0, screen.scrollback_len(), 0);
    assert!(full.row(1).unwrap().wraps_to_next);
    assert!(full.row(2).unwrap().wrapped);
    assert!(Arc::ptr_eq(&window.rows()[1], &full.rows()[1]));
}

#[test]
fn snapshot_recomputes_outgoing_wrap_after_reflow() {
    let mut screen = TerminalScreen::new(4, 3);
    screen.advance(b"abcdef");
    let narrow = screen.snapshot();
    assert!(narrow.row(0).unwrap().wraps_to_next);
    screen.resize(8, 3);
    let wide = screen.snapshot();
    assert_eq!(wide.row(0).unwrap().text, "abcdef");
    assert!(!wide.row(0).unwrap().wraps_to_next);
    screen.resize(4, 3);
    // Shrinking can move the first segment into history to retain the cursor.
    let narrow = screen.viewport_snapshot_with_window(0, screen.scrollback_len(), 0);
    let first = narrow
        .rows()
        .iter()
        .position(|row| row.text == "abcd")
        .unwrap();
    assert!(narrow.row(first).unwrap().wraps_to_next);
    assert!(narrow.row(first + 1).unwrap().wrapped);
}

#[test]
fn reverse_snapshot_preserves_source_colors_and_sgr_reset_semantics() {
    let mut screen = TerminalScreen::new(16, 2);
    screen.advance(b"\x1b[1;4;31;44mA\x1b[7mB\x1b[27mC\x1b[7mD\x1b[0mE");
    let snapshot = screen.snapshot();
    let row = snapshot.row(0).unwrap();
    for (col, reverse) in [false, true, false, true].into_iter().enumerate() {
        let style = row.cells[col].style;
        assert_eq!(style.fg, Some(1));
        assert_eq!(style.bg, Some(4));
        assert_eq!(style.reverse, reverse);
        assert!(style.bold && style.underline);
        assert_eq!(row.styled_spans[col].style, style);
    }
    assert_eq!(row.cells[4].style, super::CellStyle::default());
}

#[test]
fn snapshot_preserves_styled_trailing_blanks_while_trimming_plain_text() {
    for alternate in [false, true] {
        let mut screen = TerminalScreen::new(24, 3);
        if alternate {
            screen.advance(b"\x1b[?1049h");
        }
        screen.advance(b"\x1b[41mTEXT\x1b[44m                \x1b[0m\r\n");
        let snapshot = screen.snapshot();
        let row = snapshot.row(0).unwrap();
        assert_eq!(row.text, "TEXT");
        assert_eq!(row.styled_spans.len(), 2);
        assert_eq!(row.styled_spans[1].text, "                ");
        assert_eq!(row.styled_spans[1].style.bg, Some(4));
        assert!(row.cells[4..20].iter().all(|cell| cell.style.bg == Some(4)));
        // Erasing the row must invalidate both the cell and compressed paint data.
        screen.advance(b"\x1b[1;1H\x1b[2K");
        let cleared = screen.snapshot();
        assert!(
            cleared
                .row(0)
                .unwrap()
                .cells
                .iter()
                .all(|cell| cell.style.bg.is_none())
        );
    }
}

#[test]
fn snapshot_retains_reverse_and_underlined_blank_spans() {
    let mut screen = TerminalScreen::new(16, 2);
    screen.advance(b"X\x1b[7m    \x1b[0;4m    \x1b[0m");
    let snapshot = screen.snapshot();
    let row = snapshot.row(0).unwrap();
    assert_eq!(row.text, "X");
    assert_eq!(row.styled_spans.len(), 3);
    assert!(row.styled_spans[1].style.reverse);
    assert!(row.styled_spans[2].style.underline);
}

#[test]
fn osc133_input_columns_rebuild_snapshot_without_changing_text_revision() {
    let mut screen = TerminalScreen::new(40, 2);
    screen.advance(b"prompt> input");
    let before = screen.snapshot();
    screen.advance(b"\r\x1b[8C\x1b]133;B\x07");
    let active = screen.snapshot();
    let row = active.row(0).unwrap();
    assert_eq!(row.shell_input_columns, Some((8, 40)));
    assert_eq!(row.revision, before.row(0).unwrap().revision);
    assert_eq!(row.signature, before.row(0).unwrap().signature);
    assert!(!Arc::ptr_eq(&before.rows()[0], &active.rows()[0]));
    screen.advance(b"\x1b[5C\x1b]133;C\x07");
    let submitted = screen.snapshot();
    let row = submitted.row(0).unwrap();
    assert_eq!(row.shell_input_columns, Some((8, 13)));
    assert_eq!(row.shell_input, Some(ShellInputLineKind::Submitted));
    assert!(submitted.shell_input_anchor.is_none());
    assert_eq!(row.revision, before.row(0).unwrap().revision);
    assert!(submitted.rows().iter().all(|row| row.shell_integration));
    screen.advance(b"\r\n\x1b]133;A\x07new> \x1b]133;B\x07next");
    let next = screen.snapshot();
    assert_eq!(next.row(0).unwrap().shell_input_columns, Some((8, 13)));
    assert_eq!(next.row(1).unwrap().shell_input_columns, Some((5, 40)));
}

#[test]
fn active_input_highlight_columns_exclude_hard_line_completion_output() {
    let mut screen = TerminalScreen::new(12, 3);
    screen.advance(b"$ \x1b]133;B\x07abcdefghijklmn");
    let wrapped = screen.snapshot();
    assert_eq!(wrapped.row(0).unwrap().shell_input_columns, Some((2, 12)));
    assert_eq!(wrapped.row(1).unwrap().shell_input_columns, Some((0, 12)));
    assert!(wrapped.row(1).unwrap().wrapped);
    screen.advance(b"\r\nAGENTS.md\r\n.bashrc\r\n--More--");
    let page = screen.snapshot();
    for row in page.rows() {
        // The legacy editing-phase marker stays separate from the proven
        // region used for semantic highlighting.
        assert_eq!(row.shell_input, Some(ShellInputLineKind::Active));
        assert!(row.shell_input_columns.is_none());
    }
    screen.advance(b"\r\x1b[2K$ \x1b]133;B\x07ls ");
    let restored = screen.snapshot();
    assert_eq!(restored.row(2).unwrap().shell_input_columns, Some((2, 12)));
    assert!(
        restored.rows()[..2]
            .iter()
            .all(|row| row.shell_input_columns.is_none())
    );
}

fn search_query(pattern: &str) -> TerminalSearchQuery {
    TerminalSearchQuery {
        pattern: pattern.to_string(),
        regex: false,
        case_sensitive: true,
        whole_word: false,
        direction: TerminalSearchDirection::Forward,
        limit: 100,
    }
}

#[test]
fn grid_search_returns_absolute_buffer_rows_without_flattening() {
    let mut screen = TerminalScreen::new(20, 3);
    screen.advance(b"alpha\r\nbeta\r\nneedle-one\r\nneedle-two");

    let matches = screen.search_grid(&search_query("needle")).unwrap();

    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0].line_index, 2);
    assert_eq!(matches[0].start_col, 0);
    assert_eq!(matches[0].end_col, 6);
    assert_eq!(matches[1].line_index, 3);
}

#[test]
fn grid_search_can_limit_work_and_results_to_visible_absolute_rows() {
    let mut screen = TerminalScreen::new(20, 3);
    screen.advance(b"needle-zero\r\nplain\r\nneedle-two\r\nneedle-three");

    let matches = screen
        .search_grid_in_absolute_range(&search_query("needle"), 2..4)
        .unwrap();

    assert_eq!(
        matches
            .iter()
            .map(|m| (m.line_index, m.start_col, m.end_col))
            .collect::<Vec<_>>(),
        vec![(2, 0, 6), (3, 0, 6)]
    );
}

#[test]
fn grid_search_honors_case_and_whole_word_flags() {
    let mut screen = TerminalScreen::new(40, 3);
    screen.advance(b"cat scatter CAT cat_ cat");

    let mut query = search_query("cat");
    query.case_sensitive = false;
    query.whole_word = true;
    let matches = screen.search_grid(&query).unwrap();

    assert_eq!(
        matches
            .iter()
            .map(|m| (m.line_index, m.start_col, m.end_col))
            .collect::<Vec<_>>(),
        vec![(0, 0, 3), (0, 12, 15), (0, 21, 24)]
    );
}

#[test]
fn grid_search_splits_soft_wrapped_match_into_row_segments() {
    let mut screen = TerminalScreen::new(5, 3);
    screen.advance(b"abcde12345");

    let matches = screen.search_grid(&search_query("de12")).unwrap();

    assert_eq!(
        matches
            .iter()
            .map(|m| (m.line_index, m.start_col, m.end_col))
            .collect::<Vec<_>>(),
        vec![(0, 3, 5), (1, 0, 2)]
    );
}

#[test]
fn occurrence_search_groups_soft_wrapped_segments_and_respects_range_mode() {
    let mut screen = TerminalScreen::new(5, 3);
    screen.advance(b"abcde12345");
    let query = search_query("de12");

    let starts = screen
        .search_grid_occurrences_in_absolute_range(
            &query,
            1..2,
            TerminalSearchRangeMode::StartsWithin,
        )
        .unwrap();
    assert!(starts.is_empty());
    let intersects = screen
        .search_grid_occurrences_in_absolute_range(
            &query,
            1..2,
            TerminalSearchRangeMode::Intersects,
        )
        .unwrap();
    assert_eq!(intersects.len(), 1);
    assert_eq!(
        intersects[0]
            .segments
            .iter()
            .map(|segment| (segment.line_index, segment.start_col, segment.end_col,))
            .collect::<Vec<_>>(),
        vec![(0, 3, 5), (1, 0, 2)]
    );
}

#[test]
fn occurrence_search_preserves_regex_and_wide_cell_positions_across_wrap() {
    let mut screen = TerminalScreen::new(5, 3);
    screen.advance("猫abcz".as_bytes());
    let mut query = search_query("猫.*z");
    query.regex = true;

    let matches = screen
        .search_grid_occurrences_in_absolute_range(
            &query,
            0..1,
            TerminalSearchRangeMode::StartsWithin,
        )
        .unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(
        matches[0]
            .segments
            .iter()
            .map(|segment| (segment.line_index, segment.start_col, segment.end_col,))
            .collect::<Vec<_>>(),
        vec![(0, 0, 5), (1, 0, 1)]
    );
}

#[test]
fn occurrence_search_limit_counts_logical_matches() {
    let mut screen = TerminalScreen::new(5, 3);
    screen.advance(b"abcde12345\r\nabcde12345");
    let mut query = search_query("de12");
    query.limit = 1;

    let matches = screen
        .search_grid_occurrences_in_absolute_range(
            &query,
            0..screen.total_rows(),
            TerminalSearchRangeMode::StartsWithin,
        )
        .unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].segments.len(), 2);
}

#[test]
fn occurrence_search_honors_case_whole_word_and_invalid_regex() {
    let mut screen = TerminalScreen::new(40, 3);
    screen.advance(b"cat scatter CAT cat_");
    let mut query = search_query("cat");
    query.case_sensitive = false;
    query.whole_word = true;
    let matches = screen
        .search_grid_occurrences_in_absolute_range(
            &query,
            0..screen.total_rows(),
            TerminalSearchRangeMode::StartsWithin,
        )
        .unwrap();
    assert_eq!(matches.len(), 2);
    query.regex = true;
    query.pattern = "[".to_string();
    assert!(
        screen
            .search_grid_occurrences_in_absolute_range(
                &query,
                0..screen.total_rows(),
                TerminalSearchRangeMode::StartsWithin,
            )
            .is_err()
    );
}

#[test]
fn occurrence_search_follows_reflow_and_scrollback_eviction() {
    let mut screen = TerminalScreen::new(5, 3);
    screen.set_scrollback_limit(3);
    let output = format!("de123\r\n{}needle", "plain\r\n".repeat(10));
    screen.advance(output.as_bytes());
    let query = search_query("de123");
    assert!(
        screen
            .search_grid_occurrences_in_absolute_range(
                &query,
                0..screen.total_rows(),
                TerminalSearchRangeMode::StartsWithin,
            )
            .unwrap()
            .is_empty()
    );

    let query = search_query("needle");
    screen.resize(10, 3);
    let matches = screen
        .search_grid_occurrences_in_absolute_range(
            &query,
            0..screen.total_rows(),
            TerminalSearchRangeMode::StartsWithin,
        )
        .unwrap();
    assert_eq!(matches.len(), 1);
    assert!(matches[0].segments[0].line_index < screen.total_rows());
}

#[test]
fn occurrence_search_uses_current_alternate_screen() {
    let mut screen = TerminalScreen::new(20, 3);
    screen.advance(b"primary");
    let query = search_query("primary");
    screen.advance(b"\x1b[?1049halt-only");
    assert!(
        screen
            .search_grid_occurrences_in_absolute_range(
                &query,
                0..screen.total_rows(),
                TerminalSearchRangeMode::StartsWithin,
            )
            .unwrap()
            .is_empty()
    );
    screen.advance(b"\x1b[?1049l");
    assert_eq!(
        screen
            .search_grid_occurrences_in_absolute_range(
                &query,
                0..screen.total_rows(),
                TerminalSearchRangeMode::StartsWithin,
            )
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn osc7_sets_cwd() {
    let mut screen = TerminalScreen::new(40, 3);
    screen.advance(b"\x1b]7;file://host/home/user/proj\x07");
    assert_eq!(screen.take_cwd().as_deref(), Some("/home/user/proj"));
    assert_eq!(screen.cwd(), Some("/home/user/proj"));
    assert!(screen.take_cwd().is_none());
}

#[test]
fn take_effects_consumes_cwd_edge() {
    let mut screen = TerminalScreen::new(40, 3);
    screen.advance(b"\x1b]7;file://host/home/user/proj\x07");

    let effects = screen.take_effects();
    assert_eq!(effects.cwd.as_deref(), Some("/home/user/proj"));
    assert!(screen.take_effects().cwd.is_none());
    assert!(screen.take_cwd().is_none());
    assert_eq!(screen.cwd(), Some("/home/user/proj"));
}

#[test]
fn osc133_shell_integration_marks() {
    let mut screen = TerminalScreen::new(40, 3);
    assert!(!screen.shell_integration_enabled());
    screen.advance(b"\x1b]133;A\x07");
    assert!(screen.shell_integration_enabled());
    screen.advance(b"\x1b]133;C\x07");
    assert!(screen.command_running());
    let (started, finished) = screen.take_shell_command_edges();
    assert!(started);
    assert!(!finished);
    screen.advance(b"\x1b]133;D;0\x07");
    assert!(!screen.command_running());
    let (started, finished) = screen.take_shell_command_edges();
    assert!(!started);
    assert!(finished);
}

#[test]
fn take_effects_consumes_shell_command_edges() {
    let mut screen = TerminalScreen::new(40, 3);
    screen.advance(b"\x1b]133;C\x07");

    let effects = screen.take_effects();
    assert!(effects.shell_command_started);
    assert!(!effects.shell_command_finished);
    assert_eq!(screen.take_shell_command_edges(), (false, false));

    let effects = screen.take_effects();
    assert!(!effects.shell_command_started);
    assert!(!effects.shell_command_finished);

    screen.advance(b"\x1b]133;D;0\x07");
    let effects = screen.take_effects();
    assert!(!effects.shell_command_started);
    assert!(effects.shell_command_finished);
    assert_eq!(screen.take_shell_command_edges(), (false, false));
}

#[test]
fn command_marks_appear_in_snapshot() {
    let mut screen = TerminalScreen::new(40, 8);
    screen.advance(b"prompt\x1b]133;A\x07");
    screen.advance(b"\x1b]133;C\x07out\n");
    screen.advance(b"\x1b]133;D;0\x07");
    let snap = screen.snapshot();
    let mut marks = snap.rows().iter().map(|row| row.command_mark);
    assert!(
        marks.clone().any(|m| {
            matches!(
                m,
                Some(
                    ShellCommandMark::Prompt
                        | ShellCommandMark::Output
                        | ShellCommandMark::Finished { .. }
                )
            )
        }),
        "marks={:?}",
        snap.rows()
            .iter()
            .map(|row| row.command_mark)
            .collect::<Vec<_>>()
    );
    assert!(
        marks.any(|m| { matches!(m, Some(ShellCommandMark::Finished { exit_code: Some(0) })) }),
        "expected Finished with exit 0, marks={:?}",
        snap.rows()
            .iter()
            .map(|row| row.command_mark)
            .collect::<Vec<_>>()
    );
}

#[test]
fn command_mark_finished_carries_exit_code() {
    let mut screen = TerminalScreen::new(40, 6);
    screen.advance(b"\x1b]133;D;1\x07");
    let snap = screen.snapshot();
    assert!(
        snap.rows()
            .iter()
            .map(|row| row.command_mark)
            .any(|m| { matches!(m, Some(ShellCommandMark::Finished { exit_code: Some(1) })) }),
        "marks={:?}",
        snap.rows()
            .iter()
            .map(|row| row.command_mark)
            .collect::<Vec<_>>()
    );
    screen.advance(b"\x1b]133;D;0\x07");
    let snap = screen.snapshot();
    assert!(
        snap.rows()
            .iter()
            .map(|row| row.command_mark)
            .any(|m| { matches!(m, Some(ShellCommandMark::Finished { exit_code: Some(0) })) }),
        "marks={:?}",
        snap.rows()
            .iter()
            .map(|row| row.command_mark)
            .collect::<Vec<_>>()
    );
}

#[test]
fn shell_input_stripes_split_at_osc_boundaries_before_following_output() {
    let mut screen = TerminalScreen::new(40, 5);
    screen.advance(b"$ \x1b]133;B\x07echo hi\r\n\x1b]133;C\x07output line 1\r\noutput line 2");
    let snapshot = screen.snapshot();
    let input_rows = snapshot
        .rows()
        .iter()
        .filter(|row| row.shell_input == Some(ShellInputLineKind::Submitted))
        .collect::<Vec<_>>();
    assert_eq!(input_rows.len(), 1, "rows={:?}", snapshot.rows());
    assert!(input_rows[0].text.contains("echo hi"));
    assert!(
        snapshot
            .rows()
            .iter()
            .filter(|row| row.text.contains("output"))
            .all(|row| row.shell_input.is_none())
    );
}

#[test]
fn shell_input_stripes_support_cross_chunk_osc_and_active_multiline_input() {
    let mut screen = TerminalScreen::new(12, 5);
    screen.advance(b"prompt ]133;B");
    screen.advance(b"\x07one two three\r\nfour");
    let active = screen
        .snapshot()
        .rows()
        .iter()
        .filter(|row| row.shell_input == Some(ShellInputLineKind::Active))
        .count();
    assert!(active >= 2, "expected multiline active input");
    screen.advance(b"\x1b]133;C\x07result");
    let snapshot = screen.snapshot();
    assert!(
        snapshot
            .rows()
            .iter()
            .any(|row| row.shell_input == Some(ShellInputLineKind::Submitted))
    );
    assert!(
        snapshot
            .rows()
            .iter()
            .filter(|row| row.text.contains("result"))
            .all(|row| row.shell_input != Some(ShellInputLineKind::Active))
    );
}

#[test]
fn shell_input_c_at_next_row_zero_excludes_output_row_and_633_is_compatible() {
    let mut screen = TerminalScreen::new(30, 4);
    screen.advance(b"prompt ]633;B\x07command\r\n\x1b]633;C\x07output");
    let snapshot = screen.snapshot();
    assert!(
        snapshot
            .rows()
            .iter()
            .any(|row| row.text.contains("command")
                && row.shell_input == Some(ShellInputLineKind::Submitted))
    );
    assert!(
        snapshot
            .rows()
            .iter()
            .filter(|row| row.text.contains("output"))
            .all(|row| row.shell_input.is_none())
    );
}

#[test]
fn shell_input_ids_change_after_reset_and_alternate_screen_does_not_mark_rows() {
    let mut screen = TerminalScreen::new(20, 3);
    screen.advance(b"p ]133;B\x07input");
    let before = screen
        .snapshot()
        .rows()
        .iter()
        .find(|row| row.text.contains("input"))
        .and_then(|row| row.line_id);
    assert!(before.is_some());
    screen.advance(b"\x1b[?1049h\x1b]133;B\x07alternate\x1b[?1049l\x1b[2J");
    assert!(
        screen
            .snapshot()
            .rows()
            .iter()
            .filter(|row| row.text.contains("alternate"))
            .all(|row| row.shell_input.is_none())
    );
    screen.clear();
    let after = screen.snapshot().rows().iter().find_map(|row| row.line_id);
    assert!(after.is_none() || after != before);
}

#[test]
fn shell_input_commit_is_bounded_to_retained_scrollback() {
    let mut screen = TerminalScreen::new(12, 3);
    screen.set_scrollback_limit(5);
    screen.advance(b"prompt \x1b]133;B\x07input");
    for line in 0..100 {
        screen.advance(format!("\r\nline-{line}").as_bytes());
    }
    screen.advance(b"\x1b]133;C\x07");

    assert!(screen.last_shell_input_commit_count <= screen.total_rows());
    assert!(screen.primary_lines.metadata.len() <= screen.total_rows());
}

fn submitted_shell_commands() -> TerminalScreen {
    let mut screen = TerminalScreen::new(24, 8);
    for command in ["echo 1", "echo 2", "echo 3", "echo 4"] {
        screen.advance(
            format!("\x1b]133;A\x07$ \x1b]133;B\x07{command}\r\n\x1b]133;C\x07\x1b]133;D;0\x07")
                .as_bytes(),
        );
    }
    screen
}

#[test]
fn ansi_erase_removes_submitted_shell_input_from_erased_rows() {
    for (erase, erased, retained) in [
        ("\x1b[H\x1b[J", vec![0, 1, 2, 3], vec![]),
        ("\x1b[2;1H\x1b[0J", vec![1, 2, 3], vec![0]),
        ("\x1b[3;24H\x1b[1J", vec![0, 1, 2], vec![3]),
        ("\x1b[2;4H\x1b[2K", vec![1], vec![0, 2, 3]),
        ("\x1b[2;1H\x1b[0K", vec![1], vec![0, 2, 3]),
        ("\x1b[2;24H\x1b[1K", vec![1], vec![0, 2, 3]),
    ] {
        let mut screen = submitted_shell_commands();
        // Keep the old snapshot alive to exercise row-cache invalidation.
        let before = screen.snapshot();
        for row in &before.rows()[..4] {
            assert_eq!(row.shell_input, Some(ShellInputLineKind::Submitted));
        }
        screen.advance(erase.as_bytes());
        let after = screen.snapshot();
        for index in erased {
            let row = after.row(index).unwrap();
            assert!(row.text.is_empty(), "{erase:?}, row {index}");
            assert_eq!(row.shell_input, None, "{erase:?}, row {index}");
            assert_eq!(row.shell_input_columns, None);
            assert_eq!(row.line_id, before.row(index).unwrap().line_id);
            assert!(!Arc::ptr_eq(&after.rows()[index], &before.rows()[index]));
        }
        for index in retained {
            assert_eq!(after.row(index), before.row(index), "{erase:?}");
        }
    }
}

#[test]
fn ansi_partial_erase_checks_input_columns_instead_of_prompt_or_output() {
    for osc in [133, 633] {
        for (erase, input_remains) in [
            ("\x1b[1;6H\x1b[0K", true),
            ("\x1b[1;5H\x1b[1K", true),
            ("\x1b[1;3H\x1b[0K", false), // Only the prompt remains.
            ("\x1b[1;8H\x1b[1K", false), // Only command output remains.
            ("\x1b[1;6H\x1b[0J", true),
            ("\x1b[1;5H\x1b[1J", true),
            ("\x1b[1;3H\x1b[0J", false),
            ("\x1b[1;8H\x1b[1J", false),
        ] {
            let mut screen = TerminalScreen::new(24, 3);
            screen.advance(format!("$ \x1b]{osc};B\x07abcdef\x1b]{osc};C\x07OUTPUT").as_bytes());
            let before = screen.snapshot();
            assert_eq!(before.row(0).unwrap().shell_input_columns, Some((2, 8)));
            screen.advance(erase.as_bytes());
            let after = screen.snapshot();
            let row = after.row(0).unwrap();
            assert!(!row.text.is_empty());
            assert_eq!(
                row.shell_input,
                input_remains.then_some(ShellInputLineKind::Submitted),
                "OSC {osc}, {erase:?}"
            );
            assert_eq!(row.line_id, before.row(0).unwrap().line_id);
        }
    }
}

#[test]
fn ansi_erase_keeps_current_active_input_even_when_empty() {
    for osc in [133, 633] {
        for erase in ["\x1b[0K", "\x1b[1K", "\x1b[2K", "\x1b[0J", "\x1b[1J"] {
            let mut screen = submitted_shell_commands();
            screen.advance(format!("$ \x1b]{osc};B\x07draft").as_bytes());
            let before = screen.snapshot();
            screen.advance(format!("\r{erase}").as_bytes());
            let after = screen.snapshot();
            assert_eq!(
                after.row(4).unwrap().shell_input,
                Some(ShellInputLineKind::Active)
            );
            assert_eq!(after.shell_input_anchor, before.shell_input_anchor);
        }
    }
}

#[test]
fn ansi_full_screen_erase_does_not_restore_submitted_input() {
    for erase in ["\x1b[2J", "\x1b[2J\x1b[3J"] {
        let mut screen = submitted_shell_commands();
        screen.advance(erase.as_bytes());
        let snapshot = screen.snapshot();
        assert!(snapshot.rows().iter().all(|row| row.shell_input.is_none()));
        screen.advance(b"\x1b[?1049hfull screen\x1b[?1049l");
        assert!(
            screen
                .snapshot()
                .rows()
                .iter()
                .all(|row| row.shell_input.is_none())
        );
    }
}

#[test]
fn ansi_erase_handles_styled_blanks_wide_input_and_split_sequences() {
    for input in ["abcdef", "中文e\u{301}", " \u{301}"] {
        for chunk_size in [1, 2, 64] {
            let mut screen = TerminalScreen::new(24, 3);
            screen.advance(format!("$ \x1b]133;B\x07{input}\x1b]133;C\x07").as_bytes());
            assert_eq!(
                screen.snapshot().row(0).unwrap().shell_input,
                Some(ShellInputLineKind::Submitted)
            );
            for chunk in b"\x1b[1;3H\x1b[44m\x1b[0K".chunks(chunk_size) {
                screen.advance(chunk);
            }
            let snapshot = screen.snapshot();
            let row = snapshot.row(0).unwrap();
            assert_eq!(row.text, "$");
            assert_eq!(row.shell_input, None);
            assert!(row.cells[2..].iter().all(|cell| cell.style.bg == Some(4)));
        }
    }
}

#[test]
fn ansi_erase_without_shell_integration_does_not_classify_rows() {
    let mut screen = TerminalScreen::new(24, 3);
    screen.advance(b"plain\r\nterminal\x1b[H\x1b[J");
    assert!(
        screen
            .snapshot()
            .rows()
            .iter()
            .all(|row| row.shell_input.is_none() && !row.shell_integration)
    );
}

#[test]
fn alternate_screen_restores_active_primary_input_without_inheriting_it() {
    let mut screen = submitted_shell_commands();
    screen.advance(b"$ \x1b]133;B\x07draft");
    let before = screen.snapshot();
    screen.advance(b"\x1b[?1049h\x1b[H\x1b[2Jalternate");
    assert!(screen.alternate_screen());
    assert!(
        screen
            .snapshot()
            .rows()
            .iter()
            .all(|row| row.shell_input.is_none())
    );
    screen.advance(b"\x1b[?1049l");
    let after = screen.snapshot();
    for index in 0..5 {
        assert_eq!(
            after.row(index).unwrap().shell_input,
            before.row(index).unwrap().shell_input
        );
        assert_eq!(
            after.row(index).unwrap().text,
            before.row(index).unwrap().text
        );
        assert_eq!(
            after.row(index).unwrap().line_id,
            before.row(index).unwrap().line_id
        );
    }
    assert_eq!(
        after.row(4).unwrap().shell_input,
        Some(ShellInputLineKind::Active)
    );
}

#[test]
fn ansi_erase_and_alternate_screen_preserve_only_valid_primary_input() {
    for erase in ["", "\x1b[2;1H\x1b[2K"] {
        for single_chunk in [false, true] {
            let mut screen = submitted_shell_commands();
            let before = screen.snapshot();
            let enter = "\x1b[?1049h\x1b[H\x1b[2J\x1b]133;B\x07alternate";
            let leave = "\x1b[?1049l";
            // Clear a primary command before the switch, including in a single read.
            if single_chunk {
                screen.advance(format!("{erase}{enter}{leave}").as_bytes());
            } else {
                screen.advance(erase.as_bytes());
                screen.advance(enter.as_bytes());
                let alternate = screen.snapshot();
                assert!(alternate.rows().iter().all(|row| row.shell_input.is_none()));
                screen.advance(leave.as_bytes());
            }
            let after = screen.snapshot();
            assert_eq!(
                after.row(1).unwrap().shell_input,
                erase.is_empty().then_some(ShellInputLineKind::Submitted)
            );
            for index in [0, 2, 3] {
                assert_eq!(after.row(index), before.row(index));
            }
        }
    }
}

#[test]
fn ansi_erase_preserves_wrapped_multiline_history_through_scroll_and_resize() {
    let mut screen = TerminalScreen::new(12, 6);
    screen.advance(b"$ \x1b]133;B\x07abcdefghijklmn\r\nsecond\r\n\x1b]133;C\x07");
    let before = screen.snapshot();
    for row in &before.rows()[..3] {
        assert_eq!(row.shell_input, Some(ShellInputLineKind::Submitted));
    }
    screen.advance(b"\x1b[2;1H\x1b[2K\x1b[6;1H\r\noutput\r\nmore");
    let history = screen.viewport_snapshot_with_window(0, screen.scrollback_len(), 0);
    assert_eq!(
        history.row(0).unwrap().shell_input,
        Some(ShellInputLineKind::Submitted)
    );
    assert_eq!(history.row(1).unwrap().shell_input, None);
    assert_eq!(
        history.row(2).unwrap().shell_input,
        Some(ShellInputLineKind::Submitted)
    );
    for index in 0..3 {
        assert_eq!(
            history.row(index).unwrap().line_id,
            before.row(index).unwrap().line_id
        );
    }
    // Width reflow deliberately invalidates uncertain semantic coordinates.
    screen.resize(8, 8);
    let reflowed = screen.viewport_snapshot_with_window(0, screen.scrollback_len(), 0);
    assert!(reflowed.rows().iter().all(|row| row.shell_input.is_none()));
}

#[test]
fn active_shell_input_range_is_clamped_or_closed_with_retained_rows() {
    let mut partially_retained = ScreenLineState {
        logical_origin: 10,
        active_input_start: Some(5),
        active_input_end: Some(12),
        ..ScreenLineState::default()
    };
    partially_retained.retain_physical_range(
        alacritty_terminal::index::Line(-2),
        alacritty_terminal::index::Line(2),
    );
    assert_eq!(partially_retained.active_input_start, Some(8));
    assert_eq!(partially_retained.active_input_end, Some(12));

    let mut evicted = ScreenLineState {
        logical_origin: 10,
        active_input_start: Some(1),
        active_input_end: Some(7),
        ..ScreenLineState::default()
    };
    evicted.retain_physical_range(
        alacritty_terminal::index::Line(-2),
        alacritty_terminal::index::Line(2),
    );
    assert_eq!(evicted.active_input_start, None);
    assert_eq!(evicted.active_input_end, None);

    let mut malformed = ScreenLineState {
        logical_origin: 10,
        active_input_start: Some(12),
        active_input_end: Some(8),
        ..ScreenLineState::default()
    };
    malformed.retain_physical_range(
        alacritty_terminal::index::Line(-2),
        alacritty_terminal::index::Line(2),
    );
    assert_eq!(malformed.active_input_start, None);
    assert_eq!(malformed.active_input_end, None);
}

#[test]
fn width_reflow_closes_active_shell_input_and_invalidates_line_ids() {
    let mut screen = TerminalScreen::new(12, 3);
    screen.advance(b"prompt \x1b]133;B\x07one two three");
    let before_ids = screen
        .snapshot()
        .rows()
        .iter()
        .filter_map(|row| row.line_id)
        .collect::<Vec<_>>();

    screen.resize(6, 4);
    let reflowed = screen.snapshot();

    assert!(reflowed.rows().iter().all(|row| row.shell_input.is_none()));
    assert!(
        reflowed
            .rows()
            .iter()
            .filter_map(|row| row.line_id)
            .all(|line_id| !before_ids.contains(&line_id))
    );
}

#[test]
fn osc8_hyperlink_spans() {
    let mut screen = TerminalScreen::new(40, 3);
    screen.advance(b"\x1b]8;;https://example.com\x07click\x1b]8;;\x07 plain");
    let snap = screen.viewport_snapshot(0);
    let spans = &snap.row(0).expect("first row").hyperlinks;
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].uri, "https://example.com");
    assert_eq!(spans[0].start_col, 0);
    assert_eq!(spans[0].end_col, 4);
    let first_link = snap
        .cell(0, 0)
        .and_then(|cell| cell.hyperlink.as_ref())
        .expect("first link cell");
    let second_link = snap
        .cell(0, 1)
        .and_then(|cell| cell.hyperlink.as_ref())
        .expect("second link cell");
    assert!(Arc::ptr_eq(first_link, second_link));
}

#[test]
fn ubuntu_motd_links_survive_resize_without_row_overwrite() {
    let mut screen = TerminalScreen::new(80, 24);
    screen.advance(
        concat!(
            "Welcome to Ubuntu 24.04.4 LTS (GNU/Linux 6.8.0-107-generic x86_64)\r\n",
            "\r\n",
            " * Documentation:  \x1b]8;;https://help.ubuntu.com\x07https://help.ubuntu.com\x1b]8;;\x07\r\n",
            " * Management:     \x1b]8;;https://landscape.canonical.com\x07https://landscape.canonical.com\x1b]8;;\x07\r\n",
            " * Support:        \x1b]8;;https://ubuntu.com/pro\x07https://ubuntu.com/pro\x1b]8;;\x07\r\n",
            "\r\n",
            "System information as of Sat Jul 25 12:54:01 PM CST 2026\r\n",
            "\r\n",
            "  System load:           4.86\r\n",
            "  Usage of /:            18.3% of 106.92GB\r\n",
            "  Memory usage:          65%\r\n",
            "  Swap usage:            100%\r\n",
            "  Temperature:           45.0 C\r\n",
            "  Processes:             500\r\n",
            "  Users logged in:       0\r\n",
        )
        .as_bytes(),
    );
    screen.resize(120, 40);

    let lines = screen.all_lines();
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Management:     https://landscape.canonical.com")),
        "lines={lines:#?}"
    );
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.contains("Users logged in:"))
            .count(),
        1,
        "lines={lines:#?}"
    );
}

#[test]
fn osc_sets_window_title() {
    let mut screen = TerminalScreen::new(20, 5);
    screen.advance(b"\x1b]2;hello-host\x07");
    assert_eq!(screen.take_window_title().as_deref(), Some("hello-host"));
    assert_eq!(screen.window_title(), Some("hello-host"));
    assert!(screen.take_window_title().is_none());
}

#[test]
fn visual_bell_on_bel() {
    let mut screen = TerminalScreen::new(20, 5);
    assert!(!screen.take_visual_bell());
    screen.advance(b"hi\x07");
    assert!(screen.take_visual_bell());
    assert!(!screen.take_visual_bell());
}

#[test]
fn device_status_query_emits_pty_write_response() {
    let mut screen = TerminalScreen::new(20, 5);
    screen.advance(b"\x1b[5n");
    let effects = screen.take_effects();
    assert_eq!(effects.pty_write, vec![b"\x1b[0n".to_vec()]);
}

#[test]
fn prints_and_wraps_lines() {
    let mut screen = TerminalScreen::new(5, 3);
    screen.advance(b"hello\nworld");
    assert_eq!(screen.lines()[0], "hello");
    assert!(screen.lines().iter().any(|line| line.contains("world")));
}

#[test]
fn snapshots_mark_wrapped_continuation_rows() {
    let mut screen = TerminalScreen::new(5, 3);
    screen.advance(b"abcdef");
    let snapshot = screen.viewport_snapshot(0);

    assert_eq!(snapshot.row(0).map(|row| row.wrapped), Some(false));
    assert_eq!(snapshot.row(1).map(|row| row.wrapped), Some(true));
}

#[test]
fn line_numbers_count_hard_lines_instead_of_soft_wrapped_rows() {
    for chunk_size in [1, 7, 128] {
        let mut screen = TerminalScreen::new(4, 16);
        let output = b"1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8\r\n9\r\n10\r\nabcdefghijkl\r\nnext";
        for chunk in output.chunks(chunk_size) {
            screen.advance(chunk);
        }
        let snapshot = screen.snapshot();
        for (row, number) in [(10, 11), (11, 11), (12, 11), (13, 12)] {
            assert_eq!(snapshot.rows()[row].display_line_number, Some(number));
        }
        assert!(!snapshot.rows()[10].wrapped);
        assert!(snapshot.rows()[11].wrapped);
        assert!(snapshot.rows()[12].wrapped);
        assert!(!snapshot.rows()[13].wrapped);
    }
}

#[test]
fn logical_line_numbers_agree_across_scrolled_and_prefetched_snapshots() {
    let mut screen = TerminalScreen::new(4, 3);
    screen.advance(b"abcdefghijkl\r\nnext\r\nlast");
    assert_eq!(screen.snapshot().rows()[1].display_line_number, Some(2));
    let older = screen.viewport_snapshot(2);
    assert_eq!(older.rows()[0].display_line_number, Some(1));
    assert_eq!(older.rows()[1].display_line_number, Some(1));
    let window = screen.viewport_snapshot_with_window(0, 2, 0);
    assert_eq!(window.rows()[3].display_line_number, Some(2));
}

#[test]
fn logical_line_numbers_reflow_with_width_and_rebase_after_local_clear() {
    let mut screen = TerminalScreen::new(4, 6);
    screen.advance(b"abcdefghijkl\r\nnext");
    for cols in [6, 3, 12, 4] {
        screen.resize(cols, 6);
        let snapshot = screen.snapshot();
        let row = snapshot
            .rows()
            .iter()
            .find(|row| row.text.starts_with("nex"))
            .unwrap();
        assert_eq!(row.display_line_number, Some(2));
    }
    screen.clear_except_input();
    screen.advance(b"\r\nabcdefghijkl\r\nlast");
    assert_eq!(
        screen.snapshot().rows().last().unwrap().display_line_number,
        Some(3)
    );
    for cols in [6, 3, 12, 4] {
        screen.resize(cols, 6);
        let snapshot = screen.snapshot();
        let row = snapshot
            .rows()
            .iter()
            .find(|row| row.text.starts_with("las"))
            .unwrap();
        assert_eq!(
            row.display_line_number,
            Some(3),
            "cols {cols}: {:?}",
            snapshot
                .rows()
                .iter()
                .map(|row| (&row.text, row.display_line_number, row.cleared, row.wrapped))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn logical_line_numbers_keep_local_clear_origin_when_wrapped_history_is_pruned() {
    let mut screen = TerminalScreen::new(4, 2);
    screen.set_scrollback_limit(2);
    screen.clear_except_input();
    for _ in 0..10 {
        screen.advance(b"abcdefghijkl\r\n");
    }
    assert_eq!(screen.snapshot().rows()[1].display_line_number, Some(11));
}

#[test]
fn logical_line_numbers_rebuild_primary_history_after_resizing_an_alternate_screen() {
    let mut screen = TerminalScreen::new(4, 3);
    screen.advance(b"abcdefghijkl\r\nnext\r\nlast");
    screen.advance(b"\x1b[?1049halternate");
    screen.resize(3, 3);
    screen.advance(b"\x1b[?1049l");
    let snapshot = screen.viewport_snapshot_with_window(0, 8, 0);
    let next = snapshot
        .rows()
        .iter()
        .find(|row| row.text == "nex")
        .unwrap();
    assert_eq!(next.display_line_number, Some(2));
    screen.advance(b"\x1bcabc\r\ndef");
    assert_eq!(screen.snapshot().rows()[1].display_line_number, Some(2));
}

#[test]
fn erasing_a_wrap_updates_following_line_numbers_and_invalidates_cached_rows() {
    let mut screen = TerminalScreen::new(4, 4);
    screen.advance(b"abcdefgh\r\nnext");
    let before = screen.snapshot();
    assert_eq!(before.rows()[2].display_line_number, Some(2));
    screen.advance(b"\x1b[H\x1b[2K");
    let after = screen.snapshot();
    assert!(!after.rows()[1].wrapped);
    assert_eq!(after.rows()[2].display_line_number, Some(3));
    assert!(!Arc::ptr_eq(&before.rows()[2], &after.rows()[2]));
}

#[test]
fn all_text_rows_preserve_wraps_across_scrollback_boundary() {
    let mut screen = TerminalScreen::new(5, 2);
    screen.advance(b"abcdef\r\nghij");

    let rows = screen.all_text_rows();
    assert_eq!(rows[0], ("abcde".to_string(), false));
    assert_eq!(rows[1], ("f".to_string(), true));
    assert_eq!(rows[2], ("ghij".to_string(), false));
}

#[test]
fn all_text_joins_soft_wraps_across_scrollback_and_keeps_hard_breaks() {
    let mut screen = TerminalScreen::new(5, 2);
    screen.advance("ab好ef\r\nghij\r\n\r\n".as_bytes());
    assert_eq!(screen.all_text().as_deref(), Some("ab好ef\nghij"));
}

#[test]
fn all_text_is_absent_for_a_blank_buffer() {
    let screen = TerminalScreen::new(5, 2);
    assert_eq!(screen.all_text(), None);
}

#[test]
fn all_text_rows_match_snapshot_text_for_wide_and_combining_cells() {
    let mut screen = TerminalScreen::new(5, 2);
    screen.advance("ab好e\u{301}f\r\ng".as_bytes());

    let rows = screen.all_text_rows();
    let snapshot = screen.viewport_snapshot_with_window(0, screen.scrollback_len(), 0);
    assert_eq!(rows.len(), snapshot.row_count());
    for (text_row, snapshot_row) in rows.iter().zip(snapshot.rows()) {
        assert_eq!(&text_row.0, &snapshot_row.text);
        assert_eq!(text_row.1, snapshot_row.wrapped);
    }
}

#[test]
fn changed_visible_lines_receive_timestamps() {
    let mut screen = TerminalScreen::new(20, 3);
    screen.advance(b"alpha\nbeta");
    let snap = screen.viewport_snapshot(0);

    assert!(
        snap.rows()
            .iter()
            .any(|row| row.timestamp_ms.is_some() && row.text.contains("alpha"))
    );
    assert!(
        snap.rows()
            .iter()
            .any(|row| row.timestamp_ms.is_some() && row.text.contains("beta"))
    );
}

#[test]
fn snapshot_includes_row_signatures() {
    let mut screen = TerminalScreen::new(20, 3);
    screen.advance(b"alpha\nbeta");
    let snap = screen.viewport_snapshot(0);

    assert!(snap.rows().iter().any(|row| row.signature != 0));
    assert!(snap.rows().iter().any(|row| row.revision != 0));
    for row in snap.rows() {
        assert_eq!(row.signature, render_row_signature(&row.cells));
    }
}

#[test]
fn consecutive_snapshots_share_unchanged_rows() {
    let mut screen = TerminalScreen::new(20, 4);
    screen.advance(b"alpha\r\nbeta");

    let (first, first_stats) = screen.viewport_snapshot_with_stats(0);
    let (second, second_stats) = screen.viewport_snapshot_with_stats(0);

    assert_eq!(first.row_count(), second.row_count());
    assert_eq!(
        first_stats.reused_rows + first_stats.rebuilt_rows,
        first.row_count()
    );
    assert_eq!(second_stats.reused_rows, second.row_count());
    assert_eq!(second_stats.rebuilt_rows, 0);
    assert!(
        first
            .rows()
            .iter()
            .zip(second.rows())
            .all(|(left, right)| Arc::ptr_eq(left, right) && left.revision == right.revision)
    );
}

#[test]
fn single_line_input_rebuilds_only_damaged_snapshot_row() {
    let mut screen = TerminalScreen::new(20, 4);
    screen.advance(b"alpha");
    let first = screen.viewport_snapshot(0);

    screen.advance(b"x");
    let (second, stats) = screen.viewport_snapshot_with_stats(0);
    let shared = first
        .rows()
        .iter()
        .zip(second.rows())
        .filter(|(left, right)| Arc::ptr_eq(left, right))
        .count();

    assert_eq!(stats.rebuilt_rows, 1);
    assert!(shared >= second.row_count().saturating_sub(1));
    assert!(
        first
            .rows()
            .iter()
            .zip(second.rows())
            .any(|(left, right)| left.revision != right.revision)
    );
}

#[test]
fn adjacent_snapshot_windows_share_overlapping_rows() {
    let mut screen = TerminalScreen::new(20, 4);
    for line in 0..20 {
        screen.advance(format!("line-{line:02}\r\n").as_bytes());
    }
    let window = screen.viewport_snapshot_with_window(4, 4, 4);
    let viewport = screen.viewport_snapshot(4);

    assert!(
        window.rows()[4..4 + viewport.row_count()]
            .iter()
            .zip(viewport.rows())
            .all(|(left, right)| Arc::ptr_eq(left, right))
    );
}

#[test]
fn snapshot_row_cache_uses_revision_as_authoritative_invalidation() {
    let mut screen = TerminalScreen::new(8, 1);
    screen.advance(b"alpha");
    let first = screen.viewport_snapshot(0);
    let original = first.rows()[0].clone();
    let mut conflicting = (*original).clone();
    conflicting.cells[0].text = Arc::from("z");
    conflicting.text = "zlpha".to_string();
    let conflicting = Arc::new(conflicting);
    let key = TerminalSnapshotRowCacheKey {
        cleared: original.cleared,
        display_line_number: original.display_line_number,
        cols: screen.cols(),
        revision: original.revision,
        signature: original.signature,
        timestamp_ms: original.timestamp_ms,
        wrapped: original.wrapped,
        wraps_to_next: original.wraps_to_next,
        command_mark: original.command_mark,
        line_id: original.line_id,
        shell_input: original.shell_input,
        shell_input_columns: original.shell_input_columns,
        shell_integration: original.shell_integration,
    };
    screen.snapshot_row_cache.lock().unwrap().entries.insert(
        key,
        TerminalSnapshotRowCacheEntry {
            row: Arc::downgrade(&conflicting),
            last_used: 0,
        },
    );
    screen.advance(b"!");

    let (next, stats) = screen.viewport_snapshot_with_stats(0);

    assert_eq!(next.row(0).map(|row| row.text.as_str()), Some("alpha!"));
    assert!(!Arc::ptr_eq(&next.rows()[0], &conflicting));
    assert_eq!(stats.rebuilt_rows, 1);
    assert_ne!(next.rows()[0].revision, original.revision);
}

#[test]
fn snapshot_row_cache_prunes_to_limit() {
    let mut cache = TerminalSnapshotRowCache::default();
    for signature in 0..=TERMINAL_SNAPSHOT_ROW_CACHE_LIMIT as u64 {
        cache.entries.insert(
            TerminalSnapshotRowCacheKey {
                cleared: false,
                display_line_number: None,
                cols: 1,
                revision: signature,
                signature,
                timestamp_ms: None,
                wrapped: false,
                wraps_to_next: false,
                command_mark: None,
                line_id: None,
                shell_input: None,
                shell_input_columns: None,
                shell_integration: false,
            },
            TerminalSnapshotRowCacheEntry {
                row: Weak::new(),
                last_used: signature,
            },
        );
    }

    cache.prune();

    assert!(cache.entries.len() <= TERMINAL_SNAPSHOT_ROW_CACHE_LIMIT);
}

#[test]
fn snapshot_keeps_blank_cell_storage_allocation_free() {
    let screen = TerminalScreen::new(80, 24);
    let snapshot = screen.viewport_snapshot(0);

    assert!(
        snapshot
            .rows()
            .iter()
            .flat_map(|row| row.cells.iter())
            .all(|cell| cell.text.is_empty())
    );
    let first_empty_text = snapshot.cell(0, 0).expect("first blank cell").text.clone();
    assert!(
        snapshot
            .rows()
            .iter()
            .flat_map(|row| row.cells.iter())
            .all(|cell| Arc::ptr_eq(&cell.text, &first_empty_text))
    );
    assert!(snapshot.rows().iter().all(|row| row.text.is_empty()));
    assert!(snapshot.rows().iter().all(|row| {
        row.styled_spans
            .iter()
            .map(|span| span.text.as_str())
            .collect::<String>()
            == " ".repeat(snapshot.cols)
    }));
}

#[test]
fn snapshot_blank_cell_storage_preserves_wide_text_and_signatures() {
    let mut screen = TerminalScreen::new(8, 2);
    screen.advance("界 a".as_bytes());
    let snapshot = screen.viewport_snapshot(0);

    assert_eq!(snapshot.line(0), Some("界 a"));
    assert!(snapshot.cell(0, 1).is_some_and(|cell| cell.text.is_empty()));
    for row in snapshot.rows() {
        assert_eq!(row.signature, render_row_signature(&row.cells));
    }
}

#[test]
fn row_signatures_change_with_content_and_style() {
    let mut screen = TerminalScreen::new(20, 2);
    let initial = screen.viewport_snapshot(0).row(0).unwrap().signature;

    screen.advance(b"alpha");
    let text_signature = screen.viewport_snapshot(0).row(0).unwrap().signature;
    assert_ne!(text_signature, initial);

    screen.clear();
    screen.advance(b"\x1b[31malpha");
    let styled_signature = screen.viewport_snapshot(0).row(0).unwrap().signature;
    assert_ne!(styled_signature, text_signature);
}

#[test]
fn consecutive_input_scans_only_alacritty_damaged_lines() {
    let mut screen = TerminalScreen::new(80, 120);
    screen.advance(b"a");

    // Reapplying the worker's unchanged output configuration must not turn
    // the next single-cell update into full terminal damage.
    screen.set_scrollback_limit(5_000);
    screen.advance(b"b");

    assert!(screen.last_signature_scan_count > 0);
    assert!(
        screen.last_signature_scan_count < screen.rows(),
        "single-line input scanned {} of {} rows",
        screen.last_signature_scan_count,
        screen.rows()
    );
}

#[test]
fn scrolled_lines_keep_timestamps_in_history_viewport() {
    let mut screen = TerminalScreen::new(20, 2);
    screen.advance(b"one\r\ntwo\r\nthree");
    assert!(screen.scrollback_len() > 0);

    let snap = screen.viewport_snapshot(1);
    assert!(
        snap.rows()
            .iter()
            .any(|row| row.timestamp_ms.is_some() && row.text.contains("one"))
    );
}

#[test]
fn output_burst_keeps_timestamps_above_the_final_screen() {
    for chunk_size in [1, 37, 4096] {
        let mut screen = TerminalScreen::new(40, 5);
        let output = (0..100)
            .map(|line| format!("line-{line:03}\r\n"))
            .collect::<String>();
        for chunk in output.as_bytes().chunks(chunk_size) {
            screen.advance(chunk);
        }
        let snapshot = screen.viewport_snapshot_with_window(0, screen.scrollback_len(), 0);
        assert_eq!(
            snapshot
                .rows()
                .iter()
                .filter(|row| !row.text.is_empty())
                .count(),
            100
        );
        for row in snapshot.rows().iter().filter(|row| !row.text.is_empty()) {
            assert!(
                row.timestamp_ms.is_some(),
                "missing timestamp for {} with chunk size {chunk_size}",
                row.text
            );
        }
    }
}

#[test]
fn long_output_keeps_history_timestamps_after_width_change() {
    let mut screen = TerminalScreen::new(40, 5);
    let output = (0..100)
        .map(|line| format!("line-{line:03}\r\n"))
        .collect::<String>();
    screen.advance(output.as_bytes());
    let before = screen.viewport_snapshot_with_window(0, screen.scrollback_len(), 0);

    screen.resize(30, 5);
    let after = screen.viewport_snapshot_with_window(0, screen.scrollback_len(), 0);
    for (old, new) in before.rows().iter().zip(after.rows()) {
        assert_eq!(new.text, old.text);
        assert_eq!(
            new.timestamp_ms, old.timestamp_ms,
            "timestamp changed for {}",
            old.text
        );
    }
}

#[test]
fn width_reflow_preserves_wrapped_output_times_and_wide_characters() {
    let mut screen = TerminalScreen::new(12, 3);
    screen.advance("abcdef   中文e\u{301}ghijklmnopqrstuvwxyz\r\nnext\r\n".as_bytes());
    for metadata in screen.primary_lines.metadata.values_mut() {
        metadata.timestamp_ms = Some(123);
    }
    for cols in [7, 18, 5, 12] {
        screen.resize(cols, 3);
        let snapshot = screen.viewport_snapshot_with_window(0, screen.scrollback_len(), 0);
        for row in snapshot.rows().iter().filter(|row| !row.text.is_empty()) {
            assert_eq!(
                row.timestamp_ms,
                Some(123),
                "timestamp changed for {} at width {cols}",
                row.text
            );
        }
    }
}

#[test]
fn width_reflow_keeps_distinct_times_for_repeated_lines_after_history_eviction() {
    let mut screen = TerminalScreen::new(12, 3);
    screen.set_scrollback_limit(5);
    screen.advance(b"repeated-line\r\nrepeated-line\r\nrepeated-line\r\n");
    for (index, metadata) in screen.primary_lines.metadata.values_mut().enumerate() {
        metadata.timestamp_ms = Some(100 + index as u64);
    }
    let before = screen.viewport_snapshot_with_window(0, screen.scrollback_len(), 0);
    screen.resize(6, 3);
    let after = screen.viewport_snapshot_with_window(0, screen.scrollback_len(), 0);
    // The last two complete repeated lines survive, even when the oldest is
    // partially evicted. Each new row inherits its source row's time.
    let last_complete = &after.rows()[after.row_count() - 7..after.row_count() - 1];
    for (index, row) in last_complete.iter().enumerate() {
        let source =
            &before.rows()[before.row_count() - 5 + index / 3 * 2 + usize::from(index % 3 == 2)];
        assert_eq!(row.timestamp_ms, source.timestamp_ms);
    }
    assert!(after.rows().iter().all(|row| row.timestamp_ms.is_some()));
}

#[test]
fn full_scrollback_rotation_keeps_metadata_and_cache_attached_to_original_line() {
    let mut screen = TerminalScreen::new(20, 2);
    screen.set_scrollback_limit(1);
    screen.advance(b"one\r\ntwo\x1b]133;C\x07");
    let before = screen.snapshot();
    let two_before = before
        .rows()
        .iter()
        .find(|row| row.text == "two")
        .expect("two before scrolling");
    let revision = two_before.revision;
    let timestamp = two_before.timestamp_ms;
    assert_eq!(two_before.command_mark, Some(ShellCommandMark::Output));

    screen.advance(b"\r\nthree");
    screen.advance(b"\r\nfour");
    assert_eq!(screen.scrollback_len(), 1);
    let history = screen.viewport_snapshot(1);
    let two_after = history
        .rows()
        .iter()
        .find(|row| row.text == "two")
        .expect("two retained at full scrollback limit");

    assert_eq!(two_after.revision, revision);
    assert_eq!(two_after.timestamp_ms, timestamp);
    assert_eq!(two_after.command_mark, Some(ShellCommandMark::Output));
    assert!(screen.last_metadata_prune_count <= 1);
}

#[test]
fn repeated_lines_receive_distinct_revisions_after_full_scrollback_rotation() {
    let mut screen = TerminalScreen::new(20, 2);
    screen.set_scrollback_limit(1);
    screen.advance(b"same\r\nsame");
    let second_revision = screen.snapshot().row(1).unwrap().revision;

    screen.advance(b"\r\nsame");
    screen.advance(b"\r\nsame");
    let history = screen.viewport_snapshot(1);

    assert_eq!(history.row(0).unwrap().revision, second_revision);
    assert_ne!(
        history.row(0).unwrap().revision,
        history.row(1).unwrap().revision
    );
}

#[test]
fn full_scrollback_rotation_keeps_image_on_original_line() {
    let mut screen = TerminalScreen::new(20, 2);
    screen.set_scrollback_limit(1);
    screen.advance(b"one\r\ntwo\x1b_Ga=T,i=9,c=1,r=1;QUI=\x1b\\");
    assert_eq!(screen.snapshot().images.len(), 1);

    screen.advance(b"\r\nthree");
    screen.advance(b"\r\nfour");
    let history = screen.viewport_snapshot(1);

    assert_eq!(history.line(0), Some("two"));
    assert_eq!(history.images.len(), 1);
    assert_eq!(history.images[0].row, 0);
}

#[test]
fn alternate_screen_presentation_state_does_not_overwrite_primary() {
    let mut screen = TerminalScreen::new(20, 2);
    screen.advance(b"primary\x1b]133;A\x07");
    let primary = screen.snapshot();
    let primary_row = primary.row(0).unwrap();
    let revision = primary_row.revision;
    let timestamp = primary_row.timestamp_ms;

    screen.advance(b"\x1b[?1049halt\r\nactivity\x1b[?1049l");
    let restored = screen.snapshot();
    let restored_row = restored.row(0).unwrap();

    assert_eq!(restored_row.text, "primary");
    assert_eq!(restored_row.revision, revision);
    assert_eq!(restored_row.timestamp_ms, timestamp);
    assert_eq!(restored_row.command_mark, Some(ShellCommandMark::Prompt));
}

#[test]
fn width_reflow_discards_uncertain_line_metadata() {
    let mut screen = TerminalScreen::new(10, 2);
    screen.advance(b"abcdefghij123");
    let old_revisions = screen
        .snapshot()
        .rows()
        .iter()
        .map(|row| row.revision)
        .collect::<Vec<_>>();

    screen.resize(6, 3);
    let reflowed = screen.snapshot();

    assert!(
        reflowed
            .rows()
            .iter()
            .all(|row| !old_revisions.contains(&row.revision))
    );
}

#[test]
fn window_snapshot_matches_adjacent_viewports() {
    let mut screen = TerminalScreen::new(20, 3);
    for line in 0..12 {
        screen.advance(format!("line-{line:02}\r\n").as_bytes());
    }
    let offset = 3;
    let older_rows = 2;
    let newer_rows = 2;
    let window = screen.viewport_snapshot_with_window(offset, older_rows, newer_rows);
    let base = screen.viewport_snapshot(offset);
    let older = screen.viewport_snapshot(offset + older_rows);
    let newer = screen.viewport_snapshot(offset - newer_rows);

    assert_eq!(window.viewport_rows, base.row_count());
    assert_eq!(
        window.row_count(),
        base.row_count() + older_rows + newer_rows
    );
    assert_eq!(
        &window.rows()[older_rows..older_rows + base.row_count()],
        base.rows()
    );
    assert_eq!(&window.rows()[..older_rows], &older.rows()[..older_rows]);
    assert_eq!(
        &window.rows()[older_rows + base.row_count()..],
        &newer.rows()[base.row_count() - newer_rows..]
    );
}

#[test]
fn live_window_snapshot_offsets_cursor_by_prepended_rows() {
    let mut screen = TerminalScreen::new(20, 3);
    for line in 0..8 {
        screen.advance(format!("line-{line:02}\r\n").as_bytes());
    }
    let base = screen.viewport_snapshot(0);
    let window = screen.viewport_snapshot_with_window(0, 4, 4);

    assert_eq!(window.row_count(), base.row_count() + 4);
    assert_eq!(window.cursor.row, base.cursor.row + 4);
    assert_eq!(window.total_rows, base.total_rows);
}

#[test]
fn scrollback_limit_updates_terminal_history() {
    let mut screen = TerminalScreen::new(20, 2);
    screen.advance(b"one\r\ntwo\r\nthree\r\nfour");
    assert!(screen.scrollback_len() > 1);

    screen.set_scrollback_limit(1);
    assert_eq!(screen.scrollback_len(), 1);
    assert_eq!(screen.total_rows(), 3);
}

#[test]
fn iterm2_clear_scrollback_clears_history() {
    let mut screen = TerminalScreen::new(20, 2);
    screen.advance(b"one\r\ntwo\r\nthree");
    assert!(screen.scrollback_len() > 0);

    screen.advance(b"\x1b]1337;ClearScrollback\x07");

    assert_eq!(screen.scrollback_len(), 0);
    assert!(screen.lines().iter().any(|line| line.contains("three")));
}

#[test]
fn clear_scrollback_then_scroll_same_chunk_stamps_history() {
    let mut screen = TerminalScreen::new(20, 2);
    screen.advance(b"one\r\ntwo\r\nthree");
    assert!(screen.scrollback_len() > 0);

    screen.advance(b"\x1b]1337;ClearScrollback\x07\r\nfour");

    assert_eq!(screen.scrollback_len(), 1);
    let snap = screen.viewport_snapshot(1);
    assert!(
        snap.rows()
            .iter()
            .any(|row| row.timestamp_ms.is_some() && row.text.contains("two")),
        "{:?}",
        snap.rows()
            .iter()
            .map(|row| row.text.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn scrollback_limit_applies_before_output() {
    let mut screen = TerminalScreen::new(20, 2);
    screen.set_scrollback_limit(1);
    screen.advance(b"one\r\ntwo\r\nthree\r\nfour");

    assert_eq!(screen.scrollback_len(), 1);
}

#[test]
fn clear_preserves_scrollback_limit() {
    let mut screen = TerminalScreen::new(20, 2);
    screen.set_scrollback_limit(1);
    screen.advance(b"one\r\ntwo\r\nthree\r\nfour");
    assert_eq!(screen.scrollback_len(), 1);

    screen.clear();
    screen.advance(b"five\r\nsix\r\nseven\r\neight");

    assert_eq!(screen.scrollback_len(), 1);
}

#[test]
fn clear_except_input_preserves_prompt_edit_and_cursor_but_removes_history() {
    let mut screen = TerminalScreen::new(20, 4);
    screen.advance(b"old one\r\nold two\r\nold three\r\nold four\r\n");
    screen.advance(b"\x1b[4;1Hbelow\x1b[3;1Hprompt> \x1b]133;B\x07draft");
    let before = screen.snapshot();
    assert!(screen.scrollback_len() > 0);

    screen.clear_except_input();

    let after = screen.snapshot();
    assert_eq!(screen.scrollback_len(), 0);
    assert!(before.cursor.row > 0);
    assert_eq!(after.cursor.row, before.cursor.row);
    assert_eq!(after.cursor.col, before.cursor.col);
    assert!(
        after.rows()[before.cursor.row]
            .text
            .contains("prompt> draft")
    );
    assert!(!screen.all_lines().join("\n").contains("old"));
    assert!(!screen.all_lines().join("\n").contains("below"));
    screen.advance(b"!\r\nnext");
    assert!(
        screen.snapshot().rows()[before.cursor.row]
            .text
            .contains("draft!")
    );
}

#[test]
fn clear_except_input_keeps_bottom_prompt_at_host_cursor_coordinates() {
    let mut screen = TerminalScreen::new(20, 12);
    screen.advance(b"\x1b[12;1Hroot# draft");
    screen.clear_except_input();

    let snapshot = screen.snapshot();
    assert_eq!(snapshot.cursor.row, 11);
    assert_eq!(snapshot.rows()[11].text, "root# draft");
    assert_eq!(snapshot.leading_cleared_rows(0), 11);
    assert!(
        snapshot
            .rows()
            .iter()
            .take(11)
            .all(|row| row.cleared && row.timestamp_ms.is_none())
    );
    assert!(!snapshot.rows()[11].cleared);
    assert!(
        snapshot
            .rows()
            .iter()
            .take(11)
            .all(|row| row.text.is_empty())
    );
    screen.advance(b"!");
    assert_eq!(screen.snapshot().rows()[11].text, "root# draft!");
}

#[test]
fn cleared_rows_follow_scrolling_and_host_rewrites() {
    let mut screen = TerminalScreen::new(20, 4);
    screen.advance(b"\x1b[4;1Hroot# draft");
    screen.clear_except_input();
    screen.advance(b"!\r\nresult");
    let snapshot = screen.snapshot();
    assert_eq!(snapshot.leading_cleared_rows(0), 2);
    assert!(
        snapshot
            .rows()
            .iter()
            .take(2)
            .all(|row| row.cleared && row.timestamp_ms.is_none())
    );
    assert_eq!(snapshot.rows()[2].text, "root# draft!");
    assert_eq!(snapshot.rows()[3].text, "result");
    assert_eq!(screen.scrollback_len(), 0);
    assert_eq!(snapshot.rows()[2].display_line_number, Some(1));
    assert_eq!(snapshot.rows()[3].display_line_number, Some(2));
    screen.advance(b"\x1b[Hnew");
    let snapshot = screen.snapshot();
    assert_eq!(snapshot.leading_cleared_rows(0), 0);
    assert!(!snapshot.rows()[0].cleared);
    assert!(snapshot.rows()[0].timestamp_ms.is_some());
}

#[test]
fn cleared_rows_do_not_hide_an_absolute_cursor_move_or_survive_reset() {
    let mut screen = TerminalScreen::new(20, 4);
    screen.advance(b"\x1b[4;1Hroot# draft");
    screen.clear_except_input();
    screen.advance(b"\x1b[H");
    assert_eq!(screen.snapshot().leading_cleared_rows(0), 0);
    screen.advance(b"\x1bc");
    assert!(screen.snapshot().rows().iter().all(|row| !row.cleared));
}

#[test]
fn clear_all_starts_numbering_at_one_and_scrolls_only_after_real_output_fills_viewport() {
    for chunk_size in [1, 8, 128] {
        let mut screen = TerminalScreen::new(20, 4);
        screen.advance(b"old\r\nold\r\nold\r\nroot# draft");
        screen.clear_except_input();
        assert_eq!(screen.snapshot().rows()[3].display_line_number, Some(1));
        let output = b"\r\ntwo\r\n\r\nfour";
        for chunk in output.chunks(chunk_size) {
            screen.advance(chunk);
        }
        assert_eq!(screen.scrollback_len(), 0, "chunk size {chunk_size}");
        let snapshot = screen.snapshot();
        assert_eq!(snapshot.scrollback_len, 0);
        assert_eq!(snapshot.total_rows, 4);
        assert_eq!(snapshot.leading_cleared_rows(0), 0);
        assert_eq!(snapshot.rows()[0].text, "root# draft");
        assert_eq!(snapshot.rows()[1].text, "two");
        assert!(snapshot.rows()[2].text.is_empty());
        assert!(
            !snapshot.rows()[2].cleared,
            "intentional blank output must survive"
        );
        for (index, row) in snapshot.rows().iter().enumerate() {
            assert_eq!(row.display_line_number, Some(index + 1));
        }
        screen.advance(b"\r\nfive");
        assert_eq!(screen.scrollback_len(), 1);
        let top = screen.viewport_snapshot(1);
        assert_eq!(top.rows()[0].text, "root# draft");
        assert_eq!(top.rows()[0].display_line_number, Some(1));
        assert_eq!(screen.snapshot().rows()[3].display_line_number, Some(5));
        assert!(
            screen
                .all_text()
                .unwrap()
                .starts_with("root# draft\ntwo\n\nfour\nfive")
        );
    }
}

#[test]
fn clear_all_preserves_new_blank_output_below_a_prompt_in_the_middle_of_the_screen() {
    let mut screen = TerminalScreen::new(20, 4);
    screen.advance(b"\x1b[2;1Hroot# draft");
    screen.clear_except_input();
    screen.advance(b"\r\n\r\nthree");
    let snapshot = screen.snapshot();
    assert!(!snapshot.rows()[2].cleared);
    assert_eq!(snapshot.rows()[2].display_line_number, Some(2));
    screen.advance(b"\r\nfour\r\nfive");
    assert_eq!(screen.scrollback_len(), 1);
    assert_eq!(screen.viewport_snapshot(1).rows()[0].text, "root# draft");
    assert_eq!(screen.viewport_snapshot(1).rows()[1].text, "");
    assert!(!screen.viewport_snapshot(1).rows()[1].cleared);
}

#[test]
fn clear_all_removes_old_history_again_and_keeps_the_configured_limit() {
    let mut screen = TerminalScreen::new(20, 4);
    screen.set_scrollback_limit(2);
    screen.advance(b"\x1b[4;1Hroot# draft");
    screen.clear_except_input();
    screen.advance(b"\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven");
    assert_eq!(screen.scrollback_len(), 2);
    assert_eq!(screen.snapshot().rows()[3].display_line_number, Some(7));
    screen.clear_except_input();
    assert_eq!(screen.scrollback_len(), 0);
    assert_eq!(screen.snapshot().rows()[3].display_line_number, Some(1));
    screen.advance(b"\r\neight");
    assert_eq!(screen.scrollback_len(), 0);
    assert_eq!(screen.snapshot().rows()[3].display_line_number, Some(2));
}

#[test]
fn clear_all_survives_width_reflow_and_keeps_erased_rows_out_of_history() {
    let mut screen = TerminalScreen::new(20, 6);
    screen.advance(b"\x1b[6;1Hroot# draft");
    screen.clear_except_input();
    screen.advance(b"\r\ntwo");
    for cols in [30, 16, 25] {
        screen.resize(cols, 6);
        assert_eq!(screen.scrollback_len(), 0);
        let snapshot = screen.snapshot();
        let prompt = snapshot
            .rows()
            .iter()
            .position(|row| row.text == "root# draft")
            .unwrap();
        assert_eq!(snapshot.leading_cleared_rows(0), prompt);
        assert_eq!(snapshot.rows()[prompt].display_line_number, Some(1));
        assert_eq!(snapshot.rows()[prompt + 1].display_line_number, Some(2));
    }
    screen.advance(b"\r\nthree");
    assert_eq!(screen.scrollback_len(), 0);
    assert_eq!(
        screen.snapshot().rows().last().unwrap().display_line_number,
        Some(3)
    );
}

#[test]
fn host_redraw_after_clear_all_rebases_line_numbers_without_creating_history() {
    let mut screen = TerminalScreen::new(20, 4);
    screen.advance(b"\x1b[4;1Hroot# draft");
    screen.clear_except_input();
    screen.advance(b"\x1b[H\x1b[0Jroot# draft");
    assert_eq!(screen.scrollback_len(), 0);
    assert_eq!(screen.snapshot().rows()[0].display_line_number, Some(1));
    screen.advance(b"\r\ntwo");
    assert_eq!(screen.snapshot().rows()[1].display_line_number, Some(2));
}

#[test]
fn clear_except_input_retains_wrapped_edit_rows() {
    let mut screen = TerminalScreen::new(8, 4);
    screen.advance(b"obsolete\r\ninput> \x1b]133;B\x07abcdefghij");
    let before = screen.snapshot();
    let start_row = before
        .rows()
        .iter()
        .position(|row| row.text.starts_with("input>"))
        .expect("input start");
    screen.clear_except_input();
    let after = screen.snapshot();
    assert_eq!(after.cursor.row, before.cursor.row);
    assert_eq!(after.cursor.col, before.cursor.col);
    assert!(after.rows()[start_row].text.starts_with("input>"));
    assert!(screen.all_lines().join("\n").contains("input>"));
    assert!(screen.all_lines().join("").contains("abcdefghij"));
    assert!(!screen.all_lines().join("\n").contains("obsolete"));
}

#[test]
fn clear_except_input_preserves_remote_saved_cursor_and_terminal_modes() {
    let mut screen = TerminalScreen::new(20, 4);
    screen.advance(b"\x1b[?2004h\x1b[4;1H\x1b7\x1b[3;1Hprompt> draft");
    screen.clear_except_input();
    assert!(screen.bracketed_paste());
    screen.advance(b"\x1b8X");
    let snapshot = screen.snapshot();
    assert!(snapshot.rows()[3].text.starts_with('X'));
    assert!(snapshot.rows()[2].text.contains("prompt> draft"));
}

#[test]
fn clear_except_input_keeps_wrapped_cursor_line_without_shell_markers() {
    let mut screen = TerminalScreen::new(8, 4);
    screen.advance(b"obsolete\r\nlong-prompt> edit");
    screen.clear_except_input();
    assert!(
        screen.snapshot().rows()[1].text.starts_with("long-pro"),
        "{:?}",
        screen.all_lines()
    );
    let text = screen.all_lines().join("");
    assert!(text.contains("long-prompt> edit"));
    assert!(!text.contains("obsolete"));
}

#[test]
fn sgr_truecolor_and_underline() {
    let mut screen = TerminalScreen::new(20, 2);
    screen.advance(b"\x1b[4;38;2;255;128;0mhi\x1b[0m");
    let styled = screen.styled_lines();
    assert_eq!(styled[0][0].text, "hi");
    assert!(styled[0][0].style.underline);
    assert_eq!(styled[0][0].style.fg_rgb, Some(0xff8000));
}

#[test]
fn bracketed_paste_mode_tracks_decset() {
    let mut screen = TerminalScreen::new(20, 2);
    assert!(!screen.bracketed_paste());
    screen.advance(b"\x1b[?2004h");
    assert!(screen.bracketed_paste());
    screen.advance(b"\x1b[?2004l");
    assert!(!screen.bracketed_paste());
}

#[test]
fn osc52_clipboard_store_emits_effect() {
    let mut screen = TerminalScreen::new(20, 2);
    // base64("hello-osc52") == aGVsbG8tb3NjNTI=
    screen.advance(b"\x1b]52;c;aGVsbG8tb3NjNTI=\x07");
    let effects = screen.take_effects();
    assert_eq!(effects.clipboard_store.as_deref(), Some("hello-osc52"));
}

#[test]
fn osc52_clipboard_load_emits_formatter() {
    let mut screen = TerminalScreen::new(20, 2);
    // Query clipboard contents via OSC 52.
    screen.advance(b"\x1b]52;c;?\x07");
    let effects = screen.take_effects();
    assert_eq!(effects.clipboard_loads.len(), 1);
    let reply = (effects.clipboard_loads[0])("payload");
    assert!(reply.starts_with("\x1b]52;c;"));
    // base64("payload") == cGF5bG9hZA==
    assert!(reply.contains("cGF5bG9hZA=="));
}

#[test]
fn text_area_size_request_uses_cell_metrics() {
    let mut screen = TerminalScreen::new(80, 24);
    screen.set_cell_metrics(10, 20);
    // CSI 14 t -> text area size in pixels
    screen.advance(b"\x1b[14t");
    let effects = screen.take_effects();
    assert_eq!(effects.pty_write.len(), 1);
    // height = 24*20 = 480, width = 80*10 = 800
    assert_eq!(effects.pty_write[0], b"\x1b[4;480;800t".to_vec());
}

#[test]
fn color_request_emits_rgb_reply() {
    let mut screen = TerminalScreen::new(20, 2);
    // OSC 10 ? -> query foreground
    screen.advance(b"\x1b]10;?\x07");
    let effects = screen.take_effects();
    assert_eq!(effects.pty_write.len(), 1);
    let reply = String::from_utf8(effects.pty_write[0].clone()).unwrap();
    assert!(reply.starts_with("\x1b]10;rgb:"));
    assert!(reply.contains("cccc"));
}

#[test]
fn focus_reporting_mode_tracks_decset() {
    let mut screen = TerminalScreen::new(20, 2);
    assert!(!screen.focus_reporting());
    screen.advance(b"\x1b[?1004h");
    assert!(screen.focus_reporting());
    assert_eq!(
        TerminalScreen::encode_focus_report(true),
        b"\x1b[I".to_vec()
    );
    assert_eq!(
        TerminalScreen::encode_focus_report(false),
        b"\x1b[O".to_vec()
    );
    screen.advance(b"\x1b[?1004l");
    assert!(!screen.focus_reporting());
}

#[test]
fn mouse_reporting_modes_track_decset() {
    let mut screen = TerminalScreen::new(20, 2);
    assert!(!screen.mouse_reporting());
    assert!(!screen.mouse_sgr());
    assert!(!screen.mouse_drag_reporting());
    assert!(!screen.mouse_motion_reporting());
    screen.advance(b"\x1b[?1000h");
    screen.advance(b"\x1b[?1006h");
    assert!(screen.mouse_reporting());
    assert!(screen.mouse_sgr());
    assert!(!screen.mouse_drag_reporting());
    screen.advance(b"\x1b[?1002h");
    assert!(screen.mouse_drag_reporting());
    assert!(!screen.mouse_motion_reporting());
    screen.advance(b"\x1b[?1003h");
    assert!(screen.mouse_motion_reporting());
}

#[test]
fn application_cursor_keys_track_decset() {
    let mut screen = TerminalScreen::new(20, 5);
    assert!(!screen.application_cursor_keys());
    screen.advance(b"\x1b[?1h");
    assert!(screen.application_cursor_keys());
    screen.advance(b"\x1b[?1l");
    assert!(!screen.application_cursor_keys());
}

#[test]
fn application_keypad_tracks_deckpam() {
    let mut screen = TerminalScreen::new(40, 3);
    assert!(!screen.application_keypad());
    screen.advance(b"\x1b=");
    assert!(screen.application_keypad());
    screen.advance(b"\x1b>");
    assert!(!screen.application_keypad());
}

#[test]
fn kitty_keyboard_disambiguate_mode_tracks_csi_u() {
    let mut screen = TerminalScreen::new(40, 3);
    assert!(!screen.kitty_keyboard_disambiguate());
    assert!(!screen.kitty_keyboard_report_event_types());
    assert!(!screen.kitty_keyboard_report_alternate_keys());
    assert!(!screen.kitty_keyboard_report_all_keys_as_esc());
    assert!(!screen.kitty_keyboard_report_associated_text());
    screen.advance(b"\x1b[=1u");
    assert!(screen.kitty_keyboard_disambiguate());
    screen.advance(b"\x1b[=31u");
    assert!(screen.kitty_keyboard_disambiguate());
    assert!(screen.kitty_keyboard_report_event_types());
    assert!(screen.kitty_keyboard_report_alternate_keys());
    assert!(screen.kitty_keyboard_report_all_keys_as_esc());
    assert!(screen.kitty_keyboard_report_associated_text());
    screen.advance(b"\x1b[=0u");
    assert!(!screen.kitty_keyboard_disambiguate());
    assert!(!screen.kitty_keyboard_report_event_types());
    assert!(!screen.kitty_keyboard_report_alternate_keys());
    assert!(!screen.kitty_keyboard_report_all_keys_as_esc());
    assert!(!screen.kitty_keyboard_report_associated_text());
}

#[test]
fn session_input_reset_preserves_screens_and_history_and_allows_new_protocols() {
    for alternate in [false, true] {
        let mut screen = TerminalScreen::new(32, 3);
        screen.advance(b"history\r\none\r\ntwo\r\nthree\r\n");
        // Leave different Kitty stacks on both screens, including nested modes.
        screen.advance(b"\x1b[>31u\x1b[>3u\x1b[?1049h\x1b[>31uALT");
        if !alternate {
            screen.advance(b"\x1b[?1049l");
        }
        screen.advance(b"\x1b[?1;1000;1002;1003;1004;1005;1006;2004h\x1b=\x1b[?1007l");
        let before = screen.all_lines();
        let history = screen.scrollback_len();
        let cursor = screen.snapshot().cursor;

        screen.reset_session_input_protocols();

        assert_eq!(screen.all_lines(), before);
        assert_eq!(screen.scrollback_len(), history);
        assert_eq!(screen.snapshot().cursor, cursor);
        assert_eq!(screen.alternate_screen(), alternate);
        assert!(!screen.mouse_reporting());
        assert!(!screen.mouse_sgr());
        assert!(!screen.mouse_utf8());
        assert!(!screen.focus_reporting());
        assert!(!screen.bracketed_paste());
        assert!(!screen.application_cursor_keys());
        assert!(!screen.application_keypad());
        assert!(screen.alternate_scroll());
        assert!(
            !screen
                .term
                .mode()
                .intersects(alacritty_terminal::term::TermMode::KITTY_KEYBOARD_PROTOCOL)
        );

        // Popping or swapping must not revive either old keyboard stack.
        screen.advance(b"\x1b[<1u\x1b[?1049h\x1b[<1u\x1b[?1049l\x1b[<1u");
        assert!(
            !screen
                .term
                .mode()
                .intersects(alacritty_terminal::term::TermMode::KITTY_KEYBOARD_PROTOCOL)
        );
        assert!(
            screen
                .all_lines()
                .iter()
                .any(|line| line.contains("history"))
        );

        screen.advance(b"\x1b[>31u\x1b[?1003;1006;1004;2004h");
        assert!(screen.kitty_keyboard_disambiguate());
        assert!(screen.kitty_keyboard_report_event_types());
        assert!(screen.kitty_keyboard_report_all_keys_as_esc());
        assert!(screen.mouse_motion_reporting());
        assert!(screen.mouse_sgr());
        assert!(screen.focus_reporting());
        assert!(screen.bracketed_paste());
    }
}

#[test]
fn stream_discontinuity_keeps_negotiated_input_protocols() {
    let mut screen = TerminalScreen::new(32, 3);
    screen.advance(b"\x1b[>31u\x1b[?1;1003;1004;1006;2004h\x1b=\x1b[");
    let modes = *screen.term.mode();

    screen.reset_stream_state();
    screen.advance(b"31mTEXT");

    assert_eq!(*screen.term.mode(), modes);
    assert!(screen.lines().iter().any(|line| line.contains("31mTEXT")));
}

#[test]
fn cursor_shape_and_visibility_follow_decscusr() {
    let mut screen = TerminalScreen::new(40, 3);
    screen.advance(b"hi");
    let snap = screen.snapshot();
    assert!(snap.cursor.visible);
    assert_eq!(snap.cursor.shape, CursorShape::Block);

    // DECSCUSR 3 = blinking underline; 4 = steady underline; 6 = steady bar; 0/1 = block.
    screen.advance(b"\x1b[3 q");
    let snap = screen.snapshot();
    assert_eq!(snap.cursor.shape, CursorShape::Underline);
    assert!(snap.cursor.blinking);
    assert!(snap.cursor.visible);

    screen.advance(b"\x1b[6 q");
    let snap = screen.snapshot();
    assert_eq!(snap.cursor.shape, CursorShape::Beam);
    assert!(!snap.cursor.blinking);

    // DECTCEM hide cursor (CSI ?25l).
    screen.advance(b"\x1b[?25l");
    let snap = screen.snapshot();
    assert!(!snap.cursor.visible);
    assert_eq!(snap.cursor.shape, CursorShape::Hidden);

    screen.advance(b"\x1b[?25h");
    let snap = screen.snapshot();
    assert!(snap.cursor.visible);
}

#[test]
fn alternate_scroll_defaults_on_and_tracks_decset() {
    let mut screen = TerminalScreen::new(20, 5);
    // Alacritty enables ALTERNATE_SCROLL by default.
    assert!(screen.alternate_scroll());
    screen.advance(b"\x1b[?1007l");
    assert!(!screen.alternate_scroll());
    screen.advance(b"\x1b[?1007h");
    assert!(screen.alternate_scroll());
}

#[test]
fn alternate_scroll_payload_requires_qualified_terminal_state() {
    let mut screen = TerminalScreen::new(20, 5);
    assert!(screen.alternate_scroll());
    assert_eq!(screen.alternate_scroll_payload(1), None);

    screen.advance(b"\x1b[?1049h");
    assert_eq!(screen.alternate_scroll_payload(0), None);
    assert_eq!(
        screen.alternate_scroll_payload(2),
        Some(b"\x1b[A\x1b[A".to_vec())
    );
    assert_eq!(
        screen.alternate_scroll_payload(-1),
        Some(b"\x1b[B".to_vec())
    );

    screen.advance(b"\x1b[?1h");
    assert_eq!(screen.alternate_scroll_payload(1), Some(b"\x1bOA".to_vec()));
    assert_eq!(
        screen.alternate_scroll_payload(-1),
        Some(b"\x1bOB".to_vec())
    );

    let capped = screen.alternate_scroll_payload(20).unwrap();
    assert_eq!(capped, b"\x1bOA".repeat(8));

    screen.advance(b"\x1b[?1000h");
    assert_eq!(screen.alternate_scroll_payload(1), None);
    screen.advance(b"\x1b[?1000l");
    screen.advance(b"\x1b[?1007l");
    assert_eq!(screen.alternate_scroll_payload(1), None);
}

#[test]
fn alternate_scroll_key_bytes_respect_cursor_mode() {
    assert_eq!(alternate_scroll_key_bytes(true, false), b"\x1b[A".to_vec());
    assert_eq!(alternate_scroll_key_bytes(false, true), b"\x1bOB".to_vec());
}

#[test]
fn encode_mouse_report_sgr_and_legacy() {
    let mut screen = TerminalScreen::new(80, 24);
    assert!(encode_mouse_report(&screen, 0, 0, 0, true).is_empty());
    screen.advance(b"\x1b[?1000h");
    let legacy = encode_mouse_report(&screen, 0, 0, 0, true);
    assert_eq!(legacy, vec![0x1b, b'[', b'M', 32, 33, 33]);
    screen.advance(b"\x1b[?1006h");
    let sgr = encode_mouse_report(&screen, 0, 1, 2, true);
    assert_eq!(sgr, b"\x1b[<0;2;3M".to_vec());
}

#[test]
fn encode_mouse_report_release_motion_and_modifiers() {
    let mut screen = TerminalScreen::new(80, 24);
    screen.advance(b"\x1b[?1000h");
    let legacy_release = encode_mouse_report(&screen, 0, 0, 0, false);
    assert_eq!(legacy_release, vec![0x1b, b'[', b'M', 35, 33, 33]);

    screen.advance(b"\x1b[?1006h");
    // SGR release reports the button that was released (0), not legacy code 3.
    let sgr_release = encode_mouse_report(&screen, 0, 3, 4, false);
    assert_eq!(sgr_release, b"\x1b[<0;4;5m".to_vec());
    let sgr_right_release = encode_mouse_report(&screen, 2, 1, 1, false);
    assert_eq!(sgr_right_release, b"\x1b[<2;2;2m".to_vec());

    let modified_motion =
        encode_mouse_report_with_modifiers(&screen, 0, 1, 2, true, true, true, true, true);
    assert_eq!(modified_motion, b"\x1b[<60;2;3M".to_vec());

    let any_motion =
        encode_mouse_report_with_modifiers(&screen, 3, 4, 5, true, true, false, false, false);
    assert_eq!(any_motion, b"\x1b[<35;5;6M".to_vec());
}

#[test]
fn graphics_iterm2_does_not_pollute_grid() {
    let mut screen = TerminalScreen::new(40, 8);
    // Minimal "PNG" base64 payload via iTerm2 inline.
    screen.advance(b"pre\x1b]1337;File=name=x.png;width=3;height=2;inline=1:UE5H\x07post");
    let snap = screen.snapshot();
    let joined = snapshot_text(&snap);
    assert!(joined.contains("pre"), "{joined:?}");
    assert!(joined.contains("post"), "{joined:?}");
    assert!(
        !joined.contains("1337") && !joined.contains("File="),
        "graphics payload leaked into grid: {joined:?}"
    );
    assert_eq!(snap.images.len(), 1);
    assert_eq!(snap.images[0].width_cells, 3);
    assert_eq!(snap.images[0].height_cells, 2);
    assert_eq!(snap.images[0].protocol, GraphicsProtocol::ITerm2);
    assert_eq!(snap.images[0].data.as_ref(), b"PNG");
}

#[test]
fn graphics_iterm2_file_without_inline_does_not_place_image() {
    let mut screen = TerminalScreen::new(40, 8);
    screen.advance(b"pre\x1b]1337;File=name=x.png;width=3;height=2:UE5H\x07post");
    let snap = screen.snapshot();
    let joined = snapshot_text(&snap);
    assert!(joined.contains("pre"), "{joined:?}");
    assert!(joined.contains("post"), "{joined:?}");
    assert!(
        !joined.contains("1337") && !joined.contains("File="),
        "download-only OSC 1337 leaked into grid: {joined:?}"
    );
    assert!(snap.images.is_empty());
}

#[test]
fn graphics_kitty_placement_appears_in_snapshot() {
    let mut screen = TerminalScreen::new(40, 8);
    screen.advance(b"\x1b_Ga=T,i=7,c=5,r=3;QUJD\x1b\\");
    let snap = screen.snapshot();
    assert_eq!(snap.images.len(), 1);
    assert_eq!(snap.images[0].protocol, GraphicsProtocol::Kitty);
    assert_eq!(snap.images[0].width_cells, 5);
    assert_eq!(snap.images[0].height_cells, 3);
    assert_eq!(snap.images[0].data.as_ref(), b"ABC");
}

#[test]
fn graphics_delete_clears_kitty_image() {
    let mut screen = TerminalScreen::new(40, 8);
    screen.advance(b"\x1b_Ga=T,i=3,c=2,r=2;QUI=\x1b\\");
    assert_eq!(screen.snapshot().images.len(), 1);
    screen.advance(b"\x1b_Ga=d,i=3\x1b\\");
    assert!(screen.snapshot().images.is_empty());
}

#[test]
fn graphics_kitty_multi_chunk_via_advance() {
    let mut screen = TerminalScreen::new(40, 8);
    // m=1 then m=0 with base64 "AB" + "CD"; a=T places after final chunk.
    // a=t would be store-only (see graphics store/place unit tests).
    screen.advance(b"\x1b_Ga=T,i=11,c=3,r=2,m=1;QUI=\x1b\\");
    assert!(screen.snapshot().images.is_empty());
    screen.advance(b"\x1b_Ga=T,i=11,m=0;Q0Q=\x1b\\");
    let snap = screen.snapshot();
    assert_eq!(snap.images.len(), 1);
    assert_eq!(snap.images[0].data.as_ref(), b"ABCD");
    assert_eq!(snap.images[0].width_cells, 3);
    assert_eq!(snap.images[0].height_cells, 2);
}

#[test]
fn graphics_same_payload_shares_content_id_across_placements() {
    let mut screen = TerminalScreen::new(40, 8);
    screen.advance(b"\x1b_Ga=T,i=7,p=1,c=2,r=1;QUJD\x1b\\");
    screen.advance(b"\x1b_Ga=T,i=8,p=2,c=2,r=1;QUJD\x1b\\");
    let snap = screen.snapshot();

    assert_eq!(snap.images.len(), 2);
    assert_ne!(snap.images[0].id, snap.images[1].id);
    assert_eq!(snap.images[0].content_id, snap.images[1].content_id);
    assert_eq!(snap.images[0].data.as_ref(), snap.images[1].data.as_ref());
}

#[test]
fn graphics_sixel_via_advance() {
    let mut screen = TerminalScreen::new(40, 8);
    // Solid red sixel column.
    screen.advance(b"\x1bP0;0;0q#0;2;100;0;0#0~\x1b\\");
    let snap = screen.snapshot();
    assert_eq!(snap.images.len(), 1);
    assert_eq!(snap.images[0].protocol, GraphicsProtocol::Sixel);
    assert!(snap.images[0].data.starts_with(b"NYAR"));
    assert!(snap.images[0].width_cells >= 1);
    assert!(snap.images[0].height_cells >= 1);
}

#[test]
fn graphics_kitty_cursor_motion_via_advance() {
    let mut screen = TerminalScreen::new(40, 8);
    // Place 3x2 at origin with C=1; cursor should leave top-left.
    screen.advance(b"\x1b_Ga=T,i=1,c=3,r=2,C=1;QUI=\x1b\\");
    let snap = screen.snapshot();
    assert_eq!(snap.images.len(), 1);
    // After CUD1 + CHA4: row=1, col=3 (0-based).
    assert_eq!(snap.cursor.row, 1);
    assert_eq!(snap.cursor.col, 3);
}

#[test]
fn graphics_after_scroll_in_same_chunk_stays_on_live_screen() {
    let mut screen = TerminalScreen::new(40, 3);
    screen.advance(b"one\r\ntwo\r\nthree\r\n\x1b_Ga=T,i=1,c=1,r=1;QUI=\x1b\\");
    let snap = screen.snapshot();
    assert_eq!(snap.images.len(), 1);
    assert_eq!(
        snap.images[0].row, snap.cursor.row,
        "image placed after scroll should not be shifted into history"
    );
}

#[test]
fn graphics_kitty_rgb24_via_advance() {
    let mut screen = TerminalScreen::new(40, 8);
    // f=24,s=1,v=1 single red RGB pixel (base64 of FF 00 00 = /wAA)
    screen.advance(b"\x1b_Ga=T,i=1,f=24,s=1,v=1,c=1,r=1;/wAA\x1b\\");
    let snap = screen.snapshot();
    assert_eq!(snap.images.len(), 1);
    assert!(snap.images[0].data.starts_with(b"NYAR"));
    assert_eq!(&snap.images[0].data[12..16], &[255, 0, 0, 255]);
}

#[test]
fn graphics_kitty_query_via_advance() {
    let mut screen = TerminalScreen::new(40, 8);
    screen.advance(b"\x1b_Ga=t,i=4,c=1,r=1,q=2;QUI=\x1b\\");
    let effects = screen.take_effects();
    assert_eq!(effects.pty_write.len(), 1);
    assert!(
        String::from_utf8_lossy(&effects.pty_write[0]).contains("OK"),
        "{:?}",
        effects.pty_write
    );
    screen.advance(b"\x1b_Ga=q,i=4\x1b\\");
    let effects = screen.take_effects();
    assert!(
        String::from_utf8_lossy(&effects.pty_write[0]).contains("OK"),
        "{:?}",
        effects.pty_write
    );
    screen.advance(b"\x1b_Ga=q,i=99\x1b\\");
    let effects = screen.take_effects();
    assert!(
        String::from_utf8_lossy(&effects.pty_write[0]).contains("ENOENT"),
        "{:?}",
        effects.pty_write
    );
}

#[test]
fn encoding_gbk_output_decodes_to_grid() {
    let mut screen = TerminalScreen::new(40, 8);
    screen.set_encoding("GBK").unwrap();
    // GBK "测"
    screen.advance(&[0xb2, 0xe2]);
    let snap = screen.snapshot();
    let joined = snapshot_text(&snap);
    assert!(joined.contains('测'), "grid={joined:?}");
}

#[test]
fn decoded_local_text_bypasses_session_charset() {
    let mut screen = TerminalScreen::new(40, 8);
    screen.set_encoding("GBK").unwrap();

    screen.advance_decoded_text("本地提示");

    let joined = snapshot_text(&screen.snapshot());
    let compact = joined.replace(' ', "");
    assert!(compact.contains("本地提示"), "grid={joined:?}");
    assert!(!joined.contains('\u{fffd}'), "grid={joined:?}");
}

#[test]
fn encoding_gbk_output_decodes_split_multibyte_to_grid() {
    let mut screen = TerminalScreen::new(40, 8);
    screen.set_encoding("GBK").unwrap();
    // GBK "测试" split in the middle of the first character.
    screen.advance(&[0xb2]);
    assert!(
        !snapshot_text(&screen.snapshot()).contains('\u{fffd}'),
        "incomplete byte should not render as replacement"
    );
    screen.advance(&[0xe2, 0xca, 0xd4]);
    let joined = snapshot_text(&screen.snapshot());
    assert!(
        joined.contains('测') && joined.contains('试') && !joined.contains('\u{fffd}'),
        "grid={joined:?}"
    );
}

#[test]
fn output_decoder_gbk_decodes_split_multibyte_text() {
    let mut decoder = TerminalOutputDecoder::new();
    decoder.set_encoding("GBK").unwrap();
    assert!(decoder.decode_output_text(&[0xb2]).is_empty());
    let text = decoder.decode_output_text(&[0xe2, 0xca, 0xd4]);
    assert_eq!(text, "测试");
}

#[test]
fn output_decoder_utf8_decodes_split_multibyte_text() {
    let mut decoder = TerminalOutputDecoder::new();
    let bytes = "测".as_bytes();
    assert!(decoder.decode_output_text(&bytes[..1]).is_empty());
    assert_eq!(decoder.decode_output_text(&bytes[1..]), "测");
}

#[test]
fn output_decoder_tail_keeps_only_the_last_bytes() {
    let mut decoder = TerminalOutputDecoder::new();
    assert_eq!(decoder.decode_output_text_tail(b"abcdefgh", 3), "fgh");
}

#[test]
fn output_decoder_tail_returns_everything_below_the_cap() {
    let mut decoder = TerminalOutputDecoder::new();
    assert_eq!(decoder.decode_output_text_tail(b"abc", 64), "abc");
}

/// The cap is a byte budget, but it must never slice a character in half.
#[test]
fn output_decoder_tail_snaps_to_a_character_boundary() {
    let mut decoder = TerminalOutputDecoder::new();
    // Each character is 3 bytes; a 4-byte budget can only fit the last one.
    assert_eq!(decoder.decode_output_text_tail("测试".as_bytes(), 4), "试");
}

/// Capping the *result* must not desync the decoder: a character split
/// across the chunk boundary still lands whole in the next call.
#[test]
fn output_decoder_tail_keeps_streaming_state_exact() {
    let mut decoder = TerminalOutputDecoder::new();
    let bytes = "测".as_bytes();
    let mut first = Vec::from(&b"abcdefgh"[..]);
    first.extend_from_slice(&bytes[..1]);

    assert_eq!(decoder.decode_output_text_tail(&first, 3), "fgh");
    assert_eq!(decoder.decode_output_text_tail(&bytes[1..], 64), "测");
}

/// Graphics payloads stay excluded from the text tail, exactly as they are
/// from the uncapped decode.
#[test]
fn output_decoder_tail_skips_graphics_payload() {
    let mut decoder = TerminalOutputDecoder::new();
    let text = decoder.decode_output_text_tail(b"pre\x1b_Ga=T,i=1,c=1,r=1;QUI=\x1b\\post", 64);
    assert_eq!(text, "prepost");
}

#[test]
fn output_decoder_skips_graphics_payload() {
    let mut decoder = TerminalOutputDecoder::new();
    decoder.set_encoding("GBK").unwrap();
    let text = decoder.decode_output_text(b"pre\x1b_Ga=T,i=1,c=1,r=1;QUI=\x1b\\post");
    assert_eq!(text, "prepost");
}

#[test]
fn output_decoder_skips_iterm2_graphics_payload() {
    let mut decoder = TerminalOutputDecoder::new();
    let text = decoder
        .decode_output_text(b"pre\x1b]1337;File=name=x.png;width=4;height=2;inline=1:UE5H\x07post");
    assert_eq!(text, "prepost");
}

#[test]
fn output_decoder_skips_sixel_graphics_payload() {
    let mut decoder = TerminalOutputDecoder::new();
    let text = decoder.decode_output_text(b"pre\x1bP0;0;0q#0;2;100;0;0#0~\x1b\\post");
    assert_eq!(text, "prepost");
}

#[test]
fn output_decoder_encoding_change_drops_pending_multibyte_state() {
    let mut decoder = TerminalOutputDecoder::new();
    decoder.set_encoding("GBK").unwrap();

    assert!(decoder.decode_output_text(&[0xb2]).is_empty());
    decoder.set_encoding("UTF-8").unwrap();

    assert_eq!(decoder.decode_output_text(b"ok"), "ok");
}

#[test]
fn terminal_screen_encoding_change_drops_pending_graphics_state() {
    let mut screen = TerminalScreen::new(40, 8);

    screen.advance(b"\x1b_Ga=T,i=1,c=1,r=1;QUI=");
    screen.set_encoding("GBK").unwrap();
    screen.advance(b"\x1b\\");

    assert!(
        screen.snapshot().images.is_empty(),
        "incomplete graphics should not survive an encoding switch"
    );
}

#[test]
fn encoding_outgoing_reencodes_utf8_text() {
    let mut screen = TerminalScreen::new(40, 8);
    screen.set_encoding("GBK").unwrap();
    assert_eq!(
        screen.encode_outgoing_str("测试").unwrap(),
        [0xb2, 0xe2, 0xca, 0xd4]
    );
    assert_eq!(screen.encode_outgoing(b"\x1b[A").unwrap(), b"\x1b[A");
}

#[test]
fn charset_controls_graphics_and_replies_are_independent_of_chunk_boundaries() {
    use nyaterm_core::character_encoding::CharacterEncoding;
    for (encoding, text) in [
        (CharacterEncoding::Utf8, "测试"),
        (CharacterEncoding::Gbk, "测试"),
        (CharacterEncoding::Gb18030, "测试😀"),
        (CharacterEncoding::Big5, "測試"),
        (CharacterEncoding::ShiftJis, "日本語"),
        (CharacterEncoding::EucKr, "한국어"),
    ] {
        let mut wire = b"\x1b[31m".to_vec();
        wire.extend(encoding.encode(text).unwrap());
        wire.extend_from_slice(b"\x1b[0m\x1b[2;3H\x1b]2;");
        wire.extend(encoding.encode(text).unwrap());
        wire.extend_from_slice(b"\x07\x1b_Ga=T,f=32,s=1,v=1,i=9;/wAA/w==\x1b\\");
        wire.extend_from_slice(b"\x1b_Ga=q,i=7;\x1b\\\x1b[6n");
        let mut baseline = TerminalScreen::new(30, 5);
        baseline.set_encoding(encoding.label()).unwrap();
        baseline.advance(&wire);
        let expected = baseline.snapshot();
        let expected_effects = baseline.take_effects();
        assert!(snapshot_text(&expected).contains(text));
        assert_eq!(expected.images.len(), 1);
        assert!(!expected_effects.pty_write.is_empty());
        for split in 0..=wire.len() {
            let mut screen = TerminalScreen::new(30, 5);
            screen.set_encoding(encoding.label()).unwrap();
            screen.advance(&wire[..split]);
            screen.advance(&wire[split..]);
            let snapshot = screen.snapshot();
            assert_eq!(snapshot.cursor, expected.cursor, "{encoding} split {split}");
            assert_eq!(
                snapshot.images.len(),
                expected.images.len(),
                "{encoding} split {split}"
            );
            for (actual, expected) in snapshot.images.iter().zip(&expected.images) {
                assert_eq!(actual.data, expected.data, "{encoding} split {split}");
                assert_eq!(
                    (
                        actual.row,
                        actual.col,
                        actual.width_cells,
                        actual.height_cells
                    ),
                    (
                        expected.row,
                        expected.col,
                        expected.width_cells,
                        expected.height_cells
                    ),
                    "{encoding} split {split}"
                );
            }
            for (actual, expected) in snapshot.rows().iter().zip(expected.rows()) {
                assert_eq!(actual.cells, expected.cells, "{encoding} split {split}");
                assert_eq!(
                    actual.styled_spans, expected.styled_spans,
                    "{encoding} split {split}"
                );
            }
            let effects = screen.take_effects();
            assert_eq!(
                effects.pty_write, expected_effects.pty_write,
                "{encoding} split {split}"
            );
            assert_eq!(
                effects.title, expected_effects.title,
                "{encoding} split {split}"
            );
        }
    }
}

#[test]
fn rejected_encoding_and_alias_switch_preserve_pending_characters() {
    let mut screen = TerminalScreen::new(20, 3);
    screen.set_encoding("GBK").unwrap();
    screen.advance(&[0xb2]);
    assert!(screen.set_encoding("KOI8-R").is_err());
    screen.set_encoding("CP936").unwrap();
    screen.advance(&[0xe2]);
    assert!(snapshot_text(&screen.snapshot()).contains('测'));
    let mut decoder = TerminalOutputDecoder::new();
    decoder.set_encoding("GBK").unwrap();
    assert!(decoder.decode_output_text(&[0xb2]).is_empty());
    assert!(decoder.set_encoding("UTF-16").is_err());
    decoder.set_encoding("GB2312").unwrap();
    assert_eq!(decoder.decode_output_text(&[0xe2]), "测");
}

#[test]
fn logical_input_rejects_unmappable_and_non_utf8_bytes() {
    use nyaterm_core::character_encoding::{CharacterEncoding, EncodingError};
    let mut screen = TerminalScreen::default();
    screen.set_encoding("GBK").unwrap();
    assert_eq!(
        screen.encode_outgoing_str("secret😀"),
        Err(EncodingError::UnrepresentableText(CharacterEncoding::Gbk))
    );
    assert_eq!(
        screen.encode_outgoing(&[0xff]),
        Err(EncodingError::InvalidUtf8Input)
    );
    screen.set_encoding("UTF-8").unwrap();
    assert_eq!(
        screen.encode_outgoing(&[0xff]),
        Err(EncodingError::InvalidUtf8Input)
    );
    screen.set_encoding("GB18030").unwrap();
    let encoded = screen.encode_outgoing_str("😀").unwrap();
    screen.advance(&encoded);
    assert!(snapshot_text(&screen.snapshot()).contains('😀'));
}

#[test]
fn utf8_mouse_coordinates_are_protocol_bytes_in_legacy_charset_sessions() {
    for label in ["UTF-8", "GBK", "GB18030", "Big5", "Shift_JIS", "EUC-KR"] {
        let mut screen = TerminalScreen::new(120, 120);
        screen.set_encoding(label).unwrap();
        screen.advance(b"\x1b[?1000h\x1b[?1005h");
        assert!(screen.mouse_utf8());
        assert_eq!(
            crate::encode_mouse_report(&screen, 0, 100, 110, true),
            b"\x1b[M \xc2\x85\xc2\x8f"
        );
        assert!(crate::encode_mouse_report(&screen, 0, 2015, 0, true).is_empty());
        screen.advance(b"\x1b[?1006h");
        assert_eq!(
            crate::encode_mouse_report(&screen, 0, 100, 110, true),
            b"\x1b[<0;101;111M"
        );
    }
}

#[test]
fn tab_cell_materializes_stop_columns_for_paint() {
    // The alacritty fork's `put_tab` keeps the literal tab in the starting cell
    // and leaves blank cells up to the next tab stop. The GPUI paint layer relies
    // on these exact columns: the tab span covers the tab cell plus the blanks,
    // so colored spans after it start at the stop. Changing tab handling in the
    // grid invalidates that contract.
    let mut screen = TerminalScreen::new(40, 2);
    screen.advance(b"System load:  \t\x1b[0;33;40m0.33\x1b[0m");
    let snapshot = screen.snapshot();
    let row = snapshot.row(0).unwrap();

    assert_eq!(row.cells[14].text(), "\t");
    assert!(row.cells[15].text().is_empty());
    assert_eq!(
        &row.cells[16..20]
            .iter()
            .map(|c| c.text())
            .collect::<String>(),
        "0.33"
    );
    assert!(
        row.cells[16..20]
            .iter()
            .all(|cell| cell.style.fg == Some(3) && cell.style.bg == Some(0))
    );

    // Span accounting must count the tab cell and the following blank as two
    // columns, so the colored span's background starts at the tab stop.
    assert_eq!(row.styled_spans[0].text, "System load:  \t ");
    assert_eq!(row.styled_spans[1].text, "0.33");
    assert_eq!(row.styled_spans[1].style.bg, Some(0));
}

#[test]
fn history_coordinates_distinguish_growth_from_full_buffer_rotation() {
    let mut screen = TerminalScreen::new(16, 3);
    screen.set_scrollback_limit(8);
    let initial = screen.history_state();
    screen.advance(b"0\r\n1\r\n2\r\n3\r\n4\r\n");
    let growing = screen.snapshot();
    assert!(growing.history.preserves_coordinates_from(initial));
    assert!(growing.history.scroll_count > initial.scroll_count);
    screen.advance(b"5\r\n6\r\n7\r\n8\r\n9\r\n10\r\n");
    let full = screen.snapshot();
    assert_eq!(full.scrollback_len, 8);
    let reading = screen.viewport_snapshot(3);
    screen.advance(b"11\r\n12\r\n");
    let rotated = screen.snapshot();
    assert_eq!(rotated.scrollback_len, full.scrollback_len);
    assert_eq!(rotated.history.scrolled_since(full.history), Some(2));
    assert!(!rotated.history.preserves_coordinates_from(full.history));
    let anchored = rotated.history.reanchor_offset(full.history, 3);
    assert_eq!(anchored, 5);
    assert_eq!(screen.viewport_snapshot(anchored).line(0), reading.line(0));
    assert_eq!(rotated.history.reanchor_offset(full.history, 0), 0);
}

#[test]
fn history_coordinates_reject_reset_resize_screen_switch_and_backwards_snapshots() {
    let mut screen = TerminalScreen::new(16, 3);
    screen.advance(b"0\r\n1\r\n2\r\n3\r\n");
    let before = screen.history_state();
    screen.advance(b"4\r\n");
    let after = screen.history_state();
    assert_eq!(before.scrolled_since(after), None);
    screen.advance(b"\x1b[?1049h");
    assert_eq!(screen.history_state().scrolled_since(after), None);
    screen.advance(b"\x1b[?1049l");
    let primary = screen.history_state();
    screen.resize(20, 3);
    assert_eq!(screen.history_state().scrolled_since(primary), None);
    let resized = screen.history_state();
    screen.advance(b"\x1bc");
    assert_eq!(screen.history_state().scrolled_since(resized), None);
}

#[test]
fn history_scroll_counter_wrap_preserves_a_continuous_observation() {
    let previous = super::TerminalHistoryState {
        scroll_count: u64::MAX,
        scrollback_len: 8,
        ..Default::default()
    };
    let current = super::TerminalHistoryState {
        scroll_count: 1,
        ..previous
    };
    assert_eq!(current.scrolled_since(previous), Some(2));
    assert_eq!(current.reanchor_offset(previous, 7), 8);
    assert!(!current.preserves_coordinates_from(previous));
}

#[test]
fn row_signature_ignores_trailing_default_blanks_but_not_styled_ones() {
    let mut screen = TerminalScreen::new(40, 3);
    screen.advance(b"ab");
    let plain = screen.snapshot().row(0).unwrap().signature;

    screen.advance(b"\x1b[2K\rab");
    assert_eq!(screen.snapshot().row(0).unwrap().signature, plain);

    screen.advance(b"\x1b[2K\rab\x1b[44m   \x1b[0m");
    assert_ne!(screen.snapshot().row(0).unwrap().signature, plain);

    screen.advance(b"\x1b[2K\rab\x1b[0m");
    let erased_and_rewritten = screen.snapshot().row(0).unwrap().signature;
    assert_eq!(erased_and_rewritten, plain);
}
