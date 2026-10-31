use super::errors::format_rdp_error;
use super::input::POINTER_MOVE_INTERVAL;
use super::lifecycle::cancel_rdp_reconnect_schedule;
use crate::features::NyaTermApp;
use crate::features::remote_desktop::state::resize::should_disable_dynamic_resize_after_state;
use crate::models::TransferJobKind;
use crate::models::TransferJobState;
use crate::models::TransferJobStatus;
use futures::StreamExt as _;
use gpui::ClipboardItem;
use gpui::Context;
use gpui::Window;
use nyaterm_remote_desktop::ClipboardOrigin;
use nyaterm_remote_desktop::RDP_FRAMEBUFFER_LIMITS;
use nyaterm_remote_desktop::RdpCapability;
use nyaterm_remote_desktop::RdpClipboardTransferStatus;
use nyaterm_remote_desktop::RdpError;
use nyaterm_remote_desktop::RdpErrorKind;
use nyaterm_remote_desktop::RdpFrameEvent;
use nyaterm_remote_desktop::RdpRuntimeEvent;
use nyaterm_remote_desktop::RdpSessionState;
use nyaterm_remote_desktop::RemoteDesktopError;
use nyaterm_remote_desktop::RemoteDesktopViewState;
use nyaterm_remote_desktop::VNC_FRAMEBUFFER_LIMITS;
use nyaterm_remote_desktop::VncRuntimeEvent;
use nyaterm_remote_desktop::VncServerCapabilities;
use nyaterm_remote_desktop::VncSessionState;
use nyaterm_store::StoreDomain;
use nyaterm_store::store_request;
use nyaterm_transport::SftpTransferProgress;
#[cfg(windows)]
use nyaterm_ui::NyaDialogWindowExt as _;
use std::time::Duration;
use std::time::Instant;

/// Cadence for remote-desktop maintenance when no pointer move is waiting.
///
/// Finer than the shortest thing it services (`RESIZE_DEBOUNCE`), so a debounce still
/// resolves promptly after the user stops; the clipboard and metrics intervals gate
/// themselves, so this only costs a cheap check for those.
pub(super) const MAINTENANCE_INTERVAL: Duration = Duration::from_millis(100);

pub(super) const METRICS_REPORT_INTERVAL: Duration = Duration::from_secs(5);

/// Frame deltas stay ordered, so a full queue must still be drained; entering
/// the UI every millisecond only moved the same work earlier and uploaded every
/// intermediate dirty update.
const HARDWARE_FRAME_DRAIN_INTERVAL: Duration = Duration::from_millis(16);

/// WARP composites the whole window on the CPU, so 60 Hz spends most of the
/// frame budget presenting rather than delivering new desktop pixels.
const SOFTWARE_FRAME_DRAIN_INTERVAL: Duration = Duration::from_millis(33);

const MIN_DRAIN_YIELD: Duration = Duration::from_millis(1);

pub(super) fn remote_desktop_drain_delay(elapsed: Duration, software_renderer: bool) -> Duration {
    let interval = if software_renderer {
        SOFTWARE_FRAME_DRAIN_INTERVAL
    } else {
        HARDWARE_FRAME_DRAIN_INTERVAL
    };
    interval.saturating_sub(elapsed).max(MIN_DRAIN_YIELD)
}

