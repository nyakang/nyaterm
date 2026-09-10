use std::time::{Duration, Instant};

use futures::StreamExt as _;
use gpui::{
    AppContext as _, Bounds, ClipboardItem, Context, DevicePixels, IntoElement as _, Modifiers,
    Point, Size, Window, point, size,
};
use nyaterm_remote_desktop::{
    CertificateDecision, CertificateMatchState, CertificatePromptReason, ClipboardOrigin,
    DirtyRect, DisplayScaleMode, DisplayTransform, Framebuffer, FramebufferLimits, LogicalPoint,
    LogicalRect, LogicalSize, RDP_FRAMEBUFFER_LIMITS, RdpCapability, RdpCertificatePolicy,
    RdpCertificateRequest, RdpCertificateResponse, RdpClipboardMode, RdpDisplayMetrics,
    RdpDisplayMode, RdpError, RdpErrorKind, RdpFrameEvent, RdpInputEvent, RdpRuntimeEvent,
    RdpServerCapabilities, RdpSessionConfig, RdpSessionState, RemoteCursorEvent,
    RemoteDesktopError, RemoteDesktopViewState, RemotePoint, RemotePointerButton,
    RemotePointerEvent, RemoteWheelAxis, VNC_FRAMEBUFFER_LIMITS, VncError, VncInputEvent,
    VncRuntimeEvent, VncScaleMode, VncServerCapabilities, VncSessionConfig, VncSessionState,
    evaluate_certificate_match,
};
use nyaterm_store::{RdpCertificateMetadata, RdpKnownHostCheck, StoreDomain, store_request};

use super::state::RdpCertificatePrompt;

use crate::features::NyaTermApp;

const RESIZE_DEBOUNCE: Duration = Duration::from_millis(150);
const RESIZE_FAILURE_WINDOW: Duration = Duration::from_secs(3);
const RESIZE_MIN_DELTA: u32 = 32;
const CLIPBOARD_POLL_INTERVAL: Duration = Duration::from_millis(250);
const POINTER_MOVE_INTERVAL: Duration = Duration::from_millis(8);
const METRICS_REPORT_INTERVAL: Duration = Duration::from_secs(5);
/// Cadence for remote-desktop maintenance when no pointer move is waiting.
///
/// Finer than the shortest thing it services (`RESIZE_DEBOUNCE`), so a debounce still
/// resolves promptly after the user stops; the clipboard and metrics intervals gate
/// themselves, so this only costs a cheap check for those.
const MAINTENANCE_INTERVAL: Duration = Duration::from_millis(100);

/// How long before the next remote-desktop maintenance pass.
///
/// A waiting pointer move gets `POINTER_MOVE_INTERVAL`, because that is the interval
/// its own send is budgeted against and a late flush is a visibly late cursor.
/// Everything else is happy on the coarser maintenance cadence.
fn remote_desktop_periodic_delay(pointer_flush_pending: bool) -> Duration {
    if pointer_flush_pending {
        POINTER_MOVE_INTERVAL
    } else {
        MAINTENANCE_INTERVAL
    }
}

