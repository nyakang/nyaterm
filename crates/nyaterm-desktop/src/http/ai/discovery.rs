use super::client::{AiProviderSession, headers, http_client};
use super::error::{http_error, status_error};
use nyaterm_core::ai::completion::{AiProviderError, AiProviderErrorKind as Kind};
use nyaterm_core::ai::provider_settings::{
    ProviderModelCursor, effective_protocol, is_builtin_provider, parse_provider_model_page,
};
use nyaterm_core::ai::{
    AiBackendKind, AiChatRequest, AiModelDiscovery, AiProviderApiProtocol, AiProviderCredential,
    AiReasoningEffort, AiSettings,
};
use tokio_util::sync::CancellationToken;

pub async fn discover_provider_models(
    settings: &AiSettings,
    credential: &AiProviderCredential,
    cancel: &CancellationToken,
) -> Result<Vec<AiModelDiscovery>, AiProviderError> {
    tokio::select! { biased; _ = cancel.cancelled() => Err(AiProviderError::new(Kind::Cancelled)), result = discover(settings, credential) => result }
}
async fn discover(
    settings: &AiSettings,
    credential: &AiProviderCredential,
) -> Result<Vec<AiModelDiscovery>, AiProviderError> {
    let base = credential
        .base_url
        .as_deref()
        .filter(|url| !url.trim().is_empty())
        .ok_or_else(|| AiProviderError::new(Kind::InvalidRequest))?;
    let protocol = effective_protocol(credential);
    let path = if protocol == AiProviderApiProtocol::Ollama {
        "api/tags"
    } else {
        "models"
    };
    let url = nyaterm_core::ai::ollama::ollama_url(base, path)
        .map_err(|_| AiProviderError::new(Kind::InvalidRequest))?;
    let mut snapshot = settings.clone();
    snapshot.timeout_ms = 15_000;
    let account = credential.clone();
    let client = tokio::task::spawn_blocking(move || http_client(&snapshot, &account))
        .await
        .map_err(|_| AiProviderError::new(Kind::Connection))??;
    let mut names = Vec::new();
    let mut cursor: Option<ProviderModelCursor> = None;
    let mut seen = std::collections::HashSet::new();
    for _ in 0..100 {
        let mut request = client.get(&url).headers(headers(credential, protocol)?);
        match protocol {
            AiProviderApiProtocol::Anthropic => request = request.query(&[("limit", "1000")]),
            AiProviderApiProtocol::Gemini => request = request.query(&[("pageSize", "1000")]),
            _ => {}
        }
        if let Some(cursor) = &cursor {
            request = request.query(&[cursor.query()]);
        }
        let response = request.send().await.map_err(http_error)?;
        if !response.status().is_success() {
            return Err(status_error(
                response.status().as_u16(),
                response.headers(),
                None,
            ));
        }
        let raw = response.text().await.map_err(http_error)?;
        let page = parse_provider_model_page(protocol, &raw)
            .map_err(|_| AiProviderError::new(Kind::Protocol))?;
        names.extend(page.names);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
        if !seen.insert(cursor.clone()) {
            return Err(AiProviderError::new(Kind::Protocol));
        }
    }
    if cursor.is_some() {
        return Err(AiProviderError::new(Kind::Protocol));
    }
    names.sort();
    names.dedup();
    Ok(names
        .into_iter()
        .map(|name| AiModelDiscovery {
            id: if is_builtin_provider(&credential.id) {
                nyaterm_core::ai_model_id_for_provider(&credential.provider_kind, &name)
            } else {
                nyaterm_core::ai_model_id_for_credential(&credential.id, &name)
            },
            name,
            source: nyaterm_core::AiModelSource::RustGenai,
            provider_kind: Some(credential.provider_kind.clone()),
            credential_id: (!is_builtin_provider(&credential.id)).then(|| credential.id.clone()),
        })
        .collect())
}

pub async fn test_model_connection(
    settings: &AiSettings,
    model_id: &str,
    cancel: &CancellationToken,
) -> Result<(), AiProviderError> {
    let mut settings = settings.clone();
    let model = settings
        .models
        .iter_mut()
        .find(|model| model.id == model_id)
        .ok_or_else(|| AiProviderError::new(Kind::InvalidModel))?;
    if model.backend != AiBackendKind::Genai {
        return Err(AiProviderError::new(Kind::UnsupportedCapability));
    }
    model.enabled = true;
    settings.default_reasoning_effort = AiReasoningEffort::Auto;
    settings.timeout_ms = settings.timeout_ms.clamp(1_000, 60_000);
    let mut request: AiChatRequest = serde_json::from_value(serde_json::json!({"action":"explain_output","mode":"ask","agentKind":"nyaterm","modelId":model_id,"userInput":"Reply with OK.","context":{},"options":{}})).map_err(|_| AiProviderError::new(Kind::InvalidRequest))?;
    request.options.connectivity_test = true;
    super::adapter::complete_chat(
        &AiProviderSession::new(settings, &request)?,
        &request,
        &[],
        cancel,
    )
    .await
    .map(|_| ())
}
