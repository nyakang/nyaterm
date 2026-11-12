//! Display formatting and search-text extraction for asset rows.
//!
//! Mirrors `src/components/app/start-workspace/assetFormatters.ts`. The
//! numeric formatting (`format_bytes`) reproduces the JS unit selection and
//! precision so table cells match the Tauri edition byte-for-byte.

use crate::models::{
    AssetAccelerator, AssetAcceleratorType, AssetDisk, AssetMetadata, ConnectionType,
    SavedConnection,
};
use crate::natural_order::natural_compare;
use std::cmp::Ordering;

pub const NOT_APPLICABLE: &str = "-";

/// Localized labels the formatters fall back to for missing or empty values.
///
/// Defaults match the Tauri `DEFAULT_ASSET_DISPLAY_LABELS`; the desktop layer
/// overrides them with translated strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetDisplayLabels {
    pub none: String,
    pub not_applicable: String,
    pub local_machine: String,
}

impl Default for AssetDisplayLabels {
    fn default() -> Self {
        Self {
            none: "None".to_string(),
            not_applicable: NOT_APPLICABLE.to_string(),
            local_machine: "Local".to_string(),
        }
    }
}

/// Trims a string and collapses "nothing meaningful" to an empty string.
fn text(value: &str) -> String {
    value.trim().to_string()
}

/// Bytes → human string using the same 1024-based units and precision rule as
/// the Tauri `formatBytes`: no decimals for `>= 10` or for bytes, one otherwise.
pub fn format_bytes(bytes: Option<u64>, labels: &AssetDisplayLabels) -> String {
    let Some(bytes) = bytes else {
        return labels.not_applicable.clone();
    };
    if bytes == 0 {
        return "0 B".to_string();
    }

    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let bytes_f = bytes as f64;
    let unit_index = ((bytes_f.ln() / 1024_f64.ln()).floor() as usize).min(UNITS.len() - 1);
    let value = bytes_f / 1024_f64.powi(unit_index as i32);
    let precision = if value >= 10.0 || unit_index == 0 {
        0
    } else {
        1
    };
    format!("{value:.precision$} {}", UNITS[unit_index])
}

/// `"<cores>C / <threads>T"`, omitting whichever value is absent.
pub fn format_cpu_summary(asset: Option<&AssetMetadata>, labels: &AssetDisplayLabels) -> String {
    let Some(asset) = asset else {
        return labels.not_applicable.clone();
    };
    let mut parts = Vec::new();
    if let Some(cores) = asset.cpu_cores {
        parts.push(format!("{cores}C"));
    }
    if let Some(threads) = asset.cpu_threads {
        parts.push(format!("{threads}T"));
    }
    if parts.is_empty() {
        labels.not_applicable.clone()
    } else {
        parts.join(" / ")
    }
}

