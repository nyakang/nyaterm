use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

use futures::channel::mpsc::UnboundedSender;

use nyaterm_core::{
    AgentApprovalDecision, AiAgentKind, AiBackendKind, AiChatCompletion, AiChatRequest,
    AiCommandCard, AiMessage, AiMessageRole, AiMode, AiSessionBackendMetadata, AiSettings,
    AppendAiAuditRequest, CommandObservation, agent_response_action, assess_agent_command_risk,
    bind_command_card_targets, decide_agent_command_execution, now_rfc3339,
    parse_agent_model_output, parse_agent_tool_call, parse_model_output, redact_context,
    redact_sensitive_text, resolve_ai_terminal_target, sanitize_ai_diagnostic, truncate_preview,
    uuid,
};
use nyaterm_store::{ConnectionStore, StoreBlockingClient, StoreDomain};

use crate::features::{
    mcp::McpEphemeralCredential, runtime_jobs::AiChatJobOutput, runtime_jobs::AiChatWorkerEvent,
};

use super::claude_code_runtime::run_claude_code;
use super::codex_runtime::run_codex_app_server;

pub(in crate::features) fn is_agent_command_card(card: &AiCommandCard) -> bool {
    card.id.starts_with("agent-")
        || card
            .category
            .as_deref()
            .is_some_and(|category| category == "AI Agent")
}

pub(in crate::features) struct AiJobRunOptions {
    pub mcp_credential: Option<McpEphemeralCredential>,
    pub stream_tx: Option<UnboundedSender<AiChatWorkerEvent>>,
    pub cancel: Arc<AtomicBool>,
    pub job_id: u64,
    pub agent_history: Option<Vec<AiMessage>>,
    pub persist_user_message: bool,
}

