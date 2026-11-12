//! Every SFTP data channel is opened from the operation's immutable access context.
use super::sudo::{SERVER_COMMAND, SftpSudoError, SftpSudoLease};
use super::{OpenSftpConnection, OpenSftpSession, SFTP_COMPATIBILITY_CACHE, open_sftp_channel};
use nyaterm_transport::ssh_channel::SshChannelConnection;
use nyaterm_transport::ssh_execution::sudo::{SudoError, open_sudo_exec};
use nyaterm_transport::{SshMultiplexHandle, SshSessionConfig};
use russh_sftp::client::{Config, SftpSession};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
pub(super) struct SftpSessionFactory {
    pub(super) config: SshSessionConfig,
    pub(super) multiplex: Option<SshMultiplexHandle>,
    pub(super) sudo: Option<SftpSudoLease>,
}

impl SftpSessionFactory {
    pub(super) fn new(
        config: SshSessionConfig,
        multiplex: Option<SshMultiplexHandle>,
        sudo: Option<SftpSudoLease>,
    ) -> Self {
        Self {
            config,
            multiplex,
            sudo,
        }
    }

    pub(super) async fn open(&self) -> anyhow::Result<OpenSftpSession> {
        let mut config = Config::default();
        if let Some(depth) = self.config.sftp.pipeline_depth {
            config.max_concurrent_reads = depth.clamp(4, 64) as usize;
            config.max_concurrent_writes = depth.clamp(4, 64) as usize;
        }
        self.open_with_config(config).await
    }

    pub(super) async fn open_with_config(
        &self,
        mut config: Config,
    ) -> anyhow::Result<OpenSftpSession> {
        if let Some(lease) = &self.sudo {
            lease.check()?;
        }
        let cache = SFTP_COMPATIBILITY_CACHE
            .try_with(Clone::clone)
            .ok()
            .flatten();
        if let Some(session) = cache
            .as_ref()
            .and_then(|cache| cache.lock().unwrap_or_else(|e| e.into_inner()).clone())
        {
            return Ok(session);
        }
        config.max_write_packet_len = config.max_packet_len;
        if self.config.sftp.compatibility_mode {
            config.max_concurrent_reads = 1;
            config.max_concurrent_writes = 1;
        }
        let (sftp, connection) = if let Some(lease) = &self.sudo {
            let multiplex = self.multiplex.as_ref().ok_or(SudoError::ExecUnavailable)?;
            anyhow::ensure!(
                multiplex.same_connection(lease.connection()),
                SudoError::ExecUnavailable
            );
            let channel = match open_sudo_exec(
                &self.config,
                multiplex,
                SERVER_COMMAND,
                lease.password()?,
                lease.cancellation(),
            )
            .await
            {
                Ok(channel) => channel,
                Err(error) => {
                    lease.invalidate();
                    return Err(error.into());
                }
            };
            lease.register_channel(channel.completion())?;
            let status = channel.status();
            let cancellation = lease.cancellation();
            let result = tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(SudoError::Cancelled.into()),
                result = tokio::time::timeout(Duration::from_secs(30), SftpSession::new_with_config(channel, config)) => result,
            };
            let sftp = match result {
                Ok(Ok(sftp)) => sftp,
                failure => {
                    lease.invalidate();
                    let error = *status.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(error) = error {
                        return Err(error.into());
                    }
                    return match failure {
                        Err(_) => Err(SftpSudoError::InitializationTimeout.into()),
                        Ok(Err(_)) => Err(SftpSudoError::ProtocolInitialization.into()),
                        Ok(Ok(_)) => unreachable!(),
                    };
                }
            };
            (sftp, OpenSftpConnection::Multiplex)
        } else {
            let owner =
                SshChannelConnection::connect(&self.config, self.multiplex.as_ref()).await?;
            let channel = open_sftp_channel(&owner, self.config.sftp.compatibility_mode).await?;
            if channel.request_subsystem(true, "sftp").await.is_err() {
                drop(channel.into_stream());
                let _ = owner.close("SFTP startup failed").await;
                anyhow::bail!("failed to start SFTP subsystem");
            }
            let result = tokio::time::timeout(
                Duration::from_secs(30),
                SftpSession::new_with_config(channel.into_stream(), config),
            )
            .await;
            let sftp = match result {
                Ok(Ok(sftp)) => sftp,
                failure => {
                    let _ = owner.close("SFTP initialization failed").await;
                    return match failure {
                        Err(_) => Err(anyhow::anyhow!("SFTP initialization timed out")),
                        Ok(Err(error)) => Err(error.into()),
                        Ok(Ok(_)) => unreachable!(),
                    };
                }
            };
            let connection = if self.multiplex.is_some() {
                OpenSftpConnection::Multiplex
            } else {
                OpenSftpConnection::Dedicated(owner)
            };
            (sftp, connection)
        };
        let session = OpenSftpSession {
            sftp: Arc::new(sftp),
            connection: Arc::new(Mutex::new(Some(connection))),
            persistent: cache.is_some(),
        };
        if let Some(lease) = &self.sudo {
            lease.register(&session.sftp)?;
        }
        if let Some(cache) = cache {
            *cache.lock().unwrap_or_else(|e| e.into_inner()) = Some(session.clone());
        }
        Ok(session)
    }
}
