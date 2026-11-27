//! Linux package ownership, validation and privileged installation. No UI or task ownership.
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use nyaterm_core::app_identity::AppFlavor;
use nyaterm_core::secret::SecretString;
use nyaterm_core::updater::{UpdatePackageKind, parse_current_version};

use crate::download::{InstallationTarget, verify_package_file};
use crate::error::{UpdateError, UpdatePackageProblem};

const OUTPUT_LIMIT: usize = 16 * 1024;

#[derive(Clone, Debug)]
pub struct PreparedPackage {
    artifact: PathBuf,
    installation: InstallationTarget,
    version: String,
    signature: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallOutcome {
    Installed,
    Cancelled,
    NeedsPassword,
    AuthorizationFailed,
    PreflightFailed(String),
    Failed(String),
}

pub enum Authorization {
    Polkit,
    Sudo(SecretString),
}

pub fn is_system_package(kind: UpdatePackageKind) -> bool {
    matches!(
        kind,
        UpdatePackageKind::LinuxDeb
            | UpdatePackageKind::LinuxDebGlibc231
            | UpdatePackageKind::LinuxRpm
    )
}

fn program(name: &str) -> PathBuf {
    // Never resolve privileged executables through a user-controlled PATH.
    for directory in ["/usr/bin", "/usr/sbin", "/bin", "/sbin"] {
        let path = Path::new(directory).join(name);
        if path.is_file() {
            return path;
        }
    }
    Path::new("/usr/bin").join(name)
}

fn package_name(kind: UpdatePackageKind) -> &'static str {
    if kind == UpdatePackageKind::LinuxDebGlibc231 {
        "nyaterm-compat"
    } else {
        AppFlavor::current().desktop_id()
    }
}

fn expected_executable(kind: UpdatePackageKind) -> PathBuf {
    Path::new("/opt").join(package_name(kind)).join("nyaterm")
}

pub fn detect_package(executable: &Path) -> Option<UpdatePackageKind> {
    let kinds: &[UpdatePackageKind] = if option_env!("NYATERM_LINUX_COMPAT") == Some("glibc231") {
        &[UpdatePackageKind::LinuxDebGlibc231]
    } else {
        &[UpdatePackageKind::LinuxDeb, UpdatePackageKind::LinuxRpm]
    };
    kinds.iter().copied().find(|kind| {
        if executable != expected_executable(*kind) {
            return false;
        }
        let mut query = if *kind == UpdatePackageKind::LinuxRpm {
            let mut command = Command::new(program("rpm"));
            command
                .args(["-qf", "--queryformat", "%{NAME}"])
                .arg(executable);
            command
        } else {
            let mut command = Command::new(program("dpkg-query"));
            command.arg("-S").arg(executable);
            command
        };
        let Ok(output) = run_command(&mut query, None) else {
            return false;
        };
        if !output.status.success() {
            return false;
        }
        let owner = output.stdout.trim();
        if *kind == UpdatePackageKind::LinuxRpm {
            owner == package_name(*kind)
        } else {
            owner.split_once(": ").is_some_and(|(name, path)| {
                name.split(':').next() == Some(package_name(*kind)) && Path::new(path) == executable
            })
        }
    })
}

impl PreparedPackage {
    pub(crate) fn prepare(
        artifact: PathBuf,
        installation: InstallationTarget,
        version: String,
        signature: String,
    ) -> Result<Self, UpdateError> {
        let package = Self {
            artifact,
            installation,
            version,
            signature,
        };
        package
            .validate()
            .map_err(|message| UpdateError::package(UpdatePackageProblem::Rejected, message))?;
        Ok(package)
    }

    pub fn restart_target(&self) -> PathBuf {
        if self.installation.package == UpdatePackageKind::LinuxDebGlibc231 {
            // The compat launcher supplies its private runtime environment.
            self.installation
                .executable
                .with_file_name("start-nyaterm.sh")
        } else {
            self.installation.executable.clone()
        }
    }

    fn validate(&self) -> Result<(), String> {
        verify_package_file(&self.artifact, &self.signature).map_err(|error| error.to_string())?;
        if detect_package(&self.installation.executable) != Some(self.installation.package) {
            return Err(
                "The running application no longer belongs to the expected system package".into(),
            );
        }
        let kind = self.installation.package;
        let mut command = if kind == UpdatePackageKind::LinuxRpm {
            let mut command = Command::new(program("rpm"));
            command
                .args([
                    "-qp",
                    "--queryformat",
                    "%{NAME}\n%{VERSION}-%{RELEASE}\n%{ARCH}\n",
                ])
                .arg(&self.artifact);
            command
        } else {
            let mut command = Command::new(program("dpkg-deb"));
            command
                .args([
                    "--show",
                    "--showformat=${Package}\n${Version}\n${Architecture}\n",
                ])
                .arg(&self.artifact);
            command
        };
        let output = run_command(&mut command, None)?;
        if !output.status.success() {
            return Err("Could not inspect the downloaded package".into());
        }
        validate_metadata(kind, &self.version, std::env::consts::ARCH, &output.stdout)
    }

