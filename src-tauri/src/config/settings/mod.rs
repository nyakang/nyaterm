mod appearance;
mod ai;
mod diagnostics;
mod general;
mod interaction;
mod proxy;
mod search;
mod security;
mod terminal;
mod transfer;
mod translation;

pub use appearance::AppearanceSettings;
pub use ai::{
    decrypt_ai_settings, encrypt_ai_settings, mask_ai_settings, merge_masked_ai_settings,
    AiProviderKind, AiProviderProfile, AiSettings,
};
pub use diagnostics::{DiagnosticsLogLevel, DiagnosticsSettings};
pub use general::GeneralSettings;
pub use interaction::InteractionSettings;
pub use proxy::ProxySettings;
pub use search::{SearchEngine, SearchSettings};
pub use security::SecuritySettings;
pub use terminal::{ActionLinksMatcherSettings, KeywordHighlightRule, TerminalSettings};
pub use transfer::TransferSettings;
pub use translation::TranslationSettings;

use super::cloud_sync::{
    decrypt_cloud_sync_settings, encrypt_cloud_sync_settings, load_cloud_sync_settings,
    CloudSyncSettings,
};
use super::ui::UiConfig;
use super::{get_config_dir, load_json, save_json};
use crate::error::AppResult;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppSettings {
    #[serde(default)]
    pub general: GeneralSettings,
    #[serde(default)]
    pub appearance: AppearanceSettings,
    #[serde(default)]
    pub proxy: ProxySettings,
    #[serde(default)]
    pub search: SearchSettings,
    #[serde(default)]
    pub translation: TranslationSettings,
    #[serde(default)]
    pub security: SecuritySettings,
    #[serde(default)]
    pub terminal: TerminalSettings,
    #[serde(default)]
    pub interaction: InteractionSettings,
    #[serde(default)]
    pub transfer: TransferSettings,
    #[serde(default)]
    pub diagnostics: DiagnosticsSettings,
    #[serde(default)]
    pub ai: AiSettings,
    #[serde(default)]
    pub cloud_sync: CloudSyncSettings,
    #[serde(default)]
    pub ui: UiConfig,
}

