//! Authoritative inline command and credential assistance state.

use nyaterm_core::CredentialPromptKind;
use nyaterm_core::command_suggestion_suppression::CommandSuggestionSuppression;
use nyaterm_terminal::TerminalLineId;

use super::state::TerminalFeatureState;
use crate::models::{
    CommandSuggestionState, CredentialAutofillMatchPipeline, CredentialAutofillMatchRequestKey,
    CredentialSuggestionState, PendingCredentialAutofill,
};

/// Inline command completion and terminal-output credential prompt assistance.
///
/// These states share the terminal input lifecycle: session switches, terminal
/// mode changes, and settings updates reset them together. The credential
/// matcher remains a background pipeline and never runs in a render path.
pub(super) struct TerminalAssistState {
    pub(super) command_suggestions: Option<CommandSuggestionState>,
    pub(super) command_suggestion_scroll: gpui::ScrollHandle,
    pub(super) command_suggestion_mode: nyaterm_core::completion::types::CompletionMode,
    pub(super) command_suggestion_suppression: CommandSuggestionSuppression,
    pub(super) pending_command_history_entry: Option<String>,
    pub(super) command_suggestion_search_gen: u64,
    pub(super) command_suggestion_refresh_task: Option<gpui::Task<()>>,
    pub(super) credential_suggestions: Option<CredentialSuggestionState>,
    pub(super) credential_autofill_buffer: String,
    pub(super) credential_autofill_recent: Option<(CredentialPromptKind, String)>,
    credential_prompt_instance: Option<(Option<TerminalLineId>, u64)>,
    pub(super) credential_prompt_awaiting_output: bool,
    pub(super) credential_autofill_generation: u64,
    pub(super) credential_autofill_pending: Option<PendingCredentialAutofill>,
    pub(super) credential_autofill_detection_pending: bool,
    pub(super) credential_autofill_next_request_id: u64,
    pub(super) credential_autofill_pending_request: Option<CredentialAutofillMatchRequestKey>,
    pub(super) credential_autofill_match_pipeline: CredentialAutofillMatchPipeline,
    pub(super) credential_autofill_sending: bool,
    pub(super) credential_prompt_input_until_ms: u64,
}

impl TerminalAssistState {
    pub(super) fn new() -> Self {
        Self {
            command_suggestions: None,
            command_suggestion_scroll: gpui::ScrollHandle::default(),
            command_suggestion_mode: nyaterm_core::completion::types::CompletionMode::Token,
            command_suggestion_suppression: CommandSuggestionSuppression::None,
            pending_command_history_entry: None,
            command_suggestion_search_gen: 0,
            command_suggestion_refresh_task: None,
            credential_suggestions: None,
            credential_autofill_buffer: String::new(),
            credential_autofill_recent: None,
            credential_prompt_instance: None,
            credential_prompt_awaiting_output: false,
            credential_autofill_generation: 0,
            credential_autofill_pending: None,
            credential_autofill_detection_pending: false,
            credential_autofill_next_request_id: 0,
            credential_autofill_pending_request: None,
            credential_autofill_match_pipeline: CredentialAutofillMatchPipeline::spawn(),
            credential_autofill_sending: false,
            credential_prompt_input_until_ms: 0,
        }
    }

    fn clear_command_tracking(&mut self) {
        self.command_suggestions = None;
        self.command_suggestion_suppression = CommandSuggestionSuppression::None;
        self.pending_command_history_entry = None;
    }

    fn invalidate_command_suggestion_search(&mut self) {
        self.command_suggestion_mode = nyaterm_core::completion::types::CompletionMode::Token;
        self.command_suggestion_search_gen = self.command_suggestion_search_gen.saturating_add(1);
    }

    fn reset_for_session_switch(&mut self) {
        self.cancel_credential_autofill();
        self.credential_prompt_instance = None;
        self.credential_prompt_awaiting_output = false;
        self.credential_prompt_input_until_ms = 0;
        self.clear_command_tracking();
        self.invalidate_command_suggestion_search();
    }

    pub(super) fn dismiss_credential_suggestions(&mut self) -> bool {
        let had_panel = self.credential_suggestions.take().is_some();
        self.credential_autofill_buffer.clear();
        // Keep this prompt instance suppressed while it remains on screen,
        // including no-echo input. New line evidence or output after submission
        // releases suppression, so an identical retry can offer credentials.
        self.credential_autofill_detection_pending = false;
        self.credential_autofill_pending_request = None;
        had_panel
    }

    pub(super) fn cancel_credential_autofill(&mut self) -> bool {
        let had_panel = self.dismiss_credential_suggestions();
        self.credential_autofill_pending = None;
        self.credential_prompt_input_until_ms = 0;
        self.credential_prompt_awaiting_output = false;
        self.credential_autofill_sending = false;
        self.credential_autofill_recent = None;
        self.credential_autofill_generation = self.credential_autofill_generation.saturating_add(1);
        had_panel
    }

    pub(super) fn credential_fill_is_current(&self, enabled: bool, generation: u64) -> bool {
        enabled && self.credential_autofill_generation == generation
    }

    pub(super) fn observe_credential_prompt_instance(
        &mut self,
        instance: Option<(Option<TerminalLineId>, u64)>,
    ) -> bool {
        if self.credential_prompt_instance == instance {
            return false;
        }
        self.credential_prompt_instance = instance;
        let had_panel = self.dismiss_credential_suggestions();
        self.credential_autofill_recent = None;
        had_panel
    }

