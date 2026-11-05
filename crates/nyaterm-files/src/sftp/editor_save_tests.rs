use super::{
    OpenSftpConnection, OpenSftpSession, RemoteFilePath, RemoteTextMetadata, RemoteTextRevision,
    RemoteTextWriteResult, SftpService, SftpWriteTextResult,
};
use nyaterm_transport::{SftpSettings, SshSessionConfig};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{
    Attrs, Data, FileAttributes, Handle, OpenFlags, Packet, Status, StatusCode, Version,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub(super) struct MockFiles {
    pub(super) files: HashMap<String, Vec<u8>>,
    attributes: HashMap<String, FileAttributes>,
    temporary_owner: Option<(u32, u32)>,
    deny_chown: bool,
    ignore_chown: bool,
    ignore_chmod: bool,
    ownership_changes: usize,
    rename_overwrites: bool,
    fail_replace: bool,
    fail_restore: bool,
    fail_cleanup: bool,
    bad_replacement_size: bool,
    atomic_replace: bool,
}

struct Peer {
    files: Arc<Mutex<MockFiles>>,
    supports_posix_rename: bool,
}

fn ok(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: String::new(),
        language_tag: "en".into(),
    }
}

fn rename(
    files: &mut MockFiles,
    source: &str,
    target: &str,
    replace: bool,
) -> Result<(), StatusCode> {
    if files.fail_replace && source.contains(".nyaterm-edit-") && target == "/file.txt" {
        return Err(StatusCode::Failure);
    }
    if files.fail_restore && source.contains(".nyaterm-backup-") && target == "/file.txt" {
        return Err(StatusCode::Failure);
    }
    if !replace && !files.rename_overwrites && files.files.contains_key(target) {
        return Err(StatusCode::Failure);
    }
    let content = files.files.remove(source).ok_or(StatusCode::NoSuchFile)?;
    files.files.insert(target.to_string(), content);
    let attrs = files.attributes.remove(source).unwrap();
    files.attributes.insert(target.to_string(), attrs);
    Ok(())
}

impl russh_sftp::server::Handler for Peer {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        let mut version = Version::new();
        if self.supports_posix_rename {
            version
                .extensions
                .insert("posix-rename@openssh.com".into(), "1".into());
        }
        Ok(version)
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        flags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let mut files = self.files.lock().unwrap();
        if flags.contains(OpenFlags::CREATE) {
            files.files.insert(filename.clone(), Vec::new());
            let (uid, gid) = files.temporary_owner.unwrap_or((1000, 1000));
            files.attributes.insert(
                filename.clone(),
                FileAttributes {
                    uid: Some(uid),
                    gid: Some(gid),
                    permissions: Some(0o100600),
                    ..FileAttributes::empty()
                },
            );
        } else if !files.files.contains_key(&filename) {
            return Err(StatusCode::NoSuchFile);
        }
        Ok(Handle {
            id,
            handle: filename,
        })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let mut files = self.files.lock().unwrap();
        let content = files.files.get_mut(&handle).ok_or(StatusCode::NoSuchFile)?;
        let offset = offset as usize;
        content.resize(content.len().max(offset + data.len()), 0);
        content[offset..offset + data.len()].copy_from_slice(&data);
        Ok(ok(id))
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let files = self.files.lock().unwrap();
        let content = files.files.get(&handle).ok_or(StatusCode::NoSuchFile)?;
        let offset = offset as usize;
        if offset >= content.len() {
            return Err(StatusCode::Eof);
        }
        let end = content.len().min(offset + len as usize);
        Ok(Data {
            id,
            data: content[offset..end].to_vec(),
        })
    }

    async fn close(&mut self, id: u32, _handle: String) -> Result<Status, Self::Error> {
        Ok(ok(id))
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let files = self.files.lock().unwrap();
        let content = files.files.get(&path).ok_or(StatusCode::NoSuchFile)?;
        Ok(Attrs {
            id,
            attrs: FileAttributes {
                size: Some(
                    if files.bad_replacement_size
                        && path == "/file.txt"
                        && content == b"new content"
                    {
                        0
                    } else {
                        content.len() as u64
                    },
                ),
                mtime: Some(1),
                ..files.attributes[&path].clone()
            },
        })
    }

    async fn setstat(
        &mut self,
        id: u32,
        path: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let mut files = self.files.lock().unwrap();
        if attrs.uid.is_some() || attrs.gid.is_some() {
            files.ownership_changes += 1;
            if files.deny_chown {
                return Err(StatusCode::PermissionDenied);
            }
            if !files.ignore_chown {
                let current = files
                    .attributes
                    .get_mut(&path)
                    .ok_or(StatusCode::NoSuchFile)?;
                current.uid = attrs.uid;
                current.gid = attrs.gid;
                // POSIX chown can clear setuid/setgid.
                current.permissions = current.permissions.map(|mode| mode & !0o6000);
            }
        }
        if let Some(mode) = attrs.permissions
            && !files.ignore_chmod
        {
            files
                .attributes
                .get_mut(&path)
                .ok_or(StatusCode::NoSuchFile)?
                .permissions = Some(0o100000 | mode);
        }
        Ok(ok(id))
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        rename(&mut self.files.lock().unwrap(), &oldpath, &newpath, false)?;
        Ok(ok(id))
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        let mut files = self.files.lock().unwrap();
        if files.fail_cleanup && filename.contains(".nyaterm-backup-") {
            return Err(StatusCode::Failure);
        }
        files
            .files
            .remove(&filename)
            .ok_or(StatusCode::NoSuchFile)?;
        files.attributes.remove(&filename);
        Ok(ok(id))
    }

    async fn extended(
        &mut self,
        id: u32,
        request: String,
        data: Vec<u8>,
    ) -> Result<Packet, Self::Error> {
        if request != "posix-rename@openssh.com" {
            return Err(StatusCode::OpUnsupported);
        }
        let mut cursor = data.as_slice();
        let mut next_path = || {
            let (length, rest) = cursor.split_at(4);
            let length = u32::from_be_bytes(length.try_into().unwrap()) as usize;
            let (path, rest) = rest.split_at(length);
            cursor = rest;
            String::from_utf8(path.to_vec()).unwrap()
        };
        let source = next_path();
        let target = next_path();
        let mut files = self.files.lock().unwrap();
        files.atomic_replace = true;
        rename(&mut files, &source, &target, true)?;
        Ok(Packet::Status(ok(id)))
    }
}

