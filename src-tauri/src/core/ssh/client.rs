use crate::error::{AppError, AppResult};
use russh::client;
use russh::keys::{Algorithm, EcdsaCurve, HashAlg, PublicKeyBase64};
use russh::{Preferred, cipher, kex, mac};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Mutex, oneshot};

/// Connection parameters for SSH (host, port, user, auth method).
#[derive(Debug, Clone, Deserialize)]
pub struct SshConfig {
    #[serde(default)]
    pub connection_id: Option<String>,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: SshAuth,
    #[serde(default)]
    pub proxy: Option<crate::config::ProxySettings>,
    #[serde(default)]
    pub proxy_jump: Option<Box<SshConfig>>,
}

/// Authentication method: none, password, or key (with optional passphrase).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum SshAuth {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "password")]
    Password { password: String },
    #[serde(rename = "key")]
    Key {
        key_data: String,
        passphrase: Option<String>,
    },
}

pub(crate) type SshRawHandle = Arc<Mutex<client::Handle<SshHandler>>>;

pub struct SshConnectionHandles {
    target: SshRawHandle,
    jump: Option<SshRawHandle>,
}

impl SshConnectionHandles {
    pub fn new(target: SshRawHandle, jump: Option<SshRawHandle>) -> Self {
        Self { target, jump }
    }

    pub fn target_handle(&self) -> SshRawHandle {
        self.target.clone()
    }

    #[allow(dead_code)]
    pub fn jump_handle(&self) -> Option<SshRawHandle> {
        self.jump.clone()
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
}

/// russh client handler; performs TOFU known_hosts verification.
pub struct SshHandler {
    app: AppHandle,
    host: String,
    port: u16,
}

impl SshHandler {
    pub fn new(app: AppHandle, host: String, port: u16) -> Self {
        Self { app, host, port }
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

fn preferred_algorithms() -> Preferred {
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
        Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP521,
        },
        Algorithm::Rsa {
            hash: Some(HashAlg::Sha512),
        },
        Algorithm::Rsa {
            hash: Some(HashAlg::Sha256),
        },
        Algorithm::Rsa { hash: None },
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
                tracing::warn!(
                    host = %self.host,
                    port = self.port,
                    reason_code = ?info.reason_code,
                    message = %info.message,
                    lang_tag = %info.lang_tag,
                    "SSH transport disconnected by server"
                );
                Ok(())
            }
            client::DisconnectReason::Error(error) => {
                tracing::error!(
                    host = %self.host,
                    port = self.port,
                    error = ?error,
                    "SSH transport disconnected with error"
                );
                Err(error)
            }
        }
    }
}

pub(super) fn build_client_config(app: &AppHandle) -> client::Config {
    let mut client_cfg = client::Config {
        window_size: 32 * 1024 * 1024,
        maximum_packet_size: 32 * 1024,
        nodelay: true,
        inactivity_timeout: None,
        keepalive_max: 3,
        preferred: preferred_algorithms(),
        ..Default::default()
    };

    if let Ok(gex) = client::GexParams::new(2048, 4096, 8192) {
        client_cfg.gex = gex;
    }

    if let Ok(app_settings) = crate::config::load_app_settings(app) {
        let interval = app_settings.terminal.keep_alive_interval;
        if interval > 0 {
            client_cfg.keepalive_interval = Some(std::time::Duration::from_secs(interval as u64));
        }
    }

    client_cfg
}

#[cfg(test)]
mod tests {
    use super::{KnownHostCheck, check_known_host_entry, preferred_algorithms};
    use russh::{cipher, kex, mac};

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
        let preferred = preferred_algorithms();

        assert!(preferred.cipher.contains(&cipher::AES_128_CBC));
        assert!(preferred.cipher.contains(&cipher::TRIPLE_DES_CBC));
        assert!(preferred.kex.contains(&kex::DH_GEX_SHA1));
        assert!(preferred.kex.contains(&kex::DH_G1_SHA1));
        assert!(preferred.mac.contains(&mac::HMAC_SHA1));
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
