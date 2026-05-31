use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use tauri::{async_runtime, Emitter, Manager};
use tokio::sync::{Mutex, Notify};

use crate::config::{
    self, CloudConflictPreview, CloudSyncHistoryEntry, CloudSyncSettings, CloudSyncState,
    CloudSyncStatus, RemoteBackupEntry,
};
use crate::error::{AppError, AppResult};

use super::crypto::{decrypt_snapshot_bytes, encrypt_snapshot_bytes, require_master_password};
use super::history_log::{log_history_entry, read_cloud_sync_history_from_logs};
use super::operator::{build_operator, ensure_remote_layout, map_storage_error};
use super::remote::{
    current_time_ms, elapsed_ms, load_backup_index, load_sync_pointer, remote_path,
    write_backup_index, write_sync_pointer, RemoteSyncPointer, BACKUPS_SNAPSHOTS_DIR,
    SYNC_SNAPSHOTS_DIR,
};

use crate::core::portable_snapshot::{
    apply_portable_snapshot, build_portable_snapshot, decode_portable_snapshot,
    encode_portable_snapshot, PortableSnapshotKind,
};

const BACKUP_CHECK_INTERVAL: Duration = Duration::from_secs(60);
const AUTOMATIC_RETRY_BACKOFF_MS: [u64; 4] = [60_000, 300_000, 900_000, 3_600_000];

pub struct CloudSyncManager {
    app_handle: OnceLock<tauri::AppHandle>,
    settings: Arc<Mutex<CloudSyncSettings>>,
    state: Arc<Mutex<CloudSyncState>>,
    status: Arc<Mutex<CloudSyncStatus>>,
    automatic_retry: Arc<Mutex<AutomaticRetryState>>,
    auto_push_notify: Arc<Notify>,
    auto_push_worker_started: AtomicBool,
    backup_worker_started: AtomicBool,
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
            backup_worker_started: AtomicBool::new(false),
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
        self.ensure_backup_worker();

        self.set_status("idle", String::new(), None, None).await;

        let settings = self.settings.lock().await.clone();
        if settings.enabled && settings.auto_check_on_startup {
            let manager = Arc::clone(self);
            async_runtime::spawn(async move {
                if let Err(error) = manager.startup_check().await {
                    manager
                        .record_failure("sync", "startup_check", &error)
                        .await;
                    tracing::warn!("Startup cloud sync check failed: {}", error);
                }
            });
        }

