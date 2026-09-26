use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use ironrdp_cliprdr::is_windows_device_name;
use ironrdp_cliprdr::pdu::{
    ClipboardFileAttributes, FileContentsFlags, FileContentsRequest, FileContentsResponse,
    FileDescriptor, LockDataId,
};

pub(super) const FILE_CHUNK_SIZE: u32 = 1024 * 1024;
const MAX_FILE_COUNT: usize = 100_000;
const MAX_TRANSFER_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const REMOTE_TRANSFER_TIMEOUT: Duration = Duration::from_secs(60);
pub(super) const CACHE_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug)]
struct LocalFileEntry {
    path: PathBuf,
    size: u64,
    is_directory: bool,
}

#[derive(Debug)]
pub(super) struct LocalFileSnapshot {
    pub descriptors: Vec<FileDescriptor>,
    entries: Vec<LocalFileEntry>,
}

#[derive(Debug)]
struct RemoteFileSpec {
    index: i32,
    path: PathBuf,
    size: Option<u64>,
}

#[derive(Debug)]
enum PendingResponse {
    Size,
    Range(u32),
}

#[derive(Debug)]
struct ActiveFile {
    spec: RemoteFileSpec,
    file: File,
    offset: u64,
    pending: PendingResponse,
}

#[derive(Debug)]
pub(super) struct RemoteTransfer {
    pub id: String,
    pub name: String,
    root: PathBuf,
    clipboard_paths: Vec<PathBuf>,
    pending: VecDeque<RemoteFileSpec>,
    active: HashMap<u32, ActiveFile>,
    clip_data_id: Option<u32>,
    pub total_bytes: u64,
    pub transferred_bytes: u64,
    pub total_files: u64,
    pub completed_files: u64,
    last_activity: Instant,
}

pub(super) struct RemoteUpdate {
    pub requests: Vec<FileContentsRequest>,
    pub completed: bool,
}

#[derive(Default)]
pub(super) struct FileState {
    current: Option<Arc<LocalFileSnapshot>>,
    locked: HashMap<u32, Arc<LocalFileSnapshot>>,
    pub remote: Option<RemoteTransfer>,
    completed_root: Option<PathBuf>,
    completed_hash: Option<u64>,
    pub last_local_hash: Option<u64>,
    next_stream_id: u32,
    closed: bool,
}

impl FileState {
    pub fn set_local_snapshot(
        &mut self,
        snapshot: LocalFileSnapshot,
        hash: u64,
    ) -> Vec<FileDescriptor> {
        let descriptors = snapshot.descriptors.clone();
        self.current = Some(Arc::new(snapshot));
        self.last_local_hash = Some(hash);
        descriptors
    }

    pub fn is_remote_clipboard(&self, hash: u64) -> bool {
        self.completed_hash == Some(hash)
    }

    pub fn release_completed(&mut self) {
        if let Some(root) = self.completed_root.take() {
            let _ = fs::remove_dir_all(root);
        }
        self.completed_hash = None;
    }

    pub fn lock(&mut self, id: LockDataId) {
        if let Some(snapshot) = &self.current {
            self.locked.insert(id.0, Arc::clone(snapshot));
        }
    }

    pub fn unlock(&mut self, id: LockDataId) {
        self.locked.remove(&id.0);
    }

