//! Import sessions from Xshell (.xts), MobaXterm (.mxtsessions), WindTerm (.sessions),
//! SecureCRT (.xml), FinalShell conn directories, NyaTerm JSON files, Electerm
//! bookmarks, and Termius IndexedDB data.

use crate::{AiExecutionProfile, ConnectionAuth, ConnectionType, SavedPassword, SshKey};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use thiserror::Error;

mod electerm;
mod finalshell;
mod mobaxterm;
mod nyaterm_json;
mod securecrt;
mod termius;
mod windterm;
mod xshell;

use mobaxterm::parse_mobaxterm;
use windterm::parse_windterm;
use xshell::parse_xshell;

const SESSION_IMPORT_MAX_BYTES: u64 = 16 * 1024 * 1024;
const XSHELL_ARCHIVE_MAX_BYTES: u64 = 64 * 1024 * 1024;
const XSHELL_ENTRY_MAX_BYTES: u64 = 1024 * 1024;
const XSHELL_ENTRY_LIMIT: usize = 10_000;

#[derive(Debug, Error)]
pub enum SessionImportError {
    #[error("{0}")]
    Config(String),
    #[error("{0}")]
    Crypto(String),
}

type AppError = SessionImportError;
type AppResult<T> = Result<T, SessionImportError>;

struct ImportedSession {
    name: String,
    host: String,
    port: u16,
    username: String,
    auth_type: String,
    /// Hierarchical group path segments, e.g. ["Production", "Web"].
    group_path: Option<Vec<String>>,
    description: Option<String>,
}

pub struct PreparedSessionConnection {
    pub saved: Option<crate::models::sessions::SavedConnection>,
    pub name: String,
    pub config: ConnectionType,
    pub group_path: Option<Vec<String>>,
    pub description: Option<String>,
    pub sort_order: i32,
    pub icon: Option<String>,
    pub auth: Option<ConnectionAuth>,
}

pub struct PreparedSessionImport {
    pub custom_icons: Vec<crate::models::sessions::ConnectionCustomIcon>,
    pub groups: Vec<Vec<String>>,
    pub passwords: Vec<SavedPassword>,
    pub ssh_keys: Vec<SshKey>,
    pub connections: Vec<PreparedSessionConnection>,
}

type PreparedJsonConnection = PreparedSessionConnection;
type PreparedJsonImport = PreparedSessionImport;

impl std::fmt::Debug for PreparedSessionImport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedSessionImport")
            .field("group_count", &self.groups.len())
            .field("password_count", &self.passwords.len())
            .field("ssh_key_count", &self.ssh_keys.len())
            .field("connection_count", &self.connections.len())
            .finish()
    }
}

/// Detect BOM (UTF-8/UTF-16) and decode accordingly; fall back to GBK.
fn decode_bytes(raw: &[u8]) -> String {
    if let Some((enc, bom_len)) = encoding_rs::Encoding::for_bom(raw) {
        let (decoded, _, _) = enc.decode(&raw[bom_len..]);
        return decoded.into_owned();
    }
    match std::str::from_utf8(raw) {
        Ok(s) => s.to_string(),
        Err(_) => {
            let (decoded, _, _) = encoding_rs::GBK.decode(raw);
            decoded.into_owned()
        }
    }
}

fn read_file_limited(path: impl AsRef<Path>, label: &str, max_bytes: u64) -> AppResult<Vec<u8>> {
    let file = std::fs::File::open(path)
        .map_err(|error| AppError::Config(format!("Cannot open {label}: {error}")))?;
    let size = file
        .metadata()
        .map_err(|error| AppError::Config(format!("Cannot inspect {label}: {error}")))?
        .len();
    if size > max_bytes {
        return Err(AppError::Config(format!(
            "{label} exceeds the {} MiB import limit",
            max_bytes / (1024 * 1024)
        )));
    }

    let mut raw = Vec::with_capacity(size as usize);
    file.take(max_bytes + 1)
        .read_to_end(&mut raw)
        .map_err(|error| AppError::Config(format!("Cannot read {label}: {error}")))?;
    if raw.len() as u64 > max_bytes {
        return Err(AppError::Config(format!(
            "{label} exceeds the {} MiB import limit",
            max_bytes / (1024 * 1024)
        )));
    }
    Ok(raw)
}

