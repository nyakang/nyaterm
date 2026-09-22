use nyaterm_core::{AiExecutionProfile, SavedConnection};
use nyaterm_remote_desktop::{RdpSessionConfig, VncSessionConfig};
use nyaterm_transport::{
    LocalSessionConfig, SerialSessionConfig, SshSessionConfig, TelnetSessionConfig,
};

#[derive(Clone)]
pub(crate) struct SessionRuntimeMetadata {
    pub(crate) ssh_config: Option<SshSessionConfig>,
    pub(crate) ssh_multiplex_key: Option<String>,
    pub(crate) source_connection_id: Option<String>,
    pub(crate) ai_execution_profile: AiExecutionProfile,
    pub(crate) launch_config: SessionLaunchConfig,
    /// Backend closed while the tab is kept for reconnect (Tauri disconnected pane).
    pub(crate) disconnected: bool,
}

#[derive(Clone)]
pub(crate) struct StartupCommandRequest {
    pub(crate) command: String,
    pub(crate) delay_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SyncInputGroup {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) color: u32,
    pub(crate) session_ids: Vec<String>,
    pub(crate) paused_session_ids: Vec<String>,
    pub(crate) enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartupCommandAction {
    Duplicate,
    Multiplex,
}

impl StartupCommandAction {
    pub(crate) fn status_opened(self) -> &'static str {
        match self {
            Self::Duplicate => "duplicate and run command opened",
            Self::Multiplex => "multiplex and run command opened",
        }
    }

    pub(crate) fn status_cancelled(self) -> &'static str {
        match self {
            Self::Duplicate => "duplicate and run command cancelled",
            Self::Multiplex => "multiplex and run command cancelled",
        }
    }
}

#[derive(Clone)]
pub(crate) enum SessionLaunchConfig {
    Local(LocalSessionConfig),
    Ssh(Box<SshSessionConfig>),
    Telnet(TelnetSessionConfig),
    Serial(SerialSessionConfig),
    Rdp(RdpSessionConfig),
    Vnc(VncSessionConfig),
}

impl SessionLaunchConfig {
    pub(crate) fn encoding(&self) -> Option<&str> {
        match self {
            Self::Local(config) => Some(&config.encoding),
            Self::Ssh(config) => Some(&config.encoding),
            Self::Telnet(config) => Some(&config.encoding),
            Self::Serial(config) => Some(&config.encoding),
            Self::Rdp(_) => None,
            Self::Vnc(_) => None,
        }
    }
}

#[derive(Clone)]
pub(crate) enum QuickSwitchItem {
    Session {
        id: String,
        title: String,
        subtitle: String,
        active: bool,
    },
    Connection {
        connection: Box<SavedConnection>,
        title: String,
        subtitle: String,
    },
    Pending {
        request_id: String,
        title: String,
        subtitle: String,
        active: bool,
        failed: bool,
        search_detail: Option<String>,
    },
}

impl QuickSwitchItem {
    pub(crate) fn id(&self) -> String {
        match self {
            Self::Session { id, .. } => format!("session:{id}"),
            Self::Connection { connection, .. } => format!("connection:{}", connection.id),
            Self::Pending { request_id, .. } => format!("session:{request_id}"),
        }
    }

    pub(crate) fn title(&self) -> &str {
        match self {
            Self::Session { title, .. }
            | Self::Connection { title, .. }
            | Self::Pending { title, .. } => title,
        }
    }

    pub(crate) fn subtitle(&self) -> &str {
        match self {
            Self::Session { subtitle, .. }
            | Self::Connection { subtitle, .. }
            | Self::Pending { subtitle, .. } => subtitle,
        }
    }

    pub(crate) fn search_text(&self) -> String {
        match self {
            Self::Session {
                id,
                title,
                subtitle,
                ..
            } => format!("{title} {subtitle} {id}"),
            Self::Connection {
                connection,
                title,
                subtitle,
            } => format!(
                "{} {} {} {}",
                title,
                subtitle,
                connection.description.clone().unwrap_or_default(),
                connection.endpoint()
            ),
            Self::Pending {
                request_id,
                title,
                subtitle,
                search_detail,
                ..
            } => format!(
                "{title} {subtitle} {request_id} {}",
                search_detail.as_deref().unwrap_or_default()
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TerminalSearchMode {
    Buffer,
    History,
}

impl TerminalSearchMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Buffer => "Buffer",
            Self::History => "History",
        }
    }
}

pub(crate) fn normalize_paste_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

pub(crate) fn is_multi_line_paste(text: &str) -> bool {
    normalize_paste_newlines(text).contains('\n')
}
