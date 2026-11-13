//! Terminal grid hit testing, gutters and retained-snapshot scroll geometry.
use gpui::{Bounds, Pixels, Point};
use nyaterm_terminal::TerminalSnapshot;
use nyaterm_terminal::selection::TerminalCellPos;
use nyaterm_terminal::snapshot_window::terminal_snapshot_absolute_window;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub struct TerminalHitTestGeometry {
    pub bounds: Bounds<Pixels>,
    pub snapshot: Arc<TerminalSnapshot>,
    pub cell_w: f32,
    pub cell_h: f32,
    pub padding_left: f32,
    pub padding_top: f32,
    pub gutter: f32,
    pub rows: usize,
    pub cols: usize,
    pub display_offset: usize,
    pub viewport_anchor_row: usize,
    pub snapshot_rows: usize,
    pub viewport_rows: usize,
    pub visual_y_offset: f32,
    pub unwrapped: Option<Arc<crate::unwrapped::TerminalUnwrappedLayout>>,
}

pub fn terminal_hit_test_visual_y_offset_px(geometry: TerminalVisualScrollGeometry) -> f32 {
    terminal_effective_visual_scroll_offset_px(geometry)
        - geometry.viewport_anchor_row as f32 * geometry.cell_height.max(1.0)
}

pub fn terminal_cell_for_visual_geometry(
    position: Point<Pixels>,
    geometry: &TerminalHitTestGeometry,
) -> TerminalCellPos {
    let cell_w = geometry.cell_w.max(1.0);
    let cell_h = geometry.cell_h.max(1.0);
    let local_x =
        f32::from(position.x - geometry.bounds.origin.x) - geometry.padding_left - geometry.gutter;
    let local_y = f32::from(position.y - geometry.bounds.origin.y) - geometry.padding_top;
    let snapshot_row = ((local_y - geometry.visual_y_offset) / cell_h)
        .floor()
        .max(0.0) as usize;
    if let Some(layout) = geometry.unwrapped.as_ref() {
        let (source_row, source_col) =
            layout.source_position(snapshot_row, local_x.max(0.0) / cell_w);
        return TerminalCellPos::new(
            source_row.saturating_sub(geometry.viewport_anchor_row),
            source_col,
        );
    }
    let row = snapshot_row.saturating_sub(geometry.viewport_anchor_row);
    let col = (local_x / cell_w).floor().max(0.0) as usize;
    TerminalCellPos::new(
        row.min(geometry.rows.saturating_sub(1)),
        col.min(geometry.cols.saturating_sub(1)),
    )
}

pub fn terminal_cell_side_for_visual_geometry(
    position: Point<Pixels>,
    geometry: &TerminalHitTestGeometry,
) -> nyaterm_terminal::selection::TerminalCellSide {
    use nyaterm_terminal::selection::TerminalCellSide;

    let cell_w = geometry.cell_w.max(1.0);
    let local_x =
        f32::from(position.x - geometry.bounds.origin.x) - geometry.padding_left - geometry.gutter;
    let local_x = local_x
        + geometry
            .unwrapped
            .as_ref()
            .map_or(0.0, |layout| layout.horizontal_offset * cell_w);
    if local_x <= 0.0 || geometry.cols == 0 {
        return TerminalCellSide::Left;
    }
    if (geometry.unwrapped.is_none() && local_x >= geometry.cols as f32 * cell_w)
        || local_x % cell_w >= cell_w * 0.5
    {
        TerminalCellSide::Right
    } else {
        TerminalCellSide::Left
    }
}

