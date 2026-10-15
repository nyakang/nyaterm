use std::future::Future;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use tauri::{Emitter, Manager, async_runtime};
use tokio::sync::{Mutex, Notify};

use crate::config::{
    self, CloudConflictPreview, CloudSyncHistoryEntry, CloudSyncSettings, CloudSyncState,
    CloudSyncStatus,
};
use crate::error::{AppError, AppResult, CloudSyncError};

use super::crypto::require_master_password;
use super::gc::{
    SYNC_SNAPSHOT_GC_GRACE_PERIOD, cleanup_sync_snapshots, is_gist_provider,
    prune_gist_snapshots_best_effort,
};
use super::history_log::{log_history_entry, read_cloud_sync_history_from_logs};
use super::migration::{
    RemoteSnapshotResolution, recover_current_remote_snapshot, resolve_remote_snapshot,
};
use super::operator::{build_remote, ensure_remote_layout};
use super::protocol::{
    commit_sync_pointer, ensure_remote_head_unchanged, pointer_from_snapshot, upload_sync_snapshot,
    verify_uploaded_sync_snapshot, write_current_sync_snapshot_compat,
};
use super::remote::{
    RemoteSyncPointer, SYNC_SNAPSHOTS_DIR, current_time_ms, elapsed_ms, load_sync_pointer,
    remote_path,
};

use crate::core::portable_snapshot::{
    PortableSnapshot, PortableSnapshotKind, apply_merged_connection_entities,
    apply_portable_snapshot, build_portable_snapshot, calculate_payload_hash,
    merge_additive_sync_connections,
};

const CLOUD_SYNC_STARTUP_CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const CLOUD_SYNC_OPERATION_TIMEOUT: Duration = Duration::from_secs(300);
const CLOUD_SYNC_QUICK_OPERATION_TIMEOUT: Duration = Duration::from_secs(60);
const CLOUD_SYNC_CLEANUP_TIMEOUT: Duration = Duration::from_secs(20);
const CLOUD_SYNC_REMOTE_CHECK_INTERVAL: Duration = Duration::from_secs(15 * 60);
const CLOUD_SYNC_FOCUS_CHECK_THROTTLE_MS: u64 = 2 * 60 * 1_000;
const CLOUD_SYNC_FULL_VALIDATION_INTERVAL_MS: u64 = 24 * 60 * 60 * 1_000;
const CLOUD_SYNC_GC_INTERVAL_MS: u64 = 24 * 60 * 60 * 1_000;
const AUTOMATIC_RETRY_BACKOFF_MS: [u64; 4] = [60_000, 300_000, 900_000, 3_600_000];

pub struct CloudSyncManager {
    app_handle: OnceLock<tauri::AppHandle>,
    settings: Arc<Mutex<CloudSyncSettings>>,
    state: Arc<Mutex<CloudSyncState>>,
    status: Arc<Mutex<CloudSyncStatus>>,
    automatic_retry: Arc<Mutex<AutomaticRetryState>>,
    auto_push_notify: Arc<Notify>,
    auto_push_worker_started: AtomicBool,
    runtime_check_notify: Arc<Notify>,
    runtime_check_worker_started: AtomicBool,
    runtime_check_requested: AtomicBool,
    operation_lock: Arc<Mutex<()>>,
}

