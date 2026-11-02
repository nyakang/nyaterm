use nyaterm_core::SavedConnection;
use nyaterm_core::sync_schema::SyncScope;
use nyaterm_core::test_support::TestTempDir;
use nyaterm_sync::engine::{SyncReplica, prepare};
use redb::{ReadableDatabase, ReadableTable};
use serde_json::json;
use uuid::Uuid;

use crate::sync_v4::PreparedApply;
use crate::{ConnectionStore, StorageError};

fn connection() -> SavedConnection {
    serde_json::from_value(json!({"id":"server","name":"Server","type":"ssh","host":"server.example","port":22,"username":"root"})).unwrap()
}

#[test]
fn batch_index_cleanup_preserves_legacy_suffix_matching_and_unaffected_rows() {
    let dir = TestTempDir::new("nyaterm-v4-batch-index-cleanup");
    let store = ConnectionStore::open(dir.path()).unwrap();
    let txn = store.db.begin_write().unwrap();
    let definitions = [
        crate::storage::IDX_CONNECTIONS_BY_GROUP_TABLE,
        crate::storage::IDX_CONNECTIONS_BY_LAST_USED_TABLE,
        crate::storage::IDX_CONNECTIONS_BY_PROTOCOL_TABLE,
    ];
    for definition in definitions {
        let mut table = txn.open_table(definition).unwrap();
        for (key, value) in [
            ("legacy|first", "wrong-value"),
            ("other-key", "second"),
            ("legacy|id|with|separator", "wrong-value"),
            ("prefix|untouched", "untouched"),
        ] {
            table.insert(key, value).unwrap();
        }
    }
    crate::storage::remove_connection_indexes_for_ids(
        &txn,
        &std::collections::BTreeSet::from(["first", "second", "id|with|separator"]),
    )
    .unwrap();
    for definition in definitions {
        let table = txn.open_table(definition).unwrap();
        let rows = table
            .iter()
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0.value(), "prefix|untouched");
        assert_eq!(rows[0].1.value(), "untouched");
    }
    txn.commit().unwrap();
}

fn enable(store: &ConnectionStore) {
    let mut settings = store.load_cloud_sync_settings().unwrap();
    settings.protocol_version = 4;
    store.save_cloud_sync_settings(settings).unwrap();
}

fn prepared(store: &ConnectionStore) -> PreparedApply {
    prepared_scope(store, SyncScope::default())
}

fn prepared_scope(store: &ConnectionStore, scope: SyncScope) -> PreparedApply {
    let view = store
        .load_v4_view("destination", "password", &scope)
        .unwrap();
    let plan = prepare(
        view.record.writer,
        view.record.replica.as_ref(),
        &view.record.effective,
        view.projection,
        &[],
        scope,
    )
    .unwrap();
    PreparedApply {
        destination: "destination".into(),
        expected_record: view.record_hash,
        generation: view.generation,
        local: view.record,
        plan,
        trust_approvals: Default::default(),
    }
}

fn replace_projection(
    proposal: &mut PreparedApply,
    next: nyaterm_core::sync_schema::SyncProjection,
) {
    let mut replica = SyncReplica::load(&proposal.plan.replica, Uuid::new_v4()).unwrap();
    replica
        .capture(&proposal.plan.effective, &next, &proposal.plan.scope)
        .unwrap();
    proposal.plan.effective = replica
        .materialize(&proposal.plan.expected_local.values, &proposal.plan.scope)
        .unwrap();
    proposal.plan.replica = replica.snapshot();
}

