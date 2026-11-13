//! Shell-proven command regions. Coordinates describe the original retained grid.
//! Snapshot blocks are ordered by both start row and monotonically allocated identity.
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommandBlockId(u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandBlock {
    pub id: CommandBlockId,
    pub rows: Range<usize>,
    /// Only whole output rows may be hidden; mixed input/output rows stay visible.
    pub output: Range<usize>,
    pub complete: bool,
    pub output_lines: usize,
}

impl CommandBlock {
    pub fn can_collapse(&self, cursor: usize) -> bool {
        self.complete && !self.output.is_empty() && !self.rows.contains(&cursor)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TrackedCommand {
    pub id: CommandBlockId,
    pub start: i64,
    pub output: Option<i64>,
    pub end: Option<i64>,
    pub output_lines: usize,
}

#[derive(Debug, Default)]
pub(crate) struct CommandBlocks {
    pub entries: Vec<TrackedCommand>,
}

impl CommandBlocks {
    pub fn start(&mut self, line: i64) {
        if self.entries.last().is_some_and(|block| block.start >= line) {
            return;
        }
        self.finish(line);
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        self.entries.push(TrackedCommand {
            id: CommandBlockId(NEXT_ID.fetch_add(1, Ordering::Relaxed)),
            start: line,
            output: None,
            end: None,
            output_lines: 0,
        });
    }

    pub fn output(&mut self, line: i64) {
        if let Some(block) = self.entries.last_mut().filter(|block| block.end.is_none()) {
            block.output = Some(line.max(block.start + 1));
        }
    }

    pub fn finish(&mut self, end: i64) {
        if let Some(block) = self.entries.last_mut().filter(|block| block.end.is_none()) {
            block.end = Some(end.max(block.start + 1));
        }
    }

    pub fn retain(&mut self, first: i64, last: i64) {
        self.entries
            .retain(|block| block.start >= first && block.start <= last);
    }

    pub fn snapshot(&self, first: i64, cursor: i64, last: i64) -> Vec<CommandBlock> {
        self.entries
            .iter()
            .filter_map(|block| {
                if block.start < first || block.start > last {
                    return None;
                }
                let end = block
                    .end
                    .unwrap_or(cursor + 1)
                    .min(last + 1)
                    .max(block.start + 1);
                let start = (block.start - first) as usize;
                let end = (end - first) as usize;
                let output = block
                    .output
                    .map_or(end, |line| (line - first).max(0) as usize)
                    .min(end);
                Some(CommandBlock {
                    id: block.id,
                    rows: start..end,
                    output: output..end,
                    complete: block.end.is_some(),
                    output_lines: block.output_lines,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::TerminalScreen;

    fn completed() -> TerminalScreen {
        let mut screen = TerminalScreen::new(20, 8);
        screen.advance(b"$ \x1b]133;B\x07echo hi\x1b]133;C\x07\r\none\r\ntwo\r\n\x1b]133;D;0\x07\x1b]133;A\x07$ \x1b]133;B\x07");
        screen
    }

    #[test]
    fn completed_output_excludes_following_prompt_and_preserves_input() {
        let screen = completed();
        let snapshot = screen.snapshot();
        let block = &snapshot.command_blocks[0];
        assert_eq!(block.rows, 0..3);
        assert_eq!(block.output, 1..3);
        assert_eq!(block.output_lines, 2);
        assert!(block.can_collapse(3));
        assert!(!block.can_collapse(1));
        assert!(!snapshot.command_blocks[1].complete);
    }

    #[test]
    fn running_and_incomplete_boundaries_cannot_hide_output() {
        let mut screen = TerminalScreen::new(20, 6);
        screen.advance(b"$ \x1b]633;B\x07run\r\n\x1b]633;C\x07first\r\nsecond");
        let snapshot = screen.snapshot();
        assert!(!snapshot.command_blocks[0].complete);
        assert_eq!(snapshot.command_blocks[0].rows, 0..3);
        screen.advance(b"\r\n\x1b]633;D;0\x07");
        assert!(screen.snapshot().command_blocks[0].complete);
        let mut missing = TerminalScreen::new(20, 6);
        missing.advance(b"$ \x1b]133;B\x07run\r\noutput\r\n\x1b]133;D;0\x07");
        assert!(missing.snapshot().command_blocks[0].complete);
        assert!(!missing.snapshot().command_blocks[0].can_collapse(3));
    }

    #[test]
    fn split_osc_and_multiline_input_have_one_identity() {
        let mut screen = TerminalScreen::new(20, 8);
        screen.advance(b"$ \x1b]133;");
        screen.advance(b"B\x07first\r\nsecond\r\n\x1b]133;C\x07out\r\n\x1b]133;D;0\x07");
        let snapshot = screen.snapshot();
        assert_eq!(snapshot.command_blocks.len(), 1);
        assert_eq!(snapshot.command_blocks[0].output, 2..3);
    }

    #[test]
    fn zero_output_and_shared_input_output_row_are_not_hidden() {
        let mut screen = TerminalScreen::new(20, 5);
        screen.advance(b"$ \x1b]133;B\x07true\r\n\x1b]133;C\x07\x1b]133;D;0\x07");
        assert!(screen.snapshot().command_blocks[0].output.is_empty());
        let mut shared = TerminalScreen::new(20, 5);
        shared.advance(b"$ \x1b]133;B\x07cmd\x1b]133;C\x07output\x1b]133;D;0\x07");
        assert!(shared.snapshot().command_blocks[0].output.is_empty());
        let mut wrapped = TerminalScreen::new(8, 8);
        wrapped.advance(
            b"$ \x1b]133;B\x07cmd\x1b]133;C\x07long-shared-output\r\nseparate\r\n\x1b]133;D;0\x07",
        );
        let snapshot = wrapped.snapshot();
        assert_eq!(snapshot.command_blocks[0].output, 3..4);
        assert_eq!(snapshot.command_blocks[0].output_lines, 1);
    }

    #[test]
    fn resize_preserves_command_identity_and_logical_output_count() {
        let mut screen = completed();
        let before = screen.snapshot().command_blocks[0].clone();
        screen.resize(4, 8);
        let snapshot = screen.snapshot();
        assert_eq!(snapshot.command_blocks[0].id, before.id);
        assert_eq!(snapshot.command_blocks[0].output_lines, before.output_lines);
        assert!(snapshot.command_blocks[0].output.start > before.output.start);
    }

    #[test]
    fn clear_reset_and_alternate_screen_do_not_leak_blocks() {
        let mut screen = completed();
        let id = screen.snapshot().command_blocks[0].id;
        screen.advance(b"\x1b[?1049h");
        assert!(screen.snapshot().command_blocks.is_empty());
        screen.advance(b"\x1b[?1049l");
        assert_eq!(screen.snapshot().command_blocks[0].id, id);
        screen.clear_except_input();
        assert!(screen.snapshot().command_blocks.is_empty());
        screen.advance(b"\x1b]133;B\x07next\r\n\x1b]133;C\x07out\r\n\x1b]133;D;0\x07");
        assert_ne!(screen.snapshot().command_blocks[0].id, id);
        screen.advance(b"\x1bc");
        assert!(screen.snapshot().command_blocks.is_empty());
    }

    #[test]
    fn folded_window_retains_source_text_and_maps_summary_without_inserting_text() {
        let mut screen = completed();
        let id = screen.snapshot().command_blocks[0].id;
        screen.set_collapsed_commands([id].into());
        let snapshot = screen.snapshot();
        let index = snapshot.display_rows.as_ref().unwrap();
        assert_eq!(index.rows(), 7);
        assert_eq!(index.summary(1), Some(id));
        assert_eq!(index.source_row(2), 3);
        assert_eq!(index.display_row(3), 2);
        assert!(index.hidden(1));
        assert!(index.hidden(2));
        assert_eq!(snapshot.row(1).unwrap().text, "one");
        assert_eq!(snapshot.row(2).unwrap().text, "two");
    }
}
