use crate::error::{AppError, AppResult};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const MAX_SAMPLE_WINDOW: Duration = Duration::from_secs(5 * 60);
const SAMPLE_TTL: Duration = Duration::from_secs(10 * 60);
const CPU_USAGE_DIFF_THRESHOLD: f64 = 5.0;
const CPU_TOTAL_GAP_RATIO_THRESHOLD: f64 = 0.02;

#[derive(serde::Serialize, Default, Clone)]
pub struct SystemInfo {
    pub hostname: String,
    pub uptime_sec: u64,
    pub os: String,
    pub arch: String,
}

#[derive(serde::Serialize, Default, Clone)]
pub struct LoadInfo {
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
}

#[derive(serde::Serialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CpuUsageSource {
    #[default]
    WarmingUp,
    Aggregate,
    CoreWeightedFallback,
}

#[derive(serde::Serialize, Debug, Clone, Copy, PartialEq)]
pub struct CpuCoreUsage {
    pub id: u32,
    pub usage: f64,
}

#[derive(serde::Serialize, Default, Clone)]
pub struct CpuInfo {
    pub model: String,
    pub cores: u32,
    pub usage: Option<f64>,
    pub per_core: Vec<CpuCoreUsage>,
    pub sample_window_ms: Option<u64>,
    pub usage_source: CpuUsageSource,
}

#[derive(serde::Serialize, Default, Clone)]
pub struct MemoryInfo {
    pub used: u64,
    pub available: u64,
    pub cached: u64,
}

#[derive(serde::Serialize, Clone)]
pub struct NetworkInfo {
    pub nic: String,
    pub state: String,
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
}

#[derive(serde::Serialize, Default, Clone)]
pub struct NetworkSummaryInfo {
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
}

#[derive(serde::Serialize, Clone)]
pub struct DiskInfo {
    pub device: String,
    pub mount: String,
    pub total: u64,
    pub available: u64,
    pub use_percent: u32,
}

