//! ADB Sync is sequential. Native random reads share a private disk snapshot.
use crate::adb::AdbFileService;
use crate::drag_export::{EXPORT_READ_CHUNK, ExportObserver, ExportReadEvent, RemoteDragFile};
use crate::file_browser::{FileBrowserBackendKind, FileTransferEntryError};
use crate::sftp::{SFTP_TRANSFER_CANCELLED, SftpTransferControl};
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

// TempDir removal may run when a native descriptor is released on the UI thread.
// Initialize the janitor on the first worker download and only enqueue on release.
static CLEANUP: std::sync::OnceLock<std::sync::mpsc::Sender<tempfile::TempDir>> =
    std::sync::OnceLock::new();

fn cleanup_sender() -> io::Result<&'static std::sync::mpsc::Sender<tempfile::TempDir>> {
    if let Some(sender) = CLEANUP.get() {
        return Ok(sender);
    }
    let (sender, receiver) = std::sync::mpsc::channel::<tempfile::TempDir>();
    std::thread::Builder::new()
        .name("adb-export-cleanup".into())
        .spawn(move || {
            for directory in receiver {
                drop(directory);
            }
        })?;
    let _ = CLEANUP.set(sender);
    Ok(CLEANUP.get().expect("cleanup initialized"))
}

fn remove_snapshot(snapshot: &Mutex<Snapshot>) {
    if let Ok(mut snapshot) = snapshot.try_lock()
        && let Some(directory) = snapshot.directory.take()
    {
        // A successful snapshot always initialized the worker janitor.
        let _ = CLEANUP
            .get()
            .expect("snapshot cleanup initialized")
            .send(directory);
    }
}

#[derive(Default)]
struct Snapshot {
    directory: Option<tempfile::TempDir>,
    failed: bool,
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        if let Some(directory) = self.directory.take() {
            let _ = CLEANUP
                .get()
                .expect("snapshot cleanup initialized")
                .send(directory);
        }
    }
}

pub struct AdbReadSource {
    service: Weak<AdbFileService>,
    file: RemoteDragFile,
    observers: Arc<dyn Fn() -> ExportObserver + Send + Sync>,
    control: SftpTransferControl,
    snapshot: Arc<Mutex<Snapshot>>,
    streams: Arc<AtomicUsize>,
}

impl AdbReadSource {
    pub fn new(
        service: Weak<AdbFileService>,
        file: RemoteDragFile,
        observers: Arc<dyn Fn() -> ExportObserver + Send + Sync>,
        control: SftpTransferControl,
    ) -> Self {
        Self {
            service,
            file,
            observers,
            control: control.child(),
            snapshot: Arc::default(),
            streams: Arc::default(),
        }
    }

    /// Only native workers call open/read_at; no content is fetched during render.
    pub fn open(&self) -> io::Result<AdbReadStream> {
        let observer = (self.observers)();
        let result = self.control.check_cancelled().and_then(|()| {
            self.streams
                .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                    (count < 16).then_some(count + 1)
                })
                .map(|_| ())
                .map_err(|_| FileTransferEntryError::LimitExceeded.into())
        });
        if let Err(error) = result {
            observer(ExportReadEvent::Failed {
                backend: FileBrowserBackendKind::Adb,
                error,
            });
            return Err(io::Error::other("Android file export unavailable"));
        }
        let control = self.control.child();
        observer(ExportReadEvent::Opened(control.clone()));
        Ok(AdbReadStream {
            service: self.service.clone(),
            file: self.file.clone(),
            observer,
            control,
            snapshot: self.snapshot.clone(),
            finished: false,
            streams: self.streams.clone(),
        })
    }

    pub fn cancel(&self) {
        self.control.cancel();
        remove_snapshot(&self.snapshot);
    }

    pub fn cancellation_cleanup(&self) -> (SftpTransferControl, Arc<dyn Fn() + Send + Sync>) {
        let snapshot = Arc::downgrade(&self.snapshot);
        (
            self.control.clone(),
            Arc::new(move || {
                if let Some(snapshot) = snapshot.upgrade() {
                    remove_snapshot(&snapshot);
                }
            }),
        )
    }
}

impl Drop for AdbReadSource {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub struct AdbReadStream {
    service: Weak<AdbFileService>,
    file: RemoteDragFile,
    observer: ExportObserver,
    control: SftpTransferControl,
    snapshot: Arc<Mutex<Snapshot>>,
    finished: bool,
    streams: Arc<AtomicUsize>,
}

impl AdbReadStream {
    pub fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.len() > EXPORT_READ_CHUNK {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        if buffer.is_empty() {
            return Ok(0);
        }
        let result = self.read_snapshot(offset, buffer);
        match result {
            Ok((length, total)) => {
                (self.observer)(ExportReadEvent::Delivered {
                    offset,
                    length: length as u64,
                    total: Some(total),
                });
                Ok(length)
            }
            Err(error) => {
                remove_snapshot(&self.snapshot);
                if !self.finished {
                    self.finished = true;
                    (self.observer)(ExportReadEvent::Failed {
                        backend: FileBrowserBackendKind::Adb,
                        error,
                    });
                }
                Err(io::Error::other("Android file export failed"))
            }
        }
    }

