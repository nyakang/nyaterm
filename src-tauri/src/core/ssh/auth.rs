use super::client::{SshAuth, SshConfig, SshHandler, SshPostLoginConfig};
use crate::error::{AppError, AppResult};
use crate::observability::{self, StructuredLog, StructuredLogLevel};
use russh::client::{self, KeyboardInteractiveAuthResponse};
use russh::MethodKind;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{oneshot, Mutex};

/// Manages pending keyboard-interactive auth requests awaiting user input from the frontend.
pub struct PendingAuthManager {
    pending: Mutex<HashMap<String, oneshot::Sender<Option<Vec<String>>>>>,
}

impl PendingAuthManager {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub async fn register(&self, request_id: String) -> oneshot::Receiver<Option<Vec<String>>> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id, tx);
        rx
    }

    pub async fn respond(&self, request_id: &str, responses: Option<Vec<String>>) -> bool {
        if let Some(tx) = self.pending.lock().await.remove(request_id) {
            tx.send(responses).is_ok()
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct OtpPrompt {
    prompt: String,
    echo: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OtpRequestPayload {
    request_id: String,
    connection_name: String,
    prompts: Vec<OtpPrompt>,
    otp_entry_id: Option<String>,
    target_window_label: Option<String>,
}

fn build_ids(connection_id: Option<&str>, request_id: Option<&str>) -> Option<Value> {
    let mut ids = Map::new();
    if let Some(connection_id) = connection_id {
        ids.insert(
            "connection_id".to_string(),
            Value::String(connection_id.to_string()),
        );
    }
    if let Some(request_id) = request_id {
        ids.insert(
            "request_id".to_string(),
            Value::String(request_id.to_string()),
        );
    }
    if ids.is_empty() {
        None
    } else {
        Some(Value::Object(ids))
    }
}

fn log_structured(
    level: StructuredLogLevel,
    domain: &str,
    event: &str,
    message: &str,
    connection_id: Option<&str>,
    request_id: Option<&str>,
    data: Option<Value>,
    error: Option<Value>,
) {
    observability::log_event(StructuredLog {
        level,
        domain: domain.to_string(),
        event: event.to_string(),
        message: message.to_string(),
        ids: build_ids(connection_id, request_id),
        data,
        error,
        client_timestamp: None,
    });
}

pub(crate) fn load_saved_ssh_config(app: &AppHandle, connection_id: &str) -> AppResult<SshConfig> {
    let conn = crate::config::load_connection_by_id(app, connection_id)?;
    resolve_saved_ssh_config(app, &conn, Some(connection_id.to_string()), true)
}

fn resolve_saved_ssh_config(
    app: &AppHandle,
    conn: &crate::config::SavedConnection,
    connection_id: Option<String>,
    include_proxy_jump: bool,
) -> AppResult<SshConfig> {
    let proxy = resolve_proxy(app, conn)?;
    let (host, port, username) = resolve_ssh_target(conn)?;
    let auth = resolve_auth(app, conn)?;
    let proxy_jump = if include_proxy_jump {
        resolve_proxy_jump(app, conn)?
    } else {
        None
    };
    let post_login = resolve_post_login(conn);

    Ok(SshConfig {
        connection_id,
        owner_window_label: None,
        name: conn.name.clone(),
        host,
        port,
        username,
        auth,
        proxy,
        proxy_jump,
        post_login,
    })
}

fn resolve_post_login(conn: &crate::config::SavedConnection) -> Option<SshPostLoginConfig> {
    let post_login = conn.post_login.as_ref()?;
    if !post_login.enabled || post_login.command.trim().is_empty() {
        return None;
    }

    Some(SshPostLoginConfig {
        command: post_login.command.clone(),
        delay_ms: post_login.delay_ms,
    })
}

fn resolve_ssh_target(conn: &crate::config::SavedConnection) -> AppResult<(String, u16, String)> {
    match &conn.config {
        crate::config::ConnectionType::Ssh {
            host,
            port,
            username,
        } => Ok((host.clone(), *port, username.clone())),
        _ => Err(AppError::Auth(
            "Connection is not an SSH connection".to_string(),
        )),
    }
}

fn resolve_auth(app: &AppHandle, conn: &crate::config::SavedConnection) -> AppResult<SshAuth> {
    let Some(conn_auth) = conn.auth.as_ref() else {
        return Ok(SshAuth::None);
    };

    match conn_auth.mode.as_str() {
        "none" => Ok(SshAuth::None),
        "password" => {
            let password = resolve_password_material(Some(app), conn_auth)?;
            Ok(SshAuth::Password { password })
        }
        "key" => {
            let Some(key_id) = conn_auth.key_id.as_deref() else {
                return Ok(SshAuth::None);
            };
            let ssh_key = crate::config::load_key_by_id(app, key_id)?;
            let key_data = crate::config::decrypt_key_pem(&ssh_key)?
                .ok_or_else(|| AppError::Auth("No key data stored".to_string()))?;
            Ok(SshAuth::Key {
                key_data,
                passphrase: ssh_key.passphrase,
            })
        }
        other => Err(AppError::Auth(format!("Unknown auth type: {}", other))),
    }
}

fn resolve_password_material(
    app: Option<&AppHandle>,
    conn_auth: &crate::config::ConnectionAuth,
) -> AppResult<String> {
    if let Some(ref ciphertext) = conn_auth.password {
        return crate::utils::crypto::decrypt(ciphertext)
            .map_err(|e| AppError::Auth(format!("Failed to decrypt inline password: {e}")));
    }

    let Some(pw_id) = conn_auth.password_id.as_deref().filter(|id| !id.is_empty()) else {
        return Err(AppError::Auth(
            "No password for this connection".to_string(),
        ));
    };

    let app = app.ok_or_else(|| AppError::Auth("No password for this connection".to_string()))?;
    let pw_entry = crate::config::load_password_by_id(app, pw_id)?;
    pw_entry
        .password
        .ok_or_else(|| AppError::Auth("No stored password".to_string()))
}

fn resolve_proxy_jump(
    app: &AppHandle,
    conn: &crate::config::SavedConnection,
) -> AppResult<Option<Box<SshConfig>>> {
    let proxy_jump_id = conn
        .network
        .as_ref()
        .and_then(|network| network.proxy_jump_id.as_deref());

    let Some(proxy_jump_id) = proxy_jump_id else {
        return Ok(None);
    };

    let jump_conn = crate::config::load_connection_by_id(app, proxy_jump_id)?;
    if !matches!(jump_conn.config, crate::config::ConnectionType::Ssh { .. }) {
        return Err(AppError::Config(
            "Only SSH connections can be used as jump hosts".to_string(),
        ));
    }
    if jump_conn
        .network
        .as_ref()
        .and_then(|network| network.proxy_jump_id.as_deref())
        .is_some()
    {
        return Err(AppError::Config(
            "Jump hosts cannot use another jump host".to_string(),
        ));
    }

    Ok(Some(Box::new(resolve_saved_ssh_config(
        app,
        &jump_conn,
        Some(proxy_jump_id.to_string()),
        false,
    )?)))
}

fn resolve_proxy(
    app: &AppHandle,
    conn: &crate::config::SavedConnection,
) -> AppResult<Option<crate::config::ProxySettings>> {
    let proxy_id = conn.network.as_ref().and_then(|n| n.proxy_id.as_deref());

    let Some(proxy_id) = proxy_id else {
        return Ok(None);
    };

    let proxy_cfg = crate::config::load_proxy_by_id(app, proxy_id)?
        .ok_or_else(|| AppError::Config(format!("Proxy '{}' not found", proxy_id)))?;
    let password = proxy_cfg
        .password
        .as_ref()
        .and_then(|ciphertext| crate::utils::crypto::decrypt(ciphertext).ok());

    Ok(Some(crate::config::ProxySettings {
        enabled: true,
        protocol: proxy_cfg.protocol,
        host: proxy_cfg.host,
        port: proxy_cfg.port,
        username: proxy_cfg.username,
        password,
    }))
}

pub(super) async fn authenticate_handle(
    handle: &mut client::Handle<SshHandler>,
    config: &SshConfig,
    app: &AppHandle,
    password_error: &str,
    key_error: &str,
) -> AppResult<()> {
    let otp_info = config
        .connection_id
        .as_deref()
        .and_then(|connection_id| resolve_otp_info(app, connection_id));

    match &config.auth {
        SshAuth::None => {
            log_structured(
                StructuredLogLevel::Info,
                "ssh.auth",
                "auth.start",
                "Starting SSH authentication",
                config.connection_id.as_deref(),
                None,
                Some(json!({
                    "host": config.host,
                    "port": config.port,
                    "username": config.username,
                    "auth_mode": "none",
                })),
                None,
            );

            let authenticated = handle
                .authenticate_none(&config.username)
                .await
                .map_err(|error| AppError::Auth(format!("None auth failed: {}", error)))?;

            try_keyboard_interactive_after_partial(
                handle,
                &authenticated,
                &config.username,
                config.connection_id.as_deref(),
                &config.name,
                config.owner_window_label.as_deref(),
                app,
                "Authentication failed: none auth rejected",
                Some(KeyboardInteractiveMode::AdditionalFactor),
                otp_info.as_ref(),
            )
            .await?;
        }
        SshAuth::Password { password } => {
            log_structured(
                StructuredLogLevel::Info,
                "ssh.auth",
                "auth.start",
                "Starting SSH authentication",
                config.connection_id.as_deref(),
                None,
                Some(json!({
                    "host": config.host,
                    "port": config.port,
                    "username": config.username,
                    "auth_mode": "password",
                })),
                None,
            );

            let authenticated = handle
                .authenticate_password(&config.username, password)
                .await
                .map_err(|error| AppError::Auth(format!("Authentication failed: {}", error)))?;

            try_keyboard_interactive_after_partial(
                handle,
                &authenticated,
                &config.username,
                config.connection_id.as_deref(),
                &config.name,
                config.owner_window_label.as_deref(),
                app,
                password_error,
                Some(KeyboardInteractiveMode::PasswordFallback { password }),
                otp_info.as_ref(),
            )
            .await?;
        }
        SshAuth::Key {
            key_data,
            passphrase,
        } => {
            let key = russh::keys::decode_secret_key(key_data, passphrase.as_deref())?;
            let hash_alg = handle
                .best_supported_rsa_hash()
                .await
                .ok()
                .flatten()
                .flatten();

            log_structured(
                StructuredLogLevel::Info,
                "ssh.auth",
                "auth.start",
                "Starting SSH authentication",
                config.connection_id.as_deref(),
                None,
                Some(json!({
                    "host": config.host,
                    "port": config.port,
                    "username": config.username,
                    "auth_mode": "publickey",
                    "key_algorithm": key.algorithm().to_string(),
                    "rsa_hash": format!("{hash_alg:?}"),
                })),
                None,
            );

            let authenticated = handle
                .authenticate_publickey(
                    &config.username,
                    russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), hash_alg),
                )
                .await
                .map_err(|error| AppError::Auth(format!("Key auth failed: {}", error)))?;

            try_keyboard_interactive_after_partial(
                handle,
                &authenticated,
                &config.username,
                config.connection_id.as_deref(),
                &config.name,
                config.owner_window_label.as_deref(),
                app,
                key_error,
                None,
                otp_info.as_ref(),
            )
            .await?;
        }
    }

    log_structured(
        StructuredLogLevel::Info,
        "ssh.auth",
        "auth.succeeded",
        "SSH authentication succeeded",
        config.connection_id.as_deref(),
        None,
        Some(json!({
            "host": config.host,
            "port": config.port,
            "username": config.username,
        })),
        None,
    );

    Ok(())
}

struct OtpAutoFillInfo {
    otp_id: String,
    auto_fill: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TotpUseCandidate {
    otp_id: String,
    code: String,
    time_step: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct UsedTotpCode {
    code: String,
    time_step: u64,
}

struct PreparedOtpResponses {
    responses: Vec<String>,
    used_totp: Option<TotpUseCandidate>,
}

static USED_TOTP_CODES: OnceLock<StdMutex<HashMap<String, UsedTotpCode>>> = OnceLock::new();

fn used_totp_codes() -> &'static StdMutex<HashMap<String, UsedTotpCode>> {
    USED_TOTP_CODES.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn unix_seconds_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_secs()
}

fn seconds_until_next_totp_step(now: u64, period: u64) -> u64 {
    let period = period.max(1);
    let remaining = period - (now % period);
    remaining.max(1)
}

fn is_totp_code_reused(candidate: &TotpUseCandidate) -> bool {
    let used = used_totp_codes()
        .lock()
        .expect("used TOTP code cache poisoned");
    used.get(&candidate.otp_id).is_some_and(|record| {
        record.code == candidate.code && record.time_step == candidate.time_step
    })
}

fn record_totp_code_use(candidate: TotpUseCandidate) {
    let mut used = used_totp_codes()
        .lock()
        .expect("used TOTP code cache poisoned");
    used.insert(
        candidate.otp_id,
        UsedTotpCode {
            code: candidate.code,
            time_step: candidate.time_step,
        },
    );
}

#[derive(Debug, Clone, Copy)]
enum KeyboardInteractiveMode<'a> {
    AdditionalFactor,
    PasswordFallback { password: &'a str },
}

impl<'a> KeyboardInteractiveMode<'a> {
    fn label(self) -> &'static str {
        match self {
            Self::AdditionalFactor => "additional-factor",
            Self::PasswordFallback { .. } => "password-fallback",
        }
    }

    fn password(self) -> Option<&'a str> {
        match self {
            Self::AdditionalFactor => None,
            Self::PasswordFallback { password } => Some(password),
        }
    }
}

