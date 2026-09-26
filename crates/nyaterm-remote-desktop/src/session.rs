use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Child;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use uuid::Uuid;

use crate::helper_process;
use crate::{
    CursorPosition, CursorShape, CursorVisibility, FRAME_PAYLOAD_LIMIT, PROTOCOL_VERSION,
    PacketType, QueueWaker, RdpCertificateResponse, RdpControlMessage, RdpDisplayMetrics, RdpError,
    RdpErrorKind, RdpFrameEvent, RdpInputEvent, RdpRuntimeEvent, RdpServerCapabilities,
    RdpSessionConfig, RdpSessionDrain, RdpSessionState, RemoteCursorEvent, RemotePointerEvent,
    decode_control, decode_cursor_packet_owned, decode_frame_packet_owned, encode_control,
    read_packet, validate_committed_text,
};

const MIN_WIDTH: u32 = 200;
const MIN_HEIGHT: u32 = 200;
const MAX_WIDTH: u32 = 8192;
const MAX_HEIGHT: u32 = 8192;
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
    state: Mutex<EventQueueState>,
    space_available: Condvar,
    frame_item_limit: usize,
    frame_byte_limit: usize,
}

impl Default for EventQueue {
    fn default() -> Self {
        Self::with_limits(FRAME_QUEUE_LIMIT, FRAME_QUEUE_BYTE_LIMIT)
    }
}

impl EventQueue {
    fn with_limits(frame_item_limit: usize, frame_byte_limit: usize) -> Self {
        Self {
            state: Mutex::new(EventQueueState::default()),
            space_available: Condvar::new(),
            frame_item_limit,
            frame_byte_limit,
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
            state = self
                .space_available
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        self.push_control(RdpRuntimeEvent::Frame {
            session_id: session_id.to_string(),
            event: RdpFrameEvent::Reset {
                epoch,
                width,
                height,
            },
        })
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
            state = self
                .space_available
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let frames: Vec<RdpFrameEvent> = state.frames.drain(..).collect();
        state.frame_bytes = 0;
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
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.closed = true;
        drop(state);
        self.space_available.notify_all();
    }
}

struct SessionRecord {
    state: Arc<Mutex<RdpSessionState>>,
    capabilities: Arc<Mutex<Option<RdpServerCapabilities>>>,
    queue: Arc<EventQueue>,
    writer: helper_process::IpcWriter,
    child: Option<Child>,
    reader: Option<JoinHandle<()>>,
}

#[derive(Default)]
pub struct RdpSessionManager {
    sessions: Mutex<HashMap<String, SessionRecord>>,
    pending_certificates: Arc<Mutex<HashMap<String, String>>>,
    /// Installed once by the application; copied into every session queue so
    /// the reader thread can wake the consumer instead of being polled.
    queue_waker: Mutex<Option<QueueWaker>>,
}

impl RdpSessionManager {
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

    pub fn create_session(&self, config: RdpSessionConfig) -> Result<String, RdpError> {
        self.create_session_with_id(Uuid::new_v4().to_string(), config)
    }

