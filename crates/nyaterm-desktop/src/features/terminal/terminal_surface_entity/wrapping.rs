use std::sync::Arc;

use gpui::{
    AnyElement, Context, IntoElement, ScrollDelta, ScrollEvent, div, point, prelude::*, px, rgb,
};
use nyaterm_core::models::settings::TerminalWrapMode;
use nyaterm_terminal::TerminalSnapshot;
use nyaterm_terminal_gpui::unwrapped::TerminalUnwrappedLayout;
use nyaterm_ui::NyaHorizontalScrollbar;

use super::{TerminalSurface, terminal_surface_visible_rows_for_viewport};

impl TerminalSurface {
    pub(in crate::features) fn painted_unwrapped_layout(
        &self,
    ) -> Option<Arc<TerminalUnwrappedLayout>> {
        self.painted_unwrapped.clone()
    }

    pub(super) fn unwrapped_layout(
        &mut self,
        snapshot: &TerminalSnapshot,
    ) -> Option<Arc<TerminalUnwrappedLayout>> {
        if self.wrap_mode != TerminalWrapMode::None && snapshot.display_rows.is_none() {
            return None;
        }
        let offset = -f32::from(self.horizontal_scroll.offset().x) / self.cell_width.max(1.0);
        let mut layout = TerminalUnwrappedLayout::new(
            snapshot,
            self.display_offset,
            self.viewport_rows,
            offset,
        )?;
        if self.scroll_offset > 0 {
            // Keep a manually chosen horizontal position while browsing
            // history, including when the next viewport contains shorter rows.
            layout.width_cols = layout
                .width_cols
                .max(offset.ceil() as usize + layout.viewport_cols);
        }
        let max = layout.width_cols.saturating_sub(layout.viewport_cols) as f32;
        let mut offset = offset.clamp(0.0, max);
        if let Some((id, col)) = self.horizontal_reveal.take()
            && let Some(first) = snapshot.rows().first().and_then(|row| row.line_id)
            && id.epoch == first.epoch
            && let Ok(row) = usize::try_from(id.logical_line - first.logical_line)
            && let Some((_, x)) = layout.display_position(row, col)
        {
            let logical_x = x + layout.horizontal_offset;
            if logical_x < offset {
                offset = logical_x.max(0.0);
            } else if logical_x >= offset + layout.viewport_cols as f32 {
                offset = (logical_x + 1.0 - layout.viewport_cols as f32).clamp(0.0, max);
            }
        }
        if (offset - layout.horizontal_offset).abs() > f32::EPSILON {
            self.horizontal_scroll
                .set_offset(point(px(-offset * self.cell_width), px(0.0)));
            return TerminalUnwrappedLayout::new(
                snapshot,
                self.display_offset,
                self.viewport_rows,
                offset,
            )
            .map(Arc::new);
        }
        Some(Arc::new(layout))
    }

    pub(in crate::features) fn reveal_input_cursor(
        &mut self,
        input_at: Option<std::time::Instant>,
    ) -> bool {
        if input_at.is_none() || !self.is_active || self.scroll_offset != 0 {
            return false;
        }
        let Some(snapshot) = self.snapshot.as_ref() else {
            return false;
        };
        let target = snapshot
            .row(snapshot.cursor.row)
            .and_then(|row| row.line_id)
            .map(|id| (id, snapshot.cursor.col));
        if target != self.last_horizontal_cursor || self.last_horizontal_input != input_at {
            self.last_horizontal_cursor = target;
            self.last_horizontal_input = input_at;
            if self.wrap_mode == TerminalWrapMode::None {
                self.horizontal_reveal = target;
                return target.is_some();
            }
        }
        false
    }

    pub(super) fn horizontal_scroll_layer(&self) -> (Option<AnyElement>, Option<AnyElement>) {
        let Some(layout) = self.painted_unwrapped.as_ref() else {
            return (None, None);
        };
        if !layout.unwrapped {
            return (None, None);
        }
        let content_width = layout.width_cols as f32 * self.cell_width;
        // This node measures the scrollbar range only. Overflow::Scroll would
        // also pan on wheel input (including mapping vertical wheel to X),
        // before the terminal processes the same event. The surface owns all
        // wheel input; the scrollbar can still change the shared handle.
        let measure = div()
            .id("terminal-horizontal-measure")
            .absolute()
            .inset_0()
            .overflow_hidden()
            .track_scroll(&self.horizontal_scroll)
            .child(div().w(px(content_width)).h(px(1.0)).flex_none())
            .into_any_element();
        let bar = div()
            .absolute()
            .inset_0()
            .child(NyaHorizontalScrollbar::new(
                "terminal-horizontal-scrollbar",
                &self.horizontal_scroll,
            ))
            .into_any_element();
        (Some(measure), Some(bar))
    }

