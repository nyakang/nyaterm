use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{PortableSnapshotError, RawPortableSnapshot};

pub mod cache;
pub mod conflict;
pub mod control;
pub mod credentials;
mod gc;
mod heads;
mod history;
pub mod io;
pub mod metadata_cache;
mod protocol;
mod remote;
mod s3;
mod settings;
mod snippet;

pub use gc::{
    SYNC_SNAPSHOT_GC_GRACE_PERIOD, SYNC_SNAPSHOT_KEEP_RECENT, cleanup_sync_snapshots_with_remote,
    cleanup_sync_snapshots_with_remote_controlled,
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
pub const REMOTE_SYNC_POINTER_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Error)]
pub enum CloudSyncError {
    #[error("{provider} {operation} failed (HTTP {status})")]
    Http {
        provider: String,
        operation: String,
        status: u16,
        retry_after_seconds: Option<u64>,
    },
    #[error("{provider} {operation} authentication failed (HTTP {status})")]
    Authentication {
        provider: String,
        operation: String,
        status: u16,
    },
    #[error("cloud credentials changed during token refresh; retry with the current account")]
    CredentialsChanged,
    #[error("refreshed cloud credentials could not be saved")]
    CredentialPersistence,
    #[error("cloud sync was cancelled before commit")]
    Cancelled,
    #[error(
        "gist_capacity: Gitee snippet has insufficient file slots; remove unrelated files or choose a new snippet, then retry sync"
    )]
    GistCapacity,

    #[error("cloud sync is disabled")]
    Disabled,
    #[error("automatic cloud pull was deferred because a session or settings editor is active")]
    AutoPullDeferred,
    #[error("local configuration changed during cloud sync; retry with the current configuration")]
    LocalChangedDuringSync,
    #[error(
        "cloud storage returned an older causal revision; retry after remote visibility catches up"
    )]
    StaleRemoteRevision,
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
    #[error("remote sync snapshot {revision} could not be decoded: {detail}")]
    CorruptedSnapshot { revision: String, detail: String },
    #[error(
        "cannot read remote sync snapshot {revision} at '{path}' ({bytes} bytes, sha256 {digest}): {cause}; decoder reported: {source}"
    )]
    RemoteSnapshotUnreadable {
        revision: String,
        path: String,
        bytes: usize,
        digest: String,
        cause: SnapshotUnreadableCause,
        /// Boxed to keep `CloudSyncError` small: an inlined decoder error would
        /// push this variant past the size at which every `Result` returning
        /// this error trips `clippy::result_large_err`.
        source: Box<PortableSnapshotError>,
    },
    #[error(
        "remote sync did not keep the snapshot just written to '{path}': read back {actual_bytes} bytes with sha256 {actual_digest} instead of {expected_bytes} bytes with sha256 {expected_digest} (revision {revision}); the remote accepted the upload but serves different data"
    )]
    RemoteWriteDiverged {
        revision: String,
        path: String,
        expected_bytes: usize,
        expected_digest: String,
        actual_bytes: usize,
        actual_digest: String,
    },
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

pub const GIST_CAPACITY_ERROR_CODE: &str = "gist_capacity";

/// Why a remote snapshot payload could not be decrypted.
///
/// An AEAD authentication failure cannot be told apart from the ciphertext
/// alone, so this is decided with local evidence: when the store can verify the
/// configured master password, the answer is authoritative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotUnreadableCause {
    /// The payload was produced by another key (another device's master
    /// password, a rotated password, or a foreign file) or was altered in
    /// transit, while the configured master password is known to be correct.
    PayloadNotAuthentic,
    /// The configured master password does not match this device's local state,
    /// so the cloud copy is most likely still valid and must not be overwritten.
    MasterPasswordMismatch,
    /// Not enough local evidence to choose between the two.
    Unknown,
}

impl SnapshotUnreadableCause {
    /// Whether overwriting the cloud copy must be refused for this cause.
    pub fn blocks_remote_overwrite(self) -> bool {
        matches!(self, Self::MasterPasswordMismatch)
    }
}

impl std::fmt::Display for SnapshotUnreadableCause {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::PayloadNotAuthentic => {
                "it was written with a different master password (for example by another device) \
                 or altered in transit, and this device verified that the configured master \
                 password is correct"
            }
            Self::MasterPasswordMismatch => {
                "the configured master password does not match this device's local state, so the \
                 cloud copy is most likely still valid; fix the master password instead of \
                 overwriting the cloud"
            }
            Self::Unknown => {
                "it was either written with a different master password or altered in transit"
            }
        })
    }
}

impl CloudSyncError {
    /// The remote snapshot path, byte count, digest and cause, when this error
    /// reports a payload that could not be decrypted.
    pub fn unreadable_remote_snapshot(
        &self,
    ) -> Option<(&str, usize, &str, SnapshotUnreadableCause)> {
        match self {
            Self::RemoteSnapshotUnreadable {
                path,
                bytes,
                digest,
                cause,
                ..
            } => Some((path.as_str(), *bytes, digest.as_str(), *cause)),
            _ => None,
        }
    }
}

