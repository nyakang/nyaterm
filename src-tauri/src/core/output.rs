use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::time::{Duration, sleep};

use super::{SessionCommand, SessionCommandSender};

const OUTPUT_FLUSH_INTERVAL_MS: u64 = 4;
const OUTPUT_NORMAL_BATCH_BYTES: usize = 64 * 1024;
const OUTPUT_FLOOD_BATCH_BYTES: usize = 128 * 1024;
const OUTPUT_PAUSE_HIGH_WATERMARK_BYTES: usize = 1024 * 1024;
const OUTPUT_RESUME_LOW_WATERMARK_BYTES: usize = 128 * 1024;
const OUTPUT_CLOSE_FLUSH_MAX_BYTES: usize = 1024 * 1024;
#[cfg(not(test))]
const OUTPUT_CATASTROPHIC_BACKLOG_BYTES: usize = 64 * 1024 * 1024;
#[cfg(test)]
const OUTPUT_CATASTROPHIC_BACKLOG_BYTES: usize = OUTPUT_PAUSE_HIGH_WATERMARK_BYTES + 64 * 1024;

type OutputSink = dyn Fn(TerminalOutputPayload) + Send + Sync + 'static;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOutputPayload {
    pub data: String,
    pub bytes: usize,
    pub dropped_bytes: usize,
}

#[derive(Default)]
struct OutputState {
    attached: bool,
    pending: VecDeque<String>,
    pending_bytes: usize,
    unacked_bytes: usize,
    next_flush_id: u64,
    scheduled_flush_id: Option<u64>,
    flow_paused: bool,
    catastrophic_close_sent: bool,
}

struct FlushResult {
    payload: Option<TerminalOutputPayload>,
    flow_change: Option<bool>,
    reschedule: bool,
}

/// Shared session-output coalescer used by all terminal backends.
///
/// It batches output before emitting it to the webview, tracks bytes emitted
/// but not yet acknowledged by xterm, and pauses the terminal source while the
/// renderer/IPC backlog is too high.
pub struct SessionOutputCoalescer {
    sink: Arc<OutputSink>,
    flow_control_tx: Option<SessionCommandSender>,
    /// Serializes all sink emissions so concurrent producer/attach threads
    /// cannot interleave terminal output chunks out of order.
    emit_lock: Mutex<()>,
    state: Mutex<OutputState>,
}

impl SessionOutputCoalescer {
    pub fn for_app(
        app: AppHandle,
        output_event: String,
        flow_control_tx: SessionCommandSender,
    ) -> Arc<Self> {
        let session_id = output_event
            .strip_prefix("terminal-output-")
            .map(str::to_string);
        let session_manager = app
            .try_state::<Arc<crate::core::SessionManager>>()
            .map(|state| state.inner().clone());
        Self::with_flow_sink(flow_control_tx, move |payload| {
            if let (Some(manager), Some(session_id)) = (&session_manager, &session_id) {
                manager.append_recent_output(session_id, &payload.data);
            }
            let _ = app.emit(&output_event, &payload);
        })
    }

    #[cfg(test)]
    pub fn with_sink<F>(sink: F) -> Arc<Self>
    where
        F: Fn(TerminalOutputPayload) + Send + Sync + 'static,
    {
        Arc::new(Self {
            sink: Arc::new(sink),
            flow_control_tx: None,
            emit_lock: Mutex::new(()),
            state: Mutex::new(OutputState::default()),
        })
    }

    pub fn with_flow_sink<F>(flow_control_tx: SessionCommandSender, sink: F) -> Arc<Self>
    where
        F: Fn(TerminalOutputPayload) + Send + Sync + 'static,
    {
        Arc::new(Self {
            sink: Arc::new(sink),
            flow_control_tx: Some(flow_control_tx),
            emit_lock: Mutex::new(()),
            state: Mutex::new(OutputState::default()),
        })
    }

