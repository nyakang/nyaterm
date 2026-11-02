use std::cell::Cell;
use std::cell::RefCell;

use super::{
    ConnectionStore, MemoryRemote, OptionsTestLocalStore, local_connection, options,
    test_local_data, unique_temp_dir,
};
use crate::SessionsConfig;
use crate::cloud_sync::{
    CloudLocalStore, CloudRemoteCheckDecision, CloudSyncError, CloudSyncOutcome, CloudSyncRemote,
    CloudSyncState, check_snapshot_with_remote_state, pull_snapshot_with_remote_guarded,
    push_snapshot_with_remote,
};

struct EditingRemote<'a> {
    inner: &'a MemoryRemote,
    edit: RefCell<Option<Box<dyn FnOnce() + 'a>>>,
}

struct CountingRemote<'a> {
    inner: &'a MemoryRemote,
    snapshot_reads: Cell<usize>,
}

#[test]
fn first_conflict_preview_reuses_the_snapshot_validated_by_that_check() {
    use crate::cloud_sync::{cache::CloudSyncCheckCache, check_snapshot_with_remote_cached};
    let dir = unique_temp_dir("cached-first-conflict");
    let options = options(&dir, &dir, "device");
    let store = OptionsTestLocalStore(&options);
    let memory = MemoryRemote::default();
    ConnectionStore::open(&options.config_dir)
        .unwrap()
        .replace_sessions(&SessionsConfig {
            connections: vec![local_connection("remote", "Remote", "bash")],
            ..Default::default()
        })
        .unwrap();
    push_snapshot_with_remote(&store, &options, &memory, &CloudSyncState::default(), false)
        .unwrap();
    ConnectionStore::open(&options.config_dir)
        .unwrap()
        .replace_sessions(&SessionsConfig {
            connections: vec![local_connection("edit", "Edited", "bash")],
            ..Default::default()
        })
        .unwrap();
    let remote = CountingRemote {
        inner: &memory,
        snapshot_reads: Cell::new(0),
    };
    let mut cache = CloudSyncCheckCache::default();
    for _ in 0..2 {
        let Err(CloudSyncError::Conflict(preview)) = check_snapshot_with_remote_cached(
            &store,
            &options,
            &remote,
            &CloudSyncState::default(),
            true,
            &mut cache,
        ) else {
            panic!("expected local and remote conflict");
        };
        assert_eq!(preview.branches.len(), 1);
        assert!(preview.branches[0].readable);
        assert_eq!(remote.snapshot_reads.get(), 1);
    }
}

#[test]
fn cached_checks_validate_daily_without_sliding_and_push_still_reads_the_snapshot() {
    use crate::cloud_sync::cache::{CloudSyncCheckCache, FULL_VALIDATION_INTERVAL_MS};
    use crate::cloud_sync::check_snapshot_with_remote_cached;
    let dir = unique_temp_dir("cached-daily-check");
    let options = options(&dir, &dir, "device");
    let store = OptionsTestLocalStore(&options);
    let memory = MemoryRemote::default();
    let published =
        push_snapshot_with_remote(&store, &options, &memory, &CloudSyncState::default(), false)
            .unwrap();
    let remote = CountingRemote {
        inner: &memory,
        snapshot_reads: Cell::new(0),
    };
    let mut cache = CloudSyncCheckCache::default();
    cache.now = Some(1_000);
    let first = check_snapshot_with_remote_cached(
        &store,
        &options,
        &remote,
        &published.state,
        true,
        &mut cache,
    )
    .unwrap();
    assert_eq!(remote.snapshot_reads.get(), 1);
    cache.now = Some(1_000 + FULL_VALIDATION_INTERVAL_MS - 1);
    let thin = check_snapshot_with_remote_cached(
        &store,
        &options,
        &remote,
        &first.state,
        true,
        &mut cache,
    )
    .unwrap();
    assert_eq!(remote.snapshot_reads.get(), 1);
    assert_eq!(thin.state.last_full_validation_at_ms, Some(1_000));
    cache.now = Some(1_000 + FULL_VALIDATION_INTERVAL_MS);
    check_snapshot_with_remote_cached(&store, &options, &remote, &thin.state, true, &mut cache)
        .unwrap();
    assert_eq!(remote.snapshot_reads.get(), 2);
    ConnectionStore::open(&options.config_dir)
        .unwrap()
        .replace_sessions(&SessionsConfig {
            connections: vec![local_connection("edit", "Edit", "bash")],
            ..Default::default()
        })
        .unwrap();
    let checked =
        check_snapshot_with_remote_cached(&store, &options, &remote, &thin.state, true, &mut cache)
            .unwrap();
    assert_eq!(checked.decision, CloudRemoteCheckDecision::LocalChanged);
    assert_eq!(remote.snapshot_reads.get(), 2);
    checked
        .push(
            &store,
            &options,
            &remote,
            &crate::cloud_sync::control::CloudSyncOperationControl::default(),
        )
        .unwrap();
    assert_eq!(
        remote.snapshot_reads.get(),
        4,
        "thin check must not replace pre-push validation or upload readback"
    );
}

