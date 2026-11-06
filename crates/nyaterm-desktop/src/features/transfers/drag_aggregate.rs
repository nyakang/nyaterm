use crate::models::{TransferJobEvent, TransferJobOutput, TransferJobResult};
use futures::channel::mpsc::UnboundedSender;
use gpui::{NativeFileDragEvent, NativeFileDragObserver, NativeFileDragOutcome};
use nyaterm_files::drag_export::{ExportObserver, ExportReadEvent, RemoteDragFile};
use nyaterm_files::sftp::{SFTP_TRANSFER_CANCELLED, SftpFileEntry, SftpTransferControl};
use nyaterm_files::sftp_transfer_types::{SftpTransferProgress, SftpTransferSummary};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct FileDelivery {
    ranges: Vec<(u64, u64)>,
    total: Option<u64>,
    eof: Option<u64>,
}
impl FileDelivery {
    fn add(&mut self, offset: u64, length: u64, total: Option<u64>) {
        self.total = total;
        if length == 0 {
            self.eof = Some(offset);
            return;
        }
        self.ranges.push((offset, offset.saturating_add(length)));
        self.ranges.sort_unstable();
        let mut merged: Vec<(u64, u64)> = Vec::with_capacity(self.ranges.len());
        for (start, end) in self.ranges.drain(..) {
            if let Some(previous) = merged.last_mut()
                && start <= previous.1
            {
                previous.1 = previous.1.max(end);
            } else {
                merged.push((start, end));
            }
        }
        self.ranges = merged;
    }
    fn bytes(&self) -> u64 {
        self.ranges.iter().map(|(start, end)| end - start).sum()
    }
    fn complete(&self) -> bool {
        self.total.or(self.eof).is_some_and(|total| {
            total == 0 && self.eof.is_some()
                || self
                    .ranges
                    .first()
                    .is_some_and(|&(start, end)| start == 0 && end >= total)
        })
    }
}

