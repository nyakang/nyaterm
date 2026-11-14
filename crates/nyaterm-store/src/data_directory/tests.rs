use std::fs;

use crate::ConnectionStore;
use crate::data_directory::{
    DATABASE_FILE_NAME, DataDirectoryError, DataRootLease, JOURNAL_NAME, LOCATION_FILE_NAME,
    TEST_FAILURE, linux_network_mount, open_startup, prepare_switch, queue_switch, read_location,
    set_active_data_dir, write_location,
};
use nyaterm_core::app_identity::AppFlavor;
use nyaterm_core::test_support::TestTempDir;
use nyaterm_core::{SavedConnection, SavedPassword};

fn fixture(path: &std::path::Path) {
    let store = ConnectionStore::open(path).unwrap();
    let connection: SavedConnection = serde_json::from_value(serde_json::json!({
        "id":"fixture", "name":"fixture", "type":"ssh", "host":"example.invalid", "port":22,
        "future_field":{"version":7}
    }))
    .unwrap();
    store.save_connection(&connection).unwrap();
    let password: SavedPassword = serde_json::from_value(serde_json::json!({"id":"password", "name":"fixture", "password":"synthetic-migration-secret"})).unwrap();
    store.save_password(password).unwrap();
    store
        .upsert_known_host("example.invalid ssh-ed25519 AAAA")
        .unwrap();
    fs::create_dir_all(path.join("plugins/nested")).unwrap();
    fs::write(path.join("plugins/nested/plugin.json"), b"fixture plugin").unwrap();
    fs::create_dir_all(path.join("recordings")).unwrap();
    fs::write(path.join("recordings/session.cast"), b"fixture recording").unwrap();
}

#[test]
fn missing_active_directory_never_initializes_an_empty_replacement() {
    let root = TestTempDir::new("nyaterm-data-directory-missing");
    let local = root.join("local");
    let custom = root.join("custom");
    fixture(&local);
    fixture(&custom);
    set_active_data_dir(&local, Some(&custom)).unwrap();
    fs::rename(&custom, root.join("moved")).unwrap();
    assert!(open_startup(&local, &local, AppFlavor::Stable).is_err());
    assert!(!custom.exists());
    assert_eq!(
        read_location(&local).unwrap().unwrap().data_dir,
        Some(custom.to_string_lossy().into_owned())
    );
}

#[test]
fn missing_active_and_missing_fallback_require_recovery() {
    let root = TestTempDir::new("nyaterm-data-directory-no-fallback");
    let local = root.join("local");
    let custom = root.join("custom");
    set_active_data_dir(&local, Some(&custom)).unwrap();
    assert!(open_startup(&local, &local, AppFlavor::Stable).is_err());
    assert!(!local.join(DATABASE_FILE_NAME).exists());
    assert!(!custom.exists());
}

#[test]
fn corrupt_adoption_is_rejected_before_pending_is_written() {
    let root = TestTempDir::new("nyaterm-data-directory-bad-adoption");
    let local = root.join("local");
    let target = root.join("target");
    fixture(&local);
    fs::create_dir(&target).unwrap();
    fs::write(target.join(DATABASE_FILE_NAME), b"not redb").unwrap();
    assert!(queue_switch(&local, &local, &target, AppFlavor::Stable).is_err());
    assert!(!local.join(LOCATION_FILE_NAME).exists());
    assert_eq!(
        fs::read(target.join(DATABASE_FILE_NAME)).unwrap(),
        b"not redb"
    );
}

#[test]
fn first_run_can_create_a_database_without_creating_configured_missing_roots() {
    let root = TestTempDir::new("nyaterm-data-directory-first-run");
    let local = root.join("local");
    let opened = open_startup(&local, &local, AppFlavor::Stable).unwrap();
    assert!(opened.store.db_path().exists());
    assert!(!opened.selection.custom);
}