    fn lock_emit(&self) -> MutexGuard<'_, ()> {
        match self.emit_lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::warn!("session output sink mutex was poisoned; recovering");
                let guard = poisoned.into_inner();
                self.emit_lock.clear_poison();
                guard
            }
        }
    }

    fn emit_payload(&self, payload: TerminalOutputPayload) {
        let _emit_guard = self.lock_emit();
        (self.sink)(payload);
    }

    fn lock_state(&self) -> MutexGuard<'_, OutputState> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                tracing::warn!("session output state mutex was poisoned; recovering");
                let state = poisoned.into_inner();
                self.state.clear_poison();
                state
            }
        }
    }

    pub fn push(self: &Arc<Self>, text: impl AsRef<str>) {
        self.push_owned(text.as_ref().to_string());
    }

    pub fn push_owned(self: &Arc<Self>, text: String) {
        if text.is_empty() {
            return;
        }

        let mut schedule_timer = None;
        let mut flush_now = false;
        let mut close_for_overflow = false;
        let flow_change = {
            let mut state = self.lock_state();
            let was_empty = state.pending.is_empty();
            state.pending_bytes = state.pending_bytes.saturating_add(text.len());
            state.pending.push_back(text);

            let flow_change = update_flow_state(&mut state);
            if should_close_for_catastrophic_backlog(&mut state) {
                close_for_overflow = true;
            }
            if state.attached && state.pending_bytes >= batch_limit(&state) {
                state.next_flush_id = state.next_flush_id.wrapping_add(1);
                state.scheduled_flush_id = None;
                flush_now = true;
            } else if state.attached && was_empty && state.scheduled_flush_id.is_none() {
                state.next_flush_id = state.next_flush_id.wrapping_add(1);
                let flush_id = state.next_flush_id;
                state.scheduled_flush_id = Some(flush_id);
                schedule_timer = Some(flush_id);
            }
            flow_change
        };

        self.send_flow_change(flow_change);
        if close_for_overflow {
            self.send_close_for_overflow();
        }

        if let Some(flush_id) = schedule_timer {
            self.schedule_flush(flush_id);
        }

        if flush_now {
            self.flush_pending();
        }
    }

    pub fn ack(self: &Arc<Self>, bytes: usize) {
        if bytes == 0 {
            return;
        }

        let (flow_change, schedule_timer) = {
            let mut state = self.lock_state();
            state.unacked_bytes = state.unacked_bytes.saturating_sub(bytes);
            let flow_change = update_flow_state(&mut state);
            let schedule_timer = if state.attached
                && !state.pending.is_empty()
                && state.scheduled_flush_id.is_none()
            {
                state.next_flush_id = state.next_flush_id.wrapping_add(1);
                let flush_id = state.next_flush_id;
                state.scheduled_flush_id = Some(flush_id);
                Some(flush_id)
            } else {
                None
            };
            (flow_change, schedule_timer)
        };

        self.send_flow_change(flow_change);
        if let Some(flush_id) = schedule_timer {
            self.schedule_flush(flush_id);
        }
    }

    pub fn is_attached(&self) -> bool {
        self.lock_state().attached
    }

    #[cfg(test)]
    pub fn attach(self: &Arc<Self>) {
        let (ack, _ack_rx) = tokio::sync::oneshot::channel();
        self.attach_confirmed(ack);
    }

    pub fn attach_confirmed(self: &Arc<Self>, ack: tokio::sync::oneshot::Sender<()>) {
        let mut state = self.lock_state();
        state.attached = true;
        state.unacked_bytes = 0;
        state.next_flush_id = state.next_flush_id.wrapping_add(1);
        state.scheduled_flush_id = None;

        // Attach may race PTY push. Serialize against every other sink
        // emission, then hold the state lock while draining; a competing push
        // queues after the detached backlog instead of overtaking it before
        // the renderer settles/publishes title state.
        let emit_guard = self.lock_emit();
        while let Some(payload) = take_pending_batch(&mut state) {
            (self.sink)(payload);
        }
        drop(emit_guard);
        let flow_change = update_flow_state(&mut state);
        drop(state);

        self.send_flow_change(flow_change);
        let _ = ack.send(());
    }

    pub fn detach(self: &Arc<Self>) {
        let flow_change = {
            let mut state = self.lock_state();
            state.attached = false;
            state.unacked_bytes = 0;
            state.next_flush_id = state.next_flush_id.wrapping_add(1);
            state.scheduled_flush_id = None;
            update_flow_state(&mut state)
        };

        self.send_flow_change(flow_change);
    }

    pub fn close(self: &Arc<Self>) {
        let result = {
            let mut state = self.lock_state();
            state.next_flush_id = state.next_flush_id.wrapping_add(1);
            state.scheduled_flush_id = None;
            flush_all_from_state(&mut state)
        };

        self.apply_flush_result(result);
    }

    fn schedule_flush(self: &Arc<Self>, flush_id: u64) {
        let output = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            sleep(Duration::from_millis(OUTPUT_FLUSH_INTERVAL_MS)).await;
            output.flush_if_scheduled(flush_id);
        });
    }

    fn flush_if_scheduled(self: &Arc<Self>, flush_id: u64) {
        let result = {
            let mut state = self.lock_state();
            if state.scheduled_flush_id != Some(flush_id) {
                return;
            }

            state.scheduled_flush_id = None;
            if !state.attached {
                return;
            }

            flush_from_state(&mut state)
        };

        self.apply_flush_result(result);
    }

    fn flush_pending(self: &Arc<Self>) {
        let result = {
            let mut state = self.lock_state();
            if !state.attached {
                return;
            }

            state.next_flush_id = state.next_flush_id.wrapping_add(1);
            state.scheduled_flush_id = None;
            flush_from_state(&mut state)
        };

        self.apply_flush_result(result);
    }

    fn apply_flush_result(self: &Arc<Self>, result: FlushResult) {
        if let Some(payload) = result.payload {
            self.emit_payload(payload);
        }
        self.send_flow_change(result.flow_change);
        if result.reschedule {
            let flush_id = {
                let mut state = self.lock_state();
                if !state.attached || state.pending.is_empty() || state.scheduled_flush_id.is_some()
                {
                    None
                } else {
                    state.next_flush_id = state.next_flush_id.wrapping_add(1);
                    let flush_id = state.next_flush_id;
                    state.scheduled_flush_id = Some(flush_id);
                    Some(flush_id)
                }
            };
            if let Some(flush_id) = flush_id {
                self.schedule_flush(flush_id);
            }
        }
    }

    fn send_flow_change(&self, flow_change: Option<bool>) {
        let Some(paused) = flow_change else {
            return;
        };
        let Some(tx) = &self.flow_control_tx else {
            return;
        };
        let command = if paused {
            SessionCommand::PauseOutput
        } else {
            SessionCommand::ResumeOutput
        };
        let _ = tx.send(command);
    }

    fn send_close_for_overflow(&self) {
        tracing::error!(
            threshold_bytes = OUTPUT_CATASTROPHIC_BACKLOG_BYTES,
            "Closing terminal session after catastrophic output backlog overflow"
        );
        let Some(tx) = &self.flow_control_tx else {
            return;
        };
        let _ = tx.send(SessionCommand::Close);
    }
}