fn format_accelerator_item(item: &AssetAccelerator) -> String {
    let label = [item.vendor.as_deref(), item.model.as_deref()]
        .into_iter()
        .flatten()
        .map(text)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let memory = match item.memory_bytes {
        Some(bytes) => format_bytes(Some(bytes), &AssetDisplayLabels::default()),
        None => String::new(),
    };
    let count = match item.count {
        Some(count) if count > 1 => format!(" × {count}"),
        _ => String::new(),
    };
    let fallback = accelerator_type_upper(&item.r#type);
    let head = if label.is_empty() { fallback } else { label };
    let joined = [head, memory]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    format!("{joined}{count}")
}

fn accelerator_type_upper(kind: &AssetAcceleratorType) -> String {
    match kind {
        AssetAcceleratorType::Gpu => "GPU".to_string(),
        AssetAcceleratorType::Npu => "NPU".to_string(),
        AssetAcceleratorType::Other => "OTHER".to_string(),
    }
}

/// Comma-joined accelerator summary. `None` → not-applicable; empty vec →
/// the "none" label, distinguishing "unknown" from "confirmed none".
pub fn format_accelerators(
    accelerators: Option<&[AssetAccelerator]>,
    labels: &AssetDisplayLabels,
    max_items: Option<usize>,
) -> String {
    let Some(accelerators) = accelerators else {
        return labels.not_applicable.clone();
    };
    if accelerators.is_empty() {
        return labels.none.clone();
    }
    let max_items = max_items.unwrap_or(accelerators.len());
    let visible: Vec<String> = accelerators
        .iter()
        .take(max_items)
        .map(format_accelerator_item)
        .collect();
    let remaining = accelerators.len() - visible.len();
    if remaining > 0 {
        format!("{} +{remaining}", visible.join(", "))
    } else {
        visible.join(", ")
    }
}

/// Total disk capacity as a human string; `None`/empty follow the accelerator
/// rule.
pub fn format_disk_summary(disks: Option<&[AssetDisk]>, labels: &AssetDisplayLabels) -> String {
    let Some(disks) = disks else {
        return labels.not_applicable.clone();
    };
    if disks.is_empty() {
        return labels.none.clone();
    }
    let total = get_disk_total_bytes(Some(disks)).unwrap_or(0);
    if total > 0 {
        format_bytes(Some(total), labels)
    } else {
        labels.not_applicable.clone()
    }
}

/// Sum of `capacity_bytes * count` (count defaulting to 1). `None` when the
/// disks are unknown.
pub fn get_disk_total_bytes(disks: Option<&[AssetDisk]>) -> Option<u64> {
    let disks = disks?;
    let total = disks.iter().fold(0u64, |sum, disk| {
        let capacity = disk.capacity_bytes.unwrap_or(0);
        let count = match disk.count {
            Some(count) if count > 0 => count as u64,
            _ => 1,
        };
        sum + capacity.saturating_mul(count)
    });
    Some(total)
}

/// `updated_at` as-is when it is not a parseable date. The GPUI edition does
/// not reformat parseable dates (locale rendering happens in the view), so this
/// simply returns the trimmed value or the not-applicable label.
pub fn format_asset_updated_at(updated_at: Option<&str>, labels: &AssetDisplayLabels) -> String {
    match updated_at.map(text) {
        Some(raw) if !raw.is_empty() => raw,
        _ => labels.not_applicable.clone(),
    }
}

/// Last-connection time, resolved from the millisecond epoch value stored on a
/// connection. The view renders the actual date; this only surfaces presence.
pub fn format_asset_connection_time(value: Option<u64>, labels: &AssetDisplayLabels) -> String {
    match get_asset_connection_time_ms(value) {
        Some(ms) => ms.to_string(),
        None => labels.not_applicable.clone(),
    }
}

/// Normalizes a stored last-used timestamp to a valid non-negative epoch ms.
pub fn get_asset_connection_time_ms(value: Option<u64>) -> Option<u64> {
    value
}

/// Address column: local shells show the local label, serial shows the port,
/// everything else shows the host.
pub fn format_asset_address(connection: &SavedConnection, labels: &AssetDisplayLabels) -> String {
    match &connection.config {
        ConnectionType::Adb { device, .. } => device.serial(),
        ConnectionType::LocalTerminal { .. } => labels.local_machine.clone(),
        ConnectionType::Serial { port_name, .. } => {
            let port = text(port_name);
            if port.is_empty() {
                labels.not_applicable.clone()
            } else {
                port
            }
        }
        ConnectionType::Ssh { host, .. }
        | ConnectionType::Telnet { host, .. }
        | ConnectionType::Rdp { host, .. }
        | ConnectionType::Vnc { host, .. } => {
            let host = text(host);
            if host.is_empty() {
                labels.not_applicable.clone()
            } else {
                host
            }
        }
    }
}

/// `"<os> <version> · <arch>"`, dropping empty segments.
pub fn format_asset_system(asset: Option<&AssetMetadata>, labels: &AssetDisplayLabels) -> String {
    let Some(asset) = asset else {
        return labels.not_applicable.clone();
    };
    let os = [asset.os_name.as_deref(), asset.os_version.as_deref()]
        .into_iter()
        .flatten()
        .map(text)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let arch = asset.architecture.as_deref().map(text).unwrap_or_default();
    let result = [os, arch]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    if result.is_empty() {
        labels.not_applicable.clone()
    } else {
        result
    }
}

fn os_name_lower(asset: Option<&AssetMetadata>) -> String {
    asset
        .and_then(|asset| asset.os_name.as_deref())
        .map(|os| text(os).to_lowercase())
        .unwrap_or_default()
}

/// Whether the OS name looks like a Linux distribution.
pub fn is_linux_asset(asset: Option<&AssetMetadata>) -> bool {
    const NEEDLES: [&str; 12] = [
        "linux",
        "ubuntu",
        "debian",
        "centos",
        "rocky",
        "almalinux",
        "fedora",
        "arch",
        "alpine",
        "openeuler",
        "openwrt",
        "suse",
    ];
    let os = os_name_lower(asset);
    NEEDLES.iter().any(|needle| os.contains(needle))
}

/// Whether the OS name mentions Windows.
pub fn is_windows_asset(asset: Option<&AssetMetadata>) -> bool {
    os_name_lower(asset).contains("windows")
}

fn has_accelerator(asset: Option<&AssetMetadata>, kind: AssetAcceleratorType) -> bool {
    asset
        .and_then(|asset| asset.accelerators.as_deref())
        .is_some_and(|accelerators| {
            accelerators
                .iter()
                .any(|accelerator| accelerator.r#type == kind)
        })
}

/// Whether any accelerator is a GPU.
pub fn has_gpu(asset: Option<&AssetMetadata>) -> bool {
    has_accelerator(asset, AssetAcceleratorType::Gpu)
}

/// Whether any accelerator is an NPU.
pub fn has_npu(asset: Option<&AssetMetadata>) -> bool {
    has_accelerator(asset, AssetAcceleratorType::Npu)
}

/// Lowercased haystack combining a connection's identifying fields, its asset
/// facts, and its group path so a single substring search covers everything.
pub fn build_asset_search_text(connection: &SavedConnection, group_path: &str) -> String {
    let asset = connection.asset.as_ref();
    let (host, username, port_name) = connection_address_fields(connection);

    let mut parts: Vec<String> = vec![connection.name.clone(), host, username, port_name];
    parts.push(connection.tags.join(" "));
    if let Some(asset) = asset {
        parts.push(asset.hostname.clone().unwrap_or_default());
        parts.push(asset.os_name.clone().unwrap_or_default());
        parts.push(asset.os_version.clone().unwrap_or_default());
        parts.push(asset.architecture.clone().unwrap_or_default());
        parts.push(asset.cpu_model.clone().unwrap_or_default());
        parts.push(asset.notes.clone().unwrap_or_default());
    }
    parts.push(group_path.to_string());

    if let Some(accelerators) = asset.and_then(|asset| asset.accelerators.as_ref()) {
        for accelerator in accelerators {
            parts.push(accelerator.vendor.clone().unwrap_or_default());
            parts.push(accelerator.model.clone().unwrap_or_default());
            parts.push(accelerator_type_upper(&accelerator.r#type).to_lowercase());
        }
    }
    if let Some(disks) = asset.and_then(|asset| asset.disks.as_ref()) {
        for disk in disks {
            parts.push(disk.model.clone().unwrap_or_default());
        }
    }

    parts
        .into_iter()
        .map(|part| text(&part))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The host/username/port fields relevant for search, per connection kind.
fn connection_address_fields(connection: &SavedConnection) -> (String, String, String) {
    match &connection.config {
        ConnectionType::Adb { device, .. } => (device.serial(), String::new(), String::new()),
        ConnectionType::Ssh { host, username, .. } => {
            (host.clone(), username.clone(), String::new())
        }
        ConnectionType::Telnet { host, username, .. } => {
            (host.clone(), username.clone(), String::new())
        }
        ConnectionType::Rdp { host, username, .. } => {
            (host.clone(), username.clone(), String::new())
        }
        ConnectionType::Vnc { host, .. } => (host.clone(), String::new(), String::new()),
        ConnectionType::Serial { port_name, .. } => {
            (String::new(), String::new(), port_name.clone())
        }
        ConnectionType::LocalTerminal { .. } => (String::new(), String::new(), String::new()),
    }
}

fn parse_ipv4(value: &str) -> Option<[u16; 4]> {
    let trimmed = value.trim();
    let mut octets = [0u16; 4];
    let mut count = 0;
    for part in trimmed.split('.') {
        if count >= 4 {
            return None;
        }
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let octet: u16 = part.parse().ok()?;
        if octet > 255 {
            return None;
        }
        octets[count] = octet;
        count += 1;
    }
    if count == 4 { Some(octets) } else { None }
}

/// Orders IPv4 addresses numerically, sorting them ahead of non-IP hosts, and
/// falls back to natural comparison otherwise.
pub fn compare_asset_address(left: &str, right: &str) -> Ordering {
    match (parse_ipv4(left), parse_ipv4(right)) {
        (Some(left_ip), Some(right_ip)) => left_ip.cmp(&right_ip),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => natural_compare(left, right),
    }
}
