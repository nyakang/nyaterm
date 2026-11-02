use crate::sync_schema::{
    SyncFieldId, SyncResourceId, SyncScope, apply_record, project_entities, project_record,
};
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn ai_scope_excludes_keys_headers_permissions_paths_and_discovery() {
    let mut ai = serde_json::to_value(crate::ai::AiSettings::default()).unwrap();
    ai["provider_credentials"] = json!([{"id":"custom","name":"Provider","provider_kind":"openai","base_url":"https://example.com/v1/","api_key":"private-key-fixture","extra_headers":[{"name":"Authorization","value":"private-header-fixture"}],"enabled":true,"future_secret":"future-fixture"}]);
    ai["models"] = json!([{"id":"model","name":"a-model","credential_id":"custom","last_seen_at":"private-runtime-fixture","supported_reasoning_efforts":["high"]}]);
    ai["codex"] = json!({"binary_path":"private-path-fixture"});
    ai["agent_command_execution_mode"] = json!("auto");
    let entities = BTreeMap::from([("settings".into(), json!({"ai":ai}).to_string())]);
    let scope = SyncScope {
        ai: true,
        ..Default::default()
    };
    let projection = project_entities(&entities, &scope).unwrap();
    let decoded = projection
        .values
        .values()
        .map(|value| value.decode().unwrap().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    for excluded in [
        "private-key-fixture",
        "private-header-fixture",
        "future-fixture",
        "private-runtime-fixture",
        "private-path-fixture",
        "agent_command_execution_mode",
        "supported_reasoning_efforts",
    ] {
        assert!(!decoded.contains(excluded));
    }
    assert!(decoded.contains("example.com"));
    assert!(
        !project_entities(&entities, &SyncScope::default())
            .unwrap()
            .complete
            .contains("ai_provider")
    );
}

#[test]
fn unsupported_ai_endpoint_and_action_cannot_establish_deletion() {
    let entities = BTreeMap::from([("settings".into(), json!({"ai":{
        "provider_credentials":[{"id":"p","name":"P","base_url":"https://example.com/?token=private"}],
        "terminal_ai_actions":[{"name":"unknown-identity"}], "file_ai_actions":[]
    }}).to_string())]);
    let projection = project_entities(
        &entities,
        &SyncScope {
            ai: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!projection.complete.contains("ai_provider"));
    assert!(!projection.complete.contains("ai_action"));
    assert!(
        !projection
            .values
            .keys()
            .any(|key| SyncFieldId::from_key(key).unwrap().resource.kind == "ai_provider")
    );
}

#[test]
fn runtime_activity_and_local_bindings_do_not_change_cloud_projection() {
    let original = json!({"id":"a","type":"ssh","name":"Server","host":"example","port":22,"username":"u","updated_at_ms":1,"last_used_at_ms":1,"asset":{"cpu_model":"old","tags":["prod"]},"auth_agent_endpoint":"old"});
    let mut used = original.clone();
    used["updated_at_ms"] = json!(9);
    used["last_used_at_ms"] = json!(8);
    used["asset"]["cpu_model"] = json!("new");
    used["asset"]["memory_bytes"] = json!(123);
    used["auth_agent_endpoint"] = json!("new");
    assert_eq!(
        project_record("connection", &original).unwrap(),
        project_record("connection", &used).unwrap()
    );
}

#[test]
fn local_shell_and_serial_port_are_not_portable() {
    for kind in ["local", "serial"] {
        let mut a = json!({"type":kind,"name":"a","shell_path":"pwsh","working_dir":"C:/","port_name":"COM1"});
        let projected = project_record("connection", &a).unwrap();
        a["shell_path"] = json!("/bin/zsh");
        a["working_dir"] = json!("/tmp");
        a["port_name"] = json!("/dev/ttyS0");
        assert_eq!(projected, project_record("connection", &a).unwrap());
    }
}

#[test]
fn settings_allowlist_excludes_unknown_security_and_device_values() {
    let entities = BTreeMap::from([("settings".into(), json!({"appearance":{"theme":"dark","font_size":14,"window_material":"mica","future_secret":"secret"},"cloud_sync":{"access_token":"secret"},"security":{"master_password":"secret"}}).to_string())]);
    let scope = SyncScope {
        preferences: true,
        ..Default::default()
    };
    let projection = project_entities(&entities, &scope).unwrap();
    assert_eq!(projection.values.len(), 4);
    assert!(!format!("{projection:?}").contains("secret"));
    assert!(
        project_entities(&entities, &SyncScope::default())
            .unwrap()
            .values
            .is_empty()
    );
}

#[test]
fn applying_portable_fields_preserves_device_and_unknown_fields() {
    let original = json!({"type":"ssh","name":"old","host":"a","auth_agent_endpoint":"local","future":{"x":1},"asset":{"cpu_model":"local","tags":["old"]}});
    let mut projected = project_record("connection", &original).unwrap();
    projected["name"] = json!("new");
    projected["asset"]["tags"] = json!(["new"]);
    let applied = apply_record("connection", &original, &projected).unwrap();
    assert_eq!(applied["auth_agent_endpoint"], "local");
    assert_eq!(applied["future"], original["future"]);
    assert_eq!(applied["asset"]["cpu_model"], "local");
    assert_eq!(applied["name"], "new");
}

#[test]
fn field_identity_cannot_alias_separator_containing_names() {
    let a = SyncFieldId::new(SyncResourceId::new("a/b", "c"), "d");
    let b = SyncFieldId::new(SyncResourceId::new("a", "b/c"), "d");
    assert_ne!(a.key(), b.key());
    assert_eq!(SyncFieldId::from_key(&a.key()), Some(a));
}

#[test]
fn deleted_fields_are_removed_and_unclassified_remote_fields_cannot_override_local_data() {
    let original = json!({"type":"ssh","host":"server","name":"old","description":"remove","tags":["remove"],"asset":{"tags":["remove"],"cpu_model":"keep"},"future":"keep","shell_path":"local"});
    let projected = json!({"name":"new","endpoint":{"type":"ssh","host":"server","shell_path":"remote"},"asset":{"notes":"new","cpu_model":"remote"},"future":"remote","shell_path":"remote"});
    let result = apply_record("connection", &original, &projected).unwrap();
    assert!(result.get("description").is_none());
    assert!(result.get("tags").is_none());
    assert!(result["asset"].get("tags").is_none());
    assert_eq!(result["asset"]["cpu_model"], "keep");
    assert_eq!(result["future"], "keep");
    assert_eq!(result["shell_path"], "local");
}

#[test]
fn unknown_theme_extensions_stay_local_and_survive_known_color_updates() {
    let mut original = serde_json::to_value(crate::themes::builtins()[0].copy_as_custom()).unwrap();
    original["future_secret"] = json!("local");
    original["colors"]["future_secret"] = json!("local");
    original["colors"]["terminal"]["future_secret"] = json!("local");
    let mut projected = project_record("theme", &original).unwrap();
    assert!(projected.get("future_secret").is_none());
    assert!(projected["colors"].get("future_secret").is_none());
    assert!(
        projected["colors"]["terminal"]
            .get("future_secret")
            .is_none()
    );
    projected["colors"]["primary"] = json!("#ff0000");
    let result = apply_record("theme", &original, &projected).unwrap();
    assert_eq!(result["future_secret"], "local");
    assert_eq!(result["colors"]["future_secret"], "local");
    assert_eq!(result["colors"]["terminal"]["future_secret"], "local");
    assert_eq!(result["colors"]["primary"], "#ff0000");
}

#[test]
fn network_identity_and_associated_configuration_are_one_atomic_value() {
    for (kind, original) in [
        (
            "proxy",
            json!({"protocol":"socks5","host":"old","port":1080,"username":"user","password":"local","command":"local command","password_id":"vault","name":"Proxy"}),
        ),
        (
            "tunnel",
            json!({"tunnel_type":"local","connection_id":"ssh","listen_port":8080,"target_host":"old","target_port":80,"bind_localhost":true,"is_open":true,"name":"Tunnel"}),
        ),
    ] {
        let mut projected = project_record(kind, &original).unwrap();
        assert!(projected.get("host").is_none());
        assert!(projected.get("target_host").is_none());
        projected["endpoint"][if kind == "proxy" {
            "host"
        } else {
            "target_host"
        }] = json!("new");
        projected["endpoint"]["password"] = json!("remote");
        let result = apply_record(kind, &original, &projected).unwrap();
        if kind == "proxy" {
            assert_eq!(result["host"], "new");
            assert_eq!(result["password"], "local");
            assert_eq!(result["command"], "local command");
        } else {
            assert_eq!(result["target_host"], "new");
            assert_eq!(result["is_open"], true);
            assert!(result.get("password").is_none());
        }
    }
}

#[test]
fn keybindings_are_opt_in_and_unsupported_entries_cannot_prove_deletion() {
    let mut entities = BTreeMap::from([(
        "settings".into(),
        json!({"keybindings":{"terminal.copy":"ctrl+shift+c"}}).to_string(),
    )]);
    assert!(
        project_entities(&entities, &SyncScope::default())
            .unwrap()
            .values
            .is_empty()
    );
    let scope = SyncScope {
        keybindings: true,
        ..Default::default()
    };
    let projected = project_entities(&entities, &scope).unwrap();
    assert!(projected.complete.contains("keybinding"));
    assert_eq!(projected.values.len(), 2);
    entities.insert(
        "settings".into(),
        json!({"keybindings":{"terminal.copy":"ctrl+shift+c","future":{"binding":"unsupported"}}})
            .to_string(),
    );
    let projected = project_entities(&entities, &scope).unwrap();
    assert!(!projected.complete.contains("keybinding"));
    assert_eq!(projected.values.len(), 2);
}

#[test]
fn unsupported_theme_entries_make_the_category_incomplete() {
    let entities = BTreeMap::from([(
        "settings".into(),
        json!({"appearance":{"custom_themes":[{"id":"future","colors":"new format"}]}}).to_string(),
    )]);
    let projected = project_entities(&entities, &SyncScope::default()).unwrap();
    assert!(!projected.complete.contains("theme"));
}

#[test]
fn nested_connection_options_use_allowlists_and_keep_local_tuning() {
    let original = json!({"type":"ssh","host":"example","name":"Server","sftp":{"enabled":true,"pipeline_depth":32,"shell_detection_timeout_ms":800,"future_secret":"local"},"ssh_algorithms":{"mode":"custom","kex":["curve25519-sha256"],"future_secret":"local"}});
    let mut projected = project_record("connection", &original).unwrap();
    assert!(projected["sftp"].get("pipeline_depth").is_none());
    assert!(projected["sftp"].get("future_secret").is_none());
    assert!(projected["ssh_algorithms"].get("future_secret").is_none());
    projected["sftp"]["enabled"] = json!(false);
    let result = apply_record("connection", &original, &projected).unwrap();
    assert_eq!(result["sftp"]["enabled"], false);
    assert_eq!(result["sftp"]["pipeline_depth"], 32);
    assert_eq!(result["sftp"]["shell_detection_timeout_ms"], 800);
    assert_eq!(result["sftp"]["future_secret"], "local");
    assert_eq!(result["ssh_algorithms"]["future_secret"], "local");
}

#[test]
fn first_monitoring_result_does_not_create_an_empty_portable_asset() {
    let before = json!({"type":"ssh","host":"server","name":"Server","icon":"manual-icon"});
    let mut after = before.clone();
    after["asset"] = json!({"os_name":"Linux","cpu_cores":16,"memory_bytes":8192});
    assert_eq!(
        project_record("connection", &before).unwrap(),
        project_record("connection", &after).unwrap()
    );
    assert_eq!(
        project_record("connection", &before).unwrap()["icon"]["icon"],
        "manual-icon"
    );
    after["icon_auto_detect"] = json!(true);
    assert!(
        project_record("connection", &after)
            .unwrap()
            .get("icon")
            .is_none()
    );
}

#[test]
fn login_monitoring_remains_local_and_does_not_affect_cloud_projection() {
    let scope = SyncScope {
        preferences: true,
        ..Default::default()
    };
    let project = |enabled| {
        project_entities(
            &BTreeMap::from([(
                "settings".into(),
                json!({"ui":{"remote_stats_show_users":enabled}}).to_string(),
            )]),
            &scope,
        )
        .unwrap()
    };
    assert_eq!(project(false), project(true));
}
