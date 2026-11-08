use nyaterm_core::{Group, SavedConnection, test_support::TestTempDir};
use nyaterm_store::ConnectionStore;
use serde_json::json;

use super::save_editor_connection;
use crate::features::connections::connection_runtime::helpers::{
    build_saved_connection_from_editor, connection_editor_from_saved,
};
use crate::models::{ConnectionEditorPasswordSource, ConnectionPasswordEdit};

fn connection(id: &str, group_id: Option<&str>, sort_order: i32) -> SavedConnection {
    // The older saved format omits timestamps and optional editor metadata.
    serde_json::from_value(json!({
        "id": id, "name": id, "type": "ssh", "host": "example.test",
        "username": "user", "group_id": group_id, "sort_order": sort_order
    }))
    .unwrap()
}

fn edited(connection: SavedConnection) -> SavedConnection {
    let mut editor = connection_editor_from_saved(connection, false);
    editor.description = "Edited configuration".to_string();
    build_saved_connection_from_editor(&editor).unwrap()
}

#[test]
fn telnet_network_settings_survive_edit_save_reload_and_raw_tcp_changes() {
    let root = TestTempDir::new("nyaterm-telnet-network");
    let store = ConnectionStore::open(&root).unwrap();
    let original: SavedConnection = serde_json::from_value(json!({
        "id":"telnet", "name":"Telnet", "type":"telnet", "host":"device.internal",
        "network":{"proxy_id":"proxy", "proxy_jump_id":"jump", "host_key_alias":"retained-alias"}
    }))
    .unwrap();
    store.save_connection(&original).unwrap();
    for raw in [false, true, false] {
        let stored = store.get_connection("telnet").unwrap().unwrap();
        let mut editor = connection_editor_from_saved(stored, false);
        editor.raw_tcp_cli = raw;
        editor.description = "Edited".into();
        let built = build_saved_connection_from_editor(&editor).unwrap();
        assert_eq!(built.network, original.network);
        save_editor_connection(&store, built, None, Some(ConnectionPasswordEdit::Keep)).unwrap();
        let reloaded = store.get_connection("telnet").unwrap().unwrap();
        assert_eq!(reloaded.network, original.network);
        assert!(
            matches!(reloaded.config, nyaterm_core::ConnectionType::Telnet { raw_tcp_cli, .. } if raw_tcp_cli == raw)
        );
    }
    let mut editor =
        connection_editor_from_saved(store.get_connection("telnet").unwrap().unwrap(), false);
    editor.proxy_id = None;
    editor.proxy_jump_id = None;
    editor.host_key_alias = None;
    assert!(
        build_saved_connection_from_editor(&editor)
            .unwrap()
            .network
            .is_none()
    );
}

#[test]
fn saving_mstsc_preserves_stored_authentication_and_unused_options() {
    let root = TestTempDir::new("nyaterm-mstsc-preserve");
    let store = ConnectionStore::open(&root).unwrap();
    for mode in ["password", "future-auth"] {
        let original: SavedConnection = serde_json::from_value(json!({
            "id":"rdp", "name":"RDP", "type":"rdp", "host":"example.test", "backend":"windows",
            "auth":{"mode":mode,"password":"sample","has_password":false,"password_id":"missing"},
            "security":{"certificate_policy":"future-policy"},
            "display":{"width":900,"height":700},
            "clipboard":{"mode":"future-clipboard"}, "reconnect":{"enabled":false,"max_attempts":21}
        }))
        .unwrap();
        store.save_connection(&original).unwrap();
        let previous = store.get_connection("rdp").unwrap().unwrap();
        let mut editor = connection_editor_from_saved(previous.clone(), false);
        editor.rdp_backend = "mstsc".into();
        editor.username.clear();
        editor.name = "External RDP".into();
        let built = build_saved_connection_from_editor(&editor).unwrap();
        save_editor_connection(
            &store,
            built.clone(),
            None,
            Some(ConnectionPasswordEdit::Keep),
        )
        .unwrap();
        let saved = store.get_connection("rdp").unwrap().unwrap();
        assert!(
            saved.auth == previous.auth,
            "hidden authentication must be preserved"
        );
        assert_eq!(saved.config, built.config);
        assert_eq!(saved.name, "External RDP");
    }
}

