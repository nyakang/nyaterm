use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::time::Duration;

use futures::StreamExt as _;
use gpui::{
    AnyWindowHandle, AppContext as _, Context, TitlebarOptions, WeakEntity, WindowOptions, point,
    px,
};
use nyaterm_core::{
    ACTIVATION_QUEUE_CAPACITY, ActivationOpenBehavior, ActivationReceiver, ActivationRequest,
    AppRuntime, AppSettingsSummary, DeviceWindowManifest, DeviceWindowState, MainWindowState,
    MoveTabTreeRequest, OpenWorkspaceRequest, WorkspaceId, WorkspaceUiState,
};
use nyaterm_store::{
    BootstrapSnapshot, FlushBarrier, LoadBootstrap, StoreDomain, StoreOperationError,
    StoreSubmitError, StoreTask, store_request,
};
use nyaterm_ui::{NyaRoot, nya_root};

use super::{
    AppShell, AppShellStartup, GlobalStateMutation, ProcessStateStore, SessionHub,
    SharedStateDomain, SharedStateEvent,
};
use crate::features::update::{UpdateCheckKind, UpdateEvent, UpdateStore};
use crate::features::{SystemTray, TraySnapshot, WorkspaceCloseSnapshot, show_tray_window};
use crate::models::NavItem;

pub struct DesktopControllerGlobal(pub gpui::Entity<DesktopController>);
impl gpui::Global for DesktopControllerGlobal {}

struct WorkspaceWindow {
    handle: AnyWindowHandle,
    shell: WeakEntity<AppShell>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorkspaceTarget {
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) ordinal: usize,
}

#[derive(Default)]
struct RecentActivationCache {
    ids: HashSet<[u8; 16]>,
    order: VecDeque<[u8; 16]>,
}

impl RecentActivationCache {
    fn remember(&mut self, id: [u8; 16]) -> bool {
        const CAPACITY: usize = ACTIVATION_QUEUE_CAPACITY * 4;
        if !self.ids.insert(id) {
            return false;
        }
        self.order.push_back(id);
        while self.order.len() > CAPACITY {
            if let Some(expired) = self.order.pop_front() {
                self.ids.remove(&expired);
            }
        }
        true
    }
}

pub struct DesktopController {
    runtime: AppRuntime,
    startup: AppShellStartup,
    session_hub: gpui::Entity<SessionHub>,
    device_windows: DeviceWindowManifest,
    windows: HashMap<WorkspaceId, WorkspaceWindow>,
    pending_tab_moves: HashMap<WorkspaceId, MoveTabTreeRequest>,
    settings_owner_workspace_id: Option<WorkspaceId>,
    tray: Option<SystemTray>,
    screen_locked: bool,
    most_recent_workspace_id: Option<WorkspaceId>,
    recent_activations: RecentActivationCache,
    closing_workspaces: HashSet<WorkspaceId>,
    process_quitting: bool,
    pending_bootstrap: Option<StoreTask<BootstrapSnapshot>>,
    bootstrap_in_flight: bool,
    process_state: Option<gpui::Entity<ProcessStateStore>>,
    update_store: gpui::Entity<UpdateStore>,
    shared_refresh_generation: u64,
    applied_shared_refresh_generation: u64,
}

impl DesktopController {
    pub fn new(runtime: AppRuntime, mut startup: AppShellStartup, cx: &mut Context<Self>) -> Self {
        let device_windows = startup.device_windows.clone();
        let pending_bootstrap = startup.take_pending_bootstrap();
        Self {
            runtime,
            session_hub: cx.new(|_| SessionHub::new()),
            most_recent_workspace_id: startup
                .device_windows
                .most_recent_workspace_id
                .or(Some(startup.workspace_id())),
            startup,
            device_windows,
            windows: HashMap::new(),
            pending_tab_moves: HashMap::new(),
            settings_owner_workspace_id: None,
            tray: None,
            screen_locked: false,
            recent_activations: RecentActivationCache::default(),
            closing_workspaces: HashSet::new(),
            process_quitting: false,
            pending_bootstrap,
            bootstrap_in_flight: false,
            process_state: None,
            update_store: cx.new(|_| UpdateStore::new()),
            shared_refresh_generation: 0,
            applied_shared_refresh_generation: 0,
        }
    }

