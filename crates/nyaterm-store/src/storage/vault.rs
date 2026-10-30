//! Secret vault persistence: SSH keys, OTP entries, passwords and credentials.
//!
//! Split out of `storage.rs` by domain. These four record kinds share the
//! `credentials` table and the same encrypt-on-write / decrypt-on-read shape.
//! Table name, key prefixes and record layout are unchanged.

use nyaterm_core::models::credentials::PasswordUpdate;
use redb::ReadableTable;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{
    CREDENTIAL_PREFIX, CREDENTIALS_TABLE, ConnectionStore, OTP_ACCOUNTS_TABLE, OTP_PREFIX,
    PASSWORD_PREFIX, SSH_KEY_FILE_IMPORT_MAX_BYTES, SSH_KEY_PREFIX, StorageError,
    decrypt_optional_secret, deserialize_json, entity_key, write_json_in_txn,
};
use nyaterm_core::{
    DecryptedOtpEntry, DecryptedSavedCredential, DecryptedSavedPassword, DecryptedSshKey, OtpEntry,
    SavedCredential, SavedPassword, SecretString, SshKey,
};

static SSH_KEY_REVISION: AtomicU64 = AtomicU64::new(0);

pub(crate) fn bump_ssh_key_revision() {
    SSH_KEY_REVISION.fetch_add(1, Ordering::AcqRel);
}

impl ConnectionStore {
    pub fn ssh_key_revision(&self) -> u64 {
        SSH_KEY_REVISION.load(Ordering::Acquire)
    }

    pub fn list_ssh_keys(&self) -> Result<Vec<SshKey>, StorageError> {
        let mut keys = self.list_json_by_prefix(CREDENTIALS_TABLE, SSH_KEY_PREFIX)?;
        for key in &mut keys {
            apply_ssh_key_status_flags(key);
        }
        keys.sort_by(|left: &SshKey, right| {
            left.sort_order
                .cmp(&right.sort_order)
                .then(left.name.cmp(&right.name))
                .then(left.id.cmp(&right.id))
        });
        Ok(keys)
    }

    pub fn load_ssh_key_by_id(&self, key_id: &str) -> Result<Option<SshKey>, StorageError> {
        let key = entity_key(SSH_KEY_PREFIX, key_id);
        let Some(mut key) = self.read_json_table(CREDENTIALS_TABLE, &key)? else {
            return Ok(None);
        };
        apply_ssh_key_status_flags(&mut key);
        Ok(Some(key))
    }

    pub fn load_decrypted_ssh_key_by_id(
        &self,
        key_id: &str,
    ) -> Result<Option<DecryptedSshKey>, StorageError> {
        let Some(key) = self.load_ssh_key_by_id(key_id)? else {
            return Ok(None);
        };
        let master_key_token = self.load_master_key_token()?;
        let crypto = self.credential_crypto()?;
        let decrypted = DecryptedSshKey {
            id: key.id,
            name: key.name,
            key_data: decrypt_optional_secret(&crypto, master_key_token.as_deref(), &key.key)?,
            cert_data: decrypt_optional_secret(&crypto, master_key_token.as_deref(), &key.cert)?,
            passphrase: decrypt_optional_secret(
                &crypto,
                master_key_token.as_deref(),
                &key.passphrase,
            )?,
        };
        Ok(Some(decrypted))
    }

    pub fn list_otp_entries(&self) -> Result<Vec<OtpEntry>, StorageError> {
        let mut entries: Vec<OtpEntry> =
            self.list_json_by_prefix(OTP_ACCOUNTS_TABLE, OTP_PREFIX)?;
        for entry in &mut entries {
            entry.has_secret = entry.secret.is_some();
        }
        entries.sort_by(|left, right| {
            left.issuer
                .cmp(&right.issuer)
                .then(left.username.cmp(&right.username))
                .then(left.id.cmp(&right.id))
        });
        Ok(entries)
    }

    pub fn load_otp_entry_by_id(&self, otp_id: &str) -> Result<Option<OtpEntry>, StorageError> {
        let key = entity_key(OTP_PREFIX, otp_id);
        let Some(mut entry) = self.read_json_table::<OtpEntry>(OTP_ACCOUNTS_TABLE, &key)? else {
            return Ok(None);
        };
        entry.has_secret = entry.secret.is_some();
        Ok(Some(entry))
    }

    pub fn load_decrypted_otp_entry_by_id(
        &self,
        otp_id: &str,
    ) -> Result<Option<DecryptedOtpEntry>, StorageError> {
        let Some(entry) = self.load_otp_entry_by_id(otp_id)? else {
            return Ok(None);
        };
        let master_key_token = self.load_master_key_token()?;
        let crypto = self.credential_crypto()?;
        Ok(Some(DecryptedOtpEntry {
            id: entry.id,
            otp_type: entry.otp_type,
            issuer: entry.issuer,
            username: entry.username,
            secret: decrypt_optional_secret(&crypto, master_key_token.as_deref(), &entry.secret)?,
            algorithm: entry.algorithm,
            digits: entry.digits,
            period: entry.period,
            counter: entry.counter,
        }))
    }

