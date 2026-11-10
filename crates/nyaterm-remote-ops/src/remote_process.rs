//! Remote process queries, parsing, and signal policy.
//!
//! Commands run through transport execution APIs without exposing SSH handlers.

use crate::host::{HostOs, HostTarget};
use nyaterm_transport::ssh_execution::{RemoteCommandOutput, ensure_remote_command_success};
use nyaterm_transport::{SshMultiplexHandle, SshSessionConfig};
use std::time::Duration;

pub const PROCESS_LIST_UNSUPPORTED_MARKER: &str = "NYATERM_PROCESS_UNSUPPORTED";
pub const PROCESS_LIST_UNSUPPORTED_ERROR: &str =
    "Process listing is unsupported on this remote host";
pub(crate) const PROCESS_TIMEOUT: Duration = Duration::from_secs(10);
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteProcess {
    pub pid: u32,
    pub ppid: u32,
    pub user: String,
    pub state: String,
    pub cpu_percent: f64,
    pub memory_percent: f64,
    pub rss_kb: u64,
    pub vsz_kb: u64,
    pub elapsed: String,
    pub command: String,
    pub command_line: String,
}

#[derive(Debug, Clone)]
pub struct SshProcessService {
    target: HostTarget,
}

pub const PROCESS_LIST_SCRIPT: &str = r#"sh -s <<'NYATERM_PROCESS_SCRIPT'
LC_ALL=C
export LC_ALL

unsupported() {
  echo "NYATERM_PROCESS_UNSUPPORTED"
  exit 42
}

clean() {
  printf "%s" "$1" | tr "\011\012\015" "   "
}

emit() {
  pid=$(clean "$1")
  ppid=$(clean "$2")
  user=$(clean "$3")
  stat=$(clean "$4")
  cpu=$(clean "$5")
  mem=$(clean "$6")
  rss=$(clean "$7")
  vsz=$(clean "$8")
  etime=$(clean "$9")
  comm=$(clean "${10}")
  args=$(clean "${11}")

  [ -n "$pid" ] || return 0
  [ -n "$ppid" ] || ppid=0
  [ -n "$user" ] || user=-
  [ -n "$stat" ] || stat=-
  [ -n "$cpu" ] || cpu=0
  [ -n "$mem" ] || mem=0
  [ -n "$rss" ] || rss=0
  [ -n "$vsz" ] || vsz=0
  [ -n "$etime" ] || etime=-
  [ -n "$comm" ] || comm=-
  [ -n "$args" ] || args=$comm

  printf "PROCESS\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n" \
    "$pid" "$ppid" "$user" "$stat" "$cpu" "$mem" "$rss" "$vsz" "$etime" "$comm" "$args"
}

parse_ps_full() {
  awk '
  function clean(value) { gsub(/[\t\r\n]/, " ", value); return value }
  NF >= 10 && $1 ~ /^[0-9]+$/ {
    args = ""
    for (i = 11; i <= NF; i++) args = args (args == "" ? "" : " ") $i
    if (args == "") args = $10
    printf "PROCESS\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n", \
      clean($1), clean($2), clean($3), clean($4), clean($5), clean($6), \
      clean($7), clean($8), clean($9), clean($10), clean(args)
  }'
}

parse_ps_basic() {
  awk '
  function clean(value) { gsub(/[\t\r\n]/, " ", value); return value }
  NR == 1 && toupper($1) == "PID" { next }
  NF >= 6 && $1 ~ /^[0-9]+$/ {
    args = ""
    for (i = 7; i <= NF; i++) args = args (args == "" ? "" : " ") $i
    if (args == "") args = $6
    printf "PROCESS\t%s\t%s\t%s\t%s\t0\t0\t0\t%s\t-\t%s\t%s\n", \
      clean($1), clean($2), clean($3), clean($4), clean($5), clean($6), clean(args)
  }'
}

parse_ps_minimal() {
  awk '
  function clean(value) { gsub(/[\t\r\n]/, " ", value); return value }
  NR == 1 && toupper($1) == "PID" { next }
  $1 ~ /^[0-9]+$/ {
    pid = $1; ppid = 0; user = "-"; stat = "-"; vsz = 0; start = 2
    if ($2 ~ /^[0-9]+$/) {
      ppid = $2; start = 3
      if (NF >= 3 && $3 !~ /^[0-9]+$/) { user = $3; start = 4 }
    } else if (NF >= 2) {
      user = $2; start = 3
    }
    if (NF >= start && $(start) ~ /^[0-9]+$/ && NF >= start + 1 && $(start + 1) ~ /^[A-Za-z]/) {
      vsz = $(start); stat = $(start + 1); start += 2
    } else if (NF >= start && $(start) ~ /^[A-Za-z][A-Za-z+<NsSlL]*$/) {
      stat = $(start); start += 1
    }
    args = ""
    for (i = start; i <= NF; i++) args = args (args == "" ? "" : " ") $i
    if (args == "") args = "-"
    comm = args; sub(/[ ].*$/, "", comm)
    printf "PROCESS\t%s\t%s\t%s\t%s\t0\t0\t0\t%s\t-\t%s\t%s\n", \
      clean(pid), clean(ppid), clean(user), clean(stat), clean(vsz), clean(comm), clean(args)
  }'
}

