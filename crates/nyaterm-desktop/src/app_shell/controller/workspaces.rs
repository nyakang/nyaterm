use super::DesktopController;
use super::WorkspaceTarget;
use super::WorkspaceWindow;
use super::tab_moves::workspace_targets_from_order;
use crate::app_shell::AppShell;
use crate::app_shell::AppShellStartup;
use crate::app_shell::GlobalStateMutation;
use crate::app_shell::window_state::MainWindowPlacement;
use crate::features::shell::tray::show_window;
use crate::models::NavItem;
use gpui::AppContext as _;
use gpui::Context;
use gpui::TitlebarOptions;
use gpui::WindowOptions;
use gpui::point;
use gpui::px;
use nyaterm_core::ActivationOpenBehavior;
use nyaterm_core::ActivationRequest;
use nyaterm_core::AppSettingsSummary;
use nyaterm_core::OpenWorkspaceRequest;
use nyaterm_core::WorkspaceId;
use nyaterm_core::WorkspaceUiState;
use nyaterm_core::activation::ParsedActivationRequest;
use nyaterm_ui::nya_root;
use std::cell::RefCell;
use std::rc::Rc;

impl DesktopController {
    pub fn screen_locked(&self) -> bool {
        self.screen_locked
    }

    pub fn set_screen_locked(&mut self, locked: bool, source: WorkspaceId, cx: &mut Context<Self>) {
        if self.screen_locked == locked {
            return;
        }
        self.screen_locked = locked;
        self.request_native_menu_refresh(cx);
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
        self.open_workspace_at(request, None, cx)
    }

