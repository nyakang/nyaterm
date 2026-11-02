use serde::Serialize;

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct RemoteNpuOverview {
    pub available: bool,
    pub driver_version: String,
    pub cann_version: String,
    pub npus: Vec<RemoteNpu>,
    pub processes: Vec<RemoteNpuProcess>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RemoteNpu {
    /// NPU ID shown in the first row of `npu-smi info`.
    pub index: u32,
    /// Chip ID shown in the second row. Most cards expose chip 0.
    pub chip_id: u32,
    /// Optional physical chip ID used by newer drivers/products.
    pub physical_id: Option<u32>,
    /// Ascend does not expose a cross-version UUID equivalent to NVIDIA's GPU UUID.
    /// Use a deterministic logical key for UI/process association instead.
    pub device_key: String,
    pub name: String,
    pub health: String,
    pub bus_id: String,
    pub temperature_c: Option<f64>,
    pub utilization_aicore_percent: Option<f64>,
    pub utilization_memory_percent: Option<f64>,
    /// Preferred accelerator memory. HBM is used when present; otherwise
    /// the generic Memory-Usage pair is used.
    pub memory_total_mb: u64,
    pub memory_used_mb: u64,
    pub memory_free_mb: u64,
    pub memory_kind: String,
    /// Raw HBM values, when the product reports them separately.
    pub hbm_total_mb: Option<u64>,
    pub hbm_used_mb: Option<u64>,
    pub power_draw_w: Option<f64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RemoteNpuProcess {
    pub npu_index: u32,
    pub chip_id: u32,
    pub device_key: String,
    pub pid: u32,
    pub process_name: String,
    pub used_memory_mb: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NpuTableSection {
    None,
    Devices,
    Processes,
}

#[derive(Debug, Clone)]
struct PendingCard {
    index: u32,
    name: String,
    health: String,
    power_draw_w: Option<f64>,
    temperature_c: Option<f64>,
    emitted_chip: bool,
}

#[derive(Debug, Clone, Default)]
struct ChipMetrics {
    aicore_percent: Option<f64>,
    memory_used_mb: Option<u64>,
    memory_total_mb: Option<u64>,
    hbm_used_mb: Option<u64>,
    hbm_total_mb: Option<u64>,
}

pub fn parse_npu_overview_output(output: &str) -> RemoteNpuOverview {
    let mut overview = RemoteNpuOverview::default();
    let mut in_smi_output = false;
    let mut raw_smi_lines = Vec::new();

    for line in output.lines() {
        match line {
            "NPU_SMI_BEGIN" => {
                in_smi_output = true;
                continue;
            }
            "NPU_SMI_END" => {
                in_smi_output = false;
                continue;
            }
            _ => {}
        }

        if in_smi_output {
            raw_smi_lines.push(line.to_string());
            continue;
        }

        let cols: Vec<&str> = line.split('\t').collect();
        if cols.first() == Some(&"NPU_AVAILABLE") && cols.len() >= 2 {
            overview.available = cols[1].trim() == "1";
        } else if cols.first() == Some(&"NPU_CANN_VERSION") && cols.len() >= 2 {
            overview.cann_version = cols[1].trim().to_string();
        }
    }

    if !overview.available || raw_smi_lines.is_empty() {
        return overview;
    }

    parse_npu_smi_info(&raw_smi_lines.join("\n"), &mut overview);
    overview
}

fn parse_npu_smi_info(raw: &str, overview: &mut RemoteNpuOverview) {
    let mut section = NpuTableSection::None;
    let mut pending_card: Option<PendingCard> = None;
    let mut has_physical_id = false;

    for line in raw.lines() {
        if overview.driver_version.is_empty()
            && let Some(version) = parse_driver_version(line)
        {
            overview.driver_version = version;
        }

        let cells = parse_table_cells(line);
        if cells.is_empty() {
            continue;
        }

        if is_process_header(&cells) {
            flush_card_without_chip(&mut pending_card, &mut overview.npus);
            section = NpuTableSection::Processes;
            continue;
        }

        if is_device_header(&cells) {
            flush_card_without_chip(&mut pending_card, &mut overview.npus);
            section = NpuTableSection::Devices;
            has_physical_id = false;
            continue;
        }

        if section == NpuTableSection::Devices && cells[0].split_whitespace().next() == Some("Chip")
        {
            // Device is a logical device number on 310P; only Phy-ID denotes
            // a physical chip identifier on products that expose one.
            has_physical_id = cells[0]
                .split_whitespace()
                .any(|token| token.eq_ignore_ascii_case("phy-id"));
            continue;
        }

        match section {
            NpuTableSection::Devices => {
                if cells.len() < 3 || is_header_or_separator_row(&cells) {
                    continue;
                }

                if is_card_summary_row(&cells, pending_card.as_ref()) {
                    flush_card_without_chip(&mut pending_card, &mut overview.npus);
                    pending_card = parse_card_summary_row(&cells);
                    continue;
                }

                if let Some(card) = pending_card.as_mut()
                    && let Some(npu) = parse_chip_row(&cells, card, has_physical_id)
                {
                    card.emitted_chip = true;
                    overview.npus.push(npu);
                }
            }
            NpuTableSection::Processes => {
                if let Some(process) = parse_process_row(&cells) {
                    overview.processes.push(process);
                }
            }
            NpuTableSection::None => {}
        }
    }

    flush_card_without_chip(&mut pending_card, &mut overview.npus);
}

fn parse_driver_version(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    if !lower.contains("npu-smi") {
        return None;
    }

    let start = lower.find("version:")? + "version:".len();
    let value = line.get(start..)?.trim().trim_matches('|').trim();
    let version = value.split_whitespace().next()?.trim();
    (!version.is_empty()).then(|| version.to_string())
}

fn parse_table_cells(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    if trimmed.len() < 2 || !trimmed.starts_with('|') || !trimmed.ends_with('|') {
        return Vec::new();
    }

    // Empty cells still occupy columns; dropping them shifts Bus-Id and metrics.
    trimmed[1..trimmed.len() - 1]
        .split('|')
        .map(|value| value.trim().to_string())
        .collect()
}

fn is_device_header(cells: &[String]) -> bool {
    cells.len() >= 2
        && cells[0].to_ascii_lowercase().contains("npu")
        && cells[0].to_ascii_lowercase().contains("name")
        && cells[1].to_ascii_lowercase().contains("health")
}

fn is_process_header(cells: &[String]) -> bool {
    cells.get(1).is_some_and(|cell| {
        let normalized = cell
            .split_whitespace()
            .collect::<String>()
            .to_ascii_lowercase();
        normalized == "processid" || normalized == "pid"
    })
}

fn is_header_or_separator_row(cells: &[String]) -> bool {
    let joined = cells.join(" ").to_ascii_lowercase();
    joined.contains("bus-id")
        || joined.contains("aicore")
        || joined.contains("memory-usage")
        || joined.contains("no running processes")
}

fn is_card_summary_row(cells: &[String], pending: Option<&PendingCard>) -> bool {
    if cells.len() < 3 {
        return false;
    }

    let left: Vec<&str> = cells[0].split_whitespace().collect();
    if left
        .first()
        .and_then(|value| parse_u32_strict(value))
        .is_none()
    {
        return false;
    }

    let second_token_is_name = left
        .get(1)
        .is_some_and(|value| parse_u32_strict(value).is_none());
    if second_token_is_name && (looks_like_health(&cells[1]) || cells[1].is_empty()) {
        return true;
    }

    // Handles rare products whose displayed Name is numeric. A chip detail row
    // normally appears only after a card summary row and has a Bus-Id/NA in the
    // second column rather than a health value.
    looks_like_health(&cells[1]) && (pending.is_none() || !is_missing(&cells[1]))
}

fn parse_card_summary_row(cells: &[String]) -> Option<PendingCard> {
    let mut left = cells[0].split_whitespace();
    let index = parse_u32_strict(left.next()?)?;
    let name = left.collect::<Vec<_>>().join(" ");
    let metrics = split_metric_tokens(&cells[2]);

    Some(PendingCard {
        index,
        name,
        health: clean_text(&cells[1]),
        power_draw_w: metrics.first().and_then(|value| parse_optional_f64(value)),
        temperature_c: metrics.get(1).and_then(|value| parse_optional_f64(value)),
        emitted_chip: false,
    })
}

fn parse_chip_row(
    cells: &[String],
    card: &PendingCard,
    has_physical_id: bool,
) -> Option<RemoteNpu> {
    if cells.len() < 3 {
        return None;
    }

    let left: Vec<&str> = cells[0].split_whitespace().collect();
    let chip_id = left.first().and_then(|value| parse_u32_strict(value))?;
    let physical_id = has_physical_id
        .then(|| left.get(1).and_then(|value| parse_u32_strict(value)))
        .flatten();
    let metrics = parse_chip_metrics(&cells[2]);

    let (memory_used_mb, memory_total_mb, memory_kind) = if metrics.hbm_total_mb.unwrap_or(0) > 0 {
        (
            metrics.hbm_used_mb,
            metrics.hbm_total_mb.unwrap_or(0),
            "hbm",
        )
    } else {
        (
            metrics.memory_used_mb,
            metrics.memory_total_mb.unwrap_or(0),
            "memory",
        )
    };

    let utilization_memory_percent = memory_used_mb
        .filter(|_| memory_total_mb > 0)
        .map(|used| (used as f64 / memory_total_mb as f64) * 100.0);
    // The existing serialized contract uses integers for memory sizes. Keep
    // unknown usage at zero there, while preserving None for its utilization.
    let memory_used_mb = memory_used_mb.unwrap_or(0);

    Some(RemoteNpu {
        index: card.index,
        chip_id,
        physical_id,
        device_key: device_key(card.index, chip_id),
        name: card.name.clone(),
        health: card.health.clone(),
        bus_id: clean_text(&cells[1]),
        temperature_c: card.temperature_c,
        utilization_aicore_percent: metrics.aicore_percent,
        utilization_memory_percent,
        memory_total_mb,
        memory_used_mb,
        memory_free_mb: memory_total_mb.saturating_sub(memory_used_mb),
        memory_kind: memory_kind.to_string(),
        hbm_total_mb: metrics.hbm_total_mb,
        hbm_used_mb: metrics.hbm_used_mb,
        power_draw_w: card.power_draw_w,
    })
}

fn parse_chip_metrics(value: &str) -> ChipMetrics {
    let tokens = split_metric_tokens(value);
    if tokens.is_empty() {
        return ChipMetrics::default();
    }

    let aicore_percent = parse_optional_f64(&tokens[0]);
    let pairs = parse_usage_pairs(&tokens[1..]);

    ChipMetrics {
        aicore_percent,
        memory_used_mb: pairs.first().and_then(|pair| pair.0),
        memory_total_mb: pairs.first().and_then(|pair| pair.1),
        hbm_used_mb: pairs.get(1).and_then(|pair| pair.0),
        hbm_total_mb: pairs.get(1).and_then(|pair| pair.1),
    }
}

fn split_metric_tokens(value: &str) -> Vec<String> {
    value
        .replace('/', " / ")
        .split_whitespace()
        .map(|value| value.trim().to_string())
        .collect()
}

fn parse_usage_pairs(tokens: &[String]) -> Vec<(Option<u64>, Option<u64>)> {
    let mut pairs = Vec::new();
    let mut index = 0;

    while index + 2 < tokens.len() {
        if tokens[index + 1] == "/" {
            pairs.push((
                parse_optional_u64(&tokens[index]),
                parse_optional_u64(&tokens[index + 2]),
            ));
            index += 3;
        } else {
            index += 1;
        }
    }

    pairs
}

fn parse_process_row(cells: &[String]) -> Option<RemoteNpuProcess> {
    if cells.len() < 4 {
        return None;
    }

    let ids: Vec<&str> = cells[0].split_whitespace().collect();
    let npu_index = ids.first().and_then(|value| parse_u32_strict(value))?;
    let chip_id = parse_u32_strict(ids.get(1)?)?;
    let pid = parse_u32_strict(&cells[1])?;

    Some(RemoteNpuProcess {
        npu_index,
        chip_id,
        device_key: device_key(npu_index, chip_id),
        pid,
        process_name: clean_text(&cells[2]),
        used_memory_mb: parse_u64(&cells[3]).unwrap_or(0),
    })
}

fn flush_card_without_chip(pending: &mut Option<PendingCard>, npus: &mut Vec<RemoteNpu>) {
    let Some(card) = pending.take() else {
        return;
    };

    if card.emitted_chip {
        return;
    }

    npus.push(RemoteNpu {
        index: card.index,
        chip_id: 0,
        physical_id: None,
        device_key: device_key(card.index, 0),
        name: card.name,
        health: card.health,
        bus_id: String::new(),
        temperature_c: card.temperature_c,
        utilization_aicore_percent: None,
        utilization_memory_percent: None,
        memory_total_mb: 0,
        memory_used_mb: 0,
        memory_free_mb: 0,
        memory_kind: "memory".to_string(),
        hbm_total_mb: None,
        hbm_used_mb: None,
        power_draw_w: card.power_draw_w,
    });
}

fn device_key(npu_index: u32, chip_id: u32) -> String {
    format!("{npu_index}:{chip_id}")
}

fn looks_like_health(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "ok" | "warning"
            | "warn"
            | "alarm"
            | "critical"
            | "fault"
            | "error"
            | "degraded"
            | "unknown"
            | "na"
            | "n/a"
    )
}

fn clean_text(value: &str) -> String {
    value.trim().to_string()
}

fn is_missing(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "" | "-" | "n/a" | "na" | "[n/a]" | "[not supported]" | "not supported"
    )
}

fn parse_optional_f64(value: &str) -> Option<f64> {
    if is_missing(value) {
        return None;
    }
    value
        .trim()
        .trim_end_matches('%')
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite())
}

