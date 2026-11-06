use std::collections::HashMap;

use serde_json::{Value, json};

use super::{AiChatRequest, AiSettings, build_agent_prompt, sanitize_ai_diagnostic, uuid};

pub const CODEX_TERMINAL_TOOLS_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq)]
pub enum CodexServerEvent {
    Response {
        id: u64,
        result: Result<Value, String>,
    },
    ServerRequest {
        id: u64,
        method: String,
        params: Value,
    },
    TextDelta {
        turn_id: String,
        delta: String,
    },
    ReasoningDelta {
        turn_id: String,
        delta: String,
    },
    TurnCompleted {
        turn_id: String,
        status: String,
        text: String,
        error: Option<String>,
        usage: Option<Value>,
    },
    Error(String),
    Ignored,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexMcpConfig {
    pub command: String,
    pub env: HashMap<String, String>,
}

pub fn codex_initialize_request(id: u64, version: &str) -> Value {
    json!({
        "method": "initialize",
        "id": id,
        "params": {
            "clientInfo": { "name": "nyaterm", "title": "NyaTerm", "version": version },
            "capabilities": { "experimentalApi": true }
        }
    })
}

pub fn codex_initialized_notification() -> Value {
    json!({ "method": "initialized", "params": {} })
}

pub fn codex_thread_start_request(
    id: u64,
    model: Option<&str>,
    ephemeral: bool,
    mcp: &CodexMcpConfig,
) -> Value {
    codex_thread_start_request_with_language(id, model, ephemeral, mcp, "en")
}

pub fn codex_thread_start_request_with_language(
    id: u64,
    model: Option<&str>,
    ephemeral: bool,
    mcp: &CodexMcpConfig,
    language: &str,
) -> Value {
    let mut params = json!({
        "cwd": null,
        "ephemeral": ephemeral,
        "approvalPolicy": {
            "granular": {
                "rules": false,
                "mcp_elicitations": false,
                "request_permissions": false,
                "sandbox_approval": false
            }
        },
        "approvalsReviewer": "user",
        "sandbox": "read-only",
        "developerInstructions": codex_developer_instructions_with_language(language),
        "config": {
            "mcp_servers": {
                "nyaterm": {
                    "command": mcp.command,
                    "args": [],
                    "env": mcp.env,
                    "required": true
                }
            }
        }
    });
    if let Some(model) = non_empty(model) {
        params["model"] = json!(model);
    }
    json!({ "method": "thread/start", "id": id, "params": params })
}

pub fn codex_thread_resume_request(id: u64, thread_id: &str) -> Value {
    json!({ "method": "thread/resume", "id": id, "params": { "threadId": thread_id } })
}

pub fn codex_thread_resume_request_with_language(
    id: u64,
    thread_id: &str,
    language: &str,
) -> Value {
    json!({ "method": "thread/resume", "id": id, "params": {
        "threadId": thread_id,
        "developerInstructions": codex_developer_instructions_with_language(language)
    } })
}

pub fn codex_turn_start_request(
    id: u64,
    thread_id: &str,
    prompt: &str,
    model: Option<&str>,
) -> Value {
    let mut params = json!({
        "threadId": thread_id,
        "clientUserMessageId": format!("msg-{}", uuid()),
        "input": [{ "type": "text", "text": prompt, "text_elements": [] }]
    });
    if let Some(model) = non_empty(model) {
        params["model"] = json!(model);
    }
    json!({ "method": "turn/start", "id": id, "params": params })
}

pub fn codex_turn_interrupt_request(id: u64, thread_id: &str, turn_id: &str) -> Value {
    json!({
        "method": "turn/interrupt",
        "id": id,
        "params": { "threadId": thread_id, "turnId": turn_id }
    })
}

pub fn codex_decline_server_request(id: u64, method: &str) -> Value {
    let result = match method {
        "mcpServer/elicitation/request" => json!({ "action": "decline", "content": null }),
        "item/tool/call" => json!({
            "success": false,
            "contentItems": [{ "type": "inputText", "text": "Dynamic tools are disabled; use the configured NyaTerm MCP server." }]
        }),
        _ => json!({ "decision": "decline" }),
    };
    json!({ "id": id, "result": result })
}

pub fn parse_codex_server_line(line: &str) -> Result<CodexServerEvent, String> {
    let value: Value = serde_json::from_str(line)
        .map_err(|error| format!("invalid Codex app-server JSONL: {error}"))?;
    if let Some(id) = value.get("id").and_then(Value::as_u64) {
        if let Some(method) = value.get("method").and_then(Value::as_str) {
            return Ok(CodexServerEvent::ServerRequest {
                id,
                method: method.to_string(),
                params: value.get("params").cloned().unwrap_or(Value::Null),
            });
        }
        let result = if let Some(error) = value.get("error") {
            Err(error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Codex app-server request failed")
                .to_string())
        } else {
            Ok(value.get("result").cloned().unwrap_or(Value::Null))
        };
        return Ok(CodexServerEvent::Response { id, result });
    }

    let Some(method) = value.get("method").and_then(Value::as_str) else {
        return Ok(CodexServerEvent::Ignored);
    };
    let params = value.get("params").cloned().unwrap_or(Value::Null);
    match method {
        "item/agentMessage/delta" => Ok(CodexServerEvent::TextDelta {
            turn_id: string_field(&params, "turnId"),
            delta: string_field(&params, "delta"),
        }),
        "item/reasoning/delta" | "item/reasoning/summaryDelta" => {
            Ok(CodexServerEvent::ReasoningDelta {
                turn_id: string_field(&params, "turnId"),
                delta: params
                    .get("delta")
                    .or_else(|| params.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
        }
        "turn/completed" => {
            let turn = params.get("turn").cloned().unwrap_or(Value::Null);
            Ok(CodexServerEvent::TurnCompleted {
                turn_id: string_field(&turn, "id"),
                status: string_field(&turn, "status"),
                text: codex_final_agent_text(&turn),
                error: turn
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                usage: turn.get("usage").cloned(),
            })
        }
        "error" => Ok(CodexServerEvent::Error(
            params
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("Codex request failed")
                .to_string(),
        )),
        _ => Ok(CodexServerEvent::Ignored),
    }
}

pub fn build_codex_agent_prompt(request: &AiChatRequest, settings: &AiSettings) -> String {
    format!(
        "{}\n\n{}",
        build_agent_prompt(request, settings),
        codex_developer_instructions_with_language(&request.options.language)
    )
}

pub fn codex_developer_instructions() -> &'static str {
    codex_developer_instructions_with_language("en")
}

pub fn codex_developer_instructions_with_language(language: &str) -> &'static str {
    super::prompts::external_system(language)
}

pub fn codex_final_agent_text(turn: &Value) -> String {
    turn.get("items")
        .and_then(Value::as_array)
        .and_then(|items| {
            items.iter().rev().find_map(|item| {
                (item.get("type").and_then(Value::as_str) == Some("agentMessage"))
                    .then(|| item.get("text").and_then(Value::as_str))
                    .flatten()
                    .map(ToOwned::to_owned)
            })
        })
        .unwrap_or_default()
}

pub fn sanitize_codex_log_line(line: &str) -> String {
    sanitize_ai_diagnostic(line, 4096)
}

fn string_field(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        CodexMcpConfig, CodexServerEvent, build_codex_agent_prompt, codex_developer_instructions,
        codex_thread_start_request, parse_codex_server_line, sanitize_codex_log_line,
    };
    use crate::AiSettings;
    use crate::ai::tests::sample_ai_request;

    #[test]
    fn codex_prompts_require_non_interactive_terminal_commands() {
        let prompt = build_codex_agent_prompt(&sample_ai_request("en"), &AiSettings::default());
        for value in [prompt.as_str(), codex_developer_instructions()] {
            assert!(value.contains("git --no-pager"));
            assert!(value.contains("journalctl --no-pager"));
        }
    }

    #[test]
    fn parses_codex_deltas_completion_and_errors() {
        assert_eq!(
            parse_codex_server_line(
                r#"{"method":"item/agentMessage/delta","params":{"turnId":"t1","delta":"hi"}}"#
            )
            .unwrap(),
            CodexServerEvent::TextDelta {
                turn_id: "t1".into(),
                delta: "hi".into()
            }
        );
        let completed = parse_codex_server_line(
            r#"{"method":"turn/completed","params":{"turn":{"id":"t1","status":"completed","items":[{"type":"agentMessage","text":"done"}]}}}"#,
        )
        .unwrap();
        assert!(
            matches!(completed, CodexServerEvent::TurnCompleted { text, .. } if text == "done")
        );
        assert!(matches!(
            parse_codex_server_line(r#"{"method":"error","params":{"error":{"message":"failed"}}}"#).unwrap(),
            CodexServerEvent::Error(message) if message == "failed"
        ));
        assert!(parse_codex_server_line("not-json").is_err());
    }

    #[test]
    fn thread_params_are_read_only_and_use_ephemeral_mcp_environment() {
        let mcp = CodexMcpConfig {
            command: "nyaterm-mcp".into(),
            env: HashMap::from([("NYATERM_MCP_EPHEMERAL".into(), "1".into())]),
        };
        let request = codex_thread_start_request(2, Some("gpt-5-codex"), false, &mcp);
        let params = &request["params"];
        assert_eq!(params["sandbox"], "read-only");
        assert_eq!(params["config"]["mcp_servers"]["nyaterm"]["required"], true);
        assert_eq!(
            params["config"]["mcp_servers"]["nyaterm"]["env"]["NYATERM_MCP_EPHEMERAL"],
            "1"
        );
    }

    #[test]
    fn sanitizes_all_codex_auth_markers() {
        let sanitized = sanitize_codex_log_line(
            "access_token=a refresh_token=b id_token=c api_key=d code=e&state=ok",
        );
        for secret in ["=a", "=b", "=c", "=d", "=e"] {
            assert!(!sanitized.contains(secret));
        }
        assert!(sanitized.contains("code=[REDACTED]&state=ok"));
    }
}