#[derive(serde::Serialize, Default, Clone)]
pub struct RemoteStats {
    pub system: SystemInfo,
    pub load: LoadInfo,
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub networks: Vec<NetworkInfo>,
    pub network_summary: NetworkSummaryInfo,
    pub disks: Vec<DiskInfo>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CpuTicks {
    user: u64,
    nice: u64,
    system: u64,
    idle: u64,
    iowait: u64,
    irq: u64,
    softirq: u64,
    steal: u64,
    guest: u64,
    guest_nice: u64,
}

#[derive(Debug, Clone, Default)]
struct CpuSnapshot {
    uptime_sec: u64,
    aggregate: CpuTicks,
    cores: BTreeMap<u32, CpuTicks>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CpuDelta {
    busy: u64,
    idle: u64,
    total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NetworkCounters {
    rx_bytes: u64,
    tx_bytes: u64,
    top: bool,
}

#[derive(Clone)]
pub struct ParsedRemoteStats {
    stats: RemoteStats,
    cpu_snapshot: Option<CpuSnapshot>,
    networks: BTreeMap<String, NetworkCounters>,
}

#[derive(Debug, Clone)]
struct CachedRemoteSample {
    cpu_snapshot: Option<CpuSnapshot>,
    networks: BTreeMap<String, NetworkCounters>,
    updated_at: Instant,
}

#[derive(Default)]
pub struct RemoteStatsSampler {
    samples: Mutex<HashMap<String, CachedRemoteSample>>,
}

impl RemoteStatsSampler {
    pub async fn complete_snapshot(
        &self,
        session_id: &str,
        parsed: ParsedRemoteStats,
    ) -> RemoteStats {
        let now = Instant::now();
        let mut samples = self.samples.lock().await;
        prune_stale_samples(&mut samples, now);

        let previous = samples.get(session_id).cloned();
        let mut stats = parsed.stats;

        if let Some(previous) = previous.as_ref() {
            if let (Some(previous_cpu), Some(current_cpu)) =
                (previous.cpu_snapshot.as_ref(), parsed.cpu_snapshot.as_ref())
            {
                apply_cpu_usage(session_id, previous, previous_cpu, current_cpu, &mut stats);
            }

            apply_network_rates(previous, &parsed.networks, &mut stats);
        }

        update_network_summary(&mut stats, &parsed.networks);

        samples.insert(
            session_id.to_string(),
            CachedRemoteSample {
                cpu_snapshot: parsed.cpu_snapshot,
                networks: parsed.networks,
                updated_at: now,
            },
        );

        stats
    }

    pub async fn clear_session(&self, session_id: &str) {
        self.samples.lock().await.remove(session_id);
    }
}

fn prune_stale_samples(samples: &mut HashMap<String, CachedRemoteSample>, now: Instant) {
    samples.retain(|_, sample| now.duration_since(sample.updated_at) <= SAMPLE_TTL);
}

fn apply_cpu_usage(
    session_id: &str,
    previous_sample: &CachedRemoteSample,
    previous: &CpuSnapshot,
    current: &CpuSnapshot,
    stats: &mut RemoteStats,
) {
    let sample_window = previous_sample.updated_at.elapsed();

    if sample_window > MAX_SAMPLE_WINDOW || current.uptime_sec < previous.uptime_sec {
        return;
    }

    let Some(aggregate_delta) = calculate_delta(&previous.aggregate, &current.aggregate) else {
        return;
    };

    let mut per_core = Vec::new();
    let mut core_busy = 0_u64;
    let mut core_total = 0_u64;

    for (id, current_ticks) in &current.cores {
        let Some(previous_ticks) = previous.cores.get(id) else {
            continue;
        };
        let Some(delta) = calculate_delta(previous_ticks, current_ticks) else {
            continue;
        };

        core_busy = core_busy.saturating_add(delta.busy);
        core_total = core_total.saturating_add(delta.total);
        per_core.push(CpuCoreUsage {
            id: *id,
            usage: usage_percent(delta),
        });
    }

    let aggregate_usage = usage_percent(aggregate_delta);
    let mut selected_usage = aggregate_usage;
    let mut source = CpuUsageSource::Aggregate;

    if core_total > 0 {
        let weighted_core_usage = core_busy as f64 * 100.0 / core_total as f64;
        let usage_difference = (aggregate_usage - weighted_core_usage).abs();
        let total_gap = aggregate_delta.total.abs_diff(core_total);
        let total_gap_ratio = total_gap as f64 / aggregate_delta.total.max(1) as f64;

        if usage_difference > CPU_USAGE_DIFF_THRESHOLD
            || total_gap_ratio > CPU_TOTAL_GAP_RATIO_THRESHOLD
        {
            tracing::warn!(
                session_id = %session_id,
                aggregate_usage,
                weighted_core_usage,
                aggregate_total = aggregate_delta.total,
                core_total,
                usage_difference,
                total_gap_ratio,
                "cpu_sample_inconsistent"
            );
            selected_usage = weighted_core_usage;
            source = CpuUsageSource::CoreWeightedFallback;
        }
    }

    stats.cpu.usage = Some(selected_usage);
    stats.cpu.per_core = per_core;
    stats.cpu.sample_window_ms = u64::try_from(sample_window.as_millis()).ok();
    stats.cpu.usage_source = source;
}

fn apply_network_rates(
    previous: &CachedRemoteSample,
    current_networks: &BTreeMap<String, NetworkCounters>,
    stats: &mut RemoteStats,
) {
    let elapsed = previous.updated_at.elapsed().as_secs_f64();
    if elapsed <= 0.0 || elapsed > MAX_SAMPLE_WINDOW.as_secs_f64() {
        return;
    }

    for net in &mut stats.networks {
        let Some(current) = current_networks.get(&net.nic) else {
            continue;
        };
        let Some(previous) = previous.networks.get(&net.nic) else {
            continue;
        };

        net.rx_bytes_per_sec = current.rx_bytes.saturating_sub(previous.rx_bytes) as f64 / elapsed;
        net.tx_bytes_per_sec = current.tx_bytes.saturating_sub(previous.tx_bytes) as f64 / elapsed;
    }
}

fn update_network_summary(
    stats: &mut RemoteStats,
    networks: &BTreeMap<String, NetworkCounters>,
) {
    stats.network_summary = NetworkSummaryInfo {
        rx_bytes_per_sec: stats
            .networks
            .iter()
            .filter(|net| networks.get(&net.nic).map_or(true, |c| c.top))
            .map(|net| net.rx_bytes_per_sec)
            .sum(),
        tx_bytes_per_sec: stats
            .networks
            .iter()
            .filter(|net| networks.get(&net.nic).map_or(true, |c| c.top))
            .map(|net| net.tx_bytes_per_sec)
            .sum(),
    };
}

fn calculate_delta(previous: &CpuTicks, current: &CpuTicks) -> Option<CpuDelta> {
    let user = current.user.checked_sub(previous.user)?;
    let nice = current.nice.checked_sub(previous.nice)?;
    let system = current.system.checked_sub(previous.system)?;
    let idle = current.idle.checked_sub(previous.idle)?;
    let iowait = current.iowait.saturating_sub(previous.iowait);
    let irq = current.irq.checked_sub(previous.irq)?;
    let softirq = current.softirq.checked_sub(previous.softirq)?;
    let steal = current.steal.checked_sub(previous.steal)?;

    let busy = user
        .checked_add(nice)?
        .checked_add(system)?
        .checked_add(irq)?
        .checked_add(softirq)?
        .checked_add(steal)?;
    let idle_all = idle.checked_add(iowait)?;
    let total = busy.checked_add(idle_all)?;

    if total == 0 {
        return None;
    }

    Some(CpuDelta {
        busy,
        idle: idle_all,
        total,
    })
}

fn usage_percent(delta: CpuDelta) -> f64 {
    delta.busy as f64 * 100.0 / delta.total as f64
}

pub const SYSINFO_SCRIPT: &str = r#"sh -c '
LC_ALL=C
export LC_ALL

base=${TMPDIR:-/tmp}/sysinfo.$$;
hostf=$base.host;
archf=$base.arch;
cpucoref=$base.cpucores;
diskf=$base.disk;
diskraw=$base.diskraw;
dfraw=$base.dfraw;
netrawf=$base.netraw;
netoutf=$base.netout;

trap "rm -f \"$base\".*" 0 HUP INT TERM;

run_limited() {
  run_out=$1;
  run_seconds=$2;
  shift 2;
  run_tmp=$run_out.tmp;

  rm -f "$run_out" "$run_tmp";

  (
    "$@" >"$run_tmp" 2>/dev/null
  ) &
  run_pid=$!;

  (
    sleep "$run_seconds" 2>/dev/null || sleep 1;
    kill "$run_pid" 2>/dev/null;
    sleep 1;
    kill -9 "$run_pid" 2>/dev/null;
  ) &
  run_watch=$!;

  wait "$run_pid" 2>/dev/null;
  run_status=$?;

  kill "$run_watch" 2>/dev/null;
  wait "$run_watch" 2>/dev/null;

  if [ "$run_status" -eq 0 ]; then
    mv "$run_tmp" "$run_out" 2>/dev/null || return 1;
    return 0;
  fi;

  rm -f "$run_tmp";
  : >"$run_out";
  return 1;
}

host=unknown;
if [ -r /proc/sys/kernel/hostname ]; then
  IFS= read -r host </proc/sys/kernel/hostname || host=unknown;
fi;

if [ -z "$host" ] || [ "$host" = "unknown" ]; then
  if run_limited "$hostf" 1 uname -n && [ -s "$hostf" ]; then
    IFS= read -r host <"$hostf" || host=unknown;
  fi;
fi;

[ -n "$host" ] || host=unknown;
host=$(printf "%s" "$host" | tr "\t\r\n" "   ");

uptime_sec=0;
if [ -r /proc/uptime ]; then
  read upraw _ </proc/uptime || upraw=0;
  uptime_sec=${upraw%.*};
fi;
[ -n "$uptime_sec" ] || uptime_sec=0;

os=unknown;
if [ -r /etc/os-release ]; then
  . /etc/os-release;
  os=${PRETTY_NAME:-unknown};
else
  if run_limited "$hostf" 1 uname -s && [ -s "$hostf" ]; then
    IFS= read -r os <"$hostf" || os=unknown;
  fi;
fi;
[ -n "$os" ] || os=unknown;
os=$(printf "%s" "$os" | tr "\t\r\n" "   ");

arch=unknown;
if run_limited "$archf" 1 uname -m && [ -s "$archf" ]; then
  IFS= read -r arch <"$archf" || arch=unknown;
fi;
[ -n "$arch" ] || arch=unknown;

l1=0;
l5=0;
l15=0;
if [ -r /proc/loadavg ]; then
  read l1 l5 l15 _ </proc/loadavg || {
    l1=0;
    l5=0;
    l15=0;
  };
fi;

cpu_model=$(awk -F: '"'"'
/^(model name|Hardware|Processor|cpu model)[[:space:]]*:/ && !m {
  gsub(/^[ \t]+/, "", $2);
  m=$2;
}
END {
  if (!m) m="unknown";
  print m;
}
'"'"' /proc/cpuinfo 2>/dev/null);

cpu_model=$(printf "%s" "$cpu_model" | tr "\t\r\n" "   ");
[ -n "$cpu_model" ] || cpu_model=unknown;

cpu_cores=$(awk '"'"'
/^processor[[:space:]]*:/ { c++ }
END { print c+0 }
'"'"' /proc/cpuinfo 2>/dev/null);

case $cpu_cores in
  ""|0)
    if run_limited "$cpucoref" 1 getconf _NPROCESSORS_ONLN && [ -s "$cpucoref" ]; then
      IFS= read -r cpu_cores <"$cpucoref" || cpu_cores=0;
    fi;
    ;;
esac;
[ -n "$cpu_cores" ] || cpu_cores=0;

set -- $(awk '"'"'
/MemTotal:/ { t=$2 }
/MemAvailable:/ { a=$2 }
/Buffers:/ { b=$2 }
/^Cached:/ { c=$2 }
/SReclaimable:/ { s=$2 }
END {
  if (!t) t=0;
  if (!a) a=0;
  if (!b) b=0;
  if (!c) c=0;
  if (!s) s=0;
  printf "%.0f %.0f %.0f\n", (t-a)*1024, a*1024, (b+c+s)*1024;
}
'"'"' /proc/meminfo 2>/dev/null);

mem_used=${1:-0};
mem_avail=${2:-0};
mem_cache=${3:-0};

printf "SYSTEM\t%s\t%s\t%s\t%s\n" "$host" "$uptime_sec" "$os" "$arch";
printf "LOAD\t%s\t%s\t%s\n" "$l1" "$l5" "$l15";
printf "CPU\t%s\t%s\n" "$cpu_model" "$cpu_cores";

if [ -r /proc/stat ]; then
  while read -r name user nice system idle iowait irq softirq steal guest guest_nice rest; do
    case "$name" in
      cpu|cpu[0-9]*)
        printf "CPUTICKS\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n" \
          "$name" \
          "${user:-0}" \
          "${nice:-0}" \
          "${system:-0}" \
          "${idle:-0}" \
          "${iowait:-0}" \
          "${irq:-0}" \
          "${softirq:-0}" \
          "${steal:-0}" \
          "${guest:-0}" \
          "${guest_nice:-0}";
        ;;
      *)
        break;
        ;;
    esac;
  done </proc/stat;
fi;

printf "MEMORY\t%s\t%s\t%s\n" "$mem_used" "$mem_avail" "$mem_cache";

if [ -r /proc/net/dev ]; then
  awk '"'"'
NR>2 {
  line=$0;
  sub(/^[ \t]+/, "", line);
  split(line, a, ":");
  nic=a[1];
  gsub(/^[ \t]+|[ \t]+$/, "", nic);
  gsub(/^[ \t]+/, "", a[2]);
  split(a[2], f, /[ \t]+/);
  print nic "\t" f[1] "\t" f[9];
}
'"'"' /proc/net/dev 2>/dev/null >"$netrawf";

  emit_netdev() {
    mode=$1;
    while IFS="$(printf "\t")" read -r nic rx tx; do
      [ -n "$nic" ] || continue;
      [ "$nic" != "lo" ] || continue;
      case "$nic" in
        docker*|veth*|br-*|virbr*|flannel*|cali*|tunl*|kube-ipvs0|cni*|zt*|tailscale*|wg*|tap*|vnet*)
          continue;
          ;;
      esac;

      state=unknown;
      if [ -r "/sys/class/net/$nic/operstate" ]; then
        IFS= read -r state <"/sys/class/net/$nic/operstate" || state=unknown;
      fi;
      [ -n "$state" ] || state=unknown;
      if [ "$state" = "down" ]; then continue; fi;

      top=yes;
      if [ -e "/sys/class/net/$nic/master" ]; then top=no; fi;
      if [ "$top" = "yes" ]; then
        aggregator=false;
        if [ -d "/sys/class/net/$nic/brif" ]; then aggregator=true; fi;
        if [ -d "/sys/class/net/$nic/bonding" ]; then aggregator=true; fi;
        if [ -d "/sys/class/net/$nic/team" ]; then aggregator=true; fi;
        if [ "$aggregator" = "false" ]; then
          for lwr in /sys/class/net/$nic/lower_*; do
            if [ -e "$lwr" ]; then top=no; break; fi;
          done;
        fi;
      fi;

      if [ "$mode" = "strict" ]; then
        carrier=0;
        if [ -r "/sys/class/net/$nic/carrier" ]; then
          IFS= read -r carrier <"/sys/class/net/$nic/carrier" || carrier=0;
        fi;
        if [ "$state" != "up" ] && [ "$carrier" != "1" ]; then continue; fi;
        if [ -e "/sys/class/net/$nic/master" ]; then continue; fi;
      fi;

      printf "NETDEV\t%s\t%s\t%s\t%s\t%s\n" "$nic" "$state" "${rx:-0}" "${tx:-0}" "$top";
    done <"$netrawf";
  };

