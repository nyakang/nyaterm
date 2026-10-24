use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::sync::Arc;

use gpui::{Bounds, ShapedLine, SharedString, font, point, px, rgb, size};
use nyaterm_core::ResolvedKeywordHighlightRule;
use nyaterm_terminal::selection::{
    TerminalBufferCellPos, TerminalCellSide, TerminalSelection, TerminalSelectionEndpoint,
    selected_row_text,
};
use nyaterm_terminal::{ShellInputLineKind, TerminalScreen, TerminalSnapshot};

use super::{
    NyaTerminalElement, NyaTerminalLayoutCache, TERMINAL_LAYOUT_CACHE_ROW_CAP,
    TERMINAL_LAYOUT_CACHE_ROW_ORDER_MIN_COMPACT, TerminalGridSelection, TerminalKeywordLayoutState,
    TerminalLineDecorations, TerminalRowBackgroundRange, TerminalRowDecorationRange,
    TerminalRowTextDecorations, append_padded_wide_cells, append_terminal_glyph_span,
    hash_styled_spans, pad_wide_cells, push_dynamic_decoration_backgrounds,
    push_dynamic_link_underlines, push_dynamic_selection_background, push_terminal_zebra_stripes,
    terminal_background_ranges_for_cells, terminal_background_ranges_for_spans,
    terminal_cursor_cell_hidden, terminal_glyph_decorations_needed, terminal_layout_height_px,
    terminal_layout_prefetch_row, terminal_link_underline_color, terminal_plain_row_fast_path,
    terminal_row_layout_key, terminal_selection_cols_for_snapshot_row, terminal_text_run_for_span,
    terminal_underline_bounds, terminal_underline_ranges_for_spans,
    terminal_visible_rows_for_bounds, terminal_visible_rows_for_clipped_bounds,
};
use crate::contrast::{TerminalContrastCache, TerminalContrastOverlays, contrast_ratio};
use crate::glyphs::CellGlyph;
use crate::keywords::{
    TerminalKeywordHighlightLookup, compile_terminal_keyword_highlighter,
    precompute_terminal_input_highlights, precompute_terminal_input_highlights_reusing,
    precompute_terminal_keyword_highlights, terminal_keyword_row_reuse_keys,
    terminal_keyword_rules_key,
};
use crate::paint::{
    apply_action_link_ranges, flatten_highlight_spans, terminal_highlight_spans_with_keyword_ranges,
};
use crate::types::{TerminalHighlightSpan, TerminalPaintGeometry};

fn edit_snapshot_row(
    snapshot: &mut TerminalSnapshot,
    row: usize,
    edit: impl FnOnce(&mut nyaterm_terminal::TerminalSnapshotRow),
) {
    let rows = Arc::make_mut(&mut snapshot.row_data);
    edit(Arc::make_mut(&mut rows[row]));
}

#[test]
fn current_keyword_layout_key_changes_when_osc133_changes_identical_text() {
    let palette = nyaterm_ui::theme_palette("github-dark");
    let highlighter =
        compile_terminal_keyword_highlighter(&nyaterm_core::get_builtin_keyword_rules(true));
    let mut screen = TerminalScreen::new(30, 2);
    screen.advance(b"ERROR");
    let output = screen.snapshot();
    let previous = precompute_terminal_keyword_highlights(&output, &highlighter, palette, None);
    screen.advance(b"\r\x1b]133;B\x07");
    let input = screen.snapshot();
    let current =
        precompute_terminal_keyword_highlights(&input, &highlighter, palette, Some(&previous));
    assert_eq!(
        output.row(0).unwrap().revision,
        input.row(0).unwrap().revision
    );
    let old_lookup = previous.lookup(0, &output);
    let new_lookup = current.lookup(0, &input);
    let old_key = super::terminal_keyword_row_paint_style_key(7, 0, old_lookup.as_ref());
    let new_key = super::terminal_keyword_row_paint_style_key(7, 0, new_lookup.as_ref());
    assert_ne!(old_key, new_key);
    assert_eq!(
        new_key,
        super::terminal_keyword_row_paint_style_key(7, 0, current.lookup(0, &input).as_ref())
    );
}

#[test]
fn append_padded_wide_cells_matches_allocating_padding() {
    let input = "a界e\u{301} 🚀";
    let expected = pad_wide_cells(input);
    let mut output = String::from("prefix:");
    let before = output.len();
    let added = append_padded_wide_cells(&mut output, input);

    assert_eq!(&output[before..], expected);
    assert_eq!(added, expected.len());
}

#[test]
fn trimmed_row_decorations_retain_ansi_tail_and_highlight_colors() {
    let palette = nyaterm_ui::theme_palette("github-dark");
    let mut screen = TerminalScreen::new(24, 2);
    screen.advance(b"\x1b[4;9mTEXT\x1b[31m        \x1b[0m");
    let snapshot = screen.snapshot();
    let row = snapshot.row(0).unwrap();
    assert_eq!(row.text, "TEXT");
    let mut spans = terminal_highlight_spans_with_keyword_ranges(
        &row.text,
        Some(&row.styled_spans),
        None,
        &[],
        palette,
    );
    spans[0].color = Some(0x123456);
    let decorations = super::terminal_text_decorations_for_row(
        &spans,
        Some(&row.cells),
        palette,
        false,
        1.0,
        &TerminalContrastOverlays::default(),
        &mut TerminalContrastCache::default(),
    );
    let expected = vec![(0x123456, 0, 4), (palette.terminal_ansi_color(1), 4, 12)];
    assert_eq!(
        underline_ranges_as_tuples(&decorations.underlines),
        expected
    );
    assert_eq!(
        underline_ranges_as_tuples(&decorations.strikethroughs),
        expected
    );
}

#[test]
fn struck_cell_graphics_cache_decorations_outside_shaped_text() {
    let palette = nyaterm_ui::theme_palette("github-dark");
    let mut screen = TerminalScreen::new(16, 2);
    screen.advance("\x1b[9m█▀──\x1b[0m".as_bytes());
    let snapshot = screen.snapshot();
    let row = snapshot.row(0).unwrap();
    let spans = terminal_highlight_spans_with_keyword_ranges(
        &row.text,
        Some(&row.styled_spans),
        None,
        &[],
        palette,
    );
    let decorations = super::terminal_text_decorations_for_row(
        &spans,
        Some(&row.cells),
        palette,
        false,
        1.0,
        &TerminalContrastOverlays::default(),
        &mut TerminalContrastCache::default(),
    );
    assert_eq!(
        underline_ranges_as_tuples(&decorations.strikethroughs),
        vec![(palette.terminal_fg, 0, 4)]
    );
    let mut text = String::new();
    let mut glyphs = Vec::new();
    let mut col = 0;
    for span in &spans {
        let len = append_terminal_glyph_span(
            &mut text,
            span,
            &mut col,
            &mut glyphs,
            Some(&row.cells),
            palette.terminal_fg,
        );
        let run = terminal_text_run_for_span(span, len, font("Consolas"), 400., 700., palette);
        assert!(run.strikethrough.is_none());
    }
    assert_eq!(text, "    ");
    assert_eq!(glyphs.len(), 4);
}

#[test]
fn cell_graphics_with_attached_marks_preserve_font_base_and_cursor() {
    let palette = nyaterm_ui::theme_palette("github-dark");
    let mut screen = TerminalScreen::new(24, 2);
    screen.advance("█\u{301}▀\u{fe0f}─中X█".as_bytes());
    let snapshot = screen.snapshot();
    let row = snapshot.row(0).unwrap();
    let mut span = highlight_span(&row.text, Some(0x123456), None, false);
    let mut text = String::new();
    let mut glyphs = Vec::new();
    let mut col = 0;
    append_terminal_glyph_span(
        &mut text,
        &span,
        &mut col,
        &mut glyphs,
        Some(&row.cells),
        palette.terminal_fg,
    );
    assert_eq!(text, "█\u{301}▀\u{fe0f} 中 X ");
    assert_eq!(
        glyphs.iter().map(|glyph| glyph.col).collect::<Vec<_>>(),
        vec![2, 6]
    );
    assert_eq!(col, 7);
    assert_eq!(super::cursor_cell_glyph("█"), Some('█'));
    assert_eq!(super::cursor_cell_glyph("█\u{301}"), None);
    // A span split before the combining mark must still preserve the base.
    text.clear();
    glyphs.clear();
    col = 0;
    span.text = "█".into();
    append_terminal_glyph_span(
        &mut text,
        &span,
        &mut col,
        &mut glyphs,
        Some(&row.cells),
        palette.terminal_fg,
    );
    assert_eq!(text, "█");
    assert!(glyphs.is_empty());
}

#[test]
fn underline_ranges_match_flatten_reference_for_wide_and_combining_text() {
    let palette = nyaterm_ui::theme_palette("github-dark");
    let spans = vec![
        TerminalHighlightSpan {
            text: "a界".to_string(),
            color: Some(0xff2244),
            bg: None,
            keyword: true,
            underline: true,
            strikeout: false,
            bold: false,
            italic: false,
            hidden: false,
        },
        TerminalHighlightSpan {
            text: "e\u{301}".to_string(),
            color: Some(0xff2244),
            bg: None,
            keyword: true,
            underline: true,
            strikeout: false,
            bold: false,
            italic: false,
            hidden: false,
        },
        TerminalHighlightSpan {
            text: "x".to_string(),
            color: None,
            bg: None,
            keyword: false,
            underline: false,
            strikeout: false,
            bold: false,
            italic: false,
            hidden: false,
        },
    ];

    let ranges = terminal_underline_ranges_for_spans(&spans, palette);
    let reference = underline_ranges_from_flatten_reference(&spans, palette);

    assert_eq!(underline_ranges_as_tuples(&ranges), reference);
}

