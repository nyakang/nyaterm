use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::{SessionDrain, SessionDrainStats, SessionEvent, SessionEventConsumerId};

pub(super) const SESSION_EVENT_QUEUE_OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
pub(super) const SESSION_EVENT_QUEUE_OUTPUT_LOW_WATERMARK: usize =
    SESSION_EVENT_QUEUE_OUTPUT_LIMIT / 2;
pub(super) const SESSION_EVENT_QUEUE_OUTPUT_EVENT_LIMIT: usize = 256 * 1024;

#[derive(Clone)]
pub(super) struct SessionEventQueue {
    shared: Arc<SessionEventQueueShared>,
}

struct SessionEventQueueShared {
    inner: Mutex<SessionEventQueueInner>,
    /// Serializes complete producer events so splitting a large output event
    /// cannot interleave its chunks with another producer. The turn lives in
    /// `inner` so cancellation can wake a waiter without acquiring the turn.
    producer_order: Condvar,
    /// Signalled on every push so a consumer can park instead of polling.
    ready: Condvar,
    /// Signalled after a drain crosses the low watermark, or when cancellation
    /// makes a producer ineligible to enqueue more output.
    output_space: Condvar,
    async_changed: tokio::sync::Notify,
}

#[derive(Default)]
struct SessionEventQueueInner {
    events: VecDeque<SessionEvent>,
    queued_output_bytes: usize,
    peak_output_bytes: usize,
    output_backpressured: bool,
    producer_active: bool,
    closed: bool,
    cancelled_sessions: HashSet<String>,
    consumers: HashMap<String, SessionEventConsumerId>,
    #[cfg(test)]
    waiting_output_producers: usize,
    #[cfg(test)]
    waiting_consumers: usize,
}

struct ProducerOrderGuard<'a> {
    shared: &'a SessionEventQueueShared,
}

impl Drop for ProducerOrderGuard<'_> {
    fn drop(&mut self) {
        let Ok(mut inner) = self.shared.inner.lock() else {
            return;
        };
        inner.producer_active = false;
        drop(inner);
        self.shared.producer_order.notify_all();
        self.shared.async_changed.notify_waiters();
    }
}

impl SessionEventQueue {
    pub(super) fn peak_output_bytes(&self) -> usize {
        self.shared
            .inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .peak_output_bytes
    }

    pub(super) fn session_cancelled(&self, session_id: &str) -> bool {
        let inner = self.shared.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner.closed || inner.cancelled_sessions.contains(session_id)
    }

    /// Inspect lifecycle without removing data while a file picker owns input.
    pub(super) fn session_ended(&self, session_id: &str) -> bool {
        let inner = self.shared.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner.closed
            || inner.cancelled_sessions.contains(session_id)
            || inner.events.iter().any(|event| {
                matches!(
                    event,
                    SessionEvent::Exited { .. } | SessionEvent::Error { .. }
                ) && event_session_id(event) == session_id
            })
    }

    pub(super) fn new() -> Self {
        Self {
            shared: Arc::new(SessionEventQueueShared {
                inner: Mutex::new(SessionEventQueueInner::default()),
                producer_order: Condvar::new(),
                ready: Condvar::new(),
                output_space: Condvar::new(),
                async_changed: tokio::sync::Notify::new(),
            }),
        }
    }

    pub(super) fn consumer_for(&self, session_id: &str) -> Option<SessionEventConsumerId> {
        self.shared
            .inner
            .lock()
            .ok()?
            .consumers
            .get(session_id)
            .copied()
    }

    pub(super) fn push_cleanup_failure(&self, event: SessionEvent) {
        let mut inner = self.shared.inner.lock().unwrap_or_else(|p| p.into_inner());
        if inner.closed {
            return;
        }
        // At most one bounded diagnostic per failed cleanup operation. No user
        // output is retained after cancellation.
        if inner
            .events
            .iter()
            .filter(|event| matches!(event, SessionEvent::CleanupFailed { .. }))
            .count()
            >= 256
        {
            return;
        }
        inner.events.push_back(event);
        self.shared.ready.notify_all();
    }

