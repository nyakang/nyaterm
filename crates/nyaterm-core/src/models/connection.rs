use serde::{Deserialize, Serialize};

use super::{
    SavedConnection, default_backspace_mode_serial, default_backspace_mode_ssh,
    default_backspace_mode_telnet, default_baud_rate, default_data_bits, default_parity,
    default_rdp_certificate_policy, default_rdp_clipboard_mode, default_rdp_color_depth,
    default_rdp_display_mode, default_rdp_height, default_rdp_port, default_rdp_reconnect_attempts,
    default_rdp_width, default_sftp_shell_detection_timeout_ms, default_ssh_port, default_ssh_user,
    default_stop_bits, default_telnet_auto_login_timeout_ms, default_telnet_enter_mode,
    default_telnet_port, default_true, default_vnc_port, default_vnc_reconnect_attempts,
    default_vnc_scale_mode, default_vnc_security_mode, is_default_sftp_shell_detection_timeout_ms,
    is_default_telnet_auto_login_config,
};

fn is_false(value: &bool) -> bool {
    !*value
}

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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SshProfile {
    #[default]
    Standard,
    NetworkDevice,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
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
    pub fn as_str(self) -> &'static str {
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

pub fn default_terminal_type_for_profile(profile: SshProfile) -> SshTerminalType {
    match profile {
        SshProfile::Standard => SshTerminalType::Xterm256Color,
        SshProfile::NetworkDevice => SshTerminalType::Vt100,
    }
}

pub fn resolve_ssh_terminal_type(
    profile: SshProfile,
    terminal_type: Option<SshTerminalType>,
) -> SshTerminalType {
    terminal_type.unwrap_or_else(|| default_terminal_type_for_profile(profile))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SftpCwdFollowMode {
    Off,
    #[default]
    ShellIntegration,
    RcFile,
}

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

/// Structural validation errors for persisted SSH Agent settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshAgentEndpointValidationError {
    /// The environment variable name or Unix socket path is empty.
    Empty,
    /// The value contains a forbidden separator or NUL byte.
    Invalid,
    /// The value exceeds its compatibility limit.
    TooLong,
    /// The forwarding endpoint count exceeds the limit.
    TooManyEndpoints,
    /// A forwarding endpoint is duplicated after normalization.
    DuplicateEndpoint,
    /// The allowlist identity count exceeds the limit.
    TooManyIdentities,
    /// A fingerprint is empty or too long.
    InvalidFingerprint,
    /// An allowlist fingerprint is duplicated.
    DuplicateFingerprint,
}

/// Unix socket path limit retained from the Tauri persistence contract.
///
/// Runtime platform support is checked by the transport layer. Persistence
/// must retain foreign-platform settings instead of silently discarding them.
const SSH_AGENT_UNIX_SOCKET_PATH_MAX_LENGTH: usize = 4096;

/// Validates the persistable shape without probing endpoint availability.
///
pub fn validate_ssh_agent_endpoint(
    endpoint: &SshAgentEndpoint,
) -> Result<(), SshAgentEndpointValidationError> {
    match endpoint {
        SshAgentEndpoint::Environment { variable } => {
            let variable = variable.trim().trim_start_matches('$').trim();
            if variable.is_empty() {
                return Err(SshAgentEndpointValidationError::Empty);
            }
            if variable.contains('=') || variable.contains('\0') {
                return Err(SshAgentEndpointValidationError::Invalid);
            }
            if variable.len() > 255 {
                return Err(SshAgentEndpointValidationError::TooLong);
            }
        }
        SshAgentEndpoint::UnixSocket { path } => {
            if path.trim().is_empty() {
                return Err(SshAgentEndpointValidationError::Empty);
            }
            if path.contains('\0') {
                return Err(SshAgentEndpointValidationError::Invalid);
            }
            if path.len() > SSH_AGENT_UNIX_SOCKET_PATH_MAX_LENGTH {
                return Err(SshAgentEndpointValidationError::TooLong);
            }
        }
        SshAgentEndpoint::Auto | SshAgentEndpoint::Pageant | SshAgentEndpoint::WindowsOpenSsh => {}
    }
    Ok(())
}

/// Returns whether an endpoint type can run on the current desktop platform.
pub fn ssh_agent_endpoint_supported_on_current_platform(endpoint: &SshAgentEndpoint) -> bool {
    match endpoint {
        SshAgentEndpoint::Auto => cfg!(any(unix, windows)),
        SshAgentEndpoint::Environment { .. } | SshAgentEndpoint::UnixSocket { .. } => cfg!(unix),
        SshAgentEndpoint::Pageant | SshAgentEndpoint::WindowsOpenSsh => cfg!(windows),
    }
}

/// Falls back to `Auto` for invalid or unsupported endpoint values.
pub fn normalize_ssh_agent_endpoint(endpoint: SshAgentEndpoint) -> SshAgentEndpoint {
    if ssh_agent_endpoint_supported_on_current_platform(&endpoint)
        && validate_ssh_agent_endpoint(&endpoint).is_ok()
    {
        endpoint
    } else {
        SshAgentEndpoint::Auto
    }
}

/// Maximum number of persisted external Agent endpoints.
pub const MAX_SSH_AGENT_FORWARDING_ENDPOINTS: usize = 16;
/// Shared identity response and allowlist limit.
pub const MAX_SSH_AGENT_FORWARDING_IDENTITIES: usize = 1024;

/// External and stored-key forwarding sources.
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

/// Identity exposure policy for Agent forwarding.
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

/// SSH Agent forwarding configuration, independent from login authentication.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SshAgentForwardingConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub sources: SshAgentForwardingSources,
    #[serde(default)]
    pub policy: SshAgentForwardingPolicy,
}

