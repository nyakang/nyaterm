use futures::channel::oneshot;
use gpui::Context;
use nyaterm_store::LoadBootstrap;

use super::DesktopController;
use crate::app_shell::configuration_replacement::ConfigurationReplacementLease;
use crate::app_shell::{GlobalStateMutation, SharedStateDomain};

impl DesktopController {
    /// Owned by the process, so closing the initiating window cannot skip refresh.
    pub(crate) fn refresh_after_configuration_replacement(
        &mut self,
        lease: ConfigurationReplacementLease,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = self.startup.shared_store_runtime() else {
            self.configuration_refresh_failed(cx);
            return;
        };
        let Ok(task) = store.ui_client().try_submit(0, LoadBootstrap) else {
            self.configuration_refresh_failed(cx);
            return;
        };
        let registration = self
            .sync_operations()
            .followup("configuration-replacement-refresh");
        self.shared_refresh_generation = self.shared_refresh_generation.saturating_add(1);
        let generation = self.shared_refresh_generation;
        cx.spawn(async move |this, cx| {
            let event = task.await;
            let Ok(mut snapshot) = event.outcome else {
                let _ = this.update(cx, |controller, cx| {
                    controller.configuration_refresh_failed(cx)
                });
                return;
            };
            let refreshes = this.update(cx, |controller, cx| {
                if generation < controller.applied_shared_refresh_generation {
                    if let Some(state) = &controller.process_state {
                        snapshot = state.read(cx).snapshot().clone();
                    }
                } else {
                    controller.applied_shared_refresh_generation = generation;
                }
                if controller.process_state.is_none() {
                    controller.install_process_snapshot(snapshot.clone(), cx);
                }
                let state = controller
                    .process_state
                    .clone()
                    .expect("installed process snapshot");
                let event = state.update(cx, |state, cx| {
                    state.mutate(
                        GlobalStateMutation::ReplaceSnapshot {
                            snapshot: Box::new(snapshot),
                            domain: SharedStateDomain::All,
                        },
                        cx,
                    )
                });
                crate::window_chrome::request_window_decoration_refresh(cx);
                controller
                    .windows
                    .values()
                    .map(|entry| {
                        let (send, receive) = oneshot::channel();
                        let shell = entry.shell.clone();
                        let state = state.clone();
                        let lease = lease.clone();
                        cx.defer(move |cx| {
                            // A destroyed window no longer owns a catalog to refresh.
                            let _ = shell
                                .update(cx, |shell, cx| shell.apply_shared_state(state, event, cx));
                            let _ = send.send(());
                            drop(lease);
                        });
                        receive
                    })
                    .collect::<Vec<_>>()
            });
            if let Ok(refreshes) = refreshes {
                let results = futures::future::join_all(refreshes).await;
                if results.iter().all(Result::is_ok) {
                    lease.refresh_completed();
                }
            }
            drop(registration);
        })
        .detach();
    }
    fn configuration_refresh_failed(&self, cx: &mut Context<Self>) {
        for entry in self.windows.values() {
            let _ = entry.shell.update(cx, |shell, cx| {
                if let Some(app) = &shell.app {
                    app.update(cx, |app, cx| {
                        app.report_configuration_refresh_failure(cx);
                    });
                }
            });
        }
    }
}

#[cfg(test)]
impl DesktopController {
    pub(crate) fn attach_configuration_test_window(
        &mut self,
        id: nyaterm_core::WorkspaceId,
        app: gpui::Entity<crate::features::NyaTermApp>,
        cx: &mut Context<Self>,
    ) {
        use gpui::{AppContext as _, px, size};
        let controller = cx.entity();
        let startup = self.startup.for_workspace(id);
        let runtime = self.runtime.clone();
        let hub = self.session_hub.clone();
        let bounds = gpui::Bounds::centered(None, size(px(800.), px(600.)), cx);
        let handle = cx
            .open_window(
                gpui::WindowOptions {
                    window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|cx| {
                        crate::app_shell::AppShell::new(
                            runtime, None, startup, id, controller, hub, cx,
                        )
                    })
                },
            )
            .unwrap();
        let shell = handle.root(cx).unwrap();
        shell.update(cx, |shell, _| shell.app = Some(app));
        self.windows.insert(
            id,
            super::WorkspaceWindow {
                handle: handle.into(),
                shell: shell.downgrade(),
            },
        );
    }
}
