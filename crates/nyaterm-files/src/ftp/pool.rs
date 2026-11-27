//! Session-owned connection leases. Factories and credentials belong to the caller.
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use suppaftp::NativeTlsFtpStream;

use crate::ftp::io::SessionIo;
use crate::ftp::{FtpError, FtpFileService};
use crate::sftp::SftpTransferControl;

pub type ConnectCancellation = Arc<dyn Fn() -> bool + Send + Sync>;
pub type ConnectionFactory =
    dyn Fn(ConnectCancellation) -> Result<FtpFileService, FtpError> + Send + Sync;

pub(super) struct Connection {
    pub stream: NativeTlsFtpStream,
    pub io: Arc<SessionIo>,
    pub secure: bool,
    pub last_used: Instant,
}

struct Slot {
    connection: Option<Connection>,
    io: Option<Arc<SessionIo>>,
    busy: bool,
}

struct State {
    slots: Vec<Slot>,
    factory: Option<Weak<ConnectionFactory>>,
    limit: usize,
    closed: bool,
    maintenance_stopped: bool,
}

pub(super) struct Pool {
    state: Mutex<State>,
    wake: Condvar,
    stop: SftpTransferControl,
    retries: std::sync::atomic::AtomicU32,
}

impl Pool {
    pub fn new(connection: Connection) -> Self {
        Self {
            state: Mutex::new(State {
                slots: vec![Slot {
                    io: Some(connection.io.clone()),
                    connection: Some(connection),
                    busy: false,
                }],
                factory: None,
                limit: 0,
                closed: false,
                maintenance_stopped: false,
            }),
            wake: Condvar::new(),
            stop: SftpTransferControl::new(),
            retries: std::sync::atomic::AtomicU32::new(0),
        }
    }

