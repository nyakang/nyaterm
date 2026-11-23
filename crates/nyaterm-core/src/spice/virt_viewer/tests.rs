use crate::secret::SecretString;
use crate::spice::settings::{
    SpiceChannel, SpiceClipboardMode, SpiceConfigError, SpiceEndpoint, SpiceSecuritySettings,
    SpiceSettings,
};
use crate::spice::virt_viewer::{MAX_VIRT_VIEWER_BYTES, VirtViewerError, parse_virt_viewer};

const PROXMOX_DOCUMENT: &str = concat!(
    "[virt-viewer]\n",
    "type=spice\n",
    "host=pvespiceproxy:ticket:100:node:signature\n",
    "proxy=http://pve.example.test:3128\n",
    "tls-port=61000\n",
    "port=-1\n",
    "password=temporary-ticket\n",
    "ca=-----BEGIN CERTIFICATE-----\\nfixture\\n-----END CERTIFICATE-----\\n\n",
    "host-subject=OU=PVE Cluster Node,O=Example,CN=node.example.test\n",
    "title=VM 100\n",
    "delete-this-file=1\n",
    "secure-channels=main;display;inputs;cursor;\n",
);

#[test]
fn proxmox_ticket_remains_opaque_and_tls_identity_is_preserved() {
    let document = parse_virt_viewer(PROXMOX_DOCUMENT.as_bytes()).unwrap();
    assert_eq!(
        document.endpoint.host,
        "pvespiceproxy:ticket:100:node:signature"
    );
    assert_eq!(document.endpoint.port, None);
    assert_eq!(document.endpoint.tls_port, Some(61000));
    assert_eq!(
        document.endpoint.proxy.unwrap(),
        "http://pve.example.test:3128"
    );
    assert_eq!(document.password, "temporary-ticket");
    assert_eq!(
        document.settings.security.ca_pem.as_deref(),
        Some("-----BEGIN CERTIFICATE-----\nfixture\n-----END CERTIFICATE-----\n")
    );
    assert_eq!(
        document.settings.security.certificate_subject.as_deref(),
        Some("OU=PVE Cluster Node,O=Example,CN=node.example.test")
    );
    assert!(document.settings.security.require_tls);
    assert_eq!(
        document.settings.security.secure_channels,
        [
            SpiceChannel::Main,
            SpiceChannel::Display,
            SpiceChannel::Inputs,
            SpiceChannel::Cursor
        ]
    );
    assert!(document.delete_after_open);
    assert_eq!(document.settings.reconnect_attempts, 0);
}

#[test]
fn document_debug_does_not_disclose_ticket_password_title_or_extensions() {
    let text = format!(
        "{PROXMOX_DOCUMENT}provider-token=extension-secret\n[ovirt]\nsso-token=oauth-secret\n"
    );
    let document = parse_virt_viewer(text.as_bytes()).unwrap();
    let debug = format!("{document:?}");
    for sensitive in [
        "temporary-ticket",
        "pvespiceproxy",
        "VM 100",
        "extension-secret",
        "oauth-secret",
        "pve.example.test",
    ] {
        assert!(!debug.contains(sensitive), "Debug leaked {sensitive}");
    }
    assert_eq!(
        document.unhandled["virt-viewer"]["provider-token"],
        "extension-secret"
    );
    assert_eq!(document.unhandled["ovirt"]["sso-token"], "oauth-secret");
}

#[test]
fn plain_qemu_document_does_not_invent_a_tls_port() {
    let document =
        parse_virt_viewer(b"[virt-viewer]\ntype=spice\nhost=localhost\nport=5900\n").unwrap();
    assert!(!document.settings.security.require_tls);
    assert_eq!(document.endpoint.port, Some(5900));
    assert_eq!(document.endpoint.tls_port, None);
    assert!(document.password.is_empty());
}

#[test]
fn tls_document_never_silently_uses_its_plaintext_port() {
    let text = PROXMOX_DOCUMENT.replace("port=-1", "port=5900");
    let document = parse_virt_viewer(text.as_bytes()).unwrap();
    assert_eq!(document.endpoint.port, Some(5900));
    assert!(document.settings.security.require_tls);
}

