use std::process::{Command, Stdio};

use gpui::{AppContext as _, Context, Task};
use nyaterm_core::app_identity::AppFlavor;
use nyaterm_core::data_directory::PreparedSwitch;
use nyaterm_store::data_directory::{queue_prepared_switch, read_location, write_location};

use crate::app_shell::DesktopController;

#[derive(Default)]
pub(super) struct DataDirectoryRestartState {
    choice: Option<PreparedSwitch>,
    launching: bool,
    recovery_launching: bool,
}

impl DataDirectoryRestartState {
    pub(super) fn requested(&self) -> bool {
        self.choice.is_some()
    }

    pub(super) fn cancel(&mut self) {
        *self = Self::default();
    }

    fn begin_launch(&mut self) -> Result<PreparedSwitch, String> {
        if self.launching {
            return Err("restart is already being prepared".into());
        }
        let choice = self
            .choice
            .clone()
            .ok_or_else(|| "restart was cancelled".to_string())?;
        self.launching = true;
        Ok(choice)
    }
}

impl DesktopController {
    pub(in crate::app_shell) fn restart_from_recovery(
        &mut self,
        choice: Option<PreparedSwitch>,
        cx: &mut Context<Self>,
    ) {
        if self.process_quitting
            || self.restart.recovery_launching
            || self.startup.shared_store_runtime().is_some()
        {
            return;
        }
        self.restart.recovery_launching = true;
        let runtime = self.runtime.clone();
        let task = cx.background_spawn(async move {
            prepare_recovery_restart(&runtime, choice.as_ref(), launch_successor)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |controller, cx| {
                controller.restart.recovery_launching = false;
                match result {
                    Ok(()) => controller.request_quit_without_ready(cx),
                    Err(error) => {
                        for entry in controller.windows.values() {
                            let _ = entry.shell.update(cx, |shell, cx| {
                                shell.recovery_restart_failed(error.clone(), cx)
                            });
                        }
                    }
                }
            });
        })
        .detach();
    }

    pub(crate) fn data_directory_restart_requested(&self) -> bool {
        self.restart.requested()
    }

    pub(crate) fn request_data_directory_restart(
        &mut self,
        choice: PreparedSwitch,
        cx: &mut Context<Self>,
    ) {
        if self.process_quitting || self.restart.requested() || !self.closing_workspaces.is_empty()
        {
            return;
        }
        let blocked = self.windows.values().any(|entry| {
            entry
                .shell
                .update(cx, |shell, cx| {
                    shell
                        .app
                        .as_ref()
                        .is_some_and(|app| app.read(cx).has_external_editors_blocking_restart())
                })
                .unwrap_or(true)
        });
        if blocked {
            for entry in self.windows.values() {
                let _ = entry.shell.update(cx, |shell, cx| {
                    if let Some(app) = &shell.app {
                        app.update(cx, |app, cx| app.notify_data_directory_restart_blocked(cx));
                    }
                });
            }
            return;
        }
        self.restart.choice = Some(choice);
        self.request_quit(cx);
    }

    /// Called only after all workspace persistence and flush barriers succeeded.
    pub(in crate::app_shell) fn launch_data_directory_restart(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let runtime = self.runtime.clone();
        let choice = self.restart.begin_launch();
        cx.background_spawn(async move {
            let choice = choice?;
            prepare_restart(&runtime, &choice, launch_successor)
        })
    }
}

fn launch_successor(marker: &std::path::Path) -> Result<(), String> {
    let executable =
        std::env::current_exe().map_err(|e| format!("could not locate executable: {e}"))?;
    Command::new(executable)
        .env(nyaterm_core::RELAUNCH_HANDOFF_ENV, marker)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not launch restart: {e}"))?;
    Ok(())
}