    pub(super) fn wrap_marker_gutter(
        &self,
        snapshot: &TerminalSnapshot,
        offset: f32,
        cell_height: f32,
    ) -> Option<AnyElement> {
        if !self.show_wrap_markers {
            return None;
        }
        // Keep the lane when glyphs are hidden in no-wrap mode. Switching the
        // display policy must not resize the remote application's grid.
        let mut rows = div().absolute().left_0().top(px(offset)).flex().flex_col();
        if self
            .painted_unwrapped
            .as_ref()
            .is_none_or(|layout| !layout.unwrapped)
        {
            let visible = terminal_surface_visible_rows_for_viewport(
                self.viewport_rows,
                self.painted_unwrapped
                    .as_ref()
                    .map_or(snapshot.row_count(), |layout| layout.viewport_rows),
                offset,
                cell_height,
            );
            rows = rows.top(px(offset + visible.start as f32 * cell_height));
            for row in visible {
                let source = self
                    .painted_unwrapped
                    .as_ref()
                    .map_or(row, |layout| layout.first_source_row(row));
                let summary = self
                    .painted_unwrapped
                    .as_ref()
                    .is_some_and(|layout| layout.summary(row).is_some());
                let marker =
                    if !summary && snapshot.row(source).is_some_and(|row| row.wraps_to_next) {
                        "↪"
                    } else {
                        ""
                    };
                rows = rows.child(
                    div()
                        .h(px(cell_height))
                        .line_height(px(cell_height))
                        .flex_none()
                        .child(marker),
                );
            }
        }
        Some(
            div()
                .relative()
                .w(px(self.cell_width))
                .h_full()
                .flex_none()
                .overflow_hidden()
                .text_color(rgb(self.palette.text_dimmed))
                .child(rows)
                .into_any_element(),
        )
    }

    pub(super) fn handle_unwrapped_scroll(
        &mut self,
        event: &ScrollEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(layout) = self.painted_unwrapped.clone() else {
            return false;
        };
        let (x, y) = match event.delta {
            ScrollDelta::Lines(delta) => (delta.x * self.cell_width * 3.0, delta.y),
            ScrollDelta::Pixels(delta) => (
                f32::from(delta.x),
                f32::from(delta.y) / self.cell_height.max(1.0),
            ),
        };
        let vertical_pixels = y * self.cell_height;
        let horizontal = if event.modifiers.shift {
            if x.abs() < f32::EPSILON {
                vertical_pixels
            } else {
                x
            }
        } else if x.abs() > vertical_pixels.abs() {
            x
        } else {
            0.0
        };
        if horizontal.is_finite() && horizontal != 0.0 {
            let max =
                layout.width_cols.saturating_sub(layout.viewport_cols) as f32 * self.cell_width;
            let next = (f32::from(self.horizontal_scroll.offset().x) + horizontal).clamp(-max, 0.0);
            self.horizontal_scroll.set_offset(point(px(next), px(0.0)));
            cx.notify();
            cx.stop_propagation();
            return true;
        }
        if !y.is_finite() || y == 0.0 {
            return false;
        }
        let delta =
            crate::features::terminal::terminal_runtime::terminal_scroll_delta_lines_from_raw(
                &mut self.logical_scroll_residual_lines,
                y,
            );
        if delta != 0 {
            let next = layout.offset_for_scroll(self.scroll_offset, delta);
            let mut state = self.current_scroll_visual_state();
            state.scroll_offset = next;
            state.display_offset = next;
            state.scroll_residual_lines = 0.0;
            self.apply_scroll_visual_state(state);
            cx.notify();
            if let Some(app) = self.app.as_ref().and_then(gpui::WeakEntity::upgrade) {
                let id = self.session_id.clone();
                cx.defer(move |cx| {
                    app.update(cx, |app, cx| {
                        app.set_terminal_scroll_offset_for_session_state_only(Some(&id), next);
                        app.request_terminal_frame_snapshot_for_user_scroll(&id, next);
                        app.notify_terminal_scroll_after_state_change(Some(&id), cx);
                        app.queue_terminal_selection_scroll_rehit(&id, cx);
                    })
                });
            }
        }
        cx.stop_propagation();
        true
    }
}
