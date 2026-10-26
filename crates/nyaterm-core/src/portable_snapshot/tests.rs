use super::{
    PortableSnapshotError, RawPortableSnapshot, preserve_device_local_sync_settings,
    project_sync_settings, validate_raw_snapshot,
};

#[test]
fn sync_settings_exclude_local_configuration_and_preserve_it_on_apply() {
    let sender = serde_json::json!({
        "cloud_sync": {"enabled": true, "webdav": {"password": "sender-encrypted"}},
        "keybindings": {"custom": "sender-binding"},
        "appearance": {"theme": "dark", "background_image_path": "C:/wallpaper.png"},
        "transfer": {
            "duplicate_strategy": "ask",
            "download_path": "C:/Downloads",
            "recording_path": "C:/Recordings"
        },
        "ui": {
            "language": "zh-CN",
            "open_tabs": ["sender-tab"],
            "left_width": 320,
            "right_width": 360,
            "active_left_panel": "notes",
            "active_right_panel": null,
            "activity_bar_layout": {
                "left_top": ["notes", "fileExplorer"],
                "right_top": ["gpuMonitor", "network"],
                "hidden_items": ["dockerManager"]
            },
            "panel_open_mode": "floating",
            "panel_multi_open": true
        },
        "security": {"master_password": "sender-encrypted-password"},
        "future_local_field": {"keep": true}
    });
    let receiver = serde_json::json!({
        "cloud_sync": {"enabled": false, "webdav": {"password": "receiver-encrypted"}},
        "keybindings": {"custom": "receiver-binding"},
        "appearance": {"theme": "light", "background_image_path": "D:/other.png"},
        "transfer": {
            "duplicate_strategy": "overwrite",
            "download_path": "D:/Downloads",
            "recording_path": "D:/Recordings"
        },
        "ui": {
            "language": "en-US",
            "open_tabs": ["receiver-tab"],
            "left_width": 210,
            "right_width": 240,
            "active_left_panel": "fileExplorer",
            "active_right_panel": "gpuMonitor",
            "activity_bar_layout": {
                "left_top": ["network"],
                "right_top": ["notes"],
                "hidden_items": []
            },
            "panel_open_mode": "docked",
            "panel_multi_open": false
        },
        "security": {"master_password": "receiver-encrypted-password"},
        "future_local_field": {"keep": false}
    });

    let mut projected = sender.clone();
    project_sync_settings(&mut projected);
    assert!(projected.get("cloud_sync").is_none());
    assert!(projected.get("keybindings").is_none());
    assert!(
        projected["appearance"]
            .get("background_image_path")
            .is_none()
    );
    for field in ["download_path", "recording_path"] {
        assert!(projected["transfer"].get(field).is_none());
    }
    for field in ["open_tabs", "left_width", "right_width"] {
        assert!(projected["ui"].get(field).is_none());
    }
    assert!(projected["security"].get("master_password").is_none());
    for field in [
        "active_left_panel",
        "active_right_panel",
        "activity_bar_layout",
        "panel_open_mode",
        "panel_multi_open",
    ] {
        assert_eq!(projected["ui"][field], sender["ui"][field], "{field}");
    }
    assert_eq!(
        projected["future_local_field"],
        sender["future_local_field"]
    );

    // The receiver adopts the sender's panel state and layout, not its local paths.
    preserve_device_local_sync_settings(&mut projected, &receiver);
    for field in [
        "active_left_panel",
        "active_right_panel",
        "activity_bar_layout",
        "panel_open_mode",
        "panel_multi_open",
    ] {
        assert_eq!(projected["ui"][field], sender["ui"][field], "{field}");
    }
    for field in ["open_tabs", "left_width", "right_width"] {
        assert_eq!(projected["ui"][field], receiver["ui"][field], "{field}");
    }
    for field in ["download_path", "recording_path"] {
        assert_eq!(
            projected["transfer"][field], receiver["transfer"][field],
            "{field}"
        );
    }
    assert_eq!(
        projected["appearance"]["theme"],
        sender["appearance"]["theme"]
    );
    assert_eq!(
        projected["appearance"]["background_image_path"],
        receiver["appearance"]["background_image_path"]
    );
    assert_eq!(projected["transfer"]["duplicate_strategy"], "ask");
    assert_eq!(projected["cloud_sync"], receiver["cloud_sync"]);
    assert_eq!(projected["keybindings"], receiver["keybindings"]);
    assert_eq!(
        projected["future_local_field"],
        sender["future_local_field"]
    );

    // The opposite direction also applies the incoming panel state.
    let mut reverse = receiver.clone();
    project_sync_settings(&mut reverse);
    preserve_device_local_sync_settings(&mut reverse, &sender);
    for field in [
        "active_left_panel",
        "active_right_panel",
        "activity_bar_layout",
        "panel_open_mode",
        "panel_multi_open",
    ] {
        assert_eq!(reverse["ui"][field], receiver["ui"][field], "{field}");
    }
    assert_eq!(reverse["ui"]["open_tabs"], sender["ui"]["open_tabs"]);
    assert_eq!(reverse["cloud_sync"], sender["cloud_sync"]);
    assert_eq!(reverse["keybindings"], sender["keybindings"]);
}

