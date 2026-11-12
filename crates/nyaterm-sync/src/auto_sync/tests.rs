use super::{
    AutoSyncCompletion, AutoSyncSchedule, AutoSyncTrigger, auto_sync_backoff,
    permanent_auto_sync_error,
};
use nyaterm_core::CloudSyncError;
use std::time::{Duration, Instant};

#[test]
fn oversized_local_replica_pauses_auto_sync_until_settings_change() {
    let now = Instant::now();
    let mut coordinator = AutoSyncSchedule::new(now, 0);
    let error = CloudSyncError::PortableSnapshot(
        nyaterm_core::PortableSnapshotError::LocalReplicaPayloadTooLarge {
            limit_bytes: 256 * 1024 * 1024,
        },
    );
    coordinator.complete(
        now,
        AutoSyncTrigger::Startup,
        &AutoSyncCompletion::from_error(&error),
        None,
    );
    assert!(
        coordinator
            .next_job(now + Duration::from_secs(16 * 60), 0, || true)
            .is_none()
    );
    coordinator.settings_changed(now);
    assert!(coordinator.next_job(now, 0, || true).is_some());
}

#[test]
fn storage_errors_pause_only_when_configuration_or_access_must_change() {
    use nyaterm_core::cloud_sync::error::CloudStorageErrorKind;
    for (kind, permanent) in [
        (CloudStorageErrorKind::Tls, true),
        (CloudStorageErrorKind::PermissionDenied, true),
        (CloudStorageErrorKind::InvalidConfiguration, true),
        (CloudStorageErrorKind::Unsupported, true),
        (CloudStorageErrorKind::Dns, false),
        (CloudStorageErrorKind::Connect, false),
        (CloudStorageErrorKind::Timeout, false),
        (CloudStorageErrorKind::ConnectionReset, false),
        (CloudStorageErrorKind::Conflict, false),
    ] {
        let error = CloudSyncError::Storage {
            provider: "webdav".into(),
            operation: "read".into(),
            kind,
        };
        assert_eq!(permanent_auto_sync_error(&error), permanent);
        assert_eq!(error.classification(), kind.classification());
    }
}

#[test]
fn completion_observes_changes_before_their_notifications_arrive() {
    let now = Instant::now();
    let mut sync = AutoSyncSchedule::new(now, 4);
    let job = sync
        .next_job(sync.next_due, 4, || true)
        .expect("startup sync");
    sync.complete(now, job.trigger, &AutoSyncCompletion::Success, Some(5));
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Change));
    assert!(sync.change_since.is_some());
    let due = sync.next_due;
    sync.record_sync_mutation(now, 5);
    assert_eq!(sync.next_due, due, "late notification keeps the debounce");
}

#[test]
fn dispatch_captures_generation_and_keeps_changes_made_during_the_job() {
    let now = Instant::now();
    let mut sync = AutoSyncSchedule::new(now, 4);
    sync.pending = None;
    sync.record_sync_mutation(now, 5);
    let now = sync.next_due;
    let job = sync.next_job(now, 5, || true).expect("change is due");
    assert_eq!(job.trigger, AutoSyncTrigger::Change);
    assert_eq!(job.change_elapsed, Duration::from_secs(1));
    assert_eq!(sync.in_flight_generation, 5);
    assert!(sync.next_job(now, 5, || true).is_none());

    sync.record_sync_mutation(now, 6);
    sync.complete(now, job.trigger, &AutoSyncCompletion::Success, None);
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Change));
    assert!(sync.change_since.is_some());
    let next = sync
        .next_job(sync.next_due, 6, || true)
        .expect("new change remains scheduled");
    assert_eq!(next.trigger, AutoSyncTrigger::Change);
    assert_eq!(sync.in_flight_generation, 6);
}

#[test]
fn deferred_pull_waits_for_safe_windows_and_does_not_replace_a_local_change() {
    let now = Instant::now();
    let mut sync = AutoSyncSchedule::new(now, 0);
    sync.pending = None;
    sync.remote_available = true;
    assert!(sync.next_job(now, 0, || false).is_none());
    assert_eq!(sync.pending, None);
    let job = sync.next_job(now, 0, || true).expect("safe pull");
    assert_eq!(job.trigger, AutoSyncTrigger::DeferredPull);

    sync.running = false;
    sync.record_sync_mutation(now, 1);
    let job = sync
        .next_job(sync.next_due, 1, || true)
        .expect("local change takes precedence");
    assert_eq!(job.trigger, AutoSyncTrigger::Change);
}

