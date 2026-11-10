use super::{
    FileMutation, FileOperationRequest, FileOperationResult, FileTransferOperation,
    FileTransferRequest, execute_file_operation,
};
use crate::file_browser::FileBrowserService;
use crate::sftp::{RemoteFilePath, SftpTransferControl};
use crate::sftp_transfer_types::{SftpDuplicatePolicy, SftpPathTransferOptions};
use nyaterm_core::test_support::TestTempDir;
use std::fs;

fn run(
    root: &TestTempDir,
    name: &str,
    operation: FileTransferOperation,
    control: SftpTransferControl,
) -> Result<FileOperationResult, String> {
    let service = FileBrowserService::local();
    let source = RemoteFilePath::new(root.join("source").to_string_lossy());
    let entry = service
        .list_dir_path(&source)
        .unwrap()
        .into_iter()
        .find(|entry| entry.name == name)
        .unwrap();
    execute_file_operation(
        FileOperationRequest::Transfer(FileTransferRequest {
            same_endpoint: true,
            entry,
            target_directory: RemoteFilePath::new(root.join("target").to_string_lossy()),
            operation,
        }),
        service.clone(),
        service,
        control,
        SftpPathTransferOptions::new(SftpDuplicatePolicy::Skip, None, Default::default()),
        |_| {},
    )
    .map_err(|error| error.to_string())
}

fn fixture(label: &str) -> TestTempDir {
    let root = TestTempDir::new(label);
    fs::create_dir_all(root.join("source")).unwrap();
    fs::create_dir_all(root.join("target")).unwrap();
    root
}

#[test]
fn move_conflict_keeps_both_originals_and_never_merges_directories() {
    for directory in [false, true] {
        let root = fixture("nyaterm-file-manager-conflict");
        if directory {
            fs::create_dir(root.join("source/item")).unwrap();
            fs::create_dir(root.join("target/item")).unwrap();
            fs::write(root.join("source/item/source"), b"source").unwrap();
            fs::write(root.join("target/item/target"), b"target").unwrap();
        } else {
            fs::write(root.join("source/item"), b"source").unwrap();
            fs::write(root.join("target/item"), b"target").unwrap();
        }
        assert!(
            run(
                &root,
                "item",
                FileTransferOperation::Move,
                SftpTransferControl::new()
            )
            .unwrap_err()
            .contains("already exists")
        );
        if directory {
            assert!(root.join("source/item/source").exists());
            assert!(root.join("target/item/target").exists());
            assert!(!root.join("target/item/source").exists());
        } else {
            assert_eq!(fs::read(root.join("source/item")).unwrap(), b"source");
            assert_eq!(fs::read(root.join("target/item")).unwrap(), b"target");
        }
    }
}

#[test]
fn unicode_directory_copy_then_move_uses_frozen_objects() {
    let root = fixture("nyaterm-file-manager-unicode");
    fs::create_dir(root.join("source/目录")).unwrap();
    fs::write(root.join("source/目录/文件.txt"), b"payload").unwrap();
    run(
        &root,
        "目录",
        FileTransferOperation::Copy,
        SftpTransferControl::new(),
    )
    .unwrap();
    assert_eq!(
        fs::read(root.join("target/目录/文件.txt")).unwrap(),
        b"payload"
    );
    assert!(root.join("source/目录/文件.txt").exists());
    fs::remove_dir_all(root.join("target/目录")).unwrap();
    run(
        &root,
        "目录",
        FileTransferOperation::Move,
        SftpTransferControl::new(),
    )
    .unwrap();
    assert!(!root.join("source/目录").exists());
    assert_eq!(
        fs::read(root.join("target/目录/文件.txt")).unwrap(),
        b"payload"
    );
}

#[test]
fn cancellation_before_move_keeps_source_and_creates_nothing() {
    let root = fixture("nyaterm-file-manager-cancel");
    fs::write(root.join("source/item"), b"keep").unwrap();
    let control = SftpTransferControl::new();
    control.cancel();
    assert!(run(&root, "item", FileTransferOperation::Move, control).is_err());
    assert_eq!(fs::read(root.join("source/item")).unwrap(), b"keep");
    assert!(!root.join("target/item").exists());
}

