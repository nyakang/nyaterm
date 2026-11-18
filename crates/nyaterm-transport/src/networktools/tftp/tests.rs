//! Loopback tests for the TFTP listener.

use std::fs;
use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc::{Receiver, sync_channel};
use std::time::Duration;

use nyaterm_core::models::networktools::TftpSettings;

use super::packet;
use super::server::TftpServer;
use crate::networktools::event::{NetworkToolEvent, TransferOutcome};
use crate::networktools::status::TransferSnapshot;

fn config(root: &std::path::Path, upload: bool, overwrite: bool) -> TftpSettings {
    TftpSettings {
        bind_address: "127.0.0.1".into(),
        port: 0,
        root_dir: root.to_string_lossy().into_owned(),
        block_size: 512,
        timeout_secs: 1,
        max_concurrency: 4,
        allow_upload: upload,
        allow_overwrite: overwrite,
    }
}

fn start(settings: TftpSettings) -> (TftpServer, SocketAddr, Receiver<NetworkToolEvent>) {
    let (events, receiver) = sync_channel(256);
    let transfers = std::sync::Arc::new(std::sync::Mutex::new(Vec::<TransferSnapshot>::new()));
    let server = TftpServer::start(settings, events, transfers, 1).expect("bind loopback");
    let addr = server.local_addr();
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(2)),
        Ok(NetworkToolEvent::Started { .. })
    ));
    (server, addr, receiver)
}

fn client() -> UdpSocket {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    socket
}

fn request(opcode: u16, name: &str) -> Vec<u8> {
    let mut out = opcode.to_be_bytes().to_vec();
    out.extend_from_slice(name.as_bytes());
    out.push(0);
    out.extend_from_slice(b"octet\0");
    out
}

#[test]
fn rrq_transfers_file_and_reports_completion() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("firmware.bin"), b"hello world").unwrap();
    let (mut server, addr, events) = start(config(dir.path(), false, false));

    let client = client();
    client
        .send_to(&request(packet::OP_RRQ, "firmware.bin"), addr)
        .unwrap();

    let mut buffer = [0u8; 1024];
    let (size, from) = client.recv_from(&mut buffer).unwrap();
    let (block, payload) = packet::parse_data(&buffer[..size]).unwrap();
    assert_eq!(block, 1);
    assert_eq!(payload, b"hello world");
    client.send_to(&packet::write_ack(block), from).unwrap();

    let finished = wait_for_finish(&events);
    assert!(matches!(finished, TransferOutcome::Completed { bytes: 11 }));

    server.stop();
}

#[test]
fn rrq_for_missing_file_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let (mut server, addr, _events) = start(config(dir.path(), false, false));

    let client = client();
    client
        .send_to(&request(packet::OP_RRQ, "missing.bin"), addr)
        .unwrap();

    let mut buffer = [0u8; 256];
    let (size, _) = client.recv_from(&mut buffer).unwrap();
    let (code, _) = packet::parse_error(&buffer[..size]).unwrap();
    assert_eq!(code, packet::ERR_FILE_NOT_FOUND);

    server.stop();
}

#[test]
fn wrq_is_rejected_when_upload_is_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let (mut server, addr, _events) = start(config(dir.path(), false, false));

    let client = client();
    client
        .send_to(&request(packet::OP_WRQ, "backup.cfg"), addr)
        .unwrap();

    let mut buffer = [0u8; 256];
    let (size, _) = client.recv_from(&mut buffer).unwrap();
    let (code, _) = packet::parse_error(&buffer[..size]).unwrap();
    assert_eq!(code, packet::ERR_ACCESS_VIOLATION);

    server.stop();
}

