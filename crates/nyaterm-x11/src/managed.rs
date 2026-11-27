use crate::process::OwnedChild;
use nyaterm_core::plugins::native::NativeLaunch;
use nyaterm_core::x11::{X11Connection, X11Endpoint, X11Error, X11Lifetime};
use nyaterm_native_protocol::x11::{self, Event, X11_PROTOCOL_VERSION};
use std::process::{Command, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

struct Lifetime {
    alive: Arc<AtomicBool>,
    stop: mpsc::SyncSender<()>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}
impl X11Lifetime for Lifetime {
    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }
    fn shutdown(&self) {
        let _ = self.stop.try_send(());
    }
    fn wait_stopped(&self) {
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}
impl Drop for Lifetime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn start_managed(
    launch: NativeLaunch,
    clipboard: bool,
    cancelled: &(impl Fn() -> bool + ?Sized),
) -> Result<X11Connection, X11Error> {
    if !cfg!(all(windows, target_arch = "x86_64")) {
        return Err(X11Error::Unsupported);
    }
    if cancelled() {
        return Err(X11Error::Cancelled);
    }
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (stop, stopped) = mpsc::sync_channel(1);
    let alive = Arc::new(AtomicBool::new(true));
    let lifetime = Arc::new(Lifetime {
        alive: alive.clone(),
        stop,
        worker: Mutex::new(None),
    });
    let worker = std::thread::Builder::new()
        .name("nyaterm-x11".into())
        .spawn(move || {
            struct MarkStopped(Arc<AtomicBool>);
            impl Drop for MarkStopped {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::Release);
                }
            }
            let _stopped = MarkStopped(alive);
            let result = supervise(launch, clipboard, &stopped, &ready_tx);
            if let Err(error) = result {
                let _ = ready_tx.try_send(Err(error));
            }
        })
        .map_err(|_| X11Error::Startup)?;
    *lifetime.worker.lock().unwrap() = Some(worker);
    let deadline = Instant::now() + Duration::from_secs(20);
    let result = (|| loop {
        if cancelled() {
            return Err(X11Error::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(X11Error::Startup);
        }
        match ready_rx.recv_timeout(Duration::from_millis(25)) {
            Ok(Ok(mut connection)) => {
                connection.lifetime = Some(lifetime.clone());
                return Ok(connection);
            }
            Ok(Err(error)) => return Err(error),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(X11Error::Startup),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    })();
    if result.is_err() {
        lifetime.shutdown();
        lifetime.wait_stopped();
    }
    result
}

fn supervise(
    launch: NativeLaunch,
    clipboard: bool,
    stopped: &mpsc::Receiver<()>,
    ready: &mpsc::SyncSender<Result<X11Connection, X11Error>>,
) -> Result<(), X11Error> {
    // The directory lease is retained until process-tree termination and pipe
    // reader reaping, including startup failure and application cancellation.
    let _directory_lease = launch.lease;
    let mut command = Command::new(launch.executable);
    command
        .current_dir(launch.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = OwnedChild::spawn(&mut command).map_err(|_| X11Error::Startup)?;
    let mut input = child.child_mut().stdin.take().ok_or(X11Error::Startup)?;
    let mut output = child.child_mut().stdout.take().ok_or(X11Error::Startup)?;
    x11::write(
        &mut input,
        &x11::Command::Start {
            version: X11_PROTOCOL_VERSION,
            clipboard,
        },
    )
    .map_err(|_| X11Error::Protocol)?;
    let (events, received) = mpsc::sync_channel(2);
    let reader = std::thread::spawn(move || {
        while let Ok(event) = x11::read::<Event>(&mut output) {
            if events.try_send(event).is_err() {
                break;
            }
        }
    });
    let result = (|| {
        let mut started = false;
        loop {
            if !matches!(stopped.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                return Ok(());
            }
            if child
                .child_mut()
                .try_wait()
                .map_err(|_| X11Error::Startup)?
                .is_some()
            {
                return Err(X11Error::Startup);
            }
            match received.recv_timeout(Duration::from_millis(25)) {
                Ok(Event::Ready {
                    version,
                    display,
                    cookie,
                }) if !started && version == X11_PROTOCOL_VERSION && display <= 59535 => {
                    let hex = cookie.expose_secret().as_bytes();
                    if hex.len() != 32 {
                        return Err(X11Error::Protocol);
                    }
                    let bytes: Option<Vec<u8>> = hex
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|pair| {
                            Some(
                                (pair[0] as char).to_digit(16)? as u8 * 16
                                    + (pair[1] as char).to_digit(16)? as u8,
                            )
                        })
                        .collect();
                    let connection = X11Connection {
                        endpoint: X11Endpoint::Tcp {
                            host: "127.0.0.1".into(),
                            port: 6000 + display,
                        },
                        cookie: Some(Zeroizing::new(bytes.ok_or(X11Error::Protocol)?)),
                        lifetime: None,
                    };
                    crate::system::probe(&connection)?;
                    ready
                        .send(Ok(connection))
                        .map_err(|_| X11Error::Cancelled)?;
                    started = true;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                _ => return Err(X11Error::Protocol),
            }
        }
    })();
    let _ = x11::write(&mut input, &x11::Command::Stop);
    drop(input);
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if child.child_mut().try_wait().ok().flatten().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    child.terminate();
    drop(received);
    let _ = reader.join();
    result
}

#[cfg(all(test, unix))]
mod tests {
    use crate::managed::supervise;
    use nyaterm_core::plugins::native::NativeLaunch;
    use nyaterm_core::x11::X11Error;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    use std::time::{Duration, Instant};

    struct DirectoryLease {
        pid_file: PathBuf,
        released: Arc<AtomicBool>,
    }
    impl Drop for DirectoryLease {
        fn drop(&mut self) {
            let pid: i32 = std::fs::read_to_string(&self.pid_file)
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            // SAFETY: signal zero only observes the fixture's recorded process.
            assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
            self.released.store(true, Ordering::Release);
        }
    }

    #[test]
    fn cancellation_and_invalid_control_reap_a_hung_helper_before_releasing_its_directory() {
        for cancel in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let executable = root.path().join("helper");
            let body = if cancel {
                "#!/bin/sh\necho $$ > pid\nexec sleep 30\n"
            } else {
                "#!/bin/sh\necho $$ > pid\nprintf '\\001\\000\\000\\000?'\nexec sleep 30\n"
            };
            std::fs::write(&executable, body).unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
            let released = Arc::new(AtomicBool::new(false));
            let pid_file = root.path().join("pid");
            let launch = NativeLaunch {
                executable,
                root: root.path().to_owned(),
                lease: Arc::new(DirectoryLease {
                    pid_file: pid_file.clone(),
                    released: released.clone(),
                }),
            };
            let (stop, stopped) = mpsc::sync_channel(1);
            let (ready, _events) = mpsc::sync_channel(1);
            let started = Instant::now();
            let worker = std::thread::spawn(move || supervise(launch, false, &stopped, &ready));
            while !pid_file.exists() {
                assert!(started.elapsed() < Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(!released.load(Ordering::Acquire));
            if cancel {
                stop.send(()).unwrap();
            }
            let result = worker.join().unwrap();
            assert_eq!(
                result,
                if cancel {
                    Ok(())
                } else {
                    Err(X11Error::Protocol)
                }
            );
            assert!(released.load(Ordering::Acquire));
            assert!(started.elapsed() < Duration::from_secs(8));
        }
    }
}