        Ok(())
    }

    pub async fn replace_settings(&self, settings: CloudSyncSettings) -> AppResult<()> {
        *self.settings.lock().await = settings.clone();
        self.reset_automatic_retry().await;
        self.set_status(
            if settings.enabled { "idle" } else { "disabled" },
            String::new(),
            None,
            None,
        )
        .await;
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

    pub async fn test_connection(&self) -> AppResult<()> {
        let started = Instant::now();
        let settings = self.settings.lock().await.clone();
        let result = async {
            let _ = require_master_password()?;
            let operator = build_operator(&settings)?;
            ensure_remote_layout(&operator, &settings.remote_root).await?;
            let _ = operator
                .exists(&remote_path(&settings.remote_root, SYNC_SNAPSHOTS_DIR))
                .await
                .map_err(map_storage_error)?;
            Ok::<(), AppError>(())
        }
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
                Ok(())
            }
            Err(error) => {
                self.record_failure("sync", "manual_test_connection", &error)
                    .await;
                Err(error)
            }
        }
    }

    pub async fn sync_push_now(self: &Arc<Self>, trigger: &str) -> AppResult<()> {
        match self.push_snapshot(trigger, false).await {
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
        match self.pull_snapshot(trigger, false).await {
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

    pub async fn run_cloud_backup_now(self: &Arc<Self>, trigger: &str) -> AppResult<()> {
        match self.backup_snapshot(trigger).await {
            Ok(()) => {
                self.reset_automatic_retry().await;
                Ok(())
            }
            Err(error) => {
                self.record_failure("backup", trigger, &error).await;
                Err(error)
            }
        }
    }

    pub async fn resolve_cloud_sync_conflict(self: &Arc<Self>, action: &str) -> AppResult<()> {
        let result = match action {
            "upload_local" => self.push_snapshot("resolve_upload", true).await,
            "download_remote" => self.pull_snapshot("resolve_download", true).await,
            _ => Err(AppError::Config(format!(
                "Unsupported conflict resolution action '{}'",
                action
            ))),
        };

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

    pub async fn list_remote_backups(&self) -> AppResult<Vec<RemoteBackupEntry>> {
        let settings = self.settings.lock().await.clone();
        let operator = build_operator(&settings)?;
        let index = load_backup_index(&operator, &settings.remote_root).await?;
        Ok(index.entries)
    }

    pub async fn restore_remote_backup(
        self: &Arc<Self>,
        revision: &str,
        trigger: &str,
    ) -> AppResult<()> {
        let result = self.restore_remote_backup_inner(revision, trigger).await;
        if let Err(error) = &result {
            self.record_failure("backup", trigger, error).await;
        }
        result
    }

    async fn restore_remote_backup_inner(
        self: &Arc<Self>,
        revision: &str,
        trigger: &str,
    ) -> AppResult<()> {
        let _guard = self.operation_lock.lock().await;
        let _ = require_master_password()?;
        let settings = self.settings.lock().await.clone();
        self.set_status(
            "running",
            "Restoring remote backup".to_string(),
            Some("restore_remote_backup".to_string()),
            None,
        )
        .await;
        let started = Instant::now();
        let operator = build_operator(&settings)?;
        let remote_file = remote_path(
            &settings.remote_root,
            &format!("{BACKUPS_SNAPSHOTS_DIR}{revision}.redb.enc"),
        );
        let raw = operator
            .read(&remote_file)
            .await
            .map_err(map_storage_error)?;
        let decrypted = decrypt_snapshot_bytes(raw.to_vec().as_slice())?;
        let envelope = decode_portable_snapshot(&decrypted)?;
        apply_portable_snapshot(&self.app()?, &envelope).await?;

        {
            let mut state = self.state.lock().await;
            state.last_synced_payload_hash = None;
            state.last_applied_remote_revision = None;
            state.last_checked_at_ms = Some(current_time_ms());
            config::save_cloud_sync_state(&self.app()?, &state)?;
        }

        self.append_history(CloudSyncHistoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp_ms: current_time_ms(),
            kind: "backup".to_string(),
            status: "success".to_string(),
            trigger: trigger.to_string(),
            provider: Some(settings.provider.clone()),
            revision: Some(revision.to_string()),
            duration_ms: Some(elapsed_ms(started.elapsed())),
            message: "Remote backup restored".to_string(),
        })
        .await;
        self.set_status("idle", "Remote backup restored".to_string(), None, None)
            .await;
        self.reset_automatic_retry().await;
        Ok(())
    }

    async fn startup_check(self: &Arc<Self>) -> AppResult<()> {
        let _guard = self.operation_lock.lock().await;
        let settings = self.settings.lock().await.clone();
        if !settings.enabled {
            self.set_status("disabled", String::new(), None, None).await;
            return Ok(());
        }
        let _ = require_master_password()?;
        let operator = build_operator(&settings)?;
        ensure_remote_layout(&operator, &settings.remote_root).await?;

        let local_envelope = {
            let state = self.state.lock().await.clone();
            build_portable_snapshot(&self.app()?, PortableSnapshotKind::Sync, &state.device_id)?
        };
        let local_hash = local_envelope.payload_hash.clone();
        let latest = load_sync_pointer(&operator, &settings.remote_root).await?;

        {
            let mut state = self.state.lock().await;
            state.last_checked_at_ms = Some(current_time_ms());
            config::save_cloud_sync_state(&self.app()?, &state)?;
        }

        let Some(remote) = latest else {
            self.set_status(
                "idle",
                "No remote sync snapshot found".to_string(),
                None,
                None,
            )
            .await;
            return Ok(());
        };

        let state = self.state.lock().await.clone();
        let local_changed = state
            .last_synced_payload_hash
            .as_deref()
            .map_or(true, |hash| hash != local_hash);
        let remote_changed = state
            .last_applied_remote_revision
            .as_deref()
            .map_or(true, |revision| revision != remote.revision_id);

        if remote.payload_hash == local_hash {
            let mut state = self.state.lock().await;
            state.last_synced_payload_hash = Some(local_hash);
            state.last_applied_remote_revision = Some(remote.revision_id.clone());
            state.last_checked_at_ms = Some(current_time_ms());
            config::save_cloud_sync_state(&self.app()?, &state)?;
            self.set_status("idle", "Cloud sync is up to date".to_string(), None, None)
                .await;
            return Ok(());
        }

        if remote_changed && local_changed {
            let conflict = CloudConflictPreview {
                detected_at_ms: current_time_ms(),
                provider: settings.provider.clone(),
                local_payload_hash: local_hash,
                remote_payload_hash: remote.payload_hash.clone(),
                remote_revision: remote.revision_id.clone(),
                remote_created_at_ms: remote.created_at_ms,
                remote_device_id: remote.device_id.clone(),
                message: "Both local and cloud state changed since last sync".to_string(),
            };
            self.append_history(CloudSyncHistoryEntry {
                id: uuid::Uuid::new_v4().to_string(),
                timestamp_ms: current_time_ms(),
                kind: "sync".to_string(),
                status: "conflict".to_string(),
                trigger: "startup_check".to_string(),
                provider: Some(settings.provider.clone()),
                revision: Some(remote.revision_id.clone()),
                duration_ms: None,
                message: conflict.message.clone(),
            })
            .await;
            self.set_status("conflict", conflict.message.clone(), None, Some(conflict))
                .await;
            return Ok(());
        }

        if remote_changed {
            drop(_guard);
            return self.pull_snapshot("startup_check", false).await;
        }

        self.set_status("idle", "Local changes pending sync".to_string(), None, None)
            .await;
        Ok(())
    }

    fn ensure_auto_push_worker(self: &Arc<Self>) {
        if self.auto_push_worker_started.swap(true, Ordering::SeqCst) {
            return;
        }

        let manager = Arc::clone(self);
        async_runtime::spawn(async move {
            loop {
                manager.auto_push_notify.notified().await;

                loop {
                    let debounce_secs = manager.settings.lock().await.sync_debounce_seconds.max(1);
                    tokio::time::sleep(Duration::from_secs(debounce_secs)).await;

                    while tokio::time::timeout(
                        Duration::from_millis(100),
                        manager.auto_push_notify.notified(),
                    )
                    .await
                    .is_ok()
                    {}

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

                    let pending_more = tokio::time::timeout(
                        Duration::from_millis(100),
                        manager.auto_push_notify.notified(),
                    )
                    .await
                    .is_ok();
                    if !pending_more {
                        break;
                    }
                }
            }
        });
    }

    fn ensure_backup_worker(self: &Arc<Self>) {
        if self.backup_worker_started.swap(true, Ordering::SeqCst) {
            return;
        }

        let manager = Arc::clone(self);
        async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(BACKUP_CHECK_INTERVAL).await;

                let settings = manager.settings.lock().await.clone();
                if !settings.enabled || !settings.scheduled_backup_enabled {
                    continue;
                }

                let state = manager.state.lock().await.clone();
                let now = current_time_ms();
                let due = state.last_backup_at_ms.map_or(true, |last| {
                    now.saturating_sub(last)
                        >= settings.backup_interval_hours.max(1) * 60 * 60 * 1000
                });
                if !due {
                    continue;
                }
                if !matches!(
                    manager.automatic_retry_gate().await,
                    AutomaticRetryGate::Run
                ) {
                    continue;
                }

                if let Err(error) = manager.run_cloud_backup_now("scheduled_backup").await {
                    tracing::warn!("Scheduled backup failed: {}", error);
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

        self.set_status(
            "running",
            "Uploading cloud sync snapshot".to_string(),
            Some("sync_push".to_string()),
            None,
        )
        .await;

        let started = Instant::now();
        let state_snapshot = self.state.lock().await.clone();
        let operator = build_operator(&settings)?;
        ensure_remote_layout(&operator, &settings.remote_root).await?;

        let envelope = build_portable_snapshot(
            &self.app()?,
            PortableSnapshotKind::Sync,
            &state_snapshot.device_id,
        )?;
        let local_hash = envelope.payload_hash.clone();
        let latest = load_sync_pointer(&operator, &settings.remote_root).await?;

        if let Some(remote) = &latest {
            if remote.payload_hash == local_hash {
                let mut state = self.state.lock().await;
                state.last_synced_payload_hash = Some(local_hash);
                state.last_applied_remote_revision = Some(remote.revision_id.clone());
                state.last_checked_at_ms = Some(current_time_ms());
                config::save_cloud_sync_state(&self.app()?, &state)?;
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
                    local_payload_hash: local_hash.clone(),
                    remote_payload_hash: remote.payload_hash.clone(),
                    remote_revision: remote.revision_id.clone(),
                    remote_created_at_ms: remote.created_at_ms,
                    remote_device_id: remote.device_id.clone(),
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

        let encoded = encode_portable_snapshot(&envelope)?;
        let encrypted = encrypt_snapshot_bytes(&encoded)?;
        let snapshot_path = remote_path(
            &settings.remote_root,
            &format!("{SYNC_SNAPSHOTS_DIR}{}.redb.enc", envelope.revision_id),
        );
        operator
            .write(&snapshot_path, encrypted)
            .await
            .map_err(map_storage_error)?;

        let pointer = RemoteSyncPointer {
            revision_id: envelope.revision_id.clone(),
            created_at_ms: envelope.created_at_ms,
            payload_hash: envelope.payload_hash.clone(),
            device_id: envelope.device_id.clone(),
            app_version: envelope.app_version.clone(),
        };
        write_sync_pointer(&operator, &settings.remote_root, &pointer).await?;

        {
            let mut state = self.state.lock().await;
            state.last_synced_payload_hash = Some(envelope.payload_hash.clone());
            state.last_applied_remote_revision = Some(envelope.revision_id.clone());
            state.last_synced_at_ms = Some(current_time_ms());
            state.last_checked_at_ms = Some(current_time_ms());
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
        let operator = build_operator(&settings)?;
        let latest = load_sync_pointer(&operator, &settings.remote_root)
            .await?
            .ok_or_else(|| AppError::Config("No remote sync snapshot found".to_string()))?;

        let state_snapshot = self.state.lock().await.clone();
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

        if latest.payload_hash == local_envelope.payload_hash {
            let mut state = self.state.lock().await;
            state.last_synced_payload_hash = Some(latest.payload_hash.clone());
            state.last_applied_remote_revision = Some(latest.revision_id.clone());
            state.last_checked_at_ms = Some(current_time_ms());
            config::save_cloud_sync_state(&self.app()?, &state)?;
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
                local_payload_hash: local_envelope.payload_hash.clone(),
                remote_payload_hash: latest.payload_hash.clone(),
                remote_revision: latest.revision_id.clone(),
                remote_created_at_ms: latest.created_at_ms,
                remote_device_id: latest.device_id.clone(),
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

        let snapshot_path = remote_path(
            &settings.remote_root,
            &format!("{SYNC_SNAPSHOTS_DIR}{}.redb.enc", latest.revision_id),
        );
        let raw = operator
            .read(&snapshot_path)
            .await
            .map_err(map_storage_error)?;
        let decrypted = decrypt_snapshot_bytes(raw.to_vec().as_slice())?;
        let envelope = decode_portable_snapshot(&decrypted)?;
        apply_portable_snapshot(&self.app()?, &envelope).await?;

        {
            let mut state = self.state.lock().await;
            state.last_synced_payload_hash = Some(envelope.payload_hash.clone());
            state.last_applied_remote_revision = Some(envelope.revision_id.clone());
            state.last_synced_at_ms = Some(current_time_ms());
            state.last_checked_at_ms = Some(current_time_ms());
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

    async fn backup_snapshot(self: &Arc<Self>, trigger: &str) -> AppResult<()> {
        let _guard = self.operation_lock.lock().await;
        let _ = require_master_password()?;
        let settings = self.settings.lock().await.clone();
        if !settings.enabled {
            return Err(AppError::Config(
                "Cloud sync is disabled in settings".to_string(),
            ));
        }

        self.set_status(
            "running",
            "Uploading cloud backup".to_string(),
            Some("backup".to_string()),
            None,
        )
        .await;

        let started = Instant::now();
        let state_snapshot = self.state.lock().await.clone();
        let operator = build_operator(&settings)?;
        ensure_remote_layout(&operator, &settings.remote_root).await?;

        let envelope = build_portable_snapshot(
            &self.app()?,
            PortableSnapshotKind::Backup,
            &state_snapshot.device_id,
        )?;
        let encoded = encode_portable_snapshot(&envelope)?;
        let encrypted = encrypt_snapshot_bytes(&encoded)?;
        let snapshot_path = remote_path(
            &settings.remote_root,
            &format!("{BACKUPS_SNAPSHOTS_DIR}{}.redb.enc", envelope.revision_id),
        );
        operator
            .write(&snapshot_path, encrypted)
            .await
            .map_err(map_storage_error)?;

        let mut index = load_backup_index(&operator, &settings.remote_root).await?;
        index.entries.insert(
            0,
            RemoteBackupEntry {
                revision: envelope.revision_id.clone(),
                created_at_ms: envelope.created_at_ms,
                payload_hash: envelope.payload_hash.clone(),
                device_id: envelope.device_id.clone(),
                app_version: envelope.app_version.clone(),
                message: format!("Backup from {}", settings.device_name),
            },
        );

        let overflow: Vec<RemoteBackupEntry> = index
            .entries
            .iter()
            .skip(settings.backup_retention_count)
            .cloned()
            .collect();
        index.entries.truncate(settings.backup_retention_count);

        write_backup_index(&operator, &settings.remote_root, &index).await?;

        for old in overflow {
            let old_path = remote_path(
                &settings.remote_root,
                &format!("{BACKUPS_SNAPSHOTS_DIR}{}.redb.enc", old.revision),
            );
            let _ = operator.delete(&old_path).await;
        }

        {
            let mut state = self.state.lock().await;
            state.last_backup_revision = Some(envelope.revision_id.clone());
            state.last_backup_at_ms = Some(current_time_ms());
            config::save_cloud_sync_state(&self.app()?, &state)?;
        }

        self.append_history(CloudSyncHistoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp_ms: current_time_ms(),
            kind: "backup".to_string(),
            status: "success".to_string(),
            trigger: trigger.to_string(),
            provider: Some(settings.provider.clone()),
            revision: Some(envelope.revision_id.clone()),
            duration_ms: Some(elapsed_ms(started.elapsed())),
            message: "Cloud backup uploaded".to_string(),
        })
        .await;
        self.set_status("idle", "Cloud backup uploaded".to_string(), None, None)
            .await;
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
        self.set_status("failed", message, status.current_operation, status.conflict)
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

    async fn set_status(
        &self,
        state_value: &str,
        message: String,
        current_operation: Option<String>,
        conflict: Option<CloudConflictPreview>,
    ) {
        let Ok(app) = self.app() else {
            return;
        };
        let settings = self.settings.lock().await.clone();
        let state = self.state.lock().await.clone();
        let status = CloudSyncStatus {
            enabled: settings.enabled,
            provider: settings.provider.clone(),
            state: state_value.to_string(),
            message,
            current_operation,
            last_checked_at_ms: state.last_checked_at_ms,
            last_synced_at_ms: state.last_synced_at_ms,
            last_backup_at_ms: state.last_backup_at_ms,
            conflict: conflict.clone(),
        };
        *self.status.lock().await = status.clone();
        let _ = app.emit("cloud-sync-status-changed", &status);
        let _ = app.emit("cloud-sync-conflict", &conflict);
        crate::tray::schedule_refresh(&app);
    }

    fn app(&self) -> AppResult<tauri::AppHandle> {
        self.app_handle
            .get()
            .cloned()
            .ok_or_else(|| AppError::Config("cloud sync app handle is not initialized".to_string()))
    }
}

fn is_automatic_trigger(trigger: &str) -> bool {
    matches!(trigger, "auto_push" | "scheduled_backup" | "startup_check")
}

fn is_non_retryable_automatic_error(error: &AppError) -> bool {
    matches!(error, AppError::Auth(_) | AppError::Config(_))
}

pub async fn notify_config_changed(app: &tauri::AppHandle) {
    let manager = app.state::<Arc<CloudSyncManager>>();
    manager.inner().notify_config_changed().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CloudSyncSettings, S3SyncSettings, WebdavSyncSettings};

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
    }

    #[test]
    fn automatic_trigger_detection_is_limited_to_background_work() {
        assert!(is_automatic_trigger("auto_push"));
        assert!(is_automatic_trigger("scheduled_backup"));
        assert!(is_automatic_trigger("startup_check"));
        assert!(!is_automatic_trigger("manual_push"));
        assert!(!is_automatic_trigger("manual_test_connection"));
    }
}
