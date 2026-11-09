use std::collections::BTreeMap;

use nyaterm_core::sync_schema::{FIXED_CLOUD_SYNC_SCOPE, SyncProjection, encryption};
use nyaterm_core::test_support::TestTempDir;
use nyaterm_sync::engine::{SyncReplica, prepare};
use redb::{ReadableDatabase, ReadableTable, ReadableTableMetadata};
use serde_json::json;
use uuid::Uuid;

use super::tests::{connection, enable};
use super::{ApplyPoint, EDITS, REPLICAS};
use crate::storage::ConnectionStore;
use crate::sync_v4::PreparedApply;

fn prepared_for(store: &ConnectionStore, destination: &str) -> PreparedApply {
    let view = store
        .load_v4_view(destination, "password", &FIXED_CLOUD_SYNC_SCOPE)
        .unwrap();
    let plan = prepare(
        view.record.writer,
        view.record.replica.as_ref(),
        &view.record.effective,
        view.projection,
        &[],
        FIXED_CLOUD_SYNC_SCOPE,
    )
    .unwrap();
    PreparedApply {
        destination: destination.into(),
        expected_record: view.record_hash,
        generation: view.generation,
        local: view.record,
        plan,
        trust_approvals: Default::default(),
    }
}

fn edits(store: &ConnectionStore) -> BTreeMap<String, u64> {
    store
        .db
        .begin_read()
        .unwrap()
        .open_table(EDITS)
        .unwrap()
        .iter()
        .unwrap()
        .map(|row| {
            let (key, value) = row.unwrap();
            (key.value().to_owned(), value.value())
        })
        .collect()
}

#[test]
fn another_destination_apply_preserves_reverted_edits_against_concurrent_deletion() {
    let dir = TestTempDir::new("nyaterm-shared-journal-reverted");
    let store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    store.save_connection(&connection()).unwrap();
    store
        .apply_v4_plan(prepared_for(&store, "A"), "password")
        .unwrap();
    let baseline = store
        .apply_v4_plan(prepared_for(&store, "B"), "password")
        .unwrap();
    let mut remote = SyncReplica::load(baseline.replica.as_ref().unwrap(), Uuid::new_v4()).unwrap();
    let mut absent = SyncProjection::default();
    absent.complete.insert("connection".into());
    remote
        .capture(&baseline.effective, &absent, &FIXED_CLOUD_SYNC_SCOPE)
        .unwrap();
    let mut edited = connection();
    edited.name = "temporary".into();
    store.save_connection(&edited).unwrap();
    store.save_connection(&connection()).unwrap();
    store
        .apply_v4_plan(prepared_for(&store, "A"), "password")
        .unwrap();
    let view = store
        .load_v4_view("B", "password", &FIXED_CLOUD_SYNC_SCOPE)
        .unwrap();
    assert_eq!(view.projection.values, baseline.effective);
    assert!(!view.projection.edited.is_empty());
    let plan = prepare(
        view.record.writer,
        view.record.replica.as_ref(),
        &view.record.effective,
        view.projection,
        &[remote.snapshot()],
        FIXED_CLOUD_SYNC_SCOPE,
    )
    .unwrap();
    assert!(
        plan.conflicts
            .iter()
            .any(|conflict| conflict.field.field == "$present")
    );
    let caught_up = store
        .apply_v4_plan(prepared_for(&store, "B"), "password")
        .unwrap();
    assert!(edits(&store).is_empty());
    assert_eq!(
        caught_up.applied_generation,
        store
            .load_v4_view("A", "password", &FIXED_CLOUD_SYNC_SCOPE)
            .unwrap()
            .generation
    );
}

