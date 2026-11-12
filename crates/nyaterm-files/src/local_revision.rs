//! Stable local content versions; callers own polling, task lifetime and uploads.
use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::time::SystemTime;

use sha2::{Digest, Sha256};

/// A stable content version captured before launching an external editor.
#[derive(Clone, PartialEq, Eq)]
pub struct LocalFileVersion {
    pub len: u64,
    pub modified: Option<SystemTime>,
    pub digest: [u8; 32],
}

impl std::fmt::Debug for LocalFileVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalFileVersion")
            .field("len", &self.len)
            .field("digest", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// Streams on a worker; refuses to label a changing file as a stable version.
pub fn read_local_file_version(
    path: &Path,
    mut cancelled: impl FnMut() -> bool,
) -> io::Result<LocalFileVersion> {
    let mut file = fs::File::open(path)?;
    let before = file.metadata()?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut len = 0_u64;
    loop {
        if cancelled() {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        len += read as u64;
    }
    let after = file.metadata()?;
    let current = fs::metadata(path)?;
    if len != before.len()
        || before.len() != after.len()
        || after.len() != current.len()
        || before.modified().ok() != after.modified().ok()
        || after.modified().ok() != current.modified().ok()
    {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "external edit changed while reading",
        ));
    }
    Ok(LocalFileVersion {
        len,
        modified: current.modified().ok(),
        digest: digest.finalize().into(),
    })
}

#[cfg(test)]
mod tests {
    use super::read_local_file_version;
    use std::fs;
    use std::time::Duration;

    #[test]
    fn version_read_rejects_concurrent_edits_and_honors_immediate_stop() {
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-external-watch");
        fs::create_dir_all(root.path()).unwrap();
        let path = root.path().join("edit.txt");
        fs::write(&path, vec![b'a'; 128 * 1024]).unwrap();
        let mut calls = 0;
        let result = read_local_file_version(&path, || {
            calls += 1;
            if calls == 2 {
                fs::write(&path, "new version").unwrap();
            }
            false
        });
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        assert_eq!(
            read_local_file_version(&path, || true).unwrap_err().kind(),
            std::io::ErrorKind::Interrupted
        );
    }

    #[test]
    fn cancellation_stops_after_the_first_read_chunk() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("large");
        fs::write(&path, vec![b'a'; 128 * 1024]).unwrap();
        let mut calls = 0;
        let result = read_local_file_version(&path, || {
            calls += 1;
            calls == 2
        });
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(calls, 2);
    }

    #[test]
    fn empty_file_has_the_sha256_empty_digest_and_missing_file_is_not_found() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("empty");
        assert_eq!(
            read_local_file_version(&path, || false).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
        fs::write(&path, []).unwrap();
        let version = read_local_file_version(&path, || false).unwrap();
        assert_eq!(version.len, 0);
        let digest: String = version
            .digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(
            digest,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn same_length_edits_change_digest_but_timestamp_only_changes_do_not() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("edit");
        let timestamp = std::time::UNIX_EPOCH + Duration::from_secs(100);
        fs::write(&path, "first").unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(timestamp)
            .unwrap();
        let first = read_local_file_version(&path, || false).unwrap();
        fs::write(&path, "other").unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(timestamp)
            .unwrap();
        let other = read_local_file_version(&path, || false).unwrap();
        assert_eq!(first.len, other.len);
        assert_eq!(first.modified, other.modified);
        assert_ne!(first.digest, other.digest);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(timestamp + Duration::from_secs(10))
            .unwrap();
        let touched = read_local_file_version(&path, || false).unwrap();
        assert_ne!(other.modified, touched.modified);
        assert_eq!(other.digest, touched.digest);
    }

    #[test]
    fn debug_redacts_content_digest() {
        let version = super::LocalFileVersion {
            len: 5,
            modified: None,
            digest: [42; 32],
        };
        let debug = format!("{version:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("42"));
    }
}
