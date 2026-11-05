use crate::models::{TransferJobEvent, TransferJobOutput, TransferJobResult};
use futures::channel::mpsc::UnboundedSender;
use gpui::PromisedFileProvider;
use nyaterm_files::remote_file::RemoteFileService;
use nyaterm_files::sftp::{SftpFileEntry, SftpTransferControl};
use nyaterm_files::sftp_transfer_types::{SftpPathTransferOptions, SftpTransferOptions};
use std::{
    io,
    path::Path,
    sync::{Arc, Weak},
};

pub(super) struct RemoteDownloadProvider {
    session_id: String,
    service: Weak<RemoteFileService>,
    entry: SftpFileEntry,
    options: SftpPathTransferOptions,
    _lifetime: Arc<nyaterm_files::drag_export::DragSourceLifetime>,
    sender: UnboundedSender<TransferJobResult>,
    control: SftpTransferControl,
}
impl RemoteDownloadProvider {
    pub(super) fn new(
        session_id: String,
        service: Weak<RemoteFileService>,
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
        let source = self
            .service
            .upgrade()
            .ok_or_else(|| io::Error::other("source session is closed"))?;
        // A snapshot owns the active operation; the authoritative connection can
        // still disappear and cancel it, even during a stalled network request.
        let service = (*source).clone();
        drop(source);
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
                let name = target
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| anyhow::anyhow!("invalid promised destination"))?;
                nyaterm_files::download_path::validate_name(name)?;
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
                anyhow::ensure!(!summary.skipped, "promised destination already exists");
                Ok(summary)
            }))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("drag download failed")))
            .map_err(|error| error.to_string());
        match result {
            Ok(summary) => {
                send(TransferJobEvent::Finished(Ok(TransferJobOutput::Summary(
                    summary,
                ))));
                Ok(())
            }
            Err(error) => {
                send(TransferJobEvent::Finished(Err(error.clone())));
                Err(io::Error::other(error))
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
