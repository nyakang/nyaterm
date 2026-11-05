use crate::blocking_jobs::{BlockingJobScheduler, JobOutcome};
use crate::models::{TransferJobEvent, TransferJobKind, TransferJobResult, TransferJobState};
use futures::channel::mpsc::UnboundedSender;
use nyaterm_files::sftp::SFTP_TRANSFER_CANCELLED;
use nyaterm_files::sftp_transfer_types::SftpTransferProgress;
use std::path::PathBuf;
use std::time::{Duration, Instant};

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
    // A normal closure sends its own typed result. Only synthesize a terminal
    // event when the scheduler skips the closure or catches its panic.
    let completed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let run_completed = completed.clone();
    let outcome_tx = rejection_tx.clone();
    let outcome_id = rejection_job_id.clone();
    if let Err(error) = scheduler.submit_with_outcome(
        name,
        move |_| {
            run();
            run_completed.store(true, std::sync::atomic::Ordering::Release);
        },
        move |outcome| {
            if completed.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
            let error = match outcome {
                JobOutcome::Cancelled => SFTP_TRANSFER_CANCELLED.to_string(),
                JobOutcome::Failed(_) => "transfer worker panicked".to_string(),
                JobOutcome::Completed => return,
            };
            let _ = outcome_tx.unbounded_send(TransferJobResult {
                id: outcome_id,
                event: TransferJobEvent::Finished(Err(error)),
            });
        },
    ) {
        let _ = rejection_tx.unbounded_send(TransferJobResult {
            id: rejection_job_id,
            event: TransferJobEvent::Finished(Err(error.to_string())),
        });
    }
}

pub(in crate::features) struct TransferProgressEventSender {
    id: String,
    tx: UnboundedSender<TransferJobResult>,
    last_sent_at: Option<Instant>,
    final_sent: bool,
}

impl TransferProgressEventSender {
    pub(in crate::features) fn new(id: String, tx: UnboundedSender<TransferJobResult>) -> Self {
        Self {
            id,
            tx,
            last_sent_at: None,
            final_sent: false,
        }
    }

    pub(in crate::features) fn send(&mut self, progress: SftpTransferProgress) {
        let now = Instant::now();
        let completed = progress
            .total_bytes
            .is_some_and(|total| progress.bytes_transferred >= total)
            && progress.item_count_total.is_none_or(|total| {
                progress
                    .item_count_completed
                    .is_some_and(|completed| completed >= total)
            });
        let due = self.last_sent_at.is_none_or(|last_sent_at| {
            now.duration_since(last_sent_at) >= TRANSFER_PROGRESS_EVENT_INTERVAL
        });
        if completed && self.final_sent {
            return;
        }
        if !completed && !due {
            return;
        }

        self.final_sent |= completed;
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

#[cfg(test)]
mod tests {
    use super::{TransferProgressEventSender, submit_transfer_blocking_job};
    use crate::blocking_jobs::BlockingJobScheduler;
    use crate::models::TransferJobEvent;
    use nyaterm_files::sftp_transfer_types::SftpTransferProgress;

    #[test]
    fn occupied_bulk_executor_does_not_block_interactive_jobs() {
        use std::sync::mpsc;
        use std::time::Duration;
        let bulk = BlockingJobScheduler::with_limits(1, 4).unwrap();
        let interactive = BlockingJobScheduler::with_limits(1, 4).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (events, _) = futures::channel::mpsc::unbounded();
        submit_transfer_blocking_job(&bulk, "paused-transfer", "bulk".into(), events, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (loaded_tx, loaded_rx) = mpsc::channel();
        interactive
            .submit_detached("directory-and-preview", move |_| {
                loaded_tx.send(()).unwrap()
            })
            .unwrap();
        let loaded = loaded_rx.recv_timeout(Duration::from_secs(5));
        release_tx.send(()).unwrap();
        bulk.shutdown();
        interactive.shutdown();
        loaded.unwrap();
    }

    #[test]
    fn panicking_or_rejected_transfer_reports_a_terminal_error() {
        let scheduler = BlockingJobScheduler::with_limits(1, 4).unwrap();
        let (events, mut receiver) = futures::channel::mpsc::unbounded();
        submit_transfer_blocking_job(&scheduler, "panic", "panic".into(), events.clone(), || {
            panic!("fixture")
        });
        let result = futures::executor::block_on(futures::StreamExt::next(&mut receiver)).unwrap();
        assert!(matches!(result.event, TransferJobEvent::Finished(Err(_))));
        scheduler.shutdown();
        submit_transfer_blocking_job(
            &scheduler,
            "rejected",
            "rejected".into(),
            events,
            || unreachable!(),
        );
        let result = receiver.try_recv().unwrap();
        assert_eq!(result.id, "rejected");
        assert!(matches!(result.event, TransferJobEvent::Finished(Err(_))));
    }

    #[test]
    fn shutdown_reports_queued_cancellation_without_running_the_transfer() {
        use std::sync::mpsc;
        use std::time::Duration;
        let scheduler = BlockingJobScheduler::with_limits(1, 4).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        scheduler
            .submit_detached("occupied", move |_| {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (events, mut receiver) = futures::channel::mpsc::unbounded();
        submit_transfer_blocking_job(
            &scheduler,
            "queued",
            "queued".into(),
            events,
            || unreachable!(),
        );
        let cleanup = scheduler.begin_shutdown();
        release_tx.send(()).unwrap();
        cleanup();
        let result = receiver.try_recv().unwrap();
        assert!(
            matches!(result.event, TransferJobEvent::Finished(Err(error)) if error == nyaterm_files::sftp::SFTP_TRANSFER_CANCELLED)
        );
        assert!(receiver.try_recv().is_err());
    }
    #[test]
    fn final_progress_is_emitted_once_and_directory_completion_waits_for_all_items() {
        let (sender, mut receiver) = futures::channel::mpsc::unbounded();
        let mut sender = TransferProgressEventSender::new("job".into(), sender);
        let mut progress = SftpTransferProgress {
            remote_path: "/folder".into(),
            local_path: Default::default(),
            bytes_transferred: 10,
            total_bytes: Some(10),
            item_count_completed: Some(0),
            item_count_total: Some(1),
        };
        sender.send(progress.clone());
        assert!(receiver.try_recv().is_ok());
        assert!(!sender.final_sent);
        progress.item_count_completed = Some(1);
        for _ in 0..100 {
            sender.send(progress.clone());
        }
        assert!(receiver.try_recv().is_ok());
        assert!(receiver.try_recv().is_err());
    }
}
