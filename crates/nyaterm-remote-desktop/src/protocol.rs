use std::fmt;
use std::sync::Arc;

use nyaterm_core::SecretString;
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 12;

/// Maximum UTF-8 payload accepted for a single committed-text event.
///
/// This intentionally leaves ample room below the control-packet envelope for
/// JSON framing and other events in the same input batch.
pub const MAX_COMMITTED_TEXT_BYTES: usize = 64 * 1024;

/// Desktop extent the protocol will negotiate, in pixels. Shared by
/// `RdpDisplayMetrics::clamped` and the connect/resize validation so a request
/// the application is allowed to make is never rejected at the session layer.
pub(crate) const MIN_WIDTH: u32 = 200;
pub(crate) const MIN_HEIGHT: u32 = 200;
pub(crate) const MAX_WIDTH: u32 = 8192;
pub(crate) const MAX_HEIGHT: u32 = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CommittedTextError {
    #[error("committed text exceeds the {MAX_COMMITTED_TEXT_BYTES}-byte limit")]
    TooLong,
    #[error("committed text contains NUL")]
    ContainsNul,
    #[error("committed text contains a control character that must be sent as a key event")]
    ContainsControl,
}

/// Validate an entire committed-text payload before it is put on the wire.
///
/// Committed text is Unicode text produced by an IME or text service. Control
/// characters are deliberately excluded: Enter, Tab, Escape, and similar input
/// must retain their protocol-specific key semantics instead of being smuggled
/// through a text event.
pub fn validate_committed_text(text: &str) -> Result<(), CommittedTextError> {
    if text.len() > MAX_COMMITTED_TEXT_BYTES {
        return Err(CommittedTextError::TooLong);
    }
    if text.contains('\0') {
        return Err(CommittedTextError::ContainsNul);
    }
    if text.chars().any(char::is_control) {
        return Err(CommittedTextError::ContainsControl);
    }
    Ok(())
}

/// Static features implemented by an RDP helper for this IPC version.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdpServerCapabilities {
    pub committed_unicode_text: bool,
    pub secure_attention: bool,
}

