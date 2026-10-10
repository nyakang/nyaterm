use std::collections::BTreeMap;
use std::time::Duration;

use genai::adapter::AdapterKind;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, ReasoningEffort};
use genai::resolver::{AuthData, Endpoint, ServiceTargetResolver};
use genai::{Client, ModelIden};
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use serde_json::Value;

use crate::config::{
    self, AI_REQUEST_USER_AGENT_DEFAULT, AiApiFormat, AiBackendKind, AiModelConfigItem,
    AiModelSource, AiProviderApiProtocol, AiProviderCredential, AiProviderKind, AiReasoningEffort,
    AiSettings, ai_model_id_for_credential, ai_model_id_for_provider,
};
use crate::error::{AppError, AppResult};
use crate::utils::url::{join_api_base_url, normalize_api_base_url};

use super::http::build_http_client;
use super::types::{AiChatRequest, AiModelDiscovery};

const MODEL_TEST_SYSTEM_PROMPT: &str = "You are NyaTerm's terminal AI connectivity check. Reply with OK only; do not suggest or run commands.";
const MODEL_TEST_USER_PROMPT: &str = "Confirm that this model can respond.";
const MODEL_TEST_MAX_OUTPUT_TOKENS: u32 = 64;

#[derive(Debug, Clone)]
pub struct ResolvedAiModel {
    pub model_name: String,
    pub provider_kind: AiProviderKind,
    pub api_format: AiApiFormat,
    pub credential: Option<AiProviderCredential>,
}

pub fn resolve_request_model_config(
    settings: &AiSettings,
    request: &AiChatRequest,
) -> AppResult<AiModelConfigItem> {
    request
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
        .cloned()
        .ok_or_else(|| AppError::Config("No enabled AI model configured".to_string()))
}

pub fn build_chat_options(settings: &AiSettings) -> ChatOptions {
    let mut options = ChatOptions::default()
        .with_capture_reasoning_content(true)
        .with_normalize_reasoning_content(true);

    if let Some(reasoning_effort) = genai_reasoning_effort(&settings.default_reasoning_effort) {
        options = options.with_reasoning_effort(reasoning_effort);
    } else if matches!(settings.default_reasoning_effort, AiReasoningEffort::Ultra) {
        options = options.with_extra_body(serde_json::json!({ "reasoning_effort": "ultra" }));
    }

    options
}

fn genai_reasoning_effort(value: &AiReasoningEffort) -> Option<ReasoningEffort> {
    match value {
        AiReasoningEffort::Auto => None,
        AiReasoningEffort::None => Some(ReasoningEffort::None),
        AiReasoningEffort::Minimal => Some(ReasoningEffort::Minimal),
        AiReasoningEffort::Low => Some(ReasoningEffort::Low),
        AiReasoningEffort::Medium => Some(ReasoningEffort::Medium),
        AiReasoningEffort::High => Some(ReasoningEffort::High),
        AiReasoningEffort::XHigh => Some(ReasoningEffort::XHigh),
        AiReasoningEffort::Max => Some(ReasoningEffort::Max),
        AiReasoningEffort::Ultra => None,
    }
}

pub fn resolve_request_model(
    settings: &AiSettings,
    request: &AiChatRequest,
) -> AppResult<ResolvedAiModel> {
    tracing::debug!(
        requested_model_id = ?request.model_id,
        default_model_id = ?settings.default_model_id,
        enabled_model_count = settings.models.iter().filter(|model| model.enabled).count(),
        "Resolving AI model for request"
    );

    let selected_model = resolve_request_model_config(settings, request)?;

    resolve_model_config(settings, &selected_model)
}

pub fn resolve_model_config(
    settings: &AiSettings,
    selected_model: &AiModelConfigItem,
) -> AppResult<ResolvedAiModel> {
    if selected_model.backend == AiBackendKind::Codex {
        return Err(AppError::Config(
            "Codex models must be routed through codex app-server".to_string(),
        ));
    }

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
        .ok_or_else(|| {
            AppError::Config(format!(
                "AI model '{}' is missing provider information",
                selected_model.name
            ))
        })?;
    validate_model_credential(&provider_kind, credential.as_ref())?;

    tracing::info!(
        resolved_model_id = %selected_model.id,
        resolved_model_name = %selected_model.name,
        provider_kind = ?provider_kind,
        credential_id = ?credential.as_ref().map(|item| item.id.as_str()),
        "Resolved AI model"
    );

    Ok(ResolvedAiModel {
        model_name: selected_model.name.clone(),
        provider_kind,
        api_format: credential
            .as_ref()
            .map(|credential| credential.api_format.clone())
            .unwrap_or_default(),
        credential,
    })
}

