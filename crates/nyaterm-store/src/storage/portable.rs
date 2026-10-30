//! Portable snapshot and config-database import/export.
//!
//! Split out of `storage.rs` by domain. The on-disk snapshot layout, the
//! encrypted envelope and the merge rules for imported settings are
//! unchanged; this only moves the code.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use redb::{ReadableDatabase, ReadableTable, TableDefinition};
use serde::de::DeserializeOwned;

use super::cloud_sync::{
    cloud_sync_settings_has_secret, decrypt_cloud_sync_settings_secrets,
    encrypt_cloud_sync_settings_secrets,
};
use super::notes::replace_notes_snapshot_in_txn;
use super::vault::bump_ssh_key_revision;
use super::{
    CREDENTIAL_PREFIX, CREDENTIALS_TABLE, ConfigBackupInfo, ConnectionStore, DATABASE_FILE,
    LEGACY_TEXT_MASTER_KEY, META_MASTER_KEY, META_PORTABLE_SOURCE_PAYLOAD_HASH,
    META_PORTABLE_SOURCE_SCHEMA_VERSION, META_TABLE, OTP_ACCOUNTS_TABLE, OTP_PREFIX,
    PASSWORD_PREFIX, PORTABLE_OPAQUE_ENTITIES_TABLE, PROXIES_TABLE, PROXY_PREFIX,
    SETTINGS_CLOUD_SYNC, SETTINGS_DEFAULT, SETTINGS_PROXY_GROUPS, SETTINGS_QUICK_COMMANDS,
    SETTINGS_TABLE, SETTINGS_TUNNEL_GROUPS, SSH_KEY_PREFIX, StorageError, TEXT_DOCS_TABLE,
    TUNNEL_PREFIX, TUNNELS_TABLE, VNC_KNOWN_HOSTS_TABLE, clear_prefix_in_txn,
    connection_inline_password_requires_encryption, copy_config_database, current_time_ms,
    encrypt_ai_settings_secrets, encrypt_string_secret, ensure_not_same_existing_file,
    ensure_parent_dir, entity_key, merge_unknown_json, prepare_connections_for_storage,
    replace_command_history_in_txn, replace_known_hosts_text_in_txn, replace_sessions_in_txn,
    set_nested_json_value, validate_config_backup_file, validate_config_backup_source,
    write_json_in_txn, write_portable_snapshot_file,
};
use crate::{
    decode_encrypted_raw_portable_snapshot, decode_raw_portable_snapshot,
    encode_encrypted_raw_portable_snapshot, encode_raw_portable_snapshot,
};
use nyaterm_core::portable_snapshot::{
    preserve_device_local_sync_settings, project_sync_settings, validate_raw_snapshot,
};
use nyaterm_core::{
    AiSettings, CloudSyncSettings, CommandHistoryEntry, ConnectionType, CredentialCrypto,
    NotesSnapshot, PortableSnapshotKind, RawPortableSnapshot, SessionsConfig, SshAgentEndpoint,
    TranslationSettings, TunnelGroup, ai_settings_has_secret, migrate_legacy_ssh_agent_settings,
    ssh_agent_endpoint_supported_on_current_platform, validate_ssh_agent_endpoint,
    validate_ssh_agent_forwarding_config,
};
use semver::Version;

impl ConnectionStore {
    pub(crate) fn apply_cloud_sync_snapshot(
        &self,
        config_dir: &Path,
        snapshot: &RawPortableSnapshot,
        snapshot_password: &str,
    ) -> Result<Option<PathBuf>, StorageError> {
        let mut current = self.build_raw_portable_snapshot(
            PortableSnapshotKind::Backup,
            "cloud-sync-safety-backup",
            env!("CARGO_PKG_VERSION"),
        )?;
        current.recalculate_hash()?;
        let bytes = encode_raw_portable_snapshot(&current)?;
        let safety_backup_path = config_dir.join(format!(
            "nyaterm.cloud-sync-backup-{}.nya",
            current_time_ms()
        ));
        ensure_parent_dir(&safety_backup_path)?;
        std::fs::write(&safety_backup_path, bytes).map_err(|source| {
            StorageError::ConfigBackupCopy {
                from: self.db_path().to_path_buf(),
                to: safety_backup_path.clone(),
                source,
            }
        })?;

        self.apply_raw_portable_snapshot_with_password(snapshot, Some(snapshot_password))?;
        Ok(Some(safety_backup_path))
    }

