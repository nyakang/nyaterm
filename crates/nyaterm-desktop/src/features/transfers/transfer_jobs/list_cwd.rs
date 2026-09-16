use std::time::{Duration, Instant};

use gpui::Context;
use nyaterm_transport::{FileBrowserBackendKind, SftpCwdFollowMode, SshProcessService};

use crate::features::NyaTermApp;
use crate::models::{
    TransferBrowserChildrenMenuStatus, TransferBrowserNavigationSnapshot,
    TransferBrowserPathMenuKind, TransferBrowserPathMenuState, TransferJobEvent, TransferJobKind,
    TransferJobOutput, TransferJobResult, TransferJobState, TransferJobStatus,
};

use super::helpers::submit_transfer_blocking_job;

impl NyaTermApp {
    pub(in crate::features) fn start_transfer_browser_children_job(
        &mut self,
        remote_path: String,
        cx: &mut Context<Self>,
    ) {
        let service = match self.active_file_browser_service() {
            Ok(service) => service,
            Err(error) => {
                self.transfer.browser.status = error.to_string();
                cx.notify();
                return;
            }
        };
        let id = self.transfer.next_transfer_job_id("sftp-children");
        if let Some(TransferBrowserPathMenuState {
            kind:
                TransferBrowserPathMenuKind::Children {
                    path,
                    request_id,
                    status,
                    ..
                },
            ..
        }) = self.transfer.browser.path_menu.as_mut()
        {
            if path != &remote_path {
                return;
            }
            *request_id = Some(id.clone());
            *status = TransferBrowserChildrenMenuStatus::Loading;
        } else {
            return;
        }
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id: self.session.active_id_owned(),
            kind: TransferJobKind::ListChildren {
                remote_path: remote_path.clone(),
            },
            status: TransferJobStatus::Running,
            detail: format!("Listing child directories in {remote_path}"),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
        });
        let transfer_tx = self.transfer.transfer_event_sender();
        submit_transfer_blocking_job(
            &self.blocking_jobs,
            "sftp-list-children",
            id.clone(),
            transfer_tx.clone(),
            move || {
                let result = service
                    .list_dir(&remote_path)
                    .map(|entries| TransferJobOutput::ChildEntries {
                        remote_path,
                        entries,
                    })
                    .map_err(|error| error.to_string());
                let _ = transfer_tx.unbounded_send(TransferJobResult {
                    id,
                    event: TransferJobEvent::Finished(result),
                });
            },
        );
        cx.notify();
    }

    pub(in crate::features) fn start_sftp_list_job(
        &mut self,
        select_after: Option<String>,
        rollback: TransferBrowserNavigationSnapshot,
        cx: &mut Context<Self>,
    ) {
        let service = match self.active_file_browser_service() {
            Ok(service) => service,
            Err(error) => {
                self.restore_transfer_browser_navigation(rollback);
                self.shell.set_status(error.to_string());
                cx.notify();
                return;
            }
        };
        let remote_file_path = self.transfer.browser_remote_file_path();
        let remote_path = remote_file_path.display_path.clone();
        self.transfer.browser.path = remote_path.clone();
        self.transfer.browser.status = format!("Listing {remote_path}...");
        self.transfer.browser.loading = true;
        self.transfer.browser.error = None;
        self.transfer.browser.selected_remote_path = None;
        self.transfer.browser.selected_remote_paths.clear();
        let id = self.transfer.next_transfer_job_id("sftp-list");
        let job_session_id = self.session.active_id_owned();
        self.transfer
            .browser
            .navigation_jobs
            .insert(job_session_id.clone().unwrap_or_default(), id.clone());
        self.transfer
            .browser
            .pending_navigations
            .insert(id.clone(), rollback);
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id: job_session_id,
            kind: TransferJobKind::ListDir {
                remote_path: remote_path.clone(),
                select_after,
            },
            status: TransferJobStatus::Running,
            detail: format!("Listing {remote_path}"),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
        });
        self.shell
            .set_status(format!("remote file list started for {remote_path}"));
        let transfer_tx = self.transfer.transfer_event_sender();
        submit_transfer_blocking_job(
            &self.blocking_jobs,
            "sftp-list-directory",
            id.clone(),
            transfer_tx.clone(),
            move || {
                let result = service
                    .list_dir_path(&remote_file_path)
                    .map(TransferJobOutput::Entries)
                    .map_err(|error| error.to_string());
                let _ = transfer_tx.unbounded_send(TransferJobResult {
                    id,
                    event: TransferJobEvent::Finished(result),
                });
            },
        );
        cx.notify();
    }

    pub(in crate::features) fn start_transfer_sync_cwd_job(&mut self, cx: &mut Context<Self>) {
        if self.transfer_sync_cwd_job_running() {
            self.transfer.browser.status = "remote cwd sync already running".to_string();
            cx.notify();
            return;
        }
        if self.session.active_file_browser_backend() == Some(FileBrowserBackendKind::Local) {
            self.start_local_transfer_sync_cwd_job(cx);
            return;
        }
        let context = match self.active_ssh_runtime_context("syncing remote cwd") {
            Ok(context) => context,
            Err(message) => {
                self.shell.set_status(message.clone());
                self.transfer.browser.status = message;
                self.transfer.browser.loading = false;
                cx.notify();
                return;
            }
        };
        let config = context.config;
        let multiplex = context.multiplex;
        let job_session_id = Some(context.session_id);
        let shell_cwd = match config.sftp.cwd_follow_mode {
            SftpCwdFollowMode::Off => {
                self.transfer.browser.status =
                    "remote cwd follow is disabled for this connection".to_string();
                self.transfer.browser.loading = false;
                self.transfer.browser.error =
                    Some("remote cwd follow is disabled for this connection".to_string());
                self.shell
                    .set_status("remote cwd follow is disabled for this connection".to_string());
                cx.notify();
                return;
            }
            SftpCwdFollowMode::ShellIntegration => {
                let cwd = self
                    .session
                    .active_id_owned()
                    .and_then(|session_id| self.session.cwd(&session_id).map(str::to_string))
                    .map(|cwd| cwd.trim().to_string())
                    .filter(|cwd| cwd.starts_with('/'));
                let Some(cwd) = cwd else {
                    self.transfer.browser.status =
                        "remote cwd is not available from shell integration".to_string();
                    self.transfer.browser.loading = false;
                    self.transfer.browser.error =
                        Some("remote cwd is not available from shell integration".to_string());
                    self.shell.set_status(
                        "remote cwd is not available from shell integration".to_string(),
                    );
                    cx.notify();
                    return;
                };
                Some(cwd)
            }
            SftpCwdFollowMode::RcFile => None,
        };
        self.transfer.browser.auto_sync_cwd_last_at = Some(Instant::now());
        let id = self.transfer.next_transfer_job_id("sftp-sync-cwd");
        self.transfer
            .browser
            .navigation_jobs
            .insert(job_session_id.clone().unwrap_or_default(), id.clone());
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id: job_session_id,
            kind: TransferJobKind::SyncCwd,
            status: TransferJobStatus::Running,
            detail: "Resolving remote cwd".to_string(),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
        });
        // A CWD poll also refreshes the listing, but it must not make an unchanged
        // directory look as though it is navigating again. In shell-integration
        // mode we already know the target, so retain a successfully loaded current
        // listing -- including an empty directory -- while the background check runs.
        let should_show_loading = shell_cwd.as_deref().is_none_or(|cwd| {
            transfer_cwd_sync_should_show_loading(
                &self.transfer.browser.path,
                self.transfer.browser.loading,
                self.transfer.browser.error.as_deref(),
                cwd,
            )
        });
        if should_show_loading {
            self.transfer.browser.status = "Resolving remote cwd...".to_string();
            self.transfer.browser.loading = true;
            self.shell.set_status("remote cwd sync started".to_string());
        }
        self.transfer.browser.error = None;
        let service = match self.active_file_browser_service() {
            Ok(service) => service,
            Err(error) => {
                self.transfer.browser.loading = false;
                self.transfer.browser.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let transfer_tx = self.transfer.transfer_event_sender();
        submit_transfer_blocking_job(
            &self.blocking_jobs,
            "sftp-sync-cwd",
            id.clone(),
            transfer_tx.clone(),
            move || {
                let result = (|| {
                    let remote_path = match shell_cwd {
                        Some(cwd) => cwd,
                        None => {
                            let timeout = Duration::from_millis(
                                config.sftp.shell_detection_timeout_ms.clamp(100, 60_000),
                            );
                            let output = SshProcessService::with_multiplex(
                                config.clone(),
                                multiplex.clone(),
                            )
                            .map_err(|error| error.to_string())?
                            .run_command("pwd -P", timeout)
                            .map_err(|error| error.to_string())?;
                            if output.exit_status.is_some_and(|status| status != 0) {
                                let detail = output
                                    .stderr
                                    .trim()
                                    .lines()
                                    .next()
                                    .or_else(|| output.stdout.trim().lines().next())
                                    .unwrap_or("remote pwd failed");
                                return Err(detail.to_string());
                            }
                            output
                                .stdout
                                .lines()
                                .map(str::trim)
                                .find(|line| line.starts_with('/'))
                                .ok_or_else(|| {
                                    "remote pwd did not return an absolute path".to_string()
                                })?
                                .to_string()
                        }
                    };
                    let entries = service
                        .list_dir(&remote_path)
                        .map_err(|error| error.to_string())?;
                    Ok(TransferJobOutput::CwdSynced {
                        remote_path,
                        entries,
                    })
                })();
                let _ = transfer_tx.unbounded_send(TransferJobResult {
                    id,
                    event: TransferJobEvent::Finished(result),
                });
            },
        );
        if should_show_loading {
            cx.notify();
        }
    }

    pub(in crate::features) fn transfer_sync_cwd_job_running(&self) -> bool {
        self.transfer.transfer_jobs().iter().any(|job| {
            job.kind == TransferJobKind::SyncCwd
                && matches!(
                    job.status,
                    TransferJobStatus::Running
                        | TransferJobStatus::Paused
                        | TransferJobStatus::Cancelling
                )
        })
    }

    pub(in crate::features) fn start_transfer_browser_home_dir_job(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.transfer.browser.home_dir_pending || !self.transfer.browser.home_dir.is_empty() {
            return;
        }
        let service = match self.active_file_browser_service() {
            Ok(service) => service,
            Err(error) => {
                self.transfer.browser.status = error.to_string();
                cx.notify();
                return;
            }
        };
        self.transfer.browser.home_dir_pending = true;
        let id = self.transfer.next_transfer_job_id("sftp-home");
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id: self.session.active_id_owned(),
            kind: TransferJobKind::ResolveHome,
            status: TransferJobStatus::Running,
            detail: "Resolving remote home".to_string(),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
        });
        self.transfer.browser.status = "Resolving remote home...".to_string();
        let transfer_tx = self.transfer.transfer_event_sender();
        submit_transfer_blocking_job(
            &self.blocking_jobs,
            "sftp-resolve-home",
            id.clone(),
            transfer_tx.clone(),
            move || {
                let result = (|| {
                    let home_dir = service.home_dir().map_err(|error| error.to_string())?;
                    Ok(TransferJobOutput::HomeDir(home_dir))
                })();
                let _ = transfer_tx.unbounded_send(TransferJobResult {
                    id,
                    event: TransferJobEvent::Finished(result),
                });
            },
        );
        cx.notify();
    }

    fn start_local_transfer_sync_cwd_job(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.session.active_id_owned() else {
            return;
        };
        let Some(cwd) = self
            .session
            .cwd(&session_id)
            .map(str::trim)
            .filter(|cwd| !cwd.is_empty())
            .map(str::to_string)
        else {
            self.transfer.browser.status = "local cwd is not available".to_string();
            self.transfer.browser.loading = false;
            self.transfer.browser.error = Some("local cwd is not available".to_string());
            cx.notify();
            return;
        };
        let service = match self.active_file_browser_service() {
            Ok(service) => service,
            Err(error) => {
                self.transfer.browser.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let id = self.transfer.next_transfer_job_id("local-sync-cwd");
        self.transfer
            .browser
            .navigation_jobs
            .insert(session_id.clone(), id.clone());
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id: Some(session_id),
            kind: TransferJobKind::SyncCwd,
            status: TransferJobStatus::Running,
            detail: "Resolving local cwd".to_string(),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
        });
        // Local sessions always expose their CWD. Keep a successfully loaded current
        // directory visible while its periodic listing check is in flight, including
        // a legitimately empty directory.
        let should_show_loading = transfer_cwd_sync_should_show_loading(
            &self.transfer.browser.path,
            self.transfer.browser.loading,
            self.transfer.browser.error.as_deref(),
            &cwd,
        );
        if should_show_loading {
            self.transfer.browser.loading = true;
        }
        self.transfer.browser.error = None;
        let transfer_tx = self.transfer.transfer_event_sender();
        submit_transfer_blocking_job(
            &self.blocking_jobs,
            "local-sync-cwd",
            id.clone(),
            transfer_tx.clone(),
            move || {
                let result = service
                    .list_dir(&cwd)
                    .map(|entries| TransferJobOutput::CwdSynced {
                        remote_path: cwd,
                        entries,
                    })
                    .map_err(|error| error.to_string());
                let _ = transfer_tx.unbounded_send(TransferJobResult {
                    id,
                    event: TransferJobEvent::Finished(result),
                });
            },
        );
        if should_show_loading {
            cx.notify();
        }
    }
}

fn transfer_cwd_sync_should_show_loading(
    current_path: &str,
    loading: bool,
    error: Option<&str>,
    target_cwd: &str,
) -> bool {
    current_path != target_cwd || loading || error.is_some()
}

#[cfg(test)]
mod tests {
    use super::transfer_cwd_sync_should_show_loading;

    #[test]
    fn cwd_poll_keeps_a_loaded_empty_directory_quiet() {
        assert!(!transfer_cwd_sync_should_show_loading(
            "/remote/empty",
            false,
            None,
            "/remote/empty"
        ));
    }

    #[test]
    fn cwd_poll_surfaces_navigation_loading_and_retry_states() {
        assert!(transfer_cwd_sync_should_show_loading(
            "/remote/one",
            false,
            None,
            "/remote/two"
        ));
        assert!(transfer_cwd_sync_should_show_loading(
            "/remote", true, None, "/remote"
        ));
        assert!(transfer_cwd_sync_should_show_loading(
            "/remote",
            false,
            Some("previous list failed"),
            "/remote"
        ));
    }
}
