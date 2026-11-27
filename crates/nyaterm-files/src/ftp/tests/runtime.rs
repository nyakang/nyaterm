use super::{FakeNode, TreeServer, connect_tree};
use crate::file_browser::{FileBrowserService, WeakFileBrowserService};
use crate::ftp::pool::ConnectionFactory;
use crate::ftp::resume::ResumeRuntime;
use crate::ftp::{FtpError, FtpFileService, FtpLogin};
use crate::sftp::{RemoteFilePath, SftpTransferControl};
use crate::sftp_transfer_types::{
    SftpDuplicatePolicy, SftpPathTransferOptions, SftpTransferOptions,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

fn pooled(server: &TreeServer, limit: usize) -> (Arc<FtpFileService>, Arc<ConnectionFactory>) {
    let service = Arc::new(connect_tree(server));
    let config = server.config();
    let factory: Arc<ConnectionFactory> = Arc::new(move |cancelled| {
        FtpFileService::connect_with_cancel(config.clone(), FtpLogin::anonymous(""), move || {
            cancelled()
        })
    });
    service.configure_connections(Arc::downgrade(&factory), limit, 0);
    (service, factory)
}

fn options() -> SftpTransferOptions {
    SftpTransferOptions::default()
        .with_resume_broken_transfer(true)
        .with_max_retries(2)
}

#[test]
fn pooled_transfer_does_not_block_directory_browsing_and_close_cancels_waiters() {
    let server = TreeServer::start_connections(vec![("/stalled", FakeNode::File(vec![0; 12]))], 2);
    let (service, _factory) = pooled(&server, 1);
    let reader = service.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        tx.send(reader.retr_to_writer("/stalled", &mut Vec::new(), |_| Ok(())))
            .unwrap();
    });
    super::wait_for_retrieve(&server);
    let (listed_tx, listed_rx) = std::sync::mpsc::channel();
    let listing = service.clone();
    let lister = std::thread::spawn(move || listed_tx.send(listing.list_dir("/")).unwrap());
    assert_eq!(
        listed_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap()
            .len(),
        1
    );
    lister.join().unwrap();
    let waiter = service.clone();
    let (waiting_tx, waiting_rx) = std::sync::mpsc::channel();
    let queued = std::thread::spawn(move || {
        waiting_tx
            .send(waiter.retr_to_writer("/stalled", &mut Vec::new(), |_| Ok(())))
            .unwrap()
    });
    service.disconnect();
    assert!(
        waiting_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .is_err()
    );
    assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap().is_err());
    worker.join().unwrap();
    queued.join().unwrap();
    assert_eq!(
        server
            .commands()
            .iter()
            .filter(|c| c.starts_with("USER "))
            .count(),
        2
    );
}

#[test]
fn interrupted_download_resumes_without_replacing_the_original_until_complete() {
    for reject_rest in [false, true] {
        let bytes: Vec<u8> = (0..300_000).map(|i| (i * 17) as u8).collect();
        let server =
            TreeServer::start_connections(vec![("/file", FakeNode::File(bytes.clone()))], 6);
        server
            .faults
            .drop_download
            .store(100_000, Ordering::Release);
        server
            .faults
            .reject_rest
            .store(reject_rest, Ordering::Release);
        let (service, _factory) = pooled(&server, 2);
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        std::fs::write(&target, b"original").unwrap();
        let runtime = ResumeRuntime::default();
        let mut last = 0;
        let count = runtime
            .download(
                &service,
                "/file",
                root.path(),
                &target,
                true,
                &SftpTransferControl::new(),
                &options(),
                true,
                |done, _| {
                    assert_eq!(std::fs::read(&target).unwrap(), b"original");
                    last = done;
                },
            )
            .unwrap();
        assert_eq!(count, bytes.len() as u64);
        assert_eq!(last, count);
        assert_eq!(std::fs::read(target).unwrap(), bytes);
        assert!(server.commands().iter().any(|c| c.starts_with("REST ")));
        service.disconnect();
    }
}

