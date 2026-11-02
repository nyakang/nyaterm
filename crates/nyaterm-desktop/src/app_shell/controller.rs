//! Process composition and activation ownership. Domain adapters live in child modules.

use crate::app_shell::AppShell;
use crate::app_shell::AppShellStartup;
use crate::app_shell::ProcessStateStore;
use crate::app_shell::SessionHub;
use crate::app_shell::tab_drag::TabDragCoordinator;
use crate::features::shell::tray::SystemTray;
use crate::features::update::UpdateStore;
use gpui::AnyWindowHandle;
use gpui::AppContext as _;
use gpui::Context;
use gpui::Task;
use gpui::WeakEntity;
use nyaterm_core::ACTIVATION_QUEUE_CAPACITY;
use nyaterm_core::ActivationReceiver;
use nyaterm_core::ActivationRequest;
use nyaterm_core::AppRuntime;
use nyaterm_core::DeviceWindowManifest;
use nyaterm_core::MoveTabTreeRequest;
use nyaterm_core::WorkspaceId;
use nyaterm_store::BootstrapSnapshot;
use nyaterm_store::StoreTask;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
mod auto_sync;
mod bootstrap;
pub(crate) mod native_menu;
mod shutdown;
#[cfg(test)]
mod shutdown_tests;
#[cfg(test)]
mod tab_drag_tests;
mod tab_moves;
#[cfg(test)]
mod tests;
mod tray;
#[cfg(test)]
mod tray_tests;
mod updates;
mod window_decorations;
mod workspaces;

use auto_sync::AutoSyncCoordinator;
#[cfg(test)]
use bootstrap::should_enable_startup_screen_lock;
#[cfg(test)]
use shutdown::next_recent_after_close;
#[cfg(test)]
use tab_moves::workspace_targets_from_order;
#[cfg(test)]
use tray::tray_click_shows_window;
#[cfg(test)]
use workspaces::normalize_new_workspace_ui;

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
    tab_drag: TabDragCoordinator,
    /// Workspaces whose settings tab is open, oldest first. The last entry owns
    /// the process-wide settings preview.
    settings_owners: Vec<WorkspaceId>,
    tray: Option<SystemTray>,
    screen_locked: bool,
    most_recent_workspace_id: Option<WorkspaceId>,
    recent_activations: RecentActivationCache,
    recent_connections: nyaterm_core::activation::RecentConnectionRequests,
    closing_workspaces: HashSet<WorkspaceId>,
    process_quitting: bool,
    pending_bootstrap: Option<StoreTask<BootstrapSnapshot>>,
    bootstrap_in_flight: bool,
    process_state: Option<gpui::Entity<ProcessStateStore>>,
    update_store: gpui::Entity<UpdateStore>,
    plugin_process: gpui::Entity<crate::features::plugins::PluginProcess>,
    shared_refresh_generation: u64,
    applied_shared_refresh_generation: u64,
    auto_sync: AutoSyncCoordinator,
    native_menu: native_menu::NativeMenuState,
}

impl DesktopController {
    pub fn new(runtime: AppRuntime, mut startup: AppShellStartup, cx: &mut Context<Self>) -> Self {
        let device_windows = startup.device_windows.clone();
        let pending_bootstrap = startup.take_pending_bootstrap();
        let mutation_generation = startup
            .shared_store_runtime()
            .map_or(0, |store| store.sync_mutation_generation());
        Self {
            native_menu: Default::default(),
            plugin_process: cx
                .new(|cx| crate::features::plugins::PluginProcess::new(runtime.clone(), cx)),
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
            tab_drag: TabDragCoordinator::default(),
            settings_owners: Vec::new(),
            tray: None,
            screen_locked: false,
            recent_activations: RecentActivationCache::default(),
            recent_connections: Default::default(),
            closing_workspaces: HashSet::new(),
            process_quitting: false,
            pending_bootstrap,
            bootstrap_in_flight: false,
            process_state: None,
            update_store: cx.new(|_| UpdateStore::new()),
            shared_refresh_generation: 0,
            applied_shared_refresh_generation: 0,
            auto_sync: AutoSyncCoordinator::new(mutation_generation),
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
            self.open_workspace_with_startup(startup, None, false, None, cx)?;
        }
        self.recent_activations
            .remember(initial_activation.request_id);
        let mut initial_activation = initial_activation.parse();
        self.deduplicate_connections(&mut initial_activation);
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
        self.start_auto_sync_runtime(cx);
        self.start_tray(cx);
        Ok(())
    }

    pub(crate) fn sync_operations(
        &self,
    ) -> crate::features::sync::operation::CloudSyncOperationRegistry {
        self.auto_sync.operations.clone()
    }

    pub(crate) fn update_store(&self) -> gpui::Entity<UpdateStore> {
        self.update_store.clone()
    }

    pub(crate) fn plugin_process(&self) -> gpui::Entity<crate::features::plugins::PluginProcess> {
        self.plugin_process.clone()
    }

    pub(crate) fn shutdown_plugins(&self, cx: &mut gpui::App) -> Task<()> {
        let service = self.plugin_process.read(cx).service();
        cx.background_executor().spawn(async move {
            if let Some(service) = service {
                service.shutdown();
            }
        })
    }
    #[cfg(test)]
    pub(crate) fn restart_plugins(&mut self, cx: &mut gpui::App) {
        let runtime = self.runtime.clone();
        self.plugin_process
            .update(cx, |process, cx| process.restart(runtime, cx));
    }
}
