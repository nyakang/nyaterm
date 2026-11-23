//! AI settings defaults, normalization and secret masking.
//!
//! Split out of `ai.rs` by domain. The `default_*` functions back the serde
//! attributes on the settings types, so they stay in scope there through the
//! re-export in `ai.rs`. Field defaults, the legacy profile migration and
//! which fields count as secrets are compatibility surface and are unchanged.

use std::collections::HashSet;

use super::provider_settings::normalize_custom_headers;
use super::{
    AI_REQUEST_USER_AGENT_DEFAULT, AgentCommandExecutionMode, AiAgentKind, AiApiFormat,
    AiBackendKind, AiCustomActionConfig, AiHttpHeader, AiMode, AiModelConfigItem, AiModelSource,
    AiPermissionMode, AiProviderCredential, AiProviderKind, AiProviderProfile, AiReasoningEffort,
    AiSettings, ClaudeCodeIntegrationSettings, CodexIntegrationSettings, ExternalMcpSettings,
    MASKED_SECRET_VALUE, RiskLevel, ai_model_id_for_credential, ai_model_id_for_provider,
};

const OLLAMA_PROVIDER_ID: &str = "ollama";
const OLLAMA_DEFAULT_BASE_URL: &str = "http://localhost:11434/";
const OLLAMA_LEGACY_DEFAULT_BASE_URL: &str = "http://localhost:11434/v1/";

pub fn mask_ai_settings(mut settings: AiSettings) -> AiSettings {
    for profile in settings.external_agents.iter_mut().flatten() {
        for variable in &mut profile.environment {
            if !variable.value.is_empty() {
                variable.value = MASKED_SECRET_VALUE.into();
            }
        }
    }
    settings.proxy.password = mask_secret(settings.proxy.password.take());
    for profile in &mut settings.provider_profiles {
        profile.api_key = mask_secret(profile.api_key.take());
    }
    for credential in &mut settings.provider_credentials {
        credential.api_key = mask_secret(credential.api_key.take());
        mask_custom_header_values(&mut credential.extra_headers);
    }
    settings
}

fn mask_custom_header_values(headers: &mut [AiHttpHeader]) {
    for header in headers.iter_mut() {
        if header.value.is_empty() {
            continue;
        }
        header.value = MASKED_SECRET_VALUE.into();
    }
}

pub fn merge_masked_ai_settings(current: &AiSettings, mut next: AiSettings) -> AiSettings {
    for profile in next.external_agents.iter_mut().flatten() {
        let previous = current
            .external_agents
            .iter()
            .flatten()
            .find(|item| item.id == profile.id);
        profile.merge_masked_environment(previous);
    }
    next.proxy.password = merge_secret(
        current.proxy.password.as_ref(),
        next.proxy.password.as_ref(),
    );
    for profile in &mut next.provider_profiles {
        let current_secret = current
            .provider_profiles
            .iter()
            .find(|item| item.id == profile.id)
            .and_then(|item| item.api_key.as_ref());
        profile.api_key = merge_secret(current_secret, profile.api_key.as_ref());
    }
    for credential in &mut next.provider_credentials {
        let current_credential = current
            .provider_credentials
            .iter()
            .find(|item| item.id == credential.id);
        let current_secret = current_credential.and_then(|item| item.api_key.as_ref());
        credential.api_key = merge_secret(current_secret, credential.api_key.as_ref());
        credential.extra_headers = merge_custom_header_values(
            current_credential.map(|item| item.extra_headers.as_slice()),
            &credential.extra_headers,
        );
    }
    normalize_ai_settings(&mut next);
    next
}

