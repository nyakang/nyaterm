//! Process-owned receipts shared by all windows and freshly constructed adapters.

use std::cell::Cell;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use nyaterm_core::cloud_sync::RemoteFileCapacity;
use nyaterm_core::cloud_sync::cache::CloudSyncCheckCache;
use nyaterm_core::cloud_sync::io::{
    CloudObjectMetadata, CloudObjectRead, CloudSyncIoEvent, CloudSyncIoStats,
};
use nyaterm_core::cloud_sync::metadata_cache::CloudMetadataCache;
use nyaterm_core::{CloudSyncError, CloudSyncRemote, CloudSyncSettings, LocalCloudSyncOptions};
use nyaterm_store::{StoreBlockingClient, StoreDomain};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

#[derive(Default)]
pub(super) struct IoCaches {
    pub metadata: Mutex<CloudMetadataCache>,
    pub validation: Mutex<CloudSyncCheckCache>,
    pub pending_remote: std::sync::atomic::AtomicBool,
}

pub(crate) struct CloudSyncIoRuntime {
    salt: [u8; 32],
    current: Mutex<Option<([u8; 32], Arc<IoCaches>)>>,
    last_check: Mutex<Option<Instant>>,
}

impl CloudSyncIoRuntime {
    pub(crate) fn shared() -> &'static Self {
        static RUNTIME: OnceLock<CloudSyncIoRuntime> = OnceLock::new();
        RUNTIME.get_or_init(|| Self {
            salt: rand::random(),
            current: Mutex::new(None),
            last_check: Mutex::new(None),
        })
    }
    pub(super) fn caches(
        &self,
        store: &StoreBlockingClient,
        settings: &CloudSyncSettings,
        options: Option<&LocalCloudSyncOptions>,
    ) -> Result<Arc<IoCaches>, CloudSyncError> {
        let (path, saved_password) = store
            .request_fn(StoreDomain::CloudSync, |store| {
                Ok((
                    store.db_path().to_path_buf(),
                    store.load_cloud_sync_master_password()?,
                ))
            })
            .map_err(|_| CloudSyncError::CredentialPersistence)?;
        let encoded = Zeroizing::new(
            serde_json::to_vec(settings).map_err(|_| CloudSyncError::CredentialPersistence)?,
        );
        let mut hash = Sha256::new();
        hash.update(self.salt);
        for value in [
            path.as_os_str().as_encoded_bytes(),
            encoded.as_slice(),
            options
                .map(|o| o.master_password.expose_secret())
                .or_else(|| saved_password.as_ref().map(|p| p.expose_secret()))
                .unwrap_or_default()
                .as_bytes(),
        ] {
            hash.update((value.len() as u64).to_le_bytes());
            hash.update(value);
        }
        let key: [u8; 32] = hash.finalize().into();
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((old, cache)) = &*current
            && old == &key
        {
            return Ok(cache.clone());
        }
        let cache = Arc::new(IoCaches::default());
        *current = Some((key, cache.clone()));
        *self
            .last_check
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        Ok(cache)
    }
    pub(crate) fn last_check(&self) -> Option<Instant> {
        *self
            .last_check
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub(super) fn checked(&self) {
        *self
            .last_check
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Instant::now());
    }
}

pub(crate) struct IoScope {
    pub(super) stats: Arc<Mutex<CloudSyncIoStats>>,
    trigger: &'static str,
    root: bool,
    outcome: &'static str,
}

impl IoScope {
    pub(crate) fn new(trigger: &'static str) -> Self {
        let (stats, root) = crate::http::cloud_sync::metrics::begin_scope();
        Self {
            stats,
            trigger,
            root,
            outcome: "interrupted",
        }
    }
    pub(crate) fn finish<T>(&mut self, result: &Result<T, CloudSyncError>) {
        self.outcome = result
            .as_ref()
            .map_or_else(|e| e.classification(), |_| "success");
    }

    pub(super) fn skipped(&mut self, reason: &'static str) {
        self.outcome = reason;
    }
}

impl Drop for IoScope {
    fn drop(&mut self) {
        if !self.root {
            return;
        }
        crate::http::cloud_sync::metrics::end_scope();
        let stats = *self
            .stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tracing::info!(
            trigger = self.trigger,
            outcome = self.outcome,
            requests = stats.requests,
            uploaded_bytes = stats.uploaded_bytes,
            downloaded_bytes = stats.downloaded_bytes,
            not_modified = stats.not_modified,
            cache_hits = stats.cache_hits,
            full_validations = stats.full_validations,
            "Cloud sync application-body I/O summary (excludes headers and TLS)"
        );
    }
}

pub(super) struct ObservedRemote<'a> {
    pub inner: &'a dyn CloudSyncRemote,
    pub stats: Arc<Mutex<CloudSyncIoStats>>,
    pub heads_checked: Cell<bool>,
}