fn resolve_otp_info(app: &AppHandle, connection_id: &str) -> Option<OtpAutoFillInfo> {
    let conn = crate::config::load_connection_by_id(app, connection_id).ok()?;
    let auth = conn.auth.as_ref()?;
    let otp_id = auth.otp_id.clone()?;
    Some(OtpAutoFillInfo {
        otp_id,
        auto_fill: auth.auto_fill_otp,
    })
}

async fn wait_for_next_totp_code(
    connection_id: Option<&str>,
    otp_id: &str,
    code: &crate::cmd::otp::TotpCodeResult,
) {
    log_structured(
        StructuredLogLevel::Info,
        "security.flow",
        "otp.reuse_wait",
        "Waiting for next TOTP code before submitting keyboard-interactive response",
        connection_id,
        None,
        Some(json!({
            "otp_entry_id": otp_id,
            "time_step": code.time_step,
            "wait_seconds": code.remaining_seconds,
        })),
        None,
    );
    tokio::time::sleep(std::time::Duration::from_secs(
        seconds_until_next_totp_step(unix_seconds_now(), code.period),
    ))
    .await;
}

async fn generate_keyboard_interactive_otp_responses(
    app: &AppHandle,
    info: &OtpAutoFillInfo,
    prompt_count: usize,
    connection_id: Option<&str>,
) -> AppResult<PreparedOtpResponses> {
    let now = unix_seconds_now();
    let Some(mut code) = crate::cmd::otp::generate_totp_code_for_entry_at(app, &info.otp_id, now)?
    else {
        let result = crate::cmd::otp::generate_otp_for_entry(app, &info.otp_id)?;
        return Ok(PreparedOtpResponses {
            responses: vec![result.code; prompt_count],
            used_totp: None,
        });
    };

    let mut candidate = TotpUseCandidate {
        otp_id: info.otp_id.clone(),
        code: code.code.clone(),
        time_step: code.time_step,
    };

    if is_totp_code_reused(&candidate) {
        wait_for_next_totp_code(connection_id, &info.otp_id, &code).await;
        code = crate::cmd::otp::generate_totp_code_for_entry_at(
            app,
            &info.otp_id,
            unix_seconds_now(),
        )?
        .ok_or_else(|| AppError::Auth("OTP entry is no longer TOTP".to_string()))?;
        candidate = TotpUseCandidate {
            otp_id: info.otp_id.clone(),
            code: code.code.clone(),
            time_step: code.time_step,
        };
    }

    Ok(PreparedOtpResponses {
        responses: vec![code.code; prompt_count],
        used_totp: Some(candidate),
    })
}

