use crate::error::AppResult;
use redb::{ReadableDatabase, ReadableTable};
use std::collections::HashSet;

use super::Storage;
use super::tables::*;
use super::util::*;

impl Storage {
    pub fn load_sessions(&self) -> AppResult<crate::config::SessionsConfig> {
        let groups = self.list_groups()?;
        let mut connections = self.list_connections()?;
        self.hydrate_connection_passwords(&mut connections)?;
        let custom_icons = self.load_connection_custom_icons_with_legacy_migration(&connections)?;
        Ok(crate::config::SessionsConfig {
            groups,
            connections,
            custom_icons,
        })
    }
    pub fn replace_sessions(&self, config: &crate::config::SessionsConfig) -> AppResult<()> {
        let txn = self.db.begin_write().map_err(storage_error)?;
        replace_sessions_in_txn(&txn, config)?;
        txn.commit().map_err(storage_error)?;
        Ok(())
    }
    pub fn list_groups(&self) -> AppResult<Vec<crate::config::Group>> {
        let mut groups = self.list_json_by_prefix(GROUPS_TABLE, GROUP_PREFIX)?;
        groups.sort_by(|left: &crate::config::Group, right| {
            left.sort_order
                .cmp(&right.sort_order)
                .then(left.name.cmp(&right.name))
                .then(left.id.cmp(&right.id))
        });
        Ok(groups)
    }
    pub fn get_group(&self, group_id: &str) -> AppResult<Option<crate::config::Group>> {
        self.read_json(GROUPS_TABLE, &entity_key(GROUP_PREFIX, group_id))
    }
    pub fn save_group(&self, group: &crate::config::Group) -> AppResult<()> {
        let txn = self.db.begin_write().map_err(storage_error)?;
        save_group_in_txn(&txn, group)?;
        txn.commit().map_err(storage_error)?;
        Ok(())
    }
    pub fn delete_group(&self, group_id: &str) -> AppResult<()> {
        self.remove_key(GROUPS_TABLE, &entity_key(GROUP_PREFIX, group_id))
    }
    pub fn list_connections(&self) -> AppResult<Vec<crate::config::SavedConnection>> {
        let mut connections = self.list_json_by_prefix(CONNECTIONS_TABLE, CONNECTION_PREFIX)?;
        sort_connections(&mut connections);
        Ok(connections)
    }
    pub fn get_connection(
        &self,
        connection_id: &str,
    ) -> AppResult<Option<crate::config::SavedConnection>> {
        self.read_json(
            CONNECTIONS_TABLE,
            &entity_key(CONNECTION_PREFIX, connection_id),
        )
    }
    pub fn get_connection_with_secret(
        &self,
        connection_id: &str,
    ) -> AppResult<Option<crate::config::SavedConnection>> {
        let Some(mut connection) = self.get_connection(connection_id)? else {
            return Ok(None);
        };
        self.hydrate_connection_passwords(std::slice::from_mut(&mut connection))?;
        Ok(Some(connection))
    }
    pub fn save_connection(&self, connection: &crate::config::SavedConnection) -> AppResult<()> {
        let txn = self.db.begin_write().map_err(storage_error)?;
        save_connection_in_txn(&txn, connection)?;
        txn.commit().map_err(storage_error)?;
        Ok(())
    }
    pub fn delete_connection(&self, connection_id: &str) -> AppResult<()> {
        let txn = self.db.begin_write().map_err(storage_error)?;
        delete_connection_in_txn(&txn, connection_id)?;
        txn.commit().map_err(storage_error)?;
        Ok(())
    }
    pub fn list_connection_custom_icons(
        &self,
    ) -> AppResult<Vec<crate::config::ConnectionCustomIcon>> {
        let mut icons =
            self.list_json_by_prefix(CONNECTIONS_TABLE, CONNECTION_CUSTOM_ICON_PREFIX)?;
        sort_connection_custom_icons(&mut icons);
        Ok(icons)
    }
    pub fn save_connection_custom_icon(
        &self,
        icon: &crate::config::ConnectionCustomIcon,
    ) -> AppResult<()> {
        let key = entity_key(CONNECTION_CUSTOM_ICON_PREFIX, &icon.id);
        self.write_json(CONNECTIONS_TABLE, &key, icon)
    }
    pub fn delete_connection_custom_icon(&self, icon_id: &str) -> AppResult<()> {
        self.remove_key(
            CONNECTIONS_TABLE,
            &entity_key(CONNECTION_CUSTOM_ICON_PREFIX, icon_id),
        )
    }
    fn load_connection_custom_icons_with_legacy_migration(
        &self,
        connections: &[crate::config::SavedConnection],
    ) -> AppResult<Vec<crate::config::ConnectionCustomIcon>> {
        let mut icons = self.list_connection_custom_icons()?;
        let mut seen = icons
            .iter()
            .map(|icon| icon.id.clone())
            .collect::<HashSet<_>>();
        let now = current_time_ms();
        let mut migrated = Vec::new();

        for connection in connections {
            let Some(icon) = connection.icon.as_deref() else {
                continue;
            };
            let Some(record) = crate::config::connection_custom_icon_from_data_url(
                icon,
                "Migrated custom icon",
                now,
            ) else {
                continue;
            };
            if seen.insert(record.id.clone()) {
                self.save_connection_custom_icon(&record)?;
                migrated.push(record);
            }
        }

        icons.extend(migrated);
        sort_connection_custom_icons(&mut icons);
        Ok(icons)
    }
    pub fn mark_connection_used(&self, connection_id: &str) -> AppResult<()> {
        let txn = self.db.begin_write().map_err(storage_error)?;
        let key = entity_key(CONNECTION_PREFIX, connection_id);
        let connection = {
            let table = txn.open_table(CONNECTIONS_TABLE).map_err(storage_error)?;
            let connection = table
                .get(key.as_str())
                .map_err(storage_error)?
                .map(|raw| deserialize_json::<crate::config::SavedConnection>(raw.value()))
                .transpose()?;
            connection
        };
        let Some(mut connection) = connection else {
            txn.commit().map_err(storage_error)?;
            return Ok(());
        };
        connection.last_used_at_ms = Some(current_time_ms());
        hydrate_connection_password_in_txn(&txn, &mut connection)?;
        save_connection_in_txn(&txn, &connection)?;
        txn.commit().map_err(storage_error)?;
        Ok(())
    }
    pub fn list_connections_by_group(
        &self,
        group_id: Option<&str>,
    ) -> AppResult<Vec<crate::config::SavedConnection>> {
        let txn = self.db.begin_read().map_err(storage_error)?;
        let index_table = match txn.open_table(IDX_CONNECTIONS_BY_GROUP_TABLE) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(error) => return Err(storage_error(error)),
        };
        let connections_table = match txn.open_table(CONNECTIONS_TABLE) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(error) => return Err(storage_error(error)),
        };
        let prefix = format!("{}|", group_id.unwrap_or_default());
        let mut connections = Vec::new();
        for entry in index_table.iter().map_err(storage_error)? {
            let (key, value) = entry.map_err(storage_error)?;
            if !key.value().starts_with(&prefix) {
                continue;
            }
            let connection_key = entity_key(CONNECTION_PREFIX, value.value());
            if let Some(raw) = connections_table
                .get(connection_key.as_str())
                .map_err(storage_error)?
            {
                connections.push(deserialize_json(raw.value())?);
            }
        }
        Ok(connections)
    }
}

