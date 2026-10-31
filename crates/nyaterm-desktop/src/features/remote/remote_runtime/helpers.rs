use nyaterm_transport::{RemoteDockerOverview, SshMultiplexHandle, SshSessionConfig};

use crate::features::NyaTermApp;

pub(in crate::features) struct ActiveSshRuntimeContext {
    pub(in crate::features) session_id: String,
    pub(in crate::features) config: SshSessionConfig,
    pub(in crate::features) multiplex: SshMultiplexHandle,
}

impl NyaTermApp {
    pub(in crate::features) fn active_ssh_runtime_context(
        &mut self,
        action: &str,
    ) -> Result<ActiveSshRuntimeContext, String> {
        let session_id = self
            .session
            .active_id_owned()
            .ok_or_else(|| format!("start an SSH session before {action}"))?;
        let config = self
            .session
            .active_ssh_config_owned()
            .ok_or_else(|| format!("start an SSH session before {action}"))?;
        // `session_id` may be a tmux pane leaf — the SSH connection and its
        // multiplex handle live on the host session, so look the connection
        // up through the host while keeping the pane id for job bookkeeping.
        let connection_session_id = self.ssh_scoped_session_id(&session_id);
        let has_multiplex_key = self
            .session
            .metadata(&connection_session_id)
            .and_then(|metadata| metadata.ssh_multiplex_key.as_ref())
            .is_some();
        if let Some(multiplex) = self
            .session
            .ssh_multiplex_handle_for_session(&connection_session_id)
        {
            return Ok(ActiveSshRuntimeContext {
                session_id,
                config,
                multiplex,
            });
        }
        if has_multiplex_key {
            Err(format!("reconnect the SSH session before {action}"))
        } else {
            Err(format!(
                "reconnect this SSH session to enable shared transport before {action}"
            ))
        }
    }
}

pub(super) const DOCKER_SHELL_SELECTOR: &str = "if command -v bash >/dev/null 2>&1; then exec bash; elif command -v zsh >/dev/null 2>&1; then exec zsh; elif command -v fish >/dev/null 2>&1; then exec fish; elif command -v ash >/dev/null 2>&1; then exec ash; else exec sh; fi";

pub(super) fn docker_compose_terminal_base(
    project_name: &str,
    config_files: Option<&str>,
) -> String {
    let mut command = String::from("docker compose");
    for file in config_files.unwrap_or_default().split(',') {
        let file = file.trim();
        if !file.is_empty() && !file.eq_ignore_ascii_case("n/a") {
            command.push_str(" -f ");
            command.push_str(&shell_quote(file));
        }
    }
    command.push_str(" -p ");
    command.push_str(&shell_quote(project_name));
    command
}

pub(super) fn docker_overview_status(overview: &RemoteDockerOverview) -> String {
    if overview.available {
        format!(
            "Docker {} · {} container(s)",
            if overview.version.trim().is_empty() {
                "available".to_string()
            } else {
                overview.version.clone()
            },
            overview.containers.len()
        )
    } else {
        "Docker is not available on this SSH host".to_string()
    }
}

pub(super) fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
