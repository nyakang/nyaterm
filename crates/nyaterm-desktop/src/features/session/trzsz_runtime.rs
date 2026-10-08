use crate::features::NyaTermApp;
use crate::features::formatting::short_id;
use crate::models::{TransferJobKind, TransferJobState, TransferJobStatus};
use crate::models::{TransferPathPromptKind, TransferPathPromptResult};
use gpui::{Context, PathPromptOptions, SharedString};
use nyaterm_transport::{
    SftpTransferProgress, TrzszAction, TrzszDetector, TrzszDownloadEngine, TrzszDownloadEvent,
    TrzszMode, TrzszOutputEvent, TrzszProtocolFrame, TrzszProtocolStream, TrzszTransferEvent,
    TrzszTransferState, TrzszTrigger, TrzszUploadEngine, TrzszUploadEntry, TrzszUploadEvent,
    TrzszUploadPayload, TrzszUploadSource, build_trzsz_action_frame, build_trzsz_string_frame,
    trzsz_fail_response,
};
use std::{
    collections::{HashMap, HashSet},
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

pub(super) struct TrzszSessionState {
    detector: TrzszDetector,
    transfer: TrzszTransferState,
    protocol: TrzszProtocolStream,
    protocol_active: bool,
    download: Option<TrzszDownloadRuntime>,
    download_worker: Option<TrzszDownloadWorker>,
    upload_prepare_worker: Option<TrzszUploadPrepareWorker>,
    upload: Option<TrzszUploadRuntime>,
    upload_worker: Option<TrzszUploadWorker>,
}

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

struct TrzszDownloadWorker {
    command_tx: mpsc::Sender<TrzszDownloadWorkerCommand>,
    event_rx: Option<mpsc::Receiver<TrzszDownloadWorkerEvent>>,
    worker: Option<thread::JoinHandle<()>>,
}

enum TrzszDownloadWorkerCommand {
    Output(Vec<u8>),
    Stop,
}

#[derive(Default)]
struct TrzszDownloadWorkerEvent {
    passthrough: Vec<u8>,
    responses: Vec<Vec<u8>>,
    progress: Vec<TrzszDownloadProgressUpdate>,
    status: Option<String>,
    completed: Option<String>,
    failed: Option<String>,
}

struct TrzszDownloadProgressUpdate {
    file_name: String,
    local_path: PathBuf,
    bytes_transferred: u64,
    total_bytes: Option<u64>,
    completed: bool,
    fail_reason: Option<String>,
}

struct TrzszUploadRuntime {
    engine: TrzszUploadEngine,
    files: HashMap<String, TrzszUploadFile>,
    remote_names: HashMap<String, String>,
    directory_mode: bool,
}

struct TrzszUploadWorker {
    command_tx: mpsc::Sender<TrzszUploadWorkerCommand>,
    event_rx: Option<mpsc::Receiver<TrzszUploadWorkerEvent>>,
    worker: Option<thread::JoinHandle<()>>,
}

enum TrzszUploadWorkerCommand {
    Begin,
    Frame(TrzszProtocolFrame),
    Stop,
}

#[derive(Default)]
struct TrzszUploadWorkerEvent {
    responses: Vec<Vec<u8>>,
    progress: Vec<TrzszUploadProgressUpdate>,
    status: Option<String>,
    completed: Option<String>,
    failed: Option<String>,
}

struct TrzszUploadFile {
    local_path: PathBuf,
    size: u64,
    is_dir: bool,
}

struct TrzszUploadProgressUpdate {
    file_name: String,
    remote_name: String,
    local_path: PathBuf,
    bytes_transferred: u64,
    total_bytes: Option<u64>,
    completed: bool,
    fail_reason: Option<String>,
}

struct TrzszUploadPrepareWorker {
    event_rx: Option<mpsc::Receiver<TrzszUploadPrepareEvent>>,
    worker: Option<thread::JoinHandle<()>>,
}

struct TrzszUploadPrepareEvent {
    remote_is_windows: bool,
    directory_mode: bool,
    result: Result<(Vec<TrzszUploadEntry>, HashMap<String, TrzszUploadFile>), String>,
}

impl TrzszDownloadWorker {
    fn spawn(download: TrzszDownloadRuntime, remote_is_windows: bool) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::sync_channel(TRZSZ_DOWNLOAD_WORKER_EVENT_CHANNEL_CAP);
        let worker = thread::Builder::new()
            .name("nyaterm-trzsz-download".to_string())
            .spawn(move || {
                run_trzsz_download_worker(download, remote_is_windows, command_rx, event_tx)
            })
            .expect("failed to spawn trzsz download worker");
        Self {
            command_tx,
            event_rx: Some(event_rx),
            worker: Some(worker),
        }
    }

    fn send_output(&self, data: Vec<u8>) {
        if data.is_empty() {
            return;
        }
        let _ = self
            .command_tx
            .send(TrzszDownloadWorkerCommand::Output(data));
    }

    fn try_recv_event(&self) -> Option<TrzszDownloadWorkerEvent> {
        match self.event_rx.as_ref()?.try_recv() {
            Ok(event) => Some(event),
            Err(mpsc::TryRecvError::Empty) | Err(mpsc::TryRecvError::Disconnected) => None,
        }
    }

    fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.event_rx.take();
        let _ = self.command_tx.send(TrzszDownloadWorkerCommand::Stop);
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::warn!("trzsz download worker panicked during shutdown");
        }
    }
}

impl Drop for TrzszDownloadWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl TrzszUploadPrepareWorker {
    fn spawn(paths: Vec<PathBuf>, directory_mode: bool, remote_is_windows: bool) -> Self {
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

    fn try_recv_event(&self) -> Option<TrzszUploadPrepareEvent> {
        match self.event_rx.as_ref()?.try_recv() {
            Ok(event) => Some(event),
            Err(mpsc::TryRecvError::Empty) | Err(mpsc::TryRecvError::Disconnected) => None,
        }
    }

    fn shutdown(&mut self) {
        self.event_rx.take();
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::warn!("trzsz upload preparation worker panicked during shutdown");
        }
    }
}

impl Drop for TrzszUploadPrepareWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl TrzszUploadWorker {
    fn spawn(upload: TrzszUploadRuntime, remote_is_windows: bool) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::sync_channel(TRZSZ_UPLOAD_WORKER_EVENT_CHANNEL_CAP);
        let worker = thread::Builder::new()
            .name("nyaterm-trzsz-upload".to_string())
            .spawn(move || run_trzsz_upload_worker(upload, remote_is_windows, command_rx, event_tx))
            .expect("failed to spawn trzsz upload worker");
        Self {
            command_tx,
            event_rx: Some(event_rx),
            worker: Some(worker),
        }
    }

    fn begin(&self) {
        let _ = self.command_tx.send(TrzszUploadWorkerCommand::Begin);
    }

    fn send_frame(&self, frame: TrzszProtocolFrame) {
        let _ = self.command_tx.send(TrzszUploadWorkerCommand::Frame(frame));
    }

    fn try_recv_event(&self) -> Option<TrzszUploadWorkerEvent> {
        match self.event_rx.as_ref()?.try_recv() {
            Ok(event) => Some(event),
            Err(mpsc::TryRecvError::Empty) | Err(mpsc::TryRecvError::Disconnected) => None,
        }
    }

    fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.event_rx.take();
        let _ = self.command_tx.send(TrzszUploadWorkerCommand::Stop);
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::warn!("trzsz upload worker panicked during shutdown");
        }
    }
}

