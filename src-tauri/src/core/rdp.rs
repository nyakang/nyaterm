use crate::config::{self, ConnectionAuth, ConnectionNetwork, ConnectionType};
use crate::core::network::{BoxedTransportStream, open_tcp_transport};
use crate::core::remote_desktop::frame::{
    RemoteDesktopFramePatch, RemoteDesktopPixelFormat, encode_frame_patch,
};
use crate::error::{AppError, AppResult};
use async_trait::async_trait;
use base64::Engine as _;
use ironrdp::client::config::DirectTransport as IronRdpDirectTransport;
use ironrdp::client::config::{
    ClipboardType as IronRdpClipboardType, Config as IronRdpConfig,
    ConfigBuilder as IronRdpConfigBuilder, Destination as IronRdpDestination,
    ServerCertificateVerifier, ServerCertificateVerifyFuture,
    TransportKind as IronRdpTransportKind,
};
use ironrdp::client::rdp::{
    RdpClient as IronRdpClient, RdpInputEvent as IronRdpInputEvent, RdpOutputEvent,
};
use ironrdp::cliprdr::backend::{
    ClipboardMessage, ClipboardMessageProxy, CliprdrBackend, CliprdrBackendFactory,
};
use ironrdp::cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardGeneralCapabilityFlags, FileContentsRequest,
    FileContentsResponse, FormatDataRequest, FormatDataResponse, LockDataId,
    OwnedFormatDataResponse,
};
use ironrdp::core::impl_as_any;
use ironrdp::input::{
    Database as IronRdpInputDatabase, MouseButton as IronRdpMouseButton,
    MousePosition as IronRdpMousePosition, Operation as IronRdpInputOperation,
    Scancode as IronRdpScancode, WheelRotations as IronRdpWheelRotations,
};
use ironrdp::pdu::input::fast_path::{
    FastPathInputEvent as IronRdpFastPathInputEvent, KeyboardFlags as IronRdpKeyboardFlags,
};
use ironrdp::pdu::rdp::capability_sets::MajorPlatformType;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::thread::JoinHandle;
use std::{fmt, io};
use tauri::async_runtime::JoinHandle as TauriJoinHandle;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::time::{Duration, sleep};
use x509_cert::der::Decode as _;

