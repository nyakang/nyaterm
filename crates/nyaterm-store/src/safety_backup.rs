//! Verified migration backups. Verification runs on the calling blocking worker.
use std::fs;
use std::path::{Path, PathBuf};

use nyaterm_core::{PortableSnapshotError, SecretString};

use crate::{ConfigBackupInfo, StoreBlockingClient, StoreClientError, StoreDomain};

#[derive(Debug, thiserror::Error)]
pub enum VerifiedBackupError {
    #[error("safety backup export failed: {0}")]
    Export(StoreClientError),
    #[error("safety backup synchronization failed: {0}")]
    Sync(std::io::Error),
    #[error("safety backup read failed: {0}")]
    Read(std::io::Error),
    #[error("safety backup validation failed: {0}")]
    Decode(PortableSnapshotError),
}

impl StoreBlockingClient {
    /// Export without mutating the source, then verify the actual durable file.
    /// Must run off the UI thread; only export occupies the database queue.
    pub fn export_verified_v4_safety_backup(
        &self,
        output: PathBuf,
        password: SecretString,
    ) -> Result<ConfigBackupInfo, VerifiedBackupError> {
        let export_path = output.clone();
        let export_password = password.clone();
        let info = self
            .request_fn(StoreDomain::CloudSync, move |store| {
                store.export_v4_safety_backup(&export_path, export_password.expose_secret())
            })
            .map_err(VerifiedBackupError::Export)?;
        verify_file(&output, password.expose_secret())?;
        Ok(info)
    }
}

fn verify_file(path: &Path, password: &str) -> Result<(), VerifiedBackupError> {
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .and_then(|file| file.sync_all())
        .map_err(VerifiedBackupError::Sync)?;
    let bytes = fs::read(path).map_err(VerifiedBackupError::Read)?;
    crate::decode_encrypted_raw_portable_snapshot(&bytes, password)
        .map_err(VerifiedBackupError::Decode)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{VerifiedBackupError, verify_file};
    use crate::{StoreConfig, StoreRuntime};
    use nyaterm_core::SecretString;
    use nyaterm_core::test_support::TestTempDir;

    #[test]
    fn verifies_the_written_backup_and_rejects_wrong_password_and_truncated_file() {
        let root = TestTempDir::new("nyaterm-verified-safety-backup");
        let runtime = StoreRuntime::spawn(StoreConfig {
            config_dir: root.path().to_path_buf(),
            portable_key_path: None,
        })
        .unwrap();
        let file = root.join("safety.nya");
        runtime
            .blocking_client()
            .export_verified_v4_safety_backup(file.clone(), SecretString::new("fixture-password"))
            .unwrap();
        verify_file(&file, "fixture-password").unwrap();
        assert!(matches!(
            verify_file(&file, "wrong-password"),
            Err(VerifiedBackupError::Decode(_))
        ));
        let bytes = std::fs::read(&file).unwrap();
        std::fs::write(&file, &bytes[..bytes.len() / 2]).unwrap();
        assert!(matches!(
            verify_file(&file, "fixture-password"),
            Err(VerifiedBackupError::Decode(_))
        ));
        assert!(matches!(
            verify_file(&root.join("missing.nya"), "fixture-password"),
            Err(VerifiedBackupError::Sync(_))
        ));
    }
}
