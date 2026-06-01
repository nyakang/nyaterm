use crate::config;
use crate::core::CloudSyncManager;
use crate::error::{AppError, AppResult};
use crate::observability::{self, StructuredLog, StructuredLogLevel};
use crate::utils::crypto;
use crate::utils::fonts::{list_system_font_families, list_system_font_infos, FontInfo};
use std::sync::Arc;
use tauri::Emitter;

fn schedule_cloud_sync_notify(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        crate::core::cloud_sync::notify_config_changed(&app).await;
    });
}

#[tauri::command]
pub async fn get_system_fonts() -> Vec<String> {
    tauri::async_runtime::spawn_blocking(list_system_font_families)
        .await
        .unwrap_or_default()
}

#[tauri::command]
pub async fn get_system_font_infos() -> Vec<FontInfo> {
    tauri::async_runtime::spawn_blocking(list_system_font_infos)
        .await
        .unwrap_or_default()
}

#[tauri::command]
pub fn get_app_settings(app: tauri::AppHandle) -> AppResult<config::AppSettings> {
    let mut settings = config::load_app_settings(&app)?;
    if settings.security.master_password.is_some() {
        settings.security.master_password = Some("__SET__".to_string());
    }
    settings.cloud_sync = config::mask_cloud_sync_settings(settings.cloud_sync);
    settings.ai = config::mask_ai_settings(settings.ai);
    Ok(settings)
}

#[tauri::command]
pub async fn save_app_settings(
    app: tauri::AppHandle,
    manager: tauri::State<'_, Arc<CloudSyncManager>>,
    settings: config::AppSettings,
) -> AppResult<()> {
    persist_app_settings(&app, manager.inner(), settings).await
}

pub async fn persist_app_settings(
    app: &tauri::AppHandle,
    manager: &Arc<CloudSyncManager>,
    mut settings: config::AppSettings,
) -> AppResult<()> {
    settings.appearance.normalize_terminal_font_family();

    let existing = match config::load_app_settings(app) {
        Ok(existing) => existing,
        Err(error) => {
            observability::log_event(StructuredLog {
                level: StructuredLogLevel::Error,
                domain: "settings.persistence".to_string(),
                event: "settings.load_failed".to_string(),
                message: "Failed to load existing app settings before save".to_string(),
                ids: None,
                data: None,
                error: Some(serde_json::json!({ "message": error.to_string() })),
                client_timestamp: None,
            });
            return Err(error);
        }
    };

    match settings.security.master_password.as_deref() {
        Some("__SET__") => {
            settings.security.master_password = existing.security.master_password;
        }
        Some("") => {
            return Err(AppError::Config(
                "Master password cannot be empty when enabled".to_string(),
            ));
        }
        None => {
            if existing.security.master_password.is_some() {
                let old_plain = crypto::decrypt_settings_secret(
                    existing.security.master_password.as_deref().unwrap(),
                )?;
                crypto::rewrap_master_key(Some(&old_plain), None)?;
                crypto::set_master_password(None);
            }
            settings.security.master_password = None;
        }
        Some(plain) => {
            let old_plain = existing
                .security
                .master_password
                .as_deref()
                .and_then(|ct| crypto::decrypt_settings_secret(ct).ok());

            crypto::rewrap_master_key(old_plain.as_deref(), Some(plain))?;
            crypto::set_master_password(Some(plain.to_string()));

            settings.security.master_password = Some(crypto::encrypt_settings_secret(plain)?);
        }
    }
    let merged_cloud_sync =
        config::merge_masked_cloud_sync_settings(&existing.cloud_sync, settings.cloud_sync);
    settings.cloud_sync = merged_cloud_sync.clone();
    let merged_ai = config::merge_masked_ai_settings(&existing.ai, settings.ai);
    settings.ai = merged_ai.clone();

    let mut persisted_settings = settings.clone();
    persisted_settings.cloud_sync = config::encrypt_cloud_sync_settings(merged_cloud_sync.clone())?;
    persisted_settings.ai = config::encrypt_ai_settings(merged_ai)?;

    if let Err(error) = config::save_app_settings(app, &persisted_settings) {
        observability::log_event(StructuredLog {
            level: StructuredLogLevel::Error,
            domain: "settings.persistence".to_string(),
            event: "settings.save_failed".to_string(),
            message: "Failed to persist app settings".to_string(),
            ids: None,
            data: Some(serde_json::json!({
                "diagnostics_level": settings.diagnostics.level.as_str(),
                "diagnostics_retention_days": settings.diagnostics.retention_days,
            })),
            error: Some(serde_json::json!({ "message": error.to_string() })),
            client_timestamp: None,
        });
        return Err(error);
    }

    manager.replace_settings(merged_cloud_sync).await?;
    schedule_cloud_sync_notify(app.clone());
    let _ = app.emit("settings-changed", ());
    crate::tray::schedule_refresh(app);

    Ok(())
}

#[tauri::command]
pub fn save_app_ui_settings(ui: config::UiConfig) -> AppResult<()> {
    use crate::storage::{self, SettingsDocKey};
    storage::update_settings_doc(SettingsDocKey::AppSettings, |settings: &mut config::AppSettings| {
        settings.ui = ui;
        Ok(())
    })
}

#[tauri::command]
pub fn verify_master_password(app: tauri::AppHandle, password: String) -> AppResult<bool> {
    let settings = config::load_app_settings(&app)?;
    match settings.security.master_password {
        Some(ref ct) => {
            let stored = crypto::decrypt_settings_secret(ct)?;
            Ok(stored == password)
        }
        None => Ok(true),
    }
}
