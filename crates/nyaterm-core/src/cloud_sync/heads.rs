use std::collections::{BTreeMap, HashSet};

use crate::portable_snapshot::SnapshotSyncClock;
use serde::{Deserialize, Serialize};

use super::{
    CloudLocalStore, CloudSyncError, CloudSyncRemote, LocalCloudSyncOptions, RemoteSyncPointer,
    conflict_preview, load_sync_pointer_from_remote, remote_path,
};

pub(super) const HEADS_DIR: &str = "sync/heads/";
const MAX_HEAD_BYTES: usize = 64 * 1024;
const MAX_DEVICES: usize = 256;

pub(super) struct RemoteHeads {
    pub compatibility: Option<RemoteSyncPointer>,
    pub selected: Option<RemoteSyncPointer>,
    pub concurrent: bool,
    heads: Vec<RemoteSyncPointer>,
}

impl RemoteHeads {
    pub(super) fn maximal(&self) -> Vec<RemoteSyncPointer> {
        self.heads
            .iter()
            .filter(|head| !self.heads.iter().any(|other| dominates(other, head)))
            .cloned()
            .collect()
    }

    pub(super) fn observed_revisions(&self) -> Vec<String> {
        let mut revisions: Vec<_> = self.maximal().into_iter().map(|p| p.revision_id).collect();
        revisions.sort();
        revisions
    }

    pub fn require_single(
        &self,
        options: &LocalCloudSyncOptions,
        provider: &str,
        local_hash: &str,
    ) -> Result<(), CloudSyncError> {
        if self.concurrent {
            let mut preview = conflict_preview(
                options,
                provider,
                local_hash,
                self.selected.as_ref().expect("concurrent heads"),
            );
            preview.message = "Multiple devices published concurrent snapshots. Both branches are retained; explicitly choose local or cloud to resolve them.".into();
            return Err(CloudSyncError::Conflict(Box::new(preview)));
        }
        Ok(())
    }

    pub fn next_clock(
        &self,
        previous: Option<&SnapshotSyncClock>,
        device: &str,
    ) -> Result<SnapshotSyncClock, CloudSyncError> {
        let mut clock = previous.cloned().unwrap_or_default();
        for head in &self.heads {
            if let Some(other) = &head.sync_clock {
                clock.merge(other);
            }
        }
        if let Some(legacy) = self
            .compatibility
            .as_ref()
            .filter(|head| head.sync_clock.is_none())
        {
            clock.legacy_revision = Some(legacy.revision_id.clone());
        }
        let next = clock
            .counters
            .get(device)
            .copied()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| CloudSyncError::Remote("cloud sync device counter exhausted".into()))?;
        clock.counters.insert(device.into(), next);
        validate_clock(&clock)?;
        Ok(clock)
    }
}

pub(super) fn device_head_path(root: &str, device: &str) -> String {
    remote_path(
        root,
        &format!(
            "{HEADS_DIR}{}.json",
            super::protocol::sha256_hex(device.as_bytes())
        ),
    )
}

#[derive(Serialize, Deserialize)]
struct DeviceHead {
    device_id: String,
    committed: Option<RemoteSyncPointer>,
    pending_revision: Option<String>,
}

fn read_records(
    remote: &dyn CloudSyncRemote,
    root: &str,
) -> Result<Vec<DeviceHead>, CloudSyncError> {
    let prefix = remote_path(root, HEADS_DIR);
    let paths = remote.list_files(&prefix)?;
    if paths.len() > MAX_DEVICES * 2 {
        return Err(CloudSyncError::Remote(
            "too many cloud sync device heads".into(),
        ));
    }
    let mut records = Vec::new();
    for path in paths {
        if !path.starts_with(&prefix) || !path.ends_with(".json") {
            continue;
        }
        let Some(bytes) = remote.read_if_exists(&path)? else {
            continue;
        };
        let record = decode_record(&bytes)?;
        let expected_path = if record.committed.is_none() && record.pending_revision.is_some() {
            pending_head_path(root, &record.device_id)
        } else {
            device_head_path(root, &record.device_id)
        };
        if path != expected_path {
            return Err(CloudSyncError::Remote(
                "cloud sync device head path does not match its device".into(),
            ));
        }
        records.push(record);
    }
    Ok(records)
}

fn decode_record(bytes: &[u8]) -> Result<DeviceHead, CloudSyncError> {
    if bytes.len() > MAX_HEAD_BYTES {
        return Err(CloudSyncError::Remote(
            "cloud sync device head is too large".into(),
        ));
    }
    let record: DeviceHead = serde_json::from_slice(bytes)?;
    if let Some(head) = &record.committed {
        if head.device_id != record.device_id {
            return Err(CloudSyncError::Remote(
                "cloud sync device identity mismatch".into(),
            ));
        }
        validate_clock(head.sync_clock.as_ref().ok_or_else(|| {
            CloudSyncError::Remote("cloud sync device head has no causal clock".into())
        })?)?;
    }
    Ok(record)
}

pub(super) fn read_device_heads(
    remote: &dyn CloudSyncRemote,
    root: &str,
) -> Result<Vec<RemoteSyncPointer>, CloudSyncError> {
    Ok(read_records(remote, root)?
        .into_iter()
        .filter_map(|record| record.committed)
        .collect())
}

