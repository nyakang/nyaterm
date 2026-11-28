//! Resume assertions include wire offsets: a full retry can otherwise hide a broken checkpoint.
use super::filename_tests::{Files, service};
use super::{SftpService, SftpTransferControl};
use crate::file_browser::FileTransferEntryError;
use crate::sftp_transfer_types::{SftpPathTransferOptions, SftpTransferOptions};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

const CHUNK: usize = 63 * 1024;

fn options() -> SftpPathTransferOptions {
    SftpPathTransferOptions::default()
        .with_transfer_options(SftpTransferOptions::default().with_resume_broken_transfer(true))
}

fn download(
    service: &SftpService,
    path: &str,
    target: &Path,
    options: &SftpPathTransferOptions,
) -> anyhow::Result<()> {
    service.download_path_with_progress_and_path_options(
        path,
        target,
        SftpTransferControl::new(),
        options.with_transfer_options(options.transfer_options().clone()),
        |_| {},
    )?;
    Ok(())
}

fn interrupt(
    service: &SftpService,
    files: &Arc<Mutex<Files>>,
    target: &Path,
    options: &SftpPathTransferOptions,
) -> anyhow::Result<u64> {
    let control = SftpTransferControl::new();
    let cancel = control.clone();
    let result = service.download_path_with_progress_and_path_options(
        "/file",
        target,
        control,
        options.with_transfer_options(options.transfer_options().clone()),
        move |progress| {
            if progress.bytes_transferred >= CHUNK as u64 {
                cancel.cancel();
            }
        },
    );
    assert!(result.is_err());
    let length = std::fs::metadata(target)?.len();
    assert!(length > 0 && length < (3 * CHUNK) as u64);
    files.lock().unwrap().reads.clear();
    Ok(length)
}

#[test]
fn overwriting_existing_files_never_adopts_their_contents_as_a_prefix() -> anyhow::Result<()> {
    let old = (10..=1000)
        .map(|n| format!("{n}\n"))
        .collect::<String>()
        .into_bytes();
    let new = (1..=1000)
        .map(|n| format!("{n}\n"))
        .collect::<String>()
        .into_bytes();
    assert_eq!(new.len(), old.len() + 18);
    for existing in [old, vec![b'x'; new.len()], vec![b'x'; new.len() + 10]] {
        let (remote, files) = service("UTF-8");
        files
            .lock()
            .unwrap()
            .contents
            .insert(b"/file".to_vec(), new.clone());
        let root = tempfile::tempdir()?;
        let target = root.path().join("file");
        std::fs::write(&target, existing)?;
        download(&remote, "/file", &target, &options())?;
        assert_eq!(std::fs::read(&target)?, new);
        assert_eq!(files.lock().unwrap().reads.first().unwrap().0, 0);
    }
    Ok(())
}

#[test]
fn complete_downloads_are_refetched_even_when_remote_metadata_is_unchanged() -> anyhow::Result<()> {
    let (remote, files) = service("UTF-8");
    files
        .lock()
        .unwrap()
        .contents
        .insert(b"/file".to_vec(), b"old!".to_vec());
    let root = tempfile::tempdir()?;
    let target = root.path().join("file");
    let options = options();
    download(&remote, "/file", &target, &options)?;
    {
        let mut peer = files.lock().unwrap();
        peer.contents.insert(b"/file".to_vec(), b"new!".to_vec());
        peer.reads.clear();
    }
    download(&remote, "/file", &target, &options)?;
    assert_eq!(std::fs::read(&target)?, b"new!");
    assert_eq!(files.lock().unwrap().reads.first().unwrap().0, 0);
    Ok(())
}

#[test]
fn direct_file_automatic_retry_resumes_only_its_confirmed_prefix() -> anyhow::Result<()> {
    let (remote, files) = service("UTF-8");
    let content = vec![7; CHUNK * 3];
    {
        let mut peer = files.lock().unwrap();
        peer.contents.insert(b"/file".to_vec(), content.clone());
        peer.fail_read_once_at = Some(CHUNK as u64);
    }
    let root = tempfile::tempdir()?;
    let target = root.path().join("file");
    remote.download_file_with_progress_and_control_options(
        "/file",
        &target,
        SftpTransferControl::new(),
        SftpTransferOptions::default()
            .with_resume_broken_transfer(true)
            .with_max_retries(1),
        |_| {},
    )?;
    assert_eq!(std::fs::read(&target)?, content);
    let offsets = files
        .lock()
        .unwrap()
        .reads
        .iter()
        .map(|(offset, _)| *offset)
        .collect::<Vec<_>>();
    assert_eq!(
        offsets,
        vec![0, CHUNK as u64, CHUNK as u64, (2 * CHUNK) as u64]
    );
    Ok(())
}

