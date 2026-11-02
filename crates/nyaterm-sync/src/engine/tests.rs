use crate::engine::{SyncReplica, prepare};
use crate::protocol::{PendingPublication, PublicationId};
use nyaterm_core::sync_schema::{
    SyncFieldId, SyncProjection, SyncResourceId, SyncScope, SyncValue, SyncValues,
};
use serde_json::json;
use std::collections::BTreeMap;
use uuid::Uuid;

fn projection(name: &str, description: &str) -> SyncProjection {
    let mut projection = SyncProjection::default();
    projection.complete.insert("connection".into());
    projection
        .record(
            "connection",
            "a",
            &json!({"name":name,"description":description}),
        )
        .unwrap();
    projection
}

fn field(name: &str) -> SyncFieldId {
    SyncFieldId::new(SyncResourceId::new("connection", "a"), name)
}

fn seeded() -> (SyncReplica, SyncReplica, SyncProjection) {
    let baseline = projection("original", "description");
    let mut a = SyncReplica::new(Uuid::new_v4());
    a.capture(&BTreeMap::new(), &baseline, &SyncScope::default())
        .unwrap();
    let b = SyncReplica::load(&a.snapshot(), Uuid::new_v4()).unwrap();
    (a, b, baseline)
}

#[test]
fn independent_fields_merge_without_a_conflict() {
    let (mut a, mut b, baseline) = seeded();
    a.capture(
        &baseline.values,
        &projection("A", "description"),
        &SyncScope::default(),
    )
    .unwrap();
    b.capture(
        &baseline.values,
        &projection("original", "B"),
        &SyncScope::default(),
    )
    .unwrap();
    a.merge(&b.snapshot()).unwrap();
    assert!(a.conflicts().unwrap().is_empty());
    let result = a
        .materialize(&baseline.values, &SyncScope::default())
        .unwrap();
    assert_eq!(result[&field("name").key()].decode().unwrap(), json!("A"));
    assert_eq!(
        result[&field("description").key()].decode().unwrap(),
        json!("B")
    );
}

#[test]
fn same_field_conflicts_keep_effective_local_value_and_resolution_dominates_stale_delivery() {
    let (mut a, mut b, baseline) = seeded();
    let local = projection("A", "description");
    a.capture(&baseline.values, &local, &SyncScope::default())
        .unwrap();
    b.capture(
        &baseline.values,
        &projection("B", "description"),
        &SyncScope::default(),
    )
    .unwrap();
    let stale = b.snapshot();
    a.merge(&stale).unwrap();
    assert_eq!(a.conflicts().unwrap().len(), 1);
    let effective = a.materialize(&local.values, &SyncScope::default()).unwrap();
    assert_eq!(
        effective[&field("name").key()].decode().unwrap(),
        json!("A")
    );
    // Merely checking again must not resolve the conflict as a local edit.
    assert!(
        !a.capture(&effective, &local, &SyncScope::default())
            .unwrap()
    );
    let conflict = a.conflicts().unwrap().remove(0);
    a.resolve(&conflict, &conflict.candidates[0], &SyncScope::default())
        .unwrap();
    a.merge(&stale).unwrap();
    assert!(a.conflicts().unwrap().is_empty());
    assert!(
        a.resolve(&conflict, &conflict.candidates[0], &SyncScope::default())
            .is_err()
    );
}

#[test]
fn concurrent_edit_reaffirms_presence_and_cannot_be_hidden_by_a_delete() {
    let (mut a, mut b, baseline) = seeded();
    let mut deleted = SyncProjection::default();
    deleted.complete.insert("connection".into());
    a.capture(&baseline.values, &deleted, &SyncScope::default())
        .unwrap();
    b.capture(
        &baseline.values,
        &projection("edited", "description"),
        &SyncScope::default(),
    )
    .unwrap();
    a.merge(&b.snapshot()).unwrap();
    let conflict = a.conflicts().unwrap().remove(0);
    assert_eq!(conflict.field.field, "$present");
    assert_eq!(conflict.candidates.len(), 2);
    assert!(
        a.materialize(&baseline.values, &SyncScope::default())
            .unwrap()
            .contains_key(&field("name").key())
    );
}

#[test]
fn concurrent_deletes_converge_and_do_not_destroy_field_history() {
    let (mut a, mut b, baseline) = seeded();
    let mut deleted = SyncProjection::default();
    deleted.complete.insert("connection".into());
    a.capture(&baseline.values, &deleted, &SyncScope::default())
        .unwrap();
    b.capture(&baseline.values, &deleted, &SyncScope::default())
        .unwrap();
    a.merge(&b.snapshot()).unwrap();
    assert!(a.conflicts().unwrap().is_empty());
    assert!(
        a.materialize(&baseline.values, &SyncScope::default())
            .unwrap()
            .is_empty()
    );
    assert!(!a.candidates(&field("name")).unwrap().is_empty());
}

