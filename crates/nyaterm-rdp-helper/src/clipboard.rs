use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use ironrdp_client::rdp::RdpInputSender;
use ironrdp_cliprdr::backend::{ClipboardMessage, CliprdrBackend};
use ironrdp_cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardGeneralCapabilityFlags, FORMAT_NAME_FILE_LIST,
    FileContentsRequest, FileContentsResponse, FormatDataRequest, FormatDataResponse, LockDataId,
    OwnedFormatDataResponse,
};
use ironrdp_cliprdr::{Cliprdr, CliprdrClient};
use ironrdp_core::impl_as_any;
use nyaterm_remote_desktop::{
    MAX_CLIPBOARD_TEXT_BYTES, RdpClipboardTransferProgress, RdpClipboardTransferStatus,
    RdpControlMessage,
};

use super::Outbound;

mod fetch;
mod files;

use fetch::{FetchKind, FetchReply, RemoteFetchState};
use files::{FileState, build_local_snapshot, file_list_fingerprint, read_clipboard_paths};

const REMOTE_FETCH_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) struct ClipboardBridge {
    session_id: String,
    output_tx: mpsc::SyncSender<Outbound>,
    state: Mutex<ClipboardState>,
    files: Mutex<FileState>,
    fetch: Mutex<RemoteFetchState>,
    file_enabled: bool,
    file_available: AtomicBool,
    watcher_started: AtomicBool,
    shutdown: AtomicBool,
}

#[derive(Default)]
struct ClipboardState {
    local_text: String,
    input: Option<ClipboardInput>,
    generation: u64,
}

#[derive(Clone)]
enum ClipboardInput {
    Rdp(RdpInputSender),
    #[cfg(test)]
    Test(mpsc::Sender<ClipboardMessage>),
}

impl ClipboardInput {
    fn send_clipboard(&self, message: ClipboardMessage) -> anyhow::Result<()> {
        match self {
            Self::Rdp(sender) => sender.send_clipboard(message).map_err(|_| ()),
            #[cfg(test)]
            Self::Test(sender) => sender.send(message).map_err(|_| ()),
        }
        .map_err(|_| anyhow::anyhow!("IronRDP clipboard input channel closed"))
    }
}

impl fmt::Debug for ClipboardBridge {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClipboardBridge")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

impl ClipboardBridge {
    pub(super) fn new(
        session_id: String,
        output_tx: mpsc::SyncSender<Outbound>,
        file_enabled: bool,
    ) -> Arc<Self> {
        files::cleanup_stale_cache(&files::cache_root(), files::CACHE_RETENTION);
        Arc::new(Self {
            session_id,
            output_tx,
            state: Mutex::new(ClipboardState::default()),
            files: Mutex::new(FileState::default()),
            fetch: Mutex::new(RemoteFetchState::default()),
            file_enabled,
            file_available: AtomicBool::new(false),
            watcher_started: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
        })
    }

    pub(super) fn set_input_sender(&self, sender: RdpInputSender) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .input = Some(ClipboardInput::Rdp(sender));
    }

