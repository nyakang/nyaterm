use crate::adb::{AdbFilePolicyError, AdbFileService, kind, raw_path};
use crate::download_path::{
    DownloadTemporary, FileIdentity, root_for_target, writable_file_permissions,
};
use crate::file_browser::FileTransferEntryError;
use crate::sftp::{RemoteFilePath, SftpFileType, SftpTransferControl};
use crate::sftp_transfer_types::{FileTransferPhase, SftpTransferOptions};
use nyaterm_android::adb::shell::{execute, execute_with_cancel, quote};
use nyaterm_android::adb::sync::{FileStat, SyncSendOptions};
use nyaterm_android::error::{AdbError, AdbResult};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) enum AdbTransferEvent {
    Data(u64),
    Phase(FileTransferPhase),
}

pub(super) struct StagedUpload<'a> {
    service: &'a AdbFileService,
    temporary: RemoteFilePath,
    target: Option<FileStat>,
    pub(super) bytes: u64,
    committed: bool,
}

pub(super) fn same_target(before: Option<&FileStat>, after: Option<&FileStat>) -> bool {
    match (before, after) {
        (None, None) => true,
        (Some(before), Some(after)) => {
            before.identity == after.identity
                && before.mode == after.mode
                && before.uid == after.uid
                && before.gid == after.gid
                && before.size == after.size
                && before.modified == after.modified
        }
        _ => false,
    }
}

impl StagedUpload<'_> {
    pub(super) fn commit(
        mut self,
        target: &RemoteFilePath,
        replace: bool,
        control: &SftpTransferControl,
    ) -> anyhow::Result<u64> {
        self.service.wait_for_transfer(control)?;
        let current = self
            .service
            .replacement_metadata_with_control(target, control)?;
        anyhow::ensure!(
            same_target(self.target.as_ref(), current.as_ref()),
            FileTransferEntryError::SourceChanged
        );
        self.service.wait_for_transfer(control)?;
        self.service.command_with_control(
            if replace { b"mv -T --" } else { b"mv -nT --" },
            &[&self.temporary, target],
            b"",
            control,
        )?;
        if !replace {
            anyhow::ensure!(
                matches!(self.service.stat_with_control(&self.temporary, control), Err(error) if error.downcast_ref::<AdbError>().is_some_and(|e| matches!(e, AdbError::FileMissing))),
                FileTransferEntryError::TargetExists
            );
        }
        self.committed = true;
        Ok(self.bytes)
    }
}
impl Drop for StagedUpload<'_> {
    fn drop(&mut self) {
        if !self.committed {
            self.service.cleanup_temp(&self.temporary);
        }
    }
}

