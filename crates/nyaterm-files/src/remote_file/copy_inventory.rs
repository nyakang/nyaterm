use crate::file_browser::{FileBrowserService, FileTransferEntryError};
use crate::remote_file::FileTransferEndpoint;
use crate::sftp::{RemoteFilePath, SftpFileProperties, SftpFileType, SftpTransferControl};
use std::collections::HashMap;

pub(crate) const MAX_ENTRIES: usize = 65_536;
pub(crate) const MAX_DEPTH: usize = 128;

pub(crate) struct Inventory {
    entries: HashMap<Vec<u8>, Entry>,
    pub(crate) bytes: Option<u64>,
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
        FileTransferEndpoint::Remote { path, .. }
        | FileTransferEndpoint::Adb { path, .. }
        | FileTransferEndpoint::Ftp { path, .. } => Ok(path
            .raw_path()?
            .unwrap_or_else(|| path.display_path.as_bytes().to_vec())),
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
        FileTransferEndpoint::Ftp { service, path } => {
            (FileBrowserService::Ftp(service.clone()), path.clone())
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
    pub(crate) fn scan_with_wait(
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
                FileBrowserService::Ftp(ftp) => crate::file_browser::properties_from_ftp_entry(
                    &ftp.stat_with_control(&path.display_path, control)?,
                ),
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
                    FileBrowserService::Ftp(ftp) => ftp
                        .list_dir_with_control(&path.display_path, control)?
                        .iter()
                        .map(crate::file_browser::entry_from_ftp_entry)
                        .collect(),
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
                        crate::file_browser::valid_file_browser_child_name(
                            service.kind(),
                            &child.name
                        ),
                        FileTransferEntryError::InvalidDestination
                    );
                    let parent = service.parent_path(&child.remote_path())?;
                    if service.kind() == crate::file_browser::FileBrowserBackendKind::Local {
                        anyhow::ensure!(
                            std::path::Path::new(&parent.display_path)
                                == std::path::Path::new(&path.display_path),
                            FileTransferEntryError::SourceChanged
                        );
                    } else {
                        let parent = parent
                            .raw_path()?
                            .unwrap_or_else(|| parent.display_path.as_bytes().to_vec());
                        let expected = path
                            .raw_path()?
                            .unwrap_or_else(|| path.display_path.as_bytes().to_vec());
                        let trim = |bytes: Vec<u8>| {
                            let end = bytes
                                .iter()
                                .rposition(|byte| *byte != b'/')
                                .map_or(1.min(bytes.len()), |index| index + 1);
                            bytes[..end].to_vec()
                        };
                        anyhow::ensure!(
                            trim(parent) == trim(expected),
                            FileTransferEntryError::SourceChanged
                        );
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
    pub(crate) fn len(&self) -> usize {
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
    use crate::adb::test_support::AdbFixture;
    use crate::remote_file::FileTransferEndpoint;
    use crate::remote_file::copy_inventory::Inventory;
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
