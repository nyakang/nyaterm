use super::DesktopController;
use crate::app_shell::GlobalStateMutation;
use crate::app_shell::ProcessStateStore;
use crate::app_shell::SharedStateDomain;
use crate::app_shell::SharedStateEvent;
use gpui::AppContext as _;
use gpui::Context;
use nyaterm_core::WorkspaceId;
use nyaterm_store::BootstrapSnapshot;
use nyaterm_store::LoadBootstrap;
use nyaterm_store::StoreOperationError;
use nyaterm_store::StoreTask;

impl DesktopController {
    pub(super) fn launch_initial_bootstrap(&mut self, cx: &mut Context<Self>) {
        let Some(task) = self.pending_bootstrap.take() else {
            return;
        };
        self.bootstrap_in_flight = true;
        self.await_process_bootstrap(task, cx);
    }

    pub(super) fn await_process_bootstrap(
        &mut self,
        task: StoreTask<BootstrapSnapshot>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let event = task.await;
            let _ = this.update(cx, |controller, cx| {
                controller.bootstrap_in_flight = false;
                match event.outcome {
                    Ok(snapshot) => controller.install_process_snapshot(snapshot, cx),
                    Err(error) => controller.report_process_bootstrap_failure(error, cx),
                }
            });
        })
        .detach();
    }

    pub(super) fn install_process_snapshot(
        &mut self,
        snapshot: BootstrapSnapshot,
        cx: &mut Context<Self>,
    ) {
        if should_enable_startup_screen_lock(
            self.process_state.is_some(),
            snapshot.settings.enable_startup_lock,
        ) {
            self.screen_locked = true;
        }

        if let Some(process_state) = self.process_state.clone() {
            let event = process_state.update(cx, |state, cx| {
                state.mutate(
                    GlobalStateMutation::ReplaceSnapshot {
                        snapshot: Box::new(snapshot),
                        domain: SharedStateDomain::All,
                    },
                    cx,
                )
            });
            self.broadcast_shared_state(event, cx);
            return;
        }
        let process_state = cx.new(|_| ProcessStateStore::new(snapshot));
        self.process_state = Some(process_state.clone());
        crate::window_chrome::request_window_decoration_refresh(cx);
        let workspace_ids = self.windows.keys().copied().collect::<Vec<_>>();
        for workspace_id in workspace_ids {
            self.deliver_process_state(workspace_id, process_state.clone(), cx);
        }
    }

    pub(super) fn broadcast_shared_state(&self, event: SharedStateEvent, cx: &mut Context<Self>) {
        self.broadcast_shared_state_except(event, None, cx);
    }

    pub(super) fn broadcast_shared_state_except(
        &self,
        event: SharedStateEvent,
        source: Option<WorkspaceId>,
        cx: &mut Context<Self>,
    ) {
        if !event.changed {
            return;
        }
        if matches!(
            event.domain,
            SharedStateDomain::Settings | SharedStateDomain::All
        ) {
            crate::window_chrome::request_window_decoration_refresh(cx);
        }
        let Some(process_state) = self.process_state.clone() else {
            return;
        };
        for (id, entry) in &self.windows {
            if Some(*id) == source {
                continue;
            }
            let shell = entry.shell.clone();
            let process_state = process_state.clone();
            let registration = self.sync_operations().followup("cloud-sync-window-refresh");
            cx.defer(move |cx| {
                let _ = shell.update(cx, |shell, cx| {
                    shell.apply_shared_state(process_state, event, cx)
                });
                drop(registration);
            });
        }
    }

    pub(crate) fn request_shared_state_refresh(
        &mut self,
        domain: SharedStateDomain,
        cx: &mut Context<Self>,
    ) {
        if self.process_quitting {
            return;
        }
        let Some(store_runtime) = self.startup.shared_store_runtime() else {
            return;
        };
        let task = match store_runtime.ui_client().try_submit(0, LoadBootstrap) {
            Ok(task) => task,
            Err(error) => {
                tracing::warn!(%error, "shared state refresh was not submitted");
                return;
            }
        };
        self.shared_refresh_generation = self.shared_refresh_generation.saturating_add(1);
        let generation = self.shared_refresh_generation;
        cx.spawn(async move |this, cx| {
            let event = task.await;
            let _ = this.update(cx, |controller, cx| match event.outcome {
                Ok(snapshot) if generation >= controller.applied_shared_refresh_generation => {
                    controller.applied_shared_refresh_generation = generation;
                    controller.apply_shared_snapshot(snapshot, domain, cx);
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(category = error.category(), "shared state refresh failed")
                }
            });
        })
        .detach();
    }

    pub(crate) fn replace_shared_snapshot(
        &mut self,
        snapshot: BootstrapSnapshot,
        domain: SharedStateDomain,
        cx: &mut Context<Self>,
    ) {
        self.shared_refresh_generation = self.shared_refresh_generation.saturating_add(1);
        self.applied_shared_refresh_generation = self.shared_refresh_generation;
        self.apply_shared_snapshot(snapshot, domain, cx);
    }

    pub(super) fn apply_shared_snapshot(
        &mut self,
        snapshot: BootstrapSnapshot,
        domain: SharedStateDomain,
        cx: &mut Context<Self>,
    ) {
        let Some(process_state) = self.process_state.clone() else {
            self.install_process_snapshot(snapshot, cx);
            return;
        };
        let event = process_state.update(cx, |state, cx| {
            state.mutate(
                GlobalStateMutation::ReplaceSnapshot {
                    snapshot: Box::new(snapshot),
                    domain,
                },
                cx,
            )
        });
        self.broadcast_shared_state(event, cx);
    }

    pub(super) fn report_process_bootstrap_failure(
        &mut self,
        error: StoreOperationError,
        cx: &mut Context<Self>,
    ) {
        for entry in self.windows.values() {
            let _ = entry
                .shell
                .update(cx, |shell, cx| shell.enter_recovery(error.clone(), cx));
        }
    }

    pub(super) fn deliver_process_state(
        &self,
        workspace_id: WorkspaceId,
        process_state: gpui::Entity<ProcessStateStore>,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.windows.get(&workspace_id) else {
            return;
        };
        let shell = entry.shell.clone();
        let handle = entry.handle;
        cx.defer(move |cx| {
            let _ = handle.update(cx, move |_, window, cx| {
                let _ = shell.update(cx, |shell, cx| {
                    shell.complete_bootstrap(process_state, window, cx)
                });
            });
        });
    }

    pub(in crate::app_shell) fn retry_process_bootstrap(&mut self, cx: &mut Context<Self>) {
        if self.bootstrap_in_flight || self.process_quitting {
            return;
        }
        let Some(store_runtime) = self.startup.shared_store_runtime() else {
            return;
        };
        match store_runtime.ui_client().try_submit(0, LoadBootstrap) {
            Ok(task) => {
                self.bootstrap_in_flight = true;
                self.await_process_bootstrap(task, cx);
            }
            Err(error) => {
                tracing::error!(%error, "process bootstrap retry was not submitted");
            }
        }
    }
}
pub(super) fn should_enable_startup_screen_lock(
    process_state_loaded: bool,
    screen_lock_enabled: bool,
) -> bool {
    !process_state_loaded && screen_lock_enabled
}