    pub fn serve(
        &self,
        request: &FileContentsRequest,
    ) -> Result<FileContentsResponse<'static>, String> {
        request.flags.validate().map_err(str::to_string)?;
        let snapshot = if let Some(id) = request.data_id {
            self.locked.get(&id)
        } else {
            self.current.as_ref()
        }
        .ok_or_else(|| "clipboard snapshot is no longer available".to_string())?;
        serve_snapshot_request(snapshot, request)
    }

    pub fn begin_remote(
        &mut self,
        descriptors: &[FileDescriptor],
        clip_data_id: Option<u32>,
    ) -> Result<RemoteUpdate, String> {
        if self.closed {
            return Err("clipboard session is closed".to_string());
        }
        self.cancel_remote();
        let id = uuid::Uuid::new_v4().to_string();
        let root = cache_root().join(&id);
        let prepared = prepare_remote_files(descriptors, &root)?;
        fs::create_dir_all(&root)
            .map_err(|error| format!("failed to create clipboard cache: {error}"))?;
        for directory in &prepared.directories {
            if let Err(error) = fs::create_dir_all(directory) {
                let _ = fs::remove_dir_all(&root);
                return Err(format!("failed to create clipboard directory: {error}"));
            }
        }
        let total_bytes = prepared
            .files
            .iter()
            .filter_map(|file| file.size)
            .try_fold(0_u64, |sum, size| sum.checked_add(size))
            .ok_or_else(|| "clipboard transfer exceeds size limit".to_string())?;
        if total_bytes > MAX_TRANSFER_BYTES {
            let _ = fs::remove_dir_all(&root);
            return Err("clipboard transfer exceeds size limit".to_string());
        }
        let mut remote = RemoteTransfer {
            id,
            name: prepared.name,
            root,
            clipboard_paths: prepared.clipboard_paths,
            pending: prepared.files.into(),
            active: HashMap::new(),
            clip_data_id,
            total_bytes,
            transferred_bytes: 0,
            total_files: prepared.file_count,
            completed_files: 0,
            last_activity: Instant::now(),
        };
        let result = self.pump(&mut remote);
        match result {
            Ok(requests) => {
                let completed = remote.pending.is_empty() && remote.active.is_empty();
                self.remote = Some(remote);
                Ok(RemoteUpdate {
                    requests,
                    completed,
                })
            }
            Err(error) => {
                let _ = fs::remove_dir_all(&remote.root);
                Err(error)
            }
        }
    }

    fn pump(&mut self, transfer: &mut RemoteTransfer) -> Result<Vec<FileContentsRequest>, String> {
        let mut requests = Vec::new();
        while transfer.active.len() < 2 {
            let Some(spec) = transfer.pending.pop_front() else {
                break;
            };
            if let Some(parent) = spec.path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("failed to create clipboard parent: {error}"))?;
            }
            let file = File::create(&spec.path)
                .map_err(|error| format!("failed to create clipboard file: {error}"))?;
            if spec.size == Some(0) {
                transfer.completed_files += 1;
                continue;
            }
            self.next_stream_id = self.next_stream_id.wrapping_add(1).max(1);
            let stream_id = self.next_stream_id;
            let (pending, flags, requested_size) = match spec.size {
                Some(size) => (
                    PendingResponse::Range(size.min(u64::from(FILE_CHUNK_SIZE)) as u32),
                    FileContentsFlags::RANGE,
                    size.min(u64::from(FILE_CHUNK_SIZE)) as u32,
                ),
                None => (PendingResponse::Size, FileContentsFlags::SIZE, 8),
            };
            requests.push(FileContentsRequest {
                stream_id,
                index: spec.index,
                flags,
                position: 0,
                requested_size,
                data_id: transfer.clip_data_id,
            });
            transfer.active.insert(
                stream_id,
                ActiveFile {
                    spec,
                    file,
                    offset: 0,
                    pending,
                },
            );
        }
        Ok(requests)
    }

    pub fn handle_response(
        &mut self,
        response: &FileContentsResponse<'_>,
    ) -> Result<RemoteUpdate, String> {
        let Some(mut transfer) = self.remote.take() else {
            return Ok(RemoteUpdate {
                requests: Vec::new(),
                completed: false,
            });
        };
        let result = (|| -> Result<RemoteUpdate, String> {
            if response.is_error() {
                return Err("remote rejected clipboard file request".to_string());
            }
            let stream_id = response.stream_id();
            let mut active = transfer
                .active
                .remove(&stream_id)
                .ok_or_else(|| "unknown clipboard stream response".to_string())?;
            transfer.last_activity = Instant::now();
            match active.pending {
                PendingResponse::Size => {
                    let size = response
                        .data_as_size()
                        .map_err(|error| format!("invalid clipboard size: {error}"))?;
                    transfer.total_bytes = transfer
                        .total_bytes
                        .checked_add(size)
                        .filter(|size| *size <= MAX_TRANSFER_BYTES)
                        .ok_or_else(|| "clipboard transfer exceeds size limit".to_string())?;
                    active.spec.size = Some(size);
                }
                PendingResponse::Range(requested) => {
                    let data = response.data();
                    let size = active
                        .spec
                        .size
                        .ok_or_else(|| "clipboard file size is unknown".to_string())?;
                    if data.is_empty()
                        || data.len() > requested as usize
                        || active.offset.saturating_add(data.len() as u64) > size
                    {
                        return Err("invalid clipboard file range".to_string());
                    }
                    active
                        .file
                        .write_all(data)
                        .map_err(|error| format!("failed to write clipboard cache: {error}"))?;
                    active.offset += data.len() as u64;
                    transfer.transferred_bytes += data.len() as u64;
                }
            }
            let mut requests = Vec::new();
            let size = active.spec.size.unwrap_or(0);
            if active.offset >= size {
                active
                    .file
                    .flush()
                    .map_err(|error| format!("failed to flush clipboard cache: {error}"))?;
                transfer.completed_files += 1;
            } else {
                let requested_size = (size - active.offset).min(u64::from(FILE_CHUNK_SIZE)) as u32;
                active.pending = PendingResponse::Range(requested_size);
                requests.push(FileContentsRequest {
                    stream_id,
                    index: active.spec.index,
                    flags: FileContentsFlags::RANGE,
                    position: active.offset,
                    requested_size,
                    data_id: transfer.clip_data_id,
                });
                transfer.active.insert(stream_id, active);
            }
            requests.extend(self.pump(&mut transfer)?);
            Ok(RemoteUpdate {
                completed: transfer.pending.is_empty() && transfer.active.is_empty(),
                requests,
            })
        })();
        match result {
            Ok(update) => {
                self.remote = Some(transfer);
                Ok(update)
            }
            Err(error) => {
                let root = transfer.root.clone();
                drop(transfer);
                let _ = fs::remove_dir_all(root);
                Err(error)
            }
        }
    }

    pub fn complete_remote(&mut self) -> Result<(), String> {
        let transfer = self
            .remote
            .take()
            .ok_or_else(|| "clipboard transfer missing".to_string())?;
        if transfer.transferred_bytes != transfer.total_bytes {
            let _ = fs::remove_dir_all(&transfer.root);
            return Err("clipboard byte count did not match declared size".to_string());
        }
        if let Err(error) = write_clipboard_paths(&transfer.clipboard_paths) {
            let _ = fs::remove_dir_all(&transfer.root);
            return Err(error);
        }
        self.release_completed();
        self.completed_hash = Some(file_list_fingerprint(&transfer.clipboard_paths));
        self.last_local_hash = self.completed_hash;
        self.completed_root = Some(transfer.root);
        Ok(())
    }

    pub fn cancel_remote(&mut self) -> Option<String> {
        let transfer = self.remote.take()?;
        let id = transfer.id.clone();
        let root = transfer.root.clone();
        drop(transfer);
        let _ = fs::remove_dir_all(root);
        Some(id)
    }

    pub fn cancel_if_id(&mut self, id: &str) -> bool {
        if self
            .remote
            .as_ref()
            .is_some_and(|transfer| transfer.id == id)
        {
            self.cancel_remote();
            true
        } else {
            false
        }
    }

    pub fn cancel_lock(&mut self, ids: &[LockDataId]) -> Option<String> {
        if self
            .remote
            .as_ref()
            .and_then(|transfer| transfer.clip_data_id)
            .is_some_and(|id| ids.iter().any(|lock| lock.0 == id))
        {
            self.cancel_remote()
        } else {
            None
        }
    }

    pub fn expire(&mut self) -> Option<String> {
        if self.is_expired() {
            self.cancel_remote()
        } else {
            None
        }
    }

    pub fn is_expired(&self) -> bool {
        self.remote
            .as_ref()
            .is_some_and(|transfer| transfer.last_activity.elapsed() >= REMOTE_TRANSFER_TIMEOUT)
    }

    pub fn shutdown(&mut self) {
        self.closed = true;
        self.cancel_remote();
        self.locked.clear();
    }
}

