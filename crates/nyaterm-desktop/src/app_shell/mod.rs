//! Root GPUI shell boundary.

mod window_state;

use self::window_state::{
    MAIN_WINDOW_STATE_SAVE_DEBOUNCE, MainWindowStateController, capture_main_window_state,
};
pub use window_state::{AppShellStartup, MainWindowPlacement};

use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use gpui::{
    AnyElement, AppContext, Context, Entity, InteractiveElement, IntoElement, Menu, MenuItem,
    OsAction, ParentElement, Render, Styled, Subscription, SystemMenuType, WeakEntity, Window,
    actions, div, prelude::FluentBuilder, px, rgb,
};
use nyaterm_core::{
    ACTIVATION_QUEUE_CAPACITY, ActivationReceiver, ActivationRequest, AppRuntime,
    DiagnosticsExportOptions, DiagnosticsRuntimeSnapshot, export_diagnostics_archive,
};
use nyaterm_store::{
    FlushBarrier, LoadBootstrap, SaveMainWindowState, StoreConfig, StoreOperationError,
    StoreRuntime, StoreTask,
};
use nyaterm_ui::{
    NyaAppMenu, NyaAppMenuBar, NyaButton, NyaButtonVariant, NyaCopy, NyaCut, NyaPaste, NyaRedo,
    NyaSelectAll, NyaUndo,
};

use crate::{
    entities::{OverlayStore, StartupRestoreStore, UiStoreHandles},
    features::{AppLifecycleEvent, NyaTermApp},
};

