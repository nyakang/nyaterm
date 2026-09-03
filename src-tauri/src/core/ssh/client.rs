use super::agent::connect_agent_stream;
use super::agent_broker::{AgentBrokerFactory, try_acquire_agent_channel_permit};
use crate::config::{
    SftpSettings, SshAgentEndpoint, SshAgentForwardingConfig, SshAlgorithmMode,
    SshAlgorithmPreferences, SshProfile, SshRuntimeMode, SshTerminalType,
};
use crate::error::{AppError, AppResult};
use russh::client;
use russh::keys::{Algorithm, EcdsaCurve, HashAlg, PublicKeyBase64};
use russh::{Preferred, cipher, kex, mac};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::fmt;
use std::process::Stdio;
use std::str::FromStr;
use std::sync::{Arc, Mutex as StdMutex};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, mpsc, oneshot};

/// Connection parameters for SSH (host, port, user, auth method).
#[derive(Debug, Clone, Deserialize)]
pub struct SshConfig {
    #[serde(default)]
    pub connection_id: Option<String>,
    #[serde(default)]
    pub owner_window_label: Option<String>,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: SshAuth,
    #[serde(default)]
    pub backspace_mode: String,
    #[serde(default)]
    pub x11_forwarding: bool,
    #[serde(default)]
    pub x11_display: String,
    #[serde(default)]
    pub auth_agent_endpoint: Option<SshAgentEndpoint>,
    #[serde(default)]
    pub agent_forwarding_config: SshAgentForwardingConfig,
    #[serde(default)]
    pub proxy: Option<crate::config::ProxySettings>,
    #[serde(default)]
    pub proxy_jump: Option<Box<SshConfig>>,
    #[serde(default)]
    pub post_login: Option<SshPostLoginConfig>,
    #[serde(default)]
    pub ssh_algorithms: Option<SshAlgorithmPreferences>,
    #[serde(default)]
    pub ssh_profile: SshProfile,
    #[serde(default)]
    pub runtime_mode: SshRuntimeMode,
    #[serde(default)]
    pub terminal_type: SshTerminalType,
    #[serde(default)]
    pub sftp: SftpSettings,
    /// Character encoding for terminal I/O (e.g. "UTF-8", "GBK").
    #[serde(default = "default_encoding")]
    pub encoding: String,
    /// Allow sanitized remote OSC 0/2 titles to decorate this saved SSH tab.
    #[serde(default)]
    pub dynamic_tab_title: bool,
}

fn default_encoding() -> String {
    "UTF-8".to_string()
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AlgorithmRisk {
    Modern,
    Legacy,
    Insecure,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AlgorithmOption {
    pub id: String,
    pub label: String,
    pub risk: AlgorithmRisk,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SshAlgorithmDefaults {
    pub kex: Vec<String>,
    pub ciphers: Vec<String>,
    pub macs: Vec<String>,
    pub host_keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SupportedSshAlgorithms {
    pub kex: Vec<AlgorithmOption>,
    pub ciphers: Vec<AlgorithmOption>,
    pub macs: Vec<AlgorithmOption>,
    pub host_keys: Vec<AlgorithmOption>,
    pub compatible: SshAlgorithmDefaults,
    pub secure: SshAlgorithmDefaults,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SshPostLoginConfig {
    pub command: String,
    pub delay_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SshStartupCommand {
    pub command: String,
    pub delay_ms: u64,
}

/// Authentication method: none, password, private key, or SSH Agent.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum SshAuth {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "password")]
    Password { password: Option<String> },
    #[serde(rename = "agent")]
    Agent,
    #[serde(rename = "key")]
    Key {
        #[serde(default)]
        key_id: Option<String>,
        key_data: String,
        #[serde(default)]
        cert_data: Option<String>,
        passphrase: Option<String>,
    },
}

pub(crate) type SshRawHandle = Arc<Mutex<client::Handle<SshHandler>>>;

pub struct RemoteForwardOpen {
    pub channel: russh::Channel<client::Msg>,
    pub connected_address: String,
    pub connected_port: u32,
    pub originator_address: String,
    pub originator_port: u32,
}

const DEFAULT_SFTP_CHANNEL_LIMIT: usize = 6;

#[derive(Clone)]
pub(crate) struct SftpChannelLimiter {
    semaphore: Arc<Semaphore>,
}

impl SftpChannelLimiter {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(limit.max(1))),
        }
    }

    pub(crate) async fn acquire(&self) -> AppResult<OwnedSemaphorePermit> {
        self.semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| AppError::Channel("SFTP channel limiter is closed".to_string()))
    }

    #[cfg(test)]
    fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }
}

pub struct SshConnectionHandles {
    target: SshRawHandle,
    jumps: Vec<SshRawHandle>,
    sftp_channel_limiter: SftpChannelLimiter,
}

impl SshConnectionHandles {
    pub fn new(target: SshRawHandle, jumps: Vec<SshRawHandle>) -> Self {
        Self {
            target,
            jumps,
            sftp_channel_limiter: SftpChannelLimiter::new(DEFAULT_SFTP_CHANNEL_LIMIT),
        }
    }

    pub fn target_handle(&self) -> SshRawHandle {
        self.target.clone()
    }

    pub(crate) async fn acquire_sftp_channel_permit(&self) -> AppResult<OwnedSemaphorePermit> {
        self.sftp_channel_limiter.acquire().await
    }

    #[allow(dead_code)]
    pub fn jump_handle(&self) -> Option<SshRawHandle> {
        self.jumps.last().cloned()
    }

    #[allow(dead_code)]
    pub fn jump_handles(&self) -> Vec<SshRawHandle> {
        self.jumps.clone()
    }
}

pub(crate) type SshHandle = Arc<SshConnectionHandles>;

/// Manages pending host-key verification prompts awaiting user input from the frontend.
pub struct HostKeyVerifyManager {
    pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
}

