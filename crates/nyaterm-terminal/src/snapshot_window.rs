//! Bounded viewport snapshots used while navigating scrollback.
use crate::{TerminalScreen, TerminalSnapshot, TerminalSnapshotBuildStats};
use std::sync::Arc;
use std::time::{Duration, Instant};

const TERMINAL_FRAME_SCROLL_WINDOW_MIN_EXTRA_ROWS: usize = 32;
const TERMINAL_FRAME_SCROLL_WINDOW_MAX_EXTRA_ROWS: usize = 192;
const TERMINAL_FRAME_PRIORITY_SCROLL_WINDOW_MIN_EXTRA_ROWS: usize = 64;
const TERMINAL_FRAME_PRIORITY_SCROLL_WINDOW_MAX_EXTRA_ROWS: usize = 256;

pub fn terminal_frame_scroll_window_extra_rows(viewport_rows: usize, priority: bool) -> usize {
    let viewport_rows = viewport_rows.max(1);
    if priority {
        return viewport_rows.saturating_mul(3).clamp(
            TERMINAL_FRAME_PRIORITY_SCROLL_WINDOW_MIN_EXTRA_ROWS,
            TERMINAL_FRAME_PRIORITY_SCROLL_WINDOW_MAX_EXTRA_ROWS,
        );
    }
    viewport_rows.saturating_mul(2).clamp(
        TERMINAL_FRAME_SCROLL_WINDOW_MIN_EXTRA_ROWS,
        TERMINAL_FRAME_SCROLL_WINDOW_MAX_EXTRA_ROWS,
    )
}

pub fn terminal_frame_snapshot_with_scroll_window(
    screen: &TerminalScreen,
    offset: usize,
    priority: bool,
) -> Arc<TerminalSnapshot> {
    terminal_frame_snapshot_with_scroll_window_and_stats(screen, offset, priority).0
}

pub fn terminal_frame_snapshot_with_scroll_window_and_stats(
    screen: &TerminalScreen,
    offset: usize,
    priority: bool,
) -> (Arc<TerminalSnapshot>, Duration, TerminalSnapshotBuildStats) {
    let extra_rows = terminal_frame_scroll_window_extra_rows(screen.rows(), priority);
    terminal_frame_snapshot_with_extra_rows_and_stats(screen, offset, extra_rows)
}

pub fn terminal_frame_live_snapshot_with_stats(
    screen: &TerminalScreen,
) -> (Arc<TerminalSnapshot>, Duration, TerminalSnapshotBuildStats) {
    let started_at = Instant::now();
    let (snapshot, stats) = screen.viewport_snapshot_with_stats(0);
    (Arc::new(snapshot), started_at.elapsed(), stats)
}

pub fn terminal_frame_snapshot_with_extra_rows_and_stats(
    screen: &TerminalScreen,
    offset: usize,
    extra_rows: usize,
) -> (Arc<TerminalSnapshot>, Duration, TerminalSnapshotBuildStats) {
    let started_at = Instant::now();
    let (snapshot, stats) =
        screen.viewport_snapshot_with_window_and_stats(offset, extra_rows, extra_rows);
    (Arc::new(snapshot), started_at.elapsed(), stats)
}

pub fn terminal_snapshot_matches_grid_geometry(
    snapshot: &TerminalSnapshot,
    cols: usize,
    viewport_rows: usize,
) -> bool {
    snapshot.cols == cols.max(1) && snapshot.viewport_rows == viewport_rows.max(1)
}

pub fn terminal_snapshot_absolute_window(snapshot: &TerminalSnapshot) -> Option<(usize, usize)> {
    if snapshot.row_count() == 0 {
        return None;
    }
    let end = snapshot.total_rows.saturating_sub(snapshot.display_offset);
    let start = end.saturating_sub(snapshot.row_count());
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::terminal_frame_scroll_window_extra_rows;

    #[test]
    fn terminal_frame_scroll_window_extra_rows_covers_fast_scroll_runs() {
        assert_eq!(terminal_frame_scroll_window_extra_rows(12, false), 32);
        assert_eq!(terminal_frame_scroll_window_extra_rows(40, false), 80);
        assert_eq!(terminal_frame_scroll_window_extra_rows(120, false), 192);
        assert_eq!(terminal_frame_scroll_window_extra_rows(12, true), 64);
        assert_eq!(terminal_frame_scroll_window_extra_rows(40, true), 120);
        assert_eq!(terminal_frame_scroll_window_extra_rows(160, true), 256);
    }
}
