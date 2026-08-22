//! SSH command execution and remote process management.
//!
//! The crate root re-exports the public service and models. Keeping command
//! collection, timeouts, process parsing and signal policy together prevents
//! the other remote services from depending on SSH channel internals.

use std::future::Future;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use russh::{ChannelMsg, Disconnect, client};

use super::{
    SshClientHandler, SshMultiplexHandle, SshSessionConfig, open_authenticated_ssh_handle,
};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_status: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct SshProcessService {
    config: SshSessionConfig,
    multiplex: Option<SshMultiplexHandle>,
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

    fn exec_command_bytes(
        &self,
        command: Vec<u8>,
        timeout: Duration,
    ) -> anyhow::Result<RemoteCommandOutput> {
        if let Some(multiplex) = self.multiplex.clone() {
            return multiplex.block_on_after_interactive_ready(exec_ssh_command_with_multiplex(
                multiplex.clone(),
                command,
                timeout,
            ));
        }
        run_ssh_exec_operation(exec_ssh_command(self.config.clone(), command, timeout))
    }

    pub fn list_processes(&self) -> anyhow::Result<Vec<RemoteProcess>> {
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
        let signal = normalize_process_signal(signal.as_ref())?;
        let output = self.exec_command_bytes(
            format!("kill -{signal} -- {pid}").into_bytes(),
            PROCESS_TIMEOUT,
        )?;
        ensure_remote_command_success(output, "Failed to signal process")
    }

    pub fn renice_process(&self, pid: u32, nice: i32) -> anyhow::Result<RemoteCommandOutput> {
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

pub fn run_local_command(
    command: impl AsRef<str>,
    cwd: Option<PathBuf>,
    timeout: Duration,
) -> anyhow::Result<RemoteCommandOutput> {
    let command = command.as_ref().to_string();
    run_ssh_exec_operation(async move {
        tokio::time::timeout(timeout, async move {
            let mut child = local_shell_command(&command);
            if let Some(cwd) = cwd.filter(|value| !value.as_os_str().is_empty()) {
                child.current_dir(cwd);
            }
            child.kill_on_drop(true);
            child.stdout(Stdio::piped());
            child.stderr(Stdio::piped());
            let output = child
                .output()
                .await
                .map_err(|error| anyhow::anyhow!("failed to run local command: {error}"))?;
            Ok(RemoteCommandOutput {
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                exit_status: output
                    .status
                    .code()
                    .and_then(|code| u32::try_from(code).ok()),
            })
        })
        .await
        .map_err(|_| anyhow::anyhow!("local command timed out"))?
    })
}

#[cfg(windows)]
fn local_shell_command(command: &str) -> tokio::process::Command {
    let mut child = tokio::process::Command::new("cmd");
    child.args(["/C", command]);
    child
}

#[cfg(not(windows))]
fn local_shell_command(command: &str) -> tokio::process::Command {
    let mut child = tokio::process::Command::new("sh");
    child.args(["-lc", command]);
    child
}
pub(crate) fn run_ssh_exec_operation<T, F>(operation: F) -> anyhow::Result<T>
where
    T: Send + 'static,
    F: Future<Output = anyhow::Result<T>> + Send + 'static,
{
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("nyaterm-ssh-exec")
        .build()
        .map_err(|error| anyhow::anyhow!("failed to start SSH exec runtime: {error}"))?;
    runtime.block_on(operation)
}

pub(crate) async fn exec_ssh_command(
    config: SshSessionConfig,
    command: Vec<u8>,
    timeout: Duration,
) -> anyhow::Result<RemoteCommandOutput> {
    tokio::time::timeout(timeout, async move {
        let (handle, jump_handles) = open_authenticated_ssh_handle(&config).await?;
        let channel = open_exec_channel_on_handle(&handle, command).await?;
        let output = collect_exec_channel(channel).await?;
        let _ = handle
            .disconnect(Disconnect::ByApplication, "ssh exec completed", "en")
            .await;
        for jump_handle in jump_handles {
            let _ = jump_handle
                .disconnect(Disconnect::ByApplication, "ssh exec completed", "en")
                .await;
        }

        Ok(output)
    })
    .await
    .map_err(|_| anyhow::anyhow!("remote command timed out"))?
}

async fn exec_ssh_command_with_multiplex(
    multiplex: SshMultiplexHandle,
    command: Vec<u8>,
    timeout: Duration,
) -> anyhow::Result<RemoteCommandOutput> {
    tokio::time::timeout(timeout, async move {
        let handle = multiplex.target_handle();
        let channel = {
            let handle = handle.lock().await;
            open_exec_channel_on_handle(&handle, command).await?
        };
        collect_exec_channel(channel).await
    })
    .await
    .map_err(|_| anyhow::anyhow!("remote command timed out"))?
}

async fn open_exec_channel_on_handle(
    handle: &client::Handle<SshClientHandler>,
    command: Vec<u8>,
) -> anyhow::Result<russh::Channel<client::Msg>> {
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|error| anyhow::anyhow!("failed to open exec channel: {error}"))?;
    channel
        .exec(true, command)
        .await
        .map_err(|error| anyhow::anyhow!("failed to execute remote command: {error}"))?;
    Ok(channel)
}

async fn collect_exec_channel(
    mut channel: russh::Channel<client::Msg>,
) -> anyhow::Result<RemoteCommandOutput> {
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut exit_status = None;
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Data { data }) => {
                stdout.push_str(&String::from_utf8_lossy(&data));
            }
            Some(ChannelMsg::ExtendedData { data, .. }) => {
                stderr.push_str(&String::from_utf8_lossy(&data));
            }
            Some(ChannelMsg::ExitStatus {
                exit_status: status,
            }) => {
                exit_status = Some(status);
            }
            Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
            Some(_) => {}
        }
    }

    let _ = channel.close().await;
    Ok(RemoteCommandOutput {
        stdout,
        stderr,
        exit_status,
    })
}

pub(crate) fn ensure_remote_command_success(
    output: RemoteCommandOutput,
    context: &str,
) -> anyhow::Result<RemoteCommandOutput> {
    if matches!(output.exit_status, Some(0) | None) {
        return Ok(output);
    }

    let stderr = output.stderr.trim();
    let stdout = output.stdout.trim();
    let detail = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        "remote command failed"
    };

    anyhow::bail!("{context}: {detail}")
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
