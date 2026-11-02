//! A byte-oriented SFTP peer: string-based server handlers would hide path corruption.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use nyaterm_core::character_encoding::CharacterEncoding;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::FileAttributes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{OpenSftpConnection, OpenSftpSession, RemoteFilePath, SftpService};
use crate::session_config::{SftpSettings, SshSessionConfig};

#[derive(Default)]
pub(super) struct Files {
    pub(super) contents: HashMap<Vec<u8>, Vec<u8>>,
    pub(super) attributes: HashMap<Vec<u8>, FileAttributes>,
    pub(super) links: HashMap<Vec<u8>, Vec<u8>>,
    pub(super) stat_error: Option<u32>,
    pub(super) setstat_error: Option<u32>,
    pub(super) ignore_chmod: bool,
    pub(super) fail_chmod_once: bool,
    pub(super) clear_special_on_write: bool,
    pub(super) change_owner_on_write: bool,
    pub(super) attribute_updates: Vec<FileAttributes>,
    pub(super) directories: HashSet<Vec<u8>>,
    pub(super) requests: usize,
    pub(super) request_kinds: Vec<u8>,
    pub(super) directory_opens: Vec<Vec<u8>>,
    pub(super) request_delay: HashMap<u8, std::time::Duration>,
    pub(super) request_delay_started: Option<std::sync::mpsc::Sender<u8>>,
    pub(super) modified: Option<u32>,
    pub(super) reads: Vec<(u64, usize)>,
    pub(super) read_paths: HashSet<Vec<u8>>,
    pub(super) active_reads: usize,
    pub(super) max_active_reads: usize,
    pub(super) read_delay: std::time::Duration,
    pub(super) read_limit: Option<usize>,
    pub(super) read_gate: Option<Arc<tokio::sync::Semaphore>>,
    pub(super) read_started: Option<std::sync::mpsc::Sender<()>>,
    pub(super) writes: Vec<usize>,
    pub(super) active_writes: usize,
    pub(super) max_active_writes: usize,
    pub(super) write_gate: Option<Arc<tokio::sync::Semaphore>>,
    pub(super) write_started: Option<std::sync::mpsc::Sender<()>>,
    pub(super) server_limits: Option<[u64; 4]>,
    pub(super) open_handles: usize,
    pub(super) max_open_handles: usize,
    pub(super) omit_size: bool,
    pub(super) fail_read_at: Option<u64>,
    pub(super) fail_read_paths: HashSet<Vec<u8>>,
    pub(super) transient_read_failure: bool,
}

fn number(input: &mut &[u8]) -> u32 {
    let (number, rest) = input.split_at(4);
    *input = rest;
    u32::from_be_bytes(number.try_into().unwrap())
}

fn string(input: &mut &[u8]) -> Vec<u8> {
    let length = number(input) as usize;
    let (value, rest) = input.split_at(length);
    *input = rest;
    value.to_vec()
}

fn push_string(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(&(value.len() as u32).to_be_bytes());
    output.extend_from_slice(value);
}

fn status(id: u32, code: u32) -> Vec<u8> {
    let mut packet = vec![101];
    packet.extend_from_slice(&id.to_be_bytes());
    packet.extend_from_slice(&code.to_be_bytes());
    push_string(&mut packet, b"");
    push_string(&mut packet, b"en");
    packet
}

fn default_attributes(size: usize, directory: bool, modified: Option<u32>) -> FileAttributes {
    FileAttributes {
        size: Some(size as u64),
        permissions: Some(if directory { 0o040755 } else { 0o100644 }),
        // Ownership is opt-in so browsing fixtures do not attempt SSH name lookups.
        atime: modified,
        mtime: modified,
        ..FileAttributes::empty()
    }
}

fn push_attributes(packet: &mut Vec<u8>, attrs: &FileAttributes) {
    let ids = attrs.uid.zip(attrs.gid);
    let times = attrs.atime.zip(attrs.mtime);
    let flags = u32::from(attrs.size.is_some())
        | (u32::from(ids.is_some()) << 1)
        | (u32::from(attrs.permissions.is_some()) << 2)
        | (u32::from(times.is_some()) << 3);
    packet.extend_from_slice(&flags.to_be_bytes());
    if let Some(size) = attrs.size {
        packet.extend_from_slice(&size.to_be_bytes());
    }
    if let Some((uid, gid)) = ids {
        packet.extend_from_slice(&uid.to_be_bytes());
        packet.extend_from_slice(&gid.to_be_bytes());
    }
    if let Some(mode) = attrs.permissions {
        packet.extend_from_slice(&mode.to_be_bytes());
    }
    if let Some((atime, mtime)) = times {
        packet.extend_from_slice(&atime.to_be_bytes());
        packet.extend_from_slice(&mtime.to_be_bytes());
    }
}

fn attributes(packet: &mut Vec<u8>, size: usize, directory: bool, modified: Option<u32>) {
    push_attributes(packet, &default_attributes(size, directory, modified));
}

