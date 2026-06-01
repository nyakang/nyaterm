//! Import sessions from Xshell (.xts), MobaXterm (.mxtsessions), WindTerm (.sessions),
//! and NyaTerm JSON files.

use crate::config::{
    self, AiExecutionProfile, ConnectionAuth, ConnectionType, Group, SavedConnection,
};
use crate::error::{AppError, AppResult};
use crate::utils::crypto;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use tauri::Emitter;

struct ImportedSession {
    name: String,
    host: String,
    port: u16,
    username: String,
    auth_type: String,
    /// Hierarchical group path segments, e.g. ["亚鸿", "湖北"].
    group_path: Option<Vec<String>>,
}

#[derive(Debug)]
struct PreparedJsonConnection {
    name: String,
    config: ConnectionType,
    group_path: Option<Vec<String>>,
    description: Option<String>,
    sort_order: i32,
    icon: Option<String>,
    auth: Option<ConnectionAuth>,
}

#[derive(Debug)]
struct PreparedJsonImport {
    groups: Vec<Vec<String>>,
    passwords: Vec<config::SavedPassword>,
    ssh_keys: Vec<config::SshKey>,
    connections: Vec<PreparedJsonConnection>,
}

#[derive(Debug, Deserialize)]
struct NyatermJsonImportFile {
    #[serde(default = "default_import_version")]
    version: u32,
    #[serde(default)]
    passwords: Vec<NyatermJsonPassword>,
    #[serde(default)]
    ssh_keys: Vec<NyatermJsonSshKey>,
    #[serde(default)]
    groups: Vec<NyatermJsonGroup>,
    #[serde(default)]
    sessions: Vec<NyatermJsonSession>,
}

fn default_import_version() -> u32 {
    1
}

