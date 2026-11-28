//! One application-owned clipboard connection outlives individual RDP helpers.
//! All OS clipboard and cache operations run on this worker, never in GPUI.
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime};

use nyaterm_remote_desktop::RdpSessionManager;
use nyaterm_remote_desktop::clipboard::{
    ClipboardAccess, ClipboardCommand, ClipboardContent, ClipboardEvent,
};

const POLL: Duration = Duration::from_millis(250);
const RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Default)]
struct Policy {
    owner: Option<(String, String)>,
    active: bool,
    epoch: u64,
}

pub(in crate::features::remote_desktop) struct ClipboardWorker {
    policy: Arc<Mutex<Policy>>,
    epoch: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    tx: mpsc::SyncSender<(String, ClipboardEvent)>,
    results: mpsc::Receiver<WorkerResult>,
}

pub(in crate::features::remote_desktop) struct WorkerResult {
    pub session_id: String,
    pub transfer_id: Option<String>,
    pub error: Option<String>,
}

impl ClipboardWorker {
    pub fn new(manager: Arc<RdpSessionManager>) -> std::io::Result<Self> {
        let policy = Arc::new(Mutex::new(Policy::default()));
        let epoch = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::sync_channel(64);
        let (results_tx, results) = mpsc::sync_channel(64);
        let thread_policy = policy.clone();
        let thread_epoch = epoch.clone();
        let thread_stop = stop.clone();
        std::thread::Builder::new()
            .name("rdp-clipboard".into())
            .spawn(move || {
                run(
                    manager,
                    thread_policy,
                    thread_epoch,
                    thread_stop,
                    rx,
                    results_tx,
                );
            })?;
        Ok(Self {
            policy,
            epoch,
            stop,
            tx,
            results,
        })
    }

    pub fn owner(&self) -> Option<String> {
        self.policy.lock().ok()?.owner.as_ref().map(|o| o.0.clone())
    }

    pub fn set_policy(&self, owner: Option<(String, String)>, active: bool) {
        let mut policy = self.policy.lock().unwrap_or_else(|p| p.into_inner());
        if policy.owner != owner {
            policy.epoch = self.epoch.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
            policy.owner = owner;
        }
        policy.active = active;
    }

    pub fn cancel_pending(&self, session_id: &str) {
        let mut policy = self.policy.lock().unwrap_or_else(|p| p.into_inner());
        if policy
            .owner
            .as_ref()
            .is_some_and(|owner| owner.0 == session_id)
        {
            policy.epoch = self.epoch.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
        }
    }

    pub fn receive(&self, session_id: &str, event: ClipboardEvent) -> bool {
        self.tx.try_send((session_id.into(), event)).is_ok()
    }

    pub fn results(&self) -> Vec<WorkerResult> {
        self.results.try_iter().collect()
    }
}

impl Drop for ClipboardWorker {
    fn drop(&mut self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
    }
}

// Content kind participates in equality, so text -> files -> the same text is
// a new copy. Read errors never replace this state or advance its revision.
#[derive(Default)]
struct Observation {
    content: Option<ClipboardContent>,
    fingerprint: u64,
    revision: u64,
    sent: u64,
    reported: u64,
    remote_floor: Option<u64>,
    pending: Option<(u64, u64)>,
    in_flight: Option<(u64, Instant)>,
}
impl Observation {
    fn observe(&mut self, snapshot: Snapshot) {
        if self.content.as_ref() != Some(&snapshot.content)
            || self.fingerprint != snapshot.fingerprint
        {
            self.revision = self.revision.wrapping_add(1).max(1);
            self.content = Some(snapshot.content);
            self.fingerprint = snapshot.fingerprint;
            self.remote_floor = None;
        }
    }
    fn start(&mut self, request: u64, revision: u64) {
        // Another remote copy can arrive before our Observed acknowledgement.
        // Only our own publications may bridge that revision gap.
        let revision = if self
            .remote_floor
            .is_some_and(|floor| revision >= floor && revision <= self.revision)
        {
            self.revision
        } else {
            revision
        };
        self.pending = Some((request, revision));
    }
    fn can_publish(&self, request: u64) -> bool {
        self.pending == Some((request, self.revision))
    }
    fn published(&mut self, content: Snapshot) {
        let floor = self.remote_floor.unwrap_or(self.revision);
        self.observe(content);
        self.remote_floor = Some(floor);
        self.sent = self.revision;
        self.in_flight = None;
        self.pending = None;
    }
}