fn reply(
    input: &[u8],
    files: &mut Files,
    enumerated: &mut HashMap<Vec<u8>, usize>,
    handles: &mut HashMap<Vec<u8>, Vec<u8>>,
) -> Vec<u8> {
    let kind = input[0];
    let mut input = &input[1..];
    let id = number(&mut input);
    if kind == 1 {
        let mut version = vec![2];
        version.extend_from_slice(&3_u32.to_be_bytes());
        if files.server_limits.is_some() {
            push_string(&mut version, b"limits@openssh.com");
            push_string(&mut version, b"1");
        }
        push_string(&mut version, b"fsync@openssh.com");
        push_string(&mut version, b"1");
        return version;
    }
    files.requests += 1;
    files.request_kinds.push(kind);
    let mut packet = Vec::new();
    match kind {
        3 | 11 => {
            let path = string(&mut input);
            let path = if kind == 3 {
                files.links.get(&path).cloned().unwrap_or(path)
            } else {
                path
            };
            if kind == 3 {
                let flags = number(&mut input);
                if flags & 8 != 0 {
                    if !files.contents.contains_key(&path) {
                        files
                            .attributes
                            .insert(path.clone(), default_attributes(0, false, None));
                    }
                    files.contents.entry(path.clone()).or_default();
                }
                if !files.contents.contains_key(&path) {
                    return status(id, 2);
                }
                if flags & 16 != 0 {
                    files.contents.get_mut(&path).unwrap().clear();
                }
            } else {
                files.directory_opens.push(path.clone());
                enumerated.remove(&path);
            }
            packet.push(102);
            packet.extend_from_slice(&id.to_be_bytes());
            let handle = format!("h{}", files.requests).into_bytes();
            files.open_handles += 1;
            files.max_open_handles = files.max_open_handles.max(files.open_handles);
            handles.insert(handle.clone(), path);
            push_string(&mut packet, &handle);
        }
        4 => {
            let handle = string(&mut input);
            if handles.remove(&handle).is_some() {
                files.open_handles -= 1;
            }
            return status(id, 0);
        }
        9 | 10 => {
            let path = string(&mut input);
            let path = if kind == 10 {
                handles[&path].clone()
            } else {
                path
            };
            let path = files.links.get(&path).cloned().unwrap_or(path);
            if let Some(error) = files.setstat_error {
                return status(id, error);
            }
            let flags = number(&mut input);
            let mut update = FileAttributes::empty();
            if flags & 1 != 0 {
                update.size = Some(u64::from_be_bytes(input[..8].try_into().unwrap()));
                input = &input[8..];
            }
            if flags & 2 != 0 {
                update.uid = Some(number(&mut input));
                update.gid = Some(number(&mut input));
            }
            if flags & 4 != 0 {
                update.permissions = Some(number(&mut input));
            }
            if flags & 8 != 0 {
                update.atime = Some(number(&mut input));
                update.mtime = Some(number(&mut input));
            }
            files.attribute_updates.push(update.clone());
            if update.permissions.is_some() && std::mem::take(&mut files.fail_chmod_once) {
                return status(id, 3);
            }
            let current = files
                .attributes
                .entry(path)
                .or_insert_with(|| default_attributes(0, false, None));
            if !files.ignore_chmod
                && let Some(mode) = update.permissions
            {
                current.permissions = Some(0o100000 | mode);
            }
            if let Some(uid) = update.uid {
                current.uid = Some(uid);
            }
            if let Some(gid) = update.gid {
                current.gid = Some(gid);
            }
            if let Some(mtime) = update.mtime {
                current.mtime = Some(mtime);
                current.atime = update.atime;
            }
            return status(id, 0);
        }
        5 => {
            let handle = string(&mut input);
            let path = handles[&handle].clone();
            let offset = u64::from_be_bytes(input[..8].try_into().unwrap()) as usize;
            input = &input[8..];
            let length = number(&mut input) as usize;
            files.reads.push((offset as u64, length));
            files.read_paths.insert(path.clone());
            if std::mem::take(&mut files.transient_read_failure) {
                return status(id, 4);
            }
            if files.fail_read_at == Some(offset as u64) || files.fail_read_paths.contains(&path) {
                return status(id, 4);
            }
            let length = files.read_limit.map_or(length, |limit| length.min(limit));
            let content = &files.contents[&path];
            if offset >= content.len() {
                return status(id, 1);
            }
            packet.push(103);
            packet.extend_from_slice(&id.to_be_bytes());
            push_string(
                &mut packet,
                &content[offset..content.len().min(offset + length)],
            );
        }
        6 => {
            let handle = string(&mut input);
            let path = handles[&handle].clone();
            if files.clear_special_on_write || files.change_owner_on_write {
                let attrs = files
                    .attributes
                    .entry(path.clone())
                    .or_insert_with(|| default_attributes(0, false, None));
                if files.clear_special_on_write {
                    attrs.permissions = attrs.permissions.map(|mode| mode & !0o6000);
                }
                if files.change_owner_on_write {
                    attrs.uid = Some(2000);
                }
            }
            let offset = u64::from_be_bytes(input[..8].try_into().unwrap()) as usize;
            input = &input[8..];
            let bytes = string(&mut input);
            files.writes.push(bytes.len());
            let content = files.contents.get_mut(&path).unwrap();
            content.resize(content.len().max(offset + bytes.len()), 0);
            content[offset..offset + bytes.len()].copy_from_slice(&bytes);
            return status(id, 0);
        }
        7 | 8 | 17 => {
            let path = string(&mut input);
            let path = if kind == 8 {
                handles[&path].clone()
            } else {
                path
            };
            let path = if kind == 17 {
                files.links.get(&path).cloned().unwrap_or(path)
            } else {
                path
            };
            if let Some(error) = files.stat_error {
                return status(id, error);
            }
            let directory = files.directories.contains(&path);
            if !directory && !files.contents.contains_key(&path) && !files.links.contains_key(&path)
            {
                return status(id, 2);
            }
            packet.push(105);
            packet.extend_from_slice(&id.to_be_bytes());
            let mut attrs = files
                .attributes
                .get(&path)
                .cloned()
                .unwrap_or_else(|| default_attributes(0, directory, files.modified));
            attrs.size =
                (!files.omit_size).then(|| files.contents.get(&path).map_or(0, Vec::len) as u64);
            if kind == 7 && files.links.contains_key(&path) {
                attrs.permissions = Some(0o120777);
            }
            push_attributes(&mut packet, &attrs);
        }
        12 | 16 => {
            let path = string(&mut input);
            let path = if kind == 12 {
                handles[&path].clone()
            } else {
                path
            };
            let directory_path = path.clone();
            let mut entries: Vec<(Vec<u8>, usize, bool)> = if kind == 16 {
                vec![(path, 0, true)]
            } else {
                let mut prefix = path;
                if !prefix.ends_with(b"/") {
                    prefix.push(b'/');
                }
                files
                    .contents
                    .iter()
                    .filter_map(|(path, content)| {
                        path.strip_prefix(prefix.as_slice())
                            .filter(|name| !name.contains(&b'/'))
                            .map(|name| (name.to_vec(), content.len(), false))
                    })
                    .chain(files.directories.iter().filter_map(|path| {
                        path.strip_prefix(prefix.as_slice())
                            .filter(|name| !name.is_empty() && !name.contains(&b'/'))
                            .map(|name| (name.to_vec(), 0, true))
                    }))
                    .collect()
            };
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            if kind == 12 {
                let cursor = enumerated.entry(directory_path).or_default();
                if *cursor >= entries.len() {
                    return status(id, 1);
                }
                let end = (*cursor + 512).min(entries.len());
                entries = entries[*cursor..end].to_vec();
                *cursor = end;
            }
            packet.push(104);
            packet.extend_from_slice(&id.to_be_bytes());
            packet.extend_from_slice(&(entries.len() as u32).to_be_bytes());
            for (name, size, directory) in entries {
                push_string(&mut packet, &name);
                push_string(&mut packet, b"");
                attributes(&mut packet, size, directory, files.modified);
            }
        }
        13 => {
            let path = string(&mut input);
            return status(
                id,
                if files.contents.remove(&path).is_some() {
                    0
                } else {
                    2
                },
            );
        }
        14 => {
            files.directories.insert(string(&mut input));
            return status(id, 0);
        }
        18 => {
            let source = string(&mut input);
            let target = string(&mut input);
            let content = files.contents.remove(&source).unwrap();
            files.contents.insert(target, content);
            return status(id, 0);
        }
        200 => {
            let extension = string(&mut input);
            if extension == b"limits@openssh.com" {
                packet.push(201);
                packet.extend_from_slice(&id.to_be_bytes());
                for value in files.server_limits.unwrap() {
                    packet.extend_from_slice(&value.to_be_bytes());
                }
            } else {
                return status(id, 0);
            }
        }
        _ => return status(id, 8),
    }
    packet
}