fn flush_from_state(state: &mut OutputState) -> FlushResult {
    let payload = take_pending_batch(state);
    let flow_change = update_flow_state(state);
    FlushResult {
        payload,
        flow_change,
        reschedule: state.attached && !state.pending.is_empty(),
    }
}

fn flush_all_from_state(state: &mut OutputState) -> FlushResult {
    trim_pending_to_max(state, OUTPUT_CLOSE_FLUSH_MAX_BYTES);
    let payload = take_all_pending(state);
    let flow_change = update_flow_state(state);
    FlushResult {
        payload,
        flow_change,
        reschedule: false,
    }
}

fn take_pending_batch(state: &mut OutputState) -> Option<TerminalOutputPayload> {
    if state.pending.is_empty() {
        return None;
    }

    let mut remaining = batch_limit(state);
    let mut data = String::new();

    while remaining > 0 {
        let Some(front) = state.pending.pop_front() else {
            break;
        };

        if front.len() <= remaining {
            remaining -= front.len();
            state.pending_bytes = state.pending_bytes.saturating_sub(front.len());
            data.push_str(&front);
            continue;
        }

        let split_at = byte_boundary_at_or_before(&front, remaining);
        let (head, tail) = front.split_at(split_at);
        data.push_str(head);
        state.pending.push_front(tail.to_string());
        state.pending_bytes = state.pending_bytes.saturating_sub(head.len());
        break;
    }

    if data.is_empty() {
        return None;
    }

    Some(payload_from_data(state, data))
}

fn take_all_pending(state: &mut OutputState) -> Option<TerminalOutputPayload> {
    if state.pending.is_empty() {
        return None;
    }

    let mut data = String::new();
    while let Some(chunk) = state.pending.pop_front() {
        state.pending_bytes = state.pending_bytes.saturating_sub(chunk.len());
        data.push_str(&chunk);
    }

    if data.is_empty() {
        None
    } else {
        Some(payload_from_data(state, data))
    }
}

