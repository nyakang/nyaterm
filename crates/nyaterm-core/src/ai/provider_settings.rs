//! Provider identity, protocol selection and model capabilities, without UI or I/O.

use super::{
    AiApiFormat, AiBackendKind, AiModelConfigItem, AiModelReasoningEffort, AiProviderApiProtocol,
    AiProviderCredential, AiProviderKind, AiReasoningEffort, MAX_AI_CUSTOM_HEADER_NAME_LEN,
    MAX_AI_CUSTOM_HEADER_VALUE_LEN, MAX_AI_CUSTOM_HEADERS,
};
use std::collections::HashMap;

/// Resolves a provider's display order and stably prioritizes matching model names.
/// Only the returned view is reordered; the provider order and configuration stay intact.
pub fn prioritized_provider_models<'a>(
    models: &'a [AiModelConfigItem],
    model_order: &[String],
    query: &str,
) -> Vec<&'a AiModelConfigItem> {
    let models_by_id: HashMap<_, _> = models.iter().map(|model| (&model.id, model)).collect();
    let mut ordered: Vec<_> = model_order
        .iter()
        .filter_map(|id| models_by_id.get(id).copied())
        .collect();
    let query = query.trim().to_lowercase();
    if !query.is_empty() {
        ordered.sort_by_cached_key(|model| {
            let name = model.name.trim().to_lowercase();
            if name == query {
                0
            } else if name.starts_with(&query) {
                1
            } else if name.contains(&query) {
                2
            } else {
                3
            }
        });
    }
    ordered
}

pub const MODEL_REASONING_EFFORTS: [AiModelReasoningEffort; 8] = [
    AiModelReasoningEffort::None,
    AiModelReasoningEffort::Minimal,
    AiModelReasoningEffort::Low,
    AiModelReasoningEffort::Medium,
    AiModelReasoningEffort::High,
    AiModelReasoningEffort::XHigh,
    AiModelReasoningEffort::Max,
    AiModelReasoningEffort::Ultra,
];
pub const DEFAULT_MODEL_REASONING_EFFORTS: [AiModelReasoningEffort; 5] = [
    AiModelReasoningEffort::None,
    AiModelReasoningEffort::Low,
    AiModelReasoningEffort::Medium,
    AiModelReasoningEffort::High,
    AiModelReasoningEffort::XHigh,
];

pub fn effective_protocol(credential: &AiProviderCredential) -> AiProviderApiProtocol {
    credential
        .api_protocol
        .unwrap_or(match credential.provider_kind {
            AiProviderKind::Anthropic => AiProviderApiProtocol::Anthropic,
            AiProviderKind::Gemini => AiProviderApiProtocol::Gemini,
            AiProviderKind::Ollama
                if !credential
                    .base_url
                    .as_deref()
                    .unwrap_or_default()
                    .trim_end_matches('/')
                    .ends_with("/v1") =>
            {
                AiProviderApiProtocol::Ollama
            }
            _ => AiProviderApiProtocol::OpenaiCompatible,
        })
}

pub fn provider_requires_api_key(credential: &AiProviderCredential) -> bool {
    // Gateways may authenticate exclusively through a custom business header.
    if custom_header_pairs(credential).is_ok_and(|headers| !headers.is_empty()) {
        return false;
    }
    match effective_protocol(credential) {
        AiProviderApiProtocol::Ollama => false,
        AiProviderApiProtocol::OpenaiCompatible => !matches!(
            credential.provider_kind,
            AiProviderKind::Ollama | AiProviderKind::OpenaiCompatible
        ),
        AiProviderApiProtocol::Anthropic | AiProviderApiProtocol::Gemini => true,
    }
}

pub fn is_builtin_provider(id: &str) -> bool {
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

pub fn provider_label(kind: &AiProviderKind) -> &'static str {
    match kind {
        AiProviderKind::Openai => "OpenAI",
        AiProviderKind::Anthropic => "Anthropic",
        AiProviderKind::Gemini => "Gemini",
        AiProviderKind::Deepseek => "DeepSeek",
        AiProviderKind::Groq => "Groq",
        AiProviderKind::Ollama => "Ollama",
        AiProviderKind::Xai => "xAI",
        AiProviderKind::Cohere => "Cohere",
        AiProviderKind::Mimo => "MiMo",
        AiProviderKind::Zai => "Z.AI",
        AiProviderKind::OpenaiCompatible => "my-provider",
    }
}

