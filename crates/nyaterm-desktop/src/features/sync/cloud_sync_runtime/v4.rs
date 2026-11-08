use gpui::Context;
use nyaterm_core::CloudSyncError;
use nyaterm_core::cloud_sync::control::CloudSyncOperationControl;
use nyaterm_store::StoreDomain;
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::sync::cloud_sync_v4;
use crate::features::sync::operation::{await_cloud_sync_job, lock_cloud_sync_operation};

enum JobResult {
    Preview(Box<cloud_sync_v4::Preview>),
    Applied(cloud_sync_v4::SyncReport),
}

enum PreviewDecision {
    Conflict(usize, usize),
}

impl NyaTermApp {
    pub(in crate::features) fn submit_store_request_with_password_rotation<R>(
        &mut self,
        generation: u64,
        password: Option<nyaterm_core::SecretString>,
        request: R,
        apply: impl FnOnce(&mut Self, nyaterm_store::StoreEvent<R::Response>, &mut Context<Self>)
        + 'static,
        cx: &mut Context<Self>,
    ) -> bool
    where
        R: nyaterm_store::StoreRequest,
    {
        let Some(password) = password else {
            return self.submit_store_request(generation, request, apply, cx);
        };
        if !self.begin_cloud_sync_job(cx) {
            return false;
        }
        self.cloud_sync.set_status(t!("settings.syncV3Rotating"));
        let store = self.store_blocking_client();
        let runtime = self.runtime.clone();
        let control = CloudSyncOperationControl::default();
        let Some(registration) =
            self.cloud_sync
                .operations
                .register("cloud-sync-v4", vec![control.clone()], None)
        else {
            self.cloud_sync.cancel_job_for_shutdown();
            return false;
        };
        let worker_control = control.clone();
        let task = self
            .blocking_jobs
            .submit_task("cloud-sync-password-rotation", move |_| {
                cloud_sync_v4::rotate_master_password(&store, &runtime, password, &worker_control)
            });
        cx.spawn(async move |this, cx| {
            let result = await_cloud_sync_job(
                task,
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(300)),
                control,
                |_| {},
            )
            .await;
            let _ = this.update(cx, |this, cx| {
                if registration.shutdown_cancelled(&result) {
                    this.cloud_sync.cancel_job_for_shutdown();
                    cx.notify();
                    return;
                }
                match result {
                    Ok(settings) => {
                        if let Some(settings) = settings {
                            if let Some(mut snapshot) = this.shell.take_settings_draft_snapshot() {
                                snapshot.cloud_sync_settings = settings.clone();
                                snapshot.cloud_sync_secret_draft = Default::default();
                                snapshot.master_password_enabled = true;
                                snapshot.master_password_draft = Default::default();
                                this.shell.set_settings_draft_snapshot(snapshot);
                            }
                            this.cloud_sync
                                .replace_settings(settings, Default::default());
                            this.cloud_sync.take_v4_preview();
                            this.settings.rebase_master_password();
                            this.request_shared_state_refresh(
                                crate::app_shell::SharedStateDomain::All,
                                cx,
                            );
                        }
                        this.cloud_sync.complete_job(
                            this.cloud_sync.state().clone(),
                            t!("settings.syncV3RotationVerified").to_string(),
                        );
                        this.submit_store_request(generation, request, apply, cx);
                    }
                    Err(error) => {
                        let message = t!("settings.applyFailed", error = error).to_string();
                        this.cloud_sync.fail_job_with_status(message.clone());
                        this.settings.update_store_status(message.clone(), false);
                        this.shell.set_status(message);
                    }
                }
                this.request_settings_panel_refresh(cx);
                cx.notify();
            });
            drop(registration);
        })
        .detach();
        self.request_settings_panel_refresh(cx);
        cx.notify();
        true
    }
    pub(in crate::features) fn run_cloud_sync_v3_preview(&mut self, cx: &mut Context<Self>) {
        self.run_cloud_sync_v3_job(false, None, cx);
    }
    pub(in crate::features) fn apply_cloud_sync_v3_preview(&mut self, cx: &mut Context<Self>) {
        self.run_cloud_sync_v3_job(true, None, cx);
    }
    pub(in crate::features) fn resolve_cloud_sync_v3_preview(
        &mut self,
        conflict: usize,
        candidate: usize,
        cx: &mut Context<Self>,
    ) {
        self.run_cloud_sync_v3_job(
            false,
            Some(PreviewDecision::Conflict(conflict, candidate)),
            cx,
        );
    }

    fn run_cloud_sync_v3_job(
        &mut self,
        apply: bool,
        resolution: Option<PreviewDecision>,
        cx: &mut Context<Self>,
    ) {
        if self.block_cloud_sync_for_settings_draft(cx) {
            return;
        }
        let replacement = if apply {
            let Some(lease) = self
                .begin_configuration_replacement("settings.syncCloseActiveSessionBeforePull", cx)
            else {
                return;
            };
            Some(lease)
        } else {
            None
        };
        if !self.begin_cloud_sync_snapshot_job(cx) {
            return;
        }
        let previous = if apply || resolution.is_some() {
            self.cloud_sync.take_v4_preview()
        } else {
            None
        };
        let settings = self.cloud_sync.settings().clone();
        let runtime = self.runtime.clone();
        let store = self.store_blocking_client();
        let control = CloudSyncOperationControl::default();
        let Some(registration) =
            self.cloud_sync
                .operations
                .register("cloud-sync-v4", vec![control.clone()], None)
        else {
            self.cloud_sync.cancel_job_for_shutdown();
            return;
        };
        let worker_control = control.clone();
        let worker_replacement = replacement.clone();
        let replacement_controller = self.desktop_controller.clone();
        let task = self.blocking_jobs.submit_task("cloud-sync-v3", move |_| {
            let _replacement = worker_replacement;
            if let Some(lease) = &_replacement {
                lease.require_refresh();
            }
            let _operation = lock_cloud_sync_operation(&worker_control)?;
            let state = store
                .request_fn(StoreDomain::CloudSync, |store| {
                    store.load_cloud_sync_state()
                })
                .map_err(|_| CloudSyncError::LocalChangedDuringSync)?;
            let options =
                super::jobs::load_cloud_sync_options(&store, &runtime, &settings, &state)?;
            if apply {
                let preview = previous.ok_or(CloudSyncError::LocalChangedDuringSync)?;
                return cloud_sync_v4::apply_and_publish(
                    &store,
                    &settings,
                    &options,
                    preview,
                    &worker_control,
                )
                .map(JobResult::Applied);
            }
            if let Some(decision) = resolution {
                let mut preview = previous.ok_or(CloudSyncError::LocalChangedDuringSync)?;
                match decision {
                    PreviewDecision::Conflict(conflict, candidate) => preview.resolve(
                        &store,
                        options.master_password.expose_secret(),
                        conflict,
                        candidate,
                    )?,
                }
                return Ok(JobResult::Preview(Box::new(preview)));
            }
            cloud_sync_v4::preview_manual(&store, &settings, &options, &worker_control)
                .map(|preview| JobResult::Preview(Box::new(preview)))
        });
        cx.spawn(async move |this, cx| {
            let result = await_cloud_sync_job(
                task,
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(300)),
                control,
                |_| {},
            )
            .await;
            let _ = this.update(cx, |this, cx| {
                if registration.shutdown_cancelled(&result) {
                    this.cloud_sync.cancel_job_for_shutdown();
                    cx.notify();
                    return;
                }
                match result {
                    Ok(JobResult::Preview(preview)) => {
                        this.cloud_sync.set_v4_preview(*preview);
                        this.cloud_sync.complete_job(
                            this.cloud_sync.state().clone(),
                            t!("settings.syncV3PreviewReady").to_string(),
                        );
                    }
                    Ok(JobResult::Applied(report)) => {
                        let mut message = if report.verified {
                            t!("settings.syncV3Verified", count = report.conflicts).to_string()
                        } else {
                            t!(
                                "settings.syncV3Pending",
                                detail = report.publication_error.unwrap_or_default()
                            )
                            .to_string()
                        };
                        if report.deferred > 0 {
                            message.push_str(&format!(
                                " · {}",
                                t!("settings.syncV3Deferred", count = report.deferred)
                            ));
                        }
                        this.cloud_sync
                            .complete_job(this.cloud_sync.state().clone(), message);
                        this.refresh_store_from_runtime_and_sync_theme(cx);
                    }
                    Err(error) => this.cloud_sync.fail_job_with_status(error.to_string()),
                }
                this.shell.set_status(this.cloud_sync.status().to_string());
                this.request_settings_panel_refresh(cx);
                cx.notify();
            });
            if let Some(lease) = replacement {
                Self::finish_configuration_replacement(lease, replacement_controller, &this, cx);
            }
            drop(registration);
        })
        .detach();
        self.request_settings_panel_refresh(cx);
        cx.notify();
    }
}
