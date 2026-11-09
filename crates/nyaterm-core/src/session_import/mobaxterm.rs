use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::{
    AiExecutionProfile, ConnectionAuth, ConnectionPasswordSource, ConnectionType,
    ConnectionUsernameSource, RdpClipboardSettings, RdpDisplaySettings, RdpReconnectSettings,
    RdpSecuritySettings, SavedPassword, SecretString, SerialFlowControl,
};

use super::{
    AppResult, PreparedSessionConnection, PreparedSessionImport, SESSION_IMPORT_MAX_BYTES,
    SessionImportError, SessionImportNotice, SessionImportNoticeKind, SessionImportOptions,
    SessionImportWarning, SessionImportWarningKind, decode_bytes, read_file_limited,
};

mod credentials;

use credentials::{EncryptedCredentials, match_encrypted};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MobaXtermCredentialDiagnosticKind {
    RegistryOnly,
    SkippedEncrypted,
    Undecryptable,
    MissingReference,
    AmbiguousMatch,
    Unmatched,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MobaXtermCredentialDiagnostic {
    pub kind: MobaXtermCredentialDiagnosticKind,
    pub count: usize,
}

#[derive(Clone, Debug, Default)]
pub struct MobaXtermImportReport {
    pub linked_credentials: usize,
    pub diagnostics: Vec<MobaXtermCredentialDiagnostic>,
}

impl MobaXtermImportReport {
    fn record(&mut self, kind: MobaXtermCredentialDiagnosticKind, count: usize) {
        if count == 0 {
            return;
        }
        if let Some(diagnostic) = self.diagnostics.iter_mut().find(|item| item.kind == kind) {
            diagnostic.count += count;
        } else {
            self.diagnostics
                .push(MobaXtermCredentialDiagnostic { kind, count });
        }
    }
}

#[derive(Debug)]
pub struct MobaXtermImportOutcome {
    pub prepared: PreparedSessionImport,
    pub report: MobaXtermImportReport,
}

const DEFAULT_SSH_PORT: u16 = 22;
const DEFAULT_TELNET_PORT: u16 = 23;
const DEFAULT_RDP_PORT: u16 = 3389;
const DEFAULT_VNC_PORT: u16 = 5900;
/// MobaXterm nests folders with a single backslash (`SubRep=Parent\Child`).
const MOBA_FOLDER_SEPARATOR: char = '\\';

/// MobaXterm stores every session in section `[Bookmarks]` or `[Bookmarks_N]`,
/// one section per folder. `SubRep` carries the folder path, so all sections
/// must be read even though only the first one is literally named `Bookmarks`.
pub(super) fn parse_mobaxterm(
    path: &Path,
    options: SessionImportOptions<'_>,
) -> AppResult<PreparedSessionImport> {
    Ok(prepare_mobaxterm_import(path, options)?.prepared)
}

pub fn prepare_mobaxterm_import(
    path: &Path,
    options: SessionImportOptions<'_>,
) -> Result<MobaXtermImportOutcome, SessionImportError> {
    let raw = read_file_limited(path, "MobaXterm session file", SESSION_IMPORT_MAX_BYTES)?;
    let content = decode_bytes(&raw);
    let stored_passwords = match options.mobaxterm_passwords_path {
        Some(passwords_path) => {
            let raw = read_file_limited(
                passwords_path,
                "MobaXterm stored passwords file",
                SESSION_IMPORT_MAX_BYTES,
            )?;
            parse_stored_passwords(&decode_bytes(&raw))
        }
        None => Vec::new(),
    };
    prepare_content(&content, &stored_passwords, &options)
}

struct MobaSection {
    name: String,
    sub_rep: Option<String>,
    entries: Vec<(String, String)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MobaProtocol {
    Ssh,
    Rdp,
    Telnet,
    Vnc,
    Serial,
}

struct MobaSession {
    name: String,
    protocol: MobaProtocol,
    host: String,
    port: u16,
    /// Login name after removing MobaXterm's `[credential]` reference brackets.
    username: String,
    /// The bracketed/plain login reference used to match stored passwords.
    /// `None` when the session has no username at all, so a default username
    /// never accidentally claims an unrelated saved password.
    login_reference: Option<String>,
    credential_reference: Option<String>,
    /// Serial device name extracted from the trailing device label.
    serial_port_name: Option<String>,
    group_path: Option<Vec<String>>,
}

struct MobaStoredPassword {
    login: String,
    username: String,
    password: SecretString,
}

impl std::fmt::Debug for MobaStoredPassword {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MobaStoredPassword")
            .field("login", &self.login)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// Reads sections in file order, unlike the shared `parse_ini_sections` map,
/// so imported connections keep the source ordering within their folder.
fn parse_sections(content: &str) -> Vec<MobaSection> {
    let mut sections: Vec<MobaSection> = Vec::new();
    let mut current: Option<usize> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            sections.push(MobaSection {
                name: line[1..line.len() - 1].trim().to_string(),
                sub_rep: None,
                entries: Vec::new(),
            });
            current = Some(sections.len() - 1);
            continue;
        }
        let Some(index) = current else {
            continue;
        };
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key.eq_ignore_ascii_case("SubRep") {
            sections[index].sub_rep = Some(value.to_string());
        }
        sections[index]
            .entries
            .push((key.to_string(), value.to_string()));
    }

    sections
}

fn parse_group_path(sub_rep: Option<&str>) -> Option<Vec<String>> {
    let segments: Vec<String> = sub_rep?
        .split(MOBA_FOLDER_SEPARATOR)
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect();
    (!segments.is_empty()).then_some(segments)
}

/// Session line format: `#<icon>#<type>%<field 1>%<field 2>%...#<theme block>`.
///
/// The first `#` after the subtype starts the terminal-theme block. SSH and RDP
/// keep a `%` before it; Telnet, VNC, and Serial glue it to their last protocol
/// field, so the protocol block must be truncated before splitting on `%`.
fn parse_session(
    name: &str,
    value: &str,
    group_path: &Option<Vec<String>>,
) -> Result<MobaSession, SessionImportNoticeKind> {
    let name = name.trim();
    if name.is_empty() {
        return Err(SessionImportNoticeKind::SkippedMalformedEntry);
    }

    let rest = value
        .strip_prefix('#')
        .ok_or(SessionImportNoticeKind::SkippedMalformedEntry)?;
    let (kind, rest) = rest
        .split_once('#')
        .ok_or(SessionImportNoticeKind::SkippedMalformedEntry)?;
    let (subtype, rest) = rest
        .split_once('%')
        .ok_or(SessionImportNoticeKind::SkippedMalformedEntry)?;
    let protocol = match (kind, subtype) {
        ("109", "0") => MobaProtocol::Ssh,
        ("91", _) => MobaProtocol::Rdp,
        ("98", _) => MobaProtocol::Telnet,
        ("128", _) => MobaProtocol::Vnc,
        ("131", _) => MobaProtocol::Serial,
        _ => return Err(SessionImportNoticeKind::SkippedUnsupportedType),
    };

    let protocol_block = rest.split_once('#').map_or(rest, |(block, _)| block);
    let fields: Vec<&str> = protocol_block.split('%').collect();

    if protocol == MobaProtocol::Serial {
        // The trailing field is the device label (for example a COM port plus
        // its Windows friendly name). Only the leading token is a usable port
        // name; fields 1..7 could not be confirmed as baud/data/parity/flow.
        let label = fields.last().map(|value| value.trim()).unwrap_or("");
        let port_name = label.split_whitespace().next().unwrap_or("");
        if port_name.is_empty() {
            return Err(SessionImportNoticeKind::SkippedMissingSerialPort);
        }
        return Ok(MobaSession {
            name: name.to_string(),
            protocol,
            host: String::new(),
            port: 0,
            username: String::new(),
            login_reference: None,
            credential_reference: None,
            serial_port_name: Some(port_name.to_string()),
            group_path: group_path.clone(),
        });
    }

    let host = fields.first().map(|value| value.trim()).unwrap_or("");
    if host.is_empty() {
        return Err(SessionImportNoticeKind::SkippedMissingEndpoint);
    }
    let port = fields
        .get(1)
        .and_then(|port| port.trim().parse::<u16>().ok())
        .filter(|port| *port > 0)
        .unwrap_or_else(|| default_port(protocol));
    let raw_username = fields.get(2).map(|value| value.trim()).unwrap_or("");
    let stripped_username = strip_login_reference(raw_username);
    let unset = stripped_username.is_empty()
        || (protocol == MobaProtocol::Vnc && stripped_username == "-1");
    let username = if unset {
        default_username(protocol).to_string()
    } else {
        stripped_username.to_string()
    };

    Ok(MobaSession {
        name: name.to_string(),
        protocol,
        host: host.to_string(),
        port,
        username,
        login_reference: (!unset).then(|| stripped_username.to_string()),
        credential_reference: (!unset
            && raw_username.starts_with('[')
            && raw_username.ends_with(']'))
        .then(|| stripped_username.to_string()),
        serial_port_name: None,
        group_path: group_path.clone(),
    })
}

fn default_port(protocol: MobaProtocol) -> u16 {
    match protocol {
        MobaProtocol::Ssh => DEFAULT_SSH_PORT,
        MobaProtocol::Rdp => DEFAULT_RDP_PORT,
        MobaProtocol::Telnet => DEFAULT_TELNET_PORT,
        MobaProtocol::Vnc => DEFAULT_VNC_PORT,
        MobaProtocol::Serial => 0,
    }
}

fn default_username(protocol: MobaProtocol) -> &'static str {
    match protocol {
        MobaProtocol::Ssh | MobaProtocol::Telnet => "root",
        MobaProtocol::Rdp | MobaProtocol::Vnc | MobaProtocol::Serial => "",
    }
}

