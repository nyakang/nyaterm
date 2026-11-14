use crate::data_directory::io_error;
#[cfg(all(unix, not(target_vendor = "apple")))]
use crate::data_directory::lock_file;
use nyaterm_core::data_directory::DataDirectoryError;
use sha2::{Digest as _, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::Path;

/// Hold redb's byte-range lock as well as the legacy Unix flock. Read through
/// the locking handle: Windows rejects reads from a different handle while locked.
pub(super) struct LockedDatabase {
    backend: redb::backends::FileBackend,
}

impl LockedDatabase {
    pub(super) fn open(path: &Path) -> Result<Self, DataDirectoryError> {
        use redb::StorageBackend as _;
        use std::ops::Bound::Unbounded;
        if !fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.len() > 0) {
            return Err(DataDirectoryError::InvalidDatabase(path.into()));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| io_error("open database", path, e))?;
        // On the Apple platforms, flock and the open file description range
        // lock below share one lock table: the range lock then fails against
        // our own flock, and every migration reports the directory as in use.
        // The range lock alone still conflicts with another process's flock.
        #[cfg(all(unix, not(target_vendor = "apple")))]
        lock_file(&file, path.parent().unwrap_or(path))?;
        let backend = redb::backends::FileBackend::new(file)
            .map_err(|_| DataDirectoryError::InvalidDatabase(path.into()))?;
        match backend.try_lock_range(Unbounded, Unbounded) {
            Ok(true) => Ok(Self { backend }),
            Ok(false) => Err(DataDirectoryError::InUse(
                path.parent().unwrap_or(path).into(),
            )),
            Err(e) => Err(io_error("lock database", path, std::io::Error::other(e))),
        }
    }

    pub(super) fn copy_to(
        &self,
        output: &mut File,
        path: &Path,
    ) -> Result<[u8; 32], DataDirectoryError> {
        use redb::StorageBackend as _;
        let length = self
            .backend
            .len()
            .map_err(|e| io_error("measure database", path, e))?;
        let mut offset = 0;
        let mut buffer = [0; 64 * 1024];
        let mut hash = Sha256::new();
        while offset < length {
            let count = (length - offset).min(buffer.len() as u64) as usize;
            let bytes = &mut buffer[..count];
            self.backend
                .read(offset, bytes)
                .map_err(|e| io_error("read database", path, e))?;
            output
                .write_all(bytes)
                .map_err(|e| io_error("copy database", path, e))?;
            hash.update(bytes);
            offset += count as u64;
        }
        Ok(hash.finalize().into())
    }
}

impl Drop for LockedDatabase {
    fn drop(&mut self) {
        use redb::StorageBackend as _;
        let _ = self.backend.close();
    }
}