#[test]
fn sync_settings_preserve_closed_and_legacy_missing_panels() {
    let current = serde_json::json!({
        "ui": {"active_left_panel": "notes", "active_right_panel": "gpuMonitor"}
    });
    let mut closed = serde_json::json!({
        "ui": {"active_left_panel": "", "active_right_panel": null}
    });
    project_sync_settings(&mut closed);
    preserve_device_local_sync_settings(&mut closed, &current);
    assert_eq!(closed["ui"]["active_left_panel"], "");
    assert!(closed["ui"]["active_right_panel"].is_null());

    // Missing fields in a legacy sync payload are not invented here.
    // The storage merge fills absent keys from the receiver as before.
    let mut legacy = serde_json::json!({"ui": {"language": "en-US"}});
    project_sync_settings(&mut legacy);
    preserve_device_local_sync_settings(&mut legacy, &current);
    assert!(legacy["ui"].get("active_left_panel").is_none());
    assert!(legacy["ui"].get("active_right_panel").is_none());
}

#[test]
fn native_window_preferences_are_device_local_but_remain_in_backups() {
    let current = serde_json::json!({"appearance": {
        "window_material": "blurred", "window_background_opacity": 35,
        "background_opacity": 0.82, "theme": "dark"
    }});
    let mut synced = current.clone();
    project_sync_settings(&mut synced);
    assert!(synced["appearance"].get("window_material").is_none());
    assert!(
        synced["appearance"]
            .get("window_background_opacity")
            .is_none()
    );
    assert_eq!(synced["appearance"]["background_opacity"], 0.82);
    synced["appearance"]["window_material"] = serde_json::json!("transparent");
    preserve_device_local_sync_settings(&mut synced, &current);
    assert_eq!(synced, current);

    let mut backup = RawPortableSnapshot::backup("device", "test");
    backup
        .entities
        .insert("settings".into(), current.to_string());
    backup.recalculate_hash().expect("backup hash");
    validate_raw_snapshot(&backup).expect("backup validity");
    let restored: serde_json::Value = serde_json::from_str(&backup.entities["settings"]).unwrap();
    assert_eq!(restored, current);
}

#[test]
fn v3_hash_protects_notes_when_the_entity_is_present() {
    let mut snapshot = RawPortableSnapshot::backup("device-1", "test");
    snapshot.entities.insert(
        "notes".to_string(),
        r#"{"folders":[],"notes":[]}"#.to_string(),
    );
    snapshot.recalculate_hash().expect("hash with notes");
    validate_raw_snapshot(&snapshot).expect("valid snapshot");

    snapshot.entities.insert(
        "notes".to_string(),
        r#"{"folders":[],"notes":[{"id":"note-1","markdown":"changed"}]}"#.to_string(),
    );
    assert!(matches!(
        validate_raw_snapshot(&snapshot),
        Err(PortableSnapshotError::PayloadHashMismatch)
    ));
}

#[test]
fn compressed_snapshot_rejects_payload_over_fifty_mebibytes() {
    let oversized = vec![0; 50 * 1024 * 1024 + 1];
    let encoded = super::encode_compressed_snapshot_payload(&oversized).expect("encode zip");
    assert!(super::decode_compressed_snapshot_payload(&encoded).is_err());
}

