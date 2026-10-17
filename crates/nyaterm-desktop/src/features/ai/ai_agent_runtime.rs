use std::time::Duration;

use gpui::Context;
use nyaterm_core::{
    AgentApprovalOutcome, AiAction, AiChatRequest, AiCommandCard, AiExecutionProfile, AiMode,
    AppendAiAuditRequest, CommandObservation, build_observation_message, uuid,
};
use nyaterm_store::StoreDomain;
use nyaterm_transport::SessionKind;

use crate::features::{
    NyaTermApp, runtime_jobs::AiAgentLoopState, runtime_jobs::AiAgentStepStatus,
    runtime_jobs::AiChatJobResult, runtime_jobs::AiChatWorkerEvent,
    runtime_jobs::await_blocking_job,
};

use super::ai_jobs::{AiJobRunOptions, observation_summary, run_ai_ask_job};
use super::presentation::AiAgentStepKind;

impl NyaTermApp {
    pub(in crate::features) fn upsert_ai_agent_step(
        &mut self,
        step_index: u16,
        status: AiAgentStepStatus,
        kind: AiAgentStepKind,
        title: impl Into<String>,
        detail: impl Into<String>,
    ) {
        self.ai
            .upsert_agent_step(step_index, status, kind, title, detail);
    }

    pub(in crate::features) fn toggle_ai_agent_thought_expanded(
        &mut self,
        step_index: u16,
        cx: &mut Context<Self>,
    ) {
        self.ai.toggle_agent_thought_expanded(step_index);
        self.defer_ai_panel_snapshot_flush(cx);
    }

    pub(in crate::features) fn toggle_ai_agent_output_expanded(
        &mut self,
        step_index: u16,
        cx: &mut Context<Self>,
    ) {
        self.ai.toggle_agent_output_expanded(step_index);
        self.defer_ai_panel_snapshot_flush(cx);
    }

    pub(in crate::features) fn record_ai_command_card_audit(
        &mut self,
        card: &AiCommandCard,
        execute: bool,
        inserted_to_terminal: bool,
        cx: &mut Context<Self>,
    ) {
        if self
            .ai
            .native_pending_call()
            .is_some_and(|call| call.id == card.id)
        {
            return;
        }
        let target_session_id = card.target_terminal_session_id.as_deref().or_else(|| {
            card.target
                .as_ref()
                .map(|target| target.terminal_session_id.as_str())
        });
        let request = AppendAiAuditRequest {
            connection_id: target_session_id
                .and_then(|id| self.session.metadata(id))
                .and_then(|metadata| metadata.source_connection_id.clone()),
            action: if execute {
                "ai.command_card_run".to_string()
            } else {
                "ai.command_card_insert".to_string()
            },
            user_input: Some(self.ai.chat_response_preview().to_string()),
            generated_command: Some(card.command.clone()),
            risk_level: card.risk_level.clone(),
            inserted_to_terminal,
            executed: execute,
            blocked: false,
            ..Default::default()
        };
        self.submit_ai_command_audit(request, cx);
    }

    fn submit_ai_command_audit(&mut self, request: AppendAiAuditRequest, cx: &mut Context<Self>) {
        let store = self.store_blocking_client();
        let write_lock = self.ai.history_audit_write_lock();
        let task = self.blocking_jobs.submit_task("ai-audit-save", move |_| {
            let _guard = write_lock
                .lock()
                .map_err(|_| "AI audit write lock poisoned".to_string())?;
            store
                .request_fn(StoreDomain::Ai, move |database| {
                    database.append_ai_audit(request)
                })
                .map(|_| ())
                .map_err(|error| error.to_string())
        });
        cx.spawn(async move |this, cx| {
            if let Err(error) = await_blocking_job(task).await.and_then(|result| result) {
                let _ = this.update(cx, |this, cx| {
                    this.settings
                        .update_store_status(format!("AI audit save failed: {error}"), false);
                    this.request_settings_panel_refresh(cx);
                    this.defer_ai_panel_snapshot_flush(cx);
                });
            } else {
                let _ = this.update(cx, |this, cx| {
                    this.refresh_ai_usage_counts(cx);
                });
            }
        })
        .detach();
    }

