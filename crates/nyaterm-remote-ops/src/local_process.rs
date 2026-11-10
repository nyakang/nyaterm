//! Native process table for the local machine, used where there is no `ps`
//! (Windows). Unix hosts keep the `ps` backend, which reports the same columns
//! with less work.

use crate::remote_process::RemoteProcess;
use std::time::Duration;
use sysinfo::{
    MINIMUM_CPU_UPDATE_INTERVAL, Pid, ProcessRefreshKind, ProcessesToUpdate, Signal, System,
    UpdateKind, Users,
};

fn refresh_kind() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing()
        .with_cpu()
        .with_memory()
        .with_user(UpdateKind::OnlyIfNotSet)
        .with_cmd(UpdateKind::OnlyIfNotSet)
}

fn format_elapsed(seconds: u64) -> String {
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (hours, rest) = (rest / 3600, rest % 3600);
    let (minutes, secs) = (rest / 60, rest % 60);
    if days > 0 {
        format!("{days}-{hours:02}:{minutes:02}:{secs:02}")
    } else if hours > 0 {
        format!("{hours:02}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes:02}:{secs:02}")
    }
}

/// Local Unix hosts: one `ps` spawn, parsed here. The generic script used for
/// SSH wraps `ps` in `sh` + `awk` (and probes several dialects), which roughly
/// doubled the cost locally.
#[cfg(unix)]
pub fn list_local_unix_processes() -> anyhow::Result<Vec<RemoteProcess>> {
    let output = std::process::Command::new("ps")
        .args([
            "axww",
            "-o",
            "pid=,ppid=,user=,stat=,pcpu=,pmem=,rss=,vsz=,etime=,command=",
        ])
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()?;
    anyhow::ensure!(output.status.success(), "ps failed");
    let rows = parse_ps_rows(&String::from_utf8_lossy(&output.stdout));
    anyhow::ensure!(!rows.is_empty(), "ps returned no processes");
    Ok(rows)
}

/// A short display name from the command line. App bundles on macOS contain
/// spaces ("Google Chrome Helper"), so for those the name ends at the first
/// flag; otherwise it is the first token.
fn command_name(command_line: &str) -> String {
    let base = if command_line.starts_with('/') && command_line.contains(".app/") {
        command_line
            .split_once(" -")
            .map_or(command_line, |(head, _)| head)
    } else {
        command_line
            .split_whitespace()
            .next()
            .unwrap_or(command_line)
    };
    base.rsplit('/').next().unwrap_or(base).to_string()
}

pub fn parse_ps_rows(output: &str) -> Vec<RemoteProcess> {
    output
        .lines()
        .filter_map(|line| {
            let mut rest = line.trim_start();
            let mut fields = [""; 9];
            for field in &mut fields {
                let end = rest.find(char::is_whitespace)?;
                *field = &rest[..end];
                rest = rest[end..].trim_start();
            }
            let command_line = rest.trim_end().to_string();
            Some(RemoteProcess {
                pid: fields[0].parse().ok()?,
                ppid: fields[1].parse().unwrap_or(0),
                user: fields[2].to_string(),
                state: fields[3].to_string(),
                cpu_percent: fields[4].parse().unwrap_or(0.0),
                memory_percent: fields[5].parse().unwrap_or(0.0),
                rss_kb: fields[6].parse().unwrap_or(0),
                vsz_kb: fields[7].parse().unwrap_or(0),
                elapsed: fields[8].to_string(),
                command: command_name(&command_line),
                command_line,
            })
        })
        .collect()
}

