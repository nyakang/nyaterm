mod certificate;
mod clipboard;
mod frame;
mod helper_process;
mod input;
mod ipc;
mod protocol;
mod session;
mod vnc;

pub use certificate::{
    CertificateDecision, CertificateEvaluation, CertificateMatchState, CertificatePromptReason,
    evaluate_certificate, evaluate_certificate_match,
};
pub use clipboard::{ClipboardOrigin, ClipboardTracker, MAX_CLIPBOARD_TEXT_BYTES};
pub use frame::{
    DirtyRect, Framebuffer, FramebufferError, FramebufferLimits, RDP_FRAMEBUFFER_LIMITS,
    VNC_FRAMEBUFFER_LIMITS, merge_dirty_rects, validate_framebuffer_dimensions,
};
pub use input::{
    DisplayScaleMode, DisplayTransform, KeyMapper, LogicalPoint, LogicalRect, LogicalSize,
    RemoteKey,
};
pub use ipc::{
    CONTROL_PAYLOAD_LIMIT, CURSOR_PAYLOAD_LIMIT, FRAME_PAYLOAD_LIMIT, HEADER_LEN, Packet,
    PacketReader, PacketType, decode_control, decode_cursor_packet, decode_cursor_packet_owned,
    decode_frame_packet, decode_frame_packet_owned, decode_vnc_control, encode_control,
    encode_cursor_packet, encode_frame_packet, encode_frame_packet_owned, encode_vnc_control,
    read_packet, write_packet, write_packet_into,
};
pub use protocol::{
    CommittedTextError, CursorPosition, CursorShape, CursorVisibility, MAX_COMMITTED_TEXT_BYTES,
    MAX_VNC_CLIPBOARD_TEXT_BYTES, MAX_VNC_FRAMEBUFFER_HEIGHT, MAX_VNC_FRAMEBUFFER_WIDTH,
    MAX_VNC_INPUT_BATCH, PROTOCOL_VERSION, PixelFormat, QueueWaker, RdpCapability,
    RdpCertificatePolicy, RdpCertificateRequest, RdpCertificateResponse, RdpClipboardConfig,
    RdpClipboardMode, RdpClipboardTransferProgress, RdpClipboardTransferStatus, RdpControlMessage,
    RdpDisplayConfig, RdpDisplayMetrics, RdpDisplayMode, RdpError, RdpErrorKind, RdpFrameEvent,
    RdpInputEvent, RdpPointerButton, RdpReconnectConfig, RdpRuntimeEvent, RdpServerCapabilities,
    RdpSessionConfig, RdpSessionDrain, RdpSessionState, RemoteCursorEvent, RemoteDesktopError,
    RemoteDesktopErrorCategory, RemoteDesktopViewState, RemoteFrameEvent, RemotePoint,
    RemotePointerButton, RemotePointerEvent, RemoteWheelAxis, VncClipboardConfig,
    VncControlMessage, VncDisplayConfig, VncError, VncErrorKind, VncInputEvent, VncReconnectConfig,
    VncRuntimeEvent, VncScaleMode, VncSecurityConfig, VncSecurityMode, VncServerCapabilities,
    VncSessionConfig, VncSessionDrain, VncSessionState, parse_rdp_certificate_policy,
    parse_rdp_clipboard_mode, parse_rdp_display_mode, parse_vnc_scale_mode,
    parse_vnc_security_mode, validate_committed_text,
};
pub use session::{RdpSessionManager, resolve_helper_path};
pub use vnc::{VncSessionManager, validate_vnc_config};