impl Drop for TrzszUploadWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Default for TrzszSessionState {
    fn default() -> Self {
        Self {
            detector: TrzszDetector::new(),
            transfer: TrzszTransferState::new(),
            protocol: TrzszProtocolStream::new(),
            protocol_active: false,
            download: None,
            download_worker: None,
            upload_prepare_worker: None,
            upload: None,
            upload_worker: None,
        }
    }
}

impl TrzszSessionState {
    fn stop_download_worker(&mut self) {
        if let Some(worker) = self.download_worker.take() {
            worker.stop();
        }
    }

    fn stop_upload_worker(&mut self) {
        if let Some(worker) = self.upload_worker.take() {
            worker.stop();
        }
    }

    pub(super) fn stop_workers(&mut self) {
        self.stop_download_worker();
        self.upload_prepare_worker = None;
        self.stop_upload_worker();
    }

    fn reset_after_discontinuity(&mut self) {
        self.detector.reset();
        self.transfer = TrzszTransferState::new();
        self.protocol.reset();
        self.protocol_active = false;
        self.download = None;
        self.upload = None;
        self.stop_workers();
    }

    fn finish_download(&mut self) {
        self.download = None;
        self.stop_download_worker();
        self.protocol_active = false;
        self.protocol.reset();
    }

    fn finish_upload(&mut self) {
        self.upload = None;
        self.upload_prepare_worker = None;
        self.stop_upload_worker();
        self.protocol_active = false;
        self.protocol.reset();
    }

    fn finish_transfer(&mut self) {
        self.finish_download();
        self.finish_upload();
    }

    fn begin_trigger(&mut self, trigger: &TrzszTrigger) {
        self.finish_transfer();
        self.transfer.observe_trigger(trigger);
        self.protocol.reset();
        self.protocol_active = true;
    }

    fn begin_download(&mut self, trigger: &TrzszTrigger, directory: PathBuf) {
        self.begin_trigger(trigger);
        self.download_worker = Some(TrzszDownloadWorker::spawn(
            TrzszDownloadRuntime {
                engine: TrzszDownloadEngine::new(trigger.remote_is_windows),
                directory,
                directory_roots: HashMap::new(),
                pending_path: None,
                current_file: None,
            },
            trigger.remote_is_windows,
        ));
    }

    fn begin_upload_path_selection(&mut self, trigger: &TrzszTrigger) {
        self.begin_trigger(trigger);
    }

    fn begin_upload_preparation(
        &mut self,
        paths: Vec<PathBuf>,
        directory_mode: bool,
        remote_is_windows: bool,
    ) {
        self.finish_download();
        self.upload = None;
        self.stop_upload_worker();
        self.protocol_active = true;
        self.upload_prepare_worker = Some(TrzszUploadPrepareWorker::spawn(
            paths,
            directory_mode,
            remote_is_windows,
        ));
    }

    fn install_prepared_upload(
        &mut self,
        remote_is_windows: bool,
        directory_mode: bool,
        entries: Vec<TrzszUploadEntry>,
        files: HashMap<String, TrzszUploadFile>,
    ) {
        self.finish_download();
        self.upload_prepare_worker = None;
        self.stop_upload_worker();
        self.protocol_active = true;
        self.upload = Some(TrzszUploadRuntime {
            engine: TrzszUploadEngine::new(remote_is_windows, entries),
            files,
            remote_names: HashMap::new(),
            directory_mode,
        });
    }
}

impl Drop for TrzszSessionState {
    fn drop(&mut self) {
        self.stop_workers();
    }
}

impl NyaTermApp {
    fn trzsz_state_mut(&mut self, session_id: &str) -> &mut TrzszSessionState {
        self.session.trzsz_state_mut_or_default(session_id)
    }

    pub(in crate::features) fn clear_trzsz_session(&mut self, session_id: &str) {
        if self.session.remove_trzsz_session_runtime(session_id) {
            self.sync_session_event_bridge_session_policy(session_id);
        }
    }

    pub(in crate::features) fn note_trzsz_output_discontinuity(&mut self, session_id: &str) {
        if let Some(state) = self.session.trzsz_state_mut(session_id) {
            state.reset_after_discontinuity();
        }
    }

    /// Consume trzsz marker and protocol bytes before they reach the terminal
    /// parser. Remote `tsz` downloads are handled locally; unsupported upload
    /// modes are rejected with protocol-level failure frames.
    pub(in crate::features) fn process_trzsz_output(
        &mut self,
        session_id: &str,
        data: &[u8],
        cx: &mut Context<Self>,
    ) -> (Vec<u8>, bool) {
        if data.is_empty() {
            return (Vec::new(), false);
        }
        if self.trzsz_output_can_bypass_detector(session_id, data) {
            return (data.to_vec(), false);
        }
        let events = {
            let state = self.trzsz_state_mut(session_id);
            state.detector.scan_terminal_output(data).events
        };
        self.process_trzsz_output_events(session_id, events, cx)
    }