    pub fn launch(
        &mut self,
        initial_activation: ActivationRequest,
        activation_rx: ActivationReceiver,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let mut ids = self.startup.restored_workspace_ids();
        if let Some(recent) = self.most_recent_workspace_id
            && let Some(index) = ids.iter().position(|id| *id == recent)
        {
            ids.swap(0, index);
        }
        for workspace_id in ids {
            let startup = self.startup.for_workspace(workspace_id);
            self.open_workspace_with_startup(startup, None, false, cx)?;
        }
        let _ = self.activate_and_deliver(self.most_recent_workspace_id, initial_activation, cx);
        self.launch_initial_bootstrap(cx);
        if let Some(recent) = self.most_recent_workspace_id
            && let Some(entry) = self.windows.get(&recent)
        {
            let _ = entry.handle.update(cx, |_, window, cx| {
                window.activate_window();
                cx.activate(true);
            });
        }
        cx.spawn(async move |this, cx| {
            let mut activation_rx = activation_rx;
            while let Some(request) = activation_rx.recv().await {
                if this
                    .update(cx, |controller, cx| {
                        controller.route_activation(request, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        self.start_update_runtime(cx);
        self.start_tray(cx);
        Ok(())
    }

    fn start_update_runtime(&mut self, cx: &mut Context<Self>) {
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
        if let Err(error) = std::thread::Builder::new()
            .name("nyaterm-update-check".to_string())
            .spawn(move || {
                let result = crate::http::update::check_native_update();
                let _ = tx.unbounded_send(UpdateEvent::Check {
                    generation,
                    kind,
                    result,
                });
            })
        {
            let _ = rejected_tx.unbounded_send(UpdateEvent::Check {
                generation,
                kind,
                result: Err(format!("could not start update check: {error}")),
            });
        }
    }

    fn launch_initial_bootstrap(&mut self, cx: &mut Context<Self>) {
        let Some(task) = self.pending_bootstrap.take() else {
            return;
        };
        self.bootstrap_in_flight = true;
        self.await_process_bootstrap(task, cx);
    }

    fn await_process_bootstrap(
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

    fn install_process_snapshot(&mut self, snapshot: BootstrapSnapshot, cx: &mut Context<Self>) {
        if should_enable_startup_screen_lock(
            self.process_state.is_some(),
            snapshot.settings.enable_screen_lock,
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
        let workspace_ids = self.windows.keys().copied().collect::<Vec<_>>();
        for workspace_id in workspace_ids {
            self.deliver_process_state(workspace_id, process_state.clone(), cx);
        }
    }

    fn broadcast_shared_state(&self, event: SharedStateEvent, cx: &mut Context<Self>) {
        if !event.changed {
            return;
        }
        let Some(process_state) = self.process_state.clone() else {
            return;
        };
        for entry in self.windows.values() {
            let shell = entry.shell.clone();
            let process_state = process_state.clone();
            cx.defer(move |cx| {
                let _ = shell.update(cx, |shell, cx| {
                    shell.apply_shared_state(process_state, event, cx)
                });
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

    fn apply_shared_snapshot(
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

    fn report_process_bootstrap_failure(
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

    fn deliver_process_state(
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

    pub(super) fn retry_process_bootstrap(&mut self, cx: &mut Context<Self>) {
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

    fn start_tray(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let image = cx
                .background_spawn(async {
                    image::load_from_memory(include_bytes!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../nyaterm-app/resources/icons/32x32.png"
                    )))
                    .map(|image| {
                        let image = image.to_rgba8();
                        (image.width(), image.height(), image.into_raw())
                    })
                })
                .await;
            let Ok((width, height, pixels)) = image else {
                return;
            };
            let initialized = this
                .update(cx, |controller, cx| {
                    let snapshot = controller.tray_snapshot(cx);
                    controller.tray = SystemTray::new(snapshot, pixels, width, height).ok();
                    controller.tray.is_some()
                })
                .unwrap_or(false);
            if !initialized {
                return;
            }
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                if this
                    .update(cx, |controller, cx| controller.poll_tray(cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn recent_app(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Entity<crate::features::NyaTermApp>> {
        let id = self
            .most_recent_workspace_id
            .or_else(|| self.windows.keys().next().copied())?;
        self.windows
            .get(&id)?
            .shell
            .update(cx, |shell, _| shell.app.clone())
            .ok()
            .flatten()
    }

    fn tray_snapshot(&self, cx: &mut Context<Self>) -> TraySnapshot {
        self.recent_app(cx)
            .map(|app| app.read(cx).tray_snapshot())
            .unwrap_or_else(TraySnapshot::empty)
    }

    fn poll_tray(&mut self, cx: &mut Context<Self>) {
        let snapshot = self.tray_snapshot(cx);
        if let Some(tray) = self.tray.as_mut() {
            tray.update(snapshot)
        }
        while let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            self.handle_tray_action(event.id.as_ref(), cx);
        }
        while let Ok(event) = tray_icon::TrayIconEvent::receiver().try_recv() {
            if matches!(event, tray_icon::TrayIconEvent::DoubleClick { .. }) {
                self.handle_tray_action("show", cx);
            }
        }
    }

    fn handle_tray_action(&mut self, action: &str, cx: &mut Context<Self>) {
        match action {
            "show" => {
                if let Some(id) = self
                    .most_recent_workspace_id
                    .or_else(|| self.windows.keys().next().copied())
                {
                    if let Some(entry) = self.windows.get(&id) {
                        let _ = entry.handle.update(cx, |_, window, cx| {
                            show_tray_window(window, cx);
                        });
                    }
                } else if let Err(error) = self.open_workspace(OpenWorkspaceRequest::default(), cx)
                {
                    tracing::error!(%error, "could not show NyaTerm window");
                }
            }
            "new-window" => {
                if let Err(error) = self.open_workspace(OpenWorkspaceRequest::default(), cx) {
                    tracing::error!(%error, "could not open NyaTerm window");
                }
            }
            "quit" => self.request_quit(cx),
            _ => {
                if let Some(app) = self.recent_app(cx) {
                    let action = action.to_string();
                    app.update(cx, |app, cx| app.handle_tray_action(action, cx));
                }
            }
        }
    }

    pub fn tray_available(&self) -> bool {
        self.tray.is_some()
    }

    pub fn screen_locked(&self) -> bool {
        self.screen_locked
    }

    pub fn set_screen_locked(&mut self, locked: bool, source: WorkspaceId, cx: &mut Context<Self>) {
        if self.screen_locked == locked {
            return;
        }
        self.screen_locked = locked;
        for (id, entry) in &self.windows {
            if *id == source {
                continue;
            }
            let app = entry
                .shell
                .update(cx, |shell, _| shell.app.clone())
                .ok()
                .flatten();
            if let Some(app) = app {
                let _ = entry.handle.update(cx, |_, window, cx| {
                    app.update(cx, |app, cx| {
                        app.apply_shared_screen_lock(locked, window, cx)
                    });
                });
            }
        }
    }

    pub fn open_workspace(
        &mut self,
        request: OpenWorkspaceRequest,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<WorkspaceId> {
        anyhow::ensure!(!self.process_quitting, "the application is closing");
        let workspace_id = WorkspaceId::new();
        let ui = self.workspace_ui_seed(request.layout_source_workspace_id, cx);
        let startup = self.startup.for_new_workspace(workspace_id, ui);
        self.open_workspace_with_startup(startup, request.activation, request.activate, cx)?;
        Ok(workspace_id)
    }

    pub(crate) fn update_store(&self) -> gpui::Entity<UpdateStore> {
        self.update_store.clone()
    }

    fn workspace_ui_seed(
        &self,
        requested_source: Option<WorkspaceId>,
        cx: &mut Context<Self>,
    ) -> WorkspaceUiState {
        let live = requested_source
            .into_iter()
            .chain(self.most_recent_workspace_id)
            .find_map(|workspace_id| self.live_workspace_ui(workspace_id, cx));
        let ui = live
            .or_else(|| {
                self.startup
                    .workspace_restore
                    .most_recent()
                    .map(|workspace| workspace.ui.clone())
            })
            .unwrap_or_default();
        normalize_new_workspace_ui(ui)
    }

    fn live_workspace_ui(
        &self,
        workspace_id: WorkspaceId,
        cx: &mut Context<Self>,
    ) -> Option<WorkspaceUiState> {
        let entry = self.windows.get(&workspace_id)?;
        let app = entry
            .shell
            .update(cx, |shell, _| shell.app.clone())
            .ok()
            .flatten()?;
        Some(app.read(cx).capture_workspace_ui_state())
    }

    fn open_workspace_with_startup(
        &mut self,
        startup: AppShellStartup,
        initial_activation: Option<ActivationRequest>,
        activate: bool,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let workspace_id = startup.workspace_id();
        let placement = startup.main_window_placement(cx);
        let runtime = self.runtime.clone();
        let controller = cx.entity();
        let session_hub = self.session_hub.clone();
        let shell_slot = Rc::new(RefCell::new(None));
        let shell_slot_for_window = shell_slot.clone();
        let flavor = nyaterm_core::app_identity::AppFlavor::current();
        let handle = cx.open_window(
            WindowOptions {
                app_id: Some(flavor.desktop_id().to_string()),
                titlebar: Some(TitlebarOptions {
                    title: Some(flavor.display_name().into()),
                    appears_transparent: true,
                    traffic_light_position: cfg!(target_os = "macos")
                        .then(|| point(px(9.), px(11.))),
                }),
                #[cfg(target_os = "linux")]
                window_decorations: Some(gpui::WindowDecorations::Client),
                window_bounds: Some(placement.window_bounds),
                display_id: placement.display_id,
                ..Default::default()
            },
            move |window, cx| {
                let shell = cx.new(|cx| {
                    AppShell::new(
                        runtime,
                        initial_activation,
                        startup,
                        workspace_id,
                        controller,
                        session_hub,
                        cx,
                    )
                });
                *shell_slot_for_window.borrow_mut() = Some(shell.downgrade());
                let close_shell = shell.clone();
                window.on_window_should_close(cx, move |window, cx| {
                    close_shell.update(cx, |shell, cx| shell.request_window_close(window, cx));
                    false
                });
                shell.update(cx, |shell, cx| shell.start_after_window_open(window, cx));
                cx.new(|cx| nya_root(shell, window, cx))
            },
        )?;
        let shell = shell_slot
            .borrow_mut()
            .take()
            .expect("window must construct its workspace shell");
        self.windows.insert(
            workspace_id,
            WorkspaceWindow {
                handle: handle.into(),
                shell,
            },
        );
        if let Some(process_state) = self.process_state.clone() {
            self.deliver_process_state(workspace_id, process_state, cx);
        }
        if !self.device_windows.window_order.contains(&workspace_id) {
            self.device_windows.window_order.push(workspace_id);
        }
        if activate || self.most_recent_workspace_id.is_none() {
            self.most_recent_workspace_id = Some(workspace_id);
        }
        if activate {
            let _ = handle.update(cx, |_, window, cx| {
                window.activate_window();
                cx.activate(true);
            });
        }
        Ok(())
    }

    pub fn route_activation(&mut self, request: ActivationRequest, cx: &mut Context<Self>) {
        if self.process_quitting {
            return;
        }
        if !self.recent_activations.remember(request.request_id) {
            return;
        }
        if request.open_behavior() == ActivationOpenBehavior::ReuseMostRecent
            && self.activate_and_deliver(self.most_recent_workspace_id, request.clone(), cx)
        {
            return;
        }
        if let Err(error) = self.open_workspace(
            OpenWorkspaceRequest {
                activation: Some(request),
                ..Default::default()
            },
            cx,
        ) {
            tracing::error!(%error, "failed to open workspace for activation");
        }
    }

    fn activate_and_deliver(
        &mut self,
        workspace_id: Option<WorkspaceId>,
        request: ActivationRequest,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(workspace_id) = workspace_id else {
            return false;
        };
        let Some(entry) = self.windows.get(&workspace_id) else {
            return false;
        };
        if entry
            .shell
            .update(cx, |shell, cx| shell.receive_activation_direct(request, cx))
            .is_err()
        {
            return false;
        }
        let _ = entry.handle.update(cx, |_, window, cx| {
            window.activate_window();
            cx.activate(true);
        });
        self.most_recent_workspace_id = Some(workspace_id);
        true
    }

    pub fn mark_active(&mut self, workspace_id: WorkspaceId) {
        if self.windows.contains_key(&workspace_id) {
            self.most_recent_workspace_id = Some(workspace_id);
            self.device_windows.most_recent_workspace_id = Some(workspace_id);
        }
    }

    pub fn publish_settings(
        &mut self,
        source: WorkspaceId,
        settings: AppSettingsSummary,
        cx: &mut Context<Self>,
    ) {
        let _ = source;
        let Some(process_state) = self.process_state.clone() else {
            return;
        };
        self.shared_refresh_generation = self.shared_refresh_generation.saturating_add(1);
        self.applied_shared_refresh_generation = self.shared_refresh_generation;
        let event = process_state.update(cx, |state, cx| {
            state.mutate(GlobalStateMutation::UpdateSettings(Box::new(settings)), cx)
        });
        self.broadcast_shared_state(event, cx);
    }

    pub fn activate_or_claim_settings(
        &mut self,
        requester: WorkspaceId,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(owner) = self.settings_owner_workspace_id else {
            self.settings_owner_workspace_id = Some(requester);
            return false;
        };
        if owner == requester {
            return false;
        }
        let Some(entry) = self.windows.get(&owner) else {
            self.settings_owner_workspace_id = Some(requester);
            return false;
        };
        let app = entry
            .shell
            .update(cx, |shell, _| shell.app.clone())
            .ok()
            .flatten();
        if let Some(app) = app {
            app.update(cx, |app, cx| {
                app.activate_settings_window(cx);
            });
            let _ = entry.handle.update(cx, |_, window, cx| {
                window.activate_window();
                cx.activate(true);
            });
            self.mark_active(owner);
            true
        } else {
            self.settings_owner_workspace_id = Some(requester);
            false
        }
    }

    pub fn release_settings_owner(&mut self, workspace_id: WorkspaceId) {
        if self.settings_owner_workspace_id == Some(workspace_id) {
            self.settings_owner_workspace_id = None;
        }
    }

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

    fn submit_closed_workspace_manifests(
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

    fn finish_workspace_close(
        &mut self,
        workspace_id: WorkspaceId,
        device_windows: DeviceWindowManifest,
        next_recent: Option<WorkspaceId>,
        cx: &mut Context<Self>,
    ) {
        self.closing_workspaces.remove(&workspace_id);
        let Some(entry) = self.windows.remove(&workspace_id) else {
            return;
        };
        let _ = entry.shell.update(cx, |shell, cx| {
            if let Some(app) = &shell.app {
                app.update(cx, |app, _| {
                    app.shutdown_workspace_sessions();
                    app.shutdown_blocking_jobs();
                });
            }
        });
        self.release_settings_owner(workspace_id);
        self.pending_tab_moves.remove(&workspace_id);
        cx.defer(move |cx| {
            let _ = entry
                .handle
                .update(cx, |_, window, _| window.remove_window());
        });
        self.most_recent_workspace_id = next_recent;
        self.device_windows = device_windows;
    }

    fn report_workspace_close_failure(
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
    }

    fn defer_workspace_close_failure(
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
        if self.process_quitting || self.closing_workspaces.contains(&workspace_id) {
            return;
        }
        if self.windows.len() == 1 {
            self.process_quitting = true;
            let Some(store_runtime) = self.startup.shared_store_runtime() else {
                self.process_quitting = false;
                self.defer_workspace_close_failure(
                    workspace_id,
                    "The storage runtime is unavailable".to_string(),
                    cx,
                );
                return;
            };
            store_runtime.begin_shutdown();
            let task = store_runtime
                .ui_client()
                .try_submit_shutdown(0, FlushBarrier);
            match task {
                Ok(task) => {
                    cx.spawn(async move |this, cx| {
                        let outcome = task.await.outcome;
                        let _ = this.update(cx, |controller, cx| match outcome {
                            Ok(()) => {
                                controller.shutdown_all_workspaces(cx);
                                cx.quit();
                            }
                            Err(error) => {
                                controller.process_quitting = false;
                                store_runtime.resume_after_failed_shutdown();
                                controller.report_workspace_close_failure(
                                    workspace_id,
                                    error.to_string(),
                                    cx,
                                );
                            }
                        });
                    })
                    .detach();
                }
                Err(error) => {
                    self.process_quitting = false;
                    store_runtime.resume_after_failed_shutdown();
                    self.defer_workspace_close_failure(workspace_id, error.to_string(), cx);
                }
            }
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
        let workspace_id = ready
            .iter()
            .copied()
            .find(|id| {
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
                    let _ =
                        shell.update(cx, |shell, cx| shell.request_application_quit(window, cx));
                });
            });
        }
    }

    pub fn live_session_count(&self, cx: &mut Context<Self>) -> usize {
        self.windows
            .values()
            .filter_map(|entry| {
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
        let Some(store_runtime) = self.startup.shared_store_runtime() else {
            self.shutdown_all_workspaces(cx);
            cx.quit();
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
                            controller.shutdown_all_workspaces(cx);
                            cx.quit();
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

    pub fn shutdown_all_workspaces(&mut self, cx: &mut Context<Self>) {
        for entry in self.windows.values() {
            let _ = entry.shell.update(cx, |shell, cx| {
                if let Some(app) = &shell.app {
                    app.update(cx, |app, _| {
                        app.shutdown_workspace_sessions();
                        app.shutdown_blocking_jobs();
                    });
                }
            });
        }
    }

    pub fn workspace_count(&self) -> usize {
        self.windows.len()
    }

    pub fn begin_process_quit(&mut self) -> bool {
        if self.process_quitting || !self.closing_workspaces.is_empty() {
            return false;
        }
        self.process_quitting = true;
        true
    }

    pub fn cancel_process_quit(&mut self) {
        self.process_quitting = false;
    }

    pub fn is_most_recent_workspace(&self, workspace_id: WorkspaceId) -> bool {
        self.most_recent_workspace_id == Some(workspace_id)
    }

    pub(crate) fn workspace_targets(&self, source: WorkspaceId) -> Vec<WorkspaceTarget> {
        let mut ordered = self.device_windows.window_order.clone();
        for workspace_id in self.windows.keys().copied() {
            if !ordered.contains(&workspace_id) {
                ordered.push(workspace_id);
            }
        }
        workspace_targets_from_order(source, &ordered, |workspace_id| {
            self.windows.contains_key(&workspace_id)
        })
    }

    pub fn move_tab_tree(
        &mut self,
        request: MoveTabTreeRequest,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if request.source_workspace_id == request.target_workspace_id {
            return Err("source and target workspaces must differ".into());
        }
        let source = self
            .windows
            .get(&request.source_workspace_id)
            .ok_or("the source window closed during the drag")?;
        let target = self
            .windows
            .get(&request.target_workspace_id)
            .ok_or("the target window closed during the drag")?;
        let source_app = source
            .shell
            .update(cx, |shell, _| shell.app.clone())
            .map_err(|_| "the source window closed during the drag")?
            .ok_or("the source window is still loading")?;
        let target_app = target
            .shell
            .update(cx, |shell, _| shell.app.clone())
            .map_err(|_| "the target window closed during the drag")?
            .ok_or("the target window is still loading")?;
        let session_ids = source_app
            .read(cx)
            .can_transfer_tab_tree(&request.root_tab_id, request.source_revision)?;
        target_app
            .read(cx)
            .can_accept_tab_tree(&session_ids, &request.placement)?;
        let bundle = source_app.update(cx, |app, cx| {
            app.detach_tab_tree_for_transfer(&request.root_tab_id, request.source_revision, cx)
        })?;
        if let Err(failure) = target_app.update(cx, |app, cx| {
            app.attach_tab_tree_from_transfer(bundle, &request.placement, cx)
        }) {
            let (error, bundle) = *failure;
            source_app
                .update(cx, |app, cx| {
                    app.restore_tab_tree_after_failed_transfer(bundle, cx)
                })
                .map_err(|restore_error| format!("{error}; rollback failed: {restore_error}"))?;
            return Err(error);
        }
        let target = self
            .windows
            .get(&request.target_workspace_id)
            .ok_or("the target window closed during the move")?;
        let _ = target.handle.update(cx, |_, window, cx| {
            window.activate_window();
            cx.activate(true);
        });
        self.mark_active(request.target_workspace_id);
        Ok(())
    }

    pub fn open_workspace_for_tab(
        &mut self,
        source_workspace_id: WorkspaceId,
        root_tab_id: String,
        source_revision: u64,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<WorkspaceId> {
        let target_workspace_id = self.open_workspace(
            OpenWorkspaceRequest {
                layout_source_workspace_id: Some(source_workspace_id),
                ..Default::default()
            },
            cx,
        )?;
        self.pending_tab_moves.insert(
            target_workspace_id,
            MoveTabTreeRequest {
                source_workspace_id,
                target_workspace_id,
                root_tab_id,
                source_revision,
                placement: Default::default(),
            },
        );
        Ok(target_workspace_id)
    }

    pub fn workspace_ready(&mut self, workspace_id: WorkspaceId, cx: &mut Context<Self>) {
        let Some(request) = self.pending_tab_moves.remove(&workspace_id) else {
            return;
        };
        if let Err(error) = self.move_tab_tree(request, cx) {
            tracing::warn!(%error, "could not move tab to newly opened window");
        }
    }
}

fn next_recent_after_close(
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

fn workspace_targets_from_order(
    source: WorkspaceId,
    window_order: &[WorkspaceId],
    is_open: impl Fn(WorkspaceId) -> bool,
) -> Vec<WorkspaceTarget> {
    window_order
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, workspace_id)| *workspace_id != source && is_open(*workspace_id))
        .map(|(index, workspace_id)| WorkspaceTarget {
            workspace_id,
            ordinal: index + 1,
        })
        .collect()
}

fn should_enable_startup_screen_lock(
    process_state_loaded: bool,
    screen_lock_enabled: bool,
) -> bool {
    !process_state_loaded && screen_lock_enabled
}

fn normalize_new_workspace_ui(mut ui: WorkspaceUiState) -> WorkspaceUiState {
    if NavItem::from_persistence_id(&ui.current_page).is_some_and(NavItem::opens_settings) {
        ui.current_page = NavItem::Workspace.persistence_id().to_string();
    }
    ui
}

#[allow(dead_code)]
fn _assert_root_type(_: gpui::WindowHandle<NyaRoot>) {}

#[cfg(test)]
mod tests {
    use super::{
        RecentActivationCache, next_recent_after_close, normalize_new_workspace_ui,
        should_enable_startup_screen_lock, workspace_targets_from_order,
    };
    use nyaterm_core::{ACTIVATION_QUEUE_CAPACITY, WorkspaceId, WorkspaceUiState};

    #[test]
    fn activation_ids_are_deduplicated_until_fifo_eviction() {
        let mut cache = RecentActivationCache::default();
        let id = |index: u64| {
            let mut id = [0_u8; 16];
            id[..8].copy_from_slice(&index.to_le_bytes());
            id
        };
        assert!(cache.remember(id(0)));
        assert!(!cache.remember(id(0)));
        for index in 1..=(ACTIVATION_QUEUE_CAPACITY * 4) as u64 {
            assert!(cache.remember(id(index)));
        }
        assert!(!cache.remember(id(1)));
        assert!(cache.remember(id(0)), "the oldest ID was evicted");
    }

    #[test]
    fn closing_recent_workspace_uses_reverse_window_order_not_hash_iteration() {
        let first = WorkspaceId::new();
        let second = WorkspaceId::new();
        let third = WorkspaceId::new();
        let order = [first, second, third];
        assert_eq!(
            next_recent_after_close(Some(third), third, &order, |id| id != third),
            Some(second)
        );
        assert_eq!(
            next_recent_after_close(Some(third), third, &order, |id| id == first),
            Some(first)
        );
        assert_eq!(
            next_recent_after_close(Some(third), third, &order, |_| false),
            None
        );
    }

    #[test]
    fn workspace_target_ordinals_do_not_change_when_source_is_filtered_out() {
        let first = WorkspaceId::new();
        let source = WorkspaceId::new();
        let third = WorkspaceId::new();

        let targets = workspace_targets_from_order(source, &[first, source, third], |_| true);

        assert_eq!(targets.len(), 2);
        assert_eq!((targets[0].workspace_id, targets[0].ordinal), (first, 1));
        assert_eq!((targets[1].workspace_id, targets[1].ordinal), (third, 3));
    }

    #[test]
    fn new_workspace_normalizes_single_owner_settings_page() {
        let ui = WorkspaceUiState {
            current_page: "settings".to_string(),
            ..WorkspaceUiState::default()
        };

        assert_eq!(normalize_new_workspace_ui(ui).current_page, "workspace");
    }

    #[test]
    fn initial_bootstrap_enables_startup_screen_lock_from_persisted_setting() {
        assert!(should_enable_startup_screen_lock(false, true));
        assert!(!should_enable_startup_screen_lock(false, false));
    }

    #[test]
    fn shared_state_refresh_does_not_retrigger_startup_screen_lock() {
        assert!(!should_enable_startup_screen_lock(true, true));
    }
}
