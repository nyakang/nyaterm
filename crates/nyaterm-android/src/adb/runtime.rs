//! Exclusive reusable Sync channels and capability cache for one immutable binding.
use crate::adb::client::{DeviceBinding, device_features_with_cancel};
use crate::adb::sync::SyncStream;
use crate::error::{AdbError, AdbResult};
use crate::operation::AdbOperationControl;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Default)]
struct Channels {
    count: usize,
    idle: Vec<SyncStream>,
}
pub struct DeviceRuntime {
    pub binding: DeviceBinding,
    pub control: AdbOperationControl,
    features: Mutex<Option<Vec<String>>>,
    channels: Mutex<Channels>,
}
impl std::fmt::Debug for DeviceRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceRuntime")
            .field("valid", &self.is_valid())
            .finish_non_exhaustive()
    }
}
impl DeviceRuntime {
    pub fn new(binding: DeviceBinding) -> Self {
        Self {
            binding,
            control: AdbOperationControl::default(),
            features: Mutex::new(None),
            channels: Mutex::default(),
        }
    }
    pub fn is_valid(&self) -> bool {
        !self.control.is_cancelled()
    }
    pub fn invalidate(&self) {
        self.control.cancel();
        let mut channels = self.channels.lock().unwrap_or_else(|p| p.into_inner());
        channels.count -= channels.idle.len();
        channels.idle.clear();
    }
    pub fn check(&self) -> AdbResult<()> {
        if !self.is_valid() {
            return Err(AdbError::Offline);
        }
        Ok(())
    }
    fn features(&self, check: &mut impl FnMut() -> AdbResult<()>) -> AdbResult<Vec<String>> {
        let mut features = loop {
            self.check()?;
            check()?;
            match self.features.try_lock() {
                Ok(features) => break features,
                Err(std::sync::TryLockError::Poisoned(error)) => break error.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
        };
        if features.is_none() {
            *features = Some(device_features_with_cancel(&self.binding, || {
                self.check()?;
                check()
            })?);
        }
        self.check()?;
        Ok(features.as_ref().expect("loaded features").clone())
    }
    pub fn with_sync<T>(
        &self,
        mut check: impl FnMut() -> AdbResult<()>,
        run: impl FnOnce(&mut SyncStream) -> AdbResult<T>,
    ) -> AdbResult<T> {
        let channel = loop {
            self.check()?;
            check()?;
            let mut channels = self.channels.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(stream) = channels.idle.pop() {
                break Some(stream);
            }
            if channels.count < 2 {
                channels.count += 1;
                break None;
            }
            drop(channels);
            std::thread::sleep(Duration::from_millis(25));
        };
        let opened = match channel {
            Some(channel) => Ok(channel),
            None => self.features(&mut check).and_then(|features| {
                SyncStream::open_with_features_controlled(&self.binding, &features, || {
                    self.check()?;
                    check()
                })
            }),
        };
        let (result, reusable) = match opened {
            Ok(mut stream) => {
                let result = self
                    .check()
                    .and_then(|()| check())
                    .and_then(|()| run(&mut stream));
                // Only a successful operation proves that its whole response was
                // consumed. Even FileMissing during listing can leave later entries.
                let reusable = result.is_ok().then_some(stream);
                (result, reusable)
            }
            Err(error) => (Err(error), None),
        };
        let mut channels = self.channels.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(stream) = reusable.filter(|_| self.is_valid()) {
            channels.idle.push(stream);
        } else {
            channels.count -= 1;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use crate::adb::client::DeviceBinding;
    use crate::adb::runtime::DeviceRuntime;
    use crate::error::AdbError;
    use crate::operation::AdbOperationControl;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn cancelling_a_waiter_does_not_wait_for_a_paused_capability_probe() {
        let runtime = Arc::new(DeviceRuntime::new(DeviceBinding {
            server: "127.0.0.1:1".parse().unwrap(),
            serial: "fixture".into(),
            transport_id: None,
            generation: 1,
        }));
        let probe = runtime.features.lock().unwrap();
        let control = AdbOperationControl::default();
        let worker_control = control.clone();
        let worker_runtime = runtime.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = worker_runtime.features(&mut || worker_control.check());
            sender
                .send(matches!(result, Err(AdbError::Cancelled)))
                .unwrap();
        });
        control.cancel();
        let result = receiver.recv_timeout(Duration::from_secs(1));
        drop(probe);
        worker.join().unwrap();
        assert!(result.unwrap());
        assert!(runtime.is_valid());
    }
}