    pub(super) fn push(&self, event: SessionEvent) {
        let session_id = event_session_id(&event).to_string();
        let Some(_producer_order) = self.begin_push(&session_id) else {
            return;
        };
        match event {
            SessionEvent::Output { session_id, data } => {
                for chunk in data.chunks(SESSION_EVENT_QUEUE_OUTPUT_EVENT_LIMIT) {
                    if chunk.is_empty() {
                        continue;
                    }
                    if !self.push_output_chunk(session_id.clone(), chunk.to_vec()) {
                        return;
                    }
                }
            }
            other => {
                let Ok(mut inner) = self.shared.inner.lock() else {
                    return;
                };
                if !inner.accepts(&session_id) {
                    return;
                }
                inner.events.push_back(other);
                drop(inner);
                self.shared.ready.notify_all();
            }
        }
    }

    /// The SSH receive task must park without blocking the Tokio executor. It
    /// shares the synchronous producers' ordering turn and byte accounting.
    pub(super) async fn push_async(&self, event: SessionEvent) {
        let session_id = event_session_id(&event).to_string();
        let _turn = loop {
            let changed = self.shared.async_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let acquired = {
                let mut inner = self.shared.inner.lock().unwrap_or_else(|p| p.into_inner());
                if !inner.accepts(&session_id) {
                    return;
                }
                if inner.producer_active {
                    false
                } else {
                    inner.producer_active = true;
                    true
                }
            };
            if acquired {
                break ProducerOrderGuard {
                    shared: &self.shared,
                };
            }
            changed.await;
        };
        match event {
            SessionEvent::Output { session_id, data } => {
                for chunk in data.chunks(SESSION_EVENT_QUEUE_OUTPUT_EVENT_LIMIT) {
                    if chunk.is_empty() {
                        continue;
                    }
                    loop {
                        let changed = self.shared.async_changed.notified();
                        tokio::pin!(changed);
                        changed.as_mut().enable();
                        let pushed = {
                            let mut inner =
                                self.shared.inner.lock().unwrap_or_else(|p| p.into_inner());
                            if !inner.accepts(&session_id) {
                                return;
                            }
                            let fits = inner.queued_output_bytes.saturating_add(chunk.len())
                                <= SESSION_EVENT_QUEUE_OUTPUT_LIMIT;
                            if !inner.output_backpressured && fits {
                                inner.queued_output_bytes += chunk.len();
                                inner.peak_output_bytes =
                                    inner.peak_output_bytes.max(inner.queued_output_bytes);
                                inner.events.push_back(SessionEvent::Output {
                                    session_id: session_id.clone(),
                                    data: chunk.to_vec(),
                                });
                                inner.output_backpressured =
                                    inner.queued_output_bytes >= SESSION_EVENT_QUEUE_OUTPUT_LIMIT;
                                true
                            } else {
                                inner.output_backpressured = true;
                                false
                            }
                        };
                        if pushed {
                            self.shared.ready.notify_all();
                            break;
                        }
                        changed.await;
                    }
                }
            }
            other => {
                let mut inner = self.shared.inner.lock().unwrap_or_else(|p| p.into_inner());
                if inner.accepts(&session_id) {
                    inner.events.push_back(other);
                }
                drop(inner);
                self.shared.ready.notify_all();
            }
        }
    }

