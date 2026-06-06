use std::sync::Arc;
use tauri::Manager;

use crate::core::{CloudSyncManager, QuickCommandsStore, SessionManager};
use crate::runtime::AppRuntime;

fn main_window_config<R: tauri::Runtime>(
    manager: &impl Manager<R>,
) -> Result<tauri::utils::config::WindowConfig, Box<dyn std::error::Error>> {
    manager
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == crate::window_state::MAIN_WINDOW_LABEL)
        .cloned()
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "main window config not found").into()
        })
}

fn apply_main_window_options<'a, R: tauri::Runtime, M: Manager<R>>(
    manager: &'a M,
    mut builder: tauri::WebviewWindowBuilder<'a, R, M>,
) -> tauri::WebviewWindowBuilder<'a, R, M> {
    let window_state = crate::window_state::load_main_window_state();
    builder = builder
        .inner_size(window_state.width, window_state.height)
        .maximized(window_state.maximized);

    if let Some(runtime) = manager.try_state::<AppRuntime>() {
        if runtime.portable() {
            builder = builder.data_directory(runtime.webview_data_dir().to_path_buf());
        }
    }

    builder
}

fn install_main_window_bridges(window: &tauri::WebviewWindow) {
    if let Err(error) = crate::platform::install_external_file_drop_bridge(window) {
        tracing::warn!(
            window_label = window.label(),
            "Failed to install Windows external file drop bridge for main window: {}",
            error
        );
    }
}

fn create_main_window_with_label(
    app: &mut tauri::App,
    label: &str,
) -> Result<tauri::WebviewWindow, Box<dyn std::error::Error>> {
    if let Some(window) = app.get_webview_window(label) {
        return Ok(window);
    }

    let mut config = main_window_config(app)?;
    config.label = label.to_string();
    let builder = tauri::WebviewWindowBuilder::from_config(app, &config)?;
    let window = apply_main_window_options(app, builder).build()?;
    install_main_window_bridges(&window);
    Ok(window)
}

pub fn create_additional_main_window(
    app: &tauri::AppHandle,
) -> Result<tauri::WebviewWindow, Box<dyn std::error::Error>> {
    let label = next_main_window_label(app);
    let mut config = main_window_config(app)?;
    config.label = label;
    let builder = tauri::WebviewWindowBuilder::from_config(app, &config)?;
    let window = apply_main_window_options(app, builder).build()?;
    install_main_window_bridges(&window);
    focus_window(&window);
    crate::tray::schedule_refresh(app);
    Ok(window)
}

fn next_main_window_label(app: &tauri::AppHandle) -> String {
    loop {
        let label = format!(
            "{}{}",
            crate::window_state::MAIN_WINDOW_PREFIX,
            uuid::Uuid::new_v4()
        );
        if app.get_webview_window(&label).is_none() {
            return label;
        }
    }
}

pub fn main_windows(app: &tauri::AppHandle) -> Vec<tauri::WebviewWindow> {
    app.webview_windows()
        .into_values()
        .filter(|window| crate::window_state::is_main_window_label(window.label()))
        .collect()
}

pub fn focused_or_first_main_window(app: &tauri::AppHandle) -> Option<tauri::WebviewWindow> {
    let windows = main_windows(app);
    windows
        .iter()
        .find(|window| window.is_focused().unwrap_or(false))
        .cloned()
        .or_else(|| {
            windows
                .iter()
                .find(|window| window.label() == crate::window_state::MAIN_WINDOW_LABEL)
                .cloned()
        })
        .or_else(|| windows.into_iter().next())
}

fn focus_window(window: &tauri::WebviewWindow) {
    if window.is_minimized().unwrap_or(false) {
        let _ = window.unminimize();
    }
    let _ = window.show();
    let _ = window.set_focus();
}

fn close_scoped_child_windows(app: &tauri::AppHandle, main_label: &str) {
    let suffix = if main_label == crate::window_state::MAIN_WINDOW_LABEL {
        None
    } else {
        Some(main_label)
    };
    let labels = match suffix {
        Some(suffix) => vec![
            format!("settings-{suffix}"),
            format!("new-session-{suffix}"),
            format!("quick-command-{suffix}"),
        ],
        None => vec![
            "settings".to_string(),
            "new-session".to_string(),
            "quick-command".to_string(),
        ],
    };

    for label in labels {
        if let Some(child) = app.get_webview_window(&label) {
            let _ = child.close();
        }
    }
}

