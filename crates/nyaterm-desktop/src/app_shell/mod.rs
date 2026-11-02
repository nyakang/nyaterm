//! Root GPUI shell boundary.

use nyaterm_core::activation::ParsedActivationRequest;

mod controller;
pub(crate) use controller::native_menu;
mod process_state;
pub(crate) mod session_hub;
pub(crate) mod tab_drag;
mod window_state;

use self::window_state::{
    MAIN_WINDOW_STATE_SAVE_DEBOUNCE, MainWindowStateController, capture_main_window_state,
};
pub use controller::{DesktopController, DesktopControllerGlobal};
pub(crate) use process_state::{
    GlobalStateMutation, ProcessStateStore, SettingsDraftRevisions, SharedStateDomain,
    SharedStateEvent, WorkspaceInitSnapshot,
};
pub use session_hub::SessionHub;
pub use window_state::{AppShellStartup, MainWindowPlacement};

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use gpui::{
    AnimationExt, AnyElement, App, AppContext, Context, Entity, InteractiveElement, IntoElement,
    KeyBinding, MouseButton, ParentElement, Render, Styled, Subscription, WeakEntity, Window,
    actions, div, prelude::FluentBuilder, px, rgb, svg,
};
use nyaterm_core::{
    AppRuntime, DiagnosticsExportOptions, DiagnosticsRuntimeSnapshot, WorkspaceId,
    export_diagnostics_archive,
};
use nyaterm_store::{FlushBarrier, StoreOperationError, StoreRuntime, StoreSubmitError, StoreTask};
use nyaterm_ui::{NyaAppMenu, NyaAppMenuBar, NyaButton, NyaButtonVariant};
use rust_i18n::t;

use crate::{
    entities::{OverlayStore, StartupRestoreStore, UiStoreHandles},
    features::{AppLifecycleEvent, NyaTermApp, NyaTermProcessEntities, NyaTermStoreClients},
};

const SHUTDOWN_STATUS_DELAY: Duration = Duration::from_millis(200);

actions!(
    nyaterm_native_menu,
    [
        NativeHide,
        NativeHideOthers,
        NativeShowAll,
        NativeNewWindow,
        NativeQuit
    ]
);

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([native_new_window_key_binding()]);
    native_menu::init(cx);
}

fn native_new_window_key_binding() -> KeyBinding {
    let keystroke = if cfg!(target_os = "macos") {
        "cmd-shift-n"
    } else {
        "ctrl-shift-n"
    };
    KeyBinding::new(keystroke, NativeNewWindow, None)
}

#[allow(dead_code)]
pub struct AppShell {
    runtime: AppRuntime,
    workspace_id: WorkspaceId,
    controller: Entity<DesktopController>,
    session_hub: Entity<SessionHub>,
    quit_requested: bool,
    lifecycle: AppShellLifecycle,
    flushing_view_ready: bool,
    app: Option<Entity<NyaTermApp>>,
    store_runtime: Option<StoreRuntime>,
    workspace_seed: Option<nyaterm_core::WorkspaceRestoreState>,
    startup_restore: Entity<StartupRestoreStore>,
    overlays: Entity<OverlayStore>,
    pending_activations: VecDeque<ParsedActivationRequest>,
    pending_process_quit_tasks: Vec<StoreTask<()>>,
    main_window_state: MainWindowStateController,
    _subscriptions: Vec<Subscription>,
}

enum AppShellLifecycle {
    Loading,
    Recovery(RecoveryState),
    Ready,
    Flushing,
    FlushFailed(String),
}

struct RecoveryState {
    category: String,
    message: String,
    diagnostics_status: Option<String>,
}

impl AppShell {
    pub fn new(
        runtime: AppRuntime,
        initial_activation: Option<ParsedActivationRequest>,
        startup: AppShellStartup,
        workspace_id: WorkspaceId,
        controller: Entity<DesktopController>,
        session_hub: Entity<SessionHub>,
        cx: &mut Context<Self>,
    ) -> Self {
        let startup_restore = cx.new(|_| StartupRestoreStore::default());
        let overlays = cx.new(|_| OverlayStore::default());

        // Do not observe UI stores for parent notify: AppShell only hosts the
        // NyaTermApp entity, and NyaTermApp already cx.notify()s on visual dirty.
        // Store observe → AppShell notify was amplifying every snapshot publish
        // into an extra shell paint (connect bursts, sideband heartbeats, drag).
        let subscriptions = Vec::new();
        let lifecycle = startup
            .recovery
            .map(|recovery| {
                AppShellLifecycle::Recovery(RecoveryState {
                    category: recovery.category,
                    message: recovery.message,
                    diagnostics_status: None,
                })
            })
            .unwrap_or(AppShellLifecycle::Loading);

        Self {
            runtime,
            workspace_id,
            controller,
            session_hub,
            quit_requested: false,
            lifecycle,
            flushing_view_ready: true,
            app: None,
            store_runtime: startup.store_runtime,
            workspace_seed: startup.workspace_seed,
            startup_restore,
            overlays,
            pending_activations: initial_activation.into_iter().collect(),
            pending_process_quit_tasks: Vec::new(),
            main_window_state: MainWindowStateController::default(),
            _subscriptions: subscriptions,
        }
    }

