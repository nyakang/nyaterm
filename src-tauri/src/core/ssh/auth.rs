use super::agent::{DynamicAgentClient, connect_agent_client};
use super::client::{SshAuth, SshConfig, SshHandler, SshPostLoginConfig};
use crate::error::{AppError, AppResult};
use crate::observability::{self, StructuredLog, StructuredLogLevel};
use russh::client::{self, KeyboardInteractiveAuthResponse};
use russh::keys::{Algorithm, HashAlg, agent::AgentIdentity};
use russh::{MethodKind, MethodSet};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Mutex, oneshot};
use tokio::time::{Duration, timeout};

const AGENT_IDENTITY_TIMEOUT: Duration = Duration::from_secs(5);
const AGENT_SIGN_TIMEOUT: Duration = Duration::from_secs(60);
const AGENT_APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);
const INTERACTIVE_AUTH_TIMEOUT: Duration = Duration::from_secs(300);

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

    pub async fn finish(&self, request_id: &str) {
        self.pending.lock().await.remove(request_id);
    }
}

/// Manages pending runtime SSH credential requests awaiting user input.
pub struct PendingSshAuthManager {
    pending: Mutex<HashMap<String, oneshot::Sender<Option<SshAuthResponse>>>>,
}

#[derive(Debug, Clone, Copy)]
pub enum SshAgentAuthAction {
    Retry,
    Cancel,
}

/// Manages requests waiting for hardware-key, PIN, or desktop Agent approval.
pub struct PendingSshAgentAuthManager {
    pending: Mutex<HashMap<String, oneshot::Sender<SshAgentAuthAction>>>,
}

impl PendingSshAgentAuthManager {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub async fn register(&self, request_id: String) -> oneshot::Receiver<SshAgentAuthAction> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id, tx);
        rx
    }

    pub async fn respond(&self, request_id: &str, action: SshAgentAuthAction) -> bool {
        self.pending
            .lock()
            .await
            .remove(request_id)
            .is_some_and(|tx| tx.send(action).is_ok())
    }

    pub async fn finish(&self, request_id: &str) {
        self.pending.lock().await.remove(request_id);
    }
}

pub(super) const SSH_AGENT_AUTH_RETRY: &str = "ssh-agent-auth-retry";

#[derive(Clone, Copy)]
enum SecurityPromptManager {
    Otp,
    SshAuth,
}

struct SecurityPromptRequestGuard {
    app: AppHandle,
    request_id: String,
    manager: SecurityPromptManager,
    resolved: AtomicBool,
}

impl SecurityPromptRequestGuard {
    fn new(app: AppHandle, request_id: String, manager: SecurityPromptManager) -> Self {
        Self {
            app,
            request_id,
            manager,
            resolved: AtomicBool::new(false),
        }
    }

    fn mark_resolved(&self) {
        self.resolved.store(true, Ordering::Release);
    }
}

impl Drop for SecurityPromptRequestGuard {
    fn drop(&mut self) {
        if self.resolved.swap(true, Ordering::AcqRel) {
            return;
        }

        let app = self.app.clone();
        let request_id = self.request_id.clone();
        let manager = self.manager;
        tauri::async_runtime::spawn(async move {
            finish_security_prompt_request(&app, &request_id, manager).await;
        });
    }
}

impl PendingSshAuthManager {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub async fn register(&self, request_id: String) -> oneshot::Receiver<Option<SshAuthResponse>> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id, tx);
        rx
    }

    pub async fn respond(&self, request_id: &str, response: Option<SshAuthResponse>) -> bool {
        if let Some(tx) = self.pending.lock().await.remove(request_id) {
            tx.send(response).is_ok()
        } else {
            false
        }
    }

    pub async fn finish(&self, request_id: &str) {
        self.pending.lock().await.remove(request_id);
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshAuthResponse {
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub key_id: Option<String>,
    #[serde(default)]
    pub password_id: Option<String>,
    #[serde(default)]
    pub save: Option<SshAuthSaveRequest>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshAuthSaveRequest {
    pub kind: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub password_id: Option<String>,
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
    name: Option<String>,
    instructions: Option<String>,
    round: u32,
    prompts: Vec<OtpPrompt>,
    otp_entry_id: Option<String>,
    target_window_label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SshAuthPromptReason {
    MissingPassword,
    PasswordRejected,
    KeyPassphraseRequired,
    KeyRejectedPasswordFallback,
    PublickeyRejected,
    PublickeyRequired,
}

impl SshAuthPromptReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::MissingPassword => "missing_password",
            Self::PasswordRejected => "password_rejected",
            Self::KeyPassphraseRequired => "key_passphrase_required",
            Self::KeyRejectedPasswordFallback => "key_rejected_password_fallback",
            Self::PublickeyRejected => "publickey_rejected",
            Self::PublickeyRequired => "publickey_required",
        }
    }

    fn prompt_kind(self) -> &'static str {
        match self {
            Self::KeyPassphraseRequired => "passphrase",
            Self::PublickeyRejected | Self::PublickeyRequired => "publickey",
            _ => "password",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SshAuthRequestPayload {
    request_id: String,
    connection_id: Option<String>,
    connection_name: String,
    host: String,
    port: u16,
    username: String,
    reason: String,
    prompt_kind: String,
    available_methods: Vec<String>,
    current_auth_mode: String,
    attempt: u32,
    can_save: bool,
    password_id: Option<String>,
    target_window_label: Option<String>,
}

#[derive(Debug, Clone)]
struct RuntimeSecret {
    value: String,
    save: Option<RuntimeSecretSave>,
}

#[derive(Debug, Clone)]
struct RuntimeKeyPassphrase {
    key_id: String,
    secret: RuntimeSecret,
}

#[derive(Debug, Clone)]
enum RuntimeSecretSave {
    ConnectionInline,
    SavedPassword { id: Option<String>, name: String },
    SavedPasswordReference { id: String },
    KeyPassphrase,
}

#[derive(Debug, Clone, Default)]
struct SshRuntimeAuthUpdates {
    password: Option<RuntimeSecret>,
    key_passphrase: Option<RuntimeKeyPassphrase>,
}

#[derive(Debug, Clone, Default)]
struct RuntimePasswordAuthOutcome {
    password: Option<RuntimeSecret>,
    key_passphrase: Option<RuntimeKeyPassphrase>,
}

#[derive(Debug, Clone)]
enum RuntimeAuthSelection {
    Password(RuntimeSecret),
    Key(String),
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

#[derive(Debug, Clone)]
struct SshAuthFailure {
    message: String,
    remaining_methods: Vec<String>,
    partial_success: bool,
}

impl SshAuthFailure {
    fn from_auth_result(
        message: impl Into<String>,
        remaining_methods: &MethodSet,
        partial_success: bool,
    ) -> Self {
        Self {
            message: message.into(),
            remaining_methods: method_names(remaining_methods),
            partial_success,
        }
    }

    fn from_error(message: impl Into<String>, error: AppError) -> Self {
        Self {
            message: format!("{}: {}", message.into(), error),
            remaining_methods: Vec::new(),
            partial_success: false,
        }
    }

    fn has_method(&self, method: &str) -> bool {
        self.remaining_methods
            .iter()
            .any(|candidate| candidate == method)
    }

    fn password_available(&self) -> bool {
        self.has_method("password") || self.has_method("keyboard-interactive")
    }

    fn publickey_available(&self) -> bool {
        self.has_method("publickey")
    }
}

impl From<SshAuthFailure> for AppError {
    fn from(value: SshAuthFailure) -> Self {
        if value.remaining_methods.is_empty() {
            AppError::Auth(value.message)
        } else {
            AppError::Auth(format!(
                "{} (remaining methods: {}; partial success: {})",
                value.message,
                value.remaining_methods.join(", "),
                value.partial_success
            ))
        }
    }
}

fn method_names(methods: &MethodSet) -> Vec<String> {
    methods.iter().map(String::from).collect()
}

fn current_auth_mode(auth: &SshAuth) -> &'static str {
    match auth {
        SshAuth::None => "none",
        SshAuth::Password { .. } => "password",
        SshAuth::Key { .. } => "key",
        SshAuth::Agent => "agent",
    }
}

fn default_available_methods(reason: SshAuthPromptReason) -> Vec<String> {
    match reason.prompt_kind() {
        "passphrase" | "publickey" => vec!["publickey".to_string()],
        _ => vec!["password".to_string(), "keyboard-interactive".to_string()],
    }
}

fn runtime_prompt_kind(reason: SshAuthPromptReason, available_methods: &[String]) -> &'static str {
    if reason == SshAuthPromptReason::KeyPassphraseRequired {
        return "passphrase";
    }

    let password_available = available_methods
        .iter()
        .any(|method| method == "password" || method == "keyboard-interactive");
    let publickey_available = available_methods.iter().any(|method| method == "publickey");
    if password_available && publickey_available {
        "auth_method"
    } else {
        reason.prompt_kind()
    }
}

pub(crate) fn load_saved_ssh_config(app: &AppHandle, connection_id: &str) -> AppResult<SshConfig> {
    let conn = crate::config::load_connection_by_id(app, connection_id)?;
    let mut visited = HashSet::new();
    visited.insert(connection_id.to_string());
    resolve_saved_ssh_config(
        app,
        &conn,
        Some(connection_id.to_string()),
        true,
        &mut visited,
    )
}

fn resolve_saved_ssh_config(
    app: &AppHandle,
    conn: &crate::config::SavedConnection,
    connection_id: Option<String>,
    include_proxy_jump: bool,
    visited_proxy_jumps: &mut HashSet<String>,
) -> AppResult<SshConfig> {
    let proxy = resolve_proxy(app, conn)?;
    let (host, port, username) = resolve_ssh_target(conn)?;
    let auth = resolve_auth(app, conn)?;
    let proxy_jump = if include_proxy_jump {
        resolve_proxy_jump(app, conn, visited_proxy_jumps)?
    } else {
        None
    };
    let post_login = resolve_post_login(conn);
    let x11_forwarding = resolve_x11_forwarding(conn);
    let (auth_agent_endpoint, agent_forwarding_config) = resolve_agent_settings(conn);
    let x11_display = crate::config::load_app_settings(app)
        .map(|settings| settings.terminal.x11_display)
        .unwrap_or_default();

    let encoding = crate::config::resolve_connection_encoding(app, conn);

    Ok(SshConfig {
        connection_id,
        owner_window_label: None,
        name: conn.name.clone(),
        host,
        port,
        username,
        auth,
        backspace_mode: resolve_ssh_backspace_mode(conn),
        x11_forwarding,
        x11_display,
        auth_agent_endpoint,
        agent_forwarding_config,
        proxy,
        proxy_jump,
        post_login,
        ssh_algorithms: conn.ssh_algorithms.clone(),
        ssh_profile: conn.ssh_profile.clone(),
        runtime_mode: crate::config::SshRuntimeMode::Standard,
        terminal_type: crate::config::resolve_ssh_terminal_type(
            &conn.ssh_profile,
            conn.terminal_type.as_ref(),
        ),
        sftp: conn.sftp.clone(),
        encoding,
        dynamic_tab_title: matches!(
            &conn.config,
            crate::config::ConnectionType::Ssh {
                dynamic_tab_title: true,
                ..
            }
        ),
    })
}

fn resolve_x11_forwarding(conn: &crate::config::SavedConnection) -> bool {
    match &conn.config {
        crate::config::ConnectionType::Ssh { x11_forwarding, .. } => *x11_forwarding,
        _ => false,
    }
}

fn resolve_agent_settings(
    conn: &crate::config::SavedConnection,
) -> (
    Option<crate::config::SshAgentEndpoint>,
    crate::config::SshAgentForwardingConfig,
) {
    match &conn.config {
        crate::config::ConnectionType::Ssh {
            auth_agent_endpoint,
            agent_forwarding_config,
            ..
        } => (
            auth_agent_endpoint.clone(),
            agent_forwarding_config.clone().unwrap_or_default(),
        ),
        _ => (None, crate::config::SshAgentForwardingConfig::default()),
    }
}

fn resolve_ssh_backspace_mode(conn: &crate::config::SavedConnection) -> String {
    match &conn.config {
        crate::config::ConnectionType::Ssh { backspace_mode, .. } => backspace_mode.clone(),
        _ => "del".to_string(),
    }
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
            ..
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
        "agent" => Ok(SshAuth::Agent),
        "key" => {
            let Some(key_id) = conn_auth.key_id.as_deref() else {
                return Ok(SshAuth::None);
            };
            let ssh_key = crate::config::load_key_by_id(app, key_id)?;
            let key_data = crate::config::decrypt_key_pem(&ssh_key)?
                .ok_or_else(|| AppError::Auth("No key data stored".to_string()))?;
            let cert_data = crate::config::decrypt_key_cert(&ssh_key)?;
            Ok(SshAuth::Key {
                key_id: Some(key_id.to_string()),
                key_data,
                cert_data,
                passphrase: ssh_key.passphrase,
            })
        }
        other => Err(AppError::Auth(format!("Unknown auth type: {}", other))),
    }
}

