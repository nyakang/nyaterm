//! GPUI presentation crate for NyaTerm.

mod action_links;
mod ai_runtime;
mod i18n;
mod image_presentation;
mod send_command;
mod shortcuts;
mod temporary_ssh_link;
#[cfg(test)]
mod test_support;
mod thread_owner;
mod window_chrome;

pub mod app_shell;
pub mod blocking_jobs;
pub mod entities;
pub mod features;
pub mod http;
pub mod models;
pub mod platform;
pub mod terminal;
pub mod theme;
pub mod widgets;

use nyaterm_i18n::SharedCatalogBackend;

// Keep the existing t! call syntax, using one shared catalog through the public backend API.
rust_i18n::i18n!(
    "i18n-bridge",
    fallback = "en",
    backend = SharedCatalogBackend
);

/// Preload the shared catalog before any window renders.
pub fn preload_i18n() -> Result<(), String> {
    nyaterm_i18n::preload()
}

pub use app_shell::{
    AppShell, AppShellStartup, DesktopController, DesktopControllerGlobal, MainWindowPlacement,
};

pub fn init(cx: &mut gpui::App) {
    app_shell::init(cx);
    features::init(cx);
    // Shut down adb servers this process started (never servers that were
    // already running), so quitting does not leave a stray adb.exe behind.
    cx.on_app_quit(|_| async move {
        nyaterm_android::adb::client::shutdown_started_servers();
    })
    .detach();
}

/// Runs the detached portable updater helper before normal application startup.
pub fn run_update_helper_if_requested() -> bool {
    features::update::install::run_update_helper_if_requested()
}
