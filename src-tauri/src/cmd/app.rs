use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine;
use serde::Deserialize;
use tauri::{Emitter, Manager};

use crate::error::{AppError, AppResult};

#[derive(Default)]
pub struct AppLockState {
    locked: AtomicBool,
}

impl AppLockState {
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::SeqCst)
    }

    pub fn set_locked(&self, locked: bool) -> bool {
        self.locked.swap(locked, Ordering::SeqCst)
    }
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppLockStateChangedPayload {
    locked: bool,
}

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
    kind: Option<ChildWindowKind>,
    parent_label: Option<String>,
    width: Option<f64>,
    height: Option<f64>,
    resizable: Option<bool>,
    always_on_top: Option<bool>,
    state_key: Option<crate::window_state::ChildWindowStateKey>,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum ChildWindowKind {
    Modal,
    Modeless,
}

#[tauri::command]
pub fn quit_application(app: tauri::AppHandle) -> AppResult<()> {
    crate::app::quit_application(&app);
    Ok(())
}

#[tauri::command]
pub fn hide_main_window(app: tauri::AppHandle) -> AppResult<()> {
    crate::app::hide_main_window(&app);
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

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSupportInfo {
    os: String,
    architecture: String,
    runtime: String,
}

#[tauri::command]
pub fn get_support_info(state: tauri::State<'_, crate::runtime::AppRuntime>) -> AppSupportInfo {
    let runtime = state.info().mode;
    AppSupportInfo {
        os: operating_system_label(),
        architecture: std::env::consts::ARCH.to_string(),
        runtime,
    }
}

fn operating_system_label() -> String {
    #[cfg(target_os = "windows")]
    {
        return windows_version_label();
    }

    #[cfg(target_os = "macos")]
    {
        return macos_version_label();
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(contents) = std::fs::read_to_string("/etc/os-release") {
            let name = contents
                .lines()
                .find_map(|line| line.strip_prefix("PRETTY_NAME="))
                .map(|value| value.trim_matches('"').to_string());
            if let Some(name) = name {
                return name;
            }
        }
        return "Linux".to_string();
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        std::env::consts::OS.to_string()
    }
}

#[cfg(target_os = "windows")]
fn windows_version_label() -> String {
    use windows::Win32::System::SystemInformation::OSVERSIONINFOEXW;
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

    const VER_NT_WORKSTATION: u8 = 1;
    type RtlGetVersion = unsafe extern "system" fn(*mut OSVERSIONINFOEXW) -> i32;

    let mut version = OSVERSIONINFOEXW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOEXW>() as u32,
        ..Default::default()
    };

    let Some(rtl_get_version) = (unsafe {
        let ntdll = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());
        if ntdll.is_null() {
            None
        } else {
            GetProcAddress(ntdll, c"RtlGetVersion".as_ptr().cast())
        }
    }) else {
        return "Windows".to_string();
    };

    let rtl_get_version: RtlGetVersion = unsafe { std::mem::transmute(rtl_get_version) };

    // RtlGetVersion queries the current Windows version in-process, avoiding a console process.
    if unsafe { rtl_get_version(&mut version) } >= 0 {
        windows_version_label_from_parts(
            version.dwMajorVersion,
            version.dwMinorVersion,
            version.dwBuildNumber,
            version.wProductType == VER_NT_WORKSTATION,
        )
    } else {
        "Windows".to_string()
    }
}

#[cfg(any(target_os = "windows", test))]
fn windows_version_label_from_parts(
    major: u32,
    minor: u32,
    build: u32,
    workstation: bool,
) -> String {
    if !workstation {
        return format!("Windows Server ({major}.{minor}.{build})");
    }

    let name = match (major, minor, build) {
        (10, 0, 22_000..) => "Windows 11",
        (10, 0, _) => "Windows 10",
        (6, 3, _) => "Windows 8.1",
        (6, 2, _) => "Windows 8",
        (6, 1, _) => "Windows 7",
        _ => "Windows",
    };
    format!("{name} ({major}.{minor}.{build})")
}

#[cfg(target_os = "macos")]
fn macos_version_label() -> String {
    let name = command_output("sw_vers", &["-productName"]).unwrap_or_else(|| "macOS".to_string());
    command_output("sw_vers", &["-productVersion"])
        .map(|version| format!("{name} {version}"))
        .unwrap_or(name)
}

#[cfg(test)]
fn macos_version_label_from_parts(name: Option<&str>, version: Option<&str>) -> String {
    let name = name.unwrap_or("macOS");
    version
        .map(|version| format!("{name} {version}"))
        .unwrap_or_else(|| name.to_string())
}

