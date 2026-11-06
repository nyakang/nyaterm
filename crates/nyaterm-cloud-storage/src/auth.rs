use super::metrics::{CountedRequest, CountedResponse};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use futures::channel::mpsc::UnboundedSender;

use serde::Deserialize;
use zed_reqwest::StatusCode;
use zed_reqwest::blocking::{Client, Response};

pub enum GithubGistAuthEvent {
    Started {
        user_code: String,
        verification_uri: String,
    },
    Polling {
        slow_down: bool,
    },
    Succeeded {
        access_token: nyaterm_core::SecretString,
        gist_id: String,
        login: String,
    },
    Failed(String),
    Cancelled,
}

#[derive(Debug)]
pub struct GithubGistAuthJobEvent {
    pub job_id: u64,
    pub event: GithubGistAuthEvent,
}

impl std::fmt::Debug for GithubGistAuthEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Started {
                user_code: _,
                verification_uri,
            } => formatter
                .debug_struct("Started")
                .field("user_code", &"<redacted>")
                .field("verification_uri", verification_uri)
                .finish(),
            Self::Polling { slow_down } => formatter
                .debug_struct("Polling")
                .field("slow_down", slow_down)
                .finish(),
            Self::Succeeded {
                access_token: _,
                gist_id,
                login,
            } => formatter
                .debug_struct("Succeeded")
                .field("access_token", &"<redacted>")
                .field("gist_id", gist_id)
                .field("login", login)
                .finish(),
            Self::Failed(_) => formatter.write_str("Failed(<redacted>)"),
            Self::Cancelled => formatter.write_str("Cancelled"),
        }
    }
}

const GITHUB_GIST_CLIENT_ID: Option<&str> = option_env!("NYATERM_GITHUB_GIST_CLIENT_ID");
const GITHUB_DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const GITHUB_ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const GITHUB_API_ENDPOINT: &str = "https://api.github.com";
const GITHUB_API_VERSION: &str = "2022-11-28";
const GITHUB_GIST_SCOPE: &str = "gist";

#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: Option<u64>,
}

#[derive(Deserialize)]
struct AccessTokenResponse {
    #[serde(default)]
    access_token: Option<nyaterm_core::SecretString>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    #[serde(rename = "error_description")]
    _error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubUserResponse {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GithubGistResponse {
    id: String,
}

pub fn run_github_gist_device_flow(
    job_id: u64,
    existing_gist_id: Option<String>,
    cancel: Arc<AtomicBool>,
    tx: UnboundedSender<GithubGistAuthJobEvent>,
) {
    let result = run_device_flow(job_id, existing_gist_id, &cancel, &tx);
    if cancel.load(Ordering::Relaxed) {
        send_event(&tx, job_id, GithubGistAuthEvent::Cancelled);
    } else if let Err(error) = result {
        send_event(&tx, job_id, GithubGistAuthEvent::Failed(error));
    }
}

fn run_device_flow(
    job_id: u64,
    existing_gist_id: Option<String>,
    cancel: &AtomicBool,
    tx: &UnboundedSender<GithubGistAuthJobEvent>,
) -> Result<(), String> {
    let client_id = github_client_id()?;
    let client = github_client()?;
    let device: DeviceCodeResponse = decode_json(
        client
            .post(GITHUB_DEVICE_CODE_URL)
            .header("Accept", "application/json")
            .form(&[("client_id", client_id), ("scope", GITHUB_GIST_SCOPE)])
            .send_counted()
            .map_err(map_http_error)?,
        "GitHub device authorization",
    )?;
    let mut interval = device.interval.unwrap_or(5).max(1);
    let deadline = Instant::now() + Duration::from_secs(device.expires_in.max(1));
    tx.unbounded_send(GithubGistAuthJobEvent {
        job_id,
        event: GithubGistAuthEvent::Started {
            user_code: device.user_code,
            verification_uri: device.verification_uri,
        },
    })
    .map_err(|_| "GitHub authorization UI is no longer available".to_string())?;

    loop {
        if wait_cancelled(Duration::from_secs(interval), cancel) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("GitHub device authorization expired".to_string());
        }

        let payload: AccessTokenResponse = decode_json(
            client
                .post(GITHUB_ACCESS_TOKEN_URL)
                .header("Accept", "application/json")
                .form(&[
                    ("client_id", client_id),
                    ("device_code", device.device_code.as_str()),
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ])
                .send_counted()
                .map_err(map_http_error)?,
            "GitHub access token",
        )?;

        match payload.error.as_deref() {
            Some("authorization_pending") => {
                send_event(
                    tx,
                    job_id,
                    GithubGistAuthEvent::Polling { slow_down: false },
                );
                continue;
            }
            Some("slow_down") => {
                interval = interval.saturating_add(5);
                send_event(tx, job_id, GithubGistAuthEvent::Polling { slow_down: true });
                continue;
            }
            Some("expired_token") => return Err("GitHub device authorization expired".into()),
            Some("access_denied") => return Err("GitHub device authorization was denied".into()),
            Some(_) => return Err("GitHub authorization failed".into()),
            None => {}
        }

        let access_token = payload
            .access_token
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "GitHub OAuth response did not include a token".to_string())?;
        let scope = payload.scope.unwrap_or_default();
        if !has_gist_scope(&scope) {
            return Err("GitHub authorization did not grant the gist scope".to_string());
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }

        let login = fetch_github_login(&client, access_token.expose_secret())?;
        let gist_id =
            resolve_github_gist_id(&client, access_token.expose_secret(), existing_gist_id)?;
        tx.unbounded_send(GithubGistAuthJobEvent {
            job_id,
            event: GithubGistAuthEvent::Succeeded {
                access_token,
                gist_id,
                login,
            },
        })
        .map_err(|_| "GitHub authorization UI is no longer available".to_string())?;
        return Ok(());
    }
}