impl AdbFileService {
    pub(crate) fn check_transfer(&self, control: &SftpTransferControl) -> AdbResult<()> {
        loop {
            self.runtime.check()?;
            self.control
                .check_cancelled()
                .and_then(|()| control.check_cancelled())
                .map_err(|_| AdbError::Cancelled)?;
            if !control.is_paused() {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    pub(crate) fn wait_for_transfer(&self, control: &SftpTransferControl) -> anyhow::Result<()> {
        Ok(self.check_transfer(control)?)
    }
    fn retry_uncommitted<T>(
        &self,
        control: &SftpTransferControl,
        options: &SftpTransferOptions,
        mut run: impl FnMut() -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        for attempt in 0..=options.max_retries() {
            self.wait_for_transfer(control)?;
            match run() {
                Ok(value) => return Ok(value),
                Err(error) => {
                    let retryable = error.chain().any(|cause| {
                        cause
                            .downcast_ref::<AdbError>()
                            .is_some_and(|error| match error {
                                AdbError::Timeout => true,
                                AdbError::Io(error) => matches!(
                                    error.kind(),
                                    std::io::ErrorKind::TimedOut
                                        | std::io::ErrorKind::WouldBlock
                                        | std::io::ErrorKind::ConnectionReset
                                        | std::io::ErrorKind::ConnectionAborted
                                        | std::io::ErrorKind::BrokenPipe
                                        | std::io::ErrorKind::UnexpectedEof
                                ),
                                AdbError::Protocol("sync EOF" | "sync write EOF") => true,
                                _ => false,
                            })
                    });
                    if !retryable || attempt == options.max_retries() {
                        return Err(error);
                    }
                    for _ in 0..(attempt + 1).min(10) {
                        self.wait_for_transfer(control)?;
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                }
            }
        }
        unreachable!("bounded retry loop returns")
    }
    fn transfer_progress(
        &self,
        control: &SftpTransferControl,
        bytes: u64,
        callback: &mut impl FnMut(u64),
    ) -> AdbResult<()> {
        self.check_transfer(control)?;
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
        self.wait_for_transfer(control)?;
        let raw = raw_path(path)?;
        Ok(self.runtime.with_sync(
            || self.check_transfer(control),
            |stream| {
                stream.receive_controlled(
                    &raw,
                    writer,
                    || self.check_transfer(control),
                    |bytes| self.transfer_progress(control, bytes.0, &mut progress),
                )
            },
        )?)
    }
    pub(super) fn replacement_metadata(
        &self,
        path: &RemoteFilePath,
    ) -> anyhow::Result<Option<FileStat>> {
        self.replacement_metadata_with_control(path, &self.control)
    }
    fn replacement_metadata_with_control(
        &self,
        path: &RemoteFilePath,
        control: &SftpTransferControl,
    ) -> anyhow::Result<Option<FileStat>> {
        let mut stat = match self.stat_with_control(path, control) {
            Ok(stat) => stat,
            Err(error)
                if error
                    .downcast_ref::<AdbError>()
                    .is_some_and(|e| matches!(e, AdbError::FileMissing)) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        anyhow::ensure!(
            kind(stat.mode) == SftpFileType::File,
            FileTransferEntryError::UnsupportedFileType
        );
        if stat.uid.is_none() || stat.gid.is_none() {
            let mut command = b"stat -c '%u %g %a %d %i' -- ".to_vec();
            command.extend(quote(&raw_path(path)?)?);
            let output = execute_with_cancel(&self.binding, &command, 128, || {
                self.check_transfer(control)
            })
            .map_err(|error| AdbFilePolicyError::AttributesUnavailable.context(error.into()))?;
            let text =
                std::str::from_utf8(&output).map_err(|_| AdbError::Protocol("file attributes"))?;
            let parts: Vec<_> = text.split_whitespace().collect();
            anyhow::ensure!(parts.len() == 5, AdbFilePolicyError::AttributesUnavailable);
            stat.identity = Some((
                parts[3]
                    .parse()
                    .map_err(|_| AdbError::Protocol("file device"))?,
                parts[4]
                    .parse()
                    .map_err(|_| AdbError::Protocol("file inode"))?,
            ));
            stat.uid = Some(
                parts[0]
                    .parse()
                    .map_err(|_| AdbError::Protocol("file owner"))?,
            );
            stat.gid = Some(
                parts[1]
                    .parse()
                    .map_err(|_| AdbError::Protocol("file group"))?,
            );
            stat.mode = (stat.mode & !0o7777)
                | u32::from_str_radix(parts[2], 8)
                    .map_err(|_| AdbError::Protocol("file permissions"))?;
        }
        Ok(Some(stat))
    }
    pub(super) fn stage_upload(
        &self,
        local: &Path,
        target: &RemoteFilePath,
        control: &SftpTransferControl,
        options: &SftpTransferOptions,
        mut progress: impl FnMut(u64),
    ) -> anyhow::Result<StagedUpload<'_>> {
        self.wait_for_transfer(control)?;
        let mut reader = crate::download_path::open_without_following(local, false, false)?;
        let before = reader.metadata()?;
        anyhow::ensure!(
            before.is_file(),
            FileTransferEntryError::UnsupportedFileType
        );
        let identity = FileIdentity::from_file(&reader)?;
        let original = self.replacement_metadata_with_control(target, control)?;
        let temporary = self.child_path(
            &target.parent()?,
            &format!(".nyaterm-{}.part", uuid::Uuid::new_v4()),
        )?;
        self.command_with_control(b"set -C; : >", &[&temporary], b"", control)?;
        let mut staged = StagedUpload {
            service: self,
            temporary,
            target: original,
            bytes: 0,
            committed: false,
        };
        #[cfg(unix)]
        let source_mode = {
            use std::os::unix::fs::MetadataExt as _;
            before.mode() & 0o777
        };
        #[cfg(not(unix))]
        let source_mode = 0o644;
        let mode = staged
            .target
            .as_ref()
            .map_or(options.default_file_mode.unwrap_or(source_mode), |s| {
                s.mode & 0o7777
            });
        let modified = if options.preserve_timestamps {
            before.modified()?
        } else {
            SystemTime::now()
        };
        let modified = modified.duration_since(UNIX_EPOCH)?.as_secs();
        let raw = raw_path(&staged.temporary)?;
        staged.bytes = self.retry_uncommitted(control, options, || {
            reader.seek(SeekFrom::Start(0))?;
            Ok(self.runtime.with_sync(
                || self.check_transfer(control),
                |stream| {
                    stream.send_controlled(
                        &raw,
                        &mut reader,
                        SyncSendOptions {
                            mode,
                            modified,
                            buffer_size: options.buffer_size_bytes(),
                        },
                        || self.check_transfer(control),
                        |bytes| self.transfer_progress(control, bytes.0, &mut progress),
                    )
                },
            )?)
        })?;
        let after = reader.metadata()?;
        anyhow::ensure!(
            staged.bytes == before.len()
                && after.len() == before.len()
                && after.modified()? == before.modified()?
                && FileIdentity::from_path(local)? == identity,
            FileTransferEntryError::SourceChanged
        );
        let mut temporary_stat = self.stat_with_control(&staged.temporary, control)?;
        anyhow::ensure!(
            temporary_stat.size == staged.bytes,
            FileTransferEntryError::SourceChanged
        );
        if let Some(original) = &staged.target {
            if temporary_stat.uid != original.uid || temporary_stat.gid != original.gid {
                let (uid, gid) = original
                    .uid
                    .zip(original.gid)
                    .ok_or(AdbFilePolicyError::AttributesUnavailable)?;
                self.command_with_control(
                    format!("chown {uid}:{gid} --").as_bytes(),
                    &[&staged.temporary],
                    b"",
                    control,
                )
                .map_err(|error| AdbFilePolicyError::PermissionsUnavailable.context(error))?;
            }
            if temporary_stat.mode & 0o7777 != mode
                || temporary_stat.uid != original.uid
                || temporary_stat.gid != original.gid
            {
                self.command_with_control(
                    format!("chmod {mode:o} --").as_bytes(),
                    &[&staged.temporary],
                    b"",
                    control,
                )
                .map_err(|error| AdbFilePolicyError::PermissionsUnavailable.context(error))?;
            }
            temporary_stat = self.stat_with_control(&staged.temporary, control)?;
            anyhow::ensure!(
                temporary_stat.mode & 0o7777 == mode
                    && temporary_stat.uid == original.uid
                    && temporary_stat.gid == original.gid,
                AdbFilePolicyError::PermissionsUnavailable
            );
        }
        let mount_assigned = staged.target.is_none()
            && options.default_file_mode.is_none()
            && self.permission_capability(&target.parent()?, control)?
                == crate::adb::permissions::PermissionCapability::MountAssigned;
        // adbd may broaden SEND permissions (e.g. 0644 -> 0666). Apply the
        // requested mode on the staged inode, then verify it before replacement.
        if !mount_assigned && temporary_stat.mode & 0o7777 != mode {
            self.command_with_control(
                format!("chmod {mode:o} --").as_bytes(),
                &[&staged.temporary],
                b"",
                control,
            )
            .map_err(|error| AdbFilePolicyError::PermissionsUnavailable.context(error))?;
            temporary_stat = self.stat_with_control(&staged.temporary, control)?;
        }
        anyhow::ensure!(
            kind(temporary_stat.mode) == SftpFileType::File
                && (temporary_stat.mode & 0o7777 == mode
                    || (mount_assigned && temporary_stat.mode & 0o7000 == 0)),
            AdbFilePolicyError::PermissionsUnavailable
        );
        if options.preserve_timestamps {
            anyhow::ensure!(
                temporary_stat.modified == modified,
                AdbFilePolicyError::TimestampUnavailable
            );
        }
        Ok(staged)
    }
    pub fn upload(
        &self,
        local: &Path,
        target: &RemoteFilePath,
        replace: bool,
        control: &SftpTransferControl,
        mut progress: impl FnMut(u64),
    ) -> anyhow::Result<u64> {
        self.upload_with_options(
            local,
            target,
            replace,
            control,
            &SftpTransferOptions::default(),
            |event| {
                if let AdbTransferEvent::Data(bytes) = event {
                    progress(bytes);
                }
            },
        )
    }
    pub(crate) fn upload_with_options(
        &self,
        local: &Path,
        target: &RemoteFilePath,
        replace: bool,
        control: &SftpTransferControl,
        options: &SftpTransferOptions,
        mut event: impl FnMut(AdbTransferEvent),
    ) -> anyhow::Result<u64> {
        event(AdbTransferEvent::Phase(FileTransferPhase::Transferring));
        let staged = self.stage_upload(local, target, control, options, |bytes| {
            event(AdbTransferEvent::Data(bytes))
        })?;
        event(AdbTransferEvent::Phase(FileTransferPhase::Committing));
        staged.commit(target, replace, control)
    }
    fn cleanup_temp(&self, temporary: &RemoteFilePath) {
        if self.runtime.check().is_err() {
            return;
        }
        if let Ok(raw) = raw_path(temporary)
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
        mut progress: impl FnMut(u64),
    ) -> anyhow::Result<u64> {
        self.download_with_options(
            path,
            local,
            replace,
            control,
            &SftpTransferOptions::default(),
            |event| {
                if let AdbTransferEvent::Data(bytes) = event {
                    progress(bytes);
                }
            },
        )
    }
    pub(crate) fn download_with_options(
        &self,
        path: &RemoteFilePath,
        local: &Path,
        replace: bool,
        control: &SftpTransferControl,
        options: &SftpTransferOptions,
        mut event: impl FnMut(AdbTransferEvent),
    ) -> anyhow::Result<u64> {
        event(AdbTransferEvent::Phase(FileTransferPhase::Transferring));
        let before = self.stat_with_control(path, control)?;
        anyhow::ensure!(
            kind(before.mode) == SftpFileType::File,
            FileTransferEntryError::UnsupportedFileType
        );
        let root = root_for_target(local);
        writable_file_permissions(root, local)?;
        if !replace {
            anyhow::ensure!(
                !crate::download_path::target_exists(local)?,
                FileTransferEntryError::TargetExists
            );
        }
        let (temporary, mut output) = DownloadTemporary::create(root, local)?;
        let bytes = self.retry_uncommitted(control, options, || {
            let current = self.stat_with_control(path, control)?;
            anyhow::ensure!(
                same_target(Some(&before), Some(&current)),
                FileTransferEntryError::SourceChanged
            );
            output.set_len(0)?;
            output.seek(SeekFrom::Start(0))?;
            self.receive(path, &mut output, control, |bytes| {
                event(AdbTransferEvent::Data(bytes))
            })
        })?;
        event(AdbTransferEvent::Phase(FileTransferPhase::Committing));
        output.sync_all()?;
        let after = self.stat_with_control(path, control)?;
        anyhow::ensure!(
            bytes == before.size
                && after.size == before.size
                && after.modified == before.modified
                && after.mode == before.mode
                && after.identity == before.identity,
            FileTransferEntryError::SourceChanged
        );
        self.wait_for_transfer(control)?;
        let modified = options
            .preserve_timestamps
            .then_some(UNIX_EPOCH + std::time::Duration::from_secs(before.modified));
        if let Some(modified) = modified {
            output
                .set_modified(modified)
                .map_err(|error| AdbFilePolicyError::TimestampUnavailable.context(error.into()))?;
        }
        drop(output);
        temporary.commit_with_modified(root, local, modified)?;
        Ok(bytes)
    }
    pub(crate) fn set_modified(&self, path: &RemoteFilePath, modified: u32) -> anyhow::Result<()> {
        let mut command = format!("touch -m -d '@{modified}' -- ").into_bytes();
        command.extend(quote(&raw_path(path)?)?);
        execute_with_cancel(&self.binding, &command, 128, || {
            self.control
                .check_cancelled()
                .map_err(|_| AdbError::Cancelled)
        })
        .map_err(|error| AdbFilePolicyError::TimestampUnavailable.context(error.into()))?;
        anyhow::ensure!(
            self.stat(path)?.modified == u64::from(modified),
            AdbFilePolicyError::TimestampUnavailable
        );
        Ok(())
    }
}