const MAX_FRAME_QUEUE: usize = 2;
const MAX_CLIPBOARD_TEXT_BYTES: usize = 16 * 1024 * 1024;
const CLIPBOARD_POLL_INTERVAL: Duration = Duration::from_millis(750);
const CLIPBOARD_TIMEOUT: Duration = Duration::from_millis(1000);
const CERTIFICATE_PROMPT_TIMEOUT: Duration = Duration::from_secs(120);
const RDP_MIN_WIDTH: u32 = 640;
const RDP_MIN_HEIGHT: u32 = 480;
const RDP_MAX_WIDTH: u32 = 7680;
const RDP_MAX_HEIGHT: u32 = 4320;
const RDP_RIGHT_SHIFT_SCAN_CODE: u16 = 0x36;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum RdpSessionState {
    Connecting,
    CertificateVerification,
    Authenticating,
    Negotiating,
    Active,
    Reconnecting,
    Disconnected,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RdpErrorKind {
    Transport,
    Tls,
    Certificate,
    Authentication,
    Negotiation,
    Session,
    Clipboard,
    Internal,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RdpStateEvent {
    pub session_id: String,
    pub state: RdpSessionState,
    pub message: Option<String>,
    pub error_kind: Option<RdpErrorKind>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RdpCertificateVerifyEvent {
    pub request_id: String,
    pub session_id: String,
    pub host: String,
    pub port: u16,
    pub fingerprint: String,
    pub subject: Option<String>,
    pub issuer: Option<String>,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub known_host_status: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum RdpPointerEvent {
    #[serde(rename = "default")]
    Default { session_id: String },
    #[serde(rename = "hidden")]
    Hidden { session_id: String },
    #[serde(rename = "position")]
    Position { session_id: String, x: u16, y: u16 },
    #[serde(rename = "bitmap")]
    Bitmap {
        session_id: String,
        width: u16,
        height: u16,
        hotspot_x: u16,
        hotspot_y: u16,
        rgba_base64: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
#[allow(dead_code)]
pub enum RdpInputEvent {
    #[serde(rename = "key-down")]
    KeyDown {
        #[serde(alias = "scanCode")]
        scan_code: u16,
        extended: bool,
        repeat: bool,
    },
    #[serde(rename = "key-up")]
    KeyUp {
        #[serde(alias = "scanCode")]
        scan_code: u16,
        extended: bool,
        repeat: bool,
    },
    #[serde(rename = "mouse-move")]
    MouseMove { x: u32, y: u32 },
    #[serde(rename = "mouse-button")]
    MouseButton {
        button: String,
        pressed: bool,
        x: u32,
        y: u32,
    },
    #[serde(rename = "mouse-wheel")]
    MouseWheel {
        #[serde(alias = "deltaX")]
        delta_x: f64,
        #[serde(alias = "deltaY")]
        delta_y: f64,
        x: u32,
        y: u32,
    },
    #[serde(rename = "unicode")]
    Unicode { text: String },
    #[serde(rename = "release-all-keys")]
    ReleaseAllKeys,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RdpConnectConfig {
    pub session_id: String,
    pub connection_id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub domain: String,
    pub password: Option<String>,
    pub width: u32,
    pub height: u32,
    pub display_mode: String,
    pub use_nla: bool,
    pub certificate_policy: String,
    pub clipboard_mode: String,
    pub reconnect_enabled: bool,
    pub reconnect_max_attempts: u32,
    pub color_depth: u8,
    pub admin: bool,
    pub network: Option<ConnectionNetwork>,
}

pub struct RdpSession {
    pub config: RdpConnectConfig,
    pub state: Mutex<RdpSessionState>,
    message: Mutex<Option<String>>,
    error_kind: Mutex<Option<RdpErrorKind>>,
    generation: Mutex<u64>,
    frame_channel: Mutex<Option<Channel<InvokeResponseBody>>>,
    pending_frames: Mutex<VecDeque<Vec<u8>>>,
    input_sender: Mutex<Option<mpsc::UnboundedSender<IronRdpInputEvent>>>,
    input_database: Mutex<IronRdpInputDatabase>,
    frame_sequence: Mutex<u64>,
    worker: Mutex<Option<RdpWorker>>,
    reconnect_attempts: Mutex<u32>,
    reconnect_task: Mutex<Option<TauriJoinHandle<()>>>,
    clipboard_bridge: Mutex<Option<Arc<RdpClipboardBridge>>>,
    close_requested: AtomicBool,
    pending_certificates: Arc<Mutex<HashMap<String, RdpCertificatePending>>>,
}

pub struct RdpSessionManager {
    sessions: Mutex<HashMap<String, Arc<RdpSession>>>,
    engine: Arc<dyn RdpEngine>,
    pending_certificates: Arc<Mutex<HashMap<String, RdpCertificatePending>>>,
}

struct RdpWorker {
    generation: u64,
    input_sender: mpsc::UnboundedSender<IronRdpInputEvent>,
    join_handle: Option<JoinHandle<()>>,
}

struct RdpCertificatePending {
    session_id: String,
    generation: u64,
    responder: oneshot::Sender<RdpCertificateDecision>,
}

#[derive(Debug, Clone)]
struct RdpCertificateDecision {
    accepted: bool,
    remember: bool,
}

#[async_trait]
pub trait RdpEngine: Send + Sync {
    async fn connect(&self, app: AppHandle, session: Arc<RdpSession>, generation: u64);
    async fn send_input(
        &self,
        session: Arc<RdpSession>,
        events: Vec<RdpInputEvent>,
    ) -> AppResult<()>;
    async fn resize(&self, session: Arc<RdpSession>, width: u32, height: u32) -> AppResult<()>;
    async fn set_clipboard_text(&self, session: Arc<RdpSession>, text: String) -> AppResult<()>;
    async fn close(&self, session: Arc<RdpSession>) -> AppResult<()>;
}

pub struct IronRdpEngine;

impl IronRdpEngine {
    pub fn new() -> Self {
        Self
    }
}

struct IronRdpTransportStreamAdapter(BoxedTransportStream);

impl AsyncRead for IronRdpTransportStreamAdapter {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for IronRdpTransportStreamAdapter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

impl RdpSessionManager {
    pub fn new() -> Self {
        Self::with_engine(Arc::new(IronRdpEngine::new()))
    }

    pub fn with_engine(engine: Arc<dyn RdpEngine>) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            engine,
            pending_certificates: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn create_session(
        &self,
        app: AppHandle,
        config: RdpConnectConfig,
    ) -> AppResult<String> {
        let session_id = config.session_id.clone();
        let session = Arc::new(RdpSession {
            config,
            state: Mutex::new(RdpSessionState::Connecting),
            message: Mutex::new(Some("Establishing RDP transport".to_string())),
            error_kind: Mutex::new(None),
            generation: Mutex::new(0),
            frame_channel: Mutex::new(None),
            pending_frames: Mutex::new(VecDeque::with_capacity(MAX_FRAME_QUEUE)),
            input_sender: Mutex::new(None),
            input_database: Mutex::new(IronRdpInputDatabase::new()),
            frame_sequence: Mutex::new(0),
            worker: Mutex::new(None),
            reconnect_attempts: Mutex::new(0),
            reconnect_task: Mutex::new(None),
            clipboard_bridge: Mutex::new(None),
            close_requested: AtomicBool::new(false),
            pending_certificates: self.pending_certificates.clone(),
        });
        self.sessions
            .lock()
            .await
            .insert(session_id.clone(), session.clone());

        emit_state(
            &app,
            &session_id,
            RdpSessionState::Connecting,
            Some("Establishing RDP transport".to_string()),
            None,
        );
        self.engine.connect(app, session, 0).await;
        Ok(session_id)
    }

    pub async fn attach_frame_channel(
        &self,
        app: &AppHandle,
        session_id: &str,
        channel: Channel<InvokeResponseBody>,
    ) -> AppResult<()> {
        let session = self.get(session_id).await?;
        {
            let mut current = session.frame_channel.lock().await;
            *current = Some(channel);
        }
        flush_pending_frames(&session).await;
        emit_current_state(app, &session).await;
        Ok(())
    }

    pub async fn send_input(&self, session_id: &str, events: Vec<RdpInputEvent>) -> AppResult<()> {
        let session = self.get(session_id).await?;
        if events.is_empty() {
            return Ok(());
        }
        self.engine.send_input(session, events).await
    }

    pub async fn resize(&self, session_id: &str, width: u32, height: u32) -> AppResult<()> {
        let session = self.get(session_id).await?;
        if !(RDP_MIN_WIDTH..=RDP_MAX_WIDTH).contains(&width)
            || !(RDP_MIN_HEIGHT..=RDP_MAX_HEIGHT).contains(&height)
        {
            return Err(AppError::Config(
                "RDP resize is outside the supported range".to_string(),
            ));
        }
        self.engine.resize(session, width, height).await
    }

    pub async fn set_clipboard_text(&self, session_id: &str, text: String) -> AppResult<()> {
        let session = self.get(session_id).await?;
        if text.len() > MAX_CLIPBOARD_TEXT_BYTES {
            return Err(AppError::Config(
                "RDP clipboard text is too large".to_string(),
            ));
        }
        self.engine.set_clipboard_text(session, text).await
    }

    pub async fn reconnect(&self, app: AppHandle, session_id: &str) -> AppResult<()> {
        let session = self.get(session_id).await?;
        cancel_reconnect_task(&session).await;
        self.cancel_pending_certificates_for_session(session_id)
            .await;
        session.close_requested.store(false, Ordering::SeqCst);
        let generation = bump_generation(&session).await;
        *session.reconnect_attempts.lock().await = 0;
        shutdown_worker(&session).await;
        set_state(
            &session,
            RdpSessionState::Reconnecting,
            Some("Reconnecting RDP session".to_string()),
            None,
        )
        .await;
        emit_state(
            &app,
            session_id,
            RdpSessionState::Reconnecting,
            Some("Reconnecting RDP session".to_string()),
            None,
        );
        self.engine.connect(app, session, generation).await;
        Ok(())
    }

    pub async fn close(&self, app: &AppHandle, session_id: &str) -> AppResult<()> {
        let removed = self.sessions.lock().await.remove(session_id);
        if let Some(session) = removed {
            session.close_requested.store(true, Ordering::SeqCst);
            cancel_reconnect_task(&session).await;
            self.cancel_pending_certificates_for_session(session_id)
                .await;
            bump_generation(&session).await;
            set_state(
                &session,
                RdpSessionState::Disconnected,
                Some("RDP session closed".to_string()),
                None,
            )
            .await;
            stop_clipboard_bridge(&session).await;
            self.engine.close(session).await?;
            emit_state(
                app,
                session_id,
                RdpSessionState::Disconnected,
                Some("RDP session closed".to_string()),
                None,
            );
            let _ = app.emit("sessions-changed", ());
        }
        Ok(())
    }

    pub async fn respond_certificate(
        &self,
        request_id: &str,
        accepted: bool,
        remember: bool,
    ) -> AppResult<()> {
        let Some(pending) = self.pending_certificates.lock().await.remove(request_id) else {
            return Err(AppError::Cancelled(format!(
                "No pending RDP certificate request with id '{request_id}'"
            )));
        };

        if let Ok(session) = self.get(&pending.session_id).await {
            if *session.generation.lock().await != pending.generation {
                return Ok(());
            }
        }

        let _ = pending
            .responder
            .send(RdpCertificateDecision { accepted, remember });
        Ok(())
    }

    async fn cancel_pending_certificates_for_session(&self, session_id: &str) {
        let mut pending = self.pending_certificates.lock().await;
        let request_ids = pending
            .iter()
            .filter_map(|(request_id, request)| {
                (request.session_id == session_id).then(|| request_id.clone())
            })
            .collect::<Vec<_>>();
        for request_id in request_ids {
            if let Some(request) = pending.remove(&request_id) {
                let _ = request.responder.send(RdpCertificateDecision {
                    accepted: false,
                    remember: false,
                });
            }
        }
    }

    async fn get(&self, session_id: &str) -> AppResult<Arc<RdpSession>> {
        self.sessions
            .lock()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| {
                AppError::SessionNotFound(format!("RDP session '{session_id}' not found"))
            })
    }
}

impl Default for RdpSessionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RdpEngine for IronRdpEngine {
    async fn connect(&self, app: AppHandle, session: Arc<RdpSession>, generation: u64) {
        spawn_ironrdp_engine(app, session, generation);
    }

    async fn send_input(
        &self,
        session: Arc<RdpSession>,
        events: Vec<RdpInputEvent>,
    ) -> AppResult<()> {
        let input_events = {
            let mut database = session.input_database.lock().await;
            let mut output = Vec::new();
            for event in events {
                let fast_path = match rdp_input_to_fast_path_input(event) {
                    Some(RdpInputAction::Operations(operations)) => database.apply(operations),
                    Some(RdpInputAction::FastPath(event)) => smallvec::smallvec![event],
                    None => database.release_all(),
                };
                if !fast_path.is_empty() {
                    output.push(IronRdpInputEvent::FastPath(fast_path));
                }
            }
            output
        };

        let sender = session.input_sender.lock().await.clone().ok_or_else(|| {
            AppError::SessionNotFound("RDP session is not connected yet".to_string())
        })?;

        for event in input_events {
            sender
                .send(event)
                .map_err(|_| AppError::Channel("RDP input channel is closed".to_string()))?;
        }
        Ok(())
    }

    async fn resize(&self, session: Arc<RdpSession>, width: u32, height: u32) -> AppResult<()> {
        let sender = session.input_sender.lock().await.clone().ok_or_else(|| {
            AppError::SessionNotFound("RDP session is not connected yet".to_string())
        })?;
        sender
            .send(IronRdpInputEvent::Resize {
                width: u16::try_from(width).map_err(|_| {
                    AppError::Config("RDP width is outside the supported range".to_string())
                })?,
                height: u16::try_from(height).map_err(|_| {
                    AppError::Config("RDP height is outside the supported range".to_string())
                })?,
                scale_factor: 100,
                physical_size: None,
            })
            .map_err(|_| AppError::Channel("RDP input channel is closed".to_string()))?;
        Ok(())
    }

    async fn set_clipboard_text(&self, session: Arc<RdpSession>, text: String) -> AppResult<()> {
        if session.config.clipboard_mode == "disabled" {
            return Err(AppError::Config(
                "RDP clipboard redirection is disabled".to_string(),
            ));
        }

        let bridge = session
            .clipboard_bridge
            .lock()
            .await
            .clone()
            .ok_or_else(|| {
                AppError::SessionNotFound("RDP clipboard bridge is not available".to_string())
            })?;
        let text_for_clipboard = text.clone();
        tokio::task::spawn_blocking(move || write_clipboard_text_blocking(text_for_clipboard))
            .await
            .map_err(|error| AppError::Channel(format!("RDP clipboard task failed: {error}")))?
            .map_err(AppError::Channel)?;
        bridge.mark_text_written_from_remote(&text);
        bridge.notify_text_available().map_err(AppError::Channel)?;
        Ok(())
    }

    async fn close(&self, session: Arc<RdpSession>) -> AppResult<()> {
        shutdown_worker(&session).await;
        Ok(())
    }
}

pub fn load_saved_rdp_config(app: &AppHandle, connection_id: &str) -> AppResult<RdpConnectConfig> {
    let conn = config::load_connection_by_id(app, connection_id)?;
    let network = conn.network.clone();
    let password = resolve_rdp_password(app, conn.auth.as_ref())?;
    let ConnectionType::Rdp {
        host,
        port,
        username,
        domain,
        display,
        security,
        clipboard,
        reconnect,
        admin,
        ..
    } = conn.config
    else {
        return Err(AppError::Config(
            "Connection is not an RDP connection".to_string(),
        ));
    };

    Ok(RdpConnectConfig {
        session_id: uuid::Uuid::new_v4().to_string(),
        connection_id: connection_id.to_string(),
        name: conn.name,
        host,
        port,
        username,
        domain,
        password,
        width: display.width,
        height: display.height,
        display_mode: display.mode,
        use_nla: security.use_nla,
        certificate_policy: security.certificate_policy,
        clipboard_mode: clipboard.mode,
        reconnect_enabled: reconnect.enabled,
        reconnect_max_attempts: reconnect.max_attempts,
        color_depth: display.color_depth,
        admin,
        network,
    })
}

fn resolve_rdp_password(
    app: &AppHandle,
    auth: Option<&ConnectionAuth>,
) -> AppResult<Option<String>> {
    let Some(auth) = auth else {
        return Ok(None);
    };
    if auth.mode != "password" {
        return Ok(None);
    }
    if let Some(password_id) = auth.password_id.as_deref().filter(|id| !id.is_empty()) {
        return Ok(config::load_password_by_id(app, password_id)?.password);
    }
    crate::utils::crypto::decrypt_optional(&auth.password)
}

fn emit_state(
    app: &AppHandle,
    session_id: &str,
    state: RdpSessionState,
    message: Option<String>,
    error_kind: Option<RdpErrorKind>,
) {
    let payload = RdpStateEvent {
        session_id: session_id.to_string(),
        state,
        message,
        error_kind,
    };
    let _ = app.emit(format!("rdp-state-{session_id}").as_str(), payload);
}

fn spawn_ironrdp_engine(app: AppHandle, session: Arc<RdpSession>, generation: u64) {
    tauri::async_runtime::spawn(async move {
        shutdown_worker(&session).await;

        let iron_config = match build_ironrdp_config(&app, &session, generation).await {
            Ok(config) => config,
            Err(message) => {
                fail_rdp_session(
                    &app,
                    &session,
                    generation,
                    message,
                    RdpErrorKind::Internal,
                    false,
                )
                .await;
                return;
            }
        };

        tauri::async_runtime::spawn(async move {
            let session_id = session.config.session_id.clone();
            if *session.generation.lock().await != generation {
                return;
            }

            set_state(
                &session,
                RdpSessionState::Authenticating,
                Some("Authenticating RDP session".to_string()),
                None,
            )
            .await;
            emit_state(
                &app,
                &session_id,
                RdpSessionState::Authenticating,
                Some("Authenticating RDP session".to_string()),
                None,
            );

            let (output_sender, mut output_receiver) = mpsc::channel(2);
            let client = IronRdpClient::new(iron_config, output_sender);
            let input_sender = client.input_sender();
            {
                *session.input_sender.lock().await = Some(input_sender.clone());
                *session.input_database.lock().await = IronRdpInputDatabase::new();
                *session.frame_sequence.lock().await = 0;
            }

            let join_handle = std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                match runtime {
                    Ok(runtime) => runtime.block_on(client.run()),
                    Err(error) => tracing::error!(%error, "Failed to start IronRDP runtime"),
                }
            });
            {
                *session.worker.lock().await = Some(RdpWorker {
                    generation,
                    input_sender,
                    join_handle: Some(join_handle),
                });
            }
            let mut saw_terminal_event = false;

            while let Some(event) = output_receiver.recv().await {
                if *session.generation.lock().await != generation {
                    shutdown_worker(&session).await;
                    break;
                }

                match event {
                    RdpOutputEvent::ImagePatch {
                        buffer,
                        desktop_width,
                        desktop_height,
                        x,
                        y,
                        width,
                        height,
                    } => {
                        let was_active =
                            matches!(*session.state.lock().await, RdpSessionState::Active);
                        if !was_active {
                            *session.reconnect_attempts.lock().await = 0;
                            set_state(&session, RdpSessionState::Active, None, None).await;
                            emit_state(&app, &session_id, RdpSessionState::Active, None, None);
                            let _ = app.emit("sessions-changed", ());
                        }
                        let sequence = next_frame_sequence(&session).await;
                        match build_frame_from_ironrdp_image(
                            &buffer,
                            desktop_width,
                            desktop_height,
                            x,
                            y,
                            width,
                            height,
                            sequence,
                        ) {
                            Ok(frame) => queue_or_send_frame(&session, frame).await,
                            Err(error) => tracing::warn!(
                                session_id = %session_id,
                                "Discarded invalid RDP frame patch: {error}"
                            ),
                        }
                    }
                    RdpOutputEvent::ConnectionFailure(error) => {
                        saw_terminal_event = true;
                        let (error_kind, allow_reconnect) = classify_connector_error(&error);
                        let message = user_facing_connector_error(&error, error_kind);
                        tracing::warn!(
                            session_id = %session_id,
                            host = %session.config.host,
                            port = session.config.port,
                            error = ?error,
                            error_kind = ?error_kind,
                            "RDP connection failed"
                        );
                        fail_rdp_session(
                            &app,
                            &session,
                            generation,
                            message,
                            error_kind,
                            allow_reconnect,
                        )
                        .await;
                        break;
                    }
                    RdpOutputEvent::Terminated(result) => {
                        saw_terminal_event = true;
                        match result {
                            Ok(reason) => {
                                let message = format!("RDP session disconnected: {reason}");
                                set_state(
                                    &session,
                                    RdpSessionState::Disconnected,
                                    Some(message.clone()),
                                    None,
                                )
                                .await;
                                emit_state(
                                    &app,
                                    &session_id,
                                    RdpSessionState::Disconnected,
                                    Some(message),
                                    None,
                                );
                                let _ = app.emit("sessions-changed", ());
                            }
                            Err(error) => {
                                let (error_kind, allow_reconnect) = classify_session_error(&error);
                                let message = user_facing_session_error(&error, error_kind);
                                tracing::warn!(
                                    session_id = %session_id,
                                    host = %session.config.host,
                                    port = session.config.port,
                                    error = ?error,
                                    error_kind = ?error_kind,
                                    "RDP active session failed"
                                );
                                fail_rdp_session(
                                    &app,
                                    &session,
                                    generation,
                                    message,
                                    error_kind,
                                    allow_reconnect,
                                )
                                .await;
                            }
                        }
                        break;
                    }
                    RdpOutputEvent::PointerDefault => emit_pointer_event(
                        &app,
                        &session_id,
                        RdpPointerEvent::Default {
                            session_id: session_id.clone(),
                        },
                    ),
                    RdpOutputEvent::PointerHidden => emit_pointer_event(
                        &app,
                        &session_id,
                        RdpPointerEvent::Hidden {
                            session_id: session_id.clone(),
                        },
                    ),
                    RdpOutputEvent::PointerPosition { x, y } => emit_pointer_event(
                        &app,
                        &session_id,
                        RdpPointerEvent::Position {
                            session_id: session_id.clone(),
                            x,
                            y,
                        },
                    ),
                    RdpOutputEvent::PointerBitmap(pointer) => emit_pointer_event(
                        &app,
                        &session_id,
                        RdpPointerEvent::Bitmap {
                            session_id: session_id.clone(),
                            width: pointer.width,
                            height: pointer.height,
                            hotspot_x: pointer.hotspot_x,
                            hotspot_y: pointer.hotspot_y,
                            rgba_base64: base64::engine::general_purpose::STANDARD
                                .encode(&pointer.bitmap_data),
                        },
                    ),
                }
            }

            shutdown_worker(&session).await;
            if !saw_terminal_event && *session.generation.lock().await == generation {
                set_state(
                    &session,
                    RdpSessionState::Disconnected,
                    Some("RDP session ended".to_string()),
                    None,
                )
                .await;
                emit_state(
                    &app,
                    &session_id,
                    RdpSessionState::Disconnected,
                    Some("RDP session ended".to_string()),
                    None,
                );
                let _ = app.emit("sessions-changed", ());
            }
        });
    });
}

async fn build_ironrdp_config(
    app: &AppHandle,
    session: &Arc<RdpSession>,
    generation: u64,
) -> Result<IronRdpConfig, String> {
    let config = &session.config;
    let width = u16::try_from(config.width)
        .map_err(|_| "RDP display width is outside the supported range".to_string())?;
    let height = u16::try_from(config.height)
        .map_err(|_| "RDP display height is outside the supported range".to_string())?;
    let color_depth = match config.color_depth {
        16 | 32 => u32::from(config.color_depth),
        _ => 32,
    };

    if config.display_mode == "native" {
        tracing::warn!(
            session_id = %config.session_id,
            "RDP native display mode is not implemented; using fixed initial size"
        );
    }

    tracing::debug!(
        session_id = %config.session_id,
        "RDP TLS backend: {}",
        rdp_tls_backend_label()
    );

    if config.admin {
        tracing::warn!(
            session_id = %config.session_id,
            "RDP admin (console) session requested, but the pinned IronRDP version does not \
             support sending the administrative session flag; this option is currently ignored"
        );
    }

    let certificate_verifier = Arc::new(NyaTermRdpCertificateVerifier {
        app: app.clone(),
        session: session.clone(),
        generation,
    });

    let mut builder = IronRdpConfigBuilder::new()
        .with_destination(IronRdpDestination::from_parts(
            config.host.clone(),
            config.port,
        ))
        .with_transport(IronRdpTransportKind::Direct)
        .with_username(config.username.clone())
        .with_domain(config.domain.clone())
        .with_password(config.password.clone().unwrap_or_default())
        .with_desktop_width(width)
        .with_desktop_height(height)
        .with_desktop_scale_factor(100)
        .with_color_depth(color_depth)
        .with_credssp(config.use_nla)
        .with_tls(true)
        .with_autologon(true)
        // Some servers send compressed FastPath bitmap updates that IronRDP 0.17 can decode
        // inconsistently after activation/reactivation, producing malformed 0 bpp bitmap PDUs.
        // Do not advertise bulk compression until the full output path is stable.
        .with_compression(false)
        .with_server_pointer(true)
        .with_client_build(client_build())
        .with_client_dir("C:\\Windows\\System32\\mstscax.dll")
        .with_client_name(client_name())
        .with_platform(current_platform())
        .with_server_certificate_verifier(certificate_verifier);

    if let Some(network) = config.network.clone() {
        let app = app.clone();
        builder = builder.with_direct_transport_connector(Arc::new(move |destination| {
            let app = app.clone();
            let network = network.clone();
            Box::pin(async move {
                let transport = open_tcp_transport(
                    &app,
                    destination.name(),
                    destination.port(),
                    Some(&network),
                    None,
                )
                .await
                .map_err(|error| io::Error::other(error.to_string()))?;
                Ok(IronRdpDirectTransport {
                    stream: Box::new(IronRdpTransportStreamAdapter(transport.stream)),
                    local_addr: transport.local_addr,
                })
            })
        }));
    }

    if config.clipboard_mode == "disabled" {
        *session.clipboard_bridge.lock().await = None;
        builder = builder.with_clipboard(IronRdpClipboardType::Disable);
    } else {
        let bridge = Arc::new(RdpClipboardBridge::new(config.session_id.clone()));
        *session.clipboard_bridge.lock().await = Some(bridge.clone());
        builder = builder
            .with_clipboard(IronRdpClipboardType::Enable)
            .with_cliprdr_factory(move |proxy| {
                Box::new(RdpClipboardBackendFactory::new(bridge.clone(), proxy))
            });
    }

    builder
        .build()
        .map_err(|error| format!("Unable to build RDP config: {error}"))
}

#[derive(Clone)]
struct NyaTermRdpCertificateVerifier {
    app: AppHandle,
    session: Arc<RdpSession>,
    generation: u64,
}

impl fmt::Debug for NyaTermRdpCertificateVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NyaTermRdpCertificateVerifier")
            .field("session_id", &self.session.config.session_id)
            .field("generation", &self.generation)
            .finish()
    }
}

impl ServerCertificateVerifier for NyaTermRdpCertificateVerifier {
    fn verify_server_certificate<'a>(
        &'a self,
        host: &'a str,
        port: u16,
        der: Vec<u8>,
    ) -> ServerCertificateVerifyFuture<'a> {
        Box::pin(async move { self.verify(host, port, &der).await })
    }
}

impl NyaTermRdpCertificateVerifier {
    async fn verify(&self, host: &str, port: u16, der: &[u8]) -> Result<(), String> {
        if *self.session.generation.lock().await != self.generation {
            return Err("certificate rejected: stale RDP connection generation".to_string());
        }

        let fingerprint = rdp_certificate_fingerprint(der);
        let metadata = parse_rdp_certificate_metadata(der);
        let known_host_status = crate::storage::check_rdp_known_host(host, port, &fingerprint)
            .map_err(|error| format!("certificate rejected: {error}"))?;

        match certificate_policy_allows_without_prompt(
            &self.session.config.certificate_policy,
            known_host_status,
        )? {
            true => Ok(()),
            false => {
                self.prompt_for_certificate(host, port, fingerprint, metadata, known_host_status)
                    .await
            }
        }
    }

    async fn prompt_for_certificate(
        &self,
        host: &str,
        port: u16,
        fingerprint: String,
        metadata: crate::storage::RdpCertificateMetadata,
        known_host_status: crate::storage::KnownHostCheck,
    ) -> Result<(), String> {
        let request_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.session.pending_certificates.lock().await.insert(
            request_id.clone(),
            RdpCertificatePending {
                session_id: self.session.config.session_id.clone(),
                generation: self.generation,
                responder: tx,
            },
        );

        set_state(
            &self.session,
            RdpSessionState::CertificateVerification,
            Some("Verifying RDP certificate".to_string()),
            None,
        )
        .await;
        emit_state(
            &self.app,
            &self.session.config.session_id,
            RdpSessionState::CertificateVerification,
            Some("Verifying RDP certificate".to_string()),
            None,
        );

        let payload = RdpCertificateVerifyEvent {
            request_id: request_id.clone(),
            session_id: self.session.config.session_id.clone(),
            host: host.to_string(),
            port,
            fingerprint: fingerprint.clone(),
            subject: metadata.subject.clone(),
            issuer: metadata.issuer.clone(),
            valid_from: metadata.valid_from.clone(),
            valid_to: metadata.valid_to.clone(),
            known_host_status: known_host_status_label(known_host_status).to_string(),
        };
        let _ = self.app.emit("rdp-certificate-verify", payload);

        let decision = match tokio::time::timeout(CERTIFICATE_PROMPT_TIMEOUT, rx).await {
            Ok(Ok(decision)) => decision,
            _ => {
                self.session
                    .pending_certificates
                    .lock()
                    .await
                    .remove(&request_id);
                return Err("certificate rejected".to_string());
            }
        };

        if *self.session.generation.lock().await != self.generation {
            return Err("certificate rejected: stale RDP connection generation".to_string());
        }
        if !decision.accepted {
            return Err("certificate rejected".to_string());
        }
        if decision.remember {
            crate::storage::upsert_rdp_known_host(host, port, &fingerprint, metadata)
                .map_err(|error| format!("certificate rejected: {error}"))?;
        }
        Ok(())
    }
}

fn rdp_certificate_fingerprint(der: &[u8]) -> String {
    let digest = Sha256::digest(der);
    format!("SHA256:{}", hex::encode(digest))
}

fn parse_rdp_certificate_metadata(der: &[u8]) -> crate::storage::RdpCertificateMetadata {
    let Ok(cert) = x509_cert::Certificate::from_der(der) else {
        return crate::storage::RdpCertificateMetadata::default();
    };
    crate::storage::RdpCertificateMetadata {
        subject: Some(cert.tbs_certificate.subject.to_string()),
        issuer: Some(cert.tbs_certificate.issuer.to_string()),
        valid_from: Some(cert.tbs_certificate.validity.not_before.to_string()),
        valid_to: Some(cert.tbs_certificate.validity.not_after.to_string()),
    }
}

fn known_host_status_label(status: crate::storage::KnownHostCheck) -> &'static str {
    match status {
        crate::storage::KnownHostCheck::Match => "match",
        crate::storage::KnownHostCheck::HostSeen => "changed",
        crate::storage::KnownHostCheck::UnknownHost => "unknown",
    }
}

