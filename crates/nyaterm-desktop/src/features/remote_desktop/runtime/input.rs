use super::errors::format_rdp_error;
use crate::features::NyaTermApp;
use gpui::Bounds;
use gpui::Context;
use gpui::Modifiers;
use gpui::Window;
use nyaterm_remote_desktop::DisplayScaleMode;
use nyaterm_remote_desktop::DisplayTransform;
use nyaterm_remote_desktop::LogicalPoint;
use nyaterm_remote_desktop::LogicalRect;
use nyaterm_remote_desktop::LogicalSize;
use nyaterm_remote_desktop::RdpInputEvent;
use nyaterm_remote_desktop::RdpServerCapabilities;
use nyaterm_remote_desktop::RemoteDesktopViewState;
use nyaterm_remote_desktop::RemotePoint;
use nyaterm_remote_desktop::RemotePointerButton;
use nyaterm_remote_desktop::RemotePointerEvent;
use nyaterm_remote_desktop::RemoteWheelAxis;
use nyaterm_remote_desktop::VncInputEvent;
use nyaterm_remote_desktop::VncScaleMode;
use nyaterm_remote_desktop::VncServerCapabilities;
use std::time::Duration;
use std::time::Instant;

pub(super) const POINTER_MOVE_INTERVAL: Duration = Duration::from_millis(8);

const HARDWARE_CURSOR_REPAINT_INTERVAL: Duration = Duration::from_millis(8);
const SOFTWARE_CURSOR_REPAINT_INTERVAL: Duration = Duration::from_millis(33);

pub(super) fn remote_cursor_repaint_interval(software_renderer: bool) -> Duration {
    if software_renderer {
        SOFTWARE_CURSOR_REPAINT_INTERVAL
    } else {
        HARDWARE_CURSOR_REPAINT_INTERVAL
    }
}

impl NyaTermApp {
    pub(in crate::features) fn clear_remote_composition(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(input) = self.remote_desktop.inputs.get(session_id) {
            input.update(cx, |input, cx| input.clear(cx));
        }
    }

    pub(in crate::features) fn ensure_rdp_focus_reporting(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.remote_desktop.focus_subscriptions.is_empty() {
            return;
        }
        let focus_in = cx.on_focus_in(&self.remote_desktop.focus, window, |this, window, _cx| {
            if let Some(session_id) = this.session.active_id_owned()
                && this.remote_desktop.is_session(&session_id)
            {
                #[cfg(windows)]
                if let Some(native) = this
                    .remote_desktop
                    .sessions
                    .get(&session_id)
                    .and_then(|session| session.native_rdp.as_ref())
                {
                    native.focus();
                    return;
                }
                let _ = this.send_remote_modifier_state(
                    &session_id,
                    window.modifiers(),
                    Some(window.capslock().on),
                );
                this.sync_rdp_keyboard_capture(window);
            }
        });
        let focus_out = cx.on_focus_out(
            &self.remote_desktop.focus,
            window,
            |this, _event, _window, _cx| {
                crate::features::remote_desktop::keyboard_capture::set_keyboard_capture(
                    this.remote_desktop.manager.clone(),
                    this.remote_desktop.vnc_manager.clone(),
                    None,
                );
                if let Some(session_id) = this.session.active_id_owned() {
                    this.release_remote_keys(&session_id);
                }
            },
        );
        self.remote_desktop.focus_subscriptions = vec![focus_in, focus_out];
    }

