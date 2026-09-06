//! Headless web-server mode.
//!
//! Exposes the existing Tauri command/event surface over HTTP + WebSocket so
//! the same frontend can run in a regular browser against a windowless
//! instance of the app. Desktop builds are unaffected: this module only
//! activates when the app is started with `NYATERM_WEB_SERVER=1` (or
//! `--server`), and everything that touches axum is behind the `server`
//! cargo feature.
//!
//! Zero existing logic is modified for this mode: the server runs inside a
//! real Tauri runtime (no windows except one hidden bridge webview), reuses
//! every managed [`tauri::State`] as-is, and calls the regular
//! `#[tauri::command]` functions directly. The hidden bridge window exists
//! only because a handful of commands take a `WebviewWindow` parameter (they
//! only read `window.label()`).

#[cfg(feature = "server")]
mod auth;
#[cfg(feature = "server")]
mod files;
#[cfg(feature = "server")]
mod rpc;
#[cfg(feature = "server")]
mod static_files;
#[cfg(feature = "server")]
mod ws;

/// Returns true when the app should boot in headless web-server mode.
pub fn is_server_mode() -> bool {
    if std::env::var("NYATERM_WEB_SERVER")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        return true;
    }
    std::env::args().any(|arg| arg == "--server")
}

#[cfg(feature = "server")]
pub(crate) const BRIDGE_WINDOW_LABEL: &str = "main";

/// Tiny static page loaded by the hidden bridge window (see `public/blank.html`).
#[cfg(feature = "server")]
pub(crate) const BRIDGE_WINDOW_URL: &str = "blank.html";

#[cfg(feature = "server")]
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub dist_dir: Option<std::path::PathBuf>,
}

#[cfg(feature = "server")]
impl ServerConfig {
    pub fn from_env() -> Self {
        let port = std::env::var("NYATERM_WEB_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8080);
        let dist_dir = std::env::var("NYATERM_WEB_DIST")
            .ok()
            .map(std::path::PathBuf::from);
        Self { port, dist_dir }
    }
}

/// Boots the HTTP/WebSocket layer next to the (windowless) Tauri runtime.
///
/// Called from `crate::app::setup` after storage/managers initialization and
/// instead of main-window/tray creation.
#[cfg(feature = "server")]
pub fn start(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::Manager;

    let config = ServerConfig::from_env();
    let app_handle = app.handle().clone();

    // Hidden webview so every `#[tauri::command]` that takes a
    // `WebviewWindow` (they only read `window.label()`) stays callable.
    if app.get_webview_window(BRIDGE_WINDOW_LABEL).is_none() {
        tauri::WebviewWindowBuilder::new(
            app,
            BRIDGE_WINDOW_LABEL,
            tauri::WebviewUrl::App(BRIDGE_WINDOW_URL.into()),
        )
        .visible(false)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .closable(false)
        .skip_taskbar(true)
        .build()?;
    }

    let config_dir = {
        let runtime = app.state::<crate::runtime::AppRuntime>();
        runtime.config_dir().to_path_buf()
    };
    let token = auth::AuthToken::load_or_create(&config_dir)?;

    let event_tx = ws::start_event_bridge(&app_handle);
    let port = config.port;

    let download_dir = config_dir.join("web-downloads");
    let staging_dir = config_dir.join("web-staging");
    std::fs::create_dir_all(&download_dir)?;
    std::fs::create_dir_all(&staging_dir)?;
    files::cleanup_old_files(&download_dir);
    files::cleanup_old_files(&staging_dir);

    let state = rpc::ServerState {
        app: app_handle.clone(),
        auth: std::sync::Arc::new(token),
        registry: rpc::build_registry(),
        events: event_tx,
        port,
        download_dir,
        staging_dir,
        dist_dir: config.dist_dir.clone(),
    };

    let port = config.port;
    let quiet = std::env::var("NYATERM_WEB_QUIET").ok().as_deref() == Some("1");
    if !quiet {
        // Server operators need this on stdout; normal tracing goes to the log file.
        println!(
            "NyaTerm web server listening on http://0.0.0.0:{port} (data dir: {})",
            config_dir.display()
        );
    }
    tracing::info!(port, "NyaTerm web server starting");

    tauri::async_runtime::spawn(async move {
        if let Err(error) = rpc::serve(state).await {
            tracing::error!("NyaTerm web server failed: {error}");
        }
    });

    Ok(())
}