fn infer_provider_kind_from_model_id(model_id: &str) -> Option<AiProviderKind> {
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

fn resolve_model_credential(
    settings: &AiSettings,
    model: &AiModelConfigItem,
    provider_kind: Option<&AiProviderKind>,
) -> AppResult<Option<AiProviderCredential>> {
    // Legacy built-in models use the provider ID as their implicit credential ID.
    // Never let a newly added account of the same kind capture those models.
    let implicit_credential_id = model
        .id
        .split_once(':')
        .map(|(prefix, _)| prefix)
        .filter(|prefix| is_builtin_ai_provider_credential_id(prefix));
    if let Some(credential_id) = model.credential_id.as_deref().or(implicit_credential_id) {
        let credential = settings
            .provider_credentials
            .iter()
            .find(|item| item.id == credential_id && item.enabled)
            .cloned()
            .ok_or_else(|| {
                AppError::Config(format!(
                    "No enabled AI credential found for model '{}'",
                    model.name
                ))
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

fn validate_model_credential(
    provider_kind: &AiProviderKind,
    credential: Option<&AiProviderCredential>,
) -> AppResult<()> {
    match provider_kind {
        AiProviderKind::Ollama => Ok(()),
        AiProviderKind::OpenaiCompatible => {
            if credential.is_none() {
                return Err(AppError::Config(
                    "No enabled OpenAI-compatible AI credential configured".to_string(),
                ));
            }
            Ok(())
        }
        _ => {
            let credential = credential.ok_or_else(|| {
                AppError::Config(format!(
                    "No enabled AI credential configured for {:?}",
                    provider_kind
                ))
            })?;
            if credential
                .api_key
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            {
                return Err(AppError::Config(format!(
                    "No API key configured for AI credential '{}'",
                    credential.name
                )));
            }
            Ok(())
        }
    }
}

pub fn build_client(model: &ResolvedAiModel, settings: &AiSettings) -> AppResult<Client> {
    tracing::debug!(
        model_name = %model.model_name,
        provider_kind = ?model.provider_kind,
        has_credential = model.credential.is_some(),
        has_base_url = model
            .credential
            .as_ref()
            .and_then(|credential| credential.base_url.as_deref())
            .is_some_and(|value| !value.trim().is_empty()),
        "Building AI client"
    );

    let adapter_kind = adapter_kind_for_model(model);
    let mapped_model = genai_model_name(&model.provider_kind, &model.model_name);
    let api_key = model
        .credential
        .as_ref()
        .and_then(|credential| credential.api_key.clone())
        .filter(|value| !value.trim().is_empty());
    let base_url = model
        .credential
        .as_ref()
        .and_then(|credential| credential.base_url.as_deref())
        .map(normalize_api_base_url)
        .transpose()?
        .filter(|value| !value.trim().is_empty());
    let allows_empty_auth = model.provider_kind == AiProviderKind::OpenaiCompatible
        || (model.provider_kind == AiProviderKind::Ollama
            && model
                .credential
                .as_ref()
                .and_then(|credential| credential.api_protocol.as_ref())
                == Some(&AiProviderApiProtocol::OpenaiCompatible));

    let resolver =
        ServiceTargetResolver::from_resolver_fn(move |service_target: genai::ServiceTarget| {
            Ok(apply_service_target_overrides(
                service_target,
                api_key.clone(),
                base_url.clone(),
                allows_empty_auth,
            ))
        });

    Ok(Client::builder()
        .with_model_mapper_fn(move |_model| Ok(ModelIden::new(adapter_kind, mapped_model.clone())))
        .with_service_target_resolver(resolver)
        .with_reqwest(build_http_client(settings)?)
        .build())
}

fn apply_service_target_overrides(
    mut service_target: genai::ServiceTarget,
    api_key: Option<String>,
    base_url: Option<String>,
    allows_empty_auth: bool,
) -> genai::ServiceTarget {
    if let Some(api_key) = api_key {
        service_target.auth = AuthData::from_single(api_key);
    } else if allows_empty_auth {
        service_target.auth = AuthData::None;
    }
    if let Some(base_url) = base_url {
        service_target.endpoint = Endpoint::from_owned(base_url);
    }
    service_target
}

fn effective_request_user_agent(settings: &AiSettings) -> &str {
    let value = settings.request_user_agent.trim();
    if value.is_empty() {
        AI_REQUEST_USER_AGENT_DEFAULT
    } else {
        value
    }
}

pub fn ai_request_headers(settings: &AiSettings) -> AppResult<HeaderMap> {
    let user_agent = effective_request_user_agent(settings);
    let user_agent_value = HeaderValue::from_str(user_agent).map_err(|error| {
        AppError::Config(format!("Invalid AI User-Agent header value: {error}"))
    })?;
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, user_agent_value);
    Ok(headers)
}

fn adapter_kind(kind: &AiProviderKind) -> AdapterKind {
    match kind {
        AiProviderKind::Openai | AiProviderKind::OpenaiCompatible => AdapterKind::OpenAI,
        AiProviderKind::Anthropic => AdapterKind::Anthropic,
        AiProviderKind::Gemini => AdapterKind::Gemini,
        AiProviderKind::Deepseek => AdapterKind::DeepSeek,
        AiProviderKind::Groq => AdapterKind::Groq,
        AiProviderKind::Ollama => AdapterKind::Ollama,
        AiProviderKind::Xai
        | AiProviderKind::Cohere
        | AiProviderKind::Mimo
        | AiProviderKind::Zai => AdapterKind::OpenAI,
    }
}

fn adapter_kind_for_model(model: &ResolvedAiModel) -> AdapterKind {
    match model
        .credential
        .as_ref()
        .and_then(|credential| credential.api_protocol.as_ref())
    {
        None => adapter_kind(&model.provider_kind),
        Some(AiProviderApiProtocol::OpenaiCompatible) => match model.provider_kind {
            AiProviderKind::Deepseek => AdapterKind::DeepSeek,
            AiProviderKind::Groq => AdapterKind::Groq,
            _ => AdapterKind::OpenAI,
        },
        Some(AiProviderApiProtocol::Anthropic) => AdapterKind::Anthropic,
        Some(AiProviderApiProtocol::Gemini) => AdapterKind::Gemini,
        Some(AiProviderApiProtocol::Ollama) => AdapterKind::Ollama,
    }
}

fn genai_model_name(provider_kind: &AiProviderKind, model_name: &str) -> String {
    if matches!(provider_kind, AiProviderKind::Deepseek)
        && let Some(base_model_name) = model_name.strip_suffix("-none")
    {
        return base_model_name.to_string();
    }

    model_name.to_string()
}

pub async fn list_model_names(app: &impl Sized) -> AppResult<Vec<AiModelDiscovery>> {
    let settings = config::load_app_settings(app)?;
    list_model_names_for_settings(&settings.ai).await
}

pub async fn list_model_names_for_settings(
    settings: &AiSettings,
) -> AppResult<Vec<AiModelDiscovery>> {
    let credentials = model_discovery_credentials(settings);

    let mut models = BTreeMap::new();
    let mut errors = Vec::new();

    for credential in credentials {
        let label = credential.name.as_str();
        tracing::info!(
            credential = label,
            provider_kind = ?credential.provider_kind,
            "Fetching model list from provider"
        );
        match fetch_credential_model_names(credential, settings).await {
            Ok(names) => {
                tracing::info!(
                    credential = label,
                    count = names.len(),
                    "Fetched models from provider"
                );
                for name in names {
                    let trimmed = name.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let builtin = is_builtin_ai_provider_credential_id(&credential.id);
                    let id = if builtin {
                        ai_model_id_for_provider(&credential.provider_kind, trimmed)
                    } else {
                        ai_model_id_for_credential(&credential.id, trimmed)
                    };
                    models.entry(id.clone()).or_insert(AiModelDiscovery {
                        id,
                        name: trimmed.to_string(),
                        backend: AiBackendKind::Genai,
                        provider_kind: Some(credential.provider_kind.clone()),
                        credential_id: if builtin {
                            None
                        } else {
                            Some(credential.id.clone())
                        },
                        source: AiModelSource::RustGenai,
                    });
                }
            }
            Err(error) => {
                tracing::warn!(credential = label, %error, "Failed to fetch models from provider");
                errors.push(format!("{label}: {error}"));
            }
        }
    }

    if models.is_empty() && !errors.is_empty() {
        return Err(AppError::Config(format!(
            "Failed to list AI models: {}",
            errors.join("; ")
        )));
    }

    Ok(models.into_values().collect())
}

pub async fn test_provider_connection(
    settings: &AiSettings,
    credential_id: &str,
) -> AppResult<Vec<String>> {
    let credential = settings
        .provider_credentials
        .iter()
        .find(|credential| credential.id == credential_id && credential.enabled)
        .ok_or_else(|| AppError::Config("No enabled AI provider selected".to_string()))?;

    fetch_credential_model_names(credential, settings).await
}

async fn fetch_credential_model_names(
    credential: &AiProviderCredential,
    settings: &AiSettings,
) -> AppResult<Vec<String>> {
    let base_url = credential.base_url.as_deref().unwrap_or_default().trim();
    if base_url.is_empty() {
        return Err(AppError::Config(
            "AI provider API base URL is required".to_string(),
        ));
    }

    let api_key = credential
        .api_key
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    let protocol =
        credential
            .api_protocol
            .clone()
            .unwrap_or_else(|| match &credential.provider_kind {
                AiProviderKind::Anthropic => AiProviderApiProtocol::Anthropic,
                AiProviderKind::Gemini => AiProviderApiProtocol::Gemini,
                AiProviderKind::Ollama => AiProviderApiProtocol::Ollama,
                _ => AiProviderApiProtocol::OpenaiCompatible,
            });
    match protocol {
        AiProviderApiProtocol::OpenaiCompatible => {
            fetch_openai_compatible_models(base_url, api_key, settings).await
        }
        AiProviderApiProtocol::Anthropic => {
            fetch_provider_model_names(
                base_url,
                api_key,
                settings,
                "models",
                Some(("limit", "1000")),
                "data",
                "id",
                AiProviderApiProtocol::Anthropic,
            )
            .await
        }
        AiProviderApiProtocol::Gemini => {
            fetch_provider_model_names(
                base_url,
                api_key,
                settings,
                "models",
                Some(("pageSize", "1000")),
                "models",
                "name",
                AiProviderApiProtocol::Gemini,
            )
            .await
        }
        AiProviderApiProtocol::Ollama => {
            fetch_provider_model_names(
                base_url,
                api_key,
                settings,
                "api/tags",
                None,
                "models",
                "name",
                AiProviderApiProtocol::Ollama,
            )
            .await
        }
    }
}

async fn fetch_provider_model_names(
    base_url: &str,
    api_key: Option<&str>,
    settings: &AiSettings,
    path: &str,
    query: Option<(&str, &str)>,
    response_field: &str,
    model_name_field: &str,
    protocol: AiProviderApiProtocol,
) -> AppResult<Vec<String>> {
    let url = join_api_base_url(base_url, path)?;
    let mut url = reqwest::Url::parse(&url)
        .map_err(|error| AppError::Config(format!("Invalid model list URL: {error}")))?;
    if let Some((name, value)) = query {
        url.query_pairs_mut().append_pair(name, value);
    }
    let client = build_http_client(settings)?;
    let mut names = Vec::new();
    let mut cursor: Option<(&str, String)> = None;
    let mut seen_cursors = std::collections::HashSet::new();
    for _ in 0..100 {
        let mut page_url = url.clone();
        if let Some((key, value)) = &cursor {
            page_url.query_pairs_mut().append_pair(key, value);
        }
        let mut request = client
            .get(page_url.as_str())
            .timeout(Duration::from_secs(15));
        if let Some(key) = api_key {
            request = match protocol {
                AiProviderApiProtocol::Anthropic => request
                    .header("x-api-key", key)
                    .header("anthropic-version", "2023-06-01"),
                AiProviderApiProtocol::Gemini => request.header("x-goog-api-key", key),
                AiProviderApiProtocol::Ollama => request.bearer_auth(key),
                AiProviderApiProtocol::OpenaiCompatible => request.bearer_auth(key),
            };
        } else if protocol == AiProviderApiProtocol::Anthropic {
            request = request.header("anthropic-version", "2023-06-01");
        }
        let response = request.send().await.map_err(|error| {
            AppError::Config(format!("Failed to fetch models from {url}: {error}"))
        })?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::Config(format!(
                "Failed to fetch models from {url}: {status} {body}"
            )));
        }
        let body: Value = response
            .json()
            .await
            .map_err(|error| AppError::Config(format!("Invalid JSON from {url}: {error}")))?;
        let entries = body[response_field]
            .as_array()
            .ok_or_else(|| AppError::Config(format!("Invalid model list from {url}")))?;
        for entry in entries {
            let name = entry[model_name_field]
                .as_str()
                .filter(|name| !name.trim().is_empty())
                .ok_or_else(|| AppError::Config("Invalid model list: missing model ID".into()))?;
            names.push(if protocol == AiProviderApiProtocol::Gemini {
                name.strip_prefix("models/").unwrap_or(name).to_string()
            } else {
                name.to_string()
            });
        }
        cursor = next_models_cursor(&body, &protocol)?;
        match &cursor {
            None => return Ok(names),
            Some((_, value)) if !seen_cursors.insert(value.clone()) => {
                return Err(AppError::Config(
                    "Model list pagination repeated a cursor".into(),
                ));
            }
            _ => {}
        }
    }
    Err(AppError::Config(
        "Model list pagination exceeded 100 pages".into(),
    ))
}

fn next_models_cursor(
    body: &Value,
    protocol: &AiProviderApiProtocol,
) -> AppResult<Option<(&'static str, String)>> {
    let (key, value) = match protocol {
        AiProviderApiProtocol::Anthropic if body["has_more"].as_bool() == Some(true) => {
            let value = body["last_id"]
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    AppError::Config("Model list has more pages but no last_id".into())
                })?;
            ("after_id", value)
        }
        AiProviderApiProtocol::Gemini => {
            let Some(value) = body["nextPageToken"]
                .as_str()
                .filter(|value| !value.is_empty())
            else {
                return Ok(None);
            };
            ("pageToken", value)
        }
        _ => return Ok(None),
    };
    Ok(Some((key, value.to_string())))
}

