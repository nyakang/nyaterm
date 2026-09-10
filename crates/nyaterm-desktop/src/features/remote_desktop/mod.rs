mod input;
mod keyboard_capture;
mod runtime;
mod state;
mod view;

use super::NyaTermApp;

impl NyaTermApp {
    pub(crate) fn shutdown_remote_desktop_workers(&mut self) {
        keyboard_capture::shutdown_keyboard_capture();
    }
}

pub(in crate::features) use state::RemoteDesktopFeatureState;
