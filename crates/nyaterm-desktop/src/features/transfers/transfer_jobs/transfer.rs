use std::path::PathBuf;
use std::sync::Arc;

use gpui::{Context, Window};
use nyaterm_transport::{
    RemoteFilePath, SftpDuplicatePolicy, SftpDuplicateResolver, SftpPathTransferOptions,
    SftpTransferControl,
};

use crate::features::NyaTermApp;
use crate::features::transfers::SftpJobSession;
use crate::models::{
    NavItem, TransferJobEvent, TransferJobKind, TransferJobOutput, TransferJobResult,
    TransferJobState, TransferJobStatus,
};

use super::helpers::{
    TransferProgressEventSender, log_sftp_upload_job_failure, submit_transfer_blocking_job,
    transfer_job_remote_parent_path,
};

impl NyaTermApp {
    pub(in crate::features) fn start_sftp_download_job_for_target(
        &mut self,
        remote_path: RemoteFilePath,
        local_path: PathBuf,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session.active_ssh_config_owned().is_none() {
            self.shell
                .set_status("start an SSH session first".to_string());
            self.ensure_panel_open(NavItem::Transfers);
            cx.notify();
            return;
        }
        let duplicate_policy = self.transfer.duplicate_policy();
        let duplicate_resolver = (duplicate_policy == SftpDuplicatePolicy::Ask)
            .then(|| self.session.prompt_duplicate_broker() as Arc<dyn SftpDuplicateResolver>);
        let transfer_options = self.sftp_transfer_options();
        let service = match self.active_remote_file_service() {
            Ok(service) => service,
            Err(error) => {
                self.shell.set_status(error.to_string());
                cx.notify();
                return;
            }
        };
        let session = SftpJobSession {
            session_id: self.session.active_id_owned(),
            service,
        };
        self.enqueue_sftp_download_job_for_target(
            session,
            remote_path,
            local_path,
            SftpPathTransferOptions::new(duplicate_policy, duplicate_resolver, transfer_options),
            cx,
        );
    }

