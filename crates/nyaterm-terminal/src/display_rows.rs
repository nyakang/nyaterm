//! Rank/select over display rows without changing terminal history or selection coordinates.
use std::collections::BTreeSet;
use std::ops::Range;
use std::sync::Arc;

use crate::command_blocks::{CommandBlock, CommandBlockId};
use crate::logical_lines::TerminalLogicalLineIndex;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Fold {
    id: CommandBlockId,
    source: Range<usize>,
    base: Range<usize>,
    display: usize,
    removed_before: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalDisplayRows {
    physical_rows: usize,
    logical: Option<Arc<TerminalLogicalLineIndex>>,
    folds: Vec<Fold>,
    rows: usize,
}

impl TerminalDisplayRows {
    pub fn new(
        physical_rows: usize,
        logical: Option<Arc<TerminalLogicalLineIndex>>,
        blocks: &[CommandBlock],
        collapsed: &BTreeSet<CommandBlockId>,
        cursor: usize,
    ) -> Self {
        let mut result = Self {
            physical_rows,
            rows: logical
                .as_ref()
                .map_or(physical_rows, |index| index.logical_rows()),
            logical,
            folds: Vec::new(),
        };
        let mut removed = 0;
        for block in collapsed.iter().filter_map(|id| {
            blocks
                .binary_search_by_key(id, |block| block.id)
                .ok()
                .map(|index| &blocks[index])
        }) {
            if !block.can_collapse(cursor) {
                continue;
            }
            let mut source = block.output.clone();
            if let Some(index) = result.logical.as_ref() {
                // A hard line shared with input or the next prompt cannot be hidden.
                if index.physical_row(index.logical_row(source.start)) < source.start {
                    source.start = index.physical_row(index.logical_row(source.start) + 1);
                }
                if source.end < physical_rows {
                    source.end = index.physical_row(index.logical_row(source.end));
                }
            }
            if source.is_empty() {
                continue;
            }
            let base = result.base_row(source.start)..result.base_row(source.end);
            if base.is_empty() {
                continue;
            }
            result.folds.push(Fold {
                id: block.id,
                source,
                display: base.start - removed,
                removed_before: removed,
                base: base.clone(),
            });
            removed += base.len() - 1;
        }
        result.rows = result.rows.saturating_sub(removed);
        result
    }

    fn base_row(&self, source: usize) -> usize {
        self.logical
            .as_ref()
            .map_or(source.min(self.physical_rows), |index| {
                if source >= self.physical_rows {
                    index.logical_rows()
                } else {
                    index.logical_row(source)
                }
            })
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn display_row(&self, source: usize) -> usize {
        let base = self.base_row(source);
        let count = self.folds.partition_point(|fold| fold.base.start <= base);
        if let Some(fold) = count.checked_sub(1).map(|i| &self.folds[i]) {
            if base < fold.base.end {
                return fold.display;
            }
            return base - fold.removed_before - (fold.base.len() - 1);
        }
        base
    }

    pub fn source_row(&self, display: usize) -> usize {
        let count = self.folds.partition_point(|fold| fold.display <= display);
        let base = if let Some(fold) = count.checked_sub(1).map(|i| &self.folds[i]) {
            if display == fold.display {
                return fold.source.start;
            }
            display + fold.removed_before + fold.base.len() - 1
        } else {
            display
        };
        self.logical
            .as_ref()
            .map_or(base.min(self.physical_rows), |index| {
                index.physical_row(base)
            })
    }

    pub fn summary(&self, display: usize) -> Option<CommandBlockId> {
        self.folds
            .binary_search_by_key(&display, |fold| fold.display)
            .ok()
            .map(|i| self.folds[i].id)
    }

    pub fn hidden(&self, source: usize) -> bool {
        let count = self
            .folds
            .partition_point(|fold| fold.source.start <= source);
        count
            .checked_sub(1)
            .is_some_and(|i| self.folds[i].source.contains(&source))
    }

    pub fn top(&self, offset: usize, viewport: usize) -> usize {
        let max = self.rows.saturating_sub(viewport);
        if offset == 0 {
            max
        } else {
            self.display_row(
                self.physical_rows
                    .saturating_sub(viewport)
                    .saturating_sub(offset),
            )
            .min(max)
        }
    }

    pub fn source_window(&self, offset: usize, viewport: usize) -> Range<usize> {
        let top = self.top(offset, viewport);
        self.source_row(top)..self.source_row((top + viewport).min(self.rows))
    }

    pub fn scroll_metrics(&self, offset: usize, viewport: usize) -> (usize, usize) {
        let max = self.rows.saturating_sub(viewport);
        (max.saturating_sub(self.top(offset, viewport)), max)
    }

    pub fn physical_offset(&self, display_offset: usize, viewport: usize) -> usize {
        if display_offset == 0 {
            return 0;
        }
        let top = self
            .rows
            .saturating_sub(viewport)
            .saturating_sub(display_offset);
        self.physical_rows
            .saturating_sub(viewport)
            .saturating_sub(self.source_row(top))
    }

    pub fn scroll(&self, offset: usize, viewport: usize, delta: i32) -> usize {
        let (current, max) = self.scroll_metrics(offset, viewport);
        let next = (current as i64 + i64::from(delta)).clamp(0, max as i64) as usize;
        self.physical_offset(next, viewport)
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalDisplayRows;
    use crate::TerminalScreen;

    #[test]
    fn multiple_folds_scroll_in_display_rows_and_round_trip_visible_sources() {
        for logical in [false, true] {
            let mut screen = TerminalScreen::new(8, 4);
            screen.set_logical_line_snapshots(logical);
            for _ in 0..8 {
                screen.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07run\r\n\x1b]133;C\x07abcdefghijk\r\nsecond\r\n\x1b]133;D;0\x07");
            }
            screen.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07");
            let snapshot = screen.snapshot();
            let ids = snapshot
                .command_blocks
                .iter()
                .take(7)
                .map(|block| block.id)
                .collect();
            let index = TerminalDisplayRows::new(
                snapshot.history.scrollback_len + snapshot.viewport_rows,
                snapshot.logical_lines.clone(),
                &snapshot.command_blocks,
                &ids,
                usize::MAX,
            );
            for display in 0..index.rows() {
                let source = index.source_row(display);
                assert_eq!(index.display_row(source), display);
                assert_eq!(index.hidden(source), index.summary(display).is_some());
            }
            let (_, max) = index.scroll_metrics(0, 4);
            for offset in 0..=max {
                let physical = index.physical_offset(offset, 4);
                assert_eq!(index.scroll_metrics(physical, 4), (offset, max));
                let next = index.scroll(physical, 4, 1);
                assert_eq!(index.scroll_metrics(next, 4).0, (offset + 1).min(max));
            }
            screen.set_collapsed_commands(ids);
            for offset in [0, screen.scrollback_len() / 2, screen.scrollback_len()] {
                let snapshot = screen.viewport_snapshot(offset);
                let index = snapshot.display_rows.as_ref().unwrap();
                let needed = index.source_window(offset, 4);
                let (start, end) =
                    crate::snapshot_window::terminal_snapshot_absolute_window(&snapshot).unwrap();
                assert!(start <= needed.start && end >= needed.end);
            }
        }
    }

    #[test]
    fn history_eviction_removes_stale_command_identities() {
        let mut screen = TerminalScreen::new(20, 3);
        screen.set_scrollback_limit(3);
        screen.advance(b"$ \x1b]133;B\x07run\r\n\x1b]133;C\x07out\r\n\x1b]133;D;0\x07");
        let id = screen.snapshot().command_blocks[0].id;
        screen.set_collapsed_commands([id].into());
        screen.advance(b"1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n");
        let snapshot = screen.snapshot();
        assert!(snapshot.command_blocks.iter().all(|block| block.id != id));
        let index = snapshot.display_rows.as_ref().unwrap();
        assert!((0..index.rows()).all(|row| index.summary(row).is_none()));
    }
}
