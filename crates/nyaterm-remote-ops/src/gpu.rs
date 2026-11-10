use crate::host::HostTarget;
use nyaterm_transport::ssh_execution::{RemoteCommandOutput, ensure_remote_command_success};
use nyaterm_transport::{SshMultiplexHandle, SshSessionConfig};
use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;

const GPU_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct RemoteGpuOverview {
    pub available: bool,
    pub driver_version: String,
    pub cuda_version: String,
    pub gpus: Vec<RemoteGpu>,
    pub processes: Vec<RemoteGpuProcess>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RemoteGpu {
    pub index: u32,
    pub uuid: String,
    pub name: String,
    pub temperature_c: Option<f64>,
    pub utilization_gpu_percent: Option<f64>,
    pub utilization_memory_percent: Option<f64>,
    pub memory_total_mb: u64,
    pub memory_used_mb: u64,
    pub memory_free_mb: u64,
    pub power_draw_w: Option<f64>,
    pub power_limit_w: Option<f64>,
    pub fan_speed_percent: Option<f64>,
    pub pstate: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RemoteGpuProcess {
    pub gpu_uuid: String,
    pub gpu_index: Option<u32>,
    pub pid: u32,
    pub process_name: String,
    pub used_memory_mb: u64,
}

#[derive(Debug, Clone)]
pub struct RemoteGpuService {
    target: HostTarget,
}

pub const GPU_OVERVIEW_SCRIPT: &str = r#"sh -s <<'NYATERM_GPU_SCRIPT'
LC_ALL=C
export LC_ALL

if ! command -v nvidia-smi >/dev/null 2>&1; then
  printf "GPU_AVAILABLE\t0\n"
  exit 0
fi

gpu_query="index,uuid,name,driver_version"
gpu_query="$gpu_query,temperature.gpu,utilization.gpu,utilization.memory"
gpu_query="$gpu_query,memory.total,memory.used,memory.free"
gpu_query="$gpu_query,power.draw,power.limit,fan.speed,pstate"

cuda_version=$(nvidia-smi 2>/dev/null | sed -n 's/.*CUDA Version: *\([^ |]*\).*/\1/p' | head -n 1)
gpu_csv=$(nvidia-smi --query-gpu="$gpu_query" --format=csv,noheader,nounits 2>/dev/null)
status=$?

if [ "$status" -ne 0 ] || [ -z "$gpu_csv" ]; then
  printf "GPU_AVAILABLE\t0\n"
  exit 0
fi

printf "GPU_AVAILABLE\t1\n"
printf "GPU_CUDA_VERSION\t%s\n" "$cuda_version"
printf "GPU_CSV_BEGIN\n"
printf "%s\n" "$gpu_csv"
printf "GPU_CSV_END\n"

process_csv=$(nvidia-smi --query-compute-apps=gpu_uuid,pid,used_gpu_memory,process_name --format=csv,noheader,nounits 2>/dev/null || true)
printf "GPU_PROCESS_CSV_BEGIN\n"
if [ -n "$process_csv" ]; then
  printf "%s\n" "$process_csv"
fi
printf "GPU_PROCESS_CSV_END\n"
NYATERM_GPU_SCRIPT
"#;

impl RemoteGpuService {
    pub fn new(config: SshSessionConfig) -> Self {
        Self::with_target(HostTarget::Ssh {
            config,
            multiplex: None,
        })
    }

    pub fn with_target(target: HostTarget) -> Self {
        Self { target }
    }

    pub fn with_multiplex(
        config: SshSessionConfig,
        multiplex: SshMultiplexHandle,
    ) -> anyhow::Result<Self> {
        multiplex.ensure_matches_config(&config)?;
        Ok(Self::with_target(HostTarget::Ssh {
            config,
            multiplex: Some(multiplex),
        }))
    }

    pub fn overview(&self) -> anyhow::Result<RemoteGpuOverview> {
        let output = self.exec_success(GPU_OVERVIEW_SCRIPT, "Failed to fetch GPU overview")?;
        Ok(parse_gpu_overview_output(&output.stdout))
    }

    fn exec_success(&self, command: &str, context: &str) -> anyhow::Result<RemoteCommandOutput> {
        self.target.ensure_probes_allowed()?;
        let output = self
            .target
            .run_raw(command.to_string(), None, GPU_TIMEOUT)?;
        ensure_remote_command_success(output, context)
    }
}

enum GpuParseSection {
    None,
    Gpus,
    Processes,
}

pub fn parse_gpu_overview_output(output: &str) -> RemoteGpuOverview {
    let mut overview = RemoteGpuOverview::default();
    let mut section = GpuParseSection::None;
    let mut process_rows = Vec::new();

    for line in output.lines() {
        match line {
            "GPU_CSV_BEGIN" => {
                section = GpuParseSection::Gpus;
                continue;
            }
            "GPU_CSV_END" => {
                section = GpuParseSection::None;
                continue;
            }
            "GPU_PROCESS_CSV_BEGIN" => {
                section = GpuParseSection::Processes;
                continue;
            }
            "GPU_PROCESS_CSV_END" => {
                section = GpuParseSection::None;
                continue;
            }
            _ => {}
        }

        let cols: Vec<&str> = line.split('\t').collect();
        if cols.first() == Some(&"GPU_AVAILABLE") && cols.len() >= 2 {
            overview.available = cols[1] == "1";
            continue;
        }
        if cols.first() == Some(&"GPU_CUDA_VERSION") && cols.len() >= 2 {
            overview.cuda_version = clean_text(cols[1]);
            continue;
        }

        match section {
            GpuParseSection::Gpus => {
                if let Some((gpu, driver)) = parse_gpu_csv_line(line) {
                    if overview.driver_version.is_empty() {
                        overview.driver_version = driver;
                    }
                    overview.gpus.push(gpu);
                }
            }
            GpuParseSection::Processes => {
                if !line.trim().is_empty() {
                    process_rows.push(line.to_string());
                }
            }
            GpuParseSection::None => {}
        }
    }

    let gpu_indexes: HashMap<String, u32> = overview
        .gpus
        .iter()
        .map(|gpu| (gpu.uuid.clone(), gpu.index))
        .collect();
    overview.processes = process_rows
        .iter()
        .filter_map(|line| parse_gpu_process_csv_line(line, &gpu_indexes))
        .collect();

    overview
}

fn parse_gpu_csv_line(line: &str) -> Option<(RemoteGpu, String)> {
    let cols = parse_csv_line(line);
    if cols.len() < 14 {
        return None;
    }
    Some((
        RemoteGpu {
            index: parse_u32(&cols[0]).unwrap_or(0),
            uuid: clean_text(&cols[1]),
            name: clean_text(&cols[2]),
            temperature_c: parse_optional_f64(&cols[4]),
            utilization_gpu_percent: parse_optional_f64(&cols[5]),
            utilization_memory_percent: parse_optional_f64(&cols[6]),
            memory_total_mb: parse_u64(&cols[7]).unwrap_or(0),
            memory_used_mb: parse_u64(&cols[8]).unwrap_or(0),
            memory_free_mb: parse_u64(&cols[9]).unwrap_or(0),
            power_draw_w: parse_optional_f64(&cols[10]),
            power_limit_w: parse_optional_f64(&cols[11]),
            fan_speed_percent: parse_optional_f64(&cols[12]),
            pstate: clean_text(&cols[13]),
        },
        clean_text(&cols[3]),
    ))
}

fn parse_gpu_process_csv_line(
    line: &str,
    gpu_indexes: &HashMap<String, u32>,
) -> Option<RemoteGpuProcess> {
    let cols = parse_csv_line(line);
    if cols.len() < 4 {
        return None;
    }
    let gpu_uuid = clean_text(&cols[0]);
    Some(RemoteGpuProcess {
        gpu_index: gpu_indexes.get(&gpu_uuid).copied(),
        gpu_uuid,
        pid: parse_u32(&cols[1])?,
        used_memory_mb: parse_u64(&cols[2]).unwrap_or(0),
        process_name: clean_text(&cols[3]),
    })
}

fn parse_csv_line(line: &str) -> Vec<String> {
    line.split(',').map(clean_text).collect()
}

fn clean_text(value: &str) -> String {
    let trimmed = value.trim();
    if matches!(trimmed, "" | "[N/A]" | "N/A" | "Not Supported") {
        String::new()
    } else {
        trimmed.to_string()
    }
}

fn parse_u32(value: &str) -> Option<u32> {
    clean_text(value).parse().ok()
}

fn parse_u64(value: &str) -> Option<u64> {
    clean_text(value).parse().ok()
}

fn parse_optional_f64(value: &str) -> Option<f64> {
    clean_text(value).parse().ok()
}

#[cfg(test)]
mod tests {
    use super::parse_gpu_overview_output;

    #[test]
    fn parses_gpu_overview_with_processes() {
        let output = "GPU_AVAILABLE\t1\nGPU_CUDA_VERSION\t12.4\nGPU_CSV_BEGIN\n0, GPU-1, RTX 4090, 550.1, 50, 20, 10, 24576, 2048, 22528, 75.5, 450, 30, P2\nGPU_CSV_END\nGPU_PROCESS_CSV_BEGIN\nGPU-1, 42, 512, python\nGPU_PROCESS_CSV_END\n";
        let overview = parse_gpu_overview_output(output);

        assert!(overview.available);
        assert_eq!(overview.cuda_version, "12.4");
        assert_eq!(overview.driver_version, "550.1");
        assert_eq!(overview.gpus.len(), 1);
        assert_eq!(overview.gpus[0].memory_used_mb, 2048);
        assert_eq!(overview.processes[0].gpu_index, Some(0));
    }

    #[test]
    fn parses_gpu_unavailable() {
        let overview = parse_gpu_overview_output("GPU_AVAILABLE\t0\n");

        assert!(!overview.available);
        assert!(overview.gpus.is_empty());
    }
}