    pub(super) fn set_local_text(&self, text: String) -> anyhow::Result<()> {
        if text.len() > MAX_CLIPBOARD_TEXT_BYTES {
            anyhow::bail!("clipboard text exceeds the 4 MiB limit");
        }
        let sender = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.local_text = text;
            state.input.clone()
        };
        if let Some(sender) = sender {
            if self.file_available.load(Ordering::SeqCst)
                && read_clipboard_paths().is_some_and(|paths| !paths.is_empty())
            {
                return Ok(());
            }
            sender.send_clipboard(ClipboardMessage::SendInitiateCopy(vec![
                ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT),
            ]))?;
        }
        Ok(())
    }

    pub(super) fn cliprdr_client(self: &Arc<Self>) -> CliprdrClient {
        Cliprdr::new(Box::new(TextClipboardBackend {
            bridge: Arc::clone(self),
        }))
    }

    fn send_clipboard_message(&self, message: ClipboardMessage) {
        let sender = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .input
            .clone();
        if let Some(sender) = sender {
            let _ = sender.send_clipboard(message);
        }
    }

    fn send_paste(&self, format: ClipboardFormatId) {
        self.send_clipboard_message(ClipboardMessage::SendInitiatePaste(format));
    }

    fn queue_remote_fetch(&self, desired: Option<(FetchKind, ClipboardFormatId)>) {
        let next = self
            .fetch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .begin_format_list(desired);
        if let Some(format) = next {
            self.send_paste(format);
        }
    }

    fn finish_remote_fetch(&self, kind: FetchKind, is_error: bool) -> FetchReply {
        let reply = self
            .fetch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .finish_reply(kind, is_error);
        if let Some(format) = reply.next {
            self.send_paste(format);
        }
        reply
    }

    pub(super) fn check_remote_fetch_timeout(&self) {
        self.expire_remote_fetch(REMOTE_FETCH_TIMEOUT);
    }

    fn expire_remote_fetch(&self, timeout: Duration) {
        let timed_out = self
            .fetch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .expire(timeout);
        if timed_out {
            let _ = self.output_tx.send(Outbound::control(RdpControlMessage::Error {
                session_id: self.session_id.clone(),
                error: nyaterm_remote_desktop::RdpError::new(
                    nyaterm_remote_desktop::RdpErrorKind::Clipboard,
                    "RDP clipboard response timed out; waiting for the outstanding response. Reconnect if clipboard copies remain unavailable.",
                ),
                fatal: false,
            }));
        }
    }

    fn publish_remote_text(&self, text: String) {
        if text.len() > MAX_CLIPBOARD_TEXT_BYTES {
            return;
        }
        let generation = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.generation = state.generation.wrapping_add(1);
            state.generation
        };
        let _ = self
            .output_tx
            .send(Outbound::control(RdpControlMessage::Clipboard {
                session_id: self.session_id.clone(),
                text,
                generation,
            }));
    }

    fn local_text(&self) -> String {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .local_text
            .clone()
    }

    fn transfer_progress(
        &self,
        status: RdpClipboardTransferStatus,
        error: Option<String>,
    ) -> Option<RdpClipboardTransferProgress> {
        self.files.lock().ok().and_then(|files| {
            files
                .remote
                .as_ref()
                .map(|transfer| RdpClipboardTransferProgress {
                    id: transfer.id.clone(),
                    name: transfer.name.clone(),
                    status,
                    total_bytes: transfer.total_bytes,
                    transferred_bytes: transfer.transferred_bytes,
                    total_files: transfer.total_files,
                    completed_files: transfer.completed_files,
                    error,
                })
        })
    }

    fn send_transfer_progress(&self, progress: RdpClipboardTransferProgress) {
        let _ = self
            .output_tx
            .send(Outbound::control(RdpControlMessage::ClipboardTransfer {
                session_id: self.session_id.clone(),
                progress,
            }));
    }

    fn publish_transfer(&self, status: RdpClipboardTransferStatus, error: Option<String>) {
        if let Some(progress) = self.transfer_progress(status, error) {
            self.send_transfer_progress(progress);
        }
    }

    fn cancel_remote(&self, reason: &str) {
        self.publish_transfer(
            RdpClipboardTransferStatus::Cancelled,
            Some(reason.to_string()),
        );
        self.files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cancel_remote();
    }

    pub(super) fn cancel_transfer(&self, transfer_id: &str) {
        let progress = self.transfer_progress(
            RdpClipboardTransferStatus::Cancelled,
            Some("RDP clipboard transfer cancelled".to_string()),
        );
        let cancelled = self
            .files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cancel_if_id(transfer_id);
        if cancelled && let Some(progress) = progress {
            self.send_transfer_progress(progress);
        }
    }

    pub(super) fn stop(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.cancel_remote("RDP session disconnected");
        self.files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .shutdown();
    }

    fn start_watcher(self: &Arc<Self>) {
        if !self.file_enabled || self.watcher_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let bridge = Arc::clone(self);
        std::thread::spawn(move || {
            while !bridge.shutdown.load(Ordering::SeqCst) {
                if bridge.file_available.load(Ordering::SeqCst) {
                    bridge.poll_local_files();
                }
                let expired = bridge
                    .files
                    .lock()
                    .ok()
                    .is_some_and(|files| files.is_expired());
                if expired {
                    bridge.publish_transfer(
                        RdpClipboardTransferStatus::Cancelled,
                        Some("RDP clipboard transfer timed out".to_string()),
                    );
                    bridge
                        .files
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .expire();
                }
                std::thread::sleep(Duration::from_millis(750));
            }
        });
    }

    fn poll_local_files(&self) {
        let Some(paths) = read_clipboard_paths().filter(|paths| !paths.is_empty()) else {
            self.files
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .release_completed();
            return;
        };
        let hash = file_list_fingerprint(&paths);
        let should_publish = {
            let mut files = self
                .files
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if files.is_remote_clipboard(hash) {
                return;
            }
            files.release_completed();
            if files.last_local_hash == Some(hash) {
                false
            } else {
                files.last_local_hash = Some(hash);
                true
            }
        };
        if !should_publish {
            return;
        }
        if let Ok(snapshot) = build_local_snapshot(&paths) {
            let descriptors = self
                .files
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .set_local_snapshot(snapshot, hash);
            self.send_clipboard_message(ClipboardMessage::SendInitiateFileCopy(descriptors));
        }
    }

    fn current_advertisement(&self) -> ClipboardMessage {
        if self.file_available.load(Ordering::SeqCst)
            && let Some(paths) = read_clipboard_paths().filter(|paths| !paths.is_empty())
            && let Ok(snapshot) = build_local_snapshot(&paths)
        {
            let hash = file_list_fingerprint(&paths);
            let descriptors = self
                .files
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .set_local_snapshot(snapshot, hash);
            ClipboardMessage::SendInitiateFileCopy(descriptors)
        } else {
            self.initial_advertisement()
        }
    }

    fn initial_advertisement(&self) -> ClipboardMessage {
        let formats = if self.local_text().is_empty() {
            Vec::new()
        } else {
            vec![ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)]
        };
        ClipboardMessage::SendInitiateCopy(formats)
    }

    fn advertise_initial(&self) {
        // The initial CLIPRDR FormatList is required even when the clipboard is empty.
        // File copy is valid only after IronRDP receives the FormatList response.
        self.send_clipboard_message(self.initial_advertisement());
    }

    fn advertise_current(&self) {
        self.send_clipboard_message(self.current_advertisement());
    }
}

