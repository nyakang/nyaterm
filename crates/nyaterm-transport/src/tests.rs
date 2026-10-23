use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use base64::Engine;
#[cfg(unix)]
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tokio::sync::mpsc as tokio_mpsc;

use super::{
    DO, DockerService, ForwardedTcpIpDispatch, IAC, LocalSessionConfig, OPT_SUPPRESS_GO_AHEAD,
    PrimarySessionGate, QueuedTransportWriter, RemoteGpuService, RemoteNpuService,
    RemoteStatsService, SESSION_EVENT_QUEUE_OUTPUT_EVENT_LIMIT, SESSION_EVENT_QUEUE_OUTPUT_LIMIT,
    SerialSessionConfig, SessionError, SessionEvent, SessionEventConsumerId, SessionEventQueue,
    SessionManager, SftpService, SftpSettings, SshAlgorithmListKind, SshAlgorithmMode,
    SshAlgorithmPreferences, SshAlgorithmRisk, SshAlgorithmValidationError, SshKeyAuthConfig,
    SshProxyConfig, SshPtyDimensions, SshSessionConfig, SshSessionProfile, TelnetSessionConfig,
    WILL, cipher, defaults_from_preferred, drain_deferred_ssh_open_commands, expand_proxy_command,
    forwarded_tcpip_sender_for, has_password_prompt, has_username_prompt,
    is_process_list_unsupported, kex, local_pty_size, mac, normalize_process_signal,
    parse_process_output, register_x11_sender, remap_del_to_bs, resolve_preferred_algorithms,
    run_local_command, ssh_client_config, ssh_host_identifier, supported_ssh_algorithms,
    unregister_x11_sender, validate_ssh_algorithm_preferences,
};
#[cfg(unix)]
use super::{EnvironmentSnapshot, EnvironmentValue};

fn wait_for_queue_state(mut predicate: impl FnMut() -> bool, description: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {description}"
        );
        std::thread::yield_now();
    }
}

/// A push must hand the event straight to a parked consumer. Before the
/// queue carried a condvar the bridge could only poll, so every event paid
/// an arbitrary slice of the poll interval before anyone looked at it.
#[test]
fn blocking_drain_wakes_on_push_rather_than_timing_out() {
    let queue = SessionEventQueue::new();
    let producer = queue.clone();
    let (ready_tx, ready_rx) = mpsc::sync_channel(0);

    let waiter = std::thread::spawn(move || {
        let _ = ready_tx.send(());
        let started = Instant::now();
        let drain = producer.drain_blocking_with_output_budget(
            16,
            Some(64 * 1024),
            Duration::from_secs(30),
        );
        (drain, started.elapsed())
    });

    // Let the waiter reach its park before producing.
    ready_rx.recv().expect("waiter started");
    std::thread::sleep(Duration::from_millis(20));
    queue.push(SessionEvent::Output {
        session_id: "s1".to_string(),
        data: b"hi".to_vec(),
    });

    let (drain, waited) = waiter.join().expect("waiter finished");
    assert_eq!(drain.events.len(), 1);
    assert!(
        waited < Duration::from_secs(5),
        "the push should have woken the park, not the 30s timeout (waited {waited:?})"
    );
}

/// An event already queued must not cost a park at all.
#[test]
fn blocking_drain_returns_queued_events_immediately() {
    let queue = SessionEventQueue::new();
    queue.push(SessionEvent::Output {
        session_id: "s1".to_string(),
        data: b"hi".to_vec(),
    });

    let started = Instant::now();
    let drain =
        queue.drain_blocking_with_output_budget(16, Some(64 * 1024), Duration::from_secs(30));

    assert_eq!(drain.events.len(), 1);
    assert!(started.elapsed() < Duration::from_secs(5));
}

/// A fully idle queue still has to return so the consumer can re-check its
/// stop flag.
#[test]
fn blocking_drain_gives_up_at_the_timeout() {
    let queue = SessionEventQueue::new();
    let drain =
        queue.drain_blocking_with_output_budget(16, Some(64 * 1024), Duration::from_millis(20));
    assert!(drain.events.is_empty());
}

struct GatedWriter {
    started_tx: mpsc::SyncSender<()>,
    release_rx: mpsc::Receiver<()>,
    output: Arc<Mutex<Vec<u8>>>,
}

impl Write for GatedWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let _ = self.started_tx.send(());
        let _ = self.release_rx.recv();
        self.output
            .lock()
            .expect("output lock")
            .extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct WriteCapture(Arc<Mutex<Vec<Vec<u8>>>>);

impl Write for WriteCapture {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("write capture lock")
            .push(data.to_vec());
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn queued_transport_writer_returns_before_blocking_write_completes() {
    let (started_tx, started_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let output = Arc::new(Mutex::new(Vec::new()));
    let mut writer = QueuedTransportWriter::spawn(
        "queued".to_string(),
        GatedWriter {
            started_tx,
            release_rx,
            output: output.clone(),
        },
        crate::WriterFlushPolicy::AfterWrite,
        SessionEventQueue::new(),
    )
    .unwrap();

    writer.write(b"input".to_vec()).expect("queue input");
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("background write should start");
    assert!(output.lock().expect("output lock").is_empty());

    release_tx.send(()).expect("release writer");
    writer.close();
    assert_eq!(*output.lock().expect("output lock"), b"input");
}

#[test]
fn queued_transport_writer_preserves_character_at_a_time_mode() {
    let writes = Arc::new(Mutex::new(Vec::new()));
    let mut writer = QueuedTransportWriter::spawn(
        "character-mode".to_string(),
        WriteCapture(writes.clone()),
        crate::WriterFlushPolicy::EachByte,
        SessionEventQueue::new(),
    )
    .unwrap();

    writer.write(b"abc".to_vec()).expect("queue input");
    let deadline = Instant::now() + Duration::from_secs(1);
    while writes.lock().unwrap().len() != 3 {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    writer.close();

    assert_eq!(
        *writes.lock().expect("write capture lock"),
        vec![vec![b'a'], vec![b'b'], vec![b'c']]
    );
}

#[test]
fn local_session_echoes_output() {
    if cfg!(target_os = "windows") {
        return;
    }

    let manager = SessionManager::new();
    let info = manager
        .create_local_session(LocalSessionConfig {
            name: "test".to_string(),
            shell_path: Some("/bin/sh".to_string()),
            shell_args: Vec::new(),
            working_dir: None,
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
            ..Default::default()
        })
        .expect("local session");

    manager
        .write(&info.id, b"printf nyaterm-transport-ready\\n\n")
        .expect("write");

    let output = collect_output(&manager, &info.id, Duration::from_secs(3));
    manager.close(&info.id).expect("close");

    assert!(
        String::from_utf8_lossy(&output).contains("nyaterm-transport-ready"),
        "output was: {}",
        String::from_utf8_lossy(&output)
    );
}

#[cfg(unix)]
#[test]
fn local_command_receives_the_cached_shell_environment() {
    let snapshot = EnvironmentSnapshot::from_test(
        HashMap::from([(
            "NYATERM_TEST_LOCAL_ENV".to_string(),
            EnvironmentValue::new("from-shell-cache".to_string()),
        )]),
        false,
    );
    let mut command = CommandBuilder::new("/bin/sh");

    super::configure_environment(&mut command, Some(&snapshot));

    assert_eq!(
        command
            .get_env("NYATERM_TEST_LOCAL_ENV")
            .and_then(|value| value.to_str()),
        Some("from-shell-cache")
    );
}

#[cfg(windows)]
#[test]
fn windows_local_session_close_releases_conpty_reader() {
    let manager = SessionManager::new();
    let info = manager
        .create_local_session(LocalSessionConfig {
            name: "windows-close-test".to_string(),
            shell_path: Some(super::default_local_shell_path()),
            cols: 80,
            rows: 24,
            ..Default::default()
        })
        .expect("windows local session");
    let session_id = info.id;
    let (closed_tx, closed_rx) = mpsc::sync_channel(1);
    let closer = std::thread::spawn(move || {
        let result = manager
            .close(&session_id)
            .map_err(|error| error.to_string());
        let _ = closed_tx.send(result);
    });

    closed_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("closing a Windows local session must not block on the ConPTY reader")
        .expect("close Windows local session");
    closer.join().expect("Windows local session closer");
}

#[cfg(windows)]
#[test]
#[ignore = "requires a working WSL installation; manual transport smoke test"]
fn wsl_local_session_reports_cwd_through_injected_integration() {
    let manager = SessionManager::new();
    let info = manager
        .create_local_session(LocalSessionConfig {
            name: "wsl-cwd-smoke".to_string(),
            shell_path: Some("wsl.exe".to_string()),
            cols: 120,
            rows: 30,
            ..Default::default()
        })
        .expect("wsl session");

    let deadline = Instant::now() + Duration::from_secs(20);
    std::thread::sleep(Duration::from_secs(10));
    manager
        .write(&info.id, b"echo NYA-PROBE-OK\r\n")
        .expect("probe write");
    let mut visible = Vec::new();
    let mut cwds = Vec::new();
    while Instant::now() < deadline && cwds.is_empty() {
        let before = visible.len();
        for event in manager.drain_events(32).expect("events").events {
            match event {
                SessionEvent::Output { data, .. } => visible.extend(data),
                SessionEvent::CwdChanged { cwd, .. } => cwds.push(cwd),
                SessionEvent::Error { message, .. } => panic!("session error: {message}"),
                _ => {}
            }
        }
        if visible[before..]
            .windows(4)
            .any(|window| window == b"\x1b[6n")
        {
            manager
                .write(&info.id, b"\x1b[1;1R")
                .expect("cursor report");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let tail_start = visible.len().saturating_sub(300);
    let tail = String::from_utf8_lossy(&visible[tail_start..]).to_string();
    let probe_seen = String::from_utf8_lossy(&visible).contains("NYA-PROBE-OK");
    manager.close(&info.id).expect("close");
    assert!(
        !cwds.is_empty(),
        "no cwd reported (probe visible: {probe_seen}); visible output: {tail:?}"
    );
}

#[test]
fn local_session_info_preserves_working_dir() {
    if cfg!(target_os = "windows") {
        return;
    }

    let dir_guard = nyaterm_core::test_support::TestTempDir::new("nyaterm-local");
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(&dir).expect("temp dir");
    let manager = SessionManager::new();
    let info = manager
        .create_local_session(LocalSessionConfig {
            name: "cwd-test".to_string(),
            shell_path: Some("/bin/sh".to_string()),
            shell_args: Vec::new(),
            working_dir: Some(dir.clone()),
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
            ..Default::default()
        })
        .expect("local session");
    let sessions = manager.list_sessions().expect("sessions");
    manager.close(&info.id).expect("close");
    std::fs::remove_dir_all(&dir).ok();

    assert_eq!(sessions[0].working_dir.as_ref(), Some(&dir));
}

#[test]
fn local_background_command_uses_working_dir_and_exit_code() {
    if cfg!(target_os = "windows") {
        return;
    }

    let dir_guard = nyaterm_core::test_support::TestTempDir::new("nyaterm-local-bg");
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(&dir).expect("temp dir");
    let output = run_local_command(
        "printf ready > marker.txt; printf output; exit 7",
        Some(dir.clone()),
        Duration::from_secs(3),
    )
    .expect("local command");
    let marker = std::fs::read_to_string(dir.join("marker.txt")).expect("marker");
    std::fs::remove_dir_all(&dir).ok();

    assert_eq!(marker, "ready");
    assert_eq!(output.stdout, "output");
    assert_eq!(output.exit_status, Some(7));
}

#[test]
fn resize_updates_session_info() {
    if cfg!(target_os = "windows") {
        return;
    }

    let manager = SessionManager::new();
    let info = manager
        .create_local_session(LocalSessionConfig {
            shell_path: Some("/bin/sh".to_string()),
            ..Default::default()
        })
        .expect("local session");
    manager.resize(&info.id, 120, 32).expect("resize");
    let sessions = manager.list_sessions().expect("sessions");
    manager.close(&info.id).expect("close");

    assert_eq!(sessions[0].cols, 120);
    assert_eq!(sessions[0].rows, 32);
}

#[test]
fn raw_tcp_session_echoes_output() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let port = listener.local_addr().expect("addr").port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("timeout");
        let mut buffer = [0_u8; 64];
        let read = stream.read(&mut buffer).expect("read");
        stream.write_all(b"echo:").expect("prefix");
        stream.write_all(&buffer[..read]).expect("echo");
    });

    let manager = SessionManager::new();
    let info = manager
        .create_telnet_session(TelnetSessionConfig {
            name: "raw".to_string(),
            host: "127.0.0.1".to_string(),
            port,
            raw_tcp: true,
            ..Default::default()
        })
        .expect("raw tcp");

    manager.write(&info.id, b"hello").expect("write");
    let output = collect_output_until(&manager, &info.id, "echo:hello", Duration::from_secs(3));
    manager.close(&info.id).expect("close");
    server.join().expect("server");

    assert!(String::from_utf8_lossy(&output).contains("echo:hello"));
}

#[test]
fn telnet_session_negotiates_and_strips_iac() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let port = listener.local_addr().expect("addr").port();
    let (tx, rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("timeout");
        stream
            .write_all(&[IAC, WILL, OPT_SUPPRESS_GO_AHEAD, b'o', b'k'])
            .expect("write greeting");

        let mut seen = Vec::new();
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(3) {
            let mut buffer = [0_u8; 64];
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    seen.extend_from_slice(&buffer[..read]);
                    if seen
                        .windows(3)
                        .any(|window| window == [IAC, DO, OPT_SUPPRESS_GO_AHEAD])
                    {
                        break;
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    continue;
                }
                Err(error) => panic!("server read failed: {error}"),
            }
        }
        tx.send(seen).expect("send seen");
    });

    let manager = SessionManager::new();
    let info = manager
        .create_telnet_session(TelnetSessionConfig {
            name: "telnet".to_string(),
            host: "127.0.0.1".to_string(),
            port,
            raw_tcp: false,
            send_sga: true,
            ..Default::default()
        })
        .expect("telnet");

    let output = collect_output_until(&manager, &info.id, "ok", Duration::from_secs(3));
    manager.close(&info.id).expect("close");
    server.join().expect("server");
    let seen = rx.recv().expect("seen");

    assert_eq!(String::from_utf8_lossy(&output), "ok");
    assert!(
        seen.windows(3)
            .any(|window| { window == [IAC, DO, OPT_SUPPRESS_GO_AHEAD] })
    );
}

