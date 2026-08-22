//! Platform SSH Agent adapters used for authentication and interactive forwarding.

use std::sync::Arc;
use std::time::{Duration, Instant};

use russh::keys::agent::client::{AgentClient, AgentStream};

use crate::{EnvironmentValue, ShellEnvironmentCache, SshAgentEndpoint};

pub(super) type DynamicAgentStream = Box<dyn AgentStream + Send + Unpin + 'static>;
pub(super) type DynamicAgentClient = AgentClient<DynamicAgentStream>;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
// Covers one login-shell lookup, a stale-socket refresh, and the socket retry.
pub(crate) const AGENT_CONNECTION_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(windows)]
const WINDOWS_OPENSSH_AGENT_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";

pub(crate) async fn connect_agent_client_with_environment_until(
    endpoint: &SshAgentEndpoint,
    environment: Option<Arc<ShellEnvironmentCache>>,
    deadline: Instant,
) -> anyhow::Result<DynamicAgentClient> {
    Ok(AgentClient::connect(
        connect_agent_stream_with_environment_until(endpoint, environment, deadline).await?,
    ))
}

pub(crate) async fn connect_agent_stream_with_environment(
    endpoint: &SshAgentEndpoint,
    environment: Option<Arc<ShellEnvironmentCache>>,
) -> anyhow::Result<DynamicAgentStream> {
    connect_agent_stream_with_environment_until(
        endpoint,
        environment,
        Instant::now() + AGENT_CONNECTION_TIMEOUT,
    )
    .await
}

pub(crate) async fn connect_agent_stream_with_environment_until(
    endpoint: &SshAgentEndpoint,
    environment: Option<Arc<ShellEnvironmentCache>>,
    deadline: Instant,
) -> anyhow::Result<DynamicAgentStream> {
    match endpoint {
        SshAgentEndpoint::Auto => connect_auto(environment, deadline).await,
        SshAgentEndpoint::Environment { variable } => {
            let path = resolve_environment_path(variable, environment.clone(), deadline).await?;
            #[cfg(unix)]
            {
                connect_unix_with_refresh(variable, path, environment, deadline).await
            }
            #[cfg(windows)]
            {
                connect_windows_named_pipe_with_refresh(variable, path, environment, deadline).await
            }
            #[cfg(not(any(unix, windows)))]
            {
                let _ = (variable, path, environment);
                anyhow::bail!("environment SSH Agent endpoints are unsupported on this platform")
            }
        }
        SshAgentEndpoint::UnixSocket { path } => {
            #[cfg(unix)]
            {
                anyhow::ensure!(!path.trim().is_empty(), "SSH Agent socket path is empty");
                connect_unix(std::path::Path::new(path), deadline).await
            }
            #[cfg(not(unix))]
            {
                let _ = path;
                anyhow::bail!("Unix SSH Agent sockets require Unix")
            }
        }
        SshAgentEndpoint::Pageant => connect_pageant(deadline).await,
        SshAgentEndpoint::WindowsOpenSsh => connect_windows_openssh(deadline).await,
    }
}

#[cfg(unix)]
async fn connect_auto(
    environment: Option<Arc<ShellEnvironmentCache>>,
    deadline: Instant,
) -> anyhow::Result<DynamicAgentStream> {
    let path = resolve_environment_path("SSH_AUTH_SOCK", environment.clone(), deadline).await?;
    connect_unix_with_refresh("SSH_AUTH_SOCK", path, environment, deadline).await
}

#[cfg(windows)]
async fn connect_auto(
    _environment: Option<Arc<ShellEnvironmentCache>>,
    deadline: Instant,
) -> anyhow::Result<DynamicAgentStream> {
    match connect_windows_openssh(deadline).await {
        Ok(stream) => Ok(stream),
        Err(_) => connect_pageant(deadline).await,
    }
}

#[cfg(not(any(unix, windows)))]
async fn connect_auto(
    _environment: Option<Arc<ShellEnvironmentCache>>,
    _deadline: Instant,
) -> anyhow::Result<DynamicAgentStream> {
    anyhow::bail!("SSH Agent is unsupported on this platform")
}

async fn resolve_environment_path(
    variable: &str,
    environment: Option<Arc<ShellEnvironmentCache>>,
    deadline: Instant,
) -> anyhow::Result<EnvironmentValue> {
    if let Some(environment) = environment {
        return environment
            .resolve_until(variable, deadline)
            .await
            .map_err(|error| anyhow::anyhow!(error))?
            .ok_or_else(|| anyhow::anyhow!("SSH Agent environment variable is not set"));
    }
    let variable = crate::normalize_environment_variable_name(variable)
        .map_err(|error| anyhow::anyhow!(error))?;
    std::env::var(variable)
        .map(EnvironmentValue::new)
        .map_err(|_| anyhow::anyhow!("SSH Agent environment variable is not set"))
}

