use std::collections::HashSet;

use gpui::{
    Bounds, Context, IntoElement, ParentElement as _, Pixels, Styled as _, canvas, div, rgb,
};
use nyaterm_remote_desktop::activex::{
    NativeRdpConnectionEvent, NativeRdpDisconnectInfo, NativeRdpRect, NativeRdpSessionDetails,
};
use nyaterm_remote_desktop::{RdpError, RdpErrorKind, RemoteDesktopViewState};
use nyaterm_windows_rdp::NativeRdpControl;
use raw_window_handle::RawWindowHandle;

use crate::features::NyaTermApp;

use super::runtime::fit_window_display_metrics;
use super::state::{RemoteDesktopSessionState, native_rdp_state_is_displayable};

impl NyaTermApp {
    pub(super) fn native_rdp_view(
        &mut self,
        session_id: String,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let app = cx.entity();
        div()
            .size_full()
            .bg(rgb(0x101010))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(0x999999))
                    .child(rust_i18n::t!("remoteDesktop.connecting").to_string()),
            )
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let clipped = bounds.intersect(&Bounds::new(
                            gpui::Point::default(),
                            window.viewport_size(),
                        ));
                        let app = app.clone();
                        let session_id = session_id.clone();
                        let scale = window.scale_factor();
                        window.defer(cx, move |_, cx| {
                            app.update(cx, |this, cx| {
                                this.update_rdp_viewport(&session_id, clipped, scale, cx);
                            });
                        });
                        bounds
                    },
                    |_, _, _, _| {},
                )
                .size_full(),
            )
            .into_any_element()
    }

    fn ensure_native_rdp(
        &mut self,
        session_id: &str,
        rect: NativeRdpRect,
        parent: isize,
        overlay: NativeRdpOverlayState,
        cx: &mut Context<Self>,
    ) {
        if self.remote_desktop.native_shutting_down {
            return;
        }
        let Some(session) = self.remote_desktop.sessions.get_mut(session_id) else {
            return;
        };
        if let Some(native) = session.native_rdp.as_mut() {
            native.set_bounds(rect);
            native.set_local_input(overlay.obscured, overlay.restore_focus);
            native.set_visible(native_rdp_surface_visible(true, true, session.state));
            return;
        }
        let Some(mut config) = session.native_rdp_config.take() else {
            return;
        };
        if config.display.mode == nyaterm_remote_desktop::RdpDisplayMode::FitWindow {
            let (width, height) = fit_native_desktop_dimensions(rect);
            config.display.width = width;
            config.display.height = height;
            config.display.desktop_scale_factor = rect.scale_percent;
        }
        match NativeRdpControl::create(parent, rect, &config) {
            Ok(mut native) => {
                native.set_visible(true);
                native.set_local_input(overlay.obscured, overlay.restore_focus);
                session.native_rdp = Some(native);
                session.state = RemoteDesktopViewState::Connecting;
                session.error = None;
                self.ensure_remote_desktop_periodic_clock(cx);
            }
            Err(message) => {
                session.state = RemoteDesktopViewState::Failed;
                session.error = Some(RdpError::new(RdpErrorKind::Unsupported, message).into());
            }
        }
        cx.notify();
    }

    pub(in crate::features) fn native_rdp_overlay_state(
        &mut self,
        dialog_open: bool,
        cx: &mut Context<Self>,
    ) -> NativeRdpOverlayState {
        NativeRdpOverlayState {
            obscured: dialog_open || self.root_overlay_visibility(cx).obscures_native_surface(),
            restore_focus: false,
        }
    }

    pub(in crate::features) fn native_rdp_session_details(
        &self,
        session_id: &str,
    ) -> Option<NativeRdpSessionDetails> {
        self.remote_desktop
            .sessions
            .get(session_id)?
            .native_rdp
            .as_ref()
            .map(NativeRdpControl::session_details)
    }

    pub(in crate::features) fn native_rdp_visible_tab_roots(&self) -> HashSet<String> {
        if self.shell.pane_focus_mode() {
            return self
                .active_workspace_tab()
                .filter(|key| crate::models::WorkspaceTabId::from_layout_key(key).is_session())
                .filter(|id| !self.shell.session_tab_is_hidden(id))
                .into_iter()
                .collect();
        }
        self.visible_terminal_session_ids()
            .into_iter()
            .filter(|id| !self.shell.session_tab_is_hidden(id))
            .map(str::to_string)
            .collect()
    }

    pub(super) fn drive_native_rdp_sessions(
        &mut self,
        window: &mut gpui::Window,
        visible_tab_roots: &HashSet<String>,
        obscured: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.remote_desktop.native_shutting_down {
            return false;
        }
        // Native children are hosted only by the main window. Any other window
        // lacks `native_child_surfaces`, and failing to enable overlays there
        // would fail every native session.
        if self
            .shell
            .main_window()
            .is_some_and(|main| gpui::AnyWindowHandle::from(main) != window.window_handle())
        {
            return false;
        }
        let needs_native = self.remote_desktop.native_presentation_needed();
        if needs_native {
            if let Err(error) = nyaterm_ui::enable_native_child_overlays(window) {
                for session in self.remote_desktop.sessions.values_mut() {
                    if session.native_rdp_config.take().is_some() || session.native_rdp.is_some() {
                        session.native_rdp = None;
                        session.state = RemoteDesktopViewState::Failed;
                        session.error = Some(RdpError::new(RdpErrorKind::Unsupported,
                            format!("Windows Native presentation unavailable: {error}. Switch to Builtin manually.")).into());
                    }
                }
                // Builtin and ordinary GPUI presentation remain usable after a
                // composition failure. Native owners are retired outside this borrow.
                let _ = window.set_native_child_surface_mode(false);
                return true;
            }
        } else {
            // A native presentation scope must not outlive the surfaces that need
            // it. Left on, it keeps the owned input popup (raised to `HWND_TOP` on
            // every frame) and its low-level mouse hook above every other
            // top-level window. A later modal child window - for example the
            // connection editor opened after the remote session was closed -
            // would then never receive input: the popup forwards its clicks to
            // the disabled owner window and takes foreground focus back, which
            // makes the whole application look unresponsive. Disabling the scope
            // drops the popup and the hook and restores ordinary GPUI
            // composition for the next session.
            let _ = window.set_native_child_surface_mode(false);
        }
        let visible: HashSet<String> = self
            .remote_desktop
            .sessions
            .iter()
            .filter(|(id, _)| visible_tab_roots.contains(&self.tab_root_for_session(id)))
            .map(|(id, _)| id.clone())
            .collect();
        // A restored background tab still needs a running session. Its saved
        // dimensions are used after the same bounded layout wait as IronRDP;
        // it stays hidden until the tab has a visible panel.
        if let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(window)
            && let RawWindowHandle::Win32(handle) = handle.as_raw()
        {
            let pending: Vec<_> = self
                .remote_desktop
                .sessions
                .iter()
                .filter_map(|(id, session)| {
                    session.native_rdp_config.as_ref().map(|config| {
                        let rect = session
                            .viewport
                            .map(|bounds| native_phys_rect(bounds, window.scale_factor()))
                            .unwrap_or(NativeRdpRect {
                                x: 0,
                                y: 0,
                                width: config.display.width as i32,
                                height: config.display.height as i32,
                                scale_percent: (window.scale_factor() * 100.)
                                    .round()
                                    .clamp(100., 500.)
                                    as u32,
                            });
                        (id.clone(), rect)
                    })
                })
                .collect();
            for (id, rect) in pending {
                self.ensure_native_rdp(
                    &id,
                    rect,
                    handle.hwnd.get(),
                    NativeRdpOverlayState {
                        obscured,
                        restore_focus: true,
                    },
                    cx,
                );
            }
        }
        let mut dirty = false;
        let details_open = self.session.dialog_session_info_is_open();
        let active_session = self.session.active_id().map(str::to_string);
        let mut display_warning = None;
        for (id, session) in &mut self.remote_desktop.sessions {
            let Some(native) = session.native_rdp.as_mut() else {
                continue;
            };
            let details_changed = native.take_details_changed();
            dirty |= details_changed && details_open;
            if details_changed && let Some(message) = native.session_details().status_text {
                display_warning = Some(message);
            }
            if let Some(bounds) = session.viewport {
                native.set_bounds(native_phys_rect(bounds, window.scale_factor()));
            }
            native.set_visible(native_rdp_surface_visible(
                window.is_visible(),
                visible.contains(id),
                session.state,
            ));
            native.set_local_input(obscured, active_session.as_deref() == Some(id.as_str()));
            let events: Vec<_> = std::iter::from_fn(|| native.poll_connection()).collect();
            for event in events {
                apply_native_connection_event(session, event);
                dirty = true;
            }
        }
        if let Some(message) = display_warning {
            self.shell.set_status(message);
            dirty = true;
        }
        dirty
    }
}

