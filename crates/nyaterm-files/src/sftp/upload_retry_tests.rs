//! Faults are injected into the wire peer, exercising the public upload APIs.

use super::filename_tests::{service, service_with_config};
use super::{RemoteFilePath, SftpTransferControl};
use crate::sftp_transfer_types::{
    SftpDuplicateDecision, SftpDuplicatePolicy, SftpDuplicateRequest, SftpDuplicateResolver,
    SftpPathTransferOptions, SftpTransferOptions,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Resolver {
    decision: SftpDuplicateDecision,
    calls: Arc<AtomicUsize>,
}

impl SftpDuplicateResolver for Resolver {
    fn resolve_duplicate(&self, _: &SftpDuplicateRequest) -> Result<SftpDuplicateDecision, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.decision)
    }
}

fn options(
    decision: SftpDuplicateDecision,
    retries: u32,
) -> (SftpPathTransferOptions, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    (
        SftpPathTransferOptions::new(
            SftpDuplicatePolicy::Ask,
            Some(Arc::new(Resolver {
                decision,
                calls: calls.clone(),
            })),
            SftpTransferOptions::default().with_max_retries(retries),
        ),
        calls,
    )
}

#[test]
fn automatic_retry_of_a_new_partial_upload_never_asks_or_skips_its_own_file() -> anyhow::Result<()>
{
    for policy in [
        SftpDuplicatePolicy::Ask,
        SftpDuplicatePolicy::Skip,
        SftpDuplicatePolicy::Rename,
    ] {
        let (service, files) = service("UTF-8");
        files.lock().unwrap().fail_write_once_at = Some(32768);
        let root = tempfile::tempdir()?;
        let local = root.path().join("source.jar");
        let payload = vec![27; 256 * 1024];
        std::fs::write(&local, &payload)?;
        let (ask, calls) = options(SftpDuplicateDecision::Skip, 1);
        let options = if policy == SftpDuplicatePolicy::Ask {
            ask
        } else {
            SftpPathTransferOptions::new(policy, None, ask.transfer_options().clone())
        };
        let summary = service.upload_path_with_progress_and_path_options(
            &local,
            "/source.jar",
            SftpTransferControl::new(),
            options,
            |_| {},
        )?;
        assert!(!summary.skipped);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let files = files.lock().unwrap();
        assert_eq!(files.contents[b"/source.jar".as_slice()], payload);
        assert_eq!(
            files.open_flags.len(),
            2,
            "one initial attempt and one retry"
        );
        assert_ne!(
            files.open_flags[0].1 & 32,
            0,
            "new files use exclusive creation"
        );
        assert_eq!(
            files.open_flags[1].1 & (8 | 16 | 32),
            0,
            "retry opens without create or truncation"
        );
        assert_eq!(files.contents.len(), 1);
        assert_eq!(files.open_handles, 0);
        assert_eq!(files.active_writes, 0);
    }
    Ok(())
}

#[test]
fn existing_targets_keep_one_decision_and_the_same_rename_across_automatic_retries()
-> anyhow::Result<()> {
    for decision in [
        SftpDuplicateDecision::Overwrite,
        SftpDuplicateDecision::Rename,
        SftpDuplicateDecision::Skip,
    ] {
        let (service, files) = service("UTF-8");
        {
            let mut files = files.lock().unwrap();
            files
                .contents
                .insert(b"/file.jar".to_vec(), b"original".to_vec());
            files.fail_write_once_at = Some(32768);
        }
        let root = tempfile::tempdir()?;
        let local = root.path().join("source");
        let payload = vec![38; 256 * 1024];
        std::fs::write(&local, &payload)?;
        let (options, calls) = options(decision, 1);
        let summary = service.upload_path_with_progress_and_path_options(
            &local,
            "/file.jar",
            SftpTransferControl::new(),
            options,
            |_| {},
        )?;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let files = files.lock().unwrap();
        if decision == SftpDuplicateDecision::Skip {
            assert!(summary.skipped);
            assert!(files.open_flags.is_empty());
        } else {
            let target = if decision == SftpDuplicateDecision::Rename {
                "/file(1).jar"
            } else {
                "/file.jar"
            };
            assert_eq!(summary.remote_path, target);
            assert_eq!(files.contents[target.as_bytes()], payload);
            assert!(
                files
                    .open_flags
                    .iter()
                    .all(|(path, _)| path == target.as_bytes())
            );
            assert_eq!(files.open_flags.len(), 2);
        }
        if decision != SftpDuplicateDecision::Overwrite {
            assert_eq!(files.contents[b"/file.jar".as_slice()], b"original");
        }
        assert!(!files.contents.contains_key(b"/file(2).jar".as_slice()));
    }
    Ok(())
}

