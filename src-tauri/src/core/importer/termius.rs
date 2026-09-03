use base64::Engine as _;
use crypto_secretbox::aead::{Aead, KeyInit};
use crypto_secretbox::{Key, Nonce, XSalsa20Poly1305};
use rusty_leveldb::LdbIterator;
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use zeroize::{Zeroize, Zeroizing};

#[derive(Debug, Clone, Default)]
struct TermiusRawStore {
    hosts: Vec<TermiusRawHost>,
    ssh_configs: Vec<TermiusRawSshConfig>,
    identities: Vec<TermiusRawIdentity>,
    ssh_keys: Vec<TermiusRawSshKey>,
    groups: Vec<TermiusRawGroup>,
}

#[derive(Debug, Clone, Default)]
struct TermiusRawHost {
    id: String,
    local_id: Option<String>,
    label: Option<String>,
    address: Option<String>,
    username: Option<String>,
    password: Option<String>,
    ssh_config_id: Option<String>,
    identity_id: Option<String>,
    group_id: Option<String>,
    port: Option<u16>,
    updated_at: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct TermiusRawSshConfig {
    id: String,
    local_id: Option<String>,
    identity_id: Option<String>,
    port: Option<u16>,
    updated_at: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct TermiusRawIdentity {
    id: String,
    local_id: Option<String>,
    label: Option<String>,
    username: Option<String>,
    password: Option<String>,
    ssh_key_id: Option<String>,
    updated_at: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct TermiusRawSshKey {
    id: String,
    local_id: Option<String>,
    label: Option<String>,
    passphrase: Option<String>,
    private_key: Option<String>,
    updated_at: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct TermiusRawGroup {
    id: String,
    local_id: Option<String>,
    label: Option<String>,
    parent_id: Option<String>,
    updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TermiusTaggedValue {
    String(String),
    Integer(i64),
}

impl TermiusTaggedValue {
    fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            Self::Integer(_) => None,
        }
    }

    fn to_field_value(&self) -> String {
        match self {
            Self::String(value) => value.clone(),
            Self::Integer(value) => value.to_string(),
        }
    }
}

fn parse_termius_indexed_db(path: Option<String>) -> AppResult<PreparedJsonImport> {
    let db_path = resolve_termius_indexed_db_path(path)?;
    let local_key = load_termius_local_key()?;
    let key: &[u8; 32] = local_key
        .as_ref()
        .try_into()
        .map_err(|_| AppError::Config("Termius localKey has an unsupported format".to_string()))?;
    parse_termius_indexed_db_with_key(&db_path, key)
}

fn parse_termius_indexed_db_with_key(
    db_path: &Path,
    local_key: &[u8; 32],
) -> AppResult<PreparedJsonImport> {
    let bytes = read_leveldb_records(db_path)?;
    let mut values = parse_tagged_values(&bytes);
    decrypt_termius_values(&mut values, local_key)?;
    let store = collect_termius_store(&values);
    prepare_termius_import(store)
}

fn resolve_termius_indexed_db_path(path: Option<String>) -> AppResult<PathBuf> {
    let resolved = if let Some(path) = normalize_optional_string(path) {
        normalize_termius_indexed_db_path(PathBuf::from(path))
    } else {
        default_termius_indexed_db_path()
    };

    if resolved.is_dir() {
        Ok(resolved)
    } else {
        Err(AppError::Config(format!(
            "Termius IndexedDB directory was not found: {}",
            resolved.display()
        )))
    }
}

fn normalize_termius_indexed_db_path(path: PathBuf) -> PathBuf {
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".indexeddb.leveldb"))
    {
        return path;
    }

    let child = path.join("file__0.indexeddb.leveldb");
    if child.exists() { child } else { path }
}

fn default_termius_indexed_db_path() -> PathBuf {
    if cfg!(target_os = "windows") {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata)
                .join("Termius")
                .join("IndexedDB")
                .join("file__0.indexeddb.leveldb");
        }
    }

    if cfg!(target_os = "macos") {
        if let Some(home) = dirs::home_dir() {
            return home
                .join("Library")
                .join("Application Support")
                .join("Termius")
                .join("IndexedDB")
                .join("file__0.indexeddb.leveldb");
        }
    }

    if let Some(xdg_config_home) = std::env::var_os("XDG_CONFIG_HOME") {
        let path = PathBuf::from(xdg_config_home)
            .join("Termius")
            .join("IndexedDB")
            .join("file__0.indexeddb.leveldb");
        if path.exists() {
            return path;
        }
    }

    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("Termius")
        .join("IndexedDB")
        .join("file__0.indexeddb.leveldb")
}

