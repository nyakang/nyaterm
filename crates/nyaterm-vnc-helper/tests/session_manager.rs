//! End-to-end coverage of the application-side manager driving a real helper.
//!
//! `VncSessionManager` lives in `nyaterm-remote-desktop`, which cannot depend on
//! this crate, so the round trip is exercised from here where both the manager and
//! the helper binary are available.

use std::net::TcpListener;
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
