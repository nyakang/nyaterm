use nyaterm_mcp_protocol::{definition_for_tool, tool_input_schema, validate_tool_arguments};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ai::{
    AiChatRequest, AiMode, AiModelError, AiProviderApiProtocol, AiToolCall, RiskLevel,
};

use super::run::{AgentPlan, AgentQuestion, AgentVerification};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentTool {
    GetEnvironment,
    SessionGet,
    TerminalExecute,
    TerminalRecentOutput,
    SftpList,
    SftpStat,
    SftpReadText,
    ToolOutputRead,
    RequestUserInput,
    UpdatePlan,
    FinalAnswer,
}

impl AgentTool {
    pub fn name(self) -> &'static str {
        match self {
            Self::GetEnvironment => "get_environment",
            Self::SessionGet => "session_get",
            Self::TerminalExecute => "terminal_execute",
            Self::TerminalRecentOutput => "terminal_recent_output",
            Self::SftpList => "sftp_list",
            Self::SftpStat => "sftp_stat",
            Self::SftpReadText => "sftp_read_text",
            Self::ToolOutputRead => "tool_output_read",
            Self::RequestUserInput => "request_user_input",
            Self::UpdatePlan => "update_plan",
            Self::FinalAnswer => "final_answer",
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentToolCall {
    #[serde(skip)]
    pub assistant_turn: Option<crate::ai::completion::AiAssistantTurn>,
    pub id: String,
    pub tool: AgentTool,
    pub arguments: Value,
    pub thought: String,
    pub model_risk: Option<RiskLevel>,
    #[serde(skip)]
    pub thought_signature: Option<String>,
    /// Original provider arguments, including metadata covered by thought signatures.
    #[serde(skip)]
    pub provider_arguments: Value,
}

impl std::fmt::Debug for AgentToolCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentToolCall")
            .field("id", &self.id)
            .field("tool", &self.tool)
            .field("arguments", &"[REDACTED]")
            .field("model_risk", &self.model_risk)
            .finish()
    }
}

pub struct AgentToolRegistry;

/// Runtime request capabilities, shared by SDK and native wire builders.
/// Tool choice is always automatic; execution is serialized by AgentRun.
#[derive(Debug, Clone, Copy)]
pub struct AgentRequestPolicy {
    pub tools: Option<&'static [AgentTool]>,
    pub strict: Option<bool>,
}

impl AgentToolRegistry {
    pub fn request_policy(
        request: &AiChatRequest,
        protocol: AiProviderApiProtocol,
    ) -> AgentRequestPolicy {
        AgentRequestPolicy {
            tools: (request.mode == AiMode::Agent
                && !request.options.agent_json_protocol
                && !request.options.connectivity_test)
                .then(|| Self::available_tools(request)),
            strict: (protocol == AiProviderApiProtocol::OpenaiCompatible).then_some(false),
        }
    }