    pub fn increment_otp_counter(&self, otp_id: &str) -> Result<(), StorageError> {
        let key = entity_key(OTP_PREFIX, otp_id);
        let txn = self.db.begin_write()?;
        {
            let table = txn.open_table(OTP_ACCOUNTS_TABLE)?;
            let Some(raw) = table.get(key.as_str())? else {
                drop(table);
                txn.commit()?;
                return Ok(());
            };
            let mut entry: OtpEntry = deserialize_json(raw.value())?;
            entry.counter = entry.counter.saturating_add(1);
            drop(raw);
            drop(table);
            write_json_in_txn(&txn, OTP_ACCOUNTS_TABLE, &key, &entry)?;
        }
        txn.commit()?;
        Ok(())
    }

    pub fn save_ssh_key(&self, mut key: SshKey) -> Result<String, StorageError> {
        let is_new = key.id.trim().is_empty();
        if is_new {
            key.id = uuid::Uuid::new_v4().to_string();
        }
        let target_id = key.id.clone();
        let existing = self.load_ssh_key_by_id(&target_id)?;
        key.sort_order = if let Some(existing) = &existing {
            existing.sort_order
        } else if is_new {
            self.list_ssh_keys()?
                .iter()
                .map(|entry| entry.sort_order)
                .max()
                .unwrap_or(-1)
                .saturating_add(1)
        } else {
            key.sort_order
        };
        let crypto = self.credential_crypto()?;

        key.key = if let Some(path) = key
            .key_file_path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            let content = read_limited_ssh_key_file(path, "key material")?;
            let token = self.get_or_create_master_key_token(&crypto)?;
            Some(crypto.encrypt_secret(&token, &content)?.into())
        } else if let Some(plain) = key
            .key
            .as_ref()
            .map(SecretString::expose_secret)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            // Treat non-empty draft key material as plaintext replacement.
            let token = self.get_or_create_master_key_token(&crypto)?;
            Some(crypto.encrypt_secret(&token, plain)?.into())
        } else {
            existing.as_ref().and_then(|entry| entry.key.clone())
        };

        key.cert = if let Some(path) = key
            .cert_file_path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            let content = read_limited_ssh_key_file(path, "certificate")?;
            let token = self.get_or_create_master_key_token(&crypto)?;
            Some(crypto.encrypt_secret(&token, &content)?.into())
        } else if let Some(plain) = key
            .cert
            .as_ref()
            .map(SecretString::expose_secret)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let token = self.get_or_create_master_key_token(&crypto)?;
            Some(crypto.encrypt_secret(&token, plain)?.into())
        } else {
            existing.as_ref().and_then(|entry| entry.cert.clone())
        };

        key.passphrase = match key
            .passphrase
            .as_ref()
            .map(SecretString::expose_secret)
            .map(str::trim)
        {
            Some("") => None,
            Some(plain) if !plain.is_empty() => {
                let token = self.get_or_create_master_key_token(&crypto)?;
                Some(crypto.encrypt_secret(&token, plain)?.into())
            }
            _ => existing.as_ref().and_then(|entry| entry.passphrase.clone()),
        };

        key.key_file_path = None;
        key.cert_file_path = None;
        apply_ssh_key_status_flags(&mut key);

        let txn = self.db.begin_write()?;
        write_json_in_txn(
            &txn,
            CREDENTIALS_TABLE,
            &entity_key(SSH_KEY_PREFIX, &target_id),
            &key,
        )?;
        txn.commit()?;
        bump_ssh_key_revision();
        Ok(target_id)
    }

    pub fn delete_ssh_key(&self, key_id: &str) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        super::cloud_sync_v4::record_remove(
            &txn,
            CREDENTIALS_TABLE,
            &entity_key(SSH_KEY_PREFIX, key_id),
        )?;
        txn.open_table(CREDENTIALS_TABLE)?
            .remove(entity_key(SSH_KEY_PREFIX, key_id).as_str())?;
        txn.commit()?;
        bump_ssh_key_revision();
        Ok(())
    }

    pub fn save_otp_entry(&self, mut entry: OtpEntry) -> Result<String, StorageError> {
        if entry.id.trim().is_empty() {
            entry.id = uuid::Uuid::new_v4().to_string();
        }
        let target_id = entry.id.clone();
        let existing = self.load_otp_entry_by_id(&target_id)?;
        let crypto = self.credential_crypto()?;

        entry.secret = match entry
            .secret
            .as_ref()
            .map(SecretString::expose_secret)
            .map(str::trim)
        {
            Some(plain) if !plain.is_empty() => {
                let token = self.get_or_create_master_key_token(&crypto)?;
                Some(crypto.encrypt_secret(&token, plain)?.into())
            }
            _ => existing.as_ref().and_then(|entry| entry.secret.clone()),
        };
        if entry.otp_type.trim().is_empty() {
            entry.otp_type = "totp".to_string();
        }
        if entry.algorithm.trim().is_empty() {
            entry.algorithm = "SHA1".to_string();
        }
        if entry.digits == 0 {
            entry.digits = 6;
        }
        if entry.period == 0 {
            entry.period = 30;
        }
        entry.has_secret = entry.secret.is_some();

        let txn = self.db.begin_write()?;
        write_json_in_txn(
            &txn,
            OTP_ACCOUNTS_TABLE,
            &entity_key(OTP_PREFIX, &target_id),
            &entry,
        )?;
        txn.commit()?;
        Ok(target_id)
    }

    pub fn delete_otp_entry(&self, otp_id: &str) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        super::cloud_sync_v4::record_remove(
            &txn,
            OTP_ACCOUNTS_TABLE,
            &entity_key(OTP_PREFIX, otp_id),
        )?;
        txn.open_table(OTP_ACCOUNTS_TABLE)?
            .remove(entity_key(OTP_PREFIX, otp_id).as_str())?;
        txn.commit()?;
        Ok(())
    }

    pub fn list_passwords(&self) -> Result<Vec<SavedPassword>, StorageError> {
        let mut passwords: Vec<SavedPassword> =
            self.list_json_by_prefix(CREDENTIALS_TABLE, PASSWORD_PREFIX)?;
        for password in &mut passwords {
            password.has_password = password.password.is_some();
            password.password = None;
        }
        passwords.sort_by(|left, right| {
            left.sort_order
                .cmp(&right.sort_order)
                .then(left.name.cmp(&right.name))
                .then(left.id.cmp(&right.id))
        });
        Ok(passwords)
    }

    pub fn load_password_by_id(
        &self,
        password_id: &str,
    ) -> Result<Option<SavedPassword>, StorageError> {
        let key = entity_key(PASSWORD_PREFIX, password_id);
        let Some(mut entry) = self.read_json_table::<SavedPassword>(CREDENTIALS_TABLE, &key)?
        else {
            return Ok(None);
        };
        entry.has_password = entry.password.is_some();
        Ok(Some(entry))
    }

    pub fn load_decrypted_password_by_id(
        &self,
        password_id: &str,
    ) -> Result<Option<DecryptedSavedPassword>, StorageError> {
        let Some(entry) = self.load_password_by_id(password_id)? else {
            return Ok(None);
        };
        let master_key_token = self.load_master_key_token()?;
        let crypto = self.credential_crypto()?;
        Ok(Some(DecryptedSavedPassword {
            id: entry.id,
            name: entry.name,
            username: entry.username,
            password: decrypt_optional_secret(
                &crypto,
                master_key_token.as_deref(),
                &entry.password,
            )?,
        }))
    }

    /// Metadata remains usable without decrypting the account password for connection-source auth.
    pub fn load_account_for_auth(
        &self,
        auth: &nyaterm_core::models::credentials::ConnectionAuth,
    ) -> Result<Option<DecryptedSavedPassword>, StorageError> {
        let Some(id) = auth.saved_account_id() else {
            return Ok(None);
        };
        if auth.uses_account_password() && auth.mode == "password" {
            return self.load_decrypted_password_by_id(id);
        }
        Ok(self
            .load_password_by_id(id)?
            .map(|entry| DecryptedSavedPassword {
                id: entry.id,
                name: entry.name,
                username: entry.username,
                password: None,
            }))
    }

    pub fn save_password(&self, mut entry: SavedPassword) -> Result<String, StorageError> {
        let is_new = entry.id.trim().is_empty();
        if is_new {
            entry.id = uuid::Uuid::new_v4().to_string();
        }
        let target_id = entry.id.clone();
        let existing = self.load_password_by_id(&target_id)?;
        entry.sort_order = if let Some(existing) = &existing {
            existing.sort_order
        } else if is_new {
            self.list_passwords()?
                .iter()
                .map(|entry| entry.sort_order)
                .max()
                .unwrap_or(-1)
                .saturating_add(1)
        } else {
            entry.sort_order
        };
        let crypto = self.credential_crypto()?;
        entry.password = match entry
            .password
            .as_ref()
            .map(SecretString::expose_secret)
            .map(str::trim)
        {
            Some(plain) if !plain.is_empty() => {
                let token = self.get_or_create_master_key_token(&crypto)?;
                Some(crypto.encrypt_secret(&token, plain)?.into())
            }
            _ => existing.as_ref().and_then(|entry| entry.password.clone()),
        };
        entry.has_password = entry.password.is_some();
        let txn = self.db.begin_write()?;
        write_json_in_txn(
            &txn,
            CREDENTIALS_TABLE,
            &entity_key(PASSWORD_PREFIX, &target_id),
            &entry,
        )?;
        txn.commit()?;
        Ok(target_id)
    }

    /// Save an account without overloading an empty input to mean deletion.
    /// Unknown fields and the existing ciphertext are retained in the transaction.
    pub fn save_password_with_update(
        &self,
        mut entry: SavedPassword,
        update: PasswordUpdate,
    ) -> Result<String, StorageError> {
        if entry.name.trim().is_empty() {
            return Err(StorageError::InvalidData("account name is required".into()));
        }
        let replacement = match &update {
            PasswordUpdate::Replace(value) => {
                if value.is_empty() {
                    return Err(StorageError::InvalidData(
                        "replacement password is empty".into(),
                    ));
                }
                let crypto = self.credential_crypto()?;
                let token = self.get_or_create_master_key_token(&crypto)?;
                Some(crypto.encrypt_secret(&token, value.expose_secret())?)
            }
            _ => None,
        };
        let is_new = entry.id.is_empty();
        if is_new {
            entry.id = uuid::Uuid::new_v4().to_string();
        }
        let key = entity_key(PASSWORD_PREFIX, &entry.id);
        let txn = self.db.begin_write()?;
        let mut raw: serde_json::Value = {
            let table = txn.open_table(CREDENTIALS_TABLE)?;
            let previous = table.get(key.as_str())?;
            match previous {
                Some(value) => {
                    let raw: serde_json::Value = deserialize_json(value.value())?;
                    let existing: SavedPassword = serde_json::from_value(raw.clone())?;
                    entry.sort_order = existing.sort_order;
                    raw
                }
                None if !is_new => {
                    return Err(StorageError::InvalidData(
                        "saved account was not found".into(),
                    ));
                }
                None => {
                    let mut order = -1;
                    for item in table.iter()? {
                        let (key, value) = item?;
                        if key.value().starts_with(PASSWORD_PREFIX) {
                            let existing: SavedPassword = deserialize_json(value.value())?;
                            order = order.max(existing.sort_order);
                        }
                    }
                    entry.sort_order = order.saturating_add(1);
                    serde_json::json!({})
                }
            }
        };
        let secret = match update {
            PasswordUpdate::Keep => raw
                .get("password")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            PasswordUpdate::Replace(_) => serde_json::to_value(replacement)?,
            PasswordUpdate::Remove => serde_json::Value::Null,
        };
        if entry.username.trim().is_empty() && secret.is_null() {
            return Err(StorageError::InvalidData(
                "username or password is required".into(),
            ));
        }
        // Only known metadata is replaced; extension fields survive account edits.
        raw["id"] = entry.id.clone().into();
        raw["name"] = entry.name.trim().into();
        raw["username"] = entry.username.trim().into();
        raw["sort_order"] = entry.sort_order.into();
        raw["password"] = secret;
        raw.as_object_mut()
            .expect("validated account object")
            .remove("has_password");
        write_json_in_txn(&txn, CREDENTIALS_TABLE, &key, &raw)?;
        txn.commit()?;
        Ok(entry.id)
    }

    pub fn delete_password(&self, password_id: &str) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        super::cloud_sync_v4::record_remove(
            &txn,
            CREDENTIALS_TABLE,
            &entity_key(PASSWORD_PREFIX, password_id),
        )?;
        txn.open_table(CREDENTIALS_TABLE)?
            .remove(entity_key(PASSWORD_PREFIX, password_id).as_str())?;
        txn.commit()?;
        Ok(())
    }

    pub fn list_credentials(&self) -> Result<Vec<SavedCredential>, StorageError> {
        let mut credentials: Vec<SavedCredential> =
            self.list_json_by_prefix(CREDENTIALS_TABLE, CREDENTIAL_PREFIX)?;
        for credential in &mut credentials {
            credential.has_password = credential.password.is_some();
            credential.password = None;
        }
        credentials.sort_by(|left, right| {
            left.sort_order
                .cmp(&right.sort_order)
                .then(left.id.cmp(&right.id))
        });
        Ok(credentials)
    }

    pub fn load_credential_by_id(
        &self,
        credential_id: &str,
    ) -> Result<Option<SavedCredential>, StorageError> {
        let key = entity_key(CREDENTIAL_PREFIX, credential_id);
        let Some(mut entry) = self.read_json_table::<SavedCredential>(CREDENTIALS_TABLE, &key)?
        else {
            return Ok(None);
        };
        entry.has_password = entry.password.is_some();
        Ok(Some(entry))
    }

    pub fn load_decrypted_credential_by_id(
        &self,
        credential_id: &str,
    ) -> Result<Option<DecryptedSavedCredential>, StorageError> {
        let Some(entry) = self.load_credential_by_id(credential_id)? else {
            return Ok(None);
        };
        let master_key_token = self.load_master_key_token()?;
        let crypto = self.credential_crypto()?;
        Ok(Some(DecryptedSavedCredential {
            id: entry.id,
            sort_order: entry.sort_order,
            name: entry.name,
            username: entry.username,
            password: decrypt_optional_secret(
                &crypto,
                master_key_token.as_deref(),
                &entry.password,
            )?,
            username_prompt_regex: entry.username_prompt_regex,
            password_prompt_regex: entry.password_prompt_regex,
            enabled: entry.enabled,
        }))
    }

    pub fn save_credential(&self, mut entry: SavedCredential) -> Result<String, StorageError> {
        let is_new = entry.id.trim().is_empty();
        if is_new {
            entry.id = uuid::Uuid::new_v4().to_string();
        }
        let target_id = entry.id.clone();
        let existing = self.load_credential_by_id(&target_id)?;
        entry.sort_order = if let Some(existing) = existing.as_ref() {
            existing.sort_order
        } else if is_new {
            self.list_credentials()?
                .into_iter()
                .map(|credential| credential.sort_order)
                .max()
                .unwrap_or(-1)
                .saturating_add(1)
        } else {
            entry.sort_order
        };
        let crypto = self.credential_crypto()?;
        entry.password = match entry
            .password
            .as_ref()
            .map(SecretString::expose_secret)
            .map(str::trim)
        {
            Some(plain) if !plain.is_empty() => {
                let token = self.get_or_create_master_key_token(&crypto)?;
                Some(crypto.encrypt_secret(&token, plain)?.into())
            }
            _ => existing.as_ref().and_then(|entry| entry.password.clone()),
        };
        entry.has_password = entry.password.is_some();
        let txn = self.db.begin_write()?;
        write_json_in_txn(
            &txn,
            CREDENTIALS_TABLE,
            &entity_key(CREDENTIAL_PREFIX, &target_id),
            &entry,
        )?;
        txn.commit()?;
        Ok(target_id)
    }

    pub fn reorder_credentials(&self, updates: &[(String, i32)]) -> Result<(), StorageError> {
        self.reorder_vault_entries::<SavedCredential>(CREDENTIAL_PREFIX, updates)
    }

    pub fn reorder_passwords(&self, updates: &[(String, i32)]) -> Result<(), StorageError> {
        self.reorder_vault_entries::<SavedPassword>(PASSWORD_PREFIX, updates)
    }

    pub fn reorder_ssh_keys(&self, updates: &[(String, i32)]) -> Result<(), StorageError> {
        self.reorder_vault_entries::<SshKey>(SSH_KEY_PREFIX, updates)?;
        bump_ssh_key_revision();
        Ok(())
    }

    fn reorder_vault_entries<T: serde::de::DeserializeOwned>(
        &self,
        prefix: &str,
        updates: &[(String, i32)],
    ) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        for (id, sort_order) in updates {
            let key = entity_key(prefix, id);
            let table = txn.open_table(CREDENTIALS_TABLE)?;
            let Some(raw) = table.get(key.as_str())? else {
                continue;
            };
            // Validate the supported record before writing, but update raw JSON so
            // unknown fields and encrypted payloads survive metadata-only edits.
            let _: T = deserialize_json(raw.value())?;
            let mut entry: serde_json::Value = deserialize_json(raw.value())?;
            let object = entry.as_object_mut().ok_or_else(|| {
                StorageError::InvalidData("vault record must be a JSON object".to_string())
            })?;
            object.insert("sort_order".to_string(), (*sort_order).into());
            drop(raw);
            drop(table);
            write_json_in_txn(&txn, CREDENTIALS_TABLE, &key, &entry)?;
        }
        txn.commit()?;
        Ok(())
    }

    pub fn delete_credential(&self, credential_id: &str) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        super::cloud_sync_v4::record_remove(
            &txn,
            CREDENTIALS_TABLE,
            &entity_key(CREDENTIAL_PREFIX, credential_id),
        )?;
        txn.open_table(CREDENTIALS_TABLE)?
            .remove(entity_key(CREDENTIAL_PREFIX, credential_id).as_str())?;
        txn.commit()?;
        Ok(())
    }
}

