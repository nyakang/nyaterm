use gpui::{Context, PathPromptOptions, SharedString};
use nyaterm_transport::{
    SftpDuplicateDecision, SftpDuplicatePolicy, SftpDuplicateRequest, SftpDuplicateResolver,
    SftpService, SftpTransferDirection, SftpTransferProgress, SshSessionConfig, ZmodemAction,
    ZmodemDetectResult, ZmodemDetector, ZmodemDirection, ZmodemEvent, ZmodemTransfer,
};
use std::{collections::HashSet, path::PathBuf, sync::mpsc, thread, time::Instant};

use crate::features::NyaTermApp;
use crate::features::formatting::short_id;
use crate::models::{
    TransferJobEvent, TransferJobKind, TransferJobOutput, TransferJobResult, TransferJobState,
    TransferJobStatus,
};

pub(super) struct ZmodemSessionState {
    detector: ZmodemDetector,
    transfer: Option<ZmodemTransfer>,
    worker: Option<ZmodemWorker>,
    pending_upload: Option<Vec<PathBuf>>,
    /// Download waiting for user to pick a save directory.
    pending_download: bool,
}

struct ZmodemWorker {
    command_tx: mpsc::Sender<ZmodemWorkerCommand>,
    event_rx: Option<mpsc::Receiver<ZmodemWorkerEvent>>,
    worker: Option<thread::JoinHandle<()>>,
}

enum ZmodemWorkerCommand {
    Input(Vec<u8>),
    AcceptDownload(PathBuf),
    AcceptUpload(Vec<PathBuf>),
    Cancel(String),
    Stop,
}

struct ZmodemWorkerEvent {
    actions: Vec<ZmodemAction>,
    done: bool,
}

impl ZmodemWorker {
    fn spawn(transfer: ZmodemTransfer) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::sync_channel(ZMODEM_WORKER_EVENT_CHANNEL_CAP);
        let worker = thread::Builder::new()
            .name("nyaterm-zmodem-transfer".to_string())
            .spawn(move || run_zmodem_worker(transfer, command_rx, event_tx))
            .expect("failed to spawn zmodem worker");
        Self {
            command_tx,
            event_rx: Some(event_rx),
            worker: Some(worker),
        }
    }

    fn send_input(&self, data: Vec<u8>) {
        if !data.is_empty() {
            let _ = self.command_tx.send(ZmodemWorkerCommand::Input(data));
        }
    }

    fn accept_download(&self, save_dir: PathBuf) {
        let _ = self
            .command_tx
            .send(ZmodemWorkerCommand::AcceptDownload(save_dir));
    }

    fn accept_upload(&self, files: Vec<PathBuf>) {
        let _ = self
            .command_tx
            .send(ZmodemWorkerCommand::AcceptUpload(files));
    }

    fn cancel(&self, reason: impl Into<String>) {
        let _ = self
            .command_tx
            .send(ZmodemWorkerCommand::Cancel(reason.into()));
    }

    fn try_recv_event(&self) -> Option<ZmodemWorkerEvent> {
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
        let _ = self.command_tx.send(ZmodemWorkerCommand::Stop);
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::warn!("ZMODEM worker panicked during shutdown");
        }
    }
}

impl Drop for ZmodemWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Default for ZmodemSessionState {
    fn default() -> Self {
        Self {
            detector: ZmodemDetector::new(),
            transfer: None,
            worker: None,
            pending_upload: None,
            pending_download: false,
        }
    }
}

impl ZmodemSessionState {
    fn flush_idle_output(&mut self, now: Instant) -> Vec<u8> {
        if self.transfer.is_some() || self.worker.is_some() {
            return Vec::new();
        }
        self.detector.flush_pending_if_idle(now)
    }

    pub(super) fn is_active(&self) -> bool {
        self.transfer.is_some()
            || self.worker.is_some()
            || self.pending_upload.is_some()
            || self.pending_download
    }
    pub(super) fn stop_worker(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.stop();
        }
    }

    fn finish_transfer(&mut self) {
        self.transfer = None;
        self.worker = None;
        self.detector = ZmodemDetector::new();
        self.pending_download = false;
        self.pending_upload = None;
    }
}

impl Drop for ZmodemSessionState {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

impl NyaTermApp {
    fn zmodem_state_mut(&mut self, session_id: &str) -> &mut ZmodemSessionState {
        self.session.zmodem_state_mut_or_default(session_id)
    }

    pub(in crate::features) fn clear_zmodem_session(&mut self, session_id: &str) {
        if self.session.remove_zmodem_session_runtime(session_id) {
            self.sync_session_event_bridge_session_policy(session_id);
        }
    }