    /// Called by the existing sideband runtime drain even without new output.
    pub(in crate::features) fn drain_trzsz_idle_output(
        &mut self,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> (Vec<(String, Vec<u8>)>, bool) {
        let expired = self
            .session
            .trzsz_states_mut()
            .filter_map(|(session_id, state)| {
                let bytes = state.detector.flush_pending_if_idle(now);
                (!bytes.is_empty()).then(|| (session_id.clone(), bytes))
            })
            .collect::<Vec<_>>();
        let mut outputs = Vec::new();
        let mut root_chrome_dirty = false;
        for (session_id, bytes) in expired {
            if !self.session.has_session(&session_id)
                || self.terminal.session_id_is_retired(&session_id)
            {
                continue;
            }
            let (bytes, dirty) = self.process_trzsz_output_events(
                &session_id,
                vec![TrzszOutputEvent::Passthrough(bytes)],
                cx,
            );
            root_chrome_dirty |= dirty;
            if !bytes.is_empty() {
                outputs.push((session_id, bytes));
            }
        }
        (outputs, root_chrome_dirty)
    }

    fn process_trzsz_output_events(
        &mut self,
        session_id: &str,
        events: Vec<TrzszOutputEvent>,
        cx: &mut Context<Self>,
    ) -> (Vec<u8>, bool) {
        let mut passthrough = Vec::new();
        let mut protocol_responses = Vec::new();
        let mut latest_trigger_status = None;
        let mut latest_protocol_status = None;
        let mut response_error = false;
        let mut root_chrome_dirty = false;

        for event in events {
            match event {
                TrzszOutputEvent::Passthrough(bytes) => {
                    let mut protocol_status = None;
                    if self.queue_trzsz_download_worker_output(session_id, &bytes) {
                        continue;
                    }
                    let protocol_output = {
                        let state = self.trzsz_state_mut(session_id);
                        if !state.protocol_active {
                            passthrough.extend(bytes);
                            continue;
                        }

                        state.protocol.filter_terminal_output(&bytes)
                    };
                    for frame in protocol_output.frames.clone() {
                        self.handle_trzsz_protocol_frame(
                            session_id,
                            frame,
                            &mut protocol_responses,
                            &mut protocol_status,
                            cx,
                        );
                    }
                    passthrough.extend(protocol_output.passthrough);
                    if protocol_status.is_some() {
                        latest_protocol_status = protocol_status;
                    }
                }
                TrzszOutputEvent::Trigger(trigger) => {
                    let action = match trigger.mode {
                        TrzszMode::Send => "send",
                        TrzszMode::Receive => "receive",
                        TrzszMode::Directory => "directory",
                    };
                    let version = trigger.version.as_str();
                    let server = if trigger.remote_is_windows {
                        " Windows server"
                    } else {
                        ""
                    };
                    if trigger.mode == TrzszMode::Send {
                        let Some(directory) = self.prepare_trzsz_download_dir(cx) else {
                            protocol_responses.push(trzsz_fail_response(
                                "trzsz download directory is not available",
                                trigger.remote_is_windows,
                            ));
                            continue;
                        };
                        let action = TrzszAction::local_default(trigger.remote_is_windows);
                        let action_frame =
                            build_trzsz_action_frame(&action, trigger.remote_is_windows);
                        self.trzsz_state_mut(session_id)
                            .begin_download(&trigger, directory.clone());
                        protocol_responses.push(action_frame);
                        latest_trigger_status = Some(format!(
                            "trzsz download accepted (v{version}{server}) -> {}",
                            directory.display()
                        ));
                    } else if trigger.mode == TrzszMode::Receive {
                        self.trzsz_state_mut(session_id)
                            .begin_upload_path_selection(&trigger);
                        if self.prompt_trzsz_upload_paths(
                            session_id.to_string(),
                            trigger.remote_is_windows,
                            false,
                            cx,
                        ) {
                            latest_trigger_status = Some(format!(
                                "trzsz upload requested (v{version}{server}) - select local files"
                            ));
                        } else {
                            protocol_responses.push(trzsz_fail_response(
                                "trzsz upload file picker is not available",
                                trigger.remote_is_windows,
                            ));
                            if let Some(state) = self.session.trzsz_state_mut(session_id) {
                                state.finish_transfer();
                            }
                            latest_trigger_status = Some(format!(
                                "trzsz {action} trigger rejected (v{version}{server}) - file picker busy"
                            ));
                        }
                    } else {
                        self.trzsz_state_mut(session_id)
                            .begin_upload_path_selection(&trigger);
                        if self.prompt_trzsz_upload_paths(
                            session_id.to_string(),
                            trigger.remote_is_windows,
                            true,
                            cx,
                        ) {
                            latest_trigger_status = Some(format!(
                                "trzsz directory upload requested (v{version}{server}) - select local directories"
                            ));
                        } else {
                            protocol_responses.push(trzsz_fail_response(
                                "trzsz upload file picker is not available",
                                trigger.remote_is_windows,
                            ));
                            if let Some(state) = self.session.trzsz_state_mut(session_id) {
                                state.finish_transfer();
                            }
                            latest_trigger_status = Some(format!(
                                "trzsz {action} trigger rejected (v{version}{server}) - file picker busy"
                            ));
                        }
                    }
                }
            }
        }

        for response in protocol_responses {
            if let Err(error) = self.write_session_protocol_response(session_id, &response) {
                self.shell
                    .set_status(format!("trzsz protocol response failed: {error}"));
                response_error = true;
                root_chrome_dirty = true;
            }
        }

        if !response_error && let Some(status) = latest_protocol_status.or(latest_trigger_status) {
            self.shell.set_status(status);
            root_chrome_dirty = true;
        }
        (passthrough, root_chrome_dirty)
    }

    fn queue_trzsz_download_worker_output(&mut self, session_id: &str, data: &[u8]) -> bool {
        let Some(state) = self.session.trzsz_state_mut(session_id) else {
            return false;
        };
        let Some(worker) = state.download_worker.as_ref() else {
            return false;
        };
        if !data.is_empty() {
            worker.send_output(data.to_vec());
        }
        true
    }

    fn queue_trzsz_upload_worker_frame(
        &mut self,
        session_id: &str,
        frame: TrzszProtocolFrame,
    ) -> bool {
        let Some(state) = self.session.trzsz_state_mut(session_id) else {
            return false;
        };
        let Some(worker) = state.upload_worker.as_ref() else {
            return false;
        };
        worker.send_frame(frame);
        true
    }

    pub(in crate::features) fn drain_trzsz_download_worker_events(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.session.has_trzsz_runtime_sessions() {
            return false;
        }
        let mut events = Vec::new();
        for (session_id, state) in self.session.trzsz_states_mut() {
            let Some(worker) = state.download_worker.as_ref() else {
                continue;
            };
            while let Some(event) = worker.try_recv_event() {
                events.push((session_id.clone(), event));
                if events.len() >= TRZSZ_DOWNLOAD_WORKER_EVENT_DRAIN_BATCH {
                    break;
                }
            }
            if events.len() >= TRZSZ_DOWNLOAD_WORKER_EVENT_DRAIN_BATCH {
                break;
            }
        }
        if events.is_empty() {
            return false;
        }

        let mut root_chrome_dirty = false;
        for (session_id, event) in events {
            root_chrome_dirty |= self.apply_trzsz_download_worker_event(&session_id, event, cx);
        }
        root_chrome_dirty
    }

    fn apply_trzsz_download_worker_event(
        &mut self,
        session_id: &str,
        event: TrzszDownloadWorkerEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut root_chrome_dirty = false;
        if !event.passthrough.is_empty() {
            let writer = self.recording.writer();
            if !writer
                .capture_policy(session_id)
                .include_binary_transfer_payloads
            {
                writer.write_raw_output(session_id, &event.passthrough);
            }
            self.submit_terminal_frame_output(session_id, event.passthrough);
        }
        for response in event.responses {
            if let Err(error) = self.write_session_protocol_response(session_id, &response) {
                self.shell
                    .set_status(format!("trzsz protocol response failed: {error}"));
                root_chrome_dirty = true;
            }
        }
        for update in event.progress {
            self.update_trzsz_download_job(session_id, update, cx);
        }
        if let Some(reason) = event.failed {
            self.finish_trzsz_download_jobs(session_id, false, Some(&reason), cx);
            if let Some(state) = self.session.trzsz_state_mut(session_id) {
                state.finish_download();
            }
            self.shell
                .set_status(format!("trzsz download failed: {reason}"));
            root_chrome_dirty = true;
        } else if let Some(message) = event.completed {
            self.finish_trzsz_download_jobs(session_id, true, None, cx);
            if let Some(state) = self.session.trzsz_state_mut(session_id) {
                state.finish_download();
            }
            self.shell.set_status(message);
            root_chrome_dirty = true;
        } else if let Some(status) = event.status {
            self.shell.set_status(status);
            root_chrome_dirty = true;
        }
        root_chrome_dirty
    }

    pub(in crate::features) fn drain_trzsz_upload_worker_events(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.session.has_trzsz_runtime_sessions() {
            return false;
        }
        let mut events = Vec::new();
        for (session_id, state) in self.session.trzsz_states_mut() {
            let Some(worker) = state.upload_worker.as_ref() else {
                continue;
            };
            while let Some(event) = worker.try_recv_event() {
                events.push((session_id.clone(), event));
                if events.len() >= TRZSZ_UPLOAD_WORKER_EVENT_DRAIN_BATCH {
                    break;
                }
            }
            if events.len() >= TRZSZ_UPLOAD_WORKER_EVENT_DRAIN_BATCH {
                break;
            }
        }
        if events.is_empty() {
            return false;
        }

        let mut root_chrome_dirty = false;
        for (session_id, event) in events {
            root_chrome_dirty |= self.apply_trzsz_upload_worker_event(&session_id, event, cx);
        }
        root_chrome_dirty
    }

    fn apply_trzsz_upload_worker_event(
        &mut self,
        session_id: &str,
        event: TrzszUploadWorkerEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut root_chrome_dirty = false;
        for response in event.responses {
            if let Err(error) = self.write_session_protocol_response(session_id, &response) {
                self.shell
                    .set_status(format!("trzsz protocol response failed: {error}"));
                root_chrome_dirty = true;
            }
        }
        for update in event.progress {
            self.update_trzsz_upload_job(session_id, update, cx);
        }
        if let Some(reason) = event.failed {
            self.finish_trzsz_upload_jobs(session_id, false, Some(&reason), cx);
            if let Some(state) = self.session.trzsz_state_mut(session_id) {
                state.finish_upload();
            }
            self.shell
                .set_status(format!("trzsz upload failed: {reason}"));
            root_chrome_dirty = true;
        } else if let Some(message) = event.completed {
            self.finish_trzsz_upload_jobs(session_id, true, None, cx);
            if let Some(state) = self.session.trzsz_state_mut(session_id) {
                state.finish_upload();
            }
            self.shell.set_status(message);
            root_chrome_dirty = true;
        } else if let Some(status) = event.status {
            self.shell.set_status(status);
            root_chrome_dirty = true;
        }
        root_chrome_dirty
    }

    pub(in crate::features) fn trzsz_output_can_bypass_detector(
        &self,
        session_id: &str,
        data: &[u8],
    ) -> bool {
        let state_is_idle = self.session.trzsz_state(session_id).is_none_or(|state| {
            !state.protocol_active
                && state.download.is_none()
                && state.download_worker.is_none()
                && state.upload.is_none()
                && state.upload_worker.is_none()
                && state.detector.is_idle()
        });
        state_is_idle && !TrzszDetector::output_may_contain_trigger(data)
    }

    fn prepare_trzsz_download_dir(&mut self, cx: &mut Context<Self>) -> Option<PathBuf> {
        let Some(directory) = self.resolved_transfer_download_dir() else {
            self.shell
                .set_status("cannot determine trzsz download directory".to_string());
            cx.notify();
            return None;
        };
        Some(directory)
    }

    fn prompt_trzsz_upload_paths(
        &mut self,
        session_id: String,
        remote_is_windows: bool,
        directory_mode: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.transfer.path_prompt_is_open() {
            self.shell
                .set_status("native path picker is already open".to_string());
            cx.notify();
            return false;
        }

        let options = PathPromptOptions {
            files: !directory_mode,
            directories: directory_mode,
            multiple: true,
            prompt: Some(SharedString::from(if directory_mode {
                "Select trzsz upload directories"
            } else {
                "Select trzsz upload files"
            })),
        };
        let prompt_kind = if directory_mode {
            TransferPathPromptKind::UploadDirectory
        } else {
            TransferPathPromptKind::UploadFile
        };
        if !self.transfer.begin_path_prompt(prompt_kind) {
            self.shell
                .set_status("native path picker is already open".to_string());
            cx.notify();
            return false;
        }
        let receiver = cx.prompt_for_paths(options);
        self.shell.set_status(if directory_mode {
            "selecting trzsz upload directories".to_string()
        } else {
            "selecting trzsz upload files".to_string()
        });
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(Ok(Some(paths))) => {
                    if paths.is_empty() {
                        TransferPathPromptResult::Cancelled
                    } else {
                        TransferPathPromptResult::Selected(paths)
                    }
                }
                Ok(Ok(None)) => TransferPathPromptResult::Cancelled,
                Ok(Err(error)) => TransferPathPromptResult::Failed(error.to_string()),
                Err(_) => TransferPathPromptResult::Closed,
            };
            let _ = this.update(cx, |this, cx| {
                this.apply_trzsz_upload_path_prompt_result(
                    session_id,
                    remote_is_windows,
                    directory_mode,
                    result,
                    cx,
                );
                cx.notify();
            });
        })
        .detach();
        cx.notify();
        true
    }

    fn apply_trzsz_upload_path_prompt_result(
        &mut self,
        session_id: String,
        remote_is_windows: bool,
        directory_mode: bool,
        result: TransferPathPromptResult,
        cx: &mut Context<Self>,
    ) {
        let prompt_kind = if directory_mode {
            TransferPathPromptKind::UploadDirectory
        } else {
            TransferPathPromptKind::UploadFile
        };
        if !self.transfer.finish_path_prompt(prompt_kind) {
            return;
        }
        match result {
            TransferPathPromptResult::Selected(paths) => {
                self.accept_trzsz_upload_paths(
                    &session_id,
                    remote_is_windows,
                    directory_mode,
                    paths,
                    cx,
                );
            }
            TransferPathPromptResult::Cancelled => {
                self.reject_trzsz_upload_prompt(
                    &session_id,
                    remote_is_windows,
                    "trzsz upload selection cancelled",
                    cx,
                );
            }
            TransferPathPromptResult::Failed(error) => {
                self.reject_trzsz_upload_prompt(
                    &session_id,
                    remote_is_windows,
                    &format!("trzsz upload path picker failed: {error}"),
                    cx,
                );
            }
            TransferPathPromptResult::Closed => {
                self.reject_trzsz_upload_prompt(
                    &session_id,
                    remote_is_windows,
                    "trzsz upload path picker closed before returning",
                    cx,
                );
            }
        }
    }

    fn accept_trzsz_upload_paths(
        &mut self,
        session_id: &str,
        remote_is_windows: bool,
        directory_mode: bool,
        paths: Vec<PathBuf>,
        _cx: &mut Context<Self>,
    ) {
        self.trzsz_state_mut(session_id).begin_upload_preparation(
            paths,
            directory_mode,
            remote_is_windows,
        );
        self.shell.set_status("preparing trzsz upload".to_string());
    }

    fn accept_prepared_trzsz_upload(
        &mut self,
        session_id: &str,
        remote_is_windows: bool,
        directory_mode: bool,
        entries: Vec<TrzszUploadEntry>,
        files: HashMap<String, TrzszUploadFile>,
        _cx: &mut Context<Self>,
    ) {
        let file_count = entries.len();
        self.trzsz_state_mut(session_id).install_prepared_upload(
            remote_is_windows,
            directory_mode,
            entries,
            files,
        );

        let action = TrzszAction::local_default(remote_is_windows);
        let action_frame = build_trzsz_action_frame(&action, remote_is_windows);
        match self.write_session_protocol_response(session_id, &action_frame) {
            Ok(()) => {
                self.shell.set_status(format!(
                    "trzsz upload accepted ({file_count} file(s)) [{session_id}]"
                ));
            }
            Err(error) => {
                if let Some(state) = self.session.trzsz_state_mut(session_id) {
                    state.finish_upload();
                }
                self.shell
                    .set_status(format!("trzsz upload ACT failed: {error}"));
            }
        }
    }

    fn reject_trzsz_upload_prompt(
        &mut self,
        session_id: &str,
        remote_is_windows: bool,
        reason: &str,
        _cx: &mut Context<Self>,
    ) {
        let fail = trzsz_fail_response(reason, remote_is_windows);
        if let Err(error) = self.write_session_protocol_response(session_id, &fail) {
            self.shell
                .set_status(format!("trzsz upload reject failed: {error}"));
        } else {
            self.shell.set_status(reason.to_string());
        }
        if let Some(state) = self.session.trzsz_state_mut(session_id) {
            state.finish_upload();
        }
    }

    pub(in crate::features) fn drain_trzsz_upload_prepare_events(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.session.has_trzsz_runtime_sessions() {
            return false;
        }
        let mut events = Vec::new();
        for (session_id, state) in self.session.trzsz_states_mut() {
            let Some(worker) = state.upload_prepare_worker.as_ref() else {
                continue;
            };
            if let Some(event) = worker.try_recv_event() {
                events.push((session_id.clone(), event));
            }
        }
        if events.is_empty() {
            return false;
        }
        for (session_id, event) in events {
            match event.result {
                Ok((entries, files)) => self.accept_prepared_trzsz_upload(
                    &session_id,
                    event.remote_is_windows,
                    event.directory_mode,
                    entries,
                    files,
                    cx,
                ),
                Err(error) => self.reject_trzsz_upload_prompt(
                    &session_id,
                    event.remote_is_windows,
                    &error,
                    cx,
                ),
            }
        }
        true
    }

    fn handle_trzsz_protocol_frame(
        &mut self,
        session_id: &str,
        frame: TrzszProtocolFrame,
        responses: &mut Vec<Vec<u8>>,
        status: &mut Option<String>,
        cx: &mut Context<Self>,
    ) {
        let transfer_event = {
            let state = self.trzsz_state_mut(session_id);
            state.transfer.observe_frame(frame.clone())
        };
        match transfer_event {
            TrzszTransferEvent::Config { config } => {
                if let Some(download) = self
                    .session
                    .trzsz_state_mut(session_id)
                    .and_then(|state| state.download.as_mut())
                {
                    download.engine.set_directory_mode(config.directory);
                    if config.directory {
                        *status = Some("trzsz directory download accepted".to_string());
                    }
                }
                if self
                    .session
                    .trzsz_state(session_id)
                    .is_some_and(|state| state.upload.is_some())
                {
                    let expected_directory = self
                        .session
                        .trzsz_state(session_id)
                        .and_then(|state| state.upload.as_ref())
                        .is_some_and(|upload| upload.directory_mode);
                    if config.directory != expected_directory {
                        let reason = if expected_directory {
                            "remote trzsz config did not enable directory upload"
                        } else {
                            "remote trzsz config unexpectedly enabled directory upload"
                        };
                        self.fail_trzsz_upload(session_id, reason, responses, status, cx);
                        return;
                    }
                    self.begin_trzsz_upload(session_id, status);
                    return;
                }
            }
            TrzszTransferEvent::Failure { message } | TrzszTransferEvent::Exit { message } => {
                self.finish_trzsz_download_jobs(session_id, false, Some(&message), cx);
                self.finish_trzsz_upload_jobs(session_id, false, Some(&message), cx);
                if let Some(state) = self.session.trzsz_state_mut(session_id) {
                    state.finish_transfer();
                }
                *status = Some(format!("trzsz transfer stopped: {message}"));
                return;
            }
            _ => {}
        }

        if self
            .session
            .trzsz_state(session_id)
            .is_some_and(|state| state.upload.is_some() || state.upload_worker.is_some())
        {
            self.handle_trzsz_upload_frame(session_id, frame, responses, status);
            return;
        }

        let mut progress_updates = Vec::new();
        let mut download_completed = None;
        let mut download_error = None;
        {
            let state = self.trzsz_state_mut(session_id);
            let Some(download) = state.download.as_mut() else {
                return;
            };
            if !is_trzsz_download_frame(&frame) {
                return;
            }
            match download.engine.observe_frame(frame) {
                Ok(step) => {
                    responses.extend(step.responses);
                    for event in step.events {
                        match apply_trzsz_download_event(download, event) {
                            Ok(TrzszDownloadRuntimeUpdate::None) => {}
                            Ok(TrzszDownloadRuntimeUpdate::Progress(update)) => {
                                progress_updates.push(update);
                            }
                            Ok(TrzszDownloadRuntimeUpdate::Completed(names)) => {
                                download_completed = Some(names);
                            }
                            Err(error) => {
                                download_error = Some(error);
                                break;
                            }
                        }
                    }
                }
                Err(error) => {
                    download_error = Some(format!("{error:?}"));
                }
            }
        }

        for update in progress_updates {
            self.update_trzsz_download_job(session_id, update, cx);
        }
        if let Some(error) = download_error {
            self.fail_trzsz_download(session_id, &error, responses, status, cx);
            return;
        }
        if let Some(names) = download_completed {
            let message = if names.is_empty() {
                "trzsz download complete".to_string()
            } else {
                format!("Saved {}", names.join(", "))
            };
            let newline = if self
                .session
                .trzsz_state(session_id)
                .is_some_and(|state| state.transfer.remote_is_windows)
            {
                "!\n"
            } else {
                "\n"
            };
            responses.push(build_trzsz_string_frame(
                "EXIT",
                message.as_bytes(),
                newline,
            ));
            self.finish_trzsz_download_jobs(session_id, true, None, cx);
            if let Some(state) = self.session.trzsz_state_mut(session_id) {
                state.finish_download();
            }
            *status = Some(message);
        }
    }

    fn fail_trzsz_download(
        &mut self,
        session_id: &str,
        reason: &str,
        responses: &mut Vec<Vec<u8>>,
        status: &mut Option<String>,
        cx: &mut Context<Self>,
    ) {
        let remote_is_windows = self
            .session
            .trzsz_state(session_id)
            .map(|state| state.transfer.remote_is_windows)
            .unwrap_or(false);
        responses.push(trzsz_fail_response(reason, remote_is_windows));
        self.finish_trzsz_download_jobs(session_id, false, Some(reason), cx);
        if let Some(state) = self.session.trzsz_state_mut(session_id) {
            state.finish_download();
        }
        *status = Some(format!("trzsz download failed: {reason}"));
    }

    fn begin_trzsz_upload(&mut self, session_id: &str, status: &mut Option<String>) {
        let remote_is_windows = self
            .session
            .trzsz_state(session_id)
            .map(|state| state.transfer.remote_is_windows)
            .unwrap_or(false);
        let Some(upload) = self
            .session
            .trzsz_state_mut(session_id)
            .and_then(|state| state.upload.take())
        else {
            return;
        };
        let worker = TrzszUploadWorker::spawn(upload, remote_is_windows);
        worker.begin();
        if let Some(state) = self.session.trzsz_state_mut(session_id) {
            state.upload_worker = Some(worker);
        }
        *status = Some("trzsz upload starting".to_string());
    }

    fn handle_trzsz_upload_frame(
        &mut self,
        session_id: &str,
        frame: TrzszProtocolFrame,
        _responses: &mut Vec<Vec<u8>>,
        status: &mut Option<String>,
    ) {
        if !is_trzsz_upload_frame(&frame) {
            return;
        }
        if self.queue_trzsz_upload_worker_frame(session_id, frame) {
            *status = Some("trzsz upload in progress".to_string());
        }
    }

    fn fail_trzsz_upload(
        &mut self,
        session_id: &str,
        reason: &str,
        responses: &mut Vec<Vec<u8>>,
        status: &mut Option<String>,
        cx: &mut Context<Self>,
    ) {
        let remote_is_windows = self
            .session
            .trzsz_state(session_id)
            .map(|state| state.transfer.remote_is_windows)
            .unwrap_or(false);
        responses.push(trzsz_fail_response(reason, remote_is_windows));
        self.finish_trzsz_upload_jobs(session_id, false, Some(reason), cx);
        if let Some(state) = self.session.trzsz_state_mut(session_id) {
            state.finish_upload();
        }
        *status = Some(format!("trzsz upload failed: {reason}"));
    }

    fn update_trzsz_download_job(
        &mut self,
        session_id: &str,
        update: TrzszDownloadProgressUpdate,
        cx: &mut Context<Self>,
    ) {
        let short = short_id(session_id);
        let progress = SftpTransferProgress {
            remote_path: format!("trzsz://{short}/{}", update.file_name),
            local_path: update.local_path.clone(),
            bytes_transferred: update.bytes_transferred,
            total_bytes: update.total_bytes,
            item_count_completed: None,
            item_count_total: None,
        };
        let existing_job_id = self
            .transfer
            .transfer_jobs()
            .iter()
            .find(|job| {
                matches!(
                    &job.kind,
                    TransferJobKind::TrzszDownload {
                        session_id: sid,
                        file_name,
                    } if sid == session_id && file_name == &update.file_name
                ) && matches!(
                    job.status,
                    TransferJobStatus::Running | TransferJobStatus::Cancelling
                )
            })
            .map(|job| job.id.clone());
        if let Some(job) = existing_job_id
            .as_deref()
            .and_then(|job_id| self.transfer.transfer_job_mut(job_id))
        {
            job.update_progress(progress);
            job.detail = if update.completed {
                "Complete".to_string()
            } else if let Some(reason) = update.fail_reason.as_deref() {
                format!("Failed: {reason}")
            } else if let Some(total) = update.total_bytes.filter(|total| *total > 0) {
                format!(
                    "{:.0}%",
                    (update.bytes_transferred as f64 / total as f64 * 100.).clamp(0., 100.)
                )
            } else {
                format!("{} bytes", update.bytes_transferred)
            };
            if update.completed {
                job.status = TransferJobStatus::Completed;
            } else if update.fail_reason.is_some() {
                job.status = TransferJobStatus::Failed;
            }
            self.defer_transfer_panel_snapshot_flush(cx);
            return;
        }

        let id = self.transfer.next_transfer_job_id("trzsz-download");
        let status = if update.completed {
            TransferJobStatus::Completed
        } else if update.fail_reason.is_some() {
            TransferJobStatus::Failed
        } else {
            TransferJobStatus::Running
        };
        let detail = update
            .fail_reason
            .as_deref()
            .map(|reason| format!("Failed: {reason}"))
            .unwrap_or_else(|| {
                if update.completed {
                    "Complete".to_string()
                } else {
                    format!("Downloading {}", update.file_name)
                }
            });
        self.transfer.enqueue_transfer_job(TransferJobState {
            id,
            session_id: Some(session_id.to_string()),
            kind: TransferJobKind::TrzszDownload {
                session_id: session_id.to_string(),
                file_name: update.file_name,
            },
            status,
            detail,
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: Some(progress),
            control: None,
            speed: Default::default(),
        });
        self.defer_transfer_panel_snapshot_flush(cx);
    }

    fn update_trzsz_upload_job(
        &mut self,
        session_id: &str,
        update: TrzszUploadProgressUpdate,
        cx: &mut Context<Self>,
    ) {
        let short = short_id(session_id);
        let progress = SftpTransferProgress {
            remote_path: format!("trzsz://{short}/{}", update.remote_name),
            local_path: update.local_path.clone(),
            bytes_transferred: update.bytes_transferred,
            total_bytes: update.total_bytes,
            item_count_completed: None,
            item_count_total: None,
        };
        let existing_job_id = self
            .transfer
            .transfer_jobs()
            .iter()
            .find(|job| {
                matches!(
                    &job.kind,
                    TransferJobKind::TrzszUpload {
                        session_id: sid,
                        file_name,
                    } if sid == session_id && file_name == &update.file_name
                ) && matches!(
                    job.status,
                    TransferJobStatus::Running | TransferJobStatus::Cancelling
                )
            })
            .map(|job| job.id.clone());
        if let Some(job) = existing_job_id
            .as_deref()
            .and_then(|job_id| self.transfer.transfer_job_mut(job_id))
        {
            job.update_progress(progress);
            job.detail = if update.completed {
                "Complete".to_string()
            } else if let Some(reason) = update.fail_reason.as_deref() {
                format!("Failed: {reason}")
            } else if let Some(total) = update.total_bytes.filter(|total| *total > 0) {
                format!(
                    "{:.0}%",
                    (update.bytes_transferred as f64 / total as f64 * 100.).clamp(0., 100.)
                )
            } else {
                format!("{} bytes", update.bytes_transferred)
            };
            if update.completed {
                job.status = TransferJobStatus::Completed;
            } else if update.fail_reason.is_some() {
                job.status = TransferJobStatus::Failed;
            }
            self.defer_transfer_panel_snapshot_flush(cx);
            return;
        }

        let id = self.transfer.next_transfer_job_id("trzsz-upload");
        let status = if update.completed {
            TransferJobStatus::Completed
        } else if update.fail_reason.is_some() {
            TransferJobStatus::Failed
        } else {
            TransferJobStatus::Running
        };
        let detail = update
            .fail_reason
            .as_deref()
            .map(|reason| format!("Failed: {reason}"))
            .unwrap_or_else(|| {
                if update.completed {
                    "Complete".to_string()
                } else {
                    format!("Uploading {}", update.file_name)
                }
            });
        self.transfer.enqueue_transfer_job(TransferJobState {
            id,
            session_id: Some(session_id.to_string()),
            kind: TransferJobKind::TrzszUpload {
                session_id: session_id.to_string(),
                file_name: update.file_name,
            },
            status,
            detail,
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: Some(progress),
            control: None,
            speed: Default::default(),
        });
        self.defer_transfer_panel_snapshot_flush(cx);
    }

    fn finish_trzsz_download_jobs(
        &mut self,
        session_id: &str,
        success: bool,
        fail_reason: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        self.transfer.visit_transfer_jobs_mut(|job| {
            let is_trzsz = matches!(
                &job.kind,
                TransferJobKind::TrzszDownload {
                    session_id: sid,
                    ..
                } if sid == session_id
            );
            if !is_trzsz
                || !matches!(
                    job.status,
                    TransferJobStatus::Running | TransferJobStatus::Cancelling
                )
            {
                return;
            }
            if success {
                job.status = TransferJobStatus::Completed;
                job.detail = "Complete".to_string();
            } else {
                job.status = TransferJobStatus::Failed;
                job.detail = fail_reason
                    .map(|reason| format!("Failed: {reason}"))
                    .unwrap_or_else(|| "Failed".to_string());
            }
            changed = true;
        });
        if changed {
            self.defer_transfer_panel_snapshot_flush(cx);
        }
    }

    fn finish_trzsz_upload_jobs(
        &mut self,
        session_id: &str,
        success: bool,
        fail_reason: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        self.transfer.visit_transfer_jobs_mut(|job| {
            let is_trzsz = matches!(
                &job.kind,
                TransferJobKind::TrzszUpload {
                    session_id: sid,
                    ..
                } if sid == session_id
            );
            if !is_trzsz
                || !matches!(
                    job.status,
                    TransferJobStatus::Running | TransferJobStatus::Cancelling
                )
            {
                return;
            }
            if success {
                job.status = TransferJobStatus::Completed;
                job.detail = "Complete".to_string();
            } else {
                job.status = TransferJobStatus::Failed;
                job.detail = fail_reason
                    .map(|reason| format!("Failed: {reason}"))
                    .unwrap_or_else(|| "Failed".to_string());
            }
            changed = true;
        });
        if changed {
            self.defer_transfer_panel_snapshot_flush(cx);
        }
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
        match command {
            TrzszDownloadWorkerCommand::Output(data) => {
                let event = process_trzsz_download_worker_output(
                    &mut download,
                    &mut protocol,
                    &mut transfer,
                    data,
                );
                let done = event.completed.is_some() || event.failed.is_some();
                let _ = event_tx.send(event);
                if done {
                    break;
                }
            }
            TrzszDownloadWorkerCommand::Stop => break,
        }
    }
}

