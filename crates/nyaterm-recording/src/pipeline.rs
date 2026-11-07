//! Recording writer runtime, bounded ingress, and coalesced status/search events.
//! The caller owns lifecycle coordination and runs `take_shutdown()` off its UI thread.

use futures::{
    Stream,
    channel::{
        mpsc::{UnboundedReceiver, UnboundedSender, unbounded},
        oneshot,
    },
};

use crate::recording::{
    RecordingCapturePolicy, RecordingCompletion, RecordingContext, RecordingManager, RecordingMode,
    RecordingProfile, RecordingStatus, TerminalHistorySearchRequest, TerminalHistorySearchResponse,
};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};
use std::task::{Context, Poll};
use std::thread;
use std::time::{Duration, Instant};

const RECORDING_WRITE_QUEUE_BYTE_LIMIT: u64 = 4 * 1024 * 1024;
const RECORDING_WRITE_QUEUE_COMMAND_LIMIT: u64 = 4096;
const STATUS_INTERVAL: Duration = Duration::from_millis(250);
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

pub enum ExecutionRecordingEvent {
    Boundary { command: Option<String> },
    Output(String),
}

impl std::fmt::Debug for ExecutionRecordingEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Boundary { .. } => "ExecutionBoundary { .. }",
            Self::Output(_) => "ExecutionOutput { .. }",
        })
    }
}

impl ExecutionRecordingEvent {
    pub fn payload_bytes(&self) -> usize {
        match self {
            Self::Boundary { command } => command.as_ref().map_or(0, String::len),
            Self::Output(text) => text.len(),
        }
    }
}

type SearchMailbox = Arc<Mutex<HashMap<String, RecordingHistorySearchEvent>>>;

type StatusMailbox = Arc<Mutex<HashMap<(String, u8), RecordingStatus>>>;

#[derive(Default)]
struct QueueState {
    count: u64,
    sessions: HashMap<String, u64>,
}

pub struct RecordingWritePipeline {
    handle: RecordingWriteHandle,
    worker: Option<thread::JoinHandle<Vec<String>>>,
    event_rx: Option<RecordingEventReceiver>,
}

#[derive(Clone)]
pub struct RecordingWriteHandle {
    command_tx: mpsc::Sender<RecordingWriteCommand>,
    queued_bytes: Arc<AtomicU64>,
    queue: Arc<Mutex<QueueState>>,
    dropped: Arc<Mutex<DroppedPayloads>>,
    policies: Arc<Mutex<HashMap<String, RecordingCapturePolicy>>>,
    history_input: Arc<AtomicBool>,
    searches: Arc<Mutex<HashMap<String, RecordingHistorySearchKey>>>,
    search_results: SearchMailbox,
}

impl std::fmt::Debug for RecordingWriteHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RecordingWriteHandle")
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct DroppedPayloads {
    sessions: HashMap<String, DroppedPayloadState>,
}
#[derive(Default)]
struct DroppedPayloadState {
    bytes: u64,
    command_pending: bool,
}

pub struct RecordingEventReceiver {
    receiver: UnboundedReceiver<RecordingWriteEvent>,
    statuses: StatusMailbox,
    search_results: SearchMailbox,
}

impl RecordingEventReceiver {
    fn resolve(&self, event: RecordingWriteEvent) -> Option<RecordingWriteEvent> {
        match event {
            RecordingWriteEvent::PendingHistorySearch { session_id } => self
                .search_results
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&session_id)
                .map(RecordingWriteEvent::HistorySearch),
            RecordingWriteEvent::PendingStatus { session_id, state } => self
                .statuses
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&(session_id, state))
                .map(RecordingWriteEvent::Status),
            event => Some(event),
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn try_recv(
        &mut self,
    ) -> Result<RecordingWriteEvent, futures::channel::mpsc::TryRecvError> {
        loop {
            let event = self.receiver.try_recv()?;
            if let Some(event) = self.resolve(event) {
                return Ok(event);
            }
        }
    }
}

impl Stream for RecordingEventReceiver {
    type Item = RecordingWriteEvent;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match Pin::new(&mut self.receiver).poll_next(cx) {
                Poll::Ready(Some(event)) => {
                    if let Some(event) = self.resolve(event) {
                        return Poll::Ready(Some(event));
                    }
                }
                other => return other,
            }
        }
    }
}

impl RecordingWritePipeline {
    pub fn spawn(memory_limit_bytes: usize) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = unbounded();
        let statuses = Arc::new(Mutex::new(HashMap::new()));
        let handle = RecordingWriteHandle {
            command_tx,
            queued_bytes: Arc::new(AtomicU64::new(0)),
            queue: Arc::new(Mutex::new(QueueState::default())),
            dropped: Arc::new(Mutex::new(DroppedPayloads::default())),
            policies: Arc::new(Mutex::new(HashMap::new())),
            history_input: Arc::new(AtomicBool::new(false)),
            searches: Arc::new(Mutex::new(HashMap::new())),
            search_results: Arc::new(Mutex::new(HashMap::new())),
        };
        let worker_handle = handle.clone();
        let worker_statuses = Arc::clone(&statuses);
        let worker = thread::Builder::new()
            .name("nyaterm-recording-writer".into())
            .spawn(move || {
                run_recording_writer(
                    memory_limit_bytes,
                    command_rx,
                    event_tx,
                    worker_handle,
                    worker_statuses,
                )
            })
            .expect("failed to spawn recording writer");
        let search_results = Arc::clone(&handle.search_results);
        Self {
            handle,
            worker: Some(worker),
            event_rx: Some(RecordingEventReceiver {
                receiver: event_rx,
                statuses,
                search_results,
            }),
        }
    }
    pub fn writer(&self) -> RecordingWriteHandle {
        self.handle.clone()
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn write_output(&self, session_id: impl Into<String>, text: impl Into<String>) {
        self.handle.write_output(session_id, text);
    }
    pub fn write_input(&self, session_id: impl Into<String>, data: impl Into<Vec<u8>>) {
        self.handle.write_input(session_id, data);
    }
    pub fn write_raw_input(&self, session_id: impl Into<String>, data: impl Into<Vec<u8>>) {
        self.handle.write_raw_input(session_id, data);
    }
    pub fn resync_input_line(&self, session_id: impl Into<String>, line: String) {
        self.handle.resync_input_line(session_id, line);
    }
    pub fn cleanup_session(&self, session_id: impl Into<String>) {
        let session_id = session_id.into();
        self.handle
            .policies
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&session_id);
        let _ = self
            .handle
            .command_tx
            .send(RecordingWriteCommand::CleanupSession { session_id });
    }
    pub fn disconnect_session(&self, session_id: impl Into<String>) {
        let _ = self
            .handle
            .command_tx
            .send(RecordingWriteCommand::DisconnectSession {
                session_id: session_id.into(),
            });
    }
    pub fn rekey_session(
        &self,
        old_session_id: impl Into<String>,
        new_session_id: impl Into<String>,
    ) {
        let old_session_id = old_session_id.into();
        let new_session_id = new_session_id.into();
        let mut policies = self
            .handle
            .policies
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(policy) = policies.remove(&old_session_id) {
            policies.insert(new_session_id.clone(), policy);
        }
        let _ = self
            .handle
            .command_tx
            .send(RecordingWriteCommand::RekeySession {
                old_session_id,
                new_session_id,
            });
    }
    pub fn set_memory_limit(&self, memory_limit_bytes: usize) {
        let _ = self
            .handle
            .command_tx
            .send(RecordingWriteCommand::SetMemoryLimit { memory_limit_bytes });
    }
    pub fn set_history_include_input(&self, enabled: bool) {
        self.handle.history_input.store(enabled, Ordering::Release);
        let _ = self
            .handle
            .command_tx
            .send(RecordingWriteCommand::SetHistoryInput { enabled });
    }
    pub fn request_history_search(&self, key: RecordingHistorySearchKey) {
        if key.query.trim().is_empty() {
            return;
        }
        let id = key.session_id.clone();
        let mut searches = self
            .handle
            .searches
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if searches.insert(id.clone(), key).is_none() {
            let _ = self
                .handle
                .command_tx
                .send(RecordingWriteCommand::HistorySearch { session_id: id });
        }
    }
    pub fn take_event_receiver(&mut self) -> Option<RecordingEventReceiver> {
        self.event_rx.take()
    }
    pub fn take_shutdown(&mut self) -> Option<impl FnOnce() -> Vec<String> + Send + 'static> {
        let worker = self.worker.take()?;
        let sender = self.handle.command_tx.clone();
        Some(move || {
            let _ = sender.send(RecordingWriteCommand::Shutdown);
            worker
                .join()
                .unwrap_or_else(|_| vec!["recording writer panicked during shutdown".into()])
        })
    }
    pub fn shutdown(&mut self) {
        if let Some(shutdown) = self.take_shutdown() {
            for error in shutdown() {
                tracing::warn!(%error, "recording shutdown failed");
            }
        }
    }
}
impl Drop for RecordingWritePipeline {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Debug)]
enum ReplySender {
    Blocking(mpsc::SyncSender<Result<String, String>>),
    Async(oneshot::Sender<Result<String, String>>),
}
impl ReplySender {
    fn send(self, result: Result<String, String>) {
        match self {
            Self::Blocking(sender) => {
                let _ = sender.send(result);
            }
            Self::Async(sender) => {
                let _ = sender.send(result);
            }
        }
    }
}

