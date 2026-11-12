//! Owned runtime for watching files opened in an external editor.

use crate::thread_owner::{BackgroundCleanup, spawn_joinable};
use nyaterm_files::local_revision::{LocalFileVersion, read_local_file_version};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use futures::channel::mpsc::UnboundedSender;

use crate::models::{TransferJobEvent, TransferJobResult};

const WATCH_INTERVAL: Duration = Duration::from_millis(1000);
const UPLOAD_SETTLE: Duration = Duration::from_millis(450);

pub(super) struct ExternalEditorWatcher {
    stop_tx: Option<mpsc::Sender<()>>,
    worker: Option<JoinHandle<()>>,
    cleanup: BackgroundCleanup,
}

impl ExternalEditorWatcher {
    pub(super) fn spawn(
        job_id: String,
        remote_path: String,
        raw_path_token: Option<String>,
        local_path: PathBuf,
        transfer_tx: UnboundedSender<TransferJobResult>,
        baseline: LocalFileVersion,
        cleanup: BackgroundCleanup,
    ) -> std::io::Result<Self> {
        let (stop_tx, stop_rx) = mpsc::channel();
        let worker = spawn_joinable("nyaterm-external-editor-watch", move || {
            watch_external_editor_file(
                job_id,
                remote_path,
                raw_path_token,
                local_path,
                transfer_tx,
                stop_rx,
                WatchTimings::default(),
                baseline,
            );
        })?;
        Ok(Self {
            stop_tx: Some(stop_tx),
            worker: Some(worker),
            cleanup,
        })
    }

    fn stop(&mut self) -> Option<JoinHandle<()>> {
        self.stop_tx.take();
        self.worker.take()
    }
}

impl Drop for ExternalEditorWatcher {
    fn drop(&mut self) {
        if let Some(worker) = self.stop() {
            // Session removal can happen on the UI thread while a file read is
            // outstanding. Request stop now, but never join there.
            self.cleanup.submit(move || {
                if worker.join().is_err() {
                    tracing::warn!("external editor watcher panicked during shutdown");
                }
            });
        }
    }
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

#[allow(clippy::too_many_arguments)]
fn watch_external_editor_file(
    job_id: String,
    remote_path: String,
    raw_path_token: Option<String>,
    local_path: PathBuf,
    transfer_tx: UnboundedSender<TransferJobResult>,
    stop_rx: mpsc::Receiver<()>,
    timings: WatchTimings,
    mut baseline: LocalFileVersion,
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
        let current = match LocalFileFingerprint::from_path(&local_path) {
            Ok(fingerprint) => fingerprint,
            // Atomic editor saves may temporarily remove the path.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if wait_for_stop(&stop_rx, timings.interval) {
                    break;
                }
                continue;
            }
            Err(error) => {
                report_watch_error(&transfer_tx, &job_id, error);
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
        let version = match read_local_file_version(&local_path, || {
            wait_for_stop(&stop_rx, Duration::ZERO)
        }) {
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
                report_watch_error(&transfer_tx, &job_id, error);
                break;
            }
        };
        if version.len != current.len || version.modified != current.modified {
            // A save during the settle window needs a full quiet interval.
            first_scan = true;
            continue;
        }
        if version.digest != baseline.digest
            && transfer_tx
                .unbounded_send(TransferJobResult {
                    id: job_id.clone(),
                    event: TransferJobEvent::ExternalModified {
                        remote_path: remote_path.clone(),
                        raw_path_token: raw_path_token.clone(),
                        local_path: local_path.clone(),
                    },
                })
                .is_err()
        {
            break;
        }
        // Advance only to the bytes actually verified above. An event is not an
        // upload acknowledgement, and a newer edit may already be on disk.
        baseline = version;
    }
}

fn report_watch_error(tx: &UnboundedSender<TransferJobResult>, job_id: &str, error: io::Error) {
    let _ = tx.unbounded_send(TransferJobResult {
        id: job_id.to_string(),
        event: TransferJobEvent::Finished(Err(format!("external editor watch failed: {error}"))),
    });
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

    use futures::channel::mpsc::unbounded;
    use nyaterm_files::local_revision::read_local_file_version;

    use super::{ExternalEditorWatcher, WatchTimings, watch_external_editor_file};

    #[gpui::test]
    fn dropping_a_watcher_signals_stop_and_registers_its_join(cx: &mut gpui::TestAppContext) {
        let cleanup = crate::thread_owner::BackgroundCleanup::new(cx.background_executor.clone());
        let (stop_tx, stop_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let worker = crate::thread_owner::spawn_joinable("watch-drop-test", move || {
            let stopped = matches!(
                stop_rx.recv_timeout(Duration::from_secs(5)),
                Err(mpsc::RecvTimeoutError::Disconnected)
            );
            finished_tx.send(stopped).unwrap();
        })
        .unwrap();
        let watcher = ExternalEditorWatcher {
            stop_tx: Some(stop_tx),
            worker: Some(worker),
            cleanup: cleanup.clone(),
        };
        drop(watcher);
        cx.foreground_executor().block_test(cleanup.drain());
        assert!(finished_rx.try_recv().unwrap());
    }

    fn next_modification(
        rx: &mut futures::channel::mpsc::UnboundedReceiver<crate::models::TransferJobResult>,
    ) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(event) = rx.try_recv() {
                assert!(matches!(
                    event.event,
                    crate::models::TransferJobEvent::ExternalModified { .. }
                ));
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
        let worker = crate::thread_owner::spawn_joinable("external-edit-test", move || {
            watch_external_editor_file(
                "job".into(),
                "/remote/edit".into(),
                None,
                worker_path,
                tx,
                stop_rx,
                WatchTimings {
                    interval: Duration::from_millis(5),
                    settle: Duration::from_millis(5),
                },
                baseline,
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
        let worker = crate::thread_owner::spawn_joinable("external-timestamp-test", move || {
            watch_external_editor_file(
                "job".into(),
                "/remote/edit".into(),
                None,
                path,
                tx,
                stop_rx,
                WatchTimings {
                    interval: Duration::from_millis(5),
                    settle: Duration::ZERO,
                },
                baseline,
            )
        })
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
        let worker =
            crate::thread_owner::spawn_joinable("nyaterm-external-watch-test", move || {
                watch_external_editor_file(
                    "job".to_string(),
                    "/remote/file".to_string(),
                    None,
                    worker_path,
                    transfer_tx,
                    stop_rx,
                    WatchTimings {
                        interval: Duration::from_secs(30),
                        settle: Duration::ZERO,
                    },
                    baseline,
                );
            })
            .expect("spawn watcher");

        let started = Instant::now();
        drop(stop_tx);
        worker.join().expect("join watcher");
        assert!(started.elapsed() < Duration::from_secs(1));
        let _ = fs::remove_file(path);
    }
}
