use gpui::Context;
use nyaterm_core::{AiCommandCard, AppendAiAuditRequest, QuickCommand, QuickCommandCategory, uuid};
use nyaterm_store::StoreDomain;

use crate::features::NyaTermApp;
use crate::features::commands::staged::StagedCommandHistory;
use crate::models::QuickCommandVariablePromptState;

use super::escapes::{
    QuickCommandChunk, plan_literal_quick_command, plan_quick_command, quick_command_needs_staging,
};
use super::helpers::{
    ai_command_card_category_name, unique_quick_command_category_id, validated_quick_command_text,
};
use super::variables::parse_quick_command_variables;

struct QuickCommandSendPlan {
    chunks: Vec<QuickCommandChunk>,
    literal: bool,
}

struct StagedQuickCommandTarget {
    session_id: String,
    source_session_id: Option<String>,
    history: StagedCommandHistory,
}

impl NyaTermApp {
    pub(in crate::features) fn save_ai_command_card_by_id(
        &mut self,
        card_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(card) = self.find_ai_command_card(&card_id) else {
            self.ai
                .set_panel_status("AI command card is no longer available");
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        };
        self.save_ai_command_card_value(card, cx);
    }

    pub(in crate::features) fn save_ai_command_card_value(
        &mut self,
        card: AiCommandCard,
        cx: &mut Context<Self>,
    ) {
        let command_text = card.command.trim().to_string();
        if command_text.is_empty() {
            self.ai.set_panel_status("AI command card has no command");
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        }

        let connection_id = self.session.active_id_owned();
        let response_preview = self.ai.chat_response_preview().to_string();
        self.submit_store_request(
            0,
            nyaterm_store::store_mutation(StoreDomain::Commands, move |store| {
                let config = store.load_quick_commands()?;
                let category_name = ai_command_card_category_name(&card);
                let existing_category = config
                    .categories
                    .iter()
                    .find(|category| category.name == category_name)
                    .cloned();
                let (category_id, new_category) = match existing_category {
                    Some(category) => (category.id, None),
                    None => {
                        let id =
                            unique_quick_command_category_id(&config.categories, &category_name);
                        (
                            id.clone(),
                            Some(QuickCommandCategory {
                                id,
                                name: category_name,
                                parent_id: None,
                                sort_order: config
                                    .categories
                                    .iter()
                                    .filter(|category| category.parent_id.is_none())
                                    .map(|category| category.sort_order)
                                    .max()
                                    .unwrap_or(-1)
                                    .saturating_add(1),
                            }),
                        )
                    }
                };
                let label = if card.title.trim().is_empty() {
                    "AI Command".to_string()
                } else {
                    card.title.trim().to_string()
                };
                let description = if card.explanation.trim().is_empty() {
                    None
                } else {
                    Some(card.explanation.trim().to_string())
                };
                store.upsert_quick_command(
                    QuickCommand {
                        id: format!("ai-{}", uuid()),
                        label: label.clone(),
                        command: command_text,
                        category_id: Some(category_id),
                        description,
                        color_tag: Some("blue".to_string()),
                        icon_tag: Some("terminal".to_string()),
                        pinned: Some(false),
                        execution_mode: Some("append".to_string()),
                        raw_send: None,
                        source: Some("ai".to_string()),
                        risk_level: card.risk_level.clone(),
                        updated_at: None,
                        created_at: None,
                        use_count: None,
                        sort_order: None,
                    },
                    new_category,
                )?;
                store
                    .append_ai_audit(AppendAiAuditRequest {
                        connection_id,
                        action: "ai.save_quick_command".to_string(),
                        user_input: Some(response_preview),
                        generated_command: Some(card.command.clone()),
                        risk_level: card.risk_level.clone(),
                        inserted_to_terminal: false,
                        executed: false,
                        blocked: false,
                        ..Default::default()
                    })
                    .map(|_| label)
            }),
            |this, event, cx| {
                match event.outcome {
                    Ok(label) => {
                        this.refresh_ai_usage_counts(cx);
                        this.refresh_quick_commands(cx);
                        this.ai.set_panel_status(format!(
                            "Saved AI command card '{}' to Quick Commands",
                            label
                        ));
                        this.settings
                            .update_store_status(this.ai.panel_status().to_string(), true);
                    }
                    Err(error) => {
                        this.ai
                            .set_panel_status(format!("Quick command save failed: {error}"));
                        this.settings
                            .update_store_status(this.ai.panel_status().to_string(), false);
                    }
                }
                this.request_settings_panel_refresh(cx);
                this.defer_ai_panel_snapshot_flush(cx);
            },
            cx,
        );
    }

