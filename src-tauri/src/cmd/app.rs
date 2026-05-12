use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalDropPathEntry {
    path: String,
    is_dir: bool,
}

#[tauri::command]
pub fn quit_application(app: tauri::AppHandle) -> AppResult<()> {
    crate::app::quit_application(&app);
    Ok(())
}

#[tauri::command]
pub fn open_download_dir(app: tauri::AppHandle) -> AppResult<()> {
    let path = resolve_download_dir(&app)?;

    if path.exists() {
        if !path.is_dir() {
            return Err(AppError::Config(
                "Configured download path is not a directory".to_string(),
            ));
        }
    } else {
        std::fs::create_dir_all(&path)?;
    }

    open_folder(&path)
}

#[tauri::command]
pub fn open_transfer_target_directory(transfer_id: String) -> AppResult<()> {
    let path = crate::core::ssh::sftp::transfer_target_directory(&transfer_id)?;
    open_folder(&path)
}

#[tauri::command]
pub fn resolve_local_drop_paths(paths: Vec<String>) -> AppResult<Vec<LocalDropPathEntry>> {
    let mut resolved = Vec::new();
    let mut seen = HashSet::new();

    for raw_path in paths {
        let trimmed = raw_path.trim();
        if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
            continue;
        }

        let path = std::path::PathBuf::from(trimmed);
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };

        resolved.push(LocalDropPathEntry {
            path: path.to_string_lossy().to_string(),
            is_dir: metadata.is_dir(),
        });
    }

    Ok(resolved)
}

fn resolve_download_dir(app: &tauri::AppHandle) -> AppResult<PathBuf> {
    let configured = crate::config::load_app_settings(app)?
        .transfer
        .download_path
        .trim()
        .to_string();

    if configured.is_empty() {
        return default_download_dir();
    }

    Ok(expand_home_path(&configured))
}

fn default_download_dir() -> AppResult<PathBuf> {
    dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Downloads")))
        .ok_or_else(|| AppError::Config("Cannot determine system download directory".to_string()))
}

fn expand_home_path(path: &str) -> PathBuf {
    if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }

    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }

    PathBuf::from(path)
}

fn open_folder(path: &Path) -> AppResult<()> {
    if !path.is_dir() {
        return Err(AppError::Config(
            "Target path is not a directory".to_string(),
        ));
    }

    open::that(path)
        .map_err(|error| AppError::Config(format!("Failed to open target directory: {error}")))
}
