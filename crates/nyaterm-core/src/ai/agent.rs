//! Agent loop protocol.
//!
//! Split out of `ai.rs` by domain: the tool schemas each provider is given,
//! how the model's reply is turned back into an action, and whether that
//! action may run unattended. The tool schemas, the parse fallbacks and the
//! legacy reply parsing remains for compatibility; new runs use the shared registry.

use serde::Deserialize;

use super::{
    AgentApprovalDecision, AgentCommandRiskAssessment, AgentLlmResponse, AiChatRequest,
    AiModelError, AiSettings, AiToolCall, CommandObservation, RiskLevel, assess_local_command_risk,
    deserialize_required_risk_level, extract_json_object, max_risk, risk_label,
};

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ExecuteCommandToolArgs {
    thought: String,
    command: String,
    #[serde(default)]
    target_terminal_session_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_risk_level")]
    risk_level: RiskLevel,
    risk_reason: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct FinalAnswerToolArgs {
    thought: String,
    answer: String,
}

pub(super) fn agent_openai_tools(tools: &[super::harness::AgentTool]) -> serde_json::Value {
    use super::harness::AgentToolRegistry;
    serde_json::Value::Array(
        tools
            .iter()
            .map(|tool| {
                let schema = AgentToolRegistry::schema(*tool);
                serde_json::json!({"type":"function","function":{
                    "name":tool.name(),"description":AgentToolRegistry::description(*tool),
                    "strict":false,"parameters":schema
                }})
            })
            .collect(),
    )
}

pub(super) fn agent_anthropic_tools(tools: &[super::harness::AgentTool]) -> serde_json::Value {
    use super::harness::AgentToolRegistry;
    serde_json::Value::Array(
        tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "name":tool.name(),"description":AgentToolRegistry::description(*tool),
                    "input_schema":AgentToolRegistry::schema(*tool)
                })
            })
            .collect(),
    )
}

pub(super) fn agent_gemini_tools(tools: &[super::harness::AgentTool]) -> serde_json::Value {
    use super::harness::AgentToolRegistry;
    serde_json::json!([{"functionDeclarations":tools.iter().map(|tool| {
        let mut schema = AgentToolRegistry::schema(*tool);
        if let Some(object) = schema.as_object_mut() { object.remove("$schema"); }
        serde_json::json!({"name":tool.name(),"description":AgentToolRegistry::description(*tool),"parameters":schema})
    }).collect::<Vec<_>>()}])
}

pub fn parse_agent_model_output(raw_text: &str) -> Result<AgentLlmResponse, AiModelError> {
    let candidate = extract_json_object(raw_text).unwrap_or_else(|| raw_text.trim().to_string());
    let response: AgentLlmResponse = serde_json::from_str(&candidate)
        .map_err(|error| AiModelError::InvalidChatJson(error.to_string()))?;
    let action = response.action.trim().to_ascii_lowercase();
    match action.as_str() {
        "execute_command" => {
            if response
                .command
                .as_deref()
                .map(str::trim)
                .filter(|command| !command.is_empty())
                .is_none()
            {
                return Err(AiModelError::MissingChatContent);
            }
        }
        "final_answer" => {
            if response
                .answer
                .as_deref()
                .map(str::trim)
                .filter(|answer| !answer.is_empty())
                .is_none()
            {
                return Err(AiModelError::MissingChatContent);
            }
        }
        _ => {
            return Err(AiModelError::InvalidChatJson(format!(
                "unknown AI agent action '{}'",
                response.action
            )));
        }
    }
    Ok(response)
}

pub fn parse_agent_tool_call(tool_calls: &[AiToolCall]) -> Result<AgentLlmResponse, AiModelError> {
    let calls = tool_calls
        .iter()
        .filter(|call| !call.name.trim().is_empty())
        .collect::<Vec<_>>();
    if calls.len() != 1 {
        return Err(AiModelError::InvalidChatJson(format!(
            "expected exactly one AI agent tool call, got {}",
            calls.len()
        )));
    }
    let call = calls[0];
    match call.name.as_str() {
        "execute_command" => {
            let args: ExecuteCommandToolArgs = serde_json::from_value(call.arguments.clone())
                .map_err(|error| AiModelError::InvalidChatJson(error.to_string()))?;
            if args.command.trim().is_empty() {
                return Err(AiModelError::MissingChatContent);
            }
            Ok(AgentLlmResponse {
                target_terminal_session_id: args.target_terminal_session_id,
                thought: args.thought,
                action: "execute_command".to_string(),
                command: Some(args.command),
                risk_level: Some(args.risk_level),
                risk_reason: Some(args.risk_reason),
                answer: None,
            })
        }
        "final_answer" => {
            let args: FinalAnswerToolArgs = serde_json::from_value(call.arguments.clone())
                .map_err(|error| AiModelError::InvalidChatJson(error.to_string()))?;
            if args.answer.trim().is_empty() {
                return Err(AiModelError::MissingChatContent);
            }
            Ok(AgentLlmResponse {
                target_terminal_session_id: None,
                thought: args.thought,
                action: "final_answer".to_string(),
                command: None,
                risk_level: None,
                risk_reason: None,
                answer: Some(args.answer),
            })
        }
        other => Err(AiModelError::InvalidChatJson(format!(
            "unknown AI agent tool call '{other}'"
        ))),
    }
}

