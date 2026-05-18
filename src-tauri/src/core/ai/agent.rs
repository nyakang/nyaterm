use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, ChatStreamEvent};
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

use crate::config::AiSettings;
use crate::core::capture;
use crate::core::session::{SessionCommand, SessionManager};
use crate::error::{AppError, AppResult};

use super::history::{append_message, save_user_message};
use super::model::{build_client, resolve_request_model};
use super::parser::{extract_json_object, parse_model_output, trim_string_to_option};
use super::prompt::{build_agent_prompt, build_observation_message, AGENT_SYSTEM_PROMPT};
use super::redaction::{redact_context, redact_sensitive_text};
use super::stream::{active_streams, emit_stream_event, is_cancelled};
use super::types::{
    uuid, now_rfc3339, AgentActionKind, AgentLlmResponse, AgentStepAction, AgentStepPayload,
    AgentStepStatus, AiChatRequest, AiMessage, AiMessageRole, AiStreamEventPayload,
    CommandObservation,
};

// ---------------------------------------------------------------------------
// Agent approval
// ---------------------------------------------------------------------------

/// Manages pending agent step approvals awaiting user confirmation from the frontend.
pub struct AgentApprovalManager {
    pending: tokio::sync::Mutex<HashMap<String, oneshot::Sender<bool>>>,
}

