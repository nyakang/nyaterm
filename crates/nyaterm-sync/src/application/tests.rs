use nyaterm_core::sync_schema::{SyncFieldId, SyncProjection, SyncResourceId, SyncValues};
use serde_json::json;

use crate::application::isolate_changes;

fn values() -> SyncValues {
    let mut projection = SyncProjection::default();
    projection
        .record("connection", "a", &json!({"name":"A","network":{}}))
        .unwrap();
    projection.values
}

fn key(id: &str, field: &str) -> String {
    SyncFieldId::new(SyncResourceId::new("connection", id), field).key()
}

#[test]
fn invalid_group_keeps_local_value_while_independent_fields_apply() {
    let local = values();
    let mut desired = local.clone();
    desired.insert(
        key("a", "name"),
        nyaterm_core::sync_schema::SyncValue::new(&json!("Renamed")),
    );
    desired.insert(
        key("a", "network"),
        nyaterm_core::sync_schema::SyncValue::new(&json!({"jump":"missing"})),
    );
    let application = isolate_changes(&local, &desired, |values| {
        values.get(&key("a", "network")) == local.get(&key("a", "network"))
    })
    .unwrap();
    assert_eq!(
        application.effective.get(&key("a", "name")),
        desired.get(&key("a", "name"))
    );
    assert_eq!(
        application.effective.get(&key("a", "network")),
        local.get(&key("a", "network"))
    );
    assert_eq!(
        application.deferred,
        vec![SyncFieldId::from_key(&key("a", "network")).unwrap()]
    );
    assert_ne!(desired, application.effective);
}

#[test]
fn creation_is_atomic_and_deferred_dependencies_are_retried() {
    let local = SyncValues::new();
    let mut projection = SyncProjection::default();
    projection
        .record("connection", "a", &json!({"network":"z"}))
        .unwrap();
    projection
        .record("connection", "bad", &json!({"network":"missing"}))
        .unwrap();
    projection
        .record("connection", "z", &json!({"network":null}))
        .unwrap();
    let desired = projection.values;
    let application = isolate_changes(&local, &desired, |values| {
        ["a", "bad", "z"].iter().all(|id| {
            values.get(&key(id, "network")).is_none_or(|value| {
                let value = value.decode().unwrap();
                value
                    .as_str()
                    .is_none_or(|target| values.contains_key(&key(target, "$present")))
            })
        })
    })
    .unwrap();
    assert!(application.effective.contains_key(&key("a", "$present")));
    assert!(application.effective.contains_key(&key("z", "$present")));
    assert!(!application.effective.contains_key(&key("bad", "$present")));
    assert_eq!(application.deferred.len(), 2);
}

#[test]
fn deletion_cannot_orphan_a_saved_reference() {
    let mut projection = SyncProjection::default();
    projection
        .record("connection", "a", &json!({"network":"z"}))
        .unwrap();
    projection
        .record("connection", "z", &json!({"name":"Z"}))
        .unwrap();
    let local = projection.values;
    let desired = local
        .iter()
        .filter(|(key, _)| SyncFieldId::from_key(key).unwrap().resource.id != "z")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let application = isolate_changes(&local, &desired, |values| {
        !values.contains_key(&key("a", "network")) || values.contains_key(&key("z", "$present"))
    })
    .unwrap();
    assert_eq!(application.effective, local);
    assert_eq!(application.deferred.len(), 2);
}

#[test]
fn unchanged_valid_data_needs_one_validation_and_invalid_local_data_fails_closed() {
    let local = values();
    let mut validations = 0;
    let application = isolate_changes(&local, &local, |_| {
        validations += 1;
        true
    })
    .unwrap();
    assert_eq!(validations, 1);
    assert!(application.deferred.is_empty());
    assert!(isolate_changes(&local, &local, |_| false).is_err());
}