    pub(in crate::features) fn run_quick_command_by_id(
        &mut self,
        command_id: String,
        cx: &mut Context<Self>,
    ) {
        self.apply_quick_command_by_id(&command_id, false, cx);
    }

    pub(in crate::features) fn send_quick_command_to_all_by_id(
        &mut self,
        command_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(command) = self
            .commands
            .quick_commands()
            .iter()
            .find(|command| command.id == command_id)
            .cloned()
        else {
            self.shell
                .set_status("quick command is no longer available".to_string());
            cx.notify();
            return;
        };
        self.apply_quick_command_by_id(&command.id, true, cx);
    }

    pub(in crate::features) fn apply_quick_command_by_id(
        &mut self,
        command_id: &str,
        send_to_all: bool,
        cx: &mut Context<Self>,
    ) {
        if send_to_all {
            if self.session.live_session_count() == 0 {
                self.shell.set_status(
                    "start a terminal session before using a quick command".to_string(),
                );
                cx.notify();
                return;
            }
        } else if self.session.active_id().is_none() {
            self.shell
                .set_status("start a terminal session before using a quick command".to_string());
            cx.notify();
            return;
        }
        let Some(command) = self
            .commands
            .quick_commands()
            .iter()
            .find(|command| command.id == command_id)
            .cloned()
        else {
            self.shell
                .set_status("quick command is no longer available".to_string());
            cx.notify();
            return;
        };
        let Some(command_text) = validated_quick_command_text(&command.command) else {
            self.shell
                .set_status("quick command has no command text".to_string());
            cx.notify();
            return;
        };
        let execute = quick_command_executes(&command);
        if command.raw_send.unwrap_or_default() {
            self.send_quick_command_plan(
                command.id,
                command.label,
                QuickCommandSendPlan {
                    chunks: plan_literal_quick_command(&command_text, execute),
                    literal: true,
                },
                execute,
                send_to_all,
                cx,
            );
            return;
        }
        let variables = parse_quick_command_variables(&command_text);
        if !variables.is_empty() {
            self.commands
                .request_quick_variable_prompt(QuickCommandVariablePromptState {
                    command_id: command.id,
                    label: command.label,
                    command: command_text,
                    execute,
                    send_to_all,
                    variables,
                });
            self.shell
                .set_status("fill quick command variables".to_string());
            cx.notify();
            return;
        }
        self.send_resolved_quick_command(
            command.id,
            command.label,
            command_text,
            execute,
            send_to_all,
            cx,
        );
    }

    pub(in crate::features) fn send_resolved_quick_command(
        &mut self,
        command_id: String,
        label: String,
        command_text: String,
        execute: bool,
        send_to_all: bool,
        cx: &mut Context<Self>,
    ) {
        // `\r \n \t \p \\` are interpreted here rather than at save time: the
        // stored script keeps the escapes, so what the panel shows is what the
        // user wrote.
        self.send_quick_command_plan(
            command_id,
            label,
            QuickCommandSendPlan {
                chunks: plan_quick_command(&command_text, execute),
                literal: false,
            },
            execute,
            send_to_all,
            cx,
        );
    }