fn underline_ranges_from_flatten_reference(
    spans: &[TerminalHighlightSpan],
    palette: nyaterm_ui::ThemePalette,
) -> Vec<(u32, usize, usize)> {
    let flat = flatten_highlight_spans(spans.to_vec());
    let mut out = Vec::new();
    let mut pending: Option<(u32, usize, usize)> = None;
    for (col, cell) in flat.iter().enumerate() {
        if cell.underline {
            let color = cell.color.unwrap_or(palette.accent);
            match pending.as_mut() {
                Some((pending_color, _, end)) if *pending_color == color && *end == col => {
                    *end = col + 1;
                }
                _ => {
                    if let Some(range) = pending.take() {
                        out.push(range);
                    }
                    pending = Some((color, col, col + 1));
                }
            }
        } else if let Some(range) = pending.take() {
            out.push(range);
        }
    }
    if let Some(range) = pending {
        out.push(range);
    }
    out
}

fn underline_ranges_as_tuples(ranges: &[TerminalRowDecorationRange]) -> Vec<(u32, usize, usize)> {
    ranges
        .iter()
        .map(|range| (range.color, range.start, range.end))
        .collect()
}

#[test]
fn layout_cache_stats_report_local_deltas_without_resetting_cache() {
    let mut cache = NyaTerminalLayoutCache::default();
    let before = cache.stats();

    let _ = cache.shaped_line(0, 42, || {
        (Arc::new(ShapedLine::default()), std::time::Duration::ZERO)
    });
    let shaped = cache.stats();

    assert_eq!(
        shaped.delta_since(before),
        super::NyaTerminalLayoutCacheStats {
            hits: 0,
            misses: 1,
            shape_calls: 1,
            shape_duration_us: 0,
        }
    );

    let _ = cache.shaped_line(7, 42, || panic!("matching key should reuse the cache"));
    assert_eq!(cache.stats().delta_since(shaped).hits, 1);
    assert_eq!(cache.stats().delta_since(shaped).shape_calls, 0);
}

#[test]
fn shaped_line_cache_reuses_matching_row_key() {
    let mut cache = NyaTerminalLayoutCache::default();

    let _ = cache.shaped_line(0, 42, || {
        (Arc::new(ShapedLine::default()), std::time::Duration::ZERO)
    });
    let _ = cache.shaped_line(7, 42, || {
        panic!("matching key should reuse cached row even at another viewport row")
    });

    assert_eq!(cache.misses, 1);
    assert_eq!(cache.hits, 1);
}

#[test]
fn clear_resets_shaped_line_cache() {
    let mut cache = NyaTerminalLayoutCache::default();
    let _ = cache.shaped_line(0, 42, || {
        (Arc::new(ShapedLine::default()), std::time::Duration::ZERO)
    });

    cache.clear();

    assert_eq!(cache.misses, 0);
    assert_eq!(cache.hits, 0);
    let _ = cache.shaped_line(0, 42, || {
        (Arc::new(ShapedLine::default()), std::time::Duration::ZERO)
    });
    assert_eq!(cache.misses, 1);
    assert_eq!(cache.row_order.len(), 1);
}

#[test]
fn cursor_glyph_cache_reuses_matching_layout() {
    let mut cache = NyaTerminalLayoutCache::default();

    let (first, did_shape, _) = cache.cursor_glyph(42, || {
        (Arc::new(ShapedLine::default()), std::time::Duration::ZERO)
    });
    assert!(did_shape);

    let (second, did_shape, _) = cache.cursor_glyph(42, || {
        panic!("matching cursor glyph should reuse its shaped layout")
    });
    assert!(!did_shape);
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(cache.misses, 1);
    assert_eq!(cache.hits, 1);
    assert_eq!(cache.shape_calls, 1);
}

#[test]
fn keyword_rules_key_reuses_equal_rule_sets() {
    let mut cache = NyaTerminalLayoutCache::default();
    let first = Arc::new(vec![ResolvedKeywordHighlightRule {
        id: "error".to_string(),
        name: "Error".to_string(),
        patterns: vec!["error".to_string()],
        color: "#ff0000".to_string(),
        enabled: true,
    }]);
    let second = Arc::new(vec![ResolvedKeywordHighlightRule {
        id: "error".to_string(),
        name: "Error".to_string(),
        patterns: vec!["error".to_string()],
        color: "#ff0000".to_string(),
        enabled: true,
    }]);

    let key = cache.keyword_rules_key(&first);
    assert_eq!(cache.keyword_rules_key(&second), key);
    assert!(
        cache
            .keyword_rules_source
            .as_ref()
            .is_some_and(|cached| Arc::ptr_eq(cached, &second))
    );
}

#[test]
fn shaped_line_cache_misses_on_style_key_change() {
    let mut cache = NyaTerminalLayoutCache::default();

    let _ = cache.shaped_line(0, 42, || {
        (Arc::new(ShapedLine::default()), std::time::Duration::ZERO)
    });
    let _ = cache.shaped_line(0, 43, || {
        (Arc::new(ShapedLine::default()), std::time::Duration::ZERO)
    });

    assert_eq!(cache.misses, 2);
    assert_eq!(cache.hits, 0);
}

#[test]
fn row_cache_evicts_incrementally_when_full() {
    let mut cache = NyaTerminalLayoutCache::default();
    for key in 0..=TERMINAL_LAYOUT_CACHE_ROW_CAP as u64 {
        let _ = cache.paint_row(0, key, || {
            (
                Arc::new(ShapedLine::default()),
                std::time::Duration::ZERO,
                1,
                Vec::new(),
                TerminalRowTextDecorations::default(),
                Vec::new(),
            )
        });
    }

    assert_eq!(cache.rows.len(), TERMINAL_LAYOUT_CACHE_ROW_CAP);
    assert!(!cache.rows.contains_key(&0));
    assert!(cache.rows.contains_key(&1));
    assert!(
        cache
            .rows
            .contains_key(&(TERMINAL_LAYOUT_CACHE_ROW_CAP as u64))
    );

    let _ = cache.paint_row(0, 1, || {
        panic!("remaining rows should survive cache pressure");
    });
    assert_eq!(cache.hits, 1);
}

#[test]
fn styled_span_hash_tracks_style_changes() {
    let plain = vec![nyaterm_terminal::StyledSpan {
        text: "same".to_string(),
        style: nyaterm_terminal::CellStyle::default(),
    }];
    let bold_style = nyaterm_terminal::CellStyle {
        bold: true,
        ..Default::default()
    };
    let bold = vec![nyaterm_terminal::StyledSpan {
        text: "same".to_string(),
        style: bold_style,
    }];

    let mut plain_hasher = DefaultHasher::new();
    hash_styled_spans(Some(&plain), &mut plain_hasher);
    let mut bold_hasher = DefaultHasher::new();
    hash_styled_spans(Some(&bold), &mut bold_hasher);

    assert_ne!(plain_hasher.finish(), bold_hasher.finish());
}

#[test]
fn row_layout_key_falls_back_to_styled_spans_without_signature() {
    let decorations = TerminalLineDecorations::default();
    let plain = vec![nyaterm_terminal::StyledSpan {
        text: "same".to_string(),
        style: nyaterm_terminal::CellStyle::default(),
    }];
    let bold_style = nyaterm_terminal::CellStyle {
        bold: true,
        ..Default::default()
    };
    let bold = vec![nyaterm_terminal::StyledSpan {
        text: "same".to_string(),
        style: bold_style,
    }];

    assert_ne!(
        terminal_row_layout_key(None, "same", Some(&plain), &decorations, &[], false, 0),
        terminal_row_layout_key(None, "same", Some(&bold), &decorations, &[], false, 0)
    );
}

#[test]
fn row_layout_key_uses_authoritative_row_revision() {
    let mut first_snapshot = TerminalScreen::default().snapshot();
    edit_snapshot_row(&mut first_snapshot, 0, |row| row.revision = 7);
    let mut second_snapshot = first_snapshot.clone();
    edit_snapshot_row(&mut second_snapshot, 0, |row| row.revision = 8);
    let make_element = |snapshot| {
        NyaTerminalElement::new(
            Arc::new(snapshot),
            Arc::new(Vec::new()),
            Vec::new(),
            false,
            "block",
            8.0,
            16.0,
            nyaterm_ui::theme_palette("github-dark"),
            "monospace".to_string(),
            14.0,
            400.0,
            700.0,
        )
    };
    let first = make_element(first_snapshot);
    let second = make_element(second_snapshot);
    let decorations = TerminalLineDecorations::default();

    assert_ne!(
        first.row_layout_key(0, "same", None, &decorations),
        second.row_layout_key(0, "same", None, &decorations),
    );
}

#[test]
fn row_layout_key_uses_revision_instead_of_hashing_display_text() {
    let mut snapshot = TerminalScreen::default().snapshot();
    edit_snapshot_row(&mut snapshot, 0, |row| row.revision = 7);
    let element = NyaTerminalElement::new(
        Arc::new(snapshot),
        Arc::new(Vec::new()),
        Vec::new(),
        false,
        "block",
        8.0,
        16.0,
        nyaterm_ui::theme_palette("github-dark"),
        "monospace".to_string(),
        14.0,
        400.0,
        700.0,
    );
    let decorations = TerminalLineDecorations::default();

    assert_eq!(
        element.row_layout_key(
            0,
            "Management: https://landscape.canonical.com",
            None,
            &decorations
        ),
        element.row_layout_key(0, "Users logged in:", None, &decorations),
    );
}

