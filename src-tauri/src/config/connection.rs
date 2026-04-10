use super::{
    get_config_dir, load_app_settings, load_json, load_proxies, save_app_settings, save_json,
    save_proxies, uuid_v4, ProxyConfig, ProxySettings,
};
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConnectionNetworkSettings {
    #[serde(default)]
    pub proxy: Option<ProxySettings>,
}

/// Saved SSH connection. Password-based auth references a managed password via `password_id`.
/// Key-based auth references a managed key via `key_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedConnection {
    #[serde(default = "uuid_v4")]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_type: String,

    /// Legacy field kept for deserialization during migration only.
    #[serde(default, skip_serializing)]
    pub password: Option<String>,

    /// References a managed password in passwords.json.
    #[serde(default)]
    pub password_id: Option<String>,

    /// References a managed key in keys.json.
    #[serde(default)]
    pub key_id: Option<String>,

    #[serde(default)]
    pub sort_order: i32,

    /// Icon key referencing a named icon (e.g. "docker", "ubuntu"). Displayed in the connections list.
    #[serde(default)]
    pub icon: Option<String>,

    /// References a standalone proxy config by id.
    #[serde(default)]
    pub proxy_id: Option<String>,

    /// References an OTP entry for two-factor authentication.
    #[serde(default)]
    pub otp_id: Option<String>,

    /// When true, auto-fill the OTP code during keyboard-interactive auth.
    #[serde(default)]
    pub auto_fill_otp: bool,

    #[serde(default)]
    pub network: ConnectionNetworkSettings,
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
}

/// Root config for groups and saved connections (sessions.json).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionsConfig {
    #[serde(default)]
    pub groups: Vec<Group>,
    pub connections: Vec<SavedConnection>,
}

/// Alias for the main app config (sessions + groups).
pub type AppConfig = SessionsConfig;

pub fn load_sessions(app: &AppHandle) -> AppResult<SessionsConfig> {
    let dir = get_config_dir(app)?;
    let path = dir.join("sessions.json");
    let cfg: SessionsConfig = load_json(&path)?;
    Ok(cfg)
}

/// Saves sessions config to disk.
pub fn save_sessions(app: &AppHandle, config: &SessionsConfig) -> AppResult<()> {
    let dir = get_config_dir(app)?;
    save_json(&dir.join("sessions.json"), config)
}

/// Loads the main app config (sessions + groups).
/// Also runs one-time migration from inline `password` to `password_id`.
pub fn load_config(app: &AppHandle) -> AppResult<AppConfig> {
    let mut cfg = load_sessions(app)?;

    let needs_migration = cfg
        .connections
        .iter()
        .any(|c| c.password.is_some() && c.password_id.is_none());

    if needs_migration {
        migrate_passwords_to_store(app, &mut cfg)?;
    }

    migrate_global_proxy_to_connections(app, &mut cfg)?;
    migrate_connection_proxies_to_standalone(app, &mut cfg)?;

    Ok(cfg)
}

/// Migrates connections that still have an inline `password` field to use the password store.
/// Creates a password entry for each, sets `password_id`, and clears the legacy field.
fn migrate_passwords_to_store(app: &AppHandle, cfg: &mut SessionsConfig) -> AppResult<()> {
    use super::password::{load_passwords, save_passwords, SavedPassword};

    let mut pw_cfg = load_passwords(app)?;

    for conn in &mut cfg.connections {
        if let Some(encrypted_pw) = conn.password.take() {
            if conn.password_id.is_some() {
                continue;
            }
            let pw_id = uuid::Uuid::new_v4().to_string();
            let entry = SavedPassword {
                id: pw_id.clone(),
                name: conn.name.clone(),
                password: Some(encrypted_pw),
                has_password: false,
            };
            pw_cfg.passwords.push(entry);

            // Re-encrypt isn't needed — the password is already AES-GCM encrypted.
            // We just move the ciphertext blob into the password store.
            conn.password_id = Some(pw_id);
        }
    }

    save_passwords(app, &pw_cfg)?;
    save_sessions(app, cfg)?;

    tracing::info!("Migrated inline passwords to password store");
    Ok(())
}

fn migrate_global_proxy_to_connections(app: &AppHandle, cfg: &mut SessionsConfig) -> AppResult<()> {
    let mut settings = load_app_settings(app)?;
    if !settings.proxy.enabled || cfg.connections.is_empty() {
        return Ok(());
    }

    let legacy_proxy = settings.proxy.clone();
    let mut migrated = false;

    for conn in &mut cfg.connections {
        if conn.network.proxy.is_none() {
            conn.network.proxy = Some(legacy_proxy.clone());
            migrated = true;
        }
    }

    if migrated {
        save_sessions(app, cfg)?;
    }

    settings.proxy = ProxySettings::default();
    save_app_settings(app, &settings)?;

    tracing::info!("Migrated legacy global proxy settings to per-connection proxy configs");
    Ok(())
}

fn migrate_connection_proxies_to_standalone(
    app: &AppHandle,
    cfg: &mut SessionsConfig,
) -> AppResult<()> {
    let needs_migration = cfg.connections.iter().any(|c| c.network.proxy.is_some());
    if !needs_migration {
        return Ok(());
    }

    let mut proxies = load_proxies(app).unwrap_or_default();

    for conn in &mut cfg.connections {
        if let Some(proxy) = conn.network.proxy.take() {
            let proxy_id = uuid::Uuid::new_v4().to_string();
            proxies.push(ProxyConfig {
                id: proxy_id.clone(),
                name: format!("{}", conn.name),
                protocol: proxy.protocol,
                host: proxy.host,
                port: proxy.port,
                username: None,
                password: None,
            });
            conn.proxy_id = Some(proxy_id);
        }
    }

    save_proxies(app, &proxies)?;
    save_sessions(app, cfg)?;

    tracing::info!("Migrated per-connection proxy settings to standalone proxy configs");
    Ok(())
}

/// Loads a single connection by ID.
///
/// Returns `AppError::SessionNotFound` if no connection with that ID exists.
pub fn load_connection_by_id(app: &AppHandle, id: &str) -> AppResult<SavedConnection> {
    let cfg = load_config(app)?;
    let conn = cfg
        .connections
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| AppError::SessionNotFound(format!("Connection '{}' not found", id)))?;
    Ok(conn)
}

/// Saves the main app config.
pub fn save_config(app: &AppHandle, config: &AppConfig) -> AppResult<()> {
    save_sessions(app, config)
}
