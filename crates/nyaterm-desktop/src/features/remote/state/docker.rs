use crate::features::formatting::docker_compose_project_key;
use crate::features::remote::job_state::{RemoteJobState, RemoteJobTicket};
use crate::features::runtime_jobs::{DockerJobResult, DockerResource};
use crate::models::DockerTab;
use futures::channel::mpsc::UnboundedReceiver;
use nyaterm_remote_ops::docker::{
    DockerComposeProject, DockerComposeService, DockerContainer, DockerContainerDetails,
    DockerImage, DockerNetwork, DockerVolume, RemoteDockerOverview,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(super) struct DockerPaneState {
    job: RemoteJobState<DockerJobResult>,
    overview: Option<Arc<RemoteDockerOverview>>,
    loaded_resources: HashSet<DockerTab>,
    resource_attempts: HashMap<DockerTab, Instant>,
    /// Bumped by every mutation that changes what `docker_presentation` returns.
    revision: u64,
    data_generation: u64,
    derived: Option<DockerDerivedCache>,
    status: String,
    details: Option<DockerContainerDetails>,
    details_container_id: Option<String>,
    details_last_refresh_at: Option<Instant>,
    container_menu_id: Option<String>,
    compose_menu_id: Option<String>,
    tab: DockerTab,
    tab_menu_open: bool,
    header_menu_open: bool,
    search_draft: String,
    compose_expanded: Arc<HashSet<String>>,
    compose_services: Arc<HashMap<String, Vec<DockerComposeService>>>,
    compose_service_errors: Arc<HashMap<String, String>>,
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

impl DockerPaneState {
    pub(super) fn resource_load_due(&self, interval: u32) -> bool {
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

    pub(super) fn is_pending(&self) -> bool {
        self.job.is_pending()
    }

    pub(super) fn last_refresh_at(&self) -> Option<Instant> {
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

    pub(super) fn set_tab(&mut self, tab: DockerTab) {
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

    pub(super) fn toggle_tab_menu(&mut self) {
        self.tab_menu_open = !self.tab_menu_open;
        self.touch();
    }

    pub(super) fn apply_search(&mut self, text: String) {
        self.search_draft = text;
        self.reconcile();
        self.status = "Docker search updated".to_string();
    }

    pub(super) fn close_details(&mut self) {
        self.details = None;
        self.details_container_id = None;
        self.details_last_refresh_at = None;
        self.status = "container details closed".to_string();
        self.touch();
    }

    pub(super) fn apply_overview(&mut self, overview: RemoteDockerOverview) {
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

    pub(super) fn apply_resource(&mut self, resource: DockerResource) {
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

    pub(super) fn clear_overview(&mut self) {
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

    pub(super) fn revision(&self) -> u64 {
        self.revision
    }

    /// The tab actually shown, which is not always the stored one.
    ///
    /// Compose falls back to Containers when the host has no compose support. That
    /// fallback used to be computed in `docker_view` and passed into the derived
    /// lookup, which is why the lookup took a tab argument at all; it belongs here
    /// because the derived list has to be keyed on the tab that is really displayed.
    pub(super) fn effective_tab(&self) -> DockerTab {
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
    pub(super) fn derived(&self) -> DockerDerivedItems {
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

    pub(super) fn reset_for_session_switch(&mut self) {
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

impl DockerPaneState {
    pub(super) fn new() -> Self {
        Self {
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
        }
    }
}

impl DockerPaneState {
    pub(super) fn docker_presentation(&self) -> DockerPresentationState {
        DockerPresentationState {
            overview: self.overview.clone(),
            loaded_resources: self.loaded_resources.clone(),
            status: self.status.clone(),
            details: self.details.clone(),
            details_container_id: self.details_container_id.clone(),
            container_menu_id: self.container_menu_id.clone(),
            compose_menu_id: self.compose_menu_id.clone(),
            tab_menu_open: self.tab_menu_open,
            search_draft: self.search_draft.clone(),
            compose_expanded: self.compose_expanded.clone(),
            compose_services: self.compose_services.clone(),
            compose_service_errors: self.compose_service_errors.clone(),
            pending: self.is_pending(),
        }
    }

    pub(super) fn docker_status(&self) -> &str {
        &self.status
    }

    pub(super) fn set_docker_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
        self.touch();
    }

    pub(super) fn docker_engine_version(&self) -> Option<String> {
        let overview = self
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

    pub(super) fn docker_can_prune(&self) -> bool {
        self.overview
            .as_ref()
            .is_some_and(|overview| overview.available)
    }

    pub(super) fn docker_header_menu_open(&self) -> bool {
        self.header_menu_open
    }

    pub(super) fn docker_details_refresh(&self) -> Option<(String, Instant)> {
        let refresh = (
            self.details_container_id.clone()?,
            self.details_last_refresh_at?,
        );
        if self.details.is_some() {
            Some(refresh)
        } else {
            None
        }
    }

    pub(super) fn toggle_docker_tab_menu(&mut self) {
        self.toggle_tab_menu();
        if self.tab_menu_open {
            self.header_menu_open = false;
            self.container_menu_id = None;
            self.compose_menu_id = None;
        }
        self.touch();
    }

    pub(super) fn toggle_docker_header_menu(&mut self) {
        self.header_menu_open = !self.header_menu_open;
        if self.header_menu_open {
            self.tab_menu_open = false;
            self.container_menu_id = None;
            self.compose_menu_id = None;
        }
        self.touch();
    }

    pub(super) fn close_docker_menus(&mut self) {
        self.tab_menu_open = false;
        self.header_menu_open = false;
        self.container_menu_id = None;
        self.compose_menu_id = None;
        self.touch();
    }

    pub(super) fn docker_menus_open(&self) -> bool {
        self.tab_menu_open
            || self.header_menu_open
            || self.container_menu_id.is_some()
            || self.compose_menu_id.is_some()
    }

    pub(super) fn toggle_docker_container_menu(&mut self, id: String) {
        let open = self.container_menu_id.as_deref() != Some(id.as_str());
        self.container_menu_id = open.then_some(id);
        if open {
            self.tab_menu_open = false;
            self.header_menu_open = false;
            self.compose_menu_id = None;
        }
        self.touch();
    }

    pub(super) fn close_docker_container_menu(&mut self) {
        self.container_menu_id = None;
        self.touch();
    }

    pub(super) fn toggle_docker_compose_menu(&mut self, id: String) {
        let open = self.compose_menu_id.as_deref() != Some(id.as_str());
        self.compose_menu_id = open.then_some(id);
        if open {
            self.tab_menu_open = false;
            self.header_menu_open = false;
            self.container_menu_id = None;
        }
        self.touch();
    }

    pub(super) fn close_docker_compose_menu(&mut self) {
        self.compose_menu_id = None;
        self.touch();
    }

    pub(super) fn toggle_compose_project(&mut self, key: String, project_name: &str) -> bool {
        let expanded = Arc::make_mut(&mut self.compose_expanded);
        if expanded.remove(&key) {
            self.status = format!("collapsed compose project {project_name}");
            return false;
        }
        expanded.insert(key.clone());
        self.status = format!("expanded compose project {project_name}");
        self.touch();
        !self.compose_services.contains_key(&key) && !self.compose_service_errors.contains_key(&key)
    }

    pub(super) fn mark_docker_resource_started(&mut self, tab: DockerTab) {
        self.resource_attempts.insert(tab, Instant::now());
    }

    pub(super) fn start_docker_container_action(&mut self, status: String) {
        self.status = status;
        self.details = None;
        self.details_container_id = None;
        self.touch();
    }

    pub(super) fn start_docker_details(&mut self, container_id: String, status: String) {
        if self.details_container_id.as_ref() != Some(&container_id) {
            self.details = None;
        }
        self.details_container_id = Some(container_id);
        self.details_last_refresh_at = Some(Instant::now());
        self.status = status;
        self.touch();
    }

    pub(super) fn apply_docker_summary(&mut self, mut overview: RemoteDockerOverview) {
        if overview.available
            && let Some(previous) = self.overview.as_deref()
        {
            overview.images = previous.images.clone();
            overview.volumes = previous.volumes.clone();
            overview.networks = previous.networks.clone();
            if overview.compose_available {
                overview.compose_projects = previous.compose_projects.clone();
            }
        }
        self.apply_overview(overview);
    }

    pub(super) fn apply_docker_details(
        &mut self,
        container_id: String,
        details: DockerContainerDetails,
    ) {
        // Closing, switching sessions, or removing the container invalidates this result.
        if self.details_container_id.as_ref() != Some(&container_id) {
            return;
        }
        self.details = Some(details);
        self.touch();
    }

    pub(super) fn clear_compose_service_error(&mut self, key: &str) {
        Arc::make_mut(&mut self.compose_service_errors).remove(key);
        self.touch();
    }

    pub(super) fn set_compose_services(
        &mut self,
        key: String,
        services: Vec<DockerComposeService>,
    ) {
        Arc::make_mut(&mut self.compose_service_errors).remove(&key);
        Arc::make_mut(&mut self.compose_services).insert(key, services);
        self.touch();
    }

    pub(super) fn set_compose_service_error(&mut self, key: String, error: String) {
        Arc::make_mut(&mut self.compose_services).remove(&key);
        Arc::make_mut(&mut self.compose_service_errors).insert(key, error);
        self.touch();
    }
}
