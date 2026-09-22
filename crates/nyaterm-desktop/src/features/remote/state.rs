//! Grouped Remote page state: Docker, process table and host stats.
//!
//! Each of the three panes owns the same shape of refresh bookkeeping (job id,
//! owning session, pending flag, failure streak, last refresh instant) plus its
//! own view state. Keeping them in one struct per pane makes that symmetry
//! visible instead of spreading fifty-five prefixed fields across `NyaTermApp`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use futures::channel::mpsc::UnboundedReceiver;
use std::time::{Duration, Instant};

use nyaterm_transport::{
    CpuUsageSource, DockerComposeProject, DockerComposeService, DockerContainer,
    DockerContainerDetails, DockerImage, DockerNetwork, DockerVolume, RemoteDockerOverview,
    RemoteGpuOverview, RemoteGpuProcess, RemoteNpuOverview, RemoteNpuProcess, RemoteProcess,
    RemoteStats, RemoteStatsSampler,
};

use crate::features::formatting::docker_compose_project_key;
use crate::features::remote::job_state::{RemoteJobState, RemoteJobTicket};
use crate::features::remote::list_window::{ACCELERATOR_PROCESS_VIEWPORT_ROWS, max_list_offset};
use crate::features::{
    runtime_jobs::DockerJobResult, runtime_jobs::DockerResource, runtime_jobs::GpuJobResult,
    runtime_jobs::NpuJobResult, runtime_jobs::ProcessJobOutput, runtime_jobs::ProcessJobResult,
    runtime_jobs::StatsJobResult,
};
use crate::models::{DockerTab, RemoteProcessSortDirection, RemoteProcessSortKey};

pub(in crate::features) enum StatsApplyOutcome {
    Ignored,
    CompletedInactive,
    Applied {
        session_id: String,
        stats: Box<RemoteStats>,
        status: String,
    },
    Failed {
        status: String,
    },
}

pub(in crate::features) enum ProcessApplyOutcome {
    Ignored,
    CompletedInactive,
    Applied { status: String },
}

pub(in crate::features) struct RemoteOpsFeatureState {
    docker: DockerPaneState,
    process: ProcessPaneState,
    stats: StatsPaneState,
    stats_sampler: Arc<RemoteStatsSampler>,
    gpu: AcceleratorPaneState<RemoteGpuOverview, GpuJobResult>,
    npu: AcceleratorPaneState<RemoteNpuOverview, NpuJobResult>,
}

/// Focus handles the Remote page needs at construction time.
pub(in crate::features) struct RemoteOpsFeatureFocus {}

struct DockerPaneState {
    job: RemoteJobState<DockerJobResult>,
    pub overview: Option<Arc<RemoteDockerOverview>>,
    loaded_resources: HashSet<DockerTab>,
    resource_attempts: HashMap<DockerTab, Instant>,
    /// Bumped by every mutation that changes what `docker_presentation` returns.
    revision: u64,
    data_generation: u64,
    derived: Option<DockerDerivedCache>,
    pub status: String,
    pub details: Option<DockerContainerDetails>,
    pub details_container_id: Option<String>,
    pub details_last_refresh_at: Option<Instant>,
    pub container_menu_id: Option<String>,
    pub compose_menu_id: Option<String>,
    pub tab: DockerTab,
    pub tab_menu_open: bool,
    pub header_menu_open: bool,
    pub search_draft: String,
    pub compose_expanded: Arc<HashSet<String>>,
    pub compose_services: Arc<HashMap<String, Vec<DockerComposeService>>>,
    pub compose_service_errors: Arc<HashMap<String, String>>,
}

#[derive(Clone, PartialEq, Eq)]
struct DockerDerivedKey {
    data_generation: u64,
    normalized_query: String,
    tab: DockerTab,
}

struct DockerDerivedCache {
    key: DockerDerivedKey,
    items: DockerDerivedItems,
}

#[derive(Clone)]
pub(in crate::features) enum DockerDerivedItems {
    Containers(Arc<[DockerContainer]>),
    Images(Arc<[DockerImage]>),
    Volumes(Arc<[DockerVolume]>),
    Networks(Arc<[DockerNetwork]>),
    Compose(Arc<[DockerComposeProject]>),
}

struct ProcessPaneState {
    job: RemoteJobState<ProcessJobResult>,
    pub items: Arc<[RemoteProcess]>,
    /// Bumped by every mutation that changes what `process_presentation` returns.
    revision: u64,
    data_generation: u64,
    derived: Option<ProcessDerivedCache>,
    /// Which sort keys the current panel width can show a column for.
    ///
    /// Pushed by the app when the right panel is resized. Held here because the sort
    /// key must be constrained to it whenever *either* changes, and the render pass is
    /// no longer allowed to do that constraining itself.
    sort_columns: ProcessSortColumns,
    pub snapshot_loaded: bool,
    pub status: String,
    pub search_draft: String,
    pub sort_key: RemoteProcessSortKey,
    pub sort_direction: RemoteProcessSortDirection,
    pub selected_pid: Option<u32>,
    pub menu_pid: Option<u32>,
    pub nice_draft: String,
}

/// Whether the process table is wide enough to sort by memory and by user.
///
/// Defaults to permissive, matching the widest layout: a narrow panel pushes the real
/// values in before anything is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) struct ProcessSortColumns {
    pub allow_memory: bool,
    pub allow_user: bool,
}

