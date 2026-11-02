use super::transfer_wire::TransferWire;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use gpui::{Context, PathPromptOptions, SharedString, Window};
use nyaterm_transport::xymodem::{
    XymodemAction, XymodemError, XymodemOptions, XymodemProgress, XymodemProtocol, XymodemSender,
};
use nyaterm_transport::{SessionKind, SftpTransferProgress};
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::models::{TransferJobKind, TransferJobState, TransferJobStatus};

pub(super) struct XymodemSessionState {
    worker: XymodemWorker,
}

#[derive(Clone, Copy)]
pub(in crate::features) enum SerialUploadProtocol {
    Xmodem,
    Ymodem,
    Zmodem,
}

struct XymodemWorker {
    wire: TransferWire,
    command_tx: mpsc::Sender<XymodemWorkerCommand>,
    event_rx: Option<mpsc::Receiver<XymodemWorkerEvent>>,
    worker: Option<thread::JoinHandle<()>>,
}

enum XymodemWorkerCommand {
    Input(Vec<u8>),
    Cancel,
    Stop,
}
struct XymodemWorkerEvent {
    actions: Vec<XymodemAction>,
    done: bool,
}

struct XymodemJobFailure {
    message: String,
    cancelled: bool,
}

impl XymodemWorker {
    #[cfg(test)]
    fn spawn(protocol: XymodemProtocol, files: Vec<PathBuf>) -> Self {
        Self::spawn_connected(protocol, files, TransferWire::default())
    }

    fn spawn_connected(protocol: XymodemProtocol, files: Vec<PathBuf>, wire: TransferWire) -> Self {
        let worker_wire = wire.clone();
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::sync_channel(64);
        let worker = thread::Builder::new()
            .name("nyaterm-xymodem-upload".into())
            .spawn(move || {
                let mut sender =
                    match XymodemSender::from_paths(protocol, &files, XymodemOptions::default()) {
                        Ok(sender) => sender,
                        Err(error) => {
                            let _ = event_tx.send(XymodemWorkerEvent {
                                actions: vec![XymodemAction::Finished(Err(error))],
                                done: true,
                            });
                            return;
                        }
                    };
                let started = Instant::now();
                loop {
                    let mut cancelling = false;
                    let mut actions = match command_rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(XymodemWorkerCommand::Input(data)) => {
                            sender.feed(&data, started.elapsed())
                        }
                        Ok(XymodemWorkerCommand::Cancel) => {
                            cancelling = true;
                            sender.cancel()
                        }
                        Ok(XymodemWorkerCommand::Stop)
                        | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => sender.tick(started.elapsed()),
                    };
                    let mut remaining = Vec::new();
                    let mut failed = false;
                    for action in actions {
                        if let XymodemAction::Send(bytes) = action {
                            match if cancelling {
                                worker_wire.send_abort(&bytes)
                            } else {
                                worker_wire.send(&bytes)
                            } {
                                Ok(true) => {}
                                Ok(false) => remaining.push(XymodemAction::Send(bytes)),
                                Err(_) => {
                                    failed = true;
                                    break;
                                }
                            }
                        } else {
                            remaining.push(action);
                        }
                    }
                    actions = if failed {
                        vec![XymodemAction::Finished(Err(XymodemError::WriteFailed))]
                    } else {
                        remaining
                    };
                    let done = sender.is_done() || failed;
                    if !actions.is_empty()
                        && event_tx.send(XymodemWorkerEvent { actions, done }).is_err()
                    {
                        break;
                    }
                    if done {
                        break;
                    }
                }
            })
            .expect("failed to spawn X/YMODEM worker");
        Self {
            wire,
            command_tx,
            event_rx: Some(event_rx),
            worker: Some(worker),
        }
    }

    fn input(&self, data: Vec<u8>) {
        if !data.is_empty() {
            let _ = self.command_tx.send(XymodemWorkerCommand::Input(data));
        }
    }
    fn cancel(&self) {
        self.wire.stop();
        let _ = self.command_tx.send(XymodemWorkerCommand::Cancel);
    }
    fn try_recv(&self) -> Option<XymodemWorkerEvent> {
        self.event_rx.as_ref()?.try_recv().ok()
    }
    fn shutdown(&mut self) {
        self.wire.stop();
        self.event_rx.take();
        let _ = self.command_tx.send(XymodemWorkerCommand::Stop);
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::warn!("X/YMODEM worker panicked during shutdown");
        }
    }
}