fn parse_optional_u64(value: &str) -> Option<u64> {
    if is_missing(value) {
        return None;
    }
    parse_u64(value)
}

fn parse_u64(value: &str) -> Option<u64> {
    value.trim().parse().ok()
}

fn parse_u32_strict(value: &str) -> Option<u32> {
    let trimmed = value.trim();
    if trimmed.is_empty() || !trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    trimmed.parse().ok()
}

#[cfg(test)]
mod tests {
    use crate::ascend_npu::parse_npu_overview_output;

    const TWO_310P3_DEVICES: &str = "NPU_AVAILABLE\t1\nNPU_CANN_VERSION\t8.5.0\nNPU_SMI_BEGIN\n\
| npu-smi 25.5.0                    Version: 25.5.0                               |\n\
+----------------------------+---------------+-----------------------------------+\n\
| NPU   Name                 | Health        | Power(W) Temp(C) Hugepages-Usage(page) |\n\
| Chip  Device               | Bus-Id        | AICore(%) Memory-Usage(MB)            |\n\
+============================+===============+===================================+\n\
| 1     310P3                | OK            | NA       50      528 / 528           |\n\
| 0     0                    | 0000:01:00.0  | 0        2932 / 21525                 |\n\
+============================+===============+===================================+\n\
| 2     310P3                | OK            | NA       49      0 / 0               |\n\
| 0     1                    | 0000:02:00.0  | 0        1851 / 21525                 |\n\
+============================+===============+===================================+\n\
| NPU   Chip                 | Process id    | Process name | Process memory(MB)     |\n\
| 1     0                    | 12345         | worker       | 1137                   |\n\
| No running processes found in NPU 2                                             |\n\
NPU_SMI_END\n";