    pub(in crate::features) fn release_remote_keys(&mut self, session_id: &str) {
        if self.session.metadata(session_id).is_some_and(|metadata| {
            matches!(
                metadata.launch_config,
                crate::models::SessionLaunchConfig::Vnc(_)
            )
        }) {
            if self.vnc_input_enabled(session_id) {
                let _ = self
                    .remote_desktop
                    .vnc_manager
                    .send_input(session_id, vec![VncInputEvent::ReleaseAllInputs]);
            }
            if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
                session.modifiers = Default::default();
                session.last_pointer = None;
                session.wheel_remainder_x = 0.0;
                session.wheel_remainder_y = 0.0;
            }
            return;
        }
        let Some(session) = self.remote_desktop.sessions.get_mut(session_id) else {
            return;
        };
        let _ = session.keys.release_all();
        session.modifiers = Default::default();
        session.last_pointer = None;
        session.wheel_remainder_x = 0.0;
        session.wheel_remainder_y = 0.0;
        let _ = self
            .remote_desktop
            .manager
            .send_input(session_id, vec![RdpInputEvent::ReleaseAllInputs]);
    }

    pub(super) fn sync_rdp_keyboard_capture(&self, window: &Window) {
        let target = self.session.active_id().and_then(|session_id| {
            let is_vnc = self.session.metadata(session_id).is_some_and(|metadata| {
                matches!(
                    metadata.launch_config,
                    crate::models::SessionLaunchConfig::Vnc(_)
                )
            });
            (window.is_window_active()
                && self.remote_desktop.focus.is_focused(window)
                && self
                    .remote_desktop
                    .sessions
                    .get(session_id)
                    .is_some_and(|session| {
                        // The Microsoft control owns keyboard input; the global
                        // capture hook would swallow those keys and send them to a
                        // helper session that does not exist.
                        #[cfg(windows)]
                        if session.native_rdp.is_some() {
                            return false;
                        }
                        matches!(session.state, RemoteDesktopViewState::Connected)
                            && (is_vnc
                                || session.viewport.is_some_and(|bounds| {
                                    bounds.contains(&window.mouse_position())
                                }))
                    }))
            .then(|| (session_id.to_string(), is_vnc))
        });
        crate::features::remote_desktop::keyboard_capture::set_keyboard_capture(
            self.remote_desktop.manager.clone(),
            self.remote_desktop.vnc_manager.clone(),
            target,
        );
    }

    pub(in crate::features) fn send_remote_committed_text(
        &mut self,
        session_id: &str,
        text: &str,
    ) -> bool {
        if text.is_empty() {
            return true;
        }
        if !self
            .remote_desktop
            .sessions
            .get(session_id)
            .is_some_and(|session| matches!(session.state, RemoteDesktopViewState::Connected))
        {
            self.shell.set_status(
                "Remote desktop text input is unavailable while disconnected".to_string(),
            );
            return false;
        }
        let is_vnc = self.session.metadata(session_id).is_some_and(|metadata| {
            matches!(
                metadata.launch_config,
                crate::models::SessionLaunchConfig::Vnc(_)
            )
        });
        let committed_text_supported =
            self.remote_desktop
                .sessions
                .get(session_id)
                .is_some_and(|session| {
                    remote_committed_text_supported(
                        is_vnc,
                        session.server_capabilities,
                        session.vnc_server_capabilities,
                    )
                });
        if !committed_text_supported {
            return false;
        }
        let result = if is_vnc {
            if !self.vnc_input_enabled(session_id) {
                self.shell
                    .set_status("VNC view-only mode does not accept input".to_string());
                return false;
            }
            self.remote_desktop
                .vnc_manager
                .send_input(
                    session_id,
                    vec![VncInputEvent::Text {
                        text: text.to_string(),
                    }],
                )
                .map_err(|error| error.to_string())
        } else {
            self.remote_desktop
                .manager
                .send_input(
                    session_id,
                    vec![RdpInputEvent::Unicode {
                        text: text.to_string(),
                    }],
                )
                .map_err(|error| format_rdp_error(&error))
        };
        match result {
            Ok(()) => true,
            Err(error) => {
                self.shell.set_status(error);
                false
            }
        }
    }

    pub(in crate::features) fn send_rdp_key_down(
        &mut self,
        session_id: &str,
        key: &str,
        key_char: Option<&str>,
        repeat: bool,
        modifiers: Modifiers,
    ) -> bool {
        if !self
            .remote_desktop
            .sessions
            .get(session_id)
            .is_some_and(|session| matches!(session.state, RemoteDesktopViewState::Connected))
        {
            return false;
        }
        let is_vnc = self.session.metadata(session_id).is_some_and(|metadata| {
            matches!(
                metadata.launch_config,
                crate::models::SessionLaunchConfig::Vnc(_)
            )
        });
        let modifiers = if is_vnc {
            modifiers
        } else {
            rdp_key_modifiers(
                key,
                modifiers,
                self.remote_desktop
                    .sessions
                    .get(session_id)
                    .is_some_and(|session| session.modifiers.modifiers.shift),
            )
        };
        if !self.send_remote_modifier_state(session_id, modifiers, None) {
            return false;
        }
        if is_vnc {
            return self.send_vnc_key(session_id, key, key_char, true);
        }
        let Some(event) = self
            .remote_desktop
            .sessions
            .get_mut(session_id)
            .and_then(|session| session.keys.key_down(key, repeat))
        else {
            return false;
        };
        self.remote_desktop
            .manager
            .send_input(session_id, vec![event])
            .is_ok()
    }

    pub(in crate::features) fn send_rdp_key_up(&mut self, session_id: &str, key: &str) -> bool {
        if self.session.metadata(session_id).is_some_and(|metadata| {
            matches!(
                metadata.launch_config,
                crate::models::SessionLaunchConfig::Vnc(_)
            )
        }) {
            return self.send_vnc_key(session_id, key, None, false);
        }
        let Some(event) = self
            .remote_desktop
            .sessions
            .get_mut(session_id)
            .and_then(|session| session.keys.key_up(key))
        else {
            return false;
        };
        self.remote_desktop
            .manager
            .send_input(session_id, vec![event])
            .is_ok()
    }

    pub(in crate::features) fn send_remote_modifier_state(
        &mut self,
        session_id: &str,
        mut modifiers: Modifiers,
        capslock: Option<bool>,
    ) -> bool {
        if !self
            .remote_desktop
            .sessions
            .get(session_id)
            .is_some_and(|session| matches!(session.state, RemoteDesktopViewState::Connected))
        {
            return false;
        }
        modifiers.function = false;
        let is_vnc = self.session.metadata(session_id).is_some_and(|metadata| {
            matches!(
                metadata.launch_config,
                crate::models::SessionLaunchConfig::Vnc(_)
            )
        });
        if is_vnc && !self.vnc_input_enabled(session_id) {
            return false;
        }
        let Some(session) = self.remote_desktop.sessions.get_mut(session_id) else {
            return false;
        };
        let transitions = remote_modifier_transitions(
            session.modifiers.modifiers,
            modifiers,
            session.modifiers.capslock,
            capslock,
        );
        session.modifiers.modifiers = modifiers;
        if let Some(capslock) = capslock {
            session.modifiers.capslock = Some(capslock);
        }
        if transitions.is_empty() {
            return true;
        }
        if is_vnc {
            let events = transitions
                .into_iter()
                .filter_map(|transition| {
                    vnc_keysym_for_key(transition.key, None).map(|keysym| VncInputEvent::Key {
                        keysym,
                        pressed: transition.pressed,
                    })
                })
                .collect::<Vec<_>>();
            return !events.is_empty()
                && self
                    .remote_desktop
                    .vnc_manager
                    .send_input(session_id, events)
                    .is_ok();
        }
        let events = transitions
            .into_iter()
            .filter_map(|transition| {
                if transition.pressed {
                    session.keys.key_down(transition.key, false)
                } else {
                    session.keys.key_up(transition.key)
                }
            })
            .collect::<Vec<_>>();
        !events.is_empty()
            && self
                .remote_desktop
                .manager
                .send_input(session_id, events)
                .is_ok()
    }

    pub(in crate::features) fn rdp_secure_attention_available(&self, session_id: &str) -> bool {
        let is_rdp = self.session.metadata(session_id).is_some_and(|metadata| {
            matches!(
                metadata.launch_config,
                crate::models::SessionLaunchConfig::Rdp(_)
            )
        });
        self.remote_desktop
            .sessions
            .get(session_id)
            .is_some_and(|session| {
                secure_attention_available(is_rdp, &session.state, session.server_capabilities)
            })
    }

    pub(in crate::features) fn send_rdp_secure_attention(&mut self, session_id: &str) -> bool {
        if !self.rdp_secure_attention_available(session_id) {
            return false;
        }
        match self
            .remote_desktop
            .manager
            .send_secure_attention(session_id)
        {
            Ok(()) => {
                self.shell
                    .set_status("RDP Secure Attention sent".to_string());
                true
            }
            Err(error) => {
                self.shell.set_status(format_rdp_error(&error));
                false
            }
        }
    }

    pub(in crate::features) fn send_rdp_pointer(
        &mut self,
        session_id: &str,
        position: gpui::Point<gpui::Pixels>,
        button: Option<RemotePointerButton>,
        pressed: bool,
    ) -> bool {
        if self.session.metadata(session_id).is_some_and(|metadata| {
            matches!(
                metadata.launch_config,
                crate::models::SessionLaunchConfig::Vnc(_)
            )
        }) {
            return self.send_vnc_pointer(session_id, position, button, pressed);
        }
        let Some(session) = self.remote_desktop.sessions.get_mut(session_id) else {
            return false;
        };
        let (Some(viewport), Some(framebuffer)) = (session.viewport, session.framebuffer.as_ref())
        else {
            return false;
        };
        let transform = display_transform(
            viewport,
            framebuffer.width(),
            framebuffer.height(),
            DisplayScaleMode::Fit,
        );
        let remote = transform
            .and_then(|transform| transform.window_to_remote(logical_point(position)))
            .or_else(|| {
                (button.is_some() && !pressed)
                    .then_some(session.last_pointer)
                    .flatten()
            });
        let Some(remote) = remote else { return false };
        let now = Instant::now();
        if button.is_none() {
            if session.last_pointer == Some(remote) {
                return false;
            }
            session.last_pointer = Some(remote);
            if session.last_pointer_sent_at.is_some_and(|sent_at| {
                now.saturating_duration_since(sent_at) < POINTER_MOVE_INTERVAL
            }) {
                defer_rdp_pointer_move(
                    &mut session.pending_pointer,
                    &mut session.cursor_position,
                    remote,
                );
                return true;
            }
        }
        session.last_pointer = Some(remote);
        session.pending_pointer = None;
        session.last_pointer_sent_at = Some(now);
        let position = RemotePoint {
            x: remote.0,
            y: remote.1,
        };
        let pointer = match button {
            Some(button) => RemotePointerEvent::Button {
                position,
                button,
                pressed,
            },
            None => RemotePointerEvent::Move { position },
        };
        let sent = self
            .remote_desktop
            .manager
            .send_input(session_id, vec![RdpInputEvent::Pointer(pointer)])
            .is_ok();
        record_remote_cursor_position_if_sent(&mut session.cursor_position, remote, sent)
    }

    pub(super) fn send_vnc_key(
        &mut self,
        session_id: &str,
        key: &str,
        key_char: Option<&str>,
        pressed: bool,
    ) -> bool {
        if !self.vnc_input_enabled(session_id) {
            return false;
        }
        let Some(keysym) = vnc_keysym_for_key(key, key_char) else {
            return false;
        };
        self.remote_desktop
            .vnc_manager
            .send_input(session_id, vec![VncInputEvent::Key { keysym, pressed }])
            .is_ok()
    }

    pub(super) fn send_vnc_pointer(
        &mut self,
        session_id: &str,
        position: gpui::Point<gpui::Pixels>,
        button: Option<RemotePointerButton>,
        pressed: bool,
    ) -> bool {
        if !self.vnc_input_enabled(session_id) {
            return false;
        }
        let Some(session) = self.remote_desktop.sessions.get_mut(session_id) else {
            return false;
        };
        let (Some(viewport), Some(framebuffer)) = (session.viewport, session.framebuffer.as_ref())
        else {
            return false;
        };
        let scale_mode = self
            .session
            .metadata(session_id)
            .and_then(|metadata| match &metadata.launch_config {
                crate::models::SessionLaunchConfig::Vnc(config) => {
                    Some(display_scale_mode(config.display.scale_mode))
                }
                _ => None,
            })
            .unwrap_or(DisplayScaleMode::Fit);
        let transform = display_transform(
            viewport,
            framebuffer.width(),
            framebuffer.height(),
            scale_mode,
        );
        let remote = transform
            .and_then(|transform| transform.window_to_remote(logical_point(position)))
            .or_else(|| {
                (button.is_some() && !pressed)
                    .then_some(session.last_pointer)
                    .flatten()
            });
        let Some(remote) = remote else { return false };
        if matches!(
            button,
            Some(RemotePointerButton::X1 | RemotePointerButton::X2)
        ) {
            return false;
        }
        if button.is_none() && session.last_pointer == Some(remote) {
            return false;
        }
        session.last_pointer = Some(remote);
        let position = RemotePoint {
            x: remote.0,
            y: remote.1,
        };
        let pointer = match button {
            Some(button) => RemotePointerEvent::Button {
                position,
                button,
                pressed,
            },
            None => RemotePointerEvent::Move { position },
        };
        let sent = self
            .remote_desktop
            .vnc_manager
            .send_input(session_id, vec![VncInputEvent::Pointer(pointer)])
            .is_ok();
        record_remote_cursor_position_if_sent(&mut session.cursor_position, remote, sent)
    }

    pub(in crate::features) fn send_remote_wheel(
        &mut self,
        session_id: &str,
        position: gpui::Point<gpui::Pixels>,
        delta_lines_x: f32,
        delta_lines_y: f32,
    ) -> bool {
        let is_vnc = self.session.metadata(session_id).is_some_and(|metadata| {
            matches!(
                metadata.launch_config,
                crate::models::SessionLaunchConfig::Vnc(_)
            )
        });
        if is_vnc && !self.vnc_input_enabled(session_id) {
            return false;
        }
        let scale_mode = if is_vnc {
            self.session
                .metadata(session_id)
                .and_then(|metadata| match &metadata.launch_config {
                    crate::models::SessionLaunchConfig::Vnc(config) => {
                        Some(display_scale_mode(config.display.scale_mode))
                    }
                    _ => None,
                })
                .unwrap_or(DisplayScaleMode::Fit)
        } else {
            DisplayScaleMode::Fit
        };
        let Some(session) = self.remote_desktop.sessions.get_mut(session_id) else {
            return false;
        };
        let (Some(viewport), Some(framebuffer)) = (session.viewport, session.framebuffer.as_ref())
        else {
            return false;
        };
        let Some(remote) = display_transform(
            viewport,
            framebuffer.width(),
            framebuffer.height(),
            scale_mode,
        )
        .and_then(|transform| transform.window_to_remote(logical_point(position))) else {
            return false;
        };
        session.last_pointer = Some(remote);
        session.wheel_remainder_x += delta_lines_x;
        session.wheel_remainder_y += delta_lines_y;
        let steps_x = session.wheel_remainder_x.trunc() as i32;
        let steps_y = session.wheel_remainder_y.trunc() as i32;
        session.wheel_remainder_x -= steps_x as f32;
        session.wheel_remainder_y -= steps_y as f32;
        let position = RemotePoint {
            x: remote.0,
            y: remote.1,
        };
        let mut pointers = Vec::with_capacity(2);
        if steps_y != 0 {
            pointers.push(RemotePointerEvent::Wheel {
                position,
                axis: RemoteWheelAxis::Vertical,
                rotation_units: (steps_y.saturating_mul(120))
                    .clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            });
        }
        if steps_x != 0 {
            pointers.push(RemotePointerEvent::Wheel {
                position,
                axis: RemoteWheelAxis::Horizontal,
                rotation_units: (steps_x.saturating_mul(-120))
                    .clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            });
        }
        if pointers.is_empty() {
            return true;
        }
        let sent = if is_vnc {
            self.remote_desktop
                .vnc_manager
                .send_input(
                    session_id,
                    pointers.into_iter().map(VncInputEvent::Pointer).collect(),
                )
                .is_ok()
        } else {
            self.remote_desktop
                .manager
                .send_input(
                    session_id,
                    pointers.into_iter().map(RdpInputEvent::Pointer).collect(),
                )
                .is_ok()
        };
        record_remote_cursor_position_if_sent(&mut session.cursor_position, remote, sent)
    }

    pub(super) fn vnc_input_enabled(&self, session_id: &str) -> bool {
        self.session.metadata(session_id).is_some_and(|metadata| {
            matches!(
                &metadata.launch_config,
                crate::models::SessionLaunchConfig::Vnc(config) if vnc_input_allowed(config.view_only)
            )
        })
    }

    pub(in crate::features) fn request_remote_cursor_repaint(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        let software_renderer = crate::platform::is_software_renderer(self.shell.renderer());
        self.repaint_remote_cursor(
            session_id,
            remote_cursor_repaint_interval(software_renderer),
            cx,
        );
    }

    pub(super) fn flush_pending_remote_cursor_repaint(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.session.active_id_owned() else {
            return;
        };
        let pending = self
            .remote_desktop
            .sessions
            .get(&session_id)
            .is_some_and(|session| session.cursor_repaint_pending);
        if !pending {
            return;
        }
        let software_renderer = crate::platform::is_software_renderer(self.shell.renderer());
        self.repaint_remote_cursor(
            &session_id,
            remote_cursor_repaint_interval(software_renderer),
            cx,
        );
    }

    fn repaint_remote_cursor(
        &mut self,
        session_id: &str,
        interval: Duration,
        cx: &mut Context<Self>,
    ) {
        if self.session.active_id() != Some(session_id) {
            if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
                session.cursor_repaint_pending = false;
            }
            return;
        }
        let now = Instant::now();
        let Some(session) = self.remote_desktop.sessions.get_mut(session_id) else {
            return;
        };
        let due = session
            .last_cursor_repaint_at
            .is_none_or(|last| now.saturating_duration_since(last) >= interval);
        if !due {
            session.cursor_repaint_pending = true;
            return;
        }
        session.cursor_repaint_pending = false;
        session.last_cursor_repaint_at = Some(now);
        if let Some(surface) = self.remote_desktop.surfaces.get(session_id) {
            surface.update(cx, |_, cx| cx.notify());
        }
    }

    pub(super) fn drive_rdp_pointer_flush(&mut self) {
        // The local cursor moved when the event was deferred; this pass only
        // releases the protocol send and must not invalidate the whole root view.
        let now = Instant::now();
        for (session_id, session) in &mut self.remote_desktop.sessions {
            let Some(pointer) = session.pending_pointer else {
                continue;
            };
            if session.last_pointer_sent_at.is_some_and(|sent_at| {
                now.saturating_duration_since(sent_at) < POINTER_MOVE_INTERVAL
            }) {
                continue;
            }
            session.pending_pointer = None;
            session.last_pointer_sent_at = Some(now);
            let pointer_sent = self
                .remote_desktop
                .manager
                .send_input(
                    session_id,
                    vec![RdpInputEvent::Pointer(RemotePointerEvent::Move {
                        position: RemotePoint {
                            x: pointer.0,
                            y: pointer.1,
                        },
                    })],
                )
                .is_ok();
            record_remote_cursor_position_if_sent(
                &mut session.cursor_position,
                pointer,
                pointer_sent,
            );
        }
    }
}

