use crate::features::remote::job_state::{RemoteJobState, RemoteJobTicket};
use crate::features::remote::list_window::{ACCELERATOR_PROCESS_VIEWPORT_ROWS, max_list_offset};
use crate::features::runtime_jobs::{GpuJobResult, NpuJobResult};
use futures::channel::mpsc::UnboundedReceiver;
use nyaterm_remote_ops::ascend_npu::{RemoteNpuOverview, RemoteNpuProcess};
use nyaterm_remote_ops::gpu::{RemoteGpuOverview, RemoteGpuProcess};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

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

pub(super) struct AcceleratorPaneState<Data: AcceleratorProcessList, Event> {
    job: RemoteJobState<Event>,
    data: Option<Data>,
    /// Bumped by every mutation that changes what `*_presentation` returns.
    revision: u64,
    data_generation: u64,
    derived: Option<AcceleratorDerivedCache<Data::Process>>,
    status: String,
    search_draft: String,
    expanded_devices: HashSet<String>,
    process_list_offset: usize,
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

impl<Data: AcceleratorProcessList, Event> AcceleratorPaneState<Data, Event> {
    pub(super) fn new(status: &str) -> Self {
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

    pub(super) fn revision(&self) -> u64 {
        self.revision
    }

    pub(super) fn set_status(&mut self, status: impl Into<String>) {
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
    pub(super) fn derived(&self) -> Arc<[Data::Process]> {
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

    pub(super) fn is_pending(&self) -> bool {
        self.job.is_pending()
    }

    pub(super) fn last_refresh_at(&self) -> Option<Instant> {
        self.job.last_refresh_at()
    }

    fn consecutive_refresh_failures(&self) -> u8 {
        self.job.consecutive_refresh_failures()
    }

    pub(super) fn is_pending_for(&self, session_id: &str) -> bool {
        self.job.is_pending_for(session_id)
    }

    pub(super) fn unavailable_for(&self, session_id: &str) -> bool {
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

    pub(super) fn apply_search(&mut self, text: String, status: &str) {
        self.search_draft = text;
        self.process_list_offset = 0;
        self.reconcile();
        self.status = status.to_string();
    }

    pub(super) fn toggle_device_expanded(&mut self, key: String) {
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

    pub(super) fn set_process_offset(&mut self, offset: usize) -> bool {
        if self.process_list_offset == offset {
            return false;
        }
        self.process_list_offset = offset;
        self.touch();
        true
    }

    pub(super) fn begin_job(&mut self, session_id: String) -> RemoteJobTicket<Event> {
        self.touch();
        self.job.begin(session_id)
    }

    pub(super) fn mark_refresh_started(&mut self) {
        self.job.mark_refresh_started();
    }

    pub(super) fn take_event_receiver(&mut self) -> Option<UnboundedReceiver<Event>> {
        self.job.take_event_receiver()
    }

    pub(super) fn complete_event(&mut self, job_id: u64, session_id: &str) -> bool {
        self.touch();
        self.job.complete_if_matches(job_id, session_id)
    }

    pub(super) fn reset_refresh_failures(&mut self) {
        self.job.reset_refresh_failures();
        self.touch();
    }

    fn record_refresh_failure(&mut self) -> u8 {
        self.touch();
        self.job.record_refresh_failure(false)
    }

    pub(super) fn reset_for_session_switch(&mut self, status: &str) {
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

impl AcceleratorPaneState<RemoteGpuOverview, GpuJobResult> {
    pub(super) fn gpu_presentation(&self) -> GpuPresentationState {
        GpuPresentationState {
            data: self.data.clone(),
            status: self.status.clone(),
            search_draft: self.search_draft.clone(),
            expanded_devices: self.expanded_devices.clone(),
            process_list_offset: self.process_list_offset,
            pending: self.is_pending(),
            consecutive_refresh_failures: self.consecutive_refresh_failures(),
        }
    }

    pub(super) fn gpu_status(&self) -> &str {
        &self.status
    }

    pub(super) fn apply_gpu(&mut self, session_id: &str, overview: RemoteGpuOverview) {
        if overview.available {
            self.clear_unavailable(session_id);
        } else {
            self.mark_unavailable(session_id.to_string());
        }
        let active_devices = overview
            .gpus
            .iter()
            .map(|gpu| gpu_device_key(gpu.index, &gpu.uuid))
            .collect::<HashSet<_>>();
        self.retain_expanded_devices(&active_devices);
        self.apply_data(overview);
    }

    pub(super) fn record_gpu_refresh_failure(&mut self) -> u8 {
        let failures = self.record_refresh_failure();
        if failures >= 3 {
            self.clear_data();
        }
        failures
    }
}

impl AcceleratorPaneState<RemoteNpuOverview, NpuJobResult> {
    pub(super) fn npu_presentation(&self) -> NpuPresentationState {
        NpuPresentationState {
            data: self.data.clone(),
            status: self.status.clone(),
            search_draft: self.search_draft.clone(),
            expanded_devices: self.expanded_devices.clone(),
            process_list_offset: self.process_list_offset,
            pending: self.is_pending(),
            consecutive_refresh_failures: self.consecutive_refresh_failures(),
        }
    }

    pub(super) fn npu_status(&self) -> &str {
        &self.status
    }

    pub(super) fn apply_npu(&mut self, session_id: &str, overview: RemoteNpuOverview) {
        if overview.available {
            self.clear_unavailable(session_id);
        } else {
            self.mark_unavailable(session_id.to_string());
        }
        let active_devices = overview
            .npus
            .iter()
            .map(|npu| npu.device_key.clone())
            .collect::<HashSet<_>>();
        self.retain_expanded_devices(&active_devices);
        self.apply_data(overview);
    }

    pub(super) fn record_npu_refresh_failure(&mut self) -> u8 {
        let failures = self.record_refresh_failure();
        if failures >= 3 {
            self.clear_data();
        }
        failures
    }
}