impl Default for ProcessSortColumns {
    fn default() -> Self {
        Self {
            allow_memory: true,
            allow_user: true,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct ProcessDerivedKey {
    data_generation: u64,
    normalized_query: String,
    sort_key: RemoteProcessSortKey,
    sort_direction: RemoteProcessSortDirection,
}

struct ProcessDerivedCache {
    key: ProcessDerivedKey,
    items: Arc<[RemoteProcess]>,
}

struct StatsPaneState {
    job: RemoteJobState<StatsJobResult>,
    pub data: Option<RemoteStats>,
    pub status: String,
    pub cpu_expanded: bool,
    manual_job_id: Option<u64>,
    warmup_retry_pending: bool,
    warmup_retry_attempted: bool,
    /// Bumped by every mutation that changes what `stats_presentation` returns.
    revision: u64,
    active_session_id: Option<String>,
    network_history: HashMap<String, VecDeque<NetworkHistorySample>>,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::features) struct NetworkHistorySample {
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
    pub interfaces: HashMap<String, (f64, f64)>,
}

/// How a GPU/NPU overview exposes its process list for filtering and sorting.
///
/// The two overviews carry different process types with different match fields and
/// different orderings, so this is the seam that lets one pane type derive both. The
/// four implementations came out of `stats_view.rs`, where they ran inside the render
/// pass -- which is why the accelerator panes had no derived cache at all and clamped
/// their scroll offset against a count only the view knew.
pub(in crate::features) trait AcceleratorProcessList {
    type Process: Clone;

    fn processes(&self) -> &[Self::Process];
    fn process_matches(process: &Self::Process, normalized_query: &str) -> bool;
    fn sort_processes(processes: &mut [Self::Process]);
}

struct AcceleratorPaneState<Data: AcceleratorProcessList, Event> {
    job: RemoteJobState<Event>,
    pub data: Option<Data>,
    /// Bumped by every mutation that changes what `*_presentation` returns.
    revision: u64,
    data_generation: u64,
    derived: Option<AcceleratorDerivedCache<Data::Process>>,
    pub status: String,
    pub search_draft: String,
    pub expanded_devices: HashSet<String>,
    pub process_list_offset: usize,
    unavailable_sessions: HashSet<String>,
}

#[derive(Clone, PartialEq, Eq)]
struct AcceleratorDerivedKey {
    data_generation: u64,
    normalized_query: String,
}

struct AcceleratorDerivedCache<Process> {
    key: AcceleratorDerivedKey,
    items: Arc<[Process]>,
}

#[derive(Clone)]
pub(in crate::features) struct DockerPresentationState {
    pub overview: Option<Arc<RemoteDockerOverview>>,
    pub loaded_resources: HashSet<DockerTab>,
    pub status: String,
    pub details: Option<DockerContainerDetails>,
    pub details_container_id: Option<String>,
    pub container_menu_id: Option<String>,
    pub compose_menu_id: Option<String>,
    pub tab_menu_open: bool,
    pub search_draft: String,
    pub compose_expanded: Arc<HashSet<String>>,
    pub compose_services: Arc<HashMap<String, Vec<DockerComposeService>>>,
    pub compose_service_errors: Arc<HashMap<String, String>>,
    pub pending: bool,
}

#[derive(Clone)]
pub(in crate::features) struct ProcessPresentationState {
    pub items: Arc<[RemoteProcess]>,
    pub snapshot_loaded: bool,
    pub status: String,
    pub search_draft: String,
    pub sort_key: RemoteProcessSortKey,
    pub sort_direction: RemoteProcessSortDirection,
    pub selected_pid: Option<u32>,
    pub menu_pid: Option<u32>,
    pub nice_draft: String,
    pub pending: bool,
}

#[derive(Clone)]
pub(in crate::features) struct StatsPresentationState {
    pub data: Option<RemoteStats>,
    pub network_history: Arc<[NetworkHistorySample]>,
    pub cpu_expanded: bool,
    pub pending: bool,
    pub error: bool,
    pub consecutive_refresh_failures: u8,
}

#[derive(Clone)]
pub(in crate::features) struct GpuPresentationState {
    pub data: Option<RemoteGpuOverview>,
    pub status: String,
    pub search_draft: String,
    pub expanded_devices: HashSet<String>,
    pub process_list_offset: usize,
    pub pending: bool,
    pub consecutive_refresh_failures: u8,
}

#[derive(Clone)]
pub(in crate::features) struct NpuPresentationState {
    pub data: Option<RemoteNpuOverview>,
    pub status: String,
    pub search_draft: String,
    pub expanded_devices: HashSet<String>,
    pub process_list_offset: usize,
    pub pending: bool,
    pub consecutive_refresh_failures: u8,
}

impl RemoteOpsFeatureState {
    pub(in crate::features) fn new(_focus: RemoteOpsFeatureFocus) -> Self {
        Self {
            docker: DockerPaneState {
                job: RemoteJobState::new(),
                overview: None,
                loaded_resources: HashSet::new(),
                resource_attempts: HashMap::new(),
                revision: 0,
                data_generation: 0,
                derived: None,
                status: "start an SSH session to inspect Docker".to_string(),
                details: None,
                details_container_id: None,
                details_last_refresh_at: None,
                container_menu_id: None,
                compose_menu_id: None,
                tab: DockerTab::Containers,
                tab_menu_open: false,
                header_menu_open: false,
                search_draft: String::new(),
                compose_expanded: Arc::default(),
                compose_services: Arc::default(),
                compose_service_errors: Arc::default(),
            },
            process: ProcessPaneState {
                job: RemoteJobState::new(),
                items: Arc::from([]),
                revision: 0,
                data_generation: 0,
                derived: None,
                sort_columns: ProcessSortColumns::default(),
                snapshot_loaded: false,
                status: "ready".to_string(),
                search_draft: String::new(),
                sort_key: RemoteProcessSortKey::Cpu,
                sort_direction: RemoteProcessSortDirection::Descending,
                selected_pid: None,
                menu_pid: None,
                nice_draft: "0".to_string(),
            },
            stats: StatsPaneState {
                job: RemoteJobState::new(),
                data: None,
                status: "start an SSH session to inspect remote stats".to_string(),
                cpu_expanded: false,
                manual_job_id: None,
                warmup_retry_pending: false,
                warmup_retry_attempted: false,
                revision: 0,
                active_session_id: None,
                network_history: HashMap::new(),
            },
            stats_sampler: Arc::new(RemoteStatsSampler::default()),
            gpu: AcceleratorPaneState::new("start an SSH session to inspect NVIDIA GPU"),
            npu: AcceleratorPaneState::new("start an SSH session to inspect Ascend NPU"),
        }
    }

    pub(in crate::features) fn reset_for_session_switch(&mut self) {
        self.process.reset_for_session_switch();
        self.stats.reset_for_session_switch();
        self.docker.reset_for_session_switch();
        self.gpu
            .reset_for_session_switch("start an SSH session to inspect NVIDIA GPU");
        self.npu
            .reset_for_session_switch("start an SSH session to inspect Ascend NPU");
    }

    pub(in crate::features) fn stats_sampler(&self) -> Arc<RemoteStatsSampler> {
        self.stats_sampler.clone()
    }

    pub(in crate::features) fn clear_stats_sample(&mut self, session_id: &str) {
        self.stats_sampler.clear_session(session_id);
        self.stats.network_history.remove(session_id);
    }

    pub(in crate::features) fn activate_stats_session(&mut self, session_id: &str) {
        self.stats.activate_session(session_id);
    }

    pub(in crate::features) fn docker_presentation(&self) -> DockerPresentationState {
        DockerPresentationState {
            overview: self.docker.overview.clone(),
            loaded_resources: self.docker.loaded_resources.clone(),
            status: self.docker.status.clone(),
            details: self.docker.details.clone(),
            details_container_id: self.docker.details_container_id.clone(),
            container_menu_id: self.docker.container_menu_id.clone(),
            compose_menu_id: self.docker.compose_menu_id.clone(),
            tab_menu_open: self.docker.tab_menu_open,
            search_draft: self.docker.search_draft.clone(),
            compose_expanded: self.docker.compose_expanded.clone(),
            compose_services: self.docker.compose_services.clone(),
            compose_service_errors: self.docker.compose_service_errors.clone(),
            pending: self.docker.is_pending(),
        }
    }

    /// The filtered Docker list for the tab actually shown. Read-only.
    pub(in crate::features) fn derived_docker_items(&self) -> DockerDerivedItems {
        self.docker.derived()
    }

    /// The tab actually shown, which falls back from Compose when unsupported.
    pub(in crate::features) fn docker_effective_tab(&self) -> DockerTab {
        self.docker.effective_tab()
    }

    pub(in crate::features) fn docker_resource_load_due(&self, interval: u32) -> bool {
        self.docker.resource_load_due(interval)
    }

    pub(in crate::features) fn process_presentation(&self) -> ProcessPresentationState {
        ProcessPresentationState {
            items: self.process.items.clone(),
            snapshot_loaded: self.process.snapshot_loaded,
            status: self.process.status.clone(),
            search_draft: self.process.search_draft.clone(),
            sort_key: self.process.sort_key,
            sort_direction: self.process.sort_direction,
            selected_pid: self.process.selected_pid,
            menu_pid: self.process.menu_pid,
            nice_draft: self.process.nice_draft.clone(),
            pending: self.process.is_pending(),
        }
    }

    /// The filtered, sorted process list. Read-only.
    pub(in crate::features) fn derived_processes(&self) -> Arc<[RemoteProcess]> {
        self.process.derived()
    }

    /// The filtered, sorted GPU process list. Read-only.
    pub(in crate::features) fn derived_gpu_processes(&self) -> Arc<[RemoteGpuProcess]> {
        self.gpu.derived()
    }

    /// The filtered, sorted NPU process list. Read-only.
    pub(in crate::features) fn derived_npu_processes(&self) -> Arc<[RemoteNpuProcess]> {
        self.npu.derived()
    }

    /// Tell the process table which sort columns the panel width can show.
    ///
    /// Returns whether anything changed, so the caller can skip a repaint. This is the
    /// one input to the pane's invariant that the pane cannot observe for itself.
    pub(in crate::features) fn set_process_sort_columns(
        &mut self,
        columns: ProcessSortColumns,
    ) -> bool {
        self.process.set_sort_columns(columns)
    }

    /// Revision of what `stats_presentation` would return.
    ///
    /// Bumped by every mutation that changes it. A panel stores the revision of the
    /// snapshot it holds; the flush pushes a new one only when they differ, and the
    /// panel render asserts they match so a missed flush boundary is loud.
    /// `#[cfg(test)]` until the flush consumes it, which is the next commit. The
    /// counter itself is maintained in production code; only the readers are test-only,
    /// so the invariant is under test before anything depends on it.
    pub(in crate::features) fn process_revision(&self) -> u64 {
        self.process.revision()
    }

    pub(in crate::features) fn docker_revision(&self) -> u64 {
        self.docker.revision()
    }

    pub(in crate::features) fn stats_revision(&self) -> u64 {
        self.stats.revision()
    }

    pub(in crate::features) fn gpu_revision(&self) -> u64 {
        self.gpu.revision()
    }

    pub(in crate::features) fn npu_revision(&self) -> u64 {
        self.npu.revision()
    }

    pub(in crate::features) fn stats_presentation(&self) -> StatsPresentationState {
        StatsPresentationState {
            data: self.stats.data.clone(),
            network_history: self.stats.active_network_history(),
            cpu_expanded: self.stats.cpu_expanded,
            pending: self.stats.is_pending(),
            error: self.stats.consecutive_refresh_failures() > 0,
            consecutive_refresh_failures: self.stats.consecutive_refresh_failures(),
        }
    }

    pub(in crate::features) fn gpu_presentation(&self) -> GpuPresentationState {
        GpuPresentationState {
            data: self.gpu.data.clone(),
            status: self.gpu.status.clone(),
            search_draft: self.gpu.search_draft.clone(),
            expanded_devices: self.gpu.expanded_devices.clone(),
            process_list_offset: self.gpu.process_list_offset,
            pending: self.gpu.is_pending(),
            consecutive_refresh_failures: self.gpu.consecutive_refresh_failures(),
        }
    }

    pub(in crate::features) fn npu_presentation(&self) -> NpuPresentationState {
        NpuPresentationState {
            data: self.npu.data.clone(),
            status: self.npu.status.clone(),
            search_draft: self.npu.search_draft.clone(),
            expanded_devices: self.npu.expanded_devices.clone(),
            process_list_offset: self.npu.process_list_offset,
            pending: self.npu.is_pending(),
            consecutive_refresh_failures: self.npu.consecutive_refresh_failures(),
        }
    }

    pub(in crate::features) fn docker_status(&self) -> &str {
        &self.docker.status
    }

    pub(in crate::features) fn set_docker_status(&mut self, status: impl Into<String>) {
        self.docker.status = status.into();
        self.docker.touch();
    }

    pub(in crate::features) fn process_status(&self) -> &str {
        &self.process.status
    }

    pub(in crate::features) fn set_process_status(&mut self, status: impl Into<String>) {
        self.process.set_status(status);
    }

    pub(in crate::features) fn stats_status(&self) -> &str {
        &self.stats.status
    }

    pub(in crate::features) fn set_stats_status(&mut self, status: impl Into<String>) {
        self.stats.set_status(status);
    }

    pub(in crate::features) fn gpu_status(&self) -> &str {
        &self.gpu.status
    }

    pub(in crate::features) fn set_gpu_status(&mut self, status: impl Into<String>) {
        self.gpu.set_status(status);
    }

    pub(in crate::features) fn npu_status(&self) -> &str {
        &self.npu.status
    }

    pub(in crate::features) fn set_npu_status(&mut self, status: impl Into<String>) {
        self.npu.set_status(status);
    }

    pub(in crate::features) fn loaded_process_count(&self) -> Option<usize> {
        (self.process.snapshot_loaded && !self.process.items.is_empty())
            .then_some(self.process.items.len())
    }

    pub(in crate::features) fn docker_engine_version(&self) -> Option<String> {
        let overview = self
            .docker
            .overview
            .as_ref()
            .filter(|overview| overview.available)?;
        let version = overview.version.trim();
        Some(if version.is_empty() {
            "-".to_string()
        } else {
            version.to_string()
        })
    }

    pub(in crate::features) fn docker_can_prune(&self) -> bool {
        self.docker
            .overview
            .as_ref()
            .is_some_and(|overview| overview.available)
    }

    pub(in crate::features) fn docker_header_menu_open(&self) -> bool {
        self.docker.header_menu_open
    }

    pub(in crate::features) fn docker_is_pending(&self) -> bool {
        self.docker.is_pending()
    }

    pub(in crate::features) fn process_is_pending(&self) -> bool {
        self.process.is_pending()
    }

    pub(in crate::features) fn stats_manual_refreshing(&self) -> bool {
        self.stats.manual_job_id.is_some()
    }

    pub(in crate::features) fn stats_is_pending(&self) -> bool {
        self.stats.is_pending()
    }

    pub(in crate::features) fn gpu_is_pending(&self) -> bool {
        self.gpu.is_pending()
    }

    pub(in crate::features) fn npu_is_pending(&self) -> bool {
        self.npu.is_pending()
    }

    pub(in crate::features) fn docker_last_refresh_at(&self) -> Option<Instant> {
        self.docker.last_refresh_at()
    }

    pub(in crate::features) fn process_last_refresh_at(&self) -> Option<Instant> {
        self.process.last_refresh_at()
    }

    pub(in crate::features) fn stats_last_refresh_at(&self) -> Option<Instant> {
        self.stats.last_refresh_at()
    }

    pub(in crate::features) fn gpu_last_refresh_at(&self) -> Option<Instant> {
        self.gpu.last_refresh_at()
    }

    pub(in crate::features) fn npu_last_refresh_at(&self) -> Option<Instant> {
        self.npu.last_refresh_at()
    }

    pub(in crate::features) fn docker_details_refresh(&self) -> Option<(String, Instant)> {
        let refresh = (
            self.docker.details_container_id.clone()?,
            self.docker.details_last_refresh_at?,
        );
        if self.docker.details.is_some() {
            Some(refresh)
        } else {
            None
        }
    }

    pub(in crate::features) fn set_docker_tab(&mut self, tab: DockerTab) {
        self.docker.set_tab(tab);
    }

    pub(in crate::features) fn toggle_docker_tab_menu(&mut self) {
        self.docker.toggle_tab_menu();
        if self.docker.tab_menu_open {
            self.docker.header_menu_open = false;
            self.docker.container_menu_id = None;
            self.docker.compose_menu_id = None;
        }
        self.docker.touch();
    }

    pub(in crate::features) fn toggle_docker_header_menu(&mut self) {
        self.docker.header_menu_open = !self.docker.header_menu_open;
        if self.docker.header_menu_open {
            self.docker.tab_menu_open = false;
            self.docker.container_menu_id = None;
            self.docker.compose_menu_id = None;
        }
        self.docker.touch();
    }

    pub(in crate::features) fn close_docker_menus(&mut self) {
        self.docker.tab_menu_open = false;
        self.docker.header_menu_open = false;
        self.docker.container_menu_id = None;
        self.docker.compose_menu_id = None;
        self.docker.touch();
    }

    pub(in crate::features) fn docker_menus_open(&self) -> bool {
        self.docker.tab_menu_open
            || self.docker.header_menu_open
            || self.docker.container_menu_id.is_some()
            || self.docker.compose_menu_id.is_some()
    }

    pub(in crate::features) fn toggle_docker_container_menu(&mut self, id: String) {
        let open = self.docker.container_menu_id.as_deref() != Some(id.as_str());
        self.docker.container_menu_id = open.then_some(id);
        if open {
            self.docker.tab_menu_open = false;
            self.docker.header_menu_open = false;
            self.docker.compose_menu_id = None;
        }
        self.docker.touch();
    }

    pub(in crate::features) fn close_docker_container_menu(&mut self) {
        self.docker.container_menu_id = None;
        self.docker.touch();
    }

    pub(in crate::features) fn toggle_docker_compose_menu(&mut self, id: String) {
        let open = self.docker.compose_menu_id.as_deref() != Some(id.as_str());
        self.docker.compose_menu_id = open.then_some(id);
        if open {
            self.docker.tab_menu_open = false;
            self.docker.header_menu_open = false;
            self.docker.container_menu_id = None;
        }
        self.docker.touch();
    }

    pub(in crate::features) fn close_docker_compose_menu(&mut self) {
        self.docker.compose_menu_id = None;
        self.docker.touch();
    }

    pub(in crate::features) fn apply_docker_search(&mut self, text: String) {
        self.docker.apply_search(text);
    }

    pub(in crate::features) fn close_docker_details(&mut self) {
        self.docker.close_details();
    }

    pub(in crate::features) fn toggle_compose_project(
        &mut self,
        key: String,
        project_name: &str,
    ) -> bool {
        let expanded = Arc::make_mut(&mut self.docker.compose_expanded);
        if expanded.remove(&key) {
            self.docker.status = format!("collapsed compose project {project_name}");
            return false;
        }
        expanded.insert(key.clone());
        self.docker.status = format!("expanded compose project {project_name}");
        self.docker.touch();
        !self.docker.compose_services.contains_key(&key)
            && !self.docker.compose_service_errors.contains_key(&key)
    }

    pub(in crate::features) fn apply_process_search(&mut self, text: String) {
        self.process.apply_search(text);
    }

    pub(in crate::features) fn toggle_process_sort(&mut self, key: RemoteProcessSortKey) {
        self.process.toggle_sort(key);
    }

    pub(in crate::features) fn toggle_process_selection(&mut self, pid: u32) {
        self.process.toggle_selection(pid);
    }

    pub(in crate::features) fn toggle_process_menu(&mut self, pid: u32) {
        self.process.toggle_menu(pid);
    }

    pub(in crate::features) fn close_process_menu(&mut self) {
        self.process.close_menu();
    }

    pub(in crate::features) fn apply_process_nice_input(&mut self, text: String) {
        self.process.apply_nice_input(text);
    }

    pub(in crate::features) fn validated_process_nice_draft(&mut self) -> Option<(u32, i32)> {
        self.process.validated_nice_draft()
    }

    pub(in crate::features) fn toggle_stats_cpu_expanded(&mut self) {
        self.stats.toggle_cpu_expanded();
    }

    pub(in crate::features) fn docker_is_pending_for(&self, session_id: &str) -> bool {
        self.docker.is_pending_for(session_id)
    }

    pub(in crate::features) fn begin_docker_job(
        &mut self,
        session_id: String,
    ) -> RemoteJobTicket<DockerJobResult> {
        self.docker.begin_job(session_id)
    }

    pub(in crate::features) fn mark_docker_refresh_started(&mut self) {
        self.docker.mark_refresh_started();
    }

    pub(in crate::features) fn mark_docker_resource_started(&mut self, tab: DockerTab) {
        self.docker.resource_attempts.insert(tab, Instant::now());
    }

    pub(in crate::features) fn take_docker_event_receiver(
        &mut self,
    ) -> Option<UnboundedReceiver<DockerJobResult>> {
        self.docker.take_event_receiver()
    }

    pub(in crate::features) fn complete_docker_event(
        &mut self,
        job_id: u64,
        session_id: &str,
    ) -> bool {
        self.docker.complete_event(job_id, session_id)
    }

    pub(in crate::features) fn start_docker_container_action(&mut self, status: String) {
        self.docker.status = status;
        self.docker.details = None;
        self.docker.details_container_id = None;
        self.docker.touch();
    }

    pub(in crate::features) fn start_docker_details(
        &mut self,
        container_id: String,
        status: String,
    ) {
        self.docker.details_container_id = Some(container_id);
        self.docker.details_last_refresh_at = Some(Instant::now());
        self.docker.status = status;
        self.docker.touch();
    }

    pub(in crate::features) fn apply_docker_overview(&mut self, overview: RemoteDockerOverview) {
        self.docker.apply_overview(overview);
    }

    pub(in crate::features) fn apply_docker_summary(&mut self, mut overview: RemoteDockerOverview) {
        if overview.available
            && let Some(previous) = self.docker.overview.as_deref()
        {
            overview.images = previous.images.clone();
            overview.volumes = previous.volumes.clone();
            overview.networks = previous.networks.clone();
            if overview.compose_available {
                overview.compose_projects = previous.compose_projects.clone();
            }
        }
        self.apply_docker_overview(overview);
    }

    pub(in crate::features) fn apply_docker_resource(&mut self, resource: DockerResource) {
        self.docker.apply_resource(resource);
    }

    pub(in crate::features) fn apply_docker_details(
        &mut self,
        container_id: String,
        details: DockerContainerDetails,
    ) {
        self.docker.details = Some(details);
        self.docker.details_container_id = Some(container_id);
        self.docker.touch();
    }

    pub(in crate::features) fn clear_compose_service_error(&mut self, key: &str) {
        Arc::make_mut(&mut self.docker.compose_service_errors).remove(key);
        self.docker.touch();
    }

    pub(in crate::features) fn set_compose_services(
        &mut self,
        key: String,
        services: Vec<DockerComposeService>,
    ) {
        Arc::make_mut(&mut self.docker.compose_service_errors).remove(&key);
        Arc::make_mut(&mut self.docker.compose_services).insert(key, services);
        self.docker.touch();
    }

    pub(in crate::features) fn set_compose_service_error(&mut self, key: String, error: String) {
        Arc::make_mut(&mut self.docker.compose_services).remove(&key);
        Arc::make_mut(&mut self.docker.compose_service_errors).insert(key, error);
        self.docker.touch();
    }

    pub(in crate::features) fn reset_docker_refresh_failures(&mut self) {
        self.docker.reset_refresh_failures();
    }

    pub(in crate::features) fn record_docker_refresh_failure(&mut self) -> u8 {
        self.docker.record_refresh_failure()
    }

    pub(in crate::features) fn clear_docker_overview(&mut self) {
        self.docker.clear_overview();
    }

    pub(in crate::features) fn process_is_pending_for(&self, session_id: &str) -> bool {
        self.process.is_pending_for(session_id)
    }

    pub(in crate::features) fn begin_process_job(
        &mut self,
        session_id: String,
    ) -> RemoteJobTicket<ProcessJobResult> {
        self.process.begin_job(session_id)
    }

    pub(in crate::features) fn mark_process_refresh_started(&mut self) {
        self.process.mark_refresh_started();
    }

    pub(in crate::features) fn take_process_event_receiver(
        &mut self,
    ) -> Option<UnboundedReceiver<ProcessJobResult>> {
        self.process.take_event_receiver()
    }

    pub(in crate::features) fn apply_process_event(
        &mut self,
        event: ProcessJobResult,
        active_session_id: Option<&str>,
    ) -> ProcessApplyOutcome {
        if !self.process.complete_event(event.job_id, &event.session_id) {
            return ProcessApplyOutcome::Ignored;
        }
        if active_session_id != Some(event.session_id.as_str()) {
            return ProcessApplyOutcome::CompletedInactive;
        }
        let was_list_refresh = self.process.status == "listing remote processes";
        let status = match event.result {
            Ok(ProcessJobOutput::Listed(processes)) => {
                self.process.reset_refresh_failures();
                let status = format!("loaded {} remote process(es)", processes.len());
                self.process.apply_processes(processes);
                status
            }
            Ok(ProcessJobOutput::Signalled {
                pid,
                signal,
                processes,
            }) => {
                self.process.apply_processes(processes);
                format!("sent {signal} to pid {pid}")
            }
            Ok(ProcessJobOutput::Reniced {
                pid,
                nice,
                processes,
            }) => {
                self.process.apply_processes(processes);
                format!("reniced pid {pid} to {nice}")
            }
            Err(error) => {
                if was_list_refresh {
                    let terminal =
                        error.contains(nyaterm_transport::PROCESS_LIST_UNSUPPORTED_ERROR);
                    if self.process.record_refresh_failure(terminal) >= 3 {
                        self.process.clear_data();
                    }
                }
                format!("process operation failed: {error}")
            }
        };
        self.process.set_status(status.clone());
        ProcessApplyOutcome::Applied { status }
    }

    #[cfg(test)]
    pub(in crate::features) fn apply_processes(&mut self, processes: Vec<RemoteProcess>) {
        self.process.apply_processes(processes);
    }

    pub(in crate::features) fn stats_is_pending_for(&self, session_id: &str) -> bool {
        self.stats.is_pending_for(session_id)
    }

    pub(in crate::features) fn begin_stats_job(
        &mut self,
        session_id: String,
        manual: bool,
    ) -> RemoteJobTicket<StatsJobResult> {
        self.stats.begin_job(session_id, manual)
    }

    pub(in crate::features) fn take_stats_warmup_retry_due(&mut self) -> bool {
        self.stats.take_warmup_retry_due()
    }

    pub(in crate::features) fn mark_stats_refresh_started(&mut self) {
        self.stats.mark_refresh_started();
    }

    pub(in crate::features) fn take_stats_event_receiver(
        &mut self,
    ) -> Option<UnboundedReceiver<StatsJobResult>> {
        self.stats.take_event_receiver()
    }

    #[cfg(test)]
    pub(in crate::features) fn reset_stats_refresh_failures(&mut self) {
        self.stats.reset_refresh_failures();
    }

    #[cfg(test)]
    pub(in crate::features) fn apply_stats(&mut self, stats: RemoteStats) {
        self.stats.apply_data("test-session", stats);
    }

    #[cfg(test)]
    pub(in crate::features) fn record_stats_refresh_failure(&mut self) -> u8 {
        let failures = self.stats.record_refresh_failure();
        if failures >= 3 {
            self.stats.clear_data();
        }
        failures
    }

    pub(in crate::features) fn apply_stats_event(
        &mut self,
        event: StatsJobResult,
        active_session_id: Option<&str>,
    ) -> StatsApplyOutcome {
        if !self.stats.complete_event(event.job_id, &event.session_id) {
            return StatsApplyOutcome::Ignored;
        }
        if active_session_id != Some(event.session_id.as_str()) {
            if let Ok(stats) = &event.result {
                self.stats.record_network_sample(&event.session_id, stats);
            }
            return StatsApplyOutcome::CompletedInactive;
        }
        match event.result {
            Ok(stats) => {
                self.stats.reset_refresh_failures();
                let status = format!(
                    "loaded stats for {} · load {:.2}/{:.2}/{:.2}",
                    if stats.system.hostname.trim().is_empty() {
                        "remote host"
                    } else {
                        stats.system.hostname.as_str()
                    },
                    stats.load.load1,
                    stats.load.load5,
                    stats.load.load15
                );
                self.stats.set_status(status.clone());
                self.stats.apply_data(&event.session_id, stats.clone());
                StatsApplyOutcome::Applied {
                    session_id: event.session_id,
                    stats: Box::new(stats),
                    status,
                }
            }
            Err(error) => {
                let failures = self.stats.record_refresh_failure();
                if failures >= 3 {
                    self.stats.clear_data();
                }
                let status = format!("stats refresh failed: {error}");
                self.stats.set_status(status.clone());
                StatsApplyOutcome::Failed { status }
            }
        }
    }

    pub(in crate::features) fn gpu_is_pending_for(&self, session_id: &str) -> bool {
        self.gpu.is_pending_for(session_id)
    }

    pub(in crate::features) fn gpu_unavailable_for(&self, session_id: &str) -> bool {
        self.gpu.unavailable_for(session_id)
    }

    pub(in crate::features) fn begin_gpu_job(
        &mut self,
        session_id: String,
    ) -> RemoteJobTicket<GpuJobResult> {
        self.gpu.begin_job(session_id)
    }

    pub(in crate::features) fn mark_gpu_refresh_started(&mut self) {
        self.gpu.mark_refresh_started();
    }

    pub(in crate::features) fn take_gpu_event_receiver(
        &mut self,
    ) -> Option<UnboundedReceiver<GpuJobResult>> {
        self.gpu.take_event_receiver()
    }

    pub(in crate::features) fn complete_gpu_event(
        &mut self,
        job_id: u64,
        session_id: &str,
    ) -> bool {
        self.gpu.complete_event(job_id, session_id)
    }

    pub(in crate::features) fn reset_gpu_refresh_failures(&mut self) {
        self.gpu.reset_refresh_failures();
    }

    pub(in crate::features) fn apply_gpu_search(&mut self, text: String) {
        self.gpu.apply_search(text, "GPU search updated");
    }

    pub(in crate::features) fn toggle_gpu_device_expanded(&mut self, key: String) {
        self.gpu.toggle_device_expanded(key);
    }

    pub(in crate::features) fn set_gpu_process_offset(&mut self, offset: usize) -> bool {
        self.gpu.set_process_offset(offset)
    }

    pub(in crate::features) fn apply_gpu(&mut self, session_id: &str, overview: RemoteGpuOverview) {
        if overview.available {
            self.gpu.clear_unavailable(session_id);
        } else {
            self.gpu.mark_unavailable(session_id.to_string());
        }
        let active_devices = overview
            .gpus
            .iter()
            .map(|gpu| gpu_device_key(gpu.index, &gpu.uuid))
            .collect::<HashSet<_>>();
        self.gpu.retain_expanded_devices(&active_devices);
        self.gpu.apply_data(overview);
    }

    pub(in crate::features) fn record_gpu_refresh_failure(&mut self) -> u8 {
        let failures = self.gpu.record_refresh_failure();
        if failures >= 3 {
            self.gpu.clear_data();
        }
        failures
    }

    pub(in crate::features) fn npu_is_pending_for(&self, session_id: &str) -> bool {
        self.npu.is_pending_for(session_id)
    }

    pub(in crate::features) fn npu_unavailable_for(&self, session_id: &str) -> bool {
        self.npu.unavailable_for(session_id)
    }

    pub(in crate::features) fn begin_npu_job(
        &mut self,
        session_id: String,
    ) -> RemoteJobTicket<NpuJobResult> {
        self.npu.begin_job(session_id)
    }

    pub(in crate::features) fn mark_npu_refresh_started(&mut self) {
        self.npu.mark_refresh_started();
    }

    pub(in crate::features) fn take_npu_event_receiver(
        &mut self,
    ) -> Option<UnboundedReceiver<NpuJobResult>> {
        self.npu.take_event_receiver()
    }

    pub(in crate::features) fn complete_npu_event(
        &mut self,
        job_id: u64,
        session_id: &str,
    ) -> bool {
        self.npu.complete_event(job_id, session_id)
    }

    pub(in crate::features) fn reset_npu_refresh_failures(&mut self) {
        self.npu.reset_refresh_failures();
    }

    pub(in crate::features) fn apply_npu_search(&mut self, text: String) {
        self.npu.apply_search(text, "NPU search updated");
    }

    pub(in crate::features) fn toggle_npu_device_expanded(&mut self, key: String) {
        self.npu.toggle_device_expanded(key);
    }

    pub(in crate::features) fn set_npu_process_offset(&mut self, offset: usize) -> bool {
        self.npu.set_process_offset(offset)
    }

    pub(in crate::features) fn apply_npu(&mut self, session_id: &str, overview: RemoteNpuOverview) {
        if overview.available {
            self.npu.clear_unavailable(session_id);
        } else {
            self.npu.mark_unavailable(session_id.to_string());
        }
        let active_devices = overview
            .npus
            .iter()
            .map(|npu| npu.device_key.clone())
            .collect::<HashSet<_>>();
        self.npu.retain_expanded_devices(&active_devices);
        self.npu.apply_data(overview);
    }

    pub(in crate::features) fn record_npu_refresh_failure(&mut self) -> u8 {
        let failures = self.npu.record_refresh_failure();
        if failures >= 3 {
            self.npu.clear_data();
        }
        failures
    }
}

impl DockerPaneState {
    fn resource_load_due(&self, interval: u32) -> bool {
        let tab = self.effective_tab();
        tab != DockerTab::Containers
            && self
                .overview
                .as_ref()
                .is_some_and(|overview| overview.available)
            && !self.loaded_resources.contains(&tab)
            && self
                .resource_attempts
                .get(&tab)
                .is_none_or(|attempt| attempt.elapsed() >= Duration::from_secs(u64::from(interval)))
    }

    pub(in crate::features) fn is_pending(&self) -> bool {
        self.job.is_pending()
    }

    pub(in crate::features) fn last_refresh_at(&self) -> Option<Instant> {
        self.job.last_refresh_at()
    }

    pub(super) fn is_pending_for(&self, session_id: &str) -> bool {
        self.job.is_pending_for(session_id)
    }

    pub(super) fn begin_job(&mut self, session_id: String) -> RemoteJobTicket<DockerJobResult> {
        self.touch();
        self.job.begin(session_id)
    }

    pub(super) fn mark_refresh_started(&mut self) {
        self.job.mark_refresh_started();
    }

    pub(super) fn take_event_receiver(&mut self) -> Option<UnboundedReceiver<DockerJobResult>> {
        self.job.take_event_receiver()
    }

    pub(super) fn complete_event(&mut self, job_id: u64, session_id: &str) -> bool {
        self.touch();
        self.job.complete_if_matches(job_id, session_id)
    }

    pub(super) fn reset_refresh_failures(&mut self) {
        self.job.reset_refresh_failures();
    }

    pub(super) fn record_refresh_failure(&mut self) -> u8 {
        self.job.record_refresh_failure(false)
    }

    pub(in crate::features) fn set_tab(&mut self, tab: DockerTab) {
        self.container_menu_id = None;
        self.compose_menu_id = None;
        self.tab_menu_open = false;
        self.header_menu_open = false;
        if tab == DockerTab::Compose
            && self
                .overview
                .as_ref()
                .is_some_and(|overview| !overview.compose_available)
        {
            self.status = "Docker Compose is not available on this host".to_string();
            return;
        }
        self.tab = tab;
        self.reconcile();
        self.status = format!("Docker tab: {}", tab.label());
    }

    pub(in crate::features) fn toggle_tab_menu(&mut self) {
        self.tab_menu_open = !self.tab_menu_open;
        self.touch();
    }

    pub(in crate::features) fn apply_search(&mut self, text: String) {
        self.search_draft = text;
        self.reconcile();
        self.status = "Docker search updated".to_string();
    }

    pub(in crate::features) fn close_details(&mut self) {
        self.details = None;
        self.details_container_id = None;
        self.details_last_refresh_at = None;
        self.status = "container details closed".to_string();
        self.touch();
    }

    pub(in crate::features) fn apply_overview(&mut self, overview: RemoteDockerOverview) {
        if !overview.available {
            self.loaded_resources.clear();
            self.resource_attempts.clear();
        } else if !overview.compose_available {
            self.loaded_resources.remove(&DockerTab::Compose);
            self.resource_attempts.remove(&DockerTab::Compose);
        }
        if let Some(details_id) = self.details_container_id.as_deref()
            && !overview
                .containers
                .iter()
                .any(|container| container.id == details_id)
        {
            self.details = None;
            self.details_container_id = None;
            self.details_last_refresh_at = None;
        }
        self.retain_compose_projects(&overview.compose_projects);
        self.overview = Some(Arc::new(overview));
        self.data_generation = self.data_generation.wrapping_add(1);
        self.derived = None;
        self.reconcile();
    }

    fn apply_resource(&mut self, resource: DockerResource) {
        if self.overview.is_none() {
            return;
        }
        let tab = match resource {
            DockerResource::Images(images) => {
                Arc::make_mut(self.overview.as_mut().expect("Docker overview exists")).images =
                    images;
                DockerTab::Images
            }
            DockerResource::Volumes(volumes) => {
                Arc::make_mut(self.overview.as_mut().expect("Docker overview exists")).volumes =
                    volumes;
                DockerTab::Volumes
            }
            DockerResource::Networks(networks) => {
                Arc::make_mut(self.overview.as_mut().expect("Docker overview exists")).networks =
                    networks;
                DockerTab::Networks
            }
            DockerResource::Compose(projects) => {
                self.retain_compose_projects(&projects);
                Arc::make_mut(self.overview.as_mut().expect("Docker overview exists"))
                    .compose_projects = projects;
                DockerTab::Compose
            }
        };
        self.loaded_resources.insert(tab);
        self.data_generation = self.data_generation.wrapping_add(1);
        self.derived = None;
        self.reconcile();
    }

    fn retain_compose_projects(&mut self, projects: &[DockerComposeProject]) {
        let active_keys = projects
            .iter()
            .map(|project| {
                docker_compose_project_key(&project.name, Some(project.config_files.as_str()))
            })
            .collect::<HashSet<_>>();
        Arc::make_mut(&mut self.compose_expanded).retain(|key| active_keys.contains(key));
        Arc::make_mut(&mut self.compose_services).retain(|key, _| active_keys.contains(key));
        Arc::make_mut(&mut self.compose_service_errors).retain(|key, _| active_keys.contains(key));
    }

    fn clear_overview(&mut self) {
        self.overview = None;
        self.loaded_resources.clear();
        self.resource_attempts.clear();
        self.data_generation = self.data_generation.wrapping_add(1);
        self.derived = None;
        self.reconcile();
    }

    /// Record that the presentation changed.
    ///
    /// `pub(super)` because several mutators on `RemoteOpsFeatureState` write Docker
    /// fields directly -- menus, details and compose state. Routing all of them
    /// through pane methods would be a larger change than this batch wants; what
    /// guarantees completeness either way is
    /// `docker_presentation_mutations_bump_the_revision`, which drives every one.
    pub(super) fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    fn revision(&self) -> u64 {
        self.revision
    }

    /// The tab actually shown, which is not always the stored one.
    ///
    /// Compose falls back to Containers when the host has no compose support. That
    /// fallback used to be computed in `docker_view` and passed into the derived
    /// lookup, which is why the lookup took a tab argument at all; it belongs here
    /// because the derived list has to be keyed on the tab that is really displayed.
    fn effective_tab(&self) -> DockerTab {
        let compose_available = self
            .overview
            .as_deref()
            .is_some_and(|overview| overview.compose_available);
        if self.tab == DockerTab::Compose && !compose_available {
            DockerTab::Containers
        } else {
            self.tab
        }
    }

    /// Bring the derived list back in step with the overview, query and active tab.
    fn reconcile(&mut self) {
        self.derived_items(self.effective_tab());
        self.touch();
    }

    /// The filtered list for the effective tab, without recomputing.
    fn derived(&self) -> DockerDerivedItems {
        self.derived
            .as_ref()
            .map(|cache| cache.items.clone())
            .unwrap_or(DockerDerivedItems::Containers(Arc::from([])))
    }

    fn derived_items(&mut self, tab: DockerTab) -> DockerDerivedItems {
        let key = DockerDerivedKey {
            data_generation: self.data_generation,
            normalized_query: self.search_draft.trim().to_ascii_lowercase(),
            tab,
        };
        if let Some(cache) = self.derived.as_ref()
            && cache.key == key
        {
            return cache.items.clone();
        }

        let items = match (self.overview.as_deref(), tab) {
            (Some(overview), DockerTab::Containers) => DockerDerivedItems::Containers(
                overview
                    .containers
                    .iter()
                    .filter(|item| docker_container_matches(item, &key.normalized_query))
                    .cloned()
                    .collect::<Vec<_>>()
                    .into(),
            ),
            (Some(overview), DockerTab::Images) => DockerDerivedItems::Images(
                overview
                    .images
                    .iter()
                    .filter(|item| docker_image_matches(item, &key.normalized_query))
                    .cloned()
                    .collect::<Vec<_>>()
                    .into(),
            ),
            (Some(overview), DockerTab::Volumes) => DockerDerivedItems::Volumes(
                overview
                    .volumes
                    .iter()
                    .filter(|item| docker_volume_matches(item, &key.normalized_query))
                    .cloned()
                    .collect::<Vec<_>>()
                    .into(),
            ),
            (Some(overview), DockerTab::Networks) => DockerDerivedItems::Networks(
                overview
                    .networks
                    .iter()
                    .filter(|item| docker_network_matches(item, &key.normalized_query))
                    .cloned()
                    .collect::<Vec<_>>()
                    .into(),
            ),
            (Some(overview), DockerTab::Compose) => DockerDerivedItems::Compose(
                overview
                    .compose_projects
                    .iter()
                    .filter(|item| docker_compose_project_matches(item, &key.normalized_query))
                    .cloned()
                    .collect::<Vec<_>>()
                    .into(),
            ),
            (None, DockerTab::Containers) => DockerDerivedItems::Containers(Arc::from([])),
            (None, DockerTab::Images) => DockerDerivedItems::Images(Arc::from([])),
            (None, DockerTab::Volumes) => DockerDerivedItems::Volumes(Arc::from([])),
            (None, DockerTab::Networks) => DockerDerivedItems::Networks(Arc::from([])),
            (None, DockerTab::Compose) => DockerDerivedItems::Compose(Arc::from([])),
        };
        self.derived = Some(DockerDerivedCache {
            key,
            items: items.clone(),
        });
        items
    }

    fn reset_for_session_switch(&mut self) {
        self.job.reset_for_session_switch();
        self.clear_overview();
        self.details = None;
        self.details_container_id = None;
        self.details_last_refresh_at = None;
        self.container_menu_id = None;
        self.compose_menu_id = None;
        self.compose_expanded = Arc::default();
        self.compose_services = Arc::default();
        self.compose_service_errors = Arc::default();
        self.status = "start an SSH session to inspect Docker".to_string();
    }
}

fn docker_container_matches(container: &DockerContainer, query: &str) -> bool {
    docker_text_matches(
        query,
        [
            container.id.as_str(),
            container.name.as_str(),
            container.image.as_str(),
            container.status.as_str(),
            container.state.as_str(),
            container.ports.as_str(),
        ],
    )
}

fn docker_image_matches(image: &DockerImage, query: &str) -> bool {
    docker_text_matches(
        query,
        [
            image.id.as_str(),
            image.repository.as_str(),
            image.tag.as_str(),
            image.size.as_str(),
            image.created_since.as_str(),
        ],
    )
}

fn docker_volume_matches(volume: &DockerVolume, query: &str) -> bool {
    docker_text_matches(query, [volume.driver.as_str(), volume.name.as_str()])
}

fn docker_network_matches(network: &DockerNetwork, query: &str) -> bool {
    docker_text_matches(
        query,
        [
            network.id.as_str(),
            network.name.as_str(),
            network.driver.as_str(),
            network.scope.as_str(),
        ],
    )
}

fn docker_compose_project_matches(project: &DockerComposeProject, query: &str) -> bool {
    docker_text_matches(
        query,
        [
            project.name.as_str(),
            project.status.as_str(),
            project.config_files.as_str(),
        ],
    )
}

fn docker_text_matches<const N: usize>(query: &str, values: [&str; N]) -> bool {
    query.is_empty()
        || values
            .iter()
            .any(|value| value.to_ascii_lowercase().contains(query))
}

impl ProcessPaneState {
    pub(in crate::features) fn is_pending(&self) -> bool {
        self.job.is_pending()
    }

    pub(in crate::features) fn last_refresh_at(&self) -> Option<Instant> {
        self.job.last_refresh_at()
    }

    pub(super) fn is_pending_for(&self, session_id: &str) -> bool {
        self.job.is_pending_for(session_id)
    }

    pub(super) fn begin_job(&mut self, session_id: String) -> RemoteJobTicket<ProcessJobResult> {
        // `pending` is part of the presentation, so starting and finishing a job moves the
        // revision. A status change happens to accompany both today, which masked it.
        self.touch();
        self.job.begin(session_id)
    }

    pub(super) fn mark_refresh_started(&mut self) {
        self.job.mark_refresh_started();
    }

    pub(super) fn take_event_receiver(&mut self) -> Option<UnboundedReceiver<ProcessJobResult>> {
        self.job.take_event_receiver()
    }

    pub(super) fn complete_event(&mut self, job_id: u64, session_id: &str) -> bool {
        self.touch();
        self.job.complete_if_matches(job_id, session_id)
    }

    pub(super) fn reset_refresh_failures(&mut self) {
        self.job.reset_refresh_failures();
    }

    pub(super) fn record_refresh_failure(&mut self, terminal: bool) -> u8 {
        self.job.record_refresh_failure(terminal)
    }

    pub(in crate::features) fn apply_search(&mut self, text: String) {
        self.search_draft = text;
        self.selected_pid = None;
        self.menu_pid = None;
        self.nice_draft = "0".to_string();
        self.reconcile();
    }

    pub(in crate::features) fn toggle_sort(&mut self, key: RemoteProcessSortKey) {
        if self.sort_key == key {
            self.sort_direction = self.sort_direction.reversed();
        } else {
            self.sort_key = key;
            self.sort_direction = match key {
                RemoteProcessSortKey::Cpu | RemoteProcessSortKey::Memory => {
                    RemoteProcessSortDirection::Descending
                }
                RemoteProcessSortKey::Pid
                | RemoteProcessSortKey::User
                | RemoteProcessSortKey::Command => RemoteProcessSortDirection::Ascending,
            };
        }
        self.reconcile();
        self.status = format!(
            "sorted processes by {} {}",
            self.sort_key.label(),
            self.sort_direction.marker()
        );
    }

    pub(in crate::features) fn toggle_selection(&mut self, pid: u32) {
        self.touch();
        self.menu_pid = None;
        self.selected_pid = (self.selected_pid != Some(pid)).then_some(pid);
        self.nice_draft = "0".to_string();
    }

    pub(in crate::features) fn apply_nice_input(&mut self, text: String) {
        let negative = text.starts_with('-');
        let digits: String = text.chars().filter(char::is_ascii_digit).take(3).collect();
        self.nice_draft = if negative {
            format!("-{digits}")
        } else {
            digits
        };
        self.touch();
    }

    pub(in crate::features) fn validated_nice_draft(&mut self) -> Option<(u32, i32)> {
        let Some(pid) = self.selected_pid else {
            self.set_status("select a process before applying nice");
            return None;
        };
        let Ok(nice) = self.nice_draft.trim().parse::<i32>() else {
            self.set_status("nice must be an integer from -20 to 19");
            return None;
        };
        if !(-20..=19).contains(&nice) {
            self.set_status("nice must be between -20 and 19");
            return None;
        }
        Some((pid, nice))
    }

    pub(in crate::features) fn apply_processes(&mut self, processes: Vec<RemoteProcess>) {
        let contains_pid = |pid| processes.iter().any(|process| process.pid == pid);
        if self.selected_pid.is_some_and(|pid| !contains_pid(pid)) {
            self.selected_pid = None;
            self.nice_draft = "0".to_string();
        }
        if self.menu_pid.is_some_and(|pid| !contains_pid(pid)) {
            self.menu_pid = None;
        }
        self.items = processes.into();
        self.data_generation = self.data_generation.wrapping_add(1);
        self.derived = None;
        self.snapshot_loaded = true;
        self.reconcile();
    }

    fn clear_data(&mut self) {
        self.items = Arc::from([]);
        self.data_generation = self.data_generation.wrapping_add(1);
        self.derived = None;
        self.snapshot_loaded = false;
        self.selected_pid = None;
        self.menu_pid = None;
        self.reconcile();
    }

    /// Record that the presentation changed.
    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    fn revision(&self) -> u64 {
        self.revision
    }

    fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
        self.touch();
    }

    fn toggle_menu(&mut self, pid: u32) {
        self.menu_pid = (self.menu_pid != Some(pid)).then_some(pid);
        self.touch();
    }

    fn close_menu(&mut self) {
        self.menu_pid = None;
        self.touch();
    }

    /// Bring the derived list and sort key back in step.
    ///
    /// Called by every mutator that changes one of their inputs, so a reader never has
    /// to trigger the recompute -- which is what the render pass used to do, by calling
    /// `derived_items` and `clamp_*` through `&mut self` while building elements.
    ///
    /// Cheap to call redundantly: the recompute is keyed, so an unchanged key returns
    /// the cached list.
    fn reconcile(&mut self) {
        // Sort first: the derived list is keyed on the sort key, so constraining after
        // recomputing would sort by a key the table cannot show.
        if (!self.sort_columns.allow_user && self.sort_key == RemoteProcessSortKey::User)
            || (!self.sort_columns.allow_memory && self.sort_key == RemoteProcessSortKey::Memory)
        {
            self.sort_key = RemoteProcessSortKey::Cpu;
        }
        self.derived_items();
        self.touch();
    }

    fn set_sort_columns(&mut self, columns: ProcessSortColumns) -> bool {
        if self.sort_columns == columns {
            return false;
        }
        self.sort_columns = columns;
        self.reconcile();
        true
    }

    /// The filtered, sorted list, without recomputing.
    ///
    /// `reconcile` guarantees the cache is populated and current, so this is a read.
    fn derived(&self) -> Arc<[RemoteProcess]> {
        self.derived
            .as_ref()
            .map(|cache| cache.items.clone())
            .unwrap_or_else(|| Arc::from([]))
    }

    fn derived_items(&mut self) -> Arc<[RemoteProcess]> {
        let key = ProcessDerivedKey {
            data_generation: self.data_generation,
            normalized_query: self.search_draft.trim().to_ascii_lowercase(),
            sort_key: self.sort_key,
            sort_direction: self.sort_direction,
        };
        if let Some(cache) = self.derived.as_ref()
            && cache.key == key
        {
            return cache.items.clone();
        }
        let mut items = self
            .items
            .iter()
            .filter(|process| process_matches(process, &key.normalized_query))
            .cloned()
            .collect::<Vec<_>>();
        sort_processes(&mut items, key.sort_key, key.sort_direction);
        let items: Arc<[RemoteProcess]> = items.into();
        self.derived = Some(ProcessDerivedCache {
            key,
            items: items.clone(),
        });
        items
    }

    fn reset_for_session_switch(&mut self) {
        self.job.reset_for_session_switch();
        self.clear_data();
        self.status = "ready".to_string();
    }
}

fn process_matches(process: &RemoteProcess, normalized_query: &str) -> bool {
    if normalized_query.is_empty() {
        return true;
    }
    format!(
        "{} {} {} {} {} {}",
        process.pid,
        process.ppid,
        process.user,
        process.state,
        process.command,
        process.command_line
    )
    .to_ascii_lowercase()
    .contains(normalized_query)
}

fn sort_processes(
    processes: &mut [RemoteProcess],
    key: RemoteProcessSortKey,
    direction: RemoteProcessSortDirection,
) {
    processes.sort_by(|left, right| {
        let ordering = match key {
            RemoteProcessSortKey::Command => left
                .command
                .cmp(&right.command)
                .then_with(|| left.pid.cmp(&right.pid)),
            RemoteProcessSortKey::Memory => left
                .memory_percent
                .partial_cmp(&right.memory_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    left.rss_kb
                        .partial_cmp(&right.rss_kb)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| left.pid.cmp(&right.pid)),
            RemoteProcessSortKey::Pid => left.pid.cmp(&right.pid),
            RemoteProcessSortKey::User => left
                .user
                .cmp(&right.user)
                .then_with(|| left.pid.cmp(&right.pid)),
            RemoteProcessSortKey::Cpu => left
                .cpu_percent
                .partial_cmp(&right.cpu_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    left.memory_percent
                        .partial_cmp(&right.memory_percent)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| left.pid.cmp(&right.pid)),
        };

        match direction {
            RemoteProcessSortDirection::Ascending => ordering,
            RemoteProcessSortDirection::Descending => ordering.reverse(),
        }
    });
}

impl StatsPaneState {
    /// Record that the presentation changed.
    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    fn revision(&self) -> u64 {
        self.revision
    }

    fn apply_data(&mut self, session_id: &str, stats: RemoteStats) {
        if stats.cpu.usage_source == CpuUsageSource::WarmingUp {
            self.warmup_retry_pending = !self.warmup_retry_attempted;
        } else {
            self.warmup_retry_pending = false;
            self.warmup_retry_attempted = false;
        }
        self.activate_session(session_id);
        self.record_network_sample(session_id, &stats);
        self.data = Some(stats);
        self.touch();
    }

    fn record_network_sample(&mut self, session_id: &str, stats: &RemoteStats) {
        let sample = NetworkHistorySample {
            rx_bytes_per_sec: stats.network_summary.rx_bytes_per_sec,
            tx_bytes_per_sec: stats.network_summary.tx_bytes_per_sec,
            interfaces: stats
                .networks
                .iter()
                .map(|network| {
                    (
                        network.nic.clone(),
                        (network.rx_bytes_per_sec, network.tx_bytes_per_sec),
                    )
                })
                .collect(),
        };
        let history = self
            .network_history
            .entry(session_id.to_string())
            .or_default();
        history.push_back(sample);
        while history.len() > 60 {
            history.pop_front();
        }
        self.touch();
    }

    fn activate_session(&mut self, session_id: &str) {
        self.active_session_id = Some(session_id.to_string());
        self.touch();
    }

    fn active_network_history(&self) -> Arc<[NetworkHistorySample]> {
        self.active_session_id
            .as_deref()
            .and_then(|session_id| self.network_history.get(session_id))
            .map(|history| Arc::from(history.iter().cloned().collect::<Vec<_>>()))
            .unwrap_or_else(|| Arc::from([]))
    }

    fn clear_data(&mut self) {
        self.data = None;
        self.touch();
    }

    fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
        self.touch();
    }

    pub(in crate::features) fn is_pending(&self) -> bool {
        self.job.is_pending()
    }

    pub(in crate::features) fn last_refresh_at(&self) -> Option<Instant> {
        self.job.last_refresh_at()
    }

    pub(in crate::features) fn consecutive_refresh_failures(&self) -> u8 {
        self.job.consecutive_refresh_failures()
    }

    pub(super) fn is_pending_for(&self, session_id: &str) -> bool {
        self.job.is_pending_for(session_id)
    }

    pub(super) fn begin_job(
        &mut self,
        session_id: String,
        manual: bool,
    ) -> RemoteJobTicket<StatsJobResult> {
        self.touch();
        let ticket = self.job.begin(session_id);
        if manual {
            self.manual_job_id = Some(ticket.job_id);
        }
        ticket
    }

    pub(super) fn mark_refresh_started(&mut self) {
        self.job.mark_refresh_started();
    }

    pub(super) fn take_event_receiver(&mut self) -> Option<UnboundedReceiver<StatsJobResult>> {
        self.job.take_event_receiver()
    }

    pub(super) fn complete_event(&mut self, job_id: u64, session_id: &str) -> bool {
        let matched = self.job.complete_if_matches(job_id, session_id);
        if matched {
            if self.manual_job_id == Some(job_id) {
                self.manual_job_id = None;
            }
            self.touch();
        }
        matched
    }

    fn take_warmup_retry_due(&mut self) -> bool {
        if !self.warmup_retry_pending
            || self
                .last_refresh_at()
                .is_none_or(|started| started.elapsed() < std::time::Duration::from_secs(1))
        {
            return false;
        }
        self.warmup_retry_pending = false;
        self.warmup_retry_attempted = true;
        self.touch();
        true
    }

    pub(super) fn reset_refresh_failures(&mut self) {
        self.job.reset_refresh_failures();
        self.touch();
    }

    pub(super) fn record_refresh_failure(&mut self) -> u8 {
        self.touch();
        self.job.record_refresh_failure(false)
    }

    pub(in crate::features) fn toggle_cpu_expanded(&mut self) {
        self.cpu_expanded = !self.cpu_expanded;
        self.status = if self.cpu_expanded {
            "showing per-core CPU usage".to_string()
        } else {
            "collapsed per-core CPU usage".to_string()
        };
        self.touch();
    }

    fn reset_for_session_switch(&mut self) {
        self.job.reset_for_session_switch();
        self.manual_job_id = None;
        self.warmup_retry_pending = false;
        self.warmup_retry_attempted = false;
        self.active_session_id = None;
        // Through the touching methods, not the fields: a session switch changes the
        // presentation, so it has to move the revision like any other mutation.
        self.clear_data();
        self.set_status("start an SSH session to inspect remote stats");
    }
}

impl<Data: AcceleratorProcessList, Event> AcceleratorPaneState<Data, Event> {
    fn new(status: &str) -> Self {
        Self {
            job: RemoteJobState::new(),
            data: None,
            revision: 0,
            data_generation: 0,
            derived: None,
            status: status.to_string(),
            search_draft: String::new(),
            expanded_devices: HashSet::new(),
            process_list_offset: 0,
            unavailable_sessions: HashSet::new(),
        }
    }