    fn parse_device_rows(rows: &str) -> crate::ascend_npu::RemoteNpuOverview {
        parse_npu_overview_output(&format!(
            "NPU_AVAILABLE\t1\nNPU_SMI_BEGIN\n\
             | NPU Name | Health | Power(W) Temp(C) Hugepages-Usage(page) |\n\
             | Chip Device | Bus-Id | AICore(%) Memory-Usage(MB) HBM-Usage(MB) |\n\
             {rows}\nNPU_SMI_END\n"
        ))
    }

    #[test]
    fn parses_two_310p3_devices_without_treating_processes_as_devices() {
        let overview = parse_npu_overview_output(TWO_310P3_DEVICES);

        assert!(overview.available);
        assert_eq!(overview.driver_version, "25.5.0");
        assert_eq!(overview.cann_version, "8.5.0");
        assert_eq!(overview.npus.len(), 2);
        for (npu, index, used, temperature, bus_id) in [
            (&overview.npus[0], 1, 2932, 50., "0000:01:00.0"),
            (&overview.npus[1], 2, 1851, 49., "0000:02:00.0"),
        ] {
            assert_eq!(npu.index, index);
            assert_eq!(npu.chip_id, 0);
            assert_eq!(npu.physical_id, None);
            assert_eq!(npu.device_key, format!("{index}:0"));
            assert_eq!(npu.name, "310P3");
            assert_eq!(npu.health, "OK");
            assert_eq!(npu.bus_id, bus_id);
            assert_eq!(npu.temperature_c, Some(temperature));
            assert_eq!(npu.power_draw_w, None);
            assert_eq!(npu.utilization_aicore_percent, Some(0.));
            assert_eq!(npu.memory_kind, "memory");
            assert_eq!(npu.memory_used_mb, used);
            assert_eq!(npu.memory_total_mb, 21525);
            assert_eq!(npu.memory_free_mb, 21525 - used);
            assert_eq!(npu.hbm_total_mb, None);
            assert_eq!(npu.hbm_used_mb, None);
            let expected_percent = used as f64 / 21525. * 100.;
            assert!((npu.utilization_memory_percent.unwrap() - expected_percent).abs() < 1e-9);
        }
        assert_eq!(
            overview
                .npus
                .iter()
                .map(|npu| npu.memory_used_mb)
                .sum::<u64>(),
            4783
        );
        assert_eq!(
            overview
                .npus
                .iter()
                .map(|npu| npu.memory_total_mb)
                .sum::<u64>(),
            43050
        );
        assert_eq!(overview.processes.len(), 1);
        let process = &overview.processes[0];
        assert_eq!(
            (process.npu_index, process.chip_id, process.pid),
            (1, 0, 12345)
        );
        assert_eq!(process.device_key, overview.npus[0].device_key);
        assert_eq!(process.process_name, "worker");
        assert_eq!(process.used_memory_mb, 1137);
    }