#[test]
fn editing_middle_connection_preserves_custom_order_after_reloading() {
    for group_id in [None, Some("group")] {
        let root = TestTempDir::new("nyaterm-editor-order");
        let store = ConnectionStore::open(&root).unwrap();
        for (id, order) in [("c", 2), ("b", 8), ("a", 15)] {
            store
                .save_connection(&connection(id, group_id, order))
                .unwrap();
        }
        let built = edited(store.get_connection("b").unwrap().unwrap());
        let sessions =
            save_editor_connection(&store, built, None, Some(ConnectionPasswordEdit::Keep))
                .unwrap();
        assert_eq!(
            sessions
                .connections
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            ["c", "b", "a"]
        );
        drop(store);
        let reopened = ConnectionStore::open(&root).unwrap();
        let connections = reopened.load_sessions().unwrap().connections;
        assert_eq!(
            connections
                .iter()
                .map(|entry| (entry.id.as_str(), entry.sort_order))
                .collect::<Vec<_>>(),
            [("c", 2), ("b", 8), ("a", 15)]
        );
        assert_eq!(
            connections[1].description.as_deref(),
            Some("Edited configuration")
        );
    }
}

#[test]
fn editor_save_preserves_latest_usage_order_and_unexposed_metadata() {
    let root = TestTempDir::new("nyaterm-editor-metadata");
    let store = ConnectionStore::open(&root).unwrap();
    store.save_connection(&connection("b", None, 8)).unwrap();
    let built = edited(store.get_connection("b").unwrap().unwrap());

    // Usage, ordering and monitoring can change while the editor remains open.
    let mut latest = store.get_connection("b").unwrap().unwrap();
    latest.sort_order = 12;
    latest.asset = Some(serde_json::from_value(json!({"os_name": "Linux"})).unwrap());
    let mut raw = serde_json::to_value(latest).unwrap();
    raw["future_metadata"] = json!({"retain": true});
    store
        .save_connection(&serde_json::from_value(raw).unwrap())
        .unwrap();
    store.mark_connection_used("b").unwrap();
    let previous = store.get_connection("b").unwrap().unwrap();
    assert!(previous.last_used_at_ms.is_some());

    save_editor_connection(&store, built, None, Some(ConnectionPasswordEdit::Keep)).unwrap();
    let saved = store.get_connection("b").unwrap().unwrap();
    assert_eq!(saved.sort_order, previous.sort_order);
    assert_eq!(saved.last_used_at_ms, previous.last_used_at_ms);
    assert_eq!(saved.created_at_ms, previous.created_at_ms);
    assert_eq!(saved.asset, previous.asset);
    assert_eq!(saved.extensions, previous.extensions);
    assert_eq!(
        serde_json::to_value(saved).unwrap()["future_metadata"],
        json!({"retain": true})
    );
}

#[test]
fn changing_group_appends_after_destination_siblings_and_preserves_usage() {
    for (source, target) in [
        (Some("source"), Some("target")),
        (None, Some("target")),
        (Some("source"), None),
    ] {
        let root = TestTempDir::new("nyaterm-editor-move");
        let store = ConnectionStore::open(&root).unwrap();
        for (id, group, order) in [
            ("moved", source, 88),
            ("source-peer", source, 99),
            ("first", target, 4),
            ("last", target, 12),
        ] {
            store
                .save_connection(&connection(id, group, order))
                .unwrap();
        }
        store.mark_connection_used("moved").unwrap();
        let previous = store.get_connection("moved").unwrap().unwrap();
        let mut editor = connection_editor_from_saved(previous.clone(), false);
        editor.group_id = target.map(str::to_owned);
        let built = build_saved_connection_from_editor(&editor).unwrap();
        let loaded =
            save_editor_connection(&store, built, None, Some(ConnectionPasswordEdit::Keep))
                .unwrap();
        let destination = loaded
            .connections
            .iter()
            .filter(|entry| entry.group_id.as_deref() == target)
            .collect::<Vec<_>>();
        assert_eq!(
            destination
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "last", "moved"]
        );
        assert_eq!(destination[2].sort_order, 13);
        assert_eq!(destination[2].last_used_at_ms, previous.last_used_at_ms);
        assert_eq!(
            store
                .get_connection("source-peer")
                .unwrap()
                .unwrap()
                .sort_order,
            99
        );
    }
}

