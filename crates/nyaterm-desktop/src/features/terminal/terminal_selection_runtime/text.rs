use gpui::{ClipboardItem, Context};
use nyaterm_ui::notification::NyaNotificationKind;

use crate::features::NyaTermApp;
use crate::features::terminal::state::TerminalSelectionTextStatus;
use crate::models::terminal::TerminalSelectionTextResult;
use crate::models::{TerminalSelection, TerminalSelectionTextError};

#[cfg(test)]
mod tests;

impl NyaTermApp {
    pub(in crate::features) fn reanchor_terminal_selection_for_session(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(history) = self
            .terminal
            .view
            .views
            .get(session_id)
            .map(|view| view.history_state())
        else {
            return;
        };
        if self.terminal.selection.advance_history(session_id, history) {
            self.ensure_terminal_selection_text(cx);
        }
    }

    pub(in crate::features) fn terminal_has_selection_for_session(&self, session_id: &str) -> bool {
        self.terminal
            .selection
            .session_id
            .as_deref()
            .or(self.session.active_id())
            == Some(session_id)
            && self
                .terminal
                .selection
                .selection
                .is_some_and(|selection| !selection.is_empty())
    }

    pub(in crate::features) fn ensure_terminal_selection_text(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.terminal.selection.session_id.clone()
            && let Some(history) = self
                .terminal
                .view
                .views
                .get(&id)
                .map(|view| view.history_state())
        {
            self.terminal.selection.advance_history(&id, history);
        }
        let Some(selection) = self
            .terminal
            .selection
            .selection
            .filter(|selection| !selection.is_empty())
        else {
            return;
        };
        let Some(session_id) = self
            .terminal
            .selection
            .session_id
            .clone()
            .or_else(|| self.session.active_id_owned())
        else {
            return;
        };
        let key = (session_id.clone(), selection);
        if self.terminal.selection.text.key.as_ref() == Some(&key) {
            return;
        }
        self.terminal.selection.text.invalidate_preserving_copy();
        // Small selections keep the snapshot fast path. Large ranges are read only
        // on the frame worker; never rebuild the entire UI-side history here.
        let immediate = self.selected_terminal_text();
        self.terminal.selection.text.key = Some(key.clone());
        let history = self.terminal.selection.source_history.or_else(|| {
            self.terminal
                .view
                .views
                .get(&session_id)
                .map(|view| view.history_state())
        });
        if let Some(source) = self.terminal.selection.source_history
            && self
                .terminal
                .view
                .views
                .get(&session_id)
                .is_some_and(|view| !view.history_state().preserves_coordinates_from(source))
        {
            self.terminal.selection.text.status =
                TerminalSelectionTextStatus::Failed(TerminalSelectionTextError::StaleHistory);
            return;
        }
        if let Some(text) = immediate {
            self.terminal.selection.text.status = TerminalSelectionTextStatus::Ready(Some(text));
            if let Some(feedback) = self.terminal.selection.text.pending_copy.take() {
                self.copy_terminal_selection_with_feedback(feedback, cx);
                self.terminal.selection.text.deferred_copy_completed = true;
            }
            return;
        }
        let Some(history) = history else {
            self.terminal.selection.text.status =
                TerminalSelectionTextStatus::Failed(TerminalSelectionTextError::MissingSession);
            return;
        };
        self.terminal.selection.text.status = TerminalSelectionTextStatus::Pending;
        let generation = self.terminal.selection.text.generation;
        let response = if selection.all_buffer {
            futures::future::Either::Left(
                self.terminal
                    .view
                    .frame_pipeline
                    .request_all_text(session_id),
            )
        } else {
            futures::future::Either::Right(
                self.terminal
                    .view
                    .frame_pipeline
                    .request_selection_text_frame(session_id, selection, history),
            )
        };
        self.terminal.selection.text.task = Some(cx.spawn(async move |this, cx| {
            let result = match response {
                futures::future::Either::Left(response) => response.await.map(|text| {
                    Ok(TerminalSelectionTextResult {
                        text,
                        selection,
                        history,
                    })
                }),
                futures::future::Either::Right(response) => response.await,
            }
            .unwrap_or(Err(TerminalSelectionTextError::WorkerUnavailable));
            let mut result = Some(result);
            loop {
                let complete = this
                    .update(cx, |this, cx| {
                        let current = this
                            .terminal
                            .view
                            .views
                            .get(&key.0)
                            .map(|view| view.history_state())
                            .unwrap_or(history);
                        let expected = if selection.all_buffer {
                            Some(selection)
                        } else {
                            selection.reanchored_history(history, current)
                        };
                        if this.terminal.selection.text.generation != generation
                            || !this.terminal_has_selection_for_session(&key.0)
                            || (expected.is_some() && this.terminal.selection.selection != expected)
                        {
                            return true;
                        }
                        if let Some(Ok(frame)) = result.as_ref()
                            && !frame.selection.all_buffer
                            && current.changes_since(frame.history).is_none()
                            && frame.history.changes_since(current).is_some()
                        {
                            // A priority copy can overtake coalesced output frames. Wait
                            // for its history without reissuing or rereading the text.
                            if let Some(view) = this.terminal.view.views.get_mut(&key.0)
                                && view.pending_snapshot_offsets.insert(0)
                            {
                                this.terminal.view.frame_pipeline.request_anchored_snapshot(
                                    key.0.clone(),
                                    0,
                                    current,
                                    false,
                                    this.settings
                                        .summary()
                                        .terminal_action_links_matchers
                                        .clone(),
                                    true,
                                );
                            }
                            this.drain_terminal_frame_events_for_input_wake(cx);
                            return false;
                        }
                        this.terminal.selection.text.task = None;
                        if expected.is_none() {
                            result = Some(Err(TerminalSelectionTextError::StaleHistory));
                        }
                        match result.take().expect("selection response committed once") {
                            Ok(frame) => {
                                let effective = if frame.selection.all_buffer {
                                    Some(frame.selection)
                                } else {
                                    frame.selection.reanchored_history(frame.history, current)
                                };
                                if effective != expected {
                                    this.terminal.selection.text.invalidate_preserving_copy();
                                    this.ensure_terminal_selection_text(cx);
                                    return true;
                                }
                                this.terminal.selection.text.status =
                                    TerminalSelectionTextStatus::Ready(frame.text);
                            }
                            Err(error) => {
                                this.terminal.selection.text.status =
                                    TerminalSelectionTextStatus::Failed(error)
                            }
                        }
                        if let Some(feedback) = this.terminal.selection.text.pending_copy.take() {
                            this.copy_terminal_selection_with_feedback(feedback, cx);
                            this.terminal.selection.text.deferred_copy_completed = true;
                        }
                        this.schedule_terminal_selected_occurrence_search(cx);
                        cx.notify();
                        true
                    })
                    .unwrap_or(true);
                if complete {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(16))
                    .await;
            }
        }));
    }

