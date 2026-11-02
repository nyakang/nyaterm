use super::{
    RecentActivationCache, next_recent_after_close, normalize_new_workspace_ui,
    should_enable_startup_screen_lock, workspace_targets_from_order,
};
use nyaterm_core::{ACTIVATION_QUEUE_CAPACITY, WorkspaceId, WorkspaceUiState};

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
fn external_routing_reuses_recent_loading_windows_deduplicates_and_opens_plain_launches() {
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
        assert_eq!(controller.windows.len(), 3);
        controller.closing_workspaces.extend(ids);
        let third = controller.most_recent_workspace_id.unwrap();
        controller.closing_workspaces.insert(third);
        controller.route_activation(request(6, &["ssh://other.example.com"]), cx);
        assert_eq!(controller.windows.len(), 4);
    });
}
