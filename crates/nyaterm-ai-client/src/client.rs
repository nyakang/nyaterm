use std::sync::Arc;
use std::time::Duration;

use genai::{
    Client, ModelIden, ServiceTarget,
    adapter::AdapterKind,
    resolver::{AuthData, Endpoint},
};
use nyaterm_core::ai::{
    AiChatRequest, AiProviderApiProtocol, AiProviderCredential, AiSettings, ResolvedAiModel,
    completion::{AiProviderError, AiProviderErrorKind},
    provider_settings::effective_protocol,
};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio::sync::OnceCell;

use super::error::http_error;

/// A run owns this immutable snapshot, including its account and HTTP client.
/// Editing settings cannot mutate an existing run or one of its continuations.
pub struct AiProviderSession {
    pub settings: AiSettings,
    pub(super) model: ResolvedAiModel,
    pub(super) protocol: AiProviderApiProtocol,
    pub(super) responses: bool,
    pub(super) ollama_compat: bool,
    client: OnceCell<ProviderClient>,
}

pub(super) struct ProviderClient {
    pub sdk: Client,
    pub http: reqwest::Client,
}

impl AiProviderSession {
    pub fn new(
        settings: AiSettings,
        request: &AiChatRequest,
    ) -> Result<Arc<Self>, AiProviderError> {
        nyaterm_core::validate_ai_request_contract(request, &settings)
            .map_err(|_| AiProviderError::new(AiProviderErrorKind::InvalidRequest))?;
        let model = nyaterm_core::resolve_request_model(&settings, request).map_err(|error| {
            AiProviderError::new(match error {
                nyaterm_core::AiModelError::MissingApiKey { .. } => {
                    AiProviderErrorKind::Authentication
                }
                _ => AiProviderErrorKind::InvalidModel,
            })
        })?;
        let credential = model
            .credential
            .as_ref()
            .ok_or_else(|| AiProviderError::new(AiProviderErrorKind::Authentication))?;
        let protocol = effective_protocol(credential);
        let responses = nyaterm_core::uses_responses_api(&model);
        let ollama_compat = protocol == AiProviderApiProtocol::Ollama
            && (model.model_name.contains("::") || request.options.generation.extra_body.is_some());
        Ok(Arc::new(Self {
            settings,
            model,
            protocol,
            responses,
            ollama_compat,
            client: OnceCell::new(),
        }))
    }

    pub fn credential_id(&self) -> &str {
        &self
            .model
            .credential
            .as_ref()
            .expect("validated credential")
            .id
    }

    pub(super) async fn client(&self) -> Result<&ProviderClient, AiProviderError> {
        self.client
            .get_or_try_init(|| async {
                let settings = self.settings.clone();
                let credential = self.model.credential.clone().expect("validated credential");
                // Trust-store/proxy discovery can do filesystem/OS work.
                tokio::task::spawn_blocking(move || {
                    let http = http_client(&settings, &credential)?;
                    let sdk = Client::builder()
                        .with_reqwest(http.clone())
                        .build()
                        .map_err(super::error::sdk_error)?;
                    Ok(ProviderClient { sdk, http })
                })
                .await
                .map_err(|_| AiProviderError::new(AiProviderErrorKind::Connection))?
            })
            .await
    }

