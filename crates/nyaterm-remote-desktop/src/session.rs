use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Child;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::JoinHandle;

use uuid::Uuid;

use crate::heartbeat::{Heartbeat, Supervisor};
use crate::helper_process;
use crate::presentation::{
    PresentationClient, PresentationWorker, RdpPresentation, RdpPresentationConfig,
    RdpPresentationId, RdpPresentationMetrics,
};
use crate::protocol::{MAX_HEIGHT, MAX_WIDTH, MIN_HEIGHT, MIN_WIDTH};
use crate::{
    CursorPosition, CursorShape, CursorVisibility, FRAME_PAYLOAD_LIMIT, PROTOCOL_VERSION,
    PacketType, QueueWaker, RdpCertificateResponse, RdpControlMessage, RdpDisplayMetrics, RdpError,
    RdpErrorKind, RdpFrameEvent, RdpInputEvent, RdpRedirectRequest, RdpRedirectResponse,
    RdpRuntimeEvent, RdpServerCapabilities, RdpSessionConfig, RdpSessionDrain, RdpSessionState,
    RemoteCursorEvent, RemotePointerEvent, decode_control, decode_cursor_packet_owned,
    decode_frame_packet_owned, encode_control, validate_committed_text,
};
use nyaterm_core::runtime::reaper::Reaper;

const FRAME_QUEUE_LIMIT: usize = 64;
const FRAME_QUEUE_BYTE_LIMIT: usize = FRAME_PAYLOAD_LIMIT;
const CONTROL_QUEUE_LIMIT: usize = 256;
const CONTROL_QUEUE_BYTE_LIMIT: usize = 4 * 1024 * 1024;
const HELPER_PACKAGE: &str = "nyaterm-rdp-helper";
const HELPER_ENV_VAR: &str = "NYATERM_RDP_HELPER";

pub fn resolve_helper_path() -> Result<PathBuf, RdpError> {
    helper_process::resolve_helper(HELPER_PACKAGE, HELPER_ENV_VAR)
        .map_err(|message| RdpError::new(RdpErrorKind::HelperMissing, message))
}

#[derive(Default)]
struct EventQueueState {
    waker: Option<QueueWaker>,
    control: VecDeque<RdpRuntimeEvent>,
    control_bytes: usize,
    frames: VecDeque<RdpFrameEvent>,
    frame_bytes: usize,
    cursor_shape: Option<CursorShape>,
    cursor_position: Option<CursorPosition>,
    cursor_visibility: Option<CursorVisibility>,
    current_epoch: Option<u64>,
    frame_update_complete: bool,
    update_sequence: u64,
    closed: bool,
}

fn frame_byte_cost(frame: &RdpFrameEvent) -> usize {
    match frame {
        RdpFrameEvent::Bitmap { pixels, .. } => pixels.len(),
        _ => 0,
    }
}

fn control_byte_cost(event: &RdpRuntimeEvent) -> usize {
    match event {
        RdpRuntimeEvent::State {
            session_id,
            message,
            ..
        } => session_id.len() + message.as_ref().map_or(0, String::len) + 64,
        RdpRuntimeEvent::Frame { session_id, .. } => session_id.len() + 64,
        RdpRuntimeEvent::Cursor { session_id, event } => {
            session_id.len()
                + match event {
                    RemoteCursorEvent::Shape(shape) => shape.pixels.len() + 64,
                    RemoteCursorEvent::Position(_) | RemoteCursorEvent::Visibility(_) => 32,
                }
        }
        RdpRuntimeEvent::ManagedClipboard { session_id, event } => {
            session_id.len()
                + serde_json::to_vec(event).map_or(crate::CONTROL_PAYLOAD_LIMIT, |v| v.len())
        }
        RdpRuntimeEvent::Clipboard {
            session_id, text, ..
        } => session_id.len() + text.len() + 64,
        RdpRuntimeEvent::ClipboardTransfer {
            session_id,
            progress,
        } => {
            session_id.len()
                + progress.id.len()
                + progress.name.len()
                + progress.error.as_ref().map_or(0, String::len)
                + 128
        }
        RdpRuntimeEvent::RedirectRequest(request) => {
            request.session_id.len()
                + request.request_id.len()
                + request.host.len()
                + request.connect_hosts.iter().map(String::len).sum::<usize>()
                + 128
        }
        RdpRuntimeEvent::CertificateRequest(request) => {
            request.request_id.len()
                + request.host.len()
                + request.sha256_fingerprint.len()
                + request.subject.as_ref().map_or(0, String::len)
                + request.issuer.as_ref().map_or(0, String::len)
                + request.valid_from.as_ref().map_or(0, String::len)
                + request.valid_to.as_ref().map_or(0, String::len)
                + 128
        }
        RdpRuntimeEvent::Capability { session_id, .. } => session_id.len() + 32,
        RdpRuntimeEvent::Error {
            session_id, error, ..
        } => session_id.len() + error.message.len() + 64,
    }
}

struct EventQueue {
    presentation: OnceLock<PresentationClient>,
    state: Mutex<EventQueueState>,
    space_available: Condvar,
    frame_item_limit: usize,
    frame_byte_limit: usize,
    heartbeat: Arc<Heartbeat>,
}

impl Default for EventQueue {
    fn default() -> Self {
        Self::with_limits(FRAME_QUEUE_LIMIT, FRAME_QUEUE_BYTE_LIMIT)
    }
}

impl EventQueue {
    fn with_limits(frame_item_limit: usize, frame_byte_limit: usize) -> Self {
        Self {
            presentation: OnceLock::new(),
            state: Mutex::new(EventQueueState::default()),
            space_available: Condvar::new(),
            frame_item_limit,
            frame_byte_limit,
            heartbeat: Arc::new(Heartbeat::default()),
        }
    }

    fn with_waker(waker: Option<QueueWaker>) -> Self {
        let queue = Self::default();
        queue
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .waker = waker;
        queue
    }

    fn wake(state: &EventQueueState) {
        if let Some(waker) = &state.waker {
            waker();
        }
    }

    fn push_control(&self, event: RdpRuntimeEvent) -> bool {
        let cost = control_byte_cost(&event);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while !state.closed
            && (state.control.len() >= CONTROL_QUEUE_LIMIT
                || state.control_bytes.saturating_add(cost) > CONTROL_QUEUE_BYTE_LIMIT)
        {
            self.heartbeat.blocked(true);
            state = self
                .space_available
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            self.heartbeat.blocked(false);
        }
        if state.closed {
            return false;
        }
        state.control_bytes += cost;
        state.control.push_back(event);
        Self::wake(&state);
        true
    }

