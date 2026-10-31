//! Transfer jobs, transfer options, path prompts and transfer widgets.

pub(in crate::features) mod clipboard_copy;
mod cwd_sync;
mod delete_runtime;
mod drag_aggregate;
mod drag_download;
pub(in crate::features) mod drag_export;
mod editor_window;
pub(in crate::features) mod external_sync_runtime;
mod external_sync_window;
pub(in crate::features) mod file_open;
pub(in crate::features) mod file_operations;
pub(in crate::features) mod preview;
mod remote_text_editor;
mod state;
pub(in crate::features) use state::TransferSessionTransferBundle;
pub(in crate::features) use state::TransferTreePresentation;
pub(in crate::features) use state::{BrowserFilterCache, BrowserFilterKey};
mod transfer_events;
pub(in crate::features) mod transfer_jobs;
mod transfer_options;
mod transfer_paths;
mod transfer_widgets;
mod tree_runtime;

use std::sync::Arc;

use nyaterm_store::{StoreBlockingClient, StoreDomain};
use nyaterm_transport::{
    FileBrowserService, RemoteFileBackendKind, RemoteFileBackendPreference,
    RemoteFileBackendPreferenceStore, RemoteFileService, SshSessionConfig,
};

use crate::features::NyaTermApp;

#[derive(Clone)]
pub(in crate::features) struct SftpJobSession {
    pub session_id: Option<String>,
    pub service: RemoteFileService,
}

struct ConnectionStoreBackendPreferences {
    store: StoreBlockingClient,
}

impl ConnectionStoreBackendPreferences {
    fn new(store: StoreBlockingClient) -> Self {
        Self { store }
    }
}

impl RemoteFileBackendPreferenceStore for ConnectionStoreBackendPreferences {
    fn load_backend(
        &self,
        endpoint_key: &str,
    ) -> anyhow::Result<Option<RemoteFileBackendPreference>> {
        let cache = self.store.request_fn(StoreDomain::Transfers, |store| {
            store.load_remote_file_backend_cache()
        })?;
        Ok(cache.entries.get(endpoint_key).and_then(|entry| {
            RemoteFileBackendKind::from_cache_name(&entry.last_working_backend).map(|backend| {
                RemoteFileBackendPreference {
                    backend,
                    sftp_unavailable: entry.sftp_unavailable,
                    last_failure_reason: entry.last_failure_reason.clone(),
                }
            })
        }))
    }

    fn save_backend(
        &self,
        endpoint_key: &str,
        preference: &RemoteFileBackendPreference,
    ) -> anyhow::Result<()> {
        let endpoint_key = endpoint_key.to_string();
        let backend = preference.backend.cache_name().to_string();
        let sftp_unavailable = preference.sftp_unavailable;
        let last_failure_reason = preference.last_failure_reason.clone();
        self.store
            .request_fn(StoreDomain::Transfers, move |store| {
                store.update_remote_file_backend_cache_entry(
                    &endpoint_key,
                    &backend,
                    sftp_unavailable,
                    last_failure_reason,
                )
            })?;
        Ok(())
    }
}

impl NyaTermApp {
    pub(in crate::features) fn submit_bulk_transfer_job(
        &mut self,
        name: &'static str,
        job_id: String,
        tx: futures::channel::mpsc::UnboundedSender<crate::models::TransferJobResult>,
        run: impl FnOnce() + Send + 'static,
    ) {
        if self.transfer.supports_bulk_admission(&job_id) {
            self.transfer
                .admit_bulk_job(&job_id, name, tx, Box::new(run));
            self.reconcile_bulk_transfer_jobs();
            return;
        }
        match self.transfer.bulk_executor() {
            Ok(executor) => {
                transfer_jobs::submit_transfer_blocking_job(executor, name, job_id, tx, run)
            }
            Err(error) => {
                let _ = tx.unbounded_send(crate::models::TransferJobResult {
                    id: job_id,
                    event: crate::models::TransferJobEvent::Finished(Err(error.to_string())),
                });
            }
        }
    }

    pub(in crate::features) fn reconcile_bulk_transfer_jobs(&mut self) {
        self.transfer.reconcile_bulk_jobs(
            self.settings.summary().transfer_download_threads,
            self.settings.summary().transfer_upload_threads,
        );
    }

