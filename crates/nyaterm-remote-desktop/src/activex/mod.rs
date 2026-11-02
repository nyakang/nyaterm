//! ActiveX host contracts. No COM or protocol decoder is linked into the application.

mod session;

use std::io;

use serde::{Deserialize, Serialize};

use crate::{Packet, RdpBackend, RdpError, RdpErrorKind, RdpSessionConfig};

pub use session::NativeRdpControl;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeRdpRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale_percent: u32,
}

impl NativeRdpRect {
    pub fn validate(self) -> Result<(), RdpError> {
        if !(1..=16384).contains(&self.width)
            || !(1..=16384).contains(&self.height)
            || !(100..=500).contains(&self.scale_percent)
        {
            return Err(RdpError::new(
                RdpErrorKind::Protocol,
                "Invalid ActiveX viewport",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NativeRdpConnectionEvent {
    Connected,
    Reconnecting,
    Disconnected(NativeRdpDisconnectInfo),
    Failed(RdpError),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeRdpDisconnectInfo {
    pub basic_reason: Option<u32>,
    pub extended_reason: Option<u32>,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeRdpSessionDetails {
    /// None until probed; false means SmartSizing remains in use.
    #[serde(default)]
    pub dynamic_resize_available: Option<bool>,
    pub version: Option<String>,
    pub cipher_strength: Option<u32>,
    pub status_text: Option<String>,
    pub desktop_width: Option<u32>,
    pub desktop_height: Option<u32>,
    pub color_depth: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum HostCommand {
    Hello {
        version: u32,
        parent_pid: u32,
    },
    Prepare {
        parent: u64,
        rect: NativeRdpRect,
        config: Box<RdpSessionConfig>,
    },
    Start,
    Viewport {
        rect: NativeRdpRect,
        visible: bool,
    },
    Focus,
    Ping,
    Close,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum HostEvent {
    Hello { version: u32 },
    Prepared { child: u64 },
    Connection(NativeRdpConnectionEvent),
    Details(NativeRdpSessionDetails),
    PointerInside(bool),
    Pong,
    Closed,
}

pub fn encode<T: Serialize>(message: &T) -> io::Result<Packet> {
    crate::ipc::encode_json_control(message)
}

pub fn decode<T: serde::de::DeserializeOwned>(packet: &Packet) -> io::Result<T> {
    crate::ipc::decode_json_control(packet)
}

pub fn validate_start(config: &RdpSessionConfig, rect: NativeRdpRect) -> Result<(), RdpError> {
    if config.backend != RdpBackend::Windows {
        return Err(RdpError::new(
            RdpErrorKind::Unsupported,
            "The ActiveX host requires the Windows backend",
        ));
    }
    config.backend.validate_config(config)?;
    rect.validate()
}

#[cfg(test)]
mod tests;