pub(super) fn service(encoding: &str) -> (SftpService, Arc<Mutex<Files>>) {
    service_with_config(encoding, Default::default(), None)
}

fn service_with_config(
    encoding: &str,
    client_config: russh_sftp::client::Config,
    limits: Option<[u64; 4]>,
) -> (SftpService, Arc<Mutex<Files>>) {
    let service = SftpService::new(SshSessionConfig {
        encoding: encoding.to_string(),
        sftp: SftpSettings {
            compatibility_mode: true,
            ..Default::default()
        },
        ..Default::default()
    });
    let files = Arc::new(Mutex::new(Files::default()));
    files.lock().unwrap().directories.insert(b"/".to_vec());
    files.lock().unwrap().server_limits = limits;
    let peer_files = files.clone();
    let compatibility = service.compatibility.as_ref().unwrap();
    let session = compatibility
        .block_on(async move {
            let (client, server) = tokio::io::duplex(65536);
            let (mut server, writer) = tokio::io::split(server);
            let writer = Arc::new(tokio::sync::Mutex::new(writer));
            tokio::spawn(async move {
                let mut enumerated = HashMap::new();
                let mut handles = HashMap::new();
                while let Ok(length) = server.read_u32().await {
                    assert!(length < 1024 * 1024);
                    let mut packet = vec![0; length as usize];
                    server.read_exact(&mut packet).await.unwrap();
                    let response = reply(
                        &packet,
                        &mut peer_files.lock().unwrap(),
                        &mut enumerated,
                        &mut handles,
                    );
                    let is_read = packet[0] == 5;
                    let is_write = packet[0] == 6;
                    let request_kind = packet[0];
                    if is_write {
                        let mut files = peer_files.lock().unwrap();
                        files.active_writes += 1;
                        files.max_active_writes = files.max_active_writes.max(files.active_writes);
                    }
                    let delay = if is_read {
                        let mut files = peer_files.lock().unwrap();
                        files.active_reads += 1;
                        files.max_active_reads = files.max_active_reads.max(files.active_reads);
                        files.read_delay
                    } else {
                        peer_files
                            .lock()
                            .unwrap()
                            .request_delay
                            .get(&packet[0])
                            .copied()
                            .unwrap_or_default()
                    };
                    let writer = writer.clone();
                    let files = peer_files.clone();
                    let gate = if is_read {
                        let files = files.lock().unwrap();
                        if let Some(started) = &files.read_started {
                            let _ = started.send(());
                        }
                        files.read_gate.clone()
                    } else if is_write {
                        let files = files.lock().unwrap();
                        if let Some(started) = &files.write_started {
                            let _ = started.send(());
                        }
                        files.write_gate.clone()
                    } else {
                        None
                    };
                    // Responses have their own delay, rather than stalling the
                    // peer parser. This exposes real concurrent SFTP requests.
                    let gated = gate.is_some();
                    let send = async move {
                        if let Some(gate) = gate {
                            let _ = gate.acquire().await.map(|permit| permit.forget());
                        }
                        if !delay.is_zero() {
                            // Signal only after this response has captured its delay;
                            // clearing the fixture cannot unblock this request.
                            if let Some(started) = &files.lock().unwrap().request_delay_started {
                                let _ = started.send(request_kind);
                            }
                            tokio::time::sleep(delay).await;
                        }
                        // The request stops being in flight once its answer is
                        // committed; release the slot before the bytes reach the
                        // client, so the follow-up request it sends cannot be
                        // counted as one more concurrent operation.
                        if is_read {
                            files.lock().unwrap().active_reads -= 1;
                        }
                        if is_write {
                            files.lock().unwrap().active_writes -= 1;
                        }
                        let mut writer = writer.lock().await;
                        async {
                            writer.write_u32(response.len() as u32).await?;
                            writer.write_all(&response).await
                        }
                        .await
                    };
                    if delay.is_zero() && !gated {
                        if send.await.is_err() {
                            break;
                        }
                    } else {
                        tokio::spawn(send);
                    }
                }
            });
            Ok(OpenSftpSession {
                sftp: Arc::new(SftpSession::new_with_config(client, client_config).await?),
                connection: Arc::new(Mutex::new(Some(OpenSftpConnection::Multiplex))),
                persistent: true,
            })
        })
        .unwrap();
    *compatibility.cache.lock().unwrap() = Some(session);
    (service, files)
}

