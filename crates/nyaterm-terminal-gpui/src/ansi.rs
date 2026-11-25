use crate::keywords::{CompiledKeywordRule, keyword_highlight_spans_compiled};
use crate::types::TerminalHighlightSpan;
use crate::{resolve_cell_bg, resolve_cell_fg};

pub(super) fn ansi_to_highlight_spans_compiled(
    ansi: &[nyaterm_terminal::StyledSpan],
    palette: nyaterm_ui::ThemePalette,
    compiled_keyword_rules: &[CompiledKeywordRule],
    bold_default_foreground: bool,
) -> Vec<TerminalHighlightSpan> {
    // Build plain line for keyword overlay, then prefer keyword fg over default ANSI fg.
    let line: String = ansi.iter().map(|s| s.text.as_str()).collect();
    let keyword = keyword_highlight_spans_compiled(&line, compiled_keyword_rules);
    if keyword.iter().all(|s| !s.keyword) {
        return ansi
            .iter()
            .filter(|s| !s.text.is_empty())
            .map(|s| {
                let bg = resolve_cell_bg(palette, s.style, bold_default_foreground);
                TerminalHighlightSpan {
                    text: s.text.clone(),
                    color: Some(if s.style.hidden {
                        bg.unwrap_or(palette.terminal_bg)
                    } else {
                        resolve_cell_fg(palette, s.style, bold_default_foreground)
                    }),
                    bg,
                    keyword: false,
                    underline: s.style.underline,
                    strikeout: s.style.strikeout,
                    bold: s.style.bold,
                    italic: s.style.italic,
                    hidden: s.style.hidden,
                }
            })
            .collect();
    }

    // Flatten keyword color and background maps by byte offset, then re-slice
    // per ANSI span. The background is the one of the rule that matched.
    let mut keyword_color_at = vec![None; line.len()];
    let mut keyword_bg_at = vec![None; line.len()];
    let mut offset = 0usize;
    for span in &keyword {
        let end = offset + span.text.len();
        if span.keyword {
            for idx in offset..end.min(keyword_color_at.len()) {
                keyword_color_at[idx] = span.color;
                keyword_bg_at[idx] = span.bg;
            }
        }
        offset = end;
    }

    let mut out = Vec::new();
    let mut cursor = 0usize;
    for s in ansi {
        if s.text.is_empty() {
            continue;
        }
        let start = cursor;
        let end = cursor + s.text.len();
        cursor = end;
        let bg = resolve_cell_bg(palette, s.style, bold_default_foreground);
        let color = resolve_cell_fg(palette, s.style, bold_default_foreground);
        if s.style.hidden {
            push_ansi_segment(
                &mut out,
                &s.text,
                bg.unwrap_or(palette.terminal_bg),
                None,
                None,
                bg,
                s.style,
            );
            continue;
        }
        if s.style.reverse || s.style.fg.is_some() || s.style.fg_rgb.is_some() {
            push_ansi_segment(&mut out, &s.text, color, None, None, bg, s.style);
            continue;
        }

        let mut segment_start = 0usize;
        let mut segment_keyword_color = keyword_color_at.get(start).copied().flatten();
        let mut segment_keyword_bg = keyword_bg_at.get(start).copied().flatten();
        for (offset, _) in s.text.char_indices().skip(1) {
            let keyword_color = keyword_color_at.get(start + offset).copied().flatten();
            let keyword_bg = keyword_bg_at.get(start + offset).copied().flatten();
            if keyword_color == segment_keyword_color && keyword_bg == segment_keyword_bg {
                continue;
            }
            push_ansi_segment(
                &mut out,
                &s.text[segment_start..offset],
                color,
                segment_keyword_color,
                segment_keyword_bg,
                bg,
                s.style,
            );
            segment_start = offset;
            segment_keyword_color = keyword_color;
            segment_keyword_bg = keyword_bg;
        }
        push_ansi_segment(
            &mut out,
            &s.text[segment_start..],
            color,
            segment_keyword_color,
            segment_keyword_bg,
            bg,
            s.style,
        );
    }
    if out.is_empty() {
        out.push(TerminalHighlightSpan {
            text: " ".to_string(),
            color: None,
            bg: None,
            keyword: false,
            underline: false,
            strikeout: false,
            bold: false,
            italic: false,
            hidden: false,
        });
    }
    out
}

fn push_ansi_segment(
    out: &mut Vec<TerminalHighlightSpan>,
    text: &str,
    default_color: u32,
    keyword_color: Option<u32>,
    keyword_bg: Option<u32>,
    bg: Option<u32>,
    style: nyaterm_terminal::CellStyle,
) {
    if text.is_empty() {
        return;
    }
    out.push(TerminalHighlightSpan {
        text: text.to_string(),
        color: Some(keyword_color.unwrap_or(default_color)),
        bg: keyword_bg.or(bg),
        keyword: keyword_color.is_some(),
        underline: style.underline,
        strikeout: style.strikeout,
        bold: style.bold,
        italic: style.italic,
        hidden: style.hidden,
    });
}

