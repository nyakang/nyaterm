use std::collections::BTreeMap;

use nyaterm_core::sync_schema::{SyncProjection, SyncValues, apply_record, project_record};
use redb::{ReadableDatabase, ReadableTable, TableDefinition};
use serde_json::{Value, json};

use crate::storage::cloud_sync_v4::{invalid, records};
use crate::storage::{
    ConnectionStore, KNOWN_HOST_PREFIX, KNOWN_HOST_RAW_PREFIX, KNOWN_HOSTS_TABLE, StorageError,
    VNC_KNOWN_HOSTS_TABLE, current_time_ms, write_json_in_txn,
};

const CATALOG: &str = "v4_trust_catalog";
type TrustCategory = (
    &'static str,
    TableDefinition<'static, &'static str, &'static [u8]>,
    &'static str,
);
const CATEGORIES: &[TrustCategory] = &[
    ("known_host", KNOWN_HOSTS_TABLE, KNOWN_HOST_PREFIX),
    ("vnc_known_host", VNC_KNOWN_HOSTS_TABLE, "vnc_known_hosts/"),
];

pub(super) fn legacy_projection(
    snapshot: &nyaterm_core::RawPortableSnapshot,
) -> Result<SyncProjection, StorageError> {
    let mut projection = SyncProjection::default();
    if let Some(raw) = snapshot.entities.get("known_hosts") {
        let text: String = serde_json::from_str(raw)?;
        for (id, value) in crate::storage::known_hosts::parse_v4_export(&text) {
            projection
                .record(
                    "known_host",
                    &id,
                    &project_record("known_host", &value).map_err(|_| invalid())?,
                )
                .map_err(|_| invalid())?;
        }
    }
    if let Some(raw) = snapshot.entities.get("vnc_known_hosts") {
        let values: Vec<Value> = serde_json::from_str(raw)?;
        for value in values {
            let record: nyaterm_core::vnc_known_hosts::VncKnownHostRecord =
                serde_json::from_value(value.clone())?;
            let id = crate::storage::vnc_known_hosts::key(&record.host, record.port);
            validate("vnc_known_host", &id, &value)?;
            projection
                .record(
                    "vnc_known_host",
                    &id,
                    &project_record("vnc_known_host", &value).map_err(|_| invalid())?,
                )
                .map_err(|_| invalid())?;
        }
    }
    Ok(projection)
}

fn validate(kind: &str, id: &str, value: &Value) -> Result<Value, StorageError> {
    if kind == "known_host" {
        return crate::storage::known_hosts::validate_v4_record(id, value);
    }
    let mut value = value.clone();
    if value.get("created_at_ms").is_none() {
        value["created_at_ms"] = json!(current_time_ms());
    }
    value["updated_at_ms"] = json!(current_time_ms());
    let record: nyaterm_core::vnc_known_hosts::VncKnownHostRecord =
        serde_json::from_value(value.clone())?;
    crate::storage::vnc_known_hosts::validate_record(&record)?;
    if crate::storage::vnc_known_hosts::key(&record.host, record.port) != id {
        return Err(invalid());
    }
    Ok(value)
}

pub(super) fn attach(
    store: &ConnectionStore,
    entities: &mut BTreeMap<String, String>,
) -> Result<SyncProjection, StorageError> {
    let txn = store.db.begin_read()?;
    let mut catalog = json!({});
    let mut projection = SyncProjection::default();
    for (kind, table, prefix) in CATEGORIES {
        catalog[*kind] = json!({});
        projection.complete.insert((*kind).into());
        for entry in txn.open_table(*table)?.iter()? {
            let (key, bytes) = entry?;
            let id = key.value();
            if !id.starts_with(prefix)
                || (*kind == "known_host" && id.starts_with(KNOWN_HOST_RAW_PREFIX))
            {
                continue;
            }
            let value: Value = serde_json::from_slice(bytes.value())?;
            catalog[*kind][id] = value.clone();
            if validate(kind, id, &value).is_err() {
                projection.complete.remove(*kind);
                continue;
            }
            projection
                .record(
                    kind,
                    id,
                    &project_record(kind, &value).map_err(|_| invalid())?,
                )
                .map_err(|_| invalid())?;
        }
    }
    entities.insert(CATALOG.into(), catalog.to_string());
    Ok(projection)
}

pub(super) fn reconstruct(
    entities: &mut BTreeMap<String, Value>,
    values: &SyncValues,
) -> Result<(), StorageError> {
    let original = entities.get(CATALOG).cloned().ok_or_else(invalid)?;
    let mut catalog = json!({});
    for (kind, _, _) in CATEGORIES {
        let old = original[*kind].as_object().ok_or_else(invalid)?;
        catalog[*kind] = json!({});
        for (id, value) in old
            .iter()
            .filter(|(id, value)| validate(kind, id, value).is_err())
        {
            catalog[*kind][id] = value.clone();
        }
        for (id, fields) in records(values, kind)? {
            let empty = json!({});
            let before = old.get(&id).unwrap_or(&empty);
            let merged = apply_record(kind, before, &fields).map_err(|_| invalid())?;
            let next = validate(kind, &id, &merged)?;
            catalog[*kind][id] = if old
                .get(&id)
                .is_some_and(|old| project_record(kind, old).ok() == Some(fields))
            {
                before.clone()
            } else {
                next
            };
        }
    }
    entities.insert(CATALOG.into(), catalog);
    Ok(())
}

pub(super) fn persist(
    txn: &redb::WriteTransaction,
    original: &BTreeMap<String, String>,
    entities: &BTreeMap<String, Value>,
) -> Result<(), StorageError> {
    let old: Value = serde_json::from_str(&original[CATALOG])?;
    for (kind, table, _) in CATEGORIES {
        let old = old[*kind].as_object().ok_or_else(invalid)?;
        let next = entities[CATALOG][*kind].as_object().ok_or_else(invalid)?;
        for id in old.keys().filter(|id| !next.contains_key(*id)) {
            crate::storage::cloud_sync_v4::record_remove(txn, *table, id)?;
            txn.open_table(*table)?.remove(id.as_str())?;
        }
        for (id, value) in next {
            if old.get(id) != Some(value) {
                write_json_in_txn(txn, *table, id, value)?;
            }
        }
    }
    Ok(())
}