    #[test]
    fn falls_back_to_generic_memory_when_reported_hbm_capacity_is_zero() {
        let overview =
            parse_device_rows("| 0 910B | OK | 0 0 100 / 100 |\n| 0 4 | NA | 0 100 / 1000 0 / 0 |");
        let npu = &overview.npus[0];
        assert_eq!(npu.memory_kind, "memory");
        assert_eq!(
            (npu.memory_used_mb, npu.memory_total_mb, npu.memory_free_mb),
            (100, 1000, 900)
        );
        assert_eq!(npu.utilization_memory_percent, Some(10.));
        assert_eq!((npu.hbm_used_mb, npu.hbm_total_mb), (Some(0), Some(0)));
        assert_eq!(npu.physical_id, None);
        assert_eq!(npu.power_draw_w, Some(0.));
        assert_eq!(npu.temperature_c, Some(0.));
    }

    #[test]
    fn preserves_empty_cells_and_unknown_metrics() {
        let overview =
            parse_device_rows("| 0 310P3 | | NA NA 99 / 100 |\n| 0 7 | | NA NA / NA NA / NA |");
        let npu = &overview.npus[0];
        assert!(npu.health.is_empty());
        assert!(npu.bus_id.is_empty());
        assert_eq!(npu.physical_id, None);
        assert_eq!(npu.temperature_c, None);
        assert_eq!(npu.power_draw_w, None);
        assert_eq!(npu.utilization_aicore_percent, None);
        assert_eq!(npu.utilization_memory_percent, None);
        assert_eq!((npu.memory_used_mb, npu.memory_total_mb), (0, 0));
        assert_eq!((npu.hbm_used_mb, npu.hbm_total_mb), (None, None));
    }

