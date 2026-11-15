//! Terminal input tracking, availability and shell editing capability adapters.
use super::diagnostics::CommandSuggestionInputTiming;
use crate::features::NyaTermApp;
use crate::terminal::terminal_byte_index_for_cell_col;
use gpui::Context;
use nyaterm_core::completion::types::{
    CompletionFillPolicy, CompletionMode, CompletionScope, ShellCommandContext, ShellDialect,
};
use nyaterm_core::terminal::editing::ShellLineEditing;
use nyaterm_core::{TerminalInputState, terminal_input_tracker_below_min_chars};
use std::time::Instant;
/// True when `next` is `previous` plus characters typed at the end.
pub(super) fn command_suggestion_draft_only_appended(previous: &str, next: &str) -> bool {
    !previous.is_empty() && next.len() > previous.len() && next.starts_with(previous)
}

pub(super) fn command_suggestion_input_can_defer_refresh(state: &TerminalInputState) -> bool {
    !state.desynced && !state.multiline && state.cursor == state.value.len()
}

pub(super) fn command_suggestion_input_obvious_pager_prefix(state: &TerminalInputState) -> bool {
    let value = state.value.trim_start();
    value.starts_with('/') || value.starts_with('?') || value.starts_with(':')
}

pub(super) fn command_suggestion_input_candidate_chars(state: &TerminalInputState) -> usize {
    state.value.trim_start().chars().count()
}

/// Whether the tracked line holds any actual command text.
///
/// Remote pagers on switches consume keys such as Space without echoing them,
/// so the local tracker can hold nothing but separators while the remote screen
/// owns the cursor. That is not a command context: the token completer reads a
/// trailing separator as an empty first token and answers with the whole
/// catalog, and the whole-line search treats it as the manual empty query and
/// answers with recent history. Requiring real text keeps this structural
/// instead of matching any vendor's pager prompt.
pub(super) fn command_suggestion_input_has_command_text(state: &TerminalInputState) -> bool {
    !state.value.trim().is_empty()
}

#[cfg(test)]
pub(super) fn command_history_input_update(
    state: &mut TerminalInputState,
    text: &str,
) -> Option<String> {
    let submitted = if text.contains('\r') || text.contains('\n') {
        let submitted =
            nyaterm_core::terminal::input_tracker::get_tracked_submission_command(state);
        (!submitted.is_empty()).then_some(submitted)
    } else {
        None
    };
    nyaterm_core::terminal::input_tracker::apply_terminal_input_data_in_place(state, text);
    submitted
}

pub(super) fn terminal_line_prefix_for_cell_col(line: &str, cell_col: usize) -> String {
    let end = terminal_byte_index_for_cell_col(line, cell_col);
    line.get(..end).unwrap_or(line).to_string()
}

impl NyaTermApp {
    pub(in crate::features::terminal) fn command_suggestion_fill_policy(
        &self,
        session_id: &str,
    ) -> CompletionFillPolicy {
        let shell_session = self.session.metadata(session_id).is_some_and(|metadata| {
            matches!(
                metadata.launch_config,
                crate::models::SessionLaunchConfig::Local(_)
                    | crate::models::SessionLaunchConfig::Ssh(_)
            )
        });
        if shell_session
            && self
                .terminal
                .editing
                .sessions
                .get(session_id)
                .is_some_and(|state| {
                    state.confirmed_input_at_end()
                        && state.allows_editing()
                        && state.editor_evidence.is_some()
                })
        {
            CompletionFillPolicy::ReplaceAllowed
        } else {
            CompletionFillPolicy::AppendOnly
        }
    }

    pub(in crate::features::terminal) fn command_suggestions_allowed_for_session(
        &self,
        session_id: &str,
    ) -> bool {
        self.session.active_id() == Some(session_id)
            && !self
                .terminal
                .editing
                .sessions
                .get(session_id)
                .is_some_and(|state| state.browsing_history)
            && !self.session.is_disconnected(session_id)
            && !self.active_terminal_input_blocked()
            && !self.security.screen_locked()
            && !self.active_terminal_uses_alternate_screen()
            && !self.is_credential_prompt_input_mode()
            && self.terminal.assist.credential_suggestions.is_none()
            && !self.terminal.assist.commands.suppression.is_suppressed()
            && !self
                .terminal_protocol_state_for_session(session_id)
                .mouse_reporting
            && self.active_terminal_display_offset() == 0
            && self
                .settings
                .summary()
                .interaction_command_suggestions_enabled
            && !self.sync_input.may_fan_out_from(session_id)
    }

