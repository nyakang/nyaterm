use crate::config;
use crate::core::CloudSyncManager;
use crate::error::{AppError, AppResult};
use crate::observability::{self, StructuredLog, StructuredLogLevel};
use crate::utils::crypto;
use crate::utils::fonts::{FontInfo, list_system_font_families, list_system_font_infos};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::Emitter;

#[derive(Debug, Clone, Serialize)]
pub struct KeywordHighlightImportResult {
    pub imported_rules: usize,
    pub updated_rules: usize,
    pub total_rules: usize,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum KeywordHighlightImportFile {
    Config {
        keyword_highlights: Vec<config::KeywordHighlightRule>,
    },
    Rules(Vec<config::KeywordHighlightRule>),
}

fn schedule_cloud_sync_notify(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        crate::core::cloud_sync::notify_config_changed(&app).await;
    });
}

fn window_transparency_settings_changed(
    existing: &config::AppearanceSettings,
    next: &config::AppearanceSettings,
) -> bool {
    existing.window_transparency != next.window_transparency
        || existing.window_transparency_tint != next.window_transparency_tint
        || existing.window_transparency_blur != next.window_transparency_blur
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
    mcp_manager: tauri::State<'_, Arc<crate::core::mcp::McpManager>>,
    settings: config::AppSettings,
    allow_master_password_change: Option<bool>,
    owner_window_label: Option<String>,
) -> AppResult<()> {
    let previous_mcp = config::load_app_settings(&app)?.ai.external_mcp;
    let next_mcp = settings.ai.external_mcp.clone();
    let external_owner = if previous_mcp != next_mcp && next_mcp.enabled {
        let owner = owner_window_label.ok_or_else(|| {
            AppError::Config("External MCP requires an owner main-window label.".into())
        })?;
        crate::cmd::mcp::validate_owner_window(&app, &owner)?;
        Some(owner)
    } else {
        None
    };
    persist_app_settings(
        &app,
        manager.inner(),
        settings,
        allow_master_password_change.unwrap_or(false),
    )
    .await?;
    if previous_mcp != next_mcp {
        if next_mcp.enabled {
            let owner = external_owner.expect("validated External MCP owner");
            if let Err(error) = mcp_manager.configure_external(next_mcp, &owner).await {
                let rollback_mcp = previous_mcp.clone();
                let _ = crate::storage::update_settings_doc(
                    crate::storage::SettingsDocKey::AppSettings,
                    |stored: &mut config::AppSettings| {
                        stored.ai.external_mcp = rollback_mcp;
                        Ok(())
                    },
                );
                if previous_mcp.enabled {
                    let _ = mcp_manager.configure_external(previous_mcp, &owner).await;
                }
                let _ = app.emit("settings-changed", ());
                return Err(error);
            }
        } else {
            mcp_manager.disable_external(false).await?;
        }
    }
    Ok(())
}

#[tauri::command]
pub fn save_app_language(app: tauri::AppHandle, language: String) -> AppResult<()> {
    use crate::storage::{self, SettingsDocKey};

    storage::update_settings_doc(
        SettingsDocKey::AppSettings,
        |settings: &mut config::AppSettings| {
            settings.ui.language = Some(language);
            Ok(())
        },
    )?;

    schedule_cloud_sync_notify(app.clone());
    let _ = app.emit("settings-changed", ());
    crate::tray::schedule_refresh(&app);

    Ok(())
}

#[tauri::command]
pub async fn import_keyword_highlight_rules(
    app: tauri::AppHandle,
    manager: tauri::State<'_, Arc<CloudSyncManager>>,
    file_path: String,
) -> AppResult<KeywordHighlightImportResult> {
    let raw = std::fs::read_to_string(file_path)
        .map_err(|error| AppError::Config(format!("Failed to read import file: {error}")))?;
    let rules = parse_keyword_highlight_import(&raw)?;

    let mut settings = config::load_app_settings(&app)?;
    let result =
        merge_keyword_highlight_rules(&mut settings.terminal.keyword_highlights, rules, || {
            uuid::Uuid::new_v4().to_string()
        })?;

    persist_app_settings(&app, manager.inner(), settings, false).await?;
    Ok(result)
}

