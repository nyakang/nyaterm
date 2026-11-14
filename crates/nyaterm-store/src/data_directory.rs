//! Filesystem transaction executor for data-directory selection.

mod backup;
mod locked_database;
use self::locked_database::LockedDatabase;

use nyaterm_core::app_identity::AppFlavor;
pub use nyaterm_core::data_directory::{
    CLOUD_SYNC_LOCAL_DIR_NAME, DATA_DIR_ENV, DATA_MARKER_FILE_NAME, DATABASE_FILE_NAME,
    DataDirectoryError, DataDirectoryRecovery, DataLocation, DataMarker, INSTANCE_DIR_NAME,
    LOCATION_FILE_NAME, LOCATION_VERSION, MARKER_VERSION, MIGRATION_SUFFIX, PLUGINS_DIR_NAME,
    PendingSwitch, PreparedSwitch, RECORDINGS_DIR_NAME, StartupDataDirectory, SwitchMode,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

pub fn location_path(local_dir: &Path) -> PathBuf {
    local_dir.join(LOCATION_FILE_NAME)
}

pub fn instance_lock_dir(_local_dir: &Path, data_dir: &Path) -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("nyaterm/runtime/data-roots")
        .join(short_path_hash(data_dir))
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
    fs::create_dir_all(local_dir).map_err(|e| io_error("create bootstrap", local_dir, e))?;
    failpoint("pointer", local_dir)?;
    write_atomic_json(&location_path(local_dir), location)
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
    queue_switch_checked(local_dir, current_data_dir, target, flavor, None)
}

pub fn queue_prepared_switch(
    local_dir: &Path,
    current_data_dir: &Path,
    choice: &PreparedSwitch,
    flavor: AppFlavor,
) -> Result<PreparedSwitch, DataDirectoryError> {
    queue_switch_checked(
        local_dir,
        current_data_dir,
        &choice.target,
        flavor,
        Some(choice),
    )
}

