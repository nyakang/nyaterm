pub mod completion;
pub mod proxy;
use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{SecretString, cloud_sync::MASKED_SECRET_VALUE};

mod agent;
mod claude_code;
mod codex;
mod diagnostics;
pub mod harness;
pub mod ollama;
mod prompts;
pub mod provider_settings;
mod providers;
mod responses;
mod risk;
mod settings;
mod targets;

pub use self::agent::*;
pub use self::claude_code::*;
pub use self::codex::*;
pub use self::diagnostics::*;
pub use self::provider_settings::{
    custom_header_pairs, is_valid_custom_header_name, new_custom_header, normalize_custom_headers,
};
use self::providers::promote_reasoning_to_text;
pub use self::providers::*;
pub use self::responses::*;
use self::risk::max_risk;
pub use self::risk::*;
pub use self::settings::*;
use self::settings::{
    default_active_profile_id, default_agent_smart_auto_execute_max_risk, default_claude_runtime,
    default_codex_runtime, default_context_line_limit, default_history_turns, default_language,
    default_max_ai_file_size_bytes, default_max_output_commands, default_mode,
    default_model_source, default_provider_profiles, default_request_user_agent,
    default_safety_mode, default_schema_version, default_terminal_output_lines, default_timeout_ms,
    default_tool_integration_mode, default_true, provider_kind_key,
};
pub use self::targets::*;

pub const AI_REQUEST_USER_AGENT_DEFAULT: &str =
    "codex-tui/0.125.0 (Ubuntu 22.4.0; x86_64) xterm-256color (codex-tui; 0.125.0)";
