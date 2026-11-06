use nyaterm_core::{AiExecutionProfile, ConnectionType, SavedConnection};
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
    pub(crate) fn matches_saved_protocol(&self, config: &ConnectionType) -> bool {
        matches!(
            (self, config),
            (Self::Local(_), ConnectionType::LocalTerminal { .. })
                | (Self::Ssh(_), ConnectionType::Ssh { .. })
                | (Self::Telnet(_), ConnectionType::Telnet { .. })
                | (Self::Serial(_), ConnectionType::Serial { .. })
                | (Self::Rdp(_), ConnectionType::Rdp { .. })
                | (Self::Vnc(_), ConnectionType::Vnc { .. })
        )
    }

    pub(crate) fn same_saved_remote_endpoint(&self, connection: &SavedConnection) -> bool {
        // Account credentials can replace the configured username. In that case
        // do not carry a previous user's working directory into a new login.
        if connection
            .auth
            .as_ref()
            .is_some_and(|auth| auth.saved_account_id().is_some())
        {
            return false;
        }
        match (self, &connection.config) {
            (
                Self::Ssh(old),
                ConnectionType::Ssh {
                    host,
                    port,
                    username,
                    ..
                },
            ) => (&old.host, old.port, &old.username) == (host, *port, username),
            (
                Self::Telnet(old),
                ConnectionType::Telnet {
                    host,
                    port,
                    username,
                    ..
                },
            ) => (&old.host, old.port, &old.username) == (host, *port, username),
            _ => false,
        }
    }

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

    /// The account the session actually logs in as. Saved-account auth can
    /// override the connection's configured username at connect time through
    /// `ConnectionAuth::resolve_account_auth`, so this is the effective login
    /// user. Only SSH and Telnet define one; other transports cannot surface a
    /// sudo-style account prompt in a terminal.
    pub(crate) fn login_username(&self) -> Option<&str> {
        let username = match self {
            Self::Ssh(config) => config.username.as_str(),
            Self::Telnet(config) => config.username.as_str(),
            Self::Local(_) | Self::Serial(_) | Self::Rdp(_) | Self::Vnc(_) => return None,
        };
        let username = username.trim();
        (!username.is_empty()).then_some(username)
    }
}

#[derive(Clone)]
pub(crate) enum QuickSwitchItem {
    Session {
        id: String,
        title: String,
        subtitle: String,
        active: bool,
        /// Original endpoint, separate from display text for structured search.
        endpoint: Option<(String, u16)>,
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

#[cfg(test)]
mod tests {
    use super::SessionLaunchConfig;
    use nyaterm_transport::{
        LocalSessionConfig, SerialSessionConfig, SshSessionConfig, TelnetSessionConfig,
    };

    #[test]
    fn reconnect_cwd_is_only_reused_for_the_same_remote_login() {
        let old = SessionLaunchConfig::Ssh(Box::new(SshSessionConfig {
            host: "host.example".into(),
            port: 22,
            username: "alice".into(),
            ..Default::default()
        }));
        let saved = |host, port, username| {
            serde_json::from_value(serde_json::json!({
            "id":"saved", "name":"SSH", "type":"ssh", "host":host, "port":port, "username":username
        })).unwrap()
        };
        assert!(old.same_saved_remote_endpoint(&saved("host.example", 22, "alice")));
        assert!(!old.same_saved_remote_endpoint(&saved("other.example", 22, "alice")));
        assert!(!old.same_saved_remote_endpoint(&saved("host.example", 2222, "alice")));
        assert!(!old.same_saved_remote_endpoint(&saved("host.example", 22, "bob")));
    }

    fn telnet(username: &str) -> SessionLaunchConfig {
        SessionLaunchConfig::Telnet(TelnetSessionConfig {
            username: username.to_string(),
            ..Default::default()
        })
    }

    #[test]
    fn launch_config_login_username_follows_effective_account() {
        assert_eq!(
            SessionLaunchConfig::Ssh(Box::new(SshSessionConfig {
                username: "root".to_string(),
                ..SshSessionConfig::default()
            }))
            .login_username(),
            Some("root")
        );
        assert_eq!(telnet("dev").login_username(), Some("dev"));
        assert_eq!(telnet("  ").login_username(), None);
    }

    #[test]
    fn launch_config_login_username_is_shell_login_only() {
        assert_eq!(
            SessionLaunchConfig::Local(LocalSessionConfig::default()).login_username(),
            None
        );
        assert_eq!(
            SessionLaunchConfig::Serial(SerialSessionConfig::default()).login_username(),
            None
        );
        assert_eq!(
            SessionLaunchConfig::Ssh(Box::new(SshSessionConfig {
                username: "  ".to_string(),
                ..SshSessionConfig::default()
            }))
            .login_username(),
            None
        );
    }
}
