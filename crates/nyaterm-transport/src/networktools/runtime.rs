//! Blocking service ownership. GPUI sends commands and consumes snapshots;
//! it never binds sockets, waits for threads, or locks a live registry.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::networktools::event::NetworkToolEvent;
use crate::networktools::registry::{NetworkToolKind, NetworkToolRegistry};
use crate::networktools::status::{NetworkToolStatus, TransferSnapshot};
use nyaterm_core::models::networktools::NetworkToolsSettings;

pub enum RuntimeCommand {
    Start(NetworkToolKind, NetworkToolsSettings),
    Stop(NetworkToolKind),
    Restart(NetworkToolKind, NetworkToolsSettings),
}

impl RuntimeCommand {
    fn kind(&self) -> NetworkToolKind {
        match self {
            Self::Start(kind, _) | Self::Stop(kind) | Self::Restart(kind, _) => *kind,
        }
    }
}

pub struct RuntimeResult {
    pub kind: NetworkToolKind,
    pub result: Result<(), String>,
}

pub struct ServiceSnapshot {
    pub generation: u64,
    pub status: NetworkToolStatus,
    pub transfers: Vec<TransferSnapshot>,
}

#[derive(Default)]
pub struct RuntimeSnapshot {
    pub services: HashMap<NetworkToolKind, ServiceSnapshot>,
}