#[test]
fn periodic_checks_cannot_be_starved_by_continuous_changes_and_still_respect_pause() {
    let now = Instant::now();
    let mut sync = AutoSyncSchedule::new(now, 0);
    sync.pending = None;
    sync.record_sync_mutation(now, 1);
    sync.next_periodic = now;
    sync.paused = true;
    assert!(
        sync.next_job(now, 1, || panic!("paused tick reads no windows"))
            .is_none()
    );
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Periodic));
    sync.paused = false;
    sync.record_sync_mutation(now, 2);
    assert_eq!(
        sync.next_job(now, 2, || true).unwrap().trigger,
        AutoSyncTrigger::Periodic
    );
    sync.running = false;
    sync.settings_changed(now);
    sync.record_sync_mutation(now, 3);
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Settings));
}

#[test]
fn auto_sync_focus_is_throttled_and_settings_resume_paused_checks() {
    let now = Instant::now();
    let mut sync = AutoSyncSchedule::new(now, 0);
    sync.pending = None;
    sync.focused(now);
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Focus));
    sync.pending = None;
    sync.focused(now);
    assert_eq!(sync.pending, None);
    sync.paused = true;
    sync.failures = 3;
    sync.settings_changed(now);
    assert!(!sync.paused);
    assert_eq!(sync.failures, 0);
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Settings));
}

#[test]
fn auto_sync_backoff_and_auth_pause_policy() {
    assert_eq!(auto_sync_backoff(0), Duration::from_secs(60));
    assert_eq!(auto_sync_backoff(1), Duration::from_secs(300));
    assert_eq!(auto_sync_backoff(2), Duration::from_secs(900));
    assert_eq!(auto_sync_backoff(3), Duration::from_secs(3_600));
    assert_eq!(auto_sync_backoff(99), Duration::from_secs(3_600));
    assert!(permanent_auto_sync_error(
        &nyaterm_core::CloudSyncError::Remote("HTTP 401 Unauthorized".into())
    ));
    assert!(!permanent_auto_sync_error(
        &nyaterm_core::CloudSyncError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "network timeout"
        ))
    ));
}

#[test]
fn completing_an_older_sync_preserves_changes_received_while_running() {
    let now = Instant::now();
    let mut sync = AutoSyncSchedule::new(now, 4);
    sync.running = true;
    sync.pending = None;
    sync.record_sync_mutation(now, 5);
    let due = sync.next_due;
    let changed = sync.change_since;

    sync.complete(
        now,
        AutoSyncTrigger::Change,
        &AutoSyncCompletion::Success,
        None,
    );

    assert!(!sync.running());
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Change));
    assert_eq!(sync.change_since, changed);
    assert_eq!(sync.next_due, due);

    sync.in_flight_generation = 5;
    sync.running = true;
    sync.complete(
        now,
        AutoSyncTrigger::Change,
        &AutoSyncCompletion::Success,
        None,
    );
    assert_eq!(sync.pending, None);
    assert_eq!(sync.change_since, None);
}

#[test]
fn duplicate_or_stale_mutations_do_not_extend_the_debounce() {
    let now = Instant::now();
    let mut sync = AutoSyncSchedule::new(now, 4);
    sync.pending = None;
    sync.record_sync_mutation(now, 5);
    let due = sync.next_due;
    let changed = sync.change_since;
    sync.record_sync_mutation(now, 5);
    sync.record_sync_mutation(now, 3);
    assert_eq!(sync.mutation_generation, 5);
    assert_eq!(sync.next_due, due);
    assert_eq!(sync.change_since, changed);
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Change));
}

#[test]
fn authentication_failure_pauses_sync_until_settings_change() {
    let now = Instant::now();
    let mut sync = AutoSyncSchedule::new(now, 0);
    sync.running = true;
    sync.complete(
        now,
        AutoSyncTrigger::Startup,
        &AutoSyncCompletion::from_error(&CloudSyncError::Remote("HTTP 401 Unauthorized".into())),
        None,
    );
    assert!(!sync.running());
    assert!(sync.paused);
    assert_eq!(sync.failures, 1);
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Retry));

    sync.settings_changed(now);
    assert!(!sync.paused);
    assert_eq!(sync.failures, 0);
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Settings));
}

