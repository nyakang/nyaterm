//! Native TRZSZ upload/download execution with bounded worker events.
use crate::protocol_transfer::inbox::TransferInbox;
use crate::protocol_transfer::wire::TransferWire;
use crate::trzsz::{
    TrzszDownloadEngine, TrzszDownloadEvent, TrzszProtocolFrame, TrzszProtocolStream,
    TrzszTransferEvent, TrzszTransferState, TrzszUploadEngine, TrzszUploadEntry, TrzszUploadEvent,
    TrzszUploadPayload, TrzszUploadSource, build_trzsz_string_frame, trzsz_fail_response,
};
use std::{
    collections::{HashMap, HashSet},
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{SystemTime, UNIX_EPOCH},
};
struct TrzszDownloadRuntime {
    engine: TrzszDownloadEngine,
    directory: PathBuf,
    directory_roots: HashMap<i64, String>,
    pending_path: Option<TrzszDownloadPath>,
    current_file: Option<TrzszDownloadFile>,
}

#[derive(Debug, Clone)]
struct TrzszDownloadPath {
    path_id: i64,
    components: Vec<String>,
}

struct TrzszDownloadFile {
    name: String,
    path: PathBuf,
    file: File,
    size: u64,
}

pub struct TrzszDownloadWorker {
    wire: TransferWire,
    command_tx: TransferInbox<TrzszDownloadWorkerCommand>,
    event_rx: Option<mpsc::Receiver<TrzszDownloadWorkerEvent>>,
    worker: Option<thread::JoinHandle<()>>,
}

enum TrzszDownloadWorkerCommand {
    Output(Vec<u8>),
}

#[derive(Default)]
pub struct TrzszDownloadWorkerEvent {
    // Ownership returns with the final event so queued terminal bytes cannot
    // disappear when the worker exits before the UI consumes completion.
    unconsumed: Option<mpsc::Receiver<TrzszDownloadWorkerCommand>>,
    pub passthrough: Vec<u8>,
    pub responses: Vec<Vec<u8>>,
    pub progress: Vec<TrzszDownloadProgressUpdate>,
    pub status: Option<String>,
    pub completed: Option<String>,
    pub failed: Option<String>,
}

pub struct TrzszDownloadProgressUpdate {
    pub file_name: String,
    pub local_path: PathBuf,
    pub bytes_transferred: u64,
    pub total_bytes: Option<u64>,
    pub completed: bool,
    pub fail_reason: Option<String>,
}

pub struct TrzszUploadRuntime {
    engine: TrzszUploadEngine,
    files: HashMap<String, TrzszUploadFile>,
    remote_names: HashMap<String, String>,
    directory_mode: bool,
}

pub struct TrzszUploadWorker {
    wire: TransferWire,
    command_tx: TransferInbox<TrzszUploadWorkerCommand>,
    event_rx: Option<mpsc::Receiver<TrzszUploadWorkerEvent>>,
    worker: Option<thread::JoinHandle<()>>,
}

enum TrzszUploadWorkerCommand {
    Begin,
    Frame(TrzszProtocolFrame),
}

#[derive(Default)]
pub struct TrzszUploadWorkerEvent {
    pub responses: Vec<Vec<u8>>,
    pub progress: Vec<TrzszUploadProgressUpdate>,
    pub status: Option<String>,
    pub completed: Option<String>,
    pub failed: Option<String>,
}

pub struct TrzszUploadFile {
    local_path: PathBuf,
    size: u64,
    is_dir: bool,
}

pub struct TrzszUploadProgressUpdate {
    pub file_name: String,
    pub remote_name: String,
    pub local_path: PathBuf,
    pub bytes_transferred: u64,
    pub total_bytes: Option<u64>,
    pub completed: bool,
    pub fail_reason: Option<String>,
}

pub struct TrzszUploadPrepareWorker {
    event_rx: Option<mpsc::Receiver<TrzszUploadPrepareEvent>>,
    worker: Option<thread::JoinHandle<()>>,
}

pub struct TrzszUploadPrepareEvent {
    pub remote_is_windows: bool,
    pub directory_mode: bool,
    pub result: Result<(Vec<TrzszUploadEntry>, HashMap<String, TrzszUploadFile>), String>,
}

impl TrzszDownloadWorker {
    #[cfg(test)]
    fn spawn(download: TrzszDownloadRuntime, remote_is_windows: bool) -> Self {
        Self::spawn_runtime_connected(download, remote_is_windows, TransferWire::default())
    }

    fn spawn_runtime_connected(
        download: TrzszDownloadRuntime,
        remote_is_windows: bool,
        wire: TransferWire,
    ) -> Self {
        let worker_wire = wire.clone();
        let (command_tx, command_rx) = TransferInbox::new(8);
        let (event_tx, event_rx) = mpsc::sync_channel(TRZSZ_DOWNLOAD_WORKER_EVENT_CHANNEL_CAP);
        let worker = thread::Builder::new()
            .name("nyaterm-trzsz-download".to_string())
            .spawn(move || {
                run_trzsz_download_worker(
                    download,
                    remote_is_windows,
                    command_rx,
                    event_tx,
                    worker_wire,
                )
            })
            .expect("failed to spawn trzsz download worker");
        Self {
            wire,
            command_tx,
            event_rx: Some(event_rx),
            worker: Some(worker),
        }
    }

    pub fn send_output(&mut self, data: Vec<u8>) {
        if data.is_empty() {
            return;
        }
        self.command_tx
            .send(TrzszDownloadWorkerCommand::Output(data));
    }

    pub fn try_recv_event(&self) -> Option<TrzszDownloadWorkerEvent> {
        match self.event_rx.as_ref()?.try_recv() {
            Ok(event) => Some(event),
            Err(mpsc::TryRecvError::Empty) | Err(mpsc::TryRecvError::Disconnected) => None,
        }
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.wire.stop();
        self.event_rx.take();
        self.command_tx.close();
        self.worker.take();
    }
}

