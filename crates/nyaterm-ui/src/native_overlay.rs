//! Presentation scopes share the owner's layout, state and focus tree.
use gpui::{IntoElement, NativeOverlay, NativeOverlayInput, Window, native_overlay};

pub fn enable_native_child_overlays(window: &mut Window) -> Result<(), String> {
    window
        .set_native_child_surface_mode(true)
        .map_err(|error| error.to_string())
}

pub fn nya_region_overlay(content: impl IntoElement) -> NativeOverlay {
    native_overlay(content, NativeOverlayInput::Region)
}

pub fn nya_passive_overlay(content: impl IntoElement) -> NativeOverlay {
    native_overlay(content, NativeOverlayInput::Passive)
}

pub fn nya_modal_overlay(content: impl IntoElement) -> NativeOverlay {
    native_overlay(content, NativeOverlayInput::Modal)
}
