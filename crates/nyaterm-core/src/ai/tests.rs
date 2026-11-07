use std::collections::HashSet;

use super::{
    AI_AUDIT_MAX_LOGS, AI_HISTORY_MAX_MESSAGES, AI_HISTORY_MAX_SESSIONS,
    AI_REQUEST_USER_AGENT_DEFAULT, AiAction, AiAuditFile, AiAuditLog, AiChatRequest, AiContext,
    AiHistoryFile, AiMessage, AiMessageRole, AiMode, AiModelError, AiProviderCredential,
    AiProviderKind, AiRequestOptions, AiSession, AiSettings, RiskLevel, agent_system_prompt,
    build_agent_prompt, build_prompt, effective_ai_request_user_agent, extract_text_from_assistant,
    genai_model_name, normalize_ai_settings, parse_model_output, redact_context,
    resolve_request_model, system_prompt, trim_ai_audit, trim_ai_history,
};
#[test]
fn provider_debug_output_redacts_api_keys() {
    let secret = "nya-ai-key-never-log";
    let credential = AiProviderCredential {
        icon_data_url: None,
        api_protocol: None,
        api_format: Default::default(),
        id: "credential-1".to_string(),
        name: "Test".to_string(),
        provider_kind: AiProviderKind::Openai,
        base_url: None,
        api_key: Some(secret.to_string().into()),
        extra_headers: Vec::new(),
        enabled: true,
    };
    let output = format!("{credential:?}");

    assert!(!output.contains(secret));
    assert!(output.contains("<redacted>"));
}

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
            agent_kind: Default::default(),
            scope: Default::default(),
            external_session_id: None,
            backend_metadata: None,
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

    trim_ai_history(&mut history);

    assert_eq!(history.sessions.len(), AI_HISTORY_MAX_SESSIONS);
    assert_eq!(history.messages.len(), AI_HISTORY_MAX_MESSAGES);
    let retained_sessions: HashSet<&str> = history
        .sessions
        .iter()
        .map(|session| session.id.as_str())
        .collect();
    assert!(!retained_sessions.contains("s-000"));
    assert!(retained_sessions.contains("s-219"));
    assert!(
        history
            .messages
            .iter()
            .all(|message| retained_sessions.contains(message.session_id.as_str()))
    );
}

#[test]
fn trims_ai_audit_to_latest_entries() {
    let mut file = AiAuditFile::default();
    for index in 0..(AI_AUDIT_MAX_LOGS + 10) {
        file.logs.push(AiAuditLog {
            id: format!("audit-{index}"),
            connection_id: None,
            action: "generate_command".to_string(),
            user_input: None,
            generated_command: None,
            risk_level: None,
            inserted_to_terminal: false,
            executed: false,
            blocked: false,
            created_at: format!("2026-04-28T00:00:{:02}Z", index % 60),
            ..Default::default()
        });
    }

    trim_ai_audit(&mut file);

    assert_eq!(file.logs.len(), AI_AUDIT_MAX_LOGS);
    assert_eq!(file.logs[0].id, "audit-10");
}

#[test]
fn redacts_sensitive_values_in_context() {
    let mut context = AiContext {
        recent_output:
            "password=secret token:abc Authorization: Bearer abc.def AKIA1234567890ABCDEF"
                .to_string(),
        selected_text: "postgres://user:pass@localhost/db".to_string(),
        input_buffer: "api_key=real".to_string(),
        ..AiContext::default()
    };

    redact_context(&mut context);

    assert!(!context.recent_output.contains("secret"));
    assert!(!context.recent_output.contains("abc.def"));
    assert!(!context.recent_output.contains("AKIA1234567890ABCDEF"));
    assert_eq!(context.selected_text, "postgres://[REDACTED]@localhost/db");
    assert_eq!(context.input_buffer, "api_key=[REDACTED]");
}

#[test]
fn parses_json_command_cards() {
    let raw = r#"{"text":"ok","commandCards":[{"id":"1","title":"CPU","command":"ps aux","explanation":"x","riskLevel":"low","riskReason":"read only","expectedEffect":"list","rollback":"none"}]}"#;
    let (text, reasoning, cards) = parse_model_output(raw, None);
    assert_eq!(text, "ok");
    assert_eq!(reasoning, None);
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].risk_level, Some(RiskLevel::Low));
}

#[test]
fn parser_extracts_think_block_and_keeps_markdown_on_json_failure() {
    let (text, reasoning, cards) =
        parse_model_output("<think>step 1\nstep 2</think>final answer", None);
    assert_eq!(text, "final answer");
    assert_eq!(reasoning.as_deref(), Some("step 1\nstep 2"));
    assert!(cards.is_empty());

    let markdown = "## Summary\n\n- item 1\n- item 2";
    let (text, reasoning, cards) = parse_model_output(markdown, None);
    assert_eq!(text, markdown);
    assert_eq!(reasoning, None);
    assert!(cards.is_empty());
}

#[test]
fn parser_promotes_reasoning_json_when_text_is_empty() {
    let reasoning = r#"{"text":"answer from reasoning","commandCards":[]}"#.to_string();
    let (text, reasoning, cards) = parse_model_output("", Some(reasoning));
    assert_eq!(text, "answer from reasoning");
    assert_eq!(reasoning, None);
    assert!(cards.is_empty());
}