#[test]
fn password_rotation_retries_after_restart_and_switches_settings_vault_and_replicas_together() {
    let dir = TestTempDir::new("nyaterm-v4-password-rotation");
    let store = ConnectionStore::open(dir.path()).unwrap();
    store.save_master_password(Some("password")).unwrap();
    enable(&store);
    store.save_connection(&connection()).unwrap();
    store
        .save_password(
            serde_json::from_value(
                json!({"id":"vault","name":"Vault","password":"fixture-vault-value"}),
            )
            .unwrap(),
        )
        .unwrap();
    let mut first = prepared(&store);
    first.local.migration_approved = true;
    store.apply_v4_plan(first, "password").unwrap();
    assert!(
        store
            .save_master_password(Some("unsafe-direct-change"))
            .is_err()
    );
    assert!(store.save_master_password(None).is_err());
    let before_root = store.load_cloud_sync_settings().unwrap().remote_root;
    let staged = store
        .stage_v4_password_rotation(
            prepared(&store),
            "next-password",
            "next-destination",
            "new-space",
        )
        .unwrap();
    assert_eq!(
        store.master_password_matches("password").unwrap(),
        Some(true)
    );
    assert_eq!(
        store.load_cloud_sync_settings().unwrap().remote_root,
        before_root
    );
    drop(store);
    let store = ConnectionStore::open(dir.path()).unwrap();
    let retried = store
        .load_v4_password_rotation("destination", "next-password")
        .unwrap()
        .unwrap();
    assert_eq!(retried.ticket, staged.ticket);
    assert_eq!(retried.pending.bytes, staged.pending.bytes);
    assert_eq!(retried.pending.id, staged.pending.id);
    assert!(
        store
            .load_v4_password_rotation("destination", "wrong-password")
            .unwrap()
            .is_none()
    );
    store
        .commit_verified_v4_password_rotation(&retried.ticket)
        .unwrap();
    assert_eq!(
        store.master_password_matches("next-password").unwrap(),
        Some(true)
    );
    assert_eq!(
        store.load_cloud_sync_settings().unwrap().remote_root,
        "new-space"
    );
    assert_eq!(
        store
            .load_decrypted_password_by_id("vault")
            .unwrap()
            .unwrap()
            .password
            .unwrap()
            .expose_secret(),
        "fixture-vault-value"
    );
    for destination in ["destination", "next-destination"] {
        assert!(
            store
                .load_v4_view(destination, "next-password", &Default::default())
                .is_ok()
        );
        assert!(
            store
                .load_v4_view(destination, "password", &Default::default())
                .is_err()
        );
    }
    assert!(
        store
            .load_v4_password_rotation("destination", "next-password")
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_saved_edit_during_password_rotation_prevents_switching_and_keeps_the_old_space() {
    let dir = TestTempDir::new("nyaterm-v4-password-rotation-race");
    let store = ConnectionStore::open(dir.path()).unwrap();
    store.save_master_password(Some("password")).unwrap();
    enable(&store);
    store.save_connection(&connection()).unwrap();
    let mut first = prepared(&store);
    first.local.migration_approved = true;
    store.apply_v4_plan(first, "password").unwrap();
    let staged = store
        .stage_v4_password_rotation(
            prepared(&store),
            "next-password",
            "next-destination",
            "new-space",
        )
        .unwrap();
    let mut changed = connection();
    changed.name = "Saved while uploading".into();
    store.save_connection(&changed).unwrap();
    assert!(matches!(
        store.commit_verified_v4_password_rotation(&staged.ticket),
        Err(StorageError::CloudSyncLocalChanged)
    ));
    assert_eq!(
        store.master_password_matches("password").unwrap(),
        Some(true)
    );
    assert_eq!(
        store.list_connections().unwrap()[0].name,
        "Saved while uploading"
    );
    assert!(
        store
            .load_v4_password_rotation("destination", "next-password")
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .load_v4_view("destination", "password", &Default::default())
            .unwrap()
            .record
            .replica
            .is_some()
    );
}

#[test]
fn independent_secret_objects_reencrypt_vault_values_and_preserve_existing_ciphertext() {
    let a = TestTempDir::new("nyaterm-v4-vault-a");
    let b = TestTempDir::new("nyaterm-v4-vault-b");
    let sa = ConnectionStore::open(a.path()).unwrap();
    let sb = ConnectionStore::open(b.path()).unwrap();
    let scope = SyncScope {
        secrets: true,
        ai: true,
        ..Default::default()
    };
    for store in [&sa, &sb] {
        enable(store);
        let mut settings = store.load_cloud_sync_settings().unwrap();
        settings.v4_scope = scope.clone();
        store.save_cloud_sync_settings(settings).unwrap();
    }
    sa.save_master_password(Some("device-a-password")).unwrap();
    sb.save_master_password(Some("device-b-password")).unwrap();
    let mut server = connection();
    server.auth = Some(
        serde_json::from_value(json!({"mode":"password","password":"fixture-inline"})).unwrap(),
    );
    sa.save_connection(&server).unwrap();
    sa.replace_proxies(&[serde_json::from_value(
        json!({"id":"proxy","name":"Proxy","password":"fixture-proxy"}),
    )
    .unwrap()])
        .unwrap();
    let mut ai = sa.load_ai_settings().unwrap();
    ai.provider_credentials = vec![serde_json::from_value(json!({"id":"provider","name":"Provider","provider_kind":"openai","base_url":"https://example.com/v1/","api_key":"fixture-ai-key","extra_headers":[{"id":"header","name":"X-Token","value":"fixture-header"}]})).unwrap()];
    ai.models.clear();
    ai.default_model_id = None;
    sa.save_ai_settings(ai).unwrap();
    let mut raw = sa.load_settings_value().unwrap();
    raw["ai"]["models"] = json!([]);
    raw["ai"]["provider_profiles"] = json!([]);
    raw["ai"]["default_model_id"] = serde_json::Value::Null;
    sa.save_settings_value(&raw).unwrap();
    sa.save_password(serde_json::from_value(json!({"id":"account","name":"Account","username":"user","password":"fixture-password"})).unwrap()).unwrap();
    sa.save_ssh_key(serde_json::from_value(json!({"id":"key","name":"Key","key":"fixture-key","cert":"fixture-cert","passphrase":"fixture-passphrase"})).unwrap()).unwrap();
    sa.save_credential(serde_json::from_value(json!({"id":"credential","name":"Credential","username":"user","password":"fixture-credential"})).unwrap()).unwrap();
    sa.save_otp_entry(
        serde_json::from_value(
            json!({"id":"otp","issuer":"Issuer","username":"user","secret":"JBSWY3DPEHPK3PXP"}),
        )
        .unwrap(),
    )
    .unwrap();
    let before = sa
        .load_password_by_id("account")
        .unwrap()
        .unwrap()
        .password
        .unwrap();
    let prepared_a = prepared_scope(&sa, scope.clone());
    let replica_a = sa.apply_v4_plan(prepared_a, "password").unwrap();
    assert_eq!(
        sa.load_password_by_id("account")
            .unwrap()
            .unwrap()
            .password
            .unwrap(),
        before
    );
    assert!(
        !String::from_utf8_lossy(&replica_a.replica.as_ref().unwrap().document)
            .contains("fixture-password")
    );
    let mut prepared_b = prepared_scope(&sb, scope.clone());
    prepared_b.plan = prepare(
        prepared_b.local.writer,
        None,
        &Default::default(),
        prepared_b.plan.expected_local.clone(),
        &[replica_a.replica.unwrap()],
        scope.clone(),
    )
    .unwrap();
    prepared_b
        .local
        .secret_objects
        .extend(replica_a.secret_objects);
    let inspection = sb
        .inspect_v4_application_with_secrets(
            &prepared_b.plan,
            &prepared_b.local.secret_objects,
            "password",
            &Default::default(),
        )
        .unwrap();
    assert!(
        inspection.deferred.is_empty(),
        "deferred fields: {:?}",
        inspection.deferred
    );
    let applied = sb.apply_v4_plan(prepared_b, "password").unwrap();
    assert!(applied.deferred.is_empty());
    assert_eq!(
        sb.load_decrypted_password_by_id("account")
            .unwrap()
            .unwrap()
            .password
            .unwrap()
            .expose_secret(),
        "fixture-password"
    );
    assert_eq!(
        sb.load_decrypted_ssh_key_by_id("key")
            .unwrap()
            .unwrap()
            .key_data
            .unwrap()
            .expose_secret(),
        "fixture-key"
    );
    assert_eq!(
        sb.load_decrypted_credential_by_id("credential")
            .unwrap()
            .unwrap()
            .password
            .unwrap()
            .expose_secret(),
        "fixture-credential"
    );
    assert_eq!(
        sb.load_decrypted_otp_entry_by_id("otp")
            .unwrap()
            .unwrap()
            .secret
            .unwrap()
            .expose_secret(),
        "JBSWY3DPEHPK3PXP"
    );
    assert_eq!(
        sb.get_connection("server")
            .unwrap()
            .unwrap()
            .auth
            .unwrap()
            .password
            .unwrap()
            .expose_secret(),
        "fixture-inline"
    );
    assert_eq!(
        sb.list_proxies().unwrap()[0]
            .password
            .as_ref()
            .unwrap()
            .expose_secret(),
        "fixture-proxy"
    );
    let ai = sb.load_ai_settings().unwrap();
    assert_eq!(
        ai.provider_credentials[0]
            .api_key
            .as_ref()
            .unwrap()
            .expose_secret(),
        "fixture-ai-key"
    );
    assert_eq!(
        ai.provider_credentials[0].extra_headers[0]
            .value
            .expose_secret(),
        "fixture-header"
    );
    assert_ne!(
        sb.load_password_by_id("account")
            .unwrap()
            .unwrap()
            .password
            .unwrap(),
        before
    );
    let after = sb.load_v4_view("destination", "password", &scope).unwrap();
    assert_eq!(after.projection.values, applied.effective);
    assert!(after.projection.edited.is_empty());
    let mut settings = sb.load_cloud_sync_settings().unwrap();
    settings.v4_scope.secrets = false;
    settings.v4_scope.ai = false;
    sb.save_cloud_sync_settings(settings).unwrap();
    sb.apply_v4_plan(prepared(&sb), "password").unwrap();
    assert!(
        sb.load_decrypted_password_by_id("account")
            .unwrap()
            .is_some()
    );
}

#[test]
fn excluded_inline_password_prevents_remote_parent_deletion_or_secret_redirection() {
    use nyaterm_core::sync_schema::{SyncFieldId, SyncResourceId, SyncValue};
    let dir = TestTempDir::new("nyaterm-v4-excluded-inline-parent");
    let store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    let mut server = connection();
    server.auth = Some(
        serde_json::from_value(json!({"mode":"password","password":"fixture-inline"})).unwrap(),
    );
    store.save_connection(&server).unwrap();
    let before = store
        .load_v4_view("destination", "password", &Default::default())
        .unwrap();
    let mut proposal = prepared(&store);
    let mut next = before.projection.clone();
    let resource = SyncResourceId::new("connection", "server");
    let endpoint = SyncFieldId::new(resource.clone(), "endpoint");
    let mut target = next.values[&endpoint.key()].decode().unwrap();
    target["host"] = json!("other.example");
    next.values.insert(endpoint.key(), SyncValue::new(&target));
    next.values.insert(
        SyncFieldId::new(resource.clone(), "name").key(),
        SyncValue::new(&json!("Renamed")),
    );
    replace_projection(&mut proposal, next);
    let applied = store.apply_v4_plan(proposal, "password").unwrap();
    assert!(applied.deferred.contains(&endpoint));
    assert_eq!(
        store.get_connection("server").unwrap().unwrap().name,
        "Renamed"
    );
    assert_eq!(
        serde_json::to_value(store.get_connection("server").unwrap().unwrap()).unwrap()["host"],
        "server.example"
    );
    let mut proposal = prepared(&store);
    let mut next = proposal.plan.expected_local.clone();
    next.values
        .retain(|key, _| SyncFieldId::from_key(key).unwrap().resource != resource);
    replace_projection(&mut proposal, next);
    assert!(
        !store
            .apply_v4_plan(proposal, "password")
            .unwrap()
            .deferred
            .is_empty()
    );
    assert!(store.get_connection("server").unwrap().is_some());
}

#[test]
fn legacy_secret_projection_is_read_only_and_replaces_master_key_material_with_value_references() {
    let dir = TestTempDir::new("nyaterm-v4-legacy-secret-projection");
    let store = ConnectionStore::open(dir.path()).unwrap();
    store.save_master_password(Some("password")).unwrap();
    store
        .save_password(
            serde_json::from_value(
                json!({"id":"account","name":"Account","password":"legacy-fixture-password"}),
            )
            .unwrap(),
        )
        .unwrap();
    let snapshot = store
        .build_raw_portable_snapshot(nyaterm_core::PortableSnapshotKind::Sync, "source", "2.0.0")
        .unwrap();
    let before = store
        .load_v4_view("destination", "password", &Default::default())
        .unwrap();
    let (projection, objects) = store
        .project_v4_legacy_secrets(&snapshot, "password")
        .unwrap();
    assert_eq!(projection.complete.len(), 6);
    assert!(!objects.is_empty());
    let codec = nyaterm_core::sync_schema::secrets::SecretValueCodec::new("password").unwrap();
    let object = objects
        .values()
        .find(|object| object.reference.resource.kind == "password")
        .unwrap();
    assert_eq!(
        codec
            .open(&object.reference, object)
            .unwrap()
            .decode()
            .unwrap()["password"],
        "legacy-fixture-password"
    );
    let after = store
        .load_v4_view("destination", "password", &Default::default())
        .unwrap();
    assert_eq!(before.generation, after.generation);
    assert_eq!(before.projection, after.projection);
    assert!(
        store
            .project_v4_legacy_secrets(&snapshot, "incorrect-password")
            .is_err()
    );
    let mut legacy = nyaterm_core::RawPortableSnapshot::sync("legacy", "1.9.0");
    legacy.entities.insert("settings".into(), json!({"ai":{"provider_credentials":[{"id":"provider","name":"Provider","provider_kind":"openai","base_url":"https://example.com/v1/","api_key":"legacy-plaintext-key"}]}}).to_string());
    let (_, objects) = store
        .project_v4_legacy_secrets(&legacy, "password")
        .unwrap();
    let object = objects
        .values()
        .find(|object| object.reference.resource.kind == "ai_secret")
        .unwrap();
    assert_eq!(
        codec
            .open(&object.reference, object)
            .unwrap()
            .decode()
            .unwrap()["api_key"],
        "legacy-plaintext-key"
    );
}

#[test]
fn remote_host_identity_requires_exact_local_confirmation_before_changing_trust() {
    let a = TestTempDir::new("nyaterm-v4-trust-a");
    let b = TestTempDir::new("nyaterm-v4-trust-b");
    let sa = ConnectionStore::open(a.path()).unwrap();
    let sb = ConnectionStore::open(b.path()).unwrap();
    let scope = SyncScope {
        trusted_hosts: true,
        ..Default::default()
    };
    for store in [&sa, &sb] {
        enable(store);
        let mut settings = store.load_cloud_sync_settings().unwrap();
        settings.v4_scope = scope.clone();
        store.save_cloud_sync_settings(settings).unwrap();
    }
    let fingerprint = format!("SHA256:{}", "12".repeat(32));
    sa.remember_vnc_known_host("example.com", 5900, &fingerprint, None)
        .unwrap();
    let line = "example.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILM+rvN+ot98qgEN796jTiQfZfG1KaT0PtFDJ/XFSqti";
    sa.upsert_known_host(line).unwrap();
    let source = sa
        .apply_v4_plan(prepared_scope(&sa, scope.clone()), "password")
        .unwrap();
    let mut proposal = prepared_scope(&sb, scope.clone());
    proposal.plan = prepare(
        proposal.local.writer,
        None,
        &Default::default(),
        proposal.plan.expected_local.clone(),
        &[source.replica.unwrap()],
        scope.clone(),
    )
    .unwrap();
    let reviews = nyaterm_sync::trust::reviews(
        &proposal.plan.expected_local.values,
        &proposal.plan.effective,
    );
    assert_eq!(reviews.len(), 2);
    let deferred = sb.apply_v4_plan(proposal, "password").unwrap();
    assert!(!deferred.deferred.is_empty());
    assert!(
        sb.load_vnc_known_host("example.com", 5900)
            .unwrap()
            .is_none()
    );
    assert!(sb.list_known_hosts().unwrap().is_empty());
    let mut proposal = prepared_scope(&sb, scope.clone());
    for review in nyaterm_sync::trust::reviews(
        &proposal.plan.expected_local.values,
        &proposal.plan.effective,
    ) {
        proposal.trust_approvals.insert(review.key, review.token);
    }
    let approved = sb.apply_v4_plan(proposal, "password").unwrap();
    assert!(approved.deferred.is_empty());
    assert_eq!(
        sb.load_vnc_known_host("example.com", 5900)
            .unwrap()
            .unwrap()
            .sha256_fingerprint,
        fingerprint
    );
    assert_eq!(sb.list_known_hosts().unwrap().len(), 1);
    let view = sb.load_v4_view("destination", "password", &scope).unwrap();
    assert_eq!(view.record.effective, view.projection.values);
    let mut proposal = prepared_scope(&sb, scope);
    proposal.trust_approvals =
        std::collections::BTreeMap::from([(reviews[0].key.clone(), reviews[0].token.clone())]);
    sb.remember_vnc_known_host(
        "example.com",
        5900,
        &format!("SHA256:{}", "34".repeat(32)),
        Some(&fingerprint),
    )
    .unwrap();
    assert!(matches!(
        sb.apply_v4_plan(proposal, "password"),
        Err(StorageError::CloudSyncLocalChanged)
    ));
}

#[test]
fn ai_configuration_merge_preserves_ciphertext_and_defers_secret_redirects_and_deletions() {
    use nyaterm_core::sync_schema::{SyncFieldId, SyncResourceId, SyncValue};
    let dir = TestTempDir::new("nyaterm-v4-ai-secret-isolation");
    let store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    store.save_master_password(Some("local-password")).unwrap();
    let mut ai = store.load_ai_settings().unwrap();
    ai.provider_credentials = vec![serde_json::from_value(json!({"id":"p","name":"Provider","provider_kind":"openai","base_url":"https://original.example/v1/","api_key":"fixture-private-key","extra_headers":[{"id":"h","name":"X-Token","value":"fixture-private-header"}]})).unwrap()];
    ai.models.clear();
    ai.default_model_id = None;
    store.save_ai_settings(ai).unwrap();
    let mut settings = store.load_cloud_sync_settings().unwrap();
    settings.v4_scope.ai = true;
    store.save_cloud_sync_settings(settings.clone()).unwrap();
    let scope = settings.v4_scope.clone();
    let before = store.load_settings_value().unwrap()["ai"]["provider_credentials"][0].clone();
    assert_ne!(before["api_key"], json!("fixture-private-key"));
    let mut proposal = prepared_scope(&store, scope.clone());
    let mut next = proposal.plan.expected_local.clone();
    let provider = SyncResourceId::new("ai_provider", "p");
    next.values.insert(
        SyncFieldId::new(provider.clone(), "name").key(),
        SyncValue::new(&json!("Renamed")),
    );
    let transport = SyncFieldId::new(provider.clone(), "transport");
    let mut transport_value = next.values[&transport.key()].decode().unwrap();
    transport_value["base_url"] = json!("https://other.example/v1/");
    next.values
        .insert(transport.key(), SyncValue::new(&transport_value));
    replace_projection(&mut proposal, next);
    let applied = store.apply_v4_plan(proposal, "password").unwrap();
    assert_eq!(applied.deferred, vec![transport]);
    let after = store.load_settings_value().unwrap()["ai"]["provider_credentials"][0].clone();
    assert_eq!(after["name"], "Renamed");
    assert_eq!(after["base_url"], before["base_url"]);
    assert_eq!(after["api_key"], before["api_key"]);
    assert_eq!(after["extra_headers"], before["extra_headers"]);
    let mut deletion = prepared_scope(&store, scope.clone());
    let mut next = deletion.plan.expected_local.clone();
    next.values
        .retain(|key, _| SyncFieldId::from_key(key).unwrap().resource != provider);
    replace_projection(&mut deletion, next);
    assert!(
        !store
            .apply_v4_plan(deletion, "password")
            .unwrap()
            .deferred
            .is_empty()
    );
    assert_eq!(
        store.load_settings_value().unwrap()["ai"]["provider_credentials"][0]["api_key"],
        before["api_key"]
    );
    settings.v4_scope.ai = false;
    store.save_cloud_sync_settings(settings).unwrap();
    store.apply_v4_plan(prepared(&store), "password").unwrap();
    assert_eq!(
        store.load_settings_value().unwrap()["ai"]["provider_credentials"][0]["api_key"],
        before["api_key"]
    );
}

#[test]
fn shortcut_scope_round_trip_and_reenable_keep_both_unconfirmed_values() {
    let dir = TestTempDir::new("nyaterm-v4-shortcut-scope");
    let store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    let mut settings = store.load_cloud_sync_settings().unwrap();
    settings.v4_scope.keybindings = true;
    store.save_cloud_sync_settings(settings.clone()).unwrap();
    let scope = settings.v4_scope.clone();
    store
        .save_keybindings(&std::collections::HashMap::from([(
            "terminal.copy".into(),
            "ctrl+shift+c".into(),
        )]))
        .unwrap();
    store
        .apply_v4_plan(prepared_scope(&store, scope.clone()), "password")
        .unwrap();
    settings.v4_scope.keybindings = false;
    store.save_cloud_sync_settings(settings.clone()).unwrap();
    let disabled = store.apply_v4_plan(prepared(&store), "password").unwrap();
    assert!(disabled.effective.is_empty());
    assert_eq!(
        store.load_app_settings_summary().unwrap().keybindings["terminal.copy"],
        "ctrl+shift+c"
    );
    store
        .save_keybindings(&std::collections::HashMap::from([(
            "terminal.copy".into(),
            "ctrl+c".into(),
        )]))
        .unwrap();
    settings.v4_scope.keybindings = true;
    store.save_cloud_sync_settings(settings).unwrap();
    let reenabled = prepared_scope(&store, scope);
    assert_eq!(reenabled.plan.conflicts.len(), 1);
    store.apply_v4_plan(reenabled, "password").unwrap();
    assert_eq!(
        store.load_app_settings_summary().unwrap().keybindings["terminal.copy"],
        "ctrl+c"
    );
}

#[test]
fn replica_and_business_apply_round_trip_without_changing_local_fields() {
    let dir = TestTempDir::new("nyaterm-v4-transaction");
    let store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    store.save_connection(&connection()).unwrap();
    let local = store.apply_v4_plan(prepared(&store), "password").unwrap();
    assert!(local.replica.is_some());
    let reopened = store
        .load_v4_view("destination", "password", &SyncScope::default())
        .unwrap();
    assert!(reopened.projection.edited.is_empty());
    assert_eq!(reopened.record.effective, reopened.projection.values);
    assert!(
        store
            .load_v4_view("destination", "wrong", &SyncScope::default())
            .is_err()
    );
    assert_eq!(store.list_connections().unwrap()[0].name, "Server");
}

#[test]
fn stale_apply_cannot_overwrite_a_saved_edit_or_delete() {
    let dir = TestTempDir::new("nyaterm-v4-stale-transaction");
    let store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    store.save_connection(&connection()).unwrap();
    let plan = prepared(&store);
    store.delete_connection("server").unwrap();
    assert!(matches!(
        store.apply_v4_plan(plan, "password"),
        Err(StorageError::CloudSyncLocalChanged)
    ));
    assert!(store.list_connections().unwrap().is_empty());
}

#[test]
fn two_confirmations_cannot_commit_different_replicas_at_the_same_business_generation() {
    let dir = TestTempDir::new("nyaterm-v4-competing-confirmations");
    let store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    store.save_connection(&connection()).unwrap();
    let first = prepared(&store);
    let second = prepared(&store);
    assert_eq!(first.generation, second.generation);
    let generation = first.generation;
    let applied = store.apply_v4_plan(first, "password").unwrap();
    let current = store
        .load_v4_view("destination", "password", &Default::default())
        .unwrap();
    assert_eq!(current.generation, generation);
    assert!(matches!(
        store.apply_v4_plan(second, "password"),
        Err(StorageError::CloudSyncLocalChanged)
    ));
    let after = store
        .load_v4_view("destination", "password", &Default::default())
        .unwrap();
    assert_eq!(after.record_hash, current.record_hash);
    assert!(after.record == applied);
}

#[test]
fn deleting_a_local_authentication_target_invalidates_an_outstanding_apply() {
    let dir = TestTempDir::new("nyaterm-v4-auth-delete-cas");
    let store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    store.save_connection(&connection()).unwrap();
    store
        .save_ssh_key(serde_json::from_value(json!({"id":"target","name":"Key"})).unwrap())
        .unwrap();
    let outstanding = prepared(&store);
    store.delete_ssh_key("target").unwrap();
    assert!(matches!(
        store.apply_v4_plan(outstanding, "password"),
        Err(StorageError::CloudSyncLocalChanged)
    ));
    assert_eq!(
        store.get_connection("server").unwrap().unwrap().name,
        "Server"
    );
}

#[test]
fn v4_crash_child() {
    let Some(directory) = std::env::var_os("NYATERM_SYNC_CRASH_DB") else {
        return;
    };
    let checkpoint: u8 = std::env::var("NYATERM_SYNC_CRASH_POINT")
        .unwrap()
        .parse()
        .unwrap();
    let store = ConnectionStore::open(std::path::Path::new(&directory)).unwrap();
    let mut proposal = prepared(&store);
    let mut next = proposal.plan.expected_local.clone();
    next.values.insert(
        nyaterm_core::sync_schema::SyncFieldId::new(
            nyaterm_core::sync_schema::SyncResourceId::new("connection", "server"),
            "name",
        )
        .key(),
        nyaterm_core::sync_schema::SyncValue::new(&json!("After")),
    );
    next.record(
        "note",
        "crash-note",
        &json!({"title":"After","markdown":"Durable note","parent_id":null,"sort_order":0}),
    )
    .unwrap();
    replace_projection(&mut proposal, next);
    store
        .apply_v4_plan_observed(proposal, "password", |point| {
            if point as u8 == checkpoint {
                std::process::exit(86);
            }
        })
        .unwrap();
    panic!("crash checkpoint was not reached");
}

#[test]
#[ignore = "creates a fresh isolated portable configuration for native UI verification"]
fn create_isolated_native_ui_fixture() {
    let directory = std::path::PathBuf::from(
        std::env::var_os("NYATERM_SYNC_UI_FIXTURE")
            .expect("set the isolated fixture configuration directory"),
    );
    assert!(
        !directory.join("nyaterm.redb").exists(),
        "fixture refuses to overwrite an existing database"
    );
    let store = ConnectionStore::open_with_portable_key_path(
        &directory,
        Some(directory.join("portable.key")),
    )
    .unwrap();
    store
        .save_master_password(Some("test-only-cloud-sync"))
        .unwrap();
    let settings = nyaterm_core::CloudSyncSettings {
        protocol_version: 4,
        enabled: false,
        auto_check_on_startup: false,
        auto_push_on_change: false,
        auto_pull_remote_changes: false,
        ..Default::default()
    };
    store.save_cloud_sync_settings(settings).unwrap();
    store.save_connection(&connection()).unwrap();
}

#[test]
fn process_exit_at_each_apply_boundary_recovers_all_business_data_and_replica_or_neither() {
    for checkpoint in 0..=7 {
        let dir = TestTempDir::new("nyaterm-v4-process-crash");
        let store = ConnectionStore::open(dir.path()).unwrap();
        enable(&store);
        store.save_connection(&connection()).unwrap();
        let initial = store.apply_v4_plan(prepared(&store), "password").unwrap();
        drop(store);
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "storage::cloud_sync_v4::tests::v4_crash_child",
                "--test-threads=1",
            ])
            .env("NYATERM_SYNC_CRASH_DB", dir.path())
            .env("NYATERM_SYNC_CRASH_POINT", checkpoint.to_string())
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(86),
            "crash boundary {checkpoint} was not reached: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let recovered = ConnectionStore::open(dir.path()).unwrap();
        let view = recovered
            .load_v4_view("destination", "password", &SyncScope::default())
            .unwrap();
        if checkpoint == 7 {
            assert_eq!(
                recovered.get_connection("server").unwrap().unwrap().name,
                "After"
            );
            assert_eq!(recovered.load_notes_snapshot().unwrap().notes.len(), 1);
            assert_eq!(view.record.effective, view.projection.values);
            assert_ne!(
                view.record.replica.unwrap().document,
                initial.replica.unwrap().document
            );
        } else {
            assert_eq!(
                recovered.get_connection("server").unwrap().unwrap().name,
                "Server"
            );
            assert!(recovered.load_notes_snapshot().unwrap().notes.is_empty());
            assert_eq!(
                view.record.replica.unwrap().document,
                initial.replica.unwrap().document
            );
        }
    }
}