#[cfg(target_os = "macos")]
fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::{macos_version_label_from_parts, windows_version_label_from_parts};

    #[test]
    fn windows_version_label_uses_native_version_parts() {
        assert_eq!(
            windows_version_label_from_parts(10, 0, 22_631, true),
            "Windows 11 (10.0.22631)"
        );
        assert_eq!(
            windows_version_label_from_parts(10, 0, 19_045, true),
            "Windows 10 (10.0.19045)"
        );
        assert_eq!(
            windows_version_label_from_parts(10, 0, 20_348, false),
            "Windows Server (10.0.20348)"
        );
        assert_eq!(
            windows_version_label_from_parts(10, 0, 26_100, false),
            "Windows Server (10.0.26100)"
        );
    }

    #[test]
    fn macos_version_label_combines_available_parts() {
        assert_eq!(
            macos_version_label_from_parts(Some("macOS"), Some("14.7.1")),
            "macOS 14.7.1"
        );
        assert_eq!(macos_version_label_from_parts(Some("macOS"), None), "macOS");
        assert_eq!(macos_version_label_from_parts(None, None), "macOS");
    }
}

#[tauri::command]
pub fn get_app_lock_state(state: tauri::State<'_, AppLockState>) -> bool {
    state.is_locked()
}

#[tauri::command]
pub fn set_app_lock_state(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppLockState>,
    locked: bool,
) -> bool {
    let previous = state.set_locked(locked);
    if previous != locked {
        let _ = app.emit(
            "app-lock-state-changed",
            AppLockStateChangedPayload { locked },
        );
        if locked {
            if let Some(manager) = app.try_state::<Arc<crate::core::mcp::McpManager>>() {
                let manager = manager.inner().clone();
                tauri::async_runtime::spawn(async move {
                    manager.cancel_pending_approvals().await;
                });
            }
        }
    }
    locked
}

#[tauri::command]
pub async fn open_child_window(
    app: tauri::AppHandle,
    options: ChildWindowOptions,
) -> AppResult<()> {
    if app.get_webview_window(&options.label).is_some() {
        return Ok(());
    }

    let requested_width = options.width.unwrap_or(720.0);
    let requested_height = options.height.unwrap_or(560.0);
    let resizable = options.resizable.unwrap_or(true);
    let state_key = if resizable {
        options
            .state_key
            .or_else(|| crate::window_state::child_window_state_key_for_label(&options.label))
    } else {
        None
    };
    let restored_state = state_key.map(|key| {
        crate::window_state::load_child_window_state(key, requested_width, requested_height)
    });
    let width = restored_state
        .as_ref()
        .map_or(requested_width, |state| state.width);
    let height = restored_state
        .as_ref()
        .map_or(requested_height, |state| state.height);
    let maximized = restored_state.as_ref().is_some_and(|state| state.maximized);
    let kind = options.kind.unwrap_or(ChildWindowKind::Modal);
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
    .maximized(maximized)
    .visible(false)
    .decorations(cfg!(target_os = "macos"))
    .resizable(resizable)
    .always_on_top(options.always_on_top.unwrap_or(false));

    #[cfg(target_os = "macos")]
    {
        builder = builder
            // On macOS, parent/addChildWindow can add the child to the parent hierarchy during
            // creation; keep it unfocusable until the ready handshake to prevent the native window
            // from stealing focus before the page is rendered.
            .focusable(false)
            .focused(false)
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            // Position the traffic light controls in logical points so the 12px native buttons
            // sit visually centered in the 40px custom header.
            .traffic_light_position(tauri::LogicalPosition::new(12.0, 18.0))
            .hidden_title(true);
    }

    if kind == ChildWindowKind::Modal {
        if let Some(parent) = parent_label
            .and_then(|label| app.get_webview_window(label))
            .or_else(|| crate::app::focused_or_first_main_window(&app))
        {
            builder = builder
                .parent(&parent)
                .map_err(|error| AppError::Config(error.to_string()))?;
        }
    }

    if let Some(runtime) = app.try_state::<crate::runtime::AppRuntime>() {
        if runtime.portable() {
            builder = builder.data_directory(runtime.webview_data_dir().to_path_buf());
        }
    }

    let window = builder
        .build()
        .map_err(|error| AppError::Config(error.to_string()))?;

    // macOS addChildWindow:ordered: can bypass builder.visible(false) and place the window
    // above its parent. Order it out immediately after build so the WebView's first frame and
    // page ready handshake complete before an empty window is exposed; revealChildWindow
    // restores focusability before showing it.
    #[cfg(target_os = "macos")]
    {
        let _ = window.hide();
        let _ = window.set_focusable(false);
    }

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

    #[cfg(windows)]
    {
        match open::that(path) {
            Ok(_) => return Ok(()),
            Err(error) if error.raw_os_error() == Some(740) => {
                std::process::Command::new("explorer.exe")
                    .arg(path.as_os_str())
                    .spawn()
                    .map_err(|fallback_error| {
                        AppError::Config(format!(
                            "Failed to open target directory: {fallback_error}; original error: {error}"
                        ))
                    })?;

                return Ok(());
            }
            Err(error) => {
                return Err(AppError::Config(format!(
                    "Failed to open target directory: {error}"
                )));
            }
        }
    }
    #[cfg(not(windows))]
    {
        open::that(path)
            .map_err(|error| AppError::Config(format!("Failed to open target directory: {error}")))
    }
}
