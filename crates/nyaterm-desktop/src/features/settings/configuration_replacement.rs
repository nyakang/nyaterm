use gpui::{AsyncApp, Context, WeakEntity};
use nyaterm_store::LoadBootstrap;
use rust_i18n::t;

use crate::app_shell::DesktopController;
use crate::app_shell::configuration_replacement::{
    ConfigurationReplacementBlockReason, ConfigurationReplacementLease,
};
use crate::features::NyaTermApp;

impl NyaTermApp {
    pub(in crate::features) fn configuration_replacement_block_reason(
        &self,
        cx: &gpui::App,
    ) -> Option<ConfigurationReplacementBlockReason> {
        if let Some(reason) = self.cloud_sync.operations.replacement_gate().block_reason() {
            return Some(reason);
        }
        if self.cloud_sync_session_restore_blocked() {
            return Some(ConfigurationReplacementBlockReason::CurrentWindowSession);
        }
        if self
            .desktop_controller
            .as_ref()
            .and_then(WeakEntity::upgrade)
            .is_some_and(|controller| {
                controller
                    .read(cx)
                    .cloud_sync_pull_blocked_excluding(self.workspace_id, cx)
            })
        {
            return Some(ConfigurationReplacementBlockReason::OtherWindowSession);
        }
        None
    }

    pub(in crate::features) fn configuration_replacement_message(
        &self,
        local_key: &str,
        cx: &gpui::App,
    ) -> Option<String> {
        let key = match self.configuration_replacement_block_reason(cx)? {
            ConfigurationReplacementBlockReason::CurrentWindowSession => local_key,
            ConfigurationReplacementBlockReason::OtherWindowSession => {
                "settings.configurationSessionInAnotherWindow"
            }
            ConfigurationReplacementBlockReason::Running => {
                "settings.configurationReplacementInProgress"
            }
            ConfigurationReplacementBlockReason::RefreshRequired => {
                "settings.configurationRefreshRequired"
            }
        };
        Some(t!(key).to_string())
    }

    /// Check and acquisition run in one UI callback, before any start can interleave.
    pub(in crate::features) fn begin_configuration_replacement(
        &mut self,
        local_key: &str,
        cx: &mut Context<Self>,
    ) -> Option<ConfigurationReplacementLease> {
        if let Some(message) = self.configuration_replacement_message(local_key, cx) {
            self.shell.set_status(message.clone());
            self.cloud_sync.set_status(message.clone());
            self.settings.set_store_message(message);
            self.retry_configuration_refresh(cx);
            cx.notify();
            return None;
        }
        self.cloud_sync.operations.replacement_gate().try_acquire()
    }

    pub(in crate::features) fn remote_start_blocked_by_configuration_replacement(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(reason) = self.cloud_sync.operations.replacement_gate().block_reason() else {
            return false;
        };
        let key = if reason == ConfigurationReplacementBlockReason::RefreshRequired {
            "settings.configurationRefreshRequired"
        } else {
            "settings.configurationReplacementInProgress"
        };
        self.shell.set_status(t!(key));
        // Retrying the connection is also the recovery entry after a failed refresh.
        self.retry_configuration_refresh(cx);
        cx.notify();
        true
    }

    pub(crate) fn report_configuration_refresh_failure(&mut self, cx: &mut Context<Self>) {
        self.shell
            .set_status(t!("settings.configurationRefreshRequired"));
        cx.notify();
    }