#[test]
fn a_local_only_binding_edit_invalidates_apply_without_creating_a_cloud_edit() {
    let dir = TestTempDir::new("nyaterm-v4-local-binding-cas");
    let store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    let local: SavedConnection = serde_json::from_value(json!({"id":"local","name":"Local","type":"local_terminal","shell_path":"old-shell","working_dir":"old-dir"})).unwrap();
    store.save_connection(&local).unwrap();
    store.apply_v4_plan(prepared(&store), "password").unwrap();
    let plan = prepared(&store);
    let mut changed = serde_json::to_value(local).unwrap();
    changed["shell_path"] = json!("new-shell");
    store
        .save_connection(&serde_json::from_value(changed).unwrap())
        .unwrap();
    let view = store
        .load_v4_view("destination", "password", &SyncScope::default())
        .unwrap();
    assert!(view.projection.edited.is_empty());
    assert!(matches!(
        store.apply_v4_plan(plan, "password"),
        Err(StorageError::CloudSyncLocalChanged)
    ));
    assert!(
        serde_json::to_value(&store.list_connections().unwrap()[0]).unwrap()["shell_path"]
            == "new-shell"
    );
}

#[test]
fn reverted_edits_still_reaffirm_existence_against_concurrent_deletion() {
    let dir = TestTempDir::new("nyaterm-v4-reverted-edit");
    let store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    store.save_connection(&connection()).unwrap();
    let baseline = store.apply_v4_plan(prepared(&store), "password").unwrap();
    let mut remote = SyncReplica::load(baseline.replica.as_ref().unwrap(), Uuid::new_v4()).unwrap();
    let mut absent = nyaterm_core::sync_schema::SyncProjection::default();
    absent.complete.insert("connection".into());
    remote
        .capture(&baseline.effective, &absent, &SyncScope::default())
        .unwrap();
    let mut edited = connection();
    edited.name = "temporary".into();
    store.save_connection(&edited).unwrap();
    store.save_connection(&connection()).unwrap();
    let view = store
        .load_v4_view("destination", "password", &SyncScope::default())
        .unwrap();
    assert!(!view.projection.edited.is_empty());
    let plan = prepare(
        view.record.writer,
        view.record.replica.as_ref(),
        &view.record.effective,
        view.projection,
        &[remote.snapshot()],
        SyncScope::default(),
    )
    .unwrap();
    assert!(
        plan.conflicts
            .iter()
            .any(|conflict| conflict.field.field == "$present")
    );
}

