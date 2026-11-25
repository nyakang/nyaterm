use crate::adb::client::AdbClient;
use crate::adb::server::{OwnedServer, ServerRegistry, ServerShutdownReport, connect};
use crate::error::{AdbError, AdbResult};
use crate::operation::AdbOperationControl;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc, Barrier,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

fn client() -> AdbClient {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    AdbClient::new("missing-fixture-adb".into(), listener.local_addr().unwrap())
}

fn spawn_fixture(client: &AdbClient, mode: &str) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "adb::server::tests::foreground_server_fixture",
            "--ignored",
        ])
        .env("NYATERM_TEST_ADB_ADDRESS", client.server.to_string())
        .env("NYATERM_TEST_ADB_MODE", mode)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn ensure(
    registry: &ServerRegistry,
    client: &AdbClient,
    control: &AdbOperationControl,
    spawn: impl FnOnce(&AdbClient) -> std::io::Result<Child>,
) -> AdbResult<()> {
    let control = registry
        .control
        .linked_to(control)
        .with_timeout(Duration::from_secs(3));
    registry.ensure_connected(
        client,
        &control,
        connect(client.server, &control),
        |client| spawn(client).map(Some),
    )
}

fn wait_ready(client: &AdbClient) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while client.version().is_err() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_closed(server: SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while TcpStream::connect(server).is_ok() {
        assert!(Instant::now() < deadline, "owned listener survived cleanup");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn stop_fixture(server: SocketAddr) {
    let mut stream = TcpStream::connect(server).unwrap();
    stream.write_all(b"0011host:fixture-stop").unwrap();
    let mut status = [0; 4];
    stream.read_exact(&mut status).unwrap();
    assert_eq!(&status, b"OKAY");
}

#[test]
#[ignore = "subprocess fixture, invoked only with an isolated test address"]
fn foreground_server_fixture() {
    let Some(address) = std::env::var_os("NYATERM_TEST_ADB_ADDRESS") else {
        return;
    };
    let mode = std::env::var("NYATERM_TEST_ADB_MODE").unwrap();
    if mode == "exit" {
        return;
    }
    let listener = TcpListener::bind(address.to_str().unwrap()).unwrap();
    for stream in listener.incoming() {
        let mut stream = stream.unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut header = [0; 4];
        if stream.read_exact(&mut header).is_err() {
            continue;
        }
        let length = usize::from_str_radix(std::str::from_utf8(&header).unwrap(), 16).unwrap();
        let mut service = vec![0; length];
        stream.read_exact(&mut service).unwrap();
        let stopping = service == b"host:fixture-stop";
        if stopping {
            stream.write_all(b"OKAY").unwrap();
        } else if mode == "silent" {
            stream.write_all(b"OK").unwrap();
        } else if mode == "incompatible" {
            stream.write_all(b"OKAY0004002a").unwrap();
        } else {
            assert_eq!(service, b"host:version");
            stream.write_all(b"OKAY00040029").unwrap();
        }
        // Client closes first, avoiding TIME_WAIT on the fixture's server port.
        let _ = stream.read_to_end(&mut Vec::new());
        if stopping {
            break;
        }
    }
}

#[test]
fn foreground_startup_retains_and_reaps_only_its_owned_process() {
    let registry = ServerRegistry::default();
    let client = client();
    ensure(
        &registry,
        &client,
        &AdbOperationControl::default(),
        |client| Ok(spawn_fixture(client, "server")),
    )
    .unwrap();
    assert_eq!(client.version().unwrap(), 41);
    assert_eq!(
        registry.begin_shutdown().finish(Duration::from_secs(1)),
        ServerShutdownReport {
            terminated: 1,
            ..ServerShutdownReport::default()
        }
    );
    wait_closed(client.server);
    assert_eq!(
        registry.begin_shutdown().finish(Duration::ZERO),
        ServerShutdownReport::default()
    );
    assert!(matches!(
        registry.ensure(&client, &AdbOperationControl::default()),
        Err(AdbError::Cancelled)
    ));
}

#[test]
fn a_foreign_replacement_at_the_same_address_survives_shutdown() {
    let registry = ServerRegistry::default();
    let client = client();
    ensure(
        &registry,
        &client,
        &AdbOperationControl::default(),
        |client| Ok(spawn_fixture(client, "server")),
    )
    .unwrap();
    stop_fixture(client.server);
    wait_closed(client.server);
    let mut foreign = OwnedServer(Some(spawn_fixture(&client, "server")), None);
    wait_ready(&client);
    let report = registry.begin_shutdown().finish(Duration::from_secs(1));
    assert_eq!(report.terminated + report.already_exited, 1);
    assert_eq!(report.failed, 0);
    assert!(foreign.0.as_mut().unwrap().try_wait().unwrap().is_none());
    assert_eq!(client.version().unwrap(), 41);
}

#[test]
fn compatible_foreign_servers_are_reused_without_spawning() {
    let registry = ServerRegistry::default();
    let client = client();
    let mut foreign = OwnedServer(Some(spawn_fixture(&client, "server")), None);
    wait_ready(&client);
    ensure(&registry, &client, &AdbOperationControl::default(), |_| {
        panic!("foreign server was replaced")
    })
    .unwrap();
    assert_eq!(
        registry.begin_shutdown().finish(Duration::ZERO),
        ServerShutdownReport::default()
    );
    assert!(foreign.0.as_mut().unwrap().try_wait().unwrap().is_none());
}

#[test]
fn a_foreign_server_arriving_after_a_failed_connect_is_not_claimed() {
    let registry = ServerRegistry::default();
    let client = client();
    let mut foreign = OwnedServer(Some(spawn_fixture(&client, "server")), None);
    wait_ready(&client);
    let control = registry.control.with_timeout(Duration::from_secs(3));
    registry
        .ensure_connected(
            &client,
            &control,
            Err(std::io::Error::from(std::io::ErrorKind::ConnectionRefused).into()),
            |_| panic!("a competing server must not invoke the launcher"),
        )
        .unwrap();
    assert_eq!(
        registry.begin_shutdown().finish(Duration::ZERO),
        ServerShutdownReport::default()
    );
    assert!(foreign.0.as_mut().unwrap().try_wait().unwrap().is_none());
}

#[test]
fn concurrent_startup_at_one_address_spawns_once() {
    let registry = Arc::new(ServerRegistry::default());
    let client = client();
    let barrier = Arc::new(Barrier::new(4));
    let spawns = Arc::new(AtomicUsize::new(0));
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let registry = Arc::clone(&registry);
            let barrier = Arc::clone(&barrier);
            let spawns = Arc::clone(&spawns);
            let client = client.clone();
            scope.spawn(move || {
                barrier.wait();
                ensure(
                    &registry,
                    &client,
                    &AdbOperationControl::default(),
                    |client| {
                        spawns.fetch_add(1, Ordering::AcqRel);
                        Ok(spawn_fixture(client, "server"))
                    },
                )
                .unwrap();
            });
        }
    });
    assert_eq!(spawns.load(Ordering::Acquire), 1);
    assert_eq!(
        registry
            .begin_shutdown()
            .finish(Duration::from_secs(1))
            .terminated,
        1
    );
}

