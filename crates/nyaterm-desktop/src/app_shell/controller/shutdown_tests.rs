use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{AppContext as _, TestAppContext};
use nyaterm_core::cloud_sync::control::CloudSyncOperationControl;
use nyaterm_core::{AppRuntime, RuntimeMode};
use nyaterm_store::{FlushBarrier, StoreDomain, StoreSubmitError};

use super::DesktopController;
use crate::app_shell::AppShellStartup;
use crate::blocking_jobs::JobRejected;
use crate::test_support::{TestAppCleanup, TestConfigDir};

#[test]
fn quit_without_ready_keeps_foreground_and_store_alive_until_sync_result_is_applied() {
    let dir = TestConfigDir::new("nyaterm-sync-shutdown-order");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let runtime = AppRuntime::from_parts_for_test(
        RuntimeMode::Portable,
        dir.path().into(),
        dir.join("config"),
        dir.join("logs"),
        dir.join("cache"),
        None,
    );
    let startup = AppShellStartup::prepare(&runtime);
    let store = startup.shared_store_runtime().unwrap();
    let controller = cx.new(|cx| DesktopController::new(runtime, startup, cx));
    let operations = cx.read(|cx| controller.read(cx).sync_operations());
    let scheduler = cx.read(|cx| controller.read(cx).update_store.read(cx).blocking_jobs());
    let control = CloudSyncOperationControl::default();
    control.begin_commit().unwrap();
    let registration = operations
        .register("committing-sync", vec![control.clone()], None)
        .unwrap();

    controller.update(&mut cx, |controller, cx| {
        controller.request_quit_without_ready(cx);
        controller.request_quit_without_ready(cx);
    });
    let foreground_ran = Rc::new(Cell::new(false));
    let observed = foreground_ran.clone();
    cx.spawn(async move |_| observed.set(true)).detach();
    cx.run_until_parked();
    assert!(foreground_ran.get());
    assert!(!operations.accepting());
    // The final snapshot/flush must not race a committing operation's store writes.
    store
        .blocking_client()
        .request_fn(StoreDomain::CloudSync, |store| {
            let mut state = store.load_cloud_sync_state()?;
            state.last_checked_at_ms = Some(42);
            store.save_cloud_sync_state(&state)
        })
        .unwrap();
    assert!(store.ui_client().try_submit(0, FlushBarrier).is_ok());
    assert!(scheduler.submit_detached("still-alive", |_| {}).is_ok());

    let history = operations.followup("history");
    drop(registration);
    cx.run_until_parked();
    assert!(store.ui_client().try_submit(0, FlushBarrier).is_ok());
    drop(history);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        if matches!(
            scheduler.submit_detached("probe", |_| {}),
            Err(JobRejected::ShuttingDown)
        ) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "process scheduler was not stopped"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(matches!(
        store.ui_client().try_submit(0, FlushBarrier),
        Err(StoreSubmitError::ShuttingDown)
    ));
    cx.run_until_parked();
}

#[test]
fn cancelled_exit_reopens_process_admission_without_closing_the_scheduler() {
    let dir = TestConfigDir::new("nyaterm-sync-shutdown-resume");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let runtime = AppRuntime::from_parts_for_test(
        RuntimeMode::Portable,
        dir.path().into(),
        dir.join("config"),
        dir.join("logs"),
        dir.join("cache"),
        None,
    );
    let startup = AppShellStartup::prepare(&runtime);
    let controller = cx.new(|cx| DesktopController::new(runtime, startup, cx));
    controller.update(&mut cx, |controller, cx| {
        assert!(controller.begin_process_quit());
        assert!(!controller.begin_process_quit());
        controller.cancel_process_quit();
        assert!(controller.sync_operations().accepting());
        assert!(
            controller
                .update_store
                .read(cx)
                .blocking_jobs()
                .submit_detached("retry", |_| {})
                .is_ok()
        );
        assert!(controller.begin_process_quit());
        controller.cancel_process_quit();
    });
}
