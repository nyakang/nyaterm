use std::time::Duration;

use nyaterm_core::SecretString;
use nyaterm_core::character_encoding::CharacterEncoding;

use crate::ftp::error::FtpError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FtpPassiveMode {
    Auto,
    Pasv,
    Epsv,
}

impl FtpPassiveMode {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "pasv" => Self::Pasv,
            "epsv" => Self::Epsv,
            _ => Self::Auto,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FtpFilenameEncodingMode {
    Auto,
    Utf8,
    Custom,
}

impl FtpFilenameEncodingMode {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "utf8" | "utf-8" => Self::Utf8,
            "custom" => Self::Custom,
            _ => Self::Auto,
        }
    }
}

/// Whether the control and data channels use FTPS.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FtpSecurityMode {
    /// Plain FTP.
    #[default]
    None,
    /// `AUTH TLS` followed by `PBSZ 0` and `PROT P`.
    Explicit,
    /// TLS is negotiated immediately after the TCP connection.
    Implicit,
}

impl FtpSecurityMode {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "explicit" => Self::Explicit,
            "implicit" => Self::Implicit,
            _ => Self::None,
        }
    }
}

/// Pinned TLS protocol version for explicit FTPS.
///
/// `Auto` leaves the native TLS backend defaults in place. The remaining
/// values set both the minimum and maximum protocol version, so an obsolete
/// protocol is either negotiated or rejected instead of silently falling back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FtpTlsVersion {
    #[default]
    Auto,
    Ssl3,
    Tls10,
    Tls11,
    Tls12,
    Tls13,
}

impl FtpTlsVersion {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "ssl3" => Self::Ssl3,
            "tls10" => Self::Tls10,
            "tls11" => Self::Tls11,
            "tls12" => Self::Tls12,
            "tls13" => Self::Tls13,
            _ => Self::Auto,
        }
    }
}

/// Immutable connection settings for one FTP session.
#[derive(Clone, Debug)]
pub struct FtpConfig {
    pub host: String,
    pub port: u16,
    pub initial_directory: String,
    pub connect_timeout: Duration,
    pub passive_mode: FtpPassiveMode,
    pub filename_encoding_mode: FtpFilenameEncodingMode,
    pub filename_encoding: String,
    pub ascii_mode: bool,
    pub security_mode: FtpSecurityMode,
    pub tls_version: FtpTlsVersion,
}

impl FtpConfig {
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            port: 21,
            initial_directory: String::new(),
            connect_timeout: Duration::from_secs(15),
            passive_mode: FtpPassiveMode::Auto,
            filename_encoding_mode: FtpFilenameEncodingMode::Auto,
            filename_encoding: String::new(),
            ascii_mode: false,
            security_mode: FtpSecurityMode::None,
            tls_version: FtpTlsVersion::Auto,
        }
    }

    pub fn resolve_filename_encoding(
        &self,
        local_encoding: CharacterEncoding,
        server_supports_utf8: bool,
    ) -> Result<CharacterEncoding, FtpError> {
        match self.filename_encoding_mode {
            FtpFilenameEncodingMode::Auto => Ok(if server_supports_utf8 {
                CharacterEncoding::Utf8
            } else {
                local_encoding
            }),
            FtpFilenameEncodingMode::Utf8 => Ok(CharacterEncoding::Utf8),
            FtpFilenameEncodingMode::Custom => CharacterEncoding::parse(&self.filename_encoding)
                .map_err(|_| FtpError::UnsupportedEncoding),
        }
    }
}

/// Credentials used for one FTP login.
#[derive(Clone, Debug)]
pub enum FtpLogin {
    Anonymous {
        username: String,
    },
    Password {
        username: String,
        password: SecretString,
    },
}

impl FtpLogin {
    pub fn anonymous(username: impl Into<String>) -> Self {
        let username = username.into();
        let username = if username.trim().is_empty() {
            "anonymous".to_string()
        } else {
            username
        };
        Self::Anonymous { username }
    }

    pub fn password(username: impl Into<String>, password: impl Into<SecretString>) -> Self {
        Self::Password {
            username: username.into(),
            password: password.into(),
        }
    }

    pub fn username(&self) -> &str {
        match self {
            Self::Anonymous { username } | Self::Password { username, .. } => username,
        }
    }
}
