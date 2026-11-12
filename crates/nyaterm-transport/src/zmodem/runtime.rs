//! One background owner consumes the session's bounded transport event queue.
use nyaterm_core::runtime::reaper::Reaper;
use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::{ZmodemAction, ZmodemDirection, ZmodemEvent, ZmodemTransfer};
use crate::{SessionEvent, SessionEventConsumerId, SessionManager};

static REAPER: Mutex<Option<Reaper<JoinHandle<()>>>> = Mutex::new(None);

const INPUT_BATCH: usize = 128 * 1024;
const EVENT_CAPACITY: usize = 256;
const IDLE_WAIT: Duration = Duration::from_millis(25);
const FINISH_WAIT: Duration = Duration::from_millis(250);

pub type RuntimeWake = Arc<dyn Fn() + Send + Sync>;
type InputObserver = Arc<dyn Fn(&str, &[u8]) + Send + Sync>;

pub struct ZmodemRuntimeEvent {
    pub actions: Vec<ZmodemAction>,
    pub done: bool,
    /// Already dequeued bytes and metadata must precede a resumed bridge.
    pub pending_events: VecDeque<SessionEvent>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ZmodemRuntimeStats {
    pub input_bytes: u64,
    pub wire_bytes: u64,
    pub source_queue_peak_bytes: u64,
    pub wire_submit_nanoseconds: u64,
    pub pump_turns: u64,
    pub capacity_wait_nanoseconds: u64,
    pub write_queue_peak_bytes: u64,
    pub sender_ack_waits: u64,
}

#[derive(Default)]
struct Counters {
    input: AtomicU64,
    wire: AtomicU64,
    submit_ns: AtomicU64,
    turns: AtomicU64,
    capacity_wait: AtomicU64,
    ack_waits: AtomicU64,
}

enum Acceptance {
    Download(PathBuf),
    Upload(Vec<PathBuf>),
}

#[derive(Default)]
struct State {
    bootstrap: Option<VecDeque<SessionEvent>>,
    acceptance: Option<Acceptance>,
    cancel_reason: Option<String>,
    events: VecDeque<ZmodemRuntimeEvent>,
    progress: Option<ZmodemEvent>,
    closed: bool,
    wake: Option<RuntimeWake>,
    input_observer: Option<InputObserver>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    cancelled: AtomicBool,
    stats: Counters,
}

pub struct ZmodemRuntime {
    manager: Arc<SessionManager>,
    session_id: String,
    consumer_id: SessionEventConsumerId,
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl ZmodemRuntime {
    /// Spawn before changing consumer ownership so creation failure is reversible.
    pub fn spawn(
        manager: Arc<SessionManager>,
        session_id: String,
        transfer: ZmodemTransfer,
    ) -> io::Result<Self> {
        {
            let mut reaper = REAPER
                .lock()
                .map_err(|_| io::Error::other("ZMODEM reaper poisoned"))?;
            if reaper.is_none() {
                *reaper = Some(Reaper::new(
                    "zmodem-reaper",
                    64,
                    |thread: JoinHandle<()>| {
                        let _ = thread.join();
                    },
                )?);
            }
        }
        let consumer_id = manager.register_event_consumer();
        let shared = Arc::new(Shared::default());
        let worker_shared = shared.clone();
        let worker_manager = manager.clone();
        let worker_session = session_id.clone();
        let worker =
            nyaterm_core::runtime::worker::spawn("nyaterm-zmodem-transfer".into(), move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(
                        transfer,
                        &worker_manager,
                        &worker_session,
                        consumer_id,
                        &worker_shared,
                    );
                }));
                if result.is_err() {
                    publish(
                        &worker_shared,
                        ZmodemRuntimeEvent {
                            actions: vec![ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                                reason: "ZMODEM worker panicked".into(),
                            })],
                            done: true,
                            pending_events: VecDeque::new(),
                        },
                    );
                }
            })?;
        Ok(Self {
            manager,
            session_id,
            consumer_id,
            shared,
            worker: Some(worker),
        })
    }

    pub fn consumer_id(&self) -> SessionEventConsumerId {
        self.consumer_id
    }

    /// Called exactly once, after the old consumer's dequeue/routing fence.
    pub fn activate(&self, bootstrap: VecDeque<SessionEvent>) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .bootstrap = Some(bootstrap);
        self.shared.changed.notify_all();
    }

    pub fn set_wake(&self, wake: RuntimeWake) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .wake = Some(wake.clone());
        wake();
    }

    pub fn set_input_observer(&self, observer: InputObserver) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .input_observer = Some(observer);
    }

    pub fn accept_download(&self, directory: PathBuf) {
        self.accept(Acceptance::Download(directory));
    }
    pub fn accept_upload(&self, files: Vec<PathBuf>) {
        self.accept(Acceptance::Upload(files));
    }
    fn accept(&self, acceptance: Acceptance) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .acceptance = Some(acceptance);
        self.shared.changed.notify_all();
    }

    pub fn cancel(&self, reason: impl Into<String>) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .cancel_reason = Some(reason.into());
        self.shared.cancelled.store(true, Ordering::Release);
        self.shared.changed.notify_all();
    }

    pub fn try_recv_event(&self) -> Option<ZmodemRuntimeEvent> {
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        let event = state.events.pop_front().or_else(|| {
            state.progress.take().map(|progress| ZmodemRuntimeEvent {
                actions: vec![ZmodemAction::EmitEvent(progress)],
                done: false,
                pending_events: VecDeque::new(),
            })
        });
        drop(state);
        self.shared.changed.notify_all();
        event
    }

    pub fn stats(&self) -> ZmodemRuntimeStats {
        let s = &self.shared.stats;
        ZmodemRuntimeStats {
            input_bytes: s.input.load(Ordering::Relaxed),
            wire_bytes: s.wire.load(Ordering::Relaxed),
            source_queue_peak_bytes: self.manager.event_queue.peak_output_bytes() as u64,
            write_queue_peak_bytes: self.manager.write_queue_peak_bytes(&self.session_id) as u64,
            wire_submit_nanoseconds: s.submit_ns.load(Ordering::Relaxed),
            pump_turns: s.turns.load(Ordering::Relaxed),
            capacity_wait_nanoseconds: s.capacity_wait.load(Ordering::Relaxed),
            sender_ack_waits: s.ack_waits.load(Ordering::Relaxed),
        }
    }

    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for ZmodemRuntime {
    fn drop(&mut self) {
        self.shared.cancelled.store(true, Ordering::Release);
        self.shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .closed = true;
        self.shared.changed.notify_all();
        self.manager
            .clear_session_event_consumer(&self.session_id, self.consumer_id);
        // Never wait for native disk I/O on GPUI. If the bounded reaper itself
        // is full, dropping the handle detaches the already-cancelled worker.
        if let Ok(reaper) = REAPER.lock()
            && let Some(reaper) = reaper.as_ref()
        {
            let _ = reaper.try_submit_with(|| self.worker.take());
        }
    }
}

