//! Blocking FTP path transfers.
//!
//! Transfers are byte-oriented and directory-aware. Every remote write goes to
//! a temporary sibling first so a cancelled or failed upload never leaves a
//! half-written destination; the temporary is renamed into place afterwards.

use std::collections::HashSet;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::ftp::{FtpEntryKind, FtpError, FtpFileService};
use crate::remote_file::{FileCopySummary, FileTransferEndpoint};
use crate::sftp::SftpTransferControl;
use crate::sftp_transfer_types::{
    FileTransferPhase, SftpDuplicateDecision, SftpDuplicatePolicy, SftpDuplicateRequest,
    SftpPathTransferOptions, SftpTransferDirection, SftpTransferProgress,
};

pub(crate) fn execute<F>(
    source: &FileTransferEndpoint,
    destination: &FileTransferEndpoint,
    control: &SftpTransferControl,
    options: &SftpPathTransferOptions,
    report: F,
) -> anyhow::Result<FileCopySummary>
where
    F: FnMut(SftpTransferProgress),
{
    control.check_cancelled()?;
    let (total_bytes, total_items) = transfer_totals(source, destination);
    let mut copier = Copier {
        control,
        options,
        report,
        bytes: 0,
        items: 0,
        total_bytes,
        total_items,
    };
    let summary = match (source, destination) {
        (
            FileTransferEndpoint::Ftp {
                service: source,
                path: source_path,
            },
            FileTransferEndpoint::Local(target),
        ) => copier.download(source, &source_path.display_path, target)?,
        (
            FileTransferEndpoint::Local(source),
            FileTransferEndpoint::Ftp {
                service: destination,
                path: destination_path,
            },
        ) => copier.upload(source, destination, &destination_path.display_path)?,
        (
            FileTransferEndpoint::Ftp {
                service: source,
                path: source_path,
            },
            FileTransferEndpoint::Ftp {
                service: destination,
                path: destination_path,
            },
        ) => copier.remote_to_remote(
            source,
            &source_path.display_path,
            destination,
            &destination_path.display_path,
        )?,
        _ => anyhow::bail!("FTP transfers only support local and FTP endpoints"),
    };
    Ok(summary)
}

struct Copier<'a, F>
where
    F: FnMut(SftpTransferProgress),
{
    control: &'a SftpTransferControl,
    options: &'a SftpPathTransferOptions,
    report: F,
    bytes: u64,
    items: u64,
    total_bytes: Option<u64>,
    total_items: Option<u64>,
}