fn payload_from_data(state: &mut OutputState, data: String) -> TerminalOutputPayload {
    let bytes = data.len();
    state.unacked_bytes = state.unacked_bytes.saturating_add(bytes);
    TerminalOutputPayload {
        data,
        bytes,
        dropped_bytes: 0,
    }
}

fn trim_pending_to_max(state: &mut OutputState, max_bytes: usize) {
    if state.pending_bytes <= max_bytes {
        return;
    }

    let mut bytes_to_drop = state.pending_bytes - max_bytes;
    while bytes_to_drop > 0 {
        let Some(front) = state.pending.pop_front() else {
            break;
        };

        if front.len() <= bytes_to_drop {
            bytes_to_drop -= front.len();
            state.pending_bytes = state.pending_bytes.saturating_sub(front.len());
            continue;
        }

        let split_at = byte_boundary_at_or_before(&front, bytes_to_drop);
        let (dropped, kept) = front.split_at(split_at);
        state.pending.push_front(kept.to_string());
        state.pending_bytes = state.pending_bytes.saturating_sub(dropped.len());
        break;
    }
}

fn update_flow_state(state: &mut OutputState) -> Option<bool> {
    let backlog = state.pending_bytes.saturating_add(state.unacked_bytes);
    if !state.flow_paused && backlog >= OUTPUT_PAUSE_HIGH_WATERMARK_BYTES {
        state.flow_paused = true;
        Some(true)
    } else if state.flow_paused && backlog <= OUTPUT_RESUME_LOW_WATERMARK_BYTES {
        state.flow_paused = false;
        Some(false)
    } else {
        None
    }
}

fn should_close_for_catastrophic_backlog(state: &mut OutputState) -> bool {
    let backlog = state.pending_bytes.saturating_add(state.unacked_bytes);
    if state.catastrophic_close_sent || backlog < OUTPUT_CATASTROPHIC_BACKLOG_BYTES {
        return false;
    }
    state.catastrophic_close_sent = true;
    true
}

fn batch_limit(state: &OutputState) -> usize {
    let backlog = state.pending_bytes.saturating_add(state.unacked_bytes);
    if state.flow_paused || backlog >= OUTPUT_PAUSE_HIGH_WATERMARK_BYTES {
        OUTPUT_FLOOD_BATCH_BYTES
    } else {
        OUTPUT_NORMAL_BATCH_BYTES
    }
}

