use rust_i18n::t;

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use gpui::Context;
use nyaterm_core::{
    CLOUD_SYNC_HISTORY_LIMIT, CloudSyncError, CloudSyncHistoryEntry, CloudSyncOutcome,
    CloudSyncSettings, CloudSyncState, LocalCloudSyncOptions, LocalDirectoryRemote,
    RemoteSyncPointer, append_cloud_sync_history, cleanup_sync_snapshots_with_remote,
    pull_local_snapshot, push_local_snapshot, read_cloud_sync_history,
    recover_local_current_snapshot,
};

use crate::blocking_jobs::{BlockingJobScheduler, JobRejected, JobTask};
use crate::features::formatting::{cloud_sync_history_status, configured_cloud_sync_provider};
use crate::features::{NyaTermApp, runtime_jobs::await_blocking_result};
use nyaterm_store::StoreDomain;

use super::super::{
    cleanup_provider_snapshots, pull_provider_snapshot, push_provider_snapshot,
    recover_provider_snapshot, test_provider_connection,
};

impl NyaTermApp {
    pub(in crate::features) fn run_provider_cloud_sync_test(&mut self, cx: &mut Context<Self>) {
        if self.block_cloud_sync_for_settings_draft(cx) {
            return;
        }
        if !self.begin_cloud_sync_job(cx) {
            return;
        }
        let settings = self.cloud_sync.settings().clone();
        let local_store = self.store_blocking_client();
        let provider = configured_cloud_sync_provider(&settings);
        self.cloud_sync
            .set_status(t!("settings.syncTestingProvider", provider = provider));
        self.shell
            .set_status(t!("settings.syncConnectionTestStarted"));
        let task = self.blocking_jobs.submit_task("cloud-sync-test", move |_| {
            test_provider_connection(&local_store, &settings)?;
            record_cloud_sync_connection_check(&local_store, current_time_ms())
        });
        cx.spawn(async move |this, cx| {
            let result = await_cloud_sync_job(task).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(checked) => {
                        let status = t!("settings.syncTestSuccess").to_string();
                        this.cloud_sync.complete_job(checked, status.clone());
                        this.shell.set_status(status);
                    }
                    Err(error) => {
                        let status = t!("settings.syncTestFailed", detail = error).to_string();
                        this.cloud_sync.fail_job_with_status(status);
                        this.shell.set_status(this.cloud_sync.status().to_string());
                    }
                }
                this.request_settings_panel_refresh(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn run_local_cloud_sync_push(
        &mut self,
        master_password: nyaterm_core::SecretString,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        if self.block_cloud_sync_for_settings_draft(cx) {
            return;
        }
        if !self.begin_cloud_sync_job(cx) {
            return;
        }
        let options = self.local_cloud_sync_options(master_password);
        let cleanup_options = options.clone();
        let state = self.cloud_sync.state().clone();
        let local_store = self.store_blocking_client();
        let started_at = Instant::now();
        self.cloud_sync.set_status(if force {
            t!("settings.syncForcePushingLocal")
        } else {
            t!("settings.syncPushingLocal")
        });
        self.shell.set_status(t!("settings.syncPushStarted"));
        let task = self
            .blocking_jobs
            .submit_task("cloud-sync-local-push", move |_| {
                push_local_snapshot(&local_store, &options, &state, force)
            });
        cx.spawn(async move |this, cx| {
            let result = await_cloud_sync_job(task).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(result) => {
                        schedule_cloud_sync_cleanup(
                            this.store_blocking_client(),
                            this.blocking_jobs.clone(),
                            None,
                            cleanup_options,
                            result.pointer.clone(),
                            cx,
                        );
                        let message = cloud_sync_outcome_message(result.outcome);
                        let mut history = CloudSyncHistoryEntry::sync(
                            "success",
                            if force {
                                "manual_force_push"
                            } else {
                                "manual_push"
                            },
                            Some(result.status.provider.clone()),
                            result
                                .pointer
                                .as_ref()
                                .map(|pointer| pointer.revision_id.clone()),
                            message.clone(),
                        );
                        history.duration_ms = Some(started_at.elapsed().as_millis() as u64);
                        this.queue_cloud_sync_history_refresh(Some(history), cx);
                        this.cloud_sync.complete_job(result.state, message.clone());
                        this.shell.set_status(message);
                    }
                    Err(error) => {
                        let status = cloud_sync_history_status(&error);
                        this.cloud_sync.fail_job(
                            &error,
                            t!("settings.syncPushFailed", detail = error).to_string(),
                            "local_directory".to_string(),
                            false,
                        );
                        this.shell.set_status(this.cloud_sync.status().to_string());
                        let mut history = CloudSyncHistoryEntry::sync(
                            status,
                            if force {
                                "manual_force_push"
                            } else {
                                "manual_push"
                            },
                            Some("local_directory".to_string()),
                            None,
                            this.cloud_sync.status().to_string(),
                        );
                        history.duration_ms = Some(started_at.elapsed().as_millis() as u64);
                        this.queue_cloud_sync_history_refresh(Some(history), cx);
                    }
                }
                this.request_settings_panel_refresh(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn run_local_cloud_sync_pull(
        &mut self,
        master_password: nyaterm_core::SecretString,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        if self.block_cloud_sync_for_settings_draft(cx) {
            return;
        }
        if !self.begin_cloud_sync_job(cx) {
            return;
        }
        let options = self.local_cloud_sync_options(master_password);
        let cleanup_options = options.clone();
        let state = self.cloud_sync.state().clone();
        let local_store = self.store_blocking_client();
        let started_at = Instant::now();
        self.cloud_sync.set_status(if force {
            t!("settings.syncForcePullingLocal")
        } else {
            t!("settings.syncPullingLocal")
        });
        self.shell.set_status(t!("settings.syncPullStarted"));
        let task = self
            .blocking_jobs
            .submit_task("cloud-sync-local-pull", move |_| {
                pull_local_snapshot(&local_store, &options, &state, force)
            });
        cx.spawn(async move |this, cx| {
            let result = await_cloud_sync_job(task).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(result) => {
                        schedule_cloud_sync_cleanup(
                            this.store_blocking_client(),
                            this.blocking_jobs.clone(),
                            None,
                            cleanup_options,
                            result.pointer.clone(),
                            cx,
                        );
                        let message = cloud_sync_outcome_message(result.outcome);
                        let mut history = CloudSyncHistoryEntry::sync(
                            "success",
                            if force {
                                "manual_force_pull"
                            } else {
                                "manual_pull"
                            },
                            Some(result.status.provider.clone()),
                            result
                                .pointer
                                .as_ref()
                                .map(|pointer| pointer.revision_id.clone()),
                            message.clone(),
                        );
                        history.duration_ms = Some(started_at.elapsed().as_millis() as u64);
                        this.queue_cloud_sync_history_refresh(Some(history), cx);
                        this.cloud_sync.complete_job(result.state, message.clone());
                        this.shell.set_status(message);
                        this.refresh_store_from_runtime_and_sync_theme(cx);
                    }
                    Err(error) => {
                        let status = cloud_sync_history_status(&error);
                        this.cloud_sync.fail_job(
                            &error,
                            t!("settings.syncPullFailed", detail = error).to_string(),
                            "local_directory".to_string(),
                            false,
                        );
                        this.shell.set_status(this.cloud_sync.status().to_string());
                        let mut history = CloudSyncHistoryEntry::sync(
                            status,
                            if force {
                                "manual_force_pull"
                            } else {
                                "manual_pull"
                            },
                            Some("local_directory".to_string()),
                            None,
                            this.cloud_sync.status().to_string(),
                        );
                        history.duration_ms = Some(started_at.elapsed().as_millis() as u64);
                        this.queue_cloud_sync_history_refresh(Some(history), cx);
                    }
                }
                this.request_settings_panel_refresh(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn run_provider_cloud_sync_push(
        &mut self,
        master_password: nyaterm_core::SecretString,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        if self.block_cloud_sync_for_settings_draft(cx) {
            return;
        }
        if !self.begin_cloud_sync_job(cx) {
            return;
        }
        let options = self.local_cloud_sync_options(master_password);
        let cleanup_options = options.clone();
        let state = self.cloud_sync.state().clone();
        let settings = self.cloud_sync.settings().clone();
        let local_store = self.store_blocking_client();
        let cleanup_settings = settings.clone();
        let result_settings = settings.clone();
        let provider = configured_cloud_sync_provider(&settings);
        let started_at = Instant::now();
        self.cloud_sync.set_status(if force {
            t!("settings.syncForcePushingProvider", provider = provider)
        } else {
            t!("settings.syncPushingProvider", provider = provider)
        });
        self.shell
            .set_status(t!("settings.syncProviderPushStarted"));
        let task = self
            .blocking_jobs
            .submit_task("cloud-sync-provider-push", move |_| {
                push_provider_snapshot(&local_store, &settings, &options, &state, force)
            });
        cx.spawn(async move |this, cx| {
            let result = await_cloud_sync_job(task).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(result) => {
                        schedule_cloud_sync_cleanup(
                            this.store_blocking_client(),
                            this.blocking_jobs.clone(),
                            Some(cleanup_settings),
                            cleanup_options,
                            result.pointer.clone(),
                            cx,
                        );
                        let message = cloud_sync_outcome_message(result.outcome);
                        let mut history = CloudSyncHistoryEntry::sync(
                            "success",
                            if force {
                                "manual_provider_force_push"
                            } else {
                                "manual_provider_push"
                            },
                            Some(result.status.provider.clone()),
                            result
                                .pointer
                                .as_ref()
                                .map(|pointer| pointer.revision_id.clone()),
                            message.clone(),
                        );
                        history.duration_ms = Some(started_at.elapsed().as_millis() as u64);
                        this.queue_cloud_sync_history_refresh(Some(history), cx);
                        this.cloud_sync.complete_job(result.state, message.clone());
                        this.shell.set_status(message);
                    }
                    Err(error) => {
                        let status = cloud_sync_history_status(&error);
                        this.cloud_sync.fail_job(
                            &error,
                            t!("settings.syncProviderPushFailed", detail = error).to_string(),
                            configured_cloud_sync_provider(&result_settings),
                            true,
                        );
                        this.shell.set_status(this.cloud_sync.status().to_string());
                        let mut history = CloudSyncHistoryEntry::sync(
                            status,
                            if force {
                                "manual_provider_force_push"
                            } else {
                                "manual_provider_push"
                            },
                            Some(configured_cloud_sync_provider(&result_settings)),
                            None,
                            this.cloud_sync.status().to_string(),
                        );
                        history.duration_ms = Some(started_at.elapsed().as_millis() as u64);
                        this.queue_cloud_sync_history_refresh(Some(history), cx);
                    }
                }
                this.request_settings_panel_refresh(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn run_provider_cloud_sync_pull(
        &mut self,
        master_password: nyaterm_core::SecretString,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        if self.block_cloud_sync_for_settings_draft(cx) {
            return;
        }
        if !self.begin_cloud_sync_job(cx) {
            return;
        }
        let options = self.local_cloud_sync_options(master_password);
        let cleanup_options = options.clone();
        let state = self.cloud_sync.state().clone();
        let settings = self.cloud_sync.settings().clone();
        let local_store = self.store_blocking_client();
        let cleanup_settings = settings.clone();
        let result_settings = settings.clone();
        let provider = configured_cloud_sync_provider(&settings);
        let started_at = Instant::now();
        self.cloud_sync.set_status(if force {
            t!("settings.syncForcePullingProvider", provider = provider)
        } else {
            t!("settings.syncPullingProvider", provider = provider)
        });
        self.shell
            .set_status(t!("settings.syncProviderPullStarted"));
        let task = self
            .blocking_jobs
            .submit_task("cloud-sync-provider-pull", move |_| {
                pull_provider_snapshot(&local_store, &settings, &options, &state, force)
            });
        cx.spawn(async move |this, cx| {
            let result = await_cloud_sync_job(task).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(result) => {
                        schedule_cloud_sync_cleanup(
                            this.store_blocking_client(),
                            this.blocking_jobs.clone(),
                            Some(cleanup_settings),
                            cleanup_options,
                            result.pointer.clone(),
                            cx,
                        );
                        let message = cloud_sync_outcome_message(result.outcome);
                        let mut history = CloudSyncHistoryEntry::sync(
                            "success",
                            if force {
                                "manual_provider_force_pull"
                            } else {
                                "manual_provider_pull"
                            },
                            Some(result.status.provider.clone()),
                            result
                                .pointer
                                .as_ref()
                                .map(|pointer| pointer.revision_id.clone()),
                            message.clone(),
                        );
                        history.duration_ms = Some(started_at.elapsed().as_millis() as u64);
                        this.queue_cloud_sync_history_refresh(Some(history), cx);
                        this.cloud_sync.complete_job(result.state, message.clone());
                        this.shell.set_status(message);
                        this.refresh_store_from_runtime_and_sync_theme(cx);
                    }
                    Err(error) => {
                        let status = cloud_sync_history_status(&error);
                        this.cloud_sync.fail_job(
                            &error,
                            t!("settings.syncProviderPullFailed", detail = error).to_string(),
                            configured_cloud_sync_provider(&result_settings),
                            true,
                        );
                        this.shell.set_status(this.cloud_sync.status().to_string());
                        let mut history = CloudSyncHistoryEntry::sync(
                            status,
                            if force {
                                "manual_provider_force_pull"
                            } else {
                                "manual_provider_pull"
                            },
                            Some(configured_cloud_sync_provider(&result_settings)),
                            None,
                            this.cloud_sync.status().to_string(),
                        );
                        history.duration_ms = Some(started_at.elapsed().as_millis() as u64);
                        this.queue_cloud_sync_history_refresh(Some(history), cx);
                    }
                }
                this.request_settings_panel_refresh(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn local_cloud_sync_options(
        &self,
        master_password: nyaterm_core::SecretString,
    ) -> LocalCloudSyncOptions {
        LocalCloudSyncOptions {
            config_dir: self.runtime.config_dir().to_path_buf(),
            portable_key_path: self.runtime.portable_key_path().map(ToOwned::to_owned),
            remote_dir: self.runtime.config_dir().join("cloud-sync-local"),
            remote_root: self.cloud_sync.settings().remote_root.clone(),
            device_id: self.cloud_sync.state().device_id.clone(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            master_password,
            enabled: true,
        }
    }

    pub(in crate::features) fn run_cloud_sync_recovery(
        &mut self,
        master_password: nyaterm_core::SecretString,
        provider_action: bool,
        cx: &mut Context<Self>,
    ) {
        if self.block_cloud_sync_for_settings_draft(cx) || !self.begin_cloud_sync_job(cx) {
            return;
        }
        let options = self.local_cloud_sync_options(master_password);
        let cleanup_options = options.clone();
        let settings = self.cloud_sync.settings().clone();
        let local_store = self.store_blocking_client();
        let cleanup_settings = settings.clone();
        let provider = if provider_action {
            configured_cloud_sync_provider(&settings)
        } else {
            "local_directory".to_string()
        };
        let started_at = Instant::now();
        self.cloud_sync.set_status(t!("settings.syncRecovering"));
        self.shell.set_status(t!("settings.syncRecoveryStarted"));
        let task = self
            .blocking_jobs
            .submit_task("cloud-sync-recover", move |_| {
                if provider_action {
                    recover_provider_snapshot(&local_store, &settings, &options)
                } else {
                    recover_local_current_snapshot(&local_store, &options)
                }
            });
        cx.spawn(async move |this, cx| {
            let result = await_cloud_sync_job(task).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(result) => {
                        schedule_cloud_sync_cleanup(
                            this.store_blocking_client(),
                            this.blocking_jobs.clone(),
                            provider_action.then_some(cleanup_settings),
                            cleanup_options,
                            result.pointer.clone(),
                            cx,
                        );
                        let message = cloud_sync_outcome_message(result.outcome);
                        let mut history = CloudSyncHistoryEntry::sync(
                            "success",
                            "recover_current_remote",
                            Some(result.status.provider.clone()),
                            result
                                .pointer
                                .as_ref()
                                .map(|pointer| pointer.revision_id.clone()),
                            message.clone(),
                        );
                        history.duration_ms = Some(started_at.elapsed().as_millis() as u64);
                        this.queue_cloud_sync_history_refresh(Some(history), cx);
                        this.cloud_sync.complete_job(result.state, message.clone());
                        this.shell.set_status(message);
                        this.refresh_store_from_runtime_and_sync_theme(cx);
                    }
                    Err(error) => {
                        let status = cloud_sync_history_status(&error);
                        this.cloud_sync.fail_job(
                            &error,
                            t!("settings.syncRecoveryFailed", detail = error).to_string(),
                            provider.clone(),
                            provider_action,
                        );
                        let mut history = CloudSyncHistoryEntry::sync(
                            status,
                            "recover_current_remote",
                            Some(provider.clone()),
                            None,
                            this.cloud_sync.status().to_string(),
                        );
                        history.duration_ms = Some(started_at.elapsed().as_millis() as u64);
                        this.queue_cloud_sync_history_refresh(Some(history), cx);
                        this.shell.set_status(this.cloud_sync.status().to_string());
                    }
                }
                this.request_settings_panel_refresh(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn begin_cloud_sync_job(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.cloud_sync.begin_job() {
            self.shell
                .set_status(t!("settings.syncOperationInProgress"));
            cx.notify();
            return false;
        }
        true
    }

    pub(in crate::features) fn queue_cloud_sync_history_refresh(
        &mut self,
        entry: Option<CloudSyncHistoryEntry>,
        cx: &mut Context<Self>,
    ) {
        let log_dir = self.runtime.log_dir().to_path_buf();
        let retention_days = self.settings.summary().diagnostics_retention_days;
        let task = self
            .blocking_jobs
            .submit_task("cloud-sync-history", move |_| {
                if let Some(entry) = entry.as_ref() {
                    append_cloud_sync_history(&log_dir, entry)?;
                }
                read_cloud_sync_history(&log_dir, retention_days, CLOUD_SYNC_HISTORY_LIMIT)
            });
        cx.spawn(async move |this, cx| {
            let result = await_blocking_result(task).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(history) => this.cloud_sync.replace_history(history),
                    Err(error) => this
                        .cloud_sync
                        .set_status(t!("settings.syncHistoryRefreshFailed", detail = error)),
                }
                this.request_settings_panel_refresh(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::features) fn toggle_cloud_sync_history_details(
        &mut self,
        entry_id: &str,
        cx: &mut Context<Self>,
    ) {
        self.cloud_sync.toggle_history_details(entry_id);
        cx.notify();
    }
}

fn schedule_cloud_sync_cleanup(
    local_store: nyaterm_store::StoreBlockingClient,
    scheduler: BlockingJobScheduler,
    settings: Option<CloudSyncSettings>,
    options: LocalCloudSyncOptions,
    latest: Option<RemoteSyncPointer>,
    cx: &mut Context<NyaTermApp>,
) {
    let provider = settings
        .as_ref()
        .map(configured_cloud_sync_provider)
        .unwrap_or_else(|| "local_directory".to_string());
    let latest_revision = latest.as_ref().map(|pointer| pointer.revision_id.clone());
    let task = scheduler.submit_task("cloud-sync-cleanup", move |_| {
        let result = if let Some(settings) = settings {
            cleanup_provider_snapshots(&local_store, &settings, &options, latest.as_ref())
        } else {
            let remote = LocalDirectoryRemote::new(options.remote_dir.clone());
            cleanup_sync_snapshots_with_remote(&local_store, &options, &remote, latest.as_ref())
        };
        // Persist the attempt only if a newer sync has not replaced this revision.
        if let Some(revision) = latest_revision {
            let attempted_at = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let _ = local_store.request_fn(StoreDomain::CloudSync, move |store| {
                let mut state = store.load_cloud_sync_state()?;
                if state.last_applied_remote_revision.as_deref() == Some(&revision) {
                    state.last_gc_attempt_at_ms = Some(attempted_at);
                    store.save_cloud_sync_state(&state)?;
                }
                Ok(())
            });
        }
        result
    });
    cx.spawn(async move |_, _| {
        if await_cloud_sync_job(task).await.is_err() {
            tracing::warn!(
                provider = %provider,
                "cloud sync snapshot cleanup failed after a successful sync"
            );
        }
    })
    .detach();
}

async fn await_cloud_sync_job<T>(
    task: Result<JobTask<Result<T, CloudSyncError>>, JobRejected>,
) -> Result<T, CloudSyncError> {
    match task {
        Ok(task) => task
            .await
            .map_err(|error| CloudSyncError::Remote(error.to_string()))?,
        Err(error) => Err(CloudSyncError::Remote(error.to_string())),
    }
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn record_cloud_sync_connection_check(
    local_store: &nyaterm_store::StoreBlockingClient,
    checked_at: u64,
) -> Result<CloudSyncState, CloudSyncError> {
    local_store
        .request_fn(StoreDomain::CloudSync, move |store| {
            let mut state = store.load_cloud_sync_state()?;
            state.last_checked_at_ms = Some(checked_at);
            store.save_cloud_sync_state(&state)?;
            Ok(state)
        })
        .map_err(|error| CloudSyncError::LocalStore(format!("{}: {error}", error.category())))
}

fn cloud_sync_outcome_message(outcome: CloudSyncOutcome) -> String {
    match outcome {
        CloudSyncOutcome::UpToDate => t!("settings.syncUpToDate").to_string(),
        CloudSyncOutcome::Uploaded => t!("settings.syncPushSuccess").to_string(),
        CloudSyncOutcome::Downloaded => t!("settings.syncPullSuccess").to_string(),
        CloudSyncOutcome::Recovered => t!("settings.syncRecoverCurrentSuccess").to_string(),
    }
}
