//! User-selectable application data directory.
//!
//! The authoritative choice lives in `location.json` under the fixed local
//! application directory so that a broken selection can always be rolled back
//! without depending on the selected directory itself.

use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::app_identity::AppFlavor;

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

const LOCATION_VERSION: u32 = 1;
const MARKER_VERSION: u32 = 1;
const MIGRATION_SUFFIX: &str = "nyaterm-migrating";

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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingSwitch {
    pub target: String,
    pub mode: SwitchMode,
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
}

impl Default for DataLocation {
    fn default() -> Self {
        Self {
            version: LOCATION_VERSION,
            data_dir: None,
            pending: None,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DataMarker {
    version: u32,
    flavor: String,
}

#[derive(Debug, Clone)]
pub struct PreparedSwitch {
    pub target: PathBuf,
    pub mode: SwitchMode,
    /// Third-party sync client detected on the target, for the warning text.
    pub sync_client: Option<&'static str>,
}

#[derive(Debug, Clone)]
pub struct StartupDataDirectory {
    pub data_dir: PathBuf,
    pub custom: bool,
    /// Non-fatal startup problem such as an unavailable custom directory.
    pub notice: Option<String>,
}

pub fn location_path(local_dir: &Path) -> PathBuf {
    local_dir.join(LOCATION_FILE_NAME)
}

pub fn instance_lock_dir(local_dir: &Path, data_dir: &Path) -> PathBuf {
    local_dir
        .join("runtime")
        .join(format!("instance-{}", short_path_hash(data_dir)))
}

pub fn read_location(local_dir: &Path) -> Result<Option<DataLocation>, DataDirectoryError> {
    let path = location_path(local_dir);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(DataDirectoryError::Io {
                action: "read",
                path,
                source,
            });
        }
    };
    let location: DataLocation =
        serde_json::from_slice(&bytes).map_err(|error| DataDirectoryError::InvalidLocation {
            path: path.clone(),
            message: error.to_string(),
        })?;
    if location.version != LOCATION_VERSION {
        return Err(DataDirectoryError::InvalidLocation {
            path,
            message: format!("unsupported version {}", location.version),
        });
    }
    Ok(Some(location))
}

pub fn write_location(local_dir: &Path, location: &DataLocation) -> Result<(), DataDirectoryError> {
    fs::create_dir_all(local_dir).map_err(|source| DataDirectoryError::Io {
        action: "create",
        path: local_dir.to_path_buf(),
        source,
    })?;
    let path = location_path(local_dir);
    let bytes = serde_json::to_vec_pretty(location).map_err(|error| {
        DataDirectoryError::InvalidLocation {
            path: path.clone(),
            message: error.to_string(),
        }
    })?;
    let temp = local_dir.join(format!(
        ".{LOCATION_FILE_NAME}.{}.tmp",
        uuid::Uuid::new_v4()
    ));
    let write_result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(source) = write_result {
        let _ = fs::remove_file(&temp);
        return Err(DataDirectoryError::Io {
            action: "write",
            path,
            source,
        });
    }
    replace_file(&temp, &path).map_err(|source| {
        let _ = fs::remove_file(&temp);
        DataDirectoryError::Io {
            action: "replace",
            path,
            source,
        }
    })
}

pub fn set_active_data_dir(
    local_dir: &Path,
    data_dir: Option<&Path>,
) -> Result<(), DataDirectoryError> {
    let mut location = read_location(local_dir)?.unwrap_or_default();
    location.data_dir = data_dir.map(|path| path.to_string_lossy().into_owned());
    location.pending = None;
    location.last_error = None;
    write_location(local_dir, &location)
}

pub fn queue_switch(
    local_dir: &Path,
    current_data_dir: &Path,
    target: &Path,
    flavor: AppFlavor,
) -> Result<PreparedSwitch, DataDirectoryError> {
    let prepared = prepare_switch(current_data_dir, local_dir, target, flavor)?;
    let mut location = read_location(local_dir)?.unwrap_or_default();
    location.pending = Some(PendingSwitch {
        target: path_for_storage(&prepared.target)
            .to_string_lossy()
            .into_owned(),
        mode: prepared.mode,
    });
    location.last_error = None;
    write_location(local_dir, &location)?;
    Ok(prepared)
}

pub fn clear_pending(local_dir: &Path) -> Result<(), DataDirectoryError> {
    let Some(mut location) = read_location(local_dir)? else {
        return Ok(());
    };
    if location.pending.is_none() {
        return Ok(());
    }
    location.pending = None;
    write_location(local_dir, &location)
}

pub fn resolve_startup_data_dir(
    local_dir: &Path,
    default_dir: &Path,
    flavor: AppFlavor,
) -> StartupDataDirectory {
    let mut notice = None;
    let mut location = match read_location(local_dir) {
        Ok(Some(location)) => location,
        Ok(None) => DataLocation::default(),
        Err(error) => {
            notice = Some(error.to_string());
            DataLocation::default()
        }
    };

    let fallback_dir = location
        .data_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| default_dir.to_path_buf());

