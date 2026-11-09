use super::workspace::{ExternalEditTestCleanup, prepare_external_edit_workspace};
use nyaterm_files::remote_file::{RemoteFileBackendKind, RemoteFileService};
use nyaterm_files::sftp::{RemoteFilePath, SftpAttributeUpdate, SftpTransferControl};
use nyaterm_files::sftp_transfer_types::SftpTransferOptions;
use nyaterm_transport::{SshHostKey, SshHostKeyDecision, SshHostKeyVerifier, SshSessionConfig};
use std::sync::Arc;
use std::{env, fs};

struct AcceptDisposableServerKey;

impl SshHostKeyVerifier for AcceptDisposableServerKey {
    fn verify(&self, _host_key: &SshHostKey) -> Result<SshHostKeyDecision, String> {
        Ok(SshHostKeyDecision::Accept)
    }
}

fn required_env(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("{name} must be set for the shell editor E2E test"))
}

#[test]
#[ignore = "requires NYATERM_TEST_SHELL_HOST/PORT/USERNAME/ROOT and a disposable SSH server without SFTP"]
fn shell_external_edit_downloads_into_private_workspace_and_preserves_remote_metadata()
-> anyhow::Result<()> {
    let service = RemoteFileService::new(SshSessionConfig {
        host: required_env("NYATERM_TEST_SHELL_HOST"),
        port: required_env("NYATERM_TEST_SHELL_PORT").parse()?,
        username: required_env("NYATERM_TEST_SHELL_USERNAME"),
        password: env::var("NYATERM_TEST_SHELL_PASSWORD").ok().map(Into::into),
        allow_none_auth: true,
        host_key_verifier: Some(Arc::new(AcceptDisposableServerKey)),
        ..SshSessionConfig::default()
    });
    let remote_dir = format!(
        "{}/nyaterm-editor-e2e-{}",
        required_env("NYATERM_TEST_SHELL_ROOT").trim_end_matches('/'),
        uuid::Uuid::new_v4()
    );
    service.create_dir_path(&remote_dir, Some(0o700))?;
    let backend = service.selected_backend().expect("probed backend");
    let remote_path = RemoteFilePath::new(format!("{remote_dir}/file ' [x].txt"));
    let mut local_root = None;

    let result = (|| -> anyhow::Result<()> {
        let workspace = prepare_external_edit_workspace("file.txt", || false)?;
        let local_path = &workspace.local_path;
        local_root = Some(ExternalEditTestCleanup::new(&workspace));
        anyhow::ensure!(
            backend != RemoteFileBackendKind::Sftp,
            "Server must disable SFTP"
        );
        if let Ok(expected) = env::var("NYATERM_TEST_SHELL_BACKEND") {
            anyhow::ensure!(backend.cache_name() == expected, "Unexpected shell backend");
        }
        service.create_file_path(&remote_path.display_path, Some(0o640))?;
        // When run against a root server, set these to another UID/GID to
        // reproduce #885's root replacing a file owned by a different user.
        service.update_path_attributes(
            &remote_path.display_path,
            SftpAttributeUpdate {
                owner: env::var("NYATERM_TEST_SHELL_OWNER").ok(),
                group: env::var("NYATERM_TEST_SHELL_GROUP").ok(),
                ..SftpAttributeUpdate::default()
            },
        )?;
        service.update_path_attributes(
            &remote_path.display_path,
            SftpAttributeUpdate {
                mode: Some(0o6755),
                ..SftpAttributeUpdate::default()
            },
        )?;
        let before = service.file_properties(&remote_path.display_path)?;
        // Cover the built-in save as well as the external edit replacement.
        service.write_text_file(&remote_path.display_path, "original", None, None, true)?;
        service.download_remote_file_with_progress_and_control_options(
            &remote_path,
            local_path,
            SftpTransferControl::default(),
            SftpTransferOptions::default(),
            |_| {},
        )?;
        anyhow::ensure!(
            fs::read(local_path)? == b"original",
            "Initial download differs"
        );
        let edited = b"external edit\0binary bytes";
        fs::write(local_path, edited)?;
        let summary = service.upload_remote_edit_with_progress_and_control_options(
            local_path,
            &remote_path,
            SftpTransferControl::default(),
            SftpTransferOptions::default(),
            |_| {},
        )?;
        anyhow::ensure!(summary.bytes == edited.len() as u64, "Upload size differs");
        let after = service.file_properties(&remote_path.display_path)?;
        anyhow::ensure!(after.owner == before.owner, "Owner changed after edit");
        anyhow::ensure!(after.group == before.group, "Group changed after edit");
        anyhow::ensure!(
            after.permissions == before.permissions,
            "Permissions changed after edit"
        );
        service.download_remote_file_with_progress_and_control_options(
            &remote_path,
            local_path,
            SftpTransferControl::default(),
            SftpTransferOptions::default(),
            |_| {},
        )?;
        anyhow::ensure!(
            fs::read(local_path)? == edited,
            "Round-trip download differs"
        );
        anyhow::ensure!(
            service
                .list_dir(&remote_dir)?
                .iter()
                .all(|entry| !entry.name.contains(".nyaterm-edit-")),
            "Remote temporary file was not removed"
        );
        Ok(())
    })();
    let cleanup = service.delete_path(&remote_dir);
    drop(local_root);
    result?;
    cleanup?;
    Ok(())
}