pub(super) fn replace_sessions_in_txn(
    txn: &redb::WriteTransaction,
    config: &crate::config::SessionsConfig,
) -> AppResult<()> {
    clear_prefix_in_txn(txn, GROUPS_TABLE, GROUP_PREFIX)?;
    clear_prefix_in_txn(txn, CONNECTIONS_TABLE, CONNECTION_PREFIX)?;
    clear_prefix_in_txn(txn, CONNECTIONS_TABLE, CONNECTION_CUSTOM_ICON_PREFIX)?;
    clear_prefix_in_txn(txn, CREDENTIALS_TABLE, CONNECTION_PASSWORD_PREFIX)?;
    clear_string_prefix_in_txn(txn, IDX_CONNECTIONS_BY_GROUP_TABLE, "")?;
    clear_string_prefix_in_txn(txn, IDX_CONNECTIONS_BY_LAST_USED_TABLE, "")?;
    clear_string_prefix_in_txn(txn, IDX_CONNECTIONS_BY_PROTOCOL_TABLE, "")?;
    for group in &config.groups {
        save_group_in_txn(txn, group)?;
    }
    for connection in &config.connections {
        save_connection_in_txn(txn, connection)?;
    }
    for icon in &config.custom_icons {
        save_connection_custom_icon_in_txn(txn, icon)?;
    }
    Ok(())
}

