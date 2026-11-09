use super::{
    RecentActivationCache, next_recent_after_close, normalize_new_workspace_ui,
    should_enable_startup_screen_lock, workspace_targets_from_order,
};
use nyaterm_core::{ACTIVATION_QUEUE_CAPACITY, WorkspaceId, WorkspaceUiState};

#[test]
fn cold_start_restores_workspaces_and_adds_only_an_explicit_new_window() {
    use super::DesktopController;
    use crate::app_shell::AppShellStartup;
    use crate::test_support::TestAppCleanup;
    use gpui::{AppContext as _, TestAppContext};
    use nyaterm_core::{
        ActivationRequest, AppRuntime, RuntimeMode, WorkspaceRestoreManifest,
        WorkspaceRestoreState, test_support::TestTempDir,
    };
    use nyaterm_store::ConnectionStore;

    for (saved_count, args, expected_count) in [
        (0, vec![], 1),
        (0, vec!["--new-window", "ssh://host"], 1),
        (2, vec![], 2),
        (2, vec!["--reuse-window"], 2),
        (2, vec!["--new-window", "ssh://host"], 3),
    ] {
        let root = TestTempDir::new("nyaterm-cold-window-routing");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let saved_ids = (0..saved_count)
            .map(|_| WorkspaceId::new())
            .collect::<Vec<_>>();
        if !saved_ids.is_empty() {
            let store = ConnectionStore::open(root.join("config")).unwrap();
            store
                .save_workspace_restore_manifest(&WorkspaceRestoreManifest {
                    version: nyaterm_core::WORKSPACE_RESTORE_MANIFEST_VERSION,
                    workspaces: saved_ids
                        .iter()
                        .copied()
                        .map(WorkspaceRestoreState::empty)
                        .collect(),
                    most_recent_workspace_id: saved_ids.last().copied(),
                    extra: Default::default(),
                })
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
        assert!(startup.recovery.is_none());
        let controller = cx.new(|cx| DesktopController::new(runtime, startup, cx));
        let request =
            ActivationRequest::from_os_args([1; 16], args.iter().map(std::ffi::OsString::from));
        controller.update(&mut cx, |controller, cx| {
            controller
                .open_startup_workspaces(request.parse(), cx)
                .unwrap();
            assert_eq!(controller.workspace_count(), expected_count);
            assert!(
                saved_ids
                    .iter()
                    .all(|id| controller.windows.contains_key(id))
            );
            let recent = controller.most_recent_workspace_id.unwrap();
            if saved_count > 0 && !args.contains(&"--new-window") {
                assert_eq!(Some(recent), saved_ids.last().copied());
            } else if saved_count > 0 {
                assert!(!saved_ids.contains(&recent));
            }
            for (id, entry) in &controller.windows {
                let shell = entry.shell.upgrade().unwrap();
                assert_eq!(
                    shell.read(cx).pending_activations.len(),
                    usize::from(*id == recent)
                );
            }
        });
    }
}

#[test]
fn cold_start_conflicting_options_open_no_windows() {
    use super::DesktopController;
    use crate::app_shell::AppShellStartup;
    use crate::test_support::TestAppCleanup;
    use gpui::{AppContext as _, TestAppContext};
    use nyaterm_core::{ActivationRequest, AppRuntime, RuntimeMode, test_support::TestTempDir};

    let root = TestTempDir::new("nyaterm-cold-window-conflict");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let runtime = AppRuntime::from_parts_for_test(
        RuntimeMode::Portable,
        root.path().into(),
        root.join("config"),
        root.join("logs"),
        root.join("cache"),
        None,
    );
    let startup = AppShellStartup::prepare(&runtime);
    let controller = cx.new(|cx| DesktopController::new(runtime, startup, cx));
    let request = ActivationRequest::from_os_args(
        [1; 16],
        ["--new-window", "--reuse-window", "ssh://host"].map(std::ffi::OsString::from),
    );
    controller.update(&mut cx, |controller, cx| {
        assert!(
            controller
                .open_startup_workspaces(request.parse(), cx)
                .is_err()
        );
        assert_eq!(controller.workspace_count(), 0);
    });
}

#[test]
fn activation_ids_are_deduplicated_until_fifo_eviction() {
    let mut cache = RecentActivationCache::default();
    let id = |index: u64| {
        let mut id = [0_u8; 16];
        id[..8].copy_from_slice(&index.to_le_bytes());
        id
    };
    assert!(cache.remember(id(0)));
    assert!(!cache.remember(id(0)));
    for index in 1..=(ACTIVATION_QUEUE_CAPACITY * 4) as u64 {
        assert!(cache.remember(id(index)));
    }
    assert!(!cache.remember(id(1)));
    assert!(cache.remember(id(0)), "the oldest ID was evicted");
}

#[test]
fn closing_recent_workspace_uses_reverse_window_order_not_hash_iteration() {
    let first = WorkspaceId::new();
    let second = WorkspaceId::new();
    let third = WorkspaceId::new();
    let order = [first, second, third];
    assert_eq!(
        next_recent_after_close(Some(third), third, &order, |id| id != third),
        Some(second)
    );
    assert_eq!(
        next_recent_after_close(Some(third), third, &order, |id| id == first),
        Some(first)
    );
    assert_eq!(
        next_recent_after_close(Some(third), third, &order, |_| false),
        None
    );
}

#[test]
fn workspace_target_ordinals_do_not_change_when_source_is_filtered_out() {
    let first = WorkspaceId::new();
    let source = WorkspaceId::new();
    let third = WorkspaceId::new();

    let targets = workspace_targets_from_order(source, &[first, source, third], |_| true);

    assert_eq!(targets.len(), 2);
    assert_eq!((targets[0].workspace_id, targets[0].ordinal), (first, 1));
    assert_eq!((targets[1].workspace_id, targets[1].ordinal), (third, 3));
}

#[test]
fn new_workspace_normalizes_single_owner_settings_page() {
    let ui = WorkspaceUiState {
        current_page: "settings".to_string(),
        ..WorkspaceUiState::default()
    };

    assert_eq!(normalize_new_workspace_ui(ui).current_page, "workspace");
}

#[test]
fn initial_bootstrap_enables_startup_screen_lock_from_persisted_setting() {
    assert!(should_enable_startup_screen_lock(false, true));
    assert!(!should_enable_startup_screen_lock(false, false));
}

#[test]
fn shared_state_refresh_does_not_retrigger_startup_screen_lock() {
    assert!(!should_enable_startup_screen_lock(true, true));
}

#[test]
fn external_routing_reuses_plain_launches_and_opens_only_explicit_new_windows() {
    use super::{DesktopController, WorkspaceWindow};
    use crate::app_shell::{AppShell, AppShellStartup};
    use crate::test_support::TestAppCleanup;
    use gpui::{AppContext as _, TestAppContext, px, size};
    use nyaterm_core::{ActivationRequest, AppRuntime, RuntimeMode, test_support::TestTempDir};

    let root = TestTempDir::new("nyaterm-external-window-routing");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let runtime = AppRuntime::from_parts_for_test(
        RuntimeMode::Portable,
        root.path().to_path_buf(),
        root.join("config"),
        root.join("logs"),
        root.join("cache"),
        None,
    );
    let startup = AppShellStartup::prepare(&runtime);
    let controller = cx.new(|cx| DesktopController::new(runtime.clone(), startup, cx));
    let session_hub = cx.read(|cx| controller.read(cx).session_hub.clone());
    let ids = [WorkspaceId::new(), WorkspaceId::new()];
    let mut shells = Vec::new();
    for id in ids {
        let startup = cx.read(|cx| controller.read(cx).startup.for_workspace(id));
        let handle = cx.open_window(size(px(800.), px(600.)), |_, cx| {
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
        let shell = handle.root(&mut cx).unwrap();
        controller.update(&mut cx, |controller, _| {
            controller.windows.insert(
                id,
                WorkspaceWindow {
                    handle: handle.into(),
                    shell: shell.downgrade(),
                },
            );
            controller.device_windows.window_order.push(id);
        });
        shells.push(shell);
    }
    let request = |id, args: &[&str]| {
        ActivationRequest::from_os_args([id; 16], args.iter().map(std::ffi::OsString::from))
    };
    controller.update(&mut cx, |controller, cx| {
        controller.most_recent_workspace_id = Some(ids[0]);
        controller.route_activation(request(1, &["ssh://example.com"]), cx);
        assert_eq!(controller.windows.len(), 2);
        assert_eq!(shells[0].read(cx).pending_activations.len(), 1);
        assert!(shells[1].read(cx).pending_activations.is_empty());
        controller.route_activation(request(1, &["ssh://example.com"]), cx);
        controller.route_activation(request(2, &["SSH://EXAMPLE.COM"]), cx);
        assert_eq!(shells[0].read(cx).pending_activations.len(), 1);

        controller.most_recent_workspace_id = Some(ids[1]);
        controller.route_activation(request(3, &["--reuse-window"]), cx);
        assert_eq!(controller.windows.len(), 2);
        assert_eq!(shells[1].read(cx).pending_activations.len(), 1);
        controller.closing_workspaces.insert(ids[1]);
        controller.route_activation(request(4, &["telnet://example.com"]), cx);
        assert_eq!(shells[0].read(cx).pending_activations.len(), 2);
        assert_eq!(shells[1].read(cx).pending_activations.len(), 1);

        controller.route_activation(request(5, &[]), cx);
        assert_eq!(controller.windows.len(), 2);
        assert_eq!(shells[0].read(cx).pending_activations.len(), 3);
        controller.route_activation(request(6, &["--new-window", "ssh://new.example.com"]), cx);
        assert_eq!(controller.windows.len(), 3);
        let third = controller.most_recent_workspace_id.unwrap();
        let new_shell = controller.windows[&third].shell.upgrade().unwrap();
        assert_eq!(new_shell.read(cx).pending_activations.len(), 1);
        // Connection deduplication must not swallow an explicit window operation.
        controller.route_activation(request(7, &["--new-window", "ssh://new.example.com"]), cx);
        assert_eq!(controller.windows.len(), 4);
        let fourth = controller.most_recent_workspace_id.unwrap();
        let new_shell = controller.windows[&fourth].shell.upgrade().unwrap();
        assert!(new_shell.read(cx).pending_activations[0].actions.is_empty());
        controller.route_activation(
            request(
                8,
                &[
                    "--new-window",
                    "--reuse-window",
                    "ssh://rejected.example.com",
                ],
            ),
            cx,
        );
        assert_eq!(controller.windows.len(), 4);
        assert!(
            new_shell
                .read(cx)
                .pending_activations
                .back()
                .unwrap()
                .has_conflicting_window_options()
        );
        controller.closing_workspaces.extend(ids);
        controller.closing_workspaces.insert(third);
        controller.closing_workspaces.insert(fourth);
        controller.route_activation(request(9, &["--reuse-window", "--new-window"]), cx);
        assert_eq!(controller.windows.len(), 4);
        controller.route_activation(request(10, &[]), cx);
        assert_eq!(controller.windows.len(), 5);
    });
}
