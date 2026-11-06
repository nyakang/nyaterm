use crate::remote_file::RemoteFileService;
use crate::sftp::SftpTransferControl;
use std::{io, sync::Weak, time::Duration};

/// One asynchronous watcher per gesture, independent of the native worker count.
pub struct DragSourceLifetime {
    task: tokio::task::AbortHandle,
    controls: Vec<SftpTransferControl>,
}

impl DragSourceLifetime {
    pub fn new(
        source: Weak<RemoteFileService>,
        controls: Vec<SftpTransferControl>,
    ) -> io::Result<Self> {
        let runtime = source
            .upgrade()
            .ok_or_else(|| io::Error::other("source session is closed"))?
            .export_sftp_service()
            .export_runtime()
            .map_err(io::Error::other)?;
        let watched = controls.clone();
        let task = runtime
            .spawn(async move {
                loop {
                    if source.upgrade().is_none() {
                        for control in &watched {
                            control.cancel();
                        }
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            })
            .abort_handle();
        Ok(Self { task, controls })
    }
}

impl Drop for DragSourceLifetime {
    fn drop(&mut self) {
        for control in &self.controls {
            control.cancel();
        }
        self.task.abort();
    }
}
