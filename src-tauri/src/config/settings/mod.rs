mod ai;
mod appearance;
mod diagnostics;
mod general;
mod interaction;
mod proxy;
mod recording;
mod search;
mod security;
mod terminal;
mod transfer;
mod translation;

pub use ai::{
    AI_REQUEST_USER_AGENT_DEFAULT, AgentCommandExecutionMode, AiAgentKind, AiApiFormat,
    AiBackendKind, AiCustomActionConfig, AiMode, AiModelConfigItem, AiModelSource,
    AiPermissionMode, AiProviderCredential, AiProviderKind, AiProviderProfile, AiReasoningEffort,
    AiSettings, ClaudeCodeIntegrationSettings, CodexIntegrationSettings, CodexThreadMode,
    ExternalMcpSessionScope, ExternalMcpSettings, RiskLevel, ai_model_id_for_credential,
    ai_model_id_for_provider, decrypt_ai_settings, encrypt_ai_settings, mask_ai_settings,
    merge_masked_ai_settings, normalize_ai_settings,
};
pub use appearance::{AppearanceSettings, TerminalColorsConfig, ThemeColorsConfig, ThemeConfig};
pub use diagnostics::{DiagnosticsLogLevel, DiagnosticsSettings};
pub use general::GeneralSettings;
pub use interaction::InteractionSettings;
pub use proxy::ProxySettings;
pub use recording::RecordingSettings;
pub use search::{SearchEngine, SearchSettings};
pub use security::SecuritySettings;
pub use terminal::{ActionLinksMatcherSettings, KeywordHighlightRule, TerminalSettings};
pub use transfer::TransferSettings;
pub use translation::TranslationSettings;

use super::cloud_sync::{
    CloudSyncSettings, decrypt_cloud_sync_settings, encrypt_cloud_sync_settings,
    load_cloud_sync_settings,
};
use super::ui::UiConfig;
use crate::error::{AppError, AppResult};
use crate::storage::{self, SettingsDocKey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
    pub recording: RecordingSettings,
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
    /// User-customized keyboard shortcut overrides. Keys are shortcut IDs, values are hotkey strings.
    #[serde(default)]
    pub keybindings: HashMap<String, String>,
}

