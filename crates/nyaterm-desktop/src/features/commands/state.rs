//! Authoritative command catalog, history, runtime and quick-command UI state.

use std::{cell::RefCell, rc::Rc, sync::Arc};

use futures::channel::mpsc::UnboundedReceiver;
use gpui::{FocusHandle, UniformListScrollHandle};
use nyaterm_core::{
    CommandHistoryEntry, QuickCommand, QuickCommandCategory, QuickCommandCategoryPosition,
    QuickCommandRelativePosition, QuickCommandsConfig, quick_command_category_move_neighbor,
};
use nyaterm_store::StoreBlockingClient;
use nyaterm_ui::{ChildWindowSlot, NyaWindowHandle};

use crate::blocking_jobs::BlockingJobScheduler;
use crate::features::{
    runtime_jobs::CommandPersistenceRequest, runtime_jobs::CommandPersistenceResult,
};
use crate::models::{
    QuickCommandCategoryCreateState, QuickCommandCategoryDeleteState,
    QuickCommandCategoryRenameState, QuickCommandDetailsState, QuickCommandEditorField,
    QuickCommandEditorState, QuickCommandImportPathPromptKind, QuickCommandSortMode,
    QuickCommandVariablePromptState, QuickCommandViewMode,
};

use super::runtime_state::CommandRuntimeState;
use super::staged::StagedQuickCommands;

pub(in crate::features) struct CommandFeatureState {
    catalog: CommandCatalogState,
    quick: QuickCommandFeatureState,
    history: Arc<[CommandHistoryEntry]>,
    runtime: CommandRuntimeState,
    pub(super) staged: StagedQuickCommands,
}

pub(in crate::features) struct CommandFeatureInit {
    pub commands: Vec<QuickCommand>,
    pub categories: Vec<QuickCommandCategory>,
    pub history: Vec<CommandHistoryEntry>,
    pub sort_mode: QuickCommandSortMode,
    pub view_mode: QuickCommandViewMode,
    pub selected_category: String,
    pub focus: QuickCommandFeatureFocus,
    pub store: StoreBlockingClient,
    pub scheduler: BlockingJobScheduler,
}

struct CommandCatalogState {
    commands: Arc<[QuickCommand]>,
    categories: Vec<QuickCommandCategory>,
}

fn valid_quick_category(category_id: &str, categories: &[QuickCommandCategory]) -> String {
    if category_id == "all"
        || category_id == "uncategorized"
        || categories.iter().any(|category| category.id == category_id)
    {
        category_id.to_string()
    } else {
        "all".to_string()
    }
}

struct QuickCommandFeatureState {
    list: QuickCommandListState,
    editor: QuickCommandEditorFeatureState,
    dialogs: QuickCommandDialogState,
    import: QuickCommandImportState,
    ai: QuickCommandAiState,
}

impl CommandFeatureState {
    pub(in crate::features) fn new(init: CommandFeatureInit) -> Self {
        let catalog = CommandCatalogState::new(init.commands, init.categories);
        let selected_category = valid_quick_category(&init.selected_category, &catalog.categories);
        Self {
            catalog,
            quick: QuickCommandFeatureState::new(
                init.sort_mode,
                init.view_mode,
                selected_category,
                init.focus,
            ),
            history: Arc::from(init.history),
            runtime: CommandRuntimeState::new(init.store, init.scheduler),
            staged: StagedQuickCommands::default(),
        }
    }

    pub(in crate::features) fn cancel_staged_quick_command(&mut self, session_id: &str) {
        self.staged.cancel(session_id);
    }

    pub(in crate::features) fn cancel_staged_sync_receiver(&mut self, session_id: &str) {
        self.staged.cancel_receiver(session_id);
    }

    pub(in crate::features) fn owns_staged_quick_command(&self, session_id: &str, id: u64) -> bool {
        self.staged.owns(session_id, id)
    }

    pub(in crate::features) fn replace_loaded(
        &mut self,
        commands: Vec<QuickCommand>,
        categories: Vec<QuickCommandCategory>,
        history: Vec<CommandHistoryEntry>,
    ) {
        self.catalog.replace(commands, categories);
        self.history = Arc::from(history);
        self.reconcile_quick_category();
    }

    pub(in crate::features) fn quick_commands(&self) -> &[QuickCommand] {
        &self.catalog.commands
    }

    pub(in crate::features) fn quick_commands_snapshot(&self) -> Arc<[QuickCommand]> {
        self.catalog.commands.clone()
    }

    pub(in crate::features) fn quick_command_categories(&self) -> &[QuickCommandCategory] {
        &self.catalog.categories
    }

    pub(in crate::features) fn command_history(&self) -> &[CommandHistoryEntry] {
        &self.history
    }

    pub(in crate::features) fn command_history_snapshot(&self) -> Arc<[CommandHistoryEntry]> {
        self.history.clone()
    }

    pub(in crate::features) fn replace_quick_command_catalog(
        &mut self,
        commands: Vec<QuickCommand>,
        categories: Vec<QuickCommandCategory>,
    ) {
        self.catalog.replace(commands, categories);
        self.reconcile_quick_category();
    }

    pub(in crate::features) fn quick_command_config(&self) -> QuickCommandsConfig {
        QuickCommandsConfig {
            commands: self.catalog.commands.to_vec(),
            categories: self.catalog.categories.clone(),
        }
    }

    pub(in crate::features) fn reorder_quick_command(
        &mut self,
        source_id: &str,
        target_id: &str,
        after: bool,
    ) -> Option<QuickCommandsConfig> {
        let mut config = self.quick_command_config();
        if !config.reorder_command_relative(
            source_id,
            target_id,
            if after {
                QuickCommandRelativePosition::After
            } else {
                QuickCommandRelativePosition::Before
            },
        ) {
            return None;
        }
        self.catalog
            .replace(config.commands.clone(), config.categories.clone());
        Some(config)
    }

    pub(in crate::features) fn move_quick_command_to_category(
        &mut self,
        source_id: &str,
        category_id: Option<String>,
    ) -> Option<QuickCommandsConfig> {
        let mut config = self.quick_command_config();
        if !config.move_command_to_category(source_id, category_id) {
            return None;
        }
        self.catalog
            .replace(config.commands.clone(), config.categories.clone());
        Some(config)
    }