pub const AI_HISTORY_MAX_SESSIONS: usize = 200;
pub const AI_HISTORY_MAX_MESSAGES: usize = 2_000;
pub const AI_AUDIT_MAX_LOGS: usize = 2_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiProviderKind {
    Openai,
    Anthropic,
    Gemini,
    Deepseek,
    Groq,
    Ollama,
    Xai,
    Cohere,
    Mimo,
    Zai,
    OpenaiCompatible,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiApiFormat {
    #[default]
    ChatCompletions,
    Responses,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiProviderApiProtocol {
    OpenaiCompatible,
    Anthropic,
    Gemini,
    Ollama,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiBackendKind {
    #[default]
    Genai,
    Codex,
    ClaudeCode,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiAgentKind {
    #[default]
    Nyaterm,
    Codex,
    ClaudeCode,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiPermissionMode {
    Observer,
    #[default]
    Confirm,
    Auto,
    FullAccess,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExternalMcpSessionScope {
    #[default]
    CurrentWindow,
    AllSessions,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum McpTerminalDisplayMode {
    #[default]
    Inline,
    Silent,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalMcpSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub permission_mode: AiPermissionMode,
    #[serde(default)]
    pub session_scope: ExternalMcpSessionScope,
    #[serde(default)]
    pub terminal_display_mode: McpTerminalDisplayMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AiMode {
    Ask,
    Agent,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AiReasoningEffort {
    #[default]
    Auto,
    None,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
    Ultra,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AiModelReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
    Ultra,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiSessionScopeType {
    Terminal,
    Workspace,
    Global,
    #[default]
    Unbound,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

pub fn validate_ai_session_scope(
    session: &AiSession,
    agent_kind: &AiAgentKind,
    owner_scope: &AiSessionScope,
) -> Result<(), &'static str> {
    if &session.agent_kind != agent_kind {
        return Err("AI session belongs to another agent type");
    }
    if session.scope.r#type != AiSessionScopeType::Unbound
        && (session.scope.r#type != owner_scope.r#type
            || session.scope.target_id != owner_scope.target_id)
    {
        return Err("AI session belongs to another terminal scope");
    }
    Ok(())
}

pub fn normalize_ai_history_sessions(history: &mut AiHistoryFile) {
    for session in &mut history.sessions {
        if session.agent_kind == AiAgentKind::Nyaterm
            && let Some(metadata) = session.backend_metadata.as_ref()
            && metadata.backend == AiBackendKind::Codex
        {
            session.agent_kind = AiAgentKind::Codex;
            if session.external_session_id.is_none() {
                session.external_session_id = metadata.external_thread_id.clone();
            }
        }
        if session.scope.r#type == AiSessionScopeType::Unbound
            && session.scope.target_id.is_none()
            && let Some(connection_id) = session.connection_id.as_ref()
            && !connection_id.trim().is_empty()
            && !session.scope.connection_ids.contains(connection_id)
        {
            session.scope.connection_ids.push(connection_id.clone());
        }
    }
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

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiTargetContext {
    #[serde(default)]
    pub target: Option<AiTerminalTarget>,
    #[serde(default)]
    pub context: AiContext,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    #[default]
    Medium,
    High,
    Critical,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandExecutionMode {
    #[default]
    ConfirmEach,
    Smart,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiAgentStepStatus {
    Planning,
    Tool,
    NeedsApproval,
    Running,
    Completed,
    Failed,
    Rejected,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentApprovalOutcome {
    Automatic,
    Approved,
    Rejected,
}

impl AgentApprovalOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AiModelSource {
    RustGenai,
    Manual,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AiProviderProfile {
    pub id: String,
    pub name: String,
    pub provider_kind: AiProviderKind,
    pub model: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<SecretString>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AiModelConfigItem {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub backend: AiBackendKind,
    #[serde(default)]
    pub provider_kind: Option<AiProviderKind>,
    #[serde(default)]
    pub credential_id: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_model_source")]
    pub source: AiModelSource,
    #[serde(default)]
    pub last_seen_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_reasoning_efforts: Option<Vec<AiModelReasoningEffort>>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AiProviderCredential {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_data_url: Option<String>,
    #[serde(default)]
    pub api_protocol: Option<AiProviderApiProtocol>,
    pub id: String,
    pub name: String,
    pub provider_kind: AiProviderKind,
    #[serde(default)]
    pub api_format: AiApiFormat,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<SecretString>,
    #[serde(default)]
    pub extra_headers: Vec<AiHttpHeader>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// A user-defined HTTP header sent with every request made for a provider
/// credential. The value is encrypted at rest, like `api_key`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AiHttpHeader {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub value: SecretString,
}

pub const MAX_AI_CUSTOM_HEADERS: usize = 32;
pub const MAX_AI_CUSTOM_HEADER_NAME_LEN: usize = 256;
pub const MAX_AI_CUSTOM_HEADER_VALUE_LEN: usize = 8192;

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CodexThreadMode {
    #[default]
    Persistent,
    Ephemeral,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodexIntegrationSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub executable_path: Option<String>,
    #[serde(default = "default_codex_runtime")]
    pub runtime: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub config_directory: Option<String>,
    #[serde(default)]
    pub permission_mode: AiPermissionMode,
    #[serde(default = "default_tool_integration_mode")]
    pub tool_integration_mode: Option<String>,
    #[serde(default)]
    pub thread_mode: CodexThreadMode,
    #[serde(default)]
    pub remote_terminal_agent_enabled: bool,
}

impl Default for CodexIntegrationSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            executable_path: None,
            runtime: default_codex_runtime(),
            default_model: None,
            config_directory: None,
            permission_mode: AiPermissionMode::Confirm,
            tool_integration_mode: default_tool_integration_mode(),
            thread_mode: CodexThreadMode::Persistent,
            remote_terminal_agent_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaudeCodeIntegrationSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub executable_path: Option<String>,
    #[serde(default = "default_claude_runtime")]
    pub runtime: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub config_directory: Option<String>,
    #[serde(default)]
    pub permission_mode: AiPermissionMode,
    #[serde(default = "default_tool_integration_mode")]
    pub tool_integration_mode: Option<String>,
}

impl Default for ClaudeCodeIntegrationSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            executable_path: None,
            runtime: default_claude_runtime(),
            default_model: None,
            config_directory: None,
            permission_mode: AiPermissionMode::Confirm,
            tool_integration_mode: default_tool_integration_mode(),
        }
    }
}

impl std::fmt::Debug for AiProviderProfile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiProviderProfile")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("provider_kind", &self.provider_kind)
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("enabled", &self.enabled)
            .finish()
    }
}

impl std::fmt::Debug for AiProviderCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiProviderCredential")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("provider_kind", &self.provider_kind)
            .field("api_format", &self.api_format)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("extra_headers", &self.extra_headers.len())
            .field("enabled", &self.enabled)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AiCustomActionConfig {
    pub id: String,
    pub name: String,
    pub prompt: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AiSettings {
    #[serde(default)]
    pub proxy: proxy::AiProxySettings,
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_context_line_limit")]
    pub context_line_limit: u32,
    #[serde(default = "default_true")]
    pub redaction_enabled: bool,
    #[serde(default = "default_true")]
    pub allow_save_command: bool,
    #[serde(default = "default_true")]
    pub record_history: bool,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_request_user_agent")]
    pub request_user_agent: String,
    #[serde(default = "default_active_profile_id")]
    pub active_profile_id: String,
    #[serde(default = "default_provider_profiles")]
    pub provider_profiles: Vec<AiProviderProfile>,
    #[serde(default = "default_mode")]
    pub default_mode: AiMode,
    #[serde(default)]
    pub default_agent_kind: AiAgentKind,
    #[serde(default)]
    pub external_agent_permission_mode: AiPermissionMode,
    #[serde(default)]
    pub default_reasoning_effort: AiReasoningEffort,
    #[serde(default)]
    pub default_model_id: Option<String>,
    #[serde(default)]
    pub models: Vec<AiModelConfigItem>,
    #[serde(default)]
    pub provider_credentials: Vec<AiProviderCredential>,
    #[serde(default)]
    pub terminal_ai_actions: Vec<AiCustomActionConfig>,
    #[serde(default)]
    pub file_ai_actions: Vec<AiCustomActionConfig>,
    #[serde(default = "default_max_ai_file_size_bytes")]
    pub max_ai_file_size_bytes: u64,
    #[serde(default)]
    pub max_agent_steps: Option<u16>,
    #[serde(default)]
    pub agent_step_timeout_ms: Option<u64>,
    #[serde(default = "default_terminal_output_lines")]
    pub terminal_output_lines: u16,
    #[serde(default)]
    pub agent_background_execution_enabled: bool,
    #[serde(default)]
    pub agent_command_execution_mode: AgentCommandExecutionMode,
    #[serde(default = "default_agent_smart_auto_execute_max_risk")]
    pub agent_smart_auto_execute_max_risk: RiskLevel,
    #[serde(default)]
    pub codex: CodexIntegrationSettings,
    #[serde(default)]
    pub claude_code: ClaudeCodeIntegrationSettings,
    #[serde(default)]
    pub external_mcp: ExternalMcpSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AiMessageRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiSessionBackendMetadata {
    #[serde(default)]
    pub backend: AiBackendKind,
    #[serde(default)]
    pub external_thread_id: Option<String>,
    #[serde(default)]
    pub codex_terminal_tools_version: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct AiHistoryFile {
    #[serde(default)]
    pub sessions: Vec<AiSession>,
    #[serde(default)]
    pub messages: Vec<AiMessage>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct AiAuditFile {
    #[serde(default)]
    pub logs: Vec<AiAuditLog>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiRequestOptions {
    #[serde(skip)]
    pub generation: completion::AiGenerationOptions,
    /// Ephemeral tool transcript; never part of settings or saved chat history.
    #[serde(skip)]
    pub agent_context: Option<harness::AgentRequestContext>,
    #[serde(skip)]
    pub connectivity_test: bool,
    #[serde(default = "default_max_output_commands")]
    pub max_output_commands: u8,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_safety_mode")]
    pub safety_mode: String,
    #[serde(default = "default_history_turns")]
    pub history_turns: u16,
    #[serde(skip)]
    pub agent_json_protocol: bool,
    /// Request-local correction after a response with no usable Agent action.
    #[serde(skip)]
    pub agent_protocol_repair: bool,
}

impl Default for AiRequestOptions {
    fn default() -> Self {
        Self {
            generation: Default::default(),
            agent_context: None,
            connectivity_test: false,
            max_output_commands: default_max_output_commands(),
            language: default_language(),
            safety_mode: default_safety_mode(),
            history_turns: default_history_turns(),
            agent_json_protocol: false,
            agent_protocol_repair: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiChatRequest {
    #[serde(default)]
    pub stream_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub connection_id: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommandObservation {
    pub output: String,
    #[serde(default)]
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiModelOutput {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub reasoning: Option<String>,
    #[serde(default)]
    pub command_cards: Vec<AiCommandCard>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAiModel {
    pub model_name: String,
    pub backend: AiBackendKind,
    pub provider_kind: AiProviderKind,
    pub api_format: AiApiFormat,
    pub credential: Option<AiProviderCredential>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct AiChatCompletion {
    pub metadata: completion::AiResponseMetadata,
    pub text: String,
    pub reasoning_content: Option<String>,
    pub tool_calls: Vec<AiToolCall>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AiToolCall {
    #[serde(skip)]
    pub thought_signature: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct AiChatStreamDelta {
    pub metadata: Option<completion::AiResponseMetadata>,
    pub text_delta: String,
    pub reasoning_delta: Option<String>,
    pub tool_call_deltas: Vec<AiToolCallDelta>,
    pub done: bool,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct AiToolCallDelta {
    pub thought_signature: Option<String>,
    pub index: usize,
    pub id_delta: Option<String>,
    pub name_delta: Option<String>,
    pub arguments_delta: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentLlmResponse {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommandRiskAssessment {
    pub auto_executable: bool,
    pub model_risk: RiskLevel,
    pub local_risk: RiskLevel,
    pub effective_risk: RiskLevel,
    pub risk_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentApprovalDecision {
    Auto,
    NeedsApproval,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiModelDiscovery {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub provider_kind: Option<AiProviderKind>,
    #[serde(default)]
    pub credential_id: Option<String>,
    pub source: AiModelSource,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AiModelError {
    #[error("no enabled AI model configured")]
    NoEnabledModel,
    #[error("AI model '{model}' is missing provider information")]
    MissingProvider { model: String },
    #[error("no enabled AI credential found for model '{model}'")]
    MissingCredentialForModel { model: String },
    #[error("no enabled OpenAI-compatible AI credential configured")]
    MissingOpenAiCompatibleCredential,
    #[error("no enabled AI credential configured for {provider:?}")]
    MissingCredential { provider: AiProviderKind },
    #[error("no API key configured for AI credential '{credential}'")]
    MissingApiKey { credential: String },
    #[error("invalid AI base URL '{base_url}': {message}")]
    InvalidBaseUrl { base_url: String, message: String },
    #[error("invalid AI models JSON: {0}")]
    InvalidModelsJson(String),
    #[error("invalid AI chat JSON: {0}")]
    InvalidChatJson(String),
    #[error(
        "AI agent response contained no tool calls and its text could not be parsed as valid JSON"
    )]
    InvalidAgentJsonFallback,
    #[error("Responses API error: {0}")]
    ResponsesError(String),
    #[error("AI chat response did not include assistant content")]
    MissingChatContent,
}

pub fn ai_model_id_for_provider(kind: &AiProviderKind, name: &str) -> String {
    format!("{}:{name}", provider_kind_key(kind))
}

pub fn ai_model_id_for_credential(credential_id: &str, name: &str) -> String {
    format!("{credential_id}:{name}")
}

pub fn resolve_request_model(
    settings: &AiSettings,
    request: &AiChatRequest,
) -> Result<ResolvedAiModel, AiModelError> {
    let selected_model = request
        .model_id
        .as_deref()
        .and_then(|id| {
            settings
                .models
                .iter()
                .find(|model| model.enabled && model.id == id)
        })
        .or_else(|| {
            settings.default_model_id.as_deref().and_then(|id| {
                settings
                    .models
                    .iter()
                    .find(|model| model.enabled && model.id == id)
            })
        })
        .or_else(|| settings.models.iter().find(|model| model.enabled))
        .ok_or(AiModelError::NoEnabledModel)?;

    let model_provider_kind = selected_model
        .provider_kind
        .clone()
        .or_else(|| infer_provider_kind_from_model_id(&selected_model.id));

    let credential =
        resolve_model_credential(settings, selected_model, model_provider_kind.as_ref())?;
    let provider_kind = credential
        .as_ref()
        .map(|credential| credential.provider_kind.clone())
        .or(model_provider_kind)
        .ok_or_else(|| AiModelError::MissingProvider {
            model: selected_model.name.clone(),
        })?;
    let api_format = credential
        .as_ref()
        .map(|credential| credential.api_format.clone())
        .unwrap_or_default();
    validate_model_credential(&provider_kind, credential.as_ref())?;

    Ok(ResolvedAiModel {
        model_name: selected_model.name.clone(),
        backend: selected_model.backend.clone(),
        provider_kind,
        api_format,
        credential,
    })
}

pub fn infer_provider_kind_from_model_id(model_id: &str) -> Option<AiProviderKind> {
    let (prefix, _) = model_id.split_once(':')?;
    match prefix {
        "openai" => Some(AiProviderKind::Openai),
        "anthropic" => Some(AiProviderKind::Anthropic),
        "gemini" => Some(AiProviderKind::Gemini),
        "deepseek" => Some(AiProviderKind::Deepseek),
        "groq" => Some(AiProviderKind::Groq),
        "ollama" => Some(AiProviderKind::Ollama),
        "xai" => Some(AiProviderKind::Xai),
        "cohere" => Some(AiProviderKind::Cohere),
        "mimo" => Some(AiProviderKind::Mimo),
        "zai" => Some(AiProviderKind::Zai),
        "openai_compatible" => Some(AiProviderKind::OpenaiCompatible),
        _ => None,
    }
}

pub fn resolve_model_credential(
    settings: &AiSettings,
    model: &AiModelConfigItem,
    provider_kind: Option<&AiProviderKind>,
) -> Result<Option<AiProviderCredential>, AiModelError> {
    let implicit_id = model
        .id
        .split_once(':')
        .map(|(id, _)| id)
        .filter(|id| provider_settings::is_builtin_provider(id));
    if let Some(credential_id) = model.credential_id.as_deref().or(implicit_id) {
        let credential = settings
            .provider_credentials
            .iter()
            .find(|item| item.id == credential_id && item.enabled)
            .cloned()
            .ok_or_else(|| AiModelError::MissingCredentialForModel {
                model: model.name.clone(),
            })?;
        return Ok(Some(credential));
    }

    Ok(provider_kind.and_then(|provider_kind| {
        settings
            .provider_credentials
            .iter()
            .find(|item| item.enabled && &item.provider_kind == provider_kind)
            .cloned()
    }))
}

pub fn validate_model_credential(
    provider_kind: &AiProviderKind,
    credential: Option<&AiProviderCredential>,
) -> Result<(), AiModelError> {
    if let Some(credential) = credential {
        if !provider_settings::provider_requires_api_key(credential) {
            return Ok(());
        }
        if credential
            .api_key
            .as_ref()
            .is_none_or(|key| key.expose_secret().trim().is_empty())
        {
            return Err(AiModelError::MissingApiKey {
                credential: credential.name.clone(),
            });
        }
        return Ok(());
    }
    match provider_kind {
        AiProviderKind::Ollama => Ok(()),
        AiProviderKind::OpenaiCompatible => {
            if credential.is_none() {
                return Err(AiModelError::MissingOpenAiCompatibleCredential);
            }
            Ok(())
        }
        _ => {
            let credential = credential.ok_or_else(|| AiModelError::MissingCredential {
                provider: provider_kind.clone(),
            })?;
            if credential
                .api_key
                .as_ref()
                .map(SecretString::expose_secret)
                .is_none_or(|value| value.trim().is_empty())
            {
                return Err(AiModelError::MissingApiKey {
                    credential: credential.name.clone(),
                });
            }
            Ok(())
        }
    }
}

pub fn genai_model_name(provider_kind: &AiProviderKind, model_name: &str) -> String {
    if matches!(provider_kind, AiProviderKind::Deepseek)
        && let Some(base_model_name) = model_name.strip_suffix("-none")
    {
        return base_model_name.to_string();
    }

    model_name.to_string()
}

pub fn effective_ai_request_user_agent(settings: &AiSettings) -> &str {
    let value = settings.request_user_agent.trim();
    if value.is_empty() {
        AI_REQUEST_USER_AGENT_DEFAULT
    } else {
        value
    }
}

pub fn merge_model_discoveries(models: Vec<AiModelDiscovery>) -> Vec<AiModelDiscovery> {
    let mut deduped = std::collections::BTreeMap::new();
    for model in models {
        deduped.entry(model.id.clone()).or_insert(model);
    }
    deduped.into_values().collect()
}

pub fn trim_ai_history(history: &mut AiHistoryFile) {
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

pub fn trim_ai_audit(file: &mut AiAuditFile) {
    if file.logs.len() > AI_AUDIT_MAX_LOGS {
        let keep_from = file.logs.len().saturating_sub(AI_AUDIT_MAX_LOGS);
        file.logs = file.logs.split_off(keep_from);
    }
}

pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

pub fn uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn redact_context(context: &mut AiContext) {
    context.recent_output = redact_sensitive_text(&context.recent_output);
    context.selected_text = redact_sensitive_text(&context.selected_text);
    context.input_buffer = redact_sensitive_text(&context.input_buffer);
}

pub fn redact_sensitive_text(input: &str) -> String {
    let mut output = input.to_string();
    for (pattern, replacement) in redaction_patterns() {
        output = pattern.replace_all(&output, *replacement).to_string();
    }
    output
}

pub fn parse_model_output(
    raw_text: &str,
    stream_reasoning: Option<String>,
) -> (String, Option<String>, Vec<AiCommandCard>) {
    let candidate = extract_json_object(raw_text).unwrap_or_else(|| raw_text.trim().to_string());
    match serde_json::from_str::<AiModelOutput>(&candidate) {
        Ok(output) => {
            let text = if output.text.trim().is_empty() {
                raw_text.trim().to_string()
            } else {
                output.text
            };
            let reasoning_content = trim_optional_to_option(output.reasoning)
                .or_else(|| trim_optional_to_option(stream_reasoning));
            let (text, extracted_reasoning) = extract_think_block(&text);
            let result = (
                text,
                extracted_reasoning.or(reasoning_content),
                output.command_cards,
            );
            if !result.0.is_empty() {
                return result;
            }
            promote_reasoning_to_text(result)
        }
        Err(_) => {
            let normalized_reasoning = trim_optional_to_option(stream_reasoning);
            let (text, extracted_reasoning) = extract_think_block(raw_text);
            let result = (text, extracted_reasoning.or(normalized_reasoning), vec![]);
            if !result.0.is_empty() {
                return result;
            }
            promote_reasoning_to_text(result)
        }
    }
}

pub fn extract_json_object(raw_text: &str) -> Option<String> {
    let trimmed = raw_text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Some(trimmed.to_string());
    }
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if start >= end {
        return None;
    }
    Some(trimmed[start..=end].to_string())
}

pub fn extract_text_from_assistant(content: &str) -> String {
    let trimmed = content.trim();
    if let Some(json_str) = extract_json_object(trimmed)
        && let Ok(output) = serde_json::from_str::<AiModelOutput>(&json_str)
        && !output.text.trim().is_empty()
    {
        return output.text;
    }
    trimmed.to_string()
}

pub fn truncate_preview(s: &str, max_len: usize) -> String {
    let trimmed = s.trim();
    if trimmed.len() <= max_len {
        trimmed.to_string()
    } else {
        let boundary = trimmed
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|&i| i <= max_len)
            .last()
            .unwrap_or(0);
        format!("{}...", &trimmed[..boundary])
    }
}

pub fn trim_string_to_option(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub fn trim_optional_to_option(value: Option<String>) -> Option<String> {
    value.and_then(trim_string_to_option)
}

fn deserialize_optional_risk_level<'de, D>(deserializer: D) -> Result<Option<RiskLevel>, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    Ok(value.and_then(|raw| parse_risk_level_label(&raw)))
}

pub(super) fn deserialize_required_risk_level<'de, D>(
    deserializer: D,
) -> Result<RiskLevel, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    parse_risk_level_label(&value)
        .ok_or_else(|| serde::de::Error::custom(format!("invalid riskLevel '{value}'")))
}

pub fn system_prompt(language: &str) -> &'static str {
    prompts::system(language, false)
}

pub fn agent_system_prompt(language: &str) -> &'static str {
    prompts::system(language, true)
}

fn request_system_prompt(request: &AiChatRequest) -> &'static str {
    if request.options.connectivity_test {
        return "You are NyaTerm's terminal AI connectivity check. Reply with OK only; do not suggest or run commands.";
    }
    if request.mode == AiMode::Agent && request.options.agent_json_protocol {
        return prompts::json_system(request);
    }
    prompts::system(&request.options.language, request.mode == AiMode::Agent)
}

fn request_user_prompt(request: &AiChatRequest, settings: &AiSettings) -> String {
    if request.options.connectivity_test {
        return "Confirm that this model can respond.".into();
    }
    prompts::user_prompt(request, settings, request.mode == AiMode::Agent)
}

pub fn build_prompt(request: &AiChatRequest, settings: &AiSettings) -> String {
    prompts::user_prompt(request, settings, false)
}

fn chat_history_for_request(
    request: &AiChatRequest,
    settings: &AiSettings,
    history: &[AiMessage],
    assistant_role: &str,
) -> Vec<serde_json::Value> {
    let mut messages = Vec::new();
    if request.options.connectivity_test {
        return vec![
            serde_json::json!({"role": "user", "content": "Confirm that this model can respond."}),
        ];
    }

    if let Some(session_id) = request.session_id.as_deref() {
        let max_turns = request.options.history_turns as usize;
        if max_turns > 0 {
            let session_messages = history
                .iter()
                .filter(|message| message.session_id == session_id)
                .collect::<Vec<_>>();
            let skip = session_messages.len().saturating_sub(max_turns);
            for message in session_messages.into_iter().skip(skip) {
                match message.role {
                    AiMessageRole::User => {
                        messages.push(serde_json::json!({
                            "role": "user",
                            "content": message.content,
                        }));
                    }
                    AiMessageRole::Assistant => {
                        let content = extract_text_from_assistant(&message.content);
                        if !content.is_empty() {
                            messages.push(serde_json::json!({
                                "role": assistant_role,
                                "content": content,
                            }));
                        }
                    }
                    AiMessageRole::System => {}
                }
            }
        }
    }

    messages.push(serde_json::json!({
        "role": "user",
        "content": request_user_prompt(request, settings),
    }));
    messages
}

fn extract_think_block(raw_text: &str) -> (String, Option<String>) {
    static THINK_REGEX: OnceLock<Regex> = OnceLock::new();
    let regex = THINK_REGEX.get_or_init(|| Regex::new(r"(?is)<think>(.*?)</think>").unwrap());

    let mut reasoning_parts = Vec::new();
    for captures in regex.captures_iter(raw_text) {
        if let Some(value) = captures.get(1) {
            let reasoning = value.as_str().trim();
            if !reasoning.is_empty() {
                reasoning_parts.push(reasoning.to_string());
            }
        }
    }

    let visible_text = regex.replace_all(raw_text, "").to_string();
    (
        visible_text.trim().to_string(),
        trim_string_to_option(reasoning_parts.join("\n\n")),
    )
}

fn redaction_patterns() -> &'static [(Regex, &'static str)] {
    static PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            (
                Regex::new(
                    r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
                )
                .unwrap(),
                "[REDACTED_PRIVATE_KEY]",
            ),
            (
                Regex::new(r"(?i)Authorization:\s*Bearer\s+[A-Za-z0-9._\-]+").unwrap(),
                "Authorization: Bearer [REDACTED]",
            ),
            (
                Regex::new(r"(?i)(password|passwd|pwd)\s*[:=]\s*[^\s;&|]+").unwrap(),
                "$1=[REDACTED]",
            ),
            (
                Regex::new(
                    r"(?i)(token|api[_-]?key|secret[_-]?key|access[_-]?key)\s*[:=]\s*[^\s;&|]+",
                )
                .unwrap(),
                "$1=[REDACTED]",
            ),
            (
                Regex::new(r"AKIA[0-9A-Z]{16}").unwrap(),
                "[REDACTED_AWS_ACCESS_KEY]",
            ),
            (
                Regex::new(r"(?i)(postgres|mysql|mongodb)://[^@\s]+@").unwrap(),
                "$1://[REDACTED]@",
            ),
        ]
    })
}

#[cfg(test)]
mod tests;

impl std::fmt::Debug for AiChatCompletion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AiChatCompletion { [REDACTED] }")
    }
}

impl std::fmt::Debug for AiToolCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AiToolCall { [REDACTED] }")
    }
}

impl std::fmt::Debug for AiChatStreamDelta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AiChatStreamDelta { [REDACTED] }")
    }
}

impl std::fmt::Debug for AiToolCallDelta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AiToolCallDelta { [REDACTED] }")
    }
}