#[test]
fn directory_download_enumerates_each_directory_once_and_preserves_totals() -> anyhow::Result<()> {
    use crate::{SftpPathTransferOptions, SftpTransferControl};

    let (service, files) = service("UTF-8");
    {
        let mut files = files.lock().unwrap();
        files.directories.extend(
            [
                "/folder",
                "/folder/sub",
                "/folder/sub/deep",
                "/folder/empty",
            ]
            .map(|path| path.as_bytes().to_vec()),
        );
        files.contents.extend([
            (b"/folder/a".to_vec(), b"abc".to_vec()),
            (b"/folder/sub/zero".to_vec(), Vec::new()),
            (b"/folder/sub/deep/b".to_vec(), b"defg".to_vec()),
        ]);
    }
    let root = tempfile::tempdir()?;
    let target = root.path().join("folder");
    let progress = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&progress);
    let summary = service.download_path_with_progress_and_path_options(
        "/folder",
        &target,
        SftpTransferControl::new(),
        SftpPathTransferOptions::default(),
        move |current| recorded.lock().unwrap().push(current),
    )?;

    assert_eq!(summary.bytes, 7);
    assert_eq!(std::fs::read(target.join("a"))?, b"abc");
    assert_eq!(std::fs::read(target.join("sub/zero"))?, b"");
    assert_eq!(std::fs::read(target.join("sub/deep/b"))?, b"defg");
    assert!(target.join("empty").is_dir());
    let mut opened = files.lock().unwrap().directory_opens.clone();
    opened.sort();
    assert_eq!(
        opened,
        [
            "/folder",
            "/folder/empty",
            "/folder/sub",
            "/folder/sub/deep"
        ]
        .map(|path| path.as_bytes().to_vec())
    );
    let progress = progress.lock().unwrap();
    assert!(!progress.is_empty());
    assert!(
        progress.iter().all(|current| {
            current.total_bytes == Some(7) && current.item_count_total == Some(3)
        })
    );
    assert_eq!(progress.last().unwrap().bytes_transferred, 7);
    assert_eq!(progress.last().unwrap().item_count_completed, Some(3));
    Ok(())
}

