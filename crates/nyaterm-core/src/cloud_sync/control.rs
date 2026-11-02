//! Cooperative cancellation with a non-interruptible publication/commit boundary.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use super::{
    CloudLocalStore, CloudSyncBackupInfo, CloudSyncError, CloudSyncRemote, CloudSyncState,
    LocalCloudSyncOptions, RemoteFileCapacity, RemoteSyncPointer,
};
use crate::RawPortableSnapshot;

const PREPARING: u8 = 0;
const CANCELLED: u8 = 1;
const COMMITTING: u8 = 2;

#[derive(Clone, Default)]
pub struct CloudSyncOperationControl(Arc<AtomicU8>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudSyncCancellation {
    Stopping,
    FinishingCommit,
}

impl CloudSyncOperationControl {
    pub fn request_cancel(&self) -> CloudSyncCancellation {
        match self
            .0
            .compare_exchange(PREPARING, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) | Err(CANCELLED) => CloudSyncCancellation::Stopping,
            Err(COMMITTING) => CloudSyncCancellation::FinishingCommit,
            Err(_) => unreachable!("sync control state"),
        }
    }

    pub fn checkpoint(&self) -> Result<(), CloudSyncError> {
        if self.0.load(Ordering::Acquire) == CANCELLED {
            Err(CloudSyncError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Whichever wins this transition determines whether publication may start.
    /// Once a write can be visible, cancellation must not hide its final result.
    pub fn begin_commit(&self) -> Result<(), CloudSyncError> {
        match self
            .0
            .compare_exchange(PREPARING, COMMITTING, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) | Err(COMMITTING) => Ok(()),
            Err(CANCELLED) => Err(CloudSyncError::Cancelled),
            Err(_) => unreachable!("sync control state"),
        }
    }

    pub fn remote<'a>(&'a self, remote: &'a dyn CloudSyncRemote) -> impl CloudSyncRemote + 'a {
        ControlledRemote {
            control: self,
            remote,
        }
    }

    pub fn local_store<'a>(&'a self, store: &'a dyn CloudLocalStore) -> impl CloudLocalStore + 'a {
        ControlledStore {
            control: self,
            store,
        }
    }
}

struct ControlledRemote<'a> {
    control: &'a CloudSyncOperationControl,
    remote: &'a dyn CloudSyncRemote,
}

impl CloudSyncRemote for ControlledRemote<'_> {
    fn write_sync_file(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        self.control.checkpoint()?;
        self.remote.write_sync_file(path, bytes)
    }
    fn list_sync_files(&self, path: &str) -> Result<Vec<String>, CloudSyncError> {
        self.control.checkpoint()?;
        self.remote.list_sync_files(path)
    }
    fn provider(&self) -> &'static str {
        self.remote.provider()
    }
    fn file_capacity(&self) -> Result<Option<RemoteFileCapacity>, CloudSyncError> {
        self.control.checkpoint()?;
        self.remote.file_capacity()
    }
    fn read_conditional(
        &self,
        path: &str,
        etag: Option<&str>,
    ) -> Result<super::io::CloudObjectRead, CloudSyncError> {
        self.control.checkpoint()?;
        let result = self.remote.read_conditional(path, etag)?;
        self.control.checkpoint()?;
        Ok(result)
    }
    fn list_metadata(
        &self,
        path: &str,
    ) -> Result<Vec<super::io::CloudObjectMetadata>, CloudSyncError> {
        self.control.checkpoint()?;
        let result = self.remote.list_metadata(path)?;
        self.control.checkpoint()?;
        Ok(result)
    }
    fn observe(&self, event: super::io::CloudSyncIoEvent) {
        self.remote.observe(event);
    }
    fn create_dir(&self, path: &str) -> Result<(), CloudSyncError> {
        self.control.checkpoint()?;
        self.remote.create_dir(path)
    }
    fn read_if_exists(&self, path: &str) -> Result<Option<Vec<u8>>, CloudSyncError> {
        self.control.checkpoint()?;
        let result = self.remote.read_if_exists(path)?;
        self.control.checkpoint()?;
        Ok(result)
    }
    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        self.control.checkpoint()?;
        self.remote.write(path, bytes)
    }
    fn delete(&self, path: &str) -> Result<(), CloudSyncError> {
        self.control.checkpoint()?;
        self.remote.delete(path)
    }
    fn list_files(&self, path: &str) -> Result<Vec<String>, CloudSyncError> {
        self.control.checkpoint()?;
        let result = self.remote.list_files(path)?;
        self.control.checkpoint()?;
        Ok(result)
    }
}

