use super::helpers::{TransferProgressEventSender, transfer_job_remote_parent_path};
use crate::features::NyaTermApp;
use crate::features::transfers::FileTransferSession;
use crate::models::{
    TransferJobEvent, TransferJobKind, TransferJobOutput, TransferJobResult, TransferJobState,
    TransferJobStatus,
};
use gpui::{Context, Window};
use nyaterm_files::sftp::{RemoteFilePath, SftpTransferControl};
use nyaterm_files::sftp_transfer_types::{
    SftpDuplicatePolicy, SftpDuplicateResolver, SftpPathTransferOptions,
};
use rust_i18n::t;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::Duration;

impl NyaTermApp {
    /// Use clone_for_download_batch on these options for sibling downloads.
    pub(in crate::features) fn sftp_download_path_options(&self) -> SftpPathTransferOptions {
        let duplicate_policy = self.transfer.duplicate_policy();
        let duplicate_resolver = (duplicate_policy == SftpDuplicatePolicy::Ask)
            .then(|| self.session.prompt_duplicate_broker() as Arc<dyn SftpDuplicateResolver>);
        SftpPathTransferOptions::new(
            duplicate_policy,
            duplicate_resolver,
            self.sftp_transfer_options(),
        )
    }

    pub(in crate::features) fn start_sftp_download_job_for_target(
        &mut self,
        remote_path: RemoteFilePath,
        local_path: PathBuf,
        path_options: SftpPathTransferOptions,
        cx: &mut Context<Self>,
    ) -> bool {
        let session = match self.browser_transfer_session() {
            Ok(session) => session,
            Err(error) => {
                self.report_browser_transfer_error("download", &error, cx);
                return false;
            }
        };
        self.enqueue_sftp_download_job_for_target(
            session,
            remote_path,
            local_path,
            path_options,
            cx,
        );
        true
    }

