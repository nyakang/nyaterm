use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use futures::channel::mpsc::UnboundedReceiver;
use nyaterm_transport::{
    SessionDrainStats, SessionEvent, SessionEventConsumerId, SessionManager, TrzszDetector,
    ZmodemDetector,
};

use super::event_wake::{ANY_INTEREST, EventWake};
use super::{RecordingWriteHandle, TerminalFrameOutputSubmission, TerminalFramePipeline};

const SESSION_EVENT_BRIDGE_DRAIN_BATCH: usize = 512;
const SESSION_EVENT_BRIDGE_OUTPUT_BUDGET: usize = 128 * 1024;
const SESSION_EVENT_BRIDGE_IDLE_SLEEP: Duration = Duration::from_millis(4);
/// Upper bound on one park in the source drain. The queue wakes this thread as
/// soon as a PTY read lands, so the timeout only bounds how long a fully idle
/// bridge waits before re-checking its stop flag.
const SESSION_EVENT_BRIDGE_WAIT_TIMEOUT: Duration = Duration::from_millis(50);
const SESSION_EVENT_BRIDGE_BUSY_SLEEP: Duration = Duration::from_millis(1);
const SESSION_EVENT_BRIDGE_UI_OUTPUT_LIMIT: usize = 1024 * 1024;
const SESSION_EVENT_BRIDGE_UI_OUTPUT_LOW_WATERMARK: usize =
    SESSION_EVENT_BRIDGE_UI_OUTPUT_LIMIT / 2;
const SESSION_EVENT_BRIDGE_DIRECT_OUTPUT_BACKPRESSURE: usize = 2 * 1024 * 1024;
const SESSION_EVENT_BRIDGE_DIRECT_OUTPUT_LOW_WATERMARK: usize =
    SESSION_EVENT_BRIDGE_DIRECT_OUTPUT_BACKPRESSURE / 2;
const SESSION_EVENT_BRIDGE_SIDEBAND_PROBE_EVENTS: usize = 4;
const SESSION_EVENT_BRIDGE_SIDEBAND_PROBE_WINDOW: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, Default)]
pub(crate) struct SessionEventBridgeStats {
    pub(crate) direct_output_events: u64,
    pub(crate) direct_output_bytes: u64,
    pub(crate) direct_backpressure_events: u64,
    pub(crate) direct_backpressure_bytes: u64,
    pub(crate) drained_ui_events: usize,
    pub(crate) drained_ui_output_bytes: usize,
    pub(crate) ui_queued_events: usize,
    pub(crate) ui_queued_output_bytes: usize,
    pub(crate) source_queued_events: usize,
    pub(crate) source_queued_output_bytes: usize,
    pub(crate) dropped_output_bytes: usize,
}

#[derive(Debug, Default)]
pub(crate) struct SessionEventBridgeDrain {
    pub(crate) events: Vec<SessionEvent>,
    pub(crate) stats: SessionEventBridgeStats,
}

pub(crate) struct SessionEventBridge {
    state: Arc<SessionEventBridgeState>,
    session_manager: Arc<SessionManager>,
    consumer_id: SessionEventConsumerId,
    worker: Option<thread::JoinHandle<()>>,
}

struct SessionEventBridgeState {
    control: Mutex<SessionEventBridgeControl>,
    transfer_gate: Mutex<()>,
    ui_queue: SessionEventBridgeQueue,
    /// Handed to `NyaTermApp::start_runtime_data_plane_drain` once, at window open.
    ui_queue_wake_rx: Mutex<Option<UnboundedReceiver<()>>>,
    source_queued_events: AtomicUsize,
    source_queued_output_bytes: AtomicUsize,
    direct_output_events: AtomicU64,
    direct_output_bytes: AtomicU64,
    direct_backpressure_events: AtomicU64,
    direct_backpressure_bytes: AtomicU64,
    stop: AtomicBool,
}

#[derive(Default)]
struct SessionEventBridgeControl {
    owned_sessions: HashSet<String>,
    protocol_consumers: HashMap<String, SessionEventConsumerId>,
    ui_routed_sessions: HashSet<String>,
    /// A UI handoff remains ordered until the UI has submitted its whole drain
    /// to the frame worker, including events already removed from `ui_queue`.
    pending_ui_sessions: HashSet<String>,
    encoding: String,
    // Read-only projection of launch metadata; default changes cannot alter live streams.
    session_encodings: HashMap<String, String>,
    scrollback_limit: usize,
    source_queued_events: usize,
    source_queued_output_bytes: usize,
}

#[derive(Clone, Copy, Debug)]
struct SessionEventBridgeSidebandProbe {
    events_remaining: usize,
    expires_at: Instant,
}

#[derive(Clone)]
struct SessionEventBridgeQueue {
    inner: Arc<Mutex<SessionEventBridgeQueueInner>>,
    /// Signalled after every push. `None` only in the queue's own unit tests.
    wake: Option<EventWake>,
}

#[derive(Default)]
struct SessionEventBridgeQueueInner {
    events: VecDeque<SessionEvent>,
    queued_output_bytes: usize,
}

impl SessionEventBridge {
    pub(crate) fn spawn(
        session_manager: Arc<SessionManager>,
        frame_pipeline: TerminalFramePipeline,
        recording_writer: Option<RecordingWriteHandle>,
        encoding: String,
        scrollback_limit: usize,
    ) -> Self {
        let (ui_queue, ui_queue_wake_rx) = SessionEventBridgeQueue::new_with_wake();
        let consumer_id = session_manager.register_event_consumer();
        let state = Arc::new(SessionEventBridgeState {
            control: Mutex::new(SessionEventBridgeControl {
                owned_sessions: HashSet::new(),
                protocol_consumers: HashMap::new(),
                ui_routed_sessions: HashSet::new(),
                pending_ui_sessions: HashSet::new(),
                encoding,
                session_encodings: HashMap::new(),
                scrollback_limit,
                source_queued_events: 0,
                source_queued_output_bytes: 0,
            }),
            transfer_gate: Mutex::new(()),
            ui_queue,
            ui_queue_wake_rx: Mutex::new(Some(ui_queue_wake_rx)),
            source_queued_events: AtomicUsize::new(0),
            source_queued_output_bytes: AtomicUsize::new(0),
            direct_output_events: AtomicU64::new(0),
            direct_output_bytes: AtomicU64::new(0),
            direct_backpressure_events: AtomicU64::new(0),
            direct_backpressure_bytes: AtomicU64::new(0),
            stop: AtomicBool::new(false),
        });
        let worker_state = state.clone();
        let worker_manager = Arc::clone(&session_manager);
        let worker = thread::Builder::new()
            .name("nyaterm-session-event-bridge".to_string())
            .spawn(move || {
                run_session_event_bridge(
                    worker_manager,
                    consumer_id,
                    frame_pipeline,
                    recording_writer,
                    worker_state,
                )
            })
            .expect("failed to spawn session event bridge");
        Self {
            state,
            session_manager,
            consumer_id,
            worker: Some(worker),
        }
    }

    pub(crate) fn claim_session(&self, session_id: &str) {
        if session_id.is_empty() {
            return;
        }
        let mut consumer = self.consumer_id;
        if let Ok(mut control) = self.state.control.lock() {
            control.owned_sessions.insert(session_id.to_string());
            consumer = control
                .protocol_consumers
                .get(session_id)
                .copied()
                .unwrap_or(consumer);
        }
        self.session_manager
            .assign_session_event_consumer(session_id, consumer);
    }

    pub(crate) fn protocol_wake(&self) -> nyaterm_transport::zmodem::runtime::RuntimeWake {
        let wake = self.state.ui_queue.wake.clone();
        Arc::new(move || {
            if let Some(wake) = &wake {
                wake.signal(ANY_INTEREST);
            }
        })
    }