  emit_netdev strict >"$netoutf";
  if [ ! -s "$netoutf" ]; then
    emit_netdev loose >"$netoutf";
  fi;
  cat "$netoutf";
fi;

: >"$diskf";

if command -v findmnt >/dev/null 2>&1; then
  if run_limited "$diskraw" 2 findmnt -b -rn -o SOURCE,TARGET,FSTYPE,SIZE,AVAIL,USE%; then
    awk '"'"'
  BEGIN {
    OFS="\t";
  }
  {
    src=$1;
    mp=$2;
    fstype=$3;
    total=$4;
    avail=$5;
    usep=$6;

    if (src !~ "^/dev/") next;
    if (mp=="" || mp=="-") next;
    if (seen[mp]++) next;

    if (fstype ~ /^(tmpfs|devtmpfs|squashfs|overlay|proc|sysfs|cgroup|cgroup2|devpts|securityfs|pstore|bpf|tracefs|debugfs|mqueue|hugetlbfs|fusectl|configfs|autofs|ramfs|binfmt_misc)$/) next;

    gsub(/%/, "", usep);

    printf "%s\t%s\t%s\t%s\t%s\n", src, mp, total, avail, usep;
  }
  '"'"' "$diskraw" >"$diskf" 2>/dev/null || : >"$diskf";
  fi;
fi;

if [ ! -s "$diskf" ] && command -v df >/dev/null 2>&1; then
  if run_limited "$dfraw" 2 df -B1 -P; then
    awk '"'"'
  BEGIN {
    OFS="\t";
  }
  NR>1 {
    src=$1;
    total=$2;
    avail=$4;
    usep=$5;
    mp=$6;

    if (src !~ "^/dev/") next;
    if (mp=="" || mp=="-") next;
    if (seen[mp]++) next;

    gsub(/%/, "", usep);

    printf "%s\t%s\t%s\t%s\t%s\n", src, mp, total, avail, usep;
  }
  '"'"' "$dfraw" >"$diskf" 2>/dev/null || : >"$diskf";
  fi;
fi;

if [ -s "$diskf" ]; then
  while IFS="$(printf "\t")" read -r disk mp total avail usep; do
    [ -n "$disk" ] || continue;
    printf "DISK\t%s\t%s\t%s\t%s\t%s\n" "$disk" "$mp" "$total" "$avail" "$usep";
  done <"$diskf";
else
  printf "DISK\t-\t-\t0\t0\t0\n";
fi
'"#;

pub fn parse_stats_output(output: &str) -> AppResult<ParsedRemoteStats> {
    let mut stats = RemoteStats::default();
    let mut cpu_snapshot = CpuSnapshot::default();
    let mut has_cpu_snapshot = false;
    let mut networks = BTreeMap::new();
    let mut seen_disk_mounts = HashSet::new();

    for line in output.lines() {
        let cols: Vec<&str> = line.split('\t').collect();

        if cols.is_empty() {
            continue;
        }

        match cols[0] {
            "SYSTEM" if cols.len() >= 5 => {
                let uptime_sec = parse_u64_field(cols[2], "SYSTEM uptime")?;
                cpu_snapshot.uptime_sec = uptime_sec;
                stats.system = SystemInfo {
                    hostname: cols[1].to_string(),
                    uptime_sec,
                    os: cols[3].to_string(),
                    arch: cols[4].to_string(),
                };
            }

            "LOAD" if cols.len() >= 4 => {
                stats.load = LoadInfo {
                    load1: cols[1].parse().unwrap_or(0.0),
                    load5: cols[2].parse().unwrap_or(0.0),
                    load15: cols[3].parse().unwrap_or(0.0),
                };
            }

            "CPU" if cols.len() >= 3 => {
                stats.cpu = CpuInfo {
                    model: cols[1].to_string(),
                    cores: cols[2].parse().unwrap_or(0),
                    usage: None,
                    per_core: Vec::new(),
                    sample_window_ms: None,
                    usage_source: CpuUsageSource::WarmingUp,
                };
            }

            "CPUTICKS" if cols.len() >= 12 => {
                let ticks = parse_cpu_ticks(&cols)?;
                match cols[1] {
                    "cpu" => {
                        cpu_snapshot.aggregate = ticks;
                        has_cpu_snapshot = true;
                    }
                    cpu_name if cpu_name.starts_with("cpu") => {
                        let id = cpu_name[3..].parse::<u32>().map_err(|error| {
                            AppError::Config(format!("Invalid CPU core id '{cpu_name}': {error}"))
                        })?;
                        cpu_snapshot.cores.insert(id, ticks);
                        has_cpu_snapshot = true;
                    }
                    _ => {}
                }
            }

            "MEMORY" if cols.len() >= 4 => {
                stats.memory = MemoryInfo {
                    used: cols[1].parse().unwrap_or(0),
                    available: cols[2].parse().unwrap_or(0),
                    cached: cols[3].parse().unwrap_or(0),
                };
            }

            "NETDEV" if cols.len() >= 5 => {
                if cols[1] != "-" {
                    let rx_bytes = parse_u64_field(cols[3], "NETDEV rx bytes")?;
                    let tx_bytes = parse_u64_field(cols[4], "NETDEV tx bytes")?;
                    let top = cols.get(5).map_or(true, |v| *v == "yes");
                    networks.insert(
                        cols[1].to_string(),
                        NetworkCounters { rx_bytes, tx_bytes, top },
                    );
                    stats.networks.push(NetworkInfo {
                        nic: cols[1].to_string(),
                        state: cols[2].to_string(),
                        rx_bytes_per_sec: 0.0,
                        tx_bytes_per_sec: 0.0,
                    });
                }
            }

            "NETWORK" if cols.len() >= 5 => {
                if cols[1] != "-" {
                    stats.networks.push(NetworkInfo {
                        nic: cols[1].to_string(),
                        state: cols[2].to_string(),
                        rx_bytes_per_sec: cols[3].parse().unwrap_or(0.0),
                        tx_bytes_per_sec: cols[4].parse().unwrap_or(0.0),
                    });
                }
            }

            "DISK" if cols.len() >= 6 => {
                if cols[1] != "-" {
                    let mount = cols[2].trim();

                    if mount.is_empty() || mount == "-" {
                        continue;
                    }

                    if seen_disk_mounts.insert(mount.to_string()) {
                        stats.disks.push(DiskInfo {
                            device: cols[1].to_string(),
                            mount: mount.to_string(),
                            total: cols[3].parse().unwrap_or(0),
                            available: cols[4].parse().unwrap_or(0),
                            use_percent: cols[5].parse().unwrap_or(0),
                        });
                    }
                }
            }

            _ => {}
        }
    }

    update_network_summary(&mut stats, &networks);

    Ok(ParsedRemoteStats {
        stats,
        cpu_snapshot: has_cpu_snapshot.then_some(cpu_snapshot),
        networks,
    })
}

fn parse_cpu_ticks(cols: &[&str]) -> AppResult<CpuTicks> {
    Ok(CpuTicks {
        user: parse_u64_field(cols[2], "CPUTICKS user")?,
        nice: parse_u64_field(cols[3], "CPUTICKS nice")?,
        system: parse_u64_field(cols[4], "CPUTICKS system")?,
        idle: parse_u64_field(cols[5], "CPUTICKS idle")?,
        iowait: parse_u64_field(cols[6], "CPUTICKS iowait")?,
        irq: parse_u64_field(cols[7], "CPUTICKS irq")?,
        softirq: parse_u64_field(cols[8], "CPUTICKS softirq")?,
        steal: parse_u64_field(cols[9], "CPUTICKS steal")?,
        guest: parse_u64_field(cols[10], "CPUTICKS guest")?,
        guest_nice: parse_u64_field(cols[11], "CPUTICKS guest_nice")?,
    })
}

fn parse_u64_field(value: &str, field: &str) -> AppResult<u64> {
    value
        .parse::<u64>()
        .map_err(|error| AppError::Config(format!("Invalid {field} value '{value}': {error}")))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        CpuTicks, CpuUsageSource, NetworkCounters, NetworkInfo, RemoteStats, RemoteStatsSampler,
        calculate_delta, parse_stats_output, update_network_summary, usage_percent,
    };