#[test]
fn serial_invalid_port_reports_open_error() {
    let manager = SessionManager::new();
    let port_name = if cfg!(target_os = "windows") {
        r"\\.\NyaTermMissingPort".to_string()
    } else {
        "/dev/nyaterm-missing-port".to_string()
    };

    let error = manager
        .create_serial_session(SerialSessionConfig {
            port_name: port_name.clone(),
            ..Default::default()
        })
        .expect_err("invalid port should not open");

    match error {
        SessionError::OpenSerial {
            port_name: actual, ..
        } => assert_eq!(actual, port_name),
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn serial_backspace_mode_remaps_delete_to_ctrl_h() {
    assert_eq!(remap_del_to_bs(b"a\x7fb"), b"a\x08b");
}

#[test]
fn telnet_local_line_edit_buffers_until_enter_and_echoes_locally() {
    let config = TelnetSessionConfig {
        local_echo: true,
        local_line_edit: true,
        ..Default::default()
    };
    let mut input = crate::telnet_input::TelnetInputState::default();

    let (send, echo) = input.prepare(b"hel", &config);
    assert!(send.is_empty());
    assert_eq!(echo, b"hel");

    let (send, echo) = input.prepare(b"lo\x08p\r", &config);
    assert_eq!(send, b"hellp\r");
    assert_eq!(echo, b"lo\x08 \x08p\r\n");
    assert_eq!(
        input.prepare(b"\r", &config),
        (b"\r".to_vec(), b"\r\n".to_vec())
    );
}

fn telnet_input_loopback(
    mut config: TelnetSessionConfig,
) -> (SessionManager, super::SessionInfo, std::net::TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    config.host = "127.0.0.1".into();
    config.port = listener.local_addr().unwrap().port();
    config.send_naws = false;
    config.send_sga = false;
    config.keep_alive_interval_secs = 0;
    config.auto_login.enabled = false;
    let manager = SessionManager::new();
    let info = manager.create_telnet_session(config).unwrap();
    let (peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    (manager, info, peer)
}

#[test]
fn telnet_text_input_sends_configured_newlines_and_echoes_independently_in_both_transports() {
    for raw_tcp in [false, true] {
        for local_line_edit in [false, true] {
            for local_echo in [false, true] {
                for (enter_mode, newline) in [
                    (crate::TelnetEnterMode::Cr, b"\r".as_slice()),
                    (crate::TelnetEnterMode::Lf, b"\n".as_slice()),
                    (crate::TelnetEnterMode::Crlf, b"\r\n".as_slice()),
                ] {
                    let (manager, info, mut peer) = telnet_input_loopback(TelnetSessionConfig {
                        raw_tcp,
                        local_line_edit,
                        local_echo,
                        enter_mode,
                        ..Default::default()
                    });
                    for enter in [b"\r".as_slice(), b"\n".as_slice(), b"\r\n".as_slice()] {
                        manager.write(&info.id, b"abc").unwrap();
                        if local_line_edit {
                            peer.set_read_timeout(Some(Duration::from_millis(20)))
                                .unwrap();
                            let error = peer.read(&mut [0; 1]).unwrap_err();
                            assert!(matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ));
                            peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                        } else {
                            let mut text = [0; 3];
                            peer.read_exact(&mut text).unwrap();
                            assert_eq!(&text, b"abc");
                        }
                        manager.write(&info.id, enter).unwrap();
                        let mut expected = if local_line_edit {
                            b"abc".to_vec()
                        } else {
                            vec![]
                        };
                        expected.extend_from_slice(newline);
                        let mut received = vec![0; expected.len()];
                        peer.read_exact(&mut received).unwrap();
                        assert_eq!(
                            received, expected,
                            "raw={raw_tcp}, edit={local_line_edit}, mode={enter_mode:?}"
                        );
                    }
                    peer.set_read_timeout(Some(Duration::from_millis(20)))
                        .unwrap();
                    assert!(
                        peer.read(&mut [0; 1]).is_err(),
                        "no duplicate newline or protocol bytes"
                    );
                    let echo: Vec<u8> = manager
                        .event_queue
                        .drain(100)
                        .events
                        .into_iter()
                        .flat_map(|event| {
                            if let SessionEvent::Output { data, .. } = event {
                                data
                            } else {
                                vec![]
                            }
                        })
                        .collect();
                    assert_eq!(
                        echo,
                        if local_echo {
                            b"abc\r\n".repeat(3)
                        } else {
                            vec![]
                        }
                    );
                    manager.close(&info.id).unwrap();
                }
            }
        }
    }
}

#[test]
fn telnet_rejected_submission_preserves_line_escape_state_and_echo_until_retry() {
    let (manager, info, _peer) = telnet_input_loopback(TelnetSessionConfig {
        local_echo: true,
        local_line_edit: true,
        enter_mode: crate::TelnetEnterMode::Crlf,
        ..Default::default()
    });
    manager.write(&info.id, b"abc").unwrap();
    manager.event_queue.drain(100);

    // An unconsumed mailbox makes backpressure deterministic. Leave room for
    // the original three bytes, but not for the edited and expanded payload.
    let mailbox = crate::writer_mailbox::WriterMailbox::default();
    mailbox
        .send(crate::TransportWriterCommand::Write(vec![
            0;
            crate::writer_mailbox::BYTE_LIMIT
                - 3
        ]))
        .unwrap();
    let original_mailbox = {
        let mut sessions = manager.sessions.lock().unwrap();
        let Some(crate::ManagedSession::Tcp(session)) = sessions.get_mut(&info.id) else {
            panic!("TCP session");
        };
        std::mem::replace(&mut session.writer.command_tx, mailbox.clone())
    };
    assert!(manager.write(&info.id, b"\r\x1b[").is_err());
    assert!(manager.event_queue.drain(100).events.is_empty());
    drop(mailbox.receive(None).unwrap().unwrap());

    manager.write(&info.id, b"\r").unwrap();
    let (command, permit) = mailbox.receive(None).unwrap().unwrap();
    assert!(matches!(command, crate::TransportWriterCommand::Write(data) if data == b"abc\r\n"));
    drop(permit);
    manager.write(&info.id, b"z\r").unwrap();
    let (command, permit) = mailbox.receive(None).unwrap().unwrap();
    assert!(matches!(command, crate::TransportWriterCommand::Write(data) if data == b"z\r\n"));
    drop(permit);
    let echo: Vec<u8> = manager
        .event_queue
        .drain(100)
        .events
        .into_iter()
        .flat_map(|event| {
            if let SessionEvent::Output { data, .. } = event {
                data
            } else {
                vec![]
            }
        })
        .collect();
    assert_eq!(echo, b"\r\nz\r\n");
    {
        let mut sessions = manager.sessions.lock().unwrap();
        let Some(crate::ManagedSession::Tcp(session)) = sessions.get_mut(&info.id) else {
            panic!("TCP session");
        };
        session.writer.command_tx = original_mailbox;
    }
    manager.close(&info.id).unwrap();
}

#[test]
fn telnet_prompt_detection_handles_credentials_and_avoids_last_login() {
    assert!(has_username_prompt("router login: "));
    assert!(has_username_prompt("Username:"));
    assert!(!has_username_prompt("Last login: Wed Jul 15"));
    assert!(has_password_prompt("Password: "));
    assert!(has_password_prompt("输入密码："));
}

#[test]
fn telnet_auto_login_sends_username_and_password_prompts() {
    let config = TelnetSessionConfig {
        username: "operator".to_string(),
        password: Some("secret".to_string().into()),
        ..Default::default()
    };
    let mut state = super::TelnetAutoLoginState::new(&config).expect("auto login state");

    let username_payload = state
        .handle_visible_output(b"router login: ", &config)
        .into_iter()
        .find_map(|action| match action {
            super::TelnetAutoLoginAction::Send(payload) => Some(payload),
            _ => None,
        })
        .expect("username payload");
    let password_payload = state
        .handle_visible_output(b"Password: ", &config)
        .into_iter()
        .find_map(|action| match action {
            super::TelnetAutoLoginAction::Send(payload) => Some(payload),
            _ => None,
        })
        .expect("password payload");

    assert_eq!(username_payload, b"operator\r");
    assert_eq!(password_payload, b"secret\r");
    assert!(
        state
            .handle_visible_output(b"Password: ", &config)
            .is_empty()
    );
}

fn telnet_send_payloads(actions: Vec<super::TelnetAutoLoginAction>) -> Vec<Vec<u8>> {
    actions
        .into_iter()
        .filter_map(|action| match action {
            super::TelnetAutoLoginAction::Send(payload) => Some(payload),
            _ => None,
        })
        .collect()
}

#[test]
fn telnet_auto_login_handles_split_and_chinese_prompts() {
    let config = TelnetSessionConfig {
        username: "admin".to_string(),
        password: Some("sekret".to_string().into()),
        ..Default::default()
    };
    let mut state = super::TelnetAutoLoginState::new(&config).expect("auto login state");

    assert!(telnet_send_payloads(state.handle_visible_output(b"User", &config)).is_empty());
    assert_eq!(
        telnet_send_payloads(state.handle_visible_output(b"name: ", &config)),
        vec![b"admin\r".to_vec()]
    );
    assert_eq!(
        telnet_send_payloads(state.handle_visible_output("请输入密码：".as_bytes(), &config)),
        vec![b"sekret\r".to_vec()]
    );
}

#[test]
fn telnet_auto_login_wakes_prompt_and_ignores_last_login() {
    let config = TelnetSessionConfig {
        username: "admin".to_string(),
        password: Some("sekret".to_string().into()),
        ..Default::default()
    };
    let mut state = super::TelnetAutoLoginState::new(&config).expect("auto login state");

    assert_eq!(
        telnet_send_payloads(state.handle_visible_output(b"Press Enter to continue", &config)),
        vec![b"\r".to_vec()]
    );
    assert!(
        telnet_send_payloads(
            state.handle_visible_output(b"Last login: Wed Jul 15 10:00:00\r\n", &config)
        )
        .is_empty()
    );
    assert_eq!(
        telnet_send_payloads(state.handle_visible_output(b"router login: ", &config)),
        vec![b"admin\r".to_vec()]
    );
}

#[test]
fn telnet_auto_login_retries_failure_and_disables_after_manual_input() {
    let config = TelnetSessionConfig {
        username: "admin".to_string(),
        password: Some("wrong".to_string().into()),
        auto_login: super::TelnetAutoLoginConfig {
            max_retries: 1,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut state = super::TelnetAutoLoginState::new(&config).expect("auto login state");

    assert_eq!(
        telnet_send_payloads(state.handle_visible_output(b"login: ", &config)),
        vec![b"admin\r".to_vec()]
    );
    assert_eq!(
        telnet_send_payloads(state.handle_visible_output(b"Password", &config)),
        vec![b"wrong\r".to_vec()]
    );
    assert!(
        state
            .handle_visible_output(b"Login incorrect\r\n", &config)
            .is_empty()
    );
    assert_eq!(
        telnet_send_payloads(state.handle_visible_output(b"login: ", &config)),
        vec![b"admin\r".to_vec()]
    );
    assert!(matches!(
        state.handle_user_input(false),
        Some(super::TelnetAutoLoginAction::Disable)
    ));
    assert!(
        state
            .handle_visible_output(b"Password: ", &config)
            .is_empty()
    );
}

#[test]
fn sftp_service_rejects_operations_when_disabled() {
    let service = SftpService::new(SshSessionConfig {
        sftp: SftpSettings {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    });

    let error = service.list_dir("/").expect_err("SFTP disabled");

    assert!(error.to_string().contains("SFTP is disabled"));
}

#[test]
fn sftp_service_rejects_network_device_profile_even_when_saved_enabled() {
    let service = SftpService::new(SshSessionConfig {
        profile: SshSessionProfile::NetworkDevice,
        sftp: SftpSettings {
            enabled: true,
            ..Default::default()
        },
        ..Default::default()
    });

    let error = service
        .list_dir("/")
        .expect_err("network device SFTP must be disabled before connecting");

    assert!(error.to_string().contains("SFTP is disabled"));
}

#[test]
fn x11_registry_replaces_stale_sender_and_rejects_concurrent_owner() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let registry = Arc::new(tokio::sync::Mutex::new(None));
        let (first_tx, first_rx) = tokio_mpsc::unbounded_channel();

        register_x11_sender(&registry, "session-a", first_tx)
            .await
            .expect("first registration");
        let (same_owner_tx, _same_owner_rx) = tokio_mpsc::unbounded_channel();
        let error = register_x11_sender(&registry, "session-a", same_owner_tx)
            .await
            .expect_err("same live owner should be rejected");
        assert!(error.to_string().contains("already active"));

        let (second_tx, _second_rx) = tokio_mpsc::unbounded_channel();
        let error = register_x11_sender(&registry, "session-b", second_tx)
            .await
            .expect_err("second live owner should be rejected");
        assert!(error.to_string().contains("already active"));

        drop(first_rx);
        let (replacement_tx, replacement_rx) = tokio_mpsc::unbounded_channel();
        register_x11_sender(&registry, "session-b", replacement_tx)
            .await
            .expect("stale sender should be replaced");

        unregister_x11_sender(&registry, "session-a").await;
        assert!(
            registry
                .lock()
                .await
                .as_ref()
                .is_some_and(|registration| registration.session_id == "session-b")
        );
        unregister_x11_sender(&registry, "session-b").await;
        assert!(registry.lock().await.is_none());
        drop(replacement_rx);
    });
}

/// `sshd` hands the PAM login banner to the first session channel on a
/// connection and clears its buffer right afterwards, so an auxiliary channel
/// that opens before the terminal's shell silently swallows the whole MOTD.
/// Session-scoped multiplexing put Stats/Docker/SFTP in that race -- with a
/// deferred PTY the shell channel is not opened until the first resize arrives
/// -- so the gate has to hold them until the shell has its channel.
#[test]
fn primary_session_gate_holds_auxiliary_channels_until_the_shell_claims_the_slot() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let gate = Arc::new(PrimarySessionGate::new());
        tokio::time::timeout(Duration::from_millis(250), gate.wait())
            .await
            .expect("a gate no interactive session claimed must never block");

        let claim = PrimarySessionGate::claim(&gate).expect("first claim");
        assert!(
            PrimarySessionGate::claim(&gate).is_none(),
            "only the connection's first interactive session may own the slot"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(100), gate.wait())
                .await
                .is_err(),
            "auxiliary channels must wait while the shell still holds the claim"
        );

        drop(claim);
        tokio::time::timeout(Duration::from_millis(250), gate.wait())
            .await
            .expect("releasing the claim must let queued channels through");
        tokio::time::timeout(Duration::from_millis(250), gate.wait())
            .await
            .expect("a released gate stays open for later channels");
    });
}

/// A session abandoned during the deferred-PTY wait drops its claim without ever
/// opening a channel. Nothing else would release the gate in that case, so the
/// drop -- not the shell request -- has to be what opens it.
#[test]
fn primary_session_gate_opens_when_a_pending_session_is_abandoned() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let gate = Arc::new(PrimarySessionGate::new());
        let claim = PrimarySessionGate::claim(&gate).expect("claim");
        let waiter = {
            let gate = gate.clone();
            tokio::spawn(async move { gate.wait().await })
        };
        drop(claim);
        tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("waiter must be released once the claim is dropped")
            .expect("waiter task");
    });
}