impl Drop for TrzszDownloadWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl TrzszUploadPrepareWorker {
    pub fn spawn(paths: Vec<PathBuf>, directory_mode: bool, remote_is_windows: bool) -> Self {
        let (event_tx, event_rx) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("nyaterm-trzsz-upload-prepare".to_string())
            .spawn(move || {
                let result = prepare_trzsz_upload_entries(paths, directory_mode);
                let _ = event_tx.send(TrzszUploadPrepareEvent {
                    remote_is_windows,
                    directory_mode,
                    result,
                });
            })
            .expect("failed to spawn trzsz upload prepare worker");
        Self {
            event_rx: Some(event_rx),
            worker: Some(worker),
        }
    }

    pub fn try_recv_event(&self) -> Option<TrzszUploadPrepareEvent> {
        match self.event_rx.as_ref()?.try_recv() {
            Ok(event) => Some(event),
            Err(mpsc::TryRecvError::Empty) | Err(mpsc::TryRecvError::Disconnected) => None,
        }
    }

    fn shutdown(&mut self) {
        self.event_rx.take();
        self.worker.take();
    }
}

impl Drop for TrzszUploadPrepareWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl TrzszUploadWorker {
    pub fn spawn_connected(
        upload: TrzszUploadRuntime,
        remote_is_windows: bool,
        wire: TransferWire,
    ) -> Self {
        let worker_wire = wire.clone();
        let (command_tx, command_rx) = TransferInbox::new(8);
        let (event_tx, event_rx) = mpsc::sync_channel(TRZSZ_UPLOAD_WORKER_EVENT_CHANNEL_CAP);
        let worker = thread::Builder::new()
            .name("nyaterm-trzsz-upload".to_string())
            .spawn(move || {
                run_trzsz_upload_worker(
                    upload,
                    remote_is_windows,
                    command_rx,
                    event_tx,
                    worker_wire,
                )
            })
            .expect("failed to spawn trzsz upload worker");
        Self {
            wire,
            command_tx,
            event_rx: Some(event_rx),
            worker: Some(worker),
        }
    }

    pub fn begin(&mut self) {
        self.command_tx.send(TrzszUploadWorkerCommand::Begin);
    }

    pub fn send_frame(&mut self, frame: TrzszProtocolFrame) {
        self.command_tx.send(TrzszUploadWorkerCommand::Frame(frame));
    }

    pub fn try_recv_event(&self) -> Option<TrzszUploadWorkerEvent> {
        match self.event_rx.as_ref()?.try_recv() {
            Ok(event) => Some(event),
            Err(mpsc::TryRecvError::Empty) | Err(mpsc::TryRecvError::Disconnected) => None,
        }
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.wire.stop();
        self.event_rx.take();
        self.command_tx.close();
        self.worker.take();
    }
}

impl Drop for TrzszUploadWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

enum TrzszDownloadRuntimeUpdate {
    None,
    Progress(TrzszDownloadProgressUpdate),
    Completed(Vec<String>),
}

enum TrzszUploadRuntimeUpdate {
    None,
    Progress(TrzszUploadProgressUpdate),
    Completed(Vec<String>),
}

fn run_trzsz_download_worker(
    mut download: TrzszDownloadRuntime,
    remote_is_windows: bool,
    command_rx: mpsc::Receiver<TrzszDownloadWorkerCommand>,
    event_tx: mpsc::SyncSender<TrzszDownloadWorkerEvent>,
    wire: TransferWire,
) {
    if let Err(error) = std::fs::create_dir_all(&download.directory) {
        let message = format!("failed to prepare trzsz download directory: {error}");
        let _ = event_tx.send(TrzszDownloadWorkerEvent {
            responses: vec![trzsz_fail_response(&message, remote_is_windows)],
            failed: Some(message),
            ..TrzszDownloadWorkerEvent::default()
        });
        return;
    }
    let mut protocol = TrzszProtocolStream::new();
    let mut transfer = TrzszTransferState::new();
    transfer.remote_is_windows = remote_is_windows;
    while let Ok(command) = command_rx.recv() {
        if wire.is_stopped() {
            break;
        }
        match command {
            TrzszDownloadWorkerCommand::Output(data) => {
                let mut event = process_trzsz_download_worker_output(
                    &mut download,
                    &mut protocol,
                    &mut transfer,
                    data,
                    &wire,
                );
                let mut remaining = Vec::new();
                for response in std::mem::take(&mut event.responses) {
                    match wire.send(&response) {
                        Ok(true) => {}
                        Ok(false) => remaining.push(response),
                        Err(error) => {
                            event.failed = Some(error);
                            event.completed = None;
                            event.progress.clear();
                            break;
                        }
                    }
                }
                event.responses = remaining;
                let done = event.completed.is_some() || event.failed.is_some();
                if done {
                    event.unconsumed = Some(command_rx);
                    let _ = event_tx.send(event);
                    break;
                }
                if event_tx.send(event).is_err() {
                    break;
                }
            }
        }
    }
}

fn process_trzsz_download_worker_output(
    download: &mut TrzszDownloadRuntime,
    protocol: &mut TrzszProtocolStream,
    transfer: &mut TrzszTransferState,
    data: Vec<u8>,
    wire: &TransferWire,
) -> TrzszDownloadWorkerEvent {
    let protocol_output = protocol.filter_terminal_output(&data);
    let mut event = TrzszDownloadWorkerEvent {
        passthrough: protocol_output.passthrough,
        ..TrzszDownloadWorkerEvent::default()
    };

    for frame in protocol_output.frames {
        if let Some(done) = process_trzsz_download_worker_frame_cancellable(
            download, transfer, frame, &mut event, wire,
        ) {
            event.failed = done.failed;
            event.completed = done.completed;
            break;
        }
    }
    event
}