    pub(crate) fn bind_protocol_consumer(
        &self,
        session_id: &str,
        consumer: SessionEventConsumerId,
    ) {
        self.state
            .control
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .protocol_consumers
            .insert(session_id.into(), consumer);
    }

    /// Fence the bridge's complete dequeue/routing turn before transferring input.
    pub(crate) fn begin_protocol_handoff(
        &self,
        session_id: &str,
        consumer: SessionEventConsumerId,
    ) -> VecDeque<SessionEvent> {
        let _gate = self
            .state
            .transfer_gate
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        self.bind_protocol_consumer(session_id, consumer);
        self.session_manager
            .assign_session_event_consumer(session_id, consumer);
        self.state.ui_queue.take_sessions(&[session_id.to_string()])
    }

    pub(crate) fn finish_protocol_handoff(&self, session_id: &str) {
        let _gate = self
            .state
            .transfer_gate
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let mut control = self.state.control.lock().unwrap_or_else(|p| p.into_inner());
        control.protocol_consumers.remove(session_id);
        control.pending_ui_sessions.insert(session_id.into());
        drop(control);
        self.session_manager
            .assign_session_event_consumer(session_id, self.consumer_id);
    }

    pub(crate) fn release_session(&self, session_id: &str) {
        if let Ok(mut control) = self.state.control.lock() {
            control.owned_sessions.remove(session_id);
            control.protocol_consumers.remove(session_id);
            control.ui_routed_sessions.remove(session_id);
            control.pending_ui_sessions.remove(session_id);
        }
        self.session_manager
            .clear_session_event_consumer(session_id, self.consumer_id);
    }

    /// Stop this bridge from consuming the selected sessions after all work
    /// already being processed by its worker has completed. Events produced
    /// after this returns remain queued in the shared manager until another
    /// workspace claims the sessions.
    pub(crate) fn pause_sessions_for_transfer(
        &self,
        session_ids: &[String],
    ) -> VecDeque<SessionEvent> {
        let Ok(_gate) = self.state.transfer_gate.lock() else {
            return VecDeque::new();
        };
        if let Ok(mut control) = self.state.control.lock() {
            for session_id in session_ids {
                control.owned_sessions.remove(session_id);
                control.protocol_consumers.remove(session_id);
                control.ui_routed_sessions.remove(session_id);
                control.pending_ui_sessions.remove(session_id);
            }
        }
        for session_id in session_ids {
            self.session_manager
                .clear_session_event_consumer(session_id, self.consumer_id);
        }
        self.state.ui_queue.take_sessions(session_ids)
    }

    /// Taken once, by the drain task that consumes this queue.
    pub(crate) fn take_ui_queue_wake_receiver(&self) -> Option<UnboundedReceiver<()>> {
        self.state.ui_queue_wake_rx.lock().ok()?.take()
    }

    /// Declare interest in the next UI-queue push. Call before draining; see
    /// [`crate::models::event_wake`] for why the other order loses wakes.
    pub(crate) fn arm_ui_queue_wake(&self) {
        self.state.ui_queue.arm_wake();
    }

    /// Enqueue as the bridge worker thread would, so a test can exercise the wake
    /// and the drain without a live PTY.
    #[cfg(test)]
    pub(crate) fn push_ui_event_for_test(&self, event: SessionEvent) {
        if let Ok(mut control) = self.state.control.lock() {
            self.state.push_ui_event(&mut control, event);
        }
    }

    #[cfg(test)]
    pub(crate) fn session_requires_ui_for_test(&self, session_id: &str) -> bool {
        let control = self.state.control.lock().unwrap();
        control.ui_routed_sessions.contains(session_id)
            || control.pending_ui_sessions.contains(session_id)
    }

    pub(crate) fn configure(
        &self,
        encoding: String,
        scrollback_limit: usize,
        session_encodings: HashMap<String, String>,
    ) {
        let Ok(mut control) = self.state.control.lock() else {
            return;
        };
        if control.encoding == encoding
            && control.scrollback_limit == scrollback_limit
            && control.session_encodings == session_encodings
        {
            return;
        }
        control.encoding = encoding;
        control.session_encodings = session_encodings;
        control.scrollback_limit = scrollback_limit;
    }

    pub(crate) fn route_session_to_ui(&self, session_id: &str) {
        if session_id.is_empty() {
            return;
        }
        if let Ok(mut control) = self.state.control.lock() {
            if control.ui_routed_sessions.contains(session_id) {
                return;
            }
            control.ui_routed_sessions.insert(session_id.to_string());
        }
    }

    pub(crate) fn begin_output_boundary(&self, session_id: &str, token: &str) {
        // The worker holds this gate through direct-frame submission, so its
        // previous batch cannot overtake the boundary or the synthetic header.
        let Ok(_gate) = self.state.transfer_gate.lock() else {
            return;
        };
        let Ok(mut control) = self.state.control.lock() else {
            return;
        };
        control.ui_routed_sessions.insert(session_id.to_string());
        // Use the source queue: bytes not yet dequeued by the worker must also
        // precede the fence, including a backlog larger than one worker batch.
        self.session_manager
            .enqueue_output_boundary(session_id, token);
    }

    pub(crate) fn resume_session_direct_output(&self, session_id: &str) {
        if let Ok(mut control) = self.state.control.lock() {
            control.ui_routed_sessions.remove(session_id);
        }
    }

    /// Install the ordering boundary before a transferred session is claimed.
    pub(crate) fn hold_session_for_ui(&self, session_id: &str) {
        if let Ok(mut control) = self.state.control.lock() {
            control.pending_ui_sessions.insert(session_id.to_string());
        }
    }

    /// Called only after the UI has submitted all accepted output in its drain.
    /// Feature routing still protects detector-held bytes and active AI capture.
    pub(crate) fn complete_ui_drain(&self, pending_sessions: &HashSet<String>) {
        self.state.complete_ui_drain(pending_sessions);
    }

    pub(crate) fn clear_session(&self, session_id: &str) {
        self.resume_session_direct_output(session_id);
    }

    pub(crate) fn drain_events_with_output_budget(
        &self,
        max_events: usize,
        max_output_bytes: usize,
    ) -> SessionEventBridgeDrain {
        let mut drain = self
            .state
            .ui_queue
            .drain_with_output_budget(max_events, max_output_bytes);
        drain.stats.direct_output_events =
            self.state.direct_output_events.swap(0, Ordering::Relaxed);
        drain.stats.direct_output_bytes = self.state.direct_output_bytes.swap(0, Ordering::Relaxed);
        drain.stats.direct_backpressure_events = self
            .state
            .direct_backpressure_events
            .swap(0, Ordering::Relaxed);
        drain.stats.direct_backpressure_bytes = self
            .state
            .direct_backpressure_bytes
            .swap(0, Ordering::Relaxed);
        drain.stats.source_queued_events = self.state.source_queued_events.load(Ordering::Relaxed);
        drain.stats.source_queued_output_bytes = self
            .state
            .source_queued_output_bytes
            .load(Ordering::Relaxed);
        drain
    }

    pub(crate) fn queued_event_count(&self) -> usize {
        self.state.ui_queue.len()
    }

    pub(crate) fn queued_output_bytes(&self) -> usize {
        self.state.ui_queue.queued_output_bytes()
    }

    pub(crate) fn source_queued_event_count(&self) -> usize {
        self.state.source_queued_events.load(Ordering::Relaxed)
    }

    pub(crate) fn source_queued_output_bytes(&self) -> usize {
        self.state
            .source_queued_output_bytes
            .load(Ordering::Relaxed)
    }

