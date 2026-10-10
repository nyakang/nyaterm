//! Unified error type for Tauri commands.
//!
//! Provides `AppError` and `AppResult` for consistent error handling across the app.

use serde::Serialize;

/// Unified error type for all Tauri commands.
///
/// Implements `Serialize` so Tauri can pass the error message to the frontend.
/// Uses `thiserror` for ergonomic `From` conversions and `Display` formatting.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("SSH error: {0}")]
    Ssh(#[from] russh::Error),

    #[error("SSH key error: {0}")]
    SshKey(#[from] russh::keys::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    SessionNotFound(String),

    #[error("{0}")]
    SessionBusy(String),

    #[error("{0}")]
    Auth(String),

    #[error("{0}")]
    Config(String),

    #[cfg_attr(not(windows), allow(dead_code))]
    #[error("{0}")]
    Unsupported(String),

    #[error("{0}")]
    Storage(String),

    #[error("{0}")]
    Channel(String),

    #[error("{0}")]
    Cancelled(String),

    #[error("SFTP error: {0}")]
    Sftp(#[from] russh_sftp::client::error::Error),

    #[error("Crypto error: {0}")]
    Crypto(String),

    #[error("{0}")]
    CloudSync(#[from] CloudSyncError),

    #[error("Translation error: {0}")]
    Translation(String),
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum CloudSyncError {
    #[error(
        "Remote sync metadata is inconsistent: latest points to {revision} but the referenced snapshot is missing."
    )]
    SnapshotMissing { revision: String },

    #[error(
        "New sync snapshot was not accepted by remote storage (candidate revision {revision}; latest was not changed)."
    )]
    SnapshotNotAccepted { revision: String },

    #[error("Remote sync storage rejected file '{filename}' (gist may be at file capacity).")]
    RemoteFileRejected { filename: String },

    #[error(
        "Remote sync snapshot revision mismatch: latest points to {pointer_revision} but snapshot contains {snapshot_revision}."
    )]
    RevisionMismatch {
        pointer_revision: String,
        snapshot_revision: String,
    },

    #[error("Remote sync snapshot hash mismatch: expected {expected} but got {actual}.")]
    HashMismatch { expected: String, actual: String },

    #[error(
        "Remote sync was updated by another device: expected {expected_revision:?} but found {actual_revision:?}."
    )]
    ConcurrentUpdate {
        expected_revision: Option<String>,
        actual_revision: Option<String>,
    },

    #[error("Remote sync snapshot {revision} is corrupted.")]
    CorruptedSnapshot { revision: String },
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

/// Convenience alias for `Result<T, AppError>`.
pub type AppResult<T> = Result<T, AppError>;