fn strip_login_reference(value: &str) -> &str {
    value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(value)
        .trim()
}

/// Stored passwords use `<login>(<username>) = <password>`; the login can be a
/// host name or the `[credential]` reference from the session file.
fn parse_stored_passwords(content: &str) -> Vec<MobaStoredPassword> {
    let mut entries = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((left, password)) = line.split_once('=') else {
            continue;
        };
        let password = password.trim();
        if password.is_empty() {
            continue;
        }
        let left = left.trim();
        let (login, username) = match (left.rfind('('), left.ends_with(')')) {
            (Some(index), true) => (left[..index].trim(), left[index + 1..left.len() - 1].trim()),
            _ => (left, ""),
        };
        if login.is_empty() {
            continue;
        }
        entries.push(MobaStoredPassword {
            login: login.to_string(),
            username: username.to_string(),
            password: SecretString::from(password),
        });
    }

    entries
}

/// Higher scores are more specific: an explicit login reference beats a
/// session name, which beats a host-only match.
fn login_match_score(session: &MobaSession, stored: &MobaStoredPassword) -> Option<u8> {
    // Serial sessions carry no login credentials; never attach a saved
    // password by name or host.
    if session.protocol == MobaProtocol::Serial {
        return None;
    }
    let login = stored.login.trim();
    if login.is_empty() {
        return None;
    }
    if session
        .login_reference
        .as_deref()
        .is_some_and(|reference| login.eq_ignore_ascii_case(reference))
    {
        return Some(3);
    }
    if login.eq_ignore_ascii_case(session.name.trim()) {
        return Some(2);
    }
    if login.eq_ignore_ascii_case(session.host.trim()) {
        return Some(1);
    }
    None
}

