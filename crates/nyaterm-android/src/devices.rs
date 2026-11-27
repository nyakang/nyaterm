//! One application-wide tracker; view channels do not own discovery workers.
use crate::adb::client::{AdbClient, AdbDevice, parse_devices};
use crate::adb::runtime::DeviceRuntime;
use crate::adb::server::AdbServerManager;
use crate::error::{AdbError, AdbResult};
use crate::operation::AdbOperationControl;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
pub struct DeviceSnapshot {
    pub devices: Vec<AdbDevice>,
    pub error: Option<String>,
    pub revision: u64,
    pub tracker_generation: u64,
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
#[derive(Default)]
struct DiscoveryDemand {
    leases: usize,
    control: AdbOperationControl,
}
#[derive(Clone, Default)]
pub struct SharedDeviceService {
    tracker: SharedTracker,
    control: AdbOperationControl,
    servers: AdbServerManager,
    demand: Arc<Mutex<DiscoveryDemand>>,
    // Only background callers take this lock. Startup, retirement and server
    // reaping form one transaction, including cancellation rollback.
    lifecycle: Arc<Mutex<()>>,
    retired: Arc<Mutex<Vec<Arc<DeviceService>>>>,
    cleanup: Arc<Mutex<Vec<JoinHandle<()>>>>,
    cleanup_running: Arc<AtomicBool>,
}
impl SharedDeviceService {
    pub fn server_manager(&self) -> AdbServerManager {
        self.servers.clone()
    }

    #[must_use = "discovery stops as soon as the lease is dropped"]
    pub fn lease(&self) -> DeviceTrackerLease {
        let mut demand = self.demand.lock().unwrap_or_else(|p| p.into_inner());
        if demand.leases == 0 {
            demand.control = self.control.child();
        }
        demand.leases += 1;
        DeviceTrackerLease {
            devices: self.clone(),
        }
    }

    /// Capture before queuing a job. A release/reacquire cannot revive this ticket.
    pub fn discovery_control(&self) -> AdbOperationControl {
        let demand = self.demand.lock().unwrap_or_else(|p| p.into_inner());
        let control = demand.control.child();
        if demand.leases == 0 {
            control.cancel();
        }
        control
    }

    fn retire_tracker(&self) {
        if let Some((_, service)) = self
            .tracker
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            // Only atomic cancellation on the UI thread; socket closure,
            // runtime invalidation and joining belong to cleanup below.
            service.control.cancel();
            self.retired
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(service);
        }
    }

    fn drop_lease(&self) {
        let mut demand = self.demand.lock().unwrap_or_else(|p| p.into_inner());
        demand.leases -= 1;
        if demand.leases != 0 {
            return;
        }
        demand.control.cancel();
        self.retire_tracker();
        self.schedule_cleanup();
    }

    fn schedule_cleanup(&self) {
        if self
            .retired
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_empty()
            && self.lifecycle.try_lock().is_ok()
        {
            return;
        }
        if self.cleanup_running.swap(true, Ordering::AcqRel) {
            return;
        }
        let owner = self.clone();
        let mut workers = self.cleanup.lock().unwrap_or_else(|p| p.into_inner());
        workers.retain(|worker| !worker.is_finished());
        match nyaterm_core::runtime::worker::spawn("android-discovery-cleanup".into(), move || {
            let _gate = owner.lifecycle.lock().unwrap_or_else(|p| p.into_inner());
            loop {
                owner.clean_retired();
                let _demand = owner.demand.lock().unwrap_or_else(|p| p.into_inner());
                if owner
                    .retired
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .is_empty()
                {
                    owner.cleanup_running.store(false, Ordering::Release);
                    break;
                }
            }
        }) {
            Ok(worker) => workers.push(worker),
            // State still owns all resources. A later ensure/shutdown retries
            // cleanup before admitting another lifecycle.
            Err(error) => {
                self.cleanup_running.store(false, Ordering::Release);
                tracing::error!(%error, "Cannot schedule Android cleanup");
            }
        }
    }

    // Requires lifecycle, never a UI-accessible mutex across blocking work.
    fn clean_retired(&self) {
        let retired = std::mem::take(&mut *self.retired.lock().unwrap_or_else(|p| p.into_inner()));
        for service in retired {
            service.stop();
            let worker = service
                .worker
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .take();
            if let Some(worker) = worker
                && worker.join().is_err()
            {
                tracing::error!("Android device tracker panicked during retirement");
            }
        }
        if !self.is_active()
            && let Err(error) = self.servers.release().finish_before_restart()
        {
            self.control.cancel();
            tracing::error!(%error, "Cannot confirm owned ADB server exit; discovery stopped");
        }
    }

    /// Background-only; the UI captures a discovery ticket before submission.
    pub fn ensure(&self, client: &AdbClient) -> AdbResult<()> {
        self.control.check()?;
        let control = self.discovery_control();
        if control.is_cancelled() {
            return Ok(());
        }
        self.ensure_with_control(client, &control)
    }