#[test]
fn runtime_usage_does_not_dirty_the_sync_projection_or_journal() {
    let dir = TestTempDir::new("nyaterm-v4-runtime-local");
    let store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    store.save_connection(&connection()).unwrap();
    store.apply_v4_plan(prepared(&store), "password").unwrap();
    let before = store
        .load_v4_view("destination", "password", &SyncScope::default())
        .unwrap();
    store.mark_connection_used("server").unwrap();
    store
        .merge_connection_asset_from_monitoring(
            "server",
            nyaterm_core::AssetMetadata {
                os_name: Some("Linux".into()),
                cpu_cores: Some(16),
                ..Default::default()
            },
        )
        .unwrap();
    let after = store
        .load_v4_view("destination", "password", &SyncScope::default())
        .unwrap();
    assert_eq!(before.projection, after.projection);
    assert_eq!(before.generation, after.generation);
}

#[test]
fn transaction_overlay_retains_usage_updated_after_the_apply_source_was_read() {
    let dir = TestTempDir::new("nyaterm-v4-runtime-apply-race");
    let store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    store.save_connection(&connection()).unwrap();
    let mut next = store.get_connection("server").unwrap().unwrap();
    next.name = "Remote edit".into();
    let next = store.prepare_connection_for_storage(&next).unwrap();
    store.mark_connection_used("server").unwrap();
    let used = store
        .get_connection("server")
        .unwrap()
        .unwrap()
        .last_used_at_ms;
    let txn = store.db.begin_write().unwrap();
    let rebased = super::rebase_connection_in_txn(&txn, &next).unwrap();
    crate::storage::save_connection_in_txn(&txn, &rebased).unwrap();
    txn.commit().unwrap();
    let saved = store.get_connection("server").unwrap().unwrap();
    assert_eq!(saved.name, "Remote edit");
    assert_eq!(saved.last_used_at_ms, used);
    assert!(saved.last_used_at_ms.is_some());
}

