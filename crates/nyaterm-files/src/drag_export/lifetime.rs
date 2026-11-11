use crate::file_browser::{FileBrowserService, WeakFileBrowserService};
use crate::sftp::SftpTransferControl;
use std::{
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

struct ExportCleanup {
    control: SftpTransferControl,
    run: Arc<dyn Fn() + Send + Sync>,
}

/// One background watcher per gesture, independent of the native worker count.
pub struct DragSourceLifetime {
    task: Option<tokio::task::AbortHandle>,
    stopped: Arc<AtomicBool>,
    controls: Vec<SftpTransferControl>,
    cleanups: Arc<Mutex<Vec<ExportCleanup>>>,
}

impl DragSourceLifetime {
    pub fn new(
        source: impl Into<WeakFileBrowserService>,
        controls: Vec<SftpTransferControl>,
    ) -> io::Result<Self> {
        let source = source.into();
        let stopped = Arc::new(AtomicBool::new(false));
        let cleanups = Arc::new(Mutex::new(Vec::new()));
        let service = source
            .upgrade()
            .ok_or_else(|| io::Error::other("source session is closed"))?;
        let FileBrowserService::Remote(service) = service else {
            let watched = controls.clone();
            let stop = stopped.clone();
            let pending_cleanups = cleanups.clone();
            std::thread::Builder::new()
                .name("adb-drag-lifetime".into())
                .spawn(move || {
                    while !stop.load(Ordering::Acquire) {
                        if source.upgrade().is_none() {
                            for control in &watched {
                                control.cancel();
                            }
                            run_cleanups(&pending_cleanups, true);
                            break;
                        }
                        run_cleanups(&pending_cleanups, false);
                        std::thread::sleep(Duration::from_millis(25));
                    }
                })?;
            return Ok(Self {
                task: None,
                stopped,
                controls,
                cleanups,
            });
        };
        let runtime = service
            .export_sftp_service()
            .export_runtime()
            .map_err(io::Error::other)?;
        let watched = controls.clone();
        let pending_cleanups = cleanups.clone();
        let task = runtime
            .spawn(async move {
                loop {
                    if source.upgrade().is_none() {
                        for control in &watched {
                            control.cancel();
                        }
                        run_cleanups(&pending_cleanups, true);
                        break;
                    }
                    run_cleanups(&pending_cleanups, false);
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            })
            .abort_handle();
        Ok(Self {
            task: Some(task),
            stopped,
            controls,
            cleanups,
        })
    }

    /// Cleanup callbacks enqueue disk cleanup, so native/UI release never does I/O.
    pub fn register_cleanup(
        &self,
        control: SftpTransferControl,
        cleanup: Arc<dyn Fn() + Send + Sync>,
    ) {
        self.cleanups
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(ExportCleanup {
                control,
                run: cleanup,
            });
    }
}

fn run_cleanups(cleanups: &Mutex<Vec<ExportCleanup>>, all: bool) {
    cleanups
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .retain(|cleanup| {
            if all || cleanup.control.is_cancelled() {
                (cleanup.run)();
                false
            } else {
                true
            }
        });
}

impl Drop for DragSourceLifetime {
    fn drop(&mut self) {
        for control in &self.controls {
            control.cancel();
        }
        self.stopped.store(true, Ordering::Release);
        run_cleanups(&self.cleanups, true);
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