#[test]
fn row_layout_key_tracks_cell_width() {
    let mut snapshot = TerminalScreen::default().snapshot();
    edit_snapshot_row(&mut snapshot, 0, |row| {
        row.text = "same".to_string();
        row.signature = 7;
    });
    let make_element = |cell_width| {
        NyaTerminalElement::new(
            Arc::new(snapshot.clone()),
            Arc::new(Vec::new()),
            Vec::new(),
            false,
            "block",
            cell_width,
            16.0,
            nyaterm_ui::theme_palette("github-dark"),
            "monospace".to_string(),
            14.0,
            400.0,
            700.0,
        )
    };
    let narrow = make_element(8.0);
    let wide = make_element(12.0);
    let decorations = TerminalLineDecorations::default();

    assert_ne!(
        narrow.row_layout_key(0, "same", None, &decorations),
        wide.row_layout_key(0, "same", None, &decorations),
    );
}

#[test]
fn row_layout_key_ignores_dynamic_overlay_decorations() {
    let mut snapshot = TerminalScreen::default().snapshot();
    edit_snapshot_row(&mut snapshot, 0, |row| {
        row.text = "same".to_string();
        row.signature = 7;
    });
    let element = NyaTerminalElement::new(
        Arc::new(snapshot),
        Arc::new(Vec::new()),
        Vec::new(),
        false,
        "block",
        8.0,
        16.0,
        nyaterm_ui::theme_palette("github-dark"),
        "monospace".to_string(),
        14.0,
        400.0,
        700.0,
    );
    let base = TerminalLineDecorations::default();
    let dynamic = TerminalLineDecorations {
        search_ranges: vec![(0, 2)],
        ..TerminalLineDecorations::default()
    };

    assert_eq!(
        element.row_layout_key(0, "same", None, &base),
        element.row_layout_key(0, "same", None, &dynamic)
    );
}

#[test]
fn zebra_state_does_not_invalidate_row_shaping() {
    let mut plain_snapshot = TerminalScreen::default().snapshot();
    edit_snapshot_row(&mut plain_snapshot, 0, |row| {
        row.text = "same".to_string();
        row.signature = 7;
    });
    let mut striped_snapshot = plain_snapshot.clone();
    edit_snapshot_row(&mut striped_snapshot, 0, |row| {
        row.shell_input = Some(ShellInputLineKind::Active);
    });
    let make_element = |snapshot| {
        NyaTerminalElement::new(
            Arc::new(snapshot),
            Arc::new(Vec::new()),
            Vec::new(),
            false,
            "block",
            8.0,
            16.0,
            nyaterm_ui::theme_palette("github-dark"),
            "monospace".to_string(),
            14.0,
            400.0,
            700.0,
        )
    };
    let plain = make_element(plain_snapshot);
    let striped = make_element(striped_snapshot);
    let decorations = TerminalLineDecorations::default();
    let plain_key = plain.row_layout_key(0, "same", None, &decorations);
    let striped_key = striped.row_layout_key(0, "same", None, &decorations);
    assert_eq!(plain_key, striped_key);

    let mut cache = NyaTerminalLayoutCache::default();
    let (_, did_shape, _) = cache.paint_row(0, plain_key, || {
        (
            Arc::new(ShapedLine::default()),
            std::time::Duration::ZERO,
            1,
            Vec::new(),
            TerminalRowTextDecorations::default(),
            Vec::new(),
        )
    });
    assert!(did_shape);
    let (_, did_shape, _) = cache.paint_row(0, striped_key, || {
        panic!("zebra-only changes must reuse the shaped row")
    });
    assert!(!did_shape);
    assert_eq!(cache.shape_calls, 1);
}

#[test]
fn zebra_stripes_merge_input_rows_and_target_row_takes_priority() {
    let mut snapshot = TerminalScreen::new(8, 4).viewport_snapshot(0);
    for row_index in 0..3 {
        edit_snapshot_row(&mut snapshot, row_index, |row| {
            row.shell_input = Some(ShellInputLineKind::Submitted);
        });
    }
    let target_line = snapshot.rows()[1].line_id.expect("stable line id");
    let palette = nyaterm_ui::theme_palette("github-dark");
    let geometry = TerminalPaintGeometry {
        bounds: Bounds::new(point(px(0.), px(0.)), size(px(64.), px(64.))),
        visual_y_offset: 0.0,
        cell_width: 8.0,
        cell_height: 16.0,
    };
    let mut stripes = Vec::new();

    push_terminal_zebra_stripes(&snapshot, 0..3, None, palette, geometry, &mut stripes);
    assert_eq!(stripes.len(), 1);
    assert_eq!(stripes[0].bounds.size.height, px(48.0));
    assert_eq!(
        stripes[0].background,
        gpui::rgba((palette.terminal_fg << 8) | 0x0f).into()
    );

    stripes.clear();
    push_terminal_zebra_stripes(
        &snapshot,
        0..3,
        Some(target_line),
        palette,
        geometry,
        &mut stripes,
    );
    assert_eq!(stripes.len(), 3);
    assert_eq!(
        stripes[1].background,
        gpui::rgb(palette.terminal_line_highlight).into()
    );
}

#[test]
fn row_layout_key_tracks_active_search_glyph_decorations() {
    let mut snapshot = TerminalScreen::default().snapshot();
    edit_snapshot_row(&mut snapshot, 0, |row| {
        row.text = "same".to_string();
        row.signature = 7;
    });
    let element = NyaTerminalElement::new(
        Arc::new(snapshot),
        Arc::new(Vec::new()),
        Vec::new(),
        false,
        "block",
        8.0,
        16.0,
        nyaterm_ui::theme_palette("github-dark"),
        "monospace".to_string(),
        14.0,
        400.0,
        700.0,
    );
    let base = TerminalLineDecorations::default();
    let active = TerminalLineDecorations {
        active_search_ranges: vec![(0, 2)],
        ..TerminalLineDecorations::default()
    };

    assert_ne!(
        element.row_layout_key(0, "same", None, &base),
        element.row_layout_key(0, "same", None, &active)
    );
}

#[test]
fn paint_row_cache_reuses_full_row_payload() {
    let mut cache = NyaTerminalLayoutCache::default();
    let mut build_calls = 0usize;

    let (row, did_shape, duration) = cache.paint_row(0, 42, || {
        build_calls += 1;
        (
            Arc::new(ShapedLine::default()),
            std::time::Duration::ZERO,
            3,
            vec![TerminalRowBackgroundRange {
                bg: 0xff00ff,
                start: 2,
                end: 4,
            }],
            TerminalRowTextDecorations {
                underlines: vec![TerminalRowDecorationRange {
                    color: 0x00ffff,
                    start: 1,
                    end: 3,
                }],
                strikethroughs: vec![TerminalRowDecorationRange {
                    color: 0xabcdef,
                    start: 2,
                    end: 3,
                }],
            },
            vec![CellGlyph {
                col: 2,
                ch: '█',
                color: 0xabcdef,
            }],
        )
    });

    assert!(did_shape);
    assert_eq!(duration, std::time::Duration::ZERO);
    assert_eq!(build_calls, 1);
    assert_eq!(row.text_run_count, 3);
    assert_eq!(row.background_ranges.len(), 1);
    assert_eq!(row.text_decorations.underlines.len(), 1);

    let (cached, did_shape, duration) = cache.paint_row(0, 42, || {
        panic!("cached row should not rebuild");
    });

    assert!(!did_shape);
    assert_eq!(duration, std::time::Duration::ZERO);
    assert_eq!(build_calls, 1);
    assert_eq!(cached.text_run_count, 3);
    assert_eq!(cached.background_ranges[0].bg, 0xff00ff);
    assert_eq!(cached.background_ranges[0].start, 2);
    assert_eq!(cached.background_ranges[0].end, 4);
    assert_eq!(cached.text_decorations.underlines[0].color, 0x00ffff);
    assert_eq!(cached.text_decorations.underlines[0].start, 1);
    assert_eq!(cached.text_decorations.underlines[0].end, 3);
    assert_eq!(
        underline_ranges_as_tuples(&cached.text_decorations.strikethroughs),
        vec![(0xabcdef, 2, 3)]
    );
    assert_eq!(
        cached.cell_glyphs,
        vec![CellGlyph {
            col: 2,
            ch: '█',
            color: 0xabcdef
        }]
    );
}

#[test]
fn cell_backgrounds_cover_trailing_blanks_wide_spacers_and_reverse_video() {
    let palette = nyaterm_ui::theme_palette("github-dark");
    let mut screen = TerminalScreen::new(24, 3);
    screen.advance("\x1b[41m中TEXT\x1b[44m        \x1b[0m\r\n\x1b[7m    \x1b[0m".as_bytes());
    let snapshot = screen.snapshot();
    let row = snapshot.row(0).unwrap();
    assert_eq!(row.text, "中TEXT");
    let ranges = terminal_background_ranges_for_cells(&row.cells, palette, false);
    assert_eq!(ranges.len(), 2);
    assert_eq!((ranges[0].start, ranges[0].end), (0, 6));
    assert_eq!((ranges[1].start, ranges[1].end), (6, 14));
    assert_eq!(ranges[1].bg, palette.terminal_ansi_color(4));
    let ranges =
        terminal_background_ranges_for_cells(&snapshot.row(1).unwrap().cells, palette, false);
    assert_eq!(
        (ranges[0].start, ranges[0].end, ranges[0].bg),
        (0, 4, palette.terminal_fg)
    );
}

