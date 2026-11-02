use serde::{Deserialize, Serialize};

use crate::SecretString;

use super::{
    default_auth_mode, default_otp_algorithm, default_otp_digits, default_otp_period,
    default_otp_type, default_true, uuid_v4,
};

/// An explicit edit operation, never part of the persisted account format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PasswordUpdate {
    Keep,
    Replace(SecretString),
    Remove,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionPasswordSource {
    Account,
    Connection,
}

/// Whether an SSH or Telnet connection takes its username from the linked account.
///
/// Absent on older records, which keep the account username when it is non-empty.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionUsernameSource {
    Account,
    Connection,
}

#[derive(Debug, Clone, Copy, thiserror::Error, PartialEq, Eq)]
pub enum AccountAuthError {
    #[error("saved account was not found")]
    MissingAccount,
    #[error("saved account password is empty or locked")]
    EmptyOrLockedAccountPassword,
    #[error("saved connection password is locked or could not be decrypted")]
    LockedConnectionPassword,
}

#[derive(Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ConnectionAuth {
    #[serde(default = "default_auth_mode")]
    pub mode: String,
    #[serde(default)]
    pub password_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_source: Option<ConnectionPasswordSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username_source: Option<ConnectionUsernameSource>,
    #[serde(default)]
    pub password: Option<SecretString>,
    #[serde(default)]
    pub key_id: Option<String>,
    #[serde(default)]
    pub otp_id: Option<String>,
    #[serde(default)]
    pub auto_fill_otp: bool,
    #[serde(default)]
    pub has_password: bool,
}

impl ConnectionAuth {
    /// New account references take precedence over legacy password-only references.
    pub fn saved_account_id(&self) -> Option<&str> {
        self.account_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .or_else(|| {
                self.password_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
            })
    }

    /// Older records omit the field and still prefer a non-empty account username.
    pub fn follows_account_username(&self) -> bool {
        !matches!(
            self.username_source,
            Some(ConnectionUsernameSource::Connection)
        )
    }

    pub fn uses_account_password(&self) -> bool {
        match self.password_source {
            Some(ConnectionPasswordSource::Account) => true,
            Some(ConnectionPasswordSource::Connection) => false,
            None => self
                .password
                .as_deref()
                .is_none_or(|password| password.trim().is_empty()),
        }
    }

