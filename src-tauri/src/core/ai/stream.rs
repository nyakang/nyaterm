use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use futures_util::StreamExt;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, ChatStreamEvent};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

use crate::config::{self, AiMode, AiSettings};
use crate::core::session::SessionManager;
use crate::error::{AppError, AppResult};

use super::agent::{run_agent_stream, AgentApprovalManager};
use super::history::{append_message, load_history, save_user_message};
use super::model::{build_client, resolve_request_model};
use super::parser::{
    extract_text_from_assistant, parse_model_output, trim_string_to_option, truncate_preview,
};
use super::prompt::{build_prompt, SYSTEM_PROMPT};
use super::redaction::{redact_context, redact_sensitive_text};
use super::types::{
    uuid, AiChatRequest, AiMessage, AiMessageRole, AiStreamEventPayload, AiStreamStart,
};

static ACTIVE_STREAMS: OnceLock<Mutex<HashMap<String, oneshot::Sender<()>>>> = OnceLock::new();

pub(super) fn active_streams() -> &'static Mutex<HashMap<String, oneshot::Sender<()>>> {
    ACTIVE_STREAMS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn cancel_all_chat_streams() {
    let senders: Vec<oneshot::Sender<()>> = active_streams()
        .lock()
        .unwrap()
        .drain()
        .map(|(_, tx)| tx)
        .collect();
    for sender in senders {
        let _ = sender.send(());
    }
}

pub(super) fn is_cancelled(cancel_rx: &mut oneshot::Receiver<()>) -> bool {
    matches!(
        cancel_rx.try_recv(),
        Ok(()) | Err(oneshot::error::TryRecvError::Closed)
    )
}

pub(super) fn emit_stream_event(app: &AppHandle, stream_id: &str, payload: AiStreamEventPayload) {
    let _ = app.emit(format!("ai-stream-{stream_id}").as_str(), payload);
}

pub fn start_chat_stream(
    app: AppHandle,
    session_manager: Arc<SessionManager>,
    mut request: AiChatRequest,
) -> AppResult<AiStreamStart> {
    let settings = config::load_app_settings(&app)?;
    if !settings.ai.enabled {
        return Err(AppError::Config("AI assistant is disabled".to_string()));
    }

    let stream_id = request
        .stream_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("ai-stream-{}", uuid()));
    let session_id = request
        .session_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("ai-session-{}", uuid()));
    request.session_id = Some(session_id.clone());

    tracing::info!(
        stream_id = %stream_id,
        session_id = %session_id,
        mode = ?request.mode,
        action = ?request.action,
        connection_id = ?request.connection_id,
        terminal_session_id = ?request.terminal_session_id,
        "Starting AI chat stream"
    );

    let (cancel_tx, cancel_rx) = oneshot::channel();
    {
        let mut streams = active_streams().lock().unwrap();
        if streams.contains_key(&stream_id) {
            return Err(AppError::Config("AI stream is already active".to_string()));
        }
        streams.insert(stream_id.clone(), cancel_tx);
    }

    let is_agent = request.mode == AiMode::Agent;
    let task_app = app.clone();
    let task_stream_id = stream_id.clone();
    let task_session_id = session_id.clone();

    if is_agent {
        use tauri::Manager;
        let approval_manager = app.state::<Arc<AgentApprovalManager>>().inner().clone();
        tauri::async_runtime::spawn(async move {
            run_agent_stream(
                task_app,
                session_manager,
                approval_manager,
                task_stream_id,
                task_session_id,
                request,
                settings.ai,
                cancel_rx,
            )
            .await;
        });
    } else {
        tauri::async_runtime::spawn(async move {
            run_chat_stream(
                task_app,
                task_stream_id,
                task_session_id,
                request,
                settings.ai,
                cancel_rx,
            )
            .await;
        });
    }

    Ok(AiStreamStart {
        stream_id,
        session_id,
    })
}

pub fn cancel_chat_stream(stream_id: String) -> AppResult<()> {
    if let Some(sender) = active_streams().lock().unwrap().remove(&stream_id) {
        let _ = sender.send(());
    }
    Ok(())
}

