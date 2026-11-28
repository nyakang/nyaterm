use crate::features::NyaTermApp;
use crate::features::remote::remote_runtime::host_tools::HostInteractionIdentity;
use crate::features::text_inputs::TextInputSetup;
use crate::models::NavItem;
use gpui::{AppContext as _, Context, Entity, WeakEntity, Window};
use nyaterm_remote_monitor_gpui::actions::{
    HostInteractionToken, RemoteMonitorAction, RemoteMonitorActionHandler,
};
pub(in crate::features) use nyaterm_remote_monitor_gpui::panels::RemoteMonitorKind;
use nyaterm_remote_monitor_gpui::panels::{
    DockerSnapshot, PanelChrome, RemoteMonitorData, RemoteMonitorKey, RemoteMonitorPanel,
    RemoteMonitorSnapshot,
};
use nyaterm_ui::{NyaNumberInputOptions, NyaSelectOption};
use rust_i18n::t;
use std::rc::Rc;
use std::time::Instant;
trait RemoteMonitorNavigation {
    fn nav_item(self) -> NavItem;
}
impl RemoteMonitorNavigation for RemoteMonitorKind {
    fn nav_item(self) -> NavItem {
        match self {
            Self::Stats => NavItem::Stats,
            Self::Gpu => NavItem::GpuMonitor,
            Self::Npu => NavItem::AscendNpuMonitor,
            Self::Processes => NavItem::Processes,
            Self::Docker => NavItem::Docker,
        }
    }
}
/// The five panel entities, as one field on `NyaTermApp`.
pub(in crate::features) struct RemotePanels {
    host_identity: Option<HostInteractionIdentity>,
    host_token: HostInteractionToken,
    stats: Entity<RemoteMonitorPanel>,
    gpu: Entity<RemoteMonitorPanel>,
    npu: Entity<RemoteMonitorPanel>,
    processes: Entity<RemoteMonitorPanel>,
    docker: Entity<RemoteMonitorPanel>,
}

impl RemotePanels {
    pub(in crate::features) fn new(
        app: WeakEntity<NyaTermApp>,
        cx: &mut Context<NyaTermApp>,
    ) -> Self {
        let panel = |kind: RemoteMonitorKind, cx: &mut Context<NyaTermApp>| {
            let app = app.clone();
            let handler: RemoteMonitorActionHandler = Rc::new(move |action, window, cx| {
                let Some(app) = app.upgrade() else {
                    return false;
                };
                app.update(cx, |app, cx| {
                    app.handle_remote_monitor_action(action, window, cx)
                })
            });
            cx.new(|_| RemoteMonitorPanel::new(kind, handler))
        };
        Self {
            host_identity: None,
            host_token: HostInteractionToken::default(),
            stats: panel(RemoteMonitorKind::Stats, cx),
            gpu: panel(RemoteMonitorKind::Gpu, cx),
            npu: panel(RemoteMonitorKind::Npu, cx),
            processes: panel(RemoteMonitorKind::Processes, cx),
            docker: panel(RemoteMonitorKind::Docker, cx),
        }
    }

    pub(in crate::features) fn entity(
        &self,
        kind: RemoteMonitorKind,
    ) -> &Entity<RemoteMonitorPanel> {
        match kind {
            RemoteMonitorKind::Stats => &self.stats,
            RemoteMonitorKind::Gpu => &self.gpu,
            RemoteMonitorKind::Npu => &self.npu,
            RemoteMonitorKind::Processes => &self.processes,
            RemoteMonitorKind::Docker => &self.docker,
        }
    }
}