fn prepare_recovery_restart(
    runtime: &nyaterm_core::AppRuntime,
    choice: Option<&PreparedSwitch>,
    launch: impl FnOnce(&std::path::Path) -> Result<(), String>,
) -> Result<(), String> {
    // A retry must also work with malformed bootstrap metadata or an environment override.
    let previous = if choice.is_some() {
        Some(
            read_location(runtime.local_dir())
                .map_err(|e| e.to_string())?
                .unwrap_or_default(),
        )
    } else {
        None
    };
    let marker = runtime
        .instance_dir()
        .join(format!("restart-{}.handoff", uuid::Uuid::new_v4()));
    let result = (|| {
        if let Some(choice) = choice {
            nyaterm_store::data_directory::commit_recovery_selection(
                runtime.local_dir(),
                choice,
                AppFlavor::current(),
            )
            .map_err(|e| e.to_string())?;
        }
        std::fs::write(&marker, []).map_err(|e| format!("could not prepare restart: {e}"))?;
        launch(&marker)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(marker);
        if let Some(previous) = previous {
            write_location(runtime.local_dir(), &previous)
                .map_err(|e| format!("could not restore recovery selection: {e}"))?;
        }
    }
    result
}

fn prepare_restart(
    runtime: &nyaterm_core::AppRuntime,
    choice: &PreparedSwitch,
    launch: impl FnOnce(&std::path::Path) -> Result<(), String>,
) -> Result<(), String> {
    let previous = read_location(runtime.local_dir())
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    let marker = runtime
        .instance_dir()
        .join(format!("restart-{}.handoff", uuid::Uuid::new_v4()));
    let result = (|| {
        queue_prepared_switch(
            runtime.local_dir(),
            runtime.data_dir(),
            choice,
            AppFlavor::current(),
        )
        .map_err(|e| e.to_string())?;
        std::fs::write(&marker, []).map_err(|e| format!("could not prepare restart: {e}"))?;
        launch(&marker)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(marker);
        write_location(runtime.local_dir(), &previous).map_err(|e| {
            format!("restart failed and pending selection could not be restored: {e}")
        })?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::prepare_restart;
    use nyaterm_core::{
        AppRuntime, RuntimeMode, app_identity::AppFlavor, test_support::TestTempDir,
    };
    use nyaterm_store::data_directory::{prepare_switch, read_location};

    #[test]
    fn a_restart_launch_is_admitted_once_and_cancel_resets_admission() {
        let choice = nyaterm_core::data_directory::PreparedSwitch {
            target: std::path::PathBuf::from("target"),
            mode: nyaterm_core::data_directory::SwitchMode::Migrate,
            sync_client: None,
            target_fingerprint: None,
        };
        let mut state = super::DataDirectoryRestartState {
            choice: Some(choice.clone()),
            launching: false,
            recovery_launching: false,
        };
        assert!(state.begin_launch().is_ok());
        assert!(state.begin_launch().is_err());
        state.cancel();
        assert!(!state.requested());
        assert!(state.begin_launch().is_err());
        state.choice = Some(choice);
        assert!(state.begin_launch().is_ok());
    }

    #[test]
    fn failed_successor_launch_restores_pending_selection_and_cleans_handoff() {
        for fail in [true, false] {
            let root = TestTempDir::new("nyaterm-restart-handoff");
            let runtime = AppRuntime::from_parts_for_test(
                RuntimeMode::Installed,
                root.join("local"),
                root.join("local"),
                root.join("logs"),
                root.join("cache"),
                None,
            );
            std::fs::create_dir_all(runtime.instance_dir()).unwrap();
            let _store = nyaterm_store::ConnectionStore::open(runtime.data_dir()).unwrap();
            let choice = prepare_switch(
                runtime.data_dir(),
                runtime.local_dir(),
                &root.join("target"),
                AppFlavor::current(),
            )
            .unwrap();
            let mut calls = 0;
            let result = prepare_restart(&runtime, &choice, |marker| {
                calls += 1;
                assert!(marker.exists());
                assert!(
                    read_location(runtime.local_dir())
                        .unwrap()
                        .unwrap()
                        .pending
                        .is_some()
                );
                if fail {
                    Err("injected spawn failure".into())
                } else {
                    Ok(())
                }
            });
            assert_eq!(calls, 1);
            assert_eq!(result.is_err(), fail);
            assert_eq!(
                read_location(runtime.local_dir())
                    .unwrap()
                    .unwrap()
                    .pending
                    .is_none(),
                fail
            );
            assert_eq!(
                std::fs::read_dir(runtime.instance_dir()).unwrap().count(),
                usize::from(!fail)
            );
        }
    }
    #[test]
    fn recovery_retry_does_not_rewrite_invalid_location_metadata() {
        let root = TestTempDir::new("nyaterm-recovery-retry");
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Installed,
            root.join("local"),
            root.join("local"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        std::fs::create_dir_all(runtime.instance_dir()).unwrap();
        let path = nyaterm_store::data_directory::location_path(runtime.local_dir());
        std::fs::write(&path, b"invalid bootstrap metadata").unwrap();
        for fail in [true, false] {
            let result = super::prepare_recovery_restart(&runtime, None, |marker| {
                assert!(marker.exists());
                if fail {
                    Err("injected launch failure".into())
                } else {
                    Ok(())
                }
            });
            assert_eq!(result.is_err(), fail);
            assert_eq!(std::fs::read(&path).unwrap(), b"invalid bootstrap metadata");
        }
    }

    #[test]
    fn failed_recovery_launch_restores_the_previous_directory_and_pending_transaction() {
        use nyaterm_store::data_directory::{prepare_recovery_selection, set_active_data_dir};
        let root = TestTempDir::new("nyaterm-recovery-choice-rollback");
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Installed,
            root.join("local"),
            root.join("local"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        std::fs::create_dir_all(runtime.instance_dir()).unwrap();
        drop(nyaterm_store::ConnectionStore::open(runtime.local_dir()).unwrap());
        set_active_data_dir(runtime.local_dir(), Some(&root.join("missing"))).unwrap();
        let previous = read_location(runtime.local_dir()).unwrap();
        let choice = prepare_recovery_selection(
            runtime.local_dir(),
            runtime.local_dir(),
            AppFlavor::current(),
        )
        .unwrap();
        let result =
            super::prepare_recovery_restart(&runtime, Some(&choice), |_| Err("injected".into()));
        assert!(result.is_err());
        assert_eq!(read_location(runtime.local_dir()).unwrap(), previous);
        assert_eq!(
            std::fs::read_dir(runtime.instance_dir()).unwrap().count(),
            0
        );
    }
}
