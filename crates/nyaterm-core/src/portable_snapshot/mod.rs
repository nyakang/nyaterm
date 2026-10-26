use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use thiserror::Error;
use zip::write::SimpleFileOptions;

const PORTABLE_SNAPSHOT_SCHEMA_VERSION: u32 = 3;
const SNAPSHOT_ZIP_MANIFEST_NAME: &str = "manifest.json";
const SNAPSHOT_ZIP_PAYLOAD_NAME: &str = "snapshot.redb";
const MAX_COMPRESSED_SNAPSHOT_PAYLOAD_BYTES: u64 = 50 * 1024 * 1024;

mod encryption;
pub use encryption::{decrypt_snapshot_bytes, encrypt_snapshot_bytes};
mod sync_clock;
pub use sync_clock::SnapshotSyncClock;

mod v2;
pub use v2::convert_v2_snapshot;

#[derive(Debug, Error)]
pub enum PortableSnapshotError {
    #[error("master password is not set")]
    MissingMasterPassword,
    #[error("cloud snapshot ciphertext is too short")]
    CiphertextTooShort,
    #[error("unsupported snapshot encryption version {0}")]
    UnsupportedEncryptionVersion(u8),
    #[error("invalid or unsupported snapshot encryption parameters")]
    InvalidEncryptionEnvelope,
    #[error("cloud snapshot encryption failed: {0}")]
    Encrypt(String),
    #[error(
        "cloud snapshot decryption failed: NyaTerm key prefix failed ({new_error}); legacy Dragonfly key prefix failed ({legacy_error})"
    )]
    Decrypt {
        new_error: String,
        legacy_error: String,
    },
    #[error("portable snapshot redb payload is corrupt or incomplete")]
    CorruptPayload,
    #[error("portable snapshot is missing metadata")]
    MissingMetadata,
    #[error("portable snapshot missing entity '{0}'")]
    MissingEntity(&'static str),
    #[error("unsupported portable snapshot version {0}")]
    UnsupportedVersion(u32),
    #[error("portable snapshot payload hash mismatch")]
    PayloadHashMismatch,
    #[error("portable snapshot entity map hash mismatch")]
    EntitiesHashMismatch,
    #[error("portable snapshot codec error: {0}")]
    Codec(String),
    #[error("zip snapshot error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PortableSnapshotKind {
    Sync,
    Backup,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortableSnapshotMeta {
    pub schema_version: u32,
    pub snapshot_kind: PortableSnapshotKind,
    pub revision_id: String,
    pub device_id: String,
    pub created_at_ms: u64,
    pub payload_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entities_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_clock: Option<SnapshotSyncClock>,
    pub app_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RawPortableSnapshot {
    pub meta: PortableSnapshotMeta,
    pub entities: BTreeMap<String, String>,
    #[serde(skip)]
    pub source_payload_hash: Option<String>,
}

impl RawPortableSnapshot {
    pub fn backup(device_id: impl Into<String>, app_version: impl Into<String>) -> Self {
        Self::new(
            PortableSnapshotKind::Backup,
            device_id.into(),
            app_version.into(),
        )
    }

    pub fn sync(device_id: impl Into<String>, app_version: impl Into<String>) -> Self {
        Self::new(
            PortableSnapshotKind::Sync,
            device_id.into(),
            app_version.into(),
        )
    }

    fn new(snapshot_kind: PortableSnapshotKind, device_id: String, app_version: String) -> Self {
        Self {
            meta: PortableSnapshotMeta {
                schema_version: PORTABLE_SNAPSHOT_SCHEMA_VERSION,
                snapshot_kind,
                revision_id: uuid::Uuid::new_v4().to_string(),
                device_id,
                created_at_ms: current_time_ms(),
                payload_hash: String::new(),
                entities_hash: None,
                sync_clock: None,
                app_version,
            },
            entities: default_entities(),
            source_payload_hash: None,
        }
    }

    pub fn recalculate_hash(&mut self) -> Result<(), PortableSnapshotError> {
        self.meta.payload_hash = calculate_v3_raw_payload_hash(&self.entities)?;
        self.meta.entities_hash = Some(calculate_entities_hash(&self.entities)?);
        self.source_payload_hash = None;
        Ok(())
    }

    pub fn source_payload_hash(&self) -> &str {
        self.source_payload_hash
            .as_deref()
            .unwrap_or(&self.meta.payload_hash)
    }

    /// Versioned change detection over every entity, including opaque entities
    /// from newer clients. The historical payload hash remains a wire contract.
    pub fn sync_content_hash(&self) -> Result<String, PortableSnapshotError> {
        Ok(format!(
            "entities-v1:{}",
            calculate_entities_hash(&self.entities)?
        ))
    }
}

const DEVICE_LOCAL_UI_FIELDS: &[&str] = &[
    "open_tabs",
    "left_width",
    "right_width",
    "quick_cmd_height",
    "quick_cmd_category_width",
    "quick_cmd_selected_category",
    "show_quick_cmd_bar",
    "show_serial_send_panel",
    "serial_send_height",
    "zoom_level",
    "transfer_height",
    "notes_expanded_folder_ids",
    "notes_last_selected_node_id",
];

const DEVICE_LOCAL_APPEARANCE_FIELDS: &[&str] = &[
    "window_material",
    "window_background_opacity",
    "background_image_path",
    "background_image_fit",
    "background_image_opacity",
];

const DEVICE_LOCAL_TRANSFER_FIELDS: &[&str] =
    &["download_path", "default_editor", "recording_path"];

pub fn project_sync_settings(settings: &mut serde_json::Value) {
    let Some(root) = settings.as_object_mut() else {
        return;
    };
    root.remove("cloud_sync");
    root.remove("keybindings");
    if let Some(ui) = root
        .get_mut("ui")
        .and_then(serde_json::Value::as_object_mut)
    {
        for field in DEVICE_LOCAL_UI_FIELDS {
            ui.remove(*field);
        }
    }
    if let Some(appearance) = root
        .get_mut("appearance")
        .and_then(serde_json::Value::as_object_mut)
    {
        for field in DEVICE_LOCAL_APPEARANCE_FIELDS {
            appearance.remove(*field);
        }
    }
    if let Some(transfer) = root
        .get_mut("transfer")
        .and_then(serde_json::Value::as_object_mut)
    {
        for field in DEVICE_LOCAL_TRANSFER_FIELDS {
            transfer.remove(*field);
        }
    }
    if let Some(security) = root
        .get_mut("security")
        .and_then(serde_json::Value::as_object_mut)
    {
        security.remove("master_password");
    }
}

pub fn preserve_device_local_sync_settings(
    incoming: &mut serde_json::Value,
    current: &serde_json::Value,
) {
    for field in ["cloud_sync", "keybindings"] {
        if let Some(value) = current.get(field) {
            incoming[field] = value.clone();
        }
    }
    for (section, fields) in [
        ("ui", DEVICE_LOCAL_UI_FIELDS),
        ("appearance", DEVICE_LOCAL_APPEARANCE_FIELDS),
        ("transfer", DEVICE_LOCAL_TRANSFER_FIELDS),
    ] {
        for field in fields {
            if let Some(value) = current.get(section).and_then(|value| value.get(*field)) {
                incoming[section][*field] = value.clone();
            }
        }
    }
}

#[doc(hidden)]
pub fn encode_compressed_snapshot_payload(
    redb_payload: &[u8],
) -> Result<Vec<u8>, PortableSnapshotError> {
    let cursor = Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(cursor);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    zip.start_file(SNAPSHOT_ZIP_MANIFEST_NAME, options)?;
    zip.write_all(
        br#"{"format":"nyaterm-portable-snapshot-zip","version":1,"payload":"snapshot.redb"}"#,
    )?;
    zip.start_file(SNAPSHOT_ZIP_PAYLOAD_NAME, options)?;
    zip.write_all(redb_payload)?;

    let cursor = zip.finish()?;
    Ok(cursor.into_inner())
}

#[doc(hidden)]
pub fn decode_compressed_snapshot_payload(bytes: &[u8]) -> Result<Vec<u8>, PortableSnapshotError> {
    let cursor = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor)?;
    let mut entry = archive.by_name(SNAPSHOT_ZIP_PAYLOAD_NAME)?;
    if entry.size() > MAX_COMPRESSED_SNAPSHOT_PAYLOAD_BYTES {
        return Err(PortableSnapshotError::Zip(
            zip::result::ZipError::InvalidArchive("snapshot payload is too large".into()),
        ));
    }
    let mut payload = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = entry.read(&mut buf)?;
        if n == 0 {
            break;
        }
        payload.extend_from_slice(&buf[..n]);
        if u64::try_from(payload.len()).unwrap_or(u64::MAX) > MAX_COMPRESSED_SNAPSHOT_PAYLOAD_BYTES
        {
            return Err(PortableSnapshotError::Zip(
                zip::result::ZipError::InvalidArchive("snapshot payload is too large".into()),
            ));
        }
    }
    Ok(payload)
}