emit_proc() {
  [ -d /proc ] || return 1
  found=0
  mem_total=$(awk '/^MemTotal:/ { print $2; exit }' /proc/meminfo 2>/dev/null)
  [ -n "$mem_total" ] || mem_total=0

  for proc_dir in /proc/[0-9]*; do
    [ -r "$proc_dir/status" ] || continue
    pid=${proc_dir##*/}
    case "$pid" in *[!0-9]*|"") continue ;; esac
    status=$(awk '
      /^Name:/ { name=$2 }
      /^State:/ { state=$2 }
      /^PPid:/ { ppid=$2 }
      /^Uid:/ { uid=$2 }
      /^VmRSS:/ { rss=$2 }
      /^VmSize:/ { vsz=$2 }
      END {
        if (name == "") name="-"; if (state == "") state="-"; if (ppid == "") ppid=0
        if (uid == "") uid=0; if (rss == "") rss=0; if (vsz == "") vsz=0
        printf "%s\t%s\t%s\t%s\t%s\t%s\n", name, state, ppid, uid, rss, vsz
      }' "$proc_dir/status" 2>/dev/null)
    [ -n "$status" ] || continue
    old_ifs=$IFS; IFS="	"; set -- $status; IFS=$old_ifs
    comm=$1; stat=$2; ppid=$3; uid=$4; rss=$5; vsz=$6; user=$uid
    if [ -r /etc/passwd ]; then
      resolved_user=$(awk -F: -v uid="$uid" '$3 == uid { print $1; exit }' /etc/passwd 2>/dev/null)
      [ -n "$resolved_user" ] && user=$resolved_user
    fi
    if [ -r "$proc_dir/cmdline" ]; then
      args=$(tr "\000" " " <"$proc_dir/cmdline" 2>/dev/null)
    else
      args=
    fi
    [ -n "$args" ] || args=$comm
    mem=$(awk -v rss="$rss" -v total="$mem_total" 'BEGIN { if (total > 0) printf "%.1f", (rss * 100) / total; else printf "0"; }')
    emit "$pid" "$ppid" "$user" "$stat" "0" "$mem" "$rss" "$vsz" "-" "$comm" "$args"
    found=1
  done
  [ "$found" -eq 1 ]
}

if command -v ps >/dev/null 2>&1; then
  rows=$(ps -eo pid=,ppid=,user=,stat=,pcpu=,pmem=,rss=,vsz=,etime=,comm=,args= --no-headers 2>/dev/null | parse_ps_full)
  [ -n "$rows" ] && { printf "%s\n" "$rows"; exit 0; }
  rows=$(ps -axo pid=,ppid=,user=,stat=,pcpu=,pmem=,rss=,vsz=,etime=,comm=,command= 2>/dev/null | parse_ps_full)
  [ -n "$rows" ] && { printf "%s\n" "$rows"; exit 0; }
  rows=$(ps -o pid,ppid,user,stat,vsz,comm,args 2>/dev/null | parse_ps_basic)
  [ -n "$rows" ] && { printf "%s\n" "$rows"; exit 0; }
  rows=$(ps w 2>/dev/null | parse_ps_minimal)
  [ -n "$rows" ] && { printf "%s\n" "$rows"; exit 0; }
  rows=$(ps 2>/dev/null | parse_ps_minimal)
  [ -n "$rows" ] && { printf "%s\n" "$rows"; exit 0; }
fi

if emit_proc; then
  exit 0
fi

unsupported
NYATERM_PROCESS_SCRIPT
"#;
impl SshProcessService {
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

    fn exec_command_bytes(
        &self,
        command: Vec<u8>,
        timeout: Duration,
    ) -> anyhow::Result<RemoteCommandOutput> {
        self.target.run_raw(
            String::from_utf8_lossy(&command).into_owned(),
            None,
            timeout,
        )
    }

    /// Windows has no `ps`: the local machine is read natively instead.
    fn native_windows(&self) -> bool {
        self.target.is_local() && matches!(HostOs::local(), HostOs::Windows)
    }

    pub fn list_processes(&self) -> anyhow::Result<Vec<RemoteProcess>> {
        if self.native_windows() {
            return Ok(crate::local_process::list_local_processes());
        }
        #[cfg(unix)]
        if self.target.is_local()
            && let Ok(rows) = crate::local_process::list_local_unix_processes()
        {
            // BusyBox and other minimal `ps` builds fall through to the script.
            return Ok(rows);
        }
        self.target.ensure_probes_allowed()?;
        let output =
            self.exec_command_bytes(PROCESS_LIST_SCRIPT.as_bytes().to_vec(), PROCESS_TIMEOUT)?;
        if is_process_list_unsupported(&output.stdout)
            || is_process_list_unsupported(&output.stderr)
        {
            anyhow::bail!(PROCESS_LIST_UNSUPPORTED_ERROR);
        }
        let output = ensure_remote_command_success(output, "Failed to list processes")?;
        if is_process_list_unsupported(&output.stdout)
            || is_process_list_unsupported(&output.stderr)
        {
            anyhow::bail!(PROCESS_LIST_UNSUPPORTED_ERROR);
        }
        Ok(parse_process_output(&output.stdout))
    }

    pub fn signal_process(
        &self,
        pid: u32,
        signal: impl AsRef<str>,
    ) -> anyhow::Result<RemoteCommandOutput> {
        self.target.ensure_probes_allowed()?;
        let signal = normalize_process_signal(signal.as_ref())?;
        crate::process_control::ProcessControlCapabilities::for_os(self.target.os_hint())
            .ensure_signal(signal)?;
        anyhow::ensure!(pid != 0, "process PID must be nonzero");
        if self.native_windows() {
            crate::local_process::kill_local_process(pid, signal == "KILL")?;
            return Ok(RemoteCommandOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_status: Some(0),
            });
        }
        let output = self.exec_command_bytes(
            format!("kill -{signal} {pid}").into_bytes(),
            PROCESS_TIMEOUT,
        )?;
        ensure_remote_command_success(output, "Failed to signal process")
    }

    pub fn renice_process(&self, pid: u32, nice: i32) -> anyhow::Result<RemoteCommandOutput> {
        self.target.ensure_probes_allowed()?;
        if !crate::process_control::ProcessControlCapabilities::for_os(self.target.os_hint())
            .supports_priority()
        {
            return Err(crate::process_control::UnsupportedProcessControl.into());
        }
        if !(-20..=19).contains(&nice) {
            anyhow::bail!("Nice value must be between -20 and 19");
        }
        let output = self.exec_command_bytes(
            format!("renice -n {nice} -p {pid}").into_bytes(),
            PROCESS_TIMEOUT,
        )?;
        ensure_remote_command_success(output, "Failed to renice process")
    }

    pub fn run_command(
        &self,
        command: impl AsRef<str>,
        timeout: Duration,
    ) -> anyhow::Result<RemoteCommandOutput> {
        self.exec_command_bytes(command.as_ref().as_bytes().to_vec(), timeout)
    }
}

