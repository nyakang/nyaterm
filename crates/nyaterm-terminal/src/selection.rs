//! UI-independent absolute-buffer selection boundaries.

use crate::{RenderCell, TerminalHistoryState, TerminalSnapshot};

mod text;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionTextError {
    OutOfBounds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerminalBufferCellPos {
    pub line: usize,
    pub col: usize,
}

impl TerminalBufferCellPos {
    pub fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerminalCellSide {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerminalSelectionEndpoint {
    pub position: TerminalBufferCellPos,
    pub side: TerminalCellSide,
}

impl TerminalSelectionEndpoint {
    pub fn after_eviction(self, evicted: usize) -> Option<Self> {
        Some(Self {
            position: TerminalBufferCellPos {
                line: self.position.line.checked_sub(evicted)?,
                col: self.position.col,
            },
            side: self.side,
        })
    }

    pub fn new(position: TerminalBufferCellPos, side: TerminalCellSide) -> Self {
        Self { position, side }
    }

    fn boundary(self) -> (usize, usize) {
        (
            self.position.line,
            self.position
                .col
                .saturating_add(usize::from(self.side == TerminalCellSide::Right)),
        )
    }
}

impl From<TerminalBufferCellPos> for TerminalSelectionEndpoint {
    fn from(position: TerminalBufferCellPos) -> Self {
        Self::new(position, TerminalCellSide::Left)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerminalSelectionMode {
    Linear,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerminalSelection {
    pub anchor: TerminalSelectionEndpoint,
    pub head: TerminalSelectionEndpoint,
    pub all_buffer: bool,
    pub mode: TerminalSelectionMode,
}

impl TerminalSelection {
    /// Preserve endpoint direction and sides, clipping only actually lost rows.
    pub fn after_eviction(self, evicted: usize) -> Option<Self> {
        if self.all_buffer || evicted == 0 {
            return Some(self);
        }
        if self.mode == TerminalSelectionMode::Block {
            if self.anchor.position.line.max(self.head.position.line) < evicted {
                return None;
            }
            let clip = |mut endpoint: TerminalSelectionEndpoint| {
                endpoint.position.line = endpoint.position.line.saturating_sub(evicted);
                endpoint
            };
            // A zero-width block may still be a live drag candidate. Keep its
            // anchor while that row survives, so output cannot cancel the drag.
            return Some(Self {
                anchor: clip(self.anchor),
                head: clip(self.head),
                ..self
            });
        }
        let first = TerminalSelectionEndpoint::new(
            TerminalBufferCellPos::new(0, 0),
            TerminalCellSide::Left,
        );
        let selection = Self {
            anchor: self.anchor.after_eviction(evicted).unwrap_or(first),
            head: self.head.after_eviction(evicted).unwrap_or(first),
            all_buffer: false,
            mode: self.mode,
        };
        (!selection.is_empty()).then_some(selection)
    }

    pub fn reanchored_history(
        self,
        previous: TerminalHistoryState,
        current: TerminalHistoryState,
    ) -> Option<Self> {
        self.after_eviction(current.changes_since(previous)?.evicted)
    }

    pub fn with_anchor(anchor: impl Into<TerminalSelectionEndpoint>) -> Self {
        Self::with_anchor_and_mode(anchor, TerminalSelectionMode::Linear)
    }

    pub fn with_anchor_and_mode(
        anchor: impl Into<TerminalSelectionEndpoint>,
        mode: TerminalSelectionMode,
    ) -> Self {
        let anchor = anchor.into();
        Self {
            anchor,
            head: anchor,
            all_buffer: false,
            mode,
        }
    }

    /// Construct a complete, inclusive cell range, including a single cell.
    pub fn from_range(anchor: TerminalBufferCellPos, head: TerminalBufferCellPos) -> Self {
        let forward = (anchor.line, anchor.col) <= (head.line, head.col);
        Self {
            anchor: TerminalSelectionEndpoint::new(
                anchor,
                if forward {
                    TerminalCellSide::Left
                } else {
                    TerminalCellSide::Right
                },
            ),
            head: TerminalSelectionEndpoint::new(
                head,
                if forward {
                    TerminalCellSide::Right
                } else {
                    TerminalCellSide::Left
                },
            ),
            all_buffer: false,
            mode: TerminalSelectionMode::Linear,
        }
    }

    pub fn all_buffer(cols: usize) -> Self {
        Self {
            all_buffer: true,
            ..Self::from_range(
                TerminalBufferCellPos::new(0, 0),
                TerminalBufferCellPos::new(0, cols.saturating_sub(1)),
            )
        }
    }

    fn ordered_endpoints(self) -> (TerminalSelectionEndpoint, TerminalSelectionEndpoint) {
        if self.anchor.boundary() <= self.head.boundary() {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// Raw bounding cells, for locating source snapshots (not for slicing text).
    pub fn ordered(self) -> (TerminalBufferCellPos, TerminalBufferCellPos) {
        let (start, end) = self.ordered_endpoints();
        (start.position, end.position)
    }

    pub fn is_empty(&self) -> bool {
        !self.all_buffer
            && match self.mode {
                TerminalSelectionMode::Linear => self.anchor.boundary() == self.head.boundary(),
                TerminalSelectionMode::Block => self.anchor.boundary().1 == self.head.boundary().1,
            }
    }

    /// Half-open columns. The unbounded end is clipped by the consuming row.
    pub fn cols_for_absolute_line(self, line: usize) -> Option<(usize, usize)> {
        if self.all_buffer {
            return Some((0, usize::MAX));
        }
        if self.is_empty() {
            return None;
        }
        let (start, end) = self.ordered_endpoints();
        let (start_line, start_col) = start.boundary();
        let (end_line, end_col) = end.boundary();
        if line < start_line || line > end_line {
            return None;
        }
        if self.mode == TerminalSelectionMode::Block {
            return Some((start_col.min(end_col), start_col.max(end_col)));
        }
        let left = if line == start_line { start_col } else { 0 };
        let right = if line == end_line {
            end_col
        } else {
            usize::MAX
        };
        (left < right).then_some((left, right))
    }

    /// Inclusive selected cells for adapters which still require cell endpoints.
    /// A block returns bounding corners, not a continuous text interval.
    pub fn cell_range(self, cols: usize) -> Option<(TerminalBufferCellPos, TerminalBufferCellPos)> {
        if self.all_buffer || self.is_empty() || cols == 0 {
            return None;
        }
        let (start, end) = self.ordered_endpoints();
        let (mut start_line, mut start_col) = start.boundary();
        let (mut end_line, end_col) = end.boundary();
        if self.mode == TerminalSelectionMode::Block {
            let left = start_col.min(end_col);
            let right = start_col.max(end_col).min(cols);
            return (left < right).then_some((
                TerminalBufferCellPos::new(start_line, left),
                TerminalBufferCellPos::new(end_line, right.saturating_sub(1)),
            ));
        }
        if start_col >= cols {
            start_line = start_line.checked_add(1)?;
            start_col = 0;
        }
        let end_col = if end_col == 0 {
            end_line = end_line.checked_sub(1)?;
            cols - 1
        } else {
            (end_col - 1).min(cols - 1)
        };
        ((start_line, start_col) <= (end_line, end_col)).then_some((
            TerminalBufferCellPos::new(start_line, start_col),
            TerminalBufferCellPos::new(end_line, end_col),
        ))
    }

    /// Resolve wide-glyph bounds without changing the mouse's original anchors.
    /// Returns None when this snapshot does not cover the selected cells.
    /// Blocks retain their raw bounds; consumers expand each row independently.
    pub fn expanded_for_snapshot(self, snapshot: &TerminalSnapshot) -> Option<Self> {
        if self.all_buffer {
            return Some(self);
        }
        let (start, end) = self.cell_range(snapshot.cols)?;
        let absolute_end = snapshot.total_rows.saturating_sub(snapshot.display_offset);
        let absolute_start = absolute_end.saturating_sub(snapshot.row_count());
        if start.line < absolute_start || end.line >= absolute_end {
            return None;
        }
        // A block's wide edges expand independently on each row. They cannot
        // be represented by widening its two original anchors globally.
        if self.mode == TerminalSelectionMode::Block {
            return Some(self);
        }
        let start_row = snapshot.row(start.line - absolute_start)?;
        let end_row = snapshot.row(end.line - absolute_start)?;
        let (left, right) = self.cols_for_absolute_line(start.line)?;
        let (start_col, _) = expand_selection_range(&start_row.cells, left, right)?;
        let (left, right) = self.cols_for_absolute_line(end.line)?;
        let (_, end_col) = expand_selection_range(&end_row.cells, left, right)?;
        Some(Self::from_range(
            TerminalBufferCellPos::new(start.line, start_col),
            TerminalBufferCellPos::new(end.line, end_col - 1),
        ))
    }

    /// Shared copy policy for live grids and retained snapshots.
    pub fn append_row_text(
        self,
        text: &mut String,
        cells: &[RenderCell],
        line: usize,
        wrapped: bool,
        wraps_to_next: bool,
    ) -> Option<()> {
        let (start, end) = self.cell_range(cells.len())?;
        let (left, right) = self.cols_for_absolute_line(line)?;
        let slice = selected_row_text(cells, left, right);
        let block = self.mode == TerminalSelectionMode::Block;
        let slice = if block || (wraps_to_next && right >= cells.len()) {
            slice.as_str()
        } else {
            slice.trim_end()
        };
        let empty_endpoint = line == end.line && end.col == 0 && slice.is_empty();
        if line > start.line && (block || (!wrapped && !empty_endpoint)) {
            text.push('\n');
        }
        text.push_str(slice);
        Some(())
    }
}

/// Expand only nonempty ranges across a wide glyph's actual trailing spacer.
/// A leading spacer at the right margin is not part of the following row's glyph.
pub fn expand_selection_range(
    cells: &[RenderCell],
    start: usize,
    end: usize,
) -> Option<(usize, usize)> {
    let mut start = start.min(cells.len());
    let mut end = end.min(cells.len());
    if start >= end {
        return None;
    }
    if start > 0 && cells[start].width == 0 && cells[start - 1].width == 2 {
        start -= 1;
    }
    if cells[end - 1].width == 2 && cells.get(end).is_some_and(|cell| cell.width == 0) {
        end += 1;
    }
    Some((start, end))
}

pub fn selected_row_text(cells: &[RenderCell], start: usize, end: usize) -> String {
    let Some((start, end)) = expand_selection_range(cells, start, end) else {
        return String::new();
    };
    let mut text = String::new();
    for cell in &cells[start..end] {
        if cell.width == 0 {
            continue;
        }
        if cell.text.is_empty() {
            text.push(' ');
        } else {
            text.push_str(cell.text());
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use crate::TerminalScreen;
    use crate::selection::{
        TerminalBufferCellPos, TerminalCellSide, TerminalSelection, TerminalSelectionEndpoint,
        TerminalSelectionMode, expand_selection_range, selected_row_text,
    };

    #[test]
    fn blocks_use_independent_half_open_columns_in_all_four_directions() {
        for (top, bottom) in [(2, 5), (5, 2)] {
            for (left, right) in [(3, 7), (7, 3)] {
                let mut selection = TerminalSelection::with_anchor_and_mode(
                    TerminalBufferCellPos::new(top, left),
                    TerminalSelectionMode::Block,
                );
                selection.head = TerminalSelectionEndpoint::new(
                    TerminalBufferCellPos::new(bottom, right),
                    TerminalCellSide::Right,
                );
                let expected = (left.min(right + 1), left.max(right + 1));
                for row in 2..=5 {
                    assert_eq!(selection.cols_for_absolute_line(row), Some(expected));
                }
                assert_eq!(selection.cols_for_absolute_line(1), None);
                assert_eq!(selection.cols_for_absolute_line(6), None);
                assert_eq!(
                    selection.cell_range(12),
                    Some((
                        TerminalBufferCellPos::new(2, expected.0),
                        TerminalBufferCellPos::new(5, expected.1 - 1),
                    ))
                );
                selection.head = TerminalBufferCellPos::new(bottom, left).into();
                assert!(selection.is_empty());
                assert_eq!(selection.cell_range(12), None);
            }
        }
    }

    #[test]
    fn block_eviction_clips_rows_without_changing_columns_or_sides() {
        let mut selection = TerminalSelection::with_anchor_and_mode(
            TerminalBufferCellPos::new(1, 6),
            TerminalSelectionMode::Block,
        );
        selection.head = TerminalSelectionEndpoint::new(
            TerminalBufferCellPos::new(5, 2),
            TerminalCellSide::Right,
        );
        for selection in [
            selection,
            TerminalSelection {
                anchor: selection.head,
                head: selection.anchor,
                ..selection
            },
        ] {
            let clipped = selection.after_eviction(3).unwrap();
            assert_eq!(clipped.mode, TerminalSelectionMode::Block);
            assert_eq!(clipped.anchor.side, selection.anchor.side);
            assert_eq!(clipped.head.side, selection.head.side);
            assert_eq!(clipped.anchor.position.col, selection.anchor.position.col);
            assert_eq!(clipped.head.position.col, selection.head.position.col);
            for row in 0..=2 {
                assert_eq!(clipped.cols_for_absolute_line(row), Some((3, 6)));
            }
            assert_eq!(selection.after_eviction(6), None);
        }
    }

    #[test]
    fn block_pending_anchor_survives_eviction_of_earlier_history_rows() {
        let candidate = TerminalSelection::with_anchor_and_mode(
            TerminalBufferCellPos::new(5, 3),
            TerminalSelectionMode::Block,
        );
        let rebased = candidate.after_eviction(2).unwrap();
        assert!(rebased.is_empty());
        assert_eq!(rebased.anchor.position, TerminalBufferCellPos::new(3, 3));
        assert_eq!(rebased.head, rebased.anchor);
        assert_eq!(candidate.after_eviction(6), None);
    }

    #[test]
    fn eviction_preserves_selected_wide_glyphs_and_soft_wrapped_text() {
        let mut screen = TerminalScreen::new(8, 3);
        screen.set_scrollback_limit(8);
        screen.advance("宽字符abc\r\n".repeat(12).as_bytes());
        let history = screen.history_state();
        let selection = TerminalSelection::from_range(
            TerminalBufferCellPos::new(3, 0),
            TerminalBufferCellPos::new(5, 5),
        );
        let text = screen.text_for_selection(selection).unwrap();
        screen.advance(b"next\r\n");
        let rebased = selection
            .reanchored_history(history, screen.history_state())
            .unwrap();
        assert_eq!(screen.text_for_selection(rebased).unwrap(), text);
    }

    #[test]
    fn eviction_clips_only_lost_cells_and_preserves_selection_direction() {
        for (anchor, head) in [
            (
                TerminalBufferCellPos::new(1, 2),
                TerminalBufferCellPos::new(5, 3),
            ),
            (
                TerminalBufferCellPos::new(5, 3),
                TerminalBufferCellPos::new(1, 2),
            ),
        ] {
            let selection = TerminalSelection::from_range(anchor, head);
            let clipped = selection.after_eviction(3).unwrap();
            assert_eq!(
                clipped.ordered(),
                (
                    TerminalBufferCellPos::new(0, 0),
                    TerminalBufferCellPos::new(2, 3)
                )
            );
            assert_eq!(
                clipped.anchor.position.line > clipped.head.position.line,
                anchor.line > head.line
            );
            assert_eq!(clipped.cols_for_absolute_line(0), Some((0, usize::MAX)));
            assert!(selection.after_eviction(6).is_none());
        }
        let boundary = TerminalSelection {
            anchor: TerminalSelectionEndpoint::from(TerminalBufferCellPos::new(1, 0)),
            head: TerminalSelectionEndpoint::from(TerminalBufferCellPos::new(3, 0)),
            all_buffer: false,
            mode: crate::selection::TerminalSelectionMode::Linear,
        };
        assert!(boundary.after_eviction(3).is_none());
        let all = TerminalSelection::all_buffer(16);
        assert_eq!(all.after_eviction(100), Some(all));
    }

    #[test]
    fn half_cell_boundaries_select_identically_in_both_directions() {
        for (a_col, a_side, b_col, b_side, expected) in [
            (
                0,
                TerminalCellSide::Left,
                1,
                TerminalCellSide::Left,
                Some((0, 1)),
            ),
            (
                0,
                TerminalCellSide::Left,
                1,
                TerminalCellSide::Right,
                Some((0, 2)),
            ),
            (
                0,
                TerminalCellSide::Left,
                0,
                TerminalCellSide::Right,
                Some((0, 1)),
            ),
            (0, TerminalCellSide::Right, 1, TerminalCellSide::Left, None),
            (0, TerminalCellSide::Right, 0, TerminalCellSide::Right, None),
        ] {
            let a = TerminalSelectionEndpoint::new(TerminalBufferCellPos::new(0, a_col), a_side);
            let b = TerminalSelectionEndpoint::new(TerminalBufferCellPos::new(0, b_col), b_side);
            for (anchor, head) in [(a, b), (b, a)] {
                let selection = TerminalSelection {
                    anchor,
                    head,
                    all_buffer: false,
                    mode: crate::selection::TerminalSelectionMode::Linear,
                };
                assert_eq!(selection.cols_for_absolute_line(0), expected);
                assert_eq!(selection.is_empty(), expected.is_none());
            }
        }
    }

    #[test]
    fn next_row_left_edge_excludes_the_entire_endpoint_row() {
        let selection = TerminalSelection {
            anchor: TerminalBufferCellPos::new(0, 0).into(),
            head: TerminalBufferCellPos::new(1, 0).into(),
            all_buffer: false,
            mode: crate::selection::TerminalSelectionMode::Linear,
        };
        assert_eq!(selection.cols_for_absolute_line(0), Some((0, usize::MAX)));
        assert_eq!(selection.cols_for_absolute_line(1), None);
        assert_eq!(
            selection.cell_range(5),
            Some((
                TerminalBufferCellPos::new(0, 0),
                TerminalBufferCellPos::new(0, 4)
            ))
        );
    }

    #[test]
    fn complete_ranges_include_single_cells_and_reverse_multiline_endpoints() {
        let selection = TerminalSelection::from_range(
            TerminalBufferCellPos::new(0, 2),
            TerminalBufferCellPos::new(0, 2),
        );
        assert_eq!(selection.cols_for_absolute_line(0), Some((2, 3)));
        let selection = TerminalSelection::from_range(
            TerminalBufferCellPos::new(12, 4),
            TerminalBufferCellPos::new(10, 2),
        );
        assert_eq!(selection.cols_for_absolute_line(10), Some((2, usize::MAX)));
        assert_eq!(selection.cols_for_absolute_line(11), Some((0, usize::MAX)));
        assert_eq!(selection.cols_for_absolute_line(12), Some((0, 5)));
        assert_eq!(
            TerminalSelection::all_buffer(80).cols_for_absolute_line(10000),
            Some((0, usize::MAX))
        );
    }

    #[test]
    fn either_half_of_a_wide_glyph_copies_and_highlights_the_whole_glyph() {
        let mut screen = TerminalScreen::new(16, 2);
        screen.advance("a界\u{301}😀e\u{301}z".as_bytes());
        let snapshot = screen.snapshot();
        let cells = &snapshot.row(0).unwrap().cells;
        for (start, end, range, text) in [
            (1, 2, (1, 3), "界\u{301}"),
            (2, 3, (1, 3), "界\u{301}"),
            (3, 4, (3, 5), "😀"),
            (4, 5, (3, 5), "😀"),
            (5, 6, (5, 6), "e\u{301}"),
        ] {
            assert_eq!(expand_selection_range(cells, start, end), Some(range));
            assert_eq!(selected_row_text(cells, start, end), text);
        }
        assert_eq!(expand_selection_range(cells, 2, 2), None);
    }

    #[test]
    fn leading_wide_spacer_does_not_shift_following_rows_copy_columns() {
        let mut screen = TerminalScreen::new(4, 3);
        screen.advance("abc界x".as_bytes());
        let snapshot = screen.snapshot();
        assert_eq!(selected_row_text(&snapshot.row(0).unwrap().cells, 3, 4), "");
        assert_eq!(
            selected_row_text(&snapshot.row(1).unwrap().cells, 2, 3),
            "x"
        );
    }

    #[test]
    fn derived_snapshot_range_expands_wide_edges_without_changing_mouse_sides() {
        let mut screen = TerminalScreen::new(8, 2);
        screen.advance("a界😀z".as_bytes());
        let snapshot = screen.snapshot();
        let selection = TerminalSelection {
            anchor: TerminalBufferCellPos::new(0, 2).into(),
            head: TerminalBufferCellPos::new(0, 4).into(),
            all_buffer: false,
            mode: crate::selection::TerminalSelectionMode::Linear,
        };
        let expanded = selection.expanded_for_snapshot(&snapshot).unwrap();
        assert_eq!(expanded.cols_for_absolute_line(0), Some((1, 5)));
        assert_eq!(selection.cols_for_absolute_line(0), Some((2, 4)));
        assert_eq!(selection.anchor.side, TerminalCellSide::Left);
        assert!(
            TerminalSelection::with_anchor(TerminalBufferCellPos::new(0, 2))
                .expanded_for_snapshot(&snapshot)
                .is_none()
        );
        let outside = TerminalSelection::from_range(
            TerminalBufferCellPos::new(3, 0),
            TerminalBufferCellPos::new(3, 1),
        );
        assert!(outside.expanded_for_snapshot(&snapshot).is_none());
    }
}

/// Inclusive cell coordinate inside the visible terminal grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerminalCellPos {
    pub row: usize,
    pub col: usize,
}

impl TerminalCellPos {
    pub fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}