pub fn load_app_settings(app: &AppHandle) -> AppResult<AppSettings> {
    let dir = get_config_dir(app)?;
    let settings_path = dir.join("settings.json");
    let mut settings: AppSettings = load_json(&settings_path)?;
    let has_embedded_cloud_sync = settings_path.exists()
        && std::fs::read_to_string(&settings_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|value| value.get("cloud_sync").cloned())
            .is_some();

    let mut migrated = false;

    if has_embedded_cloud_sync {
        settings.cloud_sync =
            decrypt_cloud_sync_settings(settings.cloud_sync.clone()).unwrap_or(settings.cloud_sync);
    } else if let Ok(legacy_cloud_sync) =
        load_cloud_sync_settings(app).and_then(decrypt_cloud_sync_settings)
    {
        settings.cloud_sync = legacy_cloud_sync;
        migrated = true;
    }

    if let Ok(ai_settings) = decrypt_ai_settings(settings.ai.clone()) {
        settings.ai = ai_settings;
    }

    for list in [
        &mut settings.ui.activity_bar_layout.left_top,
        &mut settings.ui.activity_bar_layout.left_bottom,
        &mut settings.ui.activity_bar_layout.right_top,
        &mut settings.ui.activity_bar_layout.right_bottom,
    ] {
        for item in list.iter_mut() {
            if item == "keyManagement" {
                *item = "securityAuth".to_string();
                migrated = true;
            }
        }
    }
    if let Some(ref mut panel) = settings.ui.active_left_panel {
        if panel == "keyManagement" {
            *panel = "securityAuth".to_string();
            migrated = true;
        }
    }

    for list in [
        &mut settings.ui.activity_bar_layout.left_top,
        &mut settings.ui.activity_bar_layout.left_bottom,
        &mut settings.ui.activity_bar_layout.right_top,
        &mut settings.ui.activity_bar_layout.right_bottom,
    ] {
        let before = list.len();
        list.retain(|id| id != "fileTransfer");
        if list.len() != before {
            migrated = true;
        }
    }
    if settings.ui.active_left_panel.as_deref() == Some("fileTransfer") {
        settings.ui.active_left_panel = Some("fileExplorer".to_string());
        migrated = true;
    }
    if settings.ui.active_right_panel.as_deref() == Some("fileTransfer") {
        settings.ui.active_right_panel = Some("savedConnections".to_string());
        migrated = true;
    }

    {
        let all_ids: Vec<&str> = settings
            .ui
            .activity_bar_layout
            .left_top
            .iter()
            .chain(&settings.ui.activity_bar_layout.left_bottom)
            .chain(&settings.ui.activity_bar_layout.right_top)
            .chain(&settings.ui.activity_bar_layout.right_bottom)
            .map(|s| s.as_str())
            .collect();
        if !all_ids.contains(&"network") {
            settings
                .ui
                .activity_bar_layout
                .left_top
                .push("network".to_string());
            migrated = true;
        }
    }

    {
        let all_ids: Vec<&str> = settings
            .ui
            .activity_bar_layout
            .left_top
            .iter()
            .chain(&settings.ui.activity_bar_layout.left_bottom)
            .chain(&settings.ui.activity_bar_layout.right_top)
            .chain(&settings.ui.activity_bar_layout.right_bottom)
            .map(|s| s.as_str())
            .collect();
        if !all_ids.contains(&"syncBackupHistory") {
            let left_bottom = &mut settings.ui.activity_bar_layout.left_bottom;
            if let Some(settings_index) = left_bottom.iter().position(|id| id == "settings") {
                left_bottom.insert(settings_index, "syncBackupHistory".to_string());
            } else {
                left_bottom.push("syncBackupHistory".to_string());
            }
            migrated = true;
        }
    }

    {
        let all_ids: Vec<&str> = settings
            .ui
            .activity_bar_layout
            .left_top
            .iter()
            .chain(&settings.ui.activity_bar_layout.left_bottom)
            .chain(&settings.ui.activity_bar_layout.right_top)
            .chain(&settings.ui.activity_bar_layout.right_bottom)
            .map(|s| s.as_str())
            .collect();
        if !all_ids.contains(&"serialSend") {
            let right_bottom = &mut settings.ui.activity_bar_layout.right_bottom;
            if let Some(quick_cmd_index) = right_bottom.iter().position(|id| id == "quickCmdBar") {
                right_bottom.insert(quick_cmd_index + 1, "serialSend".to_string());
            } else if let Some(recording_index) =
                right_bottom.iter().position(|id| id == "recording")
            {
                right_bottom.insert(recording_index, "serialSend".to_string());
            } else if let Some(lock_index) = right_bottom.iter().position(|id| id == "lock") {
                right_bottom.insert(lock_index, "serialSend".to_string());
            } else {
                right_bottom.push("serialSend".to_string());
            }
            migrated = true;
        }
    }

    {
        let all_ids: Vec<&str> = settings
            .ui
            .activity_bar_layout
            .left_top
            .iter()
            .chain(&settings.ui.activity_bar_layout.left_bottom)
            .chain(&settings.ui.activity_bar_layout.right_top)
            .chain(&settings.ui.activity_bar_layout.right_bottom)
            .map(|s| s.as_str())
            .collect();
        if !all_ids.contains(&"recording") {
            let right_bottom = &mut settings.ui.activity_bar_layout.right_bottom;
            if let Some(serial_send_index) = right_bottom.iter().position(|id| id == "serialSend") {
                right_bottom.insert(serial_send_index + 1, "recording".to_string());
            } else if let Some(lock_index) = right_bottom.iter().position(|id| id == "lock") {
                right_bottom.insert(lock_index, "recording".to_string());
            } else {
                right_bottom.push("recording".to_string());
            }
            migrated = true;
        }
    }

    for tab in &mut settings.ui.open_tabs {
        if tab.normalize() {
            migrated = true;
        }
    }

    if migrated {
        let mut persisted = settings.clone();
        persisted.cloud_sync = encrypt_cloud_sync_settings(persisted.cloud_sync.clone())?;
        persisted.ai = encrypt_ai_settings(persisted.ai.clone())?;
        let _ = save_app_settings(app, &persisted);
    }

    Ok(settings)
}

pub fn save_app_settings(app: &AppHandle, config: &AppSettings) -> AppResult<()> {
    let dir = get_config_dir(app)?;
    save_json(&dir.join("settings.json"), config)
}
