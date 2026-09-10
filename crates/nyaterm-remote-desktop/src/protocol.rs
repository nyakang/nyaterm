use std::fmt;
use std::sync::Arc;

use nyaterm_core::SecretString;
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 7;

/// Maximum UTF-8 payload accepted for a single committed-text event.
///
/// This intentionally leaves ample room below the control-packet envelope for
/// JSON framing and other events in the same input batch.
pub const MAX_COMMITTED_TEXT_BYTES: usize = 64 * 1024;

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

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdpSessionConfig {
    #[serde(default)]
    pub relay: Option<nyaterm_core::connection_route::RelayEndpoint>,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub domain: String,
    pub password: Option<SecretString>,
    pub use_nla: bool,
    pub certificate_policy: RdpCertificatePolicy,
    pub display: RdpDisplayConfig,
    pub clipboard: RdpClipboardConfig,
    pub reconnect: RdpReconnectConfig,
}

impl fmt::Debug for RdpSessionConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RdpSessionConfig")
            .field("name", &self.name)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("domain", &self.domain)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .field("use_nla", &self.use_nla)
            .field("certificate_policy", &self.certificate_policy)
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
    pub color_depth: u8,
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

impl Default for RdpDisplayConfig {
    fn default() -> Self {
        Self {
            mode: RdpDisplayMode::FitWindow,
            width: 1920,
            height: 1080,
            color_depth: 32,
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
        pixels: Vec<u8>,
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
    pub pixels: Vec<u8>,
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
    Authentication,
    CertificateRejected,
    Timeout,
    ConnectionRefused,
    Tls,
    Transport,
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
}

impl VncError {
    pub fn new(kind: VncErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VncRuntimeEvent {
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

pub fn parse_rdp_clipboard_mode(value: &str) -> RdpClipboardMode {
    match value.trim() {
        "disabled" | "off" => RdpClipboardMode::Disabled,
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
        CommittedTextError, MAX_COMMITTED_TEXT_BYTES, RdpCertificatePolicy, RdpDisplayMode,
        parse_rdp_certificate_policy, parse_rdp_display_mode, validate_committed_text,
    };

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