actions!(
    nyaterm_native_menu,
    [
        NativeAbout,
        NativeHide,
        NativeHideOthers,
        NativeShowAll,
        NativeNewSession,
        NativeQuickSwitch,
        NativeImportConfig,
        NativeExportConfig,
        NativeOpenDocumentation,
        NativeCheckUpdates,
        NativeViewLogs,
        NativeOpenSettings,
        NativeToggleLeftSidebar,
        NativeToggleRightSidebar,
        NativeZoomIn,
        NativeZoomOut,
        NativeResetZoom,
        NativeRefitTerminals,
        NativeTerminalCopy,
        NativeTerminalPaste,
        NativeTerminalFind,
        NativeTerminalClear,
        NativeTerminalSelectAll,
        NativeManageSyncGroups,
        NativeQuit
    ]
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeMenuCommand {
    NewSession,
    QuickSwitch,
    OpenSettings,
    ToggleLeftSidebar,
    ToggleRightSidebar,
    ZoomIn,
    ZoomOut,
    ResetZoom,
    TerminalCopy,
    TerminalPaste,
    TerminalFind,
    TerminalClear,
    TerminalSelectAll,
    ManageSyncGroups,
}

#[allow(dead_code)]
pub struct AppShell {
    runtime: AppRuntime,
    lifecycle: AppShellLifecycle,
    app: Option<Entity<NyaTermApp>>,
    store_runtime: Option<StoreRuntime>,
    pending_bootstrap: Option<StoreTask<nyaterm_store::BootstrapSnapshot>>,
    startup_restore: Entity<StartupRestoreStore>,
    overlays: Entity<OverlayStore>,
    activation_rx: Option<ActivationReceiver>,
    pending_activations: VecDeque<ActivationRequest>,
    recent_activation_ids: HashSet<[u8; 16]>,
    recent_activation_order: VecDeque<[u8; 16]>,
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
        activation_rx: ActivationReceiver,
        startup: AppShellStartup,
        cx: &mut Context<Self>,
    ) -> Self {
        let startup_restore = cx.new(|_| StartupRestoreStore::default());
        let overlays = cx.new(|_| OverlayStore::default());
        install_native_app_menus(cx);
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

        let mut shell = Self {
            runtime,
            lifecycle,
            app: None,
            store_runtime: startup.store_runtime,
            pending_bootstrap: startup.pending_bootstrap,
            startup_restore,
            overlays,
            activation_rx: Some(activation_rx),
            pending_activations: VecDeque::new(),
            recent_activation_ids: HashSet::new(),
            recent_activation_order: VecDeque::new(),
            main_window_state: MainWindowStateController::default(),
            _subscriptions: subscriptions,
        };
        let quit_subscription = cx.on_app_quit(|this, cx| {
            let store = this
                .store_runtime
                .as_ref()
                .map(StoreRuntime::blocking_client);
            let window_state = this.main_window_state.latest_for_shutdown();
            cx.background_executor().spawn(async move {
                if let Some(store) = store {
                    if let Some(state) = window_state {
                        let _ = store.request_shutdown(u64::MAX - 1, SaveMainWindowState(state));
                    }
                    let _ = store.request_shutdown(u64::MAX, FlushBarrier);
                }
            })
        });
        shell._subscriptions.push(quit_subscription);
        shell
    }

    pub fn start_after_window_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_main_window_state_persistence(window, cx);
        self.start_activation_drain(window, cx);
        self.launch_pending_bootstrap(window, cx);
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
                .update(cx, |this, _| {
                    let (generation, state) = this.main_window_state.take_debounced_save()?;
                    let store = this.store_runtime.as_ref()?.ui_client();
                    match store.try_submit(generation, SaveMainWindowState(state)) {
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

    fn start_activation_drain(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut activation_rx) = self.activation_rx.take() else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            loop {
                let can_receive = this
                    .update(cx, |this, _| {
                        matches!(this.lifecycle, AppShellLifecycle::Ready)
                            || this.pending_activations.len() < ACTIVATION_QUEUE_CAPACITY
                    })
                    .unwrap_or(false);
                if !can_receive {
                    cx.background_executor()
                        .timer(Duration::from_millis(40))
                        .await;
                    continue;
                }
                let Some(request) = activation_rx.recv().await else {
                    break;
                };
                if cx
                    .update(|window, cx| {
                        window.activate_window();
                        cx.activate(true);
                        this.update(cx, |this, cx| this.receive_activation(request, cx))
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn receive_activation(&mut self, request: ActivationRequest, cx: &mut Context<Self>) {
        if !self.remember_activation(request.request_id) {
            return;
        }
        if matches!(self.lifecycle, AppShellLifecycle::Ready) {
            if let Some(app) = &self.app {
                app.update(cx, |app, cx| app.handle_activation(request, cx));
            }
        } else if !matches!(self.lifecycle, AppShellLifecycle::Flushing) {
            self.pending_activations.push_back(request);
        }
    }

    fn remember_activation(&mut self, request_id: [u8; 16]) -> bool {
        const RECENT_ACTIVATION_CAPACITY: usize = ACTIVATION_QUEUE_CAPACITY * 4;

        if !self.recent_activation_ids.insert(request_id) {
            return false;
        }
        self.recent_activation_order.push_back(request_id);
        while self.recent_activation_order.len() > RECENT_ACTIVATION_CAPACITY {
            if let Some(expired) = self.recent_activation_order.pop_front() {
                self.recent_activation_ids.remove(&expired);
            }
        }
        true
    }

    fn drain_pending_activations(&mut self, cx: &mut Context<Self>) {
        let Some(app) = self.app.clone() else {
            return;
        };
        while let Some(request) = self.pending_activations.pop_front() {
            app.update(cx, |app, cx| app.handle_activation(request, cx));
        }
    }

    fn begin_bootstrap(&mut self) {
        self.app = None;
        self.lifecycle = AppShellLifecycle::Loading;
        let store_runtime = match StoreRuntime::spawn(StoreConfig {
            config_dir: self.runtime.config_dir().to_path_buf(),
            portable_key_path: self.runtime.portable_key_path().map(ToOwned::to_owned),
        }) {
            Ok(runtime) => runtime,
            Err(error) => {
                self.lifecycle = AppShellLifecycle::Recovery(RecoveryState {
                    category: "worker_start".to_string(),
                    message: error.to_string(),
                    diagnostics_status: None,
                });
                return;
            }
        };
        match store_runtime.ui_client().try_submit(0, LoadBootstrap) {
            Ok(task) => {
                self.pending_bootstrap = Some(task);
                self.store_runtime = Some(store_runtime);
            }
            Err(error) => {
                self.lifecycle = AppShellLifecycle::Recovery(RecoveryState {
                    category: "request_submit".to_string(),
                    message: error.to_string(),
                    diagnostics_status: None,
                });
            }
        }
    }

    fn launch_pending_bootstrap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = self.pending_bootstrap.take() else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let event = task.await;
            let _ = cx.update(|window, cx| {
                this.update(cx, |this, cx| match event.outcome {
                    Ok(bootstrap) => this.complete_bootstrap(bootstrap, window, cx),
                    Err(error) => this.enter_recovery(error, cx),
                })
            });
        })
        .detach();
    }

    fn complete_bootstrap(
        &mut self,
        bootstrap: nyaterm_store::BootstrapSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(store_runtime) = &self.store_runtime else {
            self.lifecycle = AppShellLifecycle::Recovery(RecoveryState {
                category: "runtime_missing".to_string(),
                message: "storage runtime disappeared during bootstrap".to_string(),
                diagnostics_status: None,
            });
            cx.notify();
            return;
        };
        let stores = UiStoreHandles {
            startup_restore: self.startup_restore.clone(),
            overlays: self.overlays.clone(),
        };
        let app = cx.new(|cx| {
            NyaTermApp::from_bootstrap(
                self.runtime.clone(),
                stores,
                bootstrap,
                store_runtime.ui_client(),
                store_runtime.blocking_client(),
                cx,
            )
        });
        let title_menu_bar = build_title_menu_bar(app.downgrade(), cx);
        app.update(cx, |app, cx| {
            app.set_title_menu_bar(title_menu_bar);
            app.start_shell_environment_preload(cx);
            app.start_system_tray(cx);
        });
        let shutdown_subscription =
            cx.subscribe(&app, |this, _, event: &AppLifecycleEvent, cx| match event {
                AppLifecycleEvent::ShutdownRequested => this.request_close(cx),
            });
        self._subscriptions.push(shutdown_subscription);
        self.app = Some(app);
        self.lifecycle = AppShellLifecycle::Ready;
        self.start_ready_app(window, cx);
        self.drain_pending_activations(cx);
        cx.notify();
    }

    fn enter_recovery(&mut self, error: StoreOperationError, cx: &mut Context<Self>) {
        self.store_runtime = None;
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

    fn perform_native_menu_command(
        &mut self,
        command: NativeMenuCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(app) = &self.app {
            app.update(cx, |app, cx| {
                app.perform_native_menu_command(command, window, cx);
            });
        }
    }

    fn update_app(
        &self,
        cx: &mut Context<Self>,
        update: impl FnOnce(&mut NyaTermApp, &mut Context<NyaTermApp>),
    ) {
        if let Some(app) = &self.app {
            app.update(cx, update);
        }
    }

    fn retry_bootstrap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.begin_bootstrap();
        self.launch_pending_bootstrap(window, cx);
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
            AppShellLifecycle::Ready | AppShellLifecycle::FlushFailed(_) => self.begin_shutdown(cx),
            AppShellLifecycle::Flushing => {}
            AppShellLifecycle::Loading | AppShellLifecycle::Recovery(_) => {
                self.quit_after_worker_shutdown(cx);
            }
        }
    }

    /// Handles a close request from the primary window or native Quit command.
    ///
    /// Once the application is ready, the feature layer owns the confirmation
    /// decision. The shell begins persistence and worker shutdown only after it
    /// receives `AppLifecycleEvent::ShutdownRequested`.
    pub fn request_window_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    }

    fn quit_after_worker_shutdown(&mut self, cx: &mut Context<Self>) {
        if let Some(app) = &self.app {
            app.update(cx, |app, _| app.shutdown_blocking_jobs());
        }
        cx.quit();
    }

    fn begin_shutdown(&mut self, cx: &mut Context<Self>) {
        let Some(store_runtime) = &self.store_runtime else {
            self.lifecycle = AppShellLifecycle::FlushFailed(
                "The storage runtime is unavailable; pending changes cannot be verified."
                    .to_string(),
            );
            cx.notify();
            return;
        };
        store_runtime.begin_shutdown();
        let store_ui = store_runtime.ui_client();
        let window_state_task = if let Some(state) = self.main_window_state.latest_for_shutdown() {
            match store_ui.try_submit_shutdown(u64::MAX - 2, SaveMainWindowState(state)) {
                Ok(task) => Some(task),
                Err(error) => {
                    self.lifecycle = AppShellLifecycle::FlushFailed(error.to_string());
                    cx.notify();
                    return;
                }
            }
        } else {
            None
        };
        let Some(app) = &self.app else {
            self.lifecycle = AppShellLifecycle::FlushFailed(
                "The application state is unavailable; pending changes cannot be captured."
                    .to_string(),
            );
            cx.notify();
            return;
        };
        let snapshot_task = match app.update(cx, |app, _| app.submit_shutdown_persistence()) {
            Ok(task) => task,
            Err(error) => {
                self.lifecycle = AppShellLifecycle::FlushFailed(error.to_string());
                cx.notify();
                return;
            }
        };
        let task = match store_ui.try_submit_shutdown(u64::MAX, FlushBarrier) {
            Ok(task) => task,
            Err(error) => {
                self.lifecycle = AppShellLifecycle::FlushFailed(error.to_string());
                cx.notify();
                return;
            }
        };
        drop(window_state_task);
        drop(snapshot_task);
        self.lifecycle = AppShellLifecycle::Flushing;
        cx.spawn(async move |this, cx| {
            let event = task.await;
            let _ = this.update(cx, |this, cx| match event.outcome {
                Ok(()) => {
                    this.quit_after_worker_shutdown(cx);
                }
                Err(error) => {
                    this.lifecycle = AppShellLifecycle::FlushFailed(error.to_string());
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn return_to_app_after_flush_failure(&mut self, cx: &mut Context<Self>) {
        let Some(store_runtime) = &self.store_runtime else {
            return;
        };
        store_runtime.resume_after_failed_shutdown();
        self.lifecycle = AppShellLifecycle::Ready;
        if let Some(app) = &self.app {
            app.update(cx, NyaTermApp::report_shutdown_retry_required);
        }
        cx.notify();
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
                                                this.quit_after_worker_shutdown(cx);
                                            })),
                                    ),
                            ),
                    )
                    .into_any_element()
            }
            AppShellLifecycle::Flushing => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgb(0x101214))
                .text_color(rgb(0xe7e9ea))
                .child("Saving changes before closing...")
                .into_any_element(),
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
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.begin_shutdown(cx);
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
                                            this.quit_after_worker_shutdown(cx);
                                        })),
                                ),
                        ),
                )
                .into_any_element(),
            AppShellLifecycle::Ready => div().size_full().into_any_element(),
        }
    }
}

