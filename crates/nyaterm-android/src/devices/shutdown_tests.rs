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
    // Discovery is admitted only for lease holders.
    let _lease = shared.lease();
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
    let _lease = shared.lease();
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
    assert!(!shared.begin_shutdown().finish(Duration::ZERO));
    assert!(started.elapsed() < Duration::from_millis(100));
    assert!(matches!(shared.ensure(&client), Err(AdbError::Cancelled)));
    release_tx.send(()).unwrap();
    finished_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(shared.begin_shutdown().finish(Duration::from_secs(2)));
}

#[test]
fn ensure_without_a_lease_admits_no_tracker() {
    let shared = SharedDeviceService::default();
    let client = AdbClient::new("missing-fixture-adb".into(), "127.0.0.1:1".parse().unwrap());
    shared.ensure(&client).unwrap();
    assert!(!shared.is_active());
}

#[test]
fn dropping_the_last_lease_stops_discovery_without_cancelling_the_owner() {
    let shared = SharedDeviceService::default();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = AdbClient::new("missing-fixture-adb".into(), listener.local_addr().unwrap());
    let (entered_tx, entered_rx) = mpsc::channel();
    let (closed_tx, closed_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(read_request(&mut stream), b"host:version");
        stream.write_all(b"OKAY00040029").unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(read_request(&mut stream), b"host:track-devices-l");
        stream.write_all(b"OK").unwrap();
        entered_tx.send(()).unwrap();
        // The last lease releases the track stream instead of restarting it.
        assert_eq!(stream.read(&mut [0]).unwrap(), 0);
        closed_tx.send(()).unwrap();
    });
    let lease = shared.lease();
    shared.ensure(&client).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(shared.is_active());
    drop(lease);
    assert!(!shared.is_active());
    closed_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(shared.begin_shutdown().finish(Duration::from_secs(1)));
    worker.join().unwrap();
}

#[test]
fn a_later_lease_admits_a_fresh_tracker() {
    let shared = SharedDeviceService::default();
    let short = shared.lease();
    drop(short);
    assert!(!shared.is_active());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = AdbClient::new("missing-fixture-adb".into(), listener.local_addr().unwrap());
    let (entered_tx, entered_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(read_request(&mut stream), b"host:version");
        stream.write_all(b"OKAY00040029").unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(read_request(&mut stream), b"host:track-devices-l");
        stream.write_all(b"OK").unwrap();
        entered_tx.send(()).unwrap();
        assert_eq!(stream.read(&mut [0]).unwrap(), 0);
    });
    let lease = shared.lease();
    shared.ensure(&client).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(shared.is_active());
    drop(lease);
    assert!(shared.begin_shutdown().finish(Duration::from_secs(1)));
    worker.join().unwrap();
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
    let _lease = second.lease();
    assert!(matches!(
        second.ensure(&AdbClient::new(
            "missing-fixture-adb".into(),
            "127.0.0.1:1".parse().unwrap()
        )),
        Err(AdbError::Io(_))
    ));
}

#[test]
fn last_release_cancels_blocked_startup_without_waiting_and_rejects_its_late_result() {
    let shared = SharedDeviceService::default();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let proof = Arc::new(());
    let released = Arc::downgrade(&proof);
    let client = AdbClient::new("missing-fixture-adb".into(), listener.local_addr().unwrap())
        .with_component(nyaterm_core::plugins::native::NativeLaunch {
            executable: "fixture".into(),
            root: "fixture".into(),
            lease: proof,
        });
    let (entered_tx, entered_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(read_request(&mut stream), b"host:version");
        stream.write_all(b"OK").unwrap();
        entered_tx.send(()).unwrap();
        assert_eq!(stream.read(&mut [0]).unwrap(), 0);
    });
    let lease = shared.lease();
    let owner = shared.clone();
    let startup = std::thread::spawn(move || owner.ensure(&client));
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let (released_tx, released_rx) = mpsc::channel();
    let ui = std::thread::spawn(move || {
        drop(lease);
        released_tx.send(()).unwrap();
    });
    released_rx
        .recv_timeout(Duration::from_millis(200))
        .expect("UI release waited for handshake");
    assert!(!shared.is_active());
    assert!(matches!(startup.join().unwrap(), Err(AdbError::Cancelled)));
    assert!(shared.begin_shutdown().finish(Duration::from_secs(2)));
    assert!(!shared.is_active());
    ui.join().unwrap();
    server.join().unwrap();
    assert!(
        released.upgrade().is_none(),
        "cancelled startup retained the native lease"
    );
}

#[test]
fn queued_ticket_cannot_be_revived_by_a_new_consumer() {
    let shared = SharedDeviceService::default();
    let old = shared.lease();
    let ticket = shared.discovery_control();
    drop(old);
    let _new = shared.lease();
    let client = AdbClient::new("missing-fixture-adb".into(), "127.0.0.1:1".parse().unwrap());
    assert!(matches!(
        shared.ensure_with_control(&client, &ticket),
        Err(AdbError::Cancelled)
    ));
    assert!(!shared.is_active());
    assert!(!shared.discovery_control().is_cancelled());
}