pub(in crate::features) fn run_ai_ask_job(
    store: StoreBlockingClient,
    settings: AiSettings,
    mut request: AiChatRequest,
    run: AiJobRunOptions,
) -> Result<AiChatJobOutput, String> {
    let AiJobRunOptions {
        mcp_credential,
        stream_tx,
        cancel,
        job_id,
        agent_history,
        persist_user_message,
    } = run;
    if ai_job_cancelled(&cancel) {
        return Err("AI request cancelled".to_string());
    }
    if settings.redaction_enabled {
        redact_context(&mut request.context);
        for target_context in &mut request.target_contexts {
            redact_context(&mut target_context.context);
        }
        request.user_input = redact_sensitive_text(&request.user_input);
    }
    let session_id = request
        .session_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("ai-session-{}", uuid()));
    request.session_id = Some(session_id.clone());

    let history = if agent_history.is_none() {
        store
            .request_fn(StoreDomain::Ai, |database| database.load_ai_history())
            .map_err(|error| error.to_string())?
    } else {
        Default::default()
    };
    if settings.record_history && persist_user_message {
        let user_session_id = session_id.clone();
        let connection_id = request.connection_id.clone();
        let user_input = request.user_input.clone();
        let agent_kind = request.agent_kind.clone();
        let owner_scope = request.owner_scope.clone();
        store
            .request_fn(StoreDomain::Ai, move |database| {
                database.append_ai_user_message_scoped(
                    &user_session_id,
                    connection_id,
                    user_input,
                    agent_kind,
                    owner_scope,
                )
            })
            .map_err(|error| error.to_string())?;
    }
    if ai_job_cancelled(&cancel) {
        return Err("AI request cancelled".to_string());
    }

    if request.mode != AiMode::Agent
        && matches!(
            request.agent_kind,
            AiAgentKind::Codex | AiAgentKind::ClaudeCode
        )
    {
        return Err("External agents require Agent mode".to_string());
    }

    if request.agent_kind == AiAgentKind::Codex {
        if settings.codex.tool_integration_mode.as_deref() != Some("nyaterm_mcp") {
            return Err("Codex requires strict NyaTerm MCP tool integration".to_string());
        }
        let persistent = settings.codex.thread_mode == nyaterm_core::CodexThreadMode::Persistent;
        let existing_thread_id = persistent
            .then(|| {
                request
                    .existing_external_session_id
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .or_else(|| {
                        history
                            .sessions
                            .iter()
                            .find(|session| session.id == session_id)
                            .and_then(|session| {
                                session
                                    .backend_metadata
                                    .as_ref()
                                    .filter(|metadata| {
                                        metadata.backend == AiBackendKind::Codex
                                            && metadata.codex_terminal_tools_version
                                                == Some(
                                                    nyaterm_core::ai::CODEX_TERMINAL_TOOLS_VERSION,
                                                )
                                    })
                                    .and_then(|metadata| metadata.external_thread_id.as_deref())
                                    .or_else(|| {
                                        (session.agent_kind == AiAgentKind::Codex)
                                            .then_some(session.external_session_id.as_deref())
                                            .flatten()
                                    })
                            })
                    })
            })
            .flatten();
        let credential = mcp_credential
            .ok_or_else(|| "Codex requires a request-scoped NyaTerm MCP credential".to_string())?;
        let resumed = existing_thread_id.is_some();
        record_external_agent_audit(
            &store,
            &request,
            "codex",
            if resumed { "resume" } else { "start" },
            true,
            None,
            None,
        );
        let started_at = Instant::now();
        let completion = run_codex_app_server(
            &settings,
            &request,
            existing_thread_id,
            stream_tx.as_ref(),
            &cancel,
            job_id,
            &session_id,
            credential,
        );
        let completion = match completion {
            Ok(completion) => completion,
            Err(error) => {
                record_external_agent_audit(
                    &store,
                    &request,
                    "codex",
                    if ai_job_cancelled(&cancel) {
                        "cancel"
                    } else {
                        "error"
                    },
                    false,
                    Some(&error),
                    Some(started_at.elapsed()),
                );
                return Err(error);
            }
        };
        record_external_agent_audit(
            &store,
            &request,
            "codex",
            "complete",
            true,
            None,
            Some(started_at.elapsed()),
        );
        let output = AiChatJobOutput {
            metadata: None,
            native_decision: None,
            mode: AiMode::Agent,
            text: completion.text,
            reasoning: completion.reasoning,
            command_cards: Vec::new(),
            auto_execute_first: false,
            approval_note: None,
        };
        if settings.record_history && persistent {
            let thread_id = completion.thread_id;
            let metadata = AiSessionBackendMetadata {
                backend: AiBackendKind::Codex,
                external_thread_id: Some(thread_id.clone()),
                codex_terminal_tools_version: Some(nyaterm_core::ai::CODEX_TERMINAL_TOOLS_VERSION),
            };
            let metadata_session_id = session_id.clone();
            store
                .request_fn(StoreDomain::Ai, move |database| {
                    database.set_ai_session_external_metadata(
                        &metadata_session_id,
                        AiAgentKind::Codex,
                        thread_id,
                        metadata,
                    )
                })
                .map_err(|error| error.to_string())?;
            let message = AiMessage {
                id: format!("msg-{}", uuid()),
                session_id,
                role: AiMessageRole::Assistant,
                content: output.text.clone(),
                created_at: now_rfc3339(),
                reasoning_content: output.reasoning.clone(),
                command_cards: Vec::new(),
            };
            store
                .request_fn(StoreDomain::Ai, move |database| {
                    database.append_ai_message(message)
                })
                .map_err(|error| error.to_string())?;
        }
        return Ok(output);
    }

    if request.agent_kind == AiAgentKind::ClaudeCode {
        if settings.claude_code.tool_integration_mode.as_deref() != Some("nyaterm_mcp") {
            return Err("Claude Code requires strict NyaTerm MCP tool integration".to_string());
        }
        let existing_session_id = request
            .existing_external_session_id
            .clone()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                history
                    .sessions
                    .iter()
                    .find(|session| session.id == session_id)
                    .and_then(|session| {
                        (session.agent_kind == AiAgentKind::ClaudeCode)
                            .then_some(session.external_session_id.clone())
                            .flatten()
                            .or_else(|| {
                                session
                                    .backend_metadata
                                    .as_ref()
                                    .filter(|metadata| {
                                        metadata.backend == AiBackendKind::ClaudeCode
                                    })
                                    .and_then(|metadata| metadata.external_thread_id.clone())
                            })
                    })
            });
        request.existing_external_session_id = existing_session_id;
        let credential = mcp_credential.ok_or_else(|| {
            "Claude Code requires a request-scoped NyaTerm MCP credential".to_string()
        })?;
        let metadata_store = store.clone();
        let metadata_session_id = session_id.clone();
        let record_history = settings.record_history;
        let resumed = request.existing_external_session_id.is_some();
        record_external_agent_audit(
            &store,
            &request,
            "claude_code",
            if resumed { "resume" } else { "start" },
            true,
            None,
            None,
        );
        let started_at = Instant::now();
        let completion = run_claude_code(
            &settings,
            &request,
            stream_tx.as_ref(),
            &cancel,
            job_id,
            &session_id,
            credential,
            move |external_session_id| {
                if !record_history {
                    return Ok(());
                }
                let external_session_id = external_session_id.to_string();
                let stored_external_session_id = external_session_id.clone();
                let metadata = AiSessionBackendMetadata {
                    backend: AiBackendKind::ClaudeCode,
                    external_thread_id: Some(external_session_id),
                    codex_terminal_tools_version: None,
                };
                let session_id = metadata_session_id.clone();
                metadata_store
                    .request_fn(StoreDomain::Ai, move |database| {
                        database.set_ai_session_external_metadata(
                            &session_id,
                            AiAgentKind::ClaudeCode,
                            stored_external_session_id,
                            metadata,
                        )
                    })
                    .map_err(|error| error.to_string())
            },
        );
        let completion = match completion {
            Ok(completion) => completion,
            Err(error) => {
                record_external_agent_audit(
                    &store,
                    &request,
                    "claude_code",
                    if ai_job_cancelled(&cancel) {
                        "cancel"
                    } else {
                        "error"
                    },
                    false,
                    Some(&error),
                    Some(started_at.elapsed()),
                );
                return Err(error);
            }
        };
        record_external_agent_audit(
            &store,
            &request,
            "claude_code",
            "complete",
            true,
            None,
            Some(started_at.elapsed()),
        );
        let output = AiChatJobOutput {
            metadata: None,
            native_decision: None,
            mode: AiMode::Agent,
            text: completion.text,
            reasoning: None,
            command_cards: Vec::new(),
            auto_execute_first: false,
            approval_note: None,
        };
        if settings.record_history {
            if let Some(external_session_id) = completion.external_session_id {
                let metadata = AiSessionBackendMetadata {
                    backend: AiBackendKind::ClaudeCode,
                    external_thread_id: Some(external_session_id.clone()),
                    codex_terminal_tools_version: None,
                };
                let metadata_session_id = session_id.clone();
                store
                    .request_fn(StoreDomain::Ai, move |database| {
                        database.set_ai_session_external_metadata(
                            &metadata_session_id,
                            AiAgentKind::ClaudeCode,
                            external_session_id,
                            metadata,
                        )
                    })
                    .map_err(|error| error.to_string())?;
            }
            let message = AiMessage {
                id: format!("msg-{}", uuid()),
                session_id,
                role: AiMessageRole::Assistant,
                content: output.text.clone(),
                created_at: now_rfc3339(),
                reasoning_content: output.reasoning.clone(),
                command_cards: Vec::new(),
            };
            store
                .request_fn(StoreDomain::Ai, move |database| {
                    database.append_ai_message(message)
                })
                .map_err(|error| error.to_string())?;
        }
        return Ok(output);
    }

    Err("Native AI requests require the asynchronous runtime".into())
}

