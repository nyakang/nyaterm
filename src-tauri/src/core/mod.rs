//! Backend services and shared domain logic.
//!
//! Groups runtime session management, SSH services, translations, importers,
//! and common error types under one backend-oriented namespace.

pub mod backup;
pub mod ai;
pub mod cloud_crypto;
pub mod cloud_sync;
pub mod history;
pub mod importer;
mod output;
pub mod portable_snapshot;
mod pty;
mod quick_commands;
mod recording;
pub mod serial;
mod session;
pub mod ssh;
pub mod stats;
pub mod telnet;
pub mod translate;
pub mod watcher;

pub use cloud_sync::CloudSyncManager;
pub(crate) use output::SessionOutputCoalescer;
pub use pty::{create_local_session, LocalSessionConfig};
pub use quick_commands::QuickCommandsStore;
pub use recording::RecordingManager;
pub use serial::{create_serial_session, list_serial_ports, SerialConfig};
pub(crate) use session::update_cwd_if_changed;
pub use session::{
    SessionCommand, SessionHandle, SessionInfo, SessionManager, SessionType, SharedCwd,
};
pub use telnet::create_telnet_session;