#[test]
fn changing_pointer_metadata_at_the_same_revision_requires_full_validation() {
    use crate::cloud_sync::{
        SYNC_LATEST_FILE, cache::CloudSyncCheckCache, check_snapshot_with_remote_cached,
        remote_path,
    };
    let dir = unique_temp_dir("cached-pointer-identity");
    let options = options(&dir, &dir, "device");
    let store = OptionsTestLocalStore(&options);
    let memory = MemoryRemote::default();
    let published =
        push_snapshot_with_remote(&store, &options, &memory, &CloudSyncState::default(), false)
            .unwrap();
    let remote = CountingRemote {
        inner: &memory,
        snapshot_reads: Cell::new(0),
    };
    let mut cache = CloudSyncCheckCache::default();
    for _ in 0..2 {
        check_snapshot_with_remote_cached(
            &store,
            &options,
            &remote,
            &published.state,
            true,
            &mut cache,
        )
        .unwrap();
    }
    assert_eq!(remote.snapshot_reads.get(), 1);
    let mut pointer = published.pointer.unwrap();
    pointer.created_at_ms += 1;
    memory
        .write(
            &remote_path(&options.remote_root, SYNC_LATEST_FILE),
            &store.encode_sync_pointer(&pointer).unwrap(),
        )
        .unwrap();
    let path = crate::cloud_sync::heads::device_head_path(&options.remote_root, &options.device_id);
    let mut head: serde_json::Value =
        serde_json::from_slice(&memory.read_if_exists(&path).unwrap().unwrap()).unwrap();
    head["committed"] = serde_json::to_value(&pointer).unwrap();
    memory
        .write(&path, &serde_json::to_vec(&head).unwrap())
        .unwrap();
    check_snapshot_with_remote_cached(
        &store,
        &options,
        &remote,
        &published.state,
        true,
        &mut cache,
    )
    .unwrap();
    assert_eq!(remote.snapshot_reads.get(), 2);
    cache = CloudSyncCheckCache::default();
    check_snapshot_with_remote_cached(
        &store,
        &options,
        &remote,
        &published.state,
        true,
        &mut cache,
    )
    .unwrap();
    assert_eq!(
        remote.snapshot_reads.get(),
        3,
        "a cold cache never trusts persisted validation timestamps"
    );
}

