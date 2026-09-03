//! Backend services and shared domain logic.
//!
//! Groups runtime session management, SSH services, translations, importers,
//! and common error types under one backend-oriented namespace.

pub mod ai;
pub mod backup;
pub mod capabilities;
pub mod capture;
pub mod cloud_sync;
pub mod history;
pub mod importer;
pub(crate) mod input;
pub mod mcp;
pub mod monitoring;
pub mod network;
mod output;
pub mod portable_snapshot;
mod quick_commands;
pub mod rdp;
pub(crate) mod rdp_keyboard_capture;
mod recording;
pub mod remote_desktop;
pub mod remote_exec;
mod session;
pub mod sftp;
pub mod ssh;
pub mod ssh_config;
pub(crate) mod terminal_session;
pub mod translate;
pub mod vnc;
pub mod watcher;
pub mod zmodem;

pub use cloud_sync::CloudSyncManager;
pub(crate) use output::{SessionOutputCoalescer, TerminalOutputPayload};
pub use quick_commands::{
    QuickCommandsImportResult, QuickCommandsImportSource, QuickCommandsStore,
};
pub use rdp::RdpSessionManager;
pub use recording::{
    ExistingFileBehavior, InputOrigin, InputSensitivity, RecordingContext, RecordingManager,
    RecordingMode, RecordingProfile, RecordingStatus, RotationPolicy, TerminalHistorySearchRequest,
    TerminalHistorySearchResponse,
};
pub use session::{
    CwdPresentation, DynamicTitleCapabilities, SessionCommand, SessionCommandReceiver,
    SessionCommandSender, SessionHandle, SessionInfo, SessionManager, SessionReadyHook,
    SessionType, SharedCwd,
};
pub(crate) use session::{
    SessionCwdReplacement, now_session_started_at, replace_cwd_state, session_command_channel,
    update_cwd_if_changed,
};
pub use terminal_session::local::{LocalSessionConfig, create_local_session};
pub use terminal_session::serial::{SerialConfig, create_serial_session, list_serial_ports};
pub use terminal_session::telnet::{
    TelnetAutoLoginConfig, TelnetEnterMode, TelnetSessionConfig, TelnetStartupCommand,
    create_telnet_session,
};
pub use vnc::VncSessionManager;