// Shared by the native event drain and restore/lifecycle regression tests.
pub(super) fn apply_native_connection_event(
    session: &mut RemoteDesktopSessionState,
    event: NativeRdpConnectionEvent,
) {
    match event {
        NativeRdpConnectionEvent::Connected => {
            session.state = RemoteDesktopViewState::Connected;
            session.error = None;
        }
        NativeRdpConnectionEvent::Reconnecting => {
            session.state = RemoteDesktopViewState::Reconnecting;
        }
        NativeRdpConnectionEvent::Disconnected(info) => {
            session.state = RemoteDesktopViewState::Failed;
            session.error =
                Some(RdpError::new(RdpErrorKind::Session, native_disconnect_message(&info)).into());
        }
        NativeRdpConnectionEvent::Failed(error) => {
            session.state = RemoteDesktopViewState::Failed;
            session.error = Some(error.into());
        }
    }
    if !native_rdp_state_is_displayable(session.state)
        && let Some(native) = session.native_rdp.as_mut()
    {
        native.request_shutdown();
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::features) struct NativeRdpOverlayState {
    pub obscured: bool,
    pub restore_focus: bool,
}

fn native_rdp_surface_visible(
    window_visible: bool,
    tab_visible: bool,
    state: RemoteDesktopViewState,
) -> bool {
    window_visible && tab_visible && native_rdp_state_is_displayable(state)
}

fn native_disconnect_message(info: &NativeRdpDisconnectInfo) -> String {
    if let Some(description) = info
        .description
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return description.to_string();
    }
    match info.extended_reason {
        Some(5) => "The remote session was taken over by another connection.".to_string(),
        Some(3 | 4) => "The remote session timed out.".to_string(),
        Some(12) => "The remote session was logged off.".to_string(),
        Some(7..=9) => "The server denied the remote connection.".to_string(),
        Some(0) | None => match info.basic_reason {
            Some(3) => {
                "The server ended the remote session; it may have been taken over by another connection."
                    .to_string()
            }
            Some(2) => "The remote session was ended by the remote user.".to_string(),
            Some(0) | None => "The remote session ended unexpectedly.".to_string(),
            Some(code) => format!("The remote session was disconnected (reason {code})."),
        },
        Some(code) => format!("The remote session ended (reason {code})."),
    }
}