pub fn normalize_process_signal(signal: &str) -> anyhow::Result<&'static str> {
    match signal.trim().to_ascii_uppercase().as_str() {
        "TERM" | "SIGTERM" | "15" => Ok("TERM"),
        "KILL" | "SIGKILL" | "9" => Ok("KILL"),
        "HUP" | "SIGHUP" | "1" => Ok("HUP"),
        "STOP" | "SIGSTOP" | "19" => Ok("STOP"),
        "CONT" | "SIGCONT" | "18" => Ok("CONT"),
        _ => anyhow::bail!("Unsupported process signal"),
    }
}

pub fn is_process_list_unsupported(output: &str) -> bool {
    output
        .lines()
        .any(|line| line.trim() == PROCESS_LIST_UNSUPPORTED_MARKER)
}

pub fn parse_process_output(output: &str) -> Vec<RemoteProcess> {
    output
        .lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() < 12 || cols[0] != "PROCESS" {
                return None;
            }

            Some(RemoteProcess {
                pid: cols[1].parse().ok()?,
                ppid: cols[2].parse().unwrap_or(0),
                user: cols[3].to_string(),
                state: cols[4].to_string(),
                cpu_percent: cols[5].parse().unwrap_or(0.0),
                memory_percent: cols[6].parse().unwrap_or(0.0),
                rss_kb: cols[7].parse().unwrap_or(0),
                vsz_kb: cols[8].parse().unwrap_or(0),
                elapsed: cols[9].to_string(),
                command: cols[10].to_string(),
                command_line: cols[11..].join("\t"),
            })
        })
        .collect()
}
