use crate::adb::client::AdbClient;
use crate::error::AdbError;
use std::io::{Error, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

fn custom_adb(root: &std::path::Path) -> std::path::PathBuf {
    let path = root.join(if cfg!(windows) { "adb.cmd" } else { "adb" });
    #[cfg(windows)]
    std::fs::write(&path, "@echo off\r\n> \"%~dp0started\" echo %*\r\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(
            &path,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$(dirname \"$0\")/started\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    path
}

fn version_request(socket: &mut TcpStream) {
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut request = [0; 16];
    socket.read_exact(&mut request).unwrap();
    assert_eq!(&request, b"000chost:version");
}

#[test]
fn tcp_connect_failures_start_the_configured_adb_and_verify_its_server() {
    let _guard = crate::adb::client::STARTED_SERVER_TEST_LOCK.lock().unwrap();
    // Inject OS connect results: reliably causing a loopback connect timeout is
    // not portable. The command and subsequent protocol probe remain real I/O.
    for kind in [
        ErrorKind::ConnectionRefused,
        ErrorKind::TimedOut,
        ErrorKind::WouldBlock,
    ] {
        let root = tempfile::tempdir().unwrap();
        let executable = custom_adb(root.path());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = AdbClient::local(Some(&executable)).unwrap();
        client.server = listener.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            version_request(&mut socket);
            socket.write_all(b"OKAY00040029").unwrap();
        });
        client
            .ensure_server_with_connection(Err(Error::from(kind)))
            .unwrap();
        let args = std::fs::read_to_string(root.path().join("started")).unwrap();
        assert_eq!(
            args.split_whitespace().collect::<Vec<_>>(),
            ["-P", &client.server.port().to_string(), "start-server"]
        );
        worker.join().unwrap();
    }
    super::take_started_servers();
}

#[test]
fn refused_server_uses_custom_adb_and_verifies_that_startup_really_succeeded() {
    let _guard = crate::adb::client::STARTED_SERVER_TEST_LOCK.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    let executable = custom_adb(root.path());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = AdbClient::local(Some(&executable)).unwrap();
    client.server = listener.local_addr().unwrap();
    drop(listener);
    // The fixture exits successfully without starting a server. Do not report
    // success until a subsequent host:version probe has actually succeeded.
    assert!(matches!(client.ensure_server(), Err(AdbError::Io(_))));
    assert!(root.path().join("started").is_file());
    super::take_started_servers();
}

#[test]
fn connected_server_timeouts_never_start_adb() {
    // Exercise both status and payload reads, including partial responses.
    std::thread::scope(|scope| {
        for response in [b"".as_slice(), b"OK", b"OKAY000400"] {
            scope.spawn(move || {
                let root = tempfile::tempdir().unwrap();
                let executable = custom_adb(root.path());
                let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                let mut client = AdbClient::local(Some(&executable)).unwrap();
                client.server = listener.local_addr().unwrap();
                let worker = std::thread::spawn(move || {
                    let (mut socket, _) = listener.accept().unwrap();
                    version_request(&mut socket);
                    socket.write_all(response).unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(30)))
                        .unwrap();
                    assert_eq!(socket.read(&mut [0]).unwrap(), 0);
                });
                assert!(matches!(
                    client.ensure_server(),
                    Err(AdbError::Io(error))
                        if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock)
                ));
                assert!(!root.path().join("started").exists());
                worker.join().unwrap();
            });
        }
    });
}

#[test]
fn other_connect_errors_never_start_adb() {
    let root = tempfile::tempdir().unwrap();
    let client = AdbClient::local(Some(&custom_adb(root.path()))).unwrap();
    for kind in [ErrorKind::PermissionDenied, ErrorKind::ConnectionReset] {
        assert!(matches!(
            client.ensure_server_with_connection(Err(Error::from(kind))),
            Err(AdbError::Io(error)) if error.kind() == kind
        ));
    }
    assert!(!root.path().join("started").exists());
}
