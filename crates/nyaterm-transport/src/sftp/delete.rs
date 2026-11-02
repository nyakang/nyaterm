//! Server-side directory deletion with a byte-preserving SFTP fallback.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh_sftp::client::{Config, SftpSession, error::Error as SftpError};
use russh_sftp::protocol::{FileType, StatusCode};
use tokio::task::JoinSet;

use super::{
    RemoteFilePath, SftpPathCodec, SftpSessionPool, SshMultiplexHandle, SshSessionConfig,
    close_sftp_session, directory_upload_worker_count, open_sftp_session,
    sftp_directory_concurrency,
};
use crate::remote_process::{exec_ssh_command, exec_ssh_command_with_multiplex};
use crate::sftp_transfer_types::SftpTransferOptions;

const DELETE_EXEC_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Default)]
struct RemoveInventory {
    files: Vec<Vec<u8>>,
    dirs: Vec<Vec<u8>>,
}

pub(super) fn is_not_found(error: &SftpError) -> bool {
    matches!(error, SftpError::Status(status) if status.status_code == StatusCode::NoSuchFile)
}

fn ignore_not_found(result: Result<(), SftpError>) -> anyhow::Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(error) if is_not_found(&error) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn normalize_path(path: &[u8]) -> &[u8] {
    let end = path
        .iter()
        .rposition(|byte| *byte != b'/')
        .map_or(0, |i| i + 1);
    &path[..end]
}

fn ensure_safe_target(path: &[u8]) -> anyhow::Result<()> {
    let path = normalize_path(path);
    if path.contains(&0)
        || !path
            .split(|byte| *byte == b'/')
            .any(|part| !part.is_empty() && part != b".")
        || path.split(|byte| *byte == b'/').any(|part| part == b"..")
    {
        anyhow::bail!("Refusing to delete unsafe remote path");
    }
    Ok(())
}

fn fast_remove_command(codec: &SftpPathCodec, display: &str, raw: &[u8]) -> Option<String> {
    let display = normalize_path(display.as_bytes());
    if !codec.is_utf8() || normalize_path(raw) != display || ensure_safe_target(display).is_err() {
        return None;
    }
    let path = std::str::from_utf8(display).ok()?;
    Some(format!("rm -rf -- '{}'", path.replace('\'', "'\\''")))
}

pub(super) async fn delete_remote_path(
    config: &SshSessionConfig,
    multiplex: Option<&SshMultiplexHandle>,
    codec: &SftpPathCodec,
    path: &RemoteFilePath,
    raw_path: Vec<u8>,
) -> anyhow::Result<()> {
    // The token is authoritative for SFTP; validating only its display label is insufficient.
    ensure_safe_target(path.display_path.trim().as_bytes())?;
    ensure_safe_target(&raw_path)?;
    let raw_path = normalize_path(&raw_path).to_vec();
    let session = open_sftp_session(config, multiplex).await?;
    let metadata = session.sftp.symlink_metadata_bytes(raw_path.clone()).await;
    let attrs = match metadata {
        Ok(attrs) => attrs,
        Err(error) => {
            close_sftp_session(session).await;
            return if is_not_found(&error) {
                Ok(())
            } else {
                Err(error.into())
            };
        }
    };
    if attrs.file_type() != FileType::Dir {
        let result = ignore_not_found(session.sftp.remove_file_bytes(raw_path).await);
        close_sftp_session(session).await;
        return result;
    }
    close_sftp_session(session).await;

    if let Some(command) = fast_remove_command(codec, &path.display_path, &raw_path) {
        let output = match multiplex {
            Some(multiplex) => {
                exec_ssh_command_with_multiplex(
                    config.clone(),
                    multiplex.clone(),
                    command.into_bytes(),
                    DELETE_EXEC_TIMEOUT,
                )
                .await
            }
            None => {
                exec_ssh_command(config.clone(), command.into_bytes(), DELETE_EXEC_TIMEOUT).await
            }
        };
        if output.is_ok_and(|output| output.exit_status == Some(0)) {
            return Ok(());
        }
        // Do not log commands, paths or server stderr: they may contain user data.
        tracing::debug!(
            stage = "delete_exec_fallback",
            "Using SFTP directory delete fallback"
        );
    }

    let session = open_sftp_session(config, multiplex).await?;
    let max_open_handles = session.sftp.max_open_handles();
    let inventory = collect_inventory(&session.sftp, raw_path).await;
    close_sftp_session(session).await;
    let inventory = inventory?;
    if inventory.dirs.is_empty() {
        return Ok(());
    }
    let concurrency = sftp_directory_concurrency(
        max_open_handles,
        &SftpTransferOptions::default(),
        config.sftp.compatibility_mode,
    );
    let workers = directory_upload_worker_count(inventory.files.len(), concurrency);
    let pool_size = if workers == 0 {
        1
    } else {
        concurrency.session_pool_size
    };
    let pool = SftpSessionPool::open(config, multiplex, pool_size, Config::default()).await?;
    let result = remove_inventory(pool.clone(), inventory, workers).await;
    pool.close_all().await;
    result
}

async fn collect_inventory(sftp: &SftpSession, root: Vec<u8>) -> anyhow::Result<RemoveInventory> {
    let mut inventory = RemoveInventory::default();
    let mut pending = vec![root];
    while let Some(path) = pending.pop() {
        let entries = match sftp.read_dir_bytes(path.clone()).await {
            Ok(entries) => entries,
            Err(error) if is_not_found(&error) => continue,
            Err(error) => return Err(error.into()),
        };
        inventory.dirs.push(path.clone());
        for entry in entries {
            let name = entry.file_name_bytes();
            if matches!(name, b"." | b"..") {
                continue;
            }
            if name.is_empty() || name.contains(&b'/') || name.contains(&0) {
                anyhow::bail!("Invalid remote directory entry during delete");
            }
            let mut child = path.clone();
            child.push(b'/');
            child.extend_from_slice(name);
            if entry.file_type() == FileType::Dir {
                pending.push(child);
            } else {
                inventory.files.push(child);
            }
        }
    }
    Ok(inventory)
}

async fn remove_inventory(
    pool: SftpSessionPool,
    mut inventory: RemoveInventory,
    workers: usize,
) -> anyhow::Result<()> {
    let queue = Arc::new(Mutex::new(VecDeque::from(inventory.files)));
    let mut tasks = JoinSet::new();
    for index in 0..workers {
        let queue = queue.clone();
        let session = pool.session_for(index);
        tasks.spawn(async move {
            let mut errors = Vec::new();
            loop {
                let path = queue
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .pop_front();
                let Some(path) = path else { break };
                if let Err(error) = ignore_not_found(session.sftp.remove_file_bytes(path).await) {
                    // Keep draining the queue after recoverable failures, including with one worker.
                    errors.push(error.to_string());
                }
            }
            errors
        });
    }
    let mut errors = Vec::new();
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(worker_errors) => errors.extend(worker_errors),
            Err(error) => errors.push(format!("Directory delete worker failed: {error}")),
        }
    }
    inventory
        .dirs
        .sort_by_key(|path| std::cmp::Reverse(path.iter().filter(|byte| **byte == b'/').count()));
    let session = pool.session_for(0);
    for path in inventory.dirs {
        if let Err(error) = ignore_not_found(session.sftp.remove_dir_bytes(path).await) {
            errors.push(error.to_string());
        }
    }
    if !queue
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .is_empty()
    {
        errors.push("Directory delete workers left unprocessed files".into());
    }
    if !errors.is_empty() {
        anyhow::bail!("{} deletion error(s):\n{}", errors.len(), errors.join("\n"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
