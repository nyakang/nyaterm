use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use aes::Aes256;
use base64::Engine as _;
use cbc::cipher::{BlockDecryptMut as _, KeyIvInit as _, block_padding::Pkcs7};
use pbkdf2::pbkdf2_hmac;
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;
use sha3::Sha3_512;
use zeroize::Zeroizing;

use crate::{
    AiExecutionProfile, ConnectionAuth, ConnectionPasswordSource, ConnectionType,
    ConnectionUsernameSource, SavedPassword, SecretString, SshKey,
};

use super::{
    AppError, AppResult, PreparedSessionConnection, PreparedSessionImport, SessionImportWarning,
    SessionImportWarningKind, read_text_file_limited,
};

const PBKDF2_ITERATIONS: u32 = 100_000;
/// Name WindTerm writes next to `user.sessions`. Lookup is case-insensitive so
/// variants like `OneKeys.config` on Windows still resolve, but this spelling is
/// what diagnostics and the UI report.
const ONE_KEYS_CONFIG_FILE: &str = "onekeys.config";

struct WindtermCrypto(Zeroizing<[u8; 48]>);

enum ProfileCrypto {
    Ready(WindtermCrypto),
    Unavailable(SessionImportWarningKind),
}

#[derive(Deserialize)]
struct WindtermSession {
    #[serde(
        deserialize_with = "default_field",
        rename = "session.protocol",
        default
    )]
    protocol: String,
    #[serde(deserialize_with = "default_field", rename = "session.target", default)]
    target: String,
    #[serde(deserialize_with = "default_field", rename = "session.label", default)]
    label: Option<String>,
    #[serde(deserialize_with = "default_field", rename = "session.port", default)]
    port: Option<serde_json::Value>,
    #[serde(deserialize_with = "default_field", rename = "session.group", default)]
    group: Option<String>,
    #[serde(
        deserialize_with = "default_field",
        rename = "session.description",
        default
    )]
    description: Option<String>,
    #[serde(deserialize_with = "default_field", rename = "session.icon", default)]
    icon: Option<String>,
    #[serde(deserialize_with = "default_field", rename = "ssh.x11", default)]
    x11: bool,
    #[serde(rename = "session.autoLogin", default)]
    auto_login: AutoLoginSource,
    #[serde(deserialize_with = "default_field", rename = "session.oneKey", default)]
    one_key: Option<String>,
    #[serde(
        deserialize_with = "default_field",
        rename = "ssh.identityFilePath.windows",
        default
    )]
    identity_path: Option<String>,
}

/// One entry of WindTerm's `onekeys.config`: a reusable one-key login account.
///
/// `auth` holds the same AES-CBC payload as `session.autoLogin`, so it is
/// decrypted with the profile fingerprint before username/password are read.
/// `expect.username` / `expect.password` are login-prompt regexes, not
/// credentials, so they are intentionally not deserialized.
#[derive(Default, Deserialize)]
struct WindtermOneKeyEntry {
    #[serde(deserialize_with = "default_field", default)]
    name: Option<String>,
    #[serde(deserialize_with = "default_field", rename = "type", default)]
    kind: Option<String>,
    #[serde(deserialize_with = "default_field", default)]
    uuid: Option<String>,
    #[serde(deserialize_with = "default_field", default)]
    auth: Option<SecretString>,
}

/// A resolved `onekeys.config` entry. `insert_password` is false when the user
/// chose to reuse an existing `SavedPassword` instead of creating a new one.
struct OneKeyAccount {
    id: String,
    one_key_uuid: String,
    name: String,
    username: String,
    password: Option<SecretString>,
    password_readable: bool,
    insert_password: bool,
}

/// Account metadata shown to the user before the import is committed.
#[derive(Clone)]
pub struct WindtermAccountPreview {
    pub id: String,
    pub one_key_uuid: String,
    pub name: String,
    pub username: String,
    pub password: Option<SecretString>,
    pub password_available: bool,
    pub password_readable: bool,
    pub session_count: usize,
}