#[test]
fn expired_validation_does_not_hide_corruption_or_authorize_a_cached_pull() {
    use crate::cloud_sync::cache::{CloudSyncCheckCache, FULL_VALIDATION_INTERVAL_MS};
    use crate::cloud_sync::{
        SYNC_CURRENT_FILE, check_snapshot_with_remote_cached, legacy_sync_snapshot_file,
        remote_path,
    };
    let dir = unique_temp_dir("cached-corruption");
    let options = options(&dir, &dir, "device");
    let store = OptionsTestLocalStore(&options);
    let remote = MemoryRemote::default();
    let published =
        push_snapshot_with_remote(&store, &options, &remote, &CloudSyncState::default(), false)
            .unwrap();
    let mut cache = CloudSyncCheckCache::default();
    cache.now = Some(1_000);
    check_snapshot_with_remote_cached(
        &store,
        &options,
        &remote,
        &published.state,
        true,
        &mut cache,
    )
    .unwrap();
    let pointer = published.pointer.unwrap();
    remote
        .write(
            &remote_path(
                &options.remote_root,
                &legacy_sync_snapshot_file(&pointer.revision_id),
            ),
            b"corrupted-encrypted-snapshot",
        )
        .unwrap();
    remote
        .write(
            &remote_path(&options.remote_root, SYNC_CURRENT_FILE),
            b"corrupted-current-snapshot",
        )
        .unwrap();
    // Even a receipt that is still fresh cannot authorize an explicit pull.
    let thin = check_snapshot_with_remote_cached(
        &store,
        &options,
        &remote,
        &published.state,
        true,
        &mut cache,
    )
    .unwrap();
    assert!(
        thin.pull(
            &store,
            &options,
            &remote,
            &|| Ok(()),
            &crate::cloud_sync::control::CloudSyncOperationControl::default()
        )
        .is_err()
    );
    cache.now = Some(1_000 + FULL_VALIDATION_INTERVAL_MS);
    assert!(matches!(
        check_snapshot_with_remote_cached(
            &store,
            &options,
            &remote,
            &published.state,
            true,
            &mut cache
        ),
        Err(CloudSyncError::Conflict(_))
    ));
    assert!(cache.get(&pointer).is_none());
}

#[test]
fn cached_checks_isolate_passwords_and_notice_concurrent_heads_with_unchanged_latest() {
    use crate::cloud_sync::{cache::CloudSyncCheckCache, check_snapshot_with_remote_cached};
    let a_dir = unique_temp_dir("cached-head-a");
    let b_dir = unique_temp_dir("cached-head-b");
    let a = options(&a_dir, &a_dir, "a");
    let b = options(&b_dir, &b_dir, "b");
    let store = OptionsTestLocalStore(&a);
    let memory = MemoryRemote::default();
    let first =
        push_snapshot_with_remote(&store, &a, &memory, &CloudSyncState::default(), false).unwrap();
    let remote = CountingRemote {
        inner: &memory,
        snapshot_reads: Cell::new(0),
    };
    let mut cache = CloudSyncCheckCache::default();
    check_snapshot_with_remote_cached(&store, &a, &remote, &first.state, true, &mut cache).unwrap();
    let mut wrong = a.clone();
    wrong.master_password = "wrong-password".into();
    assert!(
        check_snapshot_with_remote_cached(&store, &wrong, &remote, &first.state, true, &mut cache)
            .is_err()
    );
    check_snapshot_with_remote_cached(&store, &a, &remote, &first.state, true, &mut cache).unwrap();
    let fork = MemoryRemote::default();
    ConnectionStore::open(&b.config_dir)
        .unwrap()
        .replace_sessions(&SessionsConfig {
            connections: vec![local_connection("b", "B", "bash")],
            ..Default::default()
        })
        .unwrap();
    push_snapshot_with_remote(
        &OptionsTestLocalStore(&b),
        &b,
        &fork,
        &CloudSyncState::default(),
        false,
    )
    .unwrap();
    // Publish a second independent device branch without changing latest.redb.
    for (path, bytes) in fork.files.lock().unwrap().iter() {
        if path.contains("/heads/") || path.contains("/snapshots/") {
            memory.write(path, bytes).unwrap();
        }
    }
    for index in 0..2 {
        let before = remote.snapshot_reads.get();
        let error =
            check_snapshot_with_remote_cached(&store, &a, &remote, &first.state, true, &mut cache)
                .err()
                .unwrap();
        let CloudSyncError::Conflict(preview) = error else {
            panic!("expected concurrent branches")
        };
        assert_eq!(preview.branches.len(), 2);
        assert!(preview.branches.iter().all(|branch| branch.readable));
        if index == 1 {
            assert_eq!(before, remote.snapshot_reads.get());
        }
    }
    let before = remote.snapshot_reads.get();
    assert!(
        check_snapshot_with_remote_cached(&store, &a, &remote, &first.state, true, &mut cache)
            .is_err()
    );
    assert_eq!(
        before,
        remote.snapshot_reads.get(),
        "unchanged conflict previews reuse validated safe summaries"
    );
    ConnectionStore::open(&a.config_dir)
        .unwrap()
        .replace_sessions(&SessionsConfig {
            connections: vec![local_connection("edited", "Edited", "bash")],
            ..Default::default()
        })
        .unwrap();
    assert!(
        check_snapshot_with_remote_cached(&store, &a, &remote, &first.state, true, &mut cache)
            .is_err()
    );
    assert_eq!(
        before + 2,
        remote.snapshot_reads.get(),
        "local changes invalidate preview differences"
    );
}

