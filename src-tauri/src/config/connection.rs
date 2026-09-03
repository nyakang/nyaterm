use super::{
    ProxyConfig, ProxySettings, load_app_settings, load_proxies, save_app_settings, save_proxies,
    uuid_v4,
};
use crate::core::{RecordingMode, RotationPolicy};
use crate::error::{AppError, AppResult};
use crate::storage;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use tauri::AppHandle;

// ── Connection type discriminator ───────────────────────────────────────────

/// Shell/CLI profile used when AI Agent mode injects executable commands.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AiExecutionProfile {
    #[default]
    Auto,
    Posix,
    Powershell,
    Cmd,
    SendOnly,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SshAlgorithmMode {
    #[default]
    Compatible,
    Secure,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SshAlgorithmPreferences {
    #[serde(default)]
    pub mode: SshAlgorithmMode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kex: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ciphers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub macs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_keys: Vec<String>,
}

/// Source used to connect to the local SSH Agent.
///
/// `Auto` selects the platform default: Unix uses `SSH_AUTH_SOCK`, while
/// Windows tries the OpenSSH Agent named pipe followed by Pageant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
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

/// Maximum number of custom forwarding Agent endpoints stored in one connection.
pub const MAX_SSH_AGENT_FORWARDING_ENDPOINTS: usize = 16;
pub const MAX_SSH_AGENT_FORWARDING_IDENTITIES: usize = 1024;
const MAX_SSH_AGENT_ENVIRONMENT_VARIABLE_LEN: usize = 255;
const MAX_SSH_AGENT_UNIX_SOCKET_PATH_LEN: usize = 4096;

/// Validates a single SSH Agent endpoint before it reaches persistent storage.
pub fn validate_ssh_agent_endpoint(endpoint: &SshAgentEndpoint) -> AppResult<()> {
    validate_ssh_agent_endpoint_shape(endpoint)?;
    match endpoint {
        SshAgentEndpoint::Auto => {
            if !cfg!(unix) && !cfg!(windows) {
                return Err(AppError::Config(
                    "Automatic SSH Agent endpoints are not supported on this platform".to_string(),
                ));
            }
        }
        SshAgentEndpoint::Environment { .. } => {
            if !cfg!(unix) {
                return Err(AppError::Config(
                    "Environment variable SSH Agent endpoints are only supported on Unix"
                        .to_string(),
                ));
            }
        }
        SshAgentEndpoint::UnixSocket { .. } => {
            if !cfg!(unix) {
                return Err(AppError::Config(
                    "Unix socket SSH Agent endpoints are only supported on Unix".to_string(),
                ));
            }
        }
        SshAgentEndpoint::Pageant => {
            if !cfg!(windows) {
                return Err(AppError::Config(
                    "Pageant SSH Agent endpoints are only supported on Windows".to_string(),
                ));
            }
        }
        SshAgentEndpoint::WindowsOpenSsh => {
            if !cfg!(windows) {
                return Err(AppError::Config(
                    "Windows OpenSSH Agent endpoints are only supported on Windows".to_string(),
                ));
            }
        }
    }

    Ok(())
}

/// Validates endpoint values without applying the current platform restriction.
pub fn validate_ssh_agent_endpoint_shape(endpoint: &SshAgentEndpoint) -> AppResult<()> {
    match endpoint {
        SshAgentEndpoint::Environment { variable } => {
            let Some(variable) = normalize_ssh_agent_environment_variable(variable) else {
                return Err(AppError::Config(
                    "SSH Agent environment variable must not be empty or contain '=' or NUL"
                        .to_string(),
                ));
            };
            if variable.len() > MAX_SSH_AGENT_ENVIRONMENT_VARIABLE_LEN {
                return Err(AppError::Config(format!(
                    "SSH Agent environment variable must not exceed {MAX_SSH_AGENT_ENVIRONMENT_VARIABLE_LEN} bytes"
                )));
            }
        }
        SshAgentEndpoint::UnixSocket { path } => {
            if path.contains('\0') {
                return Err(AppError::Config(
                    "SSH Agent Unix socket path must not contain NUL".to_string(),
                ));
            }
            if path.trim().is_empty() {
                return Err(AppError::Config(
                    "SSH Agent Unix socket path must not be empty".to_string(),
                ));
            }
            if path.len() > MAX_SSH_AGENT_UNIX_SOCKET_PATH_LEN {
                return Err(AppError::Config(format!(
                    "SSH Agent Unix socket path must not exceed {MAX_SSH_AGENT_UNIX_SOCKET_PATH_LEN} bytes"
                )));
            }
        }
        SshAgentEndpoint::Auto | SshAgentEndpoint::Pageant | SshAgentEndpoint::WindowsOpenSsh => {}
    }
    Ok(())
}

fn normalize_ssh_agent_environment_variable(value: &str) -> Option<String> {
    let variable = value.trim().trim_start_matches('$').trim();
    if variable.is_empty() || variable.contains('=') || variable.contains('\0') {
        return None;
    }
    Some(variable.to_string())
}

/// Returns a normalized, non-secret key for comparing Agent endpoints.
pub fn ssh_agent_endpoint_key(endpoint: &SshAgentEndpoint) -> String {
    match endpoint {
        SshAgentEndpoint::Auto if cfg!(unix) => "environment:SSH_AUTH_SOCK".to_string(),
        SshAgentEndpoint::Auto => "auto".to_string(),
        SshAgentEndpoint::Environment { variable } => format!(
            "environment:{}",
            normalize_ssh_agent_environment_variable(variable)
                .unwrap_or_else(|| variable.trim().to_string())
        ),
        SshAgentEndpoint::UnixSocket { path } => format!("unix_socket:{path}"),
        SshAgentEndpoint::Pageant => "pageant".to_string(),
        SshAgentEndpoint::WindowsOpenSsh => "windows_open_ssh".to_string(),
    }
}

/// Validates SSH Agent fields at the connection save boundary.
pub fn validate_ssh_agent_settings(connection: &ConnectionType) -> AppResult<()> {
    let ConnectionType::Ssh {
        auth_agent_endpoint,
        agent_forwarding_config,
        ..
    } = connection
    else {
        return Ok(());
    };

    if let Some(endpoint) = auth_agent_endpoint {
        validate_ssh_agent_endpoint(endpoint)?;
    }

    if let Some(config) = agent_forwarding_config {
        validate_ssh_agent_forwarding_config(config)?;
    }

    Ok(())
}

/// Validates the forwarding configuration used by persistence, runtime, and IPC paths.
pub fn validate_ssh_agent_forwarding_config(config: &SshAgentForwardingConfig) -> AppResult<()> {
    validate_ssh_agent_forwarding_shape(config)?;

    for endpoint in &config.sources.external_agent_endpoints {
        validate_ssh_agent_endpoint(endpoint)?;
    }

    Ok(())
}

/// Validates forwarding values without applying the current platform restriction.
pub fn validate_ssh_agent_forwarding_shape(config: &SshAgentForwardingConfig) -> AppResult<()> {
    let endpoints = &config.sources.external_agent_endpoints;
    if endpoints.len() > MAX_SSH_AGENT_FORWARDING_ENDPOINTS {
        return Err(AppError::Config(format!(
            "SSH Agent forwarding supports at most {MAX_SSH_AGENT_FORWARDING_ENDPOINTS} custom endpoints"
        )));
    }

    let mut seen = HashSet::with_capacity(endpoints.len());
    for endpoint in endpoints {
        validate_ssh_agent_endpoint_shape(endpoint)?;
        let key = ssh_agent_endpoint_key(endpoint);
        if !seen.insert(key) {
            return Err(AppError::Config(
                "SSH Agent forwarding endpoints must be unique".to_string(),
            ));
        }
    }

    if let SshAgentForwardingPolicy::Allowlist { fingerprints } = &config.policy {
        if fingerprints.len() > MAX_SSH_AGENT_FORWARDING_IDENTITIES {
            return Err(AppError::Config(format!(
                "SSH Agent forwarding allowlists support at most {MAX_SSH_AGENT_FORWARDING_IDENTITIES} identities"
            )));
        }
        let mut seen = HashSet::with_capacity(fingerprints.len());
        for fingerprint in fingerprints {
            if fingerprint.is_empty() || fingerprint.len() > 255 {
                return Err(AppError::Config(
                    "SSH Agent forwarding fingerprints must be between 1 and 255 bytes".to_string(),
                ));
            }
            if !seen.insert(fingerprint) {
                return Err(AppError::Config(
                    "SSH Agent forwarding fingerprints must be unique".to_string(),
                ));
            }
        }
    }

    Ok(())
}

/// Policy applied to identities merged from the external Agent and stored keys.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SshAgentForwardingPolicy {
    Allowlist {
        #[serde(default)]
        fingerprints: Vec<String>,
    },
    All,
}

