use super::DesktopController;
use crate::features::update::UpdateCheckKind;
use crate::features::update::UpdateEvent;
use futures::StreamExt as _;
use gpui::{AppContext as _, Context};
use nyaterm_core::packaging;
use std::time::Duration;

impl DesktopController {
    pub(super) fn start_update_runtime(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = crate::features::update::install::take_update_cleanup_path() {
            let blocking_jobs = self.update_store.read(cx).blocking_jobs();
            let operations = self.sync_operations();
            cx.spawn(async move |_, cx| {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                if !operations.accepting() {
                    return;
                }
                let _ = blocking_jobs.submit_detached("update-cleanup", move |_| {
                    crate::features::update::install::cleanup_update_work_dir(path);
                });
            })
            .detach();
        }

        if let Some(mut rx) = self
            .update_store
            .update(cx, |store, _| store.take_event_receiver())
        {
            let update_store = self.update_store.clone();
            cx.spawn(async move |_, cx| {
                while let Some(event) = rx.next().await {
                    update_store.update(cx, |store, cx| {
                        if store.apply_event(event) {
                            cx.notify();
                        }
                    });
                }
            })
            .detach();
        }

        let portable = self.runtime.mode() == nyaterm_core::RuntimeMode::Portable;
        let update_store = self.update_store.clone();
        cx.spawn(async move |_, cx| {
            let installation = cx
                .background_spawn(async move {
                    nyaterm_updater::download::detect_installation(portable)
                })
                .await;
            update_store.update(cx, |store, cx| {
                store.apply_event(UpdateEvent::Installation(installation));
                cx.notify();
            });
        })
        .detach();

        if packaging::updates_managed_externally() {
            // An external package manager owns this build's update channel, so a
            // probe would only advertise a version the install cannot apply.
            return;
        }

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            let _ = this.update(cx, |controller, cx| {
                if controller.process_quitting {
                    return;
                }
                let should_start = controller
                    .update_store
                    .update(cx, |store, _| store.mark_startup_check_started());
                if should_start {
                    controller.start_update_check(UpdateCheckKind::Silent, cx);
                }
            });
        })
        .detach();
    }

    pub(crate) fn start_update_check(&mut self, kind: UpdateCheckKind, cx: &mut Context<Self>) {
        if self.process_quitting || !self.auto_sync.operations.accepting() {
            return;
        }
        let Some((tx, generation)) = self.update_store.update(cx, |store, cx| {
            let request = store.begin_check(kind);
            if request.is_some() {
                cx.notify();
            }
            request
        }) else {
            return;
        };
        let rejected_tx = tx.clone();
        let blocking_jobs = self.update_store.read(cx).blocking_jobs();
        if let Err(error) = blocking_jobs.submit_detached("update-check", move |_| {
            let result = crate::http::update::check_native_update();
            let _ = tx.unbounded_send(UpdateEvent::Check {
                generation,
                kind,
                result,
            });
        }) {
            let _ = rejected_tx.unbounded_send(UpdateEvent::Check {
                generation,
                kind,
                result: Err(format!("could not start update check: {error}")),
            });
        }
    }
}

#[cfg(target_os = "linux")]
impl DesktopController {
    pub(in crate::app_shell) fn launch_package_restart(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Task<Result<(), String>>> {
        let executable = self.update_store.read(cx).package_restart_target()?;
        let instance_dir = self.runtime.instance_dir();
        Some(cx.background_spawn(async move {
            prepare_package_restart(&executable, &instance_dir, |executable, marker| {
                std::process::Command::new(executable)
                    .env(nyaterm_core::RELAUNCH_HANDOFF_ENV, marker)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            })
        }))
    }
}

#[cfg(target_os = "linux")]
fn prepare_package_restart(
    executable: &std::path::Path,
    instance_dir: &std::path::Path,
    launch: impl FnOnce(&std::path::Path, &std::path::Path) -> Result<(), String>,
) -> Result<(), String> {
    let marker = instance_dir.join(format!("restart-{}.handoff", nyaterm_core::uuid()));
    std::fs::write(&marker, []).map_err(|error| error.to_string())?;
    let result = launch(executable, &marker);
    if result.is_err() {
        let _ = std::fs::remove_file(marker);
    }
    result
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::prepare_package_restart;
    use nyaterm_core::test_support::TestTempDir;
    use std::path::Path;

    #[test]
    fn package_restart_uses_captured_install_path_and_instance_handoff() {
        let root = TestTempDir::new("nyaterm-package-restart-handoff");
        std::fs::create_dir_all(root.path()).unwrap();
        let target = Path::new("/opt/nyaterm/nyaterm");
        prepare_package_restart(target, root.path(), |executable, marker| {
            assert_eq!(executable, target);
            assert_eq!(marker.parent(), Some(root.path()));
            assert!(marker.is_file());
            Ok(())
        })
        .unwrap();
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_package_restart_removes_handoff_without_reinstalling() {
        let root = TestTempDir::new("nyaterm-package-restart-failure");
        std::fs::create_dir_all(root.path()).unwrap();
        let error = prepare_package_restart(Path::new("/missing/nyaterm"), root.path(), |_, _| {
            Err("spawn failed".into())
        })
        .unwrap_err();
        assert_eq!(error, "spawn failed");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}
