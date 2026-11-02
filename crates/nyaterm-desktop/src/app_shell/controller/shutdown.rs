use super::DesktopController;
use crate::features::WorkspaceCloseSnapshot;
use crate::features::shell::tray::show_window as show_tray_window;
use gpui::AppContext as _;
use gpui::Context;
use gpui::Task;
use nyaterm_core::DeviceWindowManifest;
use nyaterm_core::DeviceWindowState;
use nyaterm_core::MainWindowState;
use nyaterm_core::WorkspaceId;
use nyaterm_store::FlushBarrier;
use nyaterm_store::StoreDomain;
use nyaterm_store::StoreSubmitError;
use nyaterm_store::StoreTask;
use nyaterm_store::store_request;

impl DesktopController {
    pub fn submit_window_state(
        &mut self,
        workspace_id: WorkspaceId,
        state: MainWindowState,
        generation: u64,
    ) -> Result<StoreTask<()>, StoreSubmitError> {
        if let Some(existing) = self
            .device_windows
            .windows
            .iter_mut()
            .find(|entry| entry.workspace_id == workspace_id)
        {
            existing.window = state;
        } else {
            self.device_windows.windows.push(DeviceWindowState {
                workspace_id,
                window: state,
                extra: Default::default(),
            });
        }
        if !self.device_windows.window_order.contains(&workspace_id) {
            self.device_windows.window_order.push(workspace_id);
        }
        self.device_windows.most_recent_workspace_id = self.most_recent_workspace_id;
        let store = self
            .startup
            .shared_store_runtime()
            .expect("desktop controller requires a store runtime")
            .ui_client();
        let device_windows = self.device_windows.clone();
        let recent = self.most_recent_workspace_id;
        store.try_submit(
            generation,
            store_request(StoreDomain::WindowState, move |store| {
                let mut manifest = store.load_workspace_restore_manifest()?;
                if let Some(recent) = recent {
                    if !manifest
                        .workspaces
                        .iter()
                        .any(|workspace| workspace.id == recent)
                    {
                        manifest
                            .workspaces
                            .push(nyaterm_core::WorkspaceRestoreState::empty(recent));
                    }
                    manifest.most_recent_workspace_id = Some(recent);
                }
                store.save_restore_manifests_atomically(&manifest, &device_windows)
            }),
        )
    }

    pub fn request_close_workspace(
        &mut self,
        workspace_id: WorkspaceId,
        persistence_task: StoreTask<()>,
        cx: &mut Context<Self>,
    ) -> Result<(), StoreSubmitError> {
        if !self.windows.contains_key(&workspace_id) {
            return Ok(());
        }
        if self.process_quitting || !self.closing_workspaces.is_empty() {
            return Err(StoreSubmitError::ShuttingDown);
        }
        self.closing_workspaces.insert(workspace_id);
        self.request_native_menu_refresh(cx);
        self.tab_drag.cancel_workspace(workspace_id);
        cx.spawn(async move |this, cx| {
            let persistence = persistence_task.await;
            if let Err(error) = persistence.outcome {
                let _ = this.update(cx, |controller, cx| {
                    controller.report_workspace_close_failure(workspace_id, error.to_string(), cx)
                });
                return;
            }

            let barrier = match this.update(cx, |controller, _| {
                controller
                    .startup
                    .shared_store_runtime()
                    .expect("desktop controller requires a store runtime")
                    .ui_client()
                    .try_submit(0, FlushBarrier)
            }) {
                Ok(Ok(task)) => task,
                Ok(Err(error)) => {
                    let _ = this.update(cx, |controller, cx| {
                        controller.report_workspace_close_failure(
                            workspace_id,
                            error.to_string(),
                            cx,
                        )
                    });
                    return;
                }
                Err(_) => return,
            };
            let barrier = barrier.await;
            if let Err(error) = barrier.outcome {
                let _ = this.update(cx, |controller, cx| {
                    controller.report_workspace_close_failure(workspace_id, error.to_string(), cx)
                });
                return;
            }

            let submitted = this.update(cx, |controller, _| {
                controller.submit_closed_workspace_manifests(workspace_id)
            });
            let (task, device_windows, next_recent) = match submitted {
                Ok(Ok(submitted)) => submitted,
                Ok(Err(error)) => {
                    let _ = this.update(cx, |controller, cx| {
                        controller.report_workspace_close_failure(
                            workspace_id,
                            error.to_string(),
                            cx,
                        )
                    });
                    return;
                }
                Err(_) => return,
            };
            let event = task.await;
            if let Err(error) = event.outcome {
                let _ = this.update(cx, |controller, cx| {
                    controller.report_workspace_close_failure(workspace_id, error.to_string(), cx)
                });
                return;
            }
            let _ = this.update(cx, |controller, cx| {
                controller.finish_workspace_close(workspace_id, device_windows, next_recent, cx)
            });
        })
        .detach();
        Ok(())
    }