    #[test]
    fn retains_device_identity_when_metric_cells_are_empty() {
        let overview = parse_device_rows("| 0 310P3 | OK | |\n| 0 7 | | |");
        assert_eq!(overview.npus.len(), 1);
        let npu = &overview.npus[0];
        assert_eq!(npu.device_key, "0:0");
        assert_eq!(npu.health, "OK");
        assert!(npu.bus_id.is_empty());
        assert_eq!(npu.temperature_c, None);
        assert_eq!(npu.power_draw_w, None);
        assert_eq!(npu.utilization_aicore_percent, None);
        assert_eq!(npu.utilization_memory_percent, None);
    }

    #[test]
    fn process_names_do_not_act_as_table_headers_or_status_messages() {
        let overview = parse_device_rows(
            "| 0 310P3 | OK | 1 42 0 / 0 |\n\
             | 0 0 | NA | 0 10 / 100 |\n\
             | NPU Chip | Process id | Process name | Process memory(MB) |\n\
             | 0 0 | 10 | pid | 10 |\n\
             | 0 0 | 11 | process id worker | 20 |",
        );
        assert_eq!(overview.npus.len(), 1);
        assert_eq!(overview.processes.len(), 2);
        assert_eq!(overview.processes[0].process_name, "pid");
        assert_eq!(overview.processes[1].process_name, "process id worker");
    }

