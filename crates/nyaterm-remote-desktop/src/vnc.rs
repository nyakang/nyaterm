use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Child;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use uuid::Uuid;

use crate::heartbeat::{Heartbeat, Supervisor};
use crate::helper_process;
use crate::{
    FRAME_PAYLOAD_LIMIT, MAX_VNC_CLIPBOARD_TEXT_BYTES, MAX_VNC_INPUT_BATCH, PROTOCOL_VERSION,
    PacketType, QueueWaker, RemoteCursorEvent, RemoteFrameEvent, RemotePointerEvent,
    VncControlMessage, VncError, VncErrorKind, VncInputEvent, VncRuntimeEvent, VncSecurityMode,
    VncServerCapabilities, VncSessionConfig, VncSessionDrain, VncSessionState,
    decode_cursor_packet_owned, decode_frame_packet_owned, decode_vnc_control, encode_vnc_control,
    validate_committed_text,
};
use nyaterm_core::runtime::reaper::Reaper;

const FRAME_QUEUE_LIMIT: usize = 64;
const FRAME_QUEUE_BYTE_LIMIT: usize = FRAME_PAYLOAD_LIMIT;
const CONTROL_QUEUE_LIMIT: usize = 256;
const CONTROL_QUEUE_BYTE_LIMIT: usize = 4 * 1024 * 1024;
const HELPER_PACKAGE: &str = "nyaterm-vnc-helper";
const HELPER_ENV_VAR: &str = "NYATERM_VNC_HELPER";

fn resolve_helper_path() -> Result<PathBuf, VncError> {
    helper_process::resolve_helper(HELPER_PACKAGE, HELPER_ENV_VAR)
        .map_err(|message| VncError::new(VncErrorKind::HelperMissing, message))
}

#[derive(Default)]
struct EventQueueState {
    waker: Option<QueueWaker>,
    control: VecDeque<VncRuntimeEvent>,
    control_bytes: usize,
    frames: VecDeque<RemoteFrameEvent>,
    frame_bytes: usize,
    cursor_shape: Option<RemoteCursorEvent>,
    cursor_position: Option<RemoteCursorEvent>,
    cursor_visibility: Option<RemoteCursorEvent>,
    current_epoch: Option<u64>,
    closed: bool,
}

fn frame_byte_cost(frame: &RemoteFrameEvent) -> usize {
    match frame {
        RemoteFrameEvent::Bitmap { pixels, .. } => pixels.len(),
        _ => 0,
    }
}

fn control_byte_cost(event: &VncRuntimeEvent) -> usize {
    match event {
        VncRuntimeEvent::ServerKeyRequest(request)
        | VncRuntimeEvent::ServerKeyAuthenticated(request) => {
            request.session_id.len()
                + request.host.len()
                + request.request_id.len()
                + request.sha256_fingerprint.len()
                + 128
        }
        VncRuntimeEvent::State {
            session_id,
            message,
            ..
        } => session_id.len() + message.as_ref().map_or(0, String::len) + 64,
        VncRuntimeEvent::Frame { session_id, .. } => session_id.len() + 64,
        VncRuntimeEvent::Clipboard { session_id, text } => session_id.len() + text.len() + 64,
        VncRuntimeEvent::Error {
            session_id, error, ..
        } => session_id.len() + error.message.len() + 64,
    }
}

struct EventQueue {
    trust: Arc<crate::vnc_trust::VncTrustState>,
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
            trust: Arc::default(),
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

