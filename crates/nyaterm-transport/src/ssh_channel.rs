//! Channel access without exposing authentication handlers or connection locks.
use crate::{SshHandleChain, SshMultiplexHandle, SshSessionConfig, open_authenticated_ssh_handle};
use russh::{Channel, Disconnect, client};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum SshChannelOpenError {
    #[error(transparent)]
    Ssh(#[from] russh::Error),
    #[error("SSH channel open timed out")]
    TimedOut,
}

pub struct SshChannelConnection {
    inner: ChannelConnection,
}

enum ChannelConnection {
    Dedicated(SshHandleChain),
    Multiplex(SshMultiplexHandle),
}

impl SshChannelConnection {
    pub async fn connect(
        config: &SshSessionConfig,
        multiplex: Option<&SshMultiplexHandle>,
    ) -> anyhow::Result<Self> {
        let inner = if let Some(multiplex) = multiplex {
            multiplex.ensure_matches_config(config)?;
            ChannelConnection::Multiplex(multiplex.clone())
        } else {
            ChannelConnection::Dedicated(open_authenticated_ssh_handle(config).await?)
        };
        Ok(Self { inner })
    }

    /// Every auxiliary session waits for the interactive shell's first channel.
    pub async fn open_session(&self) -> Result<Channel<client::Msg>, russh::Error> {
        match &self.inner {
            ChannelConnection::Dedicated((handle, _)) => handle.channel_open_session().await,
            ChannelConnection::Multiplex(multiplex) => {
                let handle = multiplex.exec_target_handle().await;
                handle.lock().await.channel_open_session().await
            }
        }
    }

    /// The wire-request timeout excludes first-channel ordering and lock waits.
    pub async fn open_session_with_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Channel<client::Msg>, SshChannelOpenError> {
        let result = match &self.inner {
            ChannelConnection::Dedicated((handle, _)) => {
                tokio::time::timeout(timeout, handle.channel_open_session()).await
            }
            ChannelConnection::Multiplex(multiplex) => {
                let handle = multiplex.exec_target_handle().await;
                let handle = handle.lock().await;
                tokio::time::timeout(timeout, handle.channel_open_session()).await
            }
        };
        result
            .map_err(|_| SshChannelOpenError::TimedOut)?
            .map_err(Into::into)
    }

    /// Closing an auxiliary operation must never disconnect a shared connection.
    pub async fn close(&self, reason: &str) -> Result<(), russh::Error> {
        let mut result = Ok(());
        if let ChannelConnection::Dedicated((handle, jumps)) = &self.inner {
            result = handle
                .disconnect(Disconnect::ByApplication, reason, "en")
                .await;
            for jump in jumps {
                let closed = jump
                    .disconnect(Disconnect::ByApplication, reason, "en")
                    .await;
                if result.is_ok() {
                    result = closed;
                }
            }
        }
        result
    }
}