fn certificate_policy_allows_without_prompt(
    policy: &str,
    status: crate::storage::KnownHostCheck,
) -> Result<bool, String> {
    match policy {
        "strict" => match status {
            crate::storage::KnownHostCheck::Match => Ok(true),
            crate::storage::KnownHostCheck::UnknownHost => Err("unknown certificate".to_string()),
            crate::storage::KnownHostCheck::HostSeen => {
                Err("certificate fingerprint changed".to_string())
            }
        },
        "accept-temporarily" => Ok(true),
        _ if status == crate::storage::KnownHostCheck::Match => Ok(true),
        _ => Ok(false),
    }
}

struct RdpClipboardBridge {
    session_id: String,
    shutdown: AtomicBool,
    watcher_started: AtomicBool,
    proxy: std::sync::Mutex<Option<Arc<std::sync::Mutex<Box<dyn ClipboardMessageProxy>>>>>,
    last_text_hash: std::sync::Mutex<Option<u64>>,
}

impl fmt::Debug for RdpClipboardBridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RdpClipboardBridge")
            .field("session_id", &self.session_id)
            .field("shutdown", &self.shutdown.load(Ordering::SeqCst))
            .field(
                "watcher_started",
                &self.watcher_started.load(Ordering::SeqCst),
            )
            .finish_non_exhaustive()
    }
}