fn progress_name(event: &ZmodemEvent) -> &str {
    match event {
        ZmodemEvent::Progress { file_name, .. } => file_name,
        _ => "",
    }
}

fn publish(shared: &Shared, mut event: ZmodemRuntimeEvent) -> bool {
    let mut state = shared.state.lock().unwrap_or_else(|p| p.into_inner());
    if !event.done
        && event.pending_events.is_empty()
        && event
            .actions
            .iter()
            .all(|a| matches!(a, ZmodemAction::EmitEvent(ZmodemEvent::Progress { .. })))
    {
        for action in event.actions {
            let ZmodemAction::EmitEvent(progress) = action else {
                continue;
            };
            if state
                .progress
                .as_ref()
                .is_some_and(|previous| progress_name(previous) != progress_name(&progress))
            {
                while state.events.len() >= EVENT_CAPACITY
                    && !state.closed
                    && !shared.cancelled.load(Ordering::Acquire)
                {
                    state = shared
                        .changed
                        .wait(state)
                        .unwrap_or_else(|p| p.into_inner());
                }
                if state.closed {
                    return false;
                }
                if let Some(previous) = state.progress.take() {
                    if state.events.len() >= EVENT_CAPACITY {
                        state
                            .events
                            .back_mut()
                            .expect("full event queue")
                            .actions
                            .push(ZmodemAction::EmitEvent(previous));
                    } else {
                        state.events.push_back(ZmodemRuntimeEvent {
                            actions: vec![ZmodemAction::EmitEvent(previous)],
                            done: false,
                            pending_events: VecDeque::new(),
                        });
                    }
                }
            }
            state.progress = Some(progress);
        }
    } else {
        while state.events.len() >= EVENT_CAPACITY
            && !state.closed
            && !shared.cancelled.load(Ordering::Acquire)
        {
            state = shared
                .changed
                .wait(state)
                .unwrap_or_else(|p| p.into_inner());
        }
        if state.closed {
            return false;
        }
        if let Some(progress) = state.progress.take() {
            event.actions.insert(0, ZmodemAction::EmitEvent(progress));
        }
        if state.events.len() >= EVENT_CAPACITY {
            // A cancel must never wait behind lifecycle backpressure. Preserve
            // ordering by appending the terminal result to the last envelope.
            let last = state.events.back_mut().expect("full event queue");
            last.actions.append(&mut event.actions);
            last.pending_events.append(&mut event.pending_events);
            last.done |= event.done;
        } else {
            state.events.push_back(event);
        }
    }
    let wake = state.wake.clone();
    drop(state);
    if let Some(wake) = wake {
        wake();
    }
    true
}