#[derive(Debug, Clone, Default)]
struct AutomaticRetryState {
    consecutive_failures: u32,
    blocked_until_ms: Option<u64>,
    suspended_until_settings_change: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutomaticRetryGate {
    Run,
    Wait(Duration),
    Suspended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteCheckDecision {
    UpToDate,
    LocalChanged,
    AutoPull,
    RemoteAvailable,
    Conflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteCheckOutcome {
    NoRemote,
    UpToDate,
    LocalChanged,
    AutoPulled,
    RemoteAvailable,
    Conflict,
    Skipped,
}

impl CloudSyncManager {
    pub fn new() -> Self {
        Self {
            app_handle: OnceLock::new(),
            settings: Arc::new(Mutex::new(CloudSyncSettings::default())),
            state: Arc::new(Mutex::new(CloudSyncState::default())),
            status: Arc::new(Mutex::new(CloudSyncStatus::default())),
            automatic_retry: Arc::new(Mutex::new(AutomaticRetryState::default())),
            auto_push_notify: Arc::new(Notify::new()),
            auto_push_worker_started: AtomicBool::new(false),
            runtime_check_notify: Arc::new(Notify::new()),
            runtime_check_worker_started: AtomicBool::new(false),
            runtime_check_requested: AtomicBool::new(false),
            operation_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn set_app_handle(&self, app: tauri::AppHandle) {
        let _ = self.app_handle.set(app);
    }

    pub async fn init(self: &Arc<Self>, app: tauri::AppHandle) -> AppResult<()> {
        self.set_app_handle(app.clone());

        {
            let settings = config::load_app_settings(&app)
                .map(|settings| settings.cloud_sync)
                .unwrap_or_default();
            *self.settings.lock().await = settings;
        }
        {
            let mut state = config::load_cloud_sync_state(&app).unwrap_or_default();
            if state.device_id.is_empty() {
                state.device_id = uuid::Uuid::new_v4().to_string();
                let _ = config::save_cloud_sync_state(&app, &state);
            }
            *self.state.lock().await = state;
        }
        self.ensure_auto_push_worker();
        self.ensure_runtime_remote_check_worker();

        self.set_status("idle", String::new(), None, None).await;

        let settings = self.settings.lock().await.clone();
        if settings.enabled && settings.auto_check_on_startup {
            if should_skip_startup_network_check(&settings) {
                self.set_status(
                    "idle",
                    "Startup cloud sync check skipped for GitHub Gist; sync will run on demand"
                        .to_string(),
                    None,
                    None,
                )
                .await;
                return Ok(());
            }

            let manager = Arc::clone(self);
            async_runtime::spawn(async move {
                if let Err(error) = with_operation_timeout(
                    "startup_check",
                    CLOUD_SYNC_STARTUP_CHECK_TIMEOUT,
                    manager.startup_check(),
                )
                .await
                {
                    manager.handle_startup_check_failure(error).await;
                }
            });
        }

        Ok(())
    }

    pub async fn replace_settings(&self, settings: CloudSyncSettings) -> AppResult<()> {
        let previous = self.settings.lock().await.clone();
        if previous == settings {
            return Ok(());
        }

        let enabled = settings.enabled;
        let provider = settings.provider.clone();
        let reset_retry = cloud_sync_connection_changed(&previous, &settings);
        let request_remote_check = cloud_sync_remote_check_required(&previous, &settings);
        *self.settings.lock().await = settings.clone();
        if reset_retry {
            self.reset_automatic_retry().await;
        }
        self.set_status_after_settings_replace(enabled, provider)
            .await;
        if request_remote_check {
            self.request_runtime_remote_check();
        }
        Ok(())
    }

    pub async fn get_status(&self) -> CloudSyncStatus {
        self.status.lock().await.clone()
    }

    pub async fn list_history(&self) -> Vec<CloudSyncHistoryEntry> {
        let Ok(app) = self.app() else {
            return Vec::new();
        };
        read_cloud_sync_history_from_logs(&app).unwrap_or_default()
    }

    pub async fn notify_config_changed(self: &Arc<Self>) {
        let settings = self.settings.lock().await.clone();
        if !settings.enabled || !settings.auto_push_on_change {
            return;
        }
        self.auto_push_notify.notify_one();
    }

    pub async fn request_focus_remote_check(self: &Arc<Self>) {
        let settings = self.settings.lock().await.clone();
        if !settings.enabled || should_skip_runtime_network_check(&settings) {
            return;
        }

        let last_checked_at_ms = self.state.lock().await.last_checked_at_ms;
        if last_checked_at_ms.is_some_and(|last_checked_at_ms| {
            current_time_ms().saturating_sub(last_checked_at_ms)
                < CLOUD_SYNC_FOCUS_CHECK_THROTTLE_MS
        }) {
            return;
        }

        self.request_runtime_remote_check();
    }

    pub async fn test_connection(&self) -> AppResult<()> {
        let started = Instant::now();
        let settings = self.settings.lock().await.clone();
        let result = with_operation_timeout(
            "test_connection",
            CLOUD_SYNC_QUICK_OPERATION_TIMEOUT,
            async {
                let _ = require_master_password()?;
                self.set_status(
                    "running",
                    "Connecting to cloud sync storage".to_string(),
                    Some("test_connection".to_string()),
                    None,
                )
                .await;
                let remote = trace_cloud_sync_step("test_connection", "build_remote", async {
                    self.build_remote_with_recovery(settings.clone()).await
                })
                .await?;
                self.set_status(
                    "running",
                    "Verifying cloud sync storage layout".to_string(),
                    Some("test_connection".to_string()),
                    None,
                )
                .await;
                trace_cloud_sync_step("test_connection", "ensure_remote_layout", async {
                    ensure_remote_layout(&remote, &settings.remote_root).await
                })
                .await?;
                self.set_status(
                    "running",
                    "Checking cloud sync storage access".to_string(),
                    Some("test_connection".to_string()),
                    None,
                )
                .await;
                let _ = trace_cloud_sync_step("test_connection", "exists_snapshots_dir", async {
                    remote
                        .exists(&remote_path(&settings.remote_root, SYNC_SNAPSHOTS_DIR))
                        .await
                })
                .await?;
                Ok::<(), AppError>(())
            },
        )
        .await;

        match result {
            Ok(()) => {
                self.reset_automatic_retry().await;
                self.append_history(CloudSyncHistoryEntry {
                    id: uuid::Uuid::new_v4().to_string(),
                    timestamp_ms: current_time_ms(),
                    kind: "sync".to_string(),
                    status: "success".to_string(),
                    trigger: "manual_test_connection".to_string(),
                    provider: Some(settings.provider),
                    revision: None,
                    duration_ms: Some(elapsed_ms(started.elapsed())),
                    message: "Cloud connection verified".to_string(),
                })
                .await;
                self.complete_test_connection_success().await;
                Ok(())
            }
            Err(error) => {
                self.record_failure("sync", "manual_test_connection", &error)
                    .await;
                Err(error)
            }
        }
    }

    async fn complete_test_connection_success(&self) {
        self.set_status("idle", "Cloud connection verified".to_string(), None, None)
            .await;
    }

    pub async fn sync_push_now(self: &Arc<Self>, trigger: &str) -> AppResult<()> {
        match with_operation_timeout(
            trigger,
            CLOUD_SYNC_OPERATION_TIMEOUT,
            self.push_snapshot(trigger, false),
        )
        .await
        {
            Ok(()) => {
                self.reset_automatic_retry().await;
                Ok(())
            }
            Err(error) => {
                self.record_failure("sync", trigger, &error).await;
                Err(error)
            }
        }
    }

    pub async fn sync_pull_now(self: &Arc<Self>, trigger: &str) -> AppResult<()> {
        match with_operation_timeout(
            trigger,
            CLOUD_SYNC_OPERATION_TIMEOUT,
            self.pull_snapshot(trigger, false),
        )
        .await
        {
            Ok(()) => {
                self.reset_automatic_retry().await;
                Ok(())
            }
            Err(error) => {
                self.record_failure("sync", trigger, &error).await;
                Err(error)
            }
        }
    }

    pub async fn resolve_cloud_sync_conflict(self: &Arc<Self>, action: &str) -> AppResult<()> {
        let result = with_operation_timeout(action, CLOUD_SYNC_OPERATION_TIMEOUT, async {
            match action {
                "upload_local" => self.push_snapshot("resolve_upload", true).await,
                "download_remote" => self.pull_snapshot("resolve_download", true).await,
                "recover_current_remote" => self.recover_current_remote(action).await,
                "merge_connections" => self.merge_connections_conflict().await,
                _ => Err(AppError::Config(format!(
                    "Unsupported conflict resolution action '{}'",
                    action
                ))),
            }
        })
        .await;

        match result {
            Ok(()) => {
                self.reset_automatic_retry().await;
                Ok(())
            }
            Err(error) => {
                self.record_failure("sync", action, &error).await;
                Err(error)
            }
        }
    }

    async fn merge_connections_conflict(self: &Arc<Self>) -> AppResult<()> {
        const TRIGGER: &str = "resolve_merge_connections";
        let _guard = self.operation_lock.lock().await;
        let _ = require_master_password()?;
        let settings = self.settings.lock().await.clone();
        if !settings.enabled {
            return Err(AppError::Config(
                "Cloud sync is disabled in settings".into(),
            ));
        }
        let conflict = self
            .status
            .lock()
            .await
            .conflict
            .clone()
            .filter(|conflict| conflict.kind == "content_conflict")
            .ok_or_else(|| {
                AppError::Config(
                    "Cloud sync merge requires a current content conflict; check for changes again"
                        .into(),
                )
            })?;
        if conflict.provider != settings.provider {
            return Err(AppError::Config(
                "Cloud sync provider changed; check for conflicts again".into(),
            ));
        }

        let started = Instant::now();
        let state_snapshot = self.state.lock().await.clone();
        let app = self.app()?;
        let local =
            build_portable_snapshot(&app, PortableSnapshotKind::Sync, &state_snapshot.device_id)?;
        if local.payload_hash != conflict.local_payload_hash {
            return Err(AppError::Config(
                "Local cloud sync data changed; check for conflicts again".into(),
            ));
        }
        let remote = trace_cloud_sync_step(TRIGGER, "build_remote", async {
            self.build_remote_with_recovery(settings.clone()).await
        })
        .await?;
        let latest = trace_cloud_sync_step(TRIGGER, "load_sync_pointer", async {
            load_sync_pointer(&remote, &settings.remote_root).await
        })
        .await?
        .ok_or_else(|| AppError::Config("No remote sync snapshot found".into()))?;
        if conflict.remote_revision != latest.revision_id
            || conflict.remote_payload_hash != latest.payload_hash
        {
            return Err(AppError::Config(
                "Remote cloud sync data changed; check for conflicts again".into(),
            ));
        }
        let remote_snapshot =
            match trace_cloud_sync_step(TRIGGER, "resolve_remote_snapshot", async {
                resolve_remote_snapshot(&remote, &settings.remote_root, &latest).await
            })
            .await?
            {
                RemoteSnapshotResolution::Current(snapshot)
                | RemoteSnapshotResolution::LegacyMigrated(snapshot) => snapshot,
                RemoteSnapshotResolution::Inconsistent { .. } => {
                    return Err(AppError::Config(
                        "Remote cloud sync metadata is inconsistent; use recovery instead".into(),
                    ));
                }
            };

        let mut merged = merge_additive_sync_connections(&local, &remote_snapshot)?;
        merged.device_id = state_snapshot.device_id.clone();
        merged.revision_id = uuid::Uuid::new_v4().to_string();
        merged.created_at_ms = current_time_ms();
        merged.payload_hash = calculate_payload_hash(&merged)?;

        let pointer = pointer_from_snapshot(&merged);
        publish_verified_merge_snapshot(
            TRIGGER,
            &remote,
            &settings,
            &latest,
            &merged,
            &local.payload_hash,
            || async {
                Ok(build_portable_snapshot(
                    &app,
                    PortableSnapshotKind::Sync,
                    &state_snapshot.device_id,
                )?
                .payload_hash)
            },
        )
        .await?;

        // The checked local state must still match before applying the remote
        // union. Never turn a partial/local mismatch into a synced success.
        let before_apply =
            build_portable_snapshot(&app, PortableSnapshotKind::Sync, &state_snapshot.device_id)?;
        if before_apply.payload_hash != local.payload_hash {
            return Err(AppError::Config(
                "Merged snapshot was published but local data changed; check sync again before applying"
                    .into(),
            ));
        }
        apply_merged_connection_entities(&app, &merged)?;
        let persisted =
            build_portable_snapshot(&app, PortableSnapshotKind::Sync, &state_snapshot.device_id)?;
        if persisted.payload_hash != merged.payload_hash {
            return Err(AppError::Config(
                "Merged cloud data was published but the local snapshot differs; check sync again"
                    .into(),
            ));
        }

        if let Err(error) =
            trace_cloud_sync_step(TRIGGER, "write_current_sync_snapshot_compat", async {
                write_current_sync_snapshot_compat(&remote, &settings.remote_root, &merged).await
            })
            .await
        {
            tracing::warn!(error = %error, "Compatible current snapshot refresh failed after merge");
        }
        self.schedule_sync_snapshot_gc(remote.clone(), settings.remote_root.clone(), Some(pointer))
            .await?;
        {
            let mut state = self.state.lock().await;
            state.last_synced_payload_hash = Some(merged.payload_hash.clone());
            state.last_applied_remote_revision = Some(merged.revision_id.clone());
            state.last_synced_at_ms = Some(current_time_ms());
            state.last_checked_at_ms = Some(current_time_ms());
            state.last_validated_remote_revision = Some(merged.revision_id.clone());
            state.last_full_validation_at_ms = Some(current_time_ms());
            config::save_cloud_sync_state(&app, &state)?;
        }
        self.append_history(CloudSyncHistoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp_ms: current_time_ms(),
            kind: "sync".to_string(),
            status: "success".to_string(),
            trigger: TRIGGER.to_string(),
            provider: Some(settings.provider),
            revision: Some(merged.revision_id),
            duration_ms: Some(elapsed_ms(started.elapsed())),
            message: "Cloud sync connections merged".to_string(),
        })
        .await;
        self.set_status(
            "idle",
            "Cloud sync connections merged".to_string(),
            None,
            None,
        )
        .await;
        Ok(())
    }

    async fn startup_check(self: &Arc<Self>) -> AppResult<RemoteCheckOutcome> {
        let allow_auto_pull = self.settings.lock().await.auto_pull_remote_changes;
        self.check_remote_changes("startup_check", allow_auto_pull)
            .await
    }

    async fn check_remote_changes(
        self: &Arc<Self>,
        trigger: &str,
        allow_auto_pull: bool,
    ) -> AppResult<RemoteCheckOutcome> {
        let started = Instant::now();
        let Ok(_guard) = self.operation_lock.try_lock() else {
            self.set_status(
                "idle",
                format!(
                    "{} cloud sync check skipped because another cloud sync operation is running",
                    remote_check_label(trigger)
                ),
                None,
                None,
            )
            .await;
            tracing::info!(
                trigger,
                "Cloud sync remote check skipped because another operation is running"
            );
            return Ok(RemoteCheckOutcome::Skipped);
        };
        let settings = self.settings.lock().await.clone();
        if !settings.enabled {
            self.set_status("disabled", String::new(), None, None).await;
            return Ok(RemoteCheckOutcome::Skipped);
        }
        let _ = require_master_password()?;
        self.set_status(
            "running",
            "Connecting to cloud sync storage".to_string(),
            Some(trigger.to_string()),
            None,
        )
        .await;
        let remote = trace_cloud_sync_step(trigger, "build_remote", async {
            self.build_remote_with_recovery(settings.clone()).await
        })
        .await?;
        self.set_status(
            "running",
            "Reading latest cloud sync pointer".to_string(),
            Some(trigger.to_string()),
            None,
        )
        .await;
        let latest = trace_cloud_sync_step(trigger, "load_sync_pointer", async {
            load_sync_pointer(&remote, &settings.remote_root).await
        })
        .await?;

        {
            let mut state = self.state.lock().await;
            state.last_checked_at_ms = Some(current_time_ms());
            config::save_cloud_sync_state(&self.app()?, &state)?;
        }

        let Some(remote_pointer) = latest else {
            self.set_status(
                "idle",
                "No remote sync snapshot found".to_string(),
                None,
                None,
            )
            .await;
            return Ok(RemoteCheckOutcome::NoRemote);
        };

        let state = self.state.lock().await.clone();
        let local_envelope =
            build_portable_snapshot(&self.app()?, PortableSnapshotKind::Sync, &state.device_id)?;
        let local_hash = local_envelope.payload_hash.clone();
        let validation_reason =
            remote_validation_reason(&state, &remote_pointer, current_time_ms());
        let mut validated_snapshot = None;
        if let Some(reason) = validation_reason {
            tracing::info!(
                trigger,
                reason,
                revision = %remote_pointer.revision_id,
                "Cloud sync full remote snapshot validation required"
            );
            match self
                .resolve_remote_snapshot_for_check(
                    trigger,
                    &remote,
                    &settings,
                    &local_hash,
                    &remote_pointer,
                )
                .await?
            {
                Ok(snapshot) => {
                    self.mark_remote_validated(&remote_pointer).await?;
                    validated_snapshot = Some(snapshot);
                }
                Err(conflict) => {
                    self.record_remote_check_conflict(
                        trigger,
                        &settings,
                        &remote_pointer,
                        conflict,
                    )
                    .await;
                    return Ok(RemoteCheckOutcome::Conflict);
                }
            }
        } else {
            tracing::info!(
                trigger,
                revision = %remote_pointer.revision_id,
                "Cloud sync remote check completed with pointer metadata only"
            );
        }

        match decide_remote_check(&state, &local_hash, &remote_pointer, allow_auto_pull) {
            RemoteCheckDecision::UpToDate => {
                {
                    let mut state = self.state.lock().await;
                    state.last_synced_payload_hash = Some(local_hash);
                    state.last_applied_remote_revision = Some(remote_pointer.revision_id.clone());
                    state.last_checked_at_ms = Some(current_time_ms());
                    config::save_cloud_sync_state(&self.app()?, &state)?;
                }
                self.set_status("idle", "Cloud sync is up to date".to_string(), None, None)
                    .await;
                Ok(RemoteCheckOutcome::UpToDate)
            }
            RemoteCheckDecision::Conflict => {
                let conflict = cloud_conflict_preview(&settings, &local_hash, &remote_pointer);
                self.append_history(CloudSyncHistoryEntry {
                    id: uuid::Uuid::new_v4().to_string(),
                    timestamp_ms: current_time_ms(),
                    kind: "sync".to_string(),
                    status: "conflict".to_string(),
                    trigger: trigger.to_string(),
                    provider: Some(settings.provider.clone()),
                    revision: Some(remote_pointer.revision_id.clone()),
                    duration_ms: None,
                    message: conflict.message.clone(),
                })
                .await;
                self.set_status("conflict", conflict.message.clone(), None, Some(conflict))
                    .await;
                Ok(RemoteCheckOutcome::Conflict)
            }
            RemoteCheckDecision::AutoPull => {
                let envelope = if let Some(snapshot) = validated_snapshot {
                    snapshot
                } else {
                    match self
                        .resolve_remote_snapshot_for_check(
                            trigger,
                            &remote,
                            &settings,
                            &local_hash,
                            &remote_pointer,
                        )
                        .await?
                    {
                        Ok(snapshot) => {
                            self.mark_remote_validated(&remote_pointer).await?;
                            snapshot
                        }
                        Err(conflict) => {
                            self.record_remote_check_conflict(
                                trigger,
                                &settings,
                                &remote_pointer,
                                conflict,
                            )
                            .await;
                            return Ok(RemoteCheckOutcome::Conflict);
                        }
                    }
                };
                self.apply_remote_snapshot_locked(
                    "auto_pull_remote",
                    &settings,
                    &remote,
                    &remote_pointer,
                    envelope,
                    started,
                )
                .await?;
                Ok(RemoteCheckOutcome::AutoPulled)
            }
            RemoteCheckDecision::RemoteAvailable => {
                self.set_status(
                    "idle",
                    "A newer cloud sync snapshot is available".to_string(),
                    None,
                    None,
                )
                .await;
                Ok(RemoteCheckOutcome::RemoteAvailable)
            }
            RemoteCheckDecision::LocalChanged => {
                self.set_status("idle", "Local changes pending sync".to_string(), None, None)
                    .await;
                Ok(RemoteCheckOutcome::LocalChanged)
            }
        }
    }

    async fn resolve_remote_snapshot_for_check(
        &self,
        trigger: &str,
        remote: &super::operator::CloudRemote,
        settings: &CloudSyncSettings,
        local_hash: &str,
        pointer: &RemoteSyncPointer,
    ) -> AppResult<Result<PortableSnapshot, CloudConflictPreview>> {
        let resolution = trace_cloud_sync_step(trigger, "resolve_remote_snapshot", async {
            resolve_remote_snapshot(remote, &settings.remote_root, pointer).await
        })
        .await?;
        Ok(match resolution {
            RemoteSnapshotResolution::Current(snapshot)
            | RemoteSnapshotResolution::LegacyMigrated(snapshot) => Ok(snapshot),
            RemoteSnapshotResolution::Inconsistent {
                pointer,
                recovery_candidate,
            } => Err(remote_inconsistent_preview(
                settings,
                local_hash,
                &pointer,
                &recovery_candidate,
            )),
        })
    }

    async fn mark_remote_validated(&self, pointer: &RemoteSyncPointer) -> AppResult<()> {
        let mut state = self.state.lock().await;
        state.last_validated_remote_revision = Some(pointer.revision_id.clone());
        state.last_full_validation_at_ms = Some(current_time_ms());
        config::save_cloud_sync_state(&self.app()?, &state)
    }

    async fn record_remote_check_conflict(
        &self,
        trigger: &str,
        settings: &CloudSyncSettings,
        pointer: &RemoteSyncPointer,
        conflict: CloudConflictPreview,
    ) {
        self.append_history(CloudSyncHistoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp_ms: current_time_ms(),
            kind: "sync".to_string(),
            status: "conflict".to_string(),
            trigger: trigger.to_string(),
            provider: Some(settings.provider.clone()),
            revision: Some(pointer.revision_id.clone()),
            duration_ms: None,
            message: conflict.message.clone(),
        })
        .await;
        self.set_status("conflict", conflict.message.clone(), None, Some(conflict))
            .await;
    }

    async fn handle_startup_check_failure(&self, error: AppError) {
        if should_record_startup_check_failure(&error) {
            self.record_failure("sync", "startup_check", &error).await;
            tracing::warn!("Startup cloud sync check failed: {}", error);
            return;
        }

        let message = format!("Startup cloud sync check skipped: {error}");
        self.set_status("idle", message.clone(), None, None).await;
        tracing::warn!("{}", message);
    }

    async fn handle_runtime_check_failure(&self, trigger: &str, error: AppError) {
        if is_non_retryable_automatic_error(&error) {
            self.record_failure("sync", trigger, &error).await;
            tracing::warn!("Cloud sync remote check failed: {}", error);
            return;
        }

        self.record_automatic_retry_failure(trigger, &error).await;
        self.set_status("idle", String::new(), None, None).await;
        tracing::warn!(
            trigger,
            error = %error,
            "Cloud sync remote check skipped after transient failure"
        );
    }

    fn ensure_runtime_remote_check_worker(self: &Arc<Self>) {
        if self
            .runtime_check_worker_started
            .swap(true, Ordering::SeqCst)
        {
            return;
        }

        let manager = Arc::clone(self);
        async_runtime::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(CLOUD_SYNC_REMOTE_CHECK_INTERVAL) => {
                        manager.run_runtime_remote_check("periodic_check").await;
                    }
                    _ = manager.runtime_check_notify.notified() => {
                        manager.runtime_check_requested.store(false, Ordering::SeqCst);
                        while tokio::time::timeout(
                            Duration::from_millis(100),
                            manager.runtime_check_notify.notified(),
                        )
                        .await
                        .is_ok()
                        {
                            manager.runtime_check_requested.store(false, Ordering::SeqCst);
                        }
                        manager.run_runtime_remote_check("focus_check").await;
                    }
                }
            }
        });
    }

    fn request_runtime_remote_check(&self) {
        if !self.runtime_check_requested.swap(true, Ordering::SeqCst) {
            self.runtime_check_notify.notify_one();
        }
    }

    async fn run_runtime_remote_check(self: &Arc<Self>, trigger: &'static str) {
        let settings = self.settings.lock().await.clone();
        if !settings.enabled || should_skip_runtime_network_check(&settings) {
            return;
        }

        match self.automatic_retry_gate().await {
            AutomaticRetryGate::Run => {}
            AutomaticRetryGate::Wait(delay) => {
                tracing::debug!(
                    trigger,
                    delay_ms = elapsed_ms(delay),
                    "Cloud sync remote check skipped during automatic retry backoff"
                );
                return;
            }
            AutomaticRetryGate::Suspended => return,
        }

        let result = with_operation_timeout(
            trigger,
            CLOUD_SYNC_STARTUP_CHECK_TIMEOUT,
            self.check_remote_changes(trigger, settings.auto_pull_remote_changes),
        )
        .await;

        match result {
            Ok(RemoteCheckOutcome::Skipped) => {}
            Ok(_) => {
                self.reset_automatic_retry().await;
            }
            Err(error) => {
                self.handle_runtime_check_failure(trigger, error).await;
            }
        }
    }

    fn ensure_auto_push_worker(self: &Arc<Self>) {
        if self.auto_push_worker_started.swap(true, Ordering::SeqCst) {
            return;
        }

        let manager = Arc::clone(self);
        async_runtime::spawn(async move {
            loop {
                manager.auto_push_notify.notified().await;
                wait_for_auto_push_debounce(&manager.settings, &manager.auto_push_notify).await;

                loop {
                    let settings = manager.settings.lock().await.clone();
                    if !settings.enabled || !settings.auto_push_on_change {
                        break;
                    }
                    match manager.automatic_retry_gate().await {
                        AutomaticRetryGate::Run => {}
                        AutomaticRetryGate::Wait(delay) => {
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        AutomaticRetryGate::Suspended => break,
                    }
                    if let Err(error) = manager.sync_push_now("auto_push").await {
                        tracing::warn!("Auto push failed: {}", error);
                        match manager.automatic_retry_gate().await {
                            AutomaticRetryGate::Wait(delay) => {
                                tokio::time::sleep(delay).await;
                                continue;
                            }
                            AutomaticRetryGate::Suspended | AutomaticRetryGate::Run => break,
                        }
                    }
                    break;
                }
            }
        });
    }

    async fn push_snapshot(self: &Arc<Self>, trigger: &str, force: bool) -> AppResult<()> {
        let _guard = self.operation_lock.lock().await;
        let _ = require_master_password()?;
        let settings = self.settings.lock().await.clone();
        if !settings.enabled {
            return Err(AppError::Config(
                "Cloud sync is disabled in settings".to_string(),
            ));
        }

        let state_snapshot = self.state.lock().await.clone();
        let envelope = build_portable_snapshot(
            &self.app()?,
            PortableSnapshotKind::Sync,
            &state_snapshot.device_id,
        )?;
        let local_hash = envelope.payload_hash.clone();
        if should_skip_automatic_push(trigger, &state_snapshot, &local_hash) {
            tracing::info!(
                trigger,
                "Cloud sync auto push skipped because local payload is unchanged"
            );
            return Ok(());
        }

        self.set_status(
            "running",
            "Uploading cloud sync snapshot".to_string(),
            Some("sync_push".to_string()),
            None,
        )
        .await;

        let started = Instant::now();
        self.set_status(
            "running",
            "Connecting to cloud sync storage".to_string(),
            Some("sync_push".to_string()),
            None,
        )
        .await;
        let remote = trace_cloud_sync_step(trigger, "build_remote", async {
            self.build_remote_with_recovery(settings.clone()).await
        })
        .await?;
        self.set_status(
            "running",
            "Verifying cloud sync storage layout".to_string(),
            Some("sync_push".to_string()),
            None,
        )
        .await;
        trace_cloud_sync_step(trigger, "ensure_remote_layout", async {
            ensure_remote_layout(&remote, &settings.remote_root).await
        })
        .await?;

        self.set_status(
            "running",
            "Preparing local cloud sync snapshot".to_string(),
            Some("sync_push".to_string()),
            None,
        )
        .await;
        self.set_status(
            "running",
            "Reading latest cloud sync pointer".to_string(),
            Some("sync_push".to_string()),
            None,
        )
        .await;
        let latest = trace_cloud_sync_step(trigger, "load_sync_pointer", async {
            load_sync_pointer(&remote, &settings.remote_root).await
        })
        .await?;

        if let Some(remote_pointer) = &latest {
            if remote_pointer.payload_hash == local_hash {
                match trace_cloud_sync_step(trigger, "resolve_remote_snapshot", async {
                    resolve_remote_snapshot(&remote, &settings.remote_root, remote_pointer).await
                })
                .await?
                {
                    RemoteSnapshotResolution::Current(_)
                    | RemoteSnapshotResolution::LegacyMigrated(_) => {}
                    RemoteSnapshotResolution::Inconsistent {
                        pointer,
                        recovery_candidate,
                    } => {
                        let conflict = remote_inconsistent_preview(
                            &settings,
                            &local_hash,
                            &pointer,
                            &recovery_candidate,
                        );
                        self.set_status("conflict", conflict.message.clone(), None, Some(conflict))
                            .await;
                        return Err(AppError::Config(
                            "Cloud sync remote metadata is inconsistent".to_string(),
                        ));
                    }
                }
                {
                    let mut state = self.state.lock().await;
                    state.last_synced_payload_hash = Some(local_hash);
                    state.last_applied_remote_revision = Some(remote_pointer.revision_id.clone());
                    state.last_checked_at_ms = Some(current_time_ms());
                    state.last_validated_remote_revision = Some(remote_pointer.revision_id.clone());
                    state.last_full_validation_at_ms = Some(current_time_ms());
                    config::save_cloud_sync_state(&self.app()?, &state)?;
                }
                self.set_status(
                    "idle",
                    "Cloud sync is already up to date".to_string(),
                    None,
                    None,
                )
                .await;
                return Ok(());
            }
        }

        let remote_changed = latest.as_ref().is_some_and(|remote| {
            state_snapshot
                .last_applied_remote_revision
                .as_deref()
                .map_or(true, |revision| revision != remote.revision_id)
        });
        let local_changed = state_snapshot
            .last_synced_payload_hash
            .as_deref()
            .map_or(true, |hash| hash != local_hash);

        if remote_changed && !force {
            if local_changed {
                let remote = latest.expect("checked above");
                let conflict = CloudConflictPreview {
                    detected_at_ms: current_time_ms(),
                    provider: settings.provider.clone(),
                    kind: "content_conflict".to_string(),
                    local_payload_hash: local_hash.clone(),
                    remote_payload_hash: remote.payload_hash.clone(),
                    remote_revision: remote.revision_id.clone(),
                    remote_created_at_ms: remote.created_at_ms,
                    remote_device_id: remote.device_id.clone(),
                    recovery_revision: None,
                    recovery_payload_hash: None,
                    recovery_created_at_ms: None,
                    message: "Both local and cloud state changed since last sync".to_string(),
                };
                self.append_history(CloudSyncHistoryEntry {
                    id: uuid::Uuid::new_v4().to_string(),
                    timestamp_ms: current_time_ms(),
                    kind: "sync".to_string(),
                    status: "conflict".to_string(),
                    trigger: trigger.to_string(),
                    provider: Some(settings.provider.clone()),
                    revision: Some(remote.revision_id.clone()),
                    duration_ms: Some(elapsed_ms(started.elapsed())),
                    message: conflict.message.clone(),
                })
                .await;
                self.set_status("conflict", conflict.message.clone(), None, Some(conflict))
                    .await;
                return Err(AppError::Config("Cloud sync conflict detected".to_string()));
            }
            return Err(AppError::Config(
                "Remote snapshot is newer than local state. Pull first.".to_string(),
            ));
        }

        self.set_status(
            "running",
            "Uploading cloud sync snapshot".to_string(),
            Some("sync_push".to_string()),
            None,
        )
        .await;

        let pointer = pointer_from_snapshot(&envelope);
        let gist_backend = is_gist_provider(&settings.provider);
        if gist_backend {
            prune_gist_snapshots_step(
                trigger,
                &remote,
                &settings.remote_root,
                latest.as_ref(),
                &envelope.revision_id,
            )
            .await;
        }

        // Gist snippets silently drop a new file once they are at capacity, so a
        // failed upload/verification is retried once after freeing capacity.
        let mut upload_attempts = 0;
        loop {
            upload_attempts += 1;
            let upload_result = trace_cloud_sync_step(trigger, "upload_sync_snapshot", async {
                upload_sync_snapshot(&remote, &settings.remote_root, &envelope).await
            })
            .await;
            match upload_result {
                Ok(()) => {}
                Err(error)
                    if gist_backend
                        && upload_attempts < 2
                        && matches!(
                            &error,
                            AppError::CloudSync(CloudSyncError::RemoteFileRejected { .. })
                        ) =>
                {
                    tracing::warn!(
                        error = %error,
                        attempt = upload_attempts,
                        "Gist upload rejected; pruning and retrying once"
                    );
                    prune_gist_snapshots_step(
                        trigger,
                        &remote,
                        &settings.remote_root,
                        latest.as_ref(),
                        &envelope.revision_id,
                    )
                    .await;
                    continue;
                }
                Err(error) => return Err(error),
            }

            match trace_cloud_sync_step(trigger, "verify_uploaded_sync_snapshot", async {
                verify_uploaded_sync_snapshot(&remote, &settings.remote_root, &pointer).await
            })
            .await
            {
                Ok(_) => break,
                Err(error)
                    if gist_backend
                        && upload_attempts < 2
                        && matches!(
                            &error,
                            AppError::CloudSync(
                                CloudSyncError::SnapshotNotAccepted { .. }
                                    | CloudSyncError::RemoteFileRejected { .. }
                            )
                        ) =>
                {
                    tracing::warn!(
                        error = %error,
                        attempt = upload_attempts,
                        "Gist upload verify failed; pruning and retrying once"
                    );
                    prune_gist_snapshots_step(
                        trigger,
                        &remote,
                        &settings.remote_root,
                        latest.as_ref(),
                        &envelope.revision_id,
                    )
                    .await;
                }
                Err(error) => return Err(error),
            }
        }

        if !force {
            trace_cloud_sync_step(trigger, "recheck_sync_pointer", async {
                ensure_remote_head_unchanged(&remote, &settings.remote_root, latest.as_ref()).await
            })
            .await?;
        }

        self.set_status(
            "running",
            "Updating cloud sync pointer".to_string(),
            Some("sync_push".to_string()),
            None,
        )
        .await;
        trace_cloud_sync_step(trigger, "commit_sync_pointer", async {
            commit_sync_pointer(&remote, &settings.remote_root, &pointer).await
        })
        .await?;
        if let Err(error) =
            trace_cloud_sync_step(trigger, "write_current_sync_snapshot_compat", async {
                write_current_sync_snapshot_compat(&remote, &settings.remote_root, &envelope).await
            })
            .await
        {
            tracing::warn!(
                error = %error,
                revision = %envelope.revision_id,
                "Compatible current cloud sync snapshot write failed after commit"
            );
        }
        self.schedule_sync_snapshot_gc(remote.clone(), settings.remote_root.clone(), Some(pointer))
            .await?;

        {
            let mut state = self.state.lock().await;
            state.last_synced_payload_hash = Some(envelope.payload_hash.clone());
            state.last_applied_remote_revision = Some(envelope.revision_id.clone());
            state.last_synced_at_ms = Some(current_time_ms());
            state.last_checked_at_ms = Some(current_time_ms());
            state.last_validated_remote_revision = Some(envelope.revision_id.clone());
            state.last_full_validation_at_ms = Some(current_time_ms());
            config::save_cloud_sync_state(&self.app()?, &state)?;
        }

        self.append_history(CloudSyncHistoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp_ms: current_time_ms(),
            kind: "sync".to_string(),
            status: "success".to_string(),
            trigger: trigger.to_string(),
            provider: Some(settings.provider.clone()),
            revision: Some(envelope.revision_id.clone()),
            duration_ms: Some(elapsed_ms(started.elapsed())),
            message: "Cloud sync snapshot uploaded".to_string(),
        })
        .await;
        self.set_status(
            "idle",
            "Cloud sync snapshot uploaded".to_string(),
            None,
            None,
        )
        .await;
        Ok(())
    }

    async fn pull_snapshot(self: &Arc<Self>, trigger: &str, force: bool) -> AppResult<()> {
        let _guard = self.operation_lock.lock().await;
        self.pull_snapshot_locked(trigger, force).await
    }

    async fn recover_current_remote(self: &Arc<Self>, trigger: &str) -> AppResult<()> {
        let _guard = self.operation_lock.lock().await;
        let _ = require_master_password()?;
        let settings = self.settings.lock().await.clone();
        if !settings.enabled {
            return Err(AppError::Config(
                "Cloud sync is disabled in settings".to_string(),
            ));
        }

        let started = Instant::now();
        self.set_status(
            "running",
            "Recovering incomplete cloud sync metadata".to_string(),
            Some("sync_recover".to_string()),
            None,
        )
        .await;
        let remote = trace_cloud_sync_step(trigger, "build_remote", async {
            self.build_remote_with_recovery(settings.clone()).await
        })
        .await?;
        trace_cloud_sync_step(trigger, "ensure_remote_layout", async {
            ensure_remote_layout(&remote, &settings.remote_root).await
        })
        .await?;
        let envelope = trace_cloud_sync_step(trigger, "recover_current_remote_snapshot", async {
            recover_current_remote_snapshot(&remote, &settings.remote_root).await
        })
        .await?;
        trace_cloud_sync_step(trigger, "apply_portable_snapshot", async {
            apply_portable_snapshot(&self.app()?, &envelope).await
        })
        .await?;
        let pointer = pointer_from_snapshot(&envelope);
        self.schedule_sync_snapshot_gc(remote.clone(), settings.remote_root.clone(), Some(pointer))
            .await?;

        {
            let mut state = self.state.lock().await;
            state.last_synced_payload_hash = Some(envelope.payload_hash.clone());
            state.last_applied_remote_revision = Some(envelope.revision_id.clone());
            state.last_synced_at_ms = Some(current_time_ms());
            state.last_checked_at_ms = Some(current_time_ms());
            state.last_validated_remote_revision = Some(envelope.revision_id.clone());
            state.last_full_validation_at_ms = Some(current_time_ms());
            config::save_cloud_sync_state(&self.app()?, &state)?;
        }

        self.append_history(CloudSyncHistoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp_ms: current_time_ms(),
            kind: "sync".to_string(),
            status: "success".to_string(),
            trigger: trigger.to_string(),
            provider: Some(settings.provider.clone()),
            revision: Some(envelope.revision_id.clone()),
            duration_ms: Some(elapsed_ms(started.elapsed())),
            message: "Cloud sync metadata recovered from current snapshot".to_string(),
        })
        .await;
        self.set_status(
            "idle",
            "Cloud sync metadata recovered".to_string(),
            None,
            None,
        )
        .await;
        Ok(())
    }

    async fn pull_snapshot_locked(self: &Arc<Self>, trigger: &str, force: bool) -> AppResult<()> {
        let _ = require_master_password()?;
        let settings = self.settings.lock().await.clone();
        if !settings.enabled {
            return Err(AppError::Config(
                "Cloud sync is disabled in settings".to_string(),
            ));
        }

        self.set_status(
            "running",
            "Downloading cloud sync snapshot".to_string(),
            Some("sync_pull".to_string()),
            None,
        )
        .await;

        let started = Instant::now();
        self.set_status(
            "running",
            "Connecting to cloud sync storage".to_string(),
            Some("sync_pull".to_string()),
            None,
        )
        .await;
        let remote = trace_cloud_sync_step(trigger, "build_remote", async {
            self.build_remote_with_recovery(settings.clone()).await
        })
        .await?;
        self.set_status(
            "running",
            "Reading latest cloud sync pointer".to_string(),
            Some("sync_pull".to_string()),
            None,
        )
        .await;
        let latest = trace_cloud_sync_step(trigger, "load_sync_pointer", async {
            load_sync_pointer(&remote, &settings.remote_root).await
        })
        .await?
        .ok_or_else(|| AppError::Config("No remote sync snapshot found".to_string()))?;

        let state_snapshot = self.state.lock().await.clone();
        self.set_status(
            "running",
            "Preparing local cloud sync snapshot".to_string(),
            Some("sync_pull".to_string()),
            None,
        )
        .await;
        let local_envelope = build_portable_snapshot(
            &self.app()?,
            PortableSnapshotKind::Sync,
            &state_snapshot.device_id,
        )?;
        let local_changed = state_snapshot
            .last_synced_payload_hash
            .as_deref()
            .map_or(true, |hash| hash != local_envelope.payload_hash);
        let remote_changed = state_snapshot
            .last_applied_remote_revision
            .as_deref()
            .map_or(true, |revision| revision != latest.revision_id);

        let remote_envelope =
            match trace_cloud_sync_step(trigger, "resolve_remote_snapshot", async {
                resolve_remote_snapshot(&remote, &settings.remote_root, &latest).await
            })
            .await?
            {
                RemoteSnapshotResolution::Current(snapshot)
                | RemoteSnapshotResolution::LegacyMigrated(snapshot) => snapshot,
                RemoteSnapshotResolution::Inconsistent {
                    pointer,
                    recovery_candidate,
                } => {
                    let conflict = remote_inconsistent_preview(
                        &settings,
                        &local_envelope.payload_hash,
                        &pointer,
                        &recovery_candidate,
                    );
                    self.append_history(CloudSyncHistoryEntry {
                        id: uuid::Uuid::new_v4().to_string(),
                        timestamp_ms: current_time_ms(),
                        kind: "sync".to_string(),
                        status: "conflict".to_string(),
                        trigger: trigger.to_string(),
                        provider: Some(settings.provider.clone()),
                        revision: Some(pointer.revision_id.clone()),
                        duration_ms: Some(elapsed_ms(started.elapsed())),
                        message: conflict.message.clone(),
                    })
                    .await;
                    self.set_status("conflict", conflict.message.clone(), None, Some(conflict))
                        .await;
                    return Err(AppError::Config(
                        "Cloud sync remote metadata is inconsistent".to_string(),
                    ));
                }
            };
        self.mark_remote_validated(&latest).await?;

        if latest.payload_hash == local_envelope.payload_hash {
            {
                let mut state = self.state.lock().await;
                state.last_synced_payload_hash = Some(latest.payload_hash.clone());
                state.last_applied_remote_revision = Some(latest.revision_id.clone());
                state.last_checked_at_ms = Some(current_time_ms());
                config::save_cloud_sync_state(&self.app()?, &state)?;
            }
            self.set_status(
                "idle",
                "Cloud sync is already up to date".to_string(),
                None,
                None,
            )
            .await;
            return Ok(());
        }

        if remote_changed && local_changed && !force {
            let conflict = CloudConflictPreview {
                detected_at_ms: current_time_ms(),
                provider: settings.provider.clone(),
                kind: "content_conflict".to_string(),
                local_payload_hash: local_envelope.payload_hash.clone(),
                remote_payload_hash: latest.payload_hash.clone(),
                remote_revision: latest.revision_id.clone(),
                remote_created_at_ms: latest.created_at_ms,
                remote_device_id: latest.device_id.clone(),
                recovery_revision: None,
                recovery_payload_hash: None,
                recovery_created_at_ms: None,
                message: "Both local and cloud state changed since last sync".to_string(),
            };
            self.append_history(CloudSyncHistoryEntry {
                id: uuid::Uuid::new_v4().to_string(),
                timestamp_ms: current_time_ms(),
                kind: "sync".to_string(),
                status: "conflict".to_string(),
                trigger: trigger.to_string(),
                provider: Some(settings.provider.clone()),
                revision: Some(latest.revision_id.clone()),
                duration_ms: Some(elapsed_ms(started.elapsed())),
                message: conflict.message.clone(),
            })
            .await;
            self.set_status("conflict", conflict.message.clone(), None, Some(conflict))
                .await;
            return Err(AppError::Config("Cloud sync conflict detected".to_string()));
        }

        if !remote_changed && !force {
            return Err(AppError::Config(
                "No newer remote sync snapshot is available".to_string(),
            ));
        }

        self.apply_remote_snapshot_locked(
            trigger,
            &settings,
            &remote,
            &latest,
            remote_envelope,
            started,
        )
        .await
    }

    async fn apply_remote_snapshot_locked(
        self: &Arc<Self>,
        trigger: &str,
        settings: &CloudSyncSettings,
        remote: &super::operator::CloudRemote,
        pointer: &RemoteSyncPointer,
        envelope: PortableSnapshot,
        started: Instant,
    ) -> AppResult<()> {
        self.set_status(
            "running",
            "Applying cloud sync snapshot".to_string(),
            Some("sync_pull".to_string()),
            None,
        )
        .await;
        trace_cloud_sync_step(trigger, "apply_portable_snapshot", async {
            apply_portable_snapshot(&self.app()?, &envelope).await
        })
        .await?;
        self.set_status(
            "running",
            "Refreshing cloud sync current snapshot".to_string(),
            Some("sync_pull".to_string()),
            None,
        )
        .await;
        if let Err(error) =
            trace_cloud_sync_step(trigger, "write_current_sync_snapshot_compat", async {
                write_current_sync_snapshot_compat(remote, &settings.remote_root, &envelope).await
            })
            .await
        {
            tracing::warn!(
                error = %error,
                revision = %envelope.revision_id,
                "Compatible current cloud sync snapshot refresh failed after pull"
            );
        }
        self.schedule_sync_snapshot_gc(
            remote.clone(),
            settings.remote_root.clone(),
            Some(pointer.clone()),
        )
        .await?;

        {
            let mut state = self.state.lock().await;
            state.last_synced_payload_hash = Some(envelope.payload_hash.clone());
            state.last_applied_remote_revision = Some(envelope.revision_id.clone());
            state.last_synced_at_ms = Some(current_time_ms());
            state.last_checked_at_ms = Some(current_time_ms());
            state.last_validated_remote_revision = Some(pointer.revision_id.clone());
            state.last_full_validation_at_ms = Some(current_time_ms());
            config::save_cloud_sync_state(&self.app()?, &state)?;
        }

        self.append_history(CloudSyncHistoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp_ms: current_time_ms(),
            kind: "sync".to_string(),
            status: "success".to_string(),
            trigger: trigger.to_string(),
            provider: Some(settings.provider.clone()),
            revision: Some(envelope.revision_id.clone()),
            duration_ms: Some(elapsed_ms(started.elapsed())),
            message: "Cloud sync snapshot downloaded".to_string(),
        })
        .await;
        self.set_status(
            "idle",
            "Cloud sync snapshot downloaded".to_string(),
            None,
            None,
        )
        .await;
        Ok(())
    }

    async fn schedule_sync_snapshot_gc(
        self: &Arc<Self>,
        remote: super::operator::CloudRemote,
        remote_root: String,
        latest: Option<RemoteSyncPointer>,
    ) -> AppResult<()> {
        let now = current_time_ms();
        let grace_period = {
            let mut state = self.state.lock().await;
            let Some(grace_period) =
                sync_snapshot_gc_policy(&remote, state.last_gc_attempt_at_ms, now)
            else {
                tracing::info!("Cloud sync snapshot cleanup skipped by daily throttle");
                return Ok(());
            };
            state.last_gc_attempt_at_ms = Some(now);
            config::save_cloud_sync_state(&self.app()?, &state)?;
            grace_period
        };

        async_runtime::spawn(async move {
            let result = with_operation_timeout(
                "cleanup_sync_snapshots",
                CLOUD_SYNC_CLEANUP_TIMEOUT,
                async {
                    cleanup_sync_snapshots(&remote, &remote_root, latest.as_ref(), grace_period)
                        .await;
                    Ok(())
                },
            )
            .await;

            if let Err(error) = result {
                tracing::warn!(
                    error = %error,
                    grace_hours = grace_period.as_secs() / 3600,
                    "Cloud sync snapshot cleanup did not complete"
                );
            }
        });
        Ok(())
    }

    async fn append_history(&self, entry: CloudSyncHistoryEntry) {
        let Ok(app) = self.app() else {
            return;
        };
        log_history_entry(&entry);
        let snapshot = read_cloud_sync_history_from_logs(&app).unwrap_or_default();
        let _ = app.emit("cloud-sync-history-changed", &snapshot);
    }

    async fn record_failure(&self, kind: &str, trigger: &str, error: &AppError) {
        let status = self.status.lock().await.clone();
        if status.state == "conflict" {
            return;
        }

        let provider = self.settings.lock().await.provider.clone();
        let message = error.to_string();
        let error_code = cloud_sync_failure_code(&provider, error);

        self.append_history(CloudSyncHistoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp_ms: current_time_ms(),
            kind: kind.to_string(),
            status: "failed".to_string(),
            trigger: trigger.to_string(),
            provider: Some(provider),
            revision: None,
            duration_ms: None,
            message: message.clone(),
        })
        .await;
        self.set_status_with_code(
            "failed",
            message,
            None,
            None,
            error_code.map(str::to_string),
        )
        .await;
        self.record_automatic_retry_failure(trigger, error).await;
    }

    async fn reset_automatic_retry(&self) {
        *self.automatic_retry.lock().await = AutomaticRetryState::default();
    }

    async fn automatic_retry_gate(&self) -> AutomaticRetryGate {
        let retry = self.automatic_retry.lock().await.clone();
        if retry.suspended_until_settings_change {
            return AutomaticRetryGate::Suspended;
        }
        let Some(blocked_until_ms) = retry.blocked_until_ms else {
            return AutomaticRetryGate::Run;
        };
        let now = current_time_ms();
        if blocked_until_ms <= now {
            return AutomaticRetryGate::Run;
        }
        AutomaticRetryGate::Wait(Duration::from_millis(blocked_until_ms.saturating_sub(now)))
    }

    async fn record_automatic_retry_failure(&self, trigger: &str, error: &AppError) {
        if !is_automatic_trigger(trigger) {
            return;
        }

        let mut retry = self.automatic_retry.lock().await;
        if is_non_retryable_automatic_error(error) {
            retry.suspended_until_settings_change = true;
            retry.blocked_until_ms = None;
            return;
        }

        retry.consecutive_failures = retry.consecutive_failures.saturating_add(1);
        let index = retry
            .consecutive_failures
            .saturating_sub(1)
            .min((AUTOMATIC_RETRY_BACKOFF_MS.len() - 1) as u32) as usize;
        retry.blocked_until_ms =
            Some(current_time_ms().saturating_add(AUTOMATIC_RETRY_BACKOFF_MS[index]));
    }

    async fn build_remote_with_recovery(
        &self,
        mut settings: CloudSyncSettings,
    ) -> AppResult<super::operator::CloudRemote> {
        if settings.provider != "github_gist" {
            return build_remote(&settings);
        }

        let Some(access_token) = settings
            .github_gist
            .access_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return build_remote(&settings);
        };

        let existing_gist_id = settings.github_gist.gist_id.clone();
        let resolved_gist_id = super::github_gist_auth::resolve_github_gist_id(
            access_token,
            Some(existing_gist_id.clone()),
        )
        .await?;

        if resolved_gist_id != existing_gist_id.trim() {
            tracing::warn!(
                old_gist_id = %existing_gist_id,
                new_gist_id = %resolved_gist_id,
                "GitHub Gist sync storage was missing; created a replacement gist"
            );
            settings.github_gist.gist_id = resolved_gist_id;
            self.persist_recovered_settings(settings.clone()).await?;
            self.refresh_status_after_recovered_settings(&settings)
                .await;
        }

        build_remote(&settings)
    }

    async fn persist_recovered_settings(&self, settings: CloudSyncSettings) -> AppResult<()> {
        *self.settings.lock().await = settings.clone();

        let app = self.app()?;
        let mut app_settings = config::load_app_settings(&app)?;
        app_settings.cloud_sync = settings;

        let mut persisted_settings = app_settings.clone();
        persisted_settings.cloud_sync =
            config::encrypt_cloud_sync_settings(persisted_settings.cloud_sync)?;
        persisted_settings.ai = config::encrypt_ai_settings(persisted_settings.ai)?;
        config::save_app_settings(&app, &persisted_settings)?;

        let _ = app.emit("settings-changed", ());
        crate::tray::schedule_refresh(&app);
        Ok(())
    }

    async fn refresh_status_after_recovered_settings(&self, settings: &CloudSyncSettings) {
        let app = self.app().ok();
        let state = self.state.lock().await.clone();
        let status = {
            let mut status = self.status.lock().await;
            status.enabled = settings.enabled;
            status.provider = settings.provider.clone();
            status.last_checked_at_ms = state.last_checked_at_ms;
            status.last_synced_at_ms = state.last_synced_at_ms;
            status.clone()
        };
        if let Some(app) = app {
            let _ = app.emit("cloud-sync-status-changed", &status);
            crate::tray::schedule_refresh(&app);
        }
    }

    async fn set_status(
        &self,
        state_value: &str,
        message: String,
        current_operation: Option<String>,
        conflict: Option<CloudConflictPreview>,
    ) {
        self.set_status_with_code(state_value, message, current_operation, conflict, None)
            .await;
    }

    async fn set_status_with_code(
        &self,
        state_value: &str,
        message: String,
        current_operation: Option<String>,
        conflict: Option<CloudConflictPreview>,
        error_code: Option<String>,
    ) {
        let app = self.app().ok();
        let settings = self.settings.lock().await.clone();
        let state = self.state.lock().await.clone();
        let status = CloudSyncStatus {
            enabled: settings.enabled,
            provider: settings.provider.clone(),
            state: state_value.to_string(),
            message,
            error_code,
            current_operation,
            last_checked_at_ms: state.last_checked_at_ms,
            last_synced_at_ms: state.last_synced_at_ms,
            conflict: conflict.clone(),
        };
        *self.status.lock().await = status.clone();
        if let Some(app) = app {
            let _ = app.emit("cloud-sync-status-changed", &status);
            let _ = app.emit("cloud-sync-conflict", &conflict);
            crate::tray::schedule_refresh(&app);
        }
    }

    async fn set_status_after_settings_replace(&self, enabled: bool, provider: String) {
        let app = self.app().ok();
        let status = {
            let mut status = self.status.lock().await;
            status.enabled = enabled;
            status.provider = provider;
            status.state = if enabled { "idle" } else { "disabled" }.to_string();
            status.message.clear();
            status.error_code = None;
            status.current_operation = None;
            status.conflict = None;
            status.clone()
        };
        if let Some(app) = app {
            let _ = app.emit("cloud-sync-status-changed", &status);
            let _ = app.emit("cloud-sync-conflict", &Option::<CloudConflictPreview>::None);
            crate::tray::schedule_refresh(&app);
        }
    }

    fn app(&self) -> AppResult<tauri::AppHandle> {
        self.app_handle
            .get()
            .cloned()
            .ok_or_else(|| AppError::Config("cloud sync app handle is not initialized".to_string()))
    }
}

