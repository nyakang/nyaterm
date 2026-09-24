//! GPUI presentation crate for NyaTerm.

mod action_links;
mod external_process;
mod i18n;
mod send_command;
mod shortcuts;
mod temporary_ssh_link;
#[cfg(test)]
mod test_support;
mod thread_owner;

pub mod app_shell;
pub mod blocking_jobs;
pub mod entities;
pub mod features;
pub mod http;
pub mod models;
pub mod terminal;
pub mod theme;
pub mod widgets;

// Generates `crate::_rust_i18n_t!`, which `rust_i18n::t!` forwards to, so this must
// stay in the crate root. `locales/` is resolved against CARGO_MANIFEST_DIR at macro
// expansion time; `build.rs` is what makes cargo notice edits to it.
rust_i18n::i18n!("locales", fallback = "en");

const I18N_PRELOAD_STACK_BYTES: usize = 16 * 1024 * 1024;
static I18N_PRELOAD_RESULT: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();

/// Build rust-i18n's generated backend before the first UI render.
///
/// rust-i18n 4.2 expands every catalog entry into one lazy initializer. With
/// NyaTerm's full multilingual catalog that initializer exceeds the default
/// Windows main-thread stack. A short-lived worker gives that one-time build a
/// bounded stack without moving GPUI off the platform main thread or increasing
/// the executable's stack reservation permanently.
pub fn preload_i18n() -> Result<(), String> {
    I18N_PRELOAD_RESULT
        .get_or_init(|| {
            std::thread::Builder::new()
                .name("nyaterm-i18n-preload".to_string())
                .stack_size(I18N_PRELOAD_STACK_BYTES)
                .spawn(|| {
                    let _ = _rust_i18n_backend();
                })
                .map_err(|error| format!("failed to spawn i18n preload worker: {error}"))?
                .join()
                .map_err(|_| "i18n preload worker panicked".to_string())
        })
        .clone()
}

pub use app_shell::{
    AppShell, AppShellStartup, DesktopController, DesktopControllerGlobal, MainWindowPlacement,
};

pub fn init(cx: &mut gpui::App) {
    app_shell::init(cx);
    features::init(cx);
}

/// Runs the detached portable updater helper before normal application startup.
pub fn run_update_helper_if_requested() -> bool {
    features::update::install::run_update_helper_if_requested()
}