fn resolve_password_material(
    app: Option<&AppHandle>,
    conn_auth: &crate::config::ConnectionAuth,
) -> AppResult<Option<String>> {
    if let Some(ref ciphertext) = conn_auth.password {
        return crate::utils::crypto::decrypt(ciphertext)
            .map(Some)
            .map_err(|e| AppError::Auth(format!("Failed to decrypt inline password: {e}")));
    }

    let Some(pw_id) = conn_auth.password_id.as_deref().filter(|id| !id.is_empty()) else {
        return Ok(None);
    };

    let app = app.ok_or_else(|| AppError::Auth("No password for this connection".to_string()))?;
    let pw_entry = crate::config::load_password_by_id(app, pw_id)?;
    Ok(pw_entry.password)
}

fn resolve_proxy_jump(
    app: &AppHandle,
    conn: &crate::config::SavedConnection,
    visited_proxy_jumps: &mut HashSet<String>,
) -> AppResult<Option<Box<SshConfig>>> {
    let proxy_jump_id = conn
        .network
        .as_ref()
        .and_then(|network| network.proxy_jump_id.as_deref());

    let Some(proxy_jump_id) = proxy_jump_id else {
        return Ok(None);
    };

    if !visited_proxy_jumps.insert(proxy_jump_id.to_string()) {
        return Err(AppError::Config(format!(
            "ProxyJump chain contains a cycle at '{}'",
            proxy_jump_id
        )));
    }

    let jump_conn = crate::config::load_connection_by_id(app, proxy_jump_id)?;
    if !matches!(jump_conn.config, crate::config::ConnectionType::Ssh { .. }) {
        return Err(AppError::Config(
            "Only SSH connections can be used as jump hosts".to_string(),
        ));
    }

    Ok(Some(Box::new(resolve_saved_ssh_config(
        app,
        &jump_conn,
        Some(proxy_jump_id.to_string()),
        true,
        visited_proxy_jumps,
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
        command: proxy_cfg.command,
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
    let mut updates = SshRuntimeAuthUpdates::default();
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

            let password_outcome = authenticate_password_with_runtime_prompt(
                handle,
                config,
                app,
                password.as_deref(),
                password_error,
                None,
                None,
                otp_info.as_ref(),
            )
            .await?;
            if let Some(secret) = password_outcome.password {
                updates.password = Some(secret);
            }
            if let Some(secret) = password_outcome.key_passphrase {
                updates.key_passphrase = Some(secret);
            }
        }
        SshAuth::Key { .. } => {
            let key_result = authenticate_publickey_with_runtime_selection(
                handle,
                config,
                app,
                key_error,
                otp_info.as_ref(),
            )
            .await;

            match key_result {
                Ok(Some(secret)) => {
                    updates.key_passphrase = Some(secret);
                }
                Ok(None) => {}
                Err(failure) if failure.password_available() => {
                    let reason = if failure.publickey_available() {
                        SshAuthPromptReason::KeyRejectedPasswordFallback
                    } else {
                        SshAuthPromptReason::PasswordRejected
                    };
                    let password_outcome = authenticate_password_with_runtime_prompt(
                        handle,
                        config,
                        app,
                        None,
                        key_error,
                        Some(reason),
                        Some(failure.remaining_methods.clone()),
                        otp_info.as_ref(),
                    )
                    .await?;
                    if let Some(secret) = password_outcome.password {
                        updates.password = Some(secret);
                    }
                    if let Some(secret) = password_outcome.key_passphrase {
                        updates.key_passphrase = Some(secret);
                    }
                }
                Err(failure) if failure.publickey_available() => {
                    let key_secret = authenticate_publickey_with_runtime_key_prompt(
                        handle,
                        config,
                        app,
                        key_error,
                        SshAuthPromptReason::PublickeyRejected,
                        failure.remaining_methods.clone(),
                        otp_info.as_ref(),
                    )
                    .await?;
                    if let Some(secret) = key_secret {
                        updates.key_passphrase = Some(secret);
                    }
                }
                Err(failure) => return Err(failure.into()),
            }
        }
        SshAuth::Agent => {
            authenticate_agent(handle, config, app, key_error, otp_info.as_ref()).await?;
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
    persist_runtime_auth_updates(app, config, &updates)?;
    Ok(())
}

const MAX_RUNTIME_SSH_AUTH_ATTEMPTS: u32 = 3;

async fn request_runtime_secret(
    app: &AppHandle,
    config: &SshConfig,
    reason: SshAuthPromptReason,
    attempt: u32,
    can_save: bool,
) -> AppResult<RuntimeSecret> {
    let response = request_runtime_auth_response(
        app,
        config,
        reason,
        attempt,
        can_save,
        default_available_methods(reason),
    )
    .await?;
    let secret = response.secret.unwrap_or_default();
    if secret.is_empty() {
        return Err(AppError::Auth(
            "SSH authentication response was empty".to_string(),
        ));
    }

    Ok(RuntimeSecret {
        value: secret,
        save: parse_runtime_secret_save(response.save, reason, can_save, None),
    })
}

async fn request_runtime_key_id(
    app: &AppHandle,
    config: &SshConfig,
    reason: SshAuthPromptReason,
    attempt: u32,
    available_methods: Vec<String>,
) -> AppResult<String> {
    let response =
        request_runtime_auth_response(app, config, reason, attempt, false, available_methods)
            .await?;
    let method = response.method.as_deref().unwrap_or("key");
    if method != "key" && method != "publickey" {
        return Err(AppError::Auth(format!(
            "Unsupported SSH authentication method '{}'",
            method
        )));
    }
    response
        .key_id
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| AppError::Auth("No SSH key was selected".to_string()))
}

async fn request_runtime_auth_selection(
    app: &AppHandle,
    config: &SshConfig,
    reason: SshAuthPromptReason,
    attempt: u32,
    can_save: bool,
    available_methods: Vec<String>,
) -> AppResult<RuntimeAuthSelection> {
    let response = request_runtime_auth_response(
        app,
        config,
        reason,
        attempt,
        can_save,
        available_methods.clone(),
    )
    .await?;
    let method = response
        .method
        .as_deref()
        .unwrap_or_else(|| {
            if available_methods.iter().any(|method| method == "publickey")
                && !available_methods.iter().any(|method| method == "password")
                && !available_methods
                    .iter()
                    .any(|method| method == "keyboard-interactive")
            {
                "key"
            } else {
                "password"
            }
        })
        .to_string();

    match method.as_str() {
        "password" => {
            let secret = response.secret.unwrap_or_default();
            if secret.is_empty() {
                return Err(AppError::Auth(
                    "SSH authentication response was empty".to_string(),
                ));
            }
            Ok(RuntimeAuthSelection::Password(RuntimeSecret {
                value: secret,
                save: parse_runtime_secret_save(response.save, reason, can_save, None),
            }))
        }
        "saved_password" => {
            let password_id = response
                .password_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .ok_or_else(|| AppError::Auth("No saved password was selected".to_string()))?;
            let password = crate::config::load_password_by_id(app, &password_id)?
                .password
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    AppError::Auth("Selected saved password has no password data".to_string())
                })?;
            Ok(RuntimeAuthSelection::Password(RuntimeSecret {
                value: password,
                save: parse_runtime_secret_save(
                    response.save,
                    reason,
                    can_save,
                    Some(&password_id),
                ),
            }))
        }
        "key" | "publickey" => response
            .key_id
            .filter(|id| !id.trim().is_empty())
            .map(RuntimeAuthSelection::Key)
            .ok_or_else(|| AppError::Auth("No SSH key was selected".to_string())),
        other => Err(AppError::Auth(format!(
            "Unsupported SSH authentication method '{}'",
            other
        ))),
    }
}

async fn request_runtime_auth_response(
    app: &AppHandle,
    config: &SshConfig,
    reason: SshAuthPromptReason,
    attempt: u32,
    can_save: bool,
    available_methods: Vec<String>,
) -> AppResult<SshAuthResponse> {
    let pending_mgr = app
        .try_state::<Arc<PendingSshAuthManager>>()
        .ok_or_else(|| AppError::Auth("PendingSshAuthManager not available".to_string()))?;
    let pending_mgr = pending_mgr.inner().clone();
    let request_id = uuid::Uuid::new_v4().to_string();
    let rx = pending_mgr.register(request_id.clone()).await;
    let request_guard = SecurityPromptRequestGuard::new(
        app.clone(),
        request_id.clone(),
        SecurityPromptManager::SshAuth,
    );
    let payload = SshAuthRequestPayload {
        request_id: request_id.clone(),
        connection_id: config.connection_id.clone(),
        connection_name: config.name.clone(),
        host: config.host.clone(),
        port: config.port,
        username: config.username.clone(),
        reason: reason.as_str().to_string(),
        prompt_kind: runtime_prompt_kind(reason, &available_methods).to_string(),
        available_methods,
        current_auth_mode: current_auth_mode(&config.auth).to_string(),
        attempt,
        can_save,
        password_id: current_password_id(app, config.connection_id.as_deref()),
        target_window_label: config.owner_window_label.clone(),
    };

    log_structured(
        StructuredLogLevel::Info,
        "security.flow",
        "ssh_auth.requested",
        "Forwarding SSH credential request to frontend",
        config.connection_id.as_deref(),
        Some(&request_id),
        Some(json!({
            "username": config.username,
            "reason": payload.reason,
            "prompt_kind": payload.prompt_kind,
            "available_methods": payload.available_methods,
            "current_auth_mode": payload.current_auth_mode,
            "attempt": attempt,
            "can_save": can_save,
        })),
        None,
    );
    let _ = app.emit("ssh-auth-request", &payload);

    let response = match timeout(INTERACTIVE_AUTH_TIMEOUT, rx).await {
        Ok(Ok(Some(response))) => Ok(response),
        Ok(Ok(None)) => Err(AppError::Auth(
            "SSH authentication cancelled by user".to_string(),
        )),
        Ok(Err(_)) => Err(AppError::Auth(
            "SSH authentication request dropped".to_string(),
        )),
        Err(_) => Err(AppError::Auth(
            "SSH authentication request timed out".to_string(),
        )),
    };
    finish_security_prompt_request(app, &request_id, SecurityPromptManager::SshAuth).await;
    request_guard.mark_resolved();
    response
}

fn current_password_id(app: &AppHandle, connection_id: Option<&str>) -> Option<String> {
    let connection_id = connection_id?;
    let conn = crate::config::load_connection_by_id(app, connection_id).ok()?;
    conn.auth?.password_id.filter(|id| !id.is_empty())
}

fn parse_runtime_secret_save(
    save: Option<SshAuthSaveRequest>,
    reason: SshAuthPromptReason,
    can_save: bool,
    selected_password_id: Option<&str>,
) -> Option<RuntimeSecretSave> {
    if !can_save {
        return None;
    }
    let save = save?;
    match (reason.prompt_kind(), save.kind.as_str()) {
        ("passphrase", "key_passphrase") => Some(RuntimeSecretSave::KeyPassphrase),
        ("password", "connection") => {
            if let Some(id) = selected_password_id {
                Some(RuntimeSecretSave::SavedPasswordReference { id: id.to_string() })
            } else {
                Some(RuntimeSecretSave::ConnectionInline)
            }
        }
        ("password", "saved_password") => {
            let name = save
                .name
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("SSH Password")
                .to_string();
            Some(RuntimeSecretSave::SavedPassword {
                id: save.password_id.filter(|id| !id.is_empty()),
                name,
            })
        }
        _ => None,
    }
}

async fn authenticate_password_with_runtime_prompt(
    handle: &mut client::Handle<SshHandler>,
    config: &SshConfig,
    app: &AppHandle,
    initial_password: Option<&str>,
    fallback_error: &str,
    first_prompt_reason: Option<SshAuthPromptReason>,
    initial_available_methods: Option<Vec<String>>,
    otp_info: Option<&OtpAutoFillInfo>,
) -> AppResult<RuntimePasswordAuthOutcome> {
    let mut runtime_secret: Option<RuntimeSecret> = None;
    let mut password = initial_password.map(str::to_string);
    let can_save = config.connection_id.is_some();
    let mut advertised_methods = initial_available_methods;

    for attempt in 1..=MAX_RUNTIME_SSH_AUTH_ATTEMPTS {
        if password.as_deref().is_none_or(str::is_empty) {
            if attempt == 1
                && first_prompt_reason
                    .is_none_or(|reason| reason == SshAuthPromptReason::MissingPassword)
            {
                match discover_auth_methods_before_secret_prompt(
                    handle,
                    config,
                    app,
                    fallback_error,
                    otp_info,
                )
                .await
                {
                    Ok(Some(outcome)) => return Ok(outcome),
                    Ok(None) => {}
                    Err(failure) if failure.password_available() => {
                        advertised_methods = Some(failure.remaining_methods.clone());
                    }
                    Err(failure) if failure.publickey_available() => {
                        let key_passphrase = authenticate_publickey_with_runtime_key_prompt(
                            handle,
                            config,
                            app,
                            fallback_error,
                            SshAuthPromptReason::PublickeyRequired,
                            failure.remaining_methods.clone(),
                            otp_info,
                        )
                        .await?;
                        return Ok(RuntimePasswordAuthOutcome {
                            password: None,
                            key_passphrase,
                        });
                    }
                    Err(failure) => return Err(failure.into()),
                }
            }
            let reason = first_prompt_reason.unwrap_or(SshAuthPromptReason::MissingPassword);
            match request_runtime_auth_selection(
                app,
                config,
                reason,
                attempt,
                can_save,
                advertised_methods
                    .clone()
                    .unwrap_or_else(|| default_available_methods(reason)),
            )
            .await?
            {
                RuntimeAuthSelection::Password(secret) => {
                    password = Some(secret.value.clone());
                    runtime_secret = Some(secret);
                }
                RuntimeAuthSelection::Key(key_id) => {
                    let key_passphrase = authenticate_runtime_key_by_id(
                        handle,
                        config,
                        app,
                        fallback_error,
                        &key_id,
                        otp_info,
                    )
                    .await?;
                    return Ok(RuntimePasswordAuthOutcome {
                        password: None,
                        key_passphrase,
                    });
                }
            }
        }

        let Some(current_password) = password.as_deref() else {
            continue;
        };
        let authenticated = handle
            .authenticate_password(&config.username, current_password)
            .await
            .map_err(|error| AppError::Auth(format!("Authentication failed: {}", error)))?;

        match try_keyboard_interactive_after_partial(
            handle,
            &authenticated,
            &config.username,
            config.connection_id.as_deref(),
            &config.name,
            config.owner_window_label.as_deref(),
            app,
            fallback_error,
            Some(KeyboardInteractiveMode::PasswordFallback {
                password: current_password,
            }),
            otp_info,
        )
        .await
        {
            Ok(()) => {
                return Ok(RuntimePasswordAuthOutcome {
                    password: runtime_secret,
                    key_passphrase: None,
                });
            }
            Err(failure) if attempt < MAX_RUNTIME_SSH_AUTH_ATTEMPTS => {
                if failure.password_available() {
                    advertised_methods = Some(failure.remaining_methods.clone());
                    match request_runtime_auth_selection(
                        app,
                        config,
                        SshAuthPromptReason::PasswordRejected,
                        attempt + 1,
                        can_save,
                        failure.remaining_methods.clone(),
                    )
                    .await?
                    {
                        RuntimeAuthSelection::Password(secret) => {
                            password = Some(secret.value.clone());
                            runtime_secret = Some(secret);
                        }
                        RuntimeAuthSelection::Key(key_id) => {
                            let key_passphrase = authenticate_runtime_key_by_id(
                                handle,
                                config,
                                app,
                                fallback_error,
                                &key_id,
                                otp_info,
                            )
                            .await?;
                            return Ok(RuntimePasswordAuthOutcome {
                                password: None,
                                key_passphrase,
                            });
                        }
                    }
                } else if failure.publickey_available() {
                    let key_passphrase = authenticate_publickey_with_runtime_key_prompt(
                        handle,
                        config,
                        app,
                        fallback_error,
                        SshAuthPromptReason::PublickeyRequired,
                        failure.remaining_methods.clone(),
                        otp_info,
                    )
                    .await?;
                    return Ok(RuntimePasswordAuthOutcome {
                        password: None,
                        key_passphrase,
                    });
                } else {
                    return Err(failure.into());
                }
            }
            Err(failure) => return Err(failure.into()),
        }
    }

    Err(AppError::Auth(fallback_error.to_string()))
}

async fn discover_auth_methods_before_secret_prompt(
    handle: &mut client::Handle<SshHandler>,
    config: &SshConfig,
    app: &AppHandle,
    fallback_error: &str,
    otp_info: Option<&OtpAutoFillInfo>,
) -> Result<Option<RuntimePasswordAuthOutcome>, SshAuthFailure> {
    let authenticated = handle
        .authenticate_none(&config.username)
        .await
        .map_err(|error| SshAuthFailure {
            message: format!("None auth probe failed: {error}"),
            remaining_methods: Vec::new(),
            partial_success: false,
        })?;

    match try_keyboard_interactive_after_partial(
        handle,
        &authenticated,
        &config.username,
        config.connection_id.as_deref(),
        &config.name,
        config.owner_window_label.as_deref(),
        app,
        fallback_error,
        None,
        otp_info,
    )
    .await
    {
        Ok(()) => Ok(Some(RuntimePasswordAuthOutcome::default())),
        Err(failure) => Err(failure),
    }
}

async fn decode_secret_key_with_runtime_prompt(
    key_data: &str,
    initial_passphrase: Option<&str>,
    config: &SshConfig,
    app: &AppHandle,
) -> AppResult<(russh::keys::PrivateKey, Option<RuntimeSecret>)> {
    match russh::keys::decode_secret_key(key_data, initial_passphrase) {
        Ok(key) => return Ok((key, None)),
        Err(error) if initial_passphrase.is_some() => {
            tracing::debug!(%error, "Stored SSH key passphrase failed, requesting runtime passphrase");
        }
        Err(error) => {
            tracing::debug!(%error, "SSH key decode failed, requesting runtime passphrase");
        }
    }

    let can_save = matches!(
        &config.auth,
        SshAuth::Key {
            key_id: Some(_),
            ..
        }
    );
    let mut last_error: Option<russh::keys::Error> = None;
    for attempt in 1..=MAX_RUNTIME_SSH_AUTH_ATTEMPTS {
        let secret = request_runtime_secret(
            app,
            config,
            SshAuthPromptReason::KeyPassphraseRequired,
            attempt,
            can_save,
        )
        .await?;
        match russh::keys::decode_secret_key(key_data, Some(&secret.value)) {
            Ok(key) => return Ok((key, Some(secret))),
            Err(error) => {
                last_error = Some(error);
            }
        }
    }

    Err(last_error
        .map(AppError::from)
        .unwrap_or_else(|| AppError::Auth("Invalid SSH key passphrase".to_string())))
}

fn load_runtime_key_auth(app: &AppHandle, key_id: &str) -> AppResult<SshAuth> {
    let ssh_key = crate::config::load_key_by_id(app, key_id)?;
    let key_data = crate::config::decrypt_key_pem(&ssh_key)?
        .ok_or_else(|| AppError::Auth("No key data stored".to_string()))?;
    let cert_data = crate::config::decrypt_key_cert(&ssh_key)?;
    Ok(SshAuth::Key {
        key_id: Some(key_id.to_string()),
        key_data,
        cert_data,
        passphrase: ssh_key.passphrase,
    })
}

async fn authenticate_publickey_with_runtime_selection(
    handle: &mut client::Handle<SshHandler>,
    config: &SshConfig,
    app: &AppHandle,
    fallback_error: &str,
    otp_info: Option<&OtpAutoFillInfo>,
) -> Result<Option<RuntimeKeyPassphrase>, SshAuthFailure> {
    authenticate_publickey_attempt(handle, config, app, &config.auth, fallback_error, otp_info)
        .await
}

async fn authenticate_publickey_with_runtime_key_prompt(
    handle: &mut client::Handle<SshHandler>,
    config: &SshConfig,
    app: &AppHandle,
    fallback_error: &str,
    reason: SshAuthPromptReason,
    available_methods: Vec<String>,
    otp_info: Option<&OtpAutoFillInfo>,
) -> AppResult<Option<RuntimeKeyPassphrase>> {
    let mut last_failure: Option<SshAuthFailure> = None;
    for attempt in 1..=MAX_RUNTIME_SSH_AUTH_ATTEMPTS {
        let key_id =
            request_runtime_key_id(app, config, reason, attempt, available_methods.clone()).await?;
        let key_auth = load_runtime_key_auth(app, &key_id)?;

        match authenticate_publickey_attempt(
            handle,
            config,
            app,
            &key_auth,
            fallback_error,
            otp_info,
        )
        .await
        {
            Ok(secret) => return Ok(secret),
            Err(failure) => {
                last_failure = Some(failure);
            }
        }
    }

    Err(last_failure
        .map(AppError::from)
        .unwrap_or_else(|| AppError::Auth(fallback_error.to_string())))
}

async fn authenticate_runtime_key_by_id(
    handle: &mut client::Handle<SshHandler>,
    config: &SshConfig,
    app: &AppHandle,
    fallback_error: &str,
    key_id: &str,
    otp_info: Option<&OtpAutoFillInfo>,
) -> AppResult<Option<RuntimeKeyPassphrase>> {
    let key_auth = load_runtime_key_auth(app, key_id)?;
    authenticate_publickey_attempt(handle, config, app, &key_auth, fallback_error, otp_info)
        .await
        .map_err(AppError::from)
}

async fn authenticate_publickey_attempt(
    handle: &mut client::Handle<SshHandler>,
    config: &SshConfig,
    app: &AppHandle,
    auth: &SshAuth,
    fallback_error: &str,
    otp_info: Option<&OtpAutoFillInfo>,
) -> Result<Option<RuntimeKeyPassphrase>, SshAuthFailure> {
    let SshAuth::Key {
        key_id,
        key_data,
        cert_data,
        passphrase,
    } = auth
    else {
        return Err(SshAuthFailure {
            message: "No SSH key is configured".to_string(),
            remaining_methods: vec!["publickey".to_string()],
            partial_success: false,
        });
    };

    let (key, key_passphrase_secret) =
        decode_secret_key_with_runtime_prompt(key_data, passphrase.as_deref(), config, app)
            .await
            .map_err(|error| SshAuthFailure::from_error("Invalid SSH key passphrase", error))?;
    let hash_alg = handle
        .best_supported_rsa_hash()
        .await
        .ok()
        .flatten()
        .flatten();
    let cert = cert_data
        .as_deref()
        .map(russh::keys::Certificate::from_openssh)
        .transpose()
        .map_err(|error| SshAuthFailure {
            message: format!("Invalid OpenSSH certificate: {error}"),
            remaining_methods: vec!["publickey".to_string()],
            partial_success: false,
        })?;
    let cert_algorithm = cert.as_ref().map(|cert| cert.algorithm().to_string());

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
            "key_id": key_id,
            "key_algorithm": key.algorithm().to_string(),
            "certificate": cert.is_some(),
            "certificate_algorithm": cert_algorithm,
            "rsa_hash": format!("{hash_alg:?}"),
        })),
        None,
    );

    let authenticated = if let Some(cert) = cert {
        handle
            .authenticate_openssh_cert(&config.username, Arc::new(key), cert)
            .await
            .map_err(|error| SshAuthFailure {
                message: format!("Certificate auth failed: {error}"),
                remaining_methods: vec!["publickey".to_string()],
                partial_success: false,
            })?
    } else {
        handle
            .authenticate_publickey(
                &config.username,
                russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), hash_alg),
            )
            .await
            .map_err(|error| SshAuthFailure {
                message: format!("Key auth failed: {error}"),
                remaining_methods: vec!["publickey".to_string()],
                partial_success: false,
            })?
    };

    try_keyboard_interactive_after_partial(
        handle,
        &authenticated,
        &config.username,
        config.connection_id.as_deref(),
        &config.name,
        config.owner_window_label.as_deref(),
        app,
        fallback_error,
        None,
        otp_info,
    )
    .await?;

    Ok(key_id.as_ref().and_then(|id| {
        key_passphrase_secret.map(|secret| RuntimeKeyPassphrase {
            key_id: id.clone(),
            secret,
        })
    }))
}

