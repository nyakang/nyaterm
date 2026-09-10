use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use nyaterm_remote_desktop::{
    PROTOCOL_VERSION, PixelFormat, RdpCapability, RdpCertificatePolicy, RdpCertificateRequest,
    RdpCertificateResponse, RdpClipboardConfig, RdpControlMessage, RdpDisplayConfig,
    RdpDisplayMetrics, RdpError, RdpErrorKind, RdpFrameEvent, RdpInputEvent, RdpReconnectConfig,
    RdpServerCapabilities, RdpSessionConfig, RdpSessionState, decode_control, encode_control,
    encode_frame_packet, read_packet, write_packet,
};

fn helper_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nyaterm-rdp-helper"))
}

fn spawn_helper() -> Child {
    helper_command()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn send(stdin: &mut impl std::io::Write, message: &RdpControlMessage) {
    write_packet(stdin, &encode_control(message).unwrap()).unwrap();
}

fn recv(stdout: &mut impl std::io::Read) -> RdpControlMessage {
    decode_control(&read_packet(stdout).unwrap().unwrap()).unwrap()
}

fn client_hello() -> RdpControlMessage {
    RdpControlMessage::ClientHello {
        version: PROTOCOL_VERSION,
    }
}

fn assert_server_hello(message: RdpControlMessage) {
    assert!(matches!(
        message,
        RdpControlMessage::ServerHello {
            version: PROTOCOL_VERSION,
            capabilities: RdpServerCapabilities {
                committed_unicode_text: true,
                secure_attention: true,
            },
        }
    ));
}

fn assert_helper_rejects(messages: Vec<RdpControlMessage>) {
    let mut child = helper_command()
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for message in messages {
        send(&mut stdin, &message);
    }
    drop(stdin);
    assert!(!child.wait().unwrap().success());
}

fn assert_helper_rejects_frame(after_hello: bool) {
    let mut child = helper_command()
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    if after_hello {
        send(&mut stdin, &client_hello());
    }
    let frame = RdpFrameEvent::Bitmap {
        epoch: 1,
        full: true,
        x: 0,
        y: 0,
        width: 1,
        height: 1,
        stride: 4,
        format: PixelFormat::Bgra8,
        pixels: vec![0; 4],
    };
    write_packet(
        &mut stdin,
        &encode_frame_packet("test-session", &frame).unwrap(),
    )
    .unwrap();
    drop(stdin);
    assert!(!child.wait().unwrap().success());
}

fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn test_config() -> RdpSessionConfig {
    RdpSessionConfig {
        relay: None,
        name: "test".to_string(),
        host: "127.0.0.1".to_string(),
        port: closed_port(),
        username: String::new(),
        domain: String::new(),
        password: None,
        use_nla: false,
        certificate_policy: RdpCertificatePolicy::Prompt,
        display: RdpDisplayConfig::default(),
        clipboard: RdpClipboardConfig::default(),
        reconnect: RdpReconnectConfig::default(),
    }
}

fn certificate_request() -> RdpCertificateRequest {
    RdpCertificateRequest {
        request_id: "request".to_string(),
        host: "127.0.0.1".to_string(),
        port: 3389,
        sha256_fingerprint: "00".to_string(),
        subject: None,
        issuer: None,
        valid_from: None,
        valid_to: None,
    }
}

fn non_client_hello_messages() -> Vec<RdpControlMessage> {
    vec![
        RdpControlMessage::ServerHello {
            version: PROTOCOL_VERSION,
            capabilities: RdpServerCapabilities::default(),
        },
        RdpControlMessage::Connect {
            session_id: "test-session".to_string(),
            config: test_config(),
        },
        RdpControlMessage::DesktopReset {
            session_id: "test-session".to_string(),
            epoch: 1,
            width: 800,
            height: 600,
        },
        RdpControlMessage::State {
            session_id: "test-session".to_string(),
            state: RdpSessionState::Connecting,
            message: None,
        },
        RdpControlMessage::Input {
            session_id: "test-session".to_string(),
            events: vec![RdpInputEvent::ReleaseAllInputs],
        },
        RdpControlMessage::SecureAttention {
            session_id: "test-session".to_string(),
        },
        RdpControlMessage::Resize {
            session_id: "test-session".to_string(),
            metrics: RdpDisplayMetrics {
                width: 800,
                height: 600,
                desktop_scale_factor: 150,
                physical_size_mm: None,
            },
        },
        RdpControlMessage::Clipboard {
            session_id: "test-session".to_string(),
            text: "clipboard".to_string(),
            generation: 0,
        },
        RdpControlMessage::CertificateRequest(certificate_request()),
        RdpControlMessage::CertificateResponse {
            request_id: "request".to_string(),
            response: RdpCertificateResponse::Reject,
        },
        RdpControlMessage::Capability {
            session_id: "test-session".to_string(),
            capability: RdpCapability::DynamicResizeUnavailable,
        },
        RdpControlMessage::Error {
            session_id: "test-session".to_string(),
            error: RdpError::new(RdpErrorKind::Protocol, "wrong direction"),
            fatal: true,
        },
        RdpControlMessage::RequestFullFrame {
            session_id: "test-session".to_string(),
        },
        RdpControlMessage::Disconnect {
            session_id: "test-session".to_string(),
        },
    ]
}

fn helper_only_messages() -> Vec<RdpControlMessage> {
    vec![
        RdpControlMessage::ServerHello {
            version: PROTOCOL_VERSION,
            capabilities: RdpServerCapabilities::default(),
        },
        RdpControlMessage::DesktopReset {
            session_id: "test-session".to_string(),
            epoch: 1,
            width: 800,
            height: 600,
        },
        RdpControlMessage::State {
            session_id: "test-session".to_string(),
            state: RdpSessionState::Connecting,
            message: None,
        },
        RdpControlMessage::CertificateRequest(certificate_request()),
        RdpControlMessage::Capability {
            session_id: "test-session".to_string(),
            capability: RdpCapability::DynamicResizeUnavailable,
        },
        RdpControlMessage::Error {
            session_id: "test-session".to_string(),
            error: RdpError::new(RdpErrorKind::Protocol, "wrong direction"),
            fatal: true,
        },
    ]
}

fn foreign_session_messages() -> Vec<RdpControlMessage> {
    vec![
        RdpControlMessage::Input {
            session_id: "foreign-session".to_string(),
            events: vec![RdpInputEvent::ReleaseAllInputs],
        },
        RdpControlMessage::SecureAttention {
            session_id: "foreign-session".to_string(),
        },
        RdpControlMessage::Resize {
            session_id: "foreign-session".to_string(),
            metrics: RdpDisplayMetrics {
                width: 800,
                height: 600,
                desktop_scale_factor: 100,
                physical_size_mm: None,
            },
        },
        RdpControlMessage::Clipboard {
            session_id: "foreign-session".to_string(),
            text: "clipboard".to_string(),
            generation: 0,
        },
        RdpControlMessage::RequestFullFrame {
            session_id: "foreign-session".to_string(),
        },
        RdpControlMessage::Disconnect {
            session_id: "foreign-session".to_string(),
        },
    ]
}

#[test]
fn helper_handshake_disconnects_and_reaps_cleanly() {
    let mut child = spawn_helper();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();

    send(&mut stdin, &client_hello());
    assert_server_hello(recv(&mut stdout));

    send(
        &mut stdin,
        &RdpControlMessage::Disconnect {
            session_id: "test-session".to_string(),
        },
    );
    assert!(matches!(
        recv(&mut stdout),
        RdpControlMessage::State {
            state: RdpSessionState::Disconnecting,
            ..
        }
    ));
    assert!(matches!(
        recv(&mut stdout),
        RdpControlMessage::State {
            state: RdpSessionState::Disconnected,
            ..
        }
    ));
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
fn pipelined_client_hello_and_connect_still_emit_server_hello_first() {
    let mut child = spawn_helper();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();

    send(&mut stdin, &client_hello());
    send(
        &mut stdin,
        &RdpControlMessage::Connect {
            session_id: "pipelined".to_string(),
            config: test_config(),
        },
    );

    assert_server_hello(recv(&mut stdout));
    assert!(matches!(
        recv(&mut stdout),
        RdpControlMessage::State {
            state: RdpSessionState::Connecting,
            ..
        }
    ));

    child.kill().unwrap();
    let status = child.wait().unwrap();
    assert!(!status.success());
}

#[test]
fn every_non_client_hello_message_and_frame_fail_before_hello() {
    for message in non_client_hello_messages() {
        assert_helper_rejects(vec![message]);
    }
    assert_helper_rejects_frame(false);
}

#[test]
fn repeated_hello_wrong_version_and_helper_only_messages_fail_closed() {
    assert_helper_rejects(vec![RdpControlMessage::ClientHello {
        version: PROTOCOL_VERSION - 1,
    }]);
    assert_helper_rejects(vec![client_hello(), client_hello()]);
    for message in helper_only_messages() {
        assert_helper_rejects(vec![client_hello(), message]);
    }
    assert_helper_rejects_frame(true);
}

#[test]
fn every_session_scoped_message_with_a_foreign_id_fails_closed() {
    for message in foreign_session_messages() {
        assert_helper_rejects(vec![
            client_hello(),
            RdpControlMessage::Connect {
                session_id: "active-session".to_string(),
                config: test_config(),
            },
            message,
        ]);
    }
}

#[test]
fn helper_crash_and_hang_processes_can_always_be_reaped() {
    let crash = helper_command()
        .env("NYATERM_RDP_HELPER_TEST_MODE", "crash")
        .status()
        .unwrap();
    assert_eq!(crash.code(), Some(91));

    let mut hung = helper_command()
        .env("NYATERM_RDP_HELPER_TEST_MODE", "hang")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_millis(100);
    while Instant::now() < deadline {
        assert!(hung.try_wait().unwrap().is_none());
        std::thread::sleep(Duration::from_millis(10));
    }
    hung.kill().unwrap();
    let status = hung.wait().unwrap();
    assert!(!status.success());
}
