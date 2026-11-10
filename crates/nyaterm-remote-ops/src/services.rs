//! System services on every supported host: systemd, launchd, FreeBSD rc,
//! OpenRC/SysV and Windows services, locally or over SSH.
//!
//! Listing is one batched script (one PowerShell call on Windows). Parsing is
//! pure and tested against captured output. Names are always shell-quoted and
//! validated before they reach a command line.

use crate::host::{Elevation, HostOs, HostTarget, sh_quote};

#[cfg(all(test, unix))]
mod regression_tests;
#[cfg(all(test, windows))]
mod windows_log_tests;
#[cfg(windows)]
mod windows_scm;
use anyhow::bail;
use std::collections::HashMap;
use std::time::Duration;

const SERVICES_TIMEOUT: Duration = Duration::from_secs(25);
const ACTION_TIMEOUT: Duration = Duration::from_secs(45);
pub const SERVICES_UNSUPPORTED_MARKER: &str = "NYATERM_SERVICES_UNSUPPORTED";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceBackend {
    Systemd,
    Launchd,
    FreeBsdRc,
    OpenRc,
    SysV,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ServiceState {
    Failed,
    Running,
    Transitioning,
    Stopped,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceStartup {
    Enabled,
    Disabled,
    /// Started on demand or as a dependency; cannot be enabled directly.
    Static,
    Masked,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceEntry {
    /// The identifier passed to the service manager.
    pub name: String,
    pub description: String,
    pub state: ServiceState,
    /// The manager's own state word (`running`, `dead`, `Start Pending`, ...).
    pub detail: String,
    pub startup: ServiceStartup,
    pub pid: Option<u32>,
    /// Launchd domain (`system`, `gui/501`); empty elsewhere.
    pub scope: String,
}

impl ServiceEntry {
    /// Scope is part of a launchd job's identity; labels may repeat across domains.
    pub fn key(&self) -> String {
        format!("{}|{}", self.scope, self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicesSnapshot {
    pub backend: ServiceBackend,
    pub entries: Vec<ServiceEntry>,
    /// The backend cannot show everything (launchd unloaded jobs, FreeBSD
    /// disabled scripts); the UI says so instead of implying completeness.
    pub partial: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceAction {
    Start,
    Stop,
    Restart,
    Enable,
    Disable,
}

impl ServiceAction {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Enable => "enable",
            Self::Disable => "disable",
        }
    }
}

impl ServiceBackend {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Systemd => "systemd",
            Self::Launchd => "launchd",
            Self::FreeBsdRc => "rc.d",
            Self::OpenRc => "OpenRC",
            Self::SysV => "SysV init",
            Self::Windows => "Windows services",
        }
    }
}

/// Reject names that could change the meaning of a command line even when quoted.
pub fn validate_service_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name.len() > 256 || name.chars().any(|c| c.is_control()) {
        bail!("invalid service name");
    }
    Ok(())
}

pub const SERVICES_SCRIPT: &str = r#"
if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
  echo "@@FORMAT systemd"
  echo "@@UNITS"
  systemctl list-units --type=service --all --no-legend --no-pager --plain 2>/dev/null
  echo "@@FILES"
  systemctl list-unit-files --type=service --no-legend --no-pager 2>/dev/null
  echo "@@PIDS"
  systemctl show '*.service' --property=Id,MainPID 2>/dev/null
elif [ "$(uname -s)" = "Darwin" ]; then
  echo "@@FORMAT launchd"
  uid=$(id -u)
  # sudo changes the effective user, but not the GUI domain being inspected.
  if [ "$uid" = 0 ]; then
    case "${SUDO_UID-}" in
      ''|*[!0-9]*) ;;
      *) uid=$SUDO_UID ;;
    esac
  fi
  for domain in system "gui/$uid"; do
    echo "@@DOMAIN $domain"
    launchctl print "$domain" 2>/dev/null | awk '/^\tservices = \{/{f=1;next} f&&/^\t\}/{f=0} f{print}'
    echo "@@DISABLED $domain"
    launchctl print-disabled "$domain" 2>/dev/null
  done