fn apply_ssh_key_status_flags(key: &mut SshKey) {
    key.has_key_data = key.key.is_some();
    key.has_cert_data = key.cert.is_some();
}

fn read_limited_ssh_key_file(path: &str, label: &str) -> Result<String, StorageError> {
    let metadata = std::fs::metadata(path).map_err(|source| {
        StorageError::InvalidData(format!("failed to read {label} from {path}: {source}"))
    })?;
    if metadata.len() > SSH_KEY_FILE_IMPORT_MAX_BYTES {
        return Err(StorageError::InvalidData(format!(
            "{label} file is too large to import ({} bytes > {} bytes)",
            metadata.len(),
            SSH_KEY_FILE_IMPORT_MAX_BYTES
        )));
    }

    std::fs::read_to_string(path).map_err(|source| {
        StorageError::InvalidData(format!("failed to read {label} from {path}: {source}"))
    })
}

#[cfg(test)]
mod tests {
    use nyaterm_core::models::credentials::{SavedPassword, SshKey};
    use nyaterm_core::portable_snapshot::PortableSnapshotKind;
    use nyaterm_core::test_support::TestTempDir;
    use serde_json::{Value, json};

    use crate::storage::{
        CREDENTIAL_PREFIX, CREDENTIALS_TABLE, ConnectionStore, PASSWORD_PREFIX, SSH_KEY_PREFIX,
        StorageError, entity_key, write_json_in_txn,
    };