fn install_native_app_menus(cx: &mut Context<AppShell>) {
    if !cfg!(target_os = "macos") {
        return;
    }
    cx.set_menus(native_app_menus());
}

fn native_app_menus() -> Vec<Menu> {
    vec![
        Menu::new("NyaTerm").items([
            MenuItem::action("About NyaTerm", NativeAbout),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide NyaTerm", NativeHide),
            MenuItem::action("Hide Others", NativeHideOthers),
            MenuItem::action("Show All", NativeShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit NyaTerm", NativeQuit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New Session", NativeNewSession),
            MenuItem::separator(),
            MenuItem::action("Import Config", NativeImportConfig),
            MenuItem::action("Export Config", NativeExportConfig),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", NyaUndo, OsAction::Undo),
            MenuItem::os_action("Redo", NyaRedo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", NyaCut, OsAction::Cut),
            MenuItem::os_action("Copy", NyaCopy, OsAction::Copy),
            MenuItem::os_action("Paste", NyaPaste, OsAction::Paste),
            MenuItem::os_action("Select All", NyaSelectAll, OsAction::SelectAll),
        ]),
        Menu::new("View").items([
            MenuItem::action("Settings", NativeOpenSettings),
            MenuItem::separator(),
            MenuItem::action("Toggle Left Sidebar", NativeToggleLeftSidebar),
            MenuItem::action("Toggle Right Sidebar", NativeToggleRightSidebar),
            MenuItem::separator(),
            MenuItem::action("Zoom In", NativeZoomIn),
            MenuItem::action("Zoom Out", NativeZoomOut),
            MenuItem::action("Reset Zoom", NativeResetZoom),
        ]),
        Menu::new("Terminal").items([
            MenuItem::action("Command Palette", NativeQuickSwitch),
            MenuItem::separator(),
            MenuItem::action("Copy", NativeTerminalCopy),
            MenuItem::action("Paste", NativeTerminalPaste),
            MenuItem::action("Find", NativeTerminalFind),
            MenuItem::action("Clear", NativeTerminalClear),
            MenuItem::action("Select All", NativeTerminalSelectAll),
            MenuItem::separator(),
            MenuItem::action("Manage Sync Groups", NativeManageSyncGroups),
            MenuItem::action("Refit Terminals", NativeRefitTerminals),
        ]),
        Menu::new("Help").items([
            MenuItem::action("Docs", NativeOpenDocumentation),
            MenuItem::action("Check Updates", NativeCheckUpdates),
            MenuItem::action("View Logs", NativeViewLogs),
        ]),
    ]
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
        NyaAppMenu::new(
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
            _ = open_app.update(cx, |app, cx| app.prepare_title_menu(cx));
        })
    })
    .collect::<Vec<_>>();
    NyaAppMenuBar::new(menus, cx)
}