impl<F> Copier<'_, F>
where
    F: FnMut(SftpTransferProgress),
{
    fn download(
        &mut self,
        service: &Arc<FtpFileService>,
        remote_path: &str,
        target: &Path,
    ) -> anyhow::Result<FileCopySummary> {
        let entry = service.stat(remote_path)?;
        let is_directory = entry.is_directory();
        let Some((target, copied)) = self.resolve_local_target(
            remote_path,
            target,
            is_directory,
            SftpTransferDirection::Download,
        )?
        else {
            return Ok(FileCopySummary {
                bytes: 0,
                used_local_staging: false,
                destination_path: target.to_string_lossy().into_owned(),
                copied: false,
            });
        };
        if copied {
            self.download_into(service, remote_path, &target, is_directory)?;
        }
        Ok(self.summary(target.to_string_lossy().into_owned(), copied))
    }

    fn upload(
        &mut self,
        source: &Path,
        service: &Arc<FtpFileService>,
        destination_path: &str,
    ) -> anyhow::Result<FileCopySummary> {
        let metadata = fs::metadata(source).map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let is_directory = metadata.is_dir();
        let Some((destination_path, copied)) = self.resolve_remote_target(
            service,
            &source.to_string_lossy(),
            destination_path,
            is_directory,
            SftpTransferDirection::Upload,
        )?
        else {
            return Ok(FileCopySummary {
                bytes: 0,
                used_local_staging: false,
                destination_path: destination_path.to_string(),
                copied: false,
            });
        };
        if copied {
            self.upload_into(source, service, &destination_path)?;
        }
        Ok(self.summary(destination_path, copied))
    }

    fn remote_to_remote(
        &mut self,
        source: &Arc<FtpFileService>,
        source_path: &str,
        destination: &Arc<FtpFileService>,
        destination_path: &str,
    ) -> anyhow::Result<FileCopySummary> {
        let entry = source.stat(source_path)?;
        let is_directory = entry.is_directory();
        let Some((destination_path, copied)) = self.resolve_remote_target(
            destination,
            source_path,
            destination_path,
            is_directory,
            SftpTransferDirection::Copy,
        )?
        else {
            return Ok(FileCopySummary {
                bytes: 0,
                used_local_staging: false,
                destination_path: destination_path.to_string(),
                copied: false,
            });
        };
        if copied {
            self.remote_to_remote_into(source, source_path, destination, &destination_path)?;
        }
        Ok(self.summary(destination_path, copied))
    }

    fn download_into(
        &mut self,
        service: &Arc<FtpFileService>,
        remote_path: &str,
        target: &Path,
        is_directory: bool,
    ) -> anyhow::Result<()> {
        self.control.check_cancelled()?;
        if is_directory {
            fs::create_dir_all(target)?;
            for entry in service.list_dir(remote_path)? {
                if matches!(entry.name.as_str(), "." | "..") {
                    continue;
                }
                let child_target = target.join(&entry.name);
                let child_remote = crate::ftp::join_remote_path(remote_path, &entry.name);
                let child_is_directory = entry.is_directory();
                // Every recursion level applies the same duplicate policy as
                // the top-level entry.
                let Some((resolved, copied)) = self.resolve_local_target(
                    &child_remote,
                    &child_target,
                    child_is_directory,
                    SftpTransferDirection::Download,
                )?
                else {
                    continue;
                };
                if copied {
                    self.download_into(service, &child_remote, &resolved, child_is_directory)?;
                }
            }
            return Ok(());
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp = local_partial_path(target);
        let control = self.control;
        let base = self.bytes;
        let items = self.items;
        let total_bytes = self.total_bytes;
        let total_items = self.total_items;
        let remote_label = remote_path.to_string();
        let local_label = target.to_path_buf();
        let mut transferred = 0u64;
        let outcome = {
            let report = &mut self.report;
            let mut file = fs::File::create(&temp)?;
            let result = service
                .retr_to_writer(remote_path, &mut file, |read| {
                    control
                        .wait_if_paused_blocking()
                        .map_err(|_| FtpError::Cancelled)?;
                    transferred = transferred.saturating_add(read as u64);
                    report(SftpTransferProgress {
                        phase: Some(FileTransferPhase::Transferring),
                        remote_path: remote_label.clone(),
                        local_path: local_label.clone(),
                        bytes_transferred: base.saturating_add(transferred),
                        total_bytes,
                        item_count_completed: Some(items),
                        item_count_total: total_items,
                    });
                    Ok(())
                })
                .map_err(map_transfer_error);
            result.and_then(|_| {
                file.sync_all()?;
                Ok(())
            })
        };
        if let Err(error) = outcome {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
        replace_local_file(&temp, target)?;
        self.record_file(remote_path, target, transferred)?;
        Ok(())
    }

    fn upload_into(
        &mut self,
        source: &Path,
        service: &Arc<FtpFileService>,
        destination_path: &str,
    ) -> anyhow::Result<()> {
        self.control.check_cancelled()?;
        let metadata = fs::metadata(source).map_err(|error| anyhow::anyhow!(error.to_string()))?;
        if metadata.is_dir() {
            service.create_dir_all(destination_path)?;
            for entry in fs::read_dir(source)? {
                let entry = entry?;
                let child_path = entry.path();
                let child_is_directory = entry.file_type()?.is_dir();
                let child_destination = crate::ftp::join_remote_path(
                    destination_path,
                    &entry.file_name().to_string_lossy(),
                );
                // Every recursion level applies the same duplicate policy as
                // the top-level entry.
                let Some((resolved, copied)) = self.resolve_remote_target(
                    service,
                    &child_path.to_string_lossy(),
                    &child_destination,
                    child_is_directory,
                    SftpTransferDirection::Upload,
                )?
                else {
                    continue;
                };
                if copied {
                    self.upload_into(&child_path, service, &resolved)?;
                }
            }
            return Ok(());
        }
        ensure_remote_parent(service, destination_path)?;
        let control = self.control;
        let base = self.bytes;
        let items = self.items;
        let total_bytes = self.total_bytes;
        let total_items = self.total_items;
        let remote_label = destination_path.to_string();
        let local_label = source.to_path_buf();
        let mut transferred = 0u64;
        let outcome = {
            let report = &mut self.report;
            let mut file = fs::File::open(source)?;
            write_remote_from_reader(service, destination_path, &mut file, |read| {
                control
                    .wait_if_paused_blocking()
                    .map_err(|_| FtpError::Cancelled)?;
                transferred = transferred.saturating_add(read as u64);
                report(SftpTransferProgress {
                    phase: Some(FileTransferPhase::Transferring),
                    remote_path: remote_label.clone(),
                    local_path: local_label.clone(),
                    bytes_transferred: base.saturating_add(transferred),
                    total_bytes,
                    item_count_completed: Some(items),
                    item_count_total: total_items,
                });
                Ok(())
            })
        };
        let bytes = outcome?;
        self.record_file(destination_path, source, bytes)?;
        Ok(())
    }

    fn remote_to_remote_into(
        &mut self,
        source: &Arc<FtpFileService>,
        source_path: &str,
        destination: &Arc<FtpFileService>,
        destination_path: &str,
    ) -> anyhow::Result<()> {
        self.control.check_cancelled()?;
        let entry = source.stat(source_path)?;
        if entry.is_directory() {
            destination.create_dir_all(destination_path)?;
            for child in source.list_dir(source_path)? {
                if matches!(child.name.as_str(), "." | "..") {
                    continue;
                }
                let child_source = crate::ftp::join_remote_path(source_path, &child.name);
                let child_destination = crate::ftp::join_remote_path(destination_path, &child.name);
                let child_is_directory = child.is_directory();
                // Every recursion level applies the same duplicate policy as
                // the top-level entry.
                let Some((resolved, copied)) = self.resolve_remote_target(
                    destination,
                    &child_source,
                    &child_destination,
                    child_is_directory,
                    SftpTransferDirection::Copy,
                )?
                else {
                    continue;
                };
                if copied {
                    self.remote_to_remote_into(source, &child_source, destination, &resolved)?;
                }
            }
            return Ok(());
        }
        let control = self.control;
        let base = self.bytes;
        let items = self.items;
        let total_bytes = self.total_bytes;
        let total_items = self.total_items;
        let remote_label = source_path.to_string();
        let mut transferred = 0u64;
        let mut buffer = Vec::new();
        {
            let report = &mut self.report;
            source
                .retr_to_writer(source_path, &mut buffer, |read| {
                    control
                        .wait_if_paused_blocking()
                        .map_err(|_| FtpError::Cancelled)?;
                    transferred = transferred.saturating_add(read as u64);
                    report(SftpTransferProgress {
                        phase: Some(FileTransferPhase::Transferring),
                        remote_path: remote_label.clone(),
                        local_path: PathBuf::new(),
                        bytes_transferred: base.saturating_add(transferred),
                        total_bytes,
                        item_count_completed: Some(items),
                        item_count_total: total_items,
                    });
                    Ok(())
                })
                .map_err(map_transfer_error)?;
        }
        ensure_remote_parent(destination, destination_path)?;
        let bytes = write_remote_from_reader(
            destination,
            destination_path,
            &mut Cursor::new(buffer),
            |_| {
                control
                    .wait_if_paused_blocking()
                    .map_err(|_| FtpError::Cancelled)
            },
        )?;
        self.record_file(destination_path, Path::new(""), bytes)?;
        Ok(())
    }

    fn record_file(
        &mut self,
        remote_path: &str,
        local_path: &Path,
        bytes: u64,
    ) -> anyhow::Result<()> {
        self.control.check_cancelled()?;
        self.control.wait_if_paused_blocking()?;
        self.bytes = self.bytes.saturating_add(bytes);
        self.items = self.items.saturating_add(1);
        (self.report)(SftpTransferProgress {
            phase: Some(FileTransferPhase::Transferring),
            remote_path: remote_path.to_string(),
            local_path: local_path.to_path_buf(),
            bytes_transferred: self.bytes,
            total_bytes: self.total_bytes,
            item_count_completed: Some(self.items),
            item_count_total: self.total_items,
        });
        Ok(())
    }

    fn summary(&mut self, destination_path: String, copied: bool) -> FileCopySummary {
        FileCopySummary {
            bytes: self.bytes,
            used_local_staging: false,
            destination_path,
            copied,
        }
    }

    fn resolve_local_target(
        &mut self,
        source_path: &str,
        target: &Path,
        is_directory: bool,
        direction: SftpTransferDirection,
    ) -> anyhow::Result<Option<(PathBuf, bool)>> {
        let exists = fs::symlink_metadata(target).is_ok();
        if !exists {
            return Ok(Some((target.to_path_buf(), true)));
        }
        let decision = self.decide(
            direction,
            source_path,
            &target.to_string_lossy(),
            is_directory,
        )?;
        match decision {
            SftpDuplicateDecision::Skip => Ok(None),
            SftpDuplicateDecision::Overwrite => Ok(Some((target.to_path_buf(), true))),
            SftpDuplicateDecision::Rename => {
                let mut index = 1;
                loop {
                    let candidate = local_conflict_candidate(target, index);
                    if fs::symlink_metadata(&candidate).is_err() {
                        return Ok(Some((candidate, true)));
                    }
                    index += 1;
                    anyhow::ensure!(index < 10_000, "unable to find a free file name");
                }
            }
        }
    }

    fn resolve_remote_target(
        &mut self,
        service: &Arc<FtpFileService>,
        source_label: &str,
        target: &str,
        is_directory: bool,
        direction: SftpTransferDirection,
    ) -> anyhow::Result<Option<(String, bool)>> {
        if service.stat(target).is_err() {
            return Ok(Some((target.to_string(), true)));
        }
        let decision = self.decide(direction, source_label, target, is_directory)?;
        match decision {
            SftpDuplicateDecision::Skip => Ok(None),
            SftpDuplicateDecision::Overwrite => Ok(Some((target.to_string(), true))),
            SftpDuplicateDecision::Rename => {
                let mut index = 1;
                loop {
                    let candidate = remote_conflict_candidate(target, index);
                    if service.stat(&candidate).is_err() {
                        return Ok(Some((candidate, true)));
                    }
                    index += 1;
                    anyhow::ensure!(index < 10_000, "unable to find a free file name");
                }
            }
        }
    }

    fn decide(
        &self,
        direction: SftpTransferDirection,
        source_path: &str,
        target_path: &str,
        is_directory: bool,
    ) -> anyhow::Result<SftpDuplicateDecision> {
        match self.options.duplicate_policy() {
            SftpDuplicatePolicy::Skip => Ok(SftpDuplicateDecision::Skip),
            SftpDuplicatePolicy::Rename => Ok(SftpDuplicateDecision::Rename),
            SftpDuplicatePolicy::Overwrite => Ok(SftpDuplicateDecision::Overwrite),
            SftpDuplicatePolicy::Ask => {
                let Some(resolver) = self.options.duplicate_resolver() else {
                    return Ok(SftpDuplicateDecision::Overwrite);
                };
                resolver
                    .resolve_duplicate(&SftpDuplicateRequest {
                        direction,
                        source_path: source_path.to_string(),
                        target_path: target_path.to_string(),
                        is_directory,
                    })
                    .map_err(anyhow::Error::msg)
            }
        }
    }
}

/// FTP conflict checks happen on the remote server; callers pass a target that
/// is created on demand. Only the top-level target is probed here.
fn ensure_remote_parent(service: &FtpFileService, destination_path: &str) -> anyhow::Result<()> {
    let (parent, _) = crate::ftp::parent_and_name(destination_path);
    if parent.is_empty() || parent == "/" {
        return Ok(());
    }
    service
        .create_dir_all(&parent)
        .map_err(|error| anyhow::anyhow!(error.to_string()))
}

fn write_remote_from_reader<R: Read + ?Sized>(
    service: &FtpFileService,
    destination_path: &str,
    reader: &mut R,
    on_chunk: impl FnMut(usize) -> Result<(), FtpError>,
) -> anyhow::Result<u64> {
    let temp = format!("{destination_path}.nyaterm-part-{}", nyaterm_core::uuid());
    let result = (|| {
        let bytes = service
            .stor_from_reader(&temp, reader, on_chunk)
            .map_err(map_transfer_error)?;
        replace_remote_file(service, &temp, destination_path)
            .map_err(|error| anyhow::anyhow!(error))?;
        Ok(bytes)
    })();
    if result.is_err() {
        let _ = service.remove(&temp);
    }
    result
}

fn map_transfer_error(error: FtpError) -> anyhow::Error {
    match error {
        FtpError::Cancelled => anyhow::anyhow!(crate::sftp::SFTP_TRANSFER_CANCELLED),
        other => anyhow::anyhow!(other),
    }
}

/// Best-effort source size used for percentage progress. A scan failure leaves
/// the totals unknown instead of failing the transfer.
fn transfer_totals(
    source: &FileTransferEndpoint,
    destination: &FileTransferEndpoint,
) -> (Option<u64>, Option<u64>) {
    let totals = match (source, destination) {
        (FileTransferEndpoint::Ftp { service, path }, _) => {
            remote_totals(service, &path.display_path).ok()
        }
        (FileTransferEndpoint::Local(path), FileTransferEndpoint::Ftp { .. }) => {
            local_totals(path).ok()
        }
        _ => None,
    };
    totals
        .map(|(bytes, items)| (Some(bytes), Some(items)))
        .unwrap_or((None, None))
}

fn remote_totals(service: &FtpFileService, path: &str) -> Result<(u64, u64), FtpError> {
    let mut visited = HashSet::new();
    remote_totals_inner(service, path, &mut visited)
}

fn remote_totals_inner(
    service: &FtpFileService,
    path: &str,
    visited: &mut HashSet<String>,
) -> Result<(u64, u64), FtpError> {
    let entry = service.stat(path)?;
    if !entry.is_directory() {
        return Ok((entry.size, 1));
    }
    if !visited.insert(path.to_string()) {
        return Ok((0, 0));
    }
    let mut bytes = 0u64;
    let mut items = 0u64;
    for child in service.list_dir(path)? {
        if matches!(child.name.as_str(), "." | "..") {
            continue;
        }
        let (child_bytes, child_items) = remote_totals_inner(service, &child.path, visited)?;
        bytes = bytes.saturating_add(child_bytes);
        items = items.saturating_add(child_items);
    }
    Ok((bytes, items))
}

fn local_totals(path: &Path) -> std::io::Result<(u64, u64)> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Ok((metadata.len(), 1));
    }
    let mut bytes = 0u64;
    let mut items = 0u64;
    for entry in fs::read_dir(path)? {
        let (child_bytes, child_items) = local_totals(&entry?.path())?;
        bytes = bytes.saturating_add(child_bytes);
        items = items.saturating_add(child_items);
    }
    Ok((bytes, items))
}