    if let Some(pending) = location.pending.clone() {
        match apply_pending(local_dir, &fallback_dir, flavor, &pending) {
            Ok(()) => {
                location.data_dir = Some(pending.target.clone());
                location.pending = None;
                location.last_error = None;
            }
            Err(error) => {
                notice = Some(error.to_string());
                // Another live instance owns the source database. Keep the queued
                // switch for the next clean restart instead of discarding the user's
                // choice, and never copy a database that a running instance holds.
                if !matches!(error, DataDirectoryError::InUse(_)) {
                    location.pending = None;
                    location.last_error = Some(error.to_string());
                }
            }
        }
        if let Err(error) = write_location(local_dir, &location) {
            notice = Some(error.to_string());
        }
    }

    let configured = location
        .data_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| default_dir.to_path_buf());

    if configured != default_dir {
        match validate_usable_directory(&configured, flavor) {
            Ok(()) => StartupDataDirectory {
                data_dir: configured,
                custom: true,
                notice,
            },
            Err(error) => {
                notice = Some(error.to_string());
                StartupDataDirectory {
                    data_dir: default_dir.to_path_buf(),
                    custom: false,
                    notice,
                }
            }
        }
    } else {
        StartupDataDirectory {
            data_dir: configured,
            custom: false,
            notice,
        }
    }
}

/// Turns a directory picked by the user into NyaTerm's data directory.
///
/// Users pick a parent folder; the data lives in the flavor's `.nyaterm`
/// child. A picked folder that already is that directory, or that already
/// holds NyaTerm data files, is used as-is so it never nests twice.
pub fn resolve_picked_data_dir(picked: &Path, flavor: AppFlavor) -> PathBuf {
    let dir_name = flavor.config_directory_name();
    let already_named = picked
        .file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(dir_name));
    if already_named
        || picked.join(DATABASE_FILE_NAME).exists()
        || picked.join(DATA_MARKER_FILE_NAME).exists()
    {
        picked.to_path_buf()
    } else {
        picked.join(dir_name)
    }
}

pub fn prepare_switch(
    current_data_dir: &Path,
    local_dir: &Path,
    picked: &Path,
    flavor: AppFlavor,
) -> Result<PreparedSwitch, DataDirectoryError> {
    if !picked.is_absolute() || picked.components().any(|part| part.as_os_str() == "..") {
        return Err(DataDirectoryError::RelativePath(picked.to_path_buf()));
    }
    // Every check below runs against the effective data directory, not the
    // parent folder the user picked.
    let target = normalize_existing_prefix(&resolve_picked_data_dir(picked, flavor));
    let current = normalize_existing_prefix(current_data_dir);
    if target == current {
        return Err(DataDirectoryError::SameDirectory(target));
    }
    if target.starts_with(&current) || current.starts_with(&target) {
        return Err(DataDirectoryError::OverlappingDirectories(target));
    }
    if target.exists() && !target.is_dir() {
        return Err(DataDirectoryError::NotADirectory(target));
    }
    fs::create_dir_all(&target).map_err(|source| DataDirectoryError::Io {
        action: "create",
        path: target.clone(),
        source,
    })?;
    if is_network_location(&target) {
        return Err(DataDirectoryError::NetworkPath(target));
    }
    probe_writable(&target)?;
    if is_in_use(local_dir, &target) {
        return Err(DataDirectoryError::InUse(target));
    }

    let marker = read_marker(&target)?;
    let install_dir = normalize_existing_prefix(local_dir);
    let is_install_dir = target == install_dir;
    let mode = match marker {
        Some(marker) => {
            if marker.flavor != flavor.desktop_id() {
                return Err(DataDirectoryError::ForeignData(target));
            }
            SwitchMode::Adopt
        }
        // `nyaterm.redb` only ever comes from NyaTerm, and the installation
        // directory or a resolved `.nyaterm` child is ours by name, so existing
        // data there is adopted as-is instead of being overwritten or rejected.
        None if target.join(DATABASE_FILE_NAME).exists() => SwitchMode::Adopt,
        None => {
            if !directory_is_effectively_empty(&target, is_install_dir)? {
                return Err(DataDirectoryError::NotEmpty(target));
            }
            SwitchMode::Migrate
        }
    };

    Ok(PreparedSwitch {
        sync_client: detect_sync_client(&target),
        target,
        mode,
    })
}

pub fn validate_usable_directory(
    data_dir: &Path,
    flavor: AppFlavor,
) -> Result<(), DataDirectoryError> {
    if !data_dir.is_absolute() {
        return Err(DataDirectoryError::RelativePath(data_dir.to_path_buf()));
    }
    if data_dir.exists() && !data_dir.is_dir() {
        return Err(DataDirectoryError::NotADirectory(data_dir.to_path_buf()));
    }
    fs::create_dir_all(data_dir).map_err(|source| DataDirectoryError::Io {
        action: "create",
        path: data_dir.to_path_buf(),
        source,
    })?;
    if is_network_location(data_dir) {
        return Err(DataDirectoryError::NetworkPath(data_dir.to_path_buf()));
    }
    probe_writable(data_dir)?;
    if let Some(marker) = read_marker(data_dir)?
        && marker.flavor != flavor.desktop_id()
    {
        return Err(DataDirectoryError::ForeignData(data_dir.to_path_buf()));
    }
    Ok(())
}