    pub(in crate::features) fn copy_terminal_selection(&mut self, cx: &mut Context<Self>) -> bool {
        self.copy_terminal_selection_with_feedback(false, cx)
    }

    pub(in crate::features) fn copy_terminal_selection_request(
        &mut self,
        key: &(String, TerminalSelection),
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        if !self.terminal.selection.text.deferred_copy_completed
            && self.terminal.selection.text.generation == generation
            // A coordinate-only rebase preserves the logical request generation.
            && self.terminal.selection.text.key.as_ref().is_some_and(|(id, cached)| {
                id == &key.0 && self.terminal.selection.selection == Some(*cached)
            })
            && self.terminal_has_selection_for_session(&key.0)
        {
            self.copy_terminal_selection_with_feedback(true, cx);
            if !matches!(
                self.terminal.selection.text.status,
                TerminalSelectionTextStatus::Pending
            ) {
                self.terminal.selection.text.deferred_copy_completed = true;
            }
        }
    }

    fn copy_terminal_selection_with_feedback(
        &mut self,
        feedback: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self
            .terminal
            .selection
            .selection
            .is_some_and(|selection| !selection.is_empty())
        {
            return false;
        }
        self.ensure_terminal_selection_text(cx);
        match &self.terminal.selection.text.status {
            TerminalSelectionTextStatus::Ready(Some(text)) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                self.shell
                    .set_status("copied terminal selection".to_string());
                self.terminal.menus.actions_open = false;
                if feedback {
                    self.notify_operation(
                        "copy-terminal-selection",
                        NyaNotificationKind::Success,
                        rust_i18n::t!("common.copied").to_string(),
                        cx,
                    );
                }
                cx.notify();
            }
            TerminalSelectionTextStatus::Pending => {
                let pending = &mut self.terminal.selection.text.pending_copy;
                *pending = Some(pending.unwrap_or(false) || feedback);
            }
            TerminalSelectionTextStatus::Failed(_)
            | TerminalSelectionTextStatus::Ready(None)
            | TerminalSelectionTextStatus::Unrequested => {
                if feedback {
                    self.notify_operation(
                        "copy-terminal-selection",
                        NyaNotificationKind::Error,
                        rust_i18n::t!("common.copyFailed").to_string(),
                        cx,
                    );
                }
            }
        }
        // An existing selection consumes Copy, including pending/failed extraction.
        // Falling back here would silently copy unrelated visible text.
        true
    }

    pub(in crate::features) fn copy_terminal_selection_or_visible(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if !self.copy_terminal_selection_with_feedback(true, cx) {
            self.copy_terminal_visible_text(cx);
        }
    }
}
