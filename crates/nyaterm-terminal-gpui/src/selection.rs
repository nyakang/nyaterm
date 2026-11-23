//! Selection drag geometry, independent of application routing.
pub fn terminal_selection_autoscroll_delta(
    pointer_y: f32,
    top: f32,
    bottom: f32,
    cell_height: f32,
) -> i32 {
    if !pointer_y.is_finite() || bottom <= top {
        return 0;
    }
    // Only a pointer outside the text grid scrolls. Treating the first and
    // last rows as edge bands makes a drag that reaches the first visible line
    // jump a row into scrollback, and a drag that stops there scrolls away
    // until the history runs out.
    let edge = cell_height.max(1.0).min((bottom - top) / 4.0);
    let distance = if pointer_y < top {
        top + edge - pointer_y
    } else if pointer_y > bottom {
        bottom - edge - pointer_y
    } else {
        return 0;
    };
    let steps = (1.0 + distance.abs() / cell_height.max(1.0)).floor() as i32;
    if distance > 0.0 {
        steps.clamp(1, 6)
    } else {
        -steps.clamp(1, 6)
    }
}

pub fn terminal_selection_line_after_scroll(
    line: usize,
    previous_offset: usize,
    next_offset: usize,
    max_line: usize,
) -> usize {
    if next_offset > previous_offset {
        line.saturating_sub(next_offset - previous_offset)
    } else {
        line.saturating_add(previous_offset - next_offset)
            .min(max_line)
    }
}

#[cfg(test)]
mod tests {
    use super::{terminal_selection_autoscroll_delta, terminal_selection_line_after_scroll};
    #[test]
    fn dragging_below_terminal_advances_selection_toward_newer_lines() {
        assert_eq!(
            terminal_selection_autoscroll_delta(101.0, 0.0, 100.0, 10.0),
            -2
        );
        assert_eq!(terminal_selection_line_after_scroll(42, 8, 6, 99), 44);
        assert_eq!(terminal_selection_line_after_scroll(99, 2, 0, 99), 99);
    }
    #[test]
    fn dragging_above_terminal_advances_selection_into_scrollback() {
        assert_eq!(
            terminal_selection_autoscroll_delta(-1.0, 0.0, 100.0, 10.0),
            2
        );
        assert_eq!(terminal_selection_line_after_scroll(42, 6, 8, 99), 40);
        assert_eq!(terminal_selection_line_after_scroll(0, 98, 99, 99), 0);
    }
    #[test]
    fn drag_inside_the_visible_rows_selects_without_autoscrolling() {
        // The first and last rows are text: a drag that reaches them selects
        // them instead of scrolling the view.
        for pointer_y in [0.0, 4.5, 9.9, 50.0, 90.0, 100.0] {
            assert_eq!(
                terminal_selection_autoscroll_delta(pointer_y, 0.0, 100.0, 10.0),
                0
            );
        }
        // Leaving the text grid still scrolls with the same ramp.
        assert_eq!(
            terminal_selection_autoscroll_delta(-1.0, 0.0, 100.0, 10.0),
            2
        );
        assert_eq!(
            terminal_selection_autoscroll_delta(100.1, 0.0, 100.0, 10.0),
            -2
        );
    }
    #[test]
    fn wheel_scroll_during_drag_keeps_selection_endpoint_with_viewport() {
        assert_eq!(
            terminal_selection_autoscroll_delta(50.0, 0.0, 100.0, 10.0),
            0
        );
        assert_eq!(terminal_selection_line_after_scroll(40, 4, 7, 99), 37);
        assert_eq!(terminal_selection_line_after_scroll(37, 7, 5, 99), 39);
    }
}