pub(super) fn prepare_native_job(
    store: &StoreBlockingClient,
    settings: &AiSettings,
    request: &mut AiChatRequest,
    history: Option<Vec<AiMessage>>,
    persist: bool,
) -> Result<Vec<AiMessage>, String> {
    if settings.redaction_enabled {
        redact_context(&mut request.context);
        for target in &mut request.target_contexts {
            redact_context(&mut target.context);
        }
        request.user_input = redact_sensitive_text(&request.user_input);
    }
    let history = match history {
        Some(history) => history,
        None => {
            store
                .request_fn(StoreDomain::Ai, |database| database.load_ai_history())
                .map_err(|error| error.to_string())?
                .messages
        }
    };
    if settings.record_history && persist {
        let session_id = request.session_id.clone().ok_or("Missing AI session")?;
        let request = request.clone();
        store
            .request_fn(StoreDomain::Ai, move |database| {
                database.append_ai_user_message_scoped(
                    &session_id,
                    request.connection_id,
                    request.user_input,
                    request.agent_kind,
                    request.owner_scope,
                )
            })
            .map_err(|error| error.to_string())?;
    }
    Ok(history)
}

pub(super) fn finish_native_job(
    store: &StoreBlockingClient,
    settings: &AiSettings,
    request: &AiChatRequest,
    completion: AiChatCompletion,
) -> Result<AiChatJobOutput, String> {
    let metadata = completion.metadata.clone();
    let mut output = if request.mode == AiMode::Agent && request.options.agent_context.is_some() {
        let decision = nyaterm_core::ai::harness::AgentToolRegistry::decision(&completion)
            .map_err(|error| error.to_string())?;
        AiChatJobOutput {
            metadata: None,
            native_decision: Some(decision),
            mode: AiMode::Agent,
            text: completion.text,
            reasoning: completion.reasoning_content,
            command_cards: Vec::new(),
            auto_execute_first: false,
            approval_note: None,
        }
    } else if request.mode == AiMode::Agent {
        ai_agent_job_output(
            settings,
            request,
            completion.text,
            completion.reasoning_content,
            completion.tool_calls,
        )?
    } else {
        let (text, reasoning, command_cards) =
            parse_model_output(&completion.text, completion.reasoning_content);
        AiChatJobOutput {
            metadata: None,
            native_decision: None,
            mode: request.mode.clone(),
            text,
            reasoning,
            command_cards,
            auto_execute_first: false,
            approval_note: None,
        }
    };
    output.metadata = Some(metadata);
    bind_command_card_targets(&mut output.command_cards, request);
    if settings.record_history && request.options.agent_context.is_none() {
        let message = AiMessage {
            id: format!("msg-{}", uuid()),
            session_id: request.session_id.clone().ok_or("Missing AI session")?,
            role: AiMessageRole::Assistant,
            content: output.text.clone(),
            created_at: now_rfc3339(),
            reasoning_content: output.reasoning.clone(),
            command_cards: output.command_cards.clone(),
        };
        store
            .request_fn(StoreDomain::Ai, move |database| {
                database.append_ai_message(message)
            })
            .map_err(|error| error.to_string())?;
    }
    Ok(output)
}