impl NyaTermApp {
    pub(in crate::features) fn settle_remote_desktop_restore(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.remote_desktop.restore_pending.clone() else {
            return;
        };
        if self
            .remote_desktop
            .sessions
            .get(&id)
            .is_some_and(|session| {
                matches!(
                    session.state,
                    RemoteDesktopViewState::Connecting | RemoteDesktopViewState::Reconnecting
                )
            })
        {
            return;
        }
        self.remote_desktop.restore_pending = None;
        let Some(window) = self.shell.main_window() else {
            return;
        };
        let app = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                let _ = app.update(cx, |app, cx| {
                    app.pump_startup_restore_queue_if_ready(window, cx);
                });
            });
        });
    }

    fn prompt_remote_desktop_password(
        &mut self,
        session_id: &str,
        reset_attempts: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
            session.state = RemoteDesktopViewState::Connecting;
        }
        let Some(window) = self.shell.main_window() else {
            return;
        };
        let app = cx.weak_entity();
        let session_id = session_id.to_string();
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                let _ = app.update(cx, |app, cx| {
                    use gpui::{ParentElement as _, Styled as _};
                    use nyaterm_ui::{NyaDialogWindowExt as _, NyaInput, NyaInputState};
                    if window.has_active_nya_dialog(cx) {
                        if let Some(session) = app.remote_desktop.sessions.get_mut(&session_id) {
                            session.state = RemoteDesktopViewState::Disconnected;
                        }
                        app.settle_remote_desktop_restore(cx);
                        return;
                    }
                    let input = cx.new(|cx| NyaInputState::new(cx, "").masked(true));
                    let render_input = input.clone();
                    let cancel_input = input.clone();
                    let cancel_id = session_id.clone();
                    let focus = input.read(cx).focus_handle();
                    app.open_form_dialog(
                        (
                            rust_i18n::t!("sshAuth.missingPassword").to_string(),
                            400.,
                            rust_i18n::t!("common.confirm").to_string(),
                            move |_, _, _| {
                                gpui::div()
                                    .w_full()
                                    .h(gpui::px(36.))
                                    .child(NyaInput::new(&render_input))
                                    .into_any_element()
                            },
                            move |app, _, cx| {
                                let password = input.read(cx).value(cx);
                                if password.is_empty() {
                                    return false;
                                }
                                input.update(cx, |input, cx| input.clear(cx));
                                if let Some(metadata) = app.session.metadata_mut(&session_id) {
                                    match &mut metadata.launch_config {
                                        crate::models::SessionLaunchConfig::Rdp(config) => {
                                            config.password = Some(password.into())
                                        }
                                        crate::models::SessionLaunchConfig::Vnc(config) => {
                                            config.password = Some(password.into())
                                        }
                                        _ => return true,
                                    }
                                } else {
                                    return true;
                                }
                                app.retry_rdp_runtime(&session_id, cx);
                                app.settle_remote_desktop_restore(cx);
                                let _ = reset_attempts;
                                true
                            },
                            move |app, cx| {
                                cancel_input.update(cx, |input, cx| input.clear(cx));
                                if let Some(session) =
                                    app.remote_desktop.sessions.get_mut(&cancel_id)
                                {
                                    session.state = RemoteDesktopViewState::Disconnected;
                                }
                                app.settle_remote_desktop_restore(cx);
                                cx.notify();
                            },
                        ),
                        window,
                        cx,
                    );
                    window.focus(&focus, cx);
                });
            });
        });
    }

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
                let _ = this.send_remote_modifier_state(
                    &session_id,
                    window.modifiers(),
                    Some(window.capslock().on),
                );
            }
        });
        let focus_out = cx.on_focus_out(
            &self.remote_desktop.focus,
            window,
            |this, _event, _window, _cx| {
                super::keyboard_capture::set_keyboard_capture(
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

    pub(in crate::features) fn create_rdp_runtime(
        &mut self,
        config: RdpSessionConfig,
    ) -> Result<String, RdpError> {
        self.remote_desktop.create_rdp_session(config)
    }

    pub(in crate::features) fn create_failed_rdp_runtime(&mut self, error: RdpError) -> String {
        let session_id = nyaterm_core::uuid();
        self.remote_desktop
            .insert_failed_session(session_id.clone(), error.kind, error.message);
        session_id
    }

    pub(in crate::features) fn create_vnc_runtime(
        &mut self,
        config: VncSessionConfig,
    ) -> Result<String, VncError> {
        self.remote_desktop.create_vnc_session(config)
    }

    pub(in crate::features) fn create_failed_vnc_runtime(&mut self, error: VncError) -> String {
        let session_id = nyaterm_core::uuid();
        self.remote_desktop.insert_connecting(session_id.clone());
        if let Some(session) = self.remote_desktop.sessions.get_mut(&session_id) {
            set_remote_view_error(session, error.into());
        }
        session_id
    }

    pub(in crate::features) fn retry_rdp_runtime(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        let route_missing = !self.remote_desktop.routes.contains_key(session_id);
        if route_missing {
            let connection = self
                .session
                .metadata(session_id)
                .and_then(|metadata| metadata.source_connection_id.as_ref())
                .and_then(|id| {
                    self.connection_state
                        .connections()
                        .iter()
                        .find(|connection| &connection.id == id)
                })
                .cloned()
                .filter(|connection| {
                    connection.network.as_ref().is_some_and(|network| {
                        network.proxy_id.is_some() || network.proxy_jump_id.is_some()
                    })
                });
            if let Some(connection) = connection {
                if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
                    session.state = RemoteDesktopViewState::Connecting;
                }
                let session_id = session_id.to_string();
                self.prepare_remote_route(
                    connection,
                    move |app, result, cx| {
                        if !app.session.has_session(&session_id) {
                            app.settle_remote_desktop_restore(cx);
                            return;
                        }
                        match result {
                            Ok(route) => {
                                if let Some(metadata) = app.session.metadata_mut(&session_id) {
                                    match &mut metadata.launch_config {
                                        crate::models::SessionLaunchConfig::Rdp(config) => {
                                            config.relay = Some(route.endpoint.clone())
                                        }
                                        crate::models::SessionLaunchConfig::Vnc(config) => {
                                            config.relay = Some(route.endpoint.clone())
                                        }
                                        _ => {}
                                    }
                                }
                                app.remote_desktop.routes.insert(session_id.clone(), route);
                                app.retry_rdp_runtime(&session_id, cx);
                            }
                            Err(error) => {
                                if let Some(session) =
                                    app.remote_desktop.sessions.get_mut(&session_id)
                                {
                                    session.state = RemoteDesktopViewState::Failed;
                                }
                                app.notify_background_operation(
                                    "remote-route",
                                    nyaterm_ui::notification::NyaNotificationKind::Error,
                                    error,
                                    cx,
                                );
                            }
                        }
                        app.settle_remote_desktop_restore(cx);
                        cx.notify();
                    },
                    cx,
                );
                return;
            }
        }
        if self.session.metadata(session_id).is_some_and(|metadata| {
            matches!(
                metadata.launch_config,
                crate::models::SessionLaunchConfig::Vnc(_)
            )
        }) {
            self.restart_vnc_runtime(session_id, true, cx);
        } else {
            self.restart_rdp_runtime(session_id, true, cx);
        }
    }

    fn restart_rdp_runtime(
        &mut self,
        session_id: &str,
        reset_attempts: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(metadata) = self.session.metadata(session_id).cloned() else {
            return;
        };
        let mut config = match metadata.launch_config {
            crate::models::SessionLaunchConfig::Rdp(config) => config,
            _ => return,
        };
        if config.password.is_none()
            && let Some(connection_id) = metadata.source_connection_id.as_deref()
            && let Some(connection) = self
                .connection_state
                .connections()
                .iter()
                .find(|connection| connection.id == connection_id)
        {
            config.password = inline_remote_desktop_password(connection.auth.as_ref());
            if config.password.is_none()
                && let Some(password_id) = remote_desktop_password_id(connection.auth.as_ref())
            {
                self.request_remote_desktop_restart_password(
                    session_id.to_string(),
                    password_id,
                    reset_attempts,
                    false,
                    cx,
                );
                return;
            }
        }
        if config.password.is_none() {
            self.prompt_remote_desktop_password(session_id, reset_attempts, cx);
            return;
        }
        let reconnect_attempts = if reset_attempts {
            0
        } else {
            self.remote_desktop
                .sessions
                .get(session_id)
                .map_or(0, |session| session.reconnect_attempts)
        };
        let dynamic_resize_disabled = self
            .remote_desktop
            .sessions
            .get(session_id)
            .is_some_and(|session| session.dynamic_resize_disabled);
        let route = self.remote_desktop.routes.remove(session_id);
        let _ = self.close_rdp_runtime(session_id);
        if let Some(route) = route {
            self.remote_desktop
                .routes
                .insert(session_id.to_string(), route);
        }
        match self
            .remote_desktop
            .manager
            .create_session_with_id(session_id.to_string(), config)
        {
            Ok(_) => {
                self.remote_desktop
                    .insert_connecting(session_id.to_string());
                if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
                    session.reconnect_attempts = reconnect_attempts;
                    session.dynamic_resize_disabled = dynamic_resize_disabled;
                }
                if let Some(metadata) = self.session.metadata_mut(session_id) {
                    metadata.disconnected = false;
                }
                self.shell.set_status("RDP reconnecting".to_string());
            }
            Err(error) => {
                self.remote_desktop
                    .insert_connecting(session_id.to_string());
                if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
                    set_rdp_view_error(session, error.kind, error.message);
                }
            }
        }
    }

    fn restart_vnc_runtime(
        &mut self,
        session_id: &str,
        reset_attempts: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(metadata) = self.session.metadata(session_id).cloned() else {
            return;
        };
        let mut config = match metadata.launch_config {
            crate::models::SessionLaunchConfig::Vnc(config) => config,
            _ => return,
        };
        if config.password.is_none()
            && let Some(connection_id) = metadata.source_connection_id.as_deref()
            && let Some(connection) = self
                .connection_state
                .connections()
                .iter()
                .find(|connection| connection.id == connection_id)
        {
            config.password = inline_remote_desktop_password(connection.auth.as_ref());
            if config.password.is_none()
                && let Some(password_id) = remote_desktop_password_id(connection.auth.as_ref())
            {
                self.request_remote_desktop_restart_password(
                    session_id.to_string(),
                    password_id,
                    reset_attempts,
                    true,
                    cx,
                );
                return;
            }
        }
        if config.password.is_none()
            && config.security.mode != nyaterm_remote_desktop::VncSecurityMode::None
        {
            self.prompt_remote_desktop_password(session_id, reset_attempts, cx);
            return;
        }
        let reconnect_attempts = if reset_attempts {
            0
        } else {
            self.remote_desktop
                .sessions
                .get(session_id)
                .map_or(0, |session| session.reconnect_attempts)
        };
        let route = self.remote_desktop.routes.remove(session_id);
        let _ = self.close_vnc_runtime(session_id);
        if let Some(route) = route {
            self.remote_desktop
                .routes
                .insert(session_id.to_string(), route);
        }
        match self
            .remote_desktop
            .vnc_manager
            .create_session_with_id(session_id.to_string(), config)
        {
            Ok(_) => {
                self.remote_desktop
                    .insert_connecting(session_id.to_string());
                if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
                    session.reconnect_attempts = reconnect_attempts;
                }
                if let Some(metadata) = self.session.metadata_mut(session_id) {
                    metadata.disconnected = false;
                }
                self.shell.set_status("VNC reconnecting".to_string());
            }
            Err(error) => {
                self.remote_desktop
                    .insert_connecting(session_id.to_string());
                if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
                    set_remote_view_error(session, error.into());
                }
            }
        }
    }

    fn request_remote_desktop_restart_password(
        &mut self,
        session_id: String,
        password_id: String,
        reset_attempts: bool,
        vnc: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.remote_desktop.sessions.get_mut(&session_id) {
            session.state = RemoteDesktopViewState::Connecting;
        }
        let response_session_id = session_id.clone();
        self.submit_store_request(
            0,
            store_request(StoreDomain::Security, move |store| {
                store.load_decrypted_password_by_id(&password_id)
            }),
            move |this, event, cx| {
                let password = match event.outcome {
                    Ok(Some(entry)) => entry
                        .password
                        .filter(|password| !password.trim().is_empty()),
                    Ok(None) => None,
                    Err(error) => {
                        this.shell.set_status(format!(
                            "remote desktop reconnect could not load saved password: {error}"
                        ));
                        if let Some(session) =
                            this.remote_desktop.sessions.get_mut(&response_session_id)
                        {
                            session.state = RemoteDesktopViewState::Failed;
                        }
                        this.settle_remote_desktop_restore(cx);
                        cx.notify();
                        return;
                    }
                };
                let Some(password) = password else {
                    this.prompt_remote_desktop_password(&response_session_id, reset_attempts, cx);
                    cx.notify();
                    return;
                };
                let Some(metadata) = this.session.metadata_mut(&response_session_id) else {
                    return;
                };
                match &mut metadata.launch_config {
                    crate::models::SessionLaunchConfig::Rdp(config) if !vnc => {
                        config.password = Some(password);
                    }
                    crate::models::SessionLaunchConfig::Vnc(config) if vnc => {
                        config.password = Some(password);
                    }
                    _ => return,
                }
                if vnc {
                    this.restart_vnc_runtime(&response_session_id, reset_attempts, cx);
                } else {
                    this.restart_rdp_runtime(&response_session_id, reset_attempts, cx);
                }
                cx.notify();
            },
            cx,
        );
    }

    pub(in crate::features) fn close_remote_desktop_runtime(
        &mut self,
        session_id: &str,
    ) -> anyhow::Result<()> {
        match self
            .session
            .metadata(session_id)
            .map(|metadata| &metadata.launch_config)
        {
            Some(crate::models::SessionLaunchConfig::Vnc(_)) => self
                .close_vnc_runtime(session_id)
                .map_err(anyhow::Error::from),
            _ => self
                .close_rdp_runtime(session_id)
                .map_err(anyhow::Error::from),
        }
    }

    pub(in crate::features) fn close_rdp_runtime(
        &mut self,
        session_id: &str,
    ) -> Result<(), RdpError> {
        if self.session.active_id() == Some(session_id) {
            super::keyboard_capture::set_keyboard_capture(
                self.remote_desktop.manager.clone(),
                self.remote_desktop.vnc_manager.clone(),
                None,
            );
            self.release_remote_keys(session_id);
        }
        self.remote_desktop.remove_session(session_id);
        self.remote_desktop.manager.close(session_id)
    }

    pub(in crate::features) fn close_vnc_runtime(
        &mut self,
        session_id: &str,
    ) -> Result<(), VncError> {
        if self.session.active_id() == Some(session_id) {
            super::keyboard_capture::set_keyboard_capture(
                self.remote_desktop.manager.clone(),
                self.remote_desktop.vnc_manager.clone(),
                None,
            );
            self.release_remote_keys(session_id);
        }
        self.remote_desktop.remove_session(session_id);
        self.remote_desktop.vnc_manager.close(session_id)
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

    /// Deliver RDP and VNC session events as the helper processes produce them.
    ///
    /// Started once at window open. Before this the runtime tick polled every
    /// session queue, which capped remote-desktop framerate at the tick cadence:
    /// `has_protocol_runtime_sessions()` keeps the tick off the 500ms quiet
    /// interval, but that still left 50ms idle / 16ms under pressure, so a helper
    /// delivering 60fps was sampled at 20-60fps.
    ///
    /// The session queues keep only the newest frame, so they stay queues and
    /// only the signal is a channel; see `models::event_wake`. `update_in` rather
    /// than `update` because applying a frame needs the `Window` for its dynamic
    /// texture.
    pub(in crate::features) fn start_remote_desktop_event_drain(&mut self, cx: &mut Context<Self>) {
        let Some(mut wake_rx) = self.remote_desktop.take_wake_receiver() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            loop {
                // Arm before draining, so a frame enqueued in between still
                // signals rather than waiting for the next one.
                let drained = this.update_in(cx, |this, window, cx| {
                    this.remote_desktop.arm_event_wake();
                    // Any event means a session exists; the periodic clock is scoped
                    // to that, and every reconnect is scheduled from an event handler.
                    this.ensure_remote_desktop_periodic_clock(cx);
                    let dirty = this.drain_remote_desktop_queues(window, cx);
                    if dirty {
                        cx.notify();
                    }
                    dirty
                });
                match drained {
                    Err(_) => break,
                    // A frame can arrive while the previous one is being applied.
                    Ok(true) => continue,
                    Ok(false) => {}
                }
                if wake_rx.next().await.is_none() {
                    break;
                }
            }
        })
        .detach();
    }

    /// The queue half: everything the helper processes push.
    fn drain_remote_desktop_queues(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        for texture in self.remote_desktop.pending_texture_removals.drain(..) {
            window.remove_dynamic_texture(texture);
        }
        let ids = self
            .remote_desktop
            .sessions
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let mut dirty = false;
        for session_id in ids {
            let drain = self.remote_desktop.manager.drain(&session_id);
            let vnc_drain = self.remote_desktop.vnc_manager.drain(&session_id);
            if drain.control.is_empty()
                && drain.frames.is_empty()
                && drain.cursors.is_empty()
                && vnc_drain.control.is_empty()
                && vnc_drain.frames.is_empty()
                && vnc_drain.cursors.is_empty()
            {
                continue;
            }
            if self.remote_desktop.metrics_enabled {
                self.remote_desktop.metrics_control_events += drain.control.len();
                self.remote_desktop.metrics_frame_updates += drain.frames.len();
                self.remote_desktop.metrics_control_events += vnc_drain.control.len();
                self.remote_desktop.metrics_control_events += vnc_drain.cursors.len();
                self.remote_desktop.metrics_frame_updates += vnc_drain.frames.len();
            }
            dirty = true;
            for event in drain.control {
                self.apply_rdp_control_event(&session_id, event, window, cx);
            }
            self.apply_remote_cursor_batch(&session_id, drain.cursors, window);
            self.apply_rdp_frame_batch(&session_id, drain.frames, window);
            for event in vnc_drain.control {
                self.apply_vnc_control_event(&session_id, event, window, cx);
            }
            self.apply_remote_cursor_batch(&session_id, vnc_drain.cursors, window);
            self.apply_rdp_frame_batch(&session_id, vnc_drain.frames, window);
            if self
                .remote_desktop
                .sessions
                .get(&session_id)
                .is_some_and(|session| {
                    matches!(
                        session.state,
                        RemoteDesktopViewState::Failed | RemoteDesktopViewState::Disconnected
                    )
                })
            {
                self.remote_desktop.routes.remove(&session_id);
            }
        }
        self.settle_remote_desktop_restore(cx);
        dirty
    }

    /// The time-based half, still driven by the runtime tick.
    ///
    /// None of these is a queue read: a pointer batch flushes after a hold, a
    /// resize is debounced, the reconnect ladder waits out a backoff, the
    /// clipboard is polled on an interval, and metrics report on one. Giving each
    /// its own timer is Phase 2 of the runtime-tick plan.
    /// Drive remote-desktop maintenance on its own cadence while a session exists.
    ///
    /// These six are all genuinely time-based -- a coalesced pointer move flushes after
    /// a hold, a resize is debounced, the reconnect ladder waits out a backoff, and
    /// the clipboard and metrics report on intervals -- so this stays a poll. What was
    /// wrong was *whose* cadence it used: `runtime_quiet_tick_allowed` has no
    /// remote-desktop term, so an otherwise-idle app with a live RDP session ran this
    /// at the 500ms quiet interval, and the trailing pointer move of a gesture --
    /// budgeted at `POINTER_MOVE_INTERVAL`, 8ms -- landed up to half a second late.
    ///
    /// Armed from the remote-desktop event drain. Every reconnect is scheduled by an
    /// event handler, and a connecting session always reports at least one state
    /// change, so an event is a reliable point to start from.
    pub(in crate::features) fn ensure_remote_desktop_periodic_clock(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.remote_desktop.periodic_clock_is_armed() || !self.remote_desktop.has_sessions() {
            return;
        }
        self.remote_desktop.set_periodic_clock_armed(true);
        cx.spawn(async move |this, cx| {
            loop {
                let Ok(delay) = this.update(cx, |this, _| {
                    remote_desktop_periodic_delay(this.remote_desktop.pointer_flush_is_pending())
                }) else {
                    break;
                };
                cx.background_executor().timer(delay).await;
                // `update_in`: keyboard-capture sync needs the window.
                let Ok(keep_running) = this.update_in(cx, |this, window, cx| {
                    if this.drive_remote_desktop_periodic(window, cx) {
                        cx.notify();
                    }
                    let running = this.remote_desktop.has_sessions();
                    if !running {
                        this.remote_desktop.set_periodic_clock_armed(false);
                    }
                    running
                }) else {
                    break;
                };
                if !keep_running {
                    break;
                }
            }
        })
        .detach();
    }

    pub(in crate::features) fn drive_remote_desktop_periodic(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut dirty = self.drive_rdp_pointer_flush();
        dirty |= self.drive_rdp_resize_debounce();
        dirty |= self.drive_rdp_reconnects(cx);
        self.sync_rdp_keyboard_capture(window);
        dirty |= self.poll_active_rdp_clipboard(cx);
        self.report_rdp_metrics();
        dirty
    }

    fn apply_vnc_control_event(
        &mut self,
        session_id: &str,
        event: VncRuntimeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            VncRuntimeEvent::State { state, message, .. } => {
                let vnc_server_capabilities = vnc_capabilities_for_state(
                    &state,
                    self.remote_desktop
                        .vnc_manager
                        .server_capabilities(session_id),
                );
                let state = RemoteDesktopViewState::from(&state);
                if remote_state_clears_input(&state)
                    && let Some(input) = self.remote_desktop.inputs.get(session_id)
                {
                    input.update(cx, |input, cx| input.clear(cx));
                }
                if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
                    if remote_state_clears_input(&state) {
                        session.keys = Default::default();
                        session.modifiers = Default::default();
                    }
                    session.vnc_server_capabilities = vnc_server_capabilities;
                    session.state = state;
                }
                if let Some(message) = message {
                    self.shell.set_status(message);
                }
            }
            VncRuntimeEvent::Frame {
                event:
                    RdpFrameEvent::Reset {
                        epoch,
                        width,
                        height,
                    },
                ..
            } => {
                self.reset_rdp_framebuffer(
                    session_id,
                    epoch,
                    width,
                    height,
                    VNC_FRAMEBUFFER_LIMITS,
                    window,
                );
            }
            VncRuntimeEvent::Frame { .. } => {}
            VncRuntimeEvent::Clipboard { text, .. } => {
                if self.session.active_id() == Some(session_id) {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
            VncRuntimeEvent::Error { error, .. } => {
                if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
                    session.keys = Default::default();
                    session.modifiers = Default::default();
                    session.vnc_server_capabilities = None;
                    set_remote_view_error(session, error.into());
                }
            }
        }
    }

    fn sync_rdp_keyboard_capture(&self, window: &Window) {
        let target = self.session.active_id().and_then(|session_id| {
            (self.remote_desktop.focus.is_focused(window)
                && self
                    .remote_desktop
                    .sessions
                    .get(session_id)
                    .is_some_and(|session| {
                        matches!(session.state, RemoteDesktopViewState::Connected)
                    }))
            .then(|| {
                let is_vnc = self.session.metadata(session_id).is_some_and(|metadata| {
                    matches!(
                        metadata.launch_config,
                        crate::models::SessionLaunchConfig::Vnc(_)
                    )
                });
                (session_id.to_string(), is_vnc)
            })
        });
        super::keyboard_capture::set_keyboard_capture(
            self.remote_desktop.manager.clone(),
            self.remote_desktop.vnc_manager.clone(),
            target,
        );
    }

    pub(in crate::features) fn update_rdp_viewport(
        &mut self,
        session_id: &str,
        bounds: Bounds<gpui::Pixels>,
        scale_factor: f32,
    ) {
        let fit_window = self.session.metadata(session_id).is_some_and(|metadata| {
            matches!(
                &metadata.launch_config,
                crate::models::SessionLaunchConfig::Rdp(config)
                    if config.display.mode == RdpDisplayMode::FitWindow
            )
        });
        let Some(session) = self.remote_desktop.sessions.get_mut(session_id) else {
            return;
        };
        session.viewport = Some(bounds);
        if !fit_window || session.dynamic_resize_disabled {
            session.pending_resize = None;
            return;
        }
        self.queue_rdp_resize(session_id, fit_window_display_metrics(bounds, scale_factor));
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

    fn send_vnc_key(
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

    fn send_vnc_pointer(
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

    fn vnc_input_enabled(&self, session_id: &str) -> bool {
        self.session.metadata(session_id).is_some_and(|metadata| {
            matches!(
                &metadata.launch_config,
                crate::models::SessionLaunchConfig::Vnc(config) if vnc_input_allowed(config.view_only)
            )
        })
    }

    fn drive_rdp_pointer_flush(&mut self) -> bool {
        let now = Instant::now();
        let mut sent = false;
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
            sent |= pointer_sent;
        }
        sent
    }

    fn report_rdp_metrics(&mut self) {
        if !self.remote_desktop.metrics_enabled {
            return;
        }
        let now = Instant::now();
        if now.saturating_duration_since(self.remote_desktop.metrics_last_report)
            < METRICS_REPORT_INTERVAL
        {
            return;
        }
        tracing::debug!(
            active_sessions = self.remote_desktop.sessions.len(),
            control_events = self.remote_desktop.metrics_control_events,
            frame_updates = self.remote_desktop.metrics_frame_updates,
            "RDP runtime metrics"
        );
        self.remote_desktop.metrics_last_report = now;
        self.remote_desktop.metrics_control_events = 0;
        self.remote_desktop.metrics_frame_updates = 0;
    }

    fn apply_rdp_control_event(
        &mut self,
        session_id: &str,
        event: RdpRuntimeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            RdpRuntimeEvent::State { state, message, .. } => {
                let server_capabilities = matches!(state, RdpSessionState::Connected)
                    .then(|| self.remote_desktop.manager.server_capabilities(session_id))
                    .flatten();
                let view_state = rdp_view_state(&state);
                if remote_state_clears_input(&view_state)
                    && let Some(input) = self.remote_desktop.inputs.get(session_id)
                {
                    input.update(cx, |input, cx| input.clear(cx));
                }
                if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
                    if remote_state_clears_input(&view_state) {
                        session.keys = Default::default();
                        session.modifiers = Default::default();
                    }
                    if should_disable_dynamic_resize_after_state(
                        &state,
                        session.last_resize_sent_at,
                        Instant::now(),
                    ) {
                        session.dynamic_resize_disabled = true;
                        session.pending_resize = None;
                    }
                    if let RdpSessionState::Failed(error) = &state {
                        session.error = Some(error.clone().into());
                    }
                    session.server_capabilities = server_capabilities;
                    session.state = view_state;
                }
                if let Some(message) = message {
                    self.shell.set_status(message);
                }
            }
            RdpRuntimeEvent::Frame {
                event:
                    RdpFrameEvent::Reset {
                        epoch,
                        width,
                        height,
                    },
                ..
            } => {
                self.reset_rdp_framebuffer(
                    session_id,
                    epoch,
                    width,
                    height,
                    RDP_FRAMEBUFFER_LIMITS,
                    window,
                );
            }
            RdpRuntimeEvent::Frame { .. } => {}
            RdpRuntimeEvent::Cursor { event, .. } => {
                self.apply_remote_cursor_batch(session_id, vec![event], window);
            }
            RdpRuntimeEvent::Clipboard { text, .. } => {
                if self.session.active_id() != Some(session_id) {
                    return;
                }
                let accepted = self
                    .remote_desktop
                    .sessions
                    .get_mut(session_id)
                    .and_then(|session| {
                        session
                            .clipboard
                            .accept(ClipboardOrigin::Remote, &text)
                            .ok()
                            .flatten()
                    })
                    .is_some();
                if accepted {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
            RdpRuntimeEvent::CertificateRequest(request) => {
                self.handle_rdp_certificate_request(session_id, request, cx);
            }
            RdpRuntimeEvent::Capability { capability, .. } => {
                if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
                    session.capability = Some(capability);
                }
                if capability == RdpCapability::DynamicResizeUnavailable {
                    self.shell
                        .set_status("RDP server does not support dynamic resize".to_string());
                }
            }
            RdpRuntimeEvent::Error { error, fatal, .. } => {
                let should_reconnect = fatal && self.schedule_rdp_reconnect(session_id, &error);
                if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
                    session.error = Some(error.clone().into());
                    if fatal {
                        session.keys = Default::default();
                        session.modifiers = Default::default();
                        session.server_capabilities = None;
                    }
                    if fatal && !should_reconnect {
                        session.state = RemoteDesktopViewState::Failed;
                    }
                }
                if !should_reconnect {
                    self.shell.set_status(format_rdp_error(&error));
                }
            }
        }
    }

    fn schedule_rdp_reconnect(&mut self, session_id: &str, error: &RdpError) -> bool {
        if !rdp_error_is_retryable(error.kind) {
            return false;
        }
        let Some(config) =
            self.session
                .metadata(session_id)
                .and_then(|metadata| match &metadata.launch_config {
                    crate::models::SessionLaunchConfig::Rdp(config) => Some(&config.reconnect),
                    _ => None,
                })
        else {
            return false;
        };
        if !config.enabled {
            return false;
        }
        let Some(session) = self.remote_desktop.sessions.get_mut(session_id) else {
            return false;
        };
        if session.reconnect_attempts >= config.max_attempts {
            return false;
        }
        session.reconnect_attempts += 1;
        let delay = rdp_reconnect_delay(session.reconnect_attempts, rand::random_range(0..250));
        session.reconnect_at = Some(Instant::now() + delay);
        session.state = RemoteDesktopViewState::Reconnecting;
        self.shell.set_status(format!(
            "RDP reconnecting in {:.1}s (attempt {}/{})",
            delay.as_secs_f32(),
            session.reconnect_attempts,
            config.max_attempts
        ));
        true
    }

    fn drive_rdp_reconnects(&mut self, cx: &mut Context<Self>) -> bool {
        let now = Instant::now();
        let due = self
            .remote_desktop
            .sessions
            .iter()
            .filter(|(_, session)| session.reconnect_at.is_some_and(|deadline| now >= deadline))
            .map(|(session_id, _)| session_id.clone())
            .collect::<Vec<_>>();
        for session_id in &due {
            if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
                session.reconnect_at = None;
            }
            self.restart_rdp_runtime(session_id, false, cx);
        }
        !due.is_empty()
    }

    fn reset_rdp_framebuffer(
        &mut self,
        session_id: &str,
        epoch: u64,
        width: u32,
        height: u32,
        limits: FramebufferLimits,
        window: &mut Window,
    ) {
        let Some(session) = self.remote_desktop.sessions.get_mut(session_id) else {
            return;
        };
        if let Some(texture) = session.texture.take() {
            window.remove_dynamic_texture(texture);
        }
        if let Some(texture) = session.cursor_texture.take() {
            window.remove_dynamic_texture(texture);
        }
        match Framebuffer::new(epoch, width, height, limits) {
            Ok(framebuffer) => {
                let texture_size = size(DevicePixels(width as i32), DevicePixels(height as i32));
                match window.create_dynamic_texture(texture_size, framebuffer.pixels(), width * 4) {
                    Ok(texture) => {
                        session.framebuffer = Some(framebuffer);
                        session.texture = Some(texture);
                        session.cursor_shape = None;
                        session.cursor_position = Default::default();
                        session.cursor_visible = true;
                    }
                    Err(error) => set_rdp_view_error(
                        session,
                        RdpErrorKind::Protocol,
                        format!("failed to create RDP texture: {error}"),
                    ),
                }
            }
            Err(error) => set_rdp_view_error(
                session,
                RdpErrorKind::Protocol,
                format!("invalid RDP desktop reset: {error}"),
            ),
        }
    }

    fn apply_rdp_frame_batch(
        &mut self,
        session_id: &str,
        frames: Vec<RdpFrameEvent>,
        window: &mut Window,
    ) {
        let Some(session) = self.remote_desktop.sessions.get_mut(session_id) else {
            return;
        };
        let mut dirty_rects = Vec::new();
        for frame in frames {
            let Some(framebuffer) = session.framebuffer.as_mut() else {
                continue;
            };
            match framebuffer.apply(&frame) {
                Ok(Some(rect)) => dirty_rects.push(rect),
                Ok(None) => {}
                Err(nyaterm_remote_desktop::FramebufferError::StaleEpoch { .. }) => {}
                Err(error) => {
                    set_rdp_view_error(
                        session,
                        RdpErrorKind::Protocol,
                        format!("invalid RDP frame: {error}"),
                    );
                    return;
                }
            }
        }
        if !dirty_rects.is_empty() {
            clear_rdp_reconnect_after_frame(session);
        }
        let (Some(framebuffer), Some(texture)) = (session.framebuffer.as_ref(), session.texture)
        else {
            return;
        };
        let framebuffer_area = u64::from(framebuffer.width()) * u64::from(framebuffer.height());
        let dirty_area = dirty_rects
            .iter()
            .map(|rect| u64::from(rect.width) * u64::from(rect.height))
            .sum::<u64>();
        if dirty_rects.len() > 64 || dirty_area.saturating_mul(100) >= framebuffer_area * 60 {
            let bounds = Bounds::new(
                Point::new(DevicePixels(0), DevicePixels(0)),
                Size::new(
                    DevicePixels(framebuffer.width() as i32),
                    DevicePixels(framebuffer.height() as i32),
                ),
            );
            let _ = window.update_dynamic_texture(
                texture,
                bounds,
                framebuffer.pixels(),
                framebuffer.width() * 4,
            );
            return;
        }
        for rect in nyaterm_remote_desktop::merge_dirty_rects(dirty_rects) {
            let _ = upload_rdp_rect(window, texture, framebuffer, rect);
        }
    }

    fn apply_remote_cursor_batch(
        &mut self,
        session_id: &str,
        cursors: Vec<RemoteCursorEvent>,
        window: &mut Window,
    ) {
        let Some(session) = self.remote_desktop.sessions.get_mut(session_id) else {
            return;
        };
        for cursor in cursors {
            match cursor {
                RemoteCursorEvent::Shape(shape) => {
                    if session
                        .cursor_shape
                        .as_ref()
                        .is_some_and(|current| current.shape_id == shape.shape_id)
                        && session.cursor_texture.is_some()
                    {
                        continue;
                    }
                    if let Some(texture) = session.cursor_texture.take() {
                        window.remove_dynamic_texture(texture);
                    }
                    if shape.width > 0
                        && shape.height > 0
                        && let Ok(texture) = window.create_dynamic_texture(
                            size(
                                DevicePixels(shape.width as i32),
                                DevicePixels(shape.height as i32),
                            ),
                            &shape.pixels,
                            shape.width * 4,
                        )
                    {
                        session.cursor_texture = Some(texture);
                    }
                    session.cursor_shape = Some(shape);
                }
                RemoteCursorEvent::Position(position) => {
                    session.cursor_position = position;
                }
                RemoteCursorEvent::Visibility(visibility) => {
                    session.cursor_visible = visibility.visible;
                }
            }
        }
    }

    fn handle_rdp_certificate_request(
        &mut self,
        session_id: &str,
        request: RdpCertificateRequest,
        cx: &mut Context<Self>,
    ) {
        let policy = self
            .session
            .metadata(session_id)
            .and_then(|metadata| match &metadata.launch_config {
                crate::models::SessionLaunchConfig::Rdp(config) => Some(config.certificate_policy),
                _ => None,
            })
            .unwrap_or(RdpCertificatePolicy::Prompt);
        if policy == RdpCertificatePolicy::Insecure {
            self.apply_rdp_certificate_check(
                session_id,
                request,
                policy,
                RdpKnownHostCheck::UnknownHost,
                cx,
            );
            return;
        }
        let host = request.host.clone();
        let port = request.port;
        let fingerprint = request.sha256_fingerprint.clone();
        let request_id = request.request_id.clone();
        let failure_request_id = request_id.clone();
        let session_id = session_id.to_string();
        let submitted = self.submit_store_request(
            0,
            store_request(StoreDomain::Security, move |store| {
                store.check_rdp_known_host(&host, port, &fingerprint)
            }),
            move |this, event, cx| match event.outcome {
                Ok(check) if this.remote_desktop.sessions.contains_key(&session_id) => {
                    this.apply_rdp_certificate_check(&session_id, request, policy, check, cx);
                }
                Ok(_) => {}
                Err(error) => {
                    this.shell
                        .set_status(format!("RDP certificate verification failed: {error}"));
                    let _ = this
                        .remote_desktop
                        .manager
                        .respond_certificate(&request_id, RdpCertificateResponse::Reject);
                    cx.notify();
                }
            },
            cx,
        );
        if !submitted {
            let _ = self
                .remote_desktop
                .manager
                .respond_certificate(&failure_request_id, RdpCertificateResponse::Reject);
        }
    }

    fn apply_rdp_certificate_check(
        &mut self,
        session_id: &str,
        request: RdpCertificateRequest,
        policy: RdpCertificatePolicy,
        check: RdpKnownHostCheck,
        cx: &mut Context<Self>,
    ) {
        let match_state = match check {
            RdpKnownHostCheck::Match => CertificateMatchState::Match,
            RdpKnownHostCheck::UnknownHost => CertificateMatchState::FirstUse,
            RdpKnownHostCheck::Changed {
                remembered_fingerprint,
            } => CertificateMatchState::Changed {
                remembered_fingerprint,
            },
        };
        let expected_previous_fingerprint = match &match_state {
            CertificateMatchState::Changed {
                remembered_fingerprint,
            } => Some(remembered_fingerprint.clone()),
            CertificateMatchState::FirstUse | CertificateMatchState::Match => None,
        };
        let evaluation =
            evaluate_certificate_match(policy, match_state, &request.sha256_fingerprint);
        match evaluation.decision {
            CertificateDecision::Accept => {
                let _ = self
                    .remote_desktop
                    .manager
                    .respond_certificate(&request.request_id, RdpCertificateResponse::TrustOnce);
            }
            CertificateDecision::AcceptAndRemember => {
                self.persist_rdp_certificate_and_respond(
                    request,
                    expected_previous_fingerprint,
                    cx,
                );
            }
            CertificateDecision::Reject => {
                let _ = self
                    .remote_desktop
                    .manager
                    .respond_certificate(&request.request_id, RdpCertificateResponse::Reject);
            }
            CertificateDecision::Prompt => {
                let Some(reason) = evaluation.prompt_reason else {
                    let _ = self
                        .remote_desktop
                        .manager
                        .respond_certificate(&request.request_id, RdpCertificateResponse::Reject);
                    return;
                };
                if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
                    session.certificate_request = Some(RdpCertificatePrompt { request, reason });
                }
            }
        }
    }

    pub(in crate::features) fn resolve_rdp_certificate(
        &mut self,
        session_id: &str,
        response: RdpCertificateResponse,
        cx: &mut Context<Self>,
    ) {
        let prompt = self
            .remote_desktop
            .sessions
            .get_mut(session_id)
            .and_then(|session| session.certificate_request.take());
        let Some(prompt) = prompt else {
            return;
        };
        let expected_previous_fingerprint = match &prompt.reason {
            CertificatePromptReason::FirstUse => None,
            CertificatePromptReason::Changed {
                previous_fingerprint,
                ..
            } => Some(previous_fingerprint.clone()),
        };
        if response == RdpCertificateResponse::TrustAndRemember {
            self.persist_rdp_certificate_and_respond(
                prompt.request,
                expected_previous_fingerprint,
                cx,
            );
            return;
        }
        let response = if matches!(prompt.reason, CertificatePromptReason::Changed { .. })
            && response == RdpCertificateResponse::TrustOnce
        {
            RdpCertificateResponse::Reject
        } else {
            response
        };
        if let Err(error) = self
            .remote_desktop
            .manager
            .respond_certificate(&prompt.request.request_id, response)
        {
            self.shell.set_status(format_rdp_error(&error));
        }
    }

    fn persist_rdp_certificate_and_respond(
        &mut self,
        request: RdpCertificateRequest,
        expected_previous_fingerprint: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let request_id = request.request_id.clone();
        let failure_request_id = request_id.clone();
        let host = request.host;
        let port = request.port;
        let fingerprint = request.sha256_fingerprint;
        let metadata = RdpCertificateMetadata {
            subject: request.subject,
            issuer: request.issuer,
            valid_from: request.valid_from,
            valid_to: request.valid_to,
        };
        let submitted = self.submit_store_request(
            0,
            store_request(StoreDomain::Security, move |store| {
                store.replace_rdp_known_host_if_matches(
                    &host,
                    port,
                    expected_previous_fingerprint.as_deref(),
                    &fingerprint,
                    metadata,
                )
            }),
            move |this, event, cx| {
                let response = match event.outcome {
                    Ok(true) => RdpCertificateResponse::TrustAndRemember,
                    Ok(false) => {
                        this.shell.set_status(
                            "RDP certificate changed again before confirmation; connection rejected"
                                .to_string(),
                        );
                        RdpCertificateResponse::Reject
                    }
                    Err(error) => {
                        this.shell.set_status(format!(
                            "RDP certificate could not be remembered: {error}"
                        ));
                        RdpCertificateResponse::Reject
                    }
                };
                if let Err(error) = this
                    .remote_desktop
                    .manager
                    .respond_certificate(&request_id, response)
                {
                    this.shell.set_status(format_rdp_error(&error));
                }
                cx.notify();
            },
            cx,
        );
        if !submitted {
            let _ = self
                .remote_desktop
                .manager
                .respond_certificate(&failure_request_id, RdpCertificateResponse::Reject);
        }
    }

    pub(in crate::features) fn queue_rdp_resize(
        &mut self,
        session_id: &str,
        mut metrics: RdpDisplayMetrics,
    ) {
        metrics.width = metrics.width.clamp(200, 8192) & !1;
        metrics.height = metrics.height.clamp(200, 8192) & !1;
        metrics.desktop_scale_factor = metrics.desktop_scale_factor.clamp(100, 500);
        if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
            let remote_size = session
                .framebuffer
                .as_ref()
                .map(|framebuffer| (framebuffer.width(), framebuffer.height()));
            if session.dynamic_resize_disabled
                || !rdp_resize_is_material(remote_size, session.last_resize, metrics)
            {
                return;
            }
            session.pending_resize = Some((metrics, Instant::now()));
        }
    }

    fn drive_rdp_resize_debounce(&mut self) -> bool {
        let now = Instant::now();
        let mut sent = false;
        for (session_id, session) in &mut self.remote_desktop.sessions {
            let Some((metrics, queued_at)) = session.pending_resize else {
                continue;
            };
            if now.saturating_duration_since(queued_at) < RESIZE_DEBOUNCE {
                continue;
            }
            session.pending_resize = None;
            if self
                .remote_desktop
                .manager
                .resize_with_metrics(session_id, metrics)
                .is_ok()
            {
                session.last_resize = Some(metrics);
                session.last_resize_sent_at = Some(now);
                sent = true;
            }
        }
        sent
    }

    fn poll_active_rdp_clipboard(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(session_id) = self.session.active_id_owned() else {
            return false;
        };
        if !self.remote_desktop.is_session(&session_id) {
            return false;
        }
        if !self
            .remote_desktop
            .sessions
            .get(&session_id)
            .is_some_and(|session| matches!(session.state, RemoteDesktopViewState::Connected))
        {
            return false;
        }
        let clipboard_target =
            self.session
                .metadata(&session_id)
                .and_then(|metadata| match &metadata.launch_config {
                    crate::models::SessionLaunchConfig::Rdp(config)
                        if config.clipboard.mode == RdpClipboardMode::TextOnly =>
                    {
                        Some(RemoteDesktopClipboardTarget::Rdp)
                    }
                    crate::models::SessionLaunchConfig::Vnc(config) if config.clipboard.enabled => {
                        Some(RemoteDesktopClipboardTarget::Vnc)
                    }
                    _ => None,
                });
        let Some(clipboard_target) = clipboard_target else {
            return false;
        };
        let now = Instant::now();
        if self
            .remote_desktop
            .last_clipboard_poll
            .is_some_and(|last| now.saturating_duration_since(last) < CLIPBOARD_POLL_INTERVAL)
        {
            return false;
        }
        self.remote_desktop.last_clipboard_poll = Some(now);
        if !clipboard_has_unicode_text() {
            return false;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return false;
        };
        let Some(session) = self.remote_desktop.sessions.get_mut(&session_id) else {
            return false;
        };
        let Ok(Some(_generation)) = session.clipboard.accept(ClipboardOrigin::Local, &text) else {
            return false;
        };
        match clipboard_target {
            RemoteDesktopClipboardTarget::Rdp => {
                if let Err(error) = self
                    .remote_desktop
                    .manager
                    .set_clipboard_text(&session_id, text)
                {
                    session.error = Some(error.into());
                }
            }
            RemoteDesktopClipboardTarget::Vnc => {
                if let Err(error) = self
                    .remote_desktop
                    .vnc_manager
                    .set_clipboard_text(&session_id, text)
                {
                    session.error = Some(error.into());
                }
            }
        }
        true
    }
}

