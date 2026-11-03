use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;

use super::{SelectionTextError, TerminalSelection, selected_row_text};
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
            let (left, right) = selection
                .cols_for_absolute_line(index)
                .ok_or(SelectionTextError::OutOfBounds)?;
            let slice = selected_row_text(&cells, left, right);
            let wraps_to_next = self.term.grid()[line][Column(cols - 1)]
                .flags
                .contains(Flags::WRAPLINE);
            let slice = if wraps_to_next && right >= cols {
                slice.as_str()
            } else {
                slice.trim_end()
            };
            let wrapped = line > topmost
                && self.term.grid()[Line(line.0 - 1)][Column(cols - 1)]
                    .flags
                    .contains(Flags::WRAPLINE);
            let empty_endpoint = index == end.line && end.col == 0 && slice.is_empty();
            if index > start.line && !wrapped && !empty_endpoint {
                text.push('\n');
            }
            text.push_str(slice);
        }
        Ok((!text.is_empty()).then_some(text))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{SelectionTextError, TerminalBufferCellPos, TerminalSelection};
    use crate::TerminalScreen;

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