    pub(in crate::features) fn begin_ai_agent_execution(
        &mut self,
        card: &AiCommandCard,
        terminal_session_id: &str,
        cx: &mut Context<Self>,
    ) -> Result<bool, String> {
        use crate::features::capability_runtime::terminal_execution::{
            TerminalExecutionPolicy, TerminalExecutionRequest,
        };
        if self
            .mcp
            .capabilities
            .terminal_executions
            .busy(terminal_session_id)
        {
            return Err("Another automatic command is using this terminal".into());
        }
        let (_, step_index) = self
            .ai
            .begin_agent_step(self.ai.settings_max_agent_steps())?;
        let timeout = Duration::from_millis(
            self.ai
                .settings_config()
                .agent_step_timeout_ms
                .unwrap_or(30_000),
        );
        let ai_session_id = self.ai.chat_session_id().to_string();
        let execution_id = format!("agent-{}", uuid());
        let state = AiAgentLoopState {
            execution_id: execution_id.clone(),
            command_card_id: Some(card.id.clone()),
            ai_session_id: ai_session_id.clone(),
            terminal_session_id: terminal_session_id.into(),
            available_targets: self
                .ai_terminal_targets_for_sessions(&self.ai_effective_target_session_ids()),
            default_target_session_id: Some(terminal_session_id.into()),
            command: card.command.clone(),
            step_index,
            max_steps: self.ai.settings_max_agent_steps(),
        };
        self.ai.set_agent_loop(state);
        let (reply, result) = tokio::sync::oneshot::channel();
        let policy = TerminalExecutionPolicy {
            background: self.ai.settings_config().agent_background_execution_enabled,
            max_lines: self.ai.settings_config().terminal_output_lines,
        };
        let session_id = terminal_session_id.to_string();
        let callback_id = execution_id.clone();
        cx.spawn(async move |this, cx| {
            let result = result.await;
            let _ = this.update(cx, |this, cx| {
                let Some(scope) = this.ai.scope_for_ai_session(&ai_session_id) else { return; };
                let visible = this.ai.active_scope_key().to_string();
                this.ai.switch_scope(&scope);
                if this.ai.agent_loop_snapshot().is_some_and(|state| state.execution_id == callback_id)
                    && let Some(state) = this.ai.take_agent_loop_for_session(&session_id) {
                    match result {
                        Ok(Ok(value)) => {
                            let value: nyaterm_mcp_protocol::TerminalExecuteResult = serde_json::from_value(value).expect("terminal execution result");
                            let observation = CommandObservation { output: value.output, exit_code: value.exit_code, duration_ms: value.duration_ms };
                            this.ai.record_agent_observation(state.step_index, &observation, observation_summary(&observation));
                            this.start_ai_agent_continuation(state, observation, cx);
                        }
                        _ => this.start_ai_agent_continuation_with_message(state, "Terminal execution failed or was cancelled; do not assume the command completed.".into(), cx),
                    }
                }
                this.ai.switch_scope(&visible);
                this.defer_ai_panel_snapshot_flush(cx);
            });
        }).detach();
        self.start_terminal_execution(
            TerminalExecutionRequest {
                id: execution_id.clone(),
                owner: format!("ai-session:{}", self.ai.chat_session_id()),
                source: "NyaTerm Agent".into(),
                track_insertion: true,
                session_id: terminal_session_id.into(),
                command: card.command.clone(),
                timeout,
                cancellation: tokio_util::sync::CancellationToken::new(),
                policy,
                reply,
            },
            cx,
        );
        Ok(self
            .mcp
            .capabilities
            .terminal_executions
            .take_inserted(&execution_id))
    }

    pub(in crate::features) fn ai_execution_profile_for_session(
        &self,
        session_id: &str,
    ) -> AiExecutionProfile {
        let configured = if self.session.active_id() == Some(session_id) {
            self.session.active_ai_execution_profile()
        } else {
            self.session
                .metadata(session_id)
                .map(|metadata| metadata.ai_execution_profile)
                .unwrap_or(AiExecutionProfile::SendOnly)
        };
        if configured != AiExecutionProfile::Auto {
            return configured;
        }
        self.session
            .session_info(session_id)
            .filter(|_| !self.session.is_disconnected(session_id))
            .map(|session| match session.kind {
                // An SSH session can land on a network-device CLI instead of a
                // shell. Those devices do not run the marker wrapper, so the
                // agent must send the bare command and fall back to quiet-output
                // completion; otherwise every command times out waiting for
                // markers that never appear (issue #910).
                SessionKind::Ssh if self.ssh_session_looks_like_network_cli(session_id) => {
                    AiExecutionProfile::SendOnly
                }
                SessionKind::LocalPty
                | SessionKind::Ssh
                | SessionKind::Telnet
                | SessionKind::RawTcp => AiExecutionProfile::Posix,
                SessionKind::Serial | SessionKind::Rdp | SessionKind::Vnc => {
                    AiExecutionProfile::SendOnly
                }
            })
            .unwrap_or(AiExecutionProfile::SendOnly)
    }