    pub(in crate::features::terminal) fn command_suggestion_input_is_confirmed(&self) -> bool {
        if self
            .session
            .active_id()
            .is_none_or(|id| !self.terminal.editing.is_active(id))
        {
            return false;
        }
        self.terminal
            .editing
            .state()
            .is_some_and(|state| state.confirmed_input_at_end())
    }

    pub(in crate::features::terminal) fn retain_command_suggestions_during_edit(
        &mut self,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        if self.terminal.assist.commands.begin_input_refresh(text) {
            cx.notify();
        }
    }

    pub(super) fn command_suggestion_refresh_can_remain_visible(&self) -> bool {
        let Some(popup) = self.terminal.assist.commands.popup.as_ref() else {
            return false;
        };
        let input = self.terminal.editing.assist_input();
        popup.refreshing
            && self.command_suggestions_allowed_for_session(&popup.session_id)
            && self.sync_input.revision() == popup.sync_revision
            && command_suggestion_input_can_defer_refresh(input)
            && !input.line_rewrite_required
            && !input.paste_mode
            && command_suggestion_input_has_command_text(input)
            && !command_suggestion_input_obvious_pager_prefix(input)
            && (popup.mode == CompletionMode::History
                || !terminal_input_tracker_below_min_chars(
                    input,
                    self.settings
                        .summary()
                        .interaction_command_suggestion_min_chars
                        .max(1) as usize,
                )
                || input.value.contains(char::is_whitespace))
    }

    /// Called from state/terminal events, never from element construction.
    pub(in crate::features) fn reconcile_command_suggestion_availability(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.command_suggestion_refresh_can_remain_visible() {
            return;
        }
        let Some(popup) = self.terminal.assist.commands.popup.as_ref() else {
            return;
        };
        if popup.refreshing
            || !self.command_suggestions_allowed_for_session(&popup.session_id)
            || !self.command_suggestion_input_is_confirmed()
            || self.sync_input.revision() != popup.sync_revision
        {
            self.hide_command_suggestions_if_present(cx);
            return;
        }
        let draft = self.terminal.editing.assist_input().value.clone();
        let version = self.terminal.editing.state().unwrap().model.version;
        if draft != popup.draft || version != popup.input_version {
            // Typing forward only narrows the visible rows, so keep the popup
            // instead of flickering until the debounced search lands. Any
            // other edit (backspace, mid-line, paste) still hides it.
            if !command_suggestion_draft_only_appended(&popup.draft, &draft) {
                self.hide_command_suggestions_if_present(cx);
                return;
            }
            if self.terminal.assist.commands.narrow_draft(draft, version) {
                cx.notify();
            }
        }
        let Some(popup) = self.terminal.assist.commands.popup.as_ref() else {
            return;
        };
        let policy = self.command_suggestion_fill_policy(&popup.session_id);
        if self.terminal.assist.commands.retain_fillable(policy) {
            cx.notify();
        }
    }

    pub(in crate::features) fn completion_has_marked_shell(&self, session_id: &str) -> bool {
        self.terminal
            .editing
            .sessions
            .get(session_id)
            .is_some_and(|state| state.observed_anchor.flatten().is_some())
    }

    /// Whether the active session's program is driving the alternate screen.
    ///
    /// Full-screen programs such as vim, less, top and nano own the keyboard
    /// while they are there: their input is not a shell command line, so command
    /// suggestions must stay closed and must never consume keys. This is a
    /// protocol signal, unlike the submitted-command heuristic in
    /// `command_starts_suggestion_suppressing_program`, which only covers
    /// programs the shell line happened to reveal.
    pub(in crate::features) fn active_terminal_uses_alternate_screen(&self) -> bool {
        self.session
            .active_id()
            .filter(|session_id| !session_id.is_empty())
            .and_then(|session_id| self.terminal.view.views.get(session_id))
            .is_some_and(|view| view.protocol_state.alternate_screen)
    }

