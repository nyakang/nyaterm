use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use zip::write::SimpleFileOptions;

use crate::{AppRuntime, RuntimeMode};

mod log_writer;

pub use log_writer::{
    DEFAULT_LOG_SHARD_BYTES, MAX_LOG_SHARD_BYTES, MIN_LOG_SHARD_BYTES, RotatingLogWriter,
    apply_log_runtime_settings, local_now, log_retention_days, log_shard_bytes,
    normalize_log_shard_bytes, set_log_retention_days, set_log_shard_bytes,
};

pub const LOG_FILE_PREFIX: &str = "nyaterm-diagnostics";
pub const LOG_FILE_SUFFIX: &str = "jsonl";
const DEFAULT_RETENTION_DAYS: u32 = 7;

#[derive(Debug, Error)]
pub enum DiagnosticsError {
    #[error("failed to create diagnostics directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to read diagnostics log directory {path}: {source}")]
    ReadLogDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to read diagnostics log file {path}: {source}")]
    ReadLogFile {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to create diagnostics archive {path}: {source}")]
    CreateArchive {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write diagnostics archive {path}: {source}")]
    WriteArchive {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write diagnostics zip entry: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("failed to serialize diagnostics payload: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiagnosticsRuntimeSnapshot {
    pub active_sessions: usize,
    pub local_sessions: usize,
    pub ssh_sessions: usize,
    pub telnet_sessions: usize,
    pub raw_tcp_sessions: usize,
    pub serial_sessions: usize,
    pub open_tunnels: usize,
    pub pending_tunnels: usize,
    pub saved_connections: usize,
    pub saved_tunnels: usize,
    pub running_transfers: usize,
    pub paused_transfers: usize,
    pub completed_transfers: usize,
    pub failed_transfers: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiagnosticsExportOptions {
    pub app_version: String,
    pub language: String,
    pub log_level: String,
    pub retention_days: u32,
    pub runtime_snapshot: DiagnosticsRuntimeSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticsExportInfo {
    pub output_path: PathBuf,
    pub log_files: usize,
    pub bytes: u64,
}

pub fn export_diagnostics_archive(
    runtime: &AppRuntime,
    options: &DiagnosticsExportOptions,
    output_path: impl AsRef<Path>,
) -> Result<DiagnosticsExportInfo, DiagnosticsError> {
    let output_path = output_path.as_ref().to_path_buf();
    if let Some(parent) = output_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|source| DiagnosticsError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::create_dir_all(runtime.log_dir()).map_err(|source| DiagnosticsError::CreateDir {
        path: runtime.log_dir().to_path_buf(),
        source,
    })?;

    let log_files = collect_log_files(
        runtime.log_dir(),
        normalize_retention_days(options.retention_days),
    )?;
    let file =
        std::fs::File::create(&output_path).map_err(|source| DiagnosticsError::CreateArchive {
            path: output_path.clone(),
            source,
        })?;
    let mut zip = zip::ZipWriter::new(file);
    let file_options =
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    for log_file in &log_files {
        write_log_file_entry(&mut zip, &output_path, log_file, file_options)?;
    }

    let manifest = json!({
        "app_version": options.app_version,
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "runtime_mode": match runtime.mode() {
            RuntimeMode::Portable => "portable",
            RuntimeMode::Installed => "installed",
        },
        "language": options.language,
        "log_level": options.log_level,
        "retention_days": normalize_retention_days(options.retention_days),
        "exported_at_ms": current_time_ms(),
    });
    zip.start_file("manifest.json", file_options)?;
    write_json_entry(&mut zip, &output_path, &manifest)?;

    let runtime_snapshot = json!({
        "directories": {
            "config": runtime.config_dir().display().to_string(),
            "data": runtime.data_dir().display().to_string(),
            "cache": runtime.cache_dir().display().to_string(),
            "logs": runtime.log_dir().display().to_string(),
        },
        "sessions": {
            "active_total": options.runtime_snapshot.active_sessions,
            "local": options.runtime_snapshot.local_sessions,
            "ssh": options.runtime_snapshot.ssh_sessions,
            "telnet": options.runtime_snapshot.telnet_sessions,
            "raw_tcp": options.runtime_snapshot.raw_tcp_sessions,
            "serial": options.runtime_snapshot.serial_sessions,
        },
        "tunnels": {
            "open": options.runtime_snapshot.open_tunnels,
            "pending": options.runtime_snapshot.pending_tunnels,
            "saved": options.runtime_snapshot.saved_tunnels,
        },
        "transfers": {
            "running": options.runtime_snapshot.running_transfers,
            "paused": options.runtime_snapshot.paused_transfers,
            "completed": options.runtime_snapshot.completed_transfers,
            "failed": options.runtime_snapshot.failed_transfers,
        },
        "configuration": {
            "saved_connections": options.runtime_snapshot.saved_connections,
        },
    });
    zip.start_file("runtime_snapshot.json", file_options)?;
    write_json_entry(&mut zip, &output_path, &runtime_snapshot)?;

    zip.finish()?;
    let bytes = std::fs::metadata(&output_path)
        .map(|metadata| metadata.len())
        .unwrap_or_default();

    Ok(DiagnosticsExportInfo {
        output_path,
        log_files: log_files.len(),
        bytes,
    })
}

fn write_json_entry<W: Write + std::io::Seek>(
    zip: &mut zip::ZipWriter<W>,
    output_path: &Path,
    value: &serde_json::Value,
) -> Result<(), DiagnosticsError> {
    let content = serde_json::to_vec_pretty(value)?;
    zip.write_all(&content)
        .map_err(|source| DiagnosticsError::WriteArchive {
            path: output_path.to_path_buf(),
            source,
        })
}

fn write_log_file_entry<W: Write + std::io::Seek>(
    zip: &mut zip::ZipWriter<W>,
    output_path: &Path,
    log_file: &Path,
    file_options: SimpleFileOptions,
) -> Result<(), DiagnosticsError> {
    let file = std::fs::File::open(log_file).map_err(|source| DiagnosticsError::ReadLogFile {
        path: log_file.to_path_buf(),
        source,
    })?;
    let file_name = log_file
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("diagnostics-log.jsonl");
    zip.start_file(format!("logs/{file_name}"), file_options)?;

    let mut reader = std::io::BufReader::new(file);
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|source| DiagnosticsError::ReadLogFile {
                path: log_file.to_path_buf(),
                source,
            })?;
        if read == 0 {
            break;
        }
        zip.write_all(&buffer[..read])
            .map_err(|source| DiagnosticsError::WriteArchive {
                path: output_path.to_path_buf(),
                source,
            })?;
    }
    Ok(())
}

fn collect_log_files(
    log_dir: &Path,
    retention_days: u32,
) -> Result<Vec<PathBuf>, DiagnosticsError> {
    let min_modified = threshold_system_time(retention_days);
    let mut files = Vec::new();
    let entries = match std::fs::read_dir(log_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(files),
        Err(source) => {
            return Err(DiagnosticsError::ReadLogDir {
                path: log_dir.to_path_buf(),
                source,
            });
        }
    };
    for entry in entries {
        let entry = entry.map_err(|source| DiagnosticsError::ReadLogDir {
            path: log_dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if !is_log_file(&path) {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if modified >= min_modified {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn is_log_file(path: &Path) -> bool {
    path.is_file()
        && path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.starts_with(LOG_FILE_PREFIX))
        && path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| extension == LOG_FILE_SUFFIX)
}

fn normalize_retention_days(retention_days: u32) -> u32 {
    match retention_days {
        0 => DEFAULT_RETENTION_DAYS,
        value => value.min(30),
    }
}

fn threshold_system_time(retention_days: u32) -> SystemTime {
    SystemTime::now()
        .checked_sub(Duration::from_secs(
            u64::from(retention_days) * 24 * 60 * 60,
        ))
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
