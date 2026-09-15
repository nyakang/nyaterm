//! redb-backed repository for `NyaTerm`'s user data.

#![allow(dead_code)]

mod credentials;
mod history;
mod known_hosts;
mod master_key;
mod migration;
mod notes;
mod rdp_known_hosts;
mod sessions;
mod settings_impl;
mod tables;
mod util;

#[cfg(test)]
mod tests;

pub(crate) use known_hosts::KnownHostEntry;
pub(crate) use rdp_known_hosts::RdpCertificateMetadata;

#[allow(unused_imports)]
pub use tables::{
    COMMAND_HISTORY_TABLE, CONNECTIONS_TABLE, CREDENTIALS_TABLE, GROUPS_TABLE,
    IDX_CONNECTIONS_BY_GROUP_TABLE, IDX_CONNECTIONS_BY_LAST_USED_TABLE,
    IDX_CONNECTIONS_BY_PROTOCOL_TABLE, KNOWN_HOSTS_TABLE, META_TABLE, NOTE_FOLDERS_TABLE,
    NOTES_TABLE, OTP_ACCOUNTS_TABLE, PROXIES_TABLE, RDP_KNOWN_HOSTS_TABLE, SETTINGS_TABLE,
    TUNNELS_TABLE,
};

use crate::error::{AppError, AppResult};
use redb::Database;
use serde::{Serialize, de::DeserializeOwned};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use migration::backup_database_before_migration_if_needed;
use tables::DATABASE_FILE;
use util::storage_error;

static STORAGE: OnceLock<Arc<Storage>> = OnceLock::new();

#[derive(Debug)]
pub struct Storage {
    pub(super) db: Database,
    pub(super) db_path: PathBuf,
    pub(super) pending_migration_backup: Option<BackupInfo>,
}

#[derive(Debug, Clone)]
pub(super) struct BackupInfo {
    pub(super) path: PathBuf,
    pub(super) created_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsDocKey {
    AppSettings,
    QuickCommands,
    CloudSyncSettings,
    CloudSyncState,
    AiHistory,
    AiAudit,
    SftpFileBackendCache,
    WindowState,
    ProxyGroups,
    TunnelGroups,
}
impl SettingsDocKey {
    fn storage_key(self) -> &'static str {
        match self {
            Self::AppSettings => tables::SETTINGS_DEFAULT,
            Self::QuickCommands => "settings/doc/quick-command",
            Self::CloudSyncSettings => "settings/doc/cloud-sync",
            Self::CloudSyncState => "settings/doc/cloud-sync-state",
            Self::AiHistory => "settings/doc/ai-history",
            Self::AiAudit => "settings/doc/ai-audit",
            Self::SftpFileBackendCache => "settings/doc/file-backend-cache",
            Self::WindowState => "settings/doc/window-state",
            Self::ProxyGroups => "settings/doc/proxy-groups",
            Self::TunnelGroups => "settings/doc/tunnel-groups",
        }
    }