impl fmt::Debug for WindtermAccountPreview {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WindtermAccountPreview")
            .field("id", &self.id)
            .field("one_key_uuid", &self.one_key_uuid)
            .field("name", &self.name)
            .field("username", &self.username)
            .field("password_available", &self.password_available)
            .field("password_readable", &self.password_readable)
            .field("session_count", &self.session_count)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindtermOneKeyConfigState {
    #[default]
    Absent,
    Readable,
    Unreadable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindtermMissingOneKey {
    pub session_name: String,
    pub one_key_uuid: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindtermUnlinkedReason {
    SkippedByUser,
    UnsupportedType,
    MissingDefinition,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindtermUnlinkedSession {
    pub session_name: String,
    pub reason: WindtermUnlinkedReason,
}

/// Import diagnostics for one WindTerm import. Counts and connection names are
/// kept here so the presentation layer never has to re-parse source files.
#[derive(Clone, Debug, Default)]
pub struct WindtermImportReport {
    pub imported_sessions: usize,
    pub skipped_sessions: usize,
    pub credential_warning_sessions: usize,
    pub missing_one_keys: Vec<WindtermMissingOneKey>,
    pub unlinked_sessions: Vec<WindtermUnlinkedSession>,
    pub unsupported_one_key_count: usize,
    pub invalid_one_key_count: usize,
    pub duplicate_one_key_uuid_count: usize,
    pub one_key_state: WindtermOneKeyConfigState,
}

#[derive(Clone, Debug)]
pub enum WindtermAccountAction {
    Create {
        username: String,
        password: Option<SecretString>,
    },
    Reuse {
        account_id: String,
        username: String,
    },
    Skip,
}

#[derive(Clone, Debug)]
pub struct WindtermAccountDecision {
    pub one_key_uuid: String,
    pub action: WindtermAccountAction,
}

#[derive(Debug)]
pub struct WindtermImportOutcome {
    pub prepared: PreparedSessionImport,
    pub report: WindtermImportReport,
}

/// Parsed WindTerm import preview: sessions are already linked to the account
/// ids that `commit_session_import` will persist in the same transaction.
#[derive(Debug)]
pub struct WindtermPreview {
    pub prepared: PreparedSessionImport,
    pub accounts: Vec<WindtermAccountPreview>,
    pub one_key_state: WindtermOneKeyConfigState,
    pub missing_one_keys: Vec<WindtermMissingOneKey>,
    pub unlinked_sessions: Vec<WindtermUnlinkedSession>,
    pub unsupported_one_key_count: usize,
    pub invalid_one_key_count: usize,
    pub duplicate_one_key_uuid_count: usize,
    pub duplicate_account_name_count: usize,
    pub unreadable_account_count: usize,
    pub skipped_session_count: usize,
    pub total_session_count: usize,
}

impl WindtermPreview {
    pub fn report(&self) -> WindtermImportReport {
        WindtermImportReport {
            imported_sessions: self.prepared.connections.len(),
            skipped_sessions: self.skipped_session_count,
            credential_warning_sessions: self.prepared.incomplete_credentials_count(),
            missing_one_keys: self.missing_one_keys.clone(),
            unlinked_sessions: self.unlinked_sessions.clone(),
            unsupported_one_key_count: self.unsupported_one_key_count,
            invalid_one_key_count: self.invalid_one_key_count,
            duplicate_one_key_uuid_count: self.duplicate_one_key_uuid_count,
            one_key_state: self.one_key_state,
        }
    }
}

// Borrow raw fields from the zeroizing input instead of materializing generic
// JSON values containing credentials. WindTerm's optional fields are lenient:
// unsupported metadata falls back, while malformed auto-login is reported.
fn default_field<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    let raw = <&RawValue>::deserialize(deserializer)?;
    Ok(serde_json::from_str(raw.get()).unwrap_or_default())
}

#[derive(Default)]
enum AutoLoginSource {
    #[default]
    Missing,
    Text(SecretString),
    Unreadable,
}

impl<'de> Deserialize<'de> for AutoLoginSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = <&RawValue>::deserialize(deserializer)?;
        Ok(
            match serde_json::from_str::<Option<SecretString>>(raw.get()) {
                Ok(Some(text)) => Self::Text(text),
                Ok(None) => Self::Missing,
                Err(_) => Self::Unreadable,
            },
        )
    }
}

#[derive(Default, Deserialize)]
struct AutoLogin {
    #[serde(deserialize_with = "default_field", rename = "session.user", default)]
    user: Option<String>,
    #[serde(rename = "PasswordEnabled", default)]
    password_enabled: bool,
    #[serde(rename = "Password", default)]
    password: Option<SecretString>,
    #[serde(deserialize_with = "default_field", rename = "Public Key", default)]
    public_key: Option<PublicKey>,
}

#[derive(Deserialize)]
struct PublicKey {
    #[serde(deserialize_with = "default_field", rename = "windows.path", default)]
    path: Option<String>,
    #[serde(deserialize_with = "default_field", rename = "windows.pass", default)]
    passphrase: Option<SecretString>,
}

pub(super) fn parse_windterm(
    path: &Path,
    master_password: Option<&str>,
) -> AppResult<PreparedSessionImport> {
    prepare_windterm_preview(path, master_password).map(|preview| preview.prepared)
}

/// Parse `user.sessions` together with the sibling `onekeys.config`.
///
/// Sessions that carry a `session.oneKey` uuid are linked to a generated
/// `SavedPassword` id, so `commit_session_import` persists accounts and
/// connections atomically. This default preview creates every referenced
/// account; the interactive flow must submit decisions instead.
pub fn prepare_windterm_preview(
    path: &Path,
    master_password: Option<&str>,
) -> AppResult<WindtermPreview> {
    let content = Zeroizing::new(read_text_file_limited(path, "WindTerm session file")?);
    let crypto = load_crypto(path, master_password)?;
    let config = parse_one_key_config(path, &crypto)?;
    let skipped_uuids = HashSet::new();
    let mut parsed = parse_content_full(
        &content,
        &crypto,
        Some(path),
        &config.accounts,
        &config.unsupported_uuids,
        &skipped_uuids,
    )?;

    let referenced = parsed
        .session_counts
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    let accounts = config
        .accounts
        .iter()
        .filter(|account| referenced.contains(&account.one_key_uuid))
        .collect::<Vec<_>>();
    parsed.prepared.passwords = build_password_entries(&accounts, &parsed.session_counts);

    let accounts_preview = accounts
        .iter()
        .map(|account| WindtermAccountPreview {
            id: account.id.clone(),
            one_key_uuid: account.one_key_uuid.clone(),
            name: account.name.clone(),
            username: account.username.clone(),
            password: account.password.clone(),
            password_available: account.password.is_some(),
            password_readable: account.password_readable,
            session_count: parsed
                .session_counts
                .get(&account.one_key_uuid)
                .copied()
                .unwrap_or(0),
        })
        .collect::<Vec<_>>();
    let unreadable_account_count = accounts
        .iter()
        .filter(|account| !account.password_readable)
        .count();

    Ok(WindtermPreview {
        prepared: parsed.prepared,
        accounts: accounts_preview,
        one_key_state: config.state,
        missing_one_keys: parsed.missing_one_keys,
        unlinked_sessions: parsed.unlinked_sessions,
        unsupported_one_key_count: config.unsupported_count,
        invalid_one_key_count: config.invalid_count,
        duplicate_one_key_uuid_count: config.duplicate_uuid_count,
        duplicate_account_name_count: config.duplicate_names,
        unreadable_account_count,
        skipped_session_count: parsed.skipped_sessions,
        total_session_count: parsed.total_sessions,
    })
}

/// Re-parse the source and commit the user's account decisions.
///
/// This is the authoritative path for interactive imports: it rebuilds the
/// sessions so `Skip` can fall back to each session's own `autoLogin` data,
/// and `Reuse` can point connections at an existing `SavedPassword` id.
pub fn prepare_windterm_import_with_decisions(
    path: &Path,
    master_password: Option<&str>,
    decisions: &[WindtermAccountDecision],
) -> AppResult<WindtermImportOutcome> {
    let content = Zeroizing::new(read_text_file_limited(path, "WindTerm session file")?);
    let crypto = load_crypto(path, master_password)?;
    let config = parse_one_key_config(path, &crypto)?;

    let decision_map = decisions
        .iter()
        .map(|decision| (decision.one_key_uuid.as_str(), &decision.action))
        .collect::<HashMap<_, _>>();
    let mut accounts = Vec::new();
    let mut skipped_uuids = HashSet::new();
    for account in config.accounts {
        match decision_map.get(account.one_key_uuid.as_str()) {
            Some(WindtermAccountAction::Create { username, password }) => {
                accounts.push(OneKeyAccount {
                    username: username.clone(),
                    password: password.clone().filter(|value| !value.is_empty()),
                    password_readable: true,
                    insert_password: true,
                    ..account
                });
            }
            Some(WindtermAccountAction::Reuse {
                account_id,
                username,
            }) => {
                accounts.push(OneKeyAccount {
                    id: account_id.clone(),
                    username: username.clone(),
                    password: None,
                    password_readable: true,
                    insert_password: false,
                    ..account
                });
            }
            Some(WindtermAccountAction::Skip) | None => {
                skipped_uuids.insert(account.one_key_uuid);
            }
        }
    }

    let parsed = parse_content_full(
        &content,
        &crypto,
        Some(path),
        &accounts,
        &config.unsupported_uuids,
        &skipped_uuids,
    )?;
    let referenced = parsed
        .session_counts
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    let insert_accounts = accounts
        .iter()
        .filter(|account| referenced.contains(&account.one_key_uuid))
        .collect::<Vec<_>>();
    let mut prepared = parsed.prepared;
    prepared.passwords = build_password_entries(&insert_accounts, &parsed.session_counts);

    let report = WindtermImportReport {
        imported_sessions: prepared.connections.len(),
        skipped_sessions: parsed.skipped_sessions,
        credential_warning_sessions: prepared.incomplete_credentials_count(),
        missing_one_keys: parsed.missing_one_keys,
        unlinked_sessions: parsed.unlinked_sessions,
        unsupported_one_key_count: config.unsupported_count,
        invalid_one_key_count: config.invalid_count,
        duplicate_one_key_uuid_count: config.duplicate_uuid_count,
        one_key_state: config.state,
    };
    Ok(WindtermImportOutcome { prepared, report })
}

fn build_password_entries(
    accounts: &[&OneKeyAccount],
    session_counts: &HashMap<String, usize>,
) -> Vec<SavedPassword> {
    accounts
        .iter()
        .filter(|account| {
            account.insert_password
                && session_counts
                    .get(&account.one_key_uuid)
                    .copied()
                    .unwrap_or(0)
                    > 0
        })
        .enumerate()
        .map(|(index, account)| SavedPassword {
            id: account.id.clone(),
            sort_order: index as i32,
            name: account.name.clone(),
            username: account.username.clone(),
            password: account.password.clone(),
            has_password: false,
        })
        .collect()
}

#[cfg(test)]
pub(super) fn parse_windterm_content(content: &str) -> AppResult<PreparedSessionImport> {
    parse_content_full(
        content,
        &ProfileCrypto::Unavailable(SessionImportWarningKind::MissingProfileConfig),
        None,
        &[],
        &HashSet::new(),
        &HashSet::new(),
    )
    .map(|parsed| parsed.prepared)
}

struct WindtermSessionParse {
    prepared: PreparedSessionImport,
    session_counts: HashMap<String, usize>,
    total_sessions: usize,
    skipped_sessions: usize,
    missing_one_keys: Vec<WindtermMissingOneKey>,
    unlinked_sessions: Vec<WindtermUnlinkedSession>,
}

fn parse_content_full(
    content: &str,
    crypto: &ProfileCrypto,
    source_path: Option<&Path>,
    one_keys: &[OneKeyAccount],
    unsupported_uuids: &HashSet<String>,
    skipped_uuids: &HashSet<String>,
) -> AppResult<WindtermSessionParse> {
    // Do not attach serde's error: invalid field values can contain credentials.
    let entries: Vec<&RawValue> = serde_json::from_str(content)
        .map_err(|_| AppError::Config("Invalid WindTerm sessions JSON".into()))?;
    let mut prepared = PreparedSessionImport {
        warnings: Vec::new(),
        custom_icons: Vec::new(),
        groups: Vec::new(),
        passwords: Vec::new(),
        ssh_keys: Vec::new(),
        connections: Vec::new(),
        notices: Vec::new(),
    };
    let mut key_ids = HashMap::new();
    let mut session_counts = HashMap::new();
    let total_sessions = entries.len();
    let mut skipped_sessions = 0;
    let mut missing_one_keys = Vec::new();
    let mut unlinked_sessions = Vec::new();

    for (session_index, raw) in entries.into_iter().enumerate() {
        let Ok(entry) = serde_json::from_str::<WindtermSession>(raw.get()) else {
            skipped_sessions += 1;
            continue;
        };
        if !entry.protocol.eq_ignore_ascii_case("SSH") {
            skipped_sessions += 1;
            continue;
        }
        let (host, target_username) = parse_windterm_target(&entry.target);
        if host.is_empty() {
            skipped_sessions += 1;
            continue;
        }
        let port = match entry.port.as_ref() {
            None | Some(serde_json::Value::Null) => 22,
            Some(value) => match value.as_u64().and_then(|port| u16::try_from(port).ok()) {
                Some(port) if port > 0 => port,
                _ => {
                    skipped_sessions += 1;
                    continue;
                }
            },
        };
        let name = normalized(entry.label.as_deref())
            .unwrap_or(&host)
            .to_owned();
        let one_key_uuid = normalized(entry.one_key.as_deref()).map(str::to_owned);
        let linked_account = one_key_uuid
            .as_deref()
            .and_then(|uuid| one_keys.iter().find(|account| account.one_key_uuid == uuid));
        if let Some(uuid) = one_key_uuid.as_deref()
            && linked_account.is_none()
        {
            let reason = if unsupported_uuids.contains(uuid) {
                WindtermUnlinkedReason::UnsupportedType
            } else if skipped_uuids.contains(uuid) {
                WindtermUnlinkedReason::SkippedByUser
            } else {
                missing_one_keys.push(WindtermMissingOneKey {
                    session_name: name.clone(),
                    one_key_uuid: uuid.to_owned(),
                });
                WindtermUnlinkedReason::MissingDefinition
            };
            unlinked_sessions.push(WindtermUnlinkedSession {
                session_name: name.clone(),
                reason,
            });
        }
        let (username, auth) = match linked_account {
            Some(account) => {
                *session_counts
                    .entry(account.one_key_uuid.clone())
                    .or_insert(0) += 1;
                (
                    normalized(Some(account.username.as_str()))
                        .unwrap_or(&target_username)
                        .to_owned(),
                    ConnectionAuth {
                        mode: "password".into(),
                        account_id: Some(account.id.clone()),
                        password_source: Some(ConnectionPasswordSource::Account),
                        username_source: Some(ConnectionUsernameSource::Account),
                        ..ConnectionAuth::default()
                    },
                )
            }
            None => {
                let parsed_auto_login = match &entry.auto_login {
                    AutoLoginSource::Missing => Ok(AutoLogin::default()),
                    AutoLoginSource::Text(raw) => parse_auto_login(Some(raw), crypto),
                    AutoLoginSource::Unreadable => {
                        Err(SessionImportWarningKind::UnreadableAutoLogin)
                    }
                };
                let auto_login = match parsed_auto_login {
                    Ok(payload) => payload,
                    Err(kind) => {
                        prepared.warnings.push(SessionImportWarning {
                            session_index,
                            kind,
                        });
                        AutoLogin::default()
                    }
                };
                let username = normalized(auto_login.user.as_deref())
                    .unwrap_or(&target_username)
                    .to_owned();
                let password = auto_login.password.filter(|value| !value.is_empty());
                let auth = if auto_login.password_enabled && password.is_some() {
                    ConnectionAuth {
                        mode: "password".into(),
                        password,
                        ..ConnectionAuth::default()
                    }
                } else {
                    let key_id = import_key(
                        auto_login.public_key,
                        entry.identity_path.as_deref(),
                        source_path,
                        normalized(entry.label.as_deref()).unwrap_or(&host),
                        &mut prepared.ssh_keys,
                        &mut key_ids,
                    );
                    match key_id {
                        Ok(Some(key_id)) => ConnectionAuth {
                            mode: "key".into(),
                            key_id: Some(key_id),
                            ..ConnectionAuth::default()
                        },
                        result => {
                            if result.is_err() {
                                prepared.warnings.push(SessionImportWarning {
                                    session_index,
                                    kind: SessionImportWarningKind::UnreadablePrivateKey,
                                });
                            }
                            ConnectionAuth {
                                mode: "none".into(),
                                ..ConnectionAuth::default()
                            }
                        }
                    }
                };
                (username, auth)
            }
        };
        let group_path = entry.group.as_deref().and_then(|group| {
            let segments: Vec<_> = group
                .split('>')
                .filter_map(|part| normalized(Some(part)))
                .map(str::to_owned)
                .collect();
            (!segments.is_empty()).then_some(segments)
        });
        if let Some(path) = &group_path
            && !prepared.groups.contains(path)
        {
            prepared.groups.push(path.clone());
        }

        prepared.connections.push(PreparedSessionConnection {
            saved: None,
            name,
            config: ConnectionType::Ssh {
                host,
                port,
                username,
                backspace_mode: "del".into(),
                ai_execution_profile: AiExecutionProfile::Auto,
                x11_forwarding: entry.x11,
                auth_agent_endpoint: None,
                agent_forwarding_config: None,
                legacy_agent_forwarding: None,
                encoding: String::new(),
                dynamic_tab_title: false,
            },
            group_path,
            description: normalized(entry.description.as_deref()).map(str::to_owned),
            sort_order: 0,
            icon: normalized(entry.icon.as_deref()).map(str::to_owned),
            auth: Some(auth),
        });
    }
    if total_sessions > 0 && prepared.connections.is_empty() {
        return Err(AppError::Config(
            "No importable WindTerm sessions were found".into(),
        ));
    }
    Ok(WindtermSessionParse {
        prepared,
        session_counts,
        total_sessions,
        skipped_sessions,
        missing_one_keys,
        unlinked_sessions,
    })
}

struct ParsedOneKeyConfig {
    state: WindtermOneKeyConfigState,
    accounts: Vec<OneKeyAccount>,
    unsupported_uuids: HashSet<String>,
    unsupported_count: usize,
    invalid_count: usize,
    duplicate_uuid_count: usize,
    duplicate_names: usize,
}

impl ParsedOneKeyConfig {
    fn absent(state: WindtermOneKeyConfigState) -> Self {
        Self {
            state,
            accounts: Vec::new(),
            unsupported_uuids: HashSet::new(),
            unsupported_count: 0,
            invalid_count: 0,
            duplicate_uuid_count: 0,
            duplicate_names: 0,
        }
    }
}

/// Find `onekeys.config` in the sessions directory, accepting case variants
/// such as `OneKeys.config` since WindTerm users are not consistent about case.
fn find_one_keys_config(directory: &Path) -> Option<PathBuf> {
    let exact = directory.join(ONE_KEYS_CONFIG_FILE);
    if exact.is_file() {
        return Some(exact);
    }
    let entries = fs::read_dir(directory).ok()?;
    entries.flatten().map(|entry| entry.path()).find(|path| {
        path.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case(ONE_KEYS_CONFIG_FILE))
    })
}

