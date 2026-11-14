//! Owned runtime for watching files opened in an external editor.

use crate::thread_owner::{BackgroundCleanup, spawn_joinable};
use nyaterm_files::external_edit::watch::{ExternalEditEvent, watch_external_edit};
use nyaterm_files::local_revision::LocalFileVersion;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::JoinHandle;

use futures::channel::mpsc::UnboundedSender;

use crate::models::{TransferJobEvent, TransferJobResult};

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
            watch_external_edit(&local_path, baseline, stop_rx, |event| {
                let event = match event {
                    ExternalEditEvent::Modified => TransferJobEvent::ExternalModified {
                        remote_path: remote_path.clone(),
                        raw_path_token: raw_path_token.clone(),
                        local_path: local_path.clone(),
                    },
                    ExternalEditEvent::Failed(error) => TransferJobEvent::Finished(Err(format!(
                        "external editor watch failed: {error}"
                    ))),
                };
                transfer_tx
                    .unbounded_send(TransferJobResult {
                        id: job_id.clone(),
                        event,
                    })
                    .is_ok()
            });
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

#[cfg(test)]
mod tests {
    use super::ExternalEditorWatcher;
    use std::{sync::mpsc, time::Duration};
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
}