fn load_termius_local_key() -> AppResult<Zeroizing<[u8; 32]>> {
    let mut errors = Vec::new();

    #[cfg(target_os = "windows")]
    {
        match keyring::Entry::new_with_target("Termius/localKey", "Termius", "localKey")
            .and_then(|entry| entry.get_secret())
        {
            Ok(secret) => return normalize_termius_local_key_bytes(&secret),
            Err(error) => errors.push(format!(
                "target=Termius/localKey service=Termius user=localKey: {error}"
            )),
        }
    }

    let candidates = [
        ("Termius/localKey", "localKey"),
        ("Termius", "localKey"),
    ];

    for (service, user) in candidates {
        match keyring::Entry::new(service, user).and_then(|entry| entry.get_secret()) {
            Ok(secret) => return normalize_termius_local_key_bytes(&secret),
            Err(error) => errors.push(format!("{service}/{user}: {error}")),
        }
    }

    Err(AppError::Config(format!(
        "Cannot read Termius localKey from the system keychain. Tried {}",
        errors.join(", ")
    )))
}

#[cfg(test)]
fn normalize_termius_local_key(secret: &str) -> AppResult<Zeroizing<[u8; 32]>> {
    normalize_termius_local_key_bytes(secret.as_bytes())
}

fn normalize_termius_local_key_bytes(secret: &[u8]) -> AppResult<Zeroizing<[u8; 32]>> {
    let mut candidates = Vec::new();
    candidates.push(secret.to_vec());

    if let Ok(text) = std::str::from_utf8(secret) {
        collect_termius_local_key_text_candidates(text, &mut candidates);
    }

    if secret.len() % 2 == 0 {
        let utf16: Vec<u16> = secret
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        if let Ok(text) = String::from_utf16(&utf16) {
            collect_termius_local_key_text_candidates(&text, &mut candidates);
        }
    }

    normalize_termius_local_key_candidates(candidates)
}

fn collect_termius_local_key_text_candidates(text: &str, candidates: &mut Vec<Vec<u8>>) {
    let trimmed = text.trim_matches(|ch: char| ch.is_whitespace() || ch == '\0');
    candidates.push(trimmed.as_bytes().to_vec());

    if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(trimmed) {
        candidates.push(decoded);
    }
    if let Ok(decoded) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(trimmed) {
        candidates.push(decoded);
    }
    if let Ok(decoded) = hex::decode(trimmed) {
        candidates.push(decoded);
    }

    if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
        collect_termius_local_key_json_candidates(&json, candidates);
    }
}

fn collect_termius_local_key_json_candidates(
    value: &serde_json::Value,
    candidates: &mut Vec<Vec<u8>>,
) {
    match value {
        serde_json::Value::String(value) => {
            collect_termius_local_key_text_candidates(value, candidates);
        }
        serde_json::Value::Object(object) => {
            for key in ["localKey", "key", "secret", "value"] {
                if let Some(value) = object.get(key) {
                    collect_termius_local_key_json_candidates(value, candidates);
                }
            }
        }
        _ => {}
    }
}

fn normalize_termius_local_key_candidates(
    candidates: Vec<Vec<u8>>,
) -> AppResult<Zeroizing<[u8; 32]>> {
    for mut candidate in candidates {
        if candidate.len() == 32 {
            let mut key = [0_u8; 32];
            key.copy_from_slice(&candidate);
            candidate.zeroize();
            return Ok(Zeroizing::new(key));
        }
        candidate.zeroize();
    }

    Err(AppError::Config(
        "Termius localKey has an unsupported format".to_string(),
    ))
}

fn read_leveldb_records(db_path: &Path) -> AppResult<Vec<u8>> {
    let tmp_path = std::env::temp_dir().join(format!(
        "nyaterm-termius-leveldb-{}",
        uuid::Uuid::new_v4()
    ));
    copy_leveldb_dir(db_path, &tmp_path)?;

    let result = read_copied_leveldb_records(&tmp_path);
    let _ = std::fs::remove_dir_all(&tmp_path);
    result
}

fn copy_leveldb_dir(source: &Path, target: &Path) -> AppResult<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if source_path.is_file() {
            std::fs::copy(&source_path, &target_path)?;
        }
    }
    Ok(())
}

