use crate::features::NyaTermApp;
use gpui::{Context, Window};
use nyaterm_transport::connection_attempt::ConnectionAttempt;
use nyaterm_updater::download::download_signed_update;
pub(in crate::features) use nyaterm_updater::download::supports_native_install;

#[derive(Clone, Debug)]
pub(crate) enum DownloadState {
    Idle,
    Downloading { received: u64, total: Option<u64> },
    Ready(super::install::PreparedUpdate),
    Failed(String),
}

fn defer_update_quit(
    controller: gpui::WeakEntity<crate::app_shell::DesktopController>,
    cx: &mut Context<NyaTermApp>,
) {
    // Quit coordination reads each workspace's app, including the caller.
    // Release the button callback's entity lease before visiting those apps.
    cx.defer(move |cx| {
        let _ = controller.update(cx, |controller, cx| controller.request_quit(cx));
    });
}

impl NyaTermApp {
    pub(crate) fn native_update_install_requested(&self, cx: &gpui::App) -> bool {
        let update = self.update.read(cx);
        update.install_requested || matches!(update.phase(), super::UpdatePhase::Applying)
    }

    pub(in crate::features) fn start_native_update_download(&mut self, cx: &mut Context<Self>) {
        if !self.cloud_sync.operations.accepting() {
            return;
        }
        if !supports_native_install(self.runtime.mode() == nyaterm_core::RuntimeMode::Portable) {
            return;
        }
        let cancel = ConnectionAttempt::default();
        let Some((info, generation, event_tx)) = self.update.update(cx, |update, cx| {
            let request = update.begin_download(cancel.clone());
            if request.is_some() {
                cx.notify();
            }
            request
        }) else {
            return;
        };
        let directory = self
            .runtime
            .cache_dir()
            .join("updates")
            .join(nyaterm_core::uuid());
        let portable = self.runtime.mode() == nyaterm_core::RuntimeMode::Portable;
        let progress_tx = event_tx.clone();
        let result_tx = event_tx.clone();
        let blocking_jobs = self.update.read(cx).blocking_jobs();
        let scheduled = blocking_jobs.submit_detached("update-download", move |_| {
            let result = download_signed_update(
                &info.latest_version,
                &directory,
                portable,
                || cancel.check(),
                |received, total| {
                    let _ = progress_tx.unbounded_send(super::UpdateEvent::Download {
                        generation,
                        state: DownloadState::Downloading { received, total },
                    });
                },
            );
            let state = match result {
                Ok(prepared) => DownloadState::Ready(prepared),
                Err(error) => DownloadState::Failed(error),
            };
            let _ = result_tx.unbounded_send(super::UpdateEvent::Download { generation, state });
        });
        if let Err(error) = scheduled {
            let _ = event_tx.unbounded_send(super::UpdateEvent::Download {
                generation,
                state: DownloadState::Failed(format!("could not start update download: {error}")),
            });
        }
        cx.notify();
    }

    pub(in crate::features) fn cancel_native_update_download(&mut self, cx: &mut Context<Self>) {
        self.update.update(cx, |update, cx| {
            update.cancel_download();
            cx.notify();
        });
        cx.notify();
    }

    pub(in crate::features) fn prepare_native_update_exit(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        let update = self.update.read(cx);
        if !update.install_requested {
            return true;
        }
        let DownloadState::Ready(_) = update.download else {
            return false;
        };
        true
    }

    pub(crate) fn launch_pending_update_after_shutdown(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let prepared = self.update.update(cx, |update, _| {
            if !update.install_requested {
                return Ok(None);
            }
            let DownloadState::Ready(prepared) = update.download.clone() else {
                return Err("the downloaded update is no longer available".to_string());
            };
            update.clear_install_request();
            Ok(Some(prepared))
        })?;
        let Some(prepared) = prepared else {
            return Ok(());
        };
        super::install::launch_installer(&prepared, &self.settings.summary().language)?;
        self.update.update(cx, |update, cx| {
            update.mark_applying();
            cx.notify();
        });
        Ok(())
    }

