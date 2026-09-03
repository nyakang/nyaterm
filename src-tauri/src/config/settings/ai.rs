use crate::config::MASKED_SECRET_VALUE;
use crate::error::AppResult;
use crate::utils::crypto;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const AI_REQUEST_USER_AGENT_DEFAULT: &str =
    "codex-tui/0.125.0 (Ubuntu 22.4.0; x86_64) xterm-256color (codex-tui; 0.125.0)";
const OLLAMA_PROVIDER_ID: &str = "ollama";
const OLLAMA_DEFAULT_BASE_URL: &str = "http://localhost:11434/";
const OLLAMA_LEGACY_DEFAULT_BASE_URL: &str = "http://localhost:11434/v1/";

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiApiFormat {
    ChatCompletions,
    Responses,
}

impl Default for AiApiFormat {
    fn default() -> Self {
        Self::ChatCompletions
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiBackendKind {
    Genai,
    Codex,
}

impl Default for AiBackendKind {
    fn default() -> Self {
        Self::Genai
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiAgentKind {
    Nyaterm,
    Codex,
    ClaudeCode,
}

impl Default for AiAgentKind {
    fn default() -> Self {
        Self::Nyaterm
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiPermissionMode {
    Observer,
    Confirm,
    Auto,
    FullAccess,
}

impl Default for AiPermissionMode {
    fn default() -> Self {
        Self::Confirm
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExternalMcpSessionScope {
    CurrentWindow,
    AllSessions,
}

impl Default for ExternalMcpSessionScope {
    fn default() -> Self {
        Self::CurrentWindow
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalMcpSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub permission_mode: AiPermissionMode,
    #[serde(default)]
    pub session_scope: ExternalMcpSessionScope,
}

impl Default for ExternalMcpSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            permission_mode: AiPermissionMode::Confirm,
            session_scope: ExternalMcpSessionScope::CurrentWindow,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AiMode {
    Ask,
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AiReasoningEffort {
    Auto,
    None,
    Low,
    Medium,
    High,
    XHigh,
}

impl Default for AiReasoningEffort {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl Default for RiskLevel {
    fn default() -> Self {
        Self::Medium
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandExecutionMode {
    ConfirmEach,
    Smart,
    Auto,
}

impl Default for AgentCommandExecutionMode {
    fn default() -> Self {
        Self::ConfirmEach
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AiModelSource {
    RustGenai,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiProviderProfile {
    pub id: String,
    pub name: String,
    pub provider_kind: AiProviderKind,
    pub model: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CodexThreadMode {
    Persistent,
    Ephemeral,
}

impl Default for CodexThreadMode {
    fn default() -> Self {
        Self::Persistent
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexIntegrationSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub executable_path: Option<String>,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub config_directory: Option<String>,
    #[serde(default)]
    pub permission_mode: AiPermissionMode,
    #[serde(default)]
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
            runtime: Some("app_server".to_string()),
            default_model: None,
            config_directory: None,
            permission_mode: AiPermissionMode::Confirm,
            tool_integration_mode: Some("nyaterm_mcp".to_string()),
            thread_mode: CodexThreadMode::Persistent,
            remote_terminal_agent_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeCodeIntegrationSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub executable_path: Option<String>,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub config_directory: Option<String>,
    #[serde(default)]
    pub permission_mode: AiPermissionMode,
    #[serde(default)]
    pub tool_integration_mode: Option<String>,
}

impl Default for ClaudeCodeIntegrationSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            executable_path: None,
            runtime: Some("stream_json_cli".to_string()),
            default_model: None,
            config_directory: None,
            permission_mode: AiPermissionMode::Confirm,
            tool_integration_mode: Some("nyaterm_mcp".to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiProviderCredential {
    pub id: String,
    pub name: String,
    pub provider_kind: AiProviderKind,
    #[serde(default)]
    pub api_format: AiApiFormat,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiCustomActionConfig {
    pub id: String,
    pub name: String,
    pub prompt: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiSettings {
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

fn default_schema_version() -> u32 {
    6
}

fn default_true() -> bool {
    true
}

fn default_context_line_limit() -> u32 {
    200
}

fn default_timeout_ms() -> u64 {
    60_000
}

fn default_request_user_agent() -> String {
    AI_REQUEST_USER_AGENT_DEFAULT.to_string()
}

fn default_mode() -> AiMode {
    AiMode::Ask
}

fn default_model_source() -> AiModelSource {
    AiModelSource::RustGenai
}

fn default_terminal_output_lines() -> u16 {
    10
}

fn default_agent_smart_auto_execute_max_risk() -> RiskLevel {
    RiskLevel::Low
}

fn default_max_ai_file_size_bytes() -> u64 {
    1_048_576
}

fn default_active_profile_id() -> String {
    "openai".to_string()
}

fn default_provider_profiles() -> Vec<AiProviderProfile> {
    vec![
        AiProviderProfile {
            id: "openai".to_string(),
            name: "OpenAI".to_string(),
            provider_kind: AiProviderKind::Openai,
            model: "gpt-4o-mini".to_string(),
            base_url: None,
            api_key: None,
            enabled: false,
        },
        AiProviderProfile {
            id: "anthropic".to_string(),
            name: "Anthropic".to_string(),
            provider_kind: AiProviderKind::Anthropic,
            model: "claude-3-haiku-20240307".to_string(),
            base_url: None,
            api_key: None,
            enabled: false,
        },
        AiProviderProfile {
            id: "gemini".to_string(),
            name: "Google Gemini".to_string(),
            provider_kind: AiProviderKind::Gemini,
            model: "gemini-2.0-flash".to_string(),
            base_url: None,
            api_key: None,
            enabled: false,
        },
        AiProviderProfile {
            id: "deepseek".to_string(),
            name: "DeepSeek".to_string(),
            provider_kind: AiProviderKind::Deepseek,
            model: "deepseek-chat".to_string(),
            base_url: None,
            api_key: None,
            enabled: false,
        },
        AiProviderProfile {
            id: OLLAMA_PROVIDER_ID.to_string(),
            name: "Ollama".to_string(),
            provider_kind: AiProviderKind::Ollama,
            model: "llama3-7b".to_string(),
            base_url: Some(OLLAMA_DEFAULT_BASE_URL.to_string()),
            api_key: None,
            enabled: false,
        },
        AiProviderProfile {
            id: "xai".to_string(),
            name: "xAI".to_string(),
            provider_kind: AiProviderKind::Xai,
            model: "grok-3".to_string(),
            base_url: Some("https://api.x.ai/v1/".to_string()),
            api_key: None,
            enabled: false,
        },
        AiProviderProfile {
            id: "cohere".to_string(),
            name: "Cohere".to_string(),
            provider_kind: AiProviderKind::Cohere,
            model: "command-a-03-2025".to_string(),
            base_url: Some("https://api.cohere.com/compatibility/v1/".to_string()),
            api_key: None,
            enabled: false,
        },
        AiProviderProfile {
            id: "mimo".to_string(),
            name: "Mimo".to_string(),
            provider_kind: AiProviderKind::Mimo,
            model: "mimo-v2.5-pro".to_string(),
            base_url: Some("https://api.xiaomimimo.com/v1/".to_string()),
            api_key: None,
            enabled: false,
        },
        AiProviderProfile {
            id: "zai".to_string(),
            name: "ZAI".to_string(),
            provider_kind: AiProviderKind::Zai,
            model: "glm-4".to_string(),
            base_url: Some("https://open.bigmodel.cn/api/paas/v4/".to_string()),
            api_key: None,
            enabled: false,
        },
    ]
}

fn provider_kind_key(kind: &AiProviderKind) -> &'static str {
    match kind {
        AiProviderKind::Openai => "openai",
        AiProviderKind::Anthropic => "anthropic",
        AiProviderKind::Gemini => "gemini",
        AiProviderKind::Deepseek => "deepseek",
        AiProviderKind::Groq => "groq",
        AiProviderKind::Ollama => "ollama",
        AiProviderKind::Xai => "xai",
        AiProviderKind::Cohere => "cohere",
        AiProviderKind::Mimo => "mimo",
        AiProviderKind::Zai => "zai",
        AiProviderKind::OpenaiCompatible => "openai_compatible",
    }
}

pub fn ai_model_id_for_provider(kind: &AiProviderKind, name: &str) -> String {
    format!("{}:{name}", provider_kind_key(kind))
}

pub fn ai_model_id_for_credential(credential_id: &str, name: &str) -> String {
    format!("{credential_id}:{name}")
}

fn credential_from_profile(profile: &AiProviderProfile) -> AiProviderCredential {
    AiProviderCredential {
        id: profile.id.clone(),
        name: profile.name.clone(),
        provider_kind: profile.provider_kind.clone(),
        api_format: AiApiFormat::default(),
        base_url: profile.base_url.clone(),
        api_key: profile.api_key.clone(),
        enabled: profile.enabled,
    }
}

fn model_from_profile(profile: &AiProviderProfile) -> Option<AiModelConfigItem> {
    let name = profile.model.trim();
    if name.is_empty() {
        return None;
    }

    let is_manual = profile.provider_kind == AiProviderKind::OpenaiCompatible
        || profile
            .base_url
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());
    let id = if is_manual {
        ai_model_id_for_credential(&profile.id, name)
    } else {
        ai_model_id_for_provider(&profile.provider_kind, name)
    };

    Some(AiModelConfigItem {
        id,
        name: name.to_string(),
        backend: AiBackendKind::Genai,
        provider_kind: Some(profile.provider_kind.clone()),
        credential_id: is_manual.then(|| profile.id.clone()),
        enabled: profile.enabled,
        source: if is_manual {
            AiModelSource::Manual
        } else {
            AiModelSource::RustGenai
        },
        last_seen_at: None,
    })
}

fn default_provider_credentials() -> Vec<AiProviderCredential> {
    default_provider_profiles()
        .iter()
        .map(credential_from_profile)
        .collect()
}

fn default_models() -> Vec<AiModelConfigItem> {
    Vec::new()
}

fn default_terminal_ai_actions() -> Vec<AiCustomActionConfig> {
    vec![
        AiCustomActionConfig {
            id: "explain-selected".to_string(),
            name: "解释选中内容".to_string(),
            prompt: "请解释终端中选中的内容，指出含义、可能原因和下一步建议。".to_string(),
            enabled: true,
        },
        AiCustomActionConfig {
            id: "generate-fix-command".to_string(),
            name: "生成修复命令".to_string(),
            prompt: "请根据终端选中内容生成可执行的修复命令，并说明风险。".to_string(),
            enabled: true,
        },
    ]
}

fn default_file_ai_actions() -> Vec<AiCustomActionConfig> {
    vec![
        AiCustomActionConfig {
            id: "summarize-file".to_string(),
            name: "总结文件".to_string(),
            prompt: "请总结选中文件的主要内容、关键风险和建议操作。".to_string(),
            enabled: true,
        },
        AiCustomActionConfig {
            id: "explain-file".to_string(),
            name: "解释文件".to_string(),
            prompt: "请解释选中文件的用途、结构和关键字段。".to_string(),
            enabled: true,
        },
    ]
}

impl Default for AiSettings {
    fn default() -> Self {
        let models = default_models();
        let default_model_id = models
            .iter()
            .find(|item| item.enabled)
            .map(|item| item.id.clone());

        Self {
            schema_version: 6,
            enabled: true,
            context_line_limit: default_context_line_limit(),
            redaction_enabled: true,
            allow_save_command: true,
            record_history: true,
            timeout_ms: default_timeout_ms(),
            request_user_agent: default_request_user_agent(),
            active_profile_id: default_active_profile_id(),
            provider_profiles: default_provider_profiles(),
            default_mode: default_mode(),
            default_agent_kind: AiAgentKind::Nyaterm,
            external_agent_permission_mode: AiPermissionMode::Confirm,
            default_reasoning_effort: AiReasoningEffort::Auto,
            default_model_id,
            models,
            provider_credentials: default_provider_credentials(),
            terminal_ai_actions: default_terminal_ai_actions(),
            file_ai_actions: default_file_ai_actions(),
            max_ai_file_size_bytes: default_max_ai_file_size_bytes(),
            max_agent_steps: Some(10),
            agent_step_timeout_ms: Some(30_000),
            terminal_output_lines: default_terminal_output_lines(),
            agent_background_execution_enabled: false,
            agent_command_execution_mode: AgentCommandExecutionMode::ConfirmEach,
            agent_smart_auto_execute_max_risk: default_agent_smart_auto_execute_max_risk(),
            codex: CodexIntegrationSettings::default(),
            claude_code: ClaudeCodeIntegrationSettings::default(),
            external_mcp: ExternalMcpSettings::default(),
        }
    }
}

pub fn decrypt_ai_settings(mut settings: AiSettings) -> AppResult<AiSettings> {
    for profile in &mut settings.provider_profiles {
        profile.api_key = decrypt_secret(profile.api_key.take())?;
    }
    for credential in &mut settings.provider_credentials {
        credential.api_key = decrypt_secret(credential.api_key.take())?;
    }
    Ok(settings)
}

pub fn encrypt_ai_settings(mut settings: AiSettings) -> AppResult<AiSettings> {
    for profile in &mut settings.provider_profiles {
        profile.api_key = encrypt_secret(profile.api_key.take())?;
    }
    for credential in &mut settings.provider_credentials {
        credential.api_key = encrypt_secret(credential.api_key.take())?;
    }
    Ok(settings)
}

pub fn mask_ai_settings(mut settings: AiSettings) -> AiSettings {
    for profile in &mut settings.provider_profiles {
        profile.api_key = mask_secret(profile.api_key.take());
    }
    for credential in &mut settings.provider_credentials {
        credential.api_key = mask_secret(credential.api_key.take());
    }
    settings
}

pub fn merge_masked_ai_settings(current: &AiSettings, mut next: AiSettings) -> AiSettings {
    for profile in &mut next.provider_profiles {
        let current_secret = current
            .provider_profiles
            .iter()
            .find(|item| item.id == profile.id)
            .and_then(|item| item.api_key.as_ref());
        profile.api_key = merge_secret(current_secret, profile.api_key.as_ref());
    }
    for credential in &mut next.provider_credentials {
        let current_secret = current
            .provider_credentials
            .iter()
            .find(|item| item.id == credential.id)
            .and_then(|item| item.api_key.as_ref());
        credential.api_key = merge_secret(current_secret, credential.api_key.as_ref());
    }
    normalize_ai_settings(&mut next);
    next
}

pub fn normalize_ai_settings(settings: &mut AiSettings) -> bool {
    let original = serde_json::to_string(settings).unwrap_or_default();

    settings.schema_version = 6;
    if settings.request_user_agent.trim().is_empty() {
        settings.request_user_agent = default_request_user_agent();
    }

    for profile in &mut settings.provider_profiles {
        if is_builtin_ollama_provider(&profile.id, &profile.provider_kind) {
            migrate_legacy_ollama_base_url(&mut profile.base_url);
        }
    }

    if settings.provider_credentials.is_empty() {
        settings.provider_credentials = settings
            .provider_profiles
            .iter()
            .map(credential_from_profile)
            .collect();
    }

    for credential in &mut settings.provider_credentials {
        if is_builtin_ollama_provider(&credential.id, &credential.provider_kind) {
            migrate_legacy_ollama_base_url(&mut credential.base_url);
        }
    }

    if settings.models.is_empty() {
        let mut seen = HashSet::new();
        settings.models = settings
            .provider_profiles
            .iter()
            .filter_map(model_from_profile)
            .filter(|model| seen.insert(model.id.clone()))
            .collect();
    }

    if settings.terminal_ai_actions.is_empty() {
        settings.terminal_ai_actions = default_terminal_ai_actions();
    }
    if settings.file_ai_actions.is_empty() {
        settings.file_ai_actions = default_file_ai_actions();
    }
    if settings.max_ai_file_size_bytes == 0 {
        settings.max_ai_file_size_bytes = default_max_ai_file_size_bytes();
    }

    for model in &mut settings.models {
        if model.backend == AiBackendKind::Codex {
            model.provider_kind = None;
            model.credential_id = None;
        }
        if model.id.trim().is_empty() {
            model.id = if model.backend == AiBackendKind::Codex {
                format!("codex:{}", model.name)
            } else if let Some(credential_id) = model.credential_id.as_deref() {
                ai_model_id_for_credential(credential_id, &model.name)
            } else if let Some(kind) = &model.provider_kind {
                ai_model_id_for_provider(kind, &model.name)
            } else {
                model.name.clone()
            };
        }
    }

    if settings.default_model_id.as_deref().is_none_or(|id| {
        !settings
            .models
            .iter()
            .any(|model| model.enabled && model.id == id)
    }) {
        let active_model = settings
            .provider_profiles
            .iter()
            .find(|profile| profile.id == settings.active_profile_id)
            .and_then(model_from_profile)
            .and_then(|legacy_model| {
                settings
                    .models
                    .iter()
                    .find(|model| model.id == legacy_model.id)
                    .map(|model| model.id.clone())
            });

        settings.default_model_id = active_model.or_else(|| {
            settings
                .models
                .iter()
                .find(|model| model.enabled)
                .map(|model| model.id.clone())
        });
    }

    serde_json::to_string(settings).unwrap_or_default() != original
}

fn decrypt_secret(value: Option<String>) -> AppResult<Option<String>> {
    match value {
        Some(ciphertext) if !ciphertext.is_empty() => crypto::decrypt(&ciphertext).map(Some),
        _ => Ok(None),
    }
}

fn encrypt_secret(value: Option<String>) -> AppResult<Option<String>> {
    match value {
        Some(plaintext) if !plaintext.is_empty() => crypto::encrypt(&plaintext).map(Some),
        _ => Ok(None),
    }
}

fn mask_secret(value: Option<String>) -> Option<String> {
    value.and_then(|secret| {
        if secret.is_empty() {
            None
        } else {
            Some(MASKED_SECRET_VALUE.to_string())
        }
    })
}

fn merge_secret(current: Option<&String>, incoming: Option<&String>) -> Option<String> {
    match incoming.map(String::as_str) {
        Some(MASKED_SECRET_VALUE) | None => current.cloned(),
        Some("") => None,
        Some(value) => Some(value.to_string()),
    }
}

fn is_builtin_ollama_provider(id: &str, provider_kind: &AiProviderKind) -> bool {
    id == OLLAMA_PROVIDER_ID && provider_kind == &AiProviderKind::Ollama
}

fn migrate_legacy_ollama_base_url(base_url: &mut Option<String>) {
    if base_url.as_deref() == Some(OLLAMA_LEGACY_DEFAULT_BASE_URL) {
        *base_url = Some(OLLAMA_DEFAULT_BASE_URL.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_external_mcp_mode_fields_are_read_but_not_written() {
        let settings: ExternalMcpSettings = serde_json::from_value(serde_json::json!({
            "enabled": true,
            "permission_mode": "confirm",
            "session_scope": "current_window",
            "server_mode": "temporary",
            "idle_timeout_minutes": 10
        }))
        .expect("legacy External MCP settings");

        assert!(settings.enabled);
        assert_eq!(settings.permission_mode, AiPermissionMode::Confirm);
        assert_eq!(
            settings.session_scope,
            ExternalMcpSessionScope::CurrentWindow
        );
        let serialized = serde_json::to_value(settings).expect("serialized External MCP settings");
        assert!(serialized.get("server_mode").is_none());
        assert!(serialized.get("idle_timeout_minutes").is_none());
    }

    #[test]
    fn full_access_permission_mode_roundtrips_as_snake_case() {
        let serialized = serde_json::to_string(&AiPermissionMode::FullAccess)
            .expect("serialized permission mode");
        assert_eq!(serialized, "\"full_access\"");
        let parsed: AiPermissionMode =
            serde_json::from_str(&serialized).expect("parsed permission mode");
        assert_eq!(parsed, AiPermissionMode::FullAccess);
    }

    fn ollama_profile(settings: &AiSettings) -> &AiProviderProfile {
        settings
            .provider_profiles
            .iter()
            .find(|profile| profile.id == OLLAMA_PROVIDER_ID)
            .expect("ollama profile")
    }

    fn ollama_profile_mut(settings: &mut AiSettings) -> &mut AiProviderProfile {
        settings
            .provider_profiles
            .iter_mut()
            .find(|profile| profile.id == OLLAMA_PROVIDER_ID)
            .expect("ollama profile")
    }

    fn ollama_credential(settings: &AiSettings) -> &AiProviderCredential {
        settings
            .provider_credentials
            .iter()
            .find(|credential| credential.id == OLLAMA_PROVIDER_ID)
            .expect("ollama credential")
    }

    fn ollama_credential_mut(settings: &mut AiSettings) -> &mut AiProviderCredential {
        settings
            .provider_credentials
            .iter_mut()
            .find(|credential| credential.id == OLLAMA_PROVIDER_ID)
            .expect("ollama credential")
    }

    #[test]
    fn merge_preserves_masked_api_key() {
        let mut current = AiSettings::default();
        current.provider_profiles[0].api_key = Some("real-key".to_string());
        current.provider_credentials[0].api_key = Some("credential-key".to_string());
        current.provider_credentials[0].api_format = AiApiFormat::Responses;
        let mut next = current.clone();
        next.provider_profiles[0].api_key = Some(MASKED_SECRET_VALUE.to_string());
        next.provider_credentials[0].api_key = Some(MASKED_SECRET_VALUE.to_string());
        next.provider_credentials[0].api_format = AiApiFormat::Responses;

        let merged = merge_masked_ai_settings(&current, next);
        assert_eq!(
            merged.provider_profiles[0].api_key.as_deref(),
            Some("real-key")
        );
        assert_eq!(
            merged.provider_credentials[0].api_key.as_deref(),
            Some("credential-key")
        );
        assert_eq!(
            merged.provider_credentials[0].api_format,
            AiApiFormat::Responses
        );
    }

    #[test]
    fn mask_replaces_configured_api_key() {
        let mut settings = AiSettings::default();
        settings.provider_profiles[0].api_key = Some("real-key".to_string());
        settings.provider_credentials[0].api_key = Some("credential-key".to_string());

        let masked = mask_ai_settings(settings);
        assert_eq!(
            masked.provider_profiles[0].api_key.as_deref(),
            Some(MASKED_SECRET_VALUE)
        );
        assert_eq!(
            masked.provider_credentials[0].api_key.as_deref(),
            Some(MASKED_SECRET_VALUE)
        );
    }

    #[test]
    fn normalize_migrates_legacy_profiles_to_v2_settings() {
        let mut settings = AiSettings {
            schema_version: 2,
            provider_credentials: vec![],
            models: vec![],
            terminal_ai_actions: vec![],
            file_ai_actions: vec![],
            default_model_id: None,
            max_ai_file_size_bytes: 0,
            ..AiSettings::default()
        };
        settings.active_profile_id = "deepseek".to_string();

        assert!(normalize_ai_settings(&mut settings));
        assert_eq!(settings.schema_version, 6);
        assert!(!settings.provider_credentials.is_empty());
        assert!(
            settings
                .models
                .iter()
                .any(|model| model.name == "deepseek-chat")
        );
        assert_eq!(
            settings.default_model_id.as_deref(),
            Some("deepseek:deepseek-chat")
        );
        assert_eq!(settings.max_ai_file_size_bytes, 1_048_576);
        assert!(!settings.terminal_ai_actions.is_empty());
        assert!(!settings.file_ai_actions.is_empty());
        assert_eq!(
            settings.agent_command_execution_mode,
            AgentCommandExecutionMode::ConfirmEach
        );
        assert_eq!(settings.agent_smart_auto_execute_max_risk, RiskLevel::Low);
        assert!(!settings.agent_background_execution_enabled);
        assert!(!settings.codex.enabled);
        assert!(
            settings
                .models
                .iter()
                .all(|model| model.backend == AiBackendKind::Genai)
        );
    }

    #[test]
    fn legacy_provider_credentials_default_to_chat_completions() {
        let mut settings: AiSettings = serde_json::from_value(serde_json::json!({
            "schema_version": 5,
            "provider_credentials": [
                {
                    "id": "openai",
                    "name": "OpenAI",
                    "provider_kind": "openai",
                    "api_key": "key",
                    "enabled": true
                }
            ],
            "models": []
        }))
        .expect("legacy settings should deserialize");

        assert_eq!(
            settings.provider_credentials[0].api_format,
            AiApiFormat::ChatCompletions
        );

        assert!(normalize_ai_settings(&mut settings));
        assert_eq!(settings.schema_version, 6);
        assert_eq!(
            settings.provider_credentials[0].api_format,
            AiApiFormat::ChatCompletions
        );
    }

    #[test]
    fn normalize_migrates_v3_models_to_v4_genai_backend() {
        let mut settings: AiSettings = serde_json::from_value(serde_json::json!({
            "schema_version": 3,
            "models": [
                {
                    "id": "openai:gpt-4o-mini",
                    "name": "gpt-4o-mini",
                    "provider_kind": "openai",
                    "enabled": true,
                    "source": "rust-genai"
                }
            ],
            "provider_credentials": []
        }))
        .expect("legacy v3 settings should deserialize");

        assert!(normalize_ai_settings(&mut settings));

        assert_eq!(settings.schema_version, 6);
        assert_eq!(settings.models[0].backend, AiBackendKind::Genai);
        assert!(!settings.codex.enabled);
    }

    #[test]
    fn normalize_codex_models_clear_genai_provider_fields() {
        let mut settings = AiSettings {
            models: vec![AiModelConfigItem {
                id: String::new(),
                name: "gpt-5-codex".to_string(),
                backend: AiBackendKind::Codex,
                provider_kind: Some(AiProviderKind::Openai),
                credential_id: Some("openai".to_string()),
                enabled: true,
                source: AiModelSource::RustGenai,
                last_seen_at: None,
            }],
            ..AiSettings::default()
        };

        assert!(normalize_ai_settings(&mut settings));

        assert_eq!(settings.models[0].id, "codex:gpt-5-codex");
        assert_eq!(settings.models[0].backend, AiBackendKind::Codex);
        assert!(settings.models[0].provider_kind.is_none());
        assert!(settings.models[0].credential_id.is_none());
    }

    #[test]
    fn default_ai_settings_include_request_user_agent() {
        let settings = AiSettings::default();

        assert_eq!(
            settings.request_user_agent.as_str(),
            AI_REQUEST_USER_AGENT_DEFAULT
        );
    }

    #[test]
    fn default_ai_settings_use_ollama_native_base_url() {
        let settings = AiSettings::default();

        assert_eq!(
            ollama_profile(&settings).base_url.as_deref(),
            Some(OLLAMA_DEFAULT_BASE_URL)
        );
        assert_eq!(
            ollama_credential(&settings).base_url.as_deref(),
            Some(OLLAMA_DEFAULT_BASE_URL)
        );
    }

    #[test]
    fn normalize_migrates_legacy_builtin_ollama_profile_base_url() {
        let mut settings = AiSettings::default();
        ollama_profile_mut(&mut settings).base_url =
            Some(OLLAMA_LEGACY_DEFAULT_BASE_URL.to_string());

        assert!(normalize_ai_settings(&mut settings));

        assert_eq!(
            ollama_profile(&settings).base_url.as_deref(),
            Some(OLLAMA_DEFAULT_BASE_URL)
        );
    }

    #[test]
    fn normalize_migrates_legacy_builtin_ollama_credential_base_url() {
        let mut settings = AiSettings::default();
        ollama_credential_mut(&mut settings).base_url =
            Some(OLLAMA_LEGACY_DEFAULT_BASE_URL.to_string());

        assert!(normalize_ai_settings(&mut settings));

        assert_eq!(
            ollama_credential(&settings).base_url.as_deref(),
            Some(OLLAMA_DEFAULT_BASE_URL)
        );
    }

    #[test]
    fn normalize_keeps_custom_ollama_base_url() {
        let mut settings = AiSettings::default();
        ollama_profile_mut(&mut settings).base_url = Some("http://192.168.1.10:11434/".to_string());
        ollama_credential_mut(&mut settings).base_url =
            Some("http://192.168.1.10:11434/".to_string());

        normalize_ai_settings(&mut settings);

        assert_eq!(
            ollama_profile(&settings).base_url.as_deref(),
            Some("http://192.168.1.10:11434/")
        );
        assert_eq!(
            ollama_credential(&settings).base_url.as_deref(),
            Some("http://192.168.1.10:11434/")
        );
    }

    #[test]
    fn normalize_keeps_openai_compatible_v1_base_url() {
        let mut settings = AiSettings {
            provider_credentials: vec![AiProviderCredential {
                id: "credential-openai-compatible".to_string(),
                name: "OpenAI Compatible".to_string(),
                provider_kind: AiProviderKind::OpenaiCompatible,
                api_format: AiApiFormat::default(),
                base_url: Some(OLLAMA_LEGACY_DEFAULT_BASE_URL.to_string()),
                api_key: None,
                enabled: true,
            }],
            ..AiSettings::default()
        };

        normalize_ai_settings(&mut settings);

        assert_eq!(
            settings.provider_credentials[0].base_url.as_deref(),
            Some(OLLAMA_LEGACY_DEFAULT_BASE_URL)
        );
    }

    #[test]
    fn normalize_legacy_ollama_base_url_is_idempotent() {
        let mut settings = AiSettings::default();
        normalize_ai_settings(&mut settings);
        ollama_profile_mut(&mut settings).base_url =
            Some(OLLAMA_LEGACY_DEFAULT_BASE_URL.to_string());
        ollama_credential_mut(&mut settings).base_url =
            Some(OLLAMA_LEGACY_DEFAULT_BASE_URL.to_string());

        assert!(normalize_ai_settings(&mut settings));
        let normalized = serde_json::to_string(&settings).expect("serialize settings");

        assert!(!normalize_ai_settings(&mut settings));
        assert_eq!(
            serde_json::to_string(&settings).expect("serialize settings"),
            normalized
        );
    }

    #[test]
    fn legacy_ai_settings_default_background_execution_to_disabled() {
        let settings: AiSettings = serde_json::from_value(serde_json::json!({
            "schema_version": 3,
            "enabled": true
        }))
        .expect("legacy settings should deserialize");

        assert!(!settings.agent_background_execution_enabled);
    }
}
