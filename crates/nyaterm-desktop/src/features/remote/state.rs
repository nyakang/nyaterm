//! Remote monitoring composition. Each pane owns its jobs, cache and presentation invariants.
//! The facade preserves callers while pane implementations cannot mutate sibling state.

use crate::features::remote::job_state::RemoteJobTicket;
use crate::features::runtime_jobs::{
    DockerJobResult, DockerResource, GpuJobResult, NpuJobResult, ProcessJobResult, StatsJobResult,
};
use crate::models::{DockerTab, RemoteProcessSortKey};
use futures::channel::mpsc::UnboundedReceiver;
use nyaterm_transport::{
    DockerComposeService, DockerContainerDetails, RemoteDockerOverview, RemoteGpuOverview,
    RemoteGpuProcess, RemoteNpuOverview, RemoteNpuProcess, RemoteProcess, RemoteStatsSampler,
};
use std::sync::Arc;
use std::time::Instant;

#[cfg(test)]
use nyaterm_transport::RemoteStats;

mod accelerator;
mod docker;
mod process;
mod stats;
#[cfg(test)]
mod tests;

use accelerator::AcceleratorPaneState;
#[cfg(test)]
use accelerator::AcceleratorProcessList;
pub(in crate::features) use accelerator::{GpuPresentationState, NpuPresentationState};
use docker::DockerPaneState;
pub(in crate::features) use docker::{DockerDerivedItems, DockerPresentationState};
use process::ProcessPaneState;
pub(in crate::features) use process::{ProcessApplyOutcome, ProcessPresentationState};
use stats::StatsPaneState;
pub(in crate::features) use stats::{StatsApplyOutcome, StatsPresentationState};

pub(in crate::features) struct RemoteOpsFeatureState {
    docker: DockerPaneState,
    process: ProcessPaneState,
    stats: StatsPaneState,
    gpu: AcceleratorPaneState<RemoteGpuOverview, GpuJobResult>,
    npu: AcceleratorPaneState<RemoteNpuOverview, NpuJobResult>,
}

/// Focus handles the Remote page needs at construction time.
pub(in crate::features) struct RemoteOpsFeatureFocus {}

