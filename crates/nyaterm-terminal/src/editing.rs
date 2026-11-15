//! Match a predicted shell input against actual cells, never display strings.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use crate::{ShellInputAnchor, TerminalLineId, TerminalSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputCell {
    pub row: usize,
    pub col: usize,
    pub width: usize,
    pub byte_start: usize,
    pub byte_end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputMapping {
    pub cells: Vec<InputCell>,
    pub end: (usize, usize),
    pub row_revisions: Vec<(TerminalLineId, u64)>,
    pub anchor: Option<ShellInputAnchor>,
}

impl InputMapping {
    pub fn byte_at(&self, row: usize, col: usize) -> Option<usize> {
        if let Some(cell) = self
            .cells
            .iter()
            .find(|cell| cell.row == row && col >= cell.col && col < cell.col + cell.width)
        {
            return Some(cell.byte_start);
        }
        (row == self.end.0 && col >= self.end.1)
            .then(|| self.cells.last().map_or(0, |cell| cell.byte_end))
    }

    /// Selection endpoints are inclusive terminal cells. Reject prompt/output spans.
    pub fn selected_range(
        &self,
        start: (usize, usize),
        end: (usize, usize),
    ) -> Option<Range<usize>> {
        let first = self.cells.iter().find(|cell| {
            cell.row == start.0 && start.1 >= cell.col && start.1 < cell.col + cell.width
        })?;
        let last = self
            .cells
            .iter()
            .find(|cell| cell.row == end.0 && end.1 >= cell.col && end.1 < cell.col + cell.width)?;
        (first.byte_start < last.byte_end).then_some(first.byte_start..last.byte_end)
    }

    pub fn cursor_at(&self, byte: usize) -> Option<(usize, usize)> {
        self.cells
            .iter()
            .find(|cell| cell.byte_start == byte)
            .map(|cell| (cell.row, cell.col))
            .or_else(|| {
                (self.cells.last().map_or(0, |cell| cell.byte_end) == byte).then_some(self.end)
            })
    }
}

fn chain(snapshot: &TerminalSnapshot) -> Option<Range<usize>> {
    if snapshot.display_offset != 0 || snapshot.cursor.row >= snapshot.row_count() {
        return None;
    }
    let mut start = snapshot.cursor.row;
    while start > 0 && snapshot.row(start)?.wrapped {
        start -= 1;
    }
    let mut end = snapshot.cursor.row + 1;
    while end < snapshot.row_count() && snapshot.row(end)?.wrapped {
        end += 1;
    }
    Some(start..end)
}

fn advance(
    snapshot: &TerminalSnapshot,
    range: &Range<usize>,
    row: &mut usize,
    col: &mut usize,
) -> Option<()> {
    if *col == snapshot.cols && *row + 1 < range.end && snapshot.row(*row + 1)?.wrapped {
        *row += 1;
        *col = 0;
    }
    Some(())
}

