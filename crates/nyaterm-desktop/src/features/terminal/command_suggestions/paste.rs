//! Immutable input checkpoint shared by suggestion fill and paste review.

use gpui::Context;
use nyaterm_core::completion::types::CompletionScope;
use nyaterm_core::terminal::editing::EditPhase;
use nyaterm_terminal::TerminalSnapshot;
use nyaterm_terminal::editing::resolve_input;
use std::sync::Arc;

use crate::features::NyaTermApp;
use crate::models::CommandSuggestionState;

pub(in crate::features::terminal) struct SuggestionPasteContext {
    pub(in crate::features::terminal) session_id: String,
    input_version: u64,
    sync_revision: u64,
    completion_revision: u64,
    scope: Option<CompletionScope>,
}

impl SuggestionPasteContext {
    pub(super) fn from_popup(popup: &CommandSuggestionState) -> Self {
        Self {
            session_id: popup.session_id.clone(),
            input_version: popup.input_version,
            sync_revision: popup.sync_revision,
            completion_revision: popup.completion_revision,
            scope: popup.scope.clone(),
        }
    }
}

impl NyaTermApp {
    pub(in crate::features::terminal) fn validate_command_suggestion_input(
        &mut self,
        context: &SuggestionPasteContext,
        cx: &mut Context<Self>,
    ) -> Result<Arc<TerminalSnapshot>, &'static str> {
        if context.scope.as_ref().is_some_and(|scope| {
            *scope != self.command_suggestion_context(&context.session_id).scope
                || self.commands.completion_runtime().revision() != context.completion_revision
        }) {
            return Err("Completion context changed; select a current suggestion");
        }
        if !self.command_suggestions_allowed_for_session(&context.session_id)
            || self.sync_input.revision() != context.sync_revision
        {
            return Err("Cannot safely fill this suggestion in the current terminal state");
        }
        // Reconcile against the latest live frame; a typing prediction or an old
        // matching search pattern is never sufficient evidence for a write.
        self.reconcile_shell_editing(&context.session_id, cx);
        let snapshot = self.terminal_snapshot_for_session(Some(&context.session_id), 0);
        let Some(state) = self.terminal.editing.state() else {
            return Err("Wait for the terminal to confirm the input");
        };
        if state.awaiting_snapshot
            || state.model.phase != EditPhase::Ready
            || state.model.version != context.input_version
            || state.mapping.is_none()
            || resolve_input(
                &snapshot,
                &state.model.input.value,
                state.model.input.cursor,
            )
            .as_ref()
                != state.mapping.as_ref()
        {
            return Err("Input changed or is not confirmed; select the suggestion again");
        }
        Ok(snapshot)
    }
}
