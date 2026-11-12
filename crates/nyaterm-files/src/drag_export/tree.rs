use crate::drag_export::RemoteDragFile;
use crate::file_browser::{FileBrowserService, FileTransferEntryError};
use crate::remote_file::RemoteFileService;
use crate::sftp::{SftpFileEntry, SftpFileType, SftpTransferControl};
use std::path::PathBuf;

/// Selecting an ancestor and its child in a tree exports the ancestor once.
/// Compare wire identities so lossy display names cannot hide distinct sources.
pub fn prune_nested_roots(roots: Vec<SftpFileEntry>) -> anyhow::Result<Vec<SftpFileEntry>> {
    anyhow::ensure!(roots.len() <= 1024, FileTransferEntryError::LimitExceeded);
    let identities = roots
        .iter()
        .map(|entry| {
            let path = entry.remote_path();
            path.raw_path()
                .map(|raw| raw.unwrap_or_else(|| path.display_path.into_bytes()))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(roots
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            !roots.iter().enumerate().any(|(parent_index, parent)| {
                if parent_index == *index || parent.file_type != SftpFileType::Directory {
                    return false;
                }
                let mut prefix = identities[parent_index].clone();
                while prefix.len() > 1 && prefix.last() == Some(&b'/') {
                    prefix.pop();
                }
                if !prefix.ends_with(b"/") {
                    prefix.push(b'/');
                }
                identities[*index].starts_with(&prefix)
            })
        })
        .map(|(_, entry)| entry.clone())
        .collect())
}

#[derive(Clone, Debug)]
pub struct RemoteDragEntry {
    pub relative_path: PathBuf,
    pub root_index: usize,
    pub file: RemoteDragFile,
    pub is_directory: bool,
}

/// Enumerate metadata on a worker; files remain deferred and empty directories
/// have descriptors of their own. Raw remote paths come from the SFTP listing.
pub fn enumerate_remote_drag(
    service: &RemoteFileService,
    roots: Vec<SftpFileEntry>,
    control: &SftpTransferControl,
) -> anyhow::Result<Vec<RemoteDragEntry>> {
    service.export_sftp_service().enumerate_drag(roots, control)
}

pub fn enumerate_remote_drag_with_root_controls(
    service: &RemoteFileService,
    roots: Vec<SftpFileEntry>,
    control: &SftpTransferControl,
    root_controls: Vec<SftpTransferControl>,
) -> anyhow::Result<Vec<RemoteDragEntry>> {
    service
        .export_sftp_service()
        .enumerate_drag_with_root_controls(roots, control, root_controls)
}

