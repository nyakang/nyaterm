//! Configuration model for the built-in network tools panel.
//!
//! These types carry no IO: the desktop reads and writes them as a settings
//! subtree and hands them to `nyaterm-transport::networktools` when a service
//! starts. Keeping the model here means the transport crate never parses user
//! settings and the store never links a network stack.

use std::fmt;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};

pub const DEFAULT_TFTP_BIND_ADDRESS: &str = "0.0.0.0";
pub const DEFAULT_TFTP_PORT: u16 = 69;
pub const DEFAULT_TFTP_BLOCK_SIZE: u16 = 512;
pub const DEFAULT_TFTP_TIMEOUT_SECS: u8 = 3;
pub const DEFAULT_TFTP_MAX_CONCURRENCY: u16 = 8;
pub const TFTP_BLOCK_SIZE_MIN: u16 = 8;
pub const TFTP_BLOCK_SIZE_MAX: u16 = 65464;
pub const TFTP_MAX_CONCURRENCY_LIMIT: u16 = 64;

/// Root of the `networktools` settings subtree.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkToolsSettings {
    #[serde(default)]
    pub tftp: TftpSettings,
}

impl NetworkToolsSettings {
    /// Validate every registered tool. Unknown or not-yet-supported subtrees are
    /// ignored so a newer build can add tools without breaking an older one.
    pub fn validate(&self) -> Result<(), NetworkToolsSettingsError> {
        self.tftp.validate()
    }
}

/// TFTP server configuration.
///
/// The default listener is read-only. Uploads and overwrites need an explicit opt-in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TftpSettings {
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Empty means "use the desktop default directory" and is resolved when the
    /// service starts, so moving the settings between machines stays neutral.
    #[serde(default)]
    pub root_dir: String,
    /// Upper limit for client-requested blksize; clients without the option use 512.
    #[serde(default = "default_block_size")]
    pub block_size: u16,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u8,
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: u16,
    #[serde(default)]
    pub allow_upload: bool,
    #[serde(default)]
    pub allow_overwrite: bool,
}

impl Default for TftpSettings {
    fn default() -> Self {
        Self {
            bind_address: default_bind_address(),
            port: default_port(),
            root_dir: String::new(),
            block_size: default_block_size(),
            timeout_secs: default_timeout_secs(),
            max_concurrency: default_max_concurrency(),
            allow_upload: false,
            allow_overwrite: false,
        }
    }
}

impl TftpSettings {
    /// A blank bind address falls back to the wildcard default instead of
    /// failing validation, which matches what the settings form shows.
    pub fn effective_bind_address(&self) -> String {
        let trimmed = self.bind_address.trim();
        if trimmed.is_empty() {
            DEFAULT_TFTP_BIND_ADDRESS.to_string()
        } else {
            trimmed.to_string()
        }
    }

    /// Resolve the address the listener will bind to.
    pub fn parsed_bind_address(&self) -> Result<IpAddr, NetworkToolsSettingsError> {
        self.effective_bind_address()
            .parse::<IpAddr>()
            .map_err(|_| NetworkToolsSettingsError::InvalidBindAddress(self.bind_address.clone()))
    }

    pub fn validate(&self) -> Result<(), NetworkToolsSettingsError> {
        self.parsed_bind_address()?;
        if self.port == 0 {
            return Err(NetworkToolsSettingsError::InvalidPort);
        }
        if !(TFTP_BLOCK_SIZE_MIN..=TFTP_BLOCK_SIZE_MAX).contains(&self.block_size) {
            return Err(NetworkToolsSettingsError::InvalidBlockSize);
        }
        if self.timeout_secs == 0 {
            return Err(NetworkToolsSettingsError::InvalidTimeout);
        }
        if self.max_concurrency == 0 || self.max_concurrency > TFTP_MAX_CONCURRENCY_LIMIT {
            return Err(NetworkToolsSettingsError::InvalidMaxConcurrency);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkToolsSettingsError {
    InvalidBindAddress(String),
    InvalidPort,
    InvalidBlockSize,
    InvalidTimeout,
    InvalidMaxConcurrency,
}

impl fmt::Display for NetworkToolsSettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBindAddress(value) => {
                write!(f, "invalid bind address: {value}")
            }
            Self::InvalidPort => f.write_str("port must be between 1 and 65535"),
            Self::InvalidBlockSize => write!(
                f,
                "block size must be between {TFTP_BLOCK_SIZE_MIN} and {TFTP_BLOCK_SIZE_MAX}"
            ),
            Self::InvalidTimeout => f.write_str("timeout must be at least 1 second"),
            Self::InvalidMaxConcurrency => write!(
                f,
                "max concurrency must be between 1 and {TFTP_MAX_CONCURRENCY_LIMIT}"
            ),
        }
    }
}