    #[test]
    fn unknown_memory_usage_does_not_become_zero_percent() {
        let overview =
            parse_device_rows("| 0 310P3 | OK | NA 42 0 / 0 |\n| 0 0 | NA | NA NA / 1000 |");
        assert_eq!(overview.npus[0].memory_total_mb, 1000);
        assert_eq!(overview.npus[0].utilization_memory_percent, None);
    }

    #[test]
    fn ignores_invalid_identifiers_and_non_data_rows() {
        let overview = parse_device_rows(
            "| invalid 310P3 | OK | 1 42 0 / 0 |\n\
             | 0 310P3 | OK | 1 42 0 / 0 |\n\
             | bad 0 | 0000:01:00.0 | 0 10 / 100 |\n\
             | 0 0 | 0000:01:00.0 | 0 10 / 100 |\n\
             |\n\
             | NPU Chip | Process id | Process name | Process memory(MB) |\n\
             | No running processes found in NPU 1 |\n\
             | 0x 0 | 12 | worker | 10 |\n\
             | 0 1x | 12 | worker | 10 |\n\
             | 0 0 | 12x | worker | 10 |\n\
             | 0 0 | 12.5 | worker | 10 |\n\
             | 0 0 | -12 | worker | 10 |\n\
             | 0 0 | 4294967296 | worker | 10 |\n\
             | 0 0 | 12 | worker | 10 |",
        );
        assert_eq!(overview.npus.len(), 1);
        assert_eq!(overview.npus[0].memory_used_mb, 10);
        assert_eq!(overview.processes.len(), 1);
        assert_eq!(overview.processes[0].pid, 12);
    }

