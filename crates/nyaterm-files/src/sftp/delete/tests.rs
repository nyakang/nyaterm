use super::{
    collect_inventory, ensure_safe_target, fast_remove_command, ignore_not_found, remove_inventory,
};
use crate::sftp::{
    OpenSftpConnection, OpenSftpSession, PooledSftpSession, RemoteFilePath, SftpPathCodec,
    SftpService, SftpSessionPool,
};
use nyaterm_transport::{SftpSettings, SshSessionConfig};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{Attrs, File, FileAttributes, Handle, Name, Status, StatusCode};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

mod ssh;

#[test]
fn fast_delete_requires_utf8_and_matching_raw_path_and_quotes_shell_metacharacters() {
    let utf8 = SftpPathCodec::from_encoding_name("UTF-8").unwrap();
    assert_eq!(
        fast_remove_command(&utf8, "/tmp/a'b;$x/", b"/tmp/a'b;$x/"),
        Some("rm -rf -- '/tmp/a'\\''b;$x'".into())
    );
    assert!(fast_remove_command(&utf8, "/tmp/display", b"/tmp/\xff").is_none());
    let gbk = SftpPathCodec::from_encoding_name("GBK").unwrap();
    assert!(fast_remove_command(&gbk, "/tmp/ascii", b"/tmp/ascii").is_none());
}

#[test]
fn unsafe_raw_and_display_targets_are_rejected_before_deletion() {
    let utf8 = SftpPathCodec::from_encoding_name("UTF-8").unwrap();
    for path in [
        "",
        "/",
        "///",
        ".",
        "./",
        "..",
        "/./",
        "/tmp/../home",
        "/tmp/\0x",
    ] {
        assert!(ensure_safe_target(path.as_bytes()).is_err(), "{path:?}");
        assert!(fast_remove_command(&utf8, path, path.as_bytes()).is_none());
    }
    assert!(ensure_safe_target(b"/tmp/\xff").is_ok());
}

#[derive(Default)]
struct Files {
    entries: HashMap<String, u32>,
    removed: Vec<String>,
    denied: HashSet<String>,
    lstat_calls: usize,
}

struct Peer {
    files: Arc<Mutex<Files>>,
    listed: HashSet<String>,
}

fn ok(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: String::new(),
        language_tag: String::new(),
    }
}

