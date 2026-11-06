//! Application diagnostics carried beside terminal bytes, never through the decoder.
use std::fmt;

#[derive(Clone, PartialEq, Eq)]
pub enum SessionNotice {
    X11ForwardingDenied,
    X11ServerUnavailable { display_target: String },
    TmuxControlUnconfirmed,
    TmuxFlowControlUnsupported,
    TmuxPaneCloseFailed,
    TmuxCommandFailed { message: String },
}

impl SessionNotice {
    pub fn category(&self) -> &'static str {
        match self {
            Self::X11ForwardingDenied => "x11-forwarding-denied",
            Self::X11ServerUnavailable { .. } => "x11-server-unavailable",
            Self::TmuxControlUnconfirmed => "tmux-control-unconfirmed",
            Self::TmuxFlowControlUnsupported => "tmux-flow-control-unsupported",
            Self::TmuxPaneCloseFailed => "tmux-pane-close-failed",
            Self::TmuxCommandFailed { .. } => "tmux-command-failed",
        }
    }
}

impl fmt::Debug for SessionNotice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Server diagnostics and display targets can contain user data.
        f.debug_struct("SessionNotice")
            .field("category", &self.category())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::SessionNotice;

    #[test]
    fn debug_does_not_expose_server_message_or_display_target() {
        for notice in [
            SessionNotice::TmuxCommandFailed {
                message: "fixture-private-message".into(),
            },
            SessionNotice::X11ServerUnavailable {
                display_target: "fixture-private-display".into(),
            },
        ] {
            assert!(!format!("{notice:?}").contains("fixture-private"));
        }
    }
}
