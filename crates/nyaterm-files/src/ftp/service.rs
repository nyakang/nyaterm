use std::collections::HashSet;
use std::fmt;
use std::io::{Cursor, Read, Write};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, SystemTime};

use nyaterm_core::character_encoding::CharacterEncoding;
use sha2::{Digest, Sha256};
use suppaftp::{NativeTlsFtpStream, Status};

use crate::ftp::config::{FtpConfig, FtpLogin};
use crate::ftp::connection::Lease;
use crate::ftp::encoding::system_filename_encoding;
use crate::ftp::error::FtpError;
use crate::ftp::io::SessionIo;
use crate::ftp::list::{
    FtpEntry, FtpEntryKind, entries_from_lines, entry_from_mlst_line, parent_and_name,
};
use crate::ftp::replace::{self, Replacement};
use crate::ftp::text::{LocalDecoder, NvtEncoder};
use crate::remote_file::{RemoteTextMetadata, RemoteTextRevision, RemoteTextWriteResult};
use crate::sftp::SftpTransferControl;

const CHUNK_BYTES: usize = 32 * 1024;
const MAX_LIST_BYTES: usize = 32 * 1024 * 1024;
const MAX_READ_BYTES: u64 = 32 * 1024 * 1024;

/// A confirmed write may leave an old backup when the server rejects cleanup.
#[derive(Default)]
pub struct FtpWriteOutcome {
    pub bytes: u64,
    pub backup_path: Option<String>,
}

/// Blocking FTP adapter. No plaintext credentials are retained for reconnects.
pub struct FtpFileService {
    pub(super) config: FtpConfig,
    pub(super) encoding: CharacterEncoding,
    pub(super) home: String,
    pub(super) peer: SocketAddr,
    pub(super) username: String,
    pub(super) io: Arc<SessionIo>,
    pub(super) stream: Mutex<NativeTlsFtpStream>,
    pub(super) mutations: Mutex<()>,
}

impl fmt::Debug for FtpFileService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FtpFileService")
            .field("encoding", &self.encoding)
            .field("available", &self.is_available())
            .finish_non_exhaustive()
    }
}

impl Drop for FtpFileService {
    fn drop(&mut self) {
        self.io.close();
    }
}

