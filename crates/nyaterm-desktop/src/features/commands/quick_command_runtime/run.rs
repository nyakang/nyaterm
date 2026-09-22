use gpui::Context;
use nyaterm_core::{AiCommandCard, AppendAiAuditRequest, QuickCommand, QuickCommandCategory, uuid};
use nyaterm_store::{StoreDomain, store_request};

use crate::features::NyaTermApp;
use crate::models::QuickCommandVariablePromptState;

use super::helpers::{ai_command_card_category_name, unique_quick_command_category_id};
use super::variables::parse_quick_command_variables;

impl NyaTermApp {
    pub(in crate::features) fn save_ai_command_card(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(card) = self.ai.command_card(index) else {
            self.ai
                .set_panel_status("AI command card is no longer available");
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        };
        self.save_ai_command_card_value(card, cx);
    }

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
            store_request(StoreDomain::Commands, move |store| {
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
        let command_text = command.command.trim().to_string();
        if command_text.is_empty() {
            self.shell
                .set_status("quick command has no command text".to_string());
            cx.notify();
            return;
        }
        let execute = quick_command_executes(&command);
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
        mut command_text: String,
        execute: bool,
        send_to_all: bool,
        cx: &mut Context<Self>,
    ) {
        if execute && !command_text.ends_with('\r') && !command_text.ends_with('\n') {
            command_text.push('\r');
        }
        let command_bytes = command_text.into_bytes();
        self.queue_quick_command_use_count(command_id);

        if send_to_all {
            let sessions = self
                .session
                .ordered_sessions()
                .into_iter()
                .filter(|session| !self.session.is_disconnected(&session.id))
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
                match self.write_session_input_recorded(&session.id, &command_bytes) {
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

        if self.send_terminal_input(command_bytes, cx) {
            self.shell.set_status(if execute {
                format!("ran quick command '{label}'")
            } else {
                format!("inserted quick command '{label}'")
            });
            cx.notify();
        }
    }
}

fn quick_command_executes(command: &QuickCommand) -> bool {
    command.execution_mode.as_deref() != Some("append")
}

#[cfg(test)]
mod tests {
    use nyaterm_core::QuickCommand;

    use super::quick_command_executes;

    fn command_with_execution_mode(execution_mode: Option<&str>) -> QuickCommand {
        QuickCommand {
            id: "command-1".to_string(),
            label: "Command".to_string(),
            command: "pwd".to_string(),
            category_id: None,
            description: None,
            color_tag: None,
            icon_tag: None,
            pinned: None,
            execution_mode: execution_mode.map(ToOwned::to_owned),
            source: None,
            risk_level: None,
            updated_at: None,
            created_at: None,
            use_count: None,
            sort_order: None,
        }
    }

    #[test]
    fn append_mode_inserts_without_execution() {
        assert!(!quick_command_executes(&command_with_execution_mode(Some(
            "append"
        ))));
    }

    #[test]
    fn execute_and_legacy_modes_execute() {
        assert!(quick_command_executes(&command_with_execution_mode(Some(
            "execute"
        ))));
        assert!(quick_command_executes(&command_with_execution_mode(None)));
    }
}
