use rust_i18n::t;

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use gpui::Context;
use nyaterm_core::{
    CLOUD_SYNC_HISTORY_LIMIT, CloudSyncError, CloudSyncHistoryEntry, CloudSyncOutcome,
    CloudSyncSettings, CloudSyncState, LocalCloudSyncOptions, LocalDirectoryRemote,
    RemoteSyncPointer, append_cloud_sync_history, read_cloud_sync_history,
};

use crate::blocking_jobs::BlockingJobScheduler;
use crate::features::formatting::{cloud_sync_history_status, configured_cloud_sync_provider};
use crate::features::sync::operation::{await_cloud_sync_job, lock_cloud_sync_operation};
use crate::features::{NyaTermApp, runtime_jobs::await_blocking_result};
use nyaterm_core::cloud_sync::{
    self,
    control::{CloudSyncCancellation, CloudSyncOperationControl},
};
use nyaterm_store::StoreDomain;

use super::super::{
    cleanup_provider_snapshots, pull_provider_snapshot, push_provider_snapshot,
    recover_provider_snapshot, test_provider_connection,
};

impl NyaTermApp {
    pub(crate) fn show_cloud_sync_timeout(
        &mut self,
        phase: CloudSyncCancellation,
        cx: &mut Context<Self>,
    ) {
        if !self.cloud_sync.operations.accepting() {
            return;
        }
        let status = match phase {
            CloudSyncCancellation::Stopping => t!("settings.syncTimeoutStopping").to_string(),
            CloudSyncCancellation::FinishingCommit => {
                t!("settings.syncTimeoutFinishing").to_string()
            }
        };
        self.cloud_sync.set_status(status.clone());
        self.shell.set_status(status);
        self.request_settings_panel_refresh(cx);
        cx.notify();
    }
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
        let control = CloudSyncOperationControl::default();
        let Some(registration) =
            self.cloud_sync
                .operations
                .register("cloud-sync-manual", vec![control.clone()], None)
        else {
            self.cloud_sync.cancel_job_for_shutdown();
            return;
        };
        let operation_control = control.clone();
        let task = self.blocking_jobs.submit_task("cloud-sync-test", move |_| {
            let _operation = lock_cloud_sync_operation(&operation_control)?;
            test_provider_connection(&local_store, &settings, &operation_control)?;
            operation_control.begin_commit()?;
            record_cloud_sync_connection_check(&local_store, current_time_ms())
        });
        cx.spawn(async move |this, cx| {
            let result = await_cloud_sync_job(
                task,
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(300)),
                control,
                |phase| {
                    let _ = this.update(cx, |this, cx| this.show_cloud_sync_timeout(phase, cx));
                },
            )
            .await;
            let _ = this.update(cx, |this, cx| {
                if registration.shutdown_cancelled(&result) {
                    this.cloud_sync.cancel_job_for_shutdown();
                    cx.notify();
                    return;
                }
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
                this.queue_cloud_sync_credentials_refresh(cx);
                this.request_settings_panel_refresh(cx);
                cx.notify();
            });
            drop(registration);
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn run_local_cloud_sync_push(
        &mut self,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_sync.settings().protocol_version == 4 {
            self.run_cloud_sync_v3_preview(cx);
            return;
        }
        if self.block_cloud_sync_for_settings_draft(cx) {
            return;
        }
        if !self.begin_cloud_sync_snapshot_job(cx) {
            return;
        }
        let resolution = if force {
            self.cloud_sync.take_resolution()
        } else {
            None
        };
        let runtime = self.runtime.clone();
        let settings = self.cloud_sync.settings().clone();
        let local_store = self.store_blocking_client();
        let started_at = Instant::now();
        self.cloud_sync.set_status(if force {
            t!("settings.syncForcePushingLocal")
        } else {
            t!("settings.syncPushingLocal")
        });
        self.shell.set_status(t!("settings.syncPushStarted"));
        let control = CloudSyncOperationControl::default();
        let Some(registration) =
            self.cloud_sync
                .operations
                .register("cloud-sync-manual", vec![control.clone()], None)
        else {
            self.cloud_sync.cancel_job_for_shutdown();
            return;
        };
        let operation_control = control.clone();
        let task = self
            .blocking_jobs
            .submit_task("cloud-sync-local-push", move |_| {
                let _operation = lock_cloud_sync_operation(&operation_control)?;
                let state = load_current_cloud_sync_state(&local_store)?;
                let options = load_cloud_sync_options(&local_store, &runtime, &settings, &state)?;
                if force && resolution.is_none() {
                    return Err(CloudSyncError::LocalChangedDuringSync);
                }
                if let Some(request) = &resolution {
                    return nyaterm_core::cloud_sync::conflict::resolve_with_remote(
                        &local_store,
                        &options,
                        &LocalDirectoryRemote::new(options.remote_dir.clone()),
                        &state,
                        request,
                        &|| Ok(()),
                        &operation_control,
                    )
                    .map(|result| (result, options));
                }
                cloud_sync::push_snapshot_with_remote_controlled(
                    &local_store,
                    &options,
                    &LocalDirectoryRemote::new(options.remote_dir.clone()),
                    &state,
                    force,
                    &operation_control,
                )
                .map(|result| (result, options))
            });
        cx.spawn(async move |this, cx| {
            let result = await_cloud_sync_job(
                task,
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(300)),
                control,
                |phase| {
                    let _ = this.update(cx, |this, cx| this.show_cloud_sync_timeout(phase, cx));
                },
            )
            .await;
            let _ = this.update(cx, |this, cx| {
                if registration.shutdown_cancelled(&result) {
                    this.cloud_sync.cancel_job_for_shutdown();
                    cx.notify();
                    return;
                }
                match result {
                    Ok((result, cleanup_options)) => {
                        schedule_cloud_sync_cleanup(
                            this.store_blocking_client(),
                            this.blocking_jobs.clone(),
                            this.cloud_sync.operations.clone(),
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
                        this.handle_cloud_sync_password_error(&error, cx);
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
                this.queue_cloud_sync_credentials_refresh(cx);
                this.request_settings_panel_refresh(cx);
                cx.notify();
            });
            drop(registration);
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn run_local_cloud_sync_pull(
        &mut self,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_sync.settings().protocol_version == 4 {
            self.run_cloud_sync_v3_preview(cx);
            return;
        }
        if self.block_cloud_sync_for_settings_draft(cx) {
            return;
        }
        if self.cloud_sync_restore_blocked(cx) {
            self.cloud_sync
                .set_status(t!("settings.syncCloseActiveSessionBeforePull"));
            cx.notify();
            return;
        }
        if !self.begin_cloud_sync_snapshot_job(cx) {
            return;
        }
        let resolution = if force {
            self.cloud_sync.take_resolution()
        } else {
            None
        };
        let runtime = self.runtime.clone();
        let settings = self.cloud_sync.settings().clone();
        let local_store = self.store_blocking_client();
        let started_at = Instant::now();
        self.cloud_sync.set_status(if force {
            t!("settings.syncForcePullingLocal")
        } else {
            t!("settings.syncPullingLocal")
        });
        self.shell.set_status(t!("settings.syncPullStarted"));
        let control = CloudSyncOperationControl::default();
        let Some(registration) =
            self.cloud_sync
                .operations
                .register("cloud-sync-manual", vec![control.clone()], None)
        else {
            self.cloud_sync.cancel_job_for_shutdown();
            return;
        };
        let operation_control = control.clone();
        let task = self
            .blocking_jobs
            .submit_task("cloud-sync-local-pull", move |_| {
                let _operation = lock_cloud_sync_operation(&operation_control)?;
                let state = load_current_cloud_sync_state(&local_store)?;
                let options = load_cloud_sync_options(&local_store, &runtime, &settings, &state)?;
                if force && resolution.is_none() {
                    return Err(CloudSyncError::LocalChangedDuringSync);
                }
                if let Some(request) = &resolution {
                    return nyaterm_core::cloud_sync::conflict::resolve_with_remote(
                        &local_store,
                        &options,
                        &LocalDirectoryRemote::new(options.remote_dir.clone()),
                        &state,
                        request,
                        &|| Ok(()),
                        &operation_control,
                    )
                    .map(|result| (result, options));
                }
                cloud_sync::pull_snapshot_with_remote_controlled(
                    &local_store,
                    &options,
                    &LocalDirectoryRemote::new(options.remote_dir.clone()),
                    &state,
                    force,
                    &|| Ok(()),
                    &operation_control,
                )
                .map(|result| (result, options))
            });
        cx.spawn(async move |this, cx| {
            let result = await_cloud_sync_job(
                task,
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(300)),
                control,
                |phase| {
                    let _ = this.update(cx, |this, cx| this.show_cloud_sync_timeout(phase, cx));
                },
            )
            .await;
            let _ = this.update(cx, |this, cx| {
                if registration.shutdown_cancelled(&result) {
                    this.cloud_sync.cancel_job_for_shutdown();
                    cx.notify();
                    return;
                }
                match result {
                    Ok((result, cleanup_options)) => {
                        schedule_cloud_sync_cleanup(
                            this.store_blocking_client(),
                            this.blocking_jobs.clone(),
                            this.cloud_sync.operations.clone(),
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
                        this.handle_cloud_sync_password_error(&error, cx);
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
                this.queue_cloud_sync_credentials_refresh(cx);
                this.request_settings_panel_refresh(cx);
                cx.notify();
            });
            drop(registration);
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn run_provider_cloud_sync_push(
        &mut self,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_sync.settings().protocol_version == 4 {
            self.run_cloud_sync_v3_preview(cx);
            return;
        }
        if self.block_cloud_sync_for_settings_draft(cx) {
            return;
        }
        if !self.begin_cloud_sync_snapshot_job(cx) {
            return;
        }
        let resolution = if force {
            self.cloud_sync.take_resolution()
        } else {
            None
        };
        let runtime = self.runtime.clone();
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
        let control = CloudSyncOperationControl::default();
        let Some(registration) =
            self.cloud_sync
                .operations
                .register("cloud-sync-manual", vec![control.clone()], None)
        else {
            self.cloud_sync.cancel_job_for_shutdown();
            return;
        };
        let operation_control = control.clone();
        let task = self
            .blocking_jobs
            .submit_task("cloud-sync-provider-push", move |_| {
                let _operation = lock_cloud_sync_operation(&operation_control)?;
                let state = load_current_cloud_sync_state(&local_store)?;
                let options = load_cloud_sync_options(&local_store, &runtime, &settings, &state)?;
                if force && resolution.is_none() {
                    return Err(CloudSyncError::LocalChangedDuringSync);
                }
                if let Some(request) = &resolution {
                    return super::super::cloud_sync_provider::resolve_provider_conflict(
                        &local_store,
                        &settings,
                        &options,
                        &state,
                        request,
                        &operation_control,
                    )
                    .map(|result| (result, options));
                }
                push_provider_snapshot(
                    &local_store,
                    &settings,
                    &options,
                    &state,
                    force,
                    &operation_control,
                )
                .map(|result| (result, options))
            });
        cx.spawn(async move |this, cx| {
            let result = await_cloud_sync_job(
                task,
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(300)),
                control,
                |phase| {
                    let _ = this.update(cx, |this, cx| this.show_cloud_sync_timeout(phase, cx));
                },
            )
            .await;
            let _ = this.update(cx, |this, cx| {
                if registration.shutdown_cancelled(&result) {
                    this.cloud_sync.cancel_job_for_shutdown();
                    cx.notify();
                    return;
                }
                match result {
                    Ok((result, cleanup_options)) => {
                        schedule_cloud_sync_cleanup(
                            this.store_blocking_client(),
                            this.blocking_jobs.clone(),
                            this.cloud_sync.operations.clone(),
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
                        this.handle_cloud_sync_password_error(&error, cx);
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
                this.queue_cloud_sync_credentials_refresh(cx);
                this.request_settings_panel_refresh(cx);
                cx.notify();
            });
            drop(registration);
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn run_provider_cloud_sync_pull(
        &mut self,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_sync.settings().protocol_version == 4 {
            self.run_cloud_sync_v3_preview(cx);
            return;
        }
        if self.block_cloud_sync_for_settings_draft(cx) {
            return;
        }
        if self.cloud_sync_restore_blocked(cx) {
            self.cloud_sync
                .set_status(t!("settings.syncCloseActiveSessionBeforePull"));
            cx.notify();
            return;
        }
        if !self.begin_cloud_sync_snapshot_job(cx) {
            return;
        }
        let resolution = if force {
            self.cloud_sync.take_resolution()
        } else {
            None
        };
        let runtime = self.runtime.clone();
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
        let control = CloudSyncOperationControl::default();
        let Some(registration) =
            self.cloud_sync
                .operations
                .register("cloud-sync-manual", vec![control.clone()], None)
        else {
            self.cloud_sync.cancel_job_for_shutdown();
            return;
        };
        let operation_control = control.clone();
        let task = self
            .blocking_jobs
            .submit_task("cloud-sync-provider-pull", move |_| {
                let _operation = lock_cloud_sync_operation(&operation_control)?;
                let state = load_current_cloud_sync_state(&local_store)?;
                let options = load_cloud_sync_options(&local_store, &runtime, &settings, &state)?;
                if force && resolution.is_none() {
                    return Err(CloudSyncError::LocalChangedDuringSync);
                }
                if let Some(request) = &resolution {
                    return super::super::cloud_sync_provider::resolve_provider_conflict(
                        &local_store,
                        &settings,
                        &options,
                        &state,
                        request,
                        &operation_control,
                    )
                    .map(|result| (result, options));
                }
                pull_provider_snapshot(
                    &local_store,
                    &settings,
                    &options,
                    &state,
                    force,
                    &operation_control,
                )
                .map(|result| (result, options))
            });
        cx.spawn(async move |this, cx| {
            let result = await_cloud_sync_job(
                task,
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(300)),
                control,
                |phase| {
                    let _ = this.update(cx, |this, cx| this.show_cloud_sync_timeout(phase, cx));
                },
            )
            .await;
            let _ = this.update(cx, |this, cx| {
                if registration.shutdown_cancelled(&result) {
                    this.cloud_sync.cancel_job_for_shutdown();
                    cx.notify();
                    return;
                }
                match result {
                    Ok((result, cleanup_options)) => {
                        schedule_cloud_sync_cleanup(
                            this.store_blocking_client(),
                            this.blocking_jobs.clone(),
                            this.cloud_sync.operations.clone(),
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
                        this.handle_cloud_sync_password_error(&error, cx);
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
                this.queue_cloud_sync_credentials_refresh(cx);
                this.request_settings_panel_refresh(cx);
                cx.notify();
            });
            drop(registration);
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn run_cloud_sync_recovery(
        &mut self,
        provider_action: bool,
        cx: &mut Context<Self>,
    ) {
        if self.block_cloud_sync_for_settings_draft(cx) {
            return;
        }
        if self.cloud_sync_restore_blocked(cx) {
            self.cloud_sync
                .set_status(t!("settings.syncCloseActiveSessionBeforeRecovery"));
            cx.notify();
            return;
        }
        if !self.begin_cloud_sync_snapshot_job(cx) {
            return;
        }
        let runtime = self.runtime.clone();
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
        let control = CloudSyncOperationControl::default();
        let Some(registration) =
            self.cloud_sync
                .operations
                .register("cloud-sync-manual", vec![control.clone()], None)
        else {
            self.cloud_sync.cancel_job_for_shutdown();
            return;
        };
        let operation_control = control.clone();
        let task = self
            .blocking_jobs
            .submit_task("cloud-sync-recover", move |_| {
                let _operation = lock_cloud_sync_operation(&operation_control)?;
                let state = load_current_cloud_sync_state(&local_store)?;
                let options = load_cloud_sync_options(&local_store, &runtime, &settings, &state)?;
                let result = if provider_action {
                    recover_provider_snapshot(&local_store, &settings, &options, &operation_control)
                } else {
                    cloud_sync::recover_current_snapshot_with_remote_controlled(
                        &local_store,
                        &options,
                        &LocalDirectoryRemote::new(options.remote_dir.clone()),
                        &operation_control,
                    )
                }?;
                Ok((result, options))
            });
        cx.spawn(async move |this, cx| {
            let result = await_cloud_sync_job(
                task,
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(300)),
                control,
                |phase| {
                    let _ = this.update(cx, |this, cx| this.show_cloud_sync_timeout(phase, cx));
                },
            )
            .await;
            let _ = this.update(cx, |this, cx| {
                if registration.shutdown_cancelled(&result) {
                    this.cloud_sync.cancel_job_for_shutdown();
                    cx.notify();
                    return;
                }
                match result {
                    Ok((result, cleanup_options)) => {
                        schedule_cloud_sync_cleanup(
                            this.store_blocking_client(),
                            this.blocking_jobs.clone(),
                            this.cloud_sync.operations.clone(),
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
                        this.handle_cloud_sync_password_error(&error, cx);
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
                this.queue_cloud_sync_credentials_refresh(cx);
                this.request_settings_panel_refresh(cx);
                cx.notify();
            });
            drop(registration);
        })
        .detach();
        cx.notify();
    }

    fn handle_cloud_sync_password_error(&mut self, error: &CloudSyncError, cx: &mut Context<Self>) {
        if matches!(
            error,
            CloudSyncError::PortableSnapshot(
                nyaterm_core::PortableSnapshotError::MissingMasterPassword
            )
        ) {
            self.focus_settings_tab(crate::models::SettingsTab::Security, cx);
            self.open_page(crate::models::NavItem::Settings, cx);
            self.cloud_sync
                .set_status(t!("settings.syncMasterPasswordRequired"));
            self.shell.set_status(self.cloud_sync.status().to_string());
        }
    }

    pub(super) fn begin_cloud_sync_snapshot_job(&mut self, cx: &mut Context<Self>) -> bool {
        if !matches!(self.cloud_sync.settings().protocol_version, 3 | 4) {
            self.cloud_sync
                .set_status("Unsupported cloud sync protocol");
            self.request_settings_panel_refresh(cx);
            cx.notify();
            return false;
        }
        if !self.cloud_sync.settings().enabled {
            self.cloud_sync.set_status(t!("settings.syncEnableFirst"));
            self.shell.set_status(self.cloud_sync.status().to_string());
            cx.notify();
            return false;
        }
        self.begin_cloud_sync_job(cx)
    }

    pub(super) fn begin_cloud_sync_job(&mut self, cx: &mut Context<Self>) -> bool {
        let other_job_running = self
            .desktop_controller
            .as_ref()
            .and_then(gpui::WeakEntity::upgrade)
            .is_some_and(|controller| {
                controller
                    .read(cx)
                    .cloud_sync_job_running_excluding(self.workspace_id, cx)
            });
        if !self.cloud_sync.operations.accepting() {
            return false;
        }
        if other_job_running || !self.cloud_sync.begin_job() {
            self.shell
                .set_status(t!("settings.syncOperationInProgress"));
            cx.notify();
            return false;
        }
        true
    }

    pub(crate) fn queue_cloud_sync_history_refresh(
        &mut self,
        entry: Option<CloudSyncHistoryEntry>,
        cx: &mut Context<Self>,
    ) {
        let registration = self.cloud_sync.operations.followup("cloud-sync-history");
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
                    Err(_) => tracing::warn!("cloud sync history refresh failed"),
                }
                this.request_settings_panel_refresh(cx);
                cx.notify();
            });
            drop(registration);
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
    operations: crate::features::sync::operation::CloudSyncOperationRegistry,
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
    let control = CloudSyncOperationControl::default();
    let Some(registration) = operations.register("cloud-sync-cleanup", vec![control.clone()], None)
    else {
        return;
    };
    let operation_control = control.clone();
    let task = scheduler.submit_task("cloud-sync-cleanup", move |_| {
        let _operation = lock_cloud_sync_operation(&operation_control)?;
        let cleanup_due = local_store
            .request_fn(StoreDomain::CloudSync, |store| {
                Ok(store
                    .load_cloud_sync_state()?
                    .last_gc_attempt_at_ms
                    .is_none_or(|last| current_time_ms().saturating_sub(last) >= 86_400_000))
            })
            .unwrap_or(false);
        if !cleanup_due {
            return Ok(());
        }
        let result = if let Some(settings) = settings {
            cleanup_provider_snapshots(
                &local_store,
                &settings,
                &options,
                latest.as_ref(),
                &operation_control,
            )
        } else {
            let remote = LocalDirectoryRemote::new(options.remote_dir.clone());
            cloud_sync::cleanup_sync_snapshots_with_remote_controlled(
                &local_store,
                &options,
                &remote,
                latest.as_ref(),
                &operation_control,
            )
        };
        operation_control.begin_commit()?;
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
    cx.spawn(async move |_, cx| {
        let result = await_cloud_sync_job(
            task,
            cx.background_executor()
                .timer(std::time::Duration::from_secs(300)),
            control,
            |_| {},
        )
        .await;
        if result.is_err() && !registration.shutdown_cancelled(&result) {
            tracing::warn!(
                provider = %provider,
                "cloud sync snapshot cleanup failed after a successful sync"
            );
        }
        drop(registration);
    })
    .detach();
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

fn load_current_cloud_sync_state(
    local_store: &nyaterm_store::StoreBlockingClient,
) -> Result<CloudSyncState, CloudSyncError> {
    local_store
        .request_fn(StoreDomain::CloudSync, |store| {
            store.load_cloud_sync_state()
        })
        .map_err(|error| CloudSyncError::LocalStore(error.category().to_string()))
}

fn cloud_sync_outcome_message(outcome: CloudSyncOutcome) -> String {
    match outcome {
        CloudSyncOutcome::UpToDate => t!("settings.syncUpToDate").to_string(),
        CloudSyncOutcome::Uploaded => t!("settings.syncPushSuccess").to_string(),
        CloudSyncOutcome::Downloaded => t!("settings.syncPullSuccess").to_string(),
        CloudSyncOutcome::Recovered => t!("settings.syncRecoverCurrentSuccess").to_string(),
    }
}

// Called only by a blocking job, under the same operation lock as automatic sync.
pub(super) fn load_cloud_sync_options(
    store: &nyaterm_store::StoreBlockingClient,
    runtime: &nyaterm_core::AppRuntime,
    settings: &CloudSyncSettings,
    state: &CloudSyncState,
) -> Result<LocalCloudSyncOptions, CloudSyncError> {
    let master_password = store
        .request_fn(StoreDomain::CloudSync, |database| {
            database.load_cloud_sync_master_password()
        })
        .map_err(|error| CloudSyncError::LocalStore(error.category().to_string()))?
        .filter(|password| !password.is_empty())
        .ok_or(nyaterm_core::PortableSnapshotError::MissingMasterPassword)?;
    Ok(LocalCloudSyncOptions {
        device_name: Some(settings.device_name.clone()),
        config_dir: runtime.config_dir().to_path_buf(),
        portable_key_path: runtime.portable_key_path().map(ToOwned::to_owned),
        remote_dir: runtime.config_dir().join("cloud-sync-local"),
        remote_root: settings.remote_root.clone(),
        device_id: state.device_id.clone(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        master_password,
        enabled: true,
    })
}

#[cfg(test)]
mod tests {
    use super::load_cloud_sync_options;
    use crate::test_support::{TestConfigDir, blocking_test_store};
    use nyaterm_core::{
        AppRuntime, CloudSyncError, CloudSyncSettings, CloudSyncState, PortableSnapshotError,
        RuntimeMode,
    };
    use nyaterm_store::{ConnectionStore, StoreDomain};
    use redb::ReadableTable as _;

    fn runtime(dir: &TestConfigDir) -> AppRuntime {
        AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            dir.path().into(),
            dir.join("config"),
            dir.join("logs"),
            dir.join("cache"),
            None,
        )
    }

    #[test]
    fn manual_cloud_sync_loads_the_saved_password_without_trimming_and_rejects_missing_password() {
        let dir = TestConfigDir::new("nyaterm-manual-sync-options");
        let store = blocking_test_store(dir.path());
        let settings = CloudSyncSettings::default();
        let state = CloudSyncState::default();
        assert!(matches!(
            load_cloud_sync_options(&store, &runtime(&dir), &settings, &state),
            Err(CloudSyncError::PortableSnapshot(
                PortableSnapshotError::MissingMasterPassword
            ))
        ));
        store
            .request_fn(StoreDomain::CloudSync, |store| {
                store.save_master_password(Some(" fixture-password "))
            })
            .unwrap();
        let options = load_cloud_sync_options(&store, &runtime(&dir), &settings, &state).unwrap();
        assert_eq!(
            options.master_password.expose_secret(),
            " fixture-password "
        );
        assert_eq!(options.device_id, state.device_id);
    }

    #[test]
    fn manual_cloud_sync_rejects_corrupted_saved_password_without_fallback_or_writes() {
        let dir = TestConfigDir::new("nyaterm-manual-sync-corrupt-password");
        let config = dir.join("config");
        let store = ConnectionStore::open(&config).unwrap();
        store
            .save_master_password(Some("fixture-password"))
            .unwrap();
        drop(store);
        // Seed a damaged supported settings document before starting the store worker.
        let database = redb::Database::open(config.join("nyaterm.redb")).unwrap();
        let transaction = database.begin_write().unwrap();
        {
            let mut table = transaction
                .open_table(redb::TableDefinition::<&str, &[u8]>::new("settings"))
                .unwrap();
            let mut settings: serde_json::Value =
                serde_json::from_slice(table.get("settings/default").unwrap().unwrap().value())
                    .unwrap();
            settings["security"]["master_password"] = serde_json::json!("corrupted-base64!");
            table
                .insert(
                    "settings/default",
                    serde_json::to_vec(&settings).unwrap().as_slice(),
                )
                .unwrap();
        }
        transaction.commit().unwrap();
        drop(database);
        let store = blocking_test_store(dir.path());
        let result = load_cloud_sync_options(
            &store,
            &runtime(&dir),
            &CloudSyncSettings::default(),
            &CloudSyncState::default(),
        );
        assert!(matches!(result, Err(CloudSyncError::LocalStore(_))));
        assert!(!config.join("cloud-sync-local").exists());
    }
}
