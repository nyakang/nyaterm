use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{PortableSnapshotError, RawPortableSnapshot};

mod gc;
mod history;
mod protocol;
mod remote;
mod s3;
mod settings;
mod snippet;

pub use gc::{
    SYNC_SNAPSHOT_GC_GRACE_PERIOD, SYNC_SNAPSHOT_KEEP_RECENT, cleanup_sync_snapshots_with_remote,
};
pub use history::{
    CLOUD_SYNC_HISTORY_DOMAIN, CLOUD_SYNC_HISTORY_EVENT, CLOUD_SYNC_HISTORY_LIMIT,
    CloudSyncHistoryEntry, append_cloud_sync_history, read_cloud_sync_history,
};
pub use remote::{
    LocalDirectoryRemote, drive_remote_segments, google_drive_query_literal,
    legacy_sync_snapshot_file, remote_path,
};
pub use s3::{
    S3HttpMethod, S3SignedRequest, build_s3_signed_request, build_s3_signed_request_with_query,
    s3_payload_sha256,
};
pub use settings::{
    AliyunDriveSyncSettings, CloudSyncSettings, GiteeSnippetSyncSettings, GithubGistSyncSettings,
    LocalCloudSyncOptions, MASKED_SECRET_VALUE, OAuthDriveSyncSettings, S3SyncSettings,
    WebdavSyncSettings, mask_cloud_sync_settings, merge_masked_cloud_sync_settings,
};
pub use snippet::{
    GiteeSnippetHttpBackend, GithubGistHttpBackend, SNIPPET_REMOTE_FILE_PREFIX,
    SNIPPET_REMOTE_FILE_SUFFIX, SnippetBlobBackend, SnippetHttpClient, SnippetHttpDocument,
    SnippetHttpFile, SnippetHttpMethod, SnippetHttpRequest, SnippetHttpResponse, SnippetRemote,
    decode_snippet_blob, encode_snippet_blob, gitee_snippet_patch_body, github_gist_patch_body,
    github_gist_update_conflict_is_retryable, snippet_remote_filename, snippet_remote_path,
};

pub const SYNC_CURRENT_FILE: &str = "sync/current.redb.enc";
pub const SYNC_LATEST_FILE: &str = "sync/latest.redb";
pub const SYNC_SNAPSHOTS_DIR: &str = "sync/snapshots/";
pub const REMOTE_SYNC_POINTER_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Error)]
pub enum CloudSyncError {
    #[error("cloud sync is disabled")]
    Disabled,
    #[error("cloud sync conflict detected: {}", .0.message)]
    Conflict(Box<CloudConflictPreview>),
    #[error("remote snapshot is newer than local state; pull first")]
    RemoteNewer,
    #[error("no remote sync snapshot found")]
    NoRemoteSnapshot,
    #[error("no newer remote sync snapshot is available")]
    NoNewerRemoteSnapshot,
    #[error(
        "remote sync metadata is inconsistent: latest points to {revision} but the referenced snapshot is missing"
    )]
    SnapshotMissing { revision: String },
    #[error(
        "remote sync snapshot revision mismatch: latest points to {pointer_revision} but snapshot contains {snapshot_revision}"
    )]
    RevisionMismatch {
        pointer_revision: String,
        snapshot_revision: String,
    },
    #[error("remote sync snapshot hash mismatch: expected {expected} but got {actual}")]
    HashMismatch { expected: String, actual: String },
    #[error(
        "remote sync was updated by another device: expected {expected_revision:?} but found {actual_revision:?}"
    )]
    ConcurrentUpdate {
        expected_revision: Option<String>,
        actual_revision: Option<String>,
    },
    #[error("remote sync snapshot {revision} is corrupted")]
    CorruptedSnapshot { revision: String },
    #[error("invalid remote path '{path}'")]
    InvalidRemotePath { path: String },
    #[error("cloud sync remote error: {0}")]
    Remote(String),
    #[error("failed to create cloud sync directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to read cloud sync file {path}: {source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write cloud sync file {path}: {source}")]
    WriteFile {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("local cloud-sync store error: {0}")]
    LocalStore(String),
    #[error("portable snapshot error: {0}")]
    PortableSnapshot(#[from] PortableSnapshotError),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("base64 error: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteSyncPointer {
    #[serde(default = "default_remote_sync_pointer_schema_version")]
    pub schema_version: u32,
    pub revision_id: String,
    pub created_at_ms: u64,
    pub payload_hash: String,
    pub device_id: String,
    pub app_version: String,
}