fn process_trzsz_download_worker_output(
    download: &mut TrzszDownloadRuntime,
    protocol: &mut TrzszProtocolStream,
    transfer: &mut TrzszTransferState,
    data: Vec<u8>,
) -> TrzszDownloadWorkerEvent {
    let protocol_output = protocol.filter_terminal_output(&data);
    let mut event = TrzszDownloadWorkerEvent {
        passthrough: protocol_output.passthrough,
        ..TrzszDownloadWorkerEvent::default()
    };

    for frame in protocol_output.frames {
        if let Some(done) =
            process_trzsz_download_worker_frame(download, transfer, frame, &mut event)
        {
            event.failed = done.failed;
            event.completed = done.completed;
            break;
        }
    }
    event
}

fn process_trzsz_download_worker_frame(
    download: &mut TrzszDownloadRuntime,
    transfer: &mut TrzszTransferState,
    frame: TrzszProtocolFrame,
    event: &mut TrzszDownloadWorkerEvent,
) -> Option<TrzszDownloadWorkerEvent> {
    match transfer.observe_frame(frame.clone()) {
        TrzszTransferEvent::Config { config } => {
            download.engine.set_directory_mode(config.directory);
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
    match download.engine.observe_frame(frame) {
        Ok(step) => {
            event.responses.extend(step.responses);
            for transfer_event in step.events {
                match apply_trzsz_download_event(download, transfer_event) {
                    Ok(TrzszDownloadRuntimeUpdate::None) => {}
                    Ok(TrzszDownloadRuntimeUpdate::Progress(update)) => {
                        event.progress.push(update);
                    }
                    Ok(TrzszDownloadRuntimeUpdate::Completed(names)) => {
                        let message = if names.is_empty() {
                            "trzsz download complete".to_string()
                        } else {
                            format!("Saved {}", names.join(", "))
                        };
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
                            ..TrzszDownloadWorkerEvent::default()
                        });
                    }
                    Err(error) => {
                        let response = trzsz_fail_response(&error, transfer.remote_is_windows);
                        event.responses.push(response);
                        return Some(TrzszDownloadWorkerEvent {
                            failed: Some(error),
                            ..TrzszDownloadWorkerEvent::default()
                        });
                    }
                }
            }
        }
        Err(error) => {
            let reason = format!("{error:?}");
            let response = trzsz_fail_response(&reason, transfer.remote_is_windows);
            event.responses.push(response);
            return Some(TrzszDownloadWorkerEvent {
                failed: Some(reason),
                ..TrzszDownloadWorkerEvent::default()
            });
        }
    }
    None
}