pub fn load_app_settings(app: &AppHandle) -> AppResult<AppSettings> {
    let raw_settings =
        storage::load_settings_doc::<serde_json::Value>(SettingsDocKey::AppSettings)?;
    let mut settings: AppSettings = if raw_settings.is_null() {
        AppSettings::default()
    } else {
        serde_json::from_value(raw_settings.clone())
            .map_err(|error| AppError::Config(format!("Failed to parse app settings: {error}")))?
    };
    let has_embedded_cloud_sync = raw_settings.get("cloud_sync").is_some();
    let has_legacy_mac_ime_compatibility = raw_settings
        .get("interaction")
        .and_then(|interaction| interaction.as_object())
        .is_some_and(|interaction| {
            interaction.contains_key("mac_ime_compatibility")
                && !interaction.contains_key("ime_compatibility")
        });
    let has_legacy_terminal_right_click_action = raw_settings
        .get("interaction")
        .and_then(|interaction| interaction.as_object())
        .is_some_and(|interaction| !interaction.contains_key("terminal_right_click_action"));

    let mut migrated = false;
    let mut secrets_ready_for_persist = true;

    if has_embedded_cloud_sync {
        match decrypt_cloud_sync_settings(settings.cloud_sync.clone()) {
            Ok(cloud_sync) => {
                settings.cloud_sync = cloud_sync;
            }
            Err(_) => {
                secrets_ready_for_persist = false;
            }
        }
    } else if let Ok(legacy_cloud_sync) =
        load_cloud_sync_settings(app).and_then(decrypt_cloud_sync_settings)
    {
        settings.cloud_sync = legacy_cloud_sync;
        migrated = true;
    }

    match decrypt_ai_settings(settings.ai.clone()) {
        Ok(ai_settings) => {
            settings.ai = ai_settings;
        }
        Err(_) => {
            secrets_ready_for_persist = false;
        }
    }
    if normalize_ai_settings(&mut settings.ai) {
        migrated = true;
    }
    if settings.security.migrate_legacy_screen_lock() {
        migrated = true;
    }
    if migrate_terminal_timestamp_format(&raw_settings, &mut settings.terminal) {
        migrated = true;
    }
    if settings.terminal.normalize_timestamp_format() {
        migrated = true;
    }
    if migrate_legacy_recording_settings(&raw_settings, &mut settings.recording, &settings.transfer)
    {
        migrated = true;
    }
    if settings.recording.normalize() {
        migrated = true;
    }
    if settings.appearance.normalize_terminal_font_family() {
        migrated = true;
    }
    if settings.appearance.normalize_window_transparency() {
        migrated = true;
    }
    if has_legacy_mac_ime_compatibility {
        migrated = true;
    }
    if has_legacy_terminal_right_click_action {
        migrated = true;
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
    for item in settings.ui.activity_bar_layout.hidden_items.iter_mut() {
        if item == "keyManagement" {
            *item = "securityAuth".to_string();
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
        if !all_ids.contains(&"notes") {
            let left_top = &mut settings.ui.activity_bar_layout.left_top;
            if let Some(file_index) = left_top.iter().position(|id| id == "fileExplorer") {
                left_top.insert(file_index + 1, "notes".to_string());
            } else {
                left_top.insert(0, "notes".to_string());
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
        if !all_ids.contains(&"ascendNpuMonitor") {
            let right_top = &mut settings.ui.activity_bar_layout.right_top;
            if let Some(gpu_index) = right_top.iter().position(|id| id == "gpuMonitor") {
                right_top.insert(gpu_index + 1, "ascendNpuMonitor".to_string());
            } else if let Some(resource_index) =
                right_top.iter().position(|id| id == "resourceMonitor")
            {
                right_top.insert(resource_index + 1, "ascendNpuMonitor".to_string());
            } else {
                right_top.push("ascendNpuMonitor".to_string());
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
        if !all_ids.contains(&"gpuMonitor") {
            let right_top = &mut settings.ui.activity_bar_layout.right_top;
            if let Some(resource_index) = right_top.iter().position(|id| id == "resourceMonitor") {
                right_top.insert(resource_index + 1, "gpuMonitor".to_string());
            } else {
                right_top.push("gpuMonitor".to_string());
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
        if !all_ids.contains(&"processManager") {
            let right_top = &mut settings.ui.activity_bar_layout.right_top;
            if let Some(ascend_index) = right_top.iter().position(|id| id == "ascendNpuMonitor") {
                right_top.insert(ascend_index + 1, "processManager".to_string());
            } else if let Some(gpu_index) = right_top.iter().position(|id| id == "gpuMonitor") {
                right_top.insert(gpu_index + 1, "processManager".to_string());
            } else if let Some(resource_index) =
                right_top.iter().position(|id| id == "resourceMonitor")
            {
                right_top.insert(resource_index + 1, "processManager".to_string());
            } else {
                right_top.push("processManager".to_string());
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
        if !all_ids.contains(&"dockerManager") {
            let right_top = &mut settings.ui.activity_bar_layout.right_top;
            if let Some(process_index) = right_top.iter().position(|id| id == "processManager") {
                right_top.insert(process_index + 1, "dockerManager".to_string());
            } else if let Some(resource_index) =
                right_top.iter().position(|id| id == "resourceMonitor")
            {
                right_top.insert(resource_index + 1, "dockerManager".to_string());
            } else {
                right_top.push("dockerManager".to_string());
            }
            migrated = true;
        }
    }

    for tab in &mut settings.ui.open_tabs {
        if tab.normalize() {
            migrated = true;
        }
    }
    if settings.ui.normalize_quick_command_sort_mode() {
        migrated = true;
    }

    if migrated && secrets_ready_for_persist {
        persist_migrated_app_settings(app, &settings);
    }

    Ok(settings)
}

fn migrate_terminal_timestamp_format(
    raw_settings: &serde_json::Value,
    terminal: &mut TerminalSettings,
) -> bool {
    let Some(raw_terminal) = raw_settings
        .get("terminal")
        .and_then(|terminal| terminal.as_object())
    else {
        return false;
    };

    if raw_terminal.contains_key("timestamp_format") {
        return false;
    }

    terminal.timestamp_format = if raw_terminal
        .get("show_timestamp_milliseconds")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        terminal::TIMESTAMP_FORMAT_WITH_MILLISECONDS.to_string()
    } else {
        terminal::DEFAULT_TIMESTAMP_FORMAT.to_string()
    };
    true
}

fn migrate_legacy_recording_settings(
    raw_settings: &serde_json::Value,
    recording: &mut RecordingSettings,
    transfer: &TransferSettings,
) -> bool {
    if raw_settings.get("recording").is_some() {
        return false;
    }

    let Some(raw_transfer) = raw_settings
        .get("transfer")
        .and_then(|value| value.as_object())
    else {
        return false;
    };

    let has_legacy_recording = [
        "recording_path",
        "recording_include_io_labels",
        "recording_include_timestamps",
        "recording_auto_start",
        "recording_memory_limit_bytes",
    ]
    .iter()
    .any(|key| raw_transfer.contains_key(*key));

    if !has_legacy_recording {
        return false;
    }

    recording.auto_start = transfer.recording_auto_start;
    recording.base_path = transfer.recording_path.clone();
    recording.include_io_labels = transfer.recording_include_io_labels;
    recording.include_timestamps = transfer.recording_include_timestamps;
    recording.memory_limit_bytes = transfer.recording_memory_limit_bytes;
    true
}

fn persist_migrated_app_settings(app: &AppHandle, settings: &AppSettings) {
    let mut persisted = settings.clone();
    let Ok(cloud_sync) = encrypt_cloud_sync_settings(persisted.cloud_sync.clone()) else {
        return;
    };
    let Ok(ai) = encrypt_ai_settings(persisted.ai.clone()) else {
        return;
    };

    persisted.cloud_sync = cloud_sync;
    persisted.ai = ai;
    let _ = save_app_settings(app, &persisted);
}

pub fn save_app_settings(app: &AppHandle, config: &AppSettings) -> AppResult<()> {
    let _ = app;
    storage::save_settings_doc(SettingsDocKey::AppSettings, config)
}

#[cfg(test)]
mod tests {
    use super::{
        RecordingSettings, TerminalSettings, TransferSettings, migrate_legacy_recording_settings,
        migrate_terminal_timestamp_format,
    };

    #[test]
    fn migrates_legacy_timestamp_milliseconds_to_format() {
        let raw_settings = serde_json::json!({
            "terminal": {
                "show_timestamps": true,
                "show_timestamp_milliseconds": true
            }
        });
        let mut terminal = TerminalSettings::default();

        assert!(migrate_terminal_timestamp_format(
            &raw_settings,
            &mut terminal
        ));
        assert_eq!(terminal.timestamp_format, "[HH:mm:ss.SSS]");
    }

    #[test]
    fn migrates_legacy_timestamp_seconds_to_format() {
        let raw_settings = serde_json::json!({
            "terminal": {
                "show_timestamps": true,
                "show_timestamp_milliseconds": false
            }
        });
        let mut terminal = TerminalSettings::default();

        assert!(migrate_terminal_timestamp_format(
            &raw_settings,
            &mut terminal
        ));
        assert_eq!(terminal.timestamp_format, "[HH:mm:ss]");
    }

    #[test]
    fn keeps_existing_timestamp_format() {
        let raw_settings = serde_json::json!({
            "terminal": {
                "timestamp_format": "HH:mm:ss",
                "show_timestamp_milliseconds": true
            }
        });
        let mut terminal = TerminalSettings {
            timestamp_format: "HH:mm:ss".to_string(),
            ..TerminalSettings::default()
        };

        assert!(!migrate_terminal_timestamp_format(
            &raw_settings,
            &mut terminal
        ));
        assert_eq!(terminal.timestamp_format, "HH:mm:ss");
    }

    #[test]
    fn migrates_legacy_transfer_recording_settings() {
        let raw_settings = serde_json::json!({
            "transfer": {
                "recording_path": "D:/logs",
                "recording_include_io_labels": false,
                "recording_include_timestamps": false,
                "recording_auto_start": true,
                "recording_memory_limit_bytes": 1048576
            }
        });
        let transfer: TransferSettings =
            serde_json::from_value(raw_settings["transfer"].clone()).expect("transfer");
        let mut recording = RecordingSettings::default();

        assert!(migrate_legacy_recording_settings(
            &raw_settings,
            &mut recording,
            &transfer
        ));
        assert!(recording.auto_start);
        assert_eq!(recording.base_path, "D:/logs");
        assert!(!recording.include_io_labels);
        assert!(!recording.include_timestamps);
        assert_eq!(recording.memory_limit_bytes, 1048576);
    }
}