    fn send_quick_command_plan(
        &mut self,
        command_id: String,
        label: String,
        plan: QuickCommandSendPlan,
        execute: bool,
        send_to_all: bool,
        cx: &mut Context<Self>,
    ) {
        let QuickCommandSendPlan {
            mut chunks,
            literal,
        } = plan;
        self.queue_quick_command_use_count(command_id);

        if quick_command_needs_staging(&chunks) {
            self.send_staged_quick_command(label, chunks, execute, send_to_all, cx);
            return;
        }

        // The common case is one chunk, which goes through the ordinary input
        // path exactly as before. The staging gate above already excluded any
        // plan that carries a pause, so this chunk's delay is zero.
        let Some(chunk) = chunks.pop() else {
            return;
        };
        let command_bytes = chunk.bytes;

        if send_to_all {
            let sessions = self
                .session
                .ordered_sessions()
                .into_iter()
                .filter(|session| self.sync_target_allowed(&session.id))
                .collect::<Vec<_>>();
            if sessions.is_empty() {
                self.shell.set_status(
                    "start a terminal session before using a quick command".to_string(),
                );
                cx.notify();
                return;
            }
            let mut sent = 0usize;
            let mut failed = 0usize;
            let mut ok_sessions = Vec::new();
            for session in sessions {
                let result = if literal {
                    self.write_session_literal_input_recorded(&session.id, &command_bytes)
                } else {
                    self.write_session_input_recorded(&session.id, &command_bytes)
                };
                match result {
                    Ok(()) => {
                        sent += 1;
                        ok_sessions.push(session.id);
                    }
                    Err(_) => failed += 1,
                }
            }
            let session_refs: Vec<&str> = ok_sessions.iter().map(String::as_str).collect();
            self.record_command_history_for_sessions(&session_refs, &command_bytes);
            self.shell.set_status(if failed == 0 {
                format!("sent quick command '{label}' to {sent} session(s)")
            } else {
                format!("sent quick command '{label}' to {sent} session(s), {failed} failed")
            });
            cx.notify();
            return;
        }

        let sent = if literal {
            self.send_terminal_literal_input(command_bytes, cx)
        } else {
            self.send_terminal_input(command_bytes, cx)
        };
        if sent {
            self.shell.set_status(if execute {
                format!("ran quick command '{label}'")
            } else {
                format!("inserted quick command '{label}'")
            });
            cx.notify();
        }
    }

    /// Send a script that `\p` split into several writes.
    ///
    /// The target sessions are pinned once, here, when the user runs the
    /// command: the rest of the script must keep arriving at what was clicked
    /// on rather than following the focus during the pauses.
    fn send_staged_quick_command(
        &mut self,
        label: String,
        chunks: Vec<QuickCommandChunk>,
        execute: bool,
        send_to_all: bool,
        cx: &mut Context<Self>,
    ) {
        if !send_to_all && self.active_terminal_input_blocked() {
            return;
        }
        let source_session_id = (!send_to_all)
            .then(|| self.session.active_id_owned())
            .flatten();
        let targets = if send_to_all {
            self.session
                .ordered_sessions()
                .into_iter()
                .filter(|session| self.sync_target_allowed(&session.id))
                .map(|session| session.id)
                .collect::<Vec<_>>()
        } else {
            let primary = self.session.active_id_owned().filter(|session_id| {
                !self.session.is_disconnected(session_id) && self.terminal_sync_session(session_id)
            });
            match primary {
                Some(session_id) => {
                    let peers = self.sync_peer_session_ids(&session_id);
                    std::iter::once(session_id).chain(peers).collect()
                }
                None => Vec::new(),
            }
        };
        if targets.is_empty() {
            self.shell
                .set_status("start a terminal session before using a quick command".to_string());
            cx.notify();
            return;
        }

        let run_id = self.commands.staged.begin(&targets);
        for id in &targets {
            if source_session_id.as_deref() != Some(id.as_str()) {
                self.commands.staged.mark_receiver(id, run_id);
            }
        }
        let targets = targets
            .into_iter()
            .map(|session_id| StagedQuickCommandTarget {
                source_session_id: source_session_id.clone(),
                session_id,
                history: StagedCommandHistory::default(),
            })
            .collect::<Vec<_>>();
        let steps = chunks.len();
        self.shell
            .set_status(format!("sending quick command '{label}' in {steps} steps"));
        cx.notify();

        cx.spawn(async move |this, cx| {
            let mut live = targets;
            let total = live.len();
            let mut cancelled = false;
            for chunk in chunks {
                if !chunk.delay.is_zero() {
                    cx.background_executor().timer(chunk.delay).await;
                }
                let Ok((survivors, interrupted)) = this.update(cx, |this, cx| {
                    let survivors = this.write_quick_command_chunk(live, run_id, &chunk.bytes, cx);
                    cx.notify();
                    survivors
                }) else {
                    // The window went away, so there is no one left to report to.
                    return;
                };
                cancelled |= interrupted;
                live = survivors;
                if live.is_empty() {
                    break;
                }
            }
            let failed = total - live.len();
            let delivered = live.len();
            let _ = this.update(cx, |this, cx| {
                for target in &live {
                    this.commands.staged.finish(&target.session_id, run_id);
                }
                // A superseded task must not replace the newer input's status.
                if cancelled {
                    return;
                }
                this.shell.set_status(quick_command_delivery_status(
                    &label,
                    execute,
                    send_to_all,
                    delivered,
                    failed,
                ));
                cx.notify();
            });
        })
        .detach();
    }

