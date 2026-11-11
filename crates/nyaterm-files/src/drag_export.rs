//! Deferred drag sources and the bounded synchronous-consumer/SFTP bridge.
//! Source metadata never contains a local destination or saved credentials.

mod adb;
mod lifetime;
pub use adb::{AdbReadSource, AdbReadStream};
pub mod tree;
pub use lifetime::DragSourceLifetime;

use crate::remote_file::RemoteFileService;
use crate::sftp::{RemoteFilePath, SftpTransferControl};
use crate::sftp_transfer_types::SftpTransferOptions;
use std::{
    ffi::OsString,
    io,
    path::PathBuf,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, SystemTime},
};
use tokio::{sync::mpsc as async_mpsc, task::JoinHandle};

pub const EXPORT_READ_CHUNK: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DragExportSource {
    Local { path: PathBuf, is_directory: bool },
    RemoteFile(RemoteDragFile),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteDragFile {
    pub display_name: OsString,
    pub size: Option<u64>,
    pub remote_path: RemoteFilePath,
    pub modified_at: Option<SystemTime>,
}

pub enum ExportReadEvent {
    Opened(SftpTransferControl),
    /// Delivered ranges allow a gesture to deduplicate independent native streams.
    Delivered {
        offset: u64,
        length: u64,
        total: Option<u64>,
    },
    Progress {
        bytes: u64,
        total: Option<u64>,
    },
    /// Content delivery finished; this does not assert that the OS committed a destination.
    Finished(Result<u64, String>),
    Failed {
        backend: crate::file_browser::FileBrowserBackendKind,
        error: anyhow::Error,
    },
}

pub type ExportObserver = Arc<dyn Fn(ExportReadEvent) + Send + Sync>;

pub(crate) struct ReadRequest {
    pub offset: u64,
    pub length: usize,
    pub reply: mpsc::SyncSender<io::Result<Vec<u8>>>,
}

struct ExportControl {
    cancelled: AtomicBool,
    streams: Mutex<Vec<Weak<SftpTransferControl>>>,
}

/// A weak reference to the authoritative connection's service. Unused drag
/// descriptors cannot extend the lifetime of credentials or a session.
pub struct SftpReadSource {
    service: Weak<RemoteFileService>,
    file: RemoteDragFile,
    observer: Arc<dyn Fn() -> ExportObserver + Send + Sync>,
    control: Arc<ExportControl>,
    options: SftpTransferOptions,
    parent: Option<SftpTransferControl>,
}

impl SftpReadSource {
    pub fn new(
        service: Weak<RemoteFileService>,
        file: RemoteDragFile,
        observer: Arc<dyn Fn() -> ExportObserver + Send + Sync>,
    ) -> Self {
        Self {
            service,
            file,
            observer,
            options: SftpTransferOptions::default(),
            parent: None,
            control: Arc::new(ExportControl {
                cancelled: AtomicBool::new(false),
                streams: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Only the download concurrency and buffer size affect drag export.
    pub fn with_transfer_options(mut self, options: SftpTransferOptions) -> Self {
        self.options = options;
        self
    }

    pub fn with_parent_control(mut self, parent: SftpTransferControl) -> Self {
        self.parent = Some(parent);
        self
    }

    /// Only allocates a bounded queue and schedules a task. SSH and file opens
    /// happen on the transport runtime after the first content request.
    pub fn open(&self) -> io::Result<SftpReadStream> {
        let control = Arc::new(
            self.parent
                .as_ref()
                .map_or_else(SftpTransferControl::new, SftpTransferControl::child),
        );
        let mut streams = self
            .control
            .streams
            .lock()
            .map_err(|_| io::Error::other("export control unavailable"))?;
        streams.retain(|stream| stream.strong_count() > 0);
        if self.control.cancelled.load(Ordering::Acquire) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        if streams.len() >= 16 {
            return Err(io::Error::other("too many active export streams"));
        }
        streams.push(Arc::downgrade(&control));
        let (sender, receiver) = async_mpsc::channel(1);
        let observer = (self.observer)();
        let task = crate::sftp::spawn_export_reader(
            self.service.clone(),
            self.file.clone(),
            self.options.clone(),
            control.clone(),
            receiver,
            observer.clone(),
        )
        .inspect_err(|error| observer(ExportReadEvent::Finished(Err(error.to_string()))))?;
        Ok(SftpReadStream {
            sender,
            control,
            _task: task,
        })
    }

    pub fn cancel(&self) {
        self.control.cancelled.store(true, Ordering::Release);
        if let Ok(streams) = self.control.streams.lock() {
            for stream in streams.iter().filter_map(Weak::upgrade) {
                stream.cancel();
            }
        }
    }
}

impl Drop for SftpReadSource {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Owns its queue, task handle and cancellation. No complete-file cache exists.
pub struct SftpReadStream {
    sender: async_mpsc::Sender<ReadRequest>,
    control: Arc<SftpTransferControl>,
    _task: JoinHandle<()>,
}

impl SftpReadStream {
    /// Call on a native worker only. The network task owns all SFTP IO.
    pub fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.len() > EXPORT_READ_CHUNK {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        if self.control.is_cancelled() {
            return Err(io::ErrorKind::Interrupted.into());
        }
        if buffer.is_empty() {
            return Ok(0);
        }
        let (reply, result) = mpsc::sync_channel(1);
        self.sender
            .try_send(ReadRequest {
                offset,
                length: buffer.len(),
                reply,
            })
            .map_err(|_| io::Error::other("remote export stream is closed"))?;
        // Cancellation wakes waiting consumers independently of network timeouts.
        loop {
            if self.control.is_cancelled() {
                return Err(io::ErrorKind::Interrupted.into());
            }
            match result.recv_timeout(Duration::from_millis(25)) {
                Ok(result) => {
                    let bytes = result?;
                    if bytes.len() > buffer.len() {
                        return Err(io::Error::other("invalid remote read length"));
                    }
                    buffer[..bytes.len()].copy_from_slice(&bytes);
                    return Ok(bytes.len());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::other("remote export task ended"));
                }
            }
        }
    }

    pub fn cancel(&self) {
        self.control.cancel();
    }
}

impl Drop for SftpReadStream {
    fn drop(&mut self) {
        // The runtime task notices cancellation within 25ms, closes its SFTP
        // handle with a bounded timeout, and releases the session. Never join UI.
        self.control.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::{EXPORT_READ_CHUNK, ReadRequest, SftpReadStream};
    use crate::sftp::SftpTransferControl;
    use std::sync::Arc;

    #[tokio::test(flavor = "multi_thread")]
    async fn bounded_bridge_maps_offsets_and_closes_on_drop() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<ReadRequest>(1);
        let control = Arc::new(SftpTransferControl::new());
        let worker_control = control.clone();
        let task = tokio::spawn(async move {
            let request = receiver.recv().await.unwrap();
            assert_eq!(request.offset, 3);
            request.reply.send(Ok(b"data".to_vec())).unwrap();
            assert!(matches!(
                worker_control.until_cancelled(receiver.recv()).await,
                Err(_) | Ok(None)
            ));
        });
        let abort = task.abort_handle();
        let mut stream = SftpReadStream {
            sender,
            control: control.clone(),
            _task: task,
        };
        let mut buffer = [0; 4];
        // This mimics the synchronous platform worker, not a runtime/UI callback.
        stream = tokio::task::spawn_blocking(move || {
            assert_eq!(stream.read_at(3, &mut buffer).unwrap(), 4);
            assert_eq!(&buffer, b"data");
            assert!(
                stream
                    .read_at(0, &mut vec![0; EXPORT_READ_CHUNK + 1])
                    .is_err()
            );
            stream
        })
        .await
        .unwrap();
        drop(stream);
        assert!(control.is_cancelled());
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !abort.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
