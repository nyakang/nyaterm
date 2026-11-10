//! One execution target for every host-inventory tool: the machine running the
//! application, or an SSH host reached through the shared multiplexed connection.
//!
//! Design constraints:
//! * No async runtime is created per call. Local commands use `std::process`
//!   with bounded pipe I/O and a deadline; SSH reuses the transport's multiplex
//!   runtime.
//! * Callers batch everything a refresh needs into one script, so a refresh is
//!   one process spawn (local) or one exec channel (SSH).
//! * Elevated commands send the sudo password on stdin only; it never enters
//!   argv, logs, or `Debug` output.

mod local_exec;

use anyhow::bail;
use local_exec::run_local;
use nyaterm_core::SecretString;
use nyaterm_transport::ssh_execution::{
    RemoteCommandOutput, run_ssh_command, run_ssh_command_with_input,
};
use nyaterm_transport::{SshMultiplexHandle, SshSessionConfig};
use std::process::Command;
use std::time::Duration;

/// Failures produced by the bounded local command supervisor. No command,
/// input, or captured output is included in these errors.
#[derive(Debug)]
pub enum HostCommandError {
    Timeout(Duration),
    OutputLimitExceeded,
}

impl std::fmt::Display for HostCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout(timeout) => write!(f, "local command timed out after {timeout:?}"),
            Self::OutputLimitExceeded => {
                f.write_str("local command output exceeds the 16 MiB per-stream limit")
            }
        }
    }
}
impl std::error::Error for HostCommandError {}

/// A failed read must not look like empty data. Return only a diagnostic line,
/// never captured stdout, which may contain command lines or service logs.
pub(crate) fn ensure_host_command_success(
    output: RemoteCommandOutput,
    fallback: &str,
) -> anyhow::Result<RemoteCommandOutput> {
    if output.exit_status.is_some_and(|code| code != 0) {
        let detail = output
            .stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or(fallback);
        bail!("{detail}");
    }
    Ok(output)
}

/// Operating-system family of a target, which selects the backend scripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostOs {
    Linux,
    MacOs,
    FreeBsd,
    /// Other POSIX systems (NetBSD, OpenBSD, illumos, ...): `sh` + `ps`/`netstat`.
    OtherUnix,
    Windows,
}

impl HostOs {
    /// The family of the machine running this process.
    pub const fn local() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "freebsd") {
            Self::FreeBsd
        } else {
            Self::OtherUnix
        }
    }

    /// Map `uname -s` output; unknown values degrade to generic POSIX.
    pub fn from_uname(output: &str) -> Self {
        match output.trim().to_ascii_lowercase().as_str() {
            "linux" => Self::Linux,
            "darwin" => Self::MacOs,
            "freebsd" | "dragonfly" => Self::FreeBsd,
            _ => Self::OtherUnix,
        }
    }

    pub const fn is_unix(self) -> bool {
        !matches!(self, Self::Windows)
    }
}

/// How a command is elevated.
#[derive(Clone, Default)]
pub enum Elevation {
    #[default]
    None,
    /// `sudo -n`: succeeds only when no password is required.
    SudoPasswordless,
    /// `sudo -S` with the password on stdin.
    SudoPassword(SecretString),
}

impl std::fmt::Debug for Elevation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("Elevation::None"),
            Self::SudoPasswordless => f.write_str("Elevation::SudoPasswordless"),
            Self::SudoPassword(_) => f.write_str("Elevation::SudoPassword(<redacted>)"),
        }
    }
}

/// Where a tool runs.
// `Ssh` carries a whole connection profile; boxing it would only move the
// allocation to every clone, and targets are created once per refresh.
#[allow(clippy::large_enum_variant)]
#[derive(Clone)]
pub enum HostTarget {
    Local,
    Ssh {
        config: SshSessionConfig,
        multiplex: Option<SshMultiplexHandle>,
    },
}

impl std::fmt::Debug for HostTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local => f.write_str("HostTarget::Local"),
            // The config carries credentials, so only the multiplex state is shown.
            Self::Ssh { multiplex, .. } => write!(
                f,
                "HostTarget::Ssh {{ multiplex: {} }}",
                if multiplex.is_some() {
                    "Some(..)"
                } else {
                    "None"
                }
            ),
        }
    }
}

