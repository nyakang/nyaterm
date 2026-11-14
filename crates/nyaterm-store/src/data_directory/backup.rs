//! Recoverable archival of an occupied default root. The journal owns all names;
//! bootstrap files and unrelated user files are never moved.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::locked_database::LockedDatabase;
use super::{
    DATA_MARKER_FILE_NAME, DATABASE_FILE_NAME, DataDirectoryError, DataRootLease, PLUGINS_DIR_NAME,
    RECORDINGS_DIR_NAME, collect_digests, failpoint, file_digest, io_error,
    normalize_existing_prefix, publish_entry, source_fingerprint, source_tree_digests,
    sync_directory,
};

pub(super) const BACKUPS_DIR: &str = "data-directory-backups";
const ENTRIES: &[&str] = &[
    PLUGINS_DIR_NAME,
    RECORDINGS_DIR_NAME,
    DATA_MARKER_FILE_NAME,
    DATABASE_FILE_NAME,
];

#[derive(Serialize, Deserialize)]
pub(super) struct BackupManifest {
    files: BTreeMap<String, String>,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

fn backup_path(local: &Path, id: uuid::Uuid) -> PathBuf {
    local.join(BACKUPS_DIR).join(id.to_string())
}

pub(super) fn prepare(
    local: &Path,
    target: &Path,
    id: uuid::Uuid,
    database: &LockedDatabase,
) -> Result<BackupManifest, DataDirectoryError> {
    if normalize_existing_prefix(local) != target || backup_path(local, id).exists() {
        return Err(DataDirectoryError::RecoveryRequired(target.into()));
    }
    let mut files = source_tree_digests(target)?;
    if target.join(DATA_MARKER_FILE_NAME).exists() {
        files.insert(
            DATA_MARKER_FILE_NAME.into(),
            hex::encode(file_digest(&target.join(DATA_MARKER_FILE_NAME))?),
        );
    }
    files.insert(
        DATABASE_FILE_NAME.into(),
        source_fingerprint(database, &target.join(DATABASE_FILE_NAME))?,
    );
    Ok(BackupManifest {
        files,
        extra: Default::default(),
    })
}

fn private_directory(path: &Path) -> Result<(), DataDirectoryError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(DataDirectoryError::UnsupportedEntry(path.into())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt as _;
                builder.mode(0o700);
            }
            builder
                .create(path)
                .map_err(|e| io_error("create backup directory", path, e))?;
        }
        Err(error) => return Err(io_error("inspect backup directory", path, error)),
    }
    // A previous attempt may have created the directory but failed before its
    // parent was synced. Repeat both barriers before moving any historical data.
    sync_directory(path.parent().unwrap_or(path))?;
    sync_directory(path)
}

pub(super) fn archive(
    local: &Path,
    target: &Path,
    id: uuid::Uuid,
    manifest: &BackupManifest,
    database: &mut Option<LockedDatabase>,
) -> Result<(), DataDirectoryError> {
    if normalize_existing_prefix(local) != target {
        return Err(DataDirectoryError::RecoveryRequired(target.into()));
    }
    for key in manifest.files.keys() {
        let path = Path::new(key);
        if path
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
            || !ENTRIES.iter().any(|entry| path.starts_with(entry))
        {
            return Err(DataDirectoryError::RecoveryRequired(target.into()));
        }
    }
    if !manifest.files.contains_key(DATABASE_FILE_NAME) {
        return Err(DataDirectoryError::RecoveryRequired(target.into()));
    }
    private_directory(&local.join(BACKUPS_DIR))?;
    let backup = backup_path(local, id);
    private_directory(&backup)?;
    let _lease = DataRootLease::acquire(&backup)?;
    // Keep the byte-range lock even when its file is renamed into the archive.
    let database_path = if backup.join(DATABASE_FILE_NAME).exists() {
        backup.join(DATABASE_FILE_NAME)
    } else {
        target.join(DATABASE_FILE_NAME)
    };
    if database.is_none() {
        *database = Some(LockedDatabase::open(&database_path)?);
    }
    let database = database.as_ref().expect("backup database locked");
    for name in ENTRIES {
        if !manifest.files.contains_key(*name) {
            continue;
        }
        let archived = backup.join(name);
        let active = target.join(name);
        let entry = if archived.exists() {
            &archived
        } else {
            &active
        };
        let root = if archived.exists() { &backup } else { target };
        let mut actual = BTreeMap::new();
        if *name == DATABASE_FILE_NAME {
            actual.insert(name.to_string(), source_fingerprint(database, entry)?);
        } else if *name == DATA_MARKER_FILE_NAME {
            if !fs::symlink_metadata(entry).is_ok_and(|m| m.is_file()) {
                return Err(DataDirectoryError::UnsupportedEntry(entry.clone()));
            }
            actual.insert(name.to_string(), hex::encode(file_digest(entry)?));
        } else {
            if !fs::symlink_metadata(entry).is_ok_and(|m| m.is_dir()) {
                return Err(DataDirectoryError::UnsupportedEntry(entry.clone()));
            }
            actual.insert(name.to_string(), "directory".into());
            collect_digests(root, entry, &mut actual)?;
        }
        let expected = manifest
            .files
            .iter()
            .filter(|(path, _)| Path::new(path).starts_with(name))
            .map(|(path, hash)| (path.clone(), hash.clone()))
            .collect::<BTreeMap<_, _>>();
        if actual != expected {
            return Err(DataDirectoryError::CopyMismatch(entry.clone()));
        }
        if !archived.exists() {
            publish_entry(&active, &archived)
                .map_err(|e| io_error("archive historical data", &active, e))?;
            sync_directory(&backup)?;
            sync_directory(target)?;
            failpoint(
                match *name {
                    PLUGINS_DIR_NAME => "archived_plugins",
                    RECORDINGS_DIR_NAME => "archived_recordings",
                    DATA_MARKER_FILE_NAME => "archived_marker",
                    _ => "archived_database",
                },
                target,
            )?;
        }
    }
    Ok(())
}