pub(in crate::features) fn ai_job_cancelled(cancel: &Arc<AtomicBool>) -> bool {
    cancel.load(Ordering::Relaxed)
}

fn record_external_agent_audit(
    store: &StoreBlockingClient,
    request: &AiChatRequest,
    source: &str,
    event: &str,
    success: bool,
    error: Option<&str>,
    duration: Option<std::time::Duration>,
) {
    let audit = AppendAiAuditRequest {
        connection_id: request.connection_id.clone(),
        action: format!("external_agent.{event}"),
        user_input: None,
        generated_command: None,
        risk_level: None,
        inserted_to_terminal: false,
        executed: false,
        blocked: !success,
        source: Some(source.to_string()),
        client: None,
        capability: Some("nyaterm_mcp".to_string()),
        session_id: request.session_id.clone(),
        permission_mode: Some(request.permission_mode.clone()),
        approval_decision: None,
        success: Some(success),
        duration_ms: duration
            .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)),
        error: error.map(|error| sanitize_ai_diagnostic(error, 256)),
    };
    if let Err(error) = store.request_fn(StoreDomain::Ai, move |database| {
        database.append_ai_audit(audit)
    }) {
        tracing::warn!(error = %error, source, event, "failed to persist external Agent audit");
    }
}

pub(in crate::features) fn observation_summary(observation: &CommandObservation) -> String {
    let status = observation
        .exit_code
        .map(|code| format!("exit {code}"))
        .unwrap_or_else(|| "exit unknown".to_string());
    let preview = truncate_preview(observation.output.trim(), 100);
    if preview.is_empty() {
        format!("{status}; {} ms; no output", observation.duration_ms)
    } else {
        format!("{status}; {} ms; {preview}", observation.duration_ms)
    }
}