    pub(in crate::features) fn request_native_update_install(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.update.read(cx).download, DownloadState::Ready(_)) {
            return;
        }
        if self.notes.has_open_editor_windows()
            || self
                .transfer
                .editor_workspace()
                .is_some_and(|workspace| workspace.tabs.iter().any(|tab| tab.dirty || tab.saving))
        {
            self.notify_operation(
                "update-unsaved",
                nyaterm_ui::notification::NyaNotificationKind::Warning,
                rust_i18n::t!("updater.saveEditorsFirst").to_string(),
                cx,
            );
            return;
        }
        self.update.update(cx, |update, cx| {
            update.install_requested = true;
            cx.notify();
        });
        self.close_update_dialog(window, cx);
        if let Some(controller) = self.desktop_controller.clone() {
            defer_update_quit(controller, cx);
        } else {
            // Installing an update is an explicit quit, even when closing a
            // window normally hides it to the tray.
            self.handle_window_close_request_with_count(0, window, cx);
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn restart_update_without_controller_bypasses_close_to_tray_and_session_confirmation() {
        use gpui::{AppContext as _, EmptyView, TestAppContext};
        use nyaterm_core::test_support::TestTempDir;
        use std::cell::Cell;
        use std::rc::Rc;

        use super::DownloadState;
        use crate::features::AppLifecycleEvent;
        use crate::features::test_support::app_with_visible_local_session;
        use crate::features::update::install::PreparedUpdate;

        let root = TestTempDir::new("nyaterm-update-without-controller");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "update-session");
        app.update(&mut cx, |app, cx| {
            assert!(app.desktop_controller.is_none());
            let mut settings = app.settings.summary().clone();
            settings.minimize_to_tray = true;
            settings.confirm_on_close = true;
            settings.startup_restore = false;
            app.settings.replace_summary(settings);
            app.sync_component_theme(cx);
            app.update.update(cx, |update, _| {
                #[cfg(windows)]
                let prepared = PreparedUpdate::WindowsInstalled {
                    helper: root.join("unused-helper.exe"),
                    installer: root.join("unused-installer.exe"),
                    target: root.join("NyaTerm.exe"),
                    work_dir: root.join("update-work"),
                };
                #[cfg(not(windows))]
                let prepared = PreparedUpdate::Installed {
                    artifact: root.join("unused-installer"),
                    target: root.join("nyaterm"),
                };
                update.download = DownloadState::Ready(prepared);
            });
        });
        let shutdown_requests = Rc::new(Cell::new(0));
        let observed = Rc::clone(&shutdown_requests);
        let _subscription = cx.update(|cx| {
            cx.subscribe(&app, move |_, event, _| {
                if matches!(event, AppLifecycleEvent::ShutdownRequested) {
                    observed.set(observed.get() + 1);
                }
            })
        });

        let (_, window_cx) = cx.add_window_view(|window, cx| {
            let content = cx.new(|_| EmptyView);
            nyaterm_ui::nya_root(content, window, cx)
        });
        window_cx.update(|window, cx| {
            app.update(cx, |app, cx| app.request_native_update_install(window, cx));
        });
        cx.run_until_parked();

        assert_eq!(shutdown_requests.get(), 1);
        cx.read(|cx| {
            let app = app.read(cx);
            assert!(app.update.read(cx).install_requested);
            assert!(app.has_live_sessions());
            assert!(!app.session.dialog_should_quit_after_close_all());
        });
    }