    pub fn start_after_window_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let workspace_id = self.workspace_id;
        let controller = self.controller.clone();
        let activation_subscription = cx.observe_window_activation(window, move |_, window, cx| {
            if window.is_window_active() {
                controller.update(cx, |controller, _| controller.mark_active(workspace_id));
            }
            native_menu::request_refresh(cx);
        });
        self._subscriptions.push(activation_subscription);
        if cfg!(target_os = "macos") {
            let subscription = cx.observe_self(|_, cx| native_menu::request_refresh(cx));
            self._subscriptions.push(subscription);
        }
        self.start_main_window_state_persistence(window, cx);
    }

    fn start_main_window_state_persistence(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.record_main_window_state(window, cx);
        let subscription = cx.observe_window_bounds(window, |this, window, cx| {
            this.record_main_window_state(window, cx);
        });
        self._subscriptions.push(subscription);
    }

    fn record_main_window_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = capture_main_window_state(window, cx) else {
            return;
        };
        self.main_window_state.record(state);
        self.schedule_main_window_state_save(window, cx);
    }

    fn schedule_main_window_state_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.main_window_state.arm_timer() {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(MAIN_WINDOW_STATE_SAVE_DEBOUNCE)
                .await;
            let pending = this
                .update(cx, |this, cx| {
                    let (generation, state) = this.main_window_state.take_debounced_save()?;
                    match this.controller.update(cx, |controller, _| {
                        controller.submit_window_state(this.workspace_id, state, generation)
                    }) {
                        Ok(task) => Some((generation, task)),
                        Err(error) => {
                            tracing::warn!(category = %error, "main window state save was not submitted");
                            None
                        }
                    }
                })
                .ok()
                .flatten();
            let Some((generation, task)) = pending else {
                return;
            };
            let event = task.await;
            let succeeded = event.outcome.is_ok();
            if let Err(error) = event.outcome {
                tracing::warn!(
                    category = error.category(),
                    "main window state could not be saved"
                );
            }
            let _ = this.update(cx, |this, _| {
                this.main_window_state
                    .complete_save(generation, succeeded);
            });
        })
        .detach();
    }

    pub(super) fn receive_activation_direct(
        &mut self,
        request: ParsedActivationRequest,
        cx: &mut Context<Self>,
    ) -> bool {
        if matches!(self.lifecycle, AppShellLifecycle::Flushing) {
            return false;
        }
        if matches!(self.lifecycle, AppShellLifecycle::Ready) {
            if let Some(app) = &self.app {
                app.update(cx, |app, cx| app.handle_activation(request, cx));
                return true;
            }
        } else {
            self.pending_activations.push_back(request);
            return true;
        }
        false
    }

    fn drain_pending_activations(&mut self, cx: &mut Context<Self>) {
        let Some(app) = self.app.clone() else {
            return;
        };
        while let Some(request) = self.pending_activations.pop_front() {
            app.update(cx, |app, cx| app.handle_activation(request, cx));
        }
    }

    fn begin_bootstrap(&mut self, cx: &mut Context<Self>) {
        self.app = None;
        self.lifecycle = AppShellLifecycle::Loading;
        let controller = self.controller.clone();
        cx.defer(move |cx| {
            controller.update(cx, |controller, cx| controller.retry_process_bootstrap(cx));
        });
    }

    pub(super) fn complete_bootstrap(
        &mut self,
        process_state: Entity<ProcessStateStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(
            self.lifecycle,
            AppShellLifecycle::Flushing | AppShellLifecycle::Ready
        ) {
            return;
        }
        let Some(store_runtime) = &self.store_runtime else {
            self.lifecycle = AppShellLifecycle::Recovery(RecoveryState {
                category: "runtime_missing".to_string(),
                message: "storage runtime disappeared during bootstrap".to_string(),
                diagnostics_status: None,
            });
            cx.notify();
            return;
        };
        let mut workspace_init = process_state.read(cx).workspace_init(self.workspace_id);
        if workspace_init.state.is_none() {
            workspace_init.state = self.workspace_seed.take();
        }
        let workspace_revision = if let Some(workspace) = workspace_init.state.as_ref() {
            self.startup_restore.update(cx, |store, _| {
                store.set_loaded_window_layouts(
                    workspace.sessions.terminal_window_layout.clone(),
                    workspace.sessions.workspace_pane_layout.clone(),
                );
            });
            workspace.revision
        } else {
            self.startup_restore
                .update(cx, |store, _| store.set_loaded_window_layouts(None, None));
            0
        };
        let stores = UiStoreHandles {
            startup_restore: self.startup_restore.clone(),
            overlays: self.overlays.clone(),
        };
        let update_store = self.controller.read(cx).update_store();
        let plugin_process = self.controller.read(cx).plugin_process();
        let sync_operations = self.controller.read(cx).sync_operations();
        let app = cx.new(|cx| {
            let session_hub = self.session_hub.read(cx).clone();
            NyaTermApp::from_bootstrap(
                self.runtime.clone(),
                stores,
                NyaTermProcessEntities::new(
                    process_state,
                    update_store.clone(),
                    plugin_process,
                    sync_operations,
                ),
                workspace_init,
                NyaTermStoreClients::new(
                    store_runtime.ui_client(),
                    store_runtime.blocking_client(),
                ),
                session_hub,
                cx,
            )
        });
        let title_menu_bar =
            (!cfg!(target_os = "macos")).then(|| build_title_menu_bar(app.downgrade(), cx));
        let update_menu_bar = title_menu_bar.clone();
        let screen_locked = self.controller.read(cx).screen_locked();
        app.update(cx, |app, cx| {
            app.set_workspace_identity(self.workspace_id, workspace_revision);
            app.set_desktop_controller(self.controller.downgrade());
            if let Some(bar) = title_menu_bar {
                app.set_title_menu_bar(bar);
            }
            app.start_shell_environment_preload(cx);
            if screen_locked {
                app.apply_shared_screen_lock(true, window, cx);
            }
        });
        let shutdown_subscription = cx.subscribe(&app, |_, _, event: &AppLifecycleEvent, cx| {
            let shell = cx.weak_entity();
            let event = *event;
            // The app can emit this while AppShell is already in a close callback.
            cx.defer(move |cx| {
                let _ = shell.update(cx, |this, cx| match event {
                    AppLifecycleEvent::ShutdownRequested => {
                        if this.quit_requested || this.controller.read(cx).workspace_count() == 1 {
                            if this.controller.update(cx, |controller, cx| {
                                controller.defer_quit_for_dirty_editors(this.workspace_id, cx)
                            }) {
                                return;
                            }
                            if !this
                                .controller
                                .update(cx, |controller, _| controller.begin_process_quit())
                            {
                                return;
                            }
                            this.request_close(cx);
                        } else {
                            let Some(app) = this.app.clone() else { return };
                            let persistence_task = match app.update(cx, |app, cx| {
                                app.submit_shutdown_persistence(false).inspect_err(|error| {
                                    app.report_close_save_failed(error.to_string(), cx);
                                })
                            }) {
                                Ok(task) => task,
                                Err(_) => return,
                            };
                            this.enter_flushing(cx);
                            let workspace_id = this.workspace_id;
                            if let Err(error) = this.controller.update(cx, |controller, cx| {
                                controller.request_close_workspace(
                                    workspace_id,
                                    persistence_task,
                                    cx,
                                )
                            }) {
                                tracing::error!(%error, "could not close workspace");
                                this.lifecycle = AppShellLifecycle::FlushFailed(error.to_string());
                                app.update(cx, |app, cx| {
                                    app.report_close_save_failed(error.to_string(), cx)
                                });
                            }
                        }
                    }
                    AppLifecycleEvent::NewWindowRequested => this.request_new_window(cx),
                    AppLifecycleEvent::ShutdownCancelled => {
                        this.quit_requested = false;
                        this.controller
                            .update(cx, |controller, cx| controller.cancel_editor_quit_gates(cx));
                    }
                });
            });
        });
        self._subscriptions.push(shutdown_subscription);
        if cfg!(target_os = "macos") {
            self._subscriptions
                .push(cx.observe(&app, |_, _, cx| native_menu::request_refresh(cx)));
        }
        self.app = Some(app);
        let update_subscription = cx.observe(&update_store, move |this, _, cx| {
            if let Some(app) = this.app.clone() {
                app.update(cx, |_, cx| cx.notify());
            }
            if let Some(bar) = &update_menu_bar {
                bar.update(cx, |bar, cx| bar.refresh_badges(cx));
            }
            native_menu::request_refresh(cx);
        });
        self._subscriptions.push(update_subscription);
        self.lifecycle = AppShellLifecycle::Ready;
        self.start_ready_app(window, cx);
        self.drain_pending_activations(cx);
        let controller = self.controller.downgrade();
        let workspace_id = self.workspace_id;
        cx.defer(move |cx| {
            let _ = controller.update(cx, |controller, cx| {
                controller.workspace_ready(workspace_id, cx)
            });
        });
        cx.notify();
    }

    pub(super) fn enter_recovery(&mut self, error: StoreOperationError, cx: &mut Context<Self>) {
        self.lifecycle = AppShellLifecycle::Recovery(RecoveryState {
            category: error.category().to_string(),
            message: error.user_message().to_string(),
            diagnostics_status: None,
        });
        cx.notify();
    }

    fn start_ready_app(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(app) = self.app.clone() else {
            return;
        };
        // Prepare the font catalog asynchronously after the window becomes ready so
        // the first terminal refresh does not enumerate system fonts.
        app.update(cx, |app, cx| app.ensure_appearance_font_options(cx));
        let should_start_restore = self.startup_restore.update(cx, |store, cx| {
            if store.mark_started_after_window_open() {
                cx.notify();
                true
            } else {
                false
            }
        });
        if should_start_restore {
            app.update(cx, |app, cx| {
                app.start_after_window_open(window, cx);
            });
        }
    }

    fn retry_bootstrap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = window;
        self.begin_bootstrap(cx);
        cx.notify();
    }

    fn export_recovery_diagnostics(&mut self, cx: &mut Context<Self>) {
        let output_path = self
            .runtime
            .log_dir()
            .join("nyaterm-recovery-diagnostics.zip");
        let runtime = self.runtime.clone();
        let options = DiagnosticsExportOptions {
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            language: "unknown".to_string(),
            log_level: "configured".to_string(),
            retention_days: 7,
            runtime_snapshot: DiagnosticsRuntimeSnapshot {
                active_sessions: 0,
                local_sessions: 0,
                ssh_sessions: 0,
                telnet_sessions: 0,
                raw_tcp_sessions: 0,
                serial_sessions: 0,
                open_tunnels: 0,
                pending_tunnels: 0,
                saved_connections: 0,
                saved_tunnels: 0,
                running_transfers: 0,
                paused_transfers: 0,
                completed_transfers: 0,
                failed_transfers: 0,
            },
        };
        if let AppShellLifecycle::Recovery(state) = &mut self.lifecycle {
            state.diagnostics_status = Some("Exporting diagnostics...".to_string());
        }
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    export_diagnostics_archive(&runtime, &options, &output_path)
                        .map(|info| {
                            format!("Diagnostics exported to {}", info.output_path.display())
                        })
                        .unwrap_or_else(|error| format!("Diagnostics export failed: {error}"))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let AppShellLifecycle::Recovery(state) = &mut this.lifecycle {
                    state.diagnostics_status = Some(result);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn request_close(&mut self, cx: &mut Context<Self>) {
        match self.lifecycle {
            AppShellLifecycle::Ready | AppShellLifecycle::FlushFailed(_) if self.app.is_some() => {
                self.begin_shutdown(cx)
            }
            AppShellLifecycle::Flushing => {}
            AppShellLifecycle::Ready
            | AppShellLifecycle::FlushFailed(_)
            | AppShellLifecycle::Loading
            | AppShellLifecycle::Recovery(_) => {
                self.enter_flushing(cx);
                let workspace_id = self.workspace_id;
                self.controller.update(cx, |controller, cx| {
                    controller.request_close_unready_workspace(workspace_id, cx)
                });
            }
        }
    }

    pub fn request_new_window(&mut self, cx: &mut Context<Self>) {
        let controller = self.controller.clone();
        let workspace_id = self.workspace_id;
        // The controller reads this shell to seed the new window's layout.
        cx.defer(move |cx| {
            if let Err(error) = controller.update(cx, |controller, cx| {
                controller.open_workspace(
                    nyaterm_core::OpenWorkspaceRequest {
                        layout_source_workspace_id: Some(workspace_id),
                        ..Default::default()
                    },
                    cx,
                )
            }) {
                tracing::error!(%error, "failed to open a new NyaTerm window");
            }
        });
    }

    pub(super) fn request_application_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.quit_requested = true;
        if let Some(app) = &self.app {
            // This shell is already being updated; only visit other shells through the controller.
            let count = app.read(cx).live_session_count()
                + self.controller.update(cx, |controller, cx| {
                    controller.live_session_count_excluding(self.workspace_id, cx)
                });
            app.update(cx, |app, cx| {
                app.handle_window_close_request_with_count(count, window, cx)
            });
        } else {
            self.controller.update(cx, |controller, cx| {
                controller.request_quit_without_ready(cx)
            });
        }
    }

    pub(super) fn can_coordinate_quit(&self) -> bool {
        self.app.is_some()
    }

    pub(super) fn apply_shared_state(
        &mut self,
        process_state: Entity<ProcessStateStore>,
        event: SharedStateEvent,
        cx: &mut Context<Self>,
    ) {
        if let Some(app) = &self.app {
            app.update(cx, |app, cx| {
                app.apply_shared_state(process_state.read(cx).snapshot().clone(), event, cx)
            });
        }
    }

    pub(super) fn submit_process_quit_persistence(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<Option<StoreTask<()>>, StoreSubmitError> {
        self.app
            .as_ref()
            .map(|app| app.update(cx, |app, _| app.submit_shutdown_persistence(false)))
            .transpose()
    }

    /// Handles a close request from the primary window or native Quit command.
    ///
    /// Once the application is ready, the feature layer owns the confirmation
    /// decision. The shell begins persistence and worker shutdown only after it
    /// receives `AppLifecycleEvent::ShutdownRequested`.
    pub fn request_window_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let started_at = Instant::now();
        match self.lifecycle {
            AppShellLifecycle::Ready | AppShellLifecycle::FlushFailed(_) => {
                if let Some(app) = &self.app {
                    app.update(cx, |app, cx| app.handle_window_close_request(window, cx));
                } else {
                    self.request_close(cx);
                }
            }
            AppShellLifecycle::Flushing => {}
            AppShellLifecycle::Loading | AppShellLifecycle::Recovery(_) => self.request_close(cx),
        }
        tracing::info!(
            elapsed_ms = started_at.elapsed().as_millis(),
            "window close callback completed"
        );
    }

    fn quit_after_worker_shutdown(&mut self, launch_update: bool, cx: &mut Context<Self>) {
        let admitted = self.controller.update(cx, |controller, _| {
            controller.is_process_quitting() || controller.begin_process_quit()
        });
        if !admitted {
            return;
        }
        self.enter_flushing(cx);
        let operations = self.controller.read(cx).sync_operations();
        operations.pause_and_cancel();
        cx.spawn(async move |this, cx| {
            operations.wait_for_idle().await;
            let _ = this.update(cx, |this, cx| {
                this.stop_workspace_workers(launch_update, cx)
            });
        })
        .detach();
    }

    fn stop_workspace_workers(&mut self, launch_update: bool, cx: &mut Context<Self>) {
        // The installer helper waits for process exit. Validate its launch before
        // destroying workspace workers so a failed launch can return to a usable app.
        if launch_update && let Some(app) = &self.app {
            let update_result =
                app.update(cx, |app, cx| app.launch_pending_update_after_shutdown(cx));
            if let Err(error) = update_result {
                if let Some(store_runtime) = &self.store_runtime {
                    store_runtime.resume_after_failed_shutdown();
                }
                self.controller.update(cx, |controller, _| {
                    controller.cancel_process_quit();
                });
                self.lifecycle = AppShellLifecycle::FlushFailed(error.clone());
                app.update(cx, |app, cx| app.report_close_save_failed(error, cx));
                cx.notify();
                return;
            }
        }
        let started_at = Instant::now();
        let mut tasks = self.controller.update(cx, |controller, cx| {
            let mut tasks = controller.shutdown_other_workspaces(self.workspace_id, cx);
            tasks.push(controller.shutdown_plugins(cx));
            tasks
        });
        // The current shell is already borrowed by the persistence completion callback.
        if let Some(app) = &self.app {
            tasks.push(app.update(cx, |app, cx| {
                app.shutdown_workspace_sessions();
                app.shutdown_blocking_jobs(cx)
            }));
        }
        if !matches!(self.lifecycle, AppShellLifecycle::Flushing) {
            self.enter_flushing(cx);
        }
        cx.spawn(async move |this, cx| {
            for task in tasks {
                task.await;
            }
            let _ = this.update(cx, |this, cx| {
                this.finish_worker_shutdown(started_at, cx);
            });
        })
        .detach();
    }

    fn finish_worker_shutdown(&mut self, started_at: Instant, cx: &mut Context<Self>) {
        tracing::info!(
            pid = std::process::id(),
            stage = "workspace_jobs",
            elapsed_ms = started_at.elapsed().as_millis(),
            "workspace workers stopped"
        );
        self.controller
            .update(cx, |controller, cx| controller.quit_after_process_jobs(cx));
    }

    fn enter_flushing(&mut self, cx: &mut Context<Self>) {
        self.pending_activations.clear();
        if let Some(app) = &self.app {
            app.update(cx, |app, _| app.cancel_external_activations());
        }
        if matches!(self.lifecycle, AppShellLifecycle::Flushing) {
            return;
        }
        self.lifecycle = AppShellLifecycle::Flushing;
        self.flushing_view_ready = self.app.is_none();
        cx.notify();

        if self.flushing_view_ready {
            return;
        }

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SHUTDOWN_STATUS_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                if matches!(this.lifecycle, AppShellLifecycle::Flushing)
                    && !this.flushing_view_ready
                {
                    this.flushing_view_ready = true;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn begin_shutdown(&mut self, cx: &mut Context<Self>) {
        let admitted = self.controller.update(cx, |controller, _| {
            controller.is_process_quitting() || controller.begin_process_quit()
        });
        if !admitted {
            return;
        }
        self.enter_flushing(cx);
        let operations = self.controller.read(cx).sync_operations();
        operations.pause_and_cancel();
        let started_at = Instant::now();
        tracing::info!(
            pid = std::process::id(),
            stage = "sync",
            "shutdown stage started"
        );
        cx.spawn(async move |this, cx| {
            operations.wait_for_idle().await;
            let _ = this.update(cx, |this, cx| {
                tracing::info!(
                    pid = std::process::id(),
                    stage = "sync",
                    elapsed_ms = started_at.elapsed().as_millis(),
                    "shutdown stage finished"
                );
                this.begin_shutdown_after_sync(cx);
            });
        })
        .detach();
    }

    fn begin_shutdown_after_sync(&mut self, cx: &mut Context<Self>) {
        let workspace_id = self.workspace_id;
        match self.controller.update(cx, |controller, cx| {
            controller.prepare_other_workspaces_for_quit(workspace_id, cx)
        }) {
            Ok(tasks) => self.pending_process_quit_tasks = tasks,
            Err(error) => {
                self.controller
                    .update(cx, |controller, _| controller.cancel_process_quit());
                self.lifecycle = AppShellLifecycle::FlushFailed(error.to_string());
                cx.notify();
                return;
            }
        }
        let started_at = Instant::now();
        let Some(store_runtime) = &self.store_runtime else {
            self.controller
                .update(cx, |controller, _| controller.cancel_process_quit());
            self.lifecycle = AppShellLifecycle::FlushFailed(
                "The storage runtime is unavailable; pending changes cannot be verified."
                    .to_string(),
            );
            cx.notify();
            return;
        };
        store_runtime.begin_shutdown();
        let store_ui = store_runtime.ui_client();
        let recent = self
            .controller
            .read(cx)
            .is_most_recent_workspace(self.workspace_id);
        let latest_window_state = recent
            .then(|| self.main_window_state.latest_for_shutdown())
            .flatten();
        let Some(app) = &self.app else {
            store_runtime.resume_after_failed_shutdown();
            self.controller
                .update(cx, |controller, _| controller.cancel_process_quit());
            self.lifecycle = AppShellLifecycle::FlushFailed(
                "The application state is unavailable; pending changes cannot be captured."
                    .to_string(),
            );
            cx.notify();
            return;
        };
        let snapshot_task = match app.update(cx, |app, _| app.submit_shutdown_persistence(false)) {
            Ok(task) => task,
            Err(error) => {
                store_runtime.resume_after_failed_shutdown();
                self.controller
                    .update(cx, |controller, _| controller.cancel_process_quit());
                self.lifecycle = AppShellLifecycle::FlushFailed(error.to_string());
                cx.notify();
                return;
            }
        };
        let current_snapshot = app.update(cx, |app, _| app.capture_workspace_close_snapshot());
        let restore_task = match self.controller.update(cx, |controller, cx| {
            controller.submit_process_restore_snapshot(
                self.workspace_id,
                current_snapshot,
                latest_window_state,
                cx,
            )
        }) {
            Ok(task) => task,
            Err(error) => {
                store_runtime.resume_after_failed_shutdown();
                self.controller
                    .update(cx, |controller, _| controller.cancel_process_quit());
                self.lifecycle = AppShellLifecycle::FlushFailed(error.to_string());
                cx.notify();
                return;
            }
        };
        let task = match store_ui.try_submit_shutdown(u64::MAX, FlushBarrier) {
            Ok(task) => task,
            Err(error) => {
                store_runtime.resume_after_failed_shutdown();
                self.controller
                    .update(cx, |controller, _| controller.cancel_process_quit());
                self.lifecycle = AppShellLifecycle::FlushFailed(error.to_string());
                cx.notify();
                return;
            }
        };
        let pending_tasks = std::mem::take(&mut self.pending_process_quit_tasks);
        self.enter_flushing(cx);
        tracing::info!(
            pid = std::process::id(),
            stage = "persistence",
            elapsed_ms = started_at.elapsed().as_millis(),
            "shutdown persistence queued"
        );
        cx.spawn(async move |this, cx| {
            let flush_started_at = Instant::now();
            let mut failure = None;
            for pending in pending_tasks {
                if let Err(error) = pending.await.outcome {
                    failure.get_or_insert(error);
                }
            }
            if let Err(error) = snapshot_task.await.outcome {
                failure.get_or_insert(error);
            }
            if let Err(error) = restore_task.await.outcome {
                failure.get_or_insert(error);
            }
            if let Err(error) = task.await.outcome {
                failure.get_or_insert(error);
            }
            tracing::info!(
                pid = std::process::id(),
                stage = "persistence",
                elapsed_ms = flush_started_at.elapsed().as_millis(),
                "shutdown persistence finished"
            );
            let _ = this.update(cx, |this, cx| {
                if let Some(error) = failure {
                    if let Some(store_runtime) = &this.store_runtime {
                        store_runtime.resume_after_failed_shutdown();
                    }
                    this.controller
                        .update(cx, |controller, _| controller.cancel_process_quit());
                    this.lifecycle = AppShellLifecycle::FlushFailed(error.to_string());
                    cx.notify();
                } else {
                    this.quit_after_worker_shutdown(true, cx);
                }
            });
        })
        .detach();
    }

    fn return_to_app_after_flush_failure(&mut self, cx: &mut Context<Self>) {
        let Some(store_runtime) = &self.store_runtime else {
            return;
        };
        store_runtime.resume_after_failed_shutdown();
        self.controller
            .update(cx, |controller, _| controller.cancel_process_quit());
        self.lifecycle = if self.app.is_some() {
            AppShellLifecycle::Ready
        } else {
            AppShellLifecycle::Recovery(RecoveryState {
                category: "close_failed".to_string(),
                message: "The window was not closed; retry loading or closing it.".to_string(),
                diagnostics_status: None,
            })
        };
        if let Some(app) = &self.app {
            app.update(cx, NyaTermApp::report_shutdown_retry_required);
        }
        cx.notify();
    }

    pub(super) fn finish_workspace_close_failure(
        &mut self,
        message: String,
        cx: &mut Context<Self>,
    ) {
        self.lifecycle = AppShellLifecycle::FlushFailed(message.clone());
        if let Some(app) = &self.app {
            app.update(cx, |app, cx| app.report_close_save_failed(message, cx));
        }
        cx.notify();
    }

    fn retry_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.app.is_none() {
            self.request_close(cx);
            return;
        }
        if self.quit_requested || self.controller.read(cx).workspace_count() == 1 {
            self.begin_shutdown(cx);
            return;
        }
        self.return_to_app_after_flush_failure(cx);
        self.request_window_close(window, cx);
    }

    fn lifecycle_view(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.lifecycle {
            AppShellLifecycle::Loading => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgb(0x101214))
                .text_color(rgb(0xe7e9ea))
                .child("Loading NyaTerm data...")
                .into_any_element(),
            AppShellLifecycle::Recovery(state) => {
                let status = state.diagnostics_status.clone();
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgb(0x101214))
                    .text_color(rgb(0xe7e9ea))
                    .child(
                        div()
                            .w(px(560.))
                            .max_w_full()
                            .p_6()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(div().text_xl().child("NyaTerm could not load its data"))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0xaeb4b8))
                                    .child(format!("{}: {}", state.category, state.message)),
                            )
                            .when_some(status, |view, status| {
                                view.child(div().text_sm().child(status))
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap_2()
                                    .child(
                                        NyaButton::new("recovery-retry", "Retry")
                                            .variant(NyaButtonVariant::Primary)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.retry_bootstrap(window, cx);
                                            })),
                                    )
                                    .child(
                                        NyaButton::new(
                                            "recovery-open-config",
                                            "Open Config Directory",
                                        )
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                cx.reveal_path(this.runtime.config_dir());
                                            }),
                                        ),
                                    )
                                    .child(
                                        NyaButton::new(
                                            "recovery-export-diagnostics",
                                            "Export Diagnostics",
                                        )
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.export_recovery_diagnostics(cx);
                                            }),
                                        ),
                                    )
                                    .child(
                                        NyaButton::new("recovery-quit", "Quit")
                                            .variant(NyaButtonVariant::Danger)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.controller.update(cx, |controller, cx| {
                                                    controller.request_quit(cx)
                                                });
                                            })),
                                    ),
                            ),
                    )
                    .into_any_element()
            }
            AppShellLifecycle::Flushing => {
                debug_assert!(self.flushing_view_ready);
                let updating = self
                    .app
                    .as_ref()
                    .is_some_and(|app| app.read(cx).native_update_install_requested(cx));
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgb(0x101214))
                    .text_color(rgb(0xe7e9ea))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_3()
                            .child(
                                svg()
                                    .size(px(28.))
                                    .path("icons/conn/spinner-arc.svg")
                                    .text_color(rgb(0x3fb950))
                                    .with_animation(
                                        "shutdown-progress-spinner",
                                        gpui::Animation::new(Duration::from_secs(1)).repeat(),
                                        |icon, progress| {
                                            icon.with_transformation(gpui::Transformation::rotate(
                                                gpui::percentage(progress),
                                            ))
                                        },
                                    ),
                            )
                            .child(if updating {
                                t!("appShell.preparingUpdate")
                            } else {
                                t!("appShell.savingBeforeClose")
                            }),
                    )
                    .into_any_element()
            }
            AppShellLifecycle::FlushFailed(message) => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgb(0x101214))
                .text_color(rgb(0xe7e9ea))
                .child(
                    div()
                        .w(px(560.))
                        .max_w_full()
                        .p_6()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(div().text_xl().child("NyaTerm could not save all changes"))
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(0xaeb4b8))
                                .child(message.clone()),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(0xd6a85f))
                                .child("Force Quit will discard changes that could not be saved."),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    NyaButton::new("shutdown-retry", "Retry")
                                        .variant(NyaButtonVariant::Primary)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.retry_close(window, cx);
                                        })),
                                )
                                .child(NyaButton::new("shutdown-return", "Return to App").on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.return_to_app_after_flush_failure(cx);
                                    }),
                                ))
                                .child(
                                    NyaButton::new("shutdown-force", "Force Quit")
                                        .variant(NyaButtonVariant::Danger)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.quit_after_worker_shutdown(false, cx);
                                        })),
                                ),
                        ),
                )
                .into_any_element(),
            AppShellLifecycle::Ready => div().size_full().into_any_element(),
        }
    }
}

