use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::secret::SecretString;

pub const MAX_SPICE_HOST_BYTES: usize = 4096;
pub const MAX_SPICE_CA_BYTES: usize = 256 * 1024;
pub const MAX_SPICE_MONITORS: usize = 16;
pub const MAX_SPICE_DIMENSION: u32 = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpiceChannel {
    Main,
    Display,
    Inputs,
    Cursor,
    Playback,
    Record,
    Smartcard,
    Usbredir,
    Port,
    Webdav,
}

impl SpiceChannel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Display => "display",
            Self::Inputs => "inputs",
            Self::Cursor => "cursor",
            Self::Playback => "playback",
            Self::Record => "record",
            Self::Smartcard => "smartcard",
            Self::Usbredir => "usbredir",
            Self::Port => "port",
            Self::Webdav => "webdav",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "main" => Some(Self::Main),
            "display" => Some(Self::Display),
            "inputs" => Some(Self::Inputs),
            "cursor" => Some(Self::Cursor),
            "playback" => Some(Self::Playback),
            "record" => Some(Self::Record),
            "smartcard" => Some(Self::Smartcard),
            "usbredir" => Some(Self::Usbredir),
            "port" => Some(Self::Port),
            "webdav" => Some(Self::Webdav),
            _ => None,
        }
    }
}

/// Transport identity is separate from the address of an optional local relay.
/// In Proxmox documents `host` itself can contain an authentication ticket.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpiceEndpoint {
    pub host: SecretString,
    pub port: Option<u16>,
    pub tls_port: Option<u16>,
    #[serde(default)]
    pub proxy: Option<SecretString>,
    #[serde(default, flatten, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl fmt::Debug for SpiceEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpiceEndpoint")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("tls_port", &self.tls_port)
            .field("proxy", &self.proxy)
            .finish()
    }
}

impl SpiceEndpoint {
    /// Tickets and proxy URL passwords belong to a transient .vv session.
    /// Saved proxy authentication uses the application's encrypted proxy profiles.
    pub fn validate_persistent_identity(&self) -> Result<(), &'static str> {
        let host = self.host.expose_secret();
        if host.contains(':')
            && host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<std::net::Ipv6Addr>()
                .is_err()
        {
            return Err("SPICE ticket hosts cannot be stored as saved connections");
        }
        if let Some(proxy) = &self.proxy {
            let proxy = url::Url::parse(proxy.expose_secret())
                .map_err(|_| "invalid saved SPICE proxy URL")?;
            if !proxy.username().is_empty() || proxy.password().is_some() {
                return Err("saved SPICE proxy authentication requires an encrypted proxy profile");
            }
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), SpiceConfigError> {
        let host = self.host.expose_secret();
        if host.is_empty()
            || host.len() > MAX_SPICE_HOST_BYTES
            || host.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return Err(SpiceConfigError::InvalidHost);
        }
        if self.port == Some(0) || self.tls_port == Some(0) {
            return Err(SpiceConfigError::InvalidPort);
        }
        if self.port.is_none() && self.tls_port.is_none() {
            return Err(SpiceConfigError::MissingPort);
        }
        if let Some(proxy) = &self.proxy {
            let url = url::Url::parse(proxy.expose_secret())
                .map_err(|_| SpiceConfigError::InvalidProxy)?;
            if url.scheme() != "http"
                || url.host_str().is_none()
                || url.port_or_known_default() == Some(0)
                || !matches!(url.path(), "" | "/")
                || url.query().is_some()
                || url.fragment().is_some()
                || proxy.chars().any(char::is_control)
            {
                return Err(SpiceConfigError::InvalidProxy);
            }
        }
        Ok(())
    }
}