impl RdpClipboardBridge {
    fn new(session_id: String) -> Self {
        Self {
            session_id,
            shutdown: AtomicBool::new(false),
            watcher_started: AtomicBool::new(false),
            proxy: std::sync::Mutex::new(None),
            last_text_hash: std::sync::Mutex::new(None),
        }
    }

    fn set_proxy(&self, proxy: Arc<std::sync::Mutex<Box<dyn ClipboardMessageProxy>>>) {
        if let Ok(mut current) = self.proxy.lock() {
            *current = Some(proxy);
        }
    }

    fn notify_text_available(&self) -> Result<(), String> {
        let proxy = self
            .proxy
            .lock()
            .map_err(|_| "RDP clipboard proxy lock is poisoned".to_string())?
            .clone()
            .ok_or_else(|| "RDP clipboard channel is not ready".to_string())?;
        let proxy = proxy
            .lock()
            .map_err(|_| "RDP clipboard proxy lock is poisoned".to_string())?;
        proxy.send_clipboard_message(ClipboardMessage::SendInitiateCopy(vec![
            ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT),
        ]));
        Ok(())
    }

    fn start_watcher(
        self: &Arc<Self>,
        proxy: Arc<std::sync::Mutex<Box<dyn ClipboardMessageProxy>>>,
    ) {
        if self.watcher_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let bridge = self.clone();
        std::thread::spawn(move || {
            while !bridge.shutdown.load(Ordering::SeqCst) {
                if let Some(text) = read_clipboard_text_blocking() {
                    if clipboard_text_within_limit(&text) {
                        let hash = stable_text_hash(&text);
                        let changed = if let Ok(mut last) = bridge.last_text_hash.lock() {
                            if *last == Some(hash) {
                                false
                            } else {
                                *last = Some(hash);
                                true
                            }
                        } else {
                            false
                        };
                        if changed && let Ok(proxy) = proxy.lock() {
                            proxy.send_clipboard_message(ClipboardMessage::SendInitiateCopy(vec![
                                ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT),
                            ]));
                        }
                    }
                }
                std::thread::sleep(CLIPBOARD_POLL_INTERVAL);
            }
        });
    }

    fn stop(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }

    fn mark_text_written_from_remote(&self, text: &str) {
        if let Ok(mut last) = self.last_text_hash.lock() {
            *last = Some(stable_text_hash(text));
        }
    }
}

