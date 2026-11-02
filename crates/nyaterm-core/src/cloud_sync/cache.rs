//! In-memory authentication receipts; no decrypted entity documents are retained.

use super::conflict::{CloudConflictBranch, summarize};
use super::{RemoteSyncPointer, current_time_ms};
use crate::RawPortableSnapshot;
use std::collections::VecDeque;

pub const FULL_VALIDATION_INTERVAL_MS: u64 = 86_400_000;

#[derive(Clone)]
pub(super) struct ValidatedSnapshot {
    pointer: RemoteSyncPointer,
    pub content_hash: String,
    pub validated_at_ms: u64,
    local_content_hash: String,
    pub remote_has_user_content: bool,
    pub branch: CloudConflictBranch,
}

pub struct CloudSyncCheckCache {
    salt: [u8; 32],
    context: Option<[u8; 32]>,
    preview_set: Vec<RemoteSyncPointer>,
    entries: VecDeque<ValidatedSnapshot>,
    // Tests can advance the clock without waiting or changing production policy.
    #[cfg(test)]
    pub(super) now: Option<u64>,
}

impl Default for CloudSyncCheckCache {
    fn default() -> Self {
        Self {
            salt: rand::random(),
            context: None,
            preview_set: Vec::new(),
            entries: VecDeque::new(),
            #[cfg(test)]
            now: None,
        }
    }
}

impl CloudSyncCheckCache {
    pub(super) fn scope(&mut self, options: &super::LocalCloudSyncOptions, provider: &str) {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        hash.update(self.salt);
        for value in [
            options.config_dir.as_os_str().as_encoded_bytes(),
            options.remote_dir.as_os_str().as_encoded_bytes(),
            options.remote_root.as_bytes(),
            options.master_password.expose_secret().as_bytes(),
            provider.as_bytes(),
        ] {
            hash.update((value.len() as u64).to_le_bytes());
            hash.update(value);
        }
        if let Some(path) = &options.portable_key_path {
            hash.update(path.as_os_str().as_encoded_bytes());
        }
        let context: [u8; 32] = hash.finalize().into();
        if self.context != Some(context) {
            self.context = Some(context);
            self.entries.clear();
            self.preview_set.clear();
        }
    }
    pub(super) fn prepare_preview(&mut self, pointers: &[RemoteSyncPointer]) {
        if self.preview_set != pointers {
            self.preview_set = pointers.to_vec();
            for entry in &mut self.entries {
                entry.local_content_hash.clear();
            }
        }
    }

    fn now(&self) -> u64 {
        #[cfg(test)]
        if let Some(now) = self.now {
            return now;
        }
        current_time_ms()
    }

    pub(super) fn get(&mut self, pointer: &RemoteSyncPointer) -> Option<ValidatedSnapshot> {
        let now = self.now();
        self.entries.retain(|e| {
            now >= e.validated_at_ms && now - e.validated_at_ms < FULL_VALIDATION_INTERVAL_MS
        });
        let index = self.entries.iter().position(|e| &e.pointer == pointer)?;
        let entry = self.entries.remove(index)?;
        self.entries.push_back(entry.clone());
        Some(entry)
    }

    pub(super) fn branch(
        &mut self,
        pointer: &RemoteSyncPointer,
        local_hash: &str,
    ) -> Option<CloudConflictBranch> {
        self.get(pointer)
            .filter(|e| e.local_content_hash == local_hash)
            .map(|e| e.branch)
    }

    pub(super) fn remove(&mut self, pointer: &RemoteSyncPointer) {
        self.entries
            .retain(|e| e.pointer.revision_id != pointer.revision_id);
    }

    pub(super) fn record(
        &mut self,
        pointer: &RemoteSyncPointer,
        snapshot: &RawPortableSnapshot,
        local: &RawPortableSnapshot,
    ) -> Result<(), super::CloudSyncError> {
        self.remove(pointer);
        self.entries.push_back(ValidatedSnapshot {
            pointer: pointer.clone(),
            content_hash: snapshot.sync_content_hash()?,
            validated_at_ms: self.now(),
            local_content_hash: local.sync_content_hash()?,
            remote_has_user_content: snapshot.has_user_content(),
            branch: CloudConflictBranch {
                revision: pointer.revision_id.clone(),
                device_id: pointer.device_id.clone(),
                device_name: snapshot.meta.device_name.clone(),
                created_at_ms: pointer.created_at_ms,
                readable: true,
                differences: summarize(local, snapshot),
            },
        });
        while self.entries.len() > 256 {
            self.entries.pop_front();
        }
        Ok(())
    }
}