    pub(in crate::features) fn note_zmodem_output_discontinuity(
        &mut self,
        session_id: &str,
        dropped_bytes: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(state) = self.session.zmodem_state_mut(session_id) else {
            return false;
        };
        let reason = format!("terminal output dropped {dropped_bytes} byte(s)");
        if let Some(worker) = state.worker.as_ref() {
            worker.cancel(reason);
        } else if let Some(transfer) = state.transfer.as_mut() {
            let actions = transfer.cancel_with_reason(reason);
            state.transfer = None;
            state.detector = ZmodemDetector::new();
            state.pending_download = false;
            if !actions.is_empty() {
                return self.apply_zmodem_actions(session_id, actions, cx);
            }
            return false;
        }
        state.transfer = None;
        state.detector = ZmodemDetector::new();
        state.pending_download = false;
        false
    }

    /// Queue local files for ZMODEM upload (remote `rz`) after optional SFTP conflict probe.
    pub(in crate::features) fn start_zmodem_upload(
        &mut self,
        session_id: String,
        files: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if files.is_empty() {
            return;
        }
        if self.session.is_disconnected(&session_id) {
            self.shell
                .set_status("session disconnected — reconnect before ZMODEM upload".to_string());
            cx.notify();
            return;
        }
        let state = self.zmodem_state_mut(&session_id);
        if state.transfer.is_some() || state.worker.is_some() {
            self.shell
                .set_status("ZMODEM transfer already active".to_string());
            cx.notify();
            return;
        }

        // Prefer SSH config bound to this session, fall back to active.
        let ssh_config = self
            .session
            .metadata(&session_id)
            .and_then(|meta| meta.ssh_config.clone())
            .or_else(|| self.session.active_ssh_config_owned());

        let Some(config) = ssh_config else {
            // Non-SSH sessions cannot probe; start rz immediately.
            self.begin_zmodem_upload_after_probe(session_id, files, false, cx);
            return;
        };

        let remote_dir = self
            .session
            .cwd(&session_id)
            .map(ToOwned::to_owned)
            .filter(|cwd| !cwd.trim().is_empty())
            .unwrap_or_else(|| "/".to_string());
        let remote_dir = remote_dir.trim_end_matches('/').to_string();
        let remote_dir = if remote_dir.is_empty() {
            "/".to_string()
        } else {
            remote_dir
        };

        let policy = self.transfer.duplicate_policy();
        let resolver = self.session.prompts.duplicate_broker();
        let id = self.transfer.next_transfer_job_id("zmodem-probe");
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id: Some(session_id.clone()),
            kind: TransferJobKind::ZmodemConflictProbe {
                session_id: session_id.clone(),
                remote_dir: remote_dir.clone(),
            },
            status: TransferJobStatus::Running,
            detail: format!(
                "Probing remote conflicts in {remote_dir} ({} file(s))",
                files.len()
            ),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
            speed: Default::default(),
        });
        self.shell.set_status(format!(
            "ZMODEM preparing upload ({} file(s)) — probing remote conflicts",
            files.len()
        ));
        let transfer_tx = self.transfer.transfer_event_sender();
        let probe_session_id = session_id.clone();
        self.submit_transfer_blocking_job(
            "zmodem-conflict-probe",
            id.clone(),
            transfer_tx.clone(),
            move || {
                let result = probe_zmodem_remote_conflicts(
                    config,
                    remote_dir,
                    files,
                    policy,
                    resolver.as_ref(),
                )
                .map(
                    |(resolved, probe_skipped, overwrite)| TransferJobOutput::ZmodemProbeReady {
                        session_id: probe_session_id,
                        files: resolved,
                        probe_skipped,
                        overwrite,
                    },
                )
                .map_err(|error| error.to_string());
                let _ = transfer_tx.unbounded_send(TransferJobResult {
                    id,
                    event: TransferJobEvent::Finished(result),
                });
            },
        );
        cx.notify();
    }

    /// Start remote `rz` after conflict resolution (or when probing is unavailable).
    pub(in crate::features) fn begin_zmodem_upload_after_probe(
        &mut self,
        session_id: String,
        files: Vec<PathBuf>,
        overwrite: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.session.manager().supports_modem_transfer(&session_id) {
            self.shell.set_status(
                rust_i18n::t!("dialog.serialSoftwareFlowControlModemDisabled").to_string(),
            );
            cx.notify();
            return;
        }
        if files.is_empty() {
            self.shell.set_status(
                "ZMODEM upload cancelled — no files remaining after conflict resolution"
                    .to_string(),
            );
            cx.notify();
            return;
        }
        if self.session.is_disconnected(&session_id) {
            self.shell
                .set_status("session disconnected — reconnect before ZMODEM upload".to_string());
            cx.notify();
            return;
        }
        let state = self.zmodem_state_mut(&session_id);
        if state.transfer.is_some() || state.worker.is_some() {
            self.shell
                .set_status("ZMODEM transfer already active".to_string());
            cx.notify();
            return;
        }
        state.pending_upload = Some(files.clone());
        state.pending_download = false;
        // Remote side runs `rz` and emits ZMODEM upload (local send) headers.
        let cmd = nyaterm_transport::zmodem::receive_command(overwrite);
        match self.write_session_input_recorded(&session_id, &cmd) {
            Ok(()) => {
                self.shell.set_status(format!(
                    "ZMODEM upload prepared ({} file(s)) — waiting for remote rz",
                    files.len()
                ));
            }
            Err(error) => {
                self.zmodem_state_mut(&session_id).pending_upload = None;
                self.shell
                    .set_status(format!("ZMODEM upload failed to start: {error}"));
            }
        }
        cx.notify();
    }

    pub(in crate::features) fn cancel_zmodem_transfer(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.session.zmodem_state_mut(session_id) else {
            return;
        };
        state.pending_upload = None;
        state.pending_download = false;
        if let Some(worker) = state.worker.as_ref() {
            worker.cancel("cancelled");
            self.shell
                .set_status("ZMODEM transfer cancelling".to_string());
            cx.notify();
            return;
        }
        let actions = state
            .transfer
            .as_mut()
            .map(ZmodemTransfer::cancel)
            .unwrap_or_default();
        state.finish_transfer();
        self.apply_zmodem_actions(session_id, actions, cx);
        self.shell
            .set_status("ZMODEM transfer cancelled".to_string());
        cx.notify();
    }

    pub(in crate::features) fn accept_zmodem_download(
        &mut self,
        session_id: String,
        save_dir: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.session.zmodem_state_mut(&session_id) else {
            return;
        };
        state.pending_download = false;
        if let Some(worker) = state.worker.as_ref() {
            worker.accept_download(save_dir);
            cx.notify();
            return;
        }
        let Some(transfer) = state.transfer.take() else {
            return;
        };
        let worker = ZmodemWorker::spawn(transfer);
        worker.accept_download(save_dir);
        state.worker = Some(worker);
        cx.notify();
    }

    fn accept_zmodem_upload(
        &mut self,
        session_id: String,
        files: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.session.zmodem_state_mut(&session_id) else {
            return;
        };
        if let Some(worker) = state.worker.as_ref() {
            worker.accept_upload(files);
            cx.notify();
            return;
        }
        let Some(transfer) = state.transfer.take() else {
            return;
        };
        let worker = ZmodemWorker::spawn(transfer);
        worker.accept_upload(files);
        state.worker = Some(worker);
        cx.notify();
    }

    pub(in crate::features) fn drain_zmodem_worker_events(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.session.has_zmodem_runtime_sessions() {
            return false;
        }
        let mut events = Vec::new();
        for (session_id, state) in self.session.zmodem_states_mut() {
            let Some(worker) = state.worker.as_ref() else {
                continue;
            };
            while let Some(event) = worker.try_recv_event() {
                events.push((session_id.clone(), event));
                if events.len() >= ZMODEM_WORKER_EVENT_DRAIN_BATCH {
                    break;
                }
            }
            if events.len() >= ZMODEM_WORKER_EVENT_DRAIN_BATCH {
                break;
            }
        }
        if events.is_empty() {
            return false;
        }

        let mut root_chrome_dirty = false;
        for (session_id, event) in events {
            root_chrome_dirty |= self.apply_zmodem_worker_event(&session_id, event, cx);
        }
        root_chrome_dirty
    }

    fn apply_zmodem_worker_event(
        &mut self,
        session_id: &str,
        event: ZmodemWorkerEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let root_chrome_dirty = self.apply_zmodem_actions(session_id, event.actions, cx);
        if event.done
            && let Some(state) = self.session.zmodem_state_mut(session_id)
            && state.worker.is_some()
        {
            state.finish_transfer();
        }
        root_chrome_dirty
    }

    /// Process raw session output for ZMODEM interception. Returns bytes that
    /// should still be painted in the terminal (empty while a transfer is active).
    pub(in crate::features) fn process_zmodem_output(
        &mut self,
        session_id: &str,
        data: &[u8],
        cx: &mut Context<Self>,
    ) -> (Vec<u8>, bool) {
        if !self.session.manager().supports_modem_transfer(session_id) {
            return (data.to_vec(), false);
        }
        if data.is_empty() {
            return (Vec::new(), false);
        }
        if self.zmodem_output_can_bypass_detector(session_id, data) {
            return (data.to_vec(), false);
        }

        // Active transfer: consume all raw bytes.
        if let Some(state) = self.session.zmodem_state(session_id)
            && let Some(worker) = state.worker.as_ref()
        {
            worker.send_input(data.to_vec());
            return (Vec::new(), false);
        }

        if self
            .session
            .zmodem_state(session_id)
            .is_some_and(|state| state.transfer.is_some())
        {
            if let Some(transfer) = self
                .session
                .zmodem_state_mut(session_id)
                .and_then(|state| state.transfer.take())
            {
                let worker = ZmodemWorker::spawn(transfer);
                worker.send_input(data.to_vec());
                if let Some(state) = self.session.zmodem_state_mut(session_id) {
                    state.worker = Some(worker);
                }
            }
            return (Vec::new(), false);
        }

        // Detection path.
        let feed_result = {
            let state = self.zmodem_state_mut(session_id);
            state.detector.feed(data)
        };
        match feed_result {
            ZmodemDetectResult::NoMatch { passthrough } => (passthrough, false),
            ZmodemDetectResult::Detected {
                direction,
                passthrough,
                initial_bytes,
            } => {
                let prepared_upload = if direction == ZmodemDirection::Upload {
                    self.zmodem_state_mut(session_id).pending_upload.take()
                } else {
                    None
                };
                let prompt_for_upload =
                    direction == ZmodemDirection::Upload && prepared_upload.is_none();
                let transfer = ZmodemTransfer::new(direction, &initial_bytes);
                {
                    let state = self.zmodem_state_mut(session_id);
                    if let Some(files) = prepared_upload {
                        let worker = ZmodemWorker::spawn(transfer);
                        worker.accept_upload(files);
                        state.worker = Some(worker);
                    } else {
                        state.transfer = Some(transfer);
                        if direction == ZmodemDirection::Download {
                            state.pending_download = true;
                        }
                    }
                }
                // Surface detection event status before opening a path picker.
                self.shell.set_status(match direction {
                    ZmodemDirection::Upload if prompt_for_upload => {
                        "ZMODEM upload detected — choose local files".to_string()
                    }
                    ZmodemDirection::Upload => "ZMODEM upload detected".to_string(),
                    ZmodemDirection::Download => {
                        "ZMODEM download detected — choose save folder".to_string()
                    }
                });
                match direction {
                    ZmodemDirection::Upload if prompt_for_upload => {
                        self.prompt_zmodem_upload_files(session_id.to_string(), cx);
                    }
                    ZmodemDirection::Download => {
                        self.prompt_zmodem_download_directory(session_id.to_string(), cx);
                    }
                    ZmodemDirection::Upload => {}
                }
                (passthrough, true)
            }
        }
    }

    pub(in crate::features) fn drain_zmodem_idle_output(
        &mut self,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> (Vec<(String, Vec<u8>)>, bool) {
        let expired = self
            .session
            .zmodem_states_mut()
            .filter_map(|(session_id, state)| {
                let bytes = state.flush_idle_output(now);
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
            // Match the ordinary output path: ZMODEM precedes trzsz.
            let (bytes, dirty) = self.process_trzsz_output(&session_id, &bytes, cx);
            root_chrome_dirty |= dirty;
            if !bytes.is_empty() {
                outputs.push((session_id, bytes));
            }
        }
        (outputs, root_chrome_dirty)
    }

    pub(in crate::features) fn zmodem_output_can_bypass_detector(
        &self,
        session_id: &str,
        data: &[u8],
    ) -> bool {
        let state_is_idle = self.session.zmodem_state(session_id).is_none_or(|state| {
            state.transfer.is_none()
                && state.worker.is_none()
                && state.pending_upload.is_none()
                && !state.pending_download
                && state.detector.is_idle()
        });
        state_is_idle && !ZmodemDetector::output_may_contain_trigger(data)
    }

    fn apply_zmodem_actions(
        &mut self,
        session_id: &str,
        actions: Vec<ZmodemAction>,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut root_chrome_dirty = false;
        for action in actions {
            match action {
                ZmodemAction::SendToRemote(bytes) => {
                    if let Err(error) = self.write_session_protocol_response(session_id, &bytes) {
                        self.shell
                            .set_status(format!("ZMODEM write failed: {error}"));
                        root_chrome_dirty = true;
                    }
                }
                ZmodemAction::EmitEvent(event) => {
                    root_chrome_dirty |= self.handle_zmodem_event(session_id, event, cx);
                }
            }
        }
        root_chrome_dirty
    }

    fn handle_zmodem_event(
        &mut self,
        session_id: &str,
        event: ZmodemEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        match event {
            ZmodemEvent::Detected { direction } => {
                self.shell.set_status(match direction {
                    ZmodemDirection::Upload => "ZMODEM upload in progress".to_string(),
                    ZmodemDirection::Download => "ZMODEM download in progress".to_string(),
                });
            }
            ZmodemEvent::Progress {
                file_name,
                bytes_transferred,
                total_size,
                direction,
            } => {
                let dir = match direction {
                    ZmodemDirection::Upload => "↑",
                    ZmodemDirection::Download => "↓",
                };
                if let Some(pct) = bytes_transferred
                    .saturating_mul(100)
                    .checked_div(total_size)
                    .map(|pct| pct.min(100))
                {
                    self.shell.set_status(format!(
                        "ZMODEM {dir} {file_name}: {pct}% ({bytes_transferred}/{total_size})"
                    ));
                } else {
                    self.shell.set_status(format!(
                        "ZMODEM {dir} {file_name}: {bytes_transferred} bytes"
                    ));
                }
                self.upsert_zmodem_transfer_job(
                    ZmodemTransferJobUpdate {
                        session_id,
                        direction,
                        file_name: &file_name,
                        bytes_transferred,
                        total_size,
                        completed: false,
                        fail_reason: None,
                    },
                    cx,
                );
            }
            ZmodemEvent::Complete {
                direction,
                file_count,
            } => {
                let dir = match direction {
                    ZmodemDirection::Upload => "upload",
                    ZmodemDirection::Download => "download",
                };
                self.shell.set_status(format!(
                    "ZMODEM {dir} complete ({file_count} file(s)) [{session_id}]"
                ));
                self.finish_zmodem_transfer_jobs(session_id, true, None, cx);
                if let Some(state) = self.session.zmodem_state_mut(session_id) {
                    state.finish_transfer();
                }
            }
            ZmodemEvent::Failed { reason } => {
                self.shell.set_status(format!("ZMODEM failed: {reason}"));
                self.finish_zmodem_transfer_jobs(session_id, false, Some(reason.as_str()), cx);
                if let Some(state) = self.session.zmodem_state_mut(session_id) {
                    state.finish_transfer();
                }
            }
        }
        true
    }

    fn upsert_zmodem_transfer_job(
        &mut self,
        update: ZmodemTransferJobUpdate<'_>,
        cx: &mut Context<Self>,
    ) {
        let ZmodemTransferJobUpdate {
            session_id,
            direction,
            file_name,
            bytes_transferred,
            total_size,
            completed,
            fail_reason,
        } = update;
        let short = short_id(session_id);
        let kind = match direction {
            ZmodemDirection::Upload => TransferJobKind::ZmodemUpload {
                session_id: session_id.to_string(),
                file_name: file_name.to_string(),
            },
            ZmodemDirection::Download => TransferJobKind::ZmodemDownload {
                session_id: session_id.to_string(),
                file_name: file_name.to_string(),
            },
        };
        let progress = SftpTransferProgress {
            remote_path: format!("zmodem://{short}/{file_name}"),
            local_path: PathBuf::from(file_name),
            bytes_transferred,
            total_bytes: (total_size > 0).then_some(total_size),
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
                    TransferJobKind::ZmodemUpload {
                        session_id: sid,
                        file_name: name
                    }
                    | TransferJobKind::ZmodemDownload {
                        session_id: sid,
                        file_name: name
                    } if sid == session_id && name == file_name
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
            job.detail = if completed {
                "Complete".to_string()
            } else if let Some(reason) = fail_reason {
                format!("Failed: {reason}")
            } else if total_size > 0 {
                format!(
                    "{:.0}%",
                    (bytes_transferred as f64 / total_size as f64 * 100.).clamp(0., 100.)
                )
            } else {
                format!("{bytes_transferred} bytes")
            };
            if completed {
                job.status = TransferJobStatus::Completed;
            } else if fail_reason.is_some() {
                job.status = TransferJobStatus::Failed;
            }
            self.defer_transfer_panel_snapshot_flush(cx);
            return;
        }

        let id = self.transfer.next_transfer_job_id("zmodem");
        let status = if completed {
            TransferJobStatus::Completed
        } else if fail_reason.is_some() {
            TransferJobStatus::Failed
        } else {
            TransferJobStatus::Running
        };
        let detail = fail_reason
            .map(|reason| format!("Failed: {reason}"))
            .unwrap_or_else(|| {
                if completed {
                    "Complete".to_string()
                } else {
                    format!("Transferring {file_name}")
                }
            });
        self.transfer.enqueue_transfer_job(TransferJobState {
            id,
            session_id: Some(session_id.to_string()),
            kind,
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

    fn finish_zmodem_transfer_jobs(
        &mut self,
        session_id: &str,
        success: bool,
        fail_reason: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        self.transfer.visit_transfer_jobs_mut(|job| {
            let is_zmodem = matches!(
                &job.kind,
                TransferJobKind::ZmodemUpload {
                    session_id: sid,
                    ..
                }
                | TransferJobKind::ZmodemDownload {
                    session_id: sid,
                    ..
                } if sid == session_id
            );
            if !is_zmodem {
                return;
            }
            if !matches!(
                job.status,
                TransferJobStatus::Running | TransferJobStatus::Cancelling
            ) {
                return;
            }
            if success {
                job.status = TransferJobStatus::Completed;
                job.detail = "Complete".to_string();
            } else {
                job.status = TransferJobStatus::Failed;
                job.detail = fail_reason
                    .map(|r| format!("Failed: {r}"))
                    .unwrap_or_else(|| "Failed".to_string());
            }
            changed = true;
        });
        if changed {
            self.defer_transfer_panel_snapshot_flush(cx);
        }
    }

    fn prompt_zmodem_upload_files(&mut self, session_id: String, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(SharedString::from("Select ZMODEM upload files")),
        });
        self.shell
            .set_status("selecting ZMODEM upload files…".to_string());
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(Ok(Some(paths))) if !paths.is_empty() => Some(paths),
                _ => None,
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(files) = result {
                    this.accept_zmodem_upload(session_id, files, cx);
                } else {
                    this.cancel_zmodem_transfer(&session_id, cx);
                }
            });
        })
        .detach();
    }

    fn prompt_zmodem_download_directory(&mut self, session_id: String, cx: &mut Context<Self>) {
        let options = PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(SharedString::from("Select ZMODEM download folder")),
        };
        let receiver = cx.prompt_for_paths(options);
        self.shell
            .set_status("selecting ZMODEM download folder…".to_string());
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(dir) = result {
                    this.accept_zmodem_download(session_id, dir, cx);
                } else {
                    this.cancel_zmodem_transfer(&session_id, cx);
                }
            });
        })
        .detach();
    }
}

