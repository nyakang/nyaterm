//! Fixtures shared by the transfers panel tests.

use nyaterm_files::sftp::{SftpFileEntry, SftpFileType};

pub(in crate::features::pages::transfers) fn browser_entry(index: usize) -> SftpFileEntry {
    SftpFileEntry {
        name: format!("entry-{index:04}"),
        path: format!("/remote/entry-{index:04}"),
        file_type: SftpFileType::File,
        size: Some(index as u64),
        permissions: None,
        owner: String::new(),
        group: String::new(),
        modified_at: None,
        raw_path_token: None,
        symlink_target_is_directory: false,
    }
}
