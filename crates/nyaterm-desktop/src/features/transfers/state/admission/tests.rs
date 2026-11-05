use super::TransferAdmission;
use crate::models::{
    TransferJobEvent, TransferJobKind, TransferJobOutput, TransferJobResult, TransferJobState,
    TransferJobStatus,
};
use futures::channel::mpsc::unbounded;
use nyaterm_files::sftp::SftpTransferControl;
use std::collections::HashSet;
use std::sync::mpsc;
use std::time::Duration;

fn job(id: &str, upload: bool) -> TransferJobState {
    TransferJobState {
        id: id.into(),
        session_id: Some(id.into()),
        kind: if upload {
            TransferJobKind::Upload {
                local_path: Default::default(),
                remote_path: "remote".into(),
            }
        } else {
            TransferJobKind::Download {
                remote_path: "remote".into(),
                raw_path_token: None,
                local_path: Default::default(),
            }
        },
        status: TransferJobStatus::Running,
        detail: String::new(),
        created_at_ms: 0,
        display_name: String::new(),
        entries: Vec::new(),
        summary: None,
        progress: None,
        control: Some(SftpTransferControl::new()),
        speed: Default::default(),
    }
}

#[test]
fn direction_limits_pause_readmission_fifo_and_resize_share_one_queue() {
    let mut scheduler = TransferAdmission::default();
    let mut jobs = vec![
        job("d1", false),
        job("d2", false),
        job("d3", false),
        job("u1", true),
    ];
    let (events, _rx) = unbounded();
    let (started, starts) = mpsc::channel();
    let mut releases = Vec::new();
    for job in &mut jobs {
        let id = job.id.clone();
        let tx = started.clone();
        let (release, gate) = mpsc::channel::<()>();
        releases.push(release);
        scheduler.enqueue(
            job,
            "queue-test",
            events.clone(),
            Box::new(move || {
                tx.send(id).unwrap();
                let _ = gate.recv();
            }),
        );
    }
    let mut retired = scheduler.reconcile(&mut jobs, 2, 2);
    let first: HashSet<_> = (0..2)
        .map(|_| starts.recv_timeout(Duration::from_secs(2)).unwrap())
        .collect();
    assert_eq!(first, HashSet::from(["d1".into(), "u1".into()]));
    assert_eq!(jobs[1].status, TransferJobStatus::Queued);
    jobs[0].control.as_ref().unwrap().pause();
    jobs[0].status = TransferJobStatus::Paused;
    retired.extend(scheduler.reconcile(&mut jobs, 2, 2));
    assert_eq!(starts.recv_timeout(Duration::from_secs(2)).unwrap(), "d2");
    jobs[0].status = TransferJobStatus::Queued;
    retired.extend(scheduler.reconcile(&mut jobs, 2, 2));
    assert!(jobs[0].control.as_ref().unwrap().is_paused());
    jobs[1].status = TransferJobStatus::Completed;
    retired.extend(scheduler.reconcile(&mut jobs, 2, 2));
    // The previously queued d3 precedes the resumed d1.
    assert_eq!(starts.recv_timeout(Duration::from_secs(2)).unwrap(), "d3");
    retired.extend(scheduler.reconcile(&mut jobs, 4, 2));
    assert_eq!(jobs[0].status, TransferJobStatus::Running);
    assert!(!jobs[0].control.as_ref().unwrap().is_paused());
    // Readmission resumes the existing worker rather than starting it again.
    assert!(starts.try_recv().is_err());
    drop(releases);
    retired.extend(scheduler.shutdown());
    for worker in retired {
        worker.join().unwrap();
    }
}

#[test]
fn cancelled_queued_work_never_runs_and_completion_releases_once() {
    let mut scheduler = TransferAdmission::default();
    let (events, mut rx) = unbounded();
    let mut jobs = vec![job("cancelled", false)];
    scheduler.enqueue(
        &mut jobs[0],
        "queue-test",
        events,
        Box::new(|| panic!("queued work ran")),
    );
    jobs[0].control.as_ref().unwrap().cancel();
    jobs[0].status = TransferJobStatus::Cancelling;
    assert!(scheduler.reconcile(&mut jobs, 1, 1).is_empty());
    assert_eq!(jobs[0].status, TransferJobStatus::Cancelling);
    assert!(rx.try_recv().is_ok());
    assert!(!scheduler.manages("cancelled"));
    assert!(scheduler.reconcile(&mut jobs, 10, 10).is_empty());
    assert!(rx.try_recv().is_err());
}