#[test]
fn complete_migration_preserves_credentials_unknown_fields_plugins_and_recordings() {
    let root = TestTempDir::new("nyaterm-data-directory-complete");
    let local = root.join("local");
    fixture(&local);
    let target = root.join("target/.nyaterm");
    queue_switch(&local, &local, &target, AppFlavor::Stable).unwrap();
    let opened = open_startup(&local, &local, AppFlavor::Stable).unwrap();
    assert_eq!(
        opened.selection.data_dir,
        fs::canonicalize(&target).unwrap()
    );
    let connection = opened.store.list_connections().unwrap().remove(0);
    assert_eq!(
        serde_json::to_value(connection).unwrap()["future_field"]["version"],
        7
    );
    let password = opened
        .store
        .load_decrypted_password_by_id("password")
        .unwrap()
        .unwrap();
    assert_eq!(
        password.password.unwrap().expose_secret(),
        "synthetic-migration-secret"
    );
    assert_eq!(
        fs::read(target.join("plugins/nested/plugin.json")).unwrap(),
        b"fixture plugin"
    );
    assert_eq!(
        fs::read(target.join("recordings/session.cast")).unwrap(),
        b"fixture recording"
    );
    assert!(
        opened
            .store
            .render_known_hosts_export()
            .unwrap()
            .contains("example.invalid ssh-ed25519 AAAA")
    );
    assert!(local.join(DATABASE_FILE_NAME).exists());
    assert!(!target.join(JOURNAL_NAME).exists());
}

#[test]
fn failure_before_publication_leaves_no_adoptable_database() {
    let root = TestTempDir::new("nyaterm-data-directory-before-publish");
    let local = root.join("local");
    fixture(&local);
    let target = root.join("target/.nyaterm");
    queue_switch(&local, &local, &target, AppFlavor::Stable).unwrap();
    TEST_FAILURE.with(|point| point.set(Some("copied")));
    assert!(open_startup(&local, &local, AppFlavor::Stable).is_err());
    assert!(!target.join(DATABASE_FILE_NAME).exists());
}

#[test]
fn partial_publication_cannot_be_adopted_and_retains_its_migration_identity() {
    let root = TestTempDir::new("nyaterm-data-directory-partial");
    let local = root.join("local");
    fixture(&local);
    let target = root.join("target/.nyaterm");
    queue_switch(&local, &local, &target, AppFlavor::Stable).unwrap();
    let id = read_location(&local)
        .unwrap()
        .unwrap()
        .pending
        .unwrap()
        .migration_id;
    TEST_FAILURE.with(|point| point.set(Some("published_plugin")));
    assert!(open_startup(&local, &local, AppFlavor::Stable).is_err());
    assert!(!target.join(DATABASE_FILE_NAME).exists());
    assert!(target.join(JOURNAL_NAME).exists());
    assert!(matches!(
        prepare_switch(&local, &local, &target, AppFlavor::Stable),
        Err(DataDirectoryError::RecoveryRequired(_))
    ));
    assert_eq!(
        read_location(&local)
            .unwrap()
            .unwrap()
            .pending
            .unwrap()
            .migration_id,
        id
    );
}

#[test]
fn pointer_commit_failure_keeps_the_original_pointer_and_source() {
    let root = TestTempDir::new("nyaterm-data-directory-pointer");
    let local = root.join("local");
    fixture(&local);
    let target = root.join("target/.nyaterm");
    queue_switch(&local, &local, &target, AppFlavor::Stable).unwrap();
    TEST_FAILURE.with(|point| point.set(Some("pointer")));
    assert!(open_startup(&local, &local, AppFlavor::Stable).is_err());
    let location = read_location(&local).unwrap().unwrap();
    assert!(location.data_dir.is_none());
    assert!(location.pending.is_some());
    assert!(local.join(DATABASE_FILE_NAME).is_file());
}

#[test]
fn data_root_lease_is_shared_across_installation_directories_and_held_until_drop() {
    let root = TestTempDir::new("nyaterm-data-directory-lease");
    let target = root.join("target");
    let lease = DataRootLease::acquire(&target).unwrap();
    assert!(matches!(
        DataRootLease::acquire(&target),
        Err(DataDirectoryError::InUse(_))
    ));
    drop(lease);
    assert!(DataRootLease::acquire(&target).is_ok());
    assert_eq!(
        crate::data_directory::instance_lock_dir(&root.join("stable"), &target),
        crate::data_directory::instance_lock_dir(&root.join("compat"), &target)
    );
}

#[test]
fn location_extensions_survive_a_pending_update() {
    let root = TestTempDir::new("nyaterm-data-directory-location-compat");
    let local = root.join("local");
    fixture(&local);
    let location =
        serde_json::from_value(serde_json::json!({"version":1,"future":{"v":9}})).unwrap();
    write_location(&local, &location).unwrap();
    queue_switch(&local, &local, &root.join("target"), AppFlavor::Stable).unwrap();
    let value = serde_json::to_value(read_location(&local).unwrap().unwrap()).unwrap();
    assert_eq!(value["future"]["v"], 9);
}