    pub(in crate::features) fn move_quick_category(
        &mut self,
        source_id: &str,
        target_id: &str,
        position: QuickCommandDropPosition,
    ) -> Option<QuickCommandsConfig> {
        let mut config = self.quick_command_config();
        let position = match position {
            QuickCommandDropPosition::Before => QuickCommandCategoryPosition::Before,
            QuickCommandDropPosition::After => QuickCommandCategoryPosition::After,
            QuickCommandDropPosition::Inside => QuickCommandCategoryPosition::Inside,
        };
        if !config.move_category(source_id, target_id, position) {
            return None;
        }
        self.catalog
            .replace(config.commands.clone(), config.categories.clone());
        Some(config)
    }

    pub(in crate::features) fn can_move_quick_category(
        &self,
        source_id: &str,
        target_id: &str,
    ) -> bool {
        QuickCommandsConfig {
            commands: Vec::new(),
            categories: self.catalog.categories.clone(),
        }
        .can_move_category(source_id, target_id)
    }

    /// The sibling this category would swap with, or `None` at either end of its
    /// run. The group menu uses it both to enable the item and to perform the move.
    pub(in crate::features) fn quick_category_move_neighbor(
        &self,
        category_id: &str,
        up: bool,
    ) -> Option<String> {
        quick_command_category_move_neighbor(&self.catalog.categories, category_id, up)
    }

    pub(in crate::features) fn move_quick_category_to_root(
        &mut self,
        category_id: &str,
    ) -> Option<QuickCommandsConfig> {
        let mut config = self.quick_command_config();
        if !config.move_category_to_parent(category_id, None, usize::MAX) {
            return None;
        }
        self.catalog
            .replace(config.commands.clone(), config.categories.clone());
        Some(config)
    }

    pub(in crate::features) fn outdent_quick_category(
        &mut self,
        category_id: &str,
    ) -> Option<QuickCommandsConfig> {
        let mut config = self.quick_command_config();
        if !config.outdent_category(category_id) {
            return None;
        }
        self.catalog
            .replace(config.commands.clone(), config.categories.clone());
        Some(config)
    }

    pub(in crate::features) fn can_outdent_quick_category(&self, category_id: &str) -> bool {
        self.catalog
            .categories
            .iter()
            .find(|item| item.id == category_id)
            .and_then(|item| item.parent_id.as_deref())
            .is_some_and(|parent| self.catalog.categories.iter().any(|item| item.id == parent))
    }

    /// Moves a category one slot within its own siblings. Reuses `move_category`,
    /// so reparenting rules, cycle rejection and `sort_order` densification stay in
    /// one place.
    pub(in crate::features) fn move_quick_category_by_one(
        &mut self,
        category_id: &str,
        up: bool,
    ) -> Option<QuickCommandsConfig> {
        let target = self.quick_category_move_neighbor(category_id, up)?;
        let position = if up {
            QuickCommandCategoryPosition::Before
        } else {
            QuickCommandCategoryPosition::After
        };
        let mut config = self.quick_command_config();
        if !config.move_category(category_id, &target, position) {
            return None;
        }
        self.catalog
            .replace(config.commands.clone(), config.categories.clone());
        Some(config)
    }

    pub(in crate::features) fn replace_command_history(
        &mut self,
        history: Vec<CommandHistoryEntry>,
    ) {
        self.history = Arc::from(history);
    }

    pub(in crate::features) fn queue_command_history(&mut self, commands: Vec<String>) -> bool {
        self.runtime
            .queue(CommandPersistenceRequest::AppendHistory(commands))
    }

    pub(in crate::features) fn queue_quick_command_use_count(
        &mut self,
        command_id: String,
    ) -> bool {
        if !self
            .runtime
            .queue(CommandPersistenceRequest::IncrementQuickCommand(
                command_id.clone(),
            ))
        {
            return false;
        }
        self.catalog.increment_use_count(&command_id);
        true
    }

    pub(in crate::features) fn take_persistence_event_receiver(
        &mut self,
    ) -> Option<UnboundedReceiver<CommandPersistenceResult>> {
        self.runtime.take_event_receiver()
    }

    pub(in crate::features) fn note_persistence_event_delivered(&mut self) {
        self.runtime.note_event_delivered();
    }

    pub(in crate::features) fn note_persistence_worker_disconnected(&mut self) -> bool {
        self.runtime.note_worker_disconnected()
    }

    pub(in crate::features) fn apply_persistence_result(
        &mut self,
        event: CommandPersistenceResult,
    ) -> Result<(), String> {
        match event {
            CommandPersistenceResult::History(Ok(history)) => {
                self.replace_command_history(history);
                Ok(())
            }
            CommandPersistenceResult::History(Err(error)) => {
                Err(format!("command history save failed: {error}"))
            }
            CommandPersistenceResult::QuickCommandUseCount { command_id, result } => result
                .map_err(|error| {
                    self.catalog.rollback_use_count(&command_id);
                    format!("quick command use count update failed: {error}")
                }),
        }
    }

    pub(in crate::features) fn quick_search_draft(&self) -> &str {
        &self.quick.list.search_draft
    }

    pub(in crate::features) fn quick_row_scroll(&self) -> &UniformListScrollHandle {
        &self.quick.list.row_scroll
    }

    pub(in crate::features) fn quick_selected_category(&self) -> &str {
        &self.quick.list.selected_category
    }

    pub(in crate::features) fn quick_command_creation_category(&self) -> Option<String> {
        let selected = self.quick_selected_category();
        if matches!(selected, "all" | "uncategorized") {
            return None;
        }
        self.catalog
            .categories
            .iter()
            .find(|category| category.id == selected)
            .map(|category| category.id.clone())
    }

    pub(in crate::features) fn quick_sort_mode(&self) -> QuickCommandSortMode {
        self.quick.list.sort_mode
    }

    pub(in crate::features) fn quick_view_mode(&self) -> QuickCommandViewMode {
        self.quick.list.view_mode
    }

    pub(in crate::features) fn quick_tooltip_control(&self) -> QuickCommandTooltipControl {
        self.quick.list.tooltip.clone()
    }

    pub(in crate::features) fn set_quick_search_draft(&mut self, text: String) {
        self.quick.list.search_draft = text;
    }

    pub(in crate::features) fn clear_quick_filters(&mut self) {
        self.quick.list.search_draft.clear();
        self.quick.list.selected_category = "all".to_string();
    }

    pub(in crate::features) fn select_quick_category(&mut self, category_id: String) {
        self.quick.list.selected_category =
            valid_quick_category(&category_id, &self.catalog.categories);
    }