async fn wait_for_auto_push_debounce(settings: &Mutex<CloudSyncSettings>, notify: &Notify) {
    loop {
        let debounce_secs = settings.lock().await.sync_debounce_seconds.max(1);
        if tokio::time::timeout(Duration::from_secs(debounce_secs), notify.notified())
            .await
            .is_err()
        {
            return;
        }
    }
}

async fn with_operation_timeout<T, F>(
    operation: &str,
    timeout_duration: Duration,
    future: F,
) -> AppResult<T>
where
    F: Future<Output = AppResult<T>>,
{
    tokio::time::timeout(timeout_duration, future)
        .await
        .map_err(|_| {
            AppError::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "Cloud sync operation '{}' timed out after {} seconds",
                    operation,
                    timeout_duration.as_secs()
                ),
            ))
        })?
}

async fn trace_cloud_sync_step<T, F>(operation: &str, step: &str, future: F) -> AppResult<T>
where
    F: Future<Output = AppResult<T>>,
{
    tracing::info!(operation, step, "Cloud sync operation step started");
    let started = Instant::now();
    let result = future.await;
    let duration_ms = elapsed_ms(started.elapsed());
    match &result {
        Ok(_) => tracing::info!(
            operation,
            step,
            duration_ms,
            "Cloud sync operation step completed"
        ),
        Err(error) => tracing::warn!(
            operation,
            step,
            duration_ms,
            error = %error,
            "Cloud sync operation step failed"
        ),
    }
    result
}