    fn cpu_line(model: &str, cores: u32) -> String {
        format!("CPU\t{model}\t{cores}\n")
    }

    fn ticks_line(name: &str, user: u64, nice: u64, system: u64, idle: u64) -> String {
        format!("CPUTICKS\t{name}\t{user}\t{nice}\t{system}\t{idle}\t0\t0\t0\t0\t0\t0\n")
    }

    fn snapshot_output(
        uptime: u64,
        aggregate: (u64, u64, u64, u64),
        cores: &[(u32, u64, u64)],
    ) -> String {
        let mut output = format!(
            "SYSTEM\tnode-1\t{uptime}\tUbuntu 24.04\tx86_64\nLOAD\t0.10\t0.20\t0.30\n{}",
            cpu_line("AMD Ryzen", cores.len() as u32)
        );
        output.push_str(&ticks_line(
            "cpu",
            aggregate.0,
            aggregate.1,
            aggregate.2,
            aggregate.3,
        ));
        for (id, busy, idle) in cores {
            output.push_str(&ticks_line(&format!("cpu{id}"), *busy, 0, 0, *idle));
        }
        output.push_str(
            "MEMORY\t1000\t3000\t500\nNETDEV\teth0\tup\t1000\t2000\nDISK\t/dev/sda1\t/\t10000\t4000\t60\n",
        );
        output
    }