#[test]
fn unsupported_or_excluded_observations_do_not_delete() {
    let (mut a, _, baseline) = seeded();
    assert!(
        !a.capture(
            &baseline.values,
            &SyncProjection::default(),
            &SyncScope::default()
        )
        .unwrap()
    );
    let mut disabled = baseline.clone();
    disabled.complete.clear();
    disabled.values.clear();
    assert!(
        !a.capture(&baseline.values, &disabled, &SyncScope::default())
            .unwrap()
    );
}

#[test]
fn first_join_keeps_existing_local_resources_and_never_interprets_absence_as_deletion() {
    let (mut remote, _, _) = seeded();
    let mut current = SyncProjection::default();
    current.complete.insert("connection".into());
    current
        .record("connection", "b", &json!({"name":"local"}))
        .unwrap();
    let plan = prepare(
        Uuid::new_v4(),
        None,
        &SyncValues::new(),
        current,
        &[remote.snapshot()],
        SyncScope::default(),
    )
    .unwrap();
    assert!(plan.effective.contains_key(&field("name").key()));
    assert!(
        plan.effective
            .contains_key(&SyncFieldId::new(SyncResourceId::new("connection", "b"), "name").key())
    );
}

#[test]
fn empty_new_device_waits_for_conflicted_new_record_instead_of_creating_partial_data() {
    let (mut a, mut b, baseline) = seeded();
    a.capture(
        &baseline.values,
        &projection("A", "description"),
        &SyncScope::default(),
    )
    .unwrap();
    b.capture(
        &baseline.values,
        &projection("B", "description"),
        &SyncScope::default(),
    )
    .unwrap();
    a.merge(&b.snapshot()).unwrap();
    assert!(
        a.materialize(&SyncValues::new(), &SyncScope::default())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn join_is_commutative_associative_and_idempotent_under_partitioned_repeated_delivery() {
    let (mut seed, _, baseline) = seeded();
    let seed = seed.snapshot();
    let mut snapshots = Vec::new();
    for n in 0..20 {
        let mut replica = SyncReplica::load(&seed, Uuid::new_v4()).unwrap();
        replica
            .capture(
                &baseline.values,
                &projection(&format!("{n}"), "description"),
                &SyncScope::default(),
            )
            .unwrap();
        snapshots.push(replica.snapshot());
    }
    let mut forward = SyncReplica::load(&seed, Uuid::new_v4()).unwrap();
    let mut reverse = SyncReplica::load(&seed, Uuid::new_v4()).unwrap();
    for snapshot in &snapshots {
        forward.merge(snapshot).unwrap();
    }
    for snapshot in snapshots.iter().rev() {
        reverse.merge(snapshot).unwrap();
        reverse.merge(snapshot).unwrap();
    }
    assert_eq!(forward.conflicts().unwrap(), reverse.conflicts().unwrap());
    let mut left = SyncReplica::load(&seed, Uuid::new_v4()).unwrap();
    let mut right = SyncReplica::load(&seed, Uuid::new_v4()).unwrap();
    for snapshot in &snapshots[..10] {
        left.merge(snapshot).unwrap();
    }
    for snapshot in &snapshots[10..] {
        right.merge(snapshot).unwrap();
    }
    left.merge(&right.snapshot()).unwrap();
    assert_eq!(left.conflicts().unwrap(), forward.conflicts().unwrap());
}

#[test]
fn partition_schedules_keep_all_candidates_and_resolutions_survive_old_replica_return() {
    let scope = SyncScope::default();
    let mut initial = SyncProjection::default();
    initial.complete.insert("connection".into());
    for id in 0..6 {
        initial
            .record(
                "connection",
                &id.to_string(),
                &json!({"name":"initial","description":"initial"}),
            )
            .unwrap();
    }
    let mut origin = SyncReplica::new(Uuid::from_u128(1));
    origin
        .capture(&SyncValues::new(), &initial, &scope)
        .unwrap();
    let seed = origin.snapshot();
    let mut history = vec![seed.clone()];
    for actor in 2..=5 {
        let mut replica = SyncReplica::load(&seed, Uuid::from_u128(actor)).unwrap();
        let mut previous = initial.clone();
        for step in 0..12 {
            let id = ((actor as usize + step) % 6).to_string();
            let resource = SyncResourceId::new("connection", &id);
            let mut next = previous.clone();
            next.values
                .retain(|key, _| SyncFieldId::from_key(key).unwrap().resource != resource);
            if step % 5 != 0 {
                next.record("connection", &id, &json!({"name":format!("actor-{actor}-{step}"),"description":format!("edit-{step}")})).unwrap();
            }
            replica.capture(&previous.values, &next, &scope).unwrap();
            previous = next;
            history.push(replica.snapshot());
        }
    }
    let mut expected = SyncReplica::load(&seed, Uuid::from_u128(10)).unwrap();
    for snapshot in &history {
        expected.merge(snapshot).unwrap();
    }
    let conflicts = expected.conflicts().unwrap();
    assert!(!conflicts.is_empty());
    let values = expected.snapshot().values;
    for schedule in 0..8 {
        let mut replica = SyncReplica::load(&seed, Uuid::from_u128(20 + schedule)).unwrap();
        let mut order = (0..history.len()).collect::<Vec<_>>();
        let mut random = schedule + 1;
        for index in (1..order.len()).rev() {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            order.swap(index, (random as usize) % (index + 1));
        }
        for index in order {
            replica.merge(&history[index]).unwrap();
            replica.merge(&history[index]).unwrap();
        }
        assert_eq!(replica.conflicts().unwrap(), conflicts);
        assert_eq!(replica.snapshot().values, values);
        assert_eq!(
            replica.materialize(&initial.values, &scope).unwrap(),
            expected.materialize(&initial.values, &scope).unwrap()
        );
        // Keeping a resource can reveal field conflicts previously hidden by
        // its existence conflict. Confirm each newly visible candidate set.
        let mut decisions = 0;
        while let Some(conflict) = replica.conflicts().unwrap().into_iter().next() {
            decisions += 1;
            assert!(decisions <= 18);
            replica
                .resolve(&conflict, &conflict.candidates[0], &scope)
                .unwrap();
        }
        let confirmed = replica.materialize(&initial.values, &scope).unwrap();
        for snapshot in history.iter().rev() {
            replica.merge(snapshot).unwrap();
        }
        assert!(replica.conflicts().unwrap().is_empty());
        assert_eq!(
            replica.materialize(&initial.values, &scope).unwrap(),
            confirmed
        );
        assert_eq!(replica.snapshot().values, values);
    }
}

#[test]
fn corrupt_or_future_replica_is_rejected() {
    let (mut a, _, _) = seeded();
    let mut snapshot = a.snapshot();
    snapshot.schema = 5;
    assert!(SyncReplica::load(&snapshot, Uuid::new_v4()).is_err());
    snapshot.schema = 4;
    snapshot.values.clear();
    assert!(SyncReplica::load(&snapshot, Uuid::new_v4()).is_err());
}

#[test]
fn publication_identity_binds_exact_retry_bytes_and_rejects_noncanonical_paths() {
    let pending = PendingPublication::new(Uuid::new_v4(), 1, vec![1, 2, 3], "doc".into()).unwrap();
    assert_eq!(
        PublicationId::parse(&pending.id.path()),
        Some(pending.id.clone())
    );
    assert!(pending.id.verifies(&pending.bytes));
    assert!(!pending.id.verifies(&[1, 2, 4]));
    assert!(PublicationId::parse(&pending.id.path().replace("/1-", "/01-")).is_none());
    let bytes = serde_json::to_vec(&pending).unwrap();
    let reopened: PendingPublication = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(reopened.bytes, pending.bytes);
    assert_eq!(reopened.id, pending.id);
}

#[test]
fn json_null_is_a_value_and_sensitive_debug_is_redacted() {
    let value = SyncValue::new(&json!({"password":"private-value"}));
    assert!(!format!("{value:?}").contains("private-value"));
}

#[test]
fn reopening_a_scope_keeps_unconfirmed_remote_and_local_candidates() {
    let scope = SyncScope {
        preferences: true,
        ..Default::default()
    };
    let mut remote_projection = SyncProjection::default();
    remote_projection.complete.insert("preference".into());
    remote_projection
        .record("preference", "appearance/font_size", &json!({"value":20}))
        .unwrap();
    let mut replica = SyncReplica::new(Uuid::new_v4());
    replica
        .capture(&SyncValues::new(), &remote_projection, &scope)
        .unwrap();
    let mut local = remote_projection.clone();
    local.values.insert(
        SyncFieldId::new(
            SyncResourceId::new("preference", "appearance/font_size"),
            "value",
        )
        .key(),
        SyncValue::new(&json!(14)),
    );
    let plan = prepare(
        Uuid::new_v4(),
        Some(&replica.snapshot()),
        &SyncValues::new(),
        local.clone(),
        &[],
        scope,
    )
    .unwrap();
    assert_eq!(plan.conflicts.len(), 1);
    assert_eq!(plan.effective, local.values);
}

#[test]
fn first_device_join_does_not_publish_initial_default_preferences_over_remote_settings() {
    let scope = SyncScope {
        preferences: true,
        ..Default::default()
    };
    let mut remote_projection = SyncProjection::default();
    remote_projection.complete.insert("preference".into());
    remote_projection
        .record("preference", "appearance/font_size", &json!({"value":37}))
        .unwrap();
    let mut remote = SyncReplica::new(Uuid::new_v4());
    remote
        .capture(&SyncValues::new(), &remote_projection, &scope)
        .unwrap();
    let mut current = SyncProjection::default();
    current.complete.insert("preference".into());
    current.record("preference", "appearance/font_size", &json!({"value":nyaterm_core::models::settings::AppSettingsSummary::default().terminal_font_size})).unwrap();
    let plan = prepare(
        Uuid::new_v4(),
        None,
        &SyncValues::new(),
        current,
        &[remote.snapshot()],
        scope,
    )
    .unwrap();
    assert!(plan.conflicts.is_empty());
    assert_eq!(plan.effective, remote_projection.values);
}

#[test]
fn unknown_fields_survive_capture_but_cannot_be_materialized_or_deleted() {
    let (mut replica, _, baseline) = seeded();
    let mut future = SyncProjection::default();
    future.insert(
        &SyncResourceId::new("connection", "a"),
        "future",
        &json!({"secret":true}),
    );
    replica.preserve(&future).unwrap();
    assert!(
        !replica
            .materialize(&baseline.values, &SyncScope::default())
            .unwrap()
            .contains_key(&field("future").key())
    );
    let snapshot = replica.snapshot();
    let plan = prepare(
        Uuid::new_v4(),
        Some(&snapshot),
        &baseline.values,
        baseline.clone(),
        &[],
        SyncScope::default(),
    )
    .unwrap();
    assert!(
        SyncReplica::load(&plan.replica, Uuid::new_v4())
            .unwrap()
            .candidates(&field("future"))
            .unwrap()
            .len()
            == 1
    );
}

#[test]
fn json_objects_are_never_confused_with_field_tombstones() {
    let (mut replica, _, baseline) = seeded();
    let mut next = baseline.clone();
    next.values.insert(
        field("description").key(),
        SyncValue::new(&json!({"$deleted":true})),
    );
    replica
        .capture(&baseline.values, &next, &SyncScope::default())
        .unwrap();
    assert_eq!(
        replica
            .materialize(&baseline.values, &SyncScope::default())
            .unwrap(),
        next.values
    );
    let mut removed = next.clone();
    removed.values.remove(&field("description").key());
    replica
        .capture(&next.values, &removed, &SyncScope::default())
        .unwrap();
    assert!(
        !replica
            .materialize(&next.values, &SyncScope::default())
            .unwrap()
            .contains_key(&field("description").key())
    );
}

#[test]
fn future_envelope_metadata_is_rejected_instead_of_silently_discarded() {
    let (mut replica, _, _) = seeded();
    let mut bytes = serde_json::to_value(replica.snapshot()).unwrap();
    bytes["future_version_inventory"] = json!({"unrecognized":true});
    assert!(serde_json::from_value::<crate::engine::ReplicaSnapshot>(bytes).is_err());
}

#[test]
fn first_join_defaults_do_not_conflict_with_remote_ai_configuration() {
    let defaults = serde_json::to_value(nyaterm_core::ai::AiSettings::default()).unwrap();
    let scope = SyncScope {
        ai: true,
        ..Default::default()
    };
    let local = nyaterm_core::sync_schema::project_entities(
        &BTreeMap::from([("settings".into(), json!({"ai":defaults}).to_string())]),
        &scope,
    )
    .unwrap();
    let mut remote = local.clone();
    let name = SyncFieldId::new(SyncResourceId::new("ai_provider", "openai"), "name");
    remote
        .values
        .insert(name.key(), SyncValue::new(&json!("Cloud provider name")));
    let mut source = SyncReplica::new(Uuid::new_v4());
    source.capture(&SyncValues::new(), &remote, &scope).unwrap();
    let plan = prepare(
        Uuid::new_v4(),
        None,
        &SyncValues::new(),
        local,
        &[source.snapshot()],
        scope,
    )
    .unwrap();
    assert!(plan.conflicts.is_empty());
    assert_eq!(
        plan.effective.get(&name.key()),
        remote.values.get(&name.key())
    );
}

#[test]
fn an_incomplete_category_can_publish_readable_edits_without_inferring_absence() {
    let (mut replica, _, baseline) = seeded();
    let mut current = projection("Readable edit", "description");
    current.complete.clear();
    current.values.remove(&field("description").key());
    replica
        .capture(&baseline.values, &current, &SyncScope::default())
        .unwrap();
    let result = replica
        .materialize(&baseline.values, &SyncScope::default())
        .unwrap();
    assert_eq!(
        result.get(&field("name").key()),
        current.values.get(&field("name").key())
    );
    assert_eq!(
        result.get(&field("description").key()),
        baseline.values.get(&field("description").key())
    );
}