#[test]
fn copy_limit_uses_the_smaller_setting_and_retry_reenters_fifo() {
    let mut scheduler = TransferAdmission::default();
    let (events, _rx) = unbounded();
    let mut jobs: Vec<_> = (0..4)
        .map(|i| {
            let mut row = job(&format!("c{i}"), false);
            row.kind = TransferJobKind::SendTo {
                source_path: "/source".into(),
                target_session_id: "other".into(),
                target_path: "/target".into(),
                target_parent_path: "/".into(),
            };
            row
        })
        .collect();
    let (started, starts) = mpsc::channel();
    for row in &mut jobs {
        let tx = started.clone();
        let id = row.id.clone();
        scheduler.enqueue(
            row,
            "copy-queue-test",
            events.clone(),
            Box::new(move || {
                tx.send(id).unwrap();
            }),
        );
    }
    let mut retired = scheduler.reconcile(&mut jobs, 10, 3);
    let admitted: HashSet<_> = (0..2)
        .map(|_| starts.recv_timeout(Duration::from_secs(2)).unwrap())
        .collect();
    assert_eq!(admitted, HashSet::from(["c0".into(), "c1".into()]));
    // Lowering settings leaves active work running and blocks admission.
    retired.extend(scheduler.reconcile(&mut jobs, 1, 1));
    assert_eq!(jobs[2].status, TransferJobStatus::Queued);
    jobs[0].status = TransferJobStatus::Failed;
    jobs[1].status = TransferJobStatus::Completed;
    retired.extend(scheduler.reconcile(&mut jobs, 1, 1));
    assert_eq!(starts.recv_timeout(Duration::from_secs(2)).unwrap(), "c2");
    let tx = started.clone();
    jobs[0].control = Some(SftpTransferControl::new());
    scheduler.enqueue(
        &mut jobs[0],
        "copy-retry-test",
        events,
        Box::new(move || {
            tx.send("retry".into()).unwrap();
        }),
    );
    jobs[2].status = TransferJobStatus::Completed;
    retired.extend(scheduler.reconcile(&mut jobs, 1, 1));
    assert_eq!(starts.recv_timeout(Duration::from_secs(2)).unwrap(), "c3");
    jobs[3].status = TransferJobStatus::Completed;
    retired.extend(scheduler.reconcile(&mut jobs, 1, 1));
    assert_eq!(
        starts.recv_timeout(Duration::from_secs(2)).unwrap(),
        "retry"
    );
    retired.extend(scheduler.shutdown());
    for thread in retired {
        thread.join().unwrap();
    }
}

#[test]
fn cancelling_a_paused_worker_does_not_reacquire_a_direction_slot() {
    let mut scheduler = TransferAdmission::default();
    let (events, _rx) = unbounded();
    let (started, starts) = mpsc::channel();
    let mut jobs = vec![
        job("paused", false),
        job("active", false),
        job("waiting", false),
    ];
    for row in &mut jobs {
        let tx = started.clone();
        let id = row.id.clone();
        scheduler.enqueue(
            row,
            "cancel-pause-test",
            events.clone(),
            Box::new(move || {
                tx.send(id).unwrap();
            }),
        );
    }
    let mut retired = scheduler.reconcile(&mut jobs, 2, 2);
    assert_eq!(
        starts.recv_timeout(Duration::from_secs(2)).unwrap(),
        "paused"
    );
    jobs[0].status = TransferJobStatus::Paused;
    retired.extend(scheduler.reconcile(&mut jobs, 2, 2));
    assert_eq!(
        starts.recv_timeout(Duration::from_secs(2)).unwrap(),
        "active"
    );
    jobs[0].control.as_ref().unwrap().cancel();
    jobs[0].status = TransferJobStatus::Cancelling;
    jobs[1].status = TransferJobStatus::Completed;
    retired.extend(scheduler.reconcile(&mut jobs, 2, 2));
    assert_eq!(
        starts.recv_timeout(Duration::from_secs(2)).unwrap(),
        "waiting"
    );
    retired.extend(scheduler.shutdown());
    for worker in retired {
        worker.join().unwrap();
    }
}

fn delete_job(id: &str) -> TransferJobState {
    let mut row = job(id, false);
    row.kind = TransferJobKind::Delete {
        remote_path: format!("/dir/{id}"),
        batch_id: "batch".into(),
    };
    row
}