enum AgentSigningSelectionError {
    Action(Result<SshAgentAuthAction, oneshot::error::RecvError>),
    Signing(AppError),
}

async fn await_agent_signing_or_action<T, F>(
    signing: F,
    action_rx: &mut oneshot::Receiver<SshAgentAuthAction>,
) -> Result<T, AgentSigningSelectionError>
where
    F: Future<Output = AppResult<T>>,
{
    tokio::select! {
        biased;
        action = &mut *action_rx => Err(AgentSigningSelectionError::Action(action)),
        result = signing => result.map_err(AgentSigningSelectionError::Signing),
    }
}

/// Try public-key authentication with identities provided by an external SSH Agent.
///
/// The Agent only signs; the server still decides whether to accept an identity.
/// Private key material, identity comments, and key contents never enter the app
/// or its logs.
async fn authenticate_agent(
    handle: &mut client::Handle<SshHandler>,
    config: &SshConfig,
    app: &AppHandle,
    fallback_error: &str,
    otp_info: Option<&OtpAutoFillInfo>,
) -> AppResult<()> {
    let (mut agent, identities, rsa_hashes) =
        prepare_agent_authentication(handle, config, app).await?;

    log_structured(
        StructuredLogLevel::Info,
        "ssh.auth",
        "auth.start",
        "Starting SSH Agent authentication",
        config.connection_id.as_deref(),
        None,
        Some(json!({
            "host": config.host,
            "port": config.port,
            "username": config.username,
            "auth_mode": "agent",
            "identity_count": identities.len(),
        })),
        None,
    );
    let mut last_failure: Option<SshAuthFailure> = None;

    for identity in identities {
        let public_key = identity.public_key();
        let hashes = if matches!(public_key.algorithm(), Algorithm::Rsa { .. }) {
            rsa_hashes.clone()
        } else {
            vec![None]
        };

        for hash_alg in hashes {
            let (request_id, mut action_rx, request_guard) =
                begin_agent_auth_request(app, config).await?;
            let signing = async {
                match &identity {
                    AgentIdentity::PublicKey { key, .. } => timeout(
                        AGENT_SIGN_TIMEOUT,
                        handle.authenticate_publickey_with(
                            &config.username,
                            key.clone(),
                            hash_alg,
                            &mut agent,
                        ),
                    )
                    .await
                    .map_err(|_| AppError::Auth("SSH Agent signing timed out".to_string()))
                    .and_then(|result| {
                        result.map_err(|error| {
                            AppError::Auth(format!("SSH Agent signing failed: {error}"))
                        })
                    }),
                    AgentIdentity::Certificate { certificate, .. } => timeout(
                        AGENT_SIGN_TIMEOUT,
                        handle.authenticate_certificate_with(
                            &config.username,
                            certificate.clone(),
                            hash_alg,
                            &mut agent,
                        ),
                    )
                    .await
                    .map_err(|_| AppError::Auth("SSH Agent signing timed out".to_string()))
                    .and_then(|result| {
                        result.map_err(|error| {
                            AppError::Auth(format!("SSH Agent signing failed: {error}"))
                        })
                    }),
                }
            };
            let result = match await_agent_signing_or_action(signing, &mut action_rx).await {
                Err(AgentSigningSelectionError::Action(action)) => {
                    // Release the local Agent stream before notifying the frontend so an immediate retry cannot reuse it.
                    drop(agent);
                    finish_agent_auth_request(app, &request_id, false).await;
                    request_guard.mark_resolved();
                    return Err(agent_auth_action_error(action));
                }
                Ok(result) => Ok(result),
                Err(AgentSigningSelectionError::Signing(error)) => Err(error),
            };

            let result = match result {
                Ok(result) => {
                    finish_agent_auth_request(app, &request_id, true).await;
                    request_guard.mark_resolved();
                    result
                }
                Err(error) => {
                    // The failed request waits for user action, so do not retain the old Agent stream.
                    drop(agent);
                    return agent_auth_failure_with_request(
                        app,
                        config,
                        request_id,
                        action_rx,
                        request_guard,
                        error.to_string(),
                    )
                    .await
                    .map(|_| ());
                }
            };

            match &result {
                client::AuthResult::Success => return Ok(()),
                client::AuthResult::Failure {
                    remaining_methods,
                    partial_success,
                } => {
                    if *partial_success {
                        drop(agent);
                        try_keyboard_interactive_after_partial(
                            handle,
                            &result,
                            &config.username,
                            config.connection_id.as_deref(),
                            &config.name,
                            config.owner_window_label.as_deref(),
                            app,
                            fallback_error,
                            None,
                            otp_info,
                        )
                        .await
                        .map_err(AppError::from)?;
                        return Ok(());
                    }

                    let auth_failure = SshAuthFailure::from_auth_result(
                        "SSH Agent identity rejected",
                        remaining_methods,
                        *partial_success,
                    );
                    if remaining_methods.contains(&MethodKind::PublicKey) {
                        last_failure = Some(auth_failure);
                        continue;
                    }
                    drop(agent);
                    return agent_auth_failure(app, config, auth_failure.message)
                        .await
                        .map(|_| ());
                }
            }
        }
    }

    drop(agent);
    agent_auth_failure(
        app,
        config,
        last_failure
            .map(|failure| failure.message)
            .unwrap_or_else(|| fallback_error.to_string()),
    )
    .await
    .map(|_| ())
}