pub(super) fn seeded_service(supports_posix_rename: bool) -> (SftpService, Arc<Mutex<MockFiles>>) {
    let service = SftpService::new(SshSessionConfig {
        sftp: SftpSettings {
            compatibility_mode: true,
            ..Default::default()
        },
        ..Default::default()
    });
    let files = Arc::new(Mutex::new(MockFiles::default()));
    files
        .lock()
        .unwrap()
        .files
        .insert("/file.txt".into(), b"old".to_vec());
    files.lock().unwrap().attributes.insert(
        "/file.txt".into(),
        FileAttributes {
            uid: Some(1000),
            gid: Some(1000),
            permissions: Some(0o100644),
            ..FileAttributes::empty()
        },
    );
    let peer = Peer {
        files: files.clone(),
        supports_posix_rename,
    };
    let compatibility = service.compatibility.as_ref().unwrap();
    let session = compatibility
        .block_on(async move {
            let (client, server) = tokio::io::duplex(65536);
            tokio::spawn(russh_sftp::server::run(server, peer));
            Ok(OpenSftpSession {
                sftp: Arc::new(SftpSession::new(client).await?),
                connection: Arc::new(Mutex::new(Some(OpenSftpConnection::Multiplex))),
                persistent: true,
            })
        })
        .unwrap();
    *compatibility.cache.lock().unwrap() = Some(session);
    (service, files)
}

fn save(service: &SftpService) -> anyhow::Result<RemoteTextWriteResult> {
    let revision = RemoteTextRevision::from_bytes(
        b"old",
        RemoteTextMetadata {
            size: 3,
            modified_at: Some(1),
        },
    );
    service.write_text_document_path(
        &RemoteFilePath {
            display_path: "/file.txt".into(),
            raw_path_token: None,
        },
        "new content",
        Some(revision),
        false,
    )
}

#[test]
fn editor_save_uses_atomic_replacement_when_available() {
    let (service, files) = seeded_service(true);
    assert!(matches!(
        save(&service).unwrap(),
        RemoteTextWriteResult::Saved { .. }
    ));
    let files = files.lock().unwrap();
    assert_eq!(files.files["/file.txt"], b"new content");
    assert!(files.atomic_replace);
    assert_eq!(files.files.len(), 1);
}