    pub(in crate::features) fn submit_transfer_blocking_job(
        &self,
        name: &'static str,
        rejection_job_id: String,
        rejection_tx: futures::channel::mpsc::UnboundedSender<crate::models::TransferJobResult>,
        run: impl FnOnce() + Send + 'static,
    ) {
        transfer_jobs::submit_transfer_blocking_job(
            &self.blocking_jobs,
            name,
            rejection_job_id,
            rejection_tx,
            run,
        );
    }

    pub(in crate::features) fn remote_file_service_for_session(
        &mut self,
        session_id: &str,
        config: SshSessionConfig,
    ) -> anyhow::Result<RemoteFileService> {
        let preferences: Arc<dyn RemoteFileBackendPreferenceStore> = Arc::new(
            ConnectionStoreBackendPreferences::new(self.store_blocking_client()),
        );
        // A tmux pane leaf shares the host's SSH connection — resolve so the
        // multiplex handle lookup and the remote-files service cache key on
        // the real session (and all panes reuse the one service).
        let session_id = self.ssh_scoped_session_id(session_id);
        self.session
            .remote_file_service_for_session(&session_id, config, preferences)
    }

    pub(in crate::features) fn active_remote_file_service(
        &mut self,
    ) -> anyhow::Result<RemoteFileService> {
        let session_id = self
            .session
            .active_id_owned()
            .ok_or_else(|| anyhow::anyhow!("start an SSH session first"))?;
        let config = self
            .session
            .active_ssh_config_owned()
            .ok_or_else(|| anyhow::anyhow!("active session is not SSH"))?;
        self.remote_file_service_for_session(&session_id, config)
    }

    pub(in crate::features) fn file_browser_service_for_session(
        &mut self,
        session_id: &str,
    ) -> anyhow::Result<FileBrowserService> {
        match self.session.file_browser_backend_for_session(session_id) {
            Some(nyaterm_transport::FileBrowserBackendKind::Local) => {
                Ok(FileBrowserService::local())
            }
            Some(nyaterm_transport::FileBrowserBackendKind::Remote) => {
                let config = self
                    .session
                    .metadata(session_id)
                    .and_then(|metadata| metadata.ssh_config.clone())
                    .ok_or_else(|| {
                        anyhow::anyhow!("SSH file browser configuration is unavailable")
                    })?;
                self.remote_file_service_for_session(session_id, config)
                    .map(FileBrowserService::remote)
            }
            None => anyhow::bail!("session does not provide a file browser"),
        }
    }

    pub(in crate::features) fn active_file_browser_service(
        &mut self,
    ) -> anyhow::Result<FileBrowserService> {
        let session_id = self
            .session
            .active_id_owned()
            .ok_or_else(|| anyhow::anyhow!("start a local or SSH session first"))?;
        self.file_browser_service_for_session(&session_id)
    }

    pub(in crate::features) fn transfer_file_browser_service(
        &mut self,
        session_id: Option<&str>,
    ) -> anyhow::Result<FileBrowserService> {
        match session_id {
            Some(id) => self.file_browser_service_for_session(id),
            None => Ok(FileBrowserService::local()),
        }
    }

    pub(in crate::features) fn transfer_remote_file_service(
        &mut self,
        session_id: Option<&str>,
        config: SshSessionConfig,
    ) -> anyhow::Result<RemoteFileService> {
        let session_id = session_id
            .or_else(|| self.session.active_id())
            .ok_or_else(|| anyhow::anyhow!("source SSH session is unavailable"))?
            .to_string();
        self.remote_file_service_for_session(&session_id, config)
    }
}

pub(in crate::features) use remote_text_editor::RemoteTextEditor;
pub(in crate::features) use state::natural_compare_ascii;
#[cfg(test)]
pub(in crate::features) use state::transfer_browser_entry_is_visible;
pub(in crate::features) use state::{
    PdfPageRequest, TransferFeatureFocus, TransferFeatureState, TransferFileClipboard,
    TransferPreviewCloseOutcome,
};
pub(in crate::features) use transfer_widgets::{
    duplicate_decision_label, duplicate_policy_label, format_file_size,
};

mod workspace_editor;