    pub(super) fn open_workspace_at(
        &mut self,
        request: OpenWorkspaceRequest,
        placement: Option<MainWindowPlacement>,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<WorkspaceId> {
        anyhow::ensure!(!self.process_quitting, "the application is closing");
        let workspace_id = WorkspaceId::new();
        let ui = self.workspace_ui_seed(request.layout_source_workspace_id, cx);
        let startup = self.startup.for_new_workspace(workspace_id, ui);
        self.open_workspace_with_startup(
            startup,
            request.activation,
            request.activate,
            placement,
            cx,
        )?;
        Ok(workspace_id)
    }

    pub(super) fn workspace_ui_seed(
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

    pub(super) fn live_workspace_ui(
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

    pub(super) fn open_workspace_with_startup(
        &mut self,
        startup: AppShellStartup,
        initial_activation: Option<ParsedActivationRequest>,
        activate: bool,
        placement_override: Option<MainWindowPlacement>,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let workspace_id = startup.workspace_id();
        let placement = placement_override.unwrap_or_else(|| startup.main_window_placement(cx));
        let runtime = self.runtime.clone();
        let controller = cx.entity();
        let session_hub = self.session_hub.clone();
        let shell_slot = Rc::new(RefCell::new(None));
        let shell_slot_for_window = shell_slot.clone();
        let flavor = nyaterm_core::app_identity::AppFlavor::current();
        #[cfg(target_os = "linux")]
        let window_decorations = self.effective_window_decorations(cx);
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
                window_decorations: Some(window_decorations),
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
        self.request_native_menu_refresh(cx);
        self.windows.insert(
            workspace_id,
            WorkspaceWindow {
                handle: handle.into(),
                shell,
            },
        );
        self.tab_drag
            .remember_normal_size(workspace_id, placement.window_bounds.get_bounds().size);
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
        let mut request = request.parse();
        self.deduplicate_connections(&mut request);
        if request.actions.is_empty() {
            return;
        }
        if request.open_behavior == ActivationOpenBehavior::ReuseMostRecent {
            let target = self
                .most_recent_workspace_id
                .filter(|id| self.windows.contains_key(id) && !self.closing_workspaces.contains(id))
                .or_else(|| {
                    self.device_windows
                        .window_order
                        .iter()
                        .rev()
                        .find(|id| {
                            self.windows.contains_key(id) && !self.closing_workspaces.contains(id)
                        })
                        .copied()
                });
            if self.activate_and_deliver(target, request.clone(), cx) {
                return;
            }
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

    pub(super) fn deduplicate_connections(&mut self, request: &mut ParsedActivationRequest) {
        let now = std::time::Instant::now();
        request.actions.retain(|action| match action {
            Ok(nyaterm_core::activation::ActivationAction::Connect(connection)) => {
                self.recent_connections.remember(connection, now)
            }
            _ => true,
        });
    }

    pub(super) fn activate_and_deliver(
        &mut self,
        workspace_id: Option<WorkspaceId>,
        request: ParsedActivationRequest,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(workspace_id) = workspace_id else {
            return false;
        };
        if self.closing_workspaces.contains(&workspace_id) {
            return false;
        }
        let Some(entry) = self.windows.get(&workspace_id) else {
            return false;
        };
        if entry
            .shell
            .update(cx, |shell, cx| shell.receive_activation_direct(request, cx))
            .ok()
            != Some(true)
        {
            return false;
        }
        let _ = entry.handle.update(cx, |_, window, cx| {
            show_window(window, cx);
        });
        self.most_recent_workspace_id = Some(workspace_id);
        true
    }

    pub fn mark_active(&mut self, workspace_id: WorkspaceId) {
        if self.windows.contains_key(&workspace_id) {
            self.most_recent_workspace_id = Some(workspace_id);
            self.device_windows.most_recent_workspace_id = Some(workspace_id);
            self.auto_sync.focused();
        }
    }

    pub fn publish_settings(
        &mut self,
        source: WorkspaceId,
        settings: AppSettingsSummary,
        cx: &mut Context<Self>,
    ) {
        self.publish_settings_change(source, settings, false, cx);
    }

    /// The source already merged this commit into its settings draft. Echoing a
    /// whole-settings event back would misreport its unrelated edits as a conflict.
    pub(crate) fn publish_theme_settings(
        &mut self,
        source: WorkspaceId,
        settings: AppSettingsSummary,
        cx: &mut Context<Self>,
    ) {
        self.publish_settings_change(source, settings, true, cx);
    }

    fn publish_settings_change(
        &mut self,
        source: WorkspaceId,
        settings: AppSettingsSummary,
        exclude_source: bool,
        cx: &mut Context<Self>,
    ) {
        self.auto_sync.settings_changed();
        let Some(process_state) = self.process_state.clone() else {
            return;
        };
        self.shared_refresh_generation = self.shared_refresh_generation.saturating_add(1);
        self.applied_shared_refresh_generation = self.shared_refresh_generation;
        let event = process_state.update(cx, |state, cx| {
            state.mutate(GlobalStateMutation::UpdateSettings(Box::new(settings)), cx)
        });
        self.broadcast_shared_state_except(event, exclude_source.then_some(source), cx);
    }

    /// Point the process-wide settings preview at the workspace that opened a
    /// settings tab.
    ///
    /// Settings is now a per-window tab, so two windows can each hold a draft.
    /// The window-decorations/material preview has to resolve to one of them;
    /// the most recently opened settings tab owns it until it closes.
    pub fn activate_or_claim_settings(
        &mut self,
        requester: WorkspaceId,
        _cx: &mut Context<Self>,
    ) -> bool {
        self.settings_owners.retain(|id| *id != requester);
        self.settings_owners.push(requester);
        false
    }

    pub fn release_settings_owner(&mut self, workspace_id: WorkspaceId, cx: &mut Context<Self>) {
        let previous_len = self.settings_owners.len();
        self.settings_owners.retain(|id| *id != workspace_id);
        if self.settings_owners.len() != previous_len {
            crate::window_chrome::request_window_decoration_refresh(cx);
        }
    }

    pub fn workspace_count(&self) -> usize {
        self.windows.len()
    }

    pub(in crate::app_shell) fn is_process_quitting(&self) -> bool {
        self.process_quitting
    }

    pub fn begin_process_quit(&mut self) -> bool {
        if self.process_quitting || !self.closing_workspaces.is_empty() {
            return false;
        }
        self.process_quitting = true;
        self.auto_sync.operations.pause_and_cancel();
        true
    }

    pub fn cancel_process_quit(&mut self) {
        self.process_quitting = false;
        self.auto_sync.operations.resume();
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
}
pub(super) fn normalize_new_workspace_ui(mut ui: WorkspaceUiState) -> WorkspaceUiState {
    if NavItem::from_persistence_id(&ui.current_page).is_some_and(NavItem::opens_settings) {
        ui.current_page = NavItem::Workspace.persistence_id().to_string();
    }
    ui
}