/// Preserve the serialized Sync snapshot fields, including timestamps, so a
/// rebuild after merge has the same payload hash as the published snapshot.
/// Ordinary editing must continue to use replace_sessions_in_txn instead.
pub(super) fn replace_synced_sessions_in_txn(
    txn: &redb::WriteTransaction,
    config: &crate::config::SessionsConfig,
) -> AppResult<()> {
    clear_prefix_in_txn(txn, GROUPS_TABLE, GROUP_PREFIX)?;
    clear_prefix_in_txn(txn, CONNECTIONS_TABLE, CONNECTION_PREFIX)?;
    clear_prefix_in_txn(txn, CONNECTIONS_TABLE, CONNECTION_CUSTOM_ICON_PREFIX)?;
    clear_prefix_in_txn(txn, CREDENTIALS_TABLE, CONNECTION_PASSWORD_PREFIX)?;
    clear_string_prefix_in_txn(txn, IDX_CONNECTIONS_BY_GROUP_TABLE, "")?;
    clear_string_prefix_in_txn(txn, IDX_CONNECTIONS_BY_LAST_USED_TABLE, "")?;
    clear_string_prefix_in_txn(txn, IDX_CONNECTIONS_BY_PROTOCOL_TABLE, "")?;

    for group in &config.groups {
        write_json_in_txn(
            txn,
            GROUPS_TABLE,
            &entity_key(GROUP_PREFIX, &group.id),
            group,
        )?;
    }
    for connection in &config.connections {
        let mut connection = connection.clone();
        if let Some(auth) = connection.auth.as_mut() {
            if let Some(password) = auth.password.take().filter(|value| !value.is_empty()) {
                let now = current_time_ms();
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
        write_json_in_txn(
            txn,
            CONNECTIONS_TABLE,
            &entity_key(CONNECTION_PREFIX, &connection.id),
            &connection,
        )?;
        insert_connection_indexes(txn, &connection)?;
    }
    for icon in &config.custom_icons {
        save_connection_custom_icon_in_txn(txn, icon)?;
    }
    Ok(())
}
pub(super) fn save_group_in_txn(
    txn: &redb::WriteTransaction,
    group: &crate::config::Group,
) -> AppResult<()> {
    let mut group = group.clone();
    let now = current_time_ms();
    let key = entity_key(GROUP_PREFIX, &group.id);
    if group.created_at_ms.is_none() {
        group.created_at_ms = existing_group_created_at(txn, &key)?.or(Some(now));
    }
    group.updated_at_ms = Some(now);
    write_json_in_txn(txn, GROUPS_TABLE, &key, &group)
}
fn existing_group_created_at(txn: &redb::WriteTransaction, key: &str) -> AppResult<Option<u64>> {
    let table = txn.open_table(GROUPS_TABLE).map_err(storage_error)?;
    let Some(raw) = table.get(key).map_err(storage_error)? else {
        return Ok(None);
    };
    let group: crate::config::Group = deserialize_json(raw.value())?;
    Ok(group.created_at_ms)
}
pub(super) fn save_connection_in_txn(
    txn: &redb::WriteTransaction,
    connection: &crate::config::SavedConnection,
) -> AppResult<()> {
    let mut connection = connection.clone();
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
pub(super) fn save_connection_custom_icon_in_txn(
    txn: &redb::WriteTransaction,
    icon: &crate::config::ConnectionCustomIcon,
) -> AppResult<()> {
    write_json_in_txn(
        txn,
        CONNECTIONS_TABLE,
        &entity_key(CONNECTION_CUSTOM_ICON_PREFIX, &icon.id),
        icon,
    )
}
fn hydrate_connection_password_in_txn(
    txn: &redb::WriteTransaction,
    connection: &mut crate::config::SavedConnection,
) -> AppResult<()> {
    let Some(auth) = connection.auth.as_mut() else {
        return Ok(());
    };
    if auth.password.is_some() {
        return Ok(());
    }
    let table = txn.open_table(CREDENTIALS_TABLE).map_err(storage_error)?;
    let key = entity_key(CONNECTION_PASSWORD_PREFIX, &connection.id);
    if let Some(raw) = table.get(key.as_str()).map_err(storage_error)? {
        let record: ConnectionPasswordRecord = deserialize_json(raw.value())?;
        auth.password = Some(record.password);
        auth.has_password = true;
    }
    Ok(())
}
fn existing_connection_created_at(
    txn: &redb::WriteTransaction,
    key: &str,
) -> AppResult<Option<u64>> {
    let table = txn.open_table(CONNECTIONS_TABLE).map_err(storage_error)?;
    let Some(raw) = table.get(key).map_err(storage_error)? else {
        return Ok(None);
    };
    let connection: crate::config::SavedConnection = deserialize_json(raw.value())?;
    Ok(connection.created_at_ms)
}
pub(super) fn delete_connection_in_txn(
    txn: &redb::WriteTransaction,
    connection_id: &str,
) -> AppResult<()> {
    {
        let mut table = txn.open_table(CONNECTIONS_TABLE).map_err(storage_error)?;
        table
            .remove(entity_key(CONNECTION_PREFIX, connection_id).as_str())
            .map_err(storage_error)?;
    }
    delete_connection_password_in_txn(txn, connection_id)?;
    remove_connection_index_entries(txn, connection_id)?;
    Ok(())
}
fn delete_connection_password_in_txn(
    txn: &redb::WriteTransaction,
    connection_id: &str,
) -> AppResult<()> {
    let mut table = txn.open_table(CREDENTIALS_TABLE).map_err(storage_error)?;
    table
        .remove(entity_key(CONNECTION_PASSWORD_PREFIX, connection_id).as_str())
        .map_err(storage_error)?;
    Ok(())
}
fn insert_connection_indexes(
    txn: &redb::WriteTransaction,
    connection: &crate::config::SavedConnection,
) -> AppResult<()> {
    insert_connection_group_index(txn, connection)?;
    insert_connection_last_used_index(txn, connection)?;
    insert_connection_protocol_index(txn, connection)
}
fn insert_connection_group_index(
    txn: &redb::WriteTransaction,
    connection: &crate::config::SavedConnection,
) -> AppResult<()> {
    let group_id = connection.group_id.as_deref().unwrap_or_default();
    let key = format!(
        "{}|{}|{}",
        group_id,
        padded_i64(i64::from(connection.sort_order)),
        connection.id
    );
    let mut table = txn
        .open_table(IDX_CONNECTIONS_BY_GROUP_TABLE)
        .map_err(storage_error)?;
    table
        .insert(key.as_str(), connection.id.as_str())
        .map_err(storage_error)?;
    Ok(())
}
fn insert_connection_last_used_index(
    txn: &redb::WriteTransaction,
    connection: &crate::config::SavedConnection,
) -> AppResult<()> {
    let last_used = connection.last_used_at_ms.unwrap_or_default();
    let reverse = u64::MAX.saturating_sub(last_used);
    let key = format!("{reverse:020}|{}", connection.id);
    let mut table = txn
        .open_table(IDX_CONNECTIONS_BY_LAST_USED_TABLE)
        .map_err(storage_error)?;
    table
        .insert(key.as_str(), connection.id.as_str())
        .map_err(storage_error)?;
    Ok(())
}
fn insert_connection_protocol_index(
    txn: &redb::WriteTransaction,
    connection: &crate::config::SavedConnection,
) -> AppResult<()> {
    let protocol = connection_protocol(&connection.config);
    let key = format!("{protocol}|{}", connection.id);
    let mut table = txn
        .open_table(IDX_CONNECTIONS_BY_PROTOCOL_TABLE)
        .map_err(storage_error)?;
    table
        .insert(key.as_str(), connection.id.as_str())
        .map_err(storage_error)?;
    Ok(())
}
fn remove_connection_index_entries(
    txn: &redb::WriteTransaction,
    connection_id: &str,
) -> AppResult<()> {
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
    definition: redb::TableDefinition<&str, &str>,
    connection_id: &str,
) -> AppResult<()> {
    let table = txn.open_table(definition).map_err(storage_error)?;
    let mut keys = Vec::new();
    for entry in table.iter().map_err(storage_error)? {
        let (key, value) = entry.map_err(storage_error)?;
        if value.value() == connection_id || key.value().ends_with(&format!("|{connection_id}")) {
            keys.push(key.value().to_string());
        }
    }
    drop(table);
    let mut table = txn.open_table(definition).map_err(storage_error)?;
    for key in keys {
        table.remove(key.as_str()).map_err(storage_error)?;
    }
    Ok(())
}
pub(super) fn rebuild_all_connection_indexes_in_txn(txn: &redb::WriteTransaction) -> AppResult<()> {
    clear_string_prefix_in_txn(txn, IDX_CONNECTIONS_BY_GROUP_TABLE, "")?;
    clear_string_prefix_in_txn(txn, IDX_CONNECTIONS_BY_LAST_USED_TABLE, "")?;
    clear_string_prefix_in_txn(txn, IDX_CONNECTIONS_BY_PROTOCOL_TABLE, "")?;
    let table = txn.open_table(CONNECTIONS_TABLE).map_err(storage_error)?;
    let mut connections = Vec::new();
    for entry in table.iter().map_err(storage_error)? {
        let (key, value) = entry.map_err(storage_error)?;
        if key.value().starts_with(CONNECTION_PREFIX) {
            connections.push(deserialize_json::<crate::config::SavedConnection>(
                value.value(),
            )?);
        }
    }
    drop(table);
    for connection in connections {
        insert_connection_indexes(txn, &connection)?;
    }
    Ok(())
}
fn connection_protocol(config: &crate::config::ConnectionType) -> &'static str {
    match config {
        crate::config::ConnectionType::Ssh { .. } => "ssh",
        crate::config::ConnectionType::LocalTerminal { .. } => "local_terminal",
        crate::config::ConnectionType::Telnet { .. } => "telnet",
        crate::config::ConnectionType::Serial { .. } => "serial",
        crate::config::ConnectionType::Rdp { .. } => "rdp",
        crate::config::ConnectionType::Vnc { .. } => "vnc",
    }
}
pub(super) fn sort_connections(connections: &mut [crate::config::SavedConnection]) {
    connections.sort_by(|left, right| {
        left.group_id
            .cmp(&right.group_id)
            .then(left.sort_order.cmp(&right.sort_order))
            .then(left.name.cmp(&right.name))
            .then(left.id.cmp(&right.id))
    });
}
pub(super) fn sort_connection_custom_icons(icons: &mut [crate::config::ConnectionCustomIcon]) {
    icons.sort_by(|left, right| {
        left.created_at_ms
            .cmp(&right.created_at_ms)
            .then(left.name.cmp(&right.name))
            .then(left.id.cmp(&right.id))
    });
}
fn padded_i64(value: i64) -> String {
    let shifted = i128::from(value) - i128::from(i64::MIN);
    format!("{shifted:020}")
}
pub(super) fn parse_sessions_config(content: &str) -> AppResult<crate::config::SessionsConfig> {
    let raw: serde_json::Value = serde_json::from_str(content)?;
    let groups = raw
        .get("groups")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
    let raw_connections = raw
        .get("connections")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let mut connections = Vec::new();
    for raw_connection in raw_connections {
        if raw_connection.get("type").is_none() {
            tracing::warn!("Skipping unsupported legacy connection entry without type");
            continue;
        }
        match serde_json::from_value::<crate::config::SavedConnection>(raw_connection) {
            Ok(connection) => connections.push(connection),
            Err(error) => {
                tracing::warn!("Skipping malformed connection during storage migration: {error}");
            }
        }
    }
    let custom_icons = raw
        .get("custom_icons")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
    Ok(crate::config::SessionsConfig {
        groups,
        connections,
        custom_icons,
    })
}
