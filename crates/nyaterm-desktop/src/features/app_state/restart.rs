use gpui::Context;
use rust_i18n::t;

use crate::features::NyaTermApp;

impl NyaTermApp {
    pub(crate) fn data_directory_restart_requested(&self, cx: &gpui::App) -> bool {
        self.desktop_controller
            .as_ref()
            .and_then(|controller| controller.upgrade())
            .is_some_and(|controller| controller.read(cx).data_directory_restart_requested())
    }

    pub(crate) fn notify_data_directory_restart_blocked(&self, cx: &mut Context<Self>) {
        self.notify_operation(
            "data-directory-restart-blocked",
            nyaterm_ui::notification::NyaNotificationKind::Warning,
            t!("settings.dataDirectoryBlockedUnsaved").to_string(),
            cx,
        );
    }

    /// Surfaces a failed pending switch that fell back to the previous
    /// directory, so the user is not left guessing after the restart.
    pub(crate) fn notify_data_directory_startup_notice(
        &self,
        notice: String,
        cx: &mut Context<Self>,
    ) {
        self.notify_background_operation(
            "data-directory-startup-notice",
            nyaterm_ui::notification::NyaNotificationKind::Warning,
            format!("{} {}", t!("settings.dataDirectoryNotice"), notice),
            cx,
        );
    }

    pub(in crate::features) fn request_data_directory_restart(
        &mut self,
        choice: nyaterm_core::data_directory::PreparedSwitch,
        cx: &mut Context<Self>,
    ) {
        if let Some(controller) = self.desktop_controller.clone() {
            cx.defer(move |cx| {
                let _ = controller.update(cx, |controller, cx| {
                    controller.request_data_directory_restart(choice, cx)
                });
            });
        }
    }

    pub(crate) fn has_external_editors_blocking_restart(&self) -> bool {
        self.connection_state.active_editor_draft().is_some()
            || self.notes.has_open_editor_windows()
            || self
                .transfer
                .editor_documents()
                .iter()
                .any(|document| document.saving)
    }
}

#[cfg(test)]
mod tests {
    use crate::app_shell::{AppShellStartup, DesktopController};
    use crate::features::AppLifecycleEvent;
    use crate::features::test_support::app_with_visible_local_session;
    use crate::test_support::TestAppCleanup;
    use gpui::{AppContext as _, EmptyView, TestAppContext, px, size};
    use nyaterm_core::data_directory::{PreparedSwitch, SwitchMode};
    use nyaterm_core::{
        AppRuntime, MainWindowBounds, MainWindowState, RuntimeMode, WorkspaceId,
        test_support::TestTempDir,
    };
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn restart_uses_process_quit_with_two_windows_and_cancellation_clears_every_gate() {
        let root = TestTempDir::new("nyaterm-restart-process");
        let roots = [
            TestTempDir::new("nyaterm-restart-first"),
            TestTempDir::new("nyaterm-restart-second"),
        ];
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Installed,
            root.join("local"),
            root.join("local"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let startup = AppShellStartup::prepare(&runtime);
        let controller = cx.new(|cx| DesktopController::new(runtime.clone(), startup, cx));
        let requests = Rc::new(Cell::new(0));
        let mut shells = Vec::new();
        let mut apps = Vec::new();
        let mut subscriptions = Vec::new();
        for (index, app_root) in roots.iter().enumerate() {
            let app = app_with_visible_local_session(
                &mut cx,
                app_root.path(),
                &format!("session-{index}"),
            );
            let id = WorkspaceId::new();
            app.update(&mut cx, |app, _| {
                app.set_desktop_controller(controller.downgrade());
                app.set_workspace_identity(id, 0);
            });
            let handle = cx.open_window(size(px(800.), px(600.)), |_, _| EmptyView);
            let shell = controller.update(&mut cx, |controller, cx| {
                let shell =
                    controller.register_tab_drag_test_workspace(id, app.clone(), handle.into(), cx);
                drop(
                    controller
                        .submit_window_state(
                            id,
                            MainWindowState::new(
                                None,
                                MainWindowBounds {
                                    x: 0,
                                    y: 0,
                                    width: 800,
                                    height: 600,
                                },
                                false,
                            ),
                            0,
                        )
                        .unwrap(),
                );
                shell
            });
            let observed = requests.clone();
            subscriptions.push(cx.update(|cx| {
                cx.subscribe(&app, move |_, event, _| {
                    if matches!(event, AppLifecycleEvent::ShutdownRequested) {
                        observed.set(observed.get() + 1);
                    }
                })
            }));
            shells.push(shell);
            apps.push(app);
        }
        controller.update(&mut cx, |controller, cx| {
            controller.request_data_directory_restart(
                PreparedSwitch {
                    target: root.join("target"),
                    mode: SwitchMode::Migrate,
                    sync_client: None,
                    target_fingerprint: None,
                },
                cx,
            )
        });
        cx.run_until_parked();
        assert_eq!(requests.get(), 1);
        cx.read(|cx| {
            assert!(controller.read(cx).data_directory_restart_requested());
            assert_eq!(controller.read(cx).workspace_count(), 2);
            assert!(apps.iter().all(|app| app.read(cx).has_live_sessions()));
        });
        assert!(
            nyaterm_store::data_directory::read_location(runtime.local_dir())
                .unwrap()
                .is_none()
        );
        controller.update(&mut cx, |controller, cx| {
            controller.cancel_editor_quit_gates(cx)
        });
        cx.run_until_parked();
        cx.read(|cx| assert!(!controller.read(cx).data_directory_restart_requested()));
        assert!(
            nyaterm_store::data_directory::read_location(runtime.local_dir())
                .unwrap()
                .is_none()
        );
    }
}
