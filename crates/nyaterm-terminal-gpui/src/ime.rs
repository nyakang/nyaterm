use std::ops::Range;

use gpui::{
    App, Bounds, ContentMask, Font, Hsla, Pixels, Point, ShapedLine, TextAlign, TextRun, Window,
    bounds, fill, point, px, size,
};

pub const CARET_WIDTH: f32 = 2.;
const CARET_OUTLINE_WIDTH: f32 = 1.;
const PREEDIT_MAX_WIDTH: f32 = 360.;

/// Composition caret. A plain insertion point is a thin beam; while the IME
/// replaces a range the caret becomes a hollow block spanning that range, the
/// same distinction Alacritty draws over its preedit cells. Deliberately
/// independent of the configured `cursor_style`: a solid block here would hide
/// the glyph it sits on, and the composition already reads as uncommitted from
/// its underline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ImeCaret {
    Beam(Bounds<Pixels>),
    Outline(Bounds<Pixels>),
}

/// One shaped line supplies both native IME geometry and the painted insertion point.
pub struct TerminalImeLayout {
    pub bounds: Bounds<Pixels>,
    line: ShapedLine,
    text_origin: Point<Pixels>,
    caret: ImeCaret,
    foreground: Hsla,
    underline: Hsla,
}

impl TerminalImeLayout {
    pub fn caret(&self) -> ImeCaret {
        self.caret
    }

    pub fn range_bounds(&self, range: Range<usize>) -> Bounds<Pixels> {
        let bytes = utf16_byte_range(&self.line.text, range);
        let left = self.text_origin.x + self.line.x_for_index(bytes.start);
        let right = self.text_origin.x + self.line.x_for_index(bytes.end);
        let left = left
            .max(self.bounds.left())
            .min(self.bounds.right() - px(CARET_WIDTH));
        let right = right.max(left + px(CARET_WIDTH)).min(self.bounds.right());
        bounds(
            point(left, self.bounds.top()),
            size(right - left, self.bounds.size.height),
        )
    }

    pub fn character_index(&self, position: Point<Pixels>) -> usize {
        let byte = self
            .line
            .closest_index_for_x(position.x - self.text_origin.x);
        self.line.text[..byte].encode_utf16().count()
    }

    pub fn paint(&self, window: &mut Window, cx: &mut App) {
        window.with_content_mask(
            Some(ContentMask {
                bounds: self.bounds,
            }),
            |window| {
                // The grid excludes these bounds before painting. Keep the
                // existing surface/wallpaper beneath the composition: another
                // translucent fill would accumulate alpha and reveal TUI carets.
                if let Err(error) = self.line.paint(
                    self.text_origin,
                    self.bounds.size.height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                ) {
                    tracing::warn!(%error, "failed to paint terminal IME preedit");
                }
                window.paint_quad(fill(
                    bounds(
                        point(self.bounds.left(), self.bounds.bottom() - px(2.)),
                        size(self.bounds.size.width, px(2.)),
                    ),
                    self.underline,
                ));
                match self.caret {
                    ImeCaret::Beam(caret) => window.paint_quad(fill(caret, self.foreground)),
                    ImeCaret::Outline(caret) => {
                        let edge = px(CARET_OUTLINE_WIDTH);
                        for quad in [
                            bounds(caret.origin, size(caret.size.width, edge)),
                            bounds(
                                point(caret.left(), caret.bottom() - edge),
                                size(caret.size.width, edge),
                            ),
                            bounds(caret.origin, size(edge, caret.size.height)),
                            bounds(
                                point(caret.right() - edge, caret.top()),
                                size(edge, caret.size.height),
                            ),
                        ] {
                            window.paint_quad(fill(quad, self.foreground));
                        }
                    }
                }
            },
        );
    }
}

/// Lay the composition out on the anchor cell itself. The text starts on the
/// cell boundary, without extra leading padding, so it lands on the same cells
/// the application would draw committed text on and never straddles the
/// application's own caret cell with a partially covered glyph.
fn preedit_geometry(
    viewport: Bounds<Pixels>,
    anchor: Bounds<Pixels>,
    text_width: Pixels,
    caret_x: Pixels,
) -> (Bounds<Pixels>, Point<Pixels>) {
    // The box covers the text plus the trailing caret, and never grows past the
    // terminal width: one line, scroll inside the box, no wrapping.
    let width = (text_width + px(CARET_WIDTH))
        .min(px(PREEDIT_MAX_WIDTH))
        .min(viewport.size.width)
        .max(px(CARET_WIDTH));
    let left = anchor
        .left()
        .min(viewport.right() - width)
        .max(viewport.left());
    let height = anchor.size.height.min(viewport.size.height);
    let top = anchor
        .top()
        .min(viewport.bottom() - height)
        .max(viewport.top());
    let scroll = (caret_x + px(CARET_WIDTH) - width).max(px(0.));
    (
        bounds(point(left, top), size(width, height)),
        point(left - scroll, top),
    )
}

