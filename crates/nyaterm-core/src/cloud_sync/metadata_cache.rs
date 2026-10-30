//! Bounded metadata reuse. Every hit is backed by a fresh server validator.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use super::io::{CloudObjectMetadata, CloudObjectRead, CloudSyncIoEvent};
use super::{CloudSyncError, CloudSyncRemote, RemoteFileCapacity};

const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 1_024;

struct Entry {
    path: String,
    bytes: Vec<u8>,
    etag: String,
}

#[derive(Default)]
pub struct CloudMetadataCache {
    entries: VecDeque<Entry>,
    bytes: usize,
}

impl CloudMetadataCache {
    fn remove(&mut self, path: &str) {
        if let Some(index) = self.entries.iter().position(|e| e.path == path) {
            let entry = self.entries.remove(index).expect("entry index");
            self.bytes -= entry.bytes.len() + entry.path.len() + entry.etag.len();
        }
    }
    fn get(&mut self, path: &str) -> Option<(Vec<u8>, String)> {
        let index = self.entries.iter().position(|e| e.path == path)?;
        let entry = self.entries.remove(index)?;
        let value = (entry.bytes.clone(), entry.etag.clone());
        self.entries.push_back(entry);
        Some(value)
    }
    fn insert(&mut self, path: &str, bytes: &[u8], etag: String) {
        self.remove(path);
        let size = bytes.len() + path.len() + etag.len();
        if size > MAX_BYTES || etag.is_empty() || etag.len() > 1_024 {
            return;
        }
        while self.bytes + size > MAX_BYTES || self.entries.len() >= MAX_ENTRIES {
            let entry = self.entries.pop_front().expect("bounded cache");
            self.bytes -= entry.bytes.len() + entry.path.len() + entry.etag.len();
        }
        self.entries.push_back(Entry {
            path: path.into(),
            bytes: bytes.into(),
            etag,
        });
        self.bytes += size;
    }
}

pub struct CachedMetadataRemote<'a> {
    pub inner: &'a dyn CloudSyncRemote,
    pub cache: &'a Mutex<CloudMetadataCache>,
    pub root: &'a str,
    // List validators belong only to this operation, never to a future scan.
    listed: Mutex<HashMap<String, Option<String>>>,
}

impl<'a> CachedMetadataRemote<'a> {
    pub fn new(
        inner: &'a dyn CloudSyncRemote,
        cache: &'a Mutex<CloudMetadataCache>,
        root: &'a str,
    ) -> Self {
        Self {
            inner,
            cache,
            root,
            listed: Mutex::new(HashMap::new()),
        }
    }
    fn metadata(&self, path: &str) -> bool {
        path == super::remote_path(self.root, super::SYNC_LATEST_FILE)
            || path.starts_with(&super::remote_path(self.root, super::heads::HEADS_DIR))
    }
    fn invalidate(&self, path: &str) {
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(path);
        self.listed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(path);
    }
}