#[test]
fn reverse_backgrounds_cover_spaces_wide_spacers_and_adjacent_color_ranges() {
    let palette = nyaterm_ui::theme_palette("github-dark");
    for alternate in [false, true] {
        let mut screen = TerminalScreen::new(16, 2);
        if alternate {
            screen.advance(b"\x1b[?1049h");
        }
        screen.advance("\x1b[31;44mN\x1b[7m  中  \x1b[32m  \x1b[27mN\x1b[0m".as_bytes());
        let snapshot = screen.snapshot();
        let row = snapshot.row(0).unwrap();
        assert_eq!(row.cells[4].width, 0);
        let ranges = terminal_background_ranges_for_cells(&row.cells, palette, false);
        let actual: Vec<_> = ranges.iter().map(|r| (r.start, r.end, r.bg)).collect();
        assert_eq!(
            actual,
            vec![
                (0, 1, palette.terminal_ansi_color(4)),
                (1, 7, palette.terminal_ansi_color(1)),
                (7, 9, palette.terminal_ansi_color(2)),
                (9, 10, palette.terminal_ansi_color(4)),
            ]
        );
        let spans =
            crate::ansi::ansi_to_highlight_spans_compiled(&row.styled_spans, palette, &[], false);
        let span_ranges = terminal_background_ranges_for_spans(&spans);
        let from_spans: Vec<_> = span_ranges.iter().map(|r| (r.start, r.end, r.bg)).collect();
        assert_eq!(from_spans, actual);

        screen.advance(b"\r\x1b[0;31;44;7m                \x1b[0m");
        let snapshot = screen.snapshot();
        let row = snapshot.row(0).unwrap();
        assert!(row.text.is_empty());
        let ranges = terminal_background_ranges_for_cells(&row.cells, palette, false);
        assert_eq!(ranges.len(), 1);
        assert_eq!(
            (ranges[0].start, ranges[0].end, ranges[0].bg),
            (0, 16, palette.terminal_ansi_color(1))
        );
    }
}

#[test]
fn reverse_button_updates_invalidate_snapshot_and_layout_cache() {
    let palette = nyaterm_ui::theme_palette("github-dark");
    let mut screen = TerminalScreen::new(16, 2);
    let mut cache = NyaTerminalLayoutCache::default();
    let mut previous: Option<TerminalSnapshot> = None;
    let decorations = TerminalLineDecorations::default();
    for reverse in [false, true, false, true, false] {
        screen.advance(if reverse {
            b"\r\x1b[0;31;44;7mSELECTED"
        } else {
            b"\r\x1b[0;31;44mSELECTED"
        });
        let snapshot = screen.snapshot();
        let row = snapshot.row(0).unwrap();
        let key = terminal_row_layout_key(
            Some(row.revision),
            &row.text,
            Some(&row.styled_spans),
            &decorations,
            &[],
            false,
            0,
        );
        if let Some(before) = &previous {
            let before = before.row(0).unwrap();
            assert_eq!(before.text, row.text);
            assert_ne!(before.revision, row.revision);
            assert_ne!(before.signature, row.signature);
            assert_ne!(
                terminal_row_layout_key(
                    None,
                    &before.text,
                    Some(&before.styled_spans),
                    &decorations,
                    &[],
                    false,
                    0
                ),
                terminal_row_layout_key(
                    None,
                    &row.text,
                    Some(&row.styled_spans),
                    &decorations,
                    &[],
                    false,
                    0
                ),
            );
        }
        let (painted, _, _) = cache.paint_row(0, key, || {
            (
                Arc::new(ShapedLine::default()),
                std::time::Duration::ZERO,
                1,
                terminal_background_ranges_for_cells(&row.cells, palette, false),
                TerminalRowTextDecorations::default(),
                Vec::new(),
            )
        });
        assert_eq!(
            painted.background_ranges[0].bg,
            palette.terminal_ansi_color(if reverse { 1 } else { 4 })
        );
        assert_eq!(row.cells[0].style.reverse, reverse);
        previous = Some(snapshot);
    }
}

#[test]
fn custom_glyphs_preserve_columns_after_wide_text_and_combining_marks() {
    let mut span = highlight_span("中e\u{301}█▀A", Some(0x123456), None, false);
    span.bold = true;
    span.italic = true;
    let mut text = String::new();
    let mut col = 0;
    let mut glyphs = Vec::new();
    let len = append_terminal_glyph_span(&mut text, &span, &mut col, &mut glyphs, None, 0);
    assert_eq!(text, "中 e\u{301}  A");
    assert_eq!(len, text.len());
    assert_eq!(col, 6);
    assert_eq!(
        glyphs,
        vec![
            CellGlyph {
                col: 3,
                ch: '█',
                color: 0x123456
            },
            CellGlyph {
                col: 4,
                ch: '▀',
                color: 0x123456
            }
        ]
    );
}

#[test]
fn hidden_custom_glyphs_do_not_paint_over_selection() {
    let mut screen = TerminalScreen::new(8, 2);
    screen.advance("\x1b[8m█\x1b[0m▀".as_bytes());
    let snapshot = screen.snapshot();
    let mut text = String::new();
    let mut col = 0;
    let mut glyphs = Vec::new();
    let span = highlight_span("█▀", Some(0x123456), None, false);
    append_terminal_glyph_span(
        &mut text,
        &span,
        &mut col,
        &mut glyphs,
        Some(&snapshot.row(0).unwrap().cells),
        0,
    );
    assert_eq!(text, "  ");
    assert_eq!(
        glyphs,
        vec![CellGlyph {
            col: 1,
            ch: '▀',
            color: 0x123456
        }]
    );
}

#[test]
fn paint_row_cache_promotes_equivalent_keyword_result() {
    let mut cache = NyaTerminalLayoutCache::default();

    let (pending, did_shape, _) = cache.paint_row(0, 41, || {
        (
            Arc::new(ShapedLine::default()),
            std::time::Duration::ZERO,
            1,
            Vec::new(),
            TerminalRowTextDecorations::default(),
            Vec::new(),
        )
    });
    assert!(did_shape);

    let (parsed, did_shape, _) = cache.paint_row_reusing(0, 42, Some(41), || {
        panic!("equivalent parsed keyword result should reuse pending layout")
    });

    assert!(!did_shape);
    assert!(Arc::ptr_eq(&pending, &parsed));
    assert!(!cache.rows.contains_key(&41));
    assert!(cache.rows.contains_key(&42));
    assert_eq!(cache.misses, 1);
    assert_eq!(cache.hits, 1);
    assert_eq!(cache.shape_calls, 1);
}

#[test]
fn paint_row_cache_promotion_does_not_accumulate_stale_order_keys() {
    let mut cache = NyaTerminalLayoutCache::default();
    cache.paint_row(0, 1, || {
        (
            Arc::new(ShapedLine::default()),
            std::time::Duration::ZERO,
            1,
            Vec::new(),
            TerminalRowTextDecorations::default(),
            Vec::new(),
        )
    });

    for key in 2..=5_000 {
        cache.paint_row_reusing(0, key, Some(key - 1), || {
            panic!("equivalent promotion should reuse the previous row")
        });
    }

    assert_eq!(cache.rows.len(), 1);
    assert!(cache.row_order.len() <= TERMINAL_LAYOUT_CACHE_ROW_ORDER_MIN_COMPACT);
    assert_eq!(cache.row_order.back(), Some(&5_000));
}

fn highlight_span(
    text: &str,
    color: Option<u32>,
    bg: Option<u32>,
    keyword: bool,
) -> TerminalHighlightSpan {
    TerminalHighlightSpan {
        text: text.to_string(),
        color,
        bg,
        keyword,
        underline: false,
        strikeout: false,
        bold: false,
        italic: false,
        hidden: false,
    }
}

#[test]
fn keyword_spans_do_not_create_background_ranges() {
    let palette = nyaterm_ui::theme_palette("github-dark");
    let span = highlight_span("ERROR", Some(0xff2244), None, true);
    let ranges = terminal_background_ranges_for_spans(std::slice::from_ref(&span));
    let run = terminal_text_run_for_span(
        &span,
        span.text.len(),
        font(SharedString::from("monospace")),
        400.0,
        700.0,
        palette,
    );

    assert!(ranges.is_empty());
    assert_eq!(run.color, rgb(0xff2244).into());
}

#[test]
fn explicit_background_ranges_survive_keyword_foreground() {
    let span = highlight_span("WARN", Some(0xffcc00), Some(0x112233), true);
    let ranges = terminal_background_ranges_for_spans(std::slice::from_ref(&span));

    assert_eq!(ranges.len(), 1);
    assert_eq!(ranges[0].bg, 0x112233);
    assert_eq!(ranges[0].start, 0);
    assert_eq!(ranges[0].end, 4);
}

#[test]
fn cached_keyword_rows_reuse_without_surface_background() {
    let mut cache = NyaTerminalLayoutCache::default();
    let span = highlight_span("ERROR", Some(0xff2244), None, true);
    let background_ranges = terminal_background_ranges_for_spans(std::slice::from_ref(&span));

    let (row, did_shape, _) = cache.paint_row(0, 42, || {
        (
            Arc::new(ShapedLine::default()),
            std::time::Duration::ZERO,
            1,
            background_ranges,
            TerminalRowTextDecorations::default(),
            Vec::new(),
        )
    });
    assert!(did_shape);
    assert!(row.background_ranges.is_empty());

    let (cached, did_shape, _) = cache.paint_row(0, 42, || {
        panic!("cached keyword row should reuse without rebuilding")
    });
    assert!(!did_shape);
    assert!(cached.background_ranges.is_empty());
}

fn underline_span(text: &str, color: Option<u32>) -> TerminalHighlightSpan {
    TerminalHighlightSpan {
        text: text.to_string(),
        color,
        bg: None,
        keyword: false,
        underline: true,
        strikeout: false,
        bold: false,
        italic: false,
        hidden: false,
    }
}

#[test]
fn underline_ranges_capture_ansi_underlines_without_text_run_underlines() {
    let palette = nyaterm_ui::theme_palette("github-dark");
    let span = underline_span("ERROR", Some(0xff2244));
    let ranges = terminal_underline_ranges_for_spans(std::slice::from_ref(&span), palette);
    let run = terminal_text_run_for_span(
        &span,
        span.text.len(),
        font(SharedString::from("monospace")),
        400.0,
        700.0,
        palette,
    );

    assert_eq!(ranges.len(), 1);
    assert_eq!(ranges[0].start, 0);
    assert_eq!(ranges[0].end, 5);
    assert_eq!(ranges[0].color, 0xff2244);
    assert!(run.underline.is_none());
}