struct RootDelivery {
    id: String,
    path: String,
    control: SftpTransferControl,
    files: HashMap<nyaterm_files::sftp::RemoteFilePath, FileDelivery>,
    active: usize,
    bytes: u64,
    completed: u64,
    total: Option<u64>,
    error: Option<String>,
    last_progress: Option<Instant>,
    finished: bool,
}
struct DeliveryState {
    roots: Vec<RootDelivery>,
    started: bool,
    outcome: Option<NativeFileDragOutcome>,
}
pub(super) struct DragAggregate {
    session_id: String,
    sender: UnboundedSender<TransferJobResult>,
    state: Mutex<DeliveryState>,
}
impl DragAggregate {
    pub(super) fn new(
        session_id: String,
        entries: &[SftpFileEntry],
        sender: UnboundedSender<TransferJobResult>,
    ) -> Arc<Self> {
        Arc::new(Self {
            session_id,
            sender,
            state: Mutex::new(DeliveryState {
                roots: entries
                    .iter()
                    .map(|entry| RootDelivery {
                        id: format!("drag-export-{}", nyaterm_core::uuid()),
                        path: entry.path.clone(),
                        control: SftpTransferControl::new(),
                        files: HashMap::new(),
                        active: 0,
                        bytes: 0,
                        completed: 0,
                        total: Some(0),
                        error: None,
                        last_progress: None,
                        finished: false,
                    })
                    .collect(),
                started: false,
                outcome: None,
            }),
        })
    }
    pub(super) fn controls(&self) -> Vec<SftpTransferControl> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .roots
            .iter()
            .map(|root| root.control.clone())
            .collect()
    }
    pub(super) fn register(&self, root_index: usize, file: &RemoteDragFile) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(root) = state.roots.get_mut(root_index)
            && let std::collections::hash_map::Entry::Vacant(entry) =
                root.files.entry(file.remote_path.clone())
        {
            root.total = root
                .total
                .and_then(|sum| file.size.and_then(|size| sum.checked_add(size)));
            entry.insert(FileDelivery {
                total: file.size,
                ..Default::default()
            });
        }
    }
    pub(super) fn observer(
        self: &Arc<Self>,
        root_index: usize,
        file: RemoteDragFile,
    ) -> ExportObserver {
        {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(root) = state.roots.get_mut(root_index) {
                root.active += 1;
            }
        }
        let aggregate = self.clone();
        Arc::new(move |event| {
            let mut state = aggregate
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let outcome = state.outcome;
            let Some(root) = state.roots.get_mut(root_index) else {
                return;
            };
            match event {
                ExportReadEvent::Delivered {
                    offset,
                    length,
                    total,
                } => {
                    let delivery = root.files.entry(file.remote_path.clone()).or_default();
                    let previous_bytes = delivery.bytes();
                    let previous_complete = delivery.complete();
                    delivery.add(offset, length, total);
                    root.bytes += delivery.bytes().saturating_sub(previous_bytes);
                    root.completed = root.completed - u64::from(previous_complete)
                        + u64::from(delivery.complete());
                    aggregate.progress(root, false);
                }
                ExportReadEvent::Finished(result) => {
                    root.active = root.active.saturating_sub(1);
                    if let Err(error) = result
                        && !error.contains(SFTP_TRANSFER_CANCELLED)
                    {
                        root.error.get_or_insert(error);
                    }
                    aggregate.settle(root, outcome);
                }
                ExportReadEvent::Opened(_) | ExportReadEvent::Progress { .. } => {}
            }
        })
    }
    fn send(&self, root: &RootDelivery, event: TransferJobEvent) {
        let _ = self.sender.unbounded_send(TransferJobResult {
            id: root.id.clone(),
            event,
        });
    }
    fn progress(&self, root: &mut RootDelivery, force: bool) {
        if root.finished
            || !force
                && root
                    .last_progress
                    .is_some_and(|last| last.elapsed() < Duration::from_millis(50))
        {
            return;
        }
        root.last_progress = Some(Instant::now());
        self.send(
            root,
            TransferJobEvent::Progress(SftpTransferProgress {
                remote_path: root.path.clone(),
                local_path: PathBuf::new(),
                bytes_transferred: root.bytes,
                total_bytes: root.total,
                item_count_completed: Some(root.completed),
                item_count_total: Some(root.files.len() as u64),
            }),
        );
    }
    fn settle(&self, root: &mut RootDelivery, outcome: Option<NativeFileDragOutcome>) {
        let Some(outcome) = outcome else {
            return;
        };
        if root.finished || root.active > 0 {
            return;
        }
        self.progress(root, true);
        root.finished = true;
        let result = if root.control.is_cancelled() || outcome == NativeFileDragOutcome::Cancelled {
            Err(SFTP_TRANSFER_CANCELLED.to_owned())
        } else if let Some(error) = &root.error {
            Err(error.clone())
        } else if outcome == NativeFileDragOutcome::Failed {
            Err("Native drag consumer failed".to_owned())
        } else {
            Ok(TransferJobOutput::Summary(SftpTransferSummary {
                remote_path: root.path.clone(),
                local_path: PathBuf::new(),
                bytes: root.bytes,
                skipped: false,
            }))
        };
        self.send(root, TransferJobEvent::Finished(result));
    }
}
impl NativeFileDragObserver for DragAggregate {
    fn observe(&self, event: NativeFileDragEvent) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        match event {
            NativeFileDragEvent::Started if !state.started => {
                state.started = true;
                for root in &state.roots {
                    self.send(
                        root,
                        TransferJobEvent::DragExportOpened {
                            session_id: self.session_id.clone(),
                            remote_path: root.path.clone(),
                            control: root.control.clone(),
                            destination: None,
                        },
                    );
                }
            }
            NativeFileDragEvent::Finished(outcome) if state.outcome.is_none() => {
                state.outcome = Some(outcome);
                for root in &mut state.roots {
                    if outcome == NativeFileDragOutcome::Cancelled {
                        root.control.cancel();
                    }
                    self.settle(root, Some(outcome));
                }
            }
            _ => {}
        }
    }
    fn is_paused(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .roots
            .iter()
            .any(|root| root.control.is_paused())
    }
}

#[cfg(test)]
mod tests {
    use super::DragAggregate;
    use crate::models::TransferJobEvent;
    use gpui::{NativeFileDragEvent, NativeFileDragObserver, NativeFileDragOutcome};
    use nyaterm_files::drag_export::{ExportReadEvent, RemoteDragFile};
    use nyaterm_files::sftp::{RemoteFilePath, SftpFileEntry, SftpFileType};

