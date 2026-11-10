//! One application-wide tracker; view channels do not own discovery workers.
use crate::adb::client::{AdbClient, AdbDevice, parse_devices};
use crate::error::{AdbError, AdbResult};
use std::io::Read;
use std::net::{Shutdown, TcpStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

#[derive(Clone, Default)]
pub struct DeviceSnapshot {
    pub devices: Vec<AdbDevice>,
    pub error: Option<String>,
    pub revision: u64,
}

pub struct DeviceService {
    stopped: Arc<AtomicBool>,
    state: Arc<Mutex<DeviceSnapshot>>,
    channel: Arc<Mutex<Option<TcpStream>>>,
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
                            state.devices = devices;
                            state.error = None;
                            state.revision = state.revision.wrapping_add(1);
                        }
                    }
                })();
                if worker_stop.load(Ordering::Acquire) {
                    break;
                }
                let mut state = worker_state.lock().unwrap_or_else(|p| p.into_inner());
                state.devices.clear();
                state.error = result.err().map(|e| e.to_string());
                state.revision = state.revision.wrapping_add(1);
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
        })
    }
    pub fn snapshot(&self) -> DeviceSnapshot {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
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
