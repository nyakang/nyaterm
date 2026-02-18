//! Config persistence for sessions, UI, and quick commands.
//!
//! Stores JSON files in `~/.dragonfly/`. Credentials are AES-256-GCM encrypted in-place.

use crate::crypto;
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

// ── Shared Helpers ─────────────────────────────────────────────────────────

fn get_config_dir(app: &AppHandle) -> AppResult<PathBuf> {
    let home_dir = app
        .path()
        .home_dir()
        .map_err(|e| AppError::Config(e.to_string()))?;
    let config_dir = home_dir.join(".dragonfly");
    fs::create_dir_all(&config_dir)?;
    Ok(config_dir)
}

fn load_json<T: serde::de::DeserializeOwned + Default>(path: &PathBuf) -> AppResult<T> {
    if !path.exists() {
        return Ok(T::default());
    }
    let content = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content)?)
}

fn save_json<T: Serialize>(path: &PathBuf, data: &T) -> AppResult<()> {
    let content = serde_json::to_string_pretty(data)?;
    fs::write(path, content)?;
    Ok(())
}

// ── sessions.json ──────────────────────────────────────────────────────────

/// Saved SSH connection. Credential fields store AES-256-GCM ciphertext on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedConnection {
    #[serde(default = "uuid_v4")]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_type: String,

    /// Ciphertext on disk; plaintext in memory after `load_connection_by_id`.
    #[serde(default)]
    pub password: Option<String>,
    /// Ciphertext on disk (PEM content); decrypted on demand via `decrypt_key_data`.
    #[serde(default)]
    pub key: Option<String>,
    /// Ciphertext on disk; plaintext in memory after `load_connection_by_id`.
    #[serde(default)]
    pub passphrase: Option<String>,

    /// File path chosen via the file picker — backend reads & encrypts the content.
    #[serde(default, skip_serializing)]
    pub key_file_path: Option<String>,
    /// True when an encrypted private key is stored in `key`.
    #[serde(default, skip_serializing)]
    pub has_key_data: bool,
}

fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Group for organizing saved connections in the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    #[serde(default = "uuid_v4")]
    pub id: String,
    pub name: String,
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

/// Decrypts `password` and `passphrase` in-place (ciphertext → plaintext).
///
/// Called by `load_connection_by_id` before an SSH session is established.
pub fn decrypt_credentials(conn: &mut SavedConnection) {
    if let Some(ct) = conn.password.clone() {
        conn.password = crypto::decrypt(&ct).ok();
    }
    if let Some(ct) = conn.passphrase.clone() {
        conn.passphrase = crypto::decrypt(&ct).ok();
    }
}

/// Decrypts and returns the stored private key (PEM) for SSH authentication.
pub fn decrypt_key_data(conn: &SavedConnection) -> AppResult<Option<String>> {
    crypto::decrypt_optional(&conn.key)
}

/// Loads sessions.json. Credential fields contain raw ciphertext; only `has_key_data` is derived.
pub fn load_sessions(app: &AppHandle) -> AppResult<SessionsConfig> {
    let dir = get_config_dir(app)?;
    let path = dir.join("sessions.json");
    let mut config: SessionsConfig = load_json(&path)?;

    for conn in &mut config.connections {
        conn.has_key_data = conn.key.is_some();
    }

    Ok(config)
}

/// Saves sessions config to disk (encrypted credentials are inline).
pub fn save_sessions(app: &AppHandle, config: &SessionsConfig) -> AppResult<()> {
    let dir = get_config_dir(app)?;
    save_json(&dir.join("sessions.json"), config)
}

/// Loads the main app config (sessions + groups).
pub fn load_config(app: &AppHandle) -> AppResult<AppConfig> {
    load_sessions(app)
}

/// Loads a single connection by ID and decrypts `password` and `passphrase` for SSH auth.
///
/// Returns `AppError::SessionNotFound` if no connection with that ID exists.
pub fn load_connection_by_id(app: &AppHandle, id: &str) -> AppResult<SavedConnection> {
    let cfg = load_config(app)?;
    let mut conn = cfg
        .connections
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| AppError::SessionNotFound(format!("Connection '{}' not found", id)))?;
    decrypt_credentials(&mut conn);
    Ok(conn)
}

/// Saves the main app config.
pub fn save_config(app: &AppHandle, config: &AppConfig) -> AppResult<()> {
    save_sessions(app, config)
}

// ── ui.json ────────────────────────────────────────────────────────────────

/// Layout and theme preferences persisted in ui.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    pub left_width: f64,
    pub right_width: f64,
    pub saved_conn_height: f64,
    pub history_height: f64,
    pub quick_cmd_height: f64,
    pub show_file_explorer: bool,
    pub show_saved_connections: bool,
    pub show_active_sessions: bool,
    pub show_command_history: bool,
    pub show_quick_commands: bool,
    pub zoom_level: f64,
    #[serde(default = "default_theme")]
    pub theme: Option<String>,
}

fn default_theme() -> Option<String> {
    Some("github-dark".to_string())
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            left_width: 256.0,
            right_width: 288.0,
            saved_conn_height: 240.0,
            history_height: 200.0,
            quick_cmd_height: 36.0,
            show_file_explorer: true,
            show_saved_connections: true,
            show_active_sessions: true,
            show_command_history: true,
            show_quick_commands: true,
            zoom_level: 1.0,
            theme: Some("github-dark".to_string()),
        }
    }
}

/// Loads UI layout/theme config from ~/.dragonfly/ui.json.
pub fn load_ui_config(app: &AppHandle) -> AppResult<UiConfig> {
    let dir = get_config_dir(app)?;
    load_json(&dir.join("ui.json"))
}

/// Saves UI config to disk.
pub fn save_ui_config(app: &AppHandle, config: &UiConfig) -> AppResult<()> {
    let dir = get_config_dir(app)?;
    save_json(&dir.join("ui.json"), config)
}

// ── quick-command.json ─────────────────────────────────────────────────────

/// Single quick command (label + shell command).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickCommand {
    pub id: String,
    pub label: String,
    pub command: String,
}

/// List of quick commands persisted in quick-command.json.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuickCommandsConfig {
    pub commands: Vec<QuickCommand>,
}

/// Loads quick commands from ~/.dragonfly/quick-command.json.
pub fn load_quick_commands(app: &AppHandle) -> AppResult<QuickCommandsConfig> {
    let dir = get_config_dir(app)?;
    load_json(&dir.join("quick-command.json"))
}

/// Saves quick commands to disk.
pub fn save_quick_commands(app: &AppHandle, config: &QuickCommandsConfig) -> AppResult<()> {
    let dir = get_config_dir(app)?;
    save_json(&dir.join("quick-command.json"), config)
}