#[cfg(test)]
fn parse_mobaxterm_content(
    content: &str,
    stored_passwords: &[MobaStoredPassword],
) -> PreparedSessionImport {
    prepare_content(content, stored_passwords, &SessionImportOptions::default())
        .expect("unencrypted synthetic MobaXterm sessions")
        .prepared
}

fn prepare_content(
    content: &str,
    stored_passwords: &[MobaStoredPassword],
    options: &SessionImportOptions<'_>,
) -> AppResult<MobaXtermImportOutcome> {
    let sections = parse_sections(content);
    if !sections
        .iter()
        .any(|section| is_bookmark_section(&section.name))
    {
        return Err(SessionImportError::Config(
            "This file is not a MobaXterm session or configuration file: no Bookmarks section"
                .into(),
        ));
    }
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut seen_groups: HashSet<Vec<String>> = HashSet::new();
    let mut sessions: Vec<MobaSession> = Vec::new();
    let mut notices: Vec<SessionImportNotice> = Vec::new();

    for section in &sections {
        if !is_bookmark_section(&section.name) {
            continue;
        }
        let group_path = parse_group_path(section.sub_rep.as_deref());
        if let Some(path) = &group_path
            && seen_groups.insert(path.clone())
        {
            groups.push(path.clone());
        }
        for (name, value) in &section.entries {
            if name.eq_ignore_ascii_case("SubRep") || name.eq_ignore_ascii_case("ImgNum") {
                continue;
            }
            match parse_session(name, value, &group_path) {
                Ok(session) => {
                    match session.protocol {
                        MobaProtocol::Vnc => notices.push(SessionImportNotice {
                            session_name: session.name.clone(),
                            kind: SessionImportNoticeKind::VncSettingsDefaulted,
                        }),
                        MobaProtocol::Serial => notices.push(SessionImportNotice {
                            session_name: session.name.clone(),
                            kind: SessionImportNoticeKind::SerialSettingsDefaulted,
                        }),
                        _ => {}
                    }
                    sessions.push(session);
                }
                Err(kind) if !name.trim().is_empty() => notices.push(SessionImportNotice {
                    session_name: name.trim().to_string(),
                    kind,
                }),
                Err(_) => {}
            }
        }
    }

    if sessions.is_empty() {
        return Err(SessionImportError::Config(
            "MobaXterm file contains no supported sessions with usable endpoints".into(),
        ));
    }
    let mut report = MobaXtermImportReport::default();
    let encrypted = EncryptedCredentials::read(&sections, options, &mut report)?;
    let plaintext_count = stored_passwords.len();
    let mut all_passwords: Vec<&MobaStoredPassword> = stored_passwords.iter().collect();
    all_passwords.extend(encrypted.entries.iter().map(|entry| &entry.stored));
    let mut incomplete = Vec::new();
    let matches: Vec<Option<usize>> = sessions
        .iter()
        .enumerate()
        .map(|(session_index, session)| {
            let plaintext = stored_passwords
                .iter()
                .enumerate()
                .filter_map(|(index, stored)| {
                    login_match_score(session, stored).map(|score| (score, index))
                })
                .fold(
                    None,
                    |best: Option<(u8, usize)>, (score, index)| match best {
                        Some((best_score, _)) if best_score >= score => best,
                        _ => Some((score, index)),
                    },
                )
                .map(|(_, index)| index);
            if plaintext.is_some() {
                return plaintext;
            }
            let result = match_encrypted(session, &sessions, &encrypted);
            if let Some(kind) = result.diagnostic {
                report.record(kind, 1);
            }
            if result.index.is_none()
                && session.protocol != MobaProtocol::Serial
                && (session.credential_reference.is_some()
                    || result.diagnostic.is_some()
                    || encrypted.unavailable_for(session))
            {
                incomplete.push(SessionImportWarning {
                    session_index,
                    kind: SessionImportWarningKind::UnreadableMobaXtermCredential,
                });
            }
            result.index.map(|index| plaintext_count + index)
        })
        .collect();

    let mut passwords: Vec<SavedPassword> = Vec::new();
    let mut used_accounts: HashMap<usize, (String, String)> = HashMap::new();
    let mut account_names: HashMap<String, usize> = HashMap::new();
    for stored_index in matches.iter().flatten() {
        if used_accounts.contains_key(stored_index) {
            continue;
        }
        let stored = all_passwords[*stored_index];
        let base_name = stored.login.trim();
        let occurrence = account_names
            .entry(base_name.to_string())
            .and_modify(|count| *count += 1)
            .or_insert(1);
        let name = if *occurrence == 1 {
            base_name.to_string()
        } else {
            format!("{base_name} ({occurrence})")
        };
        let id = uuid::Uuid::new_v4().to_string();
        used_accounts.insert(*stored_index, (id.clone(), stored.username.clone()));
        passwords.push(SavedPassword {
            id,
            sort_order: passwords.len() as i32,
            name,
            username: stored.username.clone(),
            password: Some(stored.password.clone()),
            has_password: false,
        });
    }

    report.linked_credentials = passwords.len();
    let matched_encrypted: HashSet<usize> = matches
        .iter()
        .flatten()
        .filter_map(|index| index.checked_sub(plaintext_count))
        .collect();
    report.record(
        MobaXtermCredentialDiagnosticKind::Unmatched,
        encrypted.entries.len() - matched_encrypted.len(),
    );

    let mut connections = Vec::with_capacity(sessions.len());
    for (sort_order, (session, stored_index)) in sessions.into_iter().zip(matches).enumerate() {
        let is_serial = session.protocol == MobaProtocol::Serial;
        let account = stored_index.and_then(|index| used_accounts.get(&index));
        let (username, auth) = match account {
            Some((account_id, account_username)) => {
                let username = if account_username.trim().is_empty() {
                    session.username.clone()
                } else {
                    account_username.trim().to_string()
                };
                (
                    username,
                    ConnectionAuth {
                        mode: "password".to_string(),
                        // SSH/Telnet resolve through `account_id`; RDP/VNC read
                        // `password_id`, and both point at the same account record.
                        password_id: Some(account_id.clone()),
                        account_id: Some(account_id.clone()),
                        password_source: Some(ConnectionPasswordSource::Account),
                        username_source: Some(ConnectionUsernameSource::Account),
                        ..ConnectionAuth::default()
                    },
                )
            }
            None => (
                encrypted
                    .referenced_username(&session)
                    .map(|username| username.trim().to_string())
                    .unwrap_or_else(|| session.username.clone()),
                ConnectionAuth {
                    mode: "password".to_string(),
                    ..ConnectionAuth::default()
                },
            ),
        };

        let config = connection_config(&session, username);
        connections.push(PreparedSessionConnection {
            saved: None,
            name: session.name,
            config,
            group_path: session.group_path,
            description: None,
            sort_order: sort_order as i32,
            icon: None,
            // Serial sessions have no login credentials.
            auth: (!is_serial).then_some(auth),
        });
    }

    Ok(MobaXtermImportOutcome {
        prepared: PreparedSessionImport {
            warnings: incomplete,
            custom_icons: Vec::new(),
            groups,
            passwords,
            ssh_keys: Vec::new(),
            connections,
            notices,
        },
        report,
    })
}