    fn reconcile_quick_category(&mut self) {
        self.quick.list.selected_category =
            valid_quick_category(&self.quick.list.selected_category, &self.catalog.categories);
    }

    pub(in crate::features) fn set_quick_view_mode(&mut self, mode: QuickCommandViewMode) {
        self.close_quick_toolbar_popovers();
        self.quick.list.view_mode = mode;
    }

    pub(in crate::features) fn set_quick_sort_mode(&mut self, mode: QuickCommandSortMode) {
        self.close_quick_toolbar_popovers();
        self.quick.list.sort_mode = mode;
    }

    pub(in crate::features) fn quick_drop_target(&self) -> Option<&QuickCommandDropTarget> {
        self.quick.list.drop_target.as_ref()
    }

    pub(in crate::features) fn set_quick_drop_target(
        &mut self,
        target: QuickCommandDropTarget,
    ) -> bool {
        if self.quick.list.drop_target.as_ref() == Some(&target) {
            return false;
        }
        self.quick.list.drop_target = Some(target);
        true
    }

    pub(in crate::features) fn clear_quick_drop_target(&mut self) -> bool {
        self.quick.list.drop_target.take().is_some()
    }

    pub(in crate::features) fn close_quick_toolbar_popovers(&mut self) -> bool {
        let changed = self.quick.ai.popover_open;
        self.quick.close_toolbar_popovers();
        changed
    }

    pub(in crate::features) fn quick_editor(&self) -> Option<&QuickCommandEditorState> {
        self.quick.editor.draft.as_ref()
    }

    pub(in crate::features) fn quick_editor_snapshot(&self) -> Option<QuickCommandEditorState> {
        self.quick.editor.draft.clone()
    }

    pub(in crate::features) fn quick_editor_focus(&self) -> &FocusHandle {
        &self.quick.editor.focus
    }

    pub(in crate::features) fn open_quick_editor(&mut self, editor: QuickCommandEditorState) {
        self.quick.editor.draft = Some(editor);
        self.quick.editor.save_in_flight = None;
        self.quick.editor.category_search_draft.clear();
        self.quick.editor.new_category_draft.clear();
        self.quick.editor.category_picker_open = false;
        self.quick.editor.icon_picker_open = false;
    }

    /// Claim the editor's single save slot; `None` while a save is already running.
    pub(in crate::features) fn begin_quick_editor_save(&mut self) -> Option<u64> {
        let editor = &mut self.quick.editor;
        if editor.draft.is_none() || editor.save_in_flight.is_some() {
            return None;
        }
        editor.save_generation = editor.save_generation.wrapping_add(1);
        editor.save_in_flight = Some(editor.save_generation);
        Some(editor.save_generation)
    }

    /// Release the save slot; false when the reply belongs to a superseded draft.
    pub(in crate::features) fn finish_quick_editor_save(&mut self, generation: u64) -> bool {
        if self.quick.editor.save_in_flight != Some(generation) {
            return false;
        }
        self.quick.editor.save_in_flight = None;
        true
    }

    pub(in crate::features) fn quick_editor_save_in_flight(&self) -> bool {
        self.quick.editor.save_in_flight.is_some()
    }

    pub(in crate::features) fn close_quick_editor(&mut self) {
        self.quick.editor.draft = None;
        self.quick.editor.save_in_flight = None;
        self.quick.editor.category_search_draft.clear();
        self.quick.editor.new_category_draft.clear();
        self.quick.editor.window.clear();
        self.quick.editor.category_picker_open = false;
        self.quick.editor.icon_picker_open = false;
    }

    pub(in crate::features) fn quick_editor_category_picker_is_open(&self) -> bool {
        self.quick.editor.category_picker_open
    }

    pub(in crate::features) fn quick_editor_category_search_draft(&self) -> &str {
        &self.quick.editor.category_search_draft
    }

    pub(in crate::features) fn apply_quick_editor_category_search(&mut self, text: String) -> bool {
        if self.quick.editor.category_search_draft == text {
            return false;
        }
        self.quick.editor.category_search_draft = text;
        true
    }

    pub(in crate::features) fn quick_editor_new_category_draft(&self) -> &str {
        &self.quick.editor.new_category_draft
    }

    pub(in crate::features) fn apply_quick_editor_new_category(&mut self, text: String) -> bool {
        if self.quick.editor.new_category_draft == text {
            return false;
        }
        self.quick.editor.new_category_draft = text;
        true
    }

    pub(in crate::features) fn commit_quick_editor_new_category(&mut self) -> bool {
        let draft = self.quick.editor.new_category_draft.trim().to_string();
        if draft.is_empty() {
            return false;
        }
        let Some(editor) = self.quick.editor.draft.as_mut() else {
            return false;
        };
        editor.category_id = None;
        editor.category_draft = draft;
        editor.error = None;
        editor.error_field = None;
        self.quick.editor.category_search_draft.clear();
        self.quick.editor.new_category_draft.clear();
        self.quick.editor.category_picker_open = false;
        true
    }

    pub(in crate::features) fn set_quick_editor_category_picker_open(
        &mut self,
        open: bool,
    ) -> bool {
        if self.quick.editor.category_picker_open == open {
            return false;
        }
        self.quick.editor.category_picker_open = open;
        if !open {
            self.quick.editor.category_search_draft.clear();
            self.quick.editor.new_category_draft.clear();
        }
        if open {
            self.quick.editor.icon_picker_open = false;
        }
        true
    }

    pub(in crate::features) fn quick_editor_icon_picker_is_open(&self) -> bool {
        self.quick.editor.icon_picker_open
    }

    pub(in crate::features) fn set_quick_editor_icon_picker_open(&mut self, open: bool) -> bool {
        if self.quick.editor.icon_picker_open == open {
            return false;
        }
        self.quick.editor.icon_picker_open = open;
        if open {
            self.quick.editor.category_picker_open = false;
        }
        true
    }

    pub(in crate::features) fn apply_quick_editor_input(
        &mut self,
        field: QuickCommandEditorField,
        text: String,
    ) -> bool {
        if field == QuickCommandEditorField::Category {
            let changed = self.apply_quick_editor_category_search(text);
            if changed && let Some(editor) = self.quick.editor.draft.as_mut() {
                editor.focused_field = field;
                editor.error = None;
                editor.error_field = None;
            }
            return changed;
        }
        let Some(editor) = self.quick.editor.draft.as_mut() else {
            return false;
        };
        editor.focused_field = field;
        match field {
            QuickCommandEditorField::Label => editor.label = text,
            QuickCommandEditorField::Command => editor.command = text,
            QuickCommandEditorField::Description => editor.description = text,
            QuickCommandEditorField::Category => unreachable!(),
        }
        editor.error = None;
        editor.error_field = None;
        true
    }