    pub(in crate::features) fn enqueue_sftp_download_job_for_target(
        &mut self,
        mut session: FileTransferSession,
        remote_path: RemoteFilePath,
        local_path: PathBuf,
        path_options: SftpPathTransferOptions,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self.refresh_manual_transfer_session(&mut session) {
            self.shell.set_status(error.to_string());
            cx.notify();
            return;
        }
        let id = self.transfer.next_transfer_job_id("sftp-download");
        let control = SftpTransferControl::new();
        let display_path = remote_path.display_path.clone();
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id: session.session_id,
            kind: TransferJobKind::Download {
                remote_path: display_path.clone(),
                raw_path_token: remote_path.raw_path_token.clone(),
                local_path: local_path.clone(),
            },
            status: TransferJobStatus::Running,
            detail: format!("Downloading {display_path}"),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: Some(control.clone()),
            speed: Default::default(),
        });
        let path_options = self
            .transfer
            .bind_transfer_job_path_options(&id, path_options);
        self.transfer
            .bind_transfer_job_source(&id, &session.service);
        self.shell
            .set_status(format!("remote download started for {display_path}"));
        let progress_tx = self.transfer.transfer_event_sender();
        let finished_tx = self.transfer.transfer_event_sender();
        self.submit_bulk_transfer_job(
            "sftp-download",
            id.clone(),
            finished_tx.clone(),
            move || {
                let mut progress_sender = TransferProgressEventSender::new(id.clone(), progress_tx);
                let result = session
                    .service
                    .download_remote_path_with_progress_and_path_options(
                        &remote_path,
                        local_path,
                        control,
                        path_options,
                        move |progress| {
                            progress_sender.send(progress);
                        },
                    )
                    .map(TransferJobOutput::Summary);
                if let Err(error) = &result {
                    crate::features::android::labels::log_file_error(
                        "download",
                        session.service.kind(),
                        &id,
                        error,
                    );
                }
                let _ = finished_tx.unbounded_send(TransferJobResult {
                    id,
                    event: TransferJobEvent::FileFinished(result),
                });
            },
        );
        cx.notify();
    }

    pub(in crate::features) fn enqueue_sftp_upload_job_for_target(
        &mut self,
        mut session: FileTransferSession,
        local_path: PathBuf,
        remote_path: String,
        path_options: SftpPathTransferOptions,
        target_lock: Arc<Mutex<()>>,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self.refresh_manual_transfer_session(&mut session) {
            self.shell.set_status(error.to_string());
            cx.notify();
            return;
        }
        let id = self.transfer.next_transfer_job_id("sftp-upload");
        let control = SftpTransferControl::new();
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id: session.session_id,
            kind: TransferJobKind::Upload {
                local_path: local_path.clone(),
                remote_path: remote_path.clone(),
            },
            status: TransferJobStatus::Running,
            detail: format!("Uploading {}", local_path.display()),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: Some(control.clone()),
            speed: Default::default(),
        });
        let path_options = self
            .transfer
            .bind_transfer_job_path_options(&id, path_options);
        self.transfer
            .bind_transfer_job_source(&id, &session.service);
        self.shell.set_status(format!(
            "remote upload started for {}",
            local_path.display()
        ));
        self.transfer
            .bind_upload_target_lock(&id, target_lock.clone());
        let progress_tx = self.transfer.transfer_event_sender();
        let finished_tx = self.transfer.transfer_event_sender();
        self.submit_bulk_transfer_job("sftp-upload", id.clone(), finished_tx.clone(), move || {
            // Siblings in one selection may have the same basename. Hold the
            // target lock through conflict resolution and the upload itself.
            let mut progress_sender = TransferProgressEventSender::new(id.clone(), progress_tx);
            let service = session.service;
            let result = (|| {
                let _target_guard = lock_upload_target(&target_lock, &control)?;
                service
                    .upload_path_with_progress_and_path_options(
                        local_path,
                        &remote_path,
                        control,
                        path_options,
                        move |progress| {
                            progress_sender.send(progress);
                        },
                    )
                    .map(|summary| {
                        if summary.skipped {
                            return TransferJobOutput::Summary(summary);
                        }
                        let parent_path = transfer_job_remote_parent_path(&summary.remote_path);
                        match service.list_dir_path(&RemoteFilePath::new(&parent_path)) {
                            Ok(entries) => TransferJobOutput::Uploaded {
                                summary,
                                parent_path,
                                entries,
                            },
                            Err(error) => {
                                crate::features::android::labels::log_file_error(
                                    "upload-refresh",
                                    service.kind(),
                                    &id,
                                    &error,
                                );
                                let _ = finished_tx.unbounded_send(TransferJobResult {
                                    id: id.clone(),
                                    event: TransferJobEvent::FileRefreshFailed(error),
                                });
                                TransferJobOutput::Summary(summary)
                            }
                        }
                    })
            })();
            if let Err(error) = &result {
                crate::features::android::labels::log_file_error(
                    "upload",
                    service.kind(),
                    &id,
                    error,
                );
            }
            let _ = finished_tx.unbounded_send(TransferJobResult {
                id,
                event: TransferJobEvent::FileFinished(result),
            });
        });
        cx.notify();
    }

    pub(in crate::features) fn cancel_transfer_job(
        &mut self,
        job_id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(job) = self.transfer.transfer_job_mut(job_id) else {
            self.shell.set_status("transfer job not found".to_string());
            cx.notify();
            return;
        };

        if !matches!(
            job.status,
            TransferJobStatus::Queued | TransferJobStatus::Running | TransferJobStatus::Paused
        ) {
            self.shell
                .set_status(format!("transfer {} is not running", job.id));
            cx.notify();
            return;
        }

        if matches!(job.kind, TransferJobKind::RdpClipboard { .. }) {
            let id = job.id.clone();
            let session_id = job.session_id.clone();
            job.status = TransferJobStatus::Cancelling;
            job.detail = "Cancelling".to_string();
            if let Some(session_id) = session_id
                && let Err(error) = self
                    .remote_desktop
                    .cancel_clipboard_transfer(&session_id, &id)
            {
                job.status = TransferJobStatus::Cancelled;
                job.detail = error.message;
            } else if job.session_id.is_none() {
                job.status = TransferJobStatus::Cancelled;
                job.detail = "RDP session unavailable".to_string();
            }
            cx.notify();
            return;
        }

        // ZMODEM jobs have no SFTP control — cancel via the session ZMODEM state.
        if let TransferJobKind::ZmodemUpload { session_id, .. }
        | TransferJobKind::ZmodemDownload { session_id, .. } = job.kind.clone()
        {
            let id = job.id.clone();
            job.status = TransferJobStatus::Cancelled;
            job.detail = "Cancelled".to_string();
            job.progress = None;
            job.speed.reset();
            self.cancel_zmodem_transfer(&session_id, cx);
            self.shell
                .set_status(format!("ZMODEM transfer cancelled: {id}"));
            cx.notify();
            return;
        }

        if let TransferJobKind::XmodemUpload { session_id, .. }
        | TransferJobKind::YmodemUpload { session_id, .. } = job.kind.clone()
        {
            let id = job.id.clone();
            job.status = TransferJobStatus::Cancelling;
            job.detail = t!("fileTransfer.cancelling").to_string();
            job.progress = None;
            job.speed.reset();
            self.cancel_xymodem_transfer(&session_id);
            self.shell
                .set_status(t!("terminalCtx.serialUploadCancelling", id = id.as_str()).to_string());
            cx.notify();
            return;
        }

        let Some(control) = job.control.as_ref() else {
            self.shell
                .set_status(format!("transfer {} cannot be cancelled", job.id));
            cx.notify();
            return;
        };

        control.cancel();
        job.status = TransferJobStatus::Cancelling;
        job.detail = "Cancelling".to_string();
        self.shell
            .set_status(format!("remote transfer cancelling: {}", job.id));
        self.reconcile_bulk_transfer_jobs();
        cx.notify();
    }

    pub(in crate::features) fn pause_transfer_job(&mut self, job_id: &str, cx: &mut Context<Self>) {
        if self
            .transfer
            .file_operation(job_id)
            .is_some_and(|context| !context.can_pause())
        {
            return;
        }
        let Some(job) = self.transfer.transfer_job_mut(job_id) else {
            self.shell.set_status("transfer job not found".to_string());
            cx.notify();
            return;
        };

        if matches!(job.kind, TransferJobKind::Delete { .. }) {
            return;
        }

        if !matches!(
            job.status,
            TransferJobStatus::Queued | TransferJobStatus::Running
        ) {
            self.shell
                .set_status(format!("transfer {} is not running", job.id));
            cx.notify();
            return;
        }

        let Some(control) = job.control.as_ref() else {
            self.shell
                .set_status(format!("transfer {} cannot be paused", job.id));
            cx.notify();
            return;
        };

        control.pause();
        job.status = TransferJobStatus::Paused;
        job.speed.reset();
        job.detail = "Paused".to_string();
        self.shell
            .set_status(format!("remote transfer paused: {}", job.id));
        self.reconcile_bulk_transfer_jobs();
        cx.notify();
    }

    pub(in crate::features) fn resume_transfer_job(
        &mut self,
        job_id: &str,
        cx: &mut Context<Self>,
    ) {
        let managed = self.transfer.manages_bulk_job(job_id);
        let Some(job) = self.transfer.transfer_job_mut(job_id) else {
            self.shell.set_status("transfer job not found".to_string());
            cx.notify();
            return;
        };

        if job.status != TransferJobStatus::Paused {
            self.shell
                .set_status(format!("transfer {} is not paused", job.id));
            cx.notify();
            return;
        }

        let Some(control) = job.control.as_ref() else {
            self.shell
                .set_status(format!("transfer {} cannot be resumed", job.id));
            cx.notify();
            return;
        };

        if managed {
            job.status = TransferJobStatus::Queued;
        } else {
            control.resume();
            job.status = TransferJobStatus::Running;
        }
        job.speed.reset();
        job.detail = "Resuming".to_string();
        self.shell
            .set_status(format!("remote transfer resumed: {}", job.id));
        self.reconcile_bulk_transfer_jobs();
        cx.notify();
    }

    pub(in crate::features) fn retry_transfer_job(
        &mut self,
        job_id: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.retry_file_operation(&job_id, cx) {
            return;
        }
        let mut promised_source = self.transfer.promised_download_source(&job_id);
        // A user retry is a new operation. ADB keeps its device lease, while SSH
        // must acquire the currently authorized manual identity.
        if promised_source.as_ref().is_some_and(|source| {
            source.kind() == nyaterm_files::file_browser::FileBrowserBackendKind::Remote
        }) && let Some(session) = self
            .transfer
            .transfer_job(&job_id)
            .and_then(|job| job.session_id.clone())
        {
            match self.file_browser_service_for_session(&session) {
                Ok(_) => {
                    promised_source = self
                        .session
                        .weak_remote_file_service(&self.ssh_scoped_session_id(&session))
                        .map(Into::into);
                }
                Err(error) => {
                    self.shell.set_status(error.to_string());
                    cx.notify();
                    return;
                }
            }
        }
        if self
            .transfer
            .transfer_job(&job_id)
            .and_then(|job| job.session_id.as_deref())
            .is_some_and(|id| {
                self.session.metadata(id).is_none() || self.session.is_disconnected(id)
            })
        {
            let backend = promised_source
                .as_ref()
                .map(|source| source.kind())
                .unwrap_or(nyaterm_files::file_browser::FileBrowserBackendKind::Remote);
            self.report_transfer_entry_error(
                "transfer-retry",
                backend,
                &nyaterm_files::file_browser::FileTransferEntryError::Unavailable.into(),
                cx,
            );
            return;
        }
        let frozen_service = promised_source.as_ref().and_then(|source| source.upgrade());
        let service = if let Some(service) = frozen_service {
            service
        } else if self.transfer.transfer_job_is_local(&job_id) {
            nyaterm_files::file_browser::FileBrowserService::local()
        } else {
            let session_id = self
                .transfer
                .transfer_job(&job_id)
                .and_then(|job| job.session_id.clone());
            let result = match session_id {
                Some(id)
                    if !self.session.is_disconnected(&id)
                        && !self.transfer.is_promised_download(&job_id) =>
                {
                    self.file_browser_service_for_session(&id)
                }
                _ => Err(nyaterm_files::file_browser::FileTransferEntryError::Unavailable.into()),
            };
            match result {
                Ok(service) => service,
                Err(error) => {
                    let backend = promised_source
                        .as_ref()
                        .map(|source| source.kind())
                        .unwrap_or(nyaterm_files::file_browser::FileBrowserBackendKind::Remote);
                    self.report_transfer_entry_error("transfer-retry", backend, &error, cx);
                    return;
                }
            }
        };
        let Some(job) = self.transfer.transfer_job(&job_id) else {
            self.shell.set_status("transfer job not found".to_string());
            cx.notify();
            return;
        };
        let kind = job.kind.clone();
        if !matches!(
            job.status,
            TransferJobStatus::Failed | TransferJobStatus::Cancelled
        ) {
            self.shell
                .set_status(format!("transfer {job_id} is not retryable"));
            cx.notify();
            return;
        }

        match kind {
            TransferJobKind::Download {
                remote_path,
                raw_path_token,
                local_path,
            } => {
                let remote_file_path = RemoteFilePath {
                    display_path: remote_path.clone(),
                    raw_path_token,
                };
                let duplicate_policy = self.transfer.duplicate_policy();
                let transfer_options = self.sftp_transfer_options();
                let duplicate_resolver =
                    (duplicate_policy == SftpDuplicatePolicy::Ask).then(|| {
                        self.session.prompt_duplicate_broker() as Arc<dyn SftpDuplicateResolver>
                    });
                let path_options = self.transfer.transfer_job_retry_path_options(
                    &job_id,
                    SftpPathTransferOptions::new(
                        duplicate_policy,
                        duplicate_resolver,
                        transfer_options,
                    ),
                );
                let control = SftpTransferControl::new();
                let job = self
                    .transfer
                    .transfer_job_mut(&job_id)
                    .expect("transfer job was read from the same queue");
                job.status = TransferJobStatus::Running;
                job.detail = format!("Retrying download {remote_path}");
                job.entries.clear();
                job.summary = None;
                job.progress = None;
                job.speed.reset();
                job.control = Some(control.clone());
                self.shell
                    .set_status(format!("retrying remote download for {remote_path}"));
                let progress_tx = self.transfer.transfer_event_sender();
                let finished_tx = self.transfer.transfer_event_sender();
                self.submit_bulk_transfer_job(
                    "sftp-download-retry",
                    job_id.clone(),
                    finished_tx.clone(),
                    move || {
                        let _lifetime = if let Some(source) =
                            promised_source.filter(|_| path_options.is_promised_download())
                        {
                            match nyaterm_files::drag_export::DragSourceLifetime::new(
                                source,
                                vec![control.clone()],
                            ) {
                                Ok(lifetime) => Some(lifetime),
                                Err(error) => {
                                    let error = anyhow::Error::new(error);
                                    crate::features::android::labels::log_file_error(
                                        "download-retry",
                                        service.kind(),
                                        &job_id,
                                        &error,
                                    );
                                    let _ = finished_tx.unbounded_send(TransferJobResult {
                                        id: job_id,
                                        event: TransferJobEvent::FileFinished(Err(error)),
                                    });
                                    return;
                                }
                            }
                        } else {
                            None
                        };
                        let mut progress_sender =
                            TransferProgressEventSender::new(job_id.clone(), progress_tx);
                        let result = service
                            .download_remote_path_with_progress_and_path_options(
                                &remote_file_path,
                                local_path,
                                control,
                                path_options,
                                move |progress| {
                                    progress_sender.send(progress);
                                },
                            )
                            .map(TransferJobOutput::Summary);
                        if let Err(error) = &result {
                            crate::features::android::labels::log_file_error(
                                "download-retry",
                                service.kind(),
                                &job_id,
                                error,
                            );
                        }
                        let _ = finished_tx.unbounded_send(TransferJobResult {
                            id: job_id,
                            event: TransferJobEvent::FileFinished(result),
                        });
                    },
                );
            }
            TransferJobKind::Upload {
                local_path,
                remote_path,
            } => {
                let target_lock = self.transfer.upload_target_lock(&job_id);
                let duplicate_policy = self.transfer.duplicate_policy();
                let transfer_options = self.sftp_transfer_options();
                let duplicate_resolver =
                    (duplicate_policy == SftpDuplicatePolicy::Ask).then(|| {
                        self.session.prompt_duplicate_broker() as Arc<dyn SftpDuplicateResolver>
                    });
                let path_options = self.transfer.transfer_job_retry_path_options(
                    &job_id,
                    SftpPathTransferOptions::new(
                        duplicate_policy,
                        duplicate_resolver,
                        transfer_options,
                    ),
                );
                let control = SftpTransferControl::new();
                let job = self
                    .transfer
                    .transfer_job_mut(&job_id)
                    .expect("transfer job was read from the same queue");
                job.status = TransferJobStatus::Running;
                job.detail = format!("Retrying upload {}", local_path.display());
                job.entries.clear();
                job.summary = None;
                job.progress = None;
                job.speed.reset();
                job.control = Some(control.clone());
                self.shell.set_status(format!(
                    "retrying remote upload for {}",
                    local_path.display()
                ));
                let progress_tx = self.transfer.transfer_event_sender();
                let finished_tx = self.transfer.transfer_event_sender();
                self.submit_bulk_transfer_job(
                    "sftp-upload-retry",
                    job_id.clone(),
                    finished_tx.clone(),
                    move || {
                        let mut progress_sender =
                            TransferProgressEventSender::new(job_id.clone(), progress_tx);
                        let result = (|| {
                            let _target_guard = lock_upload_target(&target_lock, &control)?;
                            service
                                .upload_path_with_progress_and_path_options(
                                    local_path,
                                    &remote_path,
                                    control,
                                    path_options,
                                    move |progress| {
                                        progress_sender.send(progress);
                                    },
                                )
                                .map(|summary| {
                                    if summary.skipped {
                                        return TransferJobOutput::Summary(summary);
                                    }
                                    let parent_path =
                                        transfer_job_remote_parent_path(&summary.remote_path);
                                    match service.list_dir_path(&RemoteFilePath::new(&parent_path))
                                    {
                                        Ok(entries) => TransferJobOutput::Uploaded {
                                            summary,
                                            parent_path,
                                            entries,
                                        },
                                        Err(error) => {
                                            crate::features::android::labels::log_file_error(
                                                "upload-refresh",
                                                service.kind(),
                                                &job_id,
                                                &error,
                                            );
                                            let _ = finished_tx.unbounded_send(TransferJobResult {
                                                id: job_id.clone(),
                                                event: TransferJobEvent::FileRefreshFailed(error),
                                            });
                                            TransferJobOutput::Summary(summary)
                                        }
                                    }
                                })
                        })();
                        if let Err(error) = &result {
                            crate::features::android::labels::log_file_error(
                                "upload-retry",
                                service.kind(),
                                &job_id,
                                error,
                            );
                        }
                        let _ = finished_tx.unbounded_send(TransferJobResult {
                            id: job_id,
                            event: TransferJobEvent::FileFinished(result),
                        });
                    },
                );
            }
            _ => {
                self.shell.set_status(format!(
                    "transfer {job_id} does not support native retry yet"
                ));
                cx.notify();
                return;
            }
        }
        cx.notify();
    }

    pub(in crate::features) fn pause_all_transfer_jobs(&mut self, cx: &mut Context<Self>) {
        let active_session_id = self.session.active_id_owned();
        let changed = self
            .transfer
            .pause_visible_transfer_jobs(active_session_id.as_deref());
        self.shell.set_status(if changed == 0 {
            "no running transfer jobs to pause".to_string()
        } else {
            format!("paused {changed} transfer job(s)")
        });
        self.reconcile_bulk_transfer_jobs();
        cx.notify();
    }

    pub(in crate::features) fn resume_all_transfer_jobs(&mut self, cx: &mut Context<Self>) {
        let active_session_id = self.session.active_id_owned();
        let changed = self
            .transfer
            .resume_visible_transfer_jobs(active_session_id.as_deref());
        self.shell.set_status(if changed == 0 {
            "no paused transfer jobs to resume".to_string()
        } else {
            format!("resumed {changed} transfer job(s)")
        });
        self.reconcile_bulk_transfer_jobs();
        cx.notify();
    }

    pub(in crate::features) fn cancel_all_transfer_jobs(&mut self, cx: &mut Context<Self>) {
        let active_session_id = self.session.active_id_owned();
        let xymodem_jobs =
            visible_xymodem_job_ids(self.transfer.transfer_jobs(), active_session_id.as_deref());
        let rdp_jobs = self
            .transfer
            .transfer_jobs()
            .iter()
            .filter(|job| {
                job.is_visible_for_session(active_session_id.as_deref())
                    && job.status == TransferJobStatus::Running
                    && matches!(job.kind, TransferJobKind::RdpClipboard { .. })
            })
            .map(|job| job.id.clone())
            .collect::<Vec<_>>();
        let mut changed = self
            .transfer
            .cancel_visible_transfer_jobs(active_session_id.as_deref());
        for job_id in xymodem_jobs.into_iter().chain(rdp_jobs) {
            self.cancel_transfer_job(&job_id, cx);
            changed += 1;
        }
        self.shell.set_status(if changed == 0 {
            "no active transfer jobs to cancel".to_string()
        } else {
            format!("cancelling {changed} transfer job(s)")
        });
        self.reconcile_bulk_transfer_jobs();
        cx.notify();
    }

    pub(in crate::features) fn clear_completed_transfer_jobs(&mut self, cx: &mut Context<Self>) {
        let active_session_id = self.session.active_id_owned();
        let removed = self
            .transfer
            .clear_completed_transfer_jobs_for_session(active_session_id.as_deref());
        self.shell.set_status(if removed == 0 {
            "no completed transfer jobs to clear".to_string()
        } else {
            format!("cleared {removed} completed transfer job(s)")
        });
        cx.notify();
    }

    pub(in crate::features) fn clear_stopped_transfer_jobs(&mut self, cx: &mut Context<Self>) {
        let active_session_id = self.session.active_id_owned();
        let removed = self
            .transfer
            .clear_stopped_transfer_jobs_for_session(active_session_id.as_deref());
        self.shell.set_status(if removed == 0 {
            "no stopped transfer jobs to clear".to_string()
        } else {
            format!("cleared {removed} stopped transfer job(s)")
        });
        cx.notify();
    }
}