#[test]
fn cloud_sync_http_authentication_pauses_and_rate_limits_respect_retry_after() {
    let now = Instant::now();
    let error = CloudSyncError::Http {
        provider: "test".into(),
        operation: "read".into(),
        status: 401,
        retry_after_seconds: None,
    };
    assert!(permanent_auto_sync_error(&error));
    let mut coordinator = AutoSyncSchedule::new(now, 0);
    let before = now;
    coordinator.complete(
        now,
        AutoSyncTrigger::Periodic,
        &AutoSyncCompletion::from_error(&CloudSyncError::Http {
            provider: "test".into(),
            operation: "read".into(),
            status: 429,
            retry_after_seconds: Some(1800),
        }),
        None,
    );
    assert!(!coordinator.paused);
    assert!(coordinator.next_due.duration_since(before) >= Duration::from_secs(1800));
}

#[test]
fn cloud_sync_rate_limit_deadline_survives_local_changes_focus_and_periodic_ticks() {
    let now = Instant::now();
    let mut sync = AutoSyncSchedule::new(now, 0);
    sync.complete(
        now,
        AutoSyncTrigger::Periodic,
        &AutoSyncCompletion::from_error(&CloudSyncError::Http {
            provider: "GitHub Gist".into(),
            operation: "read".into(),
            status: 403,
            retry_after_seconds: Some(1800),
        }),
        None,
    );
    assert!(!sync.paused);
    let deadline = sync.retry_not_before.unwrap();
    sync.record_sync_mutation(now, 1);
    sync.focused(now);
    assert!(
        sync.next_job(deadline - Duration::from_secs(1), 1, || panic!(
            "rate limited"
        ))
        .is_none()
    );
    assert!(sync.next_job(deadline, 1, || true).is_some());
}

#[test]
fn recent_manual_checks_suppress_focus_and_noop_changes_do_not_postpone_periodic_checks() {
    let now = Instant::now();
    let mut sync = AutoSyncSchedule::new(now, 0);
    sync.pending = None;
    sync.observed_remote_check(now);
    sync.focused(now);
    assert!(sync.pending.is_none());
    let periodic = sync.next_periodic;
    sync.record_sync_mutation(now, 1);
    let job = sync.next_job(sync.next_due, 1, || true).unwrap();
    assert_eq!(job.trigger, AutoSyncTrigger::Change);
    sync.complete(now, job.trigger, &AutoSyncCompletion::Skipped, Some(1));
    assert_eq!(sync.next_periodic, periodic);
    assert!(sync.change_since.is_none());
    assert_eq!(
        sync.next_job(periodic, 1, || true).unwrap().trigger,
        AutoSyncTrigger::Periodic
    );
}

#[test]
fn skipped_preflight_keeps_new_mutations_remote_work_and_error_history() {
    let now = Instant::now();
    let mut sync = AutoSyncSchedule::new(now, 0);
    sync.pending = None;
    sync.remote_available = true;
    sync.record_sync_mutation(now, 1);
    let job = sync.next_job(sync.next_due, 1, || true).unwrap();
    assert!(job.remote_available);
    let periodic = sync.next_periodic;
    sync.complete(now, job.trigger, &AutoSyncCompletion::Skipped, Some(2));
    assert!(sync.remote_available);
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Change));
    assert!(sync.change_since.is_some());
    assert_eq!(sync.next_periodic, periodic);
}

#[test]
fn startup_and_retry_requests_survive_local_notifications() {
    let now = Instant::now();
    let mut sync = AutoSyncSchedule::new(now, 0);
    sync.record_sync_mutation(now, 1);
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Startup));
    sync.pending = Some(AutoSyncTrigger::Retry);
    sync.record_sync_mutation(now, 2);
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Retry));
}

#[test]
fn periodic_inspection_during_upload_debounce_advances_only_the_remote_timer() {
    let now = Instant::now();
    let mut sync = AutoSyncSchedule::new(now, 0);
    sync.pending = None;
    sync.record_sync_mutation(now, 1);
    sync.next_periodic = now;
    let job = sync.next_job(now, 1, || true).unwrap();
    assert_eq!(job.trigger, AutoSyncTrigger::Periodic);
    let before = now;
    sync.complete(
        now,
        job.trigger,
        &AutoSyncCompletion::Debouncing(Duration::from_secs(50)),
        Some(1),
    );
    assert!(sync.next_periodic >= before + Duration::from_secs(900));
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Change));
    assert!(sync.next_due >= before + Duration::from_secs(50));
}

