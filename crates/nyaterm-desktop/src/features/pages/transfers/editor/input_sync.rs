use super::helpers::{external_editor_watch_key, upload_external_editor_file};
use crate::features::NyaTermApp;
use crate::models::TransferExternalSyncPromptState;
use gpui::Context;
use std::path::PathBuf;

impl NyaTermApp {
    pub(in crate::features) fn upload_external_editor_sync(
        &mut self,
        session_id: Option<String>,
        job_id: String,
        remote_path: String,
        raw_path_token: Option<String>,
        local_path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        self.spawn_external_editor_sync_upload(
            session_id,
            job_id,
            remote_path,
            raw_path_token,
            local_path,
        );
        cx.notify();
    }

    pub(in crate::features) fn spawn_external_editor_sync_upload(
        &mut self,
        session_id: Option<String>,
        job_id: String,
        remote_path: String,
        raw_path_token: Option<String>,
        local_path: PathBuf,
    ) {
        if session_id
            .as_deref()
            .is_some_and(|id| self.file_sudo_status(id).is_busy())
        {
            self.transfer.insert_external_sync_prompt(
                job_id.clone(),
                TransferExternalSyncPromptState {
                    session_id,
                    job_id,
                    remote_path,
                    raw_path_token,
                    local_path,
                },
            );
            return;
        }
        let config = session_id
            .as_deref()
            .and_then(|session_id| self.session.metadata(session_id))
            .and_then(|metadata| metadata.ssh_config.clone())
            .or_else(|| {
                (session_id.as_deref() == self.session.active_id())
                    .then(|| self.session.active_ssh_config_owned())
                    .flatten()
            });
        let Some(config) = config else {
            self.shell
                .set_status("start an SSH session before syncing external edits".to_string());
            return;
        };
        let service = match self.transfer_remote_file_service(session_id.as_deref(), config) {
            Ok(service) => service,
            Err(error) => {
                self.shell.set_status(error.to_string());
                return;
            }
        };
        let transfer_tx = self.transfer.transfer_event_sender();
        let transfer_options = self.sftp_transfer_options();
        let control = self.transfer.external_editor_upload_control();
        if let Some(job) = self.transfer.transfer_job_mut(&job_id) {
            job.control = Some(control.clone());
        }
        let task_guard = self
            .transfer
            .begin_external_file_task(session_id.as_deref().unwrap_or_default());
        self.submit_bulk_transfer_job(
            "sftp-external-editor-upload",
            job_id.clone(),
            transfer_tx.clone(),
            move || {
                let _task_guard = task_guard;
                upload_external_editor_file(
                    service,
                    &job_id,
                    nyaterm_files::sftp::RemoteFilePath {
                        display_path: remote_path,
                        raw_path_token,
                    },
                    &local_path,
                    control,
                    transfer_options,
                    &transfer_tx,
                );
            },
        );
    }

    pub(in crate::features) fn active_external_editor_sync_prompt(
        &self,
    ) -> Option<(String, TransferExternalSyncPromptState)> {
        let active_session_id = self.session.active_id()?;
        self.transfer.active_external_sync_prompt(active_session_id)
    }

    pub(in crate::features) fn upload_external_editor_sync_prompt(
        &mut self,
        prompt_id: &str,
        always: bool,
        cx: &mut Context<Self>,
    ) {
        if self
            .transfer
            .external_sync_prompt(prompt_id)
            .and_then(|p| p.session_id.as_deref())
            .is_some_and(|id| self.file_sudo_status(id).is_busy())
        {
            self.shell
                .set_status(rust_i18n::t!("fileSudo.changing").to_string());
            cx.notify();
            return;
        }
        let always_watch_key = if always {
            self.transfer
                .external_sync_prompt(prompt_id)
                .map(|prompt| external_editor_watch_key(&prompt.remote_path, &prompt.local_path))
        } else {
            None
        };
        let Some(prompt) = self
            .transfer
            .take_external_sync_prompt_for_upload(prompt_id, always_watch_key)
        else {
            cx.notify();
            return;
        };
        self.upload_external_editor_sync(
            prompt.session_id,
            prompt.job_id,
            prompt.remote_path,
            prompt.raw_path_token,
            prompt.local_path,
            cx,
        );
    }

    pub(in crate::features) fn ignore_external_editor_sync_prompt(
        &mut self,
        prompt_id: &str,
        cx: &mut Context<Self>,
    ) {
        self.transfer.dismiss_external_sync_prompt(prompt_id);
        self.shell
            .set_status("external edit sync skipped".to_string());
        cx.notify();
    }
}