struct ZmodemTransferJobUpdate<'a> {
    session_id: &'a str,
    direction: ZmodemDirection,
    file_name: &'a str,
    bytes_transferred: u64,
    total_size: u64,
    completed: bool,
    fail_reason: Option<&'a str>,
}

fn run_zmodem_worker(
    mut transfer: ZmodemTransfer,
    command_rx: mpsc::Receiver<ZmodemWorkerCommand>,
    event_tx: mpsc::SyncSender<ZmodemWorkerEvent>,
) {
    while let Ok(command) = command_rx.recv() {
        let Some(event) = process_zmodem_worker_command(&mut transfer, command) else {
            break;
        };
        let done = event.done;
        let _ = event_tx.send(event);
        if done {
            break;
        }
    }
}

fn process_zmodem_worker_command(
    transfer: &mut ZmodemTransfer,
    command: ZmodemWorkerCommand,
) -> Option<ZmodemWorkerEvent> {
    let actions = match command {
        ZmodemWorkerCommand::Input(data) => transfer.feed_incoming(&data),
        ZmodemWorkerCommand::AcceptDownload(save_dir) => transfer.accept_download(save_dir),
        ZmodemWorkerCommand::AcceptUpload(files) => transfer.accept_upload(files),
        ZmodemWorkerCommand::Cancel(reason) => transfer.cancel_with_reason(reason),
        ZmodemWorkerCommand::Stop => return None,
    };
    Some(ZmodemWorkerEvent {
        actions,
        done: transfer.is_done(),
    })
}

