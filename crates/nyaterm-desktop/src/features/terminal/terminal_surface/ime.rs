use std::ops::Range;

use gpui::{
    App, Bounds, ContentMask, FontWeight, Hsla, Pixels, Point, ShapedLine, TextAlign, TextRun,
    Window, bounds, fill, point, px, rgb, rgba, size,
};

use crate::features::NyaTermApp;
use crate::features::terminal::ime_state::utf16_byte_range;
use crate::features::terminal::terminal_surface_entity::terminal_snapshot_anchor_row_for_display_offset;

const PADDING: f32 = 4.;
const CARET_WIDTH: f32 = 2.;

/// One shaped line supplies both native IME geometry and the painted insertion point.
pub(in crate::features) struct TerminalImeLayout {
    bounds: Bounds<Pixels>,
    line: ShapedLine,
    text_origin: Point<Pixels>,
    caret: Bounds<Pixels>,
    background: Hsla,
    foreground: Hsla,
    underline: Hsla,
}

impl TerminalImeLayout {
    pub(in crate::features) fn range_bounds(&self, range: Range<usize>) -> Bounds<Pixels> {
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

    pub(in crate::features) fn character_index(&self, position: Point<Pixels>) -> usize {
        let byte = self
            .line
            .closest_index_for_x(position.x - self.text_origin.x);
        self.line.text[..byte].encode_utf16().count()
    }

    pub(in crate::features) fn paint(&self, window: &mut Window, cx: &mut App) {
        window.with_content_mask(
            Some(ContentMask {
                bounds: self.bounds,
            }),
            |window| {
                window.paint_quad(fill(self.bounds, self.background));
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
                window.paint_quad(fill(self.caret, self.foreground));
            },
        );
    }
}

fn preedit_geometry(
    viewport: Bounds<Pixels>,
    anchor: Bounds<Pixels>,
    text_width: Pixels,
    caret_x: Pixels,
) -> (Bounds<Pixels>, Point<Pixels>) {
    let width = (text_width + px(2. * PADDING + CARET_WIDTH))
        .min(px(360.))
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
    let padding = px(PADDING).min((width - px(CARET_WIDTH)) / 2.);
    let available = (width - padding * 2. - px(CARET_WIDTH)).max(px(0.));
    let scroll = (caret_x - available).max(px(0.));
    (
        bounds(point(left, top), size(width, height)),
        point(left + padding - scroll, top),
    )
}

impl NyaTermApp {
    pub(in crate::features) fn terminal_ime_anchor_bounds(
        &self,
        element_bounds: Bounds<Pixels>,
        cx: &App,
    ) -> Bounds<Pixels> {
        let session_id = self.session.active_id();
        if let Some(geometry) = self.terminal_hit_test_geometry_for_session(session_id, cx) {
            let cursor = &geometry.snapshot.cursor;
            let row = cursor
                .row
                .min(geometry.snapshot.row_count().saturating_sub(1));
            let (display_row, display_col) = geometry
                .unwrapped
                .as_ref()
                .and_then(|layout| layout.display_position(row, cursor.col))
                .map_or(
                    (
                        row as f32,
                        cursor.col.min(geometry.cols.saturating_sub(1)) as f32,
                    ),
                    |(row, col)| (row as f32, col),
                );
            return bounds(
                point(
                    geometry.bounds.left()
                        + px(geometry.padding_left
                            + geometry.gutter
                            + display_col * geometry.cell_w),
                    geometry.bounds.top()
                        + px(geometry.padding_top
                            + display_row * geometry.cell_h
                            + geometry.visual_y_offset),
                ),
                size(px(geometry.cell_w.max(1.)), px(geometry.cell_h.max(1.))),
            );
        }
        // The first native query may precede the first painted surface geometry.
        // Live element bounds already exclude workspace padding and the scrollbar.
        let (cell_w, cell_h) = self.terminal_cell_size();
        let snapshot = self.terminal_snapshot_for_session(session_id, 0);
        let anchor = terminal_snapshot_anchor_row_for_display_offset(
            &snapshot,
            0,
            self.terminal_viewport_rows_for_session(session_id),
            self.terminal_scrollback_len_for_session(session_id),
        );
        let row = snapshot
            .cursor
            .row
            .min(snapshot.row_count().saturating_sub(1))
            .saturating_sub(anchor + snapshot.leading_cleared_rows(anchor));
        let x = element_bounds.left()
            + px(self.terminal_gutter_width_px()
                + snapshot.cursor.col.min(snapshot.cols.saturating_sub(1)) as f32 * cell_w);
        let y = element_bounds.top() + px(row as f32 * cell_h);
        bounds(
            point(
                x.min(element_bounds.right() - px(cell_w))
                    .max(element_bounds.left()),
                y.min(element_bounds.bottom() - px(cell_h))
                    .max(element_bounds.top()),
            ),
            size(px(cell_w.max(1.)), px(cell_h.max(1.))),
        )
    }