#[test]
fn underline_ranges_clamp_action_links_to_actual_text() {
    let palette = nyaterm_ui::theme_palette("github-dark");
    let spans = apply_action_link_ranges(
        vec![TerminalHighlightSpan {
            text: "abc".to_string(),
            color: None,
            bg: None,
            keyword: false,
            underline: false,
            strikeout: false,
            bold: false,
            italic: false,
            hidden: false,
        }],
        &[(1, 99)],
        palette,
    );
    let ranges = terminal_underline_ranges_for_spans(&spans, palette);

    assert_eq!(ranges.len(), 1);
    assert_eq!(ranges[0].start, 1);
    assert_eq!(ranges[0].end, 3);
    assert_eq!(ranges[0].color, palette.accent);
}

#[test]
fn pad_wide_cells_gives_every_column_a_glyph() {
    assert_eq!(pad_wide_cells("ab"), "ab");
    assert_eq!(pad_wide_cells("a\u{4f60}b"), "a\u{4f60} b");
    assert_eq!(pad_wide_cells("\u{4f60}\u{597d}"), "\u{4f60} \u{597d} ");
}

#[test]
fn pad_wide_cells_leaves_attached_marks_alone() {
    // A combining mark belongs to the cell before it and takes none of its
    // own, so padding it would push the rest of the row sideways.
    assert_eq!(pad_wide_cells("e\u{0301}"), "e\u{0301}");
}

#[test]
fn pad_wide_cells_pads_vs16_emoji_to_two_cells() {
    // U+2139 U+FE0F renders two cells wide; without the padding the next
    // glyph is drawn on top of it (issue #946).
    assert_eq!(pad_wide_cells("\u{2139}\u{fe0f}x"), "\u{2139}\u{fe0f} x");
    assert_eq!(pad_wide_cells("\u{2764}\u{fe0f}"), "\u{2764}\u{fe0f} ");
    // Plain letters with VS16 stay one cell.
    assert_eq!(pad_wide_cells("a\u{fe0f}x"), "a\u{fe0f}x");
}

#[test]
fn underline_ranges_count_wide_and_attached_marks_as_terminal_cells() {
    let palette = nyaterm_ui::theme_palette("github-dark");
    let wide =
        terminal_underline_ranges_for_spans(&[underline_span("界x", Some(0xff2244))], palette);
    let combining = terminal_underline_ranges_for_spans(
        &[underline_span("e\u{301}x", Some(0xff2244))],
        palette,
    );
    let variation = terminal_underline_ranges_for_spans(
        &[underline_span("a\u{fe0f}x", Some(0xff2244))],
        palette,
    );

    assert_eq!(wide[0].start, 0);
    assert_eq!(wide[0].end, 3);
    assert_eq!(combining[0].start, 0);
    assert_eq!(combining[0].end, 2);
    assert_eq!(variation[0].start, 0);
    assert_eq!(variation[0].end, 2);
}

#[test]
fn underline_bounds_follow_visual_scroll_offset() {
    let bounds = Bounds::new(point(px(10.0), px(20.0)), size(px(200.0), px(120.0)));

    let base = terminal_underline_bounds(
        2,
        1,
        4,
        TerminalPaintGeometry {
            bounds,
            visual_y_offset: 0.0,
            cell_width: 8.0,
            cell_height: 16.0,
        },
    );
    let shifted = terminal_underline_bounds(
        2,
        1,
        4,
        TerminalPaintGeometry {
            bounds,
            visual_y_offset: -8.0,
            cell_width: 8.0,
            cell_height: 16.0,
        },
    );

    assert_eq!(f32::from(base.left()), 18.0);
    assert_eq!(f32::from(base.top()), 66.0);
    assert_eq!(f32::from(base.size.width), 24.0);
    assert_eq!(f32::from(shifted.top()), 58.0);
}

#[test]
fn dynamic_link_underlines_follow_visual_offset_and_clamp_to_text() {
    let palette = nyaterm_ui::theme_palette("github-dark");
    let bounds = Bounds::new(point(px(10.0), px(20.0)), size(px(200.0), px(120.0)));
    let decorations = TerminalLineDecorations {
        link_ranges: vec![(1, 99)],
        ..TerminalLineDecorations::default()
    };
    let mut out = Vec::new();

    push_dynamic_link_underlines(
        2,
        "abc",
        &decorations,
        palette,
        TerminalPaintGeometry {
            bounds,
            visual_y_offset: -8.0,
            cell_width: 8.0,
            cell_height: 16.0,
        },
        &mut out,
    );

    assert_eq!(out.len(), 1);
    assert_eq!(f32::from(out[0].bounds.left()), 18.0);
    assert_eq!(f32::from(out[0].bounds.top()), 58.0);
    assert_eq!(f32::from(out[0].bounds.size.width), 16.0);
    assert_eq!(terminal_link_underline_color(palette), palette.text_muted);
    assert_ne!(terminal_link_underline_color(palette), palette.accent);

    out.clear();
    push_dynamic_link_underlines(
        0,
        "",
        &decorations,
        palette,
        TerminalPaintGeometry {
            bounds,
            visual_y_offset: 0.0,
            cell_width: 8.0,
            cell_height: 16.0,
        },
        &mut out,
    );
    assert!(out.is_empty());
}

#[test]
fn dynamic_link_underline_count_is_stable_across_selection_changes() {
    let palette = nyaterm_ui::theme_palette("github-dark");
    let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(160.0), px(32.0)));
    let geometry = TerminalPaintGeometry {
        bounds,
        visual_y_offset: 0.0,
        cell_width: 8.0,
        cell_height: 16.0,
    };
    let decorations = TerminalLineDecorations {
        link_ranges: vec![(1, 4)],
        ..TerminalLineDecorations::default()
    };
    let underline_count = |decorations: &TerminalLineDecorations| {
        let mut underlines = Vec::new();
        push_dynamic_link_underlines(
            0,
            "address",
            decorations,
            palette,
            geometry,
            &mut underlines,
        );
        underlines.len()
    };

    assert_eq!(underline_count(&decorations), 1);
}

#[test]
fn row_layout_key_ignores_dynamic_link_underlines() {
    let mut snapshot = TerminalScreen::default().snapshot();
    edit_snapshot_row(&mut snapshot, 0, |row| {
        row.text = "same".to_string();
        row.signature = 7;
    });
    let element = NyaTerminalElement::new(
        Arc::new(snapshot),
        Arc::new(Vec::new()),
        Vec::new(),
        false,
        "block",
        8.0,
        16.0,
        nyaterm_ui::theme_palette("github-dark"),
        "monospace".to_string(),
        14.0,
        400.0,
        700.0,
    );
    let base = TerminalLineDecorations::default();
    let linked = TerminalLineDecorations {
        link_ranges: vec![(0, 2)],
        ..TerminalLineDecorations::default()
    };

    assert_eq!(
        element.row_layout_key(0, "same", None, &base),
        element.row_layout_key(0, "same", None, &linked)
    );
}

#[test]
fn row_layout_key_ignores_link_ranges_when_keyword_rules_can_paint() {
    let mut snapshot = TerminalScreen::default().snapshot();
    edit_snapshot_row(&mut snapshot, 0, |row| {
        row.text = "https://help.ubuntu.com".to_string();
        row.signature = 7;
    });
    let keyword_rule = ResolvedKeywordHighlightRule {
        id: "url".to_string(),
        name: "URL".to_string(),
        patterns: vec![r"https://[^\s]+".to_string()],
        color: "#8be9fd".to_string(),
        enabled: true,
    };
    let keyword_rules = Arc::new(vec![keyword_rule]);
    let make_element = |decorations: Vec<TerminalLineDecorations>| {
        NyaTerminalElement::new(
            Arc::new(snapshot.clone()),
            keyword_rules.clone(),
            decorations,
            false,
            "block",
            8.0,
            16.0,
            nyaterm_ui::theme_palette("github-dark"),
            "monospace".to_string(),
            14.0,
            400.0,
            700.0,
        )
    };
    let base = make_element(vec![TerminalLineDecorations::default()]);
    let linked = make_element(vec![TerminalLineDecorations {
        link_ranges: vec![(0, "https://help.ubuntu.com".len())],
        ..TerminalLineDecorations::default()
    }]);
    let rules_key = terminal_keyword_rules_key(keyword_rules.as_ref());
    let base_paint_key = base.paint_style_key(rules_key);
    let base_empty_key = base.paint_style_key(0);
    let linked_paint_key = linked.paint_style_key(rules_key);
    let linked_empty_key = linked.paint_style_key(0);

    assert_eq!(
        base.row_layout_cache_keys(0, base_paint_key, base_empty_key, None)
            .0,
        linked
            .row_layout_cache_keys(0, linked_paint_key, linked_empty_key, None)
            .0
    );
}

#[test]
fn row_layout_key_tracks_precomputed_keyword_presence() {
    let mut snapshot = TerminalScreen::default().snapshot();
    edit_snapshot_row(&mut snapshot, 0, |row| {
        row.text = "ERROR".to_string();
        row.signature = 7;
    });
    let keyword_rule = ResolvedKeywordHighlightRule {
        id: "errors".to_string(),
        name: "Errors".to_string(),
        patterns: vec!["ERROR".to_string()],
        color: "#ff0000".to_string(),
        enabled: true,
    };
    let element = NyaTerminalElement::new(
        Arc::new(snapshot),
        Arc::new(Vec::new()),
        Vec::new(),
        false,
        "block",
        8.0,
        16.0,
        nyaterm_ui::theme_palette("github-dark"),
        "monospace".to_string(),
        14.0,
        400.0,
        700.0,
    );
    let decorations = TerminalLineDecorations::default();
    let keyword_rules_key = terminal_keyword_rules_key(&[keyword_rule]);

    assert_ne!(
        element.row_layout_key_with_keyword_key(
            0,
            "ERROR",
            None,
            &decorations,
            keyword_rules_key,
            false,
        ),
        element.row_layout_key_with_keyword_key(
            0,
            "ERROR",
            None,
            &decorations,
            keyword_rules_key,
            true,
        ),
    );
}