#[test]
fn tls_requirement_without_tls_endpoint_is_an_error() {
    let text = "[virt-viewer]\ntype=spice\nhost=localhost\nport=5900\nsecure-channels=main\n";
    assert_eq!(
        parse_virt_viewer(text.as_bytes()).unwrap_err(),
        VirtViewerError::Config(SpiceConfigError::TlsPortRequired)
    );
}

#[test]
fn duplicate_identity_or_password_is_rejected_without_echoing_values() {
    for extra in ["host=secret-host\n", "password=secret-password\n"] {
        let error = parse_virt_viewer(format!("{PROXMOX_DOCUMENT}{extra}").as_bytes()).unwrap_err();
        assert!(matches!(error, VirtViewerError::Duplicate { .. }));
        assert!(!error.to_string().contains("secret"));
    }
}

#[test]
fn key_file_escaping_preserves_password_spaces_and_literal_backslashes() {
    let text = "\u{feff}[virt-viewer]\r\ntype=spice\r\nhost=::1\r\nport=5900\r\npassword=\\ssecret\\\\password=1#2\\s\r\ntitle=VM\\sconsole\r\n";
    let document = parse_virt_viewer(text.as_bytes()).unwrap();
    assert_eq!(document.endpoint.host, "::1");
    assert_eq!(document.password, " secret\\password=1#2 ");
    assert_eq!(document.title, "VM console");
}

#[test]
fn extension_values_are_retained_without_interpreting_foreign_escape_syntax() {
    let text = format!("{PROXMOX_DOCUMENT}vendor-option=raw\\x42\\custom\n");
    let document = parse_virt_viewer(text.as_bytes()).unwrap();
    assert_eq!(
        document.unhandled["virt-viewer"]["vendor-option"],
        "raw\\x42\\custom"
    );
}

#[test]
fn importing_device_options_does_not_authorize_device_redirection() {
    let text = format!(
        "{PROXMOX_DOCUMENT}enable-usbredir=1\nenable-smartcard=1\nenable-usb-autoshare=1\n"
    );
    let document = parse_virt_viewer(text.as_bytes()).unwrap();
    assert_eq!(document.unhandled["virt-viewer"]["enable-usbredir"], "1");
    assert_eq!(document.unhandled["virt-viewer"].len(), 3);
}

#[test]
fn invalid_ports_hosts_and_proxy_urls_fail_before_a_connection_is_started() {
    for field in ["port=0", "port=65536", "port=-2", "port=secret", "port="] {
        let text = format!("[virt-viewer]\ntype=spice\nhost=localhost\n{field}\n");
        assert_eq!(
            parse_virt_viewer(text.as_bytes()).unwrap_err(),
            VirtViewerError::InvalidField("port")
        );
    }
    for host in ["", "one two", "host\tname"] {
        let text = format!("[virt-viewer]\ntype=spice\nhost={host}\nport=5900\n");
        assert_eq!(
            parse_virt_viewer(text.as_bytes()).unwrap_err(),
            VirtViewerError::Config(SpiceConfigError::InvalidHost)
        );
    }
    for proxy in [
        "socks5://proxy:1080",
        "file:///secret",
        "http://proxy/path",
        "http://proxy/#secret",
        "http://proxy:0",
    ] {
        let text = format!("[virt-viewer]\ntype=spice\nhost=vm\nport=5900\nproxy={proxy}\n");
        let error = parse_virt_viewer(text.as_bytes()).unwrap_err();
        assert_eq!(
            error,
            VirtViewerError::Config(SpiceConfigError::InvalidProxy)
        );
        assert!(!error.to_string().contains(proxy));
    }
}