impl NyaTermApp {
    /// Deliver RDP and VNC session events as the helper processes produce them.
    ///
    /// Started once at window open. Before this the runtime tick polled every
    /// session queue, which capped remote-desktop framerate at the tick cadence:
    /// `has_protocol_runtime_sessions()` keeps the tick off the 500ms quiet
    /// interval, but that still left 50ms idle / 16ms under pressure, so a helper
    /// delivering 60fps was sampled at 20-60fps.
    ///
    /// Frame deltas stay ordered in bounded queues; only the wake signal is a
    /// channel (see `models::event_wake`). `update_in` rather
    /// than `update` because applying a frame needs the `Window` for its dynamic
    /// texture.
    pub(in crate::features) fn start_remote_desktop_event_drain(&mut self, cx: &mut Context<Self>) {
        let Some(mut wake_rx) = self.remote_desktop.take_wake_receiver() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            loop {
                let drain_started_at = Instant::now();
                // Arm before draining, so a frame enqueued in between still
                // signals rather than waiting for the next one.
                let drained = this.update_in(cx, |this, window, cx| {
                    this.remote_desktop.arm_event_wake();
                    // Any event means a session exists; the periodic clock is scoped
                    // to that, and every reconnect is scheduled from an event handler.
                    this.ensure_remote_desktop_periodic_clock(cx);
                    let drained = this.drain_remote_desktop_queues(window, cx);
                    let software_renderer =
                        crate::platform::is_software_renderer(this.shell.renderer());
                    (drained, software_renderer)
                });
                match drained {
                    Err(_) => break,
                    // A frame can arrive while the previous one is being applied.
                    // Keep ordered deltas queued until the next display-frame slot.
                    Ok((true, software_renderer)) => {
                        let delay = remote_desktop_drain_delay(
                            drain_started_at.elapsed(),
                            software_renderer,
                        );
                        cx.background_executor().timer(delay).await;
                        continue;
                    }
                    Ok((false, _)) => {}
                }
                if wake_rx.next().await.is_none() {
                    break;
                }
            }
        })
        .detach();
    }

    /// The queue half: everything the helper processes push.
    pub(super) fn drain_remote_desktop_queues(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        for texture in self.remote_desktop.pending_texture_removals.drain(..) {
            window.remove_dynamic_texture(texture);
        }
        let mut ids = self
            .remote_desktop
            .sessions
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        ids.sort_unstable();
        if !ids.is_empty() {
            let offset = self.remote_desktop.drain_cursor % ids.len();
            ids.rotate_left(offset);
        }
        let started = Instant::now();
        let mut dirty = false;
        let mut root_dirty = false;
        for session_id in ids {
            self.remote_desktop.drain_cursor = self.remote_desktop.drain_cursor.wrapping_add(1);
            let drain = self.remote_desktop.manager.drain_batch(&session_id);
            let vnc_drain = self.remote_desktop.vnc_manager.drain_batch(&session_id);
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
            root_dirty |= !drain.control.is_empty() || !vnc_drain.control.is_empty();
            let previous_state = self
                .remote_desktop
                .sessions
                .get(&session_id)
                .map(|session| session.state);
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
            root_dirty |= previous_state
                != self
                    .remote_desktop
                    .sessions
                    .get(&session_id)
                    .map(|session| session.state);
            if self.session.active_id() == Some(session_id.as_str())
                && let Some(surface) = self.remote_desktop.surfaces.get(&session_id)
            {
                surface.update(cx, |_, cx| cx.notify());
            }
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
            if started.elapsed() >= Duration::from_millis(4) {
                break;
            }
        }
        self.settle_remote_desktop_restore(cx);
        if root_dirty {
            cx.notify();
        }
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
                    let cursor_repaint_pending = this
                        .session
                        .active_id()
                        .and_then(|session_id| this.remote_desktop.sessions.get(session_id))
                        .is_some_and(|session| session.cursor_repaint_pending);
                    remote_desktop_periodic_delay(
                        this.remote_desktop.pointer_flush_is_pending(),
                        cursor_repaint_pending,
                    )
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
        for (id, session) in &self.remote_desktop.sessions {
            if session.uploads_stopped {
                let _ = self.remote_desktop.manager.close_detached(id);
                let _ = self.remote_desktop.vnc_manager.close_detached(id);
            }
        }
        self.drive_rdp_pointer_flush();
        self.flush_pending_remote_cursor_repaint(cx);
        #[cfg(windows)]
        let mut dirty = {
            let dialog_open =
                window.has_active_nya_dialog(cx) || nyaterm_ui::nya_popup_is_active(window, cx);
            let overlay = self.native_rdp_overlay_state(dialog_open, cx);
            let visible_tab_roots = self.native_rdp_visible_tab_roots();
            self.drive_native_rdp_sessions(window, &visible_tab_roots, overlay.obscured, cx)
        };
        #[cfg(not(windows))]
        let mut dirty = false;
        dirty |= self.drive_deferred_rdp_starts();
        dirty |= self.drive_rdp_resize_debounce();
        dirty |= self.drive_rdp_reconnects(cx);
        self.settle_remote_desktop_restore(cx);
        self.sync_rdp_keyboard_capture(window);
        dirty |= self.poll_active_rdp_clipboard(cx);
        self.report_rdp_metrics();
        dirty
    }

    pub(super) fn apply_vnc_control_event(
        &mut self,
        session_id: &str,
        event: VncRuntimeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            VncRuntimeEvent::ServerKeyRequest(request) => self.handle_vnc_key_request(request, cx),
            VncRuntimeEvent::ServerKeyAuthenticated(request) => {
                let previous = self
                    .remote_desktop
                    .sessions
                    .get(session_id)
                    .and_then(|session| session.vnc_trust_previous.clone());
                if let Some(trust) = self.remote_desktop.vnc_manager.trust_state(session_id) {
                    self.submit_store_request(
                        0,
                        store_request(StoreDomain::Security, move |store| {
                            trust
                                .commit_if_current(&request, |current| {
                                    store.remember_vnc_known_host_if_current(
                                        &request.host,
                                        request.port,
                                        &request.sha256_fingerprint,
                                        previous.as_deref(),
                                        current,
                                    )
                                })
                                .transpose()
                                .map(|_| ())
                        }),
                        |_, _, _| {},
                        cx,
                    );
                }
            }

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
                    if state != RemoteDesktopViewState::Connected {
                        session.vnc_key_request = None;
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
            VncRuntimeEvent::Error { error, fatal, .. } => {
                if fatal && error.kind == nyaterm_remote_desktop::VncErrorKind::ResourceExhausted {
                    self.stop_remote_desktop_uploads(
                        session_id,
                        window,
                        RdpErrorKind::ResourceExhausted,
                        error.message.clone(),
                    );
                }
                let retry = fatal
                    && matches!(
                        error.kind,
                        nyaterm_remote_desktop::VncErrorKind::Timeout
                            | nyaterm_remote_desktop::VncErrorKind::HelperCrashed
                    )
                    && self.schedule_vnc_helper_reconnect(session_id);
                // A server that asks for credentials the session does not hold is not a
                // dead end: ask for a password instead of parking a failed session. This
                // is what keeps an `auto` connection working when the server actually
                // demands VNC authentication. The helper checks the effective credentials,
                // including inherited passwords; generic authentication errors never
                // prompt (NyaTerm issue #1046).
                let prompt = fatal
                    && !retry
                    && error.kind == nyaterm_remote_desktop::VncErrorKind::Authentication
                    && error.password_required
                    && self.vnc_auth_failure_accepts_password_prompt(session_id);
                if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
                    session.keys = Default::default();
                    session.modifiers = Default::default();
                    session.vnc_server_capabilities = None;
                    session.vnc_key_request = None;
                    if retry {
                        session.error = Some(error.into());
                    } else if !prompt {
                        set_remote_view_error(session, error.into());
                    }
                }
                if prompt {
                    self.prompt_remote_desktop_password(session_id, false, cx);
                }
            }
        }
    }

    /// Whether the session is a VNC launch that can still be authenticated by asking.
    fn vnc_auth_failure_accepts_password_prompt(&self, session_id: &str) -> bool {
        self.session.metadata(session_id).is_some_and(|metadata| {
            matches!(
                &metadata.launch_config,
                crate::models::SessionLaunchConfig::Vnc(config)
                    if super::lifecycle::vnc_auth_failure_accepts_password_prompt(config)
            )
        })
    }

    pub(super) fn report_rdp_metrics(&mut self) {
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

    pub(super) fn apply_rdp_control_event(
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
                if matches!(
                    state,
                    RdpSessionState::Disconnected | RdpSessionState::Failed(_)
                ) {
                    self.stop_rdp_clipboard_jobs(session_id, cx);
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
            RdpRuntimeEvent::ClipboardTransfer { progress, .. } => {
                let status = match progress.status {
                    RdpClipboardTransferStatus::Running => TransferJobStatus::Running,
                    RdpClipboardTransferStatus::Completed => TransferJobStatus::Completed,
                    RdpClipboardTransferStatus::Failed => TransferJobStatus::Failed,
                    RdpClipboardTransferStatus::Cancelled => TransferJobStatus::Cancelled,
                };
                let sample = SftpTransferProgress {
                    remote_path: "RDP clipboard".to_string(),
                    local_path: std::path::PathBuf::new(),
                    bytes_transferred: progress.transferred_bytes,
                    total_bytes: Some(progress.total_bytes),
                    item_count_completed: Some(progress.completed_files),
                    item_count_total: Some(progress.total_files),
                };
                if let Some(job) = self.transfer.transfer_job_mut(&progress.id) {
                    job.status = status;
                    job.detail = progress.error.unwrap_or_default();
                    job.update_progress(sample);
                } else {
                    self.transfer.enqueue_transfer_job(TransferJobState {
                        id: progress.id,
                        session_id: Some(session_id.to_string()),
                        kind: TransferJobKind::RdpClipboard {
                            file_name: progress.name.clone(),
                        },
                        status,
                        detail: progress.error.unwrap_or_default(),
                        created_at_ms: TransferJobState::now_ms(),
                        display_name: progress.name,
                        entries: Vec::new(),
                        summary: None,
                        progress: Some(sample),
                        control: None,
                        speed: Default::default(),
                    });
                }
                self.defer_transfer_panel_snapshot_flush(cx);
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
                if fatal && error.kind == RdpErrorKind::ResourceExhausted {
                    self.stop_remote_desktop_uploads(
                        session_id,
                        window,
                        error.kind,
                        error.message.clone(),
                    );
                }
                if fatal {
                    self.stop_rdp_clipboard_jobs(session_id, cx);
                }
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
                        // A deadline left over from an earlier transient failure would still
                        // fire in `drive_rdp_reconnects`, silently reconnecting a session the
                        // server just refused to keep.
                        cancel_rdp_reconnect_schedule(session);
                    }
                }
                if !should_reconnect {
                    self.shell.set_status(format_rdp_error(&error));
                }
            }
        }
    }
}