#[cfg(test)]
fn process_trzsz_download_worker_frame(
    download: &mut TrzszDownloadRuntime,
    transfer: &mut TrzszTransferState,
    frame: TrzszProtocolFrame,
    event: &mut TrzszDownloadWorkerEvent,
) -> Option<TrzszDownloadWorkerEvent> {
    process_trzsz_download_worker_frame_cancellable(
        download,
        transfer,
        frame,
        event,
        &TransferWire::default(),
    )
}

fn process_trzsz_download_worker_frame_cancellable(
    download: &mut TrzszDownloadRuntime,
    transfer: &mut TrzszTransferState,
    frame: TrzszProtocolFrame,
    event: &mut TrzszDownloadWorkerEvent,
    wire: &TransferWire,
) -> Option<TrzszDownloadWorkerEvent> {
    match transfer.observe_frame(frame.clone()) {
        TrzszTransferEvent::Config { config } => {
            if let Err(error) = download.engine.configure(&config) {
                event
                    .responses
                    .push(trzsz_fail_response(&error, transfer.remote_is_windows));
                return Some(TrzszDownloadWorkerEvent {
                    failed: Some(error),
                    ..Default::default()
                });
            }
            if config.directory {
                event.status = Some("trzsz directory download accepted".to_string());
            }
        }
        TrzszTransferEvent::Failure { message } | TrzszTransferEvent::Exit { message } => {
            return Some(TrzszDownloadWorkerEvent {
                failed: Some(message),
                ..TrzszDownloadWorkerEvent::default()
            });
        }
        _ => {}
    }

    if !is_trzsz_download_frame(&frame) {
        return None;
    }
    // Keep the protocol engine separate from the filesystem sink so each decoded
    // block is committed immediately; a compressed DATA frame cannot accumulate
    // an entire file in the GPUI event queue.
    let mut engine = std::mem::replace(
        &mut download.engine,
        TrzszDownloadEngine::new(transfer.remote_is_windows),
    );
    let result = engine.observe_frame_into(frame, |transfer_event| {
        if wire.is_stopped() {
            return Err("trzsz download cancelled".into());
        }
        match apply_trzsz_download_event(download, transfer_event)? {
            TrzszDownloadRuntimeUpdate::None => {}
            TrzszDownloadRuntimeUpdate::Progress(update) => {
                if let Some(previous) = event.progress.last_mut()
                    && previous.local_path == update.local_path
                    && !previous.completed
                {
                    *previous = update;
                } else {
                    event.progress.push(update);
                }
            }
            TrzszDownloadRuntimeUpdate::Completed(names) => {
                event.completed = Some(if names.is_empty() {
                    "trzsz download complete".into()
                } else {
                    format!("Saved {}", names.join(", "))
                });
            }
        }
        Ok(())
    });
    download.engine = engine;
    match result {
        Ok(step) => event.responses.extend(step.responses),
        Err(error) => {
            event
                .responses
                .push(trzsz_fail_response(&error, transfer.remote_is_windows));
            return Some(TrzszDownloadWorkerEvent {
                failed: Some(error),
                ..Default::default()
            });
        }
    }
    if let Some(message) = event.completed.take() {
        let newline = if transfer.remote_is_windows {
            "!\n"
        } else {
            "\n"
        };
        event.responses.push(build_trzsz_string_frame(
            "EXIT",
            message.as_bytes(),
            newline,
        ));
        return Some(TrzszDownloadWorkerEvent {
            completed: Some(message),
            ..Default::default()
        });
    }
    None
}

fn run_trzsz_upload_worker(
    mut upload: TrzszUploadRuntime,
    remote_is_windows: bool,
    command_rx: mpsc::Receiver<TrzszUploadWorkerCommand>,
    event_tx: mpsc::SyncSender<TrzszUploadWorkerEvent>,
    wire: TransferWire,
) {
    while let Ok(command) = command_rx.recv() {
        if wire.is_stopped() {
            break;
        }
        let mut event = match command {
            TrzszUploadWorkerCommand::Begin => {
                process_trzsz_upload_worker_begin(&mut upload, remote_is_windows)
            }
            TrzszUploadWorkerCommand::Frame(frame) => {
                process_trzsz_upload_worker_frame(&mut upload, remote_is_windows, frame)
            }
        };
        let mut remaining = Vec::new();
        for response in std::mem::take(&mut event.responses) {
            match wire.send(&response) {
                Ok(true) => {}
                Ok(false) => remaining.push(response),
                Err(error) => {
                    event.failed = Some(error);
                    event.completed = None;
                    event.progress.clear();
                    break;
                }
            }
        }
        event.responses = remaining;
        let done = event.completed.is_some() || event.failed.is_some();
        if event_tx.send(event).is_err() {
            break;
        }
        if done {
            break;
        }
    }
}

fn process_trzsz_upload_worker_begin(
    upload: &mut TrzszUploadRuntime,
    remote_is_windows: bool,
) -> TrzszUploadWorkerEvent {
    let mut event = TrzszUploadWorkerEvent::default();
    match upload.engine.begin() {
        Ok(step) => {
            event.responses.extend(step.responses);
            let mut count = 0;
            for upload_event in step.events {
                match upload_event {
                    TrzszUploadEvent::Started { count: started } => {
                        count = started;
                    }
                    other => match apply_trzsz_upload_event(upload, other) {
                        TrzszUploadRuntimeUpdate::None => {}
                        TrzszUploadRuntimeUpdate::Progress(update) => {
                            event.progress.push(update);
                        }
                        TrzszUploadRuntimeUpdate::Completed(names) => {
                            return complete_trzsz_upload_worker_event(
                                upload,
                                names,
                                remote_is_windows,
                                event,
                            );
                        }
                    },
                }
            }
            event.status = Some(format!("trzsz upload started ({count} file(s))"));
        }
        Err(error) => {
            let reason = format!("{error:?}");
            event
                .responses
                .push(trzsz_fail_response(&reason, remote_is_windows));
            event.failed = Some(reason);
        }
    }
    event
}

