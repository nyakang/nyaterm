use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use thiserror::Error;

use nyaterm_core::{
    AiSettings, AssetMetadata, ConnectionType, CredentialCrypto, CredentialCryptoError, Group,
    KeywordHighlightConfig, KeywordHighlightImportResult, KeywordHighlightRule,
    PortableSnapshotError, ProxyConfig, ProxyGroup, ProxyGroupsConfig, QuickCommand,
    QuickCommandCategory, QuickCommandsConfig, SavedConnection, SessionsConfig,
    TranslationSettings, TunnelConfig, TunnelGroup, TunnelGroupsConfig, ai_settings_has_secret,
    merge_masked_ai_settings, merge_masked_translation_settings, migrate_legacy_ssh_agent_settings,
    normalize_ai_settings, translation_settings_has_secret, uuid,
    validate_ssh_agent_forwarding_config,
};

mod ai_history;
mod app_settings;
mod cloud_sync;
mod command_history;
mod config_backup;
mod keyword_highlights;
mod known_hosts;
mod notes;
mod portable;
mod remote_file_backend;
mod session_import;
mod vault;
mod window_state;

use self::command_history::replace_command_history_in_txn;
pub use self::config_backup::ConfigBackupInfo;
use self::config_backup::{
    copy_config_database, ensure_not_same_existing_file, ensure_parent_dir,
    validate_config_backup_source, write_portable_snapshot_file,
};
use self::keyword_highlights::{
    merge_keyword_highlight_rules, normalize_keyword_highlight_rule, parse_keyword_highlight_import,
};
use self::known_hosts::replace_known_hosts_text_in_txn;
pub use self::known_hosts::{KnownHostCheck, RdpCertificateMetadata, RdpKnownHostCheck};
pub use self::remote_file_backend::{RemoteFileBackendCache, RemoteFileBackendCacheEntry};

const DATABASE_FILE: &str = "nyaterm.redb";
const GROUP_PREFIX: &str = "groups/";
const CONNECTION_PREFIX: &str = "connections/";
const TUNNEL_PREFIX: &str = "tunnels/";
const SSH_KEY_PREFIX: &str = "credentials/key/";
const CREDENTIAL_PREFIX: &str = "credentials/credential/";
const PASSWORD_PREFIX: &str = "credentials/password/";
const CONNECTION_PASSWORD_PREFIX: &str = "credentials/connection-password/";
const SSH_KEY_FILE_IMPORT_MAX_BYTES: u64 = 1024 * 1024;
const OTP_PREFIX: &str = "otp_accounts/";
const PROXY_PREFIX: &str = "proxies/";
const KNOWN_HOST_PREFIX: &str = "known_hosts/";
const KNOWN_HOST_RAW_PREFIX: &str = "known_hosts/raw/";
const RDP_KNOWN_HOST_PREFIX: &str = "rdp_known_hosts/";
const COMMAND_HISTORY_PREFIX: &str = "command_history/";
const NOTE_FOLDER_PREFIX: &str = "note_folders/";
const NOTE_DOCUMENT_PREFIX: &str = "notes/";
const NOTE_SUMMARY_PREFIX: &str = "note_summaries/";
const NOTE_SNAPSHOT_EXTRA_KEY: &str = "snapshot_extra";
const META_NOTE_SUMMARY_INDEX_VERSION: &str = "notes/summary_index_version";
const META_MASTER_KEY: &str = "security/master_key";
const META_PORTABLE_SOURCE_PAYLOAD_HASH: &str = "portable_snapshot/source_payload_hash";
const META_PORTABLE_SOURCE_SCHEMA_VERSION: &str = "portable_snapshot/source_schema_version";
const LEGACY_TEXT_MASTER_KEY: &str = "master.key";
const LEGACY_TEXT_KNOWN_HOSTS: &str = "known_hosts";
const SETTINGS_DEFAULT: &str = "settings/default";
const SETTINGS_AI_FIELD: &str = "ai";
const SETTINGS_TRANSLATION_FIELD: &str = "translation";
const SETTINGS_AI_HISTORY: &str = "settings/doc/ai-history";
const SETTINGS_AI_AUDIT: &str = "settings/doc/ai-audit";
const SETTINGS_TUNNEL_GROUPS: &str = "settings/doc/tunnel-groups";
const SETTINGS_PROXY_GROUPS: &str = "settings/doc/proxy-groups";
const SETTINGS_CLOUD_SYNC: &str = "settings/doc/cloud-sync";
const SETTINGS_QUICK_COMMANDS: &str = "settings/doc/quick-command";
const SETTINGS_CLOUD_SYNC_STATE: &str = "settings/doc/cloud-sync-state";
const SETTINGS_REMOTE_FILE_BACKEND_CACHE: &str = "settings/doc/file-backend-cache";
const SETTINGS_MAIN_WINDOW_STATE: &str = "settings/window_state";
const LEGACY_TEXT_CLOUD_SYNC_STATE: &str = "cloud-sync-state";
const LEGACY_TEXT_REMOTE_FILE_BACKEND_CACHE: &str = "file-backend-cache";

const META_TABLE: TableDefinition<&str, &str> = TableDefinition::new("meta");
const TEXT_DOCS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("text_docs");
const GROUPS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("groups");
const CONNECTIONS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("connections");
const TUNNELS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("tunnels");
const PROXIES_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("proxies");
const CREDENTIALS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("credentials");
const OTP_ACCOUNTS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("otp_accounts");
const KNOWN_HOSTS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("known_hosts");
const RDP_KNOWN_HOSTS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("rdp_known_hosts");
const PORTABLE_OPAQUE_ENTITIES_TABLE: TableDefinition<&str, &str> =
    TableDefinition::new("portable_snapshot_opaque_entities");
const COMMAND_HISTORY_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("command_history");
const NOTE_FOLDERS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("note_folders");
const NOTES_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("notes");
const NOTE_SUMMARIES_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("note_summaries");
const SETTINGS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("settings");
const IDX_CONNECTIONS_BY_GROUP_TABLE: TableDefinition<&str, &str> =
    TableDefinition::new("idx_connections_by_group");
const IDX_CONNECTIONS_BY_LAST_USED_TABLE: TableDefinition<&str, &str> =
    TableDefinition::new("idx_connections_by_last_used");