#[doc(hidden)]
pub fn validate_raw_snapshot(snapshot: &RawPortableSnapshot) -> Result<(), PortableSnapshotError> {
    if snapshot.meta.schema_version != PORTABLE_SNAPSHOT_SCHEMA_VERSION {
        return Err(PortableSnapshotError::UnsupportedVersion(
            snapshot.meta.schema_version,
        ));
    }
    let expected = calculate_v3_raw_payload_hash(&snapshot.entities)?;
    if expected != snapshot.meta.payload_hash {
        return Err(PortableSnapshotError::PayloadHashMismatch);
    }
    if let Some(expected) = snapshot.meta.entities_hash.as_deref()
        && calculate_entities_hash(&snapshot.entities)? != expected
    {
        return Err(PortableSnapshotError::EntitiesHashMismatch);
    }
    Ok(())
}

fn calculate_entities_hash(
    entities: &BTreeMap<String, String>,
) -> Result<String, PortableSnapshotError> {
    let bytes = serde_json::to_vec(entities)?;
    Ok(hex_encode(&Sha256::digest(bytes)))
}

fn calculate_v3_raw_payload_hash(
    entities: &BTreeMap<String, String>,
) -> Result<String, PortableSnapshotError> {
    let settings = read_raw_entity(entities, "settings")?;
    let sessions = read_raw_entity(entities, "sessions")?;
    let keys = read_raw_entity(entities, "keys")?;
    let passwords = read_raw_entity(entities, "passwords")?;
    let credentials = read_raw_entity(entities, "credentials")?;
    let otp = read_raw_entity(entities, "otp")?;
    let proxies = read_raw_entity(entities, "proxies")?;
    let tunnels = read_raw_entity(entities, "tunnels")?;
    let quick_commands = read_raw_entity(entities, "quick_commands")?;
    let history = read_raw_entity(entities, "history")?;
    let master_key_token = read_raw_entity(entities, "master_key_token")?;
    let known_hosts = read_raw_entity(entities, "known_hosts")?;

    if entities.contains_key("proxy_groups") || entities.contains_key("tunnel_groups") {
        let proxy_groups = read_raw_entity(entities, "proxy_groups")?;
        let tunnel_groups = read_raw_entity(entities, "tunnel_groups")?;
        if entities.contains_key("notes") {
            let notes = read_raw_entity(entities, "notes")?;
            let payload_bytes = serde_json::to_vec(&SnapshotRawHashInputWithNotes {
                settings: settings.as_ref(),
                sessions: sessions.as_ref(),
                keys: keys.as_ref(),
                passwords: passwords.as_ref(),
                credentials: credentials.as_ref(),
                otp: otp.as_ref(),
                proxies: proxies.as_ref(),
                proxy_groups: proxy_groups.as_ref(),
                tunnels: tunnels.as_ref(),
                tunnel_groups: tunnel_groups.as_ref(),
                quick_commands: quick_commands.as_ref(),
                history: history.as_ref(),
                master_key_token: master_key_token.as_ref(),
                known_hosts: known_hosts.as_ref(),
                notes: notes.as_ref(),
            })?;
            return Ok(hex_encode(&Sha256::digest(&payload_bytes)));
        }
        let payload_bytes = serde_json::to_vec(&SnapshotRawHashInput {
            settings: settings.as_ref(),
            sessions: sessions.as_ref(),
            keys: keys.as_ref(),
            passwords: passwords.as_ref(),
            credentials: credentials.as_ref(),
            otp: otp.as_ref(),
            proxies: proxies.as_ref(),
            proxy_groups: proxy_groups.as_ref(),
            tunnels: tunnels.as_ref(),
            tunnel_groups: tunnel_groups.as_ref(),
            quick_commands: quick_commands.as_ref(),
            history: history.as_ref(),
            master_key_token: master_key_token.as_ref(),
            known_hosts: known_hosts.as_ref(),
        })?;
        return Ok(hex_encode(&Sha256::digest(&payload_bytes)));
    }

    let payload_bytes = serde_json::to_vec(&LegacySnapshotRawHashInput {
        settings: settings.as_ref(),
        sessions: sessions.as_ref(),
        keys: keys.as_ref(),
        passwords: passwords.as_ref(),
        credentials: credentials.as_ref(),
        otp: otp.as_ref(),
        proxies: proxies.as_ref(),
        tunnels: tunnels.as_ref(),
        quick_commands: quick_commands.as_ref(),
        history: history.as_ref(),
        master_key_token: master_key_token.as_ref(),
        known_hosts: known_hosts.as_ref(),
    })?;
    Ok(hex_encode(&Sha256::digest(&payload_bytes)))
}

