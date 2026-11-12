use std::time::{Duration, Instant};

use futures::{FutureExt as _, StreamExt as _};
use gpui::{Context, Window};
use nyaterm_ui::NyaRoot;

use crate::features::shell::event_pump::helpers::{
    PENDING_SESSION_STILL_CONNECTING_AFTER, PendingSessionAuthWait, RUNTIME_DATA_PLANE_DRAIN_SLOW,
    RuntimeDataPlaneDrain, RuntimeOutputPressureCounts, SLOW_DIAGNOSTIC_THROTTLE,
    TITLE_DRAG_ACTIVE_HOLD, TerminalFrameApplyDecision, connect_settle_active,
    connect_settle_deadline, pending_session_status_message, remote_refresh_should_defer,
    runtime_background_should_defer_terminal_frames, runtime_data_plane_wake_delay,
    runtime_output_pressure_active_from_counts, terminal_cell_metrics_refresh_needed,
    terminal_frame_apply_should_defer, terminal_input_idle_remaining_delay,
    terminal_user_scroll_frame_apply_pending, viewport_change_terminal_session_ids,
    window_geometry_churn_active,
};
use crate::features::{
    NyaTermApp, session::credential_prompt_target, session::keyboard_interactive_prompt_target,
    text_inputs::TextInputSetup,
};
use crate::models::HeaderStatusMode;

mod bridge;
mod helpers;

/// The pressure input recovery accounting is measured against, shared so the recovery
/// clock computes it exactly as the visual plane did.
/// Re-exported for `shell::post_start_work`'s assertion that its retry covers the
/// longest hold it waits out.
#[cfg(test)]
pub(in crate::features::shell) use helpers::CONNECT_SETTLE_HOLD;

pub(in crate::features) fn terminal_performance_pressure(
    app: &NyaTermApp,
    now: std::time::Instant,
) -> bool {
    let output_pressure = app.runtime_output_pressure_active()
        || connect_settle_active(app.shell.runtime.connect_settle_until, now);
    helpers::terminal_render_work_pressure_active(output_pressure, app.session.start_has_pending())
}

pub(super) use helpers::PENDING_SESSION_STATUS_INTERVAL;
mod session_events;

use crate::features::terminal::terminal_runtime::{
    TERMINAL_INPUT_LATENCY_WINDOW, TERMINAL_USER_SCROLL_ACTIVE_WINDOW,
};

// These intervals produce wake deadlines at 4ms, 12ms, and 24ms. The timer
// calls below are sequential, so storing the absolute deadlines here would
// accidentally move the final echo poll out to 40ms.
const TERMINAL_INPUT_WAKE_INTERVALS: [Duration; 3] = [
    Duration::from_millis(4),
    Duration::from_millis(8),
    Duration::from_millis(12),
];

impl NyaTermApp {
    /// Deliver session events and terminal frames as they are produced.
    ///
    /// Started once at window open, replacing the runtime tick's data plane. Before
    /// this the tick polled both queues at 500ms / 50ms / 16ms depending on
    /// `runtime_quiet_tick_allowed`, and the frame queue's own wake was only ever
    /// armed by the input-echo accelerator, so output delivery latency was a
    /// function of the cadence rather than of the output.
    ///
    /// **One task, not two.** The pacing decision between the two drains reads what
    /// the session drain just moved (`session_event_last_output_event_count` and
    /// `..._last_drained_output_bytes`), so splitting them would either duplicate
    /// that state across tasks or lose the tick's output-before-frames order.
    ///
    /// Neither queue can become a channel: the bridge queue merges and trims output
    /// under its byte limit and the frame queue compacts consecutive output, so the
    /// queues stay and only the signal is a channel. See [`crate::models::event_wake`].
    pub(in crate::features) fn start_runtime_data_plane_drain(&mut self, cx: &mut Context<Self>) {
        let Some(session_wake_rx) = self.session.take_event_bridge_wake_receiver() else {
            return;
        };
        let Some(frame_wake_rx) = self.terminal.take_frame_event_wake_receiver() else {
            return;
        };
        // Merged rather than selected on: both mean the same thing to this task --
        // look for work -- and one park point keeps the arm/check ordering simple.
        let mut wake_rx = futures::stream::select(session_wake_rx, frame_wake_rx);
        cx.spawn(async move |this, cx| {
            loop {
                let Ok(drain) = this.update(cx, |this, cx| this.drive_runtime_data_plane_drain(cx))
                else {
                    break;
                };
                let Some(delay) = drain.wake_delay else {
                    if wake_rx.next().await.is_none() {
                        break;
                    }
                    continue;
                };
                // Paced: wait out the delay, but take a wake that lands inside the
                // window rather than leaving it queued to cause an empty cycle
                // after the burst ends.
                let mut timer = cx.background_executor().timer(delay).fuse();
                futures::select_biased! {
                    wake = wake_rx.next() => {
                        if wake.is_none() {
                            break;
                        }
                    }
                    _ = timer => {}
                }
            }
        })
        .detach();
    }

