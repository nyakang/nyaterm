use nyaterm_terminal::command_blocks::{CommandBlock, CommandBlockId};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TerminalCommandBlockState {
    pub selected: Option<CommandBlockId>,
    pub collapsed: BTreeSet<CommandBlockId>,
    /// Validation tags for requested folds, owned by this presentation state.
    collapsed_revisions: BTreeMap<CommandBlockId, u64>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum CommandBlockAction {
    Select(CommandBlockId),
    Toggle(CommandBlockId),
}

/// Late worker frames may still carry folds after the user disables the rail.
/// Keep their physical rows and no-wrap index, but remove the obsolete projection.
pub(crate) fn expanded_command_snapshot(
    mut snapshot: std::sync::Arc<nyaterm_terminal::TerminalSnapshot>,
) -> std::sync::Arc<nyaterm_terminal::TerminalSnapshot> {
    if snapshot
        .display_rows
        .as_ref()
        .is_some_and(|index| index.has_folds())
    {
        std::sync::Arc::make_mut(&mut snapshot).display_rows = None;
    }
    snapshot
}

impl TerminalCommandBlockState {
    pub fn expand_all(&mut self) -> bool {
        let changed = !self.collapsed.is_empty();
        self.collapsed.clear();
        self.collapsed_revisions.clear();
        changed
    }

    pub fn retain(&mut self, blocks: &[CommandBlock]) {
        let retained =
            |id: &CommandBlockId| blocks.binary_search_by_key(id, |block| block.id).is_ok();
        self.collapsed.retain(|id| {
            blocks
                .binary_search_by_key(id, |block| block.id)
                .ok()
                .map(|index| &blocks[index])
                .is_some_and(|block| {
                    block.complete
                        && !block.output.is_empty()
                        && self.collapsed_revisions.get(id).copied().unwrap_or(0)
                            == block.collapse_revision
                })
        });
        self.collapsed_revisions
            .retain(|id, _| self.collapsed.contains(id));
        if self.selected.is_some_and(|id| !retained(&id)) {
            self.selected = None;
        }
    }

    pub fn apply(
        &mut self,
        action: CommandBlockAction,
        blocks: &[CommandBlock],
        cursor: usize,
    ) -> bool {
        let (CommandBlockAction::Select(id) | CommandBlockAction::Toggle(id)) = action;
        let Some(block) = blocks
            .binary_search_by_key(&id, |block| block.id)
            .ok()
            .map(|index| &blocks[index])
        else {
            return false;
        };
        self.selected = Some(id);
        if matches!(action, CommandBlockAction::Toggle(_)) && block.can_collapse(cursor) {
            if !self.collapsed.remove(&id) {
                self.collapsed.insert(id);
                self.collapsed_revisions.insert(id, block.collapse_revision);
            } else {
                self.collapsed_revisions.remove(&id);
            }
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandBlockAction, TerminalCommandBlockState};
    use nyaterm_terminal::TerminalScreen;

    #[test]
    fn revoked_and_reclosed_regions_discard_stale_fold_requests() {
        use nyaterm_core::terminal::input_tracker::TerminalInputState;
        use nyaterm_terminal::command_navigation::command_submission_evidence;
        let mut screen = TerminalScreen::new(80, 8);
        screen.advance(b"$ echo hi");
        let evidence =
            command_submission_evidence(&screen.snapshot(), &TerminalInputState::new(), b"\r")
                .unwrap();
        screen.note_command_submission(evidence);
        screen.advance(b"\r\nhi\r\n$ ");
        let blocks = screen.snapshot().command_blocks;
        let id = blocks[0].id;
        let mut state = TerminalCommandBlockState::default();
        assert!(state.apply(CommandBlockAction::Toggle(id), &blocks, 2));
        // Coalescing may hide the intermediate reopened frame from desktop.
        screen.advance(b"\r\nlate output\r\n$ ");
        state.retain(&screen.snapshot().command_blocks);
        assert!(!state.collapsed.contains(&id));
        assert_eq!(state.selected, Some(id));
        assert!(state.apply(
            CommandBlockAction::Toggle(id),
            &screen.snapshot().command_blocks,
            4
        ));
        state.retain(&screen.snapshot().command_blocks);
        assert!(state.collapsed.contains(&id));
    }

    #[test]
    fn expanding_late_command_snapshots_preserves_text_and_no_wrap_metadata() {
        use super::expanded_command_snapshot;
        use std::sync::Arc;
        for no_wrap in [false, true] {
            let mut screen = TerminalScreen::new(20, 8);
            screen.set_logical_line_snapshots(no_wrap);
            screen.advance(b"$ \x1b]133;B\x07run\r\n\x1b]133;C\x07one\r\ntwo\r\n\x1b]133;D;0\x07\x1b]133;A\x07$ ");
            let snapshot = screen.snapshot();
            let id = snapshot.command_blocks[0].id;
            screen.set_collapsed_commands([id].into());
            let folded = Arc::new(screen.viewport_snapshot(0));
            assert!(folded.display_rows.as_ref().unwrap().has_folds());
            let expanded = expanded_command_snapshot(folded.clone());
            assert!(expanded.display_rows.is_none());
            assert_eq!(expanded.rows(), folded.rows());
            assert_eq!(expanded.logical_lines.is_some(), no_wrap);
            assert!(Arc::ptr_eq(
                &expanded,
                &expanded_command_snapshot(expanded.clone())
            ));
            assert!(folded.display_rows.as_ref().unwrap().has_folds());
        }
    }

    #[test]
    fn selection_is_exclusive_and_running_blocks_do_not_collapse() {
        let mut screen = TerminalScreen::new(20, 8);
        screen.advance(b"$ \x1b]133;B\x07first\r\n\x1b]133;C\x07out\r\n\x1b]133;D;0\x07\x1b]133;A\x07$ \x1b]133;B\x07second");
        let snapshot = screen.snapshot();
        let first = snapshot.command_blocks[0].id;
        let second = snapshot.command_blocks[1].id;
        let mut state = TerminalCommandBlockState::default();
        assert!(state.apply(
            CommandBlockAction::Toggle(first),
            &snapshot.command_blocks,
            2
        ));
        assert_eq!(state.selected, Some(first));
        assert!(state.collapsed.contains(&first));
        assert!(!state.apply(
            CommandBlockAction::Toggle(second),
            &snapshot.command_blocks,
            2
        ));
        assert_eq!(state.selected, Some(second));
        assert!(!state.collapsed.contains(&second));
        assert!(state.apply(
            CommandBlockAction::Toggle(first),
            &snapshot.command_blocks,
            2
        ));
        assert!(state.collapsed.is_empty());
        state.retain(&[]);
        assert_eq!(state, TerminalCommandBlockState::default());
    }
}