    #[test]
    fn parse_stats_output_parses_complete_snapshot() {
        let parsed = parse_stats_output(
            "SYSTEM\tnode-1\t12345\tUbuntu 24.04\tx86_64\n\
             LOAD\t0.10\t0.20\t0.30\n\
             CPU\tAMD Ryzen\t8\n\
             CPUTICKS\tcpu\t100\t0\t50\t850\t0\t0\t0\t0\t0\t0\n\
             CPUTICKS\tcpu0\t10\t0\t5\t85\t0\t0\t0\t0\t0\t0\n\
             CPUTICKS\tcpu1\t15\t0\t5\t80\t0\t0\t0\t0\t0\t0\n\
             MEMORY\t1000\t3000\t500\n\
             NETDEV\teth0\tup\t100\t200\n\
             NETDEV\twlan0\tup\t50\t25\n\
             DISK\t/dev/sda1\t/\t10000\t4000\t60\n",
        )
        .unwrap();
        let stats = parsed.stats;

        assert_eq!(stats.system.hostname, "node-1");
        assert_eq!(stats.system.uptime_sec, 12345);
        assert_eq!(stats.system.os, "Ubuntu 24.04");
        assert_eq!(stats.system.arch, "x86_64");
        assert_eq!(stats.load.load1, 0.10);
        assert_eq!(stats.cpu.model, "AMD Ryzen");
        assert_eq!(stats.cpu.cores, 8);
        assert_eq!(stats.cpu.usage, None);
        assert!(stats.cpu.per_core.is_empty());
        assert_eq!(stats.cpu.usage_source, CpuUsageSource::WarmingUp);
        assert_eq!(stats.memory.used, 1000);
        assert_eq!(stats.memory.available, 3000);
        assert_eq!(stats.memory.cached, 500);
        assert_eq!(stats.networks.len(), 2);
        assert_eq!(stats.network_summary.rx_bytes_per_sec, 0.0);
        assert_eq!(stats.network_summary.tx_bytes_per_sec, 0.0);
        assert_eq!(stats.disks.len(), 1);
        assert_eq!(stats.disks[0].mount, "/");
        assert_eq!(stats.disks[0].available, 4000);
        assert!(parsed.cpu_snapshot.is_some());
        assert_eq!(parsed.networks.len(), 2);
    }