#[test]
fn interrupted_upload_resumes_task_temporary_and_commits_once() {
    for reject_rest in [false, true] {
        let bytes: Vec<u8> = (0..300_000).map(|i| (i * 13) as u8).collect();
        let server =
            TreeServer::start_connections(vec![("/file", FakeNode::File(b"original".to_vec()))], 6);
        server.faults.drop_upload.store(96_000, Ordering::Release);
        server
            .faults
            .reject_rest
            .store(reject_rest, Ordering::Release);
        let (service, _factory) = pooled(&server, 2);
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        std::fs::write(&source, &bytes).unwrap();
        let result = ResumeRuntime::default()
            .upload(
                &service,
                &source,
                "/file",
                true,
                &SftpTransferControl::new(),
                &options(),
                true,
                None,
                |_, _| {
                    assert_eq!(server.read("/file").unwrap(), b"original");
                },
            )
            .unwrap();
        assert_eq!(result.bytes, bytes.len() as u64);
        assert_eq!(server.read("/file").unwrap(), bytes);
        assert!(server.temporary_paths().is_empty());
        assert!(server.commands().iter().any(|c| c.starts_with("REST ")));
        assert_eq!(
            server
                .commands()
                .iter()
                .filter(|c| c.as_str() == "RNTO /file")
                .count(),
            1
        );
        service.disconnect();
    }
}

#[test]
fn manual_retry_keeps_owned_download_checkpoint() {
    let bytes = vec![42; 200_000];
    let server = TreeServer::start_connections(vec![("/file", FakeNode::File(bytes.clone()))], 6);
    let (service, _factory) = pooled(&server, 1);
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("file");
    let runtime = ResumeRuntime::default();
    let options = options().with_max_retries(0);
    server.faults.drop_download.store(90_000, Ordering::Release);
    assert!(
        runtime
            .download(
                &service,
                "/file",
                root.path(),
                &path,
                false,
                &SftpTransferControl::new(),
                &options,
                true,
                |_, _| {}
            )
            .is_err()
    );
    assert!(!path.exists());
    runtime
        .download(
            &service,
            "/file",
            root.path(),
            &path,
            false,
            &SftpTransferControl::new(),
            &options,
            true,
            |_, _| {},
        )
        .unwrap();
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    assert!(server.commands().iter().any(|c| c == "REST 90000"));
    service.disconnect();
}

#[test]
fn external_binary_edit_ignores_ascii_mode_and_requires_conflict_confirmation() {
    let original = b"PK\0\r\noriginal";
    let server = TreeServer::start_connections(
        vec![("/no-extension", FakeNode::File(original.to_vec()))],
        1,
    );
    let mut config = server.config();
    config.ascii_mode = true;
    let ftp = Arc::new(FtpFileService::connect(config, FtpLogin::anonymous("")).unwrap());
    let service = FileBrowserService::Ftp(ftp.clone());
    let directory = tempfile::tempdir().unwrap();
    let local = directory.path().join("no-extension");
    let target = service
        .download_external_edit(
            &RemoteFilePath::new("/no-extension"),
            &local,
            SftpTransferControl::new(),
            options(),
            |_| {},
        )
        .unwrap();
    assert_eq!(std::fs::read(&local).unwrap(), original);
    std::fs::write(&local, b"PK\0\r\nnew").unwrap();
    server.tree.lock().unwrap().insert(
        "/no-extension".into(),
        FakeNode::File(b"someone else's edit".to_vec()),
    );
    let error = target
        .upload(&local, SftpTransferControl::new(), options(), |_| {})
        .err()
        .unwrap();
    assert_eq!(error.downcast_ref::<FtpError>(), Some(&FtpError::Conflict));
    assert!(target.has_conflict());
    assert_eq!(
        server.read("/no-extension").unwrap(),
        b"someone else's edit"
    );
    // A later watcher notification cannot approve an earlier conflict.
    assert_eq!(
        target
            .upload(&local, SftpTransferControl::new(), options(), |_| {})
            .err()
            .unwrap()
            .downcast_ref::<FtpError>(),
        Some(&FtpError::Conflict)
    );
    target.approve_conflict();
    target
        .upload(&local, SftpTransferControl::new(), options(), |_| {})
        .unwrap();
    assert_eq!(server.read("/no-extension").unwrap(), b"PK\0\r\nnew");
    assert!(!target.has_conflict());
    ftp.disconnect();
}