    pub fn create_session_with_id(
        &self,
        session_id: String,
        config: RdpSessionConfig,
    ) -> Result<String, RdpError> {
        validate_config(&config)?;
        {
            let mut sessions = self.sessions.lock().map_err(|_| {
                RdpError::new(RdpErrorKind::Ipc, "RDP session registry lock is poisoned")
            })?;
            if sessions
                .get(&session_id)
                .is_some_and(|record| record.child.is_some())
            {
                return Err(RdpError::new(
                    RdpErrorKind::Protocol,
                    format!("RDP session '{session_id}' is already running"),
                ));
            }
            sessions.remove(&session_id);
        }
        let helper = resolve_helper_path()?;
        let helper_process::HelperProcess {
            mut child,
            stdin,
            stdout,
        } = helper_process::spawn_helper(&helper).map_err(|error| {
            RdpError::new(
                RdpErrorKind::HelperMissing,
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
                return Err(RdpError::new(
                    RdpErrorKind::Ipc,
                    format!("failed to start the RDP IPC writer: {error}"),
                ));
            }
        };
        let queue = Arc::new(EventQueue::with_waker(self.queue_waker()));
        let state = Arc::new(Mutex::new(RdpSessionState::Connecting));
        let capabilities = Arc::new(Mutex::new(None));
        let reader = spawn_reader(
            session_id.clone(),
            stdout,
            queue.clone(),
            state.clone(),
            capabilities.clone(),
            self.pending_certificates.clone(),
        );
        let mut record = SessionRecord {
            state,
            capabilities,
            queue,
            writer,
            child: Some(child),
            reader: Some(reader),
        };
        let start_result = send_control(
            &record.writer,
            &RdpControlMessage::ClientHello {
                version: PROTOCOL_VERSION,
            },
        )
        .and_then(|()| {
            send_control(
                &record.writer,
                &RdpControlMessage::Connect {
                    session_id: session_id.clone(),
                    config,
                },
            )
        });
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

    pub fn shutdown(&self) {
        let ids = self
            .sessions
            .lock()
            .map(|sessions| {
                sessions
                    .iter()
                    .filter_map(|(id, record)| record.child.is_some().then_some(id.clone()))
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

fn spawn_reader(
    session_id: String,
    mut stdout: std::process::ChildStdout,
    queue: Arc<EventQueue>,
    state: Arc<Mutex<RdpSessionState>>,
    capabilities: Arc<Mutex<Option<RdpServerCapabilities>>>,
    pending_certificates: Arc<Mutex<HashMap<String, String>>>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name(format!("nyaterm-rdp-reader-{session_id}"))
        .spawn(move || {
            let mut hello_received = false;
            loop {
                let packet = match read_packet(&mut stdout) {
                    Ok(Some(packet)) => packet,
                    Ok(None) => break,
                    Err(error) => {
                        push_reader_error(
                            &session_id,
                            &queue,
                            &state,
                            RdpErrorKind::Ipc,
                            error.to_string(),
                        );
                        return;
                    }
                };
                let result: Result<(), RdpError> = match packet.packet_type {
                    PacketType::Control => decode_control(&packet)
                        .map_err(|error| RdpError::new(RdpErrorKind::Protocol, error.to_string()))
                        .and_then(|message| {
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
                        .map(|(frame_session, frame)| {
                            if frame_session != session_id {
                                return;
                            }
                            queue.push_frame(frame);
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
                if let Err(error) = result {
                    push_reader_error(&session_id, &queue, &state, error.kind, error.message);
                    return;
                }
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
            }
        })
        .expect("failed to spawn RDP helper reader")
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
                format!("RDP helper protocol version {version} does not match {PROTOCOL_VERSION}"),
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
        RdpControlMessage::ClientHello { .. }
        | RdpControlMessage::Connect { .. }
        | RdpControlMessage::Input { .. }
        | RdpControlMessage::SecureAttention { .. }
        | RdpControlMessage::Resize { .. }
        | RdpControlMessage::ClipboardTransferCancel { .. }
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
        RdpControlMessage::State {
            session_id: event_session,
            state: new_state,
            message,
        } if event_session == session_id => {
            set_state(state, new_state.clone());
            queue.push_control(RdpRuntimeEvent::State {
                session_id: session_id.to_string(),
                state: new_state,
                message,
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
        RdpControlMessage::CertificateRequest(request) => {
            pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(request.request_id.clone(), session_id.to_string());
            queue.push_control(RdpRuntimeEvent::CertificateRequest(request));
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
            if fatal {
                set_state(state, RdpSessionState::Failed(error.clone()));
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
    let error = RdpError::new(kind, message);
    set_state(state, RdpSessionState::Failed(error.clone()));
    queue.push_control(RdpRuntimeEvent::Error {
        session_id: session_id.to_string(),
        error: error.clone(),
        fatal: true,
    });
    queue.push_control(RdpRuntimeEvent::State {
        session_id: session_id.to_string(),
        state: RdpSessionState::Failed(error),
        message: None,
    });
}

fn set_state(state: &Arc<Mutex<RdpSessionState>>, new_state: RdpSessionState) {
    *state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = new_state;
}

fn cleanup_child(record: &mut SessionRecord) {
    record.queue.close();
    helper_process::cleanup_child(&mut record.child, &mut record.reader);
    record.writer.shutdown();
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
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::{EventQueue, handle_control, require_server_hello, validate_rdp_input};
    use crate::{
        CursorPosition, PROTOCOL_VERSION, PixelFormat, RdpCapability, RdpClipboardTransferProgress,
        RdpClipboardTransferStatus, RdpControlMessage, RdpFrameEvent, RdpInputEvent,
        RdpRuntimeEvent, RdpServerCapabilities, RdpSessionState, RemoteCursorEvent,
    };

    #[test]
    fn server_hello_must_be_first_and_records_capabilities_only_once() {
        let queue = Arc::new(EventQueue::default());
        let state = Arc::new(Mutex::new(RdpSessionState::Connecting));
        let capabilities = Arc::new(Mutex::new(None));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let advertised = RdpServerCapabilities {
            committed_unicode_text: true,
            secure_attention: true,
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
                version: PROTOCOL_VERSION,
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
            committed_unicode_text: true,
            secure_attention: true,
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
                committed_unicode_text: true,
                secure_attention: true,
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
            pixels: vec![x as u8; 4],
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
