//! Local integration is installed through shell startup hooks, never PTY input.
//! Unrecognized launch modes retain their original command and byte stream.

use crate::local_shell::{LocalShellPathStyle, ResolvedLocalShell};
use crate::ssh_shell_integration::{
    SshIntegrationOutput, SshShellIntegrationState, build_ssh_ready_marker,
};
use std::path::{Path, PathBuf};

#[cfg(all(test, windows))]
mod lifecycle_tests;
#[cfg(test)]
mod tests;

pub(super) struct LocalShellIntegration {
    state: SshShellIntegrationState,
    pub(super) path_style: LocalShellPathStyle,
    pub(super) completion_distro: Option<String>,
    pub(super) startup_files: Option<tempfile::TempDir>,
}

impl LocalShellIntegration {
    pub(super) fn prepare(
        shell: &ResolvedLocalShell,
        session_id: &str,
    ) -> Option<(ResolvedLocalShell, Self)> {
        let (launch, startup_files) = integrated_launch(shell, session_id)?;
        let path_style = if program_name(&shell.program) == "wsl" {
            LocalShellPathStyle::Wsl
        } else {
            LocalShellPathStyle::from_program(&launch.program)
        };
        Some((
            launch,
            Self {
                path_style,
                startup_files: Some(startup_files),
                completion_distro: None,
                // No ready marker or suppression is needed: the script is a startup
                // argument, not echoed PTY input. Startup errors and prompts stay visible.
                state: SshShellIntegrationState::new(
                    None,
                    build_ssh_ready_marker(session_id),
                    None,
                ),
            },
        ))
    }

    pub(super) fn completion_context(
        &self,
        identity: crate::ssh_shell_integration::ShellIdentity,
    ) -> nyaterm_core::completion::types::ShellCommandContext {
        let dialect =
            nyaterm_core::completion::types::ShellDialect::from_program(&identity.program);
        nyaterm_core::completion::types::ShellCommandContext {
            scope: nyaterm_core::completion::types::CompletionScope::new(&[
                "local",
                &identity.user,
                &identity.program,
                self.completion_distro.as_deref().unwrap_or(""),
                &format!("{dialect:?}"),
            ]),
            dialect,
            command_events: true,
        }
    }
    pub(super) fn filter(&mut self, bytes: &[u8]) -> SshIntegrationOutput {
        self.state.filter_output(bytes)
    }
}

fn program_name(program: &str) -> String {
    let name = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    name.strip_suffix(".exe").unwrap_or(&name).to_string()
}

/// The WSL distribution the session targets: an explicit `--distro`/`-d`
/// launcher argument wins, otherwise the host default distribution. Only WSL
/// launchers carry one; other shells return `None`.
pub(super) fn wsl_distro(shell: &ResolvedLocalShell) -> Option<String> {
    if program_name(&shell.program) != "wsl" {
        return None;
    }
    wsl_distro_from_launcher(&shell.program, &shell.args).or_else(wsl_default_distro)
}

/// Distribution selection carried by a WSL launcher configuration
/// (`-d`/`--distribution`), for callers that reason about a launch
/// configuration without a live session. The host default distribution is only
/// known once the session exists.
pub fn wsl_distro_from_launcher(program: &str, args: &[String]) -> Option<String> {
    if program_name(program) != "wsl" {
        return None;
    }
    wsl_distro_from_args(args)
}

/// Read just the distribution selector, tolerating launcher switches this
/// module does not otherwise understand. `wsl_target` rejects unfamiliar
/// switches on purpose because it rewrites the launch, but path translation
/// still needs the distribution for those sessions.
fn wsl_distro_from_args(args: &[String]) -> Option<String> {
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        // Distribution switches only exist before `--exec`/`-e`/`--`.
        if matches!(arg.as_str(), "--exec" | "-e" | "--") {
            return None;
        }
        if matches!(arg.as_str(), "--distribution" | "-d") {
            return args
                .get(index + 1)
                .filter(|value| !value.is_empty() && !value.starts_with('-'))
                .cloned();
        }
        if let Some(value) = arg.strip_prefix("--distribution=") {
            return (!value.is_empty()).then(|| value.to_string());
        }
        index += 1;
    }
    None
}