#[test]
fn editor_save_uses_backup_on_older_servers() {
    let (service, files) = seeded_service(false);
    assert!(matches!(
        save(&service).unwrap(),
        RemoteTextWriteResult::Saved { .. }
    ));
    let files = files.lock().unwrap();
    assert_eq!(files.files["/file.txt"], b"new content");
    assert!(!files.atomic_replace);
    assert_eq!(files.files.len(), 1);
}

#[test]
fn editor_save_restores_original_when_replacement_fails() {
    let (service, files) = seeded_service(false);
    files.lock().unwrap().fail_replace = true;
    assert!(
        save(&service)
            .unwrap_err()
            .to_string()
            .contains("replace original")
    );
    let files = files.lock().unwrap();
    assert_eq!(files.files["/file.txt"], b"old");
    assert_eq!(files.files.len(), 1);
}

#[test]
fn editor_save_preserves_backup_when_restore_fails() {
    let (service, files) = seeded_service(false);
    {
        let mut files = files.lock().unwrap();
        files.fail_replace = true;
        files.fail_restore = true;
    }
    let error = save(&service).unwrap_err().to_string();
    let files = files.lock().unwrap();
    let backup = files
        .files
        .iter()
        .find(|(path, _)| path.contains(".nyaterm-backup-"))
        .unwrap();
    assert_eq!(backup.1, b"old");
    assert!(error.contains(backup.0));
}

#[test]
fn editor_save_reports_cleanup_warning_after_success() {
    let (service, files) = seeded_service(false);
    files.lock().unwrap().fail_cleanup = true;
    let result = save(&service).unwrap();
    let RemoteTextWriteResult::SavedWithBackup { backup_path, .. } = result else {
        panic!("expected backup warning");
    };
    let files = files.lock().unwrap();
    assert_eq!(files.files["/file.txt"], b"new content");
    assert_eq!(files.files[&backup_path], b"old");
}

#[test]
fn editor_save_restores_original_when_replacement_verification_fails() {
    let (service, files) = seeded_service(false);
    files.lock().unwrap().bad_replacement_size = true;
    assert!(
        save(&service)
            .unwrap_err()
            .to_string()
            .contains("verification")
    );
    let files = files.lock().unwrap();
    assert_eq!(files.files["/file.txt"], b"old");
    assert_eq!(files.files.len(), 1);
}

#[test]
fn editor_save_preserves_ownership_and_special_permissions_for_both_rename_paths() {
    for atomic in [false, true] {
        for (original_owner, temporary_owner) in [
            ((1000, 1000), (1000, 1000)), // ordinary user
            ((0, 0), (0, 0)),             // root's own file
            ((1000, 1001), (0, 0)),       // root editing another user's file
            ((1000, 1001), (1000, 1000)), // directory group differs
        ] {
            let (service, files) = seeded_service(atomic);
            {
                let mut files = files.lock().unwrap();
                files.temporary_owner = Some(temporary_owner);
                let attrs = files.attributes.get_mut("/file.txt").unwrap();
                attrs.uid = Some(original_owner.0);
                attrs.gid = Some(original_owner.1);
                attrs.permissions = Some(0o106755);
            }
            assert!(matches!(
                save(&service).unwrap(),
                RemoteTextWriteResult::Saved { .. }
            ));
            let files = files.lock().unwrap();
            let attrs = &files.attributes["/file.txt"];
            assert_eq!(attrs.uid, Some(original_owner.0));
            assert_eq!(attrs.gid, Some(original_owner.1));
            assert_eq!(attrs.permissions, Some(0o106755));
            assert_eq!(
                files.ownership_changes,
                usize::from(original_owner != temporary_owner)
            );
            assert_eq!(files.files.len(), 1);
        }
    }
}

#[test]
fn editor_save_of_own_file_does_not_require_chown_permission() {
    let (service, files) = seeded_service(true);
    files.lock().unwrap().deny_chown = true;
    assert!(matches!(
        save(&service).unwrap(),
        RemoteTextWriteResult::Saved { .. }
    ));
    assert_eq!(files.lock().unwrap().ownership_changes, 0);
}

