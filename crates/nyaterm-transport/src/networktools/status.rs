//! Runtime status and read-only transfer snapshots for the network tools.

use std::net::SocketAddr;
use std::time::Instant;

use super::event::TransferDirection;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NetworkToolState {
    #[default]
    Stopped,
    Running,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NetworkToolStatus {
    pub state: NetworkToolState,
    pub addr: Option<SocketAddr>,
    pub message: Option<String>,
}

impl NetworkToolStatus {
    pub fn running(addr: SocketAddr) -> Self {
        Self {
            state: NetworkToolState::Running,
            addr: Some(addr),
            message: None,
        }
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            state: NetworkToolState::Stopped,
            addr: None,
            message: Some(message.into()),
        }
    }

    pub fn is_running(&self) -> bool {
        self.state == NetworkToolState::Running
    }
}

/// A transfer in flight, as shown in the configuration window.
#[derive(Debug, Clone)]
pub struct TransferSnapshot {
    pub id: u64,
    pub direction: TransferDirection,
    pub file: String,
    pub remote: String,
    pub done: u64,
    pub total: Option<u64>,
    pub elapsed_secs: u64,
    pub(crate) started_at: Instant,
}

impl TransferSnapshot {
    pub fn new(
        id: u64,
        direction: TransferDirection,
        file: String,
        remote: String,
        total: Option<u64>,
    ) -> Self {
        Self {
            id,
            direction,
            file,
            remote,
            done: 0,
            total,
            elapsed_secs: 0,
            started_at: Instant::now(),
        }
    }

    pub(crate) fn advance(&mut self, done: u64) {
        self.done = done;
        self.elapsed_secs = self.started_at.elapsed().as_secs();
    }
}
