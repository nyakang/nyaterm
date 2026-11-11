pub(super) mod resize;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::models::event_wake::{ANY_INTEREST, EventWake};
use futures::channel::mpsc::UnboundedReceiver;
use gpui::{Bounds, DynamicTexture, FocusHandle, Pixels, Subscription};
use nyaterm_remote_desktop::{
    CertificatePromptReason, ClipboardTracker, CursorPosition, CursorShape, Framebuffer, KeyMapper,
    RdpCapability, RdpCertificateRequest, RdpDisplayMetrics, RdpError, RdpErrorKind,
    RdpServerCapabilities, RdpSessionConfig, RdpSessionManager, RemoteDesktopError,
    RemoteDesktopViewState, VncServerCapabilities, VncSessionManager,
};

pub(in crate::features) struct RemoteDesktopFeatureState {
    pub(super) helper_launches: HashMap<String, String>,
    pub(in crate::features) routes:
        HashMap<String, Arc<nyaterm_transport::network_route::NetworkRoute>>,
    pub(super) manager: Arc<RdpSessionManager>,
    pub(super) vnc_manager: Arc<VncSessionManager>,
    pub(super) sessions: HashMap<String, RemoteDesktopSessionState>,
    pub(super) focus: FocusHandle,
    pub(in crate::features) restore_pending: Option<String>,
    pub(super) inputs: HashMap<String, gpui::Entity<super::input::RemoteDesktopInput>>,
    pub(super) surfaces: HashMap<String, gpui::Entity<super::surface::RemoteDesktopSurface>>,
    pub(super) drain_cursor: usize,
    pub(super) last_clipboard_poll: Option<Instant>,
    pub(super) metrics_enabled: bool,
    pub(super) metrics_last_report: Instant,
    pub(super) metrics_control_events: usize,
    pub(super) metrics_frame_updates: usize,
    pub(super) pending_texture_removals: Vec<DynamicTexture>,
    pub(super) focus_subscriptions: Vec<Subscription>,
    /// Signalled by both session managers after they enqueue. Taken once by
    /// `NyaTermApp::start_remote_desktop_event_drain`.
    wake: EventWake,
    wake_rx: Option<UnboundedReceiver<()>>,
    /// True while the periodic-maintenance clock task is alive.
    periodic_clock_armed: bool,
    #[cfg(windows)]
    pub(super) native_shutting_down: bool,
}

