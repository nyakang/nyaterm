mod activation;
mod ai;
mod app_state;
mod assets;
mod capability_runtime;
mod commands;
mod connections;
mod file_manager;
mod font_catalog;
mod formatting;
mod icons;
mod inspector;
pub(crate) mod layout;
mod mcp;
mod notes;
mod pages;
mod panels;
mod perf;
pub(crate) mod plugins;
mod recording;
mod remote;
mod remote_desktop;
mod root;
mod runtime_jobs;
mod selects;
mod session;
mod settings;
pub(crate) mod shell;
pub(crate) mod sync;

pub(crate) fn cloud_sync_operation_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    &LOCK
}
mod sync_input;

pub(crate) use sync::{AutoSyncResult, AutoSyncTrigger, run_auto_sync};
mod tab_transfer;
mod terminal;
#[cfg(test)]
pub(crate) mod test_support;
mod text_inputs;
mod tmux;
mod transfers;
mod translation;
mod tunnels;
pub(crate) mod update;
mod view_widgets;

pub(crate) fn init(cx: &mut gpui::App) {
    // gpui-kit has already installed its bindings. Capture that immutable
    // baseline before adding NyaTerm's rebuildable bindings and protections.
    crate::shortcuts::init(cx);
    init_protection_key_bindings(cx);
}

pub(crate) fn init_protection_key_bindings(cx: &mut gpui::App) {
    terminal::init_key_bindings(cx);
    remote_desktop::init_key_bindings(cx);
    view_widgets::init_child_window_key_bindings(cx);
}

pub(crate) use app_state::AppLifecycleEvent;
pub use app_state::NyaTermApp;
pub(crate) use app_state::NyaTermProcessEntities;
pub(crate) use app_state::NyaTermStoreClients;
pub(crate) use app_state::WorkspaceCloseSnapshot;
pub(in crate::features) use font_catalog::{
    FontAvailability, FontAvailabilityReason, FontCatalogEntry, FontCatalogKind,
    FontCatalogLoadState, FontCatalogPresentation, FontCatalogSnapshot, FontCatalogState,
    FontResolutionSource, FontResolutionStatus, font_names_fingerprint, normalize_font_family,
};
