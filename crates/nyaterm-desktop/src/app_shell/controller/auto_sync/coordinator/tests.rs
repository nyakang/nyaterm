use super::{AutoSyncCoordinator, auto_sync_backoff, permanent_auto_sync_error};
use crate::features::{AutoSyncResult, AutoSyncTrigger};
use nyaterm_core::CloudSyncError;
use std::time::Duration;
use std::time::Instant;

#[test]
fn oversized_local_replica_pauses_auto_sync_until_settings_change() {
    let mut coordinator = AutoSyncCoordinator::new(0);
    let error = CloudSyncError::PortableSnapshot(
        nyaterm_core::PortableSnapshotError::LocalReplicaPayloadTooLarge {
            limit_bytes: 256 * 1024 * 1024,
        },
    );
    coordinator.complete(AutoSyncTrigger::Startup, &Err(error), None);
    assert!(
        coordinator
            .next_job(Instant::now() + Duration::from_secs(16 * 60), 0, || true)
            .is_none()
    );
    coordinator.settings_changed();
    assert!(coordinator.next_job(Instant::now(), 0, || true).is_some());
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
    let mut sync = AutoSyncCoordinator::new(4);
    let job = sync
        .next_job(sync.next_due, 4, || true)
        .expect("startup sync");
    sync.complete(
        job.trigger,
        &Ok(AutoSyncResult::UpToDate(Box::default())),
        Some(5),
    );
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Change));
    assert!(sync.change_since.is_some());
    let due = sync.next_due;
    sync.record_sync_mutation(5);
    assert_eq!(sync.next_due, due, "late notification keeps the debounce");
}

#[test]
fn dispatch_captures_generation_and_keeps_changes_made_during_the_job() {
    let mut sync = AutoSyncCoordinator::new(4);
    sync.pending = None;
    sync.record_sync_mutation(5);
    let now = sync.next_due;
    let job = sync.next_job(now, 5, || true).expect("change is due");
    assert_eq!(job.trigger, AutoSyncTrigger::Change);
    assert_eq!(job.change_elapsed, Duration::from_secs(1));
    assert_eq!(sync.in_flight_generation, 5);
    assert!(sync.next_job(now, 5, || true).is_none());

    sync.record_sync_mutation(6);
    sync.complete(
        job.trigger,
        &Ok(AutoSyncResult::UpToDate(Box::default())),
        None,
    );
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
    let mut sync = AutoSyncCoordinator::new(0);
    sync.pending = None;
    sync.remote_available = true;
    let now = Instant::now();
    assert!(sync.next_job(now, 0, || false).is_none());
    assert_eq!(sync.pending, None);
    let job = sync.next_job(now, 0, || true).expect("safe pull");
    assert_eq!(job.trigger, AutoSyncTrigger::DeferredPull);

    sync.running = false;
    sync.record_sync_mutation(1);
    let job = sync
        .next_job(sync.next_due, 1, || true)
        .expect("local change takes precedence");
    assert_eq!(job.trigger, AutoSyncTrigger::Change);
}

#[test]
fn periodic_checks_cannot_be_starved_by_continuous_changes_and_still_respect_pause() {
    let mut sync = AutoSyncCoordinator::new(0);
    sync.pending = None;
    sync.record_sync_mutation(1);
    let now = Instant::now();
    sync.next_periodic = now;
    sync.paused = true;
    assert!(
        sync.next_job(now, 1, || panic!("paused tick reads no windows"))
            .is_none()
    );
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Periodic));
    sync.paused = false;
    sync.record_sync_mutation(2);
    assert_eq!(
        sync.next_job(now, 2, || true).unwrap().trigger,
        AutoSyncTrigger::Periodic
    );
    sync.running = false;
    sync.settings_changed();
    sync.record_sync_mutation(3);
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Settings));
}

