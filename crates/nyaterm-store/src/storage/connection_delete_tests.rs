use std::collections::BTreeMap;
use std::time::Instant;

use nyaterm_core::{SavedConnection, SessionsConfig, test_support::TestTempDir};
use redb::{ReadableDatabase as _, ReadableTable as _, TableDefinition};

use crate::storage::{
    CONNECTION_PASSWORD_PREFIX, CONNECTIONS_TABLE, CREDENTIALS_TABLE, ConnectionStore,
    IDX_CONNECTIONS_BY_GROUP_TABLE, IDX_CONNECTIONS_BY_LAST_USED_TABLE,
    IDX_CONNECTIONS_BY_PROTOCOL_TABLE, META_TABLE, entity_key,
};

type StorageSnapshot = BTreeMap<String, Vec<(String, Vec<u8>)>>;

fn connection(id: &str) -> SavedConnection {
    serde_json::from_value(serde_json::json!({
        "id": id, "name": id, "type": "ssh", "host": "example.invalid",
        "port": 22, "username": "fixture", "future": {"keep": true},
        "auth": {"mode": "password", "password": "fixture-password"}
    }))
    .unwrap()
}

// Include the journal and generation so an aborted delete cannot leak sync edits.
fn snapshot(store: &ConnectionStore) -> StorageSnapshot {
    let txn = store.db.begin_read().unwrap();
    let mut result = BTreeMap::new();
    for definition in [CONNECTIONS_TABLE, CREDENTIALS_TABLE] {
        let rows = txn
            .open_table(definition)
            .unwrap()
            .iter()
            .unwrap()
            .map(|entry| {
                let (key, value) = entry.unwrap();
                (key.value().to_string(), value.value().to_vec())
            })
            .collect();
        result.insert(definition.to_string(), rows);
    }
    for definition in [
        META_TABLE,
        IDX_CONNECTIONS_BY_GROUP_TABLE,
        IDX_CONNECTIONS_BY_LAST_USED_TABLE,
        IDX_CONNECTIONS_BY_PROTOCOL_TABLE,
    ] {
        let rows = txn
            .open_table(definition)
            .unwrap()
            .iter()
            .unwrap()
            .map(|entry| {
                let (key, value) = entry.unwrap();
                (key.value().to_string(), value.value().as_bytes().to_vec())
            })
            .collect();
        result.insert(definition.to_string(), rows);
    }
    let definition: TableDefinition<&str, u64> = TableDefinition::new("sync_v4_field_edits");
    let rows = txn
        .open_table(definition)
        .unwrap()
        .iter()
        .unwrap()
        .map(|entry| {
            let (key, value) = entry.unwrap();
            (
                key.value().to_string(),
                value.value().to_be_bytes().to_vec(),
            )
        })
        .collect();
    result.insert(definition.to_string(), rows);
    result
}

#[test]
fn bulk_delete_rolls_back_records_passwords_indexes_and_sync_on_later_failure() {
    let root = TestTempDir::new("nyaterm-delete-rollback");
    let store = ConnectionStore::open(root.path()).unwrap();
    store
        .replace_sessions(&SessionsConfig {
            connections: vec![connection("first"), connection("second")],
            ..Default::default()
        })
        .unwrap();
    // The first delete and its password cleanup succeed; the next journal change
    // fails. This exercises rollback after writes without a synthetic test hook.
    let txn = store.db.begin_write().unwrap();
    txn.open_table(META_TABLE)
        .unwrap()
        .insert("sync_v4/generation", (u64::MAX - 2).to_string().as_str())
        .unwrap();
    txn.commit().unwrap();
    let before = snapshot(&store);
    assert!(
        store
            .delete_connections(&["first".into(), "second".into()])
            .is_err()
    );
    assert_eq!(snapshot(&store), before);
    drop(store);
    assert_eq!(
        snapshot(&ConnectionStore::open(root.path()).unwrap()),
        before
    );
}