    pub(in crate::features) fn set_quick_editor_category(
        &mut self,
        category_id: Option<String>,
        category_draft: String,
    ) -> bool {
        let Some(editor) = self.quick.editor.draft.as_mut() else {
            return false;
        };
        editor.category_id = category_id;
        editor.category_draft = category_draft;
        editor.error = None;
        editor.error_field = None;
        self.quick.editor.category_search_draft.clear();
        self.quick.editor.new_category_draft.clear();
        self.quick.editor.category_picker_open = false;
        true
    }

    pub(in crate::features) fn set_quick_editor_color(
        &mut self,
        color_tag: Option<String>,
    ) -> bool {
        let Some(editor) = self.quick.editor.draft.as_mut() else {
            return false;
        };
        editor.color_tag = color_tag;
        editor.icon_tag = None;
        editor.error = None;
        editor.error_field = None;
        self.quick.editor.icon_picker_open = false;
        true
    }

    pub(in crate::features) fn set_quick_editor_icon(&mut self, icon_tag: Option<String>) -> bool {
        let Some(editor) = self.quick.editor.draft.as_mut() else {
            return false;
        };
        editor.icon_tag = icon_tag;
        if editor.icon_tag.is_some() {
            editor.color_tag = None;
        }
        editor.error = None;
        editor.error_field = None;
        self.quick.editor.icon_picker_open = false;
        true
    }

    pub(in crate::features) fn toggle_quick_editor_pinned(&mut self) -> bool {
        let Some(editor) = self.quick.editor.draft.as_mut() else {
            return false;
        };
        editor.pinned = !editor.pinned;
        editor.error = None;
        editor.error_field = None;
        true
    }

    pub(in crate::features) fn toggle_quick_editor_raw_send(&mut self) -> bool {
        let Some(editor) = self.quick.editor.draft.as_mut() else {
            return false;
        };
        editor.raw_send = !editor.raw_send;
        true
    }

    pub(in crate::features) fn set_quick_editor_execution_mode(&mut self, mode: &str) -> bool {
        let Some(editor) = self.quick.editor.draft.as_mut() else {
            return false;
        };
        editor.execution_mode = if mode == "append" {
            "append"
        } else {
            "execute"
        }
        .to_string();
        editor.error = None;
        editor.error_field = None;
        true
    }

    pub(in crate::features) fn set_quick_editor_error(
        &mut self,
        error: String,
        field: Option<QuickCommandEditorField>,
    ) {
        if let Some(editor) = self.quick.editor.draft.as_mut() {
            editor.error = Some(error);
            editor.error_field = field;
            if let Some(field) = field {
                editor.focused_field = field;
            }
        }
    }

    pub(in crate::features) fn quick_editor_window(&self) -> Option<NyaWindowHandle> {
        self.quick.editor.window.handle()
    }

    pub(in crate::features) fn quick_editor_window_is_pending(&self) -> bool {
        self.quick.editor.window.is_pending()
    }

    pub(in crate::features) fn quick_editor_window_is_open_or_pending(&self) -> bool {
        self.quick.editor.window.is_open_or_pending()
    }

    pub(in crate::features) fn quick_editor_is_inline(&self) -> bool {
        self.quick.editor.draft.is_some() && !self.quick.editor.window.is_open_or_pending()
    }

    pub(in crate::features) fn quick_editor_window_slot(&mut self) -> &mut ChildWindowSlot {
        &mut self.quick.editor.window
    }

    /// Claim the right to open the editor window.
    ///
    /// Also refuses when there is no draft to show, so the caller can fall back
    /// to focusing the inline editor.
    pub(in crate::features) fn request_quick_editor_window(&mut self) -> bool {
        if self.quick.editor.draft.is_none() {
            return false;
        }
        self.quick.editor.window.begin_open()
    }

    pub(in crate::features::commands) fn finish_quick_editor_window_open(
        &mut self,
        window: Option<NyaWindowHandle>,
    ) {
        match window {
            Some(window) => self.quick.editor.window.finish_open(window),
            None => self.quick.editor.window.fail_open(),
        }
    }

    pub(in crate::features) fn cancel_quick_editor_window_request(&mut self) {
        self.quick.editor.window.cancel_open();
    }

    pub(in crate::features) fn quick_details(&self) -> Option<&QuickCommandDetailsState> {
        self.quick.dialogs.details.as_ref()
    }

    pub(in crate::features) fn quick_details_focus(&self) -> &FocusHandle {
        &self.quick.dialogs.details_focus
    }

    pub(in crate::features) fn request_quick_details(&mut self, state: QuickCommandDetailsState) {
        self.quick.dialogs.details = Some(state);
    }

    pub(in crate::features) fn clear_quick_details(&mut self) {
        self.quick.dialogs.details = None;
    }

    pub(in crate::features) fn quick_category_delete(
        &self,
    ) -> Option<&QuickCommandCategoryDeleteState> {
        self.quick.dialogs.category_delete.as_ref()
    }

    pub(in crate::features) fn request_quick_category_delete(
        &mut self,
        state: QuickCommandCategoryDeleteState,
    ) {
        self.quick.dialogs.category_delete = Some(state);
    }

    pub(in crate::features) fn clear_quick_category_delete(&mut self) {
        self.quick.dialogs.category_delete = None;
    }

    pub(in crate::features) fn finish_quick_category_delete(&mut self, category_id: &str) {
        self.quick.dialogs.category_delete = None;
        if self.quick.list.selected_category == category_id {
            self.quick.list.selected_category = "all".to_string();
        }
        if let Some(editor) = self.quick.editor.draft.as_mut()
            && editor.category_id.as_deref() == Some(category_id)
        {
            editor.category_id = None;
            editor.category_draft.clear();
        }
    }

    pub(in crate::features) fn quick_category_create(
        &self,
    ) -> Option<&QuickCommandCategoryCreateState> {
        self.quick.dialogs.category_create.as_ref()
    }

    pub(in crate::features) fn request_quick_category_create(
        &mut self,
        state: QuickCommandCategoryCreateState,
    ) {
        self.quick.dialogs.category_create = Some(state);
    }

    pub(in crate::features) fn clear_quick_category_create(&mut self) {
        self.quick.dialogs.category_create = None;
    }

