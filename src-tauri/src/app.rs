use std::sync::Arc;
use tauri::Manager;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::core::SessionManager;

pub fn init_tracing(log_dir: std::path::PathBuf) {
    let _ = std::fs::create_dir_all(&log_dir);

    let file_appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("dragonfly")
        .filename_suffix("log")
        .max_log_files(7)
        .build(&log_dir)
        .expect("failed to initialize rolling file appender");

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("dragonfly=info,user_action=debug,warn"));

    let local_time = fmt::time::OffsetTime::local_rfc_3339().unwrap_or_else(|_| {
        fmt::time::OffsetTime::new(
            time::UtcOffset::UTC,
            time::format_description::well_known::Rfc3339,
        )
    });

    tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_writer(file_appender)
                .with_ansi(false)
                .with_target(true)
                .with_timer(local_time.clone()),
        )
        .with(
            fmt::layer()
                .with_writer(std::io::stderr)
                .compact()
                .with_timer(local_time),
        )
        .init();

    tracing::info!("Dragonfly starting");
}

pub fn setup(
    app: &mut tauri::App,
    session_manager: Arc<SessionManager>,
) -> Result<(), Box<dyn std::error::Error>> {
    let home_dir = app
        .path()
        .home_dir()
        .map_err(|e: tauri::Error| e.to_string())?;

    let log_dir = app.path().app_log_dir().map_err(|e| e.to_string())?;
    init_tracing(log_dir);

    session_manager.set_app_handle(app.handle().clone());

    // Restore the master password for wrapping-key derivation.
    if let Ok(settings) = crate::config::load_app_settings(app.handle()) {
        if let Some(ref ct) = settings.security.master_password {
            if let Ok(plain) = crate::utils::crypto::decrypt_settings_secret(ct) {
                crate::utils::crypto::set_master_password(Some(plain));
            }
        }
    }

    let config_dir = home_dir.join(".dragonfly");
    let mgr = session_manager.clone();
    tauri::async_runtime::spawn(async move {
        mgr.init_history_store(config_dir).await;
    });

    let _tray = tauri::tray::TrayIconBuilder::new()
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("Dragonfly")
        .on_tray_icon_event(|tray, event| match event {
            tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } => {
                if let Some(window) = tray.app_handle().get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            _ => {}
        })
        .build(app)?;

    Ok(())
}

pub fn on_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        if window.label() == "main" {
            if let Ok(settings) = crate::config::load_app_settings(window.app_handle()) {
                if settings.general.minimize_to_tray {
                    let _ = window.hide();
                    api.prevent_close();
                    return;
                }
            }
            for label in &["settings", "new-session", "quick-command"] {
                if let Some(child) = window.app_handle().get_webview_window(label) {
                    let _ = child.close();
                }
            }
        }
    }
}
