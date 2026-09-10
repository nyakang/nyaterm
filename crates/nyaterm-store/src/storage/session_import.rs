use std::collections::{HashMap, HashSet};

use nyaterm_core::{
    Group, PreparedSessionImport, SavedConnection, SavedPassword, SecretString, SshKey,
};

use super::vault::bump_ssh_key_revision;
use super::{
    CREDENTIALS_TABLE, ConnectionStore, LEGACY_TEXT_MASTER_KEY, META_MASTER_KEY, META_TABLE,
    PASSWORD_PREFIX, SSH_KEY_PREFIX, StorageError, TEXT_DOCS_TABLE, entity_key,
    save_connection_in_txn, save_group_in_txn, write_json_in_txn,
};

impl ConnectionStore {
    pub fn commit_session_import(
        &self,
        prepared: PreparedSessionImport,
    ) -> Result<usize, StorageError> {
        if prepared.connections.is_empty() {
            return Ok(0);
        }

        let mut groups = self.list_groups()?;
        let existing_group_count = groups.len();
        let mut path_map = build_group_path_map(&groups);
        let mut next_sort = groups
            .iter()
            .map(|group| group.sort_order)
            .max()
            .unwrap_or(0)
            + 1;

        for group_path in &prepared.groups {
            ensure_group_path(&mut groups, &mut path_map, &mut next_sort, group_path);
        }

        let count = prepared.connections.len();
        let mut connections = Vec::with_capacity(count);
        for connection in prepared.connections {
            let group_id = connection.group_path.as_ref().and_then(|segments| {
                ensure_group_path(&mut groups, &mut path_map, &mut next_sort, segments)
            });
            if let Some(mut saved) = connection.saved {
                saved.id = uuid::Uuid::new_v4().to_string();
                saved.name = connection.name;
                saved.group_id = group_id;
                saved.description = connection.description;
                saved.sort_order = connection.sort_order;
                saved.icon = connection.icon;
                saved.auth = connection.auth;
                connections.push(saved);
                continue;
            }
            connections.push(SavedConnection {
                extensions: Default::default(),
                id: uuid::Uuid::new_v4().to_string(),
                name: connection.name,
                config: connection.config,
                group_id,
                description: connection.description,
                sort_order: connection.sort_order,
                icon: connection.icon,
                icon_auto_detect: None,
                auth: connection.auth,
                network: None,
                post_login: None,
                recording: None,
                ssh_algorithms: None,
                ssh_profile: Default::default(),
                terminal_type: None,
                sftp: Default::default(),
                asset: None,
                created_at_ms: None,
                updated_at_ms: None,
                last_used_at_ms: None,
            });
        }

        let crypto = self.credential_crypto()?;
        let needs_master_key = prepared.passwords.iter().any(|entry| {
            entry
                .password
                .as_ref()
                .map(SecretString::expose_secret)
                .is_some_and(|value| !value.is_empty())
        }) || prepared.ssh_keys.iter().any(ssh_key_has_secret);
        let existing_master_key = self.load_master_key_token()?;
        let generated_master_key = if needs_master_key && existing_master_key.is_none() {
            Some(crypto.generate_master_key_token()?)
        } else {
            None
        };
        let master_key = existing_master_key
            .as_deref()
            .or(generated_master_key.as_deref());
        let passwords = prepared
            .passwords
            .into_iter()
            .map(|entry| prepare_password(entry, &crypto, master_key))
            .collect::<Result<Vec<_>, _>>()?;
        let ssh_keys = prepared
            .ssh_keys
            .into_iter()
            .map(|entry| prepare_ssh_key(entry, &crypto, master_key))
            .collect::<Result<Vec<_>, _>>()?;

        let txn = self.db.begin_write()?;
        if let Some(token) = generated_master_key.as_deref() {
            txn.open_table(META_TABLE)?.insert(META_MASTER_KEY, token)?;
            txn.open_table(TEXT_DOCS_TABLE)?
                .insert(LEGACY_TEXT_MASTER_KEY, token)?;
        }
        for password in &passwords {
            write_json_in_txn(
                &txn,
                CREDENTIALS_TABLE,
                &entity_key(PASSWORD_PREFIX, &password.id),
                password,
            )?;
        }
        for key in &ssh_keys {
            write_json_in_txn(
                &txn,
                CREDENTIALS_TABLE,
                &entity_key(SSH_KEY_PREFIX, &key.id),
                key,
            )?;
        }
        for group in &groups[existing_group_count..] {
            save_group_in_txn(&txn, group)?;
        }
        for icon in &prepared.custom_icons {
            write_json_in_txn(
                &txn,
                super::CONNECTIONS_TABLE,
                &entity_key("connection_custom_icons/", &icon.id),
                icon,
            )?;
        }
        for connection in &connections {
            save_connection_in_txn(&txn, connection)?;
        }
        txn.commit()?;
        if !ssh_keys.is_empty() {
            bump_ssh_key_revision();
        }
        Ok(count)
    }
}

fn ssh_key_has_secret(key: &SshKey) -> bool {
    [&key.key, &key.cert, &key.passphrase]
        .into_iter()
        .any(|value| {
            value
                .as_ref()
                .map(SecretString::expose_secret)
                .is_some_and(|value| !value.trim().is_empty())
        })
}

fn prepare_password(
    mut entry: SavedPassword,
    crypto: &nyaterm_core::CredentialCrypto,
    master_key: Option<&str>,
) -> Result<SavedPassword, StorageError> {
    entry.password = match entry.password.as_ref().map(SecretString::expose_secret) {
        Some(value) if !value.is_empty() => Some(
            crypto
                .encrypt_secret(master_key.ok_or(StorageError::MissingMasterKey)?, value)?
                .into(),
        ),
        _ => None,
    };
    entry.has_password = entry.password.is_some();
    Ok(entry)
}