impl Default for SshAgentForwardingPolicy {
    fn default() -> Self {
        Self::Allowlist {
            fingerprints: Vec::new(),
        }
    }
}

/// External Agent sources and stored-key sources used by Agent forwarding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SshAgentForwardingSources {
    #[serde(default)]
    pub external_agent: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_agent_endpoints: Vec<SshAgentEndpoint>,
    #[serde(default = "default_true")]
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

/// Agent forwarding configuration stored independently from the authentication Agent endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SshAgentForwardingConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub sources: SshAgentForwardingSources,
    #[serde(default)]
    pub policy: SshAgentForwardingPolicy,
}

impl Default for SshAgentForwardingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            sources: SshAgentForwardingSources::default(),
            policy: SshAgentForwardingPolicy::default(),
        }
    }
}

/// Migrates legacy Agent fields and normalizes authentication-dependent settings.
///
/// The migration is intentionally one-way: legacy fields are accepted on input but are never
/// serialized again. Existing forwarding behavior is preserved without exposing stored keys.
#[allow(deprecated)]
pub fn migrate_legacy_ssh_agent_settings(connection: &mut SavedConnection) -> bool {
    let auth_mode = connection.auth.as_ref().map(|auth| auth.mode.as_str());

    let ConnectionType::Ssh {
        auth_agent_endpoint,
        legacy_agent_forwarding,
        agent_forwarding_config,
        ..
    } = &mut connection.config
    else {
        return false;
    };

    let mut changed = false;
    if agent_forwarding_config.is_none() && *legacy_agent_forwarding == Some(true) {
        let endpoint = auth_agent_endpoint.clone().unwrap_or_default();
        *agent_forwarding_config = Some(SshAgentForwardingConfig {
            enabled: true,
            sources: SshAgentForwardingSources {
                external_agent: true,
                external_agent_endpoints: vec![endpoint],
                stored_keys: false,
            },
            policy: SshAgentForwardingPolicy::All,
        });
        changed = true;
    }

    if legacy_agent_forwarding.take().is_some() {
        changed = true;
    }

    if auth_mode == Some("agent") {
        if auth_agent_endpoint.is_none() {
            *auth_agent_endpoint = Some(SshAgentEndpoint::Auto);
            changed = true;
        }
    } else if auth_agent_endpoint.take().is_some() {
        changed = true;
    }

    changed
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SftpCwdFollowMode {
    Off,
    #[default]
    ShellIntegration,
    RcFile,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SshProfile {
    #[default]
    Standard,
    NetworkDevice,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SshRuntimeMode {
    #[default]
    Standard,
    Terminal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum SshTerminalType {
    #[default]
    #[serde(rename = "xterm-256color")]
    Xterm256Color,
    #[serde(rename = "xterm")]
    Xterm,
    #[serde(rename = "vt100")]
    Vt100,
    #[serde(rename = "vt220")]
    Vt220,
    #[serde(rename = "ansi")]
    Ansi,
    #[serde(rename = "linux")]
    Linux,
}

impl SshTerminalType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Xterm256Color => "xterm-256color",
            Self::Xterm => "xterm",
            Self::Vt100 => "vt100",
            Self::Vt220 => "vt220",
            Self::Ansi => "ansi",
            Self::Linux => "linux",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SftpSettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub cwd_follow_mode: SftpCwdFollowMode,
    #[serde(
        default = "default_sftp_shell_detection_timeout_ms",
        skip_serializing_if = "is_default_sftp_shell_detection_timeout_ms"
    )]
    pub shell_detection_timeout_ms: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub filename_encoding: String,
    #[serde(
        default,
        deserialize_with = "deserialize_sftp_pipeline_depth",
        skip_serializing_if = "Option::is_none"
    )]
    pub pipeline_depth: Option<u32>,
}

impl Default for SftpSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            cwd_follow_mode: SftpCwdFollowMode::ShellIntegration,
            shell_detection_timeout_ms: default_sftp_shell_detection_timeout_ms(),
            filename_encoding: String::new(),
            pipeline_depth: None,
        }
    }
}

pub fn effective_cwd_follow_mode(settings: &SftpSettings) -> SftpCwdFollowMode {
    if settings.enabled {
        settings.cwd_follow_mode.clone()
    } else {
        SftpCwdFollowMode::Off
    }
}

pub fn effective_cwd_follow_mode_for_profile(
    settings: &SftpSettings,
    profile: &SshProfile,
) -> SftpCwdFollowMode {
    if *profile == SshProfile::NetworkDevice {
        SftpCwdFollowMode::Off
    } else {
        effective_cwd_follow_mode(settings)
    }
}

pub fn effective_cwd_follow_mode_for_runtime(
    settings: &SftpSettings,
    profile: &SshProfile,
    runtime_mode: &SshRuntimeMode,
) -> SftpCwdFollowMode {
    if *runtime_mode == SshRuntimeMode::Terminal {
        SftpCwdFollowMode::Off
    } else {
        effective_cwd_follow_mode_for_profile(settings, profile)
    }
}

pub fn default_terminal_type_for_profile(profile: &SshProfile) -> SshTerminalType {
    match profile {
        SshProfile::Standard => SshTerminalType::Xterm256Color,
        SshProfile::NetworkDevice => SshTerminalType::Vt100,
    }
}

pub fn resolve_ssh_terminal_type(
    profile: &SshProfile,
    terminal_type: Option<&SshTerminalType>,
) -> SshTerminalType {
    terminal_type
        .cloned()
        .unwrap_or_else(|| default_terminal_type_for_profile(profile))
}

pub const MIN_SFTP_SHELL_DETECTION_TIMEOUT_MS: u64 = 100;
pub const MAX_SFTP_SHELL_DETECTION_TIMEOUT_MS: u64 = 60_000;
pub const MIN_SFTP_PIPELINE_DEPTH: u32 = 4;
pub const MAX_SFTP_PIPELINE_DEPTH: u32 = 64;

fn deserialize_sftp_pipeline_depth<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum PipelineDepthValue {
        Signed(i64),
        Unsigned(u64),
        Invalid(serde::de::IgnoredAny),
    }

    let value = Option::<PipelineDepthValue>::deserialize(deserializer)?;
    Ok(match value {
        Some(PipelineDepthValue::Signed(value)) => Some(value.clamp(
            i64::from(MIN_SFTP_PIPELINE_DEPTH),
            i64::from(MAX_SFTP_PIPELINE_DEPTH),
        ) as u32),
        Some(PipelineDepthValue::Unsigned(value)) => Some(value.clamp(
            u64::from(MIN_SFTP_PIPELINE_DEPTH),
            u64::from(MAX_SFTP_PIPELINE_DEPTH),
        ) as u32),
        Some(PipelineDepthValue::Invalid(_)) | None => None,
    })
}

pub fn default_sftp_shell_detection_timeout_ms() -> u64 {
    3000
}

fn is_default_sftp_shell_detection_timeout_ms(value: &u64) -> bool {
    *value == default_sftp_shell_detection_timeout_ms()
}

fn is_default_sftp_settings(value: &SftpSettings) -> bool {
    value == &SftpSettings::default()
}

fn is_standard_ssh_profile(value: &SshProfile) -> bool {
    *value == SshProfile::Standard
}

