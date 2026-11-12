use nyaterm_core::SavedConnection;
use nyaterm_core::sync_schema::SyncScope;
use nyaterm_core::test_support::TestTempDir;
use nyaterm_sync::engine::{SyncReplica, prepare};
use redb::{ReadableDatabase, ReadableTable};
use serde_json::json;
use uuid::Uuid;

use crate::sync_v4::PreparedApply;
use crate::{ConnectionStore, StorageError};

pub(super) fn connection() -> SavedConnection {
    serde_json::from_value(json!({"id":"server","name":"Server","type":"ssh","host":"server.example","port":22,"username":"root"})).unwrap()
}

#[test]
fn ssh_password_and_key_coexist_encrypted_and_survive_backup_and_sync() {
    let dir = TestTempDir::new("nyaterm-ssh-coexisting-credentials");
    let mut store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    store
        .save_ssh_key(
            serde_json::from_value(json!({
                "id":"fixture-key", "name":"Fixture key", "key":"fixture-key-data"
            }))
            .unwrap(),
        )
        .unwrap();
    for mode in ["key", "password", "agent", "none"] {
        let mut saved = connection();
        saved.auth = Some(serde_json::from_value(json!({
            "mode":mode, "password_source":"connection", "password":" fixture-password ", "key_id":"fixture-key"
        })).unwrap());
        store.save_connection(&saved).unwrap();
        let record = crate::storage::tests::connection_password_record(&store, &saved.id);
        assert_ne!(record.password.expose_secret(), " fixture-password ");
        let original_ciphertext = record.password;
        drop(store);
        store = ConnectionStore::open(dir.path()).unwrap();
        let expected = store.get_connection(&saved.id).unwrap().unwrap();
        assert_eq!(expected.auth, saved.auth);

        let backup_dir = TestTempDir::new("nyaterm-ssh-coexisting-backup");
        let backup = backup_dir.path().join("connections.nya");
        store
            .export_encrypted_portable_snapshot_from_open_store(
                &backup, "source", "fixture", "password",
            )
            .unwrap();
        let bytes = std::fs::read(&backup).unwrap();
        assert!(crate::decode_encrypted_raw_portable_snapshot(&bytes, "wrong-password").is_err());
        let restored = ConnectionStore::open(backup_dir.path().join("restored")).unwrap();
        restored
            .import_encrypted_portable_snapshot_into_open_store(&backup, "password")
            .unwrap();
        assert_eq!(
            restored.get_connection(&saved.id).unwrap().unwrap().auth,
            expected.auth
        );
        assert_eq!(
            restored
                .load_decrypted_ssh_key_by_id("fixture-key")
                .unwrap()
                .unwrap()
                .key_data
                .as_deref(),
            Some("fixture-key-data")
        );

        store.apply_v4_plan(prepared(&store), "password").unwrap();
        let view = store
            .load_v4_view("destination", "password", &SyncScope::default())
            .unwrap();
        assert!(view.projection.edited.is_empty());
        assert_eq!(view.record.effective, view.projection.values);
        assert_eq!(
            store.get_connection(&saved.id).unwrap().unwrap().auth,
            expected.auth
        );
        assert_eq!(
            crate::storage::tests::connection_password_record(&store, &saved.id).password,
            original_ciphertext
        );
    }
    store.delete_connection("server").unwrap();
    let txn = store.db.begin_read().unwrap();
    let table = txn.open_table(crate::storage::CREDENTIALS_TABLE).unwrap();
    let key = crate::storage::entity_key(crate::storage::CONNECTION_PASSWORD_PREFIX, "server");
    assert!(table.get(key.as_str()).unwrap().is_none());
}