/// Establish a cancellable pre-authentication phase for a single SSH Agent authentication.
///
/// The pre-authentication request is created before accessing the Agent, ensuring that when the Agent is blocked by the previous hardware challenge,
/// the current connection can still present an independent authentication status to the frontend and respond to cancellation operations.
async fn prepare_agent_authentication(
    handle: &mut client::Handle<SshHandler>,
    config: &SshConfig,
    app: &AppHandle,
) -> AppResult<(DynamicAgentClient, Vec<AgentIdentity>, Vec<Option<HashAlg>>)> {
    let (request_id, mut action_rx, request_guard) = begin_agent_auth_request(app, config).await?;
    let auth_agent_endpoint = config.auth_agent_endpoint.clone().unwrap_or_default();

    let mut agent = match tokio::select! {
        biased;
        action = &mut action_rx => {
            finish_agent_auth_request(app, &request_id, false).await;
            request_guard.mark_resolved();
            return Err(agent_auth_action_error(action));
        }
        result = connect_agent_client(&auth_agent_endpoint) => result,
    } {
        Ok(agent) => agent,
        Err(error) => {
            return Err(agent_auth_failure_error(
                app,
                config,
                request_id,
                action_rx,
                request_guard,
                error.to_string(),
            )
            .await);
        }
    };

    let identities = match tokio::select! {
        biased;
        action = &mut action_rx => {
            drop(agent);
            finish_agent_auth_request(app, &request_id, false).await;
            request_guard.mark_resolved();
            return Err(agent_auth_action_error(action));
        }
        result = timeout(AGENT_IDENTITY_TIMEOUT, agent.request_identities()) => result,
    } {
        Ok(Ok(identities)) => identities,
        Ok(Err(error)) => {
            drop(agent);
            return Err(agent_auth_failure_error(
                app,
                config,
                request_id,
                action_rx,
                request_guard,
                format!("SSH Agent identity request failed: {error}"),
            )
            .await);
        }
        Err(_) => {
            drop(agent);
            return Err(agent_auth_failure_error(
                app,
                config,
                request_id,
                action_rx,
                request_guard,
                "SSH Agent identity request timed out".to_string(),
            )
            .await);
        }
    };

    if identities.is_empty() {
        drop(agent);
        return Err(agent_auth_failure_error(
            app,
            config,
            request_id,
            action_rx,
            request_guard,
            "SSH Agent has no identities".to_string(),
        )
        .await);
    }

    let rsa_hashes = match tokio::select! {
        biased;
        action = &mut action_rx => {
            drop(agent);
            finish_agent_auth_request(app, &request_id, false).await;
            request_guard.mark_resolved();
            return Err(agent_auth_action_error(action));
        }
        result = handle.best_supported_rsa_hash() => result,
    } {
        Ok(Some(Some(hash))) => vec![Some(hash)],
        Ok(Some(None)) => vec![None],
        Ok(None) => vec![Some(HashAlg::Sha512), Some(HashAlg::Sha256), None],
        Err(error) => {
            drop(agent);
            return Err(agent_auth_failure_error(
                app,
                config,
                request_id,
                action_rx,
                request_guard,
                format!("Unable to determine SSH Agent RSA hash algorithm: {error}"),
            )
            .await);
        }
    };

    finish_agent_auth_request(app, &request_id, true).await;
    request_guard.mark_resolved();
    Ok((agent, identities, rsa_hashes))
}