#[test]
fn precomputed_keyword_match_does_not_reuse_plain_pending_row() {
    let mut snapshot = TerminalScreen::default().snapshot();
    edit_snapshot_row(&mut snapshot, 0, |row| {
        row.text = "ERROR".to_string();
        row.styled_spans = vec![nyaterm_terminal::StyledSpan {
            text: "ERROR".to_string(),
            style: nyaterm_terminal::CellStyle::default(),
        }]
        .into_boxed_slice();
        for (idx, ch) in "ERROR".chars().enumerate() {
            row.cells[idx].text = ch.to_string().into();
            row.cells[idx].width = 1;
        }
        row.signature = 7;
    });
    let rules = vec![ResolvedKeywordHighlightRule {
        id: "errors".to_string(),
        name: "Errors".to_string(),
        patterns: vec!["ERROR".to_string()],
        color: "#ff0000".to_string(),
        enabled: true,
    }];
    let highlighter = compile_terminal_keyword_highlighter(&rules);
    let palette = nyaterm_ui::theme_palette("github-dark");
    let highlights = Arc::new(precompute_terminal_keyword_highlights(
        &snapshot,
        &highlighter,
        palette,
        None,
    ));
    let element = NyaTerminalElement::new(
        Arc::new(snapshot),
        Arc::new(Vec::new()),
        Vec::new(),
        false,
        "block",
        8.0,
        16.0,
        palette,
        "monospace".to_string(),
        14.0,
        400.0,
        700.0,
    )
    .with_keyword_highlights(highlights.clone());
    let keyword_paint_style_key = element.paint_style_key(highlights.rules_key());
    let empty_keyword_paint_style_key = element.paint_style_key(0);

    let keyword_reuse_key = terminal_keyword_row_reuse_keys(element.snapshot.as_ref())[0];
    let (_, pending_key) = element.row_layout_cache_keys(
        0,
        keyword_paint_style_key,
        empty_keyword_paint_style_key,
        keyword_reuse_key,
    );

    assert!(pending_key.is_none());
}

#[test]
fn stale_keyword_prefix_is_painted_and_not_reused_as_the_final_parse() {
    let mut screen = TerminalScreen::new(40, 3);
    screen.advance(b"# ps -ef");
    let original = screen.snapshot();
    let rules = vec![ResolvedKeywordHighlightRule {
        id: "options".into(),
        name: "Options".into(),
        patterns: vec!["-[a-z]+".into()],
        color: "#ff2244".into(),
        enabled: true,
    }];
    let highlighter = compile_terminal_keyword_highlighter(&rules);
    let palette = nyaterm_ui::theme_palette("github-dark");
    let old = Arc::new(precompute_terminal_keyword_highlights(
        &original,
        &highlighter,
        palette,
        None,
    ));
    screen.advance(b" -aux");
    let changed = Arc::new(screen.snapshot());
    let current = Arc::new(precompute_terminal_keyword_highlights(
        &changed,
        &highlighter,
        palette,
        Some(&old),
    ));
    let make_element = || {
        NyaTerminalElement::new(
            changed.clone(),
            Arc::new(Vec::new()),
            Vec::new(),
            false,
            "block",
            8.0,
            16.0,
            palette,
            "monospace".to_string(),
            14.0,
            400.0,
            700.0,
        )
    };
    let plain = make_element();
    let stale = make_element().with_keyword_highlights(old.clone());
    let parsed = make_element().with_keyword_highlights(current.clone());
    let keyword_key = parsed.paint_style_key(current.rules_key());
    let empty_key = parsed.paint_style_key(0);
    let reuse_key = terminal_keyword_row_reuse_keys(&changed)[0];
    let keys = |element: &NyaTerminalElement| {
        element.row_layout_cache_keys(0, keyword_key, empty_key, reuse_key)
    };
    assert_ne!(keys(&plain).0, keys(&stale).0);
    assert_ne!(keys(&stale).0, keys(&parsed).0);
    assert!(keys(&stale).1.is_none());
    assert!(keys(&parsed).1.is_none());

    let row = changed.row(0).unwrap();
    let lookup = old.stale_lookup(0, &changed).unwrap();
    let spans = terminal_highlight_spans_with_keyword_ranges(
        &row.text,
        Some(&row.styled_spans),
        lookup.ranges(),
        &[],
        palette,
    );
    let cells = flatten_highlight_spans(spans);
    assert!(cells[5..8].iter().all(|cell| cell.color == Some(0xff2244)));
    assert!(cells[9..13].iter().all(|cell| !cell.keyword));
    assert_eq!(
        current.lookup(0, &changed).unwrap().ranges().unwrap().len(),
        2
    );
}

#[test]
fn paired_input_colors_override_late_batches_and_reuse_the_final_layout() {
    let rules = nyaterm_core::keyword_highlight_presets::get_builtin_keyword_rules(true);
    let highlighter = compile_terminal_keyword_highlighter(&rules);
    let palette = nyaterm_ui::theme_palette("github-dark");
    let mut screen = TerminalScreen::new(40, 3);
    screen.advance(b"$ l");
    let old = Arc::new(precompute_terminal_keyword_highlights(
        &screen.snapshot(),
        &highlighter,
        palette,
        None,
    ));
    let mut previous_input = None;
    for echo in [
        b"s".as_slice(),
        b"\x08 \x08",
        b"\x08 \x08",
        b"\r\n$ ",
        b"\r\n$ ",
        b"\r\n$ ",
        b"\r\n$ ",
    ] {
        screen.advance(echo);
        let snapshot = Arc::new(screen.viewport_snapshot(0));
        let input = Arc::new(
            precompute_terminal_input_highlights_reusing(
                &snapshot,
                &highlighter,
                previous_input.as_deref(),
            )
            .unwrap(),
        );
        previous_input = Some(input.clone());
        let final_batch = Arc::new(precompute_terminal_keyword_highlights(
            &snapshot,
            &highlighter,
            palette,
            None,
        ));
        let make_element = || {
            NyaTerminalElement::new(
                snapshot.clone(),
                Arc::new(Vec::new()),
                Vec::new(),
                false,
                "block",
                8.0,
                16.0,
                palette,
                "monospace".into(),
                14.0,
                400.0,
                700.0,
            )
        };
        let pending = make_element()
            .with_keyword_highlights(old.clone())
            .with_input_keyword_highlights(input);
        let complete = make_element().with_keyword_highlights(final_batch);
        let style_key = pending.paint_style_key(pending.keyword_rules_key());
        let empty_key = pending.paint_style_key(0);
        for (row, reuse_key) in terminal_keyword_row_reuse_keys(&snapshot)
            .into_iter()
            .enumerate()
        {
            if !snapshot.row(row).unwrap().text.starts_with('$') {
                continue;
            }
            assert_eq!(
                pending
                    .keyword_highlight_lookup(row, reuse_key)
                    .unwrap()
                    .ranges(),
                complete
                    .keyword_highlight_lookup(row, reuse_key)
                    .unwrap()
                    .ranges()
            );
            assert_eq!(
                pending.row_layout_cache_keys(row, style_key, empty_key, reuse_key),
                complete.row_layout_cache_keys(row, style_key, empty_key, reuse_key)
            );
        }
    }
}

#[test]
fn known_empty_input_result_suppresses_stale_command_colors() {
    let rules = nyaterm_core::keyword_highlight_presets::get_builtin_keyword_rules(true)
        .into_iter()
        .filter(|rule| rule.id == "builtin-command")
        .collect::<Vec<_>>();
    let highlighter = compile_terminal_keyword_highlighter(&rules);
    let palette = nyaterm_ui::theme_palette("github-dark");
    let mut screen = TerminalScreen::new(40, 3);
    screen.advance(b"$ l");
    let old = Arc::new(precompute_terminal_keyword_highlights(
        &screen.snapshot(),
        &highlighter,
        palette,
        None,
    ));
    screen.advance(b"\x08 \x08");
    let snapshot = Arc::new(screen.snapshot());
    let input = Arc::new(precompute_terminal_input_highlights(&snapshot, &highlighter).unwrap());
    let element = NyaTerminalElement::new(
        snapshot.clone(),
        Arc::new(Vec::new()),
        Vec::new(),
        false,
        "block",
        8.0,
        16.0,
        palette,
        "monospace".into(),
        14.0,
        400.0,
        700.0,
    )
    .with_keyword_highlights(old)
    .with_input_keyword_highlights(input);
    let lookup = element
        .keyword_highlight_lookup(0, terminal_keyword_row_reuse_keys(&snapshot)[0])
        .unwrap();
    assert!(lookup.is_known_empty());
    assert!(!lookup.is_stale());
}

#[test]
fn stale_keyword_cache_key_tracks_the_retained_ranges() {
    let ranges = [
        crate::types::TerminalKeywordRange {
            start_col: 5,
            end_col: 8,
            color: 0xff2244,
        },
        crate::types::TerminalKeywordRange {
            start_col: 9,
            end_col: 13,
            color: 0xff2244,
        },
    ];
    let first = TerminalKeywordHighlightLookup::Stale(&ranges[..1]);
    let second = TerminalKeywordHighlightLookup::Stale(&ranges);
    assert_ne!(
        super::terminal_keyword_row_paint_style_key(41, 0, Some(&first)),
        super::terminal_keyword_row_paint_style_key(41, 0, Some(&second)),
    );
}