    fn push_control_force(&self, event: RdpRuntimeEvent) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.control_bytes = state
            .control_bytes
            .saturating_add(control_byte_cost(&event));
        state.control.push_back(event);
        Self::wake(&state);
    }

    fn push_cursor(&self, cursor: RemoteCursorEvent) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed {
            return false;
        }
        match cursor {
            RemoteCursorEvent::Shape(shape) => state.cursor_shape = Some(shape),
            RemoteCursorEvent::Position(position) => state.cursor_position = Some(position),
            RemoteCursorEvent::Visibility(visibility) => {
                state.cursor_visibility = Some(visibility);
            }
        }
        Self::wake(&state);
        true
    }

    fn push_reset(&self, session_id: &str, epoch: u64, width: u32, height: u32) -> bool {
        if let Some(presentation) = self.presentation.get()
            && !presentation
                .frame(RdpFrameEvent::Reset {
                    epoch,
                    width,
                    height,
                })
                .unwrap_or(false)
        {
            return false;
        }
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.closed {
                return false;
            }
            state.current_epoch = Some(epoch);
            state.update_sequence = 0;
            state.frames.clear();
            state.frame_bytes = 0;
            state.cursor_shape = None;
            state.cursor_position = None;
            state.cursor_visibility = None;
        }
        self.space_available.notify_all();
        self.push_control(RdpRuntimeEvent::Frame {
            session_id: session_id.to_string(),
            event: RdpFrameEvent::Reset {
                epoch,
                width,
                height,
            },
        })
    }

    /// Only the IPC reader submits pixels, preserving reset and completion order.
    fn receive_frame(&self, frame: RdpFrameEvent) -> Result<bool, RdpError> {
        let Some(presentation) = self.presentation.get() else {
            return Ok(self.push_frame(frame));
        };
        let RdpFrameEvent::Bitmap { epoch, .. } = &frame else {
            return Err(RdpError::new(
                RdpErrorKind::Protocol,
                "unexpected reset in frame packet",
            ));
        };
        let epoch = *epoch;
        let sequence = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.closed {
                return Ok(false);
            }
            if state.current_epoch != Some(epoch) {
                return Ok(true);
            }
            if state.frame_update_complete {
                None
            } else {
                state.update_sequence = state.update_sequence.checked_add(1).ok_or_else(|| {
                    RdpError::new(RdpErrorKind::Protocol, "RDP update sequence exhausted")
                })?;
                Some(state.update_sequence)
            }
        };
        if !presentation.frame(frame).map_err(presentation_error)? {
            return Ok(false);
        }
        if let Some(sequence) = sequence {
            return presentation
                .complete(epoch, sequence)
                .map_err(presentation_error);
        }
        Ok(true)
    }

    fn complete_update(&self, epoch: u64, sequence: u64) -> Result<(), RdpError> {
        let Some(presentation) = self.presentation.get() else {
            return Err(RdpError::new(
                RdpErrorKind::Protocol,
                "unrequested RDP update completion",
            ));
        };
        {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.closed {
                return Ok(());
            }
            if !state.frame_update_complete {
                return Err(RdpError::new(
                    RdpErrorKind::Protocol,
                    "unnegotiated RDP update completion",
                ));
            }
            if state.current_epoch != Some(epoch) {
                return Ok(());
            }
            if sequence <= state.update_sequence {
                return Err(RdpError::new(
                    RdpErrorKind::Protocol,
                    "RDP update completion is out of order",
                ));
            }
            state.update_sequence = sequence;
        }
        presentation
            .complete(epoch, sequence)
            .map_err(presentation_error)?;
        Ok(())
    }

    fn push_frame(&self, frame: RdpFrameEvent) -> bool {
        let cost = frame_byte_cost(&frame);
        if cost > self.frame_byte_limit {
            return false;
        }
        let epoch = match &frame {
            RdpFrameEvent::Bitmap { epoch, .. } => Some(*epoch),
            _ => None,
        };
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if epoch.is_some() && state.current_epoch != epoch {
            return false;
        }
        while !state.closed
            && (state.frames.len() >= self.frame_item_limit
                || state.frame_bytes.saturating_add(cost) > self.frame_byte_limit)
        {
            self.heartbeat.blocked(true);
            state = self
                .space_available
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            self.heartbeat.blocked(false);
            if epoch.is_some() && state.current_epoch != epoch {
                return false;
            }
        }
        if state.closed {
            return false;
        }
        state.frame_bytes += cost;
        state.frames.push_back(frame);
        Self::wake(&state);
        true
    }

    fn drain(&self) -> RdpSessionDrain {
        self.drain_with_limit(false)
    }

    fn drain_with_limit(&self, bounded: bool) -> RdpSessionDrain {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let count = if bounded {
            crate::frame::frame_batch_len(&state.frames)
        } else {
            state.frames.len()
        };
        let frames: Vec<RdpFrameEvent> = state.frames.drain(..count).collect();
        state.frame_bytes = state
            .frame_bytes
            .saturating_sub(frames.iter().map(frame_byte_cost).sum());
        let mut cursors = Vec::with_capacity(3);
        cursors.extend(state.cursor_shape.take().map(RemoteCursorEvent::Shape));
        cursors.extend(
            state
                .cursor_position
                .take()
                .map(RemoteCursorEvent::Position),
        );
        cursors.extend(
            state
                .cursor_visibility
                .take()
                .map(RemoteCursorEvent::Visibility),
        );
        let drain = RdpSessionDrain {
            control: state.control.drain(..).collect(),
            frames,
            cursors,
        };
        state.control_bytes = 0;
        drop(state);
        self.space_available.notify_all();
        drain
    }

    fn close(&self) {
        if let Some(presentation) = self.presentation.get() {
            presentation.close();
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.closed = true;
        state.control.clear();
        state.control_bytes = 0;
        state.frames.clear();
        state.frame_bytes = 0;
        state.cursor_shape = None;
        state.cursor_position = None;
        state.cursor_visibility = None;
        drop(state);
        self.space_available.notify_all();
    }
}

struct SessionRecord {
    presentation_worker: Option<PresentationWorker>,
    _component: Option<nyaterm_core::plugins::native::NativeLaunch>,
    state: Arc<Mutex<RdpSessionState>>,
    capabilities: Arc<Mutex<Option<RdpServerCapabilities>>>,
    queue: Arc<EventQueue>,
    writer: Arc<helper_process::IpcWriter>,
    child: Arc<Mutex<Option<Child>>>,
    supervisor: Option<Supervisor>,
    reader: Option<JoinHandle<()>>,
    stderr: Option<helper_process::StderrCapture>,
}

#[derive(Default)]
pub struct RdpSessionManager {
    presentation_config: Option<RdpPresentationConfig>,
    sessions: Mutex<HashMap<String, SessionRecord>>,
    reaper: Mutex<Option<Reaper<SessionRecord>>>,
    pending_certificates: Arc<Mutex<HashMap<String, String>>>,
    /// Installed once by the application; copied into every session queue so
    /// the reader thread can wake the consumer instead of being polled.
    queue_waker: Mutex<Option<QueueWaker>>,
}