    fn root() -> SftpFileEntry {
        SftpFileEntry {
            name: "folder".into(),
            path: "/folder".into(),
            file_type: SftpFileType::Directory,
            size: None,
            permissions: None,
            owner: String::new(),
            group: String::new(),
            modified_at: None,
            raw_path_token: None,
            symlink_target_is_directory: false,
        }
    }
    fn file(path: &str) -> RemoteDragFile {
        RemoteDragFile {
            display_name: "file".into(),
            remote_path: RemoteFilePath::new(path),
            size: Some(10),
            modified_at: None,
        }
    }
    #[test]
    fn seek_clone_and_reopen_merge_ranges_and_native_finish_waits_for_stream_cleanup() {
        let (sender, mut receiver) = futures::channel::mpsc::unbounded();
        let aggregate = DragAggregate::new("session".into(), &[root()], sender);
        let file = file("/folder/file");
        aggregate.register(0, &file);
        aggregate.observe(NativeFileDragEvent::Started);
        assert!(matches!(
            receiver.try_recv().unwrap().event,
            TransferJobEvent::DragExportOpened { .. }
        ));
        let first = aggregate.observer(0, file.clone());
        let second = aggregate.observer(0, file);
        first(ExportReadEvent::Delivered {
            offset: 0,
            length: 6,
            total: Some(10),
        });
        second(ExportReadEvent::Delivered {
            offset: 4,
            length: 6,
            total: Some(10),
        });
        second(ExportReadEvent::Delivered {
            offset: 0,
            length: 10,
            total: Some(10),
        });
        aggregate.observe(NativeFileDragEvent::Finished(
            NativeFileDragOutcome::Provided,
        ));
        assert!(!aggregate.state.lock().unwrap().roots[0].finished);
        first(ExportReadEvent::Finished(Ok(6)));
        second(ExportReadEvent::Finished(Ok(10)));
        aggregate.observe(NativeFileDragEvent::Finished(
            NativeFileDragOutcome::Cancelled,
        ));
        let state = aggregate.state.lock().unwrap();
        assert_eq!(state.roots[0].bytes, 10);
        assert_eq!(state.roots[0].completed, 1);
        assert!(state.roots[0].finished);
        let mut finishes = 0;
        while let Ok(event) = receiver.try_recv() {
            if let TransferJobEvent::Finished(result) = event.event {
                assert!(result.is_ok());
                finishes += 1;
            }
        }
        assert_eq!(finishes, 1);
    }
    #[test]
    fn empty_roots_and_unrequested_files_finish_without_inventing_read_failures() {
        let (sender, mut receiver) = futures::channel::mpsc::unbounded();
        let aggregate = DragAggregate::new("session".into(), &[root(), root()], sender);
        aggregate.register(1, &file("/folder/skipped"));
        aggregate.observe(NativeFileDragEvent::Started);
        aggregate.observe(NativeFileDragEvent::Finished(
            NativeFileDragOutcome::Provided,
        ));
        let mut opened = 0;
        let mut finished = 0;
        while let Ok(event) = receiver.try_recv() {
            match event.event {
                TransferJobEvent::DragExportOpened { .. } => opened += 1,
                TransferJobEvent::Finished(result) => {
                    assert!(result.is_ok());
                    finished += 1;
                }
                _ => {}
            }
        }
        assert_eq!((opened, finished), (2, 2));
        assert_eq!(aggregate.state.lock().unwrap().roots[1].completed, 0);
    }
    #[test]
    fn thousands_of_children_create_one_row_and_throttle_progress_per_root() {
        let (sender, mut receiver) = futures::channel::mpsc::unbounded();
        let aggregate = DragAggregate::new("session".into(), &[root()], sender);
        for index in 0..10_000 {
            aggregate.register(0, &file(&format!("/folder/{index}")));
        }
        aggregate.observe(NativeFileDragEvent::Started);
        let observe = aggregate.observer(0, file("/folder/0"));
        for _ in 0..1000 {
            observe(ExportReadEvent::Delivered {
                offset: 0,
                length: 10,
                total: Some(10),
            });
        }
        observe(ExportReadEvent::Finished(Ok(10)));
        aggregate.observe(NativeFileDragEvent::Finished(
            NativeFileDragOutcome::Provided,
        ));
        let mut opened = 0;
        let mut progress = 0;
        while let Ok(event) = receiver.try_recv() {
            match event.event {
                TransferJobEvent::DragExportOpened { .. } => opened += 1,
                TransferJobEvent::Progress(_) => progress += 1,
                _ => {}
            }
        }
        assert_eq!(opened, 1);
        assert!(progress <= 3);
    }
    #[test]
    fn released_partial_stream_does_not_cancel_parent_and_real_error_is_retained() {
        let (sender, _) = futures::channel::mpsc::unbounded();
        let aggregate = DragAggregate::new("session".into(), &[root()], sender);
        let observe = aggregate.observer(0, file("/folder/file"));
        observe(ExportReadEvent::Finished(Err(
            nyaterm_files::sftp::SFTP_TRANSFER_CANCELLED.into(),
        )));
        assert!(!aggregate.controls()[0].is_cancelled());
        let observe = aggregate.observer(0, file("/folder/file"));
        observe(ExportReadEvent::Finished(Err("read failed".into())));
        assert_eq!(
            aggregate.state.lock().unwrap().roots[0].error.as_deref(),
            Some("read failed")
        );
    }
}
