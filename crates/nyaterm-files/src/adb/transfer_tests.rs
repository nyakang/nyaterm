use crate::adb::test_support::AdbFixture;
use crate::file_browser::FileBrowserService;
use crate::sftp::{RemoteFilePath, SftpTransferControl};
use crate::sftp_transfer_types::{
    SftpDuplicatePolicy, SftpPathTransferOptions, SftpTransferOptions,
};

fn options(policy: SftpDuplicatePolicy) -> SftpPathTransferOptions {
    SftpPathTransferOptions::new(policy, None, SftpTransferOptions::default())
}

#[test]
fn adb_download_reports_every_chunk_and_reads_the_device_once() {
    let fixture = AdbFixture::new();
    fixture.file(b"/device/file", &vec![3; 150_000]);
    let root = tempfile::tempdir().unwrap();
    let updates = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let capture = updates.clone();
    FileBrowserService::Adb(fixture.service.clone())
        .download_remote_path_with_progress_and_path_options(
            &RemoteFilePath::new("/device/file"),
            root.path().join("file"),
            SftpTransferControl::new(),
            options(SftpDuplicatePolicy::Overwrite),
            move |value| capture.lock().unwrap().push(value.bytes_transferred),
        )
        .unwrap();
    let updates = updates.lock().unwrap();
    assert!(
        updates.contains(&131_072),
        "second chunk must advance progress"
    );
    assert_eq!(fixture.device.lock().unwrap().receives, 1);
    assert_eq!(
        std::fs::read(root.path().join("file")).unwrap(),
        vec![3; 150_000]
    );
}

#[test]
fn adb_safe_edit_rechecks_revision_after_uploading_the_temporary_file() {
    use crate::remote_file::RemoteTextWriteResult;
    let fixture = AdbFixture::new();
    let path = RemoteFilePath::new("/device/editor");
    fixture.file(b"/device/editor", b"original");
    let revision = fixture
        .service
        .read_text_document_path(&path, 1024)
        .unwrap()
        .revision;
    fixture.device.lock().unwrap().change_on_send =
        Some((b"/device/editor".to_vec(), b"external".to_vec()));
    let result = fixture
        .service
        .write_text_document_path(&path, "our edit", Some(&revision), false)
        .unwrap();
    assert!(matches!(result, RemoteTextWriteResult::Conflict));
    assert_eq!(
        fixture.device.lock().unwrap().nodes[b"/device/editor".as_slice()].data,
        b"external"
    );
    let error = fixture
        .service
        .write_text_document_path(&path, "unsafe", None, false)
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<crate::adb::AdbFilePolicyError>(),
        Some(&crate::adb::AdbFilePolicyError::RevisionRequired)
    );
    assert!(
        fixture
            .device
            .lock()
            .unwrap()
            .nodes
            .keys()
            .all(|path| !path.windows(9).any(|part| part == b".nyaterm-"))
    );
}

#[test]
fn browser_adb_upload_and_download_round_trip_special_names_and_empty_files() {
    let fixture = AdbFixture::new();
    let browser = FileBrowserService::Adb(fixture.service.clone());
    let root = tempfile::tempdir().unwrap();
    for (index, name) in [
        "space name",
        "中文",
        "quote'\"",
        "line\nbreak",
        "$(touch unexpected)",
        "a\\b",
    ]
    .into_iter()
    .enumerate()
    {
        let data: Vec<u8> = (0..if index == 0 { 0 } else { 150_000 })
            .map(|n| (n * 31) as u8)
            .collect();
        let local = root.path().join("input");
        std::fs::write(&local, &data).unwrap();
        let remote = browser
            .child_path(&RemoteFilePath::new("/device"), name)
            .unwrap();
        let summary = browser
            .upload_remote_path_with_progress_and_path_options(
                local,
                &remote,
                SftpTransferControl::new(),
                options(SftpDuplicatePolicy::Overwrite),
                |_| {},
            )
            .unwrap();
        assert_eq!(summary.bytes, data.len() as u64);
        // All names must round-trip on the Android wire, independent of host naming rules.
        let mut received = Vec::new();
        fixture
            .service
            .receive(&remote, &mut received, &SftpTransferControl::new(), |_| {})
            .unwrap();
        assert_eq!(received, data);
        let output = root.path().join(format!("outputs/{index}"));
        browser
            .download_remote_path_with_progress_and_path_options(
                &remote,
                output.clone(),
                SftpTransferControl::new(),
                options(SftpDuplicatePolicy::Overwrite),
                |_| {},
            )
            .unwrap();
        assert_eq!(std::fs::read(output).unwrap(), data);
    }
    assert!(
        fixture
            .device
            .lock()
            .unwrap()
            .nodes
            .keys()
            .all(|path| !path.windows(9).any(|w| w == b".nyaterm-"))
    );
}