    pub(in crate::features) fn apply_quick_category_create(&mut self, text: String) -> bool {
        let Some(create) = self.quick.dialogs.category_create.as_mut() else {
            return false;
        };
        create.draft = text;
        create.error = None;
        true
    }

    pub(in crate::features) fn set_quick_category_create_error(&mut self, error: String) {
        if let Some(create) = self.quick.dialogs.category_create.as_mut() {
            create.error = Some(error);
        }
    }

    pub(in crate::features) fn quick_category_rename(
        &self,
    ) -> Option<&QuickCommandCategoryRenameState> {
        self.quick.dialogs.category_rename.as_ref()
    }

    pub(in crate::features) fn request_quick_category_rename(
        &mut self,
        state: QuickCommandCategoryRenameState,
    ) {
        self.quick.dialogs.category_rename = Some(state);
    }

    pub(in crate::features) fn clear_quick_category_rename(&mut self) {
        self.quick.dialogs.category_rename = None;
    }

    pub(in crate::features) fn apply_quick_category_rename(&mut self, text: String) -> bool {
        let Some(rename) = self.quick.dialogs.category_rename.as_mut() else {
            return false;
        };
        rename.draft = text;
        rename.error = None;
        true
    }

    pub(in crate::features) fn set_quick_category_rename_error(&mut self, error: String) {
        if let Some(rename) = self.quick.dialogs.category_rename.as_mut() {
            rename.error = Some(error);
        }
    }

    pub(in crate::features) fn quick_variable_prompt(
        &self,
    ) -> Option<&QuickCommandVariablePromptState> {
        self.quick.dialogs.variable_prompt.as_ref()
    }

    pub(in crate::features) fn quick_variable_focus(&self) -> &FocusHandle {
        &self.quick.dialogs.variable_focus
    }

    pub(in crate::features) fn request_quick_variable_prompt(
        &mut self,
        prompt: QuickCommandVariablePromptState,
    ) {
        self.quick.dialogs.variable_prompt = Some(prompt);
    }

    pub(in crate::features) fn take_quick_variable_prompt(
        &mut self,
    ) -> Option<QuickCommandVariablePromptState> {
        self.quick.dialogs.variable_prompt.take()
    }

    pub(in crate::features) fn clear_quick_variable_prompt(&mut self) {
        self.quick.dialogs.variable_prompt = None;
    }

    pub(in crate::features) fn set_quick_variable_value(
        &mut self,
        index: usize,
        value: String,
    ) -> bool {
        let Some(prompt) = self.quick.dialogs.variable_prompt.as_mut() else {
            return false;
        };
        let Some(variable) = prompt.variables.get(index) else {
            return false;
        };
        let name = variable.name.clone();
        for variable in &mut prompt.variables {
            if variable.name == name {
                variable.value = value.clone();
            }
        }
        true
    }

    pub(in crate::features) fn quick_import_path_prompt(
        &self,
    ) -> Option<QuickCommandImportPathPromptKind> {
        self.quick.import.path_prompt
    }

    pub(in crate::features) fn request_quick_import_path(
        &mut self,
        kind: QuickCommandImportPathPromptKind,
    ) -> bool {
        if self.quick.import.path_prompt.is_some() {
            return false;
        }
        self.quick.import.path_prompt = Some(kind);
        true
    }

    pub(in crate::features) fn finish_quick_import_path(&mut self) {
        self.quick.import.path_prompt = None;
    }

    pub(in crate::features) fn quick_ai_popover_is_open(&self) -> bool {
        self.quick.ai.popover_open
    }

    pub(in crate::features) fn quick_ai_prompt_draft(&self) -> &str {
        &self.quick.ai.prompt_draft
    }

    pub(in crate::features) fn toggle_quick_ai_popover(&mut self) -> bool {
        let open = !self.quick.ai.popover_open;
        self.close_quick_toolbar_popovers();
        self.quick.ai.popover_open = open;
        open
    }

    pub(in crate::features) fn close_quick_ai_popover(&mut self) {
        self.quick.ai.popover_open = false;
    }

    pub(in crate::features) fn set_quick_ai_prompt_draft(&mut self, text: String) {
        self.quick.ai.prompt_draft = text;
    }

    pub(in crate::features) fn take_quick_ai_prompt(&mut self) -> Option<String> {
        let prompt = self.quick.ai.prompt_draft.trim().to_string();
        if prompt.is_empty() {
            return None;
        }
        self.quick.ai.prompt_draft.clear();
        Some(prompt)
    }
}

impl CommandCatalogState {
    fn new(commands: Vec<QuickCommand>, categories: Vec<QuickCommandCategory>) -> Self {
        Self {
            commands: Arc::from(commands),
            categories,
        }
    }

    pub(in crate::features) fn replace(
        &mut self,
        commands: Vec<QuickCommand>,
        categories: Vec<QuickCommandCategory>,
    ) {
        self.commands = Arc::from(commands);
        self.categories = categories;
    }

    fn increment_use_count(&mut self, command_id: &str) {
        if let Some(command) = Arc::make_mut(&mut self.commands)
            .iter_mut()
            .find(|command| command.id == command_id)
        {
            command.use_count = Some(command.use_count.unwrap_or_default().saturating_add(1));
        }
    }

    fn rollback_use_count(&mut self, command_id: &str) {
        if let Some(command) = Arc::make_mut(&mut self.commands)
            .iter_mut()
            .find(|command| command.id == command_id)
        {
            command.use_count = Some(command.use_count.unwrap_or_default().saturating_sub(1));
        }
    }
}

/// Focus handles the quick command state needs at construction time.
pub(in crate::features) struct QuickCommandFeatureFocus {
    pub editor: FocusHandle,
    pub details: FocusHandle,
    pub variable: FocusHandle,
}

type QuickCommandTooltipDismiss = Rc<dyn Fn(&mut gpui::App)>;

#[derive(Clone, Default)]
pub(in crate::features) struct QuickCommandTooltipControl {
    inner: Rc<RefCell<QuickCommandTooltipControlInner>>,
}

#[derive(Default)]
struct QuickCommandTooltipControlInner {
    suppressed_command_id: Option<String>,
    active: Option<(String, QuickCommandTooltipDismiss)>,
}

impl QuickCommandTooltipControl {
    pub(in crate::features) fn begin_hover(&self, command_id: &str) {
        let mut inner = self.inner.borrow_mut();
        if inner.suppressed_command_id.as_deref() == Some(command_id) {
            inner.suppressed_command_id = None;
        }
        if inner
            .active
            .as_ref()
            .is_some_and(|(active_id, _)| active_id != command_id)
        {
            inner.active = None;
        }
    }

