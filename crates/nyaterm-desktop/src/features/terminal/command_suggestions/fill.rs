//! Terminal fill authorization, writes, failures and echo confirmation.
use super::paste::SuggestionPasteContext;
use super::state::command_suggestion_item_for_selection;
use crate::features::NyaTermApp;
use crate::features::terminal::editing_payload::shell_edit_payload_with_backend;
use crate::features::terminal::editing_state::{SnapshotEvidence, SuggestionFillRequest};
use gpui::Context;
use nyaterm_core::completion::fill::SuggestionFillRejection;
use nyaterm_core::completion::fill::{SuggestionFill, plan_suggestion_fill};
use nyaterm_core::completion::types::{CompletionFillPolicy, CompletionMode};
use nyaterm_core::terminal::editing::{EDIT_CONFIRMATION_TIMEOUT, EditIntent, plan_edit};
use std::time::Instant;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SuggestionSubmitResult {
    NotHandled,
    Rejected,
    WriteFailed,
    Submitted,
    PasteReviewRequested,
    NoChange,
}

impl SuggestionSubmitResult {
    pub(super) fn handled(&self) -> bool {
        *self != Self::NotHandled
    }
}

pub(super) fn suggestion_fill_rejection_message(reason: SuggestionFillRejection) -> &'static str {
    match reason {
        SuggestionFillRejection::UnsupportedText => "Suggestion contains unsupported text",
        SuggestionFillRejection::UnconfirmedInput => {
            "Input is not confirmed at the end of the line"
        }
    }
}

impl NyaTermApp {
    pub(in crate::features) fn apply_selected_command_suggestion(
        &mut self,
        execute: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        self.submit_selected_command_suggestion(execute, cx)
            .handled()
    }

    pub(super) fn reject_command_suggestion(
        &mut self,
        message: &'static str,
        cx: &mut Context<Self>,
    ) -> SuggestionSubmitResult {
        self.dismiss_command_suggestions(cx);
        self.shell.set_status(message);
        cx.notify();
        SuggestionSubmitResult::Rejected
    }

