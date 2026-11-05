//! Row background policy shared by the terminal grid and its gutter.

use nyaterm_terminal::{TerminalLineId, TerminalSnapshotRow};
use nyaterm_ui::ThemePalette;

#[derive(Debug, Clone, Copy)]
pub enum TerminalStripeMode {
    ShellInput,
    Rows,
    UnwrappedRows,
}

impl TerminalStripeMode {
    pub fn new(by_row: bool, unwrapped: bool) -> Self {
        match (by_row, unwrapped) {
            (false, _) => Self::ShellInput,
            (true, false) => Self::Rows,
            (true, true) => Self::UnwrappedRows,
        }
    }
}

pub fn terminal_row_stripe_color(
    row: &TerminalSnapshotRow,
    mode: TerminalStripeMode,
    target_line: Option<TerminalLineId>,
    palette: ThemePalette,
) -> Option<u32> {
    let id = row.line_id?;
    if row.cleared {
        return None;
    }
    if Some(id) == target_line {
        return Some((palette.terminal_line_highlight << 8) | 0xff);
    }
    let highlighted = match mode {
        TerminalStripeMode::ShellInput => row.shell_input.is_some(),
        TerminalStripeMode::Rows => id.logical_line.rem_euclid(2) == 1,
        TerminalStripeMode::UnwrappedRows => row.unwrapped_line_index?.rem_euclid(2) == 1,
    };
    highlighted.then_some((palette.terminal_fg << 8) | 0x0f)
}

#[cfg(test)]
mod tests {
    use nyaterm_terminal::TerminalScreen;

    use super::{TerminalStripeMode, terminal_row_stripe_color};
    use crate::unwrapped::TerminalUnwrappedLayout;

    #[test]
    fn row_stripes_alternate_across_input_output_and_blank_rows() {
        let mut screen = TerminalScreen::new(20, 6);
        screen.advance(b"$ \x1b]133;B\x07echo hi\r\n\x1b]133;C\x07output\r\n\r\nmore");
        let snapshot = screen.snapshot();
        let palette = nyaterm_ui::theme_palette("github-dark");
        let colors = snapshot
            .rows()
            .iter()
            .map(|row| terminal_row_stripe_color(row, TerminalStripeMode::Rows, None, palette))
            .collect::<Vec<_>>();
        for pair in colors.windows(2) {
            assert_ne!(pair[0].is_some(), pair[1].is_some());
        }
        assert_eq!(snapshot.row(2).unwrap().text, "");
        for (index, row) in snapshot.rows().iter().enumerate() {
            assert_eq!(
                terminal_row_stripe_color(row, TerminalStripeMode::ShellInput, None, palette)
                    .is_some(),
                index == 0,
            );
        }
        let row = snapshot.row(0).unwrap();
        assert_eq!(
            terminal_row_stripe_color(row, TerminalStripeMode::Rows, row.line_id, palette),
            Some((palette.terminal_line_highlight << 8) | 0xff)
        );
        let mut excluded = row.clone();
        excluded.cleared = true;
        assert!(
            terminal_row_stripe_color(&excluded, TerminalStripeMode::Rows, None, palette).is_none()
        );
        excluded.cleared = false;
        excluded.line_id = None;
        assert!(
            terminal_row_stripe_color(&excluded, TerminalStripeMode::Rows, None, palette).is_none()
        );
    }

    #[test]
    fn reflow_clear_and_reset_do_not_keep_stale_stripes_or_target_ids() {
        let palette = nyaterm_ui::theme_palette("github-dark");
        let mut screen = TerminalScreen::new(8, 4);
        screen.advance(b"abcdefghijk\r\nsecond");
        let old_target = screen.snapshot().row(0).unwrap().line_id;
        screen.resize(4, 6);
        let reflowed = screen.snapshot();
        for pair in reflowed.rows().windows(2) {
            let colors = pair
                .iter()
                .map(|row| {
                    terminal_row_stripe_color(row, TerminalStripeMode::Rows, old_target, palette)
                })
                .collect::<Vec<_>>();
            assert_ne!(colors[0].is_some(), colors[1].is_some());
        }
        screen.clear_except_input();
        let cleared = screen.snapshot();
        assert!(cleared.rows().iter().any(|row| row.cleared));
        for row in cleared.rows().iter().filter(|row| row.cleared) {
            assert!(
                terminal_row_stripe_color(row, TerminalStripeMode::Rows, None, palette).is_none()
            );
        }
        screen.clear();
        for row in screen.snapshot().rows() {
            assert_ne!(row.line_id, old_target);
            assert_ne!(
                terminal_row_stripe_color(row, TerminalStripeMode::Rows, old_target, palette),
                Some((palette.terminal_line_highlight << 8) | 0xff)
            );
        }
    }

    #[test]
    fn unwrapped_rows_alternate_independently_of_soft_wrap_count_and_horizontal_scroll() {
        let mut screen = TerminalScreen::new(4, 5);
        screen.set_logical_line_snapshots(true);
        screen.advance(b"abcdefgh\r\nijklmnop\r\nqrst");
        let snapshot = screen.snapshot();
        let palette = nyaterm_ui::theme_palette("github-dark");
        let layouts =
            [0.0, 3.0].map(|offset| TerminalUnwrappedLayout::new(&snapshot, 0, 3, offset).unwrap());
        let mut previous = None;
        for display_row in 0..3 {
            let source = layouts[0].first_source_row(display_row);
            let row = snapshot.row(source).unwrap();
            let color =
                terminal_row_stripe_color(row, TerminalStripeMode::UnwrappedRows, None, palette);
            if let Some(previous) = previous {
                assert_ne!(previous, color.is_some());
            }
            previous = Some(color.is_some());
            assert_eq!(source, layouts[1].first_source_row(display_row));
            if row.wraps_to_next {
                let target = snapshot.row(source + 1).unwrap().line_id;
                for layout in &layouts {
                    let mapped = layout.target_line_for_display_row(&snapshot, display_row, target);
                    assert_eq!(mapped, row.line_id);
                    assert_eq!(
                        terminal_row_stripe_color(
                            row,
                            TerminalStripeMode::UnwrappedRows,
                            mapped,
                            palette
                        ),
                        Some((palette.terminal_line_highlight << 8) | 0xff)
                    );
                }
            }
        }
    }
}