fn replace_remote_file(
    service: &FtpFileService,
    temp: &str,
    destination_path: &str,
) -> Result<(), crate::ftp::FtpError> {
    match service.rename(temp, destination_path) {
        Ok(()) => Ok(()),
        Err(first_error) => match service.stat(destination_path) {
            Ok(entry) if entry.kind != FtpEntryKind::Directory => {
                service.remove(destination_path)?;
                service.rename(temp, destination_path)
            }
            _ => Err(first_error),
        },
    }
}

fn replace_local_file(temp: &Path, destination: &Path) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        if fs::symlink_metadata(destination).is_ok() {
            fs::remove_file(destination)?;
        }
    }
    fs::rename(temp, destination)?;
    Ok(())
}

fn local_partial_path(destination: &Path) -> PathBuf {
    let name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".to_string());
    destination.with_file_name(format!(".{name}.nyaterm-part"))
}

fn local_conflict_candidate(path: &Path, index: usize) -> PathBuf {
    path.with_file_name(format!(
        "{} ({index}){}",
        path.file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("file"),
        path.extension()
            .and_then(|value| value.to_str())
            .map_or(String::new(), |value| format!(".{value}"))
    ))
}

fn remote_conflict_candidate(remote_path: &str, index: usize) -> String {
    let (parent, name) = crate::ftp::parent_and_name(remote_path);
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem.to_string(), format!(".{extension}")),
        _ => (name, String::new()),
    };
    crate::ftp::join_remote_path(&parent, &format!("{stem}({index}){extension}"))
}
