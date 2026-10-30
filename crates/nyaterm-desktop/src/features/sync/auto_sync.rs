use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::operation::AutoSyncOperationControls;
use gpui::Context;
use nyaterm_core::cloud_sync::control::CloudSyncOperationControl;
use nyaterm_core::{
    AppRuntime, CloudRemoteCheckDecision, CloudSyncError, CloudSyncResult, CloudSyncSettings,
    CloudSyncState, LocalCloudSyncOptions,
};
use nyaterm_store::{StoreBlockingClient, StoreDomain};
use rust_i18n::t;

use crate::features::NyaTermApp;

use super::cloud_sync_provider::{
    check_provider_snapshot, cleanup_provider_snapshots, pull_checked_provider_snapshot,
    push_checked_provider_snapshot,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutoSyncTrigger {
    Startup,
    Periodic,
    Focus,
    Settings,
    Change,
    DeferredPull,
    Retry,
}

impl AutoSyncTrigger {
    pub(crate) fn history_name(self) -> &'static str {
        match self {
            Self::Startup => "auto_startup",
            Self::Periodic => "auto_periodic",
            Self::Focus => "auto_focus",
            Self::Settings => "auto_settings",
            Self::Change => "auto_change",
            Self::DeferredPull => "auto_deferred_pull",
            Self::Retry => "auto_retry",
        }
    }
}

pub(crate) enum AutoSyncResult {
    V4(super::cloud_sync_v4::SyncReport),
    Disabled,
    Skipped,
    Debouncing(Duration),
    Busy,
    RemoteAvailable {
        retry_when_safe: bool,
        revision: Option<String>,
    },
    UpToDate(Box<CloudSyncState>),
    Synced(Box<CloudSyncResult>),
}

pub(crate) fn run_auto_sync(
    store: &StoreBlockingClient,
    runtime: &AppRuntime,
    trigger: AutoSyncTrigger,
    change_elapsed: Duration,
    pull_safe: bool,
    before_apply: &dyn Fn() -> Result<(), CloudSyncError>,
    controls: &AutoSyncOperationControls,
) -> Result<AutoSyncResult, CloudSyncError> {
    let mut scope = super::io_runtime::IoScope::new(trigger.history_name());
    let result = run_auto_sync_inner(
        store,
        runtime,
        trigger,
        change_elapsed,
        pull_safe,
        before_apply,
        controls,
    );
    scope.finish(&result);
    match &result {
        Ok(AutoSyncResult::Disabled) => scope.skipped("disabled"),
        Ok(AutoSyncResult::Skipped) => scope.skipped("skipped"),
        Ok(AutoSyncResult::Busy) => scope.skipped("busy"),
        Ok(AutoSyncResult::Debouncing(_)) => scope.skipped("debouncing"),
        _ => {}
    }
    result
}

