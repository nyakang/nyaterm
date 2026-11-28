mod worker;
use crate::features::NyaTermApp;
use crate::models::TransferJobKind;
use crate::models::TransferJobStatus;
use gpui::Context;
use nyaterm_remote_desktop::ClipboardOrigin;
use nyaterm_remote_desktop::RemoteDesktopViewState;
use std::time::Duration;
use std::time::Instant;
pub(in crate::features::remote_desktop) use worker::ClipboardWorker;

pub(super) const CLIPBOARD_POLL_INTERVAL: Duration = Duration::from_millis(250);

impl NyaTermApp {
    pub(in crate::features) fn revoke_rdp_clipboard(
        &mut self,
        session_id: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let owner = self
            .remote_desktop
            .clipboard_worker
            .as_ref()
            .and_then(|worker| worker.owner());
        if let Some(owner) = owner
            && session_id.is_none_or(|id| id == owner)
        {
            self.remote_desktop.revoke_clipboard(Some(&owner));
            self.stop_rdp_clipboard_jobs(&owner, cx);
        }
    }

    pub(super) fn drive_managed_rdp_clipboard(
        &mut self,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) {
        if self.remote_desktop.clipboard_worker.is_none() {
            if !self.remote_desktop.sessions.values().any(|session| {
                session
                    .server_capabilities
                    .is_some_and(|c| c.managed_clipboard)
            }) {
                return;
            }
            match ClipboardWorker::new(self.remote_desktop.manager.clone()) {
                Ok(worker) => self.remote_desktop.clipboard_worker = Some(worker),
                Err(_) => {
                    self.shell.set_status("Cannot start RDP clipboard worker");
                    return;
                }
            }
        }
        let worker = self
            .remote_desktop
            .clipboard_worker
            .as_ref()
            .expect("clipboard worker");
        let owner = worker.owner();
        let eligible = |id: &str| {
            self.remote_desktop.sessions.get(id).is_some_and(|session| {
                matches!(session.state, RemoteDesktopViewState::Connected)
                    && session
                        .server_capabilities
                        .is_some_and(|c| c.managed_clipboard)
                    && self.session.metadata(id).is_some_and(|m| {
                        matches!(&m.launch_config,
                    crate::models::SessionLaunchConfig::Rdp(c) if c.redirection.clipboard)
                    })
            })
        };
        if self.security.screen_locked() || owner.as_deref().is_some_and(|id| !eligible(id)) {
            worker.set_policy(None, false);
        } else {
            let active = window.is_window_active()
                && cx
                    .active_window()
                    .is_none_or(|w| w == window.window_handle());
            let owns_window = owner
                .as_deref()
                .is_some_and(|id| self.remote_desktop_session_in_window(id, window));
            if active || owns_window {
                let target = self
                    .remote_desktop_input_session(window)
                    .filter(|id| eligible(id));
                let target = target.and_then(|id| {
                    self.remote_desktop
                        .sessions
                        .get(id)
                        .map(|session| (id.to_owned(), session.start_token.clone()))
                });
                worker.set_policy(target, active);
            }
        }
        let next_owner = worker.owner();
        let results = worker.results();
        if !results.is_empty() {
            cx.notify();
        }
        if next_owner != owner
            && let Some(previous) = owner
        {
            self.stop_rdp_clipboard_jobs(&previous, cx);
        }
        for result in results {
            if let Some(error) = &result.error {
                self.shell.set_status(error.clone());
            }
            if let Some(transfer_id) = result.transfer_id {
                self.transfer.visit_transfer_jobs_mut(|job| {
                    if job.session_id.as_deref() == Some(&result.session_id)
                        && matches!(&job.kind, TransferJobKind::RdpClipboard { .. })
                        && job.id == transfer_id
                        && matches!(job.status, TransferJobStatus::Running)
                    {
                        job.status = if result.error.is_some() {
                            TransferJobStatus::Cancelled
                        } else {
                            TransferJobStatus::Completed
                        };
                        job.detail = result.error.clone().unwrap_or_default();
                    }
                });
                self.defer_transfer_panel_snapshot_flush(cx);
            }
        }
    }

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
                job.detail = "RDP clipboard ownership ended".to_string();
                changed = true;
            }
        });
        if changed {
            self.defer_transfer_panel_snapshot_flush(cx);
        }
    }

    pub(super) fn poll_active_rdp_clipboard(
        &mut self,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.security.screen_locked() {
            return false;
        }
        let Some(session_id) = self.remote_desktop_input_session(window).map(str::to_owned) else {
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
                    crate::models::SessionLaunchConfig::Rdp(_) => None,
                    crate::models::SessionLaunchConfig::Vnc(config) if config.clipboard.enabled => {
                        Some(RemoteDesktopClipboardTarget::Vnc)
                    }
                    crate::models::SessionLaunchConfig::Spice(config)
                        if config.session.settings.clipboard.can_send()
                            && active_session
                                .spice
                                .as_ref()
                                .is_some_and(|s| s.capabilities.clipboard) =>
                    {
                        Some(RemoteDesktopClipboardTarget::Spice)
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
            RemoteDesktopClipboardTarget::Spice => {
                if let Err(error) = self.remote_desktop.spice_manager.send(
                    &session_id,
                    nyaterm_remote_desktop::spice::protocol::SpiceControl::Clipboard {
                        text: text.into(),
                    },
                ) {
                    session.error = Some(super::spice::spice_error(error));
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
    Vnc,
    Spice,
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
