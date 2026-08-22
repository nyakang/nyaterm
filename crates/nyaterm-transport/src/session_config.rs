use std::path::PathBuf;
use std::sync::Arc;
use zeroize::Zeroize;

use crate::ssh_auth::format_keyboard_interactive_prompt;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SshAgentEndpoint {
    #[default]
    Auto,
    Environment {
        variable: String,
    },
    UnixSocket {
        path: String,
    },
    Pageant,
    WindowsOpenSsh,
}

/// Runtime-only SSH Agent forwarding sources.
///
/// Persistent fields remain owned by `nyaterm-core`, keeping the transport
/// crate independent from GPUI and storage implementations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshAgentForwardingSources {
    pub external_agent: bool,
    pub external_agent_endpoints: Vec<SshAgentEndpoint>,
    pub stored_keys: bool,
}

impl Default for SshAgentForwardingSources {
    fn default() -> Self {
        Self {
            external_agent: false,
            external_agent_endpoints: Vec::new(),
            stored_keys: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshAgentForwardingPolicy {
    Allowlist { fingerprints: Vec<String> },
    All,
}

impl Default for SshAgentForwardingPolicy {
    fn default() -> Self {
        Self::Allowlist {
            fingerprints: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SshAgentForwardingConfig {
    pub enabled: bool,
    pub sources: SshAgentForwardingSources,
    pub policy: SshAgentForwardingPolicy,
}

/// A decrypted stored key supplied by the desktop layer.
///
/// Debug output always redacts secret material and Drop clears the plaintext
/// buffers after the broker finishes parsing them.
#[derive(Clone, PartialEq, Eq)]
pub struct SshAgentStoredKey {
    pub key_data: String,
    pub cert_data: Option<String>,
    pub passphrase: Option<String>,
    pub comment: String,
}

impl std::fmt::Debug for SshAgentStoredKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SshAgentStoredKey")
            .field("key_data", &"<redacted>")
            .field("cert_data", &self.cert_data.as_ref().map(|_| "<redacted>"))
            .field(
                "passphrase",
                &self.passphrase.as_ref().map(|_| "<redacted>"),
            )
            .field("comment", &self.comment)
            .finish()
    }
}

impl Drop for SshAgentStoredKey {
    fn drop(&mut self) {
        self.key_data.zeroize();
        self.cert_data.zeroize();
        self.passphrase.zeroize();
    }
}

#[derive(Debug)]
pub struct SshAgentStoredKeySnapshot {
    pub revision: u64,
    pub keys: Vec<SshAgentStoredKey>,
}

pub trait SshAgentStoredKeyProvider: Send + Sync {
    fn revision(&self) -> Result<u64, String>;
    fn load_snapshot(&self) -> Result<SshAgentStoredKeySnapshot, String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSessionConfig {
    pub name: String,
    pub shell_path: Option<String>,
    pub shell_args: Vec<String>,
    pub working_dir: Option<PathBuf>,
    pub encoding: String,
    pub cols: u16,
    pub rows: u16,
    /// Total terminal pixel width (cols * cell_width). Zero means unknown.
    pub pixel_width: u16,
    /// Total terminal pixel height (rows * cell_height). Zero means unknown.
    pub pixel_height: u16,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TelnetEnterMode {
    Crlf,
    #[default]
    Cr,
    Lf,
}

#[derive(Clone, PartialEq, Eq)]
pub struct TelnetSessionConfig {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Option<String>,
    pub backspace_mode: String,
    pub raw_tcp: bool,
    pub enter_mode: TelnetEnterMode,
    pub local_echo: bool,
    pub local_line_edit: bool,
    pub force_character_at_a_time: bool,
    pub send_naws: bool,
    pub send_sga: bool,
    pub auto_login: TelnetAutoLoginConfig,
    pub encoding: String,
    pub cols: u16,
    pub rows: u16,
}

impl std::fmt::Debug for TelnetSessionConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TelnetSessionConfig")
            .field("name", &self.name)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("backspace_mode", &self.backspace_mode)
            .field("raw_tcp", &self.raw_tcp)
            .field("enter_mode", &self.enter_mode)
            .field("local_echo", &self.local_echo)
            .field("local_line_edit", &self.local_line_edit)
            .field("force_character_at_a_time", &self.force_character_at_a_time)
            .field("send_naws", &self.send_naws)
            .field("send_sga", &self.send_sga)
            .field("auto_login", &self.auto_login)
            .field("encoding", &self.encoding)
            .field("cols", &self.cols)
            .field("rows", &self.rows)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelnetAutoLoginConfig {
    pub enabled: bool,
    pub send_wake_enter: bool,
    pub timeout_ms: u64,
    pub username_prompt_regex: Option<String>,
    pub password_prompt_regex: Option<String>,
    pub success_prompt_regex: Option<String>,
    pub failure_prompt_regex: Option<String>,
    pub max_retries: u8,
}

impl Default for TelnetAutoLoginConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            send_wake_enter: true,
            timeout_ms: 60_000,
            username_prompt_regex: None,
            password_prompt_regex: None,
            success_prompt_regex: None,
            failure_prompt_regex: None,
            max_retries: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerialSessionConfig {
    pub name: String,
    pub port_name: String,
    pub baud_rate: u32,
    pub data_bits: u8,
    pub parity: String,
    pub stop_bits: String,
    pub backspace_mode: String,
    pub encoding: String,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SshAlgorithmMode {
    #[default]
    Compatible,
    Secure,
    Custom,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SshAlgorithmPreferences {
    pub mode: SshAlgorithmMode,
    pub kex: Vec<String>,
    pub ciphers: Vec<String>,
    pub macs: Vec<String>,
    pub host_keys: Vec<String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SftpCwdFollowMode {
    Off,
    #[default]
    ShellIntegration,
    RcFile,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SshSessionProfile {
    #[default]
    Standard,
    NetworkDevice,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SftpSettings {
    pub enabled: bool,
    pub cwd_follow_mode: SftpCwdFollowMode,
    pub shell_detection_timeout_ms: u64,
    pub filename_encoding: String,
}

impl Default for SftpSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            cwd_follow_mode: SftpCwdFollowMode::ShellIntegration,
            shell_detection_timeout_ms: 3000,
            filename_encoding: String::new(),
        }
    }
}

#[derive(Clone)]
pub struct SshSessionConfig {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Option<String>,
    pub key_auth: Option<SshKeyAuthConfig>,
    pub agent_auth: bool,
    pub agent_endpoint: SshAgentEndpoint,
    /// Only interactive terminal sessions request forwarding. Shared configs
    /// used by SFTP, tunnels and jump hosts never request it themselves.
    pub agent_forwarding: bool,
    /// Multi-endpoint broker configuration. Legacy callers without this value
    /// are normalized to a compatible runtime configuration at the boundary.
    pub agent_forwarding_config: Option<SshAgentForwardingConfig>,
    pub otp_id: Option<String>,
    pub auto_fill_otp: bool,
    pub proxy_jump: Option<Box<SshSessionConfig>>,
    pub proxy: Option<SshProxyConfig>,
    pub allow_none_auth: bool,
    pub backspace_mode: String,
    pub profile: SshSessionProfile,
    pub term: String,
    pub x11_forwarding: bool,
    pub x11_display: String,
    pub encoding: String,
    pub ssh_algorithms: Option<SshAlgorithmPreferences>,
    pub sftp: SftpSettings,
    /// Install shell semantic markers for terminal row highlighting.
    pub terminal_shell_integration: bool,
    pub deferred_pty: bool,
    /// Seconds between SSH keepalive packets. Zero disables keepalive.
    pub keep_alive_interval_secs: u32,
    pub cols: u16,
    pub rows: u16,
    /// Total terminal pixel width (cols * cell_width). Zero means unknown.
    pub pixel_width: u16,
    /// Total terminal pixel height (rows * cell_height). Zero means unknown.
    pub pixel_height: u16,
    pub host_key_verifier: Option<Arc<dyn SshHostKeyVerifier>>,
    pub credential_provider: Option<Arc<dyn SshCredentialProvider>>,
    pub agent_prompt_provider: Option<Arc<dyn SshAgentPromptProvider>>,
    pub agent_stored_key_provider: Option<Arc<dyn SshAgentStoredKeyProvider>>,
    pub otp_provider: Option<Arc<dyn SshOtpProvider>>,
}

impl SshSessionConfig {
    pub fn is_network_device(&self) -> bool {
        self.profile == SshSessionProfile::NetworkDevice
    }

    pub fn remote_file_browser_enabled(&self) -> bool {
        self.sftp.enabled && !self.is_network_device()
    }

    pub fn remote_stats_enabled(&self) -> bool {
        !self.is_network_device()
    }

    pub fn effective_cwd_follow_mode(&self) -> SftpCwdFollowMode {
        if self.remote_file_browser_enabled() {
            self.sftp.cwd_follow_mode
        } else {
            SftpCwdFollowMode::Off
        }
    }

    pub fn effective_terminal_shell_integration(&self) -> bool {
        self.terminal_shell_integration && !self.is_network_device()
    }

    pub fn shell_integration_detection_timeout_ms(
        &self,
        cwd_follow_mode: SftpCwdFollowMode,
    ) -> u64 {
        let configured = self.sftp.shell_detection_timeout_ms.clamp(100, 60_000);
        if matches!(cwd_follow_mode, SftpCwdFollowMode::Off) {
            configured.min(1_000)
        } else {
            configured
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SshProxyConfig {
    pub protocol: String,
    pub host: String,
    pub port: u16,
    pub command: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl std::fmt::Debug for SshProxyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshProxyConfig")
            .field("protocol", &self.protocol)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("command", &self.command)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SshKeyAuthConfig {
    pub key_data: String,
    pub cert_data: Option<String>,
    pub passphrase: Option<String>,
}

impl std::fmt::Debug for SshKeyAuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshKeyAuthConfig")
            .field("key_data", &"<redacted>")
            .field("cert_data", &self.cert_data.as_ref().map(|_| "<redacted>"))
            .field(
                "passphrase",
                &self.passphrase.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl std::fmt::Debug for SshSessionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshSessionConfig")
            .field("name", &self.name)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("key_auth", &self.key_auth.as_ref().map(|_| "<redacted>"))
            .field("agent_auth", &self.agent_auth)
            .field("agent_endpoint", &self.agent_endpoint)
            .field("agent_forwarding", &self.agent_forwarding)
            .field(
                "agent_forwarding_config",
                &self
                    .agent_forwarding_config
                    .as_ref()
                    .map(|_| "<configured>"),
            )
            .field("otp_id", &self.otp_id)
            .field("auto_fill_otp", &self.auto_fill_otp)
            .field("proxy_jump", &self.proxy_jump.is_some())
            .field("proxy", &self.proxy)
            .field("allow_none_auth", &self.allow_none_auth)
            .field("backspace_mode", &self.backspace_mode)
            .field("profile", &self.profile)
            .field("term", &self.term)
            .field("x11_forwarding", &self.x11_forwarding)
            .field("x11_display", &self.x11_display)
            .field("encoding", &self.encoding)
            .field("ssh_algorithms", &self.ssh_algorithms)
            .field("sftp", &self.sftp)
            .field("deferred_pty", &self.deferred_pty)
            .field("keep_alive_interval_secs", &self.keep_alive_interval_secs)
            .field("cols", &self.cols)
            .field("rows", &self.rows)
            .field("pixel_width", &self.pixel_width)
            .field("pixel_height", &self.pixel_height)
            .field("host_key_verifier", &self.host_key_verifier.is_some())
            .field("credential_provider", &self.credential_provider.is_some())
            .field(
                "agent_prompt_provider",
                &self.agent_prompt_provider.is_some(),
            )
            .field(
                "agent_stored_key_provider",
                &self.agent_stored_key_provider.is_some(),
            )
            .field("otp_provider", &self.otp_provider.is_some())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshHostKey {
    pub host: String,
    pub port: u16,
    pub host_identifier: String,
    pub key_type: String,
    pub key_base64: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshHostKeyDecision {
    Accept,
    Reject(String),
}

pub trait SshHostKeyVerifier: Send + Sync {
    fn verify(&self, host_key: &SshHostKey) -> Result<SshHostKeyDecision, String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SshCredentialPromptKind {
    Password,
    KeyPassphrase,
    KeyboardInteractive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SshCredentialPromptReason {
    MissingPassword,
    PasswordRejected,
    KeyPassphraseRequired,
    KeyboardInteractive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshCredentialPrompt {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub connection_name: String,
    pub kind: SshCredentialPromptKind,
    pub reason: SshCredentialPromptReason,
    pub attempt: u32,
    pub prompt_text: Option<String>,
    pub echo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SshKeyboardInteractivePrompt {
    pub prompt: String,
    pub echo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SshKeyboardInteractiveRequest {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub connection_name: String,
    pub name: String,
    pub instructions: String,
    pub round: u32,
    pub prompts: Vec<SshKeyboardInteractivePrompt>,
    pub otp_id: Option<String>,
}

pub trait SshCredentialProvider: Send + Sync {
    fn request_secret(&self, prompt: &SshCredentialPrompt) -> Result<Option<String>, String>;

    fn request_keyboard_interactive(
        &self,
        request: &SshKeyboardInteractiveRequest,
    ) -> Result<Option<Vec<String>>, String> {
        let prompt_count = request.prompts.len();
        let mut responses = Vec::with_capacity(prompt_count);
        for (index, prompt) in request.prompts.iter().enumerate() {
            let response = self.request_secret(&SshCredentialPrompt {
                host: request.host.clone(),
                port: request.port,
                username: request.username.clone(),
                connection_name: request.connection_name.clone(),
                kind: SshCredentialPromptKind::KeyboardInteractive,
                reason: SshCredentialPromptReason::KeyboardInteractive,
                attempt: request.round,
                prompt_text: Some(format_keyboard_interactive_prompt(
                    &request.name,
                    &request.instructions,
                    &prompt.prompt,
                    index,
                    prompt_count,
                )),
                echo: prompt.echo,
            })?;
            let Some(response) = response else {
                return Ok(None);
            };
            responses.push(response);
        }
        Ok(Some(responses))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SshAgentPromptPhase {
    Connect,
    ListIdentities,
    Sign,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshAgentPrompt {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub connection_name: String,
    pub phase: SshAgentPromptPhase,
    pub attempt: u32,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshAgentPromptAction {
    Retry,
    Cancel,
}

/// A live SSH Agent authentication prompt that can be updated while signing.
pub trait SshAgentPromptRequest: Send + Sync {
    /// Blocks until the UI supplies an action or the prompt times out.
    fn wait_action(&self) -> Result<SshAgentPromptAction, String>;

    /// Changes the visible request from the pending state to a failed state.
    fn mark_failed(&self, prompt: &SshAgentPrompt) -> Result<(), String>;

    /// Resolves and removes the request from the UI.
    fn finish(&self);
}

pub trait SshAgentPromptProvider: Send + Sync {
    fn request_action(&self, prompt: &SshAgentPrompt) -> Result<SshAgentPromptAction, String>;

    /// Starts a request that can be observed while the Agent operation runs.
    ///
    /// Returning `Ok(None)` keeps compatibility with providers that only
    /// support the legacy failure-then-prompt flow.
    fn begin_request(
        &self,
        _prompt: &SshAgentPrompt,
    ) -> Result<Option<Arc<dyn SshAgentPromptRequest>>, String> {
        Ok(None)
    }
}

pub trait SshOtpProvider: Send + Sync {
    fn request_otp_code(&self, otp_id: &str) -> Result<Option<String>, String>;
}

impl Default for SerialSessionConfig {
    fn default() -> Self {
        Self {
            name: "Serial".to_string(),
            port_name: String::new(),
            baud_rate: 115_200,
            data_bits: 8,
            parity: "none".to_string(),
            stop_bits: "1".to_string(),
            backspace_mode: "ctrl_h".to_string(),
            encoding: "UTF-8".to_string(),
        }
    }
}

impl Default for SshSessionConfig {
    fn default() -> Self {
        Self {
            name: "SSH".to_string(),
            host: String::new(),
            port: 22,
            username: "root".to_string(),
            password: None,
            key_auth: None,
            agent_auth: false,
            agent_endpoint: SshAgentEndpoint::Auto,
            agent_forwarding: false,
            agent_forwarding_config: None,
            otp_id: None,
            auto_fill_otp: false,
            proxy_jump: None,
            proxy: None,
            allow_none_auth: false,
            backspace_mode: "del".to_string(),
            profile: SshSessionProfile::Standard,
            term: "xterm-256color".to_string(),
            x11_forwarding: false,
            x11_display: String::new(),
            encoding: "UTF-8".to_string(),
            ssh_algorithms: None,
            sftp: SftpSettings::default(),
            terminal_shell_integration: true,
            deferred_pty: false,
            keep_alive_interval_secs: 30,
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
            host_key_verifier: None,
            credential_provider: None,
            agent_prompt_provider: None,
            agent_stored_key_provider: None,
            otp_provider: None,
        }
    }
}

impl Default for TelnetSessionConfig {
    fn default() -> Self {
        Self {
            name: "Telnet".to_string(),
            host: String::new(),
            port: 23,
            username: String::new(),
            password: None,
            backspace_mode: "del".to_string(),
            raw_tcp: false,
            enter_mode: TelnetEnterMode::Cr,
            local_echo: false,
            local_line_edit: false,
            force_character_at_a_time: false,
            send_naws: true,
            send_sga: true,
            auto_login: TelnetAutoLoginConfig::default(),
            encoding: "UTF-8".to_string(),
            cols: 80,
            rows: 24,
        }
    }
}

impl Default for LocalSessionConfig {
    fn default() -> Self {
        Self {
            name: "Local Terminal".to_string(),
            shell_path: None,
            shell_args: Vec::new(),
            working_dir: None,
            encoding: "UTF-8".to_string(),
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SftpCwdFollowMode, SftpSettings, SshSessionConfig, SshSessionProfile, TelnetSessionConfig,
    };

    #[test]
    fn telnet_debug_output_redacts_password() {
        let secret = "nya-telnet-password-never-log";
        let config = TelnetSessionConfig {
            password: Some(secret.to_string()),
            ..TelnetSessionConfig::default()
        };
        let output = format!("{config:?}");

        assert!(!output.contains(secret));
        assert!(output.contains("<redacted>"));
    }

    #[test]
    fn network_device_runtime_capabilities_do_not_mutate_saved_settings() {
        let config = SshSessionConfig {
            profile: SshSessionProfile::NetworkDevice,
            sftp: SftpSettings {
                enabled: true,
                cwd_follow_mode: SftpCwdFollowMode::RcFile,
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(!config.remote_file_browser_enabled());
        assert!(!config.remote_stats_enabled());
        assert_eq!(config.effective_cwd_follow_mode(), SftpCwdFollowMode::Off);
        assert!(!config.effective_terminal_shell_integration());
        assert!(config.sftp.enabled);
        assert_eq!(config.sftp.cwd_follow_mode, SftpCwdFollowMode::RcFile);
    }

    #[test]
    fn standard_runtime_capabilities_remain_enabled() {
        let config = SshSessionConfig::default();
        assert!(config.remote_file_browser_enabled());
        assert!(config.remote_stats_enabled());
        assert_eq!(
            config.effective_cwd_follow_mode(),
            SftpCwdFollowMode::ShellIntegration
        );
        assert!(config.effective_terminal_shell_integration());
        assert_eq!(
            config.shell_integration_detection_timeout_ms(SftpCwdFollowMode::Off),
            1_000
        );
        assert_eq!(
            config.shell_integration_detection_timeout_ms(SftpCwdFollowMode::ShellIntegration),
            3_000
        );
    }
}