    pub fn export_config_database(
        config_dir: impl AsRef<Path>,
        portable_key_path: Option<PathBuf>,
        output_path: impl AsRef<Path>,
    ) -> Result<ConfigBackupInfo, StorageError> {
        let store = Self::open_with_portable_key_path(config_dir, portable_key_path)?;
        let database_path = store.db_path().to_path_buf();
        drop(store);

        let backup_path = output_path.as_ref().to_path_buf();
        ensure_not_same_existing_file(&database_path, &backup_path)?;
        ensure_parent_dir(&backup_path)?;
        let bytes = copy_config_database(&database_path, &backup_path)?;

        Ok(ConfigBackupInfo {
            database_path,
            backup_path,
            bytes,
            safety_backup_path: None,
        })
    }
    pub fn import_config_database(
        config_dir: impl AsRef<Path>,
        portable_key_path: Option<PathBuf>,
        input_path: impl AsRef<Path>,
    ) -> Result<ConfigBackupInfo, StorageError> {
        let config_dir = config_dir.as_ref();
        let input_path = input_path.as_ref().to_path_buf();
        validate_config_backup_source(&input_path)?;

        let database_path = config_dir.join(DATABASE_FILE);
        ensure_not_same_existing_file(&database_path, &input_path)?;
        validate_config_backup_file(&input_path, portable_key_path.clone())?;
        std::fs::create_dir_all(config_dir).map_err(|source| StorageError::CreateDir {
            path: config_dir.to_path_buf(),
            source,
        })?;

        let safety_backup_path = if database_path.exists() {
            let backup_path = config_dir.join(format!(
                "{DATABASE_FILE}.import-backup-{}.redb",
                current_time_ms()
            ));
            copy_config_database(&database_path, &backup_path)?;
            Some(backup_path)
        } else {
            None
        };

        let temp_path =
            config_dir.join(format!("{DATABASE_FILE}.import-{}.tmp", current_time_ms()));
        let bytes = copy_config_database(&input_path, &temp_path)?;
        if database_path.exists() {
            std::fs::remove_file(&database_path).map_err(|source| {
                StorageError::ConfigBackupRemove {
                    path: database_path.clone(),
                    source,
                }
            })?;
        }
        std::fs::rename(&temp_path, &database_path).map_err(|source| {
            if let Some(safety_backup_path) = &safety_backup_path {
                let _ = std::fs::copy(safety_backup_path, &database_path);
            }
            StorageError::ConfigBackupRename {
                from: temp_path,
                to: database_path.clone(),
                source,
            }
        })?;

        let store =
            Self::open_with_portable_key_path(config_dir, portable_key_path).inspect_err(|_| {
                if let Some(safety_backup_path) = &safety_backup_path {
                    let _ = std::fs::copy(safety_backup_path, &database_path);
                }
            })?;
        store.load_sessions()?;
        store.load_app_settings_summary()?;
        store.list_tunnels()?;
        drop(store);

        Ok(ConfigBackupInfo {
            database_path,
            backup_path: input_path,
            bytes,
            safety_backup_path,
        })
    }
    pub fn export_portable_snapshot(
        config_dir: impl AsRef<Path>,
        portable_key_path: Option<PathBuf>,
        output_path: impl AsRef<Path>,
        device_id: impl Into<String>,
        app_version: impl Into<String>,
    ) -> Result<ConfigBackupInfo, StorageError> {
        let store = Self::open_with_portable_key_path(config_dir, portable_key_path)?;
        let database_path = store.db_path().to_path_buf();
        let mut snapshot = store.build_raw_portable_snapshot(
            PortableSnapshotKind::Backup,
            device_id,
            app_version,
        )?;
        snapshot.recalculate_hash()?;
        let encoded = encode_raw_portable_snapshot(&snapshot)?;

        let backup_path = output_path.as_ref().to_path_buf();
        ensure_parent_dir(&backup_path)?;
        std::fs::write(&backup_path, &encoded).map_err(|source| {
            StorageError::ConfigBackupCopy {
                from: database_path.clone(),
                to: backup_path.clone(),
                source,
            }
        })?;

        Ok(ConfigBackupInfo {
            database_path,
            backup_path,
            bytes: encoded.len().try_into().unwrap_or(u64::MAX),
            safety_backup_path: None,
        })
    }
    pub fn export_encrypted_portable_snapshot(
        config_dir: impl AsRef<Path>,
        portable_key_path: Option<PathBuf>,
        output_path: impl AsRef<Path>,
        device_id: impl Into<String>,
        app_version: impl Into<String>,
        master_password: &str,
    ) -> Result<ConfigBackupInfo, StorageError> {
        let store = Self::open_with_portable_key_path(config_dir, portable_key_path)?;
        store.export_encrypted_portable_snapshot_from_open_store(
            output_path,
            device_id,
            app_version,
            master_password,
        )
    }
    pub fn export_encrypted_portable_snapshot_from_open_store(
        &self,
        output_path: impl AsRef<Path>,
        device_id: impl Into<String>,
        app_version: impl Into<String>,
        master_password: &str,
    ) -> Result<ConfigBackupInfo, StorageError> {
        let database_path = self.db_path().to_path_buf();
        let mut snapshot =
            self.build_raw_portable_snapshot(PortableSnapshotKind::Backup, device_id, app_version)?;
        snapshot.recalculate_hash()?;
        let encoded = encode_encrypted_raw_portable_snapshot(&snapshot, master_password)?;
        write_portable_snapshot_file(database_path, output_path, &encoded)
    }
    /// Independent .nya backup for V4 confirmation. Unlike the ordinary legacy
    /// reader, this does not migrate icons or credentials while taking it.
    pub fn export_v4_safety_backup(
        &self,
        output_path: impl AsRef<Path>,
        master_password: &str,
    ) -> Result<ConfigBackupInfo, StorageError> {
        let sessions = self.v4_backup_sessions()?;
        let mut snapshot = self.build_raw_portable_snapshot_with_sessions(
            PortableSnapshotKind::Backup,
            "v4-migration",
            env!("CARGO_PKG_VERSION"),
            Some(sessions),
        )?;
        snapshot.recalculate_hash()?;
        let encoded = encode_encrypted_raw_portable_snapshot(&snapshot, master_password)?;
        write_portable_snapshot_file(self.db_path().to_path_buf(), output_path, &encoded)
    }
    pub fn import_portable_snapshot(
        config_dir: impl AsRef<Path>,
        portable_key_path: Option<PathBuf>,
        input_path: impl AsRef<Path>,
    ) -> Result<ConfigBackupInfo, StorageError> {
        let config_dir = config_dir.as_ref();
        let input_path = input_path.as_ref().to_path_buf();
        validate_config_backup_source(&input_path)?;
        std::fs::create_dir_all(config_dir).map_err(|source| StorageError::CreateDir {
            path: config_dir.to_path_buf(),
            source,
        })?;
        let bytes =
            std::fs::read(&input_path).map_err(|source| StorageError::ConfigBackupCopy {
                from: input_path.clone(),
                to: config_dir.join(DATABASE_FILE),
                source,
            })?;
        let snapshot = decode_raw_portable_snapshot(&bytes)?;

        let database_path = config_dir.join(DATABASE_FILE);
        let safety_backup_path = if database_path.exists() {
            let backup_path = config_dir.join(format!(
                "{DATABASE_FILE}.portable-import-backup-{}.redb",
                current_time_ms()
            ));
            copy_config_database(&database_path, &backup_path)?;
            Some(backup_path)
        } else {
            None
        };

        let store = Self::open_with_portable_key_path(config_dir, portable_key_path)?;
        if let Err(error) = store.apply_raw_portable_snapshot(&snapshot) {
            if let Some(safety_backup_path) = &safety_backup_path {
                let _ = std::fs::copy(safety_backup_path, &database_path);
            }
            return Err(error);
        }
        store.load_sessions()?;
        store.load_app_settings_summary()?;
        store.list_tunnels()?;

        Ok(ConfigBackupInfo {
            database_path,
            backup_path: input_path,
            bytes: bytes.len().try_into().unwrap_or(u64::MAX),
            safety_backup_path,
        })
    }
    pub fn import_encrypted_portable_snapshot(
        config_dir: impl AsRef<Path>,
        portable_key_path: Option<PathBuf>,
        input_path: impl AsRef<Path>,
        master_password: &str,
    ) -> Result<ConfigBackupInfo, StorageError> {
        let store = Self::open_with_portable_key_path(config_dir, portable_key_path)?;
        store.import_encrypted_portable_snapshot_into_open_store(input_path, master_password)
    }
    pub fn import_encrypted_portable_snapshot_into_open_store(
        &self,
        input_path: impl AsRef<Path>,
        master_password: &str,
    ) -> Result<ConfigBackupInfo, StorageError> {
        let input_path = input_path.as_ref().to_path_buf();
        validate_config_backup_source(&input_path)?;
        let bytes =
            std::fs::read(&input_path).map_err(|source| StorageError::ConfigBackupCopy {
                from: input_path.clone(),
                to: self.db_path().to_path_buf(),
                source,
            })?;
        let snapshot = decode_encrypted_raw_portable_snapshot(&bytes, master_password)?;

        let mut current = self.build_raw_portable_snapshot(
            PortableSnapshotKind::Backup,
            "portable-import-safety-backup",
            env!("CARGO_PKG_VERSION"),
        )?;
        current.recalculate_hash()?;
        let safety_bytes = encode_encrypted_raw_portable_snapshot(&current, master_password)?;
        let config_dir = self.db_path().parent().ok_or_else(|| {
            StorageError::InvalidData("configuration database has no parent directory".to_string())
        })?;
        let safety_backup_path = config_dir.join(format!(
            "nyaterm.portable-import-backup-{}.nya",
            current_time_ms()
        ));
        ensure_parent_dir(&safety_backup_path)?;
        std::fs::write(&safety_backup_path, safety_bytes).map_err(|source| {
            StorageError::ConfigBackupCopy {
                from: self.db_path().to_path_buf(),
                to: safety_backup_path.clone(),
                source,
            }
        })?;

        self.apply_raw_portable_snapshot_with_password(&snapshot, Some(master_password))?;
        self.load_sessions()?;
        self.load_app_settings_summary()?;
        self.list_tunnels()?;

        Ok(ConfigBackupInfo {
            database_path: self.db_path().to_path_buf(),
            backup_path: input_path,
            bytes: bytes.len().try_into().unwrap_or(u64::MAX),
            safety_backup_path: Some(safety_backup_path),
        })
    }
    pub(crate) fn build_raw_portable_snapshot(
        &self,
        snapshot_kind: PortableSnapshotKind,
        device_id: impl Into<String>,
        app_version: impl Into<String>,
    ) -> Result<RawPortableSnapshot, StorageError> {
        self.build_raw_portable_snapshot_with_sessions(snapshot_kind, device_id, app_version, None)
    }