#[test]
fn bulk_delete_thousand_assets_cleans_legacy_indexes_and_preserves_unselected_data() {
    let root = TestTempDir::new("nyaterm-delete-thousand");
    let store = ConnectionStore::open(root.path()).unwrap();
    let mut ids = (0..1000)
        .map(|i| format!("host-{i:04}"))
        .collect::<Vec<_>>();
    ids.push("legacy|id".into());
    let mut connections = ids.iter().map(|id| connection(id)).collect::<Vec<_>>();
    connections.push(connection("survivor|id"));
    store
        .replace_sessions(&SessionsConfig {
            connections,
            ..Default::default()
        })
        .unwrap();
    let survivor = store.get_connection("survivor|id").unwrap().unwrap();
    let txn = store.db.begin_write().unwrap();
    // Released records can contain legacy plaintext passwords; deletion must
    // remove these without requiring decryption or rewriting the record first.
    txn.open_table(CREDENTIALS_TABLE)
        .unwrap()
        .insert(
            entity_key(CONNECTION_PASSWORD_PREFIX, "legacy|id").as_str(),
            serde_json::json!({
                "id": "legacy|id", "connection_id": "legacy|id",
                "password": "legacy-fixture", "created_at_ms": 1, "updated_at_ms": 1
            })
            .to_string()
            .as_bytes(),
        )
        .unwrap();
    for definition in [
        IDX_CONNECTIONS_BY_GROUP_TABLE,
        IDX_CONNECTIONS_BY_LAST_USED_TABLE,
        IDX_CONNECTIONS_BY_PROTOCOL_TABLE,
    ] {
        let mut table = txn.open_table(definition).unwrap();
        table.insert("stale|legacy|id", "inconsistent").unwrap();
        table.insert("stale|survivor|id", "inconsistent").unwrap();
    }
    txn.commit().unwrap();
    let before = snapshot(&store);
    let started = Instant::now();
    store.delete_connections(&ids).unwrap();
    eprintln!("deleted {} assets in {:?}", ids.len(), started.elapsed());
    let after = snapshot(&store);
    for definition in [CONNECTIONS_TABLE, CREDENTIALS_TABLE] {
        let key = definition.to_string();
        let expected = before[&key]
            .iter()
            .filter(|(key, _)| key.ends_with("survivor|id"))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(after[&key], expected);
        assert_eq!(after[&key].len(), 1);
    }
    for definition in [
        IDX_CONNECTIONS_BY_GROUP_TABLE,
        IDX_CONNECTIONS_BY_LAST_USED_TABLE,
        IDX_CONNECTIONS_BY_PROTOCOL_TABLE,
    ] {
        let key = definition.to_string();
        let expected = before[&key]
            .iter()
            .filter(|(key, _)| key.ends_with("survivor|id"))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(after[&key], expected);
        assert_eq!(after[&key].len(), 2);
    }
    drop(store);
    let reopened = ConnectionStore::open(root.path()).unwrap();
    assert_eq!(reopened.list_connections().unwrap().len(), 1);
    assert_eq!(
        reopened.get_connection("survivor|id").unwrap().unwrap(),
        survivor
    );
}

#[test]
fn bulk_delete_empty_duplicate_and_missing_ids_keep_single_delete_semantics() {
    let root = TestTempDir::new("nyaterm-delete-idempotent");
    let store = ConnectionStore::open(root.path()).unwrap();
    store.save_connection(&connection("selected")).unwrap();
    let before = snapshot(&store);
    store.delete_connections(&[]).unwrap();
    assert_eq!(snapshot(&store), before);
    store
        .delete_connections(&["selected".into(), "selected".into(), "missing".into()])
        .unwrap();
    assert!(store.get_connection("selected").unwrap().is_none());
    assert!(snapshot(&store)[&CREDENTIALS_TABLE.to_string()].is_empty());
    store.delete_connections(&["selected".into()]).unwrap();
    assert!(store.list_connections().unwrap().is_empty());
    assert!(snapshot(&store)[&CONNECTIONS_TABLE.to_string()].is_empty());
}
