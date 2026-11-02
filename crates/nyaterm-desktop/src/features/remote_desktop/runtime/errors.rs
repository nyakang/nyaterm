use nyaterm_remote_desktop::{RdpError, RdpErrorKind, RemoteDesktopError};

pub(in crate::features::remote_desktop) fn format_rdp_error(error: &RdpError) -> String {
    let category = match error.kind {
        RdpErrorKind::ResourceExhausted => "Remote desktop resources exhausted",
        RdpErrorKind::Authentication => "Authentication failed",
        RdpErrorKind::CertificateRejected => "Certificate rejected",
        RdpErrorKind::Timeout => "Connection timed out",
        RdpErrorKind::ConnectionRefused => "Connection refused",
        RdpErrorKind::Tls => "RDP TLS connection failed",
        RdpErrorKind::Transport => "RDP transport interrupted",
        RdpErrorKind::Session => "RDP session terminated",
        RdpErrorKind::Clipboard => "RDP clipboard failed",
        RdpErrorKind::Negotiation => "RDP negotiation failed",
        RdpErrorKind::HelperMissing => "RDP helper is missing",
        RdpErrorKind::HelperCrashed => "RDP helper crashed",
        RdpErrorKind::Ipc => "RDP helper communication failed",
        RdpErrorKind::Protocol => "RDP protocol error",
        RdpErrorKind::Unsupported => "RDP feature is unsupported",
    };
    format!("{category}: {}", error.message)
}

pub(in crate::features::remote_desktop) fn format_remote_desktop_error(
    error: &RemoteDesktopError,
) -> String {
    format!("{:?}: {}", error.category, error.message)
}