    pub fn ensure_with_control(
        &self,
        client: &AdbClient,
        control: &AdbOperationControl,
    ) -> AdbResult<()> {
        control.check()?;
        let _gate = self.lock_lifecycle(control)?;
        self.clean_retired();
        self.control.check()?;
        control.check()?;
        if self.is_active() {
            return Ok(());
        }
        let client = client.clone().with_server_manager(self.servers.clone());
        let lifecycle_control = self.discovery_control();
        let result = DeviceService::start_with_controls(
            client.clone(),
            control.linked_to(&lifecycle_control),
            lifecycle_control.clone(),
        );
        let demand = self.demand.lock().unwrap_or_else(|p| p.into_inner());
        if let Err(error) = control
            .check()
            .and_then(|_| self.control.check())
            .and_then(|_| lifecycle_control.check())
        {
            if let Ok(service) = result {
                self.retired
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(Arc::new(service));
            }
            drop(demand);
            self.clean_retired();
            return Err(error);
        }
        match result {
            Ok(service) => {
                *self.tracker.lock().unwrap_or_else(|p| p.into_inner()) =
                    Some((client, Arc::new(service)));
                Ok(())
            }
            Err(error) => {
                drop(demand);
                self.clean_retired();
                Err(error)
            }
        }
    }
    fn lock_lifecycle(&self, control: &AdbOperationControl) -> AdbResult<MutexGuard<'_, ()>> {
        loop {
            self.control.check()?;
            control.check()?;
            match self.lifecycle.try_lock() {
                Ok(guard) => return Ok(guard),
                Err(TryLockError::Poisoned(error)) => return Ok(error.into_inner()),
                // Poll only cancellation while waiting for the serialization
                // gate. Time never authorizes startup or completes retirement.
                Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
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
        if snapshot.tracker_generation != expected.tracker_generation
            || snapshot.server_generation != expected.server_generation
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
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|(_, service)| service.snapshot())
            .unwrap_or_default()
    }

    pub fn snapshot_if_changed(&self, previous: (u64, u64)) -> Option<DeviceSnapshot> {
        let tracker = self.tracker.lock().unwrap_or_else(|p| p.into_inner());
        let (_, service) = tracker.as_ref()?;
        let state = service.state.lock().unwrap_or_else(|p| p.into_inner());
        ((state.tracker_generation, state.revision) != previous).then(|| state.clone())
    }

    /// Cancellation and ownership transfer only; finish on a background worker.
    pub fn begin_shutdown(&self) -> DeviceServiceShutdown {
        self.control.cancel();
        let _demand = self.demand.lock().unwrap_or_else(|p| p.into_inner());
        self.retire_tracker();
        self.schedule_cleanup();
        DeviceServiceShutdown {
            owner: self.clone(),
        }
    }
}

pub struct DeviceTrackerLease {
    devices: SharedDeviceService,
}
impl std::fmt::Debug for DeviceTrackerLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceTrackerLease").finish_non_exhaustive()
    }
}
impl Drop for DeviceTrackerLease {
    fn drop(&mut self) {
        self.devices.drop_lease();
    }
}

#[must_use = "finish discovery shutdown on a background worker"]
pub struct DeviceServiceShutdown {
    owner: SharedDeviceService,
}
impl DeviceServiceShutdown {
    pub fn finish(self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let workers =
            std::mem::take(&mut *self.owner.cleanup.lock().unwrap_or_else(|p| p.into_inner()));
        let mut stopped = true;
        for worker in workers {
            if wait_worker(&worker, deadline) {
                stopped &= worker.join().is_ok();
            } else {
                // Keep the handle for the next shutdown wait. The worker also
                // retains all resources until cancellation cleanup completes.
                self.owner
                    .cleanup
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(worker);
                stopped = false;
            }
        }
        stopped
            && !self.owner.cleanup_running.load(Ordering::Acquire)
            && self
                .owner
                .retired
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_empty()
    }
}

fn wait_worker(worker: &JoinHandle<()>, deadline: Instant) -> bool {
    while !worker.is_finished() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(
            Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    true
}

impl DeviceService {
    /// Startup belongs on a background job: probing/starting ADB may block.
    pub fn start(client: AdbClient) -> AdbResult<Self> {
        Self::start_with_control(client, AdbOperationControl::default())
    }

    fn start_with_control(client: AdbClient, control: AdbOperationControl) -> AdbResult<Self> {
        Self::start_with_controls(client, control.clone(), control)
    }

    fn start_with_controls(
        client: AdbClient,
        startup: AdbOperationControl,
        control: AdbOperationControl,
    ) -> AdbResult<Self> {
        client.ensure_server_with_control(&startup)?;
        startup.check()?;
        control.check()?;
        static NEXT_TRACKER: AtomicU64 = AtomicU64::new(1);
        let state = Arc::new(Mutex::new(DeviceSnapshot {
            tracker_generation: NEXT_TRACKER.fetch_add(1, Ordering::Relaxed),
            ..DeviceSnapshot::default()
        }));
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