elif [ "$(uname -s)" = "FreeBSD" ] && command -v service >/dev/null 2>&1; then
  echo "@@FORMAT freebsd"
  enabled=$(service -e 2>/dev/null)
  for path in $enabled; do
    name=${path##*/}
    if service "$name" onestatus >/dev/null 2>&1; then echo "$name 1"; else echo "$name 0"; fi
  done
elif command -v rc-status >/dev/null 2>&1; then
  echo "@@FORMAT openrc"
  rc-status -a -C 2>/dev/null
elif command -v service >/dev/null 2>&1; then
  echo "@@FORMAT sysv"
  service --status-all 2>&1
else
  echo "NYATERM_SERVICES_UNSUPPORTED"
  exit 42
fi
"#;

pub const WINDOWS_SERVICES_SCRIPT: &str = r#"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
Get-CimInstance Win32_Service | ForEach-Object {
  $d = if ($_.Description) { ($_.Description -replace '[\t\r\n]+', ' ') } else { '' }
  if ($d.Length -gt 240) { $d = $d.Substring(0, 240) }
  '{0}{6}{1}{6}{2}{6}{3}{6}{4}{6}{5}' -f $_.Name, $_.DisplayName, $_.State, $_.StartMode, $_.ProcessId, $d, "`t"
}
"#;

pub fn detect_backend(os: HostOs, output_format: &str) -> Option<ServiceBackend> {
    if matches!(os, HostOs::Windows) {
        return Some(ServiceBackend::Windows);
    }
    Some(match output_format {
        "systemd" => ServiceBackend::Systemd,
        "launchd" => ServiceBackend::Launchd,
        "freebsd" => ServiceBackend::FreeBsdRc,
        "openrc" => ServiceBackend::OpenRc,
        "sysv" => ServiceBackend::SysV,
        _ => return None,
    })
}

pub fn list_services(
    target: &HostTarget,
    os: HostOs,
    elevation: Elevation,
) -> anyhow::Result<ServicesSnapshot> {
    target.ensure_probes_allowed()?;
    #[cfg(windows)]
    if matches!(os, HostOs::Windows) && target.is_local() {
        return windows_scm::list().map(|entries| ServicesSnapshot {
            backend: ServiceBackend::Windows,
            entries,
            partial: false,
        });
    }
    if matches!(os, HostOs::Windows) {
        let output = target.run_powershell(WINDOWS_SERVICES_SCRIPT, SERVICES_TIMEOUT)?;
        if output.exit_status.is_some_and(|code| code != 0) && output.stdout.trim().is_empty() {
            bail!("{}", first_line(&output.stderr, "service listing failed"));
        }
        return Ok(parse_windows_output(&output.stdout));
    }
    let output = target.run_sh(SERVICES_SCRIPT, elevation, SERVICES_TIMEOUT)?;
    if output.stdout.contains(SERVICES_UNSUPPORTED_MARKER) {
        bail!("no supported service manager found on this host");
    }
    parse_unix_output(&output.stdout)
        .ok_or_else(|| anyhow::anyhow!("{}", first_line(&output.stderr, "service listing failed")))
}

fn first_line<'a>(text: &'a str, fallback: &'a str) -> &'a str {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or(fallback)
}

/// Split `@@NAME arg` section markers into `(name, arg) -> lines`.
type Section<'a> = (String, String, Vec<&'a str>);

fn sections(output: &str) -> (String, Vec<Section<'_>>) {
    let mut format = String::new();
    let mut parts: Vec<Section<'_>> = Vec::new();
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("@@") {
            let (name, arg) = rest.split_once(' ').unwrap_or((rest, ""));
            if name == "FORMAT" {
                format = arg.trim().to_string();
            } else {
                parts.push((name.to_string(), arg.trim().to_string(), Vec::new()));
            }
        } else if let Some((_, _, lines)) = parts.last_mut() {
            lines.push(line);
        } else if format.is_empty() {
            // Before the format marker: a sysv/openrc body has no sections.
        }
    }
    (format, parts)
}

pub fn parse_unix_output(output: &str) -> Option<ServicesSnapshot> {
    let (format, parts) = sections(output);
    let backend = detect_backend(HostOs::Linux, &format)?;
    let (entries, partial) = match backend {
        ServiceBackend::Systemd => (parse_systemd(&parts), false),
        ServiceBackend::Launchd => (parse_launchd(&parts), true),
        ServiceBackend::FreeBsdRc => (parse_freebsd(output), true),
        ServiceBackend::OpenRc => (parse_openrc(output), false),
        ServiceBackend::SysV => (parse_sysv(output), false),
        ServiceBackend::Windows => return None,
    };
    Some(ServicesSnapshot {
        backend,
        entries,
        partial,
    })
}

