//! Cloud sync state, settings and drive credentials.
//!
//! The embedded app-settings field is authoritative, matching Tauri. Older
//! standalone settings documents remain readable and migrate on an explicit save.

use super::{
    ConnectionStore, LEGACY_TEXT_CLOUD_SYNC_STATE, SETTINGS_CLOUD_SYNC, SETTINGS_CLOUD_SYNC_STATE,
    SETTINGS_DEFAULT, SETTINGS_TABLE, StorageError, TEXT_DOCS_TABLE, decrypt_optional_secret,
    encrypt_optional_secret, merge_unknown_json, optional_secret_present, set_nested_json_value,
    write_json_in_txn,
};
use nyaterm_core::{
    AliyunDriveSyncSettings, CloudSyncSettings, CloudSyncState, CredentialCrypto,
    OAuthDriveSyncSettings, merge_masked_cloud_sync_settings,
};

impl ConnectionStore {
    pub fn load_cloud_sync_master_password(
        &self,
    ) -> Result<Option<nyaterm_core::SecretString>, StorageError> {
        let Some(token) = self.load_encrypted_master_password()? else {
            return Ok(None);
        };
        let bootstrap = CredentialCrypto::new(self.portable_key_path.clone(), None);
        Ok(Some(bootstrap.decrypt_settings_secret(&token)?.into()))
    }

    pub fn load_cloud_sync_state(&self) -> Result<CloudSyncState, StorageError> {
        let mut state = if let Some(state) =
            self.read_json_table::<CloudSyncState>(SETTINGS_TABLE, SETTINGS_CLOUD_SYNC_STATE)?
        {
            state
        } else if let Some(raw) =
            self.read_string_table(TEXT_DOCS_TABLE, LEGACY_TEXT_CLOUD_SYNC_STATE)?
        {
            serde_json::from_str(&raw)?
        } else {
            CloudSyncState::default()
        };
        if state.device_id.trim().is_empty() {
            state.device_id = CloudSyncState::default().device_id;
        }
        Ok(state)
    }
    pub fn save_cloud_sync_state(&self, state: &CloudSyncState) -> Result<(), StorageError> {
        let mut state = state.clone();
        if state.device_id.trim().is_empty() {
            state.device_id = CloudSyncState::default().device_id;
        }
        self.save_settings_doc_value(SETTINGS_CLOUD_SYNC_STATE, &serde_json::to_value(state)?)?;
        Ok(())
    }
    pub fn load_cloud_sync_settings(&self) -> Result<CloudSyncSettings, StorageError> {
        let mut settings = self
            .load_cloud_sync_settings_value()?
            .map(serde_json::from_value::<CloudSyncSettings>)
            .transpose()?
            .unwrap_or_default();
        self.decrypt_cloud_sync_settings(&mut settings)?;
        Ok(settings)
    }
    pub fn save_cloud_sync_settings(
        &self,
        next: CloudSyncSettings,
    ) -> Result<CloudSyncSettings, StorageError> {
        let current = self.load_cloud_sync_settings()?;
        let merged = merge_masked_cloud_sync_settings(&current, next);
        let encrypted = self.encrypt_cloud_sync_settings(merged.clone())?;
        let mut encrypted_value = serde_json::to_value(encrypted)?;
        if let Some(current_value) = self.load_cloud_sync_settings_value()? {
            merge_unknown_json(&current_value, &mut encrypted_value);
        }
        let mut value = self.load_settings_value()?;
        set_nested_json_value(&mut value, &["cloud_sync"], encrypted_value);
        // The legacy receipt belongs to the selected remote. Commit its
        // invalidation together with the provider change, including on restart.
        let state = if current.provider != merged.provider {
            let previous = self.load_cloud_sync_state()?;
            let reset = CloudSyncState {
                device_id: previous.device_id,
                ..Default::default()
            };
            let mut next = serde_json::to_value(reset)?;
            let mut previous = if let Some(value) = self
                .read_json_table::<serde_json::Value>(SETTINGS_TABLE, SETTINGS_CLOUD_SYNC_STATE)?
            {
                value
            } else if let Some(raw) =
                self.read_string_table(TEXT_DOCS_TABLE, LEGACY_TEXT_CLOUD_SYNC_STATE)?
            {
                serde_json::from_str::<serde_json::Value>(&raw)?
            } else {
                serde_json::Value::Null
            };
            // These known optional fields are omitted by serde when empty;
            // they must not be mistaken for unknown extension fields.
            if let Some(previous) = previous.as_object_mut() {
                previous.remove("last_synced_content_hash");
                previous.remove("last_synced_clock");
            }
            merge_unknown_json(&previous, &mut next);
            Some(next)
        } else {
            None
        };
        let txn = self.db.begin_write()?;
        write_json_in_txn(&txn, SETTINGS_TABLE, SETTINGS_DEFAULT, &value)?;
        if let Some(state) = state {
            write_json_in_txn(&txn, SETTINGS_TABLE, SETTINGS_CLOUD_SYNC_STATE, &state)?;
        }
        txn.commit()?;
        Ok(merged)
    }
    pub fn update_cloud_credentials(
        &self,
        update: &nyaterm_core::cloud_sync::credentials::CloudCredentialUpdate,
    ) -> Result<nyaterm_core::cloud_sync::credentials::CredentialUpdateResult, StorageError> {
        use nyaterm_core::cloud_sync::credentials::{CredentialUpdateResult, DriveProvider};
        let mut settings = self.load_cloud_sync_settings()?;
        let (client_id, client_secret, refresh_token, access_token) = match update.provider {
            DriveProvider::Google => {
                let s = &mut settings.google_drive;
                (
                    &s.client_id,
                    &s.client_secret,
                    &mut s.refresh_token,
                    &mut s.access_token,
                )
            }
            DriveProvider::OneDrive => {
                let s = &mut settings.onedrive;
                (
                    &s.client_id,
                    &s.client_secret,
                    &mut s.refresh_token,
                    &mut s.access_token,
                )
            }
            DriveProvider::Aliyun => {
                let s = &mut settings.aliyun_drive;
                (
                    &s.client_id,
                    &s.client_secret,
                    &mut s.refresh_token,
                    &mut s.access_token,
                )
            }
        };
        fn text(value: Option<&str>) -> Option<&str> {
            value.map(str::trim).filter(|s| !s.is_empty())
        }
        if text(client_id.as_deref()) != text(update.client_id.as_deref())
            || text(client_secret.as_deref()) != text(update.client_secret.as_deref())
            || text(refresh_token.as_deref()) != text(update.expected_refresh_token.as_deref())
            || text(access_token.as_deref()) != text(update.expected_access_token.as_deref())
        {
            return Ok(CredentialUpdateResult::Superseded);
        }
        *access_token = Some(update.access_token.clone());
        if let Some(next) = &update.refresh_token {
            *refresh_token = Some(next.clone());
        }
        self.save_cloud_sync_settings(settings)?;
        Ok(CredentialUpdateResult::Applied)
    }