fn agent_auth_action_error(
    action: Result<SshAgentAuthAction, oneshot::error::RecvError>,
) -> AppError {
    match action {
        Ok(SshAgentAuthAction::Retry) => AppError::Auth(SSH_AGENT_AUTH_RETRY.to_string()),
        Ok(SshAgentAuthAction::Cancel) | Err(_) => {
            AppError::Cancelled("SSH Agent authentication cancelled".to_string())
        }
    }
}

async fn agent_auth_failure_error(
    app: &AppHandle,
    config: &SshConfig,
    request_id: String,
    action_rx: oneshot::Receiver<SshAgentAuthAction>,
    request_guard: AgentAuthRequestGuard,
    error: String,
) -> AppError {
    match agent_auth_failure_with_request(app, config, request_id, action_rx, request_guard, error)
        .await
    {
        Err(error) => error,
        Ok(()) => AppError::Auth(
            "SSH Agent failure approval unexpectedly completed successfully".to_string(),
        ),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SshAgentAuthEvent {
    request_id: String,
    connection_name: String,
    username: String,
    endpoint: String,
    state: String,
    error: Option<String>,
    target_window_label: Option<String>,
}

fn agent_endpoint_name(endpoint: &crate::config::SshAgentEndpoint) -> &'static str {
    match endpoint {
        crate::config::SshAgentEndpoint::Auto => "auto",
        crate::config::SshAgentEndpoint::Environment { .. } => "environment",
        crate::config::SshAgentEndpoint::UnixSocket { .. } => "unix_socket",
        crate::config::SshAgentEndpoint::Pageant => "pageant",
        crate::config::SshAgentEndpoint::WindowsOpenSsh => "windows_open_ssh",
    }
}

fn configured_agent_endpoint_name(config: &SshConfig) -> &'static str {
    config
        .auth_agent_endpoint
        .as_ref()
        .map(agent_endpoint_name)
        .unwrap_or("auto")
}