    pub(crate) fn has_pending_ui_work(&self) -> bool {
        self.queued_event_count() > 0
            || self.source_queued_event_count() > 0
            || self.source_queued_output_bytes() > 0
    }

    /// Harvest direct/source counters without locking the UI queue.
    /// Used on idle ticks when there is no UI-routed work to drain.
    pub(crate) fn harvest_direct_stats(&self) -> SessionEventBridgeStats {
        SessionEventBridgeStats {
            direct_output_events: self.state.direct_output_events.swap(0, Ordering::Relaxed),
            direct_output_bytes: self.state.direct_output_bytes.swap(0, Ordering::Relaxed),
            direct_backpressure_events: self
                .state
                .direct_backpressure_events
                .swap(0, Ordering::Relaxed),
            direct_backpressure_bytes: self
                .state
                .direct_backpressure_bytes
                .swap(0, Ordering::Relaxed),
            drained_ui_events: 0,
            drained_ui_output_bytes: 0,
            ui_queued_events: self.queued_event_count(),
            ui_queued_output_bytes: self.queued_output_bytes(),
            source_queued_events: self.source_queued_event_count(),
            source_queued_output_bytes: self.source_queued_output_bytes(),
            dropped_output_bytes: 0,
        }
    }

    pub(crate) fn take_shutdown(&mut self) -> impl FnOnce() + Send + 'static {
        self.state.stop.store(true, Ordering::Release);
        let worker = self.worker.take();
        let state = Arc::clone(&self.state);
        let manager = Arc::clone(&self.session_manager);
        let consumer_id = self.consumer_id;
        move || {
            if let Some(worker) = worker
                && worker.join().is_err()
            {
                tracing::warn!("session event bridge panicked during shutdown");
            }
            let owned_sessions = state
                .control
                .lock()
                .map(|mut control| std::mem::take(&mut control.owned_sessions))
                .unwrap_or_default();
            for session_id in owned_sessions {
                manager.clear_session_event_consumer(&session_id, consumer_id);
            }
        }
    }

    pub(crate) fn shutdown(&mut self) {
        self.take_shutdown()();
    }
}

impl Drop for SessionEventBridge {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl SessionEventBridgeState {
    fn push_ui_event(&self, control: &mut SessionEventBridgeControl, event: SessionEvent) {
        control
            .pending_ui_sessions
            .insert(bridge_event_session_id(&event).to_string());
        self.ui_queue.push(event);
    }

    fn complete_ui_drain(&self, pending_sessions: &HashSet<String>) {
        // Routing, enqueue and release use the same lock. A new UI event cannot
        // be chosen using stale policy and enqueued after its boundary is released.
        let Ok(mut control) = self.control.lock() else {
            return;
        };
        let Ok(queue) = self.ui_queue.inner.lock() else {
            return;
        };
        let queued_sessions = queue
            .events
            .iter()
            .map(bridge_event_session_id)
            .collect::<HashSet<_>>();
        control.pending_ui_sessions.retain(|session_id| {
            pending_sessions.contains(session_id) || queued_sessions.contains(session_id.as_str())
        });
    }

    fn update_source_stats(&self, stats: &SessionDrainStats) {
        self.source_queued_events
            .store(stats.queued_events, Ordering::Relaxed);
        self.source_queued_output_bytes
            .store(stats.queued_output_bytes, Ordering::Relaxed);
        if let Ok(mut control) = self.control.lock() {
            control.source_queued_events = stats.queued_events;
            control.source_queued_output_bytes = stats.queued_output_bytes;
        }
    }
}

fn bridge_event_session_id(event: &SessionEvent) -> &str {
    match event {
        SessionEvent::Output { session_id, .. }
        | SessionEvent::OutputBoundary { session_id, .. }
        | SessionEvent::OutputDropped { session_id, .. }
        | SessionEvent::CwdChanged { session_id, .. }
        | SessionEvent::CommandAccepted { session_id, .. }
        | SessionEvent::Exited { session_id, .. }
        | SessionEvent::CleanupFailed { session_id, .. }
        | SessionEvent::TmuxState { session_id, .. }
        | SessionEvent::Error { session_id, .. } => session_id,
    }
}

impl SessionEventBridgeQueue {
    fn take_sessions(&self, session_ids: &[String]) -> VecDeque<SessionEvent> {
        let selected = session_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let Ok(mut inner) = self.inner.lock() else {
            return VecDeque::new();
        };
        let mut selected_events = VecDeque::new();
        let mut retained = VecDeque::new();
        while let Some(event) = inner.events.pop_front() {
            let session_id = match &event {
                SessionEvent::Output { session_id, .. }
                | SessionEvent::OutputBoundary { session_id, .. }
                | SessionEvent::OutputDropped { session_id, .. }
                | SessionEvent::CwdChanged { session_id, .. }
                | SessionEvent::CommandAccepted { session_id, .. }
                | SessionEvent::Exited { session_id, .. }
                | SessionEvent::CleanupFailed { session_id, .. }
                | SessionEvent::TmuxState { session_id, .. }
                | SessionEvent::Error { session_id, .. } => session_id,
            };
            if selected.contains(session_id.as_str()) {
                if let SessionEvent::Output { data, .. } = &event {
                    inner.queued_output_bytes =
                        inner.queued_output_bytes.saturating_sub(data.len());
                }
                selected_events.push_back(event);
            } else {
                retained.push_back(event);
            }
        }
        inner.events = retained;
        selected_events
    }

    #[cfg(test)]
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(SessionEventBridgeQueueInner::default())),
            wake: None,
        }
    }

    fn new_with_wake() -> (Self, UnboundedReceiver<()>) {
        let (wake, wake_rx) = EventWake::new();
        (
            Self {
                inner: Arc::new(Mutex::new(SessionEventBridgeQueueInner::default())),
                wake: Some(wake),
            },
            wake_rx,
        )
    }

    fn arm_wake(&self) {
        if let Some(wake) = &self.wake {
            wake.arm(ANY_INTEREST);
        }
    }

    #[cfg(test)]
    fn wake_count(&self) -> u64 {
        self.wake.as_ref().map(EventWake::signal_count).unwrap_or(0)
    }

    /// The single place the bridge's UI queue signals its consumer.
    ///
    /// The wake lives here rather than at the six `ui_queue.push` sites in
    /// `run_session_event_bridge`, so no producer path can be added later that
    /// enqueues without waking. `ANY_INTEREST` because the consumer treats every
    /// entry the same; the interest gate is what turns a flood into one wake per
    /// drain cycle instead of one per event.
    fn push(&self, event: SessionEvent) {
        {
            let Ok(mut inner) = self.inner.lock() else {
                return;
            };
            inner.push(event);
        }
        // Signalled outside the lock: nothing here needs it, and the consumer may
        // start draining the moment it is woken.
        if let Some(wake) = &self.wake {
            wake.signal(ANY_INTEREST);
        }
    }

    fn drain_with_output_budget(
        &self,
        max_events: usize,
        max_output_bytes: usize,
    ) -> SessionEventBridgeDrain {
        let Ok(mut inner) = self.inner.lock() else {
            return SessionEventBridgeDrain::default();
        };
        inner.drain(max_events, max_output_bytes)
    }

    fn len(&self) -> usize {
        self.inner
            .lock()
            .map(|inner| inner.events.len())
            .unwrap_or(0)
    }

    fn queued_output_bytes(&self) -> usize {
        self.inner
            .lock()
            .map(|inner| inner.queued_output_bytes)
            .unwrap_or(0)
    }
}