#[test]
fn network_collection_overlay_preserves_local_command_and_tunnel_runtime_state() {
    let dir = TestTempDir::new("nyaterm-v4-network-apply-race");
    let store = ConnectionStore::open(dir.path()).unwrap();
    let old = json!([{"id":"proxy","name":"Proxy","protocol":"socks5","host":"old","port":1080,"command":"old-local"}]);
    let mut edited = old.clone();
    edited[0]["command"] = json!("new-local");
    store
        .replace_proxies(&serde_json::from_value::<Vec<nyaterm_core::ProxyConfig>>(edited).unwrap())
        .unwrap();
    let mut incoming = old.clone();
    incoming[0]["host"] = json!("remote");
    let txn = store.db.begin_write().unwrap();
    super::write_collection(
        &txn,
        super::PROXIES_TABLE,
        super::PROXY_PREFIX,
        &old,
        &incoming,
    )
    .unwrap();
    txn.commit().unwrap();
    let result = &store.list_proxies().unwrap()[0];
    assert_eq!(result.host, "remote");
    assert_eq!(result.command.as_deref(), Some("new-local"));

    let old = json!([{"id":"tunnel","name":"Tunnel","target_host":"old","is_open":false}]);
    let mut current = old.clone();
    current[0]["is_open"] = json!(true);
    let txn = store.db.begin_write().unwrap();
    super::write_json_in_txn(&txn, super::TUNNELS_TABLE, "tunnels/tunnel", &current[0]).unwrap();
    txn.commit().unwrap();
    let mut incoming = old.clone();
    incoming[0]["target_host"] = json!("remote");
    let txn = store.db.begin_write().unwrap();
    super::write_collection(
        &txn,
        super::TUNNELS_TABLE,
        super::TUNNEL_PREFIX,
        &old,
        &incoming,
    )
    .unwrap();
    let key = super::entity_key(super::TUNNEL_PREFIX, "tunnel");
    let bytes = txn
        .open_table(super::TUNNELS_TABLE)
        .unwrap()
        .get(key.as_str())
        .unwrap()
        .unwrap()
        .value()
        .to_vec();
    let saved: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(saved["is_open"], true);
    assert_eq!(saved["target_host"], "remote");
}