#[test]
fn ftp_virtual_streams_share_one_snapshot_and_expire_with_the_session() {
    use crate::drag_export::{FtpReadSource, RemoteDragFile};
    let bytes: Vec<u8> = (0..120_000).map(|i| (i * 19) as u8).collect();
    let server = TreeServer::start(vec![("/file", FakeNode::File(bytes.clone()))]);
    let service = Arc::new(connect_tree(&server));
    let source = FtpReadSource::new(
        Arc::downgrade(&service),
        RemoteDragFile {
            display_name: "file".into(),
            size: Some(bytes.len() as u64),
            modified_at: None,
            remote_path: RemoteFilePath::new("/file"),
        },
        Arc::new(|| Arc::new(|_| {})),
        SftpTransferControl::new(),
    )
    .with_path_options(
        SftpPathTransferOptions::new(SftpDuplicatePolicy::Skip, None, options())
            .with_staging_permissions(|_| Ok(())),
    );
    let mut first = source.open().unwrap();
    let mut second = source.open().unwrap();
    assert!(!server.commands().iter().any(|c| c.starts_with("RETR ")));
    for (use_first, offset) in [(true, 100_000usize), (false, 3usize), (true, 0usize)] {
        let reader = if use_first { &mut first } else { &mut second };
        let mut buffer = [0; 32];
        assert_eq!(reader.read_at(offset as u64, &mut buffer).unwrap(), 32);
        assert_eq!(&buffer, &bytes[offset..offset + 32]);
    }
    assert_eq!(
        server
            .commands()
            .iter()
            .filter(|c| c.as_str() == "RETR /file")
            .count(),
        1
    );
    service.disconnect();
    assert!(first.read_at(0, &mut [0; 8]).is_err());
    assert!(
        WeakFileBrowserService::Ftp(Arc::downgrade(&service))
            .upgrade()
            .is_none()
    );
}

#[test]
fn ftp_export_inventory_includes_empty_directories_and_rejects_links() {
    let server = TreeServer::start(vec![
        ("/folder", FakeNode::Directory),
        ("/folder/empty", FakeNode::Directory),
        ("/folder/中文", FakeNode::File(vec![1, 2])),
    ]);
    let service = FileBrowserService::Ftp(Arc::new(connect_tree(&server)));
    let entry = service.list_dir("/").unwrap().remove(0);
    let entries = crate::drag_export::tree::enumerate_file_browser_drag_with_root_controls(
        &service,
        vec![entry.clone()],
        &SftpTransferControl::new(),
        Vec::new(),
    )
    .unwrap();
    assert_eq!(entries.len(), 3);
    assert!(
        entries
            .iter()
            .any(|e| e.is_directory && e.relative_path.ends_with("empty"))
    );
    server
        .tree
        .lock()
        .unwrap()
        .insert("/folder/link".into(), FakeNode::Symlink);
    assert!(
        crate::drag_export::tree::enumerate_file_browser_drag_with_root_controls(
            &service,
            vec![entry],
            &SftpTransferControl::new(),
            Vec::new()
        )
        .is_err()
    );
}

#[test]
fn missing_source_timestamp_restarts_instead_of_splicing_an_unverified_prefix() {
    let bytes = vec![23; 160_000];
    let server = TreeServer::start_connections(vec![("/file", FakeNode::File(bytes.clone()))], 4);
    server.faults.omit_modified.store(true, Ordering::Release);
    server.faults.drop_download.store(90_000, Ordering::Release);
    let (service, _factory) = pooled(&server, 1);
    let root = tempfile::tempdir().unwrap();
    let local = root.path().join("file");
    ResumeRuntime::default()
        .download(
            &service,
            "/file",
            root.path(),
            &local,
            false,
            &SftpTransferControl::new(),
            &options(),
            true,
            |_, _| {},
        )
        .unwrap();
    assert_eq!(std::fs::read(local).unwrap(), bytes);
    assert!(!server.commands().iter().any(|c| c.starts_with("REST ")));
    service.disconnect();
}

