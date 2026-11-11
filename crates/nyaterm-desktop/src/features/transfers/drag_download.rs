use crate::models::{TransferJobEvent, TransferJobOutput, TransferJobResult};
use futures::channel::mpsc::UnboundedSender;
use gpui::PromisedFileProvider;
use nyaterm_files::file_browser::WeakFileBrowserService;
use nyaterm_files::sftp::{SftpFileEntry, SftpTransferControl};
use nyaterm_files::sftp_transfer_types::{SftpPathTransferOptions, SftpTransferOptions};
use std::{io, path::Path, sync::Arc};

pub(super) struct RemoteDownloadProvider {
    session_id: String,
    service: WeakFileBrowserService,
    entry: SftpFileEntry,
    options: SftpPathTransferOptions,
    _lifetime: Arc<nyaterm_files::drag_export::DragSourceLifetime>,
    sender: UnboundedSender<TransferJobResult>,
    control: SftpTransferControl,
}
impl RemoteDownloadProvider {
    pub(super) fn new(
        session_id: String,
        service: WeakFileBrowserService,
        entry: SftpFileEntry,
        options: SftpTransferOptions,
        sender: UnboundedSender<TransferJobResult>,
        control: SftpTransferControl,
        lifetime: Arc<nyaterm_files::drag_export::DragSourceLifetime>,
    ) -> Self {
        Self {
            session_id,
            service,
            entry: entry.clone(),
            options: SftpPathTransferOptions::for_promised_download(options, entry.file_type),
            _lifetime: lifetime,
            sender,
            control,
        }
    }
}
impl PromisedFileProvider for RemoteDownloadProvider {
    fn write_to(&self, target: &Path) -> io::Result<()> {
        let control = self.control.child();
        let path_options = self.options.clone();
        let remote_path = self.entry.remote_path();
        let id = format!("drag-download-{}", nyaterm_core::uuid());
        let send = |event| {
            let _ = self.sender.unbounded_send(TransferJobResult {
                id: id.clone(),
                event,
            });
        };
        send(TransferJobEvent::DragExportOpened {
            session_id: self.session_id.clone(),
            remote_path: remote_path.display_path.clone(),
            control: control.clone(),
            destination: Some(crate::models::PromisedDownloadDestination {
                raw_path_token: remote_path.raw_path_token.clone(),
                local_path: target.to_path_buf(),
                source: self.service.clone(),
                options: path_options
                    .with_transfer_options(path_options.transfer_options().clone()),
            }),
        });
        let sender = self.sender.clone();
        let mut progress_sender =
            super::transfer_jobs::TransferProgressEventSender::new(id.clone(), sender);
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> anyhow::Result<_> {
                control.check_cancelled()?;
                let service = self.service.upgrade().ok_or_else(|| {
                    anyhow::Error::new(
                        nyaterm_files::file_browser::FileTransferEntryError::Unavailable,
                    )
                })?;
                let name = target.file_name().and_then(|name| name.to_str()).ok_or(
                    nyaterm_files::file_browser::FileTransferEntryError::InvalidDestination,
                )?;
                nyaterm_files::download_path::validate_name(name).map_err(|_| {
                    nyaterm_files::file_browser::FileTransferEntryError::InvalidDestination
                })?;
                if matches!(service, nyaterm_files::file_browser::FileBrowserService::Adb(_))
                    && self.entry.is_directory()
                {
                    // Finder requests the promise on a worker. Validate the same
                    // inventory limits and representable names as virtual exports
                    // before creating anything in its negotiated destination.
                    nyaterm_files::drag_export::tree::enumerate_file_browser_drag_with_root_controls(
                        &service,
                        vec![self.entry.clone()],
                        &control,
                        vec![control.clone()],
                    )?;
                }
                // Finder resolves top-level naming conflicts. A race must never turn
                // its negotiated promise into an overwrite of another user's file.
                let summary = service.download_remote_path_with_progress_and_path_options(
                    &remote_path,
                    target.to_path_buf(),
                    control,
                    path_options.with_transfer_options(path_options.transfer_options().clone()),
                    move |progress| {
                        progress_sender.send(progress);
                    },
                )?;
                anyhow::ensure!(
                    !summary.skipped,
                    nyaterm_files::file_browser::FileTransferEntryError::TargetExists
                );
                Ok(summary)
            }))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("drag download failed")));
        if let Err(error) = &result {
            crate::features::android::labels::log_file_error(
                "drag-download",
                self.service.kind(),
                &id,
                error,
            );
        }
        match result {
            Ok(summary) => {
                send(TransferJobEvent::FileFinished(Ok(
                    TransferJobOutput::Summary(summary),
                )));
                Ok(())
            }
            Err(error) => {
                let message = crate::features::android::labels::file_error_message(&error);
                send(TransferJobEvent::FileFinished(Err(error)));
                Err(io::Error::other(message))
            }
        }
    }
    fn cancel(&self) {
        self.control.cancel();
    }
}
impl Drop for RemoteDownloadProvider {
    fn drop(&mut self) {
        self.control.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::RemoteDownloadProvider;
    use crate::features::android::labels::file_error_code;
    use crate::models::TransferJobEvent;
    use gpui::PromisedFileProvider as _;
    use nyaterm_files::adb::AdbFileService;
    use nyaterm_files::drag_export::DragSourceLifetime;
    use nyaterm_files::file_browser::FileBrowserService;
    use nyaterm_files::sftp::{SftpFileEntry, SftpFileType, SftpTransferControl};
    use std::sync::Arc;

    #[test]
    fn cancelled_file_promises_create_a_terminal_task_without_fetching_content() {
        let service = FileBrowserService::Adb(Arc::new(AdbFileService::new(
            nyaterm_android::adb::client::DeviceBinding {
                server: "127.0.0.1:1".parse().unwrap(),
                serial: "fixture".into(),
                transport_id: None,
                generation: 1,
            },
        )));
        let weak = service.downgrade().unwrap();
        let control = SftpTransferControl::new();
        let lifetime =
            Arc::new(DragSourceLifetime::new(weak.clone(), vec![control.clone()]).unwrap());
        let (sender, mut receiver) = futures::channel::mpsc::unbounded();
        let provider = RemoteDownloadProvider::new(
            "original".into(),
            weak,
            SftpFileEntry {
                name: "file".into(),
                path: "/device/file".into(),
                file_type: SftpFileType::File,
                size: Some(7),
                permissions: None,
                owner: String::new(),
                group: String::new(),
                modified_at: None,
                raw_path_token: None,
                symlink_target_is_directory: false,
            },
            Default::default(),
            sender,
            control,
            lifetime,
        );
        provider.cancel();
        assert!(provider.write_to(std::path::Path::new("unused")).is_err());
        let opened = receiver.try_recv().unwrap();
        let TransferJobEvent::DragExportOpened {
            session_id,
            destination,
            ..
        } = opened.event
        else {
            panic!("promise must create a task")
        };
        assert_eq!(session_id, "original");
        assert!(destination.is_some());
        let finished = receiver.try_recv().unwrap();
        assert_eq!(finished.id, opened.id);
        let TransferJobEvent::FileFinished(Err(error)) = finished.event else {
            panic!("cancelled promise must finish")
        };
        assert_eq!(file_error_code(&error), "Cancelled");
        assert!(receiver.try_recv().is_err());
    }
}