    pub(super) fn target(&self, stream: bool) -> Result<ServiceTarget, AiProviderError> {
        let credential = self
            .model
            .credential
            .as_ref()
            .expect("validated credential");
        let base = credential
            .base_url
            .as_deref()
            .filter(|base| !base.trim().is_empty())
            .unwrap_or_else(|| {
                nyaterm_core::ai::provider_settings::provider_default_url(&credential.provider_kind)
            });
        let result = if self.responses {
            nyaterm_core::openai_responses_url(&self.model)
        } else {
            match self.protocol {
                AiProviderApiProtocol::Anthropic => nyaterm_core::anthropic_messages_url(base),
                AiProviderApiProtocol::Gemini if stream => {
                    nyaterm_core::gemini_stream_generate_content_url(base, &self.model.model_name)
                }
                AiProviderApiProtocol::Gemini => {
                    nyaterm_core::gemini_generate_content_url(base, &self.model.model_name)
                }
                AiProviderApiProtocol::Ollama => {
                    nyaterm_core::ai::ollama::ollama_url(base, "api/chat")
                }
                AiProviderApiProtocol::OpenaiCompatible => {
                    nyaterm_core::openai_compatible_chat_completions_url(base)
                }
            }
        };
        let url = result.map_err(|_| AiProviderError::new(AiProviderErrorKind::InvalidRequest))?;
        let kind = if self.responses {
            AdapterKind::OpenAIResp
        } else {
            match self.protocol {
                AiProviderApiProtocol::Anthropic => AdapterKind::Anthropic,
                AiProviderApiProtocol::Gemini => AdapterKind::Gemini,
                AiProviderApiProtocol::Ollama => AdapterKind::Ollama,
                AiProviderApiProtocol::OpenaiCompatible => AdapterKind::OpenAI,
            }
        };
        let headers = headers(credential, self.protocol)?;
        // SDK Headers is case-sensitive. Build one canonical map and replace the
        // SDK's complete auth map, so a custom `authorization` truly overrides it.
        Ok(ServiceTarget {
            // The wire model is restored by request::convert (or Gemini's exact
            // URL above). A neutral suffix
            // prevents SDK convenience parsing from turning Auto into reasoning.
            model: ModelIden::new(kind, {
                let name = nyaterm_core::genai_model_name(
                    &self.model.provider_kind,
                    &self.model.model_name,
                );
                if matches!(
                    self.protocol,
                    AiProviderApiProtocol::OpenaiCompatible
                        | AiProviderApiProtocol::Anthropic
                        | AiProviderApiProtocol::Gemini
                ) && genai::chat::ReasoningEffort::from_model_name(&name)
                    .0
                    .is_some()
                {
                    format!("{name}-nyaterm-literal")
                } else {
                    name
                }
            }),
            endpoint: Endpoint::from_owned(format!("{}/", base.trim_end_matches('/'))),
            auth: AuthData::RequestOverride {
                url,
                headers: headers
                    .iter()
                    .map(|(name, value)| {
                        Ok((
                            name.to_string(),
                            value
                                .to_str()
                                .map_err(|_| {
                                    AiProviderError::new(AiProviderErrorKind::InvalidRequest)
                                })?
                                .to_string(),
                        ))
                    })
                    .collect::<Result<Vec<_>, AiProviderError>>()?
                    .into(),
            },
        })
    }
}

pub(super) fn headers(
    credential: &AiProviderCredential,
    protocol: AiProviderApiProtocol,
) -> Result<HeaderMap, AiProviderError> {
    let invalid = || AiProviderError::new(AiProviderErrorKind::InvalidRequest);
    let mut map = HeaderMap::new();
    if protocol == AiProviderApiProtocol::Anthropic {
        map.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    }
    if let Some(key) = credential
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
    {
        let (name, value) = match protocol {
            AiProviderApiProtocol::Anthropic => ("x-api-key", key.to_string()),
            AiProviderApiProtocol::Gemini => ("x-goog-api-key", key.to_string()),
            _ => ("authorization", format!("Bearer {key}")),
        };
        let mut value = HeaderValue::from_str(&value).map_err(|_| invalid())?;
        value.set_sensitive(true);
        map.insert(name, value);
    }
    for (name, value) in nyaterm_core::custom_header_pairs(credential).map_err(|_| invalid())? {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| invalid())?;
        let mut value = HeaderValue::from_str(&value).map_err(|_| invalid())?;
        value.set_sensitive(true);
        map.insert(name, value);
    }
    Ok(map)
}

pub(super) fn http_client(
    settings: &AiSettings,
    credential: &AiProviderCredential,
) -> Result<reqwest::Client, AiProviderError> {
    let has_custom = !headers(credential, effective_protocol(credential))?.is_empty();
    let mut builder = super::async_proxy::apply_proxy(reqwest::Client::builder(), &settings.proxy)
        .map_err(|_| AiProviderError::new(AiProviderErrorKind::InvalidRequest))?
        .timeout(Duration::from_millis(settings.timeout_ms))
        .user_agent(nyaterm_core::effective_ai_request_user_agent(settings))
        .retry(reqwest::retry::never());
    if has_custom {
        builder = builder.redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 {
                attempt.error("AI redirect limit reached")
            } else if attempt
                .previous()
                .first()
                .is_some_and(|original| original.origin() != attempt.url().origin())
            {
                attempt.error("AI credentials cannot follow cross-origin redirects")
            } else {
                attempt.follow()
            }
        }));
    }
    builder.build().map_err(http_error)
}
