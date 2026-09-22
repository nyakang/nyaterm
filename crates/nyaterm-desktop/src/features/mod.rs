mod activation;
mod ai;
mod app_state;
mod assets;
mod commands;
mod connections;
mod font_catalog;
mod formatting;
mod icons;
mod inspector;
mod layout;
mod mcp;
mod notes;
mod pages;
mod panels;
mod perf;
mod recording;
mod remote;
mod remote_desktop;
mod root;
mod runtime_jobs;
mod selects;
mod session;
mod settings;
mod shell;
mod sync;
mod sync_input;
mod tab_transfer;
mod terminal;
#[cfg(test)]
mod test_support;
mod text_inputs;
mod transfers;
mod translation;
mod tunnels;
mod update;
mod view_widgets;

pub(crate) fn init(cx: &mut gpui::App) {
    // gpui-kit has already installed its bindings. Capture that immutable
    // baseline before adding NyaTerm's rebuildable bindings and protections.
    crate::shortcuts::init(cx);
    init_protection_key_bindings(cx);
}

pub(crate) fn init_protection_key_bindings(cx: &mut gpui::App) {
    terminal::init_key_bindings(cx);
    view_widgets::init_child_window_key_bindings(cx);
}

pub(crate) use app_state::AppLifecycleEvent;
pub use app_state::NyaTermApp;
pub(crate) use app_state::NyaTermStoreClients;
pub(crate) use app_state::WorkspaceCloseSnapshot;
pub(in crate::features) use font_catalog::{
    FontAvailability, FontAvailabilityReason, FontCatalogEntry, FontCatalogKind,
    FontCatalogLoadState, FontCatalogPresentation, FontCatalogSnapshot, FontCatalogState,
    FontResolutionSource, FontResolutionStatus, font_names_fingerprint, normalize_font_family,
};
pub(crate) use shell::tray::{SystemTray, TraySnapshot, show_window as show_tray_window};
