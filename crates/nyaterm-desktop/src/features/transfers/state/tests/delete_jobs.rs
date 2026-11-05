use super::{transfer_job, transfer_state};
use crate::features::transfers::file_operations::{
    FileMutation, FileMutationIntent, FileOperationContext, FileOperationEndpoint,
    FileOperationRequest,
};
use crate::models::{
    TransferJobEvent, TransferJobKind, TransferJobOutput, TransferJobResult, TransferJobStatus,
};
use gpui::TestAppContext;
use nyaterm_files::file_browser::{FileBrowserBackendKind, FileBrowserService};
use nyaterm_files::sftp::RemoteFilePath;
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

#[test]
fn thousand_deletes_from_both_entry_points_share_two_slots_and_complete_each_file_once() {
    let cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = transfer_state(&cx);
    let root = tempfile::tempdir().unwrap();
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let (started, starts) = mpsc::channel();
    let mut releases = Vec::new();
    let mut batch_jobs = [Vec::new(), Vec::new()];
    let tx = state.transfer_event_sender();
    let mut rx = state.take_transfer_event_receiver().unwrap();
    for index in 0..1000 {
        let id = format!("delete-{index}");
        let path = root.path().join(format!("test_{index}"));
        std::fs::write(&path, []).unwrap();
        let path = RemoteFilePath::new(path.to_string_lossy());
        let mut row = transfer_job(&id, "session", TransferJobStatus::Running, true);
        if index < 500 {
            row.kind = TransferJobKind::Delete {
                remote_path: path.display_path.clone(),
                batch_id: format!("batch-{}", index / 250),
            };
            batch_jobs[index / 250].push((id.clone(), path.clone()));
        } else {
            // File-manager mutations retain their SendTo row for visibility and retries.
            row.kind = TransferJobKind::SendTo {
                source_path: path.display_path.clone(),
                target_session_id: "session".into(),
                target_path: path.display_path.clone(),
                target_parent_path: root.path().to_string_lossy().into_owned(),
            };
            state.track_file_operation(
                id.clone(),
                FileOperationContext {
                    request: FileOperationRequest::Mutation(FileMutationIntent {
                        endpoint: FileOperationEndpoint::Session("session".into()),
                        directory: RemoteFilePath::new(root.path().to_string_lossy()),
                        mutation: FileMutation::Delete(path.clone()),
                    }),
                    source_label: "session".into(),
                    target_label: "session".into(),
                },
            );
        }
        state.enqueue_transfer_job(row);
        assert!(state.supports_bulk_admission(&id));
        let gate = (index < 2).then(|| {
            let (release, gate) = mpsc::channel();
            releases.push(release);
            gate
        });
        let worker_active = active.clone();
        let worker_peak = peak.clone();
        let worker_started = started.clone();
        let worker_tx = tx.clone();
        let worker_id = id.clone();
        state.admit_bulk_job(
            &id,
            "bulk-delete-regression",
            tx.clone(),
            Box::new(move || {
                let running = worker_active.fetch_add(1, Ordering::SeqCst) + 1;
                worker_peak.fetch_max(running, Ordering::SeqCst);
                worker_started.send(index).unwrap();
                if let Some(gate) = gate {
                    gate.recv_timeout(Duration::from_secs(30)).unwrap();
                }
                let result = FileBrowserService::local()
                    .delete_remote_path(&path)
                    .map(|_| TransferJobOutput::Deleted)
                    .map_err(|error| error.to_string());
                worker_active.fetch_sub(1, Ordering::SeqCst);
                worker_tx
                    .unbounded_send(TransferJobResult {
                        id: worker_id,
                        event: TransferJobEvent::Finished(result),
                    })
                    .unwrap();
            }),
        );
    }
    for (index, jobs) in batch_jobs.into_iter().enumerate() {
        state.begin_delete_batch(
            format!("batch-{index}"),
            FileBrowserBackendKind::Local,
            jobs,
        );
    }
    assert_eq!(state.pause_visible_transfer_jobs(Some("session")), 0);
    state.reconcile_bulk_jobs(10, 10);
    let first: HashSet<_> = (0..2)
        .map(|_| starts.recv_timeout(Duration::from_secs(5)).unwrap())
        .collect();
    assert_eq!(first, HashSet::from([0, 1]));
    assert!(starts.try_recv().is_err());
    assert_eq!(
        state
            .queue
            .jobs
            .iter()
            .filter(|row| row.status == TransferJobStatus::Queued)
            .count(),
        998
    );
    assert!(state.bulk_executor.is_none());
    for release in releases {
        release.send(()).unwrap();
    }
    let mut seen = HashSet::new();
    let mut outcomes = Vec::new();
    for index in 0..1000 {
        let event = futures::executor::block_on(futures::StreamExt::next(&mut rx)).unwrap();
        assert!(seen.insert(event.id.clone()), "duplicate terminal event");
        let TransferJobEvent::Finished(result) = event.event else {
            panic!("terminal result")
        };
        assert!(result.is_ok(), "{result:?}");
        let (position, mut row) = state.take_transfer_job_for_event(&event.id).unwrap();
        if let TransferJobKind::Delete { batch_id, .. } = &row.kind {
            if let Some(outcome) = state.settle_delete_job(batch_id, &event.id, result.map(|_| ()))
            {
                outcomes.push(outcome);
            }
        } else {
            row.status = TransferJobStatus::Completed;
            state.restore_transfer_job_after_event((position, row));
        }
        // Deletion concurrency is independent of upload/download preference changes.
        let setting = if index % 2 == 0 { 1 } else { 10 };
        state.reconcile_bulk_jobs(setting, setting);
    }
    assert_eq!(peak.load(Ordering::SeqCst), 2);
    assert_eq!(seen.len(), 1000);
    assert_eq!(outcomes.len(), 2);
    for outcome in outcomes {
        assert_eq!(outcome.deleted.len(), 250);
        assert!(outcome.errors.is_empty());
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    assert!(state.bulk_executor.is_none());
    assert!(rx.try_recv().is_err());
    let cleanup = state.begin_bulk_shutdown();
    cleanup();
    cx.foreground_executor()
        .block_test(state.worker_cleanup.drain());
}

#[test]
fn disconnect_cancels_waiting_deletes_but_preserves_results_of_running_deletes() {
    let cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = transfer_state(&cx);
    let (started, starts) = mpsc::channel();
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let mut releases = Vec::new();
    for index in 0..4 {
        let id = format!("delete-{index}");
        let mut row = transfer_job(
            &id,
            if index < 3 { "closed" } else { "other" },
            TransferJobStatus::Running,
            true,
        );
        row.kind = TransferJobKind::Delete {
            remote_path: format!("/dir/{id}"),
            batch_id: "batch".into(),
        };
        state.enqueue_transfer_job(row);
        let gate = (index < 2).then(|| {
            let (release, gate) = mpsc::channel();
            releases.push(release);
            gate
        });
        let worker_started = started.clone();
        let worker_tx = tx.clone();
        let worker_id = id.clone();
        state.admit_bulk_job(
            &id,
            "disconnect-delete",
            tx.clone(),
            Box::new(move || {
                assert_ne!(index, 2, "a queued delete ran after disconnect");
                worker_started.send(index).unwrap();
                if let Some(gate) = gate {
                    gate.recv_timeout(Duration::from_secs(30)).unwrap();
                }
                worker_tx
                    .unbounded_send(TransferJobResult {
                        id: worker_id,
                        event: TransferJobEvent::Finished(Ok(TransferJobOutput::Deleted)),
                    })
                    .unwrap();
            }),
        );
    }
    state.reconcile_bulk_jobs(1, 1);
    for _ in 0..2 {
        starts.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    state.cancel_bulk_jobs_for_session("closed");
    state.reconcile_bulk_jobs(1, 1);
    let cancelled = rx.try_recv().unwrap();
    assert_eq!(cancelled.id, "delete-2");
    assert!(
        matches!(cancelled.event, TransferJobEvent::Finished(Err(ref error)) if error == nyaterm_files::sftp::SFTP_TRANSFER_CANCELLED)
    );
    state.take_transfer_job_for_event(&cancelled.id).unwrap();
    assert_eq!(
        state.transfer_job("delete-3").unwrap().status,
        TransferJobStatus::Queued
    );
    for release in releases {
        release.send(()).unwrap();
    }
    let mut completed = HashSet::new();
    for _ in 0..3 {
        let event = futures::executor::block_on(futures::StreamExt::next(&mut rx)).unwrap();
        assert!(matches!(event.event, TransferJobEvent::Finished(Ok(_))));
        completed.insert(event.id.clone());
        state.take_transfer_job_for_event(&event.id).unwrap();
        state.reconcile_bulk_jobs(10, 10);
    }
    assert_eq!(
        completed,
        HashSet::from(["delete-0".into(), "delete-1".into(), "delete-3".into()])
    );
    assert!(rx.try_recv().is_err());
    let cleanup = state.begin_bulk_shutdown();
    cleanup();
    cx.foreground_executor()
        .block_test(state.worker_cleanup.drain());
}
