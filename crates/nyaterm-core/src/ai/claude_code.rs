use serde_json::Value;

use super::{
    AiChatRequest, AiPermissionMode, AiSettings, build_agent_prompt, sanitize_ai_diagnostic,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeCodeInvocation {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub prompt_stdin: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeCodeStreamEvent {
    SessionId(String),
    TextDelta(String),
    Error(String),
    Ignored,
}

#[derive(Debug, Default)]
pub struct ClaudeCodeStreamParser {
    last_partial: String,
    session_id: Option<String>,
}

impl ClaudeCodeStreamParser {
    pub fn parse_line(&mut self, line: &str) -> Result<Vec<ClaudeCodeStreamEvent>, String> {
        let value: Value = serde_json::from_str(line)
            .map_err(|error| format!("invalid Claude Code stream JSON: {error}"))?;
        let mut events = Vec::new();
        if self.session_id.is_none()
            && let Some(session_id) = extract_claude_session_id(&value)
        {
            self.session_id = Some(session_id.clone());
            events.push(ClaudeCodeStreamEvent::SessionId(session_id));
        }
        if let Some(error) = extract_claude_error(&value) {
            events.push(ClaudeCodeStreamEvent::Error(error));
            return Ok(events);
        }
        if let Some(delta) = extract_claude_text_delta(&value, &mut self.last_partial) {
            events.push(ClaudeCodeStreamEvent::TextDelta(delta));
        }
        if events.is_empty() {
            events.push(ClaudeCodeStreamEvent::Ignored);
        }
        Ok(events)
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }
}

pub fn build_claude_code_invocation(
    request: &AiChatRequest,
    settings: &AiSettings,
    mcp_config: Option<&Value>,
) -> ClaudeCodeInvocation {
    let mut args = vec![
        "-p".to_string(),
        "--verbose".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--include-partial-messages".to_string(),
        "--permission-mode".to_string(),
        claude_code_permission_mode(&request.permission_mode).to_string(),
        "--append-system-prompt".to_string(),
        claude_code_system_context(request),
    ];
    if let Some(model) = request
        .model_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            settings
                .claude_code
                .default_model
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
    {
        args.extend(["--model".to_string(), model.to_string()]);
    }
    if let Some(session_id) = request
        .existing_external_session_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        args.extend(["--resume".to_string(), session_id.to_string()]);
    }
    if let Some(config) = mcp_config {
        args.extend([
            "--mcp-config".to_string(),
            config.to_string(),
            "--strict-mcp-config".to_string(),
        ]);
    }
    let mut env = Vec::new();
    if let Some(directory) = settings
        .claude_code
        .config_directory
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        env.push(("CLAUDE_CONFIG_DIR".to_string(), directory.to_string()));
    }
    ClaudeCodeInvocation {
        args,
        env,
        prompt_stdin: build_agent_prompt(request, settings),
    }
}

pub fn claude_code_permission_mode(mode: &AiPermissionMode) -> &'static str {
    match mode {
        AiPermissionMode::Observer => "plan",
        AiPermissionMode::Confirm => "manual",
        AiPermissionMode::Auto | AiPermissionMode::FullAccess => "auto",
    }
}

pub fn claude_code_system_context(request: &AiChatRequest) -> String {
    let default_target = request
        .default_target_session_id
        .as_deref()
        .or(request.terminal_session_id.as_deref())
        .unwrap_or("none");
    format!(
        "{} {}",
        // Windows .cmd launchers reject line breaks in command-line arguments.
        super::prompts::external_system(&request.options.language).replace('\n', " "),
        super::prompts::default_session(&request.options.language, default_target)
    )
}

pub fn sanitize_claude_code_log_line(line: &str) -> String {
    sanitize_ai_diagnostic(line, 4096)
}