#[test]
fn browser_adb_directory_copy_preserves_empty_directories_and_conflict_policies() {
    let fixture = AdbFixture::new();
    let browser = FileBrowserService::Adb(fixture.service.clone());
    let root = tempfile::tempdir().unwrap();
    let local = root.path().join("tree");
    std::fs::create_dir_all(local.join("empty")).unwrap();
    std::fs::write(local.join("file"), b"first").unwrap();
    browser
        .upload_path_with_progress_and_path_options(
            local.clone(),
            "/device/tree",
            SftpTransferControl::new(),
            options(SftpDuplicatePolicy::Overwrite),
            |_| {},
        )
        .unwrap();
    assert!(
        fixture
            .device
            .lock()
            .unwrap()
            .nodes
            .contains_key(b"/device/tree/empty".as_slice())
    );
    std::fs::write(local.join("file"), b"second").unwrap();
    let skipped = browser
        .upload_path_with_progress_and_path_options(
            local.clone(),
            "/device/tree",
            SftpTransferControl::new(),
            options(SftpDuplicatePolicy::Skip),
            |_| {},
        )
        .unwrap();
    assert!(skipped.skipped);
    let renamed = browser
        .upload_path_with_progress_and_path_options(
            local,
            "/device/tree",
            SftpTransferControl::new(),
            options(SftpDuplicatePolicy::Rename),
            |_| {},
        )
        .unwrap();
    assert_eq!(renamed.remote_path, "/device/tree (1)");
    let output = root.path().join("download");
    browser
        .download_remote_path_with_progress_and_path_options(
            &RemoteFilePath::new("/device/tree"),
            output.clone(),
            SftpTransferControl::new(),
            options(SftpDuplicatePolicy::Overwrite),
            |_| {},
        )
        .unwrap();
    assert!(output.join("empty").is_dir());
    assert_eq!(std::fs::read(output.join("file")).unwrap(), b"first");
}

#[test]
fn cancelled_adb_upload_keeps_destination_and_removes_its_temporary_file() {
    let fixture = AdbFixture::new();
    fixture.file(b"/device/target", b"original");
    let local = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(local.path(), vec![1; 150_000]).unwrap();
    let control = SftpTransferControl::new();
    let cancel = control.clone();
    assert!(
        fixture
            .service
            .upload(
                local.path(),
                &RemoteFilePath::new("/device/target"),
                true,
                &control,
                move |bytes| {
                    if bytes > 0 {
                        cancel.cancel();
                    }
                }
            )
            .is_err()
    );
    let state = fixture.device.lock().unwrap();
    assert_eq!(
        state.nodes.get(b"/device/target".as_slice()).unwrap().data,
        b"original"
    );
    assert_eq!(state.nodes.len(), 3);
}