#[test]
fn wrq_uploads_when_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let (mut server, addr, events) = start(config(dir.path(), true, false));

    let client = client();
    client
        .send_to(&request(packet::OP_WRQ, "backup.cfg"), addr)
        .unwrap();

    let mut buffer = [0u8; 512];
    let (size, from) = client.recv_from(&mut buffer).unwrap();
    assert_eq!(packet::parse_ack(&buffer[..size]).unwrap(), 0);

    client
        .send_to(&packet::write_data(1, b"router config"), from)
        .unwrap();
    let (size, _) = client.recv_from(&mut buffer).unwrap();
    assert_eq!(packet::parse_ack(&buffer[..size]).unwrap(), 1);

    let finished = wait_for_finish(&events);
    assert!(matches!(finished, TransferOutcome::Completed { bytes: 13 }));

    let mut uploaded = None;
    for _ in 0..50 {
        if let Ok(contents) = fs::read(dir.path().join("backup.cfg")) {
            uploaded = Some(contents);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(uploaded.as_deref(), Some(&b"router config"[..]));

    server.stop();
}

#[test]
fn wrq_refuses_overwrite_by_default() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("backup.cfg"), b"existing").unwrap();
    let (mut server, addr, _events) = start(config(dir.path(), true, false));

    let client = client();
    client
        .send_to(&request(packet::OP_WRQ, "backup.cfg"), addr)
        .unwrap();

    let mut buffer = [0u8; 256];
    let (size, _) = client.recv_from(&mut buffer).unwrap();
    let (code, _) = packet::parse_error(&buffer[..size]).unwrap();
    assert_eq!(code, packet::ERR_FILE_EXISTS);

    server.stop();
}

#[test]
fn traversal_names_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (mut server, addr, _events) = start(config(dir.path(), false, false));

    let client = client();
    client
        .send_to(&request(packet::OP_RRQ, "../secret"), addr)
        .unwrap();

    let mut buffer = [0u8; 256];
    let (size, _) = client.recv_from(&mut buffer).unwrap();
    let (code, _) = packet::parse_error(&buffer[..size]).unwrap();
    assert_eq!(code, packet::ERR_ACCESS_VIOLATION);

    server.stop();
}

#[test]
fn download_abort_with_an_empty_error_text_keeps_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("firmware.bin"), vec![0u8; 2048]).unwrap();
    let (mut server, addr, events) = start(config(dir.path(), false, false));

    let client = client();
    client
        .send_to(&request(packet::OP_RRQ, "firmware.bin"), addr)
        .unwrap();
    let mut buffer = [0u8; 1024];
    let (size, from) = client.recv_from(&mut buffer).unwrap();
    packet::parse_data(&buffer[..size]).unwrap();
    client
        .send_to(&packet::write_error(packet::ERR_NOT_DEFINED, ""), from)
        .unwrap();

    match wait_for_finish(&events) {
        TransferOutcome::Failed { message } => {
            assert!(!message.trim().is_empty(), "reason must never be empty");
            assert!(message.contains("aborted"), "unexpected reason: {message}");
        }
        TransferOutcome::Completed { .. } => panic!("expected a failed transfer"),
    }

    server.stop();
}

#[test]
fn upload_rejection_with_an_empty_error_text_keeps_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let (mut server, addr, events) = start(config(dir.path(), true, false));

    let client = client();
    client
        .send_to(&request(packet::OP_WRQ, "backup.cfg"), addr)
        .unwrap();
    let mut buffer = [0u8; 512];
    let (size, from) = client.recv_from(&mut buffer).unwrap();
    assert_eq!(packet::parse_ack(&buffer[..size]).unwrap(), 0);
    client
        .send_to(&packet::write_error(packet::ERR_ACCESS_VIOLATION, ""), from)
        .unwrap();

    match wait_for_finish(&events) {
        TransferOutcome::Failed { message } => {
            assert_eq!(message, "access denied");
        }
        TransferOutcome::Completed { .. } => panic!("expected a failed transfer"),
    }

    server.stop();
}

fn wait_for_finish(events: &Receiver<NetworkToolEvent>) -> TransferOutcome {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        match events.recv_timeout(Duration::from_millis(200)) {
            Ok(NetworkToolEvent::TransferFinished { outcome, .. }) => return outcome,
            Ok(_) => {}
            Err(_) => {}
        }
    }
    panic!("transfer did not finish");
}

fn request_with_options(opcode: u16, name: &str, options: &[(&str, &str)]) -> Vec<u8> {
    let mut bytes = request(opcode, name);
    for (name, value) in options {
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(value.as_bytes());
        bytes.push(0);
    }
    bytes
}

fn begin_upload(socket: &UdpSocket, addr: SocketAddr, name: &str) -> SocketAddr {
    socket
        .send_to(&request(packet::OP_WRQ, name), addr)
        .unwrap();
    let mut buffer = [0u8; 512];
    let (size, tid) = socket.recv_from(&mut buffer).unwrap();
    assert_eq!(packet::parse_ack(&buffer[..size]).unwrap(), 0);
    tid
}

