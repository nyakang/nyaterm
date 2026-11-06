use super::helpers::{submit_transfer_blocking_job, submit_transfer_cancellable_blocking_job};
use crate::features::NyaTermApp;
use crate::models::{
    TransferBrowserChildrenMenuStatus, TransferBrowserNavigationSnapshot,
    TransferBrowserPathMenuKind, TransferBrowserPathMenuState, TransferJobEvent, TransferJobKind,
    TransferJobOutput, TransferJobResult, TransferJobState, TransferJobStatus,
};
use gpui::Context;
use nyaterm_files::sftp::SftpFileEntry;

impl NyaTermApp {
    pub(in crate::features) fn start_transfer_initial_directory_job(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let service = match self.active_file_browser_service() {
            Ok(service) => service,
            Err(error) => {
                self.transfer.browser.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let Some(session_id) = self.session.active_id_owned() else {
            return;
        };
        let rollback = self.prepare_transfer_browser_navigation();
        let id = self.transfer.next_transfer_job_id("sftp-initial-directory");
        self.transfer
            .browser
            .pending_navigations
            .insert(id.clone(), rollback);
        self.transfer
            .browser
            .navigation_jobs
            .insert(session_id.clone(), id.clone());
        if let Some(backend) = self.session.active_file_browser_backend() {
            self.transfer
                .begin_tree_navigation(&session_id, backend, id.clone());
        }
        self.transfer.begin_browser_directory_load("/".to_string());
        self.transfer.set_remote_path("/");
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id: Some(session_id),
            kind: TransferJobKind::InitialDirectory,
            status: TransferJobStatus::Running,
            detail: "Resolving initial remote directory".to_string(),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
            speed: Default::default(),
        });
        #[cfg(test)]
        if let Some(requests) = self.transfer.browser.directory_requests.as_mut() {
            requests.push((id, nyaterm_files::sftp::RemoteFilePath::new("/")));
            cx.notify();
            return;
        }
        let transfer_tx = self.transfer.transfer_event_sender();
        let handle_id = id.clone();
        let handle = submit_transfer_cancellable_blocking_job(
            &self.blocking_jobs,
            "sftp-initial-directory",
            id.clone(),
            transfer_tx.clone(),
            move || {
                let result = resolve_initial_directory(
                    || service.home_dir().map_err(|error| error.to_string()),
                    |path| service.list_dir(path).map_err(|error| error.to_string()),
                );
                let _ = transfer_tx.unbounded_send(TransferJobResult {
                    id,
                    event: TransferJobEvent::Finished(result),
                });
            },
        );
        if let Some(handle) = handle {
            self.transfer
                .browser
                .navigation_handles
                .insert(handle_id, handle);
        }
        cx.notify();
    }

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
            speed: Default::default(),
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
        self.start_transfer_directory_navigation_job(select_after, rollback, None, cx);
    }

    pub(in crate::features) fn start_transfer_directory_navigation_job(
        &mut self,
        select_after: Option<String>,
        rollback: TransferBrowserNavigationSnapshot,
        cwd_reason: Option<crate::models::TransferCwdSyncReason>,
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
        if let (Some(session_id), Some(backend)) = (
            job_session_id.as_deref(),
            self.session.active_file_browser_backend(),
        ) {
            self.transfer
                .begin_tree_navigation(session_id, backend, id.clone());
        }
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id: job_session_id,
            kind: cwd_reason.map_or_else(
                || TransferJobKind::ListDir {
                    remote_path: remote_path.clone(),
                    select_after,
                },
                |reason| TransferJobKind::SyncCwd { reason },
            ),
            status: TransferJobStatus::Running,
            detail: format!("Listing {remote_path}"),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
            speed: Default::default(),
        });
        self.shell
            .set_status(format!("remote file list started for {remote_path}"));
        #[cfg(test)]
        if let Some(requests) = self.transfer.browser.directory_requests.as_mut() {
            requests.push((id, remote_file_path));
            cx.notify();
            return;
        }
        let transfer_tx = self.transfer.transfer_event_sender();
        let handle_id = id.clone();
        let handle = submit_transfer_cancellable_blocking_job(
            &self.blocking_jobs,
            "sftp-list-directory",
            id.clone(),
            transfer_tx.clone(),
            move || {
                let result = service
                    .list_dir_path(&remote_file_path)
                    .map(|entries| match cwd_reason {
                        Some(_) => TransferJobOutput::CwdSynced {
                            remote_path,
                            entries,
                        },
                        None => TransferJobOutput::Entries(entries),
                    })
                    .map_err(|error| error.to_string());
                let _ = transfer_tx.unbounded_send(TransferJobResult {
                    id,
                    event: TransferJobEvent::Finished(result),
                });
            },
        );
        if let Some(handle) = handle {
            self.transfer
                .browser
                .navigation_handles
                .insert(handle_id, handle);
        }
        cx.notify();
    }

    pub(in crate::features) fn start_transfer_browser_home_dir_job(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.transfer.browser.home_dir_pending
            || !self.transfer.browser.home_dir.is_empty()
            || self.session.active_id().is_some_and(|session_id| {
                self.transfer
                    .browser_navigation_job_running_for_session(session_id)
            })
        {
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
            speed: Default::default(),
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
}

fn resolve_initial_directory(
    home_dir: impl FnOnce() -> Result<String, String>,
    mut list_dir: impl FnMut(&str) -> Result<Vec<SftpFileEntry>, String>,
) -> Result<TransferJobOutput, String> {
    if let Ok(home) = home_dir() {
        let home = home.trim();
        if home.starts_with('/')
            && home != "/"
            && let Ok(entries) = list_dir(home)
        {
            return Ok(TransferJobOutput::InitialDirectory {
                path: home.to_string(),
                entries,
            });
        }
    }
    list_dir("/").map(|entries| TransferJobOutput::InitialDirectory {
        path: "/".to_string(),
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::resolve_initial_directory;
    use crate::models::TransferJobOutput;

    #[test]
    fn initial_directory_uses_home_when_it_can_be_listed() {
        let result = resolve_initial_directory(
            || Ok("/home/user".to_string()),
            |path| {
                assert_eq!(path, "/home/user");
                Ok(Vec::new())
            },
        )
        .unwrap();
        assert!(
            matches!(result, TransferJobOutput::InitialDirectory { path, .. } if path == "/home/user")
        );
    }

    #[test]
    fn initial_directory_falls_back_to_root_after_home_error() {
        let result = resolve_initial_directory(
            || Err("home unavailable".to_string()),
            |path| {
                assert_eq!(path, "/");
                Ok(Vec::new())
            },
        )
        .unwrap();
        assert!(matches!(result, TransferJobOutput::InitialDirectory { path, .. } if path == "/"));
    }

    #[test]
    fn initial_directory_falls_back_to_root_after_home_listing_error() {
        let mut attempted = Vec::new();
        let result = resolve_initial_directory(
            || Ok("/home/user".to_string()),
            |path| {
                attempted.push(path.to_string());
                if path == "/" {
                    Ok(Vec::new())
                } else {
                    Err("denied".to_string())
                }
            },
        )
        .unwrap();
        assert_eq!(attempted, vec!["/home/user", "/"]);
        assert!(matches!(result, TransferJobOutput::InitialDirectory { path, .. } if path == "/"));
    }

    #[test]
    fn initial_directory_reports_root_error_when_both_locations_fail() {
        let result = resolve_initial_directory(
            || Ok("/home/user".to_string()),
            |_| Err("root unavailable".to_string()),
        );
        assert!(matches!(result, Err(error) if error == "root unavailable"));
    }
}
