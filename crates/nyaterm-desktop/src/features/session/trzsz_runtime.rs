use super::transfer_wire::TransferWire;
use crate::features::NyaTermApp;
use crate::features::formatting::short_id;
use crate::models::{
    TransferJobKind, TransferJobState, TransferJobStatus, TransferPathPromptKind,
    TransferPathPromptResult,
};
use gpui::{Context, PathPromptOptions, SharedString};
use nyaterm_files::sftp_transfer_types::SftpTransferProgress;
use nyaterm_transport::trzsz::runtime::is_trzsz_upload_frame;
use nyaterm_transport::trzsz::runtime::{
    TrzszDownloadProgressUpdate, TrzszDownloadWorker, TrzszDownloadWorkerEvent, TrzszUploadFile,
    TrzszUploadPrepareWorker, TrzszUploadProgressUpdate, TrzszUploadRuntime, TrzszUploadWorker,
    TrzszUploadWorkerEvent,
};
use nyaterm_transport::{
    TrzszAction, TrzszDetector, TrzszMode, TrzszOutputEvent, TrzszProtocolFrame,
    TrzszProtocolStream, TrzszTransferEvent, TrzszTransferState, TrzszTrigger, TrzszUploadEntry,
    build_trzsz_action_frame, trzsz_fail_response,
};
use std::{collections::HashMap, path::PathBuf, time::Instant};

pub(super) struct TrzszSessionState {
    detector: TrzszDetector,
    transfer: TrzszTransferState,
    protocol: TrzszProtocolStream,
    protocol_active: bool,
    download_worker: Option<TrzszDownloadWorker>,
    upload_prepare_worker: Option<TrzszUploadPrepareWorker>,
    upload: Option<TrzszUploadRuntime>,
    upload_worker: Option<TrzszUploadWorker>,
}

impl Default for TrzszSessionState {
    fn default() -> Self {
        Self {
            detector: TrzszDetector::new(),
            transfer: TrzszTransferState::new(),
            protocol: TrzszProtocolStream::new(),
            protocol_active: false,
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
        self.upload = None;
        self.stop_workers();
    }

    fn finish_download(&mut self) {
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

    fn begin_download(&mut self, trigger: &TrzszTrigger, directory: PathBuf, wire: TransferWire) {
        self.begin_trigger(trigger);
        self.download_worker = Some(TrzszDownloadWorker::spawn_connected(
            directory,
            trigger.remote_is_windows,
            wire,
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
        self.upload = Some(TrzszUploadRuntime::new(
            remote_is_windows,
            directory_mode,
            entries,
            files,
        ));
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
                        let wire = TransferWire::connected(
                            self.session.manager_handle(),
                            session_id.to_string(),
                        );
                        self.trzsz_state_mut(session_id).begin_download(
                            &trigger,
                            directory.clone(),
                            wire,
                        );
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

    pub(in crate::features) fn trzsz_output_ready(&mut self, session_id: &str) -> bool {
        let Some(state) = self.session.trzsz_state_mut(session_id) else {
            return true;
        };
        let download_ready = state
            .download_worker
            .as_mut()
            .is_none_or(|worker| worker.ready());
        let upload_ready = state
            .upload_worker
            .as_mut()
            .is_none_or(|worker| worker.ready());
        download_ready && upload_ready
    }

    fn queue_trzsz_download_worker_output(&mut self, session_id: &str, data: &[u8]) -> bool {
        let Some(state) = self.session.trzsz_state_mut(session_id) else {
            return false;
        };
        let Some(worker) = state.download_worker.as_mut() else {
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
        let Some(worker) = state.upload_worker.as_mut() else {
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
        mut event: TrzszDownloadWorkerEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let trailing = event.take_trailing_output(
            self.session
                .trzsz_state_mut(session_id)
                .and_then(|state| state.download_worker.as_mut()),
        );
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
        if !trailing.is_empty() {
            self.session
                .defer_pending_event(nyaterm_transport::SessionEvent::Output {
                    session_id: session_id.to_string(),
                    data: trailing,
                });
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
                if self
                    .session
                    .trzsz_state(session_id)
                    .is_some_and(|state| state.upload.is_some())
                {
                    if let Some(upload) = self
                        .session
                        .trzsz_state_mut(session_id)
                        .and_then(|state| state.upload.as_mut())
                        && let Err(error) = upload.configure(&config)
                    {
                        self.fail_trzsz_upload(session_id, &error, responses, status, cx);
                        return;
                    }
                    let expected_directory = self
                        .session
                        .trzsz_state(session_id)
                        .and_then(|state| state.upload.as_ref())
                        .is_some_and(|upload| upload.directory_mode());
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
        }
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
        let mut worker = TrzszUploadWorker::spawn_connected(
            upload,
            remote_is_windows,
            TransferWire::connected(self.session.manager_handle(), session_id.to_string()),
        );
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

const TRZSZ_DOWNLOAD_WORKER_EVENT_DRAIN_BATCH: usize = 32;
const TRZSZ_UPLOAD_WORKER_EVENT_DRAIN_BATCH: usize = 32;

#[cfg(test)]
mod tests {
    use crate::features::test_support::app_with_visible_local_session;
    use crate::test_support::TestConfigDir;
    use gpui::{AppContext as _, TestAppContext};
    use nyaterm_transport::TrzszProtocolPayload;
    use std::time::{Duration, Instant};
    #[test]
    fn idle_zmodem_output_stays_in_active_trzsz_protocol() {
        let root = TestConfigDir::new("nyaterm-zmodem-idle-trzsz");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
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
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
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
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
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
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
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
}