    pub(in crate::features) fn terminal_ime_layout(
        &self,
        element_bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &App,
    ) -> Option<TerminalImeLayout> {
        let ime = &self.terminal.input.ime;
        if !ime.is_active_for(self.session.active_id())
            || !self.terminal.input.focus.is_focused(window)
            || self.terminal_display_offset_for_session(self.session.active_id()) != 0
            || element_bounds.size.width < px(CARET_WIDTH)
            || element_bounds.size.height <= px(0.)
        {
            return None;
        }
        let viewport = if let Some(geometry) =
            self.terminal_hit_test_geometry_for_session(self.session.active_id(), cx)
        {
            let left = geometry.bounds.left() + px(geometry.padding_left + geometry.gutter);
            bounds(
                point(left, geometry.bounds.top() + px(geometry.padding_top)),
                size(
                    geometry.bounds.right() - left,
                    geometry.bounds.size.height - px(geometry.padding_top),
                ),
            )
            .intersect(&element_bounds)
        } else {
            let gutter = px(self.terminal_gutter_width_px());
            bounds(
                point(element_bounds.left() + gutter, element_bounds.top()),
                size(
                    element_bounds.size.width - gutter,
                    element_bounds.size.height,
                ),
            )
        };
        if viewport.size.width < px(CARET_WIDTH) || viewport.size.height <= px(0.) {
            return None;
        }
        let palette = self.terminal_theme_palette();
        let mut font = self.gpui_terminal_font().font();
        font.weight = FontWeight(self.settings.summary().terminal_font_weight as f32);
        let text = ime.text();
        let line = window.text_system().shape_line(
            text.to_owned().into(),
            px(self.settings.summary().terminal_font_size as f32),
            &[TextRun {
                len: text.len(),
                font,
                color: rgb(palette.terminal_fg).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        );
        let anchor = self.terminal_ime_anchor_bounds(element_bounds, cx);
        let selected_bytes = utf16_byte_range(text, ime.selection());
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
            caret: anchor,
            background: rgba((palette.terminal_cursor << 8) | 0x33).into(),
            foreground: rgb(palette.terminal_fg).into(),
            underline: rgb(palette.link).into(),
        };
        let end = ime.selection().end;
        layout.caret = layout.range_bounds(end..end);
        Some(layout)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui::{
        Context, Entity, EntityInputHandler, IntoElement, Render, Subscription, TestAppContext,
        Window, bounds, div, point, prelude::*, px, size,
    };

    use crate::features::NyaTermApp;
    use crate::features::terminal::terminal_selection_runtime::terminal_bounds_tracker;
    use crate::features::test_support::app_with_visible_local_session;
    use crate::test_support::TestConfigDir;

    use super::preedit_geometry;

    struct ImeHost(Entity<NyaTermApp>);

    struct ImeRepaintObserver {
        count: usize,
        _subscription: Subscription,
    }

    impl Render for ImeHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(terminal_bounds_tracker(
                self.0.clone(),
                Some("s1".into()),
                true,
            ))
        }
    }