#[test]
fn linux_mount_detection_uses_mountpoint_and_filesystem_columns_and_longest_match() {
    let mounts = "server:/ /mnt/team\\040share nfs4 rw 0 0\n/dev/sda / ext4 rw 0 0\n/dev/sdb /mnt/team\\040share/local ext4 rw 0 0\n";
    assert!(linux_network_mount(
        std::path::Path::new("/mnt/team share/data"),
        mounts
    ));
    assert!(!linux_network_mount(
        std::path::Path::new("/mnt/team share/local/data"),
        mounts
    ));
    assert!(!linux_network_mount(
        std::path::Path::new("/mnt/team share-other/data"),
        mounts
    ));
}

#[cfg(target_os = "macos")]
#[test]
fn macos_mount_detection_accepts_local_directories_that_do_not_exist_yet() {
    let root = TestTempDir::new("nyaterm-macos-local-mount");
    fs::create_dir_all(root.path()).unwrap();
    let existing = root.path();
    let missing = root.path().join("not").join("created");
    assert!(!crate::data_directory::is_network_location(existing).unwrap());
    assert!(!crate::data_directory::is_network_location(&missing).unwrap());
}

#[cfg(unix)]
#[test]
fn special_plugin_entries_fail_without_publishing_the_database() {
    let root = TestTempDir::new("nyaterm-data-directory-special");
    let local = root.join("local");
    fixture(&local);
    let _socket = std::os::unix::net::UnixListener::bind(local.join("plugins/socket")).unwrap();
    let target = root.join("target/.nyaterm");
    queue_switch(&local, &local, &target, AppFlavor::Stable).unwrap();
    assert!(open_startup(&local, &local, AppFlavor::Stable).is_err());
    assert!(!target.join(DATABASE_FILE_NAME).exists());
}

#[test]
fn defaults_updates_keep_unknown_nested_fields_and_other_protocols() {
    let root = TestTempDir::new("nyaterm-default-session-compat");
    let store = ConnectionStore::open(root.path()).unwrap();
    let original = serde_json::json!({"id":"defaults", "name":"", "type":"ssh", "host":"127.0.0.1", "future":9, "sftp":{"future_nested":7}});
    store
        .save_default_session_settings("ssh", &original)
        .unwrap();
    let mut updated = original.clone();
    updated.as_object_mut().unwrap().remove("future");
    updated["sftp"]
        .as_object_mut()
        .unwrap()
        .remove("future_nested");
    store
        .save_default_session_settings("ssh", &updated)
        .unwrap();
    let loaded = store.load_default_session_settings().unwrap().unwrap();
    assert_eq!(loaded["ssh"]["future"], 9);
    assert_eq!(loaded["ssh"]["sftp"]["future_nested"], 7);
}

#[test]
fn interrupted_publication_retries_the_same_transaction_without_adopting_partial_data() {
    let root = TestTempDir::new("nyaterm-data-directory-retry");
    let local = root.join("local");
    fixture(&local);
    let target = root.join("target/.nyaterm");
    queue_switch(&local, &local, &target, AppFlavor::Stable).unwrap();
    TEST_FAILURE.with(|point| point.set(Some("published_plugin")));
    assert!(open_startup(&local, &local, AppFlavor::Stable).is_err());
    let opened = open_startup(&local, &local, AppFlavor::Stable).unwrap();
    assert_eq!(
        fs::canonicalize(opened.selection.data_dir).unwrap(),
        fs::canonicalize(&target).unwrap()
    );
    assert!(target.join("recordings/session.cast").is_file());
}

