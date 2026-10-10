use crate::{
    error::{Result, WebError},
    state::{Event, State},
};
use nyaterm_core::{
    config::{self, AiAgentKind, AiMode},
    core::ai::{catalog, history, model, parser, redaction, stream, types::*},
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

pub async fn command(
    state: &Arc<State>,
    owner: &str,
    command: &str,
    args: &Value,
) -> Result<Value> {
    match command {
        "append_ai_audit" => {
            let mut request: AppendAiAuditRequest =
                serde_json::from_value(args["request"].clone())?;
            request.user_input = request
                .user_input
                .map(|v| redaction::redact_sensitive_text(&v));
            request.generated_command = request
                .generated_command
                .map(|v| redaction::redact_sensitive_text(&v));
            request.error = request.error.map(|v| redaction::redact_sensitive_text(&v));
            Ok(json!(history::append_ai_audit(&(), request)?))
        }
        "reveal_ai_provider_api_key" => {
            let settings = config::load_app_settings(&())?.ai;
            let id = crate::commands::text(args, "credentialId")?;
            let key = settings
                .provider_credentials
                .iter()
                .find(|c| c.id == id)
                .and_then(|c| c.api_key.as_ref())
                .or_else(|| {
                    settings
                        .provider_profiles
                        .iter()
                        .find(|c| c.id == id)
                        .and_then(|c| c.api_key.as_ref())
                })
                .ok_or(WebError::bad("Provider key not configured"))?;
            Ok(json!(key))
        }
        "clear_ai_history" => {
            history::clear_ai_history_storage(&())?;
            Ok(Value::Null)
        }
        "rebind_ai_session" => {
            let scope: AiSessionScope = serde_json::from_value(args["ownerScope"].clone())?;
            validate_scope(state, owner, &scope).await?;
            Ok(json!(history::rebind_ai_session(
                &(),
                args["sessionId"]
                    .as_str()
                    .ok_or(WebError::bad("AI session required"))?
                    .into(),
                scope
            )?))
        }
        "get_ai_sessions" => Ok(json!(history::get_ai_sessions(&())?)),
        "get_ai_messages" => Ok(json!(history::get_ai_messages(
            &(),
            args["sessionId"]
                .as_str()
                .ok_or(WebError::bad("AI session required"))?
                .into()
        )?)),
        "delete_ai_session" => {
            history::delete_ai_session(
                &(),
                args["sessionId"]
                    .as_str()
                    .ok_or(WebError::bad("AI session required"))?
                    .into(),
            )?;
            Ok(Value::Null)
        }
        "list_ai_model_names" => Ok(json!(model::list_model_names(&()).await?)),
        "test_ai_model_connection" => {
            let settings = merged_settings(args)?;
            model::test_model_connection(
                &settings,
                args["modelId"]
                    .as_str()
                    .ok_or(WebError::bad("AI model required"))?,
            )
            .await?;
            Ok(Value::Null)
        }
        "test_ai_provider_connection" => {
            let settings = merged_settings(args)?;
            Ok(json!(
                model::test_provider_connection(
                    &settings,
                    args["credentialId"]
                        .as_str()
                        .ok_or(WebError::bad("Provider required"))?
                )
                .await?
            ))
        }
        "refresh_ai_model_settings" => {
            let _guard = state.mutation.lock().await;
            let mut existing = config::load_app_settings(&())?;
            let mut settings = merged_settings(args)?;
            let discoveries = model::list_model_names_for_settings(&settings).await?;
            settings.models = catalog::merge_model_discoveries(&settings, discoveries);
            settings.default_model_id =
                catalog::update_default_model_id(&settings, &settings.models);
            config::normalize_ai_settings(&mut settings);
            existing.ai = config::encrypt_ai_settings(settings.clone())?;
            existing.cloud_sync = config::encrypt_cloud_sync_settings(existing.cloud_sync)?;
            config::save_app_settings(&(), &existing)?;
            state.broadcast("settings-changed", Value::Null).await;
            Ok(json!(config::mask_ai_settings(settings)))
        }
        "cancel_ai_chat_stream" => {
            let id = args["streamId"]
                .as_str()
                .ok_or(WebError::bad("AI stream required"))?;
            if let Some(token) = state
                .ai_streams
                .lock()
                .await
                .remove(&(owner.into(), id.into()))
            {
                token.cancel();
            }
            Ok(Value::Null)
        }
        "start_ai_chat_stream" => start(state, owner, args).await,
        _ => Err(WebError::unsupported()),
    }
}
pub fn supports(command: &str) -> bool {
    matches!(
        command,
        "clear_ai_history"
            | "append_ai_audit"
            | "reveal_ai_provider_api_key"
            | "rebind_ai_session"
            | "test_ai_provider_connection"
            | "refresh_ai_model_settings"
            | "get_ai_sessions"
            | "get_ai_messages"
            | "delete_ai_session"
            | "list_ai_model_names"
            | "test_ai_model_connection"
            | "cancel_ai_chat_stream"
            | "start_ai_chat_stream"
    )
}
fn merged_settings(args: &Value) -> Result<config::AiSettings> {
    let existing = config::load_app_settings(&())?.ai;
    if args["aiSettings"].is_null() {
        return Ok(existing);
    }
    Ok(config::merge_masked_ai_settings(
        &existing,
        serde_json::from_value(args["aiSettings"].clone())?,
    ))
}
async fn validate_scope(state: &Arc<State>, owner: &str, scope: &AiSessionScope) -> Result<()> {
    if scope.r#type == AiSessionScopeType::Terminal {
        state
            .session(
                owner,
                scope
                    .target_id
                    .as_deref()
                    .ok_or(WebError::bad("Terminal scope requires session"))?,
            )
            .await?;
    }
    Ok(())
}
async fn start(state: &Arc<State>, owner: &str, args: &Value) -> Result<Value> {
    let mut request: AiChatRequest = serde_json::from_value(args["request"].clone())?;
    if request.mode != AiMode::Ask
        || request.agent_kind != AiAgentKind::Nyaterm
        || !request.attachments.is_empty()
    {
        return Err(WebError::unsupported());
    }
    for id in request
        .terminal_session_id
        .iter()
        .chain(request.default_target_session_id.iter())
        .chain(request.targets.iter().map(|t| &t.terminal_session_id))
        .chain(
            request
                .target_contexts
                .iter()
                .filter_map(|t| t.target.as_ref().map(|t| &t.terminal_session_id)),
        )
    {
        state.session(owner, id).await?;
    }
    validate_scope(state, owner, &request.owner_scope).await?;
    let mut settings = config::load_app_settings(&())?.ai;
    if !settings.enabled {
        return Err(WebError::bad("AI assistant is disabled"));
    }
    settings.redaction_enabled = true;
    model::resolve_request_model(&settings, &request)?;
    let stream_id = request
        .stream_id
        .clone()
        .filter(|s| s.len() <= 128 && !s.is_empty())
        .unwrap_or_else(|| format!("ai-stream-{}", uuid()));
    let session_id = request
        .session_id
        .clone()
        .filter(|s| s.len() <= 128 && !s.is_empty())
        .unwrap_or_else(|| format!("ai-session-{}", uuid()));
    request.session_id = Some(session_id.clone());
    history::validate_session_scope(&(), &session_id, &request)?;
    redaction::redact_request(&mut request);
    let login = state.login(owner).await?;
    let cancel = login.cancel.child_token();
    let key = (owner.to_owned(), stream_id.clone());
    let mut streams = state.ai_streams.lock().await;
    if streams.contains_key(&key) || streams.keys().filter(|(id, _)| id == owner).count() >= 4 {
        return Err(WebError::bad("Too many AI streams"));
    }
    streams.insert(key.clone(), cancel.clone());
    drop(streams);
    let state = state.clone();
    let events = login.events.clone();
    let task_stream = stream_id.clone();
    let task_session = session_id.clone();
    crate::observability::spawn(async move {
        let sink = stream::EventSink(move |id: &str, payload: AiStreamEventPayload| {
            let _ = events.send(Event {
                event: format!("ai-stream-{id}"),
                payload: json!(payload),
            });
        });
        let make_event = |kind: &str| AiStreamEventPayload {
            event_type: kind.into(),
            stream_id: task_stream.clone(),
            session_id: Some(task_session.clone()),
            text_delta: None,
            reasoning_delta: None,
            message: None,
            command_cards: vec![],
            usage: None,
            error: None,
        };
        stream::emit_stream_event(&sink, &task_stream, make_event("start"));
        if settings.record_history {
            let _ = history::save_user_message(&(), &task_session, &request);
        }
        let (_cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();
        let result = tokio::select! {
            _ = cancel.cancelled()=>Err(WebError::bad("AI stream cancelled")),
            result=tokio::time::timeout(Duration::from_secs(600),stream::run_model_stream(&sink,&task_stream,&request,&settings,&mut cancel_rx))=>match result{Ok(result)=>result.map_err(Into::into),Err(_)=>Err(WebError::bad("AI stream timed out"))},
        };
        let event = match result {
            Ok(output) => {
                let (text, reasoning, mut cards) =
                    parser::parse_model_output(&output.text, output.reasoning_content);
                parser::bind_command_card_targets(&mut cards, &request);
                let message = AiMessage {
                    id: format!("msg-{}", uuid()),
                    session_id: task_session.clone(),
                    role: AiMessageRole::Assistant,
                    content: text,
                    created_at: now_rfc3339(),
                    reasoning_content: reasoning,
                    command_cards: cards.clone(),
                    attachments: vec![],
                    references: vec![],
                };
                if settings.record_history {
                    let _ = history::append_message(&(), message.clone());
                }
                let mut event = make_event("done");
                event.message = Some(message);
                event.command_cards = cards;
                event
            }
            Err(_) => {
                let mut event = make_event("error");
                event.error = Some("AI request failed or was cancelled".into());
                event
            }
        };
        stream::emit_stream_event(&sink, &task_stream, event);
        state.ai_streams.lock().await.remove(&key);
    });
    Ok(json!({"streamId":stream_id,"sessionId":session_id}))
}