    #[test]
    fn parse_stats_output_keeps_partial_snapshot_without_disks() {
        let without_disk = parse_stats_output(
            "SYSTEM\tnode-1\t12345\tUbuntu 24.04\tx86_64\n\
             LOAD\t0.10\t0.20\t0.30\n\
             CPU\tAMD Ryzen\t8\n\
             CPUTICKS\tcpu\t100\t0\t50\t850\t0\t0\t0\t0\t0\t0\n\
             MEMORY\t1000\t3000\t500\n\
             NETDEV\teth0\tup\t100\t200\n",
        )
        .unwrap()
        .stats;

        assert_eq!(without_disk.cpu.usage, None);
        assert_eq!(without_disk.memory.available, 3000);
        assert_eq!(without_disk.network_summary.rx_bytes_per_sec, 0.0);
        assert!(without_disk.disks.is_empty());

        let placeholder_disk = parse_stats_output(
            "SYSTEM\tnode-1\t12345\tUbuntu 24.04\tx86_64\n\
             LOAD\t0.10\t0.20\t0.30\n\
             CPU\tAMD Ryzen\t8\n\
             CPUTICKS\tcpu\t100\t0\t50\t850\t0\t0\t0\t0\t0\t0\n\
             MEMORY\t1000\t3000\t500\n\
             NETDEV\teth0\tup\t100\t200\n\
             DISK\t-\t-\t0\t0\t0\n",
        )
        .unwrap()
        .stats;

        assert_eq!(placeholder_disk.cpu.usage, None);
        assert_eq!(placeholder_disk.network_summary.tx_bytes_per_sec, 0.0);
        assert!(placeholder_disk.disks.is_empty());
    }