pub(super) fn display_scale_mode(mode: VncScaleMode) -> DisplayScaleMode {
    match mode {
        VncScaleMode::Fit => DisplayScaleMode::Fit,
        VncScaleMode::Stretch => DisplayScaleMode::Stretch,
        VncScaleMode::Actual => DisplayScaleMode::Actual,
    }
}

pub(super) fn display_transform(
    viewport: Bounds<gpui::Pixels>,
    remote_width: u32,
    remote_height: u32,
    scale_mode: DisplayScaleMode,
) -> Option<DisplayTransform> {
    DisplayTransform::new(
        LogicalRect {
            origin: LogicalPoint {
                x: f32::from(viewport.origin.x),
                y: f32::from(viewport.origin.y),
            },
            size: LogicalSize {
                width: f32::from(viewport.size.width),
                height: f32::from(viewport.size.height),
            },
        },
        remote_width,
        remote_height,
        scale_mode,
    )
}

pub(super) fn logical_point(point: gpui::Point<gpui::Pixels>) -> LogicalPoint {
    LogicalPoint {
        x: f32::from(point.x),
        y: f32::from(point.y),
    }
}

pub(super) fn defer_rdp_pointer_move(
    pending_pointer: &mut Option<(u32, u32)>,
    cursor_position: &mut nyaterm_remote_desktop::CursorPosition,
    remote: (u32, u32),
) {
    *pending_pointer = Some(remote);
    *cursor_position = nyaterm_remote_desktop::CursorPosition {
        x: remote.0,
        y: remote.1,
    };
}