pub fn agent_response_action(response: &AgentLlmResponse) -> &str {
    match response.action.trim().to_ascii_lowercase().as_str() {
        "execute_command" => "execute_command",
        "final_answer" => "final_answer",
        _ => "unknown",
    }
}

pub fn assess_agent_command_risk(
    response: &AgentLlmResponse,
    command: &str,
) -> AgentCommandRiskAssessment {
    let model_risk = response.risk_level.clone().unwrap_or(RiskLevel::Medium);
    let (local_risk, local_reason) = assess_local_command_risk(command);
    let effective_risk = max_risk(model_risk.clone(), local_risk.clone());
    let risk_reason = response
        .risk_reason
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            format!(
                "model {}: {}; local {}: {}",
                risk_label(&model_risk),
                value.trim(),
                risk_label(&local_risk),
                local_reason,
            )
        })
        .or_else(|| Some(format!("local {}: {local_reason}", risk_label(&local_risk))));

    AgentCommandRiskAssessment {
        auto_executable: crate::capabilities::assess_command_risk(command).auto_executable,
        model_risk,
        local_risk,
        effective_risk,
        risk_reason,
    }
}

pub fn decide_agent_command_execution(
    settings: &AiSettings,
    assessment: &AgentCommandRiskAssessment,
) -> (AgentApprovalDecision, Option<String>) {
    let risk = crate::capabilities::RiskAssessment {
        level: assessment.effective_risk.clone(),
        reason: "effective command risk".into(),
        auto_executable: assessment.auto_executable,
    };
    let decision = crate::capabilities::decide_native_policy(
        &settings.agent_command_execution_mode,
        &settings.agent_smart_auto_execute_max_risk,
        crate::capabilities::CapabilityAccess::Write,
        Some(&risk),
    );
    if decision == crate::capabilities::PolicyDecision::Allow {
        (AgentApprovalDecision::Auto, None)
    } else {
        (
            AgentApprovalDecision::NeedsApproval,
            Some("execution policy requires explicit confirmation".into()),
        )
    }
}

pub fn build_agent_prompt(request: &AiChatRequest, settings: &AiSettings) -> String {
    super::prompts::user_prompt(request, settings, true)
}

pub fn build_observation_message(
    obs: &CommandObservation,
    command: &str,
    language: &str,
) -> String {
    super::prompts::observation(obs, command, language)
}

pub fn build_agent_rejection_message(command: &str, language: &str) -> String {
    super::prompts::rejected(command, language)
}

pub fn build_agent_tool_result_message(
    call_id: &str,
    value: &serde_json::Value,
    language: &str,
) -> String {
    format!(
        "{} [{call_id}]:\n{value}",
        super::prompts::catalog(language).tool_result
    )
}

#[cfg(test)]
mod tests {
    use super::{
        AgentApprovalDecision, AiSettings, CommandObservation, RiskLevel, agent_anthropic_tools,
        agent_gemini_tools, agent_openai_tools, agent_response_action, assess_agent_command_risk,
        build_agent_prompt, build_observation_message, decide_agent_command_execution,
        parse_agent_model_output, parse_agent_tool_call,
    };
    use crate::ai::AgentCommandExecutionMode;
    use crate::ai::tests::sample_ai_request;
    use crate::{
        AiMode, AiProviderKind, AiToolCall, ResolvedAiModel,
        build_openai_compatible_chat_request_body,
    };

    #[test]
    fn agent_prompts_require_non_interactive_commands_in_all_languages() {
        let settings = AiSettings::default();
        for language in ["en", "zh-CN", "zh-TW", "ja", "ko", "fr"] {
            let request = sample_ai_request(language);
            let prompt = build_agent_prompt(&request, &settings);
            assert!(prompt.contains("git --no-pager"));
            assert!(prompt.contains("journalctl --no-pager"));
        }
    }

    #[test]
    fn traditional_chinese_and_korean_prompts_select_localized_text() {
        let settings = AiSettings::default();
        let traditional = sample_ai_request("zh-TW");
        let korean = sample_ai_request("ko-KR");
        assert!(crate::ai::system_prompt("zh-Hant-TW").contains("唯讀"));
        assert!(crate::ai::agent_system_prompt("ko_KR").contains("도구"));
        assert!(build_agent_prompt(&traditional, &settings).contains("使用者任務"));
        assert!(build_agent_prompt(&korean, &settings).contains("사용자 작업"));
    }

