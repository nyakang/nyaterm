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
            prepare_restart(&runtime, &choice, |marker| {
                let executable = std::env::current_exe()
                    .map_err(|e| format!("could not locate executable: {e}"))?;
                Command::new(executable)
                    .env(nyaterm_core::RELAUNCH_HANDOFF_ENV, marker)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|e| format!("could not launch restart: {e}"))?;
                Ok(())
            })
        })
    }
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
        };
        let mut state = super::DataDirectoryRestartState {
            choice: Some(choice.clone()),
            launching: false,
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
}
