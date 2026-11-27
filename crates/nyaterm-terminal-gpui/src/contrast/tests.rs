use nyaterm_terminal::{CellStyle, StyledSpan, TerminalScreen};

use super::{
    CONTRAST_CACHE_CAP, TerminalContrastCache, TerminalContrastOverlays, apply_contrast,
    contrast_ratio, distance, ensure_contrast, normalize_ratio,
};
use crate::ansi::ansi_to_highlight_spans_compiled;
use crate::keywords::CompiledKeywordRule;
use crate::paint::{
    compress_flat_cells, flatten_highlight_spans,
    terminal_highlight_spans_with_keyword_ranges_and_options,
};
use crate::types::{TerminalHighlightSpan, TerminalKeywordRange};

fn span(text: &str, fg: u32, bg: u32) -> TerminalHighlightSpan {
    TerminalHighlightSpan {
        text: text.into(),
        color: Some(fg),
        bg: Some(bg),
        keyword: false,
        underline: false,
        strikeout: false,
        bold: false,
        italic: false,
        hidden: false,
    }
}

#[test]
fn correction_meets_reachable_targets_on_light_dark_and_intermediate_backgrounds() {
    assert_eq!(contrast_ratio(0, 0xffffff), 21.0);
    for fg in [0, 0xffffff, 0x00ffff, 0x28dcdc, 0x808080, 0x123456] {
        for bg in [0, 0xffffff, 0x00ff00, 0x41d255, 0x777777, 0x112233] {
            for target in [1.0, 3.0, 4.5, 7.0, 21.0] {
                let result = ensure_contrast(fg, bg, target);
                let available = contrast_ratio(0, bg).max(contrast_ratio(0xffffff, bg));
                assert!(contrast_ratio(result, bg) >= target.min(available));
                if contrast_ratio(fg, bg) >= target {
                    assert_eq!(result, fg);
                }
                if available < target {
                    assert!(result == 0 || result == 0xffffff);
                }
            }
        }
    }
}

#[test]
fn correction_can_darken_text_and_chooses_the_closer_passing_direction() {
    let dark = ensure_contrast(0x28dcdc, 0x41d255, 7.0);
    assert!(dark < 0x28dcdc);
    let fg = 0x888888;
    let bg = 0x777777;
    let target = 3.0;
    let dark = super::candidate(fg, bg, 0, target).unwrap();
    let light = super::candidate(fg, bg, 0xffffff, target).unwrap();
    let actual = ensure_contrast(fg, bg, target);
    assert!(distance(fg, actual) <= distance(fg, dark));
    assert!(distance(fg, actual) <= distance(fg, light));
}

#[test]
fn invalid_ratios_disable_correction_and_finite_ratios_are_clamped() {
    let mut cache = TerminalContrastCache::default();
    for ratio in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 0.0, 1.0] {
        assert_eq!(cache.resolve(0x28dcdc, 0x41d255, ratio), 0x28dcdc);
    }
    assert!(cache.colors.is_empty());
    assert_eq!(normalize_ratio(100.0), 21.0);
}

#[test]
fn repeated_colors_hit_the_cache_and_oldest_entries_are_evicted_at_capacity() {
    let mut cache = TerminalContrastCache::default();
    let first = cache.resolve(0, 0x123456, 4.5);
    let oldest = cache.order[0];
    assert_eq!(cache.resolve(0, 0x123456, 4.5), first);
    assert_eq!(cache.order.len(), 1);
    for fg in 1..=CONTRAST_CACHE_CAP as u32 {
        cache.resolve(fg, 0x123456, 4.5);
    }
    assert_eq!(cache.colors.len(), CONTRAST_CACHE_CAP);
    assert_eq!(cache.order.len(), CONTRAST_CACHE_CAP);
    assert!(!cache.colors.contains_key(&oldest));
    cache.resolve(0, 0x123456, 7.0);
    cache.resolve(0, 0x654321, 4.5);
    assert_eq!(cache.colors.len(), CONTRAST_CACHE_CAP);
}

#[test]
fn both_style_paths_correct_ansi_16_256_and_truecolor_without_changing_source_backgrounds() {
    let palette = nyaterm_ui::theme_palette("github-dark");
    for sgr in [
        "36;42",
        "38;5;51;48;5;46",
        "38;2;40;220;220;48;2;65;210;85",
        "1;36;42",
        "7;36;42",
        "1",
    ] {
        let mut screen = TerminalScreen::new(30, 2);
        screen.advance(format!("\x1b[{sgr}mresources/\x1b[0m").as_bytes());
        let snapshot = screen.snapshot();
        let row = snapshot.row(0).unwrap();
        let ansi = ansi_to_highlight_spans_compiled(&row.styled_spans, palette, &[], true);
        let runs = terminal_highlight_spans_with_keyword_ranges_and_options(
            &row.text,
            Some(&row.styled_spans),
            None,
            &[],
            palette,
            true,
        );
        for ratio in [1.0, 4.5, 7.0] {
            let mut cache = TerminalContrastCache::default();
            let left = apply_contrast(
                &ansi,
                palette,
                ratio,
                &TerminalContrastOverlays::default(),
                &mut cache,
            );
            let right = apply_contrast(
                &runs,
                palette,
                ratio,
                &TerminalContrastOverlays::default(),
                &mut cache,
            );
            let (left, right) = (
                flatten_highlight_spans(left),
                flatten_highlight_spans(right),
            );
            for (a, b) in left.iter().zip(&right).take(10) {
                assert_eq!(a.color, b.color, "{sgr}");
                assert_eq!(a.bg, b.bg, "{sgr}");
                assert!(contrast_ratio(a.color.unwrap(), a.bg.unwrap()) >= ratio);
            }
            assert_eq!(left[0].bg, Some(ansi[0].bg.unwrap_or(palette.terminal_bg)));
            if ratio == 1.0 {
                assert_eq!(left[0].color, ansi[0].color);
            }
        }
    }
}