async fn prepare_manual_otp_responses(
    app: &AppHandle,
    otp_info: Option<&OtpAutoFillInfo>,
    responses: Vec<String>,
    connection_id: Option<&str>,
) -> AppResult<PreparedOtpResponses> {
    let Some(info) = otp_info else {
        return Ok(PreparedOtpResponses {
            responses,
            used_totp: None,
        });
    };
    if responses.is_empty() {
        return Ok(PreparedOtpResponses {
            responses,
            used_totp: None,
        });
    }

    let Some(mut code) =
        crate::cmd::otp::generate_totp_code_for_entry_at(app, &info.otp_id, unix_seconds_now())?
    else {
        return Ok(PreparedOtpResponses {
            responses,
            used_totp: None,
        });
    };

    let matching_indices: Vec<usize> = responses
        .iter()
        .enumerate()
        .filter_map(|(index, response)| (response == &code.code).then_some(index))
        .collect();
    if matching_indices.is_empty() {
        return Ok(PreparedOtpResponses {
            responses,
            used_totp: None,
        });
    }

    let mut candidate = TotpUseCandidate {
        otp_id: info.otp_id.clone(),
        code: code.code.clone(),
        time_step: code.time_step,
    };
    let mut responses = responses;

    if is_totp_code_reused(&candidate) {
        wait_for_next_totp_code(connection_id, &info.otp_id, &code).await;
        code = crate::cmd::otp::generate_totp_code_for_entry_at(
            app,
            &info.otp_id,
            unix_seconds_now(),
        )?
        .ok_or_else(|| AppError::Auth("OTP entry is no longer TOTP".to_string()))?;
        for index in matching_indices {
            if let Some(response) = responses.get_mut(index) {
                *response = code.code.clone();
            }
        }
        candidate = TotpUseCandidate {
            otp_id: info.otp_id.clone(),
            code: code.code.clone(),
            time_step: code.time_step,
        };
    }

    Ok(PreparedOtpResponses {
        responses,
        used_totp: Some(candidate),
    })
}