impl FtpFileService {
    pub fn connect(config: FtpConfig, login: FtpLogin) -> Result<Self, FtpError> {
        Self::connect_with_local_encoding(config, login, system_filename_encoding())
    }
    pub fn connect_with_local_encoding(
        config: FtpConfig,
        login: FtpLogin,
        encoding: CharacterEncoding,
    ) -> Result<Self, FtpError> {
        Self::connect_inner(config, login, encoding, || false)
    }
    pub fn connect_with_cancel(
        config: FtpConfig,
        login: FtpLogin,
        cancelled: impl Fn() -> bool + Send + 'static,
    ) -> Result<Self, FtpError> {
        Self::connect_inner(config, login, system_filename_encoding(), cancelled)
    }
    pub fn encoding(&self) -> CharacterEncoding {
        self.encoding
    }
    pub fn home_dir(&self) -> &str {
        &self.home
    }
    pub fn is_available(&self) -> bool {
        self.io.valid()
    }
    /// Closing is independent of the control mutex, including blocked workers.
    pub fn disconnect(&self) {
        self.io.close();
    }
    pub(crate) fn ascii_mode(&self) -> bool {
        self.config.ascii_mode
    }
    pub(crate) fn same_endpoint(&self, other: &Self) -> bool {
        self.peer == other.peer && self.username == other.username
    }
    pub(crate) fn ensure_available(&self) -> Result<(), FtpError> {
        if self.is_available() {
            Ok(())
        } else {
            Err(FtpError::Unavailable)
        }
    }
    pub(super) fn lock_mutations(
        &self,
        control: &SftpTransferControl,
    ) -> Result<MutexGuard<'_, ()>, FtpError> {
        loop {
            self.ensure_available()?;
            if control.is_cancelled() {
                return Err(FtpError::Cancelled);
            }
            match self.mutations.try_lock() {
                Ok(guard) => return Ok(guard),
                Err(TryLockError::Poisoned(_)) => return Err(FtpError::Unavailable),
                Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(25)),
            }
        }
    }
    pub(crate) fn list_dir_with_control(
        &self,
        path: &str,
        control: &SftpTransferControl,
    ) -> Result<Vec<FtpEntry>, FtpError> {
        let path = if path.is_empty() { &self.home } else { path };
        validate_path(path)?;
        let mut lease = self.lock_stream(control)?;
        let listing = list(&mut lease, path, self.encoding, true);
        match listing {
            Ok(entries) => Ok(entries),
            Err(FtpError::Protocol(_)) => list(&mut lease, path, self.encoding, false),
            Err(error) => Err(error),
        }
    }
    pub fn list_dir(&self, path: &str) -> Result<Vec<FtpEntry>, FtpError> {
        self.list_dir_with_control(path, &SftpTransferControl::new())
    }
    pub(crate) fn completion_target_is_directory(
        &self,
        path: &str,
        control: &SftpTransferControl,
        deadline: std::time::Instant,
    ) -> Result<bool, FtpError> {
        validate_path(path)?;
        let path = format!("{}/", path.trim_end_matches('/'));
        let mut lease = self.lock_stream_until(control, Some(deadline))?;
        // Never change the shared connection's CWD just to inspect a link.
        let line = lease.command(|stream| stream.mlst(Some(&path)))?;
        Ok(entry_from_mlst_line(&line, &path).is_some_and(|entry| entry.is_directory()))
    }

    pub(crate) fn list_completion_directory(
        &self,
        path: &crate::sftp::RemoteFilePath,
        limits: crate::file_browser::completion::CompletionLimits,
        control: &SftpTransferControl,
        deadline: std::time::Instant,
    ) -> anyhow::Result<crate::file_browser::completion::CompletionListing> {
        use crate::file_browser::completion::CompletionListing;
        validate_path(&path.display_path)?;
        let mut lease = self.lock_stream_until(control, Some(deadline))?;
        let mut result =
            completion_list(&mut lease, &path.display_path, self.encoding, true, limits);
        if matches!(result, Err(FtpError::Protocol(_))) {
            result = completion_list(&mut lease, &path.display_path, self.encoding, false, limits);
        }
        let (entries, complete) = result?;
        Ok(CompletionListing {
            entries: entries
                .iter()
                .map(crate::file_browser::entry_from_ftp_entry)
                .collect(),
            complete,
        })
    }

    pub fn stat(&self, path: &str) -> Result<FtpEntry, FtpError> {
        self.stat_with_control(path, &SftpTransferControl::new())
    }
    pub(crate) fn stat_with_control(
        &self,
        path: &str,
        control: &SftpTransferControl,
    ) -> Result<FtpEntry, FtpError> {
        self.ensure_available()?;
        validate_path(path)?;
        if path == "/" {
            return Ok(FtpEntry {
                name: "/".into(),
                path: "/".into(),
                kind: FtpEntryKind::Directory,
                size: 0,
                modified: None,
            });
        }
        {
            let mut lease = self.lock_stream(control)?;
            match lease.command(|stream| stream.mlst(Some(path))) {
                Ok(line) => {
                    if let Some(entry) = entry_from_mlst_line(&line, path) {
                        return Ok(entry);
                    }
                }
                Err(FtpError::Protocol(_)) => {}
                Err(error) => return Err(error),
            }
        }
        let (parent, name) = parent_and_name(path);
        self.list_dir_with_control(&parent, control)?
            .into_iter()
            .find(|entry| entry.name == name)
            .ok_or(FtpError::NotFound)
    }
    pub fn create_dir(&self, path: &str) -> Result<(), FtpError> {
        self.create_dir_with_control(path, &SftpTransferControl::new())
    }
    pub(crate) fn create_dir_with_control(
        &self,
        path: &str,
        control: &SftpTransferControl,
    ) -> Result<(), FtpError> {
        validate_path(path)?;
        let _mutation = self.lock_mutations(control)?;
        self.lock_stream(control)?
            .command(|stream| stream.mkdir(path))
    }
    pub fn create_dir_all(&self, path: &str) -> Result<(), FtpError> {
        let path = normalized_path(&self.home, path)?;
        let mut current = String::new();
        for part in path.split('/').filter(|part| !part.is_empty()) {
            current.push('/');
            current.push_str(part);
            match self.stat(&current) {
                Ok(entry) if entry.is_directory() => {}
                Ok(_) => return Err(FtpError::InvalidPath),
                Err(FtpError::NotFound) => self.create_dir(&current)?,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
    pub fn create_file(&self, path: &str) -> Result<(), FtpError> {
        if !matches!(self.stat(path), Err(FtpError::NotFound)) {
            return Err(FtpError::InvalidPath);
        }
        self.write_from_reader(
            path,
            false,
            &mut Cursor::new([]),
            &SftpTransferControl::new(),
            |_| Ok(()),
        )
        .map(|_| ())
    }
    pub fn remove(&self, path: &str) -> Result<(), FtpError> {
        let path = normalized_path(&self.home, path)?;
        if path == "/" {
            return Err(FtpError::InvalidPath);
        }
        let control = SftpTransferControl::new();
        let _mutation = self.lock_mutations(&control)?;
        self.remove_unlocked(&path)
    }
    fn remove_unlocked(&self, path: &str) -> Result<(), FtpError> {
        let directory = self.stat(path)?.is_directory();
        self.lock_stream(&SftpTransferControl::new())?
            .command(|stream| {
                if directory {
                    stream.rmdir(path)
                } else {
                    stream.rm(path)
                }
            })
    }
    pub fn remove_tree(&self, path: &str) -> Result<(), FtpError> {
        let path = normalized_path(&self.home, path)?;
        if path == "/" {
            return Err(FtpError::InvalidPath);
        }
        self.remove_tree_entry(&path, 0, &mut HashSet::new())
    }
    fn remove_tree_entry(
        &self,
        path: &str,
        depth: usize,
        visited: &mut HashSet<String>,
    ) -> Result<(), FtpError> {
        if depth > 128 || visited.len() >= 65_536 {
            return Err(FtpError::LimitExceeded);
        }
        if !visited.insert(path.to_owned()) {
            return Err(FtpError::InvalidPath);
        }
        if self.stat(path)?.is_directory() {
            for child in self.list_dir(path)? {
                self.remove_tree_entry(&child.path, depth + 1, visited)?;
            }
        }
        self.remove(path)
    }
    pub fn rename(&self, from: &str, to: &str) -> Result<(), FtpError> {
        let from = normalized_path(&self.home, from)?;
        let to = normalized_path(&self.home, to)?;
        if from == "/" || to == "/" {
            return Err(FtpError::InvalidPath);
        }
        let _mutation = self.lock_mutations(&SftpTransferControl::new())?;
        self.rename_unlocked(&from, &to)
    }
    fn rename_unlocked(&self, from: &str, to: &str) -> Result<(), FtpError> {
        validate_path(from)?;
        validate_path(to)?;
        self.lock_stream(&SftpTransferControl::new())?
            .command(|stream| stream.rename(from, to))
    }
    /// Resolve directories with CWD/PWD without leaving the shared CWD changed.
    pub(crate) fn canonical_target(
        &self,
        path: &str,
        directory: bool,
        control: &SftpTransferControl,
    ) -> Result<String, FtpError> {
        let path = normalized_path(&self.home, path)?;
        let (parent, name) = parent_and_name(&path);
        let directory_path = if directory { &path } else { &parent };
        let mut lease = self.lock_stream(control)?;
        let original = lease.command(|stream| stream.pwd())?;
        let resolved = (|| {
            lease.command(|stream| stream.cwd(directory_path))?;
            lease.command(|stream| stream.pwd())
        })();
        if lease.command(|stream| stream.cwd(&original)).is_err() {
            self.disconnect();
            return Err(if control.is_cancelled() {
                FtpError::Cancelled
            } else {
                FtpError::Unavailable
            });
        }
        let resolved = resolved?;
        if directory {
            normalized_path("/", &resolved)
        } else {
            normalized_path(&resolved, &name)
        }
    }
    pub fn read_file_bytes(&self, path: &str) -> Result<Vec<u8>, FtpError> {
        self.read_file_bytes_limited(path, MAX_READ_BYTES)
    }
    pub fn read_file_bytes_limited(&self, path: &str, limit: u64) -> Result<Vec<u8>, FtpError> {
        let mut bytes = Vec::new();
        self.retrieve(
            path,
            &mut bytes,
            Some(limit),
            &SftpTransferControl::new(),
            |_| Ok(()),
        )?;
        Ok(bytes)
    }
    pub fn write_file(&self, path: &str, bytes: &[u8]) -> Result<FtpWriteOutcome, FtpError> {
        self.write_from_reader(
            path,
            true,
            &mut Cursor::new(bytes),
            &SftpTransferControl::new(),
            |_| Ok(()),
        )
    }
    pub(crate) fn write_text_document(
        &self,
        path: &str,
        content: &str,
        expected: Option<&RemoteTextRevision>,
        force: bool,
    ) -> Result<RemoteTextWriteResult, FtpError> {
        let control = SftpTransferControl::new();
        let _mutation = self.lock_mutations(&control)?;
        validate_path(path)?;
        if !force && expected.is_none() {
            return Err(FtpError::InvalidPath);
        }
        let matches = || -> Result<bool, FtpError> {
            let Some(expected) = expected else {
                return Ok(true);
            };
            let before = match self.stat(path) {
                Ok(entry) if entry.kind == FtpEntryKind::File => entry,
                Ok(_) | Err(FtpError::NotFound) => return Ok(false),
                Err(error) => return Err(error),
            };
            let mut hash = HashWriter(Sha256::new());
            let size = match self.retrieve(
                path,
                &mut hash,
                Some(expected.metadata.size),
                &control,
                |_| Ok(()),
            ) {
                Ok(size) => size,
                Err(FtpError::LimitExceeded) => return Ok(false),
                Err(error) => return Err(error),
            };
            let after = self.stat(path)?;
            Ok(before == after
                && size == expected.metadata.size
                && <[u8; 32]>::from(hash.0.finalize()) == expected.content_sha256
                && modified_secs(after.modified) == expected.metadata.modified_at)
        };
        if !force && !matches()? {
            return Ok(RemoteTextWriteResult::Conflict);
        }
        let temporary = temporary_path(path);
        let result = (|| {
            self.store(
                &temporary,
                &mut Cursor::new(content.as_bytes()),
                &control,
                |_| Ok(()),
            )?;
            if !force && !matches()? {
                return Ok(RemoteTextWriteResult::Conflict);
            }
            let backup_path = replace::commit(&RemoteReplacement(self), &temporary, path, true)?;
            let revision = RemoteTextRevision::from_bytes(
                content.as_bytes(),
                RemoteTextMetadata {
                    size: content.len() as u64,
                    modified_at: self
                        .stat(path)
                        .ok()
                        .and_then(|entry| modified_secs(entry.modified)),
                },
            );
            Ok(match backup_path {
                Some(backup_path) => RemoteTextWriteResult::SavedWithBackup {
                    revision,
                    backup_path,
                },
                None => RemoteTextWriteResult::Saved { revision },
            })
        })();
        if !matches!(result, Err(FtpError::RecoveryRequired { .. })) {
            let _ = self.remove_unlocked(&temporary);
        }
        result
    }
    pub(crate) fn write_from_reader<R: Read + ?Sized>(
        &self,
        path: &str,
        replace_existing: bool,
        reader: &mut R,
        control: &SftpTransferControl,
        on_chunk: impl FnMut(usize) -> Result<(), FtpError>,
    ) -> Result<FtpWriteOutcome, FtpError> {
        validate_path(path)?;
        let _mutation = self.lock_mutations(control)?;
        let temporary = temporary_path(path);
        let result = (|| {
            let bytes = self.store(&temporary, reader, control, on_chunk)?;
            if control.is_cancelled() {
                return Err(FtpError::Cancelled);
            }
            let backup_path =
                replace::commit(&RemoteReplacement(self), &temporary, path, replace_existing)?;
            Ok(FtpWriteOutcome { bytes, backup_path })
        })();
        if result.is_err() && !matches!(result, Err(FtpError::RecoveryRequired { .. })) {
            let _ = self.remove_unlocked(&temporary);
        }
        result
    }
    pub fn retr_to_writer<W: Write + ?Sized>(
        &self,
        path: &str,
        writer: &mut W,
        on_chunk: impl FnMut(usize) -> Result<(), FtpError>,
    ) -> Result<u64, FtpError> {
        self.retrieve(path, writer, None, &SftpTransferControl::new(), on_chunk)
    }
    pub(crate) fn retrieve<W: Write + ?Sized>(
        &self,
        path: &str,
        writer: &mut W,
        limit: Option<u64>,
        control: &SftpTransferControl,
        mut on_chunk: impl FnMut(usize) -> Result<(), FtpError>,
    ) -> Result<u64, FtpError> {
        validate_path(path)?;
        let mut lease = self.lock_stream(control)?;
        let mut data = lease.command(|stream| stream.retr_as_stream(path))?;
        let mut buffer = vec![0; CHUNK_BYTES];
        let mut converted = Vec::with_capacity(CHUNK_BYTES * 2);
        let mut decoder = self.config.ascii_mode.then(LocalDecoder::default);
        let mut total = 0u64;
        let result = (|| {
            loop {
                while control.is_paused() {
                    lease.watch.check()?;
                    if control.is_cancelled() {
                        return Err(FtpError::Cancelled);
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                if control.is_cancelled() {
                    return Err(FtpError::Cancelled);
                }
                lease.watch.check()?;
                let room = limit.map_or(CHUNK_BYTES, |limit| {
                    limit
                        .saturating_sub(total)
                        .saturating_add(1)
                        .min(CHUNK_BYTES as u64) as usize
                });
                let read = data.read(&mut buffer[..room]).map_err(io_error)?;
                converted.clear();
                let output = if let Some(decoder) = decoder.as_mut() {
                    if read == 0 {
                        decoder.finish(&mut converted);
                    } else {
                        decoder.decode(&buffer[..read], &mut converted);
                    }
                    converted.as_slice()
                } else {
                    &buffer[..read]
                };
                total = total
                    .checked_add(output.len() as u64)
                    .ok_or(FtpError::LimitExceeded)?;
                if limit.is_some_and(|limit| total > limit) {
                    return Err(FtpError::LimitExceeded);
                }
                writer.write_all(output).map_err(io_error)?;
                lease.watch.progress();
                if !output.is_empty() {
                    on_chunk(output.len())?;
                }
                if read == 0 {
                    break;
                }
            }
            Ok(total)
        })();
        lease.complete(data, result)
    }
    fn store<R: Read + ?Sized>(
        &self,
        path: &str,
        reader: &mut R,
        control: &SftpTransferControl,
        mut on_chunk: impl FnMut(usize) -> Result<(), FtpError>,
    ) -> Result<u64, FtpError> {
        let mut lease = self.lock_stream(control)?;
        let mut data = lease.command(|stream| stream.put_with_stream(path))?;
        let mut buffer = vec![0; CHUNK_BYTES];
        let mut converted = Vec::with_capacity(CHUNK_BYTES * 2);
        let mut encoder = self.config.ascii_mode.then(NvtEncoder::default);
        let result = (|| {
            let mut total = 0u64;
            loop {
                while control.is_paused() {
                    lease.watch.check()?;
                    if control.is_cancelled() {
                        return Err(FtpError::Cancelled);
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                if control.is_cancelled() {
                    return Err(FtpError::Cancelled);
                }
                lease.watch.check()?;
                let read = reader.read(&mut buffer).map_err(io_error)?;
                if read == 0 {
                    break;
                }
                converted.clear();
                let output = if let Some(encoder) = encoder.as_mut() {
                    encoder.encode(&buffer[..read], &mut converted);
                    converted.as_slice()
                } else {
                    &buffer[..read]
                };
                data.write_all(output).map_err(io_error)?;
                lease.watch.progress();
                total = total.saturating_add(read as u64);
                on_chunk(read)?;
            }
            Ok(total)
        })();
        lease.complete(data, result)
    }
}

struct RemoteReplacement<'a>(&'a FtpFileService);
impl Replacement for RemoteReplacement<'_> {
    fn stat(&self, path: &str) -> Result<FtpEntry, FtpError> {
        self.0.stat(path)
    }
    fn rename(&self, from: &str, to: &str) -> Result<(), FtpError> {
        self.0.rename_unlocked(from, to)
    }
    fn remove(&self, path: &str) -> Result<(), FtpError> {
        self.0.remove_unlocked(path)
    }
}
struct HashWriter(Sha256);
impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn modified_secs(value: Option<SystemTime>) -> Option<u64> {
    value?
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|value| value.as_secs())
}
fn temporary_path(path: &str) -> String {
    format!("{path}.nyaterm-part-{}", nyaterm_core::uuid())
}
pub(super) fn io_error(error: std::io::Error) -> FtpError {
    if matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) {
        FtpError::Timeout
    } else {
        FtpError::Connection
    }
}
fn validate_path(path: &str) -> Result<(), FtpError> {
    if path.is_empty() || path.contains(['\0', '\r', '\n']) {
        Err(FtpError::InvalidPath)
    } else {
        Ok(())
    }
}
pub(crate) fn normalized_path(home: &str, path: &str) -> Result<String, FtpError> {
    validate_path(path)?;
    let absolute = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("{home}/{path}")
    };
    let mut parts = Vec::new();
    for part in absolute.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    Ok(format!("/{}", parts.join("/")))
}
fn list(
    lease: &mut Lease<'_>,
    path: &str,
    encoding: CharacterEncoding,
    machine: bool,
) -> Result<Vec<FtpEntry>, FtpError> {
    let verb = if machine { "MLSD" } else { "LIST" };
    let (_, mut data) = lease.command(|stream| {
        stream.custom_data_command(
            format!("{verb} {path}"),
            &[Status::AboutToSend, Status::AlreadyOpen],
        )
    })?;
    let mut bytes = Vec::new();
    let mut chunk = [0; CHUNK_BYTES];
    let result = (|| {
        loop {
            lease.watch.check()?;
            let read = data.read(&mut chunk).map_err(io_error)?;
            if read == 0 {
                break;
            }
            if bytes.len().saturating_add(read) > MAX_LIST_BYTES {
                return Err(FtpError::LimitExceeded);
            }
            bytes.extend_from_slice(&chunk[..read]);
            lease.watch.progress();
        }
        let (text, _, invalid) = encoding.codec().decode(&bytes);
        if invalid {
            return Err(FtpError::UnsupportedEncoding);
        }
        let lines: Vec<String> = text.lines().map(str::to_owned).collect();
        if lines.len() > 65_536 {
            return Err(FtpError::LimitExceeded);
        }
        let entries = entries_from_lines(&lines, path, machine);
        for entry in &entries {
            if !crate::file_browser::valid_file_browser_child_name(
                crate::file_browser::FileBrowserBackendKind::Remote,
                &entry.name,
            ) {
                return Err(FtpError::InvalidPath);
            }
        }
        if machine && !lines.is_empty() && entries.is_empty() {
            return Err(FtpError::Protocol("unsupported listing".into()));
        }
        Ok(entries)
    })();
    lease.complete(data, result)
}

// Unlike full listings, completion stops receiving at its budget. ABOR plus the
// existing NOOP fence consumes completion replies before the lease is reused.
fn completion_list(
    lease: &mut Lease<'_>,
    path: &str,
    encoding: CharacterEncoding,
    machine: bool,
    limits: crate::file_browser::completion::CompletionLimits,
) -> Result<(Vec<FtpEntry>, bool), FtpError> {
    let verb = if machine { "MLSD" } else { "LIST" };
    let (_, mut data) = lease.command(|stream| {
        stream.custom_data_command(
            format!("{verb} {path}"),
            &[Status::AboutToSend, Status::AlreadyOpen],
        )
    })?;
    let mut bytes = Vec::new();
    let mut chunk = [0u8; CHUNK_BYTES];
    let mut complete = true;
    let result = (|| {
        let mut lines = 0usize;
        loop {
            lease.watch.check()?;
            let read = data.read(&mut chunk).map_err(io_error)?;
            if read == 0 {
                break;
            }
            lease.watch.progress();
            for byte in &chunk[..read] {
                if bytes.len() >= limits.name_bytes || lines >= limits.entries {
                    complete = false;
                    return Err(FtpError::LimitExceeded);
                }
                bytes.push(*byte);
                if *byte == b'\n' {
                    lines += 1;
                }
            }
        }
        Ok(())
    })();
    let finished = lease.complete(data, result);
    if complete {
        finished?;
    } else {
        if !matches!(finished, Err(FtpError::LimitExceeded)) {
            finished?;
        }
        // A truncated line is not a directory entry, especially in multibyte encodings.
        bytes.truncate(bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1));
    }
    let (text, _, invalid) = encoding.codec().decode(&bytes);
    if invalid {
        return Err(FtpError::UnsupportedEncoding);
    }
    let lines: Vec<_> = text.lines().map(str::to_owned).collect();
    let entries = entries_from_lines(&lines, path, machine);
    if machine && !lines.is_empty() && entries.is_empty() {
        return Err(FtpError::Protocol("unsupported listing".into()));
    }
    for entry in &entries {
        if !crate::file_browser::valid_file_browser_child_name(
            crate::file_browser::FileBrowserBackendKind::Remote,
            &entry.name,
        ) {
            return Err(FtpError::InvalidPath);
        }
    }
    Ok((entries, complete))
}