    pub(super) fn submit_closed_workspace_manifests(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<(StoreTask<()>, DeviceWindowManifest, Option<WorkspaceId>), StoreSubmitError> {
        let mut device_windows = self.device_windows.clone();
        device_windows
            .windows
            .retain(|entry| entry.workspace_id != workspace_id);
        device_windows.window_order.retain(|id| *id != workspace_id);
        let next_recent = next_recent_after_close(
            self.most_recent_workspace_id,
            workspace_id,
            &device_windows.window_order,
            |id| self.windows.contains_key(&id),
        );
        device_windows.most_recent_workspace_id = next_recent;
        let store = self
            .startup
            .shared_store_runtime()
            .expect("desktop controller requires a store runtime")
            .ui_client();
        let persisted_device_windows = device_windows.clone();
        let task = store.try_submit(
            0,
            store_request(StoreDomain::Sessions, move |store| {
                let mut manifest = store.load_workspace_restore_manifest()?;
                manifest
                    .workspaces
                    .retain(|workspace| workspace.id != workspace_id);
                manifest.most_recent_workspace_id = next_recent
                    .filter(|id| {
                        manifest
                            .workspaces
                            .iter()
                            .any(|workspace| workspace.id == *id)
                    })
                    .or_else(|| manifest.workspaces.last().map(|workspace| workspace.id));
                store.save_restore_manifests_atomically(&manifest, &persisted_device_windows)
            }),
        )?;
        Ok((task, device_windows, next_recent))
    }

    pub(super) fn finish_workspace_close(
        &mut self,
        workspace_id: WorkspaceId,
        device_windows: DeviceWindowManifest,
        next_recent: Option<WorkspaceId>,
        cx: &mut Context<Self>,
    ) {
        self.tab_drag.close(workspace_id);
        let Some(entry) = self.windows.remove(&workspace_id) else {
            self.closing_workspaces.remove(&workspace_id);
            return;
        };
        let cleanup = entry
            .shell
            .update(cx, |shell, cx| {
                shell.app.as_ref().map(|app| {
                    app.update(cx, |app, cx| {
                        app.shutdown_workspace_sessions();
                        app.shutdown_blocking_jobs(cx)
                    })
                })
            })
            .ok()
            .flatten();
        self.release_settings_owner(workspace_id, cx);
        self.pending_tab_moves.remove(&workspace_id);
        cx.spawn(async move |this, cx| {
            if let Some(cleanup) = cleanup {
                cleanup.await;
            }
            let _ = this.update(cx, |controller, cx| {
                controller.closing_workspaces.remove(&workspace_id);
                controller.close_failed_tab_windows(cx);
                cx.defer(move |cx| {
                    let _ = entry
                        .handle
                        .update(cx, |_, window, _| window.remove_window());
                });
            });
        })
        .detach();
        self.most_recent_workspace_id = next_recent;
        self.request_native_menu_refresh(cx);
        self.device_windows = device_windows;
    }

    pub(super) fn report_workspace_close_failure(
        &mut self,
        workspace_id: WorkspaceId,
        message: String,
        cx: &mut Context<Self>,
    ) {
        self.closing_workspaces.remove(&workspace_id);
        if let Some(entry) = self.windows.get(&workspace_id) {
            let _ = entry.shell.update(cx, |shell, cx| {
                shell.finish_workspace_close_failure(message, cx)
            });
        }
        self.close_failed_tab_windows(cx);
    }

    pub(super) fn defer_workspace_close_failure(
        &self,
        workspace_id: WorkspaceId,
        message: String,
        cx: &mut Context<Self>,
    ) {
        let controller = cx.entity().downgrade();
        cx.defer(move |cx| {
            let _ = controller.update(cx, |controller, cx| {
                controller.report_workspace_close_failure(workspace_id, message, cx)
            });
        });
    }

    pub fn request_close_unready_workspace(
        &mut self,
        workspace_id: WorkspaceId,
        cx: &mut Context<Self>,
    ) {
        if self.process_quitting || !self.closing_workspaces.is_empty() {
            return;
        }
        if self.windows.len() == 1 {
            self.request_quit_without_ready(cx);
            return;
        }
        let Some(store_runtime) = self.startup.shared_store_runtime() else {
            self.defer_workspace_close_failure(
                workspace_id,
                "The storage runtime is unavailable".to_string(),
                cx,
            );
            return;
        };
        let persistence_task = store_runtime.ui_client().try_submit(0, FlushBarrier);
        match persistence_task {
            Ok(task) => {
                if let Err(error) = self.request_close_workspace(workspace_id, task, cx) {
                    self.defer_workspace_close_failure(workspace_id, error.to_string(), cx);
                }
            }
            Err(error) => {
                self.defer_workspace_close_failure(workspace_id, error.to_string(), cx);
            }
        }
    }

    pub fn request_quit(&mut self, cx: &mut Context<Self>) {
        self.request_native_menu_refresh(cx);
        self.tab_drag.cancel();
        if self.process_quitting {
            return;
        }
        let ready = self
            .device_windows
            .window_order
            .iter()
            .rev()
            .copied()
            .filter(|id| {
                self.windows.get(id).is_some_and(|entry| {
                    entry
                        .shell
                        .update(cx, |shell, _| shell.can_coordinate_quit())
                        .unwrap_or(false)
                })
            })
            .collect::<Vec<_>>();
        // Route process quit through the settings owner so its theme draft is
        // handled first; the dirty-draft gate then visits any other window that
        // holds its own settings tab draft.
        let workspace_id = self
            .settings_owners
            .iter()
            .rev()
            .copied()
            .find(|id| ready.contains(id))
            .or_else(|| {
                ready.iter().copied().find(|id| {
                    self.windows.get(id).is_some_and(|entry| {
                        entry
                            .shell
                            .update(cx, |shell, cx| {
                                shell
                                    .app
                                    .as_ref()
                                    .is_some_and(|app| app.read(cx).has_live_sessions())
                            })
                            .unwrap_or(false)
                    })
                })
            })
            .or_else(|| {
                self.most_recent_workspace_id
                    .filter(|id| ready.contains(id))
            })
            .or_else(|| ready.first().copied());
        let Some(workspace_id) = workspace_id else {
            self.request_quit_without_ready(cx);
            return;
        };
        if let Some(entry) = self.windows.get(&workspace_id) {
            let shell = entry.shell.clone();
            let handle = entry.handle;
            cx.defer(move |cx| {
                let _ = handle.update(cx, move |_, window, cx| {
                    let _ = shell.update(cx, |shell, cx| {
                        show_tray_window(window, cx);
                        shell.request_application_quit(window, cx)
                    });
                });
            });
        }
    }

    pub fn live_session_count_excluding(
        &self,
        excluded_workspace_id: WorkspaceId,
        cx: &mut Context<Self>,
    ) -> usize {
        self.windows
            .iter()
            .filter(|(workspace_id, _)| **workspace_id != excluded_workspace_id)
            .filter_map(|(_, entry)| {
                entry
                    .shell
                    .update(cx, |shell, cx| {
                        shell
                            .app
                            .as_ref()
                            .map(|app| app.read(cx).live_session_count())
                    })
                    .ok()
                    .flatten()
            })
            .sum()
    }

    pub fn request_quit_without_ready(&mut self, cx: &mut Context<Self>) {
        if !self.begin_process_quit() {
            return;
        }
        let operations = self.sync_operations();
        let started_at = std::time::Instant::now();
        tracing::info!(
            pid = std::process::id(),
            stage = "sync",
            "shutdown stage started"
        );
        // A loading shell can request quit while it is already leased by GPUI.
        let shells = self
            .windows
            .values()
            .map(|entry| entry.shell.clone())
            .collect::<Vec<_>>();
        cx.defer(move |cx| {
            for shell in shells {
                let _ = shell.update(cx, |shell, cx| shell.enter_flushing(cx));
            }
        });
        cx.spawn(async move |this, cx| {
            operations.wait_for_idle().await;
            tracing::info!(
                pid = std::process::id(),
                stage = "sync",
                elapsed_ms = started_at.elapsed().as_millis(),
                "shutdown stage finished"
            );
            let _ = this.update(cx, |controller, cx| {
                controller.quit_without_ready_after_sync(cx)
            });
        })
        .detach();
    }

    fn quit_without_ready_after_sync(&mut self, cx: &mut Context<Self>) {
        let Some(store_runtime) = self.startup.shared_store_runtime() else {
            self.quit_after_workspace_shutdown(cx);
            return;
        };
        store_runtime.begin_shutdown();
        let barrier = store_runtime
            .ui_client()
            .try_submit_shutdown(0, FlushBarrier);
        let workspace_id = self.most_recent_workspace_id;
        match barrier {
            Ok(barrier) => {
                cx.spawn(async move |this, cx| {
                    let outcome = barrier.await.outcome;
                    let _ = this.update(cx, |controller, cx| match outcome {
                        Ok(()) => {
                            controller.quit_after_workspace_shutdown(cx);
                        }
                        Err(error) => {
                            store_runtime.resume_after_failed_shutdown();
                            controller.cancel_process_quit();
                            if let Some(id) = workspace_id {
                                controller.report_workspace_close_failure(
                                    id,
                                    error.to_string(),
                                    cx,
                                );
                            }
                        }
                    });
                })
                .detach();
            }
            Err(error) => {
                store_runtime.resume_after_failed_shutdown();
                self.cancel_process_quit();
                if let Some(id) = workspace_id {
                    self.report_workspace_close_failure(id, error.to_string(), cx);
                }
            }
        }
    }

    /// Run each workspace's unsaved-draft gate before releasing any process
    /// resources. Covers editor documents and all per-window settings drafts.
    pub fn defer_quit_for_dirty_editors(
        &mut self,
        current: WorkspaceId,
        cx: &mut Context<Self>,
    ) -> bool {
        let target = self.windows.iter().find_map(|(id, entry)| {
            if *id == current {
                return None;
            }
            let dirty = entry
                .shell
                .update(cx, |shell, cx| {
                    shell.app.as_ref().is_some_and(|app| {
                        let app = app.read(cx);
                        app.has_unconfirmed_editor_changes()
                            || app.has_unconfirmed_settings_changes()
                    })
                })
                .unwrap_or(false);
            dirty.then(|| (entry.shell.clone(), entry.handle))
        });
        let Some((shell, handle)) = target else {
            return false;
        };
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, cx| {
                let _ = shell.update(cx, |shell, cx| {
                    show_tray_window(window, cx);
                    shell.quit_requested = true;
                    if let Some(app) = &shell.app {
                        app.update(cx, |app, cx| {
                            app.handle_window_close_request_with_count(0, window, cx)
                        });
                    }
                });
            });
        });
        true
    }

    pub fn cancel_editor_quit_gates(&mut self, cx: &mut Context<Self>) {
        let shells = self
            .windows
            .values()
            .map(|entry| entry.shell.clone())
            .collect::<Vec<_>>();
        cx.defer(move |cx| {
            for shell in shells {
                let _ = shell.update(cx, |shell, cx| {
                    shell.quit_requested = false;
                    if let Some(app) = &shell.app {
                        app.update(cx, |app, _| app.cancel_editor_exit_approval());
                    }
                });
            }
        });
    }

    pub fn prepare_other_workspaces_for_quit(
        &mut self,
        current_workspace_id: WorkspaceId,
        cx: &mut Context<Self>,
    ) -> Result<Vec<StoreTask<()>>, StoreSubmitError> {
        let mut tasks = Vec::new();
        for (workspace_id, entry) in &self.windows {
            if *workspace_id == current_workspace_id {
                continue;
            }
            let task = entry
                .shell
                .update(cx, |shell, cx| shell.submit_process_quit_persistence(cx))
                .map_err(|_| StoreSubmitError::Disconnected)??;
            if let Some(task) = task {
                tasks.push(task);
            }
        }
        Ok(tasks)
    }

    pub(crate) fn submit_process_restore_snapshot(
        &mut self,
        current_workspace_id: WorkspaceId,
        current_snapshot: WorkspaceCloseSnapshot,
        current_state: Option<MainWindowState>,
        cx: &mut Context<Self>,
    ) -> Result<StoreTask<()>, StoreSubmitError> {
        let mut device_windows = self.device_windows.clone();
        let mut workspace_snapshots = vec![current_snapshot];
        for (workspace_id, entry) in &self.windows {
            if *workspace_id != current_workspace_id
                && let Some(app) = entry
                    .shell
                    .update(cx, |shell, _| shell.app.clone())
                    .ok()
                    .flatten()
            {
                workspace_snapshots
                    .push(app.update(cx, |app, _| app.capture_workspace_close_snapshot()));
            }
            let state = if *workspace_id == current_workspace_id {
                current_state.clone()
            } else {
                entry
                    .shell
                    .update(cx, |shell, _| shell.main_window_state.latest_for_shutdown())
                    .ok()
                    .flatten()
            };
            if let Some(state) = state {
                if let Some(existing) = device_windows
                    .windows
                    .iter_mut()
                    .find(|window| window.workspace_id == *workspace_id)
                {
                    existing.window = state;
                } else {
                    device_windows.windows.push(DeviceWindowState {
                        workspace_id: *workspace_id,
                        window: state,
                        extra: Default::default(),
                    });
                }
            }
            if !device_windows.window_order.contains(workspace_id) {
                device_windows.window_order.push(*workspace_id);
            }
        }
        device_windows.most_recent_workspace_id = self.most_recent_workspace_id;
        let recent = self.most_recent_workspace_id;
        let store = self
            .startup
            .shared_store_runtime()
            .expect("desktop controller requires a store runtime")
            .ui_client();
        store.try_submit_shutdown(
            u64::MAX - 1,
            store_request(StoreDomain::Shutdown, move |store| {
                let mut manifest = store.load_workspace_restore_manifest()?;
                for snapshot in workspace_snapshots {
                    let workspace = if let Some(index) = manifest
                        .workspaces
                        .iter()
                        .position(|workspace| workspace.id == snapshot.workspace_id)
                    {
                        &mut manifest.workspaces[index]
                    } else {
                        manifest
                            .workspaces
                            .push(nyaterm_core::WorkspaceRestoreState::empty(
                                snapshot.workspace_id,
                            ));
                        manifest.workspaces.last_mut().expect("workspace inserted")
                    };
                    snapshot.apply_to(workspace);
                }
                manifest.most_recent_workspace_id = recent.filter(|id| {
                    manifest
                        .workspaces
                        .iter()
                        .any(|workspace| workspace.id == *id)
                });
                store.save_restore_manifests_atomically(&manifest, &device_windows)
            }),
        )
    }

    pub(super) fn quit_after_workspace_shutdown(&mut self, cx: &mut Context<Self>) {
        let mut tasks = self.shutdown_workspaces_except(None, cx);
        tasks.push(self.shutdown_plugins(cx));
        cx.spawn(async move |this, cx| {
            for task in tasks {
                task.await;
            }
            let _ = this.update(cx, |controller, cx| controller.quit_after_process_jobs(cx));
        })
        .detach();
    }

    pub(in crate::app_shell) fn quit_after_process_jobs(&mut self, cx: &mut Context<Self>) {
        // Sync results and history have been consumed before scheduler cancellation.
        let Some(cleanup) = self
            .update_store
            .update(cx, |store, _| store.begin_shutdown())
        else {
            return;
        };
        let started_at = std::time::Instant::now();
        tracing::info!(
            pid = std::process::id(),
            stage = "process_jobs",
            "shutdown stage started"
        );
        let cleanup = cx.background_spawn(async move {
            cleanup();
        });
        cx.spawn(async move |this, cx| {
            cleanup.await;
            let _ = this.update(cx, |_, cx| {
                tracing::info!(
                    pid = std::process::id(),
                    stage = "process_jobs",
                    elapsed_ms = started_at.elapsed().as_millis(),
                    "shutdown stage finished"
                );
                cx.quit();
            });
        })
        .detach();
    }

    pub fn shutdown_other_workspaces(
        &mut self,
        excluded_workspace_id: WorkspaceId,
        cx: &mut Context<Self>,
    ) -> Vec<Task<()>> {
        self.shutdown_workspaces_except(Some(excluded_workspace_id), cx)
    }

    pub(super) fn shutdown_workspaces_except(
        &mut self,
        excluded_workspace_id: Option<WorkspaceId>,
        cx: &mut Context<Self>,
    ) -> Vec<Task<()>> {
        let mut tasks = Vec::new();
        for (workspace_id, entry) in &self.windows {
            if excluded_workspace_id == Some(*workspace_id) {
                continue;
            }
            if let Ok(Some(task)) = entry.shell.update(cx, |shell, cx| {
                shell.app.as_ref().map(|app| {
                    app.update(cx, |app, cx| {
                        app.shutdown_workspace_sessions();
                        app.shutdown_blocking_jobs(cx)
                    })
                })
            }) {
                tasks.push(task);
            }
        }
        tasks
    }
}
pub(super) fn next_recent_after_close(
    current: Option<WorkspaceId>,
    closing: WorkspaceId,
    window_order: &[WorkspaceId],
    is_open: impl Fn(WorkspaceId) -> bool,
) -> Option<WorkspaceId> {
    current
        .filter(|id| *id != closing && is_open(*id))
        .or_else(|| {
            window_order
                .iter()
                .rev()
                .copied()
                .find(|id| *id != closing && is_open(*id))
        })
}