    /// One drain cycle: session events, then the pacing decision, then frames.
    fn drive_runtime_data_plane_drain(&mut self, cx: &mut Context<Self>) -> RuntimeDataPlaneDrain {
        let now = Instant::now();
        // Arm before looking for work. Checking first and arming afterwards loses a
        // producer that pushed in between: the check sees an empty queue, the arm
        // comes too late to make that push signal, and the last entry of a burst
        // then sits unapplied until something unrelated arrives. Arming first can
        // only cost one redundant wake. See `models::event_wake` for the contract.
        self.session.arm_event_bridge_wake();
        self.terminal.arm_frame_event_wakes();

        let session_started_at = Instant::now();
        let session_root_chrome_dirty = self.drain_session_events(cx);
        let session_events = session_started_at.elapsed();

        let decision = self.terminal_frame_apply_decision(now);
        let frames_started_at = Instant::now();
        let frame_root_chrome_dirty = if decision.defer {
            false
        } else {
            self.drain_terminal_frame_events(cx)
        };
        let terminal_frames = frames_started_at.elapsed();

        // Both of these read what the frame drain just applied. Autofill detection
        // scans the active snapshot and is gated on the output backlog being low, and
        // render requests ask for the snapshots a visible session is missing -- so the
        // cycle that applied a frame is exactly when they want to run, rather than a
        // cadence that has to be fast enough to notice.
        let credential_overlay_dirty = self.drain_pending_credential_autofill_detection(cx);
        let _requested_terminal_render_work = self.drive_terminal_render_requests(true);

        // A DECSCUSR arrives as a frame, and a tab becoming visible produces a
        // snapshot, so this cycle sees every change to what the blink clock depends
        // on. Cheap: one bool once the clock is already running.
        self.ensure_cursor_blink_clock(cx);
        // Entering degraded rendering is a consequence of output being applied, so
        // this cycle is where recovery accounting starts needing to run.
        self.ensure_terminal_recovery_clock(cx);

        if session_root_chrome_dirty || frame_root_chrome_dirty || credential_overlay_dirty {
            cx.notify();
        }
        let work_remaining = decision.defer || self.runtime_data_plane_work_remaining();
        let mut wake_delay = runtime_data_plane_wake_delay(
            work_remaining,
            self.session.has_protocol_runtime_sessions(),
        );
        if self.terminal_buffer_search_needs_wake() {
            wake_delay = Some(
                wake_delay
                    .unwrap_or(Duration::from_millis(100))
                    .min(Duration::from_millis(100)),
            );
        }

        let drain = RuntimeDataPlaneDrain {
            wake_delay,
            session_events,
            terminal_frames,
            decision,
        };
        self.maybe_log_slow_runtime_data_plane_drain(&drain);
        drain
    }

    /// Work this task must come back for on its own, because nothing will wake it.
    ///
    /// Read from queue state, never from the drains' return values: those report
    /// chrome dirtiness, and a pure output burst leaves both of them `false`.
    ///
    /// Deliberately excluded are the frame pipeline's queued *commands* and the
    /// bridge's source-side counts. Both mean a worker thread still owes us a push,
    /// and that push signals the interest armed at the top of this cycle, so parking
    /// is correct there -- and that is where the idle polling actually goes away.
    #[cfg(test)]
    fn runtime_data_plane_work_remaining_for_test(&self) -> bool {
        self.runtime_data_plane_work_remaining()
    }

    fn runtime_data_plane_work_remaining(&self) -> bool {
        let frame = self.terminal.frame_queue_metrics();
        !self.session.pending_events_are_empty()
            || self.session.event_bridge_queued_event_count() > 0
            || frame.pending_event_count > 0
            || frame.event_count > 0
            // Detection is marked while output is being processed and is gated on the
            // backlog clearing, so the cycle that marked it usually cannot run it. It
            // clears itself when it does run, so this cannot spin.
            || self.terminal.credential_autofill_detection_is_pending()
    }