#[test]
fn new_connection_appends_to_its_group_without_inheriting_usage() {
    for group_id in [None, Some("target")] {
        let root = TestTempDir::new("nyaterm-editor-create");
        let store = ConnectionStore::open(&root).unwrap();
        store
            .save_connection(&connection("existing", group_id, 8))
            .unwrap();
        store
            .save_connection(&connection("unrelated", Some("other"), 100))
            .unwrap();
        let mut editor = connection_editor_from_saved(connection("new", group_id, 0), false);
        editor.id = None;
        let built = build_saved_connection_from_editor(&editor).unwrap();
        let new_id = built.id.clone();
        save_editor_connection(&store, built, None, Some(ConnectionPasswordEdit::Keep)).unwrap();
        let saved = store.get_connection(&new_id).unwrap().unwrap();
        assert_eq!(saved.sort_order, 9);
        assert_eq!(saved.last_used_at_ms, None);
    }
}

#[test]
fn editor_save_into_new_group_starts_at_zero_and_saves_group() {
    for is_new in [false, true] {
        let root = TestTempDir::new("nyaterm-editor-new-group");
        let store = ConnectionStore::open(&root).unwrap();
        let original = connection("connection", None, 8);
        if !is_new {
            store.save_connection(&original).unwrap();
        }
        let group: Group = serde_json::from_value(
            json!({"id": "new-group", "name": "New group", "sort_order": 0}),
        )
        .unwrap();
        let mut editor = connection_editor_from_saved(original, false);
        editor.group_id = Some(group.id.clone());
        if is_new {
            editor.id = None;
        }
        let built = build_saved_connection_from_editor(&editor).unwrap();
        let loaded = save_editor_connection(
            &store,
            built,
            Some(&group),
            Some(ConnectionPasswordEdit::Keep),
        )
        .unwrap();
        assert_eq!(loaded.groups.len(), 1);
        assert_eq!(loaded.groups[0].id, group.id);
        assert_eq!(loaded.connections.len(), 1);
        assert_eq!(loaded.connections[0].group_id.as_deref(), Some("new-group"));
        assert_eq!(loaded.connections[0].sort_order, 0);
    }
}

#[test]
fn password_save_preserves_latest_secret_after_lock_and_round_trips_replace_and_clear() {
    for kind in ["ssh", "telnet", "rdp", "vnc"] {
        let root = TestTempDir::new("nyaterm-single-password-save");
        let store = ConnectionStore::open(&root).unwrap();
        let original: SavedConnection = serde_json::from_value(json!({
            "id": "password", "name": "Password", "type": kind,
            "host": "example.test", "username": "user",
            "auth": {"mode": "password", "password_source": "connection", "password": "old"}
        }))
        .unwrap();
        store.save_connection(&original).unwrap();
        let mut editor =
            connection_editor_from_saved(store.get_connection("password").unwrap().unwrap(), false);
        // A lock releases all plaintext; Keep must fetch the latest database value.
        editor.password = Default::default();
        editor.existing_password = None;
        let mut latest = store.get_connection("password").unwrap().unwrap();
        latest.auth.as_mut().unwrap().password = Some("latest".into());
        store.save_connection(&latest).unwrap();
        let built = build_saved_connection_from_editor(&editor).unwrap();
        save_editor_connection(&store, built, None, Some(ConnectionPasswordEdit::Keep)).unwrap();
        assert_eq!(
            store
                .get_connection("password")
                .unwrap()
                .unwrap()
                .auth
                .unwrap()
                .password
                .as_deref(),
            Some("latest")
        );

        editor.password = " new ".into();
        editor.password_edit = ConnectionPasswordEdit::Replace;
        let built = build_saved_connection_from_editor(&editor).unwrap();
        save_editor_connection(&store, built, None, Some(editor.password_edit)).unwrap();
        assert_eq!(
            store
                .get_connection("password")
                .unwrap()
                .unwrap()
                .auth
                .unwrap()
                .password
                .as_deref(),
            Some(" new ")
        );
        editor.password = Default::default();
        editor.password_edit = ConnectionPasswordEdit::Remove;
        let built = build_saved_connection_from_editor(&editor).unwrap();
        save_editor_connection(&store, built, None, Some(editor.password_edit)).unwrap();
        drop(store);
        let reopened = ConnectionStore::open(&root).unwrap();
        assert!(
            reopened
                .get_connection("password")
                .unwrap()
                .unwrap()
                .auth
                .unwrap()
                .password
                .is_none()
        );
    }
}