fn run_auto_sync_inner(
    store: &StoreBlockingClient,
    runtime: &AppRuntime,
    trigger: AutoSyncTrigger,
    change_elapsed: Duration,
    pull_safe: bool,
    before_apply: &dyn Fn() -> Result<(), CloudSyncError>,
    controls: &AutoSyncOperationControls,
) -> Result<AutoSyncResult, CloudSyncError> {
    controls.sync.checkpoint()?;
    let _operation = match crate::features::cloud_sync_operation_lock().try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::WouldBlock) => return Ok(AutoSyncResult::Busy),
        Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
    };
    let (settings, state, password) = store
        .request_fn(StoreDomain::CloudSync, |database| {
            Ok((
                database.load_cloud_sync_settings()?,
                database.load_cloud_sync_state()?,
                database.load_cloud_sync_master_password()?,
            ))
        })
        .map_err(|error| CloudSyncError::LocalStore(error.category().to_string()))?;
    if let Some(skipped) = auto_sync_skip(&settings, trigger, change_elapsed) {
        return Ok(skipped);
    }
    if !matches!(settings.protocol_version, 3 | 4) {
        return Err(CloudSyncError::Remote(
            "Unsupported cloud sync protocol".into(),
        ));
    }
    let password = password.ok_or(nyaterm_core::PortableSnapshotError::MissingMasterPassword)?;
    let options = LocalCloudSyncOptions {
        device_name: Some(settings.device_name.clone()),
        config_dir: runtime.config_dir().to_path_buf(),
        portable_key_path: runtime.portable_key_path().map(ToOwned::to_owned),
        remote_dir: runtime.config_dir().join("cloud-sync-local"),
        remote_root: settings.remote_root.clone(),
        device_id: state.device_id.clone(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        master_password: password,
        enabled: true,
    };
    if settings.protocol_version == 4 {
        let preview = super::cloud_sync_v4::preview(store, &settings, &options, &controls.sync)?;
        if preview.migration
            || !pull_safe
            || !settings.auto_pull_remote_changes
            || !settings.auto_push_on_change
        {
            return Ok(AutoSyncResult::RemoteAvailable {
                retry_when_safe: !pull_safe,
                revision: None,
            });
        }
        let unchanged = preview
            .apply
            .local
            .replica
            .as_ref()
            .is_some_and(|replica| replica.document == preview.apply.plan.replica.document);
        let published = preview.apply.local.published_document.as_deref()
            == Some(nyaterm_sync::engine::digest(&preview.apply.plan.replica.document).as_str());
        if unchanged
            && published
            && preview.apply.local.pending.is_none()
            && (preview.apply.local.deferred.is_empty()
                || preview.apply.local.applied_generation == preview.apply.generation)
        {
            return Ok(AutoSyncResult::Skipped);
        }
        before_apply()?;
        return super::cloud_sync_v4::apply_and_publish(
            store,
            &settings,
            &options,
            preview,
            &controls.sync,
        )
        .map(AutoSyncResult::V4);
    }
    if trigger == AutoSyncTrigger::Change {
        let caches = super::io_runtime::CloudSyncIoRuntime::shared().caches(
            store,
            &settings,
            Some(&options),
        )?;
        if !caches
            .pending_remote
            .load(std::sync::atomic::Ordering::Acquire)
        {
            use nyaterm_core::cloud_sync::CloudLocalStore;
            let local = store.build_sync_snapshot(&options)?;
            if state.last_synced_content_hash.as_deref()
                == Some(local.sync_content_hash()?.as_str())
            {
                controls.sync.checkpoint()?;
                return Ok(AutoSyncResult::Skipped);
            }
        }
    }
    let checked = check_provider_snapshot(store, &settings, &options, &state, &controls.sync)?;
    let revision = checked.pointer.as_ref().map(|p| p.revision_id.clone());
    match checked.decision {
        CloudRemoteCheckDecision::LocalChanged if settings.auto_push_on_change => {
            let debounce = Duration::from_secs(settings.sync_debounce_seconds.clamp(1, 3_600));
            if !change_elapsed.is_zero() && change_elapsed < debounce {
                return Ok(AutoSyncResult::Debouncing(debounce - change_elapsed));
            }
            let result = push_checked_provider_snapshot(
                store,
                &settings,
                &options,
                checked,
                &controls.sync,
            )?;
            cleanup_if_due(store, &settings, &options, &result, &controls.cleanup);
            Ok(AutoSyncResult::Synced(Box::new(result)))
        }
        CloudRemoteCheckDecision::AutoPull if pull_safe => {
            let result = match pull_checked_provider_snapshot(
                store,
                &settings,
                &options,
                checked,
                before_apply,
                &controls.sync,
            ) {
                Err(CloudSyncError::AutoPullDeferred) => {
                    return Ok(AutoSyncResult::RemoteAvailable {
                        retry_when_safe: true,
                        revision: revision.clone(),
                    });
                }
                result => result?,
            };
            cleanup_if_due(store, &settings, &options, &result, &controls.cleanup);
            Ok(AutoSyncResult::Synced(Box::new(result)))
        }
        CloudRemoteCheckDecision::AutoPull => Ok(AutoSyncResult::RemoteAvailable {
            retry_when_safe: true,
            revision: revision.clone(),
        }),
        CloudRemoteCheckDecision::RemoteAvailable => Ok(AutoSyncResult::RemoteAvailable {
            retry_when_safe: false,
            revision,
        }),
        CloudRemoteCheckDecision::Conflict => unreachable!("conflict returned by remote check"),
        CloudRemoteCheckDecision::UpToDate | CloudRemoteCheckDecision::LocalChanged => {
            let checked = checked.state;
            let persisted = checked.clone();
            controls.sync.begin_commit()?;
            store
                .request_fn(StoreDomain::CloudSync, move |database| {
                    database.save_cloud_sync_state(&persisted)
                })
                .map_err(|error| CloudSyncError::LocalStore(error.category().to_string()))?;
            Ok(AutoSyncResult::UpToDate(Box::new(checked)))
        }
    }
}