/// Read `onekeys.config` from the same directory as `user.sessions`.
///
/// A missing, unreadable, or malformed file is reported through
/// `WindtermOneKeyConfigState` instead of failing the whole session import.
fn parse_one_key_config(path: &Path, crypto: &ProfileCrypto) -> AppResult<ParsedOneKeyConfig> {
    let Some(config_path) = path.parent().and_then(find_one_keys_config) else {
        return Ok(ParsedOneKeyConfig::absent(
            WindtermOneKeyConfigState::Absent,
        ));
    };
    let content = match read_text_file_limited(&config_path, "WindTerm onekeys.config") {
        Ok(content) => Zeroizing::new(content),
        Err(_) => {
            return Ok(ParsedOneKeyConfig::absent(
                WindtermOneKeyConfigState::Unreadable,
            ));
        }
    };
    let entries: Vec<&RawValue> = match serde_json::from_str(&content) {
        Ok(entries) => entries,
        Err(_) => {
            return Ok(ParsedOneKeyConfig::absent(
                WindtermOneKeyConfigState::Unreadable,
            ));
        }
    };
    let mut parsed = ParsedOneKeyConfig::absent(WindtermOneKeyConfigState::Readable);
    let mut seen_uuids = HashSet::new();
    let mut used_names: HashMap<String, usize> = HashMap::new();

    for raw in entries {
        let Ok(entry) = serde_json::from_str::<WindtermOneKeyEntry>(raw.get()) else {
            parsed.invalid_count += 1;
            continue;
        };
        let supported = entry
            .kind
            .as_deref()
            .map(str::trim)
            .filter(|kind| !kind.is_empty())
            .is_none_or(|kind| kind.eq_ignore_ascii_case("SshOneKey"));
        let uuid = normalized(entry.uuid.as_deref()).map(str::to_owned);
        if !supported {
            parsed.unsupported_count += 1;
            if let Some(uuid) = uuid {
                parsed.unsupported_uuids.insert(uuid);
            }
            continue;
        }
        let Some(uuid) = uuid else {
            parsed.invalid_count += 1;
            continue;
        };
        if !seen_uuids.insert(uuid.clone()) {
            parsed.duplicate_uuid_count += 1;
            continue;
        }
        let (username, password, password_readable) =
            parse_one_key_auth(entry.auth.as_deref(), crypto);
        let requested_name = normalized(entry.name.as_deref())
            .unwrap_or(uuid.as_str())
            .to_owned();
        let occurrences = used_names.entry(requested_name.clone()).or_insert(0);
        *occurrences += 1;
        let name = if *occurrences > 1 {
            parsed.duplicate_names += 1;
            format!("{requested_name} ({occurrences})")
        } else {
            requested_name
        };
        parsed.accounts.push(OneKeyAccount {
            id: uuid::Uuid::new_v4().to_string(),
            one_key_uuid: uuid,
            name,
            username: username.unwrap_or_default(),
            password,
            password_readable,
            insert_password: true,
        });
    }

    Ok(parsed)
}

