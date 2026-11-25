use crate::models::{DockerTab, RemoteProcessSortDirection, RemoteProcessSortKey};
use crate::network::NetworkChart;
use nyaterm_remote_ops::ascend_npu::RemoteNpuOverview;
use nyaterm_remote_ops::docker::{
    DockerComposeProject, DockerComposeService, DockerContainer, DockerContainerDetails,
    DockerImage, DockerNetwork, DockerVolume, RemoteDockerOverview,
};
use nyaterm_remote_ops::gpu::RemoteGpuOverview;
use nyaterm_remote_ops::remote_process::RemoteProcess;
use nyaterm_remote_ops::stats::{NetworkSummaryInfo, RemoteStats};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
#[derive(Clone)]
pub struct GpuPresentationState {
    pub data: Option<RemoteGpuOverview>,
    pub status: String,
    pub search_draft: String,
    pub expanded_devices: HashSet<String>,
    pub process_list_offset: usize,
    pub pending: bool,
    pub consecutive_refresh_failures: u8,
}

#[derive(Clone)]
pub struct NpuPresentationState {
    pub data: Option<RemoteNpuOverview>,
    pub status: String,
    pub search_draft: String,
    pub expanded_devices: HashSet<String>,
    pub process_list_offset: usize,
    pub pending: bool,
    pub consecutive_refresh_failures: u8,
}

#[derive(Clone)]
pub enum DockerDerivedItems {
    Containers(Arc<[DockerContainer]>),
    Images(Arc<[DockerImage]>),
    Volumes(Arc<[DockerVolume]>),
    Networks(Arc<[DockerNetwork]>),
    Compose(Arc<[DockerComposeProject]>),
}

#[derive(Clone)]
pub struct DockerPresentationState {
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
pub struct ProcessPresentationState {
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
pub struct StatsPresentationState {
    pub data: Option<Arc<RemoteStats>>,
    pub network_chart: Arc<NetworkChart>,
    pub network_interfaces_expanded: bool,
    pub last_success_at: Option<Instant>,
    pub selected_network_interface: Option<String>,
    pub cpu_expanded: bool,
    pub memory_expanded: bool,
    pub pending: bool,
    pub error: bool,
    pub consecutive_refresh_failures: u8,
}

impl StatsPresentationState {
    pub fn stale(&self, now: Instant, interval: Duration) -> bool {
        self.error
            || self.last_success_at.is_some_and(|at| {
                now.saturating_duration_since(at)
                    > nyaterm_remote_ops::stats::network_stale_after(interval)
            })
    }

    pub fn network_traffic(&self) -> Option<NetworkSummaryInfo> {
        let stats = self.data.as_ref()?;
        match self.selected_network_interface.as_deref() {
            Some(nic) => stats
                .networks
                .iter()
                .find(|network| network.nic == nic)
                .and_then(|network| network.traffic()),
            None => stats.network_traffic(),
        }
    }
}
pub(crate) use crate::list_window::{ACCELERATOR_PROCESS_VIEWPORT_ROWS, max_list_offset};