    #[test]
    fn network_summary_excludes_derived_interfaces() {
        let mut stats = RemoteStats::default();
        stats.networks = vec![
            NetworkInfo {
                nic: "eth0".to_string(),
                state: "up".to_string(),
                rx_bytes_per_sec: 100.0,
                tx_bytes_per_sec: 200.0,
            },
            NetworkInfo {
                nic: "eth0.100".to_string(),
                state: "up".to_string(),
                rx_bytes_per_sec: 40.0,
                tx_bytes_per_sec: 50.0,
            },
        ];
        let mut counters = BTreeMap::new();
        counters.insert(
            "eth0".to_string(),
            NetworkCounters { rx_bytes: 0, tx_bytes: 0, top: true },
        );
        counters.insert(
            "eth0.100".to_string(),
            NetworkCounters { rx_bytes: 0, tx_bytes: 0, top: false },
        );

        update_network_summary(&mut stats, &counters);

        assert_eq!(stats.network_summary.rx_bytes_per_sec, 100.0);
        assert_eq!(stats.network_summary.tx_bytes_per_sec, 200.0);
    }

    #[test]
    fn network_summary_treats_missing_top_field_as_top_level() {
        let mut stats = RemoteStats::default();
        stats.networks = vec![
            NetworkInfo {
                nic: "eth0".to_string(),
                state: "up".to_string(),
                rx_bytes_per_sec: 100.0,
                tx_bytes_per_sec: 200.0,
            },
            NetworkInfo {
                nic: "legacy".to_string(),
                state: "up".to_string(),
                rx_bytes_per_sec: 10.0,
                tx_bytes_per_sec: 20.0,
            },
        ];
        let mut counters = BTreeMap::new();
        counters.insert(
            "legacy".to_string(),
            NetworkCounters { rx_bytes: 0, tx_bytes: 0, top: false },
        );

        update_network_summary(&mut stats, &counters);

        assert_eq!(stats.network_summary.rx_bytes_per_sec, 100.0);
        assert_eq!(stats.network_summary.tx_bytes_per_sec, 200.0);
    }

    #[test]
    fn parse_stats_output_deduplicates_disk_mounts() {
        let stats = parse_stats_output(
            "DISK\t/dev/sda1\t/\t10000\t4000\t60\n\
             DISK\t/dev/disk/by-uuid/root\t/\t10000\t3000\t70\n\
             DISK\t/dev/sdb1\t/data\t20000\t15000\t25\n",
        )
        .unwrap()
        .stats;

        assert_eq!(stats.disks.len(), 2);
        assert_eq!(stats.disks[0].device, "/dev/sda1");
        assert_eq!(stats.disks[0].mount, "/");
        assert_eq!(stats.disks[1].mount, "/data");
    }

    #[tokio::test]
    async fn first_sample_returns_warming_up() {
        let sampler = RemoteStatsSampler::default();
        let parsed =
            parse_stats_output(&snapshot_output(10, (100, 0, 0, 900), &[(0, 10, 90)])).unwrap();
        let stats = sampler.complete_snapshot("s1", parsed).await;

        assert_eq!(stats.cpu.usage, None);
        assert_eq!(stats.cpu.usage_source, CpuUsageSource::WarmingUp);
    }

    #[tokio::test]
    async fn one_busy_core_on_32_cpus_is_about_three_percent() {
        let sampler = RemoteStatsSampler::default();
        let cores1: Vec<_> = (0..32).map(|id| (id, 0, 100)).collect();
        let cores2: Vec<_> = (0..32)
            .map(|id| {
                if id == 0 {
                    (id, 100, 100)
                } else {
                    (id, 0, 200)
                }
            })
            .collect();

        let first = parse_stats_output(&snapshot_output(10, (0, 0, 0, 3200), &cores1)).unwrap();
        sampler.complete_snapshot("s1", first).await;
        let second = parse_stats_output(&snapshot_output(11, (100, 0, 0, 6300), &cores2)).unwrap();
        let stats = sampler.complete_snapshot("s1", second).await;

        let usage = stats.cpu.usage.unwrap();
        assert!((usage - 3.125).abs() < 0.01, "{usage}");
    }