/// Writes the marker only for a directory that does not have one yet.
pub fn ensure_data_marker(data_dir: &Path, flavor: AppFlavor) -> Result<(), DataDirectoryError> {
    fs::create_dir_all(data_dir).map_err(|source| DataDirectoryError::Io {
        action: "create",
        path: data_dir.to_path_buf(),
        source,
    })?;
    if read_marker(data_dir)?.is_some() {
        return Ok(());
    }
    let marker = DataMarker {
        version: MARKER_VERSION,
        flavor: flavor.desktop_id().to_string(),
    };
    let bytes = serde_json::to_vec_pretty(&marker).expect("data marker serializes");
    let path = data_dir.join(DATA_MARKER_FILE_NAME);
    let temp = data_dir.join(format!(
        ".{DATA_MARKER_FILE_NAME}.{}.tmp",
        uuid::Uuid::new_v4()
    ));
    fs::write(&temp, bytes).map_err(|source| DataDirectoryError::Io {
        action: "write",
        path: temp.clone(),
        source,
    })?;
    replace_file(&temp, &path).map_err(|source| DataDirectoryError::Io {
        action: "replace",
        path,
        source,
    })
}

pub fn detect_sync_client(path: &Path) -> Option<&'static str> {
    let mut current = Some(path);
    for _ in 0..3 {
        let dir = current?;
        if dir.join(".stfolder").exists() || dir.join(".stignore").exists() {
            return Some("Syncthing");
        }
        if dir.join(".dropbox").exists() || dir.join(".dropbox.attr").exists() {
            return Some("Dropbox");
        }
        if dir.join("@eaDir").exists() {
            return Some("Synology Drive");
        }
        if dir.join(".nextcloudsync.log").exists() {
            return Some("Nextcloud");
        }
        if dir.join(".owncloudsync.log").exists() {
            return Some("ownCloud");
        }
        if let Ok(contents) = fs::read_to_string(dir.join("desktop.ini"))
            && contents.to_ascii_lowercase().contains("onedrive")
        {
            return Some("OneDrive");
        }
        current = dir.parent();
    }
    None
}

/// Short stable hash used to keep per-data-directory instance locks separate.
pub fn short_path_hash(path: &Path) -> String {
    let normalized = normalize_existing_prefix(path);
    let mut hasher = Sha256::new();
    #[cfg(windows)]
    {
        hasher.update(normalized.to_string_lossy().to_lowercase().as_bytes());
    }
    #[cfg(not(windows))]
    {
        hasher.update(normalized.to_string_lossy().as_bytes());
    }
    hex::encode(&hasher.finalize()[..8])
}

fn apply_pending(
    local_dir: &Path,
    source_data_dir: &Path,
    flavor: AppFlavor,
    pending: &PendingSwitch,
) -> Result<(), DataDirectoryError> {
    // Copying a redb file that another instance keeps open yields a torn snapshot,
    // so a pending migration is deferred until that instance exits.
    if is_in_use(local_dir, source_data_dir) {
        return Err(DataDirectoryError::InUse(source_data_dir.to_path_buf()));
    }
    let target = PathBuf::from(&pending.target);
    let prepared = prepare_switch(source_data_dir, local_dir, &target, flavor)?;
    let resolved_mode = match prepared.mode {
        SwitchMode::Adopt => {
            if pending.mode != SwitchMode::Adopt {
                // The target gained data between queueing and restart; never
                // overwrite it.
                return Err(DataDirectoryError::ForeignData(target));
            }
            SwitchMode::Adopt
        }
        SwitchMode::Migrate => SwitchMode::Migrate,
    };

    if resolved_mode == SwitchMode::Migrate {
        migrate_durable_entries(source_data_dir, &target)?;
    }
    ensure_data_marker(&target, flavor)?;
    cleanup_staging(&target);
    Ok(())
}

fn migrate_durable_entries(source: &Path, target: &Path) -> Result<(), DataDirectoryError> {
    let database = source.join(DATABASE_FILE_NAME);
    if database.is_file() {
        copy_file_verified(&database, &target.join(DATABASE_FILE_NAME))?;
    }
    for name in [PLUGINS_DIR_NAME, RECORDINGS_DIR_NAME] {
        let source_dir = source.join(name);
        if source_dir.is_dir() {
            copy_directory_verified(&source_dir, &target.join(name))?;
        }
    }
    Ok(())
}