#[test]
fn editor_force_save_and_legacy_save_cannot_bypass_ownership_protection() {
    for legacy in [false, true] {
        for denied in [false, true] {
            let (service, files) = seeded_service(false);
            {
                let mut files = files.lock().unwrap();
                files.temporary_owner = Some((0, 0));
                files.deny_chown = denied;
                files.rename_overwrites = legacy;
            }
            let path = RemoteFilePath::new("/file.txt");
            let result = if legacy {
                service
                    .write_text_file_path(&path, "new content", None, None, true)
                    .map(|_| ())
            } else {
                service
                    .write_text_document_path(&path, "new content", None, true)
                    .map(|_| ())
            };
            let files = files.lock().unwrap();
            assert_eq!(files.ownership_changes, 1);
            if denied {
                assert!(result.unwrap_err().to_string().contains("ownership"));
                assert_eq!(files.files["/file.txt"], b"old");
            } else {
                result.unwrap();
                assert_eq!(files.files["/file.txt"], b"new content");
                assert_eq!(files.attributes["/file.txt"].uid, Some(1000));
                assert_eq!(files.attributes["/file.txt"].gid, Some(1000));
            }
            assert_eq!(files.files.len(), 1);
        }
    }
}

#[test]
fn editor_save_leaves_original_untouched_when_attributes_cannot_be_preserved() {
    for atomic in [false, true] {
        for failure in [
            "denied",
            "ignored ownership",
            "ignored mode",
            "missing ownership",
            "missing mode",
        ] {
            let (service, files) = seeded_service(atomic);
            {
                let mut files = files.lock().unwrap();
                files.temporary_owner = Some((0, 0));
                match failure {
                    "denied" => files.deny_chown = true,
                    "ignored ownership" => files.ignore_chown = true,
                    "ignored mode" => files.ignore_chmod = true,
                    "missing ownership" => {
                        // SFTP v3 carries uid/gid together under one flag.
                        let attrs = files.attributes.get_mut("/file.txt").unwrap();
                        attrs.uid = None;
                        attrs.gid = None;
                    }
                    "missing mode" => {
                        files.attributes.get_mut("/file.txt").unwrap().permissions = None
                    }
                    _ => unreachable!(),
                }
            }
            let original = files.lock().unwrap().attributes["/file.txt"].clone();
            assert!(save(&service).is_err(), "{failure}, atomic={atomic}");
            let files = files.lock().unwrap();
            assert_eq!(files.files["/file.txt"], b"old");
            assert_eq!(files.attributes["/file.txt"].uid, original.uid);
            assert_eq!(files.attributes["/file.txt"].gid, original.gid);
            assert_eq!(
                files.attributes["/file.txt"].permissions,
                original.permissions
            );
            assert!(!files.atomic_replace);
            assert_eq!(files.files.len(), 1);
        }
    }
}

#[test]
fn write_text_file_creates_missing_files_and_replaces_existing_ones() {
    for supports_posix_rename in [false, true] {
        let (service, files) = seeded_service(supports_posix_rename);
        let created = service
            .write_text_file_path(
                &RemoteFilePath::new("/new.txt"),
                "created",
                None,
                None,
                false,
            )
            .unwrap();
        assert!(matches!(
            created,
            SftpWriteTextResult::Saved { size: 7, .. }
        ));
        let replaced = service
            .write_text_file_path(
                &RemoteFilePath::new("/file.txt"),
                "new content",
                None,
                None,
                false,
            )
            .unwrap();
        assert!(matches!(
            replaced,
            SftpWriteTextResult::Saved { size: 11, .. }
        ));

        let files = files.lock().unwrap();
        assert_eq!(files.files["/new.txt"], b"created");
        assert_eq!(files.files["/file.txt"], b"new content");
        assert_eq!(files.attributes["/file.txt"].permissions, Some(0o100644));
        assert_eq!(
            files.files.len(),
            2,
            "temporary and backup files are cleaned up"
        );
    }
}

#[test]
fn write_text_file_reports_a_conflict_when_an_expected_file_is_missing() {
    let (service, files) = seeded_service(true);
    let result = service
        .write_text_file_path(
            &RemoteFilePath::new("/new.txt"),
            "created",
            None,
            Some(3),
            false,
        )
        .unwrap();
    assert!(matches!(result, SftpWriteTextResult::Conflict { .. }));
    assert!(!files.lock().unwrap().files.contains_key("/new.txt"));
}
