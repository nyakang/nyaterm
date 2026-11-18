//! Explicit database recovery. Opening a directory never invokes this path.
use std::fs::{self, File, OpenOptions};
use std::io::Read as _;
use std::path::{Path, PathBuf};

use nyaterm_core::app_identity::AppFlavor;
use nyaterm_core::data_directory::{DataDirectoryError, PreparedDatabaseRebuild};
use sha2::{Digest as _, Sha256};

use crate::ConnectionStore;
use crate::data_directory::locked_database::LockedDatabase;
use crate::data_directory::{
    DATA_DIR_ENV, DATABASE_FILE_NAME, DataRootLease, classify_storage_error, failpoint, io_error,
    open_existing, read_location, sync_directory, validate_directory, write_location,
};

fn validate_target(
    local: &Path,
    default_dir: &Path,
    data_dir: &Path,
    flavor: AppFlavor,
) -> Result<(), DataDirectoryError> {
    let location = read_location(local)?.unwrap_or_default();
    let configured = location
        .data_dir
        .as_deref()
        .map(Path::new)
        .unwrap_or(default_dir);
    if std::env::var_os(DATA_DIR_ENV).is_some()
        || location.pending.is_some()
        || configured != data_dir
    {
        return Err(DataDirectoryError::RecoveryRequired(data_dir.into()));
    }
    validate_directory(data_dir, flavor, false, false)?;
    if !data_dir.is_dir() {
        return Err(DataDirectoryError::RecoveryRequired(data_dir.into()));
    }
    Ok(())
}

fn recoverable(data_dir: &Path) -> Result<(), DataDirectoryError> {
    match open_existing(data_dir) {
        Err(DataDirectoryError::DatabaseMissing(_) | DataDirectoryError::DatabaseCorrupt(_)) => {
            Ok(())
        }
        Err(error) => Err(error),
        Ok(_) => Err(DataDirectoryError::InvalidDatabase(data_dir.into())),
    }
}

fn database_digest(data_dir: &Path) -> Result<Option<[u8; 32]>, DataDirectoryError> {
    let path = data_dir.join(DATABASE_FILE_NAME);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error("inspect database", &path, error)),
    };
    if !metadata.is_file() {
        return Err(DataDirectoryError::InvalidDatabase(data_dir.into()));
    }
    let mut input = File::open(&path).map_err(|error| io_error("read database", &path, error))?;
    let mut digest = Sha256::new();
    let mut bytes = [0u8; 64 * 1024];
    loop {
        let count = input
            .read(&mut bytes)
            .map_err(|error| io_error("read database", &path, error))?;
        if count == 0 {
            break;
        }
        digest.update(&bytes[..count]);
    }
    Ok(Some(digest.finalize().into()))
}

pub fn prepare_database_rebuild(
    local: &Path,
    default_dir: &Path,
    data_dir: &Path,
    flavor: AppFlavor,
) -> Result<PreparedDatabaseRebuild, DataDirectoryError> {
    let _lease = DataRootLease::acquire(data_dir)?;
    validate_target(local, default_dir, data_dir, flavor)?;
    recoverable(data_dir)?;
    Ok(PreparedDatabaseRebuild {
        data_dir: data_dir.into(),
        database_digest: database_digest(data_dir)?,
    })
}