pub fn provider_default_url(kind: &AiProviderKind) -> &'static str {
    match kind {
        AiProviderKind::Openai => "https://api.openai.com/v1/",
        AiProviderKind::Anthropic => "https://api.anthropic.com/v1/",
        AiProviderKind::Gemini => "https://generativelanguage.googleapis.com/v1beta/",
        AiProviderKind::Deepseek => "https://api.deepseek.com/v1/",
        AiProviderKind::Groq => "https://api.groq.com/openai/v1/",
        AiProviderKind::Ollama => "http://localhost:11434/",
        AiProviderKind::Xai => "https://api.x.ai/v1/",
        AiProviderKind::Cohere => "https://api.cohere.com/compatibility/v1/",
        AiProviderKind::Mimo => "https://api.xiaomimimo.com/v1/",
        AiProviderKind::Zai => "https://open.bigmodel.cn/api/paas/v4/",
        AiProviderKind::OpenaiCompatible => "",
    }
}

pub fn model_belongs_to_provider(
    model: &AiModelConfigItem,
    credential: &AiProviderCredential,
) -> bool {
    if model.backend != AiBackendKind::Genai {
        return false;
    }
    if let Some(id) = &model.credential_id {
        return id == &credential.id;
    }
    // Legacy implicit IDs must never be captured by a second account of the same brand.
    is_builtin_provider(&credential.id)
        && model
            .id
            .split_once(':')
            .is_some_and(|(id, _)| id == credential.id)
}

pub fn provider_name_taken(
    name: &str,
    credentials: &[AiProviderCredential],
    exclude: &str,
) -> bool {
    let name = name.trim().to_lowercase();
    !name.is_empty()
        && credentials.iter().any(|credential| {
            credential.enabled
                && credential.id != exclude
                && credential.name.trim().to_lowercase() == name
        })
}

pub fn available_provider_name(
    base: &str,
    credentials: &[AiProviderCredential],
    exclude: &str,
) -> String {
    let mut name = base.to_string();
    let mut suffix = 2;
    while provider_name_taken(&name, credentials, exclude) {
        name = format!("{base}-{suffix}");
        suffix += 1;
    }
    name
}

pub fn protocol_value(credential: &AiProviderCredential) -> &'static str {
    match effective_protocol(credential) {
        AiProviderApiProtocol::OpenaiCompatible
            if credential.api_format == AiApiFormat::Responses =>
        {
            "responses"
        }
        AiProviderApiProtocol::OpenaiCompatible => "chat_completions",
        AiProviderApiProtocol::Anthropic => "anthropic",
        AiProviderApiProtocol::Gemini => "gemini",
        AiProviderApiProtocol::Ollama => "ollama",
    }
}

pub fn set_provider_protocol(credential: &mut AiProviderCredential, value: &str) {
    let previous = effective_protocol(credential);
    let protocol = match value {
        "anthropic" => AiProviderApiProtocol::Anthropic,
        "gemini" => AiProviderApiProtocol::Gemini,
        "ollama" => AiProviderApiProtocol::Ollama,
        _ => AiProviderApiProtocol::OpenaiCompatible,
    };
    if credential.provider_kind == AiProviderKind::Ollama {
        let url = credential
            .base_url
            .as_deref()
            .unwrap_or_default()
            .trim_end_matches('/');
        if previous == AiProviderApiProtocol::Ollama
            && protocol == AiProviderApiProtocol::OpenaiCompatible
            && url == "http://localhost:11434"
        {
            credential.base_url = Some("http://localhost:11434/v1/".into());
        } else if previous == AiProviderApiProtocol::OpenaiCompatible
            && protocol == AiProviderApiProtocol::Ollama
            && url == "http://localhost:11434/v1"
        {
            credential.base_url = Some("http://localhost:11434/".into());
        }
    }
    credential.api_protocol = Some(protocol);
    credential.api_format = if value == "responses" {
        AiApiFormat::Responses
    } else {
        AiApiFormat::ChatCompletions
    };
}