    pub fn available_tools(request: &crate::ai::AiChatRequest) -> &'static [AgentTool] {
        if request
            .options
            .agent_context
            .as_ref()
            .is_some_and(|context| context.remaining_steps == 0)
        {
            &[AgentTool::FinalAnswer]
        } else {
            Self::TOOLS
        }
    }
    pub const TOOLS: &'static [AgentTool] = &[
        AgentTool::GetEnvironment,
        AgentTool::SessionGet,
        AgentTool::TerminalExecute,
        AgentTool::TerminalRecentOutput,
        AgentTool::SftpList,
        AgentTool::SftpStat,
        AgentTool::SftpReadText,
        AgentTool::ToolOutputRead,
        AgentTool::RequestUserInput,
        AgentTool::UpdatePlan,
        AgentTool::FinalAnswer,
    ];

    pub fn schema(tool: AgentTool) -> Value {
        let mut schema = if definition_for_tool(tool.name()).is_some() {
            tool_input_schema(tool.name()).expect("registered capability schema")
        } else {
            match tool {
                AgentTool::RequestUserInput => json!({"type":"object", "properties": {
                    "questions": {"type":"array", "minItems":1, "maxItems":3, "items": {
                        "type":"object", "properties": {"id":{"type":"string"},
                        "question":{"type":"string"}, "options":{"type":"array", "items":{"type":"string"}}},
                        "required":["id","question"], "additionalProperties":false}}
                }, "required":["questions"], "additionalProperties":false}),
                AgentTool::UpdatePlan => json!({"type":"object", "properties": {
                    "tasks":{"type":"array", "items":{"type":"object", "properties":{
                        "id":{"type":"string"}, "description":{"type":"string"},
                        "status":{"type":"string", "enum":["pending","in_progress","completed","blocked"]},
                        "verification":{"type":"string"}},
                        "required":["id","description","status"], "additionalProperties":false}}
                }, "required":["tasks"], "additionalProperties":false}),
                AgentTool::FinalAnswer => json!({"type":"object", "properties":{
                    "answer":{"type":"string"}, "verification":{"type":"object", "properties":{
                        "status":{"type":"string", "enum":["verified","unverified","blocked"]},
                        "summary":{"type":"string"}}, "required":["status","summary"], "additionalProperties":false}
                }, "required":["answer","verification"], "additionalProperties":false}),
                _ => unreachable!("capability has a schema"),
            }
        };
        if schema.get("properties").is_none() {
            schema["properties"] = json!({});
        }
        let properties = schema["properties"]
            .as_object_mut()
            .expect("object tool schema");
        properties.insert(
            "thought".into(),
            json!({"type":"string", "description":"Brief reason for this action."}),
        );
        if tool == AgentTool::TerminalExecute {
            properties.insert(
                "riskLevel".into(),
                json!({"type":"string", "enum":["low","medium","high","critical"]}),
            );
            properties.insert("riskReason".into(), json!({"type":"string"}));
        }
        schema
    }

    pub fn description(tool: AgentTool) -> &'static str {
        if let Some(definition) = definition_for_tool(tool.name()) {
            return definition.description;
        }
        match tool {
            AgentTool::RequestUserInput => {
                "Ask the user for missing information and wait for their answers in this run."
            }
            AgentTool::UpdatePlan => {
                "Update the task plan using stable task ids; at most one task may be in progress."
            }
            AgentTool::FinalAnswer => {
                "Finish with an answer and explicit verified, unverified, or blocked verification."
            }
            _ => unreachable!("registered capability"),
        }
    }

    pub fn parse(calls: &[AiToolCall], text: &str) -> Result<AgentToolCall, AiModelError> {
        let invalid = |message: String| AiModelError::InvalidChatJson(message);
        let (id, mut name, mut arguments) = if calls.is_empty() {
            let candidate =
                crate::ai::extract_json_object(text).unwrap_or_else(|| text.trim().into());
            let mut value: Value = serde_json::from_str(&candidate)
                .map_err(|_| AiModelError::InvalidAgentJsonFallback)?;
            let name = value
                .get("action")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("missing agent action".into()))?
                .to_string();
            let arguments = if value.get("arguments").is_some() {
                let mut args = value["arguments"].take();
                if !args.is_object() {
                    return Err(invalid("tool arguments must be an object".into()));
                }
                for key in ["thought", "riskLevel", "riskReason"] {
                    if let Some(metadata) = value.get(key) {
                        args[key] = metadata.clone();
                    }
                }
                args
            } else {
                value
                    .as_object_mut()
                    .ok_or_else(|| invalid("expected agent object".into()))?
                    .remove("action");
                value
            };
            (format!("call-{}", uuid::Uuid::new_v4()), name, arguments)
        } else {
            if calls.len() != 1 {
                return Err(invalid("expected exactly one AI agent tool call".into()));
            }
            let call = &calls[0];
            (
                call.id
                    .clone()
                    .filter(|id| !id.is_empty())
                    .unwrap_or_else(|| format!("call-{}", uuid::Uuid::new_v4())),
                call.name.clone(),
                call.arguments.clone(),
            )
        };
        let provider_arguments = arguments.clone();
        let object = arguments
            .as_object_mut()
            .ok_or_else(|| invalid("tool arguments must be an object".into()))?;
        if name == "execute_command" {
            name = "terminal_execute".into();
            if let Some(target) = object.remove("targetTerminalSessionId") {
                object.insert("sessionId".into(), target);
            }
        }
        let thought = match object.remove("thought") {
            Some(Value::String(value)) => value,
            None | Some(Value::Null) => String::new(),
            _ => return Err(invalid("thought must be text".into())),
        };
        let risk = object.remove("riskLevel");
        object.remove("riskReason");
        let model_risk = match risk {
            None | Some(Value::Null) => None,
            Some(Value::String(value)) => Some(
                crate::ai::parse_risk_level_label(&value)
                    .ok_or_else(|| invalid("invalid risk level".into()))?,
            ),
            _ => return Err(invalid("invalid risk level".into())),
        };
        let tool = Self::TOOLS
            .iter()
            .copied()
            .find(|tool| tool.name() == name)
            .ok_or_else(|| invalid("unknown agent tool".into()))?;
        // Older final_answer replies cannot claim verified completion.
        if tool == AgentTool::FinalAnswer && arguments.get("verification").is_none() {
            arguments["verification"] =
                json!({"status":"unverified","summary":"No explicit verification was supplied."});
        }
        let call = AgentToolCall {
            assistant_turn: None,
            id,
            tool,
            arguments,
            thought,
            model_risk,
            provider_arguments,
            thought_signature: calls
                .first()
                .and_then(|call| call.thought_signature.clone()),
        };
        Self::validate(&call).map_err(invalid)?;
        Ok(call)
    }

    pub fn validate(call: &AgentToolCall) -> Result<(), String> {
        if definition_for_tool(call.tool.name()).is_some() {
            return validate_tool_arguments(call.tool.name(), &call.arguments)
                .map_err(|_| "invalid capability arguments".into());
        }
        match call.tool {
            AgentTool::UpdatePlan => serde_json::from_value::<AgentPlan>(call.arguments.clone())
                .map_err(|_| "invalid plan".to_string())?
                .validate(),
            AgentTool::RequestUserInput => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Questions {
                    questions: Vec<AgentQuestion>,
                }
                let args = serde_json::from_value::<Questions>(call.arguments.clone())
                    .map_err(|_| "invalid questions".to_string())?;
                AgentQuestion::validate_all(&args.questions)
            }
            AgentTool::FinalAnswer => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Final {
                    answer: String,
                    verification: AgentVerification,
                }
                let args = serde_json::from_value::<Final>(call.arguments.clone())
                    .map_err(|_| "invalid final answer".to_string())?;
                if args.answer.trim().is_empty() || args.verification.summary.trim().is_empty() {
                    Err("answer and verification summary are required".into())
                } else {
                    Ok(())
                }
            }
            _ => unreachable!("registered capability"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentTool, AgentToolRegistry};
    use crate::ai::{AiModelError, AiToolCall, RiskLevel};
    use serde_json::json;

    #[test]
    fn legacy_command_routes_to_validated_capability_with_risk_outside_arguments() {
        let call = AgentToolRegistry::parse(&[],r#"{"action":"execute_command","thought":"inspect","command":"pwd","targetTerminalSessionId":"s","riskLevel":"high","riskReason":"model assessment"}"#).unwrap();
        assert_eq!(call.tool, AgentTool::TerminalExecute);
        assert_eq!(call.arguments, json!({"sessionId":"s","command":"pwd"}));
        assert_eq!(call.model_risk, Some(RiskLevel::High));
    }

    #[test]
    fn invalid_or_multiple_tools_never_fall_back_to_reply_text() {
        let bad = AiToolCall {
            thought_signature: None,
            id: Some("call".into()),
            name: "sftp_write_text".into(),
            arguments: json!({}),
        };
        let final_text = r#"{"action":"execute_command","command":"pwd"}"#;
        assert!(AgentToolRegistry::parse(std::slice::from_ref(&bad), final_text).is_err());
        assert!(AgentToolRegistry::parse(&[bad.clone(), bad], final_text).is_err());
        assert!(AgentToolRegistry::parse(&[],r#"{"action":"sftp_read_text","arguments":{"sessionId":"s","path":"/tmp/test","unexpected":true}}"#).is_err());
    }

    #[test]
    fn old_final_answer_defaults_to_unverified() {
        let call =
            AgentToolRegistry::parse(&[], r#"{"action":"final_answer","answer":"done"}"#).unwrap();
        assert_eq!(call.arguments["verification"]["status"], "unverified");
    }

    #[test]
    fn missing_tools_with_unparseable_text_have_a_distinct_protocol_error() {
        for text in [
            "",
            "ordinary answer",
            "{\"action\":",
            "```json\n{broken}\n```",
        ] {
            let error = AgentToolRegistry::parse(&[], text).unwrap_err();
            assert_eq!(error, AiModelError::InvalidAgentJsonFallback);
            assert!(!error.to_string().starts_with("invalid AI chat JSON"));
        }
        assert_ne!(
            AgentToolRegistry::parse(&[], r#"{"action":"unknown"}"#).unwrap_err(),
            AiModelError::InvalidAgentJsonFallback
        );
        assert_ne!(
            AgentToolRegistry::parse(&[], r#"{"answer":"missing action"}"#).unwrap_err(),
            AiModelError::InvalidAgentJsonFallback
        );
    }

    #[test]
    fn debug_does_not_include_command_context_or_signatures() {
        let call = AgentToolRegistry::parse(&[],r#"{"action":"execute_command","command":"echo private-context","thought":"private-reason"}"#).unwrap();
        let debug = format!("{call:?}");
        assert!(!debug.contains("private-context"));
        assert!(!debug.contains("private-reason"));
    }

    #[test]
    fn provider_metadata_is_preserved_in_memory_but_never_enters_operation_arguments() {
        let arguments = json!({"sessionId":"s","command":"pwd","thought":"inspect","riskLevel":"high","riskReason":"fixture"});
        let parsed = AgentToolRegistry::parse(
            &[AiToolCall {
                thought_signature: Some("signature".into()),
                id: Some("call".into()),
                name: "terminal_execute".into(),
                arguments: arguments.clone(),
            }],
            "",
        )
        .unwrap();
        assert_eq!(parsed.provider_arguments, arguments);
        assert_eq!(parsed.arguments, json!({"sessionId":"s","command":"pwd"}));
        assert_eq!(parsed.thought_signature.as_deref(), Some("signature"));
        assert!(
            !serde_json::to_string(&parsed)
                .unwrap()
                .contains("provider_arguments")
        );
    }
}
