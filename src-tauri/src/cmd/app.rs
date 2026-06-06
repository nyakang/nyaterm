use std::collections::HashSet;
use std::path::{Path, PathBuf};

use base64::Engine;
use serde::Deserialize;
use tauri::Manager;

use crate::error::{AppError, AppResult};

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalDropPathEntry {
    path: String,
    is_dir: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildWindowOptions {
    label: String,
    title: String,
    url: String,
    parent_label: Option<String>,
    width: Option<f64>,
    height: Option<f64>,
    resizable: Option<bool>,
    always_on_top: Option<bool>,
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
pub fn open_log_dir(app: tauri::AppHandle) -> AppResult<()> {
    let path = crate::runtime::log_dir(&app)?;
    if !path.exists() {
        std::fs::create_dir_all(&path)?;
    }
    open_folder(&path)
}

#[tauri::command]
pub fn get_app_runtime_info(
    state: tauri::State<'_, crate::runtime::AppRuntime>,
) -> crate::runtime::AppRuntimeInfo {
    state.info()
}

#[tauri::command]
pub async fn open_child_window(
    app: tauri::AppHandle,
    options: ChildWindowOptions,
) -> AppResult<()> {
    if app.get_webview_window(&options.label).is_some() {
        return Ok(());
    }

    let width = options.width.unwrap_or(720.0);
    let height = options.height.unwrap_or(560.0);
    let parent_label = options
        .parent_label
        .as_deref()
        .filter(|label| crate::window_state::is_main_window_label(label));
    let placement =
        crate::window_state::center_child_in_parent_monitor(&app, parent_label, width, height);

    let mut builder = tauri::WebviewWindowBuilder::new(
        &app,
        options.label,
        tauri::WebviewUrl::App(options.url.into()),
    )
    .title(options.title)
    .inner_size(width, height)
    .visible(false)
    .decorations(cfg!(target_os = "macos"))
    .resizable(options.resizable.unwrap_or(true))
    .always_on_top(options.always_on_top.unwrap_or(false));

    #[cfg(target_os = "macos")]
    {
        builder = builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true);
    }

    if let Some(parent) = parent_label
        .and_then(|label| app.get_webview_window(label))
        .or_else(|| crate::app::focused_or_first_main_window(&app))
    {
        builder = builder
            .parent(&parent)
            .map_err(|error| AppError::Config(error.to_string()))?;
    }

    if let Some(runtime) = app.try_state::<crate::runtime::AppRuntime>() {
        if runtime.portable() {
            builder = builder.data_directory(runtime.webview_data_dir().to_path_buf());
        }
    }

    let window = builder
        .build()
        .map_err(|error| AppError::Config(error.to_string()))?;

    if let Some(placement) = placement {
        if window
            .set_position(crate::window_state::placement_to_position(placement))
            .is_err()
        {
            let _ = window.center();
        }
    } else {
        let _ = window.center();
    }

    Ok(())
}

#[tauri::command]
pub fn open_transfer_target_directory(transfer_id: String) -> AppResult<()> {
    let path = crate::core::sftp::transfer_target_directory(&transfer_id)?;
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

const MAX_BACKGROUND_IMAGE_SIZE: u64 = 50 * 1024 * 1024; // 50 MB

#[tauri::command]
pub fn read_background_image_data_url(path: String) -> AppResult<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(AppError::Config(
            "Background image path is empty".to_string(),
        ));
    }

    let path = PathBuf::from(trimmed);
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);

    let mime = match extension.as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        _ => {
            return Err(AppError::Config(
                "Unsupported background image format".to_string(),
            ));
        }
    };

    let metadata = std::fs::metadata(&path).map_err(|_| {
        AppError::Config(format!(
            "Background image file not found: {}",
            path.display()
        ))
    })?;

    if metadata.len() > MAX_BACKGROUND_IMAGE_SIZE {
        return Err(AppError::Config(format!(
            "Background image too large ({:.1} MB, max {:.0} MB)",
            metadata.len() as f64 / (1024.0 * 1024.0),
            MAX_BACKGROUND_IMAGE_SIZE as f64 / (1024.0 * 1024.0),
        )));
    }

    let bytes = std::fs::read(&path)?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);

    Ok(format!("data:{mime};base64,{encoded}"))
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
