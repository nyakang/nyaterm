use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

use crate::config::{self, AiAgentKind, AiMode, AiSettings};
use crate::core::session::SessionManager;
use crate::error::{AppError, AppResult};

use super::agent::{AgentApprovalManager, run_agent_stream};
use super::history::{append_message, save_user_message, validate_session_scope};
use super::parser::{bind_command_card_targets, parse_model_output, truncate_preview};
use super::redaction::redact_request;
use super::types::{
    AiChatRequest, AiMessage, AiMessageRole, AiStreamEventPayload, AiStreamStart, uuid,
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
    validate_session_scope(&app, &session_id, &request)?;

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
    let agent_kind = request.agent_kind.clone();
    let task_app = app.clone();
    let task_stream_id = stream_id.clone();
    let task_session_id = session_id.clone();

    match agent_kind {
        AiAgentKind::Codex => {
            use tauri::Manager;
            let approval_manager = app.state::<Arc<AgentApprovalManager>>().inner().clone();
            let codex_manager = app
                .state::<Arc<super::CodexAppServerManager>>()
                .inner()
                .clone();
            tauri::async_runtime::spawn(async move {
                super::run_codex_stream(
                    task_app,
                    session_manager,
                    approval_manager,
                    codex_manager,
                    task_stream_id,
                    task_session_id,
                    request,
                    settings.ai,
                    cancel_rx,
                )
                .await;
            });
        }
        AiAgentKind::ClaudeCode => {
            use tauri::Manager;
            let claude_runtime = app.state::<Arc<super::ClaudeCodeRuntime>>().inner().clone();
            tauri::async_runtime::spawn(async move {
                super::run_claude_code_stream(
                    task_app,
                    claude_runtime,
                    task_stream_id,
                    task_session_id,
                    request,
                    settings.ai,
                    cancel_rx,
                )
                .await;
            });
        }
        AiAgentKind::Nyaterm if is_agent => {
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
        }
        AiAgentKind::Nyaterm => {
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
        redact_request(&mut request);
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

            let (text, reasoning_content, mut command_cards) =
                parse_model_output(&stream_result.text, stream_result.reasoning_content);
            bind_command_card_targets(&mut command_cards, &request);
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
                attachments: vec![],
                references: vec![],
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

pub use nyaterm_core::core::ai::stream::AiStreamResult;
pub struct DesktopAiSink<'a>(pub &'a AppHandle);
impl nyaterm_core::core::ai::stream::AiEventSink for DesktopAiSink<'_> {
    fn emit(&self, id: &str, payload: AiStreamEventPayload) {
        emit_stream_event(self.0, id, payload)
    }
}
pub(super) async fn run_model_stream(
    app: &AppHandle,
    stream_id: &str,
    request: &AiChatRequest,
    settings: &AiSettings,
    cancel_rx: &mut oneshot::Receiver<()>,
) -> AppResult<AiStreamResult> {
    nyaterm_core::core::ai::stream::run_model_stream(
        &DesktopAiSink(app),
        stream_id,
        request,
        settings,
        cancel_rx,
    )
    .await
}