#[derive(Clone)]
pub(super) struct RdpCertificatePrompt {
    pub(super) request: RdpCertificateRequest,
    pub(super) reason: CertificatePromptReason,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct RemoteModifierState {
    pub(super) modifiers: gpui::Modifiers,
    pub(super) capslock: Option<bool>,
}

pub(super) struct RemoteDesktopSessionState {
    pub(super) start_token: String,
    pub(super) start_attempt: nyaterm_transport::connection_attempt::ConnectionAttempt,
    pub(super) state: RemoteDesktopViewState,
    pub(super) framebuffer: Option<Framebuffer>,
    pub(super) texture: Option<DynamicTexture>,
    pub(super) texture_dirty: bool,
    pub(super) uploads_stopped: bool,
    pub(super) cursor_shape: Option<CursorShape>,
    pub(super) cursor_position: CursorPosition,
    pub(super) cursor_visible: bool,
    pub(super) cursor_texture: Option<DynamicTexture>,
    pub(super) cursor_repaint_pending: bool,
    pub(super) last_cursor_repaint_at: Option<Instant>,
    pub(super) certificate_request: Option<RdpCertificatePrompt>,
    pub(super) vnc_key_request:
        Option<(nyaterm_remote_desktop::VncServerKeyRequest, Option<String>)>,
    pub(super) vnc_trust_previous: Option<String>,
    pub(super) error: Option<RemoteDesktopError>,
    pub(super) capability: Option<RdpCapability>,
    pub(super) server_capabilities: Option<RdpServerCapabilities>,
    pub(super) vnc_server_capabilities: Option<VncServerCapabilities>,
    pub(super) clipboard: ClipboardTracker,
    pub(super) keys: KeyMapper,
    pub(super) modifiers: RemoteModifierState,
    pub(super) last_pointer: Option<(u32, u32)>,
    pub(super) wheel_remainder_x: f32,
    pub(super) wheel_remainder_y: f32,
    pub(super) last_pointer_sent_at: Option<Instant>,
    pub(super) pending_pointer: Option<(u32, u32)>,
    pub(super) last_resize: Option<RdpDisplayMetrics>,
    pub(super) last_resize_sent_at: Option<Instant>,
    pub(super) pending_resize: Option<(RdpDisplayMetrics, Instant)>,
    pub(super) dynamic_resize_disabled: bool,
    pub(super) viewport: Option<Bounds<Pixels>>,
    /// Window scale factor that `viewport` was measured with, so a reconnect can
    /// ask for the same desktop without waiting for a fresh layout.
    pub(super) viewport_scale_factor: f32,
    /// A fit-window session waiting for its panel to be measured.
    ///
    /// This is the launch request, not a second copy of the live configuration:
    /// it is taken by whoever starts the helper, and it is dropped if the tab
    /// goes away first.
    pub(super) deferred_start: Option<(RdpSessionConfig, Instant)>,
    pub(super) reconnect_attempts: u32,
    pub(super) reconnect_at: Option<Instant>,
    #[cfg(windows)]
    pub(super) native_rdp_config: Option<RdpSessionConfig>,
    #[cfg(windows)]
    pub(super) native_rdp: Option<nyaterm_windows_rdp::NativeRdpControl>,
}

impl Default for RemoteDesktopSessionState {
    fn default() -> Self {
        Self {
            start_token: nyaterm_core::uuid(),
            start_attempt: Default::default(),
            state: RemoteDesktopViewState::Connecting,
            framebuffer: None,
            texture: None,
            texture_dirty: false,
            uploads_stopped: false,
            cursor_shape: None,
            cursor_position: CursorPosition::default(),
            cursor_visible: true,
            cursor_texture: None,
            cursor_repaint_pending: false,
            last_cursor_repaint_at: None,
            certificate_request: None,
            vnc_key_request: None,
            vnc_trust_previous: None,
            error: None,
            capability: None,
            server_capabilities: None,
            vnc_server_capabilities: None,
            clipboard: ClipboardTracker::default(),
            keys: KeyMapper::default(),
            modifiers: RemoteModifierState::default(),
            last_pointer: None,
            wheel_remainder_x: 0.0,
            wheel_remainder_y: 0.0,
            last_pointer_sent_at: None,
            pending_pointer: None,
            last_resize: None,
            last_resize_sent_at: None,
            pending_resize: None,
            dynamic_resize_disabled: false,
            viewport: None,
            viewport_scale_factor: 1.0,
            deferred_start: None,
            reconnect_attempts: 0,
            reconnect_at: None,
            #[cfg(windows)]
            native_rdp_config: None,
            #[cfg(windows)]
            native_rdp: None,
        }
    }
}

impl RemoteDesktopFeatureState {
    #[cfg(windows)]
    pub(in crate::features) fn take_native_hosts_for_shutdown(
        &mut self,
    ) -> Vec<nyaterm_windows_rdp::NativeRdpControl> {
        // Latch before taking owners: COM shutdown may pump the UI queue and
        // re-enter maintenance or complete an asynchronous reconnect.
        self.native_shutting_down = true;
        self.sessions
            .values_mut()
            .filter_map(|session| {
                session.native_rdp_config = None;
                if session.deferred_start.as_ref().is_some_and(|(config, _)| {
                    config.backend == nyaterm_remote_desktop::RdpBackend::Windows
                }) {
                    session.deferred_start = None;
                }
                session.native_rdp.take()
            })
            .collect()
    }

    pub(in crate::features) fn release_inactive_textures(&mut self, active: &str) {
        for (id, session) in &mut self.sessions {
            if id == active {
                continue;
            }
            if let Some(texture) = session.texture.take() {
                self.pending_texture_removals.push(texture);
            }
            if let Some(texture) = session.cursor_texture.take() {
                self.pending_texture_removals.push(texture);
            }
            session.cursor_repaint_pending = false;
            #[cfg(windows)]
            if let Some(native) = session.native_rdp.as_mut() {
                native.set_visible(false);
            }
            session.texture_dirty = session.framebuffer.is_some();
        }
        if !self.pending_texture_removals.is_empty() {
            self.wake.signal(ANY_INTEREST);
        }
    }