impl Render for AppShell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let show_app = matches!(self.lifecycle, AppShellLifecycle::Ready);
        div()
            .size_full()
            .on_action(cx.listener(|this, _: &NativeNewSession, window, cx| {
                this.perform_native_menu_command(NativeMenuCommand::NewSession, window, cx);
            }))
            .on_action(|_: &NativeHide, _window, cx| {
                cx.hide();
            })
            .on_action(|_: &NativeHideOthers, _window, cx| {
                cx.hide_other_apps();
            })
            .on_action(|_: &NativeShowAll, _window, cx| {
                cx.unhide_other_apps();
            })
            .on_action(cx.listener(|this, _: &NativeImportConfig, window, cx| {
                this.update_app(cx, |app, cx| {
                    app.open_connection_import_dialog_for_menu(window, cx);
                });
            }))
            .on_action(cx.listener(|this, _: &NativeExportConfig, window, cx| {
                this.update_app(cx, |app, cx| {
                    app.prompt_encrypted_portable_snapshot_export_for_menu(window, cx);
                });
            }))
            .on_action(
                cx.listener(|this, _: &NativeOpenDocumentation, _window, cx| {
                    this.update_app(cx, |app, cx| {
                        app.open_documentation_for_menu(cx);
                    });
                }),
            )
            .on_action(cx.listener(|this, _: &NativeCheckUpdates, window, cx| {
                this.update_app(cx, |app, cx| {
                    app.open_update_dialog_for_menu(window, cx);
                });
            }))
            .on_action(cx.listener(|this, _: &NativeViewLogs, _window, cx| {
                this.update_app(cx, |app, cx| {
                    app.reveal_log_dir_for_menu(cx);
                });
            }))
            .on_action(cx.listener(|this, _: &NativeAbout, window, cx| {
                this.update_app(cx, |app, cx| {
                    app.open_about_for_menu(window, cx);
                });
            }))
            .on_action(cx.listener(|this, _: &NativeQuickSwitch, window, cx| {
                this.perform_native_menu_command(NativeMenuCommand::QuickSwitch, window, cx);
            }))
            .on_action(cx.listener(|this, _: &NativeOpenSettings, window, cx| {
                this.perform_native_menu_command(NativeMenuCommand::OpenSettings, window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &NativeToggleLeftSidebar, window, cx| {
                    this.perform_native_menu_command(
                        NativeMenuCommand::ToggleLeftSidebar,
                        window,
                        cx,
                    );
                }),
            )
            .on_action(
                cx.listener(|this, _: &NativeToggleRightSidebar, window, cx| {
                    this.perform_native_menu_command(
                        NativeMenuCommand::ToggleRightSidebar,
                        window,
                        cx,
                    );
                }),
            )
            .on_action(cx.listener(|this, _: &NativeZoomIn, window, cx| {
                this.perform_native_menu_command(NativeMenuCommand::ZoomIn, window, cx);
            }))
            .on_action(cx.listener(|this, _: &NativeZoomOut, window, cx| {
                this.perform_native_menu_command(NativeMenuCommand::ZoomOut, window, cx);
            }))
            .on_action(cx.listener(|this, _: &NativeResetZoom, window, cx| {
                this.perform_native_menu_command(NativeMenuCommand::ResetZoom, window, cx);
            }))
            .on_action(cx.listener(|this, _: &NativeRefitTerminals, _window, cx| {
                this.update_app(cx, |app, cx| {
                    app.resize_all_known_terminal_surfaces_for_menu(cx);
                });
            }))
            .on_action(cx.listener(|this, _: &NativeTerminalCopy, window, cx| {
                this.perform_native_menu_command(NativeMenuCommand::TerminalCopy, window, cx);
            }))
            .on_action(cx.listener(|this, _: &NativeTerminalPaste, window, cx| {
                this.perform_native_menu_command(NativeMenuCommand::TerminalPaste, window, cx);
            }))
            .on_action(cx.listener(|this, _: &NativeTerminalFind, window, cx| {
                this.perform_native_menu_command(NativeMenuCommand::TerminalFind, window, cx);
            }))
            .on_action(cx.listener(|this, _: &NativeTerminalClear, window, cx| {
                this.perform_native_menu_command(NativeMenuCommand::TerminalClear, window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &NativeTerminalSelectAll, window, cx| {
                    this.perform_native_menu_command(
                        NativeMenuCommand::TerminalSelectAll,
                        window,
                        cx,
                    );
                }),
            )
            .on_action(cx.listener(|this, _: &NativeManageSyncGroups, window, cx| {
                this.perform_native_menu_command(NativeMenuCommand::ManageSyncGroups, window, cx);
            }))
            .on_action(cx.listener(|this, _: &NativeQuit, window, cx| {
                this.request_window_close(window, cx);
            }))
            .when_some(self.app.clone().filter(|_| show_app), |root, app| {
                root.child(app)
            })
            .when(!show_app, |root| root.child(self.lifecycle_view(cx)))
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Menu, MenuItem};

    use crate::app_shell::native_app_menus;

    fn menu_names(menus: &[Menu]) -> Vec<&str> {
        menus.iter().map(|menu| menu.name.as_ref()).collect()
    }

    fn item_name(item: &MenuItem) -> Option<&str> {
        match item {
            MenuItem::Action { name, .. } => Some(name.as_ref()),
            MenuItem::Submenu(menu) => Some(menu.name.as_ref()),
            MenuItem::SystemMenu(menu) => Some(menu.name.as_ref()),
            MenuItem::Separator => None,
        }
    }

    fn item_names(menu: &Menu) -> Vec<&str> {
        menu.items.iter().filter_map(item_name).collect()
    }

    #[test]
    fn native_menu_keeps_tauri_macos_top_level_order() {
        let menus = native_app_menus();

        assert_eq!(
            menu_names(&menus),
            ["NyaTerm", "File", "Edit", "View", "Terminal", "Help"]
        );
    }

    #[test]
    fn native_edit_menu_is_standard_macos_edit_layer() {
        let menus = native_app_menus();
        let edit = menus
            .iter()
            .find(|menu| menu.name.as_ref() == "Edit")
            .expect("edit menu");

        assert_eq!(
            item_names(edit),
            ["Undo", "Redo", "Cut", "Copy", "Paste", "Select All"]
        );
    }

    #[test]
    fn native_about_lives_in_app_menu_not_help_menu() {
        let menus = native_app_menus();
        let app = menus
            .iter()
            .find(|menu| menu.name.as_ref() == "NyaTerm")
            .expect("app menu");
        let help = menus
            .iter()
            .find(|menu| menu.name.as_ref() == "Help")
            .expect("help menu");

        assert!(item_names(app).contains(&"About NyaTerm"));
        assert!(!item_names(help).contains(&"About NyaTerm"));
    }
}
