use crate::ascend_npu::{RemoteNpuOverview, RemoteNpuProcess};
use crate::gpu::{RemoteGpuOverview, RemoteGpuProcess};

/// How a GPU/NPU overview exposes its process list for filtering and sorting.
///
/// GPU and NPU records have different searchable fields and tie-breaking keys.
/// Callers normalize the query and retain ownership of any derived cache.
pub trait AcceleratorProcessList {
    type Process: Clone;

    fn processes(&self) -> &[Self::Process];
    fn process_matches(process: &Self::Process, normalized_query: &str) -> bool;
    fn sort_processes(processes: &mut [Self::Process]);
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
