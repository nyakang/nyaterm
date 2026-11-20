//! AI chat history and audit-log persistence.
//!
//! Split out of `storage.rs` by domain. Document keys, record shapes and the
//! trimming rules are unchanged; this only moves the code.

use super::{
    ConnectionStore, SETTINGS_AI_AUDIT, SETTINGS_AI_HISTORY, SETTINGS_TABLE, StorageError,
    merge_unknown_json,
};
use nyaterm_core::ai::{normalize_ai_history_sessions, validate_ai_session_scope};
use nyaterm_core::{
    AiAgentKind, AiAuditFile, AiAuditLog, AiHistoryFile, AiMessage, AiMessageRole, AiSession,
    AiSessionBackendMetadata, AiSessionScope, AiSessionScopeType, AppendAiAuditRequest,
    now_rfc3339, trim_ai_audit, trim_ai_history, uuid,
};

impl ConnectionStore {
    pub fn load_ai_history(&self) -> Result<AiHistoryFile, StorageError> {
        self.read_json_table::<AiHistoryFile>(SETTINGS_TABLE, SETTINGS_AI_HISTORY)
            .map(|history| history.unwrap_or_default())
            .map(|mut history| {
                normalize_ai_history_sessions(&mut history);
                history
            })
    }
    pub fn save_ai_history(&self, mut history: AiHistoryFile) -> Result<(), StorageError> {
        trim_ai_history(&mut history);
        let mut next = serde_json::to_value(history)?;
        let current = self.load_settings_doc_value(SETTINGS_AI_HISTORY, serde_json::Value::Null)?;
        merge_unknown_json(&current, &mut next);
        self.save_settings_doc_value(SETTINGS_AI_HISTORY, &next)?;
        Ok(())
    }
    pub fn append_ai_user_message(
        &self,
        session_id: &str,
        connection_id: Option<String>,
        user_input: String,
    ) -> Result<(), StorageError> {
        self.append_ai_user_message_scoped(
            session_id,
            connection_id,
            user_input,
            AiAgentKind::Nyaterm,
            AiSessionScope::default(),
        )
    }

    pub fn append_ai_user_message_scoped(
        &self,
        session_id: &str,
        connection_id: Option<String>,
        user_input: String,
        agent_kind: AiAgentKind,
        scope: AiSessionScope,
    ) -> Result<(), StorageError> {
        let now = now_rfc3339();
        let title = ai_session_title(&user_input);
        let session_id = session_id.to_string();
        let mut history = self.load_ai_history()?;
        if let Some(session) = history
            .sessions
            .iter_mut()
            .find(|item| item.id == session_id)
        {
            validate_ai_session_scope(session, &agent_kind, &scope)
                .map_err(|error| StorageError::InvalidData(error.to_string()))?;
            session.updated_at = now.clone();
            if session.scope.r#type == AiSessionScopeType::Unbound
                && session.scope.target_id.is_none()
            {
                session.scope = scope;
                session.agent_kind = agent_kind;
            }
        } else {
            history.sessions.push(AiSession {
                id: session_id.clone(),
                agent_kind,
                scope,
                connection_id,
                title,
                created_at: now.clone(),
                updated_at: now.clone(),
                external_session_id: None,
                backend_metadata: None,
            });
        }
        history.messages.push(AiMessage {
            id: format!("msg-{}", uuid()),
            session_id,
            role: AiMessageRole::User,
            content: user_input,
            created_at: now,
            reasoning_content: None,
            command_cards: Vec::new(),
        });
        self.save_ai_history(history)
    }
    pub fn append_ai_message(&self, message: AiMessage) -> Result<(), StorageError> {
        let mut history = self.load_ai_history()?;
        if let Some(session) = history
            .sessions
            .iter_mut()
            .find(|item| item.id == message.session_id)
        {
            session.updated_at = message.created_at.clone();
        }
        // Native agent runs re-persist the in-memory assistant message at every
        // projection point, so the same id is appended repeatedly with growing
        // content. The newest snapshot must replace the earlier projection of
        // that id instead of leaving duplicate rows behind.
        if let Some(existing) = history
            .messages
            .iter_mut()
            .find(|item| item.id == message.id && item.session_id == message.session_id)
        {
            *existing = message;
        } else {
            history.messages.push(message);
        }
        self.save_ai_history(history)
    }