pub(super) fn read_clipboard_paths() -> Option<Vec<PathBuf>> {
    arboard::Clipboard::new().ok()?.get().file_list().ok()
}

fn write_clipboard_paths(paths: &[PathBuf]) -> Result<(), String> {
    arboard::Clipboard::new()
        .map_err(|error| error.to_string())?
        .set()
        .file_list(paths)
        .map_err(|error| error.to_string())
}

pub(super) fn file_list_fingerprint(paths: &[PathBuf]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for path in paths {
        path.hash(&mut hasher);
        if let Ok(metadata) = fs::metadata(path) {
            metadata.len().hash(&mut hasher);
            metadata.is_dir().hash(&mut hasher);
            metadata.modified().ok().hash(&mut hasher);
        }
    }
    hasher.finish()
}

pub(super) fn cache_root() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("nyaterm")
        .join("rdp-clipboard")
}

pub(super) fn cleanup_stale_cache(root: &Path, retention: Duration) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        if !entry
            .file_type()
            .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
        {
            continue;
        }
        let stale = entry
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= retention);
        if stale {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

pub(super) fn build_local_snapshot(paths: &[PathBuf]) -> Result<LocalFileSnapshot, String> {
    if paths.is_empty() || paths.len() > MAX_FILE_COUNT {
        return Err("invalid local clipboard file count".to_string());
    }
    let mut snapshot = LocalFileSnapshot {
        descriptors: Vec::new(),
        entries: Vec::new(),
    };
    let mut names = HashSet::new();
    for selected in paths {
        let metadata = fs::symlink_metadata(selected).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err("clipboard symlink is not allowed".to_string());
        }
        let canonical = selected.canonicalize().map_err(|error| error.to_string())?;
        let name = selected
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "invalid clipboard file name".to_string())?;
        if !names.insert(name.to_lowercase()) {
            return Err("duplicate clipboard root name".to_string());
        }
        append_local_entry(&canonical, Path::new(name), &mut snapshot)?;
    }
    let total = snapshot
        .entries
        .iter()
        .try_fold(0_u64, |sum, entry| sum.checked_add(entry.size))
        .ok_or_else(|| "clipboard transfer exceeds size limit".to_string())?;
    if total > MAX_TRANSFER_BYTES {
        return Err("clipboard transfer exceeds size limit".to_string());
    }
    Ok(snapshot)
}