fn model_discovery_credentials(settings: &AiSettings) -> Vec<&AiProviderCredential> {
    settings
        .provider_credentials
        .iter()
        .filter(|credential| credential.enabled)
        .collect()
}

fn is_builtin_ai_provider_credential_id(id: &str) -> bool {
    matches!(
        id,
        "openai"
            | "anthropic"
            | "gemini"
            | "deepseek"
            | "groq"
            | "ollama"
            | "xai"
            | "cohere"
            | "mimo"
            | "zai"
    )
}

/// Fetches model names from an OpenAI-compatible `/v1/models` endpoint directly via HTTP,
/// bypassing `genai::Client::all_model_names` which does not apply the `ServiceTargetResolver`
/// (and therefore ignores custom auth/endpoint configuration).
async fn fetch_openai_compatible_models(
    base_url: &str,
    api_key: Option<&str>,
    settings: &AiSettings,
) -> AppResult<Vec<String>> {
    let url = openai_compatible_models_url(base_url)?;
    let client = build_http_client(settings)?;
    let mut req = client.get(&url);
    if let Some(key) = api_key {
        req = req.bearer_auth(key);
    }
    let resp = req
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| AppError::Config(format!("Failed to fetch models from {url}: {e}")))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Config(format!(
            "Failed to fetch models from {url}: {status} {body}"
        )));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| AppError::Config(format!("Invalid JSON from {url}: {e}")))?;
    parse_openai_model_names(&body)
}