struct RdpClipboardBackendFactory {
    bridge: Arc<RdpClipboardBridge>,
    proxy: Arc<std::sync::Mutex<Box<dyn ClipboardMessageProxy>>>,
}

impl RdpClipboardBackendFactory {
    fn new(bridge: Arc<RdpClipboardBridge>, proxy: Box<dyn ClipboardMessageProxy>) -> Self {
        let proxy = Arc::new(std::sync::Mutex::new(proxy));
        bridge.set_proxy(proxy.clone());
        Self { bridge, proxy }
    }
}

impl CliprdrBackendFactory for RdpClipboardBackendFactory {
    fn build_cliprdr_backend(&self) -> Box<dyn CliprdrBackend> {
        Box::new(RdpClipboardBackend {
            bridge: self.bridge.clone(),
            proxy: self.proxy.clone(),
            negotiated_capabilities: ClipboardGeneralCapabilityFlags::empty(),
        })
    }
}

#[derive(Debug)]
struct RdpClipboardBackend {
    bridge: Arc<RdpClipboardBridge>,
    proxy: Arc<std::sync::Mutex<Box<dyn ClipboardMessageProxy>>>,
    negotiated_capabilities: ClipboardGeneralCapabilityFlags,
}

impl_as_any!(RdpClipboardBackend);

impl RdpClipboardBackend {
    fn send(&self, message: ClipboardMessage) {
        if let Ok(proxy) = self.proxy.lock() {
            proxy.send_clipboard_message(message);
        }
    }

    fn advertise_text_if_available(&self) {
        if read_clipboard_text_blocking()
            .filter(|text| !text.is_empty() && clipboard_text_within_limit(text))
            .is_some()
        {
            self.send(ClipboardMessage::SendInitiateCopy(vec![
                ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT),
            ]));
        }
    }
}

impl CliprdrBackend for RdpClipboardBackend {
    fn temporary_directory(&self) -> &str {
        ".nyaterm-rdp-cliprdr"
    }

    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        ClipboardGeneralCapabilityFlags::USE_LONG_FORMAT_NAMES
    }

    fn on_ready(&mut self) {
        self.bridge.start_watcher(self.proxy.clone());
        self.advertise_text_if_available();
    }

    fn on_request_format_list(&mut self) {
        self.advertise_text_if_available();
    }

    fn on_process_negotiated_capabilities(
        &mut self,
        capabilities: ClipboardGeneralCapabilityFlags,
    ) {
        self.negotiated_capabilities = capabilities;
    }

    fn on_remote_copy(&mut self, available_formats: &[ClipboardFormat]) {
        if available_formats
            .iter()
            .any(|format| format.id == ClipboardFormatId::CF_UNICODETEXT)
        {
            self.send(ClipboardMessage::SendInitiatePaste(
                ClipboardFormatId::CF_UNICODETEXT,
            ));
        }
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        if request.format != ClipboardFormatId::CF_UNICODETEXT {
            self.send(ClipboardMessage::SendFormatData(
                OwnedFormatDataResponse::new_error(),
            ));
            return;
        }
        let response = match read_clipboard_text_blocking() {
            Some(text) if clipboard_text_within_limit(&text) => {
                OwnedFormatDataResponse::new_unicode_string(&text)
            }
            _ => OwnedFormatDataResponse::new_error(),
        };
        self.send(ClipboardMessage::SendFormatData(response));
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        if response.is_error() {
            return;
        }
        let Ok(text) = response.to_unicode_string() else {
            return;
        };
        if !clipboard_text_within_limit(&text) {
            tracing::warn!(
                session_id = %self.bridge.session_id,
                "Ignoring oversized RDP clipboard text from remote"
            );
            return;
        }
        self.bridge.mark_text_written_from_remote(&text);
        std::thread::spawn(move || {
            let _ = write_clipboard_text_blocking(text);
        });
    }

    fn on_file_contents_request(&mut self, request: FileContentsRequest) {
        self.send(ClipboardMessage::SendFileContentsResponse(
            FileContentsResponse::new_error(request.stream_id),
        ));
    }

    fn on_file_contents_response(&mut self, _response: FileContentsResponse<'_>) {}

    fn on_lock(&mut self, _data_id: LockDataId) {}

    fn on_unlock(&mut self, _data_id: LockDataId) {}
}

fn read_clipboard_text_blocking() -> Option<String> {
    let start = std::time::Instant::now();
    let mut clipboard = arboard::Clipboard::new().ok()?;
    if start.elapsed() > CLIPBOARD_TIMEOUT {
        return None;
    }
    clipboard.get_text().ok()
}

fn write_clipboard_text_blocking(text: String) -> Result<(), String> {
    let start = std::time::Instant::now();
    let mut clipboard =
        arboard::Clipboard::new().map_err(|error| format!("failed to open clipboard: {error}"))?;
    if start.elapsed() > CLIPBOARD_TIMEOUT {
        return Err("clipboard write timed out".to_string());
    }
    clipboard
        .set_text(text)
        .map_err(|error| format!("failed to write clipboard text: {error}"))
}

fn clipboard_text_within_limit(text: &str) -> bool {
    text.len() <= MAX_CLIPBOARD_TEXT_BYTES
}

fn stable_text_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

enum RdpInputAction {
    Operations(Vec<IronRdpInputOperation>),
    FastPath(IronRdpFastPathInputEvent),
}

fn rdp_input_to_fast_path_input(event: RdpInputEvent) -> Option<RdpInputAction> {
    match event {
        RdpInputEvent::KeyDown {
            scan_code,
            extended,
            ..
        } if is_right_shift_scan_code(scan_code, extended) => Some(RdpInputAction::FastPath(
            IronRdpFastPathInputEvent::KeyboardEvent(
                IronRdpKeyboardFlags::empty(),
                RDP_RIGHT_SHIFT_SCAN_CODE as u8,
            ),
        )),
        RdpInputEvent::KeyDown {
            scan_code,
            extended,
            ..
        } => Some(RdpInputAction::Operations(vec![
            IronRdpInputOperation::KeyPressed(IronRdpScancode::from_u8(extended, scan_code as u8)),
        ])),
        RdpInputEvent::KeyUp {
            scan_code,
            extended,
            ..
        } if is_right_shift_scan_code(scan_code, extended) => Some(RdpInputAction::FastPath(
            IronRdpFastPathInputEvent::KeyboardEvent(
                IronRdpKeyboardFlags::RELEASE,
                RDP_RIGHT_SHIFT_SCAN_CODE as u8,
            ),
        )),
        RdpInputEvent::KeyUp {
            scan_code,
            extended,
            ..
        } => Some(RdpInputAction::Operations(vec![
            IronRdpInputOperation::KeyReleased(IronRdpScancode::from_u8(extended, scan_code as u8)),
        ])),
        RdpInputEvent::MouseMove { x, y } => Some(RdpInputAction::Operations(vec![
            IronRdpInputOperation::MouseMove(IronRdpMousePosition {
                x: clamp_u32_to_u16(x),
                y: clamp_u32_to_u16(y),
            }),
        ])),
        RdpInputEvent::MouseButton {
            button,
            pressed,
            x,
            y,
        } => {
            let Some(button) = ironrdp_mouse_button(&button) else {
                return Some(RdpInputAction::Operations(Vec::new()));
            };
            let mut operations = vec![IronRdpInputOperation::MouseMove(IronRdpMousePosition {
                x: clamp_u32_to_u16(x),
                y: clamp_u32_to_u16(y),
            })];
            operations.push(if pressed {
                IronRdpInputOperation::MouseButtonPressed(button)
            } else {
                IronRdpInputOperation::MouseButtonReleased(button)
            });
            Some(RdpInputAction::Operations(operations))
        }
        RdpInputEvent::MouseWheel {
            delta_x,
            delta_y,
            x,
            y,
        } => {
            let mut operations = vec![IronRdpInputOperation::MouseMove(IronRdpMousePosition {
                x: clamp_u32_to_u16(x),
                y: clamp_u32_to_u16(y),
            })];
            if delta_x.abs() > 0.001 {
                operations.push(IronRdpInputOperation::WheelRotations(
                    IronRdpWheelRotations {
                        is_vertical: false,
                        rotation_units: clamp_f64_to_i16(-delta_x),
                    },
                ));
            }
            if delta_y.abs() > 0.001 {
                operations.push(IronRdpInputOperation::WheelRotations(
                    IronRdpWheelRotations {
                        is_vertical: true,
                        rotation_units: clamp_f64_to_i16(-delta_y),
                    },
                ));
            }
            Some(RdpInputAction::Operations(operations))
        }
        RdpInputEvent::Unicode { text } => {
            let mut operations = Vec::new();
            for character in text.chars() {
                operations.push(IronRdpInputOperation::UnicodeKeyPressed(character));
                operations.push(IronRdpInputOperation::UnicodeKeyReleased(character));
            }
            Some(RdpInputAction::Operations(operations))
        }
        RdpInputEvent::ReleaseAllKeys => None,
    }
}