fn append_local_entry(
    path: &Path,
    relative: &Path,
    snapshot: &mut LocalFileSnapshot,
) -> Result<(), String> {
    if snapshot.entries.len() >= MAX_FILE_COUNT {
        return Err("clipboard contains too many files".to_string());
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || (!metadata.is_file() && !metadata.is_dir()) {
        return Err("unsupported clipboard file type".to_string());
    }
    if relative.to_string_lossy().encode_utf16().count() + 1 > 260 {
        return Err("clipboard relative path exceeds RDP limit".to_string());
    }
    let name = relative
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "invalid clipboard file name".to_string())?;
    let parent = relative
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let mut descriptor = FileDescriptor::new(name).with_attributes(if metadata.is_dir() {
        ClipboardFileAttributes::DIRECTORY
    } else {
        ClipboardFileAttributes::NORMAL | ClipboardFileAttributes::ARCHIVE
    });
    if metadata.is_file() {
        descriptor = descriptor.with_file_size(metadata.len());
    }
    if let Some(parent) = parent {
        descriptor = descriptor.with_relative_path(path_to_wire(parent)?);
    }
    snapshot.descriptors.push(descriptor);
    snapshot.entries.push(LocalFileEntry {
        path: path.to_path_buf(),
        size: if metadata.is_file() {
            metadata.len()
        } else {
            0
        },
        is_directory: metadata.is_dir(),
    });
    if metadata.is_dir() {
        let mut children = fs::read_dir(path)
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            append_local_entry(&child.path(), &relative.join(child.file_name()), snapshot)?;
        }
    }
    Ok(())
}

fn path_to_wire(path: &Path) -> Result<String, String> {
    path.components()
        .map(|component| match component {
            Component::Normal(part) => part
                .to_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| "clipboard path is not UTF-8".to_string()),
            _ => Err("clipboard path is not relative".to_string()),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join("\\"))
}

fn serve_snapshot_request(
    snapshot: &LocalFileSnapshot,
    request: &FileContentsRequest,
) -> Result<FileContentsResponse<'static>, String> {
    let index =
        usize::try_from(request.index).map_err(|_| "invalid clipboard file index".to_string())?;
    let entry = snapshot
        .entries
        .get(index)
        .ok_or_else(|| "clipboard file index out of range".to_string())?;
    if request.flags.contains(FileContentsFlags::SIZE) {
        if request.position != 0 || request.requested_size != 8 {
            return Err("invalid clipboard SIZE request".to_string());
        }
        return Ok(FileContentsResponse::new_size_response(
            request.stream_id,
            entry.size,
        ));
    }
    if entry.is_directory
        || request.requested_size == 0
        || request.requested_size > FILE_CHUNK_SIZE
        || request.position > entry.size
    {
        return Err("invalid clipboard RANGE request".to_string());
    }
    let count = (entry.size - request.position).min(u64::from(request.requested_size)) as usize;
    let mut file = File::open(&entry.path).map_err(|error| error.to_string())?;
    file.seek(SeekFrom::Start(request.position))
        .map_err(|error| error.to_string())?;
    let mut data = vec![0; count];
    file.read_exact(&mut data)
        .map_err(|error| error.to_string())?;
    Ok(FileContentsResponse::new_data_response(
        request.stream_id,
        data,
    ))
}