fn read_copied_leveldb_records(db_path: &Path) -> AppResult<Vec<u8>> {
    let mut options = rusty_leveldb::Options::default();
    options.create_if_missing = false;
    let mut db = rusty_leveldb::DB::open(db_path, options)
        .map_err(|error| AppError::Config(format!("Cannot open Termius IndexedDB: {error}")))?;
    let mut iter = db
        .new_iter()
        .map_err(|error| AppError::Config(format!("Cannot read Termius IndexedDB: {error}")))?;
    let mut bytes = Vec::new();
    iter.seek_to_first();
    while iter.valid() {
        if let Some((key, value)) = iter.current() {
            bytes.extend_from_slice(&key);
            bytes.push(0);
            bytes.extend_from_slice(&value);
            bytes.push(0);
        }
        iter.advance();
    }
    Ok(bytes)
}

#[cfg(test)]
fn parse_tagged_strings(bytes: &[u8]) -> Vec<String> {
    parse_tagged_values(bytes)
        .into_iter()
        .filter_map(|value| match value {
            TermiusTaggedValue::String(value) => Some(value),
            TermiusTaggedValue::Integer(_) => None,
        })
        .collect()
}

fn parse_tagged_values(bytes: &[u8]) -> Vec<TermiusTaggedValue> {
    let mut strings = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'I' {
            if let Some((value, consumed)) = read_v8_signed_integer(&bytes[index + 1..]) {
                strings.push(TermiusTaggedValue::Integer(value));
                index += 1 + consumed;
                continue;
            }
        }

        if bytes[index] != b'"' {
            index += 1;
            continue;
        }
        let Some((length, consumed)) = read_v8_varint(&bytes[index + 1..]) else {
            index += 1;
            continue;
        };
        let start = index + 1 + consumed;
        let end = start.saturating_add(length);
        if end > bytes.len() {
            index += 1;
            continue;
        }
        if let Ok(value) = std::str::from_utf8(&bytes[start..end]) {
            strings.push(TermiusTaggedValue::String(value.to_string()));
        }
        index = end;
    }

    strings
}

fn read_v8_varint(bytes: &[u8]) -> Option<(usize, usize)> {
    let mut value = 0_usize;
    let mut shift = 0_usize;
    for (index, byte) in bytes.iter().copied().take(5).enumerate() {
        value |= usize::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some((value, index + 1));
        }
        shift += 7;
    }
    None
}

fn read_v8_signed_integer(bytes: &[u8]) -> Option<(i64, usize)> {
    let (raw, consumed) = read_v8_varint(bytes)?;
    let value = ((raw >> 1) as i64) ^ -((raw & 1) as i64);
    Some((value, consumed))
}

fn decrypt_termius_values(
    values: &mut [TermiusTaggedValue],
    local_key: &[u8; 32],
) -> AppResult<()> {
    for value in values {
        let TermiusTaggedValue::String(text) = value else {
            continue;
        };
        if !is_termius_encrypted_value(text) {
            continue;
        }
        let decrypted = decrypt_termius_secret(text, local_key)?;
        *text = decrypted.to_string();
    }
    Ok(())
}

fn is_termius_encrypted_value(value: &str) -> bool {
    value.starts_with("BA")
        && value.len() >= 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
}

fn decrypt_termius_secret(value: &str, local_key: &[u8; 32]) -> AppResult<Zeroizing<String>> {
    let mut decoded = base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|_| AppError::Crypto("Cannot decode Termius encrypted value".to_string()))?;
    if decoded.len() < 26 + 16 {
        decoded.zeroize();
        return Err(AppError::Crypto(
            "Termius encrypted value is too short".to_string(),
        ));
    }

    let nonce = Nonce::from_slice(&decoded[2..26]);
    let cipher = XSalsa20Poly1305::new(Key::from_slice(local_key));
    let mut plaintext = cipher
        .decrypt(nonce, &decoded[26..])
        .map_err(|_| AppError::Crypto("Cannot decrypt Termius encrypted value".to_string()))?;
    decoded.zeroize();

    let text = String::from_utf8(std::mem::take(&mut plaintext)).map_err(|_| {
        plaintext.zeroize();
        AppError::Crypto("Termius encrypted value is not valid UTF-8".to_string())
    })?;
    plaintext.zeroize();
    Ok(Zeroizing::new(text))
}

