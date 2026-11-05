use super::filename_tests::service;
use crate::sftp::{RemoteFilePath, SftpTransferControl};
use crate::sftp_transfer_types::{SftpPathTransferOptions, SftpTransferOptions};
use russh_sftp::protocol::FileAttributes;
use std::fs;

fn original(mode: u32) -> FileAttributes {
    FileAttributes {
        uid: Some(1234),
        gid: Some(4321),
        permissions: Some(0o100000 | mode),
        ..FileAttributes::empty()
    }
}

#[test]
fn overwrite_preserves_ownership_and_full_mode_despite_configured_default() -> anyhow::Result<()> {
    for mode in [0o755, 0o640, 0o6755, 0o7644] {
        for raw_path in [false, true] {
            let (service, files) = service("UTF-8");
            let path = b"/file".to_vec();
            {
                let mut files = files.lock().unwrap();
                files
                    .contents
                    .insert(path.clone(), b"old contents".to_vec());
                files.attributes.insert(path.clone(), original(mode));
                files.clear_special_on_write = true;
            }
            let root = tempfile::tempdir()?;
            let local = root.path().join("source");
            fs::write(&local, b"new")?;
            let options = SftpTransferOptions::default().with_default_file_permissions("644");
            if raw_path {
                service.upload_remote_file_with_progress_and_control_options(
                    &local,
                    &RemoteFilePath::from_raw("/file", &path),
                    SftpTransferControl::new(),
                    options,
                    |_| {},
                )?;
            } else {
                service.upload_file_with_progress_and_control_options(
                    &local,
                    "/file",
                    SftpTransferControl::new(),
                    options,
                    |_| {},
                )?;
            }
            let files = files.lock().unwrap();
            assert_eq!(files.contents[&path], b"new");
            let attrs = &files.attributes[&path];
            assert_eq!(
                (attrs.uid, attrs.gid, attrs.permissions),
                (Some(1234), Some(4321), Some(0o100000 | mode))
            );
            assert!(
                files
                    .attribute_updates
                    .iter()
                    .all(|attrs| attrs.uid.is_none() && attrs.gid.is_none())
            );
            if mode & 0o6000 == 0 {
                assert!(files.attribute_updates.is_empty());
            }
        }
    }
    Ok(())
}

#[test]
fn new_upload_uses_configured_mode_and_empty_configuration_keeps_server_mode() -> anyhow::Result<()>
{
    for configured in ["600", ""] {
        let (service, files) = service("UTF-8");
        let root = tempfile::tempdir()?;
        let local = root.path().join("source");
        fs::write(&local, b"new")?;
        service.upload_file_with_progress_and_control_options(
            &local,
            "/new",
            SftpTransferControl::new(),
            SftpTransferOptions::default().with_default_file_permissions(configured),
            |_| {},
        )?;
        let files = files.lock().unwrap();
        assert_eq!(
            files.attributes[b"/new".as_slice()].permissions,
            Some(if configured.is_empty() {
                0o100644
            } else {
                0o100600
            })
        );
    }
    Ok(())
}

#[test]
fn unreadable_or_missing_original_permissions_refuse_upload_before_truncation() -> anyhow::Result<()>
{
    for failure in ["denied", "unsupported", "missing mode"] {
        let (service, files) = service("UTF-8");
        {
            let mut files = files.lock().unwrap();
            files
                .contents
                .insert(b"/file".to_vec(), b"original".to_vec());
            let mut attrs = original(0o755);
            match failure {
                "denied" => files.stat_error = Some(3),
                "unsupported" => files.stat_error = Some(8),
                "missing mode" => attrs.permissions = None,
                _ => unreachable!(),
            }
            files.attributes.insert(b"/file".to_vec(), attrs);
        }
        let root = tempfile::tempdir()?;
        let local = root.path().join("source");
        fs::write(&local, b"new")?;
        assert!(service.upload_file(&local, "/file").is_err());
        let files = files.lock().unwrap();
        assert_eq!(files.contents[b"/file".as_slice()], b"original");
        assert!(
            !files.request_kinds.contains(&3),
            "must not open or truncate target"
        );
    }
    Ok(())
}

#[test]
fn rejected_or_ignored_permission_restore_and_ownership_changes_are_reported() -> anyhow::Result<()>
{
    for failure in ["denied", "ignored", "owner"] {
        let (service, files) = service("UTF-8");
        {
            let mut files = files.lock().unwrap();
            files.contents.insert(b"/file".to_vec(), b"old".to_vec());
            files.attributes.insert(b"/file".to_vec(), original(0o6755));
            files.clear_special_on_write = true;
            match failure {
                "denied" => files.setstat_error = Some(3),
                "ignored" => files.ignore_chmod = true,
                "owner" => files.change_owner_on_write = true,
                _ => unreachable!(),
            }
        }
        let root = tempfile::tempdir()?;
        let local = root.path().join("source");
        fs::write(&local, b"new")?;
        let error = service.upload_file(&local, "/file").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("contents may already have changed"),
            "{error:#}"
        );
        assert_eq!(files.lock().unwrap().contents[b"/file".as_slice()], b"new");
    }
    Ok(())
}