/// CPU percentages need two refreshes a minimum interval apart; the wait happens
/// on the job thread, never in the UI.
pub fn list_local_processes() -> Vec<RemoteProcess> {
    let mut system = System::new();
    system.refresh_memory();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind());
    std::thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL.max(Duration::from_millis(250)));
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind());
    system.refresh_cpu_usage();

    let users = Users::new_with_refreshed_list();
    let total_memory = system.total_memory().max(1);
    let cores = system.cpus().len().max(1) as f64;
    let mut processes: Vec<RemoteProcess> = system
        .processes()
        .values()
        .map(|process| {
            let command = process.name().to_string_lossy().into_owned();
            let command_line = process
                .cmd()
                .iter()
                .map(|part| part.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            RemoteProcess {
                pid: process.pid().as_u32(),
                ppid: process.parent().map(Pid::as_u32).unwrap_or(0),
                user: process
                    .user_id()
                    .and_then(|id| users.get_user_by_id(id))
                    .map(|user| user.name().to_string())
                    .unwrap_or_else(|| "-".to_string()),
                state: process.status().to_string(),
                // sysinfo reports percent of one core; ps reports percent of all.
                cpu_percent: f64::from(process.cpu_usage()) / cores,
                memory_percent: process.memory() as f64 * 100.0 / total_memory as f64,
                rss_kb: process.memory() / 1024,
                vsz_kb: process.virtual_memory() / 1024,
                elapsed: format_elapsed(process.run_time()),
                command_line: if command_line.is_empty() {
                    command.clone()
                } else {
                    command_line
                },
                command,
            }
        })
        .collect();
    processes.sort_by_key(|process| process.pid);
    processes
}

/// Terminate a local process. `kill` is a hard stop on Windows, which has no
/// signal model; other signals are not offered there.
pub fn kill_local_process(pid: u32, force: bool) -> anyhow::Result<()> {
    #[cfg(windows)]
    if !force {
        return Err(crate::process_control::UnsupportedProcessControl.into());
    }
    let mut system = System::new();
    let pid = Pid::from_u32(pid);
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing(),
    );
    let process = system
        .process(pid)
        .ok_or_else(|| anyhow::anyhow!("process {} no longer exists", pid.as_u32()))?;
    let sent = if force {
        process.kill()
    } else {
        process
            .kill_with(Signal::Term)
            .unwrap_or_else(|| process.kill())
    };
    if sent {
        Ok(())
    } else {
        anyhow::bail!("not permitted to stop process {}", pid.as_u32())
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::list_local_unix_processes;
    use super::{format_elapsed, list_local_processes, parse_ps_rows};

    #[test]
    fn native_table_contains_this_process_with_plausible_fields() {
        let processes = list_local_processes();
        let me = processes
            .iter()
            .find(|process| process.pid == std::process::id())
            .expect("own pid");
        assert!(me.rss_kb > 0);
        assert!(!me.command.is_empty());
        assert!(me.memory_percent > 0.0 && me.memory_percent < 100.0);
        assert!(processes.len() > 5);
    }

    #[test]
    fn ps_rows_keep_spaced_bundle_names_and_full_command_lines() {
        let rows = parse_ps_rows(
            "  1     0 root      Ss    0.0  0.1  1200  4000 10-02:03:04 /sbin/launchd\n\
             812   1 me        S     1.5  2.0  9000 99000    01:02 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome Helper --type=gpu --x=1\n\
             900   1 me        R     0.0  0.0   100   200       00:05 node server.js --port 80\n\
             bad line\n",
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].command, "launchd");
        assert_eq!(rows[0].elapsed, "10-02:03:04");
        assert_eq!(rows[1].command, "Google Chrome Helper");
        assert!(rows[1].command_line.ends_with("--x=1"));
        assert_eq!(rows[2].command, "node");
        assert_eq!(rows[2].rss_kb, 100);
    }

    #[cfg(unix)]
    #[test]
    fn direct_ps_lists_this_test_process() {
        let rows = list_local_unix_processes().expect("ps");
        let me = rows
            .iter()
            .find(|p| p.pid == std::process::id())
            .expect("own pid");
        assert!(me.rss_kb > 0 && !me.command.is_empty());
    }

    #[test]
    fn elapsed_formats_like_ps_etime() {
        assert_eq!(format_elapsed(65), "01:05");
        assert_eq!(format_elapsed(3_661), "01:01:01");
        assert_eq!(format_elapsed(90_061), "1-01:01:01");
    }
}