#[test]
fn independent_tasks_changed_sources_and_disabled_resume_start_at_zero() -> anyhow::Result<()> {
    for case in [
        "new_task",
        "new_service",
        "new_path",
        "disabled",
        "mtime",
        "size",
        "missing_size",
        "missing_mtime",
    ] {
        let (remote, files) = service("UTF-8");
        let content = vec![7; CHUNK * 3];
        files
            .lock()
            .unwrap()
            .contents
            .insert(b"/file".to_vec(), content.clone());
        let root = tempfile::tempdir()?;
        let target = root.path().join("file");
        let options = options();
        interrupt(&remote, &files, &target, &options)?;
        let mut expected = content.clone();
        let mut path = "/file";
        let mut retry = options.with_transfer_options(options.transfer_options().clone());
        let (reader, observed) = if case == "new_service" {
            let (other, peer) = service("UTF-8");
            expected.fill(8);
            peer.lock()
                .unwrap()
                .contents
                .insert(b"/file".to_vec(), expected.clone());
            (other, peer)
        } else {
            (remote.clone(), files.clone())
        };
        match case {
            "new_task" => retry = options.clone(),
            "new_path" => {
                path = "/other";
                expected.fill(9);
                files
                    .lock()
                    .unwrap()
                    .contents
                    .insert(path.as_bytes().to_vec(), expected.clone());
            }
            "disabled" => retry = options.with_transfer_options(SftpTransferOptions::default()),
            "mtime" => {
                expected.fill(10);
                let mut peer = files.lock().unwrap();
                peer.modified = Some(42);
                peer.contents.insert(b"/file".to_vec(), expected.clone());
            }
            "size" => {
                expected.insert(0, 11);
                files
                    .lock()
                    .unwrap()
                    .contents
                    .insert(b"/file".to_vec(), expected.clone());
            }
            "missing_size" => files.lock().unwrap().omit_size = true,
            "missing_mtime" => {
                let attrs = russh_sftp::protocol::FileAttributes {
                    permissions: Some(0o100644),
                    ..Default::default()
                };
                files
                    .lock()
                    .unwrap()
                    .attributes
                    .insert(b"/file".to_vec(), attrs);
            }
            _ => {}
        }
        download(&reader, path, &target, &retry)?;
        assert_eq!(std::fs::read(&target)?, expected, "case: {case}");
        assert_eq!(
            observed.lock().unwrap().reads.first().unwrap().0,
            0,
            "case: {case}"
        );
    }
    Ok(())
}

#[test]
fn changed_or_replaced_local_partial_is_not_overwritten_by_retry() -> anyhow::Result<()> {
    for replaced in [false, true] {
        let (remote, files) = service("UTF-8");
        files
            .lock()
            .unwrap()
            .contents
            .insert(b"/file".to_vec(), vec![7; CHUNK * 3]);
        let root = tempfile::tempdir()?;
        let target = root.path().join("file");
        let options = options();
        let length = interrupt(&remote, &files, &target, &options)?;
        let modified = std::fs::metadata(&target)?
            .modified()?
            .duration_since(UNIX_EPOCH)?;
        if replaced {
            // Keep the original inode alive, including on filesystems that eagerly recycle it.
            std::fs::rename(&target, root.path().join("previous"))?;
        }
        let changed = vec![8; length as usize];
        std::fs::write(&target, &changed)?;
        let nanos = (modified.subsec_nanos() + 100_000_000) % 1_000_000_000;
        std::fs::OpenOptions::new()
            .write(true)
            .open(&target)?
            .set_modified(UNIX_EPOCH + Duration::new(modified.as_secs(), nanos))?;
        assert!(download(&remote, "/file", &target, &options).is_err());
        assert_eq!(std::fs::read(&target)?, changed);
        assert!(files.lock().unwrap().reads.is_empty());
    }
    Ok(())
}

