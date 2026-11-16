//! Placement-only transactions preserve stored ciphertext and unknown properties verbatim.
use nyaterm_core::connection_tree::{MoveRequest, Node, NodeKind, plan_move};
use nyaterm_core::{Group, SavedConnection};
use redb::ReadableTable;
use serde_json::Value;

use super::{
    CONNECTION_PREFIX, CONNECTIONS_TABLE, ConnectionStore, GROUP_PREFIX, GROUPS_TABLE,
    StorageError, current_time_ms, entity_key, insert_connection_indexes,
    remove_connection_index_entries, write_json_in_txn,
};

/// User-created/reparented nodes append to a mixed parent. Untouched legacy
/// parents stay unconverted; imported unranked siblings keep their fallback order.
pub(super) fn append_tree_order_in_txn(
    txn: &redb::WriteTransaction,
    parent: Option<&str>,
) -> Result<Option<i64>, StorageError> {
    let mut siblings = Vec::new();
    for (definition, prefix, field) in [
        (GROUPS_TABLE, GROUP_PREFIX, "parent_id"),
        (CONNECTIONS_TABLE, CONNECTION_PREFIX, "group_id"),
    ] {
        for item in txn.open_table(definition)?.iter()? {
            let (key, value) = item?;
            if !key.value().starts_with(prefix) {
                continue;
            }
            let raw: Value = serde_json::from_slice(value.value())?;
            if raw.get(field).and_then(Value::as_str) != parent {
                continue;
            }
            let node = if field == "parent_id" {
                Node::group(&serde_json::from_value(raw.clone())?)
            } else {
                Node::connection(&serde_json::from_value(raw.clone())?)
            };
            siblings.push((node, raw, definition, prefix));
        }
    }
    let Some(mut next) = siblings
        .iter()
        .filter_map(|(node, ..)| node.tree_order)
        .max()
    else {
        return Ok(None);
    };
    siblings.sort_by(|a, b| nyaterm_core::connection_tree::compare(&a.0, &b.0));
    for (node, mut raw, definition, prefix) in siblings {
        if node.tree_order.is_none() {
            next = next
                .checked_add(1)
                .ok_or_else(|| StorageError::InvalidData("tree order exhausted".into()))?;
            raw["tree_order"] = next.into();
            write_json_in_txn(txn, definition, &entity_key(prefix, &node.key.id), &raw)?;
        }
    }
    next.checked_add(1)
        .map(Some)
        .ok_or_else(|| StorageError::InvalidData("tree order exhausted".into()))
}

impl ConnectionStore {
    pub fn move_connection_tree(&self, request: &MoveRequest) -> Result<(), StorageError> {
        self.move_connection_trees(std::slice::from_ref(request))
    }

    pub fn move_connection_trees(&self, requests: &[MoveRequest]) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        let mut nodes = Vec::new();
        for item in txn.open_table(GROUPS_TABLE)?.iter()? {
            let (key, value) = item?;
            if !key.value().starts_with(GROUP_PREFIX) {
                continue;
            }
            let group: Group = serde_json::from_slice(value.value())?;
            nodes.push(Node::group(&group));
        }
        for item in txn.open_table(CONNECTIONS_TABLE)?.iter()? {
            let (key, value) = item?;
            if !key.value().starts_with(CONNECTION_PREFIX) {
                continue;
            }
            let connection: SavedConnection = serde_json::from_slice(value.value())?;
            nodes.push(Node::connection(&connection));
        }
        let mut updates = std::collections::BTreeMap::new();
        for request in requests {
            let changes = plan_move(&nodes, request).map_err(|error| {
                StorageError::InvalidData(format!("invalid connection tree move: {error:?}"))
            })?;
            for change in changes {
                if let Some(node) = nodes.iter_mut().find(|n| n.key == change.key) {
                    *node = change.clone();
                }
                updates.insert(change.key.clone(), change);
            }
        }
        for change in updates.into_values() {
            let (table, prefix, parent_field) = match change.key.kind {
                NodeKind::Group => (GROUPS_TABLE, GROUP_PREFIX, "parent_id"),
                NodeKind::Connection => (CONNECTIONS_TABLE, CONNECTION_PREFIX, "group_id"),
            };
            let key = entity_key(prefix, &change.key.id);
            let mut raw: Value = {
                let table = txn.open_table(table)?;
                let value = table
                    .get(key.as_str())?
                    .ok_or_else(|| StorageError::InvalidData("tree node disappeared".into()))?;
                serde_json::from_slice(value.value())?
            };
            raw[parent_field] = serde_json::to_value(&change.parent)?;
            raw["sort_order"] = change.sort_order.into();
            raw["tree_order"] = serde_json::to_value(change.tree_order)?;
            raw["updated_at_ms"] = current_time_ms().into();
            if change.key.kind == NodeKind::Connection {
                let connection: SavedConnection = serde_json::from_value(raw.clone())?;
                remove_connection_index_entries(&txn, &connection.id)?;
                insert_connection_indexes(&txn, &connection)?;
            }
            write_json_in_txn(&txn, table, &key, &raw)?;
        }
        txn.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::storage::tests::unique_temp_dir;
    use crate::storage::{CONNECTIONS_TABLE, ConnectionStore, GROUPS_TABLE, write_json_in_txn};
    use nyaterm_core::connection_tree::{Destination, MoveRequest, Node, NodeId, NodeKind};
    use nyaterm_core::{Group, SavedConnection};
    use redb::ReadableDatabase;
    use serde_json::{Value, json};