#[cfg(windows)]
fn wsl_default_distro() -> Option<String> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegOpenKeyExW,
        RegQueryValueExW,
    };

    fn wide(value: &str) -> Vec<u16> {
        OsStr::new(value).encode_wide().chain(Some(0)).collect()
    }

    fn read_string(key: HKEY, name: &[u16]) -> Option<String> {
        let mut kind: REG_VALUE_TYPE = 0;
        let mut size: u32 = 0;
        let status = unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if status != ERROR_SUCCESS || kind != REG_SZ || size < 2 {
            return None;
        }
        let mut buffer = vec![0_u16; size as usize / 2];
        let status = unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        while buffer.last() == Some(&0) {
            buffer.pop();
        }
        (!buffer.is_empty()).then(|| String::from_utf16_lossy(&buffer))
    }

    const LXSS_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Lxss";
    let mut lxss: HKEY = std::ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide(LXSS_KEY).as_ptr(),
            0,
            KEY_READ,
            &mut lxss,
        )
    } != ERROR_SUCCESS
    {
        return None;
    }
    let distro = (|| {
        let guid = read_string(lxss, &wide("DefaultDistribution"))?;
        let mut key: HKEY = std::ptr::null_mut();
        if unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                wide(&format!("{LXSS_KEY}\\{guid}")).as_ptr(),
                0,
                KEY_READ,
                &mut key,
            )
        } != ERROR_SUCCESS
        {
            return None;
        }
        let name = read_string(key, &wide("DistributionName"));
        unsafe { RegCloseKey(key) };
        name
    })();
    unsafe { RegCloseKey(lxss) };
    distro
}

#[cfg(not(windows))]
fn wsl_default_distro() -> Option<String> {
    None
}

fn interactive_args(kind: &str, args: &[String]) -> bool {
    // This deliberately accepts only known startup switches. Commands, scripts,
    // option values, '--', and unfamiliar switches must not be rewritten.
    args.iter().all(|arg| {
        matches!(arg.as_str(), "--login" | "--interactive")
            || (kind == "bash" && matches!(arg.as_str(), "--noprofile" | "--norc"))
            || (kind == "fish" && arg == "--no-config")
            || (arg.starts_with('-')
                && arg.len() > 1
                && arg[1..].chars().all(|c| matches!(c, 'i' | 'l')))
    })
}

fn wsl_target(args: &[String]) -> Option<(Vec<String>, String, Vec<String>)> {
    let mut prefix = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if matches!(arg.as_str(), "--exec" | "-e" | "--") {
            let program = args.get(index + 1)?;
            let kind = program_name(program);
            let rest = &args[index + 2..];
            return (matches!(kind.as_str(), "bash" | "zsh" | "fish")
                && interactive_args(&kind, rest))
            .then(|| (prefix, program.clone(), rest.to_vec()));
        }
        if matches!(
            arg.as_str(),
            "--distribution" | "-d" | "--user" | "-u" | "--cd"
        ) {
            prefix.push(arg.clone());
            prefix.push(args.get(index + 1)?.clone());
            index += 2;
        } else if ["--distribution=", "--user=", "--cd="]
            .iter()
            .any(|name| arg.starts_with(name) && arg.len() > name.len())
        {
            prefix.push(arg.clone());
            index += 1;
        } else {
            return None;
        }
    }
    // The guest bootstrap discovers the selected user's login shell. Never
    // infer Bash from the Windows launcher name or the host environment.
    Some((prefix, String::new(), Vec::new()))
}