fn section<'a, 'b>(parts: &'b [(String, String, Vec<&'a str>)], name: &str) -> &'b [&'a str] {
    parts
        .iter()
        .find(|(n, _, _)| n == name)
        .map(|(_, _, lines)| lines.as_slice())
        .unwrap_or(&[])
}

fn parse_systemd(parts: &[Section<'_>]) -> Vec<ServiceEntry> {
    let mut startup: HashMap<&str, ServiceStartup> = HashMap::new();
    for line in section(parts, "FILES") {
        let mut cols = line.split_whitespace();
        let (Some(unit), Some(state)) = (cols.next(), cols.next()) else {
            continue;
        };
        startup.insert(
            unit,
            match state {
                "enabled" | "enabled-runtime" => ServiceStartup::Enabled,
                "disabled" => ServiceStartup::Disabled,
                "masked" | "masked-runtime" => ServiceStartup::Masked,
                "static" | "indirect" | "generated" | "transient" | "alias" => {
                    ServiceStartup::Static
                }
                _ => ServiceStartup::Unknown,
            },
        );
    }
    let mut pids: HashMap<&str, u32> = HashMap::new();
    let mut current: Option<&str> = None;
    for line in section(parts, "PIDS") {
        if let Some(id) = line.strip_prefix("Id=") {
            current = Some(id.trim());
        } else if let (Some(id), Some(pid)) = (current, line.strip_prefix("MainPID="))
            && let Ok(pid) = pid.trim().parse::<u32>()
            && pid > 0
        {
            pids.insert(id, pid);
        }
    }
    let mut entries = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in section(parts, "UNITS") {
        let line = line.trim_start_matches(['●', '*', '○', '×', ' ']);
        let mut cols = line.split_whitespace();
        let (Some(unit), Some(load), Some(active), Some(sub)) =
            (cols.next(), cols.next(), cols.next(), cols.next())
        else {
            continue;
        };
        if !unit.ends_with(".service") {
            continue;
        }
        let description = cols.collect::<Vec<_>>().join(" ");
        let state = match active {
            "active" if sub == "exited" => ServiceState::Stopped,
            "active" | "reloading" => ServiceState::Running,
            "failed" => ServiceState::Failed,
            "activating" | "deactivating" | "maintenance" => ServiceState::Transitioning,
            "inactive" => ServiceState::Stopped,
            _ => ServiceState::Unknown,
        };
        let name = unit.trim_end_matches(".service").to_string();
        seen.insert(unit);
        entries.push(ServiceEntry {
            description: if load == "not-found" {
                String::new()
            } else {
                description
            },
            detail: if active == "active" && sub == "exited" {
                "exited".to_string()
            } else {
                sub.to_string()
            },
            startup: startup
                .get(unit)
                .copied()
                .unwrap_or(ServiceStartup::Unknown),
            pid: pids.get(unit).copied(),
            scope: String::new(),
            state,
            name,
        });
    }
    // Installed-but-never-loaded units only appear in the unit-file list.
    for (unit, startup) in &startup {
        if seen.contains(unit) || !unit.ends_with(".service") || unit.contains("@.") {
            continue;
        }
        entries.push(ServiceEntry {
            name: unit.trim_end_matches(".service").to_string(),
            description: String::new(),
            state: ServiceState::Stopped,
            detail: "inactive".to_string(),
            startup: *startup,
            pid: None,
            scope: String::new(),
        });
    }
    sort_by_name(&mut entries);
    entries
}

fn parse_launchd(parts: &[Section<'_>]) -> Vec<ServiceEntry> {
    let mut entries = Vec::new();
    for (name, domain, lines) in parts {
        if name != "DOMAIN" {
            continue;
        }
        let mut disabled: HashMap<String, bool> = HashMap::new();
        if let Some((_, _, rows)) = parts
            .iter()
            .find(|(n, d, _)| n == "DISABLED" && d == domain)
        {
            for row in rows {
                if let Some((label, state)) = row.split_once("=>") {
                    disabled.insert(
                        label.trim().trim_matches('"').to_string(),
                        state.trim() == "disabled",
                    );
                }
            }
        }
        for line in lines {
            let mut cols = line.split_whitespace();
            let (Some(pid), Some(status), Some(label)) = (cols.next(), cols.next(), cols.next())
            else {
                continue;
            };
            let Ok(pid) = pid.parse::<u32>() else {
                continue;
            };
            let exit: Option<i32> = status.parse().ok();
            let (state, detail) = if pid > 0 {
                (ServiceState::Running, "running".to_string())
            } else if let Some(code) = exit.filter(|code| *code != 0) {
                (ServiceState::Stopped, format!("last exit {code}"))
            } else {
                (ServiceState::Stopped, "idle".to_string())
            };
            entries.push(ServiceEntry {
                name: label.to_string(),
                description: String::new(),
                state,
                detail,
                startup: match disabled.get(label) {
                    Some(true) => ServiceStartup::Disabled,
                    _ => ServiceStartup::Enabled,
                },
                pid: (pid > 0).then_some(pid),
                scope: domain.clone(),
            });
        }
    }
    sort_by_name(&mut entries);
    entries
}

fn parse_freebsd(output: &str) -> Vec<ServiceEntry> {
    let mut entries: Vec<ServiceEntry> = output
        .lines()
        .filter(|line| !line.starts_with("@@"))
        .filter_map(|line| {
            let (name, running) = line.trim().rsplit_once(' ')?;
            let running = running == "1";
            Some(ServiceEntry {
                name: name.to_string(),
                description: String::new(),
                state: if running {
                    ServiceState::Running
                } else {
                    ServiceState::Stopped
                },
                detail: if running { "running" } else { "stopped" }.to_string(),
                startup: ServiceStartup::Enabled,
                pid: None,
                scope: String::new(),
            })
        })
        .collect();
    sort_by_name(&mut entries);
    entries
}

fn parse_openrc(output: &str) -> Vec<ServiceEntry> {
    let mut entries = Vec::new();
    for line in output.lines() {
        let Some((name, status)) = line.split_once('[') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() || line.starts_with("Runlevel") {
            continue;
        }
        let status = status.trim_end_matches(']').trim().to_ascii_lowercase();
        let state = match status.as_str() {
            "started" => ServiceState::Running,
            "stopped" => ServiceState::Stopped,
            "crashed" => ServiceState::Failed,
            "starting" | "stopping" => ServiceState::Transitioning,
            _ => ServiceState::Unknown,
        };
        entries.push(ServiceEntry {
            name: name.to_string(),
            description: String::new(),
            state,
            detail: status,
            startup: ServiceStartup::Unknown,
            pid: None,
            scope: String::new(),
        });
    }
    sort_by_name(&mut entries);
    entries
}

fn parse_sysv(output: &str) -> Vec<ServiceEntry> {
    let mut entries = Vec::new();
    for line in output.lines() {
        let Some(rest) = line.trim().strip_prefix('[') else {
            continue;
        };
        let Some((mark, name)) = rest.split_once(']') else {
            continue;
        };
        let state = match mark.trim() {
            "+" => ServiceState::Running,
            "-" => ServiceState::Stopped,
            _ => ServiceState::Unknown,
        };
        entries.push(ServiceEntry {
            name: name.trim().to_string(),
            description: String::new(),
            state,
            detail: String::new(),
            startup: ServiceStartup::Unknown,
            pid: None,
            scope: String::new(),
        });
    }
    sort_by_name(&mut entries);
    entries
}

pub fn parse_windows_output(output: &str) -> ServicesSnapshot {
    let mut entries: Vec<ServiceEntry> = output
        .lines()
        .filter_map(|line| {
            let mut cols = line.trim_end_matches('\r').split('\t');
            let name = cols.next()?.trim();
            if name.is_empty() {
                return None;
            }
            let display = cols.next().unwrap_or("").trim();
            let state_word = cols.next().unwrap_or("").trim();
            let mode = cols.next().unwrap_or("").trim();
            let pid = cols.next().and_then(|v| v.trim().parse::<u32>().ok());
            let description = cols.next().unwrap_or("").trim();
            let state = match state_word {
                "Running" => ServiceState::Running,
                "Stopped" => ServiceState::Stopped,
                "Paused" => ServiceState::Stopped,
                "Start Pending" | "Stop Pending" | "Continue Pending" | "Pause Pending" => {
                    ServiceState::Transitioning
                }
                _ => ServiceState::Unknown,
            };
            Some(ServiceEntry {
                name: name.to_string(),
                description: if description.is_empty() {
                    display.to_string()
                } else if display.is_empty() || display == name {
                    description.to_string()
                } else {
                    format!("{display} — {description}")
                },
                state,
                detail: state_word.to_ascii_lowercase(),
                startup: match mode {
                    "Auto" => ServiceStartup::Enabled,
                    "Manual" => ServiceStartup::Static,
                    "Disabled" => ServiceStartup::Disabled,
                    _ => ServiceStartup::Unknown,
                },
                pid: pid.filter(|pid| *pid > 0),
                scope: String::new(),
            })
        })
        .collect();
    sort_by_name(&mut entries);
    ServicesSnapshot {
        backend: ServiceBackend::Windows,
        entries,
        partial: false,
    }
}

fn sort_by_name(entries: &mut [ServiceEntry]) {
    entries.sort_by_cached_key(|entry| entry.name.to_ascii_lowercase());
}

/// The command line that performs `action`, for the given backend.
pub fn action_command(
    backend: ServiceBackend,
    entry: &ServiceEntry,
    action: ServiceAction,
) -> anyhow::Result<String> {
    validate_service_name(&entry.name)?;
    let name = sh_quote(&entry.name);
    Ok(match backend {
        ServiceBackend::Systemd => {
            let unit = sh_quote(&format!("{}.service", entry.name));
            match action {
                ServiceAction::Start => format!("systemctl start {unit}"),
                ServiceAction::Stop => format!("systemctl stop {unit}"),
                ServiceAction::Restart => format!("systemctl restart {unit}"),
                ServiceAction::Enable => format!("systemctl enable {unit}"),
                ServiceAction::Disable => format!("systemctl disable {unit}"),
            }
        }
        ServiceBackend::Launchd => {
            if entry.scope.is_empty() {
                bail!("launchd service has no domain");
            }
            let target = sh_quote(&format!("{}/{}", entry.scope, entry.name));
            match action {
                ServiceAction::Start => format!("launchctl kickstart {target}"),
                ServiceAction::Stop => format!("launchctl kill SIGTERM {target}"),
                ServiceAction::Restart => format!("launchctl kickstart -k {target}"),
                ServiceAction::Enable => format!("launchctl enable {target}"),
                ServiceAction::Disable => format!("launchctl disable {target}"),
            }
        }
        ServiceBackend::FreeBsdRc => match action {
            ServiceAction::Start => format!("service {name} onestart"),
            ServiceAction::Stop => format!("service {name} onestop"),
            ServiceAction::Restart => format!("service {name} onerestart"),
            ServiceAction::Enable => format!("sysrc {}_enable=YES", sanitize_rc_var(&entry.name)?),
            ServiceAction::Disable => format!("sysrc {}_enable=NO", sanitize_rc_var(&entry.name)?),
        },
        ServiceBackend::OpenRc => match action {
            ServiceAction::Start => format!("rc-service {name} start"),
            ServiceAction::Stop => format!("rc-service {name} stop"),
            ServiceAction::Restart => format!("rc-service {name} restart"),
            ServiceAction::Enable => format!("rc-update add {name} default"),
            ServiceAction::Disable => format!("rc-update del {name} default"),
        },
        ServiceBackend::SysV => match action {
            ServiceAction::Start => format!("service {name} start"),
            ServiceAction::Stop => format!("service {name} stop"),
            ServiceAction::Restart => format!("service {name} restart"),
            ServiceAction::Enable | ServiceAction::Disable => {
                bail!("this host's init system cannot enable or disable services here")
            }
        },
        ServiceBackend::Windows => bail!("Windows actions use PowerShell"),
    })
}

fn sanitize_rc_var(name: &str) -> anyhow::Result<String> {
    if name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        Ok(name.replace('-', "_"))
    } else {
        bail!("service name cannot be used as an rc variable")
    }
}

fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

pub fn windows_action_script(
    entry: &ServiceEntry,
    action: ServiceAction,
) -> anyhow::Result<String> {
    validate_service_name(&entry.name)?;
    let name = powershell_quote(&entry.name);
    Ok(format!(
        "$ErrorActionPreference = 'Stop'\n{}",
        match action {
            ServiceAction::Start => format!("Start-Service -Name {name}"),
            ServiceAction::Stop => format!("Stop-Service -Name {name} -Force"),
            ServiceAction::Restart => format!("Restart-Service -Name {name} -Force"),
            ServiceAction::Enable => format!("Set-Service -Name {name} -StartupType Automatic"),
            ServiceAction::Disable => format!("Set-Service -Name {name} -StartupType Disabled"),
        }
    ))
}

/// Run an action. Returns the tool's own message on failure so the UI can show
/// the real reason (permission denied, unit masked, ...).
pub fn run_action(
    target: &HostTarget,
    os: HostOs,
    backend: ServiceBackend,
    entry: &ServiceEntry,
    action: ServiceAction,
    elevation: Elevation,
) -> anyhow::Result<()> {
    target.ensure_probes_allowed()?;
    #[cfg(windows)]
    if matches!(os, HostOs::Windows) && target.is_local() {
        validate_service_name(&entry.name)?;
        return windows_scm::run_action(&entry.name, action);
    }
    let output = if matches!(os, HostOs::Windows) {
        target.run_powershell(&windows_action_script(entry, action)?, ACTION_TIMEOUT)?
    } else {
        target.run_sh(
            &action_command(backend, entry, action)?,
            elevation,
            ACTION_TIMEOUT,
        )?
    };
    if output.exit_status.is_some_and(|code| code != 0) {
        let detail = if output.stderr.trim().is_empty() {
            output.stdout.as_str()
        } else {
            output.stderr.as_str()
        };
        bail!("{}", first_line(detail, "service action failed"));
    }
    Ok(())
}

/// Recent log lines (systemd/Windows) or the manager's own description.
pub fn fetch_logs(
    target: &HostTarget,
    os: HostOs,
    backend: ServiceBackend,
    entry: &ServiceEntry,
    lines: u32,
    elevation: Elevation,
) -> anyhow::Result<String> {
    target.ensure_probes_allowed()?;
    validate_service_name(&entry.name)?;
    let lines = lines.clamp(10, 2000);
    let output = match backend {
        _ if matches!(os, HostOs::Windows) => {
            target.run_powershell(&windows_logs_script(&entry.name, lines), ACTION_TIMEOUT)?
        }
        _ => {
            let command = match backend {
                ServiceBackend::Systemd => format!(
                    "journalctl -u {} -n {lines} --no-pager -o short-iso",
                    sh_quote(&format!("{}.service", entry.name))
                ),
                ServiceBackend::Launchd => format!(
                    "launchctl print {}",
                    sh_quote(&format!("{}/{}", entry.scope, entry.name))
                ),
                _ => file_logs_command(
                    &entry.name,
                    lines,
                    &["/var/log/messages", "/var/log/syslog"],
                ),
            };
            target.run_sh(&command, elevation, ACTION_TIMEOUT)?
        }
    };
    let output = crate::host::ensure_host_command_success(output, "service logs unavailable")?;
    let text = if output.stdout.trim().is_empty() {
        output.stderr
    } else {
        output.stdout
    };
    Ok(text)
}

fn windows_logs_script(name: &str, lines: u32) -> String {
    let name = powershell_quote(name);
    format!(
        r#"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$ErrorActionPreference = 'Stop'
try {{
  Get-WinEvent -FilterHashtable @{{LogName='System'; ProviderName={name}}} -MaxEvents {lines} -ErrorAction Stop |
    ForEach-Object {{ '{{0}} {{1}}' -f $_.TimeCreated.ToString('s'), ($_.Message -replace '\s+', ' ') }}
}} catch {{
  if (($_.FullyQualifiedErrorId -split ',')[0] -eq 'NoMatchingEventsFound') {{ exit 0 }}
  [Console]::Error.WriteLine($_.Exception.Message)
  exit 1
}}
"#
    )
}

fn file_logs_command(name: &str, lines: u32, paths: &[&str]) -> String {
    let paths = paths
        .iter()
        .map(|path| sh_quote(path))
        .collect::<Vec<_>>()
        .join(" ");
    let name = sh_quote(name);
    // A pipeline would hide the reader's exit status behind tail. The ring
    // buffer retains only the requested lines, and awk errors remain failures.
    format!(
        r#"
set --
for file in {paths}; do
  if [ -e "$file" ]; then
    if [ ! -f "$file" ]; then
      printf 'service log file is not a regular file\n' >&2
      exit 1
    fi
    if [ ! -r "$file" ]; then
      printf 'permission denied: service log file is not readable\n' >&2
      exit 1
    fi
    set -- "$@" "$file"
  fi
done
if [ "$#" -eq 0 ]; then
  printf 'service log files are unavailable\n' >&2
  exit 1
fi
NYATERM_LOG_FILTER={name} awk -v limit={lines} '
BEGIN {{ needle = tolower(ENVIRON["NYATERM_LOG_FILTER"]) }}
index(tolower($0), needle) {{ records[count % limit] = $0; count++ }}
END {{
  start = count > limit ? count - limit : 0
  for (i = start; i < count; i++) print records[i % limit]
}}' "$@"
"#
    )
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    use super::list_services;
    use super::{
        ServiceAction, ServiceBackend, ServiceEntry, ServiceStartup, ServiceState, action_command,
        parse_unix_output, parse_windows_output, windows_action_script,
    };
    #[cfg(target_os = "macos")]
    use crate::host::{Elevation, HostOs, HostTarget};

    const SYSTEMD: &str = "@@FORMAT systemd
@@UNITS
  ssh.service                loaded    active   running OpenBSD Secure Shell server
● nginx.service              loaded    failed   failed  A high performance web server
  cloud-init.service         loaded    active   exited  Initial cloud-init job
  ghost.service              not-found inactive dead    ghost.service
@@FILES
ssh.service                enabled         enabled
nginx.service              enabled         enabled
cloud-init.service         enabled         enabled
cron.service               disabled        enabled
getty@.service             enabled         enabled
dbus.service               static          -
bad.service                masked          enabled
@@PIDS
Id=ssh.service
MainPID=812

Id=nginx.service
MainPID=0
";

    #[test]
    fn systemd_merges_units_files_and_pids() {
        let snapshot = parse_unix_output(SYSTEMD).unwrap();
        assert_eq!(snapshot.backend, ServiceBackend::Systemd);
        let by = |name: &str| snapshot.entries.iter().find(|e| e.name == name).unwrap();
        let ssh = by("ssh");
        assert_eq!(ssh.state, ServiceState::Running);
        assert_eq!(ssh.pid, Some(812));
        assert_eq!(ssh.startup, ServiceStartup::Enabled);
        assert_eq!(ssh.description, "OpenBSD Secure Shell server");
        assert_eq!(by("nginx").state, ServiceState::Failed);
        assert_eq!(by("nginx").pid, None);
        assert_eq!(by("cloud-init").state, ServiceState::Stopped);
        assert_eq!(by("cloud-init").detail, "exited");
        // Never-loaded units come from the unit-file list.
        assert_eq!(by("cron").startup, ServiceStartup::Disabled);
        assert_eq!(by("dbus").startup, ServiceStartup::Static);
        assert_eq!(by("bad").startup, ServiceStartup::Masked);
        assert!(snapshot.entries.iter().all(|e| !e.name.contains('@')));
        assert_eq!(by("ghost").description, "");
    }

    #[test]
    fn launchd_reads_both_domains_and_disabled_overrides() {
        let output = "@@FORMAT launchd
@@DOMAIN system
\t\t     678      - \tcom.apple.kernelmanager_helper
\t\t       0      1 \tcom.apple.wifiFirmwareLoader
\t\t       0      - \tcom.apple.uninstalld
@@DISABLED system
\tdisabled services = {
\t\t\"com.apple.uninstalld\" => disabled
\t\t\"net.tunnelblick\" => enabled
@@DOMAIN gui/501
\t\t    1579      0 \tcom.apple.progressd
@@DISABLED gui/501
";
        let snapshot = parse_unix_output(output).unwrap();
        assert!(snapshot.partial);
        assert_eq!(snapshot.entries.len(), 4);
        let by = |name: &str| snapshot.entries.iter().find(|e| e.name == name).unwrap();
        assert_eq!(by("com.apple.kernelmanager_helper").pid, Some(678));
        assert_eq!(by("com.apple.kernelmanager_helper").scope, "system");
        assert_eq!(by("com.apple.wifiFirmwareLoader").detail, "last exit 1");
        assert_eq!(by("com.apple.uninstalld").startup, ServiceStartup::Disabled);
        assert_eq!(by("com.apple.progressd").scope, "gui/501");
        assert_eq!(by("com.apple.progressd").startup, ServiceStartup::Enabled);
    }

    #[test]
    fn freebsd_rc_lists_enabled_scripts_with_running_state() {
        let snapshot = parse_unix_output("@@FORMAT freebsd\nsshd 1\nnginx 0\n").unwrap();
        assert_eq!(snapshot.backend, ServiceBackend::FreeBsdRc);
        assert_eq!(snapshot.entries[0].name, "nginx");
        assert_eq!(snapshot.entries[0].state, ServiceState::Stopped);
        assert_eq!(snapshot.entries[1].state, ServiceState::Running);
    }

    #[test]
    fn openrc_and_sysv_status_lines_are_parsed() {
        let openrc = parse_unix_output(
            "@@FORMAT openrc\nRunlevel: default\n sshd                                 [  started  ]\n crond   [  stopped  ]\n",
        )
        .unwrap();
        assert_eq!(openrc.entries.len(), 2);
        assert_eq!(openrc.entries[1].state, ServiceState::Running);
        let sysv =
            parse_unix_output("@@FORMAT sysv\n [ + ]  ssh\n [ - ]  cron\n [ ? ]  x\n").unwrap();
        assert_eq!(sysv.entries[0].name, "cron");
        assert_eq!(sysv.entries[0].state, ServiceState::Stopped);
        assert_eq!(sysv.entries[1].state, ServiceState::Running);
    }

    #[test]
    fn windows_rows_map_state_start_mode_and_pid() {
        let output = "Spooler\tPrint Spooler\tRunning\tAuto\t1234\tLoads files to memory\r\nBITS\tBackground Intelligent Transfer Service\tStopped\tManual\t0\t\r\nDead\tDead\tStart Pending\tDisabled\t0\t\r\n";
        let snapshot = parse_windows_output(output);
        assert_eq!(snapshot.entries.len(), 3);
        let spooler = snapshot
            .entries
            .iter()
            .find(|e| e.name == "Spooler")
            .unwrap();
        assert_eq!(spooler.pid, Some(1234));
        assert_eq!(spooler.startup, ServiceStartup::Enabled);
        assert!(spooler.description.contains("Print Spooler"));
        let bits = snapshot.entries.iter().find(|e| e.name == "BITS").unwrap();
        assert_eq!(
            (bits.state, bits.pid, bits.startup),
            (ServiceState::Stopped, None, ServiceStartup::Static)
        );
        let dead = snapshot.entries.iter().find(|e| e.name == "Dead").unwrap();
        assert_eq!(dead.state, ServiceState::Transitioning);
    }

    #[test]
    fn action_commands_quote_names_and_target_the_right_domain() {
        let entry = ServiceEntry {
            name: "my svc'; rm -rf /".to_string(),
            description: String::new(),
            state: ServiceState::Running,
            detail: String::new(),
            startup: ServiceStartup::Enabled,
            pid: None,
            scope: "gui/501".to_string(),
        };
        let cmd = action_command(ServiceBackend::Systemd, &entry, ServiceAction::Restart).unwrap();
        assert!(cmd.starts_with("systemctl restart '"));
        assert!(
            cmd.contains("'\\''"),
            "single quotes must be escaped: {cmd}"
        );
        let mac = action_command(ServiceBackend::Launchd, &entry, ServiceAction::Restart).unwrap();
        assert!(mac.starts_with("launchctl kickstart -k 'gui/501/"));
        assert!(action_command(ServiceBackend::FreeBsdRc, &entry, ServiceAction::Enable).is_err());
        let bad = ServiceEntry {
            name: "a\nb".into(),
            ..entry
        };
        assert!(action_command(ServiceBackend::Systemd, &bad, ServiceAction::Start).is_err());
    }

    #[test]
    fn windows_actions_escape_powershell_quotes() {
        let entry = ServiceEntry {
            name: "o'neil".into(),
            description: String::new(),
            state: ServiceState::Stopped,
            detail: String::new(),
            startup: ServiceStartup::Unknown,
            pid: None,
            scope: String::new(),
        };
        let script = windows_action_script(&entry, ServiceAction::Stop).unwrap();
        assert!(script.contains("'o''neil'"));
        assert!(script.contains("-Force"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn local_macos_listing_finds_running_services() {
        let snapshot =
            list_services(&HostTarget::Local, HostOs::local(), Elevation::None).expect("launchctl");
        assert_eq!(snapshot.backend, ServiceBackend::Launchd);
        assert!(snapshot.entries.len() > 50);
        assert!(
            snapshot
                .entries
                .iter()
                .any(|e| e.state == ServiceState::Running && e.pid.is_some())
        );
        assert!(snapshot.entries.iter().any(|e| e.scope == "system"));
    }
}