fn default_remote_sync_pointer_schema_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloudSyncState {
    #[serde(default = "uuid_v4")]
    pub device_id: String,
    #[serde(default)]
    pub last_synced_payload_hash: Option<String>,
    #[serde(default)]
    pub last_applied_remote_revision: Option<String>,
    #[serde(default)]
    pub last_checked_at_ms: Option<u64>,
    #[serde(default)]
    pub last_synced_at_ms: Option<u64>,
    #[serde(default)]
    pub last_validated_remote_revision: Option<String>,
    #[serde(default)]
    pub last_full_validation_at_ms: Option<u64>,
    #[serde(default)]
    pub last_gc_attempt_at_ms: Option<u64>,
}

impl Default for CloudSyncState {
    fn default() -> Self {
        Self {
            device_id: uuid_v4(),
            last_synced_payload_hash: None,
            last_applied_remote_revision: None,
            last_checked_at_ms: None,
            last_synced_at_ms: None,
            last_validated_remote_revision: None,
            last_full_validation_at_ms: None,
            last_gc_attempt_at_ms: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CloudConflictKind {
    #[default]
    ContentConflict,
    RemoteInconsistent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloudConflictPreview {
    pub detected_at_ms: u64,
    pub provider: String,
    #[serde(default)]
    pub kind: CloudConflictKind,
    pub local_payload_hash: String,
    pub remote_payload_hash: String,
    pub remote_revision: String,
    pub remote_created_at_ms: u64,
    pub remote_device_id: String,
    #[serde(default)]
    pub recovery_revision: Option<String>,
    #[serde(default)]
    pub recovery_payload_hash: Option<String>,
    #[serde(default)]
    pub recovery_created_at_ms: Option<u64>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloudSyncStatus {
    pub enabled: bool,
    pub provider: String,
    pub state: String,
    pub message: String,
    pub current_operation: Option<String>,
    pub last_checked_at_ms: Option<u64>,
    pub last_synced_at_ms: Option<u64>,
    pub conflict: Option<CloudConflictPreview>,
}

impl Default for CloudSyncStatus {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "local_directory".to_string(),
            state: "idle".to_string(),
            message: String::new(),
            current_operation: None,
            last_checked_at_ms: None,
            last_synced_at_ms: None,
            conflict: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudRemoteCheckDecision {
    UpToDate,
    LocalChanged,
    AutoPull,
    RemoteAvailable,
    Conflict,
}

pub fn decide_cloud_remote_check(
    state: &CloudSyncState,
    local_hash: &str,
    remote: &RemoteSyncPointer,
    allow_auto_pull: bool,
) -> CloudRemoteCheckDecision {
    if remote.payload_hash == local_hash {
        return CloudRemoteCheckDecision::UpToDate;
    }

    let local_changed = state
        .last_synced_payload_hash
        .as_deref()
        .is_none_or(|hash| hash != local_hash);
    let remote_changed = state
        .last_applied_remote_revision
        .as_deref()
        .is_none_or(|revision| revision != remote.revision_id);

    match (remote_changed, local_changed, allow_auto_pull) {
        (true, true, _) => CloudRemoteCheckDecision::Conflict,
        (true, false, true) => CloudRemoteCheckDecision::AutoPull,
        (true, false, false) => CloudRemoteCheckDecision::RemoteAvailable,
        (false, true, _) => CloudRemoteCheckDecision::LocalChanged,
        (false, false, _) => CloudRemoteCheckDecision::UpToDate,
    }
}

fn required_secret(
    value: Option<&str>,
    message: &str,
) -> Result<crate::SecretString, CloudSyncError> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(crate::SecretString::from)
        .ok_or_else(|| CloudSyncError::Remote(message.to_string()))
}

#[derive(Debug, Clone)]
pub struct CloudSyncResult {
    pub state: CloudSyncState,
    pub status: CloudSyncStatus,
    pub pointer: Option<RemoteSyncPointer>,
    pub backup: Option<CloudSyncBackupInfo>,
    pub outcome: CloudSyncOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudSyncOutcome {
    UpToDate,
    Uploaded,
    Downloaded,
    Recovered,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudSyncBackupInfo {
    pub database_path: PathBuf,
    pub safety_backup_path: Option<PathBuf>,
}

pub trait CloudSyncRemote {
    fn provider(&self) -> &'static str;
    fn create_dir(&self, path: &str) -> Result<(), CloudSyncError>;
    fn read_if_exists(&self, path: &str) -> Result<Option<Vec<u8>>, CloudSyncError>;
    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError>;
    fn delete(&self, path: &str) -> Result<(), CloudSyncError>;
    fn list_files(&self, path: &str) -> Result<Vec<String>, CloudSyncError>;
}

pub trait CloudLocalStore: Send + Sync {
    fn encode_sync_snapshot(
        &self,
        snapshot: &RawPortableSnapshot,
        master_password: &str,
    ) -> Result<Vec<u8>, CloudSyncError>;

    fn decode_sync_snapshot(
        &self,
        bytes: &[u8],
        master_password: &str,
    ) -> Result<RawPortableSnapshot, CloudSyncError>;

    fn encode_sync_pointer(&self, pointer: &RemoteSyncPointer) -> Result<Vec<u8>, CloudSyncError>;

    fn decode_sync_pointer(&self, bytes: &[u8]) -> Result<RemoteSyncPointer, CloudSyncError>;

    fn build_sync_snapshot(
        &self,
        options: &LocalCloudSyncOptions,
    ) -> Result<RawPortableSnapshot, CloudSyncError>;

    fn apply_sync_snapshot(
        &self,
        options: &LocalCloudSyncOptions,
        snapshot: &RawPortableSnapshot,
    ) -> Result<CloudSyncBackupInfo, CloudSyncError>;

    fn persist_cloud_sync_state(&self, state: &CloudSyncState) -> Result<(), CloudSyncError>;
}

pub fn push_local_snapshot(
    local_store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
    state: &CloudSyncState,
    force: bool,
) -> Result<CloudSyncResult, CloudSyncError> {
    let remote = LocalDirectoryRemote::new(options.remote_dir.clone());
    push_snapshot_with_remote(local_store, options, &remote, state, force)
}

pub fn push_snapshot_with_remote(
    local_store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
    remote: &dyn CloudSyncRemote,
    state: &CloudSyncState,
    force: bool,
) -> Result<CloudSyncResult, CloudSyncError> {
    ensure_enabled(options)?;
    ensure_remote_layout(remote, &options.remote_root)?;
    let mut next_state = normalized_state(state, &options.device_id);
    let mut snapshot = local_store.build_sync_snapshot(options)?;
    snapshot.recalculate_hash()?;
    let local_hash = snapshot.meta.payload_hash.clone();
    let latest = load_sync_pointer_from_remote(local_store, remote, &options.remote_root)?;

    if let Some(remote_pointer) = &latest
        && remote_pointer.payload_hash == local_hash
    {
        match protocol::resolve_remote_snapshot(local_store, remote, options, remote_pointer)? {
            protocol::RemoteSnapshotResolution::Current(_)
            | protocol::RemoteSnapshotResolution::LegacyMigrated(_) => {}
            protocol::RemoteSnapshotResolution::Inconsistent {
                pointer,
                recovery_candidate,
            } => {
                return Err(CloudSyncError::Conflict(Box::new(
                    remote_inconsistent_preview(
                        remote.provider(),
                        &local_hash,
                        &pointer,
                        &recovery_candidate,
                    ),
                )));
            }
        }
        next_state.last_synced_payload_hash = Some(local_hash);
        next_state.last_applied_remote_revision = Some(remote_pointer.revision_id.clone());
        next_state.last_checked_at_ms = Some(current_time_ms());
        next_state.last_validated_remote_revision = Some(remote_pointer.revision_id.clone());
        next_state.last_full_validation_at_ms = Some(current_time_ms());
        let result = result(
            next_state,
            remote.provider(),
            "Cloud sync is already up to date",
            latest,
            None,
            CloudSyncOutcome::UpToDate,
        );
        local_store.persist_cloud_sync_state(&result.state)?;
        return Ok(result);
    }

    let remote_changed = latest.as_ref().is_some_and(|remote| {
        next_state
            .last_applied_remote_revision
            .as_deref()
            .is_none_or(|revision| revision != remote.revision_id)
    });
    let local_changed = next_state
        .last_synced_payload_hash
        .as_deref()
        .is_none_or(|hash| hash != local_hash);

    if remote_changed && !force {
        let remote_pointer = latest.expect("remote changed requires remote pointer");
        if local_changed {
            let conflict =
                conflict_preview(options, remote.provider(), &local_hash, &remote_pointer);
            return Err(CloudSyncError::Conflict(Box::new(conflict)));
        }
        return Err(CloudSyncError::RemoteNewer);
    }

    protocol::upload_sync_snapshot(local_store, remote, options, &snapshot)?;
    let pointer = protocol::pointer_from_snapshot(&snapshot);
    protocol::read_snapshot_for_pointer(local_store, remote, options, &pointer)?;
    next_state.last_validated_remote_revision = Some(pointer.revision_id.clone());
    next_state.last_full_validation_at_ms = Some(current_time_ms());
    if !force {
        protocol::ensure_remote_head_unchanged(
            local_store,
            remote,
            &options.remote_root,
            latest.as_ref(),
        )?;
    }
    write_sync_pointer(local_store, remote, &options.remote_root, &pointer)?;
    let _ = protocol::write_current_sync_snapshot_compat(local_store, remote, options, &snapshot);
    next_state.last_synced_payload_hash = Some(pointer.payload_hash.clone());
    next_state.last_applied_remote_revision = Some(pointer.revision_id.clone());
    next_state.last_synced_at_ms = Some(current_time_ms());
    next_state.last_checked_at_ms = Some(current_time_ms());
    let result = result(
        next_state,
        remote.provider(),
        "Cloud sync snapshot uploaded",
        Some(pointer),
        None,
        CloudSyncOutcome::Uploaded,
    );
    local_store.persist_cloud_sync_state(&result.state)?;
    Ok(result)
}

pub fn pull_local_snapshot(
    local_store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
    state: &CloudSyncState,
    force: bool,
) -> Result<CloudSyncResult, CloudSyncError> {
    let remote = LocalDirectoryRemote::new(options.remote_dir.clone());
    pull_snapshot_with_remote(local_store, options, &remote, state, force)
}

pub fn pull_snapshot_with_remote(
    local_store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
    remote: &dyn CloudSyncRemote,
    state: &CloudSyncState,
    force: bool,
) -> Result<CloudSyncResult, CloudSyncError> {
    ensure_enabled(options)?;
    ensure_remote_layout(remote, &options.remote_root)?;
    let latest = load_sync_pointer_from_remote(local_store, remote, &options.remote_root)?
        .ok_or(CloudSyncError::NoRemoteSnapshot)?;
    let mut next_state = normalized_state(state, &options.device_id);
    let mut local_snapshot = local_store.build_sync_snapshot(options)?;
    local_snapshot.recalculate_hash()?;
    let remote_snapshot =
        match protocol::resolve_remote_snapshot(local_store, remote, options, &latest)? {
            protocol::RemoteSnapshotResolution::Current(snapshot)
            | protocol::RemoteSnapshotResolution::LegacyMigrated(snapshot) => snapshot,
            protocol::RemoteSnapshotResolution::Inconsistent {
                pointer,
                recovery_candidate,
            } => {
                return Err(CloudSyncError::Conflict(Box::new(
                    remote_inconsistent_preview(
                        remote.provider(),
                        &local_snapshot.meta.payload_hash,
                        &pointer,
                        &recovery_candidate,
                    ),
                )));
            }
        };
    next_state.last_validated_remote_revision = Some(latest.revision_id.clone());
    next_state.last_full_validation_at_ms = Some(current_time_ms());

    if latest.payload_hash == local_snapshot.meta.payload_hash {
        next_state.last_synced_payload_hash = Some(latest.payload_hash.clone());
        next_state.last_applied_remote_revision = Some(latest.revision_id.clone());
        next_state.last_checked_at_ms = Some(current_time_ms());
        let result = result(
            next_state,
            remote.provider(),
            "Cloud sync is already up to date",
            Some(latest),
            None,
            CloudSyncOutcome::UpToDate,
        );
        local_store.persist_cloud_sync_state(&result.state)?;
        return Ok(result);
    }

    let local_changed = next_state
        .last_synced_payload_hash
        .as_deref()
        .is_none_or(|hash| hash != local_snapshot.meta.payload_hash);
    let remote_changed = next_state
        .last_applied_remote_revision
        .as_deref()
        .is_none_or(|revision| revision != latest.revision_id);

    if remote_changed && local_changed && !force {
        let conflict = conflict_preview(
            options,
            remote.provider(),
            &local_snapshot.meta.payload_hash,
            &latest,
        );
        return Err(CloudSyncError::Conflict(Box::new(conflict)));
    }
    if !remote_changed && !force {
        return Err(CloudSyncError::NoNewerRemoteSnapshot);
    }

    let snapshot = remote_snapshot;
    let backup = local_store.apply_sync_snapshot(options, &snapshot)?;
    let _ = protocol::write_current_sync_snapshot_compat(local_store, remote, options, &snapshot);
    next_state.last_synced_payload_hash = Some(snapshot.meta.payload_hash.clone());
    next_state.last_applied_remote_revision = Some(snapshot.meta.revision_id.clone());
    next_state.last_synced_at_ms = Some(current_time_ms());
    next_state.last_checked_at_ms = Some(current_time_ms());
    let result = result(
        next_state,
        remote.provider(),
        "Cloud sync snapshot downloaded",
        Some(latest),
        Some(backup),
        CloudSyncOutcome::Downloaded,
    );
    local_store.persist_cloud_sync_state(&result.state)?;
    Ok(result)
}

pub fn recover_local_current_snapshot(
    local_store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
) -> Result<CloudSyncResult, CloudSyncError> {
    let remote = LocalDirectoryRemote::new(options.remote_dir.clone());
    recover_current_snapshot_with_remote(local_store, options, &remote)
}

pub fn recover_current_snapshot_with_remote(
    local_store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
    remote: &dyn CloudSyncRemote,
) -> Result<CloudSyncResult, CloudSyncError> {
    ensure_enabled(options)?;
    ensure_remote_layout(remote, &options.remote_root)?;
    let snapshot = protocol::recover_current_remote_snapshot(local_store, remote, options)?;
    let backup = local_store.apply_sync_snapshot(options, &snapshot)?;
    let pointer = protocol::pointer_from_snapshot(&snapshot);
    let now = current_time_ms();
    let state = CloudSyncState {
        device_id: options.device_id.clone(),
        last_synced_payload_hash: Some(pointer.payload_hash.clone()),
        last_applied_remote_revision: Some(pointer.revision_id.clone()),
        last_checked_at_ms: Some(now),
        last_synced_at_ms: Some(now),
        last_validated_remote_revision: Some(pointer.revision_id.clone()),
        last_full_validation_at_ms: Some(now),
        last_gc_attempt_at_ms: None,
    };
    let result = result(
        state,
        remote.provider(),
        "Cloud sync metadata recovered",
        Some(pointer),
        Some(backup),
        CloudSyncOutcome::Recovered,
    );
    local_store.persist_cloud_sync_state(&result.state)?;
    Ok(result)
}

pub fn load_sync_pointer(
    local_store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
) -> Result<Option<RemoteSyncPointer>, CloudSyncError> {
    let remote = LocalDirectoryRemote::new(options.remote_dir.clone());
    load_sync_pointer_from_remote(local_store, &remote, &options.remote_root)
}

pub fn load_sync_pointer_from_remote(
    local_store: &dyn CloudLocalStore,
    remote: &dyn CloudSyncRemote,
    remote_root: &str,
) -> Result<Option<RemoteSyncPointer>, CloudSyncError> {
    let path = remote_path(remote_root, SYNC_LATEST_FILE);
    let Some(bytes) = remote.read_if_exists(&path)? else {
        return Ok(None);
    };
    local_store.decode_sync_pointer(bytes.as_slice()).map(Some)
}

fn ensure_enabled(options: &LocalCloudSyncOptions) -> Result<(), CloudSyncError> {
    if options.enabled {
        Ok(())
    } else {
        Err(CloudSyncError::Disabled)
    }
}

fn ensure_remote_layout(
    remote: &dyn CloudSyncRemote,
    remote_root: &str,
) -> Result<(), CloudSyncError> {
    for child in ["sync", SYNC_SNAPSHOTS_DIR] {
        remote.create_dir(&remote_path(remote_root, child))?;
    }
    Ok(())
}

fn write_sync_pointer(
    local_store: &dyn CloudLocalStore,
    remote: &dyn CloudSyncRemote,
    remote_root: &str,
    pointer: &RemoteSyncPointer,
) -> Result<(), CloudSyncError> {
    let bytes = local_store.encode_sync_pointer(pointer)?;
    remote.write(&remote_path(remote_root, SYNC_LATEST_FILE), &bytes)
}

fn conflict_preview(
    options: &LocalCloudSyncOptions,
    provider: &str,
    local_hash: &str,
    remote: &RemoteSyncPointer,
) -> CloudConflictPreview {
    CloudConflictPreview {
        detected_at_ms: current_time_ms(),
        provider: provider.to_string(),
        kind: CloudConflictKind::ContentConflict,
        local_payload_hash: local_hash.to_string(),
        remote_payload_hash: remote.payload_hash.clone(),
        remote_revision: remote.revision_id.clone(),
        remote_created_at_ms: remote.created_at_ms,
        remote_device_id: remote.device_id.clone(),
        recovery_revision: None,
        recovery_payload_hash: None,
        recovery_created_at_ms: None,
        message: format!(
            "Both local and cloud state changed since last sync ({})",
            options.remote_dir.display()
        ),
    }
}

fn remote_inconsistent_preview(
    provider: &str,
    local_hash: &str,
    pointer: &RemoteSyncPointer,
    recovery_candidate: &RawPortableSnapshot,
) -> CloudConflictPreview {
    CloudConflictPreview {
        detected_at_ms: current_time_ms(),
        provider: provider.to_string(),
        kind: CloudConflictKind::RemoteInconsistent,
        local_payload_hash: local_hash.to_string(),
        remote_payload_hash: pointer.payload_hash.clone(),
        remote_revision: pointer.revision_id.clone(),
        remote_created_at_ms: pointer.created_at_ms,
        remote_device_id: pointer.device_id.clone(),
        recovery_revision: Some(recovery_candidate.meta.revision_id.clone()),
        recovery_payload_hash: Some(recovery_candidate.meta.payload_hash.clone()),
        recovery_created_at_ms: Some(recovery_candidate.meta.created_at_ms),
        message: "Remote cloud sync metadata is incomplete. The latest pointer references a missing snapshot, but current.redb.enc contains a recoverable snapshot."
            .to_string(),
    }
}

fn result(
    state: CloudSyncState,
    provider: &str,
    message: &str,
    pointer: Option<RemoteSyncPointer>,
    backup: Option<CloudSyncBackupInfo>,
    outcome: CloudSyncOutcome,
) -> CloudSyncResult {
    CloudSyncResult {
        status: CloudSyncStatus {
            enabled: true,
            provider: provider.to_string(),
            state: "idle".to_string(),
            message: message.to_string(),
            current_operation: None,
            last_checked_at_ms: state.last_checked_at_ms,
            last_synced_at_ms: state.last_synced_at_ms,
            conflict: None,
        },
        state,
        pointer,
        backup,
        outcome,
    }
}

fn normalized_state(state: &CloudSyncState, device_id: &str) -> CloudSyncState {
    let mut state = state.clone();
    if state.device_id.trim().is_empty() {
        state.device_id = device_id.to_string();
    }
    state
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests;
