use crate::config::MASKED_SECRET_VALUE;
use crate::error::AppResult;
use crate::utils::crypto;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiProviderKind {
    Openai,
    Anthropic,
    Gemini,
    Deepseek,
    Groq,
    Ollama,
    OpenaiCompatible,
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
pub struct AiSettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_context_line_limit")]
    pub context_line_limit: u32,
    #[serde(default = "default_true")]
    pub redaction_enabled: bool,
    #[serde(default = "default_true")]
    pub risk_check_enabled: bool,
    #[serde(default = "default_true")]
    pub allow_save_command: bool,
    #[serde(default = "default_true")]
    pub record_history: bool,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: u32,
    #[serde(default = "default_active_profile_id")]
    pub active_profile_id: String,
    #[serde(default = "default_provider_profiles")]
    pub provider_profiles: Vec<AiProviderProfile>,
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

fn default_max_output_tokens() -> u32 {
    1_200
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
            enabled: true,
        },
        AiProviderProfile {
            id: "anthropic".to_string(),
            name: "Anthropic".to_string(),
            provider_kind: AiProviderKind::Anthropic,
            model: "claude-3-haiku-20240307".to_string(),
            base_url: None,
            api_key: None,
            enabled: true,
        },
        AiProviderProfile {
            id: "gemini".to_string(),
            name: "Gemini".to_string(),
            provider_kind: AiProviderKind::Gemini,
            model: "gemini-2.0-flash".to_string(),
            base_url: None,
            api_key: None,
            enabled: true,
        },
        AiProviderProfile {
            id: "deepseek".to_string(),
            name: "DeepSeek".to_string(),
            provider_kind: AiProviderKind::Deepseek,
            model: "deepseek-chat".to_string(),
            base_url: None,
            api_key: None,
            enabled: true,
        },
        AiProviderProfile {
            id: "ollama".to_string(),
            name: "Ollama".to_string(),
            provider_kind: AiProviderKind::Ollama,
            model: "qwen2.5-coder:7b".to_string(),
            base_url: Some("http://localhost:11434/v1/".to_string()),
            api_key: None,
            enabled: true,
        },
        AiProviderProfile {
            id: "custom-openai".to_string(),
            name: "OpenAI Compatible".to_string(),
            provider_kind: AiProviderKind::OpenaiCompatible,
            model: "gpt-4o-mini".to_string(),
            base_url: Some("https://api.openai.com/v1/".to_string()),
            api_key: None,
            enabled: true,
        },
    ]
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            context_line_limit: default_context_line_limit(),
            redaction_enabled: true,
            risk_check_enabled: true,
            allow_save_command: true,
            record_history: true,
            timeout_ms: default_timeout_ms(),
            max_output_tokens: default_max_output_tokens(),
            active_profile_id: default_active_profile_id(),
            provider_profiles: default_provider_profiles(),
        }
    }
}

pub fn decrypt_ai_settings(mut settings: AiSettings) -> AppResult<AiSettings> {
    for profile in &mut settings.provider_profiles {
        profile.api_key = decrypt_secret(profile.api_key.take())?;
    }
    Ok(settings)
}

pub fn encrypt_ai_settings(mut settings: AiSettings) -> AppResult<AiSettings> {
    for profile in &mut settings.provider_profiles {
        profile.api_key = encrypt_secret(profile.api_key.take())?;
    }
    Ok(settings)
}

pub fn mask_ai_settings(mut settings: AiSettings) -> AiSettings {
    for profile in &mut settings.provider_profiles {
        profile.api_key = mask_secret(profile.api_key.take());
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
    next
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_preserves_masked_api_key() {
        let mut current = AiSettings::default();
        current.provider_profiles[0].api_key = Some("real-key".to_string());
        let mut next = current.clone();
        next.provider_profiles[0].api_key = Some(MASKED_SECRET_VALUE.to_string());

        let merged = merge_masked_ai_settings(&current, next);
        assert_eq!(
            merged.provider_profiles[0].api_key.as_deref(),
            Some("real-key")
        );
    }

    #[test]
    fn mask_replaces_configured_api_key() {
        let mut settings = AiSettings::default();
        settings.provider_profiles[0].api_key = Some("real-key".to_string());

        let masked = mask_ai_settings(settings);
        assert_eq!(
            masked.provider_profiles[0].api_key.as_deref(),
            Some(MASKED_SECRET_VALUE)
        );
    }
}
