//! Connection-bound, revocable authorization for manually requested file operations.
use nyaterm_core::SecretString;
use nyaterm_transport::SshMultiplexHandle;
use nyaterm_transport::ssh_execution::sudo::SudoError;
use russh_sftp::client::SftpSession;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub(super) const SERVER_COMMAND: &str = concat!(
    "for server in /usr/lib/openssh/sftp-server /usr/libexec/openssh/sftp-server ",
    "/usr/lib/ssh/sftp-server /usr/libexec/sftp-server ",
    "/usr/local/libexec/sftp-server /usr/local/lib/sftp-server; do ",
    "if [ -x \"$server\" ]; then exec \"$server\"; fi; done; exit 127"
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SftpSudoError {
    #[error("privileged SFTP initialization timed out")]
    InitializationTimeout,
    #[error(
        "privileged SFTP protocol initialization failed; verify OpenSSH sftp-server and sudo policy"
    )]
    ProtocolInitialization,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FileAccessMode {
    #[default]
    LoginUser,
    SudoRoot,
}

struct Authorization {
    connection: SshMultiplexHandle,
    password: Mutex<Option<SecretString>>,
    cancellation: CancellationToken,
    sessions: Mutex<Vec<Weak<SftpSession>>>,
    channels: Mutex<Vec<CancellationToken>>,
}

/// The desktop session owns this value; dropping it revokes every borrowed lease.
pub struct SftpSudoGrant(Arc<Authorization>);

#[derive(Clone)]
pub struct SftpSudoLease(Arc<Authorization>);

impl std::fmt::Debug for SftpSudoLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SftpSudoLease")
            .field("revoked", &self.is_revoked())
            .finish_non_exhaustive()
    }
}

impl SftpSudoGrant {
    pub fn new(connection: SshMultiplexHandle, password: Option<SecretString>) -> Self {
        Self(Arc::new(Authorization {
            connection,
            password: Mutex::new(password.filter(|value| !value.is_empty())),
            cancellation: CancellationToken::new(),
            sessions: Mutex::new(Vec::new()),
            channels: Mutex::new(Vec::new()),
        }))
    }

    pub fn lease(&self) -> SftpSudoLease {
        SftpSudoLease(Arc::clone(&self.0))
    }

    pub fn revoke(&self) {
        self.lease().invalidate();
    }

    /// Called on a background worker before publishing a completed mode switch.
    pub fn close(&self) {
        self.revoke();
        let sessions = self
            .0
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .drain(..)
            .filter_map(|session| session.upgrade())
            .collect::<Vec<_>>();
        let channels = std::mem::take(
            &mut *self
                .0
                .channels
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        let _ = self.0.connection.block_on(async move {
            let _ = tokio::time::timeout(Duration::from_secs(5), async move {
                for session in sessions {
                    let _ = session.close().await;
                }
                for channel in channels {
                    channel.cancelled().await;
                }
            })
            .await;
            Ok(())
        });
    }
}

impl Drop for SftpSudoGrant {
    fn drop(&mut self) {
        self.revoke();
    }
}

impl SftpSudoLease {
    pub fn is_revoked(&self) -> bool {
        self.0.cancellation.is_cancelled() || self.0.connection.is_closed()
    }

    pub fn check(&self) -> Result<(), SudoError> {
        if self.is_revoked() {
            Err(SudoError::Cancelled)
        } else {
            Ok(())
        }
    }

    pub(crate) fn same_authorization(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub(super) fn connection(&self) -> &SshMultiplexHandle {
        &self.0.connection
    }
    pub(super) fn cancellation(&self) -> CancellationToken {
        self.0.cancellation.clone()
    }
    pub(super) fn password(&self) -> Result<Option<SecretString>, SudoError> {
        let password = self
            .0
            .password
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.check()?;
        Ok(password.clone())
    }

    pub(super) fn register_channel(&self, completion: CancellationToken) -> Result<(), SudoError> {
        let mut channels = self
            .0
            .channels
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.check()?;
        channels.retain(|channel| !channel.is_cancelled());
        channels.push(completion);
        Ok(())
    }

    pub(super) fn register(&self, session: &Arc<SftpSession>) -> Result<(), SudoError> {
        let mut sessions = self
            .0
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.check()?;
        sessions.retain(|session| session.strong_count() != 0);
        sessions.push(Arc::downgrade(session));
        Ok(())
    }

    pub(super) fn invalidate(&self) {
        self.0.cancellation.cancel();
        self.0
            .password
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
    }
}

/// Revocation also interrupts waits outside the protocol, such as paused jobs
/// and connection-pool admission. A closed SSH stream alone cannot wake those.
pub(super) async fn guard_operation<T>(
    lease: Option<SftpSudoLease>,
    operation: impl std::future::Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    let Some(lease) = lease else {
        return operation.await;
    };
    lease.check()?;
    let cancellation = lease.cancellation();
    tokio::select! {
        biased;
        result = operation => {
            if result.is_ok() { lease.check()?; }
            result
        },
        _ = cancellation.cancelled() => Err(SudoError::Cancelled.into()),
    }
}