#[test]
fn snapshot_apply_preserves_shared_edits_and_records_its_own_changes_without_a_replica() {
    let dir = TestTempDir::new("nyaterm-shared-journal-snapshot");
    let store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    store.save_connection(&connection()).unwrap();
    store
        .apply_v4_plan(prepared_for(&store, "A"), "password")
        .unwrap();
    let mut changed = connection();
    changed.name = "Temporary".into();
    store.save_connection(&changed).unwrap();
    store.save_connection(&connection()).unwrap();
    let before = edits(&store);
    assert!(!before.is_empty());
    let mut snapshot = store
        .build_fixed_sync_snapshot("fixture", "fixture")
        .unwrap();
    let mut sessions: serde_json::Value =
        serde_json::from_str(&snapshot.entities["sessions"]).unwrap();
    sessions["connections"][0]["name"] = json!("Imported");
    snapshot
        .entities
        .insert("sessions".into(), sessions.to_string());
    snapshot.recalculate_hash().unwrap();
    store
        .apply_fixed_sync_snapshot(&snapshot, "password")
        .unwrap();
    let after = edits(&store);
    for (field, edited_at) in before {
        assert!(
            after
                .get(&field)
                .is_some_and(|generation| *generation >= edited_at)
        );
    }
    assert_eq!(
        store.get_connection("server").unwrap().unwrap().name,
        "Imported"
    );
    let view = store
        .load_v4_view("A", "password", &FIXED_CLOUD_SYNC_SCOPE)
        .unwrap();
    assert!(!view.projection.edited.is_empty());
    assert!(
        after
            .values()
            .any(|edited_at| *edited_at > view.record.applied_generation)
    );
    assert_eq!(
        store
            .db
            .begin_read()
            .unwrap()
            .open_table(REPLICAS)
            .unwrap()
            .len()
            .unwrap(),
        1
    );
}

#[test]
fn unreadable_future_and_abnormal_archived_replicas_prevent_cleanup_without_blocking_apply() {
    for case in [
        "corrupt",
        "future-schema",
        "future-field",
        "future-generation",
        "paused",
    ] {
        let dir = TestTempDir::new(&format!("nyaterm-shared-journal-{case}"));
        let store = ConnectionStore::open(dir.path()).unwrap();
        enable(&store);
        store.save_connection(&connection()).unwrap();
        store
            .apply_v4_plan(prepared_for(&store, "A"), "password")
            .unwrap();
        let mut record = serde_json::to_value(crate::sync_v4::LocalReplica::default()).unwrap();
        match case {
            "future-schema" => record["schema"] = json!(9),
            "future-field" => record["future"] = json!({"opaque": "keep"}),
            "future-generation" => record["applied_generation"] = json!(u64::MAX),
            _ => {}
        }
        let raw = if case == "corrupt" {
            b"NYASYNC4broken".to_vec()
        } else {
            encryption::seal(
                "password",
                "local-replica",
                "archived",
                &serde_json::to_vec(&record).unwrap(),
            )
            .unwrap()
        };
        let txn = store.db.begin_write().unwrap();
        txn.open_table(REPLICAS)
            .unwrap()
            .insert("archived", raw.as_slice())
            .unwrap();
        txn.commit().unwrap();
        let mut changed = connection();
        changed.name = "Retained".into();
        store.save_connection(&changed).unwrap();
        let before = edits(&store);
        store
            .apply_v4_plan(prepared_for(&store, "A"), "password")
            .unwrap();
        assert_eq!(edits(&store), before, "{case}");
        assert_eq!(
            store
                .db
                .begin_read()
                .unwrap()
                .open_table(REPLICAS)
                .unwrap()
                .get("archived")
                .unwrap()
                .unwrap()
                .value(),
            raw.as_slice()
        );
    }
}

#[test]
fn interrupted_apply_rolls_back_the_replica_and_shared_journal_cleanup() {
    let dir = TestTempDir::new("nyaterm-shared-journal-rollback");
    let store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    store.save_connection(&connection()).unwrap();
    store
        .apply_v4_plan(prepared_for(&store, "A"), "password")
        .unwrap();
    let mut changed = connection();
    changed.name = "Saved".into();
    store.save_connection(&changed).unwrap();
    let before = edits(&store);
    let hash = store.load_v4_replica("A", "password").unwrap().unwrap().1;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        store
            .apply_v4_plan_observed(prepared_for(&store, "A"), "password", |point| {
                if point == ApplyPoint::Replica {
                    panic!("synthetic interruption before commit");
                }
            })
            .unwrap();
    }));
    assert!(outcome.is_err());
    assert_eq!(edits(&store), before);
    assert_eq!(
        store.load_v4_replica("A", "password").unwrap().unwrap().1,
        hash
    );
    assert_eq!(
        store.get_connection("server").unwrap().unwrap().name,
        "Saved"
    );
}