    /// Keep history and liveness per pinned target; the active editor may now
    /// belong to an unrelated tab and must never supply this script's history.
    fn write_quick_command_chunk(
        &mut self,
        targets: Vec<StagedQuickCommandTarget>,
        run_id: u64,
        bytes: &[u8],
        cx: &mut Context<Self>,
    ) -> (Vec<StagedQuickCommandTarget>, bool) {
        let mut survivors = Vec::with_capacity(targets.len());
        let mut cancelled = false;
        let mut submissions: Vec<(Vec<String>, Vec<String>)> = Vec::new();
        for mut target in targets {
            let session_id = &target.session_id;
            if !self.commands.staged.owns(session_id, run_id) {
                cancelled = true;
                continue;
            }
            let allowed = match target.source_session_id.as_deref() {
                Some(source) if source == session_id => self.terminal_sync_session(session_id),
                Some(source) => self.sync_peer_session_ids(source).contains(session_id),
                None => self.sync_target_allowed(session_id),
            };
            if !allowed {
                self.commands.staged.finish(session_id, run_id);
                cancelled = true;
                continue;
            }
            let sensitive = self.recording_input_is_sensitive(session_id);
            if self.session.is_disconnected(session_id)
                || self
                    .write_staged_quick_command_input(session_id, bytes, run_id)
                    .is_err()
            {
                self.commands.staged.finish(session_id, run_id);
                continue;
            }
            let submitted = target.history.push(bytes, sensitive);
            if !submitted.is_empty() {
                if let Some((_, sessions)) = submissions
                    .iter_mut()
                    .find(|(lines, _)| *lines == submitted)
                {
                    sessions.push(session_id.clone());
                } else {
                    submissions.push((submitted, vec![session_id.clone()]));
                }
            }
            survivors.push(target);
        }
        for (submitted, sessions) in submissions {
            let session_refs = sessions.iter().map(String::as_str).collect::<Vec<_>>();
            self.record_submitted_command_history(&session_refs, submitted);
        }
        if !survivors.is_empty() {
            self.arm_terminal_input_wake(cx);
        }
        (survivors, cancelled)
    }
}

fn quick_command_delivery_status(
    label: &str,
    execute: bool,
    send_to_all: bool,
    delivered: usize,
    failed: usize,
) -> String {
    if send_to_all {
        return if failed == 0 {
            format!("sent quick command '{label}' to {delivered} session(s)")
        } else {
            format!("sent quick command '{label}' to {delivered} session(s), {failed} failed")
        };
    }
    if failed > 0 {
        return format!("quick command '{label}' failed on {failed} session(s)");
    }
    if execute {
        format!("ran quick command '{label}'")
    } else {
        format!("inserted quick command '{label}'")
    }
}

fn quick_command_executes(command: &QuickCommand) -> bool {
    command.execution_mode.as_deref() != Some("append")
}

#[cfg(test)]
mod tests;