fn probe_zmodem_remote_conflicts(
    config: SshSessionConfig,
    remote_dir: String,
    files: Vec<PathBuf>,
    policy: SftpDuplicatePolicy,
    resolver: &dyn SftpDuplicateResolver,
) -> Result<(Vec<PathBuf>, bool, bool), String> {
    let service = SftpService::new(config);
    let entries = match service.list_dir(&remote_dir) {
        Ok(entries) => entries,
        Err(_) => {
            // Tauri: SFTP probe failure/timeout falls through without blocking upload.
            return Ok((files, true, false));
        }
    };
    let existing: HashSet<String> = entries.into_iter().map(|entry| entry.name).collect();
    let mut clean = Vec::new();
    let mut conflicts = Vec::new();
    for path in files {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        if existing.contains(&name) {
            conflicts.push((path, name));
        } else {
            clean.push(path);
        }
    }
    if conflicts.is_empty() {
        return Ok((clean, false, false));
    }

    let join_remote = |name: &str| -> String {
        if remote_dir == "/" {
            format!("/{name}")
        } else {
            format!("{remote_dir}/{name}")
        }
    };

    match policy {
        SftpDuplicatePolicy::Skip => Ok((clean, false, false)),
        SftpDuplicatePolicy::Overwrite | SftpDuplicatePolicy::Rename => {
            let mut all = clean;
            all.extend(conflicts.into_iter().map(|(path, _)| path));
            Ok((all, false, true))
        }
        SftpDuplicatePolicy::Ask => {
            let mut resolved = clean;
            let mut overwrite = false;
            for (path, name) in conflicts {
                let remote_path = join_remote(&name);
                let request = SftpDuplicateRequest {
                    direction: SftpTransferDirection::Upload,
                    source_path: path.display().to_string(),
                    target_path: remote_path.clone(),
                    is_directory: false,
                };
                let decision = resolver
                    .resolve_duplicate(&request)
                    .unwrap_or(SftpDuplicateDecision::Skip);
                match decision {
                    SftpDuplicateDecision::Overwrite | SftpDuplicateDecision::Rename => {
                        overwrite = true;
                        resolved.push(path);
                    }
                    SftpDuplicateDecision::Skip => {}
                }
            }
            Ok((resolved, false, overwrite))
        }
    }
}