    pub(in crate::features) fn blocks_configuration_replacement(&self) -> bool {
        !self.helper_launches.is_empty()
            || self.restore_pending.is_some()
            || !self.routes.is_empty()
            || self.sessions.iter().any(|(id, session)| {
                let pending = session.deferred_start.is_some()
                    || session.reconnect_at.is_some()
                    || session.certificate_request.is_some()
                    || session.vnc_key_request.is_some();
                #[cfg(windows)]
                let pending =
                    pending || session.native_rdp.is_some() || session.native_rdp_config.is_some();
                pending
                    || matches!(
                        session.state,
                        RemoteDesktopViewState::Connecting
                            | RemoteDesktopViewState::Connected
                            | RemoteDesktopViewState::Reconnecting
                            | RemoteDesktopViewState::Disconnecting
                    )
                    // A terminal state can arrive before the helper is reaped.
                    // Production teardown removes the manager record through
                    // close_detached; until then it still owns backend resources.
                    || self.manager.state(id).is_some()
                    || self.vnc_manager.state(id).is_some()
            })
    }

    /// Whether this feature still requires the native child-window presentation
    /// scope.
    ///
    /// Workspace panels read the same answer: while the scope exists the ActiveX
    /// child window is composed above the owner's GPUI scene, so anything that
    /// overlaps it must be painted in the owned overlay popup instead.
    #[cfg(windows)]
    pub(in crate::features) fn native_presentation_needed(&self) -> bool {
        !self.native_shutting_down
            && self.sessions.values().any(|session| {
                native_surface_needed(
                    session.native_rdp_config.is_some(),
                    session.native_rdp.is_some(),
                    session.state,
                )
            })
    }

    pub(in crate::features) fn new(focus: FocusHandle) -> Self {
        let (wake, wake_rx) = EventWake::new();
        let manager = Arc::new(RdpSessionManager::new());
        let vnc_manager = Arc::new(VncSessionManager::new());
        // Installed before any session exists, so every session queue gets it.
        // The closure only touches an atomic and an unbounded channel, which is
        // what `QueueWaker` requires of a callback invoked under the queue lock.
        let rdp_wake = wake.clone();
        manager.set_queue_waker(Arc::new(move || {
            rdp_wake.signal(ANY_INTEREST);
        }));
        let vnc_wake = wake.clone();
        vnc_manager.set_queue_waker(Arc::new(move || {
            vnc_wake.signal(ANY_INTEREST);
        }));
        Self {
            helper_launches: HashMap::new(),
            routes: HashMap::new(),
            manager,
            vnc_manager,
            sessions: HashMap::new(),
            focus,
            restore_pending: None,
            inputs: HashMap::new(),
            surfaces: HashMap::new(),
            drain_cursor: 0,
            last_clipboard_poll: None,
            metrics_enabled: std::env::var("NYATERM_RDP_METRICS").as_deref() == Ok("1"),
            metrics_last_report: Instant::now(),
            metrics_control_events: 0,
            metrics_frame_updates: 0,
            pending_texture_removals: Vec::new(),
            focus_subscriptions: Vec::new(),
            wake,
            wake_rx: Some(wake_rx),
            periodic_clock_armed: false,
            #[cfg(windows)]
            native_shutting_down: false,
        }
    }

    pub(in crate::features) fn take_wake_receiver(&mut self) -> Option<UnboundedReceiver<()>> {
        self.wake_rx.take()
    }

    /// Declare interest in the next enqueued event. See `models::event_wake`:
    /// this must happen before the consumer checks the queues.
    pub(in crate::features) fn arm_event_wake(&self) {
        self.wake.arm(ANY_INTEREST);
    }

    pub(in crate::features) fn has_sessions(&self) -> bool {
        !self.sessions.is_empty()
    }

    /// Whether any session is holding a coalesced pointer move that still needs
    /// sending. This is what decides the periodic clock's cadence.
    pub(in crate::features) fn pointer_flush_is_pending(&self) -> bool {
        self.sessions
            .values()
            .any(|session| session.pending_pointer.is_some())
    }

    pub(in crate::features) fn periodic_clock_is_armed(&self) -> bool {
        self.periodic_clock_armed
    }

    pub(in crate::features) fn set_periodic_clock_armed(&mut self, armed: bool) {
        self.periodic_clock_armed = armed;
    }

    pub(in crate::features) fn is_session(&self, session_id: &str) -> bool {
        self.sessions.contains_key(session_id)
    }

