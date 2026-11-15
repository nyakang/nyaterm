//! Sole owner of command completion presentation and search lifecycle state.
use super::path_source::PathCompletionRuntime;
use gpui::{ScrollHandle, Task, point, px};
use nyaterm_core::command_suggestion_suppression::CommandSuggestionSuppression;
use nyaterm_core::completion::types::{CompletionFillPolicy, CompletionMode};

pub(in crate::features::terminal) struct CommandSuggestionFeatureState {
    pub(in crate::features::terminal) path_completion: PathCompletionRuntime,
    pub(in crate::features::terminal) popup: Option<CommandSuggestionState>,
    pub(in crate::features::terminal) scroll: ScrollHandle,
    pub(in crate::features::terminal) mode: CompletionMode,
    pub(in crate::features::terminal) suppression: CommandSuggestionSuppression,
    pub(in crate::features::terminal) pending_history_entry: Option<String>,
    pub(in crate::features::terminal) search_generation: u64,
    pub(in crate::features::terminal) refresh_task: Option<Task<()>>,
}

impl Default for CommandSuggestionFeatureState {
    fn default() -> Self {
        Self {
            path_completion: Default::default(),
            popup: None,
            scroll: ScrollHandle::default(),
            mode: CompletionMode::Token,
            suppression: CommandSuggestionSuppression::None,
            pending_history_entry: None,
            search_generation: 0,
            refresh_task: None,
        }
    }
}

impl CommandSuggestionFeatureState {
    /// Visibility-only changes must not cancel a pending search or reset its mode.
    pub(in crate::features::terminal) fn hide(&mut self) -> bool {
        self.popup.take().is_some()
    }

    pub(in crate::features::terminal) fn invalidate_search(&mut self) -> u64 {
        self.search_generation = self.search_generation.saturating_add(1);
        self.search_generation
    }

    pub(in crate::features::terminal) fn dismiss(&mut self) -> bool {
        self.mode = CompletionMode::Token;
        self.invalidate_search();
        self.refresh_task = None;
        self.hide()
    }

    /// Tracking resets preserve the current search generation, task and mode.
    pub(in crate::features::terminal) fn clear_tracking(&mut self) {
        self.hide();
        self.suppression = CommandSuggestionSuppression::None;
        self.pending_history_entry = None;
    }

    /// Session switches invalidate callbacks without dropping their task handle.
    pub(in crate::features::terminal) fn reset_for_session_switch(&mut self) {
        self.clear_tracking();
        self.mode = CompletionMode::Token;
        self.invalidate_search();
    }

    pub(in crate::features::terminal) fn take_pending_history_entry(&mut self) -> Option<String> {
        self.pending_history_entry.take()
    }

    pub(super) fn begin_input_refresh(&mut self, text: &str) -> bool {
        self.popup
            .as_mut()
            .is_some_and(|popup| popup.begin_input_refresh(text))
    }

    pub(super) fn clear_selection(&mut self) -> bool {
        self.popup
            .as_mut()
            .is_some_and(|popup| popup.selected_index.take().is_some())
    }

    pub(super) fn select_index(&mut self, index: usize) -> bool {
        let Some(popup) = self.popup.as_mut() else {
            return false;
        };
        let changed = popup.selected_index != Some(index);
        popup.selected_index = Some(index);
        changed
    }

    pub(super) fn move_selection(&mut self, direction: i32) -> bool {
        let Some(popup) = self.popup.as_mut() else {
            return false;
        };
        let selected =
            command_suggestion_step_selection(popup.selected_index, popup.items.len(), direction);
        let changed = selected != popup.selected_index;
        popup.selected_index = selected;
        if let Some(index) = selected {
            self.scroll.scroll_to_item(index);
        }
        changed
    }

    pub(super) fn replace_results(
        &mut self,
        mut next: CommandSuggestionState,
        incremental: bool,
    ) -> bool {
        if incremental {
            preserve_suggestion_selection(self.popup.as_ref(), &mut next);
        }
        if !command_suggestion_state_changed(self.popup.as_ref(), &next) {
            return false;
        }
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.popup = Some(next);
        true
    }

    pub(super) fn narrow_draft(&mut self, draft: String, version: u64) -> bool {
        let Some(popup) = self.popup.as_mut() else {
            return false;
        };
        let selected = popup.selected_command();
        popup.items.retain(|item| item.command.starts_with(&draft));
        popup.draft = draft;
        popup.input_version = version;
        popup.restore_selection(selected);
        if popup.items.is_empty() {
            self.hide();
        }
        true
    }