struct Snapshot {
    content: ClipboardContent,
    fingerprint: u64,
}
impl From<ClipboardContent> for Snapshot {
    fn from(content: ClipboardContent) -> Self {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        if let ClipboardContent::Files(paths) = &content {
            for path in paths {
                path.hash(&mut hash);
                if let Ok(meta) = std::fs::metadata(path) {
                    meta.len().hash(&mut hash);
                    meta.modified().ok().hash(&mut hash);
                }
            }
        }
        Self {
            content,
            fingerprint: hash.finish(),
        }
    }
}

trait ClipboardPort {
    fn read(&mut self) -> Result<Snapshot, String>;
    fn write(&mut self, content: &ClipboardContent) -> Result<(), String>;
}
impl ClipboardPort for arboard::Clipboard {
    fn read(&mut self) -> Result<Snapshot, String> {
        match self.get().file_list() {
            Ok(paths) if !paths.is_empty() => {
                return Ok(system_snapshot(ClipboardContent::Files(paths)));
            }
            Ok(_) | Err(arboard::Error::ContentNotAvailable) => {}
            Err(_) => return Err("Cannot read the system file clipboard; retrying".into()),
        }
        match self.get_text() {
            Ok(text) => Ok(system_snapshot(ClipboardContent::Text(text))),
            Err(arboard::Error::ContentNotAvailable) => {
                let mut snapshot = system_snapshot(ClipboardContent::Empty);
                // Images are not transferred, but copying one must invalidate a
                // pending remote publication just like copying text or files.
                if let Ok(image) = self.get_image() {
                    let mut hash = std::collections::hash_map::DefaultHasher::new();
                    image.bytes.hash(&mut hash);
                    image.width.hash(&mut hash);
                    image.height.hash(&mut hash);
                    snapshot.fingerprint ^= hash.finish();
                }
                Ok(snapshot)
            }
            Err(_) => Err("Cannot read the system text clipboard; retrying".into()),
        }
    }
    fn write(&mut self, content: &ClipboardContent) -> Result<(), String> {
        let result = match content {
            ClipboardContent::Text(text) => self.set_text(text),
            ClipboardContent::Files(paths) => {
                // CF_HDROP alone leaves the previous Explorer OLE data object.
                self.clear().and_then(|()| self.set().file_list(paths))
            }
            ClipboardContent::Empty => self.clear(),
        };
        result.map_err(|_| "Cannot publish the remote clipboard; copy again to retry".into())
    }
}

fn system_snapshot(content: ClipboardContent) -> Snapshot {
    let snapshot = Snapshot::from(content);
    #[cfg(windows)]
    let snapshot = Snapshot {
        fingerprint: snapshot.fingerprint
            ^ u64::from(unsafe {
                windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber()
            }),
        ..snapshot
    };
    snapshot
}

fn publish(
    port: &mut dyn ClipboardPort,
    observed: &mut Observation,
    epoch: &AtomicU64,
    expected_epoch: u64,
    request: u64,
    root: &Path,
    content: ClipboardContent,
) -> Result<(), String> {
    let content = match content {
        ClipboardContent::Files(paths) => ClipboardContent::Files(resolve_files(root, paths)?),
        other => other,
    };
    // Re-read after cache validation: a large directory walk may take long enough
    // for the user to copy something else while it runs.
    observed.observe(port.read()?);
    if !observed.can_publish(request) {
        return Err("Clipboard changed; remote copy was not published".into());
    }
    if epoch.load(Ordering::SeqCst) != expected_epoch {
        return Err("Clipboard ownership changed".into());
    }
    port.write(&content)?;
    let snapshot = port.read()?;
    if snapshot.content != content {
        observed.observe(snapshot);
        return Err("Clipboard changed during publication".into());
    }
    observed.published(snapshot);
    Ok(())
}