    /// Whether frames may be applied this cycle. Lifted out of the runtime tick's
    /// data plane unchanged; both predicates and every interval stay in `helpers`.
    fn terminal_frame_apply_decision(&self, now: Instant) -> TerminalFrameApplyDecision {
        let output_pressure = self.runtime_output_pressure_active();
        let terminal_frame_backlog_active = self.terminal_frame_backlog_active();
        let user_scroll_frame_pending = terminal_user_scroll_frame_apply_pending(
            self.shell.runtime.last_terminal_user_scroll_at,
            self.visible_terminal_session_ids()
                .into_iter()
                .any(|session_id| self.terminal_visual_scroll_active_for_session(Some(session_id))),
            now,
            TERMINAL_USER_SCROLL_ACTIVE_WINDOW,
        );
        let input_latency_active = self
            .shell
            .runtime
            .last_terminal_input_at
            .is_some_and(|last| {
                now.saturating_duration_since(last) < TERMINAL_INPUT_LATENCY_WINDOW
            });
        let paced = terminal_frame_backlog_active
            && terminal_frame_apply_should_defer(
                self.shell.runtime.last_terminal_frame_apply_at,
                now,
                output_pressure,
                user_scroll_frame_pending,
                input_latency_active,
            );
        let deferred_after_output = runtime_background_should_defer_terminal_frames(
            self.shell.runtime.session_event_last_output_event_count,
            self.shell.runtime.session_event_last_drained_output_bytes,
            terminal_frame_backlog_active,
            paced,
            user_scroll_frame_pending,
            input_latency_active,
        );
        TerminalFrameApplyDecision {
            defer: deferred_after_output || paced,
            deferred_after_output,
            paced,
        }
    }

    fn maybe_log_slow_runtime_data_plane_drain(&mut self, drain: &RuntimeDataPlaneDrain) {
        let total = drain.session_events + drain.terminal_frames;
        if total < RUNTIME_DATA_PLANE_DRAIN_SLOW
            || !self.should_log_slow_diagnostic("runtime_data_plane_drain", Instant::now())
        {
            return;
        }
        let frame = self.terminal.frame_queue_metrics();
        tracing::warn!(
            diagnostic = "runtime_data_plane_drain",
            total_ms = total.as_millis(),
            session_events_ms = drain.session_events.as_millis(),
            terminal_frames_ms = drain.terminal_frames.as_millis(),
            terminal_frames_deferred = drain.decision.defer,
            terminal_frames_deferred_after_output = drain.decision.deferred_after_output,
            terminal_frames_deferred_for_pacing = drain.decision.paced,
            wake_delay_ms = drain.wake_delay.map(|delay| delay.as_millis()),
            queued_session_events = self.shell.runtime.session_event_queued_events,
            queued_session_output_bytes = self.shell.runtime.session_event_queued_output_bytes,
            bridge_queued_events = self.session.event_bridge_queued_event_count(),
            bridge_queued_output_bytes = self.session.event_bridge_queued_output_bytes(),
            frame_command_count = frame.command_count,
            frame_event_count = frame.event_count,
            frame_event_wake_count = frame.event_wake_count,
            pending_frame_events = frame.pending_event_count,
            output_pressure = self.runtime_output_pressure_active(),
            "slow runtime data plane drain"
        );
    }

