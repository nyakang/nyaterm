//! X11 configuration and ephemeral connection contracts shared by desktop adapters and transport.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use zeroize::Zeroizing;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum X11Mode {
    #[default]
    Auto,
    Managed,
    External,
    #[serde(untagged)]
    Unknown(String),
}

impl X11Mode {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Auto => "auto",
            Self::Managed => "managed",
            Self::External => "external",
            Self::Unknown(value) => value,
        }
    }
    pub fn parse(value: &str) -> Self {
        match value {
            "auto" => Self::Auto,
            "managed" => Self::Managed,
            "external" => Self::External,
            value => Self::Unknown(value.into()),
        }
    }
    pub fn uses_managed(&self, windows_x64: bool, display: &str) -> bool {
        windows_x64
            && (matches!(self, Self::Managed)
                || matches!(self, Self::Auto) && display.trim().is_empty())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum X11Endpoint {
    Tcp { host: String, port: u16 },
    UnixSocket { path: PathBuf },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum X11PreparationChoice {
    Install,
    ContinueWithout,
    Cancel,
}

/// Implementations keep child processes and component directories alive. Dropping
/// the final owner schedules shutdown; it must never block a UI thread.
pub trait X11Lifetime: Send + Sync {
    fn is_alive(&self) -> bool;
    fn shutdown(&self);
    /// Blocking process-tree and pipe-reader barrier; call only off the UI thread.
    fn wait_stopped(&self) {}
}

#[derive(Clone)]
pub struct X11Connection {
    pub endpoint: X11Endpoint,
    pub cookie: Option<Zeroizing<Vec<u8>>>,
    pub lifetime: Option<Arc<dyn X11Lifetime>>,
}

impl std::fmt::Debug for X11Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("X11Connection").finish_non_exhaustive()
    }
}

/// One authenticated forwarding channel. The opaque guard releases only this
/// channel's reservation; it cannot stop the application-owned server.
pub struct X11ChannelConnection {
    pub connection: X11Connection,
    pub guard: Option<Box<dyn Send + Sync>>,
}

/// Invoked only after the SSH session's fake cookie has been authenticated.
/// Dropping the returned future withdraws this request, not other callers.
pub trait X11ServerProvider: Send + Sync {
    fn acquire(
        &self,
    ) -> futures::future::BoxFuture<'static, Result<X11ChannelConnection, X11Error>>;
}

#[derive(Clone, Copy, Debug, thiserror::Error, PartialEq, Eq)]
pub enum X11Error {
    #[error("X11 preparation cancelled")]
    Cancelled,
    #[error("Timed out waiting for the X11 server")]
    Timeout,
    #[error("No local X11 display is available")]
    NoDisplay,
    #[error("XQuartz is not installed or did not become ready")]
    XQuartzUnavailable,
    #[error("The X11 runtime is unsupported on this platform")]
    Unsupported,
    #[error("Could not authenticate to the local X11 server")]
    Authentication,
    #[error("Could not start the X11 component")]
    Startup,
    #[error("The X11 component protocol is incompatible")]
    Protocol,
}

#[cfg(test)]
mod tests {
    use crate::x11::X11Mode;
    #[test]
    fn auto_preserves_explicit_external_display() {
        assert!(!X11Mode::Auto.uses_managed(true, "localhost:1"));
        assert!(X11Mode::Auto.uses_managed(true, ""));
        assert!(!X11Mode::Auto.uses_managed(false, ""));
        assert!(X11Mode::Managed.uses_managed(true, "localhost:1"));
    }
    #[test]
    fn future_mode_roundtrips_without_becoming_auto() {
        let mode: X11Mode = serde_json::from_str("\"future\"").unwrap();
        assert_eq!(serde_json::to_string(&mode).unwrap(), "\"future\"");
        assert!(!mode.uses_managed(true, ""));
    }
}