    pub(in crate::features) fn retry_configuration_refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(lease) = self.cloud_sync.operations.replacement_gate().try_recover() {
            self.refresh_configuration_replacement(lease, cx);
        }
    }

    pub(in crate::features) fn finish_configuration_replacement(
        lease: ConfigurationReplacementLease,
        controller: Option<WeakEntity<DesktopController>>,
        app: &WeakEntity<Self>,
        cx: &mut AsyncApp,
    ) {
        if let Some(controller) = controller.and_then(|controller| controller.upgrade()) {
            controller.update(cx, |controller, cx| {
                controller.refresh_after_configuration_replacement(lease, cx)
            });
        } else {
            let _ = app.update(cx, |app, cx| {
                app.refresh_configuration_replacement(lease, cx)
            });
        }
    }

    fn refresh_configuration_replacement(
        &mut self,
        lease: ConfigurationReplacementLease,
        cx: &mut Context<Self>,
    ) {
        if let Some(controller) = self.desktop_controller.clone() {
            cx.defer(move |cx| {
                let _ = controller.update(cx, |controller, cx| {
                    controller.refresh_after_configuration_replacement(lease, cx)
                });
            });
        } else {
            self.submit_store_request(
                0,
                LoadBootstrap,
                move |app, event, cx| match event.outcome {
                    Ok(snapshot) => {
                        let rebase = app.shell.has_settings_draft() && !app.settings_draft_dirty();
                        app.apply_store_refresh(snapshot, cx);
                        if rebase {
                            app.rebase_open_settings_draft(cx);
                        }
                        lease.refresh_completed();
                    }
                    Err(_) => {
                        app.shell
                            .set_status(t!("settings.configurationRefreshRequired"));
                        cx.notify();
                    }
                },
                cx,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app_shell::configuration_replacement::ConfigurationReplacementBlockReason;
    use crate::app_shell::{AppShellStartup, DesktopController};
    use gpui::{AppContext as _, TestAppContext};
    use nyaterm_core::AiExecutionProfile;
    use nyaterm_core::{AppRuntime, RuntimeMode, WorkspaceId};
    use nyaterm_transport::{LocalSessionConfig, TelnetSessionConfig};

    use crate::features::session::SavedConnectionStartOptions;
    use crate::features::test_support::app_with_visible_local_session;
    use crate::models::SessionLaunchConfig;
    use crate::test_support::{TestAppCleanup, TestConfigDir};

    #[test]
    fn replacement_permit_blocks_remote_launches_but_allows_local_shells() {
        let root = TestConfigDir::new("nyaterm-configuration-start-admission");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, cx| {
            let lease = app
                .begin_configuration_replacement("settings.closeActiveSessionBeforeImport", cx)
                .unwrap();
            app.begin_background_session_start(
                "remote".into(),
                SessionLaunchConfig::Telnet(TelnetSessionConfig::default()),
                None,
                AiExecutionProfile::Disabled,
                SavedConnectionStartOptions::default(),
                cx,
            );
            assert!(!app.session.start_has_pending());
            app.begin_background_session_start(
                "local".into(),
                SessionLaunchConfig::Local(LocalSessionConfig {
                    shell_path: Some(
                        root.path()
                            .join("missing-test-shell")
                            .to_string_lossy()
                            .into_owned(),
                    ),
                    ..Default::default()
                }),
                None,
                AiExecutionProfile::Disabled,
                SavedConnectionStartOptions::default(),
                cx,
            );
            assert!(app.session.start_has_pending());
            drop(lease);
        });
    }

    #[test]
    fn preparing_saved_credentials_blocks_replacement_before_a_worker_exists() {
        let root = TestConfigDir::new("nyaterm-configuration-preparing");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, cx| {
            let placement = app
                .prepare_session_start_options(Default::default())
                .tab_placement
                .unwrap();
            let preparing_id = app.session.simulate_saved_preparation("saved", placement);
            assert!(app.session.start_has_pending());
            assert!(
                app.begin_configuration_replacement("settings.closeActiveSessionBeforeImport", cx)
                    .is_none()
            );
            app.session.simulate_close_preparation(&preparing_id);
            assert!(
                app.begin_configuration_replacement("settings.closeActiveSessionBeforeImport", cx)
                    .is_some()
            );
        });
    }
    #[test]
    fn import_rechecks_other_windows_and_shares_start_admission_with_them() {
        let root = TestConfigDir::new("nyaterm-import-multiple-windows");
        let first_root = TestConfigDir::new("nyaterm-import-first-window");
        let second_root = TestConfigDir::new("nyaterm-import-second-window");
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
        let controller = cx.new(|cx| DesktopController::new(runtime.clone(), startup, cx));
        let first = app_with_visible_local_session(&mut cx, first_root.path(), "first");
        let second = app_with_visible_local_session(&mut cx, second_root.path(), "second");
        let operations = cx.read(|cx| controller.read(cx).sync_operations());
        for app in [&first, &second] {
            let id = WorkspaceId::new();
            app.update(&mut cx, |app, _| {
                app.workspace_id = id;
                app.desktop_controller = Some(controller.downgrade());
                app.cloud_sync.operations = operations.clone();
            });
            controller.update(&mut cx, |controller, cx| {
                controller.attach_configuration_test_window(id, app.clone(), cx)
            });
        }
        // Opening a picker is allowed while both windows contain local shells.
        second.update(&mut cx, |app, cx| {
            assert!(app.configuration_replacement_block_reason(cx).is_none())
        });
        first.update(&mut cx, |app, _| {
            app.session.metadata_mut("first").unwrap().launch_config =
                SessionLaunchConfig::Ssh(Box::default());
        });
        second.update(&mut cx, |app, cx| {
            assert_eq!(
                app.configuration_replacement_block_reason(cx),
                Some(ConfigurationReplacementBlockReason::OtherWindowSession)
            );
            assert!(
                app.begin_configuration_replacement("settings.closeActiveSessionBeforeImport", cx)
                    .is_none()
            );
        });
        second.update(&mut cx, |app, cx| {
            app.apply_cloud_sync_v3_preview(cx);
            assert_eq!(
                app.cloud_sync.status(),
                rust_i18n::t!("settings.configurationSessionInAnotherWindow")
            );
        });
        first.update(&mut cx, |app, _| {
            app.session.mark_session_disconnected("first");
        });
        let lease = second.update(&mut cx, |app, cx| {
            app.begin_configuration_replacement("settings.closeActiveSessionBeforeImport", cx)
                .unwrap()
        });
        first.update(&mut cx, |app, cx| {
            assert!(app.remote_start_blocked_by_configuration_replacement(cx))
        });
        lease.require_refresh();
        controller.update(&mut cx, |controller, cx| {
            controller.refresh_after_configuration_replacement(lease, cx)
        });
        assert!(operations.replacement_gate().block_reason().is_some());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while operations.replacement_gate().block_reason().is_some()
            && std::time::Instant::now() < deadline
        {
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(operations.replacement_gate().block_reason().is_none());
        first.update(&mut cx, |app, cx| {
            assert!(!app.remote_start_blocked_by_configuration_replacement(cx))
        });
    }
}