#[test]
fn changed_adb_download_source_does_not_replace_existing_local_data() {
    let fixture = AdbFixture::new();
    fixture.file(b"/device/source", b"content");
    fixture.device.lock().unwrap().change_on_receive = true;
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("output");
    std::fs::write(&output, b"original").unwrap();
    assert!(
        fixture
            .service
            .download(
                &RemoteFilePath::new("/device/source"),
                &output,
                true,
                &SftpTransferControl::new(),
                |_| {}
            )
            .is_err()
    );
    assert_eq!(std::fs::read(&output).unwrap(), b"original");
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn adb_promised_download_preserves_existing_targets_and_checks_the_original_source_type() {
    use crate::sftp::SftpFileType;
    let fixture = AdbFixture::new();
    fixture.file(b"/device/file", b"new data");
    let browser = FileBrowserService::Adb(fixture.service.clone());
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("existing");
    std::fs::write(&target, b"original").unwrap();
    let summary = browser
        .download_remote_path_with_progress_and_path_options(
            &RemoteFilePath::new("/device/file"),
            target.clone(),
            SftpTransferControl::new(),
            SftpPathTransferOptions::for_promised_download(
                SftpTransferOptions::default(),
                SftpFileType::File,
            ),
            |_| {},
        )
        .unwrap();
    assert!(summary.skipped);
    assert_eq!(std::fs::read(&target).unwrap(), b"original");
    assert_eq!(fixture.device.lock().unwrap().receives, 0);
    let error = browser
        .download_remote_path_with_progress_and_path_options(
            &RemoteFilePath::new("/device/file"),
            root.path().join("new"),
            SftpTransferControl::new(),
            SftpPathTransferOptions::for_promised_download(
                SftpTransferOptions::default(),
                SftpFileType::Directory,
            ),
            |_| {},
        )
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<crate::file_browser::FileTransferEntryError>(),
        Some(crate::file_browser::FileTransferEntryError::SourceChanged)
    ));
    assert!(!root.path().join("new").exists());
}

#[test]
fn adb_ask_decision_is_retained_across_retries_and_download_cancellation_cleans_staging() {
    use crate::sftp_transfer_types::{
        SftpDuplicateDecision, SftpDuplicateRequest, SftpDuplicateResolver,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Resolver(Arc<AtomicUsize>);
    impl SftpDuplicateResolver for Resolver {
        fn resolve_duplicate(
            &self,
            _: &SftpDuplicateRequest,
        ) -> Result<SftpDuplicateDecision, String> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(SftpDuplicateDecision::Overwrite)
        }
    }
    let fixture = AdbFixture::new();
    fixture.file(b"/device/file", &vec![3; 150_000]);
    let browser = FileBrowserService::Adb(fixture.service.clone());
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("existing");
    std::fs::write(&target, b"original").unwrap();
    let decisions = Arc::new(AtomicUsize::new(0));
    let options = SftpPathTransferOptions::new(
        SftpDuplicatePolicy::Ask,
        Some(Arc::new(Resolver(decisions.clone()))),
        SftpTransferOptions::default(),
    );
    let control = SftpTransferControl::new();
    let cancel = control.clone();
    assert!(
        browser
            .download_remote_path_with_progress_and_path_options(
                &RemoteFilePath::new("/device/file"),
                target.clone(),
                control,
                options.with_transfer_options(SftpTransferOptions::default()),
                move |progress| {
                    if progress.bytes_transferred > 0 {
                        cancel.cancel();
                    }
                }
            )
            .is_err()
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"original");
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    browser
        .download_remote_path_with_progress_and_path_options(
            &RemoteFilePath::new("/device/file"),
            target.clone(),
            SftpTransferControl::new(),
            options.with_transfer_options(SftpTransferOptions::default()),
            |_| {},
        )
        .unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), vec![3; 150_000]);
    assert_eq!(decisions.load(Ordering::Relaxed), 1);
}

