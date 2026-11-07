//! Secret-free conflict inspection and revision-pinned, explicit resolution.

use super::control::CloudSyncOperationControl;
use super::{
    CloudLocalStore, CloudSyncError, CloudSyncRemote, CloudSyncResult, CloudSyncState,
    LocalCloudSyncOptions, PreparedSync, PullExecution, RemoteFileCapacity, conflict_preview,
    heads, protocol, pull_snapshot_prepared, push_snapshot_prepared,
};
use crate::RawPortableSnapshot;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloudEntityDiff {
    pub entity: String,
    pub added: usize,
    pub removed: usize,
    pub modified: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloudConflictBranch {
    pub revision: String,
    pub device_id: String,
    pub device_name: Option<String>,
    pub created_at_ms: u64,
    pub readable: bool,
    pub differences: Vec<CloudEntityDiff>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudResolutionRequest {
    pub observed_revisions: Vec<String>,
    pub selected_revision: Option<String>,
    pub local_content_hash: String,
}

fn records(
    value: &serde_json::Value,
    scope: &str,
    output: &mut BTreeMap<String, serde_json::Value>,
) {
    match value {
        serde_json::Value::Object(map) => {
            let scope =
                if let Some(id) = map.get("id").filter(|id| id.is_string() || id.is_number()) {
                    let identity = format!("{scope}/{id}");
                    output.insert(identity.clone(), value.clone());
                    identity
                } else {
                    scope.to_string()
                };
            for (field, child) in map {
                records(child, &format!("{scope}/{field}"), output);
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                records(child, scope, output);
            }
        }
        _ => {}
    }
}

/// Compare portable records, never returning their names, values, or secrets.
pub fn summarize(
    local: &RawPortableSnapshot,
    remote: &RawPortableSnapshot,
) -> Vec<CloudEntityDiff> {
    let (Ok(local), Ok(remote)) = (
        crate::portable_snapshot::project_fixed_sync_snapshot(local),
        crate::portable_snapshot::project_fixed_sync_snapshot(remote),
    ) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    let keys: BTreeSet<_> = local
        .entities
        .keys()
        .chain(remote.entities.keys())
        .collect();
    let mut extension_changed = false;
    for key in keys {
        if key == "fixed_sync_complete_kinds" {
            continue;
        }
        let (before, after) = (local.entities.get(key), remote.entities.get(key));
        if before == after {
            continue;
        }
        if key == "settings" {
            let before: serde_json::Value = before
                .and_then(|v| serde_json::from_str(v).ok())
                .unwrap_or_default();
            let after: serde_json::Value = after
                .and_then(|v| serde_json::from_str(v).ok())
                .unwrap_or_default();
            for category in [
                "appearance",
                "terminal",
                "security",
                "ai",
                "translation",
                "transfer",
                "connection",
                "general",
                "ui",
                "proxy",
                "search",
                "interaction",
                "diagnostics",
            ] {
                if before.get(category) != after.get(category) {
                    result.push(CloudEntityDiff {
                        entity: format!("settings.{category}"),
                        added: 0,
                        removed: 0,
                        modified: 1,
                    });
                }
            }
            // A generic settings marker also accounts for unknown categories.
            result.push(CloudEntityDiff {
                entity: "settings".into(),
                added: 0,
                removed: 0,
                modified: 1,
            });
            continue;
        }
        if !matches!(
            key.as_str(),
            "sessions"
                | "keys"
                | "passwords"
                | "credentials"
                | "otp"
                | "proxies"
                | "proxy_groups"
                | "tunnels"
                | "tunnel_groups"
                | "quick_commands"
                | "known_hosts"
                | "vnc_known_hosts"
                | "notes"
                | "master_key_token"
                | "history"
        ) {
            extension_changed = true;
            continue;
        }
        let mut left = BTreeMap::new();
        let mut right = BTreeMap::new();
        if let Some(value) = before.and_then(|v| serde_json::from_str(v).ok()) {
            records(&value, "", &mut left);
        }
        if let Some(value) = after.and_then(|v| serde_json::from_str(v).ok()) {
            records(&value, "", &mut right);
        }
        let mut diff = CloudEntityDiff {
            entity: key.clone(),
            added: right.keys().filter(|id| !left.contains_key(*id)).count(),
            removed: left.keys().filter(|id| !right.contains_key(*id)).count(),
            modified: right
                .iter()
                .filter(|(id, value)| left.get(*id).is_some_and(|old| old != *value))
                .count(),
        };
        if left.is_empty() && right.is_empty() {
            diff.modified = 1;
        }
        result.push(diff);
    }
    if extension_changed {
        result.push(CloudEntityDiff {
            entity: "extensions".into(),
            added: 0,
            removed: 0,
            modified: 1,
        });
    }
    result
}

pub(super) fn with_details<T>(
    result: Result<T, CloudSyncError>,
    store: &dyn CloudLocalStore,
    remote: &dyn CloudSyncRemote,
    options: &LocalCloudSyncOptions,
) -> Result<T, CloudSyncError> {
    details(result, store, remote, options, None)
}

pub(super) fn with_cached_details<T>(
    result: Result<T, CloudSyncError>,
    store: &dyn CloudLocalStore,
    remote: &dyn CloudSyncRemote,
    options: &LocalCloudSyncOptions,
    cache: &mut super::cache::CloudSyncCheckCache,
) -> Result<T, CloudSyncError> {
    details(result, store, remote, options, Some(cache))
}

fn details<T>(
    result: Result<T, CloudSyncError>,
    store: &dyn CloudLocalStore,
    remote: &dyn CloudSyncRemote,
    options: &LocalCloudSyncOptions,
    mut cache: Option<&mut super::cache::CloudSyncCheckCache>,
) -> Result<T, CloudSyncError> {
    result.map_err(|error| {
        let CloudSyncError::Conflict(mut preview) = error else {
            return error;
        };
        if let (Ok(local), Ok(heads)) = (
            store.build_sync_snapshot(options),
            heads::read(store, remote, &options.remote_root),
        ) {
            preview.local_content_hash = local.sync_content_hash().ok();
            preview.observed_revisions = heads.observed_revisions();
            let pointers = heads.maximal();
            if let Some(cache) = cache.as_deref_mut() {
                cache.prepare_preview(&pointers);
            }
            preview.branches = pointers
                .into_iter()
                .map(|pointer| {
                    if let Some(branch) = cache.as_deref_mut().and_then(|cache| {
                        cache.branch(
                            &pointer,
                            preview.local_content_hash.as_deref().unwrap_or_default(),
                        )
                    }) {
                        remote.observe(super::io::CloudSyncIoEvent::CacheHit);
                        return branch;
                    }
                    if let Some(cache) = cache.as_deref_mut() {
                        cache.remove(&pointer);
                    }
                    let decoded =
                        protocol::read_snapshot_for_pointer(store, remote, options, &pointer);
                    if let Err(error) = &decoded {
                        preview.overwrite_blocked |= blocks_overwrite(error);
                    }
                    let snapshot = decoded.ok();
                    if let (Some(snapshot), Some(cache)) = (&snapshot, cache.as_deref_mut()) {
                        let _ = cache.record(&pointer, snapshot, &local);
                    }
                    CloudConflictBranch {
                        revision: pointer.revision_id,
                        device_id: pointer.device_id,
                        device_name: snapshot.as_ref().and_then(|s| s.meta.device_name.clone()),
                        created_at_ms: pointer.created_at_ms,
                        readable: snapshot.is_some(),
                        differences: snapshot
                            .as_ref()
                            .map_or_else(Vec::new, |s| summarize(&local, s)),
                    }
                })
                .collect();
        }
        CloudSyncError::Conflict(preview)
    })
}

fn blocks_overwrite(error: &CloudSyncError) -> bool {
    matches!(error,
        CloudSyncError::RemoteSnapshotUnreadable { cause, .. } if cause.blocks_remote_overwrite()
    ) || matches!(
        error,
        CloudSyncError::PortableSnapshot(
            crate::PortableSnapshotError::MissingMasterPassword
                | crate::PortableSnapshotError::UnsupportedEncryptionVersion(_)
                | crate::PortableSnapshotError::InvalidEncryptionEnvelope
                | crate::PortableSnapshotError::UnsupportedVersion(_)
        )
    )
}

fn changed_preview(
    store: &dyn CloudLocalStore,
    remote: &dyn CloudSyncRemote,
    options: &LocalCloudSyncOptions,
) -> CloudSyncError {
    let Ok(local) = store.build_sync_snapshot(options) else {
        return CloudSyncError::LocalChangedDuringSync;
    };
    let pointer = heads::read(store, remote, &options.remote_root)
        .ok()
        .and_then(|heads| heads.selected)
        .unwrap_or_else(|| protocol::pointer_from_snapshot(&local));
    let mut preview = conflict_preview(
        options,
        remote.provider(),
        &local.meta.payload_hash,
        &pointer,
    );
    preview.message =
        "Cloud sync changed; review the refreshed branches before confirming again".into();
    CloudSyncError::Conflict(Box::new(preview))
}

/// Revalidate observed branches immediately before publication as well as at
/// operation start. A writer arriving after that boundary remains concurrent.
struct ResolutionRemote<'a> {
    inner: &'a dyn CloudSyncRemote,
    store: &'a dyn CloudLocalStore,
    options: &'a LocalCloudSyncOptions,
    expected: &'a [String],
    selected: Option<&'a str>,
    local_content_hash: &'a str,
}
impl ResolutionRemote<'_> {
    fn validate(&self) -> Result<heads::RemoteHeads, CloudSyncError> {
        let heads = heads::read(self.store, self.inner, &self.options.remote_root)?;
        if heads.observed_revisions() != self.expected {
            if let Some(pointer) = heads.selected {
                return Err(CloudSyncError::Conflict(Box::new(conflict_preview(
                    self.options,
                    self.inner.provider(),
                    "",
                    &pointer,
                ))));
            }
            return Err(changed_preview(self.store, self.inner, self.options));
        }
        Ok(heads)
    }
}
impl CloudSyncRemote for ResolutionRemote<'_> {
    fn provider(&self) -> &'static str {
        self.inner.provider()
    }
    fn observe(&self, event: super::io::CloudSyncIoEvent) {
        self.inner.observe(event);
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
    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        if path == heads::device_head_path(&self.options.remote_root, &self.options.device_id) {
            let heads = self.validate()?;
            if self
                .store
                .build_sync_snapshot(self.options)?
                .sync_content_hash()?
                != self.local_content_hash
            {
                return Err(changed_preview(self.store, self.inner, self.options));
            }
            if let Some(revision) = self.selected {
                let pointer = heads
                    .maximal()
                    .into_iter()
                    .find(|p| p.revision_id == revision)
                    .ok_or_else(|| changed_preview(self.store, self.inner, self.options))?;
                if protocol::read_snapshot_for_pointer(
                    self.store,
                    self.inner,
                    self.options,
                    &pointer,
                )
                .is_err()
                {
                    return Err(changed_preview(self.store, self.inner, self.options));
                }
            }
        }
        self.inner.write(path, bytes)
    }
    fn delete(&self, path: &str) -> Result<(), CloudSyncError> {
        self.inner.delete(path)
    }
    fn list_files(&self, path: &str) -> Result<Vec<String>, CloudSyncError> {
        self.inner.list_files(path)
    }
}