impl CloudSyncRemote for CountingRemote<'_> {
    fn provider(&self) -> &'static str {
        "memory"
    }
    fn create_dir(&self, path: &str) -> Result<(), CloudSyncError> {
        self.inner.create_dir(path)
    }
    fn read_if_exists(&self, path: &str) -> Result<Option<Vec<u8>>, CloudSyncError> {
        if path.contains("sync/snapshots/") {
            self.snapshot_reads.set(self.snapshot_reads.get() + 1);
        }
        self.inner.read_if_exists(path)
    }
    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
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
fn automatic_push_reuses_checked_snapshot_and_upload_ciphertext() {
    let dir = unique_temp_dir("sync-count-reads");
    let options = options(&dir, &dir, "device");
    let store = OptionsTestLocalStore(&options);
    let memory = MemoryRemote::default();
    let first =
        push_snapshot_with_remote(&store, &options, &memory, &CloudSyncState::default(), false)
            .unwrap();
    let remote = CountingRemote {
        inner: &memory,
        snapshot_reads: Cell::new(0),
    };
    let local = ConnectionStore::open(&options.config_dir).unwrap();
    local
        .replace_sessions(&SessionsConfig {
            connections: vec![local_connection("checked", "Checked edit", "bash")],
            ..SessionsConfig::default()
        })
        .unwrap();
    let checked =
        check_snapshot_with_remote_state(&store, &options, &remote, &first.state, true).unwrap();
    local
        .replace_sessions(&SessionsConfig {
            connections: vec![local_connection("later", "Later edit", "bash")],
            ..SessionsConfig::default()
        })
        .unwrap();
    let result = checked
        .push(
            &store,
            &options,
            &remote,
            &crate::cloud_sync::control::CloudSyncOperationControl::default(),
        )
        .unwrap();
    assert_eq!(
        remote.snapshot_reads.get(),
        2,
        "one existing snapshot plus one upload readback"
    );
    let revision = result.pointer.unwrap().revision_id;
    assert_eq!(
        memory
            .read_if_exists(&crate::cloud_sync::remote_path(
                &options.remote_root,
                &crate::cloud_sync::legacy_sync_snapshot_file(&revision)
            ))
            .unwrap(),
        memory
            .read_if_exists(&crate::cloud_sync::remote_path(
                &options.remote_root,
                crate::cloud_sync::SYNC_CURRENT_FILE
            ))
            .unwrap()
    );
    assert_eq!(
        check_snapshot_with_remote_state(&store, &options, &remote, &result.state, true)
            .unwrap()
            .decision,
        CloudRemoteCheckDecision::LocalChanged
    );
}

