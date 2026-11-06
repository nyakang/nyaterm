use std::error::Error as StdError;
use std::time::{SystemTime, UNIX_EPOCH};

use nyaterm_core::cloud_sync::error::CloudStorageErrorKind;

use nyaterm_core::CloudSyncError;
use sha2::{Digest as ShaDigest, Sha256};
use zed_reqwest::header::WWW_AUTHENTICATE;

pub(super) fn safe_http_error(provider: &str, error: zed_reqwest::Error) -> CloudSyncError {
    // Do not format reqwest errors: their URLs may contain tokens or signatures.
    if let Some(status) = error.status() {
        http_failure(provider, "request", status.as_u16(), None)
    } else {
        CloudSyncError::Storage {
            provider: provider.into(),
            operation: "request".into(),
            kind: http_error_kind(&error),
        }
    }
}

pub(super) fn http_error_kind(error: &zed_reqwest::Error) -> CloudStorageErrorKind {
    let mut kind = if error.is_timeout() {
        CloudStorageErrorKind::Timeout
    } else if error.is_connect() {
        CloudStorageErrorKind::Connect
    } else {
        CloudStorageErrorKind::Unexpected
    };
    let mut source = error.source();
    while let Some(current) = source {
        if let Some(io) = current.downcast_ref::<std::io::Error>() {
            match io.kind() {
                std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::UnexpectedEof
                | std::io::ErrorKind::BrokenPipe => kind = CloudStorageErrorKind::ConnectionReset,
                std::io::ErrorKind::TimedOut => kind = CloudStorageErrorKind::Timeout,
                _ => {}
            }
        }
        // Source text is inspected only for classification, never retained or
        // emitted. reqwest's top-level Display would include credential URLs.
        let detail = current.to_string().to_ascii_lowercase();
        if detail.contains("connection closed")
            || detail.contains("unexpected eof")
            || detail.contains("incomplete message")
            || detail.contains("connection reset")
            || detail.contains("connection aborted")
        {
            kind = CloudStorageErrorKind::ConnectionReset;
        }
        if detail.contains("certificate") || detail.contains("tls") {
            kind = CloudStorageErrorKind::Tls;
            break;
        }
        if detail.contains("dns")
            || detail.contains("failed to lookup")
            || detail.contains("name resolution")
        {
            kind = CloudStorageErrorKind::Dns;
        }
        source = current.source();
    }
    kind
}

pub(super) fn http_failure(
    provider: &str,
    operation: &str,
    status: impl Into<u16>,
    retry_after_seconds: Option<u64>,
) -> CloudSyncError {
    CloudSyncError::Http {
        provider: provider.into(),
        operation: operation.into(),
        status: status.into(),
        retry_after_seconds,
    }
}

pub(super) fn retry_after_seconds(headers: &zed_reqwest::header::HeaderMap) -> Option<u64> {
    if let Some(value) = headers
        .get(zed_reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
    {
        if let Ok(seconds) = value.parse() {
            return Some(seconds);
        }
        if let Ok(date) = httpdate::parse_http_date(value) {
            return Some(
                date.duration_since(SystemTime::now())
                    .unwrap_or_default()
                    .as_secs(),
            );
        }
    }
    // GitHub reports primary rate limits as HTTP 403 with reset headers.
    if headers
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        == Some("0")
    {
        let reset: u64 = headers
            .get("x-ratelimit-reset")?
            .to_str()
            .ok()?
            .parse()
            .ok()?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        return Some(reset.saturating_sub(now));
    }
    None
}

pub(super) fn map_s3_http_error(error: zed_reqwest::Error) -> CloudSyncError {
    safe_http_error("S3", error)
}
pub(super) fn map_webdav_http_error(error: zed_reqwest::Error) -> CloudSyncError {
    safe_http_error("WebDAV", error)
}

pub(super) fn normalize_endpoint(endpoint: &str) -> String {
    endpoint.trim().trim_end_matches('/').to_string()
}

pub(super) fn trim_remote_path(path: &str) -> String {
    path.trim().trim_matches('/').to_string()
}

pub(super) fn trim_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn trim_optional_secret(value: Option<&str>) -> String {
    value.map(str::trim).unwrap_or_default().to_string()
}

pub(super) fn percent_encode_path(path: &str) -> String {
    path.split('/')
        .map(percent_encode_uri_component)
        .collect::<Vec<_>>()
        .join("/")
}

pub(super) fn percent_encode_uri_component(value: &str) -> String {
    let mut output = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                output.push(*byte as char);
            }
            other => output.push_str(&format!("%{other:02X}")),
        }
    }
    output
}

