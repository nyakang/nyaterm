//! Loopback tests for the TFTP listener.

use std::fs;
use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc::{Receiver, sync_channel};
use std::time::Duration;

use nyaterm_core::TftpSettings;

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
    let server = TftpServer::start(settings, events, transfers).expect("bind loopback");
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