#[test]
fn checked_pull_still_rejects_local_edits_saved_after_the_check() {
    let source_dir = unique_temp_dir("checked-source");
    let target_dir = unique_temp_dir("checked-target");
    let source = options(&source_dir, &source_dir, "source");
    let target = options(&target_dir, &target_dir, "target");
    let remote = MemoryRemote::default();
    let first = push_snapshot_with_remote(
        &OptionsTestLocalStore(&source),
        &source,
        &remote,
        &CloudSyncState::default(),
        false,
    )
    .unwrap();
    let target_store = OptionsTestLocalStore(&target);
    let state = check_snapshot_with_remote_state(
        &target_store,
        &target,
        &remote,
        &CloudSyncState::default(),
        true,
    )
    .unwrap()
    .state;
    ConnectionStore::open(&source.config_dir)
        .unwrap()
        .replace_sessions(&SessionsConfig {
            connections: vec![local_connection("remote", "Remote edit", "bash")],
            ..SessionsConfig::default()
        })
        .unwrap();
    push_snapshot_with_remote(
        &OptionsTestLocalStore(&source),
        &source,
        &remote,
        &first.state,
        false,
    )
    .unwrap();
    let checked =
        check_snapshot_with_remote_state(&target_store, &target, &remote, &state, true).unwrap();
    assert_eq!(checked.decision, CloudRemoteCheckDecision::AutoPull);
    ConnectionStore::open(&target.config_dir)
        .unwrap()
        .replace_sessions(&SessionsConfig {
            connections: vec![local_connection("local", "Local edit", "bash")],
            ..SessionsConfig::default()
        })
        .unwrap();
    assert!(matches!(
        checked.pull(
            &target_store,
            &target,
            &remote,
            &|| Ok(()),
            &crate::cloud_sync::control::CloudSyncOperationControl::default()
        ),
        Err(CloudSyncError::LocalChangedDuringSync)
    ));
}

impl CloudSyncRemote for EditingRemote<'_> {
    fn provider(&self) -> &'static str {
        "memory"
    }
    fn create_dir(&self, path: &str) -> Result<(), CloudSyncError> {
        self.inner.create_dir(path)
    }
    fn read_if_exists(&self, path: &str) -> Result<Option<Vec<u8>>, CloudSyncError> {
        let bytes = self.inner.read_if_exists(path)?;
        if path.contains("sync/snapshots/")
            && let Some(edit) = self.edit.borrow_mut().take()
        {
            edit();
        }
        Ok(bytes)
    }
    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
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
fn vnc_and_unknown_entity_only_changes_are_uploaded_even_with_legacy_baselines() {
    for entity in ["vnc_known_hosts", "future_entity"] {
        let dir = unique_temp_dir("sync-complete-hash");
        let options = options(&dir, &dir.join("remote"), "device");
        let store = OptionsTestLocalStore(&options);
        let remote = MemoryRemote::default();
        let first =
            push_snapshot_with_remote(&store, &options, &remote, &CloudSyncState::default(), false)
                .unwrap();
        test_local_data()
            .lock()
            .unwrap()
            .get_mut(&options.config_dir)
            .unwrap()
            .entities
            .insert(entity.into(), "[]".into());
        let mut old_state = first.state.clone();
        old_state.last_synced_content_hash = None;
        for state in [&first.state, &old_state] {
            assert_eq!(
                check_snapshot_with_remote_state(&store, &options, &remote, state, true)
                    .unwrap()
                    .decision,
                CloudRemoteCheckDecision::LocalChanged
            );
        }
        let pushed =
            push_snapshot_with_remote(&store, &options, &remote, &old_state, false).unwrap();
        assert_eq!(pushed.outcome, CloudSyncOutcome::Uploaded);
        assert_eq!(
            first.state.last_synced_payload_hash,
            pushed.state.last_synced_payload_hash
        );
        assert_ne!(
            first.state.last_synced_content_hash,
            pushed.state.last_synced_content_hash
        );
    }
}