    #[cfg(windows)]
    #[test]
    fn installer_failure_preserves_sync_result_in_final_snapshot_and_allows_retry() {
        use crate::features::settings::SettingsPersistenceDomain;
        use gpui::{AppContext as _, EmptyView, TestAppContext, px, size};
        use nyaterm_core::cloud_sync::control::CloudSyncOperationControl;
        use nyaterm_core::{
            AppRuntime, MainWindowBounds, MainWindowState, RuntimeMode, test_support::TestTempDir,
        };
        use nyaterm_store::StoreDomain;
        use std::time::{Duration, Instant};

        use super::DownloadState;
        use crate::app_shell::{AppShellStartup, DesktopController};
        use crate::features::test_support::app_with_visible_local_session;
        use crate::features::update::install::PreparedUpdate;

        let root = TestTempDir::new("nyaterm-update-failed-launch-controller");
        let app_root = TestTempDir::new("nyaterm-update-failed-launch-app");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
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
        let app = app_with_visible_local_session(&mut cx, &app_root, "update-failure-session");
        let handle = cx.open_window(size(px(800.), px(600.)), |_, _| EmptyView);
        let workspace_id = cx.read(|cx| app.read(cx).workspace_id);
        let shell = controller.update(&mut cx, |controller, cx| {
            let shell = controller.register_tab_drag_test_workspace(
                workspace_id,
                app.clone(),
                handle.into(),
                cx,
            );
            drop(
                controller
                    .submit_window_state(
                        workspace_id,
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
        let operations = cx.read(|cx| controller.read(cx).sync_operations());
        let update = cx.read(|cx| controller.read(cx).update_store());
        app.update(&mut cx, |app, _| {
            app.update = update.clone();
            app.cloud_sync.operations = operations.clone();
        });
        // Run the real persistence and exit coordination twice. The helper path
        // deliberately does not exist, so no installer process can be launched.
        for language in ["ja", "en"] {
            let control = CloudSyncOperationControl::default();
            control.begin_commit().unwrap();
            let registration = operations
                .register("committing-sync", vec![control], None)
                .unwrap();
            app.update(&mut cx, |app, _| {
                app.settings
                    .mark_persistence_dirty(SettingsPersistenceDomain::General);
            });
            update.update(&mut cx, |update, _| {
                update.install_requested = true;
                update.download = DownloadState::Ready(PreparedUpdate::WindowsInstalled {
                    helper: root.join("missing-helper.exe"),
                    installer: root.join("missing-installer.exe"),
                    target: root.join("NyaTerm.exe"),
                    work_dir: root.join("update-work"),
                });
            });
            shell.update(&mut cx, |shell, cx| shell.request_close(cx));
            // A committed pull updates storage first and then reloads GPUI state.
            // The exit snapshot must wait for that reload, even after the main
            // operation registration has been released.
            let store = app.update(&mut cx, |app, cx| {
                let store = app.store_blocking_client();
                store
                    .request_fn(StoreDomain::CloudSync, move |store| {
                        let mut settings = store.load_app_settings_summary()?;
                        settings.language = language.into();
                        store.save_general_settings(&settings)
                    })
                    .unwrap();
                app.refresh_store_from_runtime_and_sync_theme(cx);
                app.queue_cloud_sync_credentials_refresh(cx);
                store
            });
            drop(registration);
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                cx.run_until_parked();
                let attempted = cx.read(|cx| !update.read(cx).install_requested);
                if attempted && operations.accepting() {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "failed installer launch did not restore the app"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            let persisted_language = store
                .request_fn(StoreDomain::Settings, |store| {
                    Ok(store.load_app_settings_summary()?.language)
                })
                .unwrap();
            assert_eq!(persisted_language, language);
            cx.read(|cx| {
                let app = app.read(cx);
                assert_eq!(app.settings.summary().language, language);
                assert!(
                    app.blocking_jobs
                        .submit_detached("manual-sync-retry", |_| {})
                        .is_ok()
                );
                assert!(
                    update
                        .read(cx)
                        .blocking_jobs()
                        .submit_detached("process-sync-retry", |_| {})
                        .is_ok()
                );
            });
        }
    }

    #[cfg(windows)]
    #[test]
    fn restart_update_releases_the_app_before_quit_coordination_reads_it() {
        use gpui::{AppContext as _, EmptyView, TestAppContext, px, size};
        use nyaterm_core::models::window_state::{MainWindowBounds, MainWindowState};
        use nyaterm_core::{WorkspaceId, test_support::TestTempDir};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        use super::{DownloadState, defer_update_quit};
        use crate::app_shell::{AppShellStartup, DesktopController};
        use crate::features::AppLifecycleEvent;
        use crate::features::test_support::app_with_visible_local_session;
        use crate::features::update::install::PreparedUpdate;

        let root = TestTempDir::new("nyaterm-update-quit-lease");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, &root, "update-session");
        let runtime = cx.read(|cx| app.read(cx).runtime.clone());
        let startup = AppShellStartup::prepare(&runtime);
        let controller = cx.new(|cx| DesktopController::new(runtime, startup, cx));
        let workspace_id = WorkspaceId::new();
        let handle = cx.open_window(size(px(800.), px(600.)), |_, _| EmptyView);
        let shell = controller.update(&mut cx, |controller, cx| {
            let shell = controller.register_tab_drag_test_workspace(
                workspace_id,
                app.clone(),
                handle.into(),
                cx,
            );
            drop(
                controller
                    .submit_window_state(
                        workspace_id,
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
        app.update(&mut cx, |app, cx| {
            app.set_desktop_controller(controller.downgrade());
            let mut settings = app.settings.summary().clone();
            settings.confirm_on_close = false;
            settings.startup_restore = false;
            app.settings.replace_summary(settings);
            app.update.update(cx, |update, _| {
                update.install_requested = true;
                update.download = DownloadState::Ready(PreparedUpdate::WindowsInstalled {
                    helper: root.join("unused-helper.exe"),
                    installer: root.join("unused-installer.exe"),
                    target: root.join("NyaTerm.exe"),
                    work_dir: root.join("update-work"),
                });
            });
        });
        let shutdown_requests = Arc::new(AtomicUsize::new(0));
        let observed = shutdown_requests.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&app, move |_, event, _| {
                if matches!(event, AppLifecycleEvent::ShutdownRequested) {
                    observed.fetch_add(1, Ordering::SeqCst);
                }
            })
        });
        app.update(&mut cx, |_, cx| {
            defer_update_quit(controller.downgrade(), cx)
        });
        cx.run_until_parked();
        assert_eq!(shutdown_requests.load(Ordering::SeqCst), 1);
        cx.read(|cx| {
            assert!(app.read(cx).update.read(cx).install_requested);
            assert!(app.read(cx).has_live_sessions());
        });
        drop(shell);
    }
}
