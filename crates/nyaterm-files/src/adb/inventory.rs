use crate::file_browser::{FileBrowserService, FileTransferEntryError};
use crate::remote_file::FileTransferEndpoint;
use crate::sftp::{RemoteFilePath, SftpFileProperties, SftpFileType, SftpTransferControl};
use std::collections::HashMap;

pub(crate) const MAX_ENTRIES: usize = 65_536;
pub(crate) const MAX_DEPTH: usize = 128;

pub(crate) struct Inventory {
    entries: HashMap<Vec<u8>, Entry>,
    pub(super) bytes: Option<u64>,
}
struct Entry {
    properties: SftpFileProperties,
    children: Vec<FileTransferEndpoint>,
}
fn key(endpoint: &FileTransferEndpoint) -> anyhow::Result<Vec<u8>> {
    match endpoint {
        FileTransferEndpoint::Local(path) => Ok(path
            .to_str()
            .ok_or(FileTransferEntryError::InvalidDestination)?
            .as_bytes()
            .to_vec()),
        FileTransferEndpoint::Remote { path, .. } | FileTransferEndpoint::Adb { path, .. } => {
            Ok(path
                .raw_path()?
                .unwrap_or_else(|| path.display_path.as_bytes().to_vec()))
        }
    }
}
fn browser(endpoint: &FileTransferEndpoint) -> (FileBrowserService, RemoteFilePath) {
    match endpoint {
        FileTransferEndpoint::Local(path) => (
            FileBrowserService::local(),
            RemoteFilePath::new(path.to_string_lossy()),
        ),
        FileTransferEndpoint::Remote { service, path } => {
            (FileBrowserService::Remote(service.clone()), path.clone())
        }
        FileTransferEndpoint::Adb { service, path } => {
            (FileBrowserService::Adb(service.clone()), path.clone())
        }
    }
}
impl Inventory {
    pub(crate) fn scan_with_limit(
        source: &FileTransferEndpoint,
        control: &SftpTransferControl,
        limit: usize,
    ) -> anyhow::Result<Self> {
        Self::scan_with_wait(source, control, limit, |control| {
            if let FileTransferEndpoint::Adb { service, .. } = source {
                service.wait_for_transfer(control)
            } else {
                control
                    .wait_if_paused_blocking()
                    .map_err(anyhow::Error::msg)
            }
        })
    }
    pub(super) fn scan_with_wait(
        source: &FileTransferEndpoint,
        control: &SftpTransferControl,
        limit: usize,
        mut wait: impl FnMut(&SftpTransferControl) -> anyhow::Result<()>,
    ) -> anyhow::Result<Self> {
        let limit = limit.min(MAX_ENTRIES);
        let mut entries = HashMap::new();
        let mut pending = vec![(source.clone(), 0usize)];
        let mut bytes = Some(0u64);
        while let Some((endpoint, depth)) = pending.pop() {
            wait(control)?;
            anyhow::ensure!(
                depth <= MAX_DEPTH && entries.len() < limit,
                FileTransferEntryError::LimitExceeded
            );
            let (service, path) = browser(&endpoint);
            let properties = match &service {
                FileBrowserService::Adb(adb) => {
                    adb.remote_file_properties_with_control(&path, control)?
                }
                _ => service.remote_file_properties(&path)?,
            };
            anyhow::ensure!(
                matches!(
                    properties.file_type,
                    SftpFileType::File | SftpFileType::Directory
                ),
                FileTransferEntryError::UnsupportedFileType
            );
            let mut children = Vec::new();
            if properties.is_directory() {
                let listed = match &service {
                    FileBrowserService::Adb(adb) => {
                        adb.list_dir_path_with_control(&path, control)?
                    }
                    _ => service.list_dir_path(&path)?,
                };
                for child in listed {
                    if matches!(child.name.as_str(), "." | "..") {
                        continue;
                    }
                    anyhow::ensure!(
                        depth < MAX_DEPTH
                            && entries.len() + pending.len() + children.len() + 1 < limit,
                        FileTransferEntryError::LimitExceeded
                    );
                    children.push(service.transfer_endpoint(&child.remote_path()));
                }
                pending.extend(
                    children
                        .iter()
                        .rev()
                        .cloned()
                        .map(|child| (child, depth + 1)),
                );
            } else {
                bytes = bytes
                    .zip(properties.size)
                    .and_then(|(a, b)| a.checked_add(b));
            }
            anyhow::ensure!(
                entries
                    .insert(
                        key(&endpoint)?,
                        Entry {
                            properties,
                            children
                        }
                    )
                    .is_none(),
                FileTransferEntryError::SourceChanged
            );
        }
        Ok(Self { entries, bytes })
    }
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }
    pub(crate) fn properties(
        &self,
        endpoint: &FileTransferEndpoint,
    ) -> anyhow::Result<SftpFileProperties> {
        Ok(self
            .entries
            .get(&key(endpoint)?)
            .ok_or(FileTransferEntryError::SourceChanged)?
            .properties
            .clone())
    }
    pub(crate) fn children(
        &self,
        endpoint: &FileTransferEndpoint,
    ) -> anyhow::Result<Vec<FileTransferEndpoint>> {
        Ok(self
            .entries
            .get(&key(endpoint)?)
            .ok_or(FileTransferEntryError::SourceChanged)?
            .children
            .clone())
    }
}

#[cfg(test)]
mod tests {
    use crate::adb::inventory::Inventory;
    use crate::adb::test_support::AdbFixture;
    use crate::remote_file::FileTransferEndpoint;
    use crate::sftp::{RemoteFilePath, SftpTransferControl};
    #[test]
    fn adb_inventory_checks_entry_limits_before_enqueuing_children() {
        let fixture = AdbFixture::new();
        for name in [b"/device/a", b"/device/b"] {
            fixture.file(name, b"data");
        }
        let source = FileTransferEndpoint::Adb {
            service: fixture.service.clone(),
            path: RemoteFilePath::new("/device"),
        };
        assert!(Inventory::scan_with_limit(&source, &SftpTransferControl::new(), 2).is_err());
        assert_eq!(
            Inventory::scan_with_limit(&source, &SftpTransferControl::new(), 3)
                .unwrap()
                .len(),
            3
        );
        assert_eq!(fixture.device.lock().unwrap().receives, 0);
    }
}