pub struct NetworkToolsRuntime {
    commands: Option<SyncSender<RuntimeCommand>>,
    results: Option<Receiver<RuntimeResult>>,
    snapshot: Arc<Mutex<Option<RuntimeSnapshot>>>,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl NetworkToolsRuntime {
    pub fn new(events: SyncSender<NetworkToolEvent>) -> io::Result<Self> {
        let (commands, requests) = mpsc::sync_channel(1);
        let (responses, results) = mpsc::sync_channel(1);
        let snapshot = Arc::new(Mutex::new(None));
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_snapshot = Arc::clone(&snapshot);
        let worker_shutdown = Arc::clone(&shutdown);
        let thread = std::thread::Builder::new()
            .name("nyaterm-networktools-runtime".into())
            .spawn(move || {
                let mut registry = NetworkToolRegistry::new(events);
                while !worker_shutdown.load(Ordering::Relaxed) {
                    let request = match requests.recv_timeout(Duration::from_millis(200)) {
                        Ok(request) => Some(request),
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    if worker_shutdown.load(Ordering::Relaxed) {
                        break;
                    }
                    let result = request.map(|request: RuntimeCommand| {
                        let kind = request.kind();
                        let result = match request {
                            RuntimeCommand::Start(kind, settings) => {
                                registry.start(kind, &settings)
                            }
                            RuntimeCommand::Stop(kind) => {
                                registry.stop(kind);
                                Ok(())
                            }
                            RuntimeCommand::Restart(kind, settings) => {
                                registry.stop(kind);
                                registry.start(kind, &settings)
                            }
                        }
                        .map_err(|error| error.to_string());
                        RuntimeResult { kind, result }
                    });
                    let mut snapshot = RuntimeSnapshot::default();
                    for kind in NetworkToolKind::ALL {
                        snapshot.services.insert(
                            kind,
                            ServiceSnapshot {
                                generation: registry.generation(kind).unwrap_or(0),
                                status: registry.status(kind),
                                transfers: registry.transfers(kind),
                            },
                        );
                    }
                    *worker_snapshot
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(snapshot);
                    if let Some(result) = result
                        && responses.send(result).is_err()
                    {
                        break;
                    }
                }
                // The owner drops on this worker, joining listeners and all
                // sessions. Receivers are disconnected before shutdown joins.
            })?;
        Ok(Self {
            commands: Some(commands),
            results: Some(results),
            snapshot,
            shutdown,
            thread: Some(thread),
        })
    }

    pub fn submit(&self, command: RuntimeCommand) -> Result<(), String> {
        self.commands
            .as_ref()
            .ok_or("network tools are shutting down")?
            .try_send(command)
            .map_err(|_| "network tools are busy or unavailable".to_string())
    }

    pub fn take_snapshot(&self) -> Option<RuntimeSnapshot> {
        // Even the short snapshot handoff must never wait inside a GPUI update.
        self.snapshot.try_lock().ok()?.take()
    }

    pub fn take_results(&self) -> Vec<RuntimeResult> {
        self.results
            .as_ref()
            .map(|results| results.try_iter().take(16).collect())
            .unwrap_or_default()
    }

    pub fn take_shutdown(&mut self) -> Option<JoinHandle<()>> {
        self.shutdown.store(true, Ordering::Relaxed);
        self.commands.take();
        self.results.take();
        self.thread.take()
    }
}

impl Drop for NetworkToolsRuntime {
    fn drop(&mut self) {
        // Normal application shutdown joins through take_shutdown off the UI
        // thread. The fallback still asks the owned worker to clean up.
        self.take_shutdown();
    }
}

#[cfg(test)]
mod tests {
    use std::net::UdpSocket;
    use std::sync::mpsc::sync_channel;
    use std::time::{Duration, Instant};

    use crate::networktools::event::NetworkToolEvent;
    use crate::networktools::registry::NetworkToolKind;
    use nyaterm_core::models::networktools::{NetworkToolsSettings, TftpSettings};
    use nyaterm_core::test_support::TestTempDir;

    use super::{NetworkToolsRuntime, RuntimeCommand, RuntimeResult};

    fn settings(root: &std::path::Path) -> NetworkToolsSettings {
        std::fs::create_dir_all(root).unwrap();
        let port = UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        NetworkToolsSettings {
            tftp: TftpSettings {
                bind_address: "127.0.0.1".into(),
                port,
                root_dir: root.display().to_string(),
                allow_upload: true,
                timeout_secs: 255,
                ..TftpSettings::default()
            },
        }
    }

    fn result(runtime: &NetworkToolsRuntime) -> RuntimeResult {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(result) = runtime.take_results().into_iter().next() {
                return result;
            }
            assert!(Instant::now() < deadline, "runtime operation timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn restart_cancels_old_transfers_and_rebinds_with_a_new_generation() {
        let dir = TestTempDir::new("nyaterm-networktools-restart");
        let settings = settings(dir.path());
        let addr = format!("127.0.0.1:{}", settings.tftp.port);
        let (sender, events) = sync_channel(16);
        let mut runtime = NetworkToolsRuntime::new(sender).unwrap();
        let kind = NetworkToolKind::Tftp;
        runtime
            .submit(RuntimeCommand::Start(kind, settings.clone()))
            .unwrap();
        result(&runtime).result.unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        client.send_to(b"\0\x02old.bin\0octet\0", &addr).unwrap();
        let mut buffer = [0u8; 512];
        let (size, old_tid) = client.recv_from(&mut buffer).unwrap();
        assert_eq!(&buffer[..size], b"\0\x04\0\0");
        let before = Instant::now();
        runtime
            .submit(RuntimeCommand::Restart(kind, settings))
            .unwrap();
        result(&runtime).result.unwrap();
        assert!(before.elapsed() < Duration::from_secs(2));
        client
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        client.send_to(b"\0\x03\0\x01late", old_tid).unwrap();
        assert!(client.recv_from(&mut buffer).is_err());
        assert!(!dir.join("old.bin").exists());
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        client.send_to(b"\0\x02new.bin\0octet\0", &addr).unwrap();
        let (size, new_tid) = client.recv_from(&mut buffer).unwrap();
        assert_eq!(&buffer[..size], b"\0\x04\0\0");
        client.send_to(b"\0\x03\0\x01new", new_tid).unwrap();
        let (size, _) = client.recv_from(&mut buffer).unwrap();
        assert_eq!(&buffer[..size], b"\0\x04\0\x01");
        assert_eq!(std::fs::read(dir.join("new.bin")).unwrap(), b"new");
        runtime.submit(RuntimeCommand::Stop(kind)).unwrap();
        result(&runtime).result.unwrap();
        let collected = events.try_iter().collect::<Vec<_>>();
        let generations = collected
            .iter()
            .filter_map(|event| match event {
                NetworkToolEvent::Started { generation, .. } => Some(*generation),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(generations.len(), 2);
        assert!(generations[1] > generations[0]);
        let ids = collected
            .iter()
            .filter_map(|event| match event {
                NetworkToolEvent::TransferStarted { id, .. } => Some(*id),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(ids.len(), 2);
        assert!(ids[1] > ids[0]);
        let finished = collected
            .iter()
            .position(|event| matches!(event, NetworkToolEvent::TransferFinished { .. }))
            .unwrap();
        let restarted = collected.iter().position(|event| matches!(event, NetworkToolEvent::Started { generation, .. } if *generation == generations[1])).unwrap();
        assert!(finished < restarted);
        assert!(UdpSocket::bind(&addr).is_ok());
        drop(events);
        runtime.take_shutdown().unwrap().join().unwrap();
    }

    #[test]
    fn disconnecting_the_ui_releases_full_lifecycle_sends_during_shutdown() {
        let dir = TestTempDir::new("nyaterm-networktools-full-shutdown");
        let settings = settings(dir.path());
        let addr = format!("127.0.0.1:{}", settings.tftp.port);
        let (sender, events) = sync_channel(1);
        let mut runtime = NetworkToolsRuntime::new(sender).unwrap();
        runtime
            .submit(RuntimeCommand::Start(NetworkToolKind::Tftp, settings))
            .unwrap();
        result(&runtime).result.unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client
            .send_to(b"\0\x02blocked.bin\0octet\0", &addr)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let registered = runtime.take_snapshot().is_some_and(|snapshot| {
                snapshot
                    .services
                    .values()
                    .any(|service| !service.transfers.is_empty())
            });
            if registered {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        runtime
            .submit(RuntimeCommand::Stop(NetworkToolKind::Tftp))
            .unwrap();
        drop(events);
        let before = Instant::now();
        runtime.take_shutdown().unwrap().join().unwrap();
        assert!(before.elapsed() < Duration::from_secs(2));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        assert!(UdpSocket::bind(addr).is_ok());
    }
}