#[test]
fn adb_promised_directory_retry_reuses_only_its_own_entries_and_rejects_replaced_roots() {
    use crate::sftp::SftpFileType;
    let fixture = AdbFixture::new();
    fixture
        .service
        .create_dir_remote_path(&RemoteFilePath::new("/device/tree"), None)
        .unwrap();
    fixture.file(b"/device/tree/a", b"a");
    fixture.file(b"/device/tree/b", &vec![2; 150_000]);
    let browser = FileBrowserService::Adb(fixture.service.clone());
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("tree");
    let options = SftpPathTransferOptions::for_promised_download(
        SftpTransferOptions::default(),
        SftpFileType::Directory,
    );
    let control = SftpTransferControl::new();
    let cancel = control.clone();
    assert!(
        browser
            .download_remote_path_with_progress_and_path_options(
                &RemoteFilePath::new("/device/tree"),
                output.clone(),
                control,
                options.with_transfer_options(SftpTransferOptions::default()),
                move |progress| {
                    if progress.bytes_transferred > 1 {
                        cancel.cancel();
                    }
                }
            )
            .is_err()
    );
    assert_eq!(std::fs::read(output.join("a")).unwrap(), b"a");
    assert!(!output.join("b").exists());
    let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let updates = observed.clone();
    let summary = browser
        .download_remote_path_with_progress_and_path_options(
            &RemoteFilePath::new("/device/tree"),
            output.clone(),
            SftpTransferControl::new(),
            options.with_transfer_options(SftpTransferOptions::default()),
            move |progress| updates.lock().unwrap().push(progress.bytes_transferred),
        )
        .unwrap();
    assert_eq!(summary.bytes, 150_001);
    assert_eq!(summary.local_path, output);
    assert_eq!(std::fs::read(output.join("b")).unwrap(), vec![2; 150_000]);
    let updates = observed.lock().unwrap();
    assert!(updates.windows(2).all(|pair| pair[0] <= pair[1]));
    assert!(updates.iter().all(|bytes| *bytes <= summary.bytes));
    std::fs::rename(&output, root.path().join("original")).unwrap();
    std::fs::create_dir(&output).unwrap();
    std::fs::write(output.join("external"), b"keep").unwrap();
    assert!(
        browser
            .download_remote_path_with_progress_and_path_options(
                &RemoteFilePath::new("/device/tree"),
                output.clone(),
                SftpTransferControl::new(),
                options.with_transfer_options(SftpTransferOptions::default()),
                |_| {}
            )
            .is_err()
    );
    assert_eq!(std::fs::read(output.join("external")).unwrap(), b"keep");
    assert!(!output.join("a").exists());
}

#[test]
fn changed_local_upload_source_keeps_remote_destination_and_cleans_staging() {
    let fixture = AdbFixture::new();
    fixture.file(b"/device/target", b"original");
    let local = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(local.path(), vec![1; 150_000]).unwrap();
    let input = local.path().to_path_buf();
    let mut changed = false;
    assert!(
        fixture
            .service
            .upload(
                local.path(),
                &RemoteFilePath::new("/device/target"),
                true,
                &SftpTransferControl::new(),
                move |bytes| {
                    if bytes > 0 && !changed {
                        std::fs::write(&input, vec![2; 150_000]).unwrap();
                        changed = true;
                    }
                }
            )
            .is_err()
    );
    let state = fixture.device.lock().unwrap();
    assert_eq!(
        state.nodes.get(b"/device/target".as_slice()).unwrap().data,
        b"original"
    );
    assert!(
        state
            .nodes
            .keys()
            .all(|path| !path.windows(9).any(|bytes| bytes == b".nyaterm-"))
    );
}

#[test]
fn adb_upload_preserves_target_permissions_and_download_preserves_requested_time() {
    let fixture = AdbFixture::new();
    fixture.file(b"/device/file", b"original");
    fixture
        .device
        .lock()
        .unwrap()
        .nodes
        .get_mut(b"/device/file".as_slice())
        .unwrap()
        .mode = 0o100755;
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("input");
    std::fs::write(&input, b"new content").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&input)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(12345))
        .unwrap();
    let browser = FileBrowserService::Adb(fixture.service.clone());
    let options = SftpPathTransferOptions::new(
        SftpDuplicatePolicy::Overwrite,
        None,
        SftpTransferOptions::default()
            .with_preserve_timestamps(true)
            .with_default_file_permissions("600"),
    );
    browser
        .upload_remote_path_with_progress_and_path_options(
            input,
            &RemoteFilePath::new("/device/file"),
            SftpTransferControl::new(),
            options.clone(),
            |_| {},
        )
        .unwrap();
    let node = fixture.device.lock().unwrap().nodes[b"/device/file".as_slice()].clone();
    assert_eq!(node.mode & 0o777, 0o755);
    assert_eq!(node.modified, 12345);
    assert_eq!(fixture.device.lock().unwrap().sends, 1);
    assert_eq!(fixture.device.lock().unwrap().receives, 0);
    let output = root.path().join("output");
    browser
        .download_remote_path_with_progress_and_path_options(
            &RemoteFilePath::new("/device/file"),
            output.clone(),
            SftpTransferControl::new(),
            options,
            |_| {},
        )
        .unwrap();
    assert_eq!(
        std::fs::metadata(output).unwrap().modified().unwrap(),
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(12345)
    );
    assert_eq!(fixture.device.lock().unwrap().receives, 1);
}

