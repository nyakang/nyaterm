use crate::features::NyaTermApp;
use crate::features::session::notices::SessionNoticeKind;
use nyaterm_core::ai::sanitize_ai_diagnostic;
use nyaterm_transport::session_notice::SessionNotice;
use rust_i18n::t;

impl NyaTermApp {
    pub(in crate::features) fn publish_session_notice(
        &mut self,
        session_id: &str,
        kind: SessionNoticeKind,
    ) -> bool {
        if session_id.is_empty()
            || self.terminal.session_id_is_retired(session_id)
            || !self.session.has_session(session_id)
        {
            return false;
        }
        self.session.notices.push(session_id, kind)
    }
}

pub(in crate::features) fn session_notice_text(kind: &SessionNoticeKind) -> (String, String) {
    let (key, detail) = match kind {
        SessionNoticeKind::Disconnected(reason) => ("sessionNotice.disconnected", reason.clone()),
        SessionNoticeKind::Error(message) => ("sessionNotice.error", message.clone()),
        SessionNoticeKind::WriteFailed { error, .. } => {
            ("sessionNotice.writeFailed", error.clone())
        }
        SessionNoticeKind::OutputDropped(bytes) => (
            "sessionNotice.outputDropped",
            t!("sessionNotice.droppedBytes", bytes = bytes).to_string(),
        ),
        SessionNoticeKind::Transport(notice) => match notice {
            SessionNotice::X11ForwardingDenied => (
                "sessionNotice.x11Denied",
                t!("sessionNotice.x11DeniedHelp").to_string(),
            ),
            SessionNotice::X11ServerUnavailable { display_target } => {
                let platform = if cfg!(windows) {
                    X11Platform::Windows
                } else if cfg!(target_os = "macos") {
                    X11Platform::Macos
                } else {
                    X11Platform::Linux
                };
                (
                    "sessionNotice.x11Unavailable",
                    format!(
                        "{}\n{}",
                        t!("sessionNotice.x11Target", target = display_target),
                        t!(x11_platform_hint_key(platform))
                    ),
                )
            }
            SessionNotice::TmuxControlUnconfirmed => (
                "sessionNotice.tmuxUnconfirmed",
                t!("sessionNotice.tmuxUnconfirmedHelp").to_string(),
            ),
            SessionNotice::TmuxFlowControlUnsupported => (
                "sessionNotice.tmuxFlowUnsupported",
                t!("sessionNotice.tmuxFlowUnsupportedHelp").to_string(),
            ),
            SessionNotice::TmuxPaneCloseFailed => ("sessionNotice.tmuxCloseFailed", String::new()),
            SessionNotice::TmuxCommandFailed { message } => {
                ("sessionNotice.tmuxCommandFailed", message.clone())
            }
        },
    };
    (t!(key).to_string(), sanitize_ai_diagnostic(&detail, 4096))
}

#[derive(Clone, Copy)]
enum X11Platform {
    Windows,
    Macos,
    Linux,
}

fn x11_platform_hint_key(platform: X11Platform) -> &'static str {
    match platform {
        X11Platform::Windows => "sessionNotice.x11WindowsHelp",
        X11Platform::Macos => "sessionNotice.x11MacosHelp",
        X11Platform::Linux => "sessionNotice.x11LinuxHelp",
    }
}

#[cfg(test)]
mod tests {
    use super::{X11Platform, x11_platform_hint_key};

    #[test]
    fn x11_help_covers_all_supported_platforms() {
        for (platform, expected) in [
            (X11Platform::Windows, "VcXsrv"),
            (X11Platform::Macos, "XQuartz"),
            (X11Platform::Linux, "DISPLAY"),
        ] {
            let key = x11_platform_hint_key(platform);
            let locales: serde_json::Value =
                serde_json::from_str(include_str!("../../../locales/en.json")).unwrap();
            let name = key.strip_prefix("sessionNotice.").unwrap();
            assert!(
                locales["sessionNotice"][name]
                    .as_str()
                    .unwrap()
                    .contains(expected)
            );
        }
    }
}