fn collect_termius_store(strings: &[TermiusTaggedValue]) -> TermiusRawStore {
    let mut store = TermiusRawStore::default();

    for (index, value) in strings.iter().enumerate() {
        if value.as_str() == Some("resource_uri")
            && strings
                .get(index + 1)
                .and_then(TermiusTaggedValue::as_str)
                .is_some_and(|uri| uri.contains("/terminal/ssh/config/"))
        {
            if let Some(config) = collect_ssh_config_record(strings, index) {
                store.ssh_configs.push(config);
            }
        }

        if !is_record_marker(strings, index) {
            continue;
        }
        match value.as_str() {
            Some("ssh_config") => {
                if let Some(host) = collect_host_record(strings, index) {
                    store.hosts.push(host);
                }
            }
            Some("identity") => {
                if let Some(identity) = collect_identity_record(strings, index) {
                    store.identities.push(identity);
                }
            }
            Some("ssh_key") => {
                if let Some(key) = collect_ssh_key_record(strings, index) {
                    store.ssh_keys.push(key);
                } else if let Some(identity) = collect_identity_record(strings, index) {
                    store.identities.push(identity);
                }
            }
            Some("group") => {
                if let Some(group) = collect_group_record(strings, index) {
                    store.groups.push(group);
                }
            }
            _ => {}
        }
    }

    store
}

fn is_record_marker(strings: &[TermiusTaggedValue], index: usize) -> bool {
    strings
        .get(index + 1)
        .and_then(TermiusTaggedValue::as_str)
        .is_some_and(|value| value == "id")
}

fn collect_host_record(strings: &[TermiusTaggedValue], index: usize) -> Option<TermiusRawHost> {
    let record = collect_fields(strings, index, &["address", "label", "username", "password"], 80);
    let address = record.get("address").cloned();
    let label = record.get("label").cloned();
    if address.as_ref().is_none_or(|value| value.trim().is_empty())
        && label.as_ref().is_none_or(|value| value.trim().is_empty())
    {
        return None;
    }

    Some(TermiusRawHost {
        id: record_id(strings, index),
        local_id: record.get("local_id").cloned(),
        label,
        address,
        username: record.get("username").cloned(),
        password: record.get("password").cloned(),
        ssh_config_id: nested_record_id(strings, index, "ssh_config"),
        identity_id: first_non_empty_field(&record, &["identity", "identity_id", "ssh_key"]),
        group_id: first_non_empty_field(&record, &["group", "group_id"]),
        port: first_non_empty_field(&record, &["port"]).and_then(|value| value.parse().ok()),
        updated_at: record.get("updated_at").cloned(),
    })
}

fn collect_ssh_config_record(
    strings: &[TermiusTaggedValue],
    resource_uri_index: usize,
) -> Option<TermiusRawSshConfig> {
    let uri = strings.get(resource_uri_index + 1)?.as_str()?;
    let id = termius_resource_id(uri)?;
    let record = collect_fields(strings, resource_uri_index, &["port"], 80);

    Some(TermiusRawSshConfig {
        id,
        local_id: record.get("local_id").cloned(),
        identity_id: nested_record_id(strings, resource_uri_index, "identity"),
        port: first_non_empty_field(&record, &["port"]).and_then(|value| value.parse().ok()),
        updated_at: record.get("updated_at").cloned(),
    })
}

fn collect_identity_record(
    strings: &[TermiusTaggedValue],
    index: usize,
) -> Option<TermiusRawIdentity> {
    let record = collect_fields(strings, index, &["label", "username", "password"], 80);
    if !record.contains_key("username") && !record.contains_key("password") && !record.contains_key("ssh_key") {
        return None;
    }

    Some(TermiusRawIdentity {
        id: record_id(strings, index),
        local_id: record.get("local_id").cloned(),
        label: record.get("label").cloned(),
        username: record.get("username").cloned(),
        password: record.get("password").cloned(),
        ssh_key_id: nested_record_id(strings, index, "ssh_key")
            .or_else(|| first_non_empty_field(&record, &["ssh_key", "ssh_key_id", "key"])),
        updated_at: record.get("updated_at").cloned(),
    })
}

fn collect_ssh_key_record(
    strings: &[TermiusTaggedValue],
    index: usize,
) -> Option<TermiusRawSshKey> {
    let record = collect_fields(strings, index, &["private_key"], 80);
    if !record.contains_key("private_key") {
        return None;
    }

    Some(TermiusRawSshKey {
        id: record_id(strings, index),
        local_id: record.get("local_id").cloned(),
        label: record.get("label").cloned(),
        passphrase: record.get("passphrase").cloned(),
        private_key: record.get("private_key").cloned(),
        updated_at: record.get("updated_at").cloned(),
    })
}

