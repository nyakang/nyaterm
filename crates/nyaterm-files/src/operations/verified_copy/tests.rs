use super::{VerifiedCopyRequest, copied_tree_matches, copy_verified};
use crate::operations::FileOperationResult;
use crate::sftp::SftpFileType;
use std::collections::BTreeMap;

#[test]
fn clipboard_copy_reports_partial_progress_before_the_finished_output() {
    use crate::file_browser::FileBrowserService;
    use crate::sftp::{RemoteFilePath, SftpTransferControl};
    use crate::sftp_transfer_types::SftpPathTransferOptions;
    use std::sync::{Arc, Mutex};

    let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-clipboard-progress");
    std::fs::create_dir_all(root.path()).unwrap();
    let source = root.join("source");
    let target = root.join("target");
    let payload = vec![7_u8; 512 * 1024];
    std::fs::write(&source, &payload).unwrap();
    let values = Arc::new(Mutex::new(Vec::new()));
    let record = values.clone();
    let output = copy_verified(
        VerifiedCopyRequest {
            source_service: FileBrowserService::local(),
            target_service: FileBrowserService::local(),
            source: RemoteFilePath::new(source.to_string_lossy()),
            target_path: RemoteFilePath::new(target.to_string_lossy()),

            cut: false,
            control: SftpTransferControl::new(),
            options: SftpPathTransferOptions::default(),
        },
        move |p| record.lock().unwrap().push(p),
    )
    .unwrap();
    assert!(
        matches!(output, FileOperationResult::Completed(crate::operations::FileOperationSummary { bytes, .. }) if bytes == payload.len() as u64)
    );
    let values = values.lock().unwrap();
    assert!(
        values
            .iter()
            .any(|p| p.bytes_transferred > 0 && p.bytes_transferred < payload.len() as u64)
    );
    assert_eq!(
        values.last().unwrap().bytes_transferred,
        payload.len() as u64
    );
    assert_eq!(std::fs::read(target).unwrap(), payload);
}

#[test]
fn local_cut_deletes_source_only_after_a_successful_copy() {
    use crate::file_browser::FileBrowserService;
    use crate::sftp::{RemoteFilePath, SftpTransferControl};
    use crate::sftp_transfer_types::SftpPathTransferOptions;

    let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-clipboard-cut");
    std::fs::create_dir_all(root.path()).unwrap();
    // macOS temporary paths can contain symlink ancestors such as /var.
    let canonical_root = std::fs::canonicalize(root.path()).unwrap();
    let source = canonical_root.join("source.txt");
    let target = canonical_root.join("target.txt");
    std::fs::write(&source, b"payload").unwrap();
    copy_verified(
        VerifiedCopyRequest {
            source_service: FileBrowserService::local(),
            target_service: FileBrowserService::local(),
            source: RemoteFilePath::new(source.to_string_lossy()),
            target_path: RemoteFilePath::new(target.to_string_lossy()),

            cut: true,
            control: SftpTransferControl::new(),
            options: SftpPathTransferOptions::default(),
        },
        |_| {},
    )
    .unwrap();
    assert!(!source.exists());
    assert_eq!(std::fs::read(target).unwrap(), b"payload");
}

#[test]
fn cancelled_cut_keeps_the_source() {
    use crate::file_browser::FileBrowserService;
    use crate::sftp::{RemoteFilePath, SftpTransferControl};
    use crate::sftp_transfer_types::SftpPathTransferOptions;

    let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-clipboard-cut-cancel");
    std::fs::create_dir_all(root.path()).unwrap();
    // Exercise cancellation without failing the symlink-ancestor check first.
    let canonical_root = std::fs::canonicalize(root.path()).unwrap();
    let source = canonical_root.join("source.txt");
    let target = canonical_root.join("target.txt");
    std::fs::write(&source, b"keep-me").unwrap();
    let control = SftpTransferControl::new();
    control.cancel();
    let result = copy_verified(
        VerifiedCopyRequest {
            source_service: FileBrowserService::local(),
            target_service: FileBrowserService::local(),
            source: RemoteFilePath::new(source.to_string_lossy()),
            target_path: RemoteFilePath::new(target.to_string_lossy()),

            cut: true,
            control,
            options: SftpPathTransferOptions::default(),
        },
        |_| {},
    );
    assert!(result.is_err());
    assert!(source.exists());
    assert_eq!(std::fs::read(source).unwrap(), b"keep-me");
}

#[test]
fn cut_verification_checks_every_file_not_only_total_bytes() {
    let before = BTreeMap::from([
        (String::new(), (SftpFileType::Directory, None, None)),
        ("/a".into(), (SftpFileType::File, Some(3), Some(1))),
    ]);
    let mut after = before.clone();
    assert!(copied_tree_matches(&before, &after));
    after.insert("/a".into(), (SftpFileType::File, Some(2), Some(1)));
    assert!(!copied_tree_matches(&before, &after));
}