struct PreparedRemoteFiles {
    name: String,
    directories: Vec<PathBuf>,
    files: Vec<RemoteFileSpec>,
    clipboard_paths: Vec<PathBuf>,
    file_count: u64,
}

fn prepare_remote_files(
    descriptors: &[FileDescriptor],
    root: &Path,
) -> Result<PreparedRemoteFiles, String> {
    if descriptors.is_empty() || descriptors.len() > MAX_FILE_COUNT {
        return Err("invalid remote clipboard file count".to_string());
    }
    let mut seen = HashSet::new();
    let mut top_level = HashSet::new();
    let mut directories = Vec::new();
    let mut files = Vec::new();
    for (index, descriptor) in descriptors.iter().enumerate() {
        let relative = descriptor_relative_path(descriptor)?;
        if !seen.insert(relative.to_string_lossy().to_lowercase()) {
            return Err("duplicate remote clipboard path".to_string());
        }
        if let Some(Component::Normal(name)) = relative.components().next() {
            top_level.insert(name.to_os_string());
        }
        let target = root.join(relative);
        if descriptor
            .attributes
            .is_some_and(|attributes| attributes.contains(ClipboardFileAttributes::DIRECTORY))
        {
            directories.push(target);
        } else {
            files.push(RemoteFileSpec {
                index: i32::try_from(index).map_err(|_| "clipboard index too large".to_string())?,
                path: target,
                size: descriptor.file_size,
            });
        }
    }
    directories.sort_by_key(|path| path.components().count());
    let mut top_level = top_level.into_iter().collect::<Vec<_>>();
    top_level.sort();
    let clipboard_paths = top_level
        .iter()
        .map(|name| root.join(name))
        .collect::<Vec<_>>();
    let name = if clipboard_paths.len() == 1 {
        top_level[0].to_string_lossy().into_owned()
    } else {
        format!("{} RDP clipboard items", clipboard_paths.len())
    };
    Ok(PreparedRemoteFiles {
        name,
        directories,
        file_count: files.len() as u64,
        files,
        clipboard_paths,
    })
}