    #[test]
    fn preserves_serialized_device_and_process_fields() {
        let overview = parse_device_rows(
            "| 0 310P3 | OK | NA 42 0 / 0 |\n\
             | 0 3 | 0000:01:00.0 | 0 100 / 1000 |\n\
             | NPU Chip | Process id | Process name | Process memory(MB) |\n\
             | 0 0 | 12 | worker | 10 |",
        );
        let serialized = serde_json::to_value(&overview).unwrap();
        assert_eq!(
            serialized,
            serde_json::json!({
                "available": true,
                "driver_version": "",
                "cann_version": "",
                "npus": [{
                    "index": 0, "chip_id": 0, "physical_id": null,
                    "device_key": "0:0", "name": "310P3", "health": "OK",
                    "bus_id": "0000:01:00.0", "temperature_c": 42.0,
                    "utilization_aicore_percent": 0.0, "utilization_memory_percent": 10.0,
                    "memory_total_mb": 1000, "memory_used_mb": 100, "memory_free_mb": 900,
                    "memory_kind": "memory", "hbm_total_mb": null, "hbm_used_mb": null,
                    "power_draw_w": null
                }],
                "processes": [{
                    "npu_index": 0, "chip_id": 0, "device_key": "0:0", "pid": 12,
                    "process_name": "worker", "used_memory_mb": 10
                }]
            })
        );
    }

    #[test]
    fn parses_unavailable_npu_output() {
        let overview = parse_npu_overview_output("NPU_AVAILABLE\t0\n");

        assert!(!overview.available);
        assert!(overview.npus.is_empty());
        assert!(overview.processes.is_empty());
    }

    #[test]
    fn parses_910a_output_and_prefers_hbm() {
        let raw = r#"NPU_AVAILABLE	1
NPU_CANN_VERSION	8.0.RC1
NPU_SMI_BEGIN
+-------------------------------------------------------------------------------------------+
| npu-smi 23.0.rc1.b050            Version: 23.0.rc1.b050                                   |
+----------------------+---------------+----------------------------------------------------+
| NPU   Name           | Health        | Power(W)    Temp(C)           Hugepages-Usage(page)|
| Chip                 | Bus-Id        | AICore(%)   Memory-Usage(MB)  HBM-Usage(MB)        |
+======================+===============+====================================================+
| 0     910A           | OK            | 67.8        45                0    / 0             |
| 0                    | 0000:61:00.0  | 12          2006 / 15039      1024 / 32768         |
+======================+===============+====================================================+
| NPU     Chip         | Process id    | Process name             | Process memory(MB)      |
+======================+===============+==========================+=========================+
| 0       0            | 4242          | python3                  | 1024                    |
+======================+===============+==========================+=========================+
NPU_SMI_END
"#;

        let overview = parse_npu_overview_output(raw);

        assert!(overview.available);
        assert_eq!(overview.driver_version, "23.0.rc1.b050");
        assert_eq!(overview.cann_version, "8.0.RC1");
        assert_eq!(overview.npus.len(), 1);
        assert_eq!(overview.npus[0].index, 0);
        assert_eq!(overview.npus[0].chip_id, 0);
        assert_eq!(overview.npus[0].name, "910A");
        assert_eq!(overview.npus[0].power_draw_w, Some(67.8));
        assert_eq!(overview.npus[0].temperature_c, Some(45.0));
        assert_eq!(overview.npus[0].utilization_aicore_percent, Some(12.0));
        assert_eq!(overview.npus[0].memory_kind, "hbm");
        assert_eq!(overview.npus[0].memory_used_mb, 1024);
        assert_eq!(overview.npus[0].memory_total_mb, 32768);
        assert_eq!(overview.processes.len(), 1);
        assert_eq!(overview.processes[0].device_key, "0:0");
        assert_eq!(overview.processes[0].pid, 4242);
    }