pub fn terminal_snapshot_row_for_visual_geometry(
    position: Point<Pixels>,
    geometry: &TerminalHitTestGeometry,
) -> usize {
    let cell_h = geometry.cell_h.max(1.0);
    let local_y = f32::from(position.y - geometry.bounds.origin.y) - geometry.padding_top;
    let row = ((local_y - geometry.visual_y_offset) / cell_h)
        .floor()
        .max(0.0) as usize;
    if let Some(layout) = geometry.unwrapped.as_ref() {
        let x = (f32::from(position.x - geometry.bounds.origin.x)
            - geometry.padding_left
            - geometry.gutter)
            .max(0.0)
            / geometry.cell_w.max(1.0);
        layout.source_position(row, x).0
    } else {
        row
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TerminalGutterMetrics {
    pub timestamp_width: f32,
    pub line_number_width: f32,
    pub gap_width: f32,
    pub trailing_padding_width: f32,
}

impl TerminalGutterMetrics {
    pub fn total_width(self) -> f32 {
        self.timestamp_width + self.line_number_width + self.gap_width + self.trailing_padding_width
    }
}

pub fn terminal_gutter_metrics(
    cell_width: f32,
    show_timestamps: bool,
    timestamp_width_chars: usize,
    show_line_numbers: bool,
    line_number_digits: usize,
) -> TerminalGutterMetrics {
    let cell_width = cell_width.max(1.0);
    let timestamp_width = if show_timestamps {
        (cell_width * timestamp_width_chars.clamp(1, 64) as f32).ceil() + 2.0
    } else {
        0.0
    };
    let line_number_width = if show_line_numbers {
        (cell_width * line_number_digits.max(1) as f32)
            .ceil()
            .max(22.0)
            + 2.0
    } else {
        0.0
    };
    let gap_width = if show_timestamps && show_line_numbers {
        8.0
    } else {
        0.0
    };
    let trailing_padding_width = crate::command_blocks::COMMAND_RAIL_WIDTH;

    TerminalGutterMetrics {
        timestamp_width,
        line_number_width,
        gap_width,
        trailing_padding_width,
    }
}

pub fn terminal_line_number_digits(snapshot: &nyaterm_terminal::TerminalSnapshot) -> usize {
    // `total_rows - display_offset` is the standard upper bound assuming
    // consecutive numbering from the top. However line numbers can exceed
    // that when `LineNumbers` retains its origin across reflows, local
    // clears, and scrollback prunes — e.g. a 2-row viewport with scrollback
    // limited to 2 can still carry row numbers well above 4 when older
    // wrapped lines were discarded from scrollback. Use whichever bound is
    // larger to keep the gutter wide enough.
    let from_rows = snapshot
        .total_rows
        .saturating_sub(snapshot.display_offset)
        .max(1);
    let from_display_line_number = snapshot.max_display_line_number().unwrap_or(1);
    terminal_line_number_digits_for_end(from_rows.max(from_display_line_number))
}

pub fn terminal_line_number_digits_for_end(visible_end: usize) -> usize {
    visible_end.max(1).to_string().len()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalVisualScrollGeometry {
    pub snapshot_pending: bool,
    pub target_offset: usize,
    pub displayed_offset: usize,
    pub residual_lines: f32,
    pub viewport_anchor_row: usize,
    pub snapshot_rows: usize,
    pub viewport_rows: usize,
    pub cell_height: f32,
}

pub fn terminal_visual_scroll_offset_px(
    target_offset: usize,
    displayed_offset: usize,
    residual_lines: f32,
    cell_height: f32,
) -> f32 {
    terminal_visual_scroll_line_delta(target_offset, displayed_offset, residual_lines)
        * cell_height.max(1.0)
}

pub fn terminal_visual_scroll_line_delta(
    target_offset: usize,
    displayed_offset: usize,
    residual_lines: f32,
) -> f32 {
    let line_delta = target_offset as isize - displayed_offset as isize;
    line_delta as f32 + residual_lines
}

pub fn terminal_retained_visual_scroll_line_bounds(
    viewport_anchor_row: usize,
    snapshot_rows: usize,
    viewport_rows: usize,
) -> (f32, f32) {
    let viewport_rows = viewport_rows.max(1);
    let older_rows = viewport_anchor_row.min(snapshot_rows);
    let newer_rows = snapshot_rows.saturating_sub(
        viewport_anchor_row
            .saturating_add(viewport_rows)
            .min(snapshot_rows),
    );
    (-(newer_rows as f32), older_rows as f32)
}

pub fn terminal_effective_visual_scroll_offset_px(geometry: TerminalVisualScrollGeometry) -> f32 {
    let TerminalVisualScrollGeometry {
        snapshot_pending,
        target_offset,
        displayed_offset,
        residual_lines,
        viewport_anchor_row,
        snapshot_rows,
        viewport_rows,
        cell_height,
    } = geometry;
    if !snapshot_pending {
        return terminal_visual_scroll_offset_px(
            target_offset,
            displayed_offset,
            residual_lines,
            cell_height,
        );
    }
    let (min_lines, max_lines) = terminal_retained_visual_scroll_line_bounds(
        viewport_anchor_row,
        snapshot_rows,
        viewport_rows,
    );
    terminal_visual_scroll_line_delta(target_offset, displayed_offset, residual_lines)
        .clamp(min_lines, max_lines)
        * cell_height.max(1.0)
}

pub fn terminal_snapshot_covers_display_offset(
    snapshot: &TerminalSnapshot,
    display_offset: usize,
    viewport_rows: usize,
    scrollback_len: usize,
) -> bool {
    let viewport_rows = viewport_rows.max(1);
    let real_total_rows = scrollback_len.saturating_add(viewport_rows);
    let Some((snapshot_start, snapshot_end)) = terminal_snapshot_absolute_window(snapshot) else {
        return false;
    };
    let desired_end = real_total_rows.saturating_sub(display_offset);
    let desired_start = desired_end.saturating_sub(viewport_rows);
    if let Some(index) = snapshot.display_rows.as_ref() {
        let needed = index.source_window(display_offset, viewport_rows);
        return snapshot_start <= needed.start && needed.end <= snapshot_end;
    }
    if let Some(index) = snapshot.logical_lines.as_ref() {
        if index.physical_rows() != real_total_rows {
            return false;
        }
        let max_top = index.logical_rows().saturating_sub(viewport_rows);
        let top = if display_offset == 0 {
            max_top
        } else {
            index.logical_row(desired_start).min(max_top)
        };
        return snapshot_start <= index.physical_row(top)
            && index.physical_row((top + viewport_rows).min(index.logical_rows())) <= snapshot_end;
    }
    snapshot_start <= desired_start && desired_end <= snapshot_end
}

pub fn terminal_snapshot_anchor_row_for_display_offset(
    snapshot: &TerminalSnapshot,
    display_offset: usize,
    viewport_rows: usize,
    scrollback_len: usize,
) -> usize {
    let viewport_rows = viewport_rows.max(1);
    let real_total_rows = scrollback_len.saturating_add(viewport_rows);
    let desired_end = real_total_rows.saturating_sub(display_offset);
    let desired_start = desired_end.saturating_sub(viewport_rows);
    terminal_snapshot_absolute_window(snapshot)
        .map(|(snapshot_start, _)| desired_start.saturating_sub(snapshot_start))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{
        TerminalHitTestGeometry, terminal_cell_for_visual_geometry, terminal_gutter_metrics,
        terminal_line_number_digits_for_end,
    };
    use gpui::{Point, Size, px};
    use nyaterm_terminal::TerminalScreen;
    use nyaterm_terminal::selection::TerminalCellSide;
    use std::sync::Arc;
    #[test]
    fn half_cell_hit_testing_uses_painted_origin_and_clamps_grid_edges() {
        let snapshot = Arc::new(TerminalScreen::new(4, 3).snapshot());
        for scale in [1.0, 1.25, 1.5] {
            let geometry = TerminalHitTestGeometry {
                unwrapped: None,
                bounds: gpui::bounds(
                    Point {
                        x: px(17.0),
                        y: px(23.0),
                    },
                    Size {
                        width: px(200.0),
                        height: px(100.0),
                    },
                ),
                snapshot: snapshot.clone(),
                cell_w: 7.5 * scale,
                cell_h: 16.0 * scale,
                padding_left: 3.0,
                padding_top: 2.0,
                gutter: 21.0,
                rows: 3,
                cols: 4,
                display_offset: 0,
                viewport_anchor_row: 0,
                snapshot_rows: 3,
                viewport_rows: 3,
                visual_y_offset: -0.375 * 16.0 * scale,
            };
            for (cell_x, col, side) in [
                (-0.75, 0, TerminalCellSide::Left),
                (0.0, 0, TerminalCellSide::Left),
                (0.25, 0, TerminalCellSide::Left),
                (0.5, 0, TerminalCellSide::Right),
                (0.75, 0, TerminalCellSide::Right),
                (1.0, 1, TerminalCellSide::Left),
                (1.5, 1, TerminalCellSide::Right),
                (4.0, 3, TerminalCellSide::Right),
                (4.25, 3, TerminalCellSide::Right),
            ] {
                let point = Point {
                    x: px(41.0 + cell_x * geometry.cell_w),
                    y: px(28.0),
                };
                assert_eq!(
                    super::terminal_cell_side_for_visual_geometry(point, &geometry),
                    side
                );
                assert_eq!(terminal_cell_for_visual_geometry(point, &geometry).col, col);
            }
        }
    }
    #[test]
    fn gutter_metrics_use_same_widths_for_ms_timestamps_and_total_hit_area() {
        let metrics = terminal_gutter_metrics(8.0, true, 14, true, 5);

        assert_eq!(metrics.timestamp_width, 114.0);
        assert_eq!(metrics.line_number_width, 42.0);
        assert_eq!(metrics.gap_width, 8.0);
        assert_eq!(metrics.trailing_padding_width, 19.0);
        assert_eq!(metrics.total_width(), 183.0);
    }
    #[test]
    fn gutter_metrics_expand_with_large_terminal_font() {
        let metrics = terminal_gutter_metrics(18.0, true, 10, true, 5);

        assert!(metrics.timestamp_width > 120.0);
        assert!(metrics.line_number_width > 70.0);
        assert_eq!(
            metrics.total_width(),
            metrics.timestamp_width
                + metrics.line_number_width
                + metrics.gap_width
                + metrics.trailing_padding_width
        );
    }
    #[test]
    fn gutter_line_number_digits_follow_visible_absolute_end() {
        assert_eq!(terminal_line_number_digits_for_end(0), 1);
        assert_eq!(terminal_line_number_digits_for_end(9), 1);
        assert_eq!(terminal_line_number_digits_for_end(10), 2);
        assert_eq!(terminal_line_number_digits_for_end(99_999), 5);
        assert_eq!(terminal_line_number_digits_for_end(100_000), 6);
    }
}
