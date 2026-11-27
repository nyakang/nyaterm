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
                "mcp_elicitations": true,
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

/// Rebind a resumed thread to this request's MCP credential and approval policy.
pub fn codex_thread_resume_request_with_mcp(
    id: u64,
    thread_id: &str,
    model: Option<&str>,
    mcp: &CodexMcpConfig,
    language: &str,
) -> Value {
    let mut request = codex_thread_start_request_with_language(id, model, false, mcp, language);
    request["method"] = json!("thread/resume");
    let params = request["params"]
        .as_object_mut()
        .expect("thread parameters");
    params.remove("ephemeral");
    params.insert("threadId".into(), json!(thread_id));
    request
}

/// Codex's confirmation only permits the request to reach NyaTerm's capability
/// authorizer. It must never authorize native tools, another server, or a form
/// asking for user data. NyaTerm still applies Confirm/Observer and session scope.
pub fn codex_server_request_response(
    id: u64,
    method: &str,
    params: &Value,
    thread_id: Option<&str>,
    turn_id: Option<&str>,
) -> Value {
    let schema = &params["requestedSchema"];
    let matches_thread = thread_id.is_some_and(|thread| params["threadId"] == thread);
    let matches_turn = params["turnId"].is_null()
        || turn_id.is_none()
        || turn_id.is_some_and(|turn| params["turnId"] == turn);
    if method == "mcpServer/elicitation/request"
        && matches_thread
        && matches_turn
        && params["serverName"] == "nyaterm"
        && params["mode"] == "form"
        && params["_meta"]["codex_approval_kind"] == "mcp_tool_call"
        && params["_meta"]["codex_requires_user_input"] != true
        && schema["type"] == "object"
        && schema["properties"]
            .as_object()
            .is_some_and(|p| p.is_empty())
        && (schema["required"].is_null()
            || schema["required"].as_array().is_some_and(|r| r.is_empty()))
    {
        return json!({"id": id, "result": {"action": "accept", "content": null, "_meta": null}});
    }
    codex_decline_server_request(id, method)
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

    #[test]
    fn only_current_nyaterm_tool_confirmation_reaches_the_capability_authorizer() {
        use super::codex_server_request_response;
        use serde_json::json;
        let params = json!({"threadId":"thread", "turnId":"turn", "serverName":"nyaterm",
            "mode":"form", "_meta":{"codex_approval_kind":"mcp_tool_call", "persist":["session","always"]},
            "requestedSchema":{"type":"object","properties":{}}});
        let respond = |params: &serde_json::Value| {
            codex_server_request_response(
                7,
                "mcpServer/elicitation/request",
                params,
                Some("thread"),
                Some("turn"),
            )
        };
        assert_eq!(
            respond(&params)["result"],
            json!({"action":"accept","content":null,"_meta":null})
        );
        for (pointer, value) in [
            ("/threadId", json!("other")),
            ("/turnId", json!("old")),
            ("/serverName", json!("other")),
            ("/mode", json!("url")),
            ("/_meta/codex_approval_kind", json!("browser_auth")),
            (
                "/requestedSchema/properties",
                json!({"password":{"type":"string"}}),
            ),
        ] {
            let mut invalid = params.clone();
            *invalid.pointer_mut(pointer).unwrap() = value;
            assert_eq!(
                respond(&invalid)["result"]["action"],
                "decline",
                "{pointer}"
            );
        }
        assert_eq!(
            codex_server_request_response(7, "mcpServer/elicitation/request", &params, None, None)
                ["result"]["action"],
            "decline"
        );
        assert_eq!(
            codex_server_request_response(
                7,
                "item/commandExecution/requestApproval",
                &params,
                Some("thread"),
                Some("turn")
            )["result"]["decision"],
            "decline"
        );
    }

    #[test]
    fn resumed_threads_refresh_mcp_credentials_and_enable_host_scoped_confirmations() {
        use super::codex_thread_resume_request_with_mcp;
        let mcp = CodexMcpConfig {
            command: "nyaterm-mcp".into(),
            env: HashMap::from([("NYATERM_MCP_TOKEN".into(), "new-fixture-token".into())]),
        };
        let resumed = codex_thread_resume_request_with_mcp(2, "saved", None, &mcp, "en");
        assert_eq!(resumed["method"], "thread/resume");
        assert_eq!(resumed["params"]["threadId"], "saved");
        assert_eq!(
            resumed["params"]["approvalPolicy"]["granular"]["mcp_elicitations"],
            true
        );
        assert_eq!(
            resumed["params"]["config"]["mcp_servers"]["nyaterm"]["env"]["NYATERM_MCP_TOKEN"],
            "new-fixture-token"
        );
    }
}