    pub(super) fn retain_fillable(&mut self, policy: CompletionFillPolicy) -> bool {
        let Some(popup) = self.popup.as_mut() else {
            return false;
        };
        let selected = popup.selected_command();
        let old_len = popup.items.len();
        popup
            .items
            .retain(|item| item.fillable && policy.allows(&popup.draft, &item.command));
        if popup.items.len() == old_len {
            return false;
        }
        popup.restore_selection(selected);
        if popup.items.is_empty() {
            self.hide();
        }
        true
    }
}
#[derive(Debug, Clone, PartialEq)]
pub(in crate::features::terminal) struct CommandSuggestionItem {
    pub(in crate::features::terminal) fillable: bool,
    pub(in crate::features::terminal) description: String,
    pub(in crate::features::terminal) command: String,
    pub(in crate::features::terminal) display: String,
    pub(in crate::features::terminal) source: String,
    pub(in crate::features::terminal) score: u32,
    pub(in crate::features::terminal) indices: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::features::terminal) struct CommandSuggestionState {
    /// Previous results may stay visible during typing, but cannot be accepted.
    pub(in crate::features::terminal) refreshing: bool,
    pub(in crate::features::terminal) mode: nyaterm_core::completion::types::CompletionMode,
    pub(in crate::features::terminal) completion_revision: u64,
    pub(in crate::features::terminal) scope:
        Option<nyaterm_core::completion::types::CompletionScope>,
    pub(in crate::features::terminal) session_id: String,
    pub(in crate::features::terminal) input_version: u64,
    pub(in crate::features::terminal) sync_revision: u64,
    pub(in crate::features::terminal) draft: String,
    pub(in crate::features::terminal) items: Vec<CommandSuggestionItem>,
    pub(in crate::features::terminal) selected_index: Option<usize>,
}

impl CommandSuggestionState {
    fn selected_command(&self) -> Option<String> {
        command_suggestion_item_for_selection(&self.items, self.selected_index)
            .map(|item| item.command.clone())
    }

    fn restore_selection(&mut self, command: Option<String>) {
        self.selected_index =
            command.and_then(|command| self.items.iter().position(|item| item.command == command));
    }

    pub(in crate::features::terminal) fn begin_input_refresh(&mut self, text: &str) -> bool {
        let ordinary_edit = (!text.is_empty() && text.chars().all(|ch| !ch.is_control()))
            || matches!(text, "\u{7f}" | "\u{8}" | "\u{1b}[3~");
        let changed =
            self.refreshing != ordinary_edit || (ordinary_edit && self.selected_index.is_some());
        self.refreshing = ordinary_edit;
        if ordinary_edit {
            self.selected_index = None;
        }
        changed
    }
}

pub(super) fn preserve_suggestion_selection(
    previous: Option<&CommandSuggestionState>,
    next: &mut CommandSuggestionState,
) {
    if let Some(previous) = previous
        && previous.session_id == next.session_id
        && previous.mode == next.mode
        && previous.input_version == next.input_version
        && previous.draft == next.draft
    {
        next.selected_index = previous
            .selected_index
            .and_then(|i| previous.items.get(i))
            .and_then(|selected| {
                next.items
                    .iter()
                    .position(|item| item.command == selected.command)
            });
    }
}

pub(super) fn command_suggestion_state_changed(
    current: Option<&CommandSuggestionState>,
    next: &CommandSuggestionState,
) -> bool {
    current != Some(next)
}

pub(super) fn command_suggestion_item_for_selection(
    items: &[CommandSuggestionItem],
    selected_index: Option<usize>,
) -> Option<&CommandSuggestionItem> {
    items.get(selected_index?)
}

pub(super) fn command_suggestion_step_selection(
    current: Option<usize>,
    len: usize,
    direction: i32,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    if direction > 0 {
        return match current {
            None => Some(0),
            Some(index) if index + 1 < len => Some(index + 1),
            Some(_) => None,
        };
    }
    if direction < 0 {
        return match current {
            None => Some(len - 1),
            Some(0) => None,
            Some(index) => Some(index.min(len - 1).saturating_sub(1)),
        };
    }
    command_suggestion_clamp_selection(current, len)
}

pub(super) fn command_suggestion_clamp_selection(
    current: Option<usize>,
    len: usize,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    current.map(|index| index.min(len - 1))
}

