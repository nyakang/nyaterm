use crate::drag_export::RemoteDragFile;
use crate::remote_file::RemoteFileService;
use crate::sftp::{SftpFileEntry, SftpFileType, SftpTransferControl};
use std::path::PathBuf;

/// Selecting an ancestor and its child in a tree exports the ancestor once.
/// Compare wire identities so lossy display names cannot hide distinct sources.
pub fn prune_nested_roots(roots: Vec<SftpFileEntry>) -> anyhow::Result<Vec<SftpFileEntry>> {
    anyhow::ensure!(roots.len() <= 1024, "too many drag roots");
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
}