#[test]
fn keyword_colors_are_corrected_after_both_keyword_style_paths() {
    let palette = nyaterm_ui::theme_palette("github-dark");
    let ansi = [StyledSpan {
        text: "ERROR".into(),
        style: CellStyle::default(),
    }];
    let rules = [CompiledKeywordRule {
        regex: regex::Regex::new("ERROR").unwrap(),
        color: palette.terminal_bg,
        background: None,
        priority: 0,
    }];
    let ranges = [TerminalKeywordRange {
        start_col: 0,
        end_col: 5,
        color: palette.terminal_bg,
        background: None,
    }];
    let paths = [
        ansi_to_highlight_spans_compiled(&ansi, palette, &rules, false),
        terminal_highlight_spans_with_keyword_ranges_and_options(
            "ERROR",
            Some(&ansi),
            Some(&ranges),
            &[],
            palette,
            false,
        ),
    ];
    for path in paths {
        assert!(path[0].keyword);
        let result = apply_contrast(
            &path,
            palette,
            7.0,
            &TerminalContrastOverlays::default(),
            &mut TerminalContrastCache::default(),
        );
        assert!(contrast_ratio(result[0].color.unwrap(), palette.terminal_bg) >= 7.0);
    }
}

#[test]
fn hidden_text_remains_hidden_through_flattening_and_overlay_changes() {
    let palette = nyaterm_ui::theme_palette("github-dark");
    let ansi = [StyledSpan {
        text: "secret".into(),
        style: CellStyle {
            hidden: true,
            ..CellStyle::default()
        },
    }];
    let paths = [
        ansi_to_highlight_spans_compiled(&ansi, palette, &[], false),
        terminal_highlight_spans_with_keyword_ranges_and_options(
            "secret",
            Some(&ansi),
            None,
            &[],
            palette,
            false,
        ),
    ];
    let overlays = TerminalContrastOverlays {
        search: &[(0, 6)],
        selection: Some((2, 4)),
        ..TerminalContrastOverlays::default()
    };
    for path in paths {
        let path = compress_flat_cells(flatten_highlight_spans(path));
        assert!(path.iter().all(|span| span.hidden));
        let result = apply_contrast(
            &path,
            palette,
            7.0,
            &overlays,
            &mut TerminalContrastCache::default(),
        );
        assert!(
            result
                .iter()
                .all(|span| span.color == span.bg && span.hidden)
        );
        assert_eq!(result[0].bg, Some(palette.terminal_find_bg));
        assert_eq!(result[1].bg, Some(palette.terminal_selection));
    }
}

#[test]
fn effective_background_uses_selection_then_search_then_occurrence_wash() {
    let palette = nyaterm_ui::theme_palette("github-dark");
    let overlays = TerminalContrastOverlays {
        occurrences: &[(0, 8)],
        search: &[(2, 6)],
        active_search: &[(4, 7)],
        selection: Some((5, 6)),
    };
    let result = apply_contrast(
        &[span("abcdefgh", 0x28dcdc, 0x41d255)],
        palette,
        7.0,
        &overlays,
        &mut TerminalContrastCache::default(),
    );
    let cells = flatten_highlight_spans(result);
    assert_eq!(cells[2].bg, Some(palette.terminal_find_bg));
    assert_eq!(cells[5].bg, Some(palette.terminal_selection));
    assert_eq!(cells[6].bg, Some(palette.terminal_find_bg));
    assert_ne!(cells[0].bg, Some(0x41d255));
    for cell in cells {
        assert!(contrast_ratio(cell.color.unwrap(), cell.bg.unwrap()) >= 7.0);
    }
}

#[test]
fn graphic_runs_keep_original_colors_and_combining_marks_stay_with_their_base() {
    let palette = nyaterm_ui::theme_palette("github-dark");
    let text = "a\u{301}界─█\u{e0b0}e\u{301}";
    let result = apply_contrast(
        &[span(text, 0x28dcdc, 0x41d255)],
        palette,
        7.0,
        &TerminalContrastOverlays::default(),
        &mut TerminalContrastCache::default(),
    );
    assert_eq!(
        result
            .iter()
            .map(|span| span.text.as_str())
            .collect::<String>(),
        text
    );
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].text, "a\u{301}界");
    assert_eq!(result[1].text, "─█\u{e0b0}");
    assert_eq!(result[1].color, Some(0x28dcdc));
    assert_eq!(result[2].text, "e\u{301}");
    assert!(contrast_ratio(result[0].color.unwrap(), 0x41d255) >= 7.0);
}