/// Type-specific configuration for each connection kind.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConnectionType {
    Ssh {
        host: String,
        #[serde(default = "default_ssh_port")]
        port: u16,
        #[serde(default = "default_ssh_user")]
        username: String,
        #[serde(default = "default_backspace_mode_ssh")]
        backspace_mode: String,
        #[serde(default, skip_serializing_if = "is_false")]
        x11_forwarding: bool,
        #[serde(
            default,
            alias = "agent_endpoint",
            skip_serializing_if = "Option::is_none"
        )]
        auth_agent_endpoint: Option<SshAgentEndpoint>,
        #[deprecated(note = "legacy read-only compatibility field; use agent_forwarding_config")]
        #[serde(default, rename = "agent_forwarding", skip_serializing)]
        legacy_agent_forwarding: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_forwarding_config: Option<SshAgentForwardingConfig>,
        #[serde(default)]
        encoding: String,
        /// Allow sanitized remote OSC 0/2 titles to decorate this saved SSH tab.
        #[serde(default, skip_serializing_if = "is_false")]
        dynamic_tab_title: bool,
    },
    LocalTerminal {
        #[serde(default)]
        shell_path: String,
        #[serde(default)]
        shell_args: String,
        #[serde(default)]
        working_dir: Option<String>,
        #[serde(default, skip_serializing_if = "is_ai_execution_profile_auto")]
        ai_execution_profile: AiExecutionProfile,
        #[serde(default)]
        encoding: String,
        /// When true, the tab label follows the session's dynamic window
        /// title (OSC 0/2) / cwd instead of the static connection name.
        #[serde(default, skip_serializing_if = "is_false")]
        dynamic_tab_title: bool,
    },
    Telnet {
        host: String,
        #[serde(default = "default_telnet_port")]
        port: u16,
        #[serde(default)]
        username: String,
        #[serde(default, skip_serializing_if = "is_ai_execution_profile_auto")]
        ai_execution_profile: AiExecutionProfile,
        #[serde(default = "default_backspace_mode_telnet")]
        backspace_mode: String,
        #[serde(default, skip_serializing_if = "is_false")]
        raw_tcp_cli: bool,
        #[serde(
            default = "default_telnet_enter_mode",
            skip_serializing_if = "is_default_telnet_enter_mode"
        )]
        enter_mode: String,
        #[serde(default, skip_serializing_if = "is_false")]
        local_echo: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        local_line_edit: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        force_character_at_a_time: bool,
        #[serde(default = "default_true", skip_serializing_if = "is_true")]
        send_naws: bool,
        #[serde(default = "default_true", skip_serializing_if = "is_true")]
        send_sga: bool,
        #[serde(default, skip_serializing_if = "is_default_telnet_auto_login")]
        auto_login: TelnetAutoLoginConfig,
        #[serde(default)]
        encoding: String,
    },
    Serial {
        port_name: String,
        #[serde(default = "default_baud_rate")]
        baud_rate: u32,
        #[serde(default = "default_data_bits")]
        data_bits: u8,
        #[serde(default = "default_parity")]
        parity: String,
        #[serde(default = "default_stop_bits")]
        stop_bits: String,
        #[serde(default, skip_serializing_if = "is_ai_execution_profile_auto")]
        ai_execution_profile: AiExecutionProfile,
        #[serde(default = "default_backspace_mode_serial")]
        backspace_mode: String,
        #[serde(default)]
        encoding: String,
    },
    Rdp {
        host: String,
        #[serde(default = "default_rdp_port")]
        port: u16,
        #[serde(default)]
        username: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        domain: String,
        #[serde(default)]
        security: RdpSecuritySettings,
        #[serde(default)]
        display: RdpDisplaySettings,
        #[serde(default)]
        clipboard: RdpClipboardSettings,
        #[serde(default)]
        reconnect: RdpReconnectSettings,
        #[serde(default, skip_serializing_if = "is_false")]
        admin: bool,
    },
    Vnc {
        host: String,
        #[serde(default = "default_vnc_port")]
        port: u16,
        #[serde(default)]
        security: VncSecuritySettings,
        #[serde(default)]
        display: VncDisplaySettings,
        #[serde(default)]
        clipboard: VncClipboardSettings,
        #[serde(default)]
        reconnect: VncReconnectSettings,
        #[serde(default = "default_true")]
        shared: bool,
        #[serde(default)]
        view_only: bool,
    },
}