pub fn model_reasoning_options(model: Option<&AiModelConfigItem>) -> Vec<AiReasoningEffort> {
    let supported = model
        .and_then(|model| model.supported_reasoning_efforts.as_deref())
        .unwrap_or(&DEFAULT_MODEL_REASONING_EFFORTS);
    let mut options = vec![AiReasoningEffort::Auto];
    options.extend(
        MODEL_REASONING_EFFORTS
            .iter()
            .filter(|effort| supported.contains(effort))
            .map(|effort| match effort {
                AiModelReasoningEffort::None => AiReasoningEffort::None,
                AiModelReasoningEffort::Minimal => AiReasoningEffort::Minimal,
                AiModelReasoningEffort::Low => AiReasoningEffort::Low,
                AiModelReasoningEffort::Medium => AiReasoningEffort::Medium,
                AiModelReasoningEffort::High => AiReasoningEffort::High,
                AiModelReasoningEffort::XHigh => AiReasoningEffort::XHigh,
                AiModelReasoningEffort::Max => AiReasoningEffort::Max,
                AiModelReasoningEffort::Ultra => AiReasoningEffort::Ultra,
            }),
    );
    options
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProviderModelCursor {
    Anthropic(String),
    Gemini(String),
}
impl ProviderModelCursor {
    pub fn query(&self) -> (&str, &str) {
        match self {
            Self::Anthropic(value) => ("after_id", value),
            Self::Gemini(value) => ("pageToken", value),
        }
    }
}

pub struct ProviderModelPage {
    pub names: Vec<String>,
    pub next_cursor: Option<ProviderModelCursor>,
}

pub fn parse_provider_model_page(
    protocol: AiProviderApiProtocol,
    raw: &str,
) -> Result<ProviderModelPage, super::AiModelError> {
    use super::AiModelError;
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|_| AiModelError::InvalidModelsJson("Invalid provider model response".into()))?;
    let field = if matches!(
        protocol,
        AiProviderApiProtocol::Gemini | AiProviderApiProtocol::Ollama
    ) {
        "models"
    } else {
        "data"
    };
    let entries = value[field]
        .as_array()
        .ok_or_else(|| AiModelError::InvalidModelsJson("Invalid provider model list".into()))?;
    let names = entries
        .iter()
        .filter_map(|entry| {
            let name = entry[if field == "models" { "name" } else { "id" }]
                .as_str()?
                .trim();
            let name = if protocol == AiProviderApiProtocol::Gemini {
                name.strip_prefix("models/").unwrap_or(name)
            } else {
                name
            };
            (!name.is_empty()).then(|| name.to_owned())
        })
        .collect();
    let next_cursor = match protocol {
        AiProviderApiProtocol::Gemini => value["nextPageToken"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(|value| ProviderModelCursor::Gemini(value.into())),
        AiProviderApiProtocol::Anthropic if value["has_more"] == true => {
            Some(ProviderModelCursor::Anthropic(
                value["last_id"]
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        AiModelError::InvalidModelsJson(
                            "Provider pagination omitted its cursor".into(),
                        )
                    })?
                    .into(),
            ))
        }
        _ => None,
    };
    Ok(ProviderModelPage { names, next_cursor })
}

/// Exact casing of well-known auth headers so a user-configured override
/// replaces the SDK-managed value instead of being sent alongside it.
fn canonical_custom_header_name(name: &str) -> &str {
    if name.eq_ignore_ascii_case("authorization") {
        "Authorization"
    } else if name.eq_ignore_ascii_case("x-api-key") {
        "x-api-key"
    } else if name.eq_ignore_ascii_case("x-goog-api-key") {
        "x-goog-api-key"
    } else if name.eq_ignore_ascii_case("anthropic-version") {
        "anthropic-version"
    } else {
        name
    }
}

/// HTTP token characters (RFC 9110 tchar) for custom header names.
pub fn is_valid_custom_header_name(name: &str) -> bool {
    let name = name.trim_matches([' ', '\t']);
    !name.is_empty()
        && name.len() <= MAX_AI_CUSTOM_HEADER_NAME_LEN
        && name.bytes().all(|byte| {
            matches!(
                byte,
                b'!' | b'#'
                    | b'$'
                    | b'%'
                    | b'&'
                    | b'\''
                    | b'*'
                    | b'+'
                    | b'-'
                    | b'.'
                    | b'^'
                    | b'_'
                    | b'`'
                    | b'|'
                    | b'~'
                    | b'0'..=b'9'
                    | b'a'..=b'z'
                    | b'A'..=b'Z'
            )
        })
}

