use std::collections::HashSet;
use std::time::Duration;

use crate::portable_snapshot::{RawPortableSnapshot, SnapshotSyncClock};

use super::{
    CloudLocalStore, CloudSyncError, CloudSyncRemote, LocalCloudSyncOptions, RemoteSyncPointer,
    SYNC_SNAPSHOTS_DIR, current_time_ms, remote_path,
};

pub const SYNC_SNAPSHOT_KEEP_RECENT: usize = 5;
pub const SYNC_SNAPSHOT_GC_GRACE_PERIOD: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
struct SnapshotGcEntry {
    path: String,
    revision_id: String,
    created_at_ms: u64,
    deletable: bool,
}

pub fn cleanup_sync_snapshots_with_remote(
    local_store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
    remote: &dyn CloudSyncRemote,
    latest: Option<&RemoteSyncPointer>,
) -> Result<(), CloudSyncError> {
    cleanup_sync_snapshots_with_remote_controlled(
        local_store,
        options,
        remote,
        latest,
        &super::control::CloudSyncOperationControl::default(),
    )
}

pub fn cleanup_sync_snapshots_with_remote_controlled(
    local_store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
    remote: &dyn CloudSyncRemote,
    latest: Option<&RemoteSyncPointer>,
    control: &super::control::CloudSyncOperationControl,
) -> Result<(), CloudSyncError> {
    control.checkpoint()?;
    let remote = &control.remote(remote);
    let local_store = &control.local_store(local_store);
    let prefix = remote_path(&options.remote_root, SYNC_SNAPSHOTS_DIR);
    let mut protected = super::heads::protected_revisions(remote, &options.remote_root)?;
    if let Some(latest) = latest {
        protected.insert(latest.revision_id.clone());
    }
    let clocks = authenticated_commit_clocks(local_store, options, remote, latest)?;
    let mut snapshots = Vec::new();
    for path in remote.list_files(&prefix)? {
        control.checkpoint()?;
        let Some(revision_id) = snapshot_revision_from_path(&path) else {
            continue;
        };
        // Active and staged branches cannot be collected, so downloading and
        // deriving their keys during GC would serve no purpose.
        if protected.contains(&revision_id) {
            continue;
        }
        let entry = match remote.read_if_exists(&path) {
            Ok(Some(bytes)) => {
                match local_store
                    .decode_sync_snapshot(&bytes, options.master_password.expose_secret())
                {
                    Ok(snapshot) if snapshot.meta.revision_id == revision_id => SnapshotGcEntry {
                        path,
                        revision_id,
                        created_at_ms: snapshot.meta.created_at_ms,
                        deletable: !snapshot.meta.payload_hash.is_empty()
                            && snapshot.meta.schema_version
                                == crate::portable_snapshot::FIXED_SYNC_SNAPSHOT_SCHEMA_VERSION
                            && causally_collectable(&snapshot, &clocks),
                    },
                    _ => SnapshotGcEntry {
                        path,
                        revision_id,
                        created_at_ms: 0,
                        deletable: false,
                    },
                }
            }
            _ => SnapshotGcEntry {
                path,
                revision_id,
                created_at_ms: 0,
                deletable: false,
            },
        };
        snapshots.push(entry);
    }

    let mut first_error = None;
    for path in plan_snapshot_gc(
        snapshots,
        latest.map(|pointer| pointer.revision_id.as_str()),
        current_time_ms(),
        SYNC_SNAPSHOT_KEEP_RECENT,
        SYNC_SNAPSHOT_GC_GRACE_PERIOD,
    ) {
        protected.extend(super::heads::protected_revisions(
            remote,
            &options.remote_root,
        )?);
        if let Some(head) =
            super::load_sync_pointer_from_remote(local_store, remote, &options.remote_root)?
        {
            protected.insert(head.revision_id);
        }
        if snapshot_revision_from_path(&path).is_some_and(|revision| protected.contains(&revision))
        {
            continue;
        }
        if let Err(error) = remote.delete(&path)
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    control.checkpoint()?;
    first_error.map_or(Ok(()), Err)
}

/// Capacity cleanup never borrows slots from other roots or unverified data.
pub(super) fn reserve_snapshot_capacity(
    local_store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
    remote: &dyn CloudSyncRemote,
    expected: Option<&RemoteSyncPointer>,
    target_revision: &str,
) -> Result<(), CloudSyncError> {
    let Some(capacity) = remote.file_capacity()? else {
        return Ok(());
    };
    let target = remote_path(
        &options.remote_root,
        &super::legacy_sync_snapshot_file(target_revision),
    );
    let mut additional = 0;
    for path in [
        &target,
        &remote_path(&options.remote_root, super::SYNC_LATEST_FILE),
        &remote_path(&options.remote_root, super::SYNC_CURRENT_FILE),
        &super::heads::device_head_path(&options.remote_root, &options.device_id),
        &super::heads::pending_head_path(&options.remote_root, &options.device_id),
    ] {
        additional += usize::from(remote.read_if_exists(path)?.is_none());
    }
    let needed = capacity
        .used
        .saturating_add(additional)
        .saturating_sub(capacity.limit);
    if needed == 0 {
        return Ok(());
    }
    let mut protected = super::heads::protected_revisions(remote, &options.remote_root)?;
    protected.insert(target_revision.to_owned());
    if let Some(expected) = expected {
        protected.insert(expected.revision_id.clone());
    }
    let clocks = authenticated_commit_clocks(local_store, options, remote, expected)?;
    if let Some(bytes) =
        remote.read_if_exists(&remote_path(&options.remote_root, super::SYNC_CURRENT_FILE))?
        && let Ok(snapshot) =
            local_store.decode_sync_snapshot(&bytes, options.master_password.expose_secret())
    {
        protected.insert(snapshot.meta.revision_id);
    }
    let prefix = format!(
        "{}/",
        remote_path(&options.remote_root, SYNC_SNAPSHOTS_DIR).trim_end_matches('/')
    );
    let mut candidates = Vec::new();
    for path in remote.list_files(&prefix)? {
        let Some(name) = path
            .strip_prefix(&prefix)
            .filter(|name| !name.contains('/'))
        else {
            continue;
        };
        let Some(revision) = snapshot_revision_from_path(name) else {
            continue;
        };
        if protected.contains(&revision) {
            continue;
        }
        let Some(bytes) = remote.read_if_exists(&path)? else {
            continue;
        };
        let Ok(snapshot) =
            local_store.decode_sync_snapshot(&bytes, options.master_password.expose_secret())
        else {
            continue;
        };
        if snapshot.meta.revision_id == revision
            && snapshot.meta.schema_version
                == crate::portable_snapshot::FIXED_SYNC_SNAPSHOT_SCHEMA_VERSION
            && crate::portable_snapshot::validate_raw_snapshot(&snapshot).is_ok()
            && causally_collectable(&snapshot, &clocks)
        {
            candidates.push((snapshot.meta.created_at_ms, path, revision));
        }
    }
    candidates.sort();
    let mut freed = 0;
    for (_, path, revision) in candidates {
        // Re-read before every deletion; an unreadable head aborts cleanup.
        if let Some(head) =
            super::load_sync_pointer_from_remote(local_store, remote, &options.remote_root)?
        {
            protected.insert(head.revision_id);
        }
        protected.extend(super::heads::protected_revisions(
            remote,
            &options.remote_root,
        )?);
        if protected.contains(&revision) {
            continue;
        }
        remote.delete(&path)?;
        if remote.read_if_exists(&path)?.is_none() {
            freed += 1;
        }
        if freed >= needed {
            return Ok(());
        }
    }
    Err(CloudSyncError::GistCapacity)
}

fn authenticated_commit_clocks(
    local_store: &dyn CloudLocalStore,
    options: &LocalCloudSyncOptions,
    remote: &dyn CloudSyncRemote,
    latest: Option<&RemoteSyncPointer>,
) -> Result<Vec<SnapshotSyncClock>, CloudSyncError> {
    let mut heads = super::heads::read_device_heads(remote, &options.remote_root)?;
    heads.extend(latest.cloned());
    let mut seen = HashSet::new();
    let mut clocks = Vec::new();
    for head in heads {
        if head.sync_clock.is_some() && seen.insert(head.revision_id.clone()) {
            // An untrusted JSON head alone cannot authorize deletion. Verify
            // its causal history against the authenticated immutable snapshot.
            let snapshot =
                super::protocol::read_snapshot_for_pointer(local_store, remote, options, &head)?;
            if let Some(clock) = snapshot.meta.sync_clock {
                clocks.push(clock);
            }
        }
    }
    Ok(clocks)
}

fn causally_collectable(snapshot: &RawPortableSnapshot, clocks: &[SnapshotSyncClock]) -> bool {
    // An eventually consistent listing may omit an active or staged head. A
    // versioned snapshot must therefore be a strict ancestor of an individually
    // authenticated commit, not merely absent from the current reference set.
    // Joining concurrent clocks here would invent ancestry and is unsafe.
    snapshot.meta.sync_clock.as_ref().is_none_or(|candidate| {
        clocks
            .iter()
            .any(|clock| clock.strictly_dominates(candidate))
    })
}

fn plan_snapshot_gc(
    mut snapshots: Vec<SnapshotGcEntry>,
    latest_revision: Option<&str>,
    now_ms: u64,
    keep_recent: usize,
    grace_period: Duration,
) -> Vec<String> {
    let mut protected = HashSet::new();
    if let Some(revision) = latest_revision {
        protected.insert(revision.to_string());
    }
    snapshots.sort_by_key(|snapshot| snapshot.created_at_ms);
    for snapshot in snapshots.iter().rev().take(keep_recent) {
        protected.insert(snapshot.revision_id.clone());
    }
    let grace_ms = u64::try_from(grace_period.as_millis()).unwrap_or(u64::MAX);
    snapshots
        .into_iter()
        .filter(|snapshot| snapshot.deletable)
        .filter(|snapshot| !protected.contains(&snapshot.revision_id))
        .filter(|snapshot| now_ms.saturating_sub(snapshot.created_at_ms) > grace_ms)
        .map(|snapshot| snapshot.path)
        .collect()
}

fn snapshot_revision_from_path(path: &str) -> Option<String> {
    let filename = path.rsplit('/').next()?;
    filename
        .strip_suffix(".redb.enc")
        .filter(|revision| !revision.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{SnapshotGcEntry, plan_snapshot_gc};
    use std::time::Duration;

    fn entry(revision: &str, created_at_ms: u64) -> SnapshotGcEntry {
        SnapshotGcEntry {
            path: format!("nyaterm/sync/snapshots/{revision}.redb.enc"),
            revision_id: revision.to_string(),
            created_at_ms,
            deletable: true,
        }
    }

    #[test]
    fn keeps_latest_and_five_most_recent_snapshots() {
        let delete = plan_snapshot_gc(
            (1..=8).map(|n| entry(&format!("r{n}"), n)).collect(),
            Some("r1"),
            100_000_000,
            5,
            Duration::ZERO,
        );
        assert!(!delete.iter().any(|path| path.ends_with("r1.redb.enc")));
        assert_eq!(delete.len(), 2);
    }

    #[test]
    fn keeps_recent_and_unreadable_snapshots() {
        let mut unreadable = entry("unreadable", 1);
        unreadable.deletable = false;
        let delete = plan_snapshot_gc(
            vec![entry("old", 1), entry("fresh", 99_000), unreadable],
            None,
            100_000,
            0,
            Duration::from_secs(2),
        );
        assert_eq!(delete, vec!["nyaterm/sync/snapshots/old.redb.enc"]);
    }
}
