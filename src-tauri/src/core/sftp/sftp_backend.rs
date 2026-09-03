//! SFTP backend: standard SSH SFTP subsystem via russh-sftp.
//!
//! This is the preferred backend when the server supports `Subsystem sftp`.

use super::CopyResolvedTarget;
use super::traits::RemoteFs;
use super::transfer::*;
use super::util::*;
use crate::core::ssh::SshConnectionHandles;
use crate::error::{AppError, AppResult};
use crate::observability::{StructuredLog, StructuredLogLevel, log_event};
use encoding_rs::{Encoding, UTF_8};
use russh::{ChannelMsg, ChannelOpenFailure};
use russh_sftp::client::{Config as SftpClientConfig, SftpSession, error::Error as SftpError};
use russh_sftp::protocol::{FileAttributes, FileType, StatusCode};
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore};

mod attrs;
mod config;
mod copy;
mod directory;
mod file_transfer;
mod fs;
mod path;
mod session;
#[cfg(test)]
mod tests;

use attrs::*;
use config::*;
use copy::*;
use directory::*;
use file_transfer::*;
use path::*;
use session::*;

#[derive(Clone)]
pub(crate) struct SftpBackend {
    ssh_handle: Arc<SshConnectionHandles>,
    identity_cache: Arc<RwLock<RemoteIdentityCache>>,
    /// Cache mapping decoded paths to their raw byte representations.
    /// Used to preserve original encoding for non-UTF-8 file names.
    path_cache: Arc<RwLock<HashMap<String, Vec<u8>>>>,
    /// Encoding for this connection (e.g., "UTF-8", "GBK")
    encoding: String,
    /// Optional per-session override for single-file SFTP request pipelining.
    pipeline_depth_override: Option<u32>,
}

#[derive(Default)]
struct RemoteIdentityCache {
    users_by_uid: HashMap<u32, String>,
    groups_by_gid: HashMap<u32, String>,
    uids_by_user: HashMap<String, u32>,
    gids_by_group: HashMap<String, u32>,
}

struct ExecResult {
    exit_code: u32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}
