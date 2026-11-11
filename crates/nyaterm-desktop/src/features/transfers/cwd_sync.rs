//! Event-driven terminal directory following. Session state owns the CWD; the
//! transfer browser owns only navigation requests and a deferred editing intent.

use crate::features::NyaTermApp;
use crate::models::{SessionLaunchConfig, TransferCwdSyncReason};
use gpui::Context;
use nyaterm_files::file_browser::{
    FileBrowserBackendKind, file_browser_path_eq, local_shell_cwd_to_browser_path,
    local_shell_cwd_to_browser_path_with_distro,
};
use nyaterm_transport::local_shell::LocalShellPathStyle;
use nyaterm_transport::{SftpCwdFollowMode, wsl_distro_from_launcher};
use rust_i18n::t;

pub(super) fn cwd_paths_equal(backend: FileBrowserBackendKind, left: &str, right: &str) -> bool {
    let normalize = |path: &str| {
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
/// A WSL cwd with no `\\wsl.localhost` representation cannot be browsed at all,
/// so it is dropped (`None`) rather than handed to the browser as a POSIX path
/// Windows would silently resolve against the current drive. Every other case
/// passes through unchanged.
fn translate_local_browser_cwd(
    backend: FileBrowserBackendKind,
    style: LocalShellPathStyle,
    wsl_distro: Option<&str>,
    cwd: String,
) -> Option<String> {
    if backend != FileBrowserBackendKind::Local || !cfg!(target_os = "windows") {
        return Some(cwd);
    }
    match style {
        LocalShellPathStyle::Wsl if cwd.starts_with('/') => {
            local_shell_cwd_to_browser_path_with_distro(style, &cwd, wsl_distro)
        }
        _ => Some(local_shell_cwd_to_browser_path(style, &cwd).unwrap_or(cwd)),
    }
}

impl NyaTermApp {
    fn local_shell_path_style_for_cwd(&self, session_id: &str) -> LocalShellPathStyle {
        self.session
            .manager()
            .local_shell_path_style(session_id)
            .or_else(|| {
                self.session.metadata(session_id).and_then(|metadata| {
                    match &metadata.launch_config {
                        SessionLaunchConfig::Local(config) => config
                            .shell_path
                            .as_deref()
                            .map(LocalShellPathStyle::from_program),
                        _ => None,
                    }
                })
            })
            .unwrap_or_default()
    }

    pub(in crate::features) fn transfer_browser_cwd_for_session(
        &self,
        session_id: &str,
    ) -> Option<String> {
        let backend = self.session.file_browser_backend_for_session(session_id)?;
        let style = self.local_shell_path_style_for_cwd(session_id);
        let wsl_distro = self
            .session
            .manager()
            .local_wsl_distro(session_id)
            .or_else(|| {
                self.session.metadata(session_id).and_then(|metadata| {
                    match &metadata.launch_config {
                        SessionLaunchConfig::Local(config) => wsl_distro_from_launcher(
                            config.shell_path.as_deref()?,
                            &config.shell_args,
                        ),
                        _ => None,
                    }
                })
            });
        let cwd = self.session.cwd(session_id)?;
        let cwd = if backend == FileBrowserBackendKind::Local && style == LocalShellPathStyle::Wsl {
            cwd
        } else {
            cwd.trim()
        };
        let cwd =
            translate_local_browser_cwd(backend, style, wsl_distro.as_deref(), cwd.to_string())?;
        (!cwd.is_empty()
            && match backend {
                FileBrowserBackendKind::Remote | FileBrowserBackendKind::Adb => {
                    cwd.starts_with('/')
                }
                FileBrowserBackendKind::Local => std::path::Path::new(&cwd).is_absolute(),
            })
        .then_some(cwd)
    }

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
        let equivalent =
            backend
                .zip(self.session.cwd(session_id))
                .is_some_and(|(backend, previous)| {
                    if backend == FileBrowserBackendKind::Local
                        && self.local_shell_path_style_for_cwd(session_id)
                            == LocalShellPathStyle::Wsl
                        && previous.starts_with('/')
                        && cwd.starts_with('/')
                    {
                        // These are guest paths, before translation to a Windows
                        // browser path. Never fold their case or trim whitespace.
                        previous.trim_end_matches('/') == cwd.trim_end_matches('/')
                    } else {
                        cwd_paths_equal(backend, previous, &cwd)
                    }
                });
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
            self.request_missing_expanded_tree_listings(cx);
        } else {
            if let Some(session_id) = self.session.active_id_owned() {
                self.transfer.cancel_browser_auto_cwd_sync(&session_id);
                self.transfer.cancel_tree_session_requests(&session_id);
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
            .transfer_browser_cwd_for_session(&session_id)
            .filter(|_| !follow_disabled);
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
                self.reveal_transfer_tree_current_path(cx);
                self.defer_transfer_panel_snapshot_flush(cx);
            }
            return true;
        }
        let rollback = self.prepare_transfer_browser_navigation();
        self.forget_text_inputs("transfer.browser.path");
        self.transfer.set_remote_path(cwd.clone());
        self.transfer.begin_browser_directory_load(cwd.clone());
        self.transfer.record_browser_history(cwd);
        self.start_transfer_directory_navigation_job(None, rollback, Some(reason), cx);
        // Reveal cached ancestors immediately, without waiting for remote metadata.
        if self
            .transfer
            .browser_navigation_job_running_for_session(&session_id)
        {
            self.reveal_transfer_tree_current_path(cx);
        }
        self.defer_transfer_panel_snapshot_flush(cx);
        true
    }
}

#[cfg(test)]
mod tests;