/// Static features implemented by a VNC helper for this IPC version.
///
/// Secure Attention is intentionally absent: VNC has no SAS wire operation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VncServerCapabilities {
    pub committed_unicode_keysyms: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RdpBackend {
    #[default]
    #[serde(alias = "auto", alias = "internal")]
    Builtin,
    #[serde(alias = "active-x")]
    Windows,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdpSessionConfig {
    #[serde(default)]
    pub relay: Option<nyaterm_core::connection_route::RelayEndpoint>,
    #[serde(default)]
    pub backend: RdpBackend,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows_authentication: Option<String>,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub domain: String,
    pub password: Option<SecretString>,
    pub use_nla: bool,
    pub certificate_policy: RdpCertificatePolicy,
    /// Request an administrative session; assignment remains server-controlled.
    #[serde(default)]
    pub administrative_session: bool,
    pub display: RdpDisplayConfig,
    pub clipboard: RdpClipboardConfig,
    pub reconnect: RdpReconnectConfig,
}

impl fmt::Debug for RdpSessionConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RdpSessionConfig")
            .field("backend", &self.backend)
            .field("name", &self.name)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("domain", &self.domain)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .field("use_nla", &self.use_nla)
            .field("certificate_policy", &self.certificate_policy)
            .field("administrative_session", &self.administrative_session)
            .field("display", &self.display)
            .field("clipboard", &self.clipboard)
            .field("reconnect", &self.reconnect)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RdpCertificatePolicy {
    #[default]
    Prompt,
    TrustOnFirstUse,
    Strict,
    #[serde(alias = "accept-temporarily")]
    Insecure,
    #[serde(alias = "reject_changed")]
    RejectChanged,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdpDisplayConfig {
    pub mode: RdpDisplayMode,
    pub width: u32,
    pub height: u32,
    /// Desktop scale factor in percent (100..=500) negotiated with the initial
    /// desktop size, or 0 to leave the choice to the server.
    ///
    /// This is ephemeral client display information, like
    /// [`RdpDisplayMetrics`]: it is filled in from the panel that hosts the
    /// session and is never taken from a saved connection. Asking for the scale
    /// in the same handshake as the size is what keeps a fit-window session
    /// from changing resolution again right after it connects.
    #[serde(default)]
    pub desktop_scale_factor: u32,
    pub color_depth: u8,
    /// RDPEGFX surface updates. See `nyaterm_core::RdpDisplaySettings`.
    #[serde(default)]
    pub graphics_pipeline: bool,
    #[serde(default = "rdp_default_true")]
    pub display_wallpaper: bool,
    #[serde(default = "rdp_default_true")]
    pub display_themes: bool,
    #[serde(default = "rdp_default_true")]
    pub enable_font_smoothing: bool,
    #[serde(default)]
    pub enable_desktop_composition: bool,
    /// Inverted: true asks the server to turn the effect off.
    #[serde(default = "rdp_default_true")]
    pub disable_full_window_drag: bool,
    #[serde(default = "rdp_default_true")]
    pub disable_menu_animations: bool,
    #[serde(default)]
    pub disable_cursor_shadow: bool,
    #[serde(default)]
    pub disable_cursor_blinking: bool,
}

/// Ephemeral client display information sent to the helper. This is not part
/// of persisted connection configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdpDisplayMetrics {
    pub width: u32,
    pub height: u32,
    pub desktop_scale_factor: u32,
    pub physical_size_mm: Option<(u32, u32)>,
}

impl RdpDisplayMetrics {
    /// Bring a panel-derived request into the range the protocol accepts.
    ///
    /// Even boundaries because the legacy desktop size fields are `u16` pixels
    /// and the server rounds the framebuffer to them; a request that is not
    /// even comes back as a different size, which looks like a resize stutter.
    pub fn clamped(self) -> Self {
        Self {
            width: self.width.clamp(MIN_WIDTH, MAX_WIDTH) & !1,
            height: self.height.clamp(MIN_HEIGHT, MAX_HEIGHT) & !1,
            desktop_scale_factor: self.desktop_scale_factor.clamp(100, 500),
            physical_size_mm: self.physical_size_mm,
        }
    }
}

fn rdp_default_true() -> bool {
    true
}

impl Default for RdpDisplayConfig {
    fn default() -> Self {
        Self {
            mode: RdpDisplayMode::FitWindow,
            width: 1920,
            height: 1080,
            desktop_scale_factor: 0,
            color_depth: 32,
            graphics_pipeline: false,
            display_wallpaper: true,
            display_themes: true,
            enable_font_smoothing: true,
            enable_desktop_composition: false,
            disable_full_window_drag: true,
            disable_menu_animations: true,
            disable_cursor_shadow: false,
            disable_cursor_blinking: false,
        }
    }
}

/// Build the helper's display configuration from a saved connection.
///
/// Both launch paths (opening a connection and restoring one at startup) go
/// through here, so a setting can never be honoured when opening a tab and
/// ignored after a restart.
impl From<&nyaterm_core::RdpDisplaySettings> for RdpDisplayConfig {
    fn from(settings: &nyaterm_core::RdpDisplaySettings) -> Self {
        Self {
            mode: parse_rdp_display_mode(&settings.mode),
            width: settings.width,
            height: settings.height,
            desktop_scale_factor: 0,
            color_depth: settings.color_depth,
            graphics_pipeline: settings.graphics_pipeline,
            display_wallpaper: settings.display_wallpaper,
            display_themes: settings.display_themes,
            enable_font_smoothing: settings.enable_font_smoothing,
            enable_desktop_composition: settings.enable_desktop_composition,
            disable_full_window_drag: settings.disable_full_window_drag,
            disable_menu_animations: settings.disable_menu_animations,
            disable_cursor_shadow: settings.disable_cursor_shadow,
            disable_cursor_blinking: settings.disable_cursor_blinking,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RdpDisplayMode {
    #[default]
    FitWindow,
    Fixed,
}

impl RdpDisplayConfig {
    /// Ask the server for exactly the desktop the hosting panel will draw into.
    ///
    /// A fit-window session that connects at the saved size and is resized to
    /// the panel afterwards shows the desktop twice at two different scales in
    /// the time it takes to connect; the panel is measurable before the connect
    /// is issued, so the first negotiation should already use it.
    pub fn fit_to_panel(&mut self, metrics: RdpDisplayMetrics) {
        let metrics = metrics.clamped();
        self.width = metrics.width;
        self.height = metrics.height;
        self.desktop_scale_factor = metrics.desktop_scale_factor;
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdpClipboardConfig {
    pub mode: RdpClipboardMode,
}

impl Default for RdpClipboardConfig {
    fn default() -> Self {
        Self {
            mode: RdpClipboardMode::TextOnly,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RdpClipboardMode {
    Disabled,
    #[default]
    TextOnly,
    TextAndFiles,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RdpClipboardTransferStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdpClipboardTransferProgress {
    pub id: String,
    pub name: String,
    pub status: RdpClipboardTransferStatus,
    pub total_bytes: u64,
    pub transferred_bytes: u64,
    pub total_files: u64,
    pub completed_files: u64,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdpReconnectConfig {
    pub enabled: bool,
    pub max_attempts: u32,
}

impl Default for RdpReconnectConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_attempts: 5,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RdpInputEvent {
    KeyDown {
        scan_code: u16,
        extended: bool,
        repeat: bool,
    },
    KeyUp {
        scan_code: u16,
        extended: bool,
        repeat: bool,
    },
    Unicode {
        text: String,
    },
    Pointer(RemotePointerEvent),
    ReleaseAllInputs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePoint {
    pub x: u32,
    pub y: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemotePointerButton {
    Left,
    Middle,
    Right,
    X1,
    X2,
}

/// Compatibility name retained for desktop call sites while pointer input is
/// moved onto the protocol-neutral model.
pub type RdpPointerButton = RemotePointerButton;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemoteWheelAxis {
    Vertical,
    Horizontal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemotePointerEvent {
    Move {
        position: RemotePoint,
    },
    Button {
        position: RemotePoint,
        button: RemotePointerButton,
        pressed: bool,
    },
    Wheel {
        position: RemotePoint,
        axis: RemoteWheelAxis,
        rotation_units: i16,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PixelFormat {
    Bgra8,
    Rgba8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteFrameEvent {
    Reset {
        epoch: u64,
        width: u32,
        height: u32,
    },
    Bitmap {
        epoch: u64,
        full: bool,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        stride: u32,
        format: PixelFormat,
        pixels: crate::pixel_memory::PixelBuffer,
    },
}

/// Source-compatible alias for integrations migrating to the protocol-neutral name.
pub type RdpFrameEvent = RemoteFrameEvent;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorShape {
    pub shape_id: u64,
    pub width: u32,
    pub height: u32,
    pub hotspot: RemotePoint,
    pub pixels: crate::pixel_memory::PixelBuffer,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CursorPosition {
    pub x: u32,
    pub y: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorVisibility {
    pub visible: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteCursorEvent {
    Shape(CursorShape),
    Position(CursorPosition),
    Visibility(CursorVisibility),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RdpSessionState {
    Idle,
    Connecting,
    Connected,
    Reconnecting,
    Disconnecting,
    Disconnected,
    Failed(RdpError),
    #[deprecated(note = "use Connected")]
    Active,
    #[deprecated(note = "use Disconnected")]
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdpCertificateRequest {
    pub request_id: String,
    pub host: String,
    pub port: u16,
    pub sha256_fingerprint: String,
    pub subject: Option<String>,
    pub issuer: Option<String>,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RdpCertificateResponse {
    Reject,
    TrustOnce,
    TrustAndRemember,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RdpCapability {
    DynamicResizeUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RdpErrorKind {
    ResourceExhausted,
    Authentication,
    CertificateRejected,
    Timeout,
    ConnectionRefused,
    Tls,
    Transport,
    /// The live session ended: the server stated a reason, or the session broke in a way
    /// the helper could not attribute to the transport.
    ///
    /// Deliberately not auto-reconnectable. A server that ended a session for a reason --
    /// most often a takeover by another connection -- ends the replacement the same way,
    /// so two NyaTerm clients chasing each other leaves neither usable.
    Session,
    Clipboard,
    Negotiation,
    HelperMissing,
    HelperCrashed,
    Ipc,
    Protocol,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{kind:?}: {message}")]
pub struct RdpError {
    pub kind: RdpErrorKind,
    pub message: String,
}

impl RdpError {
    pub fn new(kind: RdpErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RdpRuntimeEvent {
    State {
        session_id: String,
        state: RdpSessionState,
        message: Option<String>,
    },
    Frame {
        session_id: String,
        event: RemoteFrameEvent,
    },
    Cursor {
        session_id: String,
        event: RemoteCursorEvent,
    },
    Clipboard {
        session_id: String,
        text: String,
        generation: u64,
    },
    ClipboardTransfer {
        session_id: String,
        progress: RdpClipboardTransferProgress,
    },
    CertificateRequest(RdpCertificateRequest),
    Capability {
        session_id: String,
        capability: RdpCapability,
    },
    Error {
        session_id: String,
        error: RdpError,
        fatal: bool,
    },
}

/// Called after an event is enqueued on a session queue, so the consumer can be
/// woken instead of polling for it.
///
/// A callback rather than a channel keeps this crate free of any async runtime;
/// the bounded session queues use a condition variable for producer backpressure
/// and invoke this callback only to schedule a consumer drain.
/// The application supplies a closure that signals its own wake.
///
/// # Contract
///
/// The waker is invoked while the session queue lock is held, so it must not
/// block, must not re-enter the session manager, and must not panic. Signalling
/// an atomic and posting a lightweight GPUI wake event is the intended shape.
pub type QueueWaker = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RdpSessionDrain {
    pub control: Vec<RdpRuntimeEvent>,
    pub frames: Vec<RemoteFrameEvent>,
    pub cursors: Vec<RemoteCursorEvent>,
}

pub const MAX_VNC_CLIPBOARD_TEXT_BYTES: usize = 1024 * 1024;
pub const MAX_VNC_INPUT_BATCH: usize = 256;
pub const MAX_VNC_FRAMEBUFFER_WIDTH: u32 = 7680;
pub const MAX_VNC_FRAMEBUFFER_HEIGHT: u32 = 4320;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VncSessionConfig {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub relay: Option<nyaterm_core::connection_route::RelayEndpoint>,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub password: Option<SecretString>,
    pub security: VncSecurityConfig,
    pub display: VncDisplayConfig,
    pub clipboard: VncClipboardConfig,
    pub reconnect: VncReconnectConfig,
    pub shared: bool,
    pub view_only: bool,
}

impl fmt::Debug for VncSessionConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VncSessionConfig")
            .field("name", &self.name)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .field("security", &self.security)
            .field("display", &self.display)
            .field("clipboard", &self.clipboard)
            .field("reconnect", &self.reconnect)
            .field("shared", &self.shared)
            .field("view_only", &self.view_only)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VncSecurityConfig {
    pub mode: VncSecurityMode,
}

impl Default for VncSecurityConfig {
    fn default() -> Self {
        Self {
            mode: VncSecurityMode::Auto,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VncSecurityMode {
    #[default]
    Auto,
    None,
    #[serde(alias = "vnc-auth")]
    VncAuth,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VncDisplayConfig {
    pub scale_mode: VncScaleMode,
}

impl Default for VncDisplayConfig {
    fn default() -> Self {
        Self {
            scale_mode: VncScaleMode::Fit,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VncScaleMode {
    #[default]
    Fit,
    Stretch,
    Actual,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VncClipboardConfig {
    pub enabled: bool,
}

impl Default for VncClipboardConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VncReconnectConfig {
    pub enabled: bool,
    pub max_attempts: u32,
}

impl Default for VncReconnectConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_attempts: 5,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VncInputEvent {
    Key {
        keysym: u32,
        pressed: bool,
    },
    /// Unicode text committed by an IME or text service. The helper translates
    /// each scalar to its VNC Unicode keysym without synthesizing SAS.
    Text {
        text: String,
    },
    Pointer(RemotePointerEvent),
    ReleaseAllInputs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VncSessionState {
    Connecting,
    Authenticating,
    Negotiating,
    Connected,
    Reconnecting,
    Disconnecting,
    Disconnected,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VncErrorKind {
    Timeout,
    ResourceExhausted,
    Transport,
    Authentication,
    Protocol,
    Encoding,
    Clipboard,
    Internal,
    /// The helper binary was not found beside the application.
    HelperMissing,
    /// The helper exited or panicked while a session was live.
    HelperCrashed,
    /// The helper IPC channel itself failed.
    Ipc,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{kind:?}: {message}")]
pub struct VncError {
    pub kind: VncErrorKind,
    pub message: String,
    /// Set only when the helper's effective credentials are missing. Other
    /// authentication failures cannot be resolved by asking for a password.
    /// Older helpers omit this hint and must never trigger an automatic prompt.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub password_required: bool,
}

impl VncError {
    pub fn new(kind: VncErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            password_required: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteDesktopViewState {
    Idle,
    Connecting,
    Connected,
    Reconnecting,
    Disconnecting,
    Disconnected,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteDesktopErrorCategory {
    ResourceExhausted,
    Authentication,
    Certificate,
    Timeout,
    ConnectionRefused,
    Security,
    Transport,
    Session,
    Clipboard,
    Negotiation,
    Encoding,
    HelperMissing,
    HelperCrashed,
    Ipc,
    Protocol,
    Unsupported,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{category:?}: {message}")]
pub struct RemoteDesktopError {
    pub category: RemoteDesktopErrorCategory,
    pub message: String,
}

impl From<RdpError> for RemoteDesktopError {
    fn from(error: RdpError) -> Self {
        let category = match error.kind {
            RdpErrorKind::Authentication => RemoteDesktopErrorCategory::Authentication,
            RdpErrorKind::CertificateRejected => RemoteDesktopErrorCategory::Certificate,
            RdpErrorKind::Timeout => RemoteDesktopErrorCategory::Timeout,
            RdpErrorKind::ConnectionRefused => RemoteDesktopErrorCategory::ConnectionRefused,
            RdpErrorKind::Tls => RemoteDesktopErrorCategory::Security,
            RdpErrorKind::Transport => RemoteDesktopErrorCategory::Transport,
            RdpErrorKind::Session => RemoteDesktopErrorCategory::Session,
            RdpErrorKind::Clipboard => RemoteDesktopErrorCategory::Clipboard,
            RdpErrorKind::Negotiation => RemoteDesktopErrorCategory::Negotiation,
            RdpErrorKind::HelperMissing => RemoteDesktopErrorCategory::HelperMissing,
            RdpErrorKind::HelperCrashed => RemoteDesktopErrorCategory::HelperCrashed,
            RdpErrorKind::Ipc => RemoteDesktopErrorCategory::Ipc,
            RdpErrorKind::Protocol => RemoteDesktopErrorCategory::Protocol,
            RdpErrorKind::ResourceExhausted => RemoteDesktopErrorCategory::ResourceExhausted,
            RdpErrorKind::Unsupported => RemoteDesktopErrorCategory::Unsupported,
        };
        Self {
            category,
            message: error.message,
        }
    }
}

impl From<VncError> for RemoteDesktopError {
    fn from(error: VncError) -> Self {
        let category = match error.kind {
            VncErrorKind::Authentication => RemoteDesktopErrorCategory::Authentication,
            VncErrorKind::Clipboard => RemoteDesktopErrorCategory::Clipboard,
            VncErrorKind::Transport => RemoteDesktopErrorCategory::Transport,
            VncErrorKind::Encoding => RemoteDesktopErrorCategory::Encoding,
            VncErrorKind::Protocol => RemoteDesktopErrorCategory::Protocol,
            VncErrorKind::Internal => RemoteDesktopErrorCategory::Internal,
            VncErrorKind::HelperMissing => RemoteDesktopErrorCategory::HelperMissing,
            VncErrorKind::HelperCrashed => RemoteDesktopErrorCategory::HelperCrashed,
            VncErrorKind::Timeout => RemoteDesktopErrorCategory::Timeout,
            VncErrorKind::ResourceExhausted => RemoteDesktopErrorCategory::ResourceExhausted,
            VncErrorKind::Ipc => RemoteDesktopErrorCategory::Ipc,
        };
        Self {
            category,
            message: error.message,
        }
    }
}

impl From<&RdpSessionState> for RemoteDesktopViewState {
    fn from(state: &RdpSessionState) -> Self {
        match state {
            RdpSessionState::Idle => Self::Idle,
            RdpSessionState::Connecting => Self::Connecting,
            RdpSessionState::Connected => Self::Connected,
            RdpSessionState::Reconnecting => Self::Reconnecting,
            RdpSessionState::Disconnecting => Self::Disconnecting,
            RdpSessionState::Disconnected => Self::Disconnected,
            RdpSessionState::Failed(_) => Self::Failed,
            #[allow(deprecated)]
            RdpSessionState::Active => Self::Connected,
            #[allow(deprecated)]
            RdpSessionState::Closed => Self::Disconnected,
        }
    }
}

impl From<&VncSessionState> for RemoteDesktopViewState {
    fn from(state: &VncSessionState) -> Self {
        match state {
            VncSessionState::Connecting
            | VncSessionState::Authenticating
            | VncSessionState::Negotiating => Self::Connecting,
            VncSessionState::Connected => Self::Connected,
            VncSessionState::Reconnecting => Self::Reconnecting,
            VncSessionState::Disconnecting => Self::Disconnecting,
            VncSessionState::Disconnected => Self::Disconnected,
            VncSessionState::Failed => Self::Failed,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VncServerKeyRequest {
    pub session_id: String,
    pub generation: u64,
    pub request_id: String,
    pub host: String,
    pub port: u16,
    pub sha256_fingerprint: String,
    pub key_bits: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VncRuntimeEvent {
    ServerKeyRequest(VncServerKeyRequest),
    ServerKeyAuthenticated(VncServerKeyRequest),
    State {
        session_id: String,
        state: VncSessionState,
        message: Option<String>,
    },
    Frame {
        session_id: String,
        event: RemoteFrameEvent,
    },
    Clipboard {
        session_id: String,
        text: String,
    },
    Error {
        session_id: String,
        error: VncError,
        fatal: bool,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VncSessionDrain {
    pub control: Vec<VncRuntimeEvent>,
    pub frames: Vec<RemoteFrameEvent>,
    pub cursors: Vec<RemoteCursorEvent>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RdpControlMessage {
    Ping {
        sequence: u64,
    },
    Pong {
        sequence: u64,
    },
    ClientHello {
        version: u32,
    },
    ServerHello {
        version: u32,
        capabilities: RdpServerCapabilities,
    },
    Connect {
        session_id: String,
        config: RdpSessionConfig,
    },
    DesktopReset {
        session_id: String,
        epoch: u64,
        width: u32,
        height: u32,
    },
    State {
        session_id: String,
        state: RdpSessionState,
        message: Option<String>,
    },
    Input {
        session_id: String,
        events: Vec<RdpInputEvent>,
    },
    /// Ask the RDP helper to invoke the remote Secure Attention sequence.
    ///
    /// This is deliberately not represented as synthetic Ctrl+Alt+Delete key
    /// events: an RDP server treats SAS as a distinct protocol operation.
    SecureAttention {
        session_id: String,
    },
    Resize {
        session_id: String,
        metrics: RdpDisplayMetrics,
    },
    Clipboard {
        session_id: String,
        text: String,
        generation: u64,
    },
    ClipboardTransfer {
        session_id: String,
        progress: RdpClipboardTransferProgress,
    },
    ClipboardTransferCancel {
        session_id: String,
        transfer_id: String,
    },
    CertificateRequest(RdpCertificateRequest),
    CertificateResponse {
        request_id: String,
        response: RdpCertificateResponse,
    },
    Capability {
        session_id: String,
        capability: RdpCapability,
    },
    Error {
        session_id: String,
        error: RdpError,
        fatal: bool,
    },
    RequestFullFrame {
        session_id: String,
    },
    Disconnect {
        session_id: String,
    },
}

/// Control vocabulary spoken between the application and `nyaterm-vnc-helper`.
///
/// Deliberately narrower than [`RdpControlMessage`]: this VNC path has no dynamic
/// resize, no TLS certificate prompt, and no capability negotiation. Framebuffer
/// and cursor payloads reuse the protocol-neutral binary packets in `crate::ipc`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum VncControlMessage {
    Ping {
        sequence: u64,
    },
    Pong {
        sequence: u64,
    },
    ServerKeyRequest(VncServerKeyRequest),
    ServerKeyAuthenticated(VncServerKeyRequest),
    ServerKeyResponse {
        session_id: String,
        generation: u64,
        request_id: String,
        accept: bool,
    },
    ClientHello {
        version: u32,
    },
    ServerHello {
        version: u32,
        capabilities: VncServerCapabilities,
    },
    Connect {
        session_id: String,
        config: VncSessionConfig,
    },
    /// The helper owns the epoch counter and stamps every frame with it.
    DesktopReset {
        session_id: String,
        epoch: u64,
        width: u32,
        height: u32,
    },
    State {
        session_id: String,
        state: VncSessionState,
        message: Option<String>,
    },
    Input {
        session_id: String,
        events: Vec<VncInputEvent>,
    },
    Clipboard {
        session_id: String,
        text: String,
    },
    Error {
        session_id: String,
        error: VncError,
        fatal: bool,
    },
    RequestFullFrame {
        session_id: String,
    },
    Disconnect {
        session_id: String,
    },
}

pub fn parse_rdp_certificate_policy(value: &str) -> RdpCertificatePolicy {
    match value.trim() {
        "trust_on_first_use" | "trust-on-first-use" | "tofu" => {
            RdpCertificatePolicy::TrustOnFirstUse
        }
        "strict" | "reject_changed" | "reject-changed" => RdpCertificatePolicy::Strict,
        "insecure" | "accept-temporarily" => RdpCertificatePolicy::Insecure,
        _ => RdpCertificatePolicy::Prompt,
    }
}

pub fn parse_rdp_display_mode(value: &str) -> RdpDisplayMode {
    match value.trim() {
        "fixed" | "native" => RdpDisplayMode::Fixed,
        _ => RdpDisplayMode::FitWindow,
    }
}

pub fn parse_rdp_backend(value: &str) -> Result<RdpBackend, RdpError> {
    match value {
        "builtin" | "internal" | "auto" => Ok(RdpBackend::Builtin),
        "windows" | "active-x" => Ok(RdpBackend::Windows),
        _ => Err(RdpError::new(
            RdpErrorKind::Unsupported,
            "Unsupported RDP backend",
        )),
    }
}

impl RdpBackend {
    /// Check before resolving a saved proxy or jump host into a live Relay.
    pub fn validate_route(self, has_relay: bool) -> Result<(), RdpError> {
        if self == Self::Builtin {
            return Ok(());
        }
        if !cfg!(windows) {
            return Err(RdpError::new(
                RdpErrorKind::Unsupported,
                "ActiveX RDP is only available on Windows",
            ));
        }
        if has_relay {
            return Err(RdpError::new(
                RdpErrorKind::Unsupported,
                "ActiveX RDP does not support Relay",
            ));
        }
        Ok(())
    }

    pub fn validate_config(self, config: &RdpSessionConfig) -> Result<(), RdpError> {
        self.validate_route(config.relay.is_some())?;
        if self == Self::Builtin {
            return Ok(());
        }
        if config.windows_authentication.as_deref() != Some("system") {
            return Err(RdpError::new(
                RdpErrorKind::Unsupported,
                "Select Windows system authentication explicitly for ActiveX RDP",
            ));
        }
        if config.clipboard.mode == RdpClipboardMode::TextOnly {
            return Err(RdpError::new(
                RdpErrorKind::Unsupported,
                "ActiveX RDP does not support text-only clipboard; select disabled or full clipboard redirection",
            ));
        }
        if config.reconnect.max_attempts > 200 {
            return Err(RdpError::new(
                RdpErrorKind::Unsupported,
                "ActiveX reconnect attempts must be between 0 and 200",
            ));
        }
        Ok(())
    }
}

pub fn parse_rdp_clipboard_mode(value: &str) -> RdpClipboardMode {
    match value.trim() {
        "disabled" | "off" => RdpClipboardMode::Disabled,
        "text-and-files" => RdpClipboardMode::TextAndFiles,
        _ => RdpClipboardMode::TextOnly,
    }
}

pub fn parse_vnc_security_mode(value: &str) -> VncSecurityMode {
    match value.trim() {
        "none" => VncSecurityMode::None,
        "vnc-auth" | "vnc_auth" | "password" => VncSecurityMode::VncAuth,
        _ => VncSecurityMode::Auto,
    }
}

pub fn parse_vnc_scale_mode(value: &str) -> VncScaleMode {
    match value.trim() {
        "stretch" => VncScaleMode::Stretch,
        "actual" | "actual-size" => VncScaleMode::Actual,
        _ => VncScaleMode::Fit,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CommittedTextError, MAX_COMMITTED_TEXT_BYTES, MAX_WIDTH, MIN_HEIGHT, RdpBackend,
        RdpCertificatePolicy, RdpClipboardConfig, RdpClipboardMode, RdpDisplayConfig,
        RdpDisplayMetrics, RdpDisplayMode, RdpSessionConfig, parse_rdp_backend,
        parse_rdp_certificate_policy, parse_rdp_clipboard_mode, parse_rdp_display_mode,
        validate_committed_text,
    };

    #[test]
    fn vnc_password_hint_is_optional_and_defaults_to_no_prompt() {
        use super::{VncError, VncErrorKind};

        let old = serde_json::json!({"kind": "Authentication", "message": "failed"});
        let decoded: VncError = serde_json::from_value(old.clone()).unwrap();
        assert!(!decoded.password_required);
        assert_eq!(serde_json::to_value(&decoded).unwrap(), old);
        let missing = VncError {
            password_required: true,
            ..VncError::new(VncErrorKind::Authentication, "password required")
        };
        assert_eq!(
            serde_json::from_value::<VncError>(serde_json::to_value(&missing).unwrap()).unwrap(),
            missing
        );
    }

    #[test]
    fn panel_metrics_become_a_negotiable_desktop() {
        let metrics = RdpDisplayMetrics {
            width: 9_000,
            height: 7,
            desktop_scale_factor: 40,
            physical_size_mm: Some((310, 170)),
        }
        .clamped();
        assert_eq!((metrics.width, metrics.height), (MAX_WIDTH, MIN_HEIGHT));
        assert_eq!(metrics.desktop_scale_factor, 100);
        assert_eq!(metrics.physical_size_mm, Some((310, 170)));

        let mut display = RdpDisplayConfig::default();
        display.fit_to_panel(RdpDisplayMetrics {
            width: 1_281,
            height: 719,
            desktop_scale_factor: 125,
            physical_size_mm: None,
        });
        assert_eq!((display.width, display.height), (1_280, 718));
        assert_eq!(display.desktop_scale_factor, 125);
    }

    #[test]
    fn legacy_display_ipc_and_saved_defaults_agree() {
        let display: RdpDisplayConfig = serde_json::from_value(serde_json::json!({
            "mode": "fit_window", "width": 1920, "height": 1080, "color_depth": 32
        }))
        .unwrap();
        assert_eq!(display, RdpDisplayConfig::default());
        assert_eq!(
            display,
            RdpDisplayConfig::from(&nyaterm_core::RdpDisplaySettings::default())
        );
        let document = serde_json::json!({
            "name":"legacy", "host":"example.invalid", "port":3389,
            "username":"", "domain":"", "password":null, "use_nla":true,
            "certificate_policy":"prompt",
            "display":{"mode":"fit_window","width":1920,"height":1080,"color_depth":32},
            "clipboard":{"mode":"text_only"}, "reconnect":{"enabled":true,"max_attempts":5}
        });
        let session: RdpSessionConfig = serde_json::from_value(document).unwrap();
        assert!(!session.administrative_session);
        assert_eq!(session.backend, RdpBackend::Builtin);
        assert_eq!(session.display, display);
        let encoded = serde_json::to_value(&session).unwrap();
        assert_eq!(
            serde_json::from_value::<RdpSessionConfig>(encoded).unwrap(),
            session
        );
    }

    #[test]
    fn certificate_policy_accepts_public_and_legacy_names() {
        assert_eq!(
            parse_rdp_certificate_policy("accept-temporarily"),
            RdpCertificatePolicy::Insecure
        );
        assert_eq!(
            parse_rdp_certificate_policy("insecure"),
            RdpCertificatePolicy::Insecure
        );
        assert_eq!(
            parse_rdp_certificate_policy("tofu"),
            RdpCertificatePolicy::TrustOnFirstUse
        );
        assert_eq!(
            parse_rdp_certificate_policy("reject-changed"),
            RdpCertificatePolicy::Strict
        );
    }

    #[test]
    fn legacy_native_display_mode_is_fixed() {
        assert_eq!(parse_rdp_display_mode("native"), RdpDisplayMode::Fixed);
    }

    #[test]
    fn rdp_backend_accepts_public_and_legacy_names() {
        for name in ["auto", "builtin", "internal"] {
            assert_eq!(parse_rdp_backend(name).unwrap(), RdpBackend::Builtin);
        }
        for name in ["windows", "active-x"] {
            assert_eq!(parse_rdp_backend(name).unwrap(), RdpBackend::Windows);
        }
        assert!(parse_rdp_backend("unknown").is_err());
        assert_eq!(RdpBackend::default(), RdpBackend::Builtin);
    }

    #[test]
    fn clipboard_mode_keeps_legacy_values_and_defaults_to_text() {
        assert_eq!(
            parse_rdp_clipboard_mode("disabled"),
            RdpClipboardMode::Disabled
        );
        assert_eq!(
            parse_rdp_clipboard_mode("text-only"),
            RdpClipboardMode::TextOnly
        );
        assert_eq!(
            parse_rdp_clipboard_mode("text-and-files"),
            RdpClipboardMode::TextAndFiles
        );
        assert_eq!(
            parse_rdp_clipboard_mode("unknown"),
            RdpClipboardMode::TextOnly
        );
        assert_eq!(
            RdpClipboardConfig::default().mode,
            RdpClipboardMode::TextOnly
        );
        assert_eq!(
            serde_json::to_string(&RdpClipboardMode::TextAndFiles).unwrap(),
            "\"text_and_files\""
        );
    }

    #[test]
    fn committed_text_accepts_unicode_at_the_byte_limit() {
        let text = "界".repeat(MAX_COMMITTED_TEXT_BYTES / 3);
        assert!(text.len() <= MAX_COMMITTED_TEXT_BYTES);
        assert_eq!(validate_committed_text(&text), Ok(()));
    }

    #[test]
    fn committed_text_rejects_oversize_nul_and_control_characters() {
        assert_eq!(
            validate_committed_text(&"a".repeat(MAX_COMMITTED_TEXT_BYTES + 1)),
            Err(CommittedTextError::TooLong)
        );
        assert_eq!(
            validate_committed_text("before\0after"),
            Err(CommittedTextError::ContainsNul)
        );
        assert_eq!(
            validate_committed_text("line\nfeed"),
            Err(CommittedTextError::ContainsControl)
        );
    }
}
