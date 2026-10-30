use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use nyaterm_core::{CloudSyncError, CloudSyncRemote, WebdavSyncSettings};

use super::NativeWebdavRemote;

fn directory_server(
    requests: Vec<(&'static str, u16)>,
) -> (NativeWebdavRemote, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let remote = NativeWebdavRemote::new(&WebdavSyncSettings {
        endpoint: format!("http://{}/dav", listener.local_addr().unwrap()),
        root: "apps/nyaterm".into(),
        ..Default::default()
    })
    .unwrap();
    let handle = std::thread::spawn(move || {
        for (path, status) in requests {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "missing MKCOL {path}");
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("fixture accept failed: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                assert!(request.len() < 16_384);
            }
            let request = String::from_utf8(request).unwrap();
            assert_eq!(
                request.lines().next(),
                Some(format!("MKCOL {path} HTTP/1.1").as_str())
            );
            write!(
                stream,
                "HTTP/1.1 {status} Fixture\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
        }
    });
    (remote, handle)
}

#[test]
fn creates_the_configured_root_before_protocol_subdirectories() {
    let (remote, server) = directory_server(vec![
        ("/dav/apps", 201),
        ("/dav/apps/nyaterm", 201),
        ("/dav/apps/nyaterm/sync", 201),
        ("/dav/apps/nyaterm/sync/v4", 201),
    ]);
    let result = remote.create_dir("sync/v4");
    server.join().unwrap();
    result.unwrap();
}

#[test]
fn existing_parent_directories_do_not_block_creation() {
    let (remote, server) = directory_server(vec![
        ("/dav/apps", 405),
        ("/dav/apps/nyaterm", 405),
        ("/dav/apps/nyaterm/sync", 201),
    ]);
    let result = remote.create_dir("sync");
    server.join().unwrap();
    result.unwrap();
}

#[test]
fn a_parent_conflict_stops_creation_instead_of_reporting_success() {
    let (remote, server) = directory_server(vec![("/dav/apps", 409)]);
    let result = remote.create_dir("sync/v4");
    server.join().unwrap();
    assert!(matches!(
        result,
        Err(CloudSyncError::Http { status: 409, .. })
    ));
}
