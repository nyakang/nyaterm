use std::io::Write;
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use nyaterm_remote_desktop::{
    PROTOCOL_VERSION, PixelFormat, RemoteFrameEvent, VncClipboardConfig, VncControlMessage,
    VncDisplayConfig, VncError, VncErrorKind, VncInputEvent, VncReconnectConfig, VncSecurityConfig,
    VncServerCapabilities, VncSessionConfig, VncSessionState, decode_vnc_control,
    encode_frame_packet, encode_vnc_control, read_packet, write_packet,
};

fn helper_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nyaterm-vnc-helper"))
}

fn spawn_helper() -> Child {
    helper_command()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn send(stdin: &mut impl Write, message: &VncControlMessage) {
    write_packet(stdin, &encode_vnc_control(message).unwrap()).unwrap();
}

fn recv(stdout: &mut impl std::io::Read) -> VncControlMessage {
    decode_vnc_control(&read_packet(stdout).unwrap().unwrap()).unwrap()
}

fn client_hello() -> VncControlMessage {
    VncControlMessage::ClientHello {
        version: PROTOCOL_VERSION,
    }
}

fn assert_server_hello(message: VncControlMessage) {
    assert!(matches!(
        message,
        VncControlMessage::ServerHello {
            version: PROTOCOL_VERSION,
            capabilities: VncServerCapabilities {
                committed_unicode_keysyms: true,
            },
        }
    ));
}

fn handshake(stdin: &mut impl Write, stdout: &mut impl std::io::Read) {
    send(stdin, &client_hello());
    assert_server_hello(recv(stdout));
}

fn assert_helper_rejects(messages: Vec<VncControlMessage>) {
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
    let frame = RemoteFrameEvent::Bitmap {
        epoch: 1,
        full: true,
        x: 0,
        y: 0,
        width: 1,
        height: 1,
        stride: 4,
        format: PixelFormat::Rgba8,
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

/// A port with nothing listening on it: bind, read the port, then release it.
fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn config(port: u16) -> VncSessionConfig {
    VncSessionConfig {
        relay: None,
        name: "lifecycle".to_string(),
        host: "127.0.0.1".to_string(),
        port,
        password: None,
        security: VncSecurityConfig::default(),
        display: VncDisplayConfig::default(),
        clipboard: VncClipboardConfig::default(),
        // Fail on the first attempt instead of walking the reconnect ladder.
        reconnect: VncReconnectConfig {
            enabled: false,
            max_attempts: 0,
        },
        shared: true,
        view_only: false,
    }
}

fn non_client_hello_messages() -> Vec<VncControlMessage> {
    vec![
        VncControlMessage::ServerHello {
            version: PROTOCOL_VERSION,
            capabilities: VncServerCapabilities::default(),
        },
        VncControlMessage::Connect {
            session_id: "test-session".to_string(),
            config: config(closed_port()),
        },
        VncControlMessage::DesktopReset {
            session_id: "test-session".to_string(),
            epoch: 1,
            width: 800,
            height: 600,
        },
        VncControlMessage::State {
            session_id: "test-session".to_string(),
            state: VncSessionState::Connecting,
            message: None,
        },
        VncControlMessage::Input {
            session_id: "test-session".to_string(),
            events: vec![VncInputEvent::ReleaseAllInputs],
        },
        VncControlMessage::Clipboard {
            session_id: "test-session".to_string(),
            text: "clipboard".to_string(),
        },
        VncControlMessage::Error {
            session_id: "test-session".to_string(),
            error: VncError::new(VncErrorKind::Protocol, "wrong direction"),
            fatal: true,
        },
        VncControlMessage::RequestFullFrame {
            session_id: "test-session".to_string(),
        },
        VncControlMessage::Disconnect {
            session_id: "test-session".to_string(),
        },
    ]
}

fn helper_only_messages() -> Vec<VncControlMessage> {
    vec![
        VncControlMessage::ServerHello {
            version: PROTOCOL_VERSION,
            capabilities: VncServerCapabilities::default(),
        },
        VncControlMessage::DesktopReset {
            session_id: "test-session".to_string(),
            epoch: 1,
            width: 800,
            height: 600,
        },
        VncControlMessage::State {
            session_id: "test-session".to_string(),
            state: VncSessionState::Connecting,
            message: None,
        },
        VncControlMessage::Error {
            session_id: "test-session".to_string(),
            error: VncError::new(VncErrorKind::Protocol, "wrong direction"),
            fatal: true,
        },
    ]
}

fn foreign_session_messages() -> Vec<VncControlMessage> {
    vec![
        VncControlMessage::Input {
            session_id: "foreign-session".to_string(),
            events: vec![VncInputEvent::ReleaseAllInputs],
        },
        VncControlMessage::Clipboard {
            session_id: "foreign-session".to_string(),
            text: "clipboard".to_string(),
        },
        VncControlMessage::RequestFullFrame {
            session_id: "foreign-session".to_string(),
        },
        VncControlMessage::Disconnect {
            session_id: "foreign-session".to_string(),
        },
    ]
}

#[test]
fn helper_handshake_disconnects_and_reaps_cleanly() {
    let mut child = spawn_helper();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();

    handshake(&mut stdin, &mut stdout);

    send(
        &mut stdin,
        &VncControlMessage::Disconnect {
            session_id: "test-session".to_string(),
        },
    );
    assert!(matches!(
        recv(&mut stdout),
        VncControlMessage::State {
            state: VncSessionState::Disconnecting,
            ..
        }
    ));
    assert!(matches!(
        recv(&mut stdout),
        VncControlMessage::State {
            state: VncSessionState::Disconnected,
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
        &VncControlMessage::Connect {
            session_id: "pipelined".to_string(),
            config: config(closed_port()),
        },
    );

    assert_server_hello(recv(&mut stdout));
    send(
        &mut stdin,
        &VncControlMessage::Disconnect {
            session_id: "pipelined".to_string(),
        },
    );
    drop(stdin);
    let _ = child.wait().unwrap();
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
    assert_helper_rejects(vec![VncControlMessage::ClientHello {
        version: PROTOCOL_VERSION + 1,
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
            VncControlMessage::Connect {
                session_id: "active-session".to_string(),
                config: config(closed_port()),
            },
            message,
        ]);
    }
}

#[test]
fn unreachable_server_reports_a_fatal_transport_error_and_still_disconnects() {
    let mut child = spawn_helper();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();

    handshake(&mut stdin, &mut stdout);

    send(
        &mut stdin,
        &VncControlMessage::Connect {
            session_id: "unreachable".to_string(),
            config: config(closed_port()),
        },
    );
    assert!(matches!(
        recv(&mut stdout),
        VncControlMessage::State {
            state: VncSessionState::Connecting,
            ..
        }
    ));
    let VncControlMessage::Error {
        session_id,
        error,
        fatal,
    } = recv(&mut stdout)
    else {
        panic!("expected a fatal connection error");
    };
    assert_eq!(session_id, "unreachable");
    assert_eq!(error.kind, VncErrorKind::Transport);
    assert!(fatal);

    // A dead worker must not wedge the control channel.
    send(
        &mut stdin,
        &VncControlMessage::Disconnect {
            session_id: "unreachable".to_string(),
        },
    );
    assert!(matches!(
        recv(&mut stdout),
        VncControlMessage::State {
            state: VncSessionState::Disconnecting,
            ..
        }
    ));
    assert!(matches!(
        recv(&mut stdout),
        VncControlMessage::State {
            state: VncSessionState::Disconnected,
            ..
        }
    ));
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
fn a_second_connect_is_rejected_without_replacing_the_session() {
    let mut child = spawn_helper();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();

    handshake(&mut stdin, &mut stdout);
    let port = closed_port();
    send(
        &mut stdin,
        &VncControlMessage::Connect {
            session_id: "first".to_string(),
            config: config(port),
        },
    );
    send(
        &mut stdin,
        &VncControlMessage::Connect {
            session_id: "second".to_string(),
            config: config(port),
        },
    );

    // The rejection names the second session and is fatal for it.
    let mut rejected = None;
    for _ in 0..6 {
        if let VncControlMessage::Error {
            session_id,
            error,
            fatal,
        } = recv(&mut stdout)
            && session_id == "second"
        {
            rejected = Some((error, fatal));
            break;
        }
    }
    let (error, fatal) = rejected.expect("the second connect should be rejected");
    assert_eq!(error.kind, VncErrorKind::Protocol);
    assert!(fatal);
    assert!(error.message.contains("already owns"));

    drop(stdin);
    child.wait().unwrap();
}

#[test]
fn helper_crash_and_hang_processes_can_always_be_reaped() {
    let crash = helper_command()
        .env("NYATERM_VNC_HELPER_TEST_MODE", "crash")
        .status()
        .unwrap();
    assert_eq!(crash.code(), Some(91));

    let mut hung = helper_command()
        .env("NYATERM_VNC_HELPER_TEST_MODE", "hang")
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