    fn push_control(&self, event: VncRuntimeEvent) -> bool {
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

    fn push_control_force(&self, event: VncRuntimeEvent) {
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

    fn push_reset(&self, session_id: &str, epoch: u64, width: u32, height: u32) -> bool {
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.closed {
                return false;
            }
            state.current_epoch = Some(epoch);
            state.frames.clear();
            state.frame_bytes = 0;
            state.cursor_shape = None;
            state.cursor_position = None;
            state.cursor_visibility = None;
        }
        self.space_available.notify_all();
        self.push_control(VncRuntimeEvent::Frame {
            session_id: session_id.to_string(),
            event: RemoteFrameEvent::Reset {
                epoch,
                width,
                height,
            },
        })
    }

    fn push_frame(&self, frame: RemoteFrameEvent) -> bool {
        let cost = frame_byte_cost(&frame);
        if cost > self.frame_byte_limit {
            return false;
        }
        let epoch = match &frame {
            RemoteFrameEvent::Bitmap { epoch, .. } => Some(*epoch),
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

    fn push_cursor(&self, cursor: RemoteCursorEvent) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed {
            return false;
        }
        match &cursor {
            RemoteCursorEvent::Shape(_) => state.cursor_shape = Some(cursor),
            RemoteCursorEvent::Position(_) => state.cursor_position = Some(cursor),
            RemoteCursorEvent::Visibility(_) => state.cursor_visibility = Some(cursor),
        }
        Self::wake(&state);
        true
    }

    fn drain(&self) -> VncSessionDrain {
        self.drain_with_limit(false)
    }

    fn drain_with_limit(&self, bounded: bool) -> VncSessionDrain {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let cursors = [
            state.cursor_shape.take(),
            state.cursor_position.take(),
            state.cursor_visibility.take(),
        ]
        .into_iter()
        .flatten()
        .collect();
        let count = if bounded {
            crate::frame::frame_batch_len(&state.frames)
        } else {
            state.frames.len()
        };
        let frames: Vec<_> = state.frames.drain(..count).collect();
        state.frame_bytes = state
            .frame_bytes
            .saturating_sub(frames.iter().map(frame_byte_cost).sum());
        let drain = VncSessionDrain {
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
    state: Arc<Mutex<VncSessionState>>,
    capabilities: Arc<Mutex<Option<VncServerCapabilities>>>,
    queue: Arc<EventQueue>,
    writer: Arc<helper_process::IpcWriter>,
    child: Arc<Mutex<Option<Child>>>,
    supervisor: Option<Supervisor>,
    reader: Option<JoinHandle<()>>,
    stderr: Option<helper_process::StderrCapture>,
}

/// Owns the `nyaterm-vnc-helper` child processes and their event queues.
///
/// The public surface is unchanged from the previous in-process implementation;
/// only the transport moved behind IPC.
#[derive(Default)]
pub struct VncSessionManager {
    sessions: Mutex<HashMap<String, SessionRecord>>,
    reaper: Mutex<Option<Reaper<SessionRecord>>>,
    /// Installed once by the application; copied into every session queue so
    /// the reader thread can wake the consumer instead of being polled.
    queue_waker: Mutex<Option<QueueWaker>>,
}

impl VncSessionManager {
    pub fn new() -> Self {
        Self::default()
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

    fn ensure_reaper(&self) -> Result<(), VncError> {
        let mut slot = self
            .reaper
            .lock()
            .map_err(|_| VncError::new(VncErrorKind::Ipc, "reaper lock poisoned"))?;
        if slot.is_none() {
            *slot = Some(
                Reaper::new("vnc-reaper", 64, |mut record: SessionRecord| {
                    cleanup_child(&mut record)
                })
                .map_err(|error| VncError::new(VncErrorKind::Ipc, error.to_string()))?,
            );
        }
        Ok(())
    }

    pub fn create_session(&self, config: VncSessionConfig) -> Result<String, VncError> {
        self.create_session_with_id(Uuid::new_v4().to_string(), config)
    }

    pub fn create_session_with_id(
        &self,
        session_id: String,
        config: VncSessionConfig,
    ) -> Result<String, VncError> {
        // Validate before spawning so a bad configuration never costs a process.
        validate_vnc_config(&config)?;
        {
            let mut sessions = self.sessions.lock().map_err(|_| registry_poisoned())?;
            if sessions
                .get(&session_id)
                .is_some_and(|record| record.reader.is_some())
            {
                return Err(VncError::new(
                    VncErrorKind::Protocol,
                    format!("VNC session '{session_id}' is already running"),
                ));
            }
            sessions.remove(&session_id);
        }
        self.ensure_reaper()?;
        let helper = resolve_helper_path()?;
        let helper_process::HelperProcess {
            mut child,
            stdin,
            stdout,
            stderr,
        } = helper_process::spawn_helper(&helper, "vnc").map_err(|error| {
            VncError::new(
                VncErrorKind::Ipc,
                format!("failed to spawn the VNC helper: {error}"),
            )
        })?;
        let writer = match helper_process::IpcWriter::spawn(
            stdin,
            format!("nyaterm-vnc-writer-{session_id}"),
        ) {
            Ok(writer) => writer,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                stderr.finish();
                return Err(VncError::new(
                    VncErrorKind::Ipc,
                    format!("failed to start the VNC IPC writer: {error}"),
                ));
            }
        };
        let queue = Arc::new(EventQueue::with_waker(self.queue_waker()));
        let state = Arc::new(Mutex::new(VncSessionState::Connecting));
        let capabilities = Arc::new(Mutex::new(None));
        let mut record = SessionRecord {
            state: state.clone(),
            capabilities: capabilities.clone(),
            queue: queue.clone(),
            writer: Arc::new(writer),
            child: Arc::new(Mutex::new(Some(child))),
            reader: None,
            stderr: Some(stderr),
            supervisor: None,
        };
        record.reader = match spawn_reader(
            session_id.clone(),
            stdout,
            queue.clone(),
            state.clone(),
            capabilities.clone(),
            ReaderStartup {
                writer: Arc::clone(&record.writer),
                config,
            },
        ) {
            Ok(reader) => Some(reader),
            Err(error) => {
                cleanup_child(&mut record);
                return Err(VncError::new(
                    VncErrorKind::Ipc,
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
                let _ = send_control(&ping_writer, &VncControlMessage::Ping { sequence });
            },
            move || {
                push_reader_error(
                    &timeout_id,
                    &timeout_queue,
                    &timeout_state,
                    VncErrorKind::Timeout,
                    "helper handshake/control loop did not respond within 10 seconds; check that the helper matches this application".into(),
                )
            },
        ) {
            Ok(supervisor) => Some(supervisor),
            Err(error) => {
                cleanup_child(&mut record);
                return Err(VncError::new(
                    VncErrorKind::Ipc,
                    format!("failed to start helper supervisor: {error}"),
                ));
            }
        };
        let start_result = send_control(
            &record.writer,
            &VncControlMessage::ClientHello {
                version: PROTOCOL_VERSION,
            },
        );
        if let Err(error) = start_result {
            cleanup_child(&mut record);
            return Err(error);
        }
        record.queue.push_control(VncRuntimeEvent::State {
            session_id: session_id.clone(),
            state: VncSessionState::Connecting,
            message: None,
        });
        let mut sessions = match self.sessions.lock() {
            Ok(sessions) => sessions,
            Err(_) => {
                cleanup_child(&mut record);
                return Err(registry_poisoned());
            }
        };
        if sessions.contains_key(&session_id) {
            drop(sessions);
            cleanup_child(&mut record);
            return Err(VncError::new(
                VncErrorKind::Protocol,
                format!("VNC session '{session_id}' was created concurrently"),
            ));
        }
        sessions.insert(session_id.clone(), record);
        Ok(session_id)
    }

    /// Return the static helper capabilities confirmed by `ServerHello`.
    ///
    /// VNC has no Secure Attention capability or operation by design.
    pub fn trust_state(&self, session_id: &str) -> Option<Arc<crate::vnc_trust::VncTrustState>> {
        Some(
            self.sessions
                .lock()
                .ok()?
                .get(session_id)?
                .queue
                .trust
                .clone(),
        )
    }

    pub fn respond_server_key(
        &self,
        request: &crate::VncServerKeyRequest,
        accept: bool,
        remember: bool,
    ) -> Result<(), VncError> {
        let sessions = self.sessions.lock().map_err(|_| registry_poisoned())?;
        let record = sessions
            .get(&request.session_id)
            .ok_or_else(|| VncError::new(VncErrorKind::Protocol, "VNC session closed"))?;
        if !record.queue.trust.respond(request, accept, remember) {
            return Err(VncError::new(
                VncErrorKind::Protocol,
                "VNC key request expired",
            ));
        }
        send_control(
            &record.writer,
            &VncControlMessage::ServerKeyResponse {
                session_id: request.session_id.clone(),
                generation: request.generation,
                request_id: request.request_id.clone(),
                accept,
            },
        )
    }

    pub fn server_capabilities(&self, session_id: &str) -> Option<VncServerCapabilities> {
        let sessions = self.sessions.lock().ok()?;
        *sessions.get(session_id)?.capabilities.lock().ok()?
    }

    pub fn send_input(&self, session_id: &str, events: Vec<VncInputEvent>) -> Result<(), VncError> {
        if events.len() > MAX_VNC_INPUT_BATCH {
            return Err(VncError::new(
                VncErrorKind::Protocol,
                format!("VNC input batch exceeds {MAX_VNC_INPUT_BATCH} events"),
            ));
        }
        let needs_committed_text = validate_vnc_input(&events)?;
        if needs_committed_text {
            let capabilities = self.confirmed_capabilities(session_id)?;
            if !capabilities.committed_unicode_keysyms {
                return Err(VncError::new(
                    VncErrorKind::Protocol,
                    "VNC helper did not advertise committed Unicode keysym input",
                ));
            }
        }
        let move_only = !events.is_empty()
            && events.iter().all(|event| {
                matches!(
                    event,
                    VncInputEvent::Pointer(RemotePointerEvent::Move { .. })
                )
            });
        let packet = encode_vnc_control(&VncControlMessage::Input {
            session_id: session_id.to_string(),
            events,
        })
        .map_err(|error| VncError::new(VncErrorKind::Ipc, error.to_string()))?;
        let sessions = self.sessions.lock().map_err(|_| registry_poisoned())?;
        let record = sessions.get(session_id).ok_or_else(|| {
            VncError::new(
                VncErrorKind::Protocol,
                format!("VNC session '{session_id}' is not running"),
            )
        })?;
        let result = if move_only {
            record.writer.send_latest_move(packet)
        } else {
            let release = encode_vnc_control(&VncControlMessage::Input {
                session_id: session_id.to_string(),
                events: vec![VncInputEvent::ReleaseAllInputs],
            })
            .map_err(|error| VncError::new(VncErrorKind::Ipc, error.to_string()))?;
            record.writer.send_reliable(packet, release)
        };
        result.map_err(|error| VncError::new(VncErrorKind::Ipc, error.to_string()))
    }

    pub fn set_clipboard_text(&self, session_id: &str, text: String) -> Result<(), VncError> {
        if !is_latin1_within_limit(&text) {
            return Err(VncError::new(
                VncErrorKind::Clipboard,
                "VNC clipboard text must be Latin-1 and no larger than 1 MiB",
            ));
        }
        self.send(
            session_id,
            VncControlMessage::Clipboard {
                session_id: session_id.to_string(),
                text,
            },
        )
    }

    pub fn request_full_frame(&self, session_id: &str) -> Result<(), VncError> {
        self.send(
            session_id,
            VncControlMessage::RequestFullFrame {
                session_id: session_id.to_string(),
            },
        )
    }

    pub fn drain(&self, session_id: &str) -> VncSessionDrain {
        let Ok(sessions) = self.sessions.lock() else {
            return VncSessionDrain::default();
        };
        let Some(record) = sessions.get(session_id) else {
            return VncSessionDrain::default();
        };
        record.queue.drain()
    }

    /// Drain control state and a bounded, ordered prefix of frame deltas.
    pub fn drain_batch(&self, session_id: &str) -> VncSessionDrain {
        let Ok(sessions) = self.sessions.lock() else {
            return VncSessionDrain::default();
        };
        sessions
            .get(session_id)
            .map(|record| record.queue.drain_with_limit(true))
            .unwrap_or_default()
    }

    /// Close a session, keeping its record so [`Self::state`] still answers.
    ///
    /// This matches `RdpSessionManager::close`: the record stays in the map with
    /// `child: None` so a caller can observe `Disconnected` afterwards, and a
    /// later `create_session_with_id` with the same id replaces the stale entry.
    pub fn close(&self, session_id: &str) -> Result<(), VncError> {
        let Some(mut record) = self
            .sessions
            .lock()
            .map_err(|_| registry_poisoned())?
            .remove(session_id)
        else {
            return Ok(());
        };
        record.queue.trust.invalidate(true);
        set_state(&record.state, VncSessionState::Disconnecting);
        let _ = send_control(
            &record.writer,
            &VncControlMessage::Input {
                session_id: session_id.to_string(),
                events: vec![VncInputEvent::ReleaseAllInputs],
            },
        );
        let _ = send_control(
            &record.writer,
            &VncControlMessage::Disconnect {
                session_id: session_id.to_string(),
            },
        );
        cleanup_child(&mut record);
        set_state(&record.state, VncSessionState::Disconnected);
        record.queue.push_control_force(VncRuntimeEvent::State {
            session_id: session_id.to_string(),
            state: VncSessionState::Disconnected,
            message: None,
        });
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(session_id.to_string(), record);
        Ok(())
    }

    /// Remove UI ownership immediately; reap the helper off the UI thread.
    /// The detached record cannot replace a later session with the same id.
    pub fn close_detached(&self, session_id: &str) -> Result<(), VncError> {
        self.ensure_reaper()?;
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| VncError::new(VncErrorKind::Ipc, "session registry lock poisoned"))?;
        let slot = self
            .reaper
            .lock()
            .map_err(|_| VncError::new(VncErrorKind::Ipc, "reaper lock poisoned"))?;
        slot.as_ref()
            .ok_or_else(|| VncError::new(VncErrorKind::Ipc, "reaper unavailable"))?
            .try_submit_with(|| {
                let record = sessions.remove(session_id)?;
                set_state(&record.state, VncSessionState::Disconnecting);
                record.queue.close();
                let _ = send_control(
                    &record.writer,
                    &VncControlMessage::Disconnect {
                        session_id: session_id.to_owned(),
                    },
                );
                Some(record)
            })
            .map_err(|error| VncError::new(VncErrorKind::Ipc, error.to_string()))
    }

    pub fn state(&self, session_id: &str) -> Option<VncSessionState> {
        let sessions = self.sessions.lock().ok()?;
        sessions
            .get(session_id)?
            .state
            .lock()
            .ok()
            .map(|state| *state)
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

    fn confirmed_capabilities(&self, session_id: &str) -> Result<VncServerCapabilities, VncError> {
        let sessions = self.sessions.lock().map_err(|_| registry_poisoned())?;
        let record = sessions.get(session_id).ok_or_else(|| {
            VncError::new(
                VncErrorKind::Protocol,
                format!("VNC session '{session_id}' is not running"),
            )
        })?;
        record
            .capabilities
            .lock()
            .map_err(|_| VncError::new(VncErrorKind::Internal, "VNC capability lock is poisoned"))?
            .as_ref()
            .copied()
            .ok_or_else(|| {
                VncError::new(
                    VncErrorKind::Protocol,
                    "VNC helper capabilities have not been confirmed",
                )
            })
    }

    fn send(&self, session_id: &str, message: VncControlMessage) -> Result<(), VncError> {
        let sessions = self.sessions.lock().map_err(|_| registry_poisoned())?;
        let record = sessions.get(session_id).ok_or_else(|| {
            VncError::new(
                VncErrorKind::Protocol,
                format!("VNC session '{session_id}' is not running"),
            )
        })?;
        send_control(&record.writer, &message)
    }
}

impl Drop for VncSessionManager {
    fn drop(&mut self) {
        // Without this, every helper process outlives the application.
        self.shutdown();
    }
}

fn validate_vnc_input(events: &[VncInputEvent]) -> Result<bool, VncError> {
    let mut has_committed_text = false;
    for event in events {
        if let VncInputEvent::Text { text } = event {
            validate_committed_text(text).map_err(|error| {
                VncError::new(
                    VncErrorKind::Protocol,
                    format!("invalid VNC committed text: {error}"),
                )
            })?;
            has_committed_text = true;
        }
    }
    Ok(has_committed_text)
}

pub fn validate_vnc_config(config: &VncSessionConfig) -> Result<(), VncError> {
    if config.host.trim().is_empty() {
        return Err(VncError::new(
            VncErrorKind::Protocol,
            "VNC host is required",
        ));
    }
    if matches!(config.security.mode, VncSecurityMode::VncAuth) && config.password.is_none() {
        return Err(VncError::new(
            VncErrorKind::Authentication,
            "VNC Authentication requires a password",
        ));
    }
    if config.username.len() > 255 {
        return Err(VncError::new(
            VncErrorKind::Authentication,
            "VNC username exceeds 255 bytes",
        ));
    }
    // Classic VNC auth keys off the first 8 bytes; str::len() is that byte count.
    if let Some(password) = config.password.as_ref()
        && password.len()
            > if config.security.mode == VncSecurityMode::VncAuth {
                8
            } else {
                255
            }
    {
        return Err(VncError::new(
            VncErrorKind::Authentication,
            "VNC password exceeds the selected authentication byte limit",
        ));
    }
    Ok(())
}

fn registry_poisoned() -> VncError {
    VncError::new(
        VncErrorKind::Internal,
        "VNC session registry lock is poisoned",
    )
}

fn send_control(
    writer: &helper_process::IpcWriter,
    message: &VncControlMessage,
) -> Result<(), VncError> {
    let packet = encode_vnc_control(message)
        .map_err(|error| VncError::new(VncErrorKind::Ipc, error.to_string()))?;
    writer
        .send_critical(packet)
        .map_err(|error| VncError::new(VncErrorKind::Ipc, error.to_string()))
}

struct ReaderStartup {
    writer: Arc<helper_process::IpcWriter>,
    config: VncSessionConfig,
}

fn spawn_reader(
    session_id: String,
    mut stdout: std::process::ChildStdout,
    queue: Arc<EventQueue>,
    state: Arc<Mutex<VncSessionState>>,
    capabilities: Arc<Mutex<Option<VncServerCapabilities>>>,
    startup: ReaderStartup,
) -> std::io::Result<JoinHandle<()>> {
    let ReaderStartup { writer, config } = startup;
    nyaterm_core::runtime::worker::spawn(format!("nyaterm-vnc-reader-{session_id}"), move || {
        struct InvalidateOnExit(Arc<crate::vnc_trust::VncTrustState>);
        impl Drop for InvalidateOnExit {
            fn drop(&mut self) {
                self.0.invalidate(true);
            }
        }
        let _trust_lifetime = InvalidateOnExit(queue.trust.clone());
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
                            VncErrorKind::ResourceExhausted
                        } else {
                            VncErrorKind::Ipc
                        },
                        error.to_string(),
                    );
                    return;
                }
            };
            let result: Result<(), VncError> = match packet.packet_type {
                PacketType::Control => decode_vnc_control(&packet)
                    .map_err(|error| VncError::new(VncErrorKind::Protocol, error.to_string()))
                    .and_then(|message| {
                        if let VncControlMessage::Pong { sequence } = message {
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
                            &mut hello_received,
                        )
                    }),
                PacketType::Frame => require_server_hello(hello_received, "frame packet")
                    .and_then(|()| {
                        decode_frame_packet_owned(packet).map_err(|error| {
                            VncError::new(VncErrorKind::Protocol, error.to_string())
                        })
                    })
                    .map(|(frame_session, frame)| {
                        if frame_session != session_id {
                            return;
                        }
                        queue.push_frame(frame);
                    }),
                PacketType::Cursor => require_server_hello(hello_received, "cursor packet")
                    .and_then(|()| {
                        decode_cursor_packet_owned(packet).map_err(|error| {
                            VncError::new(VncErrorKind::Protocol, error.to_string())
                        })
                    })
                    .and_then(|(cursor_session, cursor)| {
                        if cursor_session != session_id {
                            return Err(VncError::new(
                                VncErrorKind::Protocol,
                                "VNC helper cursor packet session id does not match",
                            ));
                        }
                        queue.push_cursor(cursor);
                        Ok(())
                    }),
            };
            if result.is_ok() && hello_received {
                queue.heartbeat.hello();
                if let Some(config) = pending_config.take()
                    && let Err(error) = send_control(
                        &writer,
                        &VncControlMessage::Connect {
                            session_id: session_id.clone(),
                            config,
                        },
                    )
                {
                    push_reader_error(&session_id, &queue, &state, error.kind, error.message);
                    return;
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
                VncErrorKind::Protocol,
                format!(
                    "VNC helper ended before IPC {PROTOCOL_VERSION} handshake; rebuild the helper matching this application"
                ),
            );
            return;
        }
        let current = *state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !matches!(
            current,
            VncSessionState::Disconnecting
                | VncSessionState::Disconnected
                | VncSessionState::Failed
        ) {
            // stdout EOF with a live session means the helper died. Reporting it
            // is what turns a silent hang into a retryable failure.
            push_reader_error(
                &session_id,
                &queue,
                &state,
                VncErrorKind::HelperCrashed,
                "the VNC helper exited unexpectedly".to_string(),
            );
        } else {
            // Reap normal EOF without a later heartbeat timeout.
            queue.heartbeat.fail_once();
        }
    })
}

fn require_server_hello(received: bool, packet_kind: &str) -> Result<(), VncError> {
    if received {
        Ok(())
    } else {
        Err(VncError::new(
            VncErrorKind::Protocol,
            format!("VNC helper sent {packet_kind} before ServerHello"),
        ))
    }
}

fn handle_control(
    session_id: &str,
    message: VncControlMessage,
    queue: &Arc<EventQueue>,
    state: &Arc<Mutex<VncSessionState>>,
    capabilities: &Arc<Mutex<Option<VncServerCapabilities>>>,
    hello_received: &mut bool,
) -> Result<(), VncError> {
    if !*hello_received {
        let VncControlMessage::ServerHello {
            version,
            capabilities: confirmed,
        } = message
        else {
            return Err(VncError::new(
                VncErrorKind::Protocol,
                "VNC helper's first control message must be ServerHello",
            ));
        };
        if version != PROTOCOL_VERSION {
            return Err(VncError::new(
                VncErrorKind::Protocol,
                format!(
                    "VNC helper protocol version {version} does not match {PROTOCOL_VERSION}; rebuild the helper matching this application"
                ),
            ));
        }
        let mut slot = capabilities.lock().map_err(|_| {
            VncError::new(VncErrorKind::Internal, "VNC capability lock is poisoned")
        })?;
        if slot.is_some() {
            return Err(VncError::new(
                VncErrorKind::Protocol,
                "VNC helper capabilities were already confirmed",
            ));
        }
        *slot = Some(confirmed);
        *hello_received = true;
        return Ok(());
    }

    match message {
        VncControlMessage::ServerHello { .. } => {
            return Err(VncError::new(
                VncErrorKind::Protocol,
                "VNC helper sent duplicate ServerHello",
            ));
        }
        VncControlMessage::Ping { .. }
        | VncControlMessage::ClientHello { .. }
        | VncControlMessage::ServerKeyResponse { .. }
        | VncControlMessage::Connect { .. }
        | VncControlMessage::Input { .. }
        | VncControlMessage::RequestFullFrame { .. }
        | VncControlMessage::Disconnect { .. } => {
            return Err(VncError::new(
                VncErrorKind::Protocol,
                "VNC helper sent an application-only control message",
            ));
        }
        VncControlMessage::ServerKeyRequest(request) if request.session_id == session_id => {
            if request.host.is_empty()
                || request.host.len() > 1024
                || request.request_id.is_empty()
                || request.request_id.len() > 128
                || !request
                    .sha256_fingerprint
                    .strip_prefix("SHA256:")
                    .is_some_and(|hash| {
                        hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
            {
                return Err(VncError::new(
                    VncErrorKind::Protocol,
                    "Invalid VNC server key challenge",
                ));
            }
            if queue.trust.request(&request) {
                queue.push_control(VncRuntimeEvent::ServerKeyRequest(request));
            }
        }
        VncControlMessage::ServerKeyAuthenticated(request) if request.session_id == session_id => {
            if queue.trust.authenticated(&request) {
                queue.push_control(VncRuntimeEvent::ServerKeyAuthenticated(request));
            }
        }
        VncControlMessage::DesktopReset {
            session_id: event_session,
            epoch,
            width,
            height,
        } if event_session == session_id => {
            queue.push_reset(session_id, epoch, width, height);
        }
        VncControlMessage::State {
            session_id: event_session,
            state: new_state,
            message,
        } if event_session == session_id => {
            if matches!(
                new_state,
                VncSessionState::Connecting
                    | VncSessionState::Reconnecting
                    | VncSessionState::Disconnecting
                    | VncSessionState::Disconnected
                    | VncSessionState::Failed
            ) {
                queue.trust.invalidate(false);
            }
            set_state(state, new_state);
            queue.push_control(VncRuntimeEvent::State {
                session_id: session_id.to_string(),
                state: new_state,
                message,
            });
        }
        VncControlMessage::Clipboard {
            session_id: event_session,
            text,
        } if event_session == session_id => {
            queue.push_control(VncRuntimeEvent::Clipboard {
                session_id: session_id.to_string(),
                text,
            });
        }
        VncControlMessage::Error {
            session_id: event_session,
            error,
            fatal,
        } if event_session == session_id => {
            if fatal {
                push_fatal_error(session_id, queue, state, error);
                return Ok(());
            }
            queue.push_control(VncRuntimeEvent::Error {
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
    state: &Arc<Mutex<VncSessionState>>,
    kind: VncErrorKind,
    message: String,
) {
    push_fatal_error(session_id, queue, state, VncError::new(kind, message));
}

fn push_fatal_error(
    session_id: &str,
    queue: &Arc<EventQueue>,
    state: &Arc<Mutex<VncSessionState>>,
    error: VncError,
) {
    if !queue.heartbeat.fail_once() {
        return;
    }
    queue.close();
    set_state(state, VncSessionState::Failed);
    queue.push_control_force(VncRuntimeEvent::Error {
        session_id: session_id.to_string(),
        error,
        fatal: true,
    });
}

fn set_state(state: &Arc<Mutex<VncSessionState>>, new_state: VncSessionState) {
    *state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = new_state;
}

fn cleanup_child(record: &mut SessionRecord) {
    record.queue.trust.invalidate(true);
    record.queue.close();
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

fn is_latin1_within_limit(text: &str) -> bool {
    text.len() <= MAX_VNC_CLIPBOARD_TEXT_BYTES && text.chars().all(|ch| u32::from(ch) <= 0xff)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::{
        EventQueue, handle_control, is_latin1_within_limit, require_server_hello,
        validate_vnc_config, validate_vnc_input,
    };
    use crate::{
        CursorPosition, CursorShape, CursorVisibility, Framebuffer, MAX_VNC_CLIPBOARD_TEXT_BYTES,
        PROTOCOL_VERSION, PixelFormat, RemoteCursorEvent, RemoteFrameEvent, RemotePoint,
        VncClipboardConfig, VncControlMessage, VncDisplayConfig, VncErrorKind, VncInputEvent,
        VncReconnectConfig, VncRuntimeEvent, VncSecurityConfig, VncSecurityMode,
        VncServerCapabilities, VncSessionConfig, VncSessionState,
    };

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
    fn server_hello_must_be_first_and_records_capabilities_only_once() {
        let queue = Arc::new(EventQueue::default());
        let state = Arc::new(Mutex::new(VncSessionState::Connecting));
        let capabilities = Arc::new(Mutex::new(None));
        let advertised = VncServerCapabilities {
            committed_unicode_keysyms: true,
        };
        let mut hello_received = false;

        let error = handle_control(
            "s",
            VncControlMessage::Error {
                session_id: "s".to_string(),
                error: crate::VncError::new(VncErrorKind::Protocol, "before hello"),
                fatal: true,
            },
            &queue,
            &state,
            &capabilities,
            &mut hello_received,
        )
        .expect_err("error before ServerHello must fail");
        assert_eq!(error.kind, VncErrorKind::Protocol);
        assert!(!hello_received);
        assert_eq!(*capabilities.lock().unwrap(), None);

        let error = handle_control(
            "s",
            VncControlMessage::ServerHello {
                version: PROTOCOL_VERSION - 1,
                capabilities: advertised,
            },
            &queue,
            &state,
            &capabilities,
            &mut hello_received,
        )
        .expect_err("a mismatched version must fail");
        assert_eq!(error.kind, VncErrorKind::Protocol);
        assert!(!hello_received);
        assert_eq!(*capabilities.lock().unwrap(), None);

        handle_control(
            "s",
            VncControlMessage::ServerHello {
                version: PROTOCOL_VERSION,
                capabilities: advertised,
            },
            &queue,
            &state,
            &capabilities,
            &mut hello_received,
        )
        .expect("matching hello");
        assert!(hello_received);
        assert_eq!(*capabilities.lock().unwrap(), Some(advertised));

        let error = handle_control(
            "s",
            VncControlMessage::ServerHello {
                version: PROTOCOL_VERSION,
                capabilities: VncServerCapabilities::default(),
            },
            &queue,
            &state,
            &capabilities,
            &mut hello_received,
        )
        .expect_err("duplicate ServerHello must fail");
        assert_eq!(error.kind, VncErrorKind::Protocol);
        assert_eq!(*capabilities.lock().unwrap(), Some(advertised));

        let error = handle_control(
            "s",
            VncControlMessage::ClientHello {
                version: PROTOCOL_VERSION,
            },
            &queue,
            &state,
            &capabilities,
            &mut hello_received,
        )
        .expect_err("application-only messages from the helper must fail");
        assert_eq!(error.kind, VncErrorKind::Protocol);
        assert!(require_server_hello(false, "frame packet").is_err());
        assert!(require_server_hello(false, "cursor packet").is_err());
        assert!(require_server_hello(true, "frame packet").is_ok());
    }

    #[test]
    fn vnc_committed_text_batch_is_fully_validated() {
        let events = vec![
            VncInputEvent::Text {
                text: "Unicode 文本".to_string(),
            },
            VncInputEvent::Text {
                text: "invalid\u{001b}".to_string(),
            },
        ];
        assert!(validate_vnc_input(&events).is_err());
        assert!(!validate_vnc_input(&[VncInputEvent::ReleaseAllInputs]).unwrap());
    }

    #[test]
    fn every_producer_path_wakes_the_consumer() {
        let signals = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&signals);
        let queue = EventQueue::with_waker(Some(Arc::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        })));

        queue.push_reset("s", 1, 4, 4);
        assert_eq!(signals.load(Ordering::Relaxed), 1, "a reset must wake");
        queue.push_frame(frame(1, 0, true));
        assert_eq!(signals.load(Ordering::Relaxed), 2, "a frame must wake");
        queue.push_control(VncRuntimeEvent::Clipboard {
            session_id: "s".to_string(),
            text: String::new(),
        });
        assert_eq!(
            signals.load(Ordering::Relaxed),
            3,
            "a control event must wake"
        );
    }

    fn config() -> VncSessionConfig {
        VncSessionConfig {
            username: String::new(),
            relay: None,
            name: "vnc".to_string(),
            host: "127.0.0.1".to_string(),
            port: 5900,
            password: None,
            security: VncSecurityConfig::default(),
            display: VncDisplayConfig::default(),
            clipboard: VncClipboardConfig::default(),
            reconnect: VncReconnectConfig::default(),
            shared: true,
            view_only: false,
        }
    }

    fn frame(epoch: u64, x: u32, full: bool) -> RemoteFrameEvent {
        RemoteFrameEvent::Bitmap {
            epoch,
            full,
            x,
            y: 0,
            width: 1,
            height: 1,
            stride: 4,
            format: PixelFormat::Rgba8,
            pixels: vec![x as u8; 4].into(),
        }
    }

    #[test]
    fn validates_password_length_for_classic_vnc_auth() {
        let mut config = config();
        config.security.mode = VncSecurityMode::VncAuth;
        config.password = Some("123456789".to_string().into());
        let error = validate_vnc_config(&config).expect_err("long password should fail");
        assert_eq!(error.kind, VncErrorKind::Authentication);
    }

    #[test]
    fn validates_clipboard_latin1_limit() {
        assert!(is_latin1_within_limit("hello"));
        assert!(!is_latin1_within_limit("hello \u{0100}"));
        assert!(!is_latin1_within_limit(
            &"a".repeat(MAX_VNC_CLIPBOARD_TEXT_BYTES + 1)
        ));
    }

    #[test]
    fn rgba_vnc_frame_reaches_shared_framebuffer_as_bgra() {
        let mut framebuffer =
            Framebuffer::new(1, 1, 1, crate::VNC_FRAMEBUFFER_LIMITS).expect("framebuffer");
        let frame = RemoteFrameEvent::Bitmap {
            epoch: 1,
            full: true,
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            stride: 4,
            format: PixelFormat::Rgba8,
            pixels: vec![10, 20, 30, 255].into(),
        };
        framebuffer.apply(&frame).expect("apply");
        assert_eq!(framebuffer.pixels(), &[30, 20, 10, 255]);
    }

    #[test]
    fn frames_from_a_superseded_epoch_are_dropped() {
        let queue = EventQueue::default();
        queue.push_reset("s", 2, 4, 4);
        assert!(!queue.push_frame(frame(1, 0, true)));
        assert!(queue.push_frame(frame(2, 0, true)));
        let drain = queue.drain();
        assert_eq!(drain.frames.len(), 1);
    }

    #[test]
    fn partial_frames_are_kept_after_a_reset() {
        // A VNC frame is flagged `full` only by starting at the origin, so interior
        // rectangles must still paint instead of being withheld.
        let queue = EventQueue::default();
        queue.push_reset("s", 1, 100, 100);
        assert!(queue.push_frame(frame(1, 40, false)));
        let drain = queue.drain();
        assert_eq!(drain.frames.len(), 1);
    }

    #[test]
    fn cursor_components_are_coalesced_and_cleared_by_drain_and_reset() {
        let queue = EventQueue::default();
        let shape = |shape_id| {
            RemoteCursorEvent::Shape(CursorShape {
                shape_id,
                width: 1,
                height: 1,
                hotspot: RemotePoint { x: 0, y: 0 },
                pixels: vec![shape_id as u8; 4].into(),
            })
        };
        assert!(queue.push_cursor(shape(1)));
        assert!(queue.push_cursor(shape(2)));
        assert!(queue.push_cursor(RemoteCursorEvent::Position(CursorPosition { x: 9, y: 7 })));
        assert!(
            queue.push_cursor(RemoteCursorEvent::Visibility(CursorVisibility {
                visible: false
            },))
        );

        let drain = queue.drain();
        assert_eq!(
            drain.cursors,
            vec![
                shape(2),
                RemoteCursorEvent::Position(CursorPosition { x: 9, y: 7 }),
                RemoteCursorEvent::Visibility(CursorVisibility { visible: false }),
            ]
        );
        assert!(queue.drain().cursors.is_empty());

        assert!(queue.push_cursor(shape(3)));
        assert!(queue.push_reset("s", 2, 4, 4));
        assert!(queue.drain().cursors.is_empty());
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
        assert_eq!(queue.state.lock().unwrap().frame_bytes, 0);
    }

    #[test]
    fn bounded_drain_preserves_delta_order_and_accounting() {
        let queue = EventQueue::default();
        queue.push_reset("s", 1, 32, 1);
        for x in 0..20 {
            assert!(queue.push_frame(frame(1, x, false)));
        }
        assert_eq!(queue.drain_with_limit(true).frames.len(), 8);
        assert_eq!(queue.state.lock().unwrap().frame_bytes, 12 * 4);
        let second = queue.drain_with_limit(true);
        assert!(matches!(
            second.frames[0],
            RemoteFrameEvent::Bitmap { x: 8, .. }
        ));
        assert_eq!(queue.drain_with_limit(true).frames.len(), 4);
        assert_eq!(queue.state.lock().unwrap().frame_bytes, 0);
    }
}