pub(super) fn record_remote_cursor_position_if_sent(
    cursor_position: &mut nyaterm_remote_desktop::CursorPosition,
    remote: (u32, u32),
    sent: bool,
) -> bool {
    if sent {
        *cursor_position = nyaterm_remote_desktop::CursorPosition {
            x: remote.0,
            y: remote.1,
        };
    }
    sent
}

pub(super) fn vnc_input_allowed(view_only: bool) -> bool {
    !view_only
}

pub(super) fn rdp_key_modifiers(
    key: &str,
    mut modifiers: Modifiers,
    shift_pressed: bool,
) -> Modifiers {
    // GPUI reports shifted punctuation as the key name and clears shift on that key event.
    if shift_pressed
        && matches!(
            key,
            "!" | "@"
                | "#"
                | "$"
                | "%"
                | "^"
                | "&"
                | "*"
                | "("
                | ")"
                | "_"
                | "+"
                | "{"
                | "}"
                | "|"
                | ":"
                | "\""
                | "~"
                | "<"
                | ">"
                | "?"
        )
    {
        modifiers.shift = true;
    }
    modifiers
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RemoteModifierTransition {
    pub(super) key: &'static str,
    pub(super) pressed: bool,
}

pub(super) fn remote_modifier_transitions(
    previous: Modifiers,
    current: Modifiers,
    previous_capslock: Option<bool>,
    current_capslock: Option<bool>,
) -> Vec<RemoteModifierTransition> {
    let states = [
        ("shift", previous.shift, current.shift),
        ("control", previous.control, current.control),
        ("alt", previous.alt, current.alt),
        ("platform", previous.platform, current.platform),
    ];
    let mut transitions = Vec::with_capacity(6);
    for (key, was_pressed, pressed) in states {
        if !was_pressed && pressed {
            transitions.push(RemoteModifierTransition { key, pressed: true });
        }
    }
    for (key, was_pressed, pressed) in states.into_iter().rev() {
        if was_pressed && !pressed {
            transitions.push(RemoteModifierTransition {
                key,
                pressed: false,
            });
        }
    }
    if previous_capslock
        .zip(current_capslock)
        .is_some_and(|(previous, current)| previous != current)
    {
        transitions.push(RemoteModifierTransition {
            key: "capslock",
            pressed: true,
        });
        transitions.push(RemoteModifierTransition {
            key: "capslock",
            pressed: false,
        });
    }
    transitions
}

pub(super) fn remote_committed_text_supported(
    is_vnc: bool,
    rdp_capabilities: Option<RdpServerCapabilities>,
    vnc_capabilities: Option<VncServerCapabilities>,
) -> bool {
    if is_vnc {
        vnc_capabilities.is_some_and(|capabilities| capabilities.committed_unicode_keysyms)
    } else {
        rdp_capabilities.is_some_and(|capabilities| capabilities.committed_unicode_text)
    }
}

pub(in crate::features::remote_desktop) fn secure_attention_available(
    is_rdp: bool,
    state: &RemoteDesktopViewState,
    capabilities: Option<RdpServerCapabilities>,
) -> bool {
    is_rdp
        && matches!(state, RemoteDesktopViewState::Connected)
        && capabilities.is_some_and(|capabilities| capabilities.secure_attention)
}

pub(super) fn vnc_keysym_for_key(key: &str, key_char: Option<&str>) -> Option<u32> {
    let key = key.to_ascii_lowercase();
    let keysym = match key.as_str() {
        "backspace" => 0xff08,
        "tab" => 0xff09,
        "enter" => 0xff0d,
        "escape" => 0xff1b,
        "insert" => 0xff63,
        "delete" => 0xffff,
        "home" => 0xff50,
        "end" => 0xff57,
        "pageup" | "page_up" | "page up" => 0xff55,
        "pagedown" | "page_down" | "page down" => 0xff56,
        "left" | "arrowleft" | "arrow_left" => 0xff51,
        "up" | "arrowup" | "arrow_up" => 0xff52,
        "right" | "arrowright" | "arrow_right" => 0xff53,
        "down" | "arrowdown" | "arrow_down" => 0xff54,
        "shift" => 0xffe1,
        "capslock" => 0xffe5,
        "control" | "ctrl" => 0xffe3,
        "alt" => 0xffe9,
        "meta" | "platform" | "command" | "super" => 0xffeb,
        "f1" => 0xffbe,
        "f2" => 0xffbf,
        "f3" => 0xffc0,
        "f4" => 0xffc1,
        "f5" => 0xffc2,
        "f6" => 0xffc3,
        "f7" => 0xffc4,
        "f8" => 0xffc5,
        "f9" => 0xffc6,
        "f10" => 0xffc7,
        "f11" => 0xffc8,
        "f12" => 0xffc9,
        _ => {
            let text = key_char
                .filter(|text| !text.is_empty())
                .unwrap_or(key.as_str());
            let mut chars = text.chars();
            let ch = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            let codepoint = u32::from(ch);
            if codepoint <= 0xff {
                codepoint
            } else {
                0x0100_0000 | codepoint
            }
        }
    };
    Some(keysym)
}