async fn begin_agent_auth_request(
    app: &AppHandle,
    config: &SshConfig,
) -> AppResult<(
    String,
    oneshot::Receiver<SshAgentAuthAction>,
    AgentAuthRequestGuard,
)> {
    let request_id = uuid::Uuid::new_v4().to_string();
    let manager = app
        .try_state::<Arc<PendingSshAgentAuthManager>>()
        .ok_or_else(|| AppError::Auth("Pending SSH Agent manager is unavailable".to_string()))?;
    let receiver = manager.register(request_id.clone()).await;
    let _ = app.emit(
        "ssh-agent-auth-pending",
        SshAgentAuthEvent {
            request_id: request_id.clone(),
            connection_name: config.name.clone(),
            username: config.username.clone(),
            endpoint: configured_agent_endpoint_name(config).to_string(),
            state: "pending".to_string(),
            error: None,
            target_window_label: config.owner_window_label.clone(),
        },
    );
    Ok((
        request_id.clone(),
        receiver,
        AgentAuthRequestGuard::new(app.clone(), request_id),
    ))
}

struct AgentAuthRequestGuard {
    app: AppHandle,
    request_id: String,
    resolved: Arc<AtomicBool>,
}

impl AgentAuthRequestGuard {
    fn new(app: AppHandle, request_id: String) -> Self {
        Self {
            app,
            request_id,
            resolved: Arc::new(AtomicBool::new(false)),
        }
    }

    fn mark_resolved(&self) {
        self.resolved.store(true, Ordering::Release);
    }
}

impl Drop for AgentAuthRequestGuard {
    fn drop(&mut self) {
        if self.resolved.swap(true, Ordering::AcqRel) {
            return;
        }

        let app = self.app.clone();
        let request_id = self.request_id.clone();
        tauri::async_runtime::spawn(async move {
            finish_agent_auth_request(&app, &request_id, false).await;
        });
    }
}

async fn finish_agent_auth_request(app: &AppHandle, request_id: &str, resolved: bool) {
    if let Some(manager) = app.try_state::<Arc<PendingSshAgentAuthManager>>() {
        manager.finish(request_id).await;
    }
    let _ = app.emit(
        "ssh-agent-auth-resolved",
        serde_json::json!({ "requestId": request_id, "resolved": resolved }),
    );
}

async fn finish_security_prompt_request(
    app: &AppHandle,
    request_id: &str,
    manager_kind: SecurityPromptManager,
) {
    match manager_kind {
        SecurityPromptManager::Otp => {
            if let Some(manager) = app.try_state::<Arc<PendingAuthManager>>() {
                manager.finish(request_id).await;
            }
        }
        SecurityPromptManager::SshAuth => {
            if let Some(manager) = app.try_state::<Arc<PendingSshAuthManager>>() {
                manager.finish(request_id).await;
            }
        }
    }
    let _ = app.emit(
        "security-prompt-resolved",
        serde_json::json!({ "requestId": request_id }),
    );
}

async fn agent_auth_failure(app: &AppHandle, config: &SshConfig, error: String) -> AppResult<()> {
    let (request_id, receiver, request_guard) = begin_agent_auth_request(app, config).await?;
    agent_auth_failure_with_request(app, config, request_id, receiver, request_guard, error).await
}