fn parse_openai_model_names(body: &Value) -> AppResult<Vec<String>> {
    let models = body["data"].as_array().ok_or_else(|| {
        AppError::Config("Invalid models response: expected a data array".to_string())
    })?;
    models
        .iter()
        .map(|item| {
            item["id"]
                .as_str()
                .filter(|name| !name.trim().is_empty())
                .map(String::from)
                .ok_or_else(|| {
                    AppError::Config("Invalid models response: missing model ID".to_string())
                })
        })
        .collect()
}

fn openai_compatible_models_url(base_url: &str) -> AppResult<String> {
    join_api_base_url(base_url, "models")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ai::types::{AiAction, AiContext, AiRequestOptions};
    use genai::resolver::{AuthData, Endpoint};

    fn test_service_target(auth: AuthData) -> genai::ServiceTarget {
        genai::ServiceTarget {
            endpoint: Endpoint::from_static("https://default.example/v1/"),
            auth,
            model: ModelIden::new(AdapterKind::OpenAI, "test-model"),
        }
    }

    #[test]
    fn builtin_models_keep_their_original_account() {
        let mut settings = AiSettings::default();
        // Defaults intentionally contain no models. Exercise a legacy built-in
        // entry explicitly instead of depending on the new-user catalog.
        settings.models = vec![
            serde_json::from_value(serde_json::json!({
                "id":"openai:test-model", "name":"Test", "model":"test-model",
                "source":"rust-genai", "provider_kind":"openai", "credential_id":"openai",
                "enabled":true
            }))
            .unwrap(),
        ];
        settings.provider_credentials = vec![
            test_credential_with_id("credential-new", AiProviderKind::Openai, Some("new-key")),
            test_credential_with_id("openai", AiProviderKind::Openai, Some("original-key")),
        ];
        let model = settings
            .models
            .iter()
            .find(|model| model.id.starts_with("openai:"))
            .unwrap();
        let credential = resolve_model_credential(&settings, model, Some(&AiProviderKind::Openai))
            .unwrap()
            .unwrap();
        assert_eq!(credential.id, "openai");

        settings.provider_credentials[1].enabled = false;
        let model = settings
            .models
            .iter()
            .find(|model| model.id.starts_with("openai:"))
            .unwrap();
        assert!(resolve_model_credential(&settings, model, Some(&AiProviderKind::Openai)).is_err());
        settings.provider_credentials.pop();
        let model = settings
            .models
            .iter()
            .find(|model| model.id.starts_with("openai:"))
            .unwrap();
        assert!(resolve_model_credential(&settings, model, Some(&AiProviderKind::Openai)).is_err());
    }

    #[test]
    fn models_response_distinguishes_empty_lists_from_invalid_payloads() {
        assert_eq!(
            parse_openai_model_names(&serde_json::json!({"data": []})).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            parse_openai_model_names(&serde_json::json!({"data": [{"id": "model-a"}]})).unwrap(),
            vec!["model-a"]
        );
        assert!(parse_openai_model_names(&serde_json::json!({"error": "unauthorized"})).is_err());
        assert!(parse_openai_model_names(&serde_json::json!({"data": [{}]})).is_err());
    }

    #[test]
    fn model_pagination_uses_protocol_specific_cursors() {
        assert_eq!(
            next_models_cursor(
                &serde_json::json!({"has_more": true, "last_id": "claude-a"}),
                &AiProviderApiProtocol::Anthropic
            )
            .unwrap(),
            Some(("after_id", "claude-a".into()))
        );
        assert!(
            next_models_cursor(
                &serde_json::json!({"has_more": true}),
                &AiProviderApiProtocol::Anthropic
            )
            .is_err()
        );
        assert_eq!(
            next_models_cursor(
                &serde_json::json!({"nextPageToken": "page-2"}),
                &AiProviderApiProtocol::Gemini
            )
            .unwrap(),
            Some(("pageToken", "page-2".into()))
        );
        assert_eq!(
            next_models_cursor(
                &serde_json::json!({"has_more": false}),
                &AiProviderApiProtocol::Anthropic
            )
            .unwrap(),
            None
        );
        assert_eq!(
            next_models_cursor(&serde_json::json!({}), &AiProviderApiProtocol::Gemini).unwrap(),
            None
        );
    }

    fn test_credential(kind: AiProviderKind, api_key: Option<&str>) -> AiProviderCredential {
        test_credential_with_id("credential-test", kind, api_key)
    }

    fn test_credential_with_id(
        id: &str,
        kind: AiProviderKind,
        api_key: Option<&str>,
    ) -> AiProviderCredential {
        AiProviderCredential {
            id: id.to_string(),
            name: "Test Provider".to_string(),
            provider_kind: kind,
            icon_data_url: None,
            api_protocol: None,
            api_format: AiApiFormat::default(),
            base_url: Some("https://api.example.com/v1/".to_string()),
            api_key: api_key.map(str::to_string),
            enabled: true,
        }
    }

    fn test_request(model_id: &str) -> AiChatRequest {
        AiChatRequest {
            stream_id: None,
            session_id: None,
            connection_id: None,
            terminal_session_id: None,
            owner_scope: Default::default(),
            targets: vec![],
            target_contexts: vec![],
            mode: crate::config::AiMode::Ask,
            agent_kind: crate::config::AiAgentKind::Nyaterm,
            permission_mode: crate::config::AiPermissionMode::Confirm,
            model_id: Some(model_id.to_string()),
            model_name: None,
            default_target_session_id: None,
            existing_external_session_id: None,
            attachments: vec![],
            references: vec![],
            action: AiAction::GenerateCommand,
            user_input: "test".to_string(),
            context: AiContext::default(),
            options: AiRequestOptions::default(),
        }
    }

    #[test]
    fn openai_compatible_models_url_accepts_missing_trailing_slash() {
        assert_eq!(
            openai_compatible_models_url("https://api.example.com/v1").unwrap(),
            "https://api.example.com/v1/models"
        );
    }

    #[test]
    fn openai_compatible_models_url_accepts_trailing_slash() {
        assert_eq!(
            openai_compatible_models_url("https://api.example.com/v1/").unwrap(),
            "https://api.example.com/v1/models"
        );
    }

    #[test]
    fn openai_compatible_models_url_preserves_query() {
        assert_eq!(
            openai_compatible_models_url("https://api.example.com/v1?api-version=1").unwrap(),
            "https://api.example.com/v1/models?api-version=1"
        );
    }

    #[test]
    fn ai_request_headers_use_custom_user_agent() {
        let mut settings = AiSettings::default();
        settings.request_user_agent = "nyaterm-test/1.0".to_string();

        let headers = ai_request_headers(&settings).unwrap();

        assert_eq!(
            headers
                .get(USER_AGENT)
                .and_then(|value| value.to_str().ok()),
            Some("nyaterm-test/1.0")
        );
    }

    #[test]
    fn ai_request_headers_fall_back_for_blank_user_agent() {
        let mut settings = AiSettings::default();
        settings.request_user_agent = "   ".to_string();

        let headers = ai_request_headers(&settings).unwrap();

        assert_eq!(
            headers
                .get(USER_AGENT)
                .and_then(|value| value.to_str().ok()),
            Some(AI_REQUEST_USER_AGENT_DEFAULT)
        );
    }

    #[test]
    fn ai_request_headers_reject_invalid_user_agent() {
        let mut settings = AiSettings::default();
        settings.request_user_agent = "bad\r\nvalue".to_string();

        let error = ai_request_headers(&settings).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Invalid AI User-Agent header value")
        );
    }

    #[test]
    fn openai_compatible_empty_key_uses_no_auth_override() {
        let target = apply_service_target_overrides(
            test_service_target(AuthData::from_env("OPENAI_API_KEY")),
            None,
            Some("https://api.example.com/v1/".to_string()),
            true,
        );

        assert!(matches!(target.auth, AuthData::None));
    }

    #[test]
    fn openai_compatible_non_empty_key_uses_configured_key() {
        let target = apply_service_target_overrides(
            test_service_target(AuthData::from_env("OPENAI_API_KEY")),
            Some("configured-key".to_string()),
            Some("https://api.example.com/v1/".to_string()),
            true,
        );

        assert!(matches!(target.auth, AuthData::Key(ref value) if value == "configured-key"));
    }

    #[test]
    fn custom_base_url_overrides_default_endpoint() {
        let target = apply_service_target_overrides(
            test_service_target(AuthData::from_env("ANTHROPIC_API_KEY")),
            Some("configured-key".to_string()),
            Some("https://anthropic-proxy.example.com/".to_string()),
            false,
        );

        assert!(format!("{:?}", target.endpoint).contains("https://anthropic-proxy.example.com/"));
    }

    #[test]
    fn custom_anthropic_and_gemini_use_native_adapters() {
        assert_eq!(
            adapter_kind(&AiProviderKind::Anthropic),
            AdapterKind::Anthropic
        );
        assert_eq!(adapter_kind(&AiProviderKind::Gemini), AdapterKind::Gemini);
    }

    #[test]
    fn ollama_uses_native_adapter_and_preserves_model_tag() {
        assert_eq!(adapter_kind(&AiProviderKind::Ollama), AdapterKind::Ollama);
        assert_eq!(
            genai_model_name(&AiProviderKind::Ollama, "qwen2.5:7b-instruct"),
            "qwen2.5:7b-instruct"
        );
    }

    #[test]
    fn anthropic_and_gemini_empty_keys_fail_validation() {
        for kind in [AiProviderKind::Anthropic, AiProviderKind::Gemini] {
            let credential = test_credential(kind.clone(), None);
            let error = validate_model_credential(&kind, Some(&credential)).unwrap_err();

            assert!(
                error
                    .to_string()
                    .contains("No API key configured for AI credential")
            );
        }
    }

    #[test]
    fn openai_compatible_empty_key_still_passes_validation() {
        let credential = test_credential(AiProviderKind::OpenaiCompatible, None);

        validate_model_credential(&AiProviderKind::OpenaiCompatible, Some(&credential)).unwrap();
    }

    #[test]
    fn openai_empty_key_still_fails_validation() {
        let credential = test_credential(AiProviderKind::Openai, None);
        let error =
            validate_model_credential(&AiProviderKind::Openai, Some(&credential)).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("No API key configured for AI credential")
        );
    }

    #[test]
    fn model_discovery_credentials_include_enabled_providers() {
        let mut settings = AiSettings::default();
        settings.provider_credentials = vec![
            test_credential_with_id("openai", AiProviderKind::OpenaiCompatible, None),
            test_credential_with_id("credential-openai", AiProviderKind::OpenaiCompatible, None),
            test_credential_with_id(
                "credential-anthropic",
                AiProviderKind::Anthropic,
                Some("key"),
            ),
            test_credential_with_id("credential-gemini", AiProviderKind::Gemini, Some("key")),
            AiProviderCredential {
                enabled: false,
                ..test_credential_with_id(
                    "credential-disabled",
                    AiProviderKind::OpenaiCompatible,
                    None,
                )
            },
        ];

        let ids: Vec<_> = model_discovery_credentials(&settings)
            .into_iter()
            .map(|credential| credential.id.as_str())
            .collect();

        assert_eq!(
            ids,
            vec![
                "openai",
                "credential-openai",
                "credential-anthropic",
                "credential-gemini"
            ]
        );
    }

    #[test]
    fn model_resolution_prefers_explicit_credential_id_for_same_protocol_credentials() {
        let mut settings = AiSettings::default();
        settings.provider_credentials = vec![
            test_credential_with_id("credential-a", AiProviderKind::Anthropic, Some("key-a")),
            test_credential_with_id("credential-b", AiProviderKind::Anthropic, Some("key-b")),
        ];
        settings.models = vec![AiModelConfigItem {
            id: "credential-b:claude-test".to_string(),
            name: "claude-test".to_string(),
            backend: AiBackendKind::Genai,
            provider_kind: Some(AiProviderKind::Anthropic),
            credential_id: Some("credential-b".to_string()),
            enabled: true,
            source: AiModelSource::Manual,
            last_seen_at: None,
            supported_reasoning_efforts: None,
        }];
        settings.default_model_id = Some("credential-b:claude-test".to_string());

        let resolved =
            resolve_request_model(&settings, &test_request("credential-b:claude-test")).unwrap();

        let credential = resolved.credential.expect("credential should resolve");
        assert_eq!(credential.id, "credential-b");
        assert_eq!(credential.api_key.as_deref(), Some("key-b"));
        assert_eq!(resolved.provider_kind, AiProviderKind::Anthropic);
    }

    #[test]
    fn default_reasoning_effort_auto_is_not_sent_to_genai() {
        let settings = AiSettings::default();
        let options = build_chat_options(&settings);

        assert!(options.reasoning_effort.is_none());
    }

    #[test]
    fn explicit_reasoning_effort_maps_to_genai_options() {
        let cases = [
            (AiReasoningEffort::None, "none"),
            (AiReasoningEffort::Low, "low"),
            (AiReasoningEffort::Medium, "medium"),
            (AiReasoningEffort::High, "high"),
            (AiReasoningEffort::XHigh, "xhigh"),
        ];

        for (effort, expected) in cases {
            let mut settings = AiSettings::default();
            settings.default_reasoning_effort = effort;

            let options = build_chat_options(&settings);

            assert_eq!(
                options
                    .reasoning_effort
                    .as_ref()
                    .map(ReasoningEffort::variant_name),
                Some(expected)
            );
        }
    }

    #[test]
    fn deepseek_none_reasoning_suffix_is_not_passed_to_genai() {
        assert_eq!(
            genai_model_name(&AiProviderKind::Deepseek, "deepseek-v4-flash-none"),
            "deepseek-v4-flash"
        );
        assert_eq!(
            genai_model_name(&AiProviderKind::Deepseek, "deepseek-v4-pro-none"),
            "deepseek-v4-pro"
        );
    }

    #[test]
    fn deepseek_supported_reasoning_suffix_stays_available_for_genai() {
        assert_eq!(
            genai_model_name(&AiProviderKind::Deepseek, "deepseek-v4-flash-max"),
            "deepseek-v4-flash-max"
        );
    }

    #[test]
    fn non_deepseek_none_suffix_is_preserved() {
        assert_eq!(
            genai_model_name(&AiProviderKind::Openai, "gpt-test-none"),
            "gpt-test-none"
        );
    }
}