#[test]
fn auto_sync_focus_is_throttled_and_settings_resume_paused_checks() {
    let mut sync = AutoSyncCoordinator::new(0);
    sync.pending = None;
    sync.focused();
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Focus));
    sync.pending = None;
    sync.focused();
    assert_eq!(sync.pending, None);
    sync.paused = true;
    sync.failures = 3;
    sync.settings_changed();
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
    let mut sync = AutoSyncCoordinator::new(4);
    sync.running = true;
    sync.pending = None;
    sync.record_sync_mutation(5);
    let due = sync.next_due;
    let changed = sync.change_since;

    sync.complete(
        AutoSyncTrigger::Change,
        &Ok(AutoSyncResult::UpToDate(Box::default())),
        None,
    );

    assert!(!sync.running());
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Change));
    assert_eq!(sync.change_since, changed);
    assert_eq!(sync.next_due, due);

    sync.in_flight_generation = 5;
    sync.running = true;
    sync.complete(
        AutoSyncTrigger::Change,
        &Ok(AutoSyncResult::UpToDate(Box::default())),
        None,
    );
    assert_eq!(sync.pending, None);
    assert_eq!(sync.change_since, None);
}

#[test]
fn duplicate_or_stale_mutations_do_not_extend_the_debounce() {
    let mut sync = AutoSyncCoordinator::new(4);
    sync.pending = None;
    sync.record_sync_mutation(5);
    let due = sync.next_due;
    let changed = sync.change_since;
    sync.record_sync_mutation(5);
    sync.record_sync_mutation(3);
    assert_eq!(sync.mutation_generation, 5);
    assert_eq!(sync.next_due, due);
    assert_eq!(sync.change_since, changed);
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Change));
}

#[test]
fn authentication_failure_pauses_sync_until_settings_change() {
    let mut sync = AutoSyncCoordinator::new(0);
    sync.running = true;
    sync.complete(
        AutoSyncTrigger::Startup,
        &Err(CloudSyncError::Remote("HTTP 401 Unauthorized".into())),
        None,
    );
    assert!(!sync.running());
    assert!(sync.paused);
    assert_eq!(sync.failures, 1);
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Retry));

    sync.settings_changed();
    assert!(!sync.paused);
    assert_eq!(sync.failures, 0);
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Settings));
}

