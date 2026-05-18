use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::error::AppResult;

use super::types::{
    uuid, now_rfc3339, AiAuditLog, AiChatRequest, AiMessage, AiMessageRole, AiSession,
    AppendAiAuditRequest,
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(super) struct AiHistoryFile {
    #[serde(default)]
    pub sessions: Vec<AiSession>,
    #[serde(default)]
    pub messages: Vec<AiMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AiAuditFile {
    #[serde(default)]
    logs: Vec<AiAuditLog>,
}

const AI_HISTORY_MAX_SESSIONS: usize = 200;
const AI_HISTORY_MAX_MESSAGES: usize = 2_000;
const AI_AUDIT_MAX_LOGS: usize = 2_000;

pub(super) fn load_history(_app: &AppHandle) -> AppResult<AiHistoryFile> {
    crate::storage::load_json_doc(crate::storage::JSON_AI_HISTORY)
}

pub(super) fn trim_history(history: &mut AiHistoryFile) {
    history
        .sessions
        .sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    if history.sessions.len() > AI_HISTORY_MAX_SESSIONS {
        history.sessions.truncate(AI_HISTORY_MAX_SESSIONS);
    }

    let retained_sessions: HashSet<&str> = history
        .sessions
        .iter()
        .map(|session| session.id.as_str())
        .collect();
    history
        .messages
        .retain(|message| retained_sessions.contains(message.session_id.as_str()));

    if history.messages.len() > AI_HISTORY_MAX_MESSAGES {
        history
            .messages
            .sort_by(|left, right| left.created_at.cmp(&right.created_at));
        let remove_count = history.messages.len() - AI_HISTORY_MAX_MESSAGES;
        history.messages.drain(0..remove_count);
    }

    let sessions_with_messages: HashSet<&str> = history
        .messages
        .iter()
        .map(|message| message.session_id.as_str())
        .collect();
    history
        .sessions
        .retain(|session| sessions_with_messages.contains(session.id.as_str()));
}

pub(super) fn save_user_message(
    app: &AppHandle,
    session_id: &str,
    request: &AiChatRequest,
) -> AppResult<()> {
    tracing::debug!(
        session_id = %session_id,
        connection_id = ?request.connection_id,
        action = ?request.action,
        "Persisting AI user message"
    );

    let now = now_rfc3339();
    let title = request
        .user_input
        .chars()
        .take(42)
        .collect::<String>()
        .trim()
        .to_string();
    let connection_id = request.connection_id.clone();
    let user_input = request.user_input.clone();
    let session_id = session_id.to_string();

    let _ = app;
    crate::storage::update_json_doc::<AiHistoryFile, _, _>(
        crate::storage::JSON_AI_HISTORY,
        |history| {
            if let Some(session) = history
                .sessions
                .iter_mut()
                .find(|item| item.id == session_id)
            {
                session.updated_at = now.clone();
            } else {
                history.sessions.push(AiSession {
                    id: session_id.clone(),
                    connection_id,
                    title: if title.is_empty() {
                        "AI Session".to_string()
                    } else {
                        title
                    },
                    created_at: now.clone(),
                    updated_at: now.clone(),
                });
            }
            history.messages.push(AiMessage {
                id: format!("msg-{}", uuid()),
                session_id,
                role: AiMessageRole::User,
                content: user_input,
                created_at: now,
                reasoning_content: None,
                command_cards: vec![],
            });
            trim_history(history);
            Ok(())
        },
    )
}

pub(super) fn append_message(app: &AppHandle, message: AiMessage) -> AppResult<()> {
    tracing::debug!(
        session_id = %message.session_id,
        role = ?message.role,
        content_len = message.content.len(),
        command_card_count = message.command_cards.len(),
        has_reasoning = message.reasoning_content.is_some(),
        "Persisting AI message"
    );

    let _ = app;
    crate::storage::update_json_doc::<AiHistoryFile, _, _>(
        crate::storage::JSON_AI_HISTORY,
        |history| {
            if let Some(session) = history
                .sessions
                .iter_mut()
                .find(|item| item.id == message.session_id)
            {
                session.updated_at = message.created_at.clone();
            }
            history.messages.push(message);
            trim_history(history);
            Ok(())
        },
    )
}

pub fn get_ai_sessions(app: &AppHandle) -> AppResult<Vec<AiSession>> {
    let mut sessions = load_history(app)?.sessions;
    sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    Ok(sessions)
}

pub fn get_ai_messages(app: &AppHandle, session_id: String) -> AppResult<Vec<AiMessage>> {
    Ok(load_history(app)?
        .messages
        .into_iter()
        .filter(|message| message.session_id == session_id)
        .collect())
}

pub fn clear_ai_history(app: &AppHandle) -> AppResult<()> {
    let _ = app;
    super::stream::cancel_all_chat_streams();
    crate::storage::save_json_doc(crate::storage::JSON_AI_HISTORY, &AiHistoryFile::default())
}

pub fn delete_ai_session(app: &AppHandle, session_id: String) -> AppResult<()> {
    let _ = app;
    crate::storage::update_json_doc::<AiHistoryFile, _, _>(
        crate::storage::JSON_AI_HISTORY,
        |history| {
            history.sessions.retain(|s| s.id != session_id);
            history.messages.retain(|m| m.session_id != session_id);
            trim_history(history);
            Ok(())
        },
    )
}

pub fn append_ai_audit(app: &AppHandle, request: AppendAiAuditRequest) -> AppResult<AiAuditLog> {
    tracing::info!(
        connection_id = ?request.connection_id,
        action = %request.action,
        risk_level = ?request.risk_level,
        inserted_to_terminal = request.inserted_to_terminal,
        executed = request.executed,
        blocked = request.blocked,
        "Appending AI audit log"
    );

    let _ = app;
    let log = AiAuditLog {
        id: format!("audit-{}", uuid()),
        connection_id: request.connection_id,
        action: request.action,
        user_input: request.user_input,
        generated_command: request.generated_command,
        risk_level: request.risk_level,
        inserted_to_terminal: request.inserted_to_terminal,
        executed: request.executed,
        blocked: request.blocked,
        created_at: now_rfc3339(),
    };
    crate::storage::update_json_doc::<AiAuditFile, _, _>(crate::storage::JSON_AI_AUDIT, |file| {
        file.logs.push(log.clone());
        if file.logs.len() > AI_AUDIT_MAX_LOGS {
            let keep_from = file.logs.len().saturating_sub(AI_AUDIT_MAX_LOGS);
            file.logs = file.logs.split_off(keep_from);
        }
        Ok(log)
    })
}

pub fn get_ai_audit_logs(app: &AppHandle, limit: Option<usize>) -> AppResult<Vec<AiAuditLog>> {
    let _ = app;
    let mut logs =
        crate::storage::load_json_doc::<AiAuditFile>(crate::storage::JSON_AI_AUDIT)?.logs;
    logs.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    if let Some(limit) = limit {
        logs.truncate(limit);
    }
    Ok(logs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_history_without_reasoning_defaults_cleanly() {
        let raw = r#"{"sessions":[],"messages":[{"id":"m1","sessionId":"s1","role":"assistant","content":"hello","createdAt":"2026-04-28T00:00:00Z","commandCards":[]}]}"#;
        let history: AiHistoryFile = serde_json::from_str(raw).unwrap();
        assert_eq!(history.messages.len(), 1);
        assert_eq!(history.messages[0].reasoning_content, None);
    }

    #[test]
    fn trims_ai_history_to_session_and_message_limits() {
        let mut history = AiHistoryFile::default();
        for session_idx in 0..220 {
            let session_id = format!("s-{session_idx:03}");
            let updated_at = format!(
                "2026-04-28T00:{:02}:{:02}Z",
                session_idx / 60,
                session_idx % 60
            );
            history.sessions.push(AiSession {
                id: session_id.clone(),
                connection_id: None,
                title: session_id.clone(),
                created_at: updated_at.clone(),
                updated_at,
            });
            for message_idx in 0..10 {
                history.messages.push(AiMessage {
                    id: format!("m-{session_idx:03}-{message_idx:02}"),
                    session_id: session_id.clone(),
                    role: if message_idx % 2 == 0 {
                        AiMessageRole::User
                    } else {
                        AiMessageRole::Assistant
                    },
                    content: "message".to_string(),
                    created_at: format!(
                        "2026-04-28T00:{:02}:{:02}.{:03}Z",
                        session_idx / 60,
                        session_idx % 60,
                        message_idx
                    ),
                    reasoning_content: None,
                    command_cards: vec![],
                });
            }
        }

        trim_history(&mut history);

        assert_eq!(history.sessions.len(), AI_HISTORY_MAX_SESSIONS);
        assert_eq!(history.messages.len(), AI_HISTORY_MAX_MESSAGES);
        let retained_sessions: HashSet<&str> = history
            .sessions
            .iter()
            .map(|session| session.id.as_str())
            .collect();
        assert!(!retained_sessions.contains("s-000"));
        assert!(retained_sessions.contains("s-219"));
        assert!(history
            .messages
            .iter()
            .all(|message| retained_sessions.contains(message.session_id.as_str())));
    }
}