    #[test]
    fn terminal_ime_text_and_selection_changes_invalidate_the_independent_surface() {
        let dir = TestConfigDir::new("nyaterm-ime-surface-invalidation");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        let surface = cx.update_entity(&app, |app, cx| app.ensure_terminal_surface("s1", cx));
        let observer = cx.new(|cx: &mut Context<ImeRepaintObserver>| ImeRepaintObserver {
            count: 0,
            _subscription: cx.observe(&surface, |observer, _, _| observer.count += 1),
        });
        let (_, vcx) = cx.add_window_view(|_, _| ImeHost(app.clone()));
        for text in ["z", "zh", "zhong"] {
            let before = vcx.update(|_, cx| observer.read(cx).count);
            vcx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    window.focus(&app.terminal.input.focus, cx);
                    app.replace_and_mark_text_in_range(None, text, None, window, cx);
                });
            });
            assert!(vcx.update(|_, cx| observer.read(cx).count) > before);
        }
        let before = vcx.update(|_, cx| observer.read(cx).count);
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| app.set_selected_text_range(2..2, window, cx));
        });
        assert!(vcx.update(|_, cx| observer.read(cx).count) > before);
    }

    #[test]
    fn terminal_ime_native_bounds_match_painted_caret_and_restore_grid_cursor() {
        let dir = TestConfigDir::new("nyaterm-ime-caret-geometry");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        let (_, vcx) = cx.add_window_view(|_, _| ImeHost(app.clone()));
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let mut settings = app.settings.summary().clone();
                settings.cursor_blink = false;
                app.settings.replace_summary(settings);
                app.shell.set_cursor_blink_on(true);
                let view = app.terminal.view.views.get_mut("s1").unwrap();
                view.grid_resize_pending = false;
                view.screen.advance(b"prompt\x1b[?25h");
                view.frame_snapshot = Some(Arc::new(view.screen.viewport_snapshot(0)));
                app.sync_terminal_surface_paint("s1", cx);
                window.focus(&app.terminal.input.focus, cx);
                assert!(app.terminal.view.surfaces["s1"].read(cx).cursor_is_shown());
                let bounds = bounds(point(px(30.), px(50.)), size(px(600.), px(300.)));
                app.replace_and_mark_text_in_range(None, "zhong", Some(2..2), window, cx);
                assert!(!app.terminal.view.surfaces["s1"].read(cx).cursor_is_shown());
                let native = app.bounds_for_range(2..2, bounds, window, cx).unwrap();
                let layout = app.terminal_ime_layout(bounds, window, cx).unwrap();
                assert_eq!(native, layout.caret);
                let start = layout.range_bounds(0..0);
                assert!(native.left() > start.left());
                let selected = app.selected_text_range(false, window, cx).unwrap();
                assert_eq!(selected.range, 2..2);

                app.set_selected_text_range(5..5, window, cx);
                let end = app.bounds_for_range(5..5, bounds, window, cx).unwrap();
                assert!(end.left() > native.left());
                assert_eq!(end.top(), native.top());
                app.unmark_text(window, cx);
                assert!(app.terminal.view.surfaces["s1"].read(cx).cursor_is_shown());
                assert_eq!(
                    app.selected_text_range(false, window, cx).unwrap().range,
                    0..0
                );
                assert!(app.terminal_ime_layout(bounds, window, cx).is_none());
                app.replace_and_mark_text_in_range(None, "wen", None, window, cx);
                assert!(!app.terminal.view.surfaces["s1"].read(cx).cursor_is_shown());
                app.report_terminal_focus(false, cx);
                assert!(app.terminal.view.surfaces["s1"].read(cx).cursor_is_shown());
                app.replace_and_mark_text_in_range(None, "wen", None, window, cx);
                app.replace_text_in_range(None, "", window, cx);
                assert!(app.terminal.view.surfaces["s1"].read(cx).cursor_is_shown());
            });
        });
    }

    #[test]
    fn terminal_ime_clears_on_blur_session_switch_and_session_close() {
        let dir = TestConfigDir::new("nyaterm-ime-lifecycle");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        let (_, vcx) = cx.add_window_view(|_, _| ImeHost(app.clone()));
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                window.focus(&app.terminal.input.focus, cx);
                app.replace_and_mark_text_in_range(None, "zhong", None, window, cx);
                let mut adjusted = None;
                assert_eq!(
                    app.text_for_range(1..4, &mut adjusted, window, cx),
                    Some("hon".into())
                );
                assert_eq!(adjusted, Some(1..4));
                app.report_terminal_focus(false, cx);
                assert_eq!(app.terminal.input.ime.selection(), 0..0);
                assert!(app.terminal.input.ime.text().is_empty());
                app.replace_and_mark_text_in_range(None, "zhong", None, window, cx);
                app.activate_session_id("s2", cx);
                assert!(app.terminal.input.ime.text().is_empty());
                app.replace_and_mark_text_in_range(None, "wen", None, window, cx);
                app.remove_session_state("s2", cx);
                assert!(app.terminal.input.ime.text().is_empty());
            });
        });
    }

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
}
