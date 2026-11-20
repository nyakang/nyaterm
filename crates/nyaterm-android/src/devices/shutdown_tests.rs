use crate::adb::client::AdbClient;
use crate::devices::{DeviceService, SharedDeviceService};
use crate::error::AdbError;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

fn read_request(stream: &mut std::net::TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut header = [0; 4];
    stream.read_exact(&mut header).unwrap();
    let length = usize::from_str_radix(std::str::from_utf8(&header).unwrap(), 16).unwrap();
    let mut request = vec![0; length];
    stream.read_exact(&mut request).unwrap();
    request
}

#[test]
fn shutdown_cancels_a_tracker_handshake_and_never_restarts_the_server() {
    let shared = SharedDeviceService::default();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = AdbClient::new("missing-fixture-adb".into(), listener.local_addr().unwrap());
    let (entered_tx, entered_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(read_request(&mut stream), b"host:version");
        stream.write_all(b"OKAY00040029").unwrap();
        assert_eq!(stream.read(&mut [0]).unwrap(), 0);
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(read_request(&mut stream), b"host:track-devices-l");
        stream.write_all(b"OK").unwrap();
        entered_tx.send(()).unwrap();
        assert_eq!(stream.read(&mut [0]).unwrap(), 0);
        listener.set_nonblocking(true).unwrap();
        assert!(listener.accept().is_err());
    });
    shared.ensure(&client).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let started = Instant::now();
    let cleanup = shared.begin_shutdown();
    assert!(started.elapsed() < Duration::from_millis(100));
    assert!(cleanup.finish(Duration::from_secs(1)));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(matches!(shared.ensure(&client), Err(AdbError::Cancelled)));
    assert!(shared.begin_shutdown().finish(Duration::ZERO));
    worker.join().unwrap();
}

#[test]
fn shutdown_does_not_wait_on_the_tracker_lock_held_by_startup() {
    let shared = SharedDeviceService::default();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = AdbClient::new("missing-fixture-adb".into(), listener.local_addr().unwrap());
    let (entered_tx, entered_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(read_request(&mut stream), b"host:version");
        stream.write_all(b"OK").unwrap();
        entered_tx.send(()).unwrap();
        assert_eq!(stream.read(&mut [0]).unwrap(), 0);
    });
    let startup_service = shared.clone();
    let startup = std::thread::spawn(move || startup_service.ensure(&client));
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let started = Instant::now();
    let cleanup = shared.begin_shutdown();
    assert!(started.elapsed() < Duration::from_millis(100));
    assert!(cleanup.finish(Duration::from_secs(1)));
    assert!(matches!(startup.join().unwrap(), Err(AdbError::Cancelled)));
    server.join().unwrap();
}

#[test]
fn a_stalled_worker_cannot_extend_the_shutdown_deadline() {
    let shared = SharedDeviceService::default();
    let (release_tx, release_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        release_rx.recv().unwrap();
        finished_tx.send(()).unwrap();
    });
    let client = AdbClient::new("missing-fixture-adb".into(), "127.0.0.1:1".parse().unwrap());
    *shared.tracker.lock().unwrap() = Some((
        client.clone(),
        Arc::new(DeviceService {
            control: shared.control.child(),
            worker: Mutex::new(Some(worker)),
            state: Arc::default(),
            channel: Arc::default(),
            runtimes: Arc::default(),
        }),
    ));
    let started = Instant::now();
    assert!(!shared.begin_shutdown().finish(Duration::ZERO));
    assert!(started.elapsed() < Duration::from_millis(100));
    assert!(matches!(shared.ensure(&client), Err(AdbError::Cancelled)));
    release_tx.send(()).unwrap();
    finished_rx.recv_timeout(Duration::from_secs(1)).unwrap();
}

#[test]
fn independent_application_owners_do_not_share_shutdown_cancellation() {
    let first = SharedDeviceService::default();
    let second = SharedDeviceService::default();
    let cleanup = first.begin_shutdown();
    assert!(cleanup.finish(Duration::ZERO));
    assert!(matches!(first.control.check(), Err(AdbError::Cancelled)));
    assert!(second.control.check().is_ok());
    // A separate GPUI application (including a later visual test) remains usable.
    assert!(matches!(
        second.ensure(&AdbClient::new(
            "missing-fixture-adb".into(),
            "127.0.0.1:1".parse().unwrap()
        )),
        Err(AdbError::Io(_))
    ));
}