fn collect_group_record(strings: &[TermiusTaggedValue], index: usize) -> Option<TermiusRawGroup> {
    let record = collect_fields(strings, index, &["label"], 80);
    let label = record.get("label").cloned()?;
    Some(TermiusRawGroup {
        id: record_id(strings, index),
        local_id: record.get("local_id").cloned(),
        label: Some(label),
        parent_id: first_non_empty_field(&record, &["parent", "parent_id", "group"]),
        updated_at: record.get("updated_at").cloned(),
    })
}

fn collect_fields(
    strings: &[TermiusTaggedValue],
    index: usize,
    required_keys: &[&str],
    max_fields: usize,
) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    let start = record_start(strings, index);
    let mut cursor = start;
    let end = record_end(strings, index, max_fields);

    while cursor < end {
        let Some(key) = strings[cursor].as_str() else {
            cursor += 1;
            continue;
        };
        if is_termius_field_name(key) {
            let value = &strings[cursor + 1];
            let is_nested_name = value
                .as_str()
                .is_some_and(|text| is_termius_field_name(text) || is_record_type_name(text));
            if !is_nested_name {
                fields.entry(key.to_string()).or_insert(value.to_field_value());
            }
        }
        cursor += 1;
    }

    if required_keys.iter().any(|key| fields.contains_key(*key)) {
        fields
    } else {
        HashMap::new()
    }
}

fn record_start(strings: &[TermiusTaggedValue], index: usize) -> usize {
    let start = index.saturating_sub(120);
    for cursor in (start..index).rev() {
        if strings[cursor].as_str() == Some("status") {
            let mut next = (cursor + 2).min(strings.len());
            while next < strings.len() && matches!(strings[next], TermiusTaggedValue::Integer(_)) {
                next += 1;
            }
            return next.min(index);
        }
    }
    index.saturating_sub(8)
}

fn record_end(strings: &[TermiusTaggedValue], index: usize, max_fields: usize) -> usize {
    let hard_end = (record_start(strings, index) + max_fields).min(strings.len().saturating_sub(1));
    for cursor in (index + 1)..hard_end {
        if strings[cursor].as_str() == Some("status") {
            return (cursor + 2).min(strings.len().saturating_sub(1));
        }
    }
    hard_end
}

fn is_record_type_name(value: &str) -> bool {
    matches!(value, "ssh_config" | "identity" | "ssh_key" | "group")
}

fn is_termius_field_name(value: &str) -> bool {
    matches!(
        value,
        "id"
            | "local_id"
            | "updated_at"
            | "label"
            | "address"
            | "username"
            | "password"
            | "private_key"
            | "passphrase"
            | "group"
            | "group_id"
            | "identity"
            | "identity_id"
            | "ssh_key"
            | "ssh_key_id"
            | "key"
            | "parent"
            | "parent_id"
            | "port"
    )
}

fn record_id(strings: &[TermiusTaggedValue], index: usize) -> String {
    let start = record_start(strings, index);
    let end = record_end(strings, index, 80);
    for cursor in start..end {
        if strings[cursor].as_str() == Some("resource_uri") {
            let Some(uri) = strings[cursor + 1].as_str().map(str::trim) else {
                continue;
            };
            if !uri.is_empty() {
                return uri.to_string();
            }
        }
    }
    for cursor in start..end {
        if strings[cursor].as_str() == Some("id") {
            let id = strings[cursor + 1].to_field_value();
            let id = id.trim();
            if !id.is_empty() && !is_termius_field_name(id) {
                return id.to_string();
            }
        }
    }
    uuid::Uuid::new_v4().to_string()
}

