use crate::models::connection_batch::{
    BatchField, ConnectionBatchEdit, apply_connection_batch_edit,
};
use crate::models::{ConnectionType, SavedConnection};

#[test]
fn saved_spice_round_trip_preserves_unknown_endpoint_and_preference_fields() {
    let document = serde_json::json!({
        "type": "spice", "id": "spice", "name": "VM console",
        "endpoint": {"host": "vm.example", "tls_port": 5901, "port": null,
            "future_endpoint": {"enabled": true}},
        "settings": {"future_preference": {"mode": 3},
            "security": {"future_security": true}},
        "future_connection": "preserve"
    });
    let mut connection: SavedConnection = serde_json::from_value(document).unwrap();
    let ConnectionType::Spice {
        username, settings, ..
    } = &mut connection.config
    else {
        panic!("SPICE")
    };
    assert!(settings.security.require_tls);
    *username = "vm-user".into();
    let saved = serde_json::to_value(&connection).unwrap();
    assert_eq!(saved["endpoint"]["future_endpoint"]["enabled"], true);
    assert_eq!(saved["settings"]["future_preference"]["mode"], 3);
    assert_eq!(saved["settings"]["security"]["future_security"], true);
    assert_eq!(saved["future_connection"], "preserve");
    assert_eq!(saved["username"], "vm-user");
    assert_eq!(connection, serde_json::from_value(saved).unwrap());
    assert_eq!(connection.kind_label(), "SPICE");
    assert_eq!(connection.endpoint(), "vm.example:5901");
}

#[test]
fn introducing_spice_does_not_change_existing_rdp_vnc_configuration_types() {
    for protocol in ["rdp", "vnc"] {
        let raw = serde_json::json!({"type": protocol, "name": "Existing",
            "host": "host.example", "port": 5900, "future_connection": {"v": 1}});
        let old: SavedConnection = serde_json::from_value(raw).unwrap();
        assert!(matches!(
            old.config,
            ConnectionType::Rdp { .. } | ConnectionType::Vnc { .. }
        ));
        let round_trip = serde_json::to_value(&old).unwrap();
        assert_eq!(round_trip["type"], protocol);
        assert_eq!(round_trip["host"], "host.example");
        assert_eq!(round_trip["port"], 5900);
        assert_eq!(round_trip["future_connection"]["v"], 1);
        assert_eq!(old, serde_json::from_value(round_trip).unwrap());
    }
}

#[test]
fn spice_settings_and_proxy_credentials_are_redacted_from_debug() {
    let config: ConnectionType = serde_json::from_value(serde_json::json!({
        "type": "spice", "name": "VM", "endpoint": {
            "host": "sensitive-host-ticket", "port": null, "tls_port": 5901,
            "proxy": "http://user:private-proxy-token@proxy.example:3128"}
    }))
    .unwrap();
    let debug = format!("{config:?}");
    assert!(!debug.contains("sensitive-host-ticket"));
    assert!(!debug.contains("private-proxy-token"));
}

#[test]
fn saved_spice_refuses_ticket_hosts_and_inline_proxy_passwords() {
    for endpoint in [
        serde_json::json!({"host":"pvespiceproxy:ticket:101:pve", "tls_port":5901}),
        serde_json::json!({"host":"vm", "tls_port":5901, "proxy":"http://user:secret@proxy:3128"}),
    ] {
        let raw = serde_json::json!({"type":"spice", "name":"VM", "endpoint":endpoint});
        assert!(serde_json::from_value::<SavedConnection>(raw).is_err());
    }
}

#[test]
fn shared_batch_port_edits_follow_the_spice_tls_policy_and_preserve_the_other_port() {
    let mut connection: SavedConnection = serde_json::from_value(serde_json::json!({
        "type": "spice", "name": "VM", "endpoint": {"host":"vm", "port":5900,"tls_port":5901}
    }))
    .unwrap();
    let edit = ConnectionBatchEdit {
        port: BatchField::Set(6100),
        ..Default::default()
    };
    apply_connection_batch_edit(&mut connection, &edit);
    let ConnectionType::Spice {
        endpoint, settings, ..
    } = &mut connection.config
    else {
        panic!("SPICE")
    };
    assert_eq!(endpoint.port, Some(5900));
    assert_eq!(endpoint.tls_port, Some(6100));
    settings.security.require_tls = false;
    apply_connection_batch_edit(&mut connection, &edit);
    let ConnectionType::Spice { endpoint, .. } = &connection.config else {
        panic!("SPICE")
    };
    assert_eq!(endpoint.port, Some(6100));
    assert_eq!(endpoint.tls_port, Some(6100));
}
