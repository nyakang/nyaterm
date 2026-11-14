//! Schema-neutral data-directory selection contracts.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

/// Environment override used by tests and unattended deployments.
pub const DATA_DIR_ENV: &str = "NYATERM_DATA_DIR";
/// Bootstrap pointer kept in the fixed local directory.
pub const LOCATION_FILE_NAME: &str = "location.json";
/// Marker written inside a data directory to identify its owner flavor.
pub const DATA_MARKER_FILE_NAME: &str = "nyaterm-data.json";
/// Must stay in sync with the store database file name.
pub const DATABASE_FILE_NAME: &str = "nyaterm.redb";
pub const PLUGINS_DIR_NAME: &str = "plugins";
pub const RECORDINGS_DIR_NAME: &str = "recordings";
pub const CLOUD_SYNC_LOCAL_DIR_NAME: &str = "cloud-sync-local";
pub const INSTANCE_DIR_NAME: &str = ".instance";

pub const LOCATION_VERSION: u32 = 1;
pub const MARKER_VERSION: u32 = 1;
pub const MIGRATION_SUFFIX: &str = "nyaterm-migrating";

#[derive(Debug, Error)]
pub enum DataDirectoryError {
    #[error("data directory path must be absolute and normalized: {0}")]
    RelativePath(PathBuf),
    #[error("data directory is already active: {0}")]
    SameDirectory(PathBuf),
    #[error("data directory overlaps the active data directory: {0}")]
    OverlappingDirectories(PathBuf),
    #[error("data directory is not a directory: {0}")]
    NotADirectory(PathBuf),
    #[error("data directory is not writable: {path}: {source}")]
    NotWritable {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("data directory is not empty: {0}")]
    NotEmpty(PathBuf),
    #[error("data directory belongs to another NyaTerm installation: {0}")]
    ForeignData(PathBuf),
    #[error("data directory is in use by another NyaTerm instance: {0}")]
    InUse(PathBuf),
    #[error("data directories on network locations are not supported: {0}")]
    NetworkPath(PathBuf),
    #[error("failed to {action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid data directory location file {path}: {message}")]
    InvalidLocation { path: PathBuf, message: String },
    #[error("migrated copy does not match its source: {0}")]
    CopyMismatch(PathBuf),
    #[error("data directory requires recovery: {0}")]
    RecoveryRequired(PathBuf),
    #[error("invalid or unsupported NyaTerm database: {0}")]
    InvalidDatabase(PathBuf),
    #[error("unsupported migration entry: {0}")]
    UnsupportedEntry(PathBuf),
}

impl DataDirectoryError {
    /// Stable code used by the UI to select a localized message.
    pub fn code(&self) -> &'static str {
        match self {
            Self::RelativePath(_) => "relative",
            Self::SameDirectory(_) => "same",
            Self::OverlappingDirectories(_) => "nested",
            Self::NotADirectory(_) => "not_directory",
            Self::NotWritable { .. } => "not_writable",
            Self::NotEmpty(_) => "not_empty",
            Self::ForeignData(_) => "foreign_data",
            Self::InUse(_) => "in_use",
            Self::NetworkPath(_) => "network",
            Self::Io { .. } => "io",
            Self::InvalidLocation { .. } => "invalid_location",
            Self::CopyMismatch(_) => "copy_mismatch",
            Self::RecoveryRequired(_) => "recovery_required",
            Self::InvalidDatabase(_) => "invalid_database",
            Self::UnsupportedEntry(_) => "unsupported_entry",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwitchMode {
    /// Copy the current database and plugins into the target directory.
    Migrate,
    /// Use an existing NyaTerm data directory without overwriting anything.
    Adopt,
    /// Archive the target's data before publishing the current data there.
    ReplaceWithBackup,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingSwitch {
    pub target: String,
    /// Actual active root, which can be a validated startup fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub mode: SwitchMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migration_id: Option<uuid::Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_fingerprint: Option<String>,
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataLocation {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<PendingSwitch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

impl Default for DataLocation {
    fn default() -> Self {
        Self {
            version: LOCATION_VERSION,
            data_dir: None,
            pending: None,
            last_error: None,
            extra: Default::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataMarker {
    pub version: u32,
    pub flavor: String,
}

#[derive(Debug, Clone)]
pub struct PreparedSwitch {
    pub target: PathBuf,
    pub mode: SwitchMode,
    /// Third-party sync client detected on the target, for the warning text.
    pub sync_client: Option<&'static str>,
    /// Binds confirmation to the existing target's managed contents.
    pub target_fingerprint: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DataDirectoryRecovery {
    pub configured: PathBuf,
    pub default_dir: PathBuf,
    pub pending: Option<PendingSwitch>,
    pub can_select: bool,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct StartupDataDirectory {
    pub pending: bool,
    pub data_dir: PathBuf,
    pub custom: bool,
    /// Non-fatal startup problem such as an unavailable custom directory.
    pub notice: Option<String>,
}