#[test]
fn applying_a_group_rename_retains_unknown_persisted_fields() {
    let dir = TestTempDir::new("nyaterm-v4-group-extension");
    let store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    let txn = store.db.begin_write().unwrap();
    super::write_json_in_txn(&txn, super::GROUPS_TABLE, "groups/group", &json!({"id":"group","name":"Old","parent_id":null,"sort_order":0,"future":{"opaque":"keep"}})).unwrap();
    txn.commit().unwrap();
    let mut prepared = prepared(&store);
    let key = nyaterm_core::sync_schema::SyncFieldId::new(
        nyaterm_core::sync_schema::SyncResourceId::new("connection_group", "group"),
        "name",
    )
    .key();
    let mut next = prepared.plan.expected_local.clone();
    next.values.insert(
        key,
        nyaterm_core::sync_schema::SyncValue::new(&json!("Remote name")),
    );
    let mut replica = SyncReplica::load(&prepared.plan.replica, Uuid::new_v4()).unwrap();
    replica
        .capture(&prepared.plan.effective, &next, &SyncScope::default())
        .unwrap();
    prepared.plan.effective = replica
        .materialize(&prepared.plan.expected_local.values, &SyncScope::default())
        .unwrap();
    prepared.plan.replica = replica.snapshot();
    store.apply_v4_plan(prepared, "password").unwrap();
    let txn = store.db.begin_read().unwrap();
    let table = txn.open_table(super::GROUPS_TABLE).unwrap();
    let raw = table.get("groups/group").unwrap().unwrap();
    let saved: serde_json::Value = serde_json::from_slice(raw.value()).unwrap();
    assert_eq!(saved["name"], "Remote name");
    assert_eq!(saved["future"], json!({"opaque":"keep"}));
}