/// Free gist capacity before an upload. Remote inspection can fail on its own, so
/// this is best-effort: it is traced, but it never fails the push.
async fn prune_gist_snapshots_step(
    trigger: &str,
    remote: &super::operator::CloudRemote,
    remote_root: &str,
    latest: Option<&RemoteSyncPointer>,
    target_revision: &str,
) {
    let _ = trace_cloud_sync_step(trigger, "prune_gist_snapshots_before_upload", async {
        prune_gist_snapshots_best_effort(remote, remote_root, latest, target_revision).await;
        Ok::<(), AppError>(())
    })
    .await;
}

/// Publish only after the encrypted snapshot is read back and both conflict
/// inputs are rechecked. This is a read-before-write check, not a provider CAS.
async fn publish_verified_merge_snapshot<F, Fut>(
    trigger: &str,
    remote: &super::operator::CloudRemote,
    settings: &CloudSyncSettings,
    latest: &RemoteSyncPointer,
    merged: &PortableSnapshot,
    expected_local_hash: &str,
    current_local_hash: F,
) -> AppResult<()>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = AppResult<String>>,
{
    let pointer = pointer_from_snapshot(merged);
    let gist_backend = is_gist_provider(&settings.provider);
    if gist_backend {
        prune_gist_snapshots_step(
            trigger,
            remote,
            &settings.remote_root,
            Some(latest),
            &merged.revision_id,
        )
        .await;
    }
    let mut attempts = 0;
    loop {
        attempts += 1;
        let upload = trace_cloud_sync_step(trigger, "upload_sync_snapshot", async {
            upload_sync_snapshot(remote, &settings.remote_root, merged).await
        })
        .await;
        match upload {
            Ok(()) => {}
            Err(error)
                if gist_backend
                    && attempts < 2
                    && matches!(
                        &error,
                        AppError::CloudSync(CloudSyncError::RemoteFileRejected { .. })
                    ) =>
            {
                prune_gist_snapshots_step(
                    trigger,
                    remote,
                    &settings.remote_root,
                    Some(latest),
                    &merged.revision_id,
                )
                .await;
                continue;
            }
            Err(error) => return Err(error),
        }

        match trace_cloud_sync_step(trigger, "verify_uploaded_sync_snapshot", async {
            verify_uploaded_sync_snapshot(remote, &settings.remote_root, &pointer).await
        })
        .await
        {
            Ok(_) => break,
            Err(error)
                if gist_backend
                    && attempts < 2
                    && matches!(
                        &error,
                        AppError::CloudSync(
                            CloudSyncError::SnapshotNotAccepted { .. }
                                | CloudSyncError::RemoteFileRejected { .. }
                        )
                    ) =>
            {
                prune_gist_snapshots_step(
                    trigger,
                    remote,
                    &settings.remote_root,
                    Some(latest),
                    &merged.revision_id,
                )
                .await;
            }
            Err(error) => return Err(error),
        }
    }
    if current_local_hash().await? != expected_local_hash {
        return Err(AppError::Config(
            "Local cloud sync data changed while merging; check for conflicts again".into(),
        ));
    }
    trace_cloud_sync_step(trigger, "recheck_sync_pointer", async {
        let actual = load_sync_pointer(remote, &settings.remote_root).await?;
        confirm_merge_remote_head(latest, actual.as_ref())
    })
    .await?;
    trace_cloud_sync_step(trigger, "commit_sync_pointer", async {
        commit_sync_pointer(remote, &settings.remote_root, &pointer).await
    })
    .await
}