fn send_actions(
    manager: &SessionManager,
    session: &str,
    shared: &Shared,
    actions: Vec<ZmodemAction>,
) -> Result<Vec<ZmodemAction>, String> {
    let mut events = Vec::new();
    let aborting = actions
        .iter()
        .any(|action| matches!(action, ZmodemAction::EmitEvent(ZmodemEvent::Failed { .. })));
    for action in actions {
        match action {
            ZmodemAction::SendToRemote(bytes) => {
                shared.stats.ack_waits.fetch_add(
                    bytes
                        .windows(2)
                        .filter(|pair| *pair == [0x18, b'k'])
                        .count() as u64,
                    Ordering::Relaxed,
                );
                let start = Instant::now();
                let result = if aborting || shared.cancelled.load(Ordering::Acquire) {
                    manager.write_raw(session, &bytes).map(|()| Duration::ZERO)
                } else {
                    manager.write_raw_cancellable_measured(session, &bytes, &shared.cancelled)
                };
                shared.stats.submit_ns.fetch_add(
                    start.elapsed().as_nanos().min(u64::MAX as u128) as u64,
                    Ordering::Relaxed,
                );
                if let Ok(waited) = &result {
                    shared.stats.capacity_wait.fetch_add(
                        waited.as_nanos().min(u64::MAX as u128) as u64,
                        Ordering::Relaxed,
                    );
                }
                if aborting || shared.cancelled.load(Ordering::Acquire) {
                    if result.is_ok() {
                        shared
                            .stats
                            .wire
                            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
                    }
                } else {
                    result.map_err(|error| error.to_string())?;
                    shared
                        .stats
                        .wire
                        .fetch_add(bytes.len() as u64, Ordering::Relaxed);
                }
            }
            event => events.push(event),
        }
    }
    Ok(events)
}

