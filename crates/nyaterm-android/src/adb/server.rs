//! Owned foreground ADB processes. A socket address never establishes ownership.

use crate::adb::client::{AdbClient, server_unreachable};
use crate::error::{AdbError, AdbResult};
use crate::operation::AdbOperationControl;
use std::collections::HashMap;
use std::io::{self, Read};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, TryLockError};
use std::time::{Duration, Instant};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Default)]
pub(crate) struct ServerRegistry {
    control: AdbOperationControl,
    slots: Mutex<HashMap<SocketAddr, Arc<ServerSlot>>>,
}

#[derive(Default)]
struct ServerSlot {
    // Only background startup callers wait on this lock. Shutdown never takes it.
    startup: Mutex<()>,
    process: Mutex<Option<OwnedServer>>,
}

struct OwnedServer(Option<Child>);

impl Drop for OwnedServer {
    fn drop(&mut self) {
        let Some(mut child) = self.0.take() else {
            return;
        };
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        let _ = child.kill();
        // Rollback and deadline expiry must not introduce an unbounded wait.
        // Keep the process handle until the OS has reaped this exact child.
        let _ = nyaterm_core::runtime::worker::spawn("adb-server-reaper".into(), move || {
            let _ = child.wait();
        });
    }
}

impl ServerSlot {
    fn take_process(&self) -> Option<OwnedServer> {
        self.process
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
    }

    fn running(&self) -> io::Result<bool> {
        let mut process = self.process.lock().unwrap_or_else(|p| p.into_inner());
        let Some(child) = process.as_mut().and_then(|process| process.0.as_mut()) else {
            return Ok(false);
        };
        if child.try_wait()?.is_some() {
            // try_wait has already reaped it; no pid survives for later cleanup.
            process.take();
            Ok(false)
        } else {
            Ok(true)
        }
    }
}

/// One server lifetime shared by an application's workspace clients.
#[derive(Clone, Default)]
pub struct AdbServerManager {
    registry: Arc<ServerRegistry>,
}

impl std::fmt::Debug for AdbServerManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdbServerManager")
            .field("shutting_down", &self.registry.control.is_cancelled())
            .finish_non_exhaustive()
    }
}

impl AdbServerManager {
    pub(crate) fn ensure(
        &self,
        client: &AdbClient,
        control: &AdbOperationControl,
    ) -> AdbResult<()> {
        self.registry.ensure(client, control)
    }

    /// Cancels this owner's startup operations permanently without waiting for I/O.
    pub fn begin_shutdown(&self) -> AdbServerShutdown {
        self.registry.begin_shutdown()
    }
}

pub(crate) fn default_manager() -> &'static AdbServerManager {
    static MANAGER: OnceLock<AdbServerManager> = OnceLock::new();
    MANAGER.get_or_init(AdbServerManager::default)
}

pub(crate) fn connect(server: SocketAddr, control: &AdbOperationControl) -> AdbResult<TcpStream> {
    control.check()?;
    let result = TcpStream::connect_timeout(&server, control.remaining(Duration::from_millis(100)));
    control.check()?;
    let stream = result?;
    stream.set_nodelay(true)?;
    Ok(stream)
}

fn startup_lock<'a>(
    slot: &'a ServerSlot,
    control: &AdbOperationControl,
) -> AdbResult<MutexGuard<'a, ()>> {
    loop {
        control.check()?;
        match slot.startup.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::Poisoned(poisoned)) => return Ok(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => std::thread::sleep(POLL_INTERVAL),
        }
    }
}