pub(super) fn set_rdp_view_error(
    session: &mut crate::features::remote_desktop::state::RemoteDesktopSessionState,
    kind: RdpErrorKind,
    message: String,
) {
    let error = RdpError::new(kind, message);
    session.error = Some(error.into());
    session.state = RemoteDesktopViewState::Failed;
}

pub(super) fn set_remote_view_error(
    session: &mut crate::features::remote_desktop::state::RemoteDesktopSessionState,
    error: RemoteDesktopError,
) {
    session.error = Some(error);
    session.state = RemoteDesktopViewState::Failed;
}

pub(super) fn vnc_capabilities_for_state(
    state: &VncSessionState,
    capabilities: Option<VncServerCapabilities>,
) -> Option<VncServerCapabilities> {
    matches!(state, VncSessionState::Connected)
        .then_some(capabilities)
        .flatten()
}

pub(super) fn remote_state_clears_input(state: &RemoteDesktopViewState) -> bool {
    matches!(
        state,
        RemoteDesktopViewState::Reconnecting
            | RemoteDesktopViewState::Disconnecting
            | RemoteDesktopViewState::Disconnected
            | RemoteDesktopViewState::Failed
    )
}

pub(super) fn rdp_view_state(state: &RdpSessionState) -> RemoteDesktopViewState {
    state.into()
}
/// How long before the next remote-desktop maintenance pass.
///
/// A waiting pointer move gets `POINTER_MOVE_INTERVAL`, because that is the interval
/// its own send is budgeted against and a late flush is a visibly late cursor.
/// Everything else is happy on the coarser maintenance cadence.
pub(super) fn remote_desktop_periodic_delay(
    pointer_flush_pending: bool,
    cursor_repaint_pending: bool,
) -> Duration {
    if pointer_flush_pending || cursor_repaint_pending {
        POINTER_MOVE_INTERVAL
    } else {
        MAINTENANCE_INTERVAL
    }
}