fn copy_file_verified(source: &Path, dest: &Path) -> Result<(), DataDirectoryError> {
    let staging = dest.with_file_name(format!(".{MIGRATION_SUFFIX}-{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<(), DataDirectoryError> {
        fs::copy(source, &staging).map_err(|source_error| DataDirectoryError::Io {
            action: "copy",
            path: source.to_path_buf(),
            source: source_error,
        })?;
        sync_file(&staging)?;
        if file_digest(source)? != file_digest(&staging)? {
            return Err(DataDirectoryError::CopyMismatch(source.to_path_buf()));
        }
        replace_file(&staging, dest).map_err(|source_error| DataDirectoryError::Io {
            action: "replace",
            path: dest.to_path_buf(),
            source: source_error,
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staging);
    }
    result
}

fn copy_directory_verified(source: &Path, dest: &Path) -> Result<(), DataDirectoryError> {
    if dest.exists() {
        return Err(DataDirectoryError::NotEmpty(dest.to_path_buf()));
    }
    let staging = dest.with_file_name(format!(".{MIGRATION_SUFFIX}-{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<(), DataDirectoryError> {
        copy_directory_tree(source, &staging)?;
        fs::rename(&staging, dest).map_err(|source_error| DataDirectoryError::Io {
            action: "rename",
            path: dest.to_path_buf(),
            source: source_error,
        })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn copy_directory_tree(source: &Path, dest: &Path) -> Result<(), DataDirectoryError> {
    fs::create_dir_all(dest).map_err(|source_error| DataDirectoryError::Io {
        action: "create",
        path: dest.to_path_buf(),
        source: source_error,
    })?;
    let entries = fs::read_dir(source).map_err(|source_error| DataDirectoryError::Io {
        action: "read",
        path: source.to_path_buf(),
        source: source_error,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source_error| DataDirectoryError::Io {
            action: "read",
            path: source.to_path_buf(),
            source: source_error,
        })?;
        let file_type = entry
            .file_type()
            .map_err(|source_error| DataDirectoryError::Io {
                action: "inspect",
                path: entry.path(),
                source: source_error,
            })?;
        // Symlinks and junctions can escape the tree or form cycles.
        if file_type.is_symlink() {
            continue;
        }
        let child_source = entry.path();
        let child_dest = dest.join(entry.file_name());
        if file_type.is_dir() {
            copy_directory_tree(&child_source, &child_dest)?;
        } else {
            fs::copy(&child_source, &child_dest).map_err(|source_error| {
                DataDirectoryError::Io {
                    action: "copy",
                    path: child_source.clone(),
                    source: source_error,
                }
            })?;
        }
    }
    Ok(())
}

fn cleanup_staging(target: &Path) {
    let Ok(entries) = fs::read_dir(target) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(&format!(".{MIGRATION_SUFFIX}-"))
        {
            let path = entry.path();
            if path.is_dir() {
                let _ = fs::remove_dir_all(path);
            } else {
                let _ = fs::remove_file(path);
            }
        }
    }
}

fn read_marker(data_dir: &Path) -> Result<Option<DataMarker>, DataDirectoryError> {
    let path = data_dir.join(DATA_MARKER_FILE_NAME);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(DataDirectoryError::Io {
                action: "read",
                path,
                source,
            });
        }
    };
    let marker: DataMarker =
        serde_json::from_slice(&bytes).map_err(|error| DataDirectoryError::InvalidLocation {
            path,
            message: error.to_string(),
        })?;
    Ok(Some(marker))
}

/// Entries the installation directory always owns, so switching back to it is
/// not blocked by logs, crash reports, instance locks or the location pointer.
const INSTALL_DIR_LOCAL_ENTRIES: &[&str] = &[
    "logs",
    "crashes",
    "cache",
    "runtime",
    "mcp",
    CLOUD_SYNC_LOCAL_DIR_NAME,
    INSTANCE_DIR_NAME,
    LOCATION_FILE_NAME,
    DATA_MARKER_FILE_NAME,
];

fn directory_is_effectively_empty(
    path: &Path,
    ignore_install_entries: bool,
) -> Result<bool, DataDirectoryError> {
    let entries = fs::read_dir(path).map_err(|source| DataDirectoryError::Io {
        action: "read",
        path: path.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| DataDirectoryError::Io {
            action: "read",
            path: path.to_path_buf(),
            source,
        })?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let ignored = matches!(
            name.as_ref(),
            "desktop.ini" | "Thumbs.db" | ".DS_Store" | ".localized"
        ) || name.starts_with(&format!(".{MIGRATION_SUFFIX}-"))
            || (ignore_install_entries && INSTALL_DIR_LOCAL_ENTRIES.contains(&name.as_ref()));
        if !ignored {
            return Ok(false);
        }
    }
    Ok(true)
}

fn is_in_use(local_dir: &Path, data_dir: &Path) -> bool {
    for instance_dir in [
        data_dir.join(INSTANCE_DIR_NAME),
        instance_lock_dir(local_dir, data_dir),
    ] {
        let lock_path = instance_dir.join("owner.lock");
        let Ok(file) = OpenOptions::new().read(true).write(true).open(&lock_path) else {
            continue;
        };
        match file.try_lock() {
            Ok(()) => {
                let _ = file.unlock();
            }
            Err(std::fs::TryLockError::WouldBlock) => return true,
            Err(std::fs::TryLockError::Error(_)) => {}
        }
    }
    false
}

fn probe_writable(path: &Path) -> Result<(), DataDirectoryError> {
    let probe = path.join(format!(".nyaterm-write-probe-{}", uuid::Uuid::new_v4()));
    let result = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&probe)
        .and_then(|mut file| file.write_all(b"nyaterm"));
    let _ = fs::remove_file(&probe);
    result.map_err(|source| DataDirectoryError::NotWritable {
        path: path.to_path_buf(),
        source,
    })
}

fn sync_file(path: &Path) -> Result<(), DataDirectoryError> {
    // FlushFileBuffers requires write access on Windows.
    let file = OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|source| DataDirectoryError::Io {
            action: "open",
            path: path.to_path_buf(),
            source,
        })?;
    file.sync_all().map_err(|source| DataDirectoryError::Io {
        action: "sync",
        path: path.to_path_buf(),
        source,
    })
}

