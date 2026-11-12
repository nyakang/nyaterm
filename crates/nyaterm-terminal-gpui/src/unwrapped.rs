//! Display geometry shared by painting, pointer input and IME positioning.
use std::sync::Arc;

use nyaterm_terminal::logical_lines::TerminalLogicalLineIndex;
use nyaterm_terminal::{TerminalLineId, TerminalSnapshot};

#[derive(Debug, Clone, PartialEq)]
pub struct TerminalUnwrappedLayout {
    index: Arc<TerminalLogicalLineIndex>,
    pub projection: Option<Arc<nyaterm_terminal::display_rows::TerminalDisplayRows>>,
    pub unwrapped: bool,
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
        if snapshot.display_rows.is_some() {
            return Self::projected(snapshot, display_offset, viewport_rows, horizontal_offset);
        }
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
            projection: None,
            unwrapped: true,
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

    fn projected(
        snapshot: &TerminalSnapshot,
        offset: usize,
        viewport: usize,
        horizontal: f32,
    ) -> Option<Self> {
        let projection = snapshot.display_rows.clone()?;
        let unwrapped = snapshot.logical_lines.is_some();
        let index = snapshot.logical_lines.clone().unwrap_or_default();
        let source_start = snapshot
            .total_rows
            .saturating_sub(snapshot.display_offset)
            .saturating_sub(snapshot.row_count());
        let top = projection.top(offset, viewport);
        let mut layout = Self {
            index,
            projection: Some(projection.clone()),
            unwrapped,
            source_start,
            logical_top: top,
            viewport_rows: viewport,
            viewport_cols: snapshot.cols,
            horizontal_offset: if unwrapped { horizontal.max(0.0) } else { 0.0 },
            width_cols: snapshot.cols,
            segments: Vec::new(),
        };
        for row in 0..viewport.min(projection.rows().saturating_sub(top)) {
            if projection.summary(top + row).is_some() {
                continue;
            }
            let start = projection.source_row(top + row);
            let end = if unwrapped {
                layout.index.group(start).end
            } else {
                start + 1
            };
            if start < source_start || end > source_start + snapshot.row_count() {
                return None;
            }
            if unwrapped {
                layout.width_cols = layout
                    .width_cols
                    .max(layout.index.logical_col(end - 1, snapshot.cols));
                let logical = layout.index.logical_row(start);
                let (left, _) = layout
                    .index
                    .source_position(logical, layout.horizontal_offset.floor() as usize);
                let (right, _) = layout.index.source_position(
                    logical,
                    (layout.horizontal_offset + snapshot.cols as f32).ceil() as usize,
                );
                for physical in left..=right {
                    layout.segments.push(TerminalUnwrappedSegment {
                        source_row: physical - source_start,
                        display_row: row,
                        column_offset: layout.index.logical_col(physical, 0) as f32
                            - layout.horizontal_offset,
                    });
                }
            } else {
                layout.segments.push(TerminalUnwrappedSegment {
                    source_row: start - source_start,
                    display_row: row,
                    column_offset: 0.0,
                });
            }
        }
        Some(layout)
    }

    pub fn summary(&self, row: usize) -> Option<nyaterm_terminal::command_blocks::CommandBlockId> {
        self.projection.as_ref()?.summary(self.logical_top + row)
    }

    /// Raw snapshot cell -> display cell. Horizontal coordinates may be clipped.
    pub fn display_position(&self, source_row: usize, col: usize) -> Option<(usize, f32)> {
        let physical = self.source_start.checked_add(source_row)?;
        if let Some(projection) = &self.projection {
            if projection.hidden(physical) {
                return None;
            }
            let display = projection.display_row(physical);
            if display < self.logical_top || display >= self.logical_top + self.viewport_rows {
                return None;
            }
            let x = if self.unwrapped {
                self.index.logical_col(physical, col) as f32 - self.horizontal_offset
            } else {
                col as f32
            };
            return Some((display - self.logical_top, x));
        }
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
        if let Some(projection) = &self.projection {
            let physical = projection
                .source_row((self.logical_top + row).min(projection.rows().saturating_sub(1)));
            if !self.unwrapped || self.summary(row).is_some() {
                return (
                    physical.saturating_sub(self.source_start),
                    (col.max(0.0) as usize).min(self.viewport_cols.saturating_sub(1)),
                );
            }
            let (physical, column) = self.index.source_position(
                self.index.logical_row(physical),
                (col.max(0.0) + self.horizontal_offset).floor() as usize,
            );
            return (physical.saturating_sub(self.source_start), column);
        }
        let logical = (self.logical_top + row.min(self.viewport_rows.saturating_sub(1)))
            .min(self.index.logical_rows().saturating_sub(1));
        let (physical, column) = self.index.source_position(
            logical,
            (col.max(0.0) + self.horizontal_offset).floor() as usize,
        );
        (physical.saturating_sub(self.source_start), column)
    }

    pub fn first_source_row(&self, display_row: usize) -> usize {
        if let Some(projection) = &self.projection {
            return projection
                .source_row(self.logical_top + display_row)
                .saturating_sub(self.source_start);
        }
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
        self.projection
            .as_ref()
            .map_or_else(|| self.index.logical_rows(), |index| index.rows())
    }

    pub fn offset_for_scroll(&self, offset: usize, delta: i32) -> usize {
        if let Some(index) = &self.projection {
            return index.scroll(offset, self.viewport_rows, delta);
        }
        self.index
            .offset_for_logical_scroll(offset, self.viewport_rows, delta)
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalUnwrappedLayout;
    use nyaterm_terminal::TerminalScreen;

    #[test]
    fn folded_rows_share_paint_pointer_and_cursor_mapping_in_both_wrap_modes() {
        for unwrapped in [false, true] {
            let mut screen = TerminalScreen::new(8, 8);
            screen.set_logical_line_snapshots(unwrapped);
            screen.advance(b"$ \x1b]133;B\x07cmd\r\n\x1b]133;C\x07abcdefghijk\r\nsecond\r\n\x1b]133;D;0\x07\x1b]133;A\x07$ \x1b]133;B\x07");
            let id = screen.snapshot().command_blocks[0].id;
            screen.set_collapsed_commands([id].into());
            let snapshot = screen.snapshot();
            let layout = TerminalUnwrappedLayout::new(&snapshot, 0, 8, 0.0).unwrap();
            assert_eq!(layout.summary(1), Some(id));
            assert_eq!(layout.display_position(1, 0), None);
            assert_eq!(layout.display_position(2, 0), None);
            assert_eq!(
                layout.display_position(snapshot.cursor.row, snapshot.cursor.col),
                Some((2, 2.0))
            );
            for segment in &layout.segments {
                assert_ne!(segment.display_row, 1);
                for col in 0..snapshot.cols {
                    let (row, x) = layout.display_position(segment.source_row, col).unwrap();
                    assert_eq!(layout.source_position(row, x), (segment.source_row, col));
                }
            }
        }
    }

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