    pub(super) fn observe_credential_prompt_output(&mut self) -> bool {
        if !self.credential_prompt_awaiting_output {
            return false;
        }
        self.credential_prompt_awaiting_output = false;
        let had_panel = self.dismiss_credential_suggestions();
        self.credential_autofill_recent = None;
        had_panel
    }

    pub(super) fn remember_credential_prompt(
        &mut self,
        kind: CredentialPromptKind,
        prompt_text: &str,
    ) -> bool {
        let key = (kind, prompt_text.to_string());
        if self.credential_autofill_recent.as_ref() == Some(&key) {
            return false;
        }
        self.credential_autofill_recent = Some(key);
        true
    }

    pub(super) fn credential_prompt_input_mode(&self, now_ms: u64) -> bool {
        self.credential_prompt_input_until_ms > now_ms
    }

    pub(super) fn shutdown_workers(&mut self) {
        self.credential_autofill_match_pipeline.shutdown();
    }
}

impl TerminalFeatureState {
    pub(in crate::features) fn command_suggestions_open(&self) -> bool {
        self.assist.command_suggestions.is_some()
    }

    pub(in crate::features) fn credential_suggestions_open(&self) -> bool {
        self.assist.credential_suggestions.is_some()
    }

    pub(in crate::features) fn take_pending_command_history_entry(&mut self) -> Option<String> {
        self.assist.pending_command_history_entry.take()
    }

    pub(in crate::features) fn clear_command_tracking(&mut self) {
        self.assist.clear_command_tracking();
    }

    pub(in crate::features) fn reset_assist_for_session_switch(&mut self) {
        self.assist.reset_for_session_switch();
    }

    pub(in crate::features) fn clear_active_session_assist(&mut self) {
        self.editing.clear_active();
        self.assist.command_suggestions = None;
        self.assist.credential_suggestions = None;
    }
}

#[cfg(test)]
mod tests {
    use nyaterm_core::CredentialPromptKind;
    use nyaterm_terminal::TerminalLineId;

    use super::TerminalAssistState;
    use crate::models::{CredentialAutofillMatchRequestKey, PendingCredentialAutofill};

    fn instance(line: i64, revision: u64) -> Option<(Option<TerminalLineId>, u64)> {
        Some((
            Some(TerminalLineId {
                epoch: 1,
                logical_line: line,
            }),
            revision,
        ))
    }

    #[test]
    fn cancellation_clears_pending_and_invalidates_password_read_callbacks() {
        let mut state = TerminalAssistState::new();
        state.credential_autofill_pending = Some(PendingCredentialAutofill {
            session_id: "session".into(),
            credential_id: "credential".into(),
            expires_at_ms: u64::MAX,
        });
        state.credential_autofill_pending_request = Some(CredentialAutofillMatchRequestKey {
            session_id: "session".into(),
            request_id: 1,
            prompt_text: "Password:".into(),
        });
        state.credential_autofill_detection_pending = true;
        state.credential_autofill_buffer = "Password:".into();
        state.credential_prompt_input_until_ms = u64::MAX;
        assert!(state.credential_prompt_input_mode(1));
        let generation = state.credential_autofill_generation;
        assert!(state.credential_fill_is_current(true, generation));
        assert!(!state.credential_fill_is_current(false, generation));
        state.cancel_credential_autofill();
        assert!(state.credential_autofill_pending.is_none());
        assert!(state.credential_autofill_pending_request.is_none());
        assert!(!state.credential_autofill_detection_pending);
        assert!(state.credential_autofill_buffer.is_empty());
        assert!(!state.credential_prompt_input_mode(1));
        // Re-enabling cannot authorize a password read started before cancellation.
        assert!(!state.credential_fill_is_current(true, generation));
        assert!(state.credential_fill_is_current(true, state.credential_autofill_generation));
    }

    #[test]
    fn dismissal_suppresses_same_prompt_until_new_line_or_line_revision() {
        let mut state = TerminalAssistState::new();
        let kind = CredentialPromptKind::Password;
        let prompt = "Enter password:";
        state.observe_credential_prompt_instance(instance(3, 1));
        assert!(state.remember_credential_prompt(kind, prompt));
        state.dismiss_credential_suggestions();
        for _ in 0..3 {
            // Output elsewhere does not identify a new prompt on this line.
            state.observe_credential_prompt_output();
            state.observe_credential_prompt_instance(instance(3, 1));
            assert!(!state.remember_credential_prompt(kind, prompt));
        }
        state.observe_credential_prompt_instance(instance(4, 1));
        assert!(state.remember_credential_prompt(kind, prompt));
        state.dismiss_credential_suggestions();
        state.observe_credential_prompt_instance(instance(4, 2));
        assert!(state.remember_credential_prompt(kind, prompt));
    }

    #[test]
    fn identical_prompt_redrawn_after_submission_can_offer_credentials_again() {
        let mut state = TerminalAssistState::new();
        let kind = CredentialPromptKind::Password;
        let prompt = "Enter password:";
        state.observe_credential_prompt_instance(instance(3, 1));
        assert!(state.remember_credential_prompt(kind, prompt));
        state.dismiss_credential_suggestions();
        state.credential_prompt_awaiting_output = true;
        state.observe_credential_prompt_instance(instance(3, 1));
        assert!(!state.remember_credential_prompt(kind, prompt));
        // A program can reuse the same line with identical final contents.
        state.observe_credential_prompt_output();
        state.observe_credential_prompt_instance(instance(3, 1));
        assert!(state.remember_credential_prompt(kind, prompt));
        assert!(!state.credential_prompt_awaiting_output);
        state.observe_credential_prompt_instance(instance(3, 1));
        assert!(!state.remember_credential_prompt(kind, prompt));
    }
}