/// Returns a normalized comparison key without exposing secret values.
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

fn normalize_ssh_agent_environment_variable(value: &str) -> Option<String> {
    let variable = value.trim().trim_start_matches('$').trim();
    if variable.is_empty() || variable.contains('=') || variable.contains('\0') {
        return None;
    }
    Some(variable.to_string())
}

/// Validates cross-platform structure without requiring local endpoint support.
pub fn validate_ssh_agent_forwarding_shape(
    config: &SshAgentForwardingConfig,
) -> Result<(), SshAgentEndpointValidationError> {
    if config.sources.external_agent_endpoints.len() > MAX_SSH_AGENT_FORWARDING_ENDPOINTS {
        return Err(SshAgentEndpointValidationError::TooManyEndpoints);
    }

    let mut endpoint_keys = std::collections::HashSet::new();
    for endpoint in &config.sources.external_agent_endpoints {
        validate_ssh_agent_endpoint(endpoint)?;
        if !endpoint_keys.insert(ssh_agent_endpoint_key(endpoint)) {
            return Err(SshAgentEndpointValidationError::DuplicateEndpoint);
        }
    }

    if let SshAgentForwardingPolicy::Allowlist { fingerprints } = &config.policy {
        if fingerprints.len() > MAX_SSH_AGENT_FORWARDING_IDENTITIES {
            return Err(SshAgentEndpointValidationError::TooManyIdentities);
        }
        let mut seen = std::collections::HashSet::new();
        for fingerprint in fingerprints {
            if fingerprint.is_empty() || fingerprint.len() > 255 {
                return Err(SshAgentEndpointValidationError::InvalidFingerprint);
            }
            if !seen.insert(fingerprint) {
                return Err(SshAgentEndpointValidationError::DuplicateFingerprint);
            }
        }
    }
    Ok(())
}

/// Validates forwarding persistence while preserving foreign-platform values.
pub fn validate_ssh_agent_forwarding_config(
    config: &SshAgentForwardingConfig,
) -> Result<(), SshAgentEndpointValidationError> {
    validate_ssh_agent_forwarding_shape(config)
}

/// Migrates legacy GPUI/Tauri fields to the canonical Agent configuration.
///
/// Migration is idempotent and canonical configuration always takes precedence.
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
        agent_forwarding_config.replace(SshAgentForwardingConfig {
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
            auth_agent_endpoint.replace(SshAgentEndpoint::Auto);
            changed = true;
        }
    } else if auth_agent_endpoint.take().is_some() {
        changed = true;
    }
    changed
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
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for SftpSettings {
    fn default() -> Self {
        Self {
            pipeline_depth: None,
            extra: Default::default(),
            enabled: true,
            cwd_follow_mode: SftpCwdFollowMode::ShellIntegration,
            shell_detection_timeout_ms: default_sftp_shell_detection_timeout_ms(),
            filename_encoding: String::new(),
        }
    }
}

pub const MIN_SFTP_SHELL_DETECTION_TIMEOUT_MS: u64 = 100;
pub const MAX_SFTP_SHELL_DETECTION_TIMEOUT_MS: u64 = 60_000;