fn lock_upload_target<'a>(
    lock: &'a Mutex<()>,
    control: &SftpTransferControl,
) -> anyhow::Result<MutexGuard<'a, ()>> {
    loop {
        control.check_cancelled()?;
        match lock.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::Poisoned(poisoned)) => return Ok(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(25)),
        }
    }
}

#[cfg(test)]
mod upload_target_tests {
    use super::lock_upload_target;
    use nyaterm_files::sftp::SftpTransferControl;
    use std::sync::Mutex;
    use std::time::Duration;

    #[test]
    fn queued_same_target_upload_can_be_cancelled_before_it_starts() {
        let lock = Mutex::new(());
        let guard = lock.lock().unwrap();
        let control = SftpTransferControl::new();

        std::thread::scope(|scope| {
            let waiting = scope.spawn(|| lock_upload_target(&lock, &control).is_err());
            std::thread::sleep(Duration::from_millis(50));
            control.cancel();
            assert!(waiting.join().unwrap());
        });

        drop(guard);
        assert!(lock_upload_target(&lock, &SftpTransferControl::new()).is_ok());
    }
}

fn visible_xymodem_job_ids(jobs: &[TransferJobState], session_id: Option<&str>) -> Vec<String> {
    jobs.iter()
        .filter(|job| {
            job.is_visible_for_session(session_id)
                && matches!(
                    job.status,
                    TransferJobStatus::Queued
                        | TransferJobStatus::Running
                        | TransferJobStatus::Paused
                )
                && matches!(
                    job.kind,
                    TransferJobKind::XmodemUpload { .. } | TransferJobKind::YmodemUpload { .. }
                )
        })
        .map(|job| job.id.clone())
        .collect()
}