    fn raw_entry(store: &ConnectionStore, prefix: &str, id: &str) -> Value {
        store
            .read_json_table(CREDENTIALS_TABLE, &entity_key(prefix, id))
            .unwrap()
            .unwrap()
    }

    fn seed_raw(store: &ConnectionStore, prefix: &str, id: &str, raw: &Value) {
        let txn = store.db.begin_write().unwrap();
        write_json_in_txn(&txn, CREDENTIALS_TABLE, &entity_key(prefix, id), raw).unwrap();
        txn.commit().unwrap();
    }

    fn seed_encrypted_accounts_and_keys(store: &ConnectionStore) {
        for (id, name, order) in [("a", "Zulu", 4), ("b", "Alpha", 8)] {
            let account: SavedPassword = serde_json::from_value(json!({
                "id":id, "name":name, "username":"user", "password":"synthetic-password", "sort_order":order
            })).unwrap();
            store.save_password(account).unwrap();
            let key: SshKey = serde_json::from_value(json!({
                "id":id, "name":name, "key":"synthetic-key", "cert":"synthetic-cert",
                "passphrase":"synthetic-passphrase", "sort_order":order
            }))
            .unwrap();
            store.save_ssh_key(key).unwrap();
        }
    }

    fn assert_reordered_catalog(store: &ConnectionStore) {
        let accounts = store.list_passwords().unwrap();
        let keys = store.list_ssh_keys().unwrap();
        assert_eq!(
            accounts
                .iter()
                .map(|entry| (entry.id.as_str(), entry.sort_order))
                .collect::<Vec<_>>(),
            vec![("b", 0), ("a", 1)]
        );
        assert_eq!(
            keys.iter()
                .map(|entry| (entry.id.as_str(), entry.sort_order))
                .collect::<Vec<_>>(),
            vec![("b", 0), ("a", 1)]
        );
        assert!(
            accounts
                .iter()
                .all(|entry| entry.password.is_none() && entry.has_password)
        );
        let decrypted = store.load_decrypted_password_by_id("a").unwrap().unwrap();
        assert_eq!(decrypted.password.as_deref(), Some("synthetic-password"));
        let decrypted = store.load_decrypted_ssh_key_by_id("a").unwrap().unwrap();
        assert_eq!(decrypted.key_data.as_deref(), Some("synthetic-key"));
        assert_eq!(decrypted.cert_data.as_deref(), Some("synthetic-cert"));
        assert_eq!(
            decrypted.passphrase.as_deref(),
            Some("synthetic-passphrase")
        );
    }