pub(super) fn form_urlencoded(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(name, value)| {
            format!(
                "{}={}",
                percent_encode_form(name),
                percent_encode_form(value)
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

pub(super) fn percent_encode_form(value: &str) -> String {
    let mut output = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                output.push(*byte as char);
            }
            b' ' => output.push('+'),
            other => output.push_str(&format!("%{other:02X}")),
        }
    }
    output
}

pub(super) fn request_nonce() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:x}-{nanos:x}", std::process::id())
}

pub(super) fn digest_challenge(response: &zed_reqwest::blocking::Response) -> Option<String> {
    digest_challenge_headers(response.headers())
}

pub(super) fn digest_challenge_headers(headers: &zed_reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get_all(WWW_AUTHENTICATE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| {
            value
                .split_once("Digest")
                .map(|(_, challenge)| challenge.trim().to_string())
        })
        .filter(|value| !value.is_empty())
}

pub(super) fn build_digest_authorization(
    challenge: &str,
    username: &str,
    password: &str,
    method: &str,
    uri: &str,
    cnonce: &str,
    nc: &str,
) -> Result<String, CloudSyncError> {
    let params = parse_digest_challenge(challenge);
    let realm = required_digest_param(&params, "realm")?;
    let nonce = required_digest_param(&params, "nonce")?;
    let qop = choose_digest_qop(params.get("qop").map(String::as_str))?;
    let algorithm = params
        .get("algorithm")
        .map_or("MD5", String::as_str)
        .trim()
        .to_ascii_uppercase();

    let ha1 = digest_hash(&algorithm, &format!("{username}:{realm}:{password}"))?;
    let ha2 = digest_hash(&algorithm, &format!("{method}:{uri}"))?;
    let response = digest_hash(
        &algorithm,
        &format!("{ha1}:{nonce}:{nc}:{cnonce}:{qop}:{ha2}"),
    )?;
    let opaque = params
        .get("opaque")
        .map(|value| format!(", opaque=\"{}\"", escape_digest_value(value)))
        .unwrap_or_default();

    Ok(format!(
        "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", algorithm={}, response=\"{}\", qop={}, nc={}, cnonce=\"{}\"{}",
        escape_digest_value(username),
        escape_digest_value(realm),
        escape_digest_value(nonce),
        escape_digest_value(uri),
        algorithm,
        response,
        qop,
        nc,
        escape_digest_value(cnonce),
        opaque
    ))
}

pub(super) fn parse_digest_challenge(
    challenge: &str,
) -> std::collections::BTreeMap<String, String> {
    let mut values = std::collections::BTreeMap::new();
    let mut rest = challenge.trim();
    while !rest.is_empty() {
        rest = rest.trim_start_matches(|ch: char| ch == ',' || ch.is_whitespace());
        let Some((key, after_key)) = rest.split_once('=') else {
            break;
        };
        let key = key.trim().to_ascii_lowercase();
        let after_key = after_key.trim_start();
        let (value, next) = if let Some(quoted) = after_key.strip_prefix('"') {
            parse_quoted_digest_value(quoted)
        } else {
            let split_at = after_key.find(',').unwrap_or(after_key.len());
            (
                after_key[..split_at].trim().to_string(),
                after_key[split_at..].trim_start_matches(','),
            )
        };
        if !key.is_empty() {
            values.insert(key, value);
        }
        rest = next;
    }
    values
}

pub(super) fn parse_quoted_digest_value(input: &str) -> (String, &str) {
    let mut value = String::new();
    let mut escaped = false;
    for (index, ch) in input.char_indices() {
        if escaped {
            value.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return (value, &input[index + ch.len_utf8()..]),
            _ => value.push(ch),
        }
    }
    (value, "")
}