fn ai_agent_job_output(
    settings: &AiSettings,
    request: &AiChatRequest,
    text: String,
    reasoning: Option<String>,
    tool_calls: Vec<nyaterm_core::AiToolCall>,
) -> Result<AiChatJobOutput, String> {
    let parsed = if tool_calls.is_empty() {
        parse_agent_model_output(&text).map_err(|error| error.to_string())?
    } else {
        parse_agent_tool_call(&tool_calls)
            .or_else(|_| parse_agent_model_output(&text))
            .map_err(|error| error.to_string())?
    };
    let action = agent_response_action(&parsed);
    if action == "final_answer" {
        let answer = parsed
            .answer
            .as_deref()
            .map(str::trim)
            .filter(|answer| !answer.is_empty())
            .unwrap_or("Agent finished without a final answer")
            .to_string();
        return Ok(AiChatJobOutput {
            metadata: None,
            native_decision: None,
            mode: AiMode::Agent,
            text: answer,
            reasoning: Some(parsed.thought).or(reasoning),
            command_cards: Vec::new(),
            auto_execute_first: false,
            approval_note: None,
        });
    }

    let command = parsed
        .command
        .as_deref()
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .ok_or_else(|| "Agent returned execute_command without a command".to_string())?;
    let target = resolve_ai_terminal_target(request, parsed.target_terminal_session_id.as_deref())
        .map_err(|error| error.to_string())?;
    let assessment = assess_agent_command_risk(&parsed, command);
    let (decision, approval_note) = decide_agent_command_execution(settings, &assessment);
    let explanation = if parsed.thought.trim().is_empty() {
        "Agent requested command execution".to_string()
    } else {
        parsed.thought.trim().to_string()
    };
    let card = AiCommandCard {
        id: format!("agent-{}", uuid()),
        title: "Agent Command".to_string(),
        command: command.to_string(),
        explanation,
        risk_level: Some(assessment.effective_risk),
        risk_reason: assessment.risk_reason,
        expected_effect: "Run the next native Agent step in the active terminal".to_string(),
        rollback: Some("Review terminal output before running additional Agent steps".to_string()),
        category: Some("AI Agent".to_string()),
        references: vec![format!("terminal:{}", target.terminal_session_id)],
        target_terminal_session_id: Some(target.terminal_session_id.clone()),
        target: Some(target),
    };
    let auto_execute_first = decision == AgentApprovalDecision::Auto;
    let approval_note = if auto_execute_first {
        Some("agent policy allows automatic execution".to_string())
    } else {
        approval_note
    };
    // The command and approval note are presented by the linked execution block.
    let text = String::new();

    Ok(AiChatJobOutput {
        metadata: None,
        native_decision: None,
        mode: AiMode::Agent,
        text,
        reasoning: Some(parsed.thought).or(reasoning),
        command_cards: vec![card],
        auto_execute_first,
        approval_note,
    })
}