#[derive(Debug, Deserialize)]
struct NyatermJsonPassword {
    #[serde(rename = "ref")]
    ref_name: String,
    name: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct NyatermJsonSshKey {
    #[serde(rename = "ref")]
    ref_name: String,
    name: String,
    private_key: String,
    #[serde(default)]
    passphrase: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NyatermJsonGroup {
    path: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct NyatermJsonSshAuth {
    mode: String,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    password_ref: Option<String>,
    #[serde(default)]
    key_ref: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum NyatermJsonSession {
    Ssh {
        name: String,
        #[serde(default)]
        group_path: Vec<String>,
        host: String,
        #[serde(default = "default_ssh_port")]
        port: u16,
        #[serde(default = "default_ssh_user")]
        username: String,
        #[serde(default)]
        auth: Option<NyatermJsonSshAuth>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        sort_order: i32,
        #[serde(default)]
        icon: Option<String>,
    },
    LocalTerminal {
        name: String,
        #[serde(default)]
        group_path: Vec<String>,
        #[serde(default)]
        shell_path: String,
        #[serde(default)]
        working_dir: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        sort_order: i32,
        #[serde(default)]
        icon: Option<String>,
    },
    Telnet {
        name: String,
        #[serde(default)]
        group_path: Vec<String>,
        host: String,
        #[serde(default = "default_telnet_port")]
        port: u16,
        #[serde(default = "default_telnet_backspace_mode")]
        backspace_mode: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        sort_order: i32,
        #[serde(default)]
        icon: Option<String>,
    },
    Serial {
        name: String,
        #[serde(default)]
        group_path: Vec<String>,
        port_name: String,
        #[serde(default = "default_serial_baud_rate")]
        baud_rate: u32,
        #[serde(default = "default_serial_data_bits")]
        data_bits: u8,
        #[serde(default = "default_serial_parity")]
        parity: String,
        #[serde(default = "default_serial_stop_bits")]
        stop_bits: String,
        #[serde(default = "default_serial_backspace_mode")]
        backspace_mode: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        sort_order: i32,
        #[serde(default)]
        icon: Option<String>,
    },
}

fn default_ssh_port() -> u16 {
    22
}

fn default_ssh_user() -> String {
    "root".to_string()
}

fn default_telnet_port() -> u16 {
    23
}

fn default_telnet_backspace_mode() -> String {
    "del".to_string()
}

fn default_serial_baud_rate() -> u32 {
    115_200
}

fn default_serial_data_bits() -> u8 {
    8
}

fn default_serial_parity() -> String {
    "none".to_string()
}

fn default_serial_stop_bits() -> String {
    "1".to_string()
}

fn default_serial_backspace_mode() -> String {
    "ctrl_h".to_string()
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

// ── Xshell (.xts) ──────────────────────────────────────────────────────────

fn parse_xshell(path: &str) -> AppResult<Vec<ImportedSession>> {
    let file = std::fs::File::open(path)
        .map_err(|e| AppError::Config(format!("Cannot open file: {e}")))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| AppError::Config(format!("Invalid ZIP/XTS file: {e}")))?;

    let mut sessions = Vec::new();

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| AppError::Config(format!("ZIP entry error: {e}")))?;

        // ZIP filenames on Chinese Windows are typically GBK-encoded
        let entry_path_raw = entry.name_raw().to_vec();
        let entry_path = decode_bytes(&entry_path_raw);
        if !entry_path.ends_with(".xsh") {
            continue;
        }

        let mut raw = Vec::new();
        entry
            .read_to_end(&mut raw)
            .map_err(|e| AppError::Config(format!("Failed to read {entry_path}: {e}")))?;

        let content = decode_bytes(&raw);

        if let Some(sess) = parse_xsh_content(&content, &entry_path) {
            sessions.push(sess);
        }
    }

    Ok(sessions)
}

fn parse_xsh_content(content: &str, entry_path: &str) -> Option<ImportedSession> {
    let sections = parse_ini_sections(content);

    let conn = sections.get("CONNECTION")?;
    let protocol = conn.get("Protocol").map(String::as_str).unwrap_or("");
    if !protocol.eq_ignore_ascii_case("SSH") {
        return None;
    }

    let host = conn.get("Host")?.clone();
    if host.is_empty() {
        return None;
    }

    let port: u16 = conn.get("Port").and_then(|p| p.parse().ok()).unwrap_or(22);

    let auth = sections.get("CONNECTION:AUTHENTICATION");
    let username = auth
        .and_then(|a| a.get("UserName"))
        .cloned()
        .unwrap_or_else(|| "root".to_string());

    let has_user_key = auth
        .and_then(|a| a.get("UserKey"))
        .is_some_and(|k| !k.is_empty());
    let auth_type = if has_user_key { "key" } else { "password" }.to_string();

    let path_obj = Path::new(entry_path);
    let name = path_obj
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Unnamed")
        .to_string();

    let group_path = path_obj.parent().and_then(|p| {
        let p_str = p.to_str().unwrap_or("");
        let stripped = p_str
            .strip_prefix("Xshell/Sessions/")
            .or_else(|| p_str.strip_prefix("Xshell/"))
            .unwrap_or(p_str);
        if stripped.is_empty() {
            None
        } else {
            let segments: Vec<String> = stripped
                .split('/')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            if segments.is_empty() {
                None
            } else {
                Some(segments)
            }
        }
    });

    Some(ImportedSession {
        name,
        host,
        port,
        username,
        auth_type,
        group_path,
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
        } else if let Some((key, value)) = line.split_once('=') {
            if let Some(section) = sections.get_mut(&current_section) {
                section.insert(key.trim().to_string(), value.trim().to_string());
            }
        }
    }

    sections
}

// ── MobaXterm (.mxtsessions) ────────────────────────────────────────────────

fn parse_mobaxterm(path: &str) -> AppResult<Vec<ImportedSession>> {
    let raw =
        std::fs::read(path).map_err(|e| AppError::Config(format!("Cannot read file: {e}")))?;
    let content = decode_bytes(&raw);

    let sections = parse_ini_sections(&content);
    let mut sessions = Vec::new();

    for (section_name, entries) in &sections {
        if !section_name.starts_with("Bookmarks") {
            continue;
        }

        let group_path = entries.get("SubRep").and_then(|s| {
            let s = s.trim();
            if s.is_empty() {
                None
            } else {
                let segments: Vec<String> = s
                    .split('\\')
                    .filter(|seg| !seg.is_empty())
                    .map(|seg| seg.trim().to_string())
                    .collect();
                if segments.is_empty() {
                    None
                } else {
                    Some(segments)
                }
            }
        });

        for (entry_name, value) in entries {
            if entry_name == "SubRep" || entry_name == "ImgNum" {
                continue;
            }

            if let Some(sess) = parse_moba_entry(entry_name, value, &group_path) {
                sessions.push(sess);
            }
        }
    }

    Ok(sessions)
}

fn parse_moba_entry(
    name: &str,
    value: &str,
    group_path: &Option<Vec<String>>,
) -> Option<ImportedSession> {
    // Format: #<type>#<subtype>%host%port%username%...
    let hash_parts: Vec<&str> = value.splitn(2, '#').skip(1).collect::<Vec<_>>();
    if hash_parts.is_empty() {
        return None;
    }

    let after_hash = hash_parts.join("#");
    let type_and_rest: Vec<&str> = after_hash.splitn(2, '%').collect();
    if type_and_rest.len() < 2 {
        return None;
    }

    // Type 109 = SSH
    let type_marker = type_and_rest[0];
    if !type_marker.starts_with("109") {
        return None;
    }

    let fields: Vec<&str> = type_and_rest[1].split('%').collect();
    if fields.len() < 3 {
        return None;
    }

    let host = fields[0].to_string();
    if host.is_empty() {
        return None;
    }

    let port: u16 = fields[1].parse().unwrap_or(22);
    let username = if fields[2].is_empty() {
        "root".to_string()
    } else {
        fields[2].to_string()
    };

    Some(ImportedSession {
        name: name.to_string(),
        host,
        port,
        username,
        auth_type: "password".to_string(),
        group_path: group_path.clone(),
    })
}

// ── WindTerm (user.sessions) ────────────────────────────────────────────────

fn parse_windterm(path: &str) -> AppResult<Vec<ImportedSession>> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| AppError::Config(format!("Cannot read file: {e}")))?;

