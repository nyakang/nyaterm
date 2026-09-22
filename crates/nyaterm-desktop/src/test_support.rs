use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use nyaterm_store::{FlushBarrier, StoreBlockingClient, StoreConfig, StoreRuntime};

pub(crate) use nyaterm_core::test_support::TestTempDir as TestConfigDir;

/// Opens an isolated test store and waits until its redb worker is ready.
///
/// Waiting on the barrier is part of the cleanup contract: without it, a very
/// short test can remove its untouched directory before the worker starts, and
/// the late worker then recreates `config/nyaterm.redb` after cleanup.
pub(crate) fn blocking_test_store(root: &Path) -> StoreBlockingClient {
    let runtime = StoreRuntime::spawn(StoreConfig {
        config_dir: root.join("config"),
        portable_key_path: None,
    })
    .expect("spawn test store");
    let store = runtime.blocking_client();
    store
        .request(0, FlushBarrier)
        .expect("receive test store barrier")
        .outcome
        .expect("initialize test store");
    store
}

pub(crate) fn spawn_webdav_service_unavailable_server() -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock WebDAV listener");
    let endpoint = format!("http://{}", listener.local_addr().expect("mock address"));
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "WebDAV request was not sent");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("mock accept failed: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        let read_deadline = Instant::now() + Duration::from_secs(15);
        while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let count = match stream.read(&mut buffer) {
                Ok(count) => count,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    assert!(
                        Instant::now() < read_deadline,
                        "WebDAV request headers timed out"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(error) => panic!("mock request headers: {error}"),
            };
            assert!(count > 0, "incomplete request");
            request.extend_from_slice(&buffer[..count]);
            assert!(request.len() < 16 * 1024, "oversized mock request");
        }
        assert!(request.starts_with(b"MKCOL "));
        stream
            .write_all(
                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .expect("mock response");
    });
    (endpoint, server)
}

/// A one-shot WebDAV server that answers any request as "no such resource", so a
/// provider connection test reading `sync/latest.redb` succeeds (`None` pointer).
pub(crate) fn spawn_webdav_healthy_server() -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock WebDAV listener");
    let endpoint = format!("http://{}", listener.local_addr().expect("mock address"));
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "WebDAV request was not sent");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("mock accept failed: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).expect("request headers");
            assert!(count > 0, "incomplete request");
            request.extend_from_slice(&buffer[..count]);
            assert!(request.len() < 16 * 1024, "oversized mock request");
        }
        assert!(request.starts_with(b"GET /"));
        stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .expect("mock response");
    });
    (endpoint, server)
}

#[test]
fn test_config_dir_removes_its_tree_on_drop() {
    let path = {
        let dir = TestConfigDir::new("nyaterm-test-cleanup");
        std::fs::create_dir_all(dir.path().join("config")).expect("create test tree");
        dir.path().to_path_buf()
    };

    assert!(!path.exists(), "temporary test tree was not removed");
}

#[test]
fn synchronized_test_store_cannot_recreate_its_tree_after_cleanup() {
    let path = {
        let dir = TestConfigDir::new("nyaterm-test-store-cleanup");
        let path = dir.path().to_path_buf();
        let store = blocking_test_store(dir.path());
        assert!(path.join("config/nyaterm.redb").is_file());
        drop(store);
        path
    };

    // This delay would expose the old race: an unsynchronized worker could
    // start after the guard returned and recreate the database behind it.
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(
        !path.exists(),
        "the storage worker recreated its temporary tree after cleanup"
    );
}
