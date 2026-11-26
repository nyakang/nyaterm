//! One application-wide tracker; view channels do not own discovery workers.
use crate::adb::client::{AdbClient, AdbDevice, parse_devices};
use crate::adb::runtime::DeviceRuntime;
use crate::adb::server::AdbServerManager;
use crate::error::{AdbError, AdbResult};
use crate::operation::AdbOperationControl;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, TryLockError, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
pub struct DeviceSnapshot {
    pub devices: Vec<AdbDevice>,
    pub error: Option<String>,
    pub revision: u64,
    pub server_generation: u64,
    pub device_generations: HashMap<String, u64>,
}

pub struct DeviceService {
    control: AdbOperationControl,
    worker: Mutex<Option<JoinHandle<()>>>,
    state: Arc<Mutex<DeviceSnapshot>>,
    channel: Arc<Mutex<Option<TcpStream>>>,
    runtimes: Arc<Mutex<Vec<Weak<DeviceRuntime>>>>,
}
type SharedTracker = Arc<Mutex<Option<(AdbClient, Arc<DeviceService>)>>>;
#[derive(Clone, Default)]
pub struct SharedDeviceService {
    tracker: SharedTracker,
    control: AdbOperationControl,
    servers: AdbServerManager,
    /// Live consumers of discovery; the last drop stops the tracker.
    leases: Arc<AtomicUsize>,
    /// Workers of released services, joined at shutdown or the next start.
    retired: Arc<Mutex<Vec<JoinHandle<()>>>>,
}
impl SharedDeviceService {
    pub fn server_manager(&self) -> AdbServerManager {
        self.servers.clone()
    }

    /// Keeps discovery running while held; take it before [`Self::ensure`].
    #[must_use = "discovery stops as soon as the lease is dropped"]
    pub fn lease(&self) -> DeviceTrackerLease {
        self.leases.fetch_add(1, Ordering::AcqRel);
        DeviceTrackerLease {
            devices: self.clone(),
        }
    }

    fn drop_lease(&self) {
        if self.leases.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }
        let service = self
            .tracker
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some((_, service)) = service {
            service.stop();
            let worker = service
                .worker
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .take();
            if let Some(worker) = worker {
                self.retired
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(worker);
            }
        }
        // No consumer left: stop the servers this owner started, but never a
        // foreign one. Safe here because dropping a lease happens on the UI
        // thread, so the waiting part moves to a worker.
        let release = self.servers.release();
        let _ = nyaterm_core::runtime::worker::spawn("adb-server-release".into(), move || {
            release.finish(Duration::from_secs(2));
        });
    }

    pub fn ensure(&self, client: &AdbClient) -> AdbResult<()> {
        self.control.check()?;
        // No lease means nobody is watching devices; starting a tracker would
        // spawn (and keep restarting) an adb server for nobody.
        if self.leases.load(Ordering::Acquire) == 0 {
            return Ok(());
        }
        let mut state = self.tracker.lock().unwrap_or_else(|p| p.into_inner());
        self.control.check()?;
        if state.as_ref().is_some_and(|(existing, _)| {
            existing.executable == client.executable && existing.server == client.server
        }) {
            return Ok(());
        }
        let client = client.clone().with_server_manager(self.servers.clone());
        // Otherwise every release/ensure cycle keeps a dead handle for life.
        self.retired
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|worker| !worker.is_finished());
        *state = Some((
            client.clone(),
            Arc::new(DeviceService::start_with_control(
                client,
                self.control.child(),
            )?),
        ));
        Ok(())
    }
    pub fn register_runtime_since(
        &self,
        runtime: &Arc<DeviceRuntime>,
        expected: &DeviceSnapshot,
    ) -> AdbResult<()> {
        self.control.check()?;
        let shared = self.tracker.lock().unwrap_or_else(|p| p.into_inner());
        let Some((_, tracker)) = shared.as_ref() else {
            runtime.invalidate();
            return Err(AdbError::Offline);
        };
        // Tracker publishes generations and invalidation under this same lock order.
        let snapshot = tracker.state.lock().unwrap_or_else(|p| p.into_inner());
        if snapshot.server_generation != expected.server_generation
            || snapshot.device_generations.get(&runtime.binding.serial)
                != expected.device_generations.get(&runtime.binding.serial)
            || !snapshot.devices.iter().any(|device| {
                device.serial == runtime.binding.serial
                    && device.status == crate::adb::client::DeviceStatus::Online
                    && device.transport_id == runtime.binding.transport_id
            })
        {
            runtime.invalidate();
            return Err(AdbError::Offline);
        }
        let mut runtimes = tracker.runtimes.lock().unwrap_or_else(|p| p.into_inner());
        runtimes.retain(|runtime| runtime.strong_count() > 0);
        runtimes.push(Arc::downgrade(runtime));
        Ok(())
    }
    /// Whether a tracker currently serves this owner.
    pub fn is_active(&self) -> bool {
        self.tracker
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
    }

    pub fn snapshot(&self) -> DeviceSnapshot {
        self.tracker
            .try_lock()
            .ok()
            .as_deref()
            .and_then(|state| state.as_ref())
            .map(|(_, service)| service.snapshot())
            .unwrap_or_default()
    }

    /// Stop admitting discovery work immediately; transfer all waiting to a
    /// background owner, including startup that currently holds the tracker lock.
    pub fn begin_shutdown(&self) -> DeviceServiceShutdown {
        self.control.cancel();
        DeviceServiceShutdown {
            tracker: Arc::clone(&self.tracker),
            retired: Arc::clone(&self.retired),
        }
    }
}

