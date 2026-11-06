use gpui::{AppContext as _, TestAppContext, px, size};
use nyaterm_core::{AppRuntime, RuntimeMode, WorkspaceId, test_support::TestTempDir};
use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};

use super::{DesktopController, WorkspaceWindow, tray_click_shows_window};
use crate::app_shell::{AppShell, AppShellStartup};
use crate::features::shell::tray::model::TrayAction;
use crate::features::test_support::app_with_visible_ssh_session;

#[test]
fn tray_click_policy_accepts_only_left_release_and_left_double_click() {
    let click = |button, button_state| TrayIconEvent::Click {
        id: "tray".into(),
        position: (0., 0.).into(),
        rect: Default::default(),
        button,
        button_state,
    };
    assert!(tray_click_shows_window(&click(
        MouseButton::Left,
        MouseButtonState::Up
    )));
    assert!(!tray_click_shows_window(&click(
        MouseButton::Left,
        MouseButtonState::Down
    )));
    assert!(!tray_click_shows_window(&click(
        MouseButton::Right,
        MouseButtonState::Up
    )));
    assert!(tray_click_shows_window(&TrayIconEvent::DoubleClick {
        id: "tray".into(),
        position: (0., 0.).into(),
        rect: Default::default(),
        button: MouseButton::Left,
    }));
    assert!(!tray_click_shows_window(&TrayIconEvent::DoubleClick {
        id: "tray".into(),
        position: (0., 0.).into(),
        rect: Default::default(),
        button: MouseButton::Right,
    }));
}

#[test]
fn tray_routes_live_ownership_after_move_and_ignores_closed_sessions() {
    // Drop GPUI state before the guards remove directories that its store workers use.
    let root = TestTempDir::new("nyaterm-tray-routing");
    let first_root = TestTempDir::new("nyaterm-tray-first");
    let second_root = TestTempDir::new("nyaterm-tray-second");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
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
    let first_id = WorkspaceId::new();
    let second_id = WorkspaceId::new();
    let mut shells = Vec::new();
    for id in [first_id, second_id] {
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
        cx.update_entity(&controller, |controller, _| {
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
    // These sessions stand for "another workspace is holding a backend", which is
    // what the cloud-sync blockers below protect; a local shell no longer counts.
    let first_app = app_with_visible_ssh_session(&mut cx, &first_root, "first-session");
    let second_app = app_with_visible_ssh_session(&mut cx, &second_root, "second-session");
    cx.update_entity(&shells[0], |shell, _| shell.app = Some(first_app.clone()));
    cx.update_entity(&shells[1], |shell, _| shell.app = Some(second_app.clone()));
    cx.update_entity(&controller, |controller, cx| {
        controller.most_recent_workspace_id = Some(second_id);
        assert_eq!(
            controller.tray_action_workspace(&TrayAction::FocusSession("first-session".into()), cx),
            Some(first_id)
        );
        assert_eq!(
            controller.tray_action_workspace(&TrayAction::ActiveSessions, cx),
            Some(second_id)
        );
        assert!(
            controller
                .tray_snapshot(cx)
                .action_enabled(&TrayAction::FocusSession("first-session".into()))
        );
        assert!(
            controller
                .tray_snapshot(cx)
                .action_enabled(&TrayAction::FocusSession("second-session".into()))
        );
        assert!(controller.cloud_sync_pull_blocked_excluding(second_id, cx));
    });
    // Transfer the owning application catalog between shells; the action contains
    // only the stable session id, never the window captured by an old menu.
    cx.update_entity(&shells[0], |shell, _| shell.app = Some(second_app));
    cx.update_entity(&shells[1], |shell, _| shell.app = Some(first_app));
    cx.update_entity(&controller, |controller, cx| {
        assert_eq!(
            controller.tray_action_workspace(&TrayAction::FocusSession("first-session".into()), cx),
            Some(second_id)
        );
    });
    cx.update_entity(&shells[1], |shell, _| shell.app = None);
    cx.update_entity(&controller, |controller, cx| {
        assert_eq!(
            controller.tray_action_workspace(&TrayAction::FocusSession("first-session".into()), cx),
            None
        );
        assert_eq!(
            controller.tray_action_workspace(&TrayAction::Settings, cx),
            Some(first_id)
        );
        assert!(controller.cloud_sync_pull_blocked_excluding(first_id, cx));
    });
}