/// Runs the keyboard-interactive auth state machine, emitting `otp-request` events
/// to the frontend for each `InfoRequest` that contains prompts, and automatically
/// responding with an empty array for empty `InfoRequest`s.
///
/// When `otp_info` is present with `auto_fill == true`, the OTP code is generated
/// automatically and used as the response without prompting the user.
async fn finish_keyboard_interactive(
    handle: &mut client::Handle<SshHandler>,
    username: &str,
    connection_id: Option<&str>,
    connection_name: &str,
    target_window_label: Option<&str>,
    app: &AppHandle,
    mode: KeyboardInteractiveMode<'_>,
    otp_info: Option<&OtpAutoFillInfo>,
) -> AppResult<()> {
    let pending_mgr = app
        .try_state::<Arc<PendingAuthManager>>()
        .ok_or_else(|| AppError::Auth("PendingAuthManager not available".to_string()))?;
    let pending_mgr = pending_mgr.inner().clone();

    log_structured(
        StructuredLogLevel::Info,
        "ssh.auth",
        "keyboard_interactive.start",
        "Starting keyboard-interactive authentication",
        connection_id,
        None,
        Some(json!({
            "username": username,
            "mode": mode.label(),
        })),
        None,
    );

    let mut step = handle
        .authenticate_keyboard_interactive_start(username, None)
        .await
        .map_err(|error| AppError::Auth(format!("Keyboard-interactive start failed: {}", error)))?;
    let mut pending_totp_use: Option<TotpUseCandidate> = None;

    loop {
        match step {
            KeyboardInteractiveAuthResponse::Success => {
                if let Some(candidate) = pending_totp_use.take() {
                    record_totp_code_use(candidate);
                }
                log_structured(
                    StructuredLogLevel::Info,
                    "ssh.auth",
                    "keyboard_interactive.succeeded",
                    "Keyboard-interactive authentication succeeded",
                    connection_id,
                    None,
                    Some(json!({
                        "username": username,
                        "mode": mode.label(),
                    })),
                    None,
                );
                return Ok(());
            }
            KeyboardInteractiveAuthResponse::Failure {
                remaining_methods,
                partial_success,
            } => {
                log_structured(
                    StructuredLogLevel::Warn,
                    "ssh.auth",
                    "keyboard_interactive.failed",
                    "Keyboard-interactive authentication failed",
                    connection_id,
                    None,
                    Some(json!({
                        "username": username,
                        "mode": mode.label(),
                        "remaining_methods": format!("{remaining_methods:?}"),
                        "partial_success": partial_success,
                    })),
                    None,
                );
                return Err(AppError::Auth(
                    "Keyboard-interactive authentication failed".to_string(),
                ));
            }
            KeyboardInteractiveAuthResponse::InfoRequest {
                name: _,
                instructions: _,
                prompts,
            } => {
                let hidden_prompts = prompts.iter().filter(|prompt| !prompt.echo).count();
                log_structured(
                    StructuredLogLevel::Debug,
                    "ssh.auth",
                    "keyboard_interactive.prompts_received",
                    "Received keyboard-interactive prompts",
                    connection_id,
                    None,
                    Some(json!({
                        "username": username,
                        "mode": mode.label(),
                        "prompt_count": prompts.len(),
                        "hidden_prompts": hidden_prompts,
                    })),
                    None,
                );

                let responses = if prompts.is_empty() {
                    Vec::new()
                } else if let Some(password) = mode
                    .password()
                    .filter(|_| should_auto_fill_password_prompts(&prompts))
                {
                    log_structured(
                        StructuredLogLevel::Info,
                        "security.flow",
                        "keyboard_interactive.password_autofill",
                        "Auto-filling password for keyboard-interactive auth",
                        connection_id,
                        None,
                        Some(json!({
                            "username": username,
                            "prompt_count": prompts.len(),
                        })),
                        None,
                    );
                    vec![password.to_string()]
                } else if let Some(info) = otp_info.filter(|i| i.auto_fill) {
                    log_structured(
                        StructuredLogLevel::Info,
                        "security.flow",
                        "otp.auto_fill",
                        "Auto-filling OTP for keyboard-interactive auth",
                        connection_id,
                        None,
                        Some(json!({
                            "username": username,
                            "otp_entry_id": info.otp_id,
                            "prompt_count": prompts.len(),
                        })),
                        None,
                    );
                    let prepared = generate_keyboard_interactive_otp_responses(
                        app,
                        info,
                        prompts.len(),
                        connection_id,
                    )
                    .await?;
                    pending_totp_use = prepared.used_totp;
                    prepared.responses
                } else {
                    let request_id = uuid::Uuid::new_v4().to_string();
                    let rx = pending_mgr.register(request_id.clone()).await;

                    let payload = OtpRequestPayload {
                        request_id: request_id.clone(),
                        connection_name: connection_name.to_string(),
                        prompts: prompts
                            .iter()
                            .map(|prompt| OtpPrompt {
                                prompt: prompt.prompt.clone(),
                                echo: prompt.echo,
                            })
                        .collect(),
                        otp_entry_id: otp_info.map(|i| i.otp_id.clone()),
                        target_window_label: target_window_label.map(str::to_string),
                    };
                    log_structured(
                        StructuredLogLevel::Info,
                        "security.flow",
                        "otp.requested",
                        "Forwarding keyboard-interactive prompts to frontend",
                        connection_id,
                        Some(&request_id),
                        Some(json!({
                            "username": username,
                            "prompt_count": payload.prompts.len(),
                            "otp_entry_id": payload.otp_entry_id,
                        })),
                        None,
                    );
                    let _ = app.emit("otp-request", &payload);

                    let responses = match rx.await {
                        Ok(Some(responses)) => responses,
                        Ok(None) => {
                            return Err(AppError::Auth(
                                "2FA authentication cancelled by user".to_string(),
                            ));
                        }
                        Err(_) => {
                            return Err(AppError::Auth(
                                "2FA authentication request dropped".to_string(),
                            ));
                        }
                    };
                    let prepared =
                        prepare_manual_otp_responses(app, otp_info, responses, connection_id)
                            .await?;
                    pending_totp_use = prepared.used_totp;
                    prepared.responses
                };

                step = handle
                    .authenticate_keyboard_interactive_respond(responses)
                    .await
                    .map_err(|error| {
                        AppError::Auth(format!("Keyboard-interactive respond failed: {}", error))
                    })?;
            }
        }
    }
}