#[test]
fn every_publication_boundary_retries_without_losing_source_data() {
    for point in [
        "file_copy",
        "file_sync",
        "file_verify",
        "metadata_sync",
        "directory_sync",
        "staged",
        "journaled",
        "published_plugin",
        "published_recording",
        "published_database",
        "ready",
        "pointer",
    ] {
        let root = TestTempDir::new("nyaterm-data-directory-fault-boundaries");
        let local = root.join("local");
        fixture(&local);
        let target = root.join("target/.nyaterm");
        queue_switch(&local, &local, &target, AppFlavor::Stable).unwrap();
        TEST_FAILURE.with(|failure| failure.set(Some(point)));
        let failed = open_startup(&local, &local, AppFlavor::Stable)
            .err()
            .expect("recovery required");
        assert!(
            TEST_FAILURE.with(|failure| failure.get().is_none()),
            "migration did not reach {point}: {:?}",
            failed
        );

        assert!(
            read_location(&local).unwrap().unwrap().data_dir.is_none(),
            "{point}"
        );
        drop(failed);
        let retried = open_startup(&local, &local, AppFlavor::Stable).unwrap();
        assert_eq!(
            retried.selection.data_dir,
            fs::canonicalize(&target).unwrap(),
            "{point}: {:?}",
            retried.selection.notice
        );
        assert_eq!(
            fs::read(target.join("recordings/session.cast")).unwrap(),
            b"fixture recording"
        );
        assert!(local.join(DATABASE_FILE_NAME).is_file());
    }
}

#[test]
fn retry_rejects_changed_source_or_published_inventory() {
    for change in [
        "database",
        "source_file",
        "source_added",
        "target_file",
        "target_added",
    ] {
        let root = TestTempDir::new("nyaterm-data-directory-changed-retry");
        let local = root.join("local");
        fixture(&local);
        let target = root.join("target/.nyaterm");
        queue_switch(&local, &local, &target, AppFlavor::Stable).unwrap();
        TEST_FAILURE.with(|failure| failure.set(Some("published_plugin")));
        assert!(open_startup(&local, &local, AppFlavor::Stable).is_err());
        match change {
            "database" => {
                let store = ConnectionStore::open(&local).unwrap();
                let mut connection = store.list_connections().unwrap().remove(0);
                connection.name = "changed connection".into();
                store.save_connection(&connection).unwrap();
            }
            "source_file" => {
                fs::write(local.join("plugins/nested/plugin.json"), b"changed").unwrap()
            }
            "source_added" => fs::write(local.join("plugins/added"), b"added").unwrap(),
            "target_file" => {
                fs::write(target.join("plugins/nested/plugin.json"), b"changed").unwrap()
            }
            "target_added" => fs::write(target.join("plugins/added"), b"added").unwrap(),
            _ => unreachable!(),
        }
        assert!(
            open_startup(&local, &local, AppFlavor::Stable).is_err(),
            "{change}"
        );
        assert!(!target.join(DATABASE_FILE_NAME).exists(), "{change}");
    }
}

#[test]
fn migration_lock_excludes_redb_in_another_process() {
    const CHILD_PATH: &str = "NYATERM_TEST_MIGRATION_LOCK_PATH";
    if let Some(path) = std::env::var_os(CHILD_PATH) {
        assert!(matches!(
            redb::Database::open(path),
            Err(redb::DatabaseError::DatabaseAlreadyOpen)
        ));
        return;
    }
    let root = TestTempDir::new("nyaterm-data-directory-process-lock");
    fixture(root.path());
    let path = root.join(DATABASE_FILE_NAME);
    let before = fs::read(&path).unwrap();
    let locked = crate::data_directory::LockedDatabase::open(&path).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "data_directory::tests::migration_lock_excludes_redb_in_another_process",
            "--nocapture",
        ])
        .env(CHILD_PATH, &path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    drop(locked);
    assert_eq!(before, fs::read(&path).unwrap());
    assert!(redb::Database::open(path).is_ok());
}

#[test]
fn active_redb_writer_prevents_migration_and_adoption() {
    let root = TestTempDir::new("nyaterm-data-directory-live-writer");
    let local = root.join("local");
    fixture(&local);
    let target = root.join("target/.nyaterm");
    queue_switch(&local, &local, &target, AppFlavor::Stable).unwrap();
    let _writer = ConnectionStore::open(&local).unwrap();
    assert!(matches!(
        open_startup(&local, &local, AppFlavor::Stable),
        Err(DataDirectoryError::InUse(_))
    ));
    assert!(matches!(
        prepare_switch(&target, &target, &local, AppFlavor::Stable),
        Err(DataDirectoryError::InUse(_))
    ));
    assert!(!target.join(DATABASE_FILE_NAME).exists());
}