#[test]
fn malformed_and_oversized_documents_have_bounded_diagnostics() {
    assert_eq!(
        parse_virt_viewer(&vec![b'x'; MAX_VIRT_VIEWER_BYTES + 1]).unwrap_err(),
        VirtViewerError::TooLarge
    );
    assert_eq!(
        parse_virt_viewer(&[0xff]).unwrap_err(),
        VirtViewerError::Encoding
    );
    assert_eq!(
        parse_virt_viewer(b"password=hidden").unwrap_err(),
        VirtViewerError::Syntax { line: 1 }
    );
    assert!(matches!(
        parse_virt_viewer(b"[virt-viewer]\ntype=spice\nhost=bad\0host"),
        Err(VirtViewerError::Syntax { line: 3 })
    ));
    let mut text = String::from(PROXMOX_DOCUMENT);
    for i in 0..512 {
        text.push_str(&format!("extension-{i}=value\n"));
    }
    assert_eq!(
        parse_virt_viewer(text.as_bytes()).unwrap_err(),
        VirtViewerError::TooManyFields
    );
}

#[test]
fn unsupported_protocol_and_security_channels_are_explicit_errors() {
    assert_eq!(
        parse_virt_viewer(b"[virt-viewer]\ntype=vnc\nhost=host\nport=5900\n").unwrap_err(),
        VirtViewerError::UnsupportedProtocol
    );
    let text = PROXMOX_DOCUMENT.replace("main;display;inputs;cursor;", "unknown-channel;");
    assert_eq!(
        parse_virt_viewer(text.as_bytes()).unwrap_err(),
        VirtViewerError::InvalidField("secure-channels")
    );
}

#[test]
fn disabled_channels_and_boolean_flags_are_parsed() {
    let text = format!("{PROXMOX_DOCUMENT}disable-channels=playback;record;\nfullscreen=true\n");
    let document = parse_virt_viewer(text.as_bytes()).unwrap();
    assert!(document.fullscreen);
    assert_eq!(
        document.settings.disabled_channels,
        [SpiceChannel::Playback, SpiceChannel::Record]
    );
}

#[test]
fn missing_preferences_default_and_unknown_settings_survive_a_round_trip() {
    let document = serde_json::json!({
        "security": {"future-trust-rule": {"opaque": "kept"}},
        "future-feature": [1, 2, 3],
        "clipboard": "to_guest"
    });
    let settings: SpiceSettings = serde_json::from_value(document.clone()).unwrap();
    assert!(settings.security.require_tls);
    assert!(settings.dynamic_resolution);
    assert_eq!(settings.clipboard, SpiceClipboardMode::ToGuest);
    let output = serde_json::to_value(&settings).unwrap();
    assert_eq!(
        output["security"]["future-trust-rule"],
        document["security"]["future-trust-rule"]
    );
    assert_eq!(output["future-feature"], document["future-feature"]);
    assert_eq!(
        serde_json::from_value::<SpiceSettings>(output).unwrap(),
        settings
    );
    assert!(!format!("{settings:?}").contains("kept"));
}

#[test]
fn manual_connections_default_to_tls_and_require_an_explicit_plaintext_choice() {
    let endpoint = SpiceEndpoint {
        extra: Default::default(),
        host: SecretString::from("localhost"),
        port: Some(5900),
        tls_port: None,
        proxy: None,
    };
    let mut security = SpiceSecuritySettings::default();
    assert_eq!(
        security.validate(&endpoint),
        Err(SpiceConfigError::TlsPortRequired)
    );
    security.require_tls = false;
    security.validate(&endpoint).unwrap();
}

#[test]
fn clipboard_permissions_are_directional() {
    assert!(!SpiceClipboardMode::Disabled.can_send());
    assert!(!SpiceClipboardMode::Disabled.can_receive());
    assert!(SpiceClipboardMode::ToGuest.can_send());
    assert!(!SpiceClipboardMode::ToGuest.can_receive());
    assert!(!SpiceClipboardMode::FromGuest.can_send());
    assert!(SpiceClipboardMode::FromGuest.can_receive());
    assert!(SpiceClipboardMode::Bidirectional.can_send());
    assert!(SpiceClipboardMode::Bidirectional.can_receive());
}
