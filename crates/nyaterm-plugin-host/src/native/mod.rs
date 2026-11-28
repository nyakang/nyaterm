//! Process-owned native component registry. Blocking I/O stays on its bounded worker.
mod catalog;
mod package;
mod registry;
use nyaterm_core::plugins::management::OperationPhase;
use nyaterm_core::plugins::native::catalog::InstalledComponent;
use registry::Registry;
use sha2::{Digest, Sha256};

use crate::package::{StagingDirectory, io_error, reject_path_links};
use futures::channel::oneshot;
use nyaterm_core::plugins::native::{
    MAX_ARCHIVE_BYTES, NativeComponentId, NativeLaunch, NativeSnapshot, NativeStatus,
};
use nyaterm_core::plugins::{ErrorCode, PluginError, PluginResult};
use nyaterm_core::runtime::AppRuntime;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex, RwLock,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone)]
pub enum NativeAction {
    Install,
    Import(PathBuf),
    Enable(bool),
    Uninstall,
    Probe,
    Rollback,
}
pub type NativeTask = oneshot::Receiver<PluginResult<()>>;
type Replies = BTreeMap<NativeComponentId, Vec<oneshot::Sender<PluginResult<()>>>>;
enum Request {
    Change(
        NativeComponentId,
        NativeAction,
        oneshot::Sender<PluginResult<()>>,
    ),
    Acquire(
        NativeComponentId,
        oneshot::Sender<PluginResult<NativeLaunch>>,
    ),
    Released,
    Stop,
}

pub struct NativeService {
    changed: std::sync::Weak<dyn Fn() + Send + Sync>,
    sender: mpsc::SyncSender<Request>,
    snapshots: Arc<RwLock<Vec<NativeSnapshot>>>,
    cancel: Arc<Mutex<BTreeMap<NativeComponentId, Arc<AtomicBool>>>>,
    installing: Arc<Mutex<Replies>>,
    stop: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

struct Lease {
    _file: Option<File>,
    sender: mpsc::SyncSender<Request>,
}
impl Drop for Lease {
    fn drop(&mut self) {
        drop(self._file.take());
        let _ = self.sender.try_send(Request::Released);
    }
}

impl NativeService {
    pub fn start(
        runtime: &AppRuntime,
        changed: Arc<dyn Fn() + Send + Sync>,
    ) -> PluginResult<Arc<Self>> {
        let (sender, receiver) = mpsc::sync_channel(16);
        let snapshots = Arc::new(RwLock::new(
            NativeComponentId::ALL
                .into_iter()
                .map(|id| NativeSnapshot {
                    id,
                    enabled: true,
                    in_use: false,
                    install_path: None,
                    phase: None,
                    installed_version: None,
                    available_version: None,
                    rollback_available: false,
                    status: NativeStatus::Missing,
                    downloaded: 0,
                    download_size: None,
                    error: None,
                })
                .collect::<Vec<_>>(),
        ));
        let service = Arc::new(Self {
            changed: Arc::downgrade(&changed),
            sender: sender.clone(),
            snapshots: snapshots.clone(),
            cancel: Arc::new(Mutex::new(BTreeMap::new())),
            installing: Arc::new(Mutex::new(BTreeMap::new())),
            stop: Arc::new(AtomicBool::new(false)),
            worker: Mutex::new(None),
        });
        let stop = service.stop.clone();
        let cancel = service.cancel.clone();
        let installing = service.installing.clone();
        let root = runtime.data_dir().join("plugins/native");
        let worker = std::thread::Builder::new()
            .name("native-components".into())
            .spawn(move || {
                let mut registry = Registry::open(root);
                if let Ok(registry) = &mut registry {
                    registry.reap();
                }
                refresh(&registry, &snapshots, &changed);
                while let Ok(request) = receiver.recv() {
                    match request {
                        Request::Stop => break,
                        Request::Released => {
                            if let Ok(registry) = &mut registry {
                                registry.reap();
                            }
                            refresh(&registry, &snapshots, &changed);
                        }
                        Request::Acquire(id, reply) => {
                            let result = registry
                                .as_mut()
                                .map_err(|e| e.clone())
                                .and_then(|r| r.acquire(id, sender.clone()));
                            refresh(&registry, &snapshots, &changed);
                            let _ = reply.send(result);
                        }
                        Request::Change(id, action, reply) => {
                            update(&snapshots, id, &changed, |s| s.error = None);
                            let installation =
                                matches!(&action, NativeAction::Install | NativeAction::Import(_));
                            let cancellation =
                                cancel.lock().unwrap().entry(id).or_default().clone();
                            let check = || {
                                if stop.load(Ordering::Acquire)
                                    || cancellation.load(Ordering::Acquire)
                                {
                                    Err(PluginError::new(
                                        ErrorCode::Cancelled,
                                        "Component installation cancelled",
                                    ))
                                } else {
                                    Ok(())
                                }
                            };
                            let result =
                                registry
                                    .as_mut()
                                    .map_err(|e| e.clone())
                                    .and_then(|registry| {
                                        check()?;
                                        change(registry, id, action, &check, &snapshots, &changed)
                                    });
                            if installation {
                                cancel.lock().unwrap().remove(&id);
                            }
                            if let Ok(registry) = &mut registry {
                                registry.reap();
                            }
                            update(&snapshots, id, &changed, |s| {
                                s.phase = None;
                                s.error = result
                                    .as_ref()
                                    .err()
                                    .filter(|error| error.code != ErrorCode::Cancelled)
                                    .cloned();
                            });
                            refresh(&registry, &snapshots, &changed);
                            let _ = reply.send(result.clone());
                            if installation
                                && let Some(waiters) = installing.lock().unwrap().remove(&id)
                            {
                                for waiter in waiters {
                                    let _ = waiter.send(result.clone());
                                }
                            }
                        }
                    }
                }
            })
            .map_err(|_| io_error())?;
        *service.worker.lock().unwrap() = Some(worker);
        Ok(service)
    }
    fn publish(&self, id: NativeComponentId, change: impl FnOnce(&mut NativeSnapshot)) {
        let mut snapshots = self.snapshots.write().unwrap();
        change(
            snapshots
                .iter_mut()
                .find(|snapshot| snapshot.id == id)
                .unwrap(),
        );
        drop(snapshots);
        // The worker owns the event sender; a retained service must not keep the
        // event stream open after shutdown.
        if let Some(changed) = self.changed.upgrade() {
            changed();
        }
    }

