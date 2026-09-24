use futures::channel::mpsc::UnboundedReceiver;
use gpui::Pixels;
use nyaterm_core::{
    CredentialPromptKind, SavedCredential, compile_prompt_regex, get_credential_prompt_pattern,
};
use rust_i18n::t;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use super::event_wake::{ANY_INTEREST, EventWake};

const CREDENTIAL_AUTOFILL_MATCH_REGEX_CACHE_LIMIT: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionKindTab {
    Ssh,
    Local,
    Telnet,
    Serial,
    Rdp,
    Vnc,
}

impl ConnectionKindTab {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Ssh => "SSH",
            Self::Local => "Local",
            Self::Telnet => "Telnet",
            Self::Serial => "Serial",
            Self::Rdp => "RDP",
            Self::Vnc => "VNC",
        }
    }

    pub(crate) fn from_connection_type(config: &nyaterm_core::ConnectionType) -> Self {
        match config {
            nyaterm_core::ConnectionType::Ssh { .. } => Self::Ssh,
            nyaterm_core::ConnectionType::LocalTerminal { .. } => Self::Local,
            nyaterm_core::ConnectionType::Telnet { .. } => Self::Telnet,
            nyaterm_core::ConnectionType::Serial { .. } => Self::Serial,
            nyaterm_core::ConnectionType::Rdp { .. } => Self::Rdp,
            nyaterm_core::ConnectionType::Vnc { .. } => Self::Vnc,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ConnectionEditorSelect {
    Authentication,
    SshAgentEndpoint,
    SshAgentForwardingPolicy,
    Group,
    SavedPassword,
    Account,
    SshKey,
    Otp,
    Proxy,
    ProxyJump,
    Backspace,
    Encoding,
    SftpCwdFollowMode,
    SftpFilenameEncoding,
    SftpPipelineDepth,
    SshAlgorithmMode,
    SshProfile,
    SshTerminalType,
    RdpCertificatePolicy,
    RdpDisplayMode,
    RdpClipboardMode,
    VncSecurityMode,
    VncScaleMode,
    RecordingMode,
    TelnetEnterMode,
    Shell,
    SerialPort,
    BaudRate,
    DataBits,
    Parity,
    StopBits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionEditorPasswordSource {
    Ask,
    Direct,
    Saved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionEditorAdvancedTab {
    Network,
    TwoFactor,
    AgentForwarding,
    PostLogin,
    Terminal,
    Sftp,
    X11,
    Backspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionEditorSshAlgorithmTab {
    KeyExchange,
    Ciphers,
    Macs,
    HostKeys,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionEditorTelnetTab {
    Input,
    Terminal,
    Compatibility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionEditorRdpTab {
    Security,
    Network,
    Display,
    Clipboard,
    Reconnect,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ConnectionEditorAdvancedVisibility {
    pub(crate) ssh: bool,
    pub(crate) local: bool,
    pub(crate) telnet: bool,
    pub(crate) serial: bool,
    pub(crate) rdp: bool,
    pub(crate) vnc: bool,
}

impl ConnectionEditorAdvancedVisibility {
    pub(crate) fn is_open(self, kind: ConnectionKindTab) -> bool {
        match kind {
            ConnectionKindTab::Ssh => self.ssh,
            ConnectionKindTab::Local => self.local,
            ConnectionKindTab::Telnet => self.telnet,
            ConnectionKindTab::Serial => self.serial,
            ConnectionKindTab::Rdp => self.rdp,
            ConnectionKindTab::Vnc => self.vnc,
        }
    }

    pub(crate) fn toggle(&mut self, kind: ConnectionKindTab) -> bool {
        let open = match kind {
            ConnectionKindTab::Ssh => &mut self.ssh,
            ConnectionKindTab::Local => &mut self.local,
            ConnectionKindTab::Telnet => &mut self.telnet,
            ConnectionKindTab::Serial => &mut self.serial,
            ConnectionKindTab::Rdp => &mut self.rdp,
            ConnectionKindTab::Vnc => &mut self.vnc,
        };
        *open = !*open;
        *open
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionEditorCredentialOverlay {
    Passwords,
    Keys,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ConnectionEditorField {
    Name,
    NewTag,
    NewGroupName,
    Description,
    Host,
    Port,
    Username,
    AgentEnvironmentVariable,
    AgentUnixSocket,
    AgentForwardingEnvironmentVariable,
    AgentForwardingSocketPath,
    Domain,
    Password,
    ShellPath,
    ShellArgs,
    WorkingDir,
    SerialPort,
    BaudRate,
    PostLoginCommand,
    PostLoginDelay,
    SftpShellDetectionTimeout,
    TelnetAutoLoginTimeout,
    TelnetAutoLoginUsernamePrompt,
    TelnetAutoLoginPasswordPrompt,
    TelnetAutoLoginSuccessPrompt,
    TelnetAutoLoginFailurePrompt,
    TelnetAutoLoginMaxRetries,
    RdpDisplayWidth,
    RdpDisplayHeight,
    RdpReconnectAttempts,
    VncReconnectAttempts,
}

impl ConnectionEditorField {
    pub(crate) fn next(
        self,
        kind: ConnectionKindTab,
        auth_mode: &str,
        password_field_visible: bool,
        post_login_fields_visible: bool,
    ) -> Self {
        match kind {
            ConnectionKindTab::Ssh => match self {
                Self::Name => Self::Description,
                Self::Description => Self::Host,
                Self::Host => Self::Port,
                Self::Port => Self::Username,
                Self::Username if auth_mode == "password" && password_field_visible => {
                    Self::Password
                }
                Self::Username if post_login_fields_visible => Self::PostLoginCommand,
                Self::Username => Self::Name,
                Self::Password if post_login_fields_visible => Self::PostLoginCommand,
                Self::Password => Self::Name,
                Self::PostLoginCommand => Self::PostLoginDelay,
                Self::PostLoginDelay => Self::Name,
                Self::SftpShellDetectionTimeout => Self::Name,
                other => other.next_fallback(kind),
            },
            ConnectionKindTab::Local => match self {
                Self::Name => Self::Description,
                Self::Description => Self::ShellPath,
                Self::ShellPath => Self::ShellArgs,
                Self::ShellArgs => Self::WorkingDir,
                Self::WorkingDir => Self::Name,
                other => other.next_fallback(kind),
            },
            ConnectionKindTab::Telnet => match self {
                Self::Name => Self::Description,
                Self::Description => Self::Host,
                Self::Host => Self::Port,
                Self::Port => Self::Name,
                other => other.next_fallback(kind),
            },
            ConnectionKindTab::Serial => match self {
                Self::Name => Self::Description,
                Self::Description => Self::SerialPort,
                Self::SerialPort => Self::BaudRate,
                Self::BaudRate => Self::Name,
                other => other.next_fallback(kind),
            },
            ConnectionKindTab::Rdp => match self {
                Self::Name => Self::Description,
                Self::Description => Self::Host,
                Self::Host => Self::Port,
                Self::Port => Self::Username,
                Self::Username => Self::Domain,
                Self::Domain if auth_mode == "password" && password_field_visible => Self::Password,
                Self::Domain => Self::Name,
                Self::Password => Self::Name,
                other => other.next_fallback(kind),
            },
            ConnectionKindTab::Vnc => match self {
                Self::Name => Self::Description,
                Self::Description => Self::Host,
                Self::Host => Self::Port,
                Self::Port if auth_mode == "password" && password_field_visible => Self::Password,
                Self::Port => Self::Name,
                Self::Password => Self::Name,
                other => other.next_fallback(kind),
            },
        }
    }

    fn next_fallback(self, kind: ConnectionKindTab) -> Self {
        match kind {
            ConnectionKindTab::Ssh => Self::Name,
            ConnectionKindTab::Local => Self::Name,
            ConnectionKindTab::Telnet => Self::Name,
            ConnectionKindTab::Serial => Self::Name,
            ConnectionKindTab::Rdp => Self::Name,
            ConnectionKindTab::Vnc => Self::Name,
        }
    }
}

#[cfg(test)]
mod connection_editor_field_tests {
    use super::{ConnectionEditorField, ConnectionKindTab};

    #[test]
    fn ssh_tab_order_skips_collapsed_post_login_fields() {
        assert_eq!(
            ConnectionEditorField::Password.next(ConnectionKindTab::Ssh, "password", true, false,),
            ConnectionEditorField::Name
        );
        assert_eq!(
            ConnectionEditorField::Username.next(ConnectionKindTab::Ssh, "key", false, false),
            ConnectionEditorField::Name
        );
    }

    #[test]
    fn ssh_tab_order_reaches_visible_post_login_fields() {
        assert_eq!(
            ConnectionEditorField::Password.next(ConnectionKindTab::Ssh, "password", true, true,),
            ConnectionEditorField::PostLoginCommand
        );
        assert_eq!(
            ConnectionEditorField::PostLoginCommand.next(
                ConnectionKindTab::Ssh,
                "password",
                true,
                true,
            ),
            ConnectionEditorField::PostLoginDelay
        );
    }

    #[test]
    fn ssh_tab_order_skips_non_direct_password_field() {
        assert_eq!(
            ConnectionEditorField::Username.next(ConnectionKindTab::Ssh, "password", false, false,),
            ConnectionEditorField::Name
        );
    }

    #[test]
    fn rdp_tab_order_reaches_domain_before_optional_password() {
        assert_eq!(
            ConnectionEditorField::Username.next(ConnectionKindTab::Rdp, "password", true, false,),
            ConnectionEditorField::Domain
        );
        assert_eq!(
            ConnectionEditorField::Domain.next(ConnectionKindTab::Rdp, "password", true, false,),
            ConnectionEditorField::Password
        );
        assert_eq!(
            ConnectionEditorField::Domain.next(ConnectionKindTab::Rdp, "none", false, false,),
            ConnectionEditorField::Name
        );
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ConnectionEditorState {
    pub(crate) id: Option<String>,
    pub(crate) kind: ConnectionKindTab,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) tags: Vec<String>,
    pub(crate) new_tag: String,
    pub(crate) icon: Option<String>,
    /// Mirrors `SavedConnection::icon_auto_detect_enabled` while editing.
    pub(crate) icon_auto_detect: bool,
    pub(crate) group_id: Option<String>,
    pub(crate) new_group_name: String,
    pub(crate) pending_group_name: Option<String>,
    pub(crate) pending_group_parent_id: Option<String>,
    pub(crate) host: String,
    pub(crate) port: String,
    pub(crate) username: String,
    pub(crate) domain: String,
    pub(crate) auth_mode: String,
    pub(crate) rdp_security: nyaterm_core::RdpSecuritySettings,
    pub(crate) rdp_display: nyaterm_core::RdpDisplaySettings,
    pub(crate) rdp_clipboard: nyaterm_core::RdpClipboardSettings,
    pub(crate) rdp_reconnect: nyaterm_core::RdpReconnectSettings,
    pub(crate) rdp_advanced_tab: ConnectionEditorRdpTab,
    pub(crate) vnc_security: nyaterm_core::VncSecuritySettings,
    pub(crate) vnc_display: nyaterm_core::VncDisplaySettings,
    pub(crate) vnc_clipboard: nyaterm_core::VncClipboardSettings,
    pub(crate) vnc_reconnect: nyaterm_core::VncReconnectSettings,
    pub(crate) vnc_shared: bool,
    pub(crate) vnc_view_only: bool,
    pub(crate) password_source: ConnectionEditorPasswordSource,
    pub(crate) password_id: Option<String>,
    pub(crate) account_id: Option<String>,
    pub(crate) password: nyaterm_core::SecretString,
    pub(crate) existing_password: Option<nyaterm_core::SecretString>,
    pub(crate) existing_password_locked: bool,
    pub(crate) key_id: Option<String>,
    pub(crate) otp_id: Option<String>,
    pub(crate) auto_fill_otp: bool,
    pub(crate) proxy_id: Option<String>,
    pub(crate) proxy_jump_id: Option<String>,
    /// Preserved for imported SSH configs even though it is not an ordinary editor field.
    pub(crate) host_key_alias: Option<String>,
    pub(crate) x11_forwarding: bool,
    pub(crate) dynamic_tab_title: bool,
    pub(crate) agent_endpoint: nyaterm_core::SshAgentEndpoint,
    pub(crate) agent_forwarding_config: nyaterm_core::SshAgentForwardingConfig,
    pub(crate) agent_allow_all_confirmed: bool,
    pub(crate) agent_forwarding_endpoint_index: usize,
    pub(crate) agent_preview: Option<nyaterm_transport::SshAgentIdentityPreviewResponse>,
    pub(crate) agent_preview_loading: bool,
    pub(crate) backspace_mode: String,
    pub(crate) encoding: String,
    pub(crate) ssh_profile: nyaterm_core::SshProfile,
    pub(crate) terminal_type: Option<nyaterm_core::SshTerminalType>,
    pub(crate) sftp_enabled: bool,
    pub(crate) sftp_compatibility_mode: bool,
    pub(crate) sftp_cwd_follow_mode: String,
    pub(crate) sftp_shell_detection_timeout_ms: String,
    pub(crate) sftp_filename_encoding: String,
    pub(crate) sftp_pipeline_depth: Option<u32>,
    pub(crate) sftp_extra: serde_json::Map<String, serde_json::Value>,
    pub(crate) ssh_algorithm_mode: String,
    pub(crate) ssh_algorithm_kex: Vec<String>,
    pub(crate) ssh_algorithm_ciphers: Vec<String>,
    pub(crate) ssh_algorithm_macs: Vec<String>,
    pub(crate) ssh_algorithm_host_keys: Vec<String>,
    pub(crate) ssh_algorithm_tab: ConnectionEditorSshAlgorithmTab,
    pub(crate) shell_path: String,
    pub(crate) shell_args: String,
    pub(crate) working_dir: String,
    pub(crate) serial_port: String,
    pub(crate) baud_rate: String,
    pub(crate) data_bits: String,
    pub(crate) parity: String,
    pub(crate) stop_bits: String,
    pub(crate) raw_tcp_cli: bool,
    pub(crate) telnet_enter_mode: String,
    pub(crate) local_echo: bool,
    pub(crate) local_line_edit: bool,
    pub(crate) force_character_at_a_time: bool,
    pub(crate) send_naws: bool,
    pub(crate) send_sga: bool,
    pub(crate) telnet_auto_login_enabled: bool,
    pub(crate) telnet_auto_login_send_wake_enter: bool,
    pub(crate) telnet_auto_login_timeout_ms: String,
    pub(crate) telnet_auto_login_username_prompt_regex: String,
    pub(crate) telnet_auto_login_password_prompt_regex: String,
    pub(crate) telnet_auto_login_success_prompt_regex: String,
    pub(crate) telnet_auto_login_failure_prompt_regex: String,
    pub(crate) telnet_auto_login_max_retries: String,
    pub(crate) post_login_enabled: bool,
    pub(crate) post_login_command: String,
    pub(crate) post_login_delay_ms: String,
    pub(crate) recording: Option<nyaterm_core::ConnectionRecordingSettings>,
    pub(crate) advanced: ConnectionEditorAdvancedVisibility,
    pub(crate) advanced_network_tab: ConnectionEditorAdvancedTab,
    pub(crate) advanced_behavior_tab: ConnectionEditorAdvancedTab,
    pub(crate) telnet_advanced_tab: ConnectionEditorTelnetTab,
    pub(crate) connect_after_save: bool,
    pub(crate) focused_field: ConnectionEditorField,
    pub(crate) error: Option<String>,
}

impl ConnectionEditorState {
    pub(crate) fn add_tag(&mut self) -> bool {
        let tag = self.new_tag.trim();
        if tag.is_empty() || self.tags.iter().any(|existing| existing == tag) {
            return false;
        }
        self.tags.push(tag.to_string());
        self.new_tag.clear();
        true
    }

    pub(crate) fn remove_tag(&mut self, tag: &str) -> bool {
        let count = self.tags.len();
        self.tags.retain(|existing| existing != tag);
        count != self.tags.len()
    }
}

impl std::fmt::Debug for ConnectionEditorState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConnectionEditorState")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("name", &self.name)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("auth_mode", &self.auth_mode)
            .field("password", &"<redacted>")
            .field("existing_password", &"<redacted>")
            .field("post_login_command", &"<redacted>")
            .field("focused_field", &self.focused_field)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// What the saved-connections list's context menu was opened on.
///
/// The list owns exactly one context menu and picks its items from this. Giving
/// a row its own nested menu instead would open both on a single right-click,
/// because `ContextMenu` registers a plain hitbox-gated mouse listener and the
/// outer one runs first, before the row can stop propagation. Only the menu that
/// receives the item click dismisses, and the one left open keeps re-focusing
/// itself every layout pass - which strands whatever opens next, since a dialog
/// dismisses through actions routed along the focused element's path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum ConnectionListContextTarget {
    /// Empty space in the list, below or beside the rows.
    #[default]
    List,
    Connection(String),
    Group(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConnectionGroupEditorMode {
    Create,
    Rename,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectionGroupEditorState {
    pub(crate) mode: ConnectionGroupEditorMode,
    pub(crate) id: Option<String>,
    pub(crate) name: String,
    pub(crate) parent_id: Option<String>,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActionLinkMenuAction {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) command: Option<String>,
    pub(crate) open_url: Option<String>,
    pub(crate) is_default: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ActionLinkMenuState {
    pub(crate) x: Pixels,
    pub(crate) y: Pixels,
    pub(crate) kind_label: String,
    pub(crate) value: String,
    pub(crate) actions: Vec<ActionLinkMenuAction>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ActionLinkTooltipState {
    pub(crate) x: Pixels,
    pub(crate) y: Pixels,
    pub(crate) kind_label: String,
    pub(crate) value: String,
    pub(crate) default_action_label: String,
    pub(crate) default_action_preview: String,
    pub(crate) has_more_actions: bool,
    /// Identity key for hover stability (kind|value|start|end).
    pub(crate) match_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TranslationDialogState {
    pub(crate) source_text: String,
    pub(crate) provider: String,
    pub(crate) provider_label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CommandSuggestionItem {
    pub(crate) command: String,
    pub(crate) display: String,
    pub(crate) source: String,
    pub(crate) score: u32,
    pub(crate) indices: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CommandSuggestionState {
    pub(crate) session_id: String,
    pub(crate) draft: String,
    pub(crate) items: Vec<CommandSuggestionItem>,
    pub(crate) selected_index: Option<usize>,
    pub(crate) cursor_row: usize,
    pub(crate) cursor_col: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CredentialSuggestionState {
    pub(crate) session_id: String,
    pub(crate) kind: CredentialPromptKind,
    pub(crate) matches: Vec<CredentialAutofillTarget>,
    pub(crate) prompt_text: String,
    pub(crate) selected_index: usize,
    pub(crate) cursor_row: usize,
    pub(crate) cursor_col: usize,
}

/// A credential the autofill pipeline can act on. Vault entries come from the
/// security center; the connection-password candidate is derived from the
/// active session's source connection and is resolved through the connection
/// store at fill time, never the credential vault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CredentialAutofillTarget {
    Vault(SavedCredential),
    ConnectionPassword(ConnectionPasswordTarget),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectionPasswordTarget {
    pub(crate) connection_id: String,
    pub(crate) connection_name: String,
    pub(crate) username: String,
}

impl CredentialAutofillTarget {
    pub(crate) fn enabled(&self) -> bool {
        match self {
            Self::Vault(credential) => credential.enabled,
            Self::ConnectionPassword(_) => true,
        }
    }

    pub(crate) fn username(&self) -> &str {
        match self {
            Self::Vault(credential) => &credential.username,
            Self::ConnectionPassword(candidate) => &candidate.username,
        }
    }

    pub(crate) fn is_connection_password(&self) -> bool {
        matches!(self, Self::ConnectionPassword(_))
    }

    /// Panel/status label. Not persisted and never a credential id.
    pub(crate) fn display_name(&self) -> String {
        match self {
            Self::Vault(credential) => credential.name.clone(),
            Self::ConnectionPassword(candidate) => format!(
                "{} ({})",
                candidate.connection_name,
                t!("credentialAutofill.connectionPasswordLabel")
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingCredentialAutofill {
    pub(crate) session_id: String,
    pub(crate) credential_id: String,
    pub(crate) expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CredentialAutofillMatchRequestKey {
    pub(crate) request_id: u64,
    pub(crate) session_id: String,
    pub(crate) prompt_text: String,
}

#[derive(Debug, Clone)]
pub(crate) struct CredentialAutofillMatchRequest {
    pub(crate) key: CredentialAutofillMatchRequestKey,
    pub(crate) current_line: String,
    pub(crate) prompt_kind: CredentialPromptKind,
    pub(crate) credentials: Vec<CredentialAutofillTarget>,
    pub(crate) pending: Option<PendingCredentialAutofill>,
}

#[derive(Debug, Clone)]
pub(crate) struct CredentialAutofillMatchEvent {
    pub(crate) key: CredentialAutofillMatchRequestKey,
    pub(crate) outcome: CredentialAutofillMatchOutcome,
}

#[derive(Debug, Clone)]
pub(crate) enum CredentialAutofillMatchOutcome {
    Suggest {
        kind: CredentialPromptKind,
        matches: Vec<CredentialAutofillTarget>,
        clear_pending: bool,
    },
    AutoFill {
        credential: CredentialAutofillTarget,
        kind: CredentialPromptKind,
    },
    NoMatch {
        clear_pending: bool,
    },
}

pub(crate) struct CredentialAutofillMatchPipeline {
    command_tx: Option<mpsc::Sender<CredentialAutofillMatchRequest>>,
    worker: Option<thread::JoinHandle<()>>,
    event_queue: CredentialAutofillMatchEventQueue,
    /// Taken once by `NyaTermApp::start_credential_autofill_match_drain`,
    /// which owns delivery from then on.
    wake_rx: Option<UnboundedReceiver<()>>,
}

impl CredentialAutofillMatchPipeline {
    pub(crate) fn spawn() -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (wake, wake_rx) = EventWake::new();
        let event_queue =
            CredentialAutofillMatchEventQueue::new(CREDENTIAL_AUTOFILL_MATCH_EVENT_CAP, wake);
        let event_queue_for_worker = event_queue.clone();
        let worker = thread::Builder::new()
            .name("nyaterm-credential-autofill".to_string())
            .spawn(move || run_credential_autofill_matcher(command_rx, event_queue_for_worker))
            .expect("failed to spawn credential autofill matcher");
        Self {
            command_tx: Some(command_tx),
            worker: Some(worker),
            event_queue,
            wake_rx: Some(wake_rx),
        }
    }

    pub(crate) fn take_wake_receiver(&mut self) -> Option<UnboundedReceiver<()>> {
        self.wake_rx.take()
    }

    /// Declare interest in the next match reply. See `models::event_wake`: this
    /// must happen before the consumer checks the queue, or a reply pushed in
    /// between is not signalled and the consumer sleeps on a non-empty queue.
    pub(crate) fn arm_event_wake(&self) {
        self.event_queue.wake.arm(ANY_INTEREST);
    }

    pub(crate) fn request(&self, request: CredentialAutofillMatchRequest) {
        if let Some(command_tx) = &self.command_tx {
            let _ = command_tx.send(request);
        }
    }

    pub(crate) fn try_recv_event(&self) -> Option<CredentialAutofillMatchEvent> {
        self.event_queue.try_recv()
    }

    pub(crate) fn shutdown(&mut self) {
        self.command_tx.take();
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::warn!("credential autofill worker panicked during shutdown");
        }
    }
}

impl Drop for CredentialAutofillMatchPipeline {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Default for CredentialAutofillMatchPipeline {
    fn default() -> Self {
        Self::spawn()
    }
}

#[derive(Clone)]
struct CredentialAutofillMatchEventQueue {
    inner: Arc<Mutex<VecDeque<CredentialAutofillMatchEvent>>>,
    cap: usize,
    /// The queue drops and dedups entries, so it cannot become a channel; only
    /// the wake signal lives outside it.
    wake: EventWake,
}

impl CredentialAutofillMatchEventQueue {
    fn new(cap: usize, wake: EventWake) -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(cap.min(128)))),
            cap,
            wake,
        }
    }

    fn push(&self, event: CredentialAutofillMatchEvent) {
        let Ok(mut queue) = self.inner.lock() else {
            return;
        };
        queue.retain(|queued| {
            queued.key.session_id != event.key.session_id
                || queued.key.prompt_text != event.key.prompt_text
        });
        while queue.len() >= self.cap.max(1) {
            queue.pop_front();
        }
        queue.push_back(event);
        drop(queue);
        self.wake.signal(ANY_INTEREST);
    }

    fn try_recv(&self) -> Option<CredentialAutofillMatchEvent> {
        self.inner.lock().ok()?.pop_front()
    }

    #[cfg(test)]
    fn arm_wake_for_test(&self) {
        self.wake.arm(ANY_INTEREST);
    }
}

fn run_credential_autofill_matcher(
    command_rx: mpsc::Receiver<CredentialAutofillMatchRequest>,
    event_queue: CredentialAutofillMatchEventQueue,
) {
    let mut regex_cache = HashMap::new();
    while let Ok(request) = command_rx.recv() {
        let event = CredentialAutofillMatchEvent {
            key: request.key.clone(),
            outcome: credential_autofill_match_outcome(request, &mut regex_cache),
        };
        event_queue.push(event);
    }
}

fn credential_autofill_match_outcome(
    request: CredentialAutofillMatchRequest,
    regex_cache: &mut HashMap<String, regex::Regex>,
) -> CredentialAutofillMatchOutcome {
    if let Some(pending) = request.pending.as_ref()
        && pending.session_id == request.key.session_id
    {
        let pending_credential = request.credentials.iter().find_map(|target| match target {
            CredentialAutofillTarget::Vault(credential)
                if credential.id == pending.credential_id =>
            {
                Some(credential)
            }
            _ => None,
        });
        if let Some(credential) = pending_credential
            && (credential_matches_prompt_cached(
                credential,
                CredentialPromptKind::Password,
                &request.current_line,
                regex_cache,
            ) || credential_matches_prompt_cached(
                credential,
                CredentialPromptKind::Password,
                &request.key.prompt_text,
                regex_cache,
            ))
        {
            return CredentialAutofillMatchOutcome::AutoFill {
                credential: CredentialAutofillTarget::Vault(credential.clone()),
                kind: CredentialPromptKind::Password,
            };
        }
        if credential_autofill_detect_prompt_kind(&request.current_line)
            != Some(CredentialPromptKind::Password)
        {
            return CredentialAutofillMatchOutcome::NoMatch {
                clear_pending: false,
            };
        }
    }

    match request.prompt_kind {
        CredentialPromptKind::Password => {
            let matches = find_matching_credential_targets_cached(
                &request.credentials,
                CredentialPromptKind::Password,
                &request.key.prompt_text,
                regex_cache,
            );
            if !matches.is_empty() {
                return CredentialAutofillMatchOutcome::Suggest {
                    kind: CredentialPromptKind::Password,
                    matches,
                    clear_pending: true,
                };
            }
            let fallback = find_password_only_fallback_targets(&request.credentials);
            if !fallback.is_empty() {
                return CredentialAutofillMatchOutcome::Suggest {
                    kind: CredentialPromptKind::Password,
                    matches: fallback,
                    clear_pending: true,
                };
            }
            CredentialAutofillMatchOutcome::NoMatch {
                clear_pending: true,
            }
        }
        CredentialPromptKind::Username => {
            let matches = find_matching_credential_targets_cached(
                &request.credentials,
                CredentialPromptKind::Username,
                &request.key.prompt_text,
                regex_cache,
            );
            if matches.is_empty() {
                CredentialAutofillMatchOutcome::NoMatch {
                    clear_pending: false,
                }
            } else {
                CredentialAutofillMatchOutcome::Suggest {
                    kind: CredentialPromptKind::Username,
                    matches,
                    clear_pending: false,
                }
            }
        }
    }
}

/// Regex-matched credential suggestions come from vault candidates only; the
/// connection-password candidate has no prompt pattern of its own.
fn find_matching_credential_targets_cached(
    credentials: &[CredentialAutofillTarget],
    kind: CredentialPromptKind,
    output: &str,
    regex_cache: &mut HashMap<String, regex::Regex>,
) -> Vec<CredentialAutofillTarget> {
    credentials
        .iter()
        .filter_map(|target| match target {
            CredentialAutofillTarget::Vault(credential)
                if credential_matches_prompt_cached(credential, kind, output, regex_cache) =>
            {
                Some(target.clone())
            }
            _ => None,
        })
        .collect()
}

/// Default password-prompt fallback (Tauri parity): every enabled target,
/// preserving order so the connection-password candidate stays first.
fn find_password_only_fallback_targets(
    credentials: &[CredentialAutofillTarget],
) -> Vec<CredentialAutofillTarget> {
    credentials
        .iter()
        .filter(|target| target.enabled())
        .cloned()
        .collect()
}

fn credential_matches_prompt_cached(
    credential: &SavedCredential,
    kind: CredentialPromptKind,
    output: &str,
    regex_cache: &mut HashMap<String, regex::Regex>,
) -> bool {
    if !credential.enabled {
        return false;
    }
    if kind == CredentialPromptKind::Username && credential.username.trim().is_empty() {
        return false;
    }
    if kind == CredentialPromptKind::Password && !credential.has_password {
        return false;
    }

    let pattern = get_credential_prompt_pattern(credential, kind);
    if pattern.is_empty() {
        return false;
    }
    let cache_key = format!("{}:{kind:?}:{pattern}", credential.id);
    if !regex_cache.contains_key(&cache_key) {
        if regex_cache.len() >= CREDENTIAL_AUTOFILL_MATCH_REGEX_CACHE_LIMIT {
            regex_cache.clear();
        }
        let Some(regex) = compile_prompt_regex(&pattern) else {
            return false;
        };
        regex_cache.insert(cache_key.clone(), regex);
    }
    regex_cache
        .get(&cache_key)
        .is_some_and(|regex| regex.is_match(output))
}

fn credential_autofill_detect_prompt_kind(prompt: &str) -> Option<CredentialPromptKind> {
    let trimmed = prompt.trim();
    if trimmed.is_empty()
        || !trimmed
            .chars()
            .last()
            .is_some_and(|ch| ch == ':' || ch == '：')
    {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("password")
        || lower.contains("passphrase")
        || lower.contains("passcode")
        || lower.contains("pin")
        || lower.contains("otp")
        || lower.contains("verification code")
        || lower.contains("authentication code")
        || lower.contains("auth code")
        || lower.contains("2fa")
        || lower.contains("mfa")
        || trimmed.contains("密码")
        || trimmed.contains("口令")
        || trimmed.contains("验证码")
        || trimmed.contains("动态码")
        || trimmed.contains("动态口令")
    {
        return Some(CredentialPromptKind::Password);
    }
    if lower.contains("username")
        || lower.contains("user name")
        || lower.contains("login as")
        || lower.contains("login")
        || lower.contains("account")
        || lower.contains("user")
        || trimmed.contains("用户名")
        || trimmed.contains("用户")
        || trimmed.contains("账号")
        || trimmed.contains("账户")
        || trimmed.contains("登录名")
    {
        return Some(CredentialPromptKind::Username);
    }
    None
}

const CREDENTIAL_AUTOFILL_MATCH_EVENT_CAP: usize = 128;

#[cfg(test)]
mod credential_autofill_match_tests {
    use std::collections::HashMap;

    use super::{
        CredentialAutofillMatchEvent, CredentialAutofillMatchEventQueue,
        CredentialAutofillMatchOutcome, CredentialAutofillMatchRequest,
        CredentialAutofillMatchRequestKey, CredentialAutofillTarget, CredentialPromptKind,
        PendingCredentialAutofill, SavedCredential, credential_autofill_match_outcome,
    };
    use crate::models::event_wake::EventWake;

    fn credential(
        id: &str,
        username: &str,
        username_prompt_regex: Option<&str>,
        password_prompt_regex: Option<&str>,
        has_password: bool,
    ) -> SavedCredential {
        SavedCredential {
            id: id.to_string(),
            sort_order: 0,
            name: id.to_string(),
            username: username.to_string(),
            password: None,
            username_prompt_regex: username_prompt_regex.map(str::to_string),
            password_prompt_regex: password_prompt_regex.map(str::to_string),
            enabled: true,
            has_password,
        }
    }

    fn vault_credentials(credentials: Vec<SavedCredential>) -> Vec<CredentialAutofillTarget> {
        credentials
            .into_iter()
            .map(CredentialAutofillTarget::Vault)
            .collect()
    }

    fn request(
        prompt_text: &str,
        prompt_kind: CredentialPromptKind,
        credentials: Vec<CredentialAutofillTarget>,
        pending: Option<PendingCredentialAutofill>,
    ) -> CredentialAutofillMatchRequest {
        CredentialAutofillMatchRequest {
            key: CredentialAutofillMatchRequestKey {
                request_id: 1,
                session_id: "s1".to_string(),
                prompt_text: prompt_text.to_string(),
            },
            current_line: prompt_text.to_string(),
            prompt_kind,
            credentials,
            pending,
        }
    }

    fn event(request_id: u64, session_id: &str, prompt_text: &str) -> CredentialAutofillMatchEvent {
        CredentialAutofillMatchEvent {
            key: CredentialAutofillMatchRequestKey {
                request_id,
                session_id: session_id.to_string(),
                prompt_text: prompt_text.to_string(),
            },
            outcome: CredentialAutofillMatchOutcome::NoMatch {
                clear_pending: false,
            },
        }
    }

    #[test]
    fn credential_autofill_event_queue_signals_once_per_arm() {
        let (wake, mut wake_rx) = EventWake::new();
        let queue = CredentialAutofillMatchEventQueue::new(8, wake);

        // No interest declared yet, so the matcher thread costs nothing.
        queue.push(event(1, "s1", "Password:"));
        assert!(wake_rx.try_recv().is_err());

        queue.arm_wake_for_test();
        queue.push(event(2, "s1", "Password:"));
        assert!(wake_rx.try_recv().is_ok(), "an armed queue must signal");
        queue.push(event(3, "s1", "Password:"));
        assert!(
            wake_rx.try_recv().is_err(),
            "a burst after one arm must not queue a wake per reply"
        );
    }

    #[test]
    fn credential_autofill_event_queue_keeps_latest_prompt_match() {
        let (wake, _wake_rx) = EventWake::new();
        let queue = CredentialAutofillMatchEventQueue::new(8, wake);

        queue.push(event(1, "s1", "Password:"));
        queue.push(event(2, "s1", "Password:"));

        assert!(matches!(
            queue.try_recv(),
            Some(CredentialAutofillMatchEvent { key, .. }) if key.request_id == 2
        ));
        assert!(queue.try_recv().is_none());
    }

    #[test]
    fn credential_autofill_event_queue_preserves_different_prompts() {
        let (wake, _wake_rx) = EventWake::new();
        let queue = CredentialAutofillMatchEventQueue::new(8, wake);

        queue.push(event(1, "s1", "login as:"));
        queue.push(event(2, "s1", "Password:"));
        queue.push(event(3, "s2", "Password:"));

        assert!(matches!(
            queue.try_recv(),
            Some(CredentialAutofillMatchEvent { key, .. })
                if key.request_id == 1 && key.session_id == "s1"
        ));
        assert!(matches!(
            queue.try_recv(),
            Some(CredentialAutofillMatchEvent { key, .. })
                if key.request_id == 2 && key.session_id == "s1"
        ));
        assert!(matches!(
            queue.try_recv(),
            Some(CredentialAutofillMatchEvent { key, .. })
                if key.request_id == 3 && key.session_id == "s2"
        ));
        assert!(queue.try_recv().is_none());
    }

    #[test]
    fn credential_autofill_worker_suggests_matching_username() {
        let mut regex_cache = HashMap::new();
        let output = credential_autofill_match_outcome(
            request(
                "login as:",
                CredentialPromptKind::Username,
                vault_credentials(vec![credential(
                    "c1",
                    "root",
                    Some("login as:"),
                    None,
                    true,
                )]),
                None,
            ),
            &mut regex_cache,
        );

        match output {
            CredentialAutofillMatchOutcome::Suggest {
                kind,
                matches,
                clear_pending,
            } => {
                assert_eq!(kind, CredentialPromptKind::Username);
                assert_eq!(matches.len(), 1);
                assert!(matches!(
                    &matches[0],
                    CredentialAutofillTarget::Vault(credential) if credential.id == "c1"
                ));
                assert!(!clear_pending);
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn credential_autofill_worker_falls_back_to_password_only_credentials() {
        let mut regex_cache = HashMap::new();
        let output = credential_autofill_match_outcome(
            request(
                "Password:",
                CredentialPromptKind::Password,
                vault_credentials(vec![credential("c1", "", None, None, true)]),
                None,
            ),
            &mut regex_cache,
        );

        match output {
            CredentialAutofillMatchOutcome::Suggest {
                kind,
                matches,
                clear_pending,
            } => {
                assert_eq!(kind, CredentialPromptKind::Password);
                assert_eq!(matches.len(), 1);
                assert!(matches!(
                    &matches[0],
                    CredentialAutofillTarget::Vault(credential) if credential.id == "c1"
                ));
                assert!(clear_pending);
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn credential_autofill_worker_autofills_pending_password() {
        let mut regex_cache = HashMap::new();
        let output = credential_autofill_match_outcome(
            request(
                "Password:",
                CredentialPromptKind::Password,
                vault_credentials(vec![credential(
                    "c1",
                    "root",
                    Some("login as:"),
                    Some("Password:"),
                    true,
                )]),
                Some(PendingCredentialAutofill {
                    session_id: "s1".to_string(),
                    credential_id: "c1".to_string(),
                    expires_at_ms: u64::MAX,
                }),
            ),
            &mut regex_cache,
        );

        match output {
            CredentialAutofillMatchOutcome::AutoFill { credential, kind } => {
                assert!(matches!(
                    credential,
                    CredentialAutofillTarget::Vault(saved) if saved.id == "c1"
                ));
                assert_eq!(kind, CredentialPromptKind::Password);
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn credential_autofill_worker_connects_password_variant_precedes_vault_fallback() {
        let mut regex_cache = HashMap::new();
        let candidate =
            CredentialAutofillTarget::ConnectionPassword(super::ConnectionPasswordTarget {
                connection_id: "conn-1".to_string(),
                connection_name: "prod".to_string(),
                username: "dev".to_string(),
            });
        let mut credentials = vec![candidate.clone()];
        credentials.extend(vault_credentials(vec![credential(
            "c1", "", None, None, true,
        )]));
        let output = credential_autofill_match_outcome(
            request(
                "Password:",
                CredentialPromptKind::Password,
                credentials,
                None,
            ),
            &mut regex_cache,
        );

        match output {
            CredentialAutofillMatchOutcome::Suggest {
                kind,
                matches,
                clear_pending,
            } => {
                assert_eq!(kind, CredentialPromptKind::Password);
                assert_eq!(matches.len(), 2);
                assert_eq!(matches[0], candidate, "connection password stays first");
                assert!(matches!(
                    &matches[1],
                    CredentialAutofillTarget::Vault(credential) if credential.id == "c1"
                ));
                assert!(clear_pending);
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn credential_autofill_worker_ignores_connection_password_as_username() {
        let mut regex_cache = HashMap::new();
        let candidate =
            CredentialAutofillTarget::ConnectionPassword(super::ConnectionPasswordTarget {
                connection_id: "conn-1".to_string(),
                connection_name: "prod".to_string(),
                username: "dev".to_string(),
            });
        let output = credential_autofill_match_outcome(
            request(
                "login as:",
                CredentialPromptKind::Username,
                vec![candidate],
                None,
            ),
            &mut regex_cache,
        );

        assert!(matches!(
            output,
            CredentialAutofillMatchOutcome::NoMatch {
                clear_pending: false
            }
        ));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionSortMode {
    Default,
    NameAsc,
    NameDesc,
}

impl ConnectionSortMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::NameAsc => "Name A-Z",
            Self::NameDesc => "Name Z-A",
        }
    }

    pub(crate) fn from_setting(value: &str) -> Self {
        match value.trim() {
            "name-asc" => Self::NameAsc,
            "name-desc" => Self::NameDesc,
            _ => Self::Default,
        }
    }

    pub(crate) fn persistence_id(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::NameAsc => "name-asc",
            Self::NameDesc => "name-desc",
        }
    }

    pub(crate) fn next(self) -> Self {
        match self {
            Self::Default => Self::NameAsc,
            Self::NameAsc => Self::NameDesc,
            Self::NameDesc => Self::Default,
        }
    }
}