impl RdpSessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Compose ordered deltas in the background instead of exposing raw pixels.
    pub fn with_presentation_pipeline(config: RdpPresentationConfig) -> Self {
        let mut manager = Self::default();
        manager.presentation_config = Some(config);
        manager
    }

    pub fn take_presentation(&self, session_id: &str) -> Option<RdpPresentation> {
        self.presentation_client(session_id)?.take()
    }

    pub fn acknowledge_presentation(&self, session_id: &str, id: RdpPresentationId) {
        if let Some(client) = self.presentation_client(session_id) {
            client.acknowledge(id);
        }
    }

    pub fn set_presentation_visible(
        &self,
        session_id: &str,
        visible: bool,
        config: RdpPresentationConfig,
    ) {
        if let Some(client) = self.presentation_client(session_id) {
            client.set_visible(visible, config);
        }
    }

    pub fn invalidate_presentation(&self, session_id: &str) {
        if let Some(client) = self.presentation_client(session_id) {
            client.invalidate();
        }
    }

    pub fn presentation_metrics(&self, session_id: &str) -> Option<RdpPresentationMetrics> {
        Some(self.presentation_client(session_id)?.metrics())
    }

    fn presentation_client(&self, session_id: &str) -> Option<PresentationClient> {
        self.sessions
            .lock()
            .ok()?
            .get(session_id)?
            .queue
            .presentation
            .get()
            .cloned()
    }

    /// Install the waker every session queue signals after enqueuing.
    ///
    /// Sessions created before this call keep polling semantics, so install it
    /// during startup, before any session exists.
    pub fn set_queue_waker(&self, waker: QueueWaker) {
        if let Ok(mut slot) = self.queue_waker.lock() {
            *slot = Some(waker);
        }
    }

    fn queue_waker(&self) -> Option<QueueWaker> {
        self.queue_waker.lock().ok()?.clone()
    }

    fn ensure_reaper(&self) -> Result<(), RdpError> {
        let mut slot = self
            .reaper
            .lock()
            .map_err(|_| RdpError::new(RdpErrorKind::Ipc, "reaper lock poisoned"))?;
        if slot.is_none() {
            *slot = Some(
                Reaper::new("rdp-reaper", 64, |mut record: SessionRecord| {
                    cleanup_child(&mut record)
                })
                .map_err(|error| RdpError::new(RdpErrorKind::Ipc, error.to_string()))?,
            );
        }
        Ok(())
    }

    pub fn create_session(&self, config: RdpSessionConfig) -> Result<String, RdpError> {
        self.create_session_with_id(Uuid::new_v4().to_string(), config)
    }

    pub fn create_session_with_id(
        &self,
        session_id: String,
        config: RdpSessionConfig,
    ) -> Result<String, RdpError> {
        self.create_session_with_component(session_id, config, None)
    }

    pub fn create_session_with_launch(
        &self,
        session_id: String,
        config: RdpSessionConfig,
        component: nyaterm_core::plugins::native::NativeLaunch,
    ) -> Result<String, RdpError> {
        self.create_session_with_component(session_id, config, Some(component))
    }

    fn create_session_with_component(
        &self,
        session_id: String,
        mut config: RdpSessionConfig,
        component: Option<nyaterm_core::plugins::native::NativeLaunch>,
    ) -> Result<String, RdpError> {
        config.normalize_backend_settings();
        config.backend.validate_config(&config)?;
        if config.backend != crate::RdpBackend::Builtin {
            return Err(RdpError::new(
                RdpErrorKind::Unsupported,
                "Use the ActiveX host manager for the Windows backend",
            ));
        }

        validate_config(&config)?;
        {
            let mut sessions = self.sessions.lock().map_err(|_| {
                RdpError::new(RdpErrorKind::Ipc, "RDP session registry lock is poisoned")
            })?;
            if sessions
                .get(&session_id)
                .is_some_and(|record| record.reader.is_some())
            {
                return Err(RdpError::new(
                    RdpErrorKind::Protocol,
                    format!("RDP session '{session_id}' is already running"),
                ));
            }
            sessions.remove(&session_id);
        }
        self.ensure_reaper()?;
        let helper = match &component {
            Some(component) => component.executable.clone(),
            None => resolve_helper_path()?,
        };
        let helper_process::HelperProcess {
            mut child,
            stdin,
            stdout,
            stderr,
        } = helper_process::spawn_helper(&helper, "rdp").map_err(|error| {
            RdpError::new(
                RdpErrorKind::Ipc,
                format!("failed to spawn the RDP helper: {error}"),
            )
        })?;
        let writer = match helper_process::IpcWriter::spawn(
            stdin,
            format!("nyaterm-rdp-writer-{session_id}"),
        ) {
            Ok(writer) => writer,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                stderr.finish();
                return Err(RdpError::new(
                    RdpErrorKind::Ipc,
                    format!("failed to start the RDP IPC writer: {error}"),
                ));
            }
        };
        let queue = Arc::new(EventQueue::with_waker(self.queue_waker()));
        let state = Arc::new(Mutex::new(RdpSessionState::Connecting));
        let capabilities = Arc::new(Mutex::new(None));
        let mut record = SessionRecord {
            presentation_worker: None,
            _component: component,
            state: state.clone(),
            capabilities: capabilities.clone(),
            queue: queue.clone(),
            writer: Arc::new(writer),
            child: Arc::new(Mutex::new(Some(child))),
            reader: None,
            stderr: Some(stderr),
            supervisor: None,
        };
        if let Some(config) = self.presentation_config {
            let failure_queue = queue.clone();
            let failure_state = state.clone();
            let failure_id = session_id.clone();
            let worker = match PresentationWorker::spawn_with_limits(
                format!("nyaterm-rdp-presentation-{session_id}"),
                config,
                crate::RDP_FRAMEBUFFER_LIMITS,
                queue.heartbeat.clone(),
                self.queue_waker(),
                move |error| {
                    let error = presentation_error(error);
                    push_reader_error(
                        &failure_id,
                        &failure_queue,
                        &failure_state,
                        error.kind,
                        error.message,
                    );
                },
            ) {
                Ok(worker) => worker,
                Err(error) => {
                    cleanup_child(&mut record);
                    return Err(RdpError::new(
                        RdpErrorKind::Ipc,
                        format!("failed to start RDP presentation worker: {error}"),
                    ));
                }
            };
            let _ = queue.presentation.set(worker.client.clone());
            record.presentation_worker = Some(worker);
        }
        record.reader = match spawn_reader(
            session_id.clone(),
            stdout,
            queue.clone(),
            state.clone(),
            capabilities.clone(),
            ReaderStartup {
                writer: Arc::clone(&record.writer),
                config,
                pending_certificates: self.pending_certificates.clone(),
            },
        ) {
            Ok(reader) => Some(reader),
            Err(error) => {
                cleanup_child(&mut record);
                return Err(RdpError::new(
                    RdpErrorKind::Ipc,
                    format!("failed to start helper reader: {error}"),
                ));
            }
        };
        let ping_writer = Arc::clone(&record.writer);
        let timeout_queue = Arc::clone(&queue);
        let timeout_state = Arc::clone(&state);
        let timeout_id = session_id.clone();
        record.supervisor = match Supervisor::spawn(
            Arc::clone(&queue.heartbeat),
            Arc::clone(&record.child),
            move |sequence| {
                let _ = send_control(&ping_writer, &RdpControlMessage::Ping { sequence });
            },
            move || {
                push_reader_error(
                    &timeout_id,
                    &timeout_queue,
                    &timeout_state,
                    RdpErrorKind::Timeout,
                    "helper handshake/control loop did not respond within 10 seconds; check that the helper matches this application".into(),
                )
            },
        ) {
            Ok(supervisor) => Some(supervisor),
            Err(error) => {
                cleanup_child(&mut record);
                return Err(RdpError::new(
                    RdpErrorKind::Ipc,
                    format!("failed to start helper supervisor: {error}"),
                ));
            }
        };
        let start_result = send_control(
            &record.writer,
            &RdpControlMessage::ClientHello {
                managed_clipboard: true,
                version: PROTOCOL_VERSION,
                frame_update_complete: self.presentation_config.is_some(),
                server_redirection: true,
            },
        );
        if let Err(error) = start_result {
            cleanup_child(&mut record);
            return Err(error);
        }
        record.queue.push_control(RdpRuntimeEvent::State {
            session_id: session_id.clone(),
            state: RdpSessionState::Connecting,
            message: None,
        });
        let mut sessions = match self.sessions.lock() {
            Ok(sessions) => sessions,
            Err(_) => {
                cleanup_child(&mut record);
                return Err(RdpError::new(
                    RdpErrorKind::Ipc,
                    "RDP session registry lock is poisoned",
                ));
            }
        };
        if sessions.contains_key(&session_id) {
            drop(sessions);
            cleanup_child(&mut record);
            return Err(RdpError::new(
                RdpErrorKind::Protocol,
                format!("RDP session '{session_id}' was created concurrently"),
            ));
        }
        sessions.insert(session_id.clone(), record);
        Ok(session_id)
    }

    pub fn state(&self, session_id: &str) -> Option<RdpSessionState> {
        let sessions = self.sessions.lock().ok()?;
        sessions
            .get(session_id)?
            .state
            .lock()
            .ok()
            .map(|state| state.clone())
    }

    pub fn drain(&self, session_id: &str) -> RdpSessionDrain {
        let Ok(sessions) = self.sessions.lock() else {
            return RdpSessionDrain::default();
        };
        let Some(record) = sessions.get(session_id) else {
            return RdpSessionDrain::default();
        };
        record.queue.drain()
    }

    pub fn drain_events(&self, session_id: &str) -> Vec<RdpRuntimeEvent> {
        let mut drain = self.drain(session_id);
        drain
            .control
            .extend(drain.frames.drain(..).map(|event| RdpRuntimeEvent::Frame {
                session_id: session_id.to_string(),
                event,
            }));
        drain.control.extend(
            drain
                .cursors
                .drain(..)
                .map(|event| RdpRuntimeEvent::Cursor {
                    session_id: session_id.to_string(),
                    event,
                }),
        );
        drain.control
    }

    /// Drain control state and a bounded, ordered prefix of frame deltas.
    pub fn drain_batch(&self, session_id: &str) -> RdpSessionDrain {
        let Ok(sessions) = self.sessions.lock() else {
            return RdpSessionDrain::default();
        };
        sessions
            .get(session_id)
            .map(|record| record.queue.drain_with_limit(true))
            .unwrap_or_default()
    }

    /// Return the static helper capabilities confirmed by `ServerHello`.
    ///
    /// `None` means that no valid hello has been received; capability-dependent
    /// operations fail closed in that state.
    pub fn server_capabilities(&self, session_id: &str) -> Option<RdpServerCapabilities> {
        let sessions = self.sessions.lock().ok()?;
        *sessions.get(session_id)?.capabilities.lock().ok()?
    }

    pub fn send_input(&self, session_id: &str, events: Vec<RdpInputEvent>) -> Result<(), RdpError> {
        let needs_committed_text = validate_rdp_input(&events)?;
        if needs_committed_text {
            let capabilities = self.confirmed_capabilities(session_id)?;
            if !capabilities.committed_unicode_text {
                return Err(RdpError::new(
                    RdpErrorKind::Unsupported,
                    "RDP helper did not advertise committed Unicode text input",
                ));
            }
        }
        let move_only = !events.is_empty()
            && events.iter().all(|event| {
                matches!(
                    event,
                    RdpInputEvent::Pointer(RemotePointerEvent::Move { .. })
                )
            });
        let message = RdpControlMessage::Input {
            session_id: session_id.to_string(),
            events,
        };
        let packet = encode_control(&message)
            .map_err(|error| RdpError::new(RdpErrorKind::Ipc, error.to_string()))?;
        let sessions = self.sessions.lock().map_err(|_| {
            RdpError::new(RdpErrorKind::Ipc, "RDP session registry lock is poisoned")
        })?;
        let record = sessions.get(session_id).ok_or_else(|| {
            RdpError::new(
                RdpErrorKind::Protocol,
                format!("RDP session '{session_id}' was not found"),
            )
        })?;
        let result = if move_only {
            record.writer.send_latest_move(packet)
        } else {
            let release = encode_control(&RdpControlMessage::Input {
                session_id: session_id.to_string(),
                events: vec![RdpInputEvent::ReleaseAllInputs],
            })
            .map_err(|error| RdpError::new(RdpErrorKind::Ipc, error.to_string()))?;
            record.writer.send_reliable(packet, release)
        };
        result.map_err(|error| RdpError::new(RdpErrorKind::Ipc, error.to_string()))
    }

    pub fn send_secure_attention(&self, session_id: &str) -> Result<(), RdpError> {
        if !self.confirmed_capabilities(session_id)?.secure_attention {
            return Err(RdpError::new(
                RdpErrorKind::Unsupported,
                "RDP helper did not advertise Secure Attention",
            ));
        }
        self.send(
            session_id,
            RdpControlMessage::SecureAttention {
                session_id: session_id.to_string(),
            },
        )
    }

    pub fn resize(&self, session_id: &str, width: u32, height: u32) -> Result<(), RdpError> {
        self.resize_with_metrics(
            session_id,
            RdpDisplayMetrics {
                width,
                height,
                desktop_scale_factor: 100,
                physical_size_mm: None,
            },
        )
    }

    pub fn resize_with_metrics(
        &self,
        session_id: &str,
        metrics: RdpDisplayMetrics,
    ) -> Result<(), RdpError> {
        validate_size(metrics.width, metrics.height)?;
        if !(100..=500).contains(&metrics.desktop_scale_factor) {
            return Err(RdpError::new(
                RdpErrorKind::Protocol,
                "RDP desktop scale factor must be between 100 and 500",
            ));
        }
        self.send(
            session_id,
            RdpControlMessage::Resize {
                session_id: session_id.to_string(),
                metrics,
            },
        )
    }

    pub fn send_managed_clipboard(
        &self,
        session_id: &str,
        command: crate::clipboard::ClipboardCommand,
    ) -> Result<(), RdpError> {
        if !self.confirmed_capabilities(session_id)?.managed_clipboard {
            return Err(RdpError::new(
                RdpErrorKind::Unsupported,
                "Update the RDP component for managed clipboard support",
            ));
        }
        self.send(
            session_id,
            RdpControlMessage::ManagedClipboard {
                session_id: session_id.into(),
                command,
            },
        )
    }

    pub fn set_clipboard_text(&self, session_id: &str, text: String) -> Result<(), RdpError> {
        if text.len() > crate::MAX_CLIPBOARD_TEXT_BYTES {
            return Err(RdpError::new(
                RdpErrorKind::Unsupported,
                "clipboard text exceeds the 4 MiB limit",
            ));
        }
        self.send(
            session_id,
            RdpControlMessage::Clipboard {
                session_id: session_id.to_string(),
                text,
                generation: 0,
            },
        )
    }

    pub fn cancel_clipboard_transfer(
        &self,
        session_id: &str,
        transfer_id: &str,
    ) -> Result<(), RdpError> {
        self.send(
            session_id,
            RdpControlMessage::ClipboardTransferCancel {
                session_id: session_id.to_string(),
                transfer_id: transfer_id.to_string(),
            },
        )
    }

    /// Respond only after the helper advertised coordinated redirection.
    pub fn respond_redirect(
        &self,
        request: &RdpRedirectRequest,
        response: RdpRedirectResponse,
    ) -> Result<(), RdpError> {
        if !self
            .confirmed_capabilities(&request.session_id)?
            .server_redirection
        {
            return Err(RdpError::new(
                RdpErrorKind::Unsupported,
                "RDP helper requires an update to coordinate server redirection",
            ));
        }
        self.send(
            &request.session_id,
            RdpControlMessage::RedirectResponse {
                request: request.clone(),
                response,
            },
        )
    }

    pub fn certificate_is_pending(&self, session_id: &str, request_id: &str) -> bool {
        // Keep registry ownership stable, and use the same queue-before-challenge
        // lock order as the reader. A fatal reader revokes trust before GPUI drains it.
        let Ok(sessions) = self.sessions.lock() else {
            return false;
        };
        let Some(record) = sessions.get(session_id) else {
            return false;
        };
        let Ok(queue) = record.queue.state.lock() else {
            return false;
        };
        if queue.closed {
            return false;
        }
        let Ok(state) = record.state.lock() else {
            return false;
        };
        if !matches!(
            *state,
            RdpSessionState::Connecting
                | RdpSessionState::Connected
                | RdpSessionState::Reconnecting
        ) {
            return false;
        }
        self.pending_certificates.lock().is_ok_and(|pending| {
            pending
                .get(request_id)
                .is_some_and(|owner| owner == session_id)
        })
    }

    pub fn respond_certificate(
        &self,
        request_id: &str,
        response: RdpCertificateResponse,
    ) -> Result<(), RdpError> {
        let session_id = self
            .pending_certificates
            .lock()
            .map_err(|_| {
                RdpError::new(
                    RdpErrorKind::Ipc,
                    "RDP certificate registry lock is poisoned",
                )
            })?
            .remove(request_id)
            .ok_or_else(|| {
                RdpError::new(
                    RdpErrorKind::Protocol,
                    format!("RDP certificate request '{request_id}' was not found"),
                )
            })?;
        self.send(
            &session_id,
            RdpControlMessage::CertificateResponse {
                request_id: request_id.to_string(),
                response,
            },
        )
    }

    pub fn close(&self, session_id: &str) -> Result<(), RdpError> {
        let mut record = self
            .sessions
            .lock()
            .map_err(|_| RdpError::new(RdpErrorKind::Ipc, "RDP session registry lock is poisoned"))?
            .remove(session_id)
            .ok_or_else(|| {
                RdpError::new(
                    RdpErrorKind::Protocol,
                    format!("RDP session '{session_id}' was not found"),
                )
            })?;
        set_state(&record.state, RdpSessionState::Disconnecting);
        let _ = send_control(
            &record.writer,
            &RdpControlMessage::Input {
                session_id: session_id.to_string(),
                events: vec![RdpInputEvent::ReleaseAllInputs],
            },
        );
        let _ = send_control(
            &record.writer,
            &RdpControlMessage::Disconnect {
                session_id: session_id.to_string(),
            },
        );
        cleanup_child(&mut record);
        set_state(&record.state, RdpSessionState::Disconnected);
        record.queue.push_control_force(RdpRuntimeEvent::State {
            session_id: session_id.to_string(),
            state: RdpSessionState::Disconnected,
            message: None,
        });
        self.pending_certificates
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|_, pending_session| pending_session != session_id);
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(session_id.to_string(), record);
        Ok(())
    }

    /// Permanently remove the record before reaping its helper in the background.
    /// A reconnect using the same id cannot be overwritten by the old reader.
    pub fn close_detached(&self, session_id: &str) -> Result<(), RdpError> {
        self.ensure_reaper()?;
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| RdpError::new(RdpErrorKind::Ipc, "session registry lock poisoned"))?;
        let slot = self
            .reaper
            .lock()
            .map_err(|_| RdpError::new(RdpErrorKind::Ipc, "reaper lock poisoned"))?;
        slot.as_ref()
            .ok_or_else(|| RdpError::new(RdpErrorKind::Ipc, "reaper unavailable"))?
            .try_submit_with(|| {
                let record = sessions.remove(session_id)?;
                set_state(&record.state, RdpSessionState::Disconnecting);
                record.queue.close();
                self.pending_certificates
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .retain(|_, pending_session| pending_session != session_id);
                let _ = send_control(
                    &record.writer,
                    &RdpControlMessage::Disconnect {
                        session_id: session_id.to_owned(),
                    },
                );
                Some(record)
            })
            .map_err(|error| RdpError::new(RdpErrorKind::Ipc, error.to_string()))
    }

    pub fn shutdown(&self) {
        let ids = self
            .sessions
            .lock()
            .map(|sessions| {
                sessions
                    .iter()
                    .filter_map(|(id, record)| record.reader.is_some().then_some(id.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for id in ids {
            let _ = self.close(&id);
        }
    }

    fn confirmed_capabilities(&self, session_id: &str) -> Result<RdpServerCapabilities, RdpError> {
        let sessions = self.sessions.lock().map_err(|_| {
            RdpError::new(RdpErrorKind::Ipc, "RDP session registry lock is poisoned")
        })?;
        let record = sessions.get(session_id).ok_or_else(|| {
            RdpError::new(
                RdpErrorKind::Protocol,
                format!("RDP session '{session_id}' was not found"),
            )
        })?;
        record
            .capabilities
            .lock()
            .map_err(|_| RdpError::new(RdpErrorKind::Ipc, "RDP capability lock is poisoned"))?
            .as_ref()
            .copied()
            .ok_or_else(|| {
                RdpError::new(
                    RdpErrorKind::Protocol,
                    "RDP helper capabilities have not been confirmed",
                )
            })
    }

    fn send(&self, session_id: &str, message: RdpControlMessage) -> Result<(), RdpError> {
        let sessions = self.sessions.lock().map_err(|_| {
            RdpError::new(RdpErrorKind::Ipc, "RDP session registry lock is poisoned")
        })?;
        let record = sessions.get(session_id).ok_or_else(|| {
            RdpError::new(
                RdpErrorKind::Protocol,
                format!("RDP session '{session_id}' was not found"),
            )
        })?;
        send_control(&record.writer, &message)
    }
}

impl Drop for RdpSessionManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn send_control(
    writer: &helper_process::IpcWriter,
    message: &RdpControlMessage,
) -> Result<(), RdpError> {
    let packet = encode_control(message)
        .map_err(|error| RdpError::new(RdpErrorKind::Ipc, error.to_string()))?;
    writer
        .send_critical(packet)
        .map_err(|error| RdpError::new(RdpErrorKind::Ipc, error.to_string()))
}

struct ReaderStartup {
    pending_certificates: Arc<Mutex<HashMap<String, String>>>,
    writer: Arc<helper_process::IpcWriter>,
    config: RdpSessionConfig,
}

fn disable_legacy_clipboard(
    config: &mut RdpSessionConfig,
    capabilities: Option<RdpServerCapabilities>,
) -> bool {
    if config.redirection.clipboard && !capabilities.is_some_and(|c| c.managed_clipboard) {
        config.redirection.clipboard = false;
        config.normalize_backend_settings();
        true
    } else {
        false
    }
}

fn spawn_reader(
    session_id: String,
    mut stdout: std::process::ChildStdout,
    queue: Arc<EventQueue>,
    state: Arc<Mutex<RdpSessionState>>,
    capabilities: Arc<Mutex<Option<RdpServerCapabilities>>>,
    startup: ReaderStartup,
) -> std::io::Result<JoinHandle<()>> {
    let ReaderStartup {
        writer,
        config,
        pending_certificates,
    } = startup;
    nyaterm_core::runtime::worker::spawn(format!("nyaterm-rdp-reader-{session_id}"), move || {
        let mut hello_received = false;
        let mut pending_config = Some(config);
        loop {
            let packet = match crate::ipc::read_packet_budgeted(&mut stdout) {
                Ok(Some(packet)) => packet,
                Ok(None) => break,
                Err(error) => {
                    push_reader_error(
                        &session_id,
                        &queue,
                        &state,
                        if error.kind() == std::io::ErrorKind::OutOfMemory {
                            RdpErrorKind::ResourceExhausted
                        } else {
                            RdpErrorKind::Ipc
                        },
                        error.to_string(),
                    );
                    return;
                }
            };
            let result: Result<(), RdpError> = match packet.packet_type {
                PacketType::Control => decode_control(&packet)
                    .map_err(|error| RdpError::new(RdpErrorKind::Protocol, error.to_string()))
                    .and_then(|message| {
                        if let RdpControlMessage::Pong { sequence } = message {
                            require_server_hello(hello_received, "Pong")?;
                            queue.heartbeat.pong(sequence);
                            return Ok(());
                        }
                        handle_control(
                            &session_id,
                            message,
                            &queue,
                            &state,
                            &capabilities,
                            &pending_certificates,
                            &mut hello_received,
                        )
                    }),
                PacketType::Frame => require_server_hello(hello_received, "frame packet")
                    .and_then(|()| {
                        decode_frame_packet_owned(packet).map_err(|error| {
                            RdpError::new(RdpErrorKind::Protocol, error.to_string())
                        })
                    })
                    .and_then(|(frame_session, frame)| {
                        if frame_session == session_id {
                            queue.receive_frame(frame)?;
                        }
                        Ok(())
                    }),
                PacketType::Cursor => require_server_hello(hello_received, "cursor packet")
                    .and_then(|()| {
                        decode_cursor_packet_owned(packet).map_err(|error| {
                            RdpError::new(RdpErrorKind::Protocol, error.to_string())
                        })
                    })
                    .map(|(cursor_session, cursor)| {
                        if cursor_session == session_id {
                            queue.push_cursor(cursor);
                        }
                    }),
            };
            if result.is_ok() && hello_received {
                queue.heartbeat.hello();
                if let Some(mut config) = pending_config.take() {
                    if disable_legacy_clipboard(
                        &mut config,
                        capabilities.lock().ok().and_then(|c| *c),
                    ) {
                        queue.push_control(RdpRuntimeEvent::Error {
                            session_id: session_id.clone(),
                            error: RdpError::new(RdpErrorKind::Clipboard,
                                "Clipboard disabled for this connection; update the RDP native component in Plugins"),
                            fatal: false,
                        });
                    }
                    if let Err(error) = send_control(
                        &writer,
                        &RdpControlMessage::Connect {
                            session_id: session_id.clone(),
                            config,
                        },
                    ) {
                        push_reader_error(&session_id, &queue, &state, error.kind, error.message);
                        return;
                    }
                }
            }
            if let Err(error) = result {
                push_reader_error(&session_id, &queue, &state, error.kind, error.message);
                return;
            }
        }
        if !hello_received {
            push_reader_error(
                &session_id,
                &queue,
                &state,
                RdpErrorKind::Protocol,
                format!(
                    "RDP helper ended before IPC {PROTOCOL_VERSION} handshake; install a component supporting this IPC version"
                ),
            );
            return;
        }
        let current = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if !matches!(
            current,
            RdpSessionState::Disconnecting
                | RdpSessionState::Disconnected
                | RdpSessionState::Failed(_)
        ) {
            push_reader_error(
                &session_id,
                &queue,
                &state,
                RdpErrorKind::HelperCrashed,
                "RDP helper exited unexpectedly".to_string(),
            );
        } else {
            // EOF after an ordinary disconnect must reap the process without
            // later turning the completed connection into a heartbeat timeout.
            queue.heartbeat.fail_once();
        }
    })
}

fn require_server_hello(received: bool, packet_kind: &str) -> Result<(), RdpError> {
    if received {
        Ok(())
    } else {
        Err(RdpError::new(
            RdpErrorKind::Protocol,
            format!("RDP helper sent {packet_kind} before ServerHello"),
        ))
    }
}

fn handle_control(
    session_id: &str,
    message: RdpControlMessage,
    queue: &Arc<EventQueue>,
    state: &Arc<Mutex<RdpSessionState>>,
    capabilities: &Arc<Mutex<Option<RdpServerCapabilities>>>,
    pending: &Arc<Mutex<HashMap<String, String>>>,
    hello_received: &mut bool,
) -> Result<(), RdpError> {
    if !*hello_received {
        let RdpControlMessage::ServerHello {
            version,
            capabilities: confirmed,
        } = message
        else {
            return Err(RdpError::new(
                RdpErrorKind::Protocol,
                "RDP helper's first control message must be ServerHello",
            ));
        };
        if version != PROTOCOL_VERSION {
            return Err(RdpError::new(
                RdpErrorKind::Protocol,
                format!(
                    "RDP helper protocol version {version} does not match {PROTOCOL_VERSION}; install a component supporting this IPC version"
                ),
            ));
        }
        let mut slot = capabilities
            .lock()
            .map_err(|_| RdpError::new(RdpErrorKind::Ipc, "RDP capability lock is poisoned"))?;
        if slot.is_some() {
            return Err(RdpError::new(
                RdpErrorKind::Protocol,
                "RDP helper capabilities were already confirmed",
            ));
        }
        queue
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .frame_update_complete =
            queue.presentation.get().is_some() && confirmed.frame_update_complete;
        *slot = Some(confirmed);
        *hello_received = true;
        return Ok(());
    }

    match message {
        RdpControlMessage::ServerHello { .. } => {
            return Err(RdpError::new(
                RdpErrorKind::Protocol,
                "RDP helper sent duplicate ServerHello",
            ));
        }
        RdpControlMessage::Ping { .. }
        | RdpControlMessage::ClientHello { .. }
        | RdpControlMessage::Connect { .. }
        | RdpControlMessage::Input { .. }
        | RdpControlMessage::SecureAttention { .. }
        | RdpControlMessage::Resize { .. }
        | RdpControlMessage::ManagedClipboard { .. }
        | RdpControlMessage::ClipboardTransferCancel { .. }
        | RdpControlMessage::RedirectResponse { .. }
        | RdpControlMessage::CertificateResponse { .. }
        | RdpControlMessage::RequestFullFrame { .. }
        | RdpControlMessage::Disconnect { .. } => {
            return Err(RdpError::new(
                RdpErrorKind::Protocol,
                "RDP helper sent an application-only control message",
            ));
        }
        RdpControlMessage::DesktopReset {
            session_id: event_session,
            epoch,
            width,
            height,
        } if event_session == session_id => {
            queue.push_reset(session_id, epoch, width, height);
        }
        RdpControlMessage::FrameUpdateComplete {
            session_id: event_session,
            epoch,
            sequence,
        } if event_session == session_id => {
            queue.complete_update(epoch, sequence)?;
        }
        RdpControlMessage::State {
            session_id: event_session,
            state: new_state,
            message,
        } if event_session == session_id => {
            if matches!(
                new_state,
                RdpSessionState::Connecting | RdpSessionState::Reconnecting
            ) {
                if let Some(presentation) = queue.presentation.get() {
                    presentation.discard();
                }
                let mut queued = queue.state.lock().unwrap_or_else(|p| p.into_inner());
                queued.current_epoch = None;
                queued.update_sequence = 0;
            }
            set_state(state, new_state.clone());
            queue.push_control(RdpRuntimeEvent::State {
                session_id: session_id.to_string(),
                state: new_state,
                message,
            });
        }
        RdpControlMessage::ManagedClipboardEvent {
            session_id: event_session,
            event,
        } if event_session == session_id => {
            if !capabilities
                .lock()
                .ok()
                .is_some_and(|c| c.is_some_and(|c| c.managed_clipboard))
            {
                return Err(RdpError::new(
                    RdpErrorKind::Protocol,
                    "Unnegotiated clipboard event",
                ));
            }
            queue.push_control(RdpRuntimeEvent::ManagedClipboard {
                session_id: session_id.into(),
                event,
            });
        }
        RdpControlMessage::Clipboard {
            session_id: event_session,
            text,
            generation,
        } if event_session == session_id => {
            queue.push_control(RdpRuntimeEvent::Clipboard {
                session_id: session_id.to_string(),
                text,
                generation,
            });
        }
        RdpControlMessage::ClipboardTransfer {
            session_id: event_session,
            progress,
        } if event_session == session_id => {
            queue.push_control(RdpRuntimeEvent::ClipboardTransfer {
                session_id: session_id.to_string(),
                progress,
            });
        }
        RdpControlMessage::RedirectRequest(request) => {
            let negotiated = capabilities
                .lock()
                .map_err(|_| RdpError::new(RdpErrorKind::Ipc, "RDP capability lock is poisoned"))?
                .is_some_and(|capabilities| capabilities.server_redirection);
            if !negotiated || request.session_id != session_id || request.validate().is_err() {
                return Err(RdpError::new(
                    RdpErrorKind::Protocol,
                    "invalid or unnegotiated RDP redirection request",
                ));
            }
            queue.push_control(RdpRuntimeEvent::RedirectRequest(request));
        }
        RdpControlMessage::CertificateRequest(request) => {
            let request_id = request.request_id.clone();
            // Serialize registration with close's queue gate. An old reader must
            // never register a prompt after ownership has been removed.
            {
                let gate = queue.state.lock().unwrap_or_else(|p| p.into_inner());
                if gate.closed {
                    return Ok(());
                }
                pending
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(request_id.clone(), session_id.to_string());
            }
            if !queue.push_control(RdpRuntimeEvent::CertificateRequest(request)) {
                pending
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(&request_id);
            }
        }
        RdpControlMessage::Capability {
            session_id: event_session,
            capability,
        } if event_session == session_id => {
            queue.push_control(RdpRuntimeEvent::Capability {
                session_id: session_id.to_string(),
                capability,
            });
        }
        RdpControlMessage::Error {
            session_id: event_session,
            error,
            fatal,
        } if event_session == session_id => {
            let mut error = error;
            if error
                .message
                .contains("server redirection is not supported over a loopback relay")
                && !capabilities
                    .lock()
                    .is_ok_and(|slot| slot.is_some_and(|value| value.server_redirection))
            {
                error.message.push_str("; update the RDP native component in Plugins to enable coordinated server redirection");
            }
            if fatal {
                push_reader_error(session_id, queue, state, error.kind, error.message);
                return Ok(());
            }
            queue.push_control(RdpRuntimeEvent::Error {
                session_id: session_id.to_string(),
                error,
                fatal,
            });
        }
        _ => {}
    }
    Ok(())
}

fn push_reader_error(
    session_id: &str,
    queue: &Arc<EventQueue>,
    state: &Arc<Mutex<RdpSessionState>>,
    kind: RdpErrorKind,
    message: String,
) {
    if !queue.heartbeat.fail_once() {
        return;
    }
    queue.close();
    let error = RdpError::new(kind, message);
    set_state(state, RdpSessionState::Failed(error.clone()));
    queue.push_control_force(RdpRuntimeEvent::Error {
        session_id: session_id.to_string(),
        error: error.clone(),
        fatal: true,
    });
}

fn presentation_error(error: crate::VncError) -> RdpError {
    let kind = match error.kind {
        crate::VncErrorKind::ResourceExhausted => RdpErrorKind::ResourceExhausted,
        crate::VncErrorKind::Internal => RdpErrorKind::Ipc,
        _ => RdpErrorKind::Protocol,
    };
    RdpError::new(kind, error.message)
}

fn set_state(state: &Arc<Mutex<RdpSessionState>>, new_state: RdpSessionState) {
    *state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = new_state;
}

fn cleanup_child(record: &mut SessionRecord) {
    record.queue.close();
    if let Some(mut worker) = record.presentation_worker.take() {
        worker.shutdown();
    }
    if let Some(mut supervisor) = record.supervisor.take() {
        supervisor.stop();
    }
    helper_process::cleanup_child(
        &mut record.child.lock().unwrap_or_else(|p| p.into_inner()),
        &mut record.reader,
    );
    if let Some(stderr) = record.stderr.take() {
        stderr.finish();
    }
    if let Some(writer) = Arc::get_mut(&mut record.writer) {
        writer.shutdown();
    }
}

fn validate_rdp_input(events: &[RdpInputEvent]) -> Result<bool, RdpError> {
    let mut has_committed_text = false;
    for event in events {
        if let RdpInputEvent::Unicode { text } = event {
            validate_committed_text(text).map_err(|error| {
                RdpError::new(
                    RdpErrorKind::Protocol,
                    format!("invalid RDP committed text: {error}"),
                )
            })?;
            has_committed_text = true;
        }
    }
    Ok(has_committed_text)
}

fn validate_config(config: &RdpSessionConfig) -> Result<(), RdpError> {
    if config.host.trim().is_empty() {
        return Err(RdpError::new(
            RdpErrorKind::Protocol,
            "RDP host is required",
        ));
    }
    validate_size(config.display.width, config.display.height)?;
    if !matches!(config.display.color_depth, 16 | 24 | 32) {
        return Err(RdpError::new(
            RdpErrorKind::Unsupported,
            "RDP color depth must be 16, 24, or 32",
        ));
    }
    Ok(())
}

fn validate_size(width: u32, height: u32) -> Result<(), RdpError> {
    if !(MIN_WIDTH..=MAX_WIDTH).contains(&width) || !(MIN_HEIGHT..=MAX_HEIGHT).contains(&height) {
        return Err(RdpError::new(
            RdpErrorKind::Unsupported,
            "RDP desktop size is outside the supported range",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod presentation_tests;

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::{
        EventQueue, RdpSessionManager, SessionRecord, handle_control, require_server_hello,
        validate_rdp_input,
    };
    use crate::{
        CursorPosition, PROTOCOL_VERSION, PixelFormat, RdpCapability, RdpClipboardTransferProgress,
        RdpClipboardTransferStatus, RdpControlMessage, RdpFrameEvent, RdpInputEvent,
        RdpRuntimeEvent, RdpServerCapabilities, RdpSessionState, RemoteCursorEvent,
    };

    #[test]
    fn legacy_helper_disables_only_ephemeral_clipboard_settings() {
        let saved = crate::RdpSessionConfig {
            relay: None,
            backend: crate::RdpBackend::Builtin,
            windows_authentication: None,
            name: "fixture".into(),
            host: "localhost".into(),
            port: 3389,
            username: String::new(),
            domain: String::new(),
            password: None,
            use_nla: true,
            certificate_policy: Default::default(),
            administrative_session: false,
            display: Default::default(),
            clipboard: Default::default(),
            redirection: Default::default(),
            reconnect: Default::default(),
        };
        let mut launch = saved.clone();
        assert!(super::disable_legacy_clipboard(
            &mut launch,
            Some(RdpServerCapabilities::default())
        ));
        assert!(!launch.redirection.clipboard);
        assert!(saved.redirection.clipboard);
        assert_eq!(launch.clipboard.mode, crate::RdpClipboardMode::Disabled);
        let mut supported = saved;
        assert!(!super::disable_legacy_clipboard(
            &mut supported,
            Some(RdpServerCapabilities {
                managed_clipboard: true,
                ..Default::default()
            })
        ));
        assert!(supported.redirection.clipboard);
    }

    #[test]
    fn pixel_budget_survives_drain_and_releases_on_reset_and_close() {
        let budget = crate::pixel_memory::PixelBudget::test(1024);
        let frame = crate::RemoteFrameEvent::Bitmap {
            epoch: 1,
            full: true,
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            stride: 4,
            format: crate::PixelFormat::Bgra8,
            pixels: vec![0; 4].into(),
        };
        let packet = crate::encode_frame_packet_owned("s", frame).unwrap();
        let mut wire = Vec::new();
        crate::write_packet(&mut wire, &packet).unwrap();
        let received = crate::ipc::read_packet_inner(&mut wire.as_slice(), Some(&budget))
            .unwrap()
            .unwrap();
        let (_, frame) = crate::decode_frame_packet_owned(received).unwrap();
        let queue = EventQueue::default();
        queue.push_reset("s", 1, 1, 1);
        queue.push_frame(frame);
        let allocated = budget.used();
        assert!(allocated > 0);
        let drain = queue.drain();
        assert_eq!(budget.used(), allocated, "draining transfers the lease");
        let retained = drain.frames.clone();
        drop(drain);
        queue.push_reset("s", 2, 1, 1);
        queue.close();
        assert_eq!(
            budget.used(),
            allocated,
            "a consumer may still own drained pixels"
        );
        drop(retained);
        assert_eq!(budget.used(), 0);
        let tiny = crate::pixel_memory::PixelBudget::test(1);
        let error = crate::ipc::read_packet_inner(&mut wire.as_slice(), Some(&tiny)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::OutOfMemory);
        assert_eq!(tiny.used(), 0);
    }

    #[test]
    fn failed_reader_revokes_certificate_before_desktop_drains_terminal_event() {
        let manager = RdpSessionManager::new();
        let queue = Arc::new(EventQueue::default());
        let state = Arc::new(Mutex::new(RdpSessionState::Connecting));
        manager.sessions.lock().unwrap().insert(
            "s".into(),
            SessionRecord {
                presentation_worker: None,
                _component: None,
                state: state.clone(),
                capabilities: Arc::new(Mutex::new(None)),
                queue: queue.clone(),
                writer: Arc::new(crate::helper_process::IpcWriter::test_mailbox()),
                child: Arc::new(Mutex::new(None)),
                supervisor: None,
                stderr: None,
                reader: None,
            },
        );
        manager
            .pending_certificates
            .lock()
            .unwrap()
            .insert("certificate".into(), "s".into());
        assert!(manager.certificate_is_pending("s", "certificate"));
        super::push_reader_error(
            "s",
            &queue,
            &state,
            crate::RdpErrorKind::Transport,
            "connection ended".into(),
        );
        assert!(
            manager
                .pending_certificates
                .lock()
                .unwrap()
                .contains_key("certificate")
        );
        // A store job runs this predicate before writing, without waiting for the UI.
        assert!(!manager.certificate_is_pending("s", "certificate"));
        assert!(matches!(
            &queue.drain().control[..],
            [RdpRuntimeEvent::Error { fatal: true, .. }]
        ));
    }

    #[test]
    fn detached_close_removes_resources_before_a_stalled_reader_is_reaped() {
        let manager = RdpSessionManager::new();
        let queue = Arc::new(EventQueue::default());
        queue.push_reset("s", 1, 1, 1);
        queue.push_frame(frame(1, 0, true));
        manager
            .pending_certificates
            .lock()
            .unwrap()
            .insert("old-request".to_string(), "s".to_string());
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let _ = release_rx.recv_timeout(std::time::Duration::from_secs(2));
            finished_tx.send(()).unwrap();
        });
        manager.sessions.lock().unwrap().insert(
            "s".to_string(),
            SessionRecord {
                presentation_worker: None,
                _component: None,
                state: Arc::new(Mutex::new(RdpSessionState::Connected)),
                capabilities: Arc::new(Mutex::new(None)),
                queue: queue.clone(),
                writer: Arc::new(crate::helper_process::IpcWriter::test_mailbox()),
                child: Arc::new(Mutex::new(None)),
                supervisor: None,
                stderr: None,
                reader: Some(reader),
            },
        );
        let start = std::time::Instant::now();
        manager.close_detached("s").unwrap();
        assert!(start.elapsed() < std::time::Duration::from_millis(500));
        assert!(manager.state("s").is_none());
        assert!(manager.pending_certificates.lock().unwrap().is_empty());
        assert!(queue.drain().frames.is_empty());
        manager.sessions.lock().unwrap().insert(
            "s".to_string(),
            SessionRecord {
                presentation_worker: None,
                _component: None,
                state: Arc::new(Mutex::new(RdpSessionState::Connected)),
                capabilities: Arc::new(Mutex::new(None)),
                queue: Arc::new(EventQueue::default()),
                writer: Arc::new(crate::helper_process::IpcWriter::test_mailbox()),
                child: Arc::new(Mutex::new(None)),
                supervisor: None,
                stderr: None,
                reader: None,
            },
        );
        release_tx.send(()).unwrap();
        finished_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while Arc::strong_count(&queue) > 1 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            Arc::strong_count(&queue),
            1,
            "the reaped record must be dropped"
        );
        assert_eq!(manager.state("s"), Some(RdpSessionState::Connected));
    }

    #[test]
    fn redirection_requires_negotiation_and_matching_session() {
        let request = crate::RdpRedirectRequest {
            session_id: "s".into(),
            request_id: "redirect".into(),
            generation: 1,
            hop: 1,
            host: "worker.example".into(),
            connect_hosts: Vec::new(),
            port: 3389,
        };
        let queue = Arc::new(EventQueue::default());
        let state = Arc::new(Mutex::new(RdpSessionState::Connecting));
        let capabilities = Arc::new(Mutex::new(Some(RdpServerCapabilities::default())));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        assert!(
            handle_control(
                "s",
                RdpControlMessage::RedirectRequest(request.clone()),
                &queue,
                &state,
                &capabilities,
                &pending,
                &mut true
            )
            .is_err()
        );
        capabilities
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .server_redirection = true;
        assert!(
            handle_control(
                "another-session",
                RdpControlMessage::RedirectRequest(request.clone()),
                &queue,
                &state,
                &capabilities,
                &pending,
                &mut true
            )
            .is_err()
        );
        handle_control(
            "s",
            RdpControlMessage::RedirectRequest(request.clone()),
            &queue,
            &state,
            &capabilities,
            &pending,
            &mut true,
        )
        .unwrap();
        assert!(
            matches!(&queue.drain().control[..], [crate::RdpRuntimeEvent::RedirectRequest(received)] if received == &request)
        );
    }

    #[test]
    fn legacy_relay_redirection_failure_explains_manual_component_update() {
        let queue = Arc::new(EventQueue::default());
        let state = Arc::new(Mutex::new(RdpSessionState::Connecting));
        let capabilities = Arc::new(Mutex::new(Some(RdpServerCapabilities::default())));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        handle_control(
            "s",
            RdpControlMessage::Error {
                session_id: "s".into(),
                error: crate::RdpError::new(
                    crate::RdpErrorKind::Transport,
                    "server redirection is not supported over a loopback relay",
                ),
                fatal: false,
            },
            &queue,
            &state,
            &capabilities,
            &pending,
            &mut true,
        )
        .unwrap();
        assert!(
            matches!(&queue.drain().control[..], [crate::RdpRuntimeEvent::Error { error, .. }] if error.message.contains("update the RDP native component in Plugins"))
        );
    }

    #[test]
    fn server_hello_must_be_first_and_records_capabilities_only_once() {
        let queue = Arc::new(EventQueue::default());
        let state = Arc::new(Mutex::new(RdpSessionState::Connecting));
        let capabilities = Arc::new(Mutex::new(None));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let advertised = RdpServerCapabilities {
            managed_clipboard: true,
            committed_unicode_text: true,
            secure_attention: true,
            frame_update_complete: false,
            server_redirection: false,
        };
        let mut hello_received = false;

        let error = handle_control(
            "s",
            RdpControlMessage::State {
                session_id: "s".to_string(),
                state: RdpSessionState::Connecting,
                message: None,
            },
            &queue,
            &state,
            &capabilities,
            &pending,
            &mut hello_received,
        )
        .expect_err("state before ServerHello must fail");
        assert_eq!(error.kind, crate::RdpErrorKind::Protocol);
        assert!(!hello_received);
        assert_eq!(*capabilities.lock().unwrap(), None);

        let error = handle_control(
            "s",
            RdpControlMessage::ServerHello {
                version: PROTOCOL_VERSION - 1,
                capabilities: advertised,
            },
            &queue,
            &state,
            &capabilities,
            &pending,
            &mut hello_received,
        )
        .expect_err("a mismatched version must fail");
        assert_eq!(error.kind, crate::RdpErrorKind::Protocol);
        assert!(!hello_received);
        assert_eq!(*capabilities.lock().unwrap(), None);

        handle_control(
            "s",
            RdpControlMessage::ServerHello {
                version: PROTOCOL_VERSION,
                capabilities: advertised,
            },
            &queue,
            &state,
            &capabilities,
            &pending,
            &mut hello_received,
        )
        .expect("matching hello");
        assert!(hello_received);
        assert_eq!(*capabilities.lock().unwrap(), Some(advertised));

        let error = handle_control(
            "s",
            RdpControlMessage::ServerHello {
                version: PROTOCOL_VERSION,
                capabilities: RdpServerCapabilities::default(),
            },
            &queue,
            &state,
            &capabilities,
            &pending,
            &mut hello_received,
        )
        .expect_err("duplicate ServerHello must fail");
        assert_eq!(error.kind, crate::RdpErrorKind::Protocol);
        assert_eq!(*capabilities.lock().unwrap(), Some(advertised));

        let error = handle_control(
            "s",
            RdpControlMessage::ClientHello {
                managed_clipboard: true,
                version: PROTOCOL_VERSION,
                frame_update_complete: false,
                server_redirection: false,
            },
            &queue,
            &state,
            &capabilities,
            &pending,
            &mut hello_received,
        )
        .expect_err("application-only messages from the helper must fail");
        assert_eq!(error.kind, crate::RdpErrorKind::Protocol);
        assert!(require_server_hello(false, "frame packet").is_err());
        assert!(require_server_hello(false, "cursor packet").is_err());
        assert!(require_server_hello(true, "frame packet").is_ok());
    }

    #[test]
    fn rdp_committed_text_batch_is_fully_validated() {
        let events = vec![
            RdpInputEvent::Unicode {
                text: "valid text".to_string(),
            },
            RdpInputEvent::Unicode {
                text: "invalid\ntext".to_string(),
            },
        ];
        assert!(validate_rdp_input(&events).is_err());
        assert!(!validate_rdp_input(&[RdpInputEvent::ReleaseAllInputs]).unwrap());
    }

    #[test]
    fn dynamic_resize_unavailable_remains_a_runtime_capability_event() {
        let queue = Arc::new(EventQueue::default());
        let state = Arc::new(Mutex::new(RdpSessionState::Connected));
        let capabilities = Arc::new(Mutex::new(Some(RdpServerCapabilities {
            managed_clipboard: true,
            committed_unicode_text: true,
            secure_attention: true,
            frame_update_complete: false,
            server_redirection: false,
        })));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let mut hello_received = true;

        handle_control(
            "s",
            RdpControlMessage::Capability {
                session_id: "s".to_string(),
                capability: RdpCapability::DynamicResizeUnavailable,
            },
            &queue,
            &state,
            &capabilities,
            &pending,
            &mut hello_received,
        )
        .expect("runtime capability event");

        assert_eq!(
            *capabilities.lock().unwrap(),
            Some(RdpServerCapabilities {
                managed_clipboard: true,
                committed_unicode_text: true,
                secure_attention: true,
                frame_update_complete: false,
                server_redirection: false,
            })
        );
        assert!(matches!(
            queue.drain().control.first(),
            Some(RdpRuntimeEvent::Capability {
                capability: RdpCapability::DynamicResizeUnavailable,
                ..
            })
        ));
    }

    #[test]
    fn clipboard_transfer_progress_is_delivered_as_typed_runtime_event() {
        let queue = Arc::new(EventQueue::default());
        let state = Arc::new(Mutex::new(RdpSessionState::Connected));
        let capabilities = Arc::new(Mutex::new(None));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let mut hello_received = true;
        let progress = RdpClipboardTransferProgress {
            id: "transfer-1".to_string(),
            name: "folder".to_string(),
            status: RdpClipboardTransferStatus::Running,
            total_bytes: 10,
            transferred_bytes: 4,
            total_files: 1,
            completed_files: 0,
            error: None,
        };
        handle_control(
            "s",
            RdpControlMessage::ClipboardTransfer {
                session_id: "s".to_string(),
                progress: progress.clone(),
            },
            &queue,
            &state,
            &capabilities,
            &pending,
            &mut hello_received,
        )
        .unwrap();
        assert!(matches!(queue.drain().control.as_slice(),
            [RdpRuntimeEvent::ClipboardTransfer { progress: delivered, .. }] if delivered == &progress));
    }

    fn frame(epoch: u64, x: u32, full: bool) -> RdpFrameEvent {
        RdpFrameEvent::Bitmap {
            epoch,
            full,
            x,
            y: 0,
            width: 1,
            height: 1,
            stride: 4,
            format: PixelFormat::Bgra8,
            pixels: vec![x as u8; 4].into(),
        }
    }

    #[test]
    fn cursor_components_are_coalesced_independently() {
        let queue = EventQueue::default();
        queue.push_reset("s", 1, 4, 4);
        queue.push_frame(frame(1, 0, true));
        for x in [1, 2, 3] {
            queue.push_cursor(RemoteCursorEvent::Position(CursorPosition { x, y: 0 }));
        }
        let drain = queue.drain();
        let cursors: Vec<_> = drain
            .cursors
            .iter()
            .filter_map(|event| match event {
                RemoteCursorEvent::Position(cursor) => Some(cursor.x),
                _ => None,
            })
            .collect();
        assert_eq!(cursors, vec![3]);
    }

    #[test]
    fn stale_epochs_are_ignored_without_waiting_for_full() {
        let queue = EventQueue::default();
        queue.push_reset("s", 2, 4, 4);
        assert!(!queue.push_frame(frame(1, 0, true)));
        assert!(queue.push_frame(frame(2, 0, false)));
        let drain = queue.drain();
        assert_eq!(drain.frames.len(), 1);
        assert!(matches!(
            drain.frames[0],
            RdpFrameEvent::Bitmap {
                epoch: 2,
                full: false,
                ..
            }
        ));
    }

    #[test]
    fn byte_budget_applies_backpressure_until_drain() {
        let queue = Arc::new(EventQueue::with_limits(2, 8));
        queue.push_reset("s", 1, 4, 4);
        assert!(queue.push_frame(frame(1, 0, false)));
        assert!(queue.push_frame(frame(1, 1, false)));
        let producer_queue = Arc::clone(&queue);
        let producer = std::thread::spawn(move || producer_queue.push_frame(frame(1, 2, false)));
        std::thread::yield_now();
        assert!(!producer.is_finished());
        assert_eq!(queue.drain().frames.len(), 2);
        assert!(producer.join().unwrap());
        assert_eq!(queue.drain().frames.len(), 1);
    }

    #[test]
    fn close_unblocks_a_backpressured_producer() {
        let queue = Arc::new(EventQueue::with_limits(1, 4));
        queue.push_reset("s", 1, 4, 4);
        assert!(queue.push_frame(frame(1, 0, false)));
        let producer_queue = Arc::clone(&queue);
        let producer = std::thread::spawn(move || producer_queue.push_frame(frame(1, 1, false)));
        std::thread::yield_now();
        queue.close();
        assert!(!producer.join().unwrap());
        let drain = queue.drain();
        assert!(drain.control.is_empty());
        assert!(drain.frames.is_empty());
        assert!(drain.cursors.is_empty());
        let state = queue.state.lock().unwrap();
        assert_eq!(state.frame_bytes, 0);
        assert_eq!(state.control_bytes, 0);
    }

    #[test]
    fn bounded_drain_preserves_delta_order_and_accounting() {
        let queue = EventQueue::default();
        queue.push_reset("s", 1, 32, 1);
        for x in 0..20 {
            assert!(queue.push_frame(frame(1, x, false)));
        }
        let first = queue.drain_with_limit(true);
        assert_eq!(first.frames.len(), 8);
        assert_eq!(queue.state.lock().unwrap().frame_bytes, 12 * 4);
        let second = queue.drain_with_limit(true);
        assert_eq!(second.frames.len(), 8);
        assert!(matches!(
            second.frames[0],
            RdpFrameEvent::Bitmap { x: 8, .. }
        ));
        assert_eq!(queue.drain_with_limit(true).frames.len(), 4);
        assert_eq!(queue.state.lock().unwrap().frame_bytes, 0);
    }

    #[test]
    fn a_closed_reader_cannot_register_a_certificate_prompt() {
        let queue = Arc::new(EventQueue::default());
        queue.close();
        let pending = Arc::new(Mutex::new(HashMap::new()));
        handle_control(
            "s",
            RdpControlMessage::CertificateRequest(crate::RdpCertificateRequest {
                request_id: "stale".to_string(),
                host: "host".to_string(),
                port: 3389,
                sha256_fingerprint: "fingerprint".to_string(),
                subject: None,
                issuer: None,
                valid_from: None,
                valid_to: None,
            }),
            &queue,
            &Arc::new(Mutex::new(RdpSessionState::Disconnecting)),
            &Arc::new(Mutex::new(None)),
            &pending,
            &mut true,
        )
        .unwrap();
        assert!(pending.lock().unwrap().is_empty());
        assert!(queue.drain().control.is_empty());
    }

    #[test]
    fn every_producer_path_wakes_the_consumer() {
        let signals = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&signals);
        let queue = EventQueue::with_waker(Some(Arc::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        })));
        queue.push_reset("s", 1, 4, 4);
        queue.push_frame(frame(1, 0, true));
        queue.push_control(RdpRuntimeEvent::Clipboard {
            session_id: "s".to_string(),
            text: String::new(),
            generation: 0,
        });
        assert_eq!(signals.load(Ordering::Relaxed), 3);
    }
}