/// After primary auth returns `Failure`, continue with keyboard-interactive when
/// the server advertises it and either partial success or an explicit fallback applies.
async fn try_keyboard_interactive_after_partial(
    handle: &mut client::Handle<SshHandler>,
    auth_result: &client::AuthResult,
    username: &str,
    connection_id: Option<&str>,
    connection_name: &str,
    target_window_label: Option<&str>,
    app: &AppHandle,
    fallback_error: &str,
    keyboard_interactive_fallback: Option<KeyboardInteractiveMode<'_>>,
    otp_info: Option<&OtpAutoFillInfo>,
) -> AppResult<()> {
    match auth_result {
        client::AuthResult::Success => Ok(()),
        client::AuthResult::Failure {
            remaining_methods,
            partial_success,
        } => {
            let keyboard_interactive_available =
                remaining_methods.contains(&MethodKind::KeyboardInteractive);
            let can_retry_keyboard_interactive =
                keyboard_interactive_available && keyboard_interactive_fallback.is_some();

            if *partial_success && keyboard_interactive_available {
                log_structured(
                    StructuredLogLevel::Info,
                    "ssh.auth",
                    "auth.partial_success",
                    "Primary auth partial success, continuing with keyboard-interactive",
                    connection_id,
                    None,
                    Some(json!({
                        "username": username,
                        "remaining_methods": format!("{remaining_methods:?}"),
                    })),
                    None,
                );
                finish_keyboard_interactive(
                    handle,
                    username,
                    connection_id,
                    connection_name,
                    target_window_label,
                    app,
                    KeyboardInteractiveMode::AdditionalFactor,
                    otp_info,
                )
                .await
            } else if can_retry_keyboard_interactive {
                log_structured(
                    StructuredLogLevel::Info,
                    "ssh.auth",
                    "auth.keyboard_interactive_fallback",
                    "Primary auth rejected, retrying with keyboard-interactive",
                    connection_id,
                    None,
                    Some(json!({
                        "username": username,
                        "remaining_methods": format!("{remaining_methods:?}"),
                    })),
                    None,
                );
                let Some(mode) = keyboard_interactive_fallback else {
                    return Err(AppError::Auth(fallback_error.to_string()));
                };
                finish_keyboard_interactive(
                    handle,
                    username,
                    connection_id,
                    connection_name,
                    target_window_label,
                    app,
                    mode,
                    otp_info,
                )
                .await
            } else {
                log_structured(
                    StructuredLogLevel::Warn,
                    "ssh.auth",
                    "auth.failed",
                    "SSH authentication failed without usable keyboard-interactive fallback",
                    connection_id,
                    None,
                    Some(json!({
                        "username": username,
                        "remaining_methods": format!("{remaining_methods:?}"),
                        "partial_success": partial_success,
                    })),
                    Some(json!({
                        "message": fallback_error,
                    })),
                );
                Err(AppError::Auth(fallback_error.to_string()))
            }
        }
    }
}