impl SessionEventBridgeQueueInner {
    fn push(&mut self, event: SessionEvent) {
        match &event {
            SessionEvent::Output { data, .. } if data.is_empty() => return,
            SessionEvent::Output { data, .. } => {
                self.queued_output_bytes = self.queued_output_bytes.saturating_add(data.len());
            }
            SessionEvent::OutputBoundary { .. }
            | SessionEvent::OutputDropped { .. }
            | SessionEvent::CwdChanged { .. }
            | SessionEvent::CommandAccepted { .. }
            | SessionEvent::Exited { .. }
            | SessionEvent::CleanupFailed { .. }
            | SessionEvent::TmuxState { .. }
            | SessionEvent::Error { .. } => {}
        }
        self.events.push_back(event);
    }

    fn drain(&mut self, max_events: usize, max_output_bytes: usize) -> SessionEventBridgeDrain {
        let mut events = Vec::new();
        let mut stats = SessionEventBridgeStats::default();
        let mut drained_events = 0usize;
        let mut drained_output_bytes = 0usize;
        for _ in 0..max_events {
            if drained_output_bytes >= max_output_bytes
                && drained_events > 0
                && matches!(self.events.front(), Some(SessionEvent::Output { .. }))
            {
                break;
            }
            let remaining_output_budget = max_output_bytes.saturating_sub(drained_output_bytes);
            if remaining_output_budget == 0
                && drained_events > 0
                && matches!(self.events.front(), Some(SessionEvent::Output { .. }))
            {
                break;
            }
            if let Some(SessionEvent::Output { session_id, data }) = self.events.front_mut() {
                let take = data.len().min(remaining_output_budget);
                if data.len() > take {
                    let remaining = data.split_off(take);
                    let chunk = std::mem::replace(data, remaining);
                    let session_id = session_id.clone();
                    self.queued_output_bytes = self.queued_output_bytes.saturating_sub(chunk.len());
                    drained_events = drained_events.saturating_add(1);
                    drained_output_bytes = drained_output_bytes.saturating_add(chunk.len());
                    events.push(SessionEvent::Output {
                        session_id,
                        data: chunk,
                    });
                    continue;
                }
            }
            let Some(event) = self.events.pop_front() else {
                break;
            };
            drained_events = drained_events.saturating_add(1);
            match &event {
                SessionEvent::Output { data, .. } => {
                    drained_output_bytes = drained_output_bytes.saturating_add(data.len());
                    self.queued_output_bytes = self.queued_output_bytes.saturating_sub(data.len());
                }
                SessionEvent::OutputDropped { bytes, .. } => {
                    stats.dropped_output_bytes = stats.dropped_output_bytes.saturating_add(*bytes);
                }
                SessionEvent::OutputBoundary { .. }
                | SessionEvent::CwdChanged { .. }
                | SessionEvent::CommandAccepted { .. }
                | SessionEvent::Exited { .. }
                | SessionEvent::CleanupFailed { .. }
                | SessionEvent::TmuxState { .. }
                | SessionEvent::Error { .. } => {}
            }
            events.push(event);
        }
        stats.drained_ui_events = drained_events;
        stats.drained_ui_output_bytes = drained_output_bytes;
        stats.ui_queued_events = self.events.len();
        stats.ui_queued_output_bytes = self.queued_output_bytes;
        SessionEventBridgeDrain { events, stats }
    }
}

fn run_session_event_bridge(
    session_manager: Arc<SessionManager>,
    consumer_id: SessionEventConsumerId,
    frame_pipeline: TerminalFramePipeline,
    recording_writer: Option<RecordingWriteHandle>,
    state: Arc<SessionEventBridgeState>,
) {
    let mut sideband_probe_sessions: HashMap<String, SessionEventBridgeSidebandProbe> =
        HashMap::new();
    let mut source_drain_backpressured = false;
    while !state.stop.load(Ordering::Relaxed) {
        source_drain_backpressured = bridge_should_pause_source_drain(
            frame_pipeline.queued_output_bytes(),
            state.ui_queue.queued_output_bytes(),
            source_drain_backpressured,
        );
        if source_drain_backpressured {
            thread::sleep(SESSION_EVENT_BRIDGE_BUSY_SLEEP);
            continue;
        }
        // Never hold the transfer gate while waiting or sleeping: repeatedly
        // reacquiring it after an idle wait can starve a workspace transfer.
        // Waiting leaves events queued, so a transfer can still change their
        // owner before the non-blocking drain below.
        if !session_manager
            .wait_for_events_for_consumer(consumer_id, SESSION_EVENT_BRIDGE_WAIT_TIMEOUT)
        {
            continue;
        }
        if state.stop.load(Ordering::Acquire) {
            break;
        }
        let Ok(transfer_gate) = state.transfer_gate.lock() else {
            thread::sleep(SESSION_EVENT_BRIDGE_IDLE_SLEEP);
            continue;
        };
        // Keep dequeue, routing and frame submission inside the same handoff
        // boundary. A pause must see every event already removed from transport.
        let Ok(drain) = session_manager.drain_events_blocking_for_consumer_with_output_budget(
            consumer_id,
            SESSION_EVENT_BRIDGE_DRAIN_BATCH,
            SESSION_EVENT_BRIDGE_OUTPUT_BUDGET,
            Duration::ZERO,
        ) else {
            drop(transfer_gate);
            thread::sleep(SESSION_EVENT_BRIDGE_IDLE_SLEEP);
            continue;
        };
        state.update_source_stats(&drain.stats);
        if drain.events.is_empty() {
            // Ownership may have changed since readiness was observed.
            continue;
        }
        let mut pending_direct_outputs = Vec::new();
        for event in drain.events {
            bridge_route_event(
                &state,
                &frame_pipeline,
                recording_writer.as_ref(),
                &mut sideband_probe_sessions,
                &mut pending_direct_outputs,
                event,
                Instant::now(),
            );
        }
        flush_bridge_direct_outputs(&frame_pipeline, &mut pending_direct_outputs);
    }
}