#[cfg(test)]
mod tests {
    use super::{
        CommandSuggestionFeatureState, CommandSuggestionItem, CommandSuggestionState,
        command_suggestion_clamp_selection, command_suggestion_item_for_selection,
        command_suggestion_state_changed, command_suggestion_step_selection,
        preserve_suggestion_selection,
    };
    use nyaterm_core::command_suggestion_suppression::CommandSuggestionSuppression;
    use nyaterm_core::completion::types::CompletionMode;
    fn open_suggestions_popup(session_id: &str) -> CommandSuggestionState {
        CommandSuggestionState {
            refreshing: false,
            mode: nyaterm_core::completion::types::CompletionMode::History,
            completion_revision: 0,
            scope: None,
            session_id: session_id.to_string(),
            input_version: 0,
            sync_revision: 0,
            draft: "1".to_string(),
            items: vec![CommandSuggestionItem {
                fillable: true,
                description: String::new(),
                command: "1111".to_string(),
                display: "1111".to_string(),
                source: "history".to_string(),
                score: 1,
                indices: Vec::new(),
            }],
            selected_index: Some(0),
        }
    }
    #[test]
    fn command_suggestion_state_change_skips_identical_overlay() {
        let state = CommandSuggestionState {
            refreshing: false,
            mode: nyaterm_core::completion::types::CompletionMode::History,
            completion_revision: 0,
            scope: None,
            session_id: "session".to_string(),
            input_version: 0,
            sync_revision: 0,
            draft: "git".to_string(),
            items: vec![CommandSuggestionItem {
                fillable: true,
                description: String::new(),
                command: "git status".to_string(),
                display: "git status".to_string(),
                source: "history".to_string(),
                score: 42,
                indices: vec![0, 1, 2],
            }],
            selected_index: None,
        };
        let mut selected = state.clone();
        selected.selected_index = Some(0);

        assert!(!command_suggestion_state_changed(Some(&state), &state));
        assert!(command_suggestion_state_changed(None, &state));
        assert!(command_suggestion_state_changed(Some(&state), &selected));
    }

    #[test]
    fn command_suggestion_step_selection_enters_and_exits_with_down() {
        assert_eq!(command_suggestion_step_selection(None, 3, 1), Some(0));
        assert_eq!(command_suggestion_step_selection(Some(0), 3, 1), Some(1));
        assert_eq!(command_suggestion_step_selection(Some(2), 3, 1), None);
        assert_eq!(command_suggestion_step_selection(None, 0, 1), None);
    }

    #[test]
    fn command_suggestion_step_selection_enters_and_exits_with_up() {
        assert_eq!(command_suggestion_step_selection(None, 3, -1), Some(2));
        assert_eq!(command_suggestion_step_selection(Some(2), 3, -1), Some(1));
        assert_eq!(command_suggestion_step_selection(Some(0), 3, -1), None);
        assert_eq!(command_suggestion_step_selection(None, 0, -1), None);
    }

    #[test]
    fn command_suggestion_clamp_selection_preserves_none_and_clamps_overflow() {
        assert_eq!(command_suggestion_clamp_selection(None, 3), None);
        assert_eq!(command_suggestion_clamp_selection(Some(1), 3), Some(1));
        assert_eq!(command_suggestion_clamp_selection(Some(7), 3), Some(2));
        assert_eq!(command_suggestion_clamp_selection(Some(0), 0), None);
    }

    #[test]
    fn command_suggestion_accept_requires_selected_item() {
        let items = vec![CommandSuggestionItem {
            fillable: true,
            description: String::new(),
            command: "git status".to_string(),
            display: "git status".to_string(),
            source: "history".to_string(),
            score: 42,
            indices: vec![0, 1, 2],
        }];

        assert!(command_suggestion_item_for_selection(&items, None).is_none());
        assert!(command_suggestion_item_for_selection(&items, Some(1)).is_none());
        assert_eq!(
            command_suggestion_item_for_selection(&items, Some(0))
                .map(|item| item.command.as_str()),
            Some("git status")
        );
    }

    #[test]
    fn incremental_selection_follows_the_command_and_never_a_reused_row_index() {
        let previous = open_suggestions_popup("s1");
        let mut next = open_suggestions_popup("s1");
        let mut added = next.items[0].clone();
        added.command = "new-path".into();
        next.items.insert(0, added);
        next.selected_index = None;
        preserve_suggestion_selection(Some(&previous), &mut next);
        assert_eq!(next.selected_index, Some(1));
        next.items.pop();
        preserve_suggestion_selection(Some(&previous), &mut next);
        assert_eq!(next.selected_index, None);
        next.input_version += 1;
        next.items = previous.items.clone();
        preserve_suggestion_selection(Some(&previous), &mut next);
        assert_eq!(next.selected_index, None);
    }

    fn active_feature() -> CommandSuggestionFeatureState {
        CommandSuggestionFeatureState {
            popup: Some(open_suggestions_popup("s1")),
            mode: CompletionMode::History,
            suppression: CommandSuggestionSuppression::CommandRunning,
            pending_history_entry: Some("fixture command".into()),
            search_generation: 7,
            refresh_task: Some(gpui::Task::ready(())),
            ..Default::default()
        }
    }

