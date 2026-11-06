use futures::StreamExt as _;
use gpui::Context;
use nyaterm_core::AiCommandCard;

use crate::features::{NyaTermApp, ai::is_agent_command_card};

impl NyaTermApp {
    pub(in crate::features) fn run_ai_command_card(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        self.apply_ai_command_card(index, true, cx);
    }

    pub(in crate::features) fn insert_ai_command_card_by_id(
        &mut self,
        card_id: String,
        cx: &mut Context<Self>,
    ) {
        self.apply_ai_command_card_by_id(card_id, false, cx);
    }

    pub(in crate::features) fn run_ai_command_card_by_id(
        &mut self,
        card_id: String,
        cx: &mut Context<Self>,
    ) {
        self.apply_ai_command_card_by_id(card_id, true, cx);
    }

    pub(in crate::features) fn find_ai_command_card(&self, card_id: &str) -> Option<AiCommandCard> {
        self.ai.find_command_card(card_id)
    }

    pub(in crate::features) fn run_history_command(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        self.apply_history_command(index, true, cx);
    }

    pub(in crate::features) fn apply_history_command(
        &mut self,
        index: usize,
        execute: bool,
        cx: &mut Context<Self>,
    ) {
        if self.session.active_id().is_none() {
            self.shell
                .set_status("start a terminal session before using history".to_string());
            cx.notify();
            return;
        }
        let Some(command_text) = self.session.active_command_history_entry(index) else {
            self.shell
                .set_status("history command is no longer available".to_string());
            cx.notify();
            return;
        };
        let mut command = command_text.trim().to_string();
        if command.is_empty() {
            self.shell
                .set_status("history command is empty".to_string());
            cx.notify();
            return;
        }
        if execute && !command.ends_with('\r') && !command.ends_with('\n') {
            command.push('\r');
        }
        self.send_terminal_input(command.into_bytes(), cx);
        self.shell.set_status(if execute {
            format!("ran history command '{command_text}'")
        } else {
            format!("inserted history command '{command_text}'")
        });
        cx.notify();
    }

    pub(in crate::features) fn apply_ai_command_card(
        &mut self,
        index: usize,
        execute: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(card) = self.ai.command_card(index) else {
            self.ai
                .set_panel_status("AI command card is no longer available");
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        };
        self.apply_ai_command_card_value(card, execute, cx);
    }

    pub(in crate::features) fn apply_ai_command_card_by_id(
        &mut self,
        card_id: String,
        execute: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(card) = self.find_ai_command_card(&card_id) else {
            self.ai
                .set_panel_status("AI command card is no longer available");
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        };
        self.apply_ai_command_card_value(card, execute, cx);
    }

    pub(in crate::features) fn apply_ai_command_card_value(
        &mut self,
        card: AiCommandCard,
        execute: bool,
        cx: &mut Context<Self>,
    ) {
        if self
            .ai
            .native_pending_call()
            .is_some_and(|call| call.id == card.id)
        {
            if execute {
                self.respond_to_mcp_approval(
                    &format!(
                        "{}:{}",
                        self.ai.native_owner().expect("native owner"),
                        card.id
                    ),
                    crate::features::mcp::McpApprovalDecision::AllowOnce,
                    cx,
                );
            }
            return;
        }
        let available_targets = self.ai_effective_target_session_ids();
        let declared_target = card
            .target_terminal_session_id
            .as_deref()
            .or_else(|| {
                card.target
                    .as_ref()
                    .map(|target| target.terminal_session_id.as_str())
            })
            .map(str::trim)
            .filter(|id| !id.is_empty());
        let target_session_id = match declared_target {
            Some(id) if available_targets.iter().any(|available| available == id) => id.to_string(),
            Some(id) => {
                self.ai.set_panel_status(format!(
                    "AI command target '{id}' is unavailable or outside the current scope"
                ));
                self.defer_ai_panel_snapshot_flush(cx);
                return;
            }
            None if available_targets.len() == 1 => available_targets[0].clone(),
            None if available_targets.len() > 1 => {
                self.ai.set_panel_status(
                    "AI command must select a target terminal when multiple targets are available",
                );
                self.defer_ai_panel_snapshot_flush(cx);
                return;
            }
            None => {
                self.ai
                    .set_panel_status("Start a terminal session before using an AI command");
                self.defer_ai_panel_snapshot_flush(cx);
                return;
            }
        };
        let mut command = card.command.trim().to_string();
        if command.is_empty() {
            self.ai.set_panel_status("AI command card has no command");
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        }
        let should_continue_agent = execute && is_agent_command_card(&card);
        if should_continue_agent && !self.ai.current_agent_command_card(&card.id) {
            self.ai
                .set_panel_status("AI Agent command is no longer awaiting approval");
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        }
        if should_continue_agent && self.ai.chat_is_pending() {
            self.ai
                .set_panel_status("AI Agent is continuing from the previous command");
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        }
        if should_continue_agent && self.ai.agent_target_busy(&target_session_id) {
            self.ai
                .set_panel_status("Another AI Agent command is running in this terminal");
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        }
        if should_continue_agent {
            match self.begin_ai_agent_execution(&card, &target_session_id, cx) {
                Ok(inserted) => self.record_ai_command_card_audit(&card, true, inserted, cx),
                Err(error) => self.ai.set_panel_status(error),
            }
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        }
        if execute && !command.ends_with('\r') && !command.ends_with('\n') {
            command.push('\r');
        }
        let input_bytes = command.clone().into_bytes();

        self.record_ai_command_card_audit(&card, execute, true, cx);

        self.send_terminal_input_to_session(target_session_id, input_bytes, cx);
        let status = if execute {
            format!("Ran AI command card '{}'", card.title)
        } else {
            format!("Inserted AI command card '{}'", card.title)
        };
        self.ai.set_panel_status(status);
        self.defer_ai_panel_snapshot_flush(cx);
    }