impl russh_sftp::server::Handler for Peer {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let mut files = self.files.lock().unwrap();
        files.lstat_calls += 1;
        let mode = *files.entries.get(&path).ok_or(StatusCode::NoSuchFile)?;
        Ok(Attrs {
            id,
            attrs: FileAttributes {
                permissions: Some(mode),
                ..Default::default()
            },
        })
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let files = self.files.lock().unwrap();
        if files.entries.get(&path) != Some(&0o040755) {
            return Err(StatusCode::NoSuchFile);
        }
        self.listed.remove(&path);
        Ok(Handle { id, handle: path })
    }

    async fn readdir(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        if !self.listed.insert(path.clone()) {
            return Err(StatusCode::Eof);
        }
        let files = self.files.lock().unwrap();
        let prefix = format!("{path}/");
        let mut entries = files
            .entries
            .iter()
            .filter_map(|(name, mode)| {
                let name = name.strip_prefix(&prefix)?;
                if name.contains('/') {
                    return None;
                }
                Some(File::new(
                    name,
                    FileAttributes {
                        permissions: Some(*mode),
                        ..Default::default()
                    },
                ))
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.filename.cmp(&right.filename));
        Ok(Name { id, files: entries })
    }

    async fn close(&mut self, id: u32, _handle: String) -> Result<Status, Self::Error> {
        Ok(ok(id))
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        let mut files = self.files.lock().unwrap();
        if files.denied.contains(&filename) {
            return Err(StatusCode::PermissionDenied);
        }
        files
            .entries
            .remove(&filename)
            .ok_or(StatusCode::NoSuchFile)?;
        files.removed.push(filename);
        Ok(ok(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        let mut files = self.files.lock().unwrap();
        if files
            .entries
            .keys()
            .any(|entry| entry.starts_with(&format!("{path}/")))
        {
            return Err(StatusCode::Failure);
        }
        files.entries.remove(&path).ok_or(StatusCode::NoSuchFile)?;
        files.removed.push(path);
        Ok(ok(id))
    }
}

async fn pool(files: Arc<Mutex<Files>>) -> SftpSessionPool {
    let (client, server) = tokio::io::duplex(65536);
    tokio::spawn(russh_sftp::server::run(
        server,
        Peer {
            files,
            listed: HashSet::new(),
        },
    ));
    let session = OpenSftpSession {
        sftp: Arc::new(SftpSession::new(client).await.unwrap()),
        connection: Arc::new(Mutex::new(Some(OpenSftpConnection::Multiplex))),
        persistent: false,
    };
    SftpSessionPool {
        sessions: Arc::new(vec![Arc::new(PooledSftpSession::from_open_session(
            session,
        ))]),
    }
}

fn files() -> Arc<Mutex<Files>> {
    Arc::new(Mutex::new(Files {
        entries: [
            ("/dir", 0o040755),
            ("/dir/sub", 0o040755),
            ("/dir/a", 0o100644),
            ("/dir/sub/b", 0o100644),
            ("/dir/link", 0o120777),
            ("/outside", 0o100644),
        ]
        .into_iter()
        .map(|(path, mode)| (path.into(), mode))
        .collect(),
        ..Default::default()
    }))
}

fn seeded_service(files: Arc<Mutex<Files>>) -> SftpService {
    let service = SftpService::new(SshSessionConfig {
        sftp: SftpSettings {
            compatibility_mode: true,
            filename_encoding: "GBK".into(),
            ..Default::default()
        },
        ..Default::default()
    });
    let compatibility = service.compatibility.as_ref().unwrap();
    let session = compatibility
        .block_on(async move {
            let (client, server) = tokio::io::duplex(65536);
            tokio::spawn(russh_sftp::server::run(
                server,
                Peer {
                    files,
                    listed: HashSet::new(),
                },
            ));
            Ok(OpenSftpSession {
                sftp: Arc::new(SftpSession::new(client).await?),
                connection: Arc::new(Mutex::new(Some(OpenSftpConnection::Multiplex))),
                persistent: true,
            })
        })
        .unwrap();
    *compatibility.cache.lock().unwrap() = Some(session);
    service
}

#[test]
fn compatibility_service_deletes_directory_without_exec_and_reuses_its_session() {
    let files = files();
    let service = seeded_service(files.clone());
    service.delete_path("/dir/").unwrap();
    service.delete_path("/dir").unwrap();
    let files = files.lock().unwrap();
    assert_eq!(files.entries.len(), 1);
    assert!(files.entries.contains_key("/outside"));
    assert_eq!(files.lstat_calls, 2);
}

#[test]
fn service_unlinks_symlinks_and_rejects_unsafe_raw_token_before_any_request() {
    let files = files();
    let service = seeded_service(files.clone());
    assert!(
        service
            .delete_remote_path(&RemoteFilePath::from_raw("/dir/safe", b"/"))
            .is_err()
    );
    assert_eq!(files.lock().unwrap().lstat_calls, 0);
    service.delete_path("/dir/link/").unwrap();
    assert!(files.lock().unwrap().entries.contains_key("/outside"));
    assert!(!files.lock().unwrap().entries.contains_key("/dir/link"));
}

#[tokio::test]
async fn fallback_removes_files_and_links_before_directories_without_child_lstat() {
    let files = files();
    let pool = pool(files.clone()).await;
    let inventory = collect_inventory(&pool.session_for(0).sftp, b"/dir".to_vec())
        .await
        .unwrap();
    assert_eq!(inventory.files.len(), 3);
    remove_inventory(pool.clone(), inventory, 3).await.unwrap();
    pool.close_all().await;
    let files = files.lock().unwrap();
    assert_eq!(files.lstat_calls, 0);
    assert_eq!(files.entries.len(), 1);
    assert!(files.entries.contains_key("/outside"));
    assert_eq!(&files.removed[3..], &["/dir/sub", "/dir"]);
}

#[tokio::test]
async fn single_worker_continues_after_permission_failure_and_reports_partial_deletion() {
    let files = files();
    files.lock().unwrap().denied.insert("/dir/a".into());
    let pool = pool(files.clone()).await;
    let inventory = collect_inventory(&pool.session_for(0).sftp, b"/dir".to_vec())
        .await
        .unwrap();
    assert!(remove_inventory(pool.clone(), inventory, 1).await.is_err());
    pool.close_all().await;
    let files = files.lock().unwrap();
    assert!(files.entries.contains_key("/dir/a"));
    assert!(!files.entries.contains_key("/dir/link"));
    assert!(!files.entries.contains_key("/dir/sub/b"));
    assert!(!files.entries.contains_key("/dir/sub"));
}

#[tokio::test]
async fn concurrently_disappearing_files_and_directories_are_successful() {
    let files = files();
    let pool = pool(files.clone()).await;
    let inventory = collect_inventory(&pool.session_for(0).sftp, b"/dir".to_vec())
        .await
        .unwrap();
    files
        .lock()
        .unwrap()
        .entries
        .retain(|path, _| path == "/outside");
    remove_inventory(pool.clone(), inventory, 2).await.unwrap();
    let missing = collect_inventory(&pool.session_for(0).sftp, b"/missing".to_vec())
        .await
        .unwrap();
    assert!(missing.dirs.is_empty());
    pool.close_all().await;
    assert!(
        ignore_not_found(Err(russh_sftp::client::error::Error::Status(Status {
            status_code: StatusCode::NoSuchFile,
            ..ok(1)
        })))
        .is_ok()
    );
    assert!(
        ignore_not_found(Err(russh_sftp::client::error::Error::Status(Status {
            status_code: StatusCode::PermissionDenied,
            ..ok(1)
        })))
        .is_err()
    );
}