#[test]
fn directory_download_merge_keeps_totals_for_all_files() -> anyhow::Result<()> {
    use crate::{
        SftpDuplicateDecision, SftpDuplicatePolicy, SftpDuplicateRequest, SftpDuplicateResolver,
        SftpPathTransferOptions, SftpTransferControl, SftpTransferOptions,
    };

    struct Resolver;
    impl SftpDuplicateResolver for Resolver {
        fn resolve_duplicate(
            &self,
            request: &SftpDuplicateRequest,
        ) -> Result<SftpDuplicateDecision, String> {
            assert_eq!(request.source_path, "/folder");
            Ok(SftpDuplicateDecision::Overwrite)
        }
    }

    let (service, files) = service("UTF-8");
    {
        let mut files = files.lock().unwrap();
        files.directories.extend(
            ["/folder", "/folder/skip", "/folder/skip/deep"].map(|path| path.as_bytes().to_vec()),
        );
        files.contents.extend([
            (b"/folder/keep".to_vec(), b"abc".to_vec()),
            (b"/folder/file/skip".to_vec(), b"unused".to_vec()),
            (b"/folder/skip/deep/valid-name".to_vec(), b"defg".to_vec()),
        ]);
        files.directories.insert(b"/folder/file".to_vec());
    }
    let root = tempfile::tempdir()?;
    let target = root.path().join("folder");
    std::fs::create_dir_all(target.join("skip"))?;
    std::fs::create_dir_all(target.join("file"))?;
    std::fs::write(target.join("file/skip"), b"local")?;
    let progress = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&progress);
    let summary = service.download_path_with_progress_and_path_options(
        "/folder",
        &target,
        SftpTransferControl::new(),
        SftpPathTransferOptions::new(
            SftpDuplicatePolicy::Ask,
            Some(Arc::new(Resolver)),
            SftpTransferOptions::default(),
        ),
        move |current| recorded.lock().unwrap().push(current),
    )?;

    assert_eq!(summary.bytes, 13);
    assert_eq!(std::fs::read(target.join("keep"))?, b"abc");
    assert_eq!(std::fs::read(target.join("file/skip"))?, b"unused");
    assert_eq!(std::fs::read(target.join("skip/deep/valid-name"))?, b"defg");
    let mut opened = files.lock().unwrap().directory_opens.clone();
    opened.sort();
    assert_eq!(
        opened,
        [
            "/folder",
            "/folder/file",
            "/folder/skip",
            "/folder/skip/deep"
        ]
        .map(|path| path.as_bytes().to_vec())
    );
    let progress = progress.lock().unwrap();
    assert!(!progress.is_empty());
    assert!(
        progress.iter().all(|current| {
            current.total_bytes == Some(13) && current.item_count_total == Some(3)
        })
    );
    assert_eq!(progress.last().unwrap().item_count_completed, Some(3));
    Ok(())
}

#[test]
fn promised_directory_retry_keeps_completed_files_and_refuses_external_changes()
-> anyhow::Result<()> {
    use crate::{SftpFileType, SftpPathTransferOptions, SftpTransferControl, SftpTransferOptions};
    let (service, files) = service("UTF-8");
    {
        let mut files = files.lock().unwrap();
        files.modified = Some(42);
        files.directories.insert(b"/folder".to_vec());
        files
            .contents
            .insert(b"/folder/a".to_vec(), b"completed".to_vec());
        files
            .contents
            .insert(b"/folder/b".to_vec(), b"unfinished".to_vec());
        files.fail_read_paths.insert(b"/folder/b".to_vec());
    }
    let root = tempfile::tempdir()?;
    let target = root.path().join("folder");
    let options = SftpPathTransferOptions::for_promised_download(
        SftpTransferOptions::default(),
        SftpFileType::Directory,
    );
    let retry = || options.with_transfer_options(options.transfer_options().clone());
    assert!(
        service
            .download_remote_path_with_progress_and_path_options(
                &RemoteFilePath::new("/folder"),
                &target,
                SftpTransferControl::new(),
                retry(),
                |_| {}
            )
            .is_err()
    );
    assert_eq!(std::fs::read(target.join("a"))?, b"completed");
    assert!(!target.join("b").exists());
    let first_reads = files.lock().unwrap().reads.len();
    files.lock().unwrap().fail_read_paths.clear();
    service.download_remote_path_with_progress_and_path_options(
        &RemoteFilePath::new("/folder"),
        &target,
        SftpTransferControl::new(),
        retry(),
        |_| {},
    )?;
    assert_eq!(
        files.lock().unwrap().reads.len() - first_reads,
        1,
        "completed unchanged source must not be downloaded again"
    );
    assert_eq!(std::fs::read(target.join("b"))?, b"unfinished");
    std::fs::write(target.join("a"), b"external change")?;
    assert!(
        service
            .download_remote_path_with_progress_and_path_options(
                &RemoteFilePath::new("/folder"),
                &target,
                SftpTransferControl::new(),
                retry(),
                |_| {}
            )
            .is_err()
    );
    assert_eq!(std::fs::read(target.join("a"))?, b"external change");
    Ok(())
}

#[test]
fn promised_directory_automatic_retry_reuses_its_exclusively_created_root() -> anyhow::Result<()> {
    use crate::{SftpFileType, SftpPathTransferOptions, SftpTransferControl, SftpTransferOptions};
    let (service, files) = service("UTF-8");
    {
        let mut files = files.lock().unwrap();
        files.modified = Some(42);
        files.directories.insert(b"/folder".to_vec());
        files
            .contents
            .insert(b"/folder/file".to_vec(), b"contents".to_vec());
        files.transient_read_failure = true;
    }
    let root = tempfile::tempdir()?;
    let target = root.path().join("folder");
    let options = SftpPathTransferOptions::for_promised_download(
        SftpTransferOptions::default().with_max_retries(1),
        SftpFileType::Directory,
    );
    let result = service.download_remote_path_with_progress_and_path_options(
        &RemoteFilePath::new("/folder"),
        &target,
        SftpTransferControl::new(),
        options,
        |_| {},
    )?;
    assert!(!result.skipped);
    assert_eq!(files.lock().unwrap().reads.len(), 2);
    assert_eq!(std::fs::read(target.join("file"))?, b"contents");
    Ok(())
}