impl HostKeyVerifyManager {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub async fn register(&self, request_id: String) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id, tx);
        rx
    }

    pub async fn respond(&self, request_id: &str, accepted: bool) -> bool {
        if let Some(tx) = self.pending.lock().await.remove(request_id) {
            tx.send(accepted).is_ok()
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HostKeyVerifyPayload {
    request_id: String,
    host: String,
    port: u16,
    key_type: String,
    fingerprint: String,
    is_key_changed: bool,
    target_window_label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshDiagnosticStage {
    Authenticating,
    Authenticated,
    OpeningInteractiveChannel,
    RequestingPty,
    RequestingShell,
    DetectingShell,
    PreparingIntegration,
    IoRunning,
}

impl fmt::Display for SshDiagnosticStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Authenticating => f.write_str("Authenticating"),
            Self::Authenticated => f.write_str("Authenticated"),
            Self::OpeningInteractiveChannel => f.write_str("OpeningInteractiveChannel"),
            Self::RequestingPty => f.write_str("RequestingPty"),
            Self::RequestingShell => f.write_str("RequestingShell"),
            Self::DetectingShell => f.write_str("DetectingShell"),
            Self::PreparingIntegration => f.write_str("PreparingIntegration"),
            Self::IoRunning => f.write_str("IoRunning"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SshDiagnosticContext {
    state: Arc<StdMutex<SshDiagnosticState>>,
}

#[derive(Debug)]
struct SshDiagnosticState {
    session_id: Option<String>,
    stage: SshDiagnosticStage,
}

impl SshDiagnosticContext {
    pub fn new(session_id: Option<String>) -> Self {
        Self {
            state: Arc::new(StdMutex::new(SshDiagnosticState {
                session_id,
                stage: SshDiagnosticStage::Authenticating,
            })),
        }
    }

    pub fn set_stage(&self, stage: SshDiagnosticStage) {
        if let Ok(mut state) = self.state.lock() {
            state.stage = stage;
        }
    }

    pub fn snapshot(&self) -> (Option<String>, SshDiagnosticStage) {
        self.state
            .lock()
            .map(|state| (state.session_id.clone(), state.stage))
            .unwrap_or((None, SshDiagnosticStage::Authenticating))
    }
}

/// russh client handler; performs TOFU known_hosts verification.
pub struct SshHandler {
    app: AppHandle,
    host: String,
    port: u16,
    owner_window_label: Option<String>,
    x11_tx: Option<mpsc::UnboundedSender<super::x11_forwarding::X11ChannelOpen>>,
    disconnect_tx: Option<mpsc::UnboundedSender<String>>,
    remote_forward_tx: Option<mpsc::UnboundedSender<RemoteForwardOpen>>,
    agent_forwarding_endpoint: Option<SshAgentEndpoint>,
    agent_broker: Option<Arc<AgentBrokerFactory>>,
    diagnostics: Option<SshDiagnosticContext>,
}

impl SshHandler {
    pub fn new(
        app: AppHandle,
        host: String,
        port: u16,
        owner_window_label: Option<String>,
    ) -> Self {
        Self {
            app,
            host,
            port,
            owner_window_label,
            x11_tx: None,
            disconnect_tx: None,
            remote_forward_tx: None,
            agent_forwarding_endpoint: None,
            agent_broker: None,
            diagnostics: None,
        }
    }

    pub fn with_x11_sender(
        mut self,
        x11_tx: mpsc::UnboundedSender<super::x11_forwarding::X11ChannelOpen>,
    ) -> Self {
        self.x11_tx = Some(x11_tx);
        self
    }

    pub fn with_disconnect_sender(mut self, disconnect_tx: mpsc::UnboundedSender<String>) -> Self {
        self.disconnect_tx = Some(disconnect_tx);
        self
    }

    pub fn with_remote_forward_sender(
        mut self,
        remote_forward_tx: mpsc::UnboundedSender<RemoteForwardOpen>,
    ) -> Self {
        self.remote_forward_tx = Some(remote_forward_tx);
        self
    }

    /// Attaches the Agent forwarding broker for the final target session.
    pub fn with_agent_broker(mut self, broker: Arc<AgentBrokerFactory>) -> Self {
        self.agent_broker = Some(broker);
        self
    }

    /// Preserves raw protocol relay for external-Agent-only forwarding with the all policy.
    pub fn with_agent_forwarding_endpoint(mut self, endpoint: SshAgentEndpoint) -> Self {
        self.agent_forwarding_endpoint = Some(endpoint);
        self
    }

    pub fn with_diagnostics(mut self, diagnostics: SshDiagnosticContext) -> Self {
        self.diagnostics = Some(diagnostics);
        self
    }

    fn append_known_host(&self, host_entry: &str) {
        if let Err(error) = crate::storage::upsert_known_host(host_entry) {
            tracing::warn!(
                host = %self.host,
                port = self.port,
                %error,
                "Failed to persist SSH host key to known_hosts"
            );
            let _ = self.app.emit(
                "ssh-error",
                format!("Failed to save known_hosts: {}", error),
            );
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KnownHostCheck {
    Match,
    HostSeen,
    UnknownHost,
}

#[cfg(test)]
fn check_known_host_entry(
    content: &str,
    host_identifier: &str,
    key_type: &str,
    key_base64: &str,
) -> KnownHostCheck {
    let mut host_seen = false;

    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 || parts[0] != host_identifier {
            continue;
        }

        host_seen = true;
        if parts[1] == key_type && parts[2] == key_base64 {
            return KnownHostCheck::Match;
        }
    }

    if host_seen {
        KnownHostCheck::HostSeen
    } else {
        KnownHostCheck::UnknownHost
    }
}

fn replace_known_host_entry(host_identifier: &str, new_entry: &str) -> AppResult<()> {
    crate::storage::replace_known_host_for_host(host_identifier, new_entry)
}

fn compatible_algorithms() -> Preferred {
    let mut preferred = Preferred::default();

    preferred.kex = Cow::Owned(vec![
        kex::MLKEM768X25519_SHA256,
        kex::CURVE25519,
        kex::CURVE25519_PRE_RFC_8731,
        kex::ECDH_SHA2_NISTP256,
        kex::ECDH_SHA2_NISTP384,
        kex::ECDH_SHA2_NISTP521,
        kex::DH_G18_SHA512,
        kex::DH_G17_SHA512,
        kex::DH_G16_SHA512,
        kex::DH_G15_SHA512,
        kex::DH_G14_SHA256,
        kex::DH_GEX_SHA256,
        kex::DH_G14_SHA1,
        kex::DH_GEX_SHA1,
        kex::DH_G1_SHA1,
        kex::EXTENSION_SUPPORT_AS_CLIENT,
        kex::EXTENSION_SUPPORT_AS_SERVER,
        kex::EXTENSION_OPENSSH_STRICT_KEX_AS_CLIENT,
        kex::EXTENSION_OPENSSH_STRICT_KEX_AS_SERVER,
    ]);

    preferred.key = Cow::Owned(vec![
        Algorithm::Ed25519,
        Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP256,
        },
        Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP384,
        },
        Algorithm::Rsa {
            hash: Some(HashAlg::Sha512),
        },
        Algorithm::Rsa {
            hash: Some(HashAlg::Sha256),
        },
        // Some network devices advertise malformed P-521 ECDSA host keys; prefer
        // plain ssh-rsa first so negotiation can reach authentication.
        Algorithm::Rsa { hash: None },
        Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP521,
        },
        Algorithm::Dsa,
    ]);

    preferred.cipher = Cow::Owned(vec![
        cipher::CHACHA20_POLY1305,
        cipher::AES_256_GCM,
        cipher::AES_128_GCM,
        cipher::AES_256_CTR,
        cipher::AES_192_CTR,
        cipher::AES_128_CTR,
        cipher::AES_256_CBC,
        cipher::AES_192_CBC,
        cipher::AES_128_CBC,
        cipher::TRIPLE_DES_CBC,
    ]);

    preferred.mac = Cow::Owned(vec![
        mac::HMAC_SHA512_ETM,
        mac::HMAC_SHA256_ETM,
        mac::HMAC_SHA512,
        mac::HMAC_SHA256,
        mac::HMAC_SHA1_ETM,
        mac::HMAC_SHA1,
    ]);

    preferred
}

fn secure_algorithms() -> Preferred {
    Preferred::default()
}

pub(crate) fn validate_ssh_algorithm_preferences(
    preferences: &SshAlgorithmPreferences,
) -> AppResult<()> {
    resolve_preferred_algorithms(Some(preferences)).map(|_| ())
}

pub(crate) fn resolve_preferred_algorithms(
    preferences: Option<&SshAlgorithmPreferences>,
) -> AppResult<Preferred> {
    let Some(preferences) = preferences else {
        return Ok(compatible_algorithms());
    };

    match preferences.mode {
        SshAlgorithmMode::Compatible => Ok(compatible_algorithms()),
        SshAlgorithmMode::Secure => Ok(secure_algorithms()),
        SshAlgorithmMode::Custom => custom_algorithms(preferences),
    }
}

fn custom_algorithms(preferences: &SshAlgorithmPreferences) -> AppResult<Preferred> {
    let mut preferred = Preferred::default();
    preferred.kex = Cow::Owned(parse_kex_names(&preferences.kex)?);
    preferred.cipher = Cow::Owned(parse_cipher_names(&preferences.ciphers)?);
    preferred.mac = Cow::Owned(parse_mac_names(&preferences.macs)?);
    preferred.key = Cow::Owned(parse_host_key_algorithms(&preferences.host_keys)?);
    Ok(preferred)
}

fn parse_required_list<T, F>(values: &[String], label: &str, mut parse: F) -> AppResult<Vec<T>>
where
    F: FnMut(&str) -> Option<T>,
{
    if values.is_empty() {
        return Err(AppError::Config(format!(
            "SSH algorithm list '{}' must not be empty",
            label
        )));
    }

    values
        .iter()
        .map(|value| {
            parse(value).ok_or_else(|| {
                AppError::Config(format!(
                    "Unsupported SSH algorithm '{}' in {}",
                    value, label
                ))
            })
        })
        .collect()
}

fn parse_kex_names(values: &[String]) -> AppResult<Vec<kex::Name>> {
    parse_required_list(values, "key exchanges", |value| {
        kex::Name::try_from(value).ok()
    })
}

fn parse_cipher_names(values: &[String]) -> AppResult<Vec<cipher::Name>> {
    parse_required_list(values, "ciphers", |value| {
        cipher::Name::try_from(value).ok()
    })
}

fn parse_mac_names(values: &[String]) -> AppResult<Vec<mac::Name>> {
    parse_required_list(values, "MACs", |value| mac::Name::try_from(value).ok())
}

fn parse_host_key_algorithms(values: &[String]) -> AppResult<Vec<Algorithm>> {
    parse_required_list(values, "host keys", |value| Algorithm::from_str(value).ok())
}

fn defaults_from_preferred(preferred: Preferred) -> SshAlgorithmDefaults {
    SshAlgorithmDefaults {
        kex: preferred
            .kex
            .iter()
            .map(|algorithm| algorithm.as_ref().to_string())
            .collect(),
        ciphers: preferred
            .cipher
            .iter()
            .map(|algorithm| algorithm.as_ref().to_string())
            .collect(),
        macs: preferred
            .mac
            .iter()
            .map(|algorithm| algorithm.as_ref().to_string())
            .collect(),
        host_keys: preferred.key.iter().map(ToString::to_string).collect(),
    }
}

fn algorithm_option(id: impl Into<String>, risk: AlgorithmRisk) -> AlgorithmOption {
    let id = id.into();
    AlgorithmOption {
        label: id.clone(),
        id,
        risk,
    }
}

fn kex_risk(id: &str) -> AlgorithmRisk {
    match id {
        "diffie-hellman-group1-sha1"
        | "diffie-hellman-group14-sha1"
        | "diffie-hellman-group-exchange-sha1" => AlgorithmRisk::Insecure,
        value if value.starts_with("diffie-hellman-") => AlgorithmRisk::Legacy,
        _ => AlgorithmRisk::Modern,
    }
}

fn cipher_risk(id: &str) -> AlgorithmRisk {
    match id {
        "3des-cbc" => AlgorithmRisk::Insecure,
        value if value.ends_with("-cbc") => AlgorithmRisk::Legacy,
        _ => AlgorithmRisk::Modern,
    }
}

fn mac_risk(id: &str) -> AlgorithmRisk {
    match id {
        "hmac-sha1" => AlgorithmRisk::Insecure,
        "hmac-sha1-etm@openssh.com" => AlgorithmRisk::Legacy,
        _ => AlgorithmRisk::Modern,
    }
}

fn host_key_risk(id: &str) -> AlgorithmRisk {
    match id {
        "ssh-dss" => AlgorithmRisk::Insecure,
        "ssh-rsa" => AlgorithmRisk::Legacy,
        _ => AlgorithmRisk::Modern,
    }
}

pub fn get_supported_ssh_algorithms() -> SupportedSshAlgorithms {
    let compatible = defaults_from_preferred(compatible_algorithms());
    let secure = defaults_from_preferred(secure_algorithms());

    let mut kex_ids = compatible.kex.clone();
    for id in &secure.kex {
        if !kex_ids.contains(id) {
            kex_ids.push(id.clone());
        }
    }

    let mut cipher_ids = compatible.ciphers.clone();
    for id in &secure.ciphers {
        if !cipher_ids.contains(id) {
            cipher_ids.push(id.clone());
        }
    }

    let mut mac_ids = compatible.macs.clone();
    for id in &secure.macs {
        if !mac_ids.contains(id) {
            mac_ids.push(id.clone());
        }
    }

    let mut host_key_ids = compatible.host_keys.clone();
    for id in &secure.host_keys {
        if !host_key_ids.contains(id) {
            host_key_ids.push(id.clone());
        }
    }

    SupportedSshAlgorithms {
        kex: kex_ids
            .into_iter()
            .map(|id| algorithm_option(id.clone(), kex_risk(&id)))
            .collect(),
        ciphers: cipher_ids
            .into_iter()
            .map(|id| algorithm_option(id.clone(), cipher_risk(&id)))
            .collect(),
        macs: mac_ids
            .into_iter()
            .map(|id| algorithm_option(id.clone(), mac_risk(&id)))
            .collect(),
        host_keys: host_key_ids
            .into_iter()
            .map(|id| algorithm_option(id.clone(), host_key_risk(&id)))
            .collect(),
        compatible,
        secure,
    }
}

impl client::Handler for SshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKey,
    ) -> Result<bool, Self::Error> {
        let key_type = server_public_key.algorithm().to_string();
        let key_base64 = server_public_key.public_key_base64();
        let fingerprint = server_public_key.fingerprint(Default::default());

        let host_identifier = if self.port != 22 {
            format!("[{}]:{}", self.host, self.port)
        } else {
            self.host.clone()
        };

        let host_entry = format!("{} {} {}", host_identifier, key_type, key_base64);

        let policy = crate::config::load_app_settings(&self.app)
            .map(|s| s.security.host_key_policy)
            .unwrap_or_else(|_| "prompt".to_string());

        let check = crate::storage::check_known_host(&host_identifier, &key_type, &key_base64)
            .unwrap_or(crate::storage::KnownHostCheck::UnknownHost);

        tracing::info!(
            host = %self.host, port = self.port,
            key_type, fingerprint = %fingerprint,
            host_key_policy = %policy,
            check_result = ?check,
            "SSH host key check"
        );

        if check == crate::storage::KnownHostCheck::Match {
            return Ok(true);
        }

        let is_key_changed = check == crate::storage::KnownHostCheck::HostSeen;

        match policy.as_str() {
            "strict" => {
                if is_key_changed {
                    tracing::warn!(
                        host = %self.host, port = self.port, key_type,
                        fingerprint = %fingerprint,
                        "SSH host key mismatch rejected (strict policy)"
                    );
                    let _ = self.app.emit(
                        "ssh-error",
                        format!(
                            "SECURITY ALERT: Host key for {}:{} has changed! Fingerprint: {}",
                            self.host, self.port, fingerprint
                        ),
                    );
                } else {
                    tracing::warn!(
                        host = %self.host, port = self.port, key_type,
                        fingerprint = %fingerprint,
                        "Unknown SSH host rejected (strict policy)"
                    );
                    let _ = self.app.emit(
                        "ssh-error",
                        format!(
                            "Unknown host key for {}:{} rejected by strict policy. Fingerprint: {}",
                            self.host, self.port, fingerprint
                        ),
                    );
                }
                Ok(false)
            }
            "accept" => {
                if is_key_changed {
                    tracing::info!(
                        host = %self.host, port = self.port, key_type,
                        fingerprint = %fingerprint,
                        "SSH host key changed, auto-accepting and updating known_hosts"
                    );
                    if let Err(error) = replace_known_host_entry(&host_identifier, &host_entry) {
                        tracing::warn!(
                            host = %self.host, port = self.port, %error,
                            "Failed to update known_hosts"
                        );
                    }
                } else {
                    tracing::info!(
                        host = %self.host, port = self.port, key_type,
                        fingerprint = %fingerprint,
                        "Auto-accepting new SSH host key and appending to known_hosts"
                    );
                    self.append_known_host(&host_entry);
                }
                Ok(true)
            }
            _ => {
                // "prompt" mode: ask user via frontend dialog
                let verify_mgr = self
                    .app
                    .try_state::<Arc<HostKeyVerifyManager>>()
                    .ok_or_else(|| russh::Error::Keys(russh::keys::Error::CouldNotReadKey))?;

                let request_id = uuid::Uuid::new_v4().to_string();
                let rx = verify_mgr.register(request_id.clone()).await;

                let payload = HostKeyVerifyPayload {
                    request_id: request_id.clone(),
                    host: self.host.clone(),
                    port: self.port,
                    key_type: key_type.clone(),
                    fingerprint: fingerprint.to_string(),
                    is_key_changed,
                    target_window_label: self.owner_window_label.clone(),
                };

                tracing::info!(
                    host = %self.host, port = self.port,
                    key_type, fingerprint = %fingerprint,
                    is_key_changed,
                    "Prompting user to verify SSH host key"
                );

                let _ = self.app.emit("host-key-verify", &payload);

                // 120s timeout prevents indefinite hang if the frontend
                // isn't ready (e.g. startup reconnect before listeners register).
                let accepted =
                    match tokio::time::timeout(std::time::Duration::from_secs(120), rx).await {
                        Ok(Ok(v)) => v,
                        _ => {
                            // Clean up the dangling sender so it doesn't leak.
                            let _ = verify_mgr.respond(&request_id, false).await;
                            tracing::warn!(
                                host = %self.host, port = self.port,
                                "Host key verification timed out or channel dropped, rejecting"
                            );
                            false
                        }
                    };
                let _ = self.app.emit(
                    "host-key-verify-resolved",
                    serde_json::json!({ "requestId": request_id }),
                );

                if accepted {
                    tracing::info!(
                        host = %self.host, port = self.port,
                        "User accepted SSH host key"
                    );
                    if is_key_changed {
                        if let Err(error) = replace_known_host_entry(&host_identifier, &host_entry)
                        {
                            tracing::warn!(
                                host = %self.host, port = self.port, %error,
                                "Failed to update known_hosts"
                            );
                        }
                    } else {
                        self.append_known_host(&host_entry);
                    }
                    Ok(true)
                } else {
                    tracing::info!(
                        host = %self.host, port = self.port,
                        "User rejected SSH host key"
                    );
                    Ok(false)
                }
            }
        }
    }

    async fn kex_done(
        &mut self,
        _shared_secret: Option<&[u8]>,
        names: &russh::Names,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        tracing::debug!(
            host = %self.host,
            port = self.port,
            kex = names.kex.as_ref(),
            host_key = %names.key,
            cipher = names.cipher.as_ref(),
            client_mac = names.client_mac.as_ref(),
            server_mac = names.server_mac.as_ref(),
            "SSH algorithms negotiated"
        );

        Ok(())
    }

    async fn disconnected(
        &mut self,
        reason: client::DisconnectReason<Self::Error>,
    ) -> Result<(), Self::Error> {
        match reason {
            client::DisconnectReason::ReceivedDisconnect(info) => {
                if let Some(tx) = &self.disconnect_tx {
                    let _ = tx.send(format!("SSH server disconnected: {}", info.message));
                }
                let (session_id, stage) = self
                    .diagnostics
                    .as_ref()
                    .map(SshDiagnosticContext::snapshot)
                    .unwrap_or((None, SshDiagnosticStage::Authenticating));
                tracing::warn!(
                    host = %self.host,
                    port = self.port,
                    session_id = session_id.as_deref().unwrap_or(""),
                    stage = %stage,
                    reason_code = ?info.reason_code,
                    message = %info.message,
                    lang_tag = %info.lang_tag,
                    "SSH transport disconnected by server"
                );
                Ok(())
            }
            client::DisconnectReason::Error(error) => {
                if let Some(tx) = &self.disconnect_tx {
                    let _ = tx.send(format!("SSH connection error: {error}"));
                }
                let (session_id, stage) = self
                    .diagnostics
                    .as_ref()
                    .map(SshDiagnosticContext::snapshot)
                    .unwrap_or((None, SshDiagnosticStage::Authenticating));
                tracing::error!(
                    host = %self.host,
                    port = self.port,
                    session_id = session_id.as_deref().unwrap_or(""),
                    stage = %stage,
                    error = ?error,
                    "SSH transport disconnected with error"
                );
                Err(error)
            }
        }
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<client::Msg>,
        connected_address: &str,
        connected_port: u32,
        originator_address: &str,
        originator_port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        if let Some(tx) = &self.remote_forward_tx {
            match tx.send(RemoteForwardOpen {
                channel,
                connected_address: connected_address.to_string(),
                connected_port,
                originator_address: originator_address.to_string(),
                originator_port,
            }) {
                Ok(()) => {
                    reply.accept().await;
                }
                Err(error) => {
                    reply
                        .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                        .await;
                    let _ = error.0.channel.close().await;
                }
            }
        } else {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
        }
        Ok(())
    }

    async fn server_channel_open_x11(
        &mut self,
        channel: russh::Channel<client::Msg>,
        originator_address: &str,
        originator_port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        if let Some(tx) = &self.x11_tx {
            if let Err(error) = tx.send(super::x11_forwarding::X11ChannelOpen {
                channel,
                originator_address: originator_address.to_string(),
                originator_port,
            }) {
                reply
                    .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                    .await;
                let _ = error.0.channel.close().await;
                return Ok(());
            }
            reply.accept().await;
        } else {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
        }
        Ok(())
    }

    async fn server_channel_open_agent_forward(
        &mut self,
        channel: russh::Channel<client::Msg>,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        if let Some(broker) = self.agent_broker.clone() {
            // Run the broker outside the SSH event loop and apply policy after merging identities.
            broker.spawn(channel, reply);
            return Ok(());
        }

        let Some(endpoint) = self.agent_forwarding_endpoint.clone() else {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };

        // Keep the raw Agent protocol for external-Agent-only all mode to preserve unknown
        // extensions. This relay intentionally has no broker idle timeout: its lifetime follows
        // the SSH channel, while the global channel permit still bounds resource consumption.
        let Some(permit) = try_acquire_agent_channel_permit() else {
            reply
                .reject(russh::ChannelOpenFailure::ResourceShortage)
                .await;
            let _ = channel.close().await;
            return Ok(());
        };
        tokio::spawn(async move {
            let _permit = permit;
            let Ok(mut agent_stream) = connect_agent_stream(&endpoint).await else {
                reply.reject(russh::ChannelOpenFailure::ConnectFailed).await;
                let _ = channel.close().await;
                return;
            };
            reply.accept().await;
            let mut channel_stream = channel.into_stream();
            if let Err(error) =
                tokio::io::copy_bidirectional(&mut agent_stream, &mut channel_stream).await
            {
                tracing::debug!(%error, "SSH Agent raw forwarding channel closed");
            }
        });
        Ok(())
    }
}

pub(super) fn build_client_config(
    app: &AppHandle,
    config: &SshConfig,
) -> AppResult<client::Config> {
    let mut client_cfg = client::Config {
        window_size: 32 * 1024 * 1024,
        maximum_packet_size: 32 * 1024,
        nodelay: true,
        inactivity_timeout: None,
        keepalive_max: 3,
        preferred: resolve_preferred_algorithms(config.ssh_algorithms.as_ref())?,
        ..Default::default()
    };

    if let Ok(gex) = client::GexParams::new(2048, 4096, 8192) {
        client_cfg.gex = gex;
    }

    if let Ok(app_settings) = crate::config::load_app_settings(app) {
        let interval = app_settings.terminal.keep_alive_interval;
        let mode = app_settings.terminal.keep_alive_mode.as_str();
        apply_keepalive_settings(&mut client_cfg, mode, interval);
        tracing::debug!(
            keep_alive_mode = %mode,
            keep_alive_interval = interval,
            keepalive_max = client_cfg.keepalive_max,
            "Configured SSH keepalive"
        );
    }

    Ok(client_cfg)
}

fn resolve_keepalive_mode(value: &str) -> client::KeepaliveMode {
    match value {
        "strict" => client::KeepaliveMode::Strict,
        _ => client::KeepaliveMode::Compatible,
    }
}

fn apply_keepalive_settings(client_cfg: &mut client::Config, mode: &str, interval: u32) {
    client_cfg.keepalive_mode = resolve_keepalive_mode(mode);

    if mode == "disabled" || interval == 0 {
        client_cfg.keepalive_interval = None;
        return;
    }

    client_cfg.keepalive_interval = Some(std::time::Duration::from_secs(interval as u64));
}

#[cfg(test)]
mod tests {
    use super::{
        KnownHostCheck, SftpChannelLimiter, apply_keepalive_settings, check_known_host_entry,
        compatible_algorithms, expand_proxy_command, get_supported_ssh_algorithms,
        resolve_keepalive_mode, resolve_preferred_algorithms, secure_algorithms, shell_quote,
    };
    use crate::config::{SshAlgorithmMode, SshAlgorithmPreferences};
    use russh::keys::{Algorithm, EcdsaCurve};
    use russh::{cipher, kex, mac};
    use std::time::Duration;

    #[test]
    fn known_hosts_accepts_exact_match_after_other_key_types() {
        let content = "\
example.com ssh-ed25519 AAAAED25519
example.com ssh-rsa AAAARSA
";

        assert_eq!(
            check_known_host_entry(content, "example.com", "ssh-rsa", "AAAARSA"),
            KnownHostCheck::Match
        );
    }

    #[test]
    fn known_hosts_flags_seen_host_without_matching_key() {
        let content = "example.com ssh-ed25519 AAAAED25519\n";

        assert_eq!(
            check_known_host_entry(content, "example.com", "ssh-rsa", "AAAARSA"),
            KnownHostCheck::HostSeen
        );
    }

    #[test]
    fn preferred_algorithms_include_legacy_fallbacks() {
        let preferred = compatible_algorithms();

        assert!(preferred.cipher.contains(&cipher::AES_128_CBC));
        assert!(preferred.cipher.contains(&cipher::TRIPLE_DES_CBC));
        assert!(preferred.kex.contains(&kex::DH_GEX_SHA1));
        assert!(preferred.kex.contains(&kex::DH_G1_SHA1));
        assert!(preferred.mac.contains(&mac::HMAC_SHA1));
        assert!(preferred.key.contains(&Algorithm::Dsa));
        assert_eq!(preferred.key.last(), Some(&Algorithm::Dsa));

        let rsa_sha1_index = preferred
            .key
            .iter()
            .position(|algorithm| matches!(algorithm, Algorithm::Rsa { hash: None }))
            .expect("plain ssh-rsa is enabled");
        let ecdsa_p521_index = preferred
            .key
            .iter()
            .position(|algorithm| {
                matches!(
                    algorithm,
                    Algorithm::Ecdsa {
                        curve: EcdsaCurve::NistP521
                    }
                )
            })
            .expect("P-521 ECDSA is enabled");
        assert!(rsa_sha1_index < ecdsa_p521_index);
    }

    #[test]
    fn secure_algorithms_exclude_legacy_fallbacks() {
        let preferred = secure_algorithms();

        assert!(!preferred.cipher.contains(&cipher::AES_128_CBC));
        assert!(!preferred.cipher.contains(&cipher::TRIPLE_DES_CBC));
        assert!(!preferred.kex.contains(&kex::DH_GEX_SHA1));
        assert!(!preferred.kex.contains(&kex::DH_G1_SHA1));
        assert!(!preferred.mac.contains(&mac::HMAC_SHA1));
        assert!(!preferred.key.contains(&Algorithm::Dsa));
    }

    #[tokio::test]
    async fn sftp_channel_limiter_waits_until_permit_is_released() {
        let limiter = SftpChannelLimiter::new(6);
        let mut permits = Vec::new();

        for _ in 0..6 {
            permits.push(limiter.acquire().await.expect("permit should be available"));
        }

        assert_eq!(limiter.available_permits(), 0);
        assert!(
            tokio::time::timeout(Duration::from_millis(25), limiter.acquire())
                .await
                .is_err(),
            "seventh acquire should wait while all permits are held"
        );

        drop(permits.pop());

        let permit = tokio::time::timeout(Duration::from_millis(25), limiter.acquire())
            .await
            .expect("acquire should resume after release")
            .expect("released permit should be acquired");
        drop(permit);
    }

    #[test]
    fn missing_algorithm_preferences_default_to_compatible() {
        let preferred = resolve_preferred_algorithms(None).expect("resolve defaults");

        assert!(preferred.cipher.contains(&cipher::TRIPLE_DES_CBC));
        assert!(preferred.kex.contains(&kex::DH_G1_SHA1));
    }

    #[test]
    fn keepalive_mode_parser_defaults_unknown_values_to_compatible() {
        assert_eq!(
            resolve_keepalive_mode("compatible"),
            russh::client::KeepaliveMode::Compatible
        );
        assert_eq!(
            resolve_keepalive_mode("unknown"),
            russh::client::KeepaliveMode::Compatible
        );
        assert_eq!(
            resolve_keepalive_mode("strict"),
            russh::client::KeepaliveMode::Strict
        );
    }

    #[test]
    fn keepalive_settings_disable_timer_for_disabled_or_zero_interval() {
        let mut disabled = russh::client::Config::default();
        apply_keepalive_settings(&mut disabled, "disabled", 60);
        assert_eq!(disabled.keepalive_interval, None);

        let mut zero_interval = russh::client::Config::default();
        apply_keepalive_settings(&mut zero_interval, "strict", 0);
        assert_eq!(zero_interval.keepalive_interval, None);
    }

    #[test]
    fn keepalive_settings_apply_strict_and_compatible_modes() {
        let mut strict = russh::client::Config::default();
        apply_keepalive_settings(&mut strict, "strict", 60);
        assert_eq!(strict.keepalive_interval, Some(Duration::from_secs(60)));
        assert_eq!(strict.keepalive_mode, russh::client::KeepaliveMode::Strict);

        let mut compatible = russh::client::Config::default();
        apply_keepalive_settings(&mut compatible, "compatible", 45);
        assert_eq!(compatible.keepalive_interval, Some(Duration::from_secs(45)));
        assert_eq!(
            compatible.keepalive_mode,
            russh::client::KeepaliveMode::Compatible
        );
    }

    #[test]
    fn custom_algorithm_preferences_preserve_order() {
        let preferences = SshAlgorithmPreferences {
            mode: SshAlgorithmMode::Custom,
            kex: vec![
                kex::CURVE25519_PRE_RFC_8731.as_ref().to_string(),
                kex::CURVE25519.as_ref().to_string(),
            ],
            ciphers: vec![
                cipher::AES_128_CTR.as_ref().to_string(),
                cipher::AES_256_CTR.as_ref().to_string(),
            ],
            macs: vec![
                mac::HMAC_SHA256.as_ref().to_string(),
                mac::HMAC_SHA512.as_ref().to_string(),
            ],
            host_keys: vec![
                Algorithm::Rsa { hash: None }.to_string(),
                Algorithm::Ed25519.to_string(),
            ],
        };

        let preferred =
            resolve_preferred_algorithms(Some(&preferences)).expect("resolve custom algorithms");

        assert_eq!(preferred.kex[0], kex::CURVE25519_PRE_RFC_8731);
        assert_eq!(preferred.kex[1], kex::CURVE25519);
        assert_eq!(preferred.cipher[0], cipher::AES_128_CTR);
        assert_eq!(preferred.cipher[1], cipher::AES_256_CTR);
        assert_eq!(preferred.mac[0], mac::HMAC_SHA256);
        assert_eq!(preferred.mac[1], mac::HMAC_SHA512);
        assert_eq!(preferred.key[0], Algorithm::Rsa { hash: None });
        assert_eq!(preferred.key[1], Algorithm::Ed25519);
    }

    #[test]
    fn custom_algorithm_preferences_reject_empty_lists() {
        let preferences = SshAlgorithmPreferences {
            mode: SshAlgorithmMode::Custom,
            kex: Vec::new(),
            ciphers: vec![cipher::AES_128_CTR.as_ref().to_string()],
            macs: vec![mac::HMAC_SHA256.as_ref().to_string()],
            host_keys: vec![Algorithm::Ed25519.to_string()],
        };

        let error = resolve_preferred_algorithms(Some(&preferences)).unwrap_err();
        assert!(error.to_string().contains("must not be empty"));
    }

    #[test]
    fn custom_algorithm_preferences_reject_unknown_algorithms() {
        let preferences = SshAlgorithmPreferences {
            mode: SshAlgorithmMode::Custom,
            kex: vec!["not-a-kex".to_string()],
            ciphers: vec![cipher::AES_128_CTR.as_ref().to_string()],
            macs: vec![mac::HMAC_SHA256.as_ref().to_string()],
            host_keys: vec![Algorithm::Ed25519.to_string()],
        };

        let error = resolve_preferred_algorithms(Some(&preferences)).unwrap_err();
        assert!(error.to_string().contains("Unsupported SSH algorithm"));
    }

    #[test]
    fn supported_algorithms_include_current_feature_fallbacks() {
        let supported = get_supported_ssh_algorithms();

        assert!(supported.ciphers.iter().any(|item| item.id == "3des-cbc"));
        assert!(supported.host_keys.iter().any(|item| item.id == "ssh-dss"));
        assert!(
            supported
                .compatible
                .kex
                .contains(&"diffie-hellman-group1-sha1".to_string())
        );
        assert!(
            !supported
                .secure
                .kex
                .contains(&"diffie-hellman-group1-sha1".to_string())
        );
    }

    #[test]
    fn expands_proxy_command_placeholders() {
        let command = expand_proxy_command(
            Some("nc -X connect -x jump:1080 %h %p --user=%r --literal=%%"),
            "example.com",
            2222,
            "alice",
        )
        .expect("command expands");

        assert!(command.contains("nc -X connect -x jump:1080"));
        assert!(command.contains(&shell_quote("example.com")));
        assert!(command.contains(&shell_quote("2222")));
        assert!(command.contains(&format!("--user={}", shell_quote("alice"))));
        assert!(command.contains("--literal=%"));
    }

    #[test]
    fn proxy_command_preserves_unknown_percent_escape() {
        let command =
            expand_proxy_command(Some("tool %x %"), "host", 22, "user").expect("command expands");

        assert_eq!(command, "tool %x %");
    }

    #[test]
    fn proxy_command_rejects_empty_template() {
        let error = expand_proxy_command(Some("   "), "host", 22, "user").unwrap_err();

        assert!(error.to_string().contains("ProxyCommand is empty"));
    }

    #[cfg(not(windows))]
    #[test]
    fn shell_quote_unix_handles_spaces_and_quotes() {
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(shell_quote("two words"), "'two words'");
        assert_eq!(shell_quote("can't"), "'can'\\''t'");
    }

    #[cfg(windows)]
    #[test]
    fn shell_quote_windows_handles_spaces_and_quotes() {
        assert_eq!(shell_quote(""), "\"\"");
        assert_eq!(shell_quote("plain"), "plain");
        assert_eq!(shell_quote("example.com:22"), "example.com:22");
        assert_eq!(shell_quote("two words"), "\"two words\"");
        assert_eq!(shell_quote("say \"hi\""), "\"say \"\"hi\"\"\"");
    }
}

pub(super) async fn connect_with_proxy(
    config: &SshConfig,
    ssh_config: Arc<client::Config>,
    handler: SshHandler,
) -> AppResult<client::Handle<SshHandler>> {
    let target = (config.host.as_str(), config.port);
    let handler_host = handler.host.clone();
    let handler_port = handler.port;
    let handle = if let Some(proxy) = config.proxy.clone().filter(|proxy| proxy.enabled) {
        tracing::info!(
            host = %config.host,
            port = config.port,
            proxy_protocol = %proxy.protocol,
            proxy_host = %proxy.host,
            proxy_port = proxy.port,
            "Opening SSH transport via proxy"
        );

        let proxy_addr = format!("{}:{}", proxy.host, proxy.port);
        match proxy.protocol.as_str() {
            "socks5" => {
                let stream = match (&proxy.username, &proxy.password) {
                    (Some(user), Some(pass)) => {
                        tokio_socks::tcp::Socks5Stream::connect_with_password(
                            proxy_addr.as_str(),
                            target,
                            user,
                            pass,
                        )
                        .await
                    }
                    _ => tokio_socks::tcp::Socks5Stream::connect(proxy_addr.as_str(), target).await,
                }
                .map_err(|error| {
                    AppError::Auth(format!("SOCKS5 proxy connection failed: {}", error))
                })?;
                client::connect_stream(ssh_config, stream.into_inner(), handler).await
            }
            "http" => {
                let mut stream =
                    tokio::net::TcpStream::connect(&proxy_addr)
                        .await
                        .map_err(|error| {
                            AppError::Auth(format!("HTTP proxy connection failed: {}", error))
                        })?;

                match (&proxy.username, &proxy.password) {
                    (Some(user), Some(pass)) => {
                        async_http_proxy::http_connect_tokio_with_basic_auth(
                            &mut stream,
                            &config.host,
                            config.port,
                            user,
                            pass,
                        )
                        .await
                    }
                    _ => {
                        async_http_proxy::http_connect_tokio(&mut stream, &config.host, config.port)
                            .await
                    }
                }
                .map_err(|error| AppError::Auth(format!("HTTP proxy tunnel failed: {}", error)))?;

                client::connect_stream(ssh_config, stream, handler).await
            }
            "proxycommand" => {
                let stream = open_proxy_command_stream(
                    proxy.command.as_deref(),
                    &config.host,
                    config.port,
                    &config.username,
                )
                .await?;
                client::connect_stream(ssh_config, stream, handler).await
            }
            _ => client::connect(ssh_config, target, handler).await,
        }
    } else {
        tracing::debug!(
            host = %config.host,
            port = config.port,
            "Opening direct SSH transport"
        );
        client::connect(ssh_config, target, handler).await
    }
    .map_err(|error| AppError::Auth(format!("SSH connection failed: {}", error)))?;

    tracing::info!(
        host = %handler_host,
        port = handler_port,
        "SSH transport established"
    );

    Ok(handle)
}

pub(crate) async fn open_proxy_command_stream(
    template: Option<&str>,
    host: &str,
    port: u16,
    username: &str,
) -> AppResult<ProxyCommandStream> {
    let command = expand_proxy_command(template, host, port, username)?;
    tracing::info!("Opening SSH transport via ProxyCommand");

    let mut process = system_shell_command(&command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| AppError::Auth(format!("ProxyCommand failed to start: {}", error)))?;

    let stdin = process
        .stdin
        .take()
        .ok_or_else(|| AppError::Auth("ProxyCommand stdin unavailable".to_string()))?;
    let stdout = process
        .stdout
        .take()
        .ok_or_else(|| AppError::Auth("ProxyCommand stdout unavailable".to_string()))?;

    if let Some(stderr) = process.stderr.take() {
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = Vec::new();
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line).await {
                    Ok(0) => break,
                    Ok(_) => {
                        let message = String::from_utf8_lossy(&line);
                        let message = message.trim_end_matches(['\r', '\n']);
                        if !message.is_empty() {
                            tracing::warn!(message = %message, "ProxyCommand stderr");
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, "Failed to read ProxyCommand stderr");
                        break;
                    }
                }
            }
        });
    }

    tokio::spawn(async move {
        match process.wait().await {
            Ok(status) if status.success() => {
                tracing::debug!(%status, "ProxyCommand exited");
            }
            Ok(status) => {
                tracing::warn!(%status, "ProxyCommand exited with failure");
            }
            Err(error) => {
                tracing::warn!(%error, "Failed to wait for ProxyCommand");
            }
        }
    });

    Ok(ProxyCommandStream { stdout, stdin })
}

