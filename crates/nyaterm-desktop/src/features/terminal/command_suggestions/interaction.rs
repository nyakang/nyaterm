//! Popup key ownership, dismissal and history deletion adapters.
use super::state::command_suggestion_item_for_selection;
use crate::features::NyaTermApp;
use gpui::{Context, KeyDownEvent};
impl NyaTermApp {
    pub(in crate::features) fn dismiss_command_suggestions(&mut self, cx: &mut Context<Self>) {
        // Keep the tracked input and suppression for continued typing.
        if self.terminal.assist.commands.dismiss() {
            cx.notify();
        }
    }

    pub(in crate::features) fn clear_command_suggestion_draft(&mut self, cx: &mut Context<Self>) {
        if self.terminal.assist.commands.dismiss() {
            cx.notify();
        }
    }

    pub(super) fn hide_command_suggestions_if_present(&mut self, cx: &mut Context<Self>) {
        if self.terminal.assist.commands.hide() {
            cx.notify();
        }
    }

    /// Handle suggestion popup keys. Returns true when the key was consumed.
    pub(in crate::features) fn handle_command_suggestion_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(state) = self.terminal.assist.commands.popup.as_ref() else {
            return false;
        };
        if state.items.is_empty() {
            return false;
        }
        if state.refreshing {
            if !event.keystroke.modifiers.platform
                && !event.keystroke.modifiers.alt
                && !event.keystroke.modifiers.control
                && matches!(
                    event.keystroke.key.as_str(),
                    "escape" | "up" | "down" | "tab" | "enter"
                )
            {
                self.dismiss_command_suggestions(cx);
                return event.keystroke.key == "escape";
            }
            return false;
        }
        let session_id = state.session_id.clone();
        let selected =
            command_suggestion_item_for_selection(&state.items, state.selected_index).is_some();
        let accepting =
            selected && event.keystroke.key == "enter" && !event.keystroke.modifiers.modified();
        if self.session.active_id() != Some(session_id.as_str())
            || self
                .terminal_surface_bounds_for_session(Some(&session_id))
                .is_none()
        {
            if accepting {
                return self
                    .reject_command_suggestion(
                        "Suggestion session or terminal surface changed; select it again",
                        cx,
                    )
                    .handled();
            }
            self.terminal.assist.commands.hide();
            cx.notify();
            return false;
        }
        if self.active_terminal_uses_alternate_screen() {
            // A stale shell suggestion must not submit to the TUI. Other keys,
            // including Escape, still belong to the foreground program.
            if accepting {
                return self
                    .reject_command_suggestion(
                        "Cannot fill a shell suggestion in the alternate screen",
                        cx,
                    )
                    .handled();
            }
            self.terminal.assist.commands.hide();
            cx.notify();
            return false;
        }
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform || keystroke.modifiers.alt || keystroke.modifiers.control {
            return false;
        }
        if keystroke.key == "enter" && keystroke.modifiers.modified() {
            return false;
        }
        if !selected && matches!(keystroke.key.as_str(), "tab" | "enter") {
            self.dismiss_command_suggestions(cx);
            return false;
        }
        if accepting || (selected && keystroke.key == "tab") {
            // A consumed suggestion key has no remote press owner. Its repeats
            // and Kitty release must not turn into a later command submission.
            if let Some(view) = self.terminal.view.views.get_mut(&session_id) {
                view.pressed_keys.remove(&keystroke.key);
            }
            if accepting && event.is_held {
                return true;
            }
        }
        match keystroke.key.as_str() {
            "escape" => {
                self.dismiss_command_suggestions(cx);
                true
            }
            "up" | "down" | "tab" => {
                let direction = if keystroke.key == "up"
                    || (keystroke.key == "tab" && keystroke.modifiers.shift)
                {
                    -1
                } else {
                    1
                };
                if self.terminal.assist.commands.move_selection(direction) {
                    cx.notify();
                }
                true
            }
            "enter" => self.apply_selected_command_suggestion(false, cx),
            _ => false,
        }
    }

    pub(in crate::features) fn delete_command_suggestion_history(
        &mut self,
        command: String,
        cx: &mut Context<Self>,
    ) {
        let command = command.trim().to_string();
        if command.is_empty() {
            return;
        }
        self.dismiss_command_suggestions(cx);
        let runtime = self.commands.completion_runtime();
        cx.spawn(async move |this, cx| {
            let outcome = runtime.delete(command.clone()).await;
            let _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(history) => {
                        this.commands.replace_command_history(history);
                        this.session.remove_command_from_all_history(&command);
                        this.shell.set_status("History command deleted");
                        this.refresh_command_suggestions(cx);
                    }
                    Err(error) => this
                        .shell
                        .set_status(format!("failed to delete history: {error}")),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
