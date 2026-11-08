//! Native resource statistics for the machine running the application.
//!
//! SSH hosts are sampled with a shell script; the local machine uses `sysinfo`,
//! which covers Windows, macOS, Linux and the BSDs without spawning a process.
//! The collector is stateful on purpose: CPU usage and network rates are deltas
//! between two refreshes, so the first snapshot reports a warming-up CPU exactly
//! like a freshly connected SSH host.

use crate::stats::{
    CpuCoreUsage, CpuInfo, CpuUsageSource, DiskInfo, LoadInfo, LoggedInUsersStatus, MemoryInfo,
    NetworkInfo, NetworkSummaryInfo, RemoteStats, SystemInfo,
};
use std::collections::HashMap;
use std::time::Instant;
use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, Networks, RefreshKind, System};

pub struct LocalStatsCollector {
    system: System,
    networks: Networks,
    disks: Disks,
    previous_network: HashMap<String, (u64, u64)>,
    last_refresh: Option<Instant>,
}

impl Default for LocalStatsCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalStatsCollector {
    pub fn new() -> Self {
        let system = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
                .with_memory(MemoryRefreshKind::everything()),
        );
        Self {
            system,
            networks: Networks::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list(),
            previous_network: HashMap::new(),
            last_refresh: None,
        }
    }

    pub fn snapshot(&mut self, collect_users: bool) -> RemoteStats {
        let now = Instant::now();
        let elapsed = self
            .last_refresh
            .map(|previous| now.duration_since(previous));
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.networks.refresh(true);
        self.disks.refresh(true);

        let cpus = self.system.cpus();
        let warmed = elapsed.is_some_and(|elapsed| elapsed >= sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        let cpu = CpuInfo {
            model: cpus
                .first()
                .map(|cpu| cpu.brand().trim().to_string())
                .filter(|brand| !brand.is_empty())
                .unwrap_or_else(|| "unknown".to_string()),
            cores: cpus.len() as u32,
            usage: warmed.then(|| f64::from(self.system.global_cpu_usage())),
            per_core: if warmed {
                cpus.iter()
                    .enumerate()
                    .map(|(id, cpu)| CpuCoreUsage {
                        id: id as u32,
                        usage: f64::from(cpu.cpu_usage()),
                    })
                    .collect()
            } else {
                Vec::new()
            },
            sample_window_ms: elapsed
                .filter(|_| warmed)
                .map(|elapsed| elapsed.as_millis() as u64),
            usage_source: if warmed {
                CpuUsageSource::Aggregate
            } else {
                CpuUsageSource::WarmingUp
            },
        };

        let total = self.system.total_memory();
        let available = self.system.available_memory();
        let memory = MemoryInfo {
            used: total.saturating_sub(available),
            available,
            // Not every OS distinguishes cache from free; "free" is the closest
            // honest value for the cached column where it does not.
            cached: available.saturating_sub(self.system.free_memory()),
        };

        let seconds = elapsed.map(|elapsed| elapsed.as_secs_f64()).unwrap_or(0.0);
        let mut networks = Vec::new();
        let mut current = HashMap::new();
        for (name, data) in self.networks.list() {
            let (rx, tx) = (data.total_received(), data.total_transmitted());
            let (rx_rate, tx_rate) = match self.previous_network.get(name) {
                Some((prev_rx, prev_tx)) if seconds > 0.0 => (
                    rx.saturating_sub(*prev_rx) as f64 / seconds,
                    tx.saturating_sub(*prev_tx) as f64 / seconds,
                ),
                _ => (0.0, 0.0),
            };
            current.insert(name.clone(), (rx, tx));
            networks.push(NetworkInfo {
                nic: name.clone(),
                state: "up".to_string(),
                rx_bytes_per_sec: rx_rate,
                tx_bytes_per_sec: tx_rate,
            });
        }
        networks.sort_by(|a, b| a.nic.cmp(&b.nic));
        self.previous_network = current;
        let network_summary = NetworkSummaryInfo {
            rx_bytes_per_sec: networks.iter().map(|n| n.rx_bytes_per_sec).sum(),
            tx_bytes_per_sec: networks.iter().map(|n| n.tx_bytes_per_sec).sum(),
        };

        let mut seen = std::collections::HashSet::new();
        let disks = self
            .disks
            .list()
            .iter()
            .filter(|disk| disk.total_space() > 0)
            .filter(|disk| seen.insert(disk.mount_point().to_path_buf()))
            .map(|disk| {
                let total = disk.total_space();
                let available = disk.available_space();
                DiskInfo {
                    device: disk.name().to_string_lossy().into_owned(),
                    mount: disk.mount_point().to_string_lossy().into_owned(),
                    total,
                    available,
                    use_percent: (total.saturating_sub(available) as f64 * 100.0 / total as f64)
                        .round() as u32,
                }
            })
            .collect();

        let load = System::load_average();
        self.last_refresh = Some(now);
        let users = if collect_users {
            let found = crate::login_sessions::collect_local();
            (found.status, found.users, found.sessions)
        } else {
            (LoggedInUsersStatus::NotCollected, Vec::new(), Vec::new())
        };
        RemoteStats {
            system: SystemInfo {
                hostname: System::host_name().unwrap_or_default(),
                uptime_sec: System::uptime(),
                os: friendly_os_name(),
                arch: System::cpu_arch(),
            },
            load: LoadInfo {
                load1: load.one,
                load5: load.five,
                load15: load.fifteen,
            },
            cpu,
            memory,
            networks,
            network_summary,
            disks,
            users: users.1,
            login_sessions: users.2,
            users_status: users.0,
        }
    }
}

/// `System::name()` reports the kernel family on some platforms ("Darwin");
/// people know the product ("macOS 15.1"), so prefer the long name.
fn friendly_os_name() -> String {
    let name = System::name().unwrap_or_default();
    let product = System::long_os_version()
        .filter(|long| !long.trim().is_empty())
        .unwrap_or_else(|| name.clone());
    if name.eq_ignore_ascii_case("darwin") && !product.to_ascii_lowercase().contains("macos") {
        return format!("macOS {product}");
    }
    product
}

#[cfg(test)]
mod tests {
    use super::LocalStatsCollector;
    use crate::stats::CpuUsageSource;

    #[test]
    fn first_snapshot_warms_up_then_reports_cpu_and_real_memory() {
        let mut collector = LocalStatsCollector::new();
        let first = collector.snapshot(false);
        assert_eq!(first.cpu.usage_source, CpuUsageSource::WarmingUp);
        assert!(first.memory.used > 0 && first.memory.available > 0);
        assert!(first.cpu.cores > 0);
        assert!(!first.system.hostname.is_empty());
        assert!(
            !first.system.os.eq_ignore_ascii_case("darwin"),
            "{}",
            first.system.os
        );
        assert!(!first.disks.is_empty());
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL * 2);
        let second = collector.snapshot(false);
        assert_eq!(second.cpu.usage_source, CpuUsageSource::Aggregate);
        assert_eq!(second.cpu.per_core.len() as u32, second.cpu.cores);
        assert!(second.system.uptime_sec > 0);
    }
}