    pub(in crate::features) fn note_command_suggestion_input(
        &mut self,
        bytes: &[u8],
        cx: &mut Context<Self>,
    ) {
        let started_at = Instant::now();
        let byte_count = bytes.len();
        let popup_visible_at_start = self.terminal.assist.commands.popup.is_some();
        let mut timing = CommandSuggestionInputTiming::default();
        macro_rules! finish {
            ($outcome:expr, $pattern_chars:expr) => {{
                self.log_command_suggestion_input_diagnostic(
                    started_at,
                    byte_count,
                    popup_visible_at_start,
                    $outcome,
                    $pattern_chars,
                    timing,
                );
                return;
            }};
        }

        if self.terminal.assist.credential_suggestions.is_some()
            || self.is_credential_prompt_input_mode()
        {
            return;
        }
        if !self
            .settings
            .summary()
            .interaction_command_suggestions_enabled
        {
            self.clear_command_suggestion_draft(cx);
            return;
        }
        if self.session.active_id().is_none() {
            self.clear_command_suggestion_draft(cx);
            return;
        }
        if self.active_terminal_uses_alternate_screen() {
            self.clear_command_suggestion_draft(cx);
            finish!("alternate_screen", 0);
        }
        let utf8_started_at = Instant::now();
        let text = match std::str::from_utf8(bytes) {
            Ok(text) => {
                timing.utf8 = utf8_started_at.elapsed();
                text
            }
            Err(_) => {
                timing.utf8 = utf8_started_at.elapsed();
                self.clear_command_suggestion_draft(cx);
                finish!("non_utf8", 0);
            }
        };
        if text.is_empty() {
            finish!("empty_text", 0);
        }

        // Typing invalidates an explicit choice before the debounced search can
        // publish its replacement. Enter must not accept the old selection.
        if self.terminal.assist.commands.clear_selection() {
            cx.notify();
        }

        if self.terminal.assist.commands.suppression.is_suppressed() {
            let hide_started_at = Instant::now();
            self.terminal.assist.commands.invalidate_search();
            if self.terminal.assist.commands.hide() {
                cx.notify();
            }
            timing.hide_popup = hide_started_at.elapsed();
            finish!("suppressed", 0);
        }

        let min_chars_started_at = Instant::now();
        let min_chars = self
            .settings
            .summary()
            .interaction_command_suggestion_min_chars
            .max(1) as usize;
        let below_min_chars = self.terminal.assist.commands.mode == CompletionMode::Token
            && terminal_input_tracker_below_min_chars(
                self.terminal.editing.assist_input(),
                min_chars,
            )
            && !self
                .terminal
                .editing
                .assist_input()
                .value
                .contains(char::is_whitespace);
        timing.min_chars = min_chars_started_at.elapsed();
        if below_min_chars {
            let hide_started_at = Instant::now();
            self.terminal.assist.commands.invalidate_search();
            if self.terminal.assist.commands.hide() {
                cx.notify();
            }
            timing.hide_popup = hide_started_at.elapsed();
            finish!("below_min_chars", 0);
        }

        // Automatic suggestions need real command text. A pager keypress (for
        // example Space on a switch's `--More--` prompt) reaches the tracker as
        // ordinary input, and only whitespace means no local command line grew.
        if self.terminal.assist.commands.mode == CompletionMode::Token
            && !command_suggestion_input_has_command_text(self.terminal.editing.assist_input())
        {
            let hide_started_at = Instant::now();
            self.terminal.assist.commands.invalidate_search();
            if self.terminal.assist.commands.hide() {
                cx.notify();
            }
            timing.hide_popup = hide_started_at.elapsed();
            finish!("blank_input", 0);
        }

        let pattern_started_at = Instant::now();
        let pattern_chars =
            command_suggestion_input_candidate_chars(self.terminal.editing.assist_input());
        timing.pattern = pattern_started_at.elapsed();

        let pager_started_at = Instant::now();
        let pager_input =
            command_suggestion_input_obvious_pager_prefix(self.terminal.editing.assist_input());
        timing.pager = pager_started_at.elapsed();
        if pager_input && self.terminal.assist.commands.mode == CompletionMode::Token {
            let hide_started_at = Instant::now();
            self.terminal.assist.commands.invalidate_search();
            if self.terminal.assist.commands.hide() {
                cx.notify();
            }
            timing.hide_popup = hide_started_at.elapsed();
            finish!("pager_input", pattern_chars);
        }

        let eligibility_started_at = Instant::now();
        let can_suggest =
            command_suggestion_input_can_defer_refresh(self.terminal.editing.assist_input());
        timing.eligibility = eligibility_started_at.elapsed();
        if !can_suggest {
            let hide_started_at = Instant::now();
            self.terminal.assist.commands.invalidate_search();
            if self.terminal.assist.commands.hide() {
                cx.notify();
            }
            timing.hide_popup = hide_started_at.elapsed();
            finish!("not_eligible", pattern_chars);
        }
        let schedule_started_at = Instant::now();
        self.schedule_command_suggestion_refresh(cx);
        timing.schedule = schedule_started_at.elapsed();
        finish!("scheduled", pattern_chars);
    }

    pub(in crate::features) fn read_terminal_input_line_for_session(
        &self,
        session_id: &str,
    ) -> Option<String> {
        let snapshot = self.terminal_snapshot_for_session(Some(session_id), 0);
        if snapshot.cursor.row == usize::MAX {
            return None;
        }
        let line = snapshot.line(snapshot.cursor.row)?;
        Some(terminal_line_prefix_for_cell_col(line, snapshot.cursor.col))
    }