#[tauri::command]
pub fn read_theme_file(file_path: String) -> AppResult<config::ThemeConfig> {
    let raw = std::fs::read_to_string(file_path)
        .map_err(|error| AppError::Config(format!("Failed to read theme file: {error}")))?;
    serde_json::from_str::<config::ThemeConfig>(&raw)
        .map_err(|error| AppError::Config(format!("Failed to parse theme file: {error}")))
}

#[tauri::command]
pub fn write_theme_file(output_path: String, theme: config::ThemeConfig) -> AppResult<()> {
    let raw = serde_json::to_string_pretty(&theme)
        .map_err(|error| AppError::Config(format!("Failed to serialize theme: {error}")))?;
    std::fs::write(output_path, raw)
        .map_err(|error| AppError::Config(format!("Failed to write theme file: {error}")))
}

pub async fn persist_app_settings(
    app: &tauri::AppHandle,
    manager: &Arc<CloudSyncManager>,
    mut settings: config::AppSettings,
    allow_master_password_change: bool,
) -> AppResult<()> {
    settings.appearance.normalize_terminal_font_family();
    settings.appearance.normalize_window_transparency();
    settings.terminal.normalize_scrollback_lines();
    settings.terminal.normalize_timestamp_format();
    settings.recording.normalize();

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
    let should_apply_window_transparency =
        window_transparency_settings_changed(&existing.appearance, &settings.appearance);
    let had_existing_password = existing.security.master_password.is_some();

    let master_password_action: &'static str;
    match settings.security.master_password.as_deref() {
        Some("__SET__") => {
            master_password_action = "preserve_existing";
            settings.security.master_password = existing.security.master_password;
        }
        Some("") => {
            log_master_password_persist_action("reject_empty", had_existing_password);
            return Err(AppError::Config(
                "Master password cannot be empty when enabled".to_string(),
            ));
        }
        None => {
            master_password_action = if had_existing_password {
                "remove_existing"
            } else {
                "keep_unset"
            };
            if had_existing_password {
                let old_plain = crypto::decrypt_settings_secret(
                    existing.security.master_password.as_deref().unwrap(),
                )?;
                crypto::rewrap_master_key(Some(&old_plain), None)?;
                crypto::set_master_password(None);
            }
            settings.security.master_password = None;
        }
        Some(plain) => {
            if !allow_master_password_change {
                let action = if had_existing_password {
                    "reject_unconfirmed_replace"
                } else {
                    "reject_unconfirmed_set"
                };
                log_master_password_persist_action(action, had_existing_password);
                return Err(AppError::Config(
                    "Master password change requires explicit confirmation".to_string(),
                ));
            }
            master_password_action = if had_existing_password {
                "replace_existing"
            } else {
                "set_new"
            };
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
    log_master_password_persist_action(master_password_action, had_existing_password);
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
    if should_apply_window_transparency {
        crate::app::apply_window_transparency_to_all(app);
    }
    let _ = app.emit("settings-changed", ());
    crate::tray::schedule_refresh(app);

    Ok(())
}

#[tauri::command]
pub fn save_app_ui_settings(ui: config::UiConfig) -> AppResult<()> {
    use crate::storage::{self, SettingsDocKey};
    storage::update_settings_doc(
        SettingsDocKey::AppSettings,
        |settings: &mut config::AppSettings| {
            settings.ui = ui;
            Ok(())
        },
    )
}

#[tauri::command]
pub fn verify_master_password(app: tauri::AppHandle, password: String) -> AppResult<bool> {
    let settings = config::load_app_settings(&app)?;
    match settings.security.master_password {
        Some(ref ct) => {
            let stored = crypto::decrypt_settings_secret(ct)?;
            let verified = stored == password;
            observability::log_event(StructuredLog {
                level: StructuredLogLevel::Debug,
                domain: "security.master_password".to_string(),
                event: "verify.result".to_string(),
                message: "Master password verification completed".to_string(),
                ids: None,
                data: Some(serde_json::json!({
                    "has_stored_password": true,
                    "password_length": password.chars().count(),
                    "verified": verified,
                })),
                error: None,
                client_timestamp: None,
            });
            Ok(verified)
        }
        None => {
            observability::log_event(StructuredLog {
                level: StructuredLogLevel::Debug,
                domain: "security.master_password".to_string(),
                event: "verify.result".to_string(),
                message: "Master password verification completed".to_string(),
                ids: None,
                data: Some(serde_json::json!({
                    "has_stored_password": false,
                    "password_length": password.chars().count(),
                    "verified": true,
                })),
                error: None,
                client_timestamp: None,
            });
            Ok(true)
        }
    }
}

fn log_master_password_persist_action(action: &'static str, had_existing_password: bool) {
    observability::log_event(StructuredLog {
        level: StructuredLogLevel::Debug,
        domain: "security.master_password".to_string(),
        event: "persist.action".to_string(),
        message: "Master password persistence branch selected".to_string(),
        ids: None,
        data: Some(serde_json::json!({
            "action": action,
            "had_existing_password": had_existing_password,
        })),
        error: None,
        client_timestamp: None,
    });
}

fn parse_keyword_highlight_import(raw: &str) -> AppResult<Vec<config::KeywordHighlightRule>> {
    let import_file: KeywordHighlightImportFile = serde_json::from_str(raw)
        .map_err(|error| AppError::Config(format!("Invalid highlight rules JSON: {error}")))?;

    Ok(match import_file {
        KeywordHighlightImportFile::Config { keyword_highlights } => keyword_highlights,
        KeywordHighlightImportFile::Rules(rules) => rules,
    })
}

fn merge_keyword_highlight_rules(
    existing: &mut Vec<config::KeywordHighlightRule>,
    imported: Vec<config::KeywordHighlightRule>,
    mut next_id: impl FnMut() -> String,
) -> AppResult<KeywordHighlightImportResult> {
    let mut imported_rules = 0;
    let mut updated_rules = 0;
    let mut indexes = existing
        .iter()
        .enumerate()
        .filter_map(|(index, rule)| (!rule.id.trim().is_empty()).then(|| (rule.id.clone(), index)))
        .collect::<HashMap<_, _>>();

    for mut rule in imported {
        rule.name = rule.name.trim().to_string();
        rule.patterns = rule
            .patterns
            .into_iter()
            .map(|pattern| pattern.trim().to_string())
            .filter(|pattern| !pattern.is_empty())
            .collect();

        if rule.name.is_empty() || rule.patterns.is_empty() {
            continue;
        }

        rule.id = rule.id.trim().to_string();
        if rule.id.is_empty() {
            rule.id = next_id();
        }

        if let Some(index) = indexes.get(&rule.id).copied() {
            existing[index] = rule;
            updated_rules += 1;
        } else {
            let id = rule.id.clone();
            existing.push(rule);
            indexes.insert(id, existing.len() - 1);
            imported_rules += 1;
        }
    }

    if imported_rules == 0 && updated_rules == 0 {
        return Err(AppError::Config(
            "No valid highlight rules found in import file".to_string(),
        ));
    }

    Ok(KeywordHighlightImportResult {
        imported_rules,
        updated_rules,
        total_rules: existing.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn existing_rule() -> config::KeywordHighlightRule {
        config::KeywordHighlightRule {
            id: "deploy-errors".to_string(),
            name: "Deploy Errors".to_string(),
            patterns: vec!["fatal".to_string()],
            color_dark: "#ff7b72".to_string(),
            color_light: "#cf222e".to_string(),
            enabled: true,
        }
    }

    fn appearance_with_window_transparency(
        mode: &str,
        tint: f64,
        blur: bool,
    ) -> config::AppearanceSettings {
        config::AppearanceSettings {
            window_transparency: mode.to_string(),
            window_transparency_tint: tint,
            window_transparency_blur: blur,
            ..Default::default()
        }
    }

    #[test]
    fn window_transparency_change_detection_only_tracks_native_fields() {
        let base = appearance_with_window_transparency("transparent", 0.6, false);

        let mut unrelated = base.clone();
        unrelated.theme = "github-light".to_string();
        assert!(!window_transparency_settings_changed(&base, &unrelated));

        let changed_mode = appearance_with_window_transparency("none", 1.0, false);
        assert!(window_transparency_settings_changed(&base, &changed_mode));

        let changed_tint = appearance_with_window_transparency("transparent", 0.4, false);
        assert!(window_transparency_settings_changed(&base, &changed_tint));

        let changed_blur = appearance_with_window_transparency("transparent", 0.6, true);
        assert!(window_transparency_settings_changed(&base, &changed_blur));
    }

    #[test]
    fn import_keyword_highlight_rules_parses_object_format() {
        let raw = r##"{
            "keyword_highlights": [
                {
                    "id": "deploy-errors",
                    "name": "Deploy Errors",
                    "patterns": ["deploy failed"],
                    "color_dark": "#ff7b72",
                    "color_light": "#cf222e",
                    "enabled": true
                }
            ]
        }"##;

        let rules = parse_keyword_highlight_import(raw).expect("parse object");

        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].id, "deploy-errors");
        assert_eq!(rules[0].patterns, vec!["deploy failed"]);
    }

    #[test]
    fn import_keyword_highlight_rules_parses_array_format() {
        let raw = r##"[
            {
                "name": "Warnings",
                "patterns": ["warn"]
            }
        ]"##;

        let rules = parse_keyword_highlight_import(raw).expect("parse array");

        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].name, "Warnings");
        assert_eq!(rules[0].color_dark, "#79c0ff");
        assert_eq!(rules[0].color_light, "#0969da");
        assert!(rules[0].enabled);
    }

    #[test]
    fn import_keyword_highlight_rules_updates_existing_id() {
        let mut existing = vec![existing_rule()];
        let imported = parse_keyword_highlight_import(
            r##"[{
                "id": "deploy-errors",
                "name": "Deploy Failures",
                "patterns": ["rollback required"],
                "color_dark": "#ffa198",
                "color_light": "#a40e26",
                "enabled": false
            }]"##,
        )
        .expect("parse");

        let result =
            merge_keyword_highlight_rules(&mut existing, imported, || "generated-id".to_string())
                .expect("merge");

        assert_eq!(result.imported_rules, 0);
        assert_eq!(result.updated_rules, 1);
        assert_eq!(result.total_rules, 1);
        assert_eq!(existing[0].name, "Deploy Failures");
        assert_eq!(existing[0].patterns, vec!["rollback required"]);
        assert!(!existing[0].enabled);
    }

    #[test]
    fn import_keyword_highlight_rules_adds_generated_id_and_defaults() {
        let mut existing = Vec::new();
        let imported = parse_keyword_highlight_import(
            r##"[{
                "name": " Status ",
                "patterns": [" success ", "", " done "]
            }]"##,
        )
        .expect("parse");

        let result =
            merge_keyword_highlight_rules(&mut existing, imported, || "generated-id".to_string())
                .expect("merge");

        assert_eq!(result.imported_rules, 1);
        assert_eq!(result.updated_rules, 0);
        assert_eq!(existing[0].id, "generated-id");
        assert_eq!(existing[0].name, "Status");
        assert_eq!(existing[0].patterns, vec!["success", "done"]);
        assert_eq!(existing[0].color_dark, "#79c0ff");
        assert_eq!(existing[0].color_light, "#0969da");
        assert!(existing[0].enabled);
    }

    #[test]
    fn import_keyword_highlight_rules_rejects_empty_or_invalid_rules() {
        let mut existing = Vec::new();
        let imported = parse_keyword_highlight_import(
            r##"{
                "keyword_highlights": [
                    { "name": "", "patterns": ["fatal"] },
                    { "name": "Empty Patterns", "patterns": ["", " "] }
                ]
            }"##,
        )
        .expect("parse");

        let error =
            merge_keyword_highlight_rules(&mut existing, imported, || "generated-id".to_string())
                .expect_err("invalid rules should fail");

        assert!(error.to_string().contains("No valid highlight rules"));
        assert!(existing.is_empty());
    }
}
