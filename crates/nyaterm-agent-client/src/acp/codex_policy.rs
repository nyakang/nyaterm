//! Version-specific Codex admission. No configuration files or saved credentials are edited.
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use nyaterm_core::ai::external_agents::ExternalAgentProfile;
use serde_json::{Value, json};

use crate::acp::{AgentError, policy};
use crate::external_process::{ExternalAgentChild, spawn_reader};

pub(super) const SERVER_NAME: &str = "nyaterm_embedded";

// Apply before initialize, not only thread/start: plugins and hooks may have startup work.
const STARTUP_OVERRIDES: &[&str] = &[
    "features.shell_tool=false",
    "features.unified_exec=false",
    "features.apply_patch_freeform=false",
    "features.js_repl=false",
    "features.code_mode=false",
    "features.code_mode_host=false",
    "features.multi_agent=false",
    "features.multi_agent_v2=false",
    "features.collab=false",
    "features.apps=false",
    "features.connectors=false",
    "features.plugins=false",
    "features.hooks=false",
    "features.codex_hooks=false",
    "features.plugin_hooks=false",
    "features.browser_use=false",
    "features.computer_use=false",
    "features.image_generation=false",
    "features.view_image=false",
    "features.memory_tool=false",
    "features.memories=false",
    "features.shell_snapshot=false",
    "features.shell_snapshot_v2=false",
    "features.goals=false",
    "features.default_mode_request_user_input=false",
    "features.collaboration_modes=false",
    "features.request_permissions=false",
    "features.request_permissions_tool=false",
    "web_search=\"disabled\"",
    "notify=[]",
    "tools.experimental_request_user_input.enabled=false",
    "tools.update_plan.enabled=false",
];

pub(super) struct CodexLaunch {
    // The shim contains only the verified engine path and static restrictions, never credentials.
    _directory: tempfile::TempDir,
    shim: PathBuf,
    config: Value,
}

impl CodexLaunch {
    pub(super) fn prepare(
        profile: &ExternalAgentProfile,
        engine: &Path,
        cwd: &Path,
        cancel: &AtomicBool,
    ) -> Result<Self, AgentError> {
        let effective = read_config(profile, engine, cwd, cancel)?;
        let mut config = session_config(&effective)?;
        // Keep the host's existing restrictions and replace only the MCP server map.
        let mut base = Command::new(engine);
        policy::configure(&mut base, profile)?;
        let base = base
            .get_envs()
            .find(|(key, _)| *key == "CODEX_CONFIG")
            .and_then(|(_, value)| value)
            .and_then(|v| v.to_str())
            .ok_or(AgentError::PolicyUnavailable)?;
        let mut base: Value =
            serde_json::from_str(base).map_err(|_| AgentError::PolicyUnavailable)?;
        base["mcp_servers"] = config.take();
        base["notify"] = json!([]);
        let directory = tempfile::Builder::new()
            .prefix("nyaterm-codex-policy-")
            .tempdir()
            .map_err(|_| AgentError::Transport)?;
        let shim = directory
            .path()
            .join(if cfg!(windows) { "codex.cmd" } else { "codex" });
        std::fs::write(&shim, shim_source(engine)?).map_err(|_| AgentError::Transport)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| AgentError::Transport)?;
        }
        Ok(Self {
            _directory: directory,
            shim,
            config: base,
        })
    }

    pub(super) fn configure(&self, command: &mut Command) {
        command
            .env("CODEX_PATH", &self.shim)
            .env("CODEX_CONFIG", self.config.to_string());
    }
}

