//! Manual virtual-list window size for accelerator process cards.
//!
//! This lives here rather than in the view because the offset it bounds is
//! authoritative state: `RemoteOpsFeatureState` clamps a stored scroll offset whenever
//! the list behind it changes length, and it needs the viewport height to know what the
//! maximum offset is. Docker and the main process table use GPUI-owned scroll handles.

/// Rows visible in a GPU/NPU card's process list.
pub(in crate::features) const ACCELERATOR_PROCESS_VIEWPORT_ROWS: usize = 6;

/// The largest scroll offset that still shows a full viewport.
///
/// `min` before `saturating_sub` so a list shorter than the viewport pins to zero
/// rather than going negative.
pub(in crate::features) fn max_list_offset(total: usize, viewport_rows: usize) -> usize {
    total.saturating_sub(viewport_rows.min(total))
}

#[cfg(test)]
mod tests {
    use super::max_list_offset;

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
}
