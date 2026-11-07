use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{MemoryRemote, OptionsTestLocalStore, options, unique_temp_dir};
use crate::CloudLocalStore;
use crate::cloud_sync::{
    CloudSyncError, CloudSyncRemote, CloudSyncState, RemoteSyncPointer, SYNC_LATEST_FILE, heads,
    probe_remote_sync, push_snapshot_with_remote, remote_path, write_sync_pointer,
};
use crate::portable_snapshot::SnapshotSyncClock;

fn pointer(device: &str, revision: &str, counters: &[(&str, u64)]) -> RemoteSyncPointer {
    RemoteSyncPointer {
        schema_version: 1,
        revision_id: revision.into(),
        created_at_ms: 1,
        payload_hash: "hash".into(),
        device_id: device.into(),
        app_version: "test".into(),
        sync_clock: Some(SnapshotSyncClock {
            counters: counters.iter().map(|(id, n)| ((*id).into(), *n)).collect(),
            ..Default::default()
        }),
    }
}

#[test]
fn corrupt_compatibility_pointer_preserves_causal_order_and_concurrent_branches() {
    let dir = unique_temp_dir("recovery-heads");
    let options = options(&dir, &dir, "local");
    let store = OptionsTestLocalStore(&options);
    let remote = MemoryRemote::default();
    let old = pointer("a", "z-old", &[("a", 1)]);
    let newest = pointer("b", "a-new", &[("a", 1), ("b", 1)]);
    heads::publish(&remote, &options.remote_root, &old).unwrap();
    heads::publish(&remote, &options.remote_root, &newest).unwrap();
    remote
        .write(
            &remote_path(&options.remote_root, SYNC_LATEST_FILE),
            b"corrupt",
        )
        .unwrap();
    let observed = heads::read(&store, &remote, &options.remote_root).unwrap();
    assert_eq!(observed.selected.as_ref(), Some(&newest));
    assert!(!observed.concurrent);
    let branch = pointer("c", "branch", &[("a", 1), ("c", 1)]);
    heads::publish(&remote, &options.remote_root, &branch).unwrap();
    let before = remote.files.lock().unwrap().clone();
    let observed = heads::read(&store, &remote, &options.remote_root).unwrap();
    assert!(observed.concurrent);
    assert!(matches!(
        observed.require_single(&options, "memory", "local"),
        Err(CloudSyncError::Conflict(_))
    ));
    assert_eq!(observed.observed_revisions(), vec!["a-new", "branch"]);
    assert_eq!(*remote.files.lock().unwrap(), before);
}

#[test]
fn corrupt_heads_and_pending_only_records_cannot_authorize_overwriting_corruption() {
    let dir = unique_temp_dir("recovery-invalid-heads");
    let options = options(&dir, &dir, "local");
    let store = OptionsTestLocalStore(&options);
    for damaged_head in [false, true] {
        let remote = MemoryRemote::default();
        remote
            .write(
                &remote_path(&options.remote_root, SYNC_LATEST_FILE),
                b"corrupt",
            )
            .unwrap();
        heads::stage(&remote, &options.remote_root, "remote", "uncommitted").unwrap();
        if damaged_head {
            remote
                .write(
                    &heads::device_head_path(&options.remote_root, "remote"),
                    b"bad head",
                )
                .unwrap();
        }
        let before = remote.files.lock().unwrap().clone();
        assert!(probe_remote_sync(&store, &remote, &options.remote_root).is_err());
        assert!(
            push_snapshot_with_remote(&store, &options, &remote, &CloudSyncState::default(), false)
                .is_err()
        );
        assert_eq!(
            *remote.files.lock().unwrap(),
            before,
            "failed recovery must not write or clean up"
        );
    }
}

struct ReadbackRemote {
    inner: MemoryRemote,
    faults: Mutex<usize>,
    network: bool,
    reads: AtomicUsize,
    writes: AtomicUsize,
}

impl CloudSyncRemote for ReadbackRemote {
    fn provider(&self) -> &'static str {
        "script"
    }
    fn create_dir(&self, path: &str) -> Result<(), CloudSyncError> {
        self.inner.create_dir(path)
    }
    fn list_files(&self, path: &str) -> Result<Vec<String>, CloudSyncError> {
        self.inner.list_files(path)
    }
    fn delete(&self, _: &str) -> Result<(), CloudSyncError> {
        panic!("failed read must not clean up")
    }
    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        self.writes.fetch_add(1, Ordering::Relaxed);
        self.inner.write(path, bytes)
    }
    fn read_if_exists(&self, path: &str) -> Result<Option<Vec<u8>>, CloudSyncError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        let mut faults = self.faults.lock().unwrap();
        if path.ends_with(SYNC_LATEST_FILE) && *faults > 0 {
            *faults -= 1;
            if self.network {
                return Err(CloudSyncError::Remote(
                    "authentication or network failed".into(),
                ));
            }
            return Ok(Some(b"corrupt".to_vec()));
        }
        self.inner.read_if_exists(path)
    }
}

#[test]
fn pointer_verification_retries_corrupt_reads_without_repeating_put_and_propagates_io_errors() {
    let dir = unique_temp_dir("recovery-readback");
    let options = options(&dir, &dir, "local");
    let store = OptionsTestLocalStore(&options);
    let pointer = pointer("a", "revision", &[("a", 1)]);
    for (faults, network, expected_reads, success) in [
        (1, false, 2, true),
        (4, false, 3, false),
        (4, true, 1, false),
    ] {
        let remote = ReadbackRemote {
            inner: MemoryRemote::default(),
            faults: Mutex::new(faults),
            network,
            reads: AtomicUsize::new(0),
            writes: AtomicUsize::new(0),
        };
        assert_eq!(
            write_sync_pointer(&store, &remote, &options.remote_root, &pointer).is_ok(),
            success
        );
        assert_eq!(remote.writes.load(Ordering::Relaxed), 1);
        assert_eq!(remote.reads.load(Ordering::Relaxed), expected_reads);
    }
    // A valid device head does not turn I/O/authentication errors into emptiness.
    let remote = ReadbackRemote {
        inner: MemoryRemote::default(),
        faults: Mutex::new(10),
        network: true,
        reads: AtomicUsize::new(0),
        writes: AtomicUsize::new(0),
    };
    let bytes =
        serde_json::to_vec(&serde_json::json!({"device_id":"a", "committed":pointer})).unwrap();
    remote
        .inner
        .write(&heads::device_head_path(&options.remote_root, "a"), &bytes)
        .unwrap();
    assert!(probe_remote_sync(&store, &remote, &options.remote_root).is_err());
    assert!(
        push_snapshot_with_remote(&store, &options, &remote, &CloudSyncState::default(), false)
            .is_err()
    );
    assert_eq!(remote.writes.load(Ordering::Relaxed), 0);
    assert!(!store.encode_sync_pointer(&pointer).unwrap().is_empty());
}