#[test]
fn cloud_sync_http_authentication_pauses_and_rate_limits_respect_retry_after() {
    let error = CloudSyncError::Http {
        provider: "test".into(),
        operation: "read".into(),
        status: 401,
        retry_after_seconds: None,
    };
    assert!(permanent_auto_sync_error(&error));
    let mut coordinator = AutoSyncCoordinator::new(0);
    let before = Instant::now();
    coordinator.complete(
        AutoSyncTrigger::Periodic,
        &Err(CloudSyncError::Http {
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
fn cloud_sync_automatic_history_deduplicates_failures_and_records_recovery_once() {
    let mut sync = AutoSyncCoordinator::new(0);
    let error = || {
        Err(CloudSyncError::Http {
            provider: "test".into(),
            operation: "read".into(),
            status: 503,
            retry_after_seconds: None,
        })
    };
    sync.started_at = Some(Instant::now());
    let entry = sync
        .history_entry(AutoSyncTrigger::Retry, &error())
        .unwrap();
    assert_eq!(entry.message, "server");
    assert!(entry.duration_ms.is_some());
    assert!(
        sync.history_entry(AutoSyncTrigger::Retry, &error())
            .is_none()
    );
    let success = Ok(AutoSyncResult::UpToDate(Box::default()));
    assert_eq!(
        sync.history_entry(AutoSyncTrigger::Retry, &success)
            .unwrap()
            .status,
        "success"
    );
    assert!(
        sync.history_entry(AutoSyncTrigger::Periodic, &success)
            .is_none()
    );
    assert!(
        sync.history_entry(AutoSyncTrigger::Change, &Ok(AutoSyncResult::Busy))
            .is_none()
    );
    assert!(
        sync.history_entry(AutoSyncTrigger::Startup, &Ok(AutoSyncResult::Disabled))
            .is_none()
    );
}

#[test]
fn cloud_sync_rate_limit_deadline_survives_local_changes_focus_and_periodic_ticks() {
    let mut sync = AutoSyncCoordinator::new(0);
    sync.complete(
        AutoSyncTrigger::Periodic,
        &Err(CloudSyncError::Http {
            provider: "GitHub Gist".into(),
            operation: "read".into(),
            status: 403,
            retry_after_seconds: Some(1800),
        }),
        None,
    );
    assert!(!sync.paused);
    let deadline = sync.retry_not_before.unwrap();
    sync.record_sync_mutation(1);
    sync.focused();
    assert!(
        sync.next_job(deadline - Duration::from_secs(1), 1, || panic!(
            "rate limited"
        ))
        .is_none()
    );
    assert!(sync.next_job(deadline, 1, || true).is_some());
}

#[test]
fn cloud_sync_successful_check_records_recovery_even_when_pull_is_deferred() {
    let mut sync = AutoSyncCoordinator::new(0);
    sync.had_error = true;
    let result = Ok(AutoSyncResult::RemoteAvailable {
        retry_when_safe: true,
        revision: Some("remote-revision".into()),
    });
    let entry = sync.history_entry(AutoSyncTrigger::Retry, &result).unwrap();
    assert_eq!(entry.status, "success");
    assert_eq!(entry.revision.as_deref(), Some("remote-revision"));
    assert!(
        sync.history_entry(AutoSyncTrigger::Retry, &result)
            .is_none()
    );
}

#[test]
fn recent_manual_checks_suppress_focus_and_noop_changes_do_not_postpone_periodic_checks() {
    let mut sync = AutoSyncCoordinator::new(0);
    sync.pending = None;
    let now = Instant::now();
    sync.observed_remote_check(now);
    sync.focused();
    assert!(sync.pending.is_none());
    let periodic = sync.next_periodic;
    sync.record_sync_mutation(1);
    let job = sync.next_job(sync.next_due, 1, || true).unwrap();
    assert_eq!(job.trigger, AutoSyncTrigger::Change);
    sync.complete(job.trigger, &Ok(AutoSyncResult::Skipped), Some(1));
    assert_eq!(sync.next_periodic, periodic);
    assert!(sync.change_since.is_none());
    assert_eq!(
        sync.next_job(periodic, 1, || true).unwrap().trigger,
        AutoSyncTrigger::Periodic
    );
}

#[test]
fn skipped_preflight_keeps_new_mutations_remote_work_and_error_history() {
    let mut sync = AutoSyncCoordinator::new(0);
    sync.pending = None;
    sync.remote_available = true;
    sync.had_error = true;
    sync.record_sync_mutation(1);
    let job = sync.next_job(sync.next_due, 1, || true).unwrap();
    assert!(job.remote_available);
    let periodic = sync.next_periodic;
    assert!(
        sync.history_entry(job.trigger, &Ok(AutoSyncResult::Skipped))
            .is_none()
    );
    sync.complete(job.trigger, &Ok(AutoSyncResult::Skipped), Some(2));
    assert!(sync.had_error);
    assert!(sync.remote_available);
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Change));
    assert!(sync.change_since.is_some());
    assert_eq!(sync.next_periodic, periodic);
}

#[test]
fn startup_and_retry_requests_survive_local_notifications() {
    let mut sync = AutoSyncCoordinator::new(0);
    sync.record_sync_mutation(1);
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Startup));
    sync.pending = Some(AutoSyncTrigger::Retry);
    sync.record_sync_mutation(2);
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Retry));
}

#[test]
fn periodic_inspection_during_upload_debounce_advances_only_the_remote_timer() {
    let mut sync = AutoSyncCoordinator::new(0);
    sync.pending = None;
    sync.record_sync_mutation(1);
    sync.next_periodic = Instant::now();
    let job = sync.next_job(Instant::now(), 1, || true).unwrap();
    assert_eq!(job.trigger, AutoSyncTrigger::Periodic);
    let before = Instant::now();
    sync.complete(
        job.trigger,
        &Ok(AutoSyncResult::Debouncing(Duration::from_secs(50))),
        Some(1),
    );
    assert!(sync.next_periodic >= before + Duration::from_secs(900));
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Change));
    assert!(sync.next_due >= before + Duration::from_secs(50));
}