#[derive(Clone, Copy)]
enum RemoteDesktopClipboardTarget {
    Rdp,
    Vnc,
}

#[cfg(target_os = "windows")]
fn clipboard_has_unicode_text() -> bool {
    // GPUI logs every unsupported OLE clipboard format it probes. Only enter
    // that path when Windows reports the text format this bridge accepts.
    unsafe {
        windows_sys::Win32::System::DataExchange::IsClipboardFormatAvailable(
            windows_sys::Win32::System::Ole::CF_UNICODETEXT as u32,
        ) != 0
    }
}

#[cfg(not(target_os = "windows"))]
fn clipboard_has_unicode_text() -> bool {
    true
}

fn upload_rdp_rect(
    window: &mut Window,
    texture: gpui::DynamicTexture,
    framebuffer: &Framebuffer,
    rect: DirtyRect,
) -> anyhow::Result<()> {
    let stride = framebuffer.width() * 4;
    let start = (u64::from(rect.y) * u64::from(stride) + u64::from(rect.x) * 4) as usize;
    let row_bytes = rect.width * 4;
    let len = (u64::from(rect.height - 1) * u64::from(stride) + u64::from(row_bytes)) as usize;
    let pixels = &framebuffer.pixels()[start..start + len];
    window.update_dynamic_texture(
        texture,
        Bounds::new(
            point(DevicePixels(rect.x as i32), DevicePixels(rect.y as i32)),
            size(
                DevicePixels(rect.width as i32),
                DevicePixels(rect.height as i32),
            ),
        ),
        pixels,
        stride,
    )
}

