use crate::features::NyaTermApp;
use crate::features::remote_desktop::runtime::input::{display_transform, logical_point};
use crate::models::SessionLaunchConfig;
use nyaterm_remote_desktop::spice::protocol::{SpiceControl, SpiceInput, SpiceMouseMode};
use nyaterm_remote_desktop::{
    DisplayScaleMode, RdpInputEvent, RemoteDesktopViewState, RemotePoint, RemotePointerButton,
    RemoteWheelAxis,
};

impl NyaTermApp {
    pub(in crate::features::remote_desktop) fn is_spice_session(&self, id: &str) -> bool {
        self.session
            .metadata(id)
            .is_some_and(|m| matches!(m.launch_config, SessionLaunchConfig::Spice(_)))
    }

    pub(in crate::features::remote_desktop) fn spice_input_enabled(&self, id: &str) -> bool {
        self.session.metadata(id).is_some_and(|m| matches!(&m.launch_config, SessionLaunchConfig::Spice(c) if !c.session.settings.view_only))
            && self.remote_desktop.sessions.get(id).is_some_and(|s| s.state == RemoteDesktopViewState::Connected)
    }

    pub(in crate::features::remote_desktop) fn send_remote_scancodes(
        &self,
        id: &str,
        keys: Vec<RdpInputEvent>,
    ) -> bool {
        if !self.is_spice_session(id) {
            return self.remote_desktop.manager.send_input(id, keys).is_ok();
        }
        if !self.spice_input_enabled(id) {
            return false;
        }
        let events: Vec<_> = keys.into_iter().filter_map(spice_key).collect();
        !events.is_empty()
            && self
                .remote_desktop
                .spice_manager
                .send(id, SpiceControl::Input { events })
                .is_ok()
    }

    pub(in crate::features::remote_desktop) fn send_spice_pointer(
        &mut self,
        id: &str,
        position: gpui::Point<gpui::Pixels>,
        button: Option<RemotePointerButton>,
        pressed: bool,
    ) -> bool {
        if !self.spice_input_enabled(id) {
            return false;
        }
        let fit = self.session.metadata(id).is_some_and(|m| matches!(&m.launch_config, SessionLaunchConfig::Spice(c) if c.session.settings.fit_window));
        if self.spice_relative_input(id) {
            return self.spice_pointer_owned(id)
                && button.is_some_and(|button| {
                    self.remote_desktop
                        .spice_manager
                        .send(
                            id,
                            SpiceControl::Input {
                                events: vec![SpiceInput::Button { button, pressed }],
                            },
                        )
                        .is_ok()
                });
        }
        let Some(session) = self.remote_desktop.sessions.get_mut(id) else {
            return false;
        };
        let Some(spice) = session.spice.as_mut() else {
            return false;
        };
        let Some(display) = spice.displayed.as_ref() else {
            return false;
        };
        let Some(viewport) = session.viewport else {
            return false;
        };
        let transform = display_transform(
            viewport,
            display.width,
            display.height,
            if fit {
                DisplayScaleMode::Fit
            } else {
                DisplayScaleMode::Actual
            },
        );
        let remote = transform
            .and_then(|t| t.window_to_remote(logical_point(position)))
            .or_else(|| {
                (button.is_some() && !pressed)
                    .then_some(session.last_pointer)
                    .flatten()
            });
        let Some(remote) = remote else {
            return false;
        };
        let mut events = Vec::with_capacity(2);
        match spice.capabilities.mouse_mode {
            SpiceMouseMode::Absolute => events.push(SpiceInput::Absolute {
                display: display.id,
                position: RemotePoint {
                    x: remote.0,
                    y: remote.1,
                },
            }),
            SpiceMouseMode::Relative => return false,
        }
        session.last_pointer = Some(remote);
        if let Some(button) = button {
            events.push(SpiceInput::Button { button, pressed });
        }
        let sent = !events.is_empty()
            && self
                .remote_desktop
                .spice_manager
                .send(id, SpiceControl::Input { events })
                .is_ok();
        if sent && spice.capabilities.mouse_mode == SpiceMouseMode::Absolute {
            session.cursor_position.x = remote.0;
            session.cursor_position.y = remote.1;
        }
        sent
    }

    pub(in crate::features::remote_desktop) fn send_spice_wheel(
        &mut self,
        id: &str,
        x: f32,
        y: f32,
    ) -> bool {
        if !self.spice_input_enabled(id) || !x.is_finite() || !y.is_finite() {
            return false;
        }
        if self.spice_relative_input(id) && !self.spice_pointer_owned(id) {
            return false;
        }
        let Some(session) = self.remote_desktop.sessions.get_mut(id) else {
            return false;
        };
        let mut events = Vec::new();
        for (axis, delta, remainder) in [
            (
                RemoteWheelAxis::Horizontal,
                x,
                &mut session.wheel_remainder_x,
            ),
            (RemoteWheelAxis::Vertical, y, &mut session.wheel_remainder_y),
        ] {
            *remainder += delta.clamp(-120., 120.);
            let steps = remainder.trunc().clamp(-120., 120.) as i16;
            *remainder -= f32::from(steps);
            if steps != 0 {
                events.push(SpiceInput::Wheel { axis, steps });
            }
        }
        !events.is_empty()
            && self
                .remote_desktop
                .spice_manager
                .send(id, SpiceControl::Input { events })
                .is_ok()
    }
}

fn spice_key(key: RdpInputEvent) -> Option<SpiceInput> {
    match key {
        RdpInputEvent::KeyDown {
            scan_code,
            extended,
            ..
        } => Some(SpiceInput::Key {
            scan_code: scan_code | if extended { 0x100 } else { 0 },
            pressed: true,
        }),
        RdpInputEvent::KeyUp {
            scan_code,
            extended,
            ..
        } => Some(SpiceInput::Key {
            scan_code: scan_code | if extended { 0x100 } else { 0 },
            pressed: false,
        }),
        RdpInputEvent::ReleaseAllInputs => Some(SpiceInput::ReleaseAll),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::spice_key;
    use nyaterm_remote_desktop::RdpInputEvent;
    use nyaterm_remote_desktop::spice::protocol::SpiceInput;

    #[test]
    fn extended_physical_keys_use_spice_scan_code_prefix_on_both_edges() {
        assert_eq!(
            spice_key(RdpInputEvent::KeyDown {
                scan_code: 0x1d,
                extended: true,
                repeat: false
            }),
            Some(SpiceInput::Key {
                scan_code: 0x11d,
                pressed: true
            })
        );
        assert_eq!(
            spice_key(RdpInputEvent::KeyUp {
                scan_code: 0x1d,
                extended: true,
                repeat: false
            }),
            Some(SpiceInput::Key {
                scan_code: 0x11d,
                pressed: false
            })
        );
    }
}