fn run(
    mut transfer: ZmodemTransfer,
    manager: &SessionManager,
    session: &str,
    consumer: SessionEventConsumerId,
    shared: &Shared,
) {
    let mut pending;
    let acceptance;
    {
        let mut state = shared.state.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if state.closed {
                return;
            }
            if state.bootstrap.is_none() {
                state = shared
                    .changed
                    .wait_timeout(state, IDLE_WAIT)
                    .unwrap_or_else(|p| p.into_inner())
                    .0;
                continue;
            }
            if shared.cancelled.load(Ordering::Acquire) {
                let reason = state
                    .cancel_reason
                    .take()
                    .unwrap_or_else(|| "cancelled".into());
                let bootstrap = state.bootstrap.take().unwrap_or_default();
                drop(state);
                let actions = transfer.cancel_with_reason(reason);
                let events =
                    send_actions(manager, session, shared, actions).unwrap_or_else(|reason| {
                        vec![ZmodemAction::EmitEvent(ZmodemEvent::Failed { reason })]
                    });
                publish(
                    shared,
                    ZmodemRuntimeEvent {
                        actions: events,
                        done: true,
                        pending_events: bootstrap,
                    },
                );
                return;
            }
            if state.bootstrap.is_some() && state.acceptance.is_some() {
                pending = state.bootstrap.take().unwrap();
                acceptance = state.acceptance.take().unwrap();
                break;
            }
            state = shared
                .changed
                .wait_timeout(state, IDLE_WAIT)
                .unwrap_or_else(|p| p.into_inner())
                .0;
            if manager.event_queue.session_ended(session)
                || state.bootstrap.as_ref().is_some_and(|events| {
                    events.iter().any(|event| {
                        matches!(
                            event,
                            SessionEvent::Exited { .. } | SessionEvent::Error { .. }
                        )
                    })
                })
            {
                shared.cancelled.store(true, Ordering::Release);
                state.cancel_reason = Some("session disconnected".into());
            }
        }
    }
    let initial = transfer.take_initial_input();
    // Detection ran on GPUI, which already applied the recording policy to
    // these bytes. Only newly dequeued protocol input needs the observer.
    let mut observer_skip = initial.len();
    if !initial.is_empty() {
        pending.push_front(SessionEvent::Output {
            session_id: session.into(),
            data: initial,
        });
    }
    let actions = match acceptance {
        Acceptance::Download(directory) => transfer.accept_download(directory),
        Acceptance::Upload(files) => transfer.accept_upload(files),
    };
    let mut actions = actions;
    let mut current: Option<(Vec<u8>, usize)> = None;
    let mut more = false;
    loop {
        if shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .closed
        {
            return;
        }
        if shared.cancelled.load(Ordering::Acquire) && !transfer.is_done() {
            let reason = shared
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .cancel_reason
                .take()
                .unwrap_or_else(|| "cancelled".into());
            actions = transfer.cancel_with_reason(reason);
        }
        let events = match send_actions(manager, session, shared, std::mem::take(&mut actions)) {
            Ok(events) => events,
            Err(reason) => {
                let abort = transfer.cancel_with_reason(reason);
                send_actions(manager, session, shared, abort).unwrap_or_default()
            }
        };
        if transfer.is_done() {
            if let Some((bytes, offset)) = current.take()
                && offset < bytes.len()
            {
                pending.push_front(SessionEvent::Output {
                    session_id: session.into(),
                    data: bytes[offset..].to_vec(),
                });
            }
            if events
                .iter()
                .any(|a| matches!(a, ZmodemAction::EmitEvent(ZmodemEvent::Complete { .. })))
            {
                let finish = strip_finish_trailer(
                    manager,
                    session,
                    consumer,
                    shared,
                    transfer.direction(),
                    &mut pending,
                );
                let skip = observer_skip.min(finish.len());
                let observer = shared
                    .state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .input_observer
                    .clone();
                if let Some(observer) = observer
                    && finish.len() > skip
                {
                    observer(session, &finish[skip..]);
                }
                shared
                    .stats
                    .input
                    .fetch_add(finish.len() as u64, Ordering::Relaxed);
            }
            publish(
                shared,
                ZmodemRuntimeEvent {
                    actions: events,
                    done: true,
                    pending_events: pending,
                },
            );
            return;
        }
        if !events.is_empty()
            && !publish(
                shared,
                ZmodemRuntimeEvent {
                    actions: events,
                    done: false,
                    pending_events: VecDeque::new(),
                },
            )
        {
            return;
        }
        if current.is_some() || more {
            let input = current
                .as_ref()
                .map_or(&[][..], |(bytes, offset)| &bytes[*offset..]);
            let input = &input[..input.len().min(INPUT_BATCH)];
            let advance = transfer.advance(input);
            let observer = shared
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .input_observer
                .clone();
            let skip = observer_skip.min(advance.consumed);
            observer_skip -= skip;
            if let Some(observer) = observer
                && advance.consumed > skip
            {
                observer(session, &input[skip..advance.consumed]);
            }
            shared.stats.turns.fetch_add(1, Ordering::Relaxed);
            shared
                .stats
                .input
                .fetch_add(advance.consumed as u64, Ordering::Relaxed);
            if let Some((bytes, offset)) = current.as_mut() {
                *offset += advance.consumed;
                if *offset == bytes.len() {
                    current = None;
                } else if advance.consumed == 0 && !advance.more && !transfer.is_done() {
                    actions = transfer.cancel_with_reason("ZMODEM decoder made no progress");
                    continue;
                }
            }
            more = advance.more;
            actions = advance.actions;
            continue;
        }
        if pending.is_empty() {
            let drain = manager.drain_events_blocking_for_consumer_with_output_budget(
                consumer,
                256,
                INPUT_BATCH,
                IDLE_WAIT,
            );
            match drain {
                Ok(drain) => {
                    pending.extend(drain.events);
                }
                Err(error) => {
                    actions = transfer.cancel_with_reason(error.to_string());
                    continue;
                }
            }
            if manager.event_queue.session_cancelled(session) {
                actions = transfer.cancel_with_reason("session disconnected");
                continue;
            }
        }
        if let Some(event) = pending.pop_front() {
            match event {
                SessionEvent::Output { data, .. } => current = Some((data, 0)),
                event @ (SessionEvent::Exited { .. }
                | SessionEvent::OutputDropped { .. }
                | SessionEvent::Error { .. }) => {
                    pending.push_front(event);
                    actions =
                        transfer.cancel_with_reason("session disconnected or output interrupted");
                }
                event => {
                    publish(
                        shared,
                        ZmodemRuntimeEvent {
                            actions: Vec::new(),
                            done: false,
                            pending_events: VecDeque::from([event]),
                        },
                    );
                }
            }
        }
    }
}