fn default_ssh_port() -> u16 {
    22
}
fn default_ssh_user() -> String {
    "root".to_string()
}
fn default_backspace_mode_ssh() -> String {
    "del".to_string()
}
fn default_telnet_port() -> u16 {
    23
}
fn default_rdp_port() -> u16 {
    3389
}
fn default_vnc_port() -> u16 {
    5900
}
fn default_baud_rate() -> u32 {
    115_200
}
fn default_data_bits() -> u8 {
    8
}
fn default_parity() -> String {
    "none".to_string()
}
fn default_stop_bits() -> String {
    "1".to_string()
}
fn default_backspace_mode_serial() -> String {
    "ctrl_h".to_string()
}
fn default_backspace_mode_telnet() -> String {
    "del".to_string()
}
fn default_telnet_enter_mode() -> String {
    "cr".to_string()
}
fn is_ai_execution_profile_auto(value: &AiExecutionProfile) -> bool {
    *value == AiExecutionProfile::Auto
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RdpSecuritySettings {
    #[serde(default = "default_true")]
    pub use_nla: bool,
    #[serde(default = "default_rdp_certificate_policy")]
    pub certificate_policy: String,
}

impl Default for RdpSecuritySettings {
    fn default() -> Self {
        Self {
            use_nla: true,
            certificate_policy: default_rdp_certificate_policy(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RdpDisplaySettings {
    #[serde(default = "default_rdp_display_mode")]
    pub mode: String,
    #[serde(default = "default_rdp_width")]
    pub width: u32,
    #[serde(default = "default_rdp_height")]
    pub height: u32,
    #[serde(default = "default_rdp_color_depth")]
    pub color_depth: u8,
}

impl Default for RdpDisplaySettings {
    fn default() -> Self {
        Self {
            mode: default_rdp_display_mode(),
            width: default_rdp_width(),
            height: default_rdp_height(),
            color_depth: default_rdp_color_depth(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RdpClipboardSettings {
    #[serde(default = "default_rdp_clipboard_mode")]
    pub mode: String,
}

impl Default for RdpClipboardSettings {
    fn default() -> Self {
        Self {
            mode: default_rdp_clipboard_mode(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RdpReconnectSettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_rdp_reconnect_attempts")]
    pub max_attempts: u32,
}

impl Default for RdpReconnectSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            max_attempts: default_rdp_reconnect_attempts(),
        }
    }
}

fn default_rdp_certificate_policy() -> String {
    "prompt".to_string()
}
fn default_rdp_display_mode() -> String {
    "fit-window".to_string()
}
fn default_rdp_width() -> u32 {
    1920
}
fn default_rdp_height() -> u32 {
    1080
}
fn default_rdp_color_depth() -> u8 {
    32
}
fn default_rdp_clipboard_mode() -> String {
    "text-only".to_string()
}
fn default_rdp_reconnect_attempts() -> u32 {
    5
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VncSecuritySettings {
    #[serde(default = "default_vnc_security_mode")]
    pub mode: String,
}

impl Default for VncSecuritySettings {
    fn default() -> Self {
        Self {
            mode: default_vnc_security_mode(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VncDisplaySettings {
    #[serde(default = "default_vnc_scale_mode")]
    pub scale_mode: String,
}

impl Default for VncDisplaySettings {
    fn default() -> Self {
        Self {
            scale_mode: default_vnc_scale_mode(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VncClipboardSettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for VncClipboardSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VncReconnectSettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_vnc_reconnect_attempts")]
    pub max_attempts: u32,
}

impl Default for VncReconnectSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            max_attempts: default_vnc_reconnect_attempts(),
        }
    }
}

fn default_vnc_security_mode() -> String {
    "auto".to_string()
}
fn default_vnc_scale_mode() -> String {
    "fit".to_string()
}
fn default_vnc_reconnect_attempts() -> u32 {
    5
}

// ── Auth block ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConnectionAuth {
    #[serde(default = "default_auth_mode")]
    pub mode: String,
    #[serde(default)]
    pub password_id: Option<String>,
    /// Inline password: AES-encrypted on disk, plaintext from frontend during save.
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub key_id: Option<String>,
    #[serde(default)]
    pub otp_id: Option<String>,
    #[serde(default)]
    pub auto_fill_otp: bool,
    /// Transient flag: true when an inline password exists on disk.
    #[serde(default, skip_serializing_if = "is_false")]
    pub has_password: bool,
}

fn default_auth_mode() -> String {
    "password".to_string()
}

fn is_false(value: &bool) -> bool {
    !*value
}
fn is_true(value: &bool) -> bool {
    *value
}
fn is_default_telnet_enter_mode(value: &str) -> bool {
    value == "cr"
}

// ── Telnet auto-login ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TelnetAutoLoginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub send_wake_enter: bool,
    #[serde(default = "default_telnet_auto_login_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub username_prompt_regex: Option<String>,
    #[serde(default)]
    pub password_prompt_regex: Option<String>,
    #[serde(default)]
    pub success_prompt_regex: Option<String>,
    #[serde(default)]
    pub failure_prompt_regex: Option<String>,
    #[serde(default)]
    pub max_retries: u8,
}

impl Default for TelnetAutoLoginConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            send_wake_enter: true,
            timeout_ms: default_telnet_auto_login_timeout_ms(),
            username_prompt_regex: None,
            password_prompt_regex: None,
            success_prompt_regex: None,
            failure_prompt_regex: None,
            max_retries: 0,
        }
    }
}

fn default_telnet_auto_login_timeout_ms() -> u64 {
    60_000
}

fn is_default_telnet_auto_login(value: &TelnetAutoLoginConfig) -> bool {
    value == &TelnetAutoLoginConfig::default()
}

// ── Network block ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConnectionNetwork {
    #[serde(default)]
    pub proxy_id: Option<String>,
    #[serde(default)]
    pub proxy_jump_id: Option<String>,
}

// ── Post-login automation ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectionPostLogin {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub command: String,
    #[serde(default = "default_post_login_delay_ms")]
    pub delay_ms: u64,
}

fn default_post_login_delay_ms() -> u64 {
    1000
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ConnectionRecordingSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_start: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<RecordingMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_timestamps: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<RotationPolicy>,
}

// ── Static asset metadata ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AssetDeviceType {
    Physical,
    Virtual,
    Cloud,
    Network,
    Storage,
    Embedded,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AssetAcceleratorType {
    Gpu,
    Npu,
    #[default]
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssetAccelerator {
    #[serde(default)]
    pub r#type: AssetAcceleratorType,
    #[serde(default)]
    pub vendor: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub count: Option<u32>,
    #[serde(default)]
    pub memory_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AssetDiskKind {
    Hdd,
    Ssd,
    Nvme,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AssetDiskPurpose {
    System,
    Data,
    Cache,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssetDisk {
    #[serde(default)]
    pub kind: Option<AssetDiskKind>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub capacity_bytes: Option<u64>,
    #[serde(default)]
    pub count: Option<u32>,
    #[serde(default)]
    pub purpose: Option<AssetDiskPurpose>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AssetMetadata {
    #[serde(default)]
    pub device_type: Option<AssetDeviceType>,
    #[serde(default)]
    pub os_name: Option<String>,
    #[serde(default)]
    pub os_version: Option<String>,
    #[serde(default)]
    pub architecture: Option<String>,
    #[serde(default)]
    pub kernel_version: Option<String>,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub cpu_model: Option<String>,
    #[serde(default)]
    pub cpu_sockets: Option<u32>,
    #[serde(default)]
    pub cpu_cores: Option<u32>,
    #[serde(default)]
    pub cpu_threads: Option<u32>,
    #[serde(default)]
    pub memory_bytes: Option<u64>,
    #[serde(default)]
    pub accelerators: Option<Vec<AssetAccelerator>>,
    #[serde(default)]
    pub disks: Option<Vec<AssetDisk>>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

// ── Saved connection ────────────────────────────────────────────────────────

/// Unified saved connection: common fields + type-discriminated config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedConnection {
    #[serde(default = "uuid_v4")]
    pub id: String,
    pub name: String,

    #[serde(flatten)]
    pub config: ConnectionType,

    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub sort_order: i32,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_auto_detect: Option<bool>,

    #[serde(default)]
    pub auth: Option<ConnectionAuth>,
    #[serde(default)]
    pub network: Option<ConnectionNetwork>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_login: Option<ConnectionPostLogin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<ConnectionRecordingSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_algorithms: Option<SshAlgorithmPreferences>,
    #[serde(default, skip_serializing_if = "is_standard_ssh_profile")]
    pub ssh_profile: SshProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_type: Option<SshTerminalType>,
    #[serde(default, skip_serializing_if = "is_default_sftp_settings")]
    pub sftp: SftpSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<AssetMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at_ms: Option<u64>,
}

/// Group for organizing saved connections in the UI.
/// Groups form a tree via `parent_id`; root groups have `parent_id = None`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    #[serde(default = "uuid_v4")]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub sort_order: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at_ms: Option<u64>,
}

/// Custom icon imported into the shared connection icon library.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectionCustomIcon {
    pub id: String,
    pub name: String,
    pub data_url: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

/// Root config for groups and saved connections.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionsConfig {
    #[serde(default)]
    pub groups: Vec<Group>,
    #[serde(default)]
    pub connections: Vec<SavedConnection>,
    #[serde(default)]
    pub custom_icons: Vec<ConnectionCustomIcon>,
}

/// Alias for the main app config (sessions + groups).
pub type AppConfig = SessionsConfig;

pub fn is_connection_custom_icon_data_url(value: &str) -> bool {
    let Some((header, encoded)) = value.trim().split_once(',') else {
        return false;
    };
    if encoded.is_empty()
        || !encoded
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
    {
        return false;
    }

    matches!(
        header.to_ascii_lowercase().as_str(),
        "data:image/png;base64"
            | "data:image/jpeg;base64"
            | "data:image/jpg;base64"
            | "data:image/webp;base64"
            | "data:image/bmp;base64"
            | "data:image/gif;base64"
    )
}

pub fn connection_custom_icon_id_for_data_url(data_url: &str) -> String {
    let digest = Sha256::digest(data_url.trim().as_bytes());
    format!("custom-icon-{}", hex::encode(digest))
}

pub fn connection_custom_icon_from_data_url(
    data_url: &str,
    name: impl Into<String>,
    now_ms: u64,
) -> Option<ConnectionCustomIcon> {
    let data_url = data_url.trim();
    if !is_connection_custom_icon_data_url(data_url) {
        return None;
    }

    let name = name.into();
    Some(ConnectionCustomIcon {
        id: connection_custom_icon_id_for_data_url(data_url),
        name: if name.trim().is_empty() {
            "Custom icon".to_string()
        } else {
            name.trim().to_string()
        },
        data_url: data_url.to_string(),
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
    })
}

// ── Loading / saving ────────────────────────────────────────────────────────

pub fn load_sessions(app: &AppHandle) -> AppResult<SessionsConfig> {
    let mut config = storage::load_sessions()?;
    let mut migrated = false;
    for connection in &mut config.connections {
        migrated |= migrate_legacy_ssh_agent_settings(connection);
    }
    if migrated {
        save_sessions(app, &config)?;
    }
    Ok(config)
}

/// Saves sessions config to disk.
pub fn save_sessions(app: &AppHandle, config: &SessionsConfig) -> AppResult<()> {
    let _ = app;
    let mut sanitized = config.clone();
    for conn in &mut sanitized.connections {
        match &mut conn.config {
            ConnectionType::LocalTerminal {
                ai_execution_profile,
                ..
            }
            | ConnectionType::Telnet {
                ai_execution_profile,
                ..
            }
            | ConnectionType::Serial {
                ai_execution_profile,
                ..
            } => {
                *ai_execution_profile = AiExecutionProfile::Auto;
            }
            ConnectionType::Ssh { .. } => {
                migrate_legacy_ssh_agent_settings(conn);
            }
            ConnectionType::Rdp { .. } | ConnectionType::Vnc { .. } => {}
        }
        validate_ssh_agent_settings(&conn.config)?;
        if let Some(auth) = &mut conn.auth {
            auth.has_password = false;
        }
    }
    storage::replace_sessions(&sanitized)
}

/// Loads the main app config (sessions + groups).
/// Also runs one-time migrations.
pub fn load_config(app: &AppHandle) -> AppResult<AppConfig> {
    let mut cfg = load_sessions(app)?;

    migrate_global_proxy_to_connections(app, &mut cfg)?;
    migrate_connection_proxies_to_standalone(app, &mut cfg)?;

    Ok(cfg)
}

// ── Config migrations ───────────────────────────────────────────────────────

fn migrate_global_proxy_to_connections(app: &AppHandle, cfg: &mut SessionsConfig) -> AppResult<()> {
    let mut settings = load_app_settings(app)?;
    if !settings.proxy.enabled || cfg.connections.is_empty() {
        return Ok(());
    }

    let legacy_proxy = settings.proxy.clone();
    let mut migrated = false;

    let mut proxies = load_proxies(app).unwrap_or_default();
    let proxy_id = uuid::Uuid::new_v4().to_string();
    proxies.push(ProxyConfig {
        id: proxy_id.clone(),
        name: "Migrated Global Proxy".to_string(),
        protocol: legacy_proxy.protocol,
        host: legacy_proxy.host,
        port: legacy_proxy.port,
        command: None,
        username: None,
        password: None,
        group_id: None,
    });

    for conn in &mut cfg.connections {
        let has_proxy = conn.network.as_ref().is_some_and(|n| n.proxy_id.is_some());
        if !has_proxy {
            let net = conn.network.get_or_insert_with(ConnectionNetwork::default);
            net.proxy_id = Some(proxy_id.clone());
            migrated = true;
        }
    }

    if migrated {
        save_proxies(app, &proxies)?;
        save_sessions(app, cfg)?;
    }

    settings.proxy = ProxySettings::default();
    save_app_settings(app, &settings)?;

    tracing::info!("Migrated legacy global proxy settings to per-connection proxy configs");
    Ok(())
}

fn migrate_connection_proxies_to_standalone(
    _app: &AppHandle,
    _cfg: &mut SessionsConfig,
) -> AppResult<()> {
    // Legacy `network.proxy` inline objects are no longer present in the new format.
    // The old migration already ran before this format change, so nothing to do.
    Ok(())
}

/// Returns the effective encoding for a connection: per-connection override if set,
/// otherwise the global default from `interaction.default_encoding`.
pub fn resolve_connection_encoding(app: &AppHandle, conn: &SavedConnection) -> String {
    let per_conn = match &conn.config {
        ConnectionType::Ssh { encoding, .. }
        | ConnectionType::LocalTerminal { encoding, .. }
        | ConnectionType::Telnet { encoding, .. }
        | ConnectionType::Serial { encoding, .. } => encoding.as_str(),
        ConnectionType::Rdp { .. } | ConnectionType::Vnc { .. } => "",
    };
    if !per_conn.is_empty() {
        return per_conn.to_string();
    }
    crate::config::load_app_settings(app)
        .map(|s| s.interaction.default_encoding)
        .unwrap_or_else(|_| "UTF-8".to_string())
}

/// Loads a single connection by ID.
///
/// Returns `AppError::SessionNotFound` if no connection with that ID exists.
pub fn load_connection_by_id(app: &AppHandle, id: &str) -> AppResult<SavedConnection> {
    let mut conn = storage::get_connection(id)?
        .ok_or_else(|| AppError::SessionNotFound(format!("Connection '{}' not found", id)))?;
    if migrate_legacy_ssh_agent_settings(&mut conn) {
        let mut config = storage::load_sessions()?;
        if let Some(stored) = config.connections.iter_mut().find(|stored| stored.id == id) {
            *stored = conn.clone();
            save_sessions(app, &config)?;
        }
    }
    Ok(conn)
}

/// Saves the main app config.
pub fn save_config(app: &AppHandle, config: &AppConfig) -> AppResult<()> {
    save_sessions(app, config)
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::{
        AssetAcceleratorType, AssetDeviceType, AssetDiskKind, AssetDiskPurpose, ConnectionType,
        MAX_SFTP_PIPELINE_DEPTH, MAX_SSH_AGENT_ENVIRONMENT_VARIABLE_LEN,
        MAX_SSH_AGENT_FORWARDING_ENDPOINTS, MAX_SSH_AGENT_FORWARDING_IDENTITIES,
        MAX_SSH_AGENT_UNIX_SOCKET_PATH_LEN, MIN_SFTP_PIPELINE_DEPTH, SavedConnection,
        SftpCwdFollowMode, SftpSettings, SshAgentEndpoint, SshAgentForwardingConfig,
        SshAgentForwardingPolicy, SshAgentForwardingSources, SshAlgorithmMode, SshProfile,
        SshTerminalType, effective_cwd_follow_mode, effective_cwd_follow_mode_for_profile,
        migrate_legacy_ssh_agent_settings, resolve_ssh_terminal_type, validate_ssh_agent_endpoint,
        validate_ssh_agent_settings,
    };

    #[test]
    fn saved_connection_defaults_missing_post_login_to_none() {
        let connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-1",
            "name": "Test",
            "type": "ssh",
            "host": "example.com",
            "port": 22,
            "username": "root"
        }))
        .expect("connection");

        assert!(matches!(connection.config, ConnectionType::Ssh { .. }));
        assert!(connection.post_login.is_none());
    }

    #[test]
    fn saved_connection_defaults_missing_ssh_algorithms_to_compatible_runtime() {
        let connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-1",
            "name": "Test",
            "type": "ssh",
            "host": "example.com",
            "port": 22,
            "username": "root"
        }))
        .expect("connection");

        assert!(matches!(connection.config, ConnectionType::Ssh { .. }));
        assert!(connection.ssh_algorithms.is_none());
        assert_eq!(SshAlgorithmMode::default(), SshAlgorithmMode::Compatible);
    }

    #[test]
    fn legacy_agent_endpoint_deserializes_into_auth_endpoint() {
        let connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-1",
            "name": "Agent",
            "type": "ssh",
            "host": "example.com",
            "agent_endpoint": {
                "type": "unix_socket",
                "path": "/tmp/agent.sock"
            }
        }))
        .expect("connection");

        let ConnectionType::Ssh {
            auth_agent_endpoint,
            ..
        } = connection.config
        else {
            panic!("expected ssh connection");
        };

        assert_eq!(
            auth_agent_endpoint,
            Some(SshAgentEndpoint::UnixSocket {
                path: "/tmp/agent.sock".to_string()
            })
        );
    }

    #[test]
    fn missing_or_false_legacy_forwarding_stays_disabled() {
        for legacy_value in [None, Some(false)] {
            let mut value = serde_json::json!({
                "id": "conn-1",
                "name": "Agent",
                "type": "ssh",
                "host": "example.com"
            });
            if let Some(enabled) = legacy_value {
                value["agent_forwarding"] = serde_json::json!(enabled);
            }
            let mut connection: SavedConnection =
                serde_json::from_value(value).expect("connection");

            migrate_legacy_ssh_agent_settings(&mut connection);

            let ConnectionType::Ssh {
                auth_agent_endpoint,
                agent_forwarding_config,
                ..
            } = connection.config
            else {
                panic!("expected SSH connection");
            };
            assert!(auth_agent_endpoint.is_none());
            assert!(agent_forwarding_config.is_none());
        }
    }

    #[test]
    fn agent_authentication_defaults_missing_endpoint_to_auto() {
        let mut connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-1",
            "name": "Agent",
            "type": "ssh",
            "host": "example.com",
            "auth": { "mode": "agent" }
        }))
        .expect("connection");

        migrate_legacy_ssh_agent_settings(&mut connection);

        let ConnectionType::Ssh {
            auth_agent_endpoint,
            ..
        } = connection.config
        else {
            panic!("expected SSH connection");
        };
        assert_eq!(auth_agent_endpoint, Some(SshAgentEndpoint::Auto));
    }

    #[test]
    fn removed_inheritance_field_is_ignored_and_not_serialized() {
        let mut connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-1",
            "name": "Agent",
            "type": "ssh",
            "host": "example.com",
            "agent_forwarding_config": {
                "enabled": true,
                "inherit_auth_source": true,
                "sources": { "stored_keys": true }
            }
        }))
        .expect("connection");

        migrate_legacy_ssh_agent_settings(&mut connection);
        let value = serde_json::to_value(connection).expect("serialized connection");
        let forwarding = value
            .get("agent_forwarding_config")
            .expect("forwarding config");
        assert!(forwarding.get("inherit_auth_source").is_none());
        assert_eq!(forwarding.get("enabled"), Some(&serde_json::json!(true)));
    }

    #[test]
    fn legacy_agent_forwarding_migrates_once_without_exposing_stored_keys() {
        let mut connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-1",
            "name": "Agent",
            "type": "ssh",
            "host": "example.com",
            "agent_endpoint": {
                "type": "unix_socket",
                "path": "/tmp/legacy-agent.sock"
            },
            "agent_forwarding": true
        }))
        .expect("connection");

        assert!(migrate_legacy_ssh_agent_settings(&mut connection));

        let ConnectionType::Ssh {
            auth_agent_endpoint,
            legacy_agent_forwarding,
            agent_forwarding_config: Some(forwarding),
            ..
        } = &connection.config
        else {
            panic!("expected forwarding config");
        };
        assert!(auth_agent_endpoint.is_none());
        assert!(legacy_agent_forwarding.is_none());
        assert!(forwarding.enabled);
        assert!(forwarding.sources.external_agent);
        assert_eq!(
            forwarding.sources.external_agent_endpoints,
            vec![SshAgentEndpoint::UnixSocket {
                path: "/tmp/legacy-agent.sock".to_string()
            }]
        );
        assert!(!forwarding.sources.stored_keys);
        assert_eq!(forwarding.policy, SshAgentForwardingPolicy::All);
        assert!(!migrate_legacy_ssh_agent_settings(&mut connection));
    }

    #[test]
    fn current_forwarding_config_takes_priority_over_legacy_flag() {
        let mut connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-1",
            "name": "Agent",
            "type": "ssh",
            "host": "example.com",
            "agent_forwarding": true,
            "agent_forwarding_config": {
                "enabled": true,
                "sources": {
                    "external_agent": true,
                    "stored_keys": true
                },
                "policy": {
                    "mode": "allowlist",
                    "fingerprints": ["SHA256:test"]
                }
            }
        }))
        .expect("connection");

        migrate_legacy_ssh_agent_settings(&mut connection);

        let ConnectionType::Ssh {
            legacy_agent_forwarding,
            agent_forwarding_config: Some(forwarding),
            ..
        } = &connection.config
        else {
            panic!("expected forwarding config");
        };
        assert!(legacy_agent_forwarding.is_none());
        assert!(forwarding.sources.stored_keys);
        assert_eq!(
            forwarding.policy,
            SshAgentForwardingPolicy::Allowlist {
                fingerprints: vec!["SHA256:test".to_string()]
            }
        );
    }

    #[test]
    fn migration_serializes_only_current_agent_fields() {
        let mut connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-1",
            "name": "Agent",
            "type": "ssh",
            "host": "example.com",
            "agent_endpoint": { "type": "auto" },
            "agent_forwarding": true,
            "auth": { "mode": "agent" }
        }))
        .expect("connection");

        migrate_legacy_ssh_agent_settings(&mut connection);
        let value = serde_json::to_value(connection).expect("serialized connection");

        assert!(value.get("auth_agent_endpoint").is_some());
        assert!(value.get("agent_forwarding_config").is_some());
        assert!(value.get("agent_endpoint").is_none());
        assert!(value.get("agent_forwarding").is_none());
    }

    #[test]
    fn forwarding_endpoint_limit_is_checked_before_duplicate_detection() {
        let endpoints = vec![SshAgentEndpoint::Auto; MAX_SSH_AGENT_FORWARDING_ENDPOINTS + 1];
        let config = ConnectionType::Ssh {
            host: "example.com".to_string(),
            port: 22,
            username: "root".to_string(),
            backspace_mode: "del".to_string(),
            x11_forwarding: false,
            auth_agent_endpoint: Some(SshAgentEndpoint::Auto),
            legacy_agent_forwarding: None,
            agent_forwarding_config: Some(SshAgentForwardingConfig {
                enabled: false,
                sources: SshAgentForwardingSources {
                    external_agent: true,
                    external_agent_endpoints: endpoints,
                    stored_keys: false,
                },
                policy: SshAgentForwardingPolicy::default(),
            }),
            encoding: String::new(),
            dynamic_tab_title: false,
        };

        let error = validate_ssh_agent_settings(&config).expect_err("endpoint limit");
        assert!(error.to_string().contains("at most 16"));
    }

    #[test]
    fn forwarding_endpoint_validation_rejects_empty_values_even_when_disabled() {
        let environment = SshAgentEndpoint::Environment {
            variable: " $ ".to_string(),
        };
        assert!(validate_ssh_agent_endpoint(&environment).is_err());

        let socket = SshAgentEndpoint::UnixSocket {
            path: "  ".to_string(),
        };
        assert!(validate_ssh_agent_endpoint(&socket).is_err());
        let nul_socket = SshAgentEndpoint::UnixSocket {
            path: "agent\0.sock".to_string(),
        };
        assert!(validate_ssh_agent_endpoint(&nul_socket).is_err());

        let long_environment = SshAgentEndpoint::Environment {
            variable: "A".repeat(MAX_SSH_AGENT_ENVIRONMENT_VARIABLE_LEN + 1),
        };
        assert!(validate_ssh_agent_endpoint(&long_environment).is_err());

        let long_socket = SshAgentEndpoint::UnixSocket {
            path: "a".repeat(MAX_SSH_AGENT_UNIX_SOCKET_PATH_LEN + 1),
        };
        assert!(validate_ssh_agent_endpoint(&long_socket).is_err());

        let config = ConnectionType::Ssh {
            host: "example.com".to_string(),
            port: 22,
            username: "root".to_string(),
            backspace_mode: "del".to_string(),
            x11_forwarding: false,
            auth_agent_endpoint: Some(SshAgentEndpoint::Auto),
            legacy_agent_forwarding: None,
            agent_forwarding_config: Some(SshAgentForwardingConfig {
                enabled: false,
                sources: SshAgentForwardingSources {
                    external_agent: false,
                    external_agent_endpoints: vec![environment],
                    stored_keys: false,
                },
                policy: SshAgentForwardingPolicy::default(),
            }),
            encoding: String::new(),
            dynamic_tab_title: false,
        };
        assert!(validate_ssh_agent_settings(&config).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn forwarding_endpoint_validation_rejects_invalid_environment_variable_names() {
        for variable in ["SSH=AUTH_SOCK", "SSH\0AUTH_SOCK"] {
            assert!(
                validate_ssh_agent_endpoint(&SshAgentEndpoint::Environment {
                    variable: variable.to_string(),
                })
                .is_err()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn forwarding_endpoint_validation_normalizes_environment_duplicates() {
        let config = ConnectionType::Ssh {
            host: "example.com".to_string(),
            port: 22,
            username: "root".to_string(),
            backspace_mode: "del".to_string(),
            x11_forwarding: false,
            auth_agent_endpoint: Some(SshAgentEndpoint::Auto),
            legacy_agent_forwarding: None,
            agent_forwarding_config: Some(SshAgentForwardingConfig {
                enabled: true,
                sources: SshAgentForwardingSources {
                    external_agent: true,
                    external_agent_endpoints: vec![
                        SshAgentEndpoint::Environment {
                            variable: "SSH_AUTH_SOCK".to_string(),
                        },
                        SshAgentEndpoint::Environment {
                            variable: "$SSH_AUTH_SOCK".to_string(),
                        },
                    ],
                    stored_keys: false,
                },
                policy: SshAgentForwardingPolicy::default(),
            }),
            encoding: String::new(),
            dynamic_tab_title: false,
        };

        let error = validate_ssh_agent_settings(&config).expect_err("duplicate endpoint");
        assert!(error.to_string().contains("must be unique"));
    }

    #[cfg(unix)]
    #[test]
    fn forwarding_endpoint_validation_treats_auto_as_default_environment() {
        let config = ConnectionType::Ssh {
            host: "example.com".to_string(),
            port: 22,
            username: "root".to_string(),
            backspace_mode: "del".to_string(),
            x11_forwarding: false,
            auth_agent_endpoint: Some(SshAgentEndpoint::Auto),
            legacy_agent_forwarding: None,
            agent_forwarding_config: Some(SshAgentForwardingConfig {
                enabled: true,
                sources: SshAgentForwardingSources {
                    external_agent: true,
                    external_agent_endpoints: vec![
                        SshAgentEndpoint::Auto,
                        SshAgentEndpoint::Environment {
                            variable: "SSH_AUTH_SOCK".to_string(),
                        },
                    ],
                    stored_keys: false,
                },
                policy: SshAgentForwardingPolicy::default(),
            }),
            encoding: String::new(),
            dynamic_tab_title: false,
        };

        let error = validate_ssh_agent_settings(&config).expect_err("duplicate endpoint");
        assert!(error.to_string().contains("must be unique"));
    }

    #[test]
    fn forwarding_allowlist_validation_rejects_duplicates_and_excessive_values() {
        let base = |fingerprints| ConnectionType::Ssh {
            host: "example.com".to_string(),
            port: 22,
            username: "root".to_string(),
            backspace_mode: "del".to_string(),
            x11_forwarding: false,
            auth_agent_endpoint: None,
            legacy_agent_forwarding: None,
            agent_forwarding_config: Some(SshAgentForwardingConfig {
                enabled: true,
                sources: SshAgentForwardingSources::default(),
                policy: SshAgentForwardingPolicy::Allowlist { fingerprints },
            }),
            encoding: String::new(),
            dynamic_tab_title: false,
        };

        let duplicate = base(vec!["SHA256:test".to_string(), "SHA256:test".to_string()]);
        assert!(validate_ssh_agent_settings(&duplicate).is_err());

        let excessive = base(vec![
            "SHA256:test".to_string();
            MAX_SSH_AGENT_FORWARDING_IDENTITIES + 1
        ]);
        assert!(validate_ssh_agent_settings(&excessive).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn forwarding_endpoint_validation_rejects_windows_only_endpoints() {
        assert!(validate_ssh_agent_endpoint(&SshAgentEndpoint::Pageant).is_err());
        assert!(validate_ssh_agent_endpoint(&SshAgentEndpoint::WindowsOpenSsh).is_err());

        let config: ConnectionType = serde_json::from_value(serde_json::json!({
            "type": "ssh",
            "host": "example.com",
            "agent_forwarding_config": {
                "sources": {
                    "external_agent": true,
                    "external_agent_endpoints": [{ "type": "pageant" }]
                }
            }
        }))
        .expect("ssh config");
        assert!(validate_ssh_agent_settings(&config).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn forwarding_endpoint_validation_rejects_unix_only_endpoints() {
        assert!(
            validate_ssh_agent_endpoint(&SshAgentEndpoint::Environment {
                variable: "SSH_AUTH_SOCK".to_string(),
            })
            .is_err()
        );
        assert!(
            validate_ssh_agent_endpoint(&SshAgentEndpoint::UnixSocket {
                path: "agent.sock".to_string(),
            })
            .is_err()
        );
    }

    #[test]
    fn saved_connection_defaults_missing_ssh_profile_to_standard() {
        let connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-1",
            "name": "Test",
            "type": "ssh",
            "host": "example.com",
            "port": 22,
            "username": "root"
        }))
        .expect("connection");

        assert_eq!(connection.ssh_profile, SshProfile::Standard);
        assert!(connection.terminal_type.is_none());
        assert_eq!(
            resolve_ssh_terminal_type(&connection.ssh_profile, connection.terminal_type.as_ref()),
            SshTerminalType::Xterm256Color
        );
    }

    #[test]
    fn network_device_defaults_terminal_type_to_vt100() {
        let connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-1",
            "name": "Switch",
            "type": "ssh",
            "host": "example.com",
            "port": 22,
            "username": "admin",
            "ssh_profile": "network_device"
        }))
        .expect("connection");

        assert_eq!(connection.ssh_profile, SshProfile::NetworkDevice);
        assert_eq!(
            resolve_ssh_terminal_type(&connection.ssh_profile, connection.terminal_type.as_ref()),
            SshTerminalType::Vt100
        );
    }

    #[test]
    fn custom_terminal_type_is_preserved() {
        let connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-1",
            "name": "Switch",
            "type": "ssh",
            "host": "example.com",
            "port": 22,
            "username": "admin",
            "ssh_profile": "network_device",
            "terminal_type": "vt220"
        }))
        .expect("connection");

        assert_eq!(connection.terminal_type, Some(SshTerminalType::Vt220));
        assert_eq!(
            resolve_ssh_terminal_type(&connection.ssh_profile, connection.terminal_type.as_ref()),
            SshTerminalType::Vt220
        );
    }

    #[test]
    fn saved_connection_defaults_missing_sftp_settings() {
        let connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-1",
            "name": "Test",
            "type": "ssh",
            "host": "example.com",
            "port": 22,
            "username": "root"
        }))
        .expect("connection");

        assert!(connection.sftp.enabled);
        assert_eq!(
            connection.sftp.cwd_follow_mode,
            SftpCwdFollowMode::ShellIntegration
        );
        assert_eq!(connection.sftp.shell_detection_timeout_ms, 3000);
        assert_eq!(connection.sftp.pipeline_depth, None);
    }

    #[test]
    fn saved_connection_preserves_sftp_shell_detection_timeout() {
        let connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-1",
            "name": "Test",
            "type": "ssh",
            "host": "example.com",
            "port": 22,
            "username": "root",
            "sftp": {
                "enabled": true,
                "cwd_follow_mode": "shell_integration",
                "shell_detection_timeout_ms": 5000
            }
        }))
        .expect("connection");

        assert_eq!(connection.sftp.shell_detection_timeout_ms, 5000);
    }

    #[test]
    fn sftp_pipeline_depth_roundtrips_and_automatic_is_omitted() {
        let automatic = serde_json::to_value(SftpSettings::default()).expect("automatic settings");
        assert!(automatic.get("pipeline_depth").is_none());

        let settings = SftpSettings {
            pipeline_depth: Some(32),
            ..SftpSettings::default()
        };
        let encoded = serde_json::to_value(&settings).expect("manual settings");
        assert_eq!(encoded["pipeline_depth"], 32);
        let decoded: SftpSettings = serde_json::from_value(encoded).expect("roundtrip settings");
        assert_eq!(decoded.pipeline_depth, Some(32));
    }

    #[test]
    fn sftp_pipeline_depth_normalizes_invalid_imported_values() {
        let below_minimum: SftpSettings =
            serde_json::from_value(serde_json::json!({ "pipeline_depth": 0 }))
                .expect("below-minimum settings");
        let above_maximum: SftpSettings =
            serde_json::from_value(serde_json::json!({ "pipeline_depth": 999 }))
                .expect("above-maximum settings");
        let negative: SftpSettings =
            serde_json::from_value(serde_json::json!({ "pipeline_depth": -10 }))
                .expect("negative settings");
        let invalid_type: SftpSettings =
            serde_json::from_value(serde_json::json!({ "pipeline_depth": "fast" }))
                .expect("invalid-type settings");

        assert_eq!(below_minimum.pipeline_depth, Some(MIN_SFTP_PIPELINE_DEPTH));
        assert_eq!(above_maximum.pipeline_depth, Some(MAX_SFTP_PIPELINE_DEPTH));
        assert_eq!(negative.pipeline_depth, Some(MIN_SFTP_PIPELINE_DEPTH));
        assert_eq!(invalid_type.pipeline_depth, None);
    }

    #[test]
    fn effective_cwd_follow_is_off_when_sftp_disabled_without_mutating_setting() {
        let settings = SftpSettings {
            enabled: false,
            cwd_follow_mode: SftpCwdFollowMode::ShellIntegration,
            ..SftpSettings::default()
        };

        assert_eq!(effective_cwd_follow_mode(&settings), SftpCwdFollowMode::Off);
        assert_eq!(
            settings.cwd_follow_mode,
            SftpCwdFollowMode::ShellIntegration
        );
    }

    #[test]
    fn effective_cwd_follow_keeps_off_when_sftp_enabled_and_mode_off() {
        let settings = SftpSettings {
            enabled: true,
            cwd_follow_mode: SftpCwdFollowMode::Off,
            ..SftpSettings::default()
        };

        assert_eq!(effective_cwd_follow_mode(&settings), SftpCwdFollowMode::Off);
    }

    #[test]
    fn effective_cwd_follow_keeps_shell_integration_when_sftp_enabled() {
        let settings = SftpSettings {
            enabled: true,
            cwd_follow_mode: SftpCwdFollowMode::ShellIntegration,
            ..SftpSettings::default()
        };

        assert_eq!(
            effective_cwd_follow_mode(&settings),
            SftpCwdFollowMode::ShellIntegration
        );
    }

    #[test]
    fn network_device_effective_cwd_follow_is_off_without_mutating_setting() {
        let settings = SftpSettings {
            enabled: true,
            cwd_follow_mode: SftpCwdFollowMode::ShellIntegration,
            ..SftpSettings::default()
        };

        assert_eq!(
            effective_cwd_follow_mode_for_profile(&settings, &SshProfile::NetworkDevice),
            SftpCwdFollowMode::Off
        );
        assert_eq!(
            settings.cwd_follow_mode,
            SftpCwdFollowMode::ShellIntegration
        );
    }

    #[test]
    fn saved_connection_defaults_missing_asset_to_none() {
        let connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-1",
            "name": "Test",
            "type": "ssh",
            "host": "example.com",
            "port": 22,
            "username": "root"
        }))
        .expect("connection");

        assert!(connection.asset.is_none());
    }

    #[test]
    fn saved_connection_roundtrips_complete_asset_metadata() {
        let raw = serde_json::json!({
            "id": "conn-1",
            "name": "Asset Host",
            "type": "ssh",
            "host": "10.0.0.2",
            "port": 22,
            "username": "root",
            "asset": {
                "device_type": "physical",
                "os_name": "Ubuntu",
                "os_version": "24.04",
                "architecture": "x86_64",
                "kernel_version": "6.8.0",
                "hostname": "gpu-node-01",
                "cpu_model": "AMD EPYC 9654",
                "cpu_sockets": 2,
                "cpu_cores": 192,
                "cpu_threads": 384,
                "memory_bytes": 1099511627776u64,
                "accelerators": [
                    {
                        "type": "gpu",
                        "vendor": "NVIDIA",
                        "model": "H100",
                        "count": 8,
                        "memory_bytes": 85899345920u64
                    }
                ],
                "disks": [
                    {
                        "kind": "nvme",
                        "model": "PM9A3",
                        "capacity_bytes": 7680000000000u64,
                        "count": 4,
                        "purpose": "data"
                    }
                ],
                "tags": ["training", "production"],
                "notes": "Static asset metadata",
                "updated_at": "2026-08-03T12:00:00.000Z"
            }
        });

        let connection: SavedConnection = serde_json::from_value(raw).expect("connection");
        let encoded = serde_json::to_value(&connection).expect("asset json");
        let asset = connection.asset.expect("asset");

        assert_eq!(asset.device_type, Some(AssetDeviceType::Physical));
        assert_eq!(asset.os_name.as_deref(), Some("Ubuntu"));
        assert_eq!(asset.cpu_threads, Some(384));
        assert_eq!(
            asset.updated_at.as_deref(),
            Some("2026-08-03T12:00:00.000Z")
        );
        let accelerator = asset
            .accelerators
            .as_ref()
            .and_then(|items| items.first())
            .expect("accelerator");
        assert_eq!(accelerator.r#type, AssetAcceleratorType::Gpu);
        assert_eq!(accelerator.count, Some(8));
        let disk = asset
            .disks
            .as_ref()
            .and_then(|items| items.first())
            .expect("disk");
        assert_eq!(disk.kind, Some(AssetDiskKind::Nvme));
        assert_eq!(disk.purpose, Some(AssetDiskPurpose::Data));
        assert_eq!(encoded["asset"]["accelerators"][0]["type"], "gpu");
        assert_eq!(encoded["asset"]["disks"][0]["kind"], "nvme");
    }

    #[test]
    fn asset_accelerators_distinguish_missing_null_and_empty() {
        let missing: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-missing",
            "name": "Missing",
            "type": "ssh",
            "host": "example.com",
            "port": 22,
            "username": "root",
            "asset": {}
        }))
        .expect("missing");
        let null_value: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-null",
            "name": "Null",
            "type": "ssh",
            "host": "example.com",
            "port": 22,
            "username": "root",
            "asset": {
                "accelerators": null
            }
        }))
        .expect("null");
        let empty: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-empty",
            "name": "Empty",
            "type": "ssh",
            "host": "example.com",
            "port": 22,
            "username": "root",
            "asset": {
                "accelerators": []
            }
        }))
        .expect("empty");

        assert!(missing.asset.expect("asset").accelerators.is_none());
        assert!(null_value.asset.expect("asset").accelerators.is_none());
        assert_eq!(
            empty
                .asset
                .expect("asset")
                .accelerators
                .expect("accelerators"),
            Vec::new()
        );
    }

    #[test]
    fn ssh_connection_defaults_backspace_mode_to_del() {
        let connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-1",
            "name": "Test",
            "type": "ssh",
            "host": "example.com",
            "port": 22,
            "username": "root"
        }))
        .expect("connection");

        let ConnectionType::Ssh { backspace_mode, .. } = connection.config else {
            panic!("expected ssh connection");
        };
        assert_eq!(backspace_mode, "del");
    }

    #[test]
    fn ssh_connection_preserves_backspace_mode() {
        let connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-1",
            "name": "Test",
            "type": "ssh",
            "host": "example.com",
            "port": 22,
            "username": "root",
            "backspace_mode": "ctrl_h"
        }))
        .expect("connection");

        let ConnectionType::Ssh { backspace_mode, .. } = connection.config else {
            panic!("expected ssh connection");
        };
        assert_eq!(backspace_mode, "ctrl_h");
    }

    #[test]
    fn ssh_connection_defaults_x11_forwarding_to_false() {
        let connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-1",
            "name": "Test",
            "type": "ssh",
            "host": "example.com",
            "port": 22,
            "username": "root"
        }))
        .expect("connection");

        let ConnectionType::Ssh { x11_forwarding, .. } = connection.config else {
            panic!("expected ssh connection");
        };
        assert!(!x11_forwarding);
    }

    #[test]
    fn ssh_dynamic_title_defaults_off_and_round_trips_when_enabled() {
        let legacy: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-legacy",
            "name": "Legacy",
            "type": "ssh",
            "host": "example.com",
            "port": 22,
            "username": "root"
        }))
        .expect("legacy connection");
        assert!(matches!(
            legacy.config,
            ConnectionType::Ssh {
                dynamic_tab_title: false,
                ..
            }
        ));

        let enabled: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-enabled",
            "name": "Enabled",
            "type": "ssh",
            "host": "example.com",
            "port": 22,
            "username": "root",
            "dynamic_tab_title": true
        }))
        .expect("enabled connection");
        assert!(matches!(
            enabled.config,
            ConnectionType::Ssh {
                dynamic_tab_title: true,
                ..
            }
        ));
        let serialized = serde_json::to_value(enabled).expect("serialize connection");
        assert_eq!(
            serialized.get("dynamic_tab_title"),
            Some(&serde_json::json!(true))
        );
    }

    #[test]
    fn local_dynamic_title_defaults_off_and_round_trips_when_enabled() {
        let legacy: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "local-legacy",
            "name": "Legacy Local",
            "type": "local_terminal",
            "shell_path": "powershell.exe"
        }))
        .expect("legacy local connection");
        assert!(matches!(
            legacy.config,
            ConnectionType::LocalTerminal {
                dynamic_tab_title: false,
                ..
            }
        ));

        let enabled: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "local-enabled",
            "name": "Enabled Local",
            "type": "local_terminal",
            "shell_path": "powershell.exe",
            "dynamic_tab_title": true
        }))
        .expect("enabled local connection");
        assert!(matches!(
            enabled.config,
            ConnectionType::LocalTerminal {
                dynamic_tab_title: true,
                ..
            }
        ));
        assert_eq!(
            serde_json::to_value(enabled)
                .expect("serialize local connection")
                .get("dynamic_tab_title"),
            Some(&serde_json::json!(true))
        );
    }

    #[test]
    fn telnet_connection_defaults_compatibility_options() {
        let connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-1",
            "name": "Test",
            "type": "telnet",
            "host": "example.com",
            "port": 23
        }))
        .expect("connection");

        let ConnectionType::Telnet {
            username,
            raw_tcp_cli,
            enter_mode,
            local_echo,
            local_line_edit,
            force_character_at_a_time,
            send_naws,
            send_sga,
            ..
        } = connection.config
        else {
            panic!("expected telnet connection");
        };

        assert!(username.is_empty());
        assert!(!raw_tcp_cli);
        assert_eq!(enter_mode, "cr");
        assert!(!local_echo);
        assert!(!local_line_edit);
        assert!(!force_character_at_a_time);
        assert!(send_naws);
        assert!(send_sga);
    }

    #[test]
    fn telnet_connection_preserves_compatibility_options() {
        let connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-1",
            "name": "Test",
            "type": "telnet",
            "host": "example.com",
            "port": 8080,
            "raw_tcp_cli": true,
            "enter_mode": "lf",
            "local_echo": true,
            "local_line_edit": true,
            "force_character_at_a_time": true,
            "send_naws": false,
            "send_sga": false
        }))
        .expect("connection");

        let ConnectionType::Telnet {
            raw_tcp_cli,
            enter_mode,
            local_echo,
            local_line_edit,
            force_character_at_a_time,
            send_naws,
            send_sga,
            ..
        } = connection.config
        else {
            panic!("expected telnet connection");
        };

        assert!(raw_tcp_cli);
        assert_eq!(enter_mode, "lf");
        assert!(local_echo);
        assert!(local_line_edit);
        assert!(force_character_at_a_time);
        assert!(!send_naws);
        assert!(!send_sga);
    }

    #[test]
    fn rdp_connection_defaults_mvp_options() {
        let connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "rdp-1",
            "name": "Windows",
            "type": "rdp",
            "host": "192.168.1.20",
            "username": "Administrator"
        }))
        .expect("connection");

        let ConnectionType::Rdp {
            port,
            security,
            display,
            clipboard,
            reconnect,
            ..
        } = connection.config
        else {
            panic!("expected rdp connection");
        };

        assert_eq!(port, 3389);
        assert!(security.use_nla);
        assert_eq!(security.certificate_policy, "prompt");
        assert_eq!(display.width, 1920);
        assert_eq!(display.height, 1080);
        assert_eq!(display.color_depth, 32);
        assert_eq!(clipboard.mode, "text-only");
        assert!(reconnect.enabled);
        assert_eq!(reconnect.max_attempts, 5);
    }

    #[test]
    fn vnc_connection_defaults_to_standard_mvp_options() {
        let connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "vnc-1",
            "name": "Linux desktop",
            "type": "vnc",
            "host": "192.168.1.30"
        }))
        .expect("connection");

        let ConnectionType::Vnc {
            port,
            security,
            display,
            clipboard,
            reconnect,
            shared,
            view_only,
            ..
        } = connection.config
        else {
            panic!("expected vnc connection");
        };

        assert_eq!(port, 5900);
        assert_eq!(security.mode, "auto");
        assert_eq!(display.scale_mode, "fit");
        assert!(clipboard.enabled);
        assert!(reconnect.enabled);
        assert_eq!(reconnect.max_attempts, 5);
        assert!(shared);
        assert!(!view_only);
    }

    #[test]
    fn rdp_and_vnc_old_configs_default_to_no_network() {
        let rdp: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "rdp-1",
            "name": "Windows",
            "type": "rdp",
            "host": "192.168.1.20",
            "username": "Administrator"
        }))
        .expect("rdp connection");
        let vnc: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "vnc-1",
            "name": "Linux desktop",
            "type": "vnc",
            "host": "192.168.1.30"
        }))
        .expect("vnc connection");

        assert!(rdp.network.is_none());
        assert!(vnc.network.is_none());
    }

    #[test]
    fn post_login_defaults_delay_when_omitted() {
        let connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "conn-1",
            "name": "Test",
            "type": "ssh",
            "host": "example.com",
            "port": 22,
            "username": "root",
            "post_login": {
                "enabled": true,
                "command": "uptime"
            }
        }))
        .expect("connection");

        let post_login = connection.post_login.expect("post_login");
        assert!(post_login.enabled);
        assert_eq!(post_login.command, "uptime");
        assert_eq!(post_login.delay_ms, 1000);
    }
}
