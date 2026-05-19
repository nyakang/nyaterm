//! Backend preference cache: remembers which remote file system backend
//! worked for each host so subsequent connections skip failed probes.

use crate::storage;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub(crate) const JSON_FILE_BACKEND_CACHE: &str = "file-backend-cache";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct FileBackendCache {
    #[serde(default)]
    pub entries: HashMap<String, FileBackendCacheEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FileBackendCacheEntry {
    pub last_working_backend: String,
    #[serde(default)]
    pub sftp_unavailable: bool,
    #[serde(default)]
    pub last_failure_reason: Option<String>,
    #[serde(default)]
    pub updated_at: u64,
}

/// Build the cache lookup key from SSH connection params.
pub(crate) fn cache_key(host: &str, port: u16, username: &str) -> String {
    format!("{}:{}:{}", host, port, username)
}

pub(crate) fn load_cached_backend(key: &str) -> Option<String> {
    let cache: FileBackendCache = storage::load_json_doc(JSON_FILE_BACKEND_CACHE).ok()?;
    cache
        .entries
        .get(key)
        .map(|e| e.last_working_backend.clone())
}

pub(crate) fn save_cached_backend(
    key: &str,
    backend: &str,
    sftp_unavailable: bool,
    failure_reason: Option<String>,
) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let _ = storage::update_json_doc::<FileBackendCache, (), _>(JSON_FILE_BACKEND_CACHE, |cache| {
        cache.entries.insert(
            key.to_string(),
            FileBackendCacheEntry {
                last_working_backend: backend.to_string(),
                sftp_unavailable,
                last_failure_reason: failure_reason,
                updated_at: now,
            },
        );
        Ok(())
    });
}