fn should_auto_fill_password_prompts(prompts: &[client::Prompt]) -> bool {
    prompts.len() == 1
        && !prompts[0].echo
        && is_password_keyboard_interactive_prompt(&prompts[0].prompt)
}

fn is_password_keyboard_interactive_prompt(prompt: &str) -> bool {
    let normalized = prompt.to_lowercase();

    let additional_factor_markers = [
        "otp",
        "totp",
        "hotp",
        "2fa",
        "mfa",
        "one-time",
        "one time",
        "verification",
        "authentication code",
        "auth code",
        "authenticator",
        "passcode",
        "token",
        "code",
        "验证码",
        "校验码",
        "动态码",
        "动态密码",
        "动态口令",
        "一次性",
        "令牌",
        "双因素",
        "二次",
        "两步",
    ];
    if additional_factor_markers
        .iter()
        .any(|marker| normalized.contains(marker))
    {
        return false;
    }

    ["password", "passphrase", "密码", "口令"]
        .iter()
        .any(|marker| normalized.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::{
        is_password_keyboard_interactive_prompt, is_totp_code_reused, record_totp_code_use,
        resolve_password_material, seconds_until_next_totp_step, should_auto_fill_password_prompts,
        used_totp_codes, KeyboardInteractiveMode, TotpUseCandidate,
    };
    use crate::config::ConnectionAuth;
    use russh::client::Prompt;

    #[test]
    fn auto_fills_single_hidden_keyboard_interactive_prompt() {
        let prompts = vec![Prompt {
            prompt: "Password: ".to_string(),
            echo: false,
        }];

        assert!(should_auto_fill_password_prompts(&prompts));
    }

    #[test]
    fn does_not_auto_fill_single_otp_keyboard_interactive_prompt() {
        let prompts = vec![Prompt {
            prompt: "Verification code: ".to_string(),
            echo: false,
        }];

        assert!(!should_auto_fill_password_prompts(&prompts));
    }

    #[test]
    fn does_not_treat_passcode_as_password_prompt() {
        assert!(!is_password_keyboard_interactive_prompt("Passcode: "));
        assert!(!is_password_keyboard_interactive_prompt("OTP Password: "));
        assert!(!is_password_keyboard_interactive_prompt("动态口令: "));
        assert!(!is_password_keyboard_interactive_prompt("验证码: "));
    }

    #[test]
    fn recognizes_password_keyboard_interactive_prompts() {
        assert!(is_password_keyboard_interactive_prompt(
            "root@example.com's password: "
        ));
        assert!(is_password_keyboard_interactive_prompt("Passphrase: "));
        assert!(is_password_keyboard_interactive_prompt("密码: "));
        assert!(is_password_keyboard_interactive_prompt("口令: "));
    }

    #[test]
    fn does_not_auto_fill_multiple_keyboard_interactive_prompts() {
        let prompts = vec![
            Prompt {
                prompt: "Password: ".to_string(),
                echo: false,
            },
            Prompt {
                prompt: "Verification code: ".to_string(),
                echo: false,
            },
        ];

        assert!(!should_auto_fill_password_prompts(&prompts));
    }

    #[test]
    fn does_not_auto_fill_echoed_prompt() {
        let prompts = vec![Prompt {
            prompt: "Username: ".to_string(),
            echo: true,
        }];

        assert!(!should_auto_fill_password_prompts(&prompts));
    }

    #[test]
    fn additional_factor_mode_never_exposes_password_fallback() {
        assert!(KeyboardInteractiveMode::AdditionalFactor
            .password()
            .is_none());
    }

    #[test]
    fn password_mode_without_material_returns_recoverable_error() {
        let auth = ConnectionAuth {
            mode: "password".to_string(),
            ..Default::default()
        };

        let error = resolve_password_material(None, &auth).unwrap_err();

        assert!(error
            .to_string()
            .contains("No password for this connection"));
    }

    #[test]
    fn totp_step_wait_uses_remaining_period() {
        assert_eq!(seconds_until_next_totp_step(31, 30), 29);
        assert_eq!(seconds_until_next_totp_step(59, 30), 1);
        assert_eq!(seconds_until_next_totp_step(60, 30), 30);
    }

    #[test]
    fn used_totp_cache_matches_same_code_and_step_only() {
        used_totp_codes()
            .lock()
            .expect("used TOTP code cache poisoned")
            .clear();
        let candidate = TotpUseCandidate {
            otp_id: "otp-1".to_string(),
            code: "123456".to_string(),
            time_step: 42,
        };

        assert!(!is_totp_code_reused(&candidate));
        record_totp_code_use(candidate.clone());
        assert!(is_totp_code_reused(&candidate));
        assert!(!is_totp_code_reused(&TotpUseCandidate {
            time_step: 43,
            ..candidate.clone()
        }));
        assert!(!is_totp_code_reused(&TotpUseCandidate {
            code: "654321".to_string(),
            ..candidate
        }));
    }
}
