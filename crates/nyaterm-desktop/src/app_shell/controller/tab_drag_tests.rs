use gpui::{AppContext as _, Entity, TestAppContext, px, size};
use nyaterm_core::{AppRuntime, RuntimeMode, WorkspaceId, test_support::TestTempDir};

use super::{DesktopController, WorkspaceWindow};
use crate::app_shell::{AppShell, AppShellLifecycle, AppShellStartup};
use crate::features::NyaTermApp;
use crate::features::test_support::app_with_visible_local_session;

impl DesktopController {
    pub(crate) fn register_tab_drag_test_workspace(
        &mut self,
        workspace_id: WorkspaceId,
        app: Entity<NyaTermApp>,
        handle: gpui::AnyWindowHandle,
        cx: &mut gpui::Context<Self>,
    ) -> Entity<AppShell> {
        let controller = cx.entity();
        let shell = cx.new(|cx| {
            AppShell::new(
                self.runtime.clone(),
                None,
                self.startup.for_workspace(workspace_id),
                workspace_id,
                controller,
                self.session_hub.clone(),
                cx,
            )
        });
        shell.update(cx, |shell, _| {
            shell.app = Some(app);
            shell.lifecycle = AppShellLifecycle::Ready;
        });
        self.windows.insert(
            workspace_id,
            WorkspaceWindow {
                handle,
                shell: shell.downgrade(),
            },
        );
        shell
    }
}

fn with_source(
    test: impl FnOnce(&Entity<DesktopController>, WorkspaceId, &Entity<NyaTermApp>, &mut TestAppContext),
) {
    let root = TestTempDir::new("nyaterm-tab-drag-preflight-controller");
    let app_root = TestTempDir::new("nyaterm-tab-drag-preflight-app");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let runtime = AppRuntime::from_parts_for_test(
        RuntimeMode::Portable,
        root.path().into(),
        root.path().join("config"),
        root.path().join("logs"),
        root.path().join("cache"),
        None,
    );
    let startup = AppShellStartup::prepare(&runtime);
    let controller = cx.new(|cx| DesktopController::new(runtime.clone(), startup, cx));
    let app = app_with_visible_local_session(&mut cx, app_root.path(), "tab");
    let source_id = WorkspaceId::new();
    let (startup, session_hub) = cx.read(|cx| {
        let controller = controller.read(cx);
        (
            controller.startup.for_workspace(source_id),
            controller.session_hub.clone(),
        )
    });
    let handle = cx.open_window(size(px(800.), px(600.)), |_, cx| {
        AppShell::new(
            runtime,
            None,
            startup,
            source_id,
            controller.clone(),
            session_hub,
            cx,
        )
    });
    let shell = handle.root(&mut cx).unwrap();
    shell.update(&mut cx, |shell, _| shell.app = Some(app.clone()));
    controller.update(&mut cx, |controller, cx| {
        controller.windows.insert(
            source_id,
            WorkspaceWindow {
                handle: handle.into(),
                shell: shell.downgrade(),
            },
        );
        let _ = cx;
    });
    test(&controller, source_id, &app, &mut cx);
}

#[test]
fn tab_drag_preflight_never_opens_a_window_for_stale_or_missing_tabs() {
    with_source(|controller, source_id, app, cx| {
        controller.update(cx, |controller, cx| {
            let revision = app.read(cx).workspace_revision();
            let count = cx.windows().len();
            let error = controller
                .open_workspace_for_tab(source_id, "tab".into(), revision + 1, cx)
                .unwrap_err();
            assert!(error.to_string().contains("changed during the drag"));
            assert!(
                controller
                    .open_workspace_for_tab(source_id, "missing".into(), revision, cx)
                    .is_err()
            );
            assert_eq!(cx.windows().len(), count);
            assert!(controller.pending_tab_moves.is_empty());
            assert!(app.read(cx).has_live_sessions());
        });
    });
}

#[test]
fn tab_drag_failed_window_cleanup_waits_for_other_closes_and_spares_user_sessions() {
    with_source(|controller, source_id, app, cx| {
        controller.update(cx, |controller, cx| {
            let other_close = WorkspaceId::new();
            controller.closing_workspaces.insert(other_close);
            controller.tab_drag.queue_failed_window(source_id);
            controller.close_failed_tab_windows(cx);
            assert_eq!(controller.tab_drag.next_failed_window(), Some(source_id));
            controller.tab_drag.queue_failed_window(source_id);
            controller.closing_workspaces.remove(&other_close);
            controller.close_failed_tab_windows(cx);
            assert!(controller.windows.contains_key(&source_id));
            assert!(controller.closing_workspaces.is_empty());
            assert!(app.read(cx).has_live_sessions());
        });
    });
}
