//! The file capability used before an SSH file session becomes ready.
use nyaterm_transport::ssh_services::SshFileSessionProbe;
use nyaterm_transport::{SshMultiplexHandle, SshSessionConfig};
use std::future::Future;
use std::pin::Pin;

pub struct SftpSessionProbe;

impl SshFileSessionProbe for SftpSessionProbe {
    fn probe(
        &self,
        config: SshSessionConfig,
        multiplex: SshMultiplexHandle,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>> {
        Box::pin(async move { crate::sftp::probe_sftp_session(&config, &multiplex).await })
    }
}