async fn run_chat_stream(
    app: AppHandle,
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
        language = %request.options.language,
        safety_mode = %request.options.safety_mode,
        history_turns = request.options.history_turns,
        "Running AI chat stream"
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
                "Failed to save AI user message before streaming"
            );
        }
    }

    let result = run_model_stream(&app, &stream_id, &request, &settings, &mut cancel_rx).await;

    tracing::debug!(
        stream_id = %stream_id,
        session_id = %session_id,
        success = result.is_ok(),
        "AI chat stream model execution finished"
    );

    match result {
        Ok(stream_result) => {
            if active_streams()
                .lock()
                .unwrap()
                .remove(&stream_id)
                .is_none()
            {
                emit_stream_event(
                    &app,
                    &stream_id,
                    AiStreamEventPayload {
                        event_type: "error".to_string(),
                        stream_id: stream_id.clone(),
                        session_id: Some(session_id),
                        text_delta: None,
                        reasoning_delta: None,
                        message: None,
                        command_cards: vec![],
                        usage: None,
                        error: Some("AI stream cancelled".to_string()),
                    },
                );
                return;
            }

            let (text, reasoning_content, command_cards) =
                parse_model_output(&stream_result.text, stream_result.reasoning_content);
            tracing::info!(
                stream_id = %stream_id,
                session_id = %session_id,
                raw_text_len = stream_result.text.len(),
                parsed_text_len = text.len(),
                has_reasoning = reasoning_content.is_some(),
                reasoning_len = reasoning_content.as_ref().map(|r| r.len()).unwrap_or(0),
                command_card_count = command_cards.len(),
                text_preview = %truncate_preview(&text, 200),
                "Parsed AI chat stream output"
            );
            let message = AiMessage {
                id: format!("msg-{}", uuid()),
                session_id: session_id.clone(),
                role: AiMessageRole::Assistant,
                content: text,
                created_at: super::types::now_rfc3339(),
                reasoning_content,
                command_cards: command_cards.clone(),
            };

            if settings.record_history {
                if let Err(error) = append_message(&app, message.clone()) {
                    tracing::warn!(
                        stream_id = %stream_id,
                        session_id = %session_id,
                        error = %error,
                        "Failed to append AI assistant message"
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
                    command_cards,
                    usage: None,
                    error: None,
                },
            );
        }
        Err(error) => {
            tracing::warn!(
                stream_id = %stream_id,
                session_id = %session_id,
                error = %error,
                "AI chat stream failed"
            );
            active_streams().lock().unwrap().remove(&stream_id);
            emit_stream_event(
                &app,
                &stream_id,
                AiStreamEventPayload {
                    event_type: "error".to_string(),
                    stream_id: stream_id.clone(),
                    session_id: Some(session_id),
                    text_delta: None,
                    reasoning_delta: None,
                    message: None,
                    command_cards: vec![],
                    usage: None,
                    error: Some(error.to_string()),
                },
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Ask mode model stream
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(super) struct AiStreamResult {
    pub text: String,
    pub reasoning_content: Option<String>,
}

pub(super) async fn run_model_stream(
    app: &AppHandle,
    stream_id: &str,
    request: &AiChatRequest,
    settings: &AiSettings,
    cancel_rx: &mut oneshot::Receiver<()>,
) -> AppResult<AiStreamResult> {
    tracing::debug!(
        stream_id = %stream_id,
        action = ?request.action,
        session_id = ?request.session_id,
        "Preparing AI model stream"
    );

    let resolved_model = resolve_request_model(settings, request)?;
    let client = build_client(&resolved_model)?;
    let prompt = build_prompt(request, settings);

    let mut messages = vec![ChatMessage::system(SYSTEM_PROMPT)];

    if let Some(session_id) = &request.session_id {
        let max_turns = request.options.history_turns as usize;
        if max_turns > 0 {
            if let Ok(history) = load_history(app) {
                let history_msgs: Vec<&AiMessage> = history
                    .messages
                    .iter()
                    .filter(|m| m.session_id == *session_id)
                    .collect();
                let skip = history_msgs.len().saturating_sub(max_turns);
                for msg in history_msgs.into_iter().skip(skip) {
                    match msg.role {
                        AiMessageRole::User => {
                            messages.push(ChatMessage::user(&msg.content));
                        }
                        AiMessageRole::Assistant => {
                            let content = extract_text_from_assistant(&msg.content);
                            if !content.is_empty() {
                                messages.push(ChatMessage::assistant(&content));
                            }
                        }
                        AiMessageRole::System => {}
                    }
                }
            }
        }
    }

    messages.push(ChatMessage::user(prompt));

    tracing::debug!(
        stream_id = %stream_id,
        message_count = messages.len(),
        model_name = %resolved_model.model_name,
        provider_kind = ?resolved_model.provider_kind,
        "Dispatching AI model stream request"
    );

    let chat_req = ChatRequest::new(messages);
    let chat_options = ChatOptions::default()
        .with_capture_reasoning_content(true)
        .with_normalize_reasoning_content(true);

    let stream_result = tokio::time::timeout(
        Duration::from_millis(settings.timeout_ms),
        client.exec_chat_stream(&resolved_model.model_name, chat_req, Some(&chat_options)),
    )
    .await
    .map_err(|_| AppError::Config("AI request timed out".to_string()))?
    .map_err(|error| AppError::Config(format!("AI request failed: {error}")))?;

    let mut stream = stream_result.stream;
    let mut output = String::new();
    let mut reasoning_output = String::new();
    let idle_duration = Duration::from_millis(settings.timeout_ms);
    let idle_deadline = tokio::time::sleep(idle_duration);
    tokio::pin!(idle_deadline);

    loop {
        tokio::select! {
            _ = &mut idle_deadline => {
                return Err(AppError::Config("AI stream timed out (no data received)".to_string()));
            }
            _ = &mut *cancel_rx => {
                return Err(AppError::Cancelled("AI stream cancelled".to_string()));
            }
            item = stream.next() => {
                idle_deadline.as_mut().reset(tokio::time::Instant::now() + idle_duration);
                match item {
                    Some(Ok(ChatStreamEvent::Chunk(chunk))) => {
                        let text_delta = chunk.content;
                        if !text_delta.is_empty() {
                            output.push_str(&text_delta);
                            emit_stream_event(app, stream_id, AiStreamEventPayload {
                                event_type: "delta".to_string(),
                                stream_id: stream_id.to_string(),
                                session_id: request.session_id.clone(),
                                text_delta: Some(text_delta),
                                reasoning_delta: None,
                                message: None,
                                command_cards: vec![],
                                usage: None,
                                error: None,
                            });
                        }
                    }
                    Some(Ok(ChatStreamEvent::ReasoningChunk(chunk))) => {
                        let reasoning_delta = chunk.content;
                        if !reasoning_delta.is_empty() {
                            reasoning_output.push_str(&reasoning_delta);
                            emit_stream_event(app, stream_id, AiStreamEventPayload {
                                event_type: "reasoning_delta".to_string(),
                                stream_id: stream_id.to_string(),
                                session_id: request.session_id.clone(),
                                text_delta: None,
                                reasoning_delta: Some(reasoning_delta),
                                message: None,
                                command_cards: vec![],
                                usage: None,
                                error: None,
                            });
                        }
                    }
                    Some(Ok(ChatStreamEvent::End(end))) => {
                        if reasoning_output.is_empty() {
                            if let Some(captured_reasoning_content) = end.captured_reasoning_content {
                                reasoning_output = captured_reasoning_content;
                            }
                        }
                        break;
                    }
                    None => break,
                    Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        return Err(AppError::Config(format!("AI stream failed: {error}")));
                    }
                }
            }
        }
    }

    tracing::info!(
        stream_id = %stream_id,
        text_len = output.len(),
        reasoning_len = reasoning_output.len(),
        text_preview = %truncate_preview(&output, 200),
        reasoning_preview = %truncate_preview(&reasoning_output, 200),
        "AI model stream completed"
    );

    Ok(AiStreamResult {
        text: output,
        reasoning_content: trim_string_to_option(reasoning_output),
    })
}
