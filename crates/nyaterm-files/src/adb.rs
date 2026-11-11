//! Android file operations own their channels independently of terminal views.
use crate::remote_file::{
    RemoteTextDocument, RemoteTextMetadata, RemoteTextRevision, RemoteTextWriteResult,
};
use crate::sftp::{
    RemoteBinaryFile, RemoteFilePath, SftpFileEntry, SftpFileProperties, SftpFileType,
    SftpRemoteTextFile, SftpTransferControl,
};
use nyaterm_android::adb::client::DeviceBinding;
use nyaterm_android::adb::shell::{execute, execute_with_cancel, quote};
use nyaterm_android::adb::sync::{FileStat, SyncStream};
use nyaterm_android::error::{AdbError, AdbResult};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
pub(crate) mod copy;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod transfer_tests;

/// Marks failures from an Android file operation without exposing device data.
#[derive(Debug, thiserror::Error)]
#[error("Android file operation failed")]
pub struct AdbFileError;

/// Local validation failures carry no paths or device-controlled output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AdbFileValidationError {
    #[error("Android requires an absolute path without NUL")]
    InvalidPath,
    #[error("Invalid Android raw path token")]
    InvalidPathToken,
    #[error("Invalid Android filename")]
    InvalidFilename,
}

pub(crate) fn file_error(error: anyhow::Error) -> anyhow::Error {
    error.context(AdbFileError)
}

#[derive(Clone, Debug)]
pub struct AdbFileService {
    pub binding: DeviceBinding,
    pub control: SftpTransferControl,
}

pub fn raw_path(path: &RemoteFilePath) -> anyhow::Result<Vec<u8>> {
    let raw = path
        .raw_path()
        .map_err(|_| AdbFileValidationError::InvalidPathToken)?
        .unwrap_or_else(|| path.display_path.as_bytes().to_vec());
    anyhow::ensure!(
        raw.starts_with(b"/") && !raw.contains(&0),
        AdbFileValidationError::InvalidPath
    );
    Ok(raw)
}

fn kind(mode: u32) -> SftpFileType {
    match mode & 0o170000 {
        0o040000 => SftpFileType::Directory,
        0o100000 => SftpFileType::File,
        0o120000 => SftpFileType::Symlink,
        _ => SftpFileType::Other,
    }
}

