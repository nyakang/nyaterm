//! End-to-end coverage of the application-side manager driving a real helper.
//!
//! `VncSessionManager` lives in `nyaterm-remote-desktop`, which cannot depend on
//! this crate, so the round trip is exercised from here where both the manager and
//! the helper binary are available.

use std::net::{TcpListener, TcpStream};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use nyaterm_remote_desktop::{
    VncClipboardConfig, VncDisplayConfig, VncErrorKind, VncInputEvent, VncReconnectConfig,
    VncRuntimeEvent, VncSecurityConfig, VncSecurityMode, VncSessionConfig, VncSessionManager,
    VncSessionState,
};

/// Point the manager's helper lookup at the binary this crate just built.
///
/// The sibling-of-`current_exe()` lookup cannot work from a test binary, which
/// lives in `deps/`. Tests in this file run concurrently, so the write happens
/// exactly once behind `LazyLock` rather than once per test.
static BUILT_HELPER: LazyLock<()> = LazyLock::new(|| unsafe {
    std::env::set_var(
        "NYATERM_VNC_HELPER",
        env!("CARGO_BIN_EXE_nyaterm-vnc-helper"),
    );
});

fn use_built_helper() {
    LazyLock::force(&BUILT_HELPER);
}

/// A port with nothing listening on it: bind, read the port, then release it.
fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn config(port: u16) -> VncSessionConfig {
    VncSessionConfig {
        username: String::new(),
        relay: None,
        name: "manager".to_string(),
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

/// Drain until `predicate` accepts one of the control events, or time out.
fn wait_for_event(
    manager: &VncSessionManager,
    session_id: &str,
    predicate: impl Fn(&VncRuntimeEvent) -> bool,
) -> VncRuntimeEvent {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        for event in manager.drain(session_id).control {
            if predicate(&event) {
                return event;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for a matching VNC runtime event");
}

#[test]
fn unreachable_server_surfaces_a_fatal_error_and_close_leaves_a_queryable_state() {
    use_built_helper();
    let manager = VncSessionManager::new();
    let session_id = manager
        .create_session_with_id("managed".to_string(), config(closed_port()))
        .expect("the helper should spawn");
    assert_eq!(session_id, "managed");

    let event = wait_for_event(&manager, &session_id, |event| {
        matches!(event, VncRuntimeEvent::Error { fatal: true, .. })
    });
    let VncRuntimeEvent::Error { error, .. } = event else {
        unreachable!("filtered above");
    };
    assert_eq!(error.kind, VncErrorKind::Transport);
    assert_eq!(manager.state(&session_id), Some(VncSessionState::Failed));

    // close() keeps the record so state() still answers, matching RdpSessionManager.
    manager.close(&session_id).expect("close");
    assert_eq!(
        manager.state(&session_id),
        Some(VncSessionState::Disconnected)
    );

    // The same id can be reused once the previous helper is gone.
    manager
        .create_session_with_id(session_id.clone(), config(closed_port()))
        .expect("a closed session id should be reusable");
    manager.close(&session_id).expect("close again");
}

#[test]
fn a_live_session_id_cannot_be_created_twice() {
    use_built_helper();
    let manager = VncSessionManager::new();
    let port = closed_port();
    manager
        .create_session_with_id("duplicate".to_string(), config(port))
        .expect("first create");
    let error = manager
        .create_session_with_id("duplicate".to_string(), config(port))
        .expect_err("the second create should be rejected");
    assert_eq!(error.kind, VncErrorKind::Protocol);
    assert!(error.message.contains("already running"));
    manager.close("duplicate").expect("close");
}

#[test]
fn invalid_configuration_fails_before_a_helper_is_spawned() {
    // Deliberately does not touch the helper override: validation must reject the
    // config before resolve_helper is ever consulted.
    let manager = VncSessionManager::new();
    let mut invalid = config(5900);
    invalid.security.mode = VncSecurityMode::VncAuth;
    invalid.password = None;
    let error = manager
        .create_session_with_id("invalid".to_string(), invalid)
        .expect_err("VncAuth without a password should fail");
    assert_eq!(error.kind, VncErrorKind::Authentication);
    assert_eq!(manager.state("invalid"), None);
}

#[test]
fn sending_input_to_an_unknown_session_is_an_error_not_a_panic() {
    use_built_helper();
    let manager = VncSessionManager::new();
    let error = manager
        .send_input("missing", vec![VncInputEvent::ReleaseAllInputs])
        .expect_err("no such session");
    assert_eq!(error.kind, VncErrorKind::Protocol);
    assert!(manager.drain("missing").control.is_empty());
}

/// A minimal RFB 3.8 server that offers exactly one security type: `None`.
///
/// `send_first_frame` runs on the client's first `FramebufferUpdateRequest`, which
/// is where a real server starts pushing updates. Later client traffic (further
/// update requests, pixel format, encodings) is drained until the helper hangs up,
/// so a mock server always ends cleanly and never races the assertion about frames.
fn spawn_no_auth_server(
    framebuffer: (u16, u16),
    send_first_frame: impl FnOnce(&mut TcpStream) -> std::io::Result<()> + Send + 'static,
) -> (u16, std::thread::JoinHandle<std::io::Result<()>>) {
    use std::io::{ErrorKind, Read as _, Write as _};

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || -> std::io::Result<()> {
        let (mut stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.write_all(b"RFB 003.008\n")?;
        let mut version = [0; 12];
        stream.read_exact(&mut version)?;
        assert_eq!(&version, b"RFB 003.008\n");
        // One security type: None (1).
        stream.write_all(&[1, 1])?;
        let mut selected = [0; 1];
        stream.read_exact(&mut selected)?;
        assert_eq!(selected, [1]);
        stream.write_all(&0_u32.to_be_bytes())?;
        let mut shared = [0; 1];
        stream.read_exact(&mut shared)?;
        assert_eq!(shared, [1]);
        // A true-colour framebuffer with an empty desktop name. The client overrides
        // the pixel format, so these components only have to be well formed.
        let (width, height) = framebuffer;
        let mut server_init = Vec::new();
        server_init.extend_from_slice(&width.to_be_bytes());
        server_init.extend_from_slice(&height.to_be_bytes());
        server_init.extend_from_slice(&[32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
        server_init.extend_from_slice(&0_u32.to_be_bytes());
        stream.write_all(&server_init)?;

        let mut send_first_frame = Some(send_first_frame);
        loop {
            let mut message = [0; 1];
            match stream.read_exact(&mut message) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::UnexpectedEof => return Ok(()),
                Err(error) => return Err(error),
            }
            match message[0] {
                0 => stream.read_exact(&mut [0; 19])?,
                2 => {
                    let mut header = [0; 3];
                    stream.read_exact(&mut header)?;
                    let count = usize::from(u16::from_be_bytes([header[1], header[2]]));
                    stream.read_exact(&mut vec![0; count * 4])?;
                }
                3 => {
                    stream.read_exact(&mut [0; 9])?;
                    if let Some(send_first_frame) = send_first_frame.take() {
                        send_first_frame(&mut stream)?;
                    }
                }
                other => panic!("unexpected client message {other}"),
            }
        }
    });
    (port, server)
}

/// Append one `FramebufferUpdate` carrying `rectangles` 1x1 Raw rectangles.
fn push_update(stream: &mut TcpStream, width: u16, height: u16, rectangles: u16) {
    use std::io::Write as _;

    let mut update = Vec::new();
    update.push(0); // FramebufferUpdate
    update.push(0); // padding
    update.extend_from_slice(&rectangles.to_be_bytes());
    for index in 0..rectangles {
        let x = index % width;
        let y = (index / width) % height;
        update.extend_from_slice(&x.to_be_bytes());
        update.extend_from_slice(&y.to_be_bytes());
        update.extend_from_slice(&1_u16.to_be_bytes());
        update.extend_from_slice(&1_u16.to_be_bytes());
        update.extend_from_slice(&0_i32.to_be_bytes()); // Raw encoding
        update.extend_from_slice(&[0x40, 0x80, 0xc0, 0xff]);
    }
    stream.write_all(&update).unwrap();
}

#[test]
fn auto_security_without_password_connects_to_a_no_auth_server() {
    use_built_helper();
    let (port, server) = spawn_no_auth_server((1, 1), |stream| {
        // Complete the initial update with one Raw rectangle and one pixel.
        push_update(stream, 1, 1, 1);
        Ok(())
    });
    let manager = VncSessionManager::new();
    let session_id = manager
        .create_session_with_id("auto-no-auth".into(), config(port))
        .unwrap();
    let event = wait_for_event(&manager, &session_id, |event| {
        matches!(
            event,
            VncRuntimeEvent::State {
                state: VncSessionState::Connected,
                ..
            } | VncRuntimeEvent::Error { fatal: true, .. }
        )
    });
    manager.close(&session_id).unwrap();
    let server_result = server.join().unwrap();
    assert!(
        matches!(
            event,
            VncRuntimeEvent::State {
                state: VncSessionState::Connected,
                ..
            }
        ),
        "expected a connected session, got {event:?}"
    );
    server_result.unwrap();
}

/// A busy server splits one update into far more rectangles than the old 1024
/// ceiling allowed. Even the maximum u16 rectangle count is legal RFB and
/// must produce every frame instead of failing the session with a protocol error
/// (NyaTerm issue #1045).
#[test]
fn busy_update_with_maximum_rectangle_count_keeps_the_session_alive() {
    const RECTANGLES: u16 = u16::MAX;
    const FRAMEBUFFER: u16 = 256;

    use_built_helper();
    let (port, server) = spawn_no_auth_server((FRAMEBUFFER, FRAMEBUFFER), |stream| {
        push_update(stream, FRAMEBUFFER, FRAMEBUFFER, RECTANGLES);
        Ok(())
    });
    let manager = VncSessionManager::new();
    let session_id = manager
        .create_session_with_id("busy-update".into(), config(port))
        .unwrap();

    // The frame queue is bounded, so drain while the helper is still producing.
    let expected = usize::from(RECTANGLES);
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut frames = 0;
    let mut failure = None;
    let mut connected = 0;
    while Instant::now() < deadline && frames < expected && failure.is_none() {
        let drain = manager.drain(&session_id);
        for event in drain.control {
            if matches!(
                &event,
                VncRuntimeEvent::State {
                    state: VncSessionState::Connected,
                    ..
                }
            ) {
                connected += 1;
            }
            if let VncRuntimeEvent::Error {
                error, fatal: true, ..
            } = event
            {
                failure = Some(error);
            }
        }
        frames += drain.frames.len();
        std::thread::sleep(Duration::from_millis(1));
    }
    let state = manager.state(&session_id);
    manager.close(&session_id).unwrap();
    let server_result = server.join().unwrap();

    assert!(
        failure.is_none(),
        "a busy update must not fail the session: {failure:?}"
    );
    assert_eq!(frames, expected, "every rectangle should reach the app");
    assert_eq!(
        connected, 1,
        "rectangles must not duplicate state notifications"
    );
    assert_eq!(state, Some(VncSessionState::Connected));
    server_result.unwrap();
}

/// Exercise the real negotiation and IPC path: the UI hint must describe the
/// credentials sent to this attempt, not the session's original launch config.
#[test]
fn password_prompt_hint_distinguishes_missing_rejected_and_unsupported_auth() {
    use std::io::{Read as _, Write as _};

    use_built_helper();
    for (security_type, password, password_required) in [
        (2, None, true),
        (129, None, true),
        (16, None, false),
        (2, Some("wrong"), false),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            stream.write_all(b"RFB 003.007\n").unwrap();
            stream.read_exact(&mut [0; 12]).unwrap();
            stream.write_all(&[1, security_type]).unwrap();
            if password.is_some() {
                let mut selected = [0];
                stream.read_exact(&mut selected).unwrap();
                assert_eq!(selected, [2]);
                stream.write_all(&[0; 16]).unwrap();
                stream.read_exact(&mut [0; 16]).unwrap();
                stream.write_all(&1_u32.to_be_bytes()).unwrap();
            }
            // Keep the peer alive until the helper finishes classifying the error.
            assert_eq!(stream.read(&mut [0]).unwrap(), 0);
        });
        let manager = VncSessionManager::new();
        let mut attempt = config(port);
        attempt.password = password.map(|value| value.to_string().into());
        let session_id = manager.create_session(attempt).unwrap();
        let event = wait_for_event(&manager, &session_id, |event| {
            matches!(event, VncRuntimeEvent::Error { fatal: true, .. })
        });
        manager.close(&session_id).unwrap();
        server.join().unwrap();
        let VncRuntimeEvent::Error { error, .. } = event else {
            unreachable!()
        };
        assert_eq!(error.kind, VncErrorKind::Authentication);
        assert_eq!(
            error.password_required,
            password_required,
            "security type {security_type}, supplied credentials: {}",
            password.is_some()
        );
    }
}