fn process_trzsz_upload_worker_frame(
    upload: &mut TrzszUploadRuntime,
    remote_is_windows: bool,
    frame: TrzszProtocolFrame,
) -> TrzszUploadWorkerEvent {
    let mut event = TrzszUploadWorkerEvent::default();
    if !is_trzsz_upload_frame(&frame) {
        return event;
    }
    match upload.engine.observe_frame(frame) {
        Ok(step) => {
            event.responses.extend(step.responses);
            for upload_event in step.events {
                match apply_trzsz_upload_event(upload, upload_event) {
                    TrzszUploadRuntimeUpdate::None => {}
                    TrzszUploadRuntimeUpdate::Progress(update) => {
                        event.progress.push(update);
                    }
                    TrzszUploadRuntimeUpdate::Completed(names) => {
                        return complete_trzsz_upload_worker_event(
                            upload,
                            names,
                            remote_is_windows,
                            event,
                        );
                    }
                }
            }
        }
        Err(error) => {
            let reason = format!("{error:?}");
            event
                .responses
                .push(trzsz_fail_response(&reason, remote_is_windows));
            event.failed = Some(reason);
        }
    }
    event
}

fn complete_trzsz_upload_worker_event(
    upload: &TrzszUploadRuntime,
    names: Vec<String>,
    remote_is_windows: bool,
    mut event: TrzszUploadWorkerEvent,
) -> TrzszUploadWorkerEvent {
    // Archive members do not have individual wire ACKs. Complete their rows
    // only after the archive's MD5 has been acknowledged by the peer.
    for (name, file) in &upload.files {
        if event
            .progress
            .iter()
            .any(|update| update.local_path == file.local_path && update.completed)
        {
            continue;
        }
        event.progress.push(TrzszUploadProgressUpdate {
            file_name: name.clone(),
            remote_name: upload
                .remote_names
                .get(name)
                .cloned()
                .unwrap_or_else(|| name.clone()),
            local_path: file.local_path.clone(),
            bytes_transferred: file.size,
            total_bytes: Some(file.size),
            completed: true,
            fail_reason: None,
        });
    }
    let message = if names.is_empty() {
        "trzsz upload complete".to_string()
    } else {
        format!("Uploaded {}", names.join(", "))
    };
    let newline = if remote_is_windows { "!\n" } else { "\n" };
    event.responses.push(build_trzsz_string_frame(
        "EXIT",
        message.as_bytes(),
        newline,
    ));
    event.completed = Some(message);
    event
}

fn is_trzsz_download_frame(frame: &TrzszProtocolFrame) -> bool {
    matches!(
        frame.frame_type.to_ascii_uppercase().as_str(),
        "NUM" | "NAME" | "SIZE" | "DATA" | "MD5" | "COMP"
    )
}

pub fn is_trzsz_upload_frame(frame: &TrzszProtocolFrame) -> bool {
    frame.frame_type.eq_ignore_ascii_case("SUCC")
}

fn prepare_trzsz_upload_entries(
    paths: Vec<PathBuf>,
    directory_mode: bool,
) -> Result<(Vec<TrzszUploadEntry>, HashMap<String, TrzszUploadFile>), String> {
    if paths.is_empty() {
        return Err("trzsz upload selection cancelled".to_string());
    }

    let mut used_names = HashSet::new();
    let mut entries = Vec::new();
    let mut files = HashMap::new();
    for (path_id, path) in paths.into_iter().enumerate() {
        let metadata = path
            .metadata()
            .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
        if directory_mode {
            let root_name = unique_trzsz_upload_name(&path, &mut used_names);
            let mut visited_dirs = HashSet::new();
            append_trzsz_upload_path(
                path_id as i64,
                &path,
                metadata,
                vec![root_name],
                &mut visited_dirs,
                &mut entries,
                &mut files,
            )?;
            continue;
        }

        if metadata.is_dir() {
            return Err(format!(
                "trzsz upload path is a directory: {}",
                path.display()
            ));
        }
        if !metadata.is_file() {
            return Err(format!(
                "trzsz upload path is not a file: {}",
                path.display()
            ));
        }
        let name = unique_trzsz_upload_name(&path, &mut used_names);
        let size = trzsz_upload_size(&path, metadata.len())?;
        entries.push(TrzszUploadEntry::from_file(
            name.clone(),
            path.clone(),
            size,
        ));
        files.insert(
            name,
            TrzszUploadFile {
                local_path: path,
                size: size as u64,
                is_dir: false,
            },
        );
    }

    if entries.is_empty() {
        Err("trzsz upload selection cancelled".to_string())
    } else {
        Ok((entries, files))
    }
}

fn append_trzsz_upload_path(
    path_id: i64,
    path: &Path,
    metadata: std::fs::Metadata,
    components: Vec<String>,
    visited_dirs: &mut HashSet<PathBuf>,
    entries: &mut Vec<TrzszUploadEntry>,
    files: &mut HashMap<String, TrzszUploadFile>,
) -> Result<(), String> {
    let entry_name = components.join("/");
    let perm = trzsz_upload_perm(&metadata);
    if metadata.is_dir() {
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("failed to resolve {}: {error}", path.display()))?;
        if !visited_dirs.insert(canonical) {
            return Err(format!("trzsz upload directory cycle: {}", path.display()));
        }
        entries.push(TrzszUploadEntry {
            name: entry_name.clone(),
            size: 0,
            payload: TrzszUploadPayload::Memory(Vec::new()),
            source: Some(TrzszUploadSource {
                archive: false,
                path_id,
                path_name: components.clone(),
                is_dir: true,
                size: 0,
                perm,
            }),
        });
        files.insert(
            entry_name,
            TrzszUploadFile {
                local_path: path.to_path_buf(),
                size: 0,
                is_dir: true,
            },
        );

        let mut children = std::fs::read_dir(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            let child_path = child.path();
            let child_metadata = child
                .metadata()
                .map_err(|error| format!("failed to inspect {}: {error}", child_path.display()))?;
            let mut child_components = components.clone();
            child_components.push(safe_trzsz_upload_file_name(
                &child.file_name().to_string_lossy(),
            ));
            append_trzsz_upload_path(
                path_id,
                &child_path,
                child_metadata,
                child_components,
                visited_dirs,
                entries,
                files,
            )?;
        }
        return Ok(());
    }

    if !metadata.is_file() {
        return Err(format!(
            "trzsz upload path is not a file: {}",
            path.display()
        ));
    }

    let size = trzsz_upload_size(path, metadata.len())?;
    let mut entry = TrzszUploadEntry::from_file(entry_name.clone(), path.to_path_buf(), size);
    entry.source = Some(TrzszUploadSource {
        archive: false,
        path_id,
        path_name: components,
        is_dir: false,
        size,
        perm,
    });
    entries.push(entry);
    files.insert(
        entry_name,
        TrzszUploadFile {
            local_path: path.to_path_buf(),
            size: size as u64,
            is_dir: false,
        },
    );
    Ok(())
}

