use std::error::Error as _;
use std::time::{Duration, SystemTime};

use nyaterm_core::ai::completion::{
    AiProviderDiagnosticCode as Code, AiProviderError, AiProviderErrorKind as Kind,
    AiProviderParameter as Parameter,
};

pub(super) fn http_error(error: reqwest::Error) -> AiProviderError {
    if error.is_timeout() {
        return AiProviderError::new(Kind::Timeout);
    }
    let mut source = error.source();
    while let Some(cause) = source {
        if cause.is::<rustls::Error>() {
            return AiProviderError::new(Kind::Tls);
        }
        source = cause.source();
    }
    AiProviderError::new(if error.is_connect() {
        Kind::Connection
    } else {
        Kind::Protocol
    })
}

pub(super) fn status_error(
    status: u16,
    headers: &reqwest::header::HeaderMap,
    body: Option<&serde_json::Value>,
) -> AiProviderError {
    let code = body
        .and_then(|body| {
            body.pointer("/error/code")
                .or_else(|| body.pointer("/error/type"))
        })
        .and_then(serde_json::Value::as_str);
    let kind = match (status, code) {
        (_, Some("invalid_api_key" | "invalid_authentication")) => Kind::InvalidApiKey,
        (_, Some("model_not_found" | "invalid_model")) => Kind::InvalidModel,
        (401 | 403, _) => Kind::Authentication,
        (404, _) => Kind::InvalidModel,
        (408 | 504, _) => Kind::Timeout,
        (429, _) => Kind::RateLimit,
        (500..=599, _) => Kind::Server,
        _ => Kind::InvalidRequest,
    };
    let mut error = AiProviderError::new(kind);
    error.status = Some(status);
    error.diagnostic_code = match code {
        Some("invalid_request_error" | "invalid_request") => Some(Code::InvalidRequest),
        Some("unsupported_parameter" | "unsupported_value") => Some(Code::UnsupportedParameter),
        Some("unsupported_tools" | "tool_use_not_supported" | "unsupported_tool_choice") => {
            Some(Code::UnsupportedTools)
        }
        Some("context_length_exceeded" | "request_too_large") => Some(Code::ContextLength),
        Some("invalid_thinking_signature" | "invalid_signature") => Some(Code::InvalidSignature),
        Some("invalid_tool_results" | "tool_call_mismatch") => Some(Code::InvalidToolResults),
        _ => None,
    };
    error.parameter = match body
        .and_then(|body| body.pointer("/error/param"))
        .and_then(serde_json::Value::as_str)
    {
        Some("tools" | "strict") => Some(Parameter::Tools),
        Some("tool_choice") => Some(Parameter::ToolChoice),
        Some("parallel_tool_calls") => Some(Parameter::ParallelToolCalls),
        Some("reasoning" | "reasoning_effort" | "thinking") => Some(Parameter::Reasoning),
        Some("temperature") => Some(Parameter::Temperature),
        Some("max_tokens" | "max_completion_tokens" | "max_output_tokens") => {
            Some(Parameter::MaxTokens)
        }
        Some("messages") => Some(Parameter::Messages),
        Some("input") => Some(Parameter::Input),
        _ => None,
    };
    error.request_id = ["x-request-id", "request-id"].iter().find_map(|name| {
        headers
            .get(*name)
            .and_then(|value| value.to_str().ok())
            .filter(|id| {
                id.len() <= 128
                    && (id.starts_with("req_") || uuid::Uuid::parse_str(id).is_ok())
                    && id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            })
            .map(str::to_string)
    });
    error.retry_after = headers
        .get("retry-after-ms")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .or_else(|| {
            headers
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| {
                    value
                        .parse::<u64>()
                        .ok()
                        .map(Duration::from_secs)
                        .or_else(|| {
                            httpdate::parse_http_date(value).ok().map(|date| {
                                date.duration_since(SystemTime::now()).unwrap_or_default()
                            })
                        })
                })
        });
    // An explicit machine code is required. Never infer retry safety from prose.
    if matches!(status, 400 | 404 | 422)
        && matches!(
            code,
            Some("unsupported_tools" | "tool_use_not_supported" | "unsupported_tool_choice")
        )
    {
        error.kind = Kind::UnsupportedCapability;
        error.tool_protocol_unsupported = true;
        error.retryable = false;
    }
    error
}

pub(super) fn sdk_error(error: genai::Error) -> AiProviderError {
    if let Some(status) = error.status() {
        let body = match &error {
            genai::Error::HttpError { body, .. } => serde_json::from_str(body).ok(),
            genai::Error::WebModelCall {
                webc_error: genai::webc::Error::ResponseFailedStatus { body, .. },
                ..
            } => serde_json::from_str(body).ok(),
            _ => None,
        };
        return status_error(
            status.as_u16(),
            error
                .headers()
                .unwrap_or(&reqwest::header::HeaderMap::new()),
            body.as_ref(),
        );
    }
    match error {
        genai::Error::WebModelCall {
            webc_error: genai::webc::Error::Reqwest(error),
            ..
        } => http_error(error),
        genai::Error::WebStream { error, .. } => {
            if error.is::<genai::webc::StreamTimeout>() {
                return AiProviderError::new(Kind::Timeout);
            }
            match error.downcast::<reqwest::Error>() {
                Ok(error) => http_error(*error),
                Err(error) => match error.downcast::<genai::Error>() {
                    Ok(error) => sdk_error(*error),
                    Err(_) => AiProviderError::new(Kind::StreamInterrupted),
                },
            }
        }
        error => {
            let mut result = AiProviderError::new(Kind::Protocol);
            result.diagnostic_code = Some(match error {
                genai::Error::StreamParse { .. } | genai::Error::SerdeJson(_) => {
                    Code::SdkStreamParse
                }
                genai::Error::ChatResponseGeneration { .. } => Code::SdkResponseGeneration,
                genai::Error::NoChatResponse { .. }
                | genai::Error::InvalidJsonResponseElement { .. }
                | genai::Error::JsonValueExt(_) => Code::SdkResponseShape,
                genai::Error::ChatResponse { .. } => Code::ProviderStreamError,
                _ => Code::SdkOther,
            });
            result
        }
    }
}

pub(super) fn at_stage(
    mut error: AiProviderError,
    stage: nyaterm_core::ai::completion::AiProviderFailureStage,
) -> AiProviderError {
    error.failure_stage = Some(stage);
    error
}

pub(super) fn protocol_error(code: Code) -> AiProviderError {
    let mut error = AiProviderError::new(Kind::Protocol);
    error.diagnostic_code = Some(code);
    error
}