#[test]
fn device_invalidation_stops_a_paused_adb_transfer_without_resuming_it() {
    let fixture = AdbFixture::new();
    fixture.file(b"/device/file", &vec![3; 150_000]);
    let service = fixture.service.clone();
    let control = SftpTransferControl::new();
    let worker_control = control.clone();
    let (paused_tx, paused_rx) = std::sync::mpsc::sync_channel(1);
    let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let mut sent = false;
        let result = service.receive(
            &RemoteFilePath::new("/device/file"),
            &mut Vec::new(),
            &worker_control,
            |_| {
                if !sent {
                    worker_control.pause();
                    paused_tx.send(()).unwrap();
                    sent = true;
                }
            },
        );
        done_tx.send(result.is_err()).unwrap();
    });
    paused_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    fixture.service.runtime.invalidate();
    assert!(
        done_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
    );
    worker.join().unwrap();
}

#[test]
fn adb_retries_temporary_data_but_never_replays_a_failed_commit() {
    let fixture = AdbFixture::new();
    let root = tempfile::tempdir().unwrap();
    let local = root.path().join("source");
    std::fs::write(&local, vec![9; 150_000]).unwrap();
    let path = RemoteFilePath::new("/device/file");
    let options = SftpTransferOptions::default().with_max_retries(2);
    fixture.device.lock().unwrap().fail_sends = 1;
    fixture
        .service
        .upload_with_options(
            &local,
            &path,
            false,
            &SftpTransferControl::new(),
            &options,
            |_| {},
        )
        .unwrap();
    assert_eq!(fixture.device.lock().unwrap().sends, 2);
    assert_eq!(fixture.device.lock().unwrap().renames, 1);
    fixture.device.lock().unwrap().fail_receives = 1;
    let target = root.path().join("download");
    fixture
        .service
        .download_with_options(
            &path,
            &target,
            false,
            &SftpTransferControl::new(),
            &options,
            |_| {},
        )
        .unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), vec![9; 150_000]);
    assert_eq!(fixture.device.lock().unwrap().receives, 2);
    fixture.device.lock().unwrap().fail_rename = true;
    assert!(
        fixture
            .service
            .upload_with_options(
                &local,
                &path,
                true,
                &SftpTransferControl::new(),
                &options,
                |_| {}
            )
            .is_err()
    );
    assert_eq!(fixture.device.lock().unwrap().renames, 2);
    assert!(
        fixture
            .device
            .lock()
            .unwrap()
            .nodes
            .keys()
            .all(|path| !path.windows(9).any(|part| part == b".nyaterm-"))
    );
}
#[test]
fn adb_safe_edit_detects_same_size_same_timestamp_content_change_and_force_keeps_type_protection() {
    use crate::remote_file::{RemoteTextMetadata, RemoteTextRevision, RemoteTextWriteResult};
    let fixture = AdbFixture::new();
    let path = RemoteFilePath::new("/device/edit");
    fixture.file(b"/device/edit", b"original");
    let revision = fixture
        .service
        .read_text_document_path(&path, 1024)
        .unwrap()
        .revision;
    fixture.device.lock().unwrap().preserve_change_time = true;
    fixture.device.lock().unwrap().change_on_send =
        Some((b"/device/edit".to_vec(), b"external".to_vec()));
    assert!(matches!(
        fixture
            .service
            .write_text_document_path(&path, "our edit", Some(&revision), false)
            .unwrap(),
        RemoteTextWriteResult::Conflict
    ));
    let result = fixture
        .service
        .write_text_document_path(&path, "our edit", None, true)
        .unwrap();
    let stat = fixture.service.stat(&path).unwrap();
    let RemoteTextWriteResult::Saved { revision } = result else {
        panic!("forced save must succeed");
    };
    assert_eq!(
        revision,
        RemoteTextRevision::from_bytes(
            b"our edit",
            RemoteTextMetadata {
                size: 8,
                modified_at: Some(stat.modified)
            }
        )
    );
    fixture
        .device
        .lock()
        .unwrap()
        .nodes
        .get_mut(b"/device/edit".as_slice())
        .unwrap()
        .mode = 0o120777;
    let sends = fixture.device.lock().unwrap().sends;
    assert!(
        fixture
            .service
            .write_text_document_path(&path, "unsafe", None, true)
            .is_err()
    );
    assert_eq!(fixture.device.lock().unwrap().sends, sends);
}