    let entries: Vec<serde_json::Value> = serde_json::from_str(&content)
        .map_err(|e| AppError::Config(format!("Invalid WindTerm JSON: {e}")))?;

    let mut sessions = Vec::new();

    for entry in &entries {
        let protocol = entry
            .get("session.protocol")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !protocol.eq_ignore_ascii_case("SSH") {
            continue;
        }

        let host = entry
            .get("session.target")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if host.is_empty() {
            continue;
        }

        let name = entry
            .get("session.label")
            .and_then(|v| v.as_str())
            .unwrap_or(&host)
            .to_string();

        let port: u16 = entry
            .get("session.port")
            .and_then(|v| v.as_u64())
            .map_or(22, |p| p as u16);

        let group_path = entry
            .get("session.group")
            .and_then(|v| v.as_str())
            .and_then(|s| {
                let s = s.trim();
                if s.is_empty() {
                    None
                } else {
                    let segments: Vec<String> = s
                        .split('>')
                        .filter(|seg| !seg.is_empty())
                        .map(|seg| seg.trim().to_string())
                        .collect();
                    if segments.is_empty() {
                        None
                    } else {
                        Some(segments)
                    }
                }
            });

        sessions.push(ImportedSession {
            name,
            host,
            port,
            username: "root".to_string(),
            auth_type: "password".to_string(),
            group_path,
        });
    }

    Ok(sessions)
}

// ── NyaTerm JSON (.json) ───────────────────────────────────────────────────

fn parse_nyaterm_json(path: &str) -> AppResult<PreparedJsonImport> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| AppError::Config(format!("Cannot read file: {e}")))?;
    parse_nyaterm_json_content(&content)
}

fn parse_nyaterm_json_content(content: &str) -> AppResult<PreparedJsonImport> {
    let file: NyatermJsonImportFile = serde_json::from_str(content)
        .map_err(|e| AppError::Config(format!("Invalid NyaTerm JSON: {e}")))?;
    prepare_nyaterm_json_import(file)
}

