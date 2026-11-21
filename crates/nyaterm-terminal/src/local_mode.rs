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

/// Clamp an insertion-point column to a line's content plus the screen width.
///
/// `content_end` is the column just after the last non-empty cell, so the caret
/// can rest there (after the final glyph) but never in the blank margin beyond
/// it. Physical screen bounds still apply as a hard ceiling.
fn clamp_col(content_end: usize, col: usize, cols: usize) -> usize {
    col.min(content_end.min(cols))
}

/// Move `cursor` by a local-mode navigation motion.
///
/// `content_end` reports the insertion-point column just after the last
/// non-empty cell on an absolute line. Keeping this separate from `cols` lets
/// callers account for wide characters and trailing blanks without pulling
/// GPUI or a snapshot into this crate-level helper, and makes `End`/`BufferEnd`
/// stop after the final glyph instead of in trailing whitespace.
pub fn move_local_mode_cursor(
    mut cursor: TerminalBufferCellPos,
    motion: LocalModeMotion,
    viewport: LocalModeViewport,
    content_end: impl Fn(usize) -> usize,
) -> TerminalBufferCellPos {
    cursor.line = viewport.clamp_line(cursor.line);
    cursor.col = clamp_col(content_end(cursor.line), cursor.col, viewport.cols);

    match motion {
        LocalModeMotion::Left => {
            cursor.col = cursor.col.saturating_sub(1);
        }
        LocalModeMotion::Right => {
            cursor.col = clamp_col(
                content_end(cursor.line),
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
            cursor.col = clamp_col(content_end(cursor.line), usize::MAX, viewport.cols);
        }
        LocalModeMotion::BufferStart => {
            cursor.line = viewport.first_line;
            cursor.col = 0;
        }
        LocalModeMotion::BufferEnd => {
            cursor.line = viewport.last_line;
            cursor.col = clamp_col(content_end(cursor.line), usize::MAX, viewport.cols);
        }
    }

    cursor.line = viewport.clamp_line(cursor.line);
    cursor.col = clamp_col(content_end(cursor.line), cursor.col, viewport.cols);
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

/// Scroll delta for a local-mode caret after a navigation motion, in
/// `scroll_terminal_by` semantics: positive scrolls toward older history
/// (viewport moves up), negative scrolls toward newer output (viewport moves
/// down), zero leaves the viewport untouched.
///
/// - Page motions pass `follow_lines`: the viewport translates by exactly the
///   number of lines the caret moved, so the caret keeps its screen row and the
///   display follows the page flip.
/// - All other motions only need a minimal reveal: scroll just enough to bring
///   `line` back inside the visible window `[visible_start, visible_end)`.
pub fn local_mode_scroll_delta(
    follow_lines: Option<i32>,
    line: usize,
    visible_start: usize,
    visible_end: usize,
) -> i32 {
    if let Some(delta) = follow_lines {
        return delta;
    }
    if line < visible_start {
        return visible_start.saturating_sub(line) as i32;
    }
    if visible_end > visible_start && line >= visible_end {
        // Signed arithmetic on purpose. `visible_end.saturating_sub(line + 1)`
        // underflows to 0 whenever `line >= visible_end`, which silently
        // skipped the scroll and left PageDown-style motion un-followed.
        return (visible_end as i32) - (line as i32) - 1;
    }
    0
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

    /// Content ends at column 40 (the last non-empty glyph occupies col 39),
    /// mirroring what the desktop layer reports for a fully occupied line.
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
            pos(12, 40)
        );
        assert_eq!(
            move_local_mode_cursor(pos(12, 0), LocalModeMotion::Home, VIEWPORT, len),
            pos(12, 0)
        );
        assert_eq!(
            move_local_mode_cursor(pos(12, 3), LocalModeMotion::End, VIEWPORT, len),
            pos(12, 40)
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
        // A short line's content ends at col 6; the caret must not sit in the
        // blank margin beyond it even when arriving from a longer line.
        let short = |line: usize| if line == 20 { 6 } else { 40 };
        assert_eq!(
            move_local_mode_cursor(pos(25, 30), LocalModeMotion::Up, VIEWPORT, short),
            pos(24, 30)
        );
        assert_eq!(
            move_local_mode_cursor(pos(21, 30), LocalModeMotion::Up, VIEWPORT, short),
            pos(20, 6)
        );
        assert_eq!(
            move_local_mode_cursor(pos(35, 3), LocalModeMotion::BufferStart, VIEWPORT, len),
            pos(10, 0)
        );
        assert_eq!(
            move_local_mode_cursor(pos(15, 3), LocalModeMotion::BufferEnd, VIEWPORT, len),
            pos(40, 40)
        );
    }

    #[test]
    fn end_stops_after_last_glyph_not_in_trailing_blank() {
        // Physical width is 80, but this line's content ends at col 12.
        let short = |_line: usize| 12;
        assert_eq!(
            move_local_mode_cursor(pos(12, 2), LocalModeMotion::End, VIEWPORT, short),
            pos(12, 12)
        );
    }

    #[test]
    fn page_motion_follows_caret_by_exact_line_delta() {
        // PageDown: caret moved down 12 lines => viewport scrolls 12 toward
        // newer output (negative). Keeps the caret on the same screen row.
        assert_eq!(local_mode_scroll_delta(Some(115 - 127), 127, 100, 130), -12);
        // PageUp: caret moved up 12 lines => viewport scrolls 12 toward older
        // history (positive).
        assert_eq!(local_mode_scroll_delta(Some(85 - 73), 73, 100, 130), 12);
        // Clamped at the buffer edge: caret moved less than a full page, so
        // the viewport follows by that smaller amount.
        assert_eq!(local_mode_scroll_delta(Some(125 - 129), 129, 100, 130), -4);
    }

    #[test]
    fn minimal_reveal_scrolls_both_directions_without_underflow() {
        // Caret above the visible window: scroll toward older history.
        assert_eq!(local_mode_scroll_delta(None, 85, 100, 130), 15);
        // Caret below the visible window: negative delta (old code returned 0
        // here because of a usize saturating_sub underflow).
        assert_eq!(local_mode_scroll_delta(None, 130, 100, 130), -1);
        assert_eq!(local_mode_scroll_delta(None, 145, 100, 130), -16);
        // Caret inside the window: no scroll.
        assert_eq!(local_mode_scroll_delta(None, 115, 100, 130), 0);
        assert_eq!(local_mode_scroll_delta(None, 100, 100, 130), 0);
    }
}