#[test]
fn filename_codecs_preserve_paths_and_file_bytes_through_service_operations() {
    for (encoding, name) in [
        (CharacterEncoding::Utf8, "测试"),
        (CharacterEncoding::Gbk, "测试"),
        (CharacterEncoding::Gb18030, "测试😀"),
        (CharacterEncoding::Big5, "測試"),
        (CharacterEncoding::ShiftJis, "日本語"),
        (CharacterEncoding::EucKr, "한국어"),
    ] {
        let (service, files) = service(encoding.label());
        let directory = format!("/{name}");
        let original = format!("{directory}/{name}.txt");
        let renamed = format!("{directory}/{name}-2.txt");
        service.create_dir_path(&directory, None).unwrap();
        service.create_file_path(&original, None).unwrap();
        service.rename_path(&original, &renamed).unwrap();
        assert!(
            files
                .lock()
                .unwrap()
                .contents
                .contains_key(&encoding.encode(&renamed).unwrap())
        );
        let local = nyaterm_core::test_support::TestTempDir::new("nyaterm-sftp-encoded-path");
        std::fs::create_dir_all(&local).unwrap();
        let source = local.join("source");
        let download = local.join("download");
        let contents = b"raw\xff\x00\xb2\xe2";
        std::fs::write(&source, contents).unwrap();
        service.upload_file(&source, &renamed).unwrap();
        let entries = service.list_dir(&directory).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, format!("{name}-2.txt"));
        let path = entries[0].remote_path();
        assert_eq!(
            path.raw_path().unwrap().unwrap(),
            encoding.encode(&renamed).unwrap()
        );
        service
            .download_remote_file_with_progress_and_control_options(
                &path,
                &download,
                Default::default(),
                Default::default(),
                |_| {},
            )
            .unwrap();
        assert_eq!(std::fs::read(download).unwrap(), contents);
        service.delete_remote_path(&path).unwrap();
        assert!(files.lock().unwrap().contents.is_empty());
    }
}

#[test]
fn lossy_labels_keep_distinct_path_identity_and_invalid_new_names_send_no_requests() {
    let (service, files) = service("GBK");
    files.lock().unwrap().contents.extend([
        (b"/\xff".to_vec(), b"one".to_vec()),
        (b"/\xfe".to_vec(), b"two".to_vec()),
    ]);
    let entries = service.list_dir("/").unwrap();
    assert_eq!(entries[0].name, entries[1].name);
    let mut contents = Vec::new();
    for entry in entries {
        let path: RemoteFilePath = entry.remote_path();
        contents.push(
            service
                .read_file_bytes_path(&path, 128)
                .unwrap()
                .content_bytes,
        );
        service.delete_remote_path(&path).unwrap();
    }
    contents.sort();
    assert_eq!(contents, [b"one".to_vec(), b"two".to_vec()]);
    let before = files.lock().unwrap().requests;
    let error = service.create_file_path("/secret😀", None).unwrap_err();
    assert!(!format!("{error:?}").contains("secret"));
    assert_eq!(files.lock().unwrap().requests, before);
    assert!(service.create_dir_path("/secret😀", None).is_err());
    assert!(service.rename_path("/existing", "/secret😀").is_err());
    let local = nyaterm_core::test_support::TestTempDir::new("nyaterm-sftp-invalid-upload");
    std::fs::create_dir_all(&local).unwrap();
    let source = local.join("source");
    std::fs::write(&source, b"data").unwrap();
    assert!(service.upload_file(&source, "/secret😀").is_err());
    assert_eq!(files.lock().unwrap().requests, before);
}

#[test]
fn ordinary_and_directory_downloads_fill_the_configured_window_before_the_first_reply()
-> anyhow::Result<()> {
    use crate::{SftpPathTransferOptions, SftpTransferControl};
    use std::time::Duration;
    for (directory, depth, compatible, expected) in [
        (false, None, false, 16),
        (true, None, false, 16),
        (false, Some(4), false, 4),
        (true, Some(64), false, 64),
        (false, Some(64), true, 1),
        (true, Some(64), true, 1),
    ] {
        let (mut service, files) = service("UTF-8");
        service.config.sftp.compatibility_mode = compatible;
        service.config.sftp.pipeline_depth = depth;
        let content = vec![7_u8; 80 * 63 * 1024];
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let (started, requests) = std::sync::mpsc::channel();
        {
            let mut peer = files.lock().unwrap();
            peer.directories.insert(b"/folder".to_vec());
            peer.contents
                .insert(b"/folder/file".to_vec(), content.clone());
            peer.read_gate = Some(gate.clone());
            peer.read_started = Some(started);
        }
        let root = tempfile::tempdir()?;
        let target = root.path().join("download");
        let control = SftpTransferControl::new();
        let worker_control = control.clone();
        let worker_target = target.clone();
        let worker = std::thread::spawn(move || {
            service.download_path_with_progress_and_path_options(
                if directory { "/folder" } else { "/folder/file" },
                &worker_target,
                worker_control,
                SftpPathTransferOptions::default(),
                |_| {},
            )
        });
        let admitted = (0..expected).all(|_| requests.recv_timeout(Duration::from_secs(2)).is_ok());
        gate.add_permits(4096);
        if !admitted {
            control.cancel();
        }
        let result = worker.join().unwrap();
        assert!(
            admitted,
            "download did not fill its {expected} request window"
        );
        result?;
        let file = if directory {
            target.join("file")
        } else {
            target
        };
        assert_eq!(std::fs::read(file)?, content);
        assert_eq!(files.lock().unwrap().max_active_reads, expected);
        assert_eq!(files.lock().unwrap().open_handles, 0);
    }
    Ok(())
}

