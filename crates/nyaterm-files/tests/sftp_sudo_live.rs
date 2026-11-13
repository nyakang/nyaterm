//! Run only with scripts/testing/sftp-sudo/run.py against its disposable fixture.
use nyaterm_files::remote_file::{RemoteFileService, RemoteTextWriteResult};
use nyaterm_files::sftp::RemoteFilePath;
use nyaterm_files::sftp::sudo::SftpSudoGrant;
use nyaterm_transport::{
    SshHostKey, SshHostKeyDecision, SshHostKeyVerifier, SshSessionConfig, open_ssh_multiplex_handle,
};
use std::sync::Arc;
struct Verify;
impl SshHostKeyVerifier for Verify {
    fn verify(&self, _: &SshHostKey) -> Result<SshHostKeyDecision, String> {
        Ok(SshHostKeyDecision::Accept)
    }
}

#[test]
#[ignore = "requires disposable OpenSSH/sudo fixture; use scripts/testing/sftp-sudo/run.py"]
fn live_sudo_policy_and_editor_metadata() {
    let case = std::env::var("NYATERM_SUDO_CASE").unwrap();
    let password =
        nyaterm_core::SecretString::from(std::env::var("NYATERM_SUDO_PASSWORD").unwrap());
    let mut config = SshSessionConfig {
        host: "127.0.0.1".into(),
        port: std::env::var("NYATERM_SUDO_PORT").unwrap().parse().unwrap(),
        username: "fixture".into(),
        password: Some(password.clone()),
        host_key_verifier: Some(Arc::new(Verify)),
        ..Default::default()
    };
    if case == "internal" {
        config.runtime_mode = nyaterm_core::models::ssh_runtime::SshRuntimeMode::Sftp;
    }
    if case == "limited" {
        config.sftp.compatibility_mode = true;
    }
    let connection = open_ssh_multiplex_handle(config.clone()).unwrap();
    let ordinary = RemoteFileService::with_multiplex(config.clone(), connection.clone()).unwrap();
    // The missing-server fixture deliberately cannot open an ordinary subsystem.
    if case != "missing" {
        assert!(ordinary.list_dir("/fixture/user").is_ok());
    }
    let grant = SftpSudoGrant::new(connection.clone(), (case != "nopass").then_some(password));
    let root = RemoteFileService::with_sudo(config, connection.clone(), grant.lease()).unwrap();
    let validation = root.list_dir("/fixture/root");
    if !matches!(case.as_str(), "password" | "nopass") {
        assert!(validation.is_err(), "{case} must not grant root access");
        grant.close();
        if case != "missing" {
            assert!(ordinary.list_dir("/fixture/user").is_ok());
        }
        connection.disconnect().unwrap();
        return;
    }
    validation.unwrap();
    assert!(ordinary.list_dir("/fixture/root").is_err());
    for file in ["/fixture/root/file", "/fixture/user/file"] {
        let path = RemoteFilePath::new(file);
        let original = root.remote_file_properties(&path).unwrap();
        let document = root.read_text_document_path(&path, 4096).unwrap();
        let saved = root
            .write_text_document_path(&path, "saved with sudo", Some(&document.revision), false)
            .unwrap();
        assert!(!matches!(saved, RemoteTextWriteResult::Conflict));
        let after = root.remote_file_properties(&path).unwrap();
        assert_eq!(
            (original.uid, original.gid, original.permissions),
            (after.uid, after.gid, after.permissions)
        );
        let stale = root
            .write_text_document_path(&path, "must not overwrite", Some(&document.revision), false)
            .unwrap();
        assert!(matches!(stale, RemoteTextWriteResult::Conflict));
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("download");
        root.download_file(file, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"saved with sudo");
    }
    // Directory transfers create a pool: every lane must retain the sudo factory.
    let local = tempfile::tempdir().unwrap();
    let tree = local.path().join("tree");
    std::fs::create_dir_all(tree.join("sub")).unwrap();
    for index in 0..12 {
        std::fs::write(
            tree.join("sub").join(index.to_string()),
            vec![index as u8; 65536],
        )
        .unwrap();
    }
    root.upload_path_with_progress_and_path_options(
        &tree,
        "/fixture/root/tree",
        Default::default(),
        Default::default(),
        |_| {},
    )
    .unwrap();
    let downloaded = local.path().join("downloaded");
    root.download_path_with_progress_and_path_options(
        "/fixture/root/tree",
        &downloaded,
        Default::default(),
        Default::default(),
        |_| {},
    )
    .unwrap();
    for index in 0..12 {
        assert_eq!(
            std::fs::read(downloaded.join("sub").join(index.to_string())).unwrap(),
            vec![index as u8; 65536]
        );
    }
    let root = Arc::new(root);
    let ordinary = Arc::new(ordinary);
    let copy = |target: &str, service: Arc<RemoteFileService>| {
        nyaterm_files::remote_file::FileCopyRequest {
            source: nyaterm_files::remote_file::FileTransferEndpoint::Remote {
                service: root.clone(),
                path: RemoteFilePath::new("/fixture/root/file"),
            },
            destination: nyaterm_files::remote_file::FileTransferEndpoint::Remote {
                service,
                path: RemoteFilePath::new(target),
            },
            control: Default::default(),
            options: Default::default(),
        }
        .execute()
    };
    assert!(copy("/fixture/root/ordinary-target", ordinary.clone()).is_err());
    assert!(copy("/fixture/user/root-to-user", ordinary.clone()).is_ok());
    assert!(copy("/fixture/root/root-target", root.clone()).is_ok());
    let entry = root
        .list_dir("/fixture/root")
        .unwrap()
        .into_iter()
        .find(|entry| entry.name == "file")
        .unwrap();
    let operation = |target: &str, kind, target_service| {
        nyaterm_files::operations::execute_file_operation(
            nyaterm_files::operations::FileOperationRequest::Transfer(
                nyaterm_files::operations::FileTransferRequest {
                    entry: entry.clone(),
                    target_directory: RemoteFilePath::new(target),
                    operation: kind,
                    same_endpoint: true,
                },
            ),
            nyaterm_files::file_browser::FileBrowserService::Remote(root.clone()),
            target_service,
            Default::default(),
            Default::default(),
            |_| {},
        )
    };
    assert!(
        operation(
            "/fixture/public",
            nyaterm_files::operations::FileTransferOperation::Move,
            nyaterm_files::file_browser::FileBrowserService::Remote(ordinary.clone())
        )
        .is_err(),
        "source root identity must not rename into an ordinary target"
    );
    assert!(
        root.file_properties("/fixture/root/file").is_ok(),
        "failed move must keep the source"
    );
    assert!(
        operation(
            "/fixture/readonly",
            nyaterm_files::operations::FileTransferOperation::Copy,
            nyaterm_files::file_browser::FileBrowserService::Remote(root.clone())
        )
        .is_ok(),
        "sudo bypasses the client's mode-bit heuristic"
    );
    let export = nyaterm_files::drag_export::SftpReadSource::new(
        Arc::downgrade(&root),
        nyaterm_files::drag_export::RemoteDragFile {
            display_name: "file".into(),
            size: Some(15),
            remote_path: RemoteFilePath::new("/fixture/root/file"),
            modified_at: None,
        },
        Arc::new(|| Arc::new(|_| {})),
    );
    let mut reader = export.open().unwrap();
    let mut first = [0; 2];
    assert_eq!(reader.read_at(0, &mut first).unwrap(), 2);
    let clone = root.clone();
    grant.close();
    assert!(reader.read_at(2, &mut first).is_err());
    assert!(clone.list_dir("/fixture/root").is_err());
    assert!(ordinary.list_dir("/fixture/user").is_ok());
    connection.disconnect().unwrap();
}