#[test]
fn android_protocol_names_are_independent_of_windows_host_names() {
    use crate::file_browser::{FileBrowserBackendKind, valid_file_browser_child_name};
    for name in ["quote\"", "line\nbreak", "a\\b", "a:b"] {
        assert!(valid_file_browser_child_name(
            FileBrowserBackendKind::Adb,
            name
        ));
        if cfg!(windows) {
            assert!(!valid_file_browser_child_name(
                FileBrowserBackendKind::Local,
                name
            ));
        }
    }
}

#[test]
fn default_new_upload_accepts_only_confirmed_mount_assigned_permissions() {
    use crate::adb::AdbFilePolicyError;
    for (mount_assigned, explicit, existing) in [
        (true, false, false),
        (true, true, false),
        (true, false, true),
        (false, false, false),
    ] {
        let fixture = AdbFixture::new();
        {
            let mut device = fixture.device.lock().unwrap();
            device.ignore_chmod = true;
            device.permission_probe = Some(if mount_assigned {
                b"/storage/emulated/0\n\0fuse\n\x001 0 0:1 / /storage/emulated rw - fuse /dev/fuse rw\n".to_vec()
            } else {
                b"/device\n\0fuse\n\x001 0 0:1 / / rw - fuse /dev/fuse rw\n".to_vec()
            });
        }
        if existing {
            fixture.file(b"/device/file", b"original");
        }
        let root = tempfile::tempdir().unwrap();
        let local = root.path().join("input");
        std::fs::write(&local, b"replacement").unwrap();
        let mut options = SftpTransferOptions::default();
        if explicit {
            options.default_file_mode = Some(0o600);
        }
        let result = fixture.service.upload_with_options(
            &local,
            &RemoteFilePath::new("/device/file"),
            true,
            &SftpTransferControl::new(),
            &options,
            |_| {},
        );
        if mount_assigned && !explicit && !existing {
            assert_eq!(result.unwrap(), 11);
            assert_eq!(
                fixture.device.lock().unwrap().nodes[b"/device/file".as_slice()].mode & 0o777,
                0o666
            );
        } else {
            assert_eq!(
                result.unwrap_err().downcast_ref::<AdbFilePolicyError>(),
                Some(&AdbFilePolicyError::PermissionsUnavailable)
            );
            if existing {
                assert_eq!(
                    fixture.device.lock().unwrap().nodes[b"/device/file".as_slice()].data,
                    b"original"
                );
            }
            assert_eq!(fixture.device.lock().unwrap().renames, 0);
        }
        assert!(
            fixture
                .device
                .lock()
                .unwrap()
                .nodes
                .keys()
                .all(|key| !key.windows(9).any(|s| s == b".nyaterm-"))
        );
    }
}

#[test]
fn cancellation_at_commit_never_renames_the_staged_upload() {
    use crate::adb::AdbTransferEvent;
    use crate::sftp_transfer_types::FileTransferPhase;
    let fixture = AdbFixture::new();
    fixture.file(b"/device/file", b"original");
    let root = tempfile::tempdir().unwrap();
    let local = root.path().join("input");
    std::fs::write(&local, b"replacement").unwrap();
    let control = SftpTransferControl::new();
    let result = fixture.service.upload_with_options(
        &local,
        &RemoteFilePath::new("/device/file"),
        true,
        &control,
        &SftpTransferOptions::default(),
        |event| {
            if matches!(
                event,
                AdbTransferEvent::Phase(FileTransferPhase::Committing)
            ) {
                control.cancel();
            }
        },
    );
    assert!(result.is_err());
    let state = fixture.device.lock().unwrap();
    assert_eq!(state.renames, 0);
    assert_eq!(state.nodes[b"/device/file".as_slice()].data, b"original");
    assert!(
        state
            .nodes
            .keys()
            .all(|key| !key.windows(9).any(|s| s == b".nyaterm-"))
    );
}
