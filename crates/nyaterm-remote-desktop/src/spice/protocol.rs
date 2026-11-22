use std::fmt;

use nyaterm_core::connection_route::RelayEndpoint;
use nyaterm_core::secret::SecretString;
use nyaterm_core::spice::settings::{MAX_SPICE_MONITORS, SpiceEndpoint, SpiceSettings};
use serde::{Deserialize, Serialize};

use crate::spice::display::{SpiceDisplayId, SpiceMonitor, SpiceMonitorRequest, validate_monitors};
use crate::{RemotePoint, RemotePointerButton, RemoteWheelAxis};

pub const SPICE_PROTOCOL_VERSION: u16 = 1;
pub const MAX_SPICE_INPUT_BATCH: usize = 256;
pub const MAX_SPICE_CLIPBOARD_BYTES: usize = 512 * 1024;
pub const MAX_SPICE_TRANSFER_FILES: usize = 128;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpiceCredentialLifetime {
    #[default]
    Saved,
    /// A downloaded ticket has no refresh provider and cannot be auto-restored.
    Temporary,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpiceSessionConfig {
    pub session_id: String,
    pub endpoint: SpiceEndpoint,
    pub username: SecretString,
    pub password: SecretString,
    pub settings: SpiceSettings,
    pub credential_lifetime: SpiceCredentialLifetime,
    /// Separate destinations are required when plaintext and TLS ports differ.
    /// With an HTTP CONNECT proxy, relays terminate at that proxy; the helper
    /// preserves CONNECT authentication and end-to-end SPICE TLS identity.
    pub tcp_relay: Option<RelayEndpoint>,
    pub tls_relay: Option<RelayEndpoint>,
}

impl SpiceSessionConfig {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self
            .settings
            .disabled_channels
            .contains(&nyaterm_core::spice::settings::SpiceChannel::Main)
        {
            return Err("SPICE main channel cannot be disabled");
        }
        if self.session_id.is_empty()
            || self.session_id.len() > 128
            || self.session_id.chars().any(char::is_control)
        {
            return Err("invalid SPICE session identity");
        }
        self.settings
            .security
            .validate(&self.endpoint)
            .map_err(|_| "invalid SPICE endpoint or security policy")?;
        if self.password.len() > 64 * 1024
            || self.username.len() > 4096
            || self.password.contains('\0')
            || self.username.contains('\0')
        {
            return Err("invalid SPICE authentication fields");
        }
        for relay in [&self.tcp_relay, &self.tls_relay].into_iter().flatten() {
            relay.validate()?;
        }
        Ok(())
    }

    pub fn can_reconnect(&self) -> bool {
        self.credential_lifetime == SpiceCredentialLifetime::Saved
            && self.settings.reconnect_attempts > 0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpiceMouseMode {
    #[default]
    Relative,
    Absolute,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpiceInput {
    Key {
        scan_code: u16,
        pressed: bool,
    },
    KeyLocks {
        caps_lock: bool,
        num_lock: bool,
        scroll_lock: bool,
    },
    Absolute {
        display: SpiceDisplayId,
        position: RemotePoint,
    },
    Relative {
        dx: i32,
        dy: i32,
    },
    Button {
        button: RemotePointerButton,
        pressed: bool,
    },
    Wheel {
        axis: RemoteWheelAxis,
        steps: i16,
    },
    ReleaseAll,
}

impl SpiceInput {
    pub fn validate(&self) -> bool {
        match self {
            Self::Key { scan_code, .. } => *scan_code != 0 && *scan_code <= 0x1ff,
            Self::Absolute { display, position } => {
                display.agent_id().is_some() && position.x < 16_384 && position.y < 16_384
            }
            Self::Relative { dx, dy } => dx.unsigned_abs() <= 32_767 && dy.unsigned_abs() <= 32_767,
            Self::Wheel { steps, .. } => steps.unsigned_abs() <= 120,
            _ => true,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpiceCapabilities {
    pub agent_connected: bool,
    pub clipboard: bool,
    pub monitor_config: bool,
    pub file_transfer: bool,
    pub audio_playback: bool,
    pub mouse_mode: SpiceMouseMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpiceSessionState {
    Connecting,
    Connected,
    Migrating,
    Disconnected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpiceErrorKind {
    Configuration,
    RuntimeMissing,
    Connection,
    Authentication,
    Certificate,
    TicketExpired,
    Protocol,
    ResourceLimit,
    AgentUnavailable,
    Clipboard,
    FileTransfer,
    Audio,
    HelperCrash,
    Timeout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpiceTransferState {
    Running,
    Completed,
    Cancelled,
    Failed,
}

/// One session per process; frame and cursor packets use the display channel
/// number in their existing binary identity slot. Pixels never enter JSON.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SpiceControl {
    Hello {
        version: u16,
    },
    HelloAck {
        version: u16,
    },
    Connect {
        config: Box<SpiceSessionConfig>,
    },
    Disconnect,
    Ping {
        sequence: u64,
    },
    Pong {
        sequence: u64,
    },
    Input {
        events: Vec<SpiceInput>,
    },
    Clipboard {
        text: SecretString,
    },
    Resize {
        monitors: Vec<SpiceMonitorRequest>,
    },
    SetVisible {
        visible: bool,
    },
    SetMuted {
        muted: bool,
    },
    Refresh,
    SendFiles {
        transfer_id: u64,
        paths: Vec<SecretString>,
    },
    CancelTransfer {
        transfer_id: u64,
    },
    State {
        state: SpiceSessionState,
    },
    Capabilities {
        capabilities: SpiceCapabilities,
    },
    Monitors {
        monitors: Vec<SpiceMonitor>,
    },
    SurfaceReset {
        channel: u32,
        epoch: u64,
        width: u32,
        height: u32,
    },
    SurfaceRemoved {
        channel: u32,
    },
    ClipboardOffer {
        text: SecretString,
    },
    TransferProgress {
        transfer_id: u64,
        completed: u64,
        total: u64,
        state: SpiceTransferState,
    },
    Error {
        error: SpiceErrorKind,
        fatal: bool,
    },
}

impl fmt::Debug for SpiceControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Commands, typed input, file paths, clipboard and credentials are
        // deliberately absent, even if a caller logs the whole message.
        f.debug_struct("SpiceControl")
            .field("variant", &std::mem::discriminant(self))
            .finish_non_exhaustive()
    }
}

impl SpiceControl {
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Connect { config } => config.validate(),
            Self::Input { events } => {
                if events.len() > MAX_SPICE_INPUT_BATCH || events.iter().any(|e| !e.validate()) {
                    Err("invalid SPICE input batch")
                } else {
                    Ok(())
                }
            }
            Self::Clipboard { text } | Self::ClipboardOffer { text } => {
                if text.len() > MAX_SPICE_CLIPBOARD_BYTES || text.contains('\0') {
                    Err("invalid SPICE clipboard payload")
                } else {
                    Ok(())
                }
            }
            Self::Resize { monitors } => {
                if monitors.len() > MAX_SPICE_MONITORS {
                    return Err("too many SPICE monitor requests");
                }
                let mut ids = std::collections::HashSet::new();
                for monitor in monitors {
                    monitor.validate()?;
                    if !ids.insert(monitor.id.agent_id()) {
                        return Err("duplicate SPICE monitor request");
                    }
                }
                Ok(())
            }
            Self::Monitors { monitors } => validate_monitors(monitors),
            Self::SurfaceReset {
                channel,
                width,
                height,
                ..
            } => {
                if *channel > 255 {
                    return Err("invalid SPICE channel");
                }
                crate::validate_framebuffer_dimensions(
                    *width,
                    *height,
                    crate::spice::display::SPICE_FRAMEBUFFER_LIMITS,
                )
                .map(|_| ())
                .map_err(|_| "invalid SPICE framebuffer size")
            }
            Self::SurfaceRemoved { channel } if *channel > 255 => Err("invalid SPICE channel"),
            Self::SendFiles { paths, .. } => {
                if paths.is_empty()
                    || paths.len() > MAX_SPICE_TRANSFER_FILES
                    || paths
                        .iter()
                        .any(|p| p.is_empty() || p.len() > 32_768 || p.contains('\0'))
                {
                    Err("invalid SPICE file transfer request")
                } else {
                    Ok(())
                }
            }
            Self::TransferProgress {
                completed, total, ..
            } if completed > total => Err("invalid SPICE transfer progress"),
            _ => Ok(()),
        }
    }
}
