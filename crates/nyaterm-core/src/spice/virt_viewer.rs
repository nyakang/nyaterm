//! Bounded parser for the GLib key-file format used by remote-viewer.
//!
//! Parsing is side-effect free: `delete-this-file` is a request for the caller,
//! never permission to delete a path from inside an imported document.

use std::collections::BTreeMap;
use std::fmt;

use thiserror::Error;

use crate::secret::SecretString;
use crate::spice::settings::{SpiceChannel, SpiceConfigError, SpiceEndpoint, SpiceSettings};

pub const MAX_VIRT_VIEWER_BYTES: usize = 1024 * 1024;
const MAX_ENTRIES: usize = 512;
const MAX_KEY_BYTES: usize = 128;

/// Deliberately not serializable: a connection file is a transient capability,
/// not a saved connection, backup record, or startup-restore entry.
#[derive(Clone, PartialEq, Eq)]
pub struct VirtViewerFile {
    pub endpoint: SpiceEndpoint,
    pub password: SecretString,
    pub username: SecretString,
    pub title: String,
    pub fullscreen: bool,
    pub delete_after_open: bool,
    pub settings: SpiceSettings,
    /// Unhandled entries, including other groups, remain available to the UI.
    /// Both keys and values are untrusted; Debug only reports their count.
    pub unhandled: BTreeMap<String, BTreeMap<String, SecretString>>,
}

