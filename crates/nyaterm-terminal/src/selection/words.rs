//! Word boundaries in snapshot cell coordinates.
use crate::{TerminalSnapshot, TerminalTextCell, terminal_is_zero_width_mark, terminal_text_cells};
pub fn terminal_word_bounds_at_snapshot_row(
    col: usize,
    snapshot: &TerminalSnapshot,
    snapshot_row: usize,
    separators: &str,
) -> (usize, usize) {
    let line = snapshot.line(snapshot_row).unwrap_or("");
    let cells = terminal_text_cells(line);
    if cells.is_empty() {
        return (col, col.saturating_add(1));
    }
    let idx = col.min(cells.len().saturating_sub(1));
    // xterm wordSeparator semantics: characters listed are separators, not word body.
    let is_word = |cell: &TerminalTextCell| terminal_text_cell_is_word(cell, separators);
    if !is_word(&cells[idx]) {
        return (idx, idx.saturating_add(1));
    }
    let mut start = idx;
    while start > 0 && is_word(&cells[start - 1]) {
        start -= 1;
    }
    let mut end = idx + 1;
    while end < cells.len() && is_word(&cells[end]) {
        end += 1;
    }
    (start, end)
}

pub fn terminal_text_cell_is_word(cell: &TerminalTextCell, separators: &str) -> bool {
    cell.text
        .chars()
        .find(|ch| !terminal_is_zero_width_mark(*ch))
        .is_some_and(|ch| !separators.contains(ch))
}

#[cfg(test)]
mod tests {
    use super::terminal_text_cell_is_word;
    use crate::terminal_text_cells;
    #[test]
    fn terminal_text_word_cells_use_base_character_for_separators() {
        let cells = terminal_text_cells("e\u{301}/x");

        assert!(terminal_text_cell_is_word(&cells[0], "/"));
        assert!(!terminal_text_cell_is_word(&cells[1], "/"));
        assert!(terminal_text_cell_is_word(&cells[2], "/"));
    }
}
