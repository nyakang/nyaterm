use gpui::{Context, IntoElement, div, prelude::*, px};

use self::browser::transfer_browser_view;
use self::panel::TransferPanel;
use self::queue::transfer_queue_view;
use self::resize::transfer_height_resize_handle;

mod browser;
mod browser_columns;
mod browser_filter;
mod browser_keys;
mod browser_navigation;
mod browser_selection;
mod context_menu_policy;
mod duplicate_banner;
mod editor;
mod entry_row;
mod file_ops;
mod flush;
mod forms;
mod helpers;
mod overlays;
mod overlays_context;
mod overlays_delete_move;
mod overlays_editor;
mod overlays_favorites;
mod overlays_unknown;
mod overlays_upload;
pub(in crate::features) mod panel;
pub(in crate::features::pages::transfers) mod path_bar;
mod preview;
mod properties;
mod properties_dialog;
mod queue;
mod resize;
#[cfg(test)]
pub(in crate::features::pages::transfers) mod tests_support;

use entry_row::{
    TransferBrowserEntryRowPresentation, transfer_browser_entry_row,
    transfer_browser_parent_entry_row,
};
use helpers::*;

const NATIVE_EDITOR_MAX_BYTES: u64 = 512 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::features) enum TransferBrowserAvailability {
    NoSession,
    UnsupportedSession,
    DisconnectedSession,
    Browsable,
}

fn transfer_browser_availability(
    has_active_session: bool,
    has_browser_backend: bool,
    is_disconnected: bool,
) -> TransferBrowserAvailability {
    if !has_active_session {
        TransferBrowserAvailability::NoSession
    } else if !has_browser_backend {
        TransferBrowserAvailability::UnsupportedSession
    } else if is_disconnected {
        TransferBrowserAvailability::DisconnectedSession
    } else {
        TransferBrowserAvailability::Browsable
    }
}

fn transfer_dialog_width(viewport_width: f32, preferred_width: f32) -> f32 {
    preferred_width.min((viewport_width - 32.).max(240.))
}

fn transfer_menu_position(
    x: f32,
    y: f32,
    menu_width: f32,
    preferred_height: f32,
    viewport_width: f32,
    viewport_height: f32,
) -> (f32, f32, f32) {
    let margin = 8.;
    let max_height = (viewport_height - margin * 2.).max(80.);
    let height = preferred_height.min(max_height);
    let max_x = (viewport_width - menu_width - margin).max(margin);
    let max_y = (viewport_height - height - margin).max(margin);
    (x.clamp(margin, max_x), y.clamp(margin, max_y), max_height)
}

/// The transfers panel body.
///
/// Takes no `NyaTermApp`. GPUI records every entity read during a draw, so an app
/// read here -- even a diagnostic one -- would put the panel back on the app's
/// invalidation path and undo the isolation. Everything it draws comes from the
/// snapshot; everything it changes goes back through `TransferPanel::with_app`.
pub(in crate::features::pages::transfers) fn transfer_panel(
    panel: &mut TransferPanel,
    window: &mut gpui::Window,
    cx: &mut Context<TransferPanel>,
) -> gpui::AnyElement {
    // Before the first flush there is nothing to draw and nothing to draw it from.
    let Some(snapshot) = panel.snapshot() else {
        return div().into_any_element();
    };
    let chrome = snapshot.chrome;
    let transfer_height = snapshot.panel_height;

    // Tauri AppPanelContent: FileExplorer (flex-1) + vertical resize + FileTransfer fixed height.
    div()
        .size_full()
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(chrome.transparent_surface)
        .child(
            div()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .child(transfer_browser_view(panel, window, cx)),
        )
        .child(transfer_height_resize_handle(
            chrome,
            snapshot.height_is_resizing,
            snapshot.resize_handle_highlighted,
            cx,
        ))
        .child(
            div()
                .h(px(transfer_height))
                .flex_none()
                .overflow_hidden()
                .child(transfer_queue_view(panel, cx)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{
        TransferBrowserAvailability, transfer_browser_availability, transfer_dialog_width,
        transfer_menu_position,
    };

    #[test]
    fn browser_availability_distinguishes_session_states() {
        assert_eq!(
            transfer_browser_availability(false, false, false),
            TransferBrowserAvailability::NoSession
        );
        assert_eq!(
            transfer_browser_availability(true, false, false),
            TransferBrowserAvailability::UnsupportedSession
        );
        assert_eq!(
            transfer_browser_availability(true, true, true),
            TransferBrowserAvailability::DisconnectedSession
        );
        assert_eq!(
            transfer_browser_availability(true, true, false),
            TransferBrowserAvailability::Browsable
        );
    }

    #[test]
    fn dialog_width_uses_preferred_size_with_narrow_viewport_fallback() {
        assert_eq!(transfer_dialog_width(1280., 500.), 500.);
        assert_eq!(transfer_dialog_width(420., 500.), 388.);
        assert_eq!(transfer_dialog_width(200., 500.), 240.);
    }

    #[test]
    fn menu_position_stays_inside_viewport() {
        assert_eq!(
            transfer_menu_position(1200., 760., 268., 360., 1280., 800.),
            (1004., 432., 784.)
        );
        assert_eq!(
            transfer_menu_position(500., 500., 268., 560., 300., 240.),
            (24., 8., 224.)
        );
    }
}