    fn begin_push(&self, session_id: &str) -> Option<ProducerOrderGuard<'_>> {
        let Ok(mut inner) = self.shared.inner.lock() else {
            return None;
        };
        loop {
            if !inner.accepts(session_id) {
                return None;
            }
            if !inner.producer_active {
                inner.producer_active = true;
                return Some(ProducerOrderGuard {
                    shared: &self.shared,
                });
            }
            let Ok(waited) = self.shared.producer_order.wait(inner) else {
                return None;
            };
            inner = waited;
        }
    }

    fn push_output_chunk(&self, session_id: String, data: Vec<u8>) -> bool {
        let Ok(mut inner) = self.shared.inner.lock() else {
            return false;
        };
        loop {
            if !inner.accepts(&session_id) {
                return false;
            }
            let fits = inner.queued_output_bytes.saturating_add(data.len())
                <= SESSION_EVENT_QUEUE_OUTPUT_LIMIT;
            if !inner.output_backpressured && fits {
                inner.queued_output_bytes = inner.queued_output_bytes.saturating_add(data.len());
                inner.peak_output_bytes = inner.peak_output_bytes.max(inner.queued_output_bytes);
                inner
                    .events
                    .push_back(SessionEvent::Output { session_id, data });
                if inner.queued_output_bytes >= SESSION_EVENT_QUEUE_OUTPUT_LIMIT {
                    inner.output_backpressured = true;
                }
                drop(inner);
                self.shared.ready.notify_all();
                return true;
            }
            inner.output_backpressured = true;
            inner.output_wait_started();
            let waited = self.shared.output_space.wait(inner);
            let Ok(mut waited) = waited else {
                return false;
            };
            waited.output_wait_finished();
            inner = waited;
        }
    }

    pub(super) fn cancel_session(&self, session_id: &str) {
        let Ok(mut inner) = self.shared.inner.lock() else {
            return;
        };
        inner.cancelled_sessions.insert(session_id.to_string());
        inner.consumers.remove(session_id);
        let removed_output_bytes = inner
            .events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::Output {
                    session_id: event_session_id,
                    data,
                } if event_session_id == session_id => Some(data.len()),
                _ => None,
            })
            .sum::<usize>();
        inner
            .events
            .retain(|event| event_session_id(event) != session_id);
        inner.queued_output_bytes = inner
            .queued_output_bytes
            .saturating_sub(removed_output_bytes);
        inner.resume_output_if_below_low_watermark();
        drop(inner);
        self.shared.producer_order.notify_all();
        self.shared.output_space.notify_all();
        self.shared.async_changed.notify_waiters();
        self.shared.ready.notify_all();
    }

    /// Lift a previous `cancel_session` for a session id that legitimately
    /// comes back. tmux pane virtual sessions respawn under the same
    /// `<host>::pane<N>` id on reattach, window re-activation, and layout
    /// churn — keeping them in `cancelled_sessions` would make `accepts`
    /// silently refuse every subsequent push. Producers parked in
    /// `begin_push` are woken so they re-check `accepts`.
    pub(super) fn resume_session(&self, session_id: &str) {
        if session_id.is_empty() {
            return;
        }
        let Ok(mut inner) = self.shared.inner.lock() else {
            return;
        };
        let resumed = inner.cancelled_sessions.remove(session_id);
        drop(inner);
        if resumed {
            self.shared.producer_order.notify_all();
            self.shared.async_changed.notify_waiters();
        }
    }

    pub(super) fn assign_consumer(&self, session_id: &str, consumer_id: SessionEventConsumerId) {
        if session_id.is_empty() {
            return;
        }
        let Ok(mut inner) = self.shared.inner.lock() else {
            return;
        };
        inner.consumers.insert(session_id.to_string(), consumer_id);
        drop(inner);
        self.shared.ready.notify_all();
    }

    pub(super) fn clear_consumer(&self, session_id: &str, consumer_id: SessionEventConsumerId) {
        let Ok(mut inner) = self.shared.inner.lock() else {
            return;
        };
        if inner.consumers.get(session_id) == Some(&consumer_id) {
            inner.consumers.remove(session_id);
        }
        drop(inner);
        self.shared.ready.notify_all();
    }

    pub(super) fn close(&self) {
        let Ok(mut inner) = self.shared.inner.lock() else {
            return;
        };
        inner.closed = true;
        drop(inner);
        self.shared.producer_order.notify_all();
        self.shared.output_space.notify_all();
        self.shared.async_changed.notify_waiters();
        self.shared.ready.notify_all();
    }

    pub(super) fn drain(&self, max_events: usize) -> SessionDrain {
        self.drain_with_output_budget(max_events, None)
    }

    pub(super) fn drain_with_output_budget(
        &self,
        max_events: usize,
        max_output_bytes: Option<usize>,
    ) -> SessionDrain {
        let Ok(mut inner) = self.shared.inner.lock() else {
            return SessionDrain::default();
        };
        let drain = inner.drain(max_events, max_output_bytes);
        let resumed = inner.resume_output_if_below_low_watermark();
        drop(inner);
        if resumed {
            self.shared.output_space.notify_all();
            self.shared.async_changed.notify_waiters();
        }
        drain
    }

    /// Drain, parking up to `timeout` for the first event rather than returning
    /// empty. Lets a dedicated consumer thread wake on the producer's push
    /// instead of sleeping on a fixed interval and eating the latency.
    ///
    /// The timeout still bounds the park so a caller keeps its shutdown flag
    /// and periodic bookkeeping on schedule. A globally closed empty queue
    /// returns immediately, including when close races with entering the wait.
    pub(super) fn drain_blocking_with_output_budget(
        &self,
        max_events: usize,
        max_output_bytes: Option<usize>,
        timeout: Duration,
    ) -> SessionDrain {
        let wait_started = Instant::now();
        let Ok(mut inner) = self.shared.inner.lock() else {
            return SessionDrain::default();
        };
        while inner.events.is_empty() && !inner.closed {
            let remaining = timeout.saturating_sub(wait_started.elapsed());
            if remaining.is_zero() {
                break;
            }
            inner.consumer_wait_started();
            let waited = self.shared.ready.wait_timeout(inner, remaining);
            let Ok((mut waited, wait_result)) = waited else {
                return SessionDrain::default();
            };
            waited.consumer_wait_finished();
            inner = waited;
            if wait_result.timed_out() {
                break;
            }
        }
        let drain = inner.drain(max_events, max_output_bytes);
        let resumed = inner.resume_output_if_below_low_watermark();
        drop(inner);
        if resumed {
            self.shared.output_space.notify_all();
            self.shared.async_changed.notify_waiters();
        }
        drain
    }

    pub(super) fn drain_blocking_for_consumer_with_output_budget(
        &self,
        consumer_id: SessionEventConsumerId,
        max_events: usize,
        max_output_bytes: Option<usize>,
        timeout: Duration,
    ) -> SessionDrain {
        let Ok(inner) = self.shared.inner.lock() else {
            return SessionDrain::default();
        };
        let Some(mut inner) = self.wait_for_consumer(inner, consumer_id, timeout) else {
            return SessionDrain::default();
        };
        let drain = inner.drain_for_consumer(consumer_id, max_events, max_output_bytes);
        let resumed = inner.resume_output_if_below_low_watermark();
        drop(inner);
        if resumed {
            self.shared.output_space.notify_all();
            self.shared.async_changed.notify_waiters();
        }
        drain
    }

    pub(super) fn wait_for_events_for_consumer(
        &self,
        consumer_id: SessionEventConsumerId,
        timeout: Duration,
    ) -> bool {
        let Ok(inner) = self.shared.inner.lock() else {
            return false;
        };
        self.wait_for_consumer(inner, consumer_id, timeout)
            .is_some_and(|inner| inner.has_event_for_consumer(consumer_id))
    }

    fn wait_for_consumer<'a>(
        &self,
        mut inner: MutexGuard<'a, SessionEventQueueInner>,
        consumer_id: SessionEventConsumerId,
        timeout: Duration,
    ) -> Option<MutexGuard<'a, SessionEventQueueInner>> {
        let wait_started = Instant::now();
        while !inner.has_event_for_consumer(consumer_id) && !inner.closed {
            let remaining = timeout.saturating_sub(wait_started.elapsed());
            if remaining.is_zero() {
                break;
            }
            inner.consumer_wait_started();
            let waited = self.shared.ready.wait_timeout(inner, remaining);
            let Ok((mut waited, wait_result)) = waited else {
                return None;
            };
            waited.consumer_wait_finished();
            inner = waited;
            if wait_result.timed_out() {
                break;
            }
        }
        Some(inner)
    }

    #[cfg(test)]
    pub(super) fn waiting_output_producers(&self) -> usize {
        self.shared
            .inner
            .lock()
            .map(|inner| inner.waiting_output_producers)
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(super) fn waiting_consumers(&self) -> usize {
        self.shared
            .inner
            .lock()
            .map(|inner| inner.waiting_consumers)
            .unwrap_or_default()
    }
}