#[test]
fn cancellation_during_an_incomplete_startup_handshake_reaps_the_child() {
    let registry = ServerRegistry::default();
    let client = client();
    let control = AdbOperationControl::default().with_timeout(Duration::from_millis(300));
    assert!(matches!(
        ensure(&registry, &client, &control, |client| Ok(spawn_fixture(
            client, "silent"
        ))),
        Err(AdbError::Timeout)
    ));
    wait_closed(client.server);
    assert_eq!(
        registry.begin_shutdown().finish(Duration::ZERO),
        ServerShutdownReport::default()
    );
}

#[test]
fn an_incompatible_owned_startup_is_rolled_back_without_fallback() {
    let registry = ServerRegistry::default();
    let client = client();
    assert!(matches!(
        ensure(
            &registry,
            &client,
            &AdbOperationControl::default(),
            |client| Ok(spawn_fixture(client, "incompatible"))
        ),
        Err(AdbError::IncompatibleServer)
    ));
    wait_closed(client.server);
}

#[test]
fn shutdown_does_not_wait_for_a_startup_launcher_and_cancels_late_registration() {
    let registry = Arc::new(ServerRegistry::default());
    let client = client();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let startup_registry = Arc::clone(&registry);
    let startup_client = client.clone();
    let worker = std::thread::spawn(move || {
        ensure(
            &startup_registry,
            &startup_client,
            &AdbOperationControl::default(),
            |client| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(spawn_fixture(client, "server"))
            },
        )
    });
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let started = Instant::now();
    let cleanup = registry.begin_shutdown();
    assert!(started.elapsed() < Duration::from_millis(100));
    assert_eq!(
        cleanup.finish(Duration::ZERO),
        ServerShutdownReport::default()
    );
    release_tx.send(()).unwrap();
    assert!(matches!(worker.join().unwrap(), Err(AdbError::Cancelled)));
    wait_closed(client.server);
}