/// Publish a validated replacement only after confirmation has been revalidated.
/// Errors retain the original database; unrelated files and credential material
/// are never removed or rewritten by recovery.
pub fn commit_database_rebuild(
    local: &Path,
    default_dir: &Path,
    choice: &PreparedDatabaseRebuild,
    flavor: AppFlavor,
) -> Result<Option<PathBuf>, DataDirectoryError> {
    let data_dir = &choice.data_dir;
    let _lease = DataRootLease::acquire(data_dir)?;
    validate_target(local, default_dir, data_dir, flavor)?;
    if database_digest(data_dir)? != choice.database_digest {
        return Err(DataDirectoryError::CopyMismatch(data_dir.clone()));
    }
    recoverable(data_dir)?;
    // Reject older clients holding only the database lock, not our directory lease.
    let database = data_dir.join(DATABASE_FILE_NAME);
    let lock = if fs::symlink_metadata(&database).is_ok_and(|metadata| metadata.len() > 0) {
        Some(LockedDatabase::open(&database)?)
    } else {
        None
    };
    let staged = data_dir.join(format!(
        ".{DATABASE_FILE_NAME}.rebuild-{}",
        uuid::Uuid::new_v4()
    ));
    let mut archive = None;
    let mut published = false;
    let mut staged_created = false;
    let result = (|| {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&staged)
            .map_err(|error| io_error("stage database", &staged, error))?;
        staged_created = true;
        let db = redb::Database::builder()
            .create_file(file)
            .map_err(|source| {
                classify_storage_error(
                    data_dir,
                    crate::StorageError::Open {
                        path: staged.clone(),
                        source,
                    },
                )
            })?;
        drop(
            ConnectionStore::from_database(db, staged.clone(), None)
                .map_err(|error| classify_storage_error(data_dir, error))?,
        );
        drop(
            ConnectionStore::validate_existing_file(&staged)
                .map_err(|error| classify_storage_error(data_dir, error))?,
        );
        OpenOptions::new()
            .write(true)
            .open(&staged)
            .and_then(|file| file.sync_all())
            .map_err(|error| io_error("sync database", &staged, error))?;
        failpoint("rebuild_staged", data_dir)?;
        // Windows requires closing the range-lock handle before moving the file.
        drop(lock);
        if database_digest(data_dir)? != choice.database_digest {
            return Err(DataDirectoryError::CopyMismatch(data_dir.clone()));
        }
        if choice.database_digest.is_some() {
            let path = data_dir.join(format!(
                "{DATABASE_FILE_NAME}.corrupt-{}",
                uuid::Uuid::new_v4()
            ));
            rename_no_replace(&database, &path)
                .map_err(|error| io_error("archive database", &database, error))?;
            archive = Some(path);
            sync_directory(data_dir)?;
        }
        failpoint("rebuild_archived", data_dir)?;
        rename_no_replace(&staged, &database)
            .map_err(|error| io_error("publish database", &database, error))?;
        published = true;
        sync_directory(data_dir)?;
        failpoint("rebuild_published", data_dir)?;
        let mut location = read_location(local)?.unwrap_or_default();
        location.last_error = Some(format!(
            "database_recreated:{}",
            archive
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default()
        ));
        write_location(local, &location)?;
        Ok(archive.clone())
    })();
    if result.is_err() {
        if published {
            fs::remove_file(&database)
                .map_err(|error| io_error("remove failed replacement", &database, error))?;
        }
        if let Some(archive) = &archive {
            rename_no_replace(archive, &database)
                .map_err(|error| io_error("restore original database", &database, error))?;
        }
        sync_directory(data_dir)?;
    }
    if staged_created {
        let _ = fs::remove_file(&staged);
    }
    result
}

/// Unlike std::fs::rename on Unix, this never overwrites a concurrent file.
fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
        let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
        // SAFETY: both C strings live through the call; paths are relative to AT_FDCWD.
        if unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } == 0
        {
            return Ok(());
        }
        Err(std::io::Error::last_os_error())
    }
    #[cfg(target_vendor = "apple")]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
        let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
        // SAFETY: valid, nul-terminated paths; exclusive rename prevents replacement.
        if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } == 0 {
            return Ok(());
        }
        Err(std::io::Error::last_os_error())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;
        let from: Vec<u16> = from.as_os_str().encode_wide().chain([0]).collect();
        let to: Vec<u16> = to.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: live nul-terminated UTF-16 paths; flush the move without replacement.
        if unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
            )
        } != 0
        {
            return Ok(());
        }
        Err(std::io::Error::last_os_error())
    }
    #[cfg(not(any(target_os = "linux", target_vendor = "apple", windows)))]
    {
        fs::hard_link(from, to)?;
        if let Err(error) = fs::remove_file(from) {
            let _ = fs::remove_file(to);
            return Err(error);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::data_directory::rebuild::rename_no_replace;
    use nyaterm_core::test_support::TestTempDir;
    use std::fs;

    #[test]
    fn exclusive_rename_preserves_an_existing_archive_and_its_source() {
        let root = TestTempDir::new("nyaterm-rebuild-archive-collision");
        fs::create_dir_all(root.path()).unwrap();
        let source = root.join("source");
        let archive = root.join("archive");
        fs::write(&source, b"original database").unwrap();
        fs::write(&archive, b"older archive").unwrap();
        assert!(rename_no_replace(&source, &archive).is_err());
        assert_eq!(fs::read(&source).unwrap(), b"original database");
        assert_eq!(fs::read(&archive).unwrap(), b"older archive");
    }
}