    pub(in crate::features) fn enqueue_sftp_download_job_for_target(
        &mut self,
        session: SftpJobSession,
        remote_path: RemoteFilePath,
        local_path: PathBuf,
        path_options: SftpPathTransferOptions,
        cx: &mut Context<Self>,
    ) {
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
        });
        let path_options = self
            .transfer
            .bind_transfer_job_path_options(&id, path_options);
        self.shell
            .set_status(format!("remote download started for {display_path}"));
        let progress_tx = self.transfer.transfer_event_sender();
        let finished_tx = self.transfer.transfer_event_sender();
        submit_transfer_blocking_job(
            &self.blocking_jobs,
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
                    .map(TransferJobOutput::Summary)
                    .map_err(|error| error.to_string());
                let _ = finished_tx.unbounded_send(TransferJobResult {
                    id,
                    event: TransferJobEvent::Finished(result),
                });
            },
        );
        cx.notify();
    }

    pub(in crate::features) fn enqueue_sftp_upload_job_for_target(
        &mut self,
        session: SftpJobSession,
        local_path: PathBuf,
        remote_path: String,
        path_options: SftpPathTransferOptions,
        cx: &mut Context<Self>,
    ) {
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
        });
        let path_options = self
            .transfer
            .bind_transfer_job_path_options(&id, path_options);
        self.shell.set_status(format!(
            "remote upload started for {}",
            local_path.display()
        ));
        let progress_tx = self.transfer.transfer_event_sender();
        let finished_tx = self.transfer.transfer_event_sender();
        submit_transfer_blocking_job(
            &self.blocking_jobs,
            "sftp-upload",
            id.clone(),
            finished_tx.clone(),
            move || {
                let mut progress_sender = TransferProgressEventSender::new(id.clone(), progress_tx);
                let service = session.service;
                let result = service
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
                        match service.list_dir(&parent_path) {
                            Ok(entries) => TransferJobOutput::Uploaded {
                                summary,
                                parent_path,
                                entries,
                            },
                            Err(_) => TransferJobOutput::Summary(summary),
                        }
                    });
                if let Err(error) = &result {
                    log_sftp_upload_job_failure(&id, error);
                }
                let result = result.map_err(|error| error.to_string());
                let _ = finished_tx.unbounded_send(TransferJobResult {
                    id,
                    event: TransferJobEvent::Finished(result),
                });
            },
        );
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
            TransferJobStatus::Running | TransferJobStatus::Paused
        ) {
            self.shell
                .set_status(format!("transfer {} is not running", job.id));
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
            self.cancel_zmodem_transfer(&session_id, cx);
            self.shell
                .set_status(format!("ZMODEM transfer cancelled: {id}"));
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
        cx.notify();
    }

    pub(in crate::features) fn pause_transfer_job(&mut self, job_id: &str, cx: &mut Context<Self>) {
        let Some(job) = self.transfer.transfer_job_mut(job_id) else {
            self.shell.set_status("transfer job not found".to_string());
            cx.notify();
            return;
        };

        if job.status != TransferJobStatus::Running {
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
        job.detail = "Paused".to_string();
        self.shell
            .set_status(format!("remote transfer paused: {}", job.id));
        cx.notify();
    }

    pub(in crate::features) fn resume_transfer_job(
        &mut self,
        job_id: &str,
        cx: &mut Context<Self>,
    ) {
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

        control.resume();
        job.status = TransferJobStatus::Running;
        job.detail = "Resuming".to_string();
        self.shell
            .set_status(format!("remote transfer resumed: {}", job.id));
        cx.notify();
    }

    pub(in crate::features) fn retry_transfer_job(
        &mut self,
        job_id: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session.active_ssh_config_owned().is_none() {
            self.shell
                .set_status("start an SSH session first".to_string());
            self.ensure_panel_open(NavItem::Transfers);
            cx.notify();
            return;
        }
        let service = match self.active_remote_file_service() {
            Ok(service) => service,
            Err(error) => {
                self.shell.set_status(error.to_string());
                cx.notify();
                return;
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
                job.control = Some(control.clone());
                self.shell
                    .set_status(format!("retrying remote download for {remote_path}"));
                let progress_tx = self.transfer.transfer_event_sender();
                let finished_tx = self.transfer.transfer_event_sender();
                submit_transfer_blocking_job(
                    &self.blocking_jobs,
                    "sftp-download-retry",
                    job_id.clone(),
                    finished_tx.clone(),
                    move || {
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
                            .map(TransferJobOutput::Summary)
                            .map_err(|error| error.to_string());
                        let _ = finished_tx.unbounded_send(TransferJobResult {
                            id: job_id,
                            event: TransferJobEvent::Finished(result),
                        });
                    },
                );
            }
            TransferJobKind::Upload {
                local_path,
                remote_path,
            } => {
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
                job.control = Some(control.clone());
                self.shell.set_status(format!(
                    "retrying remote upload for {}",
                    local_path.display()
                ));
                let progress_tx = self.transfer.transfer_event_sender();
                let finished_tx = self.transfer.transfer_event_sender();
                submit_transfer_blocking_job(
                    &self.blocking_jobs,
                    "sftp-upload-retry",
                    job_id.clone(),
                    finished_tx.clone(),
                    move || {
                        let mut progress_sender =
                            TransferProgressEventSender::new(job_id.clone(), progress_tx);
                        let result = service
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
                                match service.list_dir(&parent_path) {
                                    Ok(entries) => TransferJobOutput::Uploaded {
                                        summary,
                                        parent_path,
                                        entries,
                                    },
                                    Err(_) => TransferJobOutput::Summary(summary),
                                }
                            });
                        if let Err(error) = &result {
                            log_sftp_upload_job_failure(&job_id, error);
                        }
                        let result = result.map_err(|error| error.to_string());
                        let _ = finished_tx.unbounded_send(TransferJobResult {
                            id: job_id,
                            event: TransferJobEvent::Finished(result),
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
        cx.notify();
    }

    pub(in crate::features) fn cancel_all_transfer_jobs(&mut self, cx: &mut Context<Self>) {
        let active_session_id = self.session.active_id_owned();
        let changed = self
            .transfer
            .cancel_visible_transfer_jobs(active_session_id.as_deref());
        self.shell.set_status(if changed == 0 {
            "no active transfer jobs to cancel".to_string()
        } else {
            format!("cancelling {changed} transfer job(s)")
        });
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
