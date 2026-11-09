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
fn workspace_close_updates_restore_lists_only_after_successful_persistence() {
    use super::WorkspaceWindow;
    use crate::app_shell::{AppShell, AppShellLifecycle};
    use gpui::{EmptyView, px, size};
    use nyaterm_core::{
        DeviceWindowManifest, DeviceWindowState, MainWindowBounds, MainWindowState, WorkspaceId,
        WorkspaceRestoreManifest, WorkspaceRestoreState,
    };
    use nyaterm_store::{
        ConnectionStore, LoadDeviceWindowManifest, LoadWorkspaceRestoreManifest, StorageError,
        store_request,
    };

    for fail_save in [false, true] {
        let root = TestConfigDir::new("nyaterm-workspace-close-restore");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let ids = [WorkspaceId::new(), WorkspaceId::new()];
        let mut manifest = WorkspaceRestoreManifest::single(WorkspaceRestoreState::empty(ids[0]));
        manifest
            .workspaces
            .push(WorkspaceRestoreState::empty(ids[1]));
        manifest.most_recent_workspace_id = Some(ids[1]);
        manifest
            .extra
            .insert("future_manifest".into(), serde_json::json!(true));
        manifest.workspaces[0]
            .extra
            .insert("future_workspace".into(), serde_json::json!([1, 2]));
        let mut device = DeviceWindowManifest::empty();
        device.window_order = ids.to_vec();
        device.most_recent_workspace_id = Some(ids[1]);
        device
            .extra
            .insert("future_device".into(), serde_json::json!(true));
        device.windows = ids
            .iter()
            .map(|id| DeviceWindowState {
                workspace_id: *id,
                window: MainWindowState::new(
                    None,
                    MainWindowBounds {
                        x: 0,
                        y: 0,
                        width: 900,
                        height: 700,
                    },
                    false,
                ),
                extra: Default::default(),
            })
            .collect();
        {
            let store = ConnectionStore::open(root.join("config")).unwrap();
            store
                .save_restore_manifests_atomically(&manifest, &device)
                .unwrap();
        }
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.path().into(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let startup = AppShellStartup::prepare(&runtime);
        let store = startup.shared_store_runtime().unwrap();
        let controller = cx.new(|cx| DesktopController::new(runtime.clone(), startup, cx));
        let session_hub = cx.read(|cx| controller.read(cx).session_hub.clone());
        // Keep shells alive without rendering them: this exercises the real
        // persistence lifecycle without making worker completion depend on font layout.
        let mut shells = Vec::new();
        for id in ids {
            let startup = cx.read(|cx| controller.read(cx).startup.for_workspace(id));
            let shell = cx.new(|cx| {
                AppShell::new(
                    runtime.clone(),
                    None,
                    startup,
                    id,
                    controller.clone(),
                    session_hub.clone(),
                    cx,
                )
            });
            let handle = cx.open_window(size(px(900.), px(700.)), |_, _| EmptyView);
            controller.update(&mut cx, |controller, _| {
                controller.windows.insert(
                    id,
                    WorkspaceWindow {
                        handle: handle.into(),
                        shell: shell.downgrade(),
                    },
                );
            });
            shells.push(shell);
        }
        let closing_shell = controller.update(&mut cx, |controller, cx| {
            let closing_shell = controller.windows[&ids[1]].shell.upgrade().unwrap();
            let task = store
                .ui_client()
                .try_submit(
                    0,
                    store_request(StoreDomain::Sessions, move |_| {
                        if fail_save {
                            Err(StorageError::InvalidData(
                                "simulated workspace save failure".into(),
                            ))
                        } else {
                            Ok(())
                        }
                    }),
                )
                .unwrap();
            controller
                .request_close_workspace(ids[1], task, cx)
                .unwrap();
            closing_shell
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            if cx.read(|cx| controller.read(cx).closing_workspaces.is_empty()) {
                break;
            }
            assert!(Instant::now() < deadline, "workspace close did not finish");
            std::thread::sleep(Duration::from_millis(1));
        }
        let loaded = store
            .blocking_client()
            .request(0, LoadWorkspaceRestoreManifest)
            .unwrap()
            .outcome
            .unwrap();
        let windows = store
            .blocking_client()
            .request(0, LoadDeviceWindowManifest(ids[0]))
            .unwrap()
            .outcome
            .unwrap();
        if fail_save {
            assert_eq!(loaded, manifest);
            assert_eq!(windows, device);
            assert_eq!(cx.read(|cx| controller.read(cx).workspace_count()), 2);
            assert!(cx.read(|cx| matches!(
                closing_shell.read(cx).lifecycle,
                AppShellLifecycle::FlushFailed(_)
            )));
        } else {
            manifest
                .workspaces
                .retain(|workspace| workspace.id != ids[1]);
            manifest.most_recent_workspace_id = Some(ids[0]);
            device.windows.retain(|entry| entry.workspace_id != ids[1]);
            device.window_order.retain(|id| *id != ids[1]);
            device.most_recent_workspace_id = Some(ids[0]);
            assert_eq!(loaded, manifest);
            assert_eq!(windows, device);
            assert_eq!(cx.read(|cx| controller.read(cx).workspace_count()), 1);
            // The same startup reader used by a cold launch sees only the retained workspace.
            let restored = store
                .blocking_client()
                .request(0, nyaterm_store::LoadWorkspaceStartupRestore)
                .unwrap()
                .outcome
                .unwrap();
            assert_eq!(
                restored
                    .manifest
                    .workspaces
                    .iter()
                    .map(|workspace| workspace.id)
                    .collect::<Vec<_>>(),
                vec![ids[0]]
            );
        }
    }
}

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