impl AgentApprovalManager {
    pub fn new() -> Self {
        Self {
            pending: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    pub async fn register(&self, key: String) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(key, tx);
        rx
    }

    pub async fn respond(&self, key: &str, approved: bool) -> bool {
        if let Some(tx) = self.pending.lock().await.remove(key) {
            tx.send(approved).is_ok()
        } else {
            false
        }
    }

    #[allow(dead_code)]
    pub async fn cancel_all_for_stream(&self, stream_id: &str) {
        let mut pending = self.pending.lock().await;
        let keys_to_remove: Vec<String> = pending
            .keys()
            .filter(|k| k.starts_with(stream_id))
            .cloned()
            .collect();
        for key in keys_to_remove {
            if let Some(tx) = pending.remove(&key) {
                let _ = tx.send(false);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Agent (ReAct) loop
// ---------------------------------------------------------------------------

const DEFAULT_MAX_AGENT_STEPS: u16 = 10;
const DEFAULT_AGENT_STEP_TIMEOUT_MS: u64 = 30_000;

fn emit_agent_step(app: &AppHandle, stream_id: &str, payload: AgentStepPayload) {
    let _ = app.emit(format!("ai-stream-{stream_id}").as_str(), payload);
}

fn emit_agent_error(app: &AppHandle, stream_id: &str, session_id: &str, error: &str) {
    tracing::warn!(
        stream_id = %stream_id,
        session_id = %session_id,
        error = %error,
        "AI agent stream failed"
    );
    active_streams().lock().unwrap().remove(stream_id);
    emit_stream_event(
        app,
        stream_id,
        AiStreamEventPayload {
            event_type: "error".to_string(),
            stream_id: stream_id.to_string(),
            session_id: Some(session_id.to_string()),
            text_delta: None,
            reasoning_delta: None,
            message: None,
            command_cards: vec![],
            usage: None,
            error: Some(error.to_string()),
        },
    );
}

/// Execute a command via marker-based PTY capture.
///
/// Injects the command directly into the interactive PTY session (wrapped with
/// unique boundary markers), then captures the output from the PTY stream.
/// This works regardless of the user's current context: nested SSH, containers,
/// `sudo su`, etc.
async fn execute_command_on_session(
    app: &AppHandle,
    session_manager: &SessionManager,
    terminal_session_id: &str,
    command: &str,
    timeout_ms: u64,
) -> AppResult<CommandObservation> {
    tracing::debug!(
        terminal_session_id = %terminal_session_id,
        timeout_ms,
        command_preview = %safe_command_preview(command),
        "Preparing to execute agent command via PTY capture"
    );

    let marker_id = uuid::Uuid::new_v4().to_string();
    let wrapped = capture::build_capture_command(&marker_id, command);
    let (tx, rx) = oneshot::channel();

    let capture_event = format!("ai-capture-{terminal_session_id}");
    let _ = app.emit(&capture_event, "start");

    session_manager
        .send_command(
            terminal_session_id,
            SessionCommand::CaptureExec {
                marker_id: marker_id.clone(),
                wrapped_command: wrapped.into_bytes(),
                result_tx: tx,
            },
        )
        .await?;

    let timeout_dur = Duration::from_millis(timeout_ms);
    let result = match tokio::time::timeout(timeout_dur, rx).await {
        Ok(Ok(captured)) => {
            let output = strip_ansi_escapes(&captured.output);
            tracing::debug!(
                terminal_session_id = %terminal_session_id,
                marker_id = %marker_id,
                exit_code = ?captured.exit_code,
                duration_ms = captured.duration_ms,
                output_len = output.len(),
                "PTY capture completed"
            );
            Ok(CommandObservation {
                output,
                exit_code: captured.exit_code,
                duration_ms: captured.duration_ms,
            })
        }
        Ok(Err(_)) => {
            tracing::warn!(
                terminal_session_id = %terminal_session_id,
                marker_id = %marker_id,
                "PTY capture channel closed without result"
            );
            Err(AppError::Channel(
                "Capture channel closed — session may have disconnected".to_string(),
            ))
        }
        Err(_) => {
            tracing::warn!(
                terminal_session_id = %terminal_session_id,
                marker_id = %marker_id,
                timeout_ms,
                "PTY capture timed out"
            );
            Ok(CommandObservation {
                output: "(command timed out — markers not detected in PTY output)".to_string(),
                exit_code: None,
                duration_ms: timeout_ms,
            })
        }
    };

    let _ = app.emit(&capture_event, "end");
    result
}

fn strip_ansi_escapes(input: &str) -> String {
    strip_ansi_escapes::strip_str(input)
}

fn truncate_for_log(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

fn safe_command_preview(command: &str) -> String {
    redact_sensitive_text(&truncate_for_log(command, 200))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_agent_stream(
    app: AppHandle,
    session_manager: Arc<SessionManager>,
    approval_manager: Arc<AgentApprovalManager>,
    stream_id: String,
    session_id: String,
    mut request: AiChatRequest,
    settings: AiSettings,
    mut cancel_rx: oneshot::Receiver<()>,
) {
    tracing::info!(
        stream_id = %stream_id,
        session_id = %session_id,
        action = ?request.action,
        connection_id = ?request.connection_id,
        terminal_session_id = ?request.terminal_session_id,
        "Running AI agent stream"
    );

    emit_stream_event(
        &app,
        &stream_id,
        AiStreamEventPayload {
            event_type: "start".to_string(),
            stream_id: stream_id.clone(),
            session_id: Some(session_id.clone()),
            text_delta: None,
            reasoning_delta: None,
            message: None,
            command_cards: vec![],
            usage: None,
            error: None,
        },
    );

    if settings.redaction_enabled {
        redact_context(&mut request.context);
        request.user_input = redact_sensitive_text(&request.user_input);
    }

    if settings.record_history {
        if let Err(error) = save_user_message(&app, &session_id, &request) {
            tracing::warn!(
                stream_id = %stream_id,
                session_id = %session_id,
                error = %error,
                "Failed to save agent user message before execution"
            );
        }
    }

    let terminal_session_id = match &request.terminal_session_id {
        Some(id) if !id.trim().is_empty() => id.clone(),
        _ => {
            emit_agent_error(
                &app,
                &stream_id,
                &session_id,
                "Agent mode requires a terminal session",
            );
            return;
        }
    };

    let resolved_model = match resolve_request_model(&settings, &request) {
        Ok(m) => m,
        Err(e) => {
            emit_agent_error(&app, &stream_id, &session_id, &e.to_string());
            return;
        }
    };

    let max_steps = settings.max_agent_steps.unwrap_or(DEFAULT_MAX_AGENT_STEPS);
    let step_timeout = settings
        .agent_step_timeout_ms
        .unwrap_or(DEFAULT_AGENT_STEP_TIMEOUT_MS);

    tracing::info!(
        stream_id = %stream_id,
        session_id = %session_id,
        model_name = %resolved_model.model_name,
        provider_kind = ?resolved_model.provider_kind,
        max_steps,
        step_timeout,
        "AI agent stream resolved configuration"
    );

    let mut conversation = vec![ChatMessage::system(AGENT_SYSTEM_PROMPT)];
    let initial_prompt = build_agent_prompt(&request, &settings);
    conversation.push(ChatMessage::user(initial_prompt));

    let mut final_answer: Option<String> = None;
    let mut all_steps: Vec<AgentStepPayload> = Vec::new();

    for step_index in 0..max_steps {
        tracing::debug!(
            stream_id = %stream_id,
            session_id = %session_id,
            step_index,
            conversation_len = conversation.len(),
            "Starting AI agent step"
        );

        if is_cancelled(&mut cancel_rx) {
            emit_agent_error(&app, &stream_id, &session_id, "AI stream cancelled");
            return;
        }

        let client = match build_client(&resolved_model) {
            Ok(c) => c,
            Err(e) => {
                emit_agent_error(&app, &stream_id, &session_id, &e.to_string());
                return;
            }
        };

        let chat_req = ChatRequest::new(conversation.clone());
        let chat_options = ChatOptions::default()
            .with_capture_reasoning_content(true)
            .with_normalize_reasoning_content(true);

        let stream_result = match tokio::time::timeout(
            Duration::from_millis(settings.timeout_ms),
            client.exec_chat_stream(&resolved_model.model_name, chat_req, Some(&chat_options)),
        )
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                emit_agent_error(
                    &app,
                    &stream_id,
                    &session_id,
                    &format!("AI request failed: {e}"),
                );
                return;
            }
            Err(_) => {
                emit_agent_error(&app, &stream_id, &session_id, "AI request timed out");
                return;
            }
        };

        let mut raw_output = String::new();
        let mut reasoning_output = String::new();
        let mut stream = stream_result.stream;
        let idle_duration = Duration::from_millis(settings.timeout_ms);
        let idle_deadline = tokio::time::sleep(idle_duration);
        tokio::pin!(idle_deadline);

        loop {
            tokio::select! {
                _ = &mut idle_deadline => break,
                _ = &mut cancel_rx => {
                    emit_agent_error(&app, &stream_id, &session_id, "AI stream cancelled");
                    return;
                }
                item = stream.next() => {
                    idle_deadline.as_mut().reset(tokio::time::Instant::now() + idle_duration);
                    match item {
                        Some(Ok(ChatStreamEvent::Chunk(chunk))) => {
                            if !chunk.content.is_empty() {
                                raw_output.push_str(&chunk.content);
                            }
                        }
                        Some(Ok(ChatStreamEvent::ReasoningChunk(chunk))) => {
                            if !chunk.content.is_empty() {
                                reasoning_output.push_str(&chunk.content);
                                emit_stream_event(&app, &stream_id, AiStreamEventPayload {
                                    event_type: "reasoning_delta".to_string(),
                                    stream_id: stream_id.clone(),
                                    session_id: Some(session_id.clone()),
                                    text_delta: None,
                                    reasoning_delta: Some(chunk.content),
                                    message: None,
                                    command_cards: vec![],
                                    usage: None,
                                    error: None,
                                });
                            }
                        }
                        Some(Ok(ChatStreamEvent::End(end))) => {
                            if reasoning_output.is_empty() {
                                if let Some(r) = end.captured_reasoning_content {
                                    reasoning_output = r;
                                }
                            }
                            break;
                        }
                        None => break,
                        Some(Ok(_)) => {}
                        Some(Err(e)) => {
                            emit_agent_error(&app, &stream_id, &session_id, &format!("AI stream failed: {e}"));
                            return;
                        }
                    }
                }
            }
        }

        let candidate =
            extract_json_object(&raw_output).unwrap_or_else(|| raw_output.trim().to_string());

        let parsed: AgentLlmResponse = match serde_json::from_str(&candidate) {
            Ok(r) => r,
            Err(error) => {
                tracing::warn!(
                    stream_id = %stream_id,
                    session_id = %session_id,
                    step_index,
                    error = %error,
                    raw_output_len = raw_output.len(),
                    "Failed to parse AI agent step response as JSON; falling back to final text"
                );
                let (text, _, _) =
                    parse_model_output(&raw_output, trim_string_to_option(reasoning_output));
                final_answer = Some(text);
                break;
            }
        };

        conversation.push(ChatMessage::assistant(&raw_output));

        tracing::debug!(
            stream_id = %stream_id,
            session_id = %session_id,
            step_index,
            action = %parsed.action,
            has_command = parsed.command.as_ref().is_some_and(|value| !value.trim().is_empty()),
            has_answer = parsed.answer.as_ref().is_some_and(|value| !value.trim().is_empty()),
            reasoning_len = reasoning_output.len(),
            "Parsed AI agent step response"
        );

        match parsed.action.as_str() {
            "final_answer" => {
                let answer = parsed.answer.unwrap_or_default();
                tracing::info!(
                    stream_id = %stream_id,
                    session_id = %session_id,
                    step_index,
                    answer_len = answer.len(),
                    "AI agent produced final answer"
                );
                let step = AgentStepPayload {
                    stream_id: stream_id.clone(),
                    session_id: Some(session_id.clone()),
                    step_index,
                    thought: parsed.thought,
                    action: AgentStepAction {
                        kind: AgentActionKind::FinalAnswer,
                        command: None,
                        risk_level: None,
                        answer: Some(answer.clone()),
                    },
                    observation: None,
                    status: AgentStepStatus::Completed,
                    error: None,
                };
                emit_agent_step(&app, &stream_id, step.clone());
                all_steps.push(step);
                final_answer = Some(answer);
                break;
            }
            "execute_command" => {
                let command = match &parsed.command {
                    Some(c) if !c.trim().is_empty() => c.trim().to_string(),
                    _ => {
                        emit_agent_error(
                            &app,
                            &stream_id,
                            &session_id,
                            "Agent returned execute_command without a command",
                        );
                        return;
                    }
                };

                tracing::info!(
                    stream_id = %stream_id,
                    session_id = %session_id,
                    step_index,
                    command_preview = %safe_command_preview(&command),
                    "AI agent proposed command, awaiting user approval"
                );

                let approval_step = AgentStepPayload {
                    stream_id: stream_id.clone(),
                    session_id: Some(session_id.clone()),
                    step_index,
                    thought: parsed.thought.clone(),
                    action: AgentStepAction {
                        kind: AgentActionKind::ExecuteCommand,
                        command: Some(command.clone()),
                        risk_level: None,
                        answer: None,
                    },
                    observation: None,
                    status: AgentStepStatus::NeedsApproval,
                    error: None,
                };
                emit_agent_step(&app, &stream_id, approval_step.clone());
                all_steps.push(approval_step);

                let approval_key = format!("{}-{}", stream_id, step_index);
                let approval_rx = approval_manager.register(approval_key).await;

                let approved = tokio::select! {
                    _ = &mut cancel_rx => {
                        emit_agent_error(&app, &stream_id, &session_id, "AI stream cancelled");
                        return;
                    }
                    result = approval_rx => result.unwrap_or(false),
                };

                if !approved {
                    let step = AgentStepPayload {
                        stream_id: stream_id.clone(),
                        session_id: Some(session_id.clone()),
                        step_index,
                        thought: parsed.thought,
                        action: AgentStepAction {
                            kind: AgentActionKind::ExecuteCommand,
                            command: Some(command.clone()),
                            risk_level: None,
                            answer: None,
                        },
                        observation: None,
                        status: AgentStepStatus::Rejected,
                        error: None,
                    };
                    emit_agent_step(&app, &stream_id, step.clone());
                    if let Some(last) = all_steps.last_mut() {
                        *last = step;
                    }

                    let skipped_msg = format!(
                        "用户拒绝执行命令 `{}`。请换用其他方案或给出 final_answer。",
                        command
                    );
                    conversation.push(ChatMessage::user(skipped_msg));
                    continue;
                }

                let running_step = AgentStepPayload {
                    stream_id: stream_id.clone(),
                    session_id: Some(session_id.clone()),
                    step_index,
                    thought: parsed.thought.clone(),
                    action: AgentStepAction {
                        kind: AgentActionKind::ExecuteCommand,
                        command: Some(command.clone()),
                        risk_level: None,
                        answer: None,
                    },
                    observation: None,
                    status: AgentStepStatus::Running,
                    error: None,
                };
                emit_agent_step(&app, &stream_id, running_step);

                let obs = match execute_command_on_session(
                    &app,
                    &session_manager,
                    &terminal_session_id,
                    &command,
                    step_timeout,
                )
                .await
                {
                    Ok(obs) => obs,
                    Err(e) => {
                        let step = AgentStepPayload {
                            stream_id: stream_id.clone(),
                            session_id: Some(session_id.clone()),
                            step_index,
                            thought: parsed.thought,
                            action: AgentStepAction {
                                kind: AgentActionKind::ExecuteCommand,
                                command: Some(command.clone()),
                                risk_level: None,
                                answer: None,
                            },
                            observation: None,
                            status: AgentStepStatus::Failed,
                            error: Some(e.to_string()),
                        };
                        emit_agent_step(&app, &stream_id, step.clone());
                        if let Some(last) = all_steps.last_mut() {
                            *last = step;
                        }

                        tracing::warn!(
                            stream_id = %stream_id,
                            session_id = %session_id,
                            step_index,
                            error = %e,
                            command_preview = %safe_command_preview(&command),
                            "AI agent command execution failed"
                        );

                        let err_msg = format!("命令执行失败：{}。请分析原因并给出下一步。", e);
                        conversation.push(ChatMessage::user(err_msg));
                        continue;
                    }
                };

                tracing::info!(
                    stream_id = %stream_id,
                    session_id = %session_id,
                    step_index,
                    exit_code = obs.exit_code,
                    duration_ms = obs.duration_ms,
                    output_len = obs.output.len(),
                    command_preview = %safe_command_preview(&command),
                    "AI agent command executed successfully"
                );

                let completed_step = AgentStepPayload {
                    stream_id: stream_id.clone(),
                    session_id: Some(session_id.clone()),
                    step_index,
                    thought: parsed.thought,
                    action: AgentStepAction {
                        kind: AgentActionKind::ExecuteCommand,
                        command: Some(command.clone()),
                        risk_level: None,
                        answer: None,
                    },
                    observation: Some(obs.clone()),
                    status: AgentStepStatus::Completed,
                    error: None,
                };
                emit_agent_step(&app, &stream_id, completed_step.clone());
                if let Some(last) = all_steps.last_mut() {
                    *last = completed_step;
                }

                let obs_msg = build_observation_message(&obs, &command);
                conversation.push(ChatMessage::user(obs_msg));
            }
            other => {
                let fallback = format!(
                    "Unknown action '{}'. Treating as final answer. {}",
                    other,
                    parsed.answer.as_deref().unwrap_or(&parsed.thought)
                );
                final_answer = Some(fallback);
                break;
            }
        }
    }

    active_streams().lock().unwrap().remove(&stream_id);

    tracing::info!(
        stream_id = %stream_id,
        session_id = %session_id,
        step_count = all_steps.len(),
        has_final_answer = final_answer.is_some(),
        "AI agent stream finished loop"
    );

    let answer_text =
        final_answer.unwrap_or_else(|| "Agent 已达到最大步数限制，任务可能未完成。".to_string());

    let message = AiMessage {
        id: format!("msg-{}", uuid()),
        session_id: session_id.clone(),
        role: AiMessageRole::Assistant,
        content: answer_text,
        created_at: now_rfc3339(),
        reasoning_content: None,
        command_cards: vec![],
    };

    if settings.record_history {
        if let Err(error) = append_message(&app, message.clone()) {
            tracing::warn!(
                stream_id = %stream_id,
                session_id = %session_id,
                error = %error,
                "Failed to append AI agent assistant message"
            );
        }
    }

    emit_stream_event(
        &app,
        &stream_id,
        AiStreamEventPayload {
            event_type: "done".to_string(),
            stream_id: stream_id.clone(),
            session_id: Some(session_id),
            text_delta: None,
            reasoning_delta: None,
            message: Some(message),
            command_cards: vec![],
            usage: None,
            error: None,
        },
    );
}