#[test]
fn rdp_backend_aliases_authentication_and_clipboard_survive_backup_and_sync() {
    for (backend, canonical) in [
        ("auto", "builtin"),
        ("windows", "windows"),
        ("active-x", "windows"),
        ("future-engine", "future-engine"),
    ] {
        let dir = TestTempDir::new("nyaterm-rdp-backup-sync");
        let store = ConnectionStore::open(dir.path()).unwrap();
        enable(&store);
        let saved: SavedConnection = serde_json::from_value(json!({
            "id":"rdp-fixture", "name":"RDP", "type":"rdp", "host":"example.invalid",
            "port":3389, "username":"fixture", "backend":backend,
            "security":{"windows_authentication":"system", "use_nla":true,
                "certificate_policy":"strict", "administrative_session":true},
            "clipboard":{"mode":"disabled"}
        }))
        .unwrap();
        store.save_connection(&saved).unwrap();
        let expected = store.get_connection("rdp-fixture").unwrap().unwrap();
        let serialized = serde_json::to_value(&expected).unwrap();
        assert_eq!(serialized["backend"], canonical);
        assert_eq!(serialized["security"]["windows_authentication"], "system");
        assert_eq!(serialized["security"]["certificate_policy"], "strict");
        assert_eq!(serialized["clipboard"]["mode"], "disabled");

        let mut snapshot = store
            .build_raw_portable_snapshot(
                nyaterm_core::PortableSnapshotKind::Backup,
                "source",
                "fixture",
            )
            .unwrap();
        snapshot.recalculate_hash().unwrap();
        let destination_dir = TestTempDir::new("nyaterm-rdp-backup-restore");
        let destination = ConnectionStore::open(destination_dir.path()).unwrap();
        destination.apply_raw_portable_snapshot(&snapshot).unwrap();
        let restored = destination.get_connection("rdp-fixture").unwrap().unwrap();
        // Restoring records a fresh local modification timestamp. The RDP and
        // credential contracts must survive that ordinary metadata update.
        assert_eq!(restored.config, expected.config);
        assert_eq!(restored.auth, expected.auth);

        store.apply_v4_plan(prepared(&store), "password").unwrap();
        let reopened = store
            .load_v4_view("destination", "password", &SyncScope::default())
            .unwrap();
        assert!(reopened.projection.edited.is_empty());
        assert_eq!(reopened.record.effective, reopened.projection.values);
        let synced = store.get_connection("rdp-fixture").unwrap().unwrap();
        assert_eq!(synced.config, expected.config);
        assert_eq!(synced.auth, expected.auth);
    }
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

pub(super) fn enable(store: &ConnectionStore) {
    let mut settings = store.load_cloud_sync_settings().unwrap();
    settings.protocol_version = 4;
    store.save_cloud_sync_settings(settings).unwrap();
}

#[test]
fn provider_switch_preserves_per_destination_v4_records() {
    let dir = TestTempDir::new("nyaterm-provider-v4-records");
    let store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    store.apply_v4_plan(prepared(&store), "password").unwrap();
    let before = store
        .load_v4_view("destination", "password", &SyncScope::default())
        .unwrap();
    for provider in ["webdav", "github_gist", "webdav"] {
        let mut settings = store.load_cloud_sync_settings().unwrap();
        settings.provider = provider.into();
        store.save_cloud_sync_settings(settings).unwrap();
        let after = store
            .load_v4_view("destination", "password", &SyncScope::default())
            .unwrap();
        assert_eq!(after.record_hash, before.record_hash);
        assert_eq!(after.record.writer, before.record.writer);
        assert_eq!(
            store.load_cloud_sync_settings().unwrap().protocol_version,
            4
        );
    }
}

pub(super) fn prepared(store: &ConnectionStore) -> PreparedApply {
    prepared_scope(store, SyncScope::default())
}

#[test]
fn fixed_snapshot_round_trip_preserves_local_vault_settings_and_unknown_fields() {
    let source_dir = TestTempDir::new("nyaterm-fixed-snapshot-source");
    let target_dir = TestTempDir::new("nyaterm-fixed-snapshot-target");
    let source = ConnectionStore::open(source_dir.path()).unwrap();
    let target = ConnectionStore::open(target_dir.path()).unwrap();
    source
        .save_master_password(Some("source-password"))
        .unwrap();
    target
        .save_master_password(Some("target-password"))
        .unwrap();
    let mut remote = connection();
    remote.name = "Remote name".into();
    source.save_connection(&remote).unwrap();
    source.save_password(serde_json::from_value(json!({"id":"remote-account","name":"Remote account","password":"fixture-remote-secret"})).unwrap()).unwrap();
    let mut local = connection();
    local.auth = Some(nyaterm_core::ConnectionAuth {
        mode: "password".into(),
        password: Some("fixture-local-inline-secret".into()),
        ..Default::default()
    });
    target.save_connection(&local).unwrap();
    let mut record = target
        .list_raw_json_values_by_prefix(super::CONNECTIONS_TABLE, "connections/")
        .unwrap()
        .remove(0);
    let txn = target.db.begin_write().unwrap();
    record["future_local"] = json!({"opaque":"keep"});
    super::write_json_in_txn(
        &txn,
        super::CONNECTIONS_TABLE,
        "connections/server",
        &record,
    )
    .unwrap();
    txn.commit().unwrap();
    let vault_before = target
        .list_raw_json_values_by_prefix(super::CREDENTIALS_TABLE, "credentials/")
        .unwrap();
    let mut settings = target.load_settings_value().unwrap();
    settings["appearance"]["theme"] = json!("github-dark");
    settings["appearance"]["font_family"] = json!("Local Font");
    settings["future_local_settings"] = json!({"opaque":"keep"});
    settings["keybindings"] = json!({"copy":"ctrl-shift-c"});
    target.save_settings_value(&settings).unwrap();
    let before = target.load_settings_value().unwrap();
    let mut custom = nyaterm_core::themes::builtins()[0].clone();
    custom.id = "fixed-custom-theme".into();
    let mut source_settings = source.load_settings_value().unwrap();
    source_settings["appearance"]["custom_themes"] = json!([custom]);
    source.save_settings_value(&source_settings).unwrap();
    let fixed = source
        .build_fixed_sync_snapshot("source", "version")
        .unwrap();
    assert_eq!(fixed.meta.schema_version, 4);
    let encoded = crate::encode_encrypted_raw_portable_snapshot(&fixed, "shared-password").unwrap();
    let decoded =
        crate::decode_encrypted_raw_portable_snapshot(&encoded, "shared-password").unwrap();
    assert!(target.apply_raw_portable_snapshot(&decoded).is_err());
    target
        .apply_fixed_sync_snapshot(&decoded, "shared-password")
        .unwrap();
    assert_eq!(
        target
            .list_raw_json_values_by_prefix(super::CREDENTIALS_TABLE, "credentials/")
            .unwrap(),
        vault_before
    );
    assert!(
        target
            .load_decrypted_password_by_id("remote-account")
            .unwrap()
            .is_none()
    );
    let saved = target.get_connection("server").unwrap().unwrap();
    assert_eq!(saved.name, "Remote name");
    let raw = target
        .list_raw_json_values_by_prefix(super::CONNECTIONS_TABLE, "connections/")
        .unwrap();
    assert_eq!(raw[0]["future_local"], json!({"opaque":"keep"}));
    let after = target.load_settings_value().unwrap();
    for field in [
        "security",
        "keybindings",
        "ai",
        "cloud_sync",
        "future_local_settings",
    ] {
        assert_eq!(after.get(field), before.get(field));
    }
    assert_eq!(after["appearance"]["theme"], before["appearance"]["theme"]);
    assert_eq!(
        after["appearance"]["font_family"],
        before["appearance"]["font_family"]
    );
    assert_eq!(
        after["appearance"]["custom_themes"][0]["id"],
        "fixed-custom-theme"
    );
    assert!(!target.has_v4_replicas().unwrap());
}

#[test]
fn both_sync_modes_round_trip_all_core_collections_edits_relationships_and_deletions() {
    use nyaterm_core::portable_snapshot::{
        project_fixed_sync_entities, project_fixed_sync_snapshot,
    };
    use nyaterm_core::sync_schema::{CORE_KINDS, FIXED_CLOUD_SYNC_SCOPE};
    for field_mode in [false, true] {
        let source_dir = TestTempDir::new("nyaterm-fixed-all-source");
        let target_dir = TestTempDir::new("nyaterm-fixed-all-target");
        let source = ConnectionStore::open(source_dir.path()).unwrap();
        let target = ConnectionStore::open(target_dir.path()).unwrap();
        enable(&source);
        enable(&target);
        for key in [super::SETTINGS_PROXY_GROUPS, super::SETTINGS_TUNNEL_GROUPS] {
            target
                .save_settings_doc_value(key, &json!({"groups":[], "future_local":{"keep":true}}))
                .unwrap();
        }
        let mut fixture = nyaterm_core::RawPortableSnapshot::sync("source", "version");
        fixture.entities.insert("sessions".into(), json!({
            "connections":[{"id":"server","name":"Server","type":"ssh","host":"server.example","port":22,"username":"root","group_id":"cg","icon":"custom:ci","icon_auto_detect":false}],
            "groups":[{"id":"cg","name":"Connections"}],
            "custom_icons":[{"id":"ci","name":"Icon","data_url":"data:image/png;base64,AA=="}]
        }).to_string());
        fixture.entities.insert("proxies".into(), json!([{"id":"proxy","name":"Proxy","protocol":"socks5","host":"proxy.example","port":1080,"group_id":"pg"}]).to_string());
        fixture.entities.insert(
            "proxy_groups".into(),
            json!([{"id":"pg","name":"Proxies"}]).to_string(),
        );
        fixture.entities.insert("tunnels".into(), json!([{"id":"tunnel","name":"Tunnel","connection_id":"server","listen_port":9000,"target_port":80,"group_id":"tg"}]).to_string());
        fixture.entities.insert(
            "tunnel_groups".into(),
            json!([{"id":"tg","name":"Tunnels"}]).to_string(),
        );
        fixture.entities.insert("quick_commands".into(), json!({"commands":[{"id":"command","label":"Command","command":"pwd","category_id":"cc"}],"categories":[{"id":"cc","name":"Commands"}]}).to_string());
        fixture.entities.insert("notes".into(), json!({"notes":[{"id":"note","title":"Note","markdown":"Text","parent_id":"nf","sort_order":0}],"folders":[{"id":"nf","name":"Notes","sort_order":0}]}).to_string());
        let mut theme = nyaterm_core::themes::builtins()[0].clone();
        theme.id = "theme".into();
        fixture.entities.insert(
            "settings".into(),
            json!({"appearance":{"custom_themes":[theme]}}).to_string(),
        );
        let fixture = project_fixed_sync_snapshot(&fixture).unwrap();
        source
            .apply_fixed_sync_snapshot(&fixture, "password")
            .unwrap();
        let mut wire = SyncReplica::new(Uuid::new_v4());
        let mut baseline = Default::default();
        for phase in 0..3 {
            if phase == 1 {
                let mut edit = source
                    .build_fixed_sync_snapshot("source", "version")
                    .unwrap();
                for raw in edit.entities.values_mut() {
                    let mut value: serde_json::Value = serde_json::from_str(raw).unwrap();
                    fn rename(value: &mut serde_json::Value) {
                        match value {
                            serde_json::Value::Array(items) => items.iter_mut().for_each(rename),
                            serde_json::Value::Object(fields) => {
                                if fields.contains_key("id") {
                                    for key in ["name", "label", "title"] {
                                        if let Some(value) = fields.get_mut(key) {
                                            *value = json!("Edited");
                                        }
                                    }
                                }
                                fields.values_mut().for_each(rename);
                            }
                            _ => {}
                        }
                    }
                    rename(&mut value);
                    *raw = value.to_string();
                }
                edit.recalculate_hash().unwrap();
                source.apply_fixed_sync_snapshot(&edit, "password").unwrap();
            } else if phase == 2 {
                let empty_dir = TestTempDir::new("nyaterm-fixed-all-empty");
                let empty = ConnectionStore::open(empty_dir.path())
                    .unwrap()
                    .build_fixed_sync_snapshot("empty", "version")
                    .unwrap();
                source
                    .apply_fixed_sync_snapshot(&empty, "password")
                    .unwrap();
            }
            let snapshot = source
                .build_fixed_sync_snapshot("source", "version")
                .unwrap();
            let projection = project_fixed_sync_entities(&snapshot).unwrap();
            if field_mode {
                wire.capture(&baseline, &projection, &FIXED_CLOUD_SYNC_SCOPE)
                    .unwrap();
                let remote = wire.fixed_snapshot().unwrap();
                let mut proposal = prepared(&target);
                proposal.plan = prepare(
                    proposal.local.writer,
                    proposal.local.replica.as_ref(),
                    &proposal.local.effective,
                    proposal.plan.expected_local.clone(),
                    &[remote],
                    FIXED_CLOUD_SYNC_SCOPE,
                )
                .unwrap();
                target.apply_v4_plan(proposal, "password").unwrap();
            } else {
                target
                    .apply_fixed_sync_snapshot(&snapshot, "password")
                    .unwrap();
            }
            baseline = projection.values.clone();
            for key in [super::SETTINGS_PROXY_GROUPS, super::SETTINGS_TUNNEL_GROUPS] {
                assert_eq!(
                    target.load_settings_doc_value(key, json!({})).unwrap()["future_local"],
                    json!({"keep":true})
                );
            }
            let actual = project_fixed_sync_entities(
                &target
                    .build_fixed_sync_snapshot("target", "version")
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                actual.values, projection.values,
                "mode={field_mode}, phase={phase}"
            );
            for kind in CORE_KINDS {
                let present = actual
                    .values
                    .keys()
                    .filter_map(|key| nyaterm_core::sync_schema::SyncFieldId::from_key(key))
                    .any(|field| field.resource.kind == *kind && field.field == "$present");
                assert_eq!(present, phase != 2, "kind={kind}");
            }
        }
    }
}

#[test]
fn incomplete_legacy_snapshot_preserves_local_notes_and_themes() {
    let dir = TestTempDir::new("nyaterm-fixed-incomplete-snapshot");
    let store = ConnectionStore::open(dir.path()).unwrap();
    store
        .create_note(None, Some("Local note".into()), None)
        .unwrap();
    let before = store.load_notes_snapshot().unwrap();
    let mut settings = store.load_settings_value().unwrap();
    let mut theme = nyaterm_core::themes::builtins()[0].clone();
    theme.id = "local-theme".into();
    settings["appearance"]["custom_themes"] = json!([theme]);
    store.save_settings_value(&settings).unwrap();
    let mut incoming = nyaterm_core::RawPortableSnapshot::sync("source", "version");
    incoming.entities.remove("notes");
    incoming
        .entities
        .insert("settings".into(), json!({}).to_string());
    incoming.recalculate_hash().unwrap();
    let incoming = nyaterm_core::portable_snapshot::project_fixed_sync_snapshot(&incoming).unwrap();
    store
        .apply_fixed_sync_snapshot(&incoming, "password")
        .unwrap();
    assert_eq!(store.load_notes_snapshot().unwrap(), before);
    assert_eq!(store.load_settings_value().unwrap(), settings);
}

#[test]
fn old_scope_settings_round_trip_but_cannot_authorize_a_vault_apply() {
    let dir = TestTempDir::new("nyaterm-fixed-old-scope");
    let store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    let mut settings = store.load_cloud_sync_settings().unwrap();
    settings.v4_scope = SyncScope {
        secrets: true,
        preferences: true,
        ai: true,
        keybindings: true,
        trusted_hosts: true,
    };
    store.save_cloud_sync_settings(settings.clone()).unwrap();
    assert_eq!(
        store.load_cloud_sync_settings().unwrap().v4_scope,
        settings.v4_scope
    );
    let prepared = prepared_scope(&store, settings.v4_scope);
    assert!(store.apply_v4_plan(prepared, "password").is_err());
    let fixed = store
        .build_fixed_sync_snapshot("device", "version")
        .unwrap();
    let projection = nyaterm_core::portable_snapshot::project_fixed_sync_entities(&fixed).unwrap();
    assert!(projection.values.keys().all(|key| {
        nyaterm_core::sync_schema::SyncFieldId::from_key(key)
            .is_some_and(|field| nyaterm_core::sync_schema::fixed_cloud_sync_field(&field))
    }));
}

#[test]
fn fixed_snapshot_keeps_proxy_ciphertext_and_defers_credential_redirection() {
    let dir = TestTempDir::new("nyaterm-fixed-proxy-auth");
    let store = ConnectionStore::open(dir.path()).unwrap();
    store.save_master_password(Some("password")).unwrap();
    store
        .replace_proxies(&[nyaterm_core::ProxyConfig {
            id: "proxy".into(),
            name: "Local".into(),
            host: "proxy.example".into(),
            password: Some("fixture-local-proxy-secret".into()),
            ..Default::default()
        }])
        .unwrap();
    let before = store
        .list_raw_json_values_by_prefix(super::PROXIES_TABLE, super::PROXY_PREFIX)
        .unwrap()[0]["password"]
        .clone();
    for redirect in [false, true] {
        let mut snapshot = store
            .build_fixed_sync_snapshot("source", "version")
            .unwrap();
        let mut proxies: serde_json::Value =
            serde_json::from_str(&snapshot.entities["proxies"]).unwrap();
        proxies[0]["name"] = json!("Remote rename");
        if redirect {
            proxies[0]["host"] = json!("other.example");
        }
        snapshot
            .entities
            .insert("proxies".into(), proxies.to_string());
        snapshot.recalculate_hash().unwrap();
        store
            .apply_fixed_sync_snapshot(&snapshot, "password")
            .unwrap();
        let raw = store
            .list_raw_json_values_by_prefix(super::PROXIES_TABLE, super::PROXY_PREFIX)
            .unwrap()
            .remove(0);
        assert_eq!(raw["password"], before);
        assert_eq!(raw["name"], "Remote rename");
        assert_eq!(raw["host"], "proxy.example");
    }
}

#[test]
fn fixed_snapshot_defers_redirecting_a_connection_with_local_authentication() {
    let dir = TestTempDir::new("nyaterm-fixed-snapshot-redirect");
    let store = ConnectionStore::open(dir.path()).unwrap();
    store.save_master_password(Some("password")).unwrap();
    let mut saved = connection();
    saved.auth = Some(nyaterm_core::ConnectionAuth {
        mode: "password".into(),
        password: Some("fixture-local-secret".into()),
        ..Default::default()
    });
    store.save_connection(&saved).unwrap();
    let mut incoming = store
        .build_fixed_sync_snapshot("source", "version")
        .unwrap();
    let mut sessions: serde_json::Value =
        serde_json::from_str(&incoming.entities["sessions"]).unwrap();
    sessions["connections"][0]["host"] = json!("other.example");
    sessions["connections"][0]["name"] = json!("Renamed");
    incoming
        .entities
        .insert("sessions".into(), sessions.to_string());
    incoming.recalculate_hash().unwrap();
    store
        .apply_fixed_sync_snapshot(&incoming, "password")
        .unwrap();
    let actual = store.get_connection("server").unwrap().unwrap();
    assert_eq!(actual.name, "Renamed");
    assert_eq!(actual.config, saved.config);
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
fn disabling_cloud_sync_does_not_bypass_replica_password_rotation_guards() {
    let dir = TestTempDir::new("nyaterm-v4-disabled-sync-password");
    let store = ConnectionStore::open(dir.path()).unwrap();
    store.save_master_password(Some("password")).unwrap();
    enable(&store);
    store.apply_v4_plan(prepared(&store), "password").unwrap();
    let before = store
        .load_v4_replica("destination", "password")
        .unwrap()
        .unwrap();
    let mut settings = store.load_cloud_sync_settings().unwrap();
    settings.enabled = false;
    store.save_cloud_sync_settings(settings).unwrap();
    drop(store);

    let store = ConnectionStore::open(dir.path()).unwrap();
    assert!(!store.load_cloud_sync_settings().unwrap().enabled);
    assert!(store.save_master_password(Some("next-password")).is_err());
    assert!(store.save_master_password(None).is_err());
    assert_eq!(
        store.master_password_matches("password").unwrap(),
        Some(true)
    );
    assert!(
        store
            .load_v4_replica("destination", "password")
            .unwrap()
            .unwrap()
            == before
    );
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
    assert!(
        store
            .update_auto_detected_connection_icon("server", "linux")
            .unwrap()
    );
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
fn ssh_compatibility_mode_survives_database_json_backup_and_cloud_sync_round_trips() {
    let dir = TestTempDir::new("nyaterm-ssh-compatibility-storage");
    let mut store = ConnectionStore::open(dir.path()).unwrap();
    enable(&store);
    for enabled in [true, false] {
        let mut saved = connection();
        saved.ssh_compatibility_mode = enabled;
        store.save_connection(&saved).unwrap();
        drop(store);
        store = ConnectionStore::open(dir.path()).unwrap();
        let loaded = store.get_connection(&saved.id).unwrap().unwrap();
        assert_eq!(loaded.ssh_compatibility_mode, enabled);

        let import_dir = TestTempDir::new("nyaterm-ssh-compatibility-json");
        std::fs::create_dir_all(import_dir.path()).unwrap();
        let json_path = import_dir.path().join("connections.json");
        std::fs::write(
            &json_path,
            serde_json::to_vec(&json!({"version":1,"sessions":[loaded]})).unwrap(),
        )
        .unwrap();
        let prepared_import =
            nyaterm_core::session_import::prepare_session_import(&json_path).unwrap();
        assert_eq!(
            prepared_import.connections[0]
                .saved
                .as_ref()
                .unwrap()
                .ssh_compatibility_mode,
            enabled
        );
        let imported = ConnectionStore::open(import_dir.path().join("imported")).unwrap();
        assert_eq!(imported.commit_session_import(prepared_import).unwrap(), 1);
        assert_eq!(
            imported.list_connections().unwrap()[0].ssh_compatibility_mode,
            enabled
        );

        let backup_dir = TestTempDir::new("nyaterm-ssh-compatibility-backup");
        let backup = backup_dir.path().join("connections.nya");
        store
            .export_encrypted_portable_snapshot_from_open_store(
                &backup, "source", "fixture", "password",
            )
            .unwrap();
        let restored = ConnectionStore::open(backup_dir.path().join("restored")).unwrap();
        restored
            .import_encrypted_portable_snapshot_into_open_store(&backup, "password")
            .unwrap();
        assert_eq!(
            restored
                .get_connection(&saved.id)
                .unwrap()
                .unwrap()
                .ssh_compatibility_mode,
            enabled
        );

        store.apply_v4_plan(prepared(&store), "password").unwrap();
        let view = store
            .load_v4_view("destination", "password", &SyncScope::default())
            .unwrap();
        assert!(view.projection.edited.is_empty());
        assert_eq!(view.record.effective, view.projection.values);
        assert_eq!(
            store
                .get_connection(&saved.id)
                .unwrap()
                .unwrap()
                .ssh_compatibility_mode,
            enabled
        );
    }
}

#[test]
fn compressed_local_replica_round_trips_through_store() {
    let dir = TestTempDir::new("nyaterm-compressed-local-replica");
    let store = ConnectionStore::open(dir.path()).unwrap();
    let record = crate::sync_v4::LocalReplica {
        owner: "fixture-owner".into(),
        effective: std::collections::BTreeMap::from([(
            "connection/server/name".into(),
            nyaterm_core::sync_schema::SyncValue::new(&json!("Server")),
        )]),
        ..Default::default()
    };
    store
        .persist_v4_record("destination", "fixture-password", None, &record)
        .unwrap();
    let (loaded, _) = store
        .load_v4_replica("destination", "fixture-password")
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_vec(&loaded).unwrap(),
        serde_json::to_vec(&record).unwrap()
    );
    let txn = store.db.begin_read().unwrap();
    let stored = txn
        .open_table(super::REPLICAS)
        .unwrap()
        .get("destination")
        .unwrap()
        .unwrap();
    assert!(stored.value().starts_with(b"NYASYNC5"));
}

#[test]
fn legacy_local_replica_records_read_back_identically() {
    let dir = TestTempDir::new("nyaterm-legacy-local-replica");
    let store = ConnectionStore::open(dir.path()).unwrap();
    let record = crate::sync_v4::LocalReplica {
        owner: "fixture-owner".into(),
        effective: std::collections::BTreeMap::from([(
            "connection/server/name".into(),
            nyaterm_core::sync_schema::SyncValue::new(&json!("Server")),
        )]),
        ..Default::default()
    };
    // A record written by a build that predates the compressed envelope.
    let plaintext = serde_json::to_vec(&record).unwrap();
    let legacy = nyaterm_core::sync_schema::encryption::seal(
        "fixture-password",
        "local-replica",
        "destination",
        &plaintext,
    )
    .unwrap();
    assert!(legacy.starts_with(b"NYASYNC4"));
    let txn = store.db.begin_write().unwrap();
    txn.open_table(super::REPLICAS)
        .unwrap()
        .insert("destination", legacy.as_slice())
        .unwrap();
    txn.commit().unwrap();
    let (loaded, hash) = store
        .load_v4_replica("destination", "fixture-password")
        .unwrap()
        .unwrap();
    let expected = serde_json::to_vec(&record).unwrap();
    assert_eq!(serde_json::to_vec(&loaded).unwrap(), expected);

    // Rewriting the same replica compresses it, and both encodings decode to
    // the same record.
    store
        .persist_v4_record("destination", "fixture-password", Some(&hash), &record)
        .unwrap();
    let (rewritten, _) = store
        .load_v4_replica("destination", "fixture-password")
        .unwrap()
        .unwrap();
    assert_eq!(serde_json::to_vec(&rewritten).unwrap(), expected);
    let txn = store.db.begin_read().unwrap();
    let stored = txn
        .open_table(super::REPLICAS)
        .unwrap()
        .get("destination")
        .unwrap()
        .unwrap();
    assert!(stored.value().starts_with(b"NYASYNC5"));
}

#[test]
fn consumed_field_edits_are_purged_and_newer_edits_are_kept() {
    let dir = TestTempDir::new("nyaterm-purge-consumed-edits");
    let store = ConnectionStore::open(dir.path()).unwrap();
    let txn = store.db.begin_write().unwrap();
    {
        let mut edits = txn.open_table(super::EDITS).unwrap();
        edits.insert("field/consumed", 3).unwrap();
        edits.insert("field/applied", 7).unwrap();
        edits.insert("field/newer", 9).unwrap();
    }
    super::purge_consumed_edits(&txn, 7).unwrap();
    // Repeated cleanup inside the same transaction stays harmless.
    super::purge_consumed_edits(&txn, 7).unwrap();
    txn.commit().unwrap();
    let txn = store.db.begin_read().unwrap();
    let edits = txn.open_table(super::EDITS).unwrap();
    assert!(edits.get("field/consumed").unwrap().is_none());
    assert!(edits.get("field/applied").unwrap().is_none());
    assert_eq!(edits.get("field/newer").unwrap().unwrap().value(), 9);
}

pub(super) fn migration_record(applied_generation: u64) -> crate::sync_v4::LocalReplica {
    crate::sync_v4::LocalReplica {
        owner: "fixture-owner".into(),
        applied_generation,
        effective: std::collections::BTreeMap::from([(
            "connection/server/name".into(),
            nyaterm_core::sync_schema::SyncValue::new(&json!("Server")),
        )]),
        ..Default::default()
    }
}

pub(super) fn legacy_bytes(record: &crate::sync_v4::LocalReplica, destination: &str) -> Vec<u8> {
    nyaterm_core::sync_schema::encryption::seal(
        "fixture-password",
        "local-replica",
        destination,
        &serde_json::to_vec(record).unwrap(),
    )
    .unwrap()
}

#[test]
fn legacy_replicas_migrate_once_and_purge_consumed_edits() {
    let dir = TestTempDir::new("nyaterm-local-replica-migration");
    let record = migration_record(7);
    let expected = serde_json::to_vec(&record).unwrap();
    let legacy = legacy_bytes(&record, "destination");
    {
        let mut store = ConnectionStore::open(dir.path()).unwrap();
        store
            .save_master_password(Some("fixture-password"))
            .unwrap();
        let txn = store.db.begin_write().unwrap();
        txn.open_table(crate::storage::META_TABLE)
            .unwrap()
            .insert(super::GENERATION, "9")
            .unwrap();
        txn.open_table(super::REPLICAS)
            .unwrap()
            .insert("destination", legacy.as_slice())
            .unwrap();
        {
            let mut edits = txn.open_table(super::EDITS).unwrap();
            edits.insert("field/consumed", 3).unwrap();
            edits.insert("field/applied", 7).unwrap();
            edits.insert("field/newer", 9).unwrap();
        }
        txn.commit().unwrap();
        assert!(store.migrate_local_replica_storage().unwrap());
        let txn = store.db.begin_read().unwrap();
        {
            let stored = txn
                .open_table(super::REPLICAS)
                .unwrap()
                .get("destination")
                .unwrap()
                .unwrap();
            assert!(stored.value().starts_with(b"NYASYNC5"));
        }
        {
            let edits = txn.open_table(super::EDITS).unwrap();
            assert!(edits.get("field/consumed").unwrap().is_none());
            assert!(edits.get("field/applied").unwrap().is_none());
            assert_eq!(edits.get("field/newer").unwrap().unwrap().value(), 9);
        }
        {
            let marker = txn
                .open_table(crate::storage::META_TABLE)
                .unwrap()
                .get(super::LOCAL_REPLICA_MIGRATION)
                .unwrap()
                .unwrap();
            assert_eq!(marker.value(), "done");
        }
        drop(txn);
        let (loaded, _) = store
            .load_v4_replica("destination", "fixture-password")
            .unwrap()
            .unwrap();
        assert_eq!(serde_json::to_vec(&loaded).unwrap(), expected);

        // A finished migration re-runs neither the rewrite nor the cleanup.
        let txn = store.db.begin_write().unwrap();
        txn.open_table(super::EDITS)
            .unwrap()
            .insert("field/late", 1)
            .unwrap();
        txn.commit().unwrap();
        assert!(!store.migrate_local_replica_storage().unwrap());
        let txn = store.db.begin_read().unwrap();
        let edits = txn.open_table(super::EDITS).unwrap();
        assert_eq!(edits.get("field/late").unwrap().unwrap().value(), 1);
    }
    let mut store = ConnectionStore::open(dir.path()).unwrap();
    assert!(!store.migrate_local_replica_storage().unwrap());
}

#[test]
fn legacy_replica_migration_reclaims_file_space() {
    let dir = TestTempDir::new("nyaterm-local-replica-migration-space");
    let path = dir.path().join("nyaterm.redb");
    let mut record = migration_record(0);
    for index in 0..4096 {
        record.effective.insert(
            format!("connection/server/field-{index}"),
            nyaterm_core::sync_schema::SyncValue::new(&json!("x".repeat(120))),
        );
    }
    let mut store = ConnectionStore::open(dir.path()).unwrap();
    store
        .save_master_password(Some("fixture-password"))
        .unwrap();
    let legacy = legacy_bytes(&record, "destination");
    let txn = store.db.begin_write().unwrap();
    txn.open_table(super::REPLICAS)
        .unwrap()
        .insert("destination", legacy.as_slice())
        .unwrap();
    txn.commit().unwrap();
    let before = std::fs::metadata(&path).unwrap().len();
    assert!(store.migrate_local_replica_storage().unwrap());
    let after = std::fs::metadata(&path).unwrap().len();
    assert!(after < before, "before={before} after={after}");
    let (loaded, _) = store
        .load_v4_replica("destination", "fixture-password")
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_vec(&loaded).unwrap(),
        serde_json::to_vec(&record).unwrap()
    );
}

#[test]
fn unreadable_legacy_replica_aborts_migration_without_marker() {
    let dir = TestTempDir::new("nyaterm-local-replica-migration-failure");
    let record = migration_record(7);
    let expected = serde_json::to_vec(&record).unwrap();
    let legacy = legacy_bytes(&record, "good");
    let mut broken = b"NYASYNC4".to_vec();
    broken.extend_from_slice(&[0xFF; 32]);
    let mut store = ConnectionStore::open(dir.path()).unwrap();
    store
        .save_master_password(Some("fixture-password"))
        .unwrap();
    let txn = store.db.begin_write().unwrap();
    txn.open_table(crate::storage::META_TABLE)
        .unwrap()
        .insert(super::GENERATION, "9")
        .unwrap();
    {
        let mut table = txn.open_table(super::REPLICAS).unwrap();
        table.insert("good", legacy.as_slice()).unwrap();
        table.insert("broken", broken.as_slice()).unwrap();
    }
    txn.open_table(super::EDITS)
        .unwrap()
        .insert("field/consumed", 3)
        .unwrap();
    txn.commit().unwrap();
    assert!(!store.migrate_local_replica_storage().unwrap());
    let txn = store.db.begin_read().unwrap();
    {
        let table = txn.open_table(super::REPLICAS).unwrap();
        let good = table.get("good").unwrap().unwrap();
        assert!(good.value().starts_with(b"NYASYNC4"));
        let broken_stored = table.get("broken").unwrap().unwrap();
        assert_eq!(broken_stored.value(), broken.as_slice());
    }
    assert!(
        txn.open_table(super::EDITS)
            .unwrap()
            .get("field/consumed")
            .unwrap()
            .is_some()
    );
    assert!(
        txn.open_table(crate::storage::META_TABLE)
            .unwrap()
            .get(super::LOCAL_REPLICA_MIGRATION)
            .unwrap()
            .is_none()
    );
    drop(txn);
    // The untouched record stays readable through the ordinary load path.
    let (loaded, _) = store
        .load_v4_replica("good", "fixture-password")
        .unwrap()
        .unwrap();
    assert_eq!(serde_json::to_vec(&loaded).unwrap(), expected);
}