    #[test]
    fn accounts_and_keys_append_and_edits_preserve_order_and_encrypted_secrets() {
        let dir = TestTempDir::new("nyaterm-vault-append-order");
        let store = ConnectionStore::open(&dir).unwrap();
        seed_encrypted_accounts_and_keys(&store);
        let account: SavedPassword =
            serde_json::from_value(json!({"id":"", "name":"New", "username":"user"})).unwrap();
        let account_id = store.save_password(account).unwrap();
        let key: SshKey = serde_json::from_value(json!({"id":"", "name":"New"})).unwrap();
        let key_id = store.save_ssh_key(key).unwrap();
        assert_eq!(store.list_passwords().unwrap()[2].id, account_id);
        assert_eq!(store.list_ssh_keys().unwrap()[2].id, key_id);
        assert_eq!(store.list_passwords().unwrap()[2].sort_order, 9);
        assert_eq!(store.list_ssh_keys().unwrap()[2].sort_order, 9);

        let password_ciphertext = raw_entry(&store, PASSWORD_PREFIX, "a")["password"].clone();
        let key_before = raw_entry(&store, SSH_KEY_PREFIX, "a");
        let account: SavedPassword =
            serde_json::from_value(json!({"id":"a", "name":"Renamed", "sort_order":99})).unwrap();
        let key: SshKey =
            serde_json::from_value(json!({"id":"a", "name":"Renamed", "sort_order":99})).unwrap();
        store.save_password(account).unwrap();
        store.save_ssh_key(key).unwrap();
        assert_eq!(
            store.load_password_by_id("a").unwrap().unwrap().sort_order,
            4
        );
        assert_eq!(
            store.load_ssh_key_by_id("a").unwrap().unwrap().sort_order,
            4
        );
        assert_eq!(
            raw_entry(&store, PASSWORD_PREFIX, "a")["password"],
            password_ciphertext
        );
        let key_after = raw_entry(&store, SSH_KEY_PREFIX, "a");
        for field in ["key", "cert", "passphrase"] {
            assert_eq!(key_after[field], key_before[field]);
        }
    }