    fn build_raw_portable_snapshot_with_sessions(
        &self,
        snapshot_kind: PortableSnapshotKind,
        device_id: impl Into<String>,
        app_version: impl Into<String>,
        sessions_override: Option<serde_json::Value>,
    ) -> Result<RawPortableSnapshot, StorageError> {
        let mut snapshot = match snapshot_kind {
            PortableSnapshotKind::Sync => RawPortableSnapshot::sync(device_id, app_version),
            PortableSnapshotKind::Backup => RawPortableSnapshot::backup(device_id, app_version),
        };
        // Preserve entities introduced by newer NyaTerm versions even when this
        // build has no domain model for them. Known entities below deliberately
        // overwrite same-named opaque values with current authoritative data.
        snapshot
            .entities
            .extend(self.load_opaque_portable_entities()?);
        let mut settings = self.load_settings_value()?;
        set_nested_json_value(
            &mut settings,
            &["security", "master_password"],
            serde_json::Value::Null,
        );
        if snapshot_kind == PortableSnapshotKind::Sync {
            project_sync_settings(&mut settings);
        }

        snapshot
            .entities
            .insert("settings".to_string(), serde_json::to_string(&settings)?);
        let sessions = if let Some(sessions) = sessions_override {
            sessions
        } else {
            let mut sessions = self.load_sessions()?;
            if snapshot_kind == PortableSnapshotKind::Sync {
                strip_device_local_ssh_agent_settings(&mut sessions);
            }
            serde_json::to_value(sessions)?
        };
        snapshot
            .entities
            .insert("sessions".to_string(), serde_json::to_string(&sessions)?);
        snapshot.entities.insert(
            "keys".to_string(),
            wrapped_raw_array_json(
                "keys",
                self.list_raw_json_values_by_prefix(CREDENTIALS_TABLE, SSH_KEY_PREFIX)?,
            )?,
        );
        snapshot.entities.insert(
            "passwords".to_string(),
            wrapped_raw_array_json(
                "passwords",
                self.list_raw_json_values_by_prefix(CREDENTIALS_TABLE, PASSWORD_PREFIX)?,
            )?,
        );
        snapshot.entities.insert(
            "credentials".to_string(),
            wrapped_raw_array_json(
                "credentials",
                self.list_raw_json_values_by_prefix(CREDENTIALS_TABLE, CREDENTIAL_PREFIX)?,
            )?,
        );
        snapshot.entities.insert(
            "otp".to_string(),
            wrapped_raw_array_json(
                "entries",
                self.list_raw_json_values_by_prefix(OTP_ACCOUNTS_TABLE, OTP_PREFIX)?,
            )?,
        );
        snapshot.entities.insert(
            "proxies".to_string(),
            serde_json::to_string(&self.portable_proxy_records()?)?,
        );
        snapshot.entities.insert(
            "proxy_groups".to_string(),
            portable_settings_doc_array(
                self.load_settings_doc_value(SETTINGS_PROXY_GROUPS, serde_json::json!({}))?,
                "groups",
            )?,
        );
        snapshot.entities.insert(
            "tunnels".to_string(),
            serde_json::to_string(
                &self.list_raw_json_values_by_prefix(TUNNELS_TABLE, TUNNEL_PREFIX)?,
            )?,
        );
        snapshot.entities.insert(
            "tunnel_groups".to_string(),
            serde_json::to_string(&self.list_tunnel_groups()?)?,
        );
        snapshot.entities.insert(
            "quick_commands".to_string(),
            serde_json::to_string(&self.load_settings_doc_value(
                SETTINGS_QUICK_COMMANDS,
                serde_json::json!({"commands":[],"categories":[]}),
            )?)?,
        );
        snapshot.entities.insert(
            "history".to_string(),
            serde_json::to_string(&if snapshot_kind == PortableSnapshotKind::Sync {
                Vec::new()
            } else {
                self.list_command_history(usize::MAX)?
            })?,
        );
        snapshot.entities.insert(
            "master_key_token".to_string(),
            serde_json::to_string(&self.load_master_key_token()?)?,
        );
        snapshot.entities.insert(
            "vnc_known_hosts".to_string(),
            serde_json::to_string(
                &self.list_raw_json_values_by_prefix(VNC_KNOWN_HOSTS_TABLE, "vnc_known_hosts/")?,
            )?,
        );
        snapshot.entities.insert(
            "known_hosts".to_string(),
            serde_json::to_string(&self.render_known_hosts_export()?)?,
        );
        snapshot.entities.insert(
            "notes".to_string(),
            serde_json::to_string(&self.load_notes_snapshot()?)?,
        );
        Ok(snapshot)
    }