impl CloudSyncError {
    pub fn classification(&self) -> &'static str {
        match self {
            Self::Authentication { .. } => "authentication",
            Self::Http {
                status: 403 | 429,
                retry_after_seconds: Some(_),
                ..
            } => "rate_limited",
            Self::Http {
                status: 401 | 403, ..
            } => "authentication",
            Self::Http { status: 429, .. } => "rate_limited",
            Self::Http {
                status: 500..=599, ..
            } => "server",
            Self::Http { .. } => "http",
            Self::Conflict(_) => "conflict",
            Self::CredentialsChanged => "credentials_changed",
            Self::CredentialPersistence => "credential_persistence",
            Self::Cancelled => "cancelled",
            Self::GistCapacity => "gist_capacity",
            Self::LocalChangedDuringSync => "local_changed",
            Self::StaleRemoteRevision => "stale_remote",
            Self::Io(_) => "io",
            Self::LocalStore(_) => "store",
            Self::PortableSnapshot(_) => "snapshot",
            _ => "sync",
        }
    }

    pub fn retry_after(&self) -> Option<std::time::Duration> {
        match self {
            Self::Http {
                retry_after_seconds: Some(seconds),
                ..
            } => Some(std::time::Duration::from_secs(*seconds)),
            _ => None,
        }
    }

    pub fn code(&self) -> Option<&'static str> {
        matches!(self, Self::GistCapacity).then_some(GIST_CAPACITY_ERROR_CODE)
    }
}

