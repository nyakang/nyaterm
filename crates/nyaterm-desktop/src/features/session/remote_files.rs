//! One mutable owner for a live session's manual file authorization.
use nyaterm_core::SecretString;
use nyaterm_files::remote_file::RemoteFileService;
use nyaterm_files::sftp::sudo::{FileAccessMode, SftpSudoGrant, SftpSudoLease};
use nyaterm_transport::SshMultiplexHandle;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicU64, Ordering},
};

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum FileAccessPurpose {
    Interactive,
    Automation,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::features) enum FilePrivilegeStatus {
    #[default]
    Normal,
    Authorizing,
    Elevated,
    Unavailable,
    Disabling,
}

impl FilePrivilegeStatus {
    pub(in crate::features) fn is_sudo(self) -> bool {
        matches!(self, Self::Elevated | Self::Unavailable | Self::Disabling)
    }
    pub(in crate::features) fn is_busy(self) -> bool {
        matches!(self, Self::Authorizing | Self::Disabling)
    }
}

pub(super) struct RemoteFilesSession {
    ordinary: RemoteFileService,
    interactive: Arc<RemoteFileService>,
    grant: Option<SftpSudoGrant>,
    generation: u64,
    transition: Option<bool>,
}

impl RemoteFilesSession {
    pub(super) fn new(ordinary: RemoteFileService) -> Self {
        Self {
            interactive: Arc::new(ordinary.clone()),
            ordinary,
            grant: None,
            generation: NEXT_GENERATION.fetch_add(1, Ordering::Relaxed),
            transition: None,
        }
    }
    pub(super) fn service(&self, purpose: FileAccessPurpose) -> anyhow::Result<RemoteFileService> {
        if purpose == FileAccessPurpose::Automation {
            return Ok(self.ordinary.clone());
        }
        anyhow::ensure!(self.transition.is_none(), "file privileges are changing");
        anyhow::ensure!(
            self.interactive.authorization_is_valid(),
            "sudo authorization has expired; disable sudo or authorize again"
        );
        Ok(self.interactive.as_ref().clone())
    }
    pub(super) fn weak(&self) -> Weak<RemoteFileService> {
        Arc::downgrade(&self.interactive)
    }
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }
    pub(super) fn status(&self) -> FilePrivilegeStatus {
        match self.transition {
            Some(true) => FilePrivilegeStatus::Authorizing,
            Some(false) => FilePrivilegeStatus::Disabling,
            None if self.interactive.access_mode() == FileAccessMode::LoginUser => {
                FilePrivilegeStatus::Normal
            }
            None if self.interactive.authorization_is_valid() => FilePrivilegeStatus::Elevated,
            None => FilePrivilegeStatus::Unavailable,
        }
    }
    pub(super) fn begin(
        &mut self,
        connection: SshMultiplexHandle,
        password: Option<SecretString>,
    ) -> anyhow::Result<(u64, SftpSudoLease)> {
        anyhow::ensure!(
            self.status() == FilePrivilegeStatus::Normal,
            "file privileges are already changing or elevated"
        );
        self.generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        let grant = SftpSudoGrant::new(connection, password);
        let lease = grant.lease();
        self.grant = Some(grant);
        self.transition = Some(true);
        Ok((self.generation, lease))
    }
    pub(super) fn finish(&mut self, generation: u64, service: Option<RemoteFileService>) -> bool {
        if self.generation != generation || self.transition != Some(true) {
            return false;
        }
        self.transition = None;
        match service {
            Some(service) if service.authorization_is_valid() => {
                self.interactive = Arc::new(service)
            }
            _ => {
                self.grant.take();
            }
        }
        true
    }
    pub(super) fn cancel(&mut self, generation: u64) {
        if self.generation == generation && self.transition == Some(true) {
            self.grant.take();
            self.transition = None;
            self.generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub(super) fn disable(&mut self) -> anyhow::Result<(u64, SftpSudoGrant)> {
        anyhow::ensure!(self.transition.is_none(), "file privileges are changing");
        let grant = self
            .grant
            .take()
            .ok_or_else(|| anyhow::anyhow!("sudo is not enabled"))?;
        grant.revoke();
        self.generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        self.transition = Some(false);
        Ok((self.generation, grant))
    }
    pub(super) fn finish_disable(&mut self, generation: u64) -> bool {
        if self.generation != generation || self.transition != Some(false) {
            return false;
        }
        self.interactive = Arc::new(self.ordinary.clone());
        self.transition = None;
        true
    }
    pub(super) fn cancel_pending(&mut self) {
        if self.transition == Some(false) {
            self.finish_disable(self.generation);
        } else {
            self.cancel(self.generation);
        }
    }
}

#[cfg(test)]
pub(in crate::features) mod tests;
