//! Disk-backed fingerprints authorizing download resume.
//!
//! A local file may serve as a resume prefix only when it is a byte-for-byte
//! prefix of the *current* remote version. File size alone cannot prove that:
//! a remote edit that inserts bytes near the head keeps the tail identical, so
//! a size-based resume silently produces a spliced, corrupt file.
//!
//! After each download we persist a small fingerprint keyed by the local path.
//! A later transfer verifies local identity plus remote path/size/mtime with
//! O(1) metadata calls. No remote content is read for the check, so the cost is
//! independent of file size. A missing, stale, or unreadable stamp always
//! degrades to a full download rather than risking corrupt content.

use super::FileIdentity;
use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const STAMP_VERSION: u8 = 1;
const RESUME_DIRECTORY: &str = "download-resume";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ResumeStamp {
    version: u8,
    local: LocalStamp,
    remote: RemoteStamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LocalStamp {
    volume: u64,
    index: u64,
    length: u64,
    modified_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RemoteStamp {
    path: Vec<u8>,
    size: u64,
    mtime: Option<u32>,
}

/// Store of download resume fingerprints.
pub(crate) struct ResumeStampStore {
    root: PathBuf,
}

impl ResumeStampStore {
    /// System store under the platform temp directory.
    pub(crate) fn system() -> Self {
        Self {
            root: std::env::temp_dir().join(".nyaterm").join(RESUME_DIRECTORY),
        }
    }

    /// Custom root for tests.
    #[cfg(test)]
    pub(crate) fn with_root(root: PathBuf) -> Self {
        Self { root }
    }

    fn stamp_path(&self, local_path: &Path) -> PathBuf {
        let digest = Sha256::digest(local_path.as_os_str().as_encoded_bytes());
        let key = digest
            .iter()
            .take(16)
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        self.root.join(format!("{key}.json"))
    }

    /// Persist the fingerprint of a just-committed local file.
    ///
    /// `identity` must be the identity returned by the commit so the stamp
    /// pins the exact inode the bytes were written to.
    pub(crate) fn write(
        &self,
        local_path: &Path,
        identity: &FileIdentity,
        remote_path: &[u8],
        remote_size: Option<u64>,
        remote_mtime: Option<u32>,
    ) -> anyhow::Result<()> {
        let remote_size = remote_size.context("download source has no size for resume stamp")?;
        let metadata = fs::symlink_metadata(local_path).with_context(|| {
            format!(
                "read metadata for download resume stamp: {}",
                local_path.display()
            )
        })?;
        anyhow::ensure!(
            metadata.is_file(),
            "download resume target is not a regular file"
        );
        let (volume, index) = identity.parts();
        let stamp = ResumeStamp {
            version: STAMP_VERSION,
            local: LocalStamp {
                volume,
                index,
                length: metadata.len(),
                modified_secs: system_time_secs(metadata.modified()?)?,
            },
            remote: RemoteStamp {
                path: remote_path.to_vec(),
                size: remote_size,
                mtime: remote_mtime,
            },
        };
        let encoded = serde_json::to_vec(&stamp).context("serialize download resume stamp")?;
        fs::create_dir_all(&self.root).context("create download resume directory")?;
        let mut builder = tempfile::Builder::new();
        builder.prefix(".nyaterm-stamp-");
        let temporary = builder
            .tempfile_in(&self.root)
            .context("create temporary download resume stamp")?;
        use std::io::Write as _;
        temporary.as_file().write_all(&encoded)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(self.stamp_path(local_path))
            .map_err(|error| error.error)
            .context("commit download resume stamp")?;
        Ok(())
    }

    /// Return the byte offset that is safe to resume from, or zero when the
    /// local file cannot be proven to be a prefix of the current remote file.
    pub(crate) fn resume_offset(
        &self,
        local_path: &Path,
        remote_path: &[u8],
        remote_size: Option<u64>,
        remote_mtime: Option<u32>,
    ) -> u64 {
        match self.check(local_path, remote_path, remote_size, remote_mtime) {
            Ok(offset) => offset,
            Err(error) => {
                tracing::debug!(
                    ?error,
                    path = %local_path.display(),
                    "download resume stamp rejected; falling back to full download"
                );
                self.clear(local_path);
                0
            }
        }
    }

    fn check(
        &self,
        local_path: &Path,
        remote_path: &[u8],
        remote_size: Option<u64>,
        remote_mtime: Option<u32>,
    ) -> anyhow::Result<u64> {
        let remote_size = remote_size.context("download source has no size for resume check")?;
        let bytes = fs::read(self.stamp_path(local_path))
            .with_context(|| format!("read download resume stamp for {}", local_path.display()))?;
        let stamp: ResumeStamp =
            serde_json::from_slice(&bytes).context("parse download resume stamp")?;
        anyhow::ensure!(
            stamp.version == STAMP_VERSION,
            "unsupported resume stamp version"
        );

        let metadata = fs::symlink_metadata(local_path)
            .with_context(|| format!("stat download resume target {}", local_path.display()))?;
        anyhow::ensure!(
            metadata.is_file(),
            "download resume target is not a regular file"
        );
        let identity = FileIdentity::from_path(local_path)?;
        let (volume, index) = identity.parts();
        anyhow::ensure!(
            stamp.local.volume == volume && stamp.local.index == index,
            "local download file was replaced after the recorded download"
        );
        anyhow::ensure!(
            stamp.local.length == metadata.len(),
            "local download file size changed after the recorded download"
        );
        anyhow::ensure!(
            stamp.local.modified_secs == system_time_secs(metadata.modified()?)?,
            "local download file was modified after the recorded download"
        );

        // Without a remote timestamp there is no reliable way to identify a
        // remote revision, so refuse to resume rather than risk a splice.
        let Some(stamp_mtime) = stamp.remote.mtime else {
            anyhow::bail!("recorded download has no remote timestamp")
        };
        anyhow::ensure!(
            remote_mtime == Some(stamp_mtime),
            "remote file changed since the recorded download"
        );
        anyhow::ensure!(stamp.remote.size == remote_size, "remote file size changed");
        anyhow::ensure!(stamp.remote.path == remote_path, "remote file path changed");

        anyhow::ensure!(
            stamp.local.length > 0 && stamp.local.length <= remote_size,
            "recorded local file is not a valid remote prefix"
        );
        Ok(stamp.local.length)
    }

    /// Whether the local file is a verified, *complete* copy of the current
    /// remote version. Used to authorize zero-byte reuse of a promised
    /// download target without trusting metadata-only checks.
    ///
    /// Unlike `resume_offset`, this requires the recorded local length to
    /// equal the remote size: a partial file must not be reported as complete.
    pub(crate) fn matches(
        &self,
        local_path: &Path,
        remote_path: &[u8],
        remote_size: Option<u64>,
        remote_mtime: Option<u32>,
    ) -> bool {
        matches!(self.check(local_path, remote_path, remote_size, remote_mtime), Ok(offset) if Some(offset) == remote_size)
    }

    /// Remove a stamp (best effort).
    pub(crate) fn clear(&self, local_path: &Path) {
        let _ = fs::remove_file(self.stamp_path(local_path));
    }
}

fn system_time_secs(time: SystemTime) -> anyhow::Result<u64> {
    Ok(time
        .duration_since(UNIX_EPOCH)
        .context("file timestamp predates the Unix epoch")?
        .as_secs())
}

#[cfg(test)]
mod tests {
    use super::ResumeStampStore;
    use crate::download_path::FileIdentity;
    use sha2::Digest as _;

    const REMOTE: &[u8] = b"/home/user/test.txt";

    fn store() -> (tempfile::TempDir, ResumeStampStore) {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("stamps");
        (dir, ResumeStampStore::with_root(root))
    }

    fn write_stamp(
        store: &ResumeStampStore,
        local: &std::path::Path,
        remote_path: &[u8],
        remote_size: Option<u64>,
        mtime: Option<u32>,
    ) {
        let identity = FileIdentity::from_path(local).expect("identity");
        store
            .write(local, &identity, remote_path, remote_size, mtime)
            .expect("write stamp");
    }

    #[test]
    fn matching_fingerprint_allows_resume() -> anyhow::Result<()> {
        let (_dir, store) = store();
        let local = _dir.path().join("test.txt");
        std::fs::write(&local, b"0123456789")?;
        write_stamp(&store, &local, REMOTE, Some(20), Some(42));

        assert_eq!(store.resume_offset(&local, REMOTE, Some(20), Some(42)), 10);
        Ok(())
    }

    #[test]
    fn remote_size_growth_rejects_resume() -> anyhow::Result<()> {
        // Reproduces issue #1367: the local file is a valid prefix length-wise,
        // but the remote grew, so its head differs even though the tail matches.
        let (_dir, store) = store();
        let local = _dir.path().join("test.txt");
        std::fs::write(&local, b"10\n11\n12\n")?;
        write_stamp(&store, &local, REMOTE, Some(9), Some(42));

        assert_eq!(store.resume_offset(&local, REMOTE, Some(18), Some(43)), 0);
        assert_eq!(store.resume_offset(&local, REMOTE, Some(18), Some(42)), 0);
        Ok(())
    }

    #[test]
    fn head_insertion_with_identical_tail_never_splices() -> anyhow::Result<()> {
        // Exact issue #1367 reproduction: `seq 10 1000000` then `seq 1 1000000`.
        // The new file is 18 bytes larger (the one-digit lines at the head) and
        // has an identical tail, so a size-only resume would splice old head
        // with new tail and yield a corrupt file. The fingerprint must reject
        // the resume so the whole file is fetched.
        let (_dir, store) = store();
        let local = _dir.path().join("test.txt");
        let old: Vec<u8> = (10..=1_000_000)
            .flat_map(|n| format!("{n}\n").into_bytes())
            .collect();
        std::fs::write(&local, &old)?;
        let old_size = old.len() as u64;
        let new_size = old_size + 18;
        write_stamp(&store, &local, REMOTE, Some(old_size), Some(1000));

        // Remote rewrite changes both size and mtime in the real world.
        assert_eq!(
            store.resume_offset(&local, REMOTE, Some(new_size), Some(1001)),
            0,
            "a head-only insertion must never be resumed from the stale local prefix"
        );
        Ok(())
    }

    #[test]
    fn remote_mtime_change_rejects_resume() -> anyhow::Result<()> {
        let (_dir, store) = store();
        let local = _dir.path().join("test.txt");
        std::fs::write(&local, b"0123456789")?;
        write_stamp(&store, &local, REMOTE, Some(20), Some(42));

        assert_eq!(store.resume_offset(&local, REMOTE, Some(20), Some(43)), 0);
        Ok(())
    }

    #[test]
    fn missing_remote_mtime_rejects_resume() -> anyhow::Result<()> {
        let (_dir, store) = store();
        let local = _dir.path().join("test.txt");
        std::fs::write(&local, b"0123456789")?;
        write_stamp(&store, &local, REMOTE, Some(20), None);

        assert_eq!(store.resume_offset(&local, REMOTE, Some(20), None), 0);
        Ok(())
    }

    #[test]
    fn replaced_local_file_rejects_resume() -> anyhow::Result<()> {
        let (_dir, store) = store();
        let local = _dir.path().join("test.txt");
        std::fs::write(&local, b"0123456789")?;
        write_stamp(&store, &local, REMOTE, Some(20), Some(42));

        // Replace the inode while keeping size and mtime plausible.
        std::fs::remove_file(&local)?;
        std::fs::write(&local, b"0123456789")?;
        assert_eq!(store.resume_offset(&local, REMOTE, Some(20), Some(42)), 0);
        Ok(())
    }

    #[test]
    fn modified_local_file_rejects_resume() -> anyhow::Result<()> {
        let (_dir, store) = store();
        let local = _dir.path().join("test.txt");
        std::fs::write(&local, b"0123456789")?;
        write_stamp(&store, &local, REMOTE, Some(20), Some(42));

        std::fs::write(&local, b"0123456789EXTRA")?;
        assert_eq!(store.resume_offset(&local, REMOTE, Some(20), Some(42)), 0);
        Ok(())
    }

    #[test]
    fn different_remote_path_rejects_resume() -> anyhow::Result<()> {
        let (_dir, store) = store();
        let local = _dir.path().join("test.txt");
        std::fs::write(&local, b"0123456789")?;
        write_stamp(&store, &local, REMOTE, Some(20), Some(42));

        assert_eq!(
            store.resume_offset(&local, b"/other/file.txt", Some(20), Some(42)),
            0
        );
        Ok(())
    }

    #[test]
    fn corrupt_stamp_degrades_to_full_download() -> anyhow::Result<()> {
        let (_dir, store) = store();
        let local = _dir.path().join("test.txt");
        std::fs::write(&local, b"0123456789")?;
        let identity = FileIdentity::from_path(&local)?;
        store
            .write(&local, &identity, REMOTE, Some(20), Some(42))
            .expect("write stamp");

        // Corrupt the on-disk stamp.
        let digest = sha2::Sha256::digest(local.as_os_str().as_encoded_bytes());
        let key = digest
            .iter()
            .take(16)
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let stamp_path = _dir.path().join("stamps").join(format!("{key}.json"));
        std::fs::write(&stamp_path, b"not json")?;
        assert_eq!(store.resume_offset(&local, REMOTE, Some(20), Some(42)), 0);
        Ok(())
    }

    #[test]
    fn missing_stamp_degrades_to_full_download() -> anyhow::Result<()> {
        let (_dir, store) = store();
        let local = _dir.path().join("test.txt");
        std::fs::write(&local, b"0123456789")?;

        assert_eq!(store.resume_offset(&local, REMOTE, Some(20), Some(42)), 0);
        Ok(())
    }

    // --- matches(): gate for promised (drag-export) zero-byte reuse ---

    #[test]
    fn matches_accepts_a_verified_complete_copy() -> anyhow::Result<()> {
        let (_dir, store) = store();
        let local = _dir.path().join("test.txt");
        std::fs::write(&local, b"0123456789")?;
        write_stamp(&store, &local, REMOTE, Some(10), Some(42));

        assert!(store.matches(&local, REMOTE, Some(10), Some(42)));
        Ok(())
    }

    #[test]
    fn matches_rejects_a_partial_copy() -> anyhow::Result<()> {
        // A partial must never be reused as if it were a complete download.
        let (_dir, store) = store();
        let local = _dir.path().join("test.txt");
        std::fs::write(&local, b"01234")?;
        write_stamp(&store, &local, REMOTE, Some(20), Some(42));

        assert!(!store.matches(&local, REMOTE, Some(20), Some(42)));
        Ok(())
    }

    #[test]
    fn matches_rejects_a_changed_remote_revision() -> anyhow::Result<()> {
        let (_dir, store) = store();
        let local = _dir.path().join("test.txt");
        std::fs::write(&local, b"10\n11\n12\n")?;
        write_stamp(&store, &local, REMOTE, Some(9), Some(42));

        // Same size, different mtime: a remote rewrite must disable reuse.
        assert!(!store.matches(&local, REMOTE, Some(9), Some(43)));
        // Same mtime, different size: likewise.
        assert!(!store.matches(&local, REMOTE, Some(18), Some(42)));
        Ok(())
    }

    #[test]
    fn matches_rejects_missing_or_replaced_stamp() -> anyhow::Result<()> {
        let (_dir, store) = store();
        let local = _dir.path().join("test.txt");
        std::fs::write(&local, b"0123456789")?;

        // No stamp at all -> fail safe to a real download.
        assert!(!store.matches(&local, REMOTE, Some(10), Some(42)));

        write_stamp(&store, &local, REMOTE, Some(10), Some(42));
        // Replace the inode while keeping size/mtime plausible.
        std::fs::remove_file(&local)?;
        std::fs::write(&local, b"0123456789")?;
        assert!(!store.matches(&local, REMOTE, Some(10), Some(42)));
        Ok(())
    }
}
