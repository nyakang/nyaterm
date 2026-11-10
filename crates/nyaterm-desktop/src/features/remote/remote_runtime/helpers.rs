use crate::features::NyaTermApp;
use nyaterm_remote_ops::docker::RemoteDockerOverview;
use nyaterm_remote_ops::host::HostTarget;

/// Where the active session's host tools run: this machine for a local shell,
/// or the shared SSH connection for a remote one.
pub(in crate::features) struct ActiveSshRuntimeContext {
    pub(in crate::features) session_id: String,
    pub(in crate::features) target: HostTarget,
    pub(in crate::features) docker_state: nyaterm_remote_ops::docker::DockerConnectionState,
}

impl NyaTermApp {
    pub(in crate::features) fn local_host_tools_unsupported(&self) -> bool {
        cfg!(windows) && self.session.active_is_local_pty()
    }

    pub(in crate::features) fn process_control_capabilities(
        &self,
    ) -> nyaterm_remote_ops::process_control::ProcessControlCapabilities {
        use nyaterm_remote_ops::host::HostOs;
        nyaterm_remote_ops::process_control::ProcessControlCapabilities::for_os(
            if self.session.active_is_local_pty() {
                HostOs::local()
            } else {
                HostOs::OtherUnix
            },
        )
    }

    pub(in crate::features) fn active_ssh_runtime_context(
        &mut self,
        action: &str,
    ) -> Result<ActiveSshRuntimeContext, String> {
        let session_id = self
            .session
            .active_id_owned()
            .ok_or_else(|| format!("start a session before {action}"))?;
        if self.session.is_local_pty(&session_id) {
            if self.session.is_disconnected(&session_id) {
                return Err(format!("reconnect the local shell before {action}"));
            }
            return Ok(ActiveSshRuntimeContext {
                session_id,
                target: HostTarget::Local,
                docker_state: self.remote_ops.local_docker_state(),
            });
        }
        let config = self
            .session
            .active_ssh_config_owned()
            .ok_or_else(|| format!("start an SSH session before {action}"))?;
        config
            .ensure_remote_resource_probes_allowed()
            .map_err(|_| rust_i18n::t!("panel.sshCompatibilityModeUnavailable").to_string())?;
        // `session_id` may be a tmux pane leaf — the SSH connection and its
        // multiplex handle live on the host session, so look the connection
        // up through the host while keeping the pane id for job bookkeeping.
        let connection_session_id = self.ssh_scoped_session_id(&session_id);
        let has_multiplex_key = self
            .session
            .metadata(&connection_session_id)
            .and_then(|metadata| metadata.ssh_multiplex_key.as_ref())
            .is_some();
        if let Some(connection) = self
            .session
            .ssh_connection_for_session(&connection_session_id)
        {
            return Ok(ActiveSshRuntimeContext {
                session_id,
                target: HostTarget::Ssh {
                    config,
                    multiplex: Some(connection.handle()),
                },
                docker_state: connection.docker_state(),
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