#[test]
fn retries_preserve_first_metadata_snapshot_for_files_and_directory_children() -> anyhow::Result<()>
{
    for entry_point in ["file", "raw", "path", "directory"] {
        let directory = entry_point == "directory";
        let (service, files) = service("UTF-8");
        let remote = if directory { "/folder/file" } else { "/file" };
        {
            let mut files = files.lock().unwrap();
            files.directories.insert(b"/folder".to_vec());
            files
                .contents
                .insert(remote.as_bytes().to_vec(), b"old".to_vec());
            files
                .attributes
                .insert(remote.as_bytes().to_vec(), original(0o6755));
            files.clear_special_on_write = true;
            files.fail_chmod_once = true;
        }
        let root = tempfile::tempdir()?;
        let source = root.path().join("source");
        if directory {
            fs::create_dir(&source)?;
            fs::write(source.join("file"), b"new")?;
        } else {
            fs::write(&source, b"new")?;
        }
        let options = SftpTransferOptions::default()
            .with_default_file_permissions("644")
            .with_max_retries(1);
        match entry_point {
            "file" => {
                service.upload_file_with_progress_and_control_options(
                    &source,
                    remote,
                    SftpTransferControl::new(),
                    options,
                    |_| {},
                )?;
            }
            "raw" => {
                service.upload_remote_file_with_progress_and_control_options(
                    &source,
                    &RemoteFilePath::from_raw(remote, remote.as_bytes()),
                    SftpTransferControl::new(),
                    options,
                    |_| {},
                )?;
            }
            _ => {
                service.upload_path_with_progress_and_path_options(
                    &source,
                    if directory { "/folder" } else { remote },
                    SftpTransferControl::new(),
                    SftpPathTransferOptions::default().with_transfer_options(options),
                    |_| {},
                )?;
            }
        }
        let files = files.lock().unwrap();
        assert_eq!(files.contents[remote.as_bytes()], b"new");
        assert_eq!(
            files.attributes[remote.as_bytes()].permissions,
            Some(0o106755)
        );
        assert_eq!(files.attribute_updates.len(), 2);
    }
    Ok(())
}

#[test]
fn upload_through_symlink_preserves_referent_mode_and_keeps_link() -> anyhow::Result<()> {
    let (service, files) = service("UTF-8");
    {
        let mut files = files.lock().unwrap();
        files.links.insert(b"/link".to_vec(), b"/target".to_vec());
        files.contents.insert(b"/target".to_vec(), b"old".to_vec());
        files
            .attributes
            .insert(b"/target".to_vec(), original(0o755));
    }
    let root = tempfile::tempdir()?;
    let local = root.path().join("source");
    fs::write(&local, b"new")?;
    service.upload_file_with_progress_and_control_options(
        &local,
        "/link",
        SftpTransferControl::new(),
        SftpTransferOptions::default().with_default_file_permissions("644"),
        |_| {},
    )?;
    let files = files.lock().unwrap();
    assert_eq!(files.contents[b"/target".as_slice()], b"new");
    assert_eq!(
        files.attributes[b"/target".as_slice()].permissions,
        Some(0o100755)
    );
    assert_eq!(files.links[b"/link".as_slice()], b"/target");
    assert!(!files.contents.contains_key(b"/link".as_slice()));
    Ok(())
}

#[test]
fn new_upload_does_not_interpret_denied_or_unsupported_stat_as_missing() -> anyhow::Result<()> {
    for code in [3, 8] {
        let (service, files) = service("UTF-8");
        files.lock().unwrap().stat_error = Some(code);
        let root = tempfile::tempdir()?;
        let local = root.path().join("source");
        fs::write(&local, b"new")?;
        assert!(service.upload_file(&local, "/new").is_err());
        let files = files.lock().unwrap();
        assert!(!files.contents.contains_key(b"/new".as_slice()));
        assert!(!files.request_kinds.contains(&3));
    }
    Ok(())
}

#[test]
fn server_without_owner_ids_can_still_preserve_reported_permissions() -> anyhow::Result<()> {
    let (service, files) = service("UTF-8");
    {
        let mut files = files.lock().unwrap();
        files.contents.insert(b"/file".to_vec(), b"old".to_vec());
        let mut attrs = original(0o755);
        attrs.uid = None;
        attrs.gid = None;
        files.attributes.insert(b"/file".to_vec(), attrs);
    }
    let root = tempfile::tempdir()?;
    let local = root.path().join("source");
    fs::write(&local, b"new")?;
    service.upload_file(&local, "/file")?;
    let files = files.lock().unwrap();
    assert_eq!(files.contents[b"/file".as_slice()], b"new");
    assert_eq!(
        files.attributes[b"/file".as_slice()].permissions,
        Some(0o100755)
    );
    Ok(())
}