fn build_title_menu_bar(
    app: WeakEntity<NyaTermApp>,
    cx: &mut Context<AppShell>,
) -> Entity<NyaAppMenuBar> {
    use crate::models::TitleMenu;

    let menus = [
        TitleMenu::File,
        TitleMenu::View,
        TitleMenu::Terminal,
        TitleMenu::Help,
    ]
    .into_iter()
    .map(|menu| {
        let label_app = app.clone();
        let items_app = app.clone();
        let open_app = app.clone();
        let mut entry = NyaAppMenu::new(
            menu.label(),
            move |cx| {
                label_app
                    .read_with(cx, |app, _| app.title_menu_label(menu).into())
                    .unwrap_or_else(|_| menu.label().into())
            },
            move |_, cx| {
                items_app
                    .update(cx, |app, cx| app.build_title_menu_items(menu, cx))
                    .unwrap_or_default()
            },
        )
        .min_width(px(220.))
        .on_open(move |_, cx| {
            _ = open_app.update(cx, |app, cx| {
                app.prepare_title_menu(cx);
                if menu == TitleMenu::Help {
                    app.acknowledge_help_update(cx);
                }
            });
        });
        if menu == TitleMenu::Help {
            let badge_app = app.clone();
            entry = entry.badge(move |cx| {
                badge_app
                    .read_with(cx, |app, cx| app.help_update_attention(cx))
                    .ok()
                    .flatten()
            });
        }
        entry
    })
    .collect::<Vec<_>>();
    NyaAppMenuBar::new(menus, cx)
}