impl NyaTermApp {
    fn handle_remote_monitor_action(
        &mut self,
        action: RemoteMonitorAction,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> bool {
        use RemoteMonitorAction as A;
        match action {
            A::CheckHost(token) => {
                return token == self.remote_panels.host_token
                    && self.remote_panels.host_identity.as_ref()
                        == Some(&self.host_interaction_identity(cx));
            }
            A::Tick(kind) => {
                self.refresh_remote_monitor_if_due(kind, cx);
                self.flush_remote_panel_snapshots(cx);
                if kind == RemoteMonitorKind::Stats && self.panel_is_rendered(kind.nav_item()) {
                    self.remote_panels
                        .entity(kind)
                        .clone()
                        .update(cx, |panel, cx| {
                            panel.advance_network_clock(Instant::now(), cx)
                        });
                }
                return true;
            }
            A::CopyProcess(value, label) => self.copy_process_text(value, label, cx),
            A::SelectProcess(pid) => self.toggle_process_selection(pid, cx),
            A::CloseProcessMenu => self.remote_ops.close_process_menu(),
            A::ToggleProcessMenu(pid) => self.remote_ops.toggle_process_menu(pid),
            A::SortProcess(key) => self.toggle_process_sort(key, cx),
            A::ToggleCurrentProcessSort => {
                self.toggle_process_sort(self.remote_ops.process_presentation().sort_key, cx)
            }
            A::ToggleGpu(id) => self.toggle_gpu_device_expanded(id, cx),
            A::ToggleNpu(id) => self.toggle_npu_device_expanded(id, cx),
            A::ProcessOffset(kind, offset) => {
                let moved = match kind {
                    RemoteMonitorKind::Gpu => self.remote_ops.set_gpu_process_offset(offset),
                    RemoteMonitorKind::Npu => self.remote_ops.set_npu_process_offset(offset),
                    _ => false,
                };
                self.defer_remote_panel_snapshot_flush(cx);
                if moved {
                    cx.notify();
                }
                return moved;
            }
            A::ToggleCpu => self.toggle_stats_cpu_expanded(cx),
            A::ToggleMemory => self.remote_ops.toggle_stats_memory_expanded(),
            A::ToggleNetworkInterfaces => {
                if let Some(id) = self.session.active_id().map(str::to_string) {
                    self.remote_ops.toggle_stats_network_interfaces(&id);
                }
            }
            A::CloseContainerMenu => self.remote_ops.close_docker_container_menu(),
            A::ToggleContainerMenu(id) => self.remote_ops.toggle_docker_container_menu(id),
            A::ContainerLogs(id) => self.send_docker_container_logs_to_terminal(id, cx),
            A::ContainerTerminal(id) => self.enter_docker_container_terminal(id, cx),
            A::SetDockerTab(tab) => self.set_docker_tab(tab, cx),
            A::ToggleDockerTabMenu => self.toggle_docker_tab_menu(cx),
            A::CloseComposeMenu => self.remote_ops.close_docker_compose_menu(),
            A::ToggleComposeMenu(id) => self.remote_ops.toggle_docker_compose_menu(id),
            A::ComposeServiceLogs(project, files, service) => {
                self.send_docker_compose_service_logs_to_terminal(project, files, service, cx)
            }
            action => {
                let Some(window) = window else {
                    return false;
                };
                match action {
                    A::SignalProcess(pid, signal) => {
                        self.request_process_signal(pid, signal, window, cx)
                    }
                    A::ApplyProcessNice => self.apply_process_nice_draft(window, cx),
                    A::ContainerDetails(id) => self.open_docker_details(id, window, cx),
                    A::ContainerAction(id, action) => {
                        self.docker_container_action(id, action, window, cx)
                    }
                    A::ConfirmDocker(confirm) => self.request_docker_confirm(confirm, window, cx),
                    A::ComposeAction(project, files, action) => {
                        self.docker_compose_action(project, files, action, window, cx)
                    }
                    A::ComposeServiceAction(project, files, service, action) => self
                        .docker_compose_service_action(project, files, service, action, window, cx),
                    A::ToggleComposeProject(project, files) => {
                        self.toggle_docker_compose_project(project, files, window, cx)
                    }
                    A::LoadComposeServices(project, files) => {
                        self.load_docker_compose_services(project, files, window, cx)
                    }
                    _ => unreachable!(),
                }
            }
        }
        self.defer_remote_panel_snapshot_flush(cx);
        cx.notify();
        true
    }

    /// Queue a remote-panel snapshot rebuild after the current GPUI entity leases end.
    pub(in crate::features) fn defer_remote_panel_snapshot_flush(&self, cx: &mut Context<Self>) {
        self.defer_app_update(cx, |app, cx| {
            app.flush_remote_panel_snapshots(cx);
        });
    }

    /// Push a fresh snapshot to every panel whose pane has moved on.
    ///
    /// **Never called from `render`.** The boundaries are the remote event drains, remote
    /// input subscriptions, a settings apply, `sync_remote_panels_after_activation` for
    /// a session switch, the panel's own refresh clock, and the injected action handler
    /// for anything the panel initiates. GPUI runs `flush_effects` when the outermost
    /// `update` finishes, so a mutation and the panel update it causes land in the same
    /// cycle, before any paint.
    ///
    /// Cheap when nothing changed: one `u64` compare per kind.
    pub(in crate::features) fn flush_remote_panel_snapshots(&mut self, cx: &mut Context<Self>) {
        let identity = self.host_interaction_identity(cx);
        if self.remote_panels.host_identity.as_ref() != Some(&identity) {
            self.remote_panels.host_identity = Some(identity);
            self.remote_panels.host_token.0 = self
                .remote_panels
                .host_token
                .0
                .checked_add(1)
                .expect("host token exhausted");
        }
        for kind in RemoteMonitorPanel::SNAPSHOT_KINDS {
            let key = self.remote_monitor_key(kind, cx);
            let panel = self.remote_panels.entity(kind).clone();
            if panel.read(cx).snapshot_key() == Some(key.clone()) {
                continue;
            }
            let snapshot = self.build_remote_monitor_snapshot(kind, key, cx);
            panel.update(cx, |panel, cx| panel.set_snapshot(snapshot, cx));
        }

        // Freshness, asserted here rather than in render -- a render-time app read would
        // be recorded as a dependency and defeat the isolation this exists to provide.
        // This catches a flush that is itself wrong: a kind missing from the loop, or a
        // snapshot built for the wrong pane. It cannot catch a *missed boundary*, since a
        // boundary never reached never runs it; that is what the freshness tests cover.
        #[cfg(debug_assertions)]
        for kind in RemoteMonitorPanel::SNAPSHOT_KINDS {
            debug_assert_eq!(
                self.remote_panels.entity(kind).read(cx).snapshot_key(),
                Some(self.remote_monitor_key(kind, cx)),
                "flush left a panel behind its pane"
            );
        }
    }

    fn remote_monitor_key(&self, kind: RemoteMonitorKind, _cx: &gpui::App) -> RemoteMonitorKey {
        let revision = match kind {
            RemoteMonitorKind::Stats => self.remote_ops.stats_revision(),
            RemoteMonitorKind::Gpu => self.remote_ops.gpu_revision(),
            RemoteMonitorKind::Npu => self.remote_ops.npu_revision(),
            RemoteMonitorKind::Processes => self.remote_ops.process_revision(),
            RemoteMonitorKind::Docker => self.remote_ops.docker_revision(),
        };
        RemoteMonitorKey {
            revision,
            host_identity: self.remote_panels.host_token,
            process_control: self.process_control_capabilities(),
            local_unsupported: self.local_host_tools_unsupported()
                && matches!(
                    kind,
                    RemoteMonitorKind::Gpu | RemoteMonitorKind::Npu | RemoteMonitorKind::Docker
                ),
            chrome: self.panel_chrome(),
            has_session: self.session.active_ssh_config().is_some()
                || self.session.active_is_local_pty(),
            ssh_compatibility_mode: self
                .session
                .active_ssh_config()
                .is_some_and(|config| config.ssh_compatibility_mode),
            panel_width: match kind {
                // Width is a presentation input, including accelerator card layouts.
                RemoteMonitorKind::Processes
                | RemoteMonitorKind::Docker
                | RemoteMonitorKind::Gpu
                | RemoteMonitorKind::Npu => self.shell.right_panel_width(),
                _ => 0.,
            },
            stats_interval: if kind == RemoteMonitorKind::Stats {
                self.settings.summary().ui_remote_stats_interval.max(1)
            } else {
                0
            },
            show_users: kind == RemoteMonitorKind::Stats
                && self.settings.summary().ui_remote_stats_show_users,
        }
    }

    pub(in crate::features) fn panel_chrome(&self) -> PanelChrome {
        let palette = self.theme_palette();
        PanelChrome {
            transparent_surface: self.shell_transparent_color(palette.surface),
            surface: self.shell_surface_color(palette.surface),
            transparent_section_header: self.shell_transparent_color(palette.section_header),
            palette,
        }
    }

    fn build_remote_monitor_snapshot(
        &mut self,
        kind: RemoteMonitorKind,
        key: RemoteMonitorKey,
        cx: &mut Context<Self>,
    ) -> RemoteMonitorSnapshot {
        let data = match kind {
            RemoteMonitorKind::Stats => {
                let state = self.remote_ops.stats_presentation();
                let mut options = vec![NyaSelectOption::new(
                    "all",
                    t!("resourceMonitor.allInterfaces"),
                )];
                if let Some(stats) = &state.data {
                    options.extend(stats.networks.iter().map(|network| {
                        NyaSelectOption::new(format!("nic:{}", network.nic), network.nic.clone())
                    }));
                }
                let selected_value = state
                    .selected_network_interface
                    .as_ref()
                    .map(|nic| format!("nic:{nic}"))
                    .unwrap_or_else(|| "all".to_string());
                let network_select = self.select_entity(
                    "remote.stats.network",
                    options,
                    Some(selected_value),
                    !key.has_session,
                    cx,
                );
                RemoteMonitorData::Stats {
                    state,
                    network_select,
                }
            }
            RemoteMonitorKind::Gpu => {
                let state = self.remote_ops.gpu_presentation();
                let processes = self.remote_ops.derived_gpu_processes();
                // The registry hands back the existing entity after the first call, so
                // the seed only applies when the field is created.
                let search = self.text_input(
                    "remote.gpu.filter",
                    &state.search_draft.clone(),
                    TextInputSetup::placeholder(t!("gpuMonitor.search")),
                    cx,
                );
                RemoteMonitorData::Gpu {
                    state,
                    processes,
                    search,
                }
            }
            RemoteMonitorKind::Npu => {
                let state = self.remote_ops.npu_presentation();
                let processes = self.remote_ops.derived_npu_processes();
                let search = self.text_input(
                    "remote.npu.filter",
                    &state.search_draft.clone(),
                    TextInputSetup::placeholder(t!("ascendNpuMonitor.search")),
                    cx,
                );
                RemoteMonitorData::Npu {
                    state,
                    processes,
                    search,
                }
            }
            RemoteMonitorKind::Processes => {
                let state = self.remote_ops.process_presentation();
                let processes = self.remote_ops.derived_processes();
                let search = self.text_input(
                    "remote.process.filter",
                    &state.search_draft.clone(),
                    TextInputSetup::placeholder(t!("processManager.search")),
                    cx,
                );
                // Keyed per pid and only while something is selected, matching what the
                // view used to build inline.
                let nice = state
                    .selected_pid
                    .filter(|_| self.process_control_capabilities().supports_priority())
                    .map(|pid| {
                        self.number_input(
                            format!("remote.process.{pid}.nice"),
                            &state.nice_draft.clone(),
                            NyaNumberInputOptions::default().range(-20.0, 19.0),
                            cx,
                        )
                    });
                let sort = self.select_entity(
                    "remote.process.sort",
                    [
                        (
                            crate::models::RemoteProcessSortKey::Command,
                            t!("processManager.process"),
                        ),
                        (
                            crate::models::RemoteProcessSortKey::Pid,
                            t!("processManager.sortPid"),
                        ),
                        (
                            crate::models::RemoteProcessSortKey::Cpu,
                            t!("processManager.sortCpu"),
                        ),
                        (
                            crate::models::RemoteProcessSortKey::Memory,
                            t!("processManager.sortMemory"),
                        ),
                        (
                            crate::models::RemoteProcessSortKey::User,
                            t!("processManager.user"),
                        ),
                    ]
                    .into_iter()
                    .map(|(key, label)| NyaSelectOption::new(key.label(), label))
                    .collect(),
                    Some(state.sort_key.label().to_owned()),
                    false,
                    cx,
                );
                RemoteMonitorData::Processes {
                    state,
                    processes,
                    search,
                    nice,
                    sort,
                }
            }
            RemoteMonitorKind::Docker => {
                let state = self.remote_ops.docker_presentation();
                let derived = self.remote_ops.derived_docker_items();
                let effective_tab = self.remote_ops.docker_effective_tab();
                let search = self.text_input(
                    "remote.docker.filter",
                    &state.search_draft.clone(),
                    TextInputSetup::placeholder(t!("dockerManager.search")),
                    cx,
                );
                RemoteMonitorData::Docker(Box::new(DockerSnapshot {
                    state,
                    derived,
                    effective_tab,
                    search,
                }))
            }
        };
        RemoteMonitorSnapshot { key, data }
    }

    /// Refresh one pane if its interval is due and the app is calm enough.
    ///
    /// The two gates are the ones the shell-wide clock applied to all five at once, so
    /// a panel-owned clock still honours them: no session means nothing to poll, and a
    /// deferral means come back next beat rather than submit now.
    pub(in crate::features) fn refresh_remote_monitor_if_due(
        &mut self,
        kind: RemoteMonitorKind,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.remote_monitor_session_active() || !self.remote_monitor_demand(kind) {
            return false;
        }
        if self.remote_refresh_is_deferred() {
            return false;
        }
        match kind {
            RemoteMonitorKind::Stats => {
                if self.panel_is_rendered(kind.nav_item()) || self.header_status_needs_stats() {
                    self.refresh_stats_if_due(cx)
                } else {
                    self.refresh_connection_icon_if_due(cx)
                }
            }
            RemoteMonitorKind::Gpu => self.refresh_gpu_if_due(cx),
            RemoteMonitorKind::Npu => self.refresh_npu_if_due(cx),
            RemoteMonitorKind::Processes => self.refresh_processes_if_due(cx),
            RemoteMonitorKind::Docker => self.refresh_docker_if_due(cx),
        }
    }

    /// Persistent panel entities own their clocks even when only the header or a
    /// bounded connection-icon probe consumes the data.
    pub(in crate::features) fn remote_monitor_demand(&self, kind: RemoteMonitorKind) -> bool {
        let shown = self.panel_is_rendered(kind.nav_item());
        match kind {
            RemoteMonitorKind::Stats => {
                shown || self.header_status_needs_stats() || self.connection_icon_probe_needed()
            }
            RemoteMonitorKind::Gpu => shown || self.header_status_needs_gpu(),
            RemoteMonitorKind::Npu => shown || self.header_status_needs_npu(),
            RemoteMonitorKind::Processes | RemoteMonitorKind::Docker => shown,
        }
    }

    fn remote_monitor_session_active(&self) -> bool {
        self.session.active_is_local_pty()
            || self
                .session
                .active_ssh_config()
                .is_some_and(|config| config.remote_stats_enabled())
                && self
                    .session
                    .active_id()
                    .and_then(|id| self.session.metadata(id))
                    .is_some_and(|metadata| !metadata.disconnected)
    }

    /// Rendering reconciles all layout consumers; session activation and replies
    /// also reconcile immediately, so no session or completed probe leaves a clock.
    /// This only updates task ownership; blocking work runs from the clock's jobs.
    pub(in crate::features) fn sync_remote_panel_demand(&mut self, cx: &mut Context<Self>) {
        let session_active = self.remote_monitor_session_active();
        for kind in RemoteMonitorKind::ALL {
            let demand = session_active && self.remote_monitor_demand(kind);
            let panel = self.remote_panels.entity(kind).clone();
            panel.update(cx, |panel, cx| panel.set_demand(demand, cx));
        }
        self.sync_host_tools(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RemoteMonitorData, RemoteMonitorKind, RemoteMonitorNavigation, RemoteMonitorPanel,
    };
    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::test_support::TestConfigDir;
    use gpui::{
        AppContext as _, Entity, IntoElement, ParentElement as _, Render, Styled as _,
        TestAppContext, VisualTestContext, div,
    };
    use nyaterm_core::{AppRuntime, RuntimeMode};
    use std::path::Path;

    const ALL_KINDS: [RemoteMonitorKind; 5] = [
        RemoteMonitorKind::Stats,
        RemoteMonitorKind::Gpu,
        RemoteMonitorKind::Npu,
        RemoteMonitorKind::Processes,
        RemoteMonitorKind::Docker,
    ];

    fn app(cx: &mut TestAppContext, root: &Path) -> Entity<NyaTermApp> {
        // A uuid rather than a clock reading: these tests run in parallel and
        // Windows' ~15ms clock granularity lets a nanosecond timestamp repeat,
        // which would share one config dir and so one settings database.
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.to_path_buf(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
    }

    struct PanelHost {
        panels: Vec<Entity<RemoteMonitorPanel>>,
    }

    impl Render for PanelHost {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div().size_full().children(
                self.panels
                    .iter()
                    .cloned()
                    .map(IntoElement::into_any_element),
            )
        }
    }

    #[test]
    fn host_token_rejects_changed_identity_before_and_after_snapshot_flush() {
        use nyaterm_remote_monitor_gpui::actions::RemoteMonitorAction;
        let test_dir = TestConfigDir::new("nyaterm-monitor-host-token");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());
        let token = cx.update_entity(&app, |app, cx| {
            app.flush_remote_panel_snapshots(cx);
            let token = app.remote_panels.host_token;
            assert!(app.handle_remote_monitor_action(
                RemoteMonitorAction::CheckHost(token),
                None,
                cx
            ));
            token
        });
        activate_ssh_session(&app, &mut cx);
        cx.update_entity(&app, |app, cx| {
            assert!(!app.handle_remote_monitor_action(
                RemoteMonitorAction::CheckHost(token),
                None,
                cx
            ));
            app.flush_remote_panel_snapshots(cx);
            assert!(!app.handle_remote_monitor_action(
                RemoteMonitorAction::CheckHost(token),
                None,
                cx
            ));
            let current = app.remote_panels.host_token;
            assert_ne!(token, current);
            assert!(app.handle_remote_monitor_action(
                RemoteMonitorAction::CheckHost(current),
                None,
                cx
            ));
        });
    }

    /// Five instances means five entity ids, which is what makes them isolated views
    /// rather than one view with a mode flag.
    #[test]
    fn every_kind_gets_its_own_entity() {
        let test_dir = TestConfigDir::new("nyaterm-remote-panels");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, _| {
            let mut ids = ALL_KINDS
                .iter()
                .map(|kind| app.remote_panels.entity(*kind).entity_id())
                .collect::<Vec<_>>();
            ids.sort_unstable();
            let distinct = ids.len();
            ids.dedup();
            assert_eq!(ids.len(), distinct, "each panel must be its own entity");
        });
    }