#[test]
fn v3_hash_still_accepts_the_legacy_shape_without_notes() {
    let mut snapshot = RawPortableSnapshot::backup("device-1", "test");
    assert!(!snapshot.entities.contains_key("notes"));
    snapshot.recalculate_hash().expect("legacy hash");
    snapshot.meta.entities_hash = None;
    validate_raw_snapshot(&snapshot).expect("legacy v3 remains valid");
}

#[test]
fn optional_entity_map_hash_protects_unknown_entities() {
    let mut snapshot = RawPortableSnapshot::backup("device-1", "test");
    snapshot.entities.insert(
        "future_entity".to_string(),
        r#"{"schema":7,"payload":{"future":true}}"#.to_string(),
    );
    snapshot.recalculate_hash().expect("hash all entities");
    validate_raw_snapshot(&snapshot).expect("valid snapshot");

    snapshot.entities.insert(
        "future_entity".to_string(),
        r#"{"schema":7,"payload":{"future":false}}"#.to_string(),
    );
    assert!(matches!(
        validate_raw_snapshot(&snapshot),
        Err(PortableSnapshotError::EntitiesHashMismatch)
    ));
}

#[test]
fn legacy_metadata_without_entity_map_hash_deserializes_and_validates() {
    let mut snapshot = RawPortableSnapshot::backup("device-1", "test");
    snapshot.recalculate_hash().expect("hash snapshot");
    let mut value = serde_json::to_value(&snapshot).expect("serialize snapshot");
    value["meta"]
        .as_object_mut()
        .expect("metadata object")
        .remove("entities_hash");

    let decoded: RawPortableSnapshot = serde_json::from_value(value).expect("legacy metadata");
    assert_eq!(decoded.meta.entities_hash, None);
    validate_raw_snapshot(&decoded).expect("legacy snapshot remains valid");
}

#[test]
fn detects_payload_hash_mismatch() {
    let mut snapshot = RawPortableSnapshot::backup("device-1", "test");
    snapshot.recalculate_hash().expect("hash");
    snapshot
        .entities
        .insert("history".to_string(), "[{\"changed\":true}]".to_string());

    assert!(matches!(
        validate_raw_snapshot(&snapshot),
        Err(PortableSnapshotError::PayloadHashMismatch)
    ));
}

#[test]
fn settings_entity_preserves_activity_bar_hidden_items_and_panel_open_mode() {
    // The portable snapshot treats the settings document as an opaque,
    // schema-neutral blob. New Tauri-compatible keys must round-trip through a
    // snapshot unchanged and be protected by the payload hash.
    let mut snapshot = RawPortableSnapshot::backup("device-1", "test");
    let settings_json = serde_json::json!({
        "ui": {
            "activity_bar_layout": {
                "left_top": ["fileExplorer", "notes", "network"],
                "right_top": ["savedConnections", "gpuMonitor"],
                "hidden_items": ["gpuMonitor", "dockerManager"]
            },
            "active_left_panel": "notes",
            "active_right_panel": null,
            "panel_open_mode": "floating",
            "panel_multi_open": true,
            "show_notes_panel": true
        },
        "appearance": { "panel_multi_open": true }
    })
    .to_string();
    snapshot
        .entities
        .insert("settings".to_string(), settings_json.clone());
    snapshot.recalculate_hash().expect("hash with settings");
    validate_raw_snapshot(&snapshot).expect("valid snapshot");

    // Serialize and deserialize to confirm the opaque blob survives a full
    // round trip with the new keys intact.
    let value = serde_json::to_value(&snapshot).expect("serialize snapshot");
    let decoded: RawPortableSnapshot = serde_json::from_value(value).expect("decode snapshot");
    let decoded_settings = decoded
        .entities
        .get("settings")
        .expect("settings entity present");
    let parsed: serde_json::Value = serde_json::from_str(decoded_settings).expect("settings json");
    assert_eq!(
        parsed["ui"]["activity_bar_layout"]["hidden_items"],
        serde_json::json!(["gpuMonitor", "dockerManager"])
    );
    assert_eq!(
        parsed["ui"]["activity_bar_layout"]["left_top"],
        serde_json::json!(["fileExplorer", "notes", "network"])
    );
    assert_eq!(
        parsed["ui"]["activity_bar_layout"]["right_top"],
        serde_json::json!(["savedConnections", "gpuMonitor"])
    );
    assert_eq!(parsed["ui"]["active_left_panel"], "notes");
    assert!(parsed["ui"]["active_right_panel"].is_null());
    assert_eq!(parsed["ui"]["panel_open_mode"], "floating");
    assert_eq!(parsed["ui"]["show_notes_panel"], true);
    assert_eq!(parsed["appearance"]["panel_multi_open"], true);
    validate_raw_snapshot(&decoded).expect("decoded snapshot remains valid");

    // Tampering with the preserved settings blob must be caught by the hash.
    let mut tampered = decoded;
    tampered.entities.insert(
        "settings".to_string(),
        settings_json.replace("floating", "docked"),
    );
    assert!(matches!(
        validate_raw_snapshot(&tampered),
        Err(PortableSnapshotError::PayloadHashMismatch)
    ));
}