    #[test]
    fn parses_newer_phy_id_layout() {
        let raw = r#"NPU_AVAILABLE	1
NPU_CANN_VERSION	8.2.RC1
NPU_SMI_BEGIN
+------------------------------------------------------------------------------------------------+
| npu-smi 25.2.0                   Version: 25.2.0                                               |
+---------------------------+---------------+----------------------------------------------------+
| NPU   Name                | Health        | Power(W)   Temp(C)          Hugepages-Usage(page)   |
| Chip  Phy-ID              | Bus-Id        | AICore(%)  Memory-Usage(MB) HBM-Usage(MB)          |
+===========================+===============+====================================================+
| 0     Ascend910           | OK            | 157.1      43               0 / 0                   |
| 0     3                   | 0000:81:00.0  | 88         0 / 0            62000 / 65536           |
+===========================+===============+====================================================+
| NPU   Chip                | Process id    | Process name             | Process memory(MB)      |
+===========================+===============+==========================+=========================+
| No running processes found in NPU 0                                                            |
+===========================+===============+==========================+=========================+
NPU_SMI_END
"#;

        let overview = parse_npu_overview_output(raw);

        assert_eq!(overview.driver_version, "25.2.0");
        assert_eq!(overview.npus.len(), 1);
        assert_eq!(overview.npus[0].physical_id, Some(3));
        assert_eq!(overview.npus[0].bus_id, "0000:81:00.0");
        assert_eq!(overview.npus[0].memory_used_mb, 62000);
        assert_eq!(overview.npus[0].memory_total_mb, 65536);
        assert!(overview.processes.is_empty());
    }

    #[test]
    fn falls_back_to_generic_memory_when_hbm_is_zero_or_absent() {
        let raw = r#"NPU_AVAILABLE	1
NPU_CANN_VERSION	
NPU_SMI_BEGIN
| npu-smi 24.1.rc2.b010                    Version: 24.1.rc2.b010                                 |
| NPU   Name               | Health       | Power(W)   Temp(C)         Hugepages-Usage(page)      |
| Chip  Device             | Bus-Id       | AICore(%)  Memory-Usage(MB)                            |
| 0     310P3              | OK           | 9.6        56              15 / 15                    |
| 0     0                  | NA           | NA         3398 / 11578                               |
NPU_SMI_END
"#;

        let overview = parse_npu_overview_output(raw);

        assert_eq!(overview.npus.len(), 1);
        assert_eq!(overview.npus[0].memory_kind, "memory");
        assert_eq!(overview.npus[0].memory_used_mb, 3398);
        assert_eq!(overview.npus[0].memory_total_mb, 11578);
        assert_eq!(overview.npus[0].utilization_aicore_percent, None);
    }

    #[test]
    fn supports_multiple_chips_under_one_npu_card() {
        let raw = r#"NPU_AVAILABLE	1
NPU_CANN_VERSION	8.1.RC1
NPU_SMI_BEGIN
| npu-smi 24.1.0 Version: 24.1.0 |
| NPU Name | Health | Power(W) Temp(C) Hugepages-Usage(page) |
| Chip Phy-ID | Bus-Id | AICore(%) Memory-Usage(MB) HBM-Usage(MB) |
| 0 Ascend910 | OK | 100 40 0 / 0 |
| 0 4 | 0000:01:00.0 | 10 100 / 1000 200 / 2000 |
| 1 5 | 0000:02:00.0 | 20 200 / 1000 400 / 2000 |
| NPU Chip | Process id | Process name | Process memory(MB) |
| 0 1 | 99 | worker | 400 |
NPU_SMI_END
"#;

        let overview = parse_npu_overview_output(raw);

        assert_eq!(overview.npus.len(), 2);
        assert_eq!(overview.npus[0].chip_id, 0);
        assert_eq!(overview.npus[1].chip_id, 1);
        assert_eq!(overview.processes[0].device_key, "0:1");
    }
}
