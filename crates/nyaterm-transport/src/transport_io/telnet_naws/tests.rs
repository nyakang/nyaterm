use std::io::ErrorKind;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, TryLockError, mpsc};
use std::time::Duration;

use super::{NawsOperation, TelnetNawsWriter};
use crate::session::{ManagedSession, SessionManager};
use crate::session_config::TelnetSessionConfig;
use crate::session_types::SessionInfo;
use crate::telnet_codec::{DO, DONT, TelnetCommandDecoder};
use crate::transport_io::TransportWriterCommand;
use crate::writer_mailbox::{BYTE_LIMIT, WriterMailbox};

fn new_writer() -> TelnetNawsWriter {
    TelnetNawsWriter::new(WriterMailbox::default(), &TelnetSessionConfig::default())
}

fn packets(writer: &TelnetNawsWriter) -> Vec<Vec<u8>> {
    let mut packets = Vec::new();
    while let Some((command, _permit)) = writer.command_tx.receive(Some(Duration::ZERO)).unwrap() {
        match command {
            TransportWriterCommand::TelnetControl(data) => packets.push(data),
            TransportWriterCommand::Write(_) => panic!("unexpected application data"),
        }
    }
    packets
}

fn enable(writer: &TelnetNawsWriter) {
    writer.offer().unwrap();
    writer.negotiate(DO).unwrap();
    assert_eq!(
        packets(writer),
        [
            vec![255, 251, 31],
            vec![255, 250, 31, 0, 80, 0, 24, 255, 240]
        ]
    );
}

#[test]
fn disabled_policy_refuses_naws_and_raw_tcp_never_emits_control_bytes() {
    for raw_tcp in [false, true] {
        let writer = TelnetNawsWriter::new(
            WriterMailbox::default(),
            &TelnetSessionConfig {
                raw_tcp,
                send_naws: false,
                ..Default::default()
            },
        );
        writer.offer().unwrap();
        writer.resize(255, 255).unwrap();
        writer.negotiate(DO).unwrap();
        writer.negotiate(DONT).unwrap();
        writer.resize(65535, 65535).unwrap();
        assert_eq!(
            packets(&writer),
            if raw_tcp {
                vec![]
            } else {
                vec![vec![255, 252, 31]]
            }
        );
    }
    let raw = TelnetNawsWriter::new(
        WriterMailbox::default(),
        &TelnetSessionConfig {
            raw_tcp: true,
            send_naws: true,
            ..Default::default()
        },
    );
    raw.offer().unwrap();
    raw.negotiate(DO).unwrap();
    raw.resize(255, 255).unwrap();
    assert!(packets(&raw).is_empty());
}

#[test]
fn fragmented_commands_only_change_naws_after_the_option_byte_arrives() {
    let writer = new_writer();
    writer.offer().unwrap();
    packets(&writer);
    let mut decoder = TelnetCommandDecoder::default();
    for (command, expected) in [
        (DO, vec![vec![255, 250, 31, 0, 100, 0, 40, 255, 240]]),
        (DO, vec![]),
        (DONT, vec![vec![255, 252, 31]]),
        (DONT, vec![]),
    ] {
        for byte in [255, command] {
            assert!(
                decoder
                    .decode(&[byte], &mut |_, _| panic!("incomplete command"))
                    .is_empty()
            );
            assert!(packets(&writer).is_empty());
        }
        if command == DO {
            // Resize while the first DO is incomplete: acceptance must use
            // the cached size rather than the size at the IAC prefix.
            writer.resize(100, 40).unwrap();
            packets(&writer);
        }
        assert!(
            decoder
                .decode(&[31], &mut |command, _| writer.negotiate(command).unwrap())
                .is_empty()
        );
        assert_eq!(packets(&writer), expected);
    }
}

#[test]
fn transition_and_enqueue_keep_acceptance_and_resize_in_order() {
    assert_serialized(
        NawsOperation::Negotiate(DO),
        NawsOperation::Resize(100, 40),
        false,
        vec![
            vec![255, 250, 31, 0, 80, 0, 24, 255, 240],
            vec![255, 250, 31, 0, 100, 0, 40, 255, 240],
        ],
    );
}

#[test]
fn disable_prevents_a_competing_resize_from_queuing_after_wont() {
    assert_serialized(
        NawsOperation::Negotiate(DONT),
        NawsOperation::Resize(100, 40),
        true,
        vec![vec![255, 252, 31]],
    );
}

#[test]
fn already_queued_resize_precedes_a_competing_disable() {
    assert_serialized(
        NawsOperation::Resize(100, 40),
        NawsOperation::Negotiate(DONT),
        true,
        vec![
            vec![255, 250, 31, 0, 100, 0, 40, 255, 240],
            vec![255, 252, 31],
        ],
    );
}

