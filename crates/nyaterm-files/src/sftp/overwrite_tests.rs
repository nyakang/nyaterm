//! Exercise conflict decisions through the public transfer entry points and an
//! in-memory SFTP peer, including real enumeration, writes and download commits.

use super::filename_tests::service;
use crate::sftp::SftpTransferControl;
use crate::sftp_transfer_types::{
    SftpDuplicateDecision, SftpDuplicatePolicy, SftpDuplicateRequest, SftpDuplicateResolver,
    SftpPathTransferOptions, SftpTransferOptions,
};
use std::fs;
use std::sync::{Arc, Mutex};

struct RecordingResolver {
    decision: SftpDuplicateDecision,
    requests: Arc<Mutex<Vec<SftpDuplicateRequest>>>,
}

impl SftpDuplicateResolver for RecordingResolver {
    fn resolve_duplicate(
        &self,
        request: &SftpDuplicateRequest,
    ) -> Result<SftpDuplicateDecision, String> {
        self.requests.lock().unwrap().push(request.clone());
        Ok(self.decision)
    }
}

fn options(
    decision: SftpDuplicateDecision,
) -> (
    SftpPathTransferOptions,
    Arc<Mutex<Vec<SftpDuplicateRequest>>>,
) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let options = SftpPathTransferOptions::new(
        SftpDuplicatePolicy::Ask,
        Some(Arc::new(RecordingResolver {
            decision,
            requests: requests.clone(),
        })),
        SftpTransferOptions::default(),
    );
    (options, requests)
}

#[test]
fn directory_upload_resolves_only_the_root_and_merges_or_skips_the_whole_tree() -> anyhow::Result<()>
{
    for decision in [
        SftpDuplicateDecision::Overwrite,
        SftpDuplicateDecision::Skip,
        SftpDuplicateDecision::Rename,
    ] {
        let (service, files) = service("UTF-8");
        {
            let mut files = files.lock().unwrap();
            files
                .directories
                .extend([b"/folder".to_vec(), b"/folder/sub".to_vec()]);
            files.contents.extend([
                (b"/folder/a".to_vec(), b"old".to_vec()),
                (b"/folder/sub/b".to_vec(), b"old".to_vec()),
                (b"/folder/sub/target-only".to_vec(), b"keep".to_vec()),
            ]);
        }
        let root = tempfile::tempdir()?;
        let local = root.path().join("source");
        fs::create_dir_all(local.join("sub/deep"))?;
        fs::write(local.join("a"), b"new payload")?;
        fs::write(local.join("sub/b"), b"new payload")?;
        fs::write(local.join("sub/deep/c"), b"new payload")?;
        let (options, requests) = options(decision);
        let retry = options.with_transfer_options(SftpTransferOptions::default());
        let summary = service.upload_path_with_progress_and_path_options(
            &local,
            "/folder",
            SftpTransferControl::new(),
            options,
            |_| {},
        )?;
        assert_eq!(requests.lock().unwrap().len(), 1);
        assert!(requests.lock().unwrap()[0].is_directory);
        assert_eq!(summary.skipped, decision == SftpDuplicateDecision::Skip);
        let target = if decision == SftpDuplicateDecision::Rename {
            "/folder(1)"
        } else {
            "/folder"
        };
        {
            let files = files.lock().unwrap();
            assert_eq!(
                files.contents[b"/folder/sub/target-only".as_slice()],
                b"keep"
            );
            if !summary.skipped {
                for name in ["a", "sub/b", "sub/deep/c"] {
                    assert_eq!(
                        files.contents[format!("{target}/{name}").as_bytes()],
                        b"new payload"
                    );
                }
            }
            if decision != SftpDuplicateDecision::Overwrite {
                assert_eq!(files.contents[b"/folder/a".as_slice()], b"old");
            }
        }
        if decision == SftpDuplicateDecision::Overwrite {
            service.upload_path_with_progress_and_path_options(
                &local,
                "/folder",
                SftpTransferControl::new(),
                retry,
                |_| {},
            )?;
            assert_eq!(
                requests.lock().unwrap().len(),
                1,
                "manual retry reuses root decision"
            );
        }
    }
    Ok(())
}