fn read_text_file_limited(path: impl AsRef<Path>, label: &str) -> AppResult<String> {
    let raw = read_file_limited(path, label, SESSION_IMPORT_MAX_BYTES)?;
    String::from_utf8(raw)
        .map_err(|error| AppError::Config(format!("{label} is not valid UTF-8: {error}")))
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn normalize_optional_secret(value: Option<crate::SecretString>) -> Option<crate::SecretString> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else if trimmed.len() == value.len() {
            Some(value)
        } else {
            Some(trimmed.to_owned().into())
        }
    })
}

fn parse_ini_sections(content: &str) -> HashMap<String, HashMap<String, String>> {
    let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current_section = String::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            current_section = line[1..line.len() - 1].to_string();
            sections.entry(current_section.clone()).or_default();
        } else if let Some((key, value)) = line.split_once('=')
            && let Some(section) = sections.get_mut(&current_section)
        {
            section.insert(key.trim().to_string(), value.trim().to_string());
        }
    }

    sections
}

// Shared import persistence helpers

fn prepare_legacy_sessions(imported: Vec<ImportedSession>) -> PreparedSessionImport {
    let connections = imported
        .into_iter()
        .map(|session| PreparedSessionConnection {
            saved: None,
            name: session.name,
            config: ConnectionType::Ssh {
                host: session.host,
                port: session.port,
                username: session.username,
                backspace_mode: "del".to_string(),
                ai_execution_profile: AiExecutionProfile::Auto,
                x11_forwarding: false,
                auth_agent_endpoint: None,
                agent_forwarding_config: None,
                legacy_agent_forwarding: None,
                encoding: String::new(),
                dynamic_tab_title: false,
            },
            group_path: session.group_path,
            description: session.description,
            sort_order: 0,
            icon: None,
            auth: Some(ConnectionAuth {
                mode: session.auth_type,
                password_id: None,
                password: None,
                key_id: None,
                otp_id: None,
                auto_fill_otp: false,
                has_password: false,
            }),
        })
        .collect();

    PreparedSessionImport {
        custom_icons: Vec::new(),
        groups: Vec::new(),
        passwords: Vec::new(),
        ssh_keys: Vec::new(),
        connections,
    }
}

pub fn prepare_session_import(
    file_path: &Path,
) -> Result<PreparedSessionImport, SessionImportError> {
    if file_path.is_dir() {
        return Ok(prepare_legacy_sessions(finalshell::parse_finalshell(
            file_path,
        )?));
    }

    let path = file_path.to_string_lossy();
    let lower = path.to_ascii_lowercase();
    let prepared = if lower.ends_with(".xts") {
        prepare_legacy_sessions(parse_xshell(&path)?)
    } else if lower.ends_with(".mxtsessions") {
        prepare_legacy_sessions(parse_mobaxterm(&path)?)
    } else if lower.ends_with(".sessions") {
        prepare_legacy_sessions(parse_windterm(&path)?)
    } else if lower.ends_with(".xml") {
        prepare_legacy_sessions(securecrt::parse_securecrt(file_path)?)
    } else if lower.ends_with(".json") {
        electerm::parse_json_import(file_path)?
    } else {
        return Err(AppError::Config(
            "Unsupported file format. Please use .xts (Xshell), .mxtsessions (MobaXterm), .sessions (WindTerm), .xml (SecureCRT), .json (NyaTerm JSON or Electerm bookmarks), or a FinalShell conn directory."
                .to_string(),
        ));
    };

    Ok(prepared)
}

pub fn prepare_termius_session_import(
    indexed_db_path: Option<&Path>,
    local_key: &[u8],
) -> Result<PreparedSessionImport, SessionImportError> {
    termius::parse_termius_indexed_db(indexed_db_path, local_key)
}

#[cfg(test)]
mod tests;