#[test]
fn concurrent_uploads_cannot_replace_the_first_committed_file() {
    let dir = tempfile::tempdir().unwrap();
    let (mut server, addr, events) = start(config(dir.path(), true, false));
    let first = client();
    let second = client();
    // Both clients pass the initial existence check before either commits.
    let tid1 = begin_upload(&first, addr, "same.bin");
    let tid2 = begin_upload(&second, addr, "same.bin");
    first
        .send_to(&packet::write_data(1, b"first"), tid1)
        .unwrap();
    let mut buffer = [0u8; 512];
    let (size, _) = first.recv_from(&mut buffer).unwrap();
    assert_eq!(packet::parse_ack(&buffer[..size]).unwrap(), 1);
    // Receipt of the final ACK is already a visibility barrier for the file.
    assert_eq!(fs::read(dir.path().join("same.bin")).unwrap(), b"first");
    second
        .send_to(&packet::write_data(1, b"second"), tid2)
        .unwrap();
    let (size, _) = second.recv_from(&mut buffer).unwrap();
    assert_eq!(
        packet::parse_error(&buffer[..size]).unwrap().0,
        packet::ERR_FILE_EXISTS
    );
    let outcomes = [wait_for_finish(&events), wait_for_finish(&events)];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, TransferOutcome::Completed { .. }))
            .count(),
        1
    );
    server.stop();
    assert_eq!(fs::read(dir.path().join("same.bin")).unwrap(), b"first");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn commit_failure_sends_error_and_removes_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let (mut server, addr, events) = start(config(dir.path(), true, false));
    let client = client();
    let tid = begin_upload(&client, addr, "blocked.bin");
    fs::create_dir(dir.path().join("blocked.bin")).unwrap();
    client
        .send_to(&packet::write_data(1, b"data"), tid)
        .unwrap();
    let mut buffer = [0u8; 512];
    let (size, _) = client.recv_from(&mut buffer).unwrap();
    assert_eq!(packet::opcode(&buffer[..size]), Some(packet::OP_ERROR));
    assert!(matches!(
        wait_for_finish(&events),
        TransferOutcome::Failed { .. }
    ));
    server.stop();
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    assert!(dir.path().join("blocked.bin").is_dir());
}

#[test]
fn malformed_and_oversized_uploads_remove_temporary_files() {
    for invalid in [vec![0, 4, 0, 1], packet::write_data(1, &[1u8; 513])] {
        let dir = tempfile::tempdir().unwrap();
        let (mut server, addr, events) = start(config(dir.path(), true, false));
        let client = client();
        let tid = begin_upload(&client, addr, "partial.bin");
        client.send_to(&invalid, tid).unwrap();
        let mut buffer = [0u8; 512];
        let (size, _) = client.recv_from(&mut buffer).unwrap();
        assert_eq!(
            packet::parse_error(&buffer[..size]).unwrap().0,
            packet::ERR_ILLEGAL_OPERATION
        );
        assert!(matches!(
            wait_for_finish(&events),
            TransferOutcome::Failed { .. }
        ));
        server.stop();
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}

#[test]
fn final_data_is_reacknowledged_without_committing_twice() {
    let dir = tempfile::tempdir().unwrap();
    let (mut server, addr, _events) = start(config(dir.path(), true, false));
    let client = client();
    let tid = begin_upload(&client, addr, "last.bin");
    let mut buffer = [0u8; 512];
    for _ in 0..2 {
        client
            .send_to(&packet::write_data(1, b"done"), tid)
            .unwrap();
        let (size, from) = client.recv_from(&mut buffer).unwrap();
        assert_eq!(from, tid);
        assert_eq!(packet::parse_ack(&buffer[..size]).unwrap(), 1);
        assert_eq!(fs::read(dir.path().join("last.bin")).unwrap(), b"done");
    }
    server.stop();
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn stop_joins_uploads_without_waiting_for_negotiated_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let mut settings = config(dir.path(), true, false);
    settings.timeout_secs = 255;
    let (mut server, addr, events) = start(settings);
    let client = client();
    begin_upload(&client, addr, "cancelled.bin");
    let before = std::time::Instant::now();
    server.stop();
    assert!(before.elapsed() < Duration::from_secs(2));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    let events = events.try_iter().collect::<Vec<_>>();
    assert!(matches!(
        events.last(),
        Some(NetworkToolEvent::Stopped { .. })
    ));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, NetworkToolEvent::TransferFinished { .. }))
            .count(),
        1
    );
}