/// Register the staged object before uploading it. Even capacity-pressure GC
/// must not delete another writer's verified but not-yet-published snapshot.
pub(super) fn stage(
    remote: &dyn CloudSyncRemote,
    root: &str,
    device: &str,
    revision: &str,
) -> Result<(), CloudSyncError> {
    // A separate marker cannot roll back a committed head if the provider
    // serves a stale read, or if cancellation interrupts staging.
    write_record(
        remote,
        &pending_head_path(root, device),
        &DeviceHead {
            device_id: device.into(),
            committed: None,
            pending_revision: Some(revision.into()),
        },
    )
}

pub(super) fn pending_head_path(root: &str, device: &str) -> String {
    remote_path(
        root,
        &format!(
            "{HEADS_DIR}{}.pending.json",
            super::protocol::sha256_hex(device.as_bytes())
        ),
    )
}

pub(super) fn read(
    local_store: &dyn CloudLocalStore,
    remote: &dyn CloudSyncRemote,
    root: &str,
) -> Result<RemoteHeads, CloudSyncError> {
    let mut heads = read_device_heads(remote, root)?;
    // The shared pointer is a compatibility cache, while device heads are
    // authoritative. An interrupted or truncated `latest.redb` upload must not
    // block every later sync, so a decode failure is treated as a missing
    // pointer once another head identifies the remote revision. Without any
    // device head there is no safe baseline, so the corruption stays fatal.
    let compatibility = match load_sync_pointer_from_remote(local_store, remote, root) {
        Ok(pointer) => pointer,
        Err(CloudSyncError::PortableSnapshot(_) | CloudSyncError::Json(_)) if !heads.is_empty() => {
            None
        }
        Err(error) => return Err(error),
    };
    if let Some(latest) = &compatibility {
        heads.push(latest.clone());
    }
    let mut unique = BTreeMap::new();
    for head in heads {
        if let Some(previous) = unique.insert(head.revision_id.clone(), head.clone())
            && previous != head
        {
            return Err(CloudSyncError::Remote(
                "conflicting metadata for the same cloud revision".into(),
            ));
        }
    }
    let heads: Vec<_> = unique.into_values().collect();
    let maximal: Vec<_> = heads
        .iter()
        .filter(|head| !heads.iter().any(|other| dominates(other, head)))
        .collect();
    let selected = compatibility
        .as_ref()
        .and_then(|latest| {
            maximal
                .iter()
                .find(|head| head.revision_id == latest.revision_id)
                .copied()
        })
        .or_else(|| maximal.last().copied())
        .cloned();
    remote.observe(super::io::CloudSyncIoEvent::HeadsChecked);
    Ok(RemoteHeads {
        compatibility,
        selected,
        concurrent: maximal.len() > 1,
        heads,
    })
}

fn dominates(new: &RemoteSyncPointer, old: &RemoteSyncPointer) -> bool {
    match (&new.sync_clock, &old.sync_clock) {
        (Some(new), Some(old)) => new.strictly_dominates(old),
        (Some(new), None) => new.legacy_revision.as_deref() == Some(&old.revision_id),
        _ => false,
    }
}

fn validate_clock(clock: &SnapshotSyncClock) -> Result<(), CloudSyncError> {
    if clock.counters.is_empty()
        || clock.counters.len() > MAX_DEVICES
        || clock
            .counters
            .iter()
            .any(|(device, count)| device.is_empty() || device.len() > 256 || *count == 0)
    {
        return Err(CloudSyncError::Remote(
            "invalid cloud sync causal clock".into(),
        ));
    }
    Ok(())
}

pub(super) fn publish(
    remote: &dyn CloudSyncRemote,
    root: &str,
    head: &RemoteSyncPointer,
) -> Result<(), CloudSyncError> {
    write_record(
        remote,
        &device_head_path(root, &head.device_id),
        &DeviceHead {
            device_id: head.device_id.clone(),
            committed: Some(head.clone()),
            pending_revision: None,
        },
    )?;
    let _ = remote.delete(&pending_head_path(root, &head.device_id));
    Ok(())
}

fn write_record(
    remote: &dyn CloudSyncRemote,
    path: &str,
    record: &DeviceHead,
) -> Result<(), CloudSyncError> {
    let bytes = serde_json::to_vec(record)?;
    if bytes.len() > MAX_HEAD_BYTES {
        return Err(CloudSyncError::Remote(
            "cloud sync device head is too large".into(),
        ));
    }
    remote.write(path, &bytes)?;
    for attempt in 0..super::protocol::READBACK_VERIFY_ATTEMPTS {
        if remote.read_if_exists(path)?.as_deref() == Some(bytes.as_slice()) {
            return Ok(());
        }
        if attempt + 1 < super::protocol::READBACK_VERIFY_ATTEMPTS {
            super::protocol::readback_backoff(attempt);
        }
    }
    Err(CloudSyncError::Remote(
        "cloud sync device commit could not be verified".into(),
    ))
}

pub(super) fn protected_revisions(
    remote: &dyn CloudSyncRemote,
    root: &str,
) -> Result<HashSet<String>, CloudSyncError> {
    Ok(read_records(remote, root)?
        .into_iter()
        .flat_map(|record| {
            record
                .committed
                .map(|head| head.revision_id)
                .into_iter()
                .chain(record.pending_revision)
        })
        .collect())
}
