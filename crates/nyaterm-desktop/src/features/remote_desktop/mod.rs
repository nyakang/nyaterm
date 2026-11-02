mod input;
mod keyboard_capture;
#[cfg(windows)]
mod native;
mod runtime;
mod state;
mod surface;
mod view;

use gpui::{App, KeyBinding, actions};

use super::NyaTermApp;

pub(super) const REMOTE_DESKTOP_KEY_CONTEXT: &str = "RemoteDesktop";

actions!(remote_desktop, [RemoteDesktopTab, RemoteDesktopShiftTab]);

pub(in crate::features) fn init_key_bindings(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", RemoteDesktopTab, Some(REMOTE_DESKTOP_KEY_CONTEXT)),
        KeyBinding::new(
            "shift-tab",
            RemoteDesktopShiftTab,
            Some(REMOTE_DESKTOP_KEY_CONTEXT),
        ),
    ]);
}

impl NyaTermApp {
    pub(crate) fn shutdown_remote_desktop_workers(&mut self) {
        keyboard_capture::shutdown_keyboard_capture();
    }
}

pub(in crate::features) use state::RemoteDesktopFeatureState;
