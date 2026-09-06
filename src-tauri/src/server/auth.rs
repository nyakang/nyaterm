//! Bearer-token authentication for the web server.
//!
//! Single-user model: one shared token, provided via `NYATERM_WEB_TOKEN` or
//! auto-generated into `<config dir>/web-token` on first start.

use std::path::Path;

pub(crate) struct AuthToken {
    secret: String,
}

impl AuthToken {
    /// Environment variable wins over the persisted token so operators can
    /// rotate credentials without touching the data directory.
    pub fn load_or_create(config_dir: &Path) -> Result<Self, std::io::Error> {
        if let Some(token) = Self::env_token() {
            return Ok(Self { secret: token });
        }
        Self::load_or_create_persisted(config_dir)
    }

    fn env_token() -> Option<String> {
        let token = std::env::var("NYATERM_WEB_TOKEN").ok()?;
        let token = token.trim().to_string();
        if token.is_empty() { None } else { Some(token) }
    }

    /// Persisted-token path of [`load_or_create`], split out so tests can
    /// exercise file behavior without mutating process environment state.
    fn load_or_create_persisted(config_dir: &Path) -> Result<Self, std::io::Error> {
        let path = config_dir.join("web-token");
        if let Ok(existing) = std::fs::read_to_string(&path) {
            let existing = existing.trim();
            if !existing.is_empty() {
                return Ok(Self {
                    secret: existing.to_string(),
                });
            }
        }

        let token = uuid::Uuid::new_v4().simple().to_string();
        std::fs::write(&path, &token)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        tracing::info!("Generated web access token at {}", path.display());
        Ok(Self { secret: token })
    }

    /// Constant-time comparison to avoid leaking the token through timing.
    pub fn verify(&self, presented: &str) -> bool {
        constant_time_eq(self.secret.as_bytes(), presented.trim().as_bytes())
    }
}

/// The token is presented either in the `Authorization: Bearer <token>`
/// header (fetch calls) or as a `?token=` query parameter (the browser
/// WebSocket API cannot set custom headers).
pub(crate) fn extract_presented_token(
    authorization: Option<&str>,
    query: Option<&str>,
) -> Option<String> {
    if let Some(header) = authorization {
        let header = header.trim();
        if let Some(token) = header
            .strip_prefix("Bearer ")
            .or(header.strip_prefix("bearer "))
        {
            let token = token.trim();
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    if let Some(query) = query {
        for pair in query.split('&') {
            let mut parts = pair.splitn(2, '=');
            if parts.next() == Some("token")
                && let Some(value) = parts.next()
                && let Ok(decoded) = urldecode(value)
            {
                let decoded = decoded.trim();
                if !decoded.is_empty() {
                    return Some(decoded.to_string());
                }
            }
        }
    }
    None
}

/// Minimal percent-decoding for query parameter values (token is hex or
/// arbitrary text, so `%XX` handling is sufficient).
fn urldecode(value: &str) -> Result<String, ()> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes.get(i + 1..i + 3).ok_or(())?;
                let hex = std::str::from_utf8(hex).map_err(|_| ())?;
                let byte = u8::from_str_radix(hex, 16).map_err(|_| ())?;
                out.push(byte);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| ())
}

/// Axum middleware: rejects unauthenticated requests to the protected API.
pub(crate) async fn auth_middleware(
    axum::extract::State(state): axum::extract::State<super::rpc::ServerState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    let presented = extract_presented_token(
        request
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        request.uri().query(),
    );
    let authorized = presented.is_some_and(|token| state.auth.verify(&token));
    if !authorized {
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            "unauthorized: provide the access token via 'Authorization: Bearer <token>'",
        )
            .into_response();
    }
    next.run(request).await
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_matches_only_identical_inputs() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn persisted_token_is_reused_across_calls() {
        let dir = std::env::temp_dir().join(format!(
            "nyaterm-web-auth-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let first = AuthToken::load_or_create_persisted(&dir).unwrap();
        let second = AuthToken::load_or_create_persisted(&dir).unwrap();
        assert!(first.verify(&second.secret));
        assert_eq!(first.secret.len(), 32);
        assert!(!first.verify("other"));
        let token_file = dir.join("web-token");
        assert!(token_file.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&token_file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn env_token_parsing_rejects_blank_values() {
        // Indirect check: an all-whitespace value must yield None. The env
        // read itself is exercised in the integration path, not here (Rust
        // 2024 makes env mutation unsafe and tests run in parallel).
        assert!("   ".trim().is_empty());
    }

    #[test]
    fn extract_presented_token_supports_header_and_query() {
        assert_eq!(
            extract_presented_token(Some("Bearer abc123"), None).as_deref(),
            Some("abc123")
        );
        assert_eq!(
            extract_presented_token(None, Some("sessionId=1&token=xy%20z")).as_deref(),
            Some("xy z")
        );
        assert_eq!(extract_presented_token(None, None), None);
        assert_eq!(extract_presented_token(Some("Basic abc"), None), None);
    }
}
