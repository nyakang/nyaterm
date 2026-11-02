use nyaterm_core::{Group, SavedConnection, test_support::TestTempDir};
use nyaterm_store::ConnectionStore;
use serde_json::json;

use super::save_editor_connection;
use crate::features::connections::connection_runtime::helpers::{
    build_saved_connection_from_editor, connection_editor_from_saved,
};
use crate::models::ConnectionPasswordEdit;

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