#[test]
fn lifecycle_events_survive_a_full_channel_and_stop_does_not_detach_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let settings = config(dir.path(), true, false);
    let (sender, events) = sync_channel(1);
    let transfers = std::sync::Arc::new(std::sync::Mutex::new(Vec::<TransferSnapshot>::new()));
    let observed = std::sync::Arc::clone(&transfers);
    let (ready, address) = sync_channel(1);
    let (stop, stop_requested) = sync_channel(1);
    let worker = std::thread::spawn(move || {
        let mut server = TftpServer::start(settings, sender, transfers, 7).unwrap();
        ready.send(server.local_addr()).unwrap();
        let _ = stop_requested.recv();
        server.stop();
    });
    let addr = address.recv_timeout(Duration::from_secs(3)).unwrap();
    let client = client();
    client
        .send_to(&request(packet::OP_WRQ, "queued.bin"), addr)
        .unwrap();
    // Leave Started in the capacity-one queue while the transfer registers.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while observed.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let registered = !observed.lock().unwrap().is_empty();
    stop.send(()).unwrap();
    let mut seen = Vec::new();
    while let Ok(event) = events.recv_timeout(Duration::from_secs(3)) {
        let stopped = matches!(event, NetworkToolEvent::Stopped { .. });
        seen.push(event);
        if stopped {
            break;
        }
    }
    drop(events); // Unblock producers even if an assertion fails below.
    worker.join().unwrap();
    assert!(registered);
    assert_eq!(seen.len(), 4);
    assert!(matches!(
        seen[0],
        NetworkToolEvent::Started { generation: 7, .. }
    ));
    assert!(matches!(
        seen[1],
        NetworkToolEvent::TransferStarted { generation: 7, .. }
    ));
    assert!(matches!(
        seen[2],
        NetworkToolEvent::TransferFinished { generation: 7, .. }
    ));
    assert!(matches!(
        seen[3],
        NetworkToolEvent::Stopped { generation: 7, .. }
    ));
    assert!(observed.lock().unwrap().is_empty());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn upload_oack_retries_use_the_negotiated_timeout_and_block_limit() {
    let dir = tempfile::tempdir().unwrap();
    let mut settings = config(dir.path(), true, false);
    settings.timeout_secs = 5;
    settings.block_size = 16;
    let (mut server, addr, _events) = start(settings);
    let client = client();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    client
        .send_to(
            &request_with_options(
                packet::OP_WRQ,
                "options.bin",
                &[("blksize", "64"), ("timeout", "1")],
            ),
            addr,
        )
        .unwrap();
    let mut buffer = [0u8; 512];
    let (size, tid) = client.recv_from(&mut buffer).unwrap();
    let oack = buffer[..size].to_vec();
    assert_eq!(packet::opcode(&oack), Some(packet::OP_OACK));
    assert!(
        oack.windows(11)
            .any(|window| window == b"blksize\x0016\x00")
    );
    let before = std::time::Instant::now();
    let (size, retry_tid) = client.recv_from(&mut buffer).unwrap();
    assert_eq!(tid, retry_tid);
    assert_eq!(&buffer[..size], &oack);
    assert!(before.elapsed() >= Duration::from_millis(700));
    client
        .send_to(&packet::write_data(1, &[3; 16]), tid)
        .unwrap();
    let (size, _) = client.recv_from(&mut buffer).unwrap();
    assert_eq!(packet::parse_ack(&buffer[..size]).unwrap(), 1);
    client.send_to(&packet::write_data(2, b"end"), tid).unwrap();
    let (size, _) = client.recv_from(&mut buffer).unwrap();
    assert_eq!(packet::parse_ack(&buffer[..size]).unwrap(), 2);
    assert_eq!(
        fs::read(dir.path().join("options.bin")).unwrap(),
        [vec![3; 16], b"end".to_vec()].concat()
    );
    server.stop();
}

#[test]
fn download_retries_oack_and_uses_the_negotiated_block_limit() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("read.bin"), [1u8; 20]).unwrap();
    let mut settings = config(dir.path(), false, false);
    settings.block_size = 16;
    settings.timeout_secs = 5;
    let (mut server, addr, _events) = start(settings);
    let client = client();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    client
        .send_to(
            &request_with_options(
                packet::OP_RRQ,
                "read.bin",
                &[("blksize", "64"), ("timeout", "1")],
            ),
            addr,
        )
        .unwrap();
    let mut buffer = [0u8; 512];
    let (size, tid) = client.recv_from(&mut buffer).unwrap();
    let oack = buffer[..size].to_vec();
    let (size, _) = client.recv_from(&mut buffer).unwrap();
    assert_eq!(&buffer[..size], &oack);
    client.send_to(&packet::write_ack(0), tid).unwrap();
    let (size, _) = client.recv_from(&mut buffer).unwrap();
    assert_eq!(
        packet::parse_data(&buffer[..size]).unwrap(),
        (1, &[1u8; 16][..])
    );
    client.send_to(&packet::write_ack(1), tid).unwrap();
    let (size, _) = client.recv_from(&mut buffer).unwrap();
    assert_eq!(
        packet::parse_data(&buffer[..size]).unwrap(),
        (2, &[1u8; 4][..])
    );
    client.send_to(&packet::write_ack(2), tid).unwrap();
    server.stop();
}