#[test]
fn keepalive_probes_idle_connections_and_recovers_a_dead_metadata_connection() {
    let server = TreeServer::start_connections(vec![], 2);
    let (service, _factory) = pooled(&server, 1);
    service.keep_alive();
    assert!(!server.commands().iter().any(|c| c == "NOOP"));
    service.pool.expire_metadata(false);
    service.keep_alive();
    assert_eq!(
        server
            .commands()
            .iter()
            .filter(|c| c.as_str() == "NOOP")
            .count(),
        1
    );
    service.pool.expire_metadata(true);
    service.keep_alive();
    assert!(service.list_dir("/").unwrap().is_empty());
    assert_eq!(
        server
            .commands()
            .iter()
            .filter(|c| c.starts_with("USER "))
            .count(),
        2
    );
    service.disconnect();
}

#[test]
fn authentication_and_tls_failures_are_not_retried_and_close_cancels_connection_creation() {
    let server = TreeServer::start(vec![]);
    let service = Arc::new(connect_tree(&server));
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    service.pool.expire_metadata(true);
    for (index, error) in [FtpError::AuthenticationFailed, FtpError::Tls]
        .into_iter()
        .enumerate()
    {
        let recorded = calls.clone();
        let expected = error.clone();
        let factory: Arc<ConnectionFactory> = Arc::new(move |_| {
            recorded.fetch_add(1, Ordering::AcqRel);
            Err(error.clone())
        });
        service.configure_connections(Arc::downgrade(&factory), 1, 3);
        assert_eq!(service.list_dir("/").err().unwrap(), expected);
        service.keep_alive();
        service.keep_alive();
        assert_eq!(calls.load(Ordering::Acquire), index + 1);
    }
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let factory: Arc<ConnectionFactory> = Arc::new(move |cancelled| {
        started_tx.send(()).unwrap();
        while !cancelled() {
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(FtpError::Cancelled)
    });
    service.configure_connections(Arc::downgrade(&factory), 1, 3);
    let worker_service = service.clone();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || done_tx.send(worker_service.list_dir("/")).unwrap());
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    service.disconnect();
    assert!(
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .is_err()
    );
    worker.join().unwrap();
}

#[test]
fn changed_resume_boundary_never_commits_mixed_download_contents() {
    let server =
        TreeServer::start_connections(vec![("/file", FakeNode::File(vec![42; 160_000]))], 5);
    let (service, _factory) = pooled(&server, 1);
    let root = tempfile::tempdir().unwrap();
    let local = root.path().join("file");
    std::fs::write(&local, b"original").unwrap();
    let runtime = ResumeRuntime::default();
    server.faults.drop_download.store(90_000, Ordering::Release);
    assert!(
        runtime
            .download(
                &service,
                "/file",
                root.path(),
                &local,
                true,
                &SftpTransferControl::new(),
                &options().with_max_retries(0),
                true,
                |_, _| {}
            )
            .is_err()
    );
    server
        .tree
        .lock()
        .unwrap()
        .insert("/file".into(), FakeNode::File(vec![7; 160_000]));
    let error = runtime
        .download(
            &service,
            "/file",
            root.path(),
            &local,
            true,
            &SftpTransferControl::new(),
            &options(),
            true,
            |_, _| {},
        )
        .err()
        .unwrap();
    assert_eq!(
        error.downcast_ref::<FtpError>(),
        Some(&FtpError::SourceChanged)
    );
    assert_eq!(std::fs::read(local).unwrap(), b"original");
    service.disconnect();
}

#[test]
fn replacing_the_local_upload_source_never_commits_the_old_handle() {
    let server =
        TreeServer::start_connections(vec![("/file", FakeNode::File(b"original".to_vec()))], 2);
    let (service, _factory) = pooled(&server, 1);
    let root = tempfile::tempdir().unwrap();
    let local = root.path().join("file");
    std::fs::write(&local, vec![1; 200_000]).unwrap();
    let mut changed = false;
    let error = ResumeRuntime::default()
        .upload(
            &service,
            &local,
            "/file",
            true,
            &SftpTransferControl::new(),
            &options(),
            true,
            None,
            |bytes, _| {
                if bytes > 0 && !changed {
                    changed = true;
                    std::fs::rename(&local, root.path().join("old")).unwrap();
                    std::fs::write(&local, vec![2; 200_000]).unwrap();
                }
            },
        )
        .err()
        .unwrap();
    assert_eq!(
        error.downcast_ref::<FtpError>(),
        Some(&FtpError::SourceChanged)
    );
    assert_eq!(server.read("/file").unwrap(), b"original");
    assert!(server.temporary_paths().is_empty());
    service.disconnect();
}