#[test]
fn extract_text_from_assistant_prefers_json_text() {
    let content = r#"```json
{"text":"visible","reasoning":"hidden","commandCards":[]}
```"#;
    assert_eq!(extract_text_from_assistant(content), "visible");
    assert_eq!(extract_text_from_assistant("plain"), "plain");
}

#[test]
fn prompt_builder_uses_locale_and_context() {
    let request = sample_ai_request("zh_CN");
    let settings = AiSettings::default();

    let system = system_prompt("zh_CN");
    let prompt = build_prompt(&request, &settings);
    let agent_prompt = build_agent_prompt(&request, &settings);

    assert!(system.contains("终端助手"));
    assert!(prompt.contains("任务："));
    assert!(prompt.contains("db-01"));
    assert!(prompt.contains("最多命令数: 5"));
    assert!(agent_prompt.contains("有依赖的操作必须等待前序结果"));
    assert!(agent_system_prompt("en-US").contains("terminal automation agent"));
}

#[test]
fn resolves_requested_ai_model_with_credential() {
    let mut settings = AiSettings::default();
    settings.provider_profiles[0].enabled = true;
    settings.provider_credentials[0].enabled = true;
    settings.provider_credentials[0].api_key = Some("key".to_string().into());
    normalize_ai_settings(&mut settings);
    let model_id = "openai:gpt-4o-mini".to_string();
    settings.default_model_id = Some(model_id.clone());

    let mut request = sample_ai_request("en");
    request.model_id = Some(model_id);
    let resolved = resolve_request_model(&settings, &request).expect("resolve model");

    assert_eq!(resolved.model_name, "gpt-4o-mini");
    assert_eq!(resolved.provider_kind, AiProviderKind::Openai);
    assert_eq!(
        resolved
            .credential
            .as_ref()
            .and_then(|credential| credential.api_key.as_deref()),
        Some("key")
    );
}

#[test]
fn resolve_model_allows_ollama_without_api_key() {
    let mut settings = AiSettings {
        active_profile_id: "ollama".to_string(),
        ..AiSettings::default()
    };
    settings.provider_profiles[4].enabled = true;
    settings.provider_credentials[4].enabled = true;
    normalize_ai_settings(&mut settings);
    settings.default_model_id = Some("ollama:llama3-7b".to_string());

    let resolved = resolve_request_model(&settings, &sample_ai_request("en"))
        .expect("ollama should not require api key");

    assert_eq!(resolved.provider_kind, AiProviderKind::Ollama);
}

#[test]
fn resolve_model_reports_missing_api_key() {
    let mut settings = AiSettings::default();
    settings.provider_profiles[0].enabled = true;
    settings.provider_credentials[0].enabled = true;
    normalize_ai_settings(&mut settings);
    settings.default_model_id = Some("openai:gpt-4o-mini".to_string());

    let error = resolve_request_model(&settings, &sample_ai_request("en")).unwrap_err();

    assert_eq!(
        error,
        AiModelError::MissingApiKey {
            credential: "OpenAI".to_string()
        }
    );
}

#[test]
fn user_agent_and_deepseek_mapping_match_legacy() {
    let mut settings = AiSettings {
        request_user_agent: "   ".to_string(),
        ..AiSettings::default()
    };
    assert_eq!(
        effective_ai_request_user_agent(&settings),
        AI_REQUEST_USER_AGENT_DEFAULT
    );
    settings.request_user_agent = "nyaterm-test/1.0".to_string();
    assert_eq!(
        effective_ai_request_user_agent(&settings),
        "nyaterm-test/1.0"
    );

    assert_eq!(
        genai_model_name(&AiProviderKind::Deepseek, "deepseek-v4-flash-none"),
        "deepseek-v4-flash"
    );
    assert_eq!(
        genai_model_name(&AiProviderKind::Openai, "gpt-test-none"),
        "gpt-test-none"
    );
}

pub(super) fn sample_ai_request(language: &str) -> AiChatRequest {
    AiChatRequest {
        owner_scope: Default::default(),
        targets: Vec::new(),
        target_contexts: Vec::new(),
        agent_kind: Default::default(),
        permission_mode: Default::default(),
        default_target_session_id: None,
        existing_external_session_id: None,
        attachments: Vec::new(),
        stream_id: None,
        session_id: Some("session-1".to_string()),
        connection_id: Some("connection-1".to_string()),
        terminal_session_id: Some("terminal-1".to_string()),
        mode: AiMode::Ask,
        model_id: None,
        model_name: None,
        action: AiAction::GenerateCommand,
        user_input: "show disk usage".to_string(),
        context: AiContext {
            connection_name: Some("prod".to_string()),
            host: Some("db-01".to_string()),
            port: Some(22),
            username: Some("root".to_string()),
            cwd: Some("/srv".to_string()),
            os: Some("linux".to_string()),
            arch: Some("x86_64".to_string()),
            recent_output: "df -h".to_string(),
            selected_text: "/srv/data".to_string(),
            input_buffer: String::new(),
        },
        options: AiRequestOptions {
            language: language.to_string(),
            ..AiRequestOptions::default()
        },
    }
}

pub(super) fn sample_ai_history() -> Vec<AiMessage> {
    vec![
        AiMessage {
            id: "m1".to_string(),
            session_id: "session-1".to_string(),
            role: AiMessageRole::User,
            content: "previous question".to_string(),
            created_at: "2026-04-28T00:00:00Z".to_string(),
            reasoning_content: None,
            command_cards: vec![],
        },
        AiMessage {
            id: "m2".to_string(),
            session_id: "session-1".to_string(),
            role: AiMessageRole::Assistant,
            content: r#"{"text":"previous answer","commandCards":[]}"#.to_string(),
            created_at: "2026-04-28T00:00:01Z".to_string(),
            reasoning_content: None,
            command_cards: vec![],
        },
    ]
}
