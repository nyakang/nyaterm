use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::Manager;

use crate::error::{AppError, AppResult};

const DEFAULT_IDENTIFIER: &str = "com.kang.nyaterm";
const PORTABLE_MARKER_FILE: &str = "portable.flag";
const PORTABLE_KEY_FILE: &str = "portable.key";

#[derive(Clone, Debug)]
pub struct AppRuntime {
    portable: bool,
    executable_dir: PathBuf,
    data_dir: PathBuf,
    config_dir: PathBuf,
    log_dir: PathBuf,
    webview_data_dir: PathBuf,
    portable_marker_path: Option<PathBuf>,
    portable_key_path: Option<PathBuf>,
    tauri_identifier: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppRuntimeInfo {
    pub portable: bool,
    pub mode: String,
    pub executable_dir: String,
    pub data_dir: String,
    pub config_dir: String,
    pub log_dir: String,
    pub webview_data_dir: String,
    pub portable_marker_path: Option<String>,
}

impl AppRuntime {
    pub fn portable(&self) -> bool {
        self.portable
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub fn log_dir(&self) -> &Path {
        &self.log_dir
    }

    pub fn webview_data_dir(&self) -> &Path {
        &self.webview_data_dir
    }

    pub fn portable_key_path(&self) -> Option<&Path> {
        self.portable_key_path.as_deref()
    }

    pub fn tauri_identifier(&self) -> &str {
        &self.tauri_identifier
    }

    pub fn ensure_directories(&self) -> AppResult<()> {
        std::fs::create_dir_all(&self.config_dir)?;
        std::fs::create_dir_all(&self.log_dir)?;
        std::fs::create_dir_all(&self.webview_data_dir)?;
        Ok(())
    }

    pub fn info(&self) -> AppRuntimeInfo {
        AppRuntimeInfo {
            portable: self.portable,
            mode: if self.portable {
                "portable".to_string()
            } else {
                "installed".to_string()
            },
            executable_dir: path_to_string(&self.executable_dir),
            data_dir: path_to_string(&self.data_dir),
            config_dir: path_to_string(&self.config_dir),
            log_dir: path_to_string(&self.log_dir),
            webview_data_dir: path_to_string(&self.webview_data_dir),
            portable_marker_path: self
                .portable_marker_path
                .as_ref()
                .map(|path| path_to_string(path)),
        }
    }
}

pub fn resolve() -> AppResult<AppRuntime> {
    let executable_dir = executable_dir()?;
    let portable_marker_path = executable_dir.join(PORTABLE_MARKER_FILE);
    let portable = portable_marker_path.exists() && cfg!(windows);

    if portable {
        return resolve_portable(executable_dir, portable_marker_path);
    }

    resolve_installed(executable_dir)
}

pub fn apply_to_context<R: tauri::Runtime>(context: &mut tauri::Context<R>, runtime: &AppRuntime) {
    if runtime.portable {
        context.config_mut().identifier = runtime.tauri_identifier().to_string();
    }
}

pub fn prepare_webview_environment(runtime: &AppRuntime) {
    if runtime.portable {
        #[cfg(windows)]
        // Called during app startup before Tauri initializes WebView threads.
        unsafe {
            std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", runtime.webview_data_dir());
        }
    }
}

pub fn log_dir(app: &tauri::AppHandle) -> AppResult<PathBuf> {
    if let Some(runtime) = app.try_state::<AppRuntime>() {
        return Ok(runtime.log_dir().to_path_buf());
    }

    app.path()
        .app_log_dir()
        .map_err(|error| AppError::Config(error.to_string()))
}

fn resolve_portable(
    executable_dir: PathBuf,
    portable_marker_path: PathBuf,
) -> AppResult<AppRuntime> {
    let data_dir = executable_dir.join("data");
    let config_dir = data_dir.join("config");
    let log_dir = data_dir.join("logs");
    let webview_data_dir = data_dir.join("webview");
    let portable_key_path = config_dir.join(PORTABLE_KEY_FILE);
    let tauri_identifier = format!(
        "{DEFAULT_IDENTIFIER}.portable.{}",
        stable_path_hash(&executable_dir)
    );

    Ok(AppRuntime {
        portable: true,
        executable_dir,
        data_dir,
        config_dir,
        log_dir,
        webview_data_dir,
        portable_marker_path: Some(portable_marker_path),
        portable_key_path: Some(portable_key_path),
        tauri_identifier,
    })
}

fn resolve_installed(executable_dir: PathBuf) -> AppResult<AppRuntime> {
    let home = dirs::home_dir()
        .ok_or_else(|| AppError::Config("cannot determine home directory".to_string()))?;
    let config_dir = home.join(".nyaterm");
    let log_dir = installed_log_dir()?;
    let webview_data_dir = installed_webview_data_dir()?;

    Ok(AppRuntime {
        portable: false,
        executable_dir,
        data_dir: config_dir.clone(),
        config_dir,
        log_dir,
        webview_data_dir,
        portable_marker_path: None,
        portable_key_path: None,
        tauri_identifier: DEFAULT_IDENTIFIER.to_string(),
    })
}

fn executable_dir() -> AppResult<PathBuf> {
    let exe = std::env::current_exe()?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| AppError::Config("failed to resolve executable directory".to_string()))
}

fn installed_log_dir() -> AppResult<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir()
            .ok_or_else(|| AppError::Config("cannot determine home directory".to_string()))?;
        return Ok(home.join("Library").join("Logs").join(DEFAULT_IDENTIFIER));
    }

    #[cfg(not(target_os = "macos"))]
    {
        let local_data = dirs::data_local_dir()
            .ok_or_else(|| AppError::Config("cannot determine local data directory".to_string()))?;
        Ok(local_data.join(DEFAULT_IDENTIFIER).join("logs"))
    }
}

fn installed_webview_data_dir() -> AppResult<PathBuf> {
    let local_data = dirs::data_local_dir()
        .ok_or_else(|| AppError::Config("cannot determine local data directory".to_string()))?;
    Ok(local_data.join(DEFAULT_IDENTIFIER))
}

fn stable_path_hash(path: &Path) -> String {
    let normalized = path_to_string(path).to_ascii_lowercase();
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..6])
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}