pub async fn test_model_connection(settings: &AiSettings, model_id: &str) -> AppResult<()> {
    // The chat panel's selected effort may belong to a different model.
    // Connectivity checks use the target model's own default reasoning behavior.
    let mut settings = settings.clone();
    settings.default_reasoning_effort = AiReasoningEffort::Auto;
    let selected_model = settings
        .models
        .iter()
        .find(|model| model.id == model_id)
        .ok_or_else(|| AppError::Config("AI model is no longer configured".to_string()))?;
    let resolved_model = resolve_model_config(&settings, selected_model)?;
    let timeout = Duration::from_millis(settings.timeout_ms.clamp(1_000, 60_000));

    tokio::time::timeout(timeout, async {
        if super::responses::uses_responses_api(&resolved_model) {
            return super::responses::test_responses_model(
                &settings,
                &resolved_model,
                MODEL_TEST_SYSTEM_PROMPT,
                MODEL_TEST_USER_PROMPT,
                MODEL_TEST_MAX_OUTPUT_TOKENS,
            )
            .await;
        }

        let client = build_client(&resolved_model, &settings)?;
        let request = ChatRequest::new(vec![
            ChatMessage::system(MODEL_TEST_SYSTEM_PROMPT),
            ChatMessage::user(MODEL_TEST_USER_PROMPT),
        ]);
        let options = build_chat_options(&settings).with_max_tokens(MODEL_TEST_MAX_OUTPUT_TOKENS);
        client
            .exec_chat(&resolved_model.model_name, request, Some(&options))
            .await
            .map_err(|error| AppError::Config(format!("AI model test failed: {error}")))?;
        Ok(())
    })
    .await
    .map_err(|_| AppError::Config("AI model test timed out".to_string()))?
}
