use nyaterm_core::sync_schema::encryption;
use nyaterm_core::test_support::TestTempDir;
use nyaterm_sync::protocol::PendingPublication;
use redb::ReadableDatabase;

use super::tests::{connection, enable, legacy_bytes, migration_record, prepared};
use super::{EDITS, GENERATION, LOCAL_REPLICA_MIGRATION, MIGRATION_RECORDS, REPLICAS};
use crate::maintenance::StorageCompactionOutcome;
use crate::storage::{ConnectionStore, META_TABLE};

#[test]
fn migration_preserves_pending_publications_and_the_slowest_mixed_format_destination() {
    let dir = TestTempDir::new("nyaterm-migration-shared-journal");
    let mut store = ConnectionStore::open(dir.path()).unwrap();
    store
        .save_master_password(Some("fixture-password"))
        .unwrap();
    let mut fast = migration_record(7);
    let bytes = encryption::seal(
        "fixture-password",
        "publication",
        "fixture/1",
        b"publication fixture",
    )
    .unwrap();
    fast.pending =
        Some(PendingPublication::new(fast.writer, 1, bytes, "document fixture".into()).unwrap());
    let slow = migration_record(3);
    let old = legacy_bytes(&fast, "fast");
    let compressed = encryption::seal_compressed(
        "fixture-password",
        "local-replica",
        "slow",
        &serde_json::to_vec(&slow).unwrap(),
    )
    .unwrap();
    let txn = store.db.begin_write().unwrap();
    txn.open_table(META_TABLE)
        .unwrap()
        .insert(GENERATION, "9")
        .unwrap();
    // Isolate the journal fixture from initialization's password-setting edits.
    txn.delete_table(EDITS).unwrap();
    {
        let mut table = txn.open_table(REPLICAS).unwrap();
        table.insert("fast", old.as_slice()).unwrap();
        table.insert("slow", compressed.as_slice()).unwrap();
    }
    {
        let mut table = txn.open_table(EDITS).unwrap();
        table.insert("consumed", 3).unwrap();
        table.insert("slow-still-needs", 7).unwrap();
        table.insert("newer", 9).unwrap();
    }
    txn.commit().unwrap();
    let result = store.maintain_local_storage().unwrap();
    assert_eq!(result.replicas_rewritten, 1);
    assert_eq!(result.journal_rows_removed, 1);
    assert!(matches!(
        result.compaction,
        StorageCompactionOutcome::Completed { .. }
    ));
    assert!(
        store
            .load_v4_replica("fast", "fixture-password")
            .unwrap()
            .unwrap()
            .0
            == fast
    );
    assert!(
        store
            .load_v4_replica("slow", "fixture-password")
            .unwrap()
            .unwrap()
            .0
            == slow
    );
    let txn = store.db.begin_read().unwrap();
    let edits = txn.open_table(EDITS).unwrap();
    assert!(edits.get("consumed").unwrap().is_none());
    assert_eq!(edits.get("slow-still-needs").unwrap().unwrap().value(), 7);
    assert_eq!(edits.get("newer").unwrap().unwrap().value(), 9);
    assert_eq!(
        txn.open_table(REPLICAS)
            .unwrap()
            .get("slow")
            .unwrap()
            .unwrap()
            .value(),
        compressed.as_slice()
    );
}

#[test]
fn migration_compaction_failure_resumes_after_reopen_without_rewriting_records() {
    let dir = TestTempDir::new("nyaterm-migration-recovery");
    let hash;
    {
        let mut store = ConnectionStore::open(dir.path()).unwrap();
        store
            .save_master_password(Some("fixture-password"))
            .unwrap();
        let old = legacy_bytes(&migration_record(0), "destination");
        let txn = store.db.begin_write().unwrap();
        txn.open_table(REPLICAS)
            .unwrap()
            .insert("destination", old.as_slice())
            .unwrap();
        txn.commit().unwrap();
        let read = store.db.begin_read().unwrap();
        let result = store.maintain_local_storage().unwrap();
        assert_eq!(result.compaction, StorageCompactionOutcome::Failed);
        assert_eq!(result.replicas_rewritten, 1);
        let txn = store.db.begin_read().unwrap();
        assert_eq!(
            txn.open_table(META_TABLE)
                .unwrap()
                .get(LOCAL_REPLICA_MIGRATION)
                .unwrap()
                .unwrap()
                .value(),
            MIGRATION_RECORDS
        );
        hash = store
            .load_v4_replica("destination", "fixture-password")
            .unwrap()
            .unwrap()
            .1;
        drop(read);
    }
    let mut store = ConnectionStore::open(dir.path()).unwrap();
    let result = store.maintain_local_storage().unwrap();
    assert_eq!(result.replicas_rewritten, 0);
    assert!(matches!(
        result.compaction,
        StorageCompactionOutcome::Completed { .. }
    ));
    assert_eq!(
        store
            .load_v4_replica("destination", "fixture-password")
            .unwrap()
            .unwrap()
            .1,
        hash
    );
    assert!(!store.migrate_local_replica_storage().unwrap());
}