    pub(in crate::features) fn is_suppressed(&self, command_id: &str) -> bool {
        self.inner.borrow().suppressed_command_id.as_deref() == Some(command_id)
    }

    pub(in crate::features) fn register(
        &self,
        command_id: String,
        dismiss: QuickCommandTooltipDismiss,
    ) {
        self.inner.borrow_mut().active = Some((command_id, dismiss));
    }

    pub(in crate::features) fn dismiss(&self, command_id: &str, cx: &mut gpui::App) {
        let dismiss = {
            let mut inner = self.inner.borrow_mut();
            inner.suppressed_command_id = Some(command_id.to_string());
            inner.active.take().map(|(_, dismiss)| dismiss)
        };
        if let Some(dismiss) = dismiss {
            dismiss(cx);
        }
    }
}

/// Panel list state: search, category filter, sort/view mode and their menus.
struct QuickCommandListState {
    search_draft: String,
    selected_category: String,
    sort_mode: QuickCommandSortMode,
    view_mode: QuickCommandViewMode,
    drop_target: Option<QuickCommandDropTarget>,
    tooltip: QuickCommandTooltipControl,
    /// Owned by the panel list so the row scrollbar and the virtualized list
    /// share one scroll position across re-renders.
    row_scroll: UniformListScrollHandle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::features) enum QuickCommandDropPosition {
    Before,
    After,
    Inside,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::features) struct QuickCommandDropTarget {
    pub id: String,
    pub position: QuickCommandDropPosition,
}

/// Quick command editor draft and its optional detached window.
struct QuickCommandEditorFeatureState {
    draft: Option<QuickCommandEditorState>,
    focus: FocusHandle,
    window: ChildWindowSlot,
    category_picker_open: bool,
    icon_picker_open: bool,
    category_search_draft: String,
    new_category_draft: String,
    save_generation: u64,
    /// Generation of the in-flight save; cleared when the draft is replaced or
    /// closed, so a late store reply cannot close or fail a different draft.
    save_in_flight: Option<u64>,
}

/// Delete/details/create/rename confirmations and the variable prompt.
struct QuickCommandDialogState {
    details: Option<QuickCommandDetailsState>,
    details_focus: FocusHandle,
    category_delete: Option<QuickCommandCategoryDeleteState>,
    category_create: Option<QuickCommandCategoryCreateState>,
    category_rename: Option<QuickCommandCategoryRenameState>,
    variable_prompt: Option<QuickCommandVariablePromptState>,
    variable_focus: FocusHandle,
}

/// Import source picker and its path prompt.
struct QuickCommandImportState {
    path_prompt: Option<QuickCommandImportPathPromptKind>,
}

/// AI-assisted quick command popover.
struct QuickCommandAiState {
    popover_open: bool,
    prompt_draft: String,
}

impl QuickCommandFeatureState {
    pub(in crate::features) fn new(
        sort_mode: QuickCommandSortMode,
        view_mode: QuickCommandViewMode,
        selected_category: String,
        focus: QuickCommandFeatureFocus,
    ) -> Self {
        Self {
            list: QuickCommandListState {
                search_draft: String::new(),
                selected_category,
                sort_mode,
                view_mode,
                drop_target: None,
                tooltip: QuickCommandTooltipControl::default(),
                row_scroll: UniformListScrollHandle::new(),
            },
            editor: QuickCommandEditorFeatureState {
                draft: None,
                focus: focus.editor,
                window: ChildWindowSlot::default(),
                category_picker_open: false,
                icon_picker_open: false,
                category_search_draft: String::new(),
                new_category_draft: String::new(),
                save_generation: 0,
                save_in_flight: None,
            },
            dialogs: QuickCommandDialogState {
                details: None,
                details_focus: focus.details,
                category_delete: None,
                category_create: None,
                category_rename: None,
                variable_prompt: None,
                variable_focus: focus.variable,
            },
            import: QuickCommandImportState { path_prompt: None },
            ai: QuickCommandAiState {
                popover_open: false,
                prompt_draft: String::new(),
            },
        }
    }
}

impl QuickCommandFeatureState {
    /// Closes the AI toolbar popover before another toolbar action takes over.
    pub(in crate::features) fn close_toolbar_popovers(&mut self) {
        self.ai.popover_open = false;
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, path::Path, rc::Rc, sync::Arc};

    use gpui::TestAppContext;
    use nyaterm_core::{QuickCommand, QuickCommandCategory};

    use crate::blocking_jobs::BlockingJobScheduler;
    use crate::models::{
        QuickCommandEditorState, QuickCommandImportPathPromptKind, QuickCommandSortMode,
        QuickCommandVariableDef, QuickCommandVariablePromptState, QuickCommandViewMode,
    };
    use crate::test_support::{TestConfigDir, blocking_test_store};

    use super::{
        CommandCatalogState, CommandFeatureInit, CommandFeatureState, QuickCommandFeatureFocus,
        QuickCommandTooltipControl,
    };

    fn command(id: &str) -> QuickCommand {
        QuickCommand {
            id: id.to_string(),
            label: id.to_string(),
            command: "pwd".to_string(),
            category_id: Some("category-1".to_string()),
            description: None,
            color_tag: None,
            icon_tag: None,
            pinned: None,
            execution_mode: None,
            raw_send: None,
            source: None,
            risk_level: None,
            updated_at: None,
            created_at: None,
            use_count: None,
            sort_order: None,
        }
    }

    fn command_state(root: &Path) -> CommandFeatureState {
        command_state_with_category(root, "all", Vec::new())
    }

    fn command_state_with_category(
        root: &Path,
        selected_category: &str,
        categories: Vec<QuickCommandCategory>,
    ) -> CommandFeatureState {
        let cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let focus = || cx.update(|cx| cx.focus_handle());
        let store = blocking_test_store(root);
        CommandFeatureState::new(CommandFeatureInit {
            commands: Vec::new(),
            categories,
            history: Vec::new(),
            sort_mode: QuickCommandSortMode::Usage,
            view_mode: QuickCommandViewMode::List,
            selected_category: selected_category.to_string(),
            focus: QuickCommandFeatureFocus {
                editor: focus(),
                details: focus(),
                variable: focus(),
            },
            store,
            scheduler: BlockingJobScheduler::new(),
        })
    }

