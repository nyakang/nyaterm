//! Monitoring → asset-patch mapping and the per-session patch cache.
//!
//! Mirrors `src/lib/assetMonitoring.ts`. Snapshots arrive as the
//! transport-neutral structs from [`super::snapshot`]; the accumulated patch is
//! merged with the same per-type accelerator rule the store uses when it writes
//! back through `merge_connection_asset_from_monitoring`.

use std::collections::HashMap;

use crate::assets::snapshot::{AssetAcceleratorSnapshot, AssetStatsSnapshot};
use crate::models::{AssetAccelerator, AssetAcceleratorType, AssetDisk, AssetMetadata};

// Historical MB names here and in memory_total_mb represent MiB (1024² bytes).
const BYTES_PER_MB: u64 = 1024 * 1024;
const UNKNOWN_VALUES: [&str; 5] = ["", "-", "unknown", "n/a", "null"];

fn clean_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if UNKNOWN_VALUES.contains(&trimmed.to_lowercase().as_str()) {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn has_monitoring_asset_patch_value(patch: &AssetMetadata) -> bool {
    patch.hostname.is_some()
        || patch.os_name.is_some()
        || patch.architecture.is_some()
        || patch.cpu_model.is_some()
        || patch.cpu_cores.is_some()
        || patch.memory_bytes.is_some()
        || patch.disks.is_some()
        || patch.accelerators.is_some()
}

/// Builds an asset patch from a remote-stats snapshot, or `None` when the
/// snapshot carries nothing worth persisting.
pub fn build_asset_patch_from_stats_snapshot(stats: &AssetStatsSnapshot) -> Option<AssetMetadata> {
    let disks: Vec<AssetDisk> = stats
        .disks
        .iter()
        .filter_map(|disk| {
            let model = clean_text(&disk.device).or_else(|| clean_text(&disk.mount));
            let capacity_bytes = (disk.total_bytes > 0).then_some(disk.total_bytes);
            if model.is_none() && capacity_bytes.is_none() {
                return None;
            }
            Some(AssetDisk {
                kind: None,
                model,
                capacity_bytes,
                count: Some(1),
                purpose: None,
            })
        })
        .collect();

    let memory_bytes = (stats.memory_total_bytes > 0).then_some(stats.memory_total_bytes);

    let patch = AssetMetadata {
        hostname: clean_text(&stats.hostname),
        os_name: clean_text(&stats.os),
        architecture: clean_text(&stats.arch),
        cpu_model: clean_text(&stats.cpu_model),
        cpu_cores: (stats.cpu_cores > 0).then_some(stats.cpu_cores),
        memory_bytes,
        disks: Some(disks),
        ..AssetMetadata::default()
    };

    has_monitoring_asset_patch_value(&patch).then_some(patch)
}

/// Builds an accelerator-only patch of a single [`AssetAcceleratorType`] from a
/// device overview. Returns `None` when the overview is unavailable or empty.
pub fn build_asset_patch_from_accelerator_snapshot(
    kind: AssetAcceleratorType,
    vendor: &str,
    available: bool,
    devices: &[AssetAcceleratorSnapshot],
) -> Option<AssetMetadata> {
    if !available || devices.is_empty() {
        return None;
    }

    let accelerators = group_accelerators(
        devices
            .iter()
            .map(|device| AssetAccelerator {
                r#type: kind.clone(),
                vendor: clean_text(if device.vendor.is_empty() {
                    vendor
                } else {
                    &device.vendor
                }),
                model: clean_text(&device.model),
                count: Some(1),
                memory_bytes: (device.memory_total_mb > 0)
                    .then(|| device.memory_total_mb.saturating_mul(BYTES_PER_MB)),
            })
            .collect(),
    );

    if accelerators.is_empty() {
        None
    } else {
        Some(AssetMetadata {
            accelerators: Some(accelerators),
            ..AssetMetadata::default()
        })
    }
}

/// Collapses identical accelerator entries, summing their counts. Two entries
/// match on type, vendor, model, and memory, matching the Tauri grouping key.
fn group_accelerators(accelerators: Vec<AssetAccelerator>) -> Vec<AssetAccelerator> {
    let mut order: Vec<String> = Vec::new();
    let mut grouped: HashMap<String, AssetAccelerator> = HashMap::new();

    for accelerator in accelerators {
        let key = format!(
            "{}\u{0}{}\u{0}{}\u{0}{}",
            accelerator_type_key(&accelerator.r#type),
            accelerator.vendor.clone().unwrap_or_default(),
            accelerator.model.clone().unwrap_or_default(),
            accelerator
                .memory_bytes
                .map(|bytes| bytes.to_string())
                .unwrap_or_default(),
        );
        if let Some(existing) = grouped.get_mut(&key) {
            existing.count = Some(existing.count.unwrap_or(1) + accelerator.count.unwrap_or(1));
        } else {
            order.push(key.clone());
            grouped.insert(key, accelerator);
        }
    }

    order
        .into_iter()
        .filter_map(|key| grouped.remove(&key))
        .collect()
}

fn accelerator_type_key(kind: &AssetAcceleratorType) -> &'static str {
    match kind {
        AssetAcceleratorType::Gpu => "gpu",
        AssetAcceleratorType::Npu => "npu",
        AssetAcceleratorType::Other => "other",
    }
}

/// One session's accumulated monitoring patch. `session_id`/`connection_id`
/// guard the merge so a rebound session key never inherits another
/// connection's facts.
#[derive(Debug, Clone, PartialEq)]
pub struct AssetMonitoringCacheEntry {
    pub session_id: String,
    pub connection_id: String,
    pub last_asset_patch: AssetMetadata,
}

/// Session-keyed cache of accumulated monitoring patches.
#[derive(Debug, Default)]
pub struct AssetMonitoringCache {
    entries: HashMap<String, AssetMonitoringCacheEntry>,
}

/// Inputs for recording a monitoring patch against the cache.
pub struct RecordAssetMonitoringPatch<'a> {
    pub source_session_id: &'a str,
    pub target_session_id: &'a str,
    pub connection_id: &'a str,
    pub patch: Option<AssetMetadata>,
}