    #[test]
    fn vault_reorder_preserves_unknown_fields_and_locked_payloads_and_rolls_back_corruption() {
        let dir = TestTempDir::new("nyaterm-vault-reorder-raw");
        let store = ConnectionStore::open(&dir).unwrap();
        type Reorder = fn(&ConnectionStore, &[(String, i32)]) -> Result<(), StorageError>;
        for (prefix, reorder) in [
            (
                PASSWORD_PREFIX,
                ConnectionStore::reorder_passwords as Reorder,
            ),
            (SSH_KEY_PREFIX, ConnectionStore::reorder_ssh_keys as Reorder),
            (
                CREDENTIAL_PREFIX,
                ConnectionStore::reorder_credentials as Reorder,
            ),
        ] {
            // Intentionally unsupported encrypted payloads: metadata reordering must
            // work without needing any decryption or a master-key token.
            let before = json!({
                "id":"a", "name":"Account", "username":"user", "sort_order":7,
                "password":"unsupported-encrypted-payload", "key":"unsupported-encrypted-key",
                "cert":"unsupported-encrypted-cert", "passphrase":"unsupported-encrypted-passphrase",
                "future_option":{"keep":true}
            });
            seed_raw(&store, prefix, "a", &before);
            reorder(&store, &[("missing".into(), 0), ("a".into(), 1)]).unwrap();
            let mut expected = before.clone();
            expected["sort_order"] = json!(1);
            assert_eq!(raw_entry(&store, prefix, "a"), expected);
            seed_raw(&store, prefix, "broken", &json!({"id":"broken", "name":17}));
            assert!(reorder(&store, &[("a".into(), 2), ("broken".into(), 3)]).is_err());
            assert_eq!(raw_entry(&store, prefix, "a"), expected);
            seed_raw(
                &store,
                prefix,
                "broken",
                &json!(["broken", 0, "Broken", "user"]),
            );
            assert!(reorder(&store, &[("a".into(), 2), ("broken".into(), 3)]).is_err());
            assert_eq!(raw_entry(&store, prefix, "a"), expected);
        }
    }

