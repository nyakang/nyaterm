use super::{UpdateEvent, UpdatePhase, UpdateStore};
use crate::features::update::download::DownloadState;
use futures::channel::mpsc::UnboundedSender;
use nyaterm_updater::install::PreparedUpdate;
use nyaterm_updater::linux::{InstallOutcome, PreparedPackage};

impl UpdateStore {
    pub(in crate::features) fn begin_package_install(
        &mut self,
    ) -> Option<(PreparedPackage, u64, UnboundedSender<UpdateEvent>)> {
        if self.shutdown_started
            || !matches!(
                self.phase,
                UpdatePhase::Ready
                    | UpdatePhase::LinuxPassword
                    | UpdatePhase::LinuxAuthorizationFailed
                    | UpdatePhase::LinuxInstallFailed
            )
        {
            return None;
        }
        let DownloadState::Ready(PreparedUpdate::LinuxPackage(package)) = &self.download else {
            return None;
        };
        let package = package.clone();
        self.install_generation = self.install_generation.wrapping_add(1);
        self.install_detail = None;
        self.assign_phase(UpdatePhase::LinuxInstalling);
        Some((package, self.install_generation, self.tx.clone()))
    }

    pub(in crate::features) fn request_password(&mut self) {
        if matches!(self.phase, UpdatePhase::LinuxAuthorizationFailed) {
            self.install_detail = None;
            self.assign_phase(UpdatePhase::LinuxPassword);
        }
    }

    pub(in crate::features) fn cancel_password(&mut self) {
        if matches!(self.phase, UpdatePhase::LinuxPassword) {
            self.install_detail = None;
            self.assign_phase(UpdatePhase::Ready);
        }
    }

    pub(crate) fn package_restart_target(&self) -> Option<std::path::PathBuf> {
        if !self.install_requested || !matches!(self.phase, UpdatePhase::LinuxInstalled) {
            return None;
        }
        let DownloadState::Ready(PreparedUpdate::LinuxPackage(package)) = &self.download else {
            return None;
        };
        Some(package.restart_target())
    }

    pub(super) fn apply_install_event(&mut self, generation: u64, outcome: InstallOutcome) -> bool {
        if generation != self.install_generation || !self.system_install_busy() {
            return false;
        }
        let phase = match outcome {
            InstallOutcome::Installed => UpdatePhase::LinuxInstalled,
            InstallOutcome::Cancelled => UpdatePhase::Ready,
            InstallOutcome::NeedsPassword => UpdatePhase::LinuxPassword,
            InstallOutcome::AuthorizationFailed => UpdatePhase::LinuxAuthorizationFailed,
            InstallOutcome::PreflightFailed(detail) | InstallOutcome::Failed(detail) => {
                self.install_detail = Some(detail);
                UpdatePhase::LinuxInstallFailed
            }
        };
        self.assign_phase(phase);
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::features::update::{UpdateCheckKind, UpdateEvent, UpdatePhase, UpdateStore};
    use nyaterm_updater::linux::InstallOutcome;

    fn installing() -> UpdateStore {
        let mut store = UpdateStore::new();
        store.phase = UpdatePhase::LinuxInstalling;
        store.install_generation = 7;
        store
    }

    #[test]
    fn package_installation_blocks_checks_downloads_and_duplicate_install_requests() {
        let mut store = installing();
        assert!(store.system_install_busy());
        assert!(store.begin_check(UpdateCheckKind::Manual).is_none());
        assert!(store.begin_download(Default::default()).is_none());
        assert!(store.begin_package_install().is_none());
        store.cancel_download();
        assert!(store.system_install_busy());
    }

    #[test]
    fn stale_and_duplicate_install_results_cannot_replace_current_state() {
        let mut store = installing();
        assert!(!store.apply_event(UpdateEvent::Install {
            generation: 6,
            outcome: InstallOutcome::Installed
        }));
        assert!(store.system_install_busy());
        assert!(store.apply_event(UpdateEvent::Install {
            generation: 7,
            outcome: InstallOutcome::Cancelled
        }));
        assert!(matches!(store.phase(), UpdatePhase::Ready));
        assert!(!store.apply_event(UpdateEvent::Install {
            generation: 7,
            outcome: InstallOutcome::Installed
        }));
    }

    #[test]
    fn authorization_cancel_does_not_enter_password_fallback() {
        let mut store = installing();
        store.apply_event(UpdateEvent::Install {
            generation: 7,
            outcome: InstallOutcome::Cancelled,
        });
        assert!(matches!(store.phase(), UpdatePhase::Ready));
        store.request_password();
        assert!(matches!(store.phase(), UpdatePhase::Ready));
    }

    #[test]
    fn unavailable_polkit_requests_password_but_authorization_failure_requires_user_choice() {
        let mut store = installing();
        store.apply_event(UpdateEvent::Install {
            generation: 7,
            outcome: InstallOutcome::AuthorizationFailed,
        });
        assert!(matches!(
            store.phase(),
            UpdatePhase::LinuxAuthorizationFailed
        ));
        store.request_password();
        assert!(matches!(store.phase(), UpdatePhase::LinuxPassword));
        store.cancel_password();
        assert!(matches!(store.phase(), UpdatePhase::Ready));

        let mut store = installing();
        store.apply_event(UpdateEvent::Install {
            generation: 7,
            outcome: InstallOutcome::NeedsPassword,
        });
        assert!(matches!(store.phase(), UpdatePhase::LinuxPassword));
    }

    #[test]
    fn installed_update_stays_ready_to_restart_when_quit_is_cancelled() {
        let mut store = installing();
        store.apply_event(UpdateEvent::Install {
            generation: 7,
            outcome: InstallOutcome::Installed,
        });
        assert!(!store.system_install_busy());
        store.install_requested = true;
        store.clear_install_request();
        assert!(!store.install_requested);
        assert!(matches!(store.phase(), UpdatePhase::LinuxInstalled));
        assert!(store.begin_check(UpdateCheckKind::Manual).is_none());
        assert!(store.begin_package_install().is_none());
    }

    #[test]
    fn installation_failure_is_separate_from_download_failure() {
        let mut store = installing();
        store.apply_event(UpdateEvent::Install {
            generation: 7,
            outcome: InstallOutcome::Failed("package lock busy".into()),
        });
        assert!(matches!(store.phase(), UpdatePhase::LinuxInstallFailed));
        assert_eq!(store.install_detail.as_deref(), Some("package lock busy"));
        assert!(!store.system_install_busy());
    }

    #[test]
    fn package_installation_rejects_process_quit_before_pausing_operations() {
        use crate::app_shell::{AppShellStartup, DesktopController};
        use crate::test_support::{TestAppCleanup, TestConfigDir};
        use gpui::{AppContext as _, TestAppContext};
        use nyaterm_core::{AppRuntime, RuntimeMode};

        let root = TestConfigDir::new("nyaterm-package-install-quit");
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
        controller.update(&mut cx, |controller, cx| {
            controller.update_store().update(cx, |store, _| {
                store.phase = UpdatePhase::LinuxInstalling;
                store.install_generation = 7;
            });
            assert!(!controller.begin_process_quit(cx));
            assert!(controller.sync_operations().accepting());
            controller.update_store().update(cx, |store, _| {
                store.apply_install_event(7, InstallOutcome::Installed);
            });
            assert!(controller.begin_process_quit(cx));
            controller.cancel_process_quit();
        });
    }
}