fn match_at(
    snapshot: &TerminalSnapshot,
    range: &Range<usize>,
    value: &str,
    start: (usize, usize),
) -> Option<InputMapping> {
    let (mut row, mut col) = start;
    let mut cells = Vec::new();
    for (byte_start, ch) in value.char_indices() {
        advance(snapshot, range, &mut row, &mut col)?;
        // Alacritty can leave a leading wide-character spacer at the right margin.
        if col + 1 == snapshot.cols && snapshot.cell(row, col)?.width == 0 {
            col += 1;
            advance(snapshot, range, &mut row, &mut col)?;
        }
        let cell = snapshot.cell(row, col)?;
        if cell.width == 0
            || (if cell.text.is_empty() {
                " "
            } else {
                cell.text.as_ref()
            })
            .chars()
            .ne(std::iter::once(ch))
        {
            return None;
        }
        let width = usize::from(cell.width);
        if width > 2 || col + width > snapshot.cols {
            return None;
        }
        if width == 2 && snapshot.cell(row, col + 1)?.width != 0 {
            return None;
        }
        cells.push(InputCell {
            row,
            col,
            width,
            byte_start,
            byte_end: byte_start + ch.len_utf8(),
        });
        col += width;
    }
    advance(snapshot, range, &mut row, &mut col)?;
    let last_row = cells.last().map_or(row, |cell| cell.row).max(row);
    let row_revisions = (start.0..=last_row)
        .map(|row| {
            let row = snapshot.row(row)?;
            Some((row.line_id?, row.revision))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(InputMapping {
        cells,
        end: (row, col),
        row_revisions,
        anchor: snapshot.shell_input_anchor,
    })
}

pub fn resolve_input(
    snapshot: &TerminalSnapshot,
    value: &str,
    cursor: usize,
) -> Option<InputMapping> {
    if !value.is_char_boundary(cursor)
        || value.contains(['\r', '\n'])
        || !value
            .graphemes(true)
            .all(|grapheme| grapheme.chars().count() == 1)
        || value.chars().any(char::is_control)
    {
        return None;
    }
    let range = chain(snapshot)?;
    let mapping = if let Some(anchor) = snapshot.shell_input_anchor {
        let row = snapshot
            .rows()
            .iter()
            .position(|row| row.line_id == Some(anchor.line_id))?;
        if !range.contains(&row) {
            return None;
        }
        match_at(snapshot, &range, value, (row, anchor.col))?
    } else {
        if value.is_empty() {
            return None;
        }
        // Flatten only this soft-wrapped chain. Searching UTF-8 avoids an
        // allocation and a full command comparison at every screen cell.
        let mut text = String::new();
        let mut positions = Vec::new();
        for row in range.clone() {
            for (col, cell) in snapshot.row(row)?.cells.iter().enumerate() {
                if cell.width == 0 {
                    continue;
                }
                positions.push((text.len(), row, col));
                if cell.text.is_empty() {
                    text.push(' ');
                } else {
                    text.push_str(&cell.text);
                }
            }
        }
        let mut offset = 0;
        let mut found = None;
        while let Some(relative) = text.get(offset..)?.find(value) {
            let index = offset + relative;
            if let Ok(position) = positions.binary_search_by_key(&index, |entry| entry.0) {
                let (_, row, col) = positions[position];
                if let Some(candidate) = match_at(snapshot, &range, value, (row, col)) {
                    if found.is_some() {
                        return None;
                    }
                    found = Some(candidate);
                }
            }
            offset = index + text[index..].chars().next()?.len_utf8();
        }
        found?
    };
    // A partially redrawn deletion can leave an old suffix beyond the predicted
    // end while already placing the cursor correctly. That is not confirmation.
    for row in mapping.end.0..range.end {
        let start = if row == mapping.end.0 {
            mapping.end.1
        } else {
            0
        };
        let cells = &snapshot.row(row)?.cells;
        if cells
            .iter()
            .skip(start)
            .any(|cell| cell.width > 0 && !cell.text.chars().all(|ch| ch == ' '))
        {
            return None;
        }
    }
    // Delayed autowrap leaves the cursor on the final cell until the next
    // printable byte. Preserve the input insertion position at that boundary.
    let mut mapping = mapping;
    if mapping.end.1 == snapshot.cols
        && snapshot.cursor.row == mapping.end.0
        && snapshot.cursor.col + 1 == snapshot.cols
        && cursor == value.len()
    {
        mapping.end.1 = snapshot.cols - 1;
    }
    (mapping.cursor_at(cursor) == Some((snapshot.cursor.row, snapshot.cursor.col)))
        .then_some(mapping)
}

/// Recovery is restricted to an explicit active input boundary. The terminal does
/// not infer a prompt by stripping punctuation or by trusting an outer shell.
pub fn recover_marked_input(
    snapshot: &TerminalSnapshot,
    max_bytes: usize,
) -> Option<(String, usize)> {
    let anchor = snapshot.shell_input_anchor?;
    let range = chain(snapshot)?;
    let start = snapshot
        .rows()
        .iter()
        .position(|row| row.line_id == Some(anchor.line_id))?;
    if !range.contains(&start) {
        return None;
    }
    let mut value = String::new();
    let mut cursor = None;
    for row in start..range.end {
        let row_data = snapshot.row(row)?;
        let col_start = if row == start { anchor.col } else { 0 };
        let col_end = if row + 1 < range.end {
            snapshot.cols
        } else {
            row_data
                .cells
                .iter()
                .rposition(|cell| cell.width > 0 && !cell.text.trim().is_empty())
                .map_or(0, |col| col + usize::from(row_data.cells[col].width))
                .max(if row == snapshot.cursor.row {
                    snapshot.cursor.col
                } else {
                    0
                })
        };
        // With an unknown buffer there is no evidence for invisible trailing
        // spaces after a cursor in the middle. Recover only at the visible end;
        // known complete buffers still support editing at any cursor position.
        if row + 1 == range.end && (row != snapshot.cursor.row || col_end > snapshot.cursor.col) {
            return None;
        }
        for col in col_start..col_end {
            if (row, col) == (snapshot.cursor.row, snapshot.cursor.col) {
                cursor = Some(value.len());
            }
            let cell = row_data.cells.get(col)?;
            if cell.width > 0 {
                if cell.text.is_empty() {
                    value.push(' ');
                } else {
                    value.push_str(&cell.text);
                }
            }
            if value.len() > max_bytes {
                return None;
            }
        }
        if (row, col_end) == (snapshot.cursor.row, snapshot.cursor.col) {
            cursor = Some(value.len());
        }
    }
    let cursor = cursor?;
    resolve_input(snapshot, &value, cursor)?;
    Some((value, cursor))
}

#[cfg(test)]
mod tests {
    use super::{recover_marked_input, resolve_input};
    use crate::TerminalScreen;

    /// Left prompt, a right prompt pinned to the margin, then the cursor back
    /// on the input column, the way zsh draws RPROMPT.
    fn screen_with_right_prompt(input: &str) -> TerminalScreen {
        let mut screen = TerminalScreen::new(80, 24);
        screen.advance(b"$ ");
        screen.advance(b"\x1b[1;73H12:00 \xe2\x94\x80\xe2\x95\xaf\x1b[1;3H");
        screen.advance(input.as_bytes());
        screen
    }

    /// What zsh does for `clear`: the command's output erases the screen, then
    /// the next prompt is drawn with a fresh input mark.
    #[test]
    fn input_is_resolved_again_after_the_screen_is_cleared() {
        for erase in [
            &b"\x1b[H\x1b[2J"[..],
            &b"\x1b[H\x1b[2J\x1b[3J"[..],
            &b"\x1b[3J\x1b[H\x1b[2J"[..],
        ] {
            let mut screen = TerminalScreen::new(80, 24);
            screen.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07clear\r\n\x1b]133;C\x07");
            screen.advance(erase);
            screen.advance(b"\x1b]133;D;0\x07\x1b]133;A\x07$ \x1b]133;B\x07cd /");
            let snapshot = screen.snapshot();
            assert!(snapshot.shell_input_anchor.is_some(), "{erase:?}");
            assert!(resolve_input(&snapshot, "cd /", 4).is_some(), "{erase:?}");
        }
    }

    #[test]
    fn an_unproven_right_prompt_cannot_confirm_the_input() {
        let screen = screen_with_right_prompt("cd /");
        let snapshot = screen.snapshot();
        assert!(resolve_input(&snapshot, "cd /", 4).is_none());
    }

    #[test]
    fn marked_recovery_does_not_discard_an_ambiguous_right_suffix() {
        let mut screen = TerminalScreen::new(80, 24);
        screen.advance(b"$ \x1b]133;B\x07");
        screen.advance(b"\x1b[1;73H12:00 \xe2\x94\x80\xe2\x95\xaf\x1b[1;3H");
        screen.advance(b"cd /");
        let snapshot = screen.snapshot();
        assert!(recover_marked_input(&snapshot, 4096).is_none());
    }

    #[test]
    fn text_adjacent_to_the_input_is_still_a_stale_suffix() {
        let mut screen = TerminalScreen::default();
        screen.advance(b"$ cd /varXYZ");
        screen.advance(b"\x1b[1;7H");
        let snapshot = screen.snapshot();
        assert!(resolve_input(&snapshot, "cd /v", 5).is_none());
    }

    #[test]
    fn a_gapped_suffix_away_from_the_margin_is_not_a_right_prompt() {
        let mut screen = TerminalScreen::default();
        screen.advance(b"$ cd /   old");
        screen.advance(b"\x1b[1;7H");
        let snapshot = screen.snapshot();
        assert!(resolve_input(&snapshot, "cd /", 4).is_none());
    }

    #[test]
    fn old_text_near_the_right_margin_is_not_confirmation() {
        for marked in [false, true] {
            for suffix in ["old", "界x"] {
                let mut screen = TerminalScreen::new(40, 4);
                screen.advance(b"$ ");
                if marked {
                    screen.advance(b"\x1b]133;B\x07");
                }
                screen.advance(format!("abc\x1b[1;37H{suffix}\x1b[1;6H").as_bytes());
                assert!(resolve_input(&screen.snapshot(), "abc", 3).is_none());
                assert!(recover_marked_input(&screen.snapshot(), 4096).is_none());
                screen.advance(b"\x1b[K");
                assert!(resolve_input(&screen.snapshot(), "abc", 3).is_some());
            }
        }
    }

    #[test]
    fn unique_echo_keeps_tail_spaces_and_wide_cell_boundaries() {
        let mut screen = TerminalScreen::default();
        screen.advance("$ a界b  ".as_bytes());
        let snapshot = screen.snapshot();
        let map = resolve_input(&snapshot, "a界b  ", "a界b  ".len()).unwrap();
        assert_eq!(map.byte_at(0, 4), Some(1));
        assert_eq!(map.byte_at(0, 20), Some("a界b  ".len()));
        assert_eq!(map.selected_range((0, 4), (0, 5)), Some(1..5));
    }

    #[test]
    fn repeated_text_needs_a_marked_boundary() {
        let mut screen = TerminalScreen::default();
        screen.advance(b"abc abc");
        assert!(resolve_input(&screen.snapshot(), "abc", 3).is_none());
        let mut screen = TerminalScreen::default();
        screen.advance(b"abc \x1b]133;B\x07abc");
        assert!(resolve_input(&screen.snapshot(), "abc", 3).is_some());
    }

    #[test]
    fn stale_suffix_after_partial_deletion_is_not_confirmed() {
        for marked in [false, true] {
            let mut screen = TerminalScreen::default();
            screen.advance(b"$ ");
            if marked {
                screen.advance(b"\x1b]133;B\x07");
            }
            screen.advance(b"abcdef\x1b[3D");
            assert!(resolve_input(&screen.snapshot(), "abc", 3).is_none());
            screen.advance(b"\x1b[K");
            assert!(resolve_input(&screen.snapshot(), "abc", 3).is_some());
        }
    }

    #[test]
    fn soft_wrap_mapping_follows_continuation_rows() {
        let mut screen = TerminalScreen::new(8, 4);
        screen.advance(b"$ abcdefghi");
        let map = resolve_input(&screen.snapshot(), "abcdefghi", 9).unwrap();
        assert_eq!(map.byte_at(1, 0), Some(6));
        assert_eq!(map.selected_range((0, 5), (1, 1)), Some(3..8));
        // A remote shell redraws across wraps; CSI Left alone never changes rows.
        screen.advance(b"\x1b[1;8H");
        assert!(resolve_input(&screen.snapshot(), "abcdefghi", 5).is_some());
    }

    #[test]
    fn recovery_requires_anchor_and_actual_cursor() {
        let mut screen = TerminalScreen::default();
        screen.advance(b"$ \x1b]133;B\x07echo file");
        assert_eq!(
            recover_marked_input(&screen.snapshot(), 4096),
            Some(("echo file".into(), 9))
        );
        screen.advance(b"\x1b[D\x1b[D");
        assert!(recover_marked_input(&screen.snapshot(), 4096).is_none());
        screen.advance(b"\x1b]133;C\x07");
        assert!(recover_marked_input(&screen.snapshot(), 4096).is_none());
    }

    #[test]
    fn complex_graphemes_and_incomplete_echo_do_not_match() {
        let mut screen = TerminalScreen::default();
        screen.advance("$ e\u{301}".as_bytes());
        assert!(resolve_input(&screen.snapshot(), "e\u{301}", 3).is_none());
        assert!(resolve_input(&screen.snapshot(), "echo", 4).is_none());
    }

    #[test]
    fn anchors_capture_column_and_close_on_shell_and_screen_boundaries() {
        for boundary in [
            "\x1b]133;A\x07",
            "\x1b]133;C\x07",
            "\x1b]133;D;0\x07",
            "\x1bc",
            "\x1b[?1049h\x1b[?1049l",
        ] {
            let mut screen = TerminalScreen::default();
            screen.advance(b"$ \x1b]133;B\x07abc");
            let anchor = screen.snapshot().shell_input_anchor.unwrap();
            assert_eq!(anchor.col, 2);
            assert_eq!(
                screen.snapshot().row(0).unwrap().line_id,
                Some(anchor.line_id)
            );
            screen.advance(boundary.as_bytes());
            assert!(screen.snapshot().shell_input_anchor.is_none());
        }
        let mut screen = TerminalScreen::new(8, 4);
        screen.advance(b"$ \x1b]133;B\x07abc");
        screen.resize(8, 5);
        assert!(screen.snapshot().shell_input_anchor.is_none());
    }

    #[test]
    fn delayed_wrap_and_wide_margin_spacer_keep_editor_positions() {
        let mut screen = TerminalScreen::new(8, 4);
        screen.advance(b"$ abcdef");
        assert!(resolve_input(&screen.snapshot(), "abcdef", 6).is_some());
        let mut screen = TerminalScreen::new(8, 4);
        screen.advance("$ abcde界x".as_bytes());
        let map = resolve_input(&screen.snapshot(), "abcde界x", "abcde界x".len()).unwrap();
        assert_eq!(map.byte_at(1, 1), Some(5));
    }

    #[test]
    fn overlapping_matches_and_history_are_not_editable() {
        let mut screen = TerminalScreen::default();
        screen.advance(b"aaaa");
        assert!(resolve_input(&screen.snapshot(), "aaa", 3).is_none());
        screen.advance(b"\r\nnext");
        assert!(resolve_input(&screen.snapshot(), "aaaa", 4).is_none());
    }
}

#[cfg(test)]
mod file_context_tests {
    use crate::TerminalScreen;
    const MARK: &str = "\x1b]7777;NyaTermFileContext;1;0123456789abcdef0123456789abcdef;123;1000;1;L2hvbWUvdXNlcg==;L3RtcA==\x07";
    #[test]
    fn context_follows_prompt_boundaries_even_across_single_byte_chunks() {
        for split in [false, true] {
            let mut screen = TerminalScreen::new(80, 24);
            let bytes = format!("\x1b]133;A\x07{MARK}$ \x1b]133;B\x07cd /");
            if split {
                for byte in bytes.bytes() {
                    screen.advance(&[byte]);
                }
            } else {
                screen.advance(bytes.as_bytes());
            }
            let snapshot = screen.snapshot();
            assert!(snapshot.shell_input_anchor.is_some());
            assert_eq!(snapshot.file_context.as_ref().unwrap().cwd, "/tmp");
            screen.advance(b"\x1b]133;C\x07");
            assert!(screen.snapshot().file_context.is_none());
            screen.advance(MARK.as_bytes());
            screen.advance(b"\x1b]133;A\x07");
            assert!(screen.snapshot().file_context.is_none());
        }
    }
    #[test]
    fn malformed_and_oversized_markers_invalidate_previous_context() {
        let mut screen = TerminalScreen::new(80, 24);
        screen.advance(MARK.as_bytes());
        assert!(screen.snapshot().file_context.is_some());
        screen.advance(b"\x1b]7777;NyaTermFileContext;99;bad\x07");
        assert!(screen.snapshot().file_context.is_none());
        screen.advance(MARK.as_bytes());
        screen
            .advance(format!("\x1b]7777;NyaTermFileContext;1;{}\x07", "x".repeat(5000)).as_bytes());
        assert!(screen.snapshot().file_context.is_none());
    }
}