#[test]
fn ipv6_wildcard_listener_uses_an_ipv6_session_tid() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("ipv6.bin"), b"ipv6").unwrap();
    // IPv6 may be disabled by the host; every IPv6-enabled CI host runs this.
    let client = match UdpSocket::bind("[::1]:0") {
        Ok(client) => client,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::AddrNotAvailable | std::io::ErrorKind::Unsupported
            ) =>
        {
            return;
        }
        Err(error) => panic!("IPv6 bind failed: {error}"),
    };
    client
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut settings = config(dir.path(), false, false);
    settings.bind_address = "::".into();
    let (mut server, addr, _events) = start(settings);
    client
        .send_to(
            &request(packet::OP_RRQ, "ipv6.bin"),
            SocketAddr::new("::1".parse().unwrap(), addr.port()),
        )
        .unwrap();
    let mut buffer = [0u8; 512];
    let (size, tid) = client.recv_from(&mut buffer).unwrap();
    assert!(tid.is_ipv6());
    assert_eq!(
        packet::parse_data(&buffer[..size]).unwrap(),
        (1, &b"ipv6"[..])
    );
    client.send_to(&packet::write_ack(1), tid).unwrap();
    server.stop();
}

#[test]
fn wrong_tid_cannot_end_upload_at_block_65535_and_blocks_wrap_to_zero() {
    let dir = tempfile::tempdir().unwrap();
    let mut settings = config(dir.path(), true, false);
    settings.block_size = 8;
    let (mut server, addr, events) = start(settings);
    let client = client();
    client
        .send_to(
            &request_with_options(packet::OP_WRQ, "wrap.bin", &[("blksize", "8")]),
            addr,
        )
        .unwrap();
    let mut buffer = [0u8; 512];
    let (size, tid) = client.recv_from(&mut buffer).unwrap();
    assert_eq!(packet::opcode(&buffer[..size]), Some(packet::OP_OACK));
    for block in 1..u16::MAX {
        client
            .send_to(&packet::write_data(block, &[42; 8]), tid)
            .unwrap();
        let (size, _) = client.recv_from(&mut buffer).unwrap();
        assert_eq!(packet::parse_ack(&buffer[..size]).unwrap(), block);
    }
    let intruder = UdpSocket::bind("127.0.0.1:0").unwrap();
    intruder
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    intruder
        .send_to(&packet::write_data(u16::MAX, b""), tid)
        .unwrap();
    let (size, _) = intruder.recv_from(&mut buffer).unwrap();
    assert_eq!(
        packet::parse_error(&buffer[..size]).unwrap().0,
        packet::ERR_UNKNOWN_TID
    );
    assert!(!dir.path().join("wrap.bin").exists());
    client
        .send_to(&packet::write_data(u16::MAX, &[42; 8]), tid)
        .unwrap();
    let (size, _) = client.recv_from(&mut buffer).unwrap();
    assert_eq!(packet::parse_ack(&buffer[..size]).unwrap(), u16::MAX);
    client
        .send_to(&packet::write_data(0, b"done"), tid)
        .unwrap();
    let (size, _) = client.recv_from(&mut buffer).unwrap();
    assert_eq!(packet::parse_ack(&buffer[..size]).unwrap(), 0);
    let data = fs::read(dir.path().join("wrap.bin")).unwrap();
    assert_eq!(data.len(), usize::from(u16::MAX) * 8 + 4);
    assert!(data[..data.len() - 4].iter().all(|byte| *byte == 42));
    assert!(data.ends_with(b"done"));
    assert!(matches!(
        wait_for_finish(&events),
        TransferOutcome::Completed { .. }
    ));
    server.stop();
}