#[cfg(unix)]
fn trzsz_upload_perm(metadata: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt as _;
    Some(metadata.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn trzsz_upload_perm(_metadata: &std::fs::Metadata) -> Option<u32> {
    None
}

fn trzsz_upload_size(path: &Path, size: u64) -> Result<i64, String> {
    i64::try_from(size).map_err(|_| format!("trzsz upload file is too large: {}", path.display()))
}

fn unique_trzsz_upload_name(path: &Path, used_names: &mut HashSet<String>) -> String {
    let base = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("trzsz-upload");
    let base = safe_trzsz_upload_file_name(base);
    if used_names.insert(base.clone()) {
        return base;
    }

    let path = Path::new(&base);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("trzsz-upload");
    let extension = path.extension().and_then(|value| value.to_str());
    for index in 1..10_000 {
        let candidate = if let Some(extension) = extension {
            format!("{stem} ({index}).{extension}")
        } else {
            format!("{stem} ({index})")
        };
        if used_names.insert(candidate.clone()) {
            return candidate;
        }
    }

    let suffix = used_names.len();
    let candidate = format!("{stem} ({suffix})");
    used_names.insert(candidate.clone());
    candidate
}

fn safe_trzsz_upload_file_name(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | '\0' => '_',
            _ => ch,
        })
        .collect::<String>();
    if sanitized.trim().is_empty() {
        "trzsz-upload".to_string()
    } else {
        sanitized
    }
}

fn apply_trzsz_upload_event(
    upload: &mut TrzszUploadRuntime,
    event: TrzszUploadEvent,
) -> TrzszUploadRuntimeUpdate {
    match event {
        TrzszUploadEvent::Started { .. } => TrzszUploadRuntimeUpdate::None,
        TrzszUploadEvent::Directory { name, remote_name } => {
            let Some(file) = upload.files.get(&name) else {
                return TrzszUploadRuntimeUpdate::None;
            };
            debug_assert!(file.is_dir);
            TrzszUploadRuntimeUpdate::Progress(TrzszUploadProgressUpdate {
                file_name: name,
                remote_name,
                local_path: file.local_path.clone(),
                bytes_transferred: 0,
                total_bytes: Some(0),
                completed: true,
                fail_reason: None,
            })
        }
        TrzszUploadEvent::FileStarted {
            name,
            remote_name,
            size,
        } => {
            upload
                .remote_names
                .insert(name.clone(), remote_name.clone());
            let Some(file) = upload.files.get(&name) else {
                return TrzszUploadRuntimeUpdate::None;
            };
            // Protocol 4 archives report aggregate progress for a directory root.
            TrzszUploadRuntimeUpdate::Progress(TrzszUploadProgressUpdate {
                file_name: name,
                remote_name,
                local_path: file.local_path.clone(),
                bytes_transferred: 0,
                total_bytes: (size > 0).then_some(size as u64),
                completed: false,
                fail_reason: None,
            })
        }
        TrzszUploadEvent::Data { name, sent, size } => {
            let Some(file) = upload.files.get(&name) else {
                return TrzszUploadRuntimeUpdate::None;
            };
            TrzszUploadRuntimeUpdate::Progress(TrzszUploadProgressUpdate {
                remote_name: upload
                    .remote_names
                    .get(&name)
                    .cloned()
                    .unwrap_or_else(|| name.clone()),
                file_name: name,
                local_path: file.local_path.clone(),
                bytes_transferred: sent.max(0) as u64,
                total_bytes: (size > 0).then_some(size as u64),
                completed: false,
                fail_reason: None,
            })
        }
        TrzszUploadEvent::FileFinished { name, .. } => {
            let Some(file) = upload.files.get(&name) else {
                return TrzszUploadRuntimeUpdate::None;
            };
            TrzszUploadRuntimeUpdate::Progress(TrzszUploadProgressUpdate {
                remote_name: upload
                    .remote_names
                    .get(&name)
                    .cloned()
                    .unwrap_or_else(|| name.clone()),
                file_name: name,
                local_path: file.local_path.clone(),
                bytes_transferred: file.size,
                total_bytes: Some(file.size),
                completed: true,
                fail_reason: None,
            })
        }
        TrzszUploadEvent::Completed { names } => TrzszUploadRuntimeUpdate::Completed(names),
    }
}