    pub fn set_ai_session_external_metadata(
        &self,
        session_id: &str,
        agent_kind: AiAgentKind,
        external_session_id: String,
        backend_metadata: AiSessionBackendMetadata,
    ) -> Result<(), StorageError> {
        let mut history = self.load_ai_history()?;
        let session = history
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
            .ok_or_else(|| {
                StorageError::InvalidData(format!("AI session {session_id} not found"))
            })?;
        session.agent_kind = agent_kind;
        session.external_session_id = Some(external_session_id);
        session.backend_metadata = Some(backend_metadata);
        session.updated_at = now_rfc3339();
        self.save_ai_history(history)
    }

    pub fn list_ai_sessions(&self) -> Result<Vec<AiSession>, StorageError> {
        let mut sessions = self.load_ai_history()?.sessions;
        sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        Ok(sessions)
    }
    pub fn list_ai_messages(&self, session_id: &str) -> Result<Vec<AiMessage>, StorageError> {
        let messages: Vec<AiMessage> = self
            .load_ai_history()?
            .messages
            .into_iter()
            .filter(|message| message.session_id == session_id)
            .collect();
        // Databases written before `append_ai_message` replaced projections in
        // place hold several rows sharing one message id. Keep the latest
        // projection of each id so every message renders once.
        let mut last_position = std::collections::HashMap::with_capacity(messages.len());
        for (index, message) in messages.iter().enumerate() {
            last_position.insert(message.id.clone(), index);
        }
        Ok(messages
            .into_iter()
            .enumerate()
            .filter(|(index, message)| last_position.get(message.id.as_str()) == Some(index))
            .map(|(_, message)| message)
            .collect())
    }
    pub fn rebind_ai_session(
        &self,
        session_id: &str,
        scope: AiSessionScope,
    ) -> Result<AiSession, StorageError> {
        let mut history = self.load_ai_history()?;
        let session = history
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
            .ok_or_else(|| StorageError::InvalidData("AI session not found".to_string()))?;
        session.scope = scope;
        session.updated_at = now_rfc3339();
        let rebound = session.clone();
        self.save_ai_history(history)?;
        Ok(rebound)
    }
    pub fn clear_ai_history(&self) -> Result<(), StorageError> {
        self.save_ai_history(AiHistoryFile::default())
    }
    pub fn delete_ai_session(&self, session_id: &str) -> Result<(), StorageError> {
        let mut history = self.load_ai_history()?;
        history.sessions.retain(|session| session.id != session_id);
        history
            .messages
            .retain(|message| message.session_id != session_id);
        self.save_ai_history(history)
    }
    pub fn append_ai_audit(
        &self,
        request: AppendAiAuditRequest,
    ) -> Result<AiAuditLog, StorageError> {
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
            source: request.source,
            client: request.client,
            capability: request.capability,
            session_id: request.session_id,
            permission_mode: request.permission_mode,
            approval_decision: request.approval_decision,
            success: request.success,
            duration_ms: request.duration_ms,
            error: request.error,
            created_at: now_rfc3339(),
        };
        let mut file = self.load_ai_audit_file()?;
        file.logs.push(log.clone());
        trim_ai_audit(&mut file);
        let mut next = serde_json::to_value(file)?;
        let current = self.load_settings_doc_value(SETTINGS_AI_AUDIT, serde_json::Value::Null)?;
        merge_unknown_json(&current, &mut next);
        self.save_settings_doc_value(SETTINGS_AI_AUDIT, &next)?;
        Ok(log)
    }
    pub fn list_ai_audit_logs(
        &self,
        limit: Option<usize>,
    ) -> Result<Vec<AiAuditLog>, StorageError> {
        let mut logs = self.load_ai_audit_file()?.logs;
        logs.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        if let Some(limit) = limit {
            logs.truncate(limit);
        }
        Ok(logs)
    }
    fn load_ai_audit_file(&self) -> Result<AiAuditFile, StorageError> {
        self.read_json_table::<AiAuditFile>(SETTINGS_TABLE, SETTINGS_AI_AUDIT)
            .map(|file| file.unwrap_or_default())
    }
}

fn ai_session_title(user_input: &str) -> String {
    let title = user_input.chars().take(42).collect::<String>();
    let title = title.trim();
    if title.is_empty() {
        "AI Session".to_string()
    } else {
        title.to_string()
    }
}

#[cfg(test)]
mod tests {
    use nyaterm_core::{
        AiAgentKind, AiAuditFile, AiAuditLog, AiBackendKind, AiMessage, AiMessageRole,
        AiSessionBackendMetadata, AiSessionScope, AiSessionScopeType, AppendAiAuditRequest,
    };

