//! Process-owned native component registry. Blocking I/O stays on its bounded worker.
mod package;

use crate::package::{StagingDirectory, io_error, reject_path_links};
use futures::channel::oneshot;
use nyaterm_core::plugins::native::{
    BUILD_ID, MAX_ARCHIVE_BYTES, NativeComponentId, NativeLaunch, NativePreferences,
    NativeSnapshot, NativeStatus, TARGET,
};
use nyaterm_core::plugins::{ErrorCode, PluginError, PluginResult};
use nyaterm_core::runtime::AppRuntime;
use nyaterm_store::native_component_preferences::NativePreferenceStore;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex, RwLock, Weak,
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

struct Registry {
    root: PathBuf,
    store: NativePreferenceStore,
    preferences: NativePreferences,
    leases: BTreeMap<NativeComponentId, Weak<Lease>>,
}

impl Registry {
    fn open(root: PathBuf) -> PluginResult<Self> {
        fs::create_dir_all(&root).map_err(|_| io_error())?;
        crate::package::reject_link(&root)?;
        fs::create_dir_all(root.join("staging")).map_err(|_| io_error())?;
        reject_path_links(&root, &root.join("staging"))?;
        let path = root.join("preferences.redb");
        if path.exists() {
            reject_path_links(&root, &path)?;
        }
        let store = NativePreferenceStore::open(&path)?;
        let preferences = store.load()?;
        let registry = Self {
            root,
            store,
            preferences,
            leases: BTreeMap::new(),
        };
        registry.recover()?;
        Ok(registry)
    }
    fn directory(&self, id: NativeComponentId) -> PathBuf {
        self.root.join(id.slug()).join(BUILD_ID).join(TARGET)
    }
    fn lock(&self, id: NativeComponentId, shared: bool) -> PluginResult<File> {
        self.lock_version(id, BUILD_ID, TARGET, shared)
    }
    fn lock_version(
        &self,
        id: NativeComponentId,
        build: &str,
        target: &str,
        shared: bool,
    ) -> PluginResult<File> {
        let path = self
            .root
            .join(format!("{}-{build}-{target}.lock", id.slug()));
        if path.exists() {
            reject_path_links(&self.root, &path)?;
        }
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|_| io_error())?;
        let result = if shared {
            file.try_lock_shared()
        } else {
            file.try_lock()
        };
        result.map_err(|_| {
            PluginError::new(
                ErrorCode::Conflict,
                "Native component is in use by another session or application",
            )
        })?;
        Ok(file)
    }
    fn lock_all_versions(&self, id: NativeComponentId) -> PluginResult<Vec<File>> {
        let mut locks = Vec::new();
        for entry in fs::read_dir(&self.root).map_err(|_| io_error())? {
            let path = entry.map_err(|_| io_error())?.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(io_error)?;
            if !name.starts_with(&format!("{}-", id.slug())) || !name.ends_with(".lock") {
                continue;
            }
            reject_path_links(&self.root, &path)?;
            let file = File::options()
                .read(true)
                .write(true)
                .open(path)
                .map_err(|_| io_error())?;
            file.try_lock().map_err(|_| {
                PluginError::new(ErrorCode::Conflict, "Native component is still in use")
            })?;
            locks.push(file);
        }
        Ok(locks)
    }
    fn recover(&self) -> PluginResult<()> {
        for entry in fs::read_dir(self.root.join("staging")).map_err(|_| io_error())? {
            let directory = entry.map_err(|_| io_error())?.path();
            reject_path_links(&self.root, &directory)?;
            if !directory.is_dir()
                || uuid::Uuid::parse_str(
                    directory.file_name().and_then(|p| p.to_str()).unwrap_or(""),
                )
                .is_err()
            {
                continue;
            }
            let journal = directory.join("transaction.json");
            if !journal.exists() {
                fs::remove_dir_all(directory).map_err(|_| io_error())?;
                continue;
            }
            let bytes = crate::package::read_bounded(&journal, 4096)?;
            let (id, build, target): (NativeComponentId, String, String) =
                serde_json::from_slice(&bytes).map_err(|_| io_error())?;
            for value in [&build, &target] {
                nyaterm_core::plugins::validate_resource_path(value)?;
                if value.contains('/') {
                    return Err(io_error());
                }
            }
            let _lock = self.lock_version(id, &build, &target, false)?;
            let destination = self.root.join(id.slug()).join(build).join(target);
            reject_path_links(&self.root, &destination)?;
            let old = directory.join("old");
            if !destination.exists() && old.exists() {
                reject_path_links(&self.root, &old)?;
                fs::create_dir_all(destination.parent().ok_or_else(io_error)?)
                    .map_err(|_| io_error())?;
                fs::rename(&old, &destination).map_err(|_| io_error())?;
            }
            fs::remove_dir_all(directory).map_err(|_| io_error())?;
        }
        Ok(())
    }
    fn status(&self, id: NativeComponentId) -> NativeStatus {
        let preference = self
            .preferences
            .components
            .get(&id)
            .cloned()
            .unwrap_or_default();
        if preference.uninstall_pending {
            return NativeStatus::PendingUninstall;
        }
        if preference.disabled {
            return NativeStatus::Disabled;
        }
        let directory = self.directory(id);
        if directory.is_dir() {
            return if package::verify(&directory, id, &|| Ok(())).is_ok() {
                NativeStatus::Ready
            } else {
                NativeStatus::Damaged
            };
        }
        if self.root.join(id.slug()).is_dir() {
            NativeStatus::NeedsUpdate
        } else {
            NativeStatus::Missing
        }
    }
    fn collect_versions(&self, id: NativeComponentId) -> PluginResult<()> {
        let directory = self.root.join(id.slug());
        if !directory.is_dir() {
            return Ok(());
        }
        reject_path_links(&self.root, &directory)?;
        let mut versions = Vec::new();
        for entry in fs::read_dir(&directory).map_err(|_| io_error())? {
            let entry = entry.map_err(|_| io_error())?;
            let path = entry.path();
            reject_path_links(&self.root, &path)?;
            if path.is_dir() && entry.file_name() != BUILD_ID {
                versions.push((
                    entry
                        .metadata()
                        .and_then(|m| m.modified())
                        .map_err(|_| io_error())?,
                    path,
                ));
            }
        }
        versions.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        // Retain one prior build for application rollback, even if it is currently idle.
        for (_, version) in versions.into_iter().skip(1) {
            let build = version
                .file_name()
                .and_then(|v| v.to_str())
                .ok_or_else(io_error)?;
            let locks: PluginResult<Vec<File>> = fs::read_dir(&version)
                .map_err(|_| io_error())?
                .map(|entry| {
                    let entry = entry.map_err(|_| io_error())?;
                    reject_path_links(&self.root, &entry.path())?;
                    let target = entry.file_name();
                    self.lock_version(id, build, target.to_str().ok_or_else(io_error)?, false)
                })
                .collect();
            if let Ok(_locks) = locks {
                fs::remove_dir_all(version).map_err(|_| io_error())?;
            }
        }
        Ok(())
    }
    fn save(&mut self, preferences: NativePreferences) -> PluginResult<()> {
        self.store.save(&preferences)?;
        self.preferences = preferences;
        Ok(())
    }
    fn acquire(
        &mut self,
        id: NativeComponentId,
        sender: mpsc::SyncSender<Request>,
    ) -> PluginResult<NativeLaunch> {
        if self
            .preferences
            .components
            .get(&id)
            .is_some_and(|p| p.disabled || p.uninstall_pending)
        {
            return Err(PluginError::new(
                ErrorCode::Disabled,
                "Native component is disabled or pending uninstall",
            ));
        }
        if let Some(path) = std::env::var_os(id.override_variable()).filter(|p| !p.is_empty()) {
            let path = PathBuf::from(path);
            if !path.is_file() {
                return Err(PluginError::new(
                    ErrorCode::NotFound,
                    "Explicit helper override does not name a file",
                ));
            }
            return Ok(NativeLaunch {
                root: path.parent().unwrap().into(),
                executable: path,
                lease: Arc::new(()),
            });
        }
        let directory = self.directory(id);
        if directory.is_dir() {
            let lease = match self.leases.get(&id).and_then(Weak::upgrade) {
                Some(lease) => lease,
                None => {
                    let lease = Arc::new(Lease {
                        _file: Some(self.lock(id, true)?),
                        sender,
                    });
                    self.leases.insert(id, Arc::downgrade(&lease));
                    lease
                }
            };
            reject_path_links(&self.root, &directory)?;
            let manifest = package::verify(&directory, id, &|| Ok(()))?;
            return Ok(NativeLaunch {
                executable: directory.join(manifest.entry),
                root: directory,
                lease,
            });
        }
        // Local debug builds retain their adjacent helper layout. Release builds require
        // a signed component or an explicit developer override; stale legacy helpers
        // cannot establish compatibility with this exact application build.
        let executable = std::env::current_exe().map_err(|_| io_error())?;
        let root = executable.parent().ok_or_else(io_error)?;
        let helper = root.join(id.helper(TARGET));
        if helper.is_file() && cfg!(debug_assertions) {
            return Ok(NativeLaunch {
                executable: helper,
                root: root.into(),
                lease: Arc::new(()),
            });
        }
        Err(PluginError::new(
            ErrorCode::NotFound,
            "Install the matching official native component in Plugins",
        ))
    }
    fn reap(&mut self) {
        for id in NativeComponentId::ALL {
            let _ = self.collect_versions(id);
            if self
                .preferences
                .components
                .get(&id)
                .is_some_and(|p| p.uninstall_pending)
                && let Ok(_locks) = self.lock_all_versions(id)
            {
                let directory = self.root.join(id.slug());
                if directory.exists()
                    && (reject_path_links(&self.root, &directory).is_err()
                        || fs::remove_dir_all(&directory).is_err())
                {
                    continue;
                }
                let mut preferences = self.preferences.clone();
                preferences.components.insert(
                    id,
                    nyaterm_core::plugins::native::NativePreference {
                        disabled: true,
                        uninstall_pending: false,
                    },
                );
                let _ = self.save(preferences);
            }
        }
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
                    status: NativeStatus::Missing,
                    downloaded: 0,
                    download_size: None,
                    error: None,
                })
                .collect::<Vec<_>>(),
        ));
        let service = Arc::new(Self {
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
                            let _ = reply.send(result);
                        }
                        Request::Change(id, action, reply) => {
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
                            refresh(&registry, &snapshots, &changed);
                            if let Err(error) = &result {
                                update(&snapshots, id, &changed, |s| s.error = Some(error.clone()));
                            }
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
        if self
            .sender
            .try_send(Request::Change(id, action, reply))
            .is_err()
        {
            if installation {
                installing.remove(&id);
                self.cancel.lock().unwrap().remove(&id);
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
            s.error = error;
        });
    }
}
fn url(id: NativeComponentId) -> PluginResult<String> {
    if BUILD_ID == "development"
        || nyaterm_core::packaging::external_package_manager()
            .is_some_and(|manager| manager != "NyaTerm glibc 2.31 compatibility packages")
    {
        return Err(PluginError::new(
            ErrorCode::Incompatible,
            "This build uses local or externally managed native components",
        ));
    }
    Ok(format!(
        "https://downloads.nyaterm.app/components/{BUILD_ID}/{TARGET}/{}.zip",
        id.slug()
    ))
}
fn client() -> PluginResult<reqwest::blocking::Client> {
    // Bound stalled reads without imposing a total deadline on large downloads.
    // The loop checks cancellation between chunks, including during shutdown.
    reqwest::blocking::ClientBuilder::from(
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none()),
    )
    .timeout(None)
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
        NativeAction::Probe => {
            let response = client()?
                .head(url(id)?)
                .send()
                .and_then(|r| r.error_for_status())
                .map_err(|_| io_error())?;
            let size = response
                .content_length()
                .filter(|size| *size <= MAX_ARCHIVE_BYTES);
            update(snapshots, id, changed, |s| s.download_size = size);
            Ok(())
        }
        NativeAction::Install | NativeAction::Import(_) => {
            let _lock = registry.lock(id, false)?;
            let mut stage = StagingDirectory::new(&registry.root.join("staging"))?;
            let payload = stage.path.join("payload");
            fs::create_dir(&payload).map_err(|_| io_error())?;
            update(snapshots, id, changed, |s| {
                s.status = NativeStatus::Installing;
                s.downloaded = 0;
                s.error = None;
            });
            let source = match action {
                NativeAction::Import(source) => source,
                _ => {
                    let mut response = client()?
                        .get(url(id)?)
                        .send()
                        .and_then(|r| r.error_for_status())
                        .map_err(|_| io_error())?;
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
                        let n = response.read(&mut buffer).map_err(|_| io_error())?;
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
            package::extract(&source, &payload, id, cancel)?;
            cancel()?;
            let target = registry.directory(id);
            fs::create_dir_all(target.parent().ok_or_else(io_error)?).map_err(|_| io_error())?;
            reject_path_links(&registry.root, target.parent().unwrap())?;
            let journal = serde_json::to_vec(&(id, BUILD_ID, TARGET)).map_err(|_| io_error())?;
            let mut transaction =
                File::create(stage.path.join("transaction.json")).map_err(|_| io_error())?;
            transaction.write_all(&journal).map_err(|_| io_error())?;
            transaction.sync_all().map_err(|_| io_error())?;
            drop(transaction);
            let old = stage.path.join("old");
            if target.exists() {
                reject_path_links(&registry.root, &target)?;
                fs::rename(&target, &old).map_err(|_| io_error())?;
            }
            if fs::rename(&payload, &target).is_err() {
                if old.exists() && fs::rename(&old, &target).is_err() {
                    stage.retain();
                }
                return Err(io_error());
            }
            let mut preferences = registry.preferences.clone();
            preferences.components.insert(id, Default::default());
            if let Err(error) = registry.save(preferences) {
                if fs::rename(&target, &payload).is_err()
                    || (old.exists() && fs::rename(&old, &target).is_err())
                {
                    stage.retain();
                }
                return Err(error);
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::native::Registry;
    use nyaterm_core::plugins::native::{BUILD_ID, NativeComponentId, NativePreference, TARGET};
    use nyaterm_core::test_support::TestTempDir;
    use std::fs;

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
                snapshots: Arc::new(RwLock::new(Vec::new())),
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
    fn uninstall_waits_for_session_lock_and_remains_disabled_after_reaping() {
        let root = TestTempDir::new("nyaterm-native-uninstall");
        let mut registry = Registry::open(root.path().into()).unwrap();
        let id = NativeComponentId::Spice;
        let directory = registry.directory(id);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("fixture"), b"running").unwrap();
        let lease = registry.lock(id, true).unwrap();
        let mut preferences = registry.preferences.clone();
        preferences.components.insert(
            id,
            NativePreference {
                disabled: true,
                uninstall_pending: true,
            },
        );
        registry.save(preferences).unwrap();
        registry.reap();
        assert!(directory.exists());
        drop(lease);
        registry.reap();
        assert!(!directory.exists());
        assert!(registry.preferences.components[&id].disabled);
        assert!(!registry.preferences.components[&id].uninstall_pending);
    }

    #[test]
    fn new_build_can_install_while_old_build_is_in_use() {
        let root = TestTempDir::new("nyaterm-native-version-lock");
        let registry = Registry::open(root.path().into()).unwrap();
        let old = registry
            .lock_version(NativeComponentId::Rdp, "old-build", TARGET, true)
            .unwrap();
        let new = registry.lock(NativeComponentId::Rdp, false).unwrap();
        assert!(registry.lock_all_versions(NativeComponentId::Rdp).is_err());
        drop(new);
        drop(old);
        assert!(registry.lock_all_versions(NativeComponentId::Rdp).is_ok());
    }

    #[test]
    fn interrupted_replacement_restores_old_directory_on_restart() {
        let root = TestTempDir::new("nyaterm-native-recovery");
        let registry = Registry::open(root.path().into()).unwrap();
        let id = NativeComponentId::Vnc;
        let stage = root.join("staging").join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(stage.join("old")).unwrap();
        fs::write(stage.join("old/fixture"), b"old-payload").unwrap();
        fs::write(
            stage.join("transaction.json"),
            serde_json::to_vec(&(id, BUILD_ID, TARGET)).unwrap(),
        )
        .unwrap();
        drop(registry);
        let registry = Registry::open(root.path().into()).unwrap();
        assert_eq!(
            fs::read(registry.directory(id).join("fixture")).unwrap(),
            b"old-payload"
        );
        assert!(!stage.exists());
    }
}
