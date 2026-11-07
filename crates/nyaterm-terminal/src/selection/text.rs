use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;

use super::{SelectionTextError, TerminalSelection};
use crate::{CellStyle, RenderCell, TerminalScreen, cell_text, render_cell_width};

impl TerminalScreen {
    /// Read only the selected grid rows, independently of retained paint snapshots.
    pub fn text_for_selection(
        &self,
        selection: TerminalSelection,
    ) -> Result<Option<String>, SelectionTextError> {
        if selection.all_buffer {
            return Ok(self.all_text());
        }
        if selection.is_empty() {
            return Ok(None);
        }
        let cols = self.term.columns();
        let (start, end) = selection
            .cell_range(cols)
            .ok_or(SelectionTextError::OutOfBounds)?;
        if end.line >= self.total_rows() {
            return Err(SelectionTextError::OutOfBounds);
        }
        let topmost = self.term.topmost_line();
        let mut text = String::new();
        for index in start.line..=end.line {
            let line = Line(topmost.0 + index as i32);
            let cells = (0..cols)
                .map(|col| {
                    let cell = &self.term.grid()[line][Column(col)];
                    RenderCell {
                        text: cell_text(cell),
                        width: render_cell_width(cell),
                        style: CellStyle::default(),
                        hyperlink: None,
                    }
                })
                .collect::<Vec<_>>();
            let wraps_to_next = self.term.grid()[line][Column(cols - 1)]
                .flags
                .contains(Flags::WRAPLINE);
            let wrapped = line > topmost
                && self.term.grid()[Line(line.0 - 1)][Column(cols - 1)]
                    .flags
                    .contains(Flags::WRAPLINE);
            selection
                .append_row_text(&mut text, &cells, index, wrapped, wraps_to_next)
                .ok_or(SelectionTextError::OutOfBounds)?;
        }
        Ok((!text.is_empty()).then_some(text))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        SelectionTextError, TerminalBufferCellPos, TerminalCellSide, TerminalSelection,
        TerminalSelectionEndpoint, TerminalSelectionMode,
    };
    use crate::TerminalScreen;

    #[test]
    fn single_row_block_copy_uses_half_cell_edges_and_clips_to_the_grid() {
        let mut screen = TerminalScreen::new(5, 2);
        screen.advance(b"ABCDE");
        let mut selection = TerminalSelection::with_anchor_and_mode(
            TerminalSelectionEndpoint::new(
                TerminalBufferCellPos::new(0, 1),
                TerminalCellSide::Right,
            ),
            TerminalSelectionMode::Block,
        );
        selection.head = TerminalBufferCellPos::new(0, 4).into();
        assert_eq!(
            screen.text_for_selection(selection).unwrap().as_deref(),
            Some("CD")
        );
        selection.head = TerminalBufferCellPos::new(0, 2).into();
        assert_eq!(screen.text_for_selection(selection).unwrap(), None);
        selection.head = TerminalBufferCellPos::new(0, 50).into();
        assert_eq!(
            screen.text_for_selection(selection).unwrap().as_deref(),
            Some("CDE")
        );
        selection.anchor = TerminalBufferCellPos::new(0, 6).into();
        assert_eq!(
            screen.text_for_selection(selection),
            Err(SelectionTextError::OutOfBounds)
        );
        selection.anchor = TerminalBufferCellPos::new(1, 0).into();
        selection.head = TerminalBufferCellPos::new(1, 3).into();
        assert_eq!(
            screen.text_for_selection(selection).unwrap().as_deref(),
            Some("   ")
        );
    }

    #[test]
    fn block_copy_preserves_spaces_blank_rows_and_physical_soft_wraps() {
        let mut screen = TerminalScreen::new(5, 5);
        screen.advance(b"abcdefghij\r\nx\r\n\r\nend");
        let mut selection = TerminalSelection::with_anchor_and_mode(
            TerminalBufferCellPos::new(0, 1),
            TerminalSelectionMode::Block,
        );
        selection.head = TerminalBufferCellPos::new(4, 4).into();
        assert_eq!(
            screen.text_for_selection(selection).unwrap().as_deref(),
            Some("bcd\nghi\n   \n   \nnd ")
        );
        assert_eq!(
            screen.text_for_selection(TerminalSelection {
                anchor: selection.head,
                head: selection.anchor,
                ..selection
            }),
            screen.text_for_selection(selection)
        );
        selection.head = TerminalBufferCellPos::new(5, 4).into();
        assert_eq!(
            screen.text_for_selection(selection),
            Err(SelectionTextError::OutOfBounds)
        );
    }