impl CloudSyncRemote for CachedMetadataRemote<'_> {
    fn list_sync_files(&self, path: &str) -> Result<Vec<String>, CloudSyncError> {
        self.inner.list_sync_files(path)
    }
    fn provider(&self) -> &'static str {
        self.inner.provider()
    }
    fn file_capacity(&self) -> Result<Option<RemoteFileCapacity>, CloudSyncError> {
        self.inner.file_capacity()
    }
    fn create_dir(&self, path: &str) -> Result<(), CloudSyncError> {
        self.inner.create_dir(path)
    }
    fn observe(&self, event: CloudSyncIoEvent) {
        self.inner.observe(event);
    }
    fn read_if_exists(&self, path: &str) -> Result<Option<Vec<u8>>, CloudSyncError> {
        if !self.metadata(path) {
            return self.inner.read_if_exists(path);
        }
        let cached = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(path);
        let listed = self
            .listed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(path)
            .cloned();
        if let (Some((bytes, etag)), Some(Some(current))) = (&cached, &listed)
            && etag == current
        {
            self.observe(CloudSyncIoEvent::CacheHit);
            return Ok(Some(bytes.clone()));
        }
        // A missing validator in a fresh listing requires an unconditional read.
        let etag = if listed == Some(None) {
            None
        } else {
            cached.as_ref().map(|(_, etag)| etag.as_str())
        };
        let mut result = self.inner.read_conditional(path, etag)?;
        if matches!(result, CloudObjectRead::NotModified) {
            if etag.is_some()
                && let Some((bytes, _)) = cached
            {
                self.observe(CloudSyncIoEvent::CacheHit);
                return Ok(Some(bytes));
            }
            result = self.inner.read_conditional(path, None)?;
        }
        self.invalidate(path);
        match result {
            CloudObjectRead::Missing => Ok(None),
            CloudObjectRead::Modified { bytes, etag } => {
                if let Some(etag) = etag {
                    self.cache
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .insert(path, &bytes, etag);
                }
                Ok(Some(bytes))
            }
            CloudObjectRead::NotModified => Err(CloudSyncError::Remote(
                "conditional read returned no object".into(),
            )),
        }
    }
    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        self.invalidate(path);
        self.inner.write(path, bytes)
    }
    fn write_sync_file(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        self.invalidate(path);
        self.inner.write_sync_file(path, bytes)
    }
    fn delete(&self, path: &str) -> Result<(), CloudSyncError> {
        self.invalidate(path);
        self.inner.delete(path)
    }
    fn list_files(&self, path: &str) -> Result<Vec<String>, CloudSyncError> {
        Ok(self
            .list_metadata(path)?
            .into_iter()
            .map(|entry| entry.path)
            .collect())
    }
    fn list_metadata(&self, path: &str) -> Result<Vec<CloudObjectMetadata>, CloudSyncError> {
        let entries = self.inner.list_metadata(path)?;
        let mut listed = self
            .listed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        listed.retain(|key, _| !key.starts_with(path));
        for entry in entries
            .iter()
            .filter(|e| self.metadata(&e.path))
            .take(MAX_ENTRIES)
        {
            listed.insert(entry.path.clone(), entry.etag.clone());
        }
        // Objects omitted by a new listing must not retain a previous receipt.
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let removed: Vec<_> = cache
            .entries
            .iter()
            .filter(|e| e.path.starts_with(path) && !listed.contains_key(&e.path))
            .map(|e| e.path.clone())
            .collect();
        for path in removed {
            cache.remove(&path);
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::{CachedMetadataRemote, CloudMetadataCache, MAX_BYTES, MAX_ENTRIES};
    use crate::cloud_sync::io::{CloudObjectMetadata, CloudObjectRead};
    use crate::cloud_sync::{CloudSyncError, CloudSyncRemote};
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Remote {
        objects: RefCell<HashMap<String, (Vec<u8>, String)>>,
        reads: Cell<usize>,
        bodies: Cell<usize>,
        validations: Cell<usize>,
        not_modified: Cell<usize>,
        missing_etag: Cell<bool>,
        stray_304: Cell<bool>,
        cancel_write: Cell<bool>,
    }
    impl CloudSyncRemote for Remote {
        fn provider(&self) -> &'static str {
            "memory"
        }
        fn create_dir(&self, _: &str) -> Result<(), CloudSyncError> {
            Ok(())
        }
        fn read_if_exists(&self, path: &str) -> Result<Option<Vec<u8>>, CloudSyncError> {
            self.reads.set(self.reads.get() + 1);
            let bytes = self.objects.borrow().get(path).map(|(b, _)| b.clone());
            self.bodies
                .set(self.bodies.get() + bytes.as_ref().map_or(0, Vec::len));
            Ok(bytes)
        }
        fn observe(&self, event: super::CloudSyncIoEvent) {
            if matches!(event, super::CloudSyncIoEvent::FullValidation) {
                self.validations.set(self.validations.get() + 1);
            }
        }
        fn read_conditional(
            &self,
            path: &str,
            etag: Option<&str>,
        ) -> Result<CloudObjectRead, CloudSyncError> {
            self.reads.set(self.reads.get() + 1);
            if self.stray_304.replace(false) {
                return Ok(CloudObjectRead::NotModified);
            }
            let objects = self.objects.borrow();
            let Some((bytes, current)) = objects.get(path) else {
                return Ok(CloudObjectRead::Missing);
            };
            if etag == Some(current) {
                self.not_modified.set(self.not_modified.get() + 1);
                return Ok(CloudObjectRead::NotModified);
            }
            self.bodies.set(self.bodies.get() + bytes.len());
            Ok(CloudObjectRead::Modified {
                bytes: bytes.clone(),
                etag: (!self.missing_etag.get()).then(|| current.clone()),
            })
        }
        fn write(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
            if self.cancel_write.replace(false) {
                return Err(CloudSyncError::Cancelled);
            }
            self.objects.borrow_mut().insert(
                path.into(),
                (
                    bytes.into(),
                    format!("\"{}\"", crate::cloud_sync::protocol::sha256_hex(bytes)),
                ),
            );
            Ok(())
        }
        fn delete(&self, path: &str) -> Result<(), CloudSyncError> {
            self.objects.borrow_mut().remove(path);
            Ok(())
        }
        fn list_files(&self, path: &str) -> Result<Vec<String>, CloudSyncError> {
            Ok(self
                .objects
                .borrow()
                .keys()
                .filter(|p| p.starts_with(path))
                .cloned()
                .collect())
        }
        fn list_metadata(&self, path: &str) -> Result<Vec<CloudObjectMetadata>, CloudSyncError> {
            Ok(self
                .objects
                .borrow()
                .iter()
                .filter(|(p, _)| p.starts_with(path))
                .map(|(path, (bytes, etag))| CloudObjectMetadata {
                    path: path.clone(),
                    etag: (!self.missing_etag.get()).then(|| etag.clone()),
                    size: Some(bytes.len() as u64),
                })
                .collect())
        }
    }

    #[test]
    fn validators_reuse_only_current_objects_and_write_readback_is_unconditional() {
        let source = Remote::default();
        let cache = Mutex::new(CloudMetadataCache::default());
        let path = "root/sync/latest.redb";
        source.write(path, b"old").unwrap();
        for _ in 0..2 {
            let remote = CachedMetadataRemote::new(&source, &cache, "root");
            assert_eq!(remote.read_if_exists(path).unwrap().unwrap(), b"old");
        }
        assert_eq!(source.bodies.get(), 3);
        assert_eq!(source.not_modified.get(), 1);
        let remote = CachedMetadataRemote::new(&source, &cache, "root");
        remote.write(path, b"new-body").unwrap();
        assert_eq!(remote.read_if_exists(path).unwrap().unwrap(), b"new-body");
        assert_eq!(source.not_modified.get(), 1);
        remote.delete(path).unwrap();
        assert!(remote.read_if_exists(path).unwrap().is_none());
        source.write(path, b"returning").unwrap();
        assert!(remote.read_if_exists(path).unwrap().is_some());
    }

    #[test]
    fn fresh_head_listing_detects_new_and_removed_branches_and_missing_validators() {
        let source = Remote::default();
        let cache = Mutex::new(CloudMetadataCache::default());
        let prefix = "root/sync/heads/";
        let path = format!("{prefix}a.json");
        source.write(&path, b"a").unwrap();
        for _ in 0..2 {
            let remote = CachedMetadataRemote::new(&source, &cache, "root");
            for path in remote.list_files(prefix).unwrap() {
                remote.read_if_exists(&path).unwrap();
            }
        }
        assert_eq!(source.reads.get(), 1);
        source.write(&format!("{prefix}b.json"), b"bb").unwrap();
        let remote = CachedMetadataRemote::new(&source, &cache, "root");
        for path in remote.list_files(prefix).unwrap() {
            remote.read_if_exists(&path).unwrap();
        }
        assert_eq!(source.reads.get(), 2);
        source.delete(&path).unwrap();
        remote.list_files(prefix).unwrap();
        assert!(cache.lock().unwrap().get(&path).is_none());
        source.missing_etag.set(true);
        for _ in 0..2 {
            for path in remote.list_files(prefix).unwrap() {
                remote.read_if_exists(&path).unwrap();
            }
        }
        assert_eq!(source.reads.get(), 4);
    }

    #[test]
    fn interrupted_publication_invalidates_metadata_before_remote_write() {
        let source = Remote::default();
        let cache = Mutex::new(CloudMetadataCache::default());
        let path = "root/sync/latest.redb";
        source.write(path, b"old").unwrap();
        let remote = CachedMetadataRemote::new(&source, &cache, "root");
        remote.read_if_exists(path).unwrap();
        source.cancel_write.set(true);
        assert!(matches!(
            remote.write(path, b"new"),
            Err(CloudSyncError::Cancelled)
        ));
        assert!(cache.lock().unwrap().get(path).is_none());
        assert_eq!(remote.read_if_exists(path).unwrap().unwrap(), b"old");
        assert_eq!(
            source.not_modified.get(),
            0,
            "publication readback requires a body after an interrupted write"
        );
    }

    #[test]
    fn unsolicited_not_modified_retries_without_a_validator_and_lru_is_bounded() {
        let source = Remote::default();
        let cache = Mutex::new(CloudMetadataCache::default());
        let path = "root/sync/latest.redb";
        source.write(path, b"data").unwrap();
        source.stray_304.set(true);
        assert_eq!(
            CachedMetadataRemote::new(&source, &cache, "root")
                .read_if_exists(path)
                .unwrap()
                .unwrap(),
            b"data"
        );
        assert_eq!(source.reads.get(), 2);
        // A LIST without a validator must not reuse an older receipt, even if
        // a server incorrectly sends 304 for our unconditional request.
        let head = "root/sync/heads/a.json";
        source.write(head, b"old").unwrap();
        let remote = CachedMetadataRemote::new(&source, &cache, "root");
        remote.read_if_exists(head).unwrap();
        source.write(head, b"new").unwrap();
        source.missing_etag.set(true);
        remote.list_files("root/sync/heads/").unwrap();
        source.stray_304.set(true);
        assert_eq!(remote.read_if_exists(head).unwrap().unwrap(), b"new");
        let mut cache = CloudMetadataCache::default();
        for i in 0..MAX_ENTRIES + 2 {
            cache.insert(&i.to_string(), b"x", "tag".into());
        }
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
        assert!(cache.get("0").is_none());
        cache.get("2");
        cache.insert("extra", b"x", "tag".into());
        assert!(cache.get("2").is_some());
        assert!(cache.get("3").is_none());
        cache.insert("huge", &vec![0; MAX_BYTES], "tag".into());
        assert!(cache.get("huge").is_none());
        assert!(cache.bytes <= MAX_BYTES);
    }

    #[test]
    fn thirty_day_three_device_fixture_reduces_downloads_by_at_least_ninety_five_percent() {
        use crate::RawPortableSnapshot;
        use crate::cloud_sync::cache::CloudSyncCheckCache;
        use crate::cloud_sync::{
            CloudLocalStore, CloudSyncBackupInfo, CloudSyncState, LocalCloudSyncOptions,
            RemoteSyncPointer, check_snapshot_with_remote_cached, push_snapshot_with_remote,
        };
        struct Store;
        impl CloudLocalStore for Store {
            fn encode_sync_snapshot(
                &self,
                snapshot: &RawPortableSnapshot,
                _: &str,
            ) -> Result<Vec<u8>, CloudSyncError> {
                let mut bytes = serde_json::to_vec(snapshot).unwrap();
                bytes.resize(1_048_576, b' ');
                Ok(bytes)
            }
            fn decode_sync_snapshot(
                &self,
                bytes: &[u8],
                _: &str,
            ) -> Result<RawPortableSnapshot, CloudSyncError> {
                Ok(serde_json::from_slice(bytes)?)
            }
            fn encode_sync_pointer(
                &self,
                p: &RemoteSyncPointer,
            ) -> Result<Vec<u8>, CloudSyncError> {
                let mut bytes = serde_json::to_vec(p).unwrap();
                bytes.resize(1_056_768, b' ');
                Ok(bytes)
            }
            fn decode_sync_pointer(&self, b: &[u8]) -> Result<RemoteSyncPointer, CloudSyncError> {
                Ok(serde_json::from_slice(b)?)
            }
            fn build_sync_snapshot(
                &self,
                o: &LocalCloudSyncOptions,
            ) -> Result<RawPortableSnapshot, CloudSyncError> {
                let mut s = RawPortableSnapshot::sync(&o.device_id, "test");
                s.recalculate_hash()?;
                Ok(s)
            }
            fn apply_sync_snapshot(
                &self,
                _: &LocalCloudSyncOptions,
                _: &RawPortableSnapshot,
            ) -> Result<CloudSyncBackupInfo, CloudSyncError> {
                unreachable!("unchanged fixture never applies data")
            }
            fn persist_cloud_sync_state(&self, _: &CloudSyncState) -> Result<(), CloudSyncError> {
                Ok(())
            }
        }
        let mut options = LocalCloudSyncOptions {
            config_dir: "fixture".into(),
            portable_key_path: None,
            remote_dir: "fixture".into(),
            remote_root: "root".into(),
            device_id: "a".into(),
            device_name: None,
            app_version: "test".into(),
            master_password: "fixture".into(),
            enabled: true,
        };
        let source = Remote::default();
        let mut state = CloudSyncState::default();
        for device in ["a", "b", "c"] {
            options.device_id = device.into();
            state = push_snapshot_with_remote(&Store, &options, &source, &state, true)
                .unwrap()
                .state;
        }
        source.bodies.set(0);
        source.reads.set(0);
        source.validations.set(0);
        let metadata = Mutex::new(CloudMetadataCache::default());
        let mut validation = CloudSyncCheckCache::default();
        for day in 0..30_u64 {
            for check in 0..64_u64 {
                validation.now = Some(1_000 + day * 86_400_000 + check * 900_000);
                let before = source.bodies.get();
                let remote = CachedMetadataRemote::new(&source, &metadata, "root");
                let result = check_snapshot_with_remote_cached(
                    &Store,
                    &options,
                    &remote,
                    &state,
                    true,
                    &mut validation,
                )
                .unwrap();
                assert_eq!(
                    result.decision,
                    crate::cloud_sync::CloudRemoteCheckDecision::UpToDate
                );
                state = result.state;
                if check != 0 {
                    assert_eq!(
                        source.bodies.get(),
                        before,
                        "warm checks return no object bodies"
                    );
                }
            }
        }
        let full = source.validations.get();
        assert_eq!(full, 30);
        assert_eq!(source.not_modified.get(), 1_919);
        let baseline = 1_920_u64 * (1_056_768 + 1_048_576);
        let optimized = source.bodies.get() as u64;
        eprintln!(
            "cloud sync bandwidth fixture: baseline_bytes={baseline}, optimized_bytes={optimized}, full_validations={full}"
        );
        assert!(
            optimized * 100 < baseline * 5,
            "baseline={baseline} optimized={optimized}"
        );
        assert!(optimized < 34 * 1_048_576);
    }
}