    use super::SETTINGS_AI_HISTORY;
    use super::{ConnectionStore, SETTINGS_AI_AUDIT};
    use crate::storage::tests::unique_temp_dir;

    #[test]
    fn ai_history_round_trips_messages_and_deletes_session() {
        let dir = unique_temp_dir("ai-history");
        let store = ConnectionStore::open(&dir).expect("store");

        store
            .append_ai_user_message("session-1", Some("conn-1".to_string()), "  ".to_string())
            .expect("append user");
        let sessions = store.list_ai_sessions().expect("sessions");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title, "AI Session");
        assert_eq!(sessions[0].connection_id.as_deref(), Some("conn-1"));

        store
            .append_ai_message(AiMessage {
                id: "assistant-1".to_string(),
                session_id: "session-1".to_string(),
                role: AiMessageRole::Assistant,
                content: "hello".to_string(),
                created_at: "2026-04-28T00:00:01Z".to_string(),
                reasoning_content: Some("reasoning".to_string()),
                command_cards: Vec::new(),
            })
            .expect("append assistant");

        let messages = store.list_ai_messages("session-1").expect("messages");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, AiMessageRole::User);
        assert_eq!(messages[1].reasoning_content.as_deref(), Some("reasoning"));

        store
            .delete_ai_session("session-1")
            .expect("delete session");
        assert!(store.list_ai_sessions().expect("sessions").is_empty());
        assert!(
            store
                .list_ai_messages("session-1")
                .expect("messages")
                .is_empty()
        );

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn history_and_audit_saves_preserve_unknown_fields_by_record_id() {
        let dir = unique_temp_dir("ai-unknown-fields");
        let store = ConnectionStore::open(&dir).expect("store");
        let history = serde_json::json!({
            "futureRoot": "keep",
            "sessions": [{
                "id": "session-future",
                "connectionId": null,
                "title": "Future",
                "createdAt": "2026-01-01T00:00:00Z",
                "updatedAt": "2026-01-01T00:00:00Z",
                "futureSession": {"enabled": true}
            }],
            "messages": [{
                "id": "message-future",
                "sessionId": "session-future",
                "role": "assistant",
                "content": "fixture",
                "createdAt": "2026-01-01T00:00:00Z",
                "futureMessage": 7
            }]
        });
        store
            .save_settings_doc_value(SETTINGS_AI_HISTORY, &history)
            .unwrap();
        let typed = store.load_ai_history().unwrap();
        store.save_ai_history(typed).unwrap();
        let saved = store
            .load_settings_doc_value(SETTINGS_AI_HISTORY, serde_json::Value::Null)
            .unwrap();
        assert_eq!(saved["futureRoot"], "keep");
        assert_eq!(saved["sessions"][0]["futureSession"]["enabled"], true);
        assert_eq!(saved["messages"][0]["futureMessage"], 7);

        let audit = serde_json::json!({
            "futureAuditRoot": true,
            "logs": [{
                "id": "audit-future",
                "connectionId": null,
                "action": "fixture",
                "userInput": null,
                "generatedCommand": null,
                "riskLevel": null,
                "insertedToTerminal": false,
                "executed": false,
                "blocked": false,
                "createdAt": "2026-01-01T00:00:00Z",
                "futureAudit": "keep"
            }]
        });
        store
            .save_settings_doc_value(SETTINGS_AI_AUDIT, &audit)
            .unwrap();
        store
            .append_ai_audit(AppendAiAuditRequest {
                action: "fixture.append".into(),
                ..Default::default()
            })
            .unwrap();
        let saved = store
            .load_settings_doc_value(SETTINGS_AI_AUDIT, serde_json::Value::Null)
            .unwrap();
        assert_eq!(saved["futureAuditRoot"], true);
        assert_eq!(saved["logs"][0]["futureAudit"], "keep");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ai_history_persists_agent_kind_and_owner_scope() {
        let dir = unique_temp_dir("ai-history-scope");
        let store = ConnectionStore::open(&dir).expect("store");
        let scope = AiSessionScope {
            r#type: AiSessionScopeType::Terminal,
            target_id: Some("terminal-1".to_string()),
            connection_ids: vec!["connection-1".to_string()],
            label: Some("Fixture terminal".to_string()),
        };

        store
            .append_ai_user_message_scoped(
                "session-scope",
                Some("connection-1".to_string()),
                "inspect".to_string(),
                AiAgentKind::ClaudeCode,
                scope.clone(),
            )
            .expect("append scoped user message");
        let sessions = store.list_ai_sessions().expect("sessions");
        assert_eq!(sessions[0].agent_kind, AiAgentKind::ClaudeCode);
        assert_eq!(sessions[0].scope, scope);

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn rebind_preserves_messages_metadata_and_unknown_fields() {
        let dir = unique_temp_dir("ai-history-rebind");
        let store = ConnectionStore::open(&dir).expect("store");
        store
            .append_ai_user_message_scoped(
                "session-rebind",
                Some("connection-1".to_string()),
                "inspect".to_string(),
                AiAgentKind::Codex,
                AiSessionScope {
                    r#type: AiSessionScopeType::Terminal,
                    target_id: Some("terminal-1".to_string()),
                    connection_ids: vec!["connection-1".to_string()],
                    label: None,
                },
            )
            .expect("create session");
        let mut raw = store
            .load_settings_doc_value(SETTINGS_AI_HISTORY, serde_json::Value::Null)
            .expect("raw history");
        raw["sessions"][0]["futureSessionField"] = serde_json::json!("preserve");
        store
            .save_settings_doc_value(SETTINGS_AI_HISTORY, &raw)
            .expect("inject unknown field");

        let new_scope = AiSessionScope {
            r#type: AiSessionScopeType::Terminal,
            target_id: Some("terminal-2".to_string()),
            connection_ids: vec!["connection-2".to_string()],
            label: Some("Second terminal".to_string()),
        };
        let session = store
            .rebind_ai_session("session-rebind", new_scope.clone())
            .expect("rebind");
        assert_eq!(session.scope, new_scope);
        assert_eq!(session.agent_kind, AiAgentKind::Codex);
        assert_eq!(store.list_ai_messages("session-rebind").unwrap().len(), 1);
        let raw = store
            .load_settings_doc_value(SETTINGS_AI_HISTORY, serde_json::Value::Null)
            .expect("raw history");
        assert_eq!(raw["sessions"][0]["futureSessionField"], "preserve");
        assert!(store.rebind_ai_session("missing", new_scope).is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn scoped_ai_session_rejects_cross_terminal_or_agent_reuse() {
        let dir = unique_temp_dir("ai-history-scope-reuse");
        let store = ConnectionStore::open(&dir).expect("store");
        let scope = AiSessionScope {
            r#type: AiSessionScopeType::Terminal,
            target_id: Some("terminal-1".to_string()),
            ..Default::default()
        };
        store
            .append_ai_user_message_scoped(
                "session-scope",
                None,
                "first".to_string(),
                AiAgentKind::Nyaterm,
                scope.clone(),
            )
            .expect("first message");
        let mut other_scope = scope.clone();
        other_scope.target_id = Some("terminal-2".to_string());
        assert!(
            store
                .append_ai_user_message_scoped(
                    "session-scope",
                    None,
                    "wrong terminal".to_string(),
                    AiAgentKind::Nyaterm,
                    other_scope,
                )
                .is_err()
        );
        assert!(
            store
                .append_ai_user_message_scoped(
                    "session-scope",
                    None,
                    "wrong agent".to_string(),
                    AiAgentKind::Codex,
                    scope,
                )
                .is_err()
        );
        assert_eq!(store.list_ai_messages("session-scope").unwrap().len(), 1);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn ai_history_persists_codex_thread_metadata() {
        let dir = unique_temp_dir("ai-history-codex");
        let store = ConnectionStore::open(&dir).expect("store");
        store
            .append_ai_user_message_scoped(
                "session-codex",
                Some("terminal-1".to_string()),
                "inspect".to_string(),
                AiAgentKind::Codex,
                AiSessionScope::default(),
            )
            .expect("append Codex user message");
        store
            .set_ai_session_external_metadata(
                "session-codex",
                AiAgentKind::Codex,
                "thread-1".to_string(),
                AiSessionBackendMetadata {
                    backend: AiBackendKind::Codex,
                    external_thread_id: Some("thread-1".to_string()),
                    codex_terminal_tools_version: Some(1),
                },
            )
            .expect("save Codex metadata");
        let session = store.list_ai_sessions().unwrap().remove(0);
        assert_eq!(session.external_session_id.as_deref(), Some("thread-1"));
        assert_eq!(
            session
                .backend_metadata
                .as_ref()
                .and_then(|metadata| metadata.external_thread_id.as_deref()),
            Some("thread-1")
        );
        std::fs::remove_dir_all(dir).ok();
    }
    #[test]
    fn ai_audit_round_trips_sorts_and_limits_logs() {
        let dir = unique_temp_dir("ai-audit");
        let store = ConnectionStore::open(&dir).expect("store");

        let first = store
            .append_ai_audit(AppendAiAuditRequest {
                connection_id: Some("conn-1".to_string()),
                action: "generate_command".to_string(),
                user_input: Some("list files".to_string()),
                generated_command: Some("ls".to_string()),
                risk_level: Some(nyaterm_core::RiskLevel::Low),
                inserted_to_terminal: true,
                executed: false,
                blocked: false,
                source: Some("mcp".to_string()),
                client: Some("fixture-client".to_string()),
                capability: Some("terminal.execute".to_string()),
                session_id: Some("session-1".to_string()),
                permission_mode: Some(nyaterm_core::AiPermissionMode::Confirm),
                approval_decision: Some("allow_once".to_string()),
                success: Some(true),
                duration_ms: Some(42),
                error: None,
            })
            .expect("append audit");
        assert!(first.id.starts_with("audit-"));
        assert_eq!(first.source.as_deref(), Some("mcp"));
        assert_eq!(first.capability.as_deref(), Some("terminal.execute"));
        assert_eq!(
            first.permission_mode,
            Some(nyaterm_core::AiPermissionMode::Confirm)
        );
        assert_eq!(first.success, Some(true));
        assert_eq!(first.duration_ms, Some(42));

        let mut file = AiAuditFile::default();
        file.logs.push(first.clone());
        file.logs.push(AiAuditLog {
            id: "audit-later".to_string(),
            connection_id: None,
            action: "execute".to_string(),
            user_input: None,
            generated_command: None,
            risk_level: Some(nyaterm_core::RiskLevel::Medium),
            inserted_to_terminal: false,
            executed: true,
            blocked: false,
            created_at: "2999-01-01T00:00:00Z".to_string(),
            ..first.clone()
        });
        store
            .save_settings_doc_value(SETTINGS_AI_AUDIT, &serde_json::to_value(file).unwrap())
            .expect("save audit");

        let limited = store.list_ai_audit_logs(Some(1)).expect("audit logs");
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].id, "audit-later");

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn append_ai_message_replaces_the_earlier_projection_of_an_id() {
        let dir = unique_temp_dir("ai-message-projection-upsert");
        let store = ConnectionStore::open(&dir).expect("store");
        store
            .append_ai_user_message("session-1", None, "prompt".to_string())
            .expect("append user");

        let message = |content: &str| AiMessage {
            id: "assistant-1".to_string(),
            session_id: "session-1".to_string(),
            role: AiMessageRole::Assistant,
            content: content.to_string(),
            created_at: "2026-04-28T00:00:02Z".to_string(),
            reasoning_content: None,
            command_cards: Vec::new(),
        };
        store
            .append_ai_message(message("partial answer"))
            .expect("first projection");
        store
            .append_ai_message(message("final answer"))
            .expect("second projection");

        let messages = store.list_ai_messages("session-1").expect("messages");
        assert_eq!(messages.len(), 2);
        // The latest projection replaces the earlier row in place, preserving
        // the message's position in the transcript.
        assert_eq!(messages[1].id, "assistant-1");
        assert_eq!(messages[1].content, "final answer");

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn list_ai_messages_keeps_the_last_projection_of_a_duplicated_id() {
        let dir = unique_temp_dir("ai-message-projection-dedupe");
        let store = ConnectionStore::open(&dir).expect("store");
        store
            .append_ai_user_message("session-1", None, "prompt".to_string())
            .expect("append user");

        // Simulate history written before projections were replaced in place:
        // several rows share one id, and only the last snapshot is current.
        let mut history = store.load_ai_history().expect("history");
        for (index, content) in ["stale", "stale", "current"].iter().enumerate() {
            history.messages.push(AiMessage {
                id: "assistant-1".to_string(),
                session_id: "session-1".to_string(),
                role: AiMessageRole::Assistant,
                content: (*content).to_string(),
                created_at: format!("2026-04-28T00:00:0{index}Z"),
                reasoning_content: None,
                command_cards: Vec::new(),
            });
        }
        store.save_ai_history(history).expect("save");

        let messages = store.list_ai_messages("session-1").expect("messages");
        let assistant: Vec<_> = messages
            .iter()
            .filter(|message| message.id == "assistant-1")
            .collect();
        assert_eq!(assistant.len(), 1);
        assert_eq!(assistant[0].content, "current");
        // The surviving row keeps the last projection's position in the log.
        assert_eq!(
            messages.last().map(|message| message.id.as_str()),
            Some("assistant-1")
        );

        std::fs::remove_dir_all(dir).ok();
    }
}