fn confirm_merge_remote_head(
    expected: &RemoteSyncPointer,
    actual: Option<&RemoteSyncPointer>,
) -> AppResult<()> {
    if !actual.is_some_and(|pointer| {
        pointer.revision_id == expected.revision_id && pointer.payload_hash == expected.payload_hash
    }) {
        return Err(AppError::Config(
            "Remote cloud sync pointer changed during merge; check for conflicts again".into(),
        ));
    }
    Ok(())
}

fn decide_remote_check(
    state: &CloudSyncState,
    local_hash: &str,
    remote: &RemoteSyncPointer,
    allow_auto_pull: bool,
) -> RemoteCheckDecision {
    if remote.payload_hash == local_hash {
        return RemoteCheckDecision::UpToDate;
    }

    let local_changed = state
        .last_synced_payload_hash
        .as_deref()
        .map_or(true, |hash| hash != local_hash);
    let remote_changed = state
        .last_applied_remote_revision
        .as_deref()
        .map_or(true, |revision| revision != remote.revision_id);

    match (remote_changed, local_changed, allow_auto_pull) {
        (true, true, _) => RemoteCheckDecision::Conflict,
        (true, false, true) => RemoteCheckDecision::AutoPull,
        (true, false, false) => RemoteCheckDecision::RemoteAvailable,
        (false, true, _) => RemoteCheckDecision::LocalChanged,
        (false, false, _) => RemoteCheckDecision::UpToDate,
    }
}