/// Unknown settings survive load/save, but are never printed in diagnostics.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpiceSecuritySettings {
    /// All channels must use TLS; connection errors never enable plaintext.
    pub require_tls: bool,
    pub ca_pem: Option<String>,
    pub certificate_subject: Option<String>,
    pub tls_ciphers: Option<String>,
    /// Additional channel constraints from a virt-viewer document.
    pub secure_channels: Vec<SpiceChannel>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for SpiceSecuritySettings {
    fn default() -> Self {
        Self {
            require_tls: true,
            ca_pem: None,
            certificate_subject: None,
            tls_ciphers: None,
            secure_channels: Vec::new(),
            extra: BTreeMap::new(),
        }
    }
}

impl fmt::Debug for SpiceSecuritySettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpiceSecuritySettings")
            .field("require_tls", &self.require_tls)
            .field("has_ca", &self.ca_pem.is_some())
            .field(
                "has_certificate_subject",
                &self.certificate_subject.is_some(),
            )
            .field("has_tls_ciphers", &self.tls_ciphers.is_some())
            .field("secure_channels", &self.secure_channels)
            .finish_non_exhaustive()
    }
}

impl SpiceSecuritySettings {
    pub fn validate(&self, endpoint: &SpiceEndpoint) -> Result<(), SpiceConfigError> {
        endpoint.validate()?;
        if (self.require_tls || !self.secure_channels.is_empty()) && endpoint.tls_port.is_none() {
            return Err(SpiceConfigError::TlsPortRequired);
        }
        if self.ca_pem.as_ref().is_some_and(|ca| {
            ca.len() > MAX_SPICE_CA_BYTES || ca.contains('\0') || ca.trim().is_empty()
        }) {
            return Err(SpiceConfigError::InvalidCertificate);
        }
        if [&self.certificate_subject, &self.tls_ciphers]
            .into_iter()
            .flatten()
            .any(|s| s.len() > MAX_SPICE_HOST_BYTES || s.chars().any(char::is_control))
        {
            return Err(SpiceConfigError::InvalidCertificate);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpiceClipboardMode {
    Disabled,
    ToGuest,
    FromGuest,
    #[default]
    Bidirectional,
}

impl SpiceClipboardMode {
    pub fn can_send(self) -> bool {
        matches!(self, Self::ToGuest | Self::Bidirectional)
    }

    pub fn can_receive(self) -> bool {
        matches!(self, Self::FromGuest | Self::Bidirectional)
    }
}

/// Persisted, non-credential feature preferences. Runtime tickets are separate.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpiceSettings {
    pub security: SpiceSecuritySettings,
    pub clipboard: SpiceClipboardMode,
    pub dynamic_resolution: bool,
    pub fit_window: bool,
    pub view_only: bool,
    pub audio_playback: bool,
    pub file_transfer: bool,
    pub reconnect_attempts: u32,
    pub disabled_channels: Vec<SpiceChannel>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for SpiceSettings {
    fn default() -> Self {
        Self {
            security: SpiceSecuritySettings::default(),
            clipboard: SpiceClipboardMode::Bidirectional,
            dynamic_resolution: true,
            fit_window: true,
            view_only: false,
            audio_playback: true,
            file_transfer: true,
            reconnect_attempts: 5,
            disabled_channels: Vec::new(),
            extra: BTreeMap::new(),
        }
    }
}

impl fmt::Debug for SpiceSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpiceSettings")
            .field("security", &self.security)
            .field("clipboard", &self.clipboard)
            .field("dynamic_resolution", &self.dynamic_resolution)
            .field("view_only", &self.view_only)
            .field("audio_playback", &self.audio_playback)
            .field("file_transfer", &self.file_transfer)
            .field("disabled_channels", &self.disabled_channels)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum SpiceConfigError {
    #[error("invalid SPICE host")]
    InvalidHost,
    #[error("invalid SPICE port")]
    InvalidPort,
    #[error("SPICE requires a TCP or TLS port")]
    MissingPort,
    #[error("SPICE TLS policy requires a TLS port")]
    TlsPortRequired,
    #[error("invalid SPICE HTTP CONNECT proxy")]
    InvalidProxy,
    #[error("invalid SPICE certificate configuration")]
    InvalidCertificate,
}