/// Decrypt a `onekeys.config` `auth` payload and extract username/password.
///
/// Unlike `session.autoLogin`, the payload shape is not documented upstream,
/// so common key names are accepted. The third value reports whether the
/// credential payload could be read at all.
fn parse_one_key_auth(
    raw: Option<&str>,
    crypto: &ProfileCrypto,
) -> (Option<String>, Option<SecretString>, bool) {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return (None, None, true);
    };
    let payload = if raw.starts_with('{') || raw.starts_with('"') {
        parse_one_key_payload(raw)
    } else {
        match crypto {
            ProfileCrypto::Ready(crypto) => {
                decrypt_windterm_payload(raw, crypto).and_then(|plaintext| {
                    std::str::from_utf8(&plaintext)
                        .ok()
                        .and_then(parse_one_key_payload)
                })
            }
            ProfileCrypto::Unavailable(_) => None,
        }
    };
    let Some(payload) = payload else {
        return (None, None, false);
    };
    // Preserve priority before normalization: an empty preferred string blocks
    // fallback to lower-priority aliases, just as the original Value lookup did.
    let username = payload
        .users
        .into_iter()
        .flatten()
        .next()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let password = payload
        .passwords
        .into_iter()
        .flatten()
        .next()
        .filter(|value| !value.is_empty());
    (username, password, true)
}