pub(crate) struct ProxyCommandStream {
    stdout: tokio::process::ChildStdout,
    stdin: tokio::process::ChildStdin,
}

impl AsyncRead for ProxyCommandStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stdout).poll_read(cx, buf)
    }
}

impl AsyncWrite for ProxyCommandStream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.stdin).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stdin).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stdin).poll_shutdown(cx)
    }
}

fn expand_proxy_command(
    template: Option<&str>,
    host: &str,
    port: u16,
    username: &str,
) -> AppResult<String> {
    let template = template.unwrap_or_default().trim();
    if template.is_empty() {
        return Err(AppError::Auth("ProxyCommand is empty".to_string()));
    }

    let quoted_host = shell_quote(host);
    let quoted_port = shell_quote(&port.to_string());
    let quoted_username = shell_quote(username);

    let mut output = String::with_capacity(template.len());
    let mut chars = template.chars();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            output.push(ch);
            continue;
        }

        match chars.next() {
            Some('%') => output.push('%'),
            Some('h') => output.push_str(&quoted_host),
            Some('p') => output.push_str(&quoted_port),
            Some('r') => output.push_str(&quoted_username),
            Some(other) => {
                output.push('%');
                output.push(other);
            }
            None => output.push('%'),
        }
    }

    Ok(output)
}

#[cfg(windows)]
fn system_shell_command(command: &str) -> tokio::process::Command {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut cmd = tokio::process::Command::new("cmd");
    cmd.arg("/C").arg(command);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(not(windows))]
fn system_shell_command(command: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(command);
    cmd
}

#[cfg(windows)]
fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "\"\"".to_string();
    }

    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':' | '@' | '%'))
    {
        return value.to_string();
    }

    let escaped = value.replace('"', "\"\"");
    format!("\"{}\"", escaped)
}

#[cfg(not(windows))]
fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }

    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) async fn connect_via_stream<S>(
    stream: S,
    ssh_config: Arc<client::Config>,
    handler: SshHandler,
) -> AppResult<client::Handle<SshHandler>>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let handler_host = handler.host.clone();
    let handler_port = handler.port;

    tracing::info!(
        host = %handler_host,
        port = handler_port,
        "Opening SSH transport over existing stream"
    );

    let handle = client::connect_stream(ssh_config, stream, handler)
        .await
        .map_err(|error| AppError::Auth(format!("SSH connection failed: {}", error)))?;

    tracing::info!(
        host = %handler_host,
        port = handler_port,
        "SSH transport established over existing stream"
    );

    Ok(handle)
}