#[test]
fn already_compressed_records_without_consumed_edits_do_not_force_compaction() {
    let dir = TestTempDir::new("nyaterm-migration-noop");
    let mut store = ConnectionStore::open(dir.path()).unwrap();
    store
        .save_master_password(Some("fixture-password"))
        .unwrap();
    store
        .persist_v4_record(
            "destination",
            "fixture-password",
            None,
            &migration_record(0),
        )
        .unwrap();
    let result = store.maintain_local_storage().unwrap();
    assert_eq!(result.replicas_rewritten, 0);
    assert_eq!(result.journal_rows_removed, 0);
    assert_eq!(result.compaction, StorageCompactionOutcome::NotNeeded);
    assert!(!store.migrate_local_replica_storage().unwrap());
}

#[test]
fn maintenance_preserves_a_staged_password_rotation_and_its_exact_retry_ticket() {
    let dir = TestTempDir::new("nyaterm-migration-rotation");
    let mut store = ConnectionStore::open(dir.path()).unwrap();
    store.save_master_password(Some("password")).unwrap();
    enable(&store);
    store.save_connection(&connection()).unwrap();
    let mut initial = prepared(&store);
    initial.local.migration_approved = true;
    store.apply_v4_plan(initial, "password").unwrap();
    let (record, _) = store
        .load_v4_replica("destination", "password")
        .unwrap()
        .unwrap();
    // A legacy ciphertext is deliberately kept as a migration candidate.
    let old = encryption::seal(
        "password",
        "local-replica",
        "destination",
        &serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let txn = store.db.begin_write().unwrap();
    txn.open_table(REPLICAS)
        .unwrap()
        .insert("destination", old.as_slice())
        .unwrap();
    txn.commit().unwrap();
    let staged = store
        .stage_v4_password_rotation(
            prepared(&store),
            "next-password",
            "next-destination",
            "next-space",
        )
        .unwrap();
    let result = store.maintain_local_storage().unwrap();
    assert_eq!(result.replicas_rewritten, 0);
    let retried = store
        .load_v4_password_rotation("destination", "next-password")
        .unwrap()
        .unwrap();
    assert_eq!(retried.ticket, staged.ticket);
    assert!(retried.pending == staged.pending);
    assert_eq!(
        store
            .db
            .begin_read()
            .unwrap()
            .open_table(REPLICAS)
            .unwrap()
            .get("destination")
            .unwrap()
            .unwrap()
            .value(),
        old.as_slice()
    );
    store
        .commit_verified_v4_password_rotation(&retried.ticket)
        .unwrap();
}

#[test]
fn unsupported_replica_aborts_all_migration_writes_and_preserves_unknown_fields() {
    for case in ["schema", "field", "generation", "marker"] {
        let dir = TestTempDir::new(&format!("nyaterm-migration-unsupported-{case}"));
        let mut store = ConnectionStore::open(dir.path()).unwrap();
        store
            .save_master_password(Some("fixture-password"))
            .unwrap();
        let valid = legacy_bytes(&migration_record(0), "valid");
        let mut record = serde_json::to_value(migration_record(0)).unwrap();
        match case {
            "schema" => record["schema"] = serde_json::json!(9),
            "field" => record["future"] = serde_json::json!({"keep": true}),
            "generation" => record["applied_generation"] = serde_json::json!(u64::MAX),
            _ => {}
        }
        let unknown = encryption::seal(
            "fixture-password",
            "local-replica",
            "unknown",
            &serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        let txn = store.db.begin_write().unwrap();
        if case == "marker" {
            txn.open_table(META_TABLE)
                .unwrap()
                .insert(LOCAL_REPLICA_MIGRATION, "future-stage")
                .unwrap();
        }
        {
            let mut table = txn.open_table(REPLICAS).unwrap();
            table.insert("valid", valid.as_slice()).unwrap();
            table.insert("unknown", unknown.as_slice()).unwrap();
        }
        txn.open_table(EDITS)
            .unwrap()
            .insert("retained", 0)
            .unwrap();
        txn.commit().unwrap();
        assert!(!store.migrate_local_replica_storage().unwrap());
        let txn = store.db.begin_read().unwrap();
        assert_eq!(
            txn.open_table(REPLICAS)
                .unwrap()
                .get("valid")
                .unwrap()
                .unwrap()
                .value(),
            valid.as_slice()
        );
        assert_eq!(
            txn.open_table(REPLICAS)
                .unwrap()
                .get("unknown")
                .unwrap()
                .unwrap()
                .value(),
            unknown.as_slice()
        );
        assert!(
            txn.open_table(EDITS)
                .unwrap()
                .get("retained")
                .unwrap()
                .is_some()
        );
        let marker = txn
            .open_table(META_TABLE)
            .unwrap()
            .get(LOCAL_REPLICA_MIGRATION)
            .unwrap()
            .map(|value| value.value().to_owned());
        assert_eq!(
            marker.as_deref(),
            if case == "marker" {
                Some("future-stage")
            } else {
                None
            }
        );
    }
}
