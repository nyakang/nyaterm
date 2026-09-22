use std::ops::Range;

use gpui::{
    Bounds, Context, ElementInputHandler, EntityInputHandler, IntoElement, Pixels, Point, Size,
    UTF16Selection, Window, prelude::*, px,
};

use crate::features::NyaTermApp;

fn terminal_input_selection() -> UTF16Selection {
    UTF16Selection {
        range: 0..0,
        reversed: false,
    }
}

fn terminal_visible_surface_bounds(
    bounds: Bounds<Pixels>,
    content_mask: Bounds<Pixels>,
) -> Option<Bounds<Pixels>> {
    let visible = content_mask.intersect(&bounds);
    (visible.size.width > px(0.) && visible.size.height > px(0.)).then_some(visible)
}

/// Invisible canvas child that records the terminal output bounds for selection hit-testing.
pub(in crate::features) fn terminal_bounds_tracker(
    entity: gpui::Entity<NyaTermApp>,
    session_id: Option<String>,
    active: bool,
) -> impl IntoElement {
    let input_entity = entity.clone();
    let bounds_entity = input_entity.clone();
    let tracked_session_id = session_id.clone();
    gpui::canvas(
        move |bounds, window, cx| {
            let Some(bounds) =
                terminal_visible_surface_bounds(bounds, window.content_mask().bounds)
            else {
                return;
            };
            let scale_factor = window.scale_factor();
            if bounds_entity
                .read(cx)
                .terminal_surface_bounds_tracking_is_current(
                    session_id.as_deref(),
                    bounds,
                    scale_factor,
                )
            {
                return;
            }
            // Defer mutation so we never re-enter the entity while layout/prepaint is running.
            let entity = entity.clone();
            let session_id = tracked_session_id.clone();
            cx.defer(move |cx| {
                entity.update(cx, |this, cx| {
                    this.remember_terminal_surface_bounds_for_session_and_sync(
                        session_id.as_deref(),
                        bounds,
                        scale_factor,
                        cx,
                    );
                });
            });
        },
        move |bounds, _state, window, cx| {
            if !active {
                return;
            }
            let focus = input_entity.read(cx).terminal.input.focus.clone();
            if input_entity
                .read(cx)
                .settings
                .summary()
                .interaction_mac_ime_compatibility
            {
                window.handle_input(&focus, ElementInputHandler::new(bounds, input_entity), cx);
            }
        },
    )
    .absolute()
    .inset_0()
    .size_full()
}

impl EntityInputHandler for NyaTermApp {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        if self.terminal.input.ime_marked_text.is_empty() {
            return None;
        }
        let len = self.terminal.input.ime_marked_text.encode_utf16().count();
        let start = range.start.min(len);
        let end = range.end.min(len).max(start);
        *adjusted_range = Some(start..end);
        Some(self.terminal.input.ime_marked_text.clone())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        // GPUI's IME contract needs a valid insertion range even when there is
        // no marked text. This is also what lets CJK candidate windows anchor to
        // the terminal cursor instead of treating the surface as non-editable.
        Some(terminal_input_selection())
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let len = self.terminal.input.ime_marked_text.encode_utf16().count();
        (len > 0).then_some(0..len)
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.terminal.input.ime_marked_text.clear();
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal.input.ime_marked_text.clear();
        if !text.is_empty() {
            if let Some(selected) = self.smart_cursor_selected_input_range()
                && self.replace_smart_input_selection(selected, text, cx)
            {
                return;
            }
            let bytes = text.as_bytes().to_vec();
            let has_buffer_selection = self.terminal.selection.selection.is_some()
                && self.smart_cursor_selected_input_range().is_none();
            if has_buffer_selection {
                self.send_terminal_input_without_suggestion_track(bytes, cx);
            } else {
                self.send_terminal_input(bytes, cx);
            }
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal.input.ime_marked_text = new_text.to_string();
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let (cell_w, cell_h) = self.terminal_cell_size();
        let insets = self.terminal_content_insets();
        let gutter = self.terminal_gutter_width_px();
        let snapshot = self.terminal_snapshot_for_session(self.session.active_id(), 0);
        let row = if snapshot.cursor.row == usize::MAX {
            snapshot.row_count().saturating_sub(1)
        } else {
            snapshot
                .cursor
                .row
                .min(snapshot.row_count().saturating_sub(1))
        };
        let col = snapshot.cursor.col.min(snapshot.cols.saturating_sub(1));
        let origin_x = f32::from(element_bounds.origin.x);
        let origin_y = f32::from(element_bounds.origin.y);
        let max_x = origin_x + f32::from(element_bounds.size.width) - cell_w.max(1.);
        let max_y = origin_y + f32::from(element_bounds.size.height) - cell_h.max(1.);
        let x = (origin_x + insets.left + gutter + col as f32 * cell_w)
            .min(max_x)
            .max(origin_x);
        let y = (origin_y + insets.top + row as f32 * cell_h)
            .min(max_y)
            .max(origin_y);
        Some(gpui::bounds(
            Point { x: px(x), y: px(y) },
            Size {
                width: px(cell_w.max(1.)),
                height: px(cell_h.max(1.)),
            },
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(0)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, px};

    use super::{terminal_input_selection, terminal_visible_surface_bounds};

    #[test]
    fn terminal_input_selection_keeps_a_valid_insertion_point() {
        let selection = terminal_input_selection();

        assert_eq!(selection.range, 0..0);
        assert!(!selection.reversed);
    }

    #[test]
    fn terminal_surface_bounds_follow_the_visible_content_mask() {
        let bounds = Bounds::new(
            gpui::point(px(10.), px(20.)),
            gpui::size(px(600.), px(4_000.)),
        );
        let content_mask = Bounds::new(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(1_280.), px(800.)),
        );

        assert_eq!(
            terminal_visible_surface_bounds(bounds, content_mask),
            Some(Bounds::new(
                gpui::point(px(10.), px(20.)),
                gpui::size(px(600.), px(780.))
            ))
        );
    }

    #[test]
    fn terminal_surface_bounds_exclude_scrollbar_column() {
        let outer_width = 610.;
        let text_width = outer_width
            - crate::features::terminal::terminal_surface::TERMINAL_SCROLLBAR_COLUMN_WIDTH;
        let bounds = Bounds::new(
            gpui::point(px(10.), px(20.)),
            gpui::size(px(text_width), px(400.)),
        );
        let content_mask = Bounds::new(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(1_280.), px(800.)),
        );

        assert_eq!(
            terminal_visible_surface_bounds(bounds, content_mask),
            Some(Bounds::new(
                gpui::point(px(10.), px(20.)),
                gpui::size(px(text_width), px(400.))
            ))
        );
    }

    #[test]
    fn terminal_surface_bounds_skip_fully_clipped_surfaces() {
        let bounds = Bounds::new(
            gpui::point(px(10.), px(900.)),
            gpui::size(px(600.), px(400.)),
        );
        let content_mask = Bounds::new(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(1_280.), px(800.)),
        );

        assert_eq!(terminal_visible_surface_bounds(bounds, content_mask), None);
    }
}

pub(super) fn open_external_url_for_action(url: &str) -> Result<(), String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("empty url".to_string());
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("failed to open url: {error}"))
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("failed to open url: {error}"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("failed to open url: {error}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::features) enum SmartSelectionEdge {
    Start,
    End,
}