    fn category(id: &str, parent: Option<&str>, order: i32) -> QuickCommandCategory {
        QuickCommandCategory {
            id: id.to_string(),
            name: id.to_string(),
            parent_id: parent.map(ToString::to_string),
            sort_order: order,
        }
    }

    #[test]
    fn command_creation_inherits_only_an_existing_selected_category() {
        let dir = TestConfigDir::new("nyaterm-command-creation-category");
        let mut state = command_state_with_category(
            dir.path(),
            "category-1",
            vec![category("category-1", None, 0)],
        );
        assert_eq!(
            state.quick_command_creation_category().as_deref(),
            Some("category-1")
        );
        for selected in ["all", "uncategorized", "removed-category"] {
            state.select_quick_category(selected.to_string());
            assert_eq!(state.quick_command_creation_category(), None);
        }
        state.select_quick_category("category-1".to_string());
        state.replace_quick_command_catalog(Vec::new(), Vec::new());
        assert_eq!(state.quick_command_creation_category(), None);
    }

    #[test]
    fn selected_category_restores_only_when_it_exists_or_is_uncategorized() {
        let valid_dir = TestConfigDir::new("nyaterm-command-category-restore");
        let uncategorized_dir = TestConfigDir::new("nyaterm-command-category-restore");
        let stale_dir = TestConfigDir::new("nyaterm-command-category-restore");
        let categories = vec![category("category-1", None, 0)];
        let state = command_state_with_category(valid_dir.path(), "category-1", categories.clone());
        assert_eq!(state.quick_selected_category(), "category-1");
        let state = command_state_with_category(
            uncategorized_dir.path(),
            "uncategorized",
            categories.clone(),
        );
        assert_eq!(state.quick_selected_category(), "uncategorized");
        let mut state =
            command_state_with_category(stale_dir.path(), "removed-category", categories);
        assert_eq!(state.quick_selected_category(), "all");
        state.select_quick_category("category-1".to_string());
        state.replace_quick_command_catalog(Vec::new(), Vec::new());
        assert_eq!(state.quick_selected_category(), "all");
    }

    #[test]
    fn quick_tooltip_control_dismisses_and_resets_on_the_next_hover() {
        let cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let control = QuickCommandTooltipControl::default();
        let dismiss_count = Rc::new(Cell::new(0));
        let dismiss_count_for_handler = dismiss_count.clone();
        control.register(
            "command-1".to_string(),
            Rc::new(move |_| dismiss_count_for_handler.set(dismiss_count_for_handler.get() + 1)),
        );

        cx.update(|cx| control.dismiss("command-1", cx));

        assert_eq!(dismiss_count.get(), 1);
        assert!(control.is_suppressed("command-1"));
        control.begin_hover("command-1");
        assert!(!control.is_suppressed("command-1"));
    }

    #[test]
    fn quick_category_move_neighbor_is_none_at_each_end() {
        let test_dir = TestConfigDir::new("nyaterm-command-state-test");
        let mut state = command_state(test_dir.path());
        state.replace_quick_command_catalog(
            Vec::new(),
            vec![
                category("first", None, 0),
                category("middle", None, 1),
                category("last", None, 2),
            ],
        );

        // These drive `.disabled(...)` on the group menu's move items.
        assert!(state.quick_category_move_neighbor("first", true).is_none());
        assert!(state.quick_category_move_neighbor("last", false).is_none());
        assert_eq!(
            state
                .quick_category_move_neighbor("middle", true)
                .as_deref(),
            Some("first")
        );
        assert_eq!(
            state
                .quick_category_move_neighbor("middle", false)
                .as_deref(),
            Some("last")
        );
    }

    #[test]
    fn move_quick_category_by_one_reorders_and_stops_at_the_ends() {
        let test_dir = TestConfigDir::new("nyaterm-command-state-test");
        let mut state = command_state(test_dir.path());
        state.replace_quick_command_catalog(
            Vec::new(),
            vec![
                category("first", None, 0),
                category("middle", None, 1),
                category("last", None, 2),
            ],
        );

        assert!(state.move_quick_category_by_one("last", true).is_some());
        let order = |state: &CommandFeatureState| {
            nyaterm_core::quick_command_category_sibling_order(
                state.quick_command_categories(),
                None,
            )
            .into_iter()
            .map(|item| item.id.clone())
            .collect::<Vec<_>>()
        };
        assert_eq!(order(&state), vec!["first", "last", "middle"]);

        // The catalog is the authority, so the refusal has to be observable there too.
        assert!(state.move_quick_category_by_one("first", true).is_none());
        assert_eq!(order(&state), vec!["first", "last", "middle"]);
        assert!(state.move_quick_category_by_one("middle", false).is_none());
        assert_eq!(order(&state), vec!["first", "last", "middle"]);
    }

    #[test]
    fn quick_ai_popover_toggles_and_closes() {
        let test_dir = TestConfigDir::new("nyaterm-command-state-test");
        let mut state = command_state(test_dir.path());

        state.toggle_quick_ai_popover();
        assert!(state.quick_ai_popover_is_open());
        assert!(state.close_quick_toolbar_popovers());
        assert!(!state.quick_ai_popover_is_open());
        assert!(!state.close_quick_toolbar_popovers());
    }

    #[test]
    fn quick_editor_literal_mode_defaults_off_and_restores_the_saved_setting() {
        let test_dir = TestConfigDir::new("nyaterm-command-state-test");
        let mut state = command_state(test_dir.path());
        assert!(!state.toggle_quick_editor_raw_send());
        state.open_quick_editor(QuickCommandEditorState::blank());
        assert!(!state.quick_editor().unwrap().raw_send);
        assert!(state.toggle_quick_editor_raw_send());
        assert!(state.quick_editor().unwrap().raw_send);
        assert!(state.toggle_quick_editor_raw_send());
        assert!(!state.quick_editor().unwrap().raw_send);

        let mut saved = command("literal");
        saved.raw_send = Some(true);
        saved.command = "  {{变量}}\\p  ".into();
        state.open_quick_editor(QuickCommandEditorState::from_command(saved.clone()));
        assert!(state.quick_editor().unwrap().raw_send);
        state.toggle_quick_editor_raw_send();
        assert_eq!(state.quick_editor().unwrap().command, saved.command);
    }