    pub(in crate::features) fn record_command_history_from_bytes(
        &mut self,
        session_id: Option<&str>,
        bytes: &[u8],
    ) {
        let sessions: Vec<&str> = session_id.into_iter().collect();
        self.record_command_history_for_sessions(&sessions, bytes);
    }

    /// Resolve a submitted command once and attach it to every successful session.
    /// Global command history is appended only once per submission.
    pub(in crate::features) fn record_command_history_for_sessions(
        &mut self,
        session_ids: &[&str],
        bytes: &[u8],
    ) {
        let Ok(text) = std::str::from_utf8(bytes) else {
            return;
        };
        if !text.contains('\n') && !text.contains('\r') {
            return;
        }
        let submitted: Vec<String> =
            if let Some(command) = self.terminal.take_pending_command_history_entry() {
                vec![command]
            } else {
                text.split(['\r', '\n'])
                    .map(str::trim)
                    .filter(|command| !command.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            };
        if submitted.is_empty() {
            return;
        }
        self.record_submitted_command_history(session_ids, submitted);
    }

    pub(in crate::features) fn record_submitted_command_history(
        &mut self,
        session_ids: &[&str],
        submitted: Vec<String>,
    ) {
        if submitted.is_empty() {
            return;
        }
        let mut recorded = std::collections::HashSet::new();
        let mut fallback = std::collections::HashSet::new();
        for session_id in session_ids {
            let context = self.commands.command_context(session_id).cloned();
            // The marked shell owns the observation. Do not consume its secret
            // latch until CommandAccepted arrives from that same submission.
            if self.completion_has_marked_shell(session_id)
                && context
                    .as_ref()
                    .is_some_and(|context| context.command_events)
            {
                continue;
            }
            let sensitive = self.commands.take_completion_sensitive(session_id)
                || self.recording_input_is_sensitive(session_id);
            let confirmed = self.commands.take_completion_confirmed(session_id);
            for command in &submitted {
                if !nyaterm_core::completion::privacy::command_may_be_learned(command, sensitive) {
                    continue;
                }
                self.session.record_command_history(session_id, command);
                if let Some(context) = context.clone().filter(|_| confirmed) {
                    let include_global = !recorded.contains(command);
                    if self.commands.observe_completion_submission(
                        context,
                        command.clone(),
                        include_global,
                    ) {
                        recorded.insert(command.clone());
                    } else {
                        self.settings
                            .update_store_status("completion worker is unavailable", false);
                    }
                } else {
                    fallback.insert(command.clone());
                }
            }
        }
        let mut fallback: Vec<_> = fallback
            .into_iter()
            .filter(|command| !recorded.contains(command))
            .collect();
        fallback.sort();
        if !fallback.is_empty() && !self.commands.queue_command_history(fallback) {
            self.settings
                .update_store_status("command history worker is unavailable", false);
        }
    }

    pub(in crate::features) fn queue_quick_command_use_count(&mut self, command_id: String) {
        if !self.commands.queue_quick_command_use_count(command_id) {
            self.settings
                .update_store_status("command persistence worker is unavailable", false);
        }
    }

    /// Deliver command-history and quick-command persistence results as they
    /// arrive.
    ///
    /// Started once at window open. Before this the runtime tick polled
    /// `poll_persistence`, which meant a result waited for the next tick and
    /// forced `runtime_quiet_tick_allowed` to carry a `commands` term to keep
    /// that wait short.
    ///
    /// The stream ending means the worker thread dropped its sender, which is
    /// terminal: no further result can arrive, so report it once if anything was
    /// still outstanding.
    pub(in crate::features) fn start_command_persistence_event_drain(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(mut rx) = self.commands.take_persistence_event_receiver() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                if this
                    .update(cx, |this, cx| {
                        this.commands.note_persistence_event_delivered();
                        if let Err(message) = this.commands.apply_persistence_result(event) {
                            this.settings.update_store_status(message, false);
                        } else {
                            this.request_shared_state_refresh(
                                crate::app_shell::SharedStateDomain::Commands,
                                cx,
                            );
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                if this.commands.note_persistence_worker_disconnected() {
                    this.settings
                        .update_store_status("command persistence worker disconnected", false);
                    cx.notify();
                }
            });
        })
        .detach();
    }
}