    fn portable_proxy_records(&self) -> Result<Vec<serde_json::Value>, StorageError> {
        let mut records = self.list_raw_json_values_by_prefix(PROXIES_TABLE, PROXY_PREFIX)?;
        let crypto = self.credential_crypto()?;
        let token = self.load_master_key_token()?;
        for record in &mut records {
            if let Some(ciphertext) = record
                .get("password")
                .and_then(serde_json::Value::as_str)
                .and_then(|value| value.strip_prefix(super::PROXY_SECRET_PREFIX))
            {
                record["password"] = serde_json::Value::String(crypto.decrypt_secret(
                    token.as_deref().ok_or(StorageError::MissingMasterKey)?,
                    ciphertext,
                )?);
            }
        }
        // Released .nya readers expect this field in plaintext inside the
        // independently encrypted backup envelope. Retain that contract and
        // every unknown proxy property, regardless of the local storage codec.
        Ok(records)
    }

    fn load_opaque_portable_entities(&self) -> Result<BTreeMap<String, String>, StorageError> {
        let txn = self.db.begin_read()?;
        let table = match txn.open_table(PORTABLE_OPAQUE_ENTITIES_TABLE) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(BTreeMap::new()),
            Err(error) => return Err(error.into()),
        };
        let mut entities = BTreeMap::new();
        for entry in table.iter()? {
            let (key, value) = entry?;
            entities.insert(key.value().to_string(), value.value().to_string());
        }
        Ok(entities)
    }

    pub(crate) fn apply_raw_portable_snapshot(
        &self,
        snapshot: &RawPortableSnapshot,
    ) -> Result<(), StorageError> {
        self.apply_raw_portable_snapshot_with_password(snapshot, None)
    }

    fn apply_raw_portable_snapshot_with_password(
        &self,
        snapshot: &RawPortableSnapshot,
        snapshot_password: Option<&str>,
    ) -> Result<(), StorageError> {
        validate_raw_snapshot(snapshot)?;
        for (entity, raw) in &snapshot.entities {
            serde_json::from_str::<serde_json::Value>(raw).map_err(|error| {
                StorageError::PortableSnapshotEntity {
                    entity: entity.clone(),
                    message: error.to_string(),
                }
            })?;
        }
        let opaque_entities = snapshot
            .entities
            .iter()
            .filter(|(entity, _)| !is_known_portable_entity(entity))
            .map(|(entity, raw)| (entity.clone(), raw.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut sessions: SessionsConfig = read_snapshot_entity(snapshot, "sessions")?;
        let mut settings: serde_json::Value = read_snapshot_entity(snapshot, "settings")?;
        let known_hosts: String = read_snapshot_entity(snapshot, "known_hosts")?;
        let vnc_known_hosts = snapshot
            .entities
            .get("vnc_known_hosts")
            .map(|raw| {
                serde_json::from_str::<Vec<nyaterm_core::vnc_known_hosts::VncKnownHostRecord>>(raw)
            })
            .transpose()?;
        if let Some(records) = &vnc_known_hosts {
            for record in records {
                super::vnc_known_hosts::validate_record(record)?;
            }
        }
        let mut master_key_token: Option<String> =
            read_snapshot_entity(snapshot, "master_key_token")?;
        let tunnel_groups: Vec<TunnelGroup> = read_snapshot_entity(snapshot, "tunnel_groups")?;
        let notes = snapshot
            .entities
            .get("notes")
            .map(|raw| {
                serde_json::from_str::<NotesSnapshot>(raw).map_err(|error| {
                    StorageError::PortableSnapshotEntity {
                        entity: "notes".to_string(),
                        message: error.to_string(),
                    }
                })
            })
            .transpose()?
            .unwrap_or_default();
        let mut current_settings = self.load_settings_value()?;
        let current_master_key_token = self.load_master_key_token()?;
        if is_legacy_tauri_snapshot(snapshot)
            && let Some(snapshot_password) = snapshot_password
        {
            self.migrate_legacy_tauri_snapshot_secrets(
                &mut settings,
                &mut master_key_token,
                snapshot_password,
            )?;
        } else if let (Some(snapshot_password), Some(token)) =
            (snapshot_password, master_key_token.as_mut())
        {
            let source_crypto = CredentialCrypto::new(
                self.portable_key_path.clone(),
                Some(snapshot_password.to_owned().into()),
            );
            let target_crypto = self.credential_crypto()?;
            *token = source_crypto.rewrap_master_key_token_for(token, &target_crypto)?;
        }
        validate_and_migrate_agent_settings(&mut sessions)?;
        match snapshot.meta.snapshot_kind {
            PortableSnapshotKind::Sync => {
                preserve_device_local_ssh_agent_settings(&mut sessions, &self.load_sessions()?);
            }
            PortableSnapshotKind::Backup => normalize_backup_agent_settings(&mut sessions),
        }

        let crypto = self.credential_crypto()?;
        // Snapshots without a vault key do not replace the device's key. In
        // particular, generating a new key for inline passwords must not strand
        // the retained local provider credentials.
        if master_key_token
            .as_deref()
            .is_none_or(|token| token.trim().is_empty())
        {
            master_key_token = current_master_key_token.clone();
        }
        if master_key_token.is_none()
            && sessions
                .connections
                .iter()
                .any(connection_inline_password_requires_encryption)
        {
            master_key_token = Some(crypto.generate_master_key_token()?);
        }
        let prepared_connections = prepare_connections_for_storage(
            &sessions.connections,
            &crypto,
            master_key_token.as_deref(),
        )?;

        // Read and decrypt retained provider secrets before installing the source
        // vault key. Both their new ciphertext and the key are committed together.
        // Keep the standalone compatibility document usable as well as the
        // authoritative embedded field, preserving unknown fields in each.
        let mut legacy_cloud_sync: Option<serde_json::Value> =
            self.read_json_table(SETTINGS_TABLE, SETTINGS_CLOUD_SYNC)?;
        if current_master_key_token != master_key_token {
            // Partial/older snapshots also retain missing AI and translation
            // fields through the unknown-field merge. Rekey the current side
            // before merging so retained known secrets use the installed key;
            // incoming fields still take precedence.
            if snapshot.meta.snapshot_kind == PortableSnapshotKind::Sync {
                if let Some(value) = current_settings.get("ai") {
                    let mut ai: AiSettings = serde_json::from_value(value.clone())?;
                    self.decrypt_ai_settings(&mut ai)?;
                    encrypt_ai_settings_secrets(&mut ai, &crypto, master_key_token.as_deref())?;
                    replace_settings_field_preserving_unknown(&mut current_settings, "ai", &ai)?;
                }
                if let Some(value) = current_settings.get("translation") {
                    let mut translation: TranslationSettings =
                        serde_json::from_value(value.clone())?;
                    self.decrypt_translation_settings(&mut translation)?;
                    for secret in [
                        &mut translation.deepl_api_key,
                        &mut translation.baidu_app_key,
                        &mut translation.ali_app_key,
                        &mut translation.youdao_app_key,
                    ] {
                        *secret =
                            encrypt_string_secret(&crypto, master_key_token.as_deref(), secret)?;
                    }
                    replace_settings_field_preserving_unknown(
                        &mut current_settings,
                        "translation",
                        &translation,
                    )?;
                }
            }
            if snapshot.meta.snapshot_kind == PortableSnapshotKind::Sync
                && let Some(value) = current_settings.get_mut("cloud_sync")
            {
                reencrypt_cloud_sync_value(
                    value,
                    &crypto,
                    current_master_key_token.as_deref(),
                    master_key_token.as_deref(),
                )?;
            }
            if let Some(value) = legacy_cloud_sync.as_mut() {
                reencrypt_cloud_sync_value(
                    value,
                    &crypto,
                    current_master_key_token.as_deref(),
                    master_key_token.as_deref(),
                )?;
            }
        }
        if snapshot.meta.snapshot_kind == PortableSnapshotKind::Sync
            && current_settings.get("cloud_sync").is_none()
            && let Some(value) = &legacy_cloud_sync
        {
            set_nested_json_value(&mut current_settings, &["cloud_sync"], value.clone());
        }
        nyaterm_core::models::settings::panel_visibility::migrate_legacy_panel_visibility(
            &mut settings,
        )
        .map_err(StorageError::InvalidData)?;
        let merged_settings = merge_imported_settings(
            settings,
            current_settings,
            snapshot.meta.snapshot_kind == PortableSnapshotKind::Sync,
        );

        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(PORTABLE_OPAQUE_ENTITIES_TABLE)?;
            for (entity, raw) in &opaque_entities {
                table.insert(entity.as_str(), raw.as_str())?;
            }
        }
        let source_schema_version = snapshot.meta.schema_version.to_string();
        {
            let mut meta = txn.open_table(META_TABLE)?;
            meta.insert(
                META_PORTABLE_SOURCE_PAYLOAD_HASH,
                snapshot.meta.payload_hash.as_str(),
            )?;
            meta.insert(
                META_PORTABLE_SOURCE_SCHEMA_VERSION,
                source_schema_version.as_str(),
            )?;
        }
        replace_sessions_in_txn(&txn, &sessions, &prepared_connections)?;
        replace_raw_wrapped_array_in_txn(
            &txn,
            CREDENTIALS_TABLE,
            SSH_KEY_PREFIX,
            snapshot,
            "keys",
            "keys",
        )?;
        replace_raw_wrapped_array_in_txn(
            &txn,
            CREDENTIALS_TABLE,
            PASSWORD_PREFIX,
            snapshot,
            "passwords",
            "passwords",
        )?;
        replace_raw_wrapped_array_in_txn(
            &txn,
            CREDENTIALS_TABLE,
            CREDENTIAL_PREFIX,
            snapshot,
            "credentials",
            "credentials",
        )?;
        replace_raw_wrapped_array_in_txn(
            &txn,
            OTP_ACCOUNTS_TABLE,
            OTP_PREFIX,
            snapshot,
            "otp",
            "entries",
        )?;
        replace_raw_array_in_txn(&txn, PROXIES_TABLE, PROXY_PREFIX, snapshot, "proxies")?;
        replace_raw_array_in_txn(&txn, TUNNELS_TABLE, TUNNEL_PREFIX, snapshot, "tunnels")?;
        write_json_in_txn(
            &txn,
            SETTINGS_TABLE,
            SETTINGS_TUNNEL_GROUPS,
            &serde_json::json!({ "groups": tunnel_groups }),
        )?;
        write_settings_doc_from_entity_in_txn(
            &txn,
            SETTINGS_PROXY_GROUPS,
            snapshot,
            "proxy_groups",
            |value| serde_json::json!({ "groups": value }),
        )?;
        write_settings_doc_from_entity_in_txn(
            &txn,
            SETTINGS_QUICK_COMMANDS,
            snapshot,
            "quick_commands",
            std::convert::identity,
        )?;
        if snapshot.meta.snapshot_kind == PortableSnapshotKind::Backup {
            let history: Vec<CommandHistoryEntry> = read_snapshot_entity(snapshot, "history")?;
            replace_command_history_in_txn(&txn, &history)?;
        }
        if snapshot.entities.contains_key("notes") {
            replace_notes_snapshot_in_txn(&txn, &notes).map_err(|error| {
                StorageError::PortableSnapshotEntity {
                    entity: "notes".to_string(),
                    message: error.to_string(),
                }
            })?;
        }

        write_json_in_txn(&txn, SETTINGS_TABLE, SETTINGS_DEFAULT, &merged_settings)?;
        if let Some(value) = legacy_cloud_sync {
            write_json_in_txn(&txn, SETTINGS_TABLE, SETTINGS_CLOUD_SYNC, &value)?;
        }
        match master_key_token {
            Some(token) if !token.trim().is_empty() => {
                txn.open_table(META_TABLE)?
                    .insert(META_MASTER_KEY, token.as_str())?;
                txn.open_table(TEXT_DOCS_TABLE)?
                    .insert(LEGACY_TEXT_MASTER_KEY, token.as_str())?;
            }
            _ => {}
        }
        if !known_hosts.is_empty() {
            replace_known_hosts_text_in_txn(&txn, &known_hosts)?;
        }
        // Missing in older snapshots: retain the device's existing trust records.
        if let Some(records) = vnc_known_hosts {
            clear_prefix_in_txn(&txn, VNC_KNOWN_HOSTS_TABLE, "vnc_known_hosts/")?;
            for record in records {
                let key = super::vnc_known_hosts::key(&record.host, record.port);
                write_json_in_txn(&txn, VNC_KNOWN_HOSTS_TABLE, &key, &record)?;
            }
        }
        txn.commit()?;
        bump_ssh_key_revision();
        Ok(())
    }

    fn migrate_legacy_tauri_snapshot_secrets(
        &self,
        settings: &mut serde_json::Value,
        master_key_token: &mut Option<String>,
        snapshot_password: &str,
    ) -> Result<(), StorageError> {
        let mut ai_settings = settings
            .get("ai")
            .cloned()
            .map(serde_json::from_value::<AiSettings>)
            .transpose()?;
        let mut cloud_sync_settings = settings
            .get("cloud_sync")
            .cloned()
            .map(serde_json::from_value::<CloudSyncSettings>)
            .transpose()?;
        let has_plaintext_secrets = ai_settings.as_ref().is_some_and(ai_settings_has_secret)
            || cloud_sync_settings
                .as_ref()
                .is_some_and(cloud_sync_settings_has_secret);

        let target_crypto = self.credential_crypto()?;
        *master_key_token = match master_key_token.take() {
            Some(token) if !token.trim().is_empty() => {
                let source_crypto = CredentialCrypto::new(
                    self.portable_key_path.clone(),
                    Some(snapshot_password.to_owned().into()),
                );
                Some(source_crypto.rewrap_master_key_token_for(&token, &target_crypto)?)
            }
            _ if has_plaintext_secrets => Some(target_crypto.generate_master_key_token()?),
            _ => None,
        };
        let master_key_token = master_key_token.as_deref();

        if let Some(ai_settings) = &mut ai_settings {
            encrypt_ai_settings_secrets(ai_settings, &target_crypto, master_key_token)?;
            replace_settings_field_preserving_unknown(settings, "ai", ai_settings)?;
        }
        if let Some(cloud_sync_settings) = &mut cloud_sync_settings {
            encrypt_cloud_sync_settings_secrets(
                cloud_sync_settings,
                &target_crypto,
                master_key_token,
            )?;
            replace_settings_field_preserving_unknown(settings, "cloud_sync", cloud_sync_settings)?;
        }
        Ok(())
    }
}