fn cloud_conflict_preview(
    settings: &CloudSyncSettings,
    local_hash: &str,
    remote: &RemoteSyncPointer,
) -> CloudConflictPreview {
    CloudConflictPreview {
        detected_at_ms: current_time_ms(),
        provider: settings.provider.clone(),
        kind: "content_conflict".to_string(),
        local_payload_hash: local_hash.to_string(),
        remote_payload_hash: remote.payload_hash.clone(),
        remote_revision: remote.revision_id.clone(),
        remote_created_at_ms: remote.created_at_ms,
        remote_device_id: remote.device_id.clone(),
        recovery_revision: None,
        recovery_payload_hash: None,
        recovery_created_at_ms: None,
        message: "Both local and cloud state changed since last sync".to_string(),
    }
}

fn remote_inconsistent_preview(
    settings: &CloudSyncSettings,
    local_hash: &str,
    pointer: &RemoteSyncPointer,
    recovery_candidate: &PortableSnapshot,
) -> CloudConflictPreview {
    CloudConflictPreview {
        detected_at_ms: current_time_ms(),
        provider: settings.provider.clone(),
        kind: "remote_inconsistent".to_string(),
        local_payload_hash: local_hash.to_string(),
        remote_payload_hash: pointer.payload_hash.clone(),
        remote_revision: pointer.revision_id.clone(),
        remote_created_at_ms: pointer.created_at_ms,
        remote_device_id: pointer.device_id.clone(),
        recovery_revision: Some(recovery_candidate.revision_id.clone()),
        recovery_payload_hash: Some(recovery_candidate.payload_hash.clone()),
        recovery_created_at_ms: Some(recovery_candidate.created_at_ms),
        message: "Remote cloud sync metadata is incomplete. The latest pointer references a missing snapshot, but current.redb.enc contains a recoverable snapshot.".to_string(),
    }
}

fn remote_validation_reason(
    state: &CloudSyncState,
    pointer: &RemoteSyncPointer,
    now_ms: u64,
) -> Option<&'static str> {
    if state.last_validated_remote_revision.as_deref() != Some(pointer.revision_id.as_str()) {
        return Some("pointer_changed");
    }
    maintenance_due(
        state.last_full_validation_at_ms,
        now_ms,
        CLOUD_SYNC_FULL_VALIDATION_INTERVAL_MS,
    )
    .then_some("scheduled_daily_validation")
}

fn maintenance_due(last_attempt_ms: Option<u64>, now_ms: u64, interval_ms: u64) -> bool {
    last_attempt_ms
        .is_none_or(|last_attempt_ms| now_ms.saturating_sub(last_attempt_ms) >= interval_ms)
}

/// A grace period when GC is due, or `None` when the daily throttle applies.
fn sync_snapshot_gc_policy(
    remote: &super::operator::CloudRemote,
    last_attempt_ms: Option<u64>,
    now_ms: u64,
) -> Option<Duration> {
    if remote.file_capacity_limit().is_some() {
        Some(Duration::ZERO)
    } else {
        maintenance_due(last_attempt_ms, now_ms, CLOUD_SYNC_GC_INTERVAL_MS)
            .then_some(SYNC_SNAPSHOT_GC_GRACE_PERIOD)
    }
}

fn should_skip_automatic_push(trigger: &str, state: &CloudSyncState, local_hash: &str) -> bool {
    trigger == "auto_push" && state.last_synced_payload_hash.as_deref() == Some(local_hash)
}

fn cloud_sync_connection_changed(previous: &CloudSyncSettings, next: &CloudSyncSettings) -> bool {
    previous.enabled != next.enabled
        || previous.provider != next.provider
        || previous.remote_root != next.remote_root
        || selected_provider_settings_changed(previous, next)
}

fn cloud_sync_remote_check_required(
    previous: &CloudSyncSettings,
    next: &CloudSyncSettings,
) -> bool {
    next.enabled
        && (!previous.enabled
            || cloud_sync_connection_changed(previous, next)
            || (!previous.auto_pull_remote_changes && next.auto_pull_remote_changes))
}

fn selected_provider_settings_changed(
    previous: &CloudSyncSettings,
    next: &CloudSyncSettings,
) -> bool {
    if previous.provider != next.provider {
        return true;
    }
    match next.provider.as_str() {
        "webdav" => previous.webdav != next.webdav,
        "s3" => previous.s3 != next.s3,
        "gitee_snippet" => previous.gitee_snippet != next.gitee_snippet,
        "google_drive" => previous.google_drive != next.google_drive,
        "onedrive" => previous.onedrive != next.onedrive,
        "aliyun_drive" => previous.aliyun_drive != next.aliyun_drive,
        "github_gist" => previous.github_gist != next.github_gist,
        _ => true,
    }
}

fn is_automatic_trigger(trigger: &str) -> bool {
    matches!(
        trigger,
        "auto_push" | "startup_check" | "periodic_check" | "focus_check" | "auto_pull_remote"
    )
}

fn is_non_retryable_automatic_error(error: &AppError) -> bool {
    match error {
        AppError::CloudSync(
            CloudSyncError::SnapshotNotAccepted { .. } | CloudSyncError::RemoteFileRejected { .. },
        ) => false,
        AppError::Auth(_) | AppError::Config(_) | AppError::Crypto(_) | AppError::CloudSync(_) => {
            true
        }
        _ => false,
    }
}

/// Machine-readable code for failures the UI offers a recovery action for.
///
/// The frontend must not parse prose error text: keeping the code here means
/// rewording a message can never silently disable the recovery button.
fn cloud_sync_failure_code(provider: &str, error: &AppError) -> Option<&'static str> {
    if provider != "gitee_snippet" {
        return None;
    }

    match error {
        AppError::CloudSync(
            CloudSyncError::SnapshotNotAccepted { .. } | CloudSyncError::RemoteFileRejected { .. },
        ) => Some(GIST_CAPACITY_ERROR_CODE),
        _ => None,
    }
}

/// `CloudSyncStatus.error_code` value for Gitee snippet capacity failures.
pub(super) const GIST_CAPACITY_ERROR_CODE: &str = "gist_capacity";

fn should_record_startup_check_failure(error: &AppError) -> bool {
    !matches!(error, AppError::Io(_))
}

fn should_skip_startup_network_check(settings: &CloudSyncSettings) -> bool {
    settings.provider == "github_gist"
}

fn should_skip_runtime_network_check(settings: &CloudSyncSettings) -> bool {
    settings.provider == "github_gist"
}

fn remote_check_label(trigger: &str) -> &'static str {
    match trigger {
        "startup_check" => "Startup",
        "focus_check" => "Focus",
        "periodic_check" => "Periodic",
        _ => "Runtime",
    }
}

pub async fn notify_config_changed(app: &tauri::AppHandle) {
    let manager = app.state::<Arc<CloudSyncManager>>();
    manager.inner().notify_config_changed().await;
}

#[cfg(test)]
mod tests {
    use super::super::MASTER_PASSWORD_TEST_LOCK;
    use super::super::operator::{CloudRemote, MemoryRemote};
    use super::super::protocol::read_snapshot_for_pointer;
    use super::*;
    use crate::config::{CloudSyncSettings, S3SyncSettings, WebdavSyncSettings};
    use crate::core::portable_snapshot::{PORTABLE_SNAPSHOT_SCHEMA_VERSION, PortableAppSettings};
    use crate::utils::crypto::set_master_password;
    use std::collections::HashMap;

    fn merge_test_snapshot(revision_id: &str) -> PortableSnapshot {
        let mut snapshot = PortableSnapshot {
            schema_version: PORTABLE_SNAPSHOT_SCHEMA_VERSION,
            snapshot_kind: PortableSnapshotKind::Sync,
            revision_id: revision_id.to_string(),
            device_id: "test-device".to_string(),
            created_at_ms: 100,
            payload_hash: String::new(),
            app_version: "test".to_string(),
            settings: PortableAppSettings::from_app_settings(
                &config::AppSettings::default(),
                &PortableSnapshotKind::Sync,
            ),
            sessions: Default::default(),
            keys: Default::default(),
            passwords: Default::default(),
            credentials: Default::default(),
            otp: Default::default(),
            proxies: Default::default(),
            proxy_groups: Default::default(),
            tunnels: Default::default(),
            tunnel_groups: Default::default(),
            quick_commands: Default::default(),
            history: Default::default(),
            master_key_token: None,
            known_hosts: String::new(),
            notes: Default::default(),
        };
        snapshot.payload_hash = calculate_payload_hash(&snapshot).expect("snapshot hash");
        snapshot
    }

    async fn merge_test_remote() -> (
        MemoryRemote,
        CloudRemote,
        CloudSyncSettings,
        RemoteSyncPointer,
    ) {
        let memory = MemoryRemote::with_files(HashMap::new());
        let remote = CloudRemote::Memory(memory.clone());
        let mut settings = CloudSyncSettings::default();
        settings.remote_root = "nyaterm".to_string();
        let base = merge_test_snapshot("base");
        let pointer = pointer_from_snapshot(&base);
        upload_sync_snapshot(&remote, &settings.remote_root, &base)
            .await
            .expect("seed encrypted base snapshot");
        commit_sync_pointer(&remote, &settings.remote_root, &pointer)
            .await
            .expect("seed base pointer");
        (memory, remote, settings, pointer)
    }