#[test]
fn remote_services_keep_dedicated_fallback_constructors() {
    let config = SshSessionConfig {
        name: "fallback".to_string(),
        host: "example.test".to_string(),
        username: "tester".to_string(),
        ..Default::default()
    };

    let stats = format!("{:?}", RemoteStatsService::new(config.clone()));
    let gpu = format!("{:?}", RemoteGpuService::new(config.clone()));
    let npu = format!("{:?}", RemoteNpuService::new(config.clone()));
    let docker = format!("{:?}", DockerService::new(config));

    for debug in [stats, gpu, npu, docker] {
        assert!(debug.contains("multiplex: None"));
    }
}

#[test]
fn ssh_refused_connection_reports_create_error() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);

    let manager = SessionManager::new();
    let error = manager
        .create_ssh_session(SshSessionConfig {
            name: "ssh".to_string(),
            host: "127.0.0.1".to_string(),
            port,
            username: "tester".to_string(),
            password: Some("secret".to_string().into()),
            ..Default::default()
        })
        .expect_err("closed port should not open");

    match error {
        SessionError::CreateSsh { addr, .. } => assert_eq!(addr, format!("127.0.0.1:{port}")),
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn ssh_host_identifier_uses_openssh_port_format() {
    assert_eq!(ssh_host_identifier("example.com", 22), "example.com");
    assert_eq!(
        ssh_host_identifier("example.com", 2222),
        "[example.com]:2222"
    );
}

#[test]
fn ssh_session_host_key_alias_is_preserved_by_defaults_and_debug() {
    let config = SshSessionConfig {
        host: "real.example".into(),
        port: 2222,
        host_key_alias: Some("stable-alias".into()),
        ..Default::default()
    };
    assert_eq!(config.host_key_alias.as_deref(), Some("stable-alias"));
    assert!(format!("{config:?}").contains("stable-alias"));
}

#[test]
fn ssh_shell_integration_script_emits_osc7_and_ready_marker() {
    let ready = super::build_ssh_ready_marker("session-1");
    let script = super::ssh_shell_injection_script(
        super::ShellKind::Bash,
        &ready,
        super::ShellIntegrationMode::Full,
    )
    .expect("bash script");

    assert!(script.contains("printf '\\033]7;file://%s%s\\007'"));
    assert!(script.contains("NyaTermCommand"));
    assert!(script.contains("NyaTermReady:session-1"));
}

#[test]
fn ssh_shell_scripts_emit_complete_standard_osc133_lifecycle() {
    let ready = super::build_ssh_ready_marker("session-lifecycle");
    for shell in [
        super::ShellKind::Bash,
        super::ShellKind::Zsh,
        super::ShellKind::Fish,
    ] {
        let script = super::persistent_script(shell).expect("persistent integration script");
        assert!(script.contains("133;A"), "missing A for {shell:?}");
        assert!(script.contains("133;B"), "missing B for {shell:?}");
        assert!(script.contains("133;C"), "missing C for {shell:?}");
        assert!(script.contains("133;D"), "missing D for {shell:?}");
        let inline =
            super::ssh_shell_injection_script(shell, &ready, super::ShellIntegrationMode::Full)
                .expect("inline script");
        assert!(inline.contains("NyaTermReady:session-lifecycle"));
    }

    let bash = super::persistent_script(super::ShellKind::Bash).expect("bash script");
    assert!(bash.contains("NYATERM_BASH_HOOKS_READY:-"));
    let zsh = super::persistent_script(super::ShellKind::Zsh).expect("zsh script");
    assert!(zsh.matches("NYATERM_ZSH_HOOKS_READY:-").count() >= 4);
    let fish = super::persistent_script(super::ShellKind::Fish).expect("fish script");
    assert!(fish.matches("NYATERM_FISH_HOOKS_READY").count() >= 4);
}

#[test]
fn cwd_only_shell_scripts_omit_semantic_markers_and_bash_debug_trap() {
    let ready = super::build_ssh_ready_marker("session-cwd-only");
    for shell in [
        super::ShellKind::Bash,
        super::ShellKind::Zsh,
        super::ShellKind::Fish,
    ] {
        let script =
            super::ssh_shell_injection_script(shell, &ready, super::ShellIntegrationMode::CwdOnly)
                .expect("cwd-only script");
        assert!(
            !script.contains("133;"),
            "unexpected semantic marker for {shell:?}"
        );
        assert!(script.contains("NyaTermCommand"));
        assert!(script.contains("NyaTermReady:session-cwd-only"));
        if shell == super::ShellKind::Bash {
            assert!(!script.contains("DEBUG"));
        }
    }
}

/// Feeds a pty master in small whole-line batches instead of one large write.
///
/// Linux's line discipline applies back-pressure to a master write once its buffer
/// fills, so a multi-KiB burst is delivered intact. Darwin compares the raw plus
/// canonical queue against `TTYHOG` (about a kilobyte) and *discards* the excess
/// instead, silently truncating the tail. Batching by whole lines keeps canonical
/// mode's per-line assembly intact while leaving the reader time to drain.
fn write_pty_paced(writer: &mut impl Write, payload: &str) {
    const BATCH_LIMIT: usize = 256;

    let mut batch = String::new();
    let mut flush_batch = |batch: &mut String, pause: bool| {
        if batch.is_empty() {
            return;
        }
        writer
            .write_all(batch.as_bytes())
            .expect("write paced pty batch");
        writer.flush().expect("flush paced pty batch");
        batch.clear();
        if pause {
            std::thread::sleep(Duration::from_millis(5));
        }
    };

    for line in payload.split_inclusive('\n') {
        if !batch.is_empty() && batch.len() + line.len() > BATCH_LIMIT {
            flush_batch(&mut batch, true);
        }
        batch.push_str(line);
    }
    flush_batch(&mut batch, false);
}

/// The batching must be byte-for-byte transparent, or a probe failure would be the
/// harness losing input rather than the behaviour under test.
#[test]
fn paced_pty_writes_preserve_the_payload_exactly() {
    let mut payload = String::new();
    for index in 0..200 {
        payload.push_str(&format!("line-{index} {}\n", "x".repeat(index % 97)));
    }
    payload.push_str("trailing line without newline");

    let mut sink = Vec::new();
    write_pty_paced(&mut sink, &payload);

    assert_eq!(String::from_utf8(sink).expect("utf8 sink"), payload);
}

#[cfg(unix)]
fn run_bash_injection_history_probe(
    mode: super::ShellIntegrationMode,
    history_initially_enabled: bool,
) -> Option<String> {
    if std::process::Command::new("bash")
        .arg("--version")
        .output()
        .is_err()
    {
        return None;
    }

    let ready = super::build_ssh_ready_marker("history-probe");
    let script = super::ssh_shell_injection_script(super::ShellKind::Bash, &ready, mode)
        .expect("bash injection script");
    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open bash history probe pty");
    let mut command = CommandBuilder::new("bash");
    command.args(["--noprofile", "--norc", "-i"]);
    command.env("HISTFILE", "/dev/null");
    command.env("PS1", "");
    command.env("TERM", "xterm-256color");
    command.env_remove("PROMPT_COMMAND");
    command.env_remove("HISTCONTROL");

    let mut reader = pty
        .master
        .try_clone_reader()
        .expect("clone bash history probe reader");
    let mut writer = pty
        .master
        .take_writer()
        .expect("take bash history probe writer");
    let mut child = pty
        .slave
        .spawn_command(command)
        .expect("spawn bash history probe");
    drop(pty.slave);

    let output = Arc::new(Mutex::new(Vec::new()));
    let reader_output = output.clone();
    let reader_thread = std::thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        while let Ok(read) = reader.read(&mut buffer) {
            if read == 0 {
                break;
            }
            reader_output
                .lock()
                .expect("bash history probe output lock")
                .extend_from_slice(&buffer[..read]);
        }
    });

    writer
        .write_all(b"stty -echo\nprintf '__NYATERM_PTY_READY__\\n'\n")
        .expect("initialize bash history probe");
    writer.flush().expect("flush bash history probe setup");

    let setup_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let ready = output
            .lock()
            .expect("bash history probe output lock")
            .windows(b"__NYATERM_PTY_READY__".len())
            .any(|window| window == b"__NYATERM_PTY_READY__");
        if ready {
            break;
        }
        if Instant::now() >= setup_deadline {
            let _ = child.kill();
            panic!("timed out preparing interactive bash");
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let mut commands = String::from("history -c\nprintf '__NYATERM_USER_BEFORE__\\n'\n");
    if !history_initially_enabled {
        commands.push_str("set +o history\n");
    }
    commands.push_str(&script);
    commands.push_str(
        "if [[ -o history ]]; then printf '__NYATERM_HISTORY_STATE__:enabled\\n'; else printf '__NYATERM_HISTORY_STATE__:disabled\\n'; fi\nprintf '__NYATERM_HISTORY_BEGIN__\\n'\nHISTTIMEFORMAT= builtin history\nprintf '__NYATERM_HISTORY_END__\\n'\nexit\n",
    );
    write_pty_paced(&mut writer, &commands);

    let child_deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll bash history probe") {
            break status;
        }
        if Instant::now() >= child_deadline {
            let _ = child.kill();
            // Report what the shell actually produced. A truncated transcript means
            // the pty dropped input; a complete one that never exits means bash
            // stalled executing the script.
            let captured =
                String::from_utf8_lossy(&output.lock().expect("bash history probe output lock"))
                    .into_owned();
            panic!("interactive bash history probe timed out; captured: {captured:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success(), "bash history probe failed: {status:?}");
    drop(writer);
    drop(pty.master);
    reader_thread
        .join()
        .expect("join bash history probe reader");

    Some(
        String::from_utf8_lossy(&output.lock().expect("bash history probe output lock"))
            .into_owned(),
    )
}

#[cfg(unix)]
fn bash_history_probe_section(output: &str) -> &str {
    let start_marker = "__NYATERM_HISTORY_BEGIN__\r\n";
    let end_marker = "__NYATERM_HISTORY_END__";
    let start = output
        .rfind(start_marker)
        .expect("bash history probe start marker")
        + start_marker.len();
    let end = output[start..]
        .find(end_marker)
        .map(|offset| start + offset)
        .expect("bash history probe end marker");
    &output[start..end]
}

#[cfg(unix)]
#[test]
fn bash_inline_shell_integration_does_not_pollute_enabled_history() {
    for mode in [
        super::ShellIntegrationMode::Full,
        super::ShellIntegrationMode::CwdOnly,
    ] {
        let Some(output) = run_bash_injection_history_probe(mode, true) else {
            return;
        };
        assert!(
            output.contains("__NYATERM_HISTORY_STATE__:enabled"),
            "history was not restored for {mode:?}: {output:?}"
        );
        let history = bash_history_probe_section(&output);
        assert!(
            history.contains("__NYATERM_USER_BEFORE__"),
            "pre-injection history was lost for {mode:?}: {history:?}"
        );
        for leaked in ["NYATERM_INJ", "__nyaterm_", "__nya_bp_"] {
            assert!(
                !history.contains(leaked),
                "injected Bash source leaked into history for {mode:?}: {history:?}"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn bash_inline_shell_integration_preserves_disabled_history() {
    for mode in [
        super::ShellIntegrationMode::Full,
        super::ShellIntegrationMode::CwdOnly,
    ] {
        let Some(output) = run_bash_injection_history_probe(mode, false) else {
            return;
        };
        assert!(
            output.contains("__NYATERM_HISTORY_STATE__:disabled"),
            "history was unexpectedly enabled for {mode:?}: {output:?}"
        );
        let history = bash_history_probe_section(&output);
        assert!(
            history.contains("__NYATERM_USER_BEFORE__"),
            "pre-injection history was lost for {mode:?}: {history:?}"
        );
        for leaked in ["NYATERM_INJ", "__nyaterm_", "__nya_bp_"] {
            assert!(
                !history.contains(leaked),
                "injected Bash source leaked into disabled history for {mode:?}: {history:?}"
            );
        }
    }
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum UploadedHistoryProbeState {
    Default,
    BashHistoryDisabled,
    FishPrivate,
}

#[cfg(unix)]
fn run_uploaded_shell_history_probe(
    shell: super::ShellKind,
    mode: super::ShellIntegrationMode,
    initial_state: UploadedHistoryProbeState,
) -> Option<(String, std::path::PathBuf)> {
    let executable = match shell {
        super::ShellKind::Bash => "bash",
        super::ShellKind::Zsh => "zsh",
        super::ShellKind::Fish => "fish",
        super::ShellKind::PosixSh | super::ShellKind::Unknown => return None,
    };
    if !std::process::Command::new(executable)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return None;
    }

    let probe_id = uuid::Uuid::new_v4().simple().to_string();
    let remote_dir = std::env::temp_dir().join(format!(".nyaterm_inj_{probe_id}"));
    let remote_path = remote_dir.join("script.sh");
    let state_dir_guard =
        nyaterm_core::test_support::TestTempDir::new(&format!("nyaterm-history-probe-{probe_id}"));
    let state_dir = state_dir_guard.path().to_path_buf();
    std::fs::create_dir(&remote_dir).expect("create uploaded integration probe directory");
    std::fs::create_dir(&state_dir).expect("create shell history probe state directory");

    let ready = super::build_ssh_ready_marker("uploaded-history-probe");
    let integration =
        super::prepare_shell_integration(shell, &ready, mode).expect("prepared shell integration");
    std::fs::write(&remote_path, integration.upload_payload())
        .expect("write uploaded integration probe script");
    let injected = String::from_utf8(integration.uploaded_script(
        remote_path.to_str().expect("utf8 uploaded script path"),
        remote_dir.to_str().expect("utf8 uploaded script directory"),
    ))
    .expect("utf8 uploaded integration command");

    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open uploaded shell history probe pty");
    let mut command = CommandBuilder::new(executable);
    match shell {
        super::ShellKind::Bash => {
            command.args(["--noprofile", "--norc", "-i"]);
            command.env("HISTFILE", "/dev/null");
            command.env("PS1", "");
            command.env_remove("PROMPT_COMMAND");
            command.env_remove("HISTCONTROL");
        }
        super::ShellKind::Zsh => {
            command.args(["-f", "-i"]);
            command.env("HISTFILE", "/dev/null");
            command.env("PROMPT", "");
            command.env("RPROMPT", "");
        }
        super::ShellKind::Fish => {
            if matches!(initial_state, UploadedHistoryProbeState::FishPrivate) {
                command.args(["--private", "--interactive"]);
            } else {
                command.arg("--interactive");
            }
            command.env("fish_greeting", "");
            command.env("fish_history", format!("nyaterm_probe_{probe_id}"));
            command.env("XDG_DATA_HOME", state_dir.to_string_lossy().to_string());
            command.env("XDG_CONFIG_HOME", state_dir.to_string_lossy().to_string());
        }
        super::ShellKind::PosixSh | super::ShellKind::Unknown => unreachable!(),
    }
    command.env("TERM", "xterm-256color");

    let mut reader = pty
        .master
        .try_clone_reader()
        .expect("clone uploaded shell history probe reader");
    let mut writer = pty
        .master
        .take_writer()
        .expect("take uploaded shell history probe writer");
    let mut child = pty
        .slave
        .spawn_command(command)
        .expect("spawn uploaded shell history probe");
    drop(pty.slave);

    let output = Arc::new(Mutex::new(Vec::new()));
    let reader_output = output.clone();
    let reader_thread = std::thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        while let Ok(read) = reader.read(&mut buffer) {
            if read == 0 {
                break;
            }
            reader_output
                .lock()
                .expect("uploaded shell history probe output lock")
                .extend_from_slice(&buffer[..read]);
        }
    });

    let setup = match shell {
        super::ShellKind::Fish => {
            "stty -echo\nset -g fish_greeting\nfunction fish_prompt; end\nprintf '__NYATERM_PTY_READY__\\n'\n"
        }
        _ => "stty -echo\nprintf '__NYATERM_PTY_READY__\\n'\n",
    };
    writer
        .write_all(setup.as_bytes())
        .expect("initialize uploaded shell history probe");
    writer
        .flush()
        .expect("flush uploaded shell history probe setup");

    let setup_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let ready = output
            .lock()
            .expect("uploaded shell history probe output lock")
            .windows(b"__NYATERM_PTY_READY__".len())
            .any(|window| window == b"__NYATERM_PTY_READY__");
        if ready {
            break;
        }
        if Instant::now() >= setup_deadline {
            let _ = child.kill();
            panic!("timed out preparing interactive {executable}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let mut commands = match shell {
        super::ShellKind::Bash => "history -c\nprintf '__NYATERM_USER_BEFORE__\\n'\n".to_string(),
        super::ShellKind::Zsh => {
            "HISTSIZE=0\nHISTSIZE=1000\nprint -r -- '__NYATERM_USER_BEFORE__'\n".to_string()
        }
        super::ShellKind::Fish => "printf '__NYATERM_USER_BEFORE__\\n'\n".to_string(),
        super::ShellKind::PosixSh | super::ShellKind::Unknown => unreachable!(),
    };
    match initial_state {
        UploadedHistoryProbeState::Default => {}
        UploadedHistoryProbeState::BashHistoryDisabled => commands.push_str("set +o history\n"),
        UploadedHistoryProbeState::FishPrivate => {}
    }
    commands.push_str(&injected);
    match shell {
        super::ShellKind::Bash => commands.push_str(
            "if [[ -o history ]]; then printf '__NYATERM_HISTORY_STATE__:enabled\\n'; else printf '__NYATERM_HISTORY_STATE__:disabled\\n'; fi\nprintf '__NYATERM_HISTORY_BEGIN__\\n'\nHISTTIMEFORMAT= builtin history\nprintf '__NYATERM_HISTORY_END__\\n'\nexit\n",
        ),
        super::ShellKind::Zsh => commands.push_str(
            "fc -P 2>/dev/null\nprintf '__NYATERM_HISTORY_STACK__:%s\\n' $?\nprintf '__NYATERM_HISTORY_BEGIN__\\n'\nfc -l 1\nprintf '__NYATERM_HISTORY_END__\\n'\nexit\n",
        ),
        super::ShellKind::Fish => commands.push_str(
            "if set -q fish_private_mode; printf '__NYATERM_PRIVATE_STATE__:enabled\\n'; else; printf '__NYATERM_PRIVATE_STATE__:disabled\\n'; end\nprintf '__NYATERM_HISTORY_BEGIN__\\n'\nhistory\nprintf '__NYATERM_HISTORY_END__\\n'\nexit\n",
        ),
        super::ShellKind::PosixSh | super::ShellKind::Unknown => unreachable!(),
    }
    write_pty_paced(&mut writer, &commands);

    let child_deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll uploaded shell history probe") {
            break status;
        }
        if Instant::now() >= child_deadline {
            let _ = child.kill();
            let captured = String::from_utf8_lossy(
                &output
                    .lock()
                    .expect("uploaded shell history probe output lock"),
            )
            .into_owned();
            panic!("interactive {executable} history probe timed out: {captured:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        status.success(),
        "uploaded {executable} history probe failed: {status:?}"
    );
    drop(writer);
    drop(pty.master);
    reader_thread
        .join()
        .expect("join uploaded shell history probe reader");
    let captured =
        String::from_utf8_lossy(&output.lock().expect("uploaded history probe output lock"))
            .into_owned();
    let _ = std::fs::remove_dir_all(&state_dir);
    Some((captured, remote_dir))
}

#[cfg(unix)]
fn uploaded_history_probe_section(output: &str) -> &str {
    let start_marker = "__NYATERM_HISTORY_BEGIN__";
    let end_marker = "__NYATERM_HISTORY_END__";
    let start_line = format!("{start_marker}\r\n");
    let start = output
        .rfind(&start_line)
        .map(|offset| offset + start_line.len())
        .expect("history probe start marker line");
    let end_line = format!("{end_marker}\r\n");
    let end = output[start..]
        .find(&end_line)
        .map(|offset| start + offset)
        .expect("history probe end marker line");
    &output[start..end]
}

#[cfg(unix)]
fn assert_uploaded_integration_is_private(
    shell: super::ShellKind,
    mode: super::ShellIntegrationMode,
    initial_state: UploadedHistoryProbeState,
) -> Option<String> {
    let (output, remote_dir) = run_uploaded_shell_history_probe(shell, mode, initial_state)?;
    let ready = super::build_ssh_ready_marker("uploaded-history-probe");
    assert!(
        output.contains(&ready),
        "ready marker missing for {shell:?} {mode:?}: {output:?}"
    );
    assert!(
        !remote_dir.exists(),
        "uploaded integration directory was not removed for {shell:?} {mode:?}: {remote_dir:?}"
    );
    let history = uploaded_history_probe_section(&output);
    assert!(
        history.contains("__NYATERM_USER_BEFORE__"),
        "pre-injection history was lost for {shell:?} {mode:?}: {history:?}"
    );
    for leaked in [
        ".nyaterm_inj_",
        "script.sh",
        "NYATERM_INJ",
        "__nyaterm_",
        "__nya_bp_",
        "NyaTermReady:",
    ] {
        assert!(
            !history.contains(leaked),
            "uploaded integration leaked {leaked:?} for {shell:?} {mode:?}: {history:?}"
        );
    }
    Some(output)
}

#[cfg(unix)]
#[test]
fn bash_uploaded_shell_integration_preserves_history_state_without_pollution() {
    for mode in [
        super::ShellIntegrationMode::Full,
        super::ShellIntegrationMode::CwdOnly,
    ] {
        let enabled = assert_uploaded_integration_is_private(
            super::ShellKind::Bash,
            mode,
            UploadedHistoryProbeState::Default,
        );
        if let Some(output) = enabled {
            assert!(output.contains("__NYATERM_HISTORY_STATE__:enabled\r\n"));
        } else {
            return;
        }
        let disabled = assert_uploaded_integration_is_private(
            super::ShellKind::Bash,
            mode,
            UploadedHistoryProbeState::BashHistoryDisabled,
        )
        .expect("Bash availability was already established");
        assert!(disabled.contains("__NYATERM_HISTORY_STATE__:disabled\r\n"));
    }
}

#[cfg(unix)]
#[test]
fn zsh_uploaded_shell_integration_restores_history_stack_without_pollution() {
    for mode in [
        super::ShellIntegrationMode::Full,
        super::ShellIntegrationMode::CwdOnly,
    ] {
        let Some(output) = assert_uploaded_integration_is_private(
            super::ShellKind::Zsh,
            mode,
            UploadedHistoryProbeState::Default,
        ) else {
            return;
        };
        assert!(
            output.contains("__NYATERM_HISTORY_STACK__:1\r\n"),
            "injection left an extra Zsh history stack for {mode:?}: {output:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn fish_uploaded_shell_integration_preserves_private_mode_without_pollution() {
    for mode in [
        super::ShellIntegrationMode::Full,
        super::ShellIntegrationMode::CwdOnly,
    ] {
        for (initial_state, expected) in [
            (UploadedHistoryProbeState::Default, "disabled"),
            (UploadedHistoryProbeState::FishPrivate, "enabled"),
        ] {
            let Some(output) =
                assert_uploaded_integration_is_private(super::ShellKind::Fish, mode, initial_state)
            else {
                return;
            };
            assert!(
                output.contains(&format!("__NYATERM_PRIVATE_STATE__:{expected}\r\n")),
                "Fish private mode changed for {mode:?}: {output:?}"
            );
        }
    }
}

/// A real GNU bash we can hand a script to, or `None` when the platform only has a
/// stand-in. On Windows `bash` on `PATH` is often `System32\bash.exe`, the WSL
/// launcher: it starts fine (so spawning succeeds) but without an installed
/// distribution it fails with its message on stdout, which used to surface as an
/// empty "bash syntax error:".
fn gnu_bash_for_syntax_check() -> Option<std::process::Command> {
    let probe = std::process::Command::new("bash")
        .arg("--version")
        .output()
        .ok()?;
    if !probe.status.success() {
        return None;
    }
    if !String::from_utf8_lossy(&probe.stdout).contains("GNU bash") {
        return None;
    }
    Some(std::process::Command::new("bash"))
}

#[test]
fn generated_bash_shell_integration_scripts_pass_syntax_check() {
    let Some(_) = gnu_bash_for_syntax_check() else {
        return;
    };
    let ready = super::build_ssh_ready_marker("syntax-check");
    let scripts = [
        super::persistent_script(super::ShellKind::Bash)
            .expect("persistent bash script")
            .to_string(),
        super::ssh_shell_injection_script(
            super::ShellKind::Bash,
            &ready,
            super::ShellIntegrationMode::Full,
        )
        .expect("full bash script"),
        super::ssh_shell_injection_script(
            super::ShellKind::Bash,
            &ready,
            super::ShellIntegrationMode::CwdOnly,
        )
        .expect("cwd-only bash script"),
        super::activation_script(
            super::ShellKind::Bash,
            &ready,
            super::ShellIntegrationMode::Full,
        )
        .expect("bash activation script"),
    ];
    for (index, script) in scripts.into_iter().enumerate() {
        // Pipe the script into `bash -n` on stdin rather than passing it as a path or
        // as `-c <script>`: on Windows `bash` is either Git Bash, which rebuilds argv
        // from the raw command line and mangles a multi-KiB argument containing
        // newlines and quotes, or the WSL launcher, which cannot open a Windows path.
        // Stdin needs neither argv quoting nor path translation.
        let mut child = gnu_bash_for_syntax_check()
            .expect("gnu bash")
            .arg("-n")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn bash syntax check");
        child
            .stdin
            .take()
            .expect("bash syntax check stdin")
            .write_all(script.as_bytes())
            .expect("write bash syntax check script");
        let output = child
            .wait_with_output()
            .expect("wait for bash syntax check");
        assert!(
            output.status.success(),
            "script {index} failed bash syntax check ({}): {}{}",
            output.status,
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout),
        );
    }
}

#[test]
fn ssh_activation_and_persistent_scripts_match_rc_file_mode_contract() {
    let ready = super::build_ssh_ready_marker("session-1");
    let activation = super::activation_script(
        super::ShellKind::Bash,
        &ready,
        super::ShellIntegrationMode::Full,
    )
    .expect("activation script");
    let persistent = super::persistent_script(super::ShellKind::Bash).expect("persistent script");
    let block = super::rc_managed_block(super::ShellKind::Bash).expect("managed block");

    assert!(activation.contains("shell-integration.bash"));
    assert!(activation.contains("NyaTermReady:session-1"));
    assert!(activation.contains("__nyaterm_install_prompt full"));
    let cwd_activation = super::activation_script(
        super::ShellKind::Bash,
        &ready,
        super::ShellIntegrationMode::CwdOnly,
    )
    .expect("cwd-only activation script");
    assert!(cwd_activation.contains("__nyaterm_install_prompt cwd"));
    assert!(persistent.contains("__nyaterm_install_prompt"));
    assert!(persistent.contains("NyaTermCommand:%s"));
    assert!(block.contains("# >>> nyaterm shell integration >>>"));
    assert!(block.contains("shell-integration.bash"));
}

#[test]
fn ssh_osc_stripper_extracts_cwd_and_command_without_leaking_private_markers() {
    let ready = super::build_ssh_ready_marker("session-1");
    let legacy = super::build_legacy_ssh_ready_marker(&ready);
    let mut stripper = super::OscStripper::new(&ready, legacy.as_deref());
    let command = base64::engine::general_purpose::STANDARD.encode("git status");

    let first = stripper.push("hello\x1b]7;file://host/home/user".as_bytes());
    assert_eq!(first.visible, b"hello");
    assert!(first.cwd_paths.is_empty());

    let second = stripper.push(
        format!(
            "\x07x\x1b]7777;DflyCommand:{command}\x07y\x1b]7777;DflyReady:session-1\x07prompt$ "
        )
        .as_bytes(),
    );

    assert_eq!(second.visible, b"xyprompt$ ");
    assert_eq!(second.visible_after_ready, b"prompt$ ");
    assert_eq!(second.cwd_paths, vec!["/home/user".to_string()]);
    assert_eq!(second.accepted_commands, vec!["git status".to_string()]);
    assert!(second.ready);
}

#[test]
fn ssh_osc_stripper_ignores_ready_marker_for_other_sessions() {
    let ready = super::build_ssh_ready_marker("session-1");
    let mut stripper = super::OscStripper::new(&ready, None);

    let result = stripper.push(b"a\x1b]7777;NyaTermReady:session-2\x07b");

    assert_eq!(result.visible, b"ab");
    assert!(!result.ready);
    assert!(result.visible_after_ready.is_empty());
}

#[test]
fn ssh_ready_marker_helpers_strip_current_and_legacy_markers() {
    let ready = super::build_ssh_ready_marker("session-1").into_bytes();
    let legacy = super::build_legacy_ssh_ready_marker("\x1b]7777;NyaTermReady:session-1\x07")
        .expect("legacy marker")
        .into_bytes();
    let payload = [
        b"before".as_slice(),
        ready.as_slice(),
        b"middle".as_slice(),
        legacy.as_slice(),
        b"after".as_slice(),
    ]
    .concat();

    assert_eq!(
        super::strip_ssh_ready_markers(&payload, &ready, Some(&legacy)),
        b"beforemiddleafter"
    );
}

#[test]
fn local_reader_thread_routes_filtered_cwd_events_and_suppressed_output() {
    let integration = crate::local_shell_integration::LocalShellIntegration::prepare(
        &crate::local_shell::ResolvedLocalShell {
            program: "wsl.exe".to_string(),
            args: Vec::new(),
        },
        "session-1",
    )
    .expect("wsl.exe receives the prompt script");

    let mailbox = crate::WriterMailbox::default();
    integration.inject_now(mailbox.clone());
    let (command, _permit) = mailbox
        .receive(None)
        .expect("mailbox open")
        .expect("injection script queued");
    let crate::TransportWriterCommand::Write(_) = command else {
        panic!("injection writes a shell script");
    };

    let mut payload = b"startup noise".to_vec();
    payload.extend_from_slice(b"\x1b]7777;NyaTermReady:session-1\x07prompt> ");
    payload.extend_from_slice(b"\x1b]7;file://host/mnt/d/Codes\x07");

    let queue = SessionEventQueue::new();
    let handle = super::spawn_reader_thread(
        "session-1".to_string(),
        Box::new(std::io::Cursor::new(payload)),
        queue.clone(),
        Some(integration),
    )
    .expect("reader thread spawns");
    handle.join().expect("reader thread reaches EOF");

    let drain = queue.drain_with_output_budget(8, Some(4096));
    assert_eq!(drain.events.len(), 3);
    assert!(matches!(
        &drain.events[0],
        SessionEvent::CwdChanged { session_id, cwd }
            if session_id == "session-1" && cwd == "/mnt/d/Codes"
    ));
    assert!(matches!(
        &drain.events[1],
        SessionEvent::Output { data, .. } if data.as_slice() == b"prompt> "
    ));
    assert!(matches!(&drain.events[2], SessionEvent::Exited { .. }));
}

#[test]
fn session_event_queue_drains_metadata_even_with_zero_output_budget() {
    let queue = SessionEventQueue::new();
    queue.push(SessionEvent::CwdChanged {
        session_id: "s1".to_string(),
        cwd: "/opt/app".to_string(),
    });
    queue.push(SessionEvent::CommandAccepted {
        session_id: "s1".to_string(),
        command: "pwd".to_string(),
    });

    let drain = queue.drain_with_output_budget(8, Some(0));

    assert_eq!(drain.events.len(), 2);
    assert!(matches!(
        &drain.events[0],
        SessionEvent::CwdChanged { cwd, .. } if cwd == "/opt/app"
    ));
    assert!(matches!(
        &drain.events[1],
        SessionEvent::CommandAccepted { command, .. } if command == "pwd"
    ));
}

#[test]
fn ssh_ready_marker_detection_returns_bytes_after_marker() {
    let ready = super::build_ssh_ready_marker("session-1").into_bytes();
    let mut split = b"echoed injection".to_vec();
    split.extend_from_slice(&ready[..8]);
    assert!(super::bytes_after_ssh_ready_marker(&split, &ready, None).is_none());
    split.extend_from_slice(&ready[8..]);
    split.extend_from_slice(b"prompt$ ");

    assert_eq!(
        super::bytes_after_ssh_ready_marker(&split, &ready, None),
        Some(b"prompt$ ".as_slice())
    );
}

#[test]
fn forwarded_tcpip_dispatch_prefers_listener_specific_sender() {
    let (fallback_tx, _fallback_rx) = tokio_mpsc::unbounded_channel();
    let (specific_tx, _specific_rx) = tokio_mpsc::unbounded_channel();
    let dispatch = ForwardedTcpIpDispatch {
        fallback: Some(fallback_tx.clone()),
        by_listener: HashMap::from([(("127.0.0.1".to_string(), 2022), specific_tx.clone())]),
    };

    let exact = forwarded_tcpip_sender_for(&dispatch, "127.0.0.1", 2022).expect("specific sender");
    assert!(exact.same_channel(&specific_tx));

    let fallback =
        forwarded_tcpip_sender_for(&dispatch, "127.0.0.1", 2200).expect("fallback sender");
    assert!(fallback.same_channel(&fallback_tx));

    let empty = ForwardedTcpIpDispatch::default();
    assert!(forwarded_tcpip_sender_for(&empty, "127.0.0.1", 2022).is_none());
}

#[test]
fn process_parser_reads_legacy_rows() {
    let rows = "PROCESS\t42\t1\troot\tSs\t0.4\t1.2\t1234\t5678\t01:02\tsshd\t/usr/sbin/sshd -D\n";

    let processes = parse_process_output(rows);

    assert_eq!(processes.len(), 1);
    assert_eq!(processes[0].pid, 42);
    assert_eq!(processes[0].ppid, 1);
    assert_eq!(processes[0].user, "root");
    assert_eq!(processes[0].cpu_percent, 0.4);
    assert_eq!(processes[0].command_line, "/usr/sbin/sshd -D");
}

#[test]
fn process_parser_preserves_command_lines_containing_tabs() {
    let rows = "PROCESS\t9\t1\troot\tS\t0\t0\t1\t2\t-\tawk\tawk\twith\ttabs\n";

    let processes = parse_process_output(rows);

    assert_eq!(processes.len(), 1);
    assert_eq!(processes[0].command_line, "awk\twith\ttabs");
}

#[test]
fn process_parser_detects_unsupported_marker() {
    assert!(is_process_list_unsupported(
        "warning\nNYATERM_PROCESS_UNSUPPORTED\n"
    ));
    assert!(!is_process_list_unsupported(
        "PROCESS\t1\t0\troot\tS\t0\t0\t0\t0\t-\tsh\tsh\n"
    ));
}

#[test]
fn process_signal_normalization_matches_legacy_allowlist() {
    assert_eq!(normalize_process_signal("sigterm").unwrap(), "TERM");
    assert_eq!(normalize_process_signal("9").unwrap(), "KILL");
    assert_eq!(normalize_process_signal("cont").unwrap(), "CONT");
    assert!(normalize_process_signal("USR1").is_err());
}

#[test]
fn ssh_config_debug_redacts_password() {
    let config = SshSessionConfig {
        password: Some("super-secret".to_string().into()),
        ..Default::default()
    };
    let debug = format!("{config:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains("super-secret"));
}

#[test]
fn ssh_config_debug_redacts_key_material() {
    let config = SshSessionConfig {
        key_auth: Some(SshKeyAuthConfig {
            key_data: "-----BEGIN PRIVATE KEY-----secret-key".to_string().into(),
            cert_data: Some(
                "ssh-ed25519-cert-v01@openssh.com secret-cert"
                    .to_string()
                    .into(),
            ),
            passphrase: Some("key-passphrase".to_string().into()),
        }),
        ..Default::default()
    };
    let debug = format!("{config:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains("secret-key"));
    assert!(!debug.contains("secret-cert"));
    assert!(!debug.contains("key-passphrase"));
}

#[test]
fn ssh_key_auth_debug_redacts_material_when_formatted_directly() {
    let key_auth = SshKeyAuthConfig {
        key_data: "private-key-material".to_string().into(),
        cert_data: Some("certificate-material".to_string().into()),
        passphrase: Some("passphrase-material".to_string().into()),
    };

    let debug = format!("{key_auth:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains("private-key-material"));
    assert!(!debug.contains("certificate-material"));
    assert!(!debug.contains("passphrase-material"));
}

#[test]
fn ssh_config_debug_redacts_proxy_password() {
    let config = SshSessionConfig {
        proxy: Some(SshProxyConfig {
            protocol: "socks5".to_string(),
            host: "127.0.0.1".to_string(),
            port: 1080,
            command: None,
            username: Some("proxy-user".to_string()),
            password: Some("proxy-secret".to_string().into()),
        }),
        ..Default::default()
    };
    let debug = format!("{config:?}");
    assert!(debug.contains("<redacted>"));
    assert!(debug.contains("proxy-user"));
    assert!(!debug.contains("proxy-secret"));
}

#[test]
fn ssh_client_config_disables_idle_timeout_and_maps_keepalive() {
    let config = SshSessionConfig {
        keep_alive_interval_secs: 45,
        ..Default::default()
    };

    let client_config = ssh_client_config(&config).expect("client config");

    assert_eq!(client_config.inactivity_timeout, None);
    assert_eq!(
        client_config.keepalive_interval,
        Some(Duration::from_secs(45))
    );
    assert_eq!(client_config.keepalive_max, 3);

    let disabled = SshSessionConfig {
        keep_alive_interval_secs: 0,
        ..Default::default()
    };
    let disabled_client_config = ssh_client_config(&disabled).expect("disabled client config");

    assert_eq!(disabled_client_config.inactivity_timeout, None);
    assert_eq!(disabled_client_config.keepalive_interval, None);
}

#[test]
fn ssh_client_config_maps_custom_algorithm_preferences() {
    let preferences = SshAlgorithmPreferences {
        mode: SshAlgorithmMode::Custom,
        kex: vec!["curve25519-sha256".to_string()],
        ciphers: vec!["aes128-ctr".to_string()],
        macs: vec!["hmac-sha2-256".to_string()],
        host_keys: vec!["ssh-ed25519".to_string()],
    };
    let config = SshSessionConfig {
        ssh_algorithms: Some(preferences),
        ..Default::default()
    };

    let client_config = ssh_client_config(&config).expect("client config");

    assert_eq!(client_config.preferred.kex.as_ref(), &[kex::CURVE25519]);
    assert_eq!(
        client_config.preferred.cipher.as_ref(),
        &[cipher::AES_128_CTR]
    );
    assert_eq!(client_config.preferred.mac.as_ref(), &[mac::HMAC_SHA256]);
    assert_eq!(
        client_config.preferred.key.as_ref(),
        &[russh::keys::Algorithm::Ed25519]
    );
}

#[test]
fn ssh_algorithm_validation_rejects_empty_or_unknown_custom_lists() {
    let empty = SshAlgorithmPreferences {
        mode: SshAlgorithmMode::Custom,
        ..Default::default()
    };
    assert_eq!(
        validate_ssh_algorithm_preferences(Some(&empty)),
        Err(SshAlgorithmValidationError::EmptyList {
            kind: SshAlgorithmListKind::KeyExchange,
        })
    );

    let unknown = SshAlgorithmPreferences {
        mode: SshAlgorithmMode::Custom,
        kex: vec!["not-a-kex".to_string()],
        ciphers: vec!["aes128-ctr".to_string()],
        macs: vec!["hmac-sha2-256".to_string()],
        host_keys: vec!["ssh-ed25519".to_string()],
    };
    assert_eq!(
        validate_ssh_algorithm_preferences(Some(&unknown)),
        Err(SshAlgorithmValidationError::Unsupported {
            kind: SshAlgorithmListKind::KeyExchange,
            algorithm: "not-a-kex".to_string(),
        })
    );
}

#[test]
fn supported_ssh_algorithms_expose_defaults_and_risk_metadata() {
    let supported = supported_ssh_algorithms();

    assert_eq!(
        supported.compatible.kex.first().map(String::as_str),
        Some("mlkem768x25519-sha256")
    );
    assert!(
        supported
            .secure
            .ciphers
            .iter()
            .all(|id| supported.ciphers.iter().any(|option| option.id == *id))
    );
    assert_eq!(
        supported
            .ciphers
            .iter()
            .find(|option| option.id == "3des-cbc")
            .map(|option| option.risk),
        Some(SshAlgorithmRisk::Insecure)
    );
    assert_eq!(
        supported
            .host_keys
            .iter()
            .find(|option| option.id == "ssh-rsa")
            .map(|option| option.risk),
        Some(SshAlgorithmRisk::Legacy)
    );
}

#[test]
fn ssh_algorithm_custom_order_reaches_runtime_unchanged() {
    let defaults = &supported_ssh_algorithms().compatible;
    let mut preferences = SshAlgorithmPreferences {
        mode: SshAlgorithmMode::Custom,
        kex: defaults.kex.clone(),
        ciphers: defaults.ciphers.clone(),
        macs: defaults.macs.clone(),
        host_keys: defaults.host_keys.clone(),
    };
    preferences.kex.swap(0, 1);
    preferences.ciphers.swap(0, 1);
    preferences.macs.swap(0, 1);
    preferences.host_keys.swap(0, 1);

    let resolved = resolve_preferred_algorithms(Some(&preferences)).expect("valid preferences");
    let resolved = defaults_from_preferred(resolved);
    assert_eq!(resolved.kex, preferences.kex);
    assert_eq!(resolved.ciphers, preferences.ciphers);
    assert_eq!(resolved.macs, preferences.macs);
    assert_eq!(resolved.host_keys, preferences.host_keys);
}

#[test]
fn local_config_defaults_to_unknown_pixel_dimensions() {
    let config = LocalSessionConfig::default();
    assert_eq!(config.cols, 80);
    assert_eq!(config.rows, 24);
    assert_eq!(config.pixel_width, 0);
    assert_eq!(config.pixel_height, 0);
}

#[test]
fn local_pty_size_preserves_cell_and_pixel_dimensions() {
    let size = local_pty_size(132, 43, 1056, 688);
    assert_eq!(size.cols, 132);
    assert_eq!(size.rows, 43);
    assert_eq!(size.pixel_width, 1056);
    assert_eq!(size.pixel_height, 688);
}

#[test]
fn ssh_pty_dimensions_clamp_to_positive_cells() {
    let dimensions = SshPtyDimensions::new(0, 0, 0, 0);
    assert_eq!(dimensions.cols, 1);
    assert_eq!(dimensions.rows, 1);
    assert_eq!(dimensions.pixel_width, 0);
    assert_eq!(dimensions.pixel_height, 0);

    let dimensions = SshPtyDimensions::new(132, 43, 1056, 688);
    assert_eq!(dimensions.cols, 132);
    assert_eq!(dimensions.rows, 43);
    assert_eq!(dimensions.pixel_width, 1056);
    assert_eq!(dimensions.pixel_height, 688);
}

#[test]
fn ssh_pty_dimensions_use_config_size() {
    let config = SshSessionConfig {
        cols: 101,
        rows: 37,
        pixel_width: 808,
        pixel_height: 592,
        ..Default::default()
    };
    let dimensions = SshPtyDimensions::from_config(&config);
    assert_eq!(dimensions.cols, 101);
    assert_eq!(dimensions.rows, 37);
    assert_eq!(dimensions.pixel_width, 808);
    assert_eq!(dimensions.pixel_height, 592);
}

#[test]
fn deferred_ssh_open_drain_keeps_writes_and_latest_resize() {
    let (tx, mut rx) = crate::ssh_mailbox::channel();
    tx.write(b"before".to_vec()).unwrap();
    tx.resize(100, 30, 800, 600);
    tx.resize(132, 43, 1056, 688);
    tx.write(b"after".to_vec()).unwrap();

    let mut dimensions = SshPtyDimensions::new(80, 24, 0, 0);
    let mut pending_writes = VecDeque::new();
    let should_close =
        drain_deferred_ssh_open_commands(&mut rx, &mut dimensions, &mut pending_writes);

    assert!(!should_close);
    assert_eq!(dimensions, SshPtyDimensions::new(132, 43, 1056, 688));
    assert_eq!(
        pending_writes
            .into_iter()
            .map(|write| write.bytes)
            .collect::<Vec<_>>(),
        vec![b"before".to_vec(), b"after".to_vec()]
    );
}

#[test]
fn deferred_ssh_open_drain_closes_before_shell_open() {
    let (tx, mut rx) = crate::ssh_mailbox::channel();
    tx.write(b"queued".to_vec()).unwrap();
    tx.close();

    let mut dimensions = SshPtyDimensions::new(80, 24, 0, 0);
    let mut pending_writes = VecDeque::new();
    let should_close =
        drain_deferred_ssh_open_commands(&mut rx, &mut dimensions, &mut pending_writes);

    assert!(should_close);
    assert!(pending_writes.is_empty(), "close bypasses queued writes");
}

#[test]
fn deferred_ssh_open_drain_closes_on_disconnected_command_channel() {
    let (tx, mut rx) = crate::ssh_mailbox::channel();
    drop(tx);

    let mut dimensions = SshPtyDimensions::new(80, 24, 0, 0);
    let mut pending_writes = VecDeque::new();

    assert!(drain_deferred_ssh_open_commands(
        &mut rx,
        &mut dimensions,
        &mut pending_writes
    ));
    assert!(pending_writes.is_empty());
}

#[test]
fn proxy_command_expansion_replaces_ssh_tokens() {
    let expanded = expand_proxy_command(
        Some("nc %h %p --user %r --literal %%"),
        "host name",
        2222,
        "user'name",
    )
    .expect("expanded command");

    #[cfg(windows)]
    {
        assert!(expanded.contains("\"host name\""));
        assert!(expanded.contains("2222"));
        assert!(expanded.contains("\"user'name\""));
    }
    #[cfg(not(windows))]
    {
        assert!(expanded.contains("'host name'"));
        assert!(expanded.contains("'2222'"));
        assert!(expanded.contains("'user'\\''name'"));
    }
    assert!(expanded.contains("--literal %"));
}

fn collect_output(manager: &SessionManager, session_id: &str, timeout: Duration) -> Vec<u8> {
    collect_output_until(manager, session_id, "nyaterm-transport-ready", timeout)
}

fn collect_output_until(
    manager: &SessionManager,
    session_id: &str,
    needle: &str,
    timeout: Duration,
) -> Vec<u8> {
    let started = Instant::now();
    let mut output = Vec::new();
    while started.elapsed() < timeout {
        for event in manager.drain_events(16).expect("events").events {
            match event {
                SessionEvent::Output {
                    session_id: event_session_id,
                    data,
                } if event_session_id == session_id => output.extend(data),
                SessionEvent::OutputDropped { .. } => {}
                SessionEvent::Error { message, .. } => panic!("session error: {message}"),
                _ => {}
            }
        }
        if String::from_utf8_lossy(&output).contains(needle) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    output
}

#[test]
fn session_event_queue_keeps_consecutive_output_chunks_separate() {
    let queue = SessionEventQueue::new();
    queue.push(SessionEvent::Output {
        session_id: "a".to_string(),
        data: b"hello ".to_vec(),
    });
    queue.push(SessionEvent::Output {
        session_id: "a".to_string(),
        data: b"world".to_vec(),
    });

    let drain = queue.drain(8);
    assert_eq!(drain.events.len(), 2);
    assert_eq!(drain.stats.drained_output_bytes, 11);
    assert_eq!(drain.stats.queued_output_bytes, 0);
    match &drain.events[0] {
        SessionEvent::Output { session_id, data } => {
            assert_eq!(session_id, "a");
            assert_eq!(data, b"hello ");
        }
        event => panic!("unexpected event: {event:?}"),
    }
    match &drain.events[1] {
        SessionEvent::Output { session_id, data } => {
            assert_eq!(session_id, "a");
            assert_eq!(data, b"world");
        }
        event => panic!("unexpected event: {event:?}"),
    }
}

#[test]
fn session_event_queue_keeps_sessions_separate() {
    let queue = SessionEventQueue::new();
    queue.push(SessionEvent::Output {
        session_id: "a".to_string(),
        data: b"a1".to_vec(),
    });
    queue.push(SessionEvent::Output {
        session_id: "b".to_string(),
        data: b"b1".to_vec(),
    });
    queue.push(SessionEvent::Output {
        session_id: "a".to_string(),
        data: b"a2".to_vec(),
    });

    let drain = queue.drain(8);
    assert_eq!(drain.events.len(), 3);
    assert!(matches!(
        &drain.events[0],
        SessionEvent::Output { session_id, data } if session_id == "a" && data == b"a1"
    ));
    assert!(matches!(
        &drain.events[1],
        SessionEvent::Output { session_id, data } if session_id == "b" && data == b"b1"
    ));
    assert!(matches!(
        &drain.events[2],
        SessionEvent::Output { session_id, data } if session_id == "a" && data == b"a2"
    ));
}

#[test]
fn consumer_wait_wakes_for_assigned_output_without_removing_events() {
    let queue = SessionEventQueue::new();
    let consumer_id = SessionEventConsumerId(1);
    queue.assign_consumer("assigned", consumer_id);
    queue.push(SessionEvent::Output {
        session_id: "other".into(),
        data: b"unrelated".to_vec(),
    });
    let waiter_queue = queue.clone();
    let (done_tx, done_rx) = mpsc::channel();
    let waiter = std::thread::spawn(move || {
        let ready = waiter_queue.wait_for_events_for_consumer(consumer_id, Duration::from_secs(30));
        done_tx.send(ready).expect("report readiness");
    });
    wait_for_queue_state(
        || queue.waiting_consumers() == 1,
        "the assigned consumer to park",
    );
    queue.push(SessionEvent::Output {
        session_id: "assigned".into(),
        data: b"ready".to_vec(),
    });
    let ready = done_rx.recv_timeout(Duration::from_secs(5));
    queue.close();
    waiter.join().expect("readiness waiter");
    assert!(ready.expect("assigned output must wake the consumer"));
    let drain = queue.drain(8);
    assert!(matches!(
        drain.events.as_slice(),
        [SessionEvent::Output { session_id: first, data: first_data },
         SessionEvent::Output { session_id: second, data: second_data }]
            if first == "other" && first_data == b"unrelated"
                && second == "assigned" && second_data == b"ready"
    ));
}

#[test]
fn consumer_readiness_does_not_reserve_events_across_reassignment() {
    let queue = SessionEventQueue::new();
    let source = SessionEventConsumerId(1);
    let target = SessionEventConsumerId(2);
    queue.assign_consumer("moved", source);
    for data in [b"before".as_slice(), b"after"] {
        queue.push(SessionEvent::Output {
            session_id: "moved".into(),
            data: data.to_vec(),
        });
    }
    assert!(queue.wait_for_events_for_consumer(source, Duration::ZERO));
    queue.clear_consumer("moved", source);
    queue.assign_consumer("moved", target);
    assert!(
        queue
            .drain_blocking_for_consumer_with_output_budget(source, 8, Some(1024), Duration::ZERO)
            .events
            .is_empty()
    );
    assert!(queue.wait_for_events_for_consumer(target, Duration::ZERO));
    let drain =
        queue.drain_blocking_for_consumer_with_output_budget(target, 8, Some(1024), Duration::ZERO);
    assert!(matches!(
        drain.events.as_slice(),
        [SessionEvent::Output { data: first, .. }, SessionEvent::Output { data: second, .. }]
            if first == b"before" && second == b"after"
    ));
}

#[test]
fn consumer_wait_returns_false_on_timeout_and_wakes_on_close() {
    let queue = SessionEventQueue::new();
    let consumer_id = SessionEventConsumerId(1);
    assert!(!queue.wait_for_events_for_consumer(consumer_id, Duration::from_millis(20)));
    let waiter_queue = queue.clone();
    let (done_tx, done_rx) = mpsc::channel();
    let waiter = std::thread::spawn(move || {
        let ready = waiter_queue.wait_for_events_for_consumer(consumer_id, Duration::from_secs(30));
        done_tx.send(ready).expect("report closed queue");
    });
    wait_for_queue_state(
        || queue.waiting_consumers() == 1,
        "the idle consumer to park",
    );
    queue.close();
    let ready = done_rx.recv_timeout(Duration::from_secs(5));
    waiter.join().expect("closed queue waiter");
    assert!(!ready.expect("closing the queue must wake the consumer"));
}

#[test]
fn session_event_consumers_only_drain_assigned_sessions_and_follow_reassignment() {
    let queue = SessionEventQueue::new();
    let first = SessionEventConsumerId(1);
    let second = SessionEventConsumerId(2);
    queue.assign_consumer("a", first);
    queue.assign_consumer("b", second);
    queue.push(SessionEvent::Output {
        session_id: "a".to_string(),
        data: b"a1".to_vec(),
    });
    queue.push(SessionEvent::Output {
        session_id: "b".to_string(),
        data: b"b1".to_vec(),
    });

    let first_drain =
        queue.drain_blocking_for_consumer_with_output_budget(first, 8, Some(1024), Duration::ZERO);
    assert!(matches!(
        first_drain.events.as_slice(),
        [SessionEvent::Output { session_id, data }]
            if session_id == "a" && data == b"a1"
    ));

    queue.assign_consumer("b", first);
    let reassigned =
        queue.drain_blocking_for_consumer_with_output_budget(first, 8, Some(1024), Duration::ZERO);
    assert!(matches!(
        reassigned.events.as_slice(),
        [SessionEvent::Output { session_id, data }]
            if session_id == "b" && data == b"b1"
    ));
    assert!(
        queue
            .drain_blocking_for_consumer_with_output_budget(second, 8, Some(1024), Duration::ZERO,)
            .events
            .is_empty()
    );
}

#[test]
fn session_event_queue_respects_output_drain_budget() {
    let queue = SessionEventQueue::new();
    queue.push(SessionEvent::Output {
        session_id: "a".to_string(),
        data: vec![b'a'; 128],
    });
    queue.push(SessionEvent::Output {
        session_id: "b".to_string(),
        data: vec![b'b'; 128],
    });

    let drain = queue.drain_with_output_budget(8, Some(200));
    assert_eq!(drain.events.len(), 2);
    assert_eq!(drain.stats.drained_output_bytes, 200);
    assert_eq!(drain.stats.queued_output_bytes, 56);
    assert!(matches!(
        &drain.events[0],
        SessionEvent::Output { session_id, data } if session_id == "a" && data.len() == 128
    ));
    assert!(matches!(
        &drain.events[1],
        SessionEvent::Output { session_id, data } if session_id == "b" && data.len() == 72
    ));

    let drain = queue.drain_with_output_budget(8, Some(200));
    assert_eq!(drain.events.len(), 1);
    assert_eq!(drain.stats.drained_output_bytes, 56);
    assert_eq!(drain.stats.queued_output_bytes, 0);
}

#[test]
fn session_event_queue_zero_output_budget_does_not_drain_output() {
    let queue = SessionEventQueue::new();
    queue.push(SessionEvent::Output {
        session_id: "a".to_string(),
        data: b"hello".to_vec(),
    });

    let drain = queue.drain_with_output_budget(8, Some(0));
    assert!(drain.events.is_empty());
    assert_eq!(drain.stats.drained_output_bytes, 0);
    assert_eq!(drain.stats.queued_output_bytes, 5);

    let drain = queue.drain_with_output_budget(8, Some(8));
    assert_eq!(drain.events.len(), 1);
    assert_eq!(drain.stats.drained_output_bytes, 5);
    assert_eq!(drain.stats.queued_output_bytes, 0);
}

#[test]
fn session_event_queue_zero_output_budget_preserves_oversized_output() {
    let queue = SessionEventQueue::new();
    queue.push(SessionEvent::Output {
        session_id: "a".to_string(),
        data: vec![b'x'; SESSION_EVENT_QUEUE_OUTPUT_EVENT_LIMIT + 32],
    });

    let drain = queue.drain_with_output_budget(8, Some(0));
    assert!(drain.events.is_empty());
    assert_eq!(drain.stats.drained_output_bytes, 0);
    assert_eq!(
        drain.stats.queued_output_bytes,
        SESSION_EVENT_QUEUE_OUTPUT_EVENT_LIMIT + 32
    );
}

#[test]
fn session_event_queue_splits_oversized_output_without_dropping_bytes() {
    let queue = SessionEventQueue::new();
    queue.push(SessionEvent::Output {
        session_id: "a".to_string(),
        data: vec![b'x'; SESSION_EVENT_QUEUE_OUTPUT_EVENT_LIMIT + 32],
    });

    let drain = queue.drain(8);
    assert_eq!(drain.events.len(), 2);
    assert!(matches!(
        &drain.events[0],
        SessionEvent::Output { data, .. } if data.len() == SESSION_EVENT_QUEUE_OUTPUT_EVENT_LIMIT
    ));
    assert!(matches!(
        &drain.events[1],
        SessionEvent::Output { data, .. } if data.len() == 32
    ));
    assert_eq!(
        drain.stats.drained_output_bytes,
        SESSION_EVENT_QUEUE_OUTPUT_EVENT_LIMIT + 32
    );
    assert_eq!(drain.stats.dropped_output_bytes, 0);
}

#[test]
fn session_event_queue_keeps_adjacent_output_chunks_separate() {
    let queue = SessionEventQueue::new();
    queue.push(SessionEvent::Output {
        session_id: "a".to_string(),
        data: vec![b'a'; SESSION_EVENT_QUEUE_OUTPUT_EVENT_LIMIT - 8],
    });
    queue.push(SessionEvent::Output {
        session_id: "a".to_string(),
        data: vec![b'b'; 16],
    });

    let drain = queue.drain(8);
    assert_eq!(drain.events.len(), 2);
    assert!(matches!(
        &drain.events[0],
        SessionEvent::Output { session_id, data } if session_id == "a"
            && data.len() == SESSION_EVENT_QUEUE_OUTPUT_EVENT_LIMIT - 8
            && data[0] == b'a'
    ));
    assert!(matches!(
        &drain.events[1],
        SessionEvent::Output { session_id, data } if session_id == "a"
            && data.len() == 16
            && *data.last().unwrap() == b'b'
    ));
    assert_eq!(drain.stats.dropped_output_bytes, 0);
}

#[test]
fn session_event_queue_backpressures_and_resumes_without_dropping_bytes() {
    let queue = SessionEventQueue::new();
    let producer_queue = queue.clone();
    let payload_bytes = SESSION_EVENT_QUEUE_OUTPUT_LIMIT + SESSION_EVENT_QUEUE_OUTPUT_EVENT_LIMIT;
    let producer = std::thread::spawn(move || {
        producer_queue.push(SessionEvent::Output {
            session_id: "session-a".to_string(),
            data: vec![b'x'; payload_bytes],
        });
    });

    let started = Instant::now();
    let mut received = Vec::with_capacity(payload_bytes);
    while received.len() < payload_bytes && started.elapsed() < Duration::from_secs(5) {
        let drain = queue.drain_with_output_budget(64, Some(1024 * 1024));
        for event in drain.events {
            match event {
                SessionEvent::Output { data, .. } => received.extend(data),
                SessionEvent::OutputDropped { bytes, .. } => {
                    panic!("local backpressure must not drop {bytes} bytes")
                }
                _ => {}
            }
        }
        if received.len() < payload_bytes {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    producer
        .join()
        .expect("producer resumes below low watermark");

    assert_eq!(received.len(), payload_bytes);
    assert!(received.iter().all(|byte| *byte == b'x'));
}

#[test]
fn cancelling_full_session_releases_producer_without_affecting_another_session() {
    let queue = SessionEventQueue::new();
    queue.push(SessionEvent::Output {
        session_id: "cancelled".to_string(),
        data: vec![b'x'; SESSION_EVENT_QUEUE_OUTPUT_LIMIT],
    });

    let blocked_queue = queue.clone();
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let producer = std::thread::spawn(move || {
        blocked_queue.push(SessionEvent::Output {
            session_id: "cancelled".to_string(),
            data: vec![b'y'; SESSION_EVENT_QUEUE_OUTPUT_EVENT_LIMIT],
        });
        done_tx.send(()).expect("report cancelled producer exit");
    });
    wait_for_queue_state(
        || queue.waiting_output_producers() == 1,
        "the full-queue producer to park",
    );

    queue.cancel_session("cancelled");
    done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("cancellation must release the producer");
    producer.join().expect("cancelled producer");

    queue.push(SessionEvent::Output {
        session_id: "cancelled".to_string(),
        data: b"late output".to_vec(),
    });
    queue.push(SessionEvent::Exited {
        session_id: "cancelled".to_string(),
        reason: "late exit".to_string(),
    });
    queue.push(SessionEvent::Error {
        session_id: "cancelled".to_string(),
        message: "late error".to_string(),
    });
    queue.push(SessionEvent::Output {
        session_id: "active".to_string(),
        data: b"still running".to_vec(),
    });
    queue.push(SessionEvent::Exited {
        session_id: "active".to_string(),
        reason: "done".to_string(),
    });

    let drain = queue.drain(64);
    assert_eq!(drain.events.len(), 2);
    assert!(matches!(
        &drain.events[0],
        SessionEvent::Output { session_id, data }
            if session_id == "active" && data == b"still running"
    ));
    assert!(matches!(
        &drain.events[1],
        SessionEvent::Exited { session_id, reason }
            if session_id == "active" && reason == "done"
    ));
    assert_eq!(drain.stats.dropped_output_bytes, 0);
    assert_eq!(drain.stats.queued_output_bytes, 0);
}

#[test]
fn global_close_releases_full_producer_and_blocking_consumer() {
    let producer_queue = SessionEventQueue::new();
    producer_queue.push(SessionEvent::Output {
        session_id: "producer".to_string(),
        data: vec![b'x'; SESSION_EVENT_QUEUE_OUTPUT_LIMIT],
    });
    let blocked_producer_queue = producer_queue.clone();
    let (producer_done_tx, producer_done_rx) = mpsc::sync_channel(1);
    let producer = std::thread::spawn(move || {
        blocked_producer_queue.push(SessionEvent::Output {
            session_id: "producer".to_string(),
            data: vec![b'y'; SESSION_EVENT_QUEUE_OUTPUT_EVENT_LIMIT],
        });
        producer_done_tx
            .send(())
            .expect("report closed producer exit");
    });
    wait_for_queue_state(
        || producer_queue.waiting_output_producers() == 1,
        "the producer to park at the high watermark",
    );

    let consumer_queue = SessionEventQueue::new();
    let blocked_consumer_queue = consumer_queue.clone();
    let (consumer_done_tx, consumer_done_rx) = mpsc::sync_channel(1);
    let consumer = std::thread::spawn(move || {
        let drain = blocked_consumer_queue.drain_blocking_with_output_budget(
            16,
            Some(64 * 1024),
            Duration::from_secs(30),
        );
        consumer_done_tx
            .send(drain)
            .expect("report closed consumer exit");
    });
    wait_for_queue_state(
        || consumer_queue.waiting_consumers() == 1,
        "the blocking consumer to park",
    );

    producer_queue.close();
    consumer_queue.close();

    producer_done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("global close must release the producer");
    let consumer_drain = consumer_done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("global close must release the consumer");
    producer.join().expect("closed producer");
    consumer.join().expect("closed consumer");
    assert!(consumer_drain.events.is_empty());

    consumer_queue.push(SessionEvent::Error {
        session_id: "late".to_string(),
        message: "after close".to_string(),
    });
    assert!(consumer_queue.drain(1).events.is_empty());
}

#[test]
fn ssh_keepalive_policy_reaches_the_client_configuration() {
    use nyaterm_core::terminal::connection_input::KeepaliveMode;
    for (mode, expected) in [
        (KeepaliveMode::Strict, russh::client::KeepaliveMode::Strict),
        (
            KeepaliveMode::Compatible,
            russh::client::KeepaliveMode::Compatible,
        ),
    ] {
        let config = SshSessionConfig {
            keep_alive_mode: mode,
            keep_alive_interval_secs: 12,
            ..Default::default()
        };
        let client = ssh_client_config(&config).unwrap();
        assert_eq!(client.keepalive_mode, expected);
        assert_eq!(
            client.keepalive_interval,
            Some(std::time::Duration::from_secs(12))
        );
    }
    let disabled = SshSessionConfig {
        keep_alive_mode: KeepaliveMode::Disabled,
        keep_alive_interval_secs: 12,
        ..Default::default()
    };
    assert_eq!(
        ssh_client_config(&disabled).unwrap().keepalive_interval,
        None
    );
}

#[test]
fn serial_writer_does_not_flush_on_windows() {
    struct NoFlushWriter(mpsc::Sender<()>);
    impl std::io::Write for NoFlushWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.0.send(()).unwrap();
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            panic!("unexpected serial flush")
        }
    }
    let policy = if cfg!(windows) {
        crate::WriterFlushPolicy::serial()
    } else {
        crate::WriterFlushPolicy::Never
    };
    let (tx, rx) = mpsc::channel();
    let mut writer = QueuedTransportWriter::spawn(
        "serial".into(),
        NoFlushWriter(tx),
        policy,
        SessionEventQueue::new(),
    )
    .unwrap();
    writer.write(b"command\r".to_vec()).unwrap();
    rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(writer.close());
}

#[test]
fn bash_prompt_hooks_do_not_leak_into_child_shells() {
    let Some(_) = gnu_bash_for_syntax_check() else {
        return;
    };
    for mode in [
        super::ShellIntegrationMode::Full,
        super::ShellIntegrationMode::CwdOnly,
    ] {
        let script = super::ssh_shell_injection_script(
            super::ShellKind::Bash,
            &super::build_ssh_ready_marker("child-shell"),
            mode,
        )
        .unwrap();
        let input = format!(
            r#"
export PROMPT_COMMAND='printf "hook=%s\n" "$?"'
export __nyaterm_cwd_prompt_hook=stale __nya_bp_precmd_hook=stale __nya_bp_interactive_hook=stale __nya_bp_install_payload=stale
{script}
if [ -n "${{NYATERM_BASH_INSTALL_PENDING:-}}" ]; then eval "$PROMPT_COMMAND"; fi
false; eval "$PROMPT_COMMAND"
bash --noprofile --norc -c 'false; eval "$PROMPT_COMMAND"'
"#
        );
        let mut child = gnu_bash_for_syntax_check()
            .unwrap()
            .args(["--noprofile", "--norc"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        assert!(!stderr.contains("command not found"), "{stderr}");
        assert_eq!(stdout.matches("hook=1").count(), 2, "{stdout} {stderr}");
    }
}

#[test]
fn bash_exported_arrays_and_readonly_prompt_commands_preserve_user_hooks() {
    for mode in [
        super::ShellIntegrationMode::Full,
        super::ShellIntegrationMode::CwdOnly,
    ] {
        for declaration in [
            "declare -ax PROMPT_COMMAND=('printf user-hook-ok')",
            "declare -rx PROMPT_COMMAND='printf user-hook-ok'",
        ] {
            let Some(mut command) = gnu_bash_for_syntax_check() else {
                return;
            };
            let script = super::ssh_shell_injection_script(
                super::ShellKind::Bash,
                &super::build_ssh_ready_marker("array-prompt"),
                mode,
            )
            .unwrap();
            let input = format!(
                "{declaration}\n{script}\nfor hook in \"${{PROMPT_COMMAND[@]}}\"; do eval \"$hook\"; done\n"
            );
            let mut child = command
                .args(["--noprofile", "--norc"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
            let output = child.wait_with_output().unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{declaration}: {stderr}");
            assert!(!stderr.contains("command not found"), "{stderr}");
            assert!(
                stdout.contains("user-hook-ok"),
                "{declaration}: {stdout} {stderr}"
            );
        }
    }
}

#[test]
fn raw_wire_input_bypasses_text_policies_and_only_escapes_telnet_iac() {
    for raw_tcp in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let expected = if raw_tcp {
            b"\r\n\x7f\xff\x80".to_vec()
        } else {
            b"\r\n\x7f\xff\xff\x80".to_vec()
        };
        let size = expected.len();
        let (tx, rx) = mpsc::channel();
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut bytes = vec![0; size];
            stream.read_exact(&mut bytes).unwrap();
            tx.send(bytes).unwrap();
        });
        let manager = SessionManager::new();
        let info = manager
            .create_telnet_session(TelnetSessionConfig {
                host: "127.0.0.1".into(),
                port,
                raw_tcp,
                backspace_mode: "bs".into(),
                local_line_edit: true,
                local_echo: true,
                encoding: "gbk".into(),
                send_naws: false,
                send_sga: false,
                force_character_at_a_time: false,
                ..Default::default()
            })
            .unwrap();
        manager.write_raw(&info.id, b"\r\n\x7f\xff\x80").unwrap();
        let actual = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        manager.close(&info.id).unwrap();
        peer.join().unwrap();
        assert_eq!(actual, expected);
    }
}

// A mock writer exposes completed write calls without depending on TCP buffering.
struct TimedWriteCapture(mpsc::Sender<(Vec<u8>, Instant)>);

impl Write for TimedWriteCapture {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.send((data.to_vec(), Instant::now())).unwrap();
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn test_telnet_writer<W: Write + Send + 'static>(
    output: W,
    interval: Option<Duration>,
    flush_policy: crate::WriterFlushPolicy,
    events: SessionEventQueue,
) -> QueuedTransportWriter {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let _peer = listener.accept().unwrap();
    QueuedTransportWriter::spawn_with_telnet_policy(
        "keep-alive-test".into(),
        output,
        flush_policy,
        events,
        Some(crate::TelnetWriterPolicy {
            idle_interval: interval,
            lifecycle: Arc::new(crate::TelnetWriterLifecycle {
                stopped: std::sync::atomic::AtomicBool::new(false),
                stream,
            }),
        }),
    )
    .unwrap()
}

#[test]
fn telnet_writer_waits_a_full_interval_between_raw_nops() {
    let (tx, rx) = mpsc::channel();
    let interval = Duration::from_millis(150);
    let started = Instant::now();
    let mut writer = test_telnet_writer(
        TimedWriteCapture(tx),
        Some(interval),
        crate::WriterFlushPolicy::EachByte,
        SessionEventQueue::new(),
    );
    let (first, first_at) = rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let (second, second_at) = rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_eq!(first, [0xff, 0xf1]);
    assert_eq!(second, first);
    assert!(first_at.duration_since(started) >= interval);
    assert!(second_at.duration_since(first_at) >= interval);
    let closed_at = Instant::now();
    writer.close();
    assert!(closed_at.elapsed() < Duration::from_secs(1));
    assert!(matches!(
        rx.recv_timeout(interval),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
}

#[test]
fn telnet_writer_resets_idle_after_application_control_and_login_sends() {
    let (tx, rx) = mpsc::channel();
    let interval = Duration::from_millis(250);
    let mut writer = test_telnet_writer(
        TimedWriteCapture(tx),
        Some(interval),
        crate::WriterFlushPolicy::EachByte,
        SessionEventQueue::new(),
    );
    let mut last_sent_at = Instant::now();
    for (payload, control) in [
        (b"x".to_vec(), false),
        (vec![IAC, DO, 3], true),
        (b"login\r".to_vec(), true),
    ] {
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        if control {
            writer.write_telnet_control(payload.clone()).unwrap();
        } else {
            writer.write(payload.clone()).unwrap();
        }
        let (sent, sent_at) = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        last_sent_at = sent_at;
        assert_eq!(sent, payload);
    }
    let (nop, nop_at) = rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(nop_at.duration_since(last_sent_at) >= interval);
    assert_eq!(nop, crate::telnet_codec::NOP);
    writer.close();
}

#[test]
fn telnet_writer_empty_commands_do_not_reset_idle() {
    let (tx, rx) = mpsc::channel();
    let mut writer = test_telnet_writer(
        TimedWriteCapture(tx),
        Some(Duration::from_millis(500)),
        crate::WriterFlushPolicy::AfterWrite,
        SessionEventQueue::new(),
    );
    assert!(matches!(
        rx.recv_timeout(Duration::from_millis(250)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    writer
        .command_tx
        .send(crate::TransportWriterCommand::Write(Vec::new()))
        .unwrap();
    let (nop, _) = rx.recv_timeout(Duration::from_millis(400)).unwrap();
    assert_eq!(nop, crate::telnet_codec::NOP);
    writer.close();
}

#[test]
fn telnet_writer_without_keepalive_blocks_until_close() {
    let (tx, rx) = mpsc::channel();
    let mut writer = test_telnet_writer(
        TimedWriteCapture(tx),
        None,
        crate::WriterFlushPolicy::AfterWrite,
        SessionEventQueue::new(),
    );
    assert!(matches!(
        rx.recv_timeout(Duration::from_millis(150)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    let closed_at = Instant::now();
    writer.close();
    assert!(closed_at.elapsed() < Duration::from_secs(1));
}

#[test]
fn telnet_writer_failure_reports_error_and_stops_lifecycle() {
    struct FailingWriter;
    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "test write failure",
            ))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let events = SessionEventQueue::new();
    let mut writer = test_telnet_writer(
        FailingWriter,
        Some(Duration::from_millis(20)),
        crate::WriterFlushPolicy::AfterWrite,
        events.clone(),
    );
    wait_for_queue_state(
        || writer.worker_thread.as_ref().unwrap().is_finished(),
        "failed Telnet writer to exit",
    );
    assert!(writer.telnet_lifecycle.as_ref().unwrap().is_stopped());
    assert!(matches!(
        events.drain(10).events.as_slice(),
        [SessionEvent::Error { .. }]
    ));
    writer.close();
}

fn loopback_telnet_session(
    raw_tcp: bool,
    interval_secs: u32,
) -> (SessionManager, super::SessionInfo, std::net::TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let manager = SessionManager::new();
    let info = manager
        .create_telnet_session(TelnetSessionConfig {
            host: "127.0.0.1".into(),
            port: listener.local_addr().unwrap().port(),
            raw_tcp,
            keep_alive_interval_secs: interval_secs,
            send_naws: false,
            auto_login: crate::TelnetAutoLoginConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    let (peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    (manager, info, peer)
}

#[test]
fn telnet_session_sends_nops_while_receiving_output_and_stops_on_close() {
    let started = Instant::now();
    let (manager, info, mut peer) = loopback_telnet_session(false, 1);
    peer.set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let mut received = Vec::new();
    let mut nop_times = Vec::new();
    while nop_times.len() < 2 {
        assert!(started.elapsed() < Duration::from_secs(6), "NOP deadline");
        peer.write_all(b"server output").unwrap();
        let mut buf = [0; 2];
        match peer.read(&mut buf) {
            Ok(0) => panic!("unexpected EOF"),
            Ok(count) => {
                received.extend_from_slice(&buf[..count]);
                if received.len() >= (nop_times.len() + 1) * 2 {
                    nop_times.push(Instant::now());
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(error) => panic!("{error}"),
        }
    }
    assert_eq!(received, [0xff, 0xf1, 0xff, 0xf1]);
    assert!(nop_times[0].duration_since(started) >= Duration::from_secs(1));
    assert!(nop_times[1].duration_since(nop_times[0]) >= Duration::from_millis(900));
    let closed_at = Instant::now();
    manager.close(&info.id).unwrap();
    assert!(closed_at.elapsed() < Duration::from_secs(1));
    assert_eq!(peer.read(&mut [0; 8]).unwrap(), 0);
}

#[test]
fn telnet_disabled_and_raw_tcp_sessions_never_send_nops() {
    for (raw_tcp, interval_secs) in [(false, 0), (true, 1)] {
        let (manager, info, mut peer) = loopback_telnet_session(raw_tcp, interval_secs);
        peer.set_read_timeout(Some(Duration::from_millis(1250)))
            .unwrap();
        let error = peer.read(&mut [0; 8]).unwrap_err();
        assert!(matches!(
            error.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ));
        manager.close(&info.id).unwrap();
    }
}

#[test]
fn telnet_peer_disconnect_stops_writer_even_before_manager_removes_session() {
    let (manager, info, peer) = loopback_telnet_session(false, 1);
    drop(peer);
    wait_for_queue_state(
        || {
            let sessions = manager.sessions.lock().unwrap();
            let Some(crate::ManagedSession::Tcp(session)) = sessions.get(&info.id) else {
                panic!("TCP session");
            };
            session.writer.worker_thread.as_ref().unwrap().is_finished()
                && session.reader_thread.as_ref().unwrap().is_finished()
        },
        "Telnet reader and writer after peer EOF",
    );
    assert!(
        manager
            .drain_events(10)
            .unwrap()
            .events
            .iter()
            .any(|event| matches!(event, SessionEvent::Exited { .. }))
    );
    manager.close(&info.id).unwrap();
}

#[test]
fn dropping_telnet_manager_interrupts_idle_writer() {
    let (manager, _, mut peer) = loopback_telnet_session(false, 60);
    let started = Instant::now();
    drop(manager);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(peer.read(&mut [0; 8]).unwrap(), 0);
}

#[test]
fn telnet_slow_write_starts_a_new_idle_interval_instead_of_catching_up() {
    struct FirstWriteGate {
        started: mpsc::Sender<()>,
        release: Option<mpsc::Receiver<()>>,
    }
    impl Write for FirstWriteGate {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.started.send(()).unwrap();
            if let Some(release) = self.release.take() {
                release.recv().unwrap();
            }
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let interval = Duration::from_millis(100);
    let mut writer = test_telnet_writer(
        FirstWriteGate {
            started: started_tx,
            release: Some(release_rx),
        },
        Some(interval),
        crate::WriterFlushPolicy::AfterWrite,
        SessionEventQueue::new(),
    );
    writer.write(b"x".to_vec()).unwrap();
    started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(matches!(
        started_rx.recv_timeout(interval * 2),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    release_tx.send(()).unwrap();
    assert!(matches!(
        started_rx.recv_timeout(Duration::from_millis(50)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    writer.close();
}

#[test]
fn telnet_single_writer_keeps_naws_negotiation_login_and_escaped_input_intact() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let config = TelnetSessionConfig {
        host: "127.0.0.1".into(),
        port: listener.local_addr().unwrap().port(),
        username: "operator".into(),
        force_character_at_a_time: true,
        keep_alive_interval_secs: 0,
        auto_login: crate::TelnetAutoLoginConfig {
            send_wake_enter: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let manager = SessionManager::new();
    let info = manager.create_telnet_session(config.clone()).unwrap();
    let (mut peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    peer.write_all(&[IAC, WILL, OPT_SUPPRESS_GO_AHEAD]).unwrap();
    peer.write_all(b"Username: ").unwrap();
    manager.write_raw(&info.id, b"a\xffb").unwrap();
    manager.resize(&info.id, 100, 40).unwrap();

    let mut packets = vec![
        crate::telnet_codec::maybe_build_naws(80, 24, &config).unwrap(),
        crate::telnet_codec::maybe_build_naws(100, 40, &config).unwrap(),
        vec![IAC, DO, OPT_SUPPRESS_GO_AHEAD],
        b"operator\r".to_vec(),
        b"a\xff\xffb".to_vec(),
    ];
    let mut received = vec![0; packets.iter().map(Vec::len).sum()];
    peer.read_exact(&mut received).unwrap();
    let mut remaining = received.as_slice();
    while !remaining.is_empty() {
        let index = packets
            .iter()
            .position(|packet| remaining.starts_with(packet))
            .expect("whole queued packet");
        remaining = &remaining[packets.remove(index).len()..];
    }
    assert!(packets.is_empty());
    manager.close(&info.id).unwrap();
}

#[test]
fn telnet_local_line_buffer_does_not_delay_idle_nop() {
    let (manager, info, mut peer) = loopback_telnet_session(false, 1);
    {
        let mut sessions = manager.sessions.lock().unwrap();
        let Some(crate::ManagedSession::Tcp(session)) = sessions.get_mut(&info.id) else {
            panic!("TCP session");
        };
        session.config.local_line_edit = true;
    }
    let started = Instant::now();
    peer.set_read_timeout(Some(Duration::from_millis(600)))
        .unwrap();
    assert!(peer.read(&mut [0; 2]).is_err());
    manager.write(&info.id, b"buffered").unwrap();
    peer.set_read_timeout(Some(Duration::from_millis(800)))
        .unwrap();
    let mut nop = [0; 2];
    peer.read_exact(&mut nop).unwrap();
    assert_eq!(nop, crate::telnet_codec::NOP);
    assert!(started.elapsed() < Duration::from_millis(1600));
    manager.close(&info.id).unwrap();
}

#[cfg(windows)]
#[test]
fn local_connection_worker_failures_roll_back_without_registered_sessions() {
    for stage in 0..4 {
        let manager = SessionManager::new();
        let _failure = nyaterm_core::runtime::worker::fail_after(stage);
        let result = manager.create_local_session(LocalSessionConfig {
            shell_path: Some(super::default_local_shell_path()),
            ..Default::default()
        });
        assert!(
            result.is_err(),
            "startup stage {stage} must return an error"
        );
        assert!(manager.sessions.lock().unwrap().is_empty());
    }
}

#[test]
fn transfer_backpressure_retries_without_reordering_and_is_cancellable() {
    for cancel in [false, true] {
        let manager = Arc::new(SessionManager::new());
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let output = Arc::new(Mutex::new(Vec::new()));
        let writer = QueuedTransportWriter::spawn(
            "transfer-backpressure".into(),
            GatedWriter {
                started_tx,
                release_rx,
                output: output.clone(),
            },
            crate::WriterFlushPolicy::Never,
            SessionEventQueue::new(),
        )
        .unwrap();
        writer
            .write(vec![0; crate::writer_mailbox::BYTE_LIMIT])
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let cancellation = Arc::new(std::sync::atomic::AtomicBool::new(false));
        // Exercise the retry API with a managed local-like writer without creating
        // a process: the Serial adapter uses the same raw write mailbox.
        manager.sessions.lock().unwrap().insert(
            "transfer-backpressure".into(),
            crate::ManagedSession::Serial(crate::SerialTransport {
                info: crate::SessionInfo {
                    id: "transfer-backpressure".into(),
                    name: String::new(),
                    kind: crate::SessionKind::Serial,
                    working_dir: None,
                    cols: 80,
                    rows: 24,
                },
                writer,
                backspace_as_bs: false,
                stop_reader: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                reader_thread: None,
                flow_control: nyaterm_core::models::connection::SerialFlowControl::None,
            }),
        );
        let worker_manager = manager.clone();
        let worker_cancel = cancellation.clone();
        let (done, result) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            done.send(
                worker_manager
                    .write_raw_cancellable("transfer-backpressure", b"second", &worker_cancel)
                    .is_ok(),
            )
            .unwrap();
        });
        assert!(result.recv_timeout(Duration::from_millis(30)).is_err());
        if cancel {
            cancellation.store(true, std::sync::atomic::Ordering::Release);
            assert!(!result.recv_timeout(Duration::from_secs(1)).unwrap());
            release_tx.send(()).unwrap();
        } else {
            release_tx.send(()).unwrap();
            assert!(result.recv_timeout(Duration::from_secs(1)).unwrap());
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            release_tx.send(()).unwrap();
            // Wait for the second accepted write before closing (which discards pending data).
            let deadline = Instant::now() + Duration::from_secs(1);
            while output.lock().unwrap().len() < crate::writer_mailbox::BYTE_LIMIT + 6 {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
        }
        worker.join().unwrap();
        manager.close("transfer-backpressure").unwrap();
        assert_eq!(
            output.lock().unwrap().len(),
            crate::writer_mailbox::BYTE_LIMIT + if cancel { 0 } else { 6 }
        );
        if !cancel {
            assert!(output.lock().unwrap().ends_with(b"second"));
        }
    }
}

#[cfg(windows)]
#[test]
fn detached_local_close_returns_before_a_full_pipe_write_finishes() {
    let manager = SessionManager::new();
    let info = manager
        .create_local_session(LocalSessionConfig {
            shell_path: Some("cmd.exe".into()),
            shell_args: vec!["/d".into(), "/c".into(), "ping -n 30 127.0.0.1 >nul".into()],
            ..Default::default()
        })
        .unwrap();
    manager
        .write_raw(&info.id, &vec![b'x'; 4 * 1024 * 1024])
        .unwrap();
    std::thread::sleep(Duration::from_millis(50));
    let started = Instant::now();
    manager.close_detached(&info.id).unwrap();
    assert!(started.elapsed() < Duration::from_millis(250));
    assert!(!manager.sessions.lock().unwrap().contains_key(&info.id));
    // A later session must still start while the former ConPTY is reclaimed.
    let next = manager
        .create_local_session(LocalSessionConfig {
            shell_path: Some("cmd.exe".into()),
            ..Default::default()
        })
        .unwrap();
    manager.close(&next.id).unwrap();
}

#[test]
fn output_boundary_preserves_source_order_across_budgeted_drains() {
    let manager = SessionManager::new();
    let consumer = manager.register_event_consumer();
    manager.assign_session_event_consumer("terminal", consumer);
    manager.event_queue.push(SessionEvent::Output {
        session_id: "terminal".into(),
        data: b"before".to_vec(),
    });
    manager.enqueue_output_boundary("terminal", "task");
    manager.event_queue.push(SessionEvent::Output {
        session_id: "terminal".into(),
        data: b"after".to_vec(),
    });
    let mut events = Vec::new();
    for _ in 0..3 {
        events.extend(
            manager
                .drain_events_blocking_for_consumer_with_output_budget(
                    consumer,
                    1,
                    1024,
                    Duration::ZERO,
                )
                .unwrap()
                .events,
        );
    }
    assert!(matches!(&events[0], SessionEvent::Output { data, .. } if data == b"before"));
    assert!(matches!(&events[1], SessionEvent::OutputBoundary { token, .. } if token == "task"));
    assert!(matches!(&events[2], SessionEvent::Output { data, .. } if data == b"after"));
}

#[test]
fn async_output_backpressure_keeps_executor_responsive_and_resumes_in_order() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let queue = SessionEventQueue::new();
        queue.push(SessionEvent::Output {
            session_id: "async".into(),
            data: vec![7; SESSION_EVENT_QUEUE_OUTPUT_LIMIT],
        });
        let producer = queue.clone();
        let task = tokio::spawn(async move {
            producer
                .push_async(SessionEvent::Output {
                    session_id: "async".into(),
                    data: vec![9; 1024],
                })
                .await;
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!task.is_finished());
        let first = queue.drain(1000);
        assert_eq!(
            first.stats.drained_output_bytes,
            SESSION_EVENT_QUEUE_OUTPUT_LIMIT
        );
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        let next = queue.drain(1000);
        assert!(
            matches!(&next.events[0], SessionEvent::Output { data, .. } if data == &vec![9; 1024])
        );
        assert_eq!(queue.peak_output_bytes(), SESSION_EVENT_QUEUE_OUTPUT_LIMIT);
    });
}

#[test]
fn async_output_wait_and_producer_turn_are_released_by_cancellation() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let queue = SessionEventQueue::new();
            queue.push(SessionEvent::Output {
                session_id: "a".into(),
                data: vec![0; SESSION_EVENT_QUEUE_OUTPUT_LIMIT],
            });
            let mut tasks = Vec::new();
            for _ in 0..8 {
                let producer = queue.clone();
                tasks.push(tokio::spawn(async move {
                    producer
                        .push_async(SessionEvent::Output {
                            session_id: "a".into(),
                            data: vec![1; 1024],
                        })
                        .await;
                }));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
            queue.cancel_session("a");
            for task in tasks {
                tokio::time::timeout(Duration::from_secs(2), task)
                    .await
                    .unwrap()
                    .unwrap();
            }
            assert!(queue.drain(100).events.is_empty());
        });
}
