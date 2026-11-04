//! Display geometry shared by painting, pointer input and IME positioning.
use std::sync::Arc;

use nyaterm_terminal::logical_lines::TerminalLogicalLineIndex;
use nyaterm_terminal::{TerminalLineId, TerminalSnapshot};

#[derive(Debug, Clone, PartialEq)]
pub struct TerminalUnwrappedLayout {
    index: Arc<TerminalLogicalLineIndex>,
    source_start: usize,
    pub logical_top: usize,
    pub viewport_rows: usize,
    pub viewport_cols: usize,
    pub horizontal_offset: f32,
    pub width_cols: usize,
    pub segments: Vec<TerminalUnwrappedSegment>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerminalUnwrappedSegment {
    pub source_row: usize,
    pub display_row: usize,
    pub column_offset: f32,
}

impl TerminalUnwrappedLayout {
    pub fn new(
        snapshot: &TerminalSnapshot,
        display_offset: usize,
        viewport_rows: usize,
        horizontal_offset: f32,
    ) -> Option<Self> {
        let index = snapshot.logical_lines.clone()?;
        let source_start = snapshot
            .total_rows
            .saturating_sub(snapshot.display_offset)
            .saturating_sub(snapshot.row_count());
        let anchor = snapshot
            .history
            .scrollback_len
            .saturating_sub(display_offset);
        let max_top = index.logical_rows().saturating_sub(viewport_rows);
        let logical_top = if display_offset == 0 {
            max_top
        } else {
            index.logical_row(anchor).min(max_top)
        };
        let horizontal_offset = if horizontal_offset.is_finite() {
            horizontal_offset.max(0.0)
        } else {
            0.0
        };
        let mut layout = Self {
            index,
            source_start,
            logical_top,
            viewport_rows,
            viewport_cols: snapshot.cols,
            horizontal_offset,
            width_cols: snapshot.cols,
            segments: Vec::new(),
        };
        for display_row in 0..viewport_rows {
            let logical = logical_top + display_row;
            if logical >= layout.index.logical_rows() {
                break;
            }
            let start = layout.index.physical_row(logical);
            let end = layout.index.physical_row(logical + 1);
            // Do not mislabel a partial cached group as a complete logical line.
            if start < source_start || end > source_start + snapshot.row_count() {
                return None;
            }
            layout.width_cols = layout
                .width_cols
                .max(layout.index.logical_col(end - 1, snapshot.cols));
            let (left, _) = layout
                .index
                .source_position(logical, horizontal_offset.floor() as usize);
            let (right, _) = layout.index.source_position(
                logical,
                (horizontal_offset + snapshot.cols as f32).ceil() as usize,
            );
            for physical in left..=right {
                layout.segments.push(TerminalUnwrappedSegment {
                    source_row: physical - source_start,
                    display_row,
                    column_offset: layout.index.logical_col(physical, 0) as f32 - horizontal_offset,
                });
            }
        }
        Some(layout)
    }

    /// Raw snapshot cell -> display cell. Horizontal coordinates may be clipped.
    pub fn display_position(&self, source_row: usize, col: usize) -> Option<(usize, f32)> {
        let physical = self.source_start.checked_add(source_row)?;
        if physical >= self.index.physical_rows() || col >= self.index.row_width(physical) {
            return None;
        }
        let logical = self.index.logical_row(physical);
        if logical < self.logical_top || logical >= self.logical_top + self.viewport_rows {
            return None;
        }
        Some((
            logical - self.logical_top,
            self.index.logical_col(physical, col) as f32 - self.horizontal_offset,
        ))
    }

    /// Display cell -> raw snapshot cell; padded space clamps to the logical end.
    pub fn source_position(&self, row: usize, col: f32) -> (usize, usize) {
        let logical = (self.logical_top + row.min(self.viewport_rows.saturating_sub(1)))
            .min(self.index.logical_rows().saturating_sub(1));
        let (physical, column) = self.index.source_position(
            logical,
            (col.max(0.0) + self.horizontal_offset).floor() as usize,
        );
        (physical.saturating_sub(self.source_start), column)
    }

    pub fn first_source_row(&self, display_row: usize) -> usize {
        self.index
            .physical_row(self.logical_top + display_row)
            .saturating_sub(self.source_start)
    }

    /// Resolve a target on any soft-wrapped source row to its displayed first row.
    pub fn target_line_for_display_row(
        &self,
        snapshot: &TerminalSnapshot,
        display_row: usize,
        target: Option<TerminalLineId>,
    ) -> Option<TerminalLineId> {
        let target = target?;
        let first = snapshot.rows().first()?.line_id?;
        if first.epoch != target.epoch {
            return None;
        }
        let source = usize::try_from(target.logical_line.checked_sub(first.logical_line)?).ok()?;
        let (row, _) = self.display_position(source, 0)?;
        if row != display_row {
            return None;
        }
        snapshot.row(self.first_source_row(display_row))?.line_id
    }

    pub fn logical_rows(&self) -> usize {
        self.index.logical_rows()
    }

    pub fn offset_for_scroll(&self, offset: usize, delta: i32) -> usize {
        self.index
            .offset_for_logical_scroll(offset, self.viewport_rows, delta)
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalUnwrappedLayout;
    use nyaterm_terminal::TerminalScreen;

    #[test]
    fn display_and_pointer_mapping_share_scrolled_source_cells() {
        let mut screen = TerminalScreen::new(4, 3);
        screen.set_logical_line_snapshots(true);
        screen.advance("abc中文e\u{301}FG\r\nlast".as_bytes());
        let snapshot = screen.snapshot();
        let layout = TerminalUnwrappedLayout::new(&snapshot, 0, 3, 2.5).unwrap();
        for segment in &layout.segments {
            for col in 0..snapshot.cols {
                if let Some((row, x)) = layout.display_position(segment.source_row, col)
                    && x >= 0.0
                    && x < snapshot.cols as f32
                {
                    assert_eq!(layout.source_position(row, x), (segment.source_row, col));
                }
            }
        }
        assert!(layout.segments.len() <= 3 * 3);
    }

    #[test]
    fn long_line_paint_segments_are_bounded_by_the_viewport() {
        let mut screen = TerminalScreen::new(80, 4);
        screen.set_logical_line_snapshots(true);
        screen.advance("x".repeat(80 * 2000).as_bytes());
        let snapshot = screen.snapshot();
        assert!(snapshot.row_count() >= 2000);
        let layout = TerminalUnwrappedLayout::new(&snapshot, 0, 4, 80.0 * 1000.0 + 5.0).unwrap();
        assert!(layout.width_cols >= 80 * 2000);
        assert!(layout.segments.len() <= 12);
        assert!(
            layout
                .segments
                .iter()
                .all(|segment| segment.source_row >= 1000)
        );
    }

    #[test]
    fn missing_source_rows_do_not_create_a_partial_logical_line() {
        let mut screen = TerminalScreen::new(4, 2);
        screen.set_logical_line_snapshots(true);
        screen.advance(b"abcdefghijklmno");
        let mut snapshot = screen.snapshot();
        let mut rows = snapshot.rows().to_vec();
        rows.remove(0);
        snapshot.row_data = rows.into();
        assert!(TerminalUnwrappedLayout::new(&snapshot, 0, 2, 0.0).is_none());
    }
}