impl HostTarget {
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local)
    }

    /// Platform family without a probe: exact locally, generic POSIX over SSH
    /// (remote Windows hosts are not a target). The tools' scripts pick the real
    /// dialect themselves, so this only decides Windows vs everything else.
    pub fn os_hint(&self) -> HostOs {
        match self {
            Self::Local => HostOs::local(),
            Self::Ssh { .. } => HostOs::OtherUnix,
        }
    }

    pub fn ssh_config(&self) -> Option<&SshSessionConfig> {
        match self {
            Self::Local => None,
            Self::Ssh { config, .. } => Some(config),
        }
    }

    /// SSH profiles can forbid resource probes; the local machine is always allowed.
    pub fn ensure_probes_allowed(&self) -> anyhow::Result<()> {
        match self {
            Self::Local => Ok(()),
            Self::Ssh { config, .. } => Ok(config.ensure_remote_resource_probes_allowed()?),
        }
    }

    /// Run a pre-built shell command line (scripts that already embed their own
    /// `sh -s` heredoc), optionally feeding `input` on stdin.
    pub fn run_raw(
        &self,
        command: String,
        input: Option<SecretString>,
        timeout: Duration,
    ) -> anyhow::Result<RemoteCommandOutput> {
        match self {
            Self::Local => {
                if cfg!(windows) {
                    bail!("this tool has no Windows backend yet");
                }
                run_local(
                    Command::new("sh").args(["-c", &command]),
                    input.as_ref().map(|s| format!("{}\n", s.expose_secret())),
                    timeout,
                )
            }
            Self::Ssh { config, multiplex } => match input {
                None => run_ssh_command(
                    config.clone(),
                    multiplex.clone(),
                    command.into_bytes(),
                    timeout,
                ),
                input => run_ssh_command_with_input(
                    config.clone(),
                    multiplex.clone(),
                    command,
                    input,
                    timeout,
                ),
            },
        }
    }

    /// Detect the platform. Local is free; remote costs one `uname -s`, so the
    /// caller should cache the answer per connection.
    pub fn detect_os(&self, timeout: Duration) -> anyhow::Result<HostOs> {
        match self {
            Self::Local => Ok(HostOs::local()),
            Self::Ssh { .. } => {
                let out = self.run_sh("uname -s", Elevation::None, timeout)?;
                if out.exit_status.is_some_and(|code| code != 0) {
                    bail!("could not detect the remote operating system");
                }
                Ok(HostOs::from_uname(&out.stdout))
            }
        }
    }

    /// Run a POSIX `sh` script. Output is returned even for a non-zero exit so
    /// scripts can report partial results through markers.
    pub fn run_sh(
        &self,
        script: &str,
        elevation: Elevation,
        timeout: Duration,
    ) -> anyhow::Result<RemoteCommandOutput> {
        let (command, input) = match elevation {
            Elevation::None => (format!("LC_ALL=C sh -c {}", sh_quote(script)), None),
            Elevation::SudoPasswordless => {
                (format!("LC_ALL=C sudo -n sh -c {}", sh_quote(script)), None)
            }
            Elevation::SudoPassword(secret) => (
                format!("LC_ALL=C sudo -S -p '' sh -c {}", sh_quote(script)),
                Some(secret),
            ),
        };
        match self {
            Self::Local => run_local(
                Command::new("sh").args(["-c", &command]),
                input.as_ref().map(|s| format!("{}\n", s.expose_secret())),
                timeout,
            ),
            Self::Ssh { config, multiplex } => match input {
                None => run_ssh_command(
                    config.clone(),
                    multiplex.clone(),
                    command.into_bytes(),
                    timeout,
                ),
                input => run_ssh_command_with_input(
                    config.clone(),
                    multiplex.clone(),
                    command,
                    input,
                    timeout,
                ),
            },
        }
    }

    /// Run a PowerShell script on the local Windows machine. Remote Windows
    /// hosts are not a target: their SSH default shell is not predictable.
    pub fn run_powershell(
        &self,
        script: &str,
        timeout: Duration,
    ) -> anyhow::Result<RemoteCommandOutput> {
        let Self::Local = self else {
            bail!("PowerShell backends only run on the local machine");
        };
        let mut command = Command::new("powershell");
        command.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        // Stdin command mode parses interactive statements and can leave the
        // final multiline block unexecuted at EOF. Pass one complete script.
        run_local(&mut command, None, timeout)
    }
}

