//! Coalesced frame events and wake/backpressure policy.

use super::frame_search::{TerminalFrameSearchPurpose, TerminalFrameSearchResult};
use super::{TerminalFrameActionLinks, TerminalProtocolState, append_terminal_frame_visible_tail};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use nyaterm_terminal::action_links::TerminalActionLinkBuildStats;
use nyaterm_terminal::{TerminalEffects, TerminalSnapshot, TerminalSnapshotBuildStats};
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalFrameSnapshotPurpose {
    Paint,
    ActionLinkEnrichment,
}

#[derive(Clone, Debug)]
pub(crate) enum TerminalFrameEvent {
    CommandNavigation {
        session_id: String,
        result: nyaterm_terminal::command_navigation::CommandNavigationResult,
        frame: TerminalFrameSnapshotEvent,
    },
    Output(TerminalFrameOutputEvent),
    Snapshot(TerminalFrameSnapshotEvent),
    ClearExceptInput(TerminalFrameSnapshotEvent),
    Search(TerminalFrameSearchEvent),
    Rekeyed {
        old_id: String,
        new_id: String,
        success: bool,
    },
}

impl TerminalFrameEvent {
    pub(crate) fn session_id(&self) -> &str {
        match self {
            Self::Output(event) => &event.session_id,
            Self::Snapshot(event) | Self::ClearExceptInput(event) => &event.session_id,
            Self::Search(event) => &event.session_id,
            Self::Rekeyed { new_id, .. } => new_id,
            Self::CommandNavigation { session_id, .. } => session_id,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct TerminalFrameEventQueue {
    pub(super) shared: Arc<TerminalFrameEventQueueShared>,
    pub(super) cap: usize,
    pub(super) wake_tx: Option<UnboundedSender<()>>,
    pub(super) wake_interests: Arc<AtomicU8>,
    pub(super) wake_count: Arc<AtomicU64>,
}

#[derive(Debug)]
pub(super) struct TerminalFrameEventQueueShared {
    pub(super) inner: Mutex<TerminalFrameEventQueueInner>,
    pub(super) space_available: Condvar,
    #[cfg(test)]
    pub(super) blocked_notifier: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}

#[derive(Debug)]
pub(super) struct TerminalFrameEventQueueInner {
    pub(super) events: VecDeque<TerminalFrameEvent>,
    pub(super) closed: bool,
}

pub(super) const TERMINAL_FRAME_EVENT_WAKE_OUTPUT: u8 = 1 << 0;
pub(super) const TERMINAL_FRAME_EVENT_WAKE_SNAPSHOT: u8 = 1 << 1;
pub(super) const TERMINAL_FRAME_EVENT_WAKE_SEARCH: u8 = 1 << 2;
/// What the data-plane drain task declares interest in: every kind, because it
/// applies every kind. Narrowing this strands whichever reply is dropped from it.
pub(super) const TERMINAL_FRAME_EVENT_WAKE_ALL: u8 = TERMINAL_FRAME_EVENT_WAKE_OUTPUT
    | TERMINAL_FRAME_EVENT_WAKE_SNAPSHOT
    | TERMINAL_FRAME_EVENT_WAKE_SEARCH;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TerminalFrameEventQueuePushOutcome {
    Enqueued,
    Closed,
    Poisoned,
}

impl TerminalFrameEventQueue {
    pub(super) fn take_sessions(&self, session_ids: &[String]) -> VecDeque<TerminalFrameEvent> {
        let selected = session_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let Ok(mut inner) = self.shared.inner.lock() else {
            return VecDeque::new();
        };
        let mut moved = VecDeque::new();
        let mut retained = VecDeque::new();
        while let Some(event) = inner.events.pop_front() {
            if selected.contains(event.session_id()) {
                moved.push_back(event);
            } else {
                retained.push_back(event);
            }
        }
        inner.events = retained;
        drop(inner);
        self.shared.space_available.notify_all();
        moved
    }

    #[cfg(test)]
    pub(super) fn new(cap: usize) -> Self {
        Self::build(cap, None)
    }

    pub(super) fn new_with_wake(cap: usize) -> (Self, UnboundedReceiver<()>) {
        let (wake_tx, wake_rx) = unbounded();
        (Self::build(cap, Some(wake_tx)), wake_rx)
    }

    pub(super) fn build(cap: usize, wake_tx: Option<UnboundedSender<()>>) -> Self {
        Self {
            shared: Arc::new(TerminalFrameEventQueueShared {
                inner: Mutex::new(TerminalFrameEventQueueInner {
                    events: VecDeque::with_capacity(cap.min(1024)),
                    closed: false,
                }),
                space_available: Condvar::new(),
                #[cfg(test)]
                blocked_notifier: Mutex::new(None),
            }),
            cap,
            wake_tx,
            wake_interests: Arc::new(AtomicU8::new(0)),
            wake_count: Arc::new(AtomicU64::new(0)),
        }
    }

    pub(super) fn arm_wake(&self, interest: u8) {
        if self.wake_tx.is_some() && interest != 0 {
            self.wake_interests.fetch_or(interest, Ordering::Release);
        }
    }

    pub(super) fn push(&self, mut event: TerminalFrameEvent) -> TerminalFrameEventQueuePushOutcome {
        let wake_interest = terminal_frame_event_wake_interest(&event);
        let Ok(mut inner) = self.shared.inner.lock() else {
            return TerminalFrameEventQueuePushOutcome::Poisoned;
        };
        loop {
            if inner.closed {
                return TerminalFrameEventQueuePushOutcome::Closed;
            }
            compact_terminal_frame_event_queue(&mut inner.events, &mut event);
            if inner.events.len() < self.cap.max(1) {
                inner.events.push_back(event);
                break;
            }
            if let Some(drop_index) = inner
                .events
                .iter()
                .position(terminal_frame_event_can_drop_under_pressure)
            {
                inner.events.remove(drop_index);
                continue;
            }
            #[cfg(test)]
            if let Ok(mut notifier) = self.shared.blocked_notifier.lock()
                && let Some(notifier) = notifier.take()
            {
                let _ = notifier.send(());
            }
            inner = match self.shared.space_available.wait(inner) {
                Ok(inner) => inner,
                Err(_) => return TerminalFrameEventQueuePushOutcome::Poisoned,
            };
        }
        drop(inner);
        if wake_interest != 0
            && self
                .wake_interests
                .fetch_and(!wake_interest, Ordering::AcqRel)
                & wake_interest
                != 0
            && let Some(wake_tx) = &self.wake_tx
            && wake_tx.unbounded_send(()).is_ok()
        {
            self.wake_count.fetch_add(1, Ordering::Relaxed);
        }
        TerminalFrameEventQueuePushOutcome::Enqueued
    }

    #[cfg(test)]
    pub(super) fn notify_when_next_push_blocks(&self) -> std::sync::mpsc::Receiver<()> {
        let (notify_tx, notify_rx) = std::sync::mpsc::channel();
        *self
            .shared
            .blocked_notifier
            .lock()
            .expect("terminal frame event queue blocked notifier poisoned") = Some(notify_tx);
        notify_rx
    }

    #[cfg(test)]
    pub(super) fn try_recv(&self) -> Option<TerminalFrameEvent> {
        let event = self.shared.inner.lock().ok()?.events.pop_front();
        if event.is_some() {
            self.shared.space_available.notify_one();
        }
        event
    }

    pub(super) fn drain_into(
        &self,
        events: &mut VecDeque<TerminalFrameEvent>,
        limit: usize,
    ) -> usize {
        if limit == 0 {
            return 0;
        }
        let Ok(mut inner) = self.shared.inner.lock() else {
            return 0;
        };
        let mut drained = 0usize;
        while drained < limit {
            let Some(event) = inner.events.pop_front() else {
                break;
            };
            events.push_back(event);
            drained += 1;
        }
        drop(inner);
        if drained > 0 {
            self.shared.space_available.notify_all();
        }
        drained
    }

    pub(super) fn close(&self) {
        if let Ok(mut inner) = self.shared.inner.lock() {
            inner.closed = true;
        }
        self.shared.space_available.notify_all();
    }

    #[cfg(test)]
    pub(super) fn is_closed(&self) -> bool {
        self.shared
            .inner
            .lock()
            .map(|inner| inner.closed)
            .unwrap_or(true)
    }

    pub(super) fn len(&self) -> usize {
        self.shared
            .inner
            .lock()
            .map(|inner| inner.events.len())
            .unwrap_or(0)
    }

    pub(super) fn wake_count(&self) -> u64 {
        self.wake_count.load(Ordering::Relaxed)
    }
}

pub(super) fn terminal_frame_event_wake_interest(event: &TerminalFrameEvent) -> u8 {
    match event {
        TerminalFrameEvent::Output(_) => TERMINAL_FRAME_EVENT_WAKE_OUTPUT,
        TerminalFrameEvent::Snapshot(_) | TerminalFrameEvent::ClearExceptInput(_) => {
            TERMINAL_FRAME_EVENT_WAKE_SNAPSHOT
        }
        TerminalFrameEvent::Search(_) => TERMINAL_FRAME_EVENT_WAKE_SEARCH,
        TerminalFrameEvent::Rekeyed { .. } | TerminalFrameEvent::CommandNavigation { .. } => {
            TERMINAL_FRAME_EVENT_WAKE_SNAPSHOT
        }
    }
}

pub(super) fn push_terminal_frame_worker_event(
    event_queue: &TerminalFrameEventQueue,
    event: TerminalFrameEvent,
) {
    if event_queue.push(event) == TerminalFrameEventQueuePushOutcome::Poisoned {
        tracing::error!(
            diagnostic = "terminal_frame_event_queue_poisoned",
            "terminal frame event queue lock was poisoned"
        );
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalFrameOutputEvent {
    pub(crate) session_id: String,
    pub(crate) visible_text: String,
    pub(crate) recording_text_bytes: usize,
    /// Full viewport grid for paint. Worker omits this for low-priority
    /// (hidden) sessions to avoid per-frame snapshot CPU/memory.
    pub(crate) snapshot: Option<Arc<TerminalSnapshot>>,
    pub(crate) action_links: Option<TerminalFrameActionLinks>,
    pub(crate) protocol_state: TerminalProtocolState,
    pub(crate) effects: TerminalEffects,
    pub(crate) command_running: bool,
    pub(crate) accepted_bytes: usize,
    pub(crate) skipped_output_bytes: usize,
    pub(crate) revision: u64,
    pub(crate) snapshot_duration: Duration,
    pub(crate) snapshot_stats: TerminalSnapshotBuildStats,
    pub(crate) process_duration: Duration,
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalFrameSnapshotEvent {
    pub(crate) session_id: String,
    pub(crate) offset: usize,
    pub(crate) snapshot: Arc<TerminalSnapshot>,
    pub(crate) action_links: Option<TerminalFrameActionLinks>,
    pub(crate) revision: u64,
    pub(crate) purpose: TerminalFrameSnapshotPurpose,
    pub(crate) snapshot_duration: Duration,
    pub(crate) snapshot_stats: TerminalSnapshotBuildStats,
    pub(crate) action_link_stats: TerminalActionLinkBuildStats,
    pub(crate) process_duration: Duration,
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalFrameSearchEvent {
    pub(crate) session_id: String,
    pub(crate) purpose: TerminalFrameSearchPurpose,
    pub(crate) result: TerminalFrameSearchResult,
    pub(crate) process_duration: Duration,
}

pub(super) fn compact_terminal_frame_event_queue(
    queue: &mut VecDeque<TerminalFrameEvent>,
    incoming: &mut TerminalFrameEvent,
) {
    let TerminalFrameEvent::Output(incoming) = incoming else {
        return;
    };
    if !terminal_frame_output_event_can_drop_under_pressure(incoming) {
        return;
    }
    let mut replaced = Vec::new();
    let mut index = queue
        .iter()
        .rposition(|event| {
            matches!(event, TerminalFrameEvent::ClearExceptInput(frame)
            if frame.session_id == incoming.session_id)
        })
        .map_or(0, |index| index + 1);
    while index < queue.len() {
        let replace = matches!(queue.get(index), Some(TerminalFrameEvent::Output(queued))
            if queued.session_id == incoming.session_id
                && terminal_frame_output_event_can_drop_under_pressure(queued));
        if replace {
            if let Some(TerminalFrameEvent::Output(queued)) = queue.remove(index) {
                replaced.push(queued);
            }
        } else {
            index += 1;
        }
    }
    for queued in replaced.into_iter().rev() {
        merge_terminal_frame_output_event_into_newer(&queued, incoming);
    }
}

pub(super) fn merge_terminal_frame_output_event_into_newer(
    older: &TerminalFrameOutputEvent,
    newer: &mut TerminalFrameOutputEvent,
) {
    newer.recording_text_bytes = older
        .recording_text_bytes
        .saturating_add(newer.recording_text_bytes);
    newer.accepted_bytes = older.accepted_bytes.saturating_add(newer.accepted_bytes);
    newer.skipped_output_bytes = older
        .skipped_output_bytes
        .saturating_add(newer.skipped_output_bytes);
    let mut visible_text = older.visible_text.clone();
    append_terminal_frame_visible_tail(&mut visible_text, &newer.visible_text);
    newer.visible_text = visible_text;
    newer.process_duration = older
        .process_duration
        .saturating_add(newer.process_duration);
}

pub(super) fn terminal_frame_event_can_drop_under_pressure(event: &TerminalFrameEvent) -> bool {
    match event {
        TerminalFrameEvent::Output(frame) => {
            terminal_frame_output_event_can_drop_under_pressure(frame)
        }
        TerminalFrameEvent::Snapshot(_)
        | TerminalFrameEvent::ClearExceptInput(_)
        | TerminalFrameEvent::Search(_)
        | TerminalFrameEvent::CommandNavigation { .. }
        | TerminalFrameEvent::Rekeyed { .. } => false,
    }
}

pub(super) fn terminal_frame_output_event_can_drop_under_pressure(
    frame: &TerminalFrameOutputEvent,
) -> bool {
    !frame.effects.bell
        && frame.effects.title.is_none()
        && !frame.effects.reset_title
        && frame.effects.cwd.is_none()
        && !frame.effects.shell_command_started
        && !frame.effects.shell_command_finished
        && frame.effects.pty_write.is_empty()
        && frame.effects.clipboard_store.is_none()
        && frame.effects.clipboard_loads.is_empty()
}

pub(super) const TERMINAL_FRAME_EVENT_QUEUE_CAP: usize = 1024;