fn integrated_launch(
    shell: &ResolvedLocalShell,
    session_id: &str,
) -> Option<(ResolvedLocalShell, tempfile::TempDir)> {
    let kind = program_name(&shell.program);
    if kind == "wsl" {
        let (mut args, target, target_args) = wsl_target(&shell.args)?;
        let startup_files = startup_directory(session_id)?;
        args.extend([
            "--exec".into(),
            "/bin/sh".into(),
            "-c".into(),
            startup_script(),
            "nyaterm-local".into(),
            startup_files.path().to_str()?.into(),
            "wsl".into(),
            target,
        ]);
        args.extend(target_args);
        return Some((
            ResolvedLocalShell {
                program: shell.program.clone(),
                args,
            },
            startup_files,
        ));
    }
    if !matches!(kind.as_str(), "bash" | "zsh" | "fish") || !interactive_args(&kind, &shell.args) {
        return None;
    }
    let (bootstrap, target) = native_bootstrap(&shell.program)?;
    let startup_files = startup_directory(session_id)?;
    let mut args = vec![
        "-c".into(),
        startup_script(),
        "nyaterm-local".into(),
        startup_files.path().to_str()?.replace('\\', "/"),
        "native".into(),
        target,
    ];
    args.extend(shell.args.clone());
    Some((
        ResolvedLocalShell {
            program: bootstrap,
            args,
        },
        startup_files,
    ))
}

fn startup_directory(session_id: &str) -> Option<tempfile::TempDir> {
    // Created before spawning, so every failure and session teardown has a host
    // owner even if TerminateProcess prevents the shell from running traps.
    let directory = tempfile::Builder::new()
        .prefix(&format!("nyaterm-shell-{session_id}-"))
        .tempdir()
        .ok()?;
    for (kind, filename) in [
        (crate::ssh_shell_integration::ShellKind::Bash, "full.bash"),
        (crate::ssh_shell_integration::ShellKind::Zsh, "full.zsh"),
        (crate::ssh_shell_integration::ShellKind::Fish, "full.fish"),
    ] {
        let script = crate::ssh_shell_integration::persistent_script(kind)?;
        // Preserve the local MSYS/WSL path conversion when reusing SSH's hooks.
        let script = script.replace(r#"printf '\033]7;file://%s%s\007' "$(__nyaterm_host)" "${PWD//%/%25}""#, "__nyaterm_local_cwd")
            .replace(r#"printf '\033]7;file://%s%s\007' (hostname) (string replace -a -- '%' '%25' $PWD)"#, "__nyaterm_local_cwd");
        std::fs::write(directory.path().join(filename), script).ok()?;
    }
    Some(directory)
}

fn native_bootstrap(program: &str) -> Option<(String, String)> {
    if !cfg!(windows) {
        return Some(("/bin/sh".into(), program.into()));
    }
    // Use the same POSIX installation, never a different sh found on PATH.
    // In particular WindowsApps' legacy bash.exe launcher has no sibling sh.
    let path = Path::new(program);
    let executable = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::split_paths(&std::env::var_os("PATH")?).find_map(|directory| {
            let candidate = directory.join(program);
            let candidate = if candidate.extension().is_none() {
                candidate.with_extension("exe")
            } else {
                candidate
            };
            candidate.is_file().then_some(candidate)
        })?
    };
    let bootstrap: PathBuf = executable.parent()?.join("sh.exe");
    bootstrap.is_file().then(|| {
        (
            bootstrap.to_string_lossy().into_owned(),
            executable.to_string_lossy().replace('\\', "/"),
        )
    })
}

fn startup_script() -> String {
    // Keep the Windows launch argument small. Rust owns the sourced files and
    // their lifetime, exactly as it owns the existing startup loaders.
    include_str!("local_shell_integration/startup.sh")
        .replace("\r\n", "\n")
        .replace(
            "__NYATERM_BASH_FULL__",
            r#". "$NYATERM_LOCAL_DIR/full.bash""#,
        )
        .replace(
            "__NYATERM_ZSH_FULL__",
            r#"source "$NYATERM_LOCAL_DIR/full.zsh""#,
        )
        .replace(
            "__NYATERM_FISH_FULL__",
            r#"source "$NYATERM_LOCAL_DIR/full.fish""#,
        )
}