    /// Replace the overview, keeping the derived list and offset in step.
    fn apply_data(&mut self, data: Data) {
        self.data = Some(data);
        self.data_generation = self.data_generation.wrapping_add(1);
        self.derived = None;
        self.reconcile();
    }

    fn clear_data(&mut self) {
        self.data = None;
        self.data_generation = self.data_generation.wrapping_add(1);
        self.derived = None;
        self.reconcile();
    }

    /// Record that the presentation changed.
    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    fn revision(&self) -> u64 {
        self.revision
    }

    fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
        self.touch();
    }

    /// Bring the derived process list and the scroll offset back in step.
    fn reconcile(&mut self) {
        let total = self.derived_items().len();
        self.process_list_offset = self
            .process_list_offset
            .min(max_list_offset(total, ACCELERATOR_PROCESS_VIEWPORT_ROWS));
        self.touch();
    }

    /// The filtered, sorted process list, without recomputing.
    fn derived(&self) -> Arc<[Data::Process]> {
        self.derived
            .as_ref()
            .map(|cache| cache.items.clone())
            .unwrap_or_else(|| Arc::from([]))
    }

    fn derived_items(&mut self) -> Arc<[Data::Process]> {
        let key = AcceleratorDerivedKey {
            data_generation: self.data_generation,
            normalized_query: self.search_draft.trim().to_ascii_lowercase(),
        };
        if let Some(cache) = self.derived.as_ref()
            && cache.key == key
        {
            return cache.items.clone();
        }
        let mut items = self
            .data
            .as_ref()
            .map(|data| {
                data.processes()
                    .iter()
                    .filter(|process| Data::process_matches(process, &key.normalized_query))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Data::sort_processes(&mut items);
        let items: Arc<[Data::Process]> = items.into();
        self.derived = Some(AcceleratorDerivedCache {
            key,
            items: items.clone(),
        });
        items
    }

    fn is_pending(&self) -> bool {
        self.job.is_pending()
    }

    fn last_refresh_at(&self) -> Option<Instant> {
        self.job.last_refresh_at()
    }

    fn consecutive_refresh_failures(&self) -> u8 {
        self.job.consecutive_refresh_failures()
    }

    fn is_pending_for(&self, session_id: &str) -> bool {
        self.job.is_pending_for(session_id)
    }

    fn unavailable_for(&self, session_id: &str) -> bool {
        self.unavailable_sessions.contains(session_id)
    }

    fn mark_unavailable(&mut self, session_id: String) {
        self.unavailable_sessions.insert(session_id);
        self.touch();
    }

    fn clear_unavailable(&mut self, session_id: &str) {
        self.unavailable_sessions.remove(session_id);
        self.touch();
    }

    fn apply_search(&mut self, text: String, status: &str) {
        self.search_draft = text;
        self.process_list_offset = 0;
        self.reconcile();
        self.status = status.to_string();
    }

    fn toggle_device_expanded(&mut self, key: String) {
        if !self.expanded_devices.remove(&key) {
            self.expanded_devices.insert(key);
        }
        self.touch();
    }

    fn retain_expanded_devices(&mut self, active_devices: &HashSet<String>) {
        self.expanded_devices
            .retain(|key| active_devices.contains(key));
        self.touch();
    }

    fn set_process_offset(&mut self, offset: usize) -> bool {
        if self.process_list_offset == offset {
            return false;
        }
        self.process_list_offset = offset;
        self.touch();
        true
    }

    fn begin_job(&mut self, session_id: String) -> RemoteJobTicket<Event> {
        self.touch();
        self.job.begin(session_id)
    }

    fn mark_refresh_started(&mut self) {
        self.job.mark_refresh_started();
    }

    fn take_event_receiver(&mut self) -> Option<UnboundedReceiver<Event>> {
        self.job.take_event_receiver()
    }

    fn complete_event(&mut self, job_id: u64, session_id: &str) -> bool {
        self.touch();
        self.job.complete_if_matches(job_id, session_id)
    }

    fn reset_refresh_failures(&mut self) {
        self.job.reset_refresh_failures();
        self.touch();
    }

    fn record_refresh_failure(&mut self) -> u8 {
        self.touch();
        self.job.record_refresh_failure(false)
    }

    fn reset_for_session_switch(&mut self, status: &str) {
        self.job.reset_for_session_switch();
        self.expanded_devices.clear();
        self.process_list_offset = 0;
        self.clear_data();
        self.status = status.to_string();
    }
}

impl AcceleratorProcessList for RemoteGpuOverview {
    type Process = RemoteGpuProcess;

    fn processes(&self) -> &[Self::Process] {
        &self.processes
    }

    fn process_matches(process: &Self::Process, normalized_query: &str) -> bool {
        if normalized_query.is_empty() {
            return true;
        }
        format!(
            "{} {} {} {}",
            process.pid,
            process
                .gpu_index
                .map(|value| value.to_string())
                .unwrap_or_default(),
            process.gpu_uuid,
            process.process_name
        )
        .to_ascii_lowercase()
        .contains(normalized_query)
    }

    fn sort_processes(processes: &mut [Self::Process]) {
        processes.sort_by(|left, right| {
            right
                .used_memory_mb
                .cmp(&left.used_memory_mb)
                .then_with(|| {
                    left.gpu_index
                        .unwrap_or(u32::MAX)
                        .cmp(&right.gpu_index.unwrap_or(u32::MAX))
                })
                .then_with(|| left.pid.cmp(&right.pid))
        });
    }
}

impl AcceleratorProcessList for RemoteNpuOverview {
    type Process = RemoteNpuProcess;

    fn processes(&self) -> &[Self::Process] {
        &self.processes
    }

    fn process_matches(process: &Self::Process, normalized_query: &str) -> bool {
        if normalized_query.is_empty() {
            return true;
        }
        format!(
            "{} {} {} {} {}",
            process.pid,
            process.npu_index,
            process.chip_id,
            process.device_key,
            process.process_name
        )
        .to_ascii_lowercase()
        .contains(normalized_query)
    }

    fn sort_processes(processes: &mut [Self::Process]) {
        processes.sort_by(|left, right| {
            right
                .used_memory_mb
                .cmp(&left.used_memory_mb)
                .then_with(|| left.npu_index.cmp(&right.npu_index))
                .then_with(|| left.chip_id.cmp(&right.chip_id))
                .then_with(|| left.pid.cmp(&right.pid))
        });
    }
}

fn gpu_device_key(index: u32, uuid: &str) -> String {
    let uuid = uuid.trim();
    if uuid.is_empty() {
        index.to_string()
    } else {
        uuid.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nyaterm_transport::{
        DockerContainer, DockerContainerDetails, DockerImage, NetworkInfo, NetworkSummaryInfo,
        RemoteDockerOverview, RemoteGpu, RemoteGpuOverview, RemoteGpuProcess, RemoteNpu,
        RemoteNpuOverview, RemoteNpuProcess, RemoteProcess, RemoteStats,
    };

    use super::{
        AcceleratorProcessList, DockerDerivedItems, ProcessApplyOutcome, ProcessSortColumns,
        RemoteOpsFeatureFocus, RemoteOpsFeatureState, StatsApplyOutcome,
    };
    use crate::features::remote::job_state::RemoteJobState;
    use crate::features::runtime_jobs::{
        DockerResource, ProcessJobOutput, ProcessJobResult, StatsJobResult,
    };
    use crate::models::{DockerTab, RemoteProcessSortKey};

    fn process(pid: u32) -> RemoteProcess {
        RemoteProcess {
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

    fn gpu_process(pid: u32) -> RemoteGpuProcess {
        RemoteGpuProcess {
            gpu_uuid: "gpu".to_string(),
            gpu_index: Some(0),
            pid,
            process_name: "proc".to_string(),
            used_memory_mb: u64::from(pid),
        }
    }

    fn docker_container(id: &str, name: &str) -> DockerContainer {
        DockerContainer {
            id: id.to_string(),
            name: name.to_string(),
            image: "image".to_string(),
            status: "Up".to_string(),
            state: "running".to_string(),
            ports: String::new(),
            created_at: String::new(),
            size: String::new(),
            stats: None,
        }
    }

    fn docker_image(id: &str, repository: &str) -> DockerImage {
        DockerImage {
            id: id.to_string(),
            repository: repository.to_string(),
            tag: "latest".to_string(),
            size: String::new(),
            created_since: String::new(),
        }
    }

    fn derived_containers(items: DockerDerivedItems) -> Arc<[DockerContainer]> {
        match items {
            DockerDerivedItems::Containers(items) => items,
            _ => panic!("expected derived Docker containers"),
        }
    }

    fn derived_images(items: DockerDerivedItems) -> Arc<[DockerImage]> {
        match items {
            DockerDerivedItems::Images(items) => items,
            _ => panic!("expected derived Docker images"),
        }
    }

    fn gpu(index: u32, uuid: &str) -> RemoteGpu {
        RemoteGpu {
            index,
            uuid: uuid.to_string(),
            name: format!("GPU {index}"),
            temperature_c: None,
            utilization_gpu_percent: None,
            utilization_memory_percent: None,
            memory_total_mb: 0,
            memory_used_mb: 0,
            memory_free_mb: 0,
            power_draw_w: None,
            power_limit_w: None,
            fan_speed_percent: None,
            pstate: String::new(),
        }
    }

    fn npu(index: u32, chip_id: u32, device_key: &str) -> RemoteNpu {
        RemoteNpu {
            index,
            chip_id,
            physical_id: None,
            device_key: device_key.to_string(),
            name: format!("NPU {index}:{chip_id}"),
            health: String::new(),
            bus_id: String::new(),
            temperature_c: None,
            utilization_aicore_percent: None,
            utilization_memory_percent: None,
            memory_total_mb: 0,
            memory_used_mb: 0,
            memory_free_mb: 0,
            memory_kind: String::new(),
            hbm_total_mb: None,
            hbm_used_mb: None,
            power_draw_w: None,
        }
    }

    #[test]
    fn remote_job_state_matches_job_and_session_before_completion() {
        let mut state = RemoteJobState::<u8>::new();
        let mut rx = state
            .take_event_receiver()
            .expect("the state holds its receiver until the drain starts");
        let first = state.begin("session-a".to_string());
        first
            .tx
            .unbounded_send(7)
            .expect("receiver should stay owned");

        let second = state.begin("session-b".to_string());

        assert_eq!(rx.try_recv().ok(), Some(7));
        assert!(!state.complete_if_matches(first.job_id, "session-a"));
        assert!(state.is_pending_for("session-b"));
        assert!(!state.complete_if_matches(second.job_id, "session-a"));
        assert!(state.complete_if_matches(second.job_id, "session-b"));
        assert!(!state.is_pending());
    }

    #[test]
    fn remote_job_state_tracks_refresh_failures_and_resets_session_runtime() {
        let mut state = RemoteJobState::<u8>::new();
        state.begin("session-a".to_string());
        state.mark_refresh_started();
        assert_eq!(state.record_refresh_failure(false), 1);
        assert_eq!(state.record_refresh_failure(true), 3);

        state.reset_for_session_switch();

        assert!(!state.is_pending());
        assert!(state.last_refresh_at().is_none());
        assert_eq!(state.consecutive_refresh_failures(), 0);
    }

    #[test]
    fn docker_owner_excludes_menus_and_cleans_removed_details_and_compose_data() {
        let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});

        state.toggle_docker_tab_menu();
        state.toggle_docker_header_menu();
        let presentation = state.docker_presentation();
        assert!(!presentation.tab_menu_open);
        assert!(state.docker_header_menu_open());

        state.toggle_docker_container_menu("container".to_string());
        state.toggle_docker_compose_menu("compose".to_string());
        let presentation = state.docker_presentation();
        assert!(!state.docker_header_menu_open());
        assert!(state.docker_menus_open());
        assert!(presentation.container_menu_id.is_none());
        assert_eq!(presentation.compose_menu_id.as_deref(), Some("compose"));

        state.start_docker_details("gone".to_string(), "loading".to_string());
        state.apply_docker_details("gone".to_string(), DockerContainerDetails::default());
        state.toggle_compose_project("old".to_string(), "old");
        state.set_compose_services("old".to_string(), Vec::new());
        state.toggle_compose_project("failed".to_string(), "failed");
        state.set_compose_service_error("failed".to_string(), "error".to_string());
        state.apply_docker_overview(RemoteDockerOverview::default());

        let presentation = state.docker_presentation();
        let second_presentation = state.docker_presentation();
        assert!(Arc::ptr_eq(
            presentation.overview.as_ref().expect("overview"),
            second_presentation.overview.as_ref().expect("overview"),
        ));
        assert!(Arc::ptr_eq(
            &presentation.compose_expanded,
            &second_presentation.compose_expanded,
        ));
        assert!(Arc::ptr_eq(
            &presentation.compose_services,
            &second_presentation.compose_services,
        ));
        assert!(Arc::ptr_eq(
            &presentation.compose_service_errors,
            &second_presentation.compose_service_errors,
        ));
        assert!(presentation.details.is_none());
        assert!(presentation.details_container_id.is_none());
        assert!(state.docker_details_refresh().is_none());
        assert!(presentation.compose_expanded.is_empty());
        assert!(presentation.compose_services.is_empty());
        assert!(presentation.compose_service_errors.is_empty());
    }

    #[test]
    fn docker_owner_caches_active_tab_derivations_and_invalidates_changed_inputs() {
        let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
        state.apply_docker_overview(RemoteDockerOverview {
            available: true,
            containers: vec![
                docker_container("one", "alpha"),
                docker_container("two", "beta"),
            ],
            images: vec![docker_image("image-one", "alpha-image")],
            ..Default::default()
        });

        let initial = derived_containers(state.derived_docker_items());
        let reused = derived_containers(state.derived_docker_items());
        assert!(Arc::ptr_eq(&initial, &reused));
        assert_eq!(initial.len(), 2);

        state.apply_docker_search("alpha".to_string());
        let searched = derived_containers(state.derived_docker_items());
        assert!(!Arc::ptr_eq(&initial, &searched));
        assert_eq!(searched.len(), 1);
        assert_eq!(searched[0].id, "one");

        state.apply_docker_search("  ALPHA  ".to_string());
        let normalized = derived_containers(state.derived_docker_items());
        assert!(Arc::ptr_eq(&searched, &normalized));

        state.set_docker_tab(DockerTab::Images);
        let images = derived_images(state.derived_docker_items());
        assert_eq!(images.len(), 1);
        assert!(Arc::ptr_eq(
            &images,
            &derived_images(state.derived_docker_items()),
        ));

        state.set_docker_tab(DockerTab::Containers);
        let before_refresh = derived_containers(state.derived_docker_items());
        state.apply_docker_overview(RemoteDockerOverview {
            available: true,
            containers: vec![docker_container("three", "alpha-new")],
            ..Default::default()
        });
        let refreshed = derived_containers(state.derived_docker_items());
        assert!(!Arc::ptr_eq(&before_refresh, &refreshed));
        assert_eq!(refreshed[0].id, "three");

        state.clear_docker_overview();
        let cleared = derived_containers(state.derived_docker_items());
        assert!(!Arc::ptr_eq(&refreshed, &cleared));
        assert!(cleared.is_empty());
    }

    #[test]
    fn docker_summary_preserves_loaded_resources_and_empty_lists_are_loaded() {
        let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
        state.apply_docker_summary(RemoteDockerOverview {
            available: true,
            containers: vec![docker_container("one", "first")],
            ..Default::default()
        });
        state.set_docker_tab(DockerTab::Images);
        assert!(state.docker_resource_load_due(10));
        state.mark_docker_resource_started(DockerTab::Images);
        assert!(!state.docker_resource_load_due(10));

        state.apply_docker_resource(DockerResource::Images(Vec::new()));
        assert!(!state.docker_resource_load_due(10));
        assert!(
            state
                .docker_presentation()
                .loaded_resources
                .contains(&DockerTab::Images)
        );
        state.apply_docker_resource(DockerResource::Images(vec![docker_image("img", "repo")]));
        state.apply_docker_summary(RemoteDockerOverview {
            available: true,
            containers: vec![docker_container("two", "second")],
            ..Default::default()
        });
        let overview = state
            .docker_presentation()
            .overview
            .expect("Docker overview");
        assert_eq!(overview.containers[0].id, "two");
        assert_eq!(overview.images[0].id, "img");

        state.set_docker_tab(DockerTab::Volumes);
        assert!(state.docker_resource_load_due(10));
        state.reset_for_session_switch();
        assert!(state.docker_presentation().loaded_resources.is_empty());
        state.apply_docker_summary(RemoteDockerOverview {
            available: true,
            ..Default::default()
        });
        assert!(state.docker_resource_load_due(10));
        assert!(
            state
                .docker_presentation()
                .overview
                .expect("new host")
                .images
                .is_empty()
        );
    }

    #[test]
    fn docker_summary_disables_compose_without_dropping_other_resource_caches() {
        let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
        state.apply_docker_summary(RemoteDockerOverview {
            available: true,
            compose_available: true,
            ..Default::default()
        });
        state.apply_docker_resource(DockerResource::Compose(vec![
            nyaterm_transport::DockerComposeProject {
                name: "project".to_string(),
                status: "running".to_string(),
                config_files: "/compose.yml".to_string(),
            },
        ]));
        state.apply_docker_resource(DockerResource::Images(vec![docker_image("img", "repo")]));
        state.apply_docker_summary(RemoteDockerOverview {
            available: true,
            compose_available: false,
            ..Default::default()
        });
        let presentation = state.docker_presentation();
        let overview = presentation.overview.expect("Docker overview");
        assert!(overview.compose_projects.is_empty());
        assert!(!presentation.loaded_resources.contains(&DockerTab::Compose));
        assert_eq!(overview.images[0].id, "img");
    }

    #[test]
    fn process_owner_cleans_pid_scoped_interaction_when_results_change() {
        let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
        state.apply_processes(vec![process(42)]);
        state.toggle_process_selection(42);
        state.toggle_process_menu(42);
        state.apply_process_nice_input("-1234x".to_string());

        let presentation = state.process_presentation();
        assert_eq!(presentation.nice_draft, "-123");
        assert_eq!(presentation.selected_pid, Some(42));
        assert_eq!(presentation.menu_pid, Some(42));

        state.apply_processes(Vec::new());

        let presentation = state.process_presentation();
        assert!(presentation.selected_pid.is_none());
        assert!(presentation.menu_pid.is_none());
        assert_eq!(presentation.nice_draft, "0");
    }

    #[test]
    fn process_owner_caches_derived_items_by_data_search_and_sort() {
        let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
        let mut first_process = process(1);
        first_process.command = "alpha".to_string();
        let mut second_process = process(2);
        second_process.command = "beta".to_string();
        state.apply_processes(vec![first_process, second_process]);

        let initial = state.derived_processes();
        assert!(Arc::ptr_eq(&initial, &state.derived_processes()));

        state.apply_process_search("alpha".to_string());
        let searched = state.derived_processes();
        assert!(!Arc::ptr_eq(&initial, &searched));
        assert_eq!(searched.len(), 1);

        state.toggle_process_sort(RemoteProcessSortKey::Pid);
        let sorted = state.derived_processes();
        assert!(!Arc::ptr_eq(&searched, &sorted));

        state.apply_processes(vec![process(3)]);
        let refreshed = state.derived_processes();
        assert!(!Arc::ptr_eq(&sorted, &refreshed));
    }

    #[test]
    fn process_owner_reduces_matching_and_stale_job_events() {
        let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
        let ticket = state.begin_process_job("session-a".to_string());
        let outcome = state.apply_process_event(
            ProcessJobResult {
                job_id: ticket.job_id,
                session_id: "session-a".to_string(),
                result: Ok(ProcessJobOutput::Listed(vec![process(7)])),
            },
            Some("session-a"),
        );
        assert!(matches!(outcome, ProcessApplyOutcome::Applied { .. }));
        assert_eq!(state.process_presentation().items[0].pid, 7);

        assert!(matches!(
            state.apply_process_event(
                ProcessJobResult {
                    job_id: ticket.job_id,
                    session_id: "session-a".to_string(),
                    result: Err("stale".to_string()),
                },
                Some("session-a"),
            ),
            ProcessApplyOutcome::Ignored
        ));
    }

    #[test]
    fn stats_owner_resets_session_runtime_without_losing_expansion_preference() {
        let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
        state.toggle_stats_cpu_expanded();
        state.begin_stats_job("session-a".to_string(), false);

        state.reset_for_session_switch();

        let presentation = state.stats_presentation();
        assert!(!presentation.pending);
        assert!(presentation.data.is_none());
        assert!(presentation.cpu_expanded);
        assert_eq!(
            state.stats_status(),
            "start an SSH session to inspect remote stats"
        );
    }

    #[test]
    fn stats_owner_reduces_matching_success_failure_and_stale_events() {
        let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
        let ticket = state.begin_stats_job("session-a".to_string(), false);
        let mut stats = RemoteStats::default();
        stats.system.hostname = "host-a".to_string();
        let outcome = state.apply_stats_event(
            StatsJobResult {
                job_id: ticket.job_id,
                session_id: "session-a".to_string(),
                result: Ok(stats),
            },
            Some("session-a"),
        );
        assert!(matches!(outcome, StatsApplyOutcome::Applied { .. }));
        assert_eq!(
            state.stats_presentation().data.unwrap().system.hostname,
            "host-a"
        );

        assert!(matches!(
            state.apply_stats_event(
                StatsJobResult {
                    job_id: ticket.job_id,
                    session_id: "session-a".to_string(),
                    result: Err("stale".to_string()),
                },
                Some("session-a"),
            ),
            StatsApplyOutcome::Ignored
        ));

        for failure in 1..=3 {
            let ticket = state.begin_stats_job("session-a".to_string(), false);
            let outcome = state.apply_stats_event(
                StatsJobResult {
                    job_id: ticket.job_id,
                    session_id: "session-a".to_string(),
                    result: Err(format!("failure-{failure}")),
                },
                Some("session-a"),
            );
            assert!(matches!(outcome, StatsApplyOutcome::Failed { .. }));
        }
        let presentation = state.stats_presentation();
        assert!(presentation.data.is_none());
        assert_eq!(presentation.consecutive_refresh_failures, 3);
    }

    #[test]
    fn stats_network_history_is_bounded_isolated_and_cleaned_up() {
        fn stats(rate: f64) -> RemoteStats {
            RemoteStats {
                networks: vec![NetworkInfo {
                    nic: "eth0".to_string(),
                    state: "up".to_string(),
                    rx_bytes_per_sec: rate,
                    tx_bytes_per_sec: rate * 2.0,
                }],
                network_summary: NetworkSummaryInfo {
                    rx_bytes_per_sec: rate,
                    tx_bytes_per_sec: rate * 2.0,
                },
                ..Default::default()
            }
        }

        let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
        for rate in 0..61 {
            let ticket = state.begin_stats_job("session-a".to_string(), false);
            assert!(matches!(
                state.apply_stats_event(
                    StatsJobResult {
                        job_id: ticket.job_id,
                        session_id: "session-a".to_string(),
                        result: Ok(stats(rate as f64)),
                    },
                    Some("session-a"),
                ),
                StatsApplyOutcome::Applied { .. }
            ));
        }
        let history = state.stats_presentation().network_history;
        assert_eq!(history.len(), 60);
        assert_eq!(history[0].rx_bytes_per_sec, 1.0);
        assert_eq!(history[59].interfaces["eth0"], (60.0, 120.0));

        let ticket = state.begin_stats_job("session-b".to_string(), false);
        assert!(matches!(
            state.apply_stats_event(
                StatsJobResult {
                    job_id: ticket.job_id,
                    session_id: "session-b".to_string(),
                    result: Ok(stats(100.0)),
                },
                Some("session-a"),
            ),
            StatsApplyOutcome::CompletedInactive
        ));
        state.activate_stats_session("session-b");
        assert_eq!(
            state.stats_presentation().network_history[0].rx_bytes_per_sec,
            100.0
        );
        state.activate_stats_session("session-a");
        assert_eq!(state.stats_presentation().network_history.len(), 60);

        state.clear_stats_sample("session-a");
        assert!(state.stats_presentation().network_history.is_empty());
    }

    #[test]
    fn accelerator_owner_tracks_search_offsets_and_prunes_missing_gpu_devices() {
        let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
        state.apply_gpu(
            "session-a",
            RemoteGpuOverview {
                available: true,
                gpus: vec![gpu(0, "gpu-a"), gpu(1, "gpu-b")],
                ..Default::default()
            },
        );

        state.toggle_gpu_device_expanded("gpu-a".to_string());
        assert!(state.set_gpu_process_offset(12));
        assert_eq!(state.gpu_presentation().process_list_offset, 12);

        state.apply_gpu_search("python".to_string());
        let presentation = state.gpu_presentation();
        assert_eq!(presentation.search_draft, "python");
        assert_eq!(presentation.process_list_offset, 0);
        assert!(presentation.expanded_devices.contains("gpu-a"));

        state.toggle_gpu_device_expanded("gpu-b".to_string());
        state.apply_gpu(
            "session-a",
            RemoteGpuOverview {
                available: true,
                gpus: vec![gpu(1, "gpu-b")],
                ..Default::default()
            },
        );

        let presentation = state.gpu_presentation();
        assert!(!presentation.expanded_devices.contains("gpu-a"));
        assert!(presentation.expanded_devices.contains("gpu-b"));
    }

    #[test]
    fn accelerator_owner_tracks_search_offsets_and_prunes_missing_npu_devices() {
        let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
        state.apply_npu(
            "session-a",
            RemoteNpuOverview {
                available: true,
                npus: vec![npu(0, 0, "npu-a"), npu(1, 0, "npu-b")],
                ..Default::default()
            },
        );

        state.toggle_npu_device_expanded("npu-a".to_string());
        assert!(state.set_npu_process_offset(9));
        assert_eq!(state.npu_presentation().process_list_offset, 9);

        state.apply_npu_search("train".to_string());
        let presentation = state.npu_presentation();
        assert_eq!(presentation.search_draft, "train");
        assert_eq!(presentation.process_list_offset, 0);
        assert!(presentation.expanded_devices.contains("npu-a"));

        state.toggle_npu_device_expanded("npu-b".to_string());
        state.apply_npu(
            "session-a",
            RemoteNpuOverview {
                available: true,
                npus: vec![npu(1, 0, "npu-b")],
                ..Default::default()
            },
        );

        let presentation = state.npu_presentation();
        assert!(!presentation.expanded_devices.contains("npu-a"));
        assert!(presentation.expanded_devices.contains("npu-b"));
    }

    #[test]
    fn accelerator_owner_caches_unavailable_sessions_until_success() {
        let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
        let session_id = "session-a";

        state.apply_gpu(
            session_id,
            nyaterm_transport::RemoteGpuOverview {
                available: false,
                ..Default::default()
            },
        );

        assert!(state.gpu_unavailable_for(session_id));

        state.reset_for_session_switch();
        assert!(state.gpu_unavailable_for(session_id));

        state.apply_gpu(
            session_id,
            nyaterm_transport::RemoteGpuOverview {
                available: true,
                ..Default::default()
            },
        );

        assert!(!state.gpu_unavailable_for(session_id));
    }

    #[test]
    /// Moved here with the four functions it exercises, which used to run inside
    /// `stats_view`'s render pass.
    fn gpu_process_filter_and_sort_follow_tauri_rules() {
        let mut processes = vec![
            RemoteGpuProcess {
                gpu_uuid: "gpu-b".to_string(),
                gpu_index: Some(1),
                pid: 20,
                process_name: "python".to_string(),
                used_memory_mb: 2048,
            },
            RemoteGpuProcess {
                gpu_uuid: "gpu-a".to_string(),
                gpu_index: Some(0),
                pid: 10,
                process_name: "worker".to_string(),
                used_memory_mb: 4096,
            },
            RemoteGpuProcess {
                gpu_uuid: "gpu-c".to_string(),
                gpu_index: None,
                pid: 5,
                process_name: "python".to_string(),
                used_memory_mb: 2048,
            },
        ];

        assert!(RemoteGpuOverview::process_matches(&processes[0], "python"));
        assert!(RemoteGpuOverview::process_matches(&processes[1], "10"));
        assert!(RemoteGpuOverview::process_matches(&processes[2], "gpu-c"));

        RemoteGpuOverview::sort_processes(&mut processes);
        assert_eq!(
            processes
                .iter()
                .map(|process| process.pid)
                .collect::<Vec<_>>(),
            [10, 20, 5]
        );
    }

    #[test]
    fn npu_process_filter_and_sort_follow_tauri_rules() {
        let mut processes = vec![
            RemoteNpuProcess {
                npu_index: 1,
                chip_id: 0,
                device_key: "npu-b".to_string(),
                pid: 30,
                process_name: "train".to_string(),
                used_memory_mb: 1024,
            },
            RemoteNpuProcess {
                npu_index: 0,
                chip_id: 1,
                device_key: "npu-a".to_string(),
                pid: 20,
                process_name: "infer".to_string(),
                used_memory_mb: 2048,
            },
            RemoteNpuProcess {
                npu_index: 0,
                chip_id: 0,
                device_key: "npu-c".to_string(),
                pid: 10,
                process_name: "train".to_string(),
                used_memory_mb: 2048,
            },
        ];

        assert!(RemoteNpuOverview::process_matches(&processes[0], "train"));
        assert!(RemoteNpuOverview::process_matches(&processes[1], "0 1"));
        assert!(RemoteNpuOverview::process_matches(&processes[2], "npu-c"));

        RemoteNpuOverview::sort_processes(&mut processes);
        assert_eq!(
            processes
                .iter()
                .map(|process| process.pid)
                .collect::<Vec<_>>(),
            [10, 20, 30]
        );
    }

    /// And for a GPU card process list, which had no derived cache at all before: its
    /// filtering ran inside `stats_view`, so only the view knew the row count.
    #[test]
    fn shorter_gpu_results_clamp_the_stored_offset_with_no_render() {
        let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
        state.apply_gpu(
            "session",
            RemoteGpuOverview {
                available: true,
                processes: (1..=40).map(gpu_process).collect(),
                ..Default::default()
            },
        );
        assert!(state.set_gpu_process_offset(30));

        state.apply_gpu(
            "session",
            RemoteGpuOverview {
                available: true,
                processes: (1..=2).map(gpu_process).collect(),
                ..Default::default()
            },
        );

        assert_eq!(state.gpu_presentation().process_list_offset, 0);
        assert_eq!(state.derived_gpu_processes().len(), 2);
    }

    /// A query or sort change must leave the derived list correct for a reader holding
    /// only `&self`.
    ///
    /// The accessor taking `&self` is the point: the old one took `&mut self` and
    /// computed on demand, so the only reader that could see a fresh list was one able
    /// to mutate -- which is why the render pass had to. The values are asserted too, so
    /// a future change that makes the recompute lazy again fails here rather than merely
    /// failing to compile.
    #[test]
    fn a_query_or_sort_change_leaves_the_derived_list_correct_for_a_reader() {
        fn read(state: &RemoteOpsFeatureState) -> Vec<u32> {
            state
                .derived_processes()
                .iter()
                .map(|process| process.pid)
                .collect()
        }

        let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
        let mut alpha = process(1);
        alpha.command = "alpha".to_string();
        let mut beta = process(2);
        beta.command = "beta".to_string();
        state.apply_processes(vec![alpha, beta]);
        assert_eq!(read(&state).len(), 2);

        state.apply_process_search("beta".to_string());
        assert_eq!(read(&state), vec![2], "the query narrowed the list");

        state.apply_process_search(String::new());
        state.toggle_process_sort(RemoteProcessSortKey::Pid);
        assert_eq!(read(&state), vec![1, 2], "ascending by pid");
        state.toggle_process_sort(RemoteProcessSortKey::Pid);
        assert_eq!(read(&state), vec![2, 1], "and reversed");
    }

    /// Narrowing the panel must move the sort key off a column that is gone.
    ///
    /// The width lives in the shell, so it is pushed in rather than observed. The
    /// derived list has to follow, because the sort key is part of its cache key -- a
    /// constrain that forgot to recompute would leave rows ordered by a hidden column.
    #[test]
    fn narrowing_the_panel_moves_the_sort_key_off_a_hidden_column() {
        let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
        let mut low_memory = process(1);
        low_memory.memory_percent = 1.0;
        low_memory.cpu_percent = 9.0;
        let mut high_memory = process(2);
        high_memory.memory_percent = 9.0;
        high_memory.cpu_percent = 1.0;
        state.apply_processes(vec![low_memory, high_memory]);
        state.toggle_process_sort(RemoteProcessSortKey::Memory);
        assert_eq!(
            state.process_presentation().sort_key,
            RemoteProcessSortKey::Memory
        );
        assert_eq!(state.derived_processes()[0].pid, 2, "highest memory first");

        // A narrow panel shows neither the memory nor the user column.
        let narrow = ProcessSortColumns {
            allow_memory: false,
            allow_user: false,
        };
        assert!(state.set_process_sort_columns(narrow));

        assert_eq!(
            state.process_presentation().sort_key,
            RemoteProcessSortKey::Cpu,
            "memory is not sortable at this width"
        );
        assert_eq!(
            state.derived_processes()[0].pid,
            1,
            "and the list is re-sorted by cpu, not left ordered by the hidden column"
        );
        assert!(
            !state.set_process_sort_columns(narrow),
            "an unchanged width must not report a change"
        );
    }

    /// Every mutation that changes a pane presentation must bump that pane revision,
    /// and must not bump another pane.
    ///
    /// The flush compares this counter to decide whether to build and push a snapshot,
    /// so a mutator that changes the presentation without bumping it would leave the
    /// panel showing stale data with nothing to detect it. Each pane is asserted
    /// individually rather than in a loop, because the point is coverage of the
    /// mutators, and a loop would hide which one stopped bumping.
    #[test]
    fn presentation_mutations_bump_only_their_own_pane_revision() {
        let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});

        let expect_bump = |state: &mut RemoteOpsFeatureState,
                           what: &str,
                           mutate: &dyn Fn(&mut RemoteOpsFeatureState)| {
            let before = (
                state.stats_revision(),
                state.gpu_revision(),
                state.npu_revision(),
            );
            mutate(state);
            let after = (
                state.stats_revision(),
                state.gpu_revision(),
                state.npu_revision(),
            );
            assert_ne!(before, after, "{what} must bump a revision");
            after
        };

        // Stats.
        let mut previous = (
            state.stats_revision(),
            state.gpu_revision(),
            state.npu_revision(),
        );
        for (what, mutate) in [
            (
                "apply_stats",
                &(|state: &mut RemoteOpsFeatureState| state.apply_stats(RemoteStats::default()))
                    as &dyn Fn(&mut RemoteOpsFeatureState),
            ),
            ("set_stats_status", &|state: &mut RemoteOpsFeatureState| {
                state.set_stats_status("loading")
            }),
            (
                "toggle_stats_cpu_expanded",
                &|state: &mut RemoteOpsFeatureState| state.toggle_stats_cpu_expanded(),
            ),
            (
                "record_stats_refresh_failure",
                &|state: &mut RemoteOpsFeatureState| {
                    state.record_stats_refresh_failure();
                },
            ),
            (
                "reset_stats_refresh_failures",
                &|state: &mut RemoteOpsFeatureState| state.reset_stats_refresh_failures(),
            ),
        ] {
            let after = expect_bump(&mut state, what, mutate);
            assert_eq!(
                (after.1, after.2),
                (previous.1, previous.2),
                "{what} must not disturb the GPU or NPU panes"
            );
            previous = after;
        }

        // GPU, and the NPU pane must not move with it.
        let after = expect_bump(
            &mut state,
            "apply_gpu",
            &|state: &mut RemoteOpsFeatureState| {
                state.apply_gpu("session", RemoteGpuOverview::default())
            },
        );
        assert_eq!(after.2, previous.2, "apply_gpu must not touch the NPU pane");
        previous = after;

        for (what, mutate) in [
            (
                "apply_gpu_search",
                &(|state: &mut RemoteOpsFeatureState| state.apply_gpu_search("q".to_string()))
                    as &dyn Fn(&mut RemoteOpsFeatureState),
            ),
            (
                "toggle_gpu_device_expanded",
                &|state: &mut RemoteOpsFeatureState| {
                    state.toggle_gpu_device_expanded("0".to_string())
                },
            ),
            ("set_gpu_status", &|state: &mut RemoteOpsFeatureState| {
                state.set_gpu_status("loading")
            }),
        ] {
            let after = expect_bump(&mut state, what, mutate);
            assert_eq!(after.2, previous.2, "{what} must not touch the NPU pane");
            previous = after;
        }

        // NPU.
        let after = expect_bump(
            &mut state,
            "apply_npu",
            &|state: &mut RemoteOpsFeatureState| {
                state.apply_npu("session", RemoteNpuOverview::default())
            },
        );
        assert_eq!(
            after.0, previous.0,
            "apply_npu must not touch the stats pane"
        );
        previous = after;

        // A session switch clears every pane, so it must move every revision. This
        // arm was missing when the counter first landed, and the stats pane silently
        // did not bump: `reset_for_session_switch` wrote `data` and `status` directly
        // rather than going through the methods that touch. A workspace-restore test
        // caught it, one layer further out than it should have needed.
        let after = expect_bump(
            &mut state,
            "reset_for_session_switch",
            &|state: &mut RemoteOpsFeatureState| state.reset_for_session_switch(),
        );
        assert_ne!(after.0, previous.0, "the stats revision must advance");
        assert_ne!(after.1, previous.1, "the GPU revision must advance");
        assert_ne!(after.2, previous.2, "the NPU revision must advance");
    }

    /// Every Docker mutation must bump the Docker pane revision, and no other pane's.
    ///
    /// Seventeen mutators on `RemoteOpsFeatureState` write Docker fields directly -- menus,
    /// offsets, details, compose state -- so nothing structural stops one from being added
    /// without a touch. This drives every one of them; it is what makes the counter
    /// trustworthy rather than the field access pattern.
    #[test]
    fn docker_presentation_mutations_bump_the_revision() {
        let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
        state.apply_docker_overview(RemoteDockerOverview {
            available: true,
            containers: vec![docker_container("c0", "name")],
            images: (0..40)
                .map(|index| docker_image(&format!("i{index}"), "repo"))
                .collect(),
            ..Default::default()
        });

        type Mutation = Box<dyn Fn(&mut RemoteOpsFeatureState)>;
        let mutations: Vec<(&str, Mutation)> = vec![
            (
                "set_docker_status",
                Box::new(|s: &mut RemoteOpsFeatureState| s.set_docker_status("x")),
            ),
            (
                "apply_docker_search",
                Box::new(|s: &mut RemoteOpsFeatureState| s.apply_docker_search("q".to_string())),
            ),
            (
                "set_docker_tab",
                Box::new(|s: &mut RemoteOpsFeatureState| s.set_docker_tab(DockerTab::Images)),
            ),
            (
                "toggle_docker_tab_menu",
                Box::new(|s: &mut RemoteOpsFeatureState| s.toggle_docker_tab_menu()),
            ),
            (
                "toggle_docker_header_menu",
                Box::new(|s: &mut RemoteOpsFeatureState| s.toggle_docker_header_menu()),
            ),
            (
                "close_docker_menus",
                Box::new(|s: &mut RemoteOpsFeatureState| {
                    s.close_docker_menus();
                }),
            ),
            (
                "toggle_docker_container_menu",
                Box::new(|s: &mut RemoteOpsFeatureState| {
                    s.toggle_docker_container_menu("c0".to_string())
                }),
            ),
            (
                "close_docker_container_menu",
                Box::new(|s: &mut RemoteOpsFeatureState| s.close_docker_container_menu()),
            ),
            (
                "toggle_docker_compose_menu",
                Box::new(|s: &mut RemoteOpsFeatureState| {
                    s.toggle_docker_compose_menu("k".to_string())
                }),
            ),
            (
                "close_docker_compose_menu",
                Box::new(|s: &mut RemoteOpsFeatureState| s.close_docker_compose_menu()),
            ),
            (
                "toggle_compose_project",
                Box::new(|s: &mut RemoteOpsFeatureState| {
                    s.toggle_compose_project("k".to_string(), "proj");
                }),
            ),
            (
                "start_docker_container_action",
                Box::new(|s: &mut RemoteOpsFeatureState| {
                    s.start_docker_container_action("acting".to_string())
                }),
            ),
            (
                "start_docker_details",
                Box::new(|s: &mut RemoteOpsFeatureState| {
                    s.start_docker_details("c0".to_string(), "loading".to_string())
                }),
            ),
            (
                "apply_docker_details",
                Box::new(|s: &mut RemoteOpsFeatureState| {
                    s.apply_docker_details("c0".to_string(), DockerContainerDetails::default())
                }),
            ),
            (
                "close_docker_details",
                Box::new(|s: &mut RemoteOpsFeatureState| s.close_docker_details()),
            ),
            (
                "set_compose_services",
                Box::new(|s: &mut RemoteOpsFeatureState| {
                    s.set_compose_services("k".to_string(), Vec::new())
                }),
            ),
            (
                "set_compose_service_error",
                Box::new(|s: &mut RemoteOpsFeatureState| {
                    s.set_compose_service_error("k".to_string(), "boom".to_string())
                }),
            ),
            (
                "clear_compose_service_error",
                Box::new(|s: &mut RemoteOpsFeatureState| s.clear_compose_service_error("k")),
            ),
            (
                "begin_docker_job",
                Box::new(|s: &mut RemoteOpsFeatureState| {
                    s.begin_docker_job("session".to_string());
                }),
            ),
            (
                "clear_docker_overview",
                Box::new(|s: &mut RemoteOpsFeatureState| s.clear_docker_overview()),
            ),
            (
                "reset_for_session_switch",
                Box::new(|s: &mut RemoteOpsFeatureState| s.reset_for_session_switch()),
            ),
        ];

        for (label, mutate) in mutations {
            let before = (
                state.docker_revision(),
                state.stats_revision(),
                state.process_revision(),
            );
            mutate(&mut state);
            let after = (
                state.docker_revision(),
                state.stats_revision(),
                state.process_revision(),
            );
            assert_ne!(
                before.0, after.0,
                "{label} changes the Docker presentation and must bump its revision"
            );
            if label != "reset_for_session_switch" {
                assert_eq!(
                    (after.1, after.2),
                    (before.1, before.2),
                    "{label} must not disturb the stats or process panes"
                );
            }
        }
    }

    /// The failure ladder clears the overview at three, and that clear has to bump.
    ///
    /// `record_docker_refresh_failure` returning >= 3 is what makes the caller call
    /// `clear_docker_overview`, so the two are only correct together: a bump on the record
    /// but not the clear would leave a panel showing containers the pane no longer has.
    #[test]
    fn the_docker_failure_ladder_clears_the_overview_and_bumps() {
        let mut state = RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {});
        state.apply_docker_overview(RemoteDockerOverview {
            available: true,
            containers: vec![docker_container("c0", "name")],
            ..Default::default()
        });
        assert!(state.docker_presentation().overview.is_some());

        // The failure count is deliberately *not* part of the Docker presentation --
        // unlike the accelerator panes, whose title-bar readout carries it -- so recording
        // a failure changes nothing rendered and must not bump. What is rendered is the
        // overview, which the third failure drops.
        let revision = state.docker_revision();
        let mut failures = 0;
        for _ in 0..3 {
            failures = state.record_docker_refresh_failure();
        }
        assert_eq!(failures, 3, "three failures is the documented threshold");
        assert_eq!(
            state.docker_revision(),
            revision,
            "a failure streak alone changes nothing on screen"
        );

        state.clear_docker_overview();
        assert!(
            state.docker_presentation().overview.is_none(),
            "the overview is dropped once the streak reaches three"
        );
        assert_ne!(
            state.docker_revision(),
            revision,
            "and the clear must bump, or the panel keeps rendering stale containers"
        );
    }
}