impl AssetMonitoringCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, session_id: &str) -> Option<&AssetMonitoringCacheEntry> {
        self.entries.get(session_id)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Drops the cache entry for a session, returning it if present. Used by the
    /// desktop layer to flush a session's accumulated patch on removal.
    pub fn take(&mut self, session_id: &str) -> Option<AssetMonitoringCacheEntry> {
        self.entries.remove(session_id)
    }

    /// Records a patch against the target session.
    ///
    /// Rejects the patch (returning `false`) when it is absent or its source
    /// session differs from the target — a snapshot only updates the session
    /// that produced it. A matching session/connection merges onto the prior
    /// patch; a rebound session starts fresh.
    pub fn record(&mut self, input: RecordAssetMonitoringPatch<'_>) -> bool {
        let Some(patch) = input.patch else {
            return false;
        };
        if input.source_session_id != input.target_session_id {
            return false;
        }

        let can_merge = self
            .entries
            .get(input.target_session_id)
            .is_some_and(|current| {
                current.session_id == input.target_session_id
                    && current.connection_id == input.connection_id
            });

        let mut base = if can_merge {
            self.entries
                .get(input.target_session_id)
                .map(|entry| entry.last_asset_patch.clone())
                .unwrap_or_default()
        } else {
            AssetMetadata::default()
        };
        base.merge_monitoring_patch(patch);

        self.entries.insert(
            input.target_session_id.to_string(),
            AssetMonitoringCacheEntry {
                session_id: input.target_session_id.to_string(),
                connection_id: input.connection_id.to_string(),
                last_asset_patch: base,
            },
        );
        true
    }
}