fn native_phys_rect(bounds: Bounds<Pixels>, scale: f32) -> NativeRdpRect {
    let metrics = fit_window_display_metrics(bounds, scale);
    NativeRdpRect {
        x: (f32::from(bounds.origin.x) * scale).round() as i32,
        y: (f32::from(bounds.origin.y) * scale).round() as i32,
        width: (f32::from(bounds.size.width) * scale)
            .round()
            .clamp(1., 16384.) as i32,
        height: (f32::from(bounds.size.height) * scale)
            .round()
            .clamp(1., 16384.) as i32,
        scale_percent: metrics.desktop_scale_factor,
    }
}

fn fit_native_desktop_dimensions(rect: NativeRdpRect) -> (u32, u32) {
    (
        (rect.width.clamp(200, 8192) as u32) & !1,
        (rect.height.clamp(200, 8192) as u32) & !1,
    )
}

#[cfg(test)]
mod tests {
    use super::{native_disconnect_message, native_rdp_surface_visible};
    use nyaterm_remote_desktop::RemoteDesktopViewState;
    use nyaterm_remote_desktop::activex::NativeRdpDisconnectInfo;

    #[test]
    fn dismissing_an_overlay_never_resurrects_a_hidden_or_failed_tab() {
        use RemoteDesktopViewState::{Connected, Failed};
        assert!(native_rdp_surface_visible(true, true, Connected));
        assert!(!native_rdp_surface_visible(true, false, Connected));
        assert!(!native_rdp_surface_visible(false, true, Connected));
        assert!(!native_rdp_surface_visible(true, true, Failed));
    }

    #[test]
    fn takeover_disconnects_explain_themselves() {
        let info = NativeRdpDisconnectInfo {
            extended_reason: Some(5),
            ..Default::default()
        };
        assert!(native_disconnect_message(&info).contains("taken over"));
    }
}
