use crate::adb::client::AdbClient;
use crate::adb::server::{ServerRegistry, ServerShutdownReport};
use crate::error::AdbError;
use crate::operation::AdbOperationControl;
use std::io::{Error, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

fn custom_adb(root: &std::path::Path) -> std::path::PathBuf {
    let path = root.join(if cfg!(windows) { "adb.cmd" } else { "adb" });
    #[cfg(windows)]
    std::fs::write(&path, "@echo off\r\nif \"%~3\"==\"server\" exit /b 1\r\n> \"%~dp0started\" echo %*\r\nif not exist \"%~dp0wait-ready\" exit /b 0\r\n:wait\r\nif not exist \"%~dp0ready\" goto wait\r\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(
            &path,
            "#!/bin/sh\ncase \"$*\" in *'server nodaemon') exit 1;; esac\nprintf '%s\\n' \"$@\" > \"$(dirname \"$0\")/started\"\nif [ -f \"$(dirname \"$0\")/wait-ready\" ]; then\nwhile [ ! -f \"$(dirname \"$0\")/ready\" ]; do sleep 0.01; done\nfi\n",
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
        drop(listener);
        std::fs::write(root.path().join("wait-ready"), b"").unwrap();
        let server = client.server;
        let server_root = root.path().to_path_buf();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !server_root.join("started").exists() {
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(10));
            }
            let listener = TcpListener::bind(server).unwrap();
            std::fs::write(server_root.join("ready"), b"").unwrap();
            let (mut socket, _) = listener.accept().unwrap();
            version_request(&mut socket);
            socket.write_all(b"OKAY00040029").unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        let registry = ServerRegistry::default();
        registry
            .ensure_with_connection(
                &client,
                &AdbOperationControl::default(),
                Err(Error::from(kind)),
            )
            .unwrap();
        let args = std::fs::read_to_string(root.path().join("started")).unwrap();
        assert_eq!(
            args.split_whitespace().collect::<Vec<_>>(),
            ["-P", &client.server.port().to_string(), "start-server"]
        );
        assert_eq!(
            registry.begin_shutdown().finish(Duration::ZERO),
            ServerShutdownReport::default()
        );
        assert!(
            TcpStream::connect(server).is_ok(),
            "the wrapper's unowned server must survive cleanup"
        );
        release_tx.send(()).unwrap();
        worker.join().unwrap();
    }
}

#[test]
fn refused_server_uses_custom_adb_and_verifies_that_startup_really_succeeded() {
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
                assert!(matches!(client.ensure_server(), Err(AdbError::Timeout)));
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