#[test]
fn uploads_use_full_write_packets_and_wait_for_the_configured_ack_window() -> anyhow::Result<()> {
    use crate::{SftpTransferControl, SftpTransferOptions};
    use std::time::Duration;
    for (packet, depth, compatible, limits, expected) in [
        (64, None, false, None, 16),
        (256, None, false, None, 8),
        (64, Some(4), false, None, 4),
        (64, Some(64), false, Some([65536, 8192, 16384, 16]), 64),
        (64, Some(64), true, None, 1),
    ] {
        let settings = SftpSettings {
            pipeline_depth: depth,
            compatibility_mode: compatible,
            ..Default::default()
        };
        let options = SftpTransferOptions::default().with_buffer_size_bytes(packet * 1024);
        let parameters = super::tuning::TransferParameters::new(&options, &settings);
        let (mut service, files) = service_with_config("UTF-8", parameters.client_config(), limits);
        service.config.sftp = settings;
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let (started, requests) = std::sync::mpsc::channel();
        {
            let mut peer = files.lock().unwrap();
            peer.write_gate = Some(gate.clone());
            peer.write_started = Some(started);
        }
        let root = tempfile::tempdir()?;
        let source = root.path().join("source");
        let content = vec![19_u8; 80 * parameters.payload];
        std::fs::write(&source, &content)?;
        let control = SftpTransferControl::new();
        let worker_control = control.clone();
        let worker = std::thread::spawn(move || {
            service.upload_file_with_progress_and_control_options(
                source,
                "/uploaded",
                worker_control,
                options,
                |_| {},
            )
        });
        let admitted = (0..expected).all(|_| requests.recv_timeout(Duration::from_secs(2)).is_ok());
        gate.add_permits(16384);
        if !admitted {
            control.cancel();
        }
        let result = worker.join().unwrap();
        assert!(admitted, "upload did not fill its {expected} ACK window");
        result?;
        let peer = files.lock().unwrap();
        assert_eq!(peer.max_active_writes, expected);
        assert_eq!(peer.contents[b"/uploaded".as_slice()], content);
        let largest = *peer.writes.iter().max().unwrap();
        assert_eq!(
            largest,
            if limits.is_some() {
                16384
            } else {
                parameters.payload
            }
        );
        // The only EXTENDED request permitted here is server-limit discovery.
        assert_eq!(
            peer.request_kinds
                .iter()
                .filter(|kind| **kind == 200)
                .count(),
            usize::from(limits.is_some())
        );
        assert_eq!(peer.active_writes, 0);
        assert_eq!(peer.open_handles, 0);
    }
    Ok(())
}

#[test]
fn downloads_preserve_short_reads_unknown_lengths_and_advertised_server_limits()
-> anyhow::Result<()> {
    use crate::{SftpTransferControl, SftpTransferOptions};
    for unknown in [false, true] {
        let options = SftpTransferOptions::default().with_buffer_size_bytes(256 * 1024);
        let settings = SftpSettings {
            pipeline_depth: Some(4),
            ..Default::default()
        };
        let parameters = super::tuning::TransferParameters::new(&options, &settings);
        let (mut service, files) = service_with_config(
            "UTF-8",
            parameters.client_config(),
            Some([65536, 8192, 16384, 12]),
        );
        service.config.sftp = settings;
        let content: Vec<_> = (0..300_111).map(|i| (i % 251) as u8).collect();
        {
            let mut peer = files.lock().unwrap();
            peer.contents.insert(b"/file".to_vec(), content.clone());
            peer.omit_size = unknown;
            peer.read_limit = Some(3001);
        }
        let root = tempfile::tempdir()?;
        let target = root.path().join("target");
        let result = service.download_file_with_progress_and_control_options(
            "/file",
            &target,
            SftpTransferControl::new(),
            options,
            |_| {},
        )?;
        assert_eq!(result.bytes, content.len() as u64);
        assert_eq!(std::fs::read(target)?, content);
        let peer = files.lock().unwrap();
        assert!(peer.reads.iter().all(|(_, length)| *length <= 8192));
        assert_eq!(peer.open_handles, 0);
    }
    Ok(())
}