impl CloudSyncRemote for ObservedRemote<'_> {
    fn write_sync_file(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        self.inner.write_sync_file(path, bytes)
    }
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
    fn read_if_exists(&self, path: &str) -> Result<Option<Vec<u8>>, CloudSyncError> {
        self.inner.read_if_exists(path)
    }
    fn read_conditional(
        &self,
        path: &str,
        etag: Option<&str>,
    ) -> Result<CloudObjectRead, CloudSyncError> {
        self.inner.read_conditional(path, etag)
    }
    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        self.inner.write(path, bytes)
    }
    fn delete(&self, path: &str) -> Result<(), CloudSyncError> {
        self.inner.delete(path)
    }
    fn list_files(&self, path: &str) -> Result<Vec<String>, CloudSyncError> {
        Ok(self
            .list_metadata(path)?
            .into_iter()
            .map(|e| e.path)
            .collect())
    }
    fn list_metadata(&self, path: &str) -> Result<Vec<CloudObjectMetadata>, CloudSyncError> {
        self.inner.list_metadata(path)
    }
    fn observe(&self, event: CloudSyncIoEvent) {
        let mut s = self
            .stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match event {
            CloudSyncIoEvent::FullValidation => s.full_validations += 1,
            CloudSyncIoEvent::CacheHit => s.cache_hits += 1,
            CloudSyncIoEvent::HeadsChecked => self.heads_checked.set(true),
        }
        self.inner.observe(event);
    }
}

#[cfg(test)]
mod tests {
    use super::{CloudSyncIoRuntime, IoScope};
    use crate::test_support::{TestConfigDir, blocking_test_store};
    use nyaterm_core::{CloudSyncSettings, LocalCloudSyncOptions};
    use std::sync::{Arc, Mutex};

    #[test]
    fn cloud_sync_cache_context_is_shared_and_isolates_accounts_passwords_roots_and_databases() {
        let dir = TestConfigDir::new("nyaterm-cloud-sync-io-context");
        let store = blocking_test_store(dir.path());
        let runtime = CloudSyncIoRuntime {
            salt: [7; 32],
            current: Mutex::new(None),
            last_check: Mutex::new(None),
        };
        let mut settings = CloudSyncSettings {
            provider: "s3".into(),
            ..Default::default()
        };
        settings.s3.access_key_id = Some("fixture-account".into());
        let mut options = LocalCloudSyncOptions {
            config_dir: dir.join("config"),
            portable_key_path: None,
            remote_dir: dir.join("remote"),
            remote_root: settings.remote_root.clone(),
            device_id: "fixture".into(),
            device_name: None,
            app_version: "test".into(),
            master_password: "fixture-password".into(),
            enabled: true,
        };
        let first = runtime.caches(&store, &settings, Some(&options)).unwrap();
        assert!(Arc::ptr_eq(
            &first,
            &runtime.caches(&store, &settings, Some(&options)).unwrap()
        ));
        runtime.checked();
        assert!(runtime.last_check().is_some());
        options.master_password = "different-password".into();
        let password = runtime.caches(&store, &settings, Some(&options)).unwrap();
        assert!(!Arc::ptr_eq(&first, &password));
        assert!(runtime.last_check().is_none());
        settings.s3.access_key_id = Some("different-account".into());
        let account = runtime.caches(&store, &settings, Some(&options)).unwrap();
        assert!(!Arc::ptr_eq(&password, &account));
        settings.s3.root = "different-root".into();
        let root = runtime.caches(&store, &settings, Some(&options)).unwrap();
        assert!(!Arc::ptr_eq(&account, &root));
        let second = TestConfigDir::new("nyaterm-cloud-sync-io-other-db");
        let other = blocking_test_store(second.path());
        assert!(!Arc::ptr_eq(
            &root,
            &runtime.caches(&other, &settings, Some(&options)).unwrap()
        ));
        let cold = CloudSyncIoRuntime {
            salt: [8; 32],
            current: Mutex::new(None),
            last_check: Mutex::new(None),
        };
        assert!(!Arc::ptr_eq(
            &first,
            &cold.caches(&store, &settings, Some(&options)).unwrap()
        ));
    }

    #[test]
    fn cloud_sync_nested_io_scopes_aggregate_without_exposing_request_data() {
        let outer = IoScope::new("fixture");
        {
            let inner = IoScope::new("check");
            assert!(Arc::ptr_eq(&outer.stats, &inner.stats));
            inner.stats.lock().unwrap().downloaded_bytes += 123;
        }
        assert_eq!(outer.stats.lock().unwrap().downloaded_bytes, 123);
        let debug = format!("{:?}", outer.stats.lock().unwrap());
        assert!(!debug.contains("fixture-password"));
    }
}