fn extract_claude_session_id(value: &Value) -> Option<String> {
    value
        .get("session_id")
        .or_else(|| value.get("sessionId"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn extract_claude_error(value: &Value) -> Option<String> {
    (value.get("type").and_then(Value::as_str) == Some("error"))
        .then(|| {
            value
                .get("message")
                .or_else(|| value.get("error"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .flatten()
}

fn extract_claude_text_delta(value: &Value, last_partial: &mut String) -> Option<String> {
    if let Some(delta) = value
        .pointer("/delta/text")
        .or_else(|| value.pointer("/delta/content"))
        .and_then(Value::as_str)
        .filter(|delta| !delta.is_empty())
    {
        return Some(delta.to_string());
    }
    let full = extract_message_text(value)?;
    if full.is_empty() || full == *last_partial {
        return None;
    }
    let delta = full
        .strip_prefix(last_partial.as_str())
        .unwrap_or(full.as_str())
        .to_string();
    *last_partial = full;
    (!delta.is_empty()).then_some(delta)
}

fn extract_message_text(value: &Value) -> Option<String> {
    if let Some(text) = value.get("text").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    let message = value.get("message").unwrap_or(value);
    let content = message.get("content")?.as_array()?;
    Some(
        content
            .iter()
            .filter_map(|item| {
                item.get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| matches!(kind, "text" | "output_text" | "assistant_text"))
                    .then(|| item.get("text").and_then(Value::as_str))
                    .flatten()
            })
            .collect::<Vec<_>>()
            .join(""),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        AiChatRequest, AiPermissionMode, AiSettings, ClaudeCodeStreamEvent, ClaudeCodeStreamParser,
        build_claude_code_invocation, sanitize_claude_code_log_line,
    };
    use crate::{AiAction, AiAgentKind, AiMode};

    fn request() -> AiChatRequest {
        AiChatRequest {
            stream_id: None,
            session_id: Some("chat-1".into()),
            connection_id: None,
            terminal_session_id: Some("terminal-1".into()),
            owner_scope: Default::default(),
            targets: Vec::new(),
            target_contexts: Vec::new(),
            mode: AiMode::Agent,
            agent_kind: AiAgentKind::ClaudeCode,
            permission_mode: AiPermissionMode::FullAccess,
            model_id: None,
            model_name: Some("claude-fixture".into()),
            default_target_session_id: Some("terminal-1".into()),
            existing_external_session_id: Some("claude-session".into()),
            attachments: Vec::new(),
            action: AiAction::GenerateCommand,
            user_input: "inspect".into(),
            context: Default::default(),
            options: Default::default(),
        }
    }

    #[test]
    fn invocation_keeps_prompt_on_stdin_and_never_bypasses_permissions() {
        let config = serde_json::json!({ "mcpServers": { "nyaterm": {} } });
        let invocation =
            build_claude_code_invocation(&request(), &AiSettings::default(), Some(&config));
        assert!(!invocation.args.contains(&invocation.prompt_stdin));
        assert!(
            invocation
                .args
                .iter()
                .any(|arg| arg == "--strict-mcp-config")
        );
        assert!(!invocation.args.iter().any(|arg| arg == "bypassPermissions"));
        assert!(
            invocation
                .args
                .windows(2)
                .any(|pair| pair == ["--permission-mode", "auto"])
        );
    }

    #[test]
    fn invocation_uses_non_blank_requested_model_before_default() {
        let mut settings = AiSettings::default();
        settings.claude_code.default_model = Some("claude-default".into());
        let mut request = request();
        request.model_name = Some("  claude-fixture  ".into());
        let invocation = build_claude_code_invocation(&request, &settings, None);
        assert!(
            invocation
                .args
                .windows(2)
                .any(|pair| pair == ["--model", "claude-fixture"])
        );
    }

    #[test]
    fn invocation_falls_back_to_default_for_blank_requested_model() {
        let mut settings = AiSettings::default();
        settings.claude_code.default_model = Some("  claude-default  ".into());
        for requested in [None, Some(String::new()), Some("   ".to_string())] {
            let mut request = request();
            request.model_name = requested;
            let invocation = build_claude_code_invocation(&request, &settings, None);
            assert!(
                invocation
                    .args
                    .windows(2)
                    .any(|pair| pair == ["--model", "claude-default"])
            );
        }
    }

    #[test]
    fn invocation_omits_model_when_request_and_default_are_blank() {
        let mut request = request();
        for requested in [None, Some(String::new()), Some(" \t".into())] {
            request.model_name = requested;
            for default in [None, Some(String::new()), Some(" \t".into())] {
                let mut settings = AiSettings::default();
                settings.claude_code.default_model = default;
                let invocation = build_claude_code_invocation(&request, &settings, None);
                assert!(!invocation.args.iter().any(|arg| arg == "--model"));
            }
        }
    }

    #[test]
    fn claude_prompts_require_non_interactive_terminal_commands() {
        let invocation = build_claude_code_invocation(&request(), &AiSettings::default(), None);
        let system_prompt = invocation
            .args
            .windows(2)
            .find(|pair| pair[0] == "--append-system-prompt")
            .map(|pair| pair[1].as_str())
            .expect("system prompt");
        assert!(system_prompt.contains("git --no-pager"));
        assert!(system_prompt.contains("journalctl --no-pager"));
        assert!(invocation.prompt_stdin.contains("git --no-pager"));
    }

    #[test]
    fn parser_diffs_partial_messages_and_extracts_session_and_errors() {
        let mut parser = ClaudeCodeStreamParser::default();
        let first = parser
            .parse_line(
                r#"{"session_id":"s1","message":{"content":[{"type":"text","text":"hello"}]}}"#,
            )
            .unwrap();
        assert!(first.contains(&ClaudeCodeStreamEvent::SessionId("s1".into())));
        assert!(first.contains(&ClaudeCodeStreamEvent::TextDelta("hello".into())));
        let second = parser
            .parse_line(r#"{"message":{"content":[{"type":"text","text":"hello world"}]}}"#)
            .unwrap();
        assert!(second.contains(&ClaudeCodeStreamEvent::TextDelta(" world".into())));
        let error = parser
            .parse_line(r#"{"type":"error","message":"failed"}"#)
            .unwrap();
        assert!(error.contains(&ClaudeCodeStreamEvent::Error("failed".into())));
    }

    #[test]
    fn sanitizes_claude_auth_material() {
        let value = sanitize_claude_code_log_line(
            "access_token=a refresh_token=b id_token=c api_key=d code=e&state=ok",
        );
        for secret in ["=a", "=b", "=c", "=d", "=e"] {
            assert!(!value.contains(secret));
        }
    }
}