impl SessionEventQueueInner {
    fn event_belongs_to_consumer(
        &self,
        event: &SessionEvent,
        consumer_id: SessionEventConsumerId,
    ) -> bool {
        if let SessionEvent::CleanupFailed {
            consumer_id: owner, ..
        } = event
        {
            return *owner == Some(consumer_id);
        }
        self.consumers.get(event_session_id(event)) == Some(&consumer_id)
    }

    fn has_event_for_consumer(&self, consumer_id: SessionEventConsumerId) -> bool {
        self.events
            .iter()
            .any(|event| self.event_belongs_to_consumer(event, consumer_id))
    }

    fn output_wait_started(&mut self) {
        #[cfg(test)]
        {
            self.waiting_output_producers = self.waiting_output_producers.saturating_add(1);
        }
    }

    fn output_wait_finished(&mut self) {
        #[cfg(test)]
        {
            self.waiting_output_producers = self.waiting_output_producers.saturating_sub(1);
        }
    }

    fn consumer_wait_started(&mut self) {
        #[cfg(test)]
        {
            self.waiting_consumers = self.waiting_consumers.saturating_add(1);
        }
    }

    fn consumer_wait_finished(&mut self) {
        #[cfg(test)]
        {
            self.waiting_consumers = self.waiting_consumers.saturating_sub(1);
        }
    }

