use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use nyaterm_native_protocol::spice::settings::{SpiceEndpoint, SpiceSettings};
use nyaterm_remote_desktop::spice::ipc::{decode_control, encode_control};
use nyaterm_remote_desktop::spice::protocol::{
    SPICE_PROTOCOL_VERSION, SpiceControl, SpiceCredentialLifetime, SpiceErrorKind,
    SpiceSessionConfig, SpiceSessionState,
};
use nyaterm_remote_desktop::{read_packet, write_packet};

struct Helper {
    child: Child,
    stdin: ChildStdin,
    events: Receiver<SpiceControl>,
}

impl Helper {
    fn spawn(native: bool) -> Self {
        let mut command = Command::new(
            std::env::var("NYATERM_SPICE_HELPER").expect("set the independent helper executable"),
        );
        if !native {
            command.env(
                "NYATERM_SPICE_RUNTIME",
                std::env::temp_dir().join("nyaterm-missing-spice-runtime"),
            );
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let (sender, events) = mpsc::sync_channel(32);
        std::thread::spawn(move || {
            while let Ok(Some(packet)) = read_packet(&mut stdout) {
                let Ok(message) = decode_control(&packet) else {
                    break;
                };
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            events,
        }
    }

    fn send(&mut self, control: SpiceControl) {
        write_packet(&mut self.stdin, &encode_control(&control).unwrap()).unwrap();
    }

    fn recv(&self) -> SpiceControl {
        self.events
            .recv_timeout(Duration::from_secs(5))
            .expect("helper response deadline")
    }

    fn hello(&mut self) {
        self.send(SpiceControl::Hello {
            version: SPICE_PROTOCOL_VERSION,
        });
        assert_eq!(
            self.recv(),
            SpiceControl::HelloAck {
                version: SPICE_PROTOCOL_VERSION
            }
        );
    }

    fn wait(&mut self) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "helper did not exit within deadline"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn config(port: u16) -> SpiceSessionConfig {
    let mut settings = SpiceSettings::default();
    settings.security.require_tls = false;
    settings.audio_playback = false;
    SpiceSessionConfig {
        session_id: "lifecycle".into(),
        endpoint: SpiceEndpoint {
            extra: Default::default(),
            host: "127.0.0.1".into(),
            port: Some(port),
            tls_port: None,
            proxy: None,
        },
        username: "".into(),
        password: "fixture-ticket".into(),
        settings,
        credential_lifetime: SpiceCredentialLifetime::Temporary,
        tcp_relay: None,
        tls_relay: None,
    }
}

#[test]
#[ignore = "requires independently built native helper"]
fn handshake_ping_and_normal_disconnect_do_not_require_native_libraries() {
    let mut helper = Helper::spawn(false);
    helper.hello();
    helper.send(SpiceControl::Ping { sequence: 42 });
    assert_eq!(helper.recv(), SpiceControl::Pong { sequence: 42 });
    helper.send(SpiceControl::Disconnect);
    assert_eq!(
        helper.recv(),
        SpiceControl::State {
            state: SpiceSessionState::Disconnected
        }
    );
    assert!(helper.wait().success());
}

#[test]
#[ignore = "requires independently built native helper"]
fn missing_runtime_is_a_typed_fatal_error_after_connecting_state() {
    let mut helper = Helper::spawn(false);
    helper.hello();
    helper.send(SpiceControl::Connect {
        config: Box::new(config(5900)),
    });
    assert_eq!(
        helper.recv(),
        SpiceControl::State {
            state: SpiceSessionState::Connecting
        }
    );
    assert_eq!(
        helper.recv(),
        SpiceControl::Error {
            error: SpiceErrorKind::RuntimeMissing,
            fatal: true
        }
    );
    assert!(!helper.wait().success());
}

#[test]
#[ignore = "requires independently built native helper"]
fn wrong_version_and_server_messages_are_rejected() {
    for command in [
        SpiceControl::Hello {
            version: SPICE_PROTOCOL_VERSION + 1,
        },
        SpiceControl::HelloAck {
            version: SPICE_PROTOCOL_VERSION,
        },
        SpiceControl::Pong { sequence: 1 },
    ] {
        let mut helper = Helper::spawn(false);
        helper.send(command);
        assert_eq!(
            helper.recv(),
            SpiceControl::Error {
                error: SpiceErrorKind::Protocol,
                fatal: true
            }
        );
        assert!(!helper.wait().success());
    }
    let mut helper = Helper::spawn(false);
    helper.hello();
    helper.send(SpiceControl::Capabilities {
        capabilities: Default::default(),
    });
    assert_eq!(
        helper.recv(),
        SpiceControl::Error {
            error: SpiceErrorKind::Protocol,
            fatal: true
        }
    );
    assert!(!helper.wait().success());
}

#[test]
#[ignore = "requires independently built native helper"]
fn oversized_or_wrong_direction_packets_fail_without_waiting_for_body() {
    for header in [[1, 255, 255, 255, 127], [2, 1, 0, 0, 0], [255, 0, 0, 0, 0]] {
        let mut helper = Helper::spawn(false);
        helper.stdin.write_all(&header).unwrap();
        helper.stdin.flush().unwrap();
        assert_eq!(
            helper.recv(),
            SpiceControl::Error {
                error: SpiceErrorKind::Protocol,
                fatal: true
            }
        );
        assert!(!helper.wait().success());
    }
}

#[test]
#[ignore = "requires independently built native helper"]
fn native_connect_stall_still_accepts_ping_and_cancellation() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut helper = Helper::spawn(true);
    helper.hello();
    helper.send(SpiceControl::Connect {
        config: Box::new(config(listener.local_addr().unwrap().port())),
    });
    assert_eq!(
        helper.recv(),
        SpiceControl::State {
            state: SpiceSessionState::Connecting
        }
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "native handshake did not arrive");
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("fixture listener: {error}"),
        }
    };
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut magic = [0; 4];
    stream.read_exact(&mut magic).unwrap();
    assert_eq!(&magic, b"REDQ");
    helper.send(SpiceControl::Ping { sequence: 99 });
    assert_eq!(helper.recv(), SpiceControl::Pong { sequence: 99 });
    helper.send(SpiceControl::Disconnect);
    assert_eq!(
        helper.recv(),
        SpiceControl::State {
            state: SpiceSessionState::Disconnected
        }
    );
    assert!(helper.wait().success());
}

#[test]
#[ignore = "requires independently built native helper"]
fn session_creation_rolls_back_every_worker_start_failure() {
    use nyaterm_remote_desktop::spice::session::SpiceSessionManager;
    for stage in 0..7 {
        let manager = SpiceSessionManager::with_presentation_pipeline();
        let _failure = nyaterm_core::runtime::worker::fail_after(stage);
        let started = Instant::now();
        assert!(
            manager
                .create_session_with_helper(
                    config(5900),
                    std::path::Path::new(
                        &std::env::var("NYATERM_SPICE_HELPER")
                            .expect("set the independent helper executable")
                    )
                )
                .is_err()
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(manager.drain("lifecycle", 128).is_empty());
        manager.shutdown();
        assert_eq!(manager.test_live_record_count(), 0);
    }
}

#[test]
#[ignore = "requires independently built native helper"]
#[cfg(feature = "test-support")]
fn spice_shutdown_waits_for_detached_helpers_and_rejects_new_generations() {
    use nyaterm_remote_desktop::spice::session::SpiceSessionManager;
    let manager = SpiceSessionManager::with_presentation_pipeline();
    let helper = std::path::PathBuf::from(
        std::env::var("NYATERM_SPICE_FIXTURE").expect("set the independent fixture executable"),
    );
    manager
        .create_session_with_helper(config(5900), &helper)
        .unwrap();
    assert_eq!(manager.test_live_record_count(), 1);
    manager.close_detached("lifecycle").unwrap();
    assert!(!manager.contains("lifecycle"));
    let started = Instant::now();
    manager.shutdown();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(manager.test_live_record_count(), 0);
    assert!(
        manager
            .create_session_with_helper(config(5900), &helper)
            .is_err()
    );
    assert_eq!(manager.test_live_record_count(), 0);
}

#[test]
#[ignore = "requires independently built native helper"]
fn manager_reports_a_helper_failure_and_reaps_on_shutdown() {
    use nyaterm_remote_desktop::spice::session::{SpiceEvent, SpiceSessionManager};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let manager = SpiceSessionManager::new();
    manager
        .create_session_with_helper(
            config(port),
            std::path::Path::new(
                &std::env::var("NYATERM_SPICE_HELPER")
                    .expect("set the independent helper executable"),
            ),
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let failed = manager.drain("lifecycle", 128).into_iter().any(|event| {
            matches!(
                event,
                SpiceEvent::Control(SpiceControl::Error { fatal: true, .. })
            )
        });
        if failed {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "helper failure must reach the manager"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let started = Instant::now();
    manager.shutdown();
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
#[ignore = "requires independently built native helper"]
#[cfg(feature = "test-support")]
fn supervisor_reports_crash_and_kills_a_hung_helper() {
    use nyaterm_remote_desktop::spice::session::{SpiceEvent, SpiceSessionManager};
    for mode in ["crash", "hang"] {
        let directory =
            std::env::temp_dir().join(format!("spice-lifecycle-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let filename = if cfg!(windows) {
            format!("spice-{mode}.exe")
        } else {
            format!("spice-{mode}")
        };
        let helper = directory.join(filename);
        std::fs::copy(
            std::env::var("NYATERM_SPICE_FIXTURE").expect("set the independent fixture executable"),
            &helper,
        )
        .unwrap();
        let manager = SpiceSessionManager::new();
        manager
            .create_session_with_helper(config(5900), &helper)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut reported = false;
        loop {
            for event in manager.drain("lifecycle", 128) {
                if let SpiceEvent::Control(SpiceControl::Error { error, fatal: true }) = event {
                    assert_eq!(
                        error,
                        if mode == "hang" {
                            SpiceErrorKind::Timeout
                        } else {
                            SpiceErrorKind::HelperCrash
                        }
                    );
                    reported = true;
                }
            }
            if reported && !manager.test_helper_running("lifecycle") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "supervisor failed to report and reap {mode}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        manager.shutdown();
        std::fs::remove_file(helper).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