fn prepare_nyaterm_json_import(file: NyatermJsonImportFile) -> AppResult<PreparedJsonImport> {
    if file.version != 1 {
        return Err(AppError::Config(format!(
            "Unsupported NyaTerm JSON import version: {}",
            file.version
        )));
    }

    let mut password_ref_map: HashMap<String, String> = HashMap::new();
    let mut passwords = Vec::new();
    for entry in file.passwords {
        let ref_name = required_string(entry.ref_name, "password ref", "passwords")?;
        if password_ref_map.contains_key(&ref_name) {
            return Err(AppError::Config(format!(
                "Duplicate password ref in import file: {ref_name}"
            )));
        }
        if entry.password.is_empty() {
            return Err(AppError::Config(format!(
                "Password entry '{ref_name}' cannot have an empty password"
            )));
        }

        let id = uuid::Uuid::new_v4().to_string();
        password_ref_map.insert(ref_name, id.clone());
        passwords.push(config::SavedPassword {
            id,
            name: required_string(entry.name, "password name", "passwords")?,
            password: Some(crypto::encrypt(&entry.password)?),
            has_password: false,
        });
    }

    let mut key_ref_map: HashMap<String, String> = HashMap::new();
    let mut ssh_keys = Vec::new();
    for entry in file.ssh_keys {
        let ref_name = required_string(entry.ref_name, "ssh key ref", "ssh_keys")?;
        if key_ref_map.contains_key(&ref_name) {
            return Err(AppError::Config(format!(
                "Duplicate ssh key ref in import file: {ref_name}"
            )));
        }
        if entry.private_key.trim().is_empty() {
            return Err(AppError::Config(format!(
                "SSH key entry '{ref_name}' cannot have empty private_key"
            )));
        }

        let id = uuid::Uuid::new_v4().to_string();
        key_ref_map.insert(ref_name, id.clone());
        ssh_keys.push(config::SshKey {
            id,
            name: required_string(entry.name, "ssh key name", "ssh_keys")?,
            key: Some(crypto::encrypt(&entry.private_key)?),
            passphrase: encrypt_optional_secret(entry.passphrase)?,
            key_file_path: None,
            has_key_data: false,
        });
    }

    let mut groups = Vec::new();
    for group in file.groups {
        let path = normalize_required_group_path(group.path, "groups.path")?;
        if !groups.contains(&path) {
            groups.push(path);
        }
    }

    let mut connections = Vec::new();
    for session in file.sessions {
        connections.push(prepare_nyaterm_json_session(
            session,
            &password_ref_map,
            &key_ref_map,
        )?);
    }

    Ok(PreparedJsonImport {
        groups,
        passwords,
        ssh_keys,
        connections,
    })
}

fn prepare_nyaterm_json_session(
    session: NyatermJsonSession,
    password_ref_map: &HashMap<String, String>,
    key_ref_map: &HashMap<String, String>,
) -> AppResult<PreparedJsonConnection> {
    match session {
        NyatermJsonSession::Ssh {
            name,
            group_path,
            host,
            port,
            username,
            auth,
            description,
            sort_order,
            icon,
        } => {
            validate_port(port, "ssh session")?;
            let context = format!("ssh session '{name}'");
            Ok(PreparedJsonConnection {
                name: required_string(name, "name", "ssh session")?,
                config: ConnectionType::Ssh {
                    host: required_string(host, "host", &context)?,
                    port,
                    username: required_string(username, "username", &context)?,
                },
                group_path: normalize_optional_group_path(group_path, &context)?,
                description: normalize_optional_string(description),
                sort_order,
                icon: normalize_optional_string(icon),
                auth: Some(prepare_json_ssh_auth(
                    auth,
                    password_ref_map,
                    key_ref_map,
                    &context,
                )?),
            })
        }
        NyatermJsonSession::LocalTerminal {
            name,
            group_path,
            shell_path,
            working_dir,
            description,
            sort_order,
            icon,
        } => {
            let context = format!("local_terminal session '{name}'");
            Ok(PreparedJsonConnection {
                name: required_string(name, "name", "local_terminal session")?,
                config: ConnectionType::LocalTerminal {
                    shell_path,
                    working_dir: normalize_optional_string(working_dir),
                    ai_execution_profile: AiExecutionProfile::Auto,
                },
                group_path: normalize_optional_group_path(group_path, &context)?,
                description: normalize_optional_string(description),
                sort_order,
                icon: normalize_optional_string(icon),
                auth: None,
            })
        }
        NyatermJsonSession::Telnet {
            name,
            group_path,
            host,
            port,
            backspace_mode,
            description,
            sort_order,
            icon,
        } => {
            validate_port(port, "telnet session")?;
            validate_backspace_mode(&backspace_mode, "telnet session")?;
            let context = format!("telnet session '{name}'");
            Ok(PreparedJsonConnection {
                name: required_string(name, "name", "telnet session")?,
                config: ConnectionType::Telnet {
                    host: required_string(host, "host", &context)?,
                    port,
                    ai_execution_profile: AiExecutionProfile::Auto,
                    backspace_mode,
                },
                group_path: normalize_optional_group_path(group_path, &context)?,
                description: normalize_optional_string(description),
                sort_order,
                icon: normalize_optional_string(icon),
                auth: None,
            })
        }
        NyatermJsonSession::Serial {
            name,
            group_path,
            port_name,
            baud_rate,
            data_bits,
            parity,
            stop_bits,
            backspace_mode,
            description,
            sort_order,
            icon,
        } => {
            validate_serial_config(baud_rate, data_bits, &parity, &stop_bits, &backspace_mode)?;
            let context = format!("serial session '{name}'");
            Ok(PreparedJsonConnection {
                name: required_string(name, "name", "serial session")?,
                config: ConnectionType::Serial {
                    port_name: required_string(port_name, "port_name", &context)?,
                    baud_rate,
                    data_bits,
                    parity,
                    stop_bits,
                    ai_execution_profile: AiExecutionProfile::Auto,
                    backspace_mode,
                },
                group_path: normalize_optional_group_path(group_path, &context)?,
                description: normalize_optional_string(description),
                sort_order,
                icon: normalize_optional_string(icon),
                auth: None,
            })
        }
    }
}