impl fmt::Debug for VirtViewerFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VirtViewerFile")
            .field("endpoint", &self.endpoint)
            .field("password", &self.password)
            .field("username", &self.username)
            .field("fullscreen", &self.fullscreen)
            .field("delete_after_open", &self.delete_after_open)
            .field("settings", &self.settings)
            .field(
                "unhandled_count",
                &self.unhandled.values().map(BTreeMap::len).sum::<usize>(),
            )
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum VirtViewerError {
    #[error("virt-viewer document exceeds the size limit")]
    TooLarge,
    #[error("virt-viewer document is not UTF-8 text")]
    Encoding,
    #[error("invalid virt-viewer syntax at line {line}")]
    Syntax { line: usize },
    #[error("duplicate virt-viewer field at line {line}")]
    Duplicate { line: usize },
    #[error("virt-viewer document contains too many fields")]
    TooManyFields,
    #[error("missing virt-viewer section")]
    MissingSection,
    #[error("virt-viewer document is not a SPICE connection")]
    UnsupportedProtocol,
    #[error("missing virt-viewer field: {0}")]
    MissingField(&'static str),
    #[error("invalid virt-viewer field: {0}")]
    InvalidField(&'static str),
    #[error("{0}")]
    Config(#[from] SpiceConfigError),
}

pub fn parse_virt_viewer(bytes: &[u8]) -> Result<VirtViewerFile, VirtViewerError> {
    if bytes.len() > MAX_VIRT_VIEWER_BYTES {
        return Err(VirtViewerError::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| VirtViewerError::Encoding)?;
    let mut groups: BTreeMap<String, BTreeMap<String, SecretString>> = BTreeMap::new();
    let mut group = None;
    let mut count = 0;
    for (index, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        let line_number = index + 1;
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.contains('\0') {
            return Err(VirtViewerError::Syntax { line: line_number });
        }
        if line.starts_with('[') {
            let name = line
                .trim_end()
                .strip_prefix('[')
                .and_then(|s| s.strip_suffix(']'))
                .filter(|s| !s.is_empty() && s.len() <= MAX_KEY_BYTES && !s.contains(['[', ']']))
                .ok_or(VirtViewerError::Syntax { line: line_number })?;
            group = Some(name.to_owned());
            continue;
        }
        let name = group
            .as_ref()
            .ok_or(VirtViewerError::Syntax { line: line_number })?;
        let (key, value) = line
            .split_once('=')
            .ok_or(VirtViewerError::Syntax { line: line_number })?;
        let key = key.trim();
        if key.is_empty() || key.len() > MAX_KEY_BYTES || key.chars().any(char::is_control) {
            return Err(VirtViewerError::Syntax { line: line_number });
        }
        count += 1;
        if count > MAX_ENTRIES {
            return Err(VirtViewerError::TooManyFields);
        }
        let fields = groups.entry(name.clone()).or_default();
        if fields.contains_key(key) {
            return Err(VirtViewerError::Duplicate { line: line_number });
        }
        // Keep unknown values losslessly. Decode only fields whose semantics
        // are known; an unknown provider extension may use its own escaping.
        fields.insert(key.to_owned(), SecretString::from(value.trim_start()));
    }

    let mut fields = groups
        .remove("virt-viewer")
        .ok_or(VirtViewerError::MissingSection)?;
    if take_string(&mut fields, "type")?.as_deref() != Some("spice") {
        return Err(VirtViewerError::UnsupportedProtocol);
    }
    let host = take_string(&mut fields, "host")?.ok_or(VirtViewerError::MissingField("host"))?;
    let endpoint = SpiceEndpoint {
        extra: Default::default(),
        host: host.into(),
        port: take_port(&mut fields, "port")?,
        tls_port: take_port(&mut fields, "tls-port")?,
        proxy: take_string(&mut fields, "proxy")?
            .filter(|s| !s.is_empty())
            .map(Into::into),
    };
    let mut settings = SpiceSettings::default();
    // A TLS endpoint never falls back to the document's plaintext endpoint.
    // A plaintext-only document is itself an explicit plaintext request.
    settings.security.require_tls = endpoint.tls_port.is_some();
    settings.security.ca_pem = take_string(&mut fields, "ca")?;
    settings.security.certificate_subject = take_string(&mut fields, "host-subject")?;
    settings.security.tls_ciphers = take_string(&mut fields, "tls-ciphers")?;
    settings.security.secure_channels = take_channels(&mut fields, "secure-channels")?;
    settings.disabled_channels = take_channels(&mut fields, "disable-channels")?;
    settings.security.validate(&endpoint)?;
    // There is no credential refresh provider for a downloaded document.
    settings.reconnect_attempts = 0;
    let result = VirtViewerFile {
        endpoint,
        password: take_string(&mut fields, "password")?
            .unwrap_or_default()
            .into(),
        username: take_string(&mut fields, "username")?
            .unwrap_or_default()
            .into(),
        title: take_string(&mut fields, "title")?.unwrap_or_default(),
        fullscreen: take_bool(&mut fields, "fullscreen")?.unwrap_or(false),
        delete_after_open: take_bool(&mut fields, "delete-this-file")?.unwrap_or(false),
        settings,
        unhandled: {
            if !fields.is_empty() {
                groups.insert("virt-viewer".to_owned(), fields);
            }
            groups
        },
    };
    Ok(result)
}

fn take_string(
    fields: &mut BTreeMap<String, SecretString>,
    key: &'static str,
) -> Result<Option<String>, VirtViewerError> {
    let Some(value) = fields.remove(key) else {
        return Ok(None);
    };
    let mut chars = value.chars();
    let mut decoded = String::with_capacity(value.len());
    while let Some(c) = chars.next() {
        if c != '\\' {
            decoded.push(c);
            continue;
        }
        decoded.push(match chars.next() {
            Some('n') => '\n',
            Some('r') => '\r',
            Some('t') => '\t',
            Some('s') => ' ',
            Some('\\') => '\\',
            Some(';') => ';',
            _ => return Err(VirtViewerError::InvalidField(key)),
        });
    }
    Ok(Some(decoded))
}

fn take_port(
    fields: &mut BTreeMap<String, SecretString>,
    key: &'static str,
) -> Result<Option<u16>, VirtViewerError> {
    match take_string(fields, key)?.as_deref().map(str::trim) {
        None | Some("-1") => Ok(None),
        Some(value) => value
            .parse::<u16>()
            .ok()
            .filter(|p| *p != 0)
            .map(Some)
            .ok_or(VirtViewerError::InvalidField(key)),
    }
}

fn take_bool(
    fields: &mut BTreeMap<String, SecretString>,
    key: &'static str,
) -> Result<Option<bool>, VirtViewerError> {
    match take_string(fields, key)?.as_deref().map(str::trim) {
        None => Ok(None),
        Some("1" | "true") => Ok(Some(true)),
        Some("0" | "false") => Ok(Some(false)),
        _ => Err(VirtViewerError::InvalidField(key)),
    }
}

fn take_channels(
    fields: &mut BTreeMap<String, SecretString>,
    key: &'static str,
) -> Result<Vec<SpiceChannel>, VirtViewerError> {
    let Some(value) = take_string(fields, key)? else {
        return Ok(Vec::new());
    };
    if value.trim().is_empty() {
        return Ok(Vec::new());
    }
    if value.trim().trim_end_matches(';') == "all" && key == "secure-channels" {
        return Ok(vec![
            SpiceChannel::Main,
            SpiceChannel::Display,
            SpiceChannel::Inputs,
            SpiceChannel::Cursor,
            SpiceChannel::Playback,
            SpiceChannel::Record,
            SpiceChannel::Smartcard,
            SpiceChannel::Usbredir,
            SpiceChannel::Port,
            SpiceChannel::Webdav,
        ]);
    }
    let mut channels = Vec::new();
    for item in value.trim().trim_end_matches(';').split(';') {
        let channel = SpiceChannel::parse(item.trim()).ok_or(VirtViewerError::InvalidField(key))?;
        if !channels.contains(&channel) {
            channels.push(channel);
        }
    }
    Ok(channels)
}

#[cfg(test)]
mod tests;