fn bridge_route_event(
    state: &SessionEventBridgeState,
    frame_pipeline: &TerminalFramePipeline,
    recording_writer: Option<&RecordingWriteHandle>,
    sideband_probe_sessions: &mut HashMap<String, SessionEventBridgeSidebandProbe>,
    pending_direct_outputs: &mut Vec<TerminalFrameOutputSubmission>,
    event: SessionEvent,
    now: Instant,
) {
    // Use live policy for every event, and hold it through enqueue. In particular,
    // a UI handoff earlier in this batch must fence every later event in it.
    let Ok(mut control) = state.control.lock() else {
        return;
    };
    match event {
        SessionEvent::Output { session_id, data } => {
            let sideband_probe_active =
                bridge_sideband_probe_active(sideband_probe_sessions, &session_id, now);
            let sideband_probe_detected = bridge_output_may_contain_sideband_trigger(&data);
            if sideband_probe_detected {
                bridge_arm_sideband_probe(sideband_probe_sessions, &session_id, now);
            }
            let needs_ui_probe = sideband_probe_active || sideband_probe_detected;
            let frame_queued_output_bytes = frame_pipeline.queued_output_bytes();
            if bridge_output_can_go_direct(
                &control,
                frame_queued_output_bytes,
                &session_id,
                needs_ui_probe,
            ) {
                state.direct_output_events.fetch_add(1, Ordering::Relaxed);
                state
                    .direct_output_bytes
                    .fetch_add(data.len() as u64, Ordering::Relaxed);
                if let Some(writer) = recording_writer {
                    writer.write_raw_output(&session_id, &data);
                }
                pending_direct_outputs.push(TerminalFrameOutputSubmission {
                    raw_already_captured: true,
                    session_id: session_id.clone(),
                    data,
                    encoding: control.session_encodings[&session_id].clone(),
                    scrollback_limit: control.scrollback_limit,
                });
            } else if bridge_output_is_backpressured(
                frame_queued_output_bytes,
                &control,
                &session_id,
                needs_ui_probe,
            ) {
                state
                    .direct_backpressure_events
                    .fetch_add(1, Ordering::Relaxed);
                state
                    .direct_backpressure_bytes
                    .fetch_add(data.len() as u64, Ordering::Relaxed);
                if let Some(writer) = recording_writer {
                    writer.write_raw_output(&session_id, &data);
                }
                pending_direct_outputs.push(TerminalFrameOutputSubmission {
                    raw_already_captured: true,
                    session_id: session_id.clone(),
                    data,
                    encoding: control.session_encodings[&session_id].clone(),
                    scrollback_limit: control.scrollback_limit,
                });
            } else {
                flush_bridge_direct_outputs(frame_pipeline, pending_direct_outputs);
                // Probe expiry must not let later bytes overtake this handoff.
                // Only completion of the UI drain can release its boundary.
                let routed_session_id = session_id.clone();
                state.push_ui_event(&mut control, SessionEvent::Output { session_id, data });
                bridge_consume_sideband_probe(sideband_probe_sessions, &routed_session_id);
            }
        }
        SessionEvent::OutputDropped { session_id, bytes } => {
            if let Some(writer) = recording_writer {
                writer.report_output_gap(&session_id, bytes);
            }
            flush_bridge_direct_outputs(frame_pipeline, pending_direct_outputs);
            sideband_probe_sessions.remove(&session_id);
            control.ui_routed_sessions.insert(session_id.clone());
            state.push_ui_event(
                &mut control,
                SessionEvent::OutputDropped { session_id, bytes },
            );
        }
        event @ SessionEvent::OutputBoundary { .. } => {
            flush_bridge_direct_outputs(frame_pipeline, pending_direct_outputs);
            state.push_ui_event(&mut control, event);
        }
        SessionEvent::CwdChanged { session_id, cwd } => {
            flush_bridge_direct_outputs(frame_pipeline, pending_direct_outputs);
            state.push_ui_event(&mut control, SessionEvent::CwdChanged { session_id, cwd });
        }
        SessionEvent::CommandAccepted {
            session_id,
            command,
        } => {
            flush_bridge_direct_outputs(frame_pipeline, pending_direct_outputs);
            state.push_ui_event(
                &mut control,
                SessionEvent::CommandAccepted {
                    session_id,
                    command,
                },
            );
        }
        SessionEvent::Exited { session_id, reason } => {
            flush_bridge_direct_outputs(frame_pipeline, pending_direct_outputs);
            sideband_probe_sessions.remove(&session_id);
            state.push_ui_event(&mut control, SessionEvent::Exited { session_id, reason });
        }
        event @ SessionEvent::CleanupFailed { .. } => {
            state.push_ui_event(&mut control, event);
        }
        event @ SessionEvent::TmuxState { .. } => {
            // Flush first: pane state may reorder the visible projection and
            // must not overtake output already submitted for the session.
            flush_bridge_direct_outputs(frame_pipeline, pending_direct_outputs);
            state.push_ui_event(&mut control, event);
        }
        SessionEvent::Error {
            session_id,
            message,
        } => {
            flush_bridge_direct_outputs(frame_pipeline, pending_direct_outputs);
            state.push_ui_event(
                &mut control,
                SessionEvent::Error {
                    session_id,
                    message,
                },
            );
        }
    }
}

fn bridge_should_pause_source_drain(
    frame_pipeline_queued_output_bytes: usize,
    ui_queued_output_bytes: usize,
    currently_backpressured: bool,
) -> bool {
    if currently_backpressured {
        frame_pipeline_queued_output_bytes > SESSION_EVENT_BRIDGE_DIRECT_OUTPUT_LOW_WATERMARK
            || ui_queued_output_bytes > SESSION_EVENT_BRIDGE_UI_OUTPUT_LOW_WATERMARK
    } else {
        frame_pipeline_queued_output_bytes >= SESSION_EVENT_BRIDGE_DIRECT_OUTPUT_BACKPRESSURE
            || ui_queued_output_bytes >= SESSION_EVENT_BRIDGE_UI_OUTPUT_LIMIT
    }
}

fn flush_bridge_direct_outputs(
    frame_pipeline: &TerminalFramePipeline,
    pending_outputs: &mut Vec<TerminalFrameOutputSubmission>,
) {
    if !pending_outputs.is_empty() {
        frame_pipeline.submit_outputs(std::mem::take(pending_outputs));
    }
}

fn bridge_output_can_go_direct(
    control: &SessionEventBridgeControl,
    frame_pipeline_queued_output_bytes: usize,
    session_id: &str,
    needs_ui_probe: bool,
) -> bool {
    control.session_encodings.contains_key(session_id)
        && !control.ui_routed_sessions.contains(session_id)
        && !control.pending_ui_sessions.contains(session_id)
        && frame_pipeline_queued_output_bytes < SESSION_EVENT_BRIDGE_DIRECT_OUTPUT_BACKPRESSURE
        && !needs_ui_probe
}

fn bridge_output_is_backpressured(
    frame_pipeline_queued_output_bytes: usize,
    control: &SessionEventBridgeControl,
    session_id: &str,
    needs_ui_probe: bool,
) -> bool {
    control.session_encodings.contains_key(session_id)
        && !control.ui_routed_sessions.contains(session_id)
        && !control.pending_ui_sessions.contains(session_id)
        && frame_pipeline_queued_output_bytes >= SESSION_EVENT_BRIDGE_DIRECT_OUTPUT_BACKPRESSURE
        && !needs_ui_probe
}

fn bridge_output_may_contain_sideband_trigger(data: &[u8]) -> bool {
    ZmodemDetector::output_may_contain_trigger(data)
        || TrzszDetector::output_may_contain_trigger(data)
}

fn bridge_arm_sideband_probe(
    probes: &mut HashMap<String, SessionEventBridgeSidebandProbe>,
    session_id: &str,
    now: Instant,
) {
    probes.insert(
        session_id.to_string(),
        SessionEventBridgeSidebandProbe {
            events_remaining: SESSION_EVENT_BRIDGE_SIDEBAND_PROBE_EVENTS,
            expires_at: now + SESSION_EVENT_BRIDGE_SIDEBAND_PROBE_WINDOW,
        },
    );
}

fn bridge_sideband_probe_active(
    probes: &mut HashMap<String, SessionEventBridgeSidebandProbe>,
    session_id: &str,
    now: Instant,
) -> bool {
    let Some(probe) = probes.get(session_id).copied() else {
        return false;
    };
    if probe.events_remaining == 0 || now >= probe.expires_at {
        probes.remove(session_id);
        return false;
    }
    true
}