enum FinishStep {
    Pending,
    Complete,
    Mismatch,
}

struct FinishTrailer {
    direction: ZmodemDirection,
    bytes: Vec<u8>,
    matched: usize,
}

impl FinishTrailer {
    fn new(direction: ZmodemDirection) -> Self {
        Self {
            direction,
            bytes: Vec::with_capacity(5),
            matched: 0,
        }
    }

    fn push(&mut self, byte: u8) -> FinishStep {
        // ZHEX parsing stops before CR/LF/XON. lrzsz can set the high bit
        // on those trailer bytes; keep the originals for recording/replay.
        let tokens: &[&[u8]] = match self.direction {
            ZmodemDirection::Upload => &[b"\r\n", b"\r\n\x11", b"\n", b"\n\x11"],
            ZmodemDirection::Download => &[
                b"OO",
                b"\r\nOO",
                b"\r\n\x11OO",
                b"\nOO",
                b"\n\x11OO",
                b"\x11OO",
            ],
        };
        self.bytes.push(byte);
        let mut complete = false;
        let mut partial = false;
        for token in tokens {
            if token.len() < self.bytes.len()
                || !token.iter().zip(&self.bytes).all(|(expected, actual)| {
                    let normalized = match actual {
                        0x8d => b'\r',
                        0x8a => b'\n',
                        0x91 => 0x11,
                        _ => *actual,
                    };
                    *expected == normalized
                })
            {
                continue;
            }
            complete |= token.len() == self.bytes.len();
            partial |= token.len() > self.bytes.len();
        }
        if complete {
            self.matched = self.bytes.len();
        }
        if partial {
            FinishStep::Pending
        } else if complete {
            FinishStep::Complete
        } else {
            // The lookahead belongs to the terminal and stays in its event.
            self.bytes.pop();
            FinishStep::Mismatch
        }
    }
}

fn strip_finish_trailer(
    manager: &SessionManager,
    session: &str,
    consumer: SessionEventConsumerId,
    shared: &Shared,
    direction: ZmodemDirection,
    pending: &mut VecDeque<SessionEvent>,
) -> Vec<u8> {
    let deadline = Instant::now() + FINISH_WAIT;
    strip_finish_trailer_with(
        session,
        direction,
        pending,
        || {
            Instant::now() < deadline
                && !shared.cancelled.load(Ordering::Acquire)
                && !shared
                    .state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .closed
                && !manager.event_queue.session_cancelled(session)
                && !manager.event_queue.session_ended(session)
        },
        || {
            manager
                .drain_events_blocking_for_consumer_with_output_budget(
                    consumer,
                    256,
                    INPUT_BATCH,
                    IDLE_WAIT.min(deadline.saturating_duration_since(Instant::now())),
                )
                .ok()
                .map(|drain| drain.events.into())
        },
    )
}

// Inject waiting and event delivery so fragmented arrivals can be tested without
// assuming the OS schedules a producer within the protocol's finish deadline.
fn strip_finish_trailer_with(
    session: &str,
    direction: ZmodemDirection,
    pending: &mut VecDeque<SessionEvent>,
    mut keep_waiting: impl FnMut() -> bool,
    mut drain: impl FnMut() -> Option<VecDeque<SessionEvent>>,
) -> Vec<u8> {
    let mut trailer = FinishTrailer::new(direction);
    loop {
        if !keep_waiting() {
            break;
        }
        if pending.is_empty() {
            let Some(events) = drain() else {
                break;
            };
            pending.extend(events);
            continue;
        }
        let Some(SessionEvent::Output { session_id, data }) = pending.front() else {
            break;
        };
        if session_id != session {
            break;
        }
        let mut used = 0;
        let mut done = false;
        for byte in data {
            match trailer.push(*byte) {
                FinishStep::Pending => used += 1,
                FinishStep::Complete => {
                    used += 1;
                    done = true;
                    break;
                }
                FinishStep::Mismatch => {
                    done = true;
                    break;
                }
            }
        }
        if used == data.len() {
            pending.pop_front();
        } else if used > 0
            && let Some(SessionEvent::Output { data, .. }) = pending.front_mut()
        {
            data.drain(..used);
        }
        if done {
            break;
        }
    }
    // Upload may have a complete newline while waiting for optional XON.
    // Download commits nothing until both O bytes have arrived.
    let replay = trailer.bytes.split_off(trailer.matched);
    if !replay.is_empty() {
        pending.push_front(SessionEvent::Output {
            session_id: session.into(),
            data: replay,
        });
    }
    trailer.bytes
}
#[cfg(test)]
mod tests;