    #[test]
    fn legacy_vault_records_keep_name_order_until_reordered() {
        let dir = TestTempDir::new("nyaterm-vault-legacy-order");
        let store = ConnectionStore::open(&dir).unwrap();
        for prefix in [PASSWORD_PREFIX, SSH_KEY_PREFIX] {
            for (id, name) in [("a", "Zulu"), ("c", "Alpha"), ("b", "Alpha")] {
                seed_raw(&store, prefix, id, &json!({"id":id,"name":name}));
            }
        }
        assert_eq!(
            store
                .list_passwords()
                .unwrap()
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            vec!["b", "c", "a"]
        );
        assert_eq!(
            store
                .list_ssh_keys()
                .unwrap()
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            vec!["b", "c", "a"]
        );
        assert!(
            store
                .list_passwords()
                .unwrap()
                .iter()
                .all(|entry| entry.sort_order == 0)
        );
        assert!(
            store
                .list_ssh_keys()
                .unwrap()
                .iter()
                .all(|entry| entry.sort_order == 0)
        );
    }

    #[test]
    fn account_and_key_order_survives_reopen_sync_snapshots_and_nya_backup() {
        let source_dir = TestTempDir::new("nyaterm-vault-order-source");
        let store = ConnectionStore::open(&source_dir).unwrap();
        seed_encrypted_accounts_and_keys(&store);
        let updates = [("b".into(), 0), ("a".into(), 1)];
        store.reorder_passwords(&updates).unwrap();
        let revision = store.ssh_key_revision();
        store.reorder_ssh_keys(&updates).unwrap();
        assert!(store.ssh_key_revision() > revision);
        drop(store);
        let store = ConnectionStore::open(&source_dir).unwrap();
        assert_reordered_catalog(&store);
        for kind in [PortableSnapshotKind::Backup, PortableSnapshotKind::Sync] {
            let dir = TestTempDir::new("nyaterm-vault-order-snapshot");
            let target = ConnectionStore::open(&dir).unwrap();
            let mut snapshot = store
                .build_raw_portable_snapshot(kind, "device", "test")
                .unwrap();
            snapshot.recalculate_hash().unwrap();
            target.apply_raw_portable_snapshot(&snapshot).unwrap();
            assert_reordered_catalog(&target);
        }
        drop(store);
        let output_dir = TestTempDir::new("nyaterm-vault-order-backup");
        std::fs::create_dir_all(&output_dir).unwrap();
        let backup = output_dir.join("vault.nya");
        let target_dir = TestTempDir::new("nyaterm-vault-order-restored");
        ConnectionStore::export_config_database(&source_dir, None, &backup).unwrap();
        ConnectionStore::import_config_database(&target_dir, None, &backup).unwrap();
        let target = ConnectionStore::open(&target_dir).unwrap();
        assert_reordered_catalog(&target);
    }
    #[test]
    fn explicit_account_password_edits_preserve_whitespace_extensions_and_order() {
        use nyaterm_core::models::credentials::PasswordUpdate;
        let dir = TestTempDir::new("nyaterm-account-password-edits");
        let store = ConnectionStore::open(&dir).unwrap();
        seed_encrypted_accounts_and_keys(&store);
        let mut raw = raw_entry(&store, PASSWORD_PREFIX, "a");
        raw["future_extension"] = json!({"version": 42});
        seed_raw(&store, PASSWORD_PREFIX, "a", &raw);
        let mut metadata = store.list_passwords().unwrap().remove(0);
        metadata.name = "Renamed".into();
        metadata.sort_order = 99;
        store
            .save_password_with_update(metadata.clone(), PasswordUpdate::Keep)
            .unwrap();
        let kept = raw_entry(&store, PASSWORD_PREFIX, "a");
        assert_eq!(kept["password"], raw["password"]);
        assert_eq!(kept["sort_order"], 4);
        assert_eq!(kept["future_extension"], raw["future_extension"]);
        store
            .save_password_with_update(
                metadata.clone(),
                PasswordUpdate::Replace("  replacement\t".into()),
            )
            .unwrap();
        assert_eq!(
            store
                .load_decrypted_password_by_id("a")
                .unwrap()
                .unwrap()
                .password
                .as_deref(),
            Some("  replacement\t")
        );
        assert_ne!(
            raw_entry(&store, PASSWORD_PREFIX, "a")["password"],
            "  replacement\t"
        );
        store
            .save_password_with_update(metadata, PasswordUpdate::Remove)
            .unwrap();
        let removed = store.load_password_by_id("a").unwrap().unwrap();
        assert!(!removed.has_password);
        assert!(removed.password.is_none());
        assert_eq!(
            raw_entry(&store, PASSWORD_PREFIX, "a")["future_extension"],
            raw["future_extension"]
        );
        assert_eq!(removed.sort_order, 4);
    }