#[test]
fn marker_only_and_future_marker_versions_cannot_be_adopted() {
    let root = TestTempDir::new("nyaterm-data-directory-marker-validation");
    let local = root.join("local");
    fixture(&local);
    let target = root.join("target");
    fs::create_dir(&target).unwrap();
    let marker = target.join(nyaterm_core::data_directory::DATA_MARKER_FILE_NAME);
    for version in [1, 99] {
        fs::write(
            &marker,
            serde_json::to_vec(
                &serde_json::json!({"version":version,"flavor":AppFlavor::Stable.desktop_id()}),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(queue_switch(&local, &local, &target, AppFlavor::Stable).is_err());
        assert!(!local.join(LOCATION_FILE_NAME).exists());
    }
}

#[test]
fn prepared_adoption_is_revalidated_before_pointer_commit() {
    let root = TestTempDir::new("nyaterm-data-directory-adopt-revalidate");
    let local = root.join("local");
    let target = root.join("target");
    fixture(&local);
    fixture(&target);
    queue_switch(&local, &local, &target, AppFlavor::Stable).unwrap();
    fs::write(
        target.join(DATABASE_FILE_NAME),
        b"corrupted since selection",
    )
    .unwrap();
    assert!(open_startup(&local, &local, AppFlavor::Stable).is_err());
    assert!(read_location(&local).unwrap().unwrap().data_dir.is_none());
}

#[test]
fn restart_does_not_silently_adopt_a_target_that_changed_after_confirmation() {
    let root = TestTempDir::new("nyaterm-data-directory-choice-changed");
    let local = root.join("local");
    fixture(&local);
    let choice = prepare_switch(&local, &local, &root.join("target"), AppFlavor::Stable).unwrap();
    fixture(&choice.target);
    assert!(
        crate::data_directory::queue_prepared_switch(&local, &local, &choice, AppFlavor::Stable)
            .is_err()
    );
    assert!(!local.join(LOCATION_FILE_NAME).exists());
}

#[test]
fn migration_preserves_master_password_wrapping_and_rejects_invalid_passwords() {
    let root = TestTempDir::new("nyaterm-data-directory-master-wrapper");
    let local = root.join("local");
    fixture(&local);
    {
        let store = ConnectionStore::open(&local).unwrap();
        store
            .save_master_password(Some("synthetic-master-password"))
            .unwrap();
    }
    let target = root.join("target");
    queue_switch(&local, &local, &target, AppFlavor::Stable).unwrap();
    let opened = open_startup(&local, &local, AppFlavor::Stable).unwrap();
    assert!(
        opened
            .store
            .verify_master_password("synthetic-master-password")
            .unwrap()
    );
    assert!(
        !opened
            .store
            .verify_master_password("wrong-password")
            .unwrap()
    );
    assert!(
        opened
            .store
            .load_decrypted_password_by_id("password")
            .unwrap()
            .is_some()
    );
}

#[test]
fn migration_loads_a_supported_legacy_database_with_only_text_documents() {
    let root = TestTempDir::new("nyaterm-data-directory-legacy-text");
    let local = root.join("local");
    fs::create_dir_all(&local).unwrap();
    {
        let db = redb::Database::create(local.join(DATABASE_FILE_NAME)).unwrap();
        let txn = db.begin_write().unwrap();
        txn.open_table(redb::TableDefinition::<&str, &str>::new("text_docs"))
            .unwrap()
            .insert("known_hosts", "legacy.example.invalid ssh-rsa AAAA")
            .unwrap();
        txn.commit().unwrap();
    }
    let choice = queue_switch(&local, &local, &root.join("target"), AppFlavor::Stable).unwrap();
    let opened = open_startup(&local, &local, AppFlavor::Stable).unwrap();
    assert_eq!(opened.selection.data_dir, choice.target);
    assert!(
        opened
            .store
            .render_known_hosts_export()
            .unwrap()
            .contains("legacy.example.invalid ssh-rsa AAAA")
    );
    assert!(local.join(DATABASE_FILE_NAME).is_file());
}

#[test]
fn explicit_recovery_selection_persists_and_does_not_switch_back_when_disk_returns() {
    use crate::data_directory::{commit_recovery_selection, prepare_recovery_selection};
    let root = TestTempDir::new("nyaterm-data-directory-explicit-recovery");
    let local = root.join("local");
    let custom = root.join("custom");
    fixture(&local);
    fixture(&custom);
    set_active_data_dir(&local, Some(&custom)).unwrap();
    fs::rename(&custom, root.join("unmounted")).unwrap();
    assert!(open_startup(&local, &local, AppFlavor::Stable).is_err());
    let choice = prepare_recovery_selection(&local, &local, AppFlavor::Stable).unwrap();
    commit_recovery_selection(&local, &choice, AppFlavor::Stable).unwrap();
    let opened = open_startup(&local, &local, AppFlavor::Stable).unwrap();
    let mut connection = opened.store.list_connections().unwrap().remove(0);
    connection.name = "edited after explicit recovery".into();
    opened.store.save_connection(&connection).unwrap();
    drop(opened);
    fs::rename(root.join("unmounted"), &custom).unwrap();
    let reopened = open_startup(&local, &local, AppFlavor::Stable).unwrap();
    assert_eq!(reopened.selection.data_dir, local);
    assert_eq!(
        reopened.store.list_connections().unwrap()[0].name,
        connection.name
    );
}

#[test]
fn returning_to_default_archives_history_and_recovers_at_every_boundary() {
    use crate::data_directory::{ensure_data_marker, prepare_migration, queue_prepared_switch};
    for failure in [
        None,
        Some("staged"),
        Some("journaled"),
        Some("archived_plugins"),
        Some("archived_recordings"),
        Some("archived_marker"),
        Some("archived_database"),
        Some("published_plugin"),
        Some("published_recording"),
        Some("published_database"),
        Some("ready"),
        Some("pointer"),
    ] {
        let root = TestTempDir::new("nyaterm-data-directory-return");
        let local = root.join("local");
        fixture(&local);
        ensure_data_marker(&local, AppFlavor::Stable).unwrap();
        let destination =
            queue_switch(&local, &local, &root.join("custom"), AppFlavor::Stable).unwrap();
        let active = open_startup(&local, &local, AppFlavor::Stable).unwrap();
        let mut connection = active.store.list_connections().unwrap().remove(0);
        connection.name = "current data".into();
        active.store.save_connection(&connection).unwrap();
        fs::write(
            destination.target.join("recordings/new.cast"),
            b"new recording",
        )
        .unwrap();
        fs::write(local.join("unrelated.txt"), b"leave alone").unwrap();
        let choice =
            prepare_migration(&destination.target, &local, &local, AppFlavor::Stable).unwrap();
        assert_eq!(
            choice.mode,
            crate::data_directory::SwitchMode::ReplaceWithBackup
        );
        queue_prepared_switch(&local, &destination.target, &choice, AppFlavor::Stable).unwrap();
        let id = read_location(&local)
            .unwrap()
            .unwrap()
            .pending
            .unwrap()
            .migration_id
            .unwrap();
        drop(active);
        if let Some(failure) = failure {
            TEST_FAILURE.with(|point| point.set(Some(failure)));
            let result = open_startup(&local, &local, AppFlavor::Stable);
            assert!(result.is_err(), "{failure}");
            assert!(
                TEST_FAILURE.with(|point| point.get().is_none()),
                "{failure}"
            );
        }
        let restored = open_startup(&local, &local, AppFlavor::Stable).unwrap();
        assert_eq!(restored.selection.data_dir, local);
        assert_eq!(
            restored.store.list_connections().unwrap()[0].name,
            "current data"
        );
        assert_eq!(
            fs::read(local.join("recordings/new.cast")).unwrap(),
            b"new recording"
        );
        assert_eq!(
            fs::read(local.join("unrelated.txt")).unwrap(),
            b"leave alone"
        );
        let backup = local.join("data-directory-backups").join(id.to_string());
        let history = ConnectionStore::open_existing(&backup).unwrap();
        assert_eq!(history.list_connections().unwrap()[0].name, "fixture");
        assert!(!backup.join("recordings/new.cast").exists());
        assert!(destination.target.join(DATABASE_FILE_NAME).exists());
    }
}

#[test]
fn a_changed_existing_target_invalidates_confirmation_without_overwriting_it() {
    use crate::data_directory::{prepare_migration, queue_prepared_switch};
    let root = TestTempDir::new("nyaterm-data-directory-changed-history");
    let local = root.join("local");
    let custom = root.join("custom");
    fixture(&local);
    fixture(&custom);
    let choice = prepare_migration(&custom, &local, &local, AppFlavor::Stable).unwrap();
    fs::write(local.join("plugins/added"), b"changed after confirmation").unwrap();
    assert!(queue_prepared_switch(&local, &custom, &choice, AppFlavor::Stable).is_err());
    assert!(!local.join("data-directory-backups").exists());
    assert!(!local.join(LOCATION_FILE_NAME).exists());
}

#[test]
fn restoring_default_does_not_mix_historical_or_unexpected_managed_trees_into_current_data() {
    use crate::data_directory::{prepare_migration, queue_prepared_switch};
    for unexpected in [false, true] {
        let root = TestTempDir::new("nyaterm-data-directory-absent-tree");
        let local = root.join("local");
        let custom = root.join("custom");
        fixture(&local);
        fixture(&custom);
        fs::remove_dir_all(custom.join("plugins")).unwrap();
        fs::remove_dir_all(custom.join("recordings")).unwrap();
        set_active_data_dir(&local, Some(&custom)).unwrap();
        let choice = prepare_migration(&custom, &local, &local, AppFlavor::Stable).unwrap();
        queue_prepared_switch(&local, &custom, &choice, AppFlavor::Stable).unwrap();
        let id = read_location(&local)
            .unwrap()
            .unwrap()
            .pending
            .unwrap()
            .migration_id
            .unwrap();
        TEST_FAILURE.with(|point| point.set(Some("archived_database")));
        assert!(open_startup(&local, &local, AppFlavor::Stable).is_err());
        if unexpected {
            fs::create_dir(local.join("recordings")).unwrap();
            fs::write(local.join("recordings/unexpected.cast"), b"unexpected").unwrap();
        }
        let result = open_startup(&local, &local, AppFlavor::Stable);
        if unexpected {
            assert!(result.is_err());
            assert!(read_location(&local).unwrap().unwrap().pending.is_some());
            assert_eq!(
                fs::read(local.join("recordings/unexpected.cast")).unwrap(),
                b"unexpected"
            );
        } else {
            assert_eq!(result.unwrap().selection.data_dir, local);
            assert!(!local.join("plugins").exists());
            assert!(!local.join("recordings").exists());
        }
        let backup = local.join("data-directory-backups").join(id.to_string());
        assert_eq!(
            fs::read(backup.join("recordings/session.cast")).unwrap(),
            b"fixture recording"
        );
        assert!(backup.join("plugins/nested/plugin.json").is_file());
        assert!(custom.join(DATABASE_FILE_NAME).is_file());
    }
}

#[test]
fn cancelling_an_interrupted_migration_persists_source_selection() {
    use crate::data_directory::{commit_recovery_selection, prepare_recovery_selection};
    let root = TestTempDir::new("nyaterm-cancel-migration");
    let local = root.join("local");
    fixture(&local);
    let target = queue_switch(&local, &local, &root.join("target"), AppFlavor::Stable)
        .unwrap()
        .target;
    TEST_FAILURE.with(|point| point.set(Some("published_plugin")));
    assert!(open_startup(&local, &local, AppFlavor::Stable).is_err());
    let choice = prepare_recovery_selection(&local, &local, AppFlavor::Stable).unwrap();
    commit_recovery_selection(&local, &choice, AppFlavor::Stable).unwrap();
    let location = read_location(&local).unwrap().unwrap();
    assert!(location.pending.is_none());
    assert!(target.join(JOURNAL_NAME).exists());
    let source = open_startup(&local, &local, AppFlavor::Stable).unwrap();
    assert_eq!(source.selection.data_dir, local);
    assert!(!target.join(DATABASE_FILE_NAME).exists());
}

#[test]
fn future_migration_journal_never_publishes_or_archives_data() {
    let root = TestTempDir::new("nyaterm-future-migration-journal");
    let local = root.join("local");
    fixture(&local);
    let target = queue_switch(&local, &local, &root.join("target"), AppFlavor::Stable)
        .unwrap()
        .target;
    TEST_FAILURE.with(|point| point.set(Some("journaled")));
    assert!(open_startup(&local, &local, AppFlavor::Stable).is_err());
    let path = target.join(JOURNAL_NAME);
    let mut journal: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    journal["version"] = serde_json::json!(99);
    let bytes = serde_json::to_vec(&journal).unwrap();
    fs::write(&path, &bytes).unwrap();
    assert!(open_startup(&local, &local, AppFlavor::Stable).is_err());
    assert_eq!(fs::read(path).unwrap(), bytes);
    assert!(local.join(DATABASE_FILE_NAME).exists());
    assert!(!target.join(DATABASE_FILE_NAME).exists());
}