    fn load_cloud_sync_settings_value(&self) -> Result<Option<serde_json::Value>, StorageError> {
        let value = self.load_settings_value()?;
        if let Some(settings) = value.get("cloud_sync") {
            return Ok(Some(settings.clone()));
        }
        self.read_json_table(SETTINGS_TABLE, SETTINGS_CLOUD_SYNC)
    }
    fn decrypt_cloud_sync_settings(
        &self,
        settings: &mut CloudSyncSettings,
    ) -> Result<(), StorageError> {
        let crypto = self.credential_crypto()?;
        let master_key_token = self.load_master_key_token()?;
        decrypt_cloud_sync_settings_secrets(settings, &crypto, master_key_token.as_deref())
    }
    fn encrypt_cloud_sync_settings(
        &self,
        mut settings: CloudSyncSettings,
    ) -> Result<CloudSyncSettings, StorageError> {
        let crypto = self.credential_crypto()?;
        let master_key_token = if cloud_sync_settings_has_secret(&settings) {
            Some(self.get_or_create_master_key_token(&crypto)?)
        } else {
            None
        };
        encrypt_cloud_sync_settings_secrets(&mut settings, &crypto, master_key_token.as_deref())?;
        Ok(settings)
    }
}

pub(super) fn decrypt_cloud_sync_settings_secrets(
    settings: &mut CloudSyncSettings,
    crypto: &CredentialCrypto,
    master_key_token: Option<&str>,
) -> Result<(), StorageError> {
    settings.webdav.password =
        decrypt_optional_secret(crypto, master_key_token, &settings.webdav.password)?;
    settings.s3.access_key_id =
        decrypt_optional_secret(crypto, master_key_token, &settings.s3.access_key_id)?;
    settings.s3.secret_access_key =
        decrypt_optional_secret(crypto, master_key_token, &settings.s3.secret_access_key)?;
    settings.s3.session_token =
        decrypt_optional_secret(crypto, master_key_token, &settings.s3.session_token)?;
    settings.gitee_snippet.access_token = decrypt_optional_secret(
        crypto,
        master_key_token,
        &settings.gitee_snippet.access_token,
    )?;
    decrypt_oauth_drive_settings(crypto, master_key_token, &mut settings.google_drive)?;
    decrypt_oauth_drive_settings(crypto, master_key_token, &mut settings.onedrive)?;
    decrypt_aliyun_drive_settings(crypto, master_key_token, &mut settings.aliyun_drive)?;
    settings.github_gist.access_token =
        decrypt_optional_secret(crypto, master_key_token, &settings.github_gist.access_token)?;
    Ok(())
}

