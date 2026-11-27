//! Real peer coverage for the application pump, including ESCCTL and ACK boundaries.
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use super::{ZmodemAction, ZmodemDirection, ZmodemEvent, ZmodemTransfer, ZmodemTransferOptions};

struct Peer(Child);

impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn upload_to_rz_crosses_streaming_window_with_control_bytes() {
    transfer_with_lrzsz(ZmodemDirection::Upload);
}

#[test]
fn download_from_sz_preserves_binary_batches() {
    transfer_with_lrzsz(ZmodemDirection::Download);
}

fn transfer_with_lrzsz(direction: ZmodemDirection) {
    let program = match direction {
        ZmodemDirection::Upload => "rz",
        ZmodemDirection::Download => "sz",
    };
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let destination = root.path().join("destination");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&destination).unwrap();
    let mut seed = 0x1234_5678u32;
    let binary: Vec<u8> = (0..(2 * 1024 * 1024 + 137))
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        })
        .collect();
    let payloads = [Vec::new(), binary, b"last file\r\n".to_vec()];
    let paths: Vec<_> = payloads
        .iter()
        .enumerate()
        .map(|(index, payload)| {
            let path = source.join(format!("file-{index}"));
            std::fs::write(&path, payload).unwrap();
            path
        })
        .collect();
    // A duplex socket gives both directions bounded blocking I/O, so a protocol
    // regression fails the test instead of hanging a pipe writer indefinitely.
    let (mut wire, peer_wire) = UnixStream::pair().unwrap();
    wire.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    wire.set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut command = Command::new(program);
    command
        .args(["-q", "-b", "-e"])
        .stdin(Stdio::from(OwnedFd::from(peer_wire.try_clone().unwrap())))
        .stdout(Stdio::from(OwnedFd::from(peer_wire)))
        .stderr(Stdio::null());
    match direction {
        ZmodemDirection::Upload => {
            command.arg("-y").current_dir(&destination);
        }
        ZmodemDirection::Download => {
            command.args(&paths);
        }
    }
    let mut peer = match command.spawn() {
        Ok(child) => Peer(child),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipping real ZMODEM peer test: {program} is not installed");
            return;
        }
        Err(error) => panic!("cannot start {program}: {error}"),
    };
    // Command retains its configured descriptors after spawn.
    drop(command);
    let mut transfer =
        ZmodemTransfer::with_options(direction, &[], ZmodemTransferOptions::reliable_transport());
    let mut actions = match direction {
        ZmodemDirection::Upload => transfer.accept_upload(paths),
        ZmodemDirection::Download => transfer.accept_download(destination.clone()),
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut input = [0; 64 * 1024];
    let mut length = 0;
    let mut offset = 0;
    let mut more = false;
    let mut complete = false;
    loop {
        assert!(Instant::now() < deadline, "{program} transfer timed out");
        for action in actions {
            match action {
                ZmodemAction::SendToRemote(bytes) => wire.write_all(&bytes).unwrap(),
                ZmodemAction::EmitEvent(ZmodemEvent::Failed { reason }) => {
                    panic!("{program} transfer failed: {reason}");
                }
                ZmodemAction::EmitEvent(ZmodemEvent::Complete { file_count, .. }) => {
                    assert_eq!(file_count, 3);
                    complete = true;
                }
                _ => {}
            }
        }
        if complete {
            break;
        }
        if offset == length && !more {
            length = wire.read(&mut input).unwrap();
            assert_ne!(length, 0, "{program} closed before completion");
            offset = 0;
        }
        let advance = transfer.advance(&input[offset..length]);
        assert!(advance.consumed > 0 || advance.more || offset == length);
        offset += advance.consumed;
        more = advance.more;
        actions = advance.actions;
    }
    loop {
        if let Some(status) = peer.0.try_wait().unwrap() {
            assert!(status.success(), "{program} failed: {status}");
            break;
        }
        assert!(Instant::now() < deadline, "{program} did not exit");
        std::thread::sleep(Duration::from_millis(5));
    }
    for (index, payload) in payloads.iter().enumerate() {
        assert_eq!(
            std::fs::read(destination.join(format!("file-{index}"))).unwrap(),
            *payload
        );
    }
}