    #[test]
    fn parses_agent_response_and_assesses_execution_policy() {
        let parsed = parse_agent_model_output(
            r#"{"thought":"Need to inspect files","action":"execute_command","command":"ls -la","riskLevel":"LOW","riskReason":"read only"}"#,
        )
        .expect("agent response");
        assert_eq!(agent_response_action(&parsed), "execute_command");
        assert_eq!(parsed.risk_level, Some(RiskLevel::Low));

        let assessment = assess_agent_command_risk(&parsed, "ls -la");
        assert_eq!(assessment.effective_risk, RiskLevel::Low);

        let mut settings = AiSettings {
            agent_command_execution_mode: AgentCommandExecutionMode::Smart,
            agent_smart_auto_execute_max_risk: RiskLevel::Low,
            ..AiSettings::default()
        };
        assert_eq!(
            decide_agent_command_execution(&settings, &assessment).0,
            AgentApprovalDecision::Auto
        );

        settings.agent_command_execution_mode = AgentCommandExecutionMode::ConfirmEach;
        assert_eq!(
            decide_agent_command_execution(&settings, &assessment).0,
            AgentApprovalDecision::NeedsApproval
        );
    }

    #[test]
    fn auto_execution_ignores_model_and_local_risk_without_approval() {
        let settings = AiSettings {
            agent_command_execution_mode: AgentCommandExecutionMode::Auto,
            agent_smart_auto_execute_max_risk: RiskLevel::Low,
            ..AiSettings::default()
        };
        for command in ["pwd", "unknown-tool", "sudo rm -rf /tmp/test", "rm -rf /"] {
            let response = super::AgentLlmResponse {
                target_terminal_session_id: None,
                thought: String::new(),
                action: "execute_command".into(),
                command: Some(command.into()),
                risk_level: Some(RiskLevel::Critical),
                risk_reason: None,
                answer: None,
            };
            let assessment = assess_agent_command_risk(&response, command);
            assert_eq!(assessment.effective_risk, RiskLevel::Critical);
            assert_eq!(
                decide_agent_command_execution(&settings, &assessment),
                (AgentApprovalDecision::Auto, None),
                "{command}"
            );
        }
    }

    #[test]
    fn agent_mode_chat_body_uses_agent_protocol_prompts() {
        let settings = AiSettings::default();
        let mut request = sample_ai_request("en");
        request.mode = AiMode::Agent;
        request.user_input = "inspect disk usage".to_string();
        let resolved = ResolvedAiModel {
            backend: Default::default(),
            api_format: Default::default(),
            model_name: "gpt-test".to_string(),
            provider_kind: AiProviderKind::Openai,
            credential: None,
        };

        let body = build_openai_compatible_chat_request_body(&resolved, &request, &settings, &[]);
        let messages = body["messages"].as_array().expect("messages array");

        assert!(
            messages[0]["content"]
                .as_str()
                .expect("system prompt")
                .contains("terminal automation agent")
        );

        assert!(
            messages[1]["content"]
                .as_str()
                .expect("user prompt")
                .contains("Wait for results before requesting dependent actions")
        );
    }

    #[test]
    fn native_execute_command_tools_expose_and_parse_target_session_id() {
        for tools in [
            agent_openai_tools(crate::ai::harness::AgentToolRegistry::TOOLS),
            agent_anthropic_tools(crate::ai::harness::AgentToolRegistry::TOOLS),
            agent_gemini_tools(crate::ai::harness::AgentToolRegistry::TOOLS),
        ] {
            assert!(tools.to_string().contains("sessionId"));
        }

        let parsed = parse_agent_tool_call(&[AiToolCall {
            thought_signature: None,
            id: Some("call-1".to_string()),
            name: "execute_command".to_string(),
            arguments: serde_json::json!({
                "thought": "inspect target b",
                "command": "df -h",
                "targetTerminalSessionId": "terminal-b",
                "riskLevel": "low",
                "riskReason": "read only"
            }),
        }])
        .expect("parse target-aware execute_command");

        assert_eq!(
            parsed.target_terminal_session_id.as_deref(),
            Some("terminal-b")
        );
        assert_eq!(parsed.command.as_deref(), Some("df -h"));
    }

    #[test]
    fn observation_message_truncates_long_output() {
        let obs = CommandObservation {
            output: "x".repeat(8_100),
            exit_code: Some(0),
            duration_ms: 42,
        };

        let message = build_observation_message(&obs, "ls", "en");

        assert!(message.contains("exit code 0"));
        assert!(message.contains("...(truncated)"));
        assert!(message.len() < 8_500);
    }

    #[test]
    fn observation_message_truncates_at_utf8_boundary() {
        let obs = CommandObservation {
            output: "你".repeat(3_000),
            exit_code: Some(0),
            duration_ms: 42,
        };

        let message = build_observation_message(&obs, "cat file", "zh-CN");

        assert!(message.contains("……（已截断）\n你"));
        assert!(message.contains("命令 `cat file` 执行完成"));
    }
}