fn cache_root() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("nyaterm")
        .join("managed-rdp-clipboard")
}
fn prepare_cache() -> Result<PathBuf, String> {
    let parent = cache_root();
    std::fs::create_dir_all(&parent).map_err(|_| "Cannot create clipboard cache")?;
    if let Ok(entries) = std::fs::read_dir(&parent) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata()
                && meta.is_dir()
                && meta
                    .modified()
                    .ok()
                    .and_then(|t| SystemTime::now().duration_since(t).ok())
                    .is_some_and(|age| age > RETENTION)
            {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    let root = parent.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir(&root).map_err(|_| "Cannot create session clipboard cache")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| "Cannot protect clipboard cache")?;
    }
    root.canonicalize()
        .map_err(|_| "Cannot resolve clipboard cache".into())
}

fn resolve_files(root: &Path, relative: Vec<PathBuf>) -> Result<Vec<PathBuf>, String> {
    if relative.is_empty() {
        return Err("Empty clipboard file list".into());
    }
    relative
        .into_iter()
        .map(|path| {
            if path
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
                || path.as_os_str().is_empty()
            {
                return Err("Unsafe clipboard cache path".into());
            }
            let mut joined = root.to_path_buf();
            for part in path.components() {
                joined.push(part);
                let metadata = std::fs::symlink_metadata(&joined)
                    .map_err(|_| "Clipboard cache file is missing")?;
                if metadata.file_type().is_symlink() {
                    return Err("Clipboard cache contains a symlink".into());
                }
            }
            let resolved = joined
                .canonicalize()
                .map_err(|_| "Cannot resolve clipboard cache file")?;
            if !resolved.starts_with(root) {
                return Err("Clipboard cache path escaped its root".into());
            }
            validate_cache_tree(root, &resolved)?;
            Ok(resolved)
        })
        .collect()
}

fn validate_cache_tree(root: &Path, path: &Path) -> Result<(), String> {
    let mut pending = vec![path.to_owned()];
    let mut count = 0;
    while let Some(path) = pending.pop() {
        count += 1;
        if count > 100_000 {
            return Err("Clipboard cache contains too many entries".into());
        }
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|_| "Clipboard cache entry is missing")?;
        if metadata.file_type().is_symlink()
            || (!metadata.is_file() && !metadata.is_dir())
            || !path
                .canonicalize()
                .map_err(|_| "Invalid clipboard cache entry")?
                .starts_with(root)
        {
            return Err("Unsafe clipboard cache entry".into());
        }
        if metadata.is_dir() {
            for entry in
                std::fs::read_dir(path).map_err(|_| "Cannot read clipboard cache directory")?
            {
                pending.push(
                    entry
                        .map_err(|_| "Cannot read clipboard cache entry")?
                        .path(),
                );
                if pending.len() + count > 100_000 {
                    return Err("Clipboard cache contains too many entries".into());
                }
            }
        }
    }
    Ok(())
}