#[test]
fn directory_workers_are_automatic_and_respect_server_handles() -> anyhow::Result<()> {
    use crate::{SftpPathTransferOptions, SftpTransferControl, SftpTransferOptions};
    use std::time::Duration;
    for (limits, expected) in [(None, 16), (Some([65536, 8192, 16384, 12]), 4)] {
        let (mut service, files) = service_with_config("UTF-8", Default::default(), limits);
        service.config.sftp.compatibility_mode = false;
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let (started, requests) = std::sync::mpsc::channel();
        {
            let mut peer = files.lock().unwrap();
            peer.directories.insert(b"/folder".to_vec());
            peer.directories.insert(b"/folder/empty".to_vec());
            for i in 0..80 {
                peer.contents
                    .insert(format!("/folder/{i:03}").into_bytes(), vec![i]);
            }
            peer.contents
                .insert(b"/folder/raw-\xff".to_vec(), b"raw".to_vec());
            peer.read_gate = Some(gate.clone());
            peer.read_started = Some(started);
        }
        let root = tempfile::tempdir()?;
        let target = root.path().join("target");
        let worker_target = target.clone();
        let control = SftpTransferControl::new();
        let worker_control = control.clone();
        let options = SftpPathTransferOptions::default()
            .with_transfer_options(SftpTransferOptions::default().with_download_threads(1));
        let worker = std::thread::spawn(move || {
            service.download_path_with_progress_and_path_options(
                "/folder",
                worker_target,
                worker_control,
                options,
                |_| {},
            )
        });
        let admitted = (0..expected).all(|_| requests.recv_timeout(Duration::from_secs(2)).is_ok());
        gate.add_permits(16384);
        if !admitted {
            control.cancel();
        }
        let result = worker.join().unwrap();
        assert!(admitted, "directory did not admit {expected} workers");
        result?;
        assert!(target.join("empty").is_dir());
        assert_eq!(std::fs::read_dir(&target)?.count(), 82);
        for i in 0..80 {
            assert_eq!(std::fs::read(target.join(format!("{i:03}")))?, vec![i]);
        }
        let peer = files.lock().unwrap();
        assert_eq!(peer.max_active_reads, expected);
        assert_eq!(peer.max_open_handles, expected);
        assert_eq!(peer.open_handles, 0);
    }
    Ok(())
}

#[test]
fn mixed_directory_keeps_two_large_file_lanes_and_small_file_workers() -> anyhow::Result<()> {
    use crate::{SftpPathTransferOptions, SftpTransferControl};
    use std::time::Duration;
    let (mut service, files) = service("UTF-8");
    service.config.sftp.compatibility_mode = false;
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let (started, requests) = std::sync::mpsc::channel();
    {
        let mut peer = files.lock().unwrap();
        peer.directories.insert(b"/folder".to_vec());
        for i in 0..3 {
            peer.contents.insert(
                format!("/folder/large{i}").into_bytes(),
                vec![i; 10 * 63 * 1024],
            );
        }
        for i in 0..40 {
            peer.contents
                .insert(format!("/folder/small{i:02}").into_bytes(), vec![i]);
        }
        peer.read_gate = Some(gate.clone());
        peer.read_started = Some(started);
    }
    let root = tempfile::tempdir()?;
    let target = root.path().join("target");
    let worker_target = target.clone();
    let control = SftpTransferControl::new();
    let worker_control = control.clone();
    let worker = std::thread::spawn(move || {
        service.download_path_with_progress_and_path_options(
            "/folder",
            worker_target,
            worker_control,
            SftpPathTransferOptions::default(),
            |_| {},
        )
    });
    // 16 workers: two large files with ten ranges each, one awaiting a
    // large-file lane, and thirteen one-range small files.
    let admitted = (0..33).all(|_| requests.recv_timeout(Duration::from_secs(2)).is_ok());
    let large_started = files
        .lock()
        .unwrap()
        .read_paths
        .iter()
        .filter(|path| path.starts_with(b"/folder/large"))
        .count();
    gate.add_permits(4096);
    if !admitted {
        control.cancel();
    }
    let result = worker.join().unwrap();
    assert!(admitted);
    assert_eq!(large_started, 2);
    result?;
    assert_eq!(std::fs::read_dir(target)?.count(), 43);
    assert_eq!(files.lock().unwrap().open_handles, 0);
    Ok(())
}

#[test]
fn failure_and_cancellation_publish_only_resumable_contiguous_prefixes() -> anyhow::Result<()> {
    use crate::{SftpPathTransferOptions, SftpTransferControl, SftpTransferOptions};
    for cancelled in [false, true] {
        for resume in [false, true] {
            let (mut service, files) = service("UTF-8");
            service.config.sftp.compatibility_mode = false;
            let content = vec![31_u8; 40 * 63 * 1024];
            {
                let mut peer = files.lock().unwrap();
                peer.contents.insert(b"/file".to_vec(), content.clone());
                if !cancelled {
                    peer.fail_read_at = Some(2 * 63 * 1024);
                }
            }
            let root = tempfile::tempdir()?;
            let target = root.path().join("target");
            let options = SftpPathTransferOptions::default().with_transfer_options(
                SftpTransferOptions::default().with_resume_broken_transfer(resume),
            );
            let control = SftpTransferControl::new();
            let callback_control = control.clone();
            assert!(
                service
                    .download_path_with_progress_and_path_options(
                        "/file",
                        &target,
                        control,
                        options.clone(),
                        move |progress| {
                            if cancelled && progress.bytes_transferred >= 63 * 1024 {
                                callback_control.cancel();
                            }
                        }
                    )
                    .is_err()
            );
            assert_eq!(target.exists(), resume);
            if resume {
                let prefix = std::fs::read(&target)?;
                assert!(!prefix.is_empty());
                assert!(prefix.len() < content.len());
                assert_eq!(prefix, content[..prefix.len()]);
            }
            assert_eq!(std::fs::read_dir(root.path())?.count(), usize::from(resume));
            files.lock().unwrap().fail_read_at = None;
            // Reuse the same owner: direct publication must have refreshed identity.
            service.download_path_with_progress_and_path_options(
                "/file",
                &target,
                SftpTransferControl::new(),
                options,
                |_| {},
            )?;
            assert_eq!(std::fs::read(&target)?, content);
            assert_eq!(files.lock().unwrap().open_handles, 0);
        }
    }
    Ok(())
}
