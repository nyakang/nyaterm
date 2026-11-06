use super::pane::PaneEndpoint;
use nyaterm_files::sftp::{RemoteFilePath, SftpFileEntry};

/// Captured before a dialog or deferred application callback can change focus.
#[derive(Clone)]
pub(in crate::features) struct FileManagerSelection {
    pub endpoint: PaneEndpoint,
    pub directory: RemoteFilePath,
    pub entries: Vec<SftpFileEntry>,
}