/// Merges masked custom header values against the stored credential.
///
/// Rows are matched by `id` (falling back to a case-insensitive name match
/// for rows without an id). A masked value keeps the stored secret, an empty
/// value clears it, and rows absent from `incoming` are dropped.
fn merge_custom_header_values(
    current: Option<&[AiHttpHeader]>,
    incoming: &[AiHttpHeader],
) -> Vec<AiHttpHeader> {
    let current = current.unwrap_or_default();
    let mut merged = Vec::with_capacity(incoming.len());
    for header in incoming {
        let name = header.name.trim_matches([' ', '\t']);
        let stored = if header.id.is_empty() {
            current
                .iter()
                .find(|item| item.name.eq_ignore_ascii_case(name))
        } else {
            current.iter().find(|item| item.id == header.id)
        };
        let incoming_value = header.value.expose_secret();
        let value = if incoming_value == MASKED_SECRET_VALUE {
            stored
                .map(|item| item.value.expose_secret().to_string())
                .unwrap_or_default()
        } else {
            incoming_value.to_string()
        };
        merged.push(AiHttpHeader {
            id: if header.id.is_empty() {
                stored.map(|item| item.id.clone()).unwrap_or_default()
            } else {
                header.id.clone()
            },
            name: name.to_string(),
            value: value.into(),
        });
    }
    merged
}