/// Keeps discovery alive while held; the last drop stops the tracker without
/// cancelling the service, so a later lease and `ensure` start a fresh one.
pub struct DeviceTrackerLease {
    devices: SharedDeviceService,
}

impl Drop for DeviceTrackerLease {
    fn drop(&mut self) {
        self.devices.drop_lease();
    }
}

#[must_use = "finish discovery shutdown on a background worker"]
pub struct DeviceServiceShutdown {
    tracker: SharedTracker,
    retired: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl DeviceServiceShutdown {
    /// Returns false if the worker could not be collected before the deadline.
    /// Cancellation remains active even if an OS operation outlives this wait.
    pub fn finish(self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let service = loop {
            match self.tracker.try_lock() {
                Ok(mut tracker) => break tracker.take(),
                Err(TryLockError::Poisoned(poisoned)) => break poisoned.into_inner().take(),
                Err(TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return false;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        };
        let mut stopped = true;
        if let Some((_, service)) = service {
            service.stop();
            let worker = service
                .worker
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .take();
            if let Some(worker) = worker {
                stopped = join_worker(worker, deadline);
            }
        }
        // Services released by their last lease still have a worker to collect.
        let retired = std::mem::take(&mut *self.retired.lock().unwrap_or_else(|p| p.into_inner()));
        stopped
            & retired
                .into_iter()
                .all(|worker| join_worker(worker, deadline))
    }
}

fn join_worker(worker: JoinHandle<()>, deadline: Instant) -> bool {
    while !worker.is_finished() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    worker.join().is_ok()
}

impl DeviceService {
    /// Startup belongs on a background job: probing/starting ADB may block.
    pub fn start(client: AdbClient) -> AdbResult<Self> {
        Self::start_with_control(client, AdbOperationControl::default())
    }

    fn start_with_control(client: AdbClient, control: AdbOperationControl) -> AdbResult<Self> {
        client.ensure_server_with_control(&control)?;
        control.check()?;
        let state = Arc::new(Mutex::new(DeviceSnapshot::default()));
        let channel = Arc::new(Mutex::new(None));
        let worker_control = control.clone();
        let worker_state = state.clone();
        let worker_channel = channel.clone();
        let runtimes = Arc::new(Mutex::new(Vec::new()));
        let worker_runtimes = runtimes.clone();
        let worker =
            nyaterm_core::runtime::worker::spawn("android-device-tracker".into(), move || {
                while !worker_control.is_cancelled() {
                    let result = (|| {
                        let mut stream = client.tracker_with_control(
                            &worker_control.with_timeout(Duration::from_secs(15)),
                        )?;
                        *worker_channel.lock().unwrap_or_else(|p| p.into_inner()) =
                            Some(stream.try_clone()?);
                        let mut pending = Vec::new();
                        let mut buffer = [0; 8192];
                        loop {
                            if worker_control.is_cancelled() {
                                return Ok(());
                            }
                            match stream.read(&mut buffer) {
                                Ok(0) => return Err(AdbError::Offline),
                                Ok(count) => pending.extend_from_slice(&buffer[..count]),
                                Err(error)
                                    if matches!(
                                        error.kind(),
                                        std::io::ErrorKind::TimedOut
                                            | std::io::ErrorKind::WouldBlock
                                    ) =>
                                {
                                    continue;
                                }
                                Err(error) => return Err(error.into()),
                            }
                            while pending.len() >= 4 {
                                let header = std::str::from_utf8(&pending[..4])
                                    .map_err(|_| AdbError::Protocol("tracker length"))?;
                                let length = usize::from_str_radix(header, 16)
                                    .map_err(|_| AdbError::Protocol("tracker length"))?;
                                if pending.len() < length + 4 {
                                    break;
                                }
                                let devices = parse_devices(&pending[4..length + 4])?;
                                pending.drain(..length + 4);
                                let mut state =
                                    worker_state.lock().unwrap_or_else(|p| p.into_inner());
                                update_snapshot(&mut state, devices);
                                invalidate_changed(&worker_runtimes, &state.devices);
                            }
                        }
                    })();
                    if worker_control.is_cancelled() {
                        break;
                    }
                    let session_error = result.err();
                    // The track stream only breaks when the server stops answering:
                    // either it exited or the socket path went away. Nothing else in
                    // the application re-runs `start-server` once a tracker exists,
                    // so a daemon that was stopped (by another tool, an update, a
                    // crash) would otherwise leave discovery dead and every session
                    // open failing with a bare "cannot communicate" I/O error.
                    let server_unreachable = matches!(
                        session_error.as_ref(),
                        Some(AdbError::Io(error))
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::ConnectionRefused
                                    | std::io::ErrorKind::TimedOut
                                    | std::io::ErrorKind::WouldBlock
                            )
                    );
                    let mut state = worker_state.lock().unwrap_or_else(|p| p.into_inner());
                    update_snapshot(&mut state, Vec::new());
                    state.server_generation = state.server_generation.wrapping_add(1);
                    state.error = session_error.map(|e| e.to_string());
                    invalidate_changed(&worker_runtimes, &[]);
                    drop(state);
                    if server_unreachable {
                        // Best effort: the next loop iteration reconnects either way,
                        // and the error stays published when the server cannot start.
                        let _ = client.ensure_server_with_control(&worker_control);
                    }
                    for _ in 0..10 {
                        if worker_control.is_cancelled() {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            })?;
        Ok(Self {
            control,
            worker: Mutex::new(Some(worker)),
            state,
            channel,
            runtimes,
        })
    }
    pub fn snapshot(&self) -> DeviceSnapshot {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
    pub fn stop(&self) {
        self.control.cancel();
        invalidate_changed(&self.runtimes, &[]);
        if let Some(stream) = self
            .channel
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}
impl Drop for DeviceService {
    fn drop(&mut self) {
        self.stop();
    }
}

fn update_snapshot(state: &mut DeviceSnapshot, devices: Vec<AdbDevice>) {
    let mut changed = HashSet::new();
    for device in state.devices.iter().chain(devices.iter()) {
        let before = state
            .devices
            .iter()
            .find(|entry| entry.serial == device.serial);
        let after = devices.iter().find(|entry| entry.serial == device.serial);
        if before.map(|d| (d.status, d.transport_id)) != after.map(|d| (d.status, d.transport_id))
            && changed.insert(device.serial.clone())
        {
            let generation = state
                .device_generations
                .entry(device.serial.clone())
                .or_default();
            *generation = generation.wrapping_add(1);
        }
    }
    state.devices = devices;
    state.error = None;
    state.revision = state.revision.wrapping_add(1);
}
fn invalidate_changed(runtimes: &Mutex<Vec<Weak<DeviceRuntime>>>, devices: &[AdbDevice]) {
    runtimes
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .retain(|weak| {
            let Some(runtime) = weak.upgrade() else {
                return false;
            };
            if !devices.iter().any(|device| {
                device.serial == runtime.binding.serial
                    && device.transport_id == runtime.binding.transport_id
                    && device.status == crate::adb::client::DeviceStatus::Online
            }) {
                runtime.invalidate();
            }
            true
        });
}

#[cfg(test)]
mod shutdown_tests;

#[cfg(test)]
mod transition_tests {
    use crate::adb::client::{AdbDevice, DeviceBinding, DeviceStatus};
    use crate::adb::runtime::DeviceRuntime;
    use crate::devices::{DeviceSnapshot, invalidate_changed, update_snapshot};
    use std::sync::{Arc, Mutex};
    #[test]
    fn transient_offline_invalidates_a_binding_even_when_the_latest_snapshot_is_online() {
        let device = AdbDevice {
            serial: "fixture".into(),
            transport_id: Some(7),
            model: None,
            status: DeviceStatus::Online,
        };
        let runtime = Arc::new(DeviceRuntime::new(DeviceBinding {
            server: "127.0.0.1:1".parse().unwrap(),
            serial: device.serial.clone(),
            transport_id: Some(7),
            generation: 1,
        }));
        let runtimes = Mutex::new(vec![Arc::downgrade(&runtime)]);
        let mut snapshot = DeviceSnapshot::default();
        update_snapshot(&mut snapshot, vec![device.clone()]);
        let generation = snapshot.device_generations["fixture"];
        update_snapshot(&mut snapshot, vec![]);
        invalidate_changed(&runtimes, &snapshot.devices);
        update_snapshot(&mut snapshot, vec![device]);
        assert!(!runtime.is_valid());
        assert_eq!(snapshot.device_generations["fixture"], generation + 2);
    }
}
