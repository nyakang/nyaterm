//! Socket ownership and bounded cancellation for the blocking FTP adapter.
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::ftp::error::FtpError;

pub(super) const IO_TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(25);

pub(super) fn connect_socket(
    addresses: Vec<SocketAddr>,
    timeout: Duration,
    cancelled: impl Fn() -> bool,
) -> Result<TcpStream, FtpError> {
    connect_target(Target::Resolved(addresses), timeout, cancelled)
}

pub(super) fn connect_host(
    host: String,
    port: u16,
    timeout: Duration,
    cancelled: impl Fn() -> bool,
) -> Result<TcpStream, FtpError> {
    connect_target(Target::Host(host, port), timeout, cancelled)
}
enum Target {
    Resolved(Vec<SocketAddr>),
    Host(String, u16),
}
fn connect_target(
    target: Target,
    timeout: Duration,
    cancelled: impl Fn() -> bool,
) -> Result<TcpStream, FtpError> {
    static RUNTIME: OnceLock<std::io::Result<tokio::runtime::Runtime>> = OnceLock::new();
    let runtime = RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .thread_name("ftp-connect")
                .build()
        })
        .as_ref()
        .map_err(|_| FtpError::Unavailable)?;
    let (send, receive) = mpsc::sync_channel(1);
    let task = runtime.spawn(async move {
        let result = tokio::time::timeout(timeout, async move {
            let addresses = match target {
                Target::Resolved(addresses) => addresses,
                Target::Host(host, port) => tokio::net::lookup_host((host, port)).await?.collect(),
            };
            tokio::net::TcpStream::connect(addresses.as_slice()).await
        })
        .await
        .map_err(|_| FtpError::Timeout)
        .and_then(|result| result.map_err(|_| FtpError::Connection))
        .and_then(|socket| socket.into_std().map_err(|_| FtpError::Connection));
        let _ = send.send(result);
    });
    loop {
        if cancelled() {
            task.abort();
            return Err(FtpError::Cancelled);
        }
        match receive.recv_timeout(POLL) {
            Ok(result) => {
                let socket = result?;
                socket
                    .set_nonblocking(false)
                    .map_err(|_| FtpError::Connection)?;
                socket
                    .set_read_timeout(Some(IO_TIMEOUT))
                    .map_err(|_| FtpError::Connection)?;
                socket
                    .set_write_timeout(Some(IO_TIMEOUT))
                    .map_err(|_| FtpError::Connection)?;
                return Ok(socket);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return Err(FtpError::Unavailable),
        }
    }
}

pub(super) struct SessionIo {
    control: TcpStream,
    data: Mutex<Option<TcpStream>>,
    valid: AtomicBool,
    interrupted: AtomicBool,
}

impl SessionIo {
    pub(super) fn new(control: &TcpStream) -> Result<Arc<Self>, FtpError> {
        Ok(Arc::new(Self {
            control: control.try_clone().map_err(|_| FtpError::Connection)?,
            data: Mutex::new(None),
            valid: AtomicBool::new(true),
            interrupted: AtomicBool::new(false),
        }))
    }

    pub(super) fn valid(&self) -> bool {
        self.valid.load(Ordering::Acquire)
    }
    pub(super) fn interrupted(&self) -> bool {
        !self.valid() || self.interrupted.load(Ordering::Acquire)
    }
    pub(super) fn set_data(&self, socket: &TcpStream) -> Result<(), FtpError> {
        *self.data.lock().map_err(|_| FtpError::Unavailable)? =
            Some(socket.try_clone().map_err(|_| FtpError::Connection)?);
        Ok(())
    }
    fn stop_data(&self) -> bool {
        if let Ok(data) = self.data.lock()
            && let Some(socket) = data.as_ref()
        {
            let _ = socket.shutdown(Shutdown::Both);
            return true;
        }
        false
    }
    pub(super) fn close(&self) {
        self.valid.store(false, Ordering::Release);
        let _ = self.control.shutdown(Shutdown::Both);
        self.stop_data();
    }
}

struct WatchState {
    done: bool,
    progress: Instant,
    cleanup: Option<Instant>,
    cancelled: bool,
    timed_out: bool,
}

pub(super) struct Operation {
    io: Arc<SessionIo>,
    state: Arc<(Mutex<WatchState>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

impl Operation {
    pub(super) fn start(
        io: Arc<SessionIo>,
        cancelled: impl Fn() -> bool + Send + 'static,
        paused: impl Fn() -> bool + Send + 'static,
    ) -> Result<Self, FtpError> {
        Self::start_with_timeout(io, cancelled, paused, IO_TIMEOUT)
    }

    pub(super) fn start_with_timeout(
        io: Arc<SessionIo>,
        cancelled: impl Fn() -> bool + Send + 'static,
        paused: impl Fn() -> bool + Send + 'static,
        timeout: Duration,
    ) -> Result<Self, FtpError> {
        io.interrupted.store(false, Ordering::Release);
        if let Ok(mut data) = io.data.lock() {
            *data = None;
        }
        let state = Arc::new((
            Mutex::new(WatchState {
                done: false,
                progress: Instant::now(),
                cleanup: None,
                cancelled: false,
                timed_out: false,
            }),
            Condvar::new(),
        ));
        let monitor = Arc::clone(&state);
        let sockets = Arc::clone(&io);
        let worker = std::thread::Builder::new()
            .name("ftp-io-watch".into())
            .spawn(move || {
                let (lock, wake) = &*monitor;
                let Ok(mut state) = lock.lock() else {
                    sockets.close();
                    return;
                };
                while !state.done {
                    let now = Instant::now();
                    if cancelled() && !state.cancelled {
                        state.cancelled = true;
                        sockets.interrupted.store(true, Ordering::Release);
                        if !sockets.stop_data() {
                            sockets.close();
                        }
                        state.cleanup = Some(now + Duration::from_millis(500));
                    }
                    if paused() {
                        state.progress = now;
                    }
                    if state.cleanup.is_some_and(|deadline| now >= deadline)
                        || now.duration_since(state.progress) >= timeout
                    {
                        state.timed_out = !state.cancelled;
                        sockets.close();
                        break;
                    }
                    let Ok((next, _)) = wake.wait_timeout(state, POLL) else {
                        sockets.close();
                        return;
                    };
                    state = next;
                }
            })
            .map_err(|_| FtpError::Unavailable)?;
        Ok(Self {
            io,
            state,
            worker: Some(worker),
        })
    }

    pub(super) fn check(&self) -> Result<(), FtpError> {
        let state = self.state.0.lock().map_err(|_| FtpError::Unavailable)?;
        if state.cancelled {
            Err(FtpError::Cancelled)
        } else if state.timed_out {
            Err(FtpError::Timeout)
        } else if !self.io.valid() {
            Err(FtpError::Unavailable)
        } else {
            Ok(())
        }
    }
    pub(super) fn progress(&self) {
        if let Ok(mut state) = self.state.0.lock() {
            state.progress = Instant::now();
        }
    }
    pub(super) fn cleanup(&self) {
        if let Ok(mut state) = self.state.0.lock() {
            state.cleanup = Some(Instant::now() + Duration::from_millis(500));
        }
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.0.lock() {
            state.done = true;
        }
        self.state.1.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Ok(mut data) = self.io.data.lock() {
            *data = None;
        }
    }
}
