//! Runtime capabilities supplied by the application composition layer.
use crate::{SshMultiplexHandle, SshSessionConfig};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub trait SshFileSessionProbe: Send + Sync {
    fn probe(
        &self,
        config: SshSessionConfig,
        multiplex: SshMultiplexHandle,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;
}

#[derive(Clone, Default)]
pub struct SshSessionServices {
    pub file_session_probe: Option<Arc<dyn SshFileSessionProbe>>,
}

impl SshSessionServices {
    pub(crate) async fn probe_files(
        &self,
        config: &SshSessionConfig,
        multiplex: &SshMultiplexHandle,
    ) -> anyhow::Result<()> {
        let probe = self
            .file_session_probe
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("SSH file session probe is not configured"))?;
        config
            .attempt
            .until_cancelled(probe.probe(config.clone(), multiplex.clone()))
            .await
            .map_err(anyhow::Error::msg)?
    }
}

#[cfg(test)]
mod tests;