#[test]
fn row_layout_key_ignores_keyword_rules_for_known_empty_keyword_rows() {
    let mut snapshot = TerminalScreen::default().snapshot();
    edit_snapshot_row(&mut snapshot, 0, |row| {
        row.text = "plain".to_string();
        row.signature = 7;
    });
    let element = NyaTerminalElement::new(
        Arc::new(snapshot),
        Arc::new(Vec::new()),
        Vec::new(),
        false,
        "block",
        8.0,
        16.0,
        nyaterm_ui::theme_palette("github-dark"),
        "monospace".to_string(),
        14.0,
        400.0,
        700.0,
    );
    let decorations = TerminalLineDecorations::default();

    assert_eq!(
        element.row_layout_key_with_keyword_state(
            0,
            "plain",
            None,
            &decorations,
            TerminalKeywordLayoutState {
                rules_key: 11,
                spans_present: false,
                result_known_empty: true,
            },
        ),
        element.row_layout_key_with_keyword_state(
            0,
            "plain",
            None,
            &decorations,
            TerminalKeywordLayoutState {
                rules_key: 99,
                spans_present: false,
                result_known_empty: true,
            },
        ),
    );
}

#[test]
fn row_layout_key_keeps_keyword_rules_for_pending_keyword_rows() {
    let mut snapshot = TerminalScreen::default().snapshot();
    edit_snapshot_row(&mut snapshot, 0, |row| {
        row.text = "plain".to_string();
        row.signature = 7;
    });
    let element = NyaTerminalElement::new(
        Arc::new(snapshot),
        Arc::new(Vec::new()),
        Vec::new(),
        false,
        "block",
        8.0,
        16.0,
        nyaterm_ui::theme_palette("github-dark"),
        "monospace".to_string(),
        14.0,
        400.0,
        700.0,
    );
    let decorations = TerminalLineDecorations::default();

    assert_ne!(
        element.row_layout_key_with_keyword_state(
            0,
            "plain",
            None,
            &decorations,
            TerminalKeywordLayoutState {
                rules_key: 11,
                spans_present: false,
                result_known_empty: false,
            },
        ),
        element.row_layout_key_with_keyword_state(
            0,
            "plain",
            None,
            &decorations,
            TerminalKeywordLayoutState {
                rules_key: 99,
                spans_present: false,
                result_known_empty: false,
            },
        ),
    );
}

#[test]
fn terminal_glyph_decorations_detects_glyph_only_work() {
    assert!(!terminal_glyph_decorations_needed(
        &TerminalLineDecorations::default()
    ));

    let mut decorations = TerminalLineDecorations {
        ..TerminalLineDecorations::default()
    };
    assert!(!terminal_glyph_decorations_needed(&decorations));

    decorations.link_ranges.push((1, 3));
    assert!(!terminal_glyph_decorations_needed(&decorations));

    decorations.link_ranges.clear();
    decorations.search_ranges.push((2, 4));
    assert!(!terminal_glyph_decorations_needed(&decorations));

    decorations.search_ranges.clear();
    decorations.active_search_ranges.push((2, 4));
    assert!(terminal_glyph_decorations_needed(&decorations));
}

#[test]
fn plain_row_fast_path_accepts_unstyled_rows() {
    let default_spans = [nyaterm_terminal::StyledSpan {
        text: "plain".to_string(),
        style: nyaterm_terminal::CellStyle::default(),
    }];

    assert!(terminal_plain_row_fast_path(
        None,
        &[],
        &TerminalLineDecorations::default()
    ));
    assert!(terminal_plain_row_fast_path(
        Some(&default_spans),
        &[],
        &TerminalLineDecorations::default()
    ));
}

#[test]
fn plain_row_fast_path_rejects_enhanced_rows() {
    let styled = nyaterm_terminal::CellStyle {
        bold: true,
        ..nyaterm_terminal::CellStyle::default()
    };
    let styled_spans = [nyaterm_terminal::StyledSpan {
        text: "bold".to_string(),
        style: styled,
    }];
    let keyword_rule = ResolvedKeywordHighlightRule {
        id: "errors".to_string(),
        name: "Errors".to_string(),
        patterns: vec!["error".to_string()],
        color: "#ff0000".to_string(),
        enabled: true,
    };
    let active_search = TerminalLineDecorations {
        active_search_ranges: vec![(0, 2)],
        ..TerminalLineDecorations::default()
    };

    assert!(!terminal_plain_row_fast_path(
        Some(&styled_spans),
        &[],
        &TerminalLineDecorations::default()
    ));
    assert!(!terminal_plain_row_fast_path(
        None,
        &[keyword_rule],
        &TerminalLineDecorations::default()
    ));
    assert!(!terminal_plain_row_fast_path(None, &[], &active_search));
}

#[test]
fn dynamic_decoration_backgrounds_include_occurrence_and_search() {
    let palette = nyaterm_ui::theme_palette("github-dark");
    let bounds = Bounds::new(point(px(0.), px(0.)), size(px(120.), px(40.)));
    let mut out = Vec::new();
    let decorations = TerminalLineDecorations {
        selected_occurrence_ranges: vec![(0, 6)],
        search_ranges: vec![(0, 2)],
        active_search_ranges: vec![(2, 4)],
        ..TerminalLineDecorations::default()
    };

    push_dynamic_decoration_backgrounds(
        0,
        &decorations,
        palette,
        TerminalPaintGeometry {
            bounds,
            visual_y_offset: -8.0,
            cell_width: 8.0,
            cell_height: 16.0,
        },
        &mut out,
    );

    assert_eq!(out.len(), 4);
    assert!(out.iter().all(|quad| quad.bounds.origin.y == px(-8.0)));
    assert_eq!(
        out[0].background,
        gpui::rgba((palette.text_muted << 8) | 0x58).into()
    );
    assert_eq!(
        out[2].background,
        gpui::rgb(palette.terminal_find_bg).into()
    );
    assert_eq!(
        out[3].border_color,
        gpui::rgb(palette.terminal_find_border).into()
    );
    assert_eq!(out[3].bounds, out[2].bounds);
    assert_eq!(out[3].border_widths.top, px(1.));
}

#[test]
fn grid_selection_maps_forward_reverse_multiline_and_scrolled_snapshots() {
    let mut screen = TerminalScreen::new(8, 3);
    screen.set_scrollback_limit(20);
    screen.advance(b"zero\r\none\r\ntwo\r\nthree\r\nfour");
    let snapshot = screen.viewport_snapshot(1);
    let absolute_end = snapshot.total_rows.saturating_sub(snapshot.display_offset);
    let absolute_start = absolute_end.saturating_sub(snapshot.row_count());
    let forward = TerminalGridSelection::new(absolute_start, 2, absolute_start + 2, 3, false);
    let reverse = TerminalGridSelection::new(absolute_start + 2, 3, absolute_start, 2, false);

    let expected = [Some((2, 8)), Some((0, 8)), Some((0, 4))];
    for (row, expected) in expected.into_iter().enumerate() {
        assert_eq!(
            terminal_selection_cols_for_snapshot_row(&snapshot, row, Some(forward)),
            expected
        );
        assert_eq!(
            terminal_selection_cols_for_snapshot_row(&snapshot, row, Some(reverse)),
            expected
        );
    }
}

#[test]
fn grid_selection_handles_empty_and_all_buffer_ranges() {
    let snapshot = TerminalScreen::new(8, 3).viewport_snapshot(0);
    let empty = TerminalGridSelection::new(0, 2, 0, 2, false);
    let all = TerminalGridSelection::new(0, 0, 0, 0, true);

    assert_eq!(
        terminal_selection_cols_for_snapshot_row(&snapshot, 0, Some(empty)),
        None
    );
    assert_eq!(
        terminal_selection_cols_for_snapshot_row(&snapshot, 0, Some(all)),
        Some((0, 8))
    );
}

#[test]
fn grid_selection_uses_cell_columns_for_wide_characters() {
    let mut screen = TerminalScreen::new(8, 2);
    screen.advance("a界b".as_bytes());
    let snapshot = screen.viewport_snapshot(0);
    let absolute_start = snapshot
        .total_rows
        .saturating_sub(snapshot.display_offset)
        .saturating_sub(snapshot.row_count());
    let selection = TerminalGridSelection::new(absolute_start, 1, absolute_start, 2, false);

    assert_eq!(
        terminal_selection_cols_for_snapshot_row(&snapshot, 0, Some(selection)),
        Some((1, 3))
    );
}