#[cfg(test)]
fn rdp_input_to_operations(event: RdpInputEvent) -> Option<Vec<IronRdpInputOperation>> {
    match rdp_input_to_fast_path_input(event)? {
        RdpInputAction::Operations(operations) => Some(operations),
        RdpInputAction::FastPath(_) => Some(Vec::new()),
    }
}

#[cfg(test)]
fn rdp_input_to_direct_fast_path(event: RdpInputEvent) -> Option<IronRdpFastPathInputEvent> {
    match rdp_input_to_fast_path_input(event)? {
        RdpInputAction::FastPath(event) => Some(event),
        RdpInputAction::Operations(_) => None,
    }
}

fn is_right_shift_scan_code(scan_code: u16, extended: bool) -> bool {
    !extended && scan_code == RDP_RIGHT_SHIFT_SCAN_CODE
}

fn ironrdp_mouse_button(button: &str) -> Option<IronRdpMouseButton> {
    match button {
        "left" => Some(IronRdpMouseButton::Left),
        "middle" => Some(IronRdpMouseButton::Middle),
        "right" => Some(IronRdpMouseButton::Right),
        "back" => Some(IronRdpMouseButton::X1),
        "forward" => Some(IronRdpMouseButton::X2),
        _ => None,
    }
}

fn clamp_u32_to_u16(value: u32) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

fn clamp_f64_to_i16(value: f64) -> i16 {
    if value.is_nan() {
        return 0;
    }
    value.clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16
}

async fn close_current_input_sender(session: &RdpSession) {
    if let Some(sender) = session.input_sender.lock().await.take() {
        let _ = sender.send(IronRdpInputEvent::Close);
    }
}

async fn shutdown_worker(session: &RdpSession) {
    close_current_input_sender(session).await;
    let mut worker = session.worker.lock().await;
    if let Some(mut worker) = worker.take() {
        tracing::debug!(
            session_id = %session.config.session_id,
            generation = worker.generation,
            "Shutting down RDP worker"
        );
        let _ = worker.input_sender.send(IronRdpInputEvent::Close);
        if let Some(handle) = worker.join_handle.take() {
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
    }
}

async fn stop_clipboard_bridge(session: &RdpSession) {
    if let Some(bridge) = session.clipboard_bridge.lock().await.take() {
        bridge.stop();
    }
}

async fn cancel_reconnect_task(session: &RdpSession) {
    if let Some(task) = session.reconnect_task.lock().await.take() {
        task.abort();
    }
}

async fn schedule_auto_reconnect(
    app: AppHandle,
    session: Arc<RdpSession>,
    failed_generation: u64,
    failure_message: String,
    error_kind: RdpErrorKind,
) -> bool {
    if session.close_requested.load(Ordering::SeqCst) || !session.config.reconnect_enabled {
        return false;
    }
    let mut attempts = session.reconnect_attempts.lock().await;
    if *attempts >= session.config.reconnect_max_attempts {
        return false;
    }
    *attempts += 1;
    let attempt = *attempts;
    drop(attempts);

    let delay = reconnect_delay(attempt);
    let max_attempts = session.config.reconnect_max_attempts;
    let session_id = session.config.session_id.clone();
    let message = format!("Reconnecting ({attempt}/{max_attempts})");

    set_state(
        &session,
        RdpSessionState::Reconnecting,
        Some(message.clone()),
        Some(error_kind),
    )
    .await;
    emit_state(
        &app,
        &session_id,
        RdpSessionState::Reconnecting,
        Some(message),
        Some(error_kind),
    );

    let task_session = session.clone();
    let task_app = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        sleep(delay).await;
        if task_session.close_requested.load(Ordering::SeqCst) {
            return;
        }
        if *task_session.generation.lock().await != failed_generation {
            return;
        }
        let generation = bump_generation(&task_session).await;
        spawn_ironrdp_engine(task_app, task_session, generation);
    });
    *session.reconnect_task.lock().await = Some(task);
    tracing::warn!(
        session_id = %session_id,
        attempt,
        max_attempts,
        delay_ms = delay.as_millis(),
        error = %failure_message,
        "Scheduled automatic RDP reconnect"
    );
    true
}

fn reconnect_delay(attempt: u32) -> Duration {
    let base = match attempt {
        0 | 1 => 1_000,
        2 => 2_000,
        3 => 4_000,
        4 => 8_000,
        5 => 15_000,
        _ => 30_000,
    };
    let jitter = u64::from(rand::random::<u8>() % 250);
    Duration::from_millis(base + jitter)
}

fn emit_pointer_event(app: &AppHandle, session_id: &str, event: RdpPointerEvent) {
    let _ = app.emit(format!("rdp-pointer-{session_id}").as_str(), event);
}

fn classify_connector_error(error: &ironrdp::connector::ConnectorError) -> (RdpErrorKind, bool) {
    let text = format!("{error:?}").to_ascii_lowercase();
    classify_connector_error_text(&text)
}

fn classify_connector_error_text(text: &str) -> (RdpErrorKind, bool) {
    if is_nyaterm_certificate_rejection(text) {
        return (RdpErrorKind::Certificate, false);
    }
    if is_tls_error_text(text) {
        return (RdpErrorKind::Tls, true);
    }
    if text.contains("credssp")
        || text.contains("authentication")
        || text.contains("password")
        || text.contains("logon")
        || text.contains("access denied")
    {
        return (RdpErrorKind::Authentication, false);
    }
    if text.contains("negotiation") || text.contains("x224") || text.contains("nla") {
        return (RdpErrorKind::Negotiation, false);
    }
    if text.contains("certificate") {
        return (RdpErrorKind::Certificate, false);
    }
    (RdpErrorKind::Transport, true)
}

fn classify_session_error<E: fmt::Debug>(error: &E) -> (RdpErrorKind, bool) {
    let text = format!("{error:?}").to_ascii_lowercase();
    if is_tls_error_text(&text) {
        return (RdpErrorKind::Tls, true);
    }
    if text.contains("authentication") || text.contains("password") {
        return (RdpErrorKind::Authentication, false);
    }
    if text.contains("clipboard") || text.contains("cliprdr") {
        return (RdpErrorKind::Clipboard, false);
    }
    if text.contains("eof")
        || text.contains("reset")
        || text.contains("timeout")
        || text.contains("broken pipe")
        || text.contains("connection aborted")
        || text.contains("transport")
    {
        return (RdpErrorKind::Transport, true);
    }
    (RdpErrorKind::Session, true)
}

fn is_tls_error_text(text: &str) -> bool {
    text.contains("tls")
        || text.contains("handshake")
        || text.contains("schannel")
        || text.contains("connectionreset")
        || text.contains("connection reset")
        || text.contains("10054")
}

fn is_nyaterm_certificate_rejection(text: &str) -> bool {
    text.contains("certificate rejected")
        || text.contains("unknown certificate")
        || text.contains("certificate fingerprint changed")
}

fn user_facing_connector_error(
    error: &ironrdp::connector::ConnectorError,
    kind: RdpErrorKind,
) -> String {
    match kind {
        RdpErrorKind::Certificate => format!("RDP certificate error: {error:?}"),
        RdpErrorKind::Authentication => "RDP authentication failed".to_string(),
        RdpErrorKind::Negotiation => format!("RDP negotiation failed: {error:?}"),
        RdpErrorKind::Tls => format!("RDP TLS connection failed: {error:?}"),
        RdpErrorKind::Transport => format!("RDP transport error: {error:?}"),
        _ => format!("RDP connection failed: {error:?}"),
    }
}

fn user_facing_session_error<E: fmt::Debug>(error: &E, kind: RdpErrorKind) -> String {
    match kind {
        RdpErrorKind::Authentication => "RDP authentication failed".to_string(),
        RdpErrorKind::Tls => format!("RDP TLS connection failed: {error:?}"),
        RdpErrorKind::Clipboard => format!("RDP clipboard error: {error:?}"),
        RdpErrorKind::Transport => format!("RDP transport interrupted: {error:?}"),
        _ => format!("RDP session error: {error:?}"),
    }
}

