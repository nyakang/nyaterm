//! File changes for external editing; callers own the thread and upload policy.
use crate::local_revision::{LocalFileVersion, read_local_file_version};
use std::{
    fs, io,
    path::Path,
    sync::mpsc,
    time::{Duration, SystemTime},
};

const WATCH_INTERVAL: Duration = Duration::from_millis(1000);
const UPLOAD_SETTLE: Duration = Duration::from_millis(450);

#[derive(Debug)]
pub enum ExternalEditEvent {
    Modified,
    Failed(io::Error),
}

/// Runs until stopped, the event receiver closes, or a permanent file error occurs.
/// Dropping the stop sender also stops the watcher. Never call on a UI thread.
pub fn watch_external_edit(
    local_path: &Path,
    baseline: LocalFileVersion,
    stop_rx: mpsc::Receiver<()>,
    emit: impl FnMut(ExternalEditEvent) -> bool,
) {
    watch_with_timings(local_path, stop_rx, WatchTimings::default(), baseline, emit);
}

#[derive(Clone, Copy)]
struct WatchTimings {
    interval: Duration,
    settle: Duration,
}

impl Default for WatchTimings {
    fn default() -> Self {
        Self {
            interval: WATCH_INTERVAL,
            settle: UPLOAD_SETTLE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalFileFingerprint {
    len: u64,
    modified: Option<SystemTime>,
}

impl LocalFileFingerprint {
    fn from_path(path: &Path) -> std::io::Result<Self> {
        let metadata = fs::metadata(path)?;
        Ok(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

fn watch_with_timings(
    local_path: &Path,
    stop_rx: mpsc::Receiver<()>,
    timings: WatchTimings,
    mut baseline: LocalFileVersion,
    mut emit: impl FnMut(ExternalEditEvent) -> bool,
) {
    let mut first_scan = true;
    loop {
        if wait_for_stop(
            &stop_rx,
            if first_scan {
                Duration::ZERO
            } else {
                timings.interval
            },
        ) {
            break;
        }
        let current = match LocalFileFingerprint::from_path(local_path) {
            Ok(fingerprint) => fingerprint,
            // Atomic editor saves may temporarily remove the path.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if wait_for_stop(&stop_rx, timings.interval) {
                    break;
                }
                continue;
            }
            Err(error) => {
                emit(ExternalEditEvent::Failed(error));
                break;
            }
        };
        if !first_scan && current.len == baseline.len && current.modified == baseline.modified {
            continue;
        }
        first_scan = false;
        if wait_for_stop(&stop_rx, timings.settle) {
            break;
        }
        let version =
            match read_local_file_version(local_path, || wait_for_stop(&stop_rx, Duration::ZERO)) {
                Ok(version) => version,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => break,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::NotFound
                    ) =>
                {
                    // Force another content check even if metadata returns to baseline.
                    first_scan = true;
                    if wait_for_stop(&stop_rx, timings.interval) {
                        break;
                    }
                    continue;
                }
                Err(error) => {
                    emit(ExternalEditEvent::Failed(error));
                    break;
                }
            };
        if version.len != current.len || version.modified != current.modified {
            // A save during the settle window needs a full quiet interval.
            first_scan = true;
            continue;
        }
        if version.digest != baseline.digest && !emit(ExternalEditEvent::Modified) {
            break;
        }
        // Advance only to the bytes actually verified above. An event is not an
        // upload acknowledgement, and a newer edit may already be on disk.
        baseline = version;
    }
}

fn wait_for_stop(stop_rx: &mpsc::Receiver<()>, timeout: Duration) -> bool {
    match stop_rx.recv_timeout(timeout) {
        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => true,
        Err(mpsc::RecvTimeoutError::Timeout) => false,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use crate::local_revision::read_local_file_version;
    use futures::channel::mpsc::unbounded;

    use super::{ExternalEditEvent, WatchTimings, watch_with_timings};

    fn next_modification(rx: &mut futures::channel::mpsc::UnboundedReceiver<ExternalEditEvent>) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(event) = rx.try_recv() {
                assert!(matches!(event, ExternalEditEvent::Modified));
                return;
            }
            assert!(Instant::now() < deadline, "missing edit notification");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn edits_before_watcher_start_and_immediate_same_length_saves_are_reported() {
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-external-watch");
        fs::create_dir_all(root.path()).unwrap();
        let path = root.path().join("edit.txt");
        fs::write(&path, "value=1").unwrap();
        let baseline = read_local_file_version(&path, || false).unwrap();
        // The editor can save before its completion event reaches the UI.
        fs::write(&path, "value=2").unwrap();
        let (tx, mut rx) = unbounded();
        let (stop_tx, stop_rx) = mpsc::channel();
        let worker_path = path.clone();
        let worker =
            nyaterm_core::runtime::worker::spawn("external-edit-test".to_string(), move || {
                watch_with_timings(
                    &worker_path,
                    stop_rx,
                    WatchTimings {
                        interval: Duration::from_millis(5),
                        settle: Duration::from_millis(5),
                    },
                    baseline,
                    move |event| tx.unbounded_send(event).is_ok(),
                )
            })
            .expect("spawn watcher");
        next_modification(&mut rx);
        for value in 3..=5 {
            fs::write(&path, format!("value={value}")).unwrap();
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(value))
                .unwrap();
            next_modification(&mut rx);
        }
        drop(stop_tx);
        worker.join().unwrap();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn timestamp_only_changes_do_not_emit_upload_notifications() {
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-external-watch");
        fs::create_dir_all(root.path()).unwrap();
        let path = root.path().join("edit.txt");
        fs::write(&path, "unchanged").unwrap();
        let baseline = read_local_file_version(&path, || false).unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(42))
            .unwrap();
        let current = read_local_file_version(&path, || false).unwrap();
        assert_ne!(baseline.modified, current.modified);
        assert_eq!(baseline.digest, current.digest);
        let (tx, mut rx) = unbounded();
        let (stop_tx, stop_rx) = mpsc::channel();
        let worker = nyaterm_core::runtime::worker::spawn(
            "external-timestamp-test".to_string(),
            move || {
                watch_with_timings(
                    &path,
                    stop_rx,
                    WatchTimings {
                        interval: Duration::from_millis(5),
                        settle: Duration::ZERO,
                    },
                    baseline,
                    move |event| tx.unbounded_send(event).is_ok(),
                )
            },
        )
        .expect("spawn watcher");
        std::thread::sleep(Duration::from_millis(50));
        drop(stop_tx);
        worker.join().unwrap();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn watcher_stop_wakes_interval_wait_without_polling_delay() {
        let path = std::env::temp_dir().join(format!(
            "nyaterm-external-watch-{}-{}",
            std::process::id(),
            nyaterm_core::uuid()
        ));
        fs::write(&path, b"initial").expect("write fixture");
        let baseline = read_local_file_version(&path, || false).unwrap();
        let (transfer_tx, _transfer_rx) = unbounded();
        let (stop_tx, stop_rx) = mpsc::channel();
        let worker_path = path.clone();
        let worker = nyaterm_core::runtime::worker::spawn(
            "nyaterm-external-watch-test".to_string(),
            move || {
                watch_with_timings(
                    &worker_path,
                    stop_rx,
                    WatchTimings {
                        interval: Duration::from_secs(30),
                        settle: Duration::ZERO,
                    },
                    baseline,
                    move |event| transfer_tx.unbounded_send(event).is_ok(),
                );
            },
        )
        .expect("spawn watcher");

        let started = Instant::now();
        drop(stop_tx);
        worker.join().expect("join watcher");
        assert!(started.elapsed() < Duration::from_secs(1));
        let _ = fs::remove_file(path);
    }
}