    pub(in crate::features) fn cancel_clipboard_transfer(
        &self,
        session_id: &str,
        transfer_id: &str,
    ) -> Result<(), RdpError> {
        self.manager
            .cancel_clipboard_transfer(session_id, transfer_id)
    }

    pub(in crate::features) fn focus(&self) -> &FocusHandle {
        &self.focus
    }

    pub(in crate::features) fn start_is_pending(&self, id: &str) -> bool {
        self.helper_launches.contains_key(id)
            || self.sessions.get(id).is_some_and(|session| {
                matches!(
                    session.state,
                    RemoteDesktopViewState::Connecting | RemoteDesktopViewState::Reconnecting
                )
            })
    }

    pub(in crate::features) fn start_matches(&self, id: &str, token: &str) -> bool {
        self.sessions.get(id).is_some_and(|session| {
            session.start_token == token && !session.start_attempt.is_cancelled()
        })
    }

    pub(super) fn renew_start(&mut self, id: &str) {
        if let Some(session) = self.sessions.get_mut(id) {
            session.start_attempt.cancel();
            session.start_attempt = Default::default();
            session.start_token = nyaterm_core::uuid();
        }
    }

    pub(in crate::features) fn insert_connecting_for_request(
        &mut self,
        id: String,
        request_id: String,
        attempt: nyaterm_transport::connection_attempt::ConnectionAttempt,
    ) {
        self.insert_connecting(id.clone());
        if let Some(session) = self.sessions.get_mut(&id) {
            session.start_token = request_id;
            session.start_attempt = attempt;
        }
    }

    /// A prepared retry is the same request when replacing its old helper.
    /// Move its cancellation context out before closing the old presentation.
    pub(super) fn take_start_context(
        &mut self,
        id: &str,
    ) -> Option<(
        String,
        nyaterm_transport::connection_attempt::ConnectionAttempt,
    )> {
        let session = self.sessions.get_mut(id)?;
        Some((
            session.start_token.clone(),
            std::mem::take(&mut session.start_attempt),
        ))
    }

    pub(in crate::features) fn insert_failed_session(
        &mut self,
        session_id: String,
        kind: RdpErrorKind,
        message: String,
    ) {
        self.insert_connecting(session_id.clone());
        if let Some(session) = self.sessions.get_mut(&session_id) {
            let error = RemoteDesktopError::from(RdpError::new(kind, message));
            session.state = RemoteDesktopViewState::Failed;
            session.error = Some(error);
            session.pending_pointer = None;
            session.pending_resize = None;
            session.reconnect_at = None;
            session.certificate_request = None;
            session.keys = KeyMapper::default();
        }
    }

    pub(in crate::features) fn remove_session(&mut self, session_id: &str) {
        self.inputs.remove(session_id);
        self.surfaces.remove(session_id);
        self.routes.remove(session_id);
        if let Some(mut session) = self.sessions.remove(session_id) {
            session.start_attempt.cancel();
            if let Some(texture) = session.texture.take() {
                self.pending_texture_removals.push(texture);
            }
            if let Some(texture) = session.cursor_texture.take() {
                self.pending_texture_removals.push(texture);
            }
        }
        // Resource disposal must run even when the last helper is already gone.
        self.wake.signal(ANY_INTEREST);
    }

    pub(super) fn insert_connecting(&mut self, session_id: String) {
        let measured = self
            .sessions
            .get(&session_id)
            .map(|session| (session.viewport, session.viewport_scale_factor));
        let mut session = RemoteDesktopSessionState::default();
        if let Some((viewport, viewport_scale_factor)) = measured {
            // The panel belongs to the tab, not to the connection: a failed or
            // restarted session is still shown in the same place, so forgetting
            // the measurement would make the next connect guess again.
            session.viewport = viewport;
            session.viewport_scale_factor = viewport_scale_factor;
        }
        if let Some(previous) = self.sessions.insert(session_id, session) {
            previous.start_attempt.cancel();
        }
    }

    pub(in crate::features) fn insert_disconnected(&mut self, session_id: String) {
        self.insert_connecting(session_id.clone());
        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.state = RemoteDesktopViewState::Disconnected;
        }
    }
}

