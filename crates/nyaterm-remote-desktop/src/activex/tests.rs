use crate::{RdpBackend, RdpErrorKind, RdpSessionConfig, RdpSessionManager};

#[cfg(windows)]
use crate::RdpClipboardMode;

use super::{HostCommand, NativeRdpRect, decode, encode};

#[test]
fn session_details_accept_old_hosts_and_round_trip_optional_resize_capability() {
    use super::{HostEvent, NativeRdpSessionDetails};
    let old: NativeRdpSessionDetails = serde_json::from_str("{}").unwrap();
    assert_eq!(old.dynamic_resize_available, None);
    for available in [None, Some(true), Some(false)] {
        let event = HostEvent::Details(NativeRdpSessionDetails {
            dynamic_resize_available: available,
            ..Default::default()
        });
        let HostEvent::Details(details) = decode(&encode(&event).unwrap()).unwrap() else {
            panic!("expected session details");
        };
        assert_eq!(details.dynamic_resize_available, available);
    }
}

fn config() -> RdpSessionConfig {
    serde_json::from_value(serde_json::json!({
        "backend":"windows", "windows_authentication":"system",
        "name":"test", "host":"example.invalid", "port":3389,
        "username":"", "domain":"", "password":null, "use_nla":true,
        "certificate_policy":"strict",
        "display":{"mode":"fit_window","width":800,"height":600,"color_depth":32},
        "clipboard":{"mode":"disabled"}, "reconnect":{"enabled":false,"max_attempts":0}
    }))
    .unwrap()
}

#[test]
fn builtin_manager_never_silently_runs_an_activex_configuration() {
    let manager = RdpSessionManager::new();
    let error = manager
        .create_session_with_id("explicit-windows".into(), config())
        .unwrap_err();
    assert_eq!(error.kind, RdpErrorKind::Unsupported);
}

#[cfg(windows)]
#[test]
fn activex_requires_explicit_system_authentication_and_clipboard_permission() {
    let mut config = config();
    assert!(config.backend.validate_config(&config).is_ok());
    config.windows_authentication = None;
    assert_eq!(
        config.backend.validate_config(&config).unwrap_err().kind,
        RdpErrorKind::Unsupported
    );
    config.windows_authentication = Some("system".into());
    config.clipboard.mode = RdpClipboardMode::TextOnly;
    assert_eq!(
        config.backend.validate_config(&config).unwrap_err().kind,
        RdpErrorKind::Unsupported
    );
    config.clipboard.mode = RdpClipboardMode::TextAndFiles;
    assert!(config.backend.validate_config(&config).is_ok());
    config.reconnect.max_attempts = 201;
    assert!(config.backend.validate_config(&config).is_err());
}

#[cfg(not(windows))]
#[test]
fn activex_is_explicitly_unsupported_on_other_platforms() {
    let config = config();
    assert_eq!(
        config.backend.validate_config(&config).unwrap_err().kind,
        RdpErrorKind::Unsupported
    );
}

#[test]
fn legacy_auto_reads_as_builtin_and_never_serializes_as_auto() {
    for name in ["auto", "internal", "builtin"] {
        let backend: RdpBackend = serde_json::from_value(serde_json::json!(name)).unwrap();
        assert_eq!(backend, RdpBackend::Builtin);
        assert_eq!(serde_json::to_value(backend).unwrap(), "builtin");
    }
}

#[test]
fn route_support_is_checked_before_a_relay_is_created() {
    assert!(RdpBackend::Builtin.validate_route(true).is_ok());
    let error = RdpBackend::Windows.validate_route(true).unwrap_err();
    assert_eq!(error.kind, RdpErrorKind::Unsupported);
    #[cfg(windows)]
    assert!(error.message.contains("Relay"));
}

#[test]
fn viewport_ipc_preserves_dpi_and_rejects_non_control_packets() {
    let rect = NativeRdpRect {
        x: 10,
        y: 20,
        width: 1600,
        height: 900,
        scale_percent: 200,
    };
    let packet = encode(&HostCommand::Viewport {
        rect,
        visible: true,
    })
    .unwrap();
    let HostCommand::Viewport {
        rect: decoded,
        visible,
    } = decode(&packet).unwrap()
    else {
        panic!("viewport")
    };
    assert_eq!(decoded, rect);
    assert!(visible);
    assert!(rect.validate().is_ok());
    assert!(
        NativeRdpRect {
            scale_percent: 0,
            ..rect
        }
        .validate()
        .is_err()
    );
    let mut wrong = packet;
    wrong.packet_type = crate::PacketType::Frame;
    assert!(decode::<HostCommand>(&wrong).is_err());
}

#[test]
fn secret_bearing_commands_have_redacted_debug() {
    let mut config = config();
    config.password = Some(nyaterm_core::SecretString::from("fixture-secret"));
    let command = HostCommand::Prepare {
        parent: 1,
        rect: NativeRdpRect::default(),
        config: Box::new(config),
    };
    assert!(!format!("{command:?}").contains("fixture-secret"));
}