#[test]
fn check_baseline_never_absorbs_an_edit_saved_during_download() {
    let dir = unique_temp_dir("sync-check-edit");
    let options = options(&dir, &dir.join("remote"), "device");
    let store = OptionsTestLocalStore(&options);
    let remote = MemoryRemote::default();
    let first =
        push_snapshot_with_remote(&store, &options, &remote, &CloudSyncState::default(), false)
            .unwrap();
    let editing = EditingRemote {
        inner: &remote,
        edit: RefCell::new(Some(Box::new(|| {
            ConnectionStore::open(&options.config_dir)
                .unwrap()
                .replace_sessions(&SessionsConfig {
                    connections: vec![local_connection("new", "Saved during download", "bash")],
                    ..SessionsConfig::default()
                })
                .unwrap();
        }))),
    };
    let checked =
        check_snapshot_with_remote_state(&store, &options, &editing, &first.state, true).unwrap();
    assert_eq!(checked.decision, CloudRemoteCheckDecision::UpToDate);
    assert_eq!(
        checked.state.last_synced_content_hash,
        first.state.last_synced_content_hash
    );
    store.persist_cloud_sync_state(&checked.state).unwrap();
    let next =
        check_snapshot_with_remote_state(&store, &options, &remote, &checked.state, true).unwrap();
    assert_eq!(next.decision, CloudRemoteCheckDecision::LocalChanged);
}

#[test]
fn pull_refuses_to_overwrite_a_save_between_download_and_apply_even_when_forced() {
    let source_dir = unique_temp_dir("sync-pull-source");
    let target_dir = unique_temp_dir("sync-pull-target");
    let source = options(&source_dir, &source_dir.join("remote"), "source");
    let target = options(&target_dir, &target_dir.join("remote"), "target");
    ConnectionStore::open(&source.config_dir)
        .unwrap()
        .replace_sessions(&SessionsConfig {
            connections: vec![local_connection("remote", "Remote", "bash")],
            ..SessionsConfig::default()
        })
        .unwrap();
    let remote = MemoryRemote::default();
    push_snapshot_with_remote(
        &OptionsTestLocalStore(&source),
        &source,
        &remote,
        &CloudSyncState::default(),
        false,
    )
    .unwrap();
    let result = pull_snapshot_with_remote_guarded(
        &OptionsTestLocalStore(&target),
        &target,
        &remote,
        &CloudSyncState::default(),
        true,
        &|| {
            ConnectionStore::open(&target.config_dir)
                .unwrap()
                .replace_sessions(&SessionsConfig {
                    connections: vec![local_connection("local", "Concurrent save", "bash")],
                    ..SessionsConfig::default()
                })
                .unwrap();
            Ok(())
        },
    );
    assert!(matches!(
        result,
        Err(CloudSyncError::LocalChangedDuringSync)
    ));
    let target = ConnectionStore::open(&target.config_dir).unwrap();
    assert_eq!(
        target.load_sessions().unwrap().connections[0].name,
        "Concurrent save"
    );
    assert!(
        target
            .load_cloud_sync_state()
            .unwrap()
            .last_applied_remote_revision
            .is_none()
    );
}

#[test]
fn missing_remote_is_repopulated_even_when_local_baseline_is_unchanged() {
    let dir = unique_temp_dir("sync-remote-deleted");
    let options = options(&dir, &dir.join("remote"), "device");
    let store = OptionsTestLocalStore(&options);
    let first = push_snapshot_with_remote(
        &store,
        &options,
        &MemoryRemote::default(),
        &CloudSyncState::default(),
        false,
    )
    .unwrap();
    let result = push_snapshot_with_remote(
        &store,
        &options,
        &MemoryRemote::default(),
        &first.state,
        false,
    )
    .unwrap();
    assert_eq!(result.outcome, CloudSyncOutcome::Uploaded);
}