async fn agent_auth_failure_with_request(
    app: &AppHandle,
    config: &SshConfig,
    request_id: String,
    receiver: oneshot::Receiver<SshAgentAuthAction>,
    request_guard: AgentAuthRequestGuard,
    error: String,
) -> AppResult<()> {
    let _ = app.emit(
        "ssh-agent-auth-failed",
        SshAgentAuthEvent {
            request_id: request_id.clone(),
            connection_name: config.name.clone(),
            username: config.username.clone(),
            endpoint: configured_agent_endpoint_name(config).to_string(),
            state: "failed".to_string(),
            error: Some(error),
            target_window_label: config.owner_window_label.clone(),
        },
    );
    let result = match timeout(AGENT_APPROVAL_TIMEOUT, receiver).await {
        Ok(Ok(SshAgentAuthAction::Retry)) => Err(AppError::Auth(SSH_AGENT_AUTH_RETRY.to_string())),
        Ok(Ok(SshAgentAuthAction::Cancel) | Err(_)) => Err(AppError::Cancelled(
            "SSH Agent authentication cancelled".to_string(),
        )),
        Err(_) => Err(AppError::Auth(
            "SSH Agent authentication approval timed out".to_string(),
        )),
    };
    finish_agent_auth_request(app, &request_id, false).await;
    request_guard.mark_resolved();
    result
}

fn persist_runtime_auth_updates(
    app: &AppHandle,
    config: &SshConfig,
    updates: &SshRuntimeAuthUpdates,
) -> AppResult<()> {
    if let Some(secret) = &updates.password {
        persist_runtime_password(app, config, secret)?;
    }
    if let Some(secret) = &updates.key_passphrase {
        persist_runtime_key_passphrase(app, secret)?;
    }
    Ok(())
}

fn persist_runtime_password(
    app: &AppHandle,
    config: &SshConfig,
    secret: &RuntimeSecret,
) -> AppResult<()> {
    let Some(save) = &secret.save else {
        return Ok(());
    };
    let Some(connection_id) = config.connection_id.as_deref() else {
        return Ok(());
    };

    match save {
        RuntimeSecretSave::ConnectionInline => {
            let mut sessions = crate::config::load_config(app)?;
            if let Some(conn) = sessions
                .connections
                .iter_mut()
                .find(|candidate| candidate.id == connection_id)
            {
                let auth = conn.auth.get_or_insert_with(Default::default);
                auth.mode = "password".to_string();
                auth.password = Some(crate::utils::crypto::encrypt(&secret.value)?);
                auth.password_id = None;
                auth.has_password = false;
                crate::config::save_config(app, &sessions)?;
                emit_config_changed(app);
            }
        }
        RuntimeSecretSave::SavedPassword { id, name } => {
            let password_id =
                upsert_runtime_saved_password(app, id.as_deref(), name, &secret.value)?;
            let mut sessions = crate::config::load_config(app)?;
            if let Some(conn) = sessions
                .connections
                .iter_mut()
                .find(|candidate| candidate.id == connection_id)
            {
                let auth = conn.auth.get_or_insert_with(Default::default);
                auth.mode = "password".to_string();
                auth.password_id = Some(password_id);
                auth.password = None;
                auth.has_password = false;
                crate::config::save_config(app, &sessions)?;
                emit_config_changed(app);
            }
        }
        RuntimeSecretSave::SavedPasswordReference { id } => {
            let mut sessions = crate::config::load_config(app)?;
            if let Some(conn) = sessions
                .connections
                .iter_mut()
                .find(|candidate| candidate.id == connection_id)
            {
                let auth = conn.auth.get_or_insert_with(Default::default);
                auth.mode = "password".to_string();
                auth.password_id = Some(id.clone());
                auth.password = None;
                auth.has_password = false;
                crate::config::save_config(app, &sessions)?;
                emit_config_changed(app);
            }
        }
        RuntimeSecretSave::KeyPassphrase => {}
    }

    Ok(())
}

fn upsert_runtime_saved_password(
    app: &AppHandle,
    id: Option<&str>,
    name: &str,
    value: &str,
) -> AppResult<String> {
    let mut cfg = crate::config::load_passwords(app)?;
    let target_id = id
        .filter(|id| cfg.passwords.iter().any(|password| password.id == *id))
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let entry_name = cfg
        .passwords
        .iter()
        .find(|password| password.id == target_id)
        .map(|password| password.name.clone())
        .unwrap_or_else(|| name.to_string());
    let entry = crate::config::SavedPassword {
        id: target_id.clone(),
        name: entry_name,
        password: Some(crate::utils::crypto::encrypt(value)?),
        has_password: false,
    };
    if let Some(existing) = cfg
        .passwords
        .iter_mut()
        .find(|password| password.id == target_id)
    {
        *existing = entry;
    } else {
        cfg.passwords.push(entry);
    }
    crate::config::save_passwords(app, &cfg)?;
    Ok(target_id)
}

fn persist_runtime_key_passphrase(
    app: &AppHandle,
    key_secret: &RuntimeKeyPassphrase,
) -> AppResult<()> {
    if !matches!(
        key_secret.secret.save,
        Some(RuntimeSecretSave::KeyPassphrase)
    ) {
        return Ok(());
    }

    let mut cfg = crate::config::load_keys(app)?;
    if let Some(key) = cfg
        .keys
        .iter_mut()
        .find(|candidate| candidate.id == key_secret.key_id)
    {
        key.passphrase = Some(crate::utils::crypto::encrypt(&key_secret.secret.value)?);
        key.has_key_data = false;
        key.has_cert_data = false;
        crate::config::save_keys(app, &cfg)?;
        emit_config_changed(app);
    }
    Ok(())
}

fn emit_config_changed(app: &AppHandle) {
    let _ = app.emit("connections-changed", ());
    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        crate::core::cloud_sync::notify_config_changed(&app_handle).await;
    });
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

const MAX_KEYBOARD_INTERACTIVE_RESTARTS: u32 = 8;

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
            "auth_round": 0,
            "partial_success_restart": false,
            "response_kind": "start",
        })),
        None,
    );

    let mut step = handle
        .authenticate_keyboard_interactive_start(username, None)
        .await
        .map_err(|error| AppError::Auth(format!("Keyboard-interactive start failed: {}", error)))?;
    let mut pending_totp_use: Option<TotpUseCandidate> = None;
    let mut round: u32 = 0;
    let mut restart_count: u32 = 0;

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
                        "auth_round": round,
                        "response_kind": "success",
                    })),
                    None,
                );
                return Ok(());
            }
            KeyboardInteractiveAuthResponse::Failure {
                remaining_methods,
                partial_success,
            } => {
                let keyboard_interactive_available =
                    remaining_methods.contains(&MethodKind::KeyboardInteractive);
                if partial_success && keyboard_interactive_available {
                    if let Some(candidate) = pending_totp_use.take() {
                        record_totp_code_use(candidate);
                    }

                    restart_count = restart_count.saturating_add(1);
                    if restart_count <= MAX_KEYBOARD_INTERACTIVE_RESTARTS {
                        log_structured(
                            StructuredLogLevel::Info,
                            "ssh.auth",
                            "keyboard_interactive.partial_success",
                            "Keyboard-interactive partial success, restarting keyboard-interactive authentication",
                            connection_id,
                            None,
                            Some(json!({
                                "username": username,
                                "mode": mode.label(),
                                "remaining_methods": format!("{remaining_methods:?}"),
                                "partial_success": partial_success,
                                "partial_success_restart": true,
                                "auth_round": round,
                                "restart_count": restart_count,
                                "max_restarts": MAX_KEYBOARD_INTERACTIVE_RESTARTS,
                                "response_kind": "failure",
                            })),
                            None,
                        );

                        log_structured(
                            StructuredLogLevel::Info,
                            "ssh.auth",
                            "keyboard_interactive.start",
                            "Restarting keyboard-interactive authentication after partial success",
                            connection_id,
                            None,
                            Some(json!({
                                "username": username,
                                "mode": mode.label(),
                                "auth_round": round,
                                "restart_count": restart_count,
                                "partial_success_restart": true,
                                "response_kind": "start",
                            })),
                            None,
                        );

                        step = handle
                            .authenticate_keyboard_interactive_start(username, None)
                            .await
                            .map_err(|error| {
                                AppError::Auth(format!(
                                    "Keyboard-interactive restart failed: {}",
                                    error
                                ))
                            })?;
                        continue;
                    }
                }

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
                        "partial_success_restart": partial_success && keyboard_interactive_available,
                        "auth_round": round,
                        "restart_count": restart_count,
                        "max_restarts": MAX_KEYBOARD_INTERACTIVE_RESTARTS,
                        "response_kind": "failure",
                    })),
                    None,
                );
                return Err(AppError::Auth(
                    "Keyboard-interactive authentication failed".to_string(),
                ));
            }
            KeyboardInteractiveAuthResponse::InfoRequest {
                name,
                instructions,
                prompts,
            } => {
                round = round.saturating_add(1);
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
                        "round": round,
                        "auth_round": round,
                        "response_kind": "info_request",
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
                } else if let Some(info) =
                    otp_info.filter(|i| i.auto_fill && should_auto_fill_otp_prompts(&prompts))
                {
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
                            "round": round,
                            "auth_round": round,
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
                    let request_guard = SecurityPromptRequestGuard::new(
                        app.clone(),
                        request_id.clone(),
                        SecurityPromptManager::Otp,
                    );

                    let payload = OtpRequestPayload {
                        request_id: request_id.clone(),
                        connection_name: connection_name.to_string(),
                        name: normalize_optional_keyboard_interactive_text(&name),
                        instructions: normalize_optional_keyboard_interactive_text(&instructions),
                        round,
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
                            "round": payload.round,
                            "auth_round": round,
                            "response_kind": "info_request",
                        })),
                        None,
                    );
                    let _ = app.emit("otp-request", &payload);

                    let responses = match timeout(INTERACTIVE_AUTH_TIMEOUT, rx).await {
                        Ok(Ok(Some(responses))) => Ok(responses),
                        Ok(Ok(None)) => Err(AppError::Auth(
                            "2FA authentication cancelled by user".to_string(),
                        )),
                        Ok(Err(_)) => Err(AppError::Auth(
                            "2FA authentication request dropped".to_string(),
                        )),
                        Err(_) => Err(AppError::Auth(
                            "2FA authentication request timed out".to_string(),
                        )),
                    };
                    finish_security_prompt_request(app, &request_id, SecurityPromptManager::Otp)
                        .await;
                    request_guard.mark_resolved();
                    let responses = responses?;
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
) -> Result<(), SshAuthFailure> {
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
                .map_err(|error| {
                    SshAuthFailure::from_auth_result(
                        format!("{fallback_error}: {error}"),
                        remaining_methods,
                        *partial_success,
                    )
                })
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
                    return Err(SshAuthFailure::from_auth_result(
                        fallback_error,
                        remaining_methods,
                        *partial_success,
                    ));
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
                .map_err(|error| {
                    SshAuthFailure::from_auth_result(
                        format!("{fallback_error}: {error}"),
                        remaining_methods,
                        *partial_success,
                    )
                })
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
                Err(SshAuthFailure::from_auth_result(
                    fallback_error,
                    remaining_methods,
                    *partial_success,
                ))
            }
        }
    }
}