    async fn assert_merge_pointer_is(
        remote: &CloudRemote,
        settings: &CloudSyncSettings,
        expected: &RemoteSyncPointer,
    ) {
        let head = load_sync_pointer(remote, &settings.remote_root)
            .await
            .expect("load pointer")
            .expect("pointer");
        assert_eq!(head.revision_id, expected.revision_id);
        assert_eq!(head.payload_hash, expected.payload_hash);
        read_snapshot_for_pointer(remote, &settings.remote_root, &head)
            .await
            .expect("original pointed-to snapshot still readable");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn publish_verified_merge_snapshot_rejects_failures_without_committing() {
        let _guard = MASTER_PASSWORD_TEST_LOCK
            .lock()
            .expect("master password lock");
        set_master_password(Some("merge-fixture-password".to_string()));
        let source_hash = "local-payload-hash";
        let merged = merge_test_snapshot("merged");

        // A rejected upload does not alter latest or the local source.
        let (memory, remote, settings, base) = merge_test_remote().await;
        memory.fail_next_write_containing("snapshots/merged");
        let failure = publish_verified_merge_snapshot(
            "test_merge",
            &remote,
            &settings,
            &base,
            &merged,
            source_hash,
            || async { Ok(source_hash.to_string()) },
        )
        .await
        .expect_err("injected snapshot upload failure");
        assert_merge_pointer_is(&remote, &settings, &base).await;
        let manager = CloudSyncManager::new();
        {
            let mut status = manager.status.lock().await;
            status.state = "conflict".to_string();
            status.conflict = Some(cloud_conflict_preview(&settings, source_hash, &base));
        }
        manager
            .record_failure("sync", "merge_connections", &failure)
            .await;
        let status = manager.status.lock().await;
        assert_eq!(status.state, "conflict");
        assert_eq!(
            status
                .conflict
                .as_ref()
                .map(|conflict| conflict.kind.as_str()),
            Some("content_conflict")
        );
        drop(status);
        let synced_state = manager.state.lock().await;
        assert!(synced_state.last_synced_payload_hash.is_none());
        assert!(synced_state.last_applied_remote_revision.is_none());
        drop(synced_state);

        // A provider that acknowledges but drops the encrypted upload must
        // fail readback verification before any latest pointer write.
        let (memory, remote, settings, base) = merge_test_remote().await;
        memory.drop_next_write_containing("snapshots/merged");
        assert!(
            publish_verified_merge_snapshot(
                "test_merge",
                &remote,
                &settings,
                &base,
                &merged,
                source_hash,
                || async { Ok(source_hash.to_string()) },
            )
            .await
            .is_err()
        );
        assert_merge_pointer_is(&remote, &settings, &base).await;

        // After a successful upload/readback, a changed local hash must still
        // leave the original pointer untouched.
        let (_memory, remote, settings, base) = merge_test_remote().await;
        assert!(
            publish_verified_merge_snapshot(
                "test_merge",
                &remote,
                &settings,
                &base,
                &merged,
                source_hash,
                || async { Ok("local-changed".to_string()) },
            )
            .await
            .is_err()
        );
        assert_merge_pointer_is(&remote, &settings, &base).await;

        // A competing device may advance the revision between upload and
        // commit. The merge must not overwrite that new pointer.
        let (_memory, remote, settings, base) = merge_test_remote().await;
        let competing = merge_test_snapshot("competing");
        let competing_pointer = pointer_from_snapshot(&competing);
        upload_sync_snapshot(&remote, &settings.remote_root, &competing)
            .await
            .expect("stage concurrent snapshot");
        let remote_for_concurrent_write = remote.clone();
        let root = settings.remote_root.clone();
        let expected_competing_pointer = competing_pointer.clone();
        assert!(
            publish_verified_merge_snapshot(
                "test_merge",
                &remote,
                &settings,
                &base,
                &merged,
                source_hash,
                || async move {
                    commit_sync_pointer(&remote_for_concurrent_write, &root, &competing_pointer)
                        .await?;
                    Ok(source_hash.to_string())
                },
            )
            .await
            .is_err()
        );
        assert_merge_pointer_is(&remote, &settings, &expected_competing_pointer).await;

        // Even if the revision is unchanged, a different remote payload hash
        // must not be replaced by the stale merged snapshot.
        let (_memory, remote, settings, base) = merge_test_remote().await;
        let mut changed_hash = base.clone();
        changed_hash.payload_hash = "changed-payload-hash".to_string();
        let changed_hash_for_assertion = changed_hash.clone();
        let remote_for_concurrent_write = remote.clone();
        let root = settings.remote_root.clone();
        assert!(
            publish_verified_merge_snapshot(
                "test_merge",
                &remote,
                &settings,
                &base,
                &merged,
                source_hash,
                || async move {
                    commit_sync_pointer(&remote_for_concurrent_write, &root, &changed_hash).await?;
                    Ok(source_hash.to_string())
                },
            )
            .await
            .is_err()
        );
        let head = load_sync_pointer(&remote, &settings.remote_root)
            .await
            .expect("read changed head")
            .expect("head");
        assert_eq!(head.revision_id, changed_hash_for_assertion.revision_id);
        assert_eq!(head.payload_hash, changed_hash_for_assertion.payload_hash);
        set_master_password(None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn publish_verified_merge_snapshot_commits_readable_revision() {
        let _guard = MASTER_PASSWORD_TEST_LOCK
            .lock()
            .expect("master password lock");
        set_master_password(Some("merge-fixture-password".to_string()));
        let (_memory, remote, settings, base) = merge_test_remote().await;
        let merged = merge_test_snapshot("merged");
        publish_verified_merge_snapshot(
            "test_merge",
            &remote,
            &settings,
            &base,
            &merged,
            "local-payload-hash",
            || async { Ok("local-payload-hash".to_string()) },
        )
        .await
        .expect("publish verified merge");
        let head = load_sync_pointer(&remote, &settings.remote_root)
            .await
            .expect("read merged head")
            .expect("head");
        assert_eq!(head.revision_id, merged.revision_id);
        assert_eq!(head.payload_hash, merged.payload_hash);
        let decoded = read_snapshot_for_pointer(&remote, &settings.remote_root, &head)
            .await
            .expect("decrypt and validate committed merged snapshot");
        assert_eq!(decoded.revision_id, merged.revision_id);
        assert_eq!(decoded.payload_hash, merged.payload_hash);
        set_master_password(None);
    }

    #[test]
    fn default_manager_constructs() {
        let _ = CloudSyncManager::new();
    }

    #[test]
    fn cloud_sync_settings_support_both_provider_shapes() {
        let mut settings = CloudSyncSettings::default();
        settings.provider = "webdav".to_string();
        settings.webdav = WebdavSyncSettings {
            endpoint: "https://dav.example.com".to_string(),
            root: "/nyaterm".to_string(),
            username: "user".to_string(),
            password: Some("cipher".to_string()),
        };
        settings.s3 = S3SyncSettings {
            endpoint: "https://s3.example.com".to_string(),
            bucket: "bucket".to_string(),
            region: "auto".to_string(),
            root: "/nyaterm".to_string(),
            access_key_id: Some("cipher".to_string()),
            secret_access_key: Some("cipher".to_string()),
            session_token: None,
            virtual_host_style: true,
        };

        assert_eq!(settings.provider, "webdav");
        assert!(settings.webdav.password.is_some());
        assert!(settings.s3.secret_access_key.is_some());
    }

    #[test]
    fn merge_recheck_requires_both_remote_revision_and_hash() {
        let expected = remote_pointer("r1", "hash-a");
        assert!(
            confirm_merge_remote_head(&expected, Some(&remote_pointer("r1", "hash-a"))).is_ok()
        );
        assert!(
            confirm_merge_remote_head(&expected, Some(&remote_pointer("r2", "hash-a"))).is_err()
        );
        assert!(
            confirm_merge_remote_head(&expected, Some(&remote_pointer("r1", "hash-b"))).is_err()
        );
        assert!(confirm_merge_remote_head(&expected, None).is_err());
    }

    fn remote_pointer(revision_id: &str, payload_hash: &str) -> RemoteSyncPointer {
        RemoteSyncPointer {
            schema_version: 2,
            revision_id: revision_id.to_string(),
            created_at_ms: 2,
            payload_hash: payload_hash.to_string(),
            device_id: "remote-device".to_string(),
            app_version: "test".to_string(),
        }
    }

    fn synced_state(revision_id: &str, payload_hash: &str) -> CloudSyncState {
        CloudSyncState {
            device_id: "local-device".to_string(),
            last_synced_payload_hash: Some(payload_hash.to_string()),
            last_applied_remote_revision: Some(revision_id.to_string()),
            last_checked_at_ms: None,
            last_synced_at_ms: None,
            ..CloudSyncState::default()
        }
    }

    #[test]
    fn remote_check_decides_up_to_date_when_local_and_remote_unchanged() {
        let state = synced_state("r1", "hash-1");
        let remote = remote_pointer("r1", "hash-1");

        assert_eq!(
            decide_remote_check(&state, "hash-1", &remote, true),
            RemoteCheckDecision::UpToDate
        );
    }

    #[test]
    fn remote_check_decides_auto_pull_when_remote_changed_and_local_clean() {
        let state = synced_state("r1", "hash-1");
        let remote = remote_pointer("r2", "hash-2");

        assert_eq!(
            decide_remote_check(&state, "hash-1", &remote, true),
            RemoteCheckDecision::AutoPull
        );
    }

    #[test]
    fn remote_check_decides_remote_available_when_auto_pull_disabled() {
        let state = synced_state("r1", "hash-1");
        let remote = remote_pointer("r2", "hash-2");

        assert_eq!(
            decide_remote_check(&state, "hash-1", &remote, false),
            RemoteCheckDecision::RemoteAvailable
        );
    }

    #[test]
    fn remote_check_decides_local_changed_when_only_local_changed() {
        let state = synced_state("r1", "hash-1");
        let remote = remote_pointer("r1", "hash-remote");

        assert_eq!(
            decide_remote_check(&state, "hash-local", &remote, true),
            RemoteCheckDecision::LocalChanged
        );
    }

    #[test]
    fn remote_check_decides_conflict_when_local_and_remote_changed() {
        let state = synced_state("r1", "hash-1");
        let remote = remote_pointer("r2", "hash-2");

        assert_eq!(
            decide_remote_check(&state, "hash-local", &remote, true),
            RemoteCheckDecision::Conflict
        );
    }

    #[test]
    fn remote_check_treats_matching_payload_as_up_to_date_despite_revision_change() {
        let state = synced_state("r1", "hash-1");
        let remote = remote_pointer("r2", "hash-local");

        assert_eq!(
            decide_remote_check(&state, "hash-local", &remote, true),
            RemoteCheckDecision::UpToDate
        );
    }

    #[test]
    fn remote_validation_runs_on_pointer_change_and_once_per_day() {
        let pointer = remote_pointer("r1", "hash-1");
        let mut state = synced_state("r1", "hash-1");

        assert_eq!(
            remote_validation_reason(&state, &pointer, 1_000),
            Some("pointer_changed")
        );

        state.last_validated_remote_revision = Some("r1".to_string());
        state.last_full_validation_at_ms = Some(1_000);
        assert_eq!(
            remote_validation_reason(
                &state,
                &pointer,
                1_000 + CLOUD_SYNC_FULL_VALIDATION_INTERVAL_MS - 1,
            ),
            None
        );
        assert_eq!(
            remote_validation_reason(
                &state,
                &pointer,
                1_000 + CLOUD_SYNC_FULL_VALIDATION_INTERVAL_MS,
            ),
            Some("scheduled_daily_validation")
        );

        let changed = remote_pointer("r2", "hash-2");
        assert_eq!(
            remote_validation_reason(&state, &changed, 1_001),
            Some("pointer_changed")
        );
    }

    #[test]
    fn maintenance_gate_runs_at_most_once_per_interval() {
        assert!(maintenance_due(None, 100, CLOUD_SYNC_GC_INTERVAL_MS));
        assert!(!maintenance_due(
            Some(100),
            100 + CLOUD_SYNC_GC_INTERVAL_MS - 1,
            CLOUD_SYNC_GC_INTERVAL_MS,
        ));
        assert!(maintenance_due(
            Some(100),
            100 + CLOUD_SYNC_GC_INTERVAL_MS,
            CLOUD_SYNC_GC_INTERVAL_MS,
        ));
    }

    #[test]
    fn snapshot_gc_policy_is_aggressive_only_for_hard_capacity_remotes() {
        let mut settings = CloudSyncSettings::default();
        settings.gitee_snippet.api_endpoint = "https://gitee.com/api/v5".to_string();
        settings.gitee_snippet.gist_id = "abc".to_string();
        settings.gitee_snippet.access_token = Some("token".to_string());
        settings.github_gist.gist_id = "abc".to_string();
        settings.github_gist.access_token = Some("token".to_string());
        settings.webdav.endpoint = "https://dav.example.com".to_string();
        settings.s3.bucket = "test-bucket".to_string();
        settings.s3.region = "us-east-1".to_string();

        for provider in ["gitee_snippet", "github_gist", "webdav", "s3"] {
            settings.provider = provider.to_string();
            let remote = build_remote(&settings).expect("build remote");
            let grace_period = if provider == "gitee_snippet" {
                Duration::ZERO
            } else {
                SYNC_SNAPSHOT_GC_GRACE_PERIOD
            };
            assert_eq!(
                sync_snapshot_gc_policy(&remote, None, 100),
                Some(grace_period),
                "{provider}: initial cleanup"
            );
            assert_eq!(
                sync_snapshot_gc_policy(&remote, Some(100), 101),
                (provider == "gitee_snippet").then_some(Duration::ZERO),
                "{provider}: cleanup within daily throttle"
            );
            assert_eq!(
                sync_snapshot_gc_policy(&remote, Some(100), 100 + CLOUD_SYNC_GC_INTERVAL_MS),
                Some(grace_period),
                "{provider}: daily cleanup"
            );
        }
    }

    #[test]
    fn unchanged_payload_only_skips_automatic_pushes() {
        let state = synced_state("r1", "hash-1");

        assert!(should_skip_automatic_push("auto_push", &state, "hash-1"));
        assert!(!should_skip_automatic_push("manual_push", &state, "hash-1"));
        assert!(!should_skip_automatic_push("auto_push", &state, "hash-2"));
    }

    #[test]
    fn cloud_sync_settings_only_request_checks_for_remote_relevant_changes() {
        let mut previous = CloudSyncSettings::default();
        previous.enabled = true;
        previous.webdav.endpoint = "https://dav.example.com".to_string();

        let mut debounce_only = previous.clone();
        debounce_only.sync_debounce_seconds = 120;
        assert!(!cloud_sync_connection_changed(&previous, &debounce_only));
        assert!(!cloud_sync_remote_check_required(&previous, &debounce_only));

        let mut device_name_only = previous.clone();
        device_name_only.device_name = "Another Device".to_string();
        assert!(!cloud_sync_remote_check_required(
            &previous,
            &device_name_only
        ));

        let mut unused_provider = previous.clone();
        unused_provider.s3.endpoint = "https://s3.example.com".to_string();
        assert!(!cloud_sync_connection_changed(&previous, &unused_provider));

        let mut endpoint_changed = previous.clone();
        endpoint_changed.webdav.endpoint = "https://dav2.example.com".to_string();
        assert!(cloud_sync_connection_changed(&previous, &endpoint_changed));
        assert!(cloud_sync_remote_check_required(
            &previous,
            &endpoint_changed
        ));

        let mut auto_pull_enabled = previous.clone();
        previous.auto_pull_remote_changes = false;
        auto_pull_enabled.auto_pull_remote_changes = true;
        assert!(cloud_sync_remote_check_required(
            &previous,
            &auto_pull_enabled
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn auto_push_debounce_waits_after_the_latest_notification() {
        let mut settings = CloudSyncSettings::default();
        settings.sync_debounce_seconds = 1;
        let settings = Arc::new(Mutex::new(settings));
        let notify = Arc::new(Notify::new());
        let task_settings = Arc::clone(&settings);
        let task_notify = Arc::clone(&notify);
        let task = tokio::spawn(async move {
            wait_for_auto_push_debounce(&task_settings, &task_notify).await;
        });

        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(500)).await;
        notify.notify_one();
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(999)).await;
        assert!(!task.is_finished());
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        assert!(task.is_finished());
        task.await.expect("debounce task");
    }

    #[test]
    fn automatic_retry_classifies_auth_and_config_as_non_retryable() {
        assert!(is_non_retryable_automatic_error(&AppError::Auth(
            "bad credentials".to_string()
        )));
        assert!(is_non_retryable_automatic_error(&AppError::Config(
            "invalid endpoint".to_string()
        )));
        assert!(!is_non_retryable_automatic_error(&AppError::Io(
            std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout")
        )));
        assert!(!is_non_retryable_automatic_error(&AppError::CloudSync(
            CloudSyncError::SnapshotNotAccepted {
                revision: "r1".to_string(),
            }
        )));
        assert!(!is_non_retryable_automatic_error(&AppError::CloudSync(
            CloudSyncError::RemoteFileRejected {
                filename: "nyaterm-x.blob".to_string(),
            }
        )));
        assert!(is_non_retryable_automatic_error(&AppError::CloudSync(
            CloudSyncError::SnapshotMissing {
                revision: "r1".to_string(),
            }
        )));
    }

    #[test]
    fn automatic_trigger_detection_is_limited_to_background_work() {
        assert!(is_automatic_trigger("auto_push"));
        assert!(is_automatic_trigger("startup_check"));
        assert!(is_automatic_trigger("periodic_check"));
        assert!(is_automatic_trigger("focus_check"));
        assert!(is_automatic_trigger("auto_pull_remote"));
        assert!(!is_automatic_trigger("manual_push"));
        assert!(!is_automatic_trigger("manual_test_connection"));
    }

    #[test]
    fn gist_capacity_failure_code_is_limited_to_gitee_snippet() {
        for error in [
            CloudSyncError::SnapshotNotAccepted {
                revision: "r1".to_string(),
            },
            CloudSyncError::RemoteFileRejected {
                filename: "nyaterm-x.blob".to_string(),
            },
        ] {
            let error = AppError::CloudSync(error);
            assert_eq!(
                cloud_sync_failure_code("gitee_snippet", &error),
                Some(GIST_CAPACITY_ERROR_CODE)
            );
            for provider in ["github_gist", "webdav", "s3"] {
                assert_eq!(
                    cloud_sync_failure_code(provider, &error),
                    None,
                    "{provider}"
                );
            }
        }
        assert_eq!(
            cloud_sync_failure_code(
                "gitee_snippet",
                &AppError::CloudSync(CloudSyncError::ConcurrentUpdate {
                    expected_revision: None,
                    actual_revision: None,
                })
            ),
            None
        );
    }

    #[tokio::test]
    async fn record_failure_publishes_the_gist_capacity_code() {
        let manager = CloudSyncManager::new();
        manager.settings.lock().await.provider = "gitee_snippet".to_string();

        manager
            .record_failure(
                "sync",
                "manual_push",
                &AppError::CloudSync(CloudSyncError::RemoteFileRejected {
                    filename: "nyaterm-x.blob".to_string(),
                }),
            )
            .await;

        let status = manager.status.lock().await.clone();
        assert_eq!(status.state, "failed");
        assert_eq!(status.error_code.as_deref(), Some(GIST_CAPACITY_ERROR_CODE));
    }

    #[tokio::test]
    async fn record_failure_leaves_the_code_empty_for_unrelated_errors() {
        let manager = CloudSyncManager::new();

        manager
            .record_failure(
                "sync",
                "manual_push",
                &AppError::Io(std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout")),
            )
            .await;

        let status = manager.status.lock().await.clone();
        assert_eq!(status.state, "failed");
        assert!(status.error_code.is_none());
    }

    #[tokio::test]
    async fn record_failure_does_not_publish_capacity_code_for_github_gist() {
        let manager = CloudSyncManager::new();
        manager.settings.lock().await.provider = "github_gist".to_string();
        manager.status.lock().await.error_code = Some(GIST_CAPACITY_ERROR_CODE.to_string());

        manager
            .record_failure(
                "sync",
                "manual_push",
                &AppError::CloudSync(CloudSyncError::RemoteFileRejected {
                    filename: "nyaterm-x.blob".to_string(),
                }),
            )
            .await;

        let status = manager.status.lock().await.clone();
        assert_eq!(status.state, "failed");
        assert!(status.error_code.is_none());
    }

    #[tokio::test]
    async fn record_failure_clears_current_operation() {
        let manager = CloudSyncManager::new();
        *manager.status.lock().await = CloudSyncStatus {
            state: "running".to_string(),
            current_operation: Some("sync_push".to_string()),
            ..CloudSyncStatus::default()
        };

        manager
            .record_failure(
                "sync",
                "manual_push",
                &AppError::Io(std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout")),
            )
            .await;

        let status = manager.status.lock().await.clone();
        assert_eq!(status.state, "failed");
        assert!(status.current_operation.is_none());
    }

    #[tokio::test]
    async fn test_connection_success_clears_running_status() {
        let manager = CloudSyncManager::new();
        *manager.status.lock().await = CloudSyncStatus {
            state: "running".to_string(),
            message: "Checking cloud sync storage access".to_string(),
            current_operation: Some("test_connection".to_string()),
            conflict: Some(CloudConflictPreview {
                detected_at_ms: 1,
                provider: "webdav".to_string(),
                kind: "content_conflict".to_string(),
                local_payload_hash: "local".to_string(),
                remote_payload_hash: "remote".to_string(),
                remote_revision: "revision".to_string(),
                remote_created_at_ms: 2,
                remote_device_id: "device".to_string(),
                recovery_revision: None,
                recovery_payload_hash: None,
                recovery_created_at_ms: None,
                message: "conflict".to_string(),
            }),
            ..CloudSyncStatus::default()
        };

        manager.complete_test_connection_success().await;

        let status = manager.status.lock().await.clone();
        assert_eq!(status.state, "idle");
        assert_eq!(status.message, "Cloud connection verified");
        assert!(status.current_operation.is_none());
        assert!(status.conflict.is_none());
    }

    #[tokio::test]
    async fn status_update_after_state_save_does_not_deadlock() {
        let manager = CloudSyncManager::new();

        {
            let mut state = manager.state.lock().await;
            state.last_checked_at_ms = Some(current_time_ms());
        }

        tokio::time::timeout(
            Duration::from_millis(100),
            manager.set_status(
                "idle",
                "Cloud sync is already up to date".to_string(),
                None,
                None,
            ),
        )
        .await
        .expect("set_status should not wait on a retained state lock");

        let status = manager.status.lock().await.clone();
        assert_eq!(status.state, "idle");
        assert_eq!(status.message, "Cloud sync is already up to date");
    }

    #[tokio::test]
    async fn automatic_timeout_failure_uses_retry_backoff() {
        let manager = CloudSyncManager::new();
        let error = AppError::Io(std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout"));

        manager
            .record_automatic_retry_failure("auto_push", &error)
            .await;

        let retry = manager.automatic_retry.lock().await.clone();
        assert!(!retry.suspended_until_settings_change);
        assert!(retry.blocked_until_ms.is_some());
    }

    #[tokio::test]
    async fn startup_check_skips_when_operation_lock_is_busy() {
        let manager = Arc::new(CloudSyncManager::new());
        let _guard = manager.operation_lock.lock().await;

        manager
            .startup_check()
            .await
            .expect("busy startup check should skip cleanly");

        let status = manager.status.lock().await.clone();
        assert_eq!(status.state, "idle");
        assert!(status.message.contains("skipped"));
    }

    #[tokio::test]
    async fn runtime_check_skips_when_cloud_sync_is_disabled() {
        let manager = Arc::new(CloudSyncManager::new());

        manager.run_runtime_remote_check("periodic_check").await;

        let status = manager.status.lock().await.clone();
        assert_eq!(status.state, CloudSyncStatus::default().state);
        assert!(status.message.is_empty());
        assert!(status.current_operation.is_none());
    }

    #[tokio::test]
    async fn runtime_check_skips_github_gist_provider() {
        let manager = Arc::new(CloudSyncManager::new());
        {
            let mut settings = manager.settings.lock().await;
            settings.enabled = true;
            settings.provider = "github_gist".to_string();
        }

        manager.run_runtime_remote_check("periodic_check").await;

        let status = manager.status.lock().await.clone();
        assert_eq!(status.state, CloudSyncStatus::default().state);
        assert!(status.message.is_empty());
        assert!(status.current_operation.is_none());
    }

    #[tokio::test]
    async fn runtime_io_failure_uses_retry_backoff_without_noisy_status() {
        let manager = CloudSyncManager::new();
        let error = AppError::Io(std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout"));

        manager
            .handle_runtime_check_failure("periodic_check", error)
            .await;

        let status = manager.status.lock().await.clone();
        assert_eq!(status.state, CloudSyncStatus::default().state);
        assert!(status.message.is_empty());
        assert!(status.current_operation.is_none());

        let retry = manager.automatic_retry.lock().await.clone();
        assert_eq!(retry.consecutive_failures, 1);
        assert!(retry.blocked_until_ms.is_some());
        assert!(!retry.suspended_until_settings_change);
    }

    #[tokio::test]
    async fn runtime_non_retryable_failure_suspends_retry_and_updates_status() {
        let manager = CloudSyncManager::new();

        manager
            .handle_runtime_check_failure(
                "periodic_check",
                AppError::Config("bad cloud sync config".to_string()),
            )
            .await;

        let status = manager.status.lock().await.clone();
        assert_eq!(status.state, "failed");
        assert!(status.message.contains("bad cloud sync config"));

        let retry = manager.automatic_retry.lock().await.clone();
        assert!(retry.suspended_until_settings_change);
        assert!(retry.blocked_until_ms.is_none());
    }

    #[tokio::test]
    async fn startup_io_failure_does_not_use_failure_history_or_retry_backoff() {
        let manager = CloudSyncManager::new();
        let error = AppError::Io(std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout"));

        manager.handle_startup_check_failure(error).await;

        let status = manager.status.lock().await.clone();
        assert_eq!(status.state, "idle");
        assert!(status.message.contains("skipped"));

        let retry = manager.automatic_retry.lock().await.clone();
        assert_eq!(retry.consecutive_failures, 0);
        assert!(retry.blocked_until_ms.is_none());
        assert!(!retry.suspended_until_settings_change);
    }

    #[test]
    fn startup_check_records_only_non_io_failures() {
        assert!(!should_record_startup_check_failure(&AppError::Io(
            std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout")
        )));
        assert!(should_record_startup_check_failure(&AppError::Config(
            "bad cloud sync config".to_string()
        )));
        assert!(should_record_startup_check_failure(&AppError::Auth(
            "bad credentials".to_string()
        )));
    }

    #[test]
    fn github_gist_skips_startup_network_check() {
        let mut settings = CloudSyncSettings::default();
        settings.provider = "github_gist".to_string();
        assert!(should_skip_startup_network_check(&settings));

        settings.provider = "webdav".to_string();
        assert!(!should_skip_startup_network_check(&settings));
    }
}