    /// Each panel must paint through its entity.
    ///
    /// The views moved from being built inline during the app's own render to being
    /// child views, which puts them behind GPUI's `with_rendered_view` boundary and
    /// changes the element-id path they live on. This draws all five to confirm that
    /// relocation did not break the ones that register text inputs or read theme
    /// caches the root chrome used to prime first.
    #[test]
    fn all_five_panels_paint_as_child_views() {
        let test_dir = TestConfigDir::new("nyaterm-remote-panels");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| app.sync_component_theme(cx));
        let panels = cx.update_entity(&app, |app, _| {
            ALL_KINDS
                .iter()
                .map(|kind| app.remote_panels.entity(*kind).clone())
                .collect::<Vec<_>>()
        });

        let (_, cx) = cx.add_window_view(move |_, _| PanelHost { panels });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
    }

    #[test]
    fn network_select_events_refresh_the_stats_snapshot_and_reconcile_removed_interfaces() {
        use crate::features::runtime_jobs::StatsJobResult;
        use nyaterm_remote_ops::stats::{NetworkInfo, NetworkSummaryInfo, RemoteStats};
        use nyaterm_ui::NyaSelectEvent;

        fn stats(include_eth1: bool) -> RemoteStats {
            let mut networks = vec![NetworkInfo {
                rate_status: nyaterm_remote_ops::stats::NetworkRateStatus::Available,
                nic: "eth0".to_string(),
                state: "up".to_string(),
                rx_bytes_per_sec: 10.,
                tx_bytes_per_sec: 20.,
            }];
            if include_eth1 {
                networks.push(NetworkInfo {
                    rate_status: nyaterm_remote_ops::stats::NetworkRateStatus::Available,
                    nic: "eth1".to_string(),
                    state: "up".to_string(),
                    rx_bytes_per_sec: 100.,
                    tx_bytes_per_sec: 200.,
                });
            }
            RemoteStats {
                network_summary: NetworkSummaryInfo {
                    rx_bytes_per_sec: networks
                        .iter()
                        .map(|network| network.rx_bytes_per_sec)
                        .sum(),
                    tx_bytes_per_sec: networks
                        .iter()
                        .map(|network| network.tx_bytes_per_sec)
                        .sum(),
                },
                networks,
                ..Default::default()
            }
        }

        let test_dir = TestConfigDir::new("nyaterm-resource-network");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());
        activate_ssh_session(&app, &mut cx);
        let (panel, select) = cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            for _ in 0..2 {
                let ticket = app
                    .remote_ops
                    .begin_stats_job("remote-panel-test".to_string(), false);
                app.remote_ops.apply_stats_event(
                    StatsJobResult {
                        users_revision: 0,
                        job_id: ticket.job_id,
                        session_id: "remote-panel-test".to_string(),
                        result: Ok(stats(true)),
                    },
                    Some("remote-panel-test"),
                );
            }
            app.flush_remote_panel_snapshots(cx);
            let panel = app.remote_panels.entity(RemoteMonitorKind::Stats).clone();
            let select = app.selects.field(&"remote.stats.network".into()).unwrap();
            assert_eq!(select.read(cx).selected_value(), Some("all"));
            (panel, select)
        });
        let host_panel = panel.clone();
        let (_, cx) = cx.add_window_view(move |_, _| PanelHost {
            panels: vec![host_panel],
        });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
            select.update(cx, |_, cx| {
                cx.emit(NyaSelectEvent::Changed(Some("nic:eth1".to_string())))
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
            let panel = panel.read(cx);
            let snapshot = panel.test_snapshot().unwrap();
            let RemoteMonitorData::Stats {
                state,
                network_select,
            } = &snapshot.data
            else {
                panic!("expected stats");
            };
            assert_eq!(state.selected_network_interface.as_deref(), Some("eth1"));
            assert_eq!(state.network_traffic().unwrap().rx_bytes_per_sec, 100.);
            assert_eq!(
                state.data.as_ref().unwrap().networks.len(),
                2,
                "selection must not filter interface rows"
            );
            assert_eq!(network_select.read(cx).selected_value(), Some("nic:eth1"));
            assert_eq!(
                snapshot.key.revision,
                app.read(cx).remote_ops.stats_revision()
            );
        });
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let ticket = app
                    .remote_ops
                    .begin_stats_job("remote-panel-test".to_string(), false);
                app.remote_ops.apply_stats_event(
                    StatsJobResult {
                        users_revision: 0,
                        job_id: ticket.job_id,
                        session_id: "remote-panel-test".to_string(),
                        result: Ok(stats(false)),
                    },
                    Some("remote-panel-test"),
                );
                app.flush_remote_panel_snapshots(cx);
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
            let panel = panel.read(cx);
            let RemoteMonitorData::Stats {
                state,
                network_select,
            } = &panel.test_snapshot().unwrap().data
            else {
                panic!("expected stats");
            };
            assert!(state.selected_network_interface.is_none());
            assert_eq!(state.network_traffic().unwrap().rx_bytes_per_sec, 10.);
            assert_eq!(network_select.read(cx).selected_value(), Some("all"));
        });
    }

    #[test]
    fn cpu_and_memory_details_toggle_from_card_headers() {
        use nyaterm_remote_ops::stats::{CpuCoreUsage, MemoryInfo, RemoteStats};

        let test_dir = TestConfigDir::new("nyaterm-stats-details");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());
        activate_ssh_session(&app, &mut cx);
        let panel = cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            let mut stats = RemoteStats::default();
            stats.cpu.cores = 2;
            stats.cpu.per_core = vec![
                CpuCoreUsage { id: 0, usage: 10. },
                CpuCoreUsage { id: 1, usage: 20. },
            ];
            stats.memory = MemoryInfo {
                used: 1024,
                available: 2048,
                free: Some(1024),
                buffers: Some(64),
                shared: Some(32),
                swap_total: Some(0),
                swap_used: Some(0),
                ..Default::default()
            };
            let ticket = app
                .remote_ops
                .begin_stats_job("remote-panel-test".into(), false);
            app.remote_ops.apply_stats_event(
                crate::features::runtime_jobs::StatsJobResult {
                    users_revision: 0,
                    job_id: ticket.job_id,
                    session_id: "remote-panel-test".into(),
                    result: Ok(stats),
                },
                Some("remote-panel-test"),
            );
            app.flush_remote_panel_snapshots(cx);
            app.remote_panels.entity(RemoteMonitorKind::Stats).clone()
        });
        let (_, vcx) = cx.add_window_view(move |_, _| PanelHost {
            panels: vec![panel],
        });
        let vcx: &mut VisualTestContext = vcx;
        for theme in ["dark", "light"] {
            vcx.update(|_, cx| {
                app.update(cx, |app, cx| {
                    app.settings.set_appearance_theme(theme.to_string());
                    app.sync_component_theme(cx);
                    app.flush_remote_panel_snapshots(cx);
                });
            });
            for width in [280., 400.] {
                vcx.simulate_resize(gpui::size(gpui::px(width), gpui::px(1600.)));
                vcx.run_until_parked();
                vcx.update(|window, cx| {
                    _ = window.draw(cx);
                });
                for (trigger, body, details) in [
                    (
                        "stats-cpu-cores-toggle-trigger",
                        "stats-cpu-body",
                        "stats-cpu-details",
                    ),
                    (
                        "stats-memory-toggle-trigger",
                        "stats-memory-body",
                        "stats-memory-details",
                    ),
                ] {
                    let bounds = vcx.debug_bounds(trigger).unwrap();
                    let body_bounds = vcx.debug_bounds(body).unwrap();
                    assert_eq!(
                        bounds.right(),
                        body_bounds.right(),
                        "{theme}: {trigger} at {width}"
                    );
                    assert!(bounds.bottom() <= body_bounds.top());
                    assert!(vcx.debug_bounds(details).is_none());
                    vcx.simulate_click(bounds.center(), gpui::Modifiers::default());
                    vcx.run_until_parked();
                    vcx.update(|window, cx| {
                        _ = window.draw(cx);
                    });
                    assert!(vcx.debug_bounds(details).is_some());
                    let bounds = vcx.debug_bounds(trigger).unwrap();
                    vcx.simulate_click(bounds.center(), gpui::Modifiers::default());
                    vcx.run_until_parked();
                    vcx.update(|window, cx| {
                        _ = window.draw(cx);
                    });
                    assert!(vcx.debug_bounds(details).is_none());
                }
            }
        }
    }

    #[test]
    fn network_chart_fits_and_rows_start_collapsed_then_preserve_folding_on_refresh() {
        use crate::features::runtime_jobs::StatsJobResult;
        use nyaterm_remote_ops::stats::{NetworkInfo, NetworkRateStatus, RemoteStats};
        use std::sync::Arc;
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let apply = |app: &mut NyaTermApp, count: usize| {
            let mut stats = RemoteStats::default();
            stats.network_sample_time.sampled_at = Some(now);
            stats.networks = (0..count)
                .map(|index| NetworkInfo {
                    nic: format!("eth{index:03}"),
                    state: "up".into(),
                    rx_bytes_per_sec: index as f64,
                    tx_bytes_per_sec: index as f64 * 2.,
                    rate_status: NetworkRateStatus::Available,
                })
                .collect();
            let ticket = app
                .remote_ops
                .begin_stats_job("remote-panel-test".into(), false);
            app.remote_ops.apply_stats_event(
                StatsJobResult {
                    users_revision: 0,
                    job_id: ticket.job_id,
                    session_id: "remote-panel-test".into(),
                    result: Ok(stats),
                },
                Some("remote-panel-test"),
            );
        };
        let test_dir = TestConfigDir::new("nyaterm-network-list");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());
        activate_ssh_session(&app, &mut cx);
        let panel = cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            apply(app, 40);
            app.flush_remote_panel_snapshots(cx);
            app.remote_panels.entity(RemoteMonitorKind::Stats).clone()
        });
        let host_panel = panel.clone();
        let (_, vcx) = cx.add_window_view(move |_, _| PanelHost {
            panels: vec![host_panel],
        });
        let vcx: &mut VisualTestContext = vcx;
        for theme in ["dark", "light"] {
            vcx.update(|_, cx| {
                app.update(cx, |app, cx| {
                    app.settings.set_appearance_theme(theme.to_string());
                    app.sync_component_theme(cx);
                    app.flush_remote_panel_snapshots(cx);
                });
            });
            for width in [280., 320., 400.] {
                vcx.simulate_resize(gpui::size(gpui::px(width), gpui::px(1600.)));
                vcx.run_until_parked();
                vcx.update(|window, cx| {
                    _ = window.draw(cx);
                });
                let metrics = vcx.debug_bounds("stats-network-metrics").unwrap();
                let frame = vcx.debug_bounds("stats-network-chart-frame").unwrap();
                let plot = vcx.debug_bounds("stats-network-chart-plot").unwrap();
                let ticks = vcx.debug_bounds("stats-network-chart-ticks").unwrap();
                let trigger = vcx
                    .debug_bounds("stats-network-interfaces-trigger")
                    .unwrap();
                let recent = vcx.debug_bounds("stats-network-recent-seconds").unwrap();
                assert_eq!(frame.left(), metrics.left(), "{theme} at {width}px");
                assert_eq!(frame.right(), metrics.right(), "{theme} at {width}px");
                assert!(frame.right() <= gpui::px(width));
                assert!(plot.size.width > frame.size.width * 0.8);
                assert_eq!(plot.left(), ticks.left());
                assert_eq!(plot.right(), ticks.right());
                if trigger.size.width + recent.size.width + gpui::px(8.) <= frame.size.width {
                    assert_eq!(
                        trigger.center().y,
                        recent.center().y,
                        "{theme} at {width}px"
                    );
                    assert!(trigger.right() < recent.left(), "{theme} at {width}px");
                } else {
                    assert!(recent.top() >= trigger.bottom(), "{theme} at {width}px");
                }
                assert_eq!(trigger.left(), frame.left());
                assert_eq!(recent.right(), frame.right());
                assert!(trigger.top() >= ticks.bottom());
                assert!(vcx.debug_bounds("stats-network-interface-list").is_none());
                assert!(vcx.debug_bounds("stats-network-empty").is_none());
            }
        }
        let recent = vcx.debug_bounds("stats-network-recent-seconds").unwrap();
        vcx.simulate_click(recent.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert!(vcx.debug_bounds("stats-network-interface-list").is_none());
        let toggle = |vcx: &mut VisualTestContext| {
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                _ = window.draw(cx);
            });
            let trigger = vcx
                .debug_bounds("stats-network-interfaces-trigger")
                .unwrap();
            vcx.simulate_click(trigger.center(), gpui::Modifiers::default());
            vcx.run_until_parked();
        };
        toggle(vcx);
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                apply(app, 40);
                app.flush_remote_panel_snapshots(cx);
                assert!(
                    app.remote_ops
                        .stats_presentation()
                        .network_interfaces_expanded
                );
            });
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert_eq!(
            vcx.debug_bounds("stats-network-interface-list")
                .unwrap()
                .size
                .height,
            gpui::px(120.)
        );
        let viewport = vcx.debug_bounds("stats-network-interface-list").unwrap();
        assert!(
            vcx.debug_bounds("stats-network-row-eth039")
                .unwrap()
                .bottom()
                > viewport.bottom()
        );
        vcx.simulate_event(gpui::ScrollEvent {
            position: viewport.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-2_000.))),
            ..Default::default()
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert!(
            vcx.debug_bounds("stats-network-row-eth039")
                .unwrap()
                .bottom()
                <= viewport.bottom()
        );
        vcx.update(|_, cx| {
            panel.update(cx, |panel, cx| {
                let RemoteMonitorData::Stats { state, .. } = &panel.test_snapshot().unwrap().data
                else {
                    panic!("stats")
                };
                let chart = state.network_chart.clone();
                let revision = panel.snapshot_revision();
                panel.advance_network_clock(now + Duration::from_secs(12), cx);
                let RemoteMonitorData::Stats { state, .. } = &panel.test_snapshot().unwrap().data
                else {
                    panic!("stats")
                };
                assert!(Arc::ptr_eq(&chart, &state.network_chart));
                assert_eq!(
                    revision,
                    panel.snapshot_revision(),
                    "a clock tick must not rebuild the data snapshot"
                );
                assert!(state.stale(panel.test_network_clock(), Duration::from_secs(3)));
            });
        });
        toggle(vcx);
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert!(vcx.debug_bounds("stats-network-interface-list").is_none());
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                apply(app, 1);
                app.flush_remote_panel_snapshots(cx);
                assert!(
                    !app.remote_ops
                        .stats_presentation()
                        .network_interfaces_expanded
                );
            });
        });
        toggle(vcx);
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert_eq!(
            vcx.debug_bounds("stats-network-interface-list")
                .unwrap()
                .size
                .height,
            gpui::px(24.)
        );
        assert!(vcx.debug_bounds("stats-network-interface-scroll").is_none());
        for count in [5, 6] {
            vcx.update(|_, cx| {
                app.update(cx, |app, cx| {
                    apply(app, count);
                    app.flush_remote_panel_snapshots(cx);
                });
            });
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                _ = window.draw(cx);
            });
            assert_eq!(
                vcx.debug_bounds("stats-network-interface-list")
                    .unwrap()
                    .size
                    .height,
                gpui::px(120.)
            );
            assert_eq!(
                vcx.debug_bounds("stats-network-interface-scroll").is_some(),
                count > 5
            );
        }
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                apply(app, 0);
                app.flush_remote_panel_snapshots(cx);
            });
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert!(vcx.debug_bounds("stats-network-interface-list").is_none());
        assert!(vcx.debug_bounds("stats-network-empty").is_some());
    }

    struct AppHost {
        app: Entity<NyaTermApp>,
    }

    impl Render for AppHost {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div().size_full().child(self.app.clone())
        }
    }

    /// The shell's panel dispatch must reach the entity.
    ///
    /// The test above renders the panels directly, which would keep passing if
    /// `panel_body` had been left pointing at the old inline views -- so it proves the
    /// entity works, not that anything uses it. This drives the real route:
    /// `render` -> `side_panel_stack` -> `right_panel_body` -> `panel_body`.
    ///
    /// The assertion is the entity's own paint count, not a side effect of the view it
    /// renders: the inline views this replaced register the same search-input ids, so
    /// an id-existence check would pass whether or not `panel_body` was ever switched
    /// over. Counting paints of the entity is the only thing that tells the two apart.
    /// The second half of each pair is what stops it passing vacuously -- it shows the
    /// right side really is rendering, and rendering the *other* panel.
    #[test]
    fn the_shell_panel_dispatch_renders_the_entity() {
        assert_eq!(
            painted_remote_panels(crate::models::NavItem::GpuMonitor),
            (true, false),
            "opening the GPU panel must paint the GPU entity and not the process one"
        );
        assert_eq!(
            painted_remote_panels(crate::models::NavItem::Processes),
            (false, true),
            "and opening the process panel must paint the other one"
        );
    }

    /// Open `panel`, paint the whole app, and report which of the GPU and process
    /// panel entities painted.
    fn painted_remote_panels(panel: crate::models::NavItem) -> (bool, bool) {
        let test_dir = TestConfigDir::new("nyaterm-remote-panels");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            // The real activation path, so this cannot pass by poking a combination of
            // panel fields the UI never produces.
            app.open_or_toggle_panel(panel, cx);
        });

        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |_, _| AppHost { app: host_app });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        // Twice: the first paint is what teaches `shell.viewport` the window size, and
        // the right side is only laid out inline once it is wide enough.
        for _ in 0..2 {
            cx.update(|window, cx| {
                app.update(cx, |_, cx| cx.notify());
                _ = window.draw(cx);
            });
            cx.run_until_parked();
        }

        cx.update(|_, cx| {
            let painted = |kind: RemoteMonitorKind| {
                app.read(cx)
                    .remote_panels
                    .entity(kind)
                    .read(cx)
                    .paint_count()
                    > 0
            };
            (
                painted(RemoteMonitorKind::Gpu),
                painted(RemoteMonitorKind::Processes),
            )
        })
    }

    /// Give the app an active session carrying an SSH config.
    ///
    /// `active_ssh_config` reads the active session's metadata, so this is the whole
    /// requirement. The host is empty because none of these tests advance the clock,
    /// so no refresh is ever submitted and nothing tries to connect.
    fn activate_ssh_session(app: &Entity<NyaTermApp>, cx: &mut TestAppContext) {
        cx.update_entity(app, |app, _| {
            app.session.register_session_metadata(
                "remote-panel-test",
                crate::models::SessionRuntimeMetadata {
                    confirmed_ssh_runtime_mode: None,
                    pending_ssh_startup_command: None,
                    ssh_config: Some(nyaterm_transport::SshSessionConfig::default()),
                    ssh_multiplex_key: None,
                    source_connection_id: None,
                    ai_execution_profile: nyaterm_core::AiExecutionProfile::Posix,
                    launch_config: crate::models::SessionLaunchConfig::Ssh(Box::default()),
                    disconnected: false,
                },
            );
            app.session
                .select_active_session("remote-panel-test".to_string());
        });
    }

    #[test]
    fn native_outside_press_refreshes_the_closed_docker_menu_snapshot() {
        let test_dir = TestConfigDir::new("nyaterm-remote-native-menu-snapshot");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());
        activate_ssh_session(&app, &mut cx);
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.remote_ops.toggle_docker_tab_menu();
            app.flush_remote_panel_snapshots(cx);
            let snapshot = app
                .remote_panels
                .entity(RemoteMonitorKind::Docker)
                .read(cx)
                .test_snapshot()
                .unwrap();
            let RemoteMonitorData::Docker(docker) = &snapshot.data else {
                panic!("Docker snapshot")
            };
            assert!(docker.state.tab_menu_open);
        });
        let app_for_view = app.clone();
        let (_, vcx) =
            cx.add_window_view(move |window, cx| nyaterm_ui::nya_root(app_for_view, window, cx));
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.handle_native_overlay_outside_mouse_down(
                    &gpui::MouseDownEvent {
                        button: gpui::MouseButton::Left,
                        position: gpui::point(gpui::px(600.), gpui::px(400.)),
                        ..Default::default()
                    },
                    window,
                    cx,
                );
            })
        });
        vcx.run_until_parked();
        vcx.read(|cx| {
            let app = app.read(cx);
            assert!(!app.remote_ops.docker_menus_open());
            let panel = app.remote_panels.entity(RemoteMonitorKind::Docker).read(cx);
            assert_eq!(
                panel.snapshot_key(),
                Some(app.remote_monitor_key(RemoteMonitorKind::Docker, cx))
            );
            let RemoteMonitorData::Docker(docker) = &panel.test_snapshot().unwrap().data else {
                panic!("Docker snapshot")
            };
            assert!(!docker.state.tab_menu_open);
        });
    }

    fn polling(app: &Entity<NyaTermApp>, cx: &mut TestAppContext) -> Vec<RemoteMonitorKind> {
        cx.update_entity(app, |app, cx| {
            RemoteMonitorKind::ALL
                .into_iter()
                .filter(|kind| app.remote_panels.entity(*kind).read(cx).is_polling())
                .collect()
        })
    }

    /// The state the app spends nearly all of its life in: nothing polls.
    #[test]
    fn nothing_polls_without_a_panel_or_a_session() {
        let test_dir = TestConfigDir::new("nyaterm-remote-panels");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| app.sync_remote_panel_demand(cx));
        assert_eq!(polling(&app, &mut cx), Vec::new());
    }

    #[test]
    fn every_header_mode_requests_only_the_data_it_displays() {
        let test_dir = TestConfigDir::new("nyaterm-monitor-header-demand");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());
        activate_ssh_session(&app, &mut cx);
        cx.update_entity(&app, |app, cx| {
            for (mode, expected) in [
                (crate::models::HeaderStatusMode::Session, None),
                (crate::models::HeaderStatusMode::DateTime, None),
                (
                    crate::models::HeaderStatusMode::Resources,
                    Some(RemoteMonitorKind::Stats),
                ),
                (
                    crate::models::HeaderStatusMode::Host,
                    Some(RemoteMonitorKind::Stats),
                ),
                (
                    crate::models::HeaderStatusMode::Gpu,
                    Some(RemoteMonitorKind::Gpu),
                ),
                (
                    crate::models::HeaderStatusMode::Npu,
                    Some(RemoteMonitorKind::Npu),
                ),
            ] {
                let mut summary = app.settings.summary().clone();
                summary.ui_header_status_visible = true;
                summary.ui_header_status_mode = mode.persistence_id().into();
                app.settings.replace_summary(summary);
                app.sync_remote_panel_demand(cx);
                for kind in RemoteMonitorKind::ALL {
                    assert_eq!(
                        app.remote_panels.entity(kind).read(cx).is_polling(),
                        expected == Some(kind),
                        "{mode:?}: {kind:?}"
                    );
                }
                let mut summary = app.settings.summary().clone();
                summary.ui_header_status_visible = false;
                app.settings.replace_summary(summary);
                app.sync_remote_panel_demand(cx);
                assert!(
                    RemoteMonitorKind::ALL.into_iter().all(|kind| !app
                        .remote_panels
                        .entity(kind)
                        .read(cx)
                        .is_polling())
                );
            }
        });
    }

    #[test]
    fn panels_poll_only_while_their_layout_is_actually_displayed() {
        use crate::models::{ActivityBarZone, NavItem, PanelOpenMode, PanelSide};
        let test_dir = TestConfigDir::new("nyaterm-monitor-layout-demand");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());
        activate_ssh_session(&app, &mut cx);
        cx.update_entity(&app, |app, cx| {
            for kind in RemoteMonitorKind::ALL {
                app.open_or_toggle_panel(kind.nav_item(), cx);
                assert!(app.remote_monitor_demand(kind));
                app.open_or_toggle_panel(kind.nav_item(), cx);
                assert!(!app.remote_monitor_demand(kind));
                assert!(!app.refresh_remote_monitor_if_due(kind, cx));
            }
            app.show_activity_entry("gpuMonitor".into(), cx);
            app.move_activity_entry("gpuMonitor".into(), ActivityBarZone::LeftTop, None, cx);
            app.open_or_toggle_panel(NavItem::GpuMonitor, cx);
            assert!(app.panel_is_rendered(NavItem::GpuMonitor));
            app.toggle_left_sidebar(cx);
            assert!(!app.panel_is_rendered(NavItem::GpuMonitor));
            app.toggle_left_sidebar(cx);
            app.ensure_panel_in_stack(NavItem::GpuMonitor, cx);
            assert!(app.panel_is_rendered(NavItem::GpuMonitor));
            app.open_page(NavItem::Settings, cx);
            assert!(
                app.panel_is_rendered(NavItem::GpuMonitor),
                "settings is a tab now and must not hide the side panels"
            );
            if !cfg!(target_os = "macos") {
                app.shell.set_viewport_size_for_test((600., 800.));
                app.shell.close_mobile_panels();
                assert!(!app.panel_is_rendered(NavItem::GpuMonitor));
                app.toggle_mobile_left_drawer(cx);
                assert!(app.panel_is_rendered(NavItem::GpuMonitor));
            }
            app.set_panel_open_mode(PanelOpenMode::Floating, cx);
            app.open_or_toggle_panel(NavItem::GpuMonitor, cx);
            assert_eq!(
                app.shell.floating_panel(PanelSide::Left),
                Some(NavItem::GpuMonitor)
            );
            assert!(app.panel_is_rendered(NavItem::GpuMonitor));
            app.open_or_toggle_panel(NavItem::GpuMonitor, cx);
            assert!(!app.panel_is_rendered(NavItem::GpuMonitor));
        });
    }

    /// An open panel polls, and only that one.
    ///
    /// The session is required: demand is gated on an active SSH config, so a panel
    /// open with no session still costs nothing.
    #[test]
    fn an_open_panel_polls_and_its_neighbours_do_not() {
        let test_dir = TestConfigDir::new("nyaterm-remote-panels");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            app.open_or_toggle_panel(crate::models::NavItem::GpuMonitor, cx);
            app.sync_remote_panel_demand(cx);
        });
        assert_eq!(
            polling(&app, &mut cx),
            Vec::new(),
            "no session means nothing to poll, however many panels are open"
        );

        activate_ssh_session(&app, &mut cx);
        cx.update_entity(&app, |app, cx| app.sync_remote_panel_demand(cx));
        assert_eq!(polling(&app, &mut cx), vec![RemoteMonitorKind::Gpu]);
    }

    /// Closing the panel must drop the clock, not merely mark it unwanted.
    ///
    /// This is the success criterion for the whole extraction: an inactive panel does
    /// not poll. It holds because the `Task` handle *is* the demand -- dropping it
    /// cancels the loop -- so there is no flag that can disagree with reality.
    #[test]
    fn closing_a_panel_drops_its_clock() {
        let test_dir = TestConfigDir::new("nyaterm-remote-panels");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());
        activate_ssh_session(&app, &mut cx);
        cx.update_entity(&app, |app, cx| {
            app.open_or_toggle_panel(crate::models::NavItem::GpuMonitor, cx);
            app.sync_remote_panel_demand(cx);
        });
        assert_eq!(polling(&app, &mut cx), vec![RemoteMonitorKind::Gpu]);

        cx.update_entity(&app, |app, cx| {
            // Toggling the same panel again closes it, which is what the activity bar
            // does.
            app.open_or_toggle_panel(crate::models::NavItem::GpuMonitor, cx);
            app.sync_remote_panel_demand(cx);
        });
        assert!(polling(&app, &mut cx).is_empty());
    }

    /// The header status bar keeps stats polling with the Stats panel closed.
    ///
    /// This is the case that decided the design. "Armed on mount, dropped on unmount"
    /// would stop the header updating as soon as its panel closed, because the header
    /// is a second, independent consumer of the same metric. It works because the
    /// entity is owned for the app's whole life and its demand is not "am I visible"
    /// but "does anything want this".
    #[test]
    fn the_header_keeps_a_metric_polling_with_its_panel_closed() {
        let test_dir = TestConfigDir::new("nyaterm-remote-panels");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());
        activate_ssh_session(&app, &mut cx);
        cx.update_entity(&app, |app, cx| {
            let mut summary = app.settings.summary().clone();
            summary.ui_header_status_visible = true;
            summary.ui_header_status_mode = crate::models::HeaderStatusMode::Resources
                .persistence_id()
                .to_string();
            app.settings.replace_summary(summary);
            app.sync_remote_panel_demand(cx);
        });
        assert!(
            !cx.update_entity(&app, |app, _| {
                app.panel_is_rendered(crate::models::NavItem::Stats)
            }),
            "the Stats panel must be closed for this test to mean anything"
        );
        assert_eq!(polling(&app, &mut cx), vec![RemoteMonitorKind::Stats]);

        // Switching away removes the final resource consumer.
        cx.update_entity(&app, |app, cx| {
            let mut summary = app.settings.summary().clone();
            summary.ui_header_status_mode = crate::models::HeaderStatusMode::Session
                .persistence_id()
                .to_string();
            app.settings.replace_summary(summary);
            app.sync_remote_panel_demand(cx);
        });
        assert!(polling(&app, &mut cx).is_empty());
    }

    /// Hidden entries remain usable through explicit panel management.
    #[test]
    fn a_hidden_entry_does_not_disable_its_explicitly_opened_panel() {
        let test_dir = TestConfigDir::new("nyaterm-remote-panels");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());
        activate_ssh_session(&app, &mut cx);
        cx.update_entity(&app, |app, cx| {
            app.open_or_toggle_panel(crate::models::NavItem::GpuMonitor, cx);
            app.sync_remote_panel_demand(cx);
        });
        assert_eq!(polling(&app, &mut cx), vec![RemoteMonitorKind::Gpu]);
    }

    /// A paint must arm the clock, with nothing calling the reconcile by hand.
    ///
    /// Every other test here calls `sync_remote_panel_demand` directly, and all of them
    /// keep passing with the call removed from `render` -- checked, not assumed. That is
    /// the same gap that let `3904c69b`'s dead clock through: they prove the mechanism and
    /// say nothing about the wiring. This one drives a real paint instead.
    ///
    /// It never advances the clock, so no refresh is submitted and nothing connects;
    /// arming is all that is being asserted.
    #[test]
    fn a_paint_arms_the_clock_for_an_open_panel() {
        let test_dir = TestConfigDir::new("nyaterm-remote-panels");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());
        activate_ssh_session(&app, &mut cx);
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.open_or_toggle_panel(crate::models::NavItem::GpuMonitor, cx);
        });
        assert_eq!(
            polling(&app, &mut cx),
            Vec::new(),
            "nothing has painted yet, so no clock can be armed"
        );

        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |_, _| AppHost { app: host_app });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();

        let armed = cx.update(|_, cx| {
            RemoteMonitorKind::ALL
                .into_iter()
                .filter(|kind| {
                    app.read(cx)
                        .remote_panels
                        .entity(*kind)
                        .read(cx)
                        .is_polling()
                })
                .collect::<Vec<_>>()
        });
        assert_eq!(
            armed,
            vec![RemoteMonitorKind::Gpu],
            "a paint arms only the visible GPU panel"
        );
    }
    #[test]
    fn ssh_compatibility_mode_blocks_panel_header_icon_and_manual_refresh_jobs() {
        let test_dir = TestConfigDir::new("nyaterm-ssh-compatibility-panels");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());
        activate_ssh_session(&app, &mut cx);
        cx.update_entity(&app, |app, cx| {
            let connection: nyaterm_core::SavedConnection = serde_json::from_value(serde_json::json!({
                "id":"icon-test", "name":"SSH", "type":"ssh", "host":"example.test", "icon_auto_detect":true
            })).unwrap();
            app.connection_state.replace_loaded(vec![connection], Vec::new());
            app.session.metadata_mut("remote-panel-test").unwrap().source_connection_id = Some("icon-test".into());
            let mut metadata = app.session.metadata("remote-panel-test").unwrap().clone();
            metadata.ssh_config.as_mut().unwrap().ssh_compatibility_mode = true;
            app.session.register_session_metadata("compatible-panel-test", metadata);
            app.session.select_active_session("compatible-panel-test");
            for mode in [crate::models::HeaderStatusMode::Resources, crate::models::HeaderStatusMode::Host,
                crate::models::HeaderStatusMode::Gpu, crate::models::HeaderStatusMode::Npu] {
                let mut settings = app.settings.summary().clone();
                settings.ui_header_status_visible = true;
                settings.ui_header_status_mode = mode.persistence_id().into();
                app.settings.replace_summary(settings);
                for kind in ALL_KINDS {
                    app.open_or_toggle_panel(kind.nav_item(), cx);
                    app.sync_remote_panel_demand(cx);
                    app.flush_remote_panel_snapshots(cx);
                    assert!(!app.remote_panels.entity(kind).read(cx).is_polling());
                    assert!(app.remote_panels.entity(kind).read(cx).test_snapshot().unwrap().key.ssh_compatibility_mode);
                    assert!(!app.refresh_remote_monitor_if_due(kind, cx));
                }
            }
            assert!(!app.connection_icon_probe_needed());
            assert!(!app.refresh_connection_icon_if_due(cx));
            assert!(!app.refresh_stats_if_due(cx));
            assert!(!app.refresh_gpu_if_due(cx));
            assert!(!app.refresh_npu_if_due(cx));
            assert!(!app.refresh_processes_if_due(cx));
            assert!(!app.refresh_docker_if_due(cx));
            app.refresh_stats(cx);
            app.refresh_gpu(cx);
            app.refresh_npu(cx);
            app.refresh_processes(cx);
            app.refresh_docker(cx);
            app.load_docker_details("container".into(), cx);
            assert!(app.shell.status().contains(&rust_i18n::t!("panel.sshCompatibilityModeUnavailable").to_string()));
            assert!(!app.remote_ops.stats_is_pending());
            assert!(!app.remote_ops.gpu_is_pending());
            assert!(!app.remote_ops.npu_is_pending());
            assert!(!app.remote_ops.process_is_pending());
            assert!(!app.remote_ops.docker_is_pending());
            app.session.select_active_session("remote-panel-test");
            app.sync_remote_panel_demand(cx);
            assert!(app.remote_panels.entity(RemoteMonitorKind::Npu).read(cx).is_polling());
            app.session.select_active_session("compatible-panel-test");
            app.sync_remote_panel_demand(cx);
            for kind in ALL_KINDS {
                assert!(!app.remote_panels.entity(kind).read(cx).is_polling());
            }
        });
    }
}