async fn fail_rdp_session(
    app: &AppHandle,
    session: &Arc<RdpSession>,
    generation: u64,
    message: String,
    error_kind: RdpErrorKind,
    allow_auto_reconnect: bool,
) {
    if *session.generation.lock().await != generation {
        return;
    }
    if allow_auto_reconnect
        && schedule_auto_reconnect(
            app.clone(),
            session.clone(),
            generation,
            message.clone(),
            error_kind,
        )
        .await
    {
        return;
    }
    set_state(
        session,
        RdpSessionState::Failed,
        Some(message.clone()),
        Some(error_kind),
    )
    .await;
    emit_state(
        app,
        &session.config.session_id,
        RdpSessionState::Failed,
        Some(message),
        Some(error_kind),
    );
    let _ = app.emit("sessions-changed", ());
}

async fn next_frame_sequence(session: &RdpSession) -> u64 {
    let mut sequence = session.frame_sequence.lock().await;
    let current = *sequence;
    *sequence = sequence.wrapping_add(1);
    current
}

fn build_frame_from_ironrdp_image(
    pixels: &[u32],
    desktop_width: u16,
    desktop_height: u16,
    patch_x: u16,
    patch_y: u16,
    patch_width: u16,
    patch_height: u16,
    sequence: u64,
) -> AppResult<Vec<u8>> {
    let pixel_count = usize::from(patch_width)
        .checked_mul(usize::from(patch_height))
        .ok_or_else(|| AppError::Config("RDP frame pixel count overflows".to_string()))?;
    if pixels.len() < pixel_count {
        return Err(AppError::Config(
            "RDP frame pixel buffer is smaller than the patch".to_string(),
        ));
    }
    let payload_len = pixel_count
        .checked_mul(4)
        .ok_or_else(|| AppError::Config("RDP frame payload size overflows".to_string()))?;
    let mut payload = vec![0_u8; payload_len];
    for (index, pixel) in pixels.iter().take(pixel_count).enumerate() {
        let [_, red, green, blue] = pixel.to_be_bytes();
        let offset = index * 4;
        payload[offset] = red;
        payload[offset + 1] = green;
        payload[offset + 2] = blue;
        payload[offset + 3] = 255;
    }

    encode_frame_patch(&RemoteDesktopFramePatch {
        sequence,
        desktop_width: u32::from(desktop_width),
        desktop_height: u32::from(desktop_height),
        x: u32::from(patch_x),
        y: u32::from(patch_y),
        width: u32::from(patch_width),
        height: u32::from(patch_height),
        stride: u32::from(patch_width) * 4,
        pixel_format: RemoteDesktopPixelFormat::Rgba8888,
        payload: &payload,
    })
}

fn client_build() -> u32 {
    env!("CARGO_PKG_VERSION")
        .split('.')
        .take(3)
        .fold(0_u32, |acc, part| {
            acc.saturating_mul(100)
                .saturating_add(part.parse::<u32>().unwrap_or(0))
        })
}

fn client_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "NyaTerm".to_string())
}

fn current_platform() -> MajorPlatformType {
    #[cfg(target_os = "windows")]
    {
        MajorPlatformType::WINDOWS
    }
    #[cfg(target_os = "macos")]
    {
        MajorPlatformType::MACINTOSH
    }
    #[cfg(target_os = "ios")]
    {
        MajorPlatformType::IOS
    }
    #[cfg(target_os = "android")]
    {
        MajorPlatformType::ANDROID
    }
    #[cfg(all(
        not(target_os = "windows"),
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    ))]
    {
        MajorPlatformType::UNIX
    }
}

#[cfg(windows)]
fn rdp_tls_backend_label() -> &'static str {
    "native-tls (Schannel)"
}

#[cfg(not(windows))]
fn rdp_tls_backend_label() -> &'static str {
    "rustls"
}

async fn set_state(
    session: &RdpSession,
    state: RdpSessionState,
    message: Option<String>,
    error_kind: Option<RdpErrorKind>,
) {
    *session.state.lock().await = state;
    *session.message.lock().await = message;
    *session.error_kind.lock().await = error_kind;
}

async fn emit_current_state(app: &AppHandle, session: &RdpSession) {
    let state = session.state.lock().await.clone();
    let message = session.message.lock().await.clone();
    let error_kind = *session.error_kind.lock().await;
    emit_state(app, &session.config.session_id, state, message, error_kind);
}

async fn bump_generation(session: &RdpSession) -> u64 {
    let mut generation = session.generation.lock().await;
    *generation = generation.wrapping_add(1);
    *generation
}

async fn queue_or_send_frame(session: &RdpSession, frame: Vec<u8>) {
    let frame = {
        let channel = session.frame_channel.lock().await;
        if let Some(sender) = channel.as_ref() {
            let _ = sender.send(InvokeResponseBody::Raw(frame));
            return;
        }
        frame
    };

    {
        let mut pending = session.pending_frames.lock().await;
        while pending.len() >= MAX_FRAME_QUEUE {
            pending.pop_front();
        }
        pending.push_back(frame);
    }
}