#[test]
fn reacquisition_waits_for_retired_workers_before_probing_the_server() {
    let shared = SharedDeviceService::default();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let client = AdbClient::new("missing-fixture-adb".into(), listener.local_addr().unwrap());
    let (retiring_tx, retiring_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        retiring_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    let old = shared.lease();
    *shared.tracker.lock().unwrap() = Some((
        client.clone(),
        Arc::new(DeviceService {
            control: shared.discovery_control(),
            worker: Mutex::new(Some(worker)),
            state: Arc::default(),
            channel: Arc::default(),
            runtimes: Arc::default(),
        }),
    ));
    retiring_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    drop(old);
    let new = shared.lease();
    let ticket = shared.discovery_control();
    let (attempt_tx, attempt_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let owner = shared.clone();
    let startup = std::thread::spawn(move || {
        attempt_tx.send(()).unwrap();
        finished_tx
            .send(owner.ensure_with_control(&client, &ticket))
            .unwrap();
    });
    attempt_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        listener.accept().is_err(),
        "new generation probed before cleanup"
    );
    // Cancel while still stopping. No socket or startup should survive the barrier.
    drop(new);
    let cancelled = finished_rx.recv_timeout(Duration::from_secs(2));
    release_tx.send(()).unwrap();
    startup.join().unwrap();
    assert!(matches!(cancelled.unwrap(), Err(AdbError::Cancelled)));
    assert!(listener.accept().is_err());
    assert!(shared.begin_shutdown().finish(Duration::from_secs(2)));
}

#[test]
fn only_the_last_of_multiple_consumers_cancels_the_generation() {
    let shared = SharedDeviceService::default();
    let first = shared.lease();
    let second = shared.lease();
    let ticket = shared.discovery_control();
    drop(first);
    assert!(!ticket.is_cancelled());
    drop(second);
    assert!(ticket.is_cancelled());
}

#[test]
fn tracker_identity_rejects_old_bindings_even_when_all_other_generations_match() {
    use crate::adb::client::{AdbDevice, DeviceBinding, DeviceStatus};
    use crate::adb::runtime::DeviceRuntime;
    use crate::devices::DeviceSnapshot;
    let shared = SharedDeviceService::default();
    let _lease = shared.lease();
    let client = AdbClient::new("fixture".into(), "127.0.0.1:1".parse().unwrap());
    let snapshot = DeviceSnapshot {
        revision: 1,
        tracker_generation: 2,
        devices: vec![AdbDevice {
            serial: "fixture".into(),
            transport_id: Some(1),
            model: None,
            status: DeviceStatus::Online,
        }],
        device_generations: [("fixture".into(), 1)].into(),
        ..Default::default()
    };
    *shared.tracker.lock().unwrap() = Some((
        client.clone(),
        Arc::new(DeviceService {
            control: shared.discovery_control(),
            worker: Mutex::new(None),
            state: Arc::new(Mutex::new(snapshot.clone())),
            channel: Arc::default(),
            runtimes: Arc::default(),
        }),
    ));
    assert!(shared.snapshot_if_changed((1, 1)).is_some());
    assert!(shared.snapshot_if_changed((2, 1)).is_none());
    let runtime = Arc::new(DeviceRuntime::new(DeviceBinding {
        server: client.server,
        serial: "fixture".into(),
        transport_id: Some(1),
        generation: 1,
    }));
    let old = DeviceSnapshot {
        tracker_generation: 1,
        ..snapshot
    };
    assert!(matches!(
        shared.register_runtime_since(&runtime, &old),
        Err(AdbError::Offline)
    ));
    assert!(!runtime.is_valid());
    assert!(shared.begin_shutdown().finish(Duration::from_secs(2)));
}

#[test]
fn reopening_while_the_owned_server_is_alive_cannot_attach_to_the_retiring_process() {
    let shared = SharedDeviceService::default();
    let address = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let client = AdbClient::new("missing-fixture-adb".into(), address);
    crate::adb::server::tests::start_owned_fixture(&shared.server_manager(), &client);
    let (release_tx, release_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        release_rx.recv().unwrap();
    });
    let old = shared.lease();
    *shared.tracker.lock().unwrap() = Some((
        client.clone(),
        Arc::new(DeviceService {
            control: shared.discovery_control(),
            worker: Mutex::new(Some(worker)),
            state: Arc::default(),
            channel: Arc::default(),
            runtimes: Arc::default(),
        }),
    ));
    drop(old);
    let _new = shared.lease();
    // Unlike the old regression test, the original owned daemon is still alive.
    assert_eq!(client.version().unwrap(), 41);
    let owner = shared.clone();
    let new_client = client.clone();
    let (entered_tx, entered_rx) = mpsc::channel();
    let startup = std::thread::spawn(move || {
        entered_tx.send(()).unwrap();
        owner.ensure(&new_client)
    });
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    release_tx.send(()).unwrap();
    // A missing executable only fails if startup waited for the old server to
    // exit. Reusing the old daemon would incorrectly return success here.
    assert!(matches!(startup.join().unwrap(), Err(AdbError::Io(_))));
    assert!(!shared.is_active());
    assert!(std::net::TcpStream::connect(address).is_err());
    assert!(shared.begin_shutdown().finish(Duration::from_secs(2)));
}

#[test]
fn a_published_tracker_does_not_inherit_one_requesters_cancellation() {
    let shared = SharedDeviceService::default();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = AdbClient::new("missing-fixture-adb".into(), listener.local_addr().unwrap());
    let (entered_tx, entered_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(read_request(&mut stream), b"host:version");
        stream.write_all(b"OKAY00040029").unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(read_request(&mut stream), b"host:track-devices-l");
        stream.write_all(b"OK").unwrap();
        entered_tx.send(()).unwrap();
        assert_eq!(stream.read(&mut [0]).unwrap(), 0);
    });
    let lease = shared.lease();
    let request = shared.discovery_control();
    shared.ensure_with_control(&client, &request).unwrap();
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    request.cancel();
    assert!(
        !shared
            .tracker
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .1
            .control
            .is_cancelled()
    );
    drop(lease);
    assert!(shared.begin_shutdown().finish(Duration::from_secs(2)));
    server.join().unwrap();
}