impl Drop for XymodemWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}
impl XymodemSessionState {
    pub(super) fn stop_worker(&mut self) {
        self.worker.shutdown();
    }
}

impl NyaTermApp {
    pub(in crate::features) fn prompt_serial_upload_files(
        &mut self,
        session_id: String,
        protocol: SerialUploadProtocol,
        cx: &mut Context<Self>,
    ) {
        let multiple = !matches!(protocol, SerialUploadProtocol::Xmodem);
        let label = match protocol {
            SerialUploadProtocol::Xmodem => "XMODEM",
            SerialUploadProtocol::Ymodem => "YMODEM",
            SerialUploadProtocol::Zmodem => "ZMODEM",
        };
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple,
            prompt: Some(SharedString::from(
                if multiple {
                    t!("terminalCtx.serialUploadFilesPrompt", protocol = label)
                } else {
                    t!("terminalCtx.serialUploadFilePrompt", protocol = label)
                }
                .to_string(),
            )),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| match protocol {
                SerialUploadProtocol::Xmodem => {
                    this.start_serial_upload(session_id, XymodemProtocol::Xmodem, paths, cx)
                }
                SerialUploadProtocol::Ymodem => {
                    this.start_serial_upload(session_id, XymodemProtocol::Ymodem, paths, cx)
                }
                SerialUploadProtocol::Zmodem => this.start_zmodem_upload(session_id, paths, cx),
            });
        })
        .detach();
    }

    pub(in crate::features) fn open_serial_upload_protocol_dialog(
        &mut self,
        session_id: String,
        files: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui::{IntoElement as _, ParentElement as _, Styled as _, div};
        use nyaterm_ui::{NyaButton, NyaButtonVariant, NyaDialogWindowExt as _};
        let x_files = files.clone();
        let y_files = files.clone();
        let z_files = files;
        self.open_content_dialog(
            t!("terminalCtx.serialUploadProtocol").to_string(),
            360.,
            move |_, _, cx| {
                let session_x = session_id.clone();
                let session_y = session_id.clone();
                let session_z = session_id.clone();
                let click_x_files = x_files.clone();
                let click_y_files = y_files.clone();
                let click_z_files = z_files.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        NyaButton::new("serial-upload-xmodem", "XMODEM")
                            .icon("icons/fe/upload.svg")
                            .variant(NyaButtonVariant::Secondary)
                            .disabled(x_files.len() != 1)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                window.close_nya_dialog(cx);
                                this.start_serial_upload(
                                    session_x.clone(),
                                    XymodemProtocol::Xmodem,
                                    click_x_files.clone(),
                                    cx,
                                );
                            })),
                    )
                    .child(
                        NyaButton::new("serial-upload-ymodem", "YMODEM")
                            .icon("icons/fe/upload.svg")
                            .variant(NyaButtonVariant::Secondary)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                window.close_nya_dialog(cx);
                                this.start_serial_upload(
                                    session_y.clone(),
                                    XymodemProtocol::Ymodem,
                                    click_y_files.clone(),
                                    cx,
                                );
                            })),
                    )
                    .child(
                        NyaButton::new("serial-upload-zmodem", "ZMODEM")
                            .icon("icons/fe/upload.svg")
                            .variant(NyaButtonVariant::Secondary)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                window.close_nya_dialog(cx);
                                this.start_zmodem_upload(
                                    session_z.clone(),
                                    click_z_files.clone(),
                                    cx,
                                );
                            })),
                    )
                    .w_full()
                    .p_2()
                    .into_any_element()
            },
            |_, _| {},
            window,
            cx,
        );
    }

    pub(in crate::features) fn start_serial_upload(
        &mut self,
        session_id: String,
        protocol: XymodemProtocol,
        files: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if !self.session.manager().supports_modem_transfer(&session_id) {
            self.shell
                .set_status(self.modem_transfer_disabled_message(&session_id));
            cx.notify();
            return;
        }
        if files.is_empty() {
            return;
        }
        let kind = self
            .session
            .ordered_sessions()
            .into_iter()
            .find(|session| session.id == session_id)
            .map(|session| session.kind);
        if kind != Some(SessionKind::Serial) || self.session.is_disconnected(&session_id) {
            self.shell.set_status(
                t!(
                    "terminalCtx.serialUploadRequiresConnected",
                    protocol = protocol.label()
                )
                .to_string(),
            );
            cx.notify();
            return;
        }
        if protocol == XymodemProtocol::Xmodem && files.len() != 1 {
            self.shell
                .set_status(t!("terminalCtx.xmodemSingleFile").to_string());
            cx.notify();
            return;
        }
        if self.session.xymodem_state(&session_id).is_some()
            || self
                .session
                .zmodem_state(&session_id)
                .is_some_and(|state| state.is_active())
        {
            self.shell
                .set_status(t!("terminalCtx.serialTransferActive").to_string());
            cx.notify();
            return;
        }
        let display_name = files
            .first()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("upload")
            .to_string();
        let id = self.transfer.next_transfer_job_id(match protocol {
            XymodemProtocol::Xmodem => "xmodem",
            XymodemProtocol::Ymodem => "ymodem",
        });
        let kind = match protocol {
            XymodemProtocol::Xmodem => TransferJobKind::XmodemUpload {
                session_id: session_id.clone(),
                file_name: display_name,
            },
            XymodemProtocol::Ymodem => TransferJobKind::YmodemUpload {
                session_id: session_id.clone(),
                file_name: display_name,
            },
        };
        self.transfer.enqueue_transfer_job(TransferJobState {
            id,
            session_id: Some(session_id.clone()),
            kind,
            status: TransferJobStatus::Running,
            detail: t!(
                "terminalCtx.serialUploadWaiting",
                protocol = protocol.label()
            )
            .to_string(),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
            speed: Default::default(),
        });
        self.session.insert_xymodem_state(
            session_id.clone(),
            XymodemSessionState {
                worker: XymodemWorker::spawn_connected(
                    protocol,
                    files,
                    TransferWire::connected(self.session.manager_handle(), session_id.clone()),
                ),
            },
        );
        self.sync_session_event_bridge_session_policy(&session_id);
        self.shell.set_status(
            t!(
                "terminalCtx.serialUploadStarted",
                protocol = protocol.label()
            )
            .to_string(),
        );
        cx.notify();
    }

    pub(in crate::features) fn clear_xymodem_session(&mut self, session_id: &str) {
        if self.session.remove_xymodem_session_runtime(session_id) {
            self.sync_session_event_bridge_session_policy(session_id);
        }
    }
    pub(in crate::features) fn cancel_xymodem_transfer(&mut self, session_id: &str) {
        if let Some(state) = self.session.xymodem_state(session_id) {
            state.worker.cancel();
        }
    }
    pub(in crate::features) fn xymodem_transfer_active(&self, session_id: &str) -> bool {
        self.session.xymodem_state(session_id).is_some()
    }

    pub(in crate::features) fn process_xymodem_output(
        &mut self,
        session_id: &str,
        data: &[u8],
    ) -> Option<Vec<u8>> {
        let state = self.session.xymodem_state(session_id)?;
        state.worker.input(data.to_vec());
        Some(Vec::new())
    }

    pub(in crate::features) fn drain_xymodem_worker_events(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut events = Vec::new();
        for (session_id, state) in self.session.xymodem_states() {
            while let Some(event) = state.worker.try_recv() {
                events.push((session_id.clone(), event));
                if events.len() >= 64 {
                    break;
                }
            }
            if events.len() >= 64 {
                break;
            }
        }
        if events.is_empty() {
            return false;
        }
        let mut dirty = false;
        for (session_id, event) in events {
            for action in event.actions {
                match action {
                    XymodemAction::Send(bytes) => {
                        if let Err(error) =
                            self.write_session_protocol_response(&session_id, &bytes)
                        {
                            self.finish_xymodem_job(
                                &session_id,
                                Err(XymodemJobFailure {
                                    message: error,
                                    cancelled: false,
                                }),
                                cx,
                            );
                        }
                    }
                    XymodemAction::Progress(progress) => {
                        self.update_xymodem_job(&session_id, progress);
                        self.flush_transfer_panel_snapshot(cx);
                    }
                    XymodemAction::Finished(result) => {
                        self.finish_xymodem_job(
                            &session_id,
                            result.map_err(xymodem_job_failure),
                            cx,
                        );
                        dirty = true;
                    }
                }
            }
            if event.done {
                self.clear_xymodem_session(&session_id);
            }
        }
        dirty
    }

    fn update_xymodem_job(&mut self, session_id: &str, progress: XymodemProgress) {
        let Some(job) = self
            .transfer
            .transfer_jobs_mut_for_protocol(session_id, true)
            .next()
        else {
            return;
        };
        let protocol = match job.kind {
            TransferJobKind::XmodemUpload { .. } => "xmodem",
            _ => "ymodem",
        };
        job.detail = t!(
            "terminalCtx.serialUploadProgress",
            protocol = protocol.to_uppercase(),
            completed = progress.files_completed,
            total = progress.files_total
        )
        .to_string();
        job.update_progress(SftpTransferProgress {
            remote_path: format!(
                "{protocol}://{}/{}",
                crate::features::formatting::short_id(session_id),
                progress.file_name
            ),
            local_path: PathBuf::from(progress.file_name),
            bytes_transferred: progress.bytes_transferred,
            total_bytes: Some(progress.total_bytes),
            item_count_completed: Some(progress.files_completed as u64),
            item_count_total: Some(progress.files_total as u64),
        });
    }

    fn finish_xymodem_job(
        &mut self,
        session_id: &str,
        result: Result<(), XymodemJobFailure>,
        cx: &mut Context<Self>,
    ) {
        if let Some(job) = self
            .transfer
            .transfer_jobs_mut_for_protocol(session_id, true)
            .next()
        {
            job.progress = None;
            job.control = None;
            job.speed.reset();
            match result {
                Ok(()) => {
                    job.status = TransferJobStatus::Completed;
                    job.detail = t!("fileTransfer.completed").to_string();
                }
                Err(error) => {
                    job.status = if error.cancelled {
                        TransferJobStatus::Cancelled
                    } else {
                        TransferJobStatus::Failed
                    };
                    job.detail = error.message;
                }
            }
        }
        self.flush_transfer_panel_snapshot(cx);
    }
}