#[test]
fn a_zero_cleanup_budget_signals_the_child_without_waiting_for_exit() {
    let registry = ServerRegistry::default();
    let client = client();
    ensure(
        &registry,
        &client,
        &AdbOperationControl::default(),
        |client| Ok(spawn_fixture(client, "server")),
    )
    .unwrap();
    let started = Instant::now();
    let report = registry.begin_shutdown().finish(Duration::ZERO);
    assert_eq!(report.terminated + report.failed, 1);
    assert!(started.elapsed() < Duration::from_millis(100));
    wait_closed(client.server);
}

#[test]
fn an_early_foreground_exit_does_not_create_an_owned_daemon_record() {
    let registry = ServerRegistry::default();
    let client = client();
    assert!(matches!(
        ensure(
            &registry,
            &client,
            &AdbOperationControl::default(),
            |client| Ok(spawn_fixture(client, "exit"))
        ),
        Err(AdbError::Io(_))
    ));
    assert_eq!(
        registry.begin_shutdown().finish(Duration::ZERO),
        ServerShutdownReport::default()
    );
}

#[test]
fn release_stops_owned_servers_and_keeps_the_manager_usable() {
    let registry = ServerRegistry::default();
    let client = client();
    ensure(
        &registry,
        &client,
        &AdbOperationControl::default(),
        |client| Ok(spawn_fixture(client, "server")),
    )
    .unwrap();
    assert_eq!(client.version().unwrap(), 41);

    assert_eq!(
        registry.release().finish(Duration::from_secs(1)),
        ServerShutdownReport {
            terminated: 1,
            ..ServerShutdownReport::default()
        }
    );
    wait_closed(client.server);

    // Release is not terminal: work coming back starts a fresh server.
    ensure(
        &registry,
        &client,
        &AdbOperationControl::default(),
        |client| Ok(spawn_fixture(client, "server")),
    )
    .unwrap();
    assert_eq!(client.version().unwrap(), 41);
    assert_eq!(
        registry.begin_shutdown().finish(Duration::from_secs(1)),
        ServerShutdownReport {
            terminated: 1,
            ..ServerShutdownReport::default()
        }
    );
}

#[test]
fn release_never_touches_foreign_servers() {
    let registry = ServerRegistry::default();
    let client = client();
    let mut foreign = OwnedServer(Some(spawn_fixture(&client, "server")), None);
    wait_ready(&client);
    ensure(&registry, &client, &AdbOperationControl::default(), |_| {
        panic!("foreign server was replaced")
    })
    .unwrap();

    assert_eq!(
        registry.release().finish(Duration::from_secs(1)),
        ServerShutdownReport::default()
    );
    assert_eq!(client.version().unwrap(), 41);
    assert!(foreign.0.as_mut().unwrap().try_wait().unwrap().is_none());
}

#[test]
fn a_pending_release_never_stops_a_server_started_after_it() {
    let registry = ServerRegistry::default();
    let client = client();
    ensure(
        &registry,
        &client,
        &AdbOperationControl::default(),
        |client| Ok(spawn_fixture(client, "server")),
    )
    .unwrap();
    assert_eq!(client.version().unwrap(), 41);

    // The last lease schedules this cleanup on a worker; the stop happens later.
    let pending = registry.release();
    // The user comes back before that worker runs, and the first server is gone.
    stop_fixture(client.server);
    wait_closed(client.server);
    ensure(
        &registry,
        &client,
        &AdbOperationControl::default(),
        |client| Ok(spawn_fixture(client, "server")),
    )
    .unwrap();
    assert_eq!(client.version().unwrap(), 41);

    // Finishing the older release must stop what it claimed, not the
    // replacement that started in the meantime.
    pending.finish(Duration::from_secs(1));
    assert_eq!(client.version().unwrap(), 41);
    assert_eq!(
        registry.begin_shutdown().finish(Duration::from_secs(1)),
        ServerShutdownReport {
            terminated: 1,
            ..ServerShutdownReport::default()
        }
    );
}