    #[test]
    fn hiding_preserves_search_mode_task_and_tracking() {
        let mut state = active_feature();
        assert!(state.hide());
        assert!(!state.hide());
        assert_eq!(state.mode, CompletionMode::History);
        assert_eq!(state.search_generation, 7);
        assert!(state.refresh_task.is_some());
        assert_eq!(
            state.suppression,
            CommandSuggestionSuppression::CommandRunning
        );
        assert!(state.pending_history_entry.is_some());
    }

    #[test]
    fn dismissal_resets_search_without_clearing_tracking() {
        let mut state = active_feature();
        assert!(state.dismiss());
        assert_eq!(state.mode, CompletionMode::Token);
        assert_eq!(state.search_generation, 8);
        assert!(state.refresh_task.is_none());
        assert_eq!(
            state.suppression,
            CommandSuggestionSuppression::CommandRunning
        );
        assert!(state.pending_history_entry.is_some());
        assert!(!state.dismiss());
        assert_eq!(state.search_generation, 9);
    }

    #[test]
    fn clearing_tracking_keeps_the_pending_search() {
        let mut state = active_feature();
        state.clear_tracking();
        assert!(state.popup.is_none());
        assert_eq!(state.suppression, CommandSuggestionSuppression::None);
        assert!(state.pending_history_entry.is_none());
        assert_eq!(state.mode, CompletionMode::History);
        assert_eq!(state.search_generation, 7);
        assert!(state.refresh_task.is_some());
    }

    #[test]
    fn session_switch_clears_tracking_and_invalidates_without_dropping_the_task() {
        let mut state = active_feature();
        state.reset_for_session_switch();
        assert!(state.popup.is_none());
        assert_eq!(state.suppression, CommandSuggestionSuppression::None);
        assert!(state.pending_history_entry.is_none());
        assert_eq!(state.mode, CompletionMode::Token);
        assert_eq!(state.search_generation, 8);
        assert!(state.refresh_task.is_some());
    }

    #[test]
    fn taking_history_entry_consumes_it_once() {
        let mut state = active_feature();
        assert_eq!(
            state.take_pending_history_entry().as_deref(),
            Some("fixture command")
        );
        assert_eq!(state.take_pending_history_entry(), None);
        assert!(state.popup.is_some());
    }

    #[test]
    fn ordinary_edits_disable_acceptance_and_clear_selection_until_results_arrive() {
        for input in ["x", "删除", "\u{7f}", "\u{8}", "\u{1b}[3~"] {
            let mut state = active_feature();
            assert!(state.begin_input_refresh(input));
            assert!(state.popup.as_ref().unwrap().refreshing);
            assert_eq!(state.popup.as_ref().unwrap().selected_index, None);
            assert!(!state.begin_input_refresh(input));
        }
        for input in ["", "\r", "\u{1b}[A"] {
            let mut state = active_feature();
            assert!(!state.begin_input_refresh(input));
            assert!(!state.popup.as_ref().unwrap().refreshing);
            assert_eq!(state.popup.as_ref().unwrap().selected_index, Some(0));
        }
    }

    #[test]
    fn replacement_preserves_selection_only_for_incremental_results_with_matching_identity() {
        let mut state = active_feature();
        let mut next = state.popup.clone().unwrap();
        next.selected_index = None;
        assert!(!state.replace_results(next.clone(), true));
        assert!(state.replace_results(next.clone(), false));
        assert_eq!(state.popup.as_ref().unwrap().selected_index, None);
        for mismatch in 0..4 {
            let mut state = active_feature();
            let mut next = next.clone();
            match mismatch {
                0 => next.session_id = "s2".into(),
                1 => next.mode = CompletionMode::Token,
                2 => next.input_version += 1,
                _ => next.draft.push('1'),
            }
            assert!(state.replace_results(next, true));
            assert_eq!(state.popup.as_ref().unwrap().selected_index, None);
        }
    }

    #[test]
    fn draft_and_policy_filtering_keep_selection_by_command_and_hide_an_empty_popup() {
        let mut state = active_feature();
        let mut extra = state.popup.as_ref().unwrap().items[0].clone();
        extra.command = "other".into();
        state.popup.as_mut().unwrap().items.insert(0, extra);
        state.popup.as_mut().unwrap().selected_index = Some(1);
        assert!(state.narrow_draft("11".into(), 4));
        assert_eq!(state.popup.as_ref().unwrap().selected_index, Some(0));
        assert_eq!(state.popup.as_ref().unwrap().input_version, 4);
        assert!(!state.retain_fillable(super::CompletionFillPolicy::AppendOnly));
        state.popup.as_mut().unwrap().items[0].fillable = false;
        assert!(state.retain_fillable(super::CompletionFillPolicy::AppendOnly));
        assert!(state.popup.is_none());
        assert_eq!(state.search_generation, 7);
    }
}