    #[test]
    fn tree_move_preserves_raw_credentials_extensions_and_rolls_back_stale_batch() {
        let directory = unique_temp_dir("mixed-tree-move");
        let store = ConnectionStore::open(&directory).unwrap();
        let icon: nyaterm_core::ConnectionCustomIcon = serde_json::from_value(
            json!({"id":"icon","name":"Icon","data_url":"data:image/png;base64,AA==","created_at_ms":1,"updated_at_ms":1}),
        )
        .unwrap();
        store.save_connection_custom_icon(&icon).unwrap();
        let connection = json!({"id":"c","name":"SSH","type":"ssh","host":"example.test","port":22,"username":"test","sort_order":0,"auth":{"mode":"password","password":"opaque-test-ciphertext"},"future":{"keep":1}});
        let group = json!({"id":"g","name":"Group","sort_order":0,"future_group":{"keep":2}});
        let txn = store.db.begin_write().unwrap();
        write_json_in_txn(&txn, CONNECTIONS_TABLE, "connections/c", &connection).unwrap();
        write_json_in_txn(&txn, GROUPS_TABLE, "groups/g", &group).unwrap();
        txn.commit().unwrap();
        let nodes = vec![
            Node::group(&serde_json::from_value::<Group>(group.clone()).unwrap()),
            Node::connection(
                &serde_json::from_value::<SavedConnection>(connection.clone()).unwrap(),
            ),
        ];
        let request = MoveRequest {
            source: NodeId {
                kind: NodeKind::Connection,
                id: "c".into(),
            },
            destination: Destination {
                parent: None,
                before: Some(NodeId {
                    kind: NodeKind::Group,
                    id: "g".into(),
                }),
            },
            expected: nodes,
        };
        let read = || {
            let txn = store.db.begin_read().unwrap();
            let table = txn.open_table(CONNECTIONS_TABLE).unwrap();
            serde_json::from_slice::<Value>(table.get("connections/c").unwrap().unwrap().value())
                .unwrap()
        };
        let mut stale = request.clone();
        stale.expected.clear();
        assert!(
            store
                .move_connection_trees(&[request.clone(), stale])
                .is_err()
        );
        assert_eq!(read(), connection);
        store.move_connection_tree(&request).unwrap();
        let saved = read();
        assert_eq!(saved["tree_order"], 0);
        assert_eq!(saved["auth"], connection["auth"]);
        assert_eq!(saved["future"], connection["future"]);
        let txn = store.db.begin_read().unwrap();
        let table = txn.open_table(GROUPS_TABLE).unwrap();
        let saved_group: Value =
            serde_json::from_slice(table.get("groups/g").unwrap().unwrap().value()).unwrap();
        assert_eq!(saved_group["tree_order"], 1);
        assert_eq!(saved_group["future_group"], group["future_group"]);
        assert!(store.move_connection_tree(&request).is_err());
        assert_eq!(read(), saved);
        assert_eq!(store.list_connection_custom_icons().unwrap()[0].id, "icon");
    }

    #[test]
    fn mixed_order_survives_portable_and_native_backups_and_new_items_append() {
        let directory = unique_temp_dir("mixed-tree-backup");
        let portable_dir = unique_temp_dir("mixed-tree-portable");
        let native_dir = unique_temp_dir("mixed-tree-native");
        let store = ConnectionStore::open(&directory).unwrap();
        let group: Group = serde_json::from_value(json!({"id":"g","name":"G"})).unwrap();
        let connection: SavedConnection = serde_json::from_value(
            json!({"id":"c","name":"C","type":"local_terminal","shell_path":"pwsh.exe"}),
        )
        .unwrap();
        store.save_group(&group).unwrap();
        store.save_connection(&connection).unwrap();
        let sessions = store.load_sessions().unwrap();
        let request = MoveRequest {
            source: Node::connection(&connection).key,
            destination: Destination {
                parent: None,
                before: Some(Node::group(&group).key),
            },
            expected: nyaterm_core::connection_tree::catalog(
                &sessions.groups,
                &sessions.connections,
            ),
        };
        store.move_connection_tree(&request).unwrap();
        let added: Group = serde_json::from_value(json!({"id":"new","name":"New"})).unwrap();
        store.save_group(&added).unwrap();
        assert_eq!(
            store
                .list_groups()
                .unwrap()
                .iter()
                .find(|g| g.id == "new")
                .unwrap()
                .tree_order,
            Some(2)
        );
        let mut snapshot = store
            .build_raw_portable_snapshot(
                nyaterm_core::PortableSnapshotKind::Backup,
                "device",
                "test",
            )
            .unwrap();
        snapshot.recalculate_hash().unwrap();
        let target = ConnectionStore::open(&portable_dir).unwrap();
        target.apply_raw_portable_snapshot(&snapshot).unwrap();
        assert_eq!(target.list_connections().unwrap()[0].tree_order, Some(0));
        assert_eq!(
            target
                .list_groups()
                .unwrap()
                .iter()
                .find(|g| g.id == "g")
                .unwrap()
                .tree_order,
            Some(1)
        );
        drop(target);
        drop(store);
        let backup = directory.join("mixed.nya");
        ConnectionStore::export_config_database(&directory, None, &backup).unwrap();
        ConnectionStore::import_config_database(&native_dir, None, &backup).unwrap();
        let target = ConnectionStore::open(&native_dir).unwrap();
        assert_eq!(target.list_connections().unwrap()[0].tree_order, Some(0));
        assert_eq!(
            target
                .list_groups()
                .unwrap()
                .iter()
                .find(|g| g.id == "new")
                .unwrap()
                .tree_order,
            Some(2)
        );
    }
}