#[test]
fn preview_does_not_migrate_icons_or_touch_the_excluded_vault() {
    let dir = TestTempDir::new("nyaterm-v4-readonly-prepare");
    let store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    let mut saved = connection();
    saved.icon = Some("data:image/png;base64,aGVsbG8=".into());
    saved.auth = Some(nyaterm_core::ConnectionAuth {
        mode: "password".into(),
        ..Default::default()
    });
    store.save_connection(&saved).unwrap();
    let txn = store.db.begin_write().unwrap();
    // A reader that hydrates credentials would try to decode this excluded
    // record. Prepare must only read the portable connection fields.
    txn.open_table(super::CREDENTIALS_TABLE)
        .unwrap()
        .insert(
            "credentials/connection-password/server",
            b"opaque unsupported credential".as_slice(),
        )
        .unwrap();
    txn.commit().unwrap();
    assert!(store.list_connection_custom_icons().unwrap().is_empty());
    let first = store
        .load_v4_view("destination", "password", &SyncScope::default())
        .unwrap();
    let second = store
        .load_v4_view("destination", "password", &SyncScope::default())
        .unwrap();
    assert_eq!(first.generation, second.generation);
    assert_eq!(first.projection, second.projection);
    assert!(store.list_connection_custom_icons().unwrap().is_empty());
    let txn = store.db.begin_read().unwrap();
    let table = txn.open_table(super::CREDENTIALS_TABLE).unwrap();
    assert_eq!(
        table
            .get("credentials/connection-password/server")
            .unwrap()
            .unwrap()
            .value(),
        b"opaque unsupported credential"
    );
}

#[test]
fn applying_a_nonsecret_edit_keeps_the_existing_inline_password() {
    let dir = TestTempDir::new("nyaterm-v4-inline-password-preservation");
    let store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    store.save_master_password(Some("password")).unwrap();
    let mut saved = connection();
    saved.auth = Some(nyaterm_core::ConnectionAuth {
        mode: "password".into(),
        password: Some("fixture-inline-password".into()),
        ..Default::default()
    });
    store.save_connection(&saved).unwrap();
    let read_slot = || {
        let txn = store.db.begin_read().unwrap();
        let table = txn.open_table(super::CREDENTIALS_TABLE).unwrap();
        table
            .get("credentials/connection-password/server")
            .unwrap()
            .unwrap()
            .value()
            .to_vec()
    };
    let original_slot = read_slot();
    let mut prepared = prepared(&store);
    let mut next = prepared.plan.expected_local.clone();
    let key = nyaterm_core::sync_schema::SyncFieldId::new(
        nyaterm_core::sync_schema::SyncResourceId::new("connection", "server"),
        "name",
    )
    .key();
    next.values.insert(
        key,
        nyaterm_core::sync_schema::SyncValue::new(&json!("Remote name")),
    );
    let mut replica = SyncReplica::load(&prepared.plan.replica, Uuid::new_v4()).unwrap();
    replica
        .capture(&prepared.plan.effective, &next, &SyncScope::default())
        .unwrap();
    prepared.plan.effective = replica
        .materialize(&prepared.plan.expected_local.values, &SyncScope::default())
        .unwrap();
    prepared.plan.replica = replica.snapshot();
    store.apply_v4_plan(prepared, "password").unwrap();
    assert!(original_slot == read_slot());
    let saved = store.get_connection("server").unwrap().unwrap();
    assert_eq!(saved.name, "Remote name");
    assert_eq!(
        saved.auth.unwrap().password.unwrap().expose_secret(),
        "fixture-inline-password"
    );
}

#[test]
fn invalid_reference_is_deferred_while_other_fields_apply_and_causal_data_survives() {
    let dir = TestTempDir::new("nyaterm-v4-invalid-reference-apply");
    let store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    store.save_connection(&connection()).unwrap();
    let mut prepared = prepared(&store);
    let mut next = prepared.plan.expected_local.clone();
    let key = nyaterm_core::sync_schema::SyncFieldId::new(
        nyaterm_core::sync_schema::SyncResourceId::new("connection", "server"),
        "network",
    )
    .key();
    next.values.insert(
        key,
        nyaterm_core::sync_schema::SyncValue::new(&json!({"proxy_jump_id":"server"})),
    );
    next.values.insert(
        nyaterm_core::sync_schema::SyncFieldId::new(
            nyaterm_core::sync_schema::SyncResourceId::new("connection", "server"),
            "name",
        )
        .key(),
        nyaterm_core::sync_schema::SyncValue::new(&json!("Renamed")),
    );
    let mut replica = SyncReplica::load(&prepared.plan.replica, Uuid::new_v4()).unwrap();
    replica
        .capture(&prepared.plan.effective, &next, &SyncScope::default())
        .unwrap();
    prepared.plan.effective = replica
        .materialize(&prepared.plan.expected_local.values, &SyncScope::default())
        .unwrap();
    prepared.plan.replica = replica.snapshot();
    let inspection = store.inspect_v4_application(&prepared.plan).unwrap();
    assert_eq!(inspection.deferred.len(), 1);
    let applied = store.apply_v4_plan(prepared, "password").unwrap();
    assert_eq!(applied.deferred, inspection.deferred);
    assert_eq!(
        store.get_connection("server").unwrap().unwrap().name,
        "Renamed"
    );
    assert!(
        store
            .get_connection("server")
            .unwrap()
            .unwrap()
            .network
            .is_none()
    );
    assert!(
        store
            .load_v4_view("destination", "password", &SyncScope::default())
            .unwrap()
            .record
            .replica
            .is_some()
    );
    let retried = prepared_scope(&store, SyncScope::default());
    assert_eq!(
        store
            .inspect_v4_application(&retried.plan)
            .unwrap()
            .deferred
            .len(),
        1
    );
    assert_eq!(
        applied.replica.unwrap().document,
        retried.plan.replica.document
    );
}