#[test]
fn remote_change_before_commit_preserves_target_and_invalidates_old_partial() -> anyhow::Result<()>
{
    for partial in [false, true] {
        let (remote, files) = service("UTF-8");
        files
            .lock()
            .unwrap()
            .contents
            .insert(b"/file".to_vec(), vec![7; CHUNK * 3]);
        let root = tempfile::tempdir()?;
        let target = root.path().join("file");
        let options = options();
        if partial {
            interrupt(&remote, &files, &target, &options)?;
        } else {
            std::fs::write(&target, b"original")?;
        }
        let before = std::fs::read(&target)?;
        let peer = files.clone();
        let result = remote.download_path_with_progress_and_path_options(
            "/file",
            &target,
            SftpTransferControl::new(),
            options.with_transfer_options(options.transfer_options().clone()),
            move |progress| {
                if progress.bytes_transferred > CHUNK as u64 {
                    peer.lock().unwrap().modified = Some(42);
                }
            },
        );
        assert!(matches!(
            result.unwrap_err().downcast_ref::<FileTransferEntryError>(),
            Some(FileTransferEntryError::SourceChanged)
        ));
        assert_eq!(std::fs::read(&target)?, before);
        assert_eq!(std::fs::read_dir(root.path())?.count(), 1);
        {
            let mut peer = files.lock().unwrap();
            peer.modified = None;
            peer.reads.clear();
        }
        download(&remote, "/file", &target, &options)?;
        assert_eq!(files.lock().unwrap().reads.first().unwrap().0, 0);
    }
    Ok(())
}

#[test]
fn promised_empty_file_is_complete_and_reuses_only_its_own_source() -> anyhow::Result<()> {
    let (remote, files) = service("UTF-8");
    files
        .lock()
        .unwrap()
        .contents
        .insert(b"/file".to_vec(), Vec::new());
    let root = tempfile::tempdir()?;
    let target = root.path().join("file");
    let options = SftpPathTransferOptions::for_promised_download(
        SftpTransferOptions::default(),
        super::SftpFileType::File,
    );
    download(&remote, "/file", &target, &options)?;
    download(&remote, "/file", &target, &options)?;
    assert_eq!(files.lock().unwrap().open_flags.len(), 1);
    assert_eq!(std::fs::metadata(&target)?.len(), 0);
    let (other, peer) = service("UTF-8");
    peer.lock()
        .unwrap()
        .contents
        .insert(b"/file".to_vec(), Vec::new());
    download(&other, "/file", &target, &options)?;
    assert_eq!(peer.lock().unwrap().open_flags.len(), 1);
    Ok(())
}

#[test]
fn directory_retry_resumes_its_child_checkpoint_without_asking_again() -> anyhow::Result<()> {
    let (remote, files) = service("UTF-8");
    let content = vec![7; CHUNK * 3];
    {
        let mut peer = files.lock().unwrap();
        peer.directories.insert(b"/folder".to_vec());
        peer.contents
            .insert(b"/folder/file".to_vec(), content.clone());
        peer.fail_read_at = Some(CHUNK as u64);
    }
    let root = tempfile::tempdir()?;
    let target = root.path().join("folder");
    let options = options();
    assert!(download(&remote, "/folder", &target, &options).is_err());
    assert_eq!(std::fs::metadata(target.join("file"))?.len(), CHUNK as u64);
    {
        let mut peer = files.lock().unwrap();
        peer.fail_read_at = None;
        peer.reads.clear();
    }
    download(&remote, "/folder", &target, &options)?;
    assert_eq!(std::fs::read(target.join("file"))?, content);
    assert_eq!(files.lock().unwrap().reads.first().unwrap().0, CHUNK as u64);
    Ok(())
}

#[test]
fn a_partial_changed_after_preparation_is_preserved_when_commit_is_refused() -> anyhow::Result<()> {
    let (remote, files) = service("UTF-8");
    files
        .lock()
        .unwrap()
        .contents
        .insert(b"/file".to_vec(), vec![7; CHUNK * 3]);
    let root = tempfile::tempdir()?;
    let target = root.path().join("file");
    let options = options();
    let length = interrupt(&remote, &files, &target, &options)?;
    let change_target = target.clone();
    let changed = vec![8; length as usize];
    let edited = changed.clone();
    let result = remote.download_path_with_progress_and_path_options(
        "/file",
        &target,
        SftpTransferControl::new(),
        options.with_transfer_options(options.transfer_options().clone()),
        move |progress| {
            if progress.bytes_transferred == length {
                std::fs::write(&change_target, &edited).unwrap();
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&change_target)
                    .unwrap()
                    .set_modified(UNIX_EPOCH + Duration::from_secs(42))
                    .unwrap();
            }
        },
    );
    assert!(result.is_err());
    assert_eq!(std::fs::read(&target)?, changed);
    assert_eq!(std::fs::read_dir(root.path())?.count(), 1);
    Ok(())
}