    pub fn snapshot(&self) -> Vec<NativeSnapshot> {
        self.snapshots.read().unwrap().clone()
    }
    pub fn submit(&self, id: NativeComponentId, action: NativeAction) -> PluginResult<NativeTask> {
        if self.stop.load(Ordering::Acquire) {
            return Err(PluginError::new(
                ErrorCode::Shutdown,
                "Component service stopped",
            ));
        }
        let (reply, task) = oneshot::channel();
        let mut installing = self.installing.lock().unwrap();
        let installation = matches!(action, NativeAction::Install | NativeAction::Import(_));
        if installing.contains_key(&id) {
            if matches!(action, NativeAction::Install) {
                installing.get_mut(&id).unwrap().push(reply);
                return Ok(task);
            }
            if !matches!(action, NativeAction::Probe) {
                return Err(PluginError::new(
                    ErrorCode::Conflict,
                    "Wait for the active component installation",
                ));
            }
        }
        if installation {
            installing.insert(id, Vec::new());
            self.cancel
                .lock()
                .unwrap()
                .insert(id, Arc::new(AtomicBool::new(false)));
        }
        if installation {
            self.publish(id, |s| {
                s.phase = Some(OperationPhase::Queued);
                s.error = None;
                s.downloaded = 0;
            });
        }
        if self
            .sender
            .try_send(Request::Change(id, action, reply))
            .is_err()
        {
            if installation {
                installing.remove(&id);
                self.cancel.lock().unwrap().remove(&id);
                self.publish(id, |s| s.phase = None);
            }
            return Err(PluginError::new(
                ErrorCode::QueueFull,
                "Component operation queue is full",
            ));
        }
        Ok(task)
    }
    pub fn acquire(
        &self,
        id: NativeComponentId,
    ) -> PluginResult<oneshot::Receiver<PluginResult<NativeLaunch>>> {
        let (reply, task) = oneshot::channel();
        self.sender
            .try_send(Request::Acquire(id, reply))
            .map_err(|_| {
                PluginError::new(ErrorCode::QueueFull, "Component operation queue is full")
            })?;
        Ok(task)
    }
    pub fn cancel(&self, id: NativeComponentId) {
        let snapshots = self.snapshots.read().unwrap();
        if !snapshots
            .iter()
            .any(|s| s.id == id && s.phase.is_some_and(OperationPhase::cancellable))
        {
            return;
        }
        self.cancel
            .lock()
            .unwrap()
            .entry(id)
            .or_default()
            .store(true, Ordering::Release);
    }
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = self.sender.send(Request::Stop);
            let _ = worker.join();
        }
    }
}
impl Drop for NativeService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn update(
    snapshots: &RwLock<Vec<NativeSnapshot>>,
    id: NativeComponentId,
    changed: &Arc<dyn Fn() + Send + Sync>,
    f: impl FnOnce(&mut NativeSnapshot),
) {
    {
        let mut snapshots = snapshots.write().unwrap();
        f(snapshots.iter_mut().find(|s| s.id == id).unwrap());
    }
    changed();
}
fn begin_commit(
    snapshots: &RwLock<Vec<NativeSnapshot>>,
    id: NativeComponentId,
    changed: &Arc<dyn Fn() + Send + Sync>,
    cancel: &impl Fn() -> PluginResult<()>,
) -> PluginResult<()> {
    // Serialize cancellation admission with the irreversible filesystem boundary.
    let mut snapshots = snapshots.write().unwrap();
    cancel()?;
    snapshots
        .iter_mut()
        .find(|snapshot| snapshot.id == id)
        .unwrap()
        .phase = Some(OperationPhase::Committing);
    drop(snapshots);
    changed();
    Ok(())
}