#[derive(Debug)]
struct TextClipboardBackend {
    bridge: Arc<ClipboardBridge>,
}

impl_as_any!(TextClipboardBackend);

impl CliprdrBackend for TextClipboardBackend {
    fn temporary_directory(&self) -> &str {
        ".nyaterm-cliprdr"
    }

    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        let mut capabilities = ClipboardGeneralCapabilityFlags::USE_LONG_FORMAT_NAMES;
        if self.bridge.file_enabled {
            capabilities |= ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED
                | ClipboardGeneralCapabilityFlags::FILECLIP_NO_FILE_PATHS
                | ClipboardGeneralCapabilityFlags::CAN_LOCK_CLIPDATA;
        }
        capabilities
    }

    fn on_ready(&mut self) {
        self.bridge.advertise_current();
        self.bridge.start_watcher();
    }

    fn on_request_format_list(&mut self) {
        self.bridge.advertise_initial();
    }

    fn on_process_negotiated_capabilities(
        &mut self,
        capabilities: ClipboardGeneralCapabilityFlags,
    ) {
        self.bridge.file_available.store(
            self.bridge.file_enabled
                && capabilities.contains(ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED),
            Ordering::SeqCst,
        );
    }

    fn on_remote_copy(&mut self, available_formats: &[ClipboardFormat]) {
        self.bridge.cancel_remote("Remote clipboard changed");
        let file_format = self
            .bridge
            .file_available
            .load(Ordering::SeqCst)
            .then(|| {
                available_formats.iter().find(|format| {
                    format
                        .name
                        .as_ref()
                        .is_some_and(|name| name.value() == FORMAT_NAME_FILE_LIST)
                })
            })
            .flatten()
            .map(|format| (FetchKind::FileList, format.id()));
        let desired = file_format.or_else(|| {
            available_formats
                .iter()
                .find(|format| format.id() == ClipboardFormatId::CF_UNICODETEXT)
                .map(|format| (FetchKind::Text, format.id()))
        });
        self.bridge.queue_remote_fetch(desired);
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        let response = if request.format == ClipboardFormatId::CF_UNICODETEXT {
            OwnedFormatDataResponse::new_unicode_string(&self.bridge.local_text())
        } else {
            OwnedFormatDataResponse::new_error()
        };
        self.bridge
            .send_clipboard_message(ClipboardMessage::SendFormatData(response));
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        let reply = self
            .bridge
            .finish_remote_fetch(FetchKind::Text, response.is_error());
        if !reply.accepted {
            return;
        }
        if let Ok(text) = response.to_unicode_string() {
            self.bridge.publish_remote_text(text);
        }
    }

    fn on_file_contents_request(&mut self, request: FileContentsRequest) {
        let response = self
            .bridge
            .files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .serve(&request)
            .unwrap_or_else(|_| FileContentsResponse::new_error(request.stream_id));
        self.bridge
            .send_clipboard_message(ClipboardMessage::SendFileContentsResponse(response));
    }

    fn on_file_contents_response(&mut self, response: FileContentsResponse<'_>) {
        let failure_progress = self
            .bridge
            .transfer_progress(RdpClipboardTransferStatus::Failed, None);
        let result = self
            .bridge
            .files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .handle_response(&response);
        match result {
            Ok(update) => {
                self.bridge
                    .publish_transfer(RdpClipboardTransferStatus::Running, None);
                for request in update.requests {
                    self.bridge
                        .send_clipboard_message(ClipboardMessage::SendFileContentsRequest(request));
                }
                if update.completed {
                    let completed = self
                        .bridge
                        .transfer_progress(RdpClipboardTransferStatus::Completed, None);
                    match self
                        .bridge
                        .files
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .complete_remote()
                    {
                        Ok(()) => {
                            if let Some(progress) = completed {
                                self.bridge.send_transfer_progress(progress);
                            }
                        }
                        Err(error) => {
                            if let Some(mut progress) = completed {
                                progress.status = RdpClipboardTransferStatus::Failed;
                                progress.error = Some(error);
                                self.bridge.send_transfer_progress(progress);
                            }
                        }
                    }
                }
            }
            Err(error) => {
                if let Some(mut progress) = failure_progress {
                    progress.error = Some(error);
                    self.bridge.send_transfer_progress(progress);
                }
            }
        }
    }

    fn on_lock(&mut self, data_id: LockDataId) {
        self.bridge
            .files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .lock(data_id);
    }

    fn on_unlock(&mut self, data_id: LockDataId) {
        self.bridge
            .files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .unlock(data_id);
    }

    fn on_remote_file_list(
        &mut self,
        files: &[ironrdp_cliprdr::pdu::FileDescriptor],
        clip_data_id: Option<u32>,
    ) {
        let reply = self.bridge.finish_remote_fetch(FetchKind::FileList, false);
        if !reply.accepted {
            return;
        }
        if self.bridge.shutdown.load(Ordering::SeqCst)
            || !self.bridge.file_available.load(Ordering::SeqCst)
        {
            return;
        }
        let result = self
            .bridge
            .files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .begin_remote(files, clip_data_id);
        match result {
            Ok(update) => {
                self.bridge
                    .publish_transfer(RdpClipboardTransferStatus::Running, None);
                for request in update.requests {
                    self.bridge
                        .send_clipboard_message(ClipboardMessage::SendFileContentsRequest(request));
                }
                if update.completed {
                    let completed = self
                        .bridge
                        .transfer_progress(RdpClipboardTransferStatus::Completed, None);
                    match self
                        .bridge
                        .files
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .complete_remote()
                    {
                        Ok(()) => {
                            if let Some(progress) = completed {
                                self.bridge.send_transfer_progress(progress);
                            }
                        }
                        Err(error) => {
                            if let Some(mut progress) = completed {
                                progress.status = RdpClipboardTransferStatus::Failed;
                                progress.error = Some(error);
                                self.bridge.send_transfer_progress(progress);
                            }
                        }
                    }
                }
            }
            Err(error) => {
                let _ = self
                    .bridge
                    .output_tx
                    .send(Outbound::control(RdpControlMessage::Error {
                        session_id: self.bridge.session_id.clone(),
                        error: nyaterm_remote_desktop::RdpError::new(
                            nyaterm_remote_desktop::RdpErrorKind::Clipboard,
                            error,
                        ),
                        fatal: false,
                    }));
            }
        }
    }

    fn on_outgoing_locks_expired(&mut self, ids: &[LockDataId]) {
        let progress = self.bridge.transfer_progress(
            RdpClipboardTransferStatus::Cancelled,
            Some("RDP clipboard lock expired".to_string()),
        );
        let cancelled = self
            .bridge
            .files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cancel_lock(ids);
        if cancelled.is_some()
            && let Some(progress) = progress
        {
            self.bridge.send_transfer_progress(progress);
        }
    }

    fn on_outgoing_locks_cleared(&mut self, ids: &[LockDataId]) {
        self.on_outgoing_locks_expired(ids);
    }
}