fn descriptor_relative_path(descriptor: &FileDescriptor) -> Result<PathBuf, String> {
    let mut path = PathBuf::new();
    let mut parts = Vec::new();
    if let Some(parent) = descriptor.relative_path.as_deref() {
        parts.extend(parent.split(['/', '\\']));
    }
    if descriptor.name.contains(['/', '\\']) {
        return Err("unsafe remote clipboard file name".to_string());
    }
    parts.push(&descriptor.name);
    for part in parts {
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.contains(['\0', ':'])
            || part.ends_with([' ', '.'])
            || is_windows_device_name(part)
        {
            return Err("unsafe remote clipboard path".to_string());
        }
        path.push(part);
    }
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("non-relative clipboard path".to_string());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use ironrdp_cliprdr::pdu::{
        FileContentsFlags, FileContentsRequest, FileContentsResponse, FileDescriptor,
    };

    use super::{
        FILE_CHUNK_SIZE, FileState, MAX_TRANSFER_BYTES, build_local_snapshot, cleanup_stale_cache,
        descriptor_relative_path, prepare_remote_files, serve_snapshot_request,
    };

    #[test]
    fn rejects_traversal_device_names_and_duplicate_paths() {
        let mut traversal = FileDescriptor::new("secret.txt");
        traversal.relative_path = Some("..\\outside".to_string());
        assert!(descriptor_relative_path(&traversal).is_err());
        assert!(descriptor_relative_path(&FileDescriptor::new("CON.txt")).is_err());
        assert!(
            prepare_remote_files(
                &[FileDescriptor::new("A"), FileDescriptor::new("a")],
                &PathBuf::from("cache")
            )
            .is_err()
        );
    }

    #[test]
    fn local_snapshot_preserves_directories_and_bounds_ranges() {
        let root = std::env::temp_dir().join(format!("nyaterm-clip-test-{}", uuid::Uuid::new_v4()));
        let directory = root.join("folder");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("sample.bin"), b"abcdefgh").unwrap();
        let snapshot = build_local_snapshot(&[directory]).unwrap();
        assert_eq!(snapshot.descriptors.len(), 2);
        assert_eq!(
            snapshot.descriptors[1].relative_path.as_deref(),
            Some("folder")
        );
        let request = FileContentsRequest {
            stream_id: 1,
            index: 1,
            flags: FileContentsFlags::RANGE,
            position: 2,
            requested_size: 3,
            data_id: None,
        };
        assert_eq!(
            serve_snapshot_request(&snapshot, &request).unwrap().data(),
            b"cde"
        );
        assert!(
            serve_snapshot_request(
                &snapshot,
                &FileContentsRequest {
                    requested_size: FILE_CHUNK_SIZE + 1,
                    ..request
                }
            )
            .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn remote_chunks_are_bounded_and_cancellation_removes_partial_cache() {
        let mut state = FileState::default();
        let update = state
            .begin_remote(&[FileDescriptor::new("sample.bin").with_file_size(8)], None)
            .unwrap();
        let root = state.remote.as_ref().unwrap().root.clone();
        assert_eq!(update.requests.len(), 1);
        assert_eq!(update.requests[0].requested_size, 8);
        let stream_id = update.requests[0].stream_id;
        let next = state
            .handle_response(&FileContentsResponse::new_data_response(
                stream_id,
                b"abc".to_vec(),
            ))
            .unwrap();
        assert_eq!(state.remote.as_ref().unwrap().transferred_bytes, 3);
        assert_eq!(next.requests[0].position, 3);
        assert_eq!(next.requests[0].requested_size, 5);
        assert!(
            state
                .handle_response(&FileContentsResponse::new_data_response(
                    stream_id,
                    b"abcdef".to_vec()
                ))
                .is_err()
        );
        assert!(!root.exists());

        let update = state
            .begin_remote(&[FileDescriptor::new("second.bin").with_file_size(8)], None)
            .unwrap();
        let root = state.remote.as_ref().unwrap().root.clone();
        assert!(!update.completed);
        state.cancel_remote();
        assert!(!root.exists());
    }

    #[test]
    fn declared_remote_size_over_limit_creates_no_cache() {
        let mut state = FileState::default();
        assert!(
            state
                .begin_remote(
                    &[FileDescriptor::new("large.bin").with_file_size(MAX_TRANSFER_BYTES + 1)],
                    None
                )
                .is_err()
        );
        assert!(state.remote.is_none());

        let request = state
            .begin_remote(&[FileDescriptor::new("unknown.bin")], None)
            .unwrap()
            .requests
            .remove(0);
        let root = state.remote.as_ref().unwrap().root.clone();
        assert!(
            state
                .handle_response(&FileContentsResponse::new_size_response(
                    request.stream_id,
                    MAX_TRANSFER_BYTES + 1
                ))
                .is_err()
        );
        assert!(!root.exists());
    }

    #[test]
    fn timed_out_generation_is_cancelled_and_cache_is_removed() {
        let mut state = FileState::default();
        state
            .begin_remote(
                &[FileDescriptor::new("partial.bin").with_file_size(10)],
                None,
            )
            .unwrap();
        let root = state.remote.as_ref().unwrap().root.clone();
        state.remote.as_mut().unwrap().last_activity = Instant::now() - Duration::from_secs(61);
        assert!(state.expire().is_some());
        assert!(state.remote.is_none());
        assert!(!root.exists());
    }

    #[test]
    fn cache_cleanup_removes_expired_directories_only() {
        let root =
            std::env::temp_dir().join(format!("nyaterm-clip-cleanup-{}", uuid::Uuid::new_v4()));
        let stale = root.join("stale");
        fs::create_dir_all(&stale).unwrap();
        let unrelated_file = root.join("keep.txt");
        fs::write(&unrelated_file, b"keep").unwrap();
        cleanup_stale_cache(&root, Duration::ZERO);
        assert!(!stale.exists());
        assert!(unrelated_file.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