#[test]
fn mutation_first_seen_at_dispatch_does_not_look_like_an_absent_debounce() {
    let mut sync = AutoSyncCoordinator::new(0);
    sync.pending = None;
    let now = Instant::now();
    sync.next_periodic = now;
    let job = sync.next_job(now, 1, || true).unwrap();
    assert_eq!(job.trigger, AutoSyncTrigger::Periodic);
    assert!(!job.change_elapsed.is_zero());
    assert!(job.change_elapsed < Duration::from_secs(60));
}

#[test]
fn disabled_and_skipped_jobs_do_not_report_or_extend_a_successful_inspection() {
    let mut sync = AutoSyncCoordinator::new(0);
    let periodic = sync.next_periodic;
    sync.complete(AutoSyncTrigger::Startup, &Ok(AutoSyncResult::Skipped), None);
    assert_eq!(sync.next_periodic, periodic);
    assert!(sync.last_remote_check.is_none());
    sync.complete(
        AutoSyncTrigger::Periodic,
        &Ok(AutoSyncResult::Disabled),
        None,
    );
    assert_eq!(sync.next_periodic, periodic);
    assert!(sync.next_job(periodic, 0, || panic!("disabled")).is_none());
    sync.settings_changed();
    assert!(!sync.paused);
}

#[test]
fn successful_manual_inspection_consumes_a_due_periodic_request_without_losing_changes() {
    let mut sync = AutoSyncCoordinator::new(0);
    sync.pending = Some(AutoSyncTrigger::Periodic);
    sync.observed_remote_check(Instant::now());
    assert!(sync.pending.is_none());
    sync.record_sync_mutation(1);
    sync.pending = Some(AutoSyncTrigger::Periodic);
    sync.observed_remote_check(Instant::now());
    assert_eq!(sync.pending, Some(AutoSyncTrigger::Change));
    assert!(sync.change_since.is_some());
}

#[test]
fn shared_settings_broadcast_does_not_bypass_change_debounce_or_local_preflight() {
    use crate::app_shell::AppShellStartup;
    use crate::app_shell::controller::DesktopController;
    use crate::features::{AutoSyncResult, AutoSyncTrigger};
    use crate::test_support::TestAppCleanup;
    use gpui::{AppContext as _, TestAppContext};
    use nyaterm_core::{AppRuntime, RuntimeMode, WorkspaceId, test_support::TestTempDir};
    use std::time::{Duration, Instant};

    let root = TestTempDir::new("nyaterm-settings-sync-broadcast");
    let runtime = AppRuntime::from_parts_for_test(
        RuntimeMode::Portable,
        root.path().into(),
        root.join("config"),
        root.join("logs"),
        root.join("cache"),
        None,
    );
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let startup = AppShellStartup::prepare(&runtime);
    let controller = cx.new(|cx| DesktopController::new(runtime, startup, cx));
    controller.update(&mut cx, |controller, cx| {
        let now = Instant::now();
        let initial = controller
            .auto_sync
            .next_job(now + Duration::from_secs(3), 0, || true)
            .unwrap();
        controller
            .auto_sync
            .complete(initial.trigger, &Ok(AutoSyncResult::Skipped), Some(0));
        controller.publish_settings(WorkspaceId::new(), Default::default(), cx);
        assert!(
            controller
                .auto_sync
                .next_job(now + Duration::from_secs(5), 0, || true)
                .is_none()
        );
        controller.cloud_sync_settings_changed();
        assert_eq!(
            controller
                .auto_sync
                .next_job(now + Duration::from_secs(5), 0, || true)
                .unwrap()
                .trigger,
            AutoSyncTrigger::Settings
        );
    });
}
