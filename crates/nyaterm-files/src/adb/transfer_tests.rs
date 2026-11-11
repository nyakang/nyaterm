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