fn read_raw_entity(
    entities: &BTreeMap<String, String>,
    key: &'static str,
) -> Result<Box<RawValue>, PortableSnapshotError> {
    let raw = entities
        .get(key)
        .ok_or(PortableSnapshotError::MissingEntity(key))?;
    Ok(RawValue::from_string(raw.clone())?)
}

fn default_entities() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("settings".to_string(), default_portable_settings_json()),
        (
            "sessions".to_string(),
            r#"{"groups":[],"connections":[]}"#.to_string(),
        ),
        ("keys".to_string(), r#"{"keys":[]}"#.to_string()),
        ("passwords".to_string(), r#"{"passwords":[]}"#.to_string()),
        (
            "credentials".to_string(),
            r#"{"credentials":[]}"#.to_string(),
        ),
        ("otp".to_string(), r#"{"entries":[]}"#.to_string()),
        ("proxies".to_string(), "[]".to_string()),
        ("proxy_groups".to_string(), "[]".to_string()),
        ("tunnels".to_string(), "[]".to_string()),
        ("tunnel_groups".to_string(), "[]".to_string()),
        (
            "quick_commands".to_string(),
            r#"{"commands":[],"categories":[]}"#.to_string(),
        ),
        ("history".to_string(), "[]".to_string()),
        ("master_key_token".to_string(), "null".to_string()),
        ("known_hosts".to_string(), r#""""#.to_string()),
    ])
}

fn default_portable_settings_json() -> String {
    serde_json::json!({
        "general": {
            "startup_restore": false,
            "startup_restore_window_layout": true,
            "confirm_on_close": true
        },
        "appearance": {
            "theme": "github-dark",
            "font_family": "JetBrains Mono",
            "font_size": 16.0
        },
        "proxy": {},
        "search": {},
        "translation": {
            "target_language": "zh-CN"
        },
        "security": {
            "use_os_keyring": true,
            "enable_screen_lock": false,
            "idle_lock_minutes": 0,
            "master_password": null,
            "host_key_policy": "prompt"
        },
        "terminal": {},
        "interaction": {},
        "transfer": {
            "duplicate_strategy": "ask"
        },
        "diagnostics": {},
        "ai": {},
        "ui": {
            "language": null,
            "show_remote_stats": false,
            "remote_stats_interval": 5,
            "saved_connections_sort_mode": "manual",
            "activity_bar_layout": "default"
        }
    })
    .to_string()
}

#[doc(hidden)]
pub fn is_zip_snapshot_payload(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[derive(Serialize)]
struct SnapshotRawHashInput<'a> {
    settings: &'a RawValue,
    sessions: &'a RawValue,
    keys: &'a RawValue,
    passwords: &'a RawValue,
    credentials: &'a RawValue,
    otp: &'a RawValue,
    proxies: &'a RawValue,
    proxy_groups: &'a RawValue,
    tunnels: &'a RawValue,
    tunnel_groups: &'a RawValue,
    quick_commands: &'a RawValue,
    history: &'a RawValue,
    master_key_token: &'a RawValue,
    known_hosts: &'a RawValue,
}

#[derive(Serialize)]
struct SnapshotRawHashInputWithNotes<'a> {
    settings: &'a RawValue,
    sessions: &'a RawValue,
    keys: &'a RawValue,
    passwords: &'a RawValue,
    credentials: &'a RawValue,
    otp: &'a RawValue,
    proxies: &'a RawValue,
    proxy_groups: &'a RawValue,
    tunnels: &'a RawValue,
    tunnel_groups: &'a RawValue,
    quick_commands: &'a RawValue,
    history: &'a RawValue,
    master_key_token: &'a RawValue,
    known_hosts: &'a RawValue,
    notes: &'a RawValue,
}

#[derive(Serialize)]
struct LegacySnapshotRawHashInput<'a> {
    settings: &'a RawValue,
    sessions: &'a RawValue,
    keys: &'a RawValue,
    passwords: &'a RawValue,
    credentials: &'a RawValue,
    otp: &'a RawValue,
    proxies: &'a RawValue,
    tunnels: &'a RawValue,
    quick_commands: &'a RawValue,
    history: &'a RawValue,
    master_key_token: &'a RawValue,
    known_hosts: &'a RawValue,
}

#[cfg(test)]
mod tests;