#[test]
fn manual_retry_retains_the_selected_target_and_plain_clone_is_an_independent_upload()
-> anyhow::Result<()> {
    let (service, files) = service("UTF-8");
    {
        let mut files = files.lock().unwrap();
        files
            .contents
            .insert(b"/file.jar".to_vec(), b"original".to_vec());
        files.fail_write_once_at = Some(32768);
    }
    let root = tempfile::tempdir()?;
    let local = root.path().join("source");
    let payload = vec![49; 256 * 1024];
    std::fs::write(&local, &payload)?;
    let (options, calls) = options(SftpDuplicateDecision::Rename, 0);
    let independent = options.clone();
    let retry = options.with_transfer_options(options.transfer_options().clone());
    assert!(
        service
            .upload_path_with_progress_and_path_options(
                &local,
                "/file.jar",
                SftpTransferControl::new(),
                options,
                |_| {},
            )
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let summary = service.upload_path_with_progress_and_path_options(
        &local,
        "/file.jar",
        SftpTransferControl::new(),
        retry,
        |_| {},
    )?;
    assert_eq!(summary.remote_path, "/file(1).jar");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let summary = service.upload_path_with_progress_and_path_options(
        &local,
        "/file.jar",
        SftpTransferControl::new(),
        independent,
        |_| {},
    )?;
    assert_eq!(summary.remote_path, "/file(2).jar");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[test]
fn changed_missing_or_unverifiable_retry_targets_are_never_opened_or_modified() -> anyhow::Result<()>
{
    for change in [
        "size",
        "mtime",
        "mode",
        "owner",
        "directory",
        "link",
        "missing",
        "denied",
        "unsupported",
        "unknown size",
        "unknown time",
    ] {
        let (service, files) = service("UTF-8");
        files.lock().unwrap().fail_write_once_at = Some(32768);
        let root = tempfile::tempdir()?;
        let local = root.path().join("source");
        std::fs::write(&local, vec![50; 256 * 1024])?;
        let (options, calls) = options(SftpDuplicateDecision::Overwrite, 0);
        let retry =
            options.with_transfer_options(SftpTransferOptions::default().with_max_retries(2));
        assert!(
            service
                .upload_path_with_progress_and_path_options(
                    &local,
                    "/file",
                    SftpTransferControl::new(),
                    options,
                    |_| {},
                )
                .is_err()
        );
        let (opens, writes, updates, contents) = {
            let mut files = files.lock().unwrap();
            match change {
                "size" => files
                    .contents
                    .get_mut(b"/file".as_slice())
                    .unwrap()
                    .push(99),
                "mtime" => files.attributes.get_mut(b"/file".as_slice()).unwrap().mtime = Some(42),
                "mode" => {
                    files
                        .attributes
                        .get_mut(b"/file".as_slice())
                        .unwrap()
                        .permissions = Some(0o100600)
                }
                "owner" => {
                    let attrs = files.attributes.get_mut(b"/file".as_slice()).unwrap();
                    attrs.uid = Some(23);
                    attrs.gid = Some(24);
                }
                "directory" => {
                    files.contents.remove(b"/file".as_slice());
                    files.attributes.remove(b"/file".as_slice());
                    files.directories.insert(b"/file".to_vec());
                }
                "link" => {
                    files
                        .contents
                        .insert(b"/other".to_vec(), b"external".to_vec());
                    files.links.insert(b"/file".to_vec(), b"/other".to_vec());
                }
                "missing" => {
                    files.contents.remove(b"/file".as_slice());
                }
                "denied" => files.stat_error = Some(3),
                "unsupported" => files.stat_error = Some(8),
                "unknown size" => files.omit_size = true,
                "unknown time" => {
                    files.attributes.get_mut(b"/file".as_slice()).unwrap().mtime = None
                }
                _ => unreachable!(),
            }
            (
                files.open_flags.len(),
                files.writes.len(),
                files.attribute_updates.len(),
                files.contents.clone(),
            )
        };
        let error = service
            .upload_path_with_progress_and_path_options(
                &local,
                "/file",
                SftpTransferControl::new(),
                retry,
                |_| {},
            )
            .unwrap_err();
        assert!(
            error.is::<super::upload_runtime::UploadRetryUnsafe>(),
            "{change}: {error:#}"
        );
        let files = files.lock().unwrap();
        assert_eq!(files.open_flags.len(), opens, "{change}");
        assert_eq!(files.writes.len(), writes, "{change}");
        assert_eq!(files.attribute_updates.len(), updates, "{change}");
        assert_eq!(files.contents, contents, "{change}");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

#[test]
fn lost_create_or_write_responses_stop_without_retrying_or_asking() -> anyhow::Result<()> {
    for request in [3, 6] {
        let (service, files) = service("UTF-8");
        files.lock().unwrap().disconnect_on_request = Some(request);
        let root = tempfile::tempdir()?;
        let local = root.path().join("source");
        std::fs::write(&local, vec![61; 128 * 1024])?;
        let (options, calls) = options(SftpDuplicateDecision::Overwrite, 2);
        let retained = options.with_transfer_options(options.transfer_options().clone());
        assert!(
            service
                .upload_path_with_progress_and_path_options(
                    &local,
                    "/file",
                    SftpTransferControl::new(),
                    options,
                    |_| {},
                )
                .is_err()
        );
        assert!(!retained.upload_runtime().retry_ready());
        assert!(
            service
                .upload_path_with_progress_and_path_options(
                    &local,
                    "/file",
                    SftpTransferControl::new(),
                    retained,
                    |_| {},
                )
                .unwrap_err()
                .is::<super::upload_runtime::UploadRetryUnsafe>()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(files.lock().unwrap().open_flags.len(), 1);
    }
    Ok(())
}

#[test]
fn raw_path_uploads_retry_the_same_bytes_from_the_start() -> anyhow::Result<()> {
    let (service, files) = service_with_config("UTF-8", Default::default(), None);
    files.lock().unwrap().fail_write_once_at = Some(32768);
    let root = tempfile::tempdir()?;
    let local = root.path().join("source");
    let payload = vec![72; 256 * 1024];
    std::fs::write(&local, &payload)?;
    let path = RemoteFilePath::from_raw("/lossy", b"/raw-\xff");
    service.upload_remote_file_with_progress_and_control_options(
        &local,
        &path,
        SftpTransferControl::new(),
        SftpTransferOptions::default()
            .with_max_retries(1)
            .with_resume_broken_transfer(true),
        |_| {},
    )?;
    let files = files.lock().unwrap();
    assert_eq!(files.contents[b"/raw-\xff".as_slice()], payload);
    assert_eq!(files.open_flags.len(), 2);
    assert!(
        files
            .open_flags
            .iter()
            .all(|(path, _)| path == b"/raw-\xff")
    );
    Ok(())
}

#[test]
fn directory_retry_reuses_a_new_or_renamed_root_and_preserves_existing_children()
-> anyhow::Result<()> {
    for (renamed, parallel) in [(false, false), (true, false), (false, true), (true, true)] {
        let (mut service, files) = service("UTF-8");
        service.config.sftp.compatibility_mode = !parallel;
        {
            let mut files = files.lock().unwrap();
            files.fail_write_once_at = Some(32768);
            if renamed {
                files.directories.insert(b"/folder".to_vec());
                files
                    .contents
                    .insert(b"/folder/keep".to_vec(), b"original".to_vec());
            }
        }
        let root = tempfile::tempdir()?;
        let local = root.path().join("source");
        std::fs::create_dir_all(local.join("sub"))?;
        let payload = vec![83; 256 * 1024];
        std::fs::write(local.join("a"), &payload)?;
        std::fs::write(local.join("sub/b"), b"nested")?;
        let (options, calls) = options(SftpDuplicateDecision::Rename, 1);
        let summary = service.upload_path_with_progress_and_path_options(
            &local,
            "/folder",
            SftpTransferControl::new(),
            options,
            |_| {},
        )?;
        let target = if renamed { "/folder(1)" } else { "/folder" };
        assert_eq!(summary.remote_path, target);
        assert_eq!(calls.load(Ordering::SeqCst), usize::from(renamed));
        let files = files.lock().unwrap();
        assert_eq!(files.contents[format!("{target}/a").as_bytes()], payload);
        assert_eq!(
            files.contents[format!("{target}/sub/b").as_bytes()],
            b"nested"
        );
        assert!(!files.directories.contains(b"/folder(2)".as_slice()));
        if renamed {
            assert_eq!(files.contents[b"/folder/keep".as_slice()], b"original");
        }
        assert_eq!(files.open_handles, 0);
    }
    Ok(())
}

#[test]
fn files_appearing_or_changing_during_open_are_not_truncated() -> anyhow::Result<()> {
    for race in ["appears", "replaced", "fstat mismatch"] {
        let (service, files) = service("UTF-8");
        {
            let mut files = files.lock().unwrap();
            if race != "appears" {
                files
                    .contents
                    .insert(b"/file".to_vec(), b"original".to_vec());
            }
            if race == "fstat mismatch" {
                files.change_before_fstat = true;
            } else {
                files.replace_on_open = Some(b"external".to_vec());
            }
        }
        let root = tempfile::tempdir()?;
        let local = root.path().join("source");
        std::fs::write(&local, b"uploaded")?;
        let (options, _) = options(SftpDuplicateDecision::Overwrite, 2);
        assert!(
            service
                .upload_path_with_progress_and_path_options(
                    &local,
                    "/file",
                    SftpTransferControl::new(),
                    options,
                    |_| {},
                )
                .is_err()
        );
        let files = files.lock().unwrap();
        assert_eq!(
            files.contents[b"/file".as_slice()],
            if race == "fstat mismatch" {
                b"original"
            } else {
                b"external"
            }
        );
        assert!(files.writes.is_empty());
        assert!(files.attribute_updates.is_empty());
        assert_eq!(files.open_flags.len(), 1);
    }
    Ok(())
}

#[test]
fn an_answered_prompt_without_a_target_snapshot_cannot_grant_future_overwrites()
-> anyhow::Result<()> {
    struct DenySnapshot(Arc<std::sync::Mutex<super::filename_tests::Files>>);
    impl SftpDuplicateResolver for DenySnapshot {
        fn resolve_duplicate(
            &self,
            _: &SftpDuplicateRequest,
        ) -> Result<SftpDuplicateDecision, String> {
            self.0.lock().unwrap().stat_error = Some(3);
            Ok(SftpDuplicateDecision::Overwrite)
        }
    }
    let (service, files) = service("UTF-8");
    files
        .lock()
        .unwrap()
        .contents
        .insert(b"/file".to_vec(), b"original".to_vec());
    let root = tempfile::tempdir()?;
    let local = root.path().join("source");
    std::fs::write(&local, b"uploaded")?;
    let options = SftpPathTransferOptions::new(
        SftpDuplicatePolicy::Ask,
        Some(Arc::new(DenySnapshot(files.clone()))),
        SftpTransferOptions::default().with_max_retries(2),
    );
    let retry = options.with_transfer_options(options.transfer_options().clone());
    assert!(
        service
            .upload_path_with_progress_and_path_options(
                &local,
                "/file",
                SftpTransferControl::new(),
                options,
                |_| {},
            )
            .unwrap_err()
            .is::<super::upload_runtime::UploadRetryUnsafe>()
    );
    files.lock().unwrap().stat_error = None;
    assert!(
        service
            .upload_path_with_progress_and_path_options(
                &local,
                "/file",
                SftpTransferControl::new(),
                retry,
                |_| {},
            )
            .unwrap_err()
            .is::<super::upload_runtime::UploadRetryUnsafe>()
    );
    let files = files.lock().unwrap();
    assert!(files.open_flags.is_empty());
    assert_eq!(files.contents[b"/file".as_slice()], b"original");
    Ok(())
}

#[test]
fn link_retries_verify_the_link_and_referent_but_ignore_access_time() -> anyhow::Result<()> {
    for change in ["unchanged", "atime", "link"] {
        let (service, files) = service("UTF-8");
        {
            let mut files = files.lock().unwrap();
            files.links.insert(b"/link".to_vec(), b"/target".to_vec());
            files
                .contents
                .insert(b"/target".to_vec(), b"original".to_vec());
            files.attributes.insert(
                b"/target".to_vec(),
                russh_sftp::protocol::FileAttributes {
                    permissions: Some(0o100755),
                    atime: Some(1),
                    mtime: Some(1),
                    ..Default::default()
                },
            );
            files.fail_write_once_at = Some(32768);
        }
        let root = tempfile::tempdir()?;
        let local = root.path().join("source");
        let payload = vec![94; 256 * 1024];
        std::fs::write(&local, &payload)?;
        let (options, calls) = options(SftpDuplicateDecision::Overwrite, 0);
        let retry = options.with_transfer_options(options.transfer_options().clone());
        assert!(
            service
                .upload_path_with_progress_and_path_options(
                    &local,
                    "/link",
                    SftpTransferControl::new(),
                    options,
                    |_| {},
                )
                .is_err()
        );
        let opens = {
            let mut files = files.lock().unwrap();
            if change == "atime" {
                files
                    .attributes
                    .get_mut(b"/target".as_slice())
                    .unwrap()
                    .atime = Some(42);
            }
            if change == "link" {
                files
                    .contents
                    .insert(b"/other".to_vec(), b"external".to_vec());
                files.links.insert(b"/link".to_vec(), b"/other".to_vec());
            }
            files.open_flags.len()
        };
        let result = service.upload_path_with_progress_and_path_options(
            &local,
            "/link",
            SftpTransferControl::new(),
            retry,
            |_| {},
        );
        let files = files.lock().unwrap();
        if change == "link" {
            assert!(
                result
                    .unwrap_err()
                    .is::<super::upload_runtime::UploadRetryUnsafe>()
            );
            assert_eq!(files.open_flags.len(), opens);
            assert_eq!(files.contents[b"/other".as_slice()], b"external");
        } else {
            result?;
            assert_eq!(files.contents[b"/target".as_slice()], payload);
            assert_eq!(
                files.attributes[b"/target".as_slice()].permissions,
                Some(0o100755)
            );
            assert_eq!(files.links[b"/link".as_slice()], b"/target");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    Ok(())
}

#[test]
fn an_unacknowledged_write_timeout_cannot_be_cleared_by_a_later_flush() -> anyhow::Result<()> {
    let (service, files) = service("UTF-8");
    service
        .compatibility
        .as_ref()
        .unwrap()
        .cache
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .sftp
        .set_timeout(1);
    files
        .lock()
        .unwrap()
        .request_delay
        .insert(6, std::time::Duration::from_secs(2));
    let root = tempfile::tempdir()?;
    let local = root.path().join("source");
    std::fs::write(&local, vec![105; 128 * 1024])?;
    let (options, calls) = options(SftpDuplicateDecision::Overwrite, 2);
    let retained = options.with_transfer_options(options.transfer_options().clone());
    let error = service
        .upload_path_with_progress_and_path_options(
            &local,
            "/file",
            SftpTransferControl::new(),
            options,
            |_| {},
        )
        .unwrap_err();
    assert!(error.is::<super::upload_runtime::UploadRetryUnsafe>());
    assert!(!retained.upload_runtime().retry_ready());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(files.lock().unwrap().open_flags.len(), 1);
    Ok(())
}

#[test]
fn ownership_changes_during_a_new_upload_cannot_be_adopted_as_a_retry_checkpoint()
-> anyhow::Result<()> {
    let (service, files) = service("UTF-8");
    files.lock().unwrap().change_owner_on_write = true;
    let root = tempfile::tempdir()?;
    let local = root.path().join("source");
    std::fs::write(&local, b"payload")?;
    let (options, calls) = options(SftpDuplicateDecision::Overwrite, 2);
    let error = service
        .upload_path_with_progress_and_path_options(
            &local,
            "/file",
            SftpTransferControl::new(),
            options,
            |_| {},
        )
        .unwrap_err();
    assert!(error.is::<super::upload_runtime::UploadRetryUnsafe>());
    assert_eq!(files.lock().unwrap().open_flags.len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn paused_uploads_can_resume_or_cancel_without_retrying() -> anyhow::Result<()> {
    for cancel in [false, true] {
        let (service, files) = service("UTF-8");
        let root = tempfile::tempdir()?;
        let local = root.path().join("source");
        std::fs::write(&local, b"payload")?;
        let (options, calls) = options(SftpDuplicateDecision::Overwrite, 2);
        let retained = options.with_transfer_options(options.transfer_options().clone());
        let control = SftpTransferControl::new();
        let worker_control = control.clone();
        let (paused, signal) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let pause = worker_control.clone();
            service.upload_path_with_progress_and_path_options(
                &local,
                "/file",
                worker_control,
                options,
                move |progress| {
                    if progress.bytes_transferred == 0 {
                        pause.pause();
                        let _ = paused.send(());
                    }
                },
            )
        });
        let started = signal
            .recv_timeout(std::time::Duration::from_secs(2))
            .is_ok();
        let writes_while_paused = files.lock().unwrap().writes.len();
        if cancel || !started {
            control.cancel();
        } else {
            control.resume();
        }
        let result = worker.join().unwrap();
        assert!(started);
        assert_eq!(writes_while_paused, 0);
        if cancel {
            assert!(super::is_sftp_transfer_cancelled(&result.unwrap_err()));
            assert!(!retained.upload_runtime().retry_ready());
        } else {
            result?;
            assert_eq!(
                files.lock().unwrap().contents[b"/file".as_slice()],
                b"payload"
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(files.lock().unwrap().open_flags.len(), 1);
    }
    Ok(())
}
