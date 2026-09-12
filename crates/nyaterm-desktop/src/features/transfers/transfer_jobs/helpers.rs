use futures::channel::mpsc::UnboundedSender;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use nyaterm_transport::{SFTP_TRANSFER_CANCELLED, SftpTransferProgress};

use crate::blocking_jobs::BlockingJobScheduler;
use crate::models::{TransferJobEvent, TransferJobKind, TransferJobResult, TransferJobState};

const TRANSFER_PROGRESS_EVENT_INTERVAL: Duration = Duration::from_millis(50);

/// Records the final error returned by an SFTP upload job at the asynchronous boundary.
///
/// The transport layer records protocol and retry details; this adds the desktop task ID
/// so log entries can be matched to failed rows in the transfer panel. User cancellations
/// are represented by the job state and are not logged as failures again.
pub(super) fn log_sftp_upload_job_failure(job_id: &str, error: &anyhow::Error) {
    if error.to_string().contains(SFTP_TRANSFER_CANCELLED) {
        return;
    }
    tracing::error!(
        transfer_id = %job_id,
        error = %error,
        error_chain = %format!("{error:#}"),
        "SFTP upload job failed"
    );
}

pub(in crate::features) fn submit_transfer_blocking_job(
    scheduler: &BlockingJobScheduler,
    name: &'static str,
    rejection_job_id: String,
    rejection_tx: UnboundedSender<TransferJobResult>,
    run: impl FnOnce() + Send + 'static,
) {
    if let Err(error) = scheduler.submit_detached(name, move |_| run()) {
        let _ = rejection_tx.unbounded_send(TransferJobResult {
            id: rejection_job_id,
            event: TransferJobEvent::Finished(Err(error.to_string())),
        });
    }
}

pub(super) struct TransferProgressEventSender {
    id: String,
    tx: UnboundedSender<TransferJobResult>,
    last_sent_at: Option<Instant>,
}

impl TransferProgressEventSender {
    pub(super) fn new(id: String, tx: UnboundedSender<TransferJobResult>) -> Self {
        Self {
            id,
            tx,
            last_sent_at: None,
        }
    }

    pub(super) fn send(&mut self, progress: SftpTransferProgress) {
        let now = Instant::now();
        let completed = progress
            .total_bytes
            .is_some_and(|total| progress.bytes_transferred >= total);
        let due = self.last_sent_at.is_none_or(|last_sent_at| {
            now.duration_since(last_sent_at) >= TRANSFER_PROGRESS_EVENT_INTERVAL
        });
        if !completed && !due {
            return;
        }

        self.last_sent_at = Some(now);
        let _ = self.tx.unbounded_send(TransferJobResult {
            id: self.id.clone(),
            event: TransferJobEvent::Progress(progress),
        });
    }
}

pub(super) fn transfer_job_remote_parent_path(path: &str) -> String {
    let path = path.trim_end_matches('/');
    match path.rfind('/') {
        Some(0) => "/".to_string(),
        Some(index) => path[..index].to_string(),
        None => ".".to_string(),
    }
}

pub(super) fn transfer_job_local_target_path(job: &TransferJobState) -> Option<PathBuf> {
    job.summary
        .as_ref()
        .map(|summary| summary.local_path.clone())
        .or_else(|| {
            job.progress
                .as_ref()
                .map(|progress| progress.local_path.clone())
        })
        .or_else(|| match &job.kind {
            TransferJobKind::Download { local_path, .. }
            | TransferJobKind::OpenExternal { local_path, .. } => Some(local_path.clone()),
            _ => None,
        })
}