    /// Whether the most recent output of an SSH session ends in a network
    /// device CLI prompt. Anything unrecognised keeps the shell profile.
    ///
    /// `allow(dead_code)`: the lib-test build only reaches this through the
    /// agent entry point, and the lint job compiles that target with
    /// `-D warnings`.
    #[allow(dead_code)]
    fn ssh_session_looks_like_network_cli(&self, session_id: &str) -> bool {
        nyaterm_core::network_cli_prompt::looks_like_network_cli_prompt(
            &self.terminal_buffer_text_for_session(session_id),
        )
    }

    pub(in crate::features) fn start_ai_agent_continuation(
        &mut self,
        state: AiAgentLoopState,
        observation: CommandObservation,
        cx: &mut Context<Self>,
    ) {
        let observation_message = build_observation_message(
            &observation,
            &state.command,
            &self.settings.summary().language,
        );
        self.start_ai_agent_continuation_with_message(state, observation_message, cx);
    }

    pub(in crate::features) fn start_ai_agent_continuation_with_message(
        &mut self,
        state: AiAgentLoopState,
        observation_message: String,
        cx: &mut Context<Self>,
    ) {
        let conversation = self.ai.agent_conversation_snapshot();
        let Some(launch) = self
            .ai
            .begin_agent_continuation(&state, &observation_message)
        else {
            return;
        };
        let settings = self.ai.settings_config_cloned();
        let terminal_session_id = state.terminal_session_id.clone();
        let owner_terminal_id = self
            .ai
            .active_scope_key()
            .strip_prefix("terminal:")
            .map(str::to_string);
        let owner_connection_id = owner_terminal_id
            .as_deref()
            .and_then(|id| self.session.metadata(id))
            .and_then(|metadata| metadata.source_connection_id.clone());
        let context = self.ai_terminal_context_for_session(Some(&terminal_session_id));
        let targets = state.available_targets.clone();
        let target_contexts = targets
            .iter()
            .cloned()
            .map(|target| nyaterm_core::AiTargetContext {
                context: self.ai_terminal_context_for_session(Some(&target.terminal_session_id)),
                target: Some(target),
            })
            .collect();
        let request = AiChatRequest {
            stream_id: None,
            session_id: Some(state.ai_session_id.clone()),
            connection_id: owner_connection_id.clone(),
            terminal_session_id: Some(terminal_session_id.clone()),
            owner_scope: nyaterm_core::AiSessionScope {
                r#type: nyaterm_core::AiSessionScopeType::Terminal,
                target_id: owner_terminal_id.clone(),
                connection_ids: owner_connection_id.into_iter().collect(),
                label: owner_terminal_id
                    .as_deref()
                    .and_then(|id| self.session.display_name(id)),
            },
            targets,
            target_contexts,
            mode: AiMode::Agent,
            agent_kind: self.ai.chat_agent_kind(),
            permission_mode: settings.external_agent_permission_mode.clone(),
            model_id: self.ai_selected_model_id(),
            model_name: None,
            default_target_session_id: state.default_target_session_id.clone(),
            existing_external_session_id: None,
            attachments: Vec::new(),
            action: AiAction::GenerateCommand,
            user_input: observation_message,
            context,
            options: nyaterm_core::AiRequestOptions {
                history_turns: state.max_steps.saturating_mul(2).saturating_add(2),
                agent_json_protocol: self.ai.agent_uses_json_protocol(),
                ..Default::default()
            },
        };
        let store = self.store_blocking_client();
        let tx = launch.tx;
        let session_id = launch.session_id;
        let job_id = launch.job_id;
        let cancel = launch.cancel;
        let rejected_tx = tx.clone();
        let rejected_session_id = session_id.clone();
        if let Err(error) =
            self.blocking_jobs
                .submit_detached("ai-agent-continuation", move |scheduler_cancel| {
                    let result = if scheduler_cancel.is_cancelled() {
                        Err("AI Agent continuation cancelled".to_string())
                    } else {
                        run_ai_ask_job(
                            store,
                            settings,
                            request,
                            AiJobRunOptions {
                                mcp_credential: None,
                                stream_tx: Some(tx.clone()),
                                cancel,
                                job_id,
                                agent_history: Some(conversation),
                                persist_user_message: false,
                            },
                        )
                    };
                    let _ =
                        tx.unbounded_send(AiChatWorkerEvent::Finished(Box::new(AiChatJobResult {
                            job_id,
                            session_id,
                            result,
                        })));
                })
        {
            let _ = rejected_tx.unbounded_send(AiChatWorkerEvent::Finished(Box::new(
                AiChatJobResult {
                    job_id,
                    session_id: rejected_session_id,
                    result: Err(error.to_string()),
                },
            )));
        }
        self.defer_ai_panel_snapshot_flush(cx);
    }