fn file_digest(path: &Path) -> Result<[u8; 32], DataDirectoryError> {
    let mut file = File::open(path).map_err(|source| DataDirectoryError::Io {
        action: "open",
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|source| DataDirectoryError::Io {
                action: "read",
                path: path.to_path_buf(),
                source,
            })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().into())
}

fn normalize_existing_prefix(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    let mut existing = path;
    let mut remainder = Vec::new();
    while let Some(parent) = existing.parent() {
        if let Some(name) = existing.file_name() {
            remainder.push(name.to_os_string());
        }
        if let Ok(canonical) = fs::canonicalize(parent) {
            let mut result = canonical;
            for part in remainder.iter().rev() {
                result.push(part);
            }
            return result;
        }
        existing = parent;
    }
    path.to_path_buf()
}

/// Converts canonical verbatim paths back to the form users expect to see.
fn path_for_storage(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path.to_path_buf()
}

fn replace_file(source: &Path, dest: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        // std::fs::rename does not replace an existing file on Windows.
        match fs::rename(source, dest) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let _ = fs::remove_file(dest);
                fs::rename(source, dest)
            }
            Err(error) => Err(error),
        }
    }
    #[cfg(not(windows))]
    {
        fs::rename(source, dest)
    }
}

fn is_network_location(path: &Path) -> bool {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        // Canonicalized paths arrive as \\?\C:\... or \\?\UNC\server\share.
        if text.to_ascii_lowercase().starts_with(r"\\?\unc\") {
            return true;
        }
        let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
        if text.starts_with(r"\\") || text.starts_with("//") {
            return true;
        }
        let bytes = text.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' {
            let root = format!("{}:\\", &text[..1]);
            let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
            // SAFETY: `wide` is a NUL-terminated UTF-16 path buffer.
            let drive_type =
                unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(wide.as_ptr()) };
            // DRIVE_REMOTE from winbase.h; windows-sys exposes it under a
            // feature we do not otherwise need.
            const DRIVE_REMOTE: u32 = 4;
            return drive_type == DRIVE_REMOTE;
        }
        false
    }
    #[cfg(target_os = "linux")]
    {
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let Ok(mounts) = fs::read_to_string("/proc/mounts") else {
            return false;
        };
        let mut best: Option<(usize, bool)> = None;
        for line in mounts.lines() {
            let mut fields = line.split_whitespace();
            let Some(mount_point) = fields.next() else {
                continue;
            };
            let Some(file_system) = fields.next() else {
                continue;
            };
            let mount_point = mount_point.replace("\\040", " ");
            let mount_path = Path::new(&mount_point);
            if !canonical.starts_with(mount_path) {
                continue;
            }
            let length = mount_path.as_os_str().len();
            if best.is_none_or(|(current, _)| length >= current) {
                best = Some((length, is_remote_file_system(file_system)));
            }
        }
        best.is_some_and(|(_, remote)| remote)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = path;
        false
    }
}