pub fn needs_gist_capacity_recovery(provider: &str, status: &str) -> bool {
    provider == "gitee_snippet"
        && [
            GIST_CAPACITY_ERROR_CODE,
            "was not accepted by remote storage",
            "rejected file",
            "gist may be at file capacity",
        ]
        .iter()
        .any(|message| status.contains(message))
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_clock: Option<crate::portable_snapshot::SnapshotSyncClock>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_synced_content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_synced_clock: Option<crate::portable_snapshot::SnapshotSyncClock>,
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
            last_synced_content_hash: None,
            last_synced_clock: None,
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
    #[serde(default)]
    pub branches: Vec<conflict::CloudConflictBranch>,
    #[serde(default)]
    pub observed_revisions: Vec<String>,
    #[serde(default)]
    pub local_content_hash: Option<String>,
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
    /// Set when pushing the local snapshot over the remote head must be refused,
    /// because doing so would overwrite a cloud copy that is still readable by
    /// the correct master password.
    #[serde(default)]
    pub overwrite_blocked: bool,
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

fn local_content_changed(state: &CloudSyncState, content_hash: &str) -> bool {
    // A legacy baseline cannot prove that entities omitted by its hash are
    // unchanged. Equal snapshots migrate automatically; differing snapshots
    // require a push or explicit conflict resolution instead of losing data.
    state.last_synced_content_hash.as_deref() != Some(content_hash)
}

fn remote_precedes_baseline(state: &CloudSyncState, pointer: &RemoteSyncPointer) -> bool {
    match (&state.last_synced_clock, &pointer.sync_clock) {
        (Some(baseline), Some(remote)) => baseline.strictly_dominates(remote),
        (Some(baseline), None) => baseline.legacy_revision.as_deref() == Some(&pointer.revision_id),
        _ => false,
    }
}

fn remote_concurrent_with_baseline(state: &CloudSyncState, pointer: &RemoteSyncPointer) -> bool {
    if state.last_applied_remote_revision.as_deref() == Some(&pointer.revision_id) {
        return false;
    }
    match (&state.last_synced_clock, &pointer.sync_clock) {
        (Some(baseline), Some(remote)) => {
            !baseline.strictly_dominates(remote) && !remote.strictly_dominates(baseline)
        }
        (Some(_), None) => !remote_precedes_baseline(state, pointer),
        _ => false,
    }
}

fn validate_remote_baseline(
    state: &CloudSyncState,
    pointer: &RemoteSyncPointer,
    options: &LocalCloudSyncOptions,
    provider: &str,
    local_hash: &str,
) -> Result<(), CloudSyncError> {
    if remote_precedes_baseline(state, pointer) {
        return Err(CloudSyncError::StaleRemoteRevision);
    }
    // A delayed listing can hide this device's published head. Its persisted
    // baseline still proves concurrency, even with no subsequent local edits.
    if remote_concurrent_with_baseline(state, pointer) {
        return Err(CloudSyncError::Conflict(Box::new(conflict_preview(
            options, provider, local_hash, pointer,
        ))));
    }
    Ok(())
}

fn decide_snapshot_check(
    state: &CloudSyncState,
    local: &RawPortableSnapshot,
    remote: &RawPortableSnapshot,
    pointer: &RemoteSyncPointer,
    allow_auto_pull: bool,
) -> Result<CloudRemoteCheckDecision, CloudSyncError> {
    let local_hash = local.sync_content_hash()?;
    if local_hash == remote.sync_content_hash()? {
        return Ok(CloudRemoteCheckDecision::UpToDate);
    }
    let local_changed = local_content_changed(state, &local_hash);
    let remote_changed =
        state.last_applied_remote_revision.as_deref() != Some(&pointer.revision_id);
    Ok(match (remote_changed, local_changed, allow_auto_pull) {
        (true, true, _) => CloudRemoteCheckDecision::Conflict,
        (true, false, true) => CloudRemoteCheckDecision::AutoPull,
        (true, false, false) => CloudRemoteCheckDecision::RemoteAvailable,
        (false, true, _) => CloudRemoteCheckDecision::LocalChanged,
        (false, false, _) => CloudRemoteCheckDecision::UpToDate,
    })
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteFileCapacity {
    pub used: usize,
    pub limit: usize,
}

pub trait CloudSyncRemote {
    /// Publish immutable V4 bytes. Implementations must expose a complete object
    /// or no object after an interrupted write; retries use the same bytes.
    fn write_sync_file(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        self.write(path, bytes)
    }
    /// Uncached listing for the V4 registry and immutable publication objects.
    fn list_sync_files(&self, path: &str) -> Result<Vec<String>, CloudSyncError> {
        self.list_files(path)
    }
    fn file_capacity(&self) -> Result<Option<RemoteFileCapacity>, CloudSyncError> {
        Ok(None)
    }

    fn read_conditional(
        &self,
        path: &str,
        _etag: Option<&str>,
    ) -> Result<io::CloudObjectRead, CloudSyncError> {
        Ok(match self.read_if_exists(path)? {
            Some(bytes) => io::CloudObjectRead::Modified { bytes, etag: None },
            None => io::CloudObjectRead::Missing,
        })
    }
    fn list_metadata(&self, path: &str) -> Result<Vec<io::CloudObjectMetadata>, CloudSyncError> {
        Ok(self
            .list_files(path)?
            .into_iter()
            .map(|path| io::CloudObjectMetadata {
                path,
                ..Default::default()
            })
            .collect())
    }
    fn observe(&self, _event: io::CloudSyncIoEvent) {}

    fn provider(&self) -> &'static str;
    fn create_dir(&self, path: &str) -> Result<(), CloudSyncError>;
    fn read_if_exists(&self, path: &str) -> Result<Option<Vec<u8>>, CloudSyncError>;
    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError>;
    fn delete(&self, path: &str) -> Result<(), CloudSyncError>;
    fn list_files(&self, path: &str) -> Result<Vec<String>, CloudSyncError>;
}

/// Read every causal legacy branch through the existing compatibility codec.
/// This is a migration reader; it never resolves a branch by overwriting data.
pub fn read_legacy_sync_branches(
    store: &dyn CloudLocalStore,
    remote: &dyn CloudSyncRemote,
    options: &LocalCloudSyncOptions,
) -> Result<Vec<RawPortableSnapshot>, CloudSyncError> {
    let heads = heads::read(store, remote, &options.remote_root)?;
    let mut pointers = heads.maximal();
    if pointers.is_empty()
        && let Some(pointer) = heads.selected
    {
        pointers.push(pointer);
    }
    let mut snapshots = Vec::new();
    for pointer in pointers {
        match protocol::resolve_remote_snapshot(store, remote, options, &pointer)? {
            protocol::RemoteSnapshotResolution::Current(snapshot)
            | protocol::RemoteSnapshotResolution::LegacyMigrated(snapshot) => {
                snapshots.push(snapshot)
            }
            _ => {
                return Err(CloudSyncError::Remote(
                    "Legacy cloud data is incomplete; migration was stopped".into(),
                ));
            }
        }
    }
    Ok(snapshots)
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

    /// Concurrent stores must override this as one serialized operation:
    /// compare the local content, apply, then persist the normalized baseline.
    /// The default is for stores already serialized by their caller.
    fn apply_sync_snapshot_checked(
        &self,
        options: &LocalCloudSyncOptions,
        snapshot: &RawPortableSnapshot,
        expected_content_hash: &str,
        mut state: CloudSyncState,
    ) -> Result<(CloudSyncBackupInfo, CloudSyncState), CloudSyncError> {
        if self.build_sync_snapshot(options)?.sync_content_hash()? != expected_content_hash {
            return Err(CloudSyncError::LocalChangedDuringSync);
        }
        let backup = self.apply_sync_snapshot(options, snapshot)?;
        let mut applied = self.build_sync_snapshot(options)?;
        applied.recalculate_hash()?;
        state.last_synced_content_hash = Some(applied.sync_content_hash()?);
        state.last_synced_payload_hash = Some(applied.meta.payload_hash);
        self.persist_cloud_sync_state(&state)?;
        Ok((backup, state))
    }

    fn persist_cloud_sync_state(&self, state: &CloudSyncState) -> Result<(), CloudSyncError>;

    /// Best-effort check that `master_password` is the password this device
    /// already unlocked its local state with.
    ///
    /// Used to tell "the remote payload was written with a different master
    /// password" apart from "the configured master password is wrong". Returning
    /// `Ok(None)` means this device cannot tell, and callers must not treat it as
    /// either answer.
    fn verify_master_password(
        &self,
        master_password: &str,
    ) -> Result<Option<bool>, CloudSyncError> {
        let _ = master_password;
        Ok(None)
    }
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
    push_snapshot_with_remote_controlled(
        local_store,
        options,
        remote,
        state,
        force,
        &control::CloudSyncOperationControl::default(),
    )
}

pub fn push_snapshot_with_remote_controlled(
    local_store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
    remote: &dyn CloudSyncRemote,
    state: &CloudSyncState,
    force: bool,
    control: &control::CloudSyncOperationControl,
) -> Result<CloudSyncResult, CloudSyncError> {
    conflict::with_details(
        push_snapshot_prepared(local_store, options, remote, state, force, control, None),
        &control.local_store(local_store),
        &control.remote(remote),
        options,
    )
}

fn push_snapshot_prepared(
    local_store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
    remote: &dyn CloudSyncRemote,
    state: &CloudSyncState,
    force: bool,
    control: &control::CloudSyncOperationControl,
    prepared: Option<PreparedSync>,
) -> Result<CloudSyncResult, CloudSyncError> {
    control.checkpoint()?;
    let remote = &control.remote(remote);
    let local_store = &control.local_store(local_store);
    ensure_enabled(options)?;
    ensure_remote_layout(remote, &options.remote_root)?;
    let mut next_state = normalized_state(state, &options.device_id);
    let PreparedSync {
        mut local,
        heads,
        remote: cached_remote,
    } = match prepared {
        Some(prepared) => prepared,
        None => PreparedSync {
            local: local_store.build_sync_snapshot(options)?,
            heads: heads::read(local_store, remote, &options.remote_root)?,
            remote: None,
        },
    };
    local.recalculate_hash()?;
    let mut snapshot = local;
    let local_hash = snapshot.meta.payload_hash.clone();
    let content_hash = snapshot.sync_content_hash()?;
    if !force {
        heads.require_single(options, remote.provider(), &local_hash)?;
    }
    let latest = heads.selected.clone();
    if !force && let Some(pointer) = &latest {
        validate_remote_baseline(state, pointer, options, remote.provider(), &local_hash)?;
    }
    let remote_snapshot = if cached_remote.is_some() {
        cached_remote
    } else if let Some(remote_pointer) = &latest {
        match protocol::resolve_remote_snapshot(local_store, remote, options, remote_pointer)? {
            protocol::RemoteSnapshotResolution::Current(snapshot)
            | protocol::RemoteSnapshotResolution::LegacyMigrated(snapshot) => Some(snapshot),
            protocol::RemoteSnapshotResolution::Inconsistent {
                pointer,
                recovery_candidate,
                unreadable,
            } => {
                let preview = remote_inconsistent_preview(
                    remote.provider(),
                    &local_hash,
                    &pointer,
                    recovery_candidate.as_ref(),
                    unreadable.as_ref(),
                );
                // A forced push is the documented way out of an unreadable
                // remote head, unless doing so would clobber a cloud copy that
                // the correct master password can still read.
                if force && !preview.overwrite_blocked {
                    None
                } else {
                    return Err(CloudSyncError::Conflict(Box::new(preview)));
                }
            }
        }
    } else {
        None
    };

    if !force
        && let Some(remote_pointer) = &latest
        && remote_snapshot
            .as_ref()
            .map(|snapshot| snapshot.sync_content_hash())
            .transpose()?
            .is_some_and(|hash| hash == content_hash)
    {
        next_state.last_synced_payload_hash = Some(local_hash);
        next_state.last_synced_content_hash = Some(content_hash);
        next_state.last_synced_clock = remote_pointer.sync_clock.clone();
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
    let local_changed = local_content_changed(&next_state, &content_hash);

    if latest.is_some() && !remote_changed && !local_changed && !force {
        next_state.last_checked_at_ms = Some(current_time_ms());
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

    if remote_changed && !force {
        let remote_pointer = latest.expect("remote changed requires remote pointer");
        if local_changed {
            let conflict =
                conflict_preview(options, remote.provider(), &local_hash, &remote_pointer);
            return Err(CloudSyncError::Conflict(Box::new(conflict)));
        }
        return Err(CloudSyncError::RemoteNewer);
    }

    snapshot.meta.device_name = options.device_name.clone();
    snapshot.meta.sync_clock =
        Some(heads.next_clock(state.last_synced_clock.as_ref(), &options.device_id)?);
    let uploaded =
        protocol::upload_sync_snapshot(local_store, remote, options, &snapshot, latest.as_ref())?;
    let pointer = protocol::pointer_from_snapshot(&snapshot);
    next_state.last_validated_remote_revision = Some(pointer.revision_id.clone());
    next_state.last_full_validation_at_ms = Some(current_time_ms());
    if !force {
        protocol::ensure_remote_head_unchanged(
            local_store,
            remote,
            &options.remote_root,
            heads.compatibility.as_ref(),
        )?;
    }
    control.begin_commit()?;
    heads::publish(remote, &options.remote_root, &pointer)?;
    // Device heads are authoritative. A shared compatibility pointer may race
    // or fail without erasing the independently committed branch.
    let _ = write_sync_pointer(local_store, remote, &options.remote_root, &pointer);
    let _ = remote.write(
        &remote_path(&options.remote_root, SYNC_CURRENT_FILE),
        &uploaded,
    );
    next_state.last_synced_payload_hash = Some(pointer.payload_hash.clone());
    next_state.last_synced_content_hash = Some(content_hash);
    next_state.last_synced_clock = pointer.sync_clock.clone();
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
    pull_snapshot_with_remote_guarded(local_store, options, remote, state, force, &|| Ok(()))
}

pub fn pull_snapshot_with_remote_guarded(
    local_store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
    remote: &dyn CloudSyncRemote,
    state: &CloudSyncState,
    force: bool,
    before_apply: &dyn Fn() -> Result<(), CloudSyncError>,
) -> Result<CloudSyncResult, CloudSyncError> {
    pull_snapshot_with_remote_controlled(
        local_store,
        options,
        remote,
        state,
        force,
        before_apply,
        &control::CloudSyncOperationControl::default(),
    )
}

pub fn pull_snapshot_with_remote_controlled(
    local_store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
    remote: &dyn CloudSyncRemote,
    state: &CloudSyncState,
    force: bool,
    before_apply: &dyn Fn() -> Result<(), CloudSyncError>,
    control: &control::CloudSyncOperationControl,
) -> Result<CloudSyncResult, CloudSyncError> {
    conflict::with_details(
        pull_snapshot_prepared(
            local_store,
            options,
            remote,
            state,
            force,
            PullExecution {
                before_apply,
                control,
            },
            None,
        ),
        &control.local_store(local_store),
        &control.remote(remote),
        options,
    )
}

struct PullExecution<'a> {
    before_apply: &'a dyn Fn() -> Result<(), CloudSyncError>,
    control: &'a control::CloudSyncOperationControl,
}

fn pull_snapshot_prepared(
    local_store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
    remote: &dyn CloudSyncRemote,
    state: &CloudSyncState,
    force: bool,
    execution: PullExecution<'_>,
    prepared: Option<PreparedSync>,
) -> Result<CloudSyncResult, CloudSyncError> {
    let PullExecution {
        before_apply,
        control,
    } = execution;
    control.checkpoint()?;
    let remote = &control.remote(remote);
    let local_store = &control.local_store(local_store);
    ensure_enabled(options)?;
    ensure_remote_layout(remote, &options.remote_root)?;
    let PreparedSync {
        local: mut local_snapshot,
        heads,
        remote: cached_remote,
    } = match prepared {
        Some(prepared) => prepared,
        None => PreparedSync {
            local: local_store.build_sync_snapshot(options)?,
            heads: heads::read(local_store, remote, &options.remote_root)?,
            remote: None,
        },
    };
    let mut latest = heads
        .selected
        .clone()
        .ok_or(CloudSyncError::NoRemoteSnapshot)?;
    if !force {
        validate_remote_baseline(
            state,
            &latest,
            options,
            remote.provider(),
            &local_snapshot.meta.payload_hash,
        )?;
    }
    let mut next_state = normalized_state(state, &options.device_id);
    local_snapshot.recalculate_hash()?;
    if !force {
        heads.require_single(
            options,
            remote.provider(),
            &local_snapshot.meta.payload_hash,
        )?;
    }
    let remote_snapshot = if let Some(snapshot) = cached_remote {
        snapshot
    } else {
        match protocol::resolve_remote_snapshot(local_store, remote, options, &latest)? {
            protocol::RemoteSnapshotResolution::Current(snapshot)
            | protocol::RemoteSnapshotResolution::LegacyMigrated(snapshot) => snapshot,
            protocol::RemoteSnapshotResolution::Inconsistent {
                pointer,
                recovery_candidate,
                unreadable,
            } => {
                return Err(CloudSyncError::Conflict(Box::new(
                    remote_inconsistent_preview(
                        remote.provider(),
                        &local_snapshot.meta.payload_hash,
                        &pointer,
                        recovery_candidate.as_ref(),
                        unreadable.as_ref(),
                    ),
                )));
            }
        }
    };
    next_state.last_validated_remote_revision = Some(latest.revision_id.clone());
    next_state.last_full_validation_at_ms = Some(current_time_ms());

    let content_hash = local_snapshot.sync_content_hash()?;
    let requires_resolution = heads.concurrent
        || remote_precedes_baseline(state, &latest)
        || remote_concurrent_with_baseline(state, &latest);
    if !requires_resolution && remote_snapshot.sync_content_hash()? == content_hash {
        next_state.last_synced_payload_hash = Some(local_snapshot.meta.payload_hash.clone());
        next_state.last_synced_content_hash = Some(content_hash);
        next_state.last_synced_clock = latest.sync_clock.clone();
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

    let local_changed = local_content_changed(&next_state, &content_hash);
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
        if local_changed {
            return Err(CloudSyncError::NoNewerRemoteSnapshot);
        }
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

    before_apply()?;
    let mut snapshot = remote_snapshot;
    if requires_resolution {
        snapshot.meta.revision_id = uuid_v4();
        snapshot.meta.created_at_ms = current_time_ms();
        snapshot.meta.device_id = options.device_id.clone();
        snapshot.meta.device_name = options.device_name.clone();
        snapshot.meta.sync_clock =
            Some(heads.next_clock(state.last_synced_clock.as_ref(), &options.device_id)?);
        snapshot.recalculate_hash()?;
        let uploaded =
            protocol::upload_sync_snapshot(local_store, remote, options, &snapshot, Some(&latest))?;
        latest = protocol::pointer_from_snapshot(&snapshot);
        control.begin_commit()?;
        heads::publish(remote, &options.remote_root, &latest)?;
        let _ = write_sync_pointer(local_store, remote, &options.remote_root, &latest);
        let _ = remote.write(
            &remote_path(&options.remote_root, SYNC_CURRENT_FILE),
            &uploaded,
        );
    }
    next_state.last_synced_clock = latest.sync_clock.clone();
    next_state.last_validated_remote_revision = Some(latest.revision_id.clone());
    next_state.last_applied_remote_revision = Some(snapshot.meta.revision_id.clone());
    next_state.last_synced_at_ms = Some(current_time_ms());
    next_state.last_checked_at_ms = Some(current_time_ms());
    let (backup, next_state) =
        local_store.apply_sync_snapshot_checked(options, &snapshot, &content_hash, next_state)?;
    let result = result(
        next_state,
        remote.provider(),
        "Cloud sync snapshot downloaded",
        Some(latest),
        Some(backup),
        CloudSyncOutcome::Downloaded,
    );
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
    recover_current_snapshot_with_remote_controlled(
        local_store,
        options,
        remote,
        &control::CloudSyncOperationControl::default(),
    )
}

pub fn recover_current_snapshot_with_remote_controlled(
    local_store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
    remote: &dyn CloudSyncRemote,
    control: &control::CloudSyncOperationControl,
) -> Result<CloudSyncResult, CloudSyncError> {
    control.checkpoint()?;
    let remote = &control.remote(remote);
    let local_store = &control.local_store(local_store);
    ensure_enabled(options)?;
    ensure_remote_layout(remote, &options.remote_root)?;
    let content_hash = local_store
        .build_sync_snapshot(options)?
        .sync_content_hash()?;
    let snapshot =
        protocol::recover_current_remote_snapshot(local_store, remote, options, control)?;
    let pointer = protocol::pointer_from_snapshot(&snapshot);
    let now = current_time_ms();
    let state = CloudSyncState {
        device_id: options.device_id.clone(),
        last_synced_content_hash: None,
        last_synced_clock: pointer.sync_clock.clone(),
        last_synced_payload_hash: None,
        last_applied_remote_revision: Some(pointer.revision_id.clone()),
        last_checked_at_ms: Some(now),
        last_synced_at_ms: Some(now),
        last_validated_remote_revision: Some(pointer.revision_id.clone()),
        last_full_validation_at_ms: Some(now),
        last_gc_attempt_at_ms: None,
    };
    let (backup, state) =
        local_store.apply_sync_snapshot_checked(options, &snapshot, &content_hash, state)?;
    let result = result(
        state,
        remote.provider(),
        "Cloud sync metadata recovered",
        Some(pointer),
        Some(backup),
        CloudSyncOutcome::Recovered,
    );
    Ok(result)
}

pub fn load_sync_pointer(
    local_store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
) -> Result<Option<RemoteSyncPointer>, CloudSyncError> {
    let remote = LocalDirectoryRemote::new(options.remote_dir.clone());
    load_sync_pointer_from_remote(local_store, &remote, &options.remote_root)
}

pub fn check_local_snapshot(
    local_store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
    state: &CloudSyncState,
    allow_auto_pull: bool,
) -> Result<(CloudRemoteCheckDecision, Option<RemoteSyncPointer>), CloudSyncError> {
    let remote = LocalDirectoryRemote::new(options.remote_dir.clone());
    check_snapshot_with_remote(local_store, options, &remote, state, allow_auto_pull)
}

pub fn check_snapshot_with_remote(
    local_store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
    remote: &dyn CloudSyncRemote,
    state: &CloudSyncState,
    allow_auto_pull: bool,
) -> Result<(CloudRemoteCheckDecision, Option<RemoteSyncPointer>), CloudSyncError> {
    let checked =
        check_snapshot_with_remote_state(local_store, options, remote, state, allow_auto_pull)?;
    Ok((checked.decision, checked.pointer))
}

pub struct CloudSyncCheck {
    pub decision: CloudRemoteCheckDecision,
    pub pointer: Option<RemoteSyncPointer>,
    /// Baseline of the exact local snapshot compared by this check. Never
    /// rebuild it after network I/O: intervening local edits are not synced.
    pub state: CloudSyncState,
    prepared: PreparedSync,
}

struct PreparedSync {
    local: RawPortableSnapshot,
    heads: heads::RemoteHeads,
    remote: Option<RawPortableSnapshot>,
}

impl CloudSyncCheck {
    pub fn push(
        self,
        store: &dyn CloudLocalStore,
        options: &LocalCloudSyncOptions,
        remote: &dyn CloudSyncRemote,
        control: &control::CloudSyncOperationControl,
    ) -> Result<CloudSyncResult, CloudSyncError> {
        conflict::with_details(
            push_snapshot_prepared(
                store,
                options,
                remote,
                &self.state,
                false,
                control,
                Some(self.prepared),
            ),
            &control.local_store(store),
            &control.remote(remote),
            options,
        )
    }

    pub fn pull(
        self,
        store: &dyn CloudLocalStore,
        options: &LocalCloudSyncOptions,
        remote: &dyn CloudSyncRemote,
        before_apply: &dyn Fn() -> Result<(), CloudSyncError>,
        control: &control::CloudSyncOperationControl,
    ) -> Result<CloudSyncResult, CloudSyncError> {
        conflict::with_details(
            pull_snapshot_prepared(
                store,
                options,
                remote,
                &self.state,
                false,
                PullExecution {
                    before_apply,
                    control,
                },
                Some(self.prepared),
            ),
            &control.local_store(store),
            &control.remote(remote),
            options,
        )
    }
}

pub fn check_snapshot_with_remote_state(
    local_store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
    remote: &dyn CloudSyncRemote,
    state: &CloudSyncState,
    allow_auto_pull: bool,
) -> Result<CloudSyncCheck, CloudSyncError> {
    conflict::with_details(
        check_snapshot_inner(local_store, options, remote, state, allow_auto_pull, None),
        local_store,
        remote,
        options,
    )
}

/// Cached inspection never authorizes a write or local application by itself.
pub fn check_snapshot_with_remote_cached(
    local_store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
    remote: &dyn CloudSyncRemote,
    state: &CloudSyncState,
    allow_auto_pull: bool,
    cache: &mut cache::CloudSyncCheckCache,
) -> Result<CloudSyncCheck, CloudSyncError> {
    cache.scope(options, remote.provider());
    let result = check_snapshot_inner(
        local_store,
        options,
        remote,
        state,
        allow_auto_pull,
        Some(cache),
    );
    conflict::with_cached_details(result, local_store, remote, options, cache)
}

fn check_snapshot_inner(
    local_store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
    remote: &dyn CloudSyncRemote,
    state: &CloudSyncState,
    allow_auto_pull: bool,
    mut cache: Option<&mut cache::CloudSyncCheckCache>,
) -> Result<CloudSyncCheck, CloudSyncError> {
    ensure_enabled(options)?;
    let mut local = local_store.build_sync_snapshot(options)?;
    local.recalculate_hash()?;
    let mut checked_state = state.clone();
    checked_state.last_checked_at_ms = Some(current_time_ms());
    let heads = heads::read(local_store, remote, &options.remote_root)?;
    if let Some(cache) = cache.as_deref_mut() {
        cache.prepare_preview(&heads.maximal());
    }
    heads.require_single(options, remote.provider(), &local.meta.payload_hash)?;
    let pointer = heads.selected.clone();
    let Some(pointer) = pointer else {
        return Ok(CloudSyncCheck {
            decision: CloudRemoteCheckDecision::LocalChanged,
            pointer: None,
            state: checked_state,
            prepared: PreparedSync {
                local,
                heads,
                remote: None,
            },
        });
    };
    validate_remote_baseline(
        state,
        &pointer,
        options,
        remote.provider(),
        &local.meta.payload_hash,
    )?;
    if let Some(receipt) = cache.as_deref_mut().and_then(|cache| cache.get(&pointer)) {
        remote.observe(io::CloudSyncIoEvent::CacheHit);
        let local_hash = local.sync_content_hash()?;
        let decision = if local_hash == receipt.content_hash {
            CloudRemoteCheckDecision::UpToDate
        } else {
            let local_changed = local_content_changed(state, &local_hash);
            let remote_changed =
                state.last_applied_remote_revision.as_deref() != Some(&pointer.revision_id);
            match (remote_changed, local_changed, allow_auto_pull) {
                (true, true, _) => CloudRemoteCheckDecision::Conflict,
                (true, false, true) => CloudRemoteCheckDecision::AutoPull,
                (true, false, false) => CloudRemoteCheckDecision::RemoteAvailable,
                (false, true, _) => CloudRemoteCheckDecision::LocalChanged,
                _ => CloudRemoteCheckDecision::UpToDate,
            }
        };
        if decision == CloudRemoteCheckDecision::Conflict {
            return Err(CloudSyncError::Conflict(Box::new(conflict_preview(
                options,
                remote.provider(),
                &local.meta.payload_hash,
                &pointer,
            ))));
        }
        if decision == CloudRemoteCheckDecision::UpToDate {
            checked_state.last_synced_content_hash = Some(local_hash);
            checked_state.last_synced_payload_hash = Some(local.meta.payload_hash.clone());
            checked_state.last_synced_clock = pointer.sync_clock.clone();
            checked_state.last_applied_remote_revision = Some(pointer.revision_id.clone());
        }
        checked_state.last_validated_remote_revision = Some(pointer.revision_id.clone());
        checked_state.last_full_validation_at_ms = Some(receipt.validated_at_ms);
        return Ok(CloudSyncCheck {
            decision,
            pointer: Some(pointer),
            state: checked_state,
            prepared: PreparedSync {
                local,
                heads,
                remote: None,
            },
        });
    }
    if let Some(cache) = cache.as_deref_mut() {
        cache.remove(&pointer);
    }
    let remote_snapshot =
        match protocol::resolve_remote_snapshot(local_store, remote, options, &pointer)? {
            protocol::RemoteSnapshotResolution::Current(snapshot)
            | protocol::RemoteSnapshotResolution::LegacyMigrated(snapshot) => snapshot,
            protocol::RemoteSnapshotResolution::Inconsistent {
                pointer,
                recovery_candidate,
                unreadable,
            } => {
                return Err(CloudSyncError::Conflict(Box::new(
                    remote_inconsistent_preview(
                        remote.provider(),
                        &local.meta.payload_hash,
                        &pointer,
                        recovery_candidate.as_ref(),
                        unreadable.as_ref(),
                    ),
                )));
            }
        };
    if let Some(cache) = cache {
        cache.record(&pointer, &remote_snapshot, &local)?;
    }
    let decision =
        decide_snapshot_check(state, &local, &remote_snapshot, &pointer, allow_auto_pull)?;
    if decision == CloudRemoteCheckDecision::Conflict {
        return Err(CloudSyncError::Conflict(Box::new(conflict_preview(
            options,
            remote.provider(),
            &local.meta.payload_hash,
            &pointer,
        ))));
    }
    if decision == CloudRemoteCheckDecision::UpToDate {
        checked_state.last_synced_content_hash = Some(local.sync_content_hash()?);
        checked_state.last_synced_clock = pointer.sync_clock.clone();
        checked_state.last_synced_payload_hash = Some(local.meta.payload_hash.clone());
        checked_state.last_applied_remote_revision = Some(pointer.revision_id.clone());
        checked_state.last_validated_remote_revision = Some(pointer.revision_id.clone());
        checked_state.last_full_validation_at_ms = checked_state.last_checked_at_ms;
    }
    Ok(CloudSyncCheck {
        decision,
        pointer: Some(pointer),
        state: checked_state,
        prepared: PreparedSync {
            local,
            heads,
            remote: Some(remote_snapshot),
        },
    })
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
    for child in ["sync", SYNC_SNAPSHOTS_DIR, heads::HEADS_DIR] {
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
    let path = remote_path(remote_root, SYNC_LATEST_FILE);
    // As with snapshot uploads, retry reads after an accepted PUT so repeated
    // writes do not restart eventual-consistency delays. File-slot stores keep
    // their write retry and capacity verdict.
    let capacity_limited = remote.file_capacity()?.is_some();
    let attempts = if capacity_limited {
        2
    } else {
        protocol::READBACK_VERIFY_ATTEMPTS
    };
    for attempt in 0..attempts {
        if capacity_limited || attempt == 0 {
            remote.write(&path, &bytes)?;
        }
        if load_sync_pointer_from_remote(local_store, remote, remote_root)?.as_ref()
            == Some(pointer)
        {
            return Ok(());
        }
        if attempt + 1 < attempts && !capacity_limited {
            protocol::readback_backoff(attempt);
        }
    }
    Err(if capacity_limited {
        CloudSyncError::GistCapacity
    } else {
        CloudSyncError::Remote(format!(
            "remote head write {path} could not be verified after {attempts} attempts"
        ))
    })
}

fn conflict_preview(
    options: &LocalCloudSyncOptions,
    provider: &str,
    local_hash: &str,
    remote: &RemoteSyncPointer,
) -> CloudConflictPreview {
    CloudConflictPreview {
        branches: Vec::new(),
        observed_revisions: Vec::new(),
        local_content_hash: None,
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
        overwrite_blocked: false,
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
    recovery_candidate: Option<&RawPortableSnapshot>,
    unreadable: Option<&protocol::RemoteSnapshotUnreadableHead>,
) -> CloudConflictPreview {
    let mut message = if recovery_candidate.is_some() {
        "Remote cloud sync metadata is inconsistent. A different current snapshot is available for explicit recovery."
    } else {
        "Remote cloud sync metadata is inconsistent and no valid current snapshot is available."
    }
    .to_string();
    if let Some(unreadable) = unreadable {
        message.push(' ');
        message.push_str(&unreadable.reason);
    }
    CloudConflictPreview {
        branches: Vec::new(),
        observed_revisions: Vec::new(),
        local_content_hash: None,
        detected_at_ms: current_time_ms(),
        provider: provider.to_string(),
        kind: CloudConflictKind::RemoteInconsistent,
        local_payload_hash: local_hash.to_string(),
        remote_payload_hash: pointer.payload_hash.clone(),
        remote_revision: pointer.revision_id.clone(),
        remote_created_at_ms: pointer.created_at_ms,
        remote_device_id: pointer.device_id.clone(),
        recovery_revision: recovery_candidate.map(|candidate| candidate.meta.revision_id.clone()),
        recovery_payload_hash: recovery_candidate
            .map(|candidate| candidate.meta.payload_hash.clone()),
        recovery_created_at_ms: recovery_candidate.map(|candidate| candidate.meta.created_at_ms),
        overwrite_blocked: unreadable.is_some_and(|unreadable| unreadable.overwrite_blocked),
        message,
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
