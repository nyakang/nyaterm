//! Virtual-list window sizes for the remote panels.
//!
//! These live here rather than in the views because the offsets they bound are
//! authoritative state: `RemoteOpsFeatureState` clamps a stored scroll offset whenever
//! the list behind it changes length, and it needs the viewport height to know what the
//! maximum offset is. The views import the same constants for their own windowing, so
//! there is one definition per list rather than one per reader.
//!
//! Docker's two were previously declared twice each -- `DOCKER_VIEWPORT_ROWS = 16` in
//! `docker/containers.rs` beside a bare `VIEWPORT_ROWS = 16` in `docker_view.rs`, and
//! the same for the resource list's 14 -- so the clamp and the window it was clamping
//! for agreed only by coincidence.

/// Container rows visible in the Docker containers list.
pub(in crate::features) const DOCKER_VIEWPORT_ROWS: usize = 16;

/// Rows visible in the Docker images/volumes/networks lists.
pub(in crate::features) const DOCKER_RESOURCE_VIEWPORT_ROWS: usize = 14;

/// Rows visible in the process table.
pub(in crate::features) const PROCESS_VIEWPORT_ROWS: usize = 28;

/// Rows visible in a GPU/NPU card's process list.
pub(in crate::features) const ACCELERATOR_PROCESS_VIEWPORT_ROWS: usize = 6;

/// The largest scroll offset that still shows a full viewport.
///
/// `min` before `saturating_sub` so a list shorter than the viewport pins to zero
/// rather than going negative.
pub(in crate::features) fn max_list_offset(total: usize, viewport_rows: usize) -> usize {
    total.saturating_sub(viewport_rows.min(total))
}

/// The rows rendered by a wheel-driven list that owns its offset as state.
///
/// These lists do not move a native scroll container. Changing `offset` replaces the
/// rendered rows, so the window must begin at the offset itself. Leading spacer rows
/// would be visible content and create an increasingly large blank area while scrolling.
/// Overscan is therefore added only after the visible viewport.
pub(in crate::features) fn state_scrolled_list_range(
    total: usize,
    offset: usize,
    viewport_rows: usize,
    overscan_rows: usize,
) -> std::ops::Range<usize> {
    let start = offset.min(max_list_offset(total, viewport_rows));
    let end = start
        .saturating_add(viewport_rows)
        .saturating_add(overscan_rows)
        .min(total);
    start..end
}

#[cfg(test)]
mod tests {
    use super::{max_list_offset, state_scrolled_list_range};

    #[test]
    fn a_list_shorter_than_the_viewport_pins_to_the_top() {
        assert_eq!(max_list_offset(0, 16), 0);
        assert_eq!(max_list_offset(1, 16), 0);
        assert_eq!(max_list_offset(16, 16), 0);
    }

    #[test]
    fn a_longer_list_can_scroll_by_the_overflow() {
        assert_eq!(max_list_offset(17, 16), 1);
        assert_eq!(max_list_offset(100, 16), 84);
    }

    #[test]
    fn a_state_scrolled_window_starts_at_the_requested_offset() {
        assert_eq!(state_scrolled_list_range(100, 20, 16, 6), 20..42);
    }

    #[test]
    fn a_state_scrolled_window_clamps_and_keeps_the_last_viewport_full() {
        assert_eq!(state_scrolled_list_range(100, usize::MAX, 16, 6), 84..100);
        assert_eq!(state_scrolled_list_range(5, 3, 16, 6), 0..5);
    }
}