#[test]
fn directory_download_resolves_only_the_root_and_preserves_destination_only_files()
-> anyhow::Result<()> {
    for decision in [
        SftpDuplicateDecision::Overwrite,
        SftpDuplicateDecision::Skip,
        SftpDuplicateDecision::Rename,
    ] {
        let (service, files) = service("UTF-8");
        {
            let mut files = files.lock().unwrap();
            files.directories.extend(
                ["/folder", "/folder/sub", "/folder/sub/deep"].map(|p| p.as_bytes().to_vec()),
            );
            files
                .contents
                .extend(["a", "sub/b", "sub/deep/c"].map(|name| {
                    (
                        format!("/folder/{name}").into_bytes(),
                        b"new payload".to_vec(),
                    )
                }));
        }
        let root = tempfile::tempdir()?;
        let local = root.path().join("folder");
        fs::create_dir_all(local.join("sub"))?;
        fs::write(local.join("a"), b"old")?;
        fs::write(local.join("sub/b"), b"old")?;
        fs::write(local.join("sub/target-only"), b"keep")?;
        let (options, requests) = options(decision);
        let retry = options.with_transfer_options(SftpTransferOptions::default());
        let summary = service.download_path_with_progress_and_path_options(
            "/folder",
            &local,
            SftpTransferControl::new(),
            options,
            |_| {},
        )?;
        assert_eq!(requests.lock().unwrap().len(), 1);
        assert!(requests.lock().unwrap()[0].is_directory);
        assert_eq!(summary.skipped, decision == SftpDuplicateDecision::Skip);
        assert_eq!(fs::read(local.join("sub/target-only"))?, b"keep");
        if !summary.skipped {
            for name in ["a", "sub/b", "sub/deep/c"] {
                assert_eq!(fs::read(summary.local_path.join(name))?, b"new payload");
            }
        }
        if decision != SftpDuplicateDecision::Overwrite {
            assert_eq!(fs::read(local.join("a"))?, b"old");
        }
        if decision == SftpDuplicateDecision::Rename {
            assert_ne!(summary.local_path, local);
        }
        if decision == SftpDuplicateDecision::Overwrite {
            service.download_path_with_progress_and_path_options(
                "/folder",
                &local,
                SftpTransferControl::new(),
                retry,
                |_| {},
            )?;
            assert_eq!(
                requests.lock().unwrap().len(),
                1,
                "manual retry reuses root decision"
            );
        }
    }
    Ok(())
}

#[test]
fn new_roots_transfer_without_asking_and_download_automatic_retry_keeps_one_confirmation()
-> anyhow::Result<()> {
    let (service, files) = service("UTF-8");
    let root = tempfile::tempdir()?;
    let local = root.path().join("source");
    fs::create_dir_all(local.join("sub"))?;
    fs::write(local.join("sub/a"), b"payload")?;
    let (upload_options, upload_requests) = options(SftpDuplicateDecision::Overwrite);
    service.upload_path_with_progress_and_path_options(
        &local,
        "/new",
        SftpTransferControl::new(),
        upload_options,
        |_| {},
    )?;
    assert!(upload_requests.lock().unwrap().is_empty());
    let target = root.path().join("download");
    let (download_options, download_requests) = options(SftpDuplicateDecision::Overwrite);
    service.download_path_with_progress_and_path_options(
        "/new",
        &target,
        SftpTransferControl::new(),
        download_options,
        |_| {},
    )?;
    assert!(download_requests.lock().unwrap().is_empty());
    assert_eq!(fs::read(target.join("sub/a"))?, b"payload");

    files.lock().unwrap().transient_read_failure = true;
    let (download_options, download_requests) = options(SftpDuplicateDecision::Overwrite);
    service.download_path_with_progress_and_path_options(
        "/new",
        &target,
        SftpTransferControl::new(),
        download_options.with_transfer_options(SftpTransferOptions::default().with_max_retries(1)),
        |_| {},
    )?;
    assert_eq!(download_requests.lock().unwrap().len(), 1);
    assert_eq!(fs::read(target.join("sub/a"))?, b"payload");
    Ok(())
}