#[derive(Default)]
struct OneKeyPayload {
    users: [Option<String>; 6],
    passwords: [Option<SecretString>; 4],
}

impl<'de> Deserialize<'de> for OneKeyPayload {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PayloadVisitor;
        impl<'de> serde::de::Visitor<'de> for PayloadVisitor {
            type Value = OneKeyPayload;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a credential object")
            }

            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let mut payload = OneKeyPayload::default();
                while let Some(key) = map.next_key::<std::borrow::Cow<'de, str>>()? {
                    // Borrow every raw value, including unknown metadata. No
                    // generic JSON tree ever owns decrypted password strings.
                    let raw = map.next_value::<&RawValue>()?;
                    if let Some(index) = [
                        "username",
                        "user",
                        "session.user",
                        "login",
                        "account",
                        "userName",
                    ]
                    .iter()
                    .position(|name| *name == key)
                    {
                        payload.users[index] = serde_json::from_str(raw.get()).ok();
                    } else if let Some(index) = ["password", "Password", "pass", "secret"]
                        .iter()
                        .position(|name| *name == key)
                    {
                        payload.passwords[index] = serde_json::from_str(raw.get()).ok();
                    }
                }
                Ok(payload)
            }
        }
        deserializer.deserialize_map(PayloadVisitor)
    }
}