async fn flush_pending_frames(session: &RdpSession) {
    let channel = session.frame_channel.lock().await;
    let Some(channel) = channel.as_ref() else {
        return;
    };
    let mut pending = session.pending_frames.lock().await;
    while let Some(frame) = pending.pop_front() {
        if channel.send(InvokeResponseBody::Raw(frame)).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> RdpConnectConfig {
        RdpConnectConfig {
            session_id: "s".to_string(),
            connection_id: "c".to_string(),
            name: "RDP".to_string(),
            host: "127.0.0.1".to_string(),
            port: 3389,
            username: "user".to_string(),
            domain: String::new(),
            password: None,
            width: 640,
            height: 480,
            display_mode: "fit-window".to_string(),
            use_nla: true,
            certificate_policy: "prompt".to_string(),
            clipboard_mode: "text-only".to_string(),
            reconnect_enabled: true,
            reconnect_max_attempts: 3,
            color_depth: 32,
            admin: false,
            network: None,
        }
    }

    fn test_session() -> Arc<RdpSession> {
        Arc::new(RdpSession {
            config: test_config(),
            state: Mutex::new(RdpSessionState::Connecting),
            message: Mutex::new(None),
            error_kind: Mutex::new(None),
            generation: Mutex::new(0),
            frame_channel: Mutex::new(None),
            pending_frames: Mutex::new(VecDeque::with_capacity(MAX_FRAME_QUEUE)),
            input_sender: Mutex::new(None),
            input_database: Mutex::new(IronRdpInputDatabase::new()),
            frame_sequence: Mutex::new(0),
            worker: Mutex::new(None),
            reconnect_attempts: Mutex::new(0),
            reconnect_task: Mutex::new(None),
            clipboard_bridge: Mutex::new(None),
            close_requested: AtomicBool::new(false),
            pending_certificates: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    #[test]
    fn ironrdp_image_patch_has_expected_header_and_payload() {
        let pixels = [0x0011_2233, 0x0044_5566, 0x0077_8899, 0x00aa_bbcc];
        let frame = build_frame_from_ironrdp_image(&pixels, 1920, 1080, 10, 20, 2, 2, 9)
            .expect("valid RDP patch should encode");

        assert_eq!(&frame[0..8], &9_u64.to_le_bytes());
        assert_eq!(&frame[8..12], &1920_u32.to_le_bytes());
        assert_eq!(&frame[12..16], &1080_u32.to_le_bytes());
        assert_eq!(&frame[16..20], &10_u32.to_le_bytes());
        assert_eq!(&frame[20..24], &20_u32.to_le_bytes());
        assert_eq!(&frame[24..28], &2_u32.to_le_bytes());
        assert_eq!(&frame[28..32], &2_u32.to_le_bytes());
        assert_eq!(&frame[32..36], &8_u32.to_le_bytes());
        assert_eq!(
            &frame[36..40],
            &crate::core::remote_desktop::frame::PIXEL_FORMAT_RGBA8888.to_le_bytes()
        );
        assert_eq!(&frame[40..44], &16_u32.to_le_bytes());
        assert_eq!(
            &frame[44..],
            &[
                0x11, 0x22, 0x33, 0xff, 0x44, 0x55, 0x66, 0xff, 0x77, 0x88, 0x99, 0xff, 0xaa, 0xbb,
                0xcc, 0xff
            ]
        );
    }

    #[tokio::test]
    async fn pending_frame_queue_keeps_latest_frames_under_pressure() {
        let session = test_session();

        queue_or_send_frame(&session, vec![1]).await;
        queue_or_send_frame(&session, vec![2]).await;
        queue_or_send_frame(&session, vec![3]).await;

        let pending = session.pending_frames.lock().await;
        assert_eq!(pending.len(), MAX_FRAME_QUEUE);
        assert_eq!(pending[0], vec![2]);
        assert_eq!(pending[1], vec![3]);
    }

    #[test]
    fn certificate_policy_handles_strict_and_prompt_decisions() {
        use crate::storage::KnownHostCheck;

        assert_eq!(
            certificate_policy_allows_without_prompt("strict", KnownHostCheck::Match),
            Ok(true)
        );
        assert!(
            certificate_policy_allows_without_prompt("strict", KnownHostCheck::UnknownHost)
                .is_err()
        );
        assert!(
            certificate_policy_allows_without_prompt("strict", KnownHostCheck::HostSeen).is_err()
        );
        assert_eq!(
            certificate_policy_allows_without_prompt("prompt", KnownHostCheck::Match),
            Ok(true)
        );
        assert_eq!(
            certificate_policy_allows_without_prompt("prompt", KnownHostCheck::UnknownHost),
            Ok(false)
        );
        assert_eq!(
            certificate_policy_allows_without_prompt(
                "accept-temporarily",
                KnownHostCheck::HostSeen
            ),
            Ok(true)
        );
    }

    #[test]
    fn rdp_tls_backend_label_matches_target() {
        #[cfg(windows)]
        assert_eq!(rdp_tls_backend_label(), "native-tls (Schannel)");
        #[cfg(not(windows))]
        assert_eq!(rdp_tls_backend_label(), "rustls");
    }

    #[test]
    fn native_tls_vendor_keeps_certificate_decision_with_nyaterm() {
        let native_tls_backend = include_str!("../../vendor/ironrdp-tls/src/native_tls.rs");

        assert!(native_tls_backend.contains(".danger_accept_invalid_certs(true)"));
        assert!(native_tls_backend.contains(".danger_accept_invalid_hostnames(true)"));
        assert!(native_tls_backend.contains(".use_sni(false)"));
    }

    #[test]
    fn connector_classifier_treats_tls_upgrade_reset_as_tls() {
        let text = r#"error { context: "tlsupgrade", kind: custom, source: some(os {
            code: 10054,
            kind: connectionreset,
            message: "remote host reset the connection"
        })) }"#;

        assert_eq!(
            classify_connector_error_text(text),
            (RdpErrorKind::Tls, true)
        );
    }

    #[test]
    fn classifier_keeps_nyaterm_certificate_rejections_as_certificate() {
        assert_eq!(
            classify_connector_error_text("tlsupgrade failed: certificate rejected"),
            (RdpErrorKind::Certificate, false)
        );
        assert_eq!(
            classify_connector_error_text("certificate fingerprint changed"),
            (RdpErrorKind::Certificate, false)
        );
    }

    #[test]
    fn session_classifier_treats_handshake_failures_as_tls() {
        assert_eq!(
            classify_session_error(&"native-tls Schannel handshake failure"),
            (RdpErrorKind::Tls, true)
        );
    }

    #[test]
    fn clipboard_limit_rejects_oversized_text() {
        let oversized = "x".repeat(MAX_CLIPBOARD_TEXT_BYTES + 1);
        assert!(clipboard_text_within_limit(""));
        assert!(clipboard_text_within_limit("hello"));
        assert!(!clipboard_text_within_limit(&oversized));
    }

    #[test]
    fn clipboard_hash_supports_loop_prevention_tokens() {
        let bridge = RdpClipboardBridge::new("s".to_string());
        bridge.mark_text_written_from_remote("same");

        let current = bridge.last_text_hash.lock().unwrap();
        assert_eq!(*current, Some(stable_text_hash("same")));
        assert_ne!(*current, Some(stable_text_hash("different")));
    }

    #[test]
    fn rdp_input_event_accepts_frontend_camel_case_fields() {
        let key_event: RdpInputEvent = serde_json::from_value(serde_json::json!({
            "type": "key-down",
            "scanCode": 29,
            "extended": false,
            "repeat": false
        }))
        .unwrap();
        match key_event {
            RdpInputEvent::KeyDown {
                scan_code,
                extended,
                repeat,
            } => {
                assert_eq!(scan_code, 29);
                assert!(!extended);
                assert!(!repeat);
            }
            _ => panic!("expected key-down event"),
        }

        let wheel_event: RdpInputEvent = serde_json::from_value(serde_json::json!({
            "type": "mouse-wheel",
            "deltaX": 1.0,
            "deltaY": -2.0,
            "x": 10,
            "y": 20
        }))
        .unwrap();
        match wheel_event {
            RdpInputEvent::MouseWheel {
                delta_x,
                delta_y,
                x,
                y,
            } => {
                assert_eq!(delta_x, 1.0);
                assert_eq!(delta_y, -2.0);
                assert_eq!(x, 10);
                assert_eq!(y, 20);
            }
            _ => panic!("expected mouse-wheel event"),
        }
    }

    #[test]
    fn right_shift_uses_direct_fast_path_keyboard_event() {
        assert_eq!(
            rdp_input_to_direct_fast_path(RdpInputEvent::KeyDown {
                scan_code: RDP_RIGHT_SHIFT_SCAN_CODE,
                extended: false,
                repeat: false,
            }),
            Some(IronRdpFastPathInputEvent::KeyboardEvent(
                IronRdpKeyboardFlags::empty(),
                RDP_RIGHT_SHIFT_SCAN_CODE as u8,
            ))
        );
        assert_eq!(
            rdp_input_to_direct_fast_path(RdpInputEvent::KeyUp {
                scan_code: RDP_RIGHT_SHIFT_SCAN_CODE,
                extended: false,
                repeat: false,
            }),
            Some(IronRdpFastPathInputEvent::KeyboardEvent(
                IronRdpKeyboardFlags::RELEASE,
                RDP_RIGHT_SHIFT_SCAN_CODE as u8,
            ))
        );
    }

    #[test]
    fn left_shift_stays_on_database_input_path() {
        let operations = rdp_input_to_operations(RdpInputEvent::KeyDown {
            scan_code: 0x2a,
            extended: false,
            repeat: false,
        })
        .unwrap();

        assert_eq!(operations.len(), 1);
        assert!(matches!(
            operations.first(),
            Some(IronRdpInputOperation::KeyPressed(_))
        ));
        assert_eq!(
            rdp_input_to_direct_fast_path(RdpInputEvent::KeyDown {
                scan_code: 0x2a,
                extended: false,
                repeat: false,
            }),
            None
        );
    }

    #[test]
    fn browser_wheel_delta_is_inverted_for_rdp_rotation_units() {
        let operations = rdp_input_to_operations(RdpInputEvent::MouseWheel {
            delta_x: 3.0,
            delta_y: 120.0,
            x: 10,
            y: 20,
        })
        .unwrap();

        assert!(matches!(
            operations.first(),
            Some(IronRdpInputOperation::MouseMove(_))
        ));
        let horizontal = operations
            .iter()
            .find_map(|operation| match operation {
                IronRdpInputOperation::WheelRotations(rotations) if !rotations.is_vertical => {
                    Some(rotations.rotation_units)
                }
                _ => None,
            })
            .expect("expected horizontal wheel operation");
        let vertical = operations
            .iter()
            .find_map(|operation| match operation {
                IronRdpInputOperation::WheelRotations(rotations) if rotations.is_vertical => {
                    Some(rotations.rotation_units)
                }
                _ => None,
            })
            .expect("expected vertical wheel operation");

        assert_eq!(horizontal, -3);
        assert_eq!(vertical, -120);
    }

    #[tokio::test]
    async fn stale_certificate_response_is_ignored() {
        let manager = RdpSessionManager::with_engine(Arc::new(NoopRdpEngine));
        let session = test_session();
        *session.generation.lock().await = 2;
        manager
            .sessions
            .lock()
            .await
            .insert("s".to_string(), session.clone());
        let (tx, mut rx) = oneshot::channel();
        manager.pending_certificates.lock().await.insert(
            "req".to_string(),
            RdpCertificatePending {
                session_id: "s".to_string(),
                generation: 1,
                responder: tx,
            },
        );

        manager
            .respond_certificate("req", true, true)
            .await
            .unwrap();

        assert!(rx.try_recv().is_err());
        assert!(manager.pending_certificates.lock().await.is_empty());
    }

    #[test]
    fn reconnect_delay_caps_after_sixth_attempt() {
        let sixth = reconnect_delay(6);
        let later = reconnect_delay(20);
        assert!(sixth >= Duration::from_millis(30_000));
        assert!(sixth <= Duration::from_millis(30_249));
        assert!(later >= Duration::from_millis(30_000));
        assert!(later <= Duration::from_millis(30_249));
    }

    struct NoopRdpEngine;

    #[async_trait]
    impl RdpEngine for NoopRdpEngine {
        async fn connect(&self, _app: AppHandle, _session: Arc<RdpSession>, _generation: u64) {}

        async fn send_input(
            &self,
            _session: Arc<RdpSession>,
            _events: Vec<RdpInputEvent>,
        ) -> AppResult<()> {
            Ok(())
        }

        async fn resize(
            &self,
            _session: Arc<RdpSession>,
            _width: u32,
            _height: u32,
        ) -> AppResult<()> {
            Ok(())
        }

        async fn set_clipboard_text(
            &self,
            _session: Arc<RdpSession>,
            _text: String,
        ) -> AppResult<()> {
            Ok(())
        }

        async fn close(&self, _session: Arc<RdpSession>) -> AppResult<()> {
            Ok(())
        }
    }
}