    pub(in crate::features) fn command_suggestion_context(
        &self,
        session: &str,
    ) -> ShellCommandContext {
        let mut context =
            self.commands
                .command_context(session)
                .cloned()
                .unwrap_or(ShellCommandContext {
                    scope: CompletionScope::legacy(),
                    dialect: ShellDialect::Unknown,
                    command_events: false,
                });
        if let Some(evidence) = self
            .terminal
            .editing
            .sessions
            .get(session)
            .and_then(|state| state.editor_evidence)
        {
            if let Some(dialect) = evidence.dialect {
                context.dialect = dialect;
            } else if evidence.platform
                == Some(nyaterm_core::action_links::ActionLinkPlatform::Posix)
                && matches!(
                    context.dialect,
                    ShellDialect::Cmd | ShellDialect::PowerShell
                )
            {
                // A POSIX prompt cannot identify bash vs zsh, but it disproves
                // a stale Windows parsing hint after leaving a nested shell.
                context.dialect = ShellDialect::Unknown;
            }
        }
        context
    }

    /// Line-editing backend of the active session's shell.
    ///
    /// Only current snapshot evidence selects a destructive edit's encoding.
    pub(super) fn shell_line_editing(&self, session_id: &str) -> Option<ShellLineEditing> {
        self.terminal
            .editing
            .sessions
            .get(session_id)?
            .editor_evidence
            .map(|evidence| evidence.line_editing)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        command_history_input_update, command_suggestion_draft_only_appended,
        command_suggestion_input_can_defer_refresh, command_suggestion_input_candidate_chars,
        command_suggestion_input_obvious_pager_prefix, terminal_line_prefix_for_cell_col,
    };
    use nyaterm_core::{TerminalInputState, apply_terminal_input_data};
    #[test]
    fn terminal_line_prefix_uses_terminal_cells_for_wide_chars() {
        assert_eq!(terminal_line_prefix_for_cell_col("界x", 0), "");
        assert_eq!(terminal_line_prefix_for_cell_col("界x", 1), "");
        assert_eq!(terminal_line_prefix_for_cell_col("界x", 2), "界");
        assert_eq!(terminal_line_prefix_for_cell_col("界x", 3), "界x");
    }

    #[test]
    fn terminal_line_prefix_keeps_combining_mark_with_base_char() {
        let text = "e\u{301}x";

        assert_eq!(terminal_line_prefix_for_cell_col(text, 0), "");
        assert_eq!(terminal_line_prefix_for_cell_col(text, 1), "e\u{301}");
        assert_eq!(terminal_line_prefix_for_cell_col(text, 2), "e\u{301}x");
    }

    #[test]
    fn only_forward_typing_keeps_the_popup_alive() {
        assert!(command_suggestion_draft_only_appended("git c", "git ch"));
        assert!(!command_suggestion_draft_only_appended("git ch", "git c"));
        assert!(!command_suggestion_draft_only_appended("git c", "git c"));
        assert!(!command_suggestion_draft_only_appended("", "g"));
        assert!(!command_suggestion_draft_only_appended("git c", "ls"));
    }

    #[test]
    fn command_suggestion_input_defer_refresh_requires_cursor_at_end() {
        let mut state = TerminalInputState {
            value: "git status".to_string(),
            cursor: 3,
            ..TerminalInputState::default()
        };
        assert!(!command_suggestion_input_can_defer_refresh(&state));

        state.cursor = state.value.len();
        assert!(command_suggestion_input_can_defer_refresh(&state));

        state.desynced = true;
        assert!(!command_suggestion_input_can_defer_refresh(&state));
    }

    #[test]
    fn command_suggestion_input_detects_obvious_pager_prefix_without_sanitizing() {
        let state = TerminalInputState {
            value: "  /search".to_string(),
            cursor: 9,
            ..TerminalInputState::default()
        };
        assert!(command_suggestion_input_obvious_pager_prefix(&state));

        let state = TerminalInputState {
            value: "git status".to_string(),
            cursor: 10,
            ..TerminalInputState::default()
        };
        assert!(!command_suggestion_input_obvious_pager_prefix(&state));
        assert_eq!(command_suggestion_input_candidate_chars(&state), 10);
    }

    #[test]
    fn command_history_input_update_captures_submission_before_enter_reset() {
        let mut state = apply_terminal_input_data(&TerminalInputState::new(), "git status");

        let submitted = command_history_input_update(&mut state, "\r");

        assert_eq!(submitted.as_deref(), Some("git status"));
        assert!(state.value.is_empty());
    }
}