    #[tokio::test]
    async fn aggregate_matches_weighted_core_usage() {
        let sampler = RemoteStatsSampler::default();
        let first = parse_stats_output(&snapshot_output(
            10,
            (100, 0, 0, 900),
            &[(0, 50, 450), (1, 50, 450)],
        ))
        .unwrap();
        sampler.complete_snapshot("s1", first).await;
        let second = parse_stats_output(&snapshot_output(
            11,
            (120, 0, 0, 980),
            &[(0, 60, 490), (1, 60, 490)],
        ))
        .unwrap();
        let stats = sampler.complete_snapshot("s1", second).await;

        assert_eq!(stats.cpu.usage_source, CpuUsageSource::Aggregate);
        assert!((stats.cpu.usage.unwrap() - 20.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn inconsistent_aggregate_uses_core_fallback() {
        let sampler = RemoteStatsSampler::default();
        let first = parse_stats_output(&snapshot_output(
            10,
            (100, 0, 0, 900),
            &[(0, 50, 450), (1, 50, 450)],
        ))
        .unwrap();
        sampler.complete_snapshot("s1", first).await;
        let second = parse_stats_output(&snapshot_output(
            11,
            (194, 0, 0, 906),
            &[(0, 55, 495), (1, 55, 495)],
        ))
        .unwrap();
        let stats = sampler.complete_snapshot("s1", second).await;

        assert_eq!(stats.cpu.usage_source, CpuUsageSource::CoreWeightedFallback);
        assert!((stats.cpu.usage.unwrap() - 10.0).abs() < 0.01);
    }

    #[test]
    fn zero_delta_returns_none_instead_of_zero() {
        let ticks = CpuTicks {
            user: 1,
            idle: 1,
            ..CpuTicks::default()
        };

        assert_eq!(calculate_delta(&ticks, &ticks), None);
    }

    #[tokio::test]
    async fn counter_rollback_resets_baseline() {
        let sampler = RemoteStatsSampler::default();
        let first =
            parse_stats_output(&snapshot_output(10, (100, 0, 0, 900), &[(0, 100, 900)])).unwrap();
        sampler.complete_snapshot("s1", first).await;
        let second =
            parse_stats_output(&snapshot_output(11, (90, 0, 0, 950), &[(0, 90, 950)])).unwrap();
        let stats = sampler.complete_snapshot("s1", second).await;

        assert_eq!(stats.cpu.usage, None);
        assert_eq!(stats.cpu.usage_source, CpuUsageSource::WarmingUp);
    }

    #[tokio::test]
    async fn uptime_rollback_detects_remote_reboot() {
        let sampler = RemoteStatsSampler::default();
        let first =
            parse_stats_output(&snapshot_output(10, (100, 0, 0, 900), &[(0, 100, 900)])).unwrap();
        sampler.complete_snapshot("s1", first).await;
        let second =
            parse_stats_output(&snapshot_output(5, (110, 0, 0, 990), &[(0, 110, 990)])).unwrap();
        let stats = sampler.complete_snapshot("s1", second).await;

        assert_eq!(stats.cpu.usage, None);
        assert_eq!(stats.cpu.usage_source, CpuUsageSource::WarmingUp);
    }

    #[test]
    fn large_counters_above_i32_max_are_parsed_as_u64() {
        let parsed = parse_stats_output(
            "SYSTEM\tnode-1\t12345\tUbuntu 24.04\tx86_64\n\
             CPU\tAMD Ryzen\t1\n\
             CPUTICKS\tcpu\t2147483648\t0\t0\t2147483649\t0\t0\t0\t0\t0\t0\n",
        )
        .unwrap();

        assert_eq!(parsed.cpu_snapshot.unwrap().aggregate.user, 2_147_483_648);
    }

    #[tokio::test]
    async fn sparse_cpu_ids_keep_their_original_ids() {
        let sampler = RemoteStatsSampler::default();
        let first = parse_stats_output(&snapshot_output(
            10,
            (100, 0, 0, 900),
            &[(0, 50, 450), (2, 50, 450)],
        ))
        .unwrap();
        sampler.complete_snapshot("s1", first).await;
        let second = parse_stats_output(&snapshot_output(
            11,
            (120, 0, 0, 980),
            &[(0, 60, 490), (2, 60, 490)],
        ))
        .unwrap();
        let stats = sampler.complete_snapshot("s1", second).await;

        assert_eq!(
            stats
                .cpu
                .per_core
                .iter()
                .map(|core| core.id)
                .collect::<Vec<_>>(),
            vec![0, 2]
        );
    }

    #[test]
    fn decreasing_iowait_does_not_create_cpu_spike() {
        let previous = CpuTicks {
            idle: 100,
            iowait: 100,
            ..CpuTicks::default()
        };
        let current = CpuTicks {
            idle: 200,
            iowait: 90,
            ..CpuTicks::default()
        };

        let delta = calculate_delta(&previous, &current).unwrap();
        assert_eq!(usage_percent(delta), 0.0);
    }

    #[test]
    fn invalid_cpu_counter_is_not_silently_parsed_as_zero() {
        let parsed = parse_stats_output(
            "SYSTEM\tnode-1\t12345\tUbuntu 24.04\tx86_64\n\
             CPU\tAMD Ryzen\t1\n\
             CPUTICKS\tcpu\tbad\t0\t0\t100\t0\t0\t0\t0\t0\t0\n",
        );

        assert!(parsed.is_err());
    }
}