    fn legacy_key(self) -> Option<&'static str> {
        match self {
            Self::AppSettings => Some(tables::LEGACY_JSON_SETTINGS),
            Self::QuickCommands => Some(tables::LEGACY_JSON_QUICK_COMMAND),
            Self::CloudSyncSettings => Some(tables::LEGACY_JSON_CLOUD_SYNC),
            Self::CloudSyncState => Some(tables::LEGACY_JSON_CLOUD_SYNC_STATE),
            Self::AiHistory => Some(tables::LEGACY_JSON_AI_HISTORY),
            Self::AiAudit => Some(tables::LEGACY_JSON_AI_AUDIT),
            Self::SftpFileBackendCache => Some("file-backend-cache"),
            Self::WindowState | Self::ProxyGroups | Self::TunnelGroups => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownHostCheck {
    Match,
    HostSeen,
    UnknownHost,
}

pub fn init(config_dir: &Path) -> AppResult<()> {
    fs::create_dir_all(config_dir)?;
    let storage = Arc::new(Storage::open(config_dir)?);
    if STORAGE.set(storage).is_err() {
        tracing::debug!("redb storage was already initialized");
    }
    Ok(())
}

#[cfg(test)]
fn database_path(config_dir: &Path) -> PathBuf {
    config_dir.join(DATABASE_FILE)
}

pub(crate) fn load_settings_doc<T: DeserializeOwned + Default>(
    key: SettingsDocKey,
) -> AppResult<T> {
    Ok(storage()?.get_settings_doc(key)?.unwrap_or_default())
}

pub(crate) fn save_settings_doc<T: Serialize>(key: SettingsDocKey, value: &T) -> AppResult<()> {
    storage()?.save_settings_doc(key, value)
}

pub(crate) fn update_settings_doc<T, R, F>(key: SettingsDocKey, updater: F) -> AppResult<R>
where
    T: DeserializeOwned + Default + Serialize,
    F: FnOnce(&mut T) -> AppResult<R>,
{
    storage()?.update_settings_doc(key, updater)
}

pub(crate) fn load_sessions() -> AppResult<crate::config::SessionsConfig> {
    storage()?.load_sessions()
}

pub(crate) fn replace_sessions(config: &crate::config::SessionsConfig) -> AppResult<()> {
    storage()?.replace_sessions(config)
}

pub(crate) fn get_connection(
    connection_id: &str,
) -> AppResult<Option<crate::config::SavedConnection>> {
    storage()?.get_connection_with_secret(connection_id)
}

pub(crate) fn mark_connection_used(connection_id: &str) -> AppResult<()> {
    storage()?.mark_connection_used(connection_id)
}

pub(crate) fn list_passwords() -> AppResult<Vec<crate::config::SavedPassword>> {
    storage()?.list_passwords()
}

pub(crate) fn replace_passwords(config: &crate::config::PasswordsConfig) -> AppResult<()> {
    storage()?.replace_passwords(config)
}

pub(crate) fn list_ssh_keys() -> AppResult<Vec<crate::config::SshKey>> {
    storage()?.list_ssh_keys()
}

pub(crate) fn replace_ssh_keys(config: &crate::config::KeysConfig) -> AppResult<()> {
    storage()?.replace_ssh_keys(config)
}

pub(crate) fn list_credentials() -> AppResult<Vec<crate::config::SavedCredential>> {
    storage()?.list_credentials()
}

pub(crate) fn replace_credentials(config: &crate::config::CredentialsConfig) -> AppResult<()> {
    storage()?.replace_credentials(config)
}

pub(crate) fn list_otp_accounts() -> AppResult<Vec<crate::config::OtpEntry>> {
    storage()?.list_otp_accounts()
}

pub(crate) fn replace_otp_accounts(config: &crate::config::OtpConfig) -> AppResult<()> {
    storage()?.replace_otp_accounts(config)
}

pub(crate) fn list_proxies() -> AppResult<Vec<crate::config::ProxyConfig>> {
    storage()?.list_proxies()
}

pub(crate) fn replace_proxies(proxies: &[crate::config::ProxyConfig]) -> AppResult<()> {
    storage()?.replace_proxies(proxies)
}

pub(crate) fn list_tunnels() -> AppResult<Vec<crate::config::TunnelConfig>> {
    storage()?.list_tunnels()
}

pub(crate) fn replace_tunnels(tunnels: &[crate::config::TunnelConfig]) -> AppResult<()> {
    storage()?.replace_tunnels(tunnels)
}

pub(crate) fn list_command_history_entries(
    limit: usize,
) -> AppResult<Vec<crate::core::history::HistoryEntry>> {
    storage()?.list_recent_command_history(limit)
}

pub(crate) fn replace_command_history_entries(
    entries: &[crate::core::history::HistoryEntry],
) -> AppResult<()> {
    storage()?.replace_command_history(entries)
}

pub(crate) fn list_note_folders() -> AppResult<Vec<crate::config::NoteFolder>> {
    storage()?.list_note_folders()
}

pub(crate) fn list_notes() -> AppResult<Vec<crate::config::NoteDocument>> {
    storage()?.list_notes()
}

pub(crate) fn list_note_summaries() -> AppResult<Vec<crate::config::NoteSummary>> {
    storage()?.list_note_summaries()
}

pub(crate) fn get_note(note_id: &str) -> AppResult<Option<crate::config::NoteDocument>> {
    storage()?.get_note(note_id)
}

pub(crate) fn create_note_folder(
    parent_id: Option<String>,
    name: Option<String>,
) -> AppResult<crate::config::NoteFolder> {
    storage()?.create_note_folder(parent_id, name)
}

pub(crate) fn create_note(
    parent_id: Option<String>,
    title: Option<String>,
    markdown: Option<String>,
) -> AppResult<crate::config::NoteDocument> {
    storage()?.create_note(parent_id, title, markdown)
}

pub(crate) fn update_note(
    note_id: &str,
    title: String,
    markdown: String,
    expected_revision: u64,
    force: bool,
) -> AppResult<crate::config::NoteUpdateResult> {
    storage()?.update_note(note_id, title, markdown, expected_revision, force)
}

pub(crate) fn rename_note_node(
    node_kind: &str,
    node_id: &str,
    name: String,
) -> AppResult<crate::config::NoteNodeChange> {
    storage()?.rename_note_node(node_kind, node_id, name)
}

pub(crate) fn move_note_node(
    node_kind: &str,
    node_id: &str,
    parent_id: Option<String>,
    sort_order: i64,
) -> AppResult<crate::config::NoteNodeChange> {
    storage()?.move_note_node(node_kind, node_id, parent_id, sort_order)
}

pub(crate) fn delete_note_node(
    node_kind: &str,
    node_id: &str,
) -> AppResult<crate::config::DeleteNoteNodeResult> {
    storage()?.delete_note_node(node_kind, node_id)
}

pub(crate) fn load_notes_snapshot() -> AppResult<crate::config::NotesSnapshot> {
    storage()?.load_notes_snapshot()
}

pub(crate) fn replace_notes_snapshot(snapshot: &crate::config::NotesSnapshot) -> AppResult<()> {
    storage()?.replace_notes_snapshot(snapshot)
}

pub(crate) fn check_known_host(
    host_identifier: &str,
    key_type: &str,
    key_base64: &str,
) -> AppResult<KnownHostCheck> {
    storage()?.check_known_host(host_identifier, key_type, key_base64)
}

pub(crate) fn upsert_known_host(line: &str) -> AppResult<()> {
    storage()?.upsert_known_host(line)
}

pub(crate) fn replace_known_host_for_host(host_identifier: &str, line: &str) -> AppResult<()> {
    storage()?.replace_known_host_for_host(host_identifier, line)
}

pub(crate) fn render_known_hosts_export() -> AppResult<String> {
    storage()?.render_known_hosts_export()
}

pub(crate) fn replace_known_hosts_export(content: &str) -> AppResult<()> {
    storage()?.replace_known_hosts_export(content)
}

pub(crate) fn list_known_hosts() -> AppResult<Vec<KnownHostEntry>> {
    storage()?.list_known_hosts()
}

pub(crate) fn delete_known_host(id: &str) -> AppResult<()> {
    storage()?.delete_known_host(id)
}

pub(crate) fn clear_known_hosts() -> AppResult<()> {
    storage()?.clear_known_hosts()
}

pub(crate) fn check_rdp_known_host(
    host: &str,
    port: u16,
    sha256_fingerprint: &str,
) -> AppResult<KnownHostCheck> {
    storage()?.check_rdp_known_host(host, port, sha256_fingerprint)
}

pub(crate) fn upsert_rdp_known_host(
    host: &str,
    port: u16,
    sha256_fingerprint: &str,
    certificate: RdpCertificateMetadata,
) -> AppResult<()> {
    storage()?.upsert_rdp_known_host(host, port, sha256_fingerprint, certificate)
}

pub(crate) fn load_master_key_token() -> AppResult<Option<String>> {
    storage()?.load_master_key_token()
}

pub(crate) fn save_master_key_token(token: &str) -> AppResult<()> {
    storage()?.save_master_key_token(token)
}

fn storage() -> AppResult<Arc<Storage>> {
    if let Some(storage) = STORAGE.get() {
        return Ok(storage.clone());
    }
    let config_dir = default_config_dir()?;
    init(&config_dir)?;
    STORAGE
        .get()
        .cloned()
        .ok_or_else(|| AppError::Storage("redb storage did not initialize".to_string()))
}
fn default_config_dir() -> AppResult<PathBuf> {
    #[cfg(test)]
    {
        return Ok(
            std::env::temp_dir().join(format!("nyaterm-test-storage-{}", std::process::id()))
        );
    }

    #[cfg(not(test))]
    {
        let home = dirs::home_dir()
            .ok_or_else(|| AppError::Config("cannot determine home directory".to_string()))?;
        Ok(home.join(".nyaterm"))
    }
}

impl Storage {
    pub fn open(config_dir: &Path) -> AppResult<Self> {
        fs::create_dir_all(config_dir)?;
        Self::open_path(&config_dir.join(DATABASE_FILE))
    }
    fn open_path(db_path: &Path) -> AppResult<Self> {
        let pending_migration_backup = if db_path.exists() {
            backup_database_before_migration_if_needed(db_path)?
        } else {
            None
        };

        let db = if db_path.exists() {
            Database::open(db_path).map_err(storage_error)?
        } else {
            Database::create(db_path).map_err(storage_error)?
        };
        let storage = Self {
            db,
            db_path: db_path.to_path_buf(),
            pending_migration_backup,
        };
        storage.migrate_if_needed()?;
        storage.record_successful_v3_startup_and_cleanup_backups()?;
        Ok(storage)
    }
    pub fn get_schema_version(&self) -> AppResult<u32> {
        Ok(self.get_schema_version_optional()?.unwrap_or(1))
    }
    pub fn set_schema_version(&self, version: u32) -> AppResult<()> {
        let txn = self.db.begin_write().map_err(storage_error)?;
        util::write_meta_u32(&txn, tables::META_SCHEMA_VERSION, version)?;
        txn.commit().map_err(storage_error)?;
        Ok(())
    }
    pub fn migrate_if_needed(&self) -> AppResult<()> {
        match self.get_schema_version_optional()? {
            Some(version) if version >= tables::SCHEMA_VERSION => {
                self.ensure_current_v3_tables()?;
                return Ok(());
            }
            Some(version) => {
                tracing::info!(schema_version = version, "Migrating redb storage schema");
            }
            None => {}
        }
        if self.has_legacy_data()? || self.get_schema_version_optional()?.is_some() {
            self.migrate_to_v3()?;
        } else {
            self.initialize_v3_schema()?;
        }
        Ok(())
    }
}
