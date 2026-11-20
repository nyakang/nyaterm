//! UI-independent cursor motion for keyboard-driven local selection mode.
//!
//! The desktop layer owns focus, key dispatch and rendering. This module only
//! answers one question: given an absolute-buffer cursor and a visible window,
//! where should the cursor land after a navigation motion?

use crate::selection::TerminalBufferCellPos;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalModeMotion {
    Left,
    Right,
    Up,
    Down,
    PageUp(usize),
    PageDown(usize),
    Home,
    End,
    BufferStart,
    BufferEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalModeViewport {
    pub first_line: usize,
    pub last_line: usize,
    pub page_rows: usize,
    pub cols: usize,
}

impl LocalModeViewport {
    fn clamp_line(self, line: usize) -> usize {
        if self.last_line < self.first_line {
            return self.first_line;
        }
        line.clamp(self.first_line, self.last_line)
    }
}

fn clamp_col(line_len: usize, col: usize, cols: usize) -> usize {
    let limit = line_len.min(cols).saturating_sub(1);
    col.min(limit)
}

/// Move `cursor` by a local-mode navigation motion.
///
/// `line_len` reports the number of addressable cells on an absolute line; it
/// lets callers account for wide characters and trailing blanks without pulling
/// GPUI or a snapshot into this crate-level helper.
pub fn move_local_mode_cursor(
    mut cursor: TerminalBufferCellPos,
    motion: LocalModeMotion,
    viewport: LocalModeViewport,
    line_len: impl Fn(usize) -> usize,
) -> TerminalBufferCellPos {
    cursor.line = viewport.clamp_line(cursor.line);
    cursor.col = clamp_col(line_len(cursor.line), cursor.col, viewport.cols);

    match motion {
        LocalModeMotion::Left => {
            cursor.col = cursor.col.saturating_sub(1);
        }
        LocalModeMotion::Right => {
            cursor.col = clamp_col(
                line_len(cursor.line),
                cursor.col.saturating_add(1),
                viewport.cols,
            );
        }
        LocalModeMotion::Up => {
            cursor.line = viewport.clamp_line(cursor.line.saturating_sub(1));
        }
        LocalModeMotion::Down => {
            cursor.line = viewport.clamp_line(cursor.line.saturating_add(1));
        }
        LocalModeMotion::PageUp(times) => {
            let rows = times.max(1).saturating_mul(viewport.page_rows.max(1));
            cursor.line = viewport.clamp_line(cursor.line.saturating_sub(rows));
        }
        LocalModeMotion::PageDown(times) => {
            let rows = times.max(1).saturating_mul(viewport.page_rows.max(1));
            cursor.line = viewport.clamp_line(cursor.line.saturating_add(rows));
        }
        LocalModeMotion::Home => {
            cursor.col = 0;
        }
        LocalModeMotion::End => {
            cursor.col = clamp_col(line_len(cursor.line), usize::MAX, viewport.cols);
        }
        LocalModeMotion::BufferStart => {
            cursor.line = viewport.first_line;
            cursor.col = 0;
        }
        LocalModeMotion::BufferEnd => {
            cursor.line = viewport.last_line;
            cursor.col = clamp_col(line_len(cursor.line), usize::MAX, viewport.cols);
        }
    }

    cursor.line = viewport.clamp_line(cursor.line);
    cursor.col = clamp_col(line_len(cursor.line), cursor.col, viewport.cols);
    cursor
}

/// Mouse hit to text-editor style caret column. `content_end` is the column
/// just after the last non-empty cell, so clicking the blank area to the right
/// of a line always snaps back to the line's real end.
pub fn local_mode_insertion_col(
    cell_col: usize,
    side: crate::selection::TerminalCellSide,
    content_end: usize,
) -> usize {
    let col = match side {
        crate::selection::TerminalCellSide::Left => cell_col,
        crate::selection::TerminalCellSide::Right => cell_col.saturating_add(1),
    };
    col.min(content_end)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEWPORT: LocalModeViewport = LocalModeViewport {
        first_line: 10,
        last_line: 40,
        page_rows: 12,
        cols: 80,
    };

    fn len(_line: usize) -> usize {
        40
    }

    fn pos(line: usize, col: usize) -> TerminalBufferCellPos {
        TerminalBufferCellPos::new(line, col)
    }

    #[test]
    fn mouse_insertion_col_follows_clicked_half_and_content_end() {
        use crate::selection::TerminalCellSide::{Left, Right};

        assert_eq!(local_mode_insertion_col(4, Left, 12), 4);
        assert_eq!(local_mode_insertion_col(4, Right, 12), 5);
        assert_eq!(local_mode_insertion_col(20, Left, 7), 7);
        assert_eq!(local_mode_insertion_col(20, Right, 7), 7);
        assert_eq!(local_mode_insertion_col(20, Right, 0), 0);
    }

    #[test]
    fn horizontal_motion_clamps_to_line_and_grid() {
        assert_eq!(
            move_local_mode_cursor(pos(12, 5), LocalModeMotion::Left, VIEWPORT, len),
            pos(12, 4)
        );
        assert_eq!(
            move_local_mode_cursor(pos(12, 0), LocalModeMotion::Left, VIEWPORT, len),
            pos(12, 0)
        );
        assert_eq!(
            move_local_mode_cursor(pos(12, 39), LocalModeMotion::Right, VIEWPORT, len),
            pos(12, 39)
        );
        assert_eq!(
            move_local_mode_cursor(pos(12, 0), LocalModeMotion::Home, VIEWPORT, len),
            pos(12, 0)
        );
        assert_eq!(
            move_local_mode_cursor(pos(12, 3), LocalModeMotion::End, VIEWPORT, len),
            pos(12, 39)
        );
    }

    #[test]
    fn page_motion_uses_page_rows_and_clamps() {
        assert_eq!(
            move_local_mode_cursor(pos(30, 3), LocalModeMotion::PageUp(1), VIEWPORT, len),
            pos(18, 3)
        );
        assert_eq!(
            move_local_mode_cursor(pos(30, 3), LocalModeMotion::PageDown(2), VIEWPORT, len),
            pos(40, 3)
        );
        assert_eq!(
            move_local_mode_cursor(pos(11, 3), LocalModeMotion::PageUp(3), VIEWPORT, len),
            pos(10, 3)
        );
    }

    #[test]
    fn buffer_edges_and_short_lines_are_respected() {
        let short = |line: usize| if line == 20 { 6 } else { 40 };
        assert_eq!(
            move_local_mode_cursor(pos(25, 30), LocalModeMotion::Up, VIEWPORT, short),
            pos(24, 30)
        );
        assert_eq!(
            move_local_mode_cursor(pos(21, 30), LocalModeMotion::Up, VIEWPORT, short),
            pos(20, 5)
        );
        assert_eq!(
            move_local_mode_cursor(pos(35, 3), LocalModeMotion::BufferStart, VIEWPORT, len),
            pos(10, 0)
        );
        assert_eq!(
            move_local_mode_cursor(pos(15, 3), LocalModeMotion::BufferEnd, VIEWPORT, len),
            pos(40, 39)
        );
    }
}
