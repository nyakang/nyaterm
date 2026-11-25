//! UI-independent extension contracts. Host services expose explicit scoped operations.
pub mod catalog;
pub mod extension;
pub mod invocation;
pub mod manifest;
pub mod native;
pub mod preferences;
pub mod template;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MANIFEST_SCHEMA: u32 = 2;
pub use nyaterm_plugin_api::abi::{API_VERSION_PARTS, API_VERSION_TEXT as API_VERSION};
pub const MAX_MANIFEST_BYTES: usize = 128 * 1024;
pub const MAX_TEXT_BYTES: usize = 256 * 1024;
pub const MAX_PARAMETERS: usize = 32;
pub const MAX_ACTIONS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    PermissionDenied,
    InvalidManifest,
    Incompatible,
    UnsafePath,
    PackageLimit,
    Conflict,
    NotFound,
    Disabled,
    QueueFull,
    Cancelled,
    Timeout,
    Budget,
    Trap,
    MemoryLimit,
    InvalidResult,
    Initialization,
    Storage,
    Io,
    Shutdown,
    NativeCatalogUnavailable,
    NativeNetwork,
}

/// Diagnostics are deliberately bounded and never include guest trap stacks or input values.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("{message}")]
pub struct PluginError {
    pub code: ErrorCode,
    pub message: String,
}

impl PluginError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        let message = message
            .into()
            .chars()
            .take(512)
            .filter(|c| !c.is_control())
            .collect();
        Self { code, message }
    }
}

pub type PluginResult<T> = Result<T, PluginError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginStatus {
    Installed,
    Enabled,
    Running,
    Incompatible,
    LoadFailed,
    Faulted,
}

/// Use the same conservative identifier grammar for plugins, actions and parameters.
pub fn validate_id(id: &str) -> PluginResult<()> {
    if id.is_empty()
        || id.len() > 96
        || !id.as_bytes()[0].is_ascii_lowercase()
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        return Err(PluginError::new(
            ErrorCode::InvalidManifest,
            "Invalid plugin, action or parameter identifier",
        ));
    }
    validate_resource_path(id)
}

/// Platform-neutral checks also reject Windows aliases on Unix package builders.
pub fn validate_resource_path(path: &str) -> PluginResult<()> {
    if path.is_empty()
        || path.len() > 240
        || path.contains('\\')
        || path.contains(':')
        || path.starts_with('/')
        || path.chars().any(|c| c.is_control())
    {
        return Err(PluginError::new(
            ErrorCode::UnsafePath,
            "Unsafe plugin resource path",
        ));
    }
    for part in path.split('/') {
        let stem = part
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with(['.', ' '])
            || part.contains(['<', '>', '"', '|', '?', '*'])
            || matches!(
                stem.as_str(),
                "CON"
                    | "PRN"
                    | "AUX"
                    | "NUL"
                    | "CONIN$"
                    | "CONOUT$"
                    | "COM¹"
                    | "COM²"
                    | "COM³"
                    | "LPT¹"
                    | "LPT²"
                    | "LPT³"
            )
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        {
            return Err(PluginError::new(
                ErrorCode::UnsafePath,
                "Unsafe plugin resource path",
            ));
        }
    }
    Ok(())
}