impl RecordingWriteHandle {
    pub fn write_execution_event(&self, session_id: &str, mut event: ExecutionRecordingEvent) {
        let policy = self.capture_policy(session_id);
        if policy.mode == RecordingMode::Raw {
            return;
        }
        if !policy.include_input
            && let ExecutionRecordingEvent::Boundary { command } = &mut event
        {
            *command = None;
        }
        self.enqueue_payload(
            session_id,
            event.payload_bytes(),
            RecordingWriteCommand::Execution {
                session_id: session_id.into(),
                event,
            },
        );
    }
    pub fn report_output_gap(&self, session_id: &str, bytes: usize) {
        self.report_dropped_payload(session_id, bytes);
    }
    pub fn capture_policy(&self, session_id: &str) -> RecordingCapturePolicy {
        self.policies
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(session_id)
            .copied()
            .unwrap_or(RecordingCapturePolicy {
                include_input: self.history_input.load(Ordering::Acquire),
                ..Default::default()
            })
    }
    pub fn write_output(&self, session_id: impl Into<String>, text: impl Into<String>) {
        let session_id = session_id.into();
        let text = text.into();
        if !text.is_empty() {
            self.enqueue_payload(
                &session_id,
                text.len(),
                RecordingWriteCommand::WriteOutput {
                    session_id: session_id.clone(),
                    text,
                },
            );
        }
    }
    pub fn write_local_message(&self, session_id: impl Into<String>, text: impl Into<String>) {
        let session_id = session_id.into();
        let text = text.into();
        if !text.is_empty() {
            self.enqueue_payload(
                &session_id,
                text.len(),
                RecordingWriteCommand::WriteLocalMessage {
                    session_id: session_id.clone(),
                    text,
                },
            );
        }
    }
    pub fn write_raw_output(&self, session_id: &str, data: &[u8]) {
        if data.is_empty() || self.capture_policy(session_id).mode != RecordingMode::Raw {
            return;
        }
        self.enqueue_payload(
            session_id,
            data.len(),
            RecordingWriteCommand::WriteRawOutput {
                session_id: session_id.into(),
                data: data.to_vec(),
            },
        );
    }
    pub fn write_input(&self, session_id: impl Into<String>, data: impl Into<Vec<u8>>) {
        let session_id = session_id.into();
        let policy = self.capture_policy(&session_id);
        if !policy.include_input || policy.mode == RecordingMode::Raw {
            return;
        }
        let data = data.into();
        if !data.is_empty() {
            self.enqueue_payload(
                &session_id,
                data.len(),
                RecordingWriteCommand::WriteInput {
                    session_id: session_id.clone(),
                    data,
                },
            );
        }
    }
    pub fn write_raw_input(&self, session_id: impl Into<String>, data: impl Into<Vec<u8>>) {
        let session_id = session_id.into();
        if !self.capture_policy(&session_id).include_input {
            return;
        }
        let data = data.into();
        if !data.is_empty() {
            self.enqueue_payload(
                &session_id,
                data.len(),
                RecordingWriteCommand::WriteRawInput {
                    session_id: session_id.clone(),
                    data,
                },
            );
        }
    }
    pub fn resync_input_line(&self, session_id: impl Into<String>, line: String) {
        let session_id = session_id.into();
        if !self.capture_policy(&session_id).include_input {
            return;
        }
        self.enqueue_payload(
            &session_id,
            line.len(),
            RecordingWriteCommand::ResyncInputLine {
                session_id: session_id.clone(),
                line,
            },
        );
    }
    fn enqueue_payload(
        &self,
        session_id: &str,
        payload_bytes: usize,
        command: RecordingWriteCommand,
    ) {
        let mut queue = self.queue.lock().unwrap_or_else(|error| error.into_inner());
        if queue.count >= RECORDING_WRITE_QUEUE_COMMAND_LIMIT
            || !try_reserve_queued_bytes(&self.queued_bytes, payload_bytes)
        {
            drop(queue);
            self.report_dropped_payload(session_id, payload_bytes);
            return;
        }
        queue.count += 1;
        *queue.sessions.entry(session_id.into()).or_default() += payload_bytes as u64;
        if self.command_tx.send(command).is_err() {
            queue.count -= 1;
            *queue
                .sessions
                .get_mut(session_id)
                .expect("reserved session") -= payload_bytes as u64;
            release_queued_bytes(&self.queued_bytes, payload_bytes);
        }
    }
    fn release_payload(&self, session_id: &str, bytes: usize) {
        let mut queue = self.queue.lock().unwrap_or_else(|error| error.into_inner());
        queue.count -= 1;
        if let Some(queued) = queue.sessions.get_mut(session_id) {
            *queued = queued.saturating_sub(bytes as u64);
            if *queued == 0 {
                queue.sessions.remove(session_id);
            }
        }
        release_queued_bytes(&self.queued_bytes, bytes);
    }
    fn report_dropped_payload(&self, session_id: &str, payload_bytes: usize) {
        let mut dropped = self
            .dropped
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let state = dropped.sessions.entry(session_id.into()).or_default();
        state.bytes = state.bytes.saturating_add(payload_bytes as u64);
        if !state.command_pending {
            state.command_pending = true;
            let _ = self.command_tx.send(RecordingWriteCommand::ReportDropped {
                session_id: session_id.into(),
            });
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn send_start(
        &self,
        session_id: String,
        context: RecordingContext,
        profile: RecordingProfile,
        explicit_path: Option<std::path::PathBuf>,
        memory_limit_bytes: usize,
        automatic: bool,
        reply: ReplySender,
    ) {
        let policy = RecordingCapturePolicy {
            mode: profile.mode,
            include_input: profile.include_input,
            include_binary_transfer_payloads: profile.include_binary_transfer_payloads,
        };
        let mut policies = self
            .policies
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        policies.entry(session_id.clone()).or_insert(policy);
        if let Err(error) = self.command_tx.send(RecordingWriteCommand::Start {
            session_id: session_id.clone(),
            context: Box::new(context),
            profile,
            explicit_path,
            memory_limit_bytes,
            automatic,
            reply_tx: reply,
        }) {
            policies.remove(&session_id);
            if let RecordingWriteCommand::Start { reply_tx, .. } = error.0 {
                reply_tx.send(Err("recording writer stopped".into()));
            }
        }
    }
    pub fn start_async(
        &self,
        session_id: String,
        context: RecordingContext,
        profile: RecordingProfile,
        explicit_path: Option<std::path::PathBuf>,
        memory_limit_bytes: usize,
        automatic: bool,
    ) -> oneshot::Receiver<Result<String, String>> {
        let (sender, receiver) = oneshot::channel();
        self.send_start(
            session_id,
            context,
            profile,
            explicit_path,
            memory_limit_bytes,
            automatic,
            ReplySender::Async(sender),
        );
        receiver
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn start(
        &self,
        session_id: String,
        context: RecordingContext,
        profile: RecordingProfile,
        explicit_path: Option<std::path::PathBuf>,
        memory_limit_bytes: usize,
    ) -> Result<String, String> {
        let (sender, receiver) = mpsc::sync_channel(0);
        self.send_start(
            session_id,
            context,
            profile,
            explicit_path,
            memory_limit_bytes,
            false,
            ReplySender::Blocking(sender),
        );
        receiver
            .recv()
            .map_err(|_| "recording writer stopped".to_string())?
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn stop(&self, session_id: String) -> Result<String, String> {
        let completion = self.stop_complete(session_id)?;
        match completion.error {
            Some(error) => Err(error),
            None => Ok(completion.file_path.to_string_lossy().into_owned()),
        }
    }
    pub fn stop_complete(&self, session_id: String) -> Result<RecordingCompletion, String> {
        let (sender, receiver) = mpsc::sync_channel(0);
        self.command_tx
            .send(RecordingWriteCommand::Stop {
                session_id,
                reply_tx: sender,
            })
            .map_err(|_| "recording writer stopped".to_string())?;
        receiver
            .recv()
            .map_err(|_| "recording writer stopped".to_string())?
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn save_transcript(
        &self,
        session_id: String,
        path: String,
        include_io_labels: bool,
        include_timestamps: bool,
        memory_limit_bytes: usize,
    ) -> Result<String, String> {
        self.save_transcript_with_policy(
            session_id,
            path,
            include_io_labels,
            include_timestamps,
            memory_limit_bytes,
            false,
        )
    }
    pub fn save_transcript_with_policy(
        &self,
        session_id: String,
        path: String,
        include_io_labels: bool,
        include_timestamps: bool,
        memory_limit_bytes: usize,
        unique: bool,
    ) -> Result<String, String> {
        let (sender, receiver) = mpsc::sync_channel(0);
        self.command_tx
            .send(RecordingWriteCommand::SaveTranscript {
                session_id,
                path,
                include_io_labels,
                include_timestamps,
                memory_limit_bytes,
                unique,
                reply_tx: ReplySender::Blocking(sender),
            })
            .map_err(|_| "recording writer stopped".to_string())?;
        receiver
            .recv()
            .map_err(|_| "recording writer stopped".to_string())?
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn flush(&self) {
        let (ack_tx, ack_rx) = mpsc::sync_channel(0);
        if self
            .command_tx
            .send(RecordingWriteCommand::Flush { ack_tx })
            .is_ok()
        {
            let _ = ack_rx.recv();
        }
    }
    #[cfg(test)]
    fn block_writer(&self) -> mpsc::SyncSender<()> {
        let (started_tx, started_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::sync_channel(0);
        self.command_tx
            .send(RecordingWriteCommand::Block {
                started_tx,
                release_rx,
            })
            .unwrap();
        started_rx.recv().unwrap();
        release_tx
    }
}

#[derive(Clone, Debug)]
pub enum RecordingWriteEvent {
    HistorySearch(RecordingHistorySearchEvent),
    PendingHistorySearch { session_id: String },
    Status(RecordingStatus),
    StatusRemoved { session_id: String },
    PendingStatus { session_id: String, state: u8 },
    ShutdownError(String),
}
#[derive(Clone, Debug)]
pub struct RecordingHistorySearchEvent {
    pub key: RecordingHistorySearchKey,
    pub result: Result<TerminalHistorySearchResponse, String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingHistorySearchKey {
    pub session_id: String,
    pub query: String,
    pub case_sensitive: bool,
    pub regex: bool,
    pub whole_word: bool,
    pub limit: Option<usize>,
    pub context_before: Option<usize>,
    pub context_after: Option<usize>,
    pub max_lines: Option<usize>,
}
impl RecordingHistorySearchKey {
    pub fn request(&self) -> TerminalHistorySearchRequest {
        TerminalHistorySearchRequest {
            session_id: self.session_id.clone(),
            query: self.query.clone(),
            case_sensitive: self.case_sensitive,
            regex: self.regex,
            whole_word: self.whole_word,
            limit: self.limit,
            context_before: self.context_before,
            context_after: self.context_after,
            max_lines: self.max_lines,
        }
    }
}
#[derive(Debug)]
enum RecordingWriteCommand {
    Execution {
        session_id: String,
        event: ExecutionRecordingEvent,
    },
    Shutdown,
    Start {
        session_id: String,
        context: Box<RecordingContext>,
        profile: RecordingProfile,
        explicit_path: Option<std::path::PathBuf>,
        memory_limit_bytes: usize,
        automatic: bool,
        reply_tx: ReplySender,
    },
    Stop {
        session_id: String,
        reply_tx: mpsc::SyncSender<Result<RecordingCompletion, String>>,
    },
    SetMemoryLimit {
        memory_limit_bytes: usize,
    },
    SetHistoryInput {
        enabled: bool,
    },
    ReportDropped {
        session_id: String,
    },
    SaveTranscript {
        session_id: String,
        path: String,
        include_io_labels: bool,
        include_timestamps: bool,
        memory_limit_bytes: usize,
        unique: bool,
        reply_tx: ReplySender,
    },
    WriteOutput {
        session_id: String,
        text: String,
    },
    WriteLocalMessage {
        session_id: String,
        text: String,
    },
    WriteRawOutput {
        session_id: String,
        data: Vec<u8>,
    },
    WriteInput {
        session_id: String,
        data: Vec<u8>,
    },
    WriteRawInput {
        session_id: String,
        data: Vec<u8>,
    },
    ResyncInputLine {
        session_id: String,
        line: String,
    },
    CleanupSession {
        session_id: String,
    },
    DisconnectSession {
        session_id: String,
    },
    RekeySession {
        old_session_id: String,
        new_session_id: String,
    },
    HistorySearch {
        session_id: String,
    },
    #[cfg(any(test, feature = "test-support"))]
    Flush {
        ack_tx: mpsc::SyncSender<()>,
    },
    #[cfg(test)]
    Block {
        started_tx: mpsc::SyncSender<()>,
        release_rx: mpsc::Receiver<()>,
    },
}
fn try_reserve_queued_bytes(queued_bytes: &AtomicU64, bytes: usize) -> bool {
    queued_bytes
        .try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            current
                .checked_add(bytes as u64)
                .filter(|next| *next <= RECORDING_WRITE_QUEUE_BYTE_LIMIT)
        })
        .is_ok()
}
fn release_queued_bytes(queued_bytes: &AtomicU64, bytes: usize) {
    queued_bytes.fetch_sub(bytes as u64, Ordering::AcqRel);
}

struct StatusPublisher {
    sender: UnboundedSender<RecordingWriteEvent>,
    mailbox: StatusMailbox,
    sent: HashMap<String, (RecordingStatus, Instant)>,
}
impl StatusPublisher {
    fn status(
        &mut self,
        manager: &RecordingManager,
        handle: &RecordingWriteHandle,
        id: &str,
        force: bool,
    ) {
        let Some(mut status) = manager.status(id) else {
            return;
        };
        status.queued_bytes = handle
            .queue
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .sessions
            .get(id)
            .copied()
            .unwrap_or(0);
        let now = Instant::now();
        if !force
            && self.sent.get(id).is_some_and(|(previous, time)| {
                previous.state == status.state
                    && previous.file_path == status.file_path
                    && (previous == &status || now.duration_since(*time) < STATUS_INTERVAL)
            })
        {
            return;
        }
        self.sent.insert(id.into(), (status.clone(), now));
        let key = (id.to_string(), status.state as u8);
        let mut mailbox = self
            .mailbox
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if mailbox.insert(key.clone(), status).is_none() {
            let _ = self
                .sender
                .unbounded_send(RecordingWriteEvent::PendingStatus {
                    session_id: key.0,
                    state: key.1,
                });
        }
    }
    fn removed(&mut self, id: &str) {
        self.sent.remove(id);
        self.mailbox
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|(session_id, _), _| session_id != id);
        let _ = self
            .sender
            .unbounded_send(RecordingWriteEvent::StatusRemoved {
                session_id: id.into(),
            });
    }
}

fn run_recording_writer(
    memory_limit_bytes: usize,
    receiver: mpsc::Receiver<RecordingWriteCommand>,
    event_tx: UnboundedSender<RecordingWriteEvent>,
    handle: RecordingWriteHandle,
    statuses: StatusMailbox,
) -> Vec<String> {
    let manager = RecordingManager::new();
    manager.set_memory_limit(memory_limit_bytes);
    let mut rekeyed: HashMap<String, Option<String>> = HashMap::new();
    let mut publisher = StatusPublisher {
        sender: event_tx,
        mailbox: statuses,
        sent: HashMap::new(),
    };
    let mut last_flush = Instant::now();
    let mut last_status = Instant::now();
    loop {
        if last_flush.elapsed() >= FLUSH_INTERVAL {
            manager.flush_recordings();
            last_flush = Instant::now();
        }
        if last_status.elapsed() >= STATUS_INTERVAL {
            for status in manager.list_recording_statuses() {
                publisher.status(&manager, &handle, &status.session_id, false);
            }
            last_status = Instant::now();
        }
        let command = match receiver.recv_timeout(STATUS_INTERVAL) {
            Ok(command) => command,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        };
        match command {
            RecordingWriteCommand::Execution { session_id, event } => {
                if let Some(id) = current_recording_id(&session_id, &rekeyed) {
                    match &event {
                        ExecutionRecordingEvent::Boundary { command } => {
                            manager.execution_boundary(id, command.as_deref())
                        }
                        ExecutionRecordingEvent::Output(text) => {
                            manager.write_execution_output(id, text)
                        }
                    }
                    publisher.status(&manager, &handle, id, false);
                }
                handle.release_payload(&session_id, event.payload_bytes());
            }
            RecordingWriteCommand::Shutdown => break,
            RecordingWriteCommand::Start {
                session_id,
                mut context,
                profile,
                explicit_path,
                memory_limit_bytes,
                automatic,
                reply_tx,
            } => {
                manager.set_memory_limit(memory_limit_bytes);
                let id = current_recording_id(&session_id, &rekeyed);
                let result = if let Some(id) = id {
                    context.session_id = id.into();
                    if automatic && manager.is_recording(id) {
                        Ok(manager
                            .status(id)
                            .and_then(|status| status.file_path)
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned())
                    } else {
                        manager
                            .start_with_profile(id, *context, profile, explicit_path)
                            .map_err(|error| error.to_string())
                    }
                } else {
                    Err("session closed before recording started".into())
                };
                let id = id.unwrap_or(&session_id);
                if manager.is_recording(id) {
                    handle
                        .policies
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .insert(id.into(), manager.capture_policy(id));
                    publisher.status(&manager, &handle, id, true);
                } else {
                    handle
                        .policies
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .remove(id);
                    publisher.removed(id);
                }
                if automatic && let Err(error) = &result {
                    let _ = publisher
                        .sender
                        .unbounded_send(RecordingWriteEvent::ShutdownError(format!(
                            "auto-recording start failed: {error}"
                        )));
                }
                reply_tx.send(result);
            }
            RecordingWriteCommand::Stop {
                session_id,
                reply_tx,
            } => {
                let id = current_recording_id(&session_id, &rekeyed).unwrap_or(&session_id);
                let result = manager.stop_complete(id).map_err(|error| error.to_string());
                handle
                    .policies
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(id);
                publisher.removed(id);
                let _ = reply_tx.send(result);
            }
            RecordingWriteCommand::SetMemoryLimit { memory_limit_bytes } => {
                manager.set_memory_limit(memory_limit_bytes)
            }
            RecordingWriteCommand::SetHistoryInput { enabled } => {
                manager.set_history_include_input(enabled)
            }
            RecordingWriteCommand::ReportDropped { session_id } => {
                let bytes = take_dropped_payloads(&handle.dropped, &session_id);
                let id = current_recording_id(&session_id, &rekeyed).unwrap_or(&session_id);
                manager.report_dropped(id, usize::try_from(bytes).unwrap_or(usize::MAX));
                publisher.status(&manager, &handle, id, false);
                if !manager.is_recording(id) {
                    remove_dropped_payloads(&handle.dropped, &session_id);
                }
            }
            RecordingWriteCommand::SaveTranscript {
                session_id,
                path,
                include_io_labels,
                include_timestamps,
                memory_limit_bytes,
                unique,
                reply_tx,
            } => {
                manager.set_memory_limit(memory_limit_bytes);
                let id = current_recording_id(&session_id, &rekeyed).unwrap_or(&session_id);
                let result = if unique {
                    manager.save_transcript_unique(id, &path, include_io_labels, include_timestamps)
                } else {
                    manager.save_transcript(id, &path, include_io_labels, include_timestamps)
                };
                reply_tx.send(result.map_err(|error| error.to_string()));
            }
            RecordingWriteCommand::WriteOutput { session_id, text } => {
                if let Some(id) = current_recording_id(&session_id, &rekeyed) {
                    manager.write_output(id, &text);
                    publisher.status(&manager, &handle, id, false);
                }
                handle.release_payload(&session_id, text.len());
            }
            RecordingWriteCommand::WriteLocalMessage { session_id, text } => {
                if let Some(id) = current_recording_id(&session_id, &rekeyed) {
                    manager.write_local_message(id, &text);
                    publisher.status(&manager, &handle, id, false);
                }
                handle.release_payload(&session_id, text.len());
            }
            RecordingWriteCommand::WriteRawOutput { session_id, data } => {
                if let Some(id) = current_recording_id(&session_id, &rekeyed) {
                    manager.write_raw_output(id, &data);
                    publisher.status(&manager, &handle, id, false);
                }
                handle.release_payload(&session_id, data.len());
            }
            RecordingWriteCommand::WriteInput { session_id, data } => {
                if let Some(id) = current_recording_id(&session_id, &rekeyed) {
                    manager.write_input(id, &data);
                    publisher.status(&manager, &handle, id, false);
                }
                handle.release_payload(&session_id, data.len());
            }
            RecordingWriteCommand::WriteRawInput { session_id, data } => {
                if let Some(id) = current_recording_id(&session_id, &rekeyed) {
                    manager.write_raw_input(id, &data);
                    publisher.status(&manager, &handle, id, false);
                }
                handle.release_payload(&session_id, data.len());
            }
            RecordingWriteCommand::ResyncInputLine { session_id, line } => {
                if let Some(id) = current_recording_id(&session_id, &rekeyed) {
                    manager.resync_input_line(id, &line);
                    publisher.status(&manager, &handle, id, false);
                }
                handle.release_payload(&session_id, line.len());
            }
            RecordingWriteCommand::CleanupSession { session_id } => {
                let id = session_id.clone();
                if manager.is_recording(&id)
                    && let Err(error) = manager.stop(&id)
                {
                    let _ = publisher
                        .sender
                        .unbounded_send(RecordingWriteEvent::ShutdownError(error.to_string()));
                }
                manager.cleanup_session(&id);
                handle
                    .searches
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(&id);
                handle
                    .search_results
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(&id);
                if !matches!(rekeyed.get(&id), Some(Some(_))) {
                    rekeyed.insert(id.clone(), None);
                }
                remove_dropped_payloads(&handle.dropped, &id);
                publisher.removed(&id);
            }
            RecordingWriteCommand::DisconnectSession { session_id } => {
                if let Some(id) = current_recording_id(&session_id, &rekeyed) {
                    manager.disconnect_session(id);
                    publisher.status(&manager, &handle, id, false);
                }
            }
            RecordingWriteCommand::RekeySession {
                old_session_id,
                new_session_id,
            } => {
                manager.rekey_session(&old_session_id, &new_session_id);
                handle
                    .searches
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(&old_session_id);
                handle
                    .search_results
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(&old_session_id);
                rekeyed.insert(old_session_id.clone(), Some(new_session_id.clone()));
                let bytes = take_dropped_payloads(&handle.dropped, &old_session_id);
                if bytes > 0 {
                    manager.report_dropped(
                        &new_session_id,
                        usize::try_from(bytes).unwrap_or(usize::MAX),
                    );
                }
                remove_dropped_payloads(&handle.dropped, &old_session_id);
                publisher.removed(&old_session_id);
                publisher.status(&manager, &handle, &new_session_id, true);
            }
            RecordingWriteCommand::HistorySearch { session_id } => {
                let key = handle
                    .searches
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(&session_id);
                if let Some(key) = key {
                    let result = manager
                        .search_history(key.request())
                        .map_err(|error| error.to_string());
                    let mut results = handle
                        .search_results
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    if results
                        .insert(
                            session_id.clone(),
                            RecordingHistorySearchEvent { key, result },
                        )
                        .is_none()
                    {
                        let _ = publisher.sender.unbounded_send(
                            RecordingWriteEvent::PendingHistorySearch { session_id },
                        );
                    }
                }
            }
            #[cfg(any(test, feature = "test-support"))]
            RecordingWriteCommand::Flush { ack_tx } => {
                for status in manager.list_recording_statuses() {
                    publisher.status(&manager, &handle, &status.session_id, false);
                }
                let _ = ack_tx.send(());
            }
            #[cfg(test)]
            RecordingWriteCommand::Block {
                started_tx,
                release_rx,
            } => {
                let _ = started_tx.send(());
                let _ = release_rx.recv();
            }
        }
    }
    manager
        .finish_all()
        .into_iter()
        .map(|error| error.to_string())
        .collect()
}
fn current_recording_id<'a>(
    session_id: &'a str,
    rekeyed: &'a HashMap<String, Option<String>>,
) -> Option<&'a str> {
    let mut id = session_id;
    for _ in 0..=rekeyed.len() {
        match rekeyed.get(id) {
            Some(Some(next)) => id = next,
            Some(None) => return None,
            None => return Some(id),
        }
    }
    None
}
fn take_dropped_payloads(dropped: &Mutex<DroppedPayloads>, id: &str) -> u64 {
    let mut dropped = dropped.lock().unwrap_or_else(|error| error.into_inner());
    let Some(state) = dropped.sessions.get_mut(id) else {
        return 0;
    };
    state.command_pending = false;
    std::mem::take(&mut state.bytes)
}
fn remove_dropped_payloads(dropped: &Mutex<DroppedPayloads>, id: &str) {
    dropped
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .sessions
        .remove(id);
}

#[cfg(test)]
mod tests {
    use super::{
        RECORDING_WRITE_QUEUE_BYTE_LIMIT, RecordingHistorySearchKey, RecordingWriteEvent,
        RecordingWritePipeline,
    };
    use crate::recording::{
        ExistingFileBehavior, RecordingContext, RecordingMode, RecordingProfile,
        RecordingRotationPolicy, RecordingStatusState,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};
    use time::OffsetDateTime;

    #[test]
    fn recording_pipeline_preserves_write_order_before_flush() {
        let mut pipeline = RecordingWritePipeline::spawn(1024 * 1024);
        pipeline.set_history_include_input(true);
        let mut event_rx = pipeline
            .take_event_receiver()
            .expect("recording events should be available");
        let session_id = "session-a";
        pipeline.write_input(session_id, b"echo hello\r".to_vec());
        pipeline.write_output(session_id, "hello\n");
        pipeline.request_history_search(RecordingHistorySearchKey {
            session_id: session_id.to_string(),
            query: "hello".to_string(),
            case_sensitive: false,
            regex: false,
            whole_word: false,
            limit: Some(10),
            context_before: Some(0),
            context_after: Some(0),
            max_lines: None,
        });
        pipeline.writer().flush();

        let results = loop {
            let event = event_rx.try_recv().expect("search result should be queued");
            if let RecordingWriteEvent::HistorySearch(event) = event {
                break event.result.expect("search should succeed");
            }
        };
        assert_eq!(results.total, 2);
        assert_eq!(results.results[0].source, "input");
        assert_eq!(results.results[1].source, "output");
    }

    #[test]
    fn recording_handle_orders_start_writes_and_stop() {
        let mut pipeline = RecordingWritePipeline::spawn(1024 * 1024);
        pipeline.set_history_include_input(true);
        let mut event_rx = pipeline
            .take_event_receiver()
            .expect("recording events should be available");
        let writer = pipeline.writer();
        let session_id = "session-handle-order";
        let path = PathBuf::from(unique_recording_path("handle-order"));

        writer
            .start(
                session_id.to_string(),
                recording_context(session_id),
                recording_profile(&path),
                Some(path.clone()),
                1024 * 1024,
            )
            .expect("recording should start through the writer");
        writer.write_input(session_id, b"echo ordered\r".to_vec());
        writer.write_output(session_id, "echo ordered\r\nordered\n");
        writer
            .stop(session_id.to_string())
            .expect("recording should stop after queued writes");

        let text = fs::read_to_string(&path).expect("recording file should exist");
        assert!(text.contains("[INPUT] echo ordered"));
        assert!(text.contains("[OUTPUT] ordered"));
        let mut removed = false;
        while let Ok(event) = event_rx.try_recv() {
            removed |= matches!(
                event,
                RecordingWriteEvent::StatusRemoved { ref session_id }
                    if session_id == "session-handle-order"
            );
        }
        assert!(removed);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn recording_pipeline_resyncs_tab_input_before_enter() {
        let pipeline = RecordingWritePipeline::spawn(1024 * 1024);
        let writer = pipeline.writer();
        let session_id = "session-tab-resync";
        let path = PathBuf::from(unique_recording_path("tab-resync"));
        writer
            .start(
                session_id.to_string(),
                recording_context(session_id),
                recording_profile(&path),
                Some(path.clone()),
                1024 * 1024,
            )
            .expect("recording should start");
        pipeline.write_input(session_id, b"vi ins\t".to_vec());
        pipeline.write_output(session_id, "\r\x1b[2Kvi install-node-exporter.sh");
        pipeline.resync_input_line(
            session_id,
            "[root@rocky9 ~]# vi install-node-exporter.sh".to_string(),
        );
        pipeline.write_input(session_id, b"\r".to_vec());
        writer
            .stop(session_id.to_string())
            .expect("recording should stop");

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[INPUT] vi install-node-exporter.sh\n"
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn failed_stop_still_removes_the_finished_recording_status() {
        let mut pipeline = RecordingWritePipeline::spawn(1024 * 1024);
        pipeline.set_history_include_input(true);
        let mut event_rx = pipeline
            .take_event_receiver()
            .expect("recording events should be available");
        let writer = pipeline.writer();
        let session_id = "session-failed-stop";
        let initial_path = PathBuf::from(unique_recording_path("failed-stop-initial"));
        let blocked_base_path = PathBuf::from(unique_recording_path("failed-stop-base"));
        let blocked_segment = initial_path.with_file_name(format!(
            "{}-1.log",
            initial_path.file_stem().unwrap().to_string_lossy()
        ));
        fs::create_dir(&blocked_segment).unwrap();
        let mut profile = recording_profile(&initial_path);
        profile.base_path = blocked_base_path.clone();
        profile.path_template = "rotated.log".to_string();
        profile.rotation = RecordingRotationPolicy::Size { max_bytes: 1 };

        writer
            .start(
                session_id.to_string(),
                recording_context(session_id),
                profile,
                Some(initial_path.clone()),
                1024 * 1024,
            )
            .expect("recording should start");
        writer.write_output(session_id, "first\n");
        writer.write_output(session_id, "rotation must fail\n");
        writer.flush();
        let mut saw_failed = false;
        while let Ok(event) = event_rx.try_recv() {
            saw_failed |= matches!(event, RecordingWriteEvent::Status(status)
                if status.session_id == session_id && status.state == RecordingStatusState::Failed);
        }
        assert!(saw_failed);
        let completion = writer.stop_complete(session_id.to_string()).unwrap();
        assert_eq!(completion.file_path, initial_path);
        assert!(completion.error.is_some());

        let mut saw_removed_after_failed = false;
        while let Ok(event) = event_rx.try_recv() {
            match event {
                RecordingWriteEvent::Status(status)
                    if status.session_id == session_id
                        && status.state == RecordingStatusState::Failed =>
                {
                    saw_failed = true;
                }
                RecordingWriteEvent::StatusRemoved {
                    session_id: removed,
                } if removed == session_id => {
                    saw_removed_after_failed = saw_failed;
                }
                _ => {}
            }
        }
        assert!(saw_failed);
        assert!(saw_removed_after_failed);

        let _ = fs::remove_file(initial_path);
        let _ = fs::remove_file(blocked_base_path);
        let _ = fs::remove_dir(blocked_segment);
    }

    #[test]
    fn recording_queue_overflow_emits_degraded_status() {
        let mut pipeline = RecordingWritePipeline::spawn(1024 * 1024);
        pipeline.set_history_include_input(true);
        let mut event_rx = pipeline
            .take_event_receiver()
            .expect("the event receiver should be available");
        let writer = pipeline.writer();
        let session_id = "session-overflow";
        let path = PathBuf::from(unique_recording_path("queue-overflow"));

        writer
            .start(
                session_id.to_string(),
                recording_context(session_id),
                recording_profile(&path),
                Some(path.clone()),
                1024 * 1024,
            )
            .expect("recording should start");
        let release_writer = writer.block_writer();
        let oversized = RECORDING_WRITE_QUEUE_BYTE_LIMIT as usize + 1;
        for _ in 0..3 {
            writer.write_output(session_id, "x".repeat(oversized));
        }
        {
            let dropped = writer
                .dropped
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let state = dropped
                .sessions
                .get(session_id)
                .expect("overflow should create one pending report");
            assert!(state.command_pending);
            assert_eq!(state.bytes, (oversized as u64) * 3);
        }
        release_writer.send(()).expect("release recording writer");
        writer.flush();

        let mut degraded_events = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            if let RecordingWriteEvent::Status(status) = event
                && status.state == RecordingStatusState::Degraded
            {
                degraded_events.push(status);
            }
        }
        assert_eq!(degraded_events.len(), 1);
        let degraded_event = degraded_events.pop().unwrap();
        assert_eq!(
            degraded_event.dropped_bytes,
            (RECORDING_WRITE_QUEUE_BYTE_LIMIT + 1) * 3
        );
        assert_eq!(degraded_event.queued_bytes, 0);
        assert!(degraded_event.last_error.is_some());

        assert!(writer.stop(session_id.to_string()).is_err());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn recording_pipeline_cleanup_runs_after_queued_writes_and_removes_status() {
        let mut pipeline = RecordingWritePipeline::spawn(1024 * 1024);
        pipeline.set_history_include_input(true);
        let mut event_rx = pipeline
            .take_event_receiver()
            .expect("recording events should be available");
        let writer = pipeline.writer();
        let session_id = "session-b";
        let path = PathBuf::from(unique_recording_path("pipeline-cleanup"));
        writer
            .start(
                session_id.to_string(),
                recording_context(session_id),
                recording_profile(&path),
                Some(path.clone()),
                1024 * 1024,
            )
            .expect("recording should start");

        pipeline.write_output(session_id, "before cleanup\n");
        pipeline.cleanup_session(session_id);
        writer.flush();

        let text = fs::read_to_string(&path).expect("recording file should exist");
        assert!(text.contains("before cleanup"));
        let mut removed = false;
        while let Ok(event) = event_rx.try_recv() {
            removed |= matches!(
                event,
                RecordingWriteEvent::StatusRemoved { session_id: removed }
                    if removed == session_id
            );
        }
        assert!(removed);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn recording_pipeline_rekeys_status_and_late_start_to_current_session() {
        let mut pipeline = RecordingWritePipeline::spawn(1024 * 1024);
        pipeline.set_history_include_input(true);
        let mut event_rx = pipeline.take_event_receiver().unwrap();
        let writer = pipeline.writer();
        let path = PathBuf::from(unique_recording_path("pipeline-rekey"));
        pipeline.write_output("s1", "before\n");
        pipeline.disconnect_session("s1");
        pipeline.write_output("s2", "early\n");
        pipeline.rekey_session("s1", "s2");
        writer
            .start(
                "s1".to_string(),
                recording_context("s1"),
                recording_profile(&path),
                Some(path.clone()),
                1024 * 1024,
            )
            .unwrap();
        pipeline.write_output("s2", "after\n");
        writer.flush();
        let mut removed_old = false;
        let mut status_new = false;
        while let Ok(event) = event_rx.try_recv() {
            match event {
                RecordingWriteEvent::StatusRemoved { session_id } if session_id == "s1" => {
                    removed_old = true;
                }
                RecordingWriteEvent::Status(status) if status.session_id == "s2" => {
                    status_new = true;
                }
                _ => {}
            }
        }
        assert!(removed_old && status_new);
        writer.stop("s2".to_string()).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("after"));
        pipeline.cleanup_session("s2");
        writer.flush();
        assert!(
            writer
                .start(
                    "s1".to_string(),
                    recording_context("s1"),
                    recording_profile(&path),
                    Some(path.clone()),
                    1024 * 1024,
                )
                .is_err()
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn recording_pipeline_search_runs_after_queued_writes() {
        let mut pipeline = RecordingWritePipeline::spawn(1024 * 1024);
        pipeline.set_history_include_input(true);
        let session_id = "session-c";
        let key = RecordingHistorySearchKey {
            session_id: session_id.to_string(),
            query: "queued".to_string(),
            case_sensitive: false,
            regex: false,
            whole_word: false,
            limit: Some(8),
            context_before: Some(0),
            context_after: Some(0),
            max_lines: Some(30_000),
        };

        pipeline.write_output(session_id, "queued history line\n");
        pipeline.request_history_search(key.clone());
        pipeline.writer().flush();

        let event = pipeline
            .take_event_receiver()
            .expect("the pipeline holds its receiver until the drain starts")
            .try_recv()
            .expect("search event should be queued");
        let RecordingWriteEvent::HistorySearch(event) = event else {
            panic!("expected history search event");
        };
        assert_eq!(event.key, key);
        assert_eq!(event.result.expect("search should succeed").total, 1);
    }

    #[test]
    fn default_input_capture_does_not_enqueue_payloads() {
        let mut pipeline = RecordingWritePipeline::spawn(1024);
        let writer = pipeline.writer();
        let release = writer.block_writer();
        writer.write_input("audit", b"synthetic-no-echo\r".to_vec());
        writer.write_raw_input("audit", b"synthetic-raw".to_vec());
        assert_eq!(
            writer
                .queued_bytes
                .load(std::sync::atomic::Ordering::Acquire),
            0
        );
        assert_eq!(writer.queue.lock().unwrap().count, 0);
        release.send(()).unwrap();
        pipeline.shutdown();
    }

    #[test]
    fn async_auto_start_orders_all_sessions_before_output_and_is_idempotent() {
        let dir = nyaterm_core::test_support::TestTempDir::new("nyaterm-recording-auto-start");
        let mut pipeline = RecordingWritePipeline::spawn(4096);
        let writer = pipeline.writer();
        let first_path = dir.path().join("first.log");
        let second_path = dir.path().join("second.log");
        let first = writer.start_async(
            "first".into(),
            recording_context("first"),
            recording_profile(&first_path),
            Some(first_path.clone()),
            4096,
            true,
        );
        let second = writer.start_async(
            "second".into(),
            recording_context("second"),
            recording_profile(&second_path),
            Some(second_path.clone()),
            4096,
            true,
        );
        writer.write_output("first", "first output\n");
        writer.write_output("second", "second output\n");
        let first_result = futures::executor::block_on(first).unwrap().unwrap();
        futures::executor::block_on(second).unwrap().unwrap();
        let repeated = writer.start_async(
            "first".into(),
            recording_context("first"),
            recording_profile(&first_path),
            Some(first_path.clone()),
            4096,
            true,
        );
        assert_eq!(
            futures::executor::block_on(repeated).unwrap().unwrap(),
            first_result
        );
        pipeline.shutdown();
        assert!(
            fs::read_to_string(first_path)
                .unwrap()
                .contains("first output")
        );
        assert!(
            fs::read_to_string(second_path)
                .unwrap()
                .contains("second output")
        );
    }

    #[test]
    fn shutdown_submits_partial_tail_and_closes_writer_after_queued_output() {
        let dir = nyaterm_core::test_support::TestTempDir::new("nyaterm-recording-shutdown");
        let path = dir.path().join("tail.log");
        let mut pipeline = RecordingWritePipeline::spawn(4096);
        let writer = pipeline.writer();
        writer
            .start(
                "audit".into(),
                recording_context("audit"),
                recording_profile(&path),
                Some(path.clone()),
                4096,
            )
            .unwrap();
        writer.write_output("audit", "partial tail");
        pipeline.shutdown();
        assert!(fs::read_to_string(path).unwrap().contains("partial tail"));
    }

    #[test]
    fn command_limit_bounds_small_payloads_and_reports_degradation() {
        let dir = nyaterm_core::test_support::TestTempDir::new("nyaterm-recording-count-limit");
        let path = dir.path().join("out.log");
        let mut pipeline = RecordingWritePipeline::spawn(4096);
        let writer = pipeline.writer();
        let mut events = pipeline.take_event_receiver().unwrap();
        writer
            .start(
                "audit".into(),
                recording_context("audit"),
                recording_profile(&path),
                Some(path),
                4096,
            )
            .unwrap();
        let release = writer.block_writer();
        for _ in 0..super::RECORDING_WRITE_QUEUE_COMMAND_LIMIT + 10 {
            writer.write_output("audit", "x");
        }
        assert_eq!(
            writer.queue.lock().unwrap().count,
            super::RECORDING_WRITE_QUEUE_COMMAND_LIMIT
        );
        release.send(()).unwrap();
        writer.flush();
        assert_eq!(writer.queue.lock().unwrap().count, 0);
        assert!(std::iter::from_fn(|| events.try_recv().ok()).any(|event|
            matches!(event, RecordingWriteEvent::Status(status) if status.state == RecordingStatusState::Degraded)));
        pipeline.shutdown();
    }

    #[test]
    fn healthy_rotation_updates_status_and_status_mailbox_is_bounded() {
        let dir = nyaterm_core::test_support::TestTempDir::new("nyaterm-recording-status-mailbox");
        let path = dir.path().join("out.log");
        let mut pipeline = RecordingWritePipeline::spawn(4096);
        let writer = pipeline.writer();
        let mut events = pipeline.take_event_receiver().unwrap();
        let mut profile = recording_profile(&path);
        profile.rotation = RecordingRotationPolicy::Size { max_bytes: 16 };
        writer
            .start(
                "audit".into(),
                recording_context("audit"),
                profile,
                Some(path.clone()),
                4096,
            )
            .unwrap();
        for _ in 0..100 {
            writer.write_output("audit", "complete output line\n");
        }
        writer.flush();
        assert_eq!(events.statuses.lock().unwrap().len(), 1);
        let status = std::iter::from_fn(|| events.try_recv().ok())
            .find_map(|event| match event {
                RecordingWriteEvent::Status(status) => Some(status),
                _ => None,
            })
            .unwrap();
        assert_ne!(status.file_path, Some(path));
        assert_eq!(status.state, RecordingStatusState::Recording);
        pipeline.shutdown();
    }

    #[test]
    fn idle_small_recordings_are_flushed_without_stop() {
        let dir = nyaterm_core::test_support::TestTempDir::new("nyaterm-recording-idle-flush");
        let path = dir.path().join("out.log");
        let mut pipeline = RecordingWritePipeline::spawn(4096);
        let writer = pipeline.writer();
        writer
            .start(
                "audit".into(),
                recording_context("audit"),
                recording_profile(&path),
                Some(path.clone()),
                4096,
            )
            .unwrap();
        writer.write_output("audit", "complete output line\n");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while fs::metadata(&path).unwrap().len() == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(fs::metadata(path).unwrap().len() > 0);
        pipeline.shutdown();
    }

    #[test]
    fn raw_stream_ignores_decoded_and_local_output() {
        let dir = nyaterm_core::test_support::TestTempDir::new("nyaterm-recording-raw-stream");
        let path = dir.path().join("out.raw.log");
        let mut pipeline = RecordingWritePipeline::spawn(4096);
        let writer = pipeline.writer();
        let mut profile = recording_profile(&path);
        profile.mode = RecordingMode::Raw;
        writer
            .start(
                "audit".into(),
                recording_context("audit"),
                profile,
                Some(path.clone()),
                4096,
            )
            .unwrap();
        writer.write_raw_output("audit", b"\xb2\xe2\xff\x1b[31m");
        writer.write_output("audit", "decoded text and local diagnostic");
        writer.write_raw_input("audit", b"\x1b[200~typed\x1b[201~".to_vec());
        pipeline.shutdown();
        assert_eq!(fs::read(&path).unwrap(), b"\xb2\xe2\xff\x1b[31m");
        assert_eq!(
            fs::read(path.with_file_name("out.raw.log.input.bin")).unwrap(),
            b"\x1b[200~typed\x1b[201~"
        );
    }

    fn recording_context(session_id: &str) -> RecordingContext {
        RecordingContext {
            session_id: session_id.to_string(),
            session_name: session_id.to_string(),
            connection_id: None,
            connection_name: None,
            group_path: None,
            protocol: "local".to_string(),
            host: None,
            port: None,
            username: None,
            started_at: OffsetDateTime::now_utc(),
        }
    }

    fn recording_profile(path: &Path) -> RecordingProfile {
        RecordingProfile {
            mode: RecordingMode::Transcript,
            base_path: path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(".")),
            path_template: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("recording.log")
                .to_string(),
            include_timestamps: false,
            include_io_labels: true,
            include_session_metadata: false,
            rotation: RecordingRotationPolicy::Session,
            existing_file_behavior: ExistingFileBehavior::Overwrite,
            include_binary_transfer_payloads: false,
            include_input: true,
        }
    }

    fn unique_recording_path(name: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos();
        std::env::temp_dir()
            .join(format!("nyaterm-{name}-{nanos}.log"))
            .to_string_lossy()
            .to_string()
    }
    #[test]
    fn completed_history_replies_are_coalesced_per_session() {
        let mut pipeline = RecordingWritePipeline::spawn(4096);
        let mut events = pipeline.take_event_receiver().unwrap();
        for index in 0..64 {
            pipeline.request_history_search(RecordingHistorySearchKey {
                session_id: "s".into(),
                query: format!("query-{index}"),
                case_sensitive: false,
                regex: false,
                whole_word: false,
                limit: None,
                context_before: None,
                context_after: None,
                max_lines: None,
            });
            pipeline.writer().flush();
        }
        assert_eq!(pipeline.handle.search_results.lock().unwrap().len(), 1);
        let RecordingWriteEvent::HistorySearch(result) = events.try_recv().unwrap() else {
            panic!("history reply");
        };
        assert_eq!(result.key.query, "query-63");
        assert!(events.try_recv().is_err());
    }
}