const IDX_CONNECTIONS_BY_PROTOCOL_TABLE: TableDefinition<&str, &str> =
    TableDefinition::new("idx_connections_by_protocol");

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("failed to create storage directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to open redb database {path}: {source}")]
    Open {
        path: PathBuf,
        source: redb::DatabaseError,
    },
    #[error("redb table error: {0}")]
    Table(#[from] redb::TableError),
    #[error("redb storage error: {0}")]
    Storage(#[from] redb::StorageError),
    #[error("redb transaction error: {0}")]
    Transaction(#[from] redb::TransactionError),
    #[error("redb commit error: {0}")]
    Commit(#[from] redb::CommitError),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("portable snapshot error: {0}")]
    PortableSnapshot(#[from] PortableSnapshotError),
    #[error("portable snapshot entity '{entity}' is invalid: {message}")]
    PortableSnapshotEntity { entity: String, message: String },
    #[error("credential crypto error: {0}")]
    Crypto(#[from] CredentialCryptoError),
    #[error("encrypted credential material exists but master key is missing")]
    MissingMasterKey,
    #[error("configuration backup does not exist: {path}")]
    ConfigBackupMissing { path: PathBuf },
    #[error("configuration backup path is not a file: {path}")]
    ConfigBackupNotFile { path: PathBuf },
    #[error("configuration backup source and destination are the same path: {path}")]
    ConfigBackupSamePath { path: PathBuf },
    #[error("failed to copy configuration database from {from} to {to}: {source}")]
    ConfigBackupCopy {
        from: PathBuf,
        to: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to remove configuration database {path}: {source}")]
    ConfigBackupRemove {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to rename configuration database from {from} to {to}: {source}")]
    ConfigBackupRename {
        from: PathBuf,
        to: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid data: {0}")]
    InvalidData(String),
}

#[derive(Debug)]
pub struct ConnectionStore {
    db: Database,
    db_path: PathBuf,
    portable_key_path: Option<PathBuf>,
}

#[derive(Clone, Serialize, Deserialize)]
struct ConnectionPasswordRecord {
    id: String,
    connection_id: String,
    password: nyaterm_core::SecretString,
    created_at_ms: u64,
    updated_at_ms: u64,
}

impl ConnectionStore {
    pub fn open(config_dir: impl AsRef<Path>) -> Result<Self, StorageError> {
        Self::open_with_portable_key_path(config_dir, None)
    }

    pub fn open_with_portable_key_path(
        config_dir: impl AsRef<Path>,
        portable_key_path: Option<PathBuf>,
    ) -> Result<Self, StorageError> {
        let config_dir = config_dir.as_ref();
        std::fs::create_dir_all(config_dir).map_err(|source| StorageError::CreateDir {
            path: config_dir.to_path_buf(),
            source,
        })?;
        let db_path = config_dir.join(DATABASE_FILE);
        let db = Database::create(&db_path).map_err(|source| StorageError::Open {
            path: db_path.clone(),
            source,
        })?;
        let store = Self {
            db,
            db_path,
            portable_key_path,
        };
        store.ensure_tables()?;
        store.migrate_legacy_rdp_known_hosts()?;
        store.import_legacy_known_hosts_if_needed()?;
        Ok(store)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn load_sessions(&self) -> Result<SessionsConfig, StorageError> {
        let groups = self.list_groups()?;
        let mut connections = self.list_connections()?;
        self.hydrate_connection_passwords(&mut connections)?;
        let mut custom_icons = self.list_connection_custom_icons()?;
        for connection in &mut connections {
            let Some(value) = connection.icon.as_deref() else {
                continue;
            };
            let Some(icon) =
                nyaterm_core::models::sessions::ConnectionCustomIcon::from_legacy_data_url(
                    value,
                    connection.name.clone(),
                    current_time_ms(),
                )
            else {
                continue;
            };
            if !custom_icons.iter().any(|existing| existing.id == icon.id) {
                self.save_connection_custom_icon(&icon)?;
                custom_icons.push(icon.clone());
            }
            connection.icon = Some(icon.id);
        }
        Ok(SessionsConfig {
            custom_icons,
            groups,
            connections,
        })
    }

    pub fn list_connection_custom_icons(
        &self,
    ) -> Result<Vec<nyaterm_core::models::sessions::ConnectionCustomIcon>, StorageError> {
        let mut icons: Vec<nyaterm_core::models::sessions::ConnectionCustomIcon> =
            self.list_json_by_prefix(CONNECTIONS_TABLE, "connection_custom_icons/")?;
        icons.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        Ok(icons)
    }

    pub fn save_connection_custom_icon(
        &self,
        icon: &nyaterm_core::models::sessions::ConnectionCustomIcon,
    ) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        write_json_in_txn(
            &txn,
            CONNECTIONS_TABLE,
            &entity_key("connection_custom_icons/", &icon.id),
            icon,
        )?;
        txn.commit()?;
        Ok(())
    }

    pub fn delete_connection_custom_icon(&self, id: &str) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        txn.open_table(CONNECTIONS_TABLE)?
            .remove(entity_key("connection_custom_icons/", id).as_str())?;
        let connections = {
            let table = txn.open_table(CONNECTIONS_TABLE)?;
            let mut values = Vec::new();
            for row in table.iter()? {
                let (key, bytes) = row?;
                if key.value().starts_with(CONNECTION_PREFIX) {
                    values.push((
                        key.value().to_string(),
                        deserialize_json::<SavedConnection>(bytes.value())?,
                    ));
                }
            }
            values
        };
        for (key, mut connection) in connections {
            let matches = connection.icon.as_deref().is_some_and(|value| {
                value == id
                    || nyaterm_core::models::sessions::ConnectionCustomIcon::from_legacy_data_url(
                        value,
                        String::new(),
                        0,
                    )
                    .is_some_and(|icon| icon.id == id)
            });
            if matches {
                connection.icon = None;
                write_json_in_txn(&txn, CONNECTIONS_TABLE, &key, &connection)?;
            }
        }
        txn.commit()?;
        Ok(())
    }

    pub fn list_tunnels(&self) -> Result<Vec<TunnelConfig>, StorageError> {
        let mut tunnels: Vec<TunnelConfig> =
            self.list_json_by_prefix(TUNNELS_TABLE, TUNNEL_PREFIX)?;
        tunnels.sort_by(|left, right| {
            left.group_id
                .cmp(&right.group_id)
                .then(left.name.cmp(&right.name))
                .then(left.id.cmp(&right.id))
        });
        Ok(tunnels)
    }

    pub fn list_tunnel_groups(&self) -> Result<Vec<TunnelGroup>, StorageError> {
        let value = self.load_settings_doc_value(SETTINGS_TUNNEL_GROUPS, serde_json::json!({}))?;
        let mut groups: Vec<TunnelGroup> =
            serde_json::from_value::<TunnelGroupsConfig>(value)?.groups;
        groups.sort_by(|left, right| {
            left.sort_order
                .cmp(&right.sort_order)
                .then(left.name.cmp(&right.name))
                .then(left.id.cmp(&right.id))
        });
        Ok(groups)
    }

    pub fn replace_tunnels(&self, tunnels: &[TunnelConfig]) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        clear_prefix_in_txn(&txn, TUNNELS_TABLE, TUNNEL_PREFIX)?;
        for tunnel in tunnels {
            write_json_in_txn(
                &txn,
                TUNNELS_TABLE,
                &entity_key(TUNNEL_PREFIX, &tunnel.id),
                tunnel,
            )?;
        }
        txn.commit()?;
        Ok(())
    }

    pub fn replace_tunnel_groups(&self, groups: &[TunnelGroup]) -> Result<(), StorageError> {
        self.save_settings_doc_value(
            SETTINGS_TUNNEL_GROUPS,
            &serde_json::json!({ "groups": groups }),
        )
    }

    pub fn list_proxies(&self) -> Result<Vec<ProxyConfig>, StorageError> {
        let mut proxies: Vec<ProxyConfig> =
            self.list_json_by_prefix(PROXIES_TABLE, PROXY_PREFIX)?;
        proxies.sort_by(|left, right| {
            left.group_id
                .cmp(&right.group_id)
                .then(left.name.cmp(&right.name))
                .then(left.id.cmp(&right.id))
        });
        Ok(proxies)
    }

    pub fn list_proxy_groups(&self) -> Result<Vec<ProxyGroup>, StorageError> {
        let value = self.load_settings_doc_value(SETTINGS_PROXY_GROUPS, serde_json::json!({}))?;
        let mut groups: Vec<ProxyGroup> =
            serde_json::from_value::<ProxyGroupsConfig>(value)?.groups;
        groups.sort_by(|left, right| {
            left.sort_order
                .cmp(&right.sort_order)
                .then(left.name.cmp(&right.name))
                .then(left.id.cmp(&right.id))
        });
        Ok(groups)
    }

    pub fn replace_proxies(&self, proxies: &[ProxyConfig]) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        clear_prefix_in_txn(&txn, PROXIES_TABLE, PROXY_PREFIX)?;
        for proxy in proxies {
            write_json_in_txn(
                &txn,
                PROXIES_TABLE,
                &entity_key(PROXY_PREFIX, &proxy.id),
                proxy,
            )?;
        }
        txn.commit()?;
        Ok(())
    }

    pub fn replace_proxy_groups(&self, groups: &[ProxyGroup]) -> Result<(), StorageError> {
        self.save_settings_doc_value(
            SETTINGS_PROXY_GROUPS,
            &serde_json::json!({ "groups": groups }),
        )
    }

    pub fn replace_sessions(&self, config: &SessionsConfig) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        replace_sessions_in_txn(&txn, config)?;
        txn.commit()?;
        Ok(())
    }

    pub fn list_groups(&self) -> Result<Vec<Group>, StorageError> {
        let mut groups = self.list_json_by_prefix(GROUPS_TABLE, GROUP_PREFIX)?;
        groups.sort_by(|left: &Group, right| {
            left.sort_order
                .cmp(&right.sort_order)
                .then(left.name.cmp(&right.name))
                .then(left.id.cmp(&right.id))
        });
        Ok(groups)
    }

    pub fn save_group(&self, group: &Group) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        save_group_in_txn(&txn, group)?;
        txn.commit()?;
        Ok(())
    }

    pub fn delete_group(&self, group_id: &str) -> Result<(), StorageError> {
        let groups = self.list_groups()?;
        let connections = self.list_connections()?;
        let mut group_ids = std::collections::HashSet::from([group_id.to_string()]);
        let mut changed = true;
        while changed {
            changed = false;
            for group in &groups {
                if group
                    .parent_id
                    .as_ref()
                    .is_some_and(|parent| group_ids.contains(parent))
                    && group_ids.insert(group.id.clone())
                {
                    changed = true;
                }
            }
        }

        let txn = self.db.begin_write()?;
        for connection in connections.iter().filter(|connection| {
            connection
                .group_id
                .as_ref()
                .is_some_and(|id| group_ids.contains(id))
        }) {
            delete_connection_in_txn(&txn, &connection.id)?;
        }
        for id in group_ids {
            txn.open_table(GROUPS_TABLE)?
                .remove(entity_key(GROUP_PREFIX, &id).as_str())?;
        }
        txn.commit()?;
        Ok(())
    }

    pub fn list_connections(&self) -> Result<Vec<SavedConnection>, StorageError> {
        let mut connections = self.list_json_by_prefix(CONNECTIONS_TABLE, CONNECTION_PREFIX)?;
        for connection in &mut connections {
            migrate_legacy_ssh_agent_settings(connection);
        }
        sort_connections(&mut connections);
        Ok(connections)
    }

    pub fn get_connection(
        &self,
        connection_id: &str,
    ) -> Result<Option<SavedConnection>, StorageError> {
        let txn = self.db.begin_read()?;
        let table = match txn.open_table(CONNECTIONS_TABLE) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let key = entity_key(CONNECTION_PREFIX, connection_id);
        let Some(raw) = table.get(key.as_str())? else {
            return Ok(None);
        };
        let mut connection: SavedConnection = deserialize_json(raw.value())?;
        migrate_legacy_ssh_agent_settings(&mut connection);
        drop(table);
        drop(txn);
        self.hydrate_connection_passwords(std::slice::from_mut(&mut connection))?;
        Ok(Some(connection))
    }

    pub fn save_connection(&self, connection: &SavedConnection) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        save_connection_in_txn(&txn, connection)?;
        txn.commit()?;
        Ok(())
    }

    pub fn save_group_and_connection(
        &self,
        group: &Group,
        connection: &SavedConnection,
    ) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        save_group_in_txn(&txn, group)?;
        save_connection_in_txn(&txn, connection)?;
        txn.commit()?;
        Ok(())
    }

    pub fn delete_connection(&self, connection_id: &str) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        delete_connection_in_txn(&txn, connection_id)?;
        txn.commit()?;
        Ok(())
    }

    pub fn mark_connection_used(&self, connection_id: &str) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        let key = entity_key(CONNECTION_PREFIX, connection_id);
        let mut connection = {
            let table = txn.open_table(CONNECTIONS_TABLE)?;
            let Some(raw) = table.get(key.as_str())? else {
                return Ok(());
            };
            deserialize_json::<SavedConnection>(raw.value())?
        };
        migrate_legacy_ssh_agent_settings(&mut connection);
        connection.last_used_at_ms = Some(current_time_ms());
        connection.updated_at_ms = Some(current_time_ms());
        write_json_in_txn(&txn, CONNECTIONS_TABLE, &key, &connection)?;
        remove_connection_index_entries(&txn, connection_id)?;
        insert_connection_indexes(&txn, &connection)?;
        txn.commit()?;
        Ok(())
    }

    /// Merges a monitoring-derived asset patch into one connection atomically.
    ///
    /// Reads the stored connection, applies [`AssetMetadata::merge_monitoring_patch`],
    /// and writes it back inside a single write transaction so concurrent
    /// monitoring updates cannot interleave a lost update. Returns `Ok(true)`
    /// only when the merge actually changed the persisted asset, so callers can
    /// skip redundant change notifications; a missing connection yields
    /// `Ok(false)` rather than an error, matching the "best effort" nature of
    /// background monitoring. The connection's own inline password lives in a
    /// separate table and is untouched here.
    pub fn merge_connection_asset_from_monitoring(
        &self,
        connection_id: &str,
        patch: AssetMetadata,
    ) -> Result<bool, StorageError> {
        let txn = self.db.begin_write()?;
        let key = entity_key(CONNECTION_PREFIX, connection_id);
        let mut connection = {
            let table = txn.open_table(CONNECTIONS_TABLE)?;
            let Some(raw) = table.get(key.as_str())? else {
                return Ok(false);
            };
            deserialize_json::<SavedConnection>(raw.value())?
        };
        migrate_legacy_ssh_agent_settings(&mut connection);

        let mut next_asset = connection.asset.clone().unwrap_or_default();
        next_asset.merge_monitoring_patch(patch);
        if connection.asset.as_ref() == Some(&next_asset) {
            return Ok(false);
        }
        connection.asset = Some(next_asset);
        connection.updated_at_ms = Some(current_time_ms());

        write_json_in_txn(&txn, CONNECTIONS_TABLE, &key, &connection)?;
        remove_connection_index_entries(&txn, connection_id)?;
        insert_connection_indexes(&txn, &connection)?;
        txn.commit()?;
        Ok(true)
    }

    pub fn load_translation_settings(&self) -> Result<TranslationSettings, StorageError> {
        let value = self.load_settings_value()?;
        let mut settings = value
            .get(SETTINGS_TRANSLATION_FIELD)
            .cloned()
            .map(serde_json::from_value::<TranslationSettings>)
            .transpose()?
            .unwrap_or_default();
        self.decrypt_translation_settings(&mut settings)?;
        if settings.target_language.trim().is_empty() {
            settings.target_language = TranslationSettings::default().target_language;
        }
        Ok(settings)
    }

    pub fn save_translation_settings(
        &self,
        next: TranslationSettings,
    ) -> Result<TranslationSettings, StorageError> {
        let current = self.load_translation_settings()?;
        let mut merged = merge_masked_translation_settings(&current, next);
        if merged.target_language.trim().is_empty() {
            merged.target_language = TranslationSettings::default().target_language;
        }
        let encrypted = self.encrypt_translation_settings(merged.clone())?;
        let mut value = self.load_settings_value()?;
        set_nested_json_value(
            &mut value,
            &[SETTINGS_TRANSLATION_FIELD],
            serde_json::to_value(encrypted)?,
        );
        self.save_settings_value(&value)?;
        Ok(merged)
    }

    pub fn load_keyword_highlights(&self) -> Result<KeywordHighlightConfig, StorageError> {
        let value = self.load_settings_value()?;
        let rules = json_path(&value, &["terminal", "keyword_highlights"])
            .cloned()
            .map(serde_json::from_value::<Vec<KeywordHighlightRule>>)
            .transpose()?
            .unwrap_or_default()
            .into_iter()
            .filter_map(normalize_keyword_highlight_rule)
            .collect();
        let builtin_rules = json_path(&value, &["terminal", "keyword_highlight_builtin_rules"])
            .cloned()
            .map(serde_json::from_value::<std::collections::HashMap<String, bool>>)
            .transpose()?
            .unwrap_or_default();
        Ok(KeywordHighlightConfig {
            enabled: json_bool(&value, &["terminal", "keyword_highlights_enabled"], false),
            across_wrapped_lines: json_bool(
                &value,
                &["terminal", "keyword_highlights_across_wrapped_lines"],
                false,
            ),
            builtin_rules,
            rules,
        })
    }

    pub fn save_keyword_highlights(
        &self,
        config: &KeywordHighlightConfig,
    ) -> Result<KeywordHighlightConfig, StorageError> {
        let mut value = self.load_settings_value()?;
        set_nested_json_value(
            &mut value,
            &["terminal", "keyword_highlights_enabled"],
            serde_json::Value::Bool(config.enabled),
        );
        set_nested_json_value(
            &mut value,
            &["terminal", "keyword_highlights_across_wrapped_lines"],
            serde_json::Value::Bool(config.across_wrapped_lines),
        );
        set_nested_json_value(
            &mut value,
            &["terminal", "keyword_highlight_builtin_rules"],
            serde_json::to_value(&config.builtin_rules)?,
        );
        let rules = config
            .rules
            .iter()
            .cloned()
            .filter_map(normalize_keyword_highlight_rule)
            .collect::<Vec<_>>();
        set_nested_json_value(
            &mut value,
            &["terminal", "keyword_highlights"],
            serde_json::to_value(rules)?,
        );
        self.save_settings_value(&value)?;
        self.load_keyword_highlights()
    }

    pub fn import_keyword_highlights_json(
        &self,
        raw: &str,
    ) -> Result<(KeywordHighlightConfig, KeywordHighlightImportResult), StorageError> {
        let imported = parse_keyword_highlight_import(raw)?;
        let mut config = self.load_keyword_highlights()?;
        let result = merge_keyword_highlight_rules(&mut config.rules, imported);
        if result.imported_rules == 0 && result.updated_rules == 0 {
            return Err(StorageError::InvalidData(
                "No valid highlight rules found in import file".to_string(),
            ));
        }
        let saved = self.save_keyword_highlights(&config)?;
        Ok((saved, result))
    }

    pub fn load_quick_commands(&self) -> Result<QuickCommandsConfig, StorageError> {
        self.read_json_table::<QuickCommandsConfig>(SETTINGS_TABLE, SETTINGS_QUICK_COMMANDS)
            .map(|config| config.unwrap_or_default())
    }

    pub fn save_quick_commands(&self, config: QuickCommandsConfig) -> Result<(), StorageError> {
        self.save_settings_doc_value(SETTINGS_QUICK_COMMANDS, &serde_json::to_value(config)?)?;
        Ok(())
    }

    pub fn upsert_quick_command(
        &self,
        mut command: QuickCommand,
        new_category: Option<QuickCommandCategory>,
    ) -> Result<QuickCommandsConfig, StorageError> {
        let mut config = self.load_quick_commands()?;
        let now = current_time_ms();

        if let Some(category) = new_category
            && !config.categories.iter().any(|item| item.id == category.id)
        {
            config.categories.push(category);
        }

        command.updated_at = Some(now);
        if let Some(existing) = config
            .commands
            .iter_mut()
            .find(|item| item.id == command.id)
        {
            let original_created_at = existing.created_at;
            let original_use_count = existing.use_count;
            *existing = command;
            if original_created_at.is_some() {
                existing.created_at = original_created_at;
            }
            if original_use_count.is_some() {
                existing.use_count = original_use_count;
            }
        } else {
            command.created_at = command.created_at.or(Some(now));
            config.commands.push(command);
        }

        self.save_quick_commands(config.clone())?;
        Ok(config)
    }

    pub fn increment_quick_command_use_count(&self, id: &str) -> Result<(), StorageError> {
        let mut config = self.load_quick_commands()?;
        if let Some(command) = config.commands.iter_mut().find(|command| command.id == id) {
            command.use_count = Some(command.use_count.unwrap_or_default().saturating_add(1));
            command.updated_at = Some(current_time_ms());
            self.save_quick_commands(config)?;
        }
        Ok(())
    }

    pub fn load_ai_settings(&self) -> Result<AiSettings, StorageError> {
        let value = self.load_settings_value()?;
        let mut settings = value
            .get(SETTINGS_AI_FIELD)
            .cloned()
            .map(serde_json::from_value::<AiSettings>)
            .transpose()?
            .unwrap_or_default();
        self.decrypt_ai_settings(&mut settings)?;
        normalize_ai_settings(&mut settings);
        Ok(settings)
    }

    pub fn save_ai_settings(&self, next: AiSettings) -> Result<AiSettings, StorageError> {
        let current = self.load_ai_settings()?;
        let merged = merge_masked_ai_settings(&current, next);
        let encrypted = self.encrypt_ai_settings(merged.clone())?;
        let mut value = self.load_settings_value()?;
        let mut encrypted_value = serde_json::to_value(encrypted)?;
        if let Some(current_value) = value.get(SETTINGS_AI_FIELD) {
            merge_unknown_json(current_value, &mut encrypted_value);
        }
        drop_deprecated_ai_fields(&mut encrypted_value);
        set_nested_json_value(&mut value, &[SETTINGS_AI_FIELD], encrypted_value);
        self.save_settings_value(&value)?;
        Ok(merged)
    }

    fn ensure_tables(&self) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        txn.open_table(META_TABLE)?;
        txn.open_table(TEXT_DOCS_TABLE)?;
        txn.open_table(GROUPS_TABLE)?;
        txn.open_table(CONNECTIONS_TABLE)?;
        txn.open_table(TUNNELS_TABLE)?;
        txn.open_table(PROXIES_TABLE)?;
        txn.open_table(CREDENTIALS_TABLE)?;
        txn.open_table(OTP_ACCOUNTS_TABLE)?;
        txn.open_table(KNOWN_HOSTS_TABLE)?;
        txn.open_table(RDP_KNOWN_HOSTS_TABLE)?;
        txn.open_table(PORTABLE_OPAQUE_ENTITIES_TABLE)?;
        txn.open_table(COMMAND_HISTORY_TABLE)?;
        txn.open_table(NOTE_FOLDERS_TABLE)?;
        txn.open_table(NOTES_TABLE)?;
        txn.open_table(NOTE_SUMMARIES_TABLE)?;
        txn.open_table(SETTINGS_TABLE)?;
        txn.open_table(IDX_CONNECTIONS_BY_GROUP_TABLE)?;
        txn.open_table(IDX_CONNECTIONS_BY_LAST_USED_TABLE)?;
        txn.open_table(IDX_CONNECTIONS_BY_PROTOCOL_TABLE)?;
        txn.commit()?;
        Ok(())
    }

    fn list_json_by_prefix<T>(
        &self,
        definition: TableDefinition<&str, &[u8]>,
        prefix: &str,
    ) -> Result<Vec<T>, StorageError>
    where
        T: DeserializeOwned,
    {
        let txn = self.db.begin_read()?;
        let table = match txn.open_table(definition) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut values = Vec::new();
        for entry in table.iter()? {
            let (key, value) = entry?;
            if key.value().starts_with(prefix) {
                values.push(deserialize_json(value.value())?);
            }
        }
        Ok(values)
    }

    fn read_json_table<T>(
        &self,
        definition: TableDefinition<&str, &[u8]>,
        key: &str,
    ) -> Result<Option<T>, StorageError>
    where
        T: DeserializeOwned,
    {
        let txn = self.db.begin_read()?;
        let table = match txn.open_table(definition) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let Some(raw) = table.get(key)? else {
            return Ok(None);
        };
        Ok(Some(deserialize_json(raw.value())?))
    }

    fn hydrate_connection_passwords(
        &self,
        connections: &mut [SavedConnection],
    ) -> Result<(), StorageError> {
        let master_key_token = self.load_master_key_token()?;
        let crypto = self.credential_crypto()?;
        let txn = self.db.begin_read()?;
        let table = match txn.open_table(CREDENTIALS_TABLE) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        for connection in connections {
            let Some(auth) = connection.auth.as_mut() else {
                continue;
            };
            if let (Some(master_key_token), Some(password)) = (
                master_key_token.as_deref(),
                auth.password
                    .as_ref()
                    .map(nyaterm_core::SecretString::expose_secret),
            ) && let Ok(plaintext) = crypto.decrypt_secret(master_key_token, password)
            {
                auth.password = Some(plaintext.into());
                auth.has_password = false;
                continue;
            }
            let key = entity_key(CONNECTION_PASSWORD_PREFIX, &connection.id);
            if let Some(raw) = table.get(key.as_str())? {
                let record: ConnectionPasswordRecord = deserialize_json(raw.value())?;
                if let Some(master_key_token) = master_key_token.as_deref() {
                    match crypto.decrypt_secret(master_key_token, record.password.expose_secret()) {
                        Ok(plaintext) => {
                            auth.password = Some(plaintext.into());
                            auth.has_password = false;
                        }
                        Err(_) => {
                            auth.password = Some(record.password);
                            auth.has_password = true;
                        }
                    }
                } else {
                    auth.password = Some(record.password);
                    auth.has_password = true;
                }
            }
        }
        Ok(())
    }

    fn credential_crypto(&self) -> Result<CredentialCrypto, StorageError> {
        let bootstrap = CredentialCrypto::new(self.portable_key_path.clone(), None);
        let master_password = self
            .load_encrypted_master_password()?
            .and_then(|token| bootstrap.decrypt_settings_secret(&token).ok())
            .map(Into::into);
        Ok(CredentialCrypto::new(
            self.portable_key_path.clone(),
            master_password,
        ))
    }

    fn load_encrypted_master_password(&self) -> Result<Option<String>, StorageError> {
        let value = self.load_settings_value()?;
        Ok(value
            .get("security")
            .and_then(|security| security.get("master_password"))
            .and_then(|master_password| master_password.as_str())
            .filter(|master_password| !master_password.is_empty())
            .map(ToOwned::to_owned))
    }

    fn load_settings_value(&self) -> Result<serde_json::Value, StorageError> {
        let txn = self.db.begin_read()?;
        let table = match txn.open_table(SETTINGS_TABLE) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(default_settings_value()),
            Err(error) => return Err(error.into()),
        };
        let Some(raw) = table.get(SETTINGS_DEFAULT)? else {
            return Ok(default_settings_value());
        };
        deserialize_json(raw.value())
    }

    fn load_settings_doc_value(
        &self,
        key: &str,
        fallback: serde_json::Value,
    ) -> Result<serde_json::Value, StorageError> {
        let txn = self.db.begin_read()?;
        let table = match txn.open_table(SETTINGS_TABLE) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(fallback),
            Err(error) => return Err(error.into()),
        };
        let Some(raw) = table.get(key)? else {
            return Ok(fallback);
        };
        deserialize_json(raw.value())
    }

    fn save_settings_value(&self, value: &serde_json::Value) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        write_json_in_txn(&txn, SETTINGS_TABLE, SETTINGS_DEFAULT, value)?;
        txn.commit()?;
        Ok(())
    }

    fn save_settings_doc_value(
        &self,
        key: &str,
        value: &serde_json::Value,
    ) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        write_json_in_txn(&txn, SETTINGS_TABLE, key, value)?;
        txn.commit()?;
        Ok(())
    }

    fn decrypt_ai_settings(&self, settings: &mut AiSettings) -> Result<(), StorageError> {
        let crypto = self.credential_crypto()?;
        let master_key_token = self.load_master_key_token()?;
        let master_key_token = master_key_token.as_deref();
        for profile in &mut settings.provider_profiles {
            profile.api_key = decrypt_optional_secret(&crypto, master_key_token, &profile.api_key)?;
        }
        for credential in &mut settings.provider_credentials {
            credential.api_key =
                decrypt_optional_secret(&crypto, master_key_token, &credential.api_key)?;
        }
        Ok(())
    }

    fn encrypt_ai_settings(&self, mut settings: AiSettings) -> Result<AiSettings, StorageError> {
        let crypto = self.credential_crypto()?;
        let master_key_token = if ai_settings_has_secret(&settings) {
            Some(self.get_or_create_master_key_token(&crypto)?)
        } else {
            None
        };
        let master_key_token = master_key_token.as_deref();
        for profile in &mut settings.provider_profiles {
            profile.api_key = encrypt_optional_secret(&crypto, master_key_token, &profile.api_key)?;
        }
        for credential in &mut settings.provider_credentials {
            credential.api_key =
                encrypt_optional_secret(&crypto, master_key_token, &credential.api_key)?;
        }
        Ok(settings)
    }

    fn decrypt_translation_settings(
        &self,
        settings: &mut TranslationSettings,
    ) -> Result<(), StorageError> {
        let crypto = self.credential_crypto()?;
        let master_key_token = self.load_master_key_token()?;
        let master_key_token = master_key_token.as_deref();
        settings.deepl_api_key =
            decrypt_legacy_plaintext_secret(&crypto, master_key_token, &settings.deepl_api_key)?;
        settings.baidu_app_key =
            decrypt_legacy_plaintext_secret(&crypto, master_key_token, &settings.baidu_app_key)?;
        settings.ali_app_key =
            decrypt_legacy_plaintext_secret(&crypto, master_key_token, &settings.ali_app_key)?;
        settings.youdao_app_key =
            decrypt_legacy_plaintext_secret(&crypto, master_key_token, &settings.youdao_app_key)?;
        Ok(())
    }

    fn encrypt_translation_settings(
        &self,
        mut settings: TranslationSettings,
    ) -> Result<TranslationSettings, StorageError> {
        let crypto = self.credential_crypto()?;
        let master_key_token = if translation_settings_has_secret(&settings) {
            Some(self.get_or_create_master_key_token(&crypto)?)
        } else {
            None
        };
        let master_key_token = master_key_token.as_deref();
        settings.deepl_api_key =
            encrypt_string_secret(&crypto, master_key_token, &settings.deepl_api_key)?;
        settings.baidu_app_key =
            encrypt_string_secret(&crypto, master_key_token, &settings.baidu_app_key)?;
        settings.ali_app_key =
            encrypt_string_secret(&crypto, master_key_token, &settings.ali_app_key)?;
        settings.youdao_app_key =
            encrypt_string_secret(&crypto, master_key_token, &settings.youdao_app_key)?;
        Ok(settings)
    }

    fn get_or_create_master_key_token(
        &self,
        crypto: &CredentialCrypto,
    ) -> Result<String, StorageError> {
        if let Some(token) = self.load_master_key_token()? {
            return Ok(token);
        }
        let token = crypto.generate_master_key_token()?;
        self.save_master_key_token(&token)?;
        Ok(token)
    }

    fn save_master_key_token(&self, token: &str) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        txn.open_table(META_TABLE)?.insert(META_MASTER_KEY, token)?;
        txn.open_table(TEXT_DOCS_TABLE)?
            .insert(LEGACY_TEXT_MASTER_KEY, token)?;
        txn.commit()?;
        Ok(())
    }

    fn load_master_key_token(&self) -> Result<Option<String>, StorageError> {
        if let Some(token) = self.read_string_table(META_TABLE, META_MASTER_KEY)? {
            return Ok(Some(token));
        }
        self.read_string_table(TEXT_DOCS_TABLE, LEGACY_TEXT_MASTER_KEY)
    }

    fn read_string_table(
        &self,
        definition: TableDefinition<&str, &str>,
        key: &str,
    ) -> Result<Option<String>, StorageError> {
        let txn = self.db.begin_read()?;
        let table = match txn.open_table(definition) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Ok(table.get(key)?.map(|raw| raw.value().to_string()))
    }

    fn list_raw_by_prefix(
        &self,
        definition: TableDefinition<&str, &[u8]>,
        prefix: &str,
    ) -> Result<Vec<(String, Vec<u8>)>, StorageError> {
        let txn = self.db.begin_read()?;
        let table = match txn.open_table(definition) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut values = Vec::new();
        for entry in table.iter()? {
            let (key, value) = entry?;
            if key.value().starts_with(prefix) {
                values.push((key.value().to_string(), value.value().to_vec()));
            }
        }
        Ok(values)
    }

    fn list_raw_json_values_by_prefix(
        &self,
        definition: TableDefinition<&str, &[u8]>,
        prefix: &str,
    ) -> Result<Vec<serde_json::Value>, StorageError> {
        self.list_raw_by_prefix(definition, prefix)?
            .into_iter()
            .map(|(_, value)| serde_json::from_slice(&value).map_err(StorageError::from))
            .collect()
    }

    fn list_keyed_json_by_prefix<T>(
        &self,
        definition: TableDefinition<&str, &[u8]>,
        prefix: &str,
    ) -> Result<Vec<(String, T)>, StorageError>
    where
        T: DeserializeOwned,
    {
        self.list_raw_by_prefix(definition, prefix)?
            .into_iter()
            .map(|(key, value)| {
                serde_json::from_slice(&value)
                    .map(|value| (key, value))
                    .map_err(StorageError::from)
            })
            .collect()
    }
}