fn send_event(
    tx: &UnboundedSender<GithubGistAuthJobEvent>,
    job_id: u64,
    event: GithubGistAuthEvent,
) {
    let _ = tx.unbounded_send(GithubGistAuthJobEvent { job_id, event });
}

fn fetch_github_login(client: &Client, access_token: &str) -> Result<String, String> {
    let payload: GithubUserResponse = decode_json(
        github_api_request(
            client.get(format!("{GITHUB_API_ENDPOINT}/user")),
            access_token,
        )
        .send_counted()
        .map_err(map_http_error)?,
        "GitHub user",
    )?;
    Ok(payload.login)
}

fn resolve_github_gist_id(
    client: &Client,
    access_token: &str,
    existing_gist_id: Option<String>,
) -> Result<String, String> {
    if let Some(gist_id) = existing_gist_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let response = github_api_request(
            client.get(format!("{GITHUB_API_ENDPOINT}/gists/{gist_id}")),
            access_token,
        )
        .send_counted()
        .map_err(map_http_error)?;
        if response.status().is_success() {
            return Ok(gist_id.to_string());
        }
        if response.status() != StatusCode::NOT_FOUND {
            return Err(response_error(response, "GitHub Gist lookup"));
        }
    }

    let payload: GithubGistResponse = decode_json(
        github_api_request(
            client.post(format!("{GITHUB_API_ENDPOINT}/gists")),
            access_token,
        )
        .json(&serde_json::json!({
            "description": "NyaTerm encrypted cloud sync storage",
            "public": false,
            "files": {
                "nyaterm-readme.txt": {
                    "content": "This private gist stores encrypted NyaTerm cloud sync objects."
                }
            }
        }))
        .send_counted()
        .map_err(map_http_error)?,
        "GitHub Gist creation",
    )?;
    Ok(payload.id)
}

fn github_api_request(
    request: zed_reqwest::blocking::RequestBuilder,
    access_token: &str,
) -> zed_reqwest::blocking::RequestBuilder {
    request
        .bearer_auth(access_token)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
}

fn decode_json<T: serde::de::DeserializeOwned>(
    response: Response,
    operation: &str,
) -> Result<T, String> {
    let status = response.status();
    let body = response.text_counted().map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(format!("{operation} failed ({status})"));
    }
    serde_json::from_str(&body).map_err(|_| format!("{operation} response is invalid"))
}

fn response_error(response: Response, operation: &str) -> String {
    let status = response.status();
    let _body = response.text_counted().unwrap_or_default();
    format!("{operation} failed ({status})")
}

fn github_client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("NyaTerm")
        .build()
        .map_err(map_http_error)
}

fn github_client_id() -> Result<&'static str, String> {
    GITHUB_GIST_CLIENT_ID
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "GitHub Gist OAuth Client ID is not configured at build time".to_string())
}

fn map_http_error(error: zed_reqwest::Error) -> String {
    if error.is_timeout() {
        "GitHub device flow operation timed out".to_string()
    } else {
        "GitHub device flow request failed".to_string()
    }
}

fn wait_cancelled(duration: Duration, cancel: &AtomicBool) -> bool {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        if cancel.load(Ordering::Relaxed) {
            return true;
        }
        std::thread::sleep(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(100)),
        );
    }
    cancel.load(Ordering::Relaxed)
}

fn has_gist_scope(scope: &str) -> bool {
    scope
        .split([',', ' '])
        .map(str::trim)
        .any(|value| value == GITHUB_GIST_SCOPE)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};

    use super::{GithubGistAuthEvent, GithubGistAuthJobEvent, has_gist_scope, wait_cancelled};

    #[test]
    fn gist_scope_parser_accepts_space_or_comma_separated_values() {
        assert!(has_gist_scope("read:user gist"));
        assert!(has_gist_scope("read:user,gist"));
        assert!(!has_gist_scope("read:user repo"));
    }

    #[test]
    fn cancelled_wait_returns_promptly() {
        let cancel = AtomicBool::new(true);
        let started = Instant::now();
        assert!(wait_cancelled(Duration::from_secs(30), &cancel));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn authorization_events_redact_tokens_user_codes_and_failure_details() {
        let secret = "nya-desktop-secret-never-log";
        let events = [
            GithubGistAuthEvent::Started {
                user_code: secret.to_owned(),
                verification_uri: "https://github.com/login/device".to_owned(),
            },
            GithubGistAuthEvent::Succeeded {
                access_token: secret.to_owned().into(),
                gist_id: "gist".to_owned(),
                login: "user".to_owned(),
            },
            GithubGistAuthEvent::Failed(secret.to_owned()),
        ];
        for event in events {
            let event = GithubGistAuthJobEvent { job_id: 1, event };
            let output = format!("{event:?}");
            assert!(!output.contains(secret));
            assert!(output.contains("redacted"));
        }
    }
}