#[cfg(test)]
mod isolation_tests {
    use super::RemoteMonitorKind;
    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::features::runtime_jobs::DockerResource;
    use crate::models::{DockerTab, NavItem, SessionLaunchConfig, SessionRuntimeMetadata};
    use crate::test_support::TestConfigDir;
    use gpui::{
        AppContext as _, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
        VisualTestContext, div,
    };
    use nyaterm_core::{AiExecutionProfile, AppRuntime, RuntimeMode};
    use nyaterm_remote_ops::gpu::RemoteGpuOverview;
    use nyaterm_remote_ops::stats::RemoteStats;
    use nyaterm_transport::{LocalSessionConfig, SshSessionConfig};
    use nyaterm_ui::NyaInputEvent;
    use std::path::Path;

    fn app(cx: &mut TestAppContext, root: &Path) -> gpui::Entity<NyaTermApp> {
        // A uuid rather than a clock reading: these tests run in parallel and Windows'
        // ~15ms clock granularity lets a nanosecond timestamp repeat, which would share
        // one config dir and so one settings database.
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.to_path_buf(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
    }

    struct AppHost {
        app: gpui::Entity<NyaTermApp>,
    }

    impl Render for AppHost {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div().size_full().child(self.app.clone())
        }
    }

    fn paints(app: &gpui::Entity<NyaTermApp>, cx: &gpui::App) -> [usize; 3] {
        let count = |kind| {
            app.read(cx)
                .remote_panels
                .entity(kind)
                .read(cx)
                .paint_count()
        };
        [
            count(RemoteMonitorKind::Stats),
            count(RemoteMonitorKind::Gpu),
            count(RemoteMonitorKind::Npu),
        ]
    }