fn reencrypt_cloud_sync_value(
    value: &mut serde_json::Value,
    crypto: &CredentialCrypto,
    old_master_key_token: Option<&str>,
    new_master_key_token: Option<&str>,
) -> Result<(), StorageError> {
    let mut settings: CloudSyncSettings = serde_json::from_value(value.clone())?;
    decrypt_cloud_sync_settings_secrets(&mut settings, crypto, old_master_key_token)?;
    encrypt_cloud_sync_settings_secrets(&mut settings, crypto, new_master_key_token)?;
    let mut replacement = serde_json::to_value(settings)?;
    merge_unknown_json(value, &mut replacement);
    *value = replacement;
    Ok(())
}

fn is_legacy_tauri_snapshot(snapshot: &RawPortableSnapshot) -> bool {
    Version::parse(snapshot.meta.app_version.trim_start_matches('v'))
        .is_ok_and(|version| version.major < 2)
}

fn replace_settings_field_preserving_unknown<T: serde::Serialize>(
    settings: &mut serde_json::Value,
    field: &'static str,
    value: &T,
) -> Result<(), StorageError> {
    let mut replacement = serde_json::to_value(value)?;
    if let Some(current) = settings.get(field) {
        merge_unknown_json(current, &mut replacement);
    }
    set_nested_json_value(settings, &[field], replacement);
    Ok(())
}