#[cfg(test)]
mod tests {
    use super::ansi_to_highlight_spans_compiled;
    use crate::keywords::CompiledKeywordRule;

    fn spans(text: &str, style: nyaterm_terminal::CellStyle) -> Vec<nyaterm_terminal::StyledSpan> {
        vec![nyaterm_terminal::StyledSpan {
            text: text.to_string(),
            style,
        }]
    }

    fn compiled(pattern: &str, color: u32) -> Vec<CompiledKeywordRule> {
        vec![CompiledKeywordRule {
            regex: regex::Regex::new(pattern).unwrap(),
            color,
            background: None,
            priority: 0,
        }]
    }

    #[test]
    fn reverse_keyword_overlay_preserves_selected_colors_and_highlights_normal_text() {
        let palette = nyaterm_ui::theme_palette("github-dark");
        let mut screen = nyaterm_terminal::TerminalScreen::new(24, 2);
        screen.advance(b"SELECTED\x1b[7mSELECTED\x1b[27mSELECTED");
        let snapshot = screen.snapshot();
        let highlighted = ansi_to_highlight_spans_compiled(
            &snapshot.row(0).unwrap().styled_spans,
            palette,
            &compiled("SELECTED", 0xff2244),
            false,
        );
        assert_eq!(highlighted.len(), 3);
        assert_eq!(highlighted[0].color, Some(0xff2244));
        assert!(highlighted[0].keyword);
        assert_eq!(highlighted[1].color, Some(palette.terminal_bg));
        assert_eq!(highlighted[1].bg, Some(palette.terminal_fg));
        assert!(!highlighted[1].keyword);
        assert_eq!(highlighted[2].color, Some(0xff2244));
        assert!(highlighted[2].keyword);
    }

    #[test]
    fn keyword_overlay_splits_default_ansi_span_at_match_boundaries() {
        let palette = nyaterm_ui::theme_palette("github-dark");
        let keyword_color = 0xff2244;
        let compiled = compiled("ERROR", keyword_color);

        let highlighted = ansi_to_highlight_spans_compiled(
            &spans(
                "prefix ERROR suffix",
                nyaterm_terminal::CellStyle::default(),
            ),
            palette,
            &compiled,
            false,
        );

        assert_eq!(highlighted.len(), 3);
        assert_eq!(highlighted[0].text, "prefix ");
        assert!(!highlighted[0].keyword);
        assert_eq!(highlighted[1].text, "ERROR");
        assert_eq!(highlighted[1].color, Some(keyword_color));
        assert!(highlighted[1].keyword);
        assert_eq!(highlighted[2].text, " suffix");
        assert!(!highlighted[2].keyword);
    }

    #[test]
    fn keyword_overlay_respects_explicit_truecolor_foreground() {
        let palette = nyaterm_ui::theme_palette("github-dark");
        let style = nyaterm_terminal::CellStyle {
            fg_rgb: Some(0x112233),
            ..nyaterm_terminal::CellStyle::default()
        };
        let compiled = compiled("ERROR", 0xff2244);

        let highlighted =
            ansi_to_highlight_spans_compiled(&spans("ERROR", style), palette, &compiled, false);

        assert_eq!(highlighted.len(), 1);
        assert_eq!(highlighted[0].color, Some(0x112233));
        assert!(!highlighted[0].keyword);
    }

    #[test]
    fn hidden_ansi_text_stays_concealed_from_keyword_overlay() {
        let palette = nyaterm_ui::theme_palette("github-dark");
        let style = nyaterm_terminal::CellStyle {
            bg_rgb: Some(0x112233),
            hidden: true,
            ..nyaterm_terminal::CellStyle::default()
        };
        let compiled = compiled("secret", 0xff2244);

        let highlighted =
            ansi_to_highlight_spans_compiled(&spans("secret", style), palette, &compiled, false);

        assert_eq!(highlighted.len(), 1);
        assert_eq!(highlighted[0].color, Some(0x112233));
        assert_eq!(highlighted[0].bg, Some(0x112233));
        assert!(!highlighted[0].keyword);
    }

    #[test]
    fn keyword_overlay_handles_multibyte_prefixes() {
        let palette = nyaterm_ui::theme_palette("github-dark");
        let keyword_color = 0xff2244;
        let compiled = compiled("ERROR", keyword_color);

        let highlighted = ansi_to_highlight_spans_compiled(
            &spans("界 ERROR", nyaterm_terminal::CellStyle::default()),
            palette,
            &compiled,
            false,
        );

        assert_eq!(highlighted.len(), 2);
        assert_eq!(highlighted[0].text, "界 ");
        assert_eq!(highlighted[1].text, "ERROR");
        assert_eq!(highlighted[1].color, Some(keyword_color));
    }
}