#[cfg(test)]
mod xymodem_cancel_tests {
    use super::visible_xymodem_job_ids;
    use crate::models::{TransferJobKind, TransferJobState, TransferJobStatus};

    fn job(id: &str, session_id: &str, kind: TransferJobKind) -> TransferJobState {
        TransferJobState {
            id: id.into(),
            session_id: Some(session_id.into()),
            kind,
            status: TransferJobStatus::Running,
            detail: String::new(),
            created_at_ms: 1,
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
            speed: Default::default(),
        }
    }

    #[test]
    fn bulk_cancel_selects_only_active_xymodem_jobs() {
        let mut completed = job(
            "completed",
            "a",
            TransferJobKind::XmodemUpload {
                session_id: "a".into(),
                file_name: "one".into(),
            },
        );
        completed.status = TransferJobStatus::Completed;
        let jobs = vec![
            job(
                "x",
                "a",
                TransferJobKind::XmodemUpload {
                    session_id: "a".into(),
                    file_name: "one".into(),
                },
            ),
            job(
                "y-other",
                "b",
                TransferJobKind::YmodemUpload {
                    session_id: "b".into(),
                    file_name: "two".into(),
                },
            ),
            completed,
        ];

        assert_eq!(visible_xymodem_job_ids(&jobs, Some("a")), ["x"]);
        assert_eq!(visible_xymodem_job_ids(&jobs, None), ["x", "y-other"]);
    }
}
