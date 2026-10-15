use super::{
    SettingsDocKey, Storage,
    credentials::*,
    history::replace_command_history_in_txn,
    known_hosts::replace_known_hosts_text_in_txn,
    notes::replace_notes_in_txn,
    sessions::{replace_sessions_in_txn, replace_synced_sessions_in_txn},
    tables::SETTINGS_TABLE,
    util::*,
};
use crate::{config, core::portable_snapshot::PortableSnapshot, error::AppResult};

pub fn restore_backup(
    snapshot: &PortableSnapshot,
    settings: &config::AppSettings,
) -> AppResult<()> {
    super::storage()?.restore_backup(snapshot, settings)
}

impl Storage {
    pub fn restore_backup(
        &self,
        snapshot: &PortableSnapshot,
        settings: &config::AppSettings,
    ) -> AppResult<()> {
        crate::core::portable_snapshot::validate_portable_snapshot(snapshot)?;
        let txn = self.db.begin_write().map_err(storage_error)?;
        replace_sessions_in_txn(&txn, &snapshot.sessions)?;
        replace_passwords_in_txn(&txn, &snapshot.passwords)?;
        replace_ssh_keys_in_txn(&txn, &snapshot.keys)?;
        replace_credentials_in_txn(&txn, &snapshot.credentials)?;
        replace_otp_in_txn(&txn, &snapshot.otp)?;
        replace_proxies_in_txn(&txn, &snapshot.proxies)?;
        replace_tunnels_in_txn(&txn, &snapshot.tunnels)?;
        replace_command_history_in_txn(&txn, &snapshot.history)?;
        replace_notes_in_txn(&txn, &snapshot.notes)?;
        replace_known_hosts_text_in_txn(&txn, &snapshot.known_hosts)?;
        for (key, value) in [
            (SettingsDocKey::AppSettings, serde_json::to_value(settings)?),
            (
                SettingsDocKey::QuickCommands,
                serde_json::to_value(&snapshot.quick_commands)?,
            ),
            (
                SettingsDocKey::ProxyGroups,
                serde_json::json!({"groups":snapshot.proxy_groups}),
            ),
            (
                SettingsDocKey::TunnelGroups,
                serde_json::json!({"groups":snapshot.tunnel_groups}),
            ),
        ] {
            write_json_in_txn(&txn, SETTINGS_TABLE, key.storage_key(), &value)?;
        }
        let mut sync = self
            .get_settings_doc::<config::CloudSyncState>(SettingsDocKey::CloudSyncState)?
            .unwrap_or_default();
        sync.last_synced_payload_hash = None;
        sync.last_applied_remote_revision = None;
        write_json_in_txn(
            &txn,
            SETTINGS_TABLE,
            SettingsDocKey::CloudSyncState.storage_key(),
            &sync,
        )?;
        txn.commit().map_err(storage_error)?;
        Ok(())
    }

    pub fn import_connections(
        &self,
        sessions: &config::SessionsConfig,
        passwords: &config::PasswordsConfig,
        keys: &config::KeysConfig,
    ) -> AppResult<()> {
        let txn = self.db.begin_write().map_err(storage_error)?;
        replace_sessions_in_txn(&txn, sessions)?;
        replace_passwords_in_txn(&txn, passwords)?;
        replace_ssh_keys_in_txn(&txn, keys)?;
        txn.commit().map_err(storage_error)?;
        Ok(())
    }
}

pub fn import_connections(
    sessions: &config::SessionsConfig,
    passwords: &config::PasswordsConfig,
    keys: &config::KeysConfig,
) -> AppResult<()> {
    super::storage()?.import_connections(sessions, passwords, keys)
}

/// Commit only the synchronized entities touched by additive connection merge.
/// History, app settings, notes, known hosts and device-local data are untouched.
pub fn apply_merged_sync_entities(
    snapshot: &PortableSnapshot,
    sessions: &config::SessionsConfig,
) -> AppResult<()> {
    crate::core::portable_snapshot::validate_portable_snapshot(snapshot)?;
    validate_backup_data(snapshot)?;
    super::storage()?.apply_merged_sync_entities(snapshot, sessions)
}

impl Storage {
    pub(super) fn apply_merged_sync_entities(
        &self,
        snapshot: &PortableSnapshot,
        sessions: &config::SessionsConfig,
    ) -> AppResult<()> {
        let txn = self.db.begin_write().map_err(storage_error)?;
        replace_synced_sessions_in_txn(&txn, sessions)?;
        replace_passwords_in_txn(&txn, &snapshot.passwords)?;
        replace_ssh_keys_in_txn(&txn, &snapshot.keys)?;
        replace_otp_in_txn(&txn, &snapshot.otp)?;
        replace_proxies_in_txn(&txn, &snapshot.proxies)?;
        write_json_in_txn(
            &txn,
            SETTINGS_TABLE,
            SettingsDocKey::ProxyGroups.storage_key(),
            &serde_json::json!({ "groups": snapshot.proxy_groups }),
        )?;
        txn.commit().map_err(storage_error)?;
        Ok(())
    }
}

/// Reject ambiguous entity IDs before writes can silently replace another entity.
pub fn validate_backup_data(snapshot: &PortableSnapshot) -> AppResult<()> {
    fn unique<'a>(ids: impl Iterator<Item = &'a str>) -> AppResult<()> {
        let mut seen = std::collections::HashSet::new();
        for id in ids {
            if id.trim().is_empty() || !seen.insert(id) {
                return Err(crate::error::AppError::Config(
                    "Backup contains empty or duplicate entity IDs".into(),
                ));
            }
        }
        Ok(())
    }
    macro_rules! check {
        ($values:expr) => {
            unique($values.iter().map(|entry| entry.id.as_str()))?
        };
    }
    check!(snapshot.sessions.connections);
    check!(snapshot.sessions.groups);
    check!(snapshot.sessions.custom_icons);
    check!(snapshot.keys.keys);
    check!(snapshot.passwords.passwords);
    check!(snapshot.credentials.credentials);
    check!(snapshot.otp.entries);
    check!(snapshot.proxies);
    check!(snapshot.proxy_groups);
    check!(snapshot.tunnels);
    check!(snapshot.tunnel_groups);
    check!(snapshot.quick_commands.commands);
    check!(snapshot.quick_commands.categories);
    super::notes::validate_notes_snapshot(&snapshot.notes)
}