    /// Run on a blocking worker. Once a package manager starts, callers must let it finish.
    pub fn install(&self, authorization: Authorization) -> InstallOutcome {
        if let Err(error) = self.validate() {
            return InstallOutcome::PreflightFailed(error);
        }
        let mut runner = run_command;
        let outcome = self.install_with(authorization, &mut runner);
        if outcome == InstallOutcome::Installed {
            // Verify the package database, not just the child exit status.
            match self.verify_installed() {
                Ok(()) => {
                    let _ = std::fs::remove_file(&self.artifact);
                    if let Some(directory) = self.artifact.parent() {
                        let _ = std::fs::remove_dir(directory);
                    }
                }
                Err(error) => return InstallOutcome::Failed(error),
            }
        }
        outcome
    }

    fn install_with(
        &self,
        authorization: Authorization,
        run: &mut impl FnMut(&mut Command, Option<SecretString>) -> Result<CommandOutput, String>,
    ) -> InstallOutcome {
        let kind = self.installation.package;
        let (tool, install): (&str, &[&str]) = if kind == UpdatePackageKind::LinuxRpm {
            ("rpm", &["-U"])
        } else {
            ("dpkg", &["--force-confdef", "--force-confold", "--install"])
        };
        // dpkg --no-act only simulates unpacking and misses unsatisfied Depends.
        // Use APT's read-only resolver, rejecting any transaction beyond this package.
        let mut preflight = if kind == UpdatePackageKind::LinuxRpm {
            let mut command = Command::new(program("rpm"));
            command.args(["--test", "-U"]).arg(&self.artifact);
            command
        } else {
            let mut command = Command::new(program("apt-get"));
            command
                .args([
                    "--simulate",
                    "--no-download",
                    "--no-remove",
                    "--no-install-recommends",
                    "-o",
                    "Debug::NoLocking=1",
                    "install",
                ])
                .arg(&self.artifact);
            command
        };
        match run(&mut preflight, None) {
            Ok(output) if output.status.success() => {
                if kind != UpdatePackageKind::LinuxRpm
                    && !deb_transaction_is_self_contained(&output.stdout, package_name(kind))
                {
                    return InstallOutcome::PreflightFailed("This update requires other package changes. Install its dependencies with the system package manager, then retry".into());
                }
            }
            Ok(output) => return InstallOutcome::PreflightFailed(output.diagnostic()),
            Err(error) => return InstallOutcome::PreflightFailed(error),
        }
        let polkit = matches!(authorization, Authorization::Polkit);
        let (mut command, password) = match authorization {
            Authorization::Polkit => {
                let executable = program("pkexec");
                let mut command = Command::new(executable);
                command.arg("--disable-internal-agent");
                (command, None)
            }
            Authorization::Sudo(password) => {
                // Authenticate and execute in one process: a separate sudo -v does
                // not reliably populate a reusable timestamp in desktop sessions.
                let mut command = Command::new(program("sudo"));
                command.args(["-S", "-k", "-p", "", "--"]);
                (command, Some(password))
            }
        };
        command.arg(program(tool)).args(install).arg(&self.artifact);
        match run(&mut command, password) {
            Ok(output) if output.status.success() => InstallOutcome::Installed,
            Ok(output) if polkit && output.status.code() == Some(126) => InstallOutcome::Cancelled,
            Ok(output) if polkit && output.status.code() == Some(127) => {
                InstallOutcome::AuthorizationFailed
            }
            Ok(output)
                if !polkit && output.stderr.lines().any(|line| line.starts_with("sudo:")) =>
            {
                InstallOutcome::AuthorizationFailed
            }
            Ok(output) => InstallOutcome::Failed(output.diagnostic()),
            Err(_) if polkit && !program("pkexec").is_file() => InstallOutcome::NeedsPassword,
            Err(error) => InstallOutcome::Failed(error),
        }
    }

