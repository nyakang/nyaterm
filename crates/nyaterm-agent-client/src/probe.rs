//! Bounded CLI version and authentication probes for settings workers.
use crate::external_process::{child_process::AgentChild, spawn_reader};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};
const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub fn resolve_claude_executable(configured: Option<&str>) -> Option<PathBuf> {
    let configured = configured.map(str::trim).filter(|value| !value.is_empty());
    if let Some(path) = configured {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let names: &[&str] = if cfg!(windows) {
        &["claude.cmd", "claude.exe", "claude"]
    } else {
        &["claude"]
    };
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
            .find(|path| path.is_file())
    })
}

pub fn cli_version(path: &Path, stopping: &AtomicBool) -> Option<String> {
    cli_output(path, &["--version"], stopping)?
        .lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
}

pub fn claude_auth_status(configured: Option<&str>, stopping: &AtomicBool) -> bool {
    resolve_claude_executable(configured)
        .and_then(|path| cli_output(&path, &["auth", "status", "--json"], stopping))
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .is_some_and(|value| value["loggedIn"] == true)
}

fn cli_output(path: &Path, args: &[&str], stopping: &AtomicBool) -> Option<String> {
    if stopping.load(Ordering::Acquire) {
        return None;
    }
    let mut command = Command::new(path);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hide_window(&mut command);
    let mut child = AgentChild::spawn(&mut command).ok()?;
    let stdout = child.child_mut().stdout.take()?;
    let (output_tx, output_rx) = mpsc::channel();
    let reader = spawn_reader("nyaterm-agent-version-reader", move || {
        use std::io::Read as _;
        let mut output = String::new();
        let result = stdout
            .take(64 * 1024)
            .read_to_string(&mut output)
            .ok()
            .map(|_| output);
        let _ = output_tx.send(result);
    })
    .ok()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = None;
    while !stopping.load(Ordering::Acquire) && Instant::now() < deadline {
        match child.child_mut().try_wait() {
            Ok(Some(status)) if status.success() => match output_rx.try_recv() {
                Ok(value) => {
                    output = value;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => break,
            },
            Ok(Some(_)) | Err(_) => break,
            Ok(None) => {}
        }
        std::thread::sleep(WORKER_POLL_INTERVAL);
    }
    child.terminate();
    let _ = reader.join();
    output
}

#[cfg(windows)]
pub(crate) fn hide_window(command: &mut Command) {
    use std::os::windows::process::CommandExt as _;
    command.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
pub(crate) fn hide_window(_command: &mut Command) {}