fn is_bookmark_section(name: &str) -> bool {
    name.eq_ignore_ascii_case("Bookmarks")
        || name
            .get(..10)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("Bookmarks_"))
            && name.get(10..).is_some_and(|suffix| {
                !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
            })
}

fn connection_config(session: &MobaSession, username: String) -> ConnectionType {
    match session.protocol {
        MobaProtocol::Ssh => ConnectionType::Ssh {
            host: session.host.clone(),
            port: session.port,
            username,
            backspace_mode: "del".to_string(),
            ai_execution_profile: AiExecutionProfile::Auto,
            x11_forwarding: false,
            auth_agent_endpoint: None,
            agent_forwarding_config: None,
            legacy_agent_forwarding: None,
            encoding: String::new(),
            dynamic_tab_title: false,
        },
        MobaProtocol::Rdp => ConnectionType::Rdp {
            host: session.host.clone(),
            port: session.port,
            username,
            domain: String::new(),
            backend: "builtin".to_string(),
            security: RdpSecuritySettings::default(),
            display: RdpDisplaySettings::default(),
            clipboard: RdpClipboardSettings::default(),
            reconnect: RdpReconnectSettings::default(),
        },
        MobaProtocol::Telnet => ConnectionType::Telnet {
            host: session.host.clone(),
            port: session.port,
            username,
            ai_execution_profile: AiExecutionProfile::Auto,
            backspace_mode: "del".to_string(),
            raw_tcp_cli: false,
            enter_mode: "cr".to_string(),
            local_echo: false,
            local_line_edit: false,
            force_character_at_a_time: false,
            send_naws: true,
            send_sga: true,
            keep_alive_interval_secs: crate::models::DEFAULT_TELNET_KEEP_ALIVE_INTERVAL_SECS,
            auto_login: Default::default(),
            encoding: String::new(),
        },
        MobaProtocol::Vnc => ConnectionType::Vnc {
            username,
            host: session.host.clone(),
            port: session.port,
            // The sample only confirms host/port/username. Security, display,
            // clipboard, reconnect, shared, and view-only settings were not
            // present in the exported session line.
            security: Default::default(),
            display: Default::default(),
            clipboard: Default::default(),
            reconnect: Default::default(),
            shared: true,
            view_only: false,
        },
        MobaProtocol::Serial => ConnectionType::Serial {
            port_name: session.serial_port_name.clone().unwrap_or_default(),
            // MobaXterm's numeric serial fields could not be confirmed from
            // the sample, so use the product defaults instead of guessing.
            baud_rate: 115_200,
            data_bits: 8,
            parity: "none".to_string(),
            flow_control: SerialFlowControl::None,
            stop_bits: "1".to_string(),
            ai_execution_profile: AiExecutionProfile::Auto,
            backspace_mode: "ctrl_h".to_string(),
            encoding: String::new(),
        },
    }
}

#[cfg(test)]
pub(super) fn parse_mobaxterm_content_with_passwords(
    content: &str,
    passwords_content: Option<&str>,
) -> PreparedSessionImport {
    let stored_passwords = passwords_content
        .map(parse_stored_passwords)
        .unwrap_or_default();
    parse_mobaxterm_content(content, &stored_passwords)
}

#[cfg(test)]
mod tests;