#[cfg(target_os = "linux")]
fn is_remote_file_system(file_system: &str) -> bool {
    matches!(
        file_system,
        "nfs"
            | "nfs4"
            | "cifs"
            | "smb3"
            | "smbfs"
            | "afpfs"
            | "davfs"
            | "fuse.sshfs"
            | "fuse.rclone"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        DATABASE_FILE_NAME, DataDirectoryError, DataLocation, LOCATION_FILE_NAME, SwitchMode,
        detect_sync_client, instance_lock_dir, prepare_switch, queue_switch, read_location,
        resolve_startup_data_dir, set_active_data_dir, short_path_hash, write_location,
    };
    use crate::app_identity::AppFlavor;
    use crate::test_support::TestTempDir;
    use std::fs;

    fn flavor() -> AppFlavor {
        AppFlavor::Stable
    }

    fn other_flavor() -> AppFlavor {
        match flavor() {
            AppFlavor::Stable => AppFlavor::Preview,
            AppFlavor::Preview => AppFlavor::Stable,
        }
    }

    #[test]
    fn location_file_round_trips_and_rejects_unknown_versions() {
        let directory = TestTempDir::new("nyaterm-location");
        let local = directory.path().join(".nyaterm");
        assert!(read_location(&local).unwrap().is_none());
        let location = DataLocation {
            version: 1,
            data_dir: Some("D:/NyaTerm".to_string()),
            pending: None,
            last_error: None,
        };
        write_location(&local, &location).unwrap();
        assert_eq!(read_location(&local).unwrap(), Some(location));

        fs::write(
            local.join(LOCATION_FILE_NAME),
            br#"{"version":99,"data_dir":"D:/NyaTerm"}"#,
        )
        .unwrap();
        let error = read_location(&local).unwrap_err();
        assert_eq!(error.code(), "invalid_location");
        assert!(error.to_string().contains("99"));
    }

    #[test]
    fn startup_uses_the_pointer_and_falls_back_when_it_is_unavailable() {
        let directory = TestTempDir::new("nyaterm-startup-data");
        let local = directory.path().join(".nyaterm");
        let default_dir = local.join("default-data");
        let custom = directory.path().join("custom-data");
        fs::create_dir_all(&custom).unwrap();

        set_active_data_dir(&local, Some(&custom)).unwrap();
        let startup = resolve_startup_data_dir(&local, &default_dir, flavor());
        assert_eq!(startup.data_dir, custom);
        assert!(startup.custom);
        assert!(startup.notice.is_none());

        // A file in place of the target directory makes the location unusable.
        let blocked = directory.path().join("blocked");
        fs::write(&blocked, b"not a directory").unwrap();
        set_active_data_dir(&local, Some(&blocked)).unwrap();
        let startup = resolve_startup_data_dir(&local, &default_dir, flavor());
        assert_eq!(startup.data_dir, default_dir);
        assert!(!startup.custom);
        assert!(startup.notice.is_some());
    }

    #[test]
    fn a_picked_parent_folder_resolves_to_its_nyaterm_child() {
        let directory = TestTempDir::new("nyaterm-picked-parent");
        let parent = directory.path().join("Backups");
        fs::create_dir_all(&parent).unwrap();
        let child = parent.join(flavor().config_directory_name());
        assert_eq!(super::resolve_picked_data_dir(&parent, flavor()), child);

        // Already the data directory itself: it must not nest twice.
        fs::create_dir_all(&child).unwrap();
        assert_eq!(super::resolve_picked_data_dir(&child, flavor()), child);

        // A picked folder that already holds NyaTerm data is used directly.
        let direct = directory.path().join("MyNyaTerm");
        fs::create_dir_all(&direct).unwrap();
        fs::write(direct.join(DATABASE_FILE_NAME), b"db").unwrap();
        assert_eq!(super::resolve_picked_data_dir(&direct, flavor()), direct);
    }

    #[test]
    fn picking_a_parent_folder_creates_and_migrates_into_its_nyaterm_child() {
        let directory = TestTempDir::new("nyaterm-picked-parent-migrate");
        let local = directory.path().join(".nyaterm");
        fs::create_dir_all(&local).unwrap();
        fs::write(local.join(DATABASE_FILE_NAME), b"db").unwrap();
        let parent = directory.path().join("Sync");
        fs::create_dir_all(&parent).unwrap();

        let prepared = prepare_switch(&local, &local, &parent, flavor()).unwrap();
        assert_eq!(prepared.mode, SwitchMode::Migrate);
        assert_eq!(
            prepared.target,
            super::normalize_existing_prefix(&parent.join(flavor().config_directory_name()))
        );
        assert!(prepared.target.is_dir(), "the child is created eagerly");
    }

    #[test]
    fn selecting_the_parent_of_the_active_directory_reports_the_same_directory() {
        let directory = TestTempDir::new("nyaterm-picked-parent-same");
        let local = directory.path().join(".nyaterm");
        let parent = directory.path().join("data");
        let current = parent.join(flavor().config_directory_name());
        fs::create_dir_all(&current).unwrap();

        assert_eq!(
            prepare_switch(&current, &local, &parent, flavor())
                .unwrap_err()
                .code(),
            "same"
        );
    }

    #[test]
    fn switching_uses_the_nyaterm_child_and_rejects_only_unusable_targets() {
        let directory = TestTempDir::new("nyaterm-switch-validation");
        let local = directory.path().join(".nyaterm");
        let current = local.clone();
        fs::create_dir_all(&current).unwrap();

        // Unrelated files in the picked parent do not matter; data goes into
        // the `.nyaterm` child.
        let parent = directory.path().join("parent");
        fs::create_dir_all(&parent).unwrap();
        fs::write(parent.join("notes.txt"), b"keep").unwrap();
        let prepared = prepare_switch(&current, &local, &parent, flavor()).unwrap();
        assert_eq!(prepared.mode, SwitchMode::Migrate);

        // An existing data child with unrelated content is rejected.
        let child = parent.join(flavor().config_directory_name());
        fs::write(child.join("notes.txt"), b"keep").unwrap();
        assert_eq!(
            prepare_switch(&current, &local, &parent, flavor())
                .unwrap_err()
                .code(),
            "not_empty"
        );
        fs::remove_file(child.join("notes.txt")).unwrap();

        // An existing child holding a NyaTerm database is adopted untouched.
        fs::write(child.join(DATABASE_FILE_NAME), b"db").unwrap();
        assert_eq!(
            prepare_switch(&current, &local, &parent, flavor())
                .unwrap()
                .mode,
            SwitchMode::Adopt
        );

        // A marker owned by another flavor is still rejected as foreign.
        let foreign = directory.path().join("foreign");
        super::ensure_data_marker(&foreign, other_flavor()).unwrap();
        assert_eq!(
            prepare_switch(&current, &local, &foreign, flavor())
                .unwrap_err()
                .code(),
            "foreign_data"
        );

        assert_eq!(
            prepare_switch(&current, &local, &current, flavor())
                .unwrap_err()
                .code(),
            "same"
        );
        assert_eq!(
            prepare_switch(&current, &local, &current.join("nested"), flavor())
                .unwrap_err()
                .code(),
            "nested"
        );
        assert_eq!(
            prepare_switch(&current, &local, std::path::Path::new("relative"), flavor())
                .unwrap_err()
                .code(),
            "relative"
        );
    }

    #[test]
    fn an_owned_target_is_adopted_not_migrated() {
        let directory = TestTempDir::new("nyaterm-switch-adopt");
        let local = directory.path().join(".nyaterm");
        let current = local.clone();
        fs::create_dir_all(&current).unwrap();
        let owned = directory.path().join("owned");
        super::ensure_data_marker(&owned, flavor()).unwrap();

        let prepared = prepare_switch(&current, &local, &owned, flavor()).unwrap();
        assert_eq!(prepared.mode, SwitchMode::Adopt);
    }

    #[test]
    fn switching_back_to_the_install_directory_ignores_machine_local_state() {
        let directory = TestTempDir::new("nyaterm-switch-reset");
        let local = directory.path().join(".nyaterm");
        let custom = directory.path().join("custom");
        fs::create_dir_all(&custom).unwrap();
        fs::write(custom.join(DATABASE_FILE_NAME), b"current-db").unwrap();
        for name in [
            "logs",
            "crashes",
            "cache",
            "runtime",
            "mcp",
            "cloud-sync-local",
        ] {
            fs::create_dir_all(local.join(name)).unwrap();
        }
        fs::write(local.join(LOCATION_FILE_NAME), b"{}").unwrap();

        let prepared = prepare_switch(&custom, &local, &local, flavor()).unwrap();
        assert_eq!(prepared.mode, SwitchMode::Migrate);
        assert_eq!(prepared.target, super::normalize_existing_prefix(&local));
    }

    #[test]
    fn switching_back_to_the_install_directory_adopts_its_previous_database() {
        let directory = TestTempDir::new("nyaterm-switch-reset-own-db");
        let local = directory.path().join(".nyaterm");
        let custom = directory.path().join("custom");
        fs::create_dir_all(&local).unwrap();
        fs::write(local.join(DATABASE_FILE_NAME), b"pre-switch-db").unwrap();
        fs::create_dir_all(&custom).unwrap();
        fs::write(custom.join(DATABASE_FILE_NAME), b"current-db").unwrap();

        let prepared = prepare_switch(&custom, &local, &local, flavor()).unwrap();
        assert_eq!(prepared.mode, SwitchMode::Adopt);
        assert_eq!(
            fs::read(local.join(DATABASE_FILE_NAME)).unwrap(),
            b"pre-switch-db",
            "adopting must never touch the existing database"
        );
    }

    #[test]
    fn switching_back_to_the_install_directory_still_rejects_unknown_content() {
        let directory = TestTempDir::new("nyaterm-switch-reset-foreign");
        let local = directory.path().join(".nyaterm");
        let custom = directory.path().join("custom");
        fs::create_dir_all(&local).unwrap();
        fs::write(local.join("notes.txt"), b"keep").unwrap();
        fs::create_dir_all(&custom).unwrap();

        assert_eq!(
            prepare_switch(&custom, &local, &local, flavor())
                .unwrap_err()
                .code(),
            "not_empty"
        );
    }

    #[test]
    fn migration_copies_the_database_and_plugins_then_commits_the_pointer() {
        let directory = TestTempDir::new("nyaterm-switch-migrate");
        let local = directory.path().join(".nyaterm");
        fs::create_dir_all(&local).unwrap();
        fs::write(local.join(DATABASE_FILE_NAME), b"database-bytes").unwrap();
        fs::create_dir_all(local.join("plugins/demo")).unwrap();
        fs::write(local.join("plugins/demo/plugin.json"), b"{}").unwrap();

        let picked = directory.path().join("target");
        let target = super::resolve_picked_data_dir(&picked, flavor());
        let prepared = queue_switch(&local, &local, &picked, flavor()).unwrap();
        assert_eq!(prepared.mode, SwitchMode::Migrate);

        let startup = resolve_startup_data_dir(&local, &local.clone(), flavor());
        assert_eq!(startup.data_dir, target);
        assert!(startup.custom);
        assert!(startup.notice.is_none());
        assert_eq!(
            fs::read(target.join(DATABASE_FILE_NAME)).unwrap(),
            b"database-bytes"
        );
        assert_eq!(
            fs::read(target.join("plugins/demo/plugin.json")).unwrap(),
            b"{}"
        );
        // The source directory is never modified.
        assert_eq!(
            fs::read(local.join(DATABASE_FILE_NAME)).unwrap(),
            b"database-bytes"
        );
        let stored = read_location(&local).unwrap().unwrap();
        assert!(stored.pending.is_none());
        assert_eq!(stored.data_dir.as_deref(), target.to_str());
    }

    #[test]
    fn a_failed_migration_keeps_the_previous_directory_and_reports_it() {
        let directory = TestTempDir::new("nyaterm-switch-rollback");
        let local = directory.path().join(".nyaterm");
        fs::create_dir_all(&local).unwrap();
        fs::write(local.join(DATABASE_FILE_NAME), b"database-bytes").unwrap();

        // The target disappears after queueing, simulating a removed drive.
        let target = directory.path().join("removable");
        fs::create_dir_all(&target).unwrap();
        queue_switch(&local, &local, &target, flavor()).unwrap();
        fs::remove_dir_all(&target).unwrap();
        fs::write(&target, b"now a file").unwrap();

        let startup = resolve_startup_data_dir(&local, &local.clone(), flavor());
        assert_eq!(startup.data_dir, local);
        assert!(!startup.custom);
        assert!(startup.notice.is_some());
        let stored = read_location(&local).unwrap().unwrap();
        assert!(stored.pending.is_none());
        assert!(stored.last_error.is_some());
        assert_eq!(
            fs::read(local.join(DATABASE_FILE_NAME)).unwrap(),
            b"database-bytes"
        );
    }

    #[test]
    fn a_pending_switch_waits_while_another_instance_owns_the_source() {
        let directory = TestTempDir::new("nyaterm-switch-in-use");
        let local = directory.path().join(".nyaterm");
        fs::create_dir_all(&local).unwrap();
        fs::write(local.join(DATABASE_FILE_NAME), b"database-bytes").unwrap();

        let target = directory.path().join("target");
        queue_switch(&local, &local, &target, flavor()).unwrap();

        // Stand in for a running instance that still holds the source database.
        let lock_dir = local.join(super::INSTANCE_DIR_NAME);
        fs::create_dir_all(&lock_dir).unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_dir.join("owner.lock"))
            .unwrap();
        lock.try_lock().unwrap();

        let startup = resolve_startup_data_dir(&local, &local.clone(), flavor());
        assert_eq!(startup.data_dir, local);
        assert!(!startup.custom);
        assert!(startup.notice.is_some());
        assert!(
            !super::resolve_picked_data_dir(&target, flavor())
                .join(DATABASE_FILE_NAME)
                .exists(),
            "a database held by another instance must not be copied"
        );
        let stored = read_location(&local).unwrap().unwrap();
        assert!(
            stored.pending.is_some(),
            "the queued switch survives for the next clean restart"
        );
        drop(lock);
    }

    #[test]
    fn instance_lock_hash_is_stable_and_case_insensitive_on_windows() {
        let directory = TestTempDir::new("nyaterm-instance-hash");
        let first = directory.path().join("Data");
        let second = directory.path().join("data");
        let first_hash = short_path_hash(&first);
        let second_hash = short_path_hash(&second);
        #[cfg(windows)]
        assert_eq!(first_hash, second_hash);
        #[cfg(not(windows))]
        assert_ne!(first_hash, second_hash);
        assert!(
            instance_lock_dir(directory.path(), &first)
                .to_string_lossy()
                .contains(&first_hash)
        );
    }

    #[test]
    fn sync_clients_are_detected_from_their_markers() {
        let directory = TestTempDir::new("nyaterm-sync-marker");
        let target = directory.path().join("OneDrive/NyaTerm");
        fs::create_dir_all(&target).unwrap();
        assert!(detect_sync_client(&target).is_none());
        fs::write(target.join("desktop.ini"), b"[OneDrive]").unwrap();
        assert_eq!(detect_sync_client(&target), Some("OneDrive"));
        fs::write(target.join(".stfolder"), b"").unwrap();
        assert_eq!(detect_sync_client(&target), Some("Syncthing"));

        // Sync markers usually sit in the parent folder the user picked.
        let child = target.join(flavor().config_directory_name());
        fs::create_dir_all(&child).unwrap();
        assert_eq!(detect_sync_client(&child), Some("Syncthing"));
    }

    #[test]
    fn error_codes_are_stable_for_the_ui() {
        assert_eq!(DataDirectoryError::SameDirectory("x".into()).code(), "same");
        assert_eq!(DataDirectoryError::InUse("x".into()).code(), "in_use");
    }
}