    fn verify_installed(&self) -> Result<(), String> {
        let kind = self.installation.package;
        let mut command = if kind == UpdatePackageKind::LinuxRpm {
            let mut command = Command::new(program("rpm"));
            command.args([
                "-q",
                "--queryformat",
                "%{NAME}\n%{VERSION}-%{RELEASE}\n%{ARCH}\n",
                package_name(kind),
            ]);
            command
        } else {
            let mut command = Command::new(program("dpkg-query"));
            command.args([
                "--show",
                "--showformat=${Package}\n${Version}\n${Architecture}\n${Status}\n",
                package_name(kind),
            ]);
            command
        };
        let output = run_command(&mut command, None)?;
        if !output.status.success()
            || (kind != UpdatePackageKind::LinuxRpm
                && output.stdout.lines().nth(3) != Some("install ok installed"))
        {
            return Err("The package manager did not finish configuring the update; repair the package installation before retrying".into());
        }
        validate_metadata(kind, &self.version, std::env::consts::ARCH, &output.stdout)
    }
}

fn deb_transaction_is_self_contained(output: &str, package: &str) -> bool {
    output.len() < OUTPUT_LIMIT
        && output.lines().all(|line| {
            if line.starts_with("Remv ") {
                return false;
            }
            if line.starts_with("Inst ") || line.starts_with("Conf ") {
                return line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|name| name.split(':').next())
                    == Some(package);
            }
            true
        })
}

fn validate_metadata(
    kind: UpdatePackageKind,
    version: &str,
    arch: &str,
    metadata: &str,
) -> Result<(), String> {
    let version = parse_current_version(version).map_err(|error| error.to_string())?;
    let expected_version = if kind == UpdatePackageKind::LinuxRpm {
        let release = if version.pre.is_empty() {
            "1".to_string()
        } else {
            let parts: Vec<_> = version
                .pre
                .as_str()
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|s| !s.is_empty())
                .collect();
            format!("0.{}", parts.join("."))
        };
        format!(
            "{}.{}.{}-{release}",
            version.major, version.minor, version.patch
        )
    } else if kind == UpdatePackageKind::LinuxDebGlibc231 {
        format!("{}-1+glibc231", version.to_string().replace('-', "~"))
    } else {
        version.to_string().replace('-', "~")
    };
    let architecture = match (kind, arch) {
        (UpdatePackageKind::LinuxRpm, "x86_64") => "x86_64",
        (UpdatePackageKind::LinuxRpm, "aarch64") => "aarch64",
        (_, "x86_64") => "amd64",
        (_, "aarch64") => "arm64",
        _ => return Err("Unsupported package architecture".into()),
    };
    let fields: Vec<_> = metadata.lines().take(3).collect();
    if fields != [package_name(kind), expected_version.as_str(), architecture] {
        return Err(
            "The update package name, version or architecture does not match this installation"
                .into(),
        );
    }
    Ok(())
}

struct CommandOutput {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}
impl CommandOutput {
    fn diagnostic(&self) -> String {
        let text = if self.stderr.trim().is_empty() {
            &self.stdout
        } else {
            &self.stderr
        };
        // Bound output and strip control characters before showing a diagnostic.
        let text: String = text
            .chars()
            .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
            .collect();
        if text.trim().is_empty() {
            format!("Package command failed ({})", self.status)
        } else {
            text
        }
    }
}

fn drain(mut reader: impl Read) -> std::io::Result<String> {
    let mut output = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = OUTPUT_LIMIT.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..count.min(remaining)]);
    }
    Ok(String::from_utf8_lossy(&output).into_owned())
}

fn run_command(
    command: &mut Command,
    password: Option<SecretString>,
) -> Result<CommandOutput, String> {
    command
        .env("LC_ALL", "C")
        .stdin(if password.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start package command: {error}"))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    std::thread::scope(|scope| {
        let out = scope.spawn(|| drain(stdout));
        let err = scope.spawn(|| drain(stderr));
        if let Some(password) = password
            && let Some(mut stdin) = child.stdin.take()
        {
            // Do not trim: whitespace can be part of a password. EOF prevents repeated prompts.
            let _ = writeln!(stdin, "{}", password.expose_secret());
        }
        let status = child.wait().map_err(|error| error.to_string())?;
        let stdout = out
            .join()
            .map_err(|_| "package output reader failed")?
            .map_err(|e| e.to_string())?;
        let stderr = err
            .join()
            .map_err(|_| "package output reader failed")?
            .map_err(|e| e.to_string())?;
        Ok(CommandOutput {
            status,
            stdout,
            stderr,
        })
    })
}

#[cfg(test)]
mod tests;