pub(super) fn encrypt_cloud_sync_settings_secrets(
    settings: &mut CloudSyncSettings,
    crypto: &CredentialCrypto,
    master_key_token: Option<&str>,
) -> Result<(), StorageError> {
    settings.webdav.password =
        encrypt_optional_secret(crypto, master_key_token, &settings.webdav.password)?;
    settings.s3.access_key_id =
        encrypt_optional_secret(crypto, master_key_token, &settings.s3.access_key_id)?;
    settings.s3.secret_access_key =
        encrypt_optional_secret(crypto, master_key_token, &settings.s3.secret_access_key)?;
    settings.s3.session_token =
        encrypt_optional_secret(crypto, master_key_token, &settings.s3.session_token)?;
    settings.gitee_snippet.access_token = encrypt_optional_secret(
        crypto,
        master_key_token,
        &settings.gitee_snippet.access_token,
    )?;
    encrypt_oauth_drive_settings(crypto, master_key_token, &mut settings.google_drive)?;
    encrypt_oauth_drive_settings(crypto, master_key_token, &mut settings.onedrive)?;
    encrypt_aliyun_drive_settings(crypto, master_key_token, &mut settings.aliyun_drive)?;
    settings.github_gist.access_token =
        encrypt_optional_secret(crypto, master_key_token, &settings.github_gist.access_token)?;
    Ok(())
}

fn decrypt_oauth_drive_settings(
    crypto: &CredentialCrypto,
    master_key_token: Option<&str>,
    settings: &mut OAuthDriveSyncSettings,
) -> Result<(), StorageError> {
    settings.access_token =
        decrypt_optional_secret(crypto, master_key_token, &settings.access_token)?;
    settings.refresh_token =
        decrypt_optional_secret(crypto, master_key_token, &settings.refresh_token)?;
    settings.client_secret =
        decrypt_optional_secret(crypto, master_key_token, &settings.client_secret)?;
    Ok(())
}

fn encrypt_oauth_drive_settings(
    crypto: &CredentialCrypto,
    master_key_token: Option<&str>,
    settings: &mut OAuthDriveSyncSettings,
) -> Result<(), StorageError> {
    settings.access_token =
        encrypt_optional_secret(crypto, master_key_token, &settings.access_token)?;
    settings.refresh_token =
        encrypt_optional_secret(crypto, master_key_token, &settings.refresh_token)?;
    settings.client_secret =
        encrypt_optional_secret(crypto, master_key_token, &settings.client_secret)?;
    Ok(())
}

fn decrypt_aliyun_drive_settings(
    crypto: &CredentialCrypto,
    master_key_token: Option<&str>,
    settings: &mut AliyunDriveSyncSettings,
) -> Result<(), StorageError> {
    settings.access_token =
        decrypt_optional_secret(crypto, master_key_token, &settings.access_token)?;
    settings.refresh_token =
        decrypt_optional_secret(crypto, master_key_token, &settings.refresh_token)?;
    settings.client_secret =
        decrypt_optional_secret(crypto, master_key_token, &settings.client_secret)?;
    Ok(())
}

fn encrypt_aliyun_drive_settings(
    crypto: &CredentialCrypto,
    master_key_token: Option<&str>,
    settings: &mut AliyunDriveSyncSettings,
) -> Result<(), StorageError> {
    settings.access_token =
        encrypt_optional_secret(crypto, master_key_token, &settings.access_token)?;
    settings.refresh_token =
        encrypt_optional_secret(crypto, master_key_token, &settings.refresh_token)?;
    settings.client_secret =
        encrypt_optional_secret(crypto, master_key_token, &settings.client_secret)?;
    Ok(())
}

pub(super) fn cloud_sync_settings_has_secret(settings: &CloudSyncSettings) -> bool {
    optional_secret_present(&settings.webdav.password)
        || optional_secret_present(&settings.s3.access_key_id)
        || optional_secret_present(&settings.s3.secret_access_key)
        || optional_secret_present(&settings.s3.session_token)
        || optional_secret_present(&settings.gitee_snippet.access_token)
        || oauth_drive_settings_has_secret(&settings.google_drive)
        || oauth_drive_settings_has_secret(&settings.onedrive)
        || aliyun_drive_settings_has_secret(&settings.aliyun_drive)
        || optional_secret_present(&settings.github_gist.access_token)
}

fn oauth_drive_settings_has_secret(settings: &OAuthDriveSyncSettings) -> bool {
    optional_secret_present(&settings.access_token)
        || optional_secret_present(&settings.refresh_token)
        || optional_secret_present(&settings.client_secret)
}

fn aliyun_drive_settings_has_secret(settings: &AliyunDriveSyncSettings) -> bool {
    optional_secret_present(&settings.access_token)
        || optional_secret_present(&settings.refresh_token)
        || optional_secret_present(&settings.client_secret)
}
