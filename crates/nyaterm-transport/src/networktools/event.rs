//! Events and log records produced by the built-in network tools.
//!
//! The transport crate only produces these values; the desktop owns their
//! presentation, persistence and i18n. Events are sent over a bounded channel
//! and are never used to call back into GPUI.

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
        addr: SocketAddr,
    },
    Stopped {
        kind: NetworkToolKind,
    },
    TransferStarted {
        kind: NetworkToolKind,
        id: u64,
        direction: TransferDirection,
        file: String,
        total: Option<u64>,
        remote: String,
    },
    TransferProgress {
        kind: NetworkToolKind,
        id: u64,
        done: u64,
        total: Option<u64>,
    },
    TransferFinished {
        kind: NetworkToolKind,
        id: u64,
        direction: TransferDirection,
        file: String,
        remote: String,
        outcome: TransferOutcome,
    },
    Failed {
        kind: NetworkToolKind,
        message: String,
    },
}