fn byte_boundary_at_or_before(text: &str, max_bytes: usize) -> usize {
    if max_bytes >= text.len() {
        return text.len();
    }
    let mut index = max_bytes;
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    if index == 0 {
        text.char_indices()
            .nth(1)
            .map(|(next, _)| next)
            .unwrap_or(text.len())
    } else {
        index
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OUTPUT_CATASTROPHIC_BACKLOG_BYTES, OUTPUT_CLOSE_FLUSH_MAX_BYTES, OUTPUT_FLOOD_BATCH_BYTES,
        OUTPUT_NORMAL_BATCH_BYTES, OUTPUT_PAUSE_HIGH_WATERMARK_BYTES,
        OUTPUT_RESUME_LOW_WATERMARK_BYTES, SessionOutputCoalescer, TerminalOutputPayload,
    };
    use crate::core::{SessionCommand, session_command_channel};
    use std::sync::{Arc, Mutex};
    use std::time::Duration as StdDuration;
    use tokio::time::{Duration, Instant, advance, sleep};

    fn collect_sink() -> (
        Arc<Mutex<Vec<TerminalOutputPayload>>>,
        impl Fn(TerminalOutputPayload) + Send + Sync + 'static,
    ) {
        let emitted = Arc::new(Mutex::new(Vec::<TerminalOutputPayload>::new()));
        let sink = emitted.clone();
        (emitted, move |payload| {
            sink.lock().unwrap().push(payload);
        })
    }

    async fn wait_for_emitted_bytes(
        emitted: &Arc<Mutex<Vec<TerminalOutputPayload>>>,
        expected: usize,
    ) -> usize {
        for _ in 0..20 {
            let bytes: usize = emitted
                .lock()
                .unwrap()
                .iter()
                .map(|payload| payload.bytes)
                .sum();
            if bytes >= expected {
                return bytes;
            }
            sleep(Duration::from_millis(10)).await;
        }
        emitted
            .lock()
            .unwrap()
            .iter()
            .map(|payload| payload.bytes)
            .sum()
    }

    #[test]
    fn every_output_state_entrypoint_recovers_from_a_poisoned_mutex() {
        let (_emitted, sink) = collect_sink();
        let output = SessionOutputCoalescer::with_sink(sink);
        let poison = |output: &Arc<SessionOutputCoalescer>| {
            let poisoned_output = output.clone();
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                let _guard = poisoned_output.state.lock().expect("lock output state");
                panic!("poison output state");
            }));
        };

        output.attach();
        poison(&output);
        output.detach();
        assert!(!output.is_attached());

        poison(&output);
        output.attach();
        assert!(output.is_attached());

        poison(&output);
        output.ack(1);
        output.detach();
        poison(&output);
        output.push_owned("queued while detached".to_string());
        assert_eq!(
            output.lock_state().pending_bytes,
            "queued while detached".len()
        );

        poison(&output);
        output.close();
        assert!(!output.state.is_poisoned());
    }

    #[tokio::test]
    async fn timer_flush_batches_pending_output() {
        let (emitted, sink) = collect_sink();
        let output = SessionOutputCoalescer::with_sink(sink);

        output.attach();
        output.push("hello");
        output.push(" world");

        sleep(Duration::from_millis(20)).await;

        let emitted = emitted.lock().unwrap();
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].data, "hello world");
        assert_eq!(emitted[0].bytes, "hello world".len());
    }

    #[tokio::test(start_paused = true)]
    async fn one_millisecond_tiny_bursts_are_coalesced_without_delaying_first_flush() {
        let emitted = Arc::new(Mutex::new(Vec::<TerminalOutputPayload>::new()));
        let first_emit_at = Arc::new(Mutex::new(None::<Duration>));
        let started_at = Instant::now();
        let emitted_sink = emitted.clone();
        let first_emit_sink = first_emit_at.clone();
        let output = SessionOutputCoalescer::with_sink(move |payload| {
            let mut first = first_emit_sink.lock().unwrap();
            if first.is_none() {
                *first = Some(Instant::now().duration_since(started_at));
            }
            emitted_sink.lock().unwrap().push(payload);
        });

        output.attach();

        let mut expected = String::new();
        for index in 0..1000 {
            let chunk = format!("{index:04};");
            expected.push_str(&chunk);
            output.push_owned(chunk);
            advance(Duration::from_millis(1)).await;
        }
        advance(Duration::from_millis(20)).await;

        let emitted = emitted.lock().unwrap();
        let actual = emitted
            .iter()
            .map(|payload| payload.data.as_str())
            .collect::<String>();
        let first_emit_at = first_emit_at.lock().unwrap().expect("first emit");

        assert_eq!(actual, expected);
        assert!(
            emitted.len() < 350,
            "expected coalesced events, got {} events",
            emitted.len()
        );
        assert!(
            first_emit_at <= Duration::from_millis(8),
            "first emit took {first_emit_at:?}"
        );
    }

    #[tokio::test]
    async fn size_threshold_flushes_immediately() {
        let (emitted, sink) = collect_sink();
        let output = SessionOutputCoalescer::with_sink(sink);

        output.attach();
        output.push_owned("x".repeat(OUTPUT_NORMAL_BATCH_BYTES));

        let emitted = emitted.lock().unwrap();
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].bytes, OUTPUT_NORMAL_BATCH_BYTES);
    }

    #[tokio::test]
    async fn emitted_bytes_are_unacked_until_ack() {
        let (emitted, sink) = collect_sink();
        let output = SessionOutputCoalescer::with_sink(sink);

        output.attach();
        output.push_owned("x".repeat(OUTPUT_NORMAL_BATCH_BYTES));
        assert_eq!(emitted.lock().unwrap()[0].bytes, OUTPUT_NORMAL_BATCH_BYTES);

        output.ack(OUTPUT_NORMAL_BATCH_BYTES / 2);
        output.ack(OUTPUT_NORMAL_BATCH_BYTES / 2);

        output.push("ok");
        sleep(Duration::from_millis(20)).await;
        assert_eq!(emitted.lock().unwrap()[1].data, "ok");
    }

    #[tokio::test]
    async fn high_and_low_watermarks_pause_and_resume_once() {
        let (emitted, sink) = collect_sink();
        let (cmd_tx, mut cmd_rx) = session_command_channel("output-flow-test");
        let output = SessionOutputCoalescer::with_flow_sink(cmd_tx, sink);

        output.attach();
        output.push_owned("x".repeat(OUTPUT_PAUSE_HIGH_WATERMARK_BYTES));
        assert!(matches!(
            cmd_rx.recv().await,
            Some(SessionCommand::PauseOutput)
        ));
        assert!(cmd_rx.try_recv().is_err());

        let emitted_bytes =
            wait_for_emitted_bytes(&emitted, OUTPUT_PAUSE_HIGH_WATERMARK_BYTES).await;
        assert_eq!(emitted_bytes, OUTPUT_PAUSE_HIGH_WATERMARK_BYTES);
        output.ack(emitted_bytes - OUTPUT_RESUME_LOW_WATERMARK_BYTES - 1);
        assert!(cmd_rx.try_recv().is_err());

        output.ack(1);
        assert!(matches!(
            cmd_rx.recv().await,
            Some(SessionCommand::ResumeOutput)
        ));
    }

    #[tokio::test]
    async fn pending_output_is_preserved_in_order_without_dropped_bytes() {
        let (emitted, sink) = collect_sink();
        let output = SessionOutputCoalescer::with_sink(sink);
        let payload = format!("{}{}", "a".repeat(OUTPUT_FLOOD_BATCH_BYTES), "tail");

        output.push_owned(payload.clone());
        output.attach();

        let emitted_bytes = wait_for_emitted_bytes(&emitted, OUTPUT_FLOOD_BATCH_BYTES).await;
        output.ack(emitted_bytes);
        let total = wait_for_emitted_bytes(&emitted, payload.len()).await;
        let emitted = emitted.lock().unwrap();
        assert_eq!(total, payload.len());
        assert_eq!(
            emitted
                .iter()
                .map(|payload| payload.data.as_str())
                .collect::<String>(),
            payload
        );
        assert!(emitted.iter().all(|payload| payload.dropped_bytes == 0));
    }

    #[tokio::test]
    async fn attach_ack_waits_until_every_detached_batch_reaches_the_sink() {
        let (emitted, sink) = collect_sink();
        let output = SessionOutputCoalescer::with_sink(sink);
        output.push_owned("a".repeat(OUTPUT_FLOOD_BATCH_BYTES));
        output.push_owned("b".repeat(OUTPUT_FLOOD_BATCH_BYTES));

        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        output.attach_confirmed(ack_tx);
        ack_rx.await.expect("attach acknowledgement");

        let emitted = emitted.lock().unwrap();
        assert!(emitted.len() >= 2);
        let combined = emitted
            .iter()
            .map(|payload| payload.data.as_str())
            .collect::<String>();
        assert_eq!(
            combined,
            format!(
                "{}{}",
                "a".repeat(OUTPUT_FLOOD_BATCH_BYTES),
                "b".repeat(OUTPUT_FLOOD_BATCH_BYTES)
            )
        );
    }

    #[test]
    fn attach_replay_cannot_be_overtaken_by_a_concurrent_push() {
        let emitted = Arc::new(Mutex::new(Vec::<String>::new()));
        let emitted_sink = emitted.clone();
        let output = SessionOutputCoalescer::with_sink(move |payload| {
            std::thread::sleep(StdDuration::from_millis(10));
            emitted_sink.lock().unwrap().push(payload.data);
        });
        output.push_owned("old-title\n".to_string());

        // Large enough to trigger an immediate size flush (no async timer),
        // so its emission is fully synchronous after attach drains.
        let new_payload = format!("new-title\n{}", "n".repeat(OUTPUT_NORMAL_BATCH_BYTES));
        let pusher = output.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(StdDuration::from_millis(5));
            pusher.push_owned(new_payload);
        });

        let (ack_tx, mut ack_rx) = tokio::sync::oneshot::channel();
        output.attach_confirmed(ack_tx);
        handle.join().expect("join concurrent push");
        assert!(ack_rx.try_recv().is_ok());

        // The racing push blocks on the state lock during replay and emits
        // after it, so the detached backlog stays first in sink order.
        let emitted = emitted.lock().unwrap();
        let data = emitted.concat();
        let old = data.find("old-title").expect("old title");
        let new = data.find("new-title").expect("new title");
        assert!(old < new, "{data:?}");
    }

    #[tokio::test]
    async fn detached_pending_output_is_preserved_until_attach() {
        let (emitted, sink) = collect_sink();
        let output = SessionOutputCoalescer::with_sink(sink);
        let payload = "\x1b[?25lhidden\x1b[?25h";

        output.detach();
        output.push(payload);
        sleep(Duration::from_millis(20)).await;
        assert!(emitted.lock().unwrap().is_empty());

        output.attach();

        let emitted = emitted.lock().unwrap();
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].data, payload);
        assert_eq!(emitted[0].dropped_bytes, 0);
    }

    #[tokio::test]
    async fn catastrophic_backlog_sends_explicit_close_instead_of_dropping() {
        let (_emitted, sink) = collect_sink();
        let (cmd_tx, mut cmd_rx) = session_command_channel("output-close-test");
        let output = SessionOutputCoalescer::with_flow_sink(cmd_tx, sink);

        output.push_owned("x".repeat(OUTPUT_CATASTROPHIC_BACKLOG_BYTES));

        assert!(matches!(
            cmd_rx.recv().await,
            Some(SessionCommand::PauseOutput)
        ));
        assert!(matches!(cmd_rx.recv().await, Some(SessionCommand::Close)));
    }

    #[tokio::test]
    async fn attach_clears_stale_unacked_bytes() {
        let (emitted, sink) = collect_sink();
        let (cmd_tx, mut cmd_rx) = session_command_channel("output-resume-test");
        let output = SessionOutputCoalescer::with_flow_sink(cmd_tx, sink);

        output.attach();
        output.push_owned("x".repeat(OUTPUT_PAUSE_HIGH_WATERMARK_BYTES));
        assert!(matches!(
            cmd_rx.recv().await,
            Some(SessionCommand::PauseOutput)
        ));
        let emitted_bytes =
            wait_for_emitted_bytes(&emitted, OUTPUT_PAUSE_HIGH_WATERMARK_BYTES).await;
        assert_eq!(emitted_bytes, OUTPUT_PAUSE_HIGH_WATERMARK_BYTES);

        output.attach();
        assert!(matches!(
            cmd_rx.recv().await,
            Some(SessionCommand::ResumeOutput)
        ));

        output.push("after");
        sleep(Duration::from_millis(20)).await;
        assert!(
            emitted
                .lock()
                .unwrap()
                .iter()
                .any(|payload| payload.data.contains("after"))
        );
    }

    #[tokio::test]
    async fn detach_stops_emitting_until_attach() {
        let (emitted, sink) = collect_sink();
        let output = SessionOutputCoalescer::with_sink(sink);

        output.attach();
        output.push("visible");
        sleep(Duration::from_millis(20)).await;
        assert_eq!(emitted.lock().unwrap().len(), 1);

        output.detach();
        output.push("hidden");
        sleep(Duration::from_millis(20)).await;
        assert_eq!(emitted.lock().unwrap().len(), 1);

        output.attach();
        sleep(Duration::from_millis(20)).await;
        let emitted = emitted.lock().unwrap();
        assert!(
            emitted
                .iter()
                .any(|payload| payload.data.contains("hidden"))
        );
    }

    #[tokio::test]
    async fn close_flushes_remaining_output() {
        let (emitted, sink) = collect_sink();
        let output = SessionOutputCoalescer::with_sink(sink);

        output.attach();
        output.push("pending");
        output.close();

        assert!(
            emitted
                .lock()
                .unwrap()
                .iter()
                .any(|payload| payload.data == "pending")
        );
    }

    #[tokio::test]
    async fn close_flushes_only_recent_pending_output_without_protocol_drop_ack() {
        let (emitted, sink) = collect_sink();
        let output = SessionOutputCoalescer::with_sink(sink);

        output.push_owned("a".repeat(OUTPUT_CLOSE_FLUSH_MAX_BYTES + 16));
        output.close();

        let emitted = emitted.lock().unwrap();
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].bytes, OUTPUT_CLOSE_FLUSH_MAX_BYTES);
        assert_eq!(emitted[0].dropped_bytes, 0);
        assert!(emitted[0].data.chars().all(|ch| ch == 'a'));
    }
}
