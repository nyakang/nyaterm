//! Config persistence for sessions, UI, and quick commands.
//!
//! Stores JSON files in `~/.dragonfly/`. Credentials are AES-256-GCM encrypted in-place.

mod cloud_sync;
mod connection;
mod key;
mod otp;
mod password;
mod proxy;
mod quick_command;
mod settings;
mod tunnel;
mod ui;

#[allow(unused_imports)]
pub use cloud_sync::{
    decrypt_cloud_sync_settings, encrypt_cloud_sync_settings, load_cloud_sync_settings,
    load_cloud_sync_state, mask_cloud_sync_settings, merge_masked_cloud_sync_settings,
    save_cloud_sync_state, CloudConflictPreview, CloudSyncHistoryEntry, CloudSyncSettings,
    CloudSyncState, CloudSyncStatus, RemoteBackupEntry, RemoteBackupIndex, S3SyncSettings,
    WebdavSyncSettings, CLOUD_SYNC_HISTORY_VERSION, MASKED_SECRET_VALUE,
};
#[allow(unused_imports)]
pub use connection::{
    load_config, load_connection_by_id, load_sessions, save_config, save_sessions, AppConfig,
    ConnectionAuth, ConnectionNetwork, ConnectionType, Group, SavedConnection, SessionsConfig,
};
#[allow(unused_imports)]
pub use key::{decrypt_key_pem, load_key_by_id, load_keys, save_keys, KeysConfig, SshKey};
#[allow(unused_imports)]
pub use otp::{load_otp_entries, load_otp_entry_by_id, save_otp_entries, OtpConfig, OtpEntry};
#[allow(unused_imports)]
pub use password::{
    load_password_by_id, load_passwords, save_passwords, PasswordsConfig, SavedPassword,
};
#[allow(unused_imports)]
pub use proxy::{load_proxies, load_proxy_by_id, save_proxies, ProxyConfig};
#[allow(unused_imports)]
pub use quick_command::{
    load_quick_commands, save_quick_commands, QuickCommand, QuickCommandCategory,
    QuickCommandsConfig,
};
#[allow(unused_imports)]
pub use settings::{
    decrypt_ai_settings, encrypt_ai_settings, load_app_settings, mask_ai_settings,
    merge_masked_ai_settings, save_app_settings, ActionLinksMatcherSettings, AiProviderKind,
    AiProviderProfile, AiSettings, AppSettings, AppearanceSettings, DiagnosticsLogLevel,
    DiagnosticsSettings, GeneralSettings, InteractionSettings, KeywordHighlightRule, ProxySettings,
    SearchEngine, SearchSettings, SecuritySettings, TerminalSettings, TransferSettings,
    TranslationSettings,
};
#[allow(unused_imports)]
pub use tunnel::{load_tunnels, save_tunnels, TunnelConfig, TunnelsConfig};
#[allow(unused_imports)]
pub use ui::{ActivityBarLayout, RestorablePaneNode, RestorableTab, UiConfig};

use crate::error::{AppError, AppResult};
use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

pub(crate) fn get_config_dir(app: &AppHandle) -> AppResult<PathBuf> {
    let home_dir = app
        .path()
        .home_dir()
        .map_err(|e| AppError::Config(e.to_string()))?;
    let config_dir = home_dir.join(".dragonfly");
    fs::create_dir_all(&config_dir)?;
    Ok(config_dir)
}

pub(crate) fn load_json<T: serde::de::DeserializeOwned + Default>(path: &PathBuf) -> AppResult<T> {
    if !path.exists() {
        return Ok(T::default());
    }
    let content = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content)?)
}

pub(crate) fn save_json<T: Serialize>(path: &PathBuf, data: &T) -> AppResult<()> {
    let content = serde_json::to_string_pretty(data)?;
    fs::write(path, content)?;
    Ok(())
}

pub(crate) fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub(crate) fn default_true() -> bool {
    true
}

pub(crate) fn default_false() -> bool {
    false
}