/// Whether a native RDP surface is expected to be on screen.
///
/// This stays independent of the Windows-only control type so the presentation
/// scope rule is testable on every platform.
#[cfg(any(windows, test))]
pub(in crate::features) fn native_rdp_state_is_displayable(state: RemoteDesktopViewState) -> bool {
    matches!(
        state,
        RemoteDesktopViewState::Connecting
            | RemoteDesktopViewState::Connected
            | RemoteDesktopViewState::Reconnecting
    )
}

/// Whether a native presentation scope is still required.
///
/// The scope exists so application overlays and workspace panels can be painted
/// above the ActiveX child window. Once a native session reaches a
/// non-displayable state its surface is hidden, so keeping the scope alive only
/// leaves an input popup and a global mouse hook behind.
#[cfg(any(windows, test))]
pub(in crate::features) fn native_surface_needed(
    config_pending: bool,
    control_present: bool,
    state: RemoteDesktopViewState,
) -> bool {
    config_pending || (control_present && native_rdp_state_is_displayable(state))
}

#[cfg(test)]
mod tests {
    use nyaterm_remote_desktop::{RdpErrorKind, RemoteDesktopViewState};

    use super::RemoteDesktopFeatureState;

    #[test]
    fn configuration_replacement_ignores_display_only_tabs_but_protects_pending_work() {
        let cx = gpui::TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let focus = cx.update(|cx| cx.focus_handle());
        let mut state = RemoteDesktopFeatureState::new(focus);
        state.insert_disconnected("disconnected".into());
        state.insert_failed_session(
            "failed".into(),
            RdpErrorKind::HelperMissing,
            "fixture".into(),
        );
        assert!(!state.blocks_configuration_replacement());
        state.sessions.get_mut("failed").unwrap().reconnect_at = Some(std::time::Instant::now());
        assert!(state.blocks_configuration_replacement());
        state.sessions.get_mut("failed").unwrap().reconnect_at = None;
        assert!(!state.blocks_configuration_replacement());
        for view in [
            RemoteDesktopViewState::Connecting,
            RemoteDesktopViewState::Connected,
            RemoteDesktopViewState::Reconnecting,
            RemoteDesktopViewState::Disconnecting,
        ] {
            state.sessions.get_mut("failed").unwrap().state = view;
            assert!(state.blocks_configuration_replacement(), "{view:?}");
        }
        state.sessions.get_mut("failed").unwrap().state = RemoteDesktopViewState::Failed;
        state.restore_pending = Some("failed".into());
        assert!(state.blocks_configuration_replacement());
    }

    #[test]
    fn removing_the_last_session_wakes_disposal_without_protocol_events() {
        let cx = gpui::TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let focus = cx.update(|cx| cx.focus_handle());
        let mut state = RemoteDesktopFeatureState::new(focus);
        let mut wake = state.take_wake_receiver().unwrap();
        state.insert_disconnected("last".to_string());
        state.arm_event_wake();
        state.remove_session("last");
        assert!(wake.try_recv().is_ok());
        assert!(state.sessions.is_empty());
    }

    #[test]
    fn failed_session_transition_clears_transient_protocol_state() {
        let cx = gpui::TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let focus = cx.update(|cx| cx.focus_handle());
        let mut state = RemoteDesktopFeatureState::new(focus);
        state.insert_failed_session(
            "failed".to_string(),
            RdpErrorKind::Protocol,
            "failure".to_string(),
        );

        let session = state.sessions.get("failed").expect("failed session");
        assert!(matches!(session.state, RemoteDesktopViewState::Failed));
        assert_eq!(
            session.error.as_ref().map(|error| error.message.as_str()),
            Some("failure")
        );
        assert!(session.pending_pointer.is_none());
        assert!(session.pending_resize.is_none());
    }

    #[test]
    fn native_surface_is_only_displayable_while_the_session_can_paint() {
        use super::native_rdp_state_is_displayable;
        use RemoteDesktopViewState::{
            Connected, Connecting, Disconnected, Disconnecting, Failed, Reconnecting,
        };

        assert!(native_rdp_state_is_displayable(Connecting));
        assert!(native_rdp_state_is_displayable(Connected));
        assert!(native_rdp_state_is_displayable(Reconnecting));
        assert!(!native_rdp_state_is_displayable(Disconnecting));
        assert!(!native_rdp_state_is_displayable(Disconnected));
        assert!(!native_rdp_state_is_displayable(Failed));
    }