fn merge_unknown_json(current: &serde_json::Value, next: &mut serde_json::Value) {
    match (current, next) {
        (serde_json::Value::Object(current), serde_json::Value::Object(next)) => {
            for (key, current_value) in current {
                if let Some(next_value) = next.get_mut(key) {
                    merge_unknown_json(current_value, next_value);
                } else {
                    next.insert(key.clone(), current_value.clone());
                }
            }
        }
        (serde_json::Value::Array(current), serde_json::Value::Array(next)) => {
            for next_item in next {
                let Some(id) = next_item.get("id").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                if let Some(current_item) = current
                    .iter()
                    .find(|item| item.get("id").and_then(serde_json::Value::as_str) == Some(id))
                {
                    merge_unknown_json(current_item, next_item);
                }
            }
        }
        _ => {}
    }
}

fn drop_deprecated_ai_fields(value: &mut serde_json::Value) {
    if let Some(external_mcp) = value
        .get_mut("external_mcp")
        .and_then(serde_json::Value::as_object_mut)
    {
        external_mcp.remove("server_mode");
        external_mcp.remove("idle_timeout_minutes");
    }
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn replace_sessions_in_txn(
    txn: &redb::WriteTransaction,
    config: &SessionsConfig,
) -> Result<(), StorageError> {
    clear_prefix_in_txn(txn, GROUPS_TABLE, GROUP_PREFIX)?;
    clear_prefix_in_txn(txn, CONNECTIONS_TABLE, CONNECTION_PREFIX)?;
    clear_prefix_in_txn(txn, CREDENTIALS_TABLE, CONNECTION_PASSWORD_PREFIX)?;
    clear_string_table(txn, IDX_CONNECTIONS_BY_GROUP_TABLE)?;
    clear_string_table(txn, IDX_CONNECTIONS_BY_LAST_USED_TABLE)?;
    clear_string_table(txn, IDX_CONNECTIONS_BY_PROTOCOL_TABLE)?;
    clear_prefix_in_txn(txn, CONNECTIONS_TABLE, "connection_custom_icons/")?;
    for icon in &config.custom_icons {
        write_json_in_txn(
            txn,
            CONNECTIONS_TABLE,
            &entity_key("connection_custom_icons/", &icon.id),
            icon,
        )?;
    }
    for group in &config.groups {
        save_group_in_txn(txn, group)?;
    }
    for connection in &config.connections {
        save_connection_in_txn(txn, connection)?;
    }
    Ok(())
}

fn save_group_in_txn(txn: &redb::WriteTransaction, group: &Group) -> Result<(), StorageError> {
    let mut group = group.clone();
    let now = current_time_ms();
    let key = entity_key(GROUP_PREFIX, &group.id);
    if group.created_at_ms.is_none() {
        group.created_at_ms = existing_group_created_at(txn, &key)?.or(Some(now));
    }
    group.updated_at_ms = Some(now);
    write_json_in_txn(txn, GROUPS_TABLE, &key, &group)
}

fn existing_group_created_at(
    txn: &redb::WriteTransaction,
    key: &str,
) -> Result<Option<u64>, StorageError> {
    let table = txn.open_table(GROUPS_TABLE)?;
    let Some(raw) = table.get(key)? else {
        return Ok(None);
    };
    let group: Group = deserialize_json(raw.value())?;
    Ok(group.created_at_ms)
}

fn save_connection_in_txn(
    txn: &redb::WriteTransaction,
    connection: &SavedConnection,
) -> Result<(), StorageError> {
    let mut connection = connection.clone();
    // Canonicalize legacy fields before every write so input-only aliases are
    // never silently discarded by unrelated updates.
    migrate_legacy_ssh_agent_settings(&mut connection);
    if let ConnectionType::Ssh {
        agent_forwarding_config: Some(config),
        ..
    } = &connection.config
    {
        validate_ssh_agent_forwarding_config(config).map_err(|error| {
            StorageError::InvalidData(format!(
                "invalid SSH Agent forwarding configuration: {error:?}"
            ))
        })?;
    }
    let now = current_time_ms();
    let connection_key = entity_key(CONNECTION_PREFIX, &connection.id);
    if connection.created_at_ms.is_none() {
        connection.created_at_ms =
            existing_connection_created_at(txn, &connection_key)?.or(Some(now));
    }
    connection.updated_at_ms = Some(now);

    remove_connection_index_entries(txn, &connection.id)?;
    delete_connection_password_in_txn(txn, &connection.id)?;
    if let Some(auth) = connection.auth.as_mut() {
        if let Some(password) = auth.password.take().filter(|value| !value.is_empty()) {
            let record = ConnectionPasswordRecord {
                id: connection.id.clone(),
                connection_id: connection.id.clone(),
                password,
                created_at_ms: now,
                updated_at_ms: now,
            };
            write_json_in_txn(
                txn,
                CREDENTIALS_TABLE,
                &entity_key(CONNECTION_PASSWORD_PREFIX, &connection.id),
                &record,
            )?;
        }
        auth.has_password = false;
    }

    write_json_in_txn(txn, CONNECTIONS_TABLE, &connection_key, &connection)?;
    insert_connection_indexes(txn, &connection)?;
    Ok(())
}

fn existing_connection_created_at(
    txn: &redb::WriteTransaction,
    key: &str,
) -> Result<Option<u64>, StorageError> {
    let table = txn.open_table(CONNECTIONS_TABLE)?;
    let Some(raw) = table.get(key)? else {
        return Ok(None);
    };
    let mut connection: SavedConnection = deserialize_json(raw.value())?;
    migrate_legacy_ssh_agent_settings(&mut connection);
    Ok(connection.created_at_ms)
}

fn delete_connection_in_txn(
    txn: &redb::WriteTransaction,
    connection_id: &str,
) -> Result<(), StorageError> {
    txn.open_table(CONNECTIONS_TABLE)?
        .remove(entity_key(CONNECTION_PREFIX, connection_id).as_str())?;
    delete_connection_password_in_txn(txn, connection_id)?;
    remove_connection_index_entries(txn, connection_id)
}

fn delete_connection_password_in_txn(
    txn: &redb::WriteTransaction,
    connection_id: &str,
) -> Result<(), StorageError> {
    txn.open_table(CREDENTIALS_TABLE)?
        .remove(entity_key(CONNECTION_PASSWORD_PREFIX, connection_id).as_str())?;
    Ok(())
}

fn insert_connection_indexes(
    txn: &redb::WriteTransaction,
    connection: &SavedConnection,
) -> Result<(), StorageError> {
    let group_id = connection.group_id.as_deref().unwrap_or_default();
    let group_key = format!(
        "{}|{}|{}",
        group_id,
        padded_i64(i64::from(connection.sort_order)),
        connection.id
    );
    txn.open_table(IDX_CONNECTIONS_BY_GROUP_TABLE)?
        .insert(group_key.as_str(), connection.id.as_str())?;

    let last_used = connection.last_used_at_ms.unwrap_or_default();
    let reverse = u64::MAX.saturating_sub(last_used);
    let last_used_key = format!("{reverse:020}|{}", connection.id);
    txn.open_table(IDX_CONNECTIONS_BY_LAST_USED_TABLE)?
        .insert(last_used_key.as_str(), connection.id.as_str())?;

    let protocol_key = format!(
        "{}|{}",
        connection.kind_label().to_lowercase(),
        connection.id
    );
    txn.open_table(IDX_CONNECTIONS_BY_PROTOCOL_TABLE)?
        .insert(protocol_key.as_str(), connection.id.as_str())?;

    Ok(())
}

fn remove_connection_index_entries(
    txn: &redb::WriteTransaction,
    connection_id: &str,
) -> Result<(), StorageError> {
    remove_connection_index_entries_from_table(txn, IDX_CONNECTIONS_BY_GROUP_TABLE, connection_id)?;
    remove_connection_index_entries_from_table(
        txn,
        IDX_CONNECTIONS_BY_LAST_USED_TABLE,
        connection_id,
    )?;
    remove_connection_index_entries_from_table(
        txn,
        IDX_CONNECTIONS_BY_PROTOCOL_TABLE,
        connection_id,
    )
}

fn remove_connection_index_entries_from_table(
    txn: &redb::WriteTransaction,
    definition: TableDefinition<&str, &str>,
    connection_id: &str,
) -> Result<(), StorageError> {
    let table = txn.open_table(definition)?;
    let mut keys = Vec::new();
    for entry in table.iter()? {
        let (key, value) = entry?;
        if value.value() == connection_id || key.value().ends_with(&format!("|{connection_id}")) {
            keys.push(key.value().to_string());
        }
    }
    drop(table);

    let mut table = txn.open_table(definition)?;
    for key in keys {
        table.remove(key.as_str())?;
    }
    Ok(())
}

fn clear_prefix_in_txn(
    txn: &redb::WriteTransaction,
    definition: TableDefinition<&str, &[u8]>,
    prefix: &str,
) -> Result<(), StorageError> {
    let table = txn.open_table(definition)?;
    let mut keys = Vec::new();
    for entry in table.iter()? {
        let (key, _) = entry?;
        if key.value().starts_with(prefix) {
            keys.push(key.value().to_string());
        }
    }
    drop(table);

    let mut table = txn.open_table(definition)?;
    for key in keys {
        table.remove(key.as_str())?;
    }
    Ok(())
}

fn clear_string_table(
    txn: &redb::WriteTransaction,
    definition: TableDefinition<&str, &str>,
) -> Result<(), StorageError> {
    let table = txn.open_table(definition)?;
    let keys = table
        .iter()?
        .map(|entry| entry.map(|(key, _)| key.value().to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    drop(table);

    let mut table = txn.open_table(definition)?;
    for key in keys {
        table.remove(key.as_str())?;
    }
    Ok(())
}

fn decrypt_optional_secret(
    crypto: &CredentialCrypto,
    master_key_token: Option<&str>,
    value: &Option<nyaterm_core::SecretString>,
) -> Result<Option<nyaterm_core::SecretString>, StorageError> {
    let Some(value) = value
        .as_ref()
        .map(nyaterm_core::SecretString::expose_secret)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let Some(master_key_token) = master_key_token else {
        return Err(StorageError::MissingMasterKey);
    };
    crypto
        .decrypt_secret(master_key_token, value)
        .map(Into::into)
        .map(Some)
        .map_err(StorageError::from)
}

fn encrypt_optional_secret(
    crypto: &CredentialCrypto,
    master_key_token: Option<&str>,
    value: &Option<nyaterm_core::SecretString>,
) -> Result<Option<nyaterm_core::SecretString>, StorageError> {
    let Some(value) = value
        .as_ref()
        .map(nyaterm_core::SecretString::expose_secret)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let Some(master_key_token) = master_key_token else {
        return Err(StorageError::MissingMasterKey);
    };
    crypto
        .encrypt_secret(master_key_token, value)
        .map(Into::into)
        .map(Some)
        .map_err(StorageError::from)
}

fn decrypt_legacy_plaintext_secret(
    crypto: &CredentialCrypto,
    master_key_token: Option<&str>,
    value: &nyaterm_core::SecretString,
) -> Result<nyaterm_core::SecretString, StorageError> {
    let value = value.expose_secret().trim();
    if value.is_empty() {
        return Ok(nyaterm_core::SecretString::default());
    }
    let Some(master_key_token) = master_key_token else {
        return Ok(value.into());
    };
    Ok(crypto
        .decrypt_secret(master_key_token, value)
        .unwrap_or_else(|_| value.to_string())
        .into())
}

fn encrypt_string_secret(
    crypto: &CredentialCrypto,
    master_key_token: Option<&str>,
    value: &nyaterm_core::SecretString,
) -> Result<nyaterm_core::SecretString, StorageError> {
    let value = value.expose_secret().trim();
    if value.is_empty() {
        return Ok(nyaterm_core::SecretString::default());
    }
    let Some(master_key_token) = master_key_token else {
        return Err(StorageError::MissingMasterKey);
    };
    crypto
        .encrypt_secret(master_key_token, value)
        .map(Into::into)
        .map_err(StorageError::from)
}

fn optional_secret_present(value: &Option<nyaterm_core::SecretString>) -> bool {
    value.as_ref().is_some_and(|value| !value.is_empty())
}

fn write_json_in_txn<T>(
    txn: &redb::WriteTransaction,
    definition: TableDefinition<&str, &[u8]>,
    key: &str,
    value: &T,
) -> Result<(), StorageError>
where
    T: Serialize,
{
    let bytes = serde_json::to_vec(value)?;
    txn.open_table(definition)?.insert(key, bytes.as_slice())?;
    Ok(())
}

fn deserialize_json<T>(value: &[u8]) -> Result<T, StorageError>
where
    T: DeserializeOwned,
{
    Ok(serde_json::from_slice(value)?)
}

fn entity_key(prefix: &str, id: &str) -> String {
    format!("{prefix}{id}")
}

fn sort_connections(connections: &mut [SavedConnection]) {
    connections.sort_by(|left, right| {
        left.group_id
            .cmp(&right.group_id)
            .then(left.sort_order.cmp(&right.sort_order))
            .then(left.name.cmp(&right.name))
            .then(left.id.cmp(&right.id))
    });
}

fn default_settings_value() -> serde_json::Value {
    serde_json::json!({
        "general": {
            "startup_restore": false,
            "startup_restore_window_layout": true,
            "minimize_to_tray": false,
            "confirm_on_close": true
        },
        "appearance": {
            "theme": "github-dark",
            "font_family": "JetBrains Mono",
            "font_size": 16.0
        },
        "translation": {
            "target_language": "zh-CN",
            "deepl_api_key": "",
            "baidu_app_id": "",
            "baidu_app_key": "",
            "ali_app_id": "",
            "ali_app_key": "",
            "youdao_app_id": "",
            "youdao_app_key": ""
        },
        "security": {
            "use_os_keyring": true,
            "enable_screen_lock": false,
            "idle_lock_minutes": 0,
            "master_password": null,
            "host_key_policy": "prompt"
        },
        "transfer": {
            "duplicate_strategy": "ask"
        }
    })
}

fn json_bool(value: &serde_json::Value, path: &[&str], fallback: bool) -> bool {
    json_path(value, path)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(fallback)
}

fn json_string_vec(value: &serde_json::Value, path: &[&str], limit: usize) -> Vec<String> {
    json_path(value, path)
        .and_then(serde_json::Value::as_array)
        .map(|array| {
            array
                .iter()
                .filter_map(|entry| {
                    entry
                        .as_str()
                        .map(str::trim)
                        .filter(|entry| !entry.is_empty())
                        .map(ToOwned::to_owned)
                })
                .fold(Vec::<String>::new(), |mut values, entry| {
                    if !values.iter().any(|existing| existing == &entry) {
                        values.push(entry);
                    }
                    values
                })
                .into_iter()
                .take(limit)
                .collect()
        })
        .unwrap_or_default()
}

fn json_path<'a>(value: &'a serde_json::Value, path: &[&str]) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    Some(current)
}

fn set_nested_json_value(
    value: &mut serde_json::Value,
    path: &[&str],
    new_value: serde_json::Value,
) {
    if !value.is_object() {
        *value = serde_json::Value::Object(Default::default());
    }
    let mut current = value;
    for key in &path[..path.len().saturating_sub(1)] {
        if !current.get(*key).is_some_and(serde_json::Value::is_object) {
            current[*key] = serde_json::Value::Object(Default::default());
        }
        current = current.get_mut(*key).expect("object child exists");
    }
    if let Some(key) = path.last() {
        current[*key] = new_value;
    }
}

fn padded_i64(value: i64) -> String {
    let shifted = i128::from(value) - i128::from(i64::MIN);
    format!("{shifted:020}")
}

fn stable_id(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut output = String::with_capacity(32);
    for byte in &digest[..16] {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn validate_config_backup_file(
    source: &Path,
    portable_key_path: Option<PathBuf>,
) -> Result<(), StorageError> {
    let validation_dir = std::env::temp_dir().join(format!(
        "nyaterm-config-backup-validate-{}-{}-{}",
        std::process::id(),
        current_time_ms(),
        uuid()
    ));
    std::fs::create_dir_all(&validation_dir).map_err(|source| StorageError::CreateDir {
        path: validation_dir.clone(),
        source,
    })?;
    let validation_db = validation_dir.join(DATABASE_FILE);
    copy_config_database(source, &validation_db)?;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
        || -> Result<(), StorageError> {
            let store =
                ConnectionStore::open_with_portable_key_path(&validation_dir, portable_key_path)?;
            store.load_sessions()?;
            store.load_app_settings_summary()?;
            store.list_tunnels()?;
            drop(store);
            Ok(())
        },
    ))
    .map_err(|_| {
        StorageError::InvalidData(format!(
            "configuration backup is not a valid redb database: {}",
            source.display()
        ))
    })?;
    std::fs::remove_dir_all(validation_dir).ok();
    result
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
