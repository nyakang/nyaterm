mod input;

pub use input::{CommandSubmissionEvidence, command_submission_evidence, visible_shell_prompt};

use crate::{TerminalCore, TerminalLineId};
use alacritty_terminal::{grid::Dimensions, index::Column, term::cell::Flags};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandNavigationAction {
    Previous,
    Next,
    Select,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandNavigationResult {
    pub display_offset: usize,
    pub selection: Option<(usize, usize)>,
    pub cols: usize,
    pub target_line: Option<TerminalLineId>,
}

#[derive(Debug, Default)]
pub(crate) struct CommandNavigationState {
    pub(crate) revision: u64,
    pub(crate) position: Option<i64>,
    pub(crate) selection: Option<(i64, i64)>,
}

impl TerminalCore {
    pub fn reset_command_navigation(&mut self) -> bool {
        // Navigation belongs to the primary buffer. Input in a TUI must also
        // end the suspended primary sequence before returning to the shell.
        let state = &mut self.primary_lines.command_navigation;
        let changed = state.position.is_some() || state.selection.is_some();
        if changed {
            let revision = state.revision.wrapping_add(1);
            *state = CommandNavigationState {
                revision,
                ..Default::default()
            };
        }
        changed
    }

    pub fn command_navigation_target(&self) -> Option<TerminalLineId> {
        if self.alternate_screen() {
            return None;
        }
        let state = self.active_line_state();
        let line = state.command_navigation.position?;
        state
            .metadata
            .get(&line)
            .filter(|metadata| metadata.command_anchor)?;
        Some(TerminalLineId {
            epoch: state.epoch,
            logical_line: line,
        })
    }

    /// Submission evidence names its original row even if echo has already scrolled.
    pub fn record_fallback_command_at(&mut self, anchor: TerminalLineId) {
        if self.alternate_screen() || anchor.epoch != self.primary_lines.epoch {
            return;
        }
        let first = self.primary_lines.logical_line(self.term.topmost_line());
        let last = self.primary_lines.logical_line(self.term.bottommost_line());
        if !(first..=last).contains(&anchor.logical_line) {
            return;
        }
        self.primary_lines
            .metadata
            .entry(anchor.logical_line)
            .or_default()
            .command_anchor = true;
        // A missing OSC output boundary must not leave this prompt available
        // for the next submission. A late boundary already shares this anchor.
        if self.primary_lines.pending_command_prompt == Some(anchor.logical_line) {
            self.primary_lines.pending_command_prompt = None;
        }
    }

    pub fn record_fallback_command(&mut self) {
        if self.alternate_screen() {
            return;
        }
        let mut cursor = self.term.grid().cursor.point.line;
        while cursor > self.term.topmost_line()
            && self.term.grid()[cursor - 1usize][Column(self.cols - 1)]
                .flags
                .contains(Flags::WRAPLINE)
        {
            cursor -= 1usize;
        }
        let logical = self.active_line_state().logical_line(cursor);
        self.active_line_state_mut()
            .metadata
            .entry(logical)
            .or_default()
            .command_anchor = true;
    }

    pub fn navigate_command(
        &mut self,
        action: CommandNavigationAction,
    ) -> Option<CommandNavigationResult> {
        self.navigate_command_from_view(action, 0)
    }

    pub fn navigate_command_from_view(
        &mut self,
        action: CommandNavigationAction,
        display_offset: usize,
    ) -> Option<CommandNavigationResult> {
        if self.alternate_screen() {
            return None;
        }
        self.retain_active_presentation_range();
        let cursor = self
            .active_line_state()
            .logical_line(self.term.grid().cursor.point.line);
        let first = self
            .active_line_state()
            .logical_line(self.term.topmost_line());
        let origin = self.active_line_state().logical_origin;
        let anchors = self
            .active_line_state()
            .metadata
            .iter()
            .filter_map(|(line, metadata)| metadata.command_anchor.then_some(*line))
            .collect::<Vec<_>>();
        let history_size = self.term.grid().history_size();
        let state = &mut self.active_line_state_mut().command_navigation;
        if state.position.is_some_and(|line| !anchors.contains(&line)) {
            state.position = None;
        }
        if state
            .selection
            .is_some_and(|(start, end)| !anchors.contains(&start) || !anchors.contains(&end))
        {
            state.selection = None;
        }
        let view_origin = if display_offset == 0 {
            cursor
        } else {
            origin.saturating_sub(display_offset.min(history_size) as i64)
        };
        let current = state.position.unwrap_or(view_origin);
        let (target, selection) = match action {
            CommandNavigationAction::Previous => {
                let target = anchors.iter().copied().rev().find(|line| *line < current)?;
                state.position = Some(target);
                state.selection = None;
                (target, None)
            }
            CommandNavigationAction::Next => {
                let target = anchors
                    .iter()
                    .copied()
                    .find(|line| *line > current && *line < cursor)
                    .unwrap_or(cursor);
                if target <= current {
                    return None;
                }
                state.position = (target != cursor).then_some(target);
                state.selection = None;
                (target, None)
            }
            CommandNavigationAction::Select => {
                let start = state
                    .selection
                    .map(|(start, _)| start)
                    .or_else(|| anchors.iter().copied().rev().find(|line| *line <= current))?;
                let last = match state.selection {
                    Some((_, end)) => anchors.iter().copied().find(|line| *line > end)?,
                    None => start,
                };
                let end = anchors
                    .iter()
                    .copied()
                    .find(|line| *line > last)
                    .map_or(cursor, |line| line - 1)
                    .max(start);
                state.position = Some(start);
                state.selection = Some((start, last));
                (
                    start,
                    Some((
                        (start - first).max(0) as usize,
                        (end - first).max(0) as usize,
                    )),
                )
            }
        };
        state.revision = state.revision.wrapping_add(1);
        Some(CommandNavigationResult {
            display_offset: origin.saturating_sub(target).max(0) as usize,
            selection,
            cols: self.cols,
            target_line: self.command_navigation_target(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::CommandNavigationAction;
    use crate::{TerminalCore, TerminalLineId};

    #[test]
    fn shell_commands_navigate_extend_and_clear_with_the_grid() {
        let mut terminal = TerminalCore::new(20, 3);
        for _ in 0..3 {
            terminal.advance(
                b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07output\r\n\x1b]133;D;0\x07",
            );
        }
        let previous = terminal
            .navigate_command(CommandNavigationAction::Previous)
            .unwrap();
        let older = terminal
            .navigate_command(CommandNavigationAction::Previous)
            .unwrap();
        assert!(older.display_offset > previous.display_offset);
        let one = terminal
            .navigate_command(CommandNavigationAction::Select)
            .unwrap()
            .selection
            .unwrap();
        let two = terminal
            .navigate_command(CommandNavigationAction::Select)
            .unwrap()
            .selection
            .unwrap();
        assert_eq!(one.0, two.0);
        assert!(two.1 > one.1);
        terminal.clear_except_input();
        assert!(
            terminal
                .navigate_command(CommandNavigationAction::Previous)
                .is_none()
        );
    }
    #[test]
    fn fallback_wrap_eviction_resize_and_alternate_screen_follow_grid_lifetime() {
        let mut terminal = TerminalCore::new(8, 3);
        terminal.set_scrollback_limit(2);
        terminal.advance(b"prompt> wrapped input");
        terminal.record_fallback_command();
        terminal.advance(b"\r\nresult\r\n");
        let selection = terminal
            .navigate_command(CommandNavigationAction::Select)
            .unwrap()
            .selection
            .unwrap();
        assert_eq!(selection.0, 0);
        terminal.advance(b"\x1b[?1049h");
        assert!(
            terminal
                .navigate_command(CommandNavigationAction::Previous)
                .is_none()
        );
        terminal.advance(b"\x1b[?1049l");
        terminal.resize(16, 3);
        assert!(terminal.command_navigation_target().is_some());
        terminal.reset_command_navigation();
        assert!(
            terminal
                .navigate_command(CommandNavigationAction::Previous)
                .is_some()
        );
        terminal.record_fallback_command();
        for _ in 0..10 {
            terminal.advance(b"\r\noutput");
        }
        assert!(
            terminal
                .navigate_command(CommandNavigationAction::Previous)
                .is_none()
        );
    }
    fn emit_command(terminal: &mut TerminalCore) {
        terminal.advance(
            b"\x1b]133;A\x07$ \x1b]133;B\x07echo same\r\n\x1b]133;C\x07same\r\n\x1b]133;D;0\x07",
        );
    }

    #[test]
    fn five_commands_keep_navigation_through_snapshots_output_and_reflow() {
        let mut terminal = TerminalCore::new(20, 3);
        for _ in 0..5 {
            emit_command(&mut terminal);
        }
        let latest = terminal
            .navigate_command(CommandNavigationAction::Previous)
            .unwrap();
        let _ = terminal.viewport_snapshot(latest.display_offset);
        terminal.advance(b"$ ");
        terminal.resize(10, 4);
        let mut positions = vec![terminal.command_navigation_target().unwrap()];
        for _ in 0..4 {
            let previous = terminal
                .navigate_command(CommandNavigationAction::Previous)
                .unwrap();
            assert!(
                previous.target_line.unwrap().logical_line < positions.last().unwrap().logical_line
            );
            let _ = terminal.viewport_snapshot(previous.display_offset);
            positions.push(previous.target_line.unwrap());
        }
        assert!(
            terminal
                .navigate_command(CommandNavigationAction::Previous)
                .is_none()
        );
        for expected in positions[..4].iter().rev() {
            assert_eq!(
                terminal
                    .navigate_command(CommandNavigationAction::Next)
                    .unwrap()
                    .target_line,
                Some(*expected)
            );
        }
        let bottom = terminal
            .navigate_command(CommandNavigationAction::Next)
            .unwrap();
        assert_eq!(bottom.display_offset, 0);
        assert_eq!(bottom.target_line, None);
    }

    #[test]
    fn alternate_screen_resize_preserves_primary_anchors_and_pending_prompt() {
        let mut terminal = TerminalCore::new(20, 5);
        for _ in 0..3 {
            emit_command(&mut terminal);
        }
        terminal.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07echo next");
        terminal.advance(b"\x1b[?1049h");
        terminal.resize(10, 4);
        terminal.advance(b"\x1b[?1049l\r\n\x1b]133;C\x07next\r\n\x1b]133;D;0\x07");
        for _ in 0..4 {
            assert!(
                terminal
                    .navigate_command(CommandNavigationAction::Previous)
                    .is_some()
            );
        }
        assert!(
            terminal
                .navigate_command(CommandNavigationAction::Previous)
                .is_none()
        );
    }

    #[test]
    fn input_reset_on_alternate_screen_ends_the_primary_navigation_sequence() {
        let mut terminal = TerminalCore::new(40, 10);
        for _ in 0..3 {
            emit_command(&mut terminal);
        }
        let latest = terminal
            .navigate_command(CommandNavigationAction::Previous)
            .unwrap()
            .target_line;
        terminal.advance(b"\x1b[?1049h");
        assert!(terminal.reset_command_navigation());
        terminal.advance(b"\x1b[?1049l");
        assert_eq!(terminal.snapshot().command_navigation_target, None);
        assert_eq!(
            terminal
                .navigate_command(CommandNavigationAction::Previous)
                .unwrap()
                .target_line,
            latest
        );
    }

    #[test]
    fn wrapped_unicode_commands_are_points_instead_of_inherited_row_flags() {
        let mut terminal = TerminalCore::new(8, 4);
        for _ in 0..3 {
            terminal.advance("\x1b]133;A\x07$ \x1b]133;B\x07echo 中文中文中文\r\n\x1b]133;C\x07result\r\n\x1b]133;D;0\x07".as_bytes());
        }
        for (cols, rows) in [(4, 5), (16, 3), (8, 4)] {
            terminal.resize(cols, rows);
            assert_eq!(
                terminal
                    .primary_lines
                    .metadata
                    .values()
                    .filter(|m| m.command_anchor)
                    .count(),
                3
            );
        }
    }

    #[test]
    fn same_screen_targets_move_even_when_offsets_stay_zero() {
        let mut terminal = TerminalCore::new(40, 20);
        for _ in 0..5 {
            emit_command(&mut terminal);
        }
        let mut previous = None;
        for _ in 0..5 {
            let result = terminal
                .navigate_command(CommandNavigationAction::Previous)
                .unwrap();
            assert_eq!(result.display_offset, 0);
            let target = result.target_line.unwrap();
            assert_ne!(previous, Some(target));
            assert_eq!(terminal.snapshot().command_navigation_target, Some(target));
            previous = Some(target);
        }
        terminal.reset_command_navigation();
        assert_eq!(terminal.snapshot().command_navigation_target, None);
    }

    #[test]
    fn manual_view_origin_and_captured_submission_use_retained_coordinates() {
        let mut terminal = TerminalCore::new(20, 3);
        for _ in 0..5 {
            emit_command(&mut terminal);
        }
        let older = terminal
            .navigate_command_from_view(CommandNavigationAction::Previous, 5)
            .unwrap();
        terminal.reset_command_navigation();
        let newer = terminal
            .navigate_command_from_view(CommandNavigationAction::Next, 5)
            .unwrap();
        assert!(older.display_offset > 5);
        assert!(newer.display_offset < 5);
        terminal.reset_command_navigation();
        let anchor = terminal.snapshot().rows().last().unwrap().line_id.unwrap();
        terminal.advance(b"\r\noutput\r\n");
        terminal.record_fallback_command_at(anchor);
        terminal.record_fallback_command_at(anchor);
        assert_eq!(
            terminal
                .primary_lines
                .metadata
                .values()
                .filter(|m| m.command_anchor)
                .count(),
            6
        );
        terminal.clear_except_input();
        terminal.record_fallback_command_at(TerminalLineId {
            epoch: u64::MAX,
            logical_line: anchor.logical_line,
        });
        assert_eq!(terminal.command_navigation_target(), None);
    }
    #[test]
    fn captured_fallback_and_late_osc_boundary_merge_the_same_prompt_anchor() {
        use crate::command_navigation::command_submission_evidence;
        use nyaterm_core::terminal::input_tracker::TerminalInputState;
        for marked_input in [false, true] {
            let mut terminal = TerminalCore::new(40, 4);
            terminal.advance(b"\x1b]133;A\x07$ ");
            if marked_input {
                terminal.advance(b"\x1b]133;B\x07");
            }
            terminal.advance(b"echo test");
            let evidence = command_submission_evidence(
                &terminal.snapshot(),
                &TerminalInputState::new(),
                b"\r",
            )
            .unwrap();
            terminal.advance(b"\r\n\x1b]133;C\x07output\r\n\x1b]133;D;0\x07");
            terminal.record_fallback_command_at(evidence.anchor);
            assert_eq!(
                terminal
                    .primary_lines
                    .metadata
                    .values()
                    .filter(|m| m.command_anchor)
                    .count(),
                1
            );
        }
    }

    #[test]
    fn missing_output_boundaries_do_not_reuse_a_completed_prompt() {
        use crate::command_navigation::command_submission_evidence;
        use nyaterm_core::terminal::input_tracker::TerminalInputState;
        for fallback in [false, true] {
            let mut terminal = TerminalCore::new(40, 4);
            terminal.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07echo first");
            let first = command_submission_evidence(
                &terminal.snapshot(),
                &TerminalInputState::new(),
                b"\r",
            )
            .unwrap();
            if fallback {
                terminal.record_fallback_command_at(first.anchor);
            }
            terminal.advance(b"\r\nfirst\r\n\x1b]133;D;0\x07$ echo recalled");
            let next = command_submission_evidence(
                &terminal.snapshot(),
                &TerminalInputState::new(),
                b"\r",
            )
            .unwrap();
            assert_ne!(next.anchor, first.anchor);
            assert_eq!(next.command, "echo recalled");
            terminal.record_fallback_command_at(next.anchor);
            terminal.advance(b"\r\nrecalled\r\n");
            assert_eq!(
                terminal
                    .primary_lines
                    .metadata
                    .values()
                    .filter(|m| m.command_anchor)
                    .count(),
                2
            );
        }
    }

    #[test]
    fn resize_history_cropping_keeps_only_the_surviving_command_points() {
        let mut terminal = TerminalCore::new(16, 3);
        terminal.set_scrollback_limit(8);
        for _ in 0..5 {
            emit_command(&mut terminal);
        }
        terminal.resize(4, 4);
        let anchors = terminal
            .primary_lines
            .metadata
            .iter()
            .filter_map(|(line, meta)| meta.command_anchor.then_some(*line))
            .collect::<Vec<_>>();
        assert!(!anchors.is_empty());
        assert!(anchors.len() < 5);
        for expected in anchors.iter().rev() {
            assert_eq!(
                terminal
                    .navigate_command(CommandNavigationAction::Previous)
                    .unwrap()
                    .target_line
                    .unwrap()
                    .logical_line,
                *expected
            );
        }
        assert!(
            terminal
                .navigate_command(CommandNavigationAction::Previous)
                .is_none()
        );
    }
}
