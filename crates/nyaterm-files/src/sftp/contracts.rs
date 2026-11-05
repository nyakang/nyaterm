use super::SFTP_TRANSFER_CANCELLED;
use super::remote_metadata::remote_parent;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SftpFileType {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SftpFileEntry {
    pub name: String,
    pub path: String,
    pub file_type: SftpFileType,
    pub size: Option<u64>,
    pub permissions: Option<u32>,
    pub owner: String,
    pub group: String,
    pub modified_at: Option<u32>,
    pub raw_path_token: Option<String>,
    pub symlink_target_is_directory: bool,
}

impl SftpFileEntry {
    pub fn is_directory(&self) -> bool {
        self.file_type == SftpFileType::Directory || self.symlink_target_is_directory
    }

    pub fn is_symlink(&self) -> bool {
        self.file_type == SftpFileType::Symlink
    }

    pub fn remote_path(&self) -> RemoteFilePath {
        RemoteFilePath {
            display_path: self.path.clone(),
            raw_path_token: self.raw_path_token.clone(),
        }
    }

    pub fn identity_key(&self) -> String {
        self.remote_path().identity_key()
    }

    pub fn matches_identity(&self, identity: &str) -> bool {
        self.path == identity || self.identity_key() == identity
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SftpFileProperties {
    pub symlink_target: Option<String>,
    pub name: String,
    pub path: String,
    pub file_type: SftpFileType,
    pub size: Option<u64>,
    pub permissions: Option<u32>,
    pub permissions_symbolic: String,
    pub owner: String,
    pub group: String,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub modified_at: Option<u32>,
    pub accessed_at: Option<u32>,
    pub raw_path_token: Option<String>,
    pub symlink_target_is_directory: bool,
}

impl SftpFileProperties {
    pub fn is_directory(&self) -> bool {
        self.file_type == SftpFileType::Directory || self.symlink_target_is_directory
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RemoteFilePath {
    pub display_path: String,
    pub raw_path_token: Option<String>,
}

impl RemoteFilePath {
    pub fn new(display_path: impl Into<String>) -> Self {
        Self {
            display_path: display_path.into(),
            raw_path_token: None,
        }
    }

    pub fn from_raw(display_path: impl Into<String>, raw_path: &[u8]) -> Self {
        Self {
            display_path: display_path.into(),
            raw_path_token: Some(URL_SAFE_NO_PAD.encode(raw_path)),
        }
    }

    pub fn raw_path(&self) -> anyhow::Result<Option<Vec<u8>>> {
        self.raw_path_token
            .as_deref()
            .map(|token| {
                URL_SAFE_NO_PAD
                    .decode(token)
                    .map_err(|error| anyhow::anyhow!("invalid remote path token: {error}"))
            })
            .transpose()
    }

    pub fn identity_key(&self) -> String {
        self.raw_path_token
            .as_ref()
            .map(|token| format!("raw-path-token:{token}"))
            .unwrap_or_else(|| self.display_path.clone())
    }

    pub fn parent(&self) -> anyhow::Result<Self> {
        let display_path = remote_parent(&self.display_path).to_string();
        let Some(mut raw_path) = self.raw_path()? else {
            return Ok(Self::new(display_path));
        };
        while raw_path.len() > 1 && raw_path.last() == Some(&b'/') {
            raw_path.pop();
        }
        match raw_path.iter().rposition(|byte| *byte == b'/') {
            Some(0) => raw_path.truncate(1),
            Some(index) => raw_path.truncate(index),
            None => return Ok(Self::new(display_path)),
        }
        Ok(Self::from_raw(display_path, &raw_path))
    }
}

impl From<&str> for RemoteFilePath {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SftpAttributeUpdate {
    pub symlink_target: Option<String>,
    pub mode: Option<u32>,
    pub owner: Option<String>,
    pub group: Option<String>,
    pub recursive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SftpRemoteTextFile {
    pub path: String,
    pub content: String,
    pub size: u64,
    pub modified_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteBinaryFile {
    pub path: String,
    pub content_bytes: Vec<u8>,
    pub size: u64,
    pub modified_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SftpWriteTextResult {
    Saved { modified_at: u64, size: u64 },
    Conflict { modified_at: u64, size: u64 },
}

#[derive(Debug, Clone, Default)]
pub struct SftpTransferControl {
    cancelled: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    parent: Option<Arc<SftpTransferControl>>,
}

impl SftpTransferControl {
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(false)),
            parent: None,
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.paused.store(false, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
            || self
                .parent
                .as_ref()
                .is_some_and(|parent| parent.is_cancelled())
    }

    pub fn pause(&self) {
        if !self.is_cancelled() {
            self.paused.store(true, Ordering::Relaxed);
        }
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
            || self
                .parent
                .as_ref()
                .is_some_and(|parent| parent.is_paused())
    }

    /// A stream may stop independently without cancelling the directory that owns it.
    pub fn child(&self) -> Self {
        Self {
            parent: Some(Arc::new(self.clone())),
            ..Self::new()
        }
    }

    pub fn check_cancelled(&self) -> anyhow::Result<()> {
        if self.is_cancelled() {
            anyhow::bail!(SFTP_TRANSFER_CANCELLED);
        }
        Ok(())
    }

    /// Wait for a paused operation on a worker thread, honoring cancellation.
    pub fn wait_if_paused_blocking(&self) -> anyhow::Result<()> {
        self.check_cancelled()?;
        while self.is_paused() {
            std::thread::sleep(Duration::from_millis(100));
            self.check_cancelled()?;
        }
        Ok(())
    }

    pub(crate) async fn until_cancelled<T>(
        &self,
        operation: impl std::future::Future<Output = T>,
    ) -> anyhow::Result<T> {
        self.check_cancelled()?;
        tokio::select! {
            result = operation => Ok(result),
            _ = async {
                while !self.is_cancelled() { tokio::time::sleep(Duration::from_millis(25)).await; }
            } => Err(anyhow::anyhow!(SFTP_TRANSFER_CANCELLED)),
        }
    }

    pub(crate) async fn wait_if_paused(&self) -> anyhow::Result<()> {
        self.check_cancelled()?;
        while self.is_paused() {
            tokio::time::sleep(Duration::from_millis(100)).await;
            self.check_cancelled()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::SftpTransferControl;
    use std::time::Duration;

    #[test]
    fn child_controls_inherit_parent_pause_and_cancel_without_cancelling_siblings() {
        let parent = SftpTransferControl::new();
        let child = parent.child();
        let sibling = parent.child();
        parent.pause();
        assert!(child.is_paused());
        child.cancel();
        assert!(!parent.is_cancelled());
        assert!(!sibling.is_cancelled());
        parent.resume();
        assert!(!sibling.is_paused());
        parent.cancel();
        assert!(sibling.is_cancelled());
    }

    #[tokio::test]
    async fn cancelling_wakes_an_inflight_request_without_waiting_for_network_timeout() {
        let control = SftpTransferControl::new();
        let cancel = control.clone();
        let task =
            tokio::spawn(
                async move { control.until_cancelled(std::future::pending::<()>()).await },
            );
        tokio::task::yield_now().await;
        cancel.cancel();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
    }
}
