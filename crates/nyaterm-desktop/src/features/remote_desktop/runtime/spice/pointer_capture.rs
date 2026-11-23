use crate::features::NyaTermApp;
use crate::features::remote_desktop::state::spice::SpicePointerCapture;
use gpui::{
    AnyElement, Context, IntoElement, Keystroke, PointerLockEvent, Window, canvas, prelude::Styled,
};
use nyaterm_remote_desktop::spice::protocol::{SpiceControl, SpiceInput, SpiceMouseMode};
use rust_i18n::t;

#[cfg(test)]
mod tests;

impl NyaTermApp {
    pub(in crate::features::remote_desktop) fn spice_relative_input(&self, id: &str) -> bool {
        self.spice_input_enabled(id)
            && self
                .remote_desktop
                .sessions
                .get(id)
                .and_then(|session| session.spice.as_ref())
                .is_some_and(|spice| spice.capabilities.mouse_mode == SpiceMouseMode::Relative)
    }

    pub(in crate::features::remote_desktop) fn spice_pointer_owned(&self, id: &str) -> bool {
        self.remote_desktop
            .spice_pointer_capture
            .as_ref()
            .is_some_and(|capture| capture.session_id == id && !capture.release_requested)
    }

    pub(in crate::features::remote_desktop) fn capture_spice_pointer(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.spice_relative_input(id) || self.spice_pointer_owned(id) {
            return;
        }
        self.release_spice_pointer(window);
        if !window.is_window_active()
            || !self.remote_desktop.focus.is_focused(window)
            || self.root_overlay_visibility(cx).obscures_native_surface()
        {
            return;
        }
        match window.set_pointer_lock(true) {
            Ok(()) => {
                self.remote_desktop.spice_pointer_capture =
                    Some(SpicePointerCapture::new(id.to_owned()));
                self.sync_rdp_keyboard_capture(window);
            }
            Err(_) => self
                .shell
                .set_status(t!("spice.pointerCaptureUnavailable").to_string()),
        }
        cx.notify();
    }

    pub(in crate::features) fn release_spice_pointer(&mut self, window: &mut Window) {
        if let Some(capture) = self.remote_desktop.spice_pointer_capture.take() {
            if window.set_pointer_lock(false).is_err() {
                self.shell
                    .set_status(t!("spice.pointerReleaseFailed").to_string());
            }
            self.release_remote_keys(&capture.session_id);
            super::super::super::keyboard_capture::set_keyboard_capture(
                self.remote_desktop.manager.clone(),
                self.remote_desktop.vnc_manager.clone(),
                self.remote_desktop.spice_manager.clone(),
                None,
            );
        }
    }

    pub(in crate::features::remote_desktop) fn release_spice_pointer_shortcut(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let modifiers = keystroke.modifiers;
        // A modifier-only Ctrl+Alt chord also matches AltGr on some host layouts.
        if self.remote_desktop.spice_pointer_capture.is_some()
            && keystroke.key == "f12"
            && modifiers.shift
            && !modifiers.control
            && !modifiers.alt
            && !modifiers.platform
        {
            self.release_spice_pointer(window);
            cx.notify();
            cx.stop_propagation();
            true
        } else {
            false
        }
    }

    pub(in crate::features) fn reconcile_spice_pointer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.remote_desktop.spice_pointer_capture.is_none() {
            return;
        }
        let obscured = self.root_overlay_visibility(cx).obscures_native_surface();
        let release = self
            .remote_desktop
            .spice_pointer_capture
            .as_ref()
            .is_some_and(|capture| {
                capture.release_requested
                    || !self.spice_relative_input(&capture.session_id)
                    || self.session.active_id() != Some(capture.session_id.as_str())
                    || !window.is_window_active()
                    || !window.is_visible()
                    || !self.remote_desktop.focus.is_focused(window)
                    || obscured
            });
        if release {
            self.release_spice_pointer(window);
        }
    }

    pub(in crate::features) fn spice_pointer_listener_layer(
        &self,
        cx: &Context<Self>,
    ) -> AnyElement {
        let app = cx.weak_entity();
        canvas(
            |_, _, _| (),
            move |_, (), window, _cx| {
                let app = app.clone();
                window.on_mouse_event(move |event: &PointerLockEvent, phase, window, cx| {
                    if !phase.capture() {
                        return;
                    }
                    let _ = app.update(cx, |app, cx| {
                        app.reconcile_spice_pointer(window, cx);
                        match event {
                            PointerLockEvent::Unlocked => {
                                app.release_spice_pointer(window);
                                cx.notify();
                            }
                            PointerLockEvent::Locked => {
                                // A compositor may grant a request after the surface lost ownership.
                                if app.remote_desktop.spice_pointer_capture.is_none()
                                    && window.set_pointer_lock(false).is_err()
                                {
                                    app.shell
                                        .set_status(t!("spice.pointerReleaseFailed").to_string());
                                }
                                cx.notify();
                            }
                            PointerLockEvent::Motion { delta } => {
                                if !window.is_pointer_locked() {
                                    return;
                                }
                                let Some(capture) =
                                    app.remote_desktop.spice_pointer_capture.as_mut()
                                else {
                                    return;
                                };
                                if let Some((dx, dy)) = capture.motion(*delta) {
                                    if app
                                        .remote_desktop
                                        .spice_manager
                                        .send(
                                            &capture.session_id,
                                            SpiceControl::Input {
                                                events: vec![SpiceInput::Relative { dx, dy }],
                                            },
                                        )
                                        .is_err()
                                    {
                                        app.release_spice_pointer(window);
                                        cx.notify();
                                    } else {
                                        app.mark_user_activity();
                                    }
                                }
                                cx.stop_propagation();
                            }
                        }
                    });
                });
            },
        )
        .absolute()
        .inset_0()
        .size_full()
        .into_any_element()
    }
}
