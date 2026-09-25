use gpui::{
    FontStyle, FontWeight, HighlightStyle, IntoElement, SharedString, StrikethroughStyle,
    StyledText, UnderlineStyle, div, prelude::*, px, rgb,
};

use crate::features::formatting::{
    InlineMdStyle, MarkdownBlock, parse_inline_markdown, parse_markdown_blocks,
};
use crate::theme::ThemePalette;

/// Lightweight GFM markdown renderer for AI transcript (Tauri MarkdownContent parity).
pub(in crate::features) fn markdown_content_view(
    palette: ThemePalette,
    content: &str,
) -> impl IntoElement {
    let blocks = parse_markdown_blocks(content);
    let mut root = div()
        .min_w_0()
        .w_full()
        .flex()
        .flex_col()
        .gap_1()
        .text_size(px(12.))
        .line_height(px(18.));
    if blocks.is_empty() {
        return root;
    }
    for (index, block) in blocks.into_iter().enumerate() {
        root = root.child(markdown_block_view(palette, index, block));
    }
    root
}

fn markdown_inline_text(palette: ThemePalette, raw: &str) -> gpui::AnyElement {
    let parsed = parse_inline_markdown(raw);
    if parsed.highlights.is_empty() {
        return div().child(parsed.text).into_any_element();
    }
    let highlights = parsed.highlights.into_iter().map(|(range, style)| {
        let highlight = match style {
            InlineMdStyle::Bold => HighlightStyle {
                font_weight: Some(FontWeight(700.)),
                ..Default::default()
            },
            InlineMdStyle::Italic => HighlightStyle {
                font_style: Some(FontStyle::Italic),
                ..Default::default()
            },
            InlineMdStyle::BoldItalic => HighlightStyle {
                font_weight: Some(FontWeight(700.)),
                font_style: Some(FontStyle::Italic),
                ..Default::default()
            },
            InlineMdStyle::Code => HighlightStyle {
                color: Some(rgb(palette.text).into()),
                background_color: Some(rgb(palette.surface_elevated).into()),
                font_weight: Some(FontWeight(500.)),
                ..Default::default()
            },
            InlineMdStyle::Link => HighlightStyle {
                color: Some(rgb(palette.link).into()),
                underline: Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(rgb(palette.link).into()),
                    wavy: false,
                }),
                ..Default::default()
            },
            InlineMdStyle::Strike => HighlightStyle {
                strikethrough: Some(StrikethroughStyle {
                    thickness: px(1.),
                    color: Some(rgb(palette.text_muted).into()),
                }),
                color: Some(rgb(palette.text_muted).into()),
                ..Default::default()
            },
            InlineMdStyle::Underline => HighlightStyle {
                underline: Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(rgb(palette.text).into()),
                    wavy: false,
                }),
                ..Default::default()
            },
        };
        (range, highlight)
    });
    StyledText::new(parsed.text)
        .with_highlights(highlights)
        .into_any_element()
}