fn parse_one_key_payload(text: &str) -> Option<OneKeyPayload> {
    let raw = serde_json::from_str::<&RawValue>(text).ok()?;
    if raw.get().starts_with('{') {
        serde_json::from_str(raw.get()).ok()
    } else if raw.get().starts_with('"') {
        let nested: SecretString = serde_json::from_str(raw.get()).ok()?;
        // Historically valid outer JSON with an unrecognized inner payload was
        // readable, but contained no usable credentials.
        Some(serde_json::from_str(nested.expose_secret()).unwrap_or_default())
    } else {
        Some(OneKeyPayload::default())
    }
}

fn decrypt_windterm_payload(raw: &str, crypto: &WindtermCrypto) -> Option<Zeroizing<Vec<u8>>> {
    let mut buffer = Zeroizing::new(base64::engine::general_purpose::STANDARD.decode(raw).ok()?);
    let material = &crypto.0;
    let cipher =
        cbc::Decryptor::<Aes256>::new_from_slices(&material[..32], &material[32..]).ok()?;
    let plaintext = cipher.decrypt_padded_mut::<Pkcs7>(&mut buffer).ok()?;
    Some(Zeroizing::new(plaintext.to_vec()))
}

fn load_crypto(path: &Path, master_password: Option<&str>) -> AppResult<ProfileCrypto> {
    let Some(profile) = path.parent().and_then(Path::parent) else {
        return Ok(ProfileCrypto::Unavailable(
            SessionImportWarningKind::MissingProfileConfig,
        ));
    };
    let config_path = profile.join("user.config");
    if !config_path.exists() {
        return Ok(ProfileCrypto::Unavailable(
            SessionImportWarningKind::MissingProfileConfig,
        ));
    }
    let config = read_text_file_limited(config_path, "WindTerm user.config")
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    let Some(config) = config else {
        return Ok(ProfileCrypto::Unavailable(
            SessionImportWarningKind::UnreadableProfileConfig,
        ));
    };
    let master_enabled = config
        .get("application.masterPassword")
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    if master_enabled && master_password.is_none_or(str::is_empty) {
        return Err(AppError::WindTermMasterPasswordRequired);
    }
    let Some(fingerprint) = config
        .get("application.fingerprint")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return Ok(ProfileCrypto::Unavailable(
            SessionImportWarningKind::UnreadableProfileConfig,
        ));
    };
    Ok(ProfileCrypto::Ready(derive_crypto(
        fingerprint,
        if master_enabled {
            master_password.unwrap_or_default()
        } else {
            ""
        },
    )))
}