#[cfg(test)]
mod protocol_tests;

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use ironrdp_cliprdr::backend::{ClipboardMessage, CliprdrBackend};
    use ironrdp_cliprdr::pdu::{ClipboardFormatId, ClipboardGeneralCapabilityFlags};

    use super::{ClipboardBridge, TextClipboardBackend};
    use nyaterm_remote_desktop::MAX_CLIPBOARD_TEXT_BYTES;

    #[test]
    fn headless_clipboard_accepts_text_and_rejects_oversize_payloads() {
        let (output_tx, _output_rx) = mpsc::sync_channel(1);
        let bridge = ClipboardBridge::new("session".to_string(), output_tx, false);
        bridge.set_local_text("hello".to_string()).unwrap();
        assert_eq!(bridge.local_text(), "hello");
        assert!(
            bridge
                .set_local_text("x".repeat(MAX_CLIPBOARD_TEXT_BYTES + 1))
                .is_err()
        );
        assert_eq!(bridge.local_text(), "hello");
    }

    #[test]
    fn initial_clipboard_advertisement_includes_an_empty_format_list() {
        let (output_tx, _output_rx) = mpsc::sync_channel(1);
        let bridge = ClipboardBridge::new("session".to_string(), output_tx, false);
        assert!(matches!(
            bridge.current_advertisement(),
            ClipboardMessage::SendInitiateCopy(formats) if formats.is_empty()
        ));

        bridge.set_local_text("hello".to_string()).unwrap();
        assert!(matches!(
            bridge.current_advertisement(),
            ClipboardMessage::SendInitiateCopy(formats)
                if formats.len() == 1 && formats[0].id() == ClipboardFormatId::CF_UNICODETEXT
        ));
    }

    #[test]
    fn initial_advertisement_uses_text_format_even_with_file_capability() {
        let (output_tx, _output_rx) = mpsc::sync_channel(1);
        let bridge = ClipboardBridge::new("session".to_string(), output_tx, true);
        bridge.set_local_text("hello".to_string()).unwrap();
        bridge
            .file_available
            .store(true, std::sync::atomic::Ordering::SeqCst);

        assert!(matches!(
            bridge.initial_advertisement(),
            ClipboardMessage::SendInitiateCopy(formats)
                if formats.len() == 1 && formats[0].id() == ClipboardFormatId::CF_UNICODETEXT
        ));
    }

    #[test]
    fn file_capability_is_opt_in_and_requires_server_negotiation() {
        let (output_tx, _output_rx) = mpsc::sync_channel(1);
        let bridge = ClipboardBridge::new("session".to_string(), output_tx, true);
        let mut backend = TextClipboardBackend {
            bridge: bridge.clone(),
        };
        assert!(
            backend
                .client_capabilities()
                .contains(ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED)
        );
        backend.on_process_negotiated_capabilities(ClipboardGeneralCapabilityFlags::empty());
        assert!(
            !bridge
                .file_available
                .load(std::sync::atomic::Ordering::SeqCst)
        );
        backend.on_process_negotiated_capabilities(
            ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED,
        );
        assert!(
            bridge
                .file_available
                .load(std::sync::atomic::Ordering::SeqCst)
        );

        let (output_tx, _output_rx) = mpsc::sync_channel(1);
        let disabled = ClipboardBridge::new("text-only".to_string(), output_tx, false);
        let backend = TextClipboardBackend { bridge: disabled };
        assert!(
            !backend
                .client_capabilities()
                .contains(ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED)
        );
    }
}