#[test]
fn mutation_first_seen_at_dispatch_does_not_look_like_an_absent_debounce() {
    let now = Instant::now();
    let mut sync = AutoSyncSchedule::new(now, 0);
    sync.pending = None;
    sync.next_periodic = now;
    let job = sync.next_job(now, 1, || true).unwrap();
    assert_eq!(job.trigger, AutoSyncTrigger::Periodic);
    assert!(!job.change_elapsed.is_zero());
    assert!(job.change_elapsed < Duration::from_secs(60));
}

#[test]
fn disabled_and_skipped_jobs_do_not_report_or_extend_a_successful_inspection() {
    let now = Instant::now();
    let mut sync = AutoSyncSchedule::new(now, 0);
    let periodic = sync.next_periodic;
    sync.complete(
        now,
        AutoSyncTrigger::Startup,
        &AutoSyncCompletion::Skipped,
        None,
    );
    assert_eq!(sync.next_periodic, periodic);
    assert!(sync.last_remote_check.is_none());
    sync.complete(
        now,
        AutoSyncTrigger::Periodic,
        &AutoSyncCompletion::Disabled,
        None,
    );
    assert_eq!(sync.next_periodic, periodic);
    assert!(sync.next_job(periodic, 0, || panic!("disabled")).is_none());
    sync.settings_changed(now);
    assert!(!sync.paused);
}

#[test]
fn successful_manual_inspection_consumes_a_due_periodic_request_without_losing_changes() {
    let now = Instant::now();
    let mut sync = AutoSyncSchedule::new(now, 0);
    sync.pending = Some(AutoSyncTrigger::Periodic);
    sync.observed_remote_check(now);
    assert!(sync.pending.is_none());
    sync.record_sync_mutation(now, 1);
    sync.pending = Some(AutoSyncTrigger::Periodic);
    sync.observed_remote_check(now + Duration::from_secs(1));
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Change));
    assert!(sync.change_since.is_some());
}

#[test]
fn startup_busy_unverified_and_pause_obey_injected_deadlines() {
    let now = Instant::now();
    let mut schedule = AutoSyncSchedule::new(now, 0);
    assert!(
        schedule
            .next_job(now + Duration::from_secs(2), 0, || panic!("not due"))
            .is_none()
    );
    let started = now + Duration::from_secs(3);
    let startup = schedule.next_job(started, 0, || true).unwrap();
    assert_eq!(startup.trigger, AutoSyncTrigger::Startup);
    schedule.complete(started, startup.trigger, &AutoSyncCompletion::Busy, None);
    assert!(
        schedule
            .next_job(started + Duration::from_secs(4), 0, || panic!("busy delay"))
            .is_none()
    );
    let retry_at = started + Duration::from_secs(5);
    let job = schedule.next_job(retry_at, 0, || true).unwrap();
    schedule.complete(
        retry_at,
        job.trigger,
        &AutoSyncCompletion::V4Unverified,
        None,
    );
    assert!(
        schedule
            .next_job(retry_at + Duration::from_secs(59), 0, || panic!(
                "unverified backoff"
            ))
            .is_none()
    );
    let verified_at = retry_at + Duration::from_secs(60);
    let job = schedule.next_job(verified_at, 0, || true).unwrap();
    schedule.complete(
        verified_at,
        job.trigger,
        &AutoSyncCompletion::Conflict {
            overwrite_blocked: true,
        },
        None,
    );
    assert!(
        schedule
            .next_job(verified_at + Duration::from_secs(900), 0, || panic!(
                "paused"
            ))
            .is_none()
    );
    schedule.settings_changed(verified_at + Duration::from_secs(901));
    assert_eq!(
        schedule
            .next_job(verified_at + Duration::from_secs(901), 0, || true)
            .unwrap()
            .trigger,
        AutoSyncTrigger::Settings
    );
}

#[test]
fn stale_remote_notifications_do_not_rewind_periodic_or_focus_deadlines() {
    let now = Instant::now();
    let mut schedule = AutoSyncSchedule::new(now, 0);
    schedule.pending = None;
    schedule.observed_remote_check(now + Duration::from_secs(10));
    schedule.observed_remote_check(now);
    assert_eq!(schedule.next_periodic, now + Duration::from_secs(910));
    schedule.focused(now + Duration::from_secs(129));
    assert!(schedule.pending.is_none());
    schedule.focused(now + Duration::from_secs(130));
    assert_eq!(schedule.pending, Some(AutoSyncTrigger::Focus));
}