    /// Open the three panels on an SSH session and paint until things settle.
    fn hosted<'a>(
        cx: &'a mut TestAppContext,
        root: &Path,
    ) -> (gpui::Entity<NyaTermApp>, &'a mut VisualTestContext) {
        let app = app(cx, root);
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.session.register_session_metadata(
                "ssh-a",
                SessionRuntimeMetadata {
                    confirmed_ssh_runtime_mode: None,
                    pending_ssh_startup_command: None,
                    ssh_config: Some(SshSessionConfig::default()),
                    ssh_multiplex_key: None,
                    source_connection_id: None,
                    ai_execution_profile: AiExecutionProfile::Posix,
                    launch_config: SessionLaunchConfig::Local(LocalSessionConfig::default()),
                    disconnected: false,
                },
            );
            app.activate_session_id("ssh-a", cx);
            app.open_or_toggle_panel(NavItem::Stats, cx);
            app.flush_remote_panel_snapshots(cx);
        });
        let host_app = app.clone();
        let (_, vcx) = cx.add_window_view(move |_, _| AppHost { app: host_app });
        let vcx: &mut VisualTestContext = vcx;
        vcx.run_until_parked();
        for _ in 0..3 {
            vcx.update(|window, cx| {
                app.update(cx, |_, cx| cx.notify());
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
        }
        (app, vcx)
    }

    /// An `app.notify()` that changed nothing in a panel must not re-run its render.
    ///
    /// This is the whole point of the batch, and it is only true because the panel render
    /// makes **zero** `NyaTermApp` accesses: GPUI records every entity read during a
    /// view's render as a dependency of that view, so one app read -- diagnostics included
    /// -- would put the panel in `window.dirty_views` on every notify and the cached
    /// subtree would never be reused.
    ///
    /// It is also the `.cached()` measurement. If caching does not engage under the
    /// current parent sizing, content mask and text style, this fails.
    #[test]
    fn an_unrelated_app_notify_does_not_re_render_the_snapshot_panels() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted(&mut cx, test_dir.path());

        let before = vcx.update(|_, cx| paints(&app, cx));
        assert!(
            before[0] > 0,
            "the Stats panel must have painted at least once, or this test proves nothing"
        );
        for _ in 0..5 {
            vcx.update(|window, cx| {
                // Nothing about the remote panes changed; this is the shape of a terminal
                // output frame or a status text update.
                app.update(cx, |_, cx| cx.notify());
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
        }
        let after = vcx.update(|_, cx| paints(&app, cx));

        assert_eq!(
            before, after,
            "five unrelated app notifies must not re-render any snapshot panel"
        );
    }

    /// A stats mutation reaches its panel in the same update cycle, with no extra paint.
    ///
    /// The snapshot arrives synchronously: the flush runs inside the same outer `update`
    /// as the mutation, so the revision matches before anything draws. The repaint that
    /// follows is a consequence of the panel's own notify, not a precondition for
    /// freshness.
    #[test]
    fn a_stats_mutation_reaches_its_panel_without_an_extra_root_paint() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted(&mut cx, test_dir.path());

        let revision = vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops.apply_stats(RemoteStats::default());
                app.remote_ops.set_stats_status("loaded stats");
                // The boundary an event drain would use.
                app.flush_remote_panel_snapshots(cx);
                app.remote_ops.stats_revision()
            })
        });

        let delivered = vcx.update(|_, cx| {
            app.read(cx)
                .remote_panels
                .entity(RemoteMonitorKind::Stats)
                .read(cx)
                .snapshot_revision()
        });
        assert_eq!(
            delivered,
            Some(revision),
            "the snapshot must be current before any paint"
        );
    }

    /// A panel interaction must not flush while its own entity is leased by GPUI.
    #[test]
    fn a_panel_interaction_flushes_after_its_lease_is_released() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted(&mut cx, test_dir.path());
        let panel = vcx.update(|_, cx| {
            app.read(cx)
                .remote_panels
                .entity(RemoteMonitorKind::Stats)
                .clone()
        });
        for action in [
            nyaterm_remote_monitor_gpui::actions::RemoteMonitorAction::ToggleCpu,
            nyaterm_remote_monitor_gpui::actions::RemoteMonitorAction::ToggleMemory,
        ] {
            let before = vcx.update(|_, cx| panel.read(cx).snapshot_revision());

            vcx.update(|_, cx| {
                panel.update(cx, |panel, cx| {
                    panel.test_dispatch(action, cx);
                    assert_eq!(
                        panel.snapshot_revision(),
                        before,
                        "the snapshot must remain unchanged until the panel lease is released"
                    );
                });
            });
            vcx.run_until_parked();

            let after = vcx.update(|_, cx| panel.read(cx).snapshot_revision());
            assert!(
                after > before,
                "the deferred interaction flush must publish the changed stats state"
            );
        }
    }

    /// A GPU mutation must not re-render Stats or NPU.
    #[test]
    fn a_gpu_mutation_does_not_re_render_stats_or_npu() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted(&mut cx, test_dir.path());

        let before = vcx.update(|_, cx| paints(&app, cx));
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops.apply_gpu(
                    "ssh-a",
                    RemoteGpuOverview {
                        available: true,
                        ..Default::default()
                    },
                );
                app.flush_remote_panel_snapshots(cx);
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        let after = vcx.update(|_, cx| paints(&app, cx));

        assert_eq!(
            (after[0], after[2]),
            (before[0], before[2]),
            "a GPU refresh must leave the Stats and NPU panels alone"
        );
    }

    /// Typing in the GPU search field invalidates the GPU panel and nothing else.
    ///
    /// The panel holds the field's `Entity<NyaInputState>` from its snapshot, so reading
    /// it during render makes the panel a dependent of that entity -- which is the wanted
    /// direction of coupling. No app render is involved.
    #[test]
    fn typing_in_the_gpu_search_invalidates_only_that_panel() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted(&mut cx, test_dir.path());
        // The GPU panel has to be the one on screen for its search field to be built.
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_or_toggle_panel(NavItem::GpuMonitor, cx);
                app.flush_remote_panel_snapshots(cx);
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();

        let before = vcx.update(|_, cx| paints(&app, cx));
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                // What the field's subscription does on a keystroke.
                app.remote_ops.apply_gpu_search("py".to_string());
                app.flush_remote_panel_snapshots(cx);
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        let after = vcx.update(|_, cx| paints(&app, cx));

        assert!(
            after[1] > before[1],
            "the GPU panel must re-render for its own search change"
        );
        assert_eq!(
            (after[0], after[2]),
            (before[0], before[2]),
            "and the other two must not"
        );
    }

    /// A remote search subscription must publish its panel snapshot after the input event.
    #[test]
    fn remote_search_input_reaches_its_snapshot_after_the_subscription_runs() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted(&mut cx, test_dir.path());
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_or_toggle_panel(NavItem::GpuMonitor, cx);
                app.flush_remote_panel_snapshots(cx);
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();

        let field = vcx.update(|_, cx| {
            app.read(cx)
                .existing_text_input("remote.gpu.filter")
                .expect("the GPU panel should own its search field")
        });
        let before = vcx.update(|_, cx| {
            app.read(cx)
                .remote_panels
                .entity(RemoteMonitorKind::Gpu)
                .read(cx)
                .snapshot_revision()
        });

        vcx.update(|_, cx| {
            field.update(cx, |_, cx| {
                cx.emit(NyaInputEvent::Changed("py".to_string()));
            });
            assert_eq!(
                app.read(cx)
                    .remote_panels
                    .entity(RemoteMonitorKind::Gpu)
                    .read(cx)
                    .snapshot_revision(),
                before,
                "the snapshot must remain unchanged until the deferred flush runs"
            );
        });
        vcx.run_until_parked();

        let after = vcx.update(|_, cx| {
            app.read(cx)
                .remote_panels
                .entity(RemoteMonitorKind::Gpu)
                .read(cx)
                .snapshot_revision()
        });
        assert!(
            after > before,
            "the remote search subscription must publish its changed state"
        );
    }

    #[test]
    fn process_sort_select_is_idempotent_and_direction_remains_independent() {
        let mut cx = TestAppContext::single();
        let root = TestConfigDir::new("nyaterm-process-sort-select");
        let (app, vcx) = hosted_processes(&mut cx, root.path());
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let key = crate::models::RemoteProcessSortKey::Pid;
                app.on_select_changed("remote.process.sort", Some(key.label()), cx);
                let direction = app.remote_ops.process_presentation().sort_direction;
                assert_eq!(app.remote_ops.process_presentation().sort_key, key);
                app.on_select_changed("remote.process.sort", Some(key.label()), cx);
                assert_eq!(
                    app.remote_ops.process_presentation().sort_direction,
                    direction
                );
                app.toggle_process_sort(key, cx);
                assert_ne!(
                    app.remote_ops.process_presentation().sort_direction,
                    direction
                );
                app.flush_remote_panel_snapshots(cx);
                assert_eq!(
                    app.selects
                        .field(&"remote.process.sort".into())
                        .unwrap()
                        .read(cx)
                        .selected_value(),
                    Some(key.label())
                );
            })
        });
    }

    /// Open the Processes panel on an SSH session and settle.
    fn hosted_processes<'a>(
        cx: &'a mut TestAppContext,
        root: &Path,
    ) -> (gpui::Entity<NyaTermApp>, &'a mut VisualTestContext) {
        let app = app(cx, root);
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.session.register_session_metadata(
                "ssh-a",
                SessionRuntimeMetadata {
                    confirmed_ssh_runtime_mode: None,
                    pending_ssh_startup_command: None,
                    ssh_config: Some(SshSessionConfig::default()),
                    ssh_multiplex_key: None,
                    source_connection_id: None,
                    ai_execution_profile: AiExecutionProfile::Posix,
                    launch_config: SessionLaunchConfig::Local(LocalSessionConfig::default()),
                    disconnected: false,
                },
            );
            app.activate_session_id("ssh-a", cx);
            app.open_or_toggle_panel(NavItem::Processes, cx);
            app.flush_remote_panel_snapshots(cx);
        });
        let host_app = app.clone();
        let (_, vcx) = cx.add_window_view(move |_, _| AppHost { app: host_app });
        let vcx: &mut VisualTestContext = vcx;
        vcx.run_until_parked();
        for _ in 0..3 {
            vcx.update(|window, cx| {
                app.update(cx, |_, cx| cx.notify());
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
        }
        (app, vcx)
    }

    fn process_paints(app: &gpui::Entity<NyaTermApp>, cx: &gpui::App) -> usize {
        app.read(cx)
            .remote_panels
            .entity(RemoteMonitorKind::Processes)
            .read(cx)
            .paint_count()
    }

    fn process(pid: u32) -> nyaterm_remote_ops::remote_process::RemoteProcess {
        nyaterm_remote_ops::remote_process::RemoteProcess {
            pid,
            ppid: 1,
            user: "user".to_string(),
            state: "S".to_string(),
            cpu_percent: 1.0,
            memory_percent: 2.0,
            rss_kb: 3,
            vsz_kb: 4,
            elapsed: "00:01".to_string(),
            command: "sleep".to_string(),
            command_line: "sleep 10".to_string(),
        }
    }

    /// An `app.notify()` that changed nothing in the process pane must not re-render it.
    #[test]
    fn an_unrelated_app_notify_does_not_re_render_processes() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_processes(&mut cx, test_dir.path());

        let before = vcx.update(|_, cx| process_paints(&app, cx));
        assert!(
            before > 0,
            "the Processes panel must have painted at least once, or this proves nothing"
        );
        for _ in 0..5 {
            vcx.update(|window, cx| {
                app.update(cx, |_, cx| cx.notify());
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
        }
        assert_eq!(
            vcx.update(|_, cx| process_paints(&app, cx)),
            before,
            "five unrelated app notifies must not re-render the Processes panel"
        );
    }

    /// A process mutation reaches its panel before anything draws.
    #[test]
    fn a_process_mutation_reaches_its_panel_without_an_extra_root_paint() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_processes(&mut cx, test_dir.path());

        let revision = vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops
                    .apply_processes((0..4).map(process).collect());
                app.flush_remote_panel_snapshots(cx);
                app.remote_ops.process_revision()
            })
        });
        assert_eq!(
            vcx.update(|_, cx| {
                app.read(cx)
                    .remote_panels
                    .entity(RemoteMonitorKind::Processes)
                    .read(cx)
                    .snapshot_revision()
            }),
            Some(revision),
            "the snapshot must be current before any paint"
        );
    }

    #[test]
    fn process_menu_and_priority_controls_follow_the_captured_capabilities() {
        use super::RemoteMonitorData;
        use nyaterm_remote_ops::process_control::ProcessControlCapabilities;
        let test_dir = TestConfigDir::new("nyaterm-process-capabilities");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_processes(&mut cx, test_dir.path());
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops.apply_processes(vec![process(42)]);
                app.remote_ops.toggle_process_selection(42);
                app.remote_ops.toggle_process_menu(42);
                app.flush_remote_panel_snapshots(cx);
            });
        });
        for capabilities in [
            ProcessControlCapabilities::TerminateOnly,
            ProcessControlCapabilities::Posix,
        ] {
            vcx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    // Build the production snapshot, substituting the host capability
                    // to verify both menus on every runner, including Linux.
                    let mut key = app.remote_monitor_key(RemoteMonitorKind::Processes, cx);
                    key.process_control = capabilities;
                    let mut snapshot =
                        app.build_remote_monitor_snapshot(RemoteMonitorKind::Processes, key, cx);
                    if capabilities == ProcessControlCapabilities::TerminateOnly
                        && let RemoteMonitorData::Processes { nice, .. } = &mut snapshot.data
                    {
                        *nice = None;
                    }
                    app.remote_panels
                        .entity(RemoteMonitorKind::Processes)
                        .update(cx, |panel, cx| {
                            panel.set_demand(false, cx);
                            panel.set_snapshot(snapshot, cx);
                        });
                });
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
            assert!(vcx.debug_bounds("process-kill-42").is_some());
            for selector in [
                "process-term-42",
                "process-hup-42",
                "process-stop-42",
                "process-cont-42",
                "process-priority-controls",
            ] {
                assert_eq!(
                    vcx.debug_bounds(selector).is_some(),
                    capabilities == ProcessControlCapabilities::Posix,
                    "{selector}"
                );
            }
        }
    }

    /// A sibling pane's data must not re-render Processes.
    #[test]
    fn a_stats_mutation_does_not_re_render_processes() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_processes(&mut cx, test_dir.path());

        let before = vcx.update(|_, cx| process_paints(&app, cx));
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops.apply_stats(RemoteStats::default());
                app.flush_remote_panel_snapshots(cx);
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        assert_eq!(
            vcx.update(|_, cx| process_paints(&app, cx)),
            before,
            "a stats refresh must leave the Processes panel alone"
        );
    }

    /// Search, sort and the nice field each re-render only the Processes panel.
    #[test]
    fn process_interactions_update_only_the_process_panel() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_processes(&mut cx, test_dir.path());
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops
                    .apply_processes((0..8).map(process).collect());
                app.flush_remote_panel_snapshots(cx);
            });
        });

        for (label, mutate) in [
            (
                "search",
                &(|app: &mut NyaTermApp| app.remote_ops.apply_process_search("sleep".to_string()))
                    as &dyn Fn(&mut NyaTermApp),
            ),
            ("sort", &|app: &mut NyaTermApp| {
                app.remote_ops
                    .toggle_process_sort(crate::models::RemoteProcessSortKey::Pid)
            }),
            ("selection", &|app: &mut NyaTermApp| {
                app.remote_ops.toggle_process_selection(3)
            }),
            ("nice input", &|app: &mut NyaTermApp| {
                app.remote_ops.apply_process_nice_input("-5".to_string())
            }),
        ] {
            let before = vcx.update(|_, cx| (process_paints(&app, cx), paints(&app, cx)));
            vcx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    mutate(app);
                    app.flush_remote_panel_snapshots(cx);
                });
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
            let after = vcx.update(|_, cx| (process_paints(&app, cx), paints(&app, cx)));
            assert!(
                after.0 > before.0,
                "{label} must re-render the Processes panel"
            );
            assert_eq!(
                after.1, before.1,
                "{label} must not re-render Stats, GPU or NPU"
            );
        }
    }

    /// Sorting by memory then narrowing the panel preserves the selected ordering, and the
    /// snapshot follows -- the width is part of the key, so no pane revision has to move.
    #[test]
    fn narrowing_the_panel_preserves_memory_sort_in_the_snapshot() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_processes(&mut cx, test_dir.path());
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops
                    .apply_processes((0..4).map(process).collect());
                app.shell.set_right_panel_width_for_test(700.);
                app.remote_ops
                    .toggle_process_sort(crate::models::RemoteProcessSortKey::Memory);
                app.flush_remote_panel_snapshots(cx);
                assert_eq!(
                    app.remote_ops.process_presentation().sort_key,
                    crate::models::RemoteProcessSortKey::Memory
                );
            });
        });

        let before = vcx.update(|_, cx| process_paints(&app, cx));
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                // What a resize drag does, one move at a time.
                app.shell.set_right_panel_width_for_test(360.);
                app.flush_remote_panel_snapshots(cx);
                assert_eq!(
                    app.remote_ops.process_presentation().sort_key,
                    crate::models::RemoteProcessSortKey::Memory,
                    "sorting remains available in the narrow layout"
                );
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        assert!(
            vcx.update(|_, cx| process_paints(&app, cx)) > before,
            "the narrower width must reach the panel, even though it is not pane state"
        );
    }

    #[test]
    fn accelerator_snapshots_track_width_without_data_changes() {
        let test_dir = TestConfigDir::new("nyaterm-accelerator-width");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            for width in [200., 359., 360., 519., 520., 720.] {
                app.shell.set_right_panel_width_for_test(width);
                app.flush_remote_panel_snapshots(cx);
                for kind in [RemoteMonitorKind::Gpu, RemoteMonitorKind::Npu] {
                    let panel = app.remote_panels.entity(kind).read(cx);
                    assert_eq!(
                        panel.test_snapshot().expect("snapshot").key.panel_width,
                        width
                    );
                }
            }
        });
    }

    #[test]
    fn expanded_process_height_is_remeasured_within_the_same_width_mode() {
        let test_dir = TestConfigDir::new("nyaterm-process-wrap");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_processes(&mut cx, test_dir.path());
        let mut heights = Vec::new();
        for width in [720., 600.] {
            vcx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    if app.remote_ops.process_presentation().selected_pid.is_none() {
                        let mut row = process(1);
                        row.command_line = "long-command-without-spaces/".repeat(100);
                        app.remote_ops.apply_processes(vec![row]);
                        app.remote_ops.toggle_process_selection(1);
                    }
                    app.shell.set_right_panel_width_for_test(width);
                    app.flush_remote_panel_snapshots(cx);
                });
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
            heights.push(vcx.update(|_, cx| {
                app.read(cx)
                    .remote_panels
                    .entity(RemoteMonitorKind::Processes)
                    .read(cx)
                    .test_process_row_bounds(0)
                    .expect("expanded row")
                    .size
                    .height
            }));
        }
        assert!(
            heights[1] > heights[0],
            "a narrower command must wrap to more lines: {heights:?}"
        );
    }

    /// The process panel owns a variable-height GPUI list and keeps its item count in
    /// step with snapshots without routing scroll state through `RemoteOpsFeatureState`.
    #[test]
    fn process_list_state_tracks_snapshots_and_clamps_a_shorter_list() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_processes(&mut cx, test_dir.path());
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops
                    .apply_processes((0..100).map(process).collect());
                app.flush_remote_panel_snapshots(cx);
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let panel = app
                .read(cx)
                .remote_panels
                .entity(RemoteMonitorKind::Processes)
                .read(cx);
            assert_eq!(panel.test_process_count(), 100);
            assert!(panel.test_process_max_scroll() > gpui::px(0.));
            panel.test_scroll_process_to(gpui::ListOffset {
                item_ix: 60,
                offset_in_item: gpui::px(0.),
            });
        });

        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops
                    .apply_processes((0..3).map(process).collect());
                app.flush_remote_panel_snapshots(cx);
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let panel = app
                .read(cx)
                .remote_panels
                .entity(RemoteMonitorKind::Processes)
                .read(cx);
            assert_eq!(panel.test_process_count(), 3);
            assert!(panel.test_process_top_index() < 3);
            assert_eq!(panel.test_process_max_scroll(), gpui::px(0.));
        });
    }

    /// Open the Docker panel on an SSH session, with an overview, and settle.
    fn hosted_docker<'a>(
        cx: &'a mut TestAppContext,
        root: &Path,
    ) -> (gpui::Entity<NyaTermApp>, &'a mut VisualTestContext) {
        let app = app(cx, root);
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.session.register_session_metadata(
                "ssh-a",
                SessionRuntimeMetadata {
                    confirmed_ssh_runtime_mode: None,
                    pending_ssh_startup_command: None,
                    ssh_config: Some(SshSessionConfig::default()),
                    ssh_multiplex_key: None,
                    source_connection_id: None,
                    ai_execution_profile: AiExecutionProfile::Posix,
                    launch_config: SessionLaunchConfig::Local(LocalSessionConfig::default()),
                    disconnected: false,
                },
            );
            app.activate_session_id("ssh-a", cx);
            app.open_or_toggle_panel(NavItem::Docker, cx);
            app.remote_ops.apply_docker_overview(docker_overview(3));
            app.flush_remote_panel_snapshots(cx);
        });
        let host_app = app.clone();
        let (_, vcx) = cx.add_window_view(move |_, _| AppHost { app: host_app });
        let vcx: &mut VisualTestContext = vcx;
        vcx.run_until_parked();
        for _ in 0..3 {
            vcx.update(|window, cx| {
                app.update(cx, |_, cx| cx.notify());
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
        }
        (app, vcx)
    }

    fn docker_overview(containers: usize) -> nyaterm_remote_ops::docker::RemoteDockerOverview {
        nyaterm_remote_ops::docker::RemoteDockerOverview {
            available: true,
            compose_available: true,
            containers: (0..containers)
                .map(|index| nyaterm_remote_ops::docker::DockerContainer {
                    id: format!("c{index}"),
                    name: format!("name{index}"),
                    image: "image".to_string(),
                    status: "Up".to_string(),
                    state: "running".to_string(),
                    ports: String::new(),
                    created_at: String::new(),
                    size: String::new(),
                    stats: None,
                })
                .collect(),
            images: (0..30)
                .map(|index| nyaterm_remote_ops::docker::DockerImage {
                    id: format!("i{index}"),
                    repository: "repo".to_string(),
                    tag: "latest".to_string(),
                    size: String::new(),
                    created_since: String::new(),
                })
                .collect(),
            ..Default::default()
        }
    }

    fn docker_paints(app: &gpui::Entity<NyaTermApp>, cx: &gpui::App) -> usize {
        app.read(cx)
            .remote_panels
            .entity(RemoteMonitorKind::Docker)
            .read(cx)
            .paint_count()
    }

    /// An `app.notify()` that changed nothing in the Docker pane must not re-render it.
    #[test]
    fn an_unrelated_app_notify_does_not_re_render_docker() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_docker(&mut cx, test_dir.path());

        let before = vcx.update(|_, cx| docker_paints(&app, cx));
        assert!(
            before > 0,
            "the Docker panel must have painted at least once, or this proves nothing"
        );
        for _ in 0..5 {
            vcx.update(|window, cx| {
                app.update(cx, |_, cx| cx.notify());
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
        }
        assert_eq!(
            vcx.update(|_, cx| docker_paints(&app, cx)),
            before,
            "five unrelated app notifies must not re-render the Docker panel"
        );
    }

    /// A Docker mutation reaches its panel before anything draws.
    #[test]
    fn a_docker_mutation_reaches_its_panel_without_an_extra_root_paint() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_docker(&mut cx, test_dir.path());

        let revision = vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops.apply_docker_overview(docker_overview(7));
                app.flush_remote_panel_snapshots(cx);
                app.remote_ops.docker_revision()
            })
        });
        assert_eq!(
            vcx.update(|_, cx| {
                app.read(cx)
                    .remote_panels
                    .entity(RemoteMonitorKind::Docker)
                    .read(cx)
                    .snapshot_revision()
            }),
            Some(revision),
            "the snapshot must be current before any paint"
        );
    }

    /// A sibling pane's data must not re-render Docker.
    #[test]
    fn a_stats_mutation_does_not_re_render_docker() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_docker(&mut cx, test_dir.path());

        let before = vcx.update(|_, cx| docker_paints(&app, cx));
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops.apply_stats(RemoteStats::default());
                app.flush_remote_panel_snapshots(cx);
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        assert_eq!(
            vcx.update(|_, cx| docker_paints(&app, cx)),
            before,
            "a stats refresh must leave the Docker panel alone"
        );
    }

    /// Search, tab, menus and compose expansion each re-render only Docker.
    #[test]
    fn docker_interactions_update_only_the_docker_panel() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_docker(&mut cx, test_dir.path());

        for (label, mutate) in [
            (
                "search",
                &(|app: &mut NyaTermApp| app.remote_ops.apply_docker_search("name1".to_string()))
                    as &dyn Fn(&mut NyaTermApp),
            ),
            ("tab", &|app: &mut NyaTermApp| {
                app.remote_ops.set_docker_tab(DockerTab::Images)
            }),
            ("container menu", &|app: &mut NyaTermApp| {
                app.remote_ops
                    .toggle_docker_container_menu("c0".to_string())
            }),
            ("header menu", &|app: &mut NyaTermApp| {
                app.remote_ops.toggle_docker_header_menu()
            }),
            ("compose project", &|app: &mut NyaTermApp| {
                app.remote_ops
                    .toggle_compose_project("k".to_string(), "proj");
            }),
        ] {
            let before = vcx.update(|_, cx| (docker_paints(&app, cx), paints(&app, cx)));
            vcx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    mutate(app);
                    app.flush_remote_panel_snapshots(cx);
                });
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
            let after = vcx.update(|_, cx| (docker_paints(&app, cx), paints(&app, cx)));
            assert!(
                after.0 > before.0,
                "{label} must re-render the Docker panel"
            );
            assert_eq!(
                after.1, before.1,
                "{label} must not re-render Stats, GPU or NPU"
            );
        }
    }

    #[test]
    fn duplicate_docker_image_ids_keep_accessibility_nodes_unique() {
        let test_dir = TestConfigDir::new("nyaterm-docker-image-a11y");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_docker(&mut cx, test_dir.path());

        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let mut overview = docker_overview(2);
                for (index, image) in overview.images.iter_mut().take(3).enumerate() {
                    image.id = "sha256:shared-image-id".to_string();
                    image.repository = format!("shared-repository-{index}");
                }
                app.remote_ops.apply_docker_overview(overview);
                app.remote_ops.set_docker_tab(DockerTab::Images);
                app.flush_remote_panel_snapshots(cx);
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
    }

    /// The compose fallback survives the move into the snapshot: a host without compose
    /// support shows Containers even with Compose stored as the tab.
    #[test]
    fn the_compose_tab_falls_back_in_the_snapshot() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_docker(&mut cx, test_dir.path());

        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops.set_docker_tab(DockerTab::Compose);
                app.flush_remote_panel_snapshots(cx);
                assert_eq!(
                    app.remote_ops.docker_effective_tab(),
                    DockerTab::Compose,
                    "the fixture host supports compose"
                );

                // A refresh from a host without compose support.
                let mut overview = docker_overview(2);
                overview.compose_available = false;
                app.remote_ops.apply_docker_overview(overview);
                app.flush_remote_panel_snapshots(cx);
                assert_eq!(
                    app.remote_ops.docker_effective_tab(),
                    DockerTab::Containers,
                    "compose is unavailable, so the effective tab falls back"
                );
            });
        });
    }

    /// Docker's fixed-height virtual lists expose real GPUI scroll handles, and the
    /// handle becomes non-scrollable again when refreshed data fits the viewport.
    #[test]
    fn docker_resource_list_uses_a_native_scroll_handle() {
        let test_dir = TestConfigDir::new("nyaterm-panel-isolation");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted_docker(&mut cx, test_dir.path());

        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops.apply_docker_overview(docker_overview(2));
                app.remote_ops.apply_docker_resource(DockerResource::Images(
                    (0..100)
                        .map(|index| nyaterm_remote_ops::docker::DockerImage {
                            id: format!("image-{index}"),
                            repository: format!("repo-{index}"),
                            tag: "latest".to_string(),
                            created_since: "now".to_string(),
                            size: "1 MB".to_string(),
                        })
                        .collect(),
                ));
                app.remote_ops.set_docker_tab(DockerTab::Images);
                app.flush_remote_panel_snapshots(cx);
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let panel = app
                .read(cx)
                .remote_panels
                .entity(RemoteMonitorKind::Docker)
                .clone();
            panel.update(cx, |panel, cx| {
                assert!(panel.test_docker_resources_scrollable());
                panel.test_scroll_docker_resources_to(60);
                cx.notify();
            });
        });
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let panel = app
                .read(cx)
                .remote_panels
                .entity(RemoteMonitorKind::Docker)
                .read(cx);
            assert!(panel.test_docker_resource_offset() < gpui::px(0.));
        });

        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.remote_ops
                    .apply_docker_resource(DockerResource::Images(vec![
                        nyaterm_remote_ops::docker::DockerImage {
                            id: "image-0".to_string(),
                            repository: "repo-0".to_string(),
                            tag: "latest".to_string(),
                            created_since: "now".to_string(),
                            size: "1 MB".to_string(),
                        },
                    ]));
                app.flush_remote_panel_snapshots(cx);
            });
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let panel = app
                .read(cx)
                .remote_panels
                .entity(RemoteMonitorKind::Docker)
                .read(cx);
            assert!(!panel.test_docker_resources_scrollable());
            assert_eq!(panel.test_docker_resource_offset(), gpui::px(0.));
        });
    }
}