impl Render for AppShell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let show_app = should_render_app(&self.lifecycle, self.flushing_view_ready);
        let block_input = matches!(self.lifecycle, AppShellLifecycle::Flushing)
            && !self.flushing_view_ready
            && self.app.is_some();

        div()
            .size_full()
            .on_action(cx.listener(|this, _: &NativeNewWindow, _window, cx| {
                if cfg!(target_os = "macos") {
                    cx.propagate();
                } else {
                    this.request_new_window(cx);
                }
            }))
            .when_some(self.app.clone().filter(|_| show_app), |root, app| {
                root.child(app)
            })
            .when(block_input, |root| {
                root.child(
                    div()
                        .id("shutdown-input-blocker")
                        .absolute()
                        .inset_0()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation()),
                )
            })
            .when(!show_app, |root| root.child(self.lifecycle_view(cx)))
    }
}

fn should_render_app(lifecycle: &AppShellLifecycle, flushing_view_ready: bool) -> bool {
    matches!(lifecycle, AppShellLifecycle::Ready)
        || (matches!(lifecycle, AppShellLifecycle::Flushing) && !flushing_view_ready)
}

#[cfg(test)]
mod tests {

    use crate::app_shell::{AppShellLifecycle, native_new_window_key_binding, should_render_app};

    #[test]
    fn native_new_window_shortcut_is_valid_for_the_current_platform() {
        let _ = native_new_window_key_binding();
    }

    #[test]
    fn app_remains_visible_until_delayed_shutdown_status_is_ready() {
        assert!(should_render_app(&AppShellLifecycle::Flushing, false));
        assert!(!should_render_app(&AppShellLifecycle::Flushing, true));
        assert!(should_render_app(&AppShellLifecycle::Ready, true));
    }
}