    #[test]
    fn native_surface_scope_ends_with_its_last_visible_session() {
        use super::native_surface_needed;
        use RemoteDesktopViewState::{
            Connected, Connecting, Disconnected, Disconnecting, Failed, Reconnecting,
        };

        // Nothing native left: the owned input popup and mouse hook must go,
        // otherwise they cover later modal child windows.
        assert!(!native_surface_needed(false, false, Connected));
        assert!(!native_surface_needed(false, false, Disconnected));
        assert!(!native_surface_needed(false, true, Failed));
        assert!(!native_surface_needed(false, true, Disconnected));

        // A pending config or a still-displayable control keeps the scope.
        assert!(native_surface_needed(true, false, Failed));
        assert!(native_surface_needed(false, true, Connecting));
        assert!(native_surface_needed(false, true, Connected));
        assert!(native_surface_needed(false, true, Reconnecting));
        assert!(!native_surface_needed(false, true, Disconnecting));
    }

    /// A floating panel joins the native overlay exactly while a native session
    /// needs the presentation scope. `native_presentation_needed` is the only
    /// gate `workspace_surface` reads before wrapping the panel.
    #[cfg(windows)]
    #[test]
    fn floating_panels_join_the_native_scope_only_while_a_native_session_needs_it() {
        use nyaterm_remote_desktop::{
            RdpBackend, RdpCertificatePolicy, RdpClipboardConfig, RdpDisplayConfig,
            RdpReconnectConfig, RdpSessionConfig,
        };

        let cx = gpui::TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let focus = cx.update(|cx| cx.focus_handle());
        let mut state = RemoteDesktopFeatureState::new(focus);
        state.insert_disconnected("native".to_string());

        let session = state.sessions.get_mut("native").expect("native session");
        session.native_rdp_config = Some(RdpSessionConfig {
            relay: None,
            backend: RdpBackend::Windows,
            windows_authentication: None,
            name: "native".into(),
            host: "127.0.0.1".into(),
            port: 1,
            username: String::new(),
            domain: String::new(),
            password: None,
            use_nla: false,
            certificate_policy: RdpCertificatePolicy::default(),
            administrative_session: false,
            display: RdpDisplayConfig::default(),
            clipboard: RdpClipboardConfig::default(),
            reconnect: RdpReconnectConfig::default(),
        });
        let config = session.native_rdp_config.as_ref().unwrap().clone();
        assert!(state.native_presentation_needed(), "pending native config");
        let session = state.sessions.get_mut("native").unwrap();
        session.native_rdp_config = None;
        // Creating the staged controller performs no COM work; only running an
        // operation would instantiate ActiveX. This needs no reachable server.
        session.native_rdp = Some(
            nyaterm_windows_rdp::NativeRdpControl::create(
                1,
                nyaterm_remote_desktop::activex::NativeRdpRect {
                    x: 0,
                    y: 0,
                    width: 800,
                    height: 600,
                    scale_percent: 100,
                },
                &config,
            )
            .unwrap(),
        );
        for view_state in [
            RemoteDesktopViewState::Connecting,
            RemoteDesktopViewState::Connected,
            RemoteDesktopViewState::Reconnecting,
        ] {
            state.sessions.get_mut("native").unwrap().state = view_state;
            assert!(state.native_presentation_needed());
        }
        for view_state in [
            RemoteDesktopViewState::Disconnecting,
            RemoteDesktopViewState::Disconnected,
            RemoteDesktopViewState::Failed,
        ] {
            state.sessions.get_mut("native").unwrap().state = view_state;
            assert!(!state.native_presentation_needed());
        }
        state.insert_disconnected("second".into());
        state.sessions.get_mut("second").unwrap().native_rdp_config = Some(config);
        assert!(
            state.native_presentation_needed(),
            "another pending session retains the scope"
        );
        state.sessions.remove("second");
        assert!(
            !state.native_presentation_needed(),
            "no displayable native sessions remain"
        );
        state.sessions.get_mut("native").unwrap().state = RemoteDesktopViewState::Connected;
        assert!(state.native_presentation_needed());
        let hosts = state.take_native_hosts_for_shutdown();
        assert!(
            !state.native_presentation_needed(),
            "shutdown suppresses native presentation"
        );
        assert!(
            state
                .sessions
                .values()
                .all(|session| session.native_rdp.is_none() && session.native_rdp_config.is_none())
        );
        drop(hosts);
    }
}
