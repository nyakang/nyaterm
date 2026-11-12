use super::{CapabilityCaller, CapabilityRequest};
use crate::app_shell::session_hub::ssh_connections::{
    BackgroundConnectionLease, SshConnectionPool,
};
use crate::features::NyaTermApp;
use crate::features::mcp::server::rpc_failure;
use crate::features::session::SshSessionConfigBuildContext;
use crate::features::session::remote_files::FileAccessPurpose;
use nyaterm_core::models::connection::{ConnectionType, SshProfile};
use nyaterm_core::models::sessions::SavedConnection;
use nyaterm_files::remote_file::RemoteFileService;
use nyaterm_mcp_protocol::{CapabilityAccess, ExecutionTarget, RpcError, definition_for_tool};
use nyaterm_store::StoreDomain;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

pub(super) struct BackgroundTarget {
    connection: SavedConnection,
    context: SshSessionConfigBuildContext,
    pool: SshConnectionPool,
    command: bool,
}

impl BackgroundTarget {
    pub(super) fn acquire(
        self,
        cancellation: &CancellationToken,
        deadline: Instant,
    ) -> anyhow::Result<BackgroundConnectionLease> {
        let id = self.connection.id.clone();
        let connection = self
            .context
            .store
            .request_fn(StoreDomain::Security, move |store| {
                store.get_connection(&id)
            })?
            .ok_or_else(|| anyhow::anyhow!("The saved SSH connection was removed"))?;
        validate_saved_target(&connection, self.command)?;
        if cancellation.is_cancelled() || Instant::now() >= deadline {
            anyhow::bail!("Background operation cancelled or timed out");
        }
        let (revision, known_hosts) = self
            .context
            .store
            .request_fn(StoreDomain::Security, |store| {
                Ok((store.ssh_key_revision(), store.render_known_hosts_export()?))
            })?;
        let policy = format!(
            "{}:{revision}:{}",
            self.context.host_key_policy,
            hex::encode(Sha256::digest(known_hosts.as_bytes()))
        );
        // Configuration preparation can prompt for the master password. Stop
        // that prompt with this waiter, but never attach its cancellation to a
        // pooled transport that other operations may already be using.
        let config = std::thread::scope(|scope| {
            let (done, finished) = std::sync::mpsc::channel::<()>();
            let attempt = &self.context.attempt;
            scope.spawn(move || {
                loop {
                    if cancellation.is_cancelled() || Instant::now() >= deadline {
                        attempt.cancel();
                        break;
                    }
                    match finished.recv_timeout(Duration::from_millis(25)) {
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        _ => break,
                    }
                }
            });
            let config = self.context.build_config(&connection);
            drop(done);
            config
        })
        .map_err(anyhow::Error::msg)?;
        self.pool
            .acquire_background(&connection.id, config, policy, cancellation, deadline)
    }
}

pub(super) enum PreparedFileService {
    Session(Box<RemoteFileService>),
    Connection(Box<BackgroundTarget>),
}

impl PreparedFileService {
    pub(super) fn open(
        self,
        cancellation: &CancellationToken,
    ) -> Result<(RemoteFileService, Option<BackgroundConnectionLease>), String> {
        match self {
            Self::Session(service) => Ok((*service, None)),
            Self::Connection(target) => {
                let lease = target
                    .acquire(cancellation, Instant::now() + Duration::from_secs(300))
                    .map_err(|_| "The saved SSH connection could not be opened.".to_string())?;
                let service =
                    RemoteFileService::with_multiplex(lease.config(), lease.connection.handle())
                        .map_err(|_| "Remote file access is unavailable.".to_string())?;
                Ok((service, Some(lease)))
            }
        }
    }
}

fn validate_saved_target(connection: &SavedConnection, command: bool) -> anyhow::Result<()> {
    let ConnectionType::Ssh {
        ai_execution_profile,
        ..
    } = &connection.config
    else {
        anyhow::bail!("Background operations require a saved SSH connection");
    };
    if command
        && !nyaterm_core::network_cli_prompt::ssh_allows_background_execution(
            *ai_execution_profile,
            connection.ssh_profile == SshProfile::NetworkDevice,
            "",
        )
    {
        anyhow::bail!("Independent command execution is disabled for this host");
    }
    Ok(())
}