    fn accepts(&self, session_id: &str) -> bool {
        !self.closed && !self.cancelled_sessions.contains(session_id)
    }

    fn resume_output_if_below_low_watermark(&mut self) -> bool {
        if self.output_backpressured
            && self.queued_output_bytes <= SESSION_EVENT_QUEUE_OUTPUT_LOW_WATERMARK
        {
            self.output_backpressured = false;
            true
        } else {
            false
        }
    }

    fn drain(&mut self, max_events: usize, max_output_bytes: Option<usize>) -> SessionDrain {
        let mut events = Vec::new();
        let mut stats = SessionDrainStats::default();
        for _ in 0..max_events {
            if let Some(max_output_bytes) = max_output_bytes {
                if stats.drained_output_bytes >= max_output_bytes
                    && stats.drained_events > 0
                    && matches!(self.events.front(), Some(SessionEvent::Output { .. }))
                {
                    break;
                }
                let remaining_output_budget =
                    max_output_bytes.saturating_sub(stats.drained_output_bytes);
                if remaining_output_budget == 0
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
                        stats.drained_events = stats.drained_events.saturating_add(1);
                        stats.drained_output_bytes =
                            stats.drained_output_bytes.saturating_add(chunk.len());
                        self.queued_output_bytes =
                            self.queued_output_bytes.saturating_sub(chunk.len());
                        events.push(SessionEvent::Output {
                            session_id,
                            data: chunk,
                        });
                        continue;
                    }
                }
            }
            let Some(event) = self.events.pop_front() else {
                break;
            };
            stats.drained_events = stats.drained_events.saturating_add(1);
            match &event {
                SessionEvent::Output { data, .. } => {
                    stats.drained_output_bytes =
                        stats.drained_output_bytes.saturating_add(data.len());
                    self.queued_output_bytes = self.queued_output_bytes.saturating_sub(data.len());
                }
                SessionEvent::OutputDropped { bytes, .. } => {
                    stats.dropped_output_bytes = stats.dropped_output_bytes.saturating_add(*bytes);
                }
                SessionEvent::CwdChanged { .. }
                | SessionEvent::CommandAccepted { .. }
                | SessionEvent::Exited { .. }
                | SessionEvent::CleanupFailed { .. }
                | SessionEvent::OutputBoundary { .. }
                | SessionEvent::TmuxState { .. }
                | SessionEvent::Error { .. } => {}
            }
            events.push(event);
        }
        stats.queued_events = self.events.len();
        stats.queued_output_bytes = self.queued_output_bytes;
        SessionDrain { events, stats }
    }

    fn drain_for_consumer(
        &mut self,
        consumer_id: SessionEventConsumerId,
        max_events: usize,
        max_output_bytes: Option<usize>,
    ) -> SessionDrain {
        let mut events = Vec::new();
        let mut stats = SessionDrainStats::default();
        while events.len() < max_events {
            let Some(index) = self
                .events
                .iter()
                .position(|event| self.event_belongs_to_consumer(event, consumer_id))
            else {
                break;
            };
            if let Some(max_output_bytes) = max_output_bytes {
                let remaining = max_output_bytes.saturating_sub(stats.drained_output_bytes);
                let is_output = matches!(self.events.get(index), Some(SessionEvent::Output { .. }));
                if remaining == 0 && is_output {
                    break;
                }
                if let Some(SessionEvent::Output { session_id, data }) = self.events.get_mut(index)
                {
                    let take = data.len().min(remaining);
                    if data.len() > take {
                        let remaining_data = data.split_off(take);
                        let chunk = std::mem::replace(data, remaining_data);
                        let session_id = session_id.clone();
                        stats.drained_events = stats.drained_events.saturating_add(1);
                        stats.drained_output_bytes =
                            stats.drained_output_bytes.saturating_add(chunk.len());
                        self.queued_output_bytes =
                            self.queued_output_bytes.saturating_sub(chunk.len());
                        events.push(SessionEvent::Output {
                            session_id,
                            data: chunk,
                        });
                        continue;
                    }
                }
            }
            let Some(event) = self.events.remove(index) else {
                break;
            };
            stats.drained_events = stats.drained_events.saturating_add(1);
            match &event {
                SessionEvent::Output { data, .. } => {
                    stats.drained_output_bytes =
                        stats.drained_output_bytes.saturating_add(data.len());
                    self.queued_output_bytes = self.queued_output_bytes.saturating_sub(data.len());
                }
                SessionEvent::OutputDropped { bytes, .. } => {
                    stats.dropped_output_bytes = stats.dropped_output_bytes.saturating_add(*bytes);
                }
                SessionEvent::CwdChanged { .. }
                | SessionEvent::CommandAccepted { .. }
                | SessionEvent::Exited { .. }
                | SessionEvent::CleanupFailed { .. }
                | SessionEvent::OutputBoundary { .. }
                | SessionEvent::TmuxState { .. }
                | SessionEvent::Error { .. } => {}
            }
            events.push(event);
        }
        stats.queued_events = self
            .events
            .iter()
            .filter(|event| self.event_belongs_to_consumer(event, consumer_id))
            .count();
        stats.queued_output_bytes = self
            .events
            .iter()
            .filter(|event| self.event_belongs_to_consumer(event, consumer_id))
            .filter_map(|event| match event {
                SessionEvent::Output { data, .. } => Some(data.len()),
                _ => None,
            })
            .sum();
        SessionDrain { events, stats }
    }
}

fn event_session_id(event: &SessionEvent) -> &str {
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