fn spawn_foreground(client: &AdbClient) -> io::Result<Option<Child>> {
    // Shell/batch launchers can leave the real daemon in an unowned descendant
    // after their own handle exits. Preserve their legacy startup path instead.
    if client
        .executable
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
        })
    {
        return Ok(None);
    }
    let mut prefix = [0; 2];
    if std::fs::File::open(&client.executable)?.read(&mut prefix)? == 2 && prefix == *b"#!" {
        return Ok(None);
    }
    client
        .process(&["server", "nodaemon"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(Some)
}

impl ServerRegistry {
    pub(crate) fn ensure(
        &self,
        client: &AdbClient,
        control: &AdbOperationControl,
    ) -> AdbResult<()> {
        let control = self
            .control
            .linked_to(control)
            .with_timeout(STARTUP_TIMEOUT);
        self.ensure_connected(
            client,
            &control,
            connect(client.server, &control),
            spawn_foreground,
        )
    }

    #[cfg(test)]
    pub(crate) fn ensure_with_connection(
        &self,
        client: &AdbClient,
        control: &AdbOperationControl,
        connection: io::Result<TcpStream>,
    ) -> AdbResult<()> {
        let control = self
            .control
            .linked_to(control)
            .with_timeout(STARTUP_TIMEOUT);
        self.ensure_connected(
            client,
            &control,
            connection.map_err(AdbError::from),
            spawn_foreground,
        )
    }

    fn ensure_connected(
        &self,
        client: &AdbClient,
        control: &AdbOperationControl,
        connection: AdbResult<TcpStream>,
        spawn: impl FnOnce(&AdbClient) -> io::Result<Option<Child>>,
    ) -> AdbResult<()> {
        control.check()?;
        let slots: Vec<_> = self
            .slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .cloned()
            .collect();
        for slot in slots {
            slot.running()?;
        }
        match connection {
            Ok(stream) => return AdbClient::version_on_stream(stream, control),
            Err(AdbError::Io(error)) if server_unreachable(&error) => {}
            Err(error) => return Err(error),
        }
        let slot = {
            let mut slots = self.slots.lock().unwrap_or_else(|p| p.into_inner());
            control.check()?;
            Arc::clone(slots.entry(client.server).or_default())
        };
        let _startup = startup_lock(&slot, control)?;
        // A competing caller or another application may have started a server
        // since our initial connect failed. Never run adb's replacement logic.
        match connect(client.server, control) {
            Ok(stream) => return AdbClient::version_on_stream(stream, control),
            Err(AdbError::Io(error)) if server_unreachable(&error) => {}
            Err(error) => return Err(error),
        }
        if client.server.ip() == Ipv4Addr::LOCALHOST {
            if !slot.running()?
                && let Some(child) = spawn(client)?
            {
                let process = OwnedServer(Some(child));
                let mut owned = slot.process.lock().unwrap_or_else(|p| p.into_inner());
                control.check()?;
                *owned = Some(process);
            }
            let started = (|| {
                while slot.running()? {
                    control.check()?;
                    match connect(client.server, control) {
                        Ok(stream) => {
                            AdbClient::version_on_stream(stream, control)?;
                            return Ok(true);
                        }
                        Err(AdbError::Io(error)) if server_unreachable(&error) => {}
                        Err(error) => return Err(error),
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
                control.check()?;
                Ok(false)
            })();
            match started {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(error) => {
                    drop(slot.take_process());
                    return Err(error);
                }
            }
        }
        // A wrapper may not support foreground mode. Re-probe before the
        // compatibility path, and never claim ownership of a detached daemon.
        match connect(client.server, control) {
            Ok(stream) => return AdbClient::version_on_stream(stream, control),
            Err(AdbError::Io(error)) if server_unreachable(&error) => {}
            Err(error) => return Err(error),
        }
        client.command_with_control(&["start-server"], control)?;
        AdbClient::version_on_stream(connect(client.server, control)?, control)
    }

    pub(crate) fn begin_shutdown(&self) -> AdbServerShutdown {
        self.control.cancel();
        let slots = std::mem::take(&mut *self.slots.lock().unwrap_or_else(|p| p.into_inner()));
        AdbServerShutdown {
            slots: slots.into_values().collect(),
        }
    }
}

/// Cancels startup and transfers owned process cleanup without waiting for I/O.
/// Terminal for this process; repeated calls return an empty cleanup.
pub fn begin_server_shutdown() -> AdbServerShutdown {
    default_manager().begin_shutdown()
}

#[must_use = "finish ADB cleanup on a background worker"]
pub struct AdbServerShutdown {
    slots: Vec<Arc<ServerSlot>>,
}

/// Counts only owned children. No subprocess output or device data is retained.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ServerShutdownReport {
    pub terminated: usize,
    pub already_exited: usize,
    pub failed: usize,
}

impl AdbServerShutdown {
    /// Signals all children first, then reaps them within one shared deadline.
    /// Remaining handles go to the background reaper; never wait on the UI thread.
    pub fn finish(self, timeout: Duration) -> ServerShutdownReport {
        let deadline = Instant::now() + timeout;
        let mut report = ServerShutdownReport::default();
        let mut pending = Vec::new();
        for slot in &self.slots {
            let Some(mut process) = slot.take_process() else {
                continue;
            };
            let child = process.0.as_mut().expect("owned ADB child");
            match child.try_wait() {
                Ok(Some(_)) => report.already_exited += 1,
                Ok(None) if child.kill().is_ok() => pending.push(process),
                _ => report.failed += 1,
            }
        }
        while !pending.is_empty() {
            let mut index = 0;
            while index < pending.len() {
                match pending[index]
                    .0
                    .as_mut()
                    .expect("owned ADB child")
                    .try_wait()
                {
                    Ok(Some(_)) => {
                        report.terminated += 1;
                        pending.swap_remove(index);
                    }
                    Err(_) => {
                        report.failed += 1;
                        pending.swap_remove(index);
                    }
                    Ok(None) => index += 1,
                }
            }
            if Instant::now() >= deadline {
                report.failed += pending.len();
                break;
            }
            if !pending.is_empty() {
                std::thread::sleep(
                    POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
                );
            }
        }
        report
    }
}

#[cfg(test)]
mod tests;
