use crate::features::NyaTermApp;
use crate::models::TransferJobKind;
use crate::models::TransferJobStatus;
use gpui::Context;
use nyaterm_remote_desktop::ClipboardOrigin;
use nyaterm_remote_desktop::RdpClipboardMode;
use nyaterm_remote_desktop::RemoteDesktopViewState;
use std::time::Duration;
use std::time::Instant;

pub(super) const CLIPBOARD_POLL_INTERVAL: Duration = Duration::from_millis(250);

impl NyaTermApp {
    pub(super) fn stop_rdp_clipboard_jobs(&mut self, session_id: &str, cx: &mut Context<Self>) {
        let mut changed = false;
        self.transfer.visit_transfer_jobs_mut(|job| {
            if job.session_id.as_deref() == Some(session_id)
                && matches!(job.kind, TransferJobKind::RdpClipboard { .. })
                && matches!(
                    job.status,
                    TransferJobStatus::Running | TransferJobStatus::Cancelling
                )
            {
                job.status = TransferJobStatus::Cancelled;
                job.detail = "RDP session disconnected".to_string();
                changed = true;
            }
        });
        if changed {
            self.defer_transfer_panel_snapshot_flush(cx);
        }
    }

    pub(super) fn poll_active_rdp_clipboard(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(session_id) = self.session.active_id_owned() else {
            return false;
        };
        if !self.remote_desktop.is_session(&session_id) {
            return false;
        }
        let Some(active_session) = self.remote_desktop.sessions.get(&session_id) else {
            return false;
        };
        if !matches!(active_session.state, RemoteDesktopViewState::Connected) {
            return false;
        }
        #[cfg(windows)]
        if active_session.native_rdp.is_some() {
            // The Microsoft control owns clipboard redirection for this session.
            return false;
        }
        let clipboard_target =
            self.session
                .metadata(&session_id)
                .and_then(|metadata| match &metadata.launch_config {
                    crate::models::SessionLaunchConfig::Rdp(config)
                        if config.clipboard.mode != RdpClipboardMode::Disabled =>
                    {
                        Some(RemoteDesktopClipboardTarget::Rdp)
                    }
                    crate::models::SessionLaunchConfig::Vnc(config) if config.clipboard.enabled => {
                        Some(RemoteDesktopClipboardTarget::Vnc)
                    }
                    _ => None,
                });
        let Some(clipboard_target) = clipboard_target else {
            return false;
        };
        let now = Instant::now();
        if self
            .remote_desktop
            .last_clipboard_poll
            .is_some_and(|last| now.saturating_duration_since(last) < CLIPBOARD_POLL_INTERVAL)
        {
            return false;
        }
        self.remote_desktop.last_clipboard_poll = Some(now);
        if !clipboard_has_unicode_text() {
            return false;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return false;
        };
        let Some(session) = self.remote_desktop.sessions.get_mut(&session_id) else {
            return false;
        };
        let Ok(Some(_generation)) = session.clipboard.accept(ClipboardOrigin::Local, &text) else {
            return false;
        };
        match clipboard_target {
            RemoteDesktopClipboardTarget::Rdp => {
                if let Err(error) = self
                    .remote_desktop
                    .manager
                    .set_clipboard_text(&session_id, text)
                {
                    session.error = Some(error.into());
                }
            }
            RemoteDesktopClipboardTarget::Vnc => {
                if let Err(error) = self
                    .remote_desktop
                    .vnc_manager
                    .set_clipboard_text(&session_id, text)
                {
                    session.error = Some(error.into());
                }
            }
        }
        true
    }
}

#[derive(Clone, Copy)]
enum RemoteDesktopClipboardTarget {
    Rdp,
    Vnc,
}

#[cfg(target_os = "windows")]
pub(super) fn clipboard_has_unicode_text() -> bool {
    // GPUI logs every unsupported OLE clipboard format it probes. Only enter
    // that path when Windows reports the text format this bridge accepts.
    unsafe {
        windows_sys::Win32::System::DataExchange::IsClipboardFormatAvailable(
            windows_sys::Win32::System::Ole::CF_UNICODETEXT as u32,
        ) != 0
    }
}

#[cfg(not(target_os = "windows"))]
pub(super) fn clipboard_has_unicode_text() -> bool {
    true
}