fn is_known_portable_entity(entity: &str) -> bool {
    matches!(
        entity,
        "settings"
            | "sessions"
            | "keys"
            | "passwords"
            | "credentials"
            | "otp"
            | "proxies"
            | "proxy_groups"
            | "tunnels"
            | "tunnel_groups"
            | "quick_commands"
            | "history"
            | "master_key_token"
            | "known_hosts"
            | "vnc_known_hosts"
            | "notes"
    )
}

fn strip_device_local_ssh_agent_settings(sessions: &mut SessionsConfig) {
    for connection in &mut sessions.connections {
        if let ConnectionType::Ssh {
            auth_agent_endpoint,
            agent_forwarding_config,
            ..
        } = &mut connection.config
        {
            // Agent endpoints, forwarding sources and allowlists are local to
            // the device and must not overwrite another device during sync.
            *auth_agent_endpoint = Some(SshAgentEndpoint::Auto);
            *agent_forwarding_config = None;
        }
    }
}

fn validate_and_migrate_agent_settings(sessions: &mut SessionsConfig) -> Result<(), StorageError> {
    for connection in &mut sessions.connections {
        migrate_legacy_ssh_agent_settings(connection);
        if let ConnectionType::Ssh {
            auth_agent_endpoint,
            agent_forwarding_config,
            ..
        } = &connection.config
        {
            if let Some(endpoint) = auth_agent_endpoint {
                validate_ssh_agent_endpoint(endpoint).map_err(|error| {
                    StorageError::PortableSnapshotEntity {
                        entity: "sessions".to_string(),
                        message: format!("invalid SSH Agent authentication endpoint: {error:?}"),
                    }
                })?;
            }
            if let Some(config) = agent_forwarding_config {
                validate_ssh_agent_forwarding_config(config).map_err(|error| {
                    StorageError::PortableSnapshotEntity {
                        entity: "sessions".to_string(),
                        message: format!("invalid SSH Agent forwarding configuration: {error:?}"),
                    }
                })?;
            }
        }
    }
    Ok(())
}