fn apply_trzsz_download_event(
    download: &mut TrzszDownloadRuntime,
    event: TrzszDownloadEvent,
) -> Result<TrzszDownloadRuntimeUpdate, String> {
    match event {
        TrzszDownloadEvent::FileCount { .. } | TrzszDownloadEvent::FileName { .. } => {
            Ok(TrzszDownloadRuntimeUpdate::None)
        }
        TrzszDownloadEvent::FilePath {
            path_id,
            components,
            ..
        } => {
            download.pending_path = Some(TrzszDownloadPath {
                path_id,
                components,
            });
            Ok(TrzszDownloadRuntimeUpdate::None)
        }
        TrzszDownloadEvent::Directory {
            name,
            path_id,
            components,
        } => {
            let path = trzsz_directory_download_path(download, path_id, &components)?;
            std::fs::create_dir_all(&path)
                .map_err(|error| format!("failed to create {}: {error}", path.display()))?;
            Ok(TrzszDownloadRuntimeUpdate::Progress(
                TrzszDownloadProgressUpdate {
                    file_name: safe_trzsz_file_name(&name),
                    local_path: path,
                    bytes_transferred: 0,
                    total_bytes: Some(0),
                    completed: true,
                    fail_reason: None,
                },
            ))
        }
        TrzszDownloadEvent::FileSize { name, size } => {
            let pending_path = download.pending_path.take();
            let safe_name = safe_trzsz_file_name(&name);
            let path = if let Some(path_meta) = pending_path {
                trzsz_file_download_path(download, &path_meta)?
            } else {
                unique_trzsz_download_path(&download.directory, &safe_name)
            };
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
            }
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|error| format!("failed to create {}: {error}", path.display()))?;
            download.current_file = Some(TrzszDownloadFile {
                name: safe_name.clone(),
                path: path.clone(),
                file,
                size: size.max(0) as u64,
            });
            Ok(TrzszDownloadRuntimeUpdate::Progress(
                TrzszDownloadProgressUpdate {
                    file_name: safe_name,
                    local_path: path,
                    bytes_transferred: 0,
                    total_bytes: (size > 0).then_some(size as u64),
                    completed: false,
                    fail_reason: None,
                },
            ))
        }
        TrzszDownloadEvent::Data {
            bytes,
            received,
            size,
            ..
        } => {
            let Some(current) = download.current_file.as_mut() else {
                return Err("received trzsz data before opening a local file".to_string());
            };
            current
                .file
                .write_all(&bytes)
                .map_err(|error| format!("failed to write {}: {error}", current.path.display()))?;
            Ok(TrzszDownloadRuntimeUpdate::Progress(
                TrzszDownloadProgressUpdate {
                    file_name: current.name.clone(),
                    local_path: current.path.clone(),
                    bytes_transferred: received.max(0) as u64,
                    total_bytes: (size > 0).then_some(size as u64),
                    completed: false,
                    fail_reason: None,
                },
            ))
        }
        TrzszDownloadEvent::FileFinished { digest, .. } => {
            let Some(mut current) = download.current_file.take() else {
                return Ok(TrzszDownloadRuntimeUpdate::None);
            };
            current
                .file
                .flush()
                .map_err(|error| format!("failed to flush {}: {error}", current.path.display()))?;
            Ok(TrzszDownloadRuntimeUpdate::Progress(
                TrzszDownloadProgressUpdate {
                    file_name: current.name,
                    local_path: current.path,
                    bytes_transferred: current.size,
                    total_bytes: Some(current.size),
                    // Archive members close before the enclosing MD5 is checked.
                    // Leave their jobs running until the transfer succeeds so a
                    // corrupt or truncated archive cannot appear verified.
                    completed: !digest.is_empty(),
                    fail_reason: None,
                },
            ))
        }
        TrzszDownloadEvent::Completed { names } => Ok(TrzszDownloadRuntimeUpdate::Completed(names)),
    }
}

fn safe_trzsz_file_name(name: &str) -> String {
    let base = Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("trzsz-download");
    let sanitized = base
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | '\0' => '_',
            _ => ch,
        })
        .collect::<String>();
    if sanitized.trim().is_empty() {
        "trzsz-download".to_string()
    } else {
        sanitized
    }
}

fn safe_trzsz_path_component(component: &str) -> String {
    let sanitized = component
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | '\0' => '_',
            _ => ch,
        })
        .collect::<String>();
    let trimmed = sanitized.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        "trzsz-download".to_string()
    } else {
        sanitized
    }
}

fn trzsz_directory_download_path(
    download: &mut TrzszDownloadRuntime,
    path_id: i64,
    components: &[String],
) -> Result<PathBuf, String> {
    trzsz_nested_download_path(download, path_id, components)
}

fn trzsz_file_download_path(
    download: &mut TrzszDownloadRuntime,
    path: &TrzszDownloadPath,
) -> Result<PathBuf, String> {
    trzsz_nested_download_path(download, path.path_id, &path.components)
}

fn trzsz_nested_download_path(
    download: &mut TrzszDownloadRuntime,
    path_id: i64,
    components: &[String],
) -> Result<PathBuf, String> {
    let Some(root) = components.first() else {
        return Err("received trzsz directory entry without a path".to_string());
    };
    let root_name = if let Some(root_name) = download.directory_roots.get(&path_id) {
        root_name.clone()
    } else {
        let safe_root = safe_trzsz_path_component(root);
        let root_path = unique_trzsz_download_path(&download.directory, &safe_root);
        let root_name = root_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("trzsz-download")
            .to_string();
        download.directory_roots.insert(path_id, root_name.clone());
        root_name
    };

    let mut path = download.directory.join(root_name);
    for component in components.iter().skip(1) {
        path.push(safe_trzsz_path_component(component));
    }
    Ok(path)
}