    #[test]
    fn invalid_account_password_edits_leave_the_original_record_unchanged() {
        use nyaterm_core::models::credentials::PasswordUpdate;
        let dir = TestTempDir::new("nyaterm-account-password-rollback");
        let store = ConnectionStore::open(&dir).unwrap();
        seed_encrypted_accounts_and_keys(&store);
        let before = raw_entry(&store, PASSWORD_PREFIX, "a");
        let mut metadata = store.list_passwords().unwrap().remove(0);
        metadata.username.clear();
        metadata.name = "Must not be written".into();
        assert!(
            store
                .save_password_with_update(metadata.clone(), PasswordUpdate::Remove)
                .is_err()
        );
        assert_eq!(raw_entry(&store, PASSWORD_PREFIX, "a"), before);
        assert!(
            store
                .save_password_with_update(
                    metadata.clone(),
                    PasswordUpdate::Replace(Default::default())
                )
                .is_err()
        );
        assert_eq!(raw_entry(&store, PASSWORD_PREFIX, "a"), before);
        store.delete_password("a").unwrap();
        assert!(
            store
                .save_password_with_update(metadata, PasswordUpdate::Keep)
                .is_err()
        );
        assert!(store.load_password_by_id("a").unwrap().is_none());
    }

    #[test]
    fn metadata_edits_keep_unreadable_passwords_and_legacy_empty_updates_keep_the_secret() {
        use nyaterm_core::models::credentials::PasswordUpdate;
        let dir = TestTempDir::new("nyaterm-account-password-compat");
        let store = ConnectionStore::open(&dir).unwrap();
        seed_encrypted_accounts_and_keys(&store);
        let mut raw = raw_entry(&store, PASSWORD_PREFIX, "a");
        raw["password"] = json!("enc:v1:deliberately-corrupted-fixture");
        seed_raw(&store, PASSWORD_PREFIX, "a", &raw);
        let mut metadata = store.list_passwords().unwrap().remove(0);
        metadata.name = "Metadata only".into();
        store
            .save_password_with_update(metadata.clone(), PasswordUpdate::Keep)
            .unwrap();
        assert_eq!(
            raw_entry(&store, PASSWORD_PREFIX, "a")["password"],
            raw["password"]
        );
        store.save_password(metadata.clone()).unwrap();
        assert_eq!(
            raw_entry(&store, PASSWORD_PREFIX, "a")["password"],
            raw["password"]
        );
        store
            .save_password_with_update(metadata, PasswordUpdate::Replace("valid-fixture".into()))
            .unwrap();
        assert_eq!(
            store
                .load_decrypted_password_by_id("a")
                .unwrap()
                .unwrap()
                .password
                .as_deref(),
            Some("valid-fixture")
        );
    }
}