pub fn resolve_with_remote(
    store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
    remote: &dyn CloudSyncRemote,
    state: &CloudSyncState,
    request: &CloudResolutionRequest,
    before_apply: &dyn Fn() -> Result<(), CloudSyncError>,
    control: &CloudSyncOperationControl,
) -> Result<CloudSyncResult, CloudSyncError> {
    control.checkpoint()?;
    let store = &control.local_store(store);
    let remote = &control.remote(remote);
    let guarded = ResolutionRemote {
        inner: remote,
        store,
        options,
        expected: &request.observed_revisions,
        selected: request.selected_revision.as_deref(),
        local_content_hash: &request.local_content_hash,
    };
    let result = (|| {
        control.checkpoint()?;
        let mut heads = guarded.validate()?;
        let local = store.build_sync_snapshot(options)?;
        if local.sync_content_hash()? != request.local_content_hash {
            return Err(changed_preview(store, remote, options));
        }
        if let Some(revision) = &request.selected_revision {
            heads.selected = Some(
                heads
                    .maximal()
                    .into_iter()
                    .find(|p| &p.revision_id == revision)
                    .ok_or(CloudSyncError::NoRemoteSnapshot)?,
            );
            let pointer = heads.selected.as_ref().expect("selected revision");
            let snapshot = protocol::read_snapshot_for_pointer(store, remote, options, pointer)?;
            pull_snapshot_prepared(
                store,
                options,
                &guarded,
                state,
                true,
                PullExecution {
                    before_apply,
                    control,
                },
                Some(PreparedSync {
                    local,
                    heads,
                    remote: Some(snapshot),
                }),
            )
        } else {
            // Every observed branch matters: a different default branch cannot
            // hide a wrong local password or unsupported snapshot format.
            for pointer in heads.maximal() {
                if let Err(error) =
                    protocol::read_snapshot_for_pointer(store, remote, options, &pointer)
                    && blocks_overwrite(&error)
                {
                    return Err(changed_preview(store, remote, options));
                }
            }
            push_snapshot_prepared(
                store,
                options,
                &guarded,
                state,
                true,
                control,
                Some(PreparedSync {
                    local,
                    heads,
                    remote: None,
                }),
            )
        }
    })();
    let result = result.map_err(|error| match error {
        CloudSyncError::NoRemoteSnapshot
        | CloudSyncError::SnapshotMissing { .. }
        | CloudSyncError::LocalChangedDuringSync
        | CloudSyncError::RemoteSnapshotUnreadable { .. }
        | CloudSyncError::CorruptedSnapshot { .. }
        | CloudSyncError::RevisionMismatch { .. }
        | CloudSyncError::HashMismatch { .. } => changed_preview(store, remote, options),
        other => other,
    });
    with_details(result, store, remote, options)
}