#[test]
fn keeping_unreadable_ciphertext_survives_saving_and_reopening() {
    for kind in ["ssh", "telnet", "rdp", "vnc"] {
        let root = TestTempDir::new("nyaterm-editor-unreadable-password");
        let store = ConnectionStore::open(&root).unwrap();
        let original: SavedConnection = serde_json::from_value(json!({
            "id":"locked", "name":"Locked", "type":kind, "host":"example.test", "username":"user",
            "auth":{"mode":"password", "password":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=", "has_password":true}
        }))
        .unwrap();
        store.save_connection(&original).unwrap();
        let mut editor =
            connection_editor_from_saved(store.get_connection("locked").unwrap().unwrap(), false);
        assert!(editor.password.is_empty());
        editor.name = "Renamed".into();
        editor.existing_password = None;
        let built = build_saved_connection_from_editor(&editor).unwrap();
        save_editor_connection(&store, built, None, Some(ConnectionPasswordEdit::Keep)).unwrap();
        drop(store);
        let reopened = ConnectionStore::open(&root).unwrap();
        let saved = reopened.get_connection("locked").unwrap().unwrap();
        assert_eq!(saved.name, "Renamed");
        let auth = saved.auth.unwrap();
        assert!(auth.has_password);
        assert_eq!(
            auth.password.as_deref(),
            Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
        );
    }
}

#[test]
fn ssh_authentication_tabs_preserve_password_sources_keys_and_agent_settings_after_restart() {
    for auth in [
        json!({"mode":"password", "password":" fixture password "}),
        json!({"mode":"password", "password":"   ", "account_id":"account", "password_source":"connection", "username_source":"connection"}),
        json!({"mode":"password", "account_id":"account", "password_source":"account"}),
        json!({"mode":"password", "password_id":"account"}),
    ] {
        let root = TestTempDir::new("nyaterm-ssh-auth-retention");
        let mut store = ConnectionStore::open(&root).unwrap();
        store.save_password(serde_json::from_value(json!({
            "id":"account", "name":"Fixture account", "username":"account-user", "password":"fixture-account-password"
        })).unwrap()).unwrap();
        let original: SavedConnection = serde_json::from_value(json!({
            "id":"ssh", "name":"SSH", "type":"ssh", "host":"example.test", "username":"user", "auth":auth
        })).unwrap();
        store.save_connection(&original).unwrap();
        let expected_password = original.auth.as_ref().unwrap().password.clone();
        let mut editor =
            connection_editor_from_saved(store.get_connection("ssh").unwrap().unwrap(), false);
        let expected_source = editor.password_source;
        let expected_account = editor.account_id.clone();
        let expected_username_source = editor.username_source;
        let endpoint = nyaterm_core::SshAgentEndpoint::Environment {
            variable: "FIXTURE_SSH_AGENT".into(),
        };
        editor.key_id = Some("fixture-key".into());
        editor.agent_endpoint = endpoint.clone();
        for mode in ["key", "agent", "none", "password", "key", "password"] {
            editor.auth_mode = mode.into();
            let built = build_saved_connection_from_editor(&editor).unwrap();
            let password_edit = (editor.password_source == ConnectionEditorPasswordSource::Direct)
                .then_some(editor.password_edit);
            save_editor_connection(&store, built, None, password_edit).unwrap();
            drop(store);
            store = ConnectionStore::open(&root).unwrap();
            let saved = store.get_connection("ssh").unwrap().unwrap();
            assert_eq!(saved.auth.as_ref().unwrap().mode, mode);
            assert_eq!(saved.auth.as_ref().unwrap().password, expected_password);
            editor = connection_editor_from_saved(saved, false);
            assert_eq!(editor.password_source, expected_source);
            assert_eq!(editor.account_id, expected_account);
            assert_eq!(editor.username_source, expected_username_source);
            assert_eq!(editor.key_id.as_deref(), Some("fixture-key"));
            assert_eq!(editor.agent_endpoint, endpoint);
            if expected_source == ConnectionEditorPasswordSource::Direct {
                assert_eq!(Some(&editor.password), expected_password.as_ref());
            }
        }
        editor.key_id = None;
        let built = build_saved_connection_from_editor(&editor).unwrap();
        save_editor_connection(
            &store,
            built,
            None,
            (expected_source == ConnectionEditorPasswordSource::Direct)
                .then_some(ConnectionPasswordEdit::Keep),
        )
        .unwrap();
        assert!(
            store
                .get_connection("ssh")
                .unwrap()
                .unwrap()
                .auth
                .unwrap()
                .key_id
                .is_none()
        );
    }
}

#[test]
fn inactive_ssh_password_keeps_latest_or_unreadable_secret_and_honors_replace_and_remove() {
    for mode in ["key", "agent", "none"] {
        for unreadable in [false, true] {
            let root = TestTempDir::new("nyaterm-inactive-ssh-password");
            let mut store = ConnectionStore::open(&root).unwrap();
            let mut original: SavedConnection = serde_json::from_value(json!({
                "id":"ssh", "name":"SSH", "type":"ssh", "host":"example.test", "username":"user",
                "auth":{"mode":"password", "password_source":"connection", "password":"fixture-old-password"}
            })).unwrap();
            store.save_connection(&original).unwrap();
            let mut editor =
                connection_editor_from_saved(store.get_connection("ssh").unwrap().unwrap(), false);
            editor.auth_mode = mode.into();
            editor.key_id = Some("fixture-key".into());
            // Model locking after opening the editor and a newer database credential.
            editor.password = Default::default();
            editor.existing_password = None;
            let latest = if unreadable {
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
            } else {
                " fixture-latest-password "
            };
            original.auth.as_mut().unwrap().password = Some(latest.into());
            original.auth.as_mut().unwrap().has_password = unreadable;
            store.save_connection(&original).unwrap();
            let built = build_saved_connection_from_editor(&editor).unwrap();
            save_editor_connection(&store, built, None, Some(ConnectionPasswordEdit::Keep))
                .unwrap();
            drop(store);
            store = ConnectionStore::open(&root).unwrap();
            let saved = store.get_connection("ssh").unwrap().unwrap();
            let auth = saved.auth.unwrap();
            assert_eq!(auth.mode, mode);
            assert_eq!(auth.password.as_deref(), Some(latest));
            assert_eq!(auth.has_password, unreadable);

            editor.password = " fixture-replacement ".into();
            editor.password_edit = ConnectionPasswordEdit::Replace;
            let built = build_saved_connection_from_editor(&editor).unwrap();
            save_editor_connection(&store, built, None, Some(editor.password_edit)).unwrap();
            assert_eq!(
                store
                    .get_connection("ssh")
                    .unwrap()
                    .unwrap()
                    .auth
                    .unwrap()
                    .password
                    .as_deref(),
                Some(" fixture-replacement ")
            );

            editor.password = Default::default();
            editor.password_edit = ConnectionPasswordEdit::Remove;
            let built = build_saved_connection_from_editor(&editor).unwrap();
            save_editor_connection(&store, built, None, Some(editor.password_edit)).unwrap();
            drop(store);
            store = ConnectionStore::open(&root).unwrap();
            let auth = store.get_connection("ssh").unwrap().unwrap().auth.unwrap();
            assert_eq!(auth.mode, mode);
            assert!(auth.password.is_none());
            assert!(!auth.has_password);
            assert_eq!(auth.key_id.as_deref(), Some("fixture-key"));
        }
    }
}

#[test]
fn ssh_compatibility_mode_survives_editor_save_reload_and_profile_changes() {
    let root = TestTempDir::new("nyaterm-ssh-compatibility-editor");
    let store = ConnectionStore::open(&root).unwrap();
    let original = connection("ssh-compatibility", None, 0);
    assert!(!connection_editor_from_saved(original.clone(), false).ssh_compatibility_mode);
    store.save_connection(&original).unwrap();
    for profile in [
        nyaterm_core::SshProfile::Standard,
        nyaterm_core::SshProfile::NetworkDevice,
    ] {
        for enabled in [true, false] {
            let stored = store.get_connection(&original.id).unwrap().unwrap();
            let mut editor = connection_editor_from_saved(stored, false);
            editor.ssh_profile = profile;
            editor.ssh_compatibility_mode = enabled;
            let built = build_saved_connection_from_editor(&editor).unwrap();
            assert_eq!(built.ssh_compatibility_mode, enabled);
            save_editor_connection(&store, built, None, Some(ConnectionPasswordEdit::Keep))
                .unwrap();
            let reloaded = store.get_connection(&original.id).unwrap().unwrap();
            assert_eq!(reloaded.ssh_profile, profile);
            assert_eq!(reloaded.ssh_compatibility_mode, enabled);
            assert_eq!(
                connection_editor_from_saved(reloaded, false).ssh_compatibility_mode,
                enabled
            );
        }
    }
}