    pub(in crate::features) fn reject_ai_agent_command_card_by_id(
        &mut self,
        card_id: String,
        cx: &mut Context<Self>,
    ) {
        if self
            .ai
            .native_pending_call()
            .is_some_and(|call| call.id == card_id)
        {
            self.respond_to_mcp_approval(
                &format!(
                    "{}:{card_id}",
                    self.ai.native_owner().expect("native owner")
                ),
                crate::features::mcp::McpApprovalDecision::Deny,
                cx,
            );
            return;
        }
        if let Some(card) = self.ai.find_command_card(&card_id) {
            self.reject_ai_agent_command_card_value(card, cx);
        }
    }

    fn reject_ai_agent_command_card_value(&mut self, card: AiCommandCard, cx: &mut Context<Self>) {
        if !super::ai_jobs::is_agent_command_card(&card)
            || !self.ai.current_agent_command_card(&card.id)
            || !self
                .ai
                .chat_command_cards()
                .iter()
                .any(|current| current.id == card.id)
            || self.ai.chat_is_pending()
            || self.ai.agent_loop_snapshot().is_some()
        {
            return;
        }
        let Some(terminal_session_id) = card.target_terminal_session_id.clone() else {
            return;
        };
        let max_steps = self.ai.settings_max_agent_steps();
        let Ok((_, step_index)) = self.ai.begin_agent_step(max_steps) else {
            self.ai
                .set_panel_status("AI Agent reached its step limit".to_string());
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        };
        self.ai.upsert_agent_step(
            step_index,
            AiAgentStepStatus::Rejected,
            AiAgentStepKind::Diagnostic,
            "Rejected",
            "User denied command execution",
        );
        self.ai.associate_agent_command(step_index, &card.id);
        self.ai.clear_chat_command_cards();
        self.submit_ai_command_audit(
            AppendAiAuditRequest {
                connection_id: self
                    .session
                    .metadata(&terminal_session_id)
                    .and_then(|metadata| metadata.source_connection_id.clone()),
                action: "ai.agent_reject_execute".to_string(),
                generated_command: Some(card.command.clone()),
                risk_level: card.risk_level.clone(),
                blocked: true,
                session_id: Some(self.ai.chat_session_id().to_string()),
                approval_decision: Some(AgentApprovalOutcome::Rejected.as_str().to_string()),
                ..Default::default()
            },
            cx,
        );
        let target_ids = self.ai_effective_target_session_ids();
        let state = AiAgentLoopState {
            execution_id: format!("rejected-{}", uuid()),
            command_card_id: Some(card.id.clone()),
            ai_session_id: self.ai.chat_session_id().to_string(),
            terminal_session_id: terminal_session_id.clone(),
            available_targets: self.ai_terminal_targets_for_sessions(&target_ids),
            default_target_session_id: Some(terminal_session_id),
            command: card.command.clone(),

            step_index,
            max_steps,
        };
        self.start_ai_agent_continuation_with_message(
            state,
            format!("The user rejected command `{}`. It was not executed. Choose a safe alternative or finish the task.", card.command),
            cx,
        );
    }
}