fn prepare_ssh_key(
    mut entry: SshKey,
    crypto: &nyaterm_core::CredentialCrypto,
    master_key: Option<&str>,
) -> Result<SshKey, StorageError> {
    entry.key = encrypt_import_secret(entry.key, crypto, master_key)?;
    entry.cert = encrypt_import_secret(entry.cert, crypto, master_key)?;
    entry.passphrase = encrypt_import_secret(entry.passphrase, crypto, master_key)?;
    entry.key_file_path = None;
    entry.cert_file_path = None;
    entry.has_key_data = entry.key.is_some();
    entry.has_cert_data = entry.cert.is_some();
    Ok(entry)
}

fn encrypt_import_secret(
    value: Option<SecretString>,
    crypto: &nyaterm_core::CredentialCrypto,
    master_key: Option<&str>,
) -> Result<Option<SecretString>, StorageError> {
    let Some(value) = value.filter(|value| !value.expose_secret().trim().is_empty()) else {
        return Ok(None);
    };
    Ok(Some(
        crypto
            .encrypt_secret(
                master_key.ok_or(StorageError::MissingMasterKey)?,
                value.expose_secret(),
            )?
            .into(),
    ))
}

fn build_group_path(groups: &[Group], id: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = id;
    let mut visited = HashSet::new();
    loop {
        if !visited.insert(current.to_string()) {
            break;
        }
        let Some(group) = groups.iter().find(|group| group.id == current) else {
            break;
        };
        segments.push(group.name.clone());
        let Some(parent_id) = group.parent_id.as_deref() else {
            break;
        };
        current = parent_id;
    }
    segments.reverse();
    segments
}

fn build_group_path_map(groups: &[Group]) -> HashMap<Vec<String>, String> {
    groups
        .iter()
        .map(|group| (build_group_path(groups, &group.id), group.id.clone()))
        .collect()
}

fn ensure_group_path(
    groups: &mut Vec<Group>,
    path_map: &mut HashMap<Vec<String>, String>,
    next_sort: &mut i32,
    segments: &[String],
) -> Option<String> {
    if segments.is_empty() {
        return None;
    }

    let mut leaf_id = String::new();
    for depth in 1..=segments.len() {
        let prefix = segments[..depth].to_vec();
        if let Some(existing) = path_map.get(&prefix) {
            leaf_id.clone_from(existing);
            continue;
        }
        let id = uuid::Uuid::new_v4().to_string();
        let parent_id = (depth > 1)
            .then(|| path_map.get(&segments[..depth - 1]).cloned())
            .flatten();
        groups.push(Group {
            id: id.clone(),
            name: segments[depth - 1].clone(),
            parent_id,
            sort_order: *next_sort,
            created_at_ms: None,
            updated_at_ms: None,
        });
        *next_sort += 1;
        path_map.insert(prefix, id.clone());
        leaf_id = id;
    }
    Some(leaf_id)
}

#[cfg(test)]
mod tests {
    use nyaterm_core::{
        AiExecutionProfile, ConnectionType, PreparedSessionConnection, PreparedSessionImport,
        SavedPassword,
    };

    use super::{ConnectionStore, META_MASTER_KEY, META_TABLE};

    #[test]
    fn commit_session_import_persists_all_domains_together() {
        let dir = crate::storage::tests::unique_temp_dir("session-import-atomic-success");
        let store = ConnectionStore::open(&dir).expect("open store");

        let count = store
            .commit_session_import(prepared_import())
            .expect("commit import");

        assert_eq!(count, 1);
        assert_eq!(store.list_groups().expect("groups").len(), 1);
        assert_eq!(store.list_connections().expect("connections").len(), 1);
        let passwords = store.list_passwords().expect("passwords");
        assert_eq!(passwords.len(), 1);
        assert!(passwords[0].has_password);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn commit_session_import_leaves_database_unchanged_when_secret_validation_fails() {
        let dir = crate::storage::tests::unique_temp_dir("session-import-atomic-failure");
        let store = ConnectionStore::open(&dir).expect("open store");
        let txn = store.db.begin_write().expect("write transaction");
        txn.open_table(META_TABLE)
            .expect("meta table")
            .insert(META_MASTER_KEY, "invalid-master-key-token")
            .expect("insert corrupt token");
        txn.commit().expect("commit corrupt token");

        assert!(store.commit_session_import(prepared_import()).is_err());
        assert!(store.list_groups().expect("groups").is_empty());
        assert!(store.list_connections().expect("connections").is_empty());
        assert!(store.list_passwords().expect("passwords").is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    fn prepared_import() -> PreparedSessionImport {
        PreparedSessionImport {
            custom_icons: Vec::new(),
            groups: vec![vec!["Imported".to_string()]],
            passwords: vec![SavedPassword {
                id: "password-1".to_string(),
                name: "Imported password".to_string(),
                password: Some("secret-value".to_string().into()),
                has_password: false,
            }],
            ssh_keys: Vec::new(),
            connections: vec![PreparedSessionConnection {
                saved: None,
                name: "Imported shell".to_string(),
                config: ConnectionType::LocalTerminal {
                    shell_path: "/bin/sh".to_string(),
                    shell_args: String::new(),
                    working_dir: None,
                    ai_execution_profile: AiExecutionProfile::Auto,
                    encoding: String::new(),
                    dynamic_tab_title: false,
                },
                group_path: Some(vec!["Imported".to_string()]),
                description: None,
                sort_order: 0,
                icon: None,
                auth: None,
            }],
        }
    }
}