fn refresh(
    registry: &PluginResult<Registry>,
    snapshots: &RwLock<Vec<NativeSnapshot>>,
    changed: &Arc<dyn Fn() + Send + Sync>,
) {
    for id in NativeComponentId::ALL {
        let (status, error) = match registry {
            Ok(r) => (r.status(id), None),
            Err(e) => (NativeStatus::Damaged, Some(e.clone())),
        };
        update(snapshots, id, changed, |s| {
            s.status = status;
            if error.is_some() {
                s.error = error;
            }
            if let Ok(registry) = registry
                && let Ok(selection) = registry.selection(id)
            {
                s.enabled = !registry
                    .preferences
                    .components
                    .get(&id)
                    .is_some_and(|p| p.disabled || p.uninstall_pending);
                s.in_use = registry.in_use(id);
                s.install_path = selection
                    .active
                    .as_ref()
                    .map(|v| registry.version_directory(id, v));
                s.installed_version = selection.active.map(|v| v.version);
                s.rollback_available = selection.previous.is_some();
                if status == NativeStatus::Ready
                    && s.installed_version
                        .as_ref()
                        .zip(s.available_version.as_ref())
                        .is_some_and(|(installed, available)| {
                            semver::Version::parse(available).ok()
                                > semver::Version::parse(installed).ok()
                        })
                {
                    s.status = NativeStatus::UpdateAvailable;
                }
            }
        });
    }
}
fn client() -> PluginResult<reqwest::blocking::Client> {
    // Blocking Response::read applies this timeout to each read. The async
    // read_timeout wrapper requires a Tokio reactor on the polling thread,
    // whereas our component worker intentionally uses ordinary blocking I/O.
    reqwest::blocking::Client::builder()
        .user_agent(concat!("nyaterm-app/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| io_error())
}
fn change(
    registry: &mut Registry,
    id: NativeComponentId,
    action: NativeAction,
    cancel: &impl Fn() -> PluginResult<()>,
    snapshots: &RwLock<Vec<NativeSnapshot>>,
    changed: &Arc<dyn Fn() + Send + Sync>,
) -> PluginResult<()> {
    if !id.supported() {
        return Err(PluginError::new(
            nyaterm_core::plugins::ErrorCode::Incompatible,
            "Native component is unavailable on this platform",
        ));
    }
    match action {
        NativeAction::Enable(enabled) => {
            let mut preferences = registry.preferences.clone();
            let pref = preferences.components.entry(id).or_default();
            pref.disabled = !enabled;
            pref.uninstall_pending = false;
            registry.save(preferences)
        }
        NativeAction::Uninstall => {
            let mut preferences = registry.preferences.clone();
            let pref = preferences.components.entry(id).or_default();
            pref.disabled = true;
            pref.uninstall_pending = true;
            registry.save(preferences)?;
            registry.reap();
            Ok(())
        }
        NativeAction::Rollback => {
            let _lock = registry.install_lock(id)?;
            registry.rollback(id)
        }
        NativeAction::Probe => {
            let release = catalog::select(id, cancel)?;
            update(snapshots, id, changed, |s| {
                s.download_size = Some(release.size);
                s.available_version = Some(release.version);
            });
            Ok(())
        }
        NativeAction::Install | NativeAction::Import(_) => {
            update(snapshots, id, changed, |s| {
                s.phase = Some(OperationPhase::Checking)
            });
            let _lock = registry.install_lock(id)?;
            let release = if matches!(action, NativeAction::Install) {
                Some(catalog::select(id, cancel)?)
            } else {
                None
            };
            let mut stage = StagingDirectory::new(&registry.staging_root(id).join("staging"))?;
            let payload = stage.path.join("payload");
            fs::create_dir(&payload).map_err(|_| io_error())?;
            update(snapshots, id, changed, |s| {
                s.status = NativeStatus::Installing;
                s.phase = Some(if release.is_some() {
                    OperationPhase::Downloading
                } else {
                    OperationPhase::Verifying
                });
                s.downloaded = 0;
                s.error = None;
            });
            let source = match action {
                NativeAction::Import(source) => source,
                _ => {
                    let mut response = client()?
                        .get(&release.as_ref().ok_or_else(io_error)?.url)
                        .send()
                        .and_then(|r| r.error_for_status())
                        .map_err(|_| catalog::network_error())?;
                    if response
                        .content_length()
                        .is_some_and(|n| n > MAX_ARCHIVE_BYTES)
                    {
                        return Err(io_error());
                    }
                    let size = response.content_length();
                    let source = stage.path.join("download.zip");
                    let mut file = File::create(&source).map_err(|_| io_error())?;
                    let mut buffer = [0; 65536];
                    let mut total = 0u64;
                    loop {
                        cancel()?;
                        let n = response
                            .read(&mut buffer)
                            .map_err(|_| catalog::network_error())?;
                        if n == 0 {
                            break;
                        }
                        total += n as u64;
                        if total > MAX_ARCHIVE_BYTES {
                            return Err(io_error());
                        }
                        file.write_all(&buffer[..n]).map_err(|_| io_error())?;
                        update(snapshots, id, changed, |s| {
                            s.downloaded = total;
                            s.download_size = size;
                        });
                    }
                    file.sync_all().map_err(|_| io_error())?;
                    source
                }
            };
            update(snapshots, id, changed, |s| {
                s.phase = Some(OperationPhase::Verifying)
            });
            if let Some(release) = &release {
                let mut file = File::open(&source).map_err(|_| io_error())?;
                if file.metadata().map_err(|_| io_error())?.len() != release.size {
                    return Err(io_error());
                }
                let mut hash = Sha256::new();
                let mut buffer = [0; 65536];
                loop {
                    cancel()?;
                    let count = file.read(&mut buffer).map_err(|_| io_error())?;
                    if count == 0 {
                        break;
                    }
                    hash.update(&buffer[..count]);
                }
                if hex::encode(hash.finalize()) != release.sha256 {
                    return Err(PluginError::new(
                        ErrorCode::InvalidManifest,
                        "Native archive digest does not match the signed index",
                    ));
                }
            }
            update(snapshots, id, changed, |s| {
                s.phase = Some(OperationPhase::Installing)
            });
            package::extract(&source, &payload, id, cancel)?;
            let manifest = package::verify(&payload, id, cancel)?;
            if release.as_ref().is_some_and(|r| {
                r.version != manifest.component_version
                    || r.host_version != manifest.host_version
                    || r.ipc_version != manifest.ipc_version
            }) {
                return Err(PluginError::new(
                    ErrorCode::InvalidManifest,
                    "Native package does not match the signed index",
                ));
            }
            let installed = InstalledComponent {
                version: manifest.component_version,
                digest: package::manifest_digest(&payload)?,
            };
            installed.validate()?;
            cancel()?;
            let target = registry.version_directory(id, &installed);
            let parent = target.parent().ok_or_else(io_error)?;
            fs::create_dir_all(parent).map_err(|_| io_error())?;
            reject_path_links(&registry.root, parent)?;
            // Published versions are immutable, including offline imports.
            for entry in fs::read_dir(parent).map_err(|_| io_error())? {
                let path = entry.map_err(|_| io_error())?.path();
                reject_path_links(&registry.root, &path)?;
                if path != target
                    && path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                        n.rsplit_once('-')
                            .is_some_and(|(version, _)| version == installed.version)
                    })
                {
                    return Err(PluginError::new(
                        ErrorCode::Conflict,
                        "This native component version already has different content",
                    ));
                }
            }
            let valid_existing = target.exists()
                && package::verify(&target, id, cancel).is_ok()
                && package::manifest_digest(&target)? == installed.digest;
            begin_commit(snapshots, id, changed, cancel)?;
            if !valid_existing {
                let _version_lock = registry.lock_version(id, &installed, false)?;
                let old = stage.path.join("old");
                if target.exists() {
                    let journal = serde_json::to_vec(&(id, &installed)).map_err(|_| io_error())?;
                    let mut file = File::create(stage.path.join("transaction.json"))
                        .map_err(|_| io_error())?;
                    file.write_all(&journal).map_err(|_| io_error())?;
                    file.sync_all().map_err(|_| io_error())?;
                    fs::rename(&target, &old).map_err(|_| io_error())?;
                }
                if fs::rename(&payload, &target).is_err() {
                    if old.exists() && fs::rename(&old, &target).is_err() {
                        stage.retain();
                    }
                    return Err(io_error());
                }
            }
            // Files become immutable before the database atomically selects them.
            // A failed transaction leaves the previous version selected.
            registry.activate(id, installed)?;

            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use nyaterm_core::plugins::native::{NativeComponentId, NativeSnapshot, NativeStatus};

    #[test]
    fn component_http_body_reads_without_a_caller_tokio_runtime() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::time::Duration;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                assert!(request.len() < 8192);
            }
            assert!(
                String::from_utf8(request)
                    .unwrap()
                    .to_ascii_lowercase()
                    .contains("user-agent: nyaterm-app/")
            );
            stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n4\r\ntest\r\n0\r\n\r\n").unwrap();
        });
        let mut response = super::client()
            .unwrap()
            .get(format!("http://{address}/"))
            .send()
            .unwrap();
        let mut body = Vec::new();
        response.read_to_end(&mut body).unwrap();
        assert_eq!(body, b"test");
        server.join().unwrap();
    }

    fn queued_service() -> (
        super::NativeService,
        std::sync::mpsc::Receiver<super::Request>,
    ) {
        use std::collections::BTreeMap;
        use std::sync::{Arc, Mutex, RwLock, atomic::AtomicBool, mpsc};
        let (sender, receiver) = mpsc::sync_channel(1);
        (
            super::NativeService {
                sender,
                changed: Arc::downgrade(&(Arc::new(|| {}) as Arc<dyn Fn() + Send + Sync>)),
                snapshots: Arc::new(RwLock::new(
                    NativeComponentId::ALL
                        .into_iter()
                        .map(|id| NativeSnapshot {
                            id,
                            enabled: true,
                            in_use: false,
                            install_path: None,
                            phase: None,
                            installed_version: None,
                            available_version: None,
                            rollback_available: false,
                            status: NativeStatus::Missing,
                            downloaded: 0,
                            download_size: None,
                            error: None,
                        })
                        .collect(),
                )),
                cancel: Arc::new(Mutex::new(BTreeMap::new())),
                installing: Arc::new(Mutex::new(BTreeMap::new())),
                stop: Arc::new(AtomicBool::new(false)),
                worker: Mutex::new(None),
            },
            receiver,
        )
    }

    #[test]
    fn import_serializes_changes_and_download_requests_share_its_result() {
        use super::NativeAction;
        use nyaterm_core::plugins::ErrorCode;
        let (service, receiver) = queued_service();
        let id = NativeComponentId::Vnc;
        let _import = service
            .submit(id, NativeAction::Import("component.zip".into()))
            .unwrap();
        let _download = service.submit(id, NativeAction::Install).unwrap();
        assert_eq!(service.installing.lock().unwrap()[&id].len(), 1);
        assert_eq!(
            service
                .submit(id, NativeAction::Uninstall)
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        assert_eq!(
            service.submit(id, NativeAction::Probe).unwrap_err().code,
            ErrorCode::QueueFull
        );
        assert_eq!(service.installing.lock().unwrap()[&id].len(), 1);
        assert!(matches!(
            receiver.try_recv().unwrap(),
            super::Request::Change(_, NativeAction::Import(_), _)
        ));
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn retry_gets_fresh_cancellation_and_queue_failure_releases_reservation() {
        use super::NativeAction;
        use std::sync::atomic::Ordering;
        let (service, receiver) = queued_service();
        let id = NativeComponentId::Vnc;
        service.cancel(id);
        let _probe = service.submit(id, NativeAction::Probe).unwrap();
        assert!(service.submit(id, NativeAction::Install).is_err());
        assert!(!service.installing.lock().unwrap().contains_key(&id));
        let _ = receiver.try_recv().unwrap();
        let _install = service.submit(id, NativeAction::Install).unwrap();
        assert!(!service.cancel.lock().unwrap()[&id].load(Ordering::Acquire));
        service.cancel(id);
        assert!(service.cancel.lock().unwrap()[&id].load(Ordering::Acquire));
    }
    #[test]
    fn accepted_cancellation_prevents_commit_and_commit_rejects_late_cancellation() {
        use nyaterm_core::plugins::management::OperationPhase;
        use nyaterm_core::plugins::{ErrorCode, PluginError};
        use std::sync::atomic::Ordering;
        let (service, _receiver) = queued_service();
        let id = NativeComponentId::Vnc;
        let _task = service.submit(id, super::NativeAction::Install).unwrap();
        let cancelled = service.cancel.lock().unwrap()[&id].clone();
        let check = || {
            if cancelled.load(Ordering::Acquire) {
                Err(PluginError::new(ErrorCode::Cancelled, "Cancelled"))
            } else {
                Ok(())
            }
        };
        service.cancel(id);
        assert_eq!(
            super::begin_commit(
                &service.snapshots,
                id,
                &(std::sync::Arc::new(|| {}) as std::sync::Arc<dyn Fn() + Send + Sync>),
                &check
            )
            .unwrap_err()
            .code,
            ErrorCode::Cancelled
        );
        cancelled.store(false, Ordering::Release);
        super::begin_commit(
            &service.snapshots,
            id,
            &(std::sync::Arc::new(|| {}) as std::sync::Arc<dyn Fn() + Send + Sync>),
            &check,
        )
        .unwrap();
        service.cancel(id);
        assert!(!cancelled.load(Ordering::Acquire));
        assert_eq!(
            service
                .snapshot()
                .into_iter()
                .find(|snapshot| snapshot.id == id)
                .unwrap()
                .phase,
            Some(OperationPhase::Committing)
        );
    }
}
