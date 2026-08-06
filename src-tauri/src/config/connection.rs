use super::{
    ProxyConfig, ProxySettings, load_app_settings, load_proxies, save_app_settings, save_proxies,
    uuid_v4,
};
use crate::error::{AppError, AppResult};
use crate::storage;
use serde::{Deserialize, Serialize};
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SftpCwdFollowMode {
    Off,
    #[default]
    ShellIntegration,
    RcFile,
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
}

impl Default for SftpSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            cwd_follow_mode: SftpCwdFollowMode::ShellIntegration,
            shell_detection_timeout_ms: default_sftp_shell_detection_timeout_ms(),
            filename_encoding: String::new(),
        }
    }
}

pub const MIN_SFTP_SHELL_DETECTION_TIMEOUT_MS: u64 = 100;
pub const MAX_SFTP_SHELL_DETECTION_TIMEOUT_MS: u64 = 60_000;

pub fn default_sftp_shell_detection_timeout_ms() -> u64 {
    3000
}

fn is_default_sftp_shell_detection_timeout_ms(value: &u64) -> bool {
    *value == default_sftp_shell_detection_timeout_ms()
}

fn is_default_sftp_settings(value: &SftpSettings) -> bool {
    value == &SftpSettings::default()
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
        #[serde(default)]
        encoding: String,
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
    pub ssh_algorithms: Option<SshAlgorithmPreferences>,
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

/// Root config for groups and saved connections.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionsConfig {
    #[serde(default)]
    pub groups: Vec<Group>,
    #[serde(default)]
    pub connections: Vec<SavedConnection>,
}

/// Alias for the main app config (sessions + groups).
pub type AppConfig = SessionsConfig;

// ── Loading / saving ────────────────────────────────────────────────────────

pub fn load_sessions(app: &AppHandle) -> AppResult<SessionsConfig> {
    let _ = app;
    storage::load_sessions()
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
            ConnectionType::Ssh { .. } => {}
        }
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
    let _ = app;
    let conn = storage::get_connection(id)?
        .ok_or_else(|| AppError::SessionNotFound(format!("Connection '{}' not found", id)))?;
    Ok(conn)
}

/// Saves the main app config.
pub fn save_config(app: &AppHandle, config: &AppConfig) -> AppResult<()> {
    save_sessions(app, config)
}

#[cfg(test)]
mod tests {
    use super::{
        AssetAcceleratorType, AssetDeviceType, AssetDiskKind, AssetDiskPurpose, ConnectionType,
        SavedConnection, SftpCwdFollowMode, SshAlgorithmMode,
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