impl RemoteOpsFeatureState {
    pub(in crate::features) fn new(_focus: RemoteOpsFeatureFocus) -> Self {
        Self {
            docker: DockerPaneState::new(),
            process: ProcessPaneState::new(),
            stats: StatsPaneState::new(),
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
        self.stats.stats_sampler()
    }

    pub(in crate::features) fn clear_stats_sample(&mut self, session_id: &str) {
        self.stats.clear_stats_sample(session_id)
    }

    pub(in crate::features) fn activate_stats_session(&mut self, session_id: &str) {
        self.stats.activate_session(session_id);
    }

    pub(in crate::features) fn icon_probe_needed(&self, session_id: &str) -> bool {
        self.stats.icon_probe_needed(session_id)
    }

    pub(in crate::features) fn icon_probe_due(&self, session_id: &str) -> bool {
        self.stats.icon_probe_due(session_id)
    }

    pub(in crate::features) fn begin_icon_probe(&mut self, session_id: &str, job_id: u64) {
        self.stats.begin_icon_probe(session_id, job_id);
    }

    pub(in crate::features) fn take_icon_probe_snapshot(
        &mut self,
        session_id: &str,
    ) -> Option<nyaterm_transport::RemoteStats> {
        self.stats.take_icon_probe_snapshot(session_id)
    }

    pub(in crate::features) fn docker_presentation(&self) -> DockerPresentationState {
        self.docker.docker_presentation()
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
        self.process.process_presentation()
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
        self.stats.stats_presentation()
    }

    pub(in crate::features) fn select_stats_network_interface(
        &mut self,
        session_id: &str,
        interface: Option<&str>,
    ) -> bool {
        self.stats.select_network_interface(session_id, interface)
    }

    pub(in crate::features) fn gpu_presentation(&self) -> GpuPresentationState {
        self.gpu.gpu_presentation()
    }

    pub(in crate::features) fn npu_presentation(&self) -> NpuPresentationState {
        self.npu.npu_presentation()
    }

    pub(in crate::features) fn docker_status(&self) -> &str {
        self.docker.docker_status()
    }

    pub(in crate::features) fn set_docker_status(&mut self, status: impl Into<String>) {
        self.docker.set_docker_status(status)
    }

    pub(in crate::features) fn process_status(&self) -> &str {
        self.process.process_status()
    }

    pub(in crate::features) fn set_process_status(&mut self, status: impl Into<String>) {
        self.process.set_status(status);
    }

    pub(in crate::features) fn stats_status(&self) -> &str {
        self.stats.stats_status()
    }

    pub(in crate::features) fn set_stats_status(&mut self, status: impl Into<String>) {
        self.stats.set_status(status);
    }

    pub(in crate::features) fn gpu_status(&self) -> &str {
        self.gpu.gpu_status()
    }

    pub(in crate::features) fn set_gpu_status(&mut self, status: impl Into<String>) {
        self.gpu.set_status(status);
    }

    pub(in crate::features) fn npu_status(&self) -> &str {
        self.npu.npu_status()
    }

    pub(in crate::features) fn set_npu_status(&mut self, status: impl Into<String>) {
        self.npu.set_status(status);
    }

    pub(in crate::features) fn loaded_process_count(&self) -> Option<usize> {
        self.process.loaded_process_count()
    }

    pub(in crate::features) fn docker_engine_version(&self) -> Option<String> {
        self.docker.docker_engine_version()
    }

    pub(in crate::features) fn docker_can_prune(&self) -> bool {
        self.docker.docker_can_prune()
    }

    pub(in crate::features) fn docker_header_menu_open(&self) -> bool {
        self.docker.docker_header_menu_open()
    }

    pub(in crate::features) fn docker_is_pending(&self) -> bool {
        self.docker.is_pending()
    }

    pub(in crate::features) fn process_is_pending(&self) -> bool {
        self.process.is_pending()
    }

    pub(in crate::features) fn stats_manual_refreshing(&self) -> bool {
        self.stats.stats_manual_refreshing()
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
        self.docker.docker_details_refresh()
    }

    pub(in crate::features) fn set_docker_tab(&mut self, tab: DockerTab) {
        self.docker.set_tab(tab);
    }

    pub(in crate::features) fn toggle_docker_tab_menu(&mut self) {
        self.docker.toggle_docker_tab_menu()
    }

    pub(in crate::features) fn toggle_docker_header_menu(&mut self) {
        self.docker.toggle_docker_header_menu()
    }

    pub(in crate::features) fn close_docker_menus(&mut self) {
        self.docker.close_docker_menus()
    }

    pub(in crate::features) fn docker_menus_open(&self) -> bool {
        self.docker.docker_menus_open()
    }

    pub(in crate::features) fn toggle_docker_container_menu(&mut self, id: String) {
        self.docker.toggle_docker_container_menu(id)
    }

    pub(in crate::features) fn close_docker_container_menu(&mut self) {
        self.docker.close_docker_container_menu()
    }

    pub(in crate::features) fn toggle_docker_compose_menu(&mut self, id: String) {
        self.docker.toggle_docker_compose_menu(id)
    }

    pub(in crate::features) fn close_docker_compose_menu(&mut self) {
        self.docker.close_docker_compose_menu()
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
        self.docker.toggle_compose_project(key, project_name)
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
        self.docker.mark_docker_resource_started(tab)
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
        self.docker.start_docker_container_action(status)
    }

    pub(in crate::features) fn start_docker_details(
        &mut self,
        container_id: String,
        status: String,
    ) {
        self.docker.start_docker_details(container_id, status)
    }

    #[cfg(test)]
    pub(in crate::features) fn apply_docker_overview(&mut self, overview: RemoteDockerOverview) {
        self.docker.apply_overview(overview);
    }

    pub(in crate::features) fn apply_docker_summary(&mut self, overview: RemoteDockerOverview) {
        self.docker.apply_docker_summary(overview)
    }

    pub(in crate::features) fn apply_docker_resource(&mut self, resource: DockerResource) {
        self.docker.apply_resource(resource);
    }

    pub(in crate::features) fn apply_docker_details(
        &mut self,
        container_id: String,
        details: DockerContainerDetails,
    ) {
        self.docker.apply_docker_details(container_id, details)
    }

    pub(in crate::features) fn clear_compose_service_error(&mut self, key: &str) {
        self.docker.clear_compose_service_error(key)
    }

    pub(in crate::features) fn set_compose_services(
        &mut self,
        key: String,
        services: Vec<DockerComposeService>,
    ) {
        self.docker.set_compose_services(key, services)
    }

    pub(in crate::features) fn set_compose_service_error(&mut self, key: String, error: String) {
        self.docker.set_compose_service_error(key, error)
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
        self.process.apply_process_event(event, active_session_id)
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

    pub(in crate::features) fn stats_users_revision(&self) -> u64 {
        self.stats.users_revision()
    }

    pub(in crate::features) fn invalidate_stats_users(&mut self, enabled: bool) {
        self.stats.invalidate_users(enabled);
    }

    pub(in crate::features) fn stats_users_refresh_pending(&self) -> bool {
        self.stats.users_refresh_pending()
    }

    pub(in crate::features) fn take_stats_users_refresh_due(&mut self) -> bool {
        self.stats.take_users_refresh_due()
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
        self.stats.record_stats_refresh_failure()
    }

    pub(in crate::features) fn apply_stats_event(
        &mut self,
        event: StatsJobResult,
        active_session_id: Option<&str>,
    ) -> StatsApplyOutcome {
        self.stats.apply_stats_event(event, active_session_id)
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
        self.gpu.apply_gpu(session_id, overview)
    }

    pub(in crate::features) fn record_gpu_refresh_failure(&mut self) -> u8 {
        self.gpu.record_gpu_refresh_failure()
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
        self.npu.apply_npu(session_id, overview)
    }

    pub(in crate::features) fn record_npu_refresh_failure(&mut self) -> u8 {
        self.npu.record_npu_refresh_failure()
    }
}