fn normalize_backup_agent_settings(sessions: &mut SessionsConfig) {
    for connection in &mut sessions.connections {
        let auth_uses_agent = connection
            .auth
            .as_ref()
            .is_some_and(|auth| auth.mode == "agent");
        let ConnectionType::Ssh {
            auth_agent_endpoint,
            agent_forwarding_config,
            ..
        } = &mut connection.config
        else {
            continue;
        };
        if auth_agent_endpoint
            .as_ref()
            .is_some_and(|endpoint| !ssh_agent_endpoint_supported_on_current_platform(endpoint))
        {
            *auth_agent_endpoint = auth_uses_agent.then_some(SshAgentEndpoint::Auto);
        }
        if let Some(config) = agent_forwarding_config {
            config
                .sources
                .external_agent_endpoints
                .retain(ssh_agent_endpoint_supported_on_current_platform);
            if config.sources.external_agent_endpoints.is_empty() {
                config.sources.external_agent = false;
            }
            if !config.sources.external_agent && !config.sources.stored_keys {
                config.enabled = false;
            }
        }
    }
}

fn preserve_device_local_ssh_agent_settings(incoming: &mut SessionsConfig, local: &SessionsConfig) {
    for connection in &mut incoming.connections {
        let Some(local_connection) = local
            .connections
            .iter()
            .find(|item| item.id == connection.id)
        else {
            continue;
        };
        if let (
            ConnectionType::Ssh {
                auth_agent_endpoint,
                agent_forwarding_config,
                ..
            },
            ConnectionType::Ssh {
                auth_agent_endpoint: local_endpoint,
                agent_forwarding_config: local_forwarding_config,
                ..
            },
        ) = (&mut connection.config, &local_connection.config)
        {
            *auth_agent_endpoint = local_endpoint.clone();
            *agent_forwarding_config = local_forwarding_config.clone();
        }
    }
}