/// Terminate a process by pid on any host, optionally elevated. Used by the
/// Ports panel, where the owning process is often another user's.
pub fn signal_pid(
    target: &HostTarget,
    os: HostOs,
    pid: u32,
    force: bool,
    elevation: Elevation,
) -> anyhow::Result<()> {
    // A caller's cached platform hint must never weaken local control policy.
    let os = if target.is_local() {
        HostOs::local()
    } else {
        os
    };
    target.ensure_probes_allowed()?;
    crate::process_control::ProcessControlCapabilities::for_os(os).ensure_signal(if force {
        "KILL"
    } else {
        "TERM"
    })?;
    if pid == 0 {
        bail!("this socket has no known owning process");
    }
    if matches!(os, HostOs::Windows) {
        if !target.is_local() {
            bail!("remote Windows hosts are not supported");
        }
        return crate::local_process::kill_local_process(pid, force);
    }
    let signal = if force { "KILL" } else { "TERM" };
    let output = target.run_sh(
        &format!("kill -{signal} {pid}"),
        elevation,
        Duration::from_secs(10),
    )?;
    if output.exit_status.is_some_and(|code| code != 0) {
        let detail = [output.stderr.trim(), output.stdout.trim()]
            .into_iter()
            .find(|text| !text.is_empty())
            .unwrap_or("kill failed");
        bail!("{}", detail.lines().next().unwrap_or(detail));
    }
    Ok(())
}

/// Whether a failure message means "needs more privileges", so the UI can offer
/// elevation instead of a bare error.
pub fn looks_like_permission_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "permission denied",
        "operation not permitted",
        "not permitted",
        "access denied",
        "access is denied",
        "interactive authentication required",
        "must be root",
        "a password is required",
        "authentication is required",
        "insufficient privileges",
        "not privileged",
        "authorization",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// POSIX single-quote escaping.
pub fn sh_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

#[cfg(all(test, unix))]
mod tests {
    use super::{
        Elevation, HostOs, HostTarget, ensure_host_command_success, looks_like_permission_error,
    };
    use nyaterm_core::SecretString;
    use std::time::Duration;

    #[test]
    fn failed_reads_return_one_stderr_line_without_exposing_stdout() {
        use nyaterm_transport::ssh_execution::RemoteCommandOutput;
        for stderr in ["\n denied\nsecond line", ""] {
            let error = ensure_host_command_success(
                RemoteCommandOutput {
                    stdout: "private command or log".into(),
                    stderr: stderr.into(),
                    exit_status: Some(1),
                },
                "read failed",
            )
            .unwrap_err();
            assert_eq!(
                error.to_string(),
                if stderr.is_empty() {
                    "read failed"
                } else {
                    "denied"
                }
            );
        }
        for exit_status in [Some(0), None] {
            let output = ensure_host_command_success(
                RemoteCommandOutput {
                    stdout: "valid data".into(),
                    stderr: String::new(),
                    exit_status,
                },
                "read failed",
            )
            .unwrap();
            assert_eq!(output.stdout, "valid data");
        }
    }

    #[test]
    fn local_sh_runs_scripts_with_quotes_and_reports_exit_status() {
        let out = HostTarget::Local
            .run_sh(
                "echo \"it's ok\"; exit 3",
                Elevation::None,
                Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(out.stdout.trim(), "it's ok");
        assert_eq!(out.exit_status, Some(3));
    }

    #[test]
    fn local_sh_enforces_the_deadline() {
        let err = HostTarget::Local
            .run_sh("sleep 5", Elevation::None, Duration::from_millis(150))
            .unwrap_err();
        assert!(err.to_string().contains("timed out"));
    }

    #[test]
    fn large_output_does_not_deadlock() {
        let out = HostTarget::Local
            .run_sh(
                "yes x | head -c 2000000",
                Elevation::None,
                Duration::from_secs(10),
            )
            .unwrap();
        assert_eq!(out.stdout.len(), 2_000_000);
    }

    #[test]
    fn permission_errors_are_recognised_across_tools() {
        for message in [
            "kill: (1): Operation not permitted",
            "Failed to stop ssh.service: Interactive authentication required.",
            "sudo: a password is required",
            "Access is denied.",
        ] {
            assert!(looks_like_permission_error(message), "{message}");
        }
        assert!(!looks_like_permission_error("Unit foo.service not found."));
    }

    #[test]
    fn uname_maps_to_platform_families() {
        assert_eq!(HostOs::from_uname("Darwin\n"), HostOs::MacOs);
        assert_eq!(HostOs::from_uname("FreeBSD"), HostOs::FreeBsd);
        assert_eq!(HostOs::from_uname("Linux"), HostOs::Linux);
        assert_eq!(HostOs::from_uname("OpenBSD"), HostOs::OtherUnix);
    }

    #[test]
    fn elevation_debug_never_prints_the_password() {
        let secret = Elevation::SudoPassword(SecretString::new("hunter2"));
        assert!(!format!("{secret:?}").contains("hunter2"));
    }
}