fn prepare_json_ssh_auth(
    auth: Option<NyatermJsonSshAuth>,
    password_ref_map: &HashMap<String, String>,
    key_ref_map: &HashMap<String, String>,
    context: &str,
) -> AppResult<ConnectionAuth> {
    let Some(auth) = auth else {
        return Ok(ConnectionAuth {
            mode: "none".to_string(),
            password_id: None,
            password: None,
            key_id: None,
            otp_id: None,
            auto_fill_otp: false,
            has_password: false,
        });
    };

    match auth.mode.trim() {
        "none" => {
            if auth.password.is_some() || auth.password_ref.is_some() || auth.key_ref.is_some() {
                return Err(AppError::Config(format!(
                    "{context}: auth.mode 'none' cannot include password, password_ref, or key_ref"
                )));
            }
            Ok(ConnectionAuth {
                mode: "none".to_string(),
                password_id: None,
                password: None,
                key_id: None,
                otp_id: None,
                auto_fill_otp: false,
                has_password: false,
            })
        }
        "password" => {
            let has_password = auth
                .password
                .as_ref()
                .is_some_and(|value| !value.is_empty());
            let password_ref = normalize_optional_string(auth.password_ref);
            if has_password == password_ref.is_some() {
                return Err(AppError::Config(format!(
                    "{context}: password auth must include exactly one of password or password_ref"
                )));
            }
            let password_id = if let Some(ref_name) = password_ref {
                Some(password_ref_map.get(&ref_name).cloned().ok_or_else(|| {
                    AppError::Config(format!(
                        "{context}: password_ref '{ref_name}' was not found"
                    ))
                })?)
            } else {
                None
            };
            let password = auth
                .password
                .map(|plain| crypto::encrypt(&plain))
                .transpose()?;

            Ok(ConnectionAuth {
                mode: "password".to_string(),
                password_id,
                password,
                key_id: None,
                otp_id: None,
                auto_fill_otp: false,
                has_password: false,
            })
        }
        "key" => {
            if auth.password.is_some() || auth.password_ref.is_some() {
                return Err(AppError::Config(format!(
                    "{context}: key auth cannot include password or password_ref"
                )));
            }
            let key_ref = normalize_optional_string(auth.key_ref)
                .ok_or_else(|| AppError::Config(format!("{context}: key auth requires key_ref")))?;
            let key_id = key_ref_map.get(&key_ref).cloned().ok_or_else(|| {
                AppError::Config(format!("{context}: key_ref '{key_ref}' was not found"))
            })?;

            Ok(ConnectionAuth {
                mode: "key".to_string(),
                password_id: None,
                password: None,
                key_id: Some(key_id),
                otp_id: None,
                auto_fill_otp: false,
                has_password: false,
            })
        }
        mode => Err(AppError::Config(format!(
            "{context}: unsupported SSH auth mode '{mode}'"
        ))),
    }
}

fn required_string(value: String, field: &str, context: &str) -> AppResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::Config(format!("{context}: {field} is required")));
    }
    Ok(trimmed.to_string())
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

fn encrypt_optional_secret(value: Option<String>) -> AppResult<Option<String>> {
    value
        .filter(|value| !value.is_empty())
        .map(|value| crypto::encrypt(&value))
        .transpose()
}