pub fn normalize_ai_settings(settings: &mut AiSettings) -> bool {
    let original = serde_json::to_string(settings).unwrap_or_default();

    super::external_agents::migrate_profiles(settings);
    settings.schema_version = settings.schema_version.max(default_schema_version());
    if settings.request_user_agent.trim().is_empty() {
        settings.request_user_agent = default_request_user_agent();
    }

    if settings.provider_profiles.is_empty() {
        settings.provider_profiles = default_provider_profiles();
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
    let mut header_ids = HashSet::new();
    for credential in &mut settings.provider_credentials {
        if credential.api_protocol.is_none()
            && is_builtin_ollama_provider(&credential.id, &credential.provider_kind)
        {
            migrate_legacy_ollama_base_url(&mut credential.base_url);
        }
        normalize_custom_headers(&mut credential.extra_headers);
        for header in &mut credential.extra_headers {
            let base = header.id.clone();
            for suffix in 0_u64.. {
                if header_ids.insert(header.id.clone()) {
                    break;
                }
                header.id = format!("{base}-{suffix}");
            }
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
    if settings.context_line_limit == 0 {
        settings.context_line_limit = default_context_line_limit();
    }
    if settings.timeout_ms == 0 {
        settings.timeout_ms = default_timeout_ms();
    }
    if settings.terminal_output_lines == 0 {
        settings.terminal_output_lines = default_terminal_output_lines();
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
            .find(|profile| profile.id == settings.active_profile_id && profile.enabled)
            .and_then(model_from_profile)
            .and_then(|legacy_model| {
                settings
                    .models
                    .iter()
                    .find(|model| model.enabled && model.id == legacy_model.id)
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

pub fn ai_settings_has_secret(settings: &AiSettings) -> bool {
    settings.external_agents.iter().flatten().any(|profile| {
        profile
            .environment
            .iter()
            .any(|variable| !variable.value.is_empty())
    }) || optional_secret_present(&settings.proxy.password)
        || settings
            .provider_profiles
            .iter()
            .any(|profile| optional_secret_present(&profile.api_key))
        || settings.provider_credentials.iter().any(|credential| {
            optional_secret_present(&credential.api_key)
                || credential
                    .extra_headers
                    .iter()
                    .any(|header| !header.value.is_empty())
        })
}

impl Default for AiSettings {
    fn default() -> Self {
        let models = default_models();
        let default_model_id = models
            .iter()
            .find(|item| item.enabled)
            .map(|item| item.id.clone());

        Self {
            external_agents: None,
            proxy: Default::default(),
            schema_version: default_schema_version(),
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

pub(super) fn default_schema_version() -> u32 {
    8
}

pub(super) fn default_codex_runtime() -> Option<String> {
    Some("app_server".to_string())
}

pub(super) fn default_claude_runtime() -> Option<String> {
    Some("stream_json_cli".to_string())
}

pub(super) fn default_tool_integration_mode() -> Option<String> {
    Some("nyaterm_mcp".to_string())
}

pub(super) fn default_true() -> bool {
    true
}

pub(super) fn default_context_line_limit() -> u32 {
    200
}

pub(super) fn default_timeout_ms() -> u64 {
    60_000
}

pub(super) fn default_request_user_agent() -> String {
    AI_REQUEST_USER_AGENT_DEFAULT.to_string()
}

pub(super) fn default_mode() -> AiMode {
    AiMode::Ask
}

pub(super) fn default_model_source() -> AiModelSource {
    AiModelSource::RustGenai
}

pub(super) fn default_terminal_output_lines() -> u16 {
    10
}

pub(super) fn default_agent_smart_auto_execute_max_risk() -> RiskLevel {
    RiskLevel::Low
}

pub(super) fn default_max_ai_file_size_bytes() -> u64 {
    1_048_576
}

pub(super) fn default_active_profile_id() -> String {
    "openai".to_string()
}

pub(super) fn default_provider_profiles() -> Vec<AiProviderProfile> {
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
            id: "ollama".to_string(),
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

pub(super) fn default_provider_credentials() -> Vec<AiProviderCredential> {
    default_provider_profiles()
        .iter()
        .map(credential_from_profile)
        .collect()
}

pub(super) fn default_models() -> Vec<AiModelConfigItem> {
    Vec::new()
}

pub(super) fn default_terminal_ai_actions() -> Vec<AiCustomActionConfig> {
    vec![
        AiCustomActionConfig {
            id: "explain-selected".to_string(),
            name: "\u{89e3}\u{91ca}\u{9009}\u{4e2d}\u{5185}\u{5bb9}".to_string(),
            prompt: "\u{8bf7}\u{89e3}\u{91ca}\u{7ec8}\u{7aef}\u{4e2d}\u{9009}\u{4e2d}\u{7684}\u{5185}\u{5bb9}\u{ff0c}\u{6307}\u{51fa}\u{542b}\u{4e49}\u{3001}\u{53ef}\u{80fd}\u{539f}\u{56e0}\u{548c}\u{4e0b}\u{4e00}\u{6b65}\u{5efa}\u{8bae}\u{3002}".to_string(),
            enabled: true,
        },
        AiCustomActionConfig {
            id: "generate-fix-command".to_string(),
            name: "\u{751f}\u{6210}\u{4fee}\u{590d}\u{547d}\u{4ee4}".to_string(),
            prompt: "\u{8bf7}\u{6839}\u{636e}\u{7ec8}\u{7aef}\u{9009}\u{4e2d}\u{5185}\u{5bb9}\u{751f}\u{6210}\u{53ef}\u{6267}\u{884c}\u{7684}\u{4fee}\u{590d}\u{547d}\u{4ee4}\u{ff0c}\u{5e76}\u{8bf4}\u{660e}\u{98ce}\u{9669}\u{3002}".to_string(),
            enabled: true,
        },
    ]
}

pub(super) fn default_file_ai_actions() -> Vec<AiCustomActionConfig> {
    vec![
        AiCustomActionConfig {
            id: "summarize-file".to_string(),
            name: "\u{603b}\u{7ed3}\u{6587}\u{4ef6}".to_string(),
            prompt: "\u{8bf7}\u{603b}\u{7ed3}\u{9009}\u{4e2d}\u{6587}\u{4ef6}\u{7684}\u{4e3b}\u{8981}\u{5185}\u{5bb9}\u{3001}\u{5173}\u{952e}\u{98ce}\u{9669}\u{548c}\u{5efa}\u{8bae}\u{64cd}\u{4f5c}\u{3002}".to_string(),
            enabled: true,
        },
        AiCustomActionConfig {
            id: "explain-file".to_string(),
            name: "\u{89e3}\u{91ca}\u{6587}\u{4ef6}".to_string(),
            prompt: "\u{8bf7}\u{89e3}\u{91ca}\u{9009}\u{4e2d}\u{6587}\u{4ef6}\u{7684}\u{7528}\u{9014}\u{3001}\u{7ed3}\u{6784}\u{548c}\u{5173}\u{952e}\u{5b57}\u{6bb5}\u{3002}".to_string(),
            enabled: true,
        },
    ]
}

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

pub(super) fn provider_kind_key(kind: &AiProviderKind) -> &'static str {
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

fn credential_from_profile(profile: &AiProviderProfile) -> AiProviderCredential {
    AiProviderCredential {
        icon_data_url: None,
        api_protocol: None,
        id: profile.id.clone(),
        name: profile.name.clone(),
        provider_kind: profile.provider_kind.clone(),
        api_format: AiApiFormat::ChatCompletions,
        base_url: profile.base_url.clone(),
        api_key: profile.api_key.clone(),
        extra_headers: Vec::new(),
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
        supported_reasoning_efforts: None,
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

fn is_builtin_ollama_provider(id: &str, provider_kind: &AiProviderKind) -> bool {
    id == OLLAMA_PROVIDER_ID && provider_kind == &AiProviderKind::Ollama
}

fn migrate_legacy_ollama_base_url(base_url: &mut Option<String>) {
    if base_url.as_deref() == Some(OLLAMA_LEGACY_DEFAULT_BASE_URL) {
        *base_url = Some(OLLAMA_DEFAULT_BASE_URL.to_string());
    }
}

fn mask_secret(value: Option<crate::SecretString>) -> Option<crate::SecretString> {
    value.and_then(|secret| {
        if secret.is_empty() {
            None
        } else {
            Some(MASKED_SECRET_VALUE.into())
        }
    })
}

fn merge_secret(
    current: Option<&crate::SecretString>,
    incoming: Option<&crate::SecretString>,
) -> Option<crate::SecretString> {
    match incoming.map(crate::SecretString::expose_secret) {
        Some(MASKED_SECRET_VALUE) | None => current.cloned(),
        Some("") => None,
        Some(value) => Some(value.into()),
    }
}

fn optional_secret_present(value: &Option<crate::SecretString>) -> bool {
    value.as_ref().is_some_and(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{
        AgentCommandExecutionMode, AiHttpHeader, AiSettings, MASKED_SECRET_VALUE, RiskLevel,
        mask_ai_settings, merge_masked_ai_settings, normalize_ai_settings,
    };

    #[test]
    fn merge_preserves_masked_api_key() {
        let mut current = AiSettings::default();
        current.provider_profiles[0].api_key = Some("real-key".to_string().into());
        current.provider_credentials[0].api_key = Some("credential-key".to_string().into());
        let mut next = current.clone();
        next.provider_profiles[0].api_key = Some(MASKED_SECRET_VALUE.to_string().into());
        next.provider_credentials[0].api_key = Some(MASKED_SECRET_VALUE.to_string().into());

        let merged = merge_masked_ai_settings(&current, next);
        assert_eq!(
            merged.provider_profiles[0].api_key.as_deref(),
            Some("real-key")
        );
        assert_eq!(
            merged.provider_credentials[0].api_key.as_deref(),
            Some("credential-key")
        );
    }

    #[test]
    fn mask_replaces_configured_api_key() {
        let mut settings = AiSettings::default();
        settings.provider_profiles[0].api_key = Some("real-key".to_string().into());
        settings.provider_credentials[0].api_key = Some("credential-key".to_string().into());

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

    fn test_header(id: &str, name: &str, value: &str) -> AiHttpHeader {
        AiHttpHeader {
            id: id.to_string(),
            name: name.to_string(),
            value: value.into(),
        }
    }

    #[test]
    fn custom_headers_normalize_preserves_rows_and_assigns_unique_ids() {
        let mut settings = AiSettings::default();
        settings.provider_credentials[0].extra_headers = vec![
            test_header("", "  X-Custom  ", "  one  "),
            test_header("keep-id", "x-custom", "two"),
            test_header("", "", "dropped-no-name"),
            test_header("", "X-Empty", ""),
            test_header("", "Authorization", "Bearer override"),
        ];

        assert!(normalize_ai_settings(&mut settings));

        let headers = &settings.provider_credentials[0].extra_headers;
        assert_eq!(headers.len(), 5);
        let custom = headers
            .iter()
            .find(|header| header.name == "x-custom")
            .expect("x-custom header");
        assert_eq!(custom.value.expose_secret(), "two");
        assert_eq!(custom.id, "keep-id");
        let auth = headers
            .iter()
            .find(|header| header.name == "Authorization")
            .expect("authorization header");
        assert_eq!(auth.value.expose_secret(), "Bearer override");
        assert!(!auth.id.is_empty());
    }

    #[test]
    fn normalize_preserves_excess_headers_for_validation() {
        use crate::ai::MAX_AI_CUSTOM_HEADERS;
        let mut settings = AiSettings::default();
        settings.provider_credentials[0].extra_headers = (0..MAX_AI_CUSTOM_HEADERS + 5)
            .map(|index| test_header("", &format!("X-Header-{index}"), "value"))
            .collect();

        normalize_ai_settings(&mut settings);

        assert_eq!(
            settings.provider_credentials[0].extra_headers.len(),
            MAX_AI_CUSTOM_HEADERS + 5
        );
    }

    #[test]
    fn custom_header_values_are_masked_and_merged() {
        let mut settings = AiSettings::default();
        settings.provider_credentials[0].extra_headers = vec![
            test_header("h1", "X-Token", "secret-1"),
            test_header("h2", "X-Other", "secret-2"),
        ];

        let masked = mask_ai_settings(settings.clone());
        assert_eq!(
            masked.provider_credentials[0].extra_headers[0]
                .value
                .expose_secret(),
            MASKED_SECRET_VALUE
        );

        let mut next = masked;
        // h1 keeps the masked value, h2 is cleared, h3 is new.
        next.provider_credentials[0].extra_headers[1].value = "".into();
        next.provider_credentials[0]
            .extra_headers
            .push(test_header("h3", "X-New", "secret-3"));
        let merged = merge_masked_ai_settings(&settings, next);
        let headers = &merged.provider_credentials[0].extra_headers;
        assert_eq!(headers.len(), 3);
        assert_eq!(headers[0].id, "h1");
        assert_eq!(headers[0].name, "X-Token");
        assert_eq!(headers[0].value.expose_secret(), "secret-1");
        assert!(headers[1].value.is_empty());
        assert_eq!(headers[2].id, "h3");
        assert_eq!(headers[2].value.expose_secret(), "secret-3");
    }

    #[test]
    fn imported_header_ids_normalize_stably_and_masked_values_keep_provider_ownership() {
        let mut imported = AiSettings::default();
        for (index, credential) in imported.provider_credentials.iter_mut().take(2).enumerate() {
            credential.extra_headers = vec![
                test_header("shared", "X-Token", &format!("fixture-{index}")),
                test_header("shared", "X-Other", "other-fixture"),
                test_header("", "X-Missing-Id", "missing-id-fixture"),
            ];
        }
        let mut current = imported.clone();
        let mut second_load = imported;
        normalize_ai_settings(&mut current);
        normalize_ai_settings(&mut second_load);
        assert_eq!(current, second_load);
        let ids: std::collections::HashSet<_> = current
            .provider_credentials
            .iter()
            .flat_map(|credential| credential.extra_headers.iter().map(|header| &header.id))
            .collect();
        assert_eq!(ids.len(), 6);
        assert_eq!(
            merge_masked_ai_settings(&current, mask_ai_settings(second_load)),
            current
        );
        let mut edited = mask_ai_settings(current.clone());
        edited.provider_credentials[0].extra_headers[0].value = "replacement-fixture".into();
        let merged = merge_masked_ai_settings(&current, edited);
        assert_eq!(
            merged.provider_credentials[1].extra_headers[0]
                .value
                .expose_secret(),
            "fixture-1"
        );
    }

    #[test]
    fn masked_header_with_new_id_does_not_capture_an_existing_names_secret() {
        let mut current = AiSettings::default();
        current.provider_credentials[0].extra_headers =
            vec![test_header("old", "X-Token", "fixture")];
        let mut next = mask_ai_settings(current.clone());
        next.provider_credentials[0].extra_headers[0].id = "new".into();
        let merged = merge_masked_ai_settings(&current, next);
        assert!(
            merged.provider_credentials[0].extra_headers[0]
                .value
                .is_empty()
        );
    }

    #[test]
    fn normalize_migrates_legacy_profiles_to_v7_settings() {
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
        settings.provider_profiles[3].enabled = true;

        assert!(normalize_ai_settings(&mut settings));
        assert_eq!(settings.schema_version, 8);
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