    fn read_snapshot(&self, offset: u64, buffer: &mut [u8]) -> anyhow::Result<(usize, u64)> {
        let service = self
            .service
            .upgrade()
            .ok_or_else(|| anyhow::Error::new(nyaterm_android::error::AdbError::Offline))?;
        service.wait_for_transfer(&self.control)?;
        let mut snapshot = loop {
            service.wait_for_transfer(&self.control)?;
            match self.snapshot.try_lock() {
                Ok(snapshot) => break snapshot,
                Err(std::sync::TryLockError::WouldBlock) => {
                    std::thread::sleep(std::time::Duration::from_millis(25))
                }
                Err(_) => anyhow::bail!("Android export snapshot unavailable"),
            }
        };
        anyhow::ensure!(!snapshot.failed, "Android export snapshot failed");
        if snapshot.directory.is_none() {
            let result = (|| {
                let before = service.stat_with_control(&self.file.remote_path, &self.control)?;
                anyhow::ensure!(
                    self.file.size.is_none_or(|size| size == before.size)
                        && self.file.modified_at.is_none_or(|mtime| mtime
                            == std::time::UNIX_EPOCH
                                + std::time::Duration::from_secs(before.modified)),
                    FileTransferEntryError::SourceChanged
                );
                cleanup_sender()?;
                let directory = tempfile::tempdir()?;
                let target = directory.path().join("content");
                let mut transferred = 0u64;
                let bytes = service.download(
                    &self.file.remote_path,
                    &target,
                    false,
                    &self.control,
                    |bytes| {
                        transferred = transferred.saturating_add(bytes);
                        (self.observer)(ExportReadEvent::Progress {
                            bytes: transferred,
                            total: self.file.size,
                        });
                    },
                )?;
                anyhow::ensure!(
                    self.file.size.is_none_or(|size| bytes == size),
                    FileTransferEntryError::SourceChanged
                );
                Ok::<_, anyhow::Error>(directory)
            })();
            match result {
                Ok(directory) => snapshot.directory = Some(directory),
                Err(error) => {
                    snapshot.failed = true;
                    return Err(error);
                }
            }
        }
        let mut reader = std::fs::File::open(
            snapshot
                .directory
                .as_ref()
                .expect("snapshot initialized")
                .path()
                .join("content"),
        )?;
        let total = reader.metadata()?.len();
        reader.seek(SeekFrom::Start(offset))?;
        let length = reader.read(buffer)?;
        self.control.check_cancelled()?;
        service.control.check_cancelled()?;
        service.runtime.check()?;
        Ok((length, total))
    }

    pub fn cancel(&self) {
        self.control.cancel();
    }
}

impl Drop for AdbReadStream {
    fn drop(&mut self) {
        self.streams.fetch_sub(1, Ordering::AcqRel);
        if !self.finished {
            (self.observer)(ExportReadEvent::Finished(if self.control.is_cancelled() {
                Err(SFTP_TRANSFER_CANCELLED.into())
            } else {
                Ok(0)
            }));
        }
        self.control.cancel();
    }
}

#[cfg(test)]
mod tests {
    use crate::adb::test_support::AdbFixture;
    use crate::drag_export::{AdbReadSource, RemoteDragFile};
    use crate::sftp::{RemoteFilePath, SftpTransferControl};
    use std::sync::Arc;

