use super::AutoSyncCoordinator;
use crate::features::{AutoSyncResult, AutoSyncTrigger};
use nyaterm_core::CloudSyncError;
use std::time::Instant;

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

#[test]
fn skipped_preflight_does_not_clear_error_history() {
    let mut sync = AutoSyncCoordinator::new(0);
    sync.had_error = true;
    assert!(
        sync.history_entry(AutoSyncTrigger::Change, &Ok(AutoSyncResult::Skipped))
            .is_none()
    );
    assert!(sync.had_error);
}