fn should_auto_fill_password_prompts(prompts: &[client::Prompt]) -> bool {
    prompts.len() == 1
        && !prompts[0].echo
        && is_password_keyboard_interactive_prompt(&prompts[0].prompt)
}

fn should_auto_fill_otp_prompts(prompts: &[client::Prompt]) -> bool {
    prompts.len() == 1 && is_otp_keyboard_interactive_prompt(&prompts[0].prompt)
}

fn is_otp_keyboard_interactive_prompt(prompt: &str) -> bool {
    let normalized = prompt.to_lowercase();

    let selection_markers = [
        "select",
        "choose",
        "choice",
        "option",
        "method",
        "delivery",
        "send to",
        "send via",
        "push",
        "sms/email",
        "sms or email",
        "email or sms",
        "选择",
        "请选择",
        "选项",
        "方式",
        "方法",
        "发送到",
        "发送至",
    ];
    if selection_markers
        .iter()
        .any(|marker| normalized.contains(marker))
    {
        return false;
    }

    [
        "otp",
        "totp",
        "hotp",
        "2fa",
        "mfa",
        "one-time",
        "one time",
        "verification code",
        "authentication code",
        "auth code",
        "authenticator",
        "passcode",
        "token",
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
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
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

fn normalize_optional_keyboard_interactive_text(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        AgentSigningSelectionError, KeyboardInteractiveMode, PendingAuthManager,
        PendingSshAgentAuthManager, PendingSshAuthManager, SshAgentAuthAction, TotpUseCandidate,
        await_agent_signing_or_action, is_otp_keyboard_interactive_prompt,
        is_password_keyboard_interactive_prompt, is_totp_code_reused, record_totp_code_use,
        resolve_password_material, seconds_until_next_totp_step, should_auto_fill_otp_prompts,
        should_auto_fill_password_prompts, used_totp_codes,
    };
    use crate::config::ConnectionAuth;
    use crate::error::AppError;
    use russh::client::Prompt;
    use tokio::sync::oneshot;

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
    fn auto_fills_single_otp_keyboard_interactive_prompt() {
        let prompts = vec![Prompt {
            prompt: "Verification code: ".to_string(),
            echo: false,
        }];

        assert!(should_auto_fill_otp_prompts(&prompts));
        assert!(is_otp_keyboard_interactive_prompt("验证码: "));
    }

    #[test]
    fn does_not_auto_fill_mfa_selection_prompts() {
        for prompt in ["Select method:", "Option:", "1) SMS 2) Email"] {
            let prompts = vec![Prompt {
                prompt: prompt.to_string(),
                echo: true,
            }];

            assert!(!should_auto_fill_otp_prompts(&prompts));
        }
    }

    #[test]
    fn password_prompt_does_not_trigger_otp_autofill() {
        let prompts = vec![Prompt {
            prompt: "Password: ".to_string(),
            echo: false,
        }];

        assert!(should_auto_fill_password_prompts(&prompts));
        assert!(!should_auto_fill_otp_prompts(&prompts));
    }

    #[test]
    fn does_not_auto_fill_otp_for_multiple_prompts() {
        let prompts = vec![
            Prompt {
                prompt: "Verification code: ".to_string(),
                echo: false,
            },
            Prompt {
                prompt: "Backup code: ".to_string(),
                echo: false,
            },
        ];

        assert!(!should_auto_fill_otp_prompts(&prompts));
    }

    #[test]
    fn additional_factor_mode_never_exposes_password_fallback() {
        assert!(
            KeyboardInteractiveMode::AdditionalFactor
                .password()
                .is_none()
        );
    }

    #[test]
    fn password_mode_without_material_returns_recoverable_error() {
        let auth = ConnectionAuth {
            mode: "password".to_string(),
            ..Default::default()
        };

        let password = resolve_password_material(None, &auth).unwrap();

        assert_eq!(password, None);
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

    #[tokio::test]
    async fn agent_action_wins_when_signing_result_is_ready() {
        let (action_tx, mut action_rx) = oneshot::channel();
        action_tx.send(SshAgentAuthAction::Cancel).unwrap();

        let result =
            await_agent_signing_or_action(async { Ok::<_, AppError>(()) }, &mut action_rx).await;

        assert!(matches!(
            result,
            Err(AgentSigningSelectionError::Action(Ok(
                SshAgentAuthAction::Cancel
            )))
        ));
    }

    #[tokio::test]
    async fn finishing_pending_interactive_requests_removes_their_senders() {
        let otp_manager = PendingAuthManager::new();
        let otp_receiver = otp_manager.register("otp-request".to_string()).await;
        otp_manager.finish("otp-request").await;
        assert!(otp_receiver.await.is_err());
        assert!(!otp_manager.respond("otp-request", None).await);

        let ssh_manager = PendingSshAuthManager::new();
        let ssh_receiver = ssh_manager.register("ssh-request".to_string()).await;
        ssh_manager.finish("ssh-request").await;
        assert!(ssh_receiver.await.is_err());
        assert!(!ssh_manager.respond("ssh-request", None).await);
    }

    #[tokio::test]
    async fn ssh_agent_prompts_are_isolated_by_request_id() {
        let manager = PendingSshAgentAuthManager::new();
        let first = manager.register("first".to_string()).await;
        let second = manager.register("second".to_string()).await;

        assert!(manager.respond("second", SshAgentAuthAction::Retry).await);
        assert!(manager.respond("first", SshAgentAuthAction::Cancel).await);

        assert!(matches!(second.await, Ok(SshAgentAuthAction::Retry)));
        assert!(matches!(first.await, Ok(SshAgentAuthAction::Cancel)));
        assert!(!manager.respond("first", SshAgentAuthAction::Retry).await);

        // Retry creates a fresh pending request instead of reusing a completed waiter.
        let retried = manager.register("second".to_string()).await;
        assert!(manager.respond("second", SshAgentAuthAction::Retry).await);
        assert!(matches!(retried.await, Ok(SshAgentAuthAction::Retry)));
    }
}