pub fn setup(
    app: &mut tauri::App,
    session_manager: Arc<SessionManager>,
    quick_commands_store: Arc<QuickCommandsStore>,
    cloud_sync_manager: Arc<CloudSyncManager>,
    runtime: AppRuntime,
) -> Result<(), Box<dyn std::error::Error>> {
    runtime.ensure_directories()?;
    let portable_key_path = runtime.portable_key_path().map(ToOwned::to_owned);
    crate::utils::crypto::set_portable_key_path(portable_key_path);
    crate::storage::init(runtime.config_dir())?;
    app.manage(runtime.clone());

    let settings_load = crate::config::load_app_settings(app.handle());
    let diagnostics = settings_load
        .as_ref()
        .map(|settings| settings.diagnostics.clone())
        .unwrap_or_default();
    crate::observability::init_tracing(runtime.log_dir().to_path_buf(), &diagnostics);

    if let Err(error) = settings_load {
        crate::observability::log_event(crate::observability::StructuredLog {
            level: crate::observability::StructuredLogLevel::Warn,
            domain: "settings.persistence".to_string(),
            event: "settings.load_failed".to_string(),
            message: "Failed to load app settings before tracing initialization".to_string(),
            ids: None,
            data: None,
            error: Some(serde_json::json!({ "message": error.to_string() })),
            client_timestamp: None,
        });
    }

    session_manager.set_app_handle(app.handle().clone());

    // Restore the master password for wrapping-key derivation.
    if let Ok(settings) = crate::config::load_app_settings(app.handle()) {
        if let Some(ref ct) = settings.security.master_password {
            if let Ok(plain) = crate::utils::crypto::decrypt_settings_secret(ct) {
                crate::utils::crypto::set_master_password(Some(plain));
            }
        }
    }

    let mgr = session_manager.clone();
    tauri::async_runtime::spawn(async move {
        mgr.init_history_store().await;
    });

    if let Err(error) = quick_commands_store.load_from_disk(app.handle()) {
        tracing::warn!("Failed to load quick commands: {}", error);
    }

    let app_handle = app.handle().clone();
    let sync_manager = cloud_sync_manager.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = sync_manager.init(app_handle).await {
            tracing::warn!("Failed to initialize cloud sync manager: {}", error);
        }
    });

    let main_window = create_main_window_with_label(app, crate::window_state::MAIN_WINDOW_LABEL)?;
    if app
        .get_webview_window(crate::window_state::MAIN_WINDOW_LABEL)
        .is_none()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "main window was not created",
        )
        .into());
    }
    if main_window.label() != crate::window_state::MAIN_WINDOW_LABEL {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "main window was not created",
        )
        .into());
    }
    crate::tray::setup(app)?;

    Ok(())
}

pub fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = focused_or_first_main_window(app) {
        focus_window(&window);
        crate::tray::schedule_refresh(app);
    } else if let Err(error) = create_additional_main_window(app) {
        tracing::warn!("Failed to create main window from tray: {}", error);
    }
}

pub fn hide_main_window(app: &tauri::AppHandle) {
    for window in main_windows(app) {
        if let Err(error) = crate::window_state::save_main_webview_window_state(&window) {
            tracing::warn!("Failed to save main window state before hide: {}", error);
        }
        let _ = window.hide();
    }
    crate::tray::schedule_refresh(app);
}

pub fn prepare_app_shutdown(app: &tauri::AppHandle) {
    for window in main_windows(app) {
        if let Err(error) = crate::window_state::save_main_webview_window_state(&window) {
            tracing::warn!(
                "Failed to save main window state before shutdown: {}",
                error
            );
        }
    }

    let session_manager = app.state::<Arc<SessionManager>>();
    session_manager.flush_history_before_shutdown();

    for window in main_windows(app) {
        close_scoped_child_windows(app, window.label());
    }
}

pub fn quit_application(app: &tauri::AppHandle) {
    prepare_app_shutdown(app);
    app.exit(0);
}

pub fn on_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if crate::window_state::is_main_window_label(window.label()) {
        match event {
            tauri::WindowEvent::Resized(_) | tauri::WindowEvent::ScaleFactorChanged { .. } => {
                crate::window_state::schedule_main_window_state_save(
                    window.app_handle(),
                    window.label(),
                );
            }
            tauri::WindowEvent::CloseRequested { api, .. } => {
                if let Err(error) = crate::window_state::save_main_window_state(window) {
                    tracing::warn!("Failed to save main window state on close: {}", error);
                }

                close_scoped_child_windows(window.app_handle(), window.label());

                let remaining_main_windows = main_windows(window.app_handle())
                    .into_iter()
                    .filter(|main_window| main_window.label() != window.label())
                    .count();

                if remaining_main_windows > 0 {
                    crate::tray::schedule_refresh(window.app_handle());
                    return;
                }

                if let Ok(settings) = crate::config::load_app_settings(window.app_handle()) {
                    if settings.general.minimize_to_tray {
                        if let Some(webview_window) =
                            window.app_handle().get_webview_window(window.label())
                        {
                            let _ = webview_window.hide();
                        }
                        crate::tray::schedule_refresh(window.app_handle());
                        api.prevent_close();
                        return;
                    }
                }

                prepare_app_shutdown(window.app_handle());
            }
            _ => {}
        }
    }
}