fn run(
    manager: Arc<RdpSessionManager>,
    policy: Arc<Mutex<Policy>>,
    epoch: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    rx: mpsc::Receiver<(String, ClipboardEvent)>,
    results: mpsc::SyncSender<WorkerResult>,
) {
    let mut clipboard = None;
    let mut applied = Policy::default();
    let mut roots = HashMap::<String, PathBuf>::new();
    let mut observed = Observation::default();
    let mut last_error = None;
    let mut last_read_error = None;
    let report = |id: &str, transfer_id, error: String| {
        let _ = results.send(WorkerResult {
            session_id: id.into(),
            transfer_id,
            error: Some(error),
        });
    };
    while !stop.load(Ordering::SeqCst) {
        let current = policy.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if current.owner != applied.owner
            || current.active != applied.active
            || current.epoch != applied.epoch
        {
            if current.owner != applied.owner || current.epoch != applied.epoch {
                if let Some((id, _)) = &applied.owner {
                    let _ = manager.send_managed_clipboard(
                        id,
                        ClipboardCommand::Authorize {
                            epoch: current.epoch,
                            access: ClipboardAccess::Disabled,
                            cache_root: PathBuf::new(),
                        },
                    );
                }
                observed = Observation::default();
            }
            if let Some((id, token)) = &current.owner {
                let root = match roots.get(token) {
                    Some(root) => Ok(root.clone()),
                    None => prepare_cache(),
                };
                match root {
                    Ok(root) => {
                        roots.insert(token.clone(), root.clone());
                        let result = manager.send_managed_clipboard(
                            id,
                            ClipboardCommand::Authorize {
                                epoch: current.epoch,
                                access: if current.active {
                                    ClipboardAccess::Active
                                } else {
                                    ClipboardAccess::Drain
                                },
                                cache_root: root,
                            },
                        );
                        if let Err(error) = result {
                            report(id, None, error.message);
                            std::thread::sleep(POLL);
                            continue;
                        }
                    }
                    Err(error) => {
                        report(id, None, error);
                        std::thread::sleep(POLL);
                        continue;
                    }
                }
            }
            applied = current.clone();
        }
        if let Some((id, _)) = &current.owner
            && clipboard.is_none()
        {
            clipboard = arboard::Clipboard::new().ok();
            if clipboard.is_none() {
                let error = "System clipboard unavailable; retrying".to_string();
                if last_read_error.as_ref() != Some(&error) {
                    report(id, None, error.clone());
                }
                last_read_error = Some(error);
            }
        }
        let read_ok = if let (Some(port), Some((id, _))) = (clipboard.as_mut(), &current.owner) {
            match port.read() {
                Ok(content) => {
                    observed.observe(content);
                    last_read_error = None;
                    true
                }
                Err(error) => {
                    if last_read_error.as_ref() != Some(&error) {
                        report(id, None, error.clone());
                    }
                    last_read_error = Some(error);
                    false
                }
            }
        } else {
            false
        };
        // Drain events before advertising local changes: a remote publish must
        // mark its content as sent so it cannot bounce back to the server.
        for (session_id, event) in rx.try_iter().take(64) {
            let Some((id, token)) = &current.owner else {
                continue;
            };
            if session_id != *id || epoch.load(Ordering::SeqCst) != current.epoch {
                continue;
            }
            match event {
                ClipboardEvent::Applied {
                    epoch: event_epoch,
                    revision,
                    accepted,
                } if event_epoch == current.epoch => {
                    if observed.in_flight.is_some_and(|(r, _)| r == revision) && accepted {
                        observed.sent = revision;
                        observed.reported = revision;
                        observed.in_flight = None;
                        last_error = None;
                    }
                }
                ClipboardEvent::Started {
                    epoch: event_epoch,
                    request,
                    revision,
                } if event_epoch == current.epoch => {
                    if read_ok {
                        observed.start(request, revision);
                    } else {
                        observed.pending = None;
                    }
                }
                ClipboardEvent::Ready {
                    epoch: event_epoch,
                    request,
                    content,
                    transfer_id,
                } if event_epoch == current.epoch => {
                    let result = match (clipboard.as_mut(), roots.get(token)) {
                        (Some(port), Some(root)) => publish(
                            port,
                            &mut observed,
                            &epoch,
                            event_epoch,
                            request,
                            root,
                            content,
                        ),
                        _ => Err("System clipboard or session cache unavailable".into()),
                    };
                    match result {
                        Err(error) => report(id, transfer_id, error),
                        Ok(()) => {
                            if transfer_id.is_some() {
                                let _ = results.send(WorkerResult {
                                    session_id: id.clone(),
                                    transfer_id,
                                    error: None,
                                });
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if observed.sent == observed.revision
            && observed.reported != observed.revision
            && epoch.load(Ordering::SeqCst) == current.epoch
            && let Some((id, _)) = &current.owner
            && manager
                .send_managed_clipboard(
                    id,
                    ClipboardCommand::Observed {
                        epoch: current.epoch,
                        revision: observed.revision,
                    },
                )
                .is_ok()
        {
            observed.reported = observed.revision;
        }
        if current.active
            && read_ok
            && observed.sent != observed.revision
            && observed.in_flight.is_none_or(|(revision, sent)| {
                revision != observed.revision || sent.elapsed() >= Duration::from_secs(2)
            })
            && epoch.load(Ordering::SeqCst) == current.epoch
            && let Some((id, _)) = &current.owner
            && let Some(content) = &observed.content
        {
            match manager.send_managed_clipboard(
                id,
                ClipboardCommand::Snapshot {
                    epoch: current.epoch,
                    revision: observed.revision,
                    content: content.clone(),
                },
            ) {
                Ok(()) => observed.in_flight = Some((observed.revision, Instant::now())),
                Err(error) => {
                    observed.in_flight = Some((observed.revision, Instant::now()));
                    if last_error.as_ref() != Some(&error.message) {
                        report(id, None, error.message.clone());
                    }
                    last_error = Some(error.message);
                }
            }
        }
        std::thread::sleep(POLL);
    }
    if let Some((id, _)) = applied.owner {
        let _ = manager.send_managed_clipboard(
            &id,
            ClipboardCommand::Authorize {
                epoch: epoch.load(Ordering::SeqCst),
                access: ClipboardAccess::Disabled,
                cache_root: PathBuf::new(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{ClipboardPort, Observation, resolve_files};
    use nyaterm_remote_desktop::clipboard::ClipboardContent;
    #[test]
    fn consecutive_remote_copies_bridge_only_our_own_unacknowledged_publications() {
        let mut state = Observation::default();
        state.observe(ClipboardContent::Text("original".into()).into());
        let baseline = state.revision;
        state.published(ClipboardContent::Text("remote one".into()).into());
        state.start(2, baseline);
        assert!(state.can_publish(2));
        state.published(ClipboardContent::Text("remote two".into()).into());
        state.start(3, baseline);
        assert!(state.can_publish(3));
        state.observe(ClipboardContent::Text("new user copy".into()).into());
        state.start(4, baseline);
        assert!(!state.can_publish(4));
    }

    #[test]
    fn delayed_remote_start_does_not_rebase_onto_a_new_local_copy() {
        let mut state = Observation::default();
        state.observe(ClipboardContent::Text("old".into()).into());
        let baseline = state.revision;
        state.observe(ClipboardContent::Text("user copied after remote request".into()).into());
        state.start(1, baseline);
        assert!(!state.can_publish(1));
        state.published(ClipboardContent::Text("a later remote copy".into()).into());
        state.start(2, state.revision);
        assert!(state.can_publish(2));
    }

    #[test]
    fn switching_sessions_and_cancelling_revoke_queued_publications_but_alt_tab_does_not() {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        use std::sync::{Arc, Mutex, mpsc};
        let (tx, _rx) = mpsc::sync_channel(1);
        let (_results, results) = mpsc::sync_channel(1);
        let worker = super::ClipboardWorker {
            policy: Arc::new(Mutex::new(super::Policy::default())),
            epoch: Arc::new(AtomicU64::new(0)),
            stop: Arc::new(AtomicBool::new(false)),
            tx,
            results,
        };
        let first = Some(("first".into(), "attempt-1".into()));
        worker.set_policy(first.clone(), true);
        let first_epoch = worker.epoch.load(Ordering::SeqCst);
        worker.set_policy(first, false);
        assert_eq!(
            worker.epoch.load(Ordering::SeqCst),
            first_epoch,
            "Alt+Tab drains the existing copy"
        );
        worker.set_policy(Some(("second".into(), "attempt-2".into())), true);
        let second_epoch = worker.epoch.load(Ordering::SeqCst);
        assert_ne!(first_epoch, second_epoch);
        worker.cancel_pending("first");
        assert_eq!(worker.epoch.load(Ordering::SeqCst), second_epoch);
        worker.cancel_pending("second");
        assert_ne!(worker.epoch.load(Ordering::SeqCst), second_epoch);
        worker.set_policy(None, false);
        assert_eq!(
            worker.owner(),
            None,
            "locking or disconnecting removes ownership"
        );
    }

    #[test]
    fn publication_checks_current_clipboard_and_authorization_before_writing() {
        struct FakePort {
            content: ClipboardContent,
            writes: usize,
        }
        impl ClipboardPort for FakePort {
            fn read(&mut self) -> Result<super::Snapshot, String> {
                Ok(self.content.clone().into())
            }
            fn write(&mut self, content: &ClipboardContent) -> Result<(), String> {
                self.content = content.clone();
                self.writes += 1;
                Ok(())
            }
        }
        let mut port = FakePort {
            content: ClipboardContent::Text("original".into()),
            writes: 0,
        };
        let mut observed = Observation::default();
        observed.observe(port.read().unwrap());
        observed.start(1, observed.revision);
        let epoch = std::sync::atomic::AtomicU64::new(2);
        let root = std::path::Path::new("unused-for-text");
        assert!(
            super::publish(
                &mut port,
                &mut observed,
                &epoch,
                1,
                1,
                root,
                ClipboardContent::Text("remote".into())
            )
            .is_err()
        );
        assert_eq!(port.writes, 0);
        port.content = ClipboardContent::Text("user copied something else".into());
        assert!(
            super::publish(
                &mut port,
                &mut observed,
                &epoch,
                2,
                1,
                root,
                ClipboardContent::Text("remote".into())
            )
            .is_err()
        );
        assert_eq!(port.writes, 0);
        observed.start(2, observed.revision);
        super::publish(
            &mut port,
            &mut observed,
            &epoch,
            2,
            2,
            root,
            ClipboardContent::Text("remote".into()),
        )
        .unwrap();
        assert_eq!(port.writes, 1);
        assert_eq!(observed.sent, observed.revision);
    }

    #[test]
    fn alternating_formats_republishes_identical_content_and_remote_writes_do_not_echo() {
        let mut state = Observation::default();
        let text = ClipboardContent::Text("hello".into());
        state.observe(text.clone().into());
        state.sent = state.revision;
        state.observe(ClipboardContent::Files(vec!["a".into()]).into());
        state.observe(text.clone().into());
        assert_ne!(state.sent, state.revision);
        state.published(text.into());
        assert_eq!(state.sent, state.revision);
    }
    #[test]
    fn a_local_copy_or_new_remote_request_invalidates_pending_publication() {
        let mut state = Observation::default();
        state.observe(ClipboardContent::Text("old".into()).into());
        state.start(1, state.revision);
        assert!(state.can_publish(1));
        state.observe(ClipboardContent::Text("new".into()).into());
        assert!(!state.can_publish(1));
        state.start(2, state.revision);
        assert!(!state.can_publish(1));
        assert!(state.can_publish(2));
    }
    #[test]
    fn cache_paths_must_exist_and_stay_inside_the_session_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::write(root.join("file"), b"fixture").unwrap();
        assert!(resolve_files(&root, vec!["file".into()]).is_ok());
        assert!(resolve_files(&root, vec!["../outside".into()]).is_err());
        assert!(resolve_files(&root, vec![root.join("file")]).is_err());
        assert!(resolve_files(&root, vec!["missing".into()]).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("file"), root.join("link")).unwrap();
            assert!(resolve_files(&root, vec!["link".into()]).is_err());
        }
    }
    #[test]
    #[ignore = "requires an isolated display: xvfb-run cargo test -p nyaterm-desktop persistent_file_clipboard -- --ignored"]
    fn persistent_file_clipboard_survives_publication_return() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fixture");
        std::fs::write(&path, b"fixture").unwrap();
        let mut owner = arboard::Clipboard::new().unwrap();
        owner
            .write(&ClipboardContent::Files(vec![path.clone()]))
            .unwrap();
        let mut reader = arboard::Clipboard::new().unwrap();
        assert_eq!(
            reader.read().unwrap().content,
            ClipboardContent::Files(vec![path])
        );
    }
}