    fn wait_for_cleanup(path: &std::path::Path) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while path.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "snapshot was not cleaned up"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn native_adb_paused_read_exits_when_the_binding_is_invalidated() {
        let fixture = AdbFixture::new();
        fixture.file(b"/device/content", b"data");
        let control = SftpTransferControl::new();
        control.pause();
        let source = AdbReadSource::new(
            Arc::downgrade(&fixture.service),
            RemoteDragFile {
                display_name: "content".into(),
                size: Some(4),
                remote_path: RemoteFilePath::new("/device/content"),
                modified_at: None,
            },
            Arc::new(|| Arc::new(|_| {})),
            control.clone(),
        );
        let mut stream = source.open().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            sender
                .send(stream.read_at(0, &mut [0; 4]).is_err())
                .unwrap();
        });
        fixture.service.runtime.invalidate();
        let result = receiver.recv_timeout(std::time::Duration::from_secs(2));
        control.cancel();
        worker.join().unwrap();
        assert!(result.unwrap());
        assert_eq!(fixture.device.lock().unwrap().receives, 0);
    }

    #[test]
    fn native_adb_reads_share_one_disk_snapshot_and_support_arbitrary_offsets() {
        let fixture = AdbFixture::new();
        let data: Vec<u8> = (0..150_000).map(|n| (n * 31) as u8).collect();
        fixture.file(b"/device/content", &data);
        let source = AdbReadSource::new(
            Arc::downgrade(&fixture.service),
            RemoteDragFile {
                display_name: "content".into(),
                size: Some(data.len() as u64),
                remote_path: RemoteFilePath::new("/device/content"),
                modified_at: None,
            },
            Arc::new(|| Arc::new(|_| {})),
            SftpTransferControl::new(),
        );
        let mut first = source.open().unwrap();
        let mut second = source.open().unwrap();
        assert_eq!(fixture.device.lock().unwrap().receives, 0);
        for (use_first, offset) in [(true, 100_000), (false, 3), (true, 100_000)] {
            let stream = if use_first { &mut first } else { &mut second };
            let mut buffer = [0; 1024];
            assert_eq!(stream.read_at(offset, &mut buffer).unwrap(), buffer.len());
            assert_eq!(
                buffer.as_slice(),
                &data[offset as usize..offset as usize + buffer.len()]
            );
        }
        assert_eq!(fixture.device.lock().unwrap().receives, 1);
        let path = source
            .snapshot
            .lock()
            .unwrap()
            .directory
            .as_ref()
            .unwrap()
            .path()
            .to_path_buf();
        assert!(path.exists());
        source.cancel();
        assert!(first.read_at(0, &mut [0; 1]).is_err());
        wait_for_cleanup(&path);
    }

    #[test]
    fn native_adb_snapshot_is_removed_after_its_last_consumer_and_device_disconnect_stops_reads() {
        let fixture = AdbFixture::new();
        fixture.file(b"/device/content", b"content");
        let source = AdbReadSource::new(
            Arc::downgrade(&fixture.service),
            RemoteDragFile {
                display_name: "content".into(),
                size: Some(7),
                remote_path: RemoteFilePath::new("/device/content"),
                modified_at: None,
            },
            Arc::new(|| Arc::new(|_| {})),
            SftpTransferControl::new(),
        );
        let mut stream = source.open().unwrap();
        stream.read_at(0, &mut [0; 7]).unwrap();
        let path = source
            .snapshot
            .lock()
            .unwrap()
            .directory
            .as_ref()
            .unwrap()
            .path()
            .to_path_buf();
        fixture.service.cancel();
        assert!(stream.read_at(0, &mut [0; 1]).is_err());
        drop(stream);
        drop(source);
        wait_for_cleanup(&path);
    }

    #[test]
    fn native_adb_stream_limit_and_release_keep_sibling_files_running() {
        let fixture = AdbFixture::new();
        let parent = SftpTransferControl::new();
        let source = AdbReadSource::new(
            Arc::downgrade(&fixture.service),
            RemoteDragFile {
                display_name: "file".into(),
                remote_path: RemoteFilePath::new("/device/file"),
                size: None,
                modified_at: None,
            },
            Arc::new(|| Arc::new(|_| {})),
            parent.clone(),
        );
        let mut streams = (0..16).map(|_| source.open().unwrap()).collect::<Vec<_>>();
        assert!(source.open().is_err());
        streams.pop();
        assert!(source.open().is_ok());
        source.cancel();
        assert!(!parent.is_cancelled());
    }

    #[test]
    fn native_adb_source_change_fails_once_without_retaining_a_snapshot() {
        use crate::drag_export::ExportReadEvent;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let fixture = AdbFixture::new();
        fixture.file(b"/device/file", b"changed");
        let errors = Arc::new(AtomicUsize::new(0));
        let observed = errors.clone();
        let source = AdbReadSource::new(
            Arc::downgrade(&fixture.service),
            RemoteDragFile {
                display_name: "file".into(),
                remote_path: RemoteFilePath::new("/device/file"),
                size: Some(3),
                modified_at: None,
            },
            Arc::new(move || {
                let observed = observed.clone();
                Arc::new(move |event| {
                    if matches!(event, ExportReadEvent::Failed { .. }) {
                        observed.fetch_add(1, Ordering::Relaxed);
                    }
                })
            }),
            SftpTransferControl::new(),
        );
        let mut stream = source.open().unwrap();
        for _ in 0..2 {
            assert!(stream.read_at(0, &mut [0; 1]).is_err());
        }
        assert_eq!(errors.load(Ordering::Relaxed), 1);
        assert!(source.snapshot.lock().unwrap().directory.is_none());
        assert_eq!(fixture.device.lock().unwrap().receives, 0);
    }
}