#[test]
fn untouched_legacy_defaults_do_not_generate_repeated_publications() {
    let dir = TestTempDir::new("nyaterm-v4-raw-default-baseline");
    let store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    let txn = store.db.begin_write().unwrap();
    super::write_json_in_txn(&txn, super::CONNECTIONS_TABLE, "connections/server", &json!({"id":"server","type":"ssh","name":"Legacy","host":"example","port":22,"username":"u"})).unwrap();
    txn.commit().unwrap();
    let first = store.apply_v4_plan(prepared(&store), "password").unwrap();
    let next = prepared(&store);
    assert_eq!(
        first.replica.as_ref().unwrap().document,
        next.plan.replica.document
    );
    assert_eq!(next.plan.expected_local.values, first.effective);
    assert!(next.plan.expected_local.edited.is_empty());
}

#[test]
fn missing_authentication_reference_is_deferred_then_applied_when_available() {
    let dir = TestTempDir::new("nyaterm-v4-deferred-auth");
    let store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    store.save_connection(&connection()).unwrap();
    let mut proposal = prepared(&store);
    let mut next = proposal.plan.expected_local.clone();
    let field = nyaterm_core::sync_schema::SyncFieldId::new(
        nyaterm_core::sync_schema::SyncResourceId::new("connection", "server"),
        "authentication",
    );
    next.values.insert(
        field.key(),
        nyaterm_core::sync_schema::SyncValue::new(
            &json!({"mode":"key","key_id":"available-later"}),
        ),
    );
    let mut replica = SyncReplica::load(&proposal.plan.replica, Uuid::new_v4()).unwrap();
    replica
        .capture(&proposal.plan.effective, &next, &SyncScope::default())
        .unwrap();
    proposal.plan.effective = replica
        .materialize(&proposal.plan.expected_local.values, &SyncScope::default())
        .unwrap();
    proposal.plan.replica = replica.snapshot();
    let applied = store.apply_v4_plan(proposal, "password").unwrap();
    assert_eq!(applied.deferred, vec![field]);
    assert!(
        store
            .get_connection("server")
            .unwrap()
            .unwrap()
            .auth
            .is_none()
    );
    store
        .save_ssh_key(
            serde_json::from_value(json!({"id":"available-later","name":"Local key"})).unwrap(),
        )
        .unwrap();
    let next = prepared(&store);
    assert!(
        store
            .inspect_v4_application(&next.plan)
            .unwrap()
            .deferred
            .is_empty()
    );
    let applied = store.apply_v4_plan(next, "password").unwrap();
    assert!(applied.deferred.is_empty());
    assert_eq!(
        store
            .get_connection("server")
            .unwrap()
            .unwrap()
            .auth
            .unwrap()
            .key_id
            .as_deref(),
        Some("available-later")
    );
}

#[test]
fn migration_safety_backup_is_read_only_and_restores_with_the_existing_nya_reader() {
    let source_dir = TestTempDir::new("nyaterm-v4-safety-source");
    let target_dir = TestTempDir::new("nyaterm-v4-safety-target");
    let source = ConnectionStore::open(source_dir.path()).unwrap();
    enable(&source);
    source.save_master_password(Some("password")).unwrap();
    let mut saved = connection();
    saved.icon = Some("data:image/png;base64,aGVsbG8=".into());
    saved.auth = Some(nyaterm_core::ConnectionAuth {
        mode: "password".into(),
        password: Some("fixture-backup-password".into()),
        ..Default::default()
    });
    source.save_connection(&saved).unwrap();
    let before = source
        .load_v4_view("destination", "password", &SyncScope::default())
        .unwrap();
    let file = source_dir.path().join("safety.nya");
    source.export_v4_safety_backup(&file, "password").unwrap();
    let after = source
        .load_v4_view("destination", "password", &SyncScope::default())
        .unwrap();
    assert_eq!(before.generation, after.generation);
    assert_eq!(before.projection, after.projection);
    assert!(source.list_connection_custom_icons().unwrap().is_empty());
    let bytes = std::fs::read(&file).unwrap();
    let backup = crate::decode_encrypted_raw_portable_snapshot(&bytes, "password").unwrap();
    assert!(!backup.entities.keys().any(|key| key.starts_with("sync_v4")));
    assert!(crate::decode_encrypted_raw_portable_snapshot(&bytes, "wrong-password").is_err());
    let target = ConnectionStore::open(target_dir.path()).unwrap();
    target
        .save_master_password(Some("target-password"))
        .unwrap();
    target
        .import_encrypted_portable_snapshot_into_open_store(&file, "password")
        .unwrap();
    assert_eq!(
        target
            .get_connection("server")
            .unwrap()
            .unwrap()
            .auth
            .unwrap()
            .password
            .unwrap()
            .expose_secret(),
        "fixture-backup-password"
    );
    let restored = target
        .load_v4_view("destination", "password", &SyncScope::default())
        .unwrap();
    assert!(restored.record.replica.is_none());
    assert!(restored.record.pending.is_none());
}

#[test]
fn appearance_apply_preserves_the_live_security_verifier_and_device_settings() {
    let dir = TestTempDir::new("nyaterm-v4-settings-security");
    let store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    store.save_master_password(Some("password")).unwrap();
    let mut raw = store.load_settings_value().unwrap();
    let verifier = raw["security"]["master_password"].clone();
    raw["security"]["future_security"] = json!(42);
    raw["appearance"] = json!({"theme":"github-dark","font_size":14,"window_material":"mica"});
    store.save_settings_value(&raw).unwrap();
    let mut settings = store.load_cloud_sync_settings().unwrap();
    settings.v4_scope.preferences = true;
    store.save_cloud_sync_settings(settings).unwrap();
    let scope = SyncScope {
        preferences: true,
        ..Default::default()
    };
    let view = store.load_v4_view("settings", "password", &scope).unwrap();
    let mut remote_projection = nyaterm_core::sync_schema::SyncProjection::default();
    remote_projection.complete.insert("preference".into());
    remote_projection
        .record("preference", "appearance/font_size", &json!({"value":23}))
        .unwrap();
    let mut remote = SyncReplica::new(Uuid::new_v4());
    remote
        .capture(&Default::default(), &remote_projection, &scope)
        .unwrap();
    let plan = prepare(
        view.record.writer,
        None,
        &Default::default(),
        view.projection,
        &[remote.snapshot()],
        scope,
    )
    .unwrap();
    store
        .apply_v4_plan(
            PreparedApply {
                destination: "settings".into(),
                expected_record: view.record_hash,
                generation: view.generation,
                local: view.record,
                plan,
                trust_approvals: Default::default(),
            },
            "password",
        )
        .unwrap();
    let after = store.load_settings_value().unwrap();
    assert_eq!(after["security"]["master_password"], verifier);
    assert!(store.verify_master_password("password").unwrap());
    assert_eq!(after["security"]["future_security"], 42);
    assert_eq!(after["appearance"]["window_material"], "mica");
}