fn normalize_required_group_path(path: Vec<String>, context: &str) -> AppResult<Vec<String>> {
    normalize_optional_group_path(path, context)?.ok_or_else(|| {
        AppError::Config(format!(
            "{context}: group path must contain at least one segment"
        ))
    })
}

fn normalize_optional_group_path(
    path: Vec<String>,
    context: &str,
) -> AppResult<Option<Vec<String>>> {
    if path.is_empty() {
        return Ok(None);
    }

    let mut segments = Vec::new();
    for segment in path {
        let trimmed = segment.trim();
        if trimmed.is_empty() {
            return Err(AppError::Config(format!(
                "{context}: group path segments cannot be empty"
            )));
        }
        segments.push(trimmed.to_string());
    }
    Ok(Some(segments))
}

fn validate_port(port: u16, context: &str) -> AppResult<()> {
    if port == 0 {
        return Err(AppError::Config(format!(
            "{context}: port must be between 1 and 65535"
        )));
    }
    Ok(())
}

fn validate_backspace_mode(value: &str, context: &str) -> AppResult<()> {
    match value {
        "ctrl_h" | "del" => Ok(()),
        _ => Err(AppError::Config(format!(
            "{context}: backspace_mode must be 'ctrl_h' or 'del'"
        ))),
    }
}

fn validate_serial_config(
    baud_rate: u32,
    data_bits: u8,
    parity: &str,
    stop_bits: &str,
    backspace_mode: &str,
) -> AppResult<()> {
    if baud_rate == 0 {
        return Err(AppError::Config(
            "serial session: baud_rate must be greater than 0".to_string(),
        ));
    }
    if !(5..=8).contains(&data_bits) {
        return Err(AppError::Config(
            "serial session: data_bits must be between 5 and 8".to_string(),
        ));
    }
    match parity {
        "none" | "even" | "odd" => {}
        _ => {
            return Err(AppError::Config(
                "serial session: parity must be 'none', 'even', or 'odd'".to_string(),
            ));
        }
    }
    match stop_bits {
        "1" | "1.5" | "2" => {}
        _ => {
            return Err(AppError::Config(
                "serial session: stop_bits must be '1', '1.5', or '2'".to_string(),
            ));
        }
    }
    validate_backspace_mode(backspace_mode, "serial session")
}

// ── Shared import persistence helpers ──────────────────────────────────────