fn derive_crypto(fingerprint: &str, master_password: &str) -> WindtermCrypto {
    let mut material = Zeroizing::new([0; 48]);
    pbkdf2_hmac::<Sha3_512>(
        master_password.as_bytes(),
        fingerprint.as_bytes(),
        PBKDF2_ITERATIONS,
        material.as_mut(),
    );
    WindtermCrypto(material)
}

fn parse_auto_login(
    raw: Option<&str>,
    crypto: &ProfileCrypto,
) -> Result<AutoLogin, SessionImportWarningKind> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(AutoLogin::default());
    };
    if raw.starts_with('{') {
        return serde_json::from_str(raw)
            .map_err(|_| SessionImportWarningKind::UnreadableAutoLogin);
    }
    let crypto = match crypto {
        ProfileCrypto::Ready(crypto) => crypto,
        ProfileCrypto::Unavailable(kind) => return Err(*kind),
    };
    let plaintext = decrypt_windterm_payload(raw, crypto)
        .ok_or(SessionImportWarningKind::UnreadableAutoLogin)?;
    serde_json::from_slice(plaintext.as_slice())
        .map_err(|_| SessionImportWarningKind::UnreadableAutoLogin)
}

// The dedupe map holds indices, so it never duplicates secret-bearing values.
fn import_key(
    public_key: Option<PublicKey>,
    identity_path: Option<&str>,
    source_path: Option<&Path>,
    name: &str,
    keys: &mut Vec<SshKey>,
    key_ids: &mut HashMap<PathBuf, Vec<usize>>,
) -> Result<Option<String>, ()> {
    let (auto_path, passphrase) = public_key
        .map(|key| (key.path, key.passphrase.filter(|value| !value.is_empty())))
        .unwrap_or_default();
    let paths: Vec<_> = [auto_path.as_deref(), identity_path]
        .into_iter()
        .filter_map(normalized)
        .collect();
    if paths.is_empty() {
        return Ok(None);
    }
    for path in paths {
        let resolved = resolve_key_path(path, source_path);
        if let Some(indices) = key_ids.get(&resolved)
            && let Some(index) = indices
                .iter()
                .find(|&&index| passphrase.is_none() || keys[index].passphrase == passphrase)
        {
            return Ok(Some(keys[*index].id.clone()));
        }
        let Ok(content) = read_text_file_limited(&resolved, "WindTerm private key") else {
            continue;
        };
        let content = Zeroizing::new(content);
        if content.trim().is_empty() {
            continue;
        }
        let id = uuid::Uuid::new_v4().to_string();
        let index = keys.len();
        keys.push(SshKey {
            id: id.clone(),
            name: format!("{name} SSH key"),
            sort_order: 0,
            key: Some(content.as_str().into()),
            cert: None,
            passphrase,
            key_file_path: None,
            cert_file_path: None,
            has_key_data: false,
            has_cert_data: false,
        });
        key_ids.entry(resolved).or_default().push(index);
        return Ok(Some(id));
    }
    Err(())
}

fn resolve_key_path(path: &str, source_path: Option<&Path>) -> PathBuf {
    let mut expanded = path.to_owned();
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        expanded = expanded
            .replace("$(HomeDir)", &home_str)
            .replace("${HomeDir}", &home_str);
        if expanded == "~" {
            expanded = home_str.into_owned();
        } else if let Some(rest) = expanded
            .strip_prefix("~/")
            .or_else(|| expanded.strip_prefix("~\\"))
        {
            expanded = home.join(rest).to_string_lossy().into_owned();
        }
    }
    let candidate = PathBuf::from(expanded);
    if candidate.is_absolute() {
        candidate
    } else {
        source_path
            .and_then(Path::parent)
            .map(|parent| parent.join(&candidate))
            .unwrap_or(candidate)
    }
}

fn normalized(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

pub(super) fn parse_windterm_target(target: &str) -> (String, String) {
    let target = target.trim();
    if let Some((username, host)) = target.rsplit_once('@')
        && !username.is_empty()
        && !host.is_empty()
    {
        return (host.to_string(), username.to_string());
    }
    (target.to_string(), "root".to_string())
}

#[cfg(test)]
mod tests;