fn markdown_block_view(
    palette: ThemePalette,
    index: usize,
    block: MarkdownBlock,
) -> gpui::AnyElement {
    match block {
        MarkdownBlock::Paragraph(text) => div()
            .id(SharedString::from(format!("md-p-{index}")))
            .text_size(px(12.))
            .text_color(rgb(palette.text))
            .line_height(px(18.))
            .child(markdown_inline_text(palette, &text))
            .into_any_element(),
        MarkdownBlock::Bullet(text) => div()
            .id(SharedString::from(format!("md-ul-{index}")))
            .flex()
            .items_start()
            .gap_2()
            .pl_1()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child("•"),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text))
                    .line_height(px(18.))
                    .child(markdown_inline_text(palette, &text)),
            )
            .into_any_element(),
        MarkdownBlock::Numbered { index: n, text } => div()
            .id(SharedString::from(format!("md-ol-{index}")))
            .flex()
            .items_start()
            .gap_2()
            .pl_1()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child(format!("{n}.")),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text))
                    .line_height(px(18.))
                    .child(markdown_inline_text(palette, &text)),
            )
            .into_any_element(),
        MarkdownBlock::Code { language, code } => div()
            .id(SharedString::from(format!("md-code-{index}")))
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.bg))
            .overflow_hidden()
            .max_h(px(256.))
            .child(
                div()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(rgb(palette.border))
                    .text_size(px(10.))
                    .font_weight(FontWeight(700.))
                    .text_color(rgb(palette.text_dimmed))
                    .child(if language.trim().is_empty() {
                        "code".to_string()
                    } else {
                        language
                    }),
            )
            .child(
                div()
                    .px_2()
                    .py_2()
                    .font_family(crate::features::shell::gpui_code_font_family())
                    .text_size(px(11.))
                    .text_color(rgb(palette.text))
                    .line_height(px(16.))
                    .child(code),
            )
            .into_any_element(),
        MarkdownBlock::Quote(text) => {
            let mut body = div().flex().flex_col().gap_1();
            for (qi, line) in text.lines().enumerate() {
                body = body.child(
                    div()
                        .id(SharedString::from(format!("md-q-{index}-{qi}")))
                        .child(markdown_inline_text(palette, line)),
                );
            }
            div()
                .id(SharedString::from(format!("md-q-{index}")))
                .pl_3()
                .border_l_2()
                .border_color(rgb(palette.border))
                .text_size(px(12.))
                .text_color(rgb(palette.text_muted))
                .line_height(px(18.))
                .child(body)
                .into_any_element()
        }
        MarkdownBlock::Heading { level, text } => {
            let size = match level {
                1 => 16.,
                2 => 14.,
                _ => 13.,
            };
            div()
                .id(SharedString::from(format!("md-h-{index}")))
                .text_size(px(size))
                .font_weight(FontWeight(800.))
                .text_color(rgb(palette.text))
                .line_height(px(size + 4.))
                .child(markdown_inline_text(palette, &text))
                .into_any_element()
        }
        MarkdownBlock::Table { headers, rows } => {
            let col_count = headers
                .len()
                .max(rows.iter().map(|r| r.len()).max().unwrap_or(0))
                .max(1);
            let mut table = div()
                .id(SharedString::from(format!("md-table-{index}")))
                .flex()
                .flex_col()
                .border_1()
                .border_color(rgb(palette.border))
                .rounded_md()
                .overflow_hidden();

            let mut header_row = div()
                .flex()
                .bg(rgb(palette.surface))
                .border_b_1()
                .border_color(rgb(palette.border));
            for col in 0..col_count {
                let cell = headers.get(col).cloned().unwrap_or_default();
                header_row = header_row.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px_2()
                        .py_1()
                        .border_r_1()
                        .border_color(rgb(palette.border))
                        .text_size(px(11.))
                        .font_weight(FontWeight(700.))
                        .text_color(rgb(palette.text))
                        .child(markdown_inline_text(palette, &cell)),
                );
            }
            table = table.child(header_row);

            for (ri, row) in rows.into_iter().enumerate() {
                let mut body_row = div()
                    .flex()
                    .border_b_1()
                    .border_color(rgb(palette.surface_elevated))
                    .bg(if ri % 2 == 0 {
                        rgb(palette.bg)
                    } else {
                        rgb(palette.section_header)
                    });
                for col in 0..col_count {
                    let cell = row.get(col).cloned().unwrap_or_default();
                    body_row = body_row.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px_2()
                            .py_1()
                            .border_r_1()
                            .border_color(rgb(palette.surface_elevated))
                            .text_size(px(11.))
                            .text_color(rgb(palette.text))
                            .child(markdown_inline_text(palette, &cell)),
                    );
                }
                table = table.child(body_row);
            }
            table.into_any_element()
        }
        MarkdownBlock::ThematicBreak => div()
            .id(SharedString::from(format!("md-hr-{index}")))
            .my_1()
            .h(px(1.))
            .w_full()
            .bg(rgb(palette.border))
            .into_any_element(),
    }
}