    pub(super) fn submit_selected_command_suggestion(
        &mut self,
        execute_requested: bool,
        cx: &mut Context<Self>,
    ) -> SuggestionSubmitResult {
        let Some(popup) = self.terminal.assist.commands.popup.clone() else {
            return SuggestionSubmitResult::NotHandled;
        };
        if popup.refreshing {
            return SuggestionSubmitResult::NotHandled;
        }
        let execute_requested = execute_requested && popup.mode == CompletionMode::History;
        let Some(item) = command_suggestion_item_for_selection(&popup.items, popup.selected_index)
        else {
            return SuggestionSubmitResult::NotHandled;
        };
        if !item.fillable {
            return self.reject_command_suggestion(
                "Cannot safely fill this match; edit the command manually",
                cx,
            );
        }
        let context = SuggestionPasteContext::from_popup(&popup);
        let snapshot = match self.validate_command_suggestion_input(&context, cx) {
            Ok(snapshot) => snapshot,
            Err(message) => return self.reject_command_suggestion(message, cx),
        };
        let state = self
            .terminal
            .editing
            .state()
            .expect("validated editing state");
        match plan_suggestion_fill(&state.model.input, &item.command) {
            SuggestionFill::Rejected(reason) => {
                return self
                    .reject_command_suggestion(suggestion_fill_rejection_message(reason), cx);
            }
            SuggestionFill::PasteReview(command) => {
                self.terminal
                    .paste
                    .queue_suggestion(context, command.to_owned());
                self.dismiss_command_suggestions(cx);
                cx.notify();
                return SuggestionSubmitResult::PasteReviewRequested;
            }
            SuggestionFill::NoChange => {
                self.dismiss_command_suggestions(cx);
                self.shell.set_status(if execute_requested {
                    "Command already filled; press Enter again to execute"
                } else {
                    "Command already filled"
                });
                cx.notify();
                return SuggestionSubmitResult::NoChange;
            }
            SuggestionFill::Append(suffix) => {
                // Logical input converts the charset and advances the shell
                // model once, after the primary write succeeds.
                self.dismiss_command_suggestions(cx);
                if !self
                    .send_terminal_input_without_suggestion_track(suffix.as_bytes().to_vec(), cx)
                {
                    self.terminal.editing.state_mut().invalidate();
                    return SuggestionSubmitResult::WriteFailed;
                }
            }
            SuggestionFill::Replace(command) => {
                // Synchronized input fans the same bytes out to every session;
                // a whole-line rewrite based on this session's echo would
                // corrupt peers whose line differs.
                if self.sync_input.may_fan_out_from(&popup.session_id) {
                    return self.reject_command_suggestion(
                        "Suggestion replacement is disabled while input is synchronized",
                        cx,
                    );
                }
                let Some(backend) = self.shell_line_editing(&popup.session_id) else {
                    return self.reject_command_suggestion("Shell editor is not confirmed", cx);
                };
                if self.command_suggestion_fill_policy(&popup.session_id)
                    != CompletionFillPolicy::ReplaceAllowed
                {
                    return self.reject_command_suggestion(
                        "This terminal has no confirmed shell editing capability",
                        cx,
                    );
                }
                let Some(plan) = plan_edit(
                    &state.model.input,
                    state.model.version,
                    EditIntent::Replace {
                        range: 0..state.model.input.value.len(),
                        text: command,
                    },
                ) else {
                    return self
                        .reject_command_suggestion("Cannot plan this shell replacement", cx);
                };
                if !backend.allows_cursor_motion() && plan.move_steps != 0 {
                    return self.reject_command_suggestion(
                        "Console replacement requires the cursor at the end",
                        cx,
                    );
                }
                let insertion_wire = match self
                    .encode_session_outgoing(&popup.session_id, plan.insert_text.as_bytes())
                {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        self.dismiss_command_suggestions(cx);
                        self.terminal.editing.state_mut().invalidate();
                        self.shell.set_status(format!("input failed: {error}"));
                        cx.notify();
                        return SuggestionSubmitResult::WriteFailed;
                    }
                };
                let mode = self.terminal_key_mode_for_session(Some(&popup.session_id));
                let (wire, logical) =
                    shell_edit_payload_with_backend(&plan, backend, mode, insertion_wire);
                self.dismiss_command_suggestions(cx);
                self.terminal.view.frame_pipeline.arm_output_event_wake();
                if let Err(error) =
                    self.write_session_wire_input_recorded_as(&popup.session_id, &wire, &logical)
                {
                    self.terminal.editing.state_mut().invalidate();
                    self.shell.set_status(format!("input failed: {error}"));
                    cx.notify();
                    return SuggestionSubmitResult::WriteFailed;
                }
                // The framed writer records logical input but does not update
                // the editing model. Commit the prediction only after success.
                let state = self.terminal.editing.state_mut();
                state.model.accept_plan(plan, Instant::now());
                state.pending_evidence = Some(SnapshotEvidence::from_snapshot(&snapshot));
                state.mapping = None;
                state.selection = None;
                state.selection_origin_version = None;
                self.clear_terminal_selection(cx);
                self.arm_terminal_input_wake(cx);
                self.schedule_shell_edit_confirmation_timeout(cx);
            }
        }
        let state = self.terminal.editing.state_mut();
        state.suggestion_fill = Some(SuggestionFillRequest {
            request_id: state.confirmation_id,
            input_version: state.model.version,
            target: item.command.clone(),
            execute_requested,
            deadline: Instant::now() + EDIT_CONFIRMATION_TIMEOUT,
            sync_revision: popup.sync_revision,
        });
        self.shell.set_status(if execute_requested {
            "Suggestion filled; waiting for echo. Press Enter again to execute"
        } else {
            "Suggestion filled; waiting for echo"
        });
        cx.notify();
        SuggestionSubmitResult::Submitted
    }
}

#[cfg(test)]
mod tests {
    use super::suggestion_fill_rejection_message;
    use nyaterm_core::completion::fill::SuggestionFillRejection;

    #[test]
    fn planning_rejections_preserve_status_messages() {
        assert_eq!(
            suggestion_fill_rejection_message(SuggestionFillRejection::UnsupportedText),
            "Suggestion contains unsupported text"
        );
        assert_eq!(
            suggestion_fill_rejection_message(SuggestionFillRejection::UnconfirmedInput),
            "Input is not confirmed at the end of the line"
        );
    }
}