fn xymodem_job_failure(error: XymodemError) -> XymodemJobFailure {
    let message = match error {
        XymodemError::InvalidOptions => t!("terminalCtx.xymodemInvalidOptions"),
        XymodemError::SingleFileRequired => t!("terminalCtx.xmodemSingleFile"),
        XymodemError::NoFiles => t!("terminalCtx.xymodemNoFiles"),
        XymodemError::InvalidFilename => t!("terminalCtx.xymodemInvalidFilename"),
        XymodemError::OpenFailed => t!("terminalCtx.xymodemOpenFailed"),
        XymodemError::WriteFailed => "Unable to write transfer data".into(),
        XymodemError::ReadFailed => t!("terminalCtx.xymodemReadFailed"),
        XymodemError::SizeOverflow => t!("terminalCtx.xymodemSizeOverflow"),
        XymodemError::TimedOut => t!("terminalCtx.xymodemTimedOut"),
        XymodemError::RetryLimit => t!("terminalCtx.xymodemRetryLimit"),
        XymodemError::Cancelled => t!("terminalCtx.xymodemCancelled"),
        XymodemError::RemoteCancelled => t!("terminalCtx.xymodemRemoteCancelled"),
        XymodemError::StreamingUnsupported => t!("terminalCtx.xymodemStreamingUnsupported"),
    }
    .to_string();
    XymodemJobFailure {
        message,
        cancelled: matches!(
            error,
            XymodemError::Cancelled | XymodemError::RemoteCancelled
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use nyaterm_transport::xymodem::{XymodemError, XymodemProtocol};

    use super::{XymodemWorker, xymodem_job_failure};
    use crate::test_support::TestConfigDir;

    #[test]
    fn protocol_worker_can_stop_and_restart_without_retaining_file_handles() {
        let temp = TestConfigDir::new("nyaterm-xymodem-worker-test");
        fs::create_dir(temp.path()).expect("create worker test directory");
        let path = temp.path().join("upload.bin");
        fs::write(&path, b"upload").expect("write worker test file");

        let mut first = XymodemWorker::spawn(XymodemProtocol::Xmodem, vec![path.clone()]);
        first.shutdown();

        let mut second = XymodemWorker::spawn(XymodemProtocol::Ymodem, vec![path.clone()]);
        second.shutdown();

        fs::remove_file(path).expect("workers must release the upload file before returning");
    }

    #[test]
    fn job_failure_classifies_only_local_and_remote_cancel_as_cancelled() {
        for error in [XymodemError::Cancelled, XymodemError::RemoteCancelled] {
            let failure = xymodem_job_failure(error);
            assert!(failure.cancelled);
            assert!(!failure.message.trim().is_empty());
        }

        for error in [XymodemError::TimedOut, XymodemError::RetryLimit] {
            let failure = xymodem_job_failure(error);
            assert!(!failure.cancelled);
            assert!(!failure.message.trim().is_empty());
        }
    }
}