pub fn is_valid_custom_header_value(value: &str) -> bool {
    let value = value.trim_matches([' ', '\t']);
    !value.is_empty()
        && value.len() <= MAX_AI_CUSTOM_HEADER_VALUE_LEN
        && value
            .bytes()
            .all(|byte| byte == b'\t' || (0x20..=0x7e).contains(&byte))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CustomHeaderError {
    #[error("Custom header name must be an HTTP token of at most 256 bytes")]
    InvalidName,
    #[error("This header is controlled by the HTTP client and cannot be customized")]
    ManagedName,
    #[error("Custom header value is required")]
    EmptyValue,
    #[error("Custom header value exceeds the size limit")]
    ValueTooLong,
    #[error("Custom header value must contain only printable ASCII characters or tabs")]
    InvalidValue,
    #[error("Custom header names must be unique (case insensitive)")]
    DuplicateName,
    #[error("Too many custom headers")]
    TooMany,
}

pub fn validate_custom_header(name: &str, value: &str) -> Result<(), CustomHeaderError> {
    if !is_valid_custom_header_name(name) {
        return Err(CustomHeaderError::InvalidName);
    }
    // Routing, framing, connection management and proxy authentication belong
    // to the client. Application auth (Authorization, x-api-key, etc.) may be overridden.
    if matches!(
        name.trim_matches([' ', '\t']).to_ascii_lowercase().as_str(),
        "host"
            | "content-length"
            | "transfer-encoding"
            | "connection"
            | "proxy-authorization"
            | "proxy-authenticate"
            | "proxy-connection"
            | "keep-alive"
            | "te"
            | "trailer"
            | "upgrade"
    ) {
        return Err(CustomHeaderError::ManagedName);
    }
    let value = value.trim_matches([' ', '\t']);
    if value.is_empty() {
        return Err(CustomHeaderError::EmptyValue);
    }
    if value.len() > MAX_AI_CUSTOM_HEADER_VALUE_LEN {
        return Err(CustomHeaderError::ValueTooLong);
    }
    if !is_valid_custom_header_value(value) {
        return Err(CustomHeaderError::InvalidValue);
    }
    Ok(())
}

/// Validate before constructing a request; errors never include secret material.
/// Completely empty editor rows are inert and may survive saves.
pub fn custom_header_pairs(
    credential: &AiProviderCredential,
) -> Result<Vec<(String, String)>, CustomHeaderError> {
    if credential.extra_headers.len() > MAX_AI_CUSTOM_HEADERS {
        return Err(CustomHeaderError::TooMany);
    }
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut names = std::collections::HashSet::new();
    for header in &credential.extra_headers {
        let name = header.name.trim_matches([' ', '\t']);
        let value = header.value.expose_secret().trim_matches([' ', '\t']);
        if name.is_empty() && value.is_empty() {
            continue;
        }
        validate_custom_header(name, value)?;
        let pair = (
            canonical_custom_header_name(name).to_string(),
            value.to_string(),
        );
        if !names.insert(name.to_ascii_lowercase()) {
            return Err(CustomHeaderError::DuplicateName);
        }
        headers.push(pair);
    }
    Ok(headers)
}

/// Repair editor identities without discarding invalid/imported configuration.
/// Validation at save/request boundaries reports rows requiring correction.
pub fn normalize_custom_headers(headers: &mut [super::AiHttpHeader]) {
    let mut ids = std::collections::HashSet::new();
    for (index, header) in headers.iter_mut().enumerate() {
        if header.id.trim().is_empty() || !ids.insert(header.id.clone()) {
            // Stable repairs are needed when loading the same imported document
            // again to merge masked values. Random IDs would lose that match.
            let base = if header.id.trim().is_empty() {
                format!("header-{index}")
            } else {
                header.id.clone()
            };
            for suffix in 0_u64.. {
                header.id = format!("{base}-{suffix}");
                if ids.insert(header.id.clone()) {
                    break;
                }
            }
        }
        header.name = header.name.trim_matches([' ', '\t']).to_string();
    }
}

/// A blank custom header row with a fresh id, for settings editors.
pub fn new_custom_header() -> super::AiHttpHeader {
    super::AiHttpHeader {
        id: super::uuid(),
        name: String::new(),
        value: crate::SecretString::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        available_provider_name, custom_header_pairs, effective_protocol,
        is_valid_custom_header_name, model_belongs_to_provider, model_reasoning_options,
        new_custom_header, normalize_custom_headers, prioritized_provider_models,
        provider_name_taken, set_provider_protocol,
    };
    use crate::ai::{
        AiApiFormat, AiHttpHeader, AiModelConfigItem, AiProviderApiProtocol, AiProviderCredential,
        AiProviderKind, AiReasoningEffort, AiSettings, normalize_ai_settings,
    };
    use serde_json::json;

    fn model(id: &str, name: &str) -> AiModelConfigItem {
        serde_json::from_value(json!({"id": id, "name": name})).unwrap()
    }

    fn ordered_ids(models: &[AiModelConfigItem], order: &[String], query: &str) -> Vec<String> {
        prioritized_provider_models(models, order, query)
            .into_iter()
            .map(|model| model.id.clone())
            .collect()
    }

    #[test]
    fn model_matches_prioritize_exact_prefix_contains_then_nonmatches_stably() {
        let models = vec![
            model("exact", " GPT-4o "),
            model("prefix-a", "gpt-4o-mini"),
            model("prefix-b", "GPT-4O-latest"),
            model("contains-a", "my-gpt-4o"),
            model("contains-b", "other-GPT-4O"),
            model("other-a", "claude"),
            model("other-b", "gemini"),
        ];
        let order: Vec<String> = [
            "other-b",
            "contains-b",
            "prefix-b",
            "other-a",
            "exact",
            "prefix-a",
            "contains-a",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        assert_eq!(
            ordered_ids(&models, &order, "  gPt-4O  "),
            [
                "exact",
                "prefix-b",
                "prefix-a",
                "contains-b",
                "contains-a",
                "other-b",
                "other-a"
            ]
        );
    }

    #[test]
    fn empty_or_unmatched_model_queries_preserve_provider_order() {
        let models = vec![model("a", "gpt-4o"), model("b", "claude")];
        let order = vec!["b".to_string(), "a".to_string()];
        for query in ["", " \t\n ", "unknown"] {
            assert_eq!(ordered_ids(&models, &order, query), order);
        }
    }

    #[test]
    fn clearing_model_query_restores_order_without_mutating_configuration() {
        let models = vec![model("a", "gpt-4o"), model("b", "claude")];
        let original_models = models.clone();
        let order = vec!["b".to_string(), "a".to_string()];
        assert_eq!(ordered_ids(&models, &order, "gpt"), ["a", "b"]);
        assert_eq!(ordered_ids(&models, &order, ""), ["b", "a"]);
        assert_eq!(order, ["b", "a"]);
        assert_eq!(models, original_models);
    }

    #[test]
    fn model_matches_only_include_provider_order_and_skip_stale_ids() {
        let models = vec![
            model("other-provider", "gpt-4o"),
            model("current-provider", "gpt-4o-mini"),
        ];
        let order = vec!["missing".to_string(), "current-provider".to_string()];
        assert_eq!(ordered_ids(&models, &order, "gpt-4o"), ["current-provider"]);
        assert!(prioritized_provider_models(&models, &[], "gpt").is_empty());
    }

    fn credential(id: &str, kind: &str) -> AiProviderCredential {
        serde_json::from_value(
            json!({"id":id,"name":"Account","provider_kind":kind,"enabled":true}),
        )
        .unwrap()
    }

    fn header(id: &str, name: &str, value: &str) -> AiHttpHeader {
        AiHttpHeader {
            id: id.to_string(),
            name: name.to_string(),
            value: value.into(),
        }
    }

    #[test]
    fn custom_header_names_follow_token_syntax() {
        assert!(is_valid_custom_header_name("X-Custom-Token"));
        assert!(is_valid_custom_header_name("authorization"));
        assert!(is_valid_custom_header_name("  X-Foo  "));
        assert!(!is_valid_custom_header_name(""));
        assert!(!is_valid_custom_header_name("   "));
        assert!(!is_valid_custom_header_name("not a header"));
        assert!(!is_valid_custom_header_name("X-Foo:bar"));
        assert!(!is_valid_custom_header_name("X-Foo\r\nbar"));
    }

    #[test]
    fn custom_headers_validate_without_silently_dropping_rows() {
        use super::{CustomHeaderError, validate_custom_header};
        let mut credential = credential("custom", "openai_compatible");
        credential.extra_headers = vec![
            header("h1", "X-Custom-Token", " secret "),
            header("h2", "authorization", "Bearer override"),
            header("h3", "", ""),
        ];
        assert_eq!(
            custom_header_pairs(&credential).unwrap(),
            vec![
                ("X-Custom-Token".into(), "secret".into()),
                ("Authorization".into(), "Bearer override".into()),
            ]
        );
        credential
            .extra_headers
            .push(header("h4", "X-CUSTOM-TOKEN", "newer"));
        assert_eq!(
            custom_header_pairs(&credential),
            Err(CustomHeaderError::DuplicateName)
        );
        for name in [
            "Host",
            "CONTENT-LENGTH",
            "Transfer-Encoding",
            "Connection",
            "Proxy-Authorization",
            "TE",
            "Trailer",
            "Upgrade",
            "Keep-Alive",
            "Proxy-Connection",
        ] {
            assert_eq!(
                validate_custom_header(name, "test"),
                Err(CustomHeaderError::ManagedName)
            );
        }
        for value in ["中文", "test\r\n", "\nsecret", "test\0", "test\u{7f}"] {
            assert_eq!(
                validate_custom_header("X-Token", value),
                Err(CustomHeaderError::InvalidValue)
            );
        }
        assert_eq!(
            validate_custom_header("X-Token", " "),
            Err(CustomHeaderError::EmptyValue)
        );
        assert_eq!(
            validate_custom_header("X-Token", &"a".repeat(8193)),
            Err(CustomHeaderError::ValueTooLong)
        );
        for name in ["Authorization", "x-api-key", "x-goog-api-key", "X-Tenant"] {
            assert!(validate_custom_header(name, "test").is_ok());
        }
        assert!(validate_custom_header("X-Token", "test\tvalue").is_ok());
        assert!(!super::is_valid_custom_header_name("X-Token\r\n"));
    }

    #[test]
    fn header_normalization_repairs_ids_and_preserves_invalid_rows() {
        let mut headers = vec![
            header("keep", "  X-Foo  ", "  one  "),
            header("keep", "x-foo", "two"),
            header("", "", "unfinished"),
        ];
        normalize_custom_headers(&mut headers);
        assert_eq!(headers.len(), 3);
        assert_eq!(headers[0].id, "keep");
        assert_eq!(headers[0].name, "X-Foo");
        assert_ne!(headers[1].id, headers[0].id);
        assert!(!headers[2].id.is_empty());
        assert_eq!(headers[2].value.expose_secret(), "unfinished");
        let original = headers.clone();
        normalize_custom_headers(&mut headers);
        assert_eq!(headers, original);
        let fresh = new_custom_header();
        assert!(!fresh.id.is_empty());
        assert!(fresh.name.is_empty());
        assert!(fresh.value.is_empty());
    }
    #[test]
    fn same_brand_accounts_never_capture_implicit_builtin_models() {
        let model: AiModelConfigItem = serde_json::from_value(
            json!({"id":"openai:gpt-5","name":"gpt-5","provider_kind":"openai"}),
        )
        .unwrap();
        assert!(model_belongs_to_provider(
            &model,
            &credential("openai", "openai")
        ));
        assert!(!model_belongs_to_provider(
            &model,
            &credential("second-account", "openai")
        ));
    }
    #[test]
    fn provider_names_are_case_insensitive_and_generate_unused_suffixes() {
        let mut one = credential("one", "openai");
        one.name = "OpenAI".into();
        let mut two = credential("two", "openai");
        two.name = "OpenAI-2".into();
        let credentials = vec![one, two];
        assert!(provider_name_taken(" openai ", &credentials, "other"));
        assert!(!provider_name_taken("OPENAI", &credentials, "one"));
        assert_eq!(
            available_provider_name("OpenAI", &credentials, "other"),
            "OpenAI-3"
        );
    }
    #[test]
    fn explicit_protocol_overrides_brand_and_survives_normalization() {
        let mut account = credential("ollama", "ollama");
        account.base_url = Some("http://localhost:11434/".into());
        set_provider_protocol(&mut account, "responses");
        assert_eq!(
            account.base_url.as_deref(),
            Some("http://localhost:11434/v1/")
        );
        assert_eq!(account.api_format, AiApiFormat::Responses);
        let mut settings = AiSettings {
            provider_credentials: vec![account],
            ..AiSettings::default()
        };
        normalize_ai_settings(&mut settings);
        assert_eq!(
            settings.provider_credentials[0].base_url.as_deref(),
            Some("http://localhost:11434/v1/")
        );
        set_provider_protocol(&mut settings.provider_credentials[0], "ollama");
        assert_eq!(
            settings.provider_credentials[0].base_url.as_deref(),
            Some("http://localhost:11434/")
        );
        let mut custom = credential("custom", "anthropic");
        custom.base_url = Some("http://proxy.invalid/custom/v1/".into());
        set_provider_protocol(&mut custom, "chat_completions");
        assert_eq!(
            effective_protocol(&custom),
            AiProviderApiProtocol::OpenaiCompatible
        );
        assert_eq!(custom.provider_kind, AiProviderKind::Anthropic);
        assert_eq!(
            custom.base_url.as_deref(),
            Some("http://proxy.invalid/custom/v1/")
        );
    }
    #[test]
    fn old_and_tauri_capabilities_round_trip_and_always_include_auto() {
        let old: AiModelConfigItem =
            serde_json::from_value(json!({"id":"account:m","name":"m","credential_id":"account"}))
                .unwrap();
        assert_eq!(
            model_reasoning_options(Some(&old)),
            vec![
                AiReasoningEffort::Auto,
                AiReasoningEffort::None,
                AiReasoningEffort::Low,
                AiReasoningEffort::Medium,
                AiReasoningEffort::High,
                AiReasoningEffort::XHigh
            ]
        );
        let new: AiModelConfigItem = serde_json::from_value(json!({"id":"account:m","name":"m","supported_reasoning_efforts":["minimal","max","ultra"]})).unwrap();
        assert_eq!(
            model_reasoning_options(Some(&new)),
            vec![
                AiReasoningEffort::Auto,
                AiReasoningEffort::Minimal,
                AiReasoningEffort::Max,
                AiReasoningEffort::Ultra
            ]
        );
        assert_eq!(
            serde_json::from_value::<AiModelConfigItem>(serde_json::to_value(&new).unwrap())
                .unwrap(),
            new
        );
        let empty: AiModelConfigItem = serde_json::from_value(
            json!({"id":"account:m","name":"m","supported_reasoning_efforts":[]}),
        )
        .unwrap();
        assert_eq!(
            model_reasoning_options(Some(&empty)),
            vec![AiReasoningEffort::Auto]
        );
        let account: AiProviderCredential = serde_json::from_value(json!({"id":"custom","name":"Custom","provider_kind":"openai_compatible","icon_data_url":"data:image/png;base64,fixture","api_protocol":"gemini"})).unwrap();
        assert_eq!(effective_protocol(&account), AiProviderApiProtocol::Gemini);
        assert_eq!(
            serde_json::from_value::<AiProviderCredential>(serde_json::to_value(&account).unwrap())
                .unwrap(),
            account
        );
    }
    #[test]
    fn protocol_authentication_requirements_are_independent_of_brand() {
        let mut account = credential("custom", "anthropic");
        set_provider_protocol(&mut account, "ollama");
        assert!(
            crate::ai::validate_model_credential(&account.provider_kind, Some(&account)).is_ok()
        );
        account.provider_kind = AiProviderKind::Ollama;
        set_provider_protocol(&mut account, "gemini");
        assert!(
            crate::ai::validate_model_credential(&account.provider_kind, Some(&account)).is_err()
        );
        account.api_key = Some("fixture-key".into());
        assert!(
            crate::ai::validate_model_credential(&account.provider_kind, Some(&account)).is_ok()
        );
    }
}