    #[test]
    fn block_copy_expands_wide_edges_on_each_row_independently() {
        for (source, cols, left, right, expected) in [
            ("a界\u{301}z\r\nabcde\r\na😀z", 8, 2, 3, "界\u{301}\nc\n😀"),
            ("e\u{301}中\r\nabc", 8, 0, 2, "e\u{301}中\nab"),
            ("abc界x", 4, 2, 4, "c\nx "),
        ] {
            let mut screen = TerminalScreen::new(cols, 3);
            screen.advance(source.as_bytes());
            let end_line = if source.contains("abcde") { 2 } else { 1 };
            let mut selection = TerminalSelection::with_anchor_and_mode(
                TerminalBufferCellPos::new(0, left),
                TerminalSelectionMode::Block,
            );
            selection.head = TerminalBufferCellPos::new(end_line, right).into();
            assert_eq!(
                screen.text_for_selection(selection).unwrap().as_deref(),
                Some(expected)
            );
        }
    }

    #[test]
    fn selection_reads_history_in_both_directions_and_joins_soft_wraps() {
        let mut screen = TerminalScreen::new(5, 2);
        screen.advance(b"abcdef\r\nghij\r\nlast");
        let selection = TerminalSelection::from_range(
            TerminalBufferCellPos::new(0, 1),
            TerminalBufferCellPos::new(2, 2),
        );
        assert_eq!(
            screen.text_for_selection(selection).unwrap().as_deref(),
            Some("bcdef\nghi")
        );
        let reversed = TerminalSelection {
            anchor: selection.head,
            head: selection.anchor,
            ..selection
        };
        assert_eq!(
            screen.text_for_selection(reversed),
            screen.text_for_selection(selection)
        );
    }

    #[test]
    fn selection_preserves_wide_glyphs_combining_marks_and_empty_lines() {
        let mut screen = TerminalScreen::new(10, 4);
        screen.advance("a中e\u{301}\r\n\r\nlast".as_bytes());
        let selection = TerminalSelection::from_range(
            TerminalBufferCellPos::new(0, 2),
            TerminalBufferCellPos::new(2, 1),
        );
        assert_eq!(
            screen.text_for_selection(selection).unwrap().as_deref(),
            Some("中e\u{301}\n\nla")
        );
    }

    #[test]
    fn selection_handles_empty_endpoints_and_rejects_incomplete_ranges() {
        let mut screen = TerminalScreen::new(10, 3);
        screen.advance(b"abc\r\n");
        let range = |line, col| {
            TerminalSelection::from_range(
                TerminalBufferCellPos::new(0, 0),
                TerminalBufferCellPos::new(line, col),
            )
        };
        assert_eq!(
            screen.text_for_selection(range(1, 0)).unwrap().as_deref(),
            Some("abc")
        );
        assert_eq!(
            screen.text_for_selection(range(1, 1)).unwrap().as_deref(),
            Some("abc\n")
        );
        let endpoint = TerminalSelection {
            anchor: TerminalBufferCellPos::new(0, 0).into(),
            head: TerminalBufferCellPos::new(1, 0).into(),
            all_buffer: false,
            mode: crate::selection::TerminalSelectionMode::Linear,
        };
        assert_eq!(
            screen.text_for_selection(endpoint).unwrap().as_deref(),
            Some("abc")
        );
        assert_eq!(
            screen.text_for_selection(range(3, 0)),
            Err(SelectionTextError::OutOfBounds)
        );
        assert_eq!(
            screen.text_for_selection(TerminalSelection::with_anchor(TerminalBufferCellPos::new(
                0, 0
            ))),
            Ok(None)
        );
        assert_eq!(
            screen.text_for_selection(TerminalSelection::from_range(
                TerminalBufferCellPos::new(2, 0),
                TerminalBufferCellPos::new(2, 3)
            )),
            Ok(None)
        );
    }
}