#[test]
fn delete_failures_and_panics_release_slots_and_do_not_stop_later_items() {
    let mut scheduler = TransferAdmission::default();
    let (events, mut rx) = unbounded();
    let mut jobs: Vec<_> = ["denied", "panic", "later"]
        .into_iter()
        .map(delete_job)
        .collect();
    for row in &mut jobs {
        let id = row.id.clone();
        let tx = events.clone();
        scheduler.enqueue(
            row,
            "delete-failures",
            events.clone(),
            Box::new(move || {
                let result = match id.as_str() {
                    "denied" => Err("permission denied".into()),
                    "panic" => panic!("delete test worker"),
                    _ => Ok(TransferJobOutput::Deleted),
                };
                tx.unbounded_send(TransferJobResult {
                    id,
                    event: TransferJobEvent::Finished(result),
                })
                .unwrap();
            }),
        );
    }
    let mut retired = scheduler.reconcile(&mut jobs, 1, 1);
    assert_eq!(jobs[2].status, TransferJobStatus::Queued);
    let mut outcomes = std::collections::HashMap::new();
    for _ in 0..3 {
        let event = futures::executor::block_on(futures::StreamExt::next(&mut rx)).unwrap();
        let TransferJobEvent::Finished(result) = event.event else {
            panic!("terminal event")
        };
        let row = jobs.iter_mut().find(|row| row.id == event.id).unwrap();
        row.status = if result.is_ok() {
            TransferJobStatus::Completed
        } else {
            TransferJobStatus::Failed
        };
        outcomes.insert(event.id, result);
        retired.extend(scheduler.reconcile(&mut jobs, 10, 10));
    }
    assert_eq!(
        outcomes["denied"].as_ref().unwrap_err(),
        "permission denied"
    );
    assert_eq!(
        outcomes["panic"].as_ref().unwrap_err(),
        "transfer worker panicked"
    );
    assert!(outcomes["later"].is_ok());
    assert!(scheduler.tasks.is_empty());
    assert!(rx.try_recv().is_err());
    for worker in retired {
        worker.join().unwrap();
    }
}

#[test]
fn delete_worker_start_failure_reports_once_and_releases_its_slot() {
    let mut scheduler = TransferAdmission::default();
    let (events, mut rx) = unbounded();
    let mut jobs = vec![delete_job("start-failed"), delete_job("next")];
    for row in &mut jobs {
        scheduler.enqueue(row, "delete-start-failed", events.clone(), Box::new(|| {}));
    }
    let mut attempts = 0;
    assert!(
        scheduler
            .reconcile_with_spawn(&mut jobs, 10, 10, |_, _| {
                attempts += 1;
                Err(std::io::Error::other("fixture: cannot create thread"))
            })
            .is_empty()
    );
    assert_eq!(attempts, 2);
    for _ in 0..2 {
        let event = rx.try_recv().unwrap();
        let TransferJobEvent::Finished(Err(error)) = event.event else {
            panic!("start failure")
        };
        assert_eq!(error, "failed to start transfer worker");
        jobs.iter_mut()
            .find(|row| row.id == event.id)
            .unwrap()
            .status = TransferJobStatus::Failed;
    }
    assert!(scheduler.reconcile(&mut jobs, 1, 1).is_empty());
    assert!(scheduler.tasks.is_empty());
    assert!(rx.try_recv().is_err());
    jobs.push(delete_job("retry"));
    let (started, starts) = mpsc::channel();
    scheduler.enqueue(
        &mut jobs[2],
        "delete-retry",
        events,
        Box::new(move || {
            started.send(()).unwrap();
        }),
    );
    let mut retired = scheduler.reconcile(&mut jobs, 1, 1);
    starts.recv_timeout(Duration::from_secs(5)).unwrap();
    retired.extend(scheduler.shutdown());
    for worker in retired {
        worker.join().unwrap();
    }
}

#[test]
fn delete_cancelled_after_admission_never_enters_the_operation() {
    let mut scheduler = TransferAdmission::default();
    let (events, mut rx) = unbounded();
    let mut jobs = vec![delete_job("cancel-before-start")];
    let control = jobs[0].control.clone().unwrap();
    scheduler.enqueue(
        &mut jobs[0],
        "cancel-before-start",
        events,
        Box::new(|| panic!("cancelled deletion entered the operation")),
    );
    let mut retired = scheduler.reconcile_with_spawn(&mut jobs, 1, 1, |name, work| {
        control.cancel();
        crate::thread_owner::spawn_joinable(name, work)
    });
    let result = futures::executor::block_on(futures::StreamExt::next(&mut rx)).unwrap();
    assert!(
        matches!(result.event, TransferJobEvent::Finished(Err(ref error))
        if error == nyaterm_files::sftp::SFTP_TRANSFER_CANCELLED)
    );
    jobs[0].status = TransferJobStatus::Cancelled;
    retired.extend(scheduler.reconcile(&mut jobs, 1, 1));
    assert!(scheduler.tasks.is_empty());
    assert!(rx.try_recv().is_err());
    for worker in retired {
        worker.join().unwrap();
    }
}