pub fn validate_sftp_settings(settings: &SftpSettings) -> Result<(), String> {
    if !(MIN_SFTP_SHELL_DETECTION_TIMEOUT_MS..=MAX_SFTP_SHELL_DETECTION_TIMEOUT_MS)
        .contains(&settings.shell_detection_timeout_ms)
    {
        return Err(format!(
            "SFTP shell detection timeout must be between {} and {} ms",
            MIN_SFTP_SHELL_DETECTION_TIMEOUT_MS, MAX_SFTP_SHELL_DETECTION_TIMEOUT_MS
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TelnetAutoLoginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub send_wake_enter: bool,
    #[serde(default = "default_telnet_auto_login_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username_prompt_regex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_prompt_regex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success_prompt_regex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
        #[serde(default)]
        ai_execution_profile: AiExecutionProfile,
        #[serde(default)]
        x11_forwarding: bool,
        #[serde(
            default,
            alias = "agent_endpoint",
            skip_serializing_if = "Option::is_none"
        )]
        auth_agent_endpoint: Option<SshAgentEndpoint>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_forwarding_config: Option<SshAgentForwardingConfig>,
        #[serde(rename = "agent_forwarding", default, skip_serializing)]
        legacy_agent_forwarding: Option<bool>,
        #[serde(default)]
        encoding: String,
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
        #[serde(default)]
        ai_execution_profile: AiExecutionProfile,
        #[serde(default)]
        encoding: String,
        #[serde(default, skip_serializing_if = "is_false")]
        dynamic_tab_title: bool,
    },
    Telnet {
        host: String,
        #[serde(default = "default_telnet_port")]
        port: u16,
        #[serde(default)]
        username: String,
        #[serde(default)]
        ai_execution_profile: AiExecutionProfile,
        #[serde(default = "default_backspace_mode_telnet")]
        backspace_mode: String,
        #[serde(default)]
        raw_tcp_cli: bool,
        #[serde(default = "default_telnet_enter_mode")]
        enter_mode: String,
        #[serde(default)]
        local_echo: bool,
        #[serde(default)]
        local_line_edit: bool,
        #[serde(default)]
        force_character_at_a_time: bool,
        #[serde(default = "default_true")]
        send_naws: bool,
        #[serde(default = "default_true")]
        send_sga: bool,
        #[serde(default, skip_serializing_if = "is_default_telnet_auto_login_config")]
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
        #[serde(default)]
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

// ── Static asset metadata ─────────────────────────────────────────────────
//
// These types mirror the Tauri persistence contract (`AssetMetadata` and
// friends). Field names, enum snake_case renamings, and the `Option`-based
// "field is present" semantics must stay compatible so that connections written
// by the Tauri edition round-trip unchanged: every field is optional and skips
// serialization when absent, and unknown values are preserved rather than
// discarded.

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

impl ConnectionType {
    pub fn dynamic_tab_title_enabled(&self) -> bool {
        match self {
            Self::Ssh {
                dynamic_tab_title, ..
            }
            | Self::LocalTerminal {
                dynamic_tab_title, ..
            } => *dynamic_tab_title,
            _ => false,
        }
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<AssetDiskKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<AssetDiskPurpose>,
}

/// Static, user- or monitoring-populated asset facts for a saved connection.
///
/// `accelerators` and `disks` distinguish "absent" (`None`) from "explicitly
/// empty" (`Some(vec![])`); the monitoring merge and JSON round-trip rely on
/// that distinction, so neither is collapsed to the other.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AssetMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_type: Option<AssetDeviceType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_sockets: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_cores: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_threads: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accelerators: Option<Vec<AssetAccelerator>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disks: Option<Vec<AssetDisk>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

impl AssetMetadata {
    /// Applies a monitoring-derived patch onto `self` in place.
    ///
    /// Only fields present in `patch` overwrite the target, so operator-entered
    /// facts that monitoring never reports (device type, tags, notes, socket and
    /// thread counts, OS/kernel version) survive a merge. Accelerators are
    /// merged per-type: any accelerator type included in the patch replaces the
    /// current entries of that type while leaving other types untouched. This
    /// mirrors the Tauri `merge_monitoring_asset_patch` contract exactly.
    pub fn merge_monitoring_patch(&mut self, patch: AssetMetadata) {
        if patch.hostname.is_some() {
            self.hostname = patch.hostname;
        }
        if patch.os_name.is_some() {
            self.os_name = patch.os_name;
        }
        if patch.architecture.is_some() {
            self.architecture = patch.architecture;
        }
        if patch.cpu_model.is_some() {
            self.cpu_model = patch.cpu_model;
        }
        if patch.cpu_cores.is_some() {
            self.cpu_cores = patch.cpu_cores;
        }
        if patch.memory_bytes.is_some() {
            self.memory_bytes = patch.memory_bytes;
        }
        if patch.disks.is_some() {
            self.disks = patch.disks;
        }
        if patch.updated_at.is_some() {
            self.updated_at = patch.updated_at;
        }
        if let Some(accelerators) = patch.accelerators {
            self.accelerators = Some(merge_monitoring_accelerators(
                self.accelerators.take(),
                accelerators,
            ));
        }
    }
}

/// Replaces every accelerator whose type appears in `patch` with the patch
/// entries, preserving accelerator types the patch does not mention.
fn merge_monitoring_accelerators(
    current: Option<Vec<AssetAccelerator>>,
    patch: Vec<AssetAccelerator>,
) -> Vec<AssetAccelerator> {
    if patch.is_empty() {
        return current.unwrap_or_default();
    }

    let patch_types: Vec<AssetAcceleratorType> = patch
        .iter()
        .map(|accelerator| accelerator.r#type.clone())
        .collect();
    let mut merged = current.unwrap_or_default();
    merged.retain(|accelerator| !patch_types.contains(&accelerator.r#type));
    merged.extend(patch);
    merged
}

/// Match the supported Tauri pipeline range while accepting legacy invalid values.
fn deserialize_sftp_pipeline_depth<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| {
        value
            .as_i64()
            .map(|number| number.clamp(4, 64) as u32)
            .or_else(|| value.as_u64().map(|number| number.clamp(4, 64) as u32))
    }))
}
