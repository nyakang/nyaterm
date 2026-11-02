use crate::{
    RemoteCommandOutput, SshMultiplexHandle, SshSessionConfig, ensure_remote_command_success,
    run_ssh_command,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const MAX_SAMPLE_WINDOW: Duration = Duration::from_secs(5 * 60);
const SAMPLE_TTL: Duration = Duration::from_secs(10 * 60);
const CPU_USAGE_DIFF_THRESHOLD: f64 = 5.0;
const CPU_TOTAL_GAP_RATIO_THRESHOLD: f64 = 0.02;

#[derive(serde::Serialize, Debug, Default, Clone, PartialEq)]
pub struct SystemInfo {
    pub hostname: String,
    pub uptime_sec: u64,
    pub os: String,
    pub arch: String,
}

#[derive(serde::Serialize, Debug, Default, Clone, PartialEq)]
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

#[derive(serde::Serialize, Debug, Default, Clone, PartialEq)]
pub struct CpuInfo {
    pub model: String,
    pub cores: u32,
    pub usage: Option<f64>,
    pub per_core: Vec<CpuCoreUsage>,
    pub sample_window_ms: Option<u64>,
    pub usage_source: CpuUsageSource,
}

#[derive(serde::Serialize, Debug, Default, Clone, PartialEq)]
pub struct MemoryInfo {
    pub used: u64,
    pub available: u64,
    pub cached: u64,
}

#[derive(serde::Serialize, Debug, Clone, PartialEq)]
pub struct NetworkInfo {
    pub nic: String,
    pub state: String,
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
}

#[derive(serde::Serialize, Debug, Default, Clone, PartialEq)]
pub struct NetworkSummaryInfo {
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
}

#[derive(serde::Serialize, Debug, Clone, PartialEq)]
pub struct DiskInfo {
    pub device: String,
    pub mount: String,
    pub total: u64,
    pub available: u64,
    pub use_percent: u32,
}

#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
pub struct LoggedInUser {
    pub name: String,
    pub sessions: u32,
}

#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
pub struct LoggedInSession {
    pub name: String,
    pub terminal: String,
    pub login_time: String,
    pub source: Option<String>,
}

/// Whether the optional login-session probe produced a trustworthy result.
#[derive(serde::Serialize, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum LoggedInUsersStatus {
    #[default]
    NotCollected,
    Available,
    Unavailable,
    Failed,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RemoteStatsOptions {
    pub collect_logged_in_users: bool,
}

impl RemoteStatsOptions {
    fn command(self) -> String {
        // Only an internal boolean becomes a shell argument. No remote/user text.
        format!(
            "{SYSINFO_SCRIPT} nyaterm-stats {}",
            u8::from(self.collect_logged_in_users)
        )
    }
}

#[derive(serde::Serialize, Debug, Default, Clone, PartialEq)]
pub struct RemoteStats {
    pub system: SystemInfo,
    pub load: LoadInfo,
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub networks: Vec<NetworkInfo>,
    pub network_summary: NetworkSummaryInfo,
    pub disks: Vec<DiskInfo>,
    pub users: Vec<LoggedInUser>,
    pub login_sessions: Vec<LoggedInSession>,
    pub users_status: LoggedInUsersStatus,
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
    pub fn complete_snapshot(&self, session_id: &str, parsed: ParsedRemoteStats) -> RemoteStats {
        let now = Instant::now();
        let mut samples = self.samples.lock().expect("remote stats sampler poisoned");
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

        samples.insert(
            session_id.to_string(),
            CachedRemoteSample {
                cpu_snapshot: parsed.cpu_snapshot,
                networks: parsed.networks,
                updated_at: now,
            },
        );

        update_network_summary(&mut stats);
        stats
    }

    pub fn clear_session(&self, session_id: &str) {
        self.samples
            .lock()
            .expect("remote stats sampler poisoned")
            .remove(session_id);
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

fn update_network_summary(stats: &mut RemoteStats) {
    stats.network_summary = NetworkSummaryInfo {
        rx_bytes_per_sec: stats.networks.iter().map(|net| net.rx_bytes_per_sec).sum(),
        tx_bytes_per_sec: stats.networks.iter().map(|net| net.tx_bytes_per_sec).sum(),
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
collect_users=${1:-0};

base=${TMPDIR:-/tmp}/sysinfo.$$;
hostf=$base.host;
archf=$base.arch;
cpucoref=$base.cpucores;
diskf=$base.disk;
diskraw=$base.diskraw;
dfraw=$base.dfraw;
whof=$base.who;

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

if [ "$collect_users" = 1 ]; then
  if ! command -v who >/dev/null 2>&1; then
    printf "USERS_STATUS\tunavailable\n";
  elif run_limited "$whof" 1 who; then
    if awk '"'"'
NF { printf "LOGIN_SESSION\t%s\n", $0 }
END { print "USERS_STATUS\tavailable" }
'"'"' "$whof" 2>/dev/null; then :; else
      printf "USERS_STATUS\tfailed\n";
    fi;
  else
    printf "USERS_STATUS\tfailed\n";
  fi;
fi;

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
'"'"' /proc/net/dev 2>/dev/null | while IFS="$(printf "\t")" read -r nic rx tx; do
    [ -n "$nic" ] || continue;
    [ "$nic" != "lo" ] || continue;
    case "$nic" in
      docker*|veth*|br-*|virbr*|flannel*|cali*|tun*|tap*|vnet*|kube-ipvs0|cni*|zt*|tailscale*|wg*|dummy*|ifb*|sit*|gre*|gretap*|ip6tnl*|vxlan*|geneve*|erspan*)
        continue;
        ;;
    esac;
    if [ ! -e "/sys/class/net/$nic/device" ]; then
      derived=false;
      [ -d "/sys/class/net/$nic/bridge" ] && derived=true;
      [ -d "/sys/class/net/$nic/bonding" ] && derived=true;
      [ -d "/sys/class/net/$nic/team" ] && derived=true;
      if [ "$derived" = false ]; then
        for lower in "/sys/class/net/$nic"/lower_*; do
          if [ -e "$lower" ]; then
            derived=true;
            break;
          fi;
        done;
      fi;
      [ "$derived" = false ] || continue;
    fi;

    state=unknown;
    if [ -r "/sys/class/net/$nic/operstate" ]; then
      IFS= read -r state <"/sys/class/net/$nic/operstate" || state=unknown;
    fi;
    [ -n "$state" ] || state=unknown;
    carrier=0;
    if [ -r "/sys/class/net/$nic/carrier" ]; then
      IFS= read -r carrier <"/sys/class/net/$nic/carrier" || carrier=0;
    fi;
    if [ "$state" != "up" ] && [ "$carrier" != "1" ]; then
      continue;
    fi;

    printf "NETDEV\t%s\t%s\t%s\t%s\n" "$nic" "$state" "${rx:-0}" "${tx:-0}";
  done;
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

#[derive(Debug, Clone)]
pub struct RemoteStatsService {
    config: SshSessionConfig,
    multiplex: Option<SshMultiplexHandle>,
}

const STATS_TIMEOUT: Duration = Duration::from_secs(15);

impl RemoteStatsService {
    pub fn new(config: SshSessionConfig) -> Self {
        Self {
            config,
            multiplex: None,
        }
    }

    pub fn with_multiplex(
        config: SshSessionConfig,
        multiplex: SshMultiplexHandle,
    ) -> anyhow::Result<Self> {
        multiplex.ensure_matches_config(&config)?;
        Ok(Self {
            config,
            multiplex: Some(multiplex),
        })
    }

    pub fn snapshot(
        &self,
        sampler: &RemoteStatsSampler,
        session_id: &str,
    ) -> anyhow::Result<RemoteStats> {
        self.snapshot_with_options(sampler, session_id, RemoteStatsOptions::default())
    }

    pub fn snapshot_with_options(
        &self,
        sampler: &RemoteStatsSampler,
        session_id: &str,
        options: RemoteStatsOptions,
    ) -> anyhow::Result<RemoteStats> {
        if !self.config.remote_stats_enabled() {
            anyhow::bail!("remote stats are disabled for this SSH profile");
        }
        let output = self.exec_success(&options.command(), "Failed to fetch stats")?;
        let parsed = parse_stats_output(&output.stdout)?;
        Ok(sampler.complete_snapshot(session_id, parsed))
    }

    fn exec_success(&self, command: &str, context: &str) -> anyhow::Result<RemoteCommandOutput> {
        let output = run_ssh_command(
            self.config.clone(),
            self.multiplex.clone(),
            command.as_bytes().to_vec(),
            STATS_TIMEOUT,
        )?;
        ensure_remote_command_success(output, context)
    }
}
fn parse_login_session(record: &str) -> Option<LoggedInSession> {
    // `who` date formats vary across systems. Preserve the time text instead of
    // interpreting it in the application's locale or timezone.
    if record.chars().any(|ch| ch.is_control() && ch != '\t') {
        return None;
    }
    let record = record.trim();
    let name_end = record.find(char::is_whitespace)?;
    let name = &record[..name_end];
    let rest = record[name_end..].trim_start();
    let terminal_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let terminal = &rest[..terminal_end];
    if terminal.is_empty() {
        return None;
    }
    let details = rest[terminal_end..].trim();
    let (login_time, source) = if details.ends_with(')') {
        let start = details.rfind('(')?;
        if start > 0 && !details[..start].ends_with(char::is_whitespace) {
            return None;
        }
        let source = &details[start + 1..details.len() - 1];
        (details[..start].trim_end(), Some(source.to_string()))
    } else {
        (details, None)
    };
    if login_time.contains(['(', ')'])
        || source
            .as_ref()
            .is_some_and(|source| source.contains(['(', ')']))
    {
        return None;
    }
    Some(LoggedInSession {
        name: name.to_string(),
        terminal: terminal.to_string(),
        login_time: login_time.to_string(),
        source: source.filter(|source| !source.is_empty()),
    })
}

pub fn parse_stats_output(output: &str) -> anyhow::Result<ParsedRemoteStats> {
    let mut stats = RemoteStats::default();
    let mut cpu_snapshot = CpuSnapshot::default();
    let mut has_cpu_snapshot = false;
    let mut networks = BTreeMap::new();
    let mut seen_disk_mounts = HashSet::new();
    let mut users = BTreeMap::<String, u32>::new();
    let mut login_sessions = Vec::new();
    let mut invalid_users = false;

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
                            anyhow::anyhow!(format!("Invalid CPU core id '{cpu_name}': {error}"))
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
                    networks.insert(cols[1].to_string(), NetworkCounters { rx_bytes, tx_bytes });
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

            "USERS_STATUS" => {
                if stats.users_status != LoggedInUsersStatus::NotCollected {
                    invalid_users = true;
                }
                stats.users_status = match cols.as_slice() {
                    [_, "available"] => LoggedInUsersStatus::Available,
                    [_, "unavailable"] => LoggedInUsersStatus::Unavailable,
                    _ => LoggedInUsersStatus::Failed,
                };
            }
            "LOGIN_SESSION" => {
                let session = line
                    .split_once('\t')
                    .and_then(|(_, record)| parse_login_session(record));
                if let Some(session) = session {
                    login_sessions.push(session);
                } else {
                    invalid_users = true;
                }
            }
            "USER" => {
                let valid = (|| {
                    if cols.len() != 3
                        || cols[1].is_empty()
                        || cols[1].chars().any(char::is_control)
                        || !cols[2].bytes().all(|byte| byte.is_ascii_digit())
                    {
                        return None;
                    }
                    let count = cols[2].parse::<u32>().ok().filter(|count| *count > 0)?;
                    let total = users.entry(cols[1].to_string()).or_default();
                    *total = total.checked_add(count)?;
                    Some(())
                })();
                invalid_users |= valid.is_none();
            }

            "DISK" if cols.len() >= 6 && cols[1] != "-" => {
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

            _ => {}
        }
    }

    if !login_sessions.is_empty() {
        // Detailed records are authoritative; legacy USER summaries must not
        // double-count the same sessions if both record types are present.
        users.clear();
        for session in &login_sessions {
            let count = users.entry(session.name.clone()).or_default();
            if let Some(total) = count.checked_add(1) {
                *count = total;
            } else {
                invalid_users = true;
            }
        }
    }
    if invalid_users || (!users.is_empty() && stats.users_status != LoggedInUsersStatus::Available)
    {
        stats.users_status = LoggedInUsersStatus::Failed;
    }
    if stats.users_status == LoggedInUsersStatus::Available {
        login_sessions.sort_by(|a, b| (&a.name, &a.terminal).cmp(&(&b.name, &b.terminal)));
        stats.login_sessions = login_sessions;
        stats.users = users
            .into_iter()
            .map(|(name, sessions)| LoggedInUser { name, sessions })
            .collect();
    }
    update_network_summary(&mut stats);

    Ok(ParsedRemoteStats {
        stats,
        cpu_snapshot: has_cpu_snapshot.then_some(cpu_snapshot),
        networks,
    })
}

fn parse_cpu_ticks(cols: &[&str]) -> anyhow::Result<CpuTicks> {
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

fn parse_u64_field(value: &str, field: &str) -> anyhow::Result<u64> {
    value
        .parse::<u64>()
        .map_err(|error| anyhow::anyhow!(format!("Invalid {field} value '{value}': {error}")))
}

#[cfg(test)]
mod tests {
    use super::{
        CpuTicks, CpuUsageSource, RemoteStatsSampler, SYSINFO_SCRIPT, calculate_delta,
        parse_stats_output, usage_percent,
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
    fn network_probe_keeps_virtual_primary_nics_and_filters_derived_interfaces() {
        assert!(!SYSINFO_SCRIPT.contains("[ -e \"/sys/class/net/$nic/device\" ] || continue"));
        assert!(SYSINFO_SCRIPT.contains("/sys/class/net/$nic/bridge"));
        assert!(SYSINFO_SCRIPT.contains("/sys/class/net/$nic/bonding"));
        assert!(SYSINFO_SCRIPT.contains("/sys/class/net/$nic/team"));
        assert!(SYSINFO_SCRIPT.contains("/sys/class/net/$nic\"/lower_*"));
        assert!(SYSINFO_SCRIPT.contains("[ \"$carrier\" != \"1\" ]"));
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

    #[test]
    fn parse_stats_output_collects_logged_in_users_sorted_by_name() {
        let stats = parse_stats_output(
            "USERS_STATUS\tavailable\nUSER\troot\t1\nUSER\talice\t2\nUSER\talice\t1\n",
        )
        .unwrap()
        .stats;

        assert_eq!(
            stats
                .users
                .iter()
                .map(|user| (user.name.as_str(), user.sessions))
                .collect::<Vec<_>>(),
            [("alice", 3), ("root", 1)]
        );
        assert!(SYSINFO_SCRIPT.contains("run_limited \"$whof\" 1 who"));
        assert!(stats.login_sessions.is_empty());
    }

    #[test]
    fn login_session_details_preserve_remote_times_and_derive_account_counts() {
        let stats = parse_stats_output(concat!(
            "USERS_STATUS\tavailable\n",
            "LOGIN_SESSION\troot     pts/0        2026-11-01 23:37 (172.16.0.46)\n",
            "LOGIN_SESSION\tkang    pts/1        2026-11-01 22:37 (172.16.0.46)\n",
            "LOGIN_SESSION\tkang pts/2 Nov  1 22:40 (2001:db8::1)\n",
            "LOGIN_SESSION\tkang\tpts/3\tNov 01 2026 22:45 (host.example)\n",
            "LOGIN_SESSION\tlocal tty1 2026-11-01 20:00\n",
            "LOGIN_SESSION\tminimal tty2\n",
            "LOGIN_SESSION\tsource-only pts/4 (:0)\n",
            "USER\tkang\t99\n",
        ))
        .unwrap()
        .stats;
        assert_eq!(stats.users_status, super::LoggedInUsersStatus::Available);
        assert_eq!(stats.login_sessions.len(), 7);
        assert_eq!(
            stats
                .users
                .iter()
                .map(|user| (user.name.as_str(), user.sessions))
                .collect::<Vec<_>>(),
            [
                ("kang", 3),
                ("local", 1),
                ("minimal", 1),
                ("root", 1),
                ("source-only", 1)
            ]
        );
        let sessions = &stats.login_sessions;
        assert_eq!(
            (&*sessions[0].name, &*sessions[0].terminal),
            ("kang", "pts/1")
        );
        assert_eq!(sessions[0].login_time, "2026-11-01 22:37");
        assert_eq!(sessions[0].source.as_deref(), Some("172.16.0.46"));
        assert_eq!(sessions[1].login_time, "Nov  1 22:40");
        assert_eq!(sessions[1].source.as_deref(), Some("2001:db8::1"));
        assert_eq!(sessions[2].login_time, "Nov 01 2026 22:45");
        assert_eq!(sessions[2].source.as_deref(), Some("host.example"));
        assert_eq!(sessions[3].source, None);
        assert!(sessions[4].login_time.is_empty());
        assert_eq!(sessions[4].source, None);
        assert!(sessions[6].login_time.is_empty());
        assert_eq!(sessions[6].source.as_deref(), Some(":0"));
    }

    #[test]
    fn invalid_login_records_do_not_break_other_resource_readings() {
        use super::LoggedInUsersStatus;
        for records in [
            "USER\talice\tnot-a-number",
            "USER\talice\t0",
            "USER\talice\t+1",
            "USER\talice\t 1",
            "USER\t\t2",
            "USER\talice\t4294967295\nUSER\talice\t1",
            "USER\talice\t1\textra",
            "USERS_STATUS\tavailable",
            "USER\talice\t2\nUSERS_STATUS\tfailed",
            "LOGIN_SESSION\talice",
            "LOGIN_SESSION\talice pts/0 2026-11-01 23:37 (host",
            "LOGIN_SESSION\talice pts/0 2026-11-01 23:37 host)",
            "LOGIN_SESSION\talice pts/0 2026-11-01 23:37 (host\u{1b})",
            "LOGIN_SESSION\talice pts/0\nLOGIN_SESSION\tmalformed",
        ] {
            let parsed = parse_stats_output(&format!("USERS_STATUS\tavailable\n{records}\nMEMORY\t1\t2\t3\nSYSTEM\thost\t10\tLinux\tx86_64\n")).unwrap().stats;
            assert_eq!(
                parsed.users_status,
                LoggedInUsersStatus::Failed,
                "{records}"
            );
            assert!(parsed.users.is_empty());
            assert!(parsed.login_sessions.is_empty());
            assert_eq!(parsed.memory.available, 2);
            assert_eq!(parsed.system.hostname, "host");
        }
        let empty = parse_stats_output("USERS_STATUS\tavailable\n")
            .unwrap()
            .stats;
        assert_eq!(empty.users_status, LoggedInUsersStatus::Available);
        assert!(empty.users.is_empty());
        assert!(empty.login_sessions.is_empty());
        let missing_status = parse_stats_output("LOGIN_SESSION\talice pts/0\n")
            .unwrap()
            .stats;
        assert_eq!(missing_status.users_status, LoggedInUsersStatus::Failed);
        assert!(missing_status.login_sessions.is_empty());
        assert_eq!(
            parse_stats_output("").unwrap().stats.users_status,
            LoggedInUsersStatus::NotCollected
        );
    }

    /// Executes the actual remote script with an isolated PATH. Fixtures can
    /// replace who or omit it entirely without depending on the host's utmp.
    #[test]
    fn login_probe_is_opt_in_and_missing_failing_or_slow_who_preserves_stats() {
        use super::{LoggedInUsersStatus, RemoteStatsOptions};
        use std::path::{Path, PathBuf};
        use std::process::Command;

        fn script(path: &Path, body: &str) {
            std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        let shell = std::env::var_os("NYATERM_TEST_SH")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                if cfg!(windows) {
                    let output = Command::new("git").arg("--exec-path").output().unwrap();
                    assert!(output.status.success());
                    PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
                        .ancestors()
                        .nth(3)
                        .unwrap()
                        .join("usr/bin/sh.exe")
                } else {
                    PathBuf::from("/bin/sh")
                }
            });
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        for utility in [
            "sh", "uname", "awk", "rm", "mv", "sleep", "df", "tr", "cat", "getconf",
        ] {
            let output = Command::new(&shell)
                .env(
                    "PATH",
                    if cfg!(windows) {
                        shell.parent().unwrap().as_os_str().to_os_string()
                    } else {
                        std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into())
                    },
                )
                .args(["-c", "command -v \"$1\"", "probe", utility])
                .output()
                .unwrap();
            assert!(output.status.success(), "missing test utility {utility}");
            let executable = String::from_utf8(output.stdout).unwrap();
            script(
                &bin.join(utility),
                &format!(
                    "exec '{}' \"$@\"",
                    executable.trim().replace('\'', "'\"'\"'")
                ),
            );
        }
        let who = bin.join("who");
        let marker = dir.path().join("calls");
        let cases = [
            (
                false,
                Some(
                    "printf 'root pts/0 2026-11-01 23:37 (172.16.0.46)\\nalice pts/1 Nov  1 22:37 (2001:db8::1)\\nalice pts/2\\n'",
                ),
                LoggedInUsersStatus::NotCollected,
                0,
            ),
            (
                true,
                Some(
                    "printf 'root pts/0 2026-11-01 23:37 (172.16.0.46)\\nalice pts/1 Nov  1 22:37 (2001:db8::1)\\nalice pts/2\\n'",
                ),
                LoggedInUsersStatus::Available,
                2,
            ),
            (true, Some(":"), LoggedInUsersStatus::Available, 0),
            (true, None, LoggedInUsersStatus::Unavailable, 0),
            (true, Some("exit 1"), LoggedInUsersStatus::Failed, 0),
            (
                true,
                Some("printf 'malformed\\n'"),
                LoggedInUsersStatus::Failed,
                0,
            ),
            (true, Some("exec sleep 3"), LoggedInUsersStatus::Failed, 0),
        ];
        for (enabled, body, status, accounts) in cases {
            let _ = std::fs::remove_file(&who);
            let _ = std::fs::remove_file(&marker);
            if let Some(body) = body {
                script(
                    &who,
                    &format!("printf called >\"$NYATERM_WHO_CALLS\"\n{body}"),
                );
            }
            let output = Command::new(&shell)
                .args([
                    "-c",
                    &RemoteStatsOptions {
                        collect_logged_in_users: enabled,
                    }
                    .command(),
                ])
                .env("PATH", &bin)
                .env("TMPDIR", dir.path())
                .env("NYATERM_WHO_CALLS", &marker)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let text = String::from_utf8(output.stdout).unwrap();
            let stats = parse_stats_output(&text).unwrap().stats;
            assert_eq!(stats.users_status, status, "{text}");
            assert_eq!(stats.users.len(), accounts);
            assert!(
                text.contains("SYSTEM\t") && text.contains("MEMORY\t") && text.contains("CPU\t")
            );
            assert_eq!(marker.exists(), enabled && body.is_some());
            if accounts > 0 {
                assert_eq!(stats.login_sessions.len(), 3);
                assert_eq!(stats.login_sessions[0].login_time, "Nov  1 22:37");
                assert_eq!(
                    stats.login_sessions[0].source.as_deref(),
                    Some("2001:db8::1")
                );
                assert!(stats.login_sessions[1].login_time.is_empty());
                assert_eq!(
                    stats.login_sessions[2].source.as_deref(),
                    Some("172.16.0.46")
                );
                assert_eq!(stats.users[0].name, "alice");
                assert_eq!(stats.users[0].sessions, 2);
                assert_eq!(stats.users[1].name, "root");
            } else {
                assert!(stats.login_sessions.is_empty());
            }
            assert!(!std::fs::read_dir(dir.path()).unwrap().any(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("sysinfo.")
            }));
        }
    }

    #[test]
    fn first_sample_returns_warming_up() {
        let sampler = RemoteStatsSampler::default();
        let parsed =
            parse_stats_output(&snapshot_output(10, (100, 0, 0, 900), &[(0, 10, 90)])).unwrap();
        let stats = sampler.complete_snapshot("s1", parsed);

        assert_eq!(stats.cpu.usage, None);
        assert_eq!(stats.cpu.usage_source, CpuUsageSource::WarmingUp);
    }

    #[test]
    fn one_busy_core_on_32_cpus_is_about_three_percent() {
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
        sampler.complete_snapshot("s1", first);
        let second = parse_stats_output(&snapshot_output(11, (100, 0, 0, 6300), &cores2)).unwrap();
        let stats = sampler.complete_snapshot("s1", second);

        let usage = stats.cpu.usage.unwrap();
        assert!((usage - 3.125).abs() < 0.01, "{usage}");
    }

    #[test]
    fn aggregate_matches_weighted_core_usage() {
        let sampler = RemoteStatsSampler::default();
        let first = parse_stats_output(&snapshot_output(
            10,
            (100, 0, 0, 900),
            &[(0, 50, 450), (1, 50, 450)],
        ))
        .unwrap();
        sampler.complete_snapshot("s1", first);
        let second = parse_stats_output(&snapshot_output(
            11,
            (120, 0, 0, 980),
            &[(0, 60, 490), (1, 60, 490)],
        ))
        .unwrap();
        let stats = sampler.complete_snapshot("s1", second);

        assert_eq!(stats.cpu.usage_source, CpuUsageSource::Aggregate);
        assert!((stats.cpu.usage.unwrap() - 20.0).abs() < 0.01);
    }

    #[test]
    fn inconsistent_aggregate_uses_core_fallback() {
        let sampler = RemoteStatsSampler::default();
        let first = parse_stats_output(&snapshot_output(
            10,
            (100, 0, 0, 900),
            &[(0, 50, 450), (1, 50, 450)],
        ))
        .unwrap();
        sampler.complete_snapshot("s1", first);
        let second = parse_stats_output(&snapshot_output(
            11,
            (194, 0, 0, 906),
            &[(0, 55, 495), (1, 55, 495)],
        ))
        .unwrap();
        let stats = sampler.complete_snapshot("s1", second);

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

    #[test]
    fn counter_rollback_resets_baseline() {
        let sampler = RemoteStatsSampler::default();
        let first =
            parse_stats_output(&snapshot_output(10, (100, 0, 0, 900), &[(0, 100, 900)])).unwrap();
        sampler.complete_snapshot("s1", first);
        let second =
            parse_stats_output(&snapshot_output(11, (90, 0, 0, 950), &[(0, 90, 950)])).unwrap();
        let stats = sampler.complete_snapshot("s1", second);

        assert_eq!(stats.cpu.usage, None);
        assert_eq!(stats.cpu.usage_source, CpuUsageSource::WarmingUp);
    }

    #[test]
    fn uptime_rollback_detects_remote_reboot() {
        let sampler = RemoteStatsSampler::default();
        let first =
            parse_stats_output(&snapshot_output(10, (100, 0, 0, 900), &[(0, 100, 900)])).unwrap();
        sampler.complete_snapshot("s1", first);
        let second =
            parse_stats_output(&snapshot_output(5, (110, 0, 0, 990), &[(0, 110, 990)])).unwrap();
        let stats = sampler.complete_snapshot("s1", second);

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

    #[test]
    fn sparse_cpu_ids_keep_their_original_ids() {
        let sampler = RemoteStatsSampler::default();
        let first = parse_stats_output(&snapshot_output(
            10,
            (100, 0, 0, 900),
            &[(0, 50, 450), (2, 50, 450)],
        ))
        .unwrap();
        sampler.complete_snapshot("s1", first);
        let second = parse_stats_output(&snapshot_output(
            11,
            (120, 0, 0, 980),
            &[(0, 60, 490), (2, 60, 490)],
        ))
        .unwrap();
        let stats = sampler.complete_snapshot("s1", second);

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
