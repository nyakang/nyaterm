//! Immutable, paged index of soft-wrapped logical lines in retained grid data.
//!
//! Pages are shared by frames. Output copies only touched pages; rank/select
//! queries do not scan terminal text or materialize arbitrarily long lines.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use crate::TerminalHistoryState;

const PAGE_ROWS: i64 = 256;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Page {
    ends: [u64; 4],
    spacers: [u64; 4],
}

impl Page {
    fn set(&mut self, row: usize, end: bool, spacer: bool) {
        let bit = 1 << (row % 64);
        for (words, value) in [(&mut self.ends, end), (&mut self.spacers, spacer)] {
            if value {
                words[row / 64] |= bit;
            } else {
                words[row / 64] &= !bit;
            }
        }
    }

    fn count(&self, range: Range<usize>, spacers: bool) -> usize {
        let words = if spacers { &self.spacers } else { &self.ends };
        (0..4)
            .map(|word| {
                let lo = range.start.saturating_sub(word * 64).min(64);
                let hi = range.end.saturating_sub(word * 64).min(64);
                if lo >= hi {
                    0
                } else {
                    let low = u64::MAX.checked_shl(lo as u32).unwrap_or(0);
                    let high = u64::MAX.checked_shr((64 - hi) as u32).unwrap_or(0);
                    (words[word] & low & high).count_ones() as usize
                }
            })
            .sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IndexedPage {
    number: i64,
    data: Arc<Page>,
    ends_before: usize,
    spacers_before: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalLogicalLineIndex {
    start: i64,
    end: i64,
    cols: usize,
    pages: Vec<IndexedPage>,
}

impl TerminalLogicalLineIndex {
    pub fn physical_rows(&self) -> usize {
        (self.end - self.start).max(0) as usize
    }

    fn rank(&self, row: usize, spacers: bool) -> usize {
        let position = (self.start + row.min(self.physical_rows()) as i64).min(self.end);
        let number = position.div_euclid(PAGE_ROWS);
        let i = self.pages.partition_point(|page| page.number < number);
        if let Some(page) = self.pages.get(i).filter(|page| page.number == number) {
            let begin = (self.start - number * PAGE_ROWS).clamp(0, PAGE_ROWS) as usize;
            let end = (position - number * PAGE_ROWS) as usize;
            let before = if spacers {
                page.spacers_before
            } else {
                page.ends_before
            };
            before + page.data.count(begin..end, spacers)
        } else if let Some(page) = self.pages.last() {
            let begin = (self.start - page.number * PAGE_ROWS).clamp(0, PAGE_ROWS) as usize;
            let end = (self.end - page.number * PAGE_ROWS).clamp(0, PAGE_ROWS) as usize;
            let before = if spacers {
                page.spacers_before
            } else {
                page.ends_before
            };
            before + page.data.count(begin..end, spacers)
        } else {
            0
        }
    }

    pub fn logical_rows(&self) -> usize {
        if self.physical_rows() == 0 {
            0
        } else {
            self.rank(self.physical_rows() - 1, false) + 1
        }
    }

    pub fn logical_row(&self, physical_row: usize) -> usize {
        self.rank(
            physical_row.min(self.physical_rows().saturating_sub(1)),
            false,
        )
    }

    pub fn physical_row(&self, logical_row: usize) -> usize {
        if logical_row == 0 {
            return 0;
        }
        if logical_row >= self.logical_rows() {
            return self.physical_rows();
        }
        // Rank is monotonic. Binary search remains bounded by the history limit.
        let mut lo = 0;
        let mut hi = self.physical_rows();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.rank(mid, false) < logical_row {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    pub fn group(&self, physical_row: usize) -> Range<usize> {
        let row = self.logical_row(physical_row);
        self.physical_row(row)..self.physical_row(row + 1)
    }

    pub fn row_width(&self, physical_row: usize) -> usize {
        self.cols
            .saturating_sub(self.rank(physical_row + 1, true) - self.rank(physical_row, true))
    }

    pub fn logical_col(&self, physical_row: usize, col: usize) -> usize {
        let start = self.physical_row(self.logical_row(physical_row));
        (physical_row - start)
            .saturating_mul(self.cols)
            .saturating_sub(self.rank(physical_row, true) - self.rank(start, true))
            + col
    }

    pub fn source_position(&self, logical_row: usize, col: usize) -> (usize, usize) {
        let start = self.physical_row(logical_row);
        let end = self.physical_row(logical_row + 1);
        if start >= end {
            return (start, 0);
        }
        let mut lo = start;
        let mut hi = end;
        while lo + 1 < hi {
            let mid = lo + (hi - lo) / 2;
            if self.logical_col(mid, 0) <= col {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        (
            lo,
            col.saturating_sub(self.logical_col(lo, 0))
                .min(self.cols.saturating_sub(1)),
        )
    }

    pub fn scroll_metrics(&self, physical_offset: usize, viewport: usize) -> (usize, usize) {
        let max = self.logical_rows().saturating_sub(viewport);
        let top = if physical_offset == 0 {
            max
        } else {
            self.logical_row(
                self.physical_rows()
                    .saturating_sub(viewport)
                    .saturating_sub(physical_offset),
            )
            .min(max)
        };
        (max.saturating_sub(top), max)
    }

    pub fn physical_offset_for_logical_offset(
        &self,
        logical_offset: usize,
        viewport: usize,
    ) -> usize {
        if logical_offset == 0 {
            return 0;
        }
        let max = self.logical_rows().saturating_sub(viewport);
        self.physical_rows()
            .saturating_sub(viewport)
            .saturating_sub(self.physical_row(max.saturating_sub(logical_offset)))
    }

    /// Physical offset from live bottom for a logical viewport. All public
    /// terminal/session offsets remain physical grid offsets.
    pub fn offset_for_logical_scroll(&self, offset: usize, viewport: usize, delta: i32) -> usize {
        let total = self.physical_rows();
        let anchor = total.saturating_sub(viewport).saturating_sub(offset);
        let logical = if offset == 0 {
            self.logical_rows().saturating_sub(viewport)
        } else {
            self.logical_row(anchor)
        };
        let target = if delta >= 0 {
            logical.saturating_sub(delta as usize)
        } else {
            logical.saturating_add(delta.unsigned_abs() as usize)
        };
        let max = self.logical_rows().saturating_sub(viewport);
        let target = target.min(max);
        if target == max {
            0
        } else {
            total
                .saturating_sub(viewport)
                .saturating_sub(self.physical_row(target))
        }
    }
}

#[derive(Default)]
pub(crate) struct LogicalLineIndexCache {
    history: Option<TerminalHistoryState>,
    end: i64,
    pages: BTreeMap<i64, Arc<Page>>,
    index: Arc<TerminalLogicalLineIndex>,
}

impl LogicalLineIndexCache {
    pub(crate) fn update(
        &mut self,
        history: TerminalHistoryState,
        origin: i64,
        damaged: &[i32],
        mut flags: impl FnMut(i32) -> (bool, bool),
    ) {
        let start = origin - history.scrollback_len as i64;
        let end = origin + history.viewport_rows as i64;
        let reset = self.history.is_none_or(|old| {
            old.epoch != history.epoch
                || old.cols != history.cols
                || old.alternate_screen != history.alternate_screen
                || end < self.end
        });
        if reset {
            self.pages.clear();
        }
        self.pages
            .retain(|number, _| (*number + 1) * PAGE_ROWS > start && *number * PAGE_ROWS < end);
        let scan_start = if reset {
            start
        } else {
            self.end
                .saturating_sub(self.history.map_or(0, |old| old.viewport_rows) as i64)
                .min(origin)
                .max(start)
        };
        for position in scan_start..end {
            self.update_row(position, flags((position - origin) as i32));
        }
        for &line in damaged {
            let position = origin + i64::from(line);
            if position >= start && position < end && position < scan_start {
                self.update_row(position, flags(line));
            }
        }
        let mut ends = 0;
        let mut spacers = 0;
        let pages = self
            .pages
            .iter()
            .map(|(&number, data)| {
                let page = IndexedPage {
                    number,
                    data: data.clone(),
                    ends_before: ends,
                    spacers_before: spacers,
                };
                let lo = (start - number * PAGE_ROWS).clamp(0, PAGE_ROWS) as usize;
                let hi = (end - number * PAGE_ROWS).clamp(0, PAGE_ROWS) as usize;
                ends += data.count(lo..hi, false);
                spacers += data.count(lo..hi, true);
                page
            })
            .collect();
        self.index = Arc::new(TerminalLogicalLineIndex {
            start,
            end,
            cols: history.cols,
            pages,
        });
        self.history = Some(history);
        self.end = end;
    }

    fn update_row(&mut self, position: i64, (wrap, spacer): (bool, bool)) {
        let page = self
            .pages
            .entry(position.div_euclid(PAGE_ROWS))
            .or_default();
        let row = position.rem_euclid(PAGE_ROWS) as usize;
        let bit = 1 << (row % 64);
        if (page.ends[row / 64] & bit != 0) != !wrap
            || (page.spacers[row / 64] & bit != 0) != spacer
        {
            Arc::make_mut(page).set(row, !wrap, spacer);
        }
    }

    pub(crate) fn snapshot(&self) -> Arc<TerminalLogicalLineIndex> {
        self.index.clone()
    }
}

#[cfg(test)]
mod tests {
    use crate::TerminalScreen;
    use std::sync::Arc;

    #[test]
    fn frames_share_long_line_source_rows_beyond_the_ordinary_row_cache_limit() {
        let mut screen = TerminalScreen::new(4, 3);
        screen.set_scrollback_limit(10_000);
        screen.set_logical_line_snapshots(true);
        screen.advance("x".repeat(4 * 9_000).as_bytes());
        let old = screen.snapshot();
        screen.advance(b"y");
        let (next, stats) = screen.viewport_snapshot_with_stats(0);
        assert!(old.row_count() > crate::TERMINAL_SNAPSHOT_ROW_CACHE_LIMIT);
        assert!(Arc::ptr_eq(&old.rows()[0], &next.rows()[0]));
        assert!(stats.rebuilt_rows <= 6, "{stats:?}");
    }

    #[test]
    fn soft_wrap_marker_requires_an_actual_wrap_and_tracks_reflow() {
        let mut screen = TerminalScreen::new(4, 3);
        screen.advance(b"abcd");
        assert!(!screen.snapshot().row(0).unwrap().wraps_to_next);
        screen.advance(b"e");
        assert!(screen.snapshot().row(0).unwrap().wraps_to_next);
        screen.advance(b"\r\nf");
        assert!(!screen.snapshot().row(1).unwrap().wraps_to_next);
        screen.resize(8, 3);
        assert!(
            screen
                .snapshot()
                .rows()
                .iter()
                .all(|row| !row.wraps_to_next)
        );
    }

    #[test]
    fn active_graphics_use_grid_until_the_placement_is_deleted() {
        let mut screen = TerminalScreen::new(8, 4);
        screen.set_logical_line_snapshots(true);
        screen.advance(b"\x1b_Ga=T,f=100,i=7,c=2,r=1;QUJD\x1b\\");
        assert!(screen.snapshot().logical_lines.is_none());
        screen.advance(b"\x1b_Ga=d,d=A\x1b\\");
        assert!(screen.snapshot().logical_lines.is_some());
    }

    #[test]
    fn logical_index_joins_only_soft_wraps_and_keeps_spaces() {
        let mut screen = TerminalScreen::new(4, 3);
        screen.set_logical_line_snapshots(true);
        screen.advance(b"ab  cd\r\nend");
        let snapshot = screen.snapshot();
        let index = snapshot.logical_lines.as_ref().unwrap();
        assert_eq!(index.logical_row(0), index.logical_row(1));
        assert_ne!(index.logical_row(1), index.logical_row(2));
        assert_eq!(index.logical_col(1, 1), 5);
        assert_eq!(index.source_position(0, 5), (1, 1));
        let selection = crate::selection::TerminalSelection::from_range(
            crate::selection::TerminalBufferCellPos::new(0, 0),
            crate::selection::TerminalBufferCellPos::new(1, 1),
        );
        assert_eq!(
            screen.text_for_selection(selection).unwrap().as_deref(),
            Some("ab  cd")
        );
        assert_eq!(screen.all_text().as_deref(), Some("ab  cd\nend"));
        assert_eq!(
            screen
                .text_for_selection(crate::selection::TerminalSelection::all_buffer(4))
                .unwrap(),
            screen.all_text()
        );
    }

    #[test]
    fn wide_character_wrap_spacer_is_not_a_logical_column() {
        let mut screen = TerminalScreen::new(4, 3);
        screen.set_logical_line_snapshots(true);
        screen.advance("abc中文".as_bytes());
        let snapshot = screen.snapshot();
        let index = snapshot.logical_lines.as_ref().unwrap();
        assert_eq!(index.logical_col(1, 0), 3);
        assert_eq!(index.source_position(0, 3), (1, 0));
        assert_eq!(index.source_position(0, 4), (1, 1));
    }

    #[test]
    fn snapshots_include_complete_group_above_the_live_viewport() {
        let mut screen = TerminalScreen::new(4, 2);
        screen.set_logical_line_snapshots(true);
        screen.advance(b"abcdefghijklmnop");
        let snapshot = screen.snapshot();
        assert!(snapshot.row_count() >= 4);
        assert_eq!(snapshot.row(0).unwrap().text, "abcd");
        assert_eq!(snapshot.row(snapshot.cursor.row).unwrap().text, "mnop");
    }

    #[test]
    fn old_index_remains_immutable_after_output_reflow_and_history_eviction() {
        let mut screen = TerminalScreen::new(4, 3);
        screen.set_scrollback_limit(300);
        screen.set_logical_line_snapshots(true);
        for _ in 0..280 {
            screen.advance(b"a\r\n");
        }
        let old = screen.snapshot();
        let index = old.logical_lines.as_ref().unwrap().clone();
        let count = index.logical_rows();
        for _ in 0..400 {
            screen.advance(b"abcdefgh\r\n");
        }
        assert_eq!(index.logical_rows(), count);
        let current = screen.snapshot();
        let next = current.logical_lines.as_ref().unwrap();
        assert_eq!(next.physical_rows(), screen.total_rows());
        for row in 0..next.physical_rows() {
            let logical = next.logical_row(row);
            assert!(next.physical_row(logical) <= row);
            assert!(next.physical_row(logical + 1) > row);
        }
        screen.resize(8, 3);
        let resized = screen.snapshot();
        assert!(!Arc::ptr_eq(next, resized.logical_lines.as_ref().unwrap()));
        screen.clear();
        assert_eq!(
            screen
                .snapshot()
                .logical_lines
                .as_ref()
                .unwrap()
                .physical_rows(),
            3
        );
    }

    #[test]
    fn coordinate_sensitive_modes_disable_and_restore_logical_frames() {
        let mut screen = TerminalScreen::new(8, 4);
        screen.set_logical_line_snapshots(true);
        for (enter, leave) in [
            ("\x1b[?1049h", "\x1b[?1049l"),
            ("\x1b[?1000h", "\x1b[?1000l"),
            ("\x1b[?1h", "\x1b[?1l"),
            ("\x1b=", "\x1b>"),
            ("\x1b[>1u", "\x1b[<u"),
            ("\x1b[2;3r", "\x1b[r"),
        ] {
            screen.advance(enter.as_bytes());
            assert!(screen.snapshot().logical_lines.is_none(), "{enter:?}");
            screen.advance(leave.as_bytes());
            assert!(screen.snapshot().logical_lines.is_some(), "{leave:?}");
            assert_eq!((screen.cols(), screen.rows()), (8, 4));
        }
    }

    #[test]
    fn redundant_resize_preserves_partial_scroll_region_fallback() {
        let mut screen = TerminalScreen::new(8, 4);
        screen.set_logical_line_snapshots(true);
        screen.advance(b"\x1b[2;3r");
        screen.resize(8, 4);
        assert_eq!(screen.scroll_region(), (1, 2));
        assert!(screen.snapshot().logical_lines.is_none());
        screen.resize(9, 4);
        assert_eq!(screen.scroll_region(), (0, 3));
        assert!(screen.snapshot().logical_lines.is_some());

        // Match VT's clamping of an oversized region to an empty range.
        screen.advance(b"\x1b[5;99r");
        assert_eq!(screen.scroll_region(), (4, 3));
        assert!(screen.snapshot().logical_lines.is_none());
        screen.advance(b"\x1b[r");
        assert!(screen.snapshot().logical_lines.is_some());
    }

    #[test]
    fn scrolling_from_live_bottom_moves_by_logical_lines() {
        let mut screen = TerminalScreen::new(4, 2);
        screen.set_logical_line_snapshots(true);
        screen.advance(b"abcdefgh\r\nijklmnop\r\nqrstuvwx");
        let snapshot = screen.snapshot();
        let index = snapshot.logical_lines.as_ref().unwrap();
        let offset = index.offset_for_logical_scroll(0, 2, 1);
        assert!(offset > 0);
        assert_eq!(index.offset_for_logical_scroll(offset, 2, -1), 0);
    }
}