    #[test]
    fn quick_editor_picker_state_is_exclusive_and_resets_on_close() {
        let test_dir = TestConfigDir::new("nyaterm-command-state-test");
        let mut state = command_state(test_dir.path());
        state.open_quick_editor(QuickCommandEditorState::blank());

        assert!(state.set_quick_editor_category_picker_open(true));
        assert!(state.quick_editor_category_picker_is_open());
        assert!(!state.quick_editor_icon_picker_is_open());

        assert!(state.set_quick_editor_icon_picker_open(true));
        assert!(!state.quick_editor_category_picker_is_open());
        assert!(state.quick_editor_icon_picker_is_open());

        assert!(state.set_quick_editor_icon(Some("docker".to_string())));
        assert!(!state.quick_editor_icon_picker_is_open());

        assert!(state.set_quick_editor_icon_picker_open(true));
        assert!(state.set_quick_editor_color(Some("blue".to_string())));
        assert!(!state.quick_editor_icon_picker_is_open());

        assert!(state.set_quick_editor_category_picker_open(true));
        assert!(state.set_quick_editor_category(Some("category-1".to_string()), String::new()));
        assert!(!state.quick_editor_category_picker_is_open());

        state.set_quick_editor_icon_picker_open(true);
        state.close_quick_editor();
        assert!(!state.quick_editor_category_picker_is_open());
        assert!(!state.quick_editor_icon_picker_is_open());
    }

    #[test]
    fn quick_editor_preserves_terminal_selection_exactly() {
        let selected = "  printf 'a\\nb'  \n".to_string();
        let editor = QuickCommandEditorState::from_terminal_selection(selected.clone());
        assert_eq!(editor.command, selected);
        assert_eq!(
            editor.focused_field,
            crate::models::QuickCommandEditorField::Command
        );
    }

    #[test]
    fn quick_editor_category_search_and_new_category_drafts_are_separate() {
        let test_dir = TestConfigDir::new("nyaterm-command-state-test");
        let mut state = command_state(test_dir.path());
        state.open_quick_editor(QuickCommandEditorState::blank());

        assert!(state.apply_quick_editor_category_search("linux".to_string()));
        assert_eq!(state.quick_editor_category_search_draft(), "linux");
        assert_eq!(state.quick_editor().unwrap().category_draft, "");

        assert!(state.apply_quick_editor_new_category("Operations".to_string()));
        assert!(state.commit_quick_editor_new_category());
        assert_eq!(state.quick_editor().unwrap().category_draft, "Operations");
        assert_eq!(state.quick_editor_category_search_draft(), "");
        assert_eq!(state.quick_editor_new_category_draft(), "");
    }

    #[test]
    fn category_deletion_clears_filter_and_matching_editor_category() {
        let test_dir = TestConfigDir::new("nyaterm-command-state-test");
        let mut state = command_state(test_dir.path());
        state.replace_quick_command_catalog(Vec::new(), vec![category("category-1", None, 0)]);
        state.open_quick_editor(QuickCommandEditorState::blank());
        assert!(state.set_quick_editor_category(Some("category-1".to_string()), String::new(),));
        state.select_quick_category("category-1".to_string());
        state.request_quick_category_delete(crate::models::QuickCommandCategoryDeleteState {
            id: "category-1".to_string(),
            name: "Common".to_string(),
        });

        state.finish_quick_category_delete("category-1");

        assert_eq!(state.quick_selected_category(), "all");
        assert_eq!(
            state
                .quick_editor()
                .and_then(|editor| editor.category_id.as_deref()),
            None
        );
        assert!(state.quick_category_delete().is_none());
    }

    #[test]
    fn variable_values_are_synchronized_by_name_at_the_owner_boundary() {
        let test_dir = TestConfigDir::new("nyaterm-command-state-test");
        let mut state = command_state(test_dir.path());
        state.request_quick_variable_prompt(QuickCommandVariablePromptState {
            command_id: "command-1".to_string(),
            label: "Command".to_string(),
            command: "{{host}} {{host}}".to_string(),
            execute: true,
            send_to_all: false,
            variables: vec![
                QuickCommandVariableDef {
                    raw: "{{host}}".to_string(),
                    name: "host".to_string(),
                    options: Vec::new(),
                    value: String::new(),
                },
                QuickCommandVariableDef {
                    raw: "{{host}}".to_string(),
                    name: "host".to_string(),
                    options: Vec::new(),
                    value: String::new(),
                },
            ],
        });

        assert!(state.set_quick_variable_value(1, "prod".to_string()));
        let prompt = state
            .quick_variable_prompt()
            .expect("prompt should remain open");
        assert_eq!(prompt.variables[0].value, "prod");
        assert_eq!(prompt.variables[1].value, "prod");
    }

    #[test]
    fn import_and_detached_editor_lifecycles_clear_pending_state_atomically() {
        let test_dir = TestConfigDir::new("nyaterm-command-state-test");
        let mut state = command_state(test_dir.path());
        assert!(state.request_quick_import_path(QuickCommandImportPathPromptKind::NyatermJson));
        assert!(!state.request_quick_import_path(QuickCommandImportPathPromptKind::XshellXts));
        state.finish_quick_import_path();
        assert!(state.request_quick_import_path(QuickCommandImportPathPromptKind::XshellXts));

        state.open_quick_editor(QuickCommandEditorState::blank());
        assert!(state.request_quick_editor_window());
        assert!(state.quick_editor_window_is_pending());
        // A second request must not start a second window.
        assert!(!state.request_quick_editor_window());
        state.close_quick_editor();
        assert!(state.quick_editor().is_none());
        assert!(state.quick_editor_window().is_none());
        assert!(!state.quick_editor_window_is_pending());
    }

    #[test]
    fn command_catalog_replaces_and_clears_commands_with_categories() {
        let mut catalog = CommandCatalogState::new(Vec::new(), Vec::new());
        catalog.replace(
            vec![command("command-1")],
            vec![QuickCommandCategory {
                id: "category-1".to_string(),
                name: "Common".to_string(),
                parent_id: None,
                sort_order: 0,
            }],
        );

        assert_eq!(catalog.commands.len(), 1);
        assert_eq!(catalog.categories.len(), 1);

        catalog.commands = Arc::default();
        catalog.categories.clear();
        assert!(catalog.commands.is_empty());
        assert!(catalog.categories.is_empty());
    }

    #[test]
    fn command_catalog_use_count_increment_can_be_rolled_back() {
        let mut catalog = CommandCatalogState::new(vec![command("command-1")], Vec::new());

        catalog.increment_use_count("command-1");
        assert_eq!(catalog.commands[0].use_count, Some(1));

        catalog.rollback_use_count("command-1");
        assert_eq!(catalog.commands[0].use_count, Some(0));
    }
}