fn auto_sync_skip(
    settings: &CloudSyncSettings,
    trigger: AutoSyncTrigger,
    change_elapsed: Duration,
) -> Option<AutoSyncResult> {
    if !settings.enabled {
        return Some(AutoSyncResult::Disabled);
    }
    if trigger == AutoSyncTrigger::Startup && !settings.auto_check_on_startup {
        return Some(AutoSyncResult::Skipped);
    }
    if trigger == AutoSyncTrigger::Change {
        if !settings.auto_push_on_change {
            return Some(AutoSyncResult::Skipped);
        }
        let debounce = Duration::from_secs(settings.sync_debounce_seconds.clamp(1, 3_600));
        if change_elapsed < debounce {
            return Some(AutoSyncResult::Debouncing(debounce - change_elapsed));
        }
    }
    None
}

fn cleanup_if_due(
    store: &StoreBlockingClient,
    settings: &CloudSyncSettings,
    options: &LocalCloudSyncOptions,
    result: &CloudSyncResult,
    control: &CloudSyncOperationControl,
) {
    const DAY_MS: u64 = 24 * 60 * 60 * 1_000;
    if result
        .state
        .last_gc_attempt_at_ms
        .is_some_and(|last| now_ms().saturating_sub(last) < DAY_MS)
    {
        return;
    }
    let _ = cleanup_provider_snapshots(store, settings, options, result.pointer.as_ref(), control);
    if control.begin_commit().is_err() {
        return;
    }
    let _ = store.request_fn(StoreDomain::CloudSync, |database| {
        let mut latest = database.load_cloud_sync_state()?;
        latest.last_gc_attempt_at_ms = Some(now_ms());
        database.save_cloud_sync_state(&latest)
    });
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

impl NyaTermApp {
    pub(crate) fn auto_cloud_sync_pull_blocked(&self) -> bool {
        self.session.session_order_len() > 0
            || self.session.start_has_pending()
            || self.remote_desktop.has_active_sessions()
            || self.settings_draft_dirty()
    }

    pub(crate) fn apply_auto_cloud_sync_result(
        &mut self,
        result: &Result<AutoSyncResult, CloudSyncError>,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_sync.job_running() {
            return;
        }
        match result {
            Ok(AutoSyncResult::V4(report)) => {
                let mut message = if report.verified {
                    t!("settings.syncV3Verified", count = report.conflicts).to_string()
                } else {
                    t!(
                        "settings.syncV3Pending",
                        detail = report.publication_error.clone().unwrap_or_default()
                    )
                    .to_string()
                };
                if report.deferred > 0 {
                    message.push_str(&format!(
                        " · {}",
                        t!("settings.syncV3Deferred", count = report.deferred)
                    ));
                }
                self.cloud_sync
                    .complete_job(self.cloud_sync.state().clone(), message.clone());
                self.shell.set_status(message);
                self.refresh_store_from_runtime_and_sync_theme(cx);
            }
            Ok(AutoSyncResult::RemoteAvailable { .. }) => {
                self.cloud_sync
                    .set_status(t!("settings.syncRemoteAvailable"));
            }
            Ok(AutoSyncResult::UpToDate(state)) => {
                self.cloud_sync.complete_job(
                    state.as_ref().clone(),
                    t!("settings.syncUpToDate").to_string(),
                );
            }
            Ok(AutoSyncResult::Synced(sync)) => {
                let message = match sync.outcome {
                    nyaterm_core::CloudSyncOutcome::Downloaded => t!("settings.syncPullSuccess"),
                    nyaterm_core::CloudSyncOutcome::Uploaded => t!("settings.syncPushSuccess"),
                    _ => t!("settings.syncUpToDate"),
                }
                .to_string();
                self.cloud_sync
                    .complete_job(sync.state.clone(), message.clone());
                self.shell.set_status(message);
                if sync.outcome == nyaterm_core::CloudSyncOutcome::Downloaded {
                    self.refresh_store_from_runtime_and_sync_theme(cx);
                }
            }
            Err(error) => {
                let message = match error {
                    CloudSyncError::Conflict(_) => t!("settings.syncConflictTitle").to_string(),
                    _ => t!("settings.syncAutoFailed", detail = error).to_string(),
                };
                self.cloud_sync.fail_job(
                    error,
                    message.clone(),
                    self.cloud_sync.settings().provider.clone(),
                    true,
                );
                self.shell.set_status(message);
            }
            _ => return,
        }
        self.request_settings_panel_refresh(cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{AutoSyncResult, AutoSyncTrigger, auto_sync_skip};
    use nyaterm_core::CloudSyncSettings;
    use std::time::Duration;

    #[test]
    fn cloud_sync_unchanged_local_notification_skips_all_network_requests() {
        use crate::features::sync::io_runtime::IoScope;
        use crate::features::sync::operation::AutoSyncOperationControls;
        use crate::test_support::{TestConfigDir, blocking_test_store};
        use nyaterm_core::cloud_sync::CloudLocalStore;
        use nyaterm_core::{AppRuntime, LocalCloudSyncOptions, RuntimeMode};
        use nyaterm_store::StoreDomain;
        let dir = TestConfigDir::new("nyaterm-cloud-sync-unchanged-notification");
        let store = blocking_test_store(dir.path());
        store
            .request_fn(StoreDomain::CloudSync, |s| {
                s.save_master_password(Some("fixture-password"))?;
                let settings = CloudSyncSettings {
                    enabled: true,
                    provider: "s3".into(),
                    ..Default::default()
                };
                // Invalid storage configuration makes any accidental connection fail.
                s.save_cloud_sync_settings(settings)?;
                Ok(())
            })
            .unwrap();
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            dir.path().into(),
            dir.join("config"),
            dir.join("logs"),
            dir.join("cache"),
            None,
        );
        let mut state = store
            .request_fn(StoreDomain::CloudSync, |s| s.load_cloud_sync_state())
            .unwrap();
        let options = LocalCloudSyncOptions {
            config_dir: dir.join("config"),
            portable_key_path: None,
            remote_dir: dir.join("cloud-sync-local"),
            remote_root: "nyaterm".into(),
            device_id: state.device_id.clone(),
            device_name: None,
            app_version: "test".into(),
            master_password: "fixture-password".into(),
            enabled: true,
        };
        state.last_synced_content_hash = Some(
            store
                .build_sync_snapshot(&options)
                .unwrap()
                .sync_content_hash()
                .unwrap(),
        );
        store
            .request_fn(StoreDomain::CloudSync, move |s| {
                s.save_cloud_sync_state(&state)
            })
            .unwrap();
        let scope = IoScope::new("fixture");
        // Other unit tests may briefly own the process operation lock.
        let mut result = Ok(AutoSyncResult::Busy);
        // Other sync tests share the process-wide operation lock, including
        // encrypted round trips and conflict resolution. Wait for that work.
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while std::time::Instant::now() < deadline {
            result = super::run_auto_sync(
                &store,
                &runtime,
                AutoSyncTrigger::Change,
                Duration::from_secs(61),
                true,
                &|| Ok(()),
                &AutoSyncOperationControls::default(),
            );
            if !matches!(result, Ok(AutoSyncResult::Busy)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(result, Ok(AutoSyncResult::Skipped)));
        assert_eq!(scope.stats.lock().unwrap().requests, 0);
    }

    #[test]
    fn github_gist_uses_every_background_trigger_and_the_same_switches_and_debounce() {
        let mut settings = CloudSyncSettings {
            enabled: true,
            provider: "github_gist".into(),
            auto_check_on_startup: true,
            auto_push_on_change: true,
            sync_debounce_seconds: 10,
            ..CloudSyncSettings::default()
        };
        for trigger in [
            AutoSyncTrigger::Startup,
            AutoSyncTrigger::Periodic,
            AutoSyncTrigger::Focus,
            AutoSyncTrigger::Settings,
            AutoSyncTrigger::Change,
            AutoSyncTrigger::DeferredPull,
            AutoSyncTrigger::Retry,
        ] {
            assert!(auto_sync_skip(&settings, trigger, Duration::from_secs(11)).is_none());
        }
        assert!(
            matches!(auto_sync_skip(&settings, AutoSyncTrigger::Change, Duration::from_secs(4)), Some(AutoSyncResult::Debouncing(delay)) if delay == Duration::from_secs(6))
        );
        settings.auto_check_on_startup = false;
        assert!(matches!(
            auto_sync_skip(&settings, AutoSyncTrigger::Startup, Duration::ZERO),
            Some(AutoSyncResult::Skipped)
        ));
        assert!(auto_sync_skip(&settings, AutoSyncTrigger::Periodic, Duration::ZERO).is_none());
        settings.auto_push_on_change = false;
        assert!(matches!(
            auto_sync_skip(&settings, AutoSyncTrigger::Change, Duration::ZERO),
            Some(AutoSyncResult::Skipped)
        ));
        settings.enabled = false;
        assert!(matches!(
            auto_sync_skip(&settings, AutoSyncTrigger::Focus, Duration::ZERO),
            Some(AutoSyncResult::Disabled)
        ));
    }
}