struct ControlledStore<'a> {
    control: &'a CloudSyncOperationControl,
    store: &'a dyn CloudLocalStore,
}

impl CloudLocalStore for ControlledStore<'_> {
    fn encode_sync_snapshot(
        &self,
        snapshot: &RawPortableSnapshot,
        password: &str,
    ) -> Result<Vec<u8>, CloudSyncError> {
        self.control.checkpoint()?;
        self.store.encode_sync_snapshot(snapshot, password)
    }
    fn decode_sync_snapshot(
        &self,
        bytes: &[u8],
        password: &str,
    ) -> Result<RawPortableSnapshot, CloudSyncError> {
        self.control.checkpoint()?;
        self.store.decode_sync_snapshot(bytes, password)
    }
    fn encode_sync_pointer(&self, pointer: &RemoteSyncPointer) -> Result<Vec<u8>, CloudSyncError> {
        self.control.checkpoint()?;
        self.store.encode_sync_pointer(pointer)
    }
    fn decode_sync_pointer(&self, bytes: &[u8]) -> Result<RemoteSyncPointer, CloudSyncError> {
        self.control.checkpoint()?;
        self.store.decode_sync_pointer(bytes)
    }
    fn build_sync_snapshot(
        &self,
        options: &LocalCloudSyncOptions,
    ) -> Result<RawPortableSnapshot, CloudSyncError> {
        self.control.checkpoint()?;
        let snapshot = self.store.build_sync_snapshot(options)?;
        self.control.checkpoint()?;
        Ok(snapshot)
    }
    fn apply_sync_snapshot(
        &self,
        options: &LocalCloudSyncOptions,
        snapshot: &RawPortableSnapshot,
    ) -> Result<CloudSyncBackupInfo, CloudSyncError> {
        self.control.begin_commit()?;
        self.store.apply_sync_snapshot(options, snapshot)
    }
    fn persist_cloud_sync_state(&self, state: &CloudSyncState) -> Result<(), CloudSyncError> {
        self.control.begin_commit()?;
        self.store.persist_cloud_sync_state(state)
    }
    fn apply_sync_snapshot_checked(
        &self,
        options: &LocalCloudSyncOptions,
        snapshot: &RawPortableSnapshot,
        expected_content_hash: &str,
        state: CloudSyncState,
    ) -> Result<(CloudSyncBackupInfo, CloudSyncState), CloudSyncError> {
        self.control.begin_commit()?;
        self.store
            .apply_sync_snapshot_checked(options, snapshot, expected_content_hash, state)
    }
    fn verify_master_password(&self, password: &str) -> Result<Option<bool>, CloudSyncError> {
        self.store.verify_master_password(password)
    }
}

#[cfg(test)]
mod tests {
    use super::{CloudSyncCancellation, CloudSyncOperationControl};
    use crate::cloud_sync::CloudSyncError;

    #[test]
    fn cancellation_prevents_commit_but_cannot_interrupt_an_existing_commit() {
        let cancelled = CloudSyncOperationControl::default();
        assert_eq!(cancelled.request_cancel(), CloudSyncCancellation::Stopping);
        assert!(matches!(
            cancelled.begin_commit(),
            Err(CloudSyncError::Cancelled)
        ));
        let committing = CloudSyncOperationControl::default();
        committing.begin_commit().unwrap();
        assert_eq!(
            committing.request_cancel(),
            CloudSyncCancellation::FinishingCommit
        );
        committing.checkpoint().unwrap();
        committing.begin_commit().unwrap();
    }

    #[test]
    fn cancellation_and_commit_have_exactly_one_winner() {
        for _ in 0..100 {
            let control = CloudSyncOperationControl::default();
            let other = control.clone();
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let worker_barrier = barrier.clone();
            let worker = std::thread::spawn(move || {
                worker_barrier.wait();
                other.begin_commit()
            });
            barrier.wait();
            let cancelled = control.request_cancel();
            assert_eq!(
                worker.join().unwrap().is_ok(),
                cancelled == CloudSyncCancellation::FinishingCommit
            );
        }
    }
    use std::sync::Arc;
}