#[test]
fn foreign_acknowledgements_do_not_exhaust_download_retries() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("download.bin"), [9u8; 600]).unwrap();
    let (mut server, addr, events) = start(config(dir.path(), false, false));
    let client = client();
    client
        .send_to(&request(packet::OP_RRQ, "download.bin"), addr)
        .unwrap();
    let mut buffer = [0u8; 1024];
    let (size, tid) = client.recv_from(&mut buffer).unwrap();
    assert_eq!(packet::parse_data(&buffer[..size]).unwrap().0, 1);
    let intruder = UdpSocket::bind("127.0.0.1:0").unwrap();
    intruder
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    for _ in 0..8 {
        intruder.send_to(&packet::write_ack(1), tid).unwrap();
        let (size, _) = intruder.recv_from(&mut buffer).unwrap();
        assert_eq!(
            packet::parse_error(&buffer[..size]).unwrap().0,
            packet::ERR_UNKNOWN_TID
        );
    }
    client.send_to(&packet::write_ack(1), tid).unwrap();
    let (size, _) = client.recv_from(&mut buffer).unwrap();
    assert_eq!(
        packet::parse_data(&buffer[..size]).unwrap(),
        (2, &[9u8; 88][..])
    );
    client.send_to(&packet::write_ack(2), tid).unwrap();
    assert_eq!(
        wait_for_finish(&events),
        TransferOutcome::Completed { bytes: 600 }
    );
    server.stop();
}

#[test]
fn duplicate_and_out_of_order_data_do_not_duplicate_uploaded_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let mut settings = config(dir.path(), true, false);
    settings.block_size = 8;
    let (mut server, addr, _events) = start(settings);
    let client = client();
    client
        .send_to(
            &request_with_options(packet::OP_WRQ, "ordered.bin", &[("blksize", "8")]),
            addr,
        )
        .unwrap();
    let mut buffer = [0u8; 512];
    let (_, tid) = client.recv_from(&mut buffer).unwrap();
    client
        .send_to(&packet::write_data(2, b"early"), tid)
        .unwrap();
    for _ in 0..2 {
        client
            .send_to(&packet::write_data(1, b"12345678"), tid)
            .unwrap();
        let (size, _) = client.recv_from(&mut buffer).unwrap();
        assert_eq!(packet::parse_ack(&buffer[..size]).unwrap(), 1);
    }
    client.send_to(&packet::write_data(2, b""), tid).unwrap();
    let (size, _) = client.recv_from(&mut buffer).unwrap();
    assert_eq!(packet::parse_ack(&buffer[..size]).unwrap(), 2);
    assert_eq!(
        fs::read(dir.path().join("ordered.bin")).unwrap(),
        b"12345678"
    );
    server.stop();
}

#[test]
fn netascii_upload_flushes_a_final_carriage_return_before_acknowledgement() {
    let dir = tempfile::tempdir().unwrap();
    let (mut server, addr, _events) = start(config(dir.path(), true, false));
    let client = client();
    client.send_to(b"\0\x02text.txt\0netascii\0", addr).unwrap();
    let mut buffer = [0u8; 512];
    let (_, tid) = client.recv_from(&mut buffer).unwrap();
    client
        .send_to(&packet::write_data(1, b"a\r\nb\r\0c\r"), tid)
        .unwrap();
    let (size, _) = client.recv_from(&mut buffer).unwrap();
    assert_eq!(packet::parse_ack(&buffer[..size]).unwrap(), 1);
    assert_eq!(fs::read(dir.path().join("text.txt")).unwrap(), b"a\nb\rc\r");
    server.stop();
}

#[test]
fn oversized_requests_are_rejected_without_stopping_the_listener() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("ok.bin"), b"ok").unwrap();
    let (mut server, addr, _events) = start(config(dir.path(), false, false));
    let client = client();
    client.send_to(&[0u8; 4096], addr).unwrap();
    let mut buffer = [0u8; 512];
    let (size, _) = client.recv_from(&mut buffer).unwrap();
    assert_eq!(
        packet::parse_error(&buffer[..size]).unwrap().0,
        packet::ERR_ILLEGAL_OPERATION
    );
    client
        .send_to(&request(packet::OP_RRQ, "ok.bin"), addr)
        .unwrap();
    let (size, tid) = client.recv_from(&mut buffer).unwrap();
    assert_eq!(
        packet::parse_data(&buffer[..size]).unwrap(),
        (1, &b"ok"[..])
    );
    client.send_to(&packet::write_ack(1), tid).unwrap();
    server.stop();
}
