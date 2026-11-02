use nyaterm_core::AiExecutionProfile;
use nyaterm_transport::{SessionKind, SshSessionConfig};

use crate::app_shell::session_hub::ssh_connections::SshConnectionLease;

use super::state::{SavedConnectionStartOptions, SessionStartTabPlacement};
use crate::models::{SessionLaunchConfig, StartupCommandRequest, WorkspaceSplitDirection};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SessionStartFocusTarget {
    Terminal,
    RemoteDesktop,
}

pub(in crate::features) struct MultiplexSshStartRequest {
    pub connection_name: String,
    pub config: SshSessionConfig,
    pub source_connection_id: Option<String>,
    pub ai_execution_profile: AiExecutionProfile,
    pub options: SavedConnectionStartOptions,
    pub existing_multiplex: Option<SshConnectionLease>,
    pub existing_multiplex_key: Option<String>,
}

pub(in crate::features) struct PendingSessionStartRegistration {
    pub requested_ssh_runtime_mode: Option<nyaterm_core::models::ssh_runtime::SshRuntimeMode>,
    pub(in crate::features) connection_name: String,
    pub(in crate::features) launch_config: Option<SessionLaunchConfig>,
    pub(in crate::features) kind: SessionKind,
    pub(in crate::features) ai_execution_profile: AiExecutionProfile,
    pub(in crate::features) custom_name: Option<String>,
    pub(in crate::features) tab_color: Option<u32>,
    pub(in crate::features) locked: bool,
    pub(in crate::features) after_session_id: Option<String>,
    pub(in crate::features) insert_index: Option<usize>,
    pub(in crate::features) seed_output: Option<String>,
    pub(in crate::features) startup_command: Option<StartupCommandRequest>,
    pub(in crate::features) multiplex_key: Option<String>,
    pub(in crate::features) source_connection_id: Option<String>,
    pub(in crate::features) reconnect_session_id: Option<String>,
    pub(in crate::features) workspace_split: Option<(WorkspaceSplitDirection, String)>,
    pub(in crate::features) tab_placement: Option<SessionStartTabPlacement>,
    pub(in crate::features) status_message: String,
    pub(in crate::features) append_start_log: bool,
}

mod background;
mod retry;
mod start;
