use std::error::Error as _;
use std::time::{Duration, SystemTime};

use nyaterm_core::ai::completion::{AiProviderError, AiProviderErrorKind as Kind};

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
        _ => AiProviderError::new(Kind::Protocol),
    }
}