fn unique_trzsz_download_path(directory: &Path, file_name: &str) -> PathBuf {
    let initial = directory.join(file_name);
    if !initial.exists() {
        return initial;
    }
    let path = Path::new(file_name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("trzsz-download");
    let extension = path.extension().and_then(|value| value.to_str());
    for index in 1..10_000 {
        let candidate_name = if let Some(extension) = extension {
            format!("{stem} ({index}).{extension}")
        } else {
            format!("{stem} ({index})")
        };
        let candidate = directory.join(candidate_name);
        if !candidate.exists() {
            return candidate;
        }
    }
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    directory.join(format!("{stem} ({suffix})"))
}

const TRZSZ_DOWNLOAD_WORKER_EVENT_CHANNEL_CAP: usize = 256;

const TRZSZ_UPLOAD_WORKER_EVENT_CHANNEL_CAP: usize = 256;

impl TrzszDownloadWorker {
    pub fn spawn_connected(
        directory: PathBuf,
        remote_is_windows: bool,
        wire: TransferWire,
    ) -> Self {
        Self::spawn_runtime_connected(
            TrzszDownloadRuntime {
                engine: TrzszDownloadEngine::new(remote_is_windows),
                directory,
                directory_roots: HashMap::new(),
                pending_path: None,
                current_file: None,
            },
            remote_is_windows,
            wire,
        )
    }
    pub fn ready(&mut self) -> bool {
        self.command_tx.ready()
    }
}
impl TrzszUploadWorker {
    pub fn ready(&mut self) -> bool {
        self.command_tx.ready()
    }
}
impl TrzszUploadRuntime {
    pub fn new(
        remote_is_windows: bool,
        directory_mode: bool,
        entries: Vec<TrzszUploadEntry>,
        files: HashMap<String, TrzszUploadFile>,
    ) -> Self {
        Self {
            engine: TrzszUploadEngine::new(remote_is_windows, entries),
            files,
            remote_names: HashMap::new(),
            directory_mode,
        }
    }
    pub fn configure(&mut self, config: &crate::trzsz::TrzszConfig) -> Result<(), String> {
        self.engine.configure(config)
    }
    pub fn directory_mode(&self) -> bool {
        self.directory_mode
    }
}
impl TrzszDownloadWorkerEvent {
    /// Return queued post-transfer bytes before resuming ordinary terminal routing.
    pub fn take_trailing_output(&mut self, worker: Option<&mut TrzszDownloadWorker>) -> Vec<u8> {
        let mut trailing = Vec::new();
        if self.completed.is_some() {
            if let Some(receiver) = self.unconsumed.take() {
                for TrzszDownloadWorkerCommand::Output(bytes) in receiver.try_iter() {
                    trailing.extend(bytes);
                }
            }
            if let Some(worker) = worker {
                for TrzszDownloadWorkerCommand::Output(bytes) in worker.command_tx.take_deferred() {
                    trailing.extend(bytes);
                }
            }
        }
        trailing
    }
}
#[cfg(test)]
mod tests {
    use super::{
        TrzszDownloadRuntime, TrzszDownloadRuntimeUpdate, TrzszDownloadWorker,
        TrzszDownloadWorkerEvent, TrzszUploadPrepareWorker, TrzszUploadRuntime,
        apply_trzsz_download_event, process_trzsz_download_worker_frame,
        process_trzsz_upload_worker_begin,
    };
    use crate::trzsz::{
        TrzszDownloadEngine, TrzszDownloadEvent, TrzszProtocolFrame, TrzszProtocolPayload,
        TrzszTransferState, TrzszUploadEngine, TrzszUploadEntry, TrzszUploadPayload,
    };
    use std::collections::HashMap;
    use std::time::Duration;
    #[test]
    fn download_worker_reports_directory_preparation_failure_through_protocol() {
        let root = unique_test_dir("trzsz-invalid-directory");
        std::fs::create_dir_all(root.path()).unwrap();
        let directory = root.path().join("file");
        std::fs::write(&directory, b"existing file").unwrap();
        let mut worker = TrzszDownloadWorker::spawn(
            TrzszDownloadRuntime {
                engine: TrzszDownloadEngine::new(false),
                directory,
                directory_roots: HashMap::new(),
                pending_path: None,
                current_file: None,
            },
            false,
        );
        let event = worker
            .event_rx
            .as_ref()
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert!(
            event
                .failed
                .unwrap()
                .contains("prepare trzsz download directory")
        );
        assert_eq!(event.responses.len(), 1);
        assert!(event.responses[0].starts_with(b"#fail:"));
        worker.shutdown();
    }
    #[test]
    fn completed_download_returns_its_mailbox_so_late_terminal_output_is_preserved() {
        let root = unique_test_dir("trzsz-complete-tail");
        let mut worker = TrzszDownloadWorker::spawn(
            TrzszDownloadRuntime {
                engine: TrzszDownloadEngine::new(false),
                directory: root.path().into(),
                directory_roots: HashMap::new(),
                pending_path: None,
                current_file: None,
            },
            false,
        );
        let mut wire = b"#NUM:1\n".to_vec();
        wire.extend(crate::trzsz::build_trzsz_string_frame(
            "NAME", b"empty", "\n",
        ));
        wire.extend(b"#SIZE:0\n");
        wire.extend(crate::trzsz::build_trzsz_string_frame(
            "MD5",
            &md5::compute([]).0,
            "\n",
        ));
        worker.send_output(wire);
        let mut event = worker
            .event_rx
            .as_ref()
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert!(event.completed.is_some());
        let mut expected = b"shell prompt$ ".to_vec();
        worker.send_output(expected.clone());
        // The completed worker returns its mailbox; exceed its capacity so the
        // caller must recover both queued and deferred bytes in source order.
        for index in 0..10 {
            let bytes = format!("late-{index} ").into_bytes();
            expected.extend(&bytes);
            worker.send_output(bytes);
        }
        assert_eq!(event.take_trailing_output(Some(&mut worker)), expected);
        assert!(event.take_trailing_output(Some(&mut worker)).is_empty());
        worker.shutdown();
    }
    #[test]
    fn archive_member_is_not_completed_before_the_enclosing_digest_is_verified() {
        let directory = unique_test_dir("trzsz-unverified-archive");
        let mut download = TrzszDownloadRuntime {
            engine: TrzszDownloadEngine::new(false),
            directory: directory.path().to_path_buf(),
            directory_roots: HashMap::new(),
            pending_path: None,
            current_file: None,
        };
        apply_trzsz_download_event(
            &mut download,
            TrzszDownloadEvent::FileSize {
                name: "empty".into(),
                size: 0,
            },
        )
        .unwrap();
        let update = apply_trzsz_download_event(
            &mut download,
            TrzszDownloadEvent::FileFinished {
                name: "empty".into(),
                digest: Vec::new(),
            },
        )
        .unwrap();
        let TrzszDownloadRuntimeUpdate::Progress(progress) = update else {
            panic!("expected progress")
        };
        assert!(!progress.completed);
        assert!(download.current_file.is_none());
        apply_trzsz_download_event(
            &mut download,
            TrzszDownloadEvent::FileSize {
                name: "verified".into(),
                size: 0,
            },
        )
        .unwrap();
        let update = apply_trzsz_download_event(
            &mut download,
            TrzszDownloadEvent::FileFinished {
                name: "verified".into(),
                digest: md5::compute(b"").0.to_vec(),
            },
        )
        .unwrap();
        let TrzszDownloadRuntimeUpdate::Progress(progress) = update else {
            panic!("expected progress")
        };
        assert!(progress.completed);
    }
    #[test]
    fn trzsz_download_worker_frame_path_writes_file_off_ui_state() {
        let directory = unique_test_dir("trzsz-worker-download");
        std::fs::create_dir_all(&directory).expect("test directory should be created");
        let mut download = TrzszDownloadRuntime {
            engine: TrzszDownloadEngine::new(false),
            directory: directory.path().to_path_buf(),
            directory_roots: HashMap::new(),
            pending_path: None,
            current_file: None,
        };
        let mut transfer = TrzszTransferState::new();
        transfer.remote_is_windows = false;

        let mut event = TrzszDownloadWorkerEvent::default();
        assert!(
            process_trzsz_download_worker_frame(
                &mut download,
                &mut transfer,
                frame("NUM", TrzszProtocolPayload::Integer(1)),
                &mut event,
            )
            .is_none()
        );
        assert_eq!(event.responses.len(), 1);

        let mut event = TrzszDownloadWorkerEvent::default();
        process_trzsz_download_worker_frame(
            &mut download,
            &mut transfer,
            frame(
                "NAME",
                TrzszProtocolPayload::EncodedBytes(b"hello.txt".to_vec()),
            ),
            &mut event,
        );
        assert_eq!(event.responses.len(), 1);

        let mut event = TrzszDownloadWorkerEvent::default();
        process_trzsz_download_worker_frame(
            &mut download,
            &mut transfer,
            frame("SIZE", TrzszProtocolPayload::Integer(5)),
            &mut event,
        );
        assert_eq!(event.progress.len(), 1);
        assert_eq!(event.progress[0].file_name, "hello.txt");

        let data = b"hello".to_vec();
        let mut event = TrzszDownloadWorkerEvent::default();
        process_trzsz_download_worker_frame(
            &mut download,
            &mut transfer,
            frame("DATA", TrzszProtocolPayload::EncodedBytes(data.clone())),
            &mut event,
        );
        assert_eq!(event.progress.len(), 1);
        assert_eq!(event.progress[0].bytes_transferred, 5);

        let digest = md5::compute(&data).0.to_vec();
        let mut event = TrzszDownloadWorkerEvent::default();
        let done = process_trzsz_download_worker_frame(
            &mut download,
            &mut transfer,
            frame("MD5", TrzszProtocolPayload::EncodedBytes(digest)),
            &mut event,
        )
        .expect("md5 should complete the download");
        assert_eq!(done.completed.as_deref(), Some("Saved hello.txt"));
        assert_eq!(
            std::fs::read(directory.join("hello.txt")).expect("download file should exist"),
            data
        );
    }
    #[test]
    fn trzsz_upload_prepare_worker_returns_entries_off_ui_state() {
        let directory = unique_test_dir("trzsz-worker-upload");
        std::fs::create_dir_all(&directory).expect("test directory should be created");
        let file_path = directory.join("hello.txt");
        std::fs::write(&file_path, b"hello").expect("test file should be written");

        let worker = TrzszUploadPrepareWorker::spawn(vec![file_path.clone()], false, false);
        let event = loop {
            if let Some(event) = worker.try_recv_event() {
                break event;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        let (entries, files) = event.result.expect("upload prepare should succeed");

        assert!(!event.remote_is_windows);
        assert!(!event.directory_mode);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "hello.txt");
        assert_eq!(
            files
                .get("hello.txt")
                .expect("file metadata should be tracked")
                .local_path,
            file_path
        );
    }
    #[test]
    fn trzsz_upload_worker_begin_emits_protocol_response_off_ui_state() {
        let mut upload = TrzszUploadRuntime {
            engine: TrzszUploadEngine::new(
                false,
                vec![TrzszUploadEntry {
                    name: "hello.txt".to_string(),
                    size: 5,
                    payload: TrzszUploadPayload::Memory(b"hello".to_vec()),
                    source: None,
                }],
            ),
            files: HashMap::new(),
            remote_names: HashMap::new(),
            directory_mode: false,
        };

        let event = process_trzsz_upload_worker_begin(&mut upload, false);

        assert!(event.failed.is_none());
        assert!(event.completed.is_none());
        assert!(event.progress.is_empty());
        assert_eq!(
            event.status.as_deref(),
            Some("trzsz upload started (1 file(s))")
        );
        assert_eq!(event.responses.len(), 1);
        assert!(
            String::from_utf8_lossy(&event.responses[0]).contains("#NUM:1"),
            "response={:?}",
            event.responses[0]
        );
    }
    fn frame(frame_type: &str, payload: TrzszProtocolPayload) -> TrzszProtocolFrame {
        TrzszProtocolFrame {
            frame_type: frame_type.to_string(),
            payload,
        }
    }
    fn unique_test_dir(name: &str) -> nyaterm_core::test_support::TestTempDir {
        nyaterm_core::test_support::TestTempDir::new(&format!("nyaterm-{name}"))
    }
}