#[test]
fn cancelled_delete_keeps_file_and_a_fresh_retry_deletes_it() {
    let root = fixture("nyaterm-file-manager-delete-retry");
    let path = root.join("source/item");
    fs::write(&path, b"keep until retry").unwrap();
    let request = FileOperationRequest::Mutation {
        directory: RemoteFilePath::new(root.join("source").to_string_lossy()),
        mutation: FileMutation::Delete(RemoteFilePath::new(path.to_string_lossy())),
    };
    let cancelled = SftpTransferControl::new();
    cancelled.cancel();
    let service = FileBrowserService::local();
    assert_eq!(
        execute_file_operation(
            request.clone(),
            service.clone(),
            service.clone(),
            cancelled,
            Default::default(),
            |_| {},
        )
        .unwrap_err()
        .to_string(),
        crate::sftp::SFTP_TRANSFER_CANCELLED,
    );
    assert_eq!(fs::read(&path).unwrap(), b"keep until retry");
    execute_file_operation(
        request,
        service.clone(),
        service,
        SftpTransferControl::new(),
        Default::default(),
        |_| {},
    )
    .unwrap();
    assert!(!path.exists());
}

#[test]
fn frozen_mutation_does_not_follow_selection_and_create_never_truncates() {
    let root = fixture("nyaterm-file-manager-mutation");
    let service = FileBrowserService::local();
    let directory = RemoteFilePath::new(root.join("source").to_string_lossy());
    let request = FileOperationRequest::Mutation {
        directory: directory.clone(),
        mutation: FileMutation::CreateFile("new.txt".into()),
    };
    execute_file_operation(
        request.clone(),
        service.clone(),
        service.clone(),
        SftpTransferControl::new(),
        Default::default(),
        |_| {},
    )
    .unwrap();
    fs::write(root.join("source/new.txt"), b"preserved").unwrap();
    assert!(
        execute_file_operation(
            request,
            service.clone(),
            service,
            SftpTransferControl::new(),
            Default::default(),
            |_| {}
        )
        .is_err()
    );
    assert_eq!(fs::read(root.join("source/new.txt")).unwrap(), b"preserved");
    assert!(!root.join("target/new.txt").exists());
}

#[test]
fn recursive_destination_is_rejected_before_any_copy() {
    let root = fixture("nyaterm-file-manager-recursive");
    fs::create_dir_all(root.join("source/item/child")).unwrap();
    let service = FileBrowserService::local();
    let entry = service
        .list_dir(root.join("source").to_string_lossy())
        .unwrap()
        .pop()
        .unwrap();
    let result = execute_file_operation(
        FileOperationRequest::Transfer(FileTransferRequest {
            same_endpoint: true,
            entry,
            target_directory: RemoteFilePath::new(root.join("source/item/child").to_string_lossy()),
            operation: FileTransferOperation::Move,
        }),
        service.clone(),
        service,
        SftpTransferControl::new(),
        Default::default(),
        |_| {},
    );
    assert!(result.unwrap_err().to_string().contains("overlap"));
    assert!(!root.join("source/item/child/item").exists());
}

#[test]
fn verified_move_keeps_source_if_it_changes_during_copy() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let root = fixture("nyaterm-file-manager-source-change");
    fs::write(root.join("source/item"), vec![7; 512 * 1024]).unwrap();
    // macOS temp paths can pass through /var -> /private/var. Resolve that
    // alias so the move's symlink-ancestor guard lets this test reach the copy.
    let source_directory = fs::canonicalize(root.join("source")).unwrap();
    let target_directory = fs::canonicalize(root.join("target")).unwrap();
    let service = FileBrowserService::local();
    let entry = service
        .list_dir(source_directory.to_string_lossy())
        .unwrap()
        .pop()
        .unwrap();
    let source = source_directory.join("item");
    let changed = Arc::new(AtomicBool::new(false));
    let record = changed.clone();
    // Distinct logical sources exercise copy/verify/delete rather than rename.
    let result = execute_file_operation(
        FileOperationRequest::Transfer(FileTransferRequest {
            same_endpoint: false,
            entry,
            target_directory: RemoteFilePath::new(target_directory.to_string_lossy()),
            operation: FileTransferOperation::Move,
        }),
        service.clone(),
        service,
        SftpTransferControl::new(),
        SftpPathTransferOptions::new(SftpDuplicatePolicy::Skip, None, Default::default()),
        move |progress| {
            if progress.bytes_transferred > 0 && !record.swap(true, Ordering::SeqCst) {
                use std::io::Write;
                fs::OpenOptions::new()
                    .append(true)
                    .open(&source)
                    .unwrap()
                    .write_all(b"changed")
                    .unwrap();
            }
        },
    );
    assert!(
        changed.load(Ordering::SeqCst),
        "copy did not reach the source mutation: {:?}",
        result.as_ref().err()
    );
    assert!(result.unwrap_err().to_string().contains("source changed"));
    assert!(root.join("source/item").exists());
}