fn nested_record_id(
    strings: &[TermiusTaggedValue],
    index: usize,
    field: &str,
) -> Option<String> {
    let start = record_start(strings, index);
    let end = record_end(strings, index, 80);
    for cursor in start..end.saturating_sub(2) {
        if strings[cursor].as_str() == Some(field)
            && strings[cursor + 1].as_str() == Some("id")
        {
            let value = strings[cursor + 2].to_field_value();
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn termius_resource_id(uri: &str) -> Option<String> {
    uri.trim_matches('/')
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn termius_record_aliases(id: &str, local_id: Option<&str>) -> Vec<String> {
    let mut aliases = vec![id.to_string()];
    if let Some(resource_id) = termius_resource_id(id) {
        aliases.push(resource_id);
    }
    if let Some(local_id) = local_id.filter(|value| !value.is_empty()) {
        aliases.push(local_id.to_string());
    }
    aliases.sort();
    aliases.dedup();
    aliases
}

fn first_non_empty_field(record: &HashMap<String, String>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        record.get(*key).and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
    })
}

fn prepare_termius_import(store: TermiusRawStore) -> AppResult<PreparedJsonImport> {
    let groups = dedupe_latest_groups(store.groups);
    let identities = dedupe_latest_identities(store.identities);
    let ssh_keys = dedupe_latest_ssh_keys(store.ssh_keys);
    let ssh_configs = dedupe_latest_ssh_configs(store.ssh_configs);
    let hosts = dedupe_latest_hosts(store.hosts);

    let group_paths = build_termius_group_paths(&groups);
    let prepared_keys = prepare_termius_keys(&ssh_keys)?;
    let prepared_passwords = prepare_termius_passwords(&hosts, &identities)?;
    let connections = prepare_termius_connections(
        hosts,
        &ssh_configs,
        &identities,
        &prepared_keys.ids,
        &prepared_passwords.ids,
        &group_paths,
    )?;

    Ok(PreparedJsonImport {
        groups: group_paths.values().cloned().collect(),
        passwords: prepared_passwords.passwords,
        ssh_keys: prepared_keys.keys,
        connections,
    })
}

fn dedupe_latest_hosts(hosts: Vec<TermiusRawHost>) -> Vec<TermiusRawHost> {
    dedupe_latest(hosts, termius_host_key, |item| item.updated_at.as_deref())
}

fn dedupe_latest_ssh_configs(configs: Vec<TermiusRawSshConfig>) -> Vec<TermiusRawSshConfig> {
    dedupe_latest(configs, termius_ssh_config_key, |item| {
        item.updated_at.as_deref()
    })
}

fn dedupe_latest_identities(identities: Vec<TermiusRawIdentity>) -> Vec<TermiusRawIdentity> {
    dedupe_latest(identities, termius_identity_key, |item| item.updated_at.as_deref())
}

fn dedupe_latest_ssh_keys(keys: Vec<TermiusRawSshKey>) -> Vec<TermiusRawSshKey> {
    dedupe_latest(keys, termius_ssh_key_key, |item| item.updated_at.as_deref())
}

fn dedupe_latest_groups(groups: Vec<TermiusRawGroup>) -> Vec<TermiusRawGroup> {
    dedupe_latest(groups, termius_group_key, |item| item.updated_at.as_deref())
}

fn dedupe_latest<T, K, F, U>(items: Vec<T>, key_fn: F, updated_fn: U) -> Vec<T>
where
    K: Ord,
    F: Fn(&T) -> K,
    U: Fn(&T) -> Option<&str>,
{
    let mut map: BTreeMap<K, T> = BTreeMap::new();
    for item in items {
        let key = key_fn(&item);
        let replace = map.get(&key).is_none_or(|existing| {
            updated_fn(&item).unwrap_or_default() >= updated_fn(existing).unwrap_or_default()
        });
        if replace {
            map.insert(key, item);
        }
    }
    map.into_values().collect()
}

fn termius_host_key(host: &TermiusRawHost) -> String {
    host.local_id
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or(&host.id)
        .to_string()
}

fn termius_identity_key(identity: &TermiusRawIdentity) -> String {
    identity
        .local_id
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or(&identity.id)
        .to_string()
}

fn termius_ssh_config_key(config: &TermiusRawSshConfig) -> String {
    config.id.clone()
}

fn termius_ssh_key_key(key: &TermiusRawSshKey) -> String {
    key.local_id
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or(&key.id)
        .to_string()
}

fn termius_group_key(group: &TermiusRawGroup) -> String {
    group
        .local_id
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or(&group.id)
        .to_string()
}

fn build_termius_group_paths(groups: &[TermiusRawGroup]) -> HashMap<String, Vec<String>> {
    let by_key: HashMap<String, &TermiusRawGroup> =
        groups.iter().map(|group| (termius_group_key(group), group)).collect();
    let mut paths = HashMap::new();
    for group in groups {
        let key = termius_group_key(group);
        let mut visited = HashSet::new();
        if let Some(path) = build_termius_group_path(&key, &by_key, &mut visited) {
            paths.insert(key, path);
        }
    }
    paths
}

fn build_termius_group_path(
    key: &str,
    by_key: &HashMap<String, &TermiusRawGroup>,
    visited: &mut HashSet<String>,
) -> Option<Vec<String>> {
    if !visited.insert(key.to_string()) {
        return None;
    }
    let group = by_key.get(key)?;
    let label = normalize_optional_string(group.label.clone())?;
    let mut path = if let Some(parent) = group.parent_id.as_deref().filter(|value| !value.is_empty())
    {
        build_termius_group_path(parent, by_key, visited).unwrap_or_default()
    } else {
        Vec::new()
    };
    path.push(label);
    Some(path)
}

struct PreparedTermiusKeys {
    keys: Vec<config::SshKey>,
    ids: HashMap<String, String>,
}

fn prepare_termius_keys(keys: &[TermiusRawSshKey]) -> AppResult<PreparedTermiusKeys> {
    let mut prepared = Vec::new();
    let mut ids = HashMap::new();

    for key in keys {
        let Some(private_key) = normalize_optional_string(key.private_key.clone()) else {
            continue;
        };
        let id = uuid::Uuid::new_v4().to_string();
        for alias in termius_record_aliases(&key.id, key.local_id.as_deref()) {
            ids.insert(alias, id.clone());
        }
        ids.insert(termius_ssh_key_key(key), id.clone());
        prepared.push(config::SshKey {
            id,
            name: normalize_optional_string(key.label.clone())
                .unwrap_or_else(|| "Termius SSH Key".to_string()),
            key: Some(encrypt_import_secret(&private_key)?),
            cert: None,
            passphrase: encrypt_optional_secret(key.passphrase.clone())?,
            key_data: None,
            cert_data: None,
            key_file_path: None,
            cert_file_path: None,
            has_key_data: false,
            has_cert_data: false,
        });
    }

    Ok(PreparedTermiusKeys {
        keys: prepared,
        ids,
    })
}

struct PreparedTermiusPasswords {
    passwords: Vec<config::SavedPassword>,
    ids: HashMap<String, String>,
}

fn prepare_termius_passwords(
    hosts: &[TermiusRawHost],
    identities: &[TermiusRawIdentity],
) -> AppResult<PreparedTermiusPasswords> {
    let mut passwords = Vec::new();
    let mut ids = HashMap::new();

    for host in hosts {
        if let Some(password) = normalize_optional_string(host.password.clone()) {
            let id = uuid::Uuid::new_v4().to_string();
            ids.insert(format!("host:{}", termius_host_key(host)), id.clone());
            for alias in termius_record_aliases(&host.id, host.local_id.as_deref()) {
                ids.insert(format!("host:{alias}"), id.clone());
            }
            passwords.push(config::SavedPassword {
                id,
                name: format!(
                    "{} password",
                    normalize_optional_string(host.label.clone())
                        .or_else(|| normalize_optional_string(host.address.clone()))
                        .unwrap_or_else(|| "Termius host".to_string())
                ),
                password: Some(encrypt_import_secret(&password)?),
                has_password: false,
            });
        }
    }

    for identity in identities {
        if let Some(password) = normalize_optional_string(identity.password.clone()) {
            let id = uuid::Uuid::new_v4().to_string();
            ids.insert(format!("identity:{}", termius_identity_key(identity)), id.clone());
            for alias in termius_record_aliases(&identity.id, identity.local_id.as_deref()) {
                ids.insert(format!("identity:{alias}"), id.clone());
            }
            passwords.push(config::SavedPassword {
                id,
                name: format!(
                    "{} password",
                    normalize_optional_string(identity.label.clone())
                        .or_else(|| normalize_optional_string(identity.username.clone()))
                        .unwrap_or_else(|| "Termius identity".to_string())
                ),
                password: Some(encrypt_import_secret(&password)?),
                has_password: false,
            });
        }
    }

    Ok(PreparedTermiusPasswords { passwords, ids })
}

fn prepare_termius_connections(
    hosts: Vec<TermiusRawHost>,
    ssh_configs: &[TermiusRawSshConfig],
    identities: &[TermiusRawIdentity],
    key_ids: &HashMap<String, String>,
    password_ids: &HashMap<String, String>,
    group_paths: &HashMap<String, Vec<String>>,
) -> AppResult<Vec<PreparedJsonConnection>> {
    let mut ssh_configs_by_key: HashMap<String, &TermiusRawSshConfig> = HashMap::new();
    for config in ssh_configs {
        for alias in termius_record_aliases(&config.id, config.local_id.as_deref()) {
            ssh_configs_by_key.insert(alias, config);
        }
        ssh_configs_by_key.insert(termius_ssh_config_key(config), config);
    }
    let mut identities_by_key: HashMap<String, &TermiusRawIdentity> = HashMap::new();
    for identity in identities {
        for alias in termius_record_aliases(&identity.id, identity.local_id.as_deref()) {
            identities_by_key.insert(alias, identity);
        }
        identities_by_key.insert(termius_identity_key(identity), identity);
    }
    let mut connections = Vec::new();

    for host in hosts {
        let Some(address) = normalize_optional_string(host.address.clone()) else {
            continue;
        };
        let ssh_config = host
            .ssh_config_id
            .as_deref()
            .and_then(|id| ssh_configs_by_key.get(id).copied());
        let identity_id = host
            .identity_id
            .as_deref()
            .or_else(|| ssh_config.and_then(|config| config.identity_id.as_deref()));
        let identity = identity_id
            .and_then(|id| identities_by_key.get(id).copied());
        let username = normalize_optional_string(host.username.clone())
            .or_else(|| identity.and_then(|item| normalize_optional_string(item.username.clone())))
            .unwrap_or_else(|| "root".to_string());
        let auth = prepare_termius_auth(&host, identity, key_ids, password_ids);
        let group_path = host
            .group_id
            .as_deref()
            .and_then(|group_id| group_paths.get(group_id).cloned());

        connections.push(PreparedJsonConnection {
            name: normalize_optional_string(host.label.clone()).unwrap_or_else(|| address.clone()),
            config: ConnectionType::Ssh {
                host: address,
                port: host
                    .port
                    .or_else(|| ssh_config.and_then(|config| config.port))
                    .unwrap_or(22)
                    .max(1),
                username,
                backspace_mode: "del".to_string(),
                x11_forwarding: false,
                auth_agent_endpoint: None,
                legacy_agent_forwarding: None,
                agent_forwarding_config: None,
                encoding: String::new(),
                dynamic_tab_title: false,
            },
            group_path,
            description: Some("Imported from Termius".to_string()),
            sort_order: 0,
            icon: None,
            auth: Some(auth),
        });
    }

    Ok(connections)
}

fn prepare_termius_auth(
    host: &TermiusRawHost,
    identity: Option<&TermiusRawIdentity>,
    key_ids: &HashMap<String, String>,
    password_ids: &HashMap<String, String>,
) -> ConnectionAuth {
    if let Some(key_id) = identity
        .and_then(|item| item.ssh_key_id.as_deref())
        .and_then(|key| key_ids.get(key))
    {
        return ConnectionAuth {
            mode: "key".to_string(),
            password_id: None,
            password: None,
            key_id: Some(key_id.clone()),
            otp_id: None,
            auto_fill_otp: false,
            has_password: false,
        };
    }

    let host_password_key = format!("host:{}", termius_host_key(host));
    let identity_password_key = identity.map(|item| format!("identity:{}", termius_identity_key(item)));
    let password_id = password_ids
        .get(&host_password_key)
        .cloned()
        .or_else(|| identity_password_key.and_then(|key| password_ids.get(&key).cloned()));

    ConnectionAuth {
        mode: "password".to_string(),
        password_id,
        password: None,
        key_id: None,
        otp_id: None,
        auto_fill_otp: false,
        has_password: false,
    }
}

#[cfg(test)]
fn encrypt_termius_secret_for_test(value: &str, local_key: &[u8; 32], nonce: &[u8; 24]) -> String {
    let cipher = XSalsa20Poly1305::new(Key::from_slice(local_key));
    let mut bytes = vec![4_u8, 0_u8];
    bytes.extend_from_slice(nonce);
    bytes.extend(
        cipher
            .encrypt(Nonce::from_slice(nonce), value.as_bytes())
            .expect("encrypt termius test value"),
    );
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
fn tagged_string(value: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    let mut length = value.len();
    while length >= 0x80 {
        out.push((length as u8 & 0x7f) | 0x80);
        length >>= 7;
    }
    out.push(length as u8);
    out.extend_from_slice(value.as_bytes());
}

#[cfg(test)]
fn tagged_integer(value: i64, out: &mut Vec<u8>) {
    out.push(b'I');
    let mut raw = ((value << 1) ^ (value >> 63)) as usize;
    while raw >= 0x80 {
        out.push((raw as u8 & 0x7f) | 0x80);
        raw >>= 7;
    }
    out.push(raw as u8);
}