#[test]
fn sessions_entity_preserves_connection_asset_metadata() {
    // The sessions document is an opaque, schema-neutral blob in the snapshot.
    // Connection asset metadata (Tauri-compatible) must round-trip unchanged and
    // stay protected by the payload hash.
    let mut snapshot = RawPortableSnapshot::backup("device-1", "test");
    let sessions_json = serde_json::json!({
        "groups": [],
        "connections": [{
            "id": "conn-asset",
            "name": "Asset Host",
            "type": "ssh",
            "host": "10.0.0.2",
            "port": 22,
            "username": "root",
            "asset": {
                "device_type": "physical",
                "hostname": "gpu-node-01",
                "cpu_cores": 192,
                "accelerators": [
                    { "type": "gpu", "vendor": "NVIDIA", "model": "H100", "count": 8 }
                ],
                "tags": ["training"]
            }
        }]
    })
    .to_string();
    snapshot
        .entities
        .insert("sessions".to_string(), sessions_json.clone());
    snapshot.recalculate_hash().expect("hash with sessions");
    validate_raw_snapshot(&snapshot).expect("valid snapshot");

    let value = serde_json::to_value(&snapshot).expect("serialize snapshot");
    let decoded: RawPortableSnapshot = serde_json::from_value(value).expect("decode snapshot");
    let parsed: serde_json::Value =
        serde_json::from_str(decoded.entities.get("sessions").expect("sessions entity"))
            .expect("sessions json");
    let asset = &parsed["connections"][0]["asset"];
    assert_eq!(asset["device_type"], "physical");
    assert_eq!(asset["hostname"], "gpu-node-01");
    assert_eq!(asset["cpu_cores"], 192);
    assert_eq!(asset["accelerators"][0]["type"], "gpu");
    assert_eq!(asset["tags"], serde_json::json!(["training"]));
    validate_raw_snapshot(&decoded).expect("decoded snapshot remains valid");

    // Tampering with the preserved asset blob must be caught by the hash.
    let mut tampered = decoded;
    tampered.entities.insert(
        "sessions".to_string(),
        sessions_json.replace("gpu-node-01", "attacker-host"),
    );
    assert!(matches!(
        validate_raw_snapshot(&tampered),
        Err(PortableSnapshotError::PayloadHashMismatch)
    ));
}

#[test]
fn settings_entity_preserves_start_workspace_and_asset_sort_keys() {
    // The new UI settings keys must round-trip through a snapshot unchanged and
    // stay protected by the payload hash.
    let mut snapshot = RawPortableSnapshot::backup("device-1", "test");
    let settings_json = serde_json::json!({
        "ui": {
            "start_workspace_mode": "assets",
            "asset_sort_key": "hostname",
            "asset_sort_direction": "desc"
        }
    })
    .to_string();
    snapshot
        .entities
        .insert("settings".to_string(), settings_json.clone());
    snapshot.recalculate_hash().expect("hash with settings");
    validate_raw_snapshot(&snapshot).expect("valid snapshot");

    let value = serde_json::to_value(&snapshot).expect("serialize snapshot");
    let decoded: RawPortableSnapshot = serde_json::from_value(value).expect("decode snapshot");
    let parsed: serde_json::Value =
        serde_json::from_str(decoded.entities.get("settings").expect("settings entity"))
            .expect("settings json");
    assert_eq!(parsed["ui"]["start_workspace_mode"], "assets");
    assert_eq!(parsed["ui"]["asset_sort_key"], "hostname");
    assert_eq!(parsed["ui"]["asset_sort_direction"], "desc");
    validate_raw_snapshot(&decoded).expect("decoded snapshot remains valid");
}