fn wrapped_raw_array_json(
    field: &str,
    values: Vec<serde_json::Value>,
) -> Result<String, StorageError> {
    serde_json::to_string(&serde_json::json!({ field: values })).map_err(StorageError::from)
}

fn portable_settings_doc_array(
    value: serde_json::Value,
    field: &str,
) -> Result<String, StorageError> {
    if value.is_array() {
        return serde_json::to_string(&value).map_err(StorageError::from);
    }
    let values = value
        .get(field)
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
    serde_json::to_string(&values).map_err(StorageError::from)
}

fn read_snapshot_entity<T>(
    snapshot: &RawPortableSnapshot,
    entity: &'static str,
) -> Result<T, StorageError>
where
    T: DeserializeOwned,
{
    let raw = snapshot
        .entities
        .get(entity)
        .ok_or(StorageError::PortableSnapshotEntity {
            entity: entity.to_string(),
            message: "missing entity".to_string(),
        })?;
    serde_json::from_str(raw).map_err(StorageError::from)
}

fn replace_raw_wrapped_array_in_txn(
    txn: &redb::WriteTransaction,
    definition: TableDefinition<&str, &[u8]>,
    prefix: &str,
    snapshot: &RawPortableSnapshot,
    entity: &'static str,
    field: &'static str,
) -> Result<(), StorageError> {
    let value: serde_json::Value = read_snapshot_entity(snapshot, entity)?;
    let values = value
        .get(field)
        .and_then(serde_json::Value::as_array)
        .cloned()
        .ok_or(StorageError::PortableSnapshotEntity {
            entity: entity.to_string(),
            message: format!("expected object field '{field}' to be an array"),
        })?;
    replace_raw_json_values_by_id_in_txn(txn, definition, prefix, entity, values)
}

fn replace_raw_array_in_txn(
    txn: &redb::WriteTransaction,
    definition: TableDefinition<&str, &[u8]>,
    prefix: &str,
    snapshot: &RawPortableSnapshot,
    entity: &'static str,
) -> Result<(), StorageError> {
    let values: Vec<serde_json::Value> = read_snapshot_entity(snapshot, entity)?;
    replace_raw_json_values_by_id_in_txn(txn, definition, prefix, entity, values)
}

fn replace_raw_json_values_by_id_in_txn(
    txn: &redb::WriteTransaction,
    definition: TableDefinition<&str, &[u8]>,
    prefix: &str,
    entity: &'static str,
    values: Vec<serde_json::Value>,
) -> Result<(), StorageError> {
    clear_prefix_in_txn(txn, definition, prefix)?;
    for (index, value) in values.iter().enumerate() {
        let id = value
            .get("id")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or(StorageError::PortableSnapshotEntity {
                entity: entity.to_string(),
                message: format!("entry {index} is missing string id"),
            })?;
        write_json_in_txn(txn, definition, &entity_key(prefix, id), value)?;
    }
    Ok(())
}

fn write_settings_doc_from_entity_in_txn(
    txn: &redb::WriteTransaction,
    key: &str,
    snapshot: &RawPortableSnapshot,
    entity: &'static str,
    wrap: impl FnOnce(serde_json::Value) -> serde_json::Value,
) -> Result<(), StorageError> {
    let value: serde_json::Value = read_snapshot_entity(snapshot, entity)?;
    write_json_in_txn(txn, SETTINGS_TABLE, key, &wrap(value))
}

fn merge_imported_settings(
    mut imported: serde_json::Value,
    current: serde_json::Value,
    sync: bool,
) -> serde_json::Value {
    if sync {
        merge_unknown_json(&current, &mut imported);
        preserve_device_local_sync_settings(&mut imported, &current);
    }
    if let Some(master_password) = current
        .get("security")
        .and_then(|security| security.get("master_password"))
        .cloned()
    {
        set_nested_json_value(
            &mut imported,
            &["security", "master_password"],
            master_password,
        );
    }
    imported
}