fn assert_serialized(
    first: NawsOperation,
    second: NawsOperation,
    enabled: bool,
    expected: Vec<Vec<u8>>,
) {
    let writer = new_writer();
    if enabled {
        enable(&writer);
    } else {
        writer.offer().unwrap();
        packets(&writer);
    }
    let (generated_tx, generated_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let first_writer = writer.clone();
    let first_thread = std::thread::spawn(move || {
        first_writer.apply(first, |payload| {
            generated_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            first_writer
                .command_tx
                .send(TransportWriterCommand::TelnetControl(payload))
        })
    });
    generated_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    // The message has been generated, but cannot escape the critical section
    // before it is queued. This is the window that regressed in PR #1205.
    assert!(matches!(
        writer.state.try_lock(),
        Err(TryLockError::WouldBlock)
    ));
    let (attempted_tx, attempted_rx) = mpsc::channel();
    let second_writer = writer.clone();
    let second_thread = std::thread::spawn(move || {
        attempted_tx.send(()).unwrap();
        second_writer.send(second)
    });
    attempted_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    release_tx.send(()).unwrap();
    first_thread.join().unwrap().unwrap();
    second_thread.join().unwrap().unwrap();
    assert_eq!(packets(&writer), expected);
    if enabled {
        writer.negotiate(DO).unwrap();
        assert_eq!(
            packets(&writer),
            [vec![255, 251, 31, 255, 250, 31, 0, 100, 0, 40, 255, 240]]
        );
    } else {
        writer.negotiate(DO).unwrap();
        assert!(packets(&writer).is_empty());
    }
}

#[test]
fn full_queue_preserves_resize_until_a_successful_retry() {
    let writer = new_writer();
    enable(&writer);
    writer
        .command_tx
        .send(TransportWriterCommand::Write(vec![0; BYTE_LIMIT]))
        .unwrap();
    assert_eq!(
        writer.resize(100, 40).unwrap_err().kind(),
        ErrorKind::WouldBlock
    );
    let (_, permit) = writer.command_tx.receive(None).unwrap().unwrap();
    drop(permit);
    writer.negotiate(DONT).unwrap();
    writer.negotiate(DO).unwrap();
    assert_eq!(
        packets(&writer),
        [
            vec![255, 252, 31],
            vec![255, 251, 31, 255, 250, 31, 0, 80, 0, 24, 255, 240]
        ]
    );
    writer.resize(100, 40).unwrap();
    assert_eq!(
        packets(&writer),
        [vec![255, 250, 31, 0, 100, 0, 40, 255, 240]]
    );
}

#[test]
fn failed_offer_or_acceptance_does_not_leave_a_pending_or_enabled_state() {
    for operation in [NawsOperation::Offer, NawsOperation::Negotiate(DO)] {
        let writer = new_writer();
        if matches!(operation, NawsOperation::Negotiate(..)) {
            writer.offer().unwrap();
            packets(&writer);
        }
        writer
            .command_tx
            .send(TransportWriterCommand::Write(vec![0; BYTE_LIMIT]))
            .unwrap();
        assert_eq!(
            writer.send(operation).unwrap_err().kind(),
            ErrorKind::WouldBlock
        );
        let (_, permit) = writer.command_tx.receive(None).unwrap().unwrap();
        drop(permit);
        writer.resize(100, 40).unwrap();
        assert!(packets(&writer).is_empty());
        writer.negotiate(DO).unwrap();
        assert_eq!(
            packets(&writer),
            [vec![255, 251, 31, 255, 250, 31, 0, 100, 0, 40, 255, 240]]
        );
    }
}

#[test]
fn failed_disable_acknowledgement_still_disables_size_sends() {
    let writer = new_writer();
    enable(&writer);
    writer
        .command_tx
        .send(TransportWriterCommand::Write(vec![0; BYTE_LIMIT]))
        .unwrap();
    assert_eq!(
        writer.negotiate(DONT).unwrap_err().kind(),
        ErrorKind::WouldBlock
    );
    let (_, permit) = writer.command_tx.receive(None).unwrap().unwrap();
    drop(permit);
    writer.resize(100, 40).unwrap();
    writer.negotiate(DONT).unwrap();
    assert!(packets(&writer).is_empty());
}

#[test]
fn stopped_mailbox_returns_errors_without_false_resize_success() {
    let writer = new_writer();
    enable(&writer);
    writer.command_tx.stop();
    assert_eq!(
        writer.resize(100, 40).unwrap_err().kind(),
        ErrorKind::BrokenPipe
    );
    let mut snapshot = *writer.state.lock().unwrap();
    snapshot.reject();
    assert_eq!(
        snapshot.accept(),
        [255, 251, 31, 255, 250, 31, 0, 80, 0, 24, 255, 240]
    );
    assert_eq!(
        writer.negotiate(DONT).unwrap_err().kind(),
        ErrorKind::BrokenPipe
    );
    writer.resize(100, 40).unwrap();
    let starting = new_writer();
    starting.command_tx.stop();
    assert_eq!(starting.offer().unwrap_err().kind(), ErrorKind::BrokenPipe);
}

#[test]
fn poisoned_state_returns_an_error_instead_of_panicking() {
    let writer = new_writer();
    let state = writer.state.clone();
    let _ = std::thread::spawn(move || {
        let _guard = state.lock().unwrap();
        panic!("poison test state");
    })
    .join();
    assert_eq!(writer.resize(100, 40).unwrap_err().kind(), ErrorKind::Other);
    assert_eq!(writer.negotiate(DONT).unwrap_err().kind(), ErrorKind::Other);
}

fn loopback(mut config: TelnetSessionConfig) -> (SessionManager, SessionInfo, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    config.host = "127.0.0.1".into();
    config.port = listener.local_addr().unwrap().port();
    config.keep_alive_interval_secs = 0;
    let manager = SessionManager::new();
    let info = manager.create_telnet_session(config).unwrap();
    let (peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    (manager, info, peer)
}

fn expect_wire(peer: &mut TcpStream, expected: &[u8]) {
    let mut actual = vec![0; expected.len()];
    peer.read_exact(&mut actual).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn refused_naws_allows_login_and_reacceptance_reports_latest_escaped_size() {
    let (manager, info, mut peer) = loopback(TelnetSessionConfig {
        username: "operator".into(),
        auto_login: crate::session_config::TelnetAutoLoginConfig {
            send_wake_enter: false,
            ..Default::default()
        },
        ..Default::default()
    });
    expect_wire(&mut peer, &[255, 251, 31]);
    manager.resize(&info.id, 100, 40).unwrap();
    // SGA's response is a stream fence: refusal must not produce extra bytes
    // before that response, and login must proceed without NAWS acceptance.
    peer.write_all(&[255, 254, 31, 255, 254, 31, 255, 251, 3])
        .unwrap();
    peer.write_all(b"Username: ").unwrap();
    expect_wire(&mut peer, &[255, 253, 3]);
    expect_wire(&mut peer, b"operator\r");
    manager.resize(&info.id, 255, 255).unwrap();
    // TCP may coalesce these writes; the decoder test separately fixes every
    // byte boundary and the size change during an incomplete command.
    for byte in [255, 253, 31] {
        peer.write_all(&[byte]).unwrap();
    }
    expect_wire(
        &mut peer,
        &[
            255, 251, 31, 255, 250, 31, 0, 255, 255, 0, 255, 255, 255, 240,
        ],
    );
    manager.write_raw(&info.id, b"x").unwrap();
    expect_wire(&mut peer, b"x");
    manager.close(&info.id).unwrap();
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
}

#[test]
fn repeated_do_dont_and_resize_keep_control_frames_in_protocol_order() {
    let (manager, info, mut peer) = loopback(TelnetSessionConfig {
        force_character_at_a_time: true,
        auto_login: crate::session_config::TelnetAutoLoginConfig {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    });
    expect_wire(&mut peer, &[255, 251, 31]);
    peer.write_all(&[255, 253, 31]).unwrap();
    expect_wire(&mut peer, &[255, 250, 31, 0, 80, 0, 24, 255, 240]);
    for cols in [100, 255, 256] {
        peer.write_all(&[255, 253, 31, 255, 254, 31, 255, 254, 31, 255, 251, 3])
            .unwrap();
        expect_wire(&mut peer, &[255, 252, 31, 255, 253, 3]);
        manager.resize(&info.id, cols, 40).unwrap();
        manager.write_raw(&info.id, b"a\xffb").unwrap();
        expect_wire(&mut peer, b"a\xff\xffb");
        peer.write_all(&[255, 253, 31, 255, 253, 31, 255, 251, 3])
            .unwrap();
        let expected: &[u8] = match cols {
            100 => &[
                255, 251, 31, 255, 250, 31, 0, 100, 0, 40, 255, 240, 255, 253, 3,
            ],
            255 => &[
                255, 251, 31, 255, 250, 31, 0, 255, 255, 0, 40, 255, 240, 255, 253, 3,
            ],
            256 => &[
                255, 251, 31, 255, 250, 31, 1, 0, 0, 40, 255, 240, 255, 253, 3,
            ],
            _ => unreachable!(),
        };
        expect_wire(&mut peer, expected);
    }
    manager.close(&info.id).unwrap();
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
}

#[test]
fn failed_resize_preserves_session_dimensions_and_close_reaps_workers() {
    let (manager, info, mut peer) = loopback(TelnetSessionConfig {
        auto_login: crate::session_config::TelnetAutoLoginConfig {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    });
    expect_wire(&mut peer, &[255, 251, 31]);
    peer.write_all(&[255, 253, 31]).unwrap();
    expect_wire(&mut peer, &[255, 250, 31, 0, 80, 0, 24, 255, 240]);
    {
        let sessions = manager.sessions.lock().unwrap();
        let Some(ManagedSession::Tcp(session)) = sessions.get(&info.id) else {
            panic!("Telnet session missing");
        };
        session.writer.command_tx.stop();
    }
    assert!(manager.resize(&info.id, 100, 40).is_err());
    assert_eq!(
        manager.list_sessions().unwrap().as_slice(),
        std::slice::from_ref(&info)
    );
    let manager = Arc::new(manager);
    let closing_manager = manager.clone();
    let (closed_tx, closed_rx) = mpsc::channel();
    let closing = std::thread::spawn(move || {
        closed_tx.send(closing_manager.close(&info.id)).unwrap();
    });
    closed_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    closing.join().unwrap();
    assert!(manager.list_sessions().unwrap().is_empty());
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
}