#[cfg(test)]
mod tests {
    use super::summarize;
    use crate::RawPortableSnapshot;
    #[test]
    fn differences_count_core_records_without_values_or_excluded_data() {
        let mut local = RawPortableSnapshot::sync("local", "test");
        let mut remote = local.clone();
        local.entities.insert("credentials".into(), r#"{"credentials":[{"id":"a","password":"PRIVATE-OLD"},{"id":"b","password":"PRIVATE-DELETE"}]}"#.into());
        remote.entities.insert("credentials".into(), r#"{"credentials":[{"id":"a","password":"PRIVATE-NEW"},{"id":"c","password":"PRIVATE-ADD"}]}"#.into());
        remote
            .entities
            .insert("PRIVATE-EXTENSION".into(), "PRIVATE-CONTENT".into());
        assert!(summarize(&local, &remote).is_empty());
        local.entities.insert("notes".into(), r#"{"notes":[{"id":"a","title":"PRIVATE-OLD"},{"id":"b","title":"PRIVATE-DELETE"}],"folders":[]}"#.into());
        remote.entities.insert("notes".into(), r#"{"notes":[{"id":"a","title":"PRIVATE-NEW"},{"id":"c","title":"PRIVATE-ADD"}],"folders":[]}"#.into());
        let differences = summarize(&local, &remote);
        let notes = differences.iter().find(|d| d.entity == "notes").unwrap();
        assert_eq!((notes.added, notes.removed, notes.modified), (1, 1, 1));
        let json = serde_json::to_string(&differences).unwrap();
        assert!(!json.contains("PRIVATE"));
        assert!(!json.contains("extensions"));
        assert!(!json.contains("credentials"));
    }
}