impl NyaTermApp {
    pub(super) fn authorize_saved_target(
        &self,
        request: &CapabilityRequest,
        id: &str,
    ) -> Result<(), RpcError> {
        if self.security.screen_locked()
            || !matches!(request.caller, CapabilityCaller::Mcp)
            || !self.mcp.is_external_generation(&request.generation)
        {
            return Err(rpc_failure(
                "scope_denied",
                "Saved connection targets are unavailable to this credential.",
            ));
        }
        if matches!(
            request.tool.as_str(),
            nyaterm_mcp_protocol::tool::TERMINAL_POLL | nyaterm_mcp_protocol::tool::TERMINAL_STOP
        ) {
            return Ok(());
        }
        if !self
            .mcp
            .capabilities
            .terminal_executions
            .accepts_background_operations()
        {
            return Err(rpc_failure("host_unavailable", "The workspace is closing."));
        }
        if self
            .cloud_sync
            .operations
            .replacement_gate()
            .block_reason()
            .is_some()
        {
            return Err(rpc_failure(
                "host_unavailable",
                "Configuration replacement is in progress.",
            ));
        }
        let connection = self.connection_state.connection_by_id(id).ok_or_else(|| {
            rpc_failure(
                "invalid_argument",
                "The saved SSH connection does not exist.",
            )
        })?;
        validate_saved_target(
            connection,
            definition_for_tool(&request.tool).is_some_and(|definition| {
                definition.access == CapabilityAccess::Write
                    && request.tool == nyaterm_mcp_protocol::tool::TERMINAL_START
            }),
        )
        .map_err(|error| rpc_failure("permission_denied", &error.to_string()))
    }

    pub(super) fn prepare_background_target(
        &self,
        id: &str,
        command: bool,
    ) -> Result<BackgroundTarget, RpcError> {
        let connection = self
            .connection_state
            .connection_by_id(id)
            .cloned()
            .ok_or_else(|| {
                rpc_failure(
                    "invalid_argument",
                    "The saved SSH connection does not exist.",
                )
            })?;
        let mut context = self.ssh_session_config_build_context();
        context.attempt = Default::default();
        context.host_key_prompts = Arc::new(
            context
                .host_key_prompts
                .for_attempt(context.attempt.clone()),
        );
        context.credential_prompts = Arc::new(
            context
                .credential_prompts
                .for_attempt(context.attempt.clone()),
        );
        Ok(BackgroundTarget {
            connection,
            context,
            pool: self.session.ssh_connection_pool(),
            command,
        })
    }

    pub(super) fn prepare_mcp_file_service(
        &mut self,
        request: &CapabilityRequest,
    ) -> Result<PreparedFileService, RpcError> {
        if let Some(ExecutionTarget::Connection { connection_id }) = request
            .arguments
            .get("target")
            .and_then(|value| serde_json::from_value::<ExecutionTarget>(value.clone()).ok())
        {
            self.authorize_saved_target(request, &connection_id)?;
            return self
                .prepare_background_target(&connection_id, false)
                .map(|target| PreparedFileService::Connection(Box::new(target)));
        }
        let id = request
            .resolved_session_id
            .as_deref()
            .ok_or_else(|| rpc_failure("scope_denied", "A scoped session is required."))?;
        let config = self
            .session
            .metadata(id)
            .and_then(|metadata| metadata.ssh_config.clone())
            .ok_or_else(|| {
                rpc_failure("permission_denied", "SFTP is unavailable for this session.")
            })?;
        self.remote_file_service_for_session(id, config, FileAccessPurpose::Automation)
            .map(|service| PreparedFileService::Session(Box::new(service)))
            .map_err(|_| rpc_failure("execution_failed", "Remote file access is unavailable."))
    }
}

#[cfg(test)]
mod tests {
    use super::validate_saved_target;
    use nyaterm_core::models::connection::{AiExecutionProfile, ConnectionType, SshProfile};
    use nyaterm_core::models::sessions::SavedConnection;
    use serde_json::json;

    #[test]
    fn saved_commands_enforce_ssh_and_execution_policy_while_files_remain_available() {
        let mut host: SavedConnection = serde_json::from_value(json!({
            "id":"saved", "name":"fixture", "type":"ssh", "host":"fixture"
        }))
        .unwrap();
        assert!(validate_saved_target(&host, true).is_ok());
        host.ssh_profile = SshProfile::NetworkDevice;
        assert!(validate_saved_target(&host, true).is_err());
        assert!(validate_saved_target(&host, false).is_ok());
        host.ssh_profile = SshProfile::Standard;
        for profile in [AiExecutionProfile::Disabled, AiExecutionProfile::SendOnly] {
            if let ConnectionType::Ssh {
                ai_execution_profile,
                ..
            } = &mut host.config
            {
                *ai_execution_profile = profile;
            }
            assert!(validate_saved_target(&host, true).is_err());
            assert!(validate_saved_target(&host, false).is_ok());
        }
        host.config = serde_json::from_value(json!({"type":"local_terminal"})).unwrap();
        assert!(validate_saved_target(&host, true).is_err());
        assert!(validate_saved_target(&host, false).is_err());
    }
}