#[cfg(unix)]
async fn connect_unix_with_refresh(
    variable: &str,
    path: EnvironmentValue,
    environment: Option<Arc<ShellEnvironmentCache>>,
    deadline: Instant,
) -> anyhow::Result<DynamicAgentStream> {
    match connect_unix(std::path::Path::new(path.as_str()), deadline).await {
        Ok(stream) => Ok(stream),
        Err(_) => {
            let Some(environment) = environment else {
                anyhow::bail!("SSH Agent socket connection failed");
            };
            let Some(refreshed) = environment
                .refresh_until(variable, deadline)
                .await
                .map_err(|refresh_error| anyhow::anyhow!(refresh_error))?
            else {
                anyhow::bail!("SSH Agent environment variable is not set");
            };
            connect_unix(std::path::Path::new(refreshed.as_str()), deadline)
                .await
                .map_err(|_| anyhow::anyhow!("SSH Agent socket connection failed after refresh"))
        }
    }
}

#[cfg(windows)]
async fn connect_windows_named_pipe_with_refresh(
    variable: &str,
    path: EnvironmentValue,
    environment: Option<Arc<ShellEnvironmentCache>>,
    deadline: Instant,
) -> anyhow::Result<DynamicAgentStream> {
    match connect_windows_named_pipe(path.as_str(), deadline).await {
        Ok(stream) => Ok(stream),
        Err(_) => {
            let Some(environment) = environment else {
                anyhow::bail!("Windows SSH Agent named pipe connection failed");
            };
            let Some(refreshed) = environment
                .refresh_until(variable, deadline)
                .await
                .map_err(|refresh_error| anyhow::anyhow!(refresh_error))?
            else {
                anyhow::bail!("SSH Agent environment variable is not set");
            };
            connect_windows_named_pipe(refreshed.as_str(), deadline)
                .await
                .map_err(|_| {
                    anyhow::anyhow!("Windows SSH Agent named pipe connection failed after refresh")
                })
        }
    }
}

#[cfg(unix)]
async fn connect_unix(
    path: &std::path::Path,
    deadline: Instant,
) -> anyhow::Result<DynamicAgentStream> {
    let stream = tokio::time::timeout(
        remaining_connect_timeout(deadline)?,
        tokio::net::UnixStream::connect(path),
    )
    .await
    .map_err(|_| anyhow::anyhow!("SSH Agent connection timed out"))?
    .map_err(|_| anyhow::anyhow!("SSH Agent socket connection failed"))?;
    Ok(Box::new(stream))
}

#[cfg(windows)]
async fn connect_windows_openssh(deadline: Instant) -> anyhow::Result<DynamicAgentStream> {
    connect_windows_named_pipe(WINDOWS_OPENSSH_AGENT_PIPE, deadline).await
}

#[cfg(windows)]
async fn connect_windows_named_pipe(
    path: &str,
    deadline: Instant,
) -> anyhow::Result<DynamicAgentStream> {
    use std::ffi::OsStr;
    let client = tokio::time::timeout(
        remaining_connect_timeout(deadline)?,
        AgentClient::connect_named_pipe(OsStr::new(path)),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Windows OpenSSH Agent connection timed out"))?
    .map_err(|_| anyhow::anyhow!("Windows SSH Agent named pipe connection failed"))?;
    Ok(Box::new(client.into_inner()))
}

#[cfg(not(windows))]
async fn connect_windows_openssh(_deadline: Instant) -> anyhow::Result<DynamicAgentStream> {
    anyhow::bail!("Windows OpenSSH Agent is available only on Windows")
}

#[cfg(windows)]
async fn connect_pageant(deadline: Instant) -> anyhow::Result<DynamicAgentStream> {
    let client = tokio::time::timeout(
        remaining_connect_timeout(deadline)?,
        AgentClient::connect_pageant(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Pageant connection timed out"))??;
    Ok(Box::new(client.into_inner()))
}

#[cfg(not(windows))]
async fn connect_pageant(_deadline: Instant) -> anyhow::Result<DynamicAgentStream> {
    anyhow::bail!("Pageant is available only on Windows")
}

fn remaining_connect_timeout(deadline: Instant) -> anyhow::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .map(|remaining| remaining.min(CONNECT_TIMEOUT))
        .ok_or_else(|| anyhow::anyhow!("SSH Agent connection timed out"))
}