    pub fn configure(&self, factory: Weak<ConnectionFactory>, limit: usize, retries: u32) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.retries
            .store(retries.min(10), std::sync::atomic::Ordering::Release);
        state.factory = Some(factory);
        state.maintenance_stopped = false;
        state.limit = limit.clamp(1, 10);
        self.wake.notify_all();
    }

    pub fn max_retries(&self) -> u32 {
        self.retries.load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn available(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        !state.closed
            && (state.factory.as_ref().is_some_and(|f| f.strong_count() > 0)
                || state
                    .slots
                    .iter()
                    .any(|s| s.io.as_ref().is_some_and(|io| io.valid())))
    }

    pub fn stop_maintenance(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .maintenance_stopped = true;
    }

    pub fn metadata_needs_reconnect(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        !state.closed
            && !state.maintenance_stopped
            && !state.slots[0].busy
            && state.factory.as_ref().is_some_and(|f| f.strong_count() > 0)
            && state.slots[0].io.as_ref().is_none_or(|io| !io.valid())
    }
    #[cfg(test)]
    pub(crate) fn expire_metadata(&self, disconnected: bool) {
        let mut state = self.state.lock().unwrap();
        if disconnected {
            state.slots[0].io.as_ref().unwrap().close();
        }
        if let Some(connection) = &mut state.slots[0].connection {
            connection.last_used = Instant::now() - Duration::from_secs(31);
        }
    }
    pub fn close(&self) {
        self.stop.cancel();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.closed = true;
        state.factory = None;
        for slot in &state.slots {
            if let Some(io) = &slot.io {
                io.close();
            }
        }
        self.wake.notify_all();
    }

    pub fn acquire(
        &self,
        transfer: bool,
        control: &SftpTransferControl,
        deadline: Option<Instant>,
    ) -> Result<PooledConnection<'_>, FtpError> {
        let mut state = self.state.lock().map_err(|_| FtpError::Unavailable)?;
        loop {
            if state.closed || control.is_cancelled() {
                return Err(FtpError::Cancelled);
            }
            if deadline.is_some_and(|at| Instant::now() >= at) {
                return Err(FtpError::Timeout);
            }
            // Standalone services preserve their single-connection behavior. Desktop
            // installs a weak factory before exposing a session to background jobs.
            let separate = transfer && state.limit > 0;
            let index = if separate {
                (1..state.slots.len()).find(|&i| !state.slots[i].busy)
            } else {
                (!state.slots[0].busy).then_some(0)
            };
            let index = index.or_else(|| {
                if separate && state.slots.len() <= state.limit {
                    let i = state.slots.len();
                    state.slots.push(Slot {
                        connection: None,
                        io: None,
                        busy: false,
                    });
                    Some(i)
                } else {
                    None
                }
            });
            if let Some(index) = index {
                state.slots[index].busy = true;
                let connection = state.slots[index].connection.take();
                if let Some(connection) = connection.filter(|c| c.io.valid()) {
                    return Ok(PooledConnection {
                        pool: self,
                        index,
                        connection: Some(connection),
                    });
                }
                let factory = state.factory.as_ref().and_then(Weak::upgrade);
                drop(state);
                let stop = self.stop.clone();
                let task = control.clone();
                let result = factory
                    .ok_or(FtpError::Unavailable)
                    .and_then(|factory| {
                        factory(Arc::new(move || {
                            stop.is_cancelled()
                                || task.is_cancelled()
                                || deadline.is_some_and(|at| Instant::now() >= at)
                        }))
                    })
                    .and_then(|service| service.pool.take_initial());
                state = self.state.lock().map_err(|_| FtpError::Unavailable)?;
                match result {
                    Ok(connection) if !state.closed && !control.is_cancelled() => {
                        state.slots[index].io = Some(connection.io.clone());
                        return Ok(PooledConnection {
                            pool: self,
                            index,
                            connection: Some(connection),
                        });
                    }
                    result => {
                        state.slots[index].busy = false;
                        self.wake.notify_all();
                        return Err(match result {
                            Ok(connection) => {
                                connection.io.close();
                                FtpError::Cancelled
                            }
                            Err(error) => error,
                        });
                    }
                }
            }
            state = self
                .wake
                .wait_timeout(state, Duration::from_millis(25))
                .map_err(|_| FtpError::Unavailable)?
                .0;
        }
    }

    fn take_initial(&self) -> Result<Connection, FtpError> {
        let mut state = self.state.lock().map_err(|_| FtpError::Unavailable)?;
        let slot = &mut state.slots[0];
        slot.io = None;
        slot.connection.take().ok_or(FtpError::Unavailable)
    }

    /// Keepalive probes only idle slots and never borrows an active transfer.
    pub fn idle(&self, idle_for: Duration) -> Vec<PooledConnection<'_>> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed || state.maintenance_stopped {
            return Vec::new();
        }
        state
            .slots
            .iter_mut()
            .enumerate()
            .filter_map(|(index, slot)| {
                if slot.busy
                    || !slot
                        .connection
                        .as_ref()
                        .is_some_and(|c| c.io.valid() && c.last_used.elapsed() >= idle_for)
                {
                    return None;
                }
                slot.busy = true;
                Some(PooledConnection {
                    pool: self,
                    index,
                    connection: slot.connection.take(),
                })
            })
            .collect()
    }
}

pub(super) struct PooledConnection<'a> {
    pool: &'a Pool,
    index: usize,
    connection: Option<Connection>,
}

impl std::ops::Deref for PooledConnection<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.connection.as_ref().expect("owned connection")
    }
}
impl std::ops::DerefMut for PooledConnection<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        self.connection.as_mut().expect("owned connection")
    }
}
impl Drop for PooledConnection<'_> {
    fn drop(&mut self) {
        let mut state = self.pool.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut connection) = self.connection.take() {
            connection.last_used = Instant::now();
            if state.closed {
                connection.io.close();
            }
            if connection.io.valid() {
                state.slots[self.index].connection = Some(connection);
            }
        }
        state.slots[self.index].busy = false;
        self.pool.wake.notify_all();
    }
}
