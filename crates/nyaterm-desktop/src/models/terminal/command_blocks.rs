use nyaterm_terminal::command_blocks::{CommandBlock, CommandBlockId};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TerminalCommandBlockState {
    pub selected: Option<CommandBlockId>,
    pub collapsed: BTreeSet<CommandBlockId>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum CommandBlockAction {
    Select(CommandBlockId),
    Toggle(CommandBlockId),
}

impl TerminalCommandBlockState {
    pub fn retain(&mut self, blocks: &[CommandBlock]) {
        let retained =
            |id: &CommandBlockId| blocks.binary_search_by_key(id, |block| block.id).is_ok();
        self.collapsed.retain(retained);
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