fn queue_switch_checked(
    local_dir: &Path,
    current_data_dir: &Path,
    target: &Path,
    flavor: AppFlavor,
    expected: Option<&PreparedSwitch>,
) -> Result<PreparedSwitch, DataDirectoryError> {
    let prepared = if expected.is_some_and(|choice| choice.mode == SwitchMode::ReplaceWithBackup) {
        prepare_migration(current_data_dir, local_dir, target, flavor)?
    } else {
        prepare_switch(current_data_dir, local_dir, target, flavor)?
    };
    if expected.is_some_and(|choice| {
        choice.mode != prepared.mode || choice.target_fingerprint != prepared.target_fingerprint
    }) {
        return Err(DataDirectoryError::RecoveryRequired(prepared.target));
    }
    let mut location = read_location(local_dir)?.unwrap_or_default();
    location.pending = Some(PendingSwitch {
        target: path_for_storage(&prepared.target)
            .to_string_lossy()
            .into_owned(),
        source: Some(
            path_for_storage(current_data_dir)
                .to_string_lossy()
                .into_owned(),
        ),
        mode: prepared.mode,
        migration_id: Some(uuid::Uuid::new_v4()),
        target_fingerprint: prepared.target_fingerprint.clone(),
        extra: Default::default(),
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

pub struct DataRootLease {
    _file: File,
}

impl DataRootLease {
    pub fn acquire(data_dir: &Path) -> Result<Self, DataDirectoryError> {
        let dir = instance_lock_dir(Path::new(""), data_dir);
        fs::create_dir_all(&dir).map_err(|e| io_error("create lock directory", &dir, e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
                .map_err(|e| io_error("restrict lock directory", &dir, e))?;
        }
        let path = dir.join("data.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| io_error("open lock", &path, e))?;
        lock_file(&file, data_dir)?;
        Ok(Self { _file: file })
    }
}

fn lock_file(file: &File, path: &Path) -> Result<(), DataDirectoryError> {
    file.try_lock().map_err(|e| match e {
        std::fs::TryLockError::WouldBlock => DataDirectoryError::InUse(path.into()),
        std::fs::TryLockError::Error(e) => io_error("lock", path, e),
    })
}

pub struct StartupStore {
    pub selection: StartupDataDirectory,
    pub store: crate::ConnectionStore,
    pub lease: DataRootLease,
}

/// Caller holds the installation's bootstrap owner through process exit.
pub fn open_startup(
    local: &Path,
    default_dir: &Path,
    flavor: AppFlavor,
) -> Result<StartupStore, DataDirectoryError> {
    let existing = read_location(local)?;
    let mut location = existing.clone().unwrap_or_default();
    if let Some(pending) = location.pending.as_mut()
        && pending.mode != SwitchMode::Adopt
        && pending.migration_id.is_none()
    {
        pending.migration_id = Some(uuid::Uuid::new_v4());
        write_location(local, &location)?;
    }
    let override_dir = std::env::var_os(DATA_DIR_ENV).map(PathBuf::from);
    let configured = override_dir
        .clone()
        .or_else(|| location.data_dir.as_ref().map(PathBuf::from))
        .unwrap_or_else(|| default_dir.into());
    if !configured.is_absolute() {
        return Err(DataDirectoryError::RelativePath(configured));
    }
    let lease = DataRootLease::acquire(&configured)?;
    let notice = location.last_error.clone();
    if override_dir.is_none()
        && let Some(pending) = location.pending.clone()
    {
        let source = pending
            .source
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| configured.clone());
        // Keep a fallback source lease through publication and the pointer commit.
        let mut _source_lease = None;
        let applied = (|| {
            if normalize_existing_prefix(&source) != normalize_existing_prefix(&configured) {
                _source_lease = Some(DataRootLease::acquire(&source)?);
            }
            apply_pending(local, &source, flavor, &pending)
        })();
        match applied {
            Ok(applied) => {
                let mut committed = location.clone();
                committed.data_dir = Some(pending.target.clone());
                committed.pending = None;
                committed.last_error = None;
                match write_location(local, &committed) {
                    Ok(()) => {
                        let target = PathBuf::from(&pending.target);
                        let _ = finish_migration(&target);
                        return Ok(StartupStore {
                            selection: StartupDataDirectory {
                                pending: false,
                                custom: target != default_dir,
                                data_dir: target,
                                notice: None,
                            },
                            store: applied.store,
                            lease: applied.lease,
                        });
                    }
                    Err(e) => {
                        let target = PathBuf::from(&pending.target);
                        let AppliedSwitch {
                            store,
                            lease: target_lease,
                            _source: source_guard,
                        } = applied;
                        drop(store);
                        location.last_error = Some(e.code().into());
                        let _ = write_location(local, &location);
                        if let Ok(bytes) = fs::read(target.join(JOURNAL_NAME))
                            && let Ok(mut manifest) = serde_json::from_slice::<Manifest>(&bytes)
                            && let Ok(digest) = file_digest(&target.join(DATABASE_FILE_NAME))
                        {
                            manifest
                                .files
                                .insert(DATABASE_FILE_NAME.into(), hex::encode(digest));
                            let _ = write_atomic_json(&target.join(JOURNAL_NAME), &manifest);
                        }
                        drop(target_lease);
                        drop(source_guard);
                        return Err(e);
                    }
                }
            }
            Err(e) => {
                location.last_error = Some(e.code().into());
                let _ = write_location(local, &location);
                // An unresolved transaction must not coexist with a writable source.
                return Err(e);
            }
        }
    }
    let committed_journal = location.pending.is_none()
        && location.data_dir.is_some()
        && fs::read(configured.join(JOURNAL_NAME))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Manifest>(&bytes).ok())
            .is_some_and(|manifest| {
                matches!(manifest.version, 1 | 2)
                    && manifest.ready
                    && manifest.target == normalize_existing_prefix(&configured)
            });
    if committed_journal {
        let _ = finish_migration(&configured);
    }
    let initial = existing.is_none()
        && configured == default_dir
        && !configured.join(DATABASE_FILE_NAME).exists()
        && !configured.join(DATA_MARKER_FILE_NAME).exists()
        && !configured.join(PLUGINS_DIR_NAME).exists()
        && !configured.join(RECORDINGS_DIR_NAME).exists();
    let opened = if initial {
        crate::ConnectionStore::open(&configured)
            .map_err(|_| DataDirectoryError::InvalidDatabase(configured.clone()))
    } else {
        validate_directory(&configured, flavor, committed_journal)
            .and_then(|()| open_existing(&configured))
    };
    match opened {
        Ok(store) => {
            ensure_data_marker(&configured, flavor)?;
            Ok(StartupStore {
                selection: StartupDataDirectory {
                    pending: location.pending.is_some(),
                    custom: configured != default_dir,
                    data_dir: configured,
                    notice,
                },
                store,
                lease,
            })
        }
        Err(e) => Err(e),
    }
}

fn open_existing(path: &Path) -> Result<crate::ConnectionStore, DataDirectoryError> {
    crate::ConnectionStore::open_existing(path).map_err(|e| match e {
        crate::StorageError::Open {
            source: redb::DatabaseError::DatabaseAlreadyOpen,
            ..
        } => DataDirectoryError::InUse(path.into()),
        _ => DataDirectoryError::InvalidDatabase(path.into()),
    })
}

fn io_error(action: &'static str, path: &Path, source: std::io::Error) -> DataDirectoryError {
    DataDirectoryError::Io {
        action,
        path: path.into(),
        source,
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
    if !picked.is_absolute()
        || picked.to_str().is_none()
        || picked.components().any(|part| part.as_os_str() == "..")
    {
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
    create_directories_durable(&target)?;
    if is_network_location(&target)? {
        return Err(DataDirectoryError::NetworkPath(target));
    }
    probe_writable(&target)?;
    let _lease = DataRootLease::acquire(&target)?;
    if target.join(JOURNAL_NAME).exists() {
        return Err(DataDirectoryError::RecoveryRequired(target));
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

    let target_fingerprint = if mode == SwitchMode::Adopt {
        validate_usable_directory(&target, flavor)?;
        Some(directory_fingerprint(&target)?)
    } else {
        None
    };
    Ok(PreparedSwitch {
        target_fingerprint,
        sync_client: detect_sync_client(&target),
        target,
        mode,
    })
}

/// Returning to the installation directory preserves today's data and archives
/// any historical data already there. Other occupied roots are adopted explicitly.
pub fn prepare_migration(
    current: &Path,
    local: &Path,
    target: &Path,
    flavor: AppFlavor,
) -> Result<PreparedSwitch, DataDirectoryError> {
    let mut choice = prepare_switch(current, local, target, flavor)?;
    if choice.mode == SwitchMode::Adopt {
        if choice.target != normalize_existing_prefix(local) {
            return Err(DataDirectoryError::NotEmpty(choice.target));
        }
        choice.mode = SwitchMode::ReplaceWithBackup;
    }
    Ok(choice)
}

pub fn startup_recovery(
    local: &Path,
    default_dir: &Path,
    error: &DataDirectoryError,
) -> DataDirectoryRecovery {
    let location = read_location(local);
    let override_dir = std::env::var_os(DATA_DIR_ENV).map(PathBuf::from);
    let can_select = override_dir.is_none() && location.is_ok();
    let location = location.ok().flatten().unwrap_or_default();
    DataDirectoryRecovery {
        configured: override_dir
            .or_else(|| location.data_dir.map(PathBuf::from))
            .unwrap_or_else(|| default_dir.to_path_buf()),
        default_dir: default_dir.to_path_buf(),
        pending: location.pending,
        can_select,
        code: error.code().into(),
        message: error.to_string(),
    }
}

/// Recovery selection never creates a database, nor needs the unavailable source.
pub fn prepare_recovery_selection(
    local: &Path,
    picked: &Path,
    flavor: AppFlavor,
) -> Result<PreparedSwitch, DataDirectoryError> {
    if std::env::var_os(DATA_DIR_ENV).is_some() {
        return Err(DataDirectoryError::RecoveryRequired(local.into()));
    }
    read_location(local)?;
    if !picked.is_absolute()
        || picked.to_str().is_none()
        || picked.components().any(|part| part.as_os_str() == "..")
    {
        return Err(DataDirectoryError::RelativePath(picked.into()));
    }
    let target = normalize_existing_prefix(&resolve_picked_data_dir(picked, flavor));
    let _lease = DataRootLease::acquire(&target)?;
    validate_usable_directory(&target, flavor)?;
    Ok(PreparedSwitch {
        target_fingerprint: Some(directory_fingerprint(&target)?),
        sync_client: detect_sync_client(&target),
        target,
        mode: SwitchMode::Adopt,
    })
}

/// Commit an explicit recovery choice before any ordinary store is opened.
pub fn commit_recovery_selection(
    local: &Path,
    choice: &PreparedSwitch,
    flavor: AppFlavor,
) -> Result<(), DataDirectoryError> {
    if std::env::var_os(DATA_DIR_ENV).is_some() || choice.mode != SwitchMode::Adopt {
        return Err(DataDirectoryError::RecoveryRequired(local.into()));
    }
    let _lease = DataRootLease::acquire(&choice.target)?;
    validate_usable_directory(&choice.target, flavor)?;
    check_target_fingerprint(&choice.target, choice.target_fingerprint.as_deref())?;
    set_active_data_dir(local, Some(&choice.target))
}

fn directory_fingerprint(path: &Path) -> Result<String, DataDirectoryError> {
    let file = LockedDatabase::open(&path.join(DATABASE_FILE_NAME))?;
    directory_fingerprint_locked(path, &file)
}

fn directory_fingerprint_locked(
    path: &Path,
    file: &LockedDatabase,
) -> Result<String, DataDirectoryError> {
    let mut inventory = source_tree_digests(path)?;
    inventory.insert(
        DATABASE_FILE_NAME.into(),
        source_fingerprint(file, &path.join(DATABASE_FILE_NAME))?,
    );
    if path.join(DATA_MARKER_FILE_NAME).exists() {
        inventory.insert(
            DATA_MARKER_FILE_NAME.into(),
            hex::encode(file_digest(&path.join(DATA_MARKER_FILE_NAME))?),
        );
    }
    let bytes = serde_json::to_vec(&inventory).expect("string map");
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn check_target_fingerprint(path: &Path, expected: Option<&str>) -> Result<(), DataDirectoryError> {
    let actual = directory_fingerprint(path)?;
    if expected.is_some_and(|expected| expected != actual) {
        return Err(DataDirectoryError::RecoveryRequired(path.into()));
    }
    Ok(())
}

pub fn validate_usable_directory(
    data_dir: &Path,
    flavor: AppFlavor,
) -> Result<(), DataDirectoryError> {
    validate_directory(data_dir, flavor, false)
}

fn validate_directory(
    data_dir: &Path,
    flavor: AppFlavor,
    committed: bool,
) -> Result<(), DataDirectoryError> {
    if !data_dir.is_absolute() {
        return Err(DataDirectoryError::RelativePath(data_dir.to_path_buf()));
    }
    if data_dir.exists() && !data_dir.is_dir() {
        return Err(DataDirectoryError::NotADirectory(data_dir.to_path_buf()));
    }
    if !data_dir.is_dir()
        || !fs::symlink_metadata(data_dir.join(DATABASE_FILE_NAME))
            .is_ok_and(|m| m.is_file() && m.len() > 0)
    {
        return Err(DataDirectoryError::RecoveryRequired(data_dir.into()));
    }
    if !committed && data_dir.join(JOURNAL_NAME).exists() {
        return Err(DataDirectoryError::RecoveryRequired(data_dir.into()));
    }
    if is_network_location(data_dir)? {
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
    if let Some(marker) = read_marker(data_dir)? {
        if marker.flavor != flavor.desktop_id() {
            return Err(DataDirectoryError::ForeignData(data_dir.into()));
        }
        return Ok(());
    }
    let marker = DataMarker {
        version: MARKER_VERSION,
        flavor: flavor.desktop_id().to_string(),
    };
    write_atomic_json(&data_dir.join(DATA_MARKER_FILE_NAME), &marker)
}

pub fn detect_sync_client(path: &Path) -> Option<&'static str> {
    let mut current = Some(path);
    for _ in 0..4 {
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
    hex::encode(hasher.finalize())
}

const JOURNAL_NAME: &str = ".nyaterm-migration.json";
const STAGING_NAME: &str = ".nyaterm-migration-stage";

#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    id: uuid::Uuid,
    source: PathBuf,
    source_hash: String,
    database_hash: String,
    #[serde(default)]
    ready: bool,
    target: PathBuf,
    files: std::collections::BTreeMap<String, String>,
    entries: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    backup: Option<backup::BackupManifest>,
    #[serde(flatten)]
    extra: std::collections::BTreeMap<String, serde_json::Value>,
}

struct AppliedSwitch {
    store: crate::ConnectionStore,
    lease: DataRootLease,
    _source: Option<LockedDatabase>,
}

fn apply_pending(
    local: &Path,
    source: &Path,
    flavor: AppFlavor,
    pending: &PendingSwitch,
) -> Result<AppliedSwitch, DataDirectoryError> {
    let target = normalize_existing_prefix(Path::new(&pending.target));
    let source_normalized = normalize_existing_prefix(source);
    if target == source_normalized
        || target.starts_with(&source_normalized)
        || source_normalized.starts_with(&target)
    {
        return Err(DataDirectoryError::OverlappingDirectories(target));
    }
    let lease = DataRootLease::acquire(&target)?;
    if pending.mode == SwitchMode::Adopt {
        validate_usable_directory(&target, flavor)?;
        check_target_fingerprint(&target, pending.target_fingerprint.as_deref())?;
        let store = open_existing(&target)?;
        ensure_data_marker(&target, flavor)?;
        return Ok(AppliedSwitch {
            store,
            lease,
            _source: None,
        });
    }
    validate_usable_directory(source, flavor)?;
    let source_path = source.join(DATABASE_FILE_NAME);
    if !fs::symlink_metadata(&source_path).is_ok_and(|m| m.is_file() && m.len() > 0) {
        return Err(DataDirectoryError::InvalidDatabase(source.into()));
    }
    let source_file = LockedDatabase::open(&source_path)?;
    let journal = target.join(JOURNAL_NAME);
    let migration_id = pending
        .migration_id
        .ok_or_else(|| DataDirectoryError::RecoveryRequired(target.clone()))?;
    let stage = staging_path(&target, migration_id);
    let replacing = pending.mode == SwitchMode::ReplaceWithBackup;
    let mut target_database = None;
    let mut manifest = if journal.exists() {
        let bytes =
            fs::read(&journal).map_err(|e| io_error("read migration journal", &journal, e))?;
        let manifest: Manifest = serde_json::from_slice(&bytes)
            .map_err(|_| DataDirectoryError::RecoveryRequired(target.clone()))?;
        if !matches!(manifest.version, 1 | 2)
            || replacing != manifest.backup.is_some()
            || (manifest.backup.is_some() && manifest.version != 2)
            || Some(manifest.id) != pending.migration_id
            || manifest.source != source_normalized
            || manifest.target != target
        {
            return Err(DataDirectoryError::RecoveryRequired(target));
        }
        if manifest.source_hash != source_fingerprint(&source_file, &source_path)? {
            return Err(DataDirectoryError::RecoveryRequired(target));
        }
        let source_entries = source_tree_digests(source)?;
        let expected_entries = manifest
            .files
            .iter()
            .filter(|(path, _)| path.as_str() != DATABASE_FILE_NAME)
            .map(|(path, hash)| (path.clone(), hash.clone()))
            .collect::<std::collections::BTreeMap<_, _>>();
        if source_entries != expected_entries {
            return Err(DataDirectoryError::RecoveryRequired(target));
        }
        manifest
    } else {
        // No entries are published before the journal. A private staging directory
        // named by this pending request can therefore be discarded after a crash.
        if stage.exists() {
            if (!replacing
                && [DATABASE_FILE_NAME, PLUGINS_DIR_NAME, RECORDINGS_DIR_NAME]
                    .iter()
                    .any(|name| target.join(name).exists()))
                || !fs::symlink_metadata(&stage).is_ok_and(|m| m.is_dir())
            {
                return Err(DataDirectoryError::RecoveryRequired(target));
            }
            fs::remove_dir_all(&stage)
                .map_err(|e| io_error("remove interrupted staging", &stage, e))?;
        }
        // Classification is repeated under the retained target lease.
        let backup = if replacing {
            validate_usable_directory(&target, flavor)?;
            let file = LockedDatabase::open(&target.join(DATABASE_FILE_NAME))?;
            let fingerprint = directory_fingerprint_locked(&target, &file)?;
            if pending.target_fingerprint.as_deref() != Some(fingerprint.as_str()) {
                return Err(DataDirectoryError::RecoveryRequired(target));
            }
            let backup = backup::prepare(local, &target, migration_id, &file)?;
            target_database = Some(file);
            Some(backup)
        } else {
            None
        };
        if !replacing
            && (target.join(DATABASE_FILE_NAME).exists()
                || (target.exists()
                    && !directory_is_effectively_empty(
                        &target,
                        target == normalize_existing_prefix(local),
                    )?))
        {
            return Err(DataDirectoryError::NotEmpty(target));
        }
        create_directories_durable(&target)?;
        if is_network_location(&target)? {
            return Err(DataDirectoryError::NetworkPath(target));
        }
        if stage.exists() {
            return Err(DataDirectoryError::RecoveryRequired(target));
        }
        fs::create_dir(&stage).map_err(|e| io_error("create staging", &stage, e))?;
        let copied = (|| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                fs::set_permissions(&stage, fs::Permissions::from_mode(0o700))
                    .map_err(|e| io_error("restrict staging", &stage, e))?;
            }
            let dest = stage.join(DATABASE_FILE_NAME);
            let mut output =
                private_new_file(&dest).map_err(|e| io_error("create database copy", &dest, e))?;
            let source_digest = source_file.copy_to(&mut output, &source_path)?;
            output
                .sync_all()
                .map_err(|e| io_error("sync database", &dest, e))?;
            // Source file lock remains held through publication and pointer commit.
            if source_digest != file_digest(&dest)? {
                return Err(DataDirectoryError::CopyMismatch(dest));
            }
            let mut entries = Vec::new();
            for name in [PLUGINS_DIR_NAME, RECORDINGS_DIR_NAME] {
                if fs::symlink_metadata(source.join(name)).is_ok() {
                    copy_directory_tree(&source.join(name), &stage.join(name))?;
                    entries.push(name.to_string());
                }
            }
            let verified = crate::ConnectionStore::validate_existing_file(&dest)
                .map_err(|_| DataDirectoryError::InvalidDatabase(dest.clone()))?;
            let source_hash = crate::ConnectionStore::database_fingerprint(&verified)
                .map_err(|_| DataDirectoryError::InvalidDatabase(dest.clone()))?;
            drop(verified);
            sync_file(&dest)?;
            failpoint("copied", &stage)?;
            entries.push(DATABASE_FILE_NAME.into()); // Database is always published last.
            let mut files = std::collections::BTreeMap::new();
            collect_digests(&stage, &stage, &mut files)?;
            let source_entries = source_tree_digests(source)?;
            let copied_entries = files
                .iter()
                .filter(|(path, _)| path.as_str() != DATABASE_FILE_NAME)
                .map(|(path, hash)| (path.clone(), hash.clone()))
                .collect::<std::collections::BTreeMap<_, _>>();
            if source_entries != copied_entries {
                return Err(DataDirectoryError::CopyMismatch(source.into()));
            }
            sync_directory(&stage)?;
            Ok(Manifest {
                version: if replacing { 2 } else { 1 },
                backup,
                extra: Default::default(),
                id: migration_id,
                source: source_normalized.clone(),
                database_hash: source_hash.clone(),
                source_hash,
                ready: false,
                target: target.clone(),
                files,
                entries,
            })
        })();
        let manifest = match copied {
            Ok(value) => value,
            Err(e) => {
                let _ = fs::remove_dir_all(&stage);
                return Err(e);
            }
        };
        failpoint("staged", &target)?;
        write_atomic_json(&journal, &manifest)?;
        failpoint("journaled", &target)?;
        manifest
    };
    if let Some(backup) = &manifest.backup {
        backup::archive(local, &target, migration_id, backup, &mut target_database)?;
    }
    // The old database lock belongs to the archived file, not the new database.
    drop(target_database);
    // Only recorded relative entries can be published; a changed target is never overwritten.
    for (relative, expected) in &manifest.files {
        let relative_path = Path::new(relative);
        if relative_path.is_absolute()
            || relative_path
                .components()
                .any(|p| !matches!(p, std::path::Component::Normal(_)))
        {
            return Err(DataDirectoryError::RecoveryRequired(target));
        }
        let staged = stage.join(relative_path);
        let published = target.join(relative_path);
        let actual = if staged.exists() { &staged } else { &published };
        if expected == "directory" {
            if !fs::symlink_metadata(actual).is_ok_and(|m| m.is_dir()) {
                return Err(DataDirectoryError::CopyMismatch(actual.clone()));
            }
            continue;
        }
        if !fs::symlink_metadata(actual).is_ok_and(|m| m.is_file()) {
            return Err(DataDirectoryError::UnsupportedEntry(actual.clone()));
        }
        if hex::encode(file_digest(actual)?) != *expected {
            if relative == DATABASE_FILE_NAME && actual == &published {
                let file = LockedDatabase::open(actual)?;
                if source_fingerprint(&file, actual)? == manifest.database_hash {
                    continue;
                }
            }
            return Err(DataDirectoryError::CopyMismatch(actual.clone()));
        }
    }
    let mut inventory = std::collections::BTreeMap::new();
    for name in &manifest.entries {
        if ![DATABASE_FILE_NAME, PLUGINS_DIR_NAME, RECORDINGS_DIR_NAME].contains(&name.as_str()) {
            return Err(DataDirectoryError::RecoveryRequired(target));
        }
        let staged = stage.join(name);
        let published = target.join(name);
        if staged.exists() && published.exists() {
            return Err(DataDirectoryError::NotEmpty(published));
        }
        let (root, entry) = if staged.exists() {
            (&stage, staged)
        } else {
            (&target, published)
        };
        if name == DATABASE_FILE_NAME {
            continue;
        }
        inventory.insert(name.clone(), "directory".into());
        collect_digests(root, &entry, &mut inventory)?;
    }
    let expected = manifest
        .files
        .iter()
        .filter(|(path, _)| path.as_str() != DATABASE_FILE_NAME)
        .map(|(path, hash)| (path.clone(), hash.clone()))
        .collect::<std::collections::BTreeMap<_, _>>();
    if inventory != expected {
        return Err(DataDirectoryError::RecoveryRequired(target));
    }
    for name in &manifest.entries {
        if ![DATABASE_FILE_NAME, PLUGINS_DIR_NAME, RECORDINGS_DIR_NAME].contains(&name.as_str()) {
            return Err(DataDirectoryError::RecoveryRequired(target));
        }
        let from = stage.join(name);
        let to = target.join(name);
        if from.exists() {
            if to.exists() {
                return Err(DataDirectoryError::NotEmpty(to));
            }
            publish_entry(&from, &to).map_err(|e| io_error("publish migration entry", &to, e))?;
            sync_directory(&target)?;
            failpoint(
                match name.as_str() {
                    PLUGINS_DIR_NAME => "published_plugin",
                    RECORDINGS_DIR_NAME => "published_recording",
                    _ => "published_database",
                },
                &target,
            )?;
        }
    }
    // Entries absent from the source must also be absent after publication.
    // Otherwise a newly introduced plugins/recordings tree could bypass the
    // per-entry checks simply because this migration never planned to copy it.
    if source_tree_digests(&target)? != expected {
        return Err(DataDirectoryError::RecoveryRequired(target));
    }
    ensure_data_marker(&target, flavor)?;
    let store = open_existing(&target)?;
    manifest.database_hash = store
        .content_fingerprint()
        .map_err(|_| DataDirectoryError::InvalidDatabase(target.clone()))?;
    manifest.ready = true;
    failpoint("ready", &target)?;
    write_atomic_json(&journal, &manifest)?;
    Ok(AppliedSwitch {
        store,
        lease,
        _source: Some(source_file),
    })
}

fn source_fingerprint(source: &LockedDatabase, path: &Path) -> Result<String, DataDirectoryError> {
    let temp =
        std::env::temp_dir().join(format!("nyaterm-fingerprint-{}.redb", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut output =
            private_new_file(&temp).map_err(|e| io_error("create validation copy", &temp, e))?;
        source.copy_to(&mut output, path)?;
        drop(output);
        let db = crate::ConnectionStore::validate_existing_file(&temp)
            .map_err(|_| DataDirectoryError::InvalidDatabase(path.into()))?;
        crate::ConnectionStore::database_fingerprint(&db)
            .map_err(|_| DataDirectoryError::InvalidDatabase(path.into()))
    })();
    let _ = fs::remove_file(temp);
    result
}

fn copy_directory_tree(source: &Path, dest: &Path) -> Result<(), DataDirectoryError> {
    if !fs::symlink_metadata(source)
        .map_err(|e| io_error("inspect", source, e))?
        .is_dir()
    {
        return Err(DataDirectoryError::UnsupportedEntry(source.into()));
    }
    fs::create_dir(dest).map_err(|e| io_error("create", dest, e))?;
    for entry in fs::read_dir(source).map_err(|e| io_error("list", source, e))? {
        let entry = entry.map_err(|e| io_error("list", source, e))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|e| io_error("inspect", &entry.path(), e))?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if metadata.is_dir() {
            copy_directory_tree(&from, &to)?;
        } else if metadata.is_file() {
            failpoint("file_copy", &from)?;
            let mut input =
                File::open(&from).map_err(|e| io_error("open source entry", &from, e))?;
            let mut output =
                private_new_file(&to).map_err(|e| io_error("create staged entry", &to, e))?;
            std::io::copy(&mut input, &mut output).map_err(|e| io_error("copy", &from, e))?;
            failpoint("file_sync", &to)?;
            output
                .set_permissions(metadata.permissions())
                .map_err(|e| io_error("preserve entry permissions", &to, e))?;
            output
                .sync_all()
                .map_err(|e| io_error("sync entry", &to, e))?;
            failpoint("file_verify", &to)?;
            if file_digest(&from)? != file_digest(&to)? {
                return Err(DataDirectoryError::CopyMismatch(to));
            }
        } else {
            return Err(DataDirectoryError::UnsupportedEntry(from));
        }
    }
    sync_directory(dest)
}

fn collect_digests(
    root: &Path,
    path: &Path,
    result: &mut std::collections::BTreeMap<String, String>,
) -> Result<(), DataDirectoryError> {
    for entry in fs::read_dir(path).map_err(|e| io_error("list staging", path, e))? {
        let entry = entry.map_err(|e| io_error("list staging", path, e))?;
        let kind = entry
            .file_type()
            .map_err(|e| io_error("inspect staging", &entry.path(), e))?;
        let relative = entry
            .path()
            .strip_prefix(root)
            .expect("staging child")
            .to_str()
            .ok_or_else(|| DataDirectoryError::UnsupportedEntry(entry.path()))?
            .to_owned();
        if kind.is_dir() {
            result.insert(relative, "directory".into());
            collect_digests(root, &entry.path(), result)?;
        } else if kind.is_file() {
            result.insert(relative, hex::encode(file_digest(&entry.path())?));
        } else {
            return Err(DataDirectoryError::UnsupportedEntry(entry.path()));
        }
    }
    Ok(())
}

fn source_tree_digests(
    source: &Path,
) -> Result<std::collections::BTreeMap<String, String>, DataDirectoryError> {
    let mut entries = std::collections::BTreeMap::new();
    for name in [PLUGINS_DIR_NAME, RECORDINGS_DIR_NAME] {
        let path = source.join(name);
        if fs::symlink_metadata(&path).is_ok() {
            if !fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                return Err(DataDirectoryError::UnsupportedEntry(path));
            }
            entries.insert(name.into(), "directory".into());
            collect_digests(source, &path, &mut entries)?;
        }
    }
    Ok(entries)
}

fn staging_path(target: &Path, id: uuid::Uuid) -> PathBuf {
    target.join(format!("{STAGING_NAME}-{id}"))
}

fn finish_migration(target: &Path) -> Result<(), DataDirectoryError> {
    let journal = target.join(JOURNAL_NAME);
    if journal.exists() {
        let manifest: Manifest = serde_json::from_slice(
            &fs::read(&journal).map_err(|e| io_error("read completed journal", &journal, e))?,
        )
        .map_err(|_| DataDirectoryError::RecoveryRequired(target.into()))?;
        let stage = staging_path(target, manifest.id);
        if stage.exists() {
            fs::remove_dir(&stage).map_err(|e| io_error("remove completed staging", &stage, e))?;
        }
        fs::remove_file(&journal).map_err(|e| io_error("remove completed journal", &journal, e))?;
    }
    sync_directory(target)
}

fn private_new_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

fn failpoint(name: &'static str, path: &Path) -> Result<(), DataDirectoryError> {
    #[cfg(test)]
    {
        if TEST_FAILURE.with(|point| point.get() == Some(name)) {
            TEST_FAILURE.with(|point| point.set(None));
            return Err(io_error(
                "injected migration failure",
                path,
                std::io::Error::other(name),
            ));
        }
    }
    let _ = (name, path);
    Ok(())
}

#[cfg(test)]
thread_local! { static TEST_FAILURE: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) }; }

fn write_atomic_json(path: &Path, value: &impl Serialize) -> Result<(), DataDirectoryError> {
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let bytes =
        serde_json::to_vec(value).map_err(|_| DataDirectoryError::RecoveryRequired(path.into()))?;
    let mut file = private_new_file(&temp).map_err(|e| io_error("create metadata", &temp, e))?;
    let result = (|| {
        file.write_all(&bytes)
            .map_err(|e| io_error("write metadata", &temp, e))?;
        failpoint("metadata_sync", path)?;
        file.sync_all()
            .map_err(|e| io_error("sync metadata", &temp, e))?;
        replace_file(&temp, path).map_err(|e| io_error("replace metadata", path, e))?;
        sync_directory(path.parent().unwrap_or(Path::new(".")))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn create_directories_durable(path: &Path) -> Result<(), DataDirectoryError> {
    let mut missing = Vec::new();
    let mut parent = path;
    while !parent.exists() {
        missing.push(parent.to_path_buf());
        parent = parent
            .parent()
            .ok_or_else(|| DataDirectoryError::RelativePath(path.into()))?;
    }
    fs::create_dir_all(path).map_err(|e| io_error("create directory", path, e))?;
    // Persist each new directory entry, including ancestors of the selected root.
    for directory in missing.iter().rev() {
        sync_directory(directory.parent().unwrap_or(parent))?;
        sync_directory(directory)?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), DataDirectoryError> {
    failpoint("directory_sync", path)?;
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(|e| io_error("sync directory", path, e))
    }
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    } // Atomic replacements use MOVEFILE_WRITE_THROUGH on Windows.
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
    if marker.version != MARKER_VERSION {
        return Err(DataDirectoryError::InvalidLocation {
            path: data_dir.join(DATA_MARKER_FILE_NAME),
            message: "unsupported marker version".into(),
        });
    }
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
    backup::BACKUPS_DIR,
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

fn publish_entry(source: &Path, dest: &Path) -> std::io::Result<()> {
    #[cfg(not(windows))]
    {
        fs::rename(source, dest)
    }
    #[cfg(windows)]
    {
        windows_move(source, dest, false)
    }
}

#[cfg(windows)]
fn windows_move(source: &Path, dest: &Path, replace: bool) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let dest = dest
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let flags = MOVEFILE_WRITE_THROUGH
        | if replace {
            MOVEFILE_REPLACE_EXISTING
        } else {
            0
        };
    // SAFETY: live, NUL-terminated UTF-16 path buffers.
    if unsafe { MoveFileExW(source.as_ptr(), dest.as_ptr(), flags) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn replace_file(source: &Path, dest: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        windows_move(source, dest, true)
    }
    #[cfg(not(windows))]
    {
        fs::rename(source, dest)
    }
}

fn is_network_location(path: &Path) -> Result<bool, DataDirectoryError> {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        // Canonicalized paths arrive as \\?\C:\... or \\?\UNC\server\share.
        if text.to_ascii_lowercase().starts_with(r"\\?\unc\") {
            return Ok(true);
        }
        let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
        if text.starts_with(r"\\") || text.starts_with("//") {
            return Ok(true);
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
            if drive_type <= 1 {
                return Err(io_error(
                    "inspect mount",
                    path,
                    std::io::Error::other("unknown drive type"),
                ));
            }
            return Ok(drive_type == DRIVE_REMOTE);
        }
        Ok(false)
    }
    #[cfg(target_os = "linux")]
    {
        let canonical = normalize_existing_prefix(path);
        let mounts = fs::read_to_string("/proc/mounts")
            .map_err(|e| io_error("read mount information", path, e))?;
        Ok(linux_network_mount(&canonical, &mounts))
    }
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt as _;
        let canonical = normalize_existing_prefix(path);
        // statfs needs an existing path. A directory that is not created yet
        // will live on the mount of its closest existing ancestor.
        let existing = canonical
            .ancestors()
            .find(|ancestor| ancestor.exists())
            .unwrap_or(&canonical);
        let path_c = CString::new(existing.as_os_str().as_bytes())
            .map_err(|_| DataDirectoryError::RelativePath(path.into()))?;
        let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: path is NUL-terminated; stat points to writable storage.
        if unsafe { libc::statfs(path_c.as_ptr(), stat.as_mut_ptr()) } != 0 {
            return Err(io_error(
                "inspect mount",
                path,
                std::io::Error::last_os_error(),
            ));
        }
        // SAFETY: statfs initialized the structure on success.
        // `f_flags` is a `u32` while libc declares `MNT_LOCAL` as a `c_int`.
        Ok(unsafe { stat.assume_init() }.f_flags & libc::MNT_LOCAL as u32 == 0)
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        Err(DataDirectoryError::UnsupportedEntry(path.into()))
    }
}

#[cfg(any(target_os = "linux", test))]
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

#[cfg(any(target_os = "linux", test))]
fn linux_network_mount(path: &Path, mounts: &str) -> bool {
    let mut best = None;
    for line in mounts.lines() {
        let mut fields = line.split_whitespace();
        let _device = fields.next();
        let Some(mount) = fields.next() else {
            continue;
        };
        let Some(kind) = fields.next() else {
            continue;
        };
        let mount = mount
            .replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\");
        if path.starts_with(&mount) && best.is_none_or(|(len, _)| mount.len() >= len) {
            best = Some((mount.len(), is_remote_file_system(kind)));
        }
    }
    best.is_some_and(|(_, remote)| remote)
}

#[cfg(test)]
mod tests;
