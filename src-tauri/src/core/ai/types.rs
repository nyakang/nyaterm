use crate::config::{
    AiAgentKind, AiBackendKind, AiMode, AiModelSource, AiPermissionMode, AiProviderKind, RiskLevel,
};
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiSessionScopeType {
    Terminal,
    Workspace,
    Global,
    Unbound,
}

impl Default for AiSessionScopeType {
    fn default() -> Self {
        Self::Unbound
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiSessionScope {
    #[serde(default)]
    pub r#type: AiSessionScopeType,
    #[serde(default)]
    pub target_id: Option<String>,
    #[serde(default)]
    pub connection_ids: Vec<String>,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiTerminalTarget {
    pub terminal_session_id: String,
    #[serde(default)]
    pub connection_id: Option<String>,
    pub label: String,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    pub session_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AiTargetContext {
    pub target: Option<AiTerminalTarget>,
    #[serde(default)]
    pub context: AiContext,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AiAttachment {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiCommandCard {
    pub id: String,
    pub title: String,
    pub command: String,
    pub explanation: String,
    #[serde(default)]
    pub risk_level: Option<RiskLevel>,
    #[serde(default)]
    pub risk_reason: Option<String>,
    pub expected_effect: String,
    #[serde(default)]
    pub rollback: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub references: Vec<String>,
    #[serde(default)]
    pub target_terminal_session_id: Option<String>,
    #[serde(default)]
    pub target: Option<AiTerminalTarget>,
}

// ---------------------------------------------------------------------------
// Agent (ReAct) types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentActionKind {
    ExecuteCommand,
    FinalAnswer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStepAction {
    pub kind: AgentActionKind,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub target: Option<AiTerminalTarget>,
    #[serde(default)]
    pub risk_level: Option<RiskLevel>,
    #[serde(default)]
    pub model_risk_level: Option<RiskLevel>,
    #[serde(default)]
    pub local_risk_level: Option<RiskLevel>,
    #[serde(default)]
    pub risk_reason: Option<String>,
    #[serde(default)]
    pub approval_reason: Option<String>,
    #[serde(default)]
    pub answer: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandObservation {
    pub output: String,
    #[serde(default)]
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentStepStatus {
    Running,
    Completed,
    NeedsApproval,
    Rejected,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStepPayload {
    pub stream_id: String,
    #[serde(default)]
    pub session_id: Option<String>,
    pub step_index: u16,
    pub thought: String,
    pub action: AgentStepAction,
    #[serde(default)]
    pub observation: Option<CommandObservation>,
    pub status: AgentStepStatus,
    #[serde(default)]
    pub error: Option<String>,
}

/// Parsed single-step agent response from the LLM.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub(super) struct AgentLlmResponse {
    #[serde(default)]
    pub thought: String,
    pub action: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub target_terminal_session_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_risk_level")]
    pub risk_level: Option<RiskLevel>,
    #[serde(default)]
    pub risk_reason: Option<String>,
    #[serde(default)]
    pub answer: Option<String>,
}

fn deserialize_optional_risk_level<'de, D>(deserializer: D) -> Result<Option<RiskLevel>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    Ok(value.and_then(|raw| parse_risk_level_label(&raw)))
}

pub(super) fn parse_risk_level_label(value: &str) -> Option<RiskLevel> {
    match value.trim().replace('-', "_").to_ascii_lowercase().as_str() {
        "low" => Some(RiskLevel::Low),
        "medium" | "moderate" => Some(RiskLevel::Medium),
        "high" => Some(RiskLevel::High),
        "critical" | "danger" | "dangerous" => Some(RiskLevel::Critical),
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AiContext {
    #[serde(default)]
    pub connection_name: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub os: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
    #[serde(default)]
    pub recent_output: String,
    #[serde(default)]
    pub selected_text: String,
    #[serde(default)]
    pub input_buffer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiAction {
    GenerateCommand,
    ExplainOutput,
    ExplainSelected,
    AnalyzeError,
    RepairFromSelection,
    CustomTerminalAction,
    CustomFileAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AiRequestOptions {
    #[serde(default = "default_max_output_commands")]
    pub max_output_commands: u8,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_safety_mode")]
    pub safety_mode: String,
    #[serde(default = "default_history_turns")]
    pub history_turns: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiChatRequest {
    #[serde(default)]
    pub stream_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub connection_id: Option<String>,
    /// The terminal session id to execute commands on (Agent mode).
    #[serde(default)]
    pub terminal_session_id: Option<String>,
    #[serde(default)]
    pub owner_scope: AiSessionScope,
    #[serde(default)]
    pub targets: Vec<AiTerminalTarget>,
    #[serde(default)]
    pub target_contexts: Vec<AiTargetContext>,
    #[serde(default = "default_mode")]
    pub mode: AiMode,
    #[serde(default)]
    pub agent_kind: AiAgentKind,
    #[serde(default)]
    pub permission_mode: AiPermissionMode,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub model_name: Option<String>,
    #[serde(default)]
    pub default_target_session_id: Option<String>,
    #[serde(default)]
    pub existing_external_session_id: Option<String>,
    #[serde(default)]
    pub attachments: Vec<AiAttachment>,
    pub action: AiAction,
    pub user_input: String,
    #[serde(default)]
    pub context: AiContext,
    #[serde(default)]
    pub options: AiRequestOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiStreamStart {
    pub stream_id: String,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiStreamEventPayload {
    #[serde(rename = "type")]
    pub event_type: String,
    pub stream_id: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub text_delta: Option<String>,
    #[serde(default)]
    pub reasoning_delta: Option<String>,
    #[serde(default)]
    pub message: Option<AiMessage>,
    #[serde(default)]
    pub command_cards: Vec<AiCommandCard>,
    #[serde(default)]
    pub usage: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSession {
    pub id: String,
    #[serde(default)]
    pub agent_kind: AiAgentKind,
    #[serde(default)]
    pub scope: AiSessionScope,
    #[serde(default)]
    pub connection_id: Option<String>,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub external_session_id: Option<String>,
    #[serde(default)]
    pub backend_metadata: Option<AiSessionBackendMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AiSessionBackendMetadata {
    #[serde(default)]
    pub backend: AiBackendKind,
    #[serde(default)]
    pub external_thread_id: Option<String>,
    #[serde(default)]
    pub codex_terminal_tools_version: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AiMessageRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiMessage {
    pub id: String,
    pub session_id: String,
    pub role: AiMessageRole,
    pub content: String,
    pub created_at: String,
    #[serde(default)]
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub command_cards: Vec<AiCommandCard>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiAuditLog {
    pub id: String,
    #[serde(default)]
    pub connection_id: Option<String>,
    pub action: String,
    #[serde(default)]
    pub user_input: Option<String>,
    #[serde(default)]
    pub generated_command: Option<String>,
    #[serde(default)]
    pub risk_level: Option<RiskLevel>,
    #[serde(default)]
    pub inserted_to_terminal: bool,
    #[serde(default)]
    pub executed: bool,
    #[serde(default)]
    pub blocked: bool,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub client: Option<String>,
    #[serde(default)]
    pub capability: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<AiPermissionMode>,
    #[serde(default)]
    pub approval_decision: Option<String>,
    #[serde(default)]
    pub success: Option<bool>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppendAiAuditRequest {
    #[serde(default)]
    pub connection_id: Option<String>,
    pub action: String,
    #[serde(default)]
    pub user_input: Option<String>,
    #[serde(default)]
    pub generated_command: Option<String>,
    #[serde(default)]
    pub risk_level: Option<RiskLevel>,
    #[serde(default)]
    pub inserted_to_terminal: bool,
    #[serde(default)]
    pub executed: bool,
    #[serde(default)]
    pub blocked: bool,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub client: Option<String>,
    #[serde(default)]
    pub capability: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<AiPermissionMode>,
    #[serde(default)]
    pub approval_decision: Option<String>,
    #[serde(default)]
    pub success: Option<bool>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiModelDiscovery {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub backend: AiBackendKind,
    #[serde(default)]
    pub provider_kind: Option<AiProviderKind>,
    #[serde(default)]
    pub credential_id: Option<String>,
    pub source: AiModelSource,
}

// ---------------------------------------------------------------------------
// AI capture terminal display events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AiCaptureEvent {
    #[serde(rename_all = "camelCase")]
    CommandStart { command: String, step_index: u16 },
    #[serde(rename_all = "camelCase")]
    CommandEnd {
        output: String,
        exit_code: Option<i32>,
        duration_ms: u64,
        truncated: bool,
    },
}

// ---------------------------------------------------------------------------
// Default value functions for serde
// ---------------------------------------------------------------------------

pub(super) fn default_max_output_commands() -> u8 {
    5
}

pub(super) fn default_language() -> String {
    "en".to_string()
}

pub(super) fn default_safety_mode() -> String {
    "strict".to_string()
}

pub(super) fn default_history_turns() -> u16 {
    20
}

pub(super) fn default_mode() -> AiMode {
    AiMode::Ask
}

// ---------------------------------------------------------------------------
// Shared utility helpers
// ---------------------------------------------------------------------------

pub(super) fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

pub(super) fn uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_agent_risk_level_case_insensitively() {
        let raw = r#"{"thought":"x","action":"execute_command","command":"ls","riskLevel":"HIGH","riskReason":"x"}"#;
        let parsed: AgentLlmResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.risk_level, Some(RiskLevel::High));
        assert_eq!(parsed.risk_reason.as_deref(), Some("x"));
    }

    #[test]
    fn invalid_agent_risk_level_does_not_fail_response_parse() {
        let raw = r#"{"thought":"x","action":"execute_command","command":"ls","riskLevel":"spicy","riskReason":"x"}"#;
        let parsed: AgentLlmResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.risk_level, None);
    }
}