impl AdbFileService {
    pub fn new(binding: DeviceBinding) -> Self {
        Self {
            binding,
            control: SftpTransferControl::new(),
        }
    }
    pub fn cancel(&self) {
        self.control.cancel();
    }
    pub fn child_path(
        &self,
        parent: &RemoteFilePath,
        name: &str,
    ) -> anyhow::Result<RemoteFilePath> {
        anyhow::ensure!(
            !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\0']),
            AdbFileValidationError::InvalidFilename
        );
        let mut raw = raw_path(parent)?;
        if raw.last() != Some(&b'/') {
            raw.push(b'/');
        }
        raw.extend_from_slice(name.as_bytes());
        Ok(RemoteFilePath::from_raw(
            String::from_utf8_lossy(&raw),
            &raw,
        ))
    }
    fn check(&self) -> anyhow::Result<()> {
        self.control.check_cancelled()
    }
    pub fn stat(&self, path: &RemoteFilePath) -> anyhow::Result<FileStat> {
        self.stat_with_control(path, &self.control)
    }
    pub(crate) fn stat_with_control(
        &self,
        path: &RemoteFilePath,
        control: &SftpTransferControl,
    ) -> anyhow::Result<FileStat> {
        self.check()?;
        control.wait_if_paused_blocking()?;
        Ok(
            SyncStream::open(&self.binding)?.stat_with_cancel(&raw_path(path)?, || {
                self.control
                    .check_cancelled()
                    .and_then(|()| control.check_cancelled())
                    .map_err(|_| AdbError::Cancelled)
            })?,
        )
    }
    pub fn exists(&self, path: &RemoteFilePath) -> anyhow::Result<bool> {
        match self.stat(path) {
            Ok(_) => Ok(true),
            Err(error)
                if matches!(
                    error.downcast_ref::<AdbError>(),
                    Some(AdbError::FileMissing)
                ) =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }
    pub fn list_dir_path(&self, path: &RemoteFilePath) -> anyhow::Result<Vec<SftpFileEntry>> {
        self.list_dir_path_with_control(path, &self.control)
    }
    pub(crate) fn list_dir_path_with_control(
        &self,
        path: &RemoteFilePath,
        control: &SftpTransferControl,
    ) -> anyhow::Result<Vec<SftpFileEntry>> {
        self.check()?;
        control.wait_if_paused_blocking()?;
        let parent = raw_path(path)?;
        SyncStream::open(&self.binding)?
            .list_with_cancel(&parent, || {
                self.control
                    .check_cancelled()
                    .and_then(|()| control.check_cancelled())
                    .map_err(|_| AdbError::Cancelled)
            })?
            .into_iter()
            .map(|entry| {
                anyhow::ensure!(
                    !entry.name.is_empty()
                        && !entry.name.contains(&b'/')
                        && !entry.name.contains(&0),
                    AdbFileValidationError::InvalidFilename
                );
                let mut raw = parent.clone();
                if raw.last() != Some(&b'/') {
                    raw.push(b'/');
                }
                raw.extend_from_slice(&entry.name);
                let path = RemoteFilePath::from_raw(String::from_utf8_lossy(&raw), &raw);
                Ok(SftpFileEntry {
                    name: String::from_utf8_lossy(&entry.name).into_owned(),
                    path: path.display_path,
                    raw_path_token: path.raw_path_token,
                    file_type: kind(entry.stat.mode),
                    size: Some(entry.stat.size),
                    permissions: Some(entry.stat.mode),
                    modified_at: u32::try_from(entry.stat.modified).ok(),
                    owner: String::new(),
                    group: String::new(),
                    symlink_target_is_directory: false,
                })
            })
            .collect()
    }
    pub fn remote_file_properties(
        &self,
        path: &RemoteFilePath,
    ) -> anyhow::Result<SftpFileProperties> {
        self.remote_file_properties_with_control(path, &self.control)
    }
    pub(crate) fn remote_file_properties_with_control(
        &self,
        path: &RemoteFilePath,
        control: &SftpTransferControl,
    ) -> anyhow::Result<SftpFileProperties> {
        let stat = self.stat_with_control(path, control)?;
        Ok(SftpFileProperties {
            name: path
                .display_path
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("/")
                .into(),
            path: path.display_path.clone(),
            file_type: kind(stat.mode),
            size: Some(stat.size),
            permissions: Some(stat.mode),
            permissions_symbolic: format!("{:o}", stat.mode & 0o777),
            owner: stat.uid.map(|id| id.to_string()).unwrap_or_default(),
            group: stat.gid.map(|id| id.to_string()).unwrap_or_default(),
            uid: stat.uid,
            gid: stat.gid,
            modified_at: u32::try_from(stat.modified).ok(),
            accessed_at: u32::try_from(stat.accessed).ok(),
            raw_path_token: path.raw_path_token.clone(),
            symlink_target: None,
            symlink_target_is_directory: false,
        })
    }
    fn command(
        &self,
        prefix: &[u8],
        paths: &[&RemoteFilePath],
        suffix: &[u8],
    ) -> anyhow::Result<()> {
        self.check()?;
        let mut command = prefix.to_vec();
        for path in paths {
            command.push(b' ');
            let raw = raw_path(path)?;
            anyhow::ensure!(
                std::str::from_utf8(&raw).is_ok(),
                "writing an unrepresentable Android filename is unavailable"
            );
            command.extend_from_slice(&quote(&raw)?);
        }
        command.extend_from_slice(suffix);
        execute_with_cancel(&self.binding, &command, 4096, || {
            self.control
                .check_cancelled()
                .map_err(|_| AdbError::Cancelled)
        })?;
        self.check()?;
        Ok(())
    }
    pub(crate) fn validate_copy_destination(
        &self,
        source: &RemoteFilePath,
        target: &Self,
        destination: &RemoteFilePath,
        directory: bool,
    ) -> anyhow::Result<()> {
        if self.binding.server != target.binding.server
            || self.binding.serial != target.binding.serial
        {
            return Ok(());
        }
        let source_raw = normalized_path(&raw_path(source)?)?;
        let destination_raw = normalized_path(&raw_path(destination)?)?;
        anyhow::ensure!(
            source_raw != destination_raw
                && !(directory
                    && destination_raw.starts_with(&[source_raw.as_slice(), b"/"].concat())),
            "source and destination overlap"
        );
        // /sdcard is itself an Android alias. Compare resolved paths on device,
        // while rejecting user-controlled symlink ancestors below those roots.
        let canonical = |service: &Self, path: &RemoteFilePath| -> anyhow::Result<Vec<u8>> {
            let mut command = b"realpath -m -- ".to_vec();
            command.extend(quote(&raw_path(path)?)?);
            let mut bytes = execute(&service.binding, &command, 4096)?;
            if bytes.last() == Some(&b'\n') {
                bytes.pop();
            }
            normalized_path(&bytes)
        };
        let src = canonical(self, source)?;
        let dst = canonical(target, destination)?;
        anyhow::ensure!(
            src != dst && !(directory && dst.starts_with(&[src.as_slice(), b"/"].concat())),
            "source and destination overlap"
        );
        if directory {
            for (service, path) in [(self, source), (target, destination)] {
                let mut current = path.parent()?;
                loop {
                    if let Ok(stat) = service.stat(&current) {
                        let raw = normalized_path(&raw_path(&current)?)?;
                        anyhow::ensure!(
                            kind(stat.mode) != SftpFileType::Symlink
                                || matches!(
                                    raw.as_slice(),
                                    b"/sdcard"
                                        | b"/storage/self/primary"
                                        | b"/storage/emulated/legacy"
                                ),
                            "copying through symbolic link ancestors is unavailable"
                        );
                    } else if service.exists(&current)? {
                        anyhow::bail!("cannot validate Android copy ancestor");
                    }
                    let parent = current.parent()?;
                    if parent == current {
                        break;
                    }
                    current = parent;
                }
            }
        }
        Ok(())
    }
    pub fn create_dir_remote_path(
        &self,
        path: &RemoteFilePath,
        _mode: Option<u32>,
    ) -> anyhow::Result<()> {
        self.command(b"mkdir --", &[path], b"")
    }
    pub fn create_file_remote_path(
        &self,
        path: &RemoteFilePath,
        _mode: Option<u32>,
    ) -> anyhow::Result<()> {
        self.command(b"set -C; : >", &[path], b"")
    }
    pub fn delete_remote_path(&self, path: &RemoteFilePath) -> anyhow::Result<()> {
        let raw = raw_path(path)?;
        let components: Vec<&[u8]> = raw
            .split(|b| *b == b'/')
            .filter(|p| !p.is_empty() && *p != b".")
            .collect();
        anyhow::ensure!(
            !components.is_empty() && !components.contains(&b"..".as_slice()),
            "refusing root or ambiguous Android deletion"
        );
        self.command(b"rm -rf --", &[path], b"")
    }
    pub fn rename_remote_paths(
        &self,
        from: &RemoteFilePath,
        to: &RemoteFilePath,
    ) -> anyhow::Result<()> {
        self.command(b"mv -T --", &[from, to], b"")
    }
    pub fn rename_remote_paths_no_replace(
        &self,
        from: &RemoteFilePath,
        to: &RemoteFilePath,
    ) -> anyhow::Result<()> {
        self.command(b"mv -nT --", &[from, to], b"")?;
        anyhow::ensure!(!self.exists(from)?, "Android destination already exists");
        Ok(())
    }
    fn progress(
        &self,
        control: &SftpTransferControl,
        bytes: u64,
        callback: &mut impl FnMut(u64),
    ) -> AdbResult<()> {
        self.control
            .check_cancelled()
            .map_err(|_| AdbError::Cancelled)?;
        control
            .wait_if_paused_blocking()
            .map_err(|_| AdbError::Cancelled)?;
        if bytes != 0 {
            callback(bytes);
        }
        Ok(())
    }
    pub fn receive(
        &self,
        path: &RemoteFilePath,
        writer: &mut impl Write,
        control: &SftpTransferControl,
        mut progress: impl FnMut(u64),
    ) -> anyhow::Result<u64> {
        self.check()?;
        Ok(
            SyncStream::open(&self.binding)?.receive(&raw_path(path)?, writer, |n| {
                self.progress(control, n, &mut progress)
            })?,
        )
    }
    pub fn upload(
        &self,
        local: &Path,
        path: &RemoteFilePath,
        replace: bool,
        control: &SftpTransferControl,
        mut progress: impl FnMut(u64),
    ) -> anyhow::Result<u64> {
        let temp = self.child_path(
            &path.parent()?,
            &format!(".nyaterm-{}.part", uuid::Uuid::new_v4()),
        )?;
        self.create_file_remote_path(&temp, None)?;
        let result = (|| {
            let mut reader = std::fs::File::open(local)?;
            let expected = reader.metadata()?.len();
            let modified = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
            let bytes = SyncStream::open(&self.binding)?.send(
                &raw_path(&temp)?,
                &mut reader,
                0o644,
                modified,
                |n| self.progress(control, n, &mut progress),
            )?;
            anyhow::ensure!(
                bytes == expected && self.stat(&temp)?.size == expected,
                "Android upload length changed"
            );
            let mut local_hash = HashWriter::default();
            std::io::copy(&mut std::fs::File::open(local)?, &mut local_hash)?;
            let mut remote_hash = HashWriter::default();
            self.receive(&temp, &mut remote_hash, control, |_| {})?;
            anyhow::ensure!(
                local_hash.digest() == remote_hash.digest(),
                "Android upload verification failed"
            );
            control.check_cancelled()?;
            self.check()?;
            if replace {
                self.rename_remote_paths(&temp, path)?;
            } else {
                self.rename_remote_paths_no_replace(&temp, path)?;
            }
            Ok(bytes)
        })();
        if result.is_err() {
            self.cleanup_temp(&temp);
        }
        result
    }
    fn cleanup_temp(&self, temp: &RemoteFilePath) {
        // Cleanup must work after cancellation, and can only name our unique file.
        if let Ok(raw) = raw_path(temp)
            && let Ok(quoted) = quote(&raw)
        {
            let mut command = b"rm -f -- ".to_vec();
            command.extend(quoted);
            let _ = execute(&self.binding, &command, 4096);
        }
    }
    pub fn download(
        &self,
        path: &RemoteFilePath,
        local: &Path,
        replace: bool,
        control: &SftpTransferControl,
        progress: impl FnMut(u64),
    ) -> anyhow::Result<u64> {
        let before = self.stat_with_control(path, control)?;
        anyhow::ensure!(
            kind(before.mode) == SftpFileType::File,
            "Android download requires a regular file"
        );
        let parent = local
            .parent()
            .ok_or_else(|| anyhow::anyhow!("download parent is missing"))?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        let bytes = self.receive(path, temp.as_file_mut(), control, progress)?;
        temp.as_file_mut().sync_all()?;
        let after = self.stat_with_control(path, control)?;
        anyhow::ensure!(
            bytes == before.size && before.size == after.size && before.modified == after.modified,
            crate::file_browser::FileTransferEntryError::SourceChanged
        );
        // mtime has only second precision on older Sync implementations. Verify
        // bytes as well so a same-size edit cannot commit a mixed download.
        let mut local_hash = HashWriter::default();
        std::io::copy(&mut std::fs::File::open(temp.path())?, &mut local_hash)?;
        let mut remote_hash = HashWriter::default();
        self.receive(path, &mut remote_hash, control, |_| {})?;
        anyhow::ensure!(
            local_hash.digest() == remote_hash.digest(),
            crate::file_browser::FileTransferEntryError::SourceChanged
        );
        control.check_cancelled()?;
        self.check()?;
        if replace {
            temp.persist(local).map_err(|e| e.error)?;
        } else {
            temp.persist_noclobber(local).map_err(|e| e.error)?;
        }
        Ok(bytes)
    }
    pub fn read_file_bytes_path(
        &self,
        path: &RemoteFilePath,
        limit: u64,
    ) -> anyhow::Result<RemoteBinaryFile> {
        let stat = self.stat(path)?;
        anyhow::ensure!(stat.size <= limit, "Android preview exceeds limit");
        let mut output = LimitedWriter {
            bytes: Vec::new(),
            limit,
        };
        self.receive(path, &mut output, &SftpTransferControl::new(), |_| {})?;
        Ok(RemoteBinaryFile {
            path: path.display_path.clone(),
            content_bytes: output.bytes,
            size: stat.size,
            modified_at: stat.modified,
        })
    }
    pub fn read_text_file_path(
        &self,
        path: &RemoteFilePath,
        limit: u64,
    ) -> anyhow::Result<SftpRemoteTextFile> {
        let file = self.read_file_bytes_path(path, limit)?;
        Ok(SftpRemoteTextFile {
            path: file.path,
            content: String::from_utf8(file.content_bytes)?,
            size: file.size,
            modified_at: file.modified_at,
        })
    }
    pub fn read_text_document_path(
        &self,
        path: &RemoteFilePath,
        limit: u64,
    ) -> anyhow::Result<RemoteTextDocument> {
        let file = self.read_text_file_path(path, limit)?;
        let revision = RemoteTextRevision::from_bytes(
            file.content.as_bytes(),
            RemoteTextMetadata {
                size: file.size,
                modified_at: Some(file.modified_at),
            },
        );
        Ok(RemoteTextDocument {
            path: file.path,
            content: file.content,
            revision,
        })
    }
    pub fn write_text_document_path(
        &self,
        path: &RemoteFilePath,
        content: impl AsRef<str>,
        expected: Option<&RemoteTextRevision>,
        force: bool,
    ) -> anyhow::Result<RemoteTextWriteResult> {
        if !force
            && let Some(expected) = expected
            && self
                .read_text_document_path(path, 16 * 1024 * 1024)?
                .revision
                != *expected
        {
            return Ok(RemoteTextWriteResult::Conflict);
        }
        let mut local = tempfile::NamedTempFile::new()?;
        local.write_all(content.as_ref().as_bytes())?;
        self.upload(
            local.path(),
            path,
            true,
            &SftpTransferControl::new(),
            |_| {},
        )?;
        Ok(RemoteTextWriteResult::Saved {
            revision: self
                .read_text_document_path(path, 16 * 1024 * 1024)?
                .revision,
        })
    }
}

#[derive(Default)]
struct HashWriter(Sha256);
impl HashWriter {
    fn digest(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}
impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
struct LimitedWriter {
    bytes: Vec<u8>,
    limit: u64,
}
impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if (self.bytes.len() as u64).saturating_add(bytes.len() as u64) > self.limit {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn normalized_path(raw: &[u8]) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(
        raw.starts_with(b"/") && !raw.contains(&0),
        AdbFileValidationError::InvalidPath
    );
    let mut components = Vec::new();
    for component in raw.split(|b| *b == b'/') {
        match component {
            b"" | b"." => {}
            b".." => {
                components.pop();
            }
            value => components.push(value),
        }
    }
    let mut result = vec![b'/'];
    for (i, component) in components.iter().enumerate() {
        if i != 0 {
            result.push(b'/');
        }
        result.extend_from_slice(component);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use crate::adb::{normalized_path, raw_path};
    use crate::sftp::RemoteFilePath;
    #[test]
    fn raw_android_paths_preserve_special_and_unrepresentable_bytes() {
        let raw = b"/sdcard/space '\n$(id)\xff";
        let path = RemoteFilePath::from_raw(String::from_utf8_lossy(raw), raw);
        assert_eq!(raw_path(&path).unwrap(), raw);
        assert_eq!(normalized_path(b"/a//b/.././c").unwrap(), b"/a/c");
        assert!(raw_path(&RemoteFilePath::new("relative")).is_err());
    }
    #[test]
    fn invalid_android_paths_tokens_and_names_have_typed_redacted_errors() {
        use crate::adb::{AdbFileService, AdbFileValidationError};
        for path in ["relative", ".", r"C:\Users\fixture", "/sdcard/\0secret"] {
            let error = raw_path(&RemoteFilePath::new(path)).unwrap_err();
            assert_eq!(
                error.downcast_ref::<AdbFileValidationError>(),
                Some(&AdbFileValidationError::InvalidPath)
            );
            assert!(!error.to_string().contains("fixture"));
        }
        let error = raw_path(&RemoteFilePath {
            display_path: "/sdcard/".into(),
            raw_path_token: Some("invalid-token".into()),
        })
        .unwrap_err();
        assert_eq!(
            error.downcast_ref::<AdbFileValidationError>(),
            Some(&AdbFileValidationError::InvalidPathToken)
        );
        let service = AdbFileService::new(nyaterm_android::adb::client::DeviceBinding {
            server: "127.0.0.1:1".parse().unwrap(),
            serial: "synthetic".into(),
            transport_id: None,
            generation: 1,
        });
        for name in ["", ".", "..", "a/b", "a\0b"] {
            let error = service
                .child_path(&RemoteFilePath::new("/sdcard/"), name)
                .unwrap_err();
            assert_eq!(
                error.downcast_ref::<AdbFileValidationError>(),
                Some(&AdbFileValidationError::InvalidFilename)
            );
        }
    }
}
