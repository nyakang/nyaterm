use super::FileTransferEndpoint;
use crate::sftp::{SftpFileType, SftpTransferControl};
use crate::sftp_transfer_types::SftpTransferProgress;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// One task owns this callback, including both legs of a staged copy. All values
/// sent to the desktop are cumulative; backend counters may restart between legs.
#[derive(Clone)]
pub(crate) struct CopyProgress(Arc<Mutex<CopyProgressState>>);

struct CopyProgressState {
    value: SftpTransferProgress,
    callback: Box<dyn FnMut(SftpTransferProgress) + Send>,
    stages: u64,
    stage_items: u64,
    byte_offset: u64,
    item_offset: u64,
}

impl CopyProgressState {
    fn emit(&mut self, finished: bool) {
        let mut value = self.value.clone();
        // A last write is not a committed copy. Keep the completion event until
        // flush, verification and rename have succeeded, including zero-byte tasks.
        if !finished && let Some(total) = value.item_count_total {
            value.item_count_completed = value.item_count_completed.map(|n| n.min(total - 1));
        }
        if value
            .total_bytes
            .is_some_and(|n| value.bytes_transferred > n)
        {
            value.total_bytes = None;
        }
        (self.callback)(value);
    }
}

impl CopyProgress {
    pub(crate) fn new(
        source: &FileTransferEndpoint,
        destination: &FileTransferEndpoint,
        stages: u64,
        callback: impl FnMut(SftpTransferProgress) + Send + 'static,
    ) -> Self {
        let remote_path = match destination {
            FileTransferEndpoint::Remote { path, .. } => path.display_path.clone(),
            FileTransferEndpoint::Local(_) => match source {
                FileTransferEndpoint::Remote { path, .. } => path.display_path.clone(),
                FileTransferEndpoint::Local(path) => path.to_string_lossy().into_owned(),
            },
        };
        let local_path = match (source, destination) {
            (_, FileTransferEndpoint::Local(path)) | (FileTransferEndpoint::Local(path), _) => {
                path.clone()
            }
            _ => PathBuf::new(),
        };
        let reporter = Self(Arc::new(Mutex::new(CopyProgressState {
            value: SftpTransferProgress {
                remote_path,
                local_path,
                bytes_transferred: 0,
                total_bytes: None,
                item_count_completed: None,
                item_count_total: None,
            },
            callback: Box::new(callback),
            stages,
            stage_items: 1,
            byte_offset: 0,
            item_offset: 0,
        })));
        reporter.0.lock().expect("copy progress lock").emit(false);
        reporter
    }

    pub(crate) fn scan(
        &self,
        source: &FileTransferEndpoint,
        control: &SftpTransferControl,
    ) -> anyhow::Result<()> {
        let mut pending = vec![source.clone()];
        let mut bytes = Some(0_u64);
        let mut items = 0_u64;
        while let Some(entry) = pending.pop() {
            control.wait_if_paused_blocking()?;
            items = items.saturating_add(1);
            match entry {
                FileTransferEndpoint::Local(path) => {
                    let metadata = fs::symlink_metadata(&path)?;
                    anyhow::ensure!(
                        !metadata.file_type().is_symlink(),
                        "copying local symbolic links is not supported"
                    );
                    if metadata.is_dir() {
                        for entry in fs::read_dir(path)? {
                            control.wait_if_paused_blocking()?;
                            pending.push(FileTransferEndpoint::Local(entry?.path()));
                        }
                    } else {
                        anyhow::ensure!(metadata.is_file(), "copy source is not a regular file");
                        bytes = bytes.and_then(|n| n.checked_add(metadata.len()));
                    }
                }
                FileTransferEndpoint::Remote { service, path } => {
                    let properties = service.remote_file_properties(&path)?;
                    if properties.file_type == SftpFileType::Directory {
                        for entry in service.list_dir_path(&path)? {
                            control.wait_if_paused_blocking()?;
                            if !matches!(entry.name.as_str(), "." | "..") {
                                pending.push(FileTransferEndpoint::Remote {
                                    service: service.clone(),
                                    path: entry.remote_path(),
                                });
                            }
                        }
                    } else {
                        bytes = bytes
                            .zip(properties.size)
                            .and_then(|(a, b)| a.checked_add(b));
                    }
                }
            }
        }
        let mut state = self.0.lock().expect("copy progress lock");
        state.stage_items = items.max(1);
        state.value.total_bytes = bytes.and_then(|n| n.checked_mul(state.stages));
        state.value.item_count_total = Some(state.stage_items.saturating_mul(state.stages));
        state.value.item_count_completed = Some(0);
        state.emit(false);
        Ok(())
    }

    pub(crate) fn add_bytes(&self, bytes: u64) {
        let mut state = self.0.lock().expect("copy progress lock");
        state.value.bytes_transferred = state.value.bytes_transferred.saturating_add(bytes);
        state.emit(false);
    }

    pub(crate) fn finish_item(&self) {
        let mut state = self.0.lock().expect("copy progress lock");
        state.value.item_count_completed = Some(
            state
                .value
                .item_count_completed
                .unwrap_or(0)
                .saturating_add(1),
        );
        state.emit(false);
    }

    pub(super) fn update(&self, value: SftpTransferProgress) {
        let mut state = self.0.lock().expect("copy progress lock");
        state.value.bytes_transferred = state
            .value
            .bytes_transferred
            .max(state.byte_offset.saturating_add(value.bytes_transferred));
        if let Some(items) = value.item_count_completed {
            state.value.item_count_completed = Some(
                state.value.item_count_completed.unwrap_or(0).max(
                    state
                        .item_offset
                        .saturating_add(items.min(state.stage_items)),
                ),
            );
        }
        state.emit(false);
    }

    pub(super) fn next_stage(&self, bytes: u64) {
        let mut state = self.0.lock().expect("copy progress lock");
        state.byte_offset = bytes;
        state.item_offset = state.stage_items;
        state.value.bytes_transferred = bytes;
        state.value.total_bytes = bytes.checked_mul(2);
        state.value.item_count_completed = Some(state.item_offset);
        state.emit(false);
    }

    pub(crate) fn finish(&self, bytes: u64) {
        let mut state = self.0.lock().expect("copy progress lock");
        let bytes = state
            .byte_offset
            .saturating_add(bytes)
            .max(state.value.bytes_transferred);
        state.value.bytes_transferred = bytes;
        state.value.total_bytes = Some(bytes);
        state.value.item_count_completed = state.value.item_count_total;
        state.emit(true);
    }
}
