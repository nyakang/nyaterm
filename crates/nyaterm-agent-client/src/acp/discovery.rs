use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::acp::AgentError;
use crate::external_process::{ExternalAgentChild, spawn_reader};

pub fn resolve(executable: &str) -> Result<PathBuf, AgentError> {
    let name = executable.trim();
    if name.is_empty() {
        return Err(AgentError::Missing);
    }
    let path = Path::new(name);
    if path.is_file() {
        return std::fs::canonicalize(path).map_err(|_| AgentError::Missing);
    }
    if path.components().count() > 1 {
        return Err(AgentError::Missing);
    }
    let roots: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).collect())
        .unwrap_or_default();
    #[cfg(windows)]
    let roots = {
        let mut roots = roots;
        for (variable, suffix) in [("APPDATA", "npm"), ("LOCALAPPDATA", "pnpm")] {
            if let Some(root) = std::env::var_os(variable) {
                roots.push(PathBuf::from(root).join(suffix));
            }
        }
        roots
    };
    for root in roots {
        for extension in if cfg!(windows) {
            &["", ".exe", ".cmd"][..]
        } else {
            &[""][..]
        } {
            let candidate = root.join(format!("{name}{extension}"));
            if candidate.is_file() {
                return std::fs::canonicalize(candidate).map_err(|_| AgentError::Missing);
            }
        }
    }
    Err(AgentError::Missing)
}

/// Only a bounded first line is returned. Diagnostics never include child output.
pub fn version(executable: &Path, cancel: &AtomicBool) -> Result<String, AgentError> {
    let mut command = Command::new(executable);
    crate::acp::policy::clear_unsafe_environment(&mut command);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = ExternalAgentChild::spawn(&mut command).map_err(|_| AgentError::Transport)?;
    let stdout = child.take_stdout().ok_or(AgentError::Transport)?;
    let stderr = child.take_stderr().ok_or(AgentError::Transport)?;
    let (tx, lines) = std::sync::mpsc::sync_channel(1);
    child.own_reader(
        spawn_reader("nyaterm-acp-version", move || {
            let mut line = Vec::new();
            let result = BufReader::new(stdout)
                .take(257)
                .read_until(b'\n', &mut line)
                .map_err(|_| ())
                .and_then(|_| {
                    if line.is_empty() || line.len() > 256 {
                        return Err(());
                    }
                    String::from_utf8(line).map_err(|_| ())
                });
            let _ = tx.send(result);
        })
        .map_err(|_| AgentError::Transport)?,
    );
    child.own_reader(
        spawn_reader("nyaterm-acp-version-stderr", move || {
            let _ = std::io::copy(&mut BufReader::new(stderr), &mut std::io::sink());
        })
        .map_err(|_| AgentError::Transport)?,
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(AgentError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(AgentError::Timeout);
        }
        match lines.recv_timeout(Duration::from_millis(50)) {
            Ok(Ok(line)) if line.len() <= 256 => return Ok(line),
            Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(AgentError::Transport);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

/// Admit the bundled release engine or npm's SDK engine. Never fall back to PATH.
pub fn claude_engine(adapter: &Path, cancel: &AtomicBool) -> Result<PathBuf, AgentError> {
    let executable_name = if cfg!(windows) {
        "claude.exe"
    } else {
        "claude"
    };
    let sibling = adapter
        .parent()
        .ok_or(AgentError::UnsupportedVersion)?
        .join(executable_name);
    let engine = if sibling.is_file() {
        sibling
    } else {
        let platform = if cfg!(windows) {
            "win32"
        } else if cfg!(target_os = "macos") {
            "darwin"
        } else {
            "linux"
        };
        let arch = if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "x64"
        };
        let package = format!("@anthropic-ai/claude-agent-sdk-{platform}-{arch}");
        // npm may install the SDK below the adapter, or hoist it to an ancestor.
        let mut candidates = Vec::new();
        for ancestor in adapter.ancestors().skip(1).take(8) {
            let modules = if ancestor
                .file_name()
                .is_some_and(|name| name == "node_modules")
            {
                ancestor.to_path_buf()
            } else {
                ancestor.join("node_modules")
            };
            candidates.push(
                modules
                    .join("@agentclientprotocol/claude-agent-acp/node_modules")
                    .join(&package)
                    .join(executable_name),
            );
            candidates.push(
                modules
                    .join("@anthropic-ai/claude-agent-sdk/node_modules")
                    .join(&package)
                    .join(executable_name),
            );
            candidates.push(modules.join(&package).join(executable_name));
        }
        candidates
            .into_iter()
            .find(|path| path.is_file())
            .ok_or(AgentError::UnsupportedVersion)?
    };
    let engine = std::fs::canonicalize(engine).map_err(|_| AgentError::UnsupportedVersion)?;
    if !crate::acp::policy::matches_version(&version(&engine, cancel)?, "2.1.330") {
        return Err(AgentError::UnsupportedVersion);
    }
    Ok(engine)
}

/// Pin the engine as well as the adapter: their approval and configuration contracts
/// evolve independently. Do not let the adapter select another Codex from PATH.
pub fn codex_engine(adapter: &Path, cancel: &AtomicBool) -> Result<PathBuf, AgentError> {
    let platform = if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    };
    let cpu = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    let target = if cfg!(windows) {
        "pc-windows-msvc"
    } else if cfg!(target_os = "macos") {
        "apple-darwin"
    } else {
        "unknown-linux-musl"
    };
    let name = if cfg!(windows) { "codex.exe" } else { "codex" };
    let mut candidates = vec![
        adapter
            .parent()
            .ok_or(AgentError::UnsupportedVersion)?
            .join(name),
    ];
    for ancestor in adapter.ancestors().skip(1).take(9) {
        let modules = if ancestor.file_name().is_some_and(|n| n == "node_modules") {
            ancestor.to_path_buf()
        } else {
            ancestor.join("node_modules")
        };
        for root in [
            modules.join("@agentclientprotocol/codex-acp/node_modules"),
            modules,
        ] {
            for package in [
                format!("@openai/codex-{platform}-{arch}"),
                "@openai/codex".into(),
            ] {
                for directory in ["bin", "codex"] {
                    candidates.push(
                        root.join(&package)
                            .join("vendor")
                            .join(format!("{cpu}-{target}"))
                            .join(directory)
                            .join(name),
                    );
                    candidates.push(
                        root.join("@openai/codex/node_modules")
                            .join(&package)
                            .join("vendor")
                            .join(format!("{cpu}-{target}"))
                            .join(directory)
                            .join(name),
                    );
                }
            }
        }
    }
    let engine = candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or(AgentError::UnsupportedVersion)?;
    let engine = std::fs::canonicalize(engine).map_err(|_| AgentError::UnsupportedVersion)?;
    if !crate::acp::policy::matches_version(
        &version(&engine, cancel)?,
        crate::acp::policy::CODEX_ENGINE_VERSION,
    ) {
        return Err(AgentError::UnsupportedVersion);
    }
    Ok(engine)
}