fn build_group_path(groups: &[Group], id: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = id;
    loop {
        if let Some(g) = groups.iter().find(|g| g.id == current) {
            segments.push(g.name.clone());
            if let Some(ref pid) = g.parent_id {
                current = pid;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    segments.reverse();
    segments
}

fn build_group_path_map(groups: &[Group]) -> HashMap<Vec<String>, String> {
    let mut path_map = HashMap::new();
    for group in groups {
        let path = build_group_path(groups, &group.id);
        path_map.insert(path, group.id.clone());
    }
    path_map
}

fn ensure_group_path(
    cfg: &mut config::AppConfig,
    path_map: &mut HashMap<Vec<String>, String>,
    next_sort: &mut i32,
    segments: &[String],
) -> Option<String> {
    if segments.is_empty() {
        return None;
    }

    let mut leaf_id = String::new();
    for depth in 1..=segments.len() {
        let prefix: Vec<String> = segments[..depth].to_vec();
        if let Some(existing) = path_map.get(&prefix) {
            leaf_id = existing.clone();
        } else {
            let id = uuid::Uuid::new_v4().to_string();
            let parent_id = if depth > 1 {
                let parent_prefix: Vec<String> = segments[..depth - 1].to_vec();
                path_map.get(&parent_prefix).cloned()
            } else {
                None
            };
            cfg.groups.push(Group {
                id: id.clone(),
                name: segments[depth - 1].clone(),
                parent_id,
                sort_order: *next_sort,
                created_at_ms: None,
                updated_at_ms: None,
            });
            *next_sort += 1;
            path_map.insert(prefix, id.clone());
            leaf_id = id;
        }
    }
    Some(leaf_id)
}

fn import_legacy_sessions(
    app: &tauri::AppHandle,
    imported: Vec<ImportedSession>,
) -> AppResult<usize> {
    if imported.is_empty() {
        return Ok(0);
    }

    let mut cfg = config::load_config(app)?;
    let count = imported.len();
    let mut path_map = build_group_path_map(&cfg.groups);
    let mut next_sort = cfg.groups.iter().map(|g| g.sort_order).max().unwrap_or(0) + 1;

    for sess in imported {
        let group_id = sess.group_path.as_ref().and_then(|segments| {
            ensure_group_path(&mut cfg, &mut path_map, &mut next_sort, segments)
        });

        cfg.connections.push(SavedConnection {
            id: uuid::Uuid::new_v4().to_string(),
            name: sess.name,
            config: ConnectionType::Ssh {
                host: sess.host,
                port: sess.port,
                username: sess.username,
            },
            group_id,
            description: None,
            sort_order: 0,
            icon: None,
            auth: Some(ConnectionAuth {
                mode: sess.auth_type,
                password_id: None,
                password: None,
                key_id: None,
                otp_id: None,
                auto_fill_otp: false,
                has_password: false,
            }),
            network: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        });
    }

    config::save_config(app, &cfg)?;
    Ok(count)
}

fn import_prepared_nyaterm_json(
    app: &tauri::AppHandle,
    prepared: PreparedJsonImport,
) -> AppResult<usize> {
    if prepared.connections.is_empty() {
        return Ok(0);
    }

    let mut cfg = config::load_config(app)?;
    let mut path_map = build_group_path_map(&cfg.groups);
    let mut next_sort = cfg.groups.iter().map(|g| g.sort_order).max().unwrap_or(0) + 1;

    for group_path in &prepared.groups {
        ensure_group_path(&mut cfg, &mut path_map, &mut next_sort, group_path);
    }

    let count = prepared.connections.len();
    for conn in prepared.connections {
        let group_id = conn.group_path.as_ref().and_then(|segments| {
            ensure_group_path(&mut cfg, &mut path_map, &mut next_sort, segments)
        });

        cfg.connections.push(SavedConnection {
            id: uuid::Uuid::new_v4().to_string(),
            name: conn.name,
            config: conn.config,
            group_id,
            description: conn.description,
            sort_order: conn.sort_order,
            icon: conn.icon,
            auth: conn.auth,
            network: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        });
    }

    let mut passwords = config::load_passwords(app)?;
    passwords.passwords.extend(prepared.passwords);
    config::save_passwords(app, &passwords)?;

    let mut keys = config::load_keys(app)?;
    keys.keys.extend(prepared.ssh_keys);
    config::save_keys(app, &keys)?;

    config::save_config(app, &cfg)?;
    Ok(count)
}

// ── Tauri Command ───────────────────────────────────────────────────────────

pub fn import_sessions(app: tauri::AppHandle, file_path: String) -> AppResult<usize> {
    let lower = file_path.to_lowercase();
    let count = if lower.ends_with(".xts") {
        import_legacy_sessions(&app, parse_xshell(&file_path)?)?
    } else if lower.ends_with(".mxtsessions") {
        import_legacy_sessions(&app, parse_mobaxterm(&file_path)?)?
    } else if lower.ends_with(".sessions") {
        import_legacy_sessions(&app, parse_windterm(&file_path)?)?
    } else if lower.ends_with(".json") {
        import_prepared_nyaterm_json(&app, parse_nyaterm_json(&file_path)?)?
    } else {
        return Err(AppError::Config(
            "Unsupported file format. Please use .xts (Xshell), .mxtsessions (MobaXterm), .sessions (WindTerm), or .json (NyaTerm JSON)."
                .to_string(),
        ));
    };

    if count > 0 {
        let _ = app.emit("connections-changed", ());
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_JSON: &str = r#"
{
  "version": 1,
  "passwords": [
    { "ref": "prod-root-password", "name": "Prod root password", "password": "replace-me" }
  ],
  "ssh_keys": [
    {
      "ref": "ops-ed25519",
      "name": "Ops ED25519",
      "private_key": "-----BEGIN OPENSSH PRIVATE KEY-----\n...\n-----END OPENSSH PRIVATE KEY-----",
      "passphrase": "optional-passphrase"
    }
  ],
  "groups": [
    { "path": ["Production"] },
    { "path": ["Production", "Web"] },
    { "path": ["Lab"] }
  ],
  "sessions": [
    {
      "name": "Prod web direct password",
      "type": "ssh",
      "group_path": ["Production", "Web"],
      "host": "web-01.example.com",
      "port": 22,
      "username": "deploy",
      "auth": { "mode": "password", "password": "replace-me" }
    },
    {
      "name": "Prod db saved password",
      "type": "ssh",
      "group_path": ["Production", "Database"],
      "host": "db-01.example.com",
      "username": "root",
      "auth": { "mode": "password", "password_ref": "prod-root-password" }
    },
    {
      "name": "Bastion saved key",
      "type": "ssh",
      "group_path": ["Production"],
      "host": "bastion.example.com",
      "username": "ops",
      "auth": { "mode": "key", "key_ref": "ops-ed25519" }
    },
    {
      "name": "Lab router",
      "type": "telnet",
      "group_path": ["Lab"],
      "host": "192.168.10.1",
      "port": 23,
      "backspace_mode": "del"
    },
    {
      "name": "USB console",
      "type": "serial",
      "group_path": ["Lab"],
      "port_name": "COM3",
      "baud_rate": 115200,
      "data_bits": 8,
      "parity": "none",
      "stop_bits": "1",
      "backspace_mode": "ctrl_h"
    },
    {
      "name": "Local PowerShell",
      "type": "local_terminal",
      "shell_path": "pwsh.exe",
      "working_dir": "C:\\Users\\me"
    }
  ]
}
"#;

    #[test]
    fn nyaterm_json_sample_import_prepares_supported_shapes() {
        crate::utils::crypto::set_master_password(None);

        let prepared = parse_nyaterm_json_content(SAMPLE_JSON).expect("parse sample");

        assert_eq!(prepared.groups.len(), 3);
        assert_eq!(prepared.passwords.len(), 1);
        assert_eq!(prepared.ssh_keys.len(), 1);
        assert_eq!(prepared.connections.len(), 6);
        assert_ne!(
            prepared.passwords[0].password.as_deref(),
            Some("replace-me")
        );
        assert_ne!(
            prepared.ssh_keys[0].key.as_deref(),
            Some("-----BEGIN OPENSSH PRIVATE KEY-----\n...\n-----END OPENSSH PRIVATE KEY-----")
        );

        let direct_auth = prepared.connections[0].auth.as_ref().expect("direct auth");
        assert_eq!(direct_auth.mode, "password");
        assert!(direct_auth.password_id.is_none());
        assert_ne!(direct_auth.password.as_deref(), Some("replace-me"));

        let saved_password_auth = prepared.connections[1]
            .auth
            .as_ref()
            .expect("saved password auth");
        assert_eq!(saved_password_auth.mode, "password");
        assert!(saved_password_auth.password_id.is_some());
        assert!(saved_password_auth.password.is_none());

        let key_auth = prepared.connections[2].auth.as_ref().expect("key auth");
        assert_eq!(key_auth.mode, "key");
        assert!(key_auth.key_id.is_some());
    }

    #[test]
    fn nyaterm_json_rejects_duplicate_password_refs() {
        let json = r#"
{
  "version": 1,
  "passwords": [
    { "ref": "dup", "name": "One", "password": "a" },
    { "ref": "dup", "name": "Two", "password": "b" }
  ],
  "sessions": []
}
"#;

        let error = parse_nyaterm_json_content(json).unwrap_err();
        assert!(error.to_string().contains("Duplicate password ref"));
    }

    #[test]
    fn nyaterm_json_rejects_missing_password_refs() {
        let json = r#"
{
  "version": 1,
  "sessions": [
    {
      "name": "Missing password",
      "type": "ssh",
      "host": "example.com",
      "username": "root",
      "auth": { "mode": "password", "password_ref": "missing" }
    }
  ]
}
"#;

        let error = parse_nyaterm_json_content(json).unwrap_err();
        assert!(error
            .to_string()
            .contains("password_ref 'missing' was not found"));
    }

    #[test]
    fn nyaterm_json_rejects_invalid_ports() {
        let json = r#"
{
  "version": 1,
  "sessions": [
    {
      "name": "Bad port",
      "type": "ssh",
      "host": "example.com",
      "port": 0,
      "username": "root",
      "auth": { "mode": "none" }
    }
  ]
}
"#;

        let error = parse_nyaterm_json_content(json).unwrap_err();
        assert!(error
            .to_string()
            .contains("port must be between 1 and 65535"));
    }
}