pub(in crate::features) fn ai_active_profile_drafts(settings: &AiSettings) -> (String, String) {
    settings
        .provider_profiles
        .iter()
        .find(|profile| profile.id == settings.active_profile_id)
        .map(|profile| {
            (
                profile.model.clone(),
                profile.base_url.clone().unwrap_or_default(),
            )
        })
        .unwrap_or_default()
}

pub(in crate::features) fn ai_usage_counts(store: &ConnectionStore) -> (usize, usize, usize) {
    let history = store.load_ai_history().unwrap_or_default();
    let audit_count = store
        .list_ai_audit_logs(None)
        .map(|logs| logs.len())
        .unwrap_or_default();
    (history.sessions.len(), history.messages.len(), audit_count)
}

#[cfg(test)]
mod tests {
    use nyaterm_core::{AiAction, AiChatRequest, AiMode, AiSettings, AiTerminalTarget, AiToolCall};

    use super::ai_agent_job_output;

    fn target(id: &str) -> AiTerminalTarget {
        AiTerminalTarget {
            terminal_session_id: id.to_string(),
            connection_id: None,
            label: id.to_string(),
            host: Some(format!("{id}.example.invalid")),
            username: Some("fixture-user".to_string()),
            session_type: "ssh".to_string(),
        }
    }

    fn request() -> AiChatRequest {
        AiChatRequest {
            stream_id: None,
            session_id: Some("chat-1".to_string()),
            connection_id: None,
            terminal_session_id: Some("terminal-a".to_string()),
            owner_scope: Default::default(),
            targets: vec![target("terminal-a"), target("terminal-b")],
            target_contexts: Vec::new(),
            mode: AiMode::Agent,
            agent_kind: Default::default(),
            permission_mode: Default::default(),
            model_id: None,
            model_name: None,
            default_target_session_id: Some("terminal-a".to_string()),
            existing_external_session_id: None,
            attachments: Vec::new(),
            action: AiAction::GenerateCommand,
            user_input: "inspect target".to_string(),
            context: Default::default(),
            options: Default::default(),
        }
    }

    fn execute_tool(target_id: Option<&str>) -> AiToolCall {
        let mut arguments = serde_json::json!({
            "thought": "inspect",
            "command": "df -h",
            "riskLevel": "low",
            "riskReason": "read only"
        });
        if let Some(target_id) = target_id {
            arguments["targetTerminalSessionId"] = serde_json::json!(target_id);
        }
        AiToolCall {
            thought_signature: None,
            id: Some("call-1".to_string()),
            name: "execute_command".to_string(),
            arguments,
        }
    }

    #[test]
    fn agent_job_binds_explicit_multi_target_tool_call() {
        let output = ai_agent_job_output(
            &AiSettings::default(),
            &request(),
            String::new(),
            None,
            vec![execute_tool(Some("terminal-b"))],
        )
        .expect("target-aware Agent output");
        assert!(
            output.text.is_empty(),
            "command proposal belongs only in the execution block"
        );

        assert_eq!(
            output.command_cards[0]
                .target_terminal_session_id
                .as_deref(),
            Some("terminal-b")
        );
        assert_eq!(
            output.command_cards[0]
                .target
                .as_ref()
                .map(|target| target.terminal_session_id.as_str()),
            Some("terminal-b")
        );
    }

    #[test]
    fn agent_job_rejects_missing_or_unknown_multi_target() {
        let missing = ai_agent_job_output(
            &AiSettings::default(),
            &request(),
            String::new(),
            None,
            vec![execute_tool(None)],
        )
        .unwrap_err();
        assert!(missing.contains("missing targetTerminalSessionId"));

        let unknown = ai_agent_job_output(
            &AiSettings::default(),
            &request(),
            String::new(),
            None,
            vec![execute_tool(Some("terminal-forged"))],
        )
        .unwrap_err();
        assert!(unknown.contains("not an available terminal"));
    }
}
