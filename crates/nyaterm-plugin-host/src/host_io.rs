//! Blocking adapters run only on bounded extension task workers.
use crate::package::{io_error, reject_link, reject_path_links};
use crate::tasks::{HostAdapter, MAX_TASK_BYTES, TaskContext};
use nyaterm_core::plugins::extension::{HostRequest, TaskEvent};
use nyaterm_core::plugins::{ErrorCode, PluginError, PluginResult, validate_resource_path};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub struct LocalHost {
    storage: PathBuf,
}

impl LocalHost {
    pub fn new(storage: PathBuf) -> PluginResult<Self> {
        fs::create_dir_all(&storage).map_err(|_| io_error())?;
        reject_link(&storage)?;
        Ok(Self { storage })
    }
}

fn completed(bytes: Vec<u8>) -> TaskEvent {
    TaskEvent::Completed {
        bytes,
        exit_code: None,
    }
}

fn scoped_path(root: &str, relative: &str) -> PluginResult<PathBuf> {
    validate_resource_path(relative)?;
    let root = Path::new(root);
    if !root.is_absolute() {
        return Err(io_error());
    }
    let target = root.join(relative);
    reject_path_links(root, &target)?;
    Ok(target)
}

fn read_chunk(path: &Path, offset: u64, length: u32) -> PluginResult<Vec<u8>> {
    if length as usize > MAX_TASK_BYTES {
        return Err(limit());
    }
    let mut file = fs::File::open(path).map_err(|_| io_error())?;
    if !file.metadata().map_err(|_| io_error())?.is_file() {
        return Err(io_error());
    }
    file.seek(SeekFrom::Start(offset)).map_err(|_| io_error())?;
    let mut bytes = Vec::new();
    file.take(u64::from(length))
        .read_to_end(&mut bytes)
        .map_err(|_| io_error())?;
    Ok(bytes)
}

fn limit() -> PluginError {
    PluginError::new(
        ErrorCode::PackageLimit,
        "Extension I/O exceeds its size limit",
    )
}

impl HostAdapter for LocalHost {
    fn execute(&self, request: HostRequest, context: &TaskContext) -> PluginResult<TaskEvent> {
        context.check(&request)?;
        match &request {
            HostRequest::LocalRead {
                root,
                path,
                offset,
                length,
            } => {
                let target = scoped_path(root, path)?;
                context.check(&request)?;
                Ok(completed(read_chunk(&target, *offset, *length)?))
            }
            HostRequest::LocalWrite {
                root,
                path,
                offset,
                bytes,
            } => {
                if bytes.len() > MAX_TASK_BYTES {
                    return Err(limit());
                }
                let target = scoped_path(root, path)?;
                context.check(&request)?;
                let mut file = OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(*offset == 0)
                    .open(target)
                    .map_err(|_| io_error())?;
                if !file.metadata().map_err(|_| io_error())?.is_file() {
                    return Err(io_error());
                }
                file.seek(SeekFrom::Start(*offset))
                    .map_err(|_| io_error())?;
                file.write_all(bytes).map_err(|_| io_error())?;
                Ok(completed(Vec::new()))
            }
            HostRequest::StorageRead { key } => {
                validate_resource_path(key)?;
                let target = self.storage.join(key);
                reject_path_links(&self.storage, &target)?;
                let size = fs::metadata(&target).map_err(|_| io_error())?.len();
                if size > MAX_TASK_BYTES as u64 {
                    return Err(limit());
                }
                let bytes = read_chunk(&target, 0, size as u32)?;
                Ok(completed(bytes))
            }
            HostRequest::StorageWrite { key, bytes } => {
                validate_resource_path(key)?;
                if key.contains('/') || bytes.len() > MAX_TASK_BYTES {
                    return Err(limit());
                }
                let target = self.storage.join(key);
                reject_path_links(&self.storage, &target)?;
                context.check(&request)?;
                fs::write(target, bytes).map_err(|_| io_error())?;
                Ok(completed(Vec::new()))
            }
            HostRequest::Http { url, method, body } => {
                if body.len() > MAX_TASK_BYTES {
                    return Err(limit());
                }
                // Redirects cannot silently widen an origin grant.
                let client = reqwest::blocking::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(std::time::Duration::from_secs(30))
                    .connect_timeout(std::time::Duration::from_secs(10))
                    .build()
                    .map_err(|_| io_error())?;
                let method =
                    reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| io_error())?;
                context.check(&request)?;
                let mut response = client
                    .request(method, url)
                    .body(body.clone())
                    .send()
                    .map_err(|_| io_error())?;
                if !response.status().is_success() {
                    return Err(PluginError::new(
                        ErrorCode::Io,
                        "Extension HTTP request did not succeed; redirects require an explicit request",
                    ));
                }
                let mut bytes = Vec::new();
                let mut chunk = [0u8; 8192];
                loop {
                    context.check(&request)?;
                    let count = response.read(&mut chunk).map_err(|_| io_error())?;
                    if count == 0 {
                        break;
                    }
                    if bytes.len() + count > MAX_TASK_BYTES {
                        return Err(limit());
                    }
                    bytes.extend_from_slice(&chunk[..count]);
                }
                Ok(completed(bytes))
            }
            _ => Err(PluginError::new(
                ErrorCode::Incompatible,
                "This operation requires a desktop host adapter",
            )),
        }
    }
}