/// Caret geometry for the current composition selection.
fn preedit_caret(layout: &TerminalImeLayout, selection: Range<usize>) -> ImeCaret {
    if selection.is_empty() {
        ImeCaret::Beam(layout.range_bounds(selection.end..selection.end))
    } else {
        ImeCaret::Outline(layout.range_bounds(selection))
    }
}

/// Shape a preedit line in viewport coordinates. The background remains transparent.
pub fn terminal_ime_layout(
    text: &str,
    selection: Range<usize>,
    viewport: Bounds<Pixels>,
    anchor: Bounds<Pixels>,
    style: TerminalImeStyle,
    window: &mut Window,
) -> Option<TerminalImeLayout> {
    if viewport.size.width < px(CARET_WIDTH) || viewport.size.height <= px(0.) {
        return None;
    }
    let TerminalImeStyle {
        font,
        font_size,
        foreground,
        underline,
    } = style;
    let line = window.text_system().shape_line(
        text.to_owned().into(),
        font_size,
        &[TextRun {
            len: text.len(),
            font,
            color: foreground,
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    );
    let selected_bytes = utf16_byte_range(text, selection.clone());
    let (bounds, text_origin) = preedit_geometry(
        viewport,
        anchor,
        line.width(),
        line.x_for_index(selected_bytes.end),
    );
    let mut layout = TerminalImeLayout {
        bounds,
        line,
        text_origin,
        caret: ImeCaret::Beam(anchor),
        foreground,
        underline,
    };
    layout.caret = preedit_caret(&layout, selection);
    Some(layout)
}

pub struct TerminalImeStyle {
    pub font: Font,
    pub font_size: Pixels,
    pub foreground: Hsla,
    pub underline: Hsla,
}

/// Clamp UTF-16 offsets to scalar boundaries before indexing Rust strings.
pub fn utf16_byte_range(text: &str, range: Range<usize>) -> Range<usize> {
    let offset = |wanted: usize| {
        let mut units = 0;
        for (byte, ch) in text.char_indices() {
            if units + ch.len_utf16() > wanted {
                return byte;
            }
            units += ch.len_utf16();
        }
        text.len()
    };
    let start = offset(range.start);
    start..offset(range.end).max(start)
}

#[cfg(test)]
mod tests {
    use super::{preedit_geometry, utf16_byte_range};
    #[test]
    fn utf16_ranges_preserve_chinese_and_clamp_surrogate_pairs() {
        let text = "a😀中";
        assert_eq!(&text[utf16_byte_range(text, 1..3)], "😀");
        assert_eq!(&text[utf16_byte_range(text, 2..4)], "😀中");
        assert_eq!(utf16_byte_range(text, 99..100), text.len()..text.len());
        assert_eq!(
            utf16_byte_range(text, std::ops::Range { start: 3, end: 1 }),
            5..5
        );
    }
    use gpui::{bounds, point, px, size};
    #[test]
    fn ime_long_preedit_keeps_caret_visible_at_the_right_edge() {
        let viewport = bounds(point(px(20.), px(40.)), size(px(600.), px(300.)));
        let anchor = bounds(point(px(610.), px(60.)), size(px(10.), px(20.)));
        for caret in [0., 200., 800.] {
            let (bounds, origin) = preedit_geometry(viewport, anchor, px(800.), px(caret));
            assert_eq!(bounds.size.width, px(360.));
            assert!(bounds.left() >= viewport.left());
            assert!(bounds.right() <= viewport.right());
            assert!(origin.x + px(caret) >= bounds.left());
            assert!(origin.x + px(caret + 2.) <= bounds.right());
        }
    }

    #[test]
    fn ime_preedit_geometry_respects_narrow_and_short_viewports() {
        let viewport = bounds(point(px(100.), px(50.)), size(px(6.), px(10.)));
        let anchor = bounds(point(px(105.), px(65.)), size(px(10.), px(20.)));
        let (bounds, origin) = preedit_geometry(viewport, anchor, px(200.), px(180.));
        assert_eq!(bounds, viewport);
        assert!(origin.x + px(180.) >= bounds.left());
        assert!(origin.x + px(182.) <= bounds.right());
    }

    #[test]
    fn ime_preedit_starts_on_the_anchor_cell() {
        let viewport = bounds(point(px(20.), px(40.)), size(px(600.), px(300.)));
        let anchor = bounds(point(px(120.), px(60.)), size(px(10.), px(20.)));
        let (bounds, origin) = preedit_geometry(viewport, anchor, px(80.), px(80.));
        // No leading padding: the first composed glyph covers the cell the
        // application put its own caret in instead of straddling it.
        assert_eq!(origin.x, anchor.left());
        assert_eq!(bounds.left(), anchor.left());
        assert_eq!(bounds.size.width, px(82.));
        assert_eq!(bounds.top(), anchor.top());
        assert_eq!(bounds.size.height, anchor.size.height);
    }
}
