use std::sync::{Barrier, Mutex};

use super::{
    ConnectionStore, MemoryRemote, OptionsTestLocalStore, local_connection, options,
    unique_temp_dir,
};
use crate::SessionsConfig;
use crate::cloud_sync::{
    CloudLocalStore, CloudRemoteCheckDecision, CloudSyncError, CloudSyncRemote, CloudSyncState,
    check_snapshot_with_remote_state, pull_snapshot_with_remote, push_snapshot_with_remote,
};

struct RacingRemote {
    inner: MemoryRemote,
    commit_barrier: Barrier,
    race: Mutex<bool>,
}

impl CloudSyncRemote for RacingRemote {
    fn provider(&self) -> &'static str {
        "memory"
    }
    fn create_dir(&self, path: &str) -> Result<(), CloudSyncError> {
        self.inner.create_dir(path)
    }
    fn read_if_exists(&self, path: &str) -> Result<Option<Vec<u8>>, CloudSyncError> {
        self.inner.read_if_exists(path)
    }
    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        if *self.race.lock().unwrap()
            && path.contains("sync/heads/")
            && !path.ends_with(".pending.json")
        {
            // Both writers have already passed the old check-before-write
            // window. Neither one can see the other's committed pointer yet.
            self.commit_barrier.wait();
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

#[test]
fn simultaneous_publications_retain_both_branches_until_explicit_resolution() {
    for choose_cloud in [false, true] {
        let a_dir = unique_temp_dir("head-race-a");
        let b_dir = unique_temp_dir("head-race-b");
        let a = options(&a_dir, &a_dir, "a");
        let b = options(&b_dir, &b_dir, "b");
        let remote = RacingRemote {
            inner: MemoryRemote::default(),
            commit_barrier: Barrier::new(2),
            race: Mutex::new(false),
        };
        let first = push_snapshot_with_remote(
            &OptionsTestLocalStore(&a),
            &a,
            &remote,
            &CloudSyncState::default(),
            false,
        )
        .unwrap();
        let synced_b = pull_snapshot_with_remote(
            &OptionsTestLocalStore(&b),
            &b,
            &remote,
            &CloudSyncState::default(),
            true,
        )
        .unwrap();
        for (device, label) in [(&a, "A edit"), (&b, "B edit")] {
            ConnectionStore::open(&device.config_dir)
                .unwrap()
                .replace_sessions(&SessionsConfig {
                    connections: vec![local_connection(label, label, "bash")],
                    ..SessionsConfig::default()
                })
                .unwrap();
        }
        *remote.race.lock().unwrap() = true;
        let (published_a, published_b) = std::thread::scope(|scope| {
            let left = scope.spawn(|| {
                push_snapshot_with_remote(
                    &OptionsTestLocalStore(&a),
                    &a,
                    &remote,
                    &first.state,
                    false,
                )
            });
            let right = scope.spawn(|| {
                push_snapshot_with_remote(
                    &OptionsTestLocalStore(&b),
                    &b,
                    &remote,
                    &synced_b.state,
                    false,
                )
            });
            (
                left.join().unwrap().unwrap(),
                right.join().unwrap().unwrap(),
            )
        });
        *remote.race.lock().unwrap() = false;
        // Listings can omit our own just-published head while serving the
        // other writer's shared pointer. The local baseline must still stop
        // an automatic pull from replacing our edit with a concurrent branch.
        let hidden = MemoryRemote {
            files: Mutex::new(remote.inner.files.lock().unwrap().clone()),
            hidden_listing_path: Some(crate::cloud_sync::heads::device_head_path(
                &a.remote_root,
                "a",
            )),
            ..Default::default()
        };
        crate::cloud_sync::write_sync_pointer(
            &OptionsTestLocalStore(&a),
            &hidden,
            &a.remote_root,
            published_b.pointer.as_ref().unwrap(),
        )
        .unwrap();
        let store = OptionsTestLocalStore(&a);
        assert!(matches!(
            check_snapshot_with_remote_state(&store, &a, &hidden, &published_a.state, true),
            Err(CloudSyncError::Conflict(_))
        ));
        assert!(matches!(
            pull_snapshot_with_remote(&store, &a, &hidden, &published_a.state, false),
            Err(CloudSyncError::Conflict(_))
        ));
        assert!(matches!(
            push_snapshot_with_remote(&store, &a, &hidden, &published_a.state, false),
            Err(CloudSyncError::Conflict(_))
        ));
        let explicit_cloud =
            pull_snapshot_with_remote(&store, &a, &hidden, &published_a.state, true).unwrap();
        assert_eq!(
            explicit_cloud
                .pointer
                .as_ref()
                .unwrap()
                .sync_clock
                .as_ref()
                .unwrap()
                .counters["a"],
            3
        );
        // Restore A's data/state before exercising the original fully visible
        // two-head resolution below; the hidden-view test used a separate remote.
        let saved_a = crate::cloud_sync::protocol::read_snapshot_for_pointer(
            &store,
            &remote,
            &a,
            published_a.pointer.as_ref().unwrap(),
        )
        .unwrap();
        store.apply_sync_snapshot(&a, &saved_a).unwrap();
        store.persist_cloud_sync_state(&published_a.state).unwrap();
        for (device, state) in [(&a, &published_a.state), (&b, &published_b.state)] {
            assert!(matches!(
                check_snapshot_with_remote_state(
                    &OptionsTestLocalStore(device),
                    device,
                    &remote,
                    state,
                    true
                ),
                Err(CloudSyncError::Conflict(_))
            ));
        }
        let protected =
            crate::cloud_sync::heads::protected_revisions(&remote, &a.remote_root).unwrap();
        assert!(protected.contains(&published_a.pointer.unwrap().revision_id));
        assert!(protected.contains(&published_b.pointer.unwrap().revision_id));
        let resolved = if choose_cloud {
            pull_snapshot_with_remote(
                &OptionsTestLocalStore(&a),
                &a,
                &remote,
                &published_a.state,
                true,
            )
            .unwrap()
        } else {
            push_snapshot_with_remote(
                &OptionsTestLocalStore(&a),
                &a,
                &remote,
                &published_a.state,
                true,
            )
            .unwrap()
        };
        let checked = check_snapshot_with_remote_state(
            &OptionsTestLocalStore(&a),
            &a,
            &remote,
            &resolved.state,
            true,
        )
        .unwrap();
        assert_eq!(checked.decision, CloudRemoteCheckDecision::UpToDate);
        let clock = resolved.pointer.unwrap().sync_clock.unwrap();
        assert_eq!(clock.counters["a"], 3);
        assert_eq!(clock.counters["b"], 1);
    }
}

#[test]
fn compatibility_pointer_failure_does_not_hide_an_independent_device_commit() {
    let dir = unique_temp_dir("head-compat-failure");
    let options = options(&dir, &dir, "device");
    let remote = MemoryRemote::default();
    remote.fail_next_write_containing(crate::cloud_sync::SYNC_LATEST_FILE);
    let published = push_snapshot_with_remote(
        &OptionsTestLocalStore(&options),
        &options,
        &remote,
        &CloudSyncState::default(),
        false,
    )
    .unwrap();
    let checked = check_snapshot_with_remote_state(
        &OptionsTestLocalStore(&options),
        &options,
        &remote,
        &published.state,
        true,
    )
    .unwrap();
    assert_eq!(checked.decision, CloudRemoteCheckDecision::UpToDate);
    assert_eq!(checked.pointer, published.pointer);
}

#[test]
fn stale_remote_revision_cannot_roll_back_an_already_synced_device() {
    let dir = unique_temp_dir("head-stale-read");
    let options = options(&dir, &dir, "device");
    let remote = MemoryRemote::default();
    let store = OptionsTestLocalStore(&options);
    let first =
        push_snapshot_with_remote(&store, &options, &remote, &CloudSyncState::default(), false)
            .unwrap();
    let old_files = remote.files.lock().unwrap().clone();
    ConnectionStore::open(&options.config_dir)
        .unwrap()
        .replace_sessions(&SessionsConfig {
            connections: vec![local_connection("new", "New configuration", "bash")],
            ..SessionsConfig::default()
        })
        .unwrap();
    let second = push_snapshot_with_remote(&store, &options, &remote, &first.state, false).unwrap();
    *remote.files.lock().unwrap() = old_files;
    assert!(matches!(
        check_snapshot_with_remote_state(&store, &options, &remote, &second.state, true),
        Err(CloudSyncError::StaleRemoteRevision)
    ));
    assert!(matches!(
        pull_snapshot_with_remote(&store, &options, &remote, &second.state, false),
        Err(CloudSyncError::StaleRemoteRevision)
    ));
    assert!(matches!(
        push_snapshot_with_remote(&store, &options, &remote, &second.state, false),
        Err(CloudSyncError::StaleRemoteRevision)
    ));
    assert_eq!(
        ConnectionStore::open(&options.config_dir)
            .unwrap()
            .load_sessions()
            .unwrap()
            .connections[0]
            .name,
        "New configuration"
    );
}

#[test]
fn cleanup_preserves_old_device_heads_and_staged_snapshots_under_capacity_pressure() {
    use crate::cloud_sync::{
        CloudLocalStore, gc, heads, legacy_sync_snapshot_file, protocol, remote_path,
    };
    use crate::portable_snapshot::SnapshotSyncClock;

    let dir = unique_temp_dir("head-gc-protection");
    let options = options(&dir, &dir, "gc-device");
    let store = OptionsTestLocalStore(&options);
    let remote = MemoryRemote {
        capacity_limit: Some(10),
        ..Default::default()
    };
    for index in 0..7 {
        let mut snapshot = crate::portable_snapshot::project_fixed_sync_snapshot(
            &store.build_sync_snapshot(&options).unwrap(),
        )
        .unwrap();
        snapshot.meta.revision_id = format!("old-{index}");
        snapshot.meta.created_at_ms = index;
        snapshot.meta.device_id = "other-device".into();
        snapshot.meta.sync_clock = Some(SnapshotSyncClock {
            counters: [(
                "other-device".into(),
                match index {
                    0 => 7,
                    1 => 8,
                    _ => index,
                },
            )]
            .into(),
            legacy_revision: None,
        });
        snapshot.recalculate_hash().unwrap();
        remote
            .write(
                &remote_path(
                    &options.remote_root,
                    &legacy_sync_snapshot_file(&snapshot.meta.revision_id),
                ),
                &store.encode_sync_snapshot(&snapshot, "secret").unwrap(),
            )
            .unwrap();
        if index == 0 {
            heads::publish(
                &remote,
                &options.remote_root,
                &protocol::pointer_from_snapshot(&snapshot),
            )
            .unwrap();
        }
    }
    heads::stage(&remote, &options.remote_root, "other-device", "old-1").unwrap();
    gc::reserve_snapshot_capacity(&store, &options, &remote, None, "next").unwrap();
    gc::cleanup_sync_snapshots_with_remote(&store, &options, &remote, None).unwrap();
    for revision in ["old-0", "old-1"] {
        assert!(
            remote
                .read_if_exists(&remote_path(
                    &options.remote_root,
                    &legacy_sync_snapshot_file(revision)
                ))
                .unwrap()
                .is_some()
        );
    }
    assert!(
        remote
            .read_if_exists(&remote_path(
                &options.remote_root,
                &legacy_sync_snapshot_file("old-2")
            ))
            .unwrap()
            .is_none()
    );
}

#[test]
fn cleanup_retains_concurrent_snapshots_when_device_listing_is_stale() {
    use crate::cloud_sync::{
        CloudLocalStore, gc, heads, legacy_sync_snapshot_file, protocol, remote_path,
    };
    use crate::portable_snapshot::SnapshotSyncClock;

    let dir = unique_temp_dir("gc-stale-device-listing");
    let options = options(&dir, &dir, "collector");
    let store = OptionsTestLocalStore(&options);
    let remote = MemoryRemote {
        capacity_limit: Some(12),
        hidden_listing_path: Some(heads::device_head_path(
            &options.remote_root,
            "hidden-device",
        )),
        ..Default::default()
    };
    for index in 0..9 {
        let device = if index == 0 {
            "visible-device"
        } else {
            "hidden-device"
        };
        let mut snapshot = crate::portable_snapshot::project_fixed_sync_snapshot(
            &store.build_sync_snapshot(&options).unwrap(),
        )
        .unwrap();
        snapshot.meta.revision_id = format!("branch-{index}");
        snapshot.meta.device_id = device.into();
        snapshot.meta.created_at_ms = index;
        snapshot.meta.sync_clock = Some(SnapshotSyncClock {
            counters: [(device.into(), index.max(1))].into(),
            legacy_revision: None,
        });
        snapshot.recalculate_hash().unwrap();
        remote
            .write(
                &remote_path(
                    &options.remote_root,
                    &legacy_sync_snapshot_file(&snapshot.meta.revision_id),
                ),
                &store.encode_sync_snapshot(&snapshot, "secret").unwrap(),
            )
            .unwrap();
        if index == 0 || index == 8 {
            heads::publish(
                &remote,
                &options.remote_root,
                &protocol::pointer_from_snapshot(&snapshot),
            )
            .unwrap();
        }
    }
    let original = remote.files.lock().unwrap().clone();
    gc::cleanup_sync_snapshots_with_remote(&store, &options, &remote, None).unwrap();
    assert_eq!(*remote.files.lock().unwrap(), original);
    assert!(matches!(
        gc::reserve_snapshot_capacity(&store, &options, &remote, None, "next"),
        Err(CloudSyncError::GistCapacity)
    ));
    assert_eq!(*remote.files.lock().unwrap(), original);
}

#[test]
fn cloud_sync_explicit_resolution_pins_a_non_default_branch_and_rejects_stale_previews() {
    use crate::cloud_sync::conflict::{CloudResolutionRequest, resolve_with_remote};
    use crate::cloud_sync::control::CloudSyncOperationControl;
    let root = unique_temp_dir("branch-choice");
    let mut a = options(&root.join("a"), &root, "a");
    a.device_name = Some("Device A".into());
    let remote = MemoryRemote::default();
    let initial = push_snapshot_with_remote(
        &OptionsTestLocalStore(&a),
        &a,
        &remote,
        &CloudSyncState::default(),
        false,
    )
    .unwrap();
    let initial_files = remote.files.lock().unwrap().clone();
    let mut states = Vec::new();
    for id in ["a", "b", "c"] {
        let mut device = options(&root.join(id), &root, id);
        device.device_name = Some(format!("Device {id}"));
        let isolated = MemoryRemote {
            files: Mutex::new(initial_files.clone()),
            ..Default::default()
        };
        let baseline = if id == "a" {
            initial.clone()
        } else {
            pull_snapshot_with_remote(
                &OptionsTestLocalStore(&device),
                &device,
                &isolated,
                &CloudSyncState::default(),
                true,
            )
            .unwrap()
        };
        ConnectionStore::open(&device.config_dir)
            .unwrap()
            .replace_sessions(&SessionsConfig {
                connections: vec![local_connection(id, &format!("branch {id}"), "bash")],
                ..Default::default()
            })
            .unwrap();
        let publication = push_snapshot_with_remote(
            &OptionsTestLocalStore(&device),
            &device,
            &isolated,
            &baseline.state,
            false,
        )
        .unwrap();
        let pointer = publication.pointer.as_ref().unwrap();
        let snapshot_path = crate::cloud_sync::remote_path(
            &device.remote_root,
            &crate::cloud_sync::legacy_sync_snapshot_file(&pointer.revision_id),
        );
        let head_path = crate::cloud_sync::heads::device_head_path(&device.remote_root, id);
        for path in [snapshot_path, head_path] {
            remote
                .write(&path, &isolated.read_if_exists(&path).unwrap().unwrap())
                .unwrap();
        }
        states.push(publication.state);
    }
    let store = OptionsTestLocalStore(&a);
    let CloudSyncError::Conflict(preview) =
        check_snapshot_with_remote_state(&store, &a, &remote, &states[0], true)
            .err()
            .unwrap()
    else {
        panic!("expected conflict");
    };
    assert_eq!(preview.branches.len(), 3);
    assert!(
        preview
            .branches
            .iter()
            .all(|b| b.readable && b.device_name.is_some())
    );
    let chosen = preview
        .branches
        .iter()
        .find(|b| b.revision != preview.remote_revision && b.device_id != "a")
        .unwrap();
    let chosen_device = chosen.device_id.clone();
    let request = CloudResolutionRequest {
        observed_revisions: preview.observed_revisions.clone(),
        selected_revision: Some(chosen.revision.clone()),
        local_content_hash: preview.local_content_hash.clone().unwrap(),
    };
    // A preview never authorizes replacing data edited during confirmation.
    let local_before = ConnectionStore::open(&a.config_dir)
        .unwrap()
        .load_sessions()
        .unwrap();
    ConnectionStore::open(&a.config_dir)
        .unwrap()
        .replace_sessions(&SessionsConfig {
            connections: vec![local_connection(
                "late",
                "edited during confirmation",
                "bash",
            )],
            ..Default::default()
        })
        .unwrap();
    let changed = resolve_with_remote(
        &store,
        &a,
        &remote,
        &states[0],
        &request,
        &|| Ok(()),
        &CloudSyncOperationControl::default(),
    )
    .unwrap_err();
    assert!(
        matches!(changed, CloudSyncError::Conflict(p) if p.local_content_hash.as_ref() != Some(&request.local_content_hash))
    );
    assert_eq!(
        ConnectionStore::open(&a.config_dir)
            .unwrap()
            .load_sessions()
            .unwrap()
            .connections[0]
            .id,
        "late"
    );
    ConnectionStore::open(&a.config_dir)
        .unwrap()
        .replace_sessions(&local_before)
        .unwrap();

    // Losing the chosen immutable snapshot refreshes readability and never
    // substitutes the protocol's default branch.
    let files_before = remote.files.lock().unwrap().clone();
    let chosen_path = crate::cloud_sync::remote_path(
        &a.remote_root,
        &crate::cloud_sync::legacy_sync_snapshot_file(&chosen.revision),
    );
    remote.files.lock().unwrap().remove(&chosen_path);
    let missing = resolve_with_remote(
        &store,
        &a,
        &remote,
        &states[0],
        &request,
        &|| Ok(()),
        &CloudSyncOperationControl::default(),
    )
    .unwrap_err();
    assert!(
        matches!(missing, CloudSyncError::Conflict(p) if p.branches.iter().any(|b| b.revision == chosen.revision && !b.readable))
    );
    *remote.files.lock().unwrap() = files_before.clone();

    // A new commit arriving after execution starts invalidates publication.
    let mut arriving = crate::cloud_sync::heads::read(&store, &remote, &a.remote_root)
        .unwrap()
        .maximal()
        .into_iter()
        .find(|p| p.revision_id == chosen.revision)
        .unwrap();
    arriving.revision_id = "arriving-during-resolution".into();
    arriving.device_id = "new-device".into();
    arriving
        .sync_clock
        .as_mut()
        .unwrap()
        .counters
        .insert("new-device".into(), 1);
    let raced = resolve_with_remote(
        &store,
        &a,
        &remote,
        &states[0],
        &request,
        &|| crate::cloud_sync::heads::publish(&remote, &a.remote_root, &arriving),
        &CloudSyncOperationControl::default(),
    )
    .unwrap_err();
    assert!(
        matches!(raced, CloudSyncError::Conflict(p) if p.observed_revisions.contains(&arriving.revision_id))
    );
    assert_eq!(
        ConnectionStore::open(&a.config_dir)
            .unwrap()
            .load_sessions()
            .unwrap(),
        local_before
    );
    *remote.files.lock().unwrap() = files_before;

    let resolved = resolve_with_remote(
        &store,
        &a,
        &remote,
        &states[0],
        &request,
        &|| Ok(()),
        &CloudSyncOperationControl::default(),
    )
    .unwrap();
    let sessions = ConnectionStore::open(&a.config_dir)
        .unwrap()
        .load_sessions()
        .unwrap();
    assert_eq!(
        sessions.connections[0].name,
        format!("branch {chosen_device}")
    );
    assert!(matches!(
        resolve_with_remote(
            &store,
            &a,
            &remote,
            &resolved.state,
            &request,
            &|| Ok(()),
            &CloudSyncOperationControl::default()
        ),
        Err(CloudSyncError::Conflict(_))
    ));
    let checked =
        check_snapshot_with_remote_state(&store, &a, &remote, &resolved.state, true).unwrap();
    assert_eq!(checked.decision, CloudRemoteCheckDecision::UpToDate);
}

#[test]
fn cloud_sync_device_name_is_optional_and_does_not_make_unchanged_content_dirty() {
    let root = unique_temp_dir("device-name");
    let mut device = options(&root, &root, "device");
    device.device_name = Some("Original name".into());
    let store = OptionsTestLocalStore(&device);
    let remote = MemoryRemote::default();
    let first =
        push_snapshot_with_remote(&store, &device, &remote, &CloudSyncState::default(), false)
            .unwrap();
    let snapshot = crate::cloud_sync::protocol::read_snapshot_for_pointer(
        &store,
        &remote,
        &device,
        first.pointer.as_ref().unwrap(),
    )
    .unwrap();
    assert_eq!(snapshot.meta.device_name.as_deref(), Some("Original name"));
    let mut old_metadata = serde_json::to_value(&snapshot.meta).unwrap();
    old_metadata.as_object_mut().unwrap().remove("device_name");
    let old: crate::PortableSnapshotMeta = serde_json::from_value(old_metadata).unwrap();
    assert!(old.device_name.is_none());
    device.device_name = Some("Renamed".into());
    let same = push_snapshot_with_remote(
        &OptionsTestLocalStore(&device),
        &device,
        &remote,
        &first.state,
        false,
    )
    .unwrap();
    assert_eq!(same.outcome, crate::CloudSyncOutcome::UpToDate);
    assert_eq!(same.pointer, first.pointer);
}