impl std::error::Error for NetworkToolsSettingsError {}

fn default_bind_address() -> String {
    DEFAULT_TFTP_BIND_ADDRESS.to_string()
}

fn default_port() -> u16 {
    DEFAULT_TFTP_PORT
}

fn default_block_size() -> u16 {
    DEFAULT_TFTP_BLOCK_SIZE
}

fn default_timeout_secs() -> u8 {
    DEFAULT_TFTP_TIMEOUT_SECS
}

fn default_max_concurrency() -> u16 {
    DEFAULT_TFTP_MAX_CONCURRENCY
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_TFTP_BLOCK_SIZE, NetworkToolsSettings, NetworkToolsSettingsError, TftpSettings,
    };

    #[test]
    fn defaults_listen_on_all_interfaces_but_stay_read_only() {
        let settings = NetworkToolsSettings::default();
        assert_eq!(settings.tftp.bind_address, "0.0.0.0");
        assert_eq!(settings.tftp.port, 69);
        assert!(!settings.tftp.allow_upload);
        assert!(!settings.tftp.allow_overwrite);
        settings.validate().expect("defaults are valid");
    }

    #[test]
    fn blank_document_loads_as_defaults() {
        let settings: NetworkToolsSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, NetworkToolsSettings::default());
    }

    #[test]
    fn unknown_keys_and_tools_are_ignored() {
        let settings: NetworkToolsSettings = serde_json::from_str(
            r#"{"ftp":{"port":2121},"tftp":{"port":6969,"future_option":true}}"#,
        )
        .unwrap();
        assert_eq!(settings.tftp.port, 6969);
        assert_eq!(settings.tftp.block_size, DEFAULT_TFTP_BLOCK_SIZE);
    }

    #[test]
    fn round_trip_keeps_explicit_values() {
        let settings = NetworkToolsSettings {
            tftp: TftpSettings {
                bind_address: "0.0.0.0".into(),
                port: 1069,
                root_dir: "/srv/tftp".into(),
                block_size: 1468,
                timeout_secs: 5,
                max_concurrency: 4,
                allow_upload: true,
                allow_overwrite: true,
            },
        };
        let encoded = serde_json::to_string(&settings).unwrap();
        let decoded: NetworkToolsSettings = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, settings);
    }

    #[test]
    fn empty_bind_address_falls_back_to_the_default_interface() {
        let settings = TftpSettings {
            bind_address: "  ".into(),
            ..TftpSettings::default()
        };
        assert_eq!(settings.effective_bind_address(), "0.0.0.0");
        settings.validate().unwrap();
    }

    #[test]
    fn invalid_values_are_rejected() {
        let mut settings = TftpSettings {
            port: 0,
            ..TftpSettings::default()
        };
        assert_eq!(
            settings.validate(),
            Err(NetworkToolsSettingsError::InvalidPort)
        );

        settings.port = 69;
        settings.block_size = 4;
        assert_eq!(
            settings.validate(),
            Err(NetworkToolsSettingsError::InvalidBlockSize)
        );

        settings.block_size = 512;
        settings.timeout_secs = 0;
        assert_eq!(
            settings.validate(),
            Err(NetworkToolsSettingsError::InvalidTimeout)
        );

        settings.timeout_secs = 3;
        settings.max_concurrency = 0;
        assert_eq!(
            settings.validate(),
            Err(NetworkToolsSettingsError::InvalidMaxConcurrency)
        );

        settings.max_concurrency = 8;
        settings.bind_address = "not-an-ip".into();
        assert!(matches!(
            settings.validate(),
            Err(NetworkToolsSettingsError::InvalidBindAddress(_))
        ));
    }
}
