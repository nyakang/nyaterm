use std::collections::HashMap;
use std::path::{Path, PathBuf};

use aes::Aes256;
use base64::Engine as _;
use cbc::cipher::{BlockDecryptMut as _, KeyIvInit as _, block_padding::Pkcs7};
use pbkdf2::pbkdf2_hmac;
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;
use sha3::Sha3_512;
use zeroize::Zeroizing;

use crate::{AiExecutionProfile, ConnectionAuth, ConnectionType, SecretString, SshKey};

use super::{
    AppError, AppResult, PreparedSessionConnection, PreparedSessionImport, SessionImportWarning,
    SessionImportWarningKind, read_text_file_limited,
};

const PBKDF2_ITERATIONS: u32 = 100_000;

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
    #[serde(
        deserialize_with = "default_field",
        rename = "ssh.identityFilePath.windows",
        default
    )]
    identity_path: Option<String>,
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
    let content = Zeroizing::new(read_text_file_limited(path, "WindTerm session file")?);
    let crypto = load_crypto(path, master_password)?;
    parse_content(&content, &crypto, Some(path))
}

#[cfg(test)]
pub(super) fn parse_windterm_content(content: &str) -> AppResult<PreparedSessionImport> {
    parse_content(
        content,
        &ProfileCrypto::Unavailable(SessionImportWarningKind::MissingProfileConfig),
        None,
    )
}

fn parse_content(
    content: &str,
    crypto: &ProfileCrypto,
    source_path: Option<&Path>,
) -> AppResult<PreparedSessionImport> {
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
    };
    let mut key_ids = HashMap::new();

    for (session_index, raw) in entries.into_iter().enumerate() {
        let Ok(entry) = serde_json::from_str::<WindtermSession>(raw.get()) else {
            continue;
        };
        if !entry.protocol.eq_ignore_ascii_case("SSH") {
            continue;
        }
        let (host, target_username) = parse_windterm_target(&entry.target);
        if host.is_empty() {
            continue;
        }
        let port = match entry.port.as_ref() {
            None | Some(serde_json::Value::Null) => 22,
            Some(value) => match value.as_u64().and_then(|port| u16::try_from(port).ok()) {
                Some(port) if port > 0 => port,
                _ => continue,
            },
        };
        let parsed_auto_login = match &entry.auto_login {
            AutoLoginSource::Missing => Ok(AutoLogin::default()),
            AutoLoginSource::Text(raw) => parse_auto_login(Some(raw), crypto),
            AutoLoginSource::Unreadable => Err(SessionImportWarningKind::UnreadableAutoLogin),
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
        let name = normalized(entry.label.as_deref())
            .unwrap_or(&host)
            .to_owned();
        let username = normalized(auto_login.user.as_deref())
            .unwrap_or(&target_username)
            .to_owned();
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
                &name,
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
    Ok(prepared)
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
    let mut buffer = Zeroizing::new(
        base64::engine::general_purpose::STANDARD
            .decode(raw)
            .map_err(|_| SessionImportWarningKind::UnreadableAutoLogin)?,
    );
    let material = &crypto.0;
    let cipher = cbc::Decryptor::<Aes256>::new_from_slices(&material[..32], &material[32..])
        .map_err(|_| SessionImportWarningKind::UnreadableAutoLogin)?;
    let plaintext = cipher
        .decrypt_padded_mut::<Pkcs7>(&mut buffer)
        .map_err(|_| SessionImportWarningKind::UnreadableAutoLogin)?;
    serde_json::from_slice(plaintext).map_err(|_| SessionImportWarningKind::UnreadableAutoLogin)
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