#[test]
fn paused_binary_download_releases_the_only_transfer_connection() {
    let bytes = vec![42; 400_000];
    let server = TreeServer::start_connections(
        vec![
            ("/file", FakeNode::File(bytes.clone())),
            ("/small", FakeNode::File(b"other transfer".to_vec())),
        ],
        5,
    );
    let (service, _factory) = pooled(&server, 1);
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().to_path_buf();
    let local = directory.join("file");
    let target = local.clone();
    let control = SftpTransferControl::new();
    let worker_control = control.clone();
    let worker_service = service.clone();
    let (paused_tx, paused_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut paused = false;
        ResumeRuntime::default().download(
            &worker_service,
            "/file",
            &directory,
            &target,
            false,
            &worker_control,
            &options(),
            true,
            |done, _| {
                if done > 0 && !paused {
                    paused = true;
                    worker_control.pause();
                    paused_tx.send(()).unwrap();
                }
            },
        )
    });
    paused_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let other_service = service.clone();
    let (other_tx, other_rx) = std::sync::mpsc::channel();
    let other = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let result = other_service.retr_to_writer("/small", &mut buffer, |_| Ok(()));
        other_tx.send((result, buffer)).unwrap();
    });
    let result = other_rx.recv_timeout(Duration::from_secs(5));
    // Always release the worker before checking assertions.
    control.resume();
    worker.join().unwrap().unwrap();
    other.join().unwrap();
    let (result, buffer) = result.expect("a paused transfer must release its pool slot");
    result.unwrap();
    assert_eq!(buffer, b"other transfer");
    assert_eq!(std::fs::read(local).unwrap(), bytes);
    assert!(server.commands().iter().any(|c| c.starts_with("REST ")));
    service.disconnect();
}

#[test]
fn manual_upload_retry_reuses_only_its_own_temporary() {
    let bytes = vec![19; 220_000];
    let server =
        TreeServer::start_connections(vec![("/file", FakeNode::File(b"original".to_vec()))], 5);
    let (service, _factory) = pooled(&server, 1);
    let root = tempfile::tempdir().unwrap();
    let local = root.path().join("source");
    std::fs::write(&local, &bytes).unwrap();
    let runtime = ResumeRuntime::default();
    let options = options().with_max_retries(0);
    server.faults.drop_upload.store(90_000, Ordering::Release);
    let error = runtime
        .upload(
            &service,
            &local,
            "/file",
            true,
            &SftpTransferControl::new(),
            &options,
            true,
            None,
            |_, _| {},
        )
        .err()
        .unwrap();
    let temporary = match error.downcast_ref::<FtpError>().unwrap() {
        FtpError::RecoveryRequired { temporary_path, .. } => temporary_path.clone(),
        error => panic!("expected retained upload: {error:?}"),
    };
    assert_eq!(server.read("/file").unwrap(), b"original");
    assert_eq!(server.read(&temporary).unwrap().len(), 90_000);
    // Another task with identical paths owns a new temporary and starts at zero.
    ResumeRuntime::default()
        .upload(
            &service,
            &local,
            "/file",
            true,
            &SftpTransferControl::new(),
            &options,
            true,
            None,
            |_, _| {},
        )
        .unwrap();
    assert!(!server.commands().iter().any(|c| c.starts_with("REST ")));
    assert!(server.read(&temporary).is_some());
    runtime
        .upload(
            &service,
            &local,
            "/file",
            true,
            &SftpTransferControl::new(),
            &options,
            true,
            None,
            |_, _| {},
        )
        .unwrap();
    assert_eq!(server.read("/file").unwrap(), bytes);
    assert!(server.temporary_paths().is_empty());
    assert!(server.commands().iter().any(|c| c == "REST 90000"));
    service.disconnect();
}