fn bridge_consume_sideband_probe(
    probes: &mut HashMap<String, SessionEventBridgeSidebandProbe>,
    session_id: &str,
) {
    let Some(probe) = probes.get_mut(session_id) else {
        return;
    };
    probe.events_remaining = probe.events_remaining.saturating_sub(1);
    if probe.events_remaining == 0 {
        probes.remove(session_id);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};

    use super::{
        SESSION_EVENT_BRIDGE_DIRECT_OUTPUT_BACKPRESSURE,
        SESSION_EVENT_BRIDGE_DIRECT_OUTPUT_LOW_WATERMARK,
        SESSION_EVENT_BRIDGE_SIDEBAND_PROBE_EVENTS, SESSION_EVENT_BRIDGE_SIDEBAND_PROBE_WINDOW,
        SESSION_EVENT_BRIDGE_UI_OUTPUT_LIMIT, SESSION_EVENT_BRIDGE_UI_OUTPUT_LOW_WATERMARK,
        SessionEventBridge, SessionEventBridgeControl, SessionEventBridgeQueue,
        SessionEventBridgeSidebandProbe, SessionEventBridgeState, bridge_arm_sideband_probe,
        bridge_consume_sideband_probe, bridge_output_can_go_direct, bridge_output_is_backpressured,
        bridge_output_may_contain_sideband_trigger, bridge_route_event,
        bridge_should_pause_source_drain, bridge_sideband_probe_active,
        flush_bridge_direct_outputs,
    };
    use crate::models::recording::RecordingWritePipeline;
    use crate::models::terminal::{TerminalFrameOutputSubmission, TerminalFramePipeline};
    use nyaterm_transport::{SessionEvent, SessionManager};

    #[test]
    fn protocol_handoff_extracts_only_its_session_and_keeps_migration_consumer() {
        let manager = Arc::new(SessionManager::new());
        let bridge = SessionEventBridge::spawn(
            manager.clone(),
            TerminalFramePipeline::default(),
            None,
            "UTF-8".into(),
            1000,
        );
        bridge.claim_session("protocol");
        for (id, bytes) in [
            ("protocol", b"first".as_slice()),
            ("other", b"unrelated".as_slice()),
            ("protocol", b"second".as_slice()),
        ] {
            bridge.push_ui_event_for_test(SessionEvent::Output {
                session_id: id.into(),
                data: bytes.to_vec(),
            });
        }
        let consumer = manager.register_event_consumer();
        let pending = bridge.begin_protocol_handoff("protocol", consumer);
        let bytes: Vec<u8> = pending
            .into_iter()
            .flat_map(|event| match event {
                SessionEvent::Output { data, .. } => data,
                _ => panic!("output"),
            })
            .collect();
        assert_eq!(bytes, b"firstsecond");
        bridge.claim_session("protocol");
        assert_eq!(
            bridge
                .state
                .control
                .lock()
                .unwrap()
                .protocol_consumers
                .get("protocol"),
            Some(&consumer)
        );
        assert_eq!(
            bridge.state.ui_queue.take_sessions(&["other".into()]).len(),
            1
        );
        bridge.finish_protocol_handoff("protocol");
        let control = bridge.state.control.lock().unwrap();
        assert!(!control.protocol_consumers.contains_key("protocol"));
        assert!(control.pending_ui_sessions.contains("protocol"));
    }

    #[test]
    fn repeated_transfers_finish_while_both_event_bridges_are_idle() {
        assert_repeated_transfers_finish(false);
    }

    #[test]
    fn repeated_transfers_finish_while_both_event_bridges_are_backpressured() {
        assert_repeated_transfers_finish(true);
    }

    fn assert_repeated_transfers_finish(backpressured: bool) {
        let manager = Arc::new(SessionManager::new());
        let source = SessionEventBridge::spawn(
            Arc::clone(&manager),
            TerminalFramePipeline::default(),
            None,
            "UTF-8".into(),
            1000,
        );
        let target = SessionEventBridge::spawn(
            manager,
            TerminalFramePipeline::default(),
            None,
            "UTF-8".into(),
            1000,
        );
        source.claim_session("moved");
        if backpressured {
            for bridge in [&source, &target] {
                bridge.state.ui_queue.push(SessionEvent::Output {
                    session_id: "unmoved".into(),
                    data: vec![b'x'; SESSION_EVENT_BRIDGE_UI_OUTPUT_LIMIT],
                });
            }
        }
        let source_state = Arc::clone(&source.state);
        let target_state = Arc::clone(&target.state);
        // Let idle workers enter their event waits before competing for the
        // transfer gate. The old loop kept the gate locked throughout each wait.
        std::thread::sleep(Duration::from_millis(20));
        let (done_tx, done_rx) = mpsc::channel();
        let mover = std::thread::spawn(move || {
            let ids = vec!["moved".to_string()];
            for _ in 0..64 {
                assert!(source.pause_sessions_for_transfer(&ids).is_empty());
                target.claim_session("moved");
                assert!(target.pause_sessions_for_transfer(&ids).is_empty());
                source.claim_session("moved");
            }
            done_tx.send(()).expect("report completed transfers");
        });
        let completed = done_rx.recv_timeout(Duration::from_secs(5));
        // Stop both workers even on timeout, so a regression cannot leave the
        // test itself stuck in a starved lock acquisition or worker join.
        source_state.stop.store(true, Ordering::Release);
        target_state.stop.store(true, Ordering::Release);
        mover.join().expect("transfer worker");
        completed.expect("event waits and backpressure must not starve workspace transfers");
    }

    struct BridgeFixture {
        state: SessionEventBridgeState,
        pipeline: TerminalFramePipeline,
        recording: RecordingWritePipeline,
        probes: HashMap<String, SessionEventBridgeSidebandProbe>,
        outputs: Vec<TerminalFrameOutputSubmission>,
    }

    impl BridgeFixture {
        fn new() -> Self {
            let recording = RecordingWritePipeline::spawn(4096);
            let pipeline = TerminalFramePipeline::spawn(recording.writer());
            Self {
                state: SessionEventBridgeState {
                    control: Mutex::new(SessionEventBridgeControl {
                        session_encodings: [
                            ("s1".into(), "UTF-8".into()),
                            ("s2".into(), "UTF-8".into()),
                        ]
                        .into_iter()
                        .collect(),
                        scrollback_limit: 1000,
                        ..SessionEventBridgeControl::default()
                    }),
                    transfer_gate: Mutex::new(()),
                    ui_queue: SessionEventBridgeQueue::new(),
                    ui_queue_wake_rx: Mutex::new(None),
                    source_queued_events: AtomicUsize::new(0),
                    source_queued_output_bytes: AtomicUsize::new(0),
                    direct_output_events: AtomicU64::new(0),
                    direct_output_bytes: AtomicU64::new(0),
                    direct_backpressure_events: AtomicU64::new(0),
                    direct_backpressure_bytes: AtomicU64::new(0),
                    stop: AtomicBool::new(false),
                },
                pipeline,
                recording,
                probes: HashMap::new(),
                outputs: Vec::new(),
            }
        }

        fn event(&mut self, event: SessionEvent, now: Instant) {
            bridge_route_event(
                &self.state,
                &self.pipeline,
                None,
                &mut self.probes,
                &mut self.outputs,
                event,
                now,
            );
        }

        fn output(&mut self, session_id: &str, data: &[u8], now: Instant) {
            self.event(
                SessionEvent::Output {
                    session_id: session_id.into(),
                    data: data.to_vec(),
                },
                now,
            );
        }

        fn flush(&mut self) {
            flush_bridge_direct_outputs(&self.pipeline, &mut self.outputs);
        }

        fn text(&self, session_id: &str) -> String {
            futures::executor::block_on(self.pipeline.request_all_text(session_id.into()))
                .unwrap()
                .unwrap_or_default()
                .trim_end()
                .to_string()
        }

        fn submit_ui_events(&self, events: Vec<SessionEvent>) {
            self.pipeline.submit_outputs(
                events
                    .into_iter()
                    .filter_map(|event| {
                        let SessionEvent::Output { session_id, data } = event else {
                            return None;
                        };
                        Some(TerminalFrameOutputSubmission {
                            session_id,
                            data,
                            encoding: "UTF-8".into(),
                            scrollback_limit: 1000,
                            raw_already_captured: true,
                        })
                    })
                    .collect(),
            );
        }
    }

    impl Drop for BridgeFixture {
        fn drop(&mut self) {
            self.pipeline.shutdown();
            self.recording.shutdown();
        }
    }

    #[test]
    fn delayed_yaml_handoff_keeps_output_and_prompt_in_wire_order_after_probe_expiry() {
        let mut fixture = BridgeFixture::new();
        let now = Instant::now();
        fixture.output("s1", b"kind: ConfigMap\r\n", now);
        fixture.output("s1", b"---\r\nkind: Deployment\r\nmetadata:", now);
        for index in 0..8 {
            fixture.output("s1", format!("\r\n  field{index}: value").as_bytes(), now);
        }
        fixture.output(
            "s1",
            b"\r\n---\r\nkind: Service\r\n$ ",
            now + Duration::from_secs(1),
        );
        fixture.flush();
        assert_eq!(fixture.text("s1"), "kind: ConfigMap");

        let drain = fixture
            .state
            .ui_queue
            .drain_with_output_budget(100, usize::MAX);
        fixture.submit_ui_events(drain.events);
        fixture.state.complete_ui_drain(&HashSet::new());
        fixture.output("s1", b"done\r\n", now + Duration::from_secs(2));
        assert_eq!(
            fixture.outputs.len(),
            1,
            "normal output must resume the direct path"
        );
        fixture.flush();
        let mut expected = "kind: ConfigMap\n---\nkind: Deployment\nmetadata:".to_string();
        for index in 0..8 {
            expected.push_str(&format!("\n  field{index}: value"));
        }
        expected.push_str("\n---\nkind: Service\n$ done");
        assert_eq!(fixture.text("s1"), expected);
    }

    #[test]
    fn partial_ui_drains_and_events_already_in_ui_keep_the_session_fenced() {
        let mut fixture = BridgeFixture::new();
        let now = Instant::now();
        fixture.output("s1", b"metadata:", now);
        let first = fixture.state.ui_queue.drain_with_output_budget(1, 4);
        fixture.submit_ui_events(first.events);
        fixture.state.complete_ui_drain(&HashSet::new());
        assert!(
            fixture
                .state
                .control
                .lock()
                .unwrap()
                .pending_ui_sessions
                .contains("s1")
        );

        let rest = fixture
            .state
            .ui_queue
            .drain_with_output_budget(100, usize::MAX);
        fixture
            .state
            .complete_ui_drain(&HashSet::from(["s1".into()]));
        fixture
            .state
            .control
            .lock()
            .unwrap()
            .ui_routed_sessions
            .remove("s1");
        fixture.output("s1", b"\r\nkind: Service\r\n", now + Duration::from_secs(1));
        fixture.state.complete_ui_drain(&HashSet::new());
        assert!(
            fixture
                .state
                .control
                .lock()
                .unwrap()
                .pending_ui_sessions
                .contains("s1")
        );
        fixture.submit_ui_events(rest.events);
        let later = fixture
            .state
            .ui_queue
            .drain_with_output_budget(100, usize::MAX);
        fixture.submit_ui_events(later.events);
        fixture.state.complete_ui_drain(&HashSet::new());
        assert!(
            !fixture
                .state
                .control
                .lock()
                .unwrap()
                .pending_ui_sessions
                .contains("s1")
        );
        assert_eq!(fixture.text("s1"), "metadata:\nkind: Service");
    }

    #[test]
    fn sideband_handoffs_fence_only_their_session_for_trzsz_and_zmodem() {
        for prefix in [b":".as_slice(), b"*", b"::TRZSZ"] {
            let mut fixture = BridgeFixture::new();
            let now = Instant::now();
            fixture.output("s1", prefix, now);
            // Queued UI work must survive both limits of the speculative probe.
            for _ in 0..8 {
                fixture.output("s1", b" continuation", now);
            }
            fixture.output("s1", b" tail", now + Duration::from_secs(1));
            fixture.output("s2", b"other session\r\n", now);
            fixture.flush();
            assert_eq!(fixture.text("s1"), "");
            assert_eq!(fixture.text("s2"), "other session");
            assert_eq!(fixture.state.ui_queue.len(), 10);
        }
    }

    #[test]
    fn live_ai_routing_and_metadata_handoffs_apply_between_events_in_one_batch() {
        let mut fixture = BridgeFixture::new();
        let now = Instant::now();
        fixture.output("s1", b"before\r\n", now);
        fixture
            .state
            .control
            .lock()
            .unwrap()
            .ui_routed_sessions
            .insert("s1".into());
        fixture.output("s1", b"captured\r\n", now);
        fixture
            .state
            .control
            .lock()
            .unwrap()
            .ui_routed_sessions
            .remove("s1");
        fixture.output("s1", b"after\r\n", now);
        fixture.flush();
        assert_eq!(fixture.text("s1"), "before");
        let drain = fixture
            .state
            .ui_queue
            .drain_with_output_budget(100, usize::MAX);
        fixture.submit_ui_events(drain.events);
        fixture.state.complete_ui_drain(&HashSet::new());
        assert_eq!(fixture.text("s1"), "before\ncaptured\nafter");

        fixture.event(
            SessionEvent::CwdChanged {
                session_id: "s1".into(),
                cwd: "/tmp".into(),
            },
            now,
        );
        fixture.output("s1", b"after metadata\r\n", now);
        fixture.flush();
        assert_eq!(fixture.state.ui_queue.len(), 2);
        assert_eq!(fixture.text("s1"), "before\ncaptured\nafter");
    }

    #[test]
    fn ui_completion_keeps_detector_held_bytes_and_ai_capture_on_the_ui_path() {
        let mut fixture = BridgeFixture::new();
        let now = Instant::now();
        fixture.output("s1", b":", now);
        let drain = fixture
            .state
            .ui_queue
            .drain_with_output_budget(100, usize::MAX);
        assert_eq!(drain.events.len(), 1);
        // The UI has consumed this event, but a detector still holds its colon.
        fixture
            .state
            .control
            .lock()
            .unwrap()
            .ui_routed_sessions
            .insert("s1".into());
        fixture.state.complete_ui_drain(&HashSet::new());
        fixture.output("s1", b" continuation\r\n", now + Duration::from_secs(1));
        fixture.flush();
        assert_eq!(fixture.text("s1"), "");
        let later = fixture
            .state
            .ui_queue
            .drain_with_output_budget(100, usize::MAX);
        // A resolved prefix is submitted before the UI requests direct routing.
        fixture.submit_ui_events(drain.events);
        fixture.submit_ui_events(later.events);
        fixture
            .state
            .control
            .lock()
            .unwrap()
            .ui_routed_sessions
            .remove("s1");
        fixture.state.complete_ui_drain(&HashSet::new());
        fixture.output("s1", b"$ ", now + Duration::from_secs(2));
        fixture.flush();
        assert_eq!(fixture.text("s1"), ": continuation\n$");
    }

    #[test]
    fn bridge_direct_policy_rejects_sideband_triggers() {
        let control = SessionEventBridgeControl {
            ui_routed_sessions: HashSet::new(),
            session_encodings: [
                ("s1".to_string(), "UTF-8".to_string()),
                ("s2".to_string(), "UTF-8".to_string()),
            ]
            .into_iter()
            .collect(),
            scrollback_limit: 1000,
            ..SessionEventBridgeControl::default()
        };
        assert!(bridge_output_can_go_direct(&control, 0, "s1", false));
        assert!(!bridge_output_can_go_direct(&control, 0, "s1", true));
        assert!(bridge_output_may_contain_sideband_trigger(b"**\x18B"));
        assert!(bridge_output_may_contain_sideband_trigger(b"::TRZSZ:"));
    }

    #[test]
    fn bridge_direct_policy_honors_ui_routed_sessions() {
        let mut routed = HashSet::new();
        routed.insert("s1".to_string());
        let control = SessionEventBridgeControl {
            ui_routed_sessions: routed,
            session_encodings: [
                ("s1".to_string(), "UTF-8".to_string()),
                ("s2".to_string(), "UTF-8".to_string()),
            ]
            .into_iter()
            .collect(),
            scrollback_limit: 1000,
            ..SessionEventBridgeControl::default()
        };
        assert!(!bridge_output_can_go_direct(&control, 0, "s1", false));
        assert!(bridge_output_can_go_direct(&control, 0, "s2", false));
    }

    #[test]
    fn bridge_direct_policy_yields_under_frame_backpressure() {
        let control = SessionEventBridgeControl {
            ui_routed_sessions: HashSet::new(),
            session_encodings: [
                ("s1".to_string(), "UTF-8".to_string()),
                ("s2".to_string(), "UTF-8".to_string()),
            ]
            .into_iter()
            .collect(),
            scrollback_limit: 1000,
            ..SessionEventBridgeControl::default()
        };

        assert!(bridge_output_can_go_direct(
            &control,
            SESSION_EVENT_BRIDGE_DIRECT_OUTPUT_BACKPRESSURE - 1,
            "s1",
            false
        ));
        assert!(!bridge_output_can_go_direct(
            &control,
            SESSION_EVENT_BRIDGE_DIRECT_OUTPUT_BACKPRESSURE,
            "s1",
            false
        ));
        assert!(bridge_output_is_backpressured(
            SESSION_EVENT_BRIDGE_DIRECT_OUTPUT_BACKPRESSURE,
            &control,
            "s1",
            false
        ));
        assert!(!bridge_output_is_backpressured(
            SESSION_EVENT_BRIDGE_DIRECT_OUTPUT_BACKPRESSURE,
            &control,
            "s1",
            true
        ));
    }

    #[test]
    fn bridge_ui_queue_coalesces_a_burst_into_one_wake() {
        let (queue, mut wake_rx) = SessionEventBridgeQueue::new_with_wake();

        // Unarmed: the consumer is already draining, so a push must cost nothing.
        queue.push(SessionEvent::Output {
            session_id: "s1".to_string(),
            data: b"before the arm".to_vec(),
        });
        assert_eq!(queue.wake_count(), 0);

        queue.arm_wake();
        for index in 0..64 {
            queue.push(SessionEvent::Output {
                session_id: "s1".to_string(),
                data: format!("chunk {index}").into_bytes(),
            });
        }

        assert_eq!(
            queue.wake_count(),
            1,
            "a flood must cost one wake per drain cycle, not one per event"
        );
        assert!(matches!(wake_rx.try_recv(), Ok(())));
        assert!(
            wake_rx.try_recv().is_err(),
            "only the first push after an arm should have queued a wake"
        );

        // And the next cycle's arm re-enables delivery.
        queue.arm_wake();
        queue.push(SessionEvent::CwdChanged {
            session_id: "s1".to_string(),
            cwd: "/srv/app".to_string(),
        });
        assert_eq!(queue.wake_count(), 2);
    }

    #[test]
    fn bridge_ui_queue_without_a_wake_still_queues() {
        let queue = SessionEventBridgeQueue::new();
        queue.arm_wake();
        queue.push(SessionEvent::CwdChanged {
            session_id: "s1".to_string(),
            cwd: "/srv/app".to_string(),
        });

        assert_eq!(queue.len(), 1);
        assert_eq!(queue.wake_count(), 0);
    }

    #[test]
    fn bridge_ui_queue_transfer_keeps_other_sessions_and_output_accounting() {
        let queue = SessionEventBridgeQueue::new();
        queue.push(SessionEvent::Output {
            session_id: "source".into(),
            data: b"before move".to_vec(),
        });
        queue.push(SessionEvent::CwdChanged {
            session_id: "other".into(),
            cwd: "/tmp".into(),
        });
        queue.push(SessionEvent::Exited {
            session_id: "source".into(),
            reason: "done".into(),
        });
        let moved = queue.take_sessions(&["source".into()]);
        assert_eq!(moved.len(), 2);
        assert_eq!(queue.queued_output_bytes(), 0);
        let remaining = queue.drain_with_output_budget(8, usize::MAX);
        assert!(matches!(remaining.events.as_slice(),
            [SessionEvent::CwdChanged { session_id, .. }] if session_id == "other"));
    }

    #[test]
    fn bridge_ui_queue_drains_metadata_when_output_budget_is_zero() {
        let queue = SessionEventBridgeQueue::new();
        queue.push(SessionEvent::CwdChanged {
            session_id: "s1".to_string(),
            cwd: "/srv/app".to_string(),
        });
        queue.push(SessionEvent::CommandAccepted {
            session_id: "s1".to_string(),
            command: "cargo test".to_string(),
        });

        let drain = queue.drain_with_output_budget(8, 0);

        assert_eq!(drain.events.len(), 2);
        assert!(matches!(
            &drain.events[0],
            SessionEvent::CwdChanged { cwd, .. } if cwd == "/srv/app"
        ));
        assert!(matches!(
            &drain.events[1],
            SessionEvent::CommandAccepted { command, .. } if command == "cargo test"
        ));
    }

    #[test]
    fn bridge_pauses_source_drain_with_high_low_watermark_hysteresis() {
        assert!(bridge_should_pause_source_drain(
            SESSION_EVENT_BRIDGE_DIRECT_OUTPUT_BACKPRESSURE,
            0,
            false,
        ));
        assert!(bridge_should_pause_source_drain(
            SESSION_EVENT_BRIDGE_DIRECT_OUTPUT_LOW_WATERMARK + 1,
            0,
            true,
        ));
        assert!(!bridge_should_pause_source_drain(
            SESSION_EVENT_BRIDGE_DIRECT_OUTPUT_LOW_WATERMARK,
            SESSION_EVENT_BRIDGE_UI_OUTPUT_LOW_WATERMARK,
            true,
        ));
        assert!(bridge_should_pause_source_drain(
            0,
            SESSION_EVENT_BRIDGE_UI_OUTPUT_LIMIT,
            false,
        ));
    }

    #[test]
    fn bridge_ui_queue_preserves_output_above_the_old_limit() {
        let queue = SessionEventBridgeQueue::new();
        let first = vec![b'a'; SESSION_EVENT_BRIDGE_UI_OUTPUT_LIMIT];
        let second = vec![b'b'; 32];
        queue.push(SessionEvent::Output {
            session_id: "s1".to_string(),
            data: first.clone(),
        });
        queue.push(SessionEvent::Output {
            session_id: "s1".to_string(),
            data: second.clone(),
        });

        let drain = queue.drain_with_output_budget(8, usize::MAX);
        let output = drain
            .events
            .into_iter()
            .flat_map(|event| match event {
                SessionEvent::Output { data, .. } => data,
                _ => Vec::new(),
            })
            .collect::<Vec<_>>();
        assert_eq!(output.len(), first.len() + second.len());
        assert_eq!(&output[..first.len()], first.as_slice());
        assert_eq!(&output[first.len()..], second.as_slice());
        assert_eq!(drain.stats.dropped_output_bytes, 0);
    }

    #[test]
    fn bridge_sideband_probe_is_bounded_by_events_and_time() {
        let mut probes = HashMap::new();
        let now = Instant::now();
        bridge_arm_sideband_probe(&mut probes, "s1", now);

        assert!(bridge_sideband_probe_active(&mut probes, "s1", now));
        for _ in 0..SESSION_EVENT_BRIDGE_SIDEBAND_PROBE_EVENTS {
            bridge_consume_sideband_probe(&mut probes, "s1");
        }
        assert!(!bridge_sideband_probe_active(&mut probes, "s1", now));

        bridge_arm_sideband_probe(&mut probes, "s2", now);
        assert!(!bridge_sideband_probe_active(
            &mut probes,
            "s2",
            now + SESSION_EVENT_BRIDGE_SIDEBAND_PROBE_WINDOW + Duration::from_millis(1)
        ));
    }
}