fn set_rdp_view_error(
    session: &mut super::state::RemoteDesktopSessionState,
    kind: RdpErrorKind,
    message: String,
) {
    let error = RdpError::new(kind, message);
    session.error = Some(error.into());
    session.state = RemoteDesktopViewState::Failed;
}

fn set_remote_view_error(
    session: &mut super::state::RemoteDesktopSessionState,
    error: RemoteDesktopError,
) {
    session.error = Some(error);
    session.state = RemoteDesktopViewState::Failed;
}

fn rdp_error_is_retryable(kind: RdpErrorKind) -> bool {
    matches!(
        kind,
        RdpErrorKind::Timeout
            | RdpErrorKind::ConnectionRefused
            | RdpErrorKind::Tls
            | RdpErrorKind::Transport
            | RdpErrorKind::Session
    )
}

fn display_scale_mode(mode: VncScaleMode) -> DisplayScaleMode {
    match mode {
        VncScaleMode::Fit => DisplayScaleMode::Fit,
        VncScaleMode::Stretch => DisplayScaleMode::Stretch,
        VncScaleMode::Actual => DisplayScaleMode::Actual,
    }
}

fn display_transform(
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

fn logical_point(point: gpui::Point<gpui::Pixels>) -> LogicalPoint {
    LogicalPoint {
        x: f32::from(point.x),
        y: f32::from(point.y),
    }
}

fn defer_rdp_pointer_move(
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

fn record_remote_cursor_position_if_sent(
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

fn vnc_input_allowed(view_only: bool) -> bool {
    !view_only
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RemoteModifierTransition {
    key: &'static str,
    pressed: bool,
}

fn remote_modifier_transitions(
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

fn remote_committed_text_supported(
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

fn vnc_capabilities_for_state(
    state: &VncSessionState,
    capabilities: Option<VncServerCapabilities>,
) -> Option<VncServerCapabilities> {
    matches!(state, VncSessionState::Connected)
        .then_some(capabilities)
        .flatten()
}

fn remote_state_clears_input(state: &RemoteDesktopViewState) -> bool {
    matches!(
        state,
        RemoteDesktopViewState::Reconnecting
            | RemoteDesktopViewState::Disconnecting
            | RemoteDesktopViewState::Disconnected
            | RemoteDesktopViewState::Failed
    )
}

fn rdp_view_state(state: &RdpSessionState) -> RemoteDesktopViewState {
    state.into()
}

pub(super) fn secure_attention_available(
    is_rdp: bool,
    state: &RemoteDesktopViewState,
    capabilities: Option<RdpServerCapabilities>,
) -> bool {
    is_rdp
        && matches!(state, RemoteDesktopViewState::Connected)
        && capabilities.is_some_and(|capabilities| capabilities.secure_attention)
}

fn vnc_keysym_for_key(key: &str, key_char: Option<&str>) -> Option<u32> {
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

fn rdp_reconnect_delay(attempt: u32, jitter_ms: u64) -> Duration {
    const BACKOFF_SECONDS: [u64; 6] = [1, 2, 4, 8, 15, 30];
    let index = attempt.saturating_sub(1) as usize;
    Duration::from_secs(BACKOFF_SECONDS[index.min(BACKOFF_SECONDS.len() - 1)])
        + Duration::from_millis(jitter_ms.min(249))
}

fn clear_rdp_reconnect_after_frame(session: &mut super::state::RemoteDesktopSessionState) {
    session.reconnect_attempts = 0;
    session.reconnect_at = None;
    session.error = None;
}

fn rdp_resize_is_material(
    remote_size: Option<(u32, u32)>,
    last_resize: Option<RdpDisplayMetrics>,
    requested: RdpDisplayMetrics,
) -> bool {
    if last_resize == Some(requested) {
        return false;
    }
    if last_resize.is_some_and(|last| {
        last.desktop_scale_factor != requested.desktop_scale_factor
            || last.physical_size_mm != requested.physical_size_mm
    }) || (last_resize.is_none() && requested.desktop_scale_factor != 100)
    {
        return true;
    }
    let Some((remote_width, remote_height)) = remote_size else {
        return true;
    };
    remote_width.abs_diff(requested.width) >= RESIZE_MIN_DELTA
        || remote_height.abs_diff(requested.height) >= RESIZE_MIN_DELTA
}

fn fit_window_display_metrics(
    bounds: Bounds<gpui::Pixels>,
    scale_factor: f32,
) -> RdpDisplayMetrics {
    let scale_factor = if scale_factor.is_finite() {
        scale_factor.max(1.0)
    } else {
        1.0
    };
    RdpDisplayMetrics {
        width: (f32::from(bounds.size.width) * scale_factor)
            .round()
            .max(1.0) as u32,
        height: (f32::from(bounds.size.height) * scale_factor)
            .round()
            .max(1.0) as u32,
        desktop_scale_factor: (scale_factor * 100.0).round().clamp(100.0, 500.0) as u32,
        physical_size_mm: None,
    }
}

fn should_disable_dynamic_resize_after_state(
    state: &RdpSessionState,
    last_resize_sent_at: Option<Instant>,
    now: Instant,
) -> bool {
    matches!(
        state,
        RdpSessionState::Reconnecting | RdpSessionState::Failed(_)
    ) && last_resize_sent_at
        .is_some_and(|sent_at| now.saturating_duration_since(sent_at) <= RESIZE_FAILURE_WINDOW)
}

pub(super) fn format_rdp_error(error: &RdpError) -> String {
    let category = match error.kind {
        RdpErrorKind::Authentication => "Authentication failed",
        RdpErrorKind::CertificateRejected => "Certificate rejected",
        RdpErrorKind::Timeout => "Connection timed out",
        RdpErrorKind::ConnectionRefused => "Connection refused",
        RdpErrorKind::Tls => "RDP TLS connection failed",
        RdpErrorKind::Transport => "RDP transport interrupted",
        RdpErrorKind::Session => "RDP session failed",
        RdpErrorKind::Clipboard => "RDP clipboard failed",
        RdpErrorKind::Negotiation => "RDP negotiation failed",
        RdpErrorKind::HelperMissing => "RDP helper is missing",
        RdpErrorKind::HelperCrashed => "RDP helper crashed",
        RdpErrorKind::Ipc => "RDP helper communication failed",
        RdpErrorKind::Protocol => "RDP protocol error",
        RdpErrorKind::Unsupported => "RDP feature is unsupported",
    };
    format!("{category}: {}", error.message)
}

pub(super) fn format_remote_desktop_error(error: &RemoteDesktopError) -> String {
    format!("{:?}: {}", error.category, error.message)
}

fn inline_remote_desktop_password(
    auth: Option<&nyaterm_core::ConnectionAuth>,
) -> Option<nyaterm_core::SecretString> {
    let auth = auth?;
    if auth.mode == "none" {
        return None;
    }
    if let Some(password) = auth
        .password
        .as_deref()
        .filter(|password| !password.trim().is_empty())
    {
        return (!auth.has_password).then(|| password.to_owned().into());
    }
    None
}

fn remote_desktop_password_id(auth: Option<&nyaterm_core::ConnectionAuth>) -> Option<String> {
    let auth = auth?;
    if auth.mode == "none"
        || auth
            .password
            .as_deref()
            .is_some_and(|password| !password.trim().is_empty() && !auth.has_password)
    {
        return None;
    }
    auth.password_id
        .as_deref()
        .map(str::trim)
        .filter(|password_id| !password_id.is_empty())
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use gpui::Modifiers;
    use nyaterm_core::ConnectionAuth;
    use nyaterm_remote_desktop::{
        CursorPosition, RdpDisplayMetrics, RdpError, RdpErrorKind, RdpServerCapabilities,
        RemoteDesktopViewState, VncServerCapabilities, VncSessionState,
    };

    use super::{
        MAINTENANCE_INTERVAL, POINTER_MOVE_INTERVAL, RESIZE_DEBOUNCE,
        clear_rdp_reconnect_after_frame, defer_rdp_pointer_move, inline_remote_desktop_password,
        rdp_error_is_retryable, rdp_reconnect_delay, rdp_resize_is_material,
        record_remote_cursor_position_if_sent, remote_committed_text_supported,
        remote_desktop_password_id, remote_desktop_periodic_delay, remote_modifier_transitions,
        remote_state_clears_input, secure_attention_available,
        should_disable_dynamic_resize_after_state, vnc_capabilities_for_state, vnc_input_allowed,
        vnc_keysym_for_key,
    };
    use crate::features::remote_desktop::state::RemoteDesktopSessionState;

    #[test]
    fn reconnect_classification_only_accepts_transient_failures() {
        for kind in [
            RdpErrorKind::Timeout,
            RdpErrorKind::ConnectionRefused,
            RdpErrorKind::Tls,
            RdpErrorKind::Transport,
            RdpErrorKind::Session,
        ] {
            assert!(rdp_error_is_retryable(kind), "{kind:?}");
        }
        for kind in [
            RdpErrorKind::Authentication,
            RdpErrorKind::CertificateRejected,
            RdpErrorKind::Negotiation,
            RdpErrorKind::Clipboard,
            RdpErrorKind::HelperMissing,
            RdpErrorKind::HelperCrashed,
            RdpErrorKind::Ipc,
            RdpErrorKind::Protocol,
            RdpErrorKind::Unsupported,
        ] {
            assert!(!rdp_error_is_retryable(kind), "{kind:?}");
        }
    }

    #[test]
    fn reconnect_password_selection_resolves_locked_values_by_id() {
        let auth = ConnectionAuth {
            mode: "password".to_string(),
            password: Some("masked-or-encrypted".to_string().into()),
            password_id: Some("pw-rdp".to_string()),
            has_password: true,
            ..ConnectionAuth::default()
        };

        assert_eq!(inline_remote_desktop_password(Some(&auth)), None);
        assert_eq!(
            remote_desktop_password_id(Some(&auth)).as_deref(),
            Some("pw-rdp")
        );
    }

    #[test]
    fn reconnect_backoff_caps_and_bounds_jitter() {
        let expected = [1, 2, 4, 8, 15, 30, 30];
        for (index, seconds) in expected.into_iter().enumerate() {
            assert_eq!(
                rdp_reconnect_delay(index as u32 + 1, 0),
                Duration::from_secs(seconds)
            );
        }
        assert_eq!(rdp_reconnect_delay(1, 999), Duration::from_millis(1_249));
    }

    /// A waiting pointer move is the one thing here that needs a fine cadence.
    ///
    /// Its send is budgeted against `POINTER_MOVE_INTERVAL`, so a flush on any coarser
    /// schedule is a visibly late cursor -- which is what the runtime tick's 500ms
    /// quiet interval was doing, since `runtime_quiet_tick_allowed` has no
    /// remote-desktop term. Everything else here debounces or gates itself.
    #[test]
    fn a_waiting_pointer_move_gets_the_fine_cadence() {
        assert_eq!(
            remote_desktop_periodic_delay(true),
            POINTER_MOVE_INTERVAL,
            "a coalesced pointer move must not wait longer than its own send interval"
        );
        assert_eq!(remote_desktop_periodic_delay(false), MAINTENANCE_INTERVAL);
        assert!(
            MAINTENANCE_INTERVAL < RESIZE_DEBOUNCE,
            "the maintenance cadence has to be finer than the shortest thing it              services, or a resize debounce resolves late"
        );
    }

    #[test]
    fn deferred_rdp_pointer_move_tracks_locally_before_protocol_flush() {
        let mut pending_pointer = Some((3, 4));
        let mut cursor_position = CursorPosition { x: 1, y: 2 };

        defer_rdp_pointer_move(&mut pending_pointer, &mut cursor_position, (9, 7));

        assert_eq!(pending_pointer, Some((9, 7)));
        assert_eq!(cursor_position, CursorPosition { x: 9, y: 7 });
    }

    #[test]
    fn remote_cursor_position_advances_only_after_pointer_send_succeeds() {
        let mut cursor_position = CursorPosition { x: 1, y: 2 };

        assert!(!record_remote_cursor_position_if_sent(
            &mut cursor_position,
            (9, 7),
            false,
        ));
        assert_eq!(cursor_position, CursorPosition { x: 1, y: 2 });

        assert!(record_remote_cursor_position_if_sent(
            &mut cursor_position,
            (9, 7),
            true,
        ));
        assert_eq!(cursor_position, CursorPosition { x: 9, y: 7 });
    }

    #[test]
    fn first_frame_clears_reconnect_attempt_and_error_state() {
        let mut session = RemoteDesktopSessionState {
            reconnect_attempts: 4,
            reconnect_at: Some(Instant::now()),
            error: Some(RdpError::new(RdpErrorKind::Transport, "interrupted").into()),
            ..Default::default()
        };

        clear_rdp_reconnect_after_frame(&mut session);

        assert_eq!(session.reconnect_attempts, 0);
        assert_eq!(session.reconnect_at, None);
        assert_eq!(session.error, None);
    }

    #[test]
    fn resize_filter_ignores_duplicate_and_sub_threshold_changes() {
        let metrics = |width, height, desktop_scale_factor| RdpDisplayMetrics {
            width,
            height,
            desktop_scale_factor,
            physical_size_mm: None,
        };
        assert!(!rdp_resize_is_material(
            Some((1280, 720)),
            None,
            metrics(1300, 740, 100)
        ));
        assert!(rdp_resize_is_material(
            Some((1280, 720)),
            None,
            metrics(1312, 720, 100)
        ));
        assert!(!rdp_resize_is_material(
            None,
            Some(metrics(1280, 720, 100)),
            metrics(1280, 720, 100)
        ));
        assert!(rdp_resize_is_material(None, None, metrics(1280, 720, 100)));
        assert!(rdp_resize_is_material(
            Some((1280, 720)),
            None,
            metrics(1280, 720, 150)
        ));
    }

    #[test]
    fn resize_related_failure_disables_dynamic_resize_only_inside_window() {
        let now = Instant::now();
        let error = RdpError::new(RdpErrorKind::Session, "resize failed");
        assert!(should_disable_dynamic_resize_after_state(
            &nyaterm_remote_desktop::RdpSessionState::Failed(error.clone()),
            Some(now - Duration::from_secs(2)),
            now,
        ));
        assert!(!should_disable_dynamic_resize_after_state(
            &nyaterm_remote_desktop::RdpSessionState::Failed(error),
            Some(now - Duration::from_secs(4)),
            now,
        ));
        assert!(!should_disable_dynamic_resize_after_state(
            &nyaterm_remote_desktop::RdpSessionState::Connected,
            Some(now),
            now,
        ));
    }

    #[test]
    fn committed_text_requires_the_confirmed_protocol_capability() {
        let rdp_supported = RdpServerCapabilities {
            committed_unicode_text: true,
            secure_attention: false,
        };
        let vnc_supported = VncServerCapabilities {
            committed_unicode_keysyms: true,
        };

        assert!(!remote_committed_text_supported(false, None, None));
        assert!(!remote_committed_text_supported(
            false,
            Some(RdpServerCapabilities::default()),
            None,
        ));
        assert!(remote_committed_text_supported(
            false,
            Some(rdp_supported),
            None,
        ));
        assert!(!remote_committed_text_supported(true, None, None));
        assert!(!remote_committed_text_supported(
            true,
            None,
            Some(VncServerCapabilities::default()),
        ));
        assert!(remote_committed_text_supported(
            true,
            None,
            Some(vnc_supported),
        ));
    }

    #[test]
    fn modifier_transitions_press_in_order_release_in_reverse_and_toggle_capslock() {
        let pressed = remote_modifier_transitions(
            Modifiers::default(),
            Modifiers {
                control: true,
                shift: true,
                platform: true,
                ..Default::default()
            },
            None,
            Some(true),
        );
        assert_eq!(
            pressed
                .iter()
                .map(|transition| (transition.key, transition.pressed))
                .collect::<Vec<_>>(),
            vec![("shift", true), ("control", true), ("platform", true)]
        );

        let released = remote_modifier_transitions(
            Modifiers {
                control: true,
                shift: true,
                platform: true,
                ..Default::default()
            },
            Modifiers::default(),
            Some(true),
            Some(false),
        );
        assert_eq!(
            released
                .iter()
                .map(|transition| (transition.key, transition.pressed))
                .collect::<Vec<_>>(),
            vec![
                ("platform", false),
                ("control", false),
                ("shift", false),
                ("capslock", true),
                ("capslock", false),
            ]
        );
    }

    #[test]
    fn vnc_capability_cache_only_survives_connected_state() {
        let supported = Some(VncServerCapabilities {
            committed_unicode_keysyms: true,
        });

        assert_eq!(
            vnc_capabilities_for_state(&VncSessionState::Connected, supported),
            supported,
        );
        assert_eq!(
            vnc_capabilities_for_state(&VncSessionState::Reconnecting, supported),
            None,
        );
        assert_eq!(
            vnc_capabilities_for_state(&VncSessionState::Disconnected, supported),
            None,
        );
    }

    #[test]
    fn physical_vnc_key_mapping_preserves_shortcut_and_navigation_keys() {
        assert_eq!(vnc_keysym_for_key("c", Some("c")), Some(u32::from('c')));
        assert_eq!(vnc_keysym_for_key("left", None), Some(0xff51));
        assert_eq!(vnc_keysym_for_key("F12", None), Some(0xffc9));
    }

    #[test]
    fn vnc_view_only_is_rejected_before_manager_dispatch() {
        assert!(vnc_input_allowed(false));
        assert!(!vnc_input_allowed(true));
    }

    #[test]
    fn secure_attention_requires_rdp_connected_state_and_confirmed_capability() {
        let supported = Some(RdpServerCapabilities {
            committed_unicode_text: true,
            secure_attention: true,
        });
        assert!(secure_attention_available(
            true,
            &RemoteDesktopViewState::Connected,
            supported,
        ));
        assert!(!secure_attention_available(
            false,
            &RemoteDesktopViewState::Connected,
            supported,
        ));
        assert!(!secure_attention_available(
            true,
            &RemoteDesktopViewState::Connecting,
            supported,
        ));
        assert!(!secure_attention_available(
            true,
            &RemoteDesktopViewState::Connected,
            None,
        ));
        assert!(!secure_attention_available(
            true,
            &RemoteDesktopViewState::Connected,
            Some(RdpServerCapabilities::default()),
        ));
    }

    #[test]
    fn disconnecting_states_clear_local_composition_and_key_suppression() {
        assert!(!remote_state_clears_input(
            &RemoteDesktopViewState::Connected
        ));
        assert!(remote_state_clears_input(
            &RemoteDesktopViewState::Reconnecting
        ));
        assert!(remote_state_clears_input(
            &RemoteDesktopViewState::Disconnected
        ));
        assert!(remote_state_clears_input(&RemoteDesktopViewState::Failed));
    }
}