    pub(in crate::features) fn refresh_window_render_inputs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let before_metrics = self.terminal.cell_metrics();
        // Child windows open relative to this window, and GPUI throws away the
        // handle `cx.open_window` returned at startup, so capture it from the
        // first frame that has a `Window` to ask. Set-once, so this is one
        // `is_none` check per frame afterwards.
        if let Some(handle) = window.window_handle().downcast::<NyaRoot>() {
            self.shell.remember_main_window(handle);
        }
        let vs = window.viewport_size();
        let viewport_changed = self
            .shell
            .viewport
            .update_size((f32::from(vs.width), f32::from(vs.height)), Instant::now());
        if viewport_changed {
            // Geometry churn (resize / some window managers during move).
            self.notify_terminal_surfaces_for_viewport_change(cx);
        }
        if terminal_cell_metrics_refresh_needed(self.terminal.cell_metrics()) {
            self.refresh_terminal_cell_metrics(cx);
        }
        if self.terminal.cell_metrics() != before_metrics {
            self.sync_terminal_cell_metrics_to_screens();
            self.resize_all_known_terminal_surfaces();
            self.refresh_visible_terminal_surfaces(cx);
        }
        viewport_changed || self.terminal.cell_metrics() != before_metrics
    }

    fn notify_terminal_surfaces_for_viewport_change(&mut self, cx: &mut Context<Self>) {
        let session_ids =
            viewport_change_terminal_session_ids(&self.visible_terminal_session_ids());
        for session_id in session_ids {
            self.notify_terminal_surface_only(Some(session_id.as_str()), cx);
        }
    }

    pub(in crate::features) fn mark_user_activity(&mut self) {
        self.security.record_screen_lock_user_activity();
    }

    pub(in crate::features) fn arm_terminal_input_wake(&mut self, cx: &mut Context<Self>) {
        self.mark_terminal_input_latency_activity(cx);
        self.shell.runtime.terminal_input_wake_generation = self
            .shell
            .runtime
            .terminal_input_wake_generation
            .saturating_add(1);
        if self.shell.runtime.terminal_input_wake_armed {
            return;
        }
        self.shell.runtime.terminal_input_wake_armed = true;
        // Keep key dispatch limited to encoding and the PTY notifier. Pull
        // through already queued echo after the current input event. The armed
        // state coalesces a burst of keys into one deferred drain.
        let app = cx.entity();
        cx.defer(move |cx| {
            app.update(cx, |this, cx| {
                this.drain_terminal_input_wake(cx);
            });
        });
        let mut observed_generation = self.shell.runtime.terminal_input_wake_generation;
        cx.spawn(async move |this, cx| {
            loop {
                for delay in TERMINAL_INPUT_WAKE_INTERVALS {
                    cx.background_executor().timer(delay).await;
                    let _ = this.update(cx, |this, cx| {
                        this.drain_terminal_input_wake(cx);
                    });
                }
                let (next_generation, finished) = this
                    .update(cx, |this, _| {
                        let next_generation = this.shell.runtime.terminal_input_wake_generation;
                        if next_generation == observed_generation {
                            this.shell.runtime.terminal_input_wake_armed = false;
                            (next_generation, true)
                        } else {
                            (next_generation, false)
                        }
                    })
                    .unwrap_or((observed_generation, true));
                if finished {
                    break;
                }
                observed_generation = next_generation;
            }
        })
        .detach();
    }

    fn drain_terminal_input_wake(&mut self, cx: &mut Context<Self>) {
        let chrome_dirty = self.drain_session_events_for_input_wake(cx)
            | self.drain_terminal_frame_events_for_input_wake(cx);
        if chrome_dirty {
            cx.notify();
        }
    }

    fn mark_terminal_input_latency_activity(&mut self, cx: &mut Context<Self>) {
        self.shell.runtime.last_terminal_input_at = Some(Instant::now());
        if let Some(session_id) = self
            .session
            .active_id()
            .filter(|session_id| !session_id.is_empty())
        {
            self.shell
                .runtime
                .pending_terminal_input_idle_sessions
                .insert(session_id.to_string());
        }
        if self.shell.runtime.terminal_input_idle_notify_armed {
            return;
        }
        self.shell.runtime.terminal_input_idle_notify_armed = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(TERMINAL_INPUT_LATENCY_WINDOW)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.flush_terminal_input_idle_notify(cx);
            });
        })
        .detach();
    }

    fn flush_terminal_input_idle_notify(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        if let Some(delay) = terminal_input_idle_remaining_delay(
            self.shell.runtime.last_terminal_input_at,
            now,
            TERMINAL_INPUT_LATENCY_WINDOW,
        ) {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(delay).await;
                let _ = this.update(cx, |this, cx| {
                    this.flush_terminal_input_idle_notify(cx);
                });
            })
            .detach();
            return;
        }
        self.shell.runtime.terminal_input_idle_notify_armed = false;
        let session_ids = self
            .shell
            .runtime
            .pending_terminal_input_idle_sessions
            .drain()
            .collect::<Vec<_>>();
        for session_id in session_ids {
            self.notify_terminal_surface_only(Some(session_id.as_str()), cx);
        }
    }

    pub(in crate::features) fn mark_title_drag_activity(&mut self) {
        self.shell
            .viewport
            .mark_title_drag(Instant::now(), TITLE_DRAG_ACTIVE_HOLD);
    }

    pub(in crate::features) fn title_drag_active(&self, now: Instant) -> bool {
        self.shell.viewport.title_drag_active(now)
    }

    /// Window move or resize churn, including the title-drag hold.
    ///
    /// Exposed for owners outside this module that must not open the config
    /// database while the compositor is moving the window.
    pub(in crate::features) fn shell_persistence_geometry_is_busy(&self, now: Instant) -> bool {
        self.title_drag_active(now)
            || window_geometry_churn_active(self.shell.viewport.last_change_at, now)
    }

    pub(in crate::features) fn connect_settle_is_active(&self, now: Instant) -> bool {
        connect_settle_active(self.shell.runtime.connect_settle_until, now)
    }

    pub(in crate::features) fn should_log_slow_diagnostic(
        &mut self,
        key: &'static str,
        now: Instant,
    ) -> bool {
        self.shell
            .diagnostics
            .should_log(key, now, SLOW_DIAGNOSTIC_THROTTLE)
    }

    pub(in crate::features) fn lock_screen_for_idle(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.cancel_file_sudo_prompt(cx);
        self.hide_secret_inputs(cx);
        self.security.remember_screen_lock_focus(window.focused(cx));
        self.security.activate_idle_screen_lock();
        self.forget_text_inputs("security.unlock.password");
        self.forget_text_inputs("security.editor.");
        self.connection_state.clear_editor_secrets(cx);
        nyaterm_ui::NyaDialogWindowExt::close_all_nya_dialogs(window, cx);
        self.forget_text_inputs("lock-screen.password");
        self.shell.set_status(format!(
            "screen locked after {} minute(s) idle",
            self.settings.summary().idle_lock_minutes
        ));
        if self.settings.summary().has_master_password {
            let field = self.text_input("lock-screen.password", "", TextInputSetup::masked(), cx);
            window.focus(&field.read(cx).focus_handle(), cx);
        } else {
            window.focus(self.security.screen_lock_focus(), cx);
        }
        true
    }

    pub(in crate::features) fn visible_terminal_performance_recovery_due(&self) -> bool {
        self.terminal
            .visible_performance_recovery_due(self.visible_terminal_session_ids())
    }

    pub(in crate::features) fn enter_connect_settle(&mut self) {
        self.shell.runtime.connect_settle_until = Some(connect_settle_deadline(Instant::now()));
    }

    pub(in crate::features) fn runtime_output_pressure_active(&self) -> bool {
        let frame = self.terminal.frame_queue_metrics();
        runtime_output_pressure_active_from_counts(RuntimeOutputPressureCounts {
            session_event_backlog_active: self.shell.runtime.session_event_backlog_active,
            session_event_queued_output_bytes: self.shell.runtime.session_event_queued_output_bytes,
            pending_session_events: self.session.pending_event_count(),
            bridge_queued_events: self.session.event_bridge_queued_event_count()
                + self.session.event_bridge_source_queued_event_count(),
            bridge_queued_output_bytes: self.session.event_bridge_queued_output_bytes()
                + self.session.event_bridge_source_queued_output_bytes(),
            pending_terminal_frame_events: frame.pending_event_count,
            queued_terminal_frame_events: frame.event_count,
            queued_terminal_frame_output_bytes: frame.output_bytes,
        })
    }

    pub(in crate::features) fn drive_pending_session_status(&mut self) -> bool {
        let Some((name, requested_at)) = self.session.start_pending_status_source() else {
            self.shell.runtime.last_pending_session_status_at = None;
            return false;
        };
        let auth_wait = self.pending_session_auth_wait();
        if auth_wait.is_none() && requested_at.elapsed() < PENDING_SESSION_STILL_CONNECTING_AFTER {
            return false;
        }
        let now = Instant::now();
        if self
            .shell
            .runtime
            .last_pending_session_status_at
            .is_some_and(|last_at| {
                now.saturating_duration_since(last_at) < PENDING_SESSION_STATUS_INTERVAL
            })
        {
            return false;
        }
        self.shell.runtime.last_pending_session_status_at = Some(now);
        let message = pending_session_status_message(&name, auth_wait.as_ref());
        if self.shell.status() == message {
            return false;
        }
        self.shell.set_status(message);
        true
    }

    /// Whether a due remote refresh must wait for the app to fall calm.
    ///
    /// Reads the same three holds the idle plane read, from the same places, so this
    /// is the pre-`15994ef6` behaviour restored rather than a new policy.
    pub(in crate::features) fn remote_refresh_is_deferred(&self) -> bool {
        let now = Instant::now();
        remote_refresh_should_defer(
            self.runtime_output_pressure_active(),
            window_geometry_churn_active(self.shell.viewport.last_change_at, now),
            connect_settle_active(self.shell.runtime.connect_settle_until, now),
        )
    }

    pub(in crate::features) fn header_status_needs_stats(&self) -> bool {
        self.settings.summary().ui_header_status_visible
            && matches!(
                HeaderStatusMode::from_setting(&self.settings.summary().ui_header_status_mode),
                HeaderStatusMode::Resources | HeaderStatusMode::Host
            )
    }

    pub(in crate::features) fn header_status_needs_gpu(&self) -> bool {
        self.settings.summary().ui_header_status_visible
            && HeaderStatusMode::from_setting(&self.settings.summary().ui_header_status_mode)
                == HeaderStatusMode::Gpu
    }

    pub(in crate::features) fn header_status_needs_npu(&self) -> bool {
        self.settings.summary().ui_header_status_visible
            && HeaderStatusMode::from_setting(&self.settings.summary().ui_header_status_mode)
                == HeaderStatusMode::Npu
    }

    fn pending_session_auth_wait(&self) -> Option<PendingSessionAuthWait> {
        if let Some(prompt) = self.session.prompt_active_agent() {
            return Some(PendingSessionAuthWait::Agent {
                target: format!(
                    "{}@{}:{}",
                    prompt.prompt.username, prompt.prompt.host, prompt.prompt.port
                ),
            });
        }
        if let Some(prompt) = self.session.prompt_active_keyboard_interactive() {
            return Some(PendingSessionAuthWait::Credential {
                target: keyboard_interactive_prompt_target(&prompt.request),
            });
        }
        if let Some(prompt) = self.session.prompt_active_credential() {
            return Some(PendingSessionAuthWait::Credential {
                target: credential_prompt_target(&prompt.prompt),
            });
        }
        if let Some(prompt) = self.session.prompt_active_host_key() {
            return Some(PendingSessionAuthWait::HostKey {
                host: prompt.host_key.host_identifier.clone(),
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use gpui::{AppContext as _, Context, Subscription, TestAppContext};
    use nyaterm_core::{AiExecutionProfile, AppRuntime, RuntimeMode, uuid};
    use nyaterm_transport::{LocalSessionConfig, SessionEvent};

    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::models::{
        GithubGistAuthEvent, GithubGistAuthJobEvent, RecordingHistorySearchEvent,
        RecordingWriteEvent, SessionLaunchConfig, SessionRuntimeMetadata, TerminalSearchMode,
    };

    use super::helpers::{
        SESSION_EVENT_DRAIN_CONNECT_SETTLE_OUTPUT_BUDGET, SESSION_EVENT_DRAIN_IDLE_OUTPUT_BUDGET,
        TERMINAL_FRAME_APPLY_PRESSURE_INTERVAL,
    };

    const SESSION_ID: &str = "event-pump-session";

    struct RootNotifyObserver {
        count: usize,
        _subscription: Subscription,
    }

    impl RootNotifyObserver {
        fn new(app: gpui::Entity<NyaTermApp>, cx: &mut Context<Self>) -> Self {
            let subscription = cx.observe(&app, |observer, _, _| {
                observer.count += 1;
            });
            Self {
                count: 0,
                _subscription: subscription,
            }
        }
    }

    fn unique_test_dir() -> PathBuf {
        // A uuid rather than a clock reading: these tests run in parallel and
        // Windows' ~15ms clock granularity lets a nanosecond timestamp repeat,
        // which would share one config dir and so one settings database.
        std::env::temp_dir().join(format!(
            "nyaterm-event-pump-{}-{}",
            std::process::id(),
            uuid()
        ))
    }

    /// A workspace with one visible local session and nothing outstanding.
    fn quiet_app_with_visible_session(cx: &mut TestAppContext) -> gpui::Entity<NyaTermApp> {
        let root = unique_test_dir();
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.clone(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        let app = cx.new(|cx| NyaTermApp::new(runtime, stores, cx));
        cx.update_entity(&app, |app, _| {
            app.session.register_session_metadata(
                SESSION_ID,
                SessionRuntimeMetadata {
                    ssh_config: None,
                    ssh_multiplex_key: None,
                    source_connection_id: None,
                    ai_execution_profile: AiExecutionProfile::Posix,
                    launch_config: SessionLaunchConfig::Local(LocalSessionConfig {
                        name: "Local session".to_string(),
                        ..LocalSessionConfig::default()
                    }),
                    disconnected: false,
                },
            );
            app.session.select_active_session(SESSION_ID);
            app.terminal
                .seed_session_view(SESSION_ID.to_string(), String::new(), "UTF-8");
            app.shell.show_workspace();
            // A fresh app starts with the terminal window layout restore
            // outstanding; complete it so these tests start from a settled app.
            app.shell.complete_terminal_windows_restore();
        });
        app
    }

    fn observe_root_notifies(
        app: gpui::Entity<NyaTermApp>,
        cx: &mut TestAppContext,
    ) -> gpui::Entity<RootNotifyObserver> {
        cx.new(|cx| RootNotifyObserver::new(app, cx))
    }

    fn root_notify_count(
        observer: &gpui::Entity<RootNotifyObserver>,
        cx: &mut TestAppContext,
    ) -> usize {
        cx.update_entity(observer, |observer, _| observer.count)
    }

    #[test]
    fn terminal_output_does_not_require_full_shell_repaint() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = quiet_app_with_visible_session(&mut cx);
        let observer = observe_root_notifies(app.clone(), &mut cx);
        cx.update_entity(&app, |app, cx| {
            app.start_runtime_data_plane_drain(cx);
            app.session
                .push_event_bridge_ui_event_for_test(SessionEvent::Output {
                    session_id: SESSION_ID.to_string(),
                    data: b"ordinary terminal output\r\n".to_vec(),
                });
        });
        cx.run_until_parked();

        assert_eq!(
            root_notify_count(&observer, &mut cx),
            0,
            "ordinary terminal output must not notify the full shell"
        );
    }

    #[test]
    fn root_status_change_still_repaints_root() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = quiet_app_with_visible_session(&mut cx);
        let observer = observe_root_notifies(app.clone(), &mut cx);
        cx.update_entity(&app, |app, cx| {
            app.start_runtime_data_plane_drain(cx);
            app.session
                .push_event_bridge_ui_event_for_test(SessionEvent::OutputDropped {
                    session_id: SESSION_ID.to_string(),
                    bytes: 1024,
                });
        });
        cx.run_until_parked();

        cx.update_entity(&app, |app, _| {
            assert!(
                app.shell.status().contains("terminal output overloaded"),
                "active-session drop status is root chrome"
            );
        });
        assert!(
            root_notify_count(&observer, &mut cx) > 0,
            "root chrome status changes must still notify the full shell"
        );
    }

    #[test]
    fn data_plane_has_no_pending_ui_repaint_work() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = quiet_app_with_visible_session(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.start_runtime_data_plane_drain(cx);
            app.session
                .push_event_bridge_ui_event_for_test(SessionEvent::Output {
                    session_id: SESSION_ID.to_string(),
                    data: b"plain data-plane output\r\n".to_vec(),
                });
        });
        cx.run_until_parked();

        cx.update_entity(&app, |app, _| {
            assert!(
                !app.runtime_data_plane_work_remaining_for_test(),
                "plain output must not leave repaint-only work behind after the data plane drains"
            );
        });
    }

    /// A pending credential-prompt detection must not be stranded by the task parking.
    ///
    /// Detection is *marked* while output is being processed but is gated on the output
    /// backlog clearing, so the cycle that marks it usually cannot also run it. If the
    /// burst then ends, nothing further wakes the task -- which is the same failure
    /// shape as a stranded burst tail, and why this flag is one of the things
    /// `runtime_data_plane_work_remaining` reports.
    #[test]
    fn a_pending_credential_detection_keeps_the_drain_task_coming_back() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = quiet_app_with_visible_session(&mut cx);
        cx.update_entity(&app, |app, _| {
            assert!(
                !app.terminal.credential_autofill_detection_is_pending(),
                "the fixture must start with nothing outstanding"
            );
            app.terminal.mark_credential_autofill_detection_for_test();
            assert!(
                app.runtime_data_plane_work_remaining_for_test(),
                "a marked detection must keep the task on its paced re-arm rather than                  letting it park with the detection never run"
            );
        });
    }

    /// The tail of a burst is the case that breaks if the wake is armed once
    /// outside the drain loop instead of before every check.
    ///
    /// The first push finds the interest armed and delivers. The task then drains,
    /// re-arms, and parks. A second push must find the interest armed *again* --
    /// `EventWake::signal` clears it on every delivery -- or it signals nothing,
    /// nothing further arrives, and that entry sits in the queue forever. On a
    /// terminal that is the last screenful of output after a flood stops.
    ///
    /// `run_until_parked` does not advance the clock, so no timer can rescue this.
    #[test]
    fn the_tail_of_a_burst_is_applied_after_the_task_parks() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = quiet_app_with_visible_session(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.start_runtime_data_plane_drain(cx);
            for index in 0..8 {
                app.session
                    .push_event_bridge_ui_event_for_test(SessionEvent::Output {
                        session_id: SESSION_ID.to_string(),
                        data: format!("line {index}\r\n").into_bytes(),
                    });
            }
            app.session
                .push_event_bridge_ui_event_for_test(SessionEvent::CwdChanged {
                    session_id: SESSION_ID.to_string(),
                    cwd: "/srv/first".to_string(),
                });
        });
        cx.run_until_parked();
        cx.update_entity(&app, |app, _| {
            assert_eq!(
                app.session.cwd(SESSION_ID),
                Some("/srv/first"),
                "the first burst should be fully applied"
            );
        });

        // The task is parked now. This push is the one that needs a live interest.
        cx.update_entity(&app, |app, _| {
            app.session
                .push_event_bridge_ui_event_for_test(SessionEvent::CwdChanged {
                    session_id: SESSION_ID.to_string(),
                    cwd: "/srv/tail".to_string(),
                });
        });
        cx.run_until_parked();
        cx.update_entity(&app, |app, _| {
            assert_eq!(
                app.session.cwd(SESSION_ID),
                Some("/srv/tail"),
                "a push arriving while the task is parked must still wake it; \
                 arming once outside the loop strands this entry"
            );
        });
    }

    /// The other route to a stranded tail: the drain hit a budget rather than the
    /// end of the queue, so no further push is coming to wake anyone.
    ///
    /// Queue more than the idle budget and enter connect settling explicitly:
    /// an asynchronously arriving first frame can otherwise change the budget
    /// mid-test. The task must notice and come back on
    /// `TERMINAL_FRAME_APPLY_PRESSURE_INTERVAL` -- this is the pacing the runtime
    /// tick used to provide, which this change keeps rather than removes. If
    /// `runtime_data_plane_wake_delay` returned `None` while work remained, the
    /// clock advance below would change nothing.
    #[test]
    fn a_drain_cut_short_by_its_budget_comes_back_on_the_pacing_interval() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = quiet_app_with_visible_session(&mut cx);
        let output_bytes = 2 * SESSION_EVENT_DRAIN_IDLE_OUTPUT_BUDGET;
        cx.update_entity(&app, |app, cx| {
            app.enter_connect_settle();
            app.start_runtime_data_plane_drain(cx);
            // Twice the idle output budget, so one cycle cannot reach the tail.
            let chunk = vec![b'x'; SESSION_EVENT_DRAIN_IDLE_OUTPUT_BUDGET / 2];
            for _ in 0..4 {
                app.session
                    .push_event_bridge_ui_event_for_test(SessionEvent::Output {
                        session_id: SESSION_ID.to_string(),
                        data: chunk.clone(),
                    });
            }
            app.session
                .push_event_bridge_ui_event_for_test(SessionEvent::CwdChanged {
                    session_id: SESSION_ID.to_string(),
                    cwd: "/srv/beyond-the-budget".to_string(),
                });
        });
        cx.run_until_parked();
        cx.update_entity(&app, |app, _| {
            assert_ne!(
                app.session.cwd(SESSION_ID),
                Some("/srv/beyond-the-budget"),
                "the output budget should have stopped this cycle short of the tail, \
                 or this test is not exercising the paced path"
            );
        });

        // Nothing will push again. Only the task's own re-arm can finish the queue.
        // Settling drains only 4 KiB per cycle; advancing GPUI's clock does not
        // expire the real-time settling window. Leave one cycle for the tail.
        let paced_cycles =
            output_bytes.div_ceil(SESSION_EVENT_DRAIN_CONNECT_SETTLE_OUTPUT_BUDGET) + 1;
        for _ in 0..paced_cycles {
            cx.executor()
                .advance_clock(TERMINAL_FRAME_APPLY_PRESSURE_INTERVAL);
            cx.run_until_parked();
        }
        cx.update_entity(&app, |app, _| {
            assert_eq!(
                app.session.cwd(SESSION_ID),
                Some("/srv/beyond-the-budget"),
                "a queue left non-empty by a budget must be finished by the paced re-arm"
            );
        });
    }

    /// The replacement for the quiet-gate term this queue used to need.
    ///
    /// No window runtime tick exists in this fixture at all, and
    /// `run_until_parked` advances the executor without advancing the clock, so
    /// the event can only have arrived through the drain task. That is what makes
    /// the old 500ms trap structurally impossible here: a queue with its own
    /// drain task needs no central predicate to stay responsive.
    #[test]
    fn github_gist_auth_events_arrive_without_any_timer() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = quiet_app_with_visible_session(&mut cx);
        let (tx, job_id) = cx.update_entity(&app, |app, cx| {
            let start = app
                .cloud_sync
                .begin_github_auth_for_test()
                .expect("the device flow should start");
            let handles = (start.sender(), start.job_id());
            app.start_github_gist_auth_event_drain(cx);
            handles
        });

        tx.unbounded_send(GithubGistAuthJobEvent {
            job_id,
            event: GithubGistAuthEvent::Polling { slow_down: true },
        })
        .expect("the drain task holds the receiver");
        cx.run_until_parked();

        cx.update_entity(&app, |app, _| {
            assert_eq!(
                app.cloud_sync.github_auth().message.as_deref(),
                Some(rust_i18n::t!("settings.githubGistSlowDown").as_ref()),
                "the polling event should already be applied"
            );
        });
    }

    /// The recording-history reply path, which used to lean on a quiet-gate term.
    /// Delivery wiring is covered by the gist test above; what is specific here is
    /// which replies get applied.
    #[test]
    fn recording_history_replies_apply_only_for_the_outstanding_query() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = quiet_app_with_visible_session(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.terminal.open_search_for_test();
            app.terminal.set_search_mode(TerminalSearchMode::History);
            app.apply_terminal_search_query("needle".to_string(), cx);
            let key = app
                .terminal_history_search_key()
                .expect("the fixture should arm a history search");

            let mut stale_key = key.clone();
            stale_key.query = "haystack".to_string();
            assert!(
                !app.apply_recording_write_event(RecordingWriteEvent::HistorySearch(
                    RecordingHistorySearchEvent {
                        key: stale_key,
                        result: Err("stale".to_string()),
                    }
                )),
                "a reply for a query the user has moved on from must be dropped"
            );
            assert!(app.terminal_history_search_pending_for_current_query());

            assert!(
                app.apply_recording_write_event(RecordingWriteEvent::HistorySearch(
                    RecordingHistorySearchEvent {
                        key,
                        result: Err("no recordings".to_string()),
                    }
                )),
                "the outstanding query's reply must settle it"
            );
            assert!(!app.terminal_history_search_pending_for_current_query());
        });
    }
}