#[test]
fn half_cell_grid_selection_paints_the_same_cells_that_are_copied() {
    let mut screen = TerminalScreen::new(8, 2);
    screen.advance("A界😀B".as_bytes());
    let snapshot = screen.snapshot();
    for (start_col, start_side, end_col, end_side, expected, text) in [
        (
            0,
            TerminalCellSide::Left,
            0,
            TerminalCellSide::Right,
            Some((0, 1)),
            "A",
        ),
        (
            0,
            TerminalCellSide::Left,
            1,
            TerminalCellSide::Left,
            Some((0, 1)),
            "A",
        ),
        (
            1,
            TerminalCellSide::Left,
            1,
            TerminalCellSide::Right,
            Some((1, 3)),
            "界",
        ),
        (
            2,
            TerminalCellSide::Left,
            2,
            TerminalCellSide::Right,
            Some((1, 3)),
            "界",
        ),
        (
            4,
            TerminalCellSide::Left,
            4,
            TerminalCellSide::Right,
            Some((3, 5)),
            "😀",
        ),
        (
            0,
            TerminalCellSide::Right,
            1,
            TerminalCellSide::Left,
            None,
            "",
        ),
    ] {
        let a =
            TerminalSelectionEndpoint::new(TerminalBufferCellPos::new(0, start_col), start_side);
        let b = TerminalSelectionEndpoint::new(TerminalBufferCellPos::new(0, end_col), end_side);
        for (anchor, head) in [(a, b), (b, a)] {
            let selection = TerminalSelection {
                anchor,
                head,
                all_buffer: false,
            };
            let grid = TerminalGridSelection::from_selection(selection);
            assert_eq!(
                terminal_selection_cols_for_snapshot_row(&snapshot, 0, Some(grid)),
                expected
            );
            let copied = selection
                .cols_for_absolute_line(0)
                .map(|(start, end)| selected_row_text(&snapshot.row(0).unwrap().cells, start, end))
                .unwrap_or_default();
            assert_eq!(copied, text);
            for scale in [1.0, 1.25, 1.5] {
                let geometry = TerminalPaintGeometry {
                    bounds: Bounds::new(
                        point(px(3.5), px(9.25)),
                        size(px(8.0 * 7.5 * scale), px(32.0)),
                    ),
                    visual_y_offset: -2.5,
                    cell_width: 7.5 * scale,
                    cell_height: 16.0 * scale,
                };
                let mut backgrounds = Vec::new();
                push_dynamic_selection_background(
                    &snapshot,
                    0,
                    Some(grid),
                    nyaterm_ui::theme_palette("github-dark"),
                    geometry,
                    &mut backgrounds,
                );
                assert_eq!(backgrounds.len(), usize::from(expected.is_some()));
                if let Some((start, end)) = expected {
                    let bounds = backgrounds[0].bounds;
                    assert_eq!(
                        f32::from(bounds.left()),
                        (3.5 + start as f32 * geometry.cell_width).floor()
                    );
                    assert_eq!(
                        f32::from(bounds.right()),
                        (3.5 + end as f32 * geometry.cell_width).ceil()
                    );
                }
            }
        }
    }
}

#[test]
fn grid_selection_paints_one_background_per_selected_snapshot_row() {
    let snapshot = TerminalScreen::new(8, 4).viewport_snapshot(0);
    let palette = nyaterm_ui::theme_palette("github-dark");
    let geometry = TerminalPaintGeometry {
        bounds: Bounds::new(point(px(0.), px(0.)), size(px(64.), px(64.))),
        visual_y_offset: 0.0,
        cell_width: 8.0,
        cell_height: 16.0,
    };
    let selection = TerminalGridSelection::new(1, 2, 3, 4, false);
    let mut backgrounds = Vec::new();

    for row in 0..snapshot.row_count() {
        push_dynamic_selection_background(
            &snapshot,
            row,
            Some(selection),
            palette,
            geometry,
            &mut backgrounds,
        );
    }

    assert_eq!(backgrounds.len(), 3);
    assert!(
        backgrounds
            .iter()
            .all(|quad| quad.background == gpui::rgb(palette.terminal_selection).into())
    );
}

#[test]
fn visible_rows_expand_for_visual_scroll_offset() {
    let bounds = Bounds::new(point(px(0.), px(0.)), size(px(100.), px(32.)));

    assert_eq!(terminal_visible_rows_for_bounds(bounds, 16., 10, 0.0), 0..3);
    assert_eq!(
        terminal_visible_rows_for_bounds(bounds, 16., 10, -8.0),
        0..4
    );
    assert_eq!(
        terminal_visible_rows_for_bounds(bounds, 16., 10, -20.0),
        0..5
    );
    assert_eq!(terminal_visible_rows_for_bounds(bounds, 16., 10, 8.0), 0..3);
    assert_eq!(
        terminal_visible_rows_for_bounds(bounds, 16., 10, 20.0),
        0..2
    );
}

#[test]
fn visible_rows_use_parent_content_mask_intersection() {
    let bounds = Bounds::new(point(px(0.), px(0.)), size(px(100.), px(160.)));
    let clipped = Bounds::new(point(px(0.), px(64.)), size(px(100.), px(32.)));

    assert_eq!(
        terminal_visible_rows_for_clipped_bounds(bounds, clipped, 16., 10, 0.0),
        3..7
    );
}

#[test]
fn layout_prefetch_waits_until_every_visible_row_is_cached() {
    assert_eq!(terminal_layout_prefetch_row(2..4, 8, |row| row == 2), None);
}

#[test]
fn layout_prefetch_moves_outward_from_the_visible_viewport() {
    assert_eq!(
        terminal_layout_prefetch_row(2..4, 8, |row| matches!(row, 2 | 3)),
        Some(1)
    );
    assert_eq!(
        terminal_layout_prefetch_row(2..4, 8, |row| matches!(row, 1..=3)),
        Some(4)
    );
    assert_eq!(terminal_layout_prefetch_row(2..4, 4, |_| true), None);
}

#[test]
fn layout_height_can_use_viewport_rows_instead_of_snapshot_window_rows() {
    assert_eq!(terminal_layout_height_px(16.0, 80, None), 1280.0);
    assert_eq!(terminal_layout_height_px(16.0, 80, Some(24)), 384.0);
    assert_eq!(terminal_layout_height_px(0.0, 0, Some(0)), 1.0);
}

#[test]
fn concealed_cursor_cell_suppresses_cursor_glyph() {
    let mut snapshot = TerminalScreen::default().snapshot();
    snapshot.cursor.row = 0;
    snapshot.cursor.col = 0;
    edit_snapshot_row(&mut snapshot, 0, |row| row.cells[0].style.hidden = true);

    assert!(terminal_cursor_cell_hidden(&snapshot));

    edit_snapshot_row(&mut snapshot, 0, |row| row.cells[0].style.hidden = false);
    assert!(!terminal_cursor_cell_hidden(&snapshot));
}

#[test]
fn contrast_selection_changes_only_the_affected_row_keys_and_off_mode_reuses_rows() {
    let mut screen = TerminalScreen::new(30, 3);
    screen.advance(b"first row\r\nsecond row");
    let snapshot = Arc::new(screen.snapshot());
    let make = |ratio| {
        NyaTerminalElement::new(
            snapshot.clone(),
            Arc::new(Vec::new()),
            Vec::new(),
            false,
            "block",
            8.0,
            16.0,
            nyaterm_ui::theme_palette("github-dark"),
            "monospace".into(),
            14.0,
            400.0,
            700.0,
        )
        .with_minimum_contrast_ratio(ratio)
    };
    let key = |element: &NyaTerminalElement, row| {
        let snapshot_row = snapshot.row(row).unwrap();
        element.row_layout_key(
            row,
            &snapshot_row.text,
            Some(&snapshot_row.styled_spans),
            &TerminalLineDecorations::default(),
        )
    };
    let plain = make(7.0);
    let selected = make(7.0).with_selection(Some(TerminalGridSelection::new(0, 0, 0, 3, false)));
    assert_ne!(key(&plain, 0), key(&selected, 0));
    assert_eq!(key(&plain, 1), key(&selected, 1));
    let moved = make(7.0).with_selection(Some(TerminalGridSelection::new(1, 0, 1, 3, false)));
    assert_eq!(key(&plain, 0), key(&moved, 0));
    assert_ne!(key(&plain, 1), key(&moved, 1));
    let disabled = make(1.0);
    let disabled_selected = make(1.0).with_selection(selected.selection);
    assert_eq!(key(&disabled, 0), key(&disabled_selected, 0));
    assert_ne!(key(&disabled, 0), key(&plain, 0));
    assert_ne!(key(&make(4.5), 0), key(&plain, 0));
}

#[test]
fn contrast_search_and_occurrence_ranges_invalidate_only_enabled_row_layouts() {
    let snapshot = Arc::new(TerminalScreen::new(30, 3).snapshot());
    let make = |ratio| {
        NyaTerminalElement::new(
            snapshot.clone(),
            Arc::new(Vec::new()),
            Vec::new(),
            false,
            "block",
            8.0,
            16.0,
            nyaterm_ui::theme_palette("github-dark"),
            "monospace".into(),
            14.0,
            400.0,
            700.0,
        )
        .with_minimum_contrast_ratio(ratio)
    };
    let empty = TerminalLineDecorations::default();
    for decorations in [
        TerminalLineDecorations {
            search_ranges: vec![(0, 3)],
            ..empty.clone()
        },
        TerminalLineDecorations {
            selected_occurrence_ranges: vec![(0, 3)],
            ..empty.clone()
        },
    ] {
        for ratio in [1.0, 7.0] {
            let element = make(ratio);
            let base = element.row_layout_key(0, "text", None, &empty);
            let decorated = element.row_layout_key(0, "text", None, &decorations);
            assert_eq!(base == decorated, ratio == 1.0);
        }
    }
}

#[test]
fn contrast_corrects_trailing_decorations_and_conceals_hidden_trailing_cells() {
    let mut screen = TerminalScreen::new(30, 3);
    screen.advance(b"x\x1b[4;38;2;40;220;220;48;2;65;210;85m  \x1b[8m  ");
    let snapshot = screen.snapshot();
    let row = snapshot.row(0).unwrap();
    let palette = nyaterm_ui::theme_palette("github-dark");
    let overlays = TerminalContrastOverlays {
        selection: Some((1, 5)),
        ..TerminalContrastOverlays::default()
    };
    let decorations = super::terminal_text_decorations_for_row(
        &[],
        Some(&row.cells),
        palette,
        false,
        7.0,
        &overlays,
        &mut TerminalContrastCache::default(),
    );
    assert!(
        decorations.underlines.iter().any(|range| range.start == 1
            && contrast_ratio(range.color, palette.terminal_selection) >= 7.0)
    );
    assert!(
        decorations
            .underlines
            .iter()
            .any(|range| range.start == 3 && range.color == palette.terminal_selection)
    );
}