pub fn enumerate_file_browser_drag_with_root_controls(
    service: &FileBrowserService,
    roots: Vec<SftpFileEntry>,
    control: &SftpTransferControl,
    root_controls: Vec<SftpTransferControl>,
) -> anyhow::Result<Vec<RemoteDragEntry>> {
    if let FileBrowserService::Remote(service) = service {
        return enumerate_remote_drag_with_root_controls(service, roots, control, root_controls);
    }
    let FileBrowserService::Adb(adb) = service else {
        anyhow::bail!(FileTransferEntryError::UnsupportedBackend);
    };
    anyhow::ensure!(roots.len() <= 1024, FileTransferEntryError::LimitExceeded);
    anyhow::ensure!(
        root_controls.is_empty() || root_controls.len() == roots.len(),
        "invalid drag root controls"
    );
    let mut found = Vec::new();
    let mut visited = std::collections::HashSet::new();
    for (root_index, root) in roots.into_iter().enumerate() {
        adb.wait_for_transfer(control)?;
        let item_control = root_controls.get(root_index).unwrap_or(control);
        if item_control.check_cancelled().is_err() {
            continue;
        }
        let root_path = root.remote_path();
        let endpoint = service.transfer_endpoint(&root_path);
        let inventory = crate::adb::inventory::Inventory::scan_with_limit(
            &endpoint,
            item_control,
            crate::adb::inventory::MAX_ENTRIES - found.len(),
        )?;
        anyhow::ensure!(
            inventory.properties(&endpoint)?.file_type == root.file_type,
            FileTransferEntryError::SourceChanged
        );
        let mut pending = vec![(endpoint, PathBuf::from(&root.name))];
        while let Some((endpoint, relative_path)) = pending.pop() {
            adb.wait_for_transfer(control)?;
            if item_control.check_cancelled().is_err() {
                break;
            }
            let crate::remote_file::FileTransferEndpoint::Adb { path, .. } = &endpoint else {
                unreachable!("ADB inventory");
            };
            let raw = crate::adb::raw_path(path)?;
            let raw_name = raw.rsplit(|byte| *byte == b'/').next().unwrap_or_default();
            let name = std::str::from_utf8(raw_name)
                .map_err(|_| FileTransferEntryError::InvalidDestination)?;
            crate::download_path::validate_name(name)
                .map_err(|_| FileTransferEntryError::InvalidDestination)?;
            anyhow::ensure!(visited.insert(raw), FileTransferEntryError::SourceChanged);
            let props = inventory.properties(&endpoint)?;
            let is_directory = props.is_directory();
            for child in inventory.children(&endpoint)?.into_iter().rev() {
                let crate::remote_file::FileTransferEndpoint::Adb { path, .. } = &child else {
                    unreachable!("ADB child");
                };
                let raw = crate::adb::raw_path(path)?;
                let name = std::str::from_utf8(
                    raw.rsplit(|byte| *byte == b'/').next().unwrap_or_default(),
                )
                .map_err(|_| FileTransferEntryError::InvalidDestination)?;
                pending.push((child.clone(), relative_path.join(name)));
            }
            found.push(RemoteDragEntry {
                relative_path,
                root_index,
                is_directory,
                file: RemoteDragFile {
                    display_name: props.name.into(),
                    remote_path: path.clone(),
                    size: if is_directory { None } else { props.size },
                    modified_at: props.modified_at.map(|time| {
                        std::time::UNIX_EPOCH + std::time::Duration::from_secs(u64::from(time))
                    }),
                },
            });
        }
    }
    anyhow::ensure!(!found.is_empty(), FileTransferEntryError::NoSelection);
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::prune_nested_roots;
    use crate::sftp::{RemoteFilePath, SftpFileEntry, SftpFileType};
    fn entry(raw: &[u8], directory: bool) -> SftpFileEntry {
        SftpFileEntry {
            name: "display".into(),
            path: "/same-display".into(),
            file_type: if directory {
                SftpFileType::Directory
            } else {
                SftpFileType::File
            },
            size: None,
            permissions: None,
            owner: String::new(),
            group: String::new(),
            modified_at: None,
            raw_path_token: RemoteFilePath::from_raw("/same-display", raw).raw_path_token,
            symlink_target_is_directory: false,
        }
    }
    #[test]
    fn ancestor_selection_removes_only_descendants_with_the_same_wire_identity() {
        let roots = vec![
            entry(b"/folder/child", false),
            entry(b"/folder", true),
            entry(b"/folder-other", false),
            entry(b"/other-\xff/child", false),
        ];
        let pruned = prune_nested_roots(roots.clone()).unwrap();
        assert_eq!(pruned, roots[1..]);
        assert_eq!(
            prune_nested_roots(vec![entry(b"/", true), entry(b"/child", false)])
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn adb_tree_inventory_preserves_empty_directories_and_raw_paths_and_rejects_links() {
        use crate::adb::test_support::{AdbFixture, Node};
        use crate::file_browser::FileBrowserService;
        use crate::sftp::SftpTransferControl;
        let fixture = AdbFixture::new();
        fixture
            .service
            .create_dir_remote_path(&RemoteFilePath::new("/device/tree"), None)
            .unwrap();
        fixture
            .service
            .create_dir_remote_path(&RemoteFilePath::new("/device/tree/empty"), None)
            .unwrap();
        fixture.file(b"/device/tree/\xe4\xb8\xad\xe6\x96\x87", b"content");
        let service = FileBrowserService::Adb(fixture.service.clone());
        let root = service
            .list_dir_path(&RemoteFilePath::new("/device"))
            .unwrap();
        let entries = super::enumerate_file_browser_drag_with_root_controls(
            &service,
            root.clone(),
            &SftpTransferControl::new(),
            Vec::new(),
        )
        .unwrap();
        assert_eq!(entries.len(), 3);
        assert!(
            entries.iter().any(|entry| entry.is_directory
                && entry.relative_path == std::path::Path::new("tree/empty"))
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry.file.remote_path.raw_path().unwrap()
                    == Some(b"/device/tree/\xe4\xb8\xad\xe6\x96\x87".to_vec()))
        );
        fixture.device.lock().unwrap().nodes.insert(
            b"/device/tree/link".to_vec(),
            Node {
                mode: 0o120777,
                data: b"target".to_vec(),
                modified: 7,
            },
        );
        assert!(
            super::enumerate_file_browser_drag_with_root_controls(
                &service,
                root,
                &SftpTransferControl::new(),
                Vec::new()
            )
            .is_err()
        );
    }

    #[test]
    fn adb_inventory_rejects_unrepresentable_names_and_observes_cancellation_before_io() {
        use crate::adb::test_support::AdbFixture;
        use crate::file_browser::FileBrowserService;
        use crate::sftp::SftpTransferControl;
        let fixture = AdbFixture::new();
        fixture.file(b"/device/\xff", b"data");
        let service = FileBrowserService::Adb(fixture.service.clone());
        let entries = service
            .list_dir_path(&RemoteFilePath::new("/device"))
            .unwrap();
        let error = super::enumerate_file_browser_drag_with_root_controls(
            &service,
            entries.clone(),
            &SftpTransferControl::new(),
            Vec::new(),
        )
        .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<crate::file_browser::FileTransferEntryError>(),
            Some(crate::file_browser::FileTransferEntryError::InvalidDestination)
        ));
        let cancelled = SftpTransferControl::new();
        cancelled.cancel();
        let error = super::enumerate_file_browser_drag_with_root_controls(
            &service,
            entries,
            &cancelled,
            Vec::new(),
        )
        .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<nyaterm_android::error::AdbError>(),
            Some(nyaterm_android::error::AdbError::Cancelled)
        ));
        assert_eq!(fixture.device.lock().unwrap().receives, 0);
    }
}