fn session_config(effective: &Value) -> Result<Value, AgentError> {
    let config = effective
        .get("config")
        .and_then(Value::as_object)
        .ok_or(AgentError::PolicyUnavailable)?;
    let mut servers = serde_json::Map::new();
    // ACP marks the requested workspace trusted. Include currently ignored layers,
    // because their servers can become effective when the adapter starts the thread.
    let layers = effective["layers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|layer| layer["config"].as_object());
    for config in std::iter::once(config).chain(layers) {
        if let Some(existing) = config.get("mcp_servers") {
            let existing = existing.as_object().ok_or(AgentError::PolicyUnavailable)?;
            // Never deep-merge our credential or transport with a user-defined server of the same name.
            if existing.contains_key(SERVER_NAME) {
                return Err(AgentError::PolicyUnavailable);
            }
            for name in existing.keys() {
                servers.insert(name.clone(), json!({"enabled": false}));
            }
        }
    }
    Ok(Value::Object(servers))
}

fn shim_source(engine: &Path) -> Result<String, AgentError> {
    let engine = engine
        .to_str()
        .ok_or(AgentError::Configuration("Invalid Codex executable path"))?;
    if engine.contains(['\r', '\n', '\0']) {
        return Err(AgentError::Configuration("Invalid Codex executable path"));
    }
    #[cfg(windows)]
    {
        // No delayed expansion; double percent signs in a batch literal. The verified
        // engine is a native executable, so no CALL reparsing is needed.
        if engine.contains('"') {
            return Err(AgentError::Configuration("Invalid Codex executable path"));
        }
        let flags = STARTUP_OVERRIDES
            .iter()
            .map(|flag| format!(" -c \"{}\"", flag.replace('"', "\\\"")))
            .collect::<String>();
        Ok(format!(
            "@echo off\r\nsetlocal DisableDelayedExpansion\r\n\"{}\" %*{flags}\r\nexit /b %errorlevel%\r\n",
            engine.replace('%', "%%")
        ))
    }
    #[cfg(not(windows))]
    {
        let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
        let flags = STARTUP_OVERRIDES
            .iter()
            .map(|flag| format!(" -c {}", quote(flag)))
            .collect::<String>();
        Ok(format!("#!/bin/sh\nexec {} \"$@\"{flags}\n", quote(engine)))
    }
}

fn read_config(
    profile: &ExternalAgentProfile,
    engine: &Path,
    cwd: &Path,
    cancel: &AtomicBool,
) -> Result<Value, AgentError> {
    let mut command = Command::new(engine);
    policy::configure(&mut command, profile)?;
    command
        .arg("app-server")
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for flag in STARTUP_OVERRIDES {
        command.args(["-c", flag]);
    }
    let mut child = ExternalAgentChild::spawn(&mut command).map_err(|_| AgentError::Transport)?;
    let mut stdin = child.take_stdin().ok_or(AgentError::Transport)?;
    let stdout = child.take_stdout().ok_or(AgentError::Transport)?;
    let (tx, rx) = mpsc::sync_channel(16);
    child.own_reader(spawn_reader("nyaterm-codex-policy", move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut bytes = Vec::new();
            if !matches!(reader.by_ref().take(8 * 1024 * 1024 + 1).read_until(b'\n', &mut bytes), Ok(n) if n > 0 && n <= 8 * 1024 * 1024) { break; }
            let value = serde_json::from_slice::<Value>(&bytes).map_err(|_| AgentError::Protocol("config encoding"));
            if tx.send(value).is_err() { break; }
        }
    }).map_err(|_| AgentError::Transport)?);
    let deadline = Instant::now() + Duration::from_secs(20);
    let result = (|| {
        for (id, method, params) in [
            (
                1,
                "initialize",
                json!({"clientInfo":{"name":"nyaterm","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}),
            ),
            (2, "config/read", json!({"cwd":cwd,"includeLayers":true})),
        ] {
            serde_json::to_writer(
                &mut stdin,
                &json!({"id":id,"method":method,"params":params}),
            )
            .map_err(|_| AgentError::Transport)?;
            stdin
                .write_all(b"\n")
                .and_then(|_| stdin.flush())
                .map_err(|_| AgentError::Transport)?;
            loop {
                if cancel.load(Ordering::Acquire) {
                    return Err(AgentError::Cancelled);
                }
                if Instant::now() >= deadline {
                    return Err(AgentError::Timeout);
                }
                let response = match rx.recv_timeout(Duration::from_millis(25)) {
                    Ok(value) => value?,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => return Err(AgentError::Transport),
                };
                if response["id"] != id {
                    continue;
                }
                if response.get("error").is_some() {
                    return Err(AgentError::PolicyUnavailable);
                }
                if id == 2 {
                    return response
                        .get("result")
                        .cloned()
                        .ok_or(AgentError::PolicyUnavailable);
                }
                stdin
                    .write_all(b"{\"method\":\"initialized\"}\n")
                    .and_then(|_| stdin.flush())
                    .map_err(|_| AgentError::Transport)?;
                break;
            }
        }
        Err(AgentError::PolicyUnavailable)
    })();
    // Release the bounded channel before joining the reader during child cleanup.
    drop(rx);
    result
}

#[cfg(test)]
mod tests {
    use super::{SERVER_NAME, session_config, shim_source};
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn inherited_servers_are_disabled_and_reserved_identity_cannot_be_merged() {
        let config = session_config(&json!({"config":{"mcp_servers":{"nyaterm":{"url":"https://fixture.invalid"},"other":{"command":"forbidden"}}}})).unwrap();
        assert_eq!(
            config,
            json!({"nyaterm":{"enabled":false},"other":{"enabled":false}})
        );
        assert!(
            session_config(
                &json!({"config":{"mcp_servers":{SERVER_NAME:{"command":"forbidden"}}}})
            )
            .is_err()
        );
        assert!(session_config(&json!({})).is_err());
        assert_eq!(
            session_config(&json!({"config":{},"layers":[{"config":{"mcp_servers":{"untrusted_project":{"command":"forbidden"}}}}]})).unwrap(),
            json!({"untrusted_project":{"enabled":false}})
        );
    }

    #[test]
    fn launcher_quotes_engine_path_without_putting_credentials_in_arguments() {
        assert!(shim_source(Path::new("/fixture/bad\npath")).is_err());
        let source = shim_source(Path::new("/fixture/some path/codex")).unwrap();
        assert!(source.contains("features.plugins=false"));
        assert!(!source.contains("TOKEN"));
    }
}
