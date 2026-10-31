use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionEventConsumerId(pub(crate) u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub kind: SessionKind,
    pub working_dir: Option<PathBuf>,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    LocalPty,
    Ssh,
    Telnet,
    RawTcp,
    Serial,
    Rdp,
    Vnc,
}

// `Eq` is impossible: `TmuxState` carries `f64` layout ratios.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// In-process ordering fence; never sent to the terminal or persisted.
    OutputBoundary {
        session_id: String,
        token: String,
    },
    CleanupFailed {
        session_id: String,
        consumer_id: Option<SessionEventConsumerId>,
        operation: CleanupOperation,
    },
    Output {
        session_id: String,
        data: Vec<u8>,
    },
    OutputDropped {
        session_id: String,
        bytes: usize,
    },
    CwdChanged {
        session_id: String,
        cwd: String,
    },
    CommandAccepted {
        session_id: String,
        command: String,
    },
    Exited {
        session_id: String,
        reason: String,
    },
    Error {
        session_id: String,
        message: String,
    },
    /// tmux control-mode state for an SSH session in control mode. The
    /// payload carries the full window/pane projection the workspace renders.
    TmuxState {
        session_id: String,
        state: Box<crate::tmux::TmuxStatePayload>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupOperation {
    Terminate,
    Wait,
    ReaderJoin,
    WriterJoin,
    StartupFiles,
}

pub trait TerminalTransport: Send {
    fn write(&mut self, data: &[u8]) -> anyhow::Result<()>;

    /// Protocol bytes bypass text editing and backspace translation.
    fn write_raw(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.write(data)
    }

    fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> anyhow::Result<()>;

    fn close(&mut self) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionDrainStats {
    pub drained_events: usize,
    pub drained_output_bytes: usize,
    pub queued_events: usize,
    pub queued_output_bytes: usize,
    pub dropped_output_bytes: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionDrain {
    pub events: Vec<SessionEvent>,
    pub stats: SessionDrainStats,
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("session background worker unavailable: {0}")]
    Worker(#[source] std::io::Error),
    #[error(
        "Local Terminal requires Windows 10 version 1809 (Build 17763) or later; current build: {build}"
    )]
    UnsupportedWindowsBuild { build: u32 },
    #[error("Unable to determine Windows version before starting Local Terminal")]
    WindowsVersionUnavailable,
    #[error("session not found: {0}")]
    NotFound(String),
    #[error("failed to open PTY: {0}")]
    OpenPty(#[source] anyhow::Error),
    #[error("failed to clone PTY reader: {0}")]
    CloneReader(#[source] anyhow::Error),
    #[error("failed to take PTY writer: {0}")]
    TakeWriter(#[source] anyhow::Error),
    #[error("failed to spawn shell: {0}")]
    Spawn(#[source] anyhow::Error),
    #[error("failed to connect TCP session to {addr}: {source}")]
    ConnectTcp {
        addr: String,
        source: std::io::Error,
    },
    #[error("failed to clone TCP stream for session {session_id}: {source}")]
    CloneTcp {
        session_id: String,
        source: std::io::Error,
    },
    #[error("failed to open serial port {port_name}: {source}")]
    OpenSerial {
        port_name: String,
        source: serialport::Error,
    },
    #[error("failed to clone serial port for session {session_id}: {source}")]
    CloneSerial {
        session_id: String,
        source: serialport::Error,
    },
    #[error("failed to create SSH session for {addr}: {source}")]
    CreateSsh { addr: String, source: anyhow::Error },
    #[error("failed to write to session {session_id}: {source}")]
    Write {
        session_id: String,
        source: anyhow::Error,
    },
    #[error("failed to resize session {session_id}: {source}")]
    Resize {
        session_id: String,
        source: anyhow::Error,
    },
    #[error("session registry lock is poisoned")]
    LockPoisoned,
}

impl SessionError {
    pub fn is_backpressure(&self) -> bool {
        matches!(self, Self::Write { source, .. } if source.downcast_ref::<std::io::Error>().is_some_and(|error| error.kind() == std::io::ErrorKind::WouldBlock))
    }
}
