//! Events and log records produced by the built-in network tools.
//!
//! The transport crate only produces these values; the desktop owns their
//! presentation, persistence and i18n. Lifecycle events use a bounded channel
//! with backpressure; progress is coalesced in transfer snapshots instead.
//! Producers never call back into GPUI.

use std::net::SocketAddr;

use serde::Serialize;

use super::registry::NetworkToolKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferDirection {
    /// The client reads a file from us (RRQ).
    Download,
    /// The client writes a file to us (WRQ).
    Upload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferOutcome {
    Completed { bytes: u64 },
    Failed { message: String },
}

#[derive(Debug, Clone)]
pub enum NetworkToolEvent {
    Started {
        kind: NetworkToolKind,
        generation: u64,
        addr: SocketAddr,
    },
    Stopped {
        kind: NetworkToolKind,
        generation: u64,
    },
    TransferStarted {
        kind: NetworkToolKind,
        generation: u64,
        id: u64,
        direction: TransferDirection,
        file: String,
        total: Option<u64>,
        remote: String,
    },
    TransferFinished {
        kind: NetworkToolKind,
        generation: u64,
        id: u64,
        direction: TransferDirection,
        file: String,
        remote: String,
        outcome: TransferOutcome,
    },
    Failed {
        kind: NetworkToolKind,
        generation: u64,
        message: String,
    },
}

impl NetworkToolEvent {
    pub fn service(&self) -> (NetworkToolKind, u64) {
        match self {
            Self::Started {
                kind, generation, ..
            }
            | Self::Stopped { kind, generation }
            | Self::TransferStarted {
                kind, generation, ..
            }
            | Self::TransferFinished {
                kind, generation, ..
            }
            | Self::Failed {
                kind, generation, ..
            } => (*kind, *generation),
        }
    }
}
