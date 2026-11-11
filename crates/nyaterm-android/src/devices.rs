//! One application-wide tracker; view channels do not own discovery workers.
use crate::adb::client::{AdbClient, AdbDevice, parse_devices};
use crate::adb::runtime::DeviceRuntime;
use crate::error::{AdbError, AdbResult};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::net::{Shutdown, TcpStream};
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

#[derive(Clone, Default)]
pub struct DeviceSnapshot {
    pub devices: Vec<AdbDevice>,
    pub error: Option<String>,
    pub revision: u64,
    pub server_generation: u64,
    pub device_generations: HashMap<String, u64>,
}

pub struct DeviceService {
    stopped: Arc<AtomicBool>,
    state: Arc<Mutex<DeviceSnapshot>>,
    channel: Arc<Mutex<Option<TcpStream>>>,
    runtimes: Arc<Mutex<Vec<Weak<DeviceRuntime>>>>,
}
type SharedTracker = Arc<Mutex<Option<(AdbClient, Arc<DeviceService>)>>>;
#[derive(Clone, Default)]
pub struct SharedDeviceService(SharedTracker);
impl SharedDeviceService {
    pub fn ensure(&self, client: &AdbClient) -> AdbResult<()> {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if state.as_ref().is_some_and(|(existing, _)| {
            existing.executable == client.executable && existing.server == client.server
        }) {
            return Ok(());
        }
        *state = Some((
            client.clone(),
            Arc::new(DeviceService::start(client.clone())?),
        ));
        Ok(())
    }
    pub fn register_runtime_since(
        &self,
        runtime: &Arc<DeviceRuntime>,
        expected: &DeviceSnapshot,
    ) -> AdbResult<()> {
        let shared = self.0.lock().unwrap_or_else(|p| p.into_inner());
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
    pub fn snapshot(&self) -> DeviceSnapshot {
        self.0
            .try_lock()
            .ok()
            .as_deref()
            .and_then(|state| state.as_ref())
            .map(|(_, service)| service.snapshot())
            .unwrap_or_default()
    }
}
impl DeviceService {
    /// Startup belongs on a background job: probing/starting ADB may block.
    pub fn start(client: AdbClient) -> AdbResult<Self> {
        client.ensure_server()?;
        let stopped = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(DeviceSnapshot::default()));
        let channel = Arc::new(Mutex::new(None));
        let worker_stop = stopped.clone();
        let worker_state = state.clone();
        let worker_channel = channel.clone();
        let runtimes = Arc::new(Mutex::new(Vec::new()));
        let worker_runtimes = runtimes.clone();
        nyaterm_core::runtime::worker::spawn("android-device-tracker".into(), move || {
            while !worker_stop.load(Ordering::Acquire) {
                let result = (|| {
                    let mut stream = client.tracker()?;
                    *worker_channel.lock().unwrap_or_else(|p| p.into_inner()) =
                        Some(stream.try_clone()?);
                    let mut pending = Vec::new();
                    let mut buffer = [0; 8192];
                    loop {
                        if worker_stop.load(Ordering::Acquire) {
                            return Ok(());
                        }
                        match stream.read(&mut buffer) {
                            Ok(0) => return Err(AdbError::Offline),
                            Ok(count) => pending.extend_from_slice(&buffer[..count]),
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
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
                            let mut state = worker_state.lock().unwrap_or_else(|p| p.into_inner());
                            update_snapshot(&mut state, devices);
                            invalidate_changed(&worker_runtimes, &state.devices);
                        }
                    }
                })();
                if worker_stop.load(Ordering::Acquire) {
                    break;
                }
                let mut state = worker_state.lock().unwrap_or_else(|p| p.into_inner());
                update_snapshot(&mut state, Vec::new());
                state.server_generation = state.server_generation.wrapping_add(1);
                state.error = result.err().map(|e| e.to_string());
                invalidate_changed(&worker_runtimes, &[]);
                drop(state);
                for _ in 0..10 {
                    if worker_stop.load(Ordering::Acquire) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        })?;
        Ok(Self {
            stopped,
            state,
            channel,
            runtimes,
        })
    }
    pub fn snapshot(&self) -> DeviceSnapshot {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
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