fn run_trzsz_upload_worker(
    mut upload: TrzszUploadRuntime,
    remote_is_windows: bool,
    command_rx: mpsc::Receiver<TrzszUploadWorkerCommand>,
    event_tx: mpsc::SyncSender<TrzszUploadWorkerEvent>,
) {
    while let Ok(command) = command_rx.recv() {
        let event = match command {
            TrzszUploadWorkerCommand::Begin => {
                process_trzsz_upload_worker_begin(&mut upload, remote_is_windows)
            }
            TrzszUploadWorkerCommand::Frame(frame) => {
                process_trzsz_upload_worker_frame(&mut upload, remote_is_windows, frame)
            }
            TrzszUploadWorkerCommand::Stop => break,
        };
        let done = event.completed.is_some() || event.failed.is_some();
        let _ = event_tx.send(event);
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
                        return complete_trzsz_upload_worker_event(names, remote_is_windows, event);
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
    names: Vec<String>,
    remote_is_windows: bool,
    mut event: TrzszUploadWorkerEvent,
) -> TrzszUploadWorkerEvent {
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
        "NUM" | "NAME" | "SIZE" | "DATA" | "MD5"
    )
}

fn is_trzsz_upload_frame(frame: &TrzszProtocolFrame) -> bool {
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
            debug_assert!(!file.is_dir);
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
        TrzszDownloadEvent::FileFinished { .. } => {
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
                    completed: true,
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
const TRZSZ_DOWNLOAD_WORKER_EVENT_DRAIN_BATCH: usize = 32;
const TRZSZ_UPLOAD_WORKER_EVENT_CHANNEL_CAP: usize = 256;
const TRZSZ_UPLOAD_WORKER_EVENT_DRAIN_BATCH: usize = 32;

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    use gpui::{AppContext as _, TestAppContext};

    use nyaterm_transport::{
        TrzszDownloadEngine, TrzszProtocolFrame, TrzszProtocolPayload, TrzszTransferState,
        TrzszUploadEngine, TrzszUploadEntry, TrzszUploadPayload,
    };

    use super::{
        TrzszDownloadRuntime, TrzszDownloadWorker, TrzszDownloadWorkerEvent,
        TrzszUploadPrepareWorker, TrzszUploadRuntime, process_trzsz_download_worker_frame,
        process_trzsz_upload_worker_begin,
    };
    use crate::features::test_support::app_with_visible_local_session;
    use crate::test_support::TestConfigDir;

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
    fn idle_zmodem_output_stays_in_active_trzsz_protocol() {
        let root = TestConfigDir::new("nyaterm-zmodem-idle-trzsz");
        let mut cx = TestAppContext::single();
        let app = app_with_visible_local_session(&mut cx, root.path(), "protocol-session");
        cx.update_entity(&app, |app, cx| {
            let state = app.trzsz_state_mut("protocol-session");
            state.protocol_active = true;
            // A binary trzsz frame consumes raw payload bytes, including '*'.
            state.protocol.filter_terminal_output(b"#DATA:2\n");
            assert_eq!(
                app.process_zmodem_output("protocol-session", b"*", cx),
                (Vec::new(), false)
            );
            assert_eq!(
                app.drain_zmodem_idle_output(Instant::now() + Duration::from_millis(100), cx),
                (Vec::new(), false)
            );
            let output = app
                .trzsz_state_mut("protocol-session")
                .protocol
                .filter_terminal_output(b"x");
            assert!(output.passthrough.is_empty());
            assert_eq!(output.frames.len(), 1);
            assert_eq!(
                output.frames[0].payload,
                TrzszProtocolPayload::EncodedBytes(b"*x".to_vec())
            );
        });
    }

    #[test]
    fn idle_trzsz_prefix_reaches_terminal_without_a_second_output_event() {
        let root = TestConfigDir::new("nyaterm-trzsz-idle-colon");
        let mut cx = TestAppContext::single();
        let app = app_with_visible_local_session(&mut cx, root.path(), "colon-session");
        cx.update_entity(&app, |app, cx| {
            assert_eq!(
                app.process_trzsz_output("colon-session", b":", cx),
                (Vec::new(), false)
            );
            assert!(
                app.session.has_protocol_runtime_sessions(),
                "the runtime must stay awake to flush the prefix"
            );
            let now = Instant::now() + Duration::from_millis(100);
            assert_eq!(
                app.drain_trzsz_idle_output(now, cx),
                (vec![("colon-session".to_string(), b":".to_vec())], false)
            );
            assert!(app.trzsz_output_can_bypass_detector("colon-session", b"normal output"));
            assert_eq!(app.drain_trzsz_idle_output(now, cx), (Vec::new(), false));
        });
    }

    #[test]
    fn idle_trzsz_prefix_stays_in_active_transfer_protocol_instead_of_terminal() {
        let root = TestConfigDir::new("nyaterm-trzsz-idle-protocol");
        let mut cx = TestAppContext::single();
        let app = app_with_visible_local_session(&mut cx, root.path(), "protocol-session");
        cx.update_entity(&app, |app, cx| {
            let state = app.trzsz_state_mut("protocol-session");
            state.protocol_active = true;
            assert!(
                state
                    .protocol
                    .filter_terminal_output(b"#NUM")
                    .frames
                    .is_empty()
            );
            assert_eq!(
                app.process_trzsz_output("protocol-session", b":", cx),
                (Vec::new(), false)
            );
            assert_eq!(
                app.drain_trzsz_idle_output(Instant::now() + Duration::from_millis(100), cx),
                (Vec::new(), false)
            );
            let output = app
                .trzsz_state_mut("protocol-session")
                .protocol
                .filter_terminal_output(b"1\n");
            assert!(output.passthrough.is_empty());
            assert_eq!(output.frames.len(), 1);
            assert_eq!(output.frames[0].frame_type, "NUM");
            assert_eq!(output.frames[0].payload, TrzszProtocolPayload::Integer(1));
        });
    }

    #[test]
    fn output_discontinuity_discards_pending_trzsz_prefix_before_idle_flush() {
        let root = TestConfigDir::new("nyaterm-trzsz-idle-reset");
        let mut cx = TestAppContext::single();
        let app = app_with_visible_local_session(&mut cx, root.path(), "reset-session");
        cx.update_entity(&app, |app, cx| {
            app.process_trzsz_output("reset-session", b":", cx);
            app.note_trzsz_output_discontinuity("reset-session");
            assert_eq!(
                app.drain_trzsz_idle_output(Instant::now() + Duration::from_secs(2), cx),
                (Vec::new(), false)
            );
        });
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
