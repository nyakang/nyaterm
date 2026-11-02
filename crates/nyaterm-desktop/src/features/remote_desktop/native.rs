use std::collections::HashSet;

use gpui::{
    Bounds, Context, IntoElement, ParentElement as _, Pixels, Styled as _, canvas, div, rgb,
};
use nyaterm_remote_desktop::activex::{
    NativeRdpConnectionEvent, NativeRdpControl, NativeRdpDisconnectInfo, NativeRdpRect,
    NativeRdpSessionDetails,
};
use nyaterm_remote_desktop::{RdpError, RdpErrorKind, RemoteDesktopViewState};
use nyaterm_ui::NyaDialogWindowExt as _;
use raw_window_handle::RawWindowHandle;

use crate::features::NyaTermApp;

use super::runtime::fit_window_display_metrics;
use super::state::RemoteDesktopSessionState;

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
                    .child(rust_i18n::t!("dialog.rdpActiveXHidden").to_string()),
            )
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(window)
                        else {
                            return bounds;
                        };
                        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
                            return bounds;
                        };
                        let parent = handle.hwnd.get();
                        let rect = native_phys_rect(bounds, window.scale_factor());
                        let dialog_open = window.has_active_nya_dialog(cx)
                            || nyaterm_ui::nya_popup_is_active(window, cx);
                        let app = app.clone();
                        let session_id = session_id.clone();
                        window.defer(cx, move |window, cx| {
                            app.update(cx, |this, cx| {
                                let mut overlay = this.native_rdp_overlay_state(dialog_open, cx);
                                overlay.obscured |= !window.is_visible()
                                    || !this
                                        .native_rdp_visible_tab_roots()
                                        .contains(&this.tab_root_for_session(&session_id));
                                this.ensure_native_rdp(&session_id, rect, parent, overlay, cx);
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
        let Some(session) = self.remote_desktop.sessions.get_mut(session_id) else {
            return;
        };
        if let Some(native) = session.native_rdp.as_mut() {
            native.set_bounds(rect);
            native.set_visible(native_rdp_surface_visible(
                true,
                true,
                overlay.obscured,
                session.state,
            ));
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
                native.set_visible(!overlay.obscured);
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
                        obscured: obscured || !window.is_visible() || !visible.contains(&id),
                    },
                    cx,
                );
            }
        }
        let mut dirty = false;
        let details_open = self.session.dialog_session_info_is_open();
        for (id, session) in &mut self.remote_desktop.sessions {
            let Some(native) = session.native_rdp.as_mut() else {
                continue;
            };
            dirty |= native.take_details_changed() && details_open;
            native.set_visible(native_rdp_surface_visible(
                window.is_visible(),
                visible.contains(id),
                obscured,
                session.state,
            ));
            if window.is_window_active() && native.cursor_is_inside() {
                dirty |= self.shell.clear_resize_handle_hover();
            }
            let events: Vec<_> = std::iter::from_fn(|| native.poll_connection()).collect();
            for event in events {
                apply_native_connection_event(session, event);
                dirty = true;
            }
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
        native.set_visible(false);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::features) struct NativeRdpOverlayState {
    pub obscured: bool,
}

fn native_rdp_state_is_displayable(state: RemoteDesktopViewState) -> bool {
    matches!(
        state,
        RemoteDesktopViewState::Connecting
            | RemoteDesktopViewState::Connected
            | RemoteDesktopViewState::Reconnecting
    )
}

fn native_rdp_surface_visible(
    window_visible: bool,
    tab_visible: bool,
    obscured: bool,
    state: RemoteDesktopViewState,
) -> bool {
    window_visible && tab_visible && !obscured && native_rdp_state_is_displayable(state)
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
        width: metrics.width as i32,
        height: metrics.height as i32,
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
    use super::{native_disconnect_message, native_rdp_state_is_displayable};
    use nyaterm_remote_desktop::RemoteDesktopViewState;
    use nyaterm_remote_desktop::activex::NativeRdpDisconnectInfo;

    #[test]
    fn failed_native_sessions_release_the_panel() {
        assert!(!native_rdp_state_is_displayable(
            RemoteDesktopViewState::Failed
        ));
        assert!(native_rdp_state_is_displayable(
            RemoteDesktopViewState::Connected
        ));
    }

    #[test]
    fn dismissing_an_overlay_never_resurrects_a_hidden_or_failed_tab() {
        use super::native_rdp_surface_visible;
        use RemoteDesktopViewState::{Connected, Failed};
        assert!(!native_rdp_surface_visible(true, true, true, Connected));
        assert!(native_rdp_surface_visible(true, true, false, Connected));
        assert!(!native_rdp_surface_visible(true, false, false, Connected));
        assert!(!native_rdp_surface_visible(false, true, false, Connected));
        assert!(!native_rdp_surface_visible(true, true, false, Failed));
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