pub(super) fn required_digest_param<'a>(
    params: &'a std::collections::BTreeMap<String, String>,
    key: &str,
) -> Result<&'a str, CloudSyncError> {
    params
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CloudSyncError::Remote(format!(
                "WebDAV Digest authentication challenge is missing {key}"
            ))
        })
}

pub(super) fn choose_digest_qop(qop: Option<&str>) -> Result<&'static str, CloudSyncError> {
    let Some(qop) = qop else {
        return Err(CloudSyncError::Remote(
            "WebDAV Digest authentication without qop=auth is not supported".to_string(),
        ));
    };
    if qop
        .split(',')
        .map(|value| value.trim().trim_matches('"').to_ascii_lowercase())
        .any(|value| value == "auth")
    {
        Ok("auth")
    } else {
        Err(CloudSyncError::Remote(
            "WebDAV Digest authentication requires qop=auth".to_string(),
        ))
    }
}

pub(super) fn digest_hash(algorithm: &str, value: &str) -> Result<String, CloudSyncError> {
    match algorithm {
        "MD5" => Ok(format!("{:x}", md5::compute(value.as_bytes()))),
        "SHA-256" | "SHA256" => {
            let mut hasher = Sha256::new();
            hasher.update(value.as_bytes());
            Ok(hex::encode(hasher.finalize()))
        }
        _ => Err(CloudSyncError::Remote(
            "WebDAV Digest algorithm is not supported".into(),
        )),
    }
}

pub(super) fn escape_digest_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(super) fn webdav_cnonce() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:x}{:x}", std::process::id(), now)
}

pub(super) fn path_and_query(url: &str) -> &str {
    let Some(after_scheme) = url.split_once("://").map(|(_, rest)| rest) else {
        return "/";
    };
    after_scheme
        .find('/')
        .map(|index| &after_scheme[index..])
        .unwrap_or("/")
}

pub(super) fn json_string_field(
    value: &serde_json::Value,
    field: &str,
    operation: &str,
) -> Result<String, CloudSyncError> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| CloudSyncError::Remote(format!("{operation} response is missing {field}")))
}

pub(super) fn safe_json<T: serde::de::DeserializeOwned>(
    body: &str,
    provider: &str,
) -> Result<T, CloudSyncError> {
    serde_json::from_str(body).map_err(|_| http_failure(provider, "invalid response", 0_u16, None))
}

/// Persistence is the boundary: failed saves must never authorize another request.
pub(super) fn install_refreshed_tokens(
    access: &std::sync::Mutex<nyaterm_core::SecretString>,
    refresh: &std::sync::Mutex<nyaterm_core::SecretString>,
    sink: &super::CloudTokenSink,
    update: nyaterm_core::cloud_sync::credentials::CloudCredentialUpdate,
) -> Result<(), CloudSyncError> {
    sink(update.clone())?;
    *access.lock().expect("access token lock") = update.access_token;
    if let Some(token) = update.refresh_token {
        *refresh.lock().expect("refresh token lock") = token;
    }
    Ok(())
}

/// OAuth uses HTTP 400 for expired/revoked refresh credentials. Classify only
/// recognized codes, without carrying a provider's description into diagnostics.
pub(super) fn oauth_failure(
    provider: &str,
    status: u16,
    body: &str,
    retry: Option<u64>,
) -> CloudSyncError {
    let value = serde_json::from_str::<serde_json::Value>(body).ok();
    let code = value
        .as_ref()
        .and_then(|v| v.get("error").or_else(|| v.get("code")))
        .and_then(|v| v.as_str());
    if matches!(status, 400 | 401)
        && matches!(
            code,
            Some(
                "invalid_grant"
                    | "invalid_client"
                    | "unauthorized_client"
                    | "invalid_token"
                    | "InvalidRefreshToken"
                    | "RefreshTokenExpired"
            )
        )
    {
        CloudSyncError::Authentication {
            provider: provider.into(),
            operation: "token refresh".into(),
            status,
        }
    } else {
        http_failure(provider, "token refresh", status, retry)
    }
}