const ZMODEM_WORKER_EVENT_CHANNEL_CAP: usize = 256;
const ZMODEM_WORKER_EVENT_DRAIN_BATCH: usize = 32;

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use gpui::{AppContext as _, Context, Subscription, TestAppContext};
    use nyaterm_core::{AiExecutionProfile, AppRuntime, RuntimeMode, uuid};
    use nyaterm_transport::{
        LocalSessionConfig, ZmodemAction, ZmodemDirection, ZmodemEvent, ZmodemTransfer,
    };

    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::features::test_support::app_with_visible_local_session;
    use crate::models::{SessionLaunchConfig, SessionRuntimeMetadata};
    use crate::test_support::TestConfigDir;

    use super::{ZmodemTransferJobUpdate, ZmodemWorkerCommand, process_zmodem_worker_command};

    const SESSION_ID: &str = "zmodem-panel-session";

    struct RootNotifyObserver {
        count: usize,
        _subscription: Subscription,
    }

    impl RootNotifyObserver {
        fn new(app: gpui::Entity<NyaTermApp>, cx: &mut Context<Self>) -> Self {
            let subscription = cx.observe(&app, |observer, _, _| {
                observer.count += 1;
            });
            Self {
                count: 0,
                _subscription: subscription,
            }
        }
    }

    fn app(cx: &mut TestAppContext) -> gpui::Entity<NyaTermApp> {
        let root = std::env::temp_dir().join(format!(
            "nyaterm-zmodem-panel-{}-{}",
            std::process::id(),
            uuid()
        ));
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.clone(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        let app = cx.new(|cx| NyaTermApp::new(runtime, stores, cx));
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.session.register_session_metadata(
                SESSION_ID,
                SessionRuntimeMetadata {
                    ssh_config: None,
                    ssh_multiplex_key: None,
                    source_connection_id: None,
                    ai_execution_profile: AiExecutionProfile::Posix,
                    launch_config: SessionLaunchConfig::Local(LocalSessionConfig::default()),
                    disconnected: false,
                },
            );
            app.session.select_active_session(SESSION_ID);
            app.flush_transfer_panel_snapshot(cx);
        });
        app
    }

    fn root_notify_count(
        observer: &gpui::Entity<RootNotifyObserver>,
        cx: &mut TestAppContext,
    ) -> usize {
        cx.update_entity(observer, |observer, _| observer.count)
    }

    #[test]
    fn idle_zmodem_asterisk_reaches_terminal_without_a_second_output_event() {
        let root = TestConfigDir::new("nyaterm-zmodem-idle-asterisk");
        let mut cx = TestAppContext::single();
        let app = app_with_visible_local_session(&mut cx, root.path(), SESSION_ID);
        cx.update_entity(&app, |app, cx| {
            assert_eq!(
                app.process_zmodem_output(SESSION_ID, b"*", cx),
                (Vec::new(), false)
            );
            assert!(app.session.has_protocol_runtime_sessions());
            let now = Instant::now() + Duration::from_millis(100);
            assert_eq!(
                app.drain_zmodem_idle_output(now, cx),
                (vec![(SESSION_ID.to_string(), b"*".to_vec())], false)
            );
            assert!(app.zmodem_output_can_bypass_detector(SESSION_ID, b"normal output"));
            assert_eq!(app.drain_zmodem_idle_output(now, cx), (Vec::new(), false));
            assert_eq!(
                app.process_zmodem_output(SESSION_ID, b"*", cx),
                (b"*".to_vec(), false)
            );
        });
    }

    #[test]
    fn output_discontinuity_discards_zmodem_prefix_before_idle_flush() {
        let root = TestConfigDir::new("nyaterm-zmodem-idle-reset");
        let mut cx = TestAppContext::single();
        let app = app_with_visible_local_session(&mut cx, root.path(), SESSION_ID);
        cx.update_entity(&app, |app, cx| {
            app.process_zmodem_output(SESSION_ID, b"**", cx);
            app.note_zmodem_output_discontinuity(SESSION_ID, 1, cx);
            assert_eq!(
                app.drain_zmodem_idle_output(Instant::now() + Duration::from_secs(2), cx),
                (Vec::new(), false)
            );
        });
    }

    #[test]
    fn zmodem_worker_cancel_emits_failed_event_off_ui_state() {
        let mut transfer = ZmodemTransfer::new(ZmodemDirection::Download, b"");

        let event = process_zmodem_worker_command(
            &mut transfer,
            ZmodemWorkerCommand::Cancel("stop".into()),
        )
        .expect("cancel should produce a worker event");

        assert!(event.done);
        assert!(event.actions.iter().any(|action| matches!(
            action,
            ZmodemAction::EmitEvent(ZmodemEvent::Failed { reason }) if reason == "stop"
        )));
    }

    #[test]
    fn manual_rz_prompts_for_upload_files_and_starts_worker() {
        let mut cx = TestAppContext::single();
        let app = app(&mut cx);

        cx.update_entity(&app, |app, cx| {
            let (passthrough, dirty) =
                app.process_zmodem_output(SESSION_ID, b"\x18z waiting to receive.**\x18B01", cx);
            assert_eq!(passthrough, b"\x18z waiting to receive.");
            assert!(dirty);
            assert!(
                app.session
                    .zmodem_state(SESSION_ID)
                    .unwrap()
                    .transfer
                    .is_some()
            );
        });
        assert!(cx.did_prompt_for_paths());

        cx.simulate_path_prompt_response(|options| {
            assert!(options.files);
            assert!(!options.directories);
            assert!(options.multiple);
            Some(vec![std::path::PathBuf::from("upload.txt")])
        });
        cx.run_until_parked();

        cx.update_entity(&app, |app, _| {
            let state = app.session.zmodem_state(SESSION_ID).unwrap();
            assert!(state.transfer.is_none());
            assert!(state.worker.is_some());
        });
    }

    #[test]
    fn prepared_zmodem_upload_starts_without_another_picker() {
        let mut cx = TestAppContext::single();
        let app = app(&mut cx);

        cx.update_entity(&app, |app, cx| {
            app.zmodem_state_mut(SESSION_ID).pending_upload =
                Some(vec![std::path::PathBuf::from("upload.txt")]);
            let (_, dirty) = app.process_zmodem_output(SESSION_ID, b"**\x18B01", cx);
            assert!(dirty);
            let state = app.session.zmodem_state(SESSION_ID).unwrap();
            assert!(state.pending_upload.is_none());
            assert!(state.transfer.is_none());
            assert!(state.worker.is_some());
        });

        assert!(!cx.did_prompt_for_paths());
    }

    #[test]
    fn zmodem_progress_refreshes_transfer_panel_without_root_notify() {
        let mut cx = TestAppContext::single();
        let app = app(&mut cx);
        let observer = cx.new(|cx| RootNotifyObserver::new(app.clone(), cx));

        cx.update_entity(&app, |app, cx| {
            assert_eq!(app.transfer_panel.read(cx).queue_row_count_for_test(), 0);
            app.upsert_zmodem_transfer_job(
                ZmodemTransferJobUpdate {
                    session_id: SESSION_ID,
                    direction: ZmodemDirection::Upload,
                    file_name: "artifact.bin",
                    bytes_transferred: 7,
                    total_size: 10,
                    completed: false,
                    fail_reason: None,
                },
                cx,
            );
        });
        cx.run_until_parked();

        cx.update_entity(&app, |app, cx| {
            assert_eq!(
                app.transfer_panel.read(cx).queue_row_count_for_test(),
                1,
                "ZMODEM progress should refresh TransferPanel through its own snapshot"
            );
        });
        assert_eq!(
            root_notify_count(&observer, &mut cx),
            0,
            "transfer progress should not rely on a root app notification"
        );
    }
}