    pub fn resolve_account_auth(
        &self,
        connection_username: &str,
        account: Option<&DecryptedSavedPassword>,
    ) -> Result<(String, Option<SecretString>), AccountAuthError> {
        if let Some(id) = self.saved_account_id()
            && account.is_none_or(|entry| entry.id != id)
        {
            return Err(AccountAuthError::MissingAccount);
        }
        let username = if self.follows_account_username() {
            account
                .map(|entry| entry.username.trim())
                .filter(|username| !username.is_empty())
                .unwrap_or(connection_username)
        } else {
            connection_username
        }
        .to_string();
        if self.mode == "none" {
            return Ok((username, None));
        }
        if self.mode == "password" && self.uses_account_password() {
            if let Some(account) = account {
                let password = account
                    .password
                    .clone()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or(AccountAuthError::EmptyOrLockedAccountPassword)?;
                return Ok((username, Some(password)));
            }
            if self.password_source == Some(ConnectionPasswordSource::Account) {
                return Err(AccountAuthError::MissingAccount);
            }
        }
        let password = self.password.clone().filter(|value| !value.is_empty());
        if password.is_some() && self.has_password {
            return Err(AccountAuthError::LockedConnectionPassword);
        }
        Ok((username, password))
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SshKey {
    #[serde(default = "uuid_v4")]
    pub id: String,
    #[serde(default)]
    pub sort_order: i32,
    pub name: String,
    #[serde(default)]
    pub key: Option<SecretString>,
    #[serde(default)]
    pub cert: Option<SecretString>,
    #[serde(default)]
    pub passphrase: Option<SecretString>,
    #[serde(default, skip_serializing)]
    pub key_file_path: Option<String>,
    #[serde(default, skip_serializing)]
    pub cert_file_path: Option<String>,
    #[serde(default, skip_serializing)]
    pub has_key_data: bool,
    #[serde(default, skip_serializing)]
    pub has_cert_data: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct DecryptedSshKey {
    pub id: String,
    pub name: String,
    pub key_data: Option<SecretString>,
    pub cert_data: Option<SecretString>,
    pub passphrase: Option<SecretString>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedPassword {
    #[serde(default = "uuid_v4")]
    pub id: String,
    #[serde(default)]
    pub sort_order: i32,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub username: String,
    #[serde(default)]
    pub password: Option<SecretString>,
    #[serde(default, skip_serializing)]
    pub has_password: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct DecryptedSavedPassword {
    pub id: String,
    pub name: String,
    pub username: String,
    pub password: Option<SecretString>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedCredential {
    #[serde(default = "uuid_v4")]
    pub id: String,
    #[serde(default)]
    pub sort_order: i32,
    pub name: String,
    pub username: String,
    #[serde(default)]
    pub password: Option<SecretString>,
    #[serde(default)]
    pub username_prompt_regex: Option<String>,
    #[serde(default)]
    pub password_prompt_regex: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing)]
    pub has_password: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct DecryptedSavedCredential {
    pub id: String,
    pub sort_order: i32,
    pub name: String,
    pub username: String,
    pub password: Option<SecretString>,
    pub username_prompt_regex: Option<String>,
    pub password_prompt_regex: Option<String>,
    pub enabled: bool,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OtpEntry {
    #[serde(default = "uuid_v4")]
    pub id: String,
    #[serde(default = "default_otp_type")]
    pub otp_type: String,
    #[serde(default)]
    pub issuer: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub secret: Option<SecretString>,
    #[serde(default = "default_otp_algorithm")]
    pub algorithm: String,
    #[serde(default = "default_otp_digits")]
    pub digits: u8,
    #[serde(default = "default_otp_period")]
    pub period: u64,
    #[serde(default)]
    pub counter: u64,
    #[serde(default, skip_serializing)]
    pub has_secret: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct DecryptedOtpEntry {
    pub id: String,
    pub otp_type: String,
    pub issuer: String,
    pub username: String,
    pub secret: Option<SecretString>,
    pub algorithm: String,
    pub digits: u8,
    pub period: u64,
    pub counter: u64,
}

macro_rules! impl_redacted_debug {
    ($type:ident, safe { $($safe:ident),* $(,)? }, secret { $($secret:ident),* $(,)? }) => {
        impl std::fmt::Debug for $type {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let mut debug = formatter.debug_struct(stringify!($type));
                $(debug.field(stringify!($safe), &self.$safe);)*
                $(debug.field(
                    stringify!($secret),
                    &self.$secret.as_ref().map(|_| "<redacted>"),
                );)*
                debug.finish()
            }
        }
    };
}

impl_redacted_debug!(
    ConnectionAuth,
    safe {
        mode,
        password_id,
        account_id,
        password_source,
        username_source,
        key_id,
        otp_id,
        auto_fill_otp,
        has_password
    },
    secret { password }
);
impl_redacted_debug!(
    SshKey,
    safe {
        id,
        name,
        key_file_path,
        cert_file_path,
        has_key_data,
        has_cert_data
    },
    secret {
        key,
        cert,
        passphrase
    }
);
impl_redacted_debug!(
    DecryptedSshKey,
    safe { id, name },
    secret {
        key_data,
        cert_data,
        passphrase
    }
);
impl_redacted_debug!(
    SavedPassword,
    safe {
        id,
        name,
        username,
        has_password
    },
    secret { password }
);
impl_redacted_debug!(
    DecryptedSavedPassword,
    safe { id, name, username },
    secret { password }
);
impl_redacted_debug!(
    SavedCredential,
    safe {
        id,
        sort_order,
        name,
        username,
        username_prompt_regex,
        password_prompt_regex,
        enabled,
        has_password
    },
    secret { password }
);
impl_redacted_debug!(
    DecryptedSavedCredential,
    safe {
        id,
        sort_order,
        name,
        username,
        username_prompt_regex,
        password_prompt_regex,
        enabled
    },
    secret { password }
);
impl_redacted_debug!(
    OtpEntry,
    safe {
        id,
        otp_type,
        issuer,
        username,
        algorithm,
        digits,
        period,
        counter,
        has_secret
    },
    secret { secret }
);
impl_redacted_debug!(
    DecryptedOtpEntry,
    safe {
        id,
        otp_type,
        issuer,
        username,
        algorithm,
        digits,
        period,
        counter
    },
    secret { secret }
);

#[cfg(test)]
mod connection_password_tests {
    use super::{ConnectionAuth, ConnectionPasswordSource};

    #[test]
    fn independent_password_resolution_preserves_spaces() {
        for password in [" secret ", "   "] {
            let auth = ConnectionAuth {
                mode: "password".into(),
                password_source: Some(ConnectionPasswordSource::Connection),
                password: Some(password.into()),
                ..ConnectionAuth::default()
            };
            let (_, resolved) = auth.resolve_account_auth("user", None).unwrap();
            assert_eq!(resolved.as_deref(), Some(password));
        }
    }
}
