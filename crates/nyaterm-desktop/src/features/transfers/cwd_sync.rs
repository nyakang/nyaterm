//! Event-driven terminal directory following. Session state owns the CWD; the
//! transfer browser owns only navigation requests and a deferred editing intent.

use gpui::Context;
use nyaterm_transport::{
    FileBrowserBackendKind, SftpCwdFollowMode, file_browser_path_eq,
    local_shell_cwd_to_browser_path,
};
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::models::{SessionLaunchConfig, TransferCwdSyncReason};

pub(super) fn cwd_paths_equal(backend: FileBrowserBackendKind, left: &str, right: &str) -> bool {
    let normalize = |path: &str| {
        let path = path.trim();
        if backend == FileBrowserBackendKind::Local {
            return std::path::Path::new(path)
                .components()
                .collect::<std::path::PathBuf>()
                .to_string_lossy()
                .into_owned();
        }
        let trimmed = path.trim_end_matches('/');
        if trimmed.is_empty() {
            path.to_string()
        } else {
            trimmed.to_string()
        }
    };
    file_browser_path_eq(backend, &normalize(left), &normalize(right))
}

/// WSL, Git Bash, and drive-letter OSC 7 payloads report POSIX paths the
/// Windows local browser cannot list; only a Windows host may translate them.
fn translate_local_browser_cwd(backend: Option<FileBrowserBackendKind>, cwd: String) -> String {
    match backend {
        Some(FileBrowserBackendKind::Local) if cfg!(target_os = "windows") => {
            local_shell_cwd_to_browser_path(&cwd).unwrap_or(cwd)
        }
        _ => cwd,
    }
}

impl NyaTermApp {
    pub(in crate::features) fn apply_session_cwd(
        &mut self,
        session_id: &str,
        cwd: String,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.session.has_session(session_id)
            || self.session.is_disconnected(session_id)
            || self.session.start_reconnect_is_pending(session_id)
        {
            return false;
        }
        let backend = self
            .session
            .file_browser_backend_support_for_session(session_id);
        let cwd = translate_local_browser_cwd(backend, cwd);
        let equivalent = backend
            .zip(self.session.cwd(session_id))
            .is_some_and(|(backend, previous)| cwd_paths_equal(backend, previous, &cwd));
        let changed = self.session.update_cwd(session_id, cwd);
        if !changed || equivalent || self.session.active_id() != Some(session_id) {
            return false;
        }
        self.sync_transfer_browser_to_cwd(crate::models::TransferCwdSyncReason::Automatic, cx)
    }

    /// Called at panel mutations, never by render or snapshot construction.
    pub(in crate::features) fn transfer_browser_visibility_changed(
        &mut self,
        was_visible: bool,
        cx: &mut Context<Self>,
    ) {
        let visible = self.transfer_browser_is_visible();
        if visible == was_visible {
            return;
        }
        if visible {
            if !self.sync_transfer_browser_to_cwd(TransferCwdSyncReason::Automatic, cx) {
                self.load_transfer_browser_for_active_session_if_needed(cx);
            }
        } else {
            if let Some(session_id) = self.session.active_id_owned() {
                self.transfer.cancel_browser_auto_cwd_sync(&session_id);
            }
            self.transfer.clear_pending_cwd_sync();
            self.transfer.finish_browser_path_edit();
            self.forget_text_inputs("transfer.browser.path");
        }
        self.defer_transfer_panel_snapshot_flush(cx);
    }

    pub(in crate::features) fn start_transfer_sync_cwd_job(&mut self, cx: &mut Context<Self>) {
        self.sync_transfer_browser_to_cwd(TransferCwdSyncReason::Explicit, cx);
    }

    pub(in crate::features) fn sync_transfer_browser_to_cwd(
        &mut self,
        reason: TransferCwdSyncReason,
        cx: &mut Context<Self>,
    ) -> bool {
        if reason == TransferCwdSyncReason::Automatic
            && (!self.transfer_browser_auto_sync_cwd_enabled()
                || !self.transfer_browser_is_visible())
        {
            return false;
        }
        let Some(session_id) = self.session.active_id_owned() else {
            return false;
        };
        if self.session.is_disconnected(&session_id) {
            return false;
        }
        let Some(backend) = self.session.active_file_browser_backend() else {
            return false;
        };
        let follow_disabled = self.session.metadata(&session_id).is_some_and(|metadata| {
            matches!(&metadata.launch_config, SessionLaunchConfig::Ssh(config)
                if config.sftp.cwd_follow_mode == SftpCwdFollowMode::Off)
        });
        let cwd = self
            .session
            .cwd(&session_id)
            .map(str::trim)
            .filter(|cwd| {
                !follow_disabled
                    && !cwd.is_empty()
                    && match backend {
                        FileBrowserBackendKind::Remote => cwd.starts_with('/'),
                        FileBrowserBackendKind::Local => std::path::Path::new(cwd).is_absolute(),
                    }
            })
            .map(str::to_string);
        let Some(cwd) = cwd else {
            if reason == TransferCwdSyncReason::Explicit {
                self.transfer.set_browser_status(if follow_disabled {
                    t!("fileExplorer.cwdTrackingUnavailable").to_string()
                } else {
                    t!("fileExplorer.cwdNotReported").to_string()
                });
                self.defer_transfer_panel_snapshot_flush(cx);
                cx.notify();
            }
            return false;
        };
        if reason == TransferCwdSyncReason::Automatic && self.transfer.browser_view().path_editing {
            self.transfer.defer_browser_cwd_sync();
            return true;
        }
        let browser = self.transfer.browser_view();
        if cwd_paths_equal(backend, browser.path, &cwd)
            && browser.error.is_none()
            && ((browser.loading
                && self
                    .transfer
                    .browser_navigation_job_running_for_session(&session_id))
                || (!browser.loading && self.transfer.has_browser_session_cache(&session_id)))
        {
            // Activation can restore an already loaded cache while an older
            // explicit request is pending. The alignment supersedes that request.
            if !browser.loading {
                self.transfer.invalidate_browser_navigation(&session_id);
            }
            if reason == TransferCwdSyncReason::Explicit {
                self.transfer.make_browser_cwd_sync_explicit(&session_id);
                self.transfer.clear_pending_cwd_sync();
            }
            return true;
        }
        let rollback = self.prepare_transfer_browser_navigation();
        self.forget_text_inputs("transfer.browser.path");
        self.transfer.set_remote_path(cwd.clone());
        self.transfer.begin_browser_directory_load(cwd.clone());
        self.transfer.record_browser_history(cwd);
        self.start_transfer_directory_navigation_job(None, rollback, Some(reason), cx);
        self.defer_transfer_panel_snapshot_flush(cx);
        true
    }
}

#[cfg(test)]
mod tests;
