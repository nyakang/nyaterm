use futures::channel::mpsc::UnboundedReceiver;
use gpui::Pixels;
use nyaterm_core::CredentialPromptKind;
use nyaterm_core::mac_address::MacAddressFormat;
use rust_i18n::t;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use crate::action_links::{ActionLinkAction, ActionLinkTarget};

use super::event_wake::{ANY_INTEREST, EventWake};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ConnectionKindTab {
    Ssh,
    Local,
    Telnet,
    Serial,
    Rdp,
    Vnc,
    Adb,
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
            Self::Adb => "Android (ADB)",
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
            nyaterm_core::ConnectionType::Adb { .. } => Self::Adb,
        }
    }
}

/// Navigation state belongs only to the editor draft, never to saved connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ConnectionEditorSection {
    Basic,
    AndroidScreen,
    Network,
    Terminal,
    Sftp,
    Forwarding,
    PostLogin,
    Algorithms,
    AutoLogin,
    Compatibility,
    Security,
    Display,
    Session,
    Serial,
}

impl ConnectionEditorSection {
    /// Sections offered by the editor. "Default session settings" reuses the
    /// same surface without the basic-info tab, because identity fields never
    /// carry over between connections.
    pub(crate) fn for_kind(kind: ConnectionKindTab, defaults_mode: bool) -> &'static [Self] {
        let sections = Self::for_kind_all_tabs(kind);
        if defaults_mode {
            &sections[1..]
        } else {
            sections
        }
    }

    fn for_kind_all_tabs(kind: ConnectionKindTab) -> &'static [Self] {
        match kind {
            ConnectionKindTab::Adb => &[Self::Basic, Self::AndroidScreen],
            ConnectionKindTab::Ssh => &[
                Self::Basic,
                Self::Network,
                Self::Terminal,
                Self::Sftp,
                Self::Forwarding,
                Self::PostLogin,
                Self::Algorithms,
            ],
            ConnectionKindTab::Telnet => &[
                Self::Basic,
                Self::Network,
                Self::AutoLogin,
                Self::Terminal,
                Self::Compatibility,
            ],
            ConnectionKindTab::Rdp | ConnectionKindTab::Vnc => &[
                Self::Basic,
                Self::Network,
                Self::Security,
                Self::Display,
                Self::Session,
            ],
            ConnectionKindTab::Serial => &[Self::Basic, Self::Serial, Self::Terminal],
            ConnectionKindTab::Local => &[Self::Basic, Self::Terminal],
        }
    }

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Basic => "basic",
            Self::AndroidScreen => "androidScreen",
            Self::Network => "network",
            Self::Terminal => "terminal",
            Self::Sftp => "sftp",
            Self::Forwarding => "forwarding",
            Self::PostLogin => "postLogin",
            Self::Algorithms => "algorithms",
            Self::AutoLogin => "autoLogin",
            Self::Compatibility => "compatibility",
            Self::Security => "security",
            Self::Display => "display",
            Self::Session => "session",
            Self::Serial => "serial",
        }
    }

    pub(crate) fn label(self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::Basic => t!("dialog.editorSections.basic"),
            Self::AndroidScreen => t!("android.screenSettings"),
            Self::Network => t!("dialog.editorSections.network"),
            Self::Terminal => t!("dialog.editorSections.terminal"),
            Self::Sftp => t!("dialog.editorSections.sftp"),
            Self::Forwarding => t!("dialog.editorSections.forwarding"),
            Self::PostLogin => t!("dialog.editorSections.postLogin"),
            Self::Algorithms => t!("dialog.editorSections.algorithms"),
            Self::AutoLogin => t!("dialog.editorSections.autoLogin"),
            Self::Compatibility => t!("dialog.editorSections.compatibility"),
            Self::Security => t!("dialog.editorSections.security"),
            Self::Display => t!("dialog.editorSections.display"),
            Self::Session => t!("dialog.editorSections.session"),
            Self::Serial => t!("dialog.editorSections.serial"),
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
    RdpBackend,
    RdpCertificatePolicy,
    RdpDisplayMode,
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
    FlowControl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionEditorPasswordSource {
    Ask,
    Direct,
    Saved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConnectionPasswordEdit {
    Keep,
    Replace,
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionEditorForwardingTab {
    Agent,
    X11,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionEditorSshAlgorithmTab {
    KeyExchange,
    Ciphers,
    Macs,
    HostKeys,
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
    AndroidMaxSize,
    AndroidFps,
    AndroidBitRate,
    SerialPort,
    BaudRate,
    PostLoginCommand,
    PostLoginDelay,
    SftpShellDetectionTimeout,
    TelnetKeepAliveInterval,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ConnectionEditorFocusTarget {
    Field(ConnectionEditorField),
    Select(ConnectionEditorSelect),
    ForwardingEndpoint {
        index: usize,
        field: ConnectionEditorField,
    },
    Heading,
}

impl ConnectionEditorField {
    pub(crate) fn section(self) -> ConnectionEditorSection {
        match self {
            Self::AndroidMaxSize | Self::AndroidFps | Self::AndroidBitRate => {
                ConnectionEditorSection::AndroidScreen
            }
            Self::Name
            | Self::NewTag
            | Self::NewGroupName
            | Self::Description
            | Self::Host
            | Self::Port
            | Self::Username
            | Self::Domain
            | Self::Password
            | Self::ShellPath
            | Self::ShellArgs
            | Self::WorkingDir
            | Self::AgentEnvironmentVariable
            | Self::AgentUnixSocket => ConnectionEditorSection::Basic,
            Self::AgentForwardingEnvironmentVariable | Self::AgentForwardingSocketPath => {
                ConnectionEditorSection::Forwarding
            }
            Self::SerialPort | Self::BaudRate => ConnectionEditorSection::Serial,
            Self::PostLoginCommand | Self::PostLoginDelay => ConnectionEditorSection::PostLogin,
            Self::SftpShellDetectionTimeout => ConnectionEditorSection::Sftp,
            Self::TelnetKeepAliveInterval => ConnectionEditorSection::Compatibility,
            Self::TelnetAutoLoginTimeout
            | Self::TelnetAutoLoginUsernamePrompt
            | Self::TelnetAutoLoginPasswordPrompt
            | Self::TelnetAutoLoginSuccessPrompt
            | Self::TelnetAutoLoginFailurePrompt
            | Self::TelnetAutoLoginMaxRetries => ConnectionEditorSection::AutoLogin,
            Self::RdpDisplayWidth | Self::RdpDisplayHeight => ConnectionEditorSection::Display,
            Self::RdpReconnectAttempts | Self::VncReconnectAttempts => {
                ConnectionEditorSection::Session
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ConnectionEditorState {
    /// "Default session settings" mode: the same editor surface minus the
    /// basic-info tab, saving reusable parameters instead of a connection.
    pub(crate) defaults_mode: bool,
    pub(crate) android_usb: bool,
    pub(crate) android_screen: nyaterm_core::models::android::AndroidScreenSettings,
    pub(crate) id: Option<String>,
    pub(crate) kind: ConnectionKindTab,
    pub(crate) section: ConnectionEditorSection,
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
    pub(crate) rdp_backend: String,
    pub(crate) rdp_security: nyaterm_core::RdpSecuritySettings,
    pub(crate) rdp_display: nyaterm_core::RdpDisplaySettings,
    pub(crate) rdp_clipboard: nyaterm_core::RdpClipboardSettings,
    pub(crate) rdp_reconnect: nyaterm_core::RdpReconnectSettings,

    pub(crate) vnc_security: nyaterm_core::VncSecuritySettings,
    pub(crate) vnc_display: nyaterm_core::VncDisplaySettings,
    pub(crate) vnc_clipboard: nyaterm_core::VncClipboardSettings,
    pub(crate) vnc_reconnect: nyaterm_core::VncReconnectSettings,
    pub(crate) vnc_shared: bool,
    pub(crate) vnc_view_only: bool,
    pub(crate) password_source: ConnectionEditorPasswordSource,
    pub(crate) password_id: Option<String>,
    pub(crate) account_id: Option<String>,
    /// SSH and Telnet only. `Account` displays the linked account username.
    pub(crate) username_source: nyaterm_core::ConnectionUsernameSource,
    /// Set once the user picks a password source, so later account changes do not replace it.
    pub(crate) password_source_touched: bool,
    /// The saved-account password stopped being available and the source fell back to asking.
    pub(crate) password_source_demoted: bool,
    pub(crate) field_errors: std::collections::HashMap<ConnectionEditorField, String>,
    pub(crate) password_edit: ConnectionPasswordEdit,
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
    pub(crate) ssh_compatibility_mode: bool,
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
    pub(crate) flow_control: nyaterm_core::models::connection::SerialFlowControl,
    pub(crate) stop_bits: String,
    pub(crate) raw_tcp_cli: bool,
    pub(crate) telnet_enter_mode: String,
    pub(crate) local_echo: bool,
    pub(crate) local_line_edit: bool,
    pub(crate) force_character_at_a_time: bool,
    pub(crate) send_naws: bool,
    pub(crate) send_sga: bool,
    pub(crate) telnet_keep_alive_interval_secs: String,
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

    pub(crate) forwarding_tab: ConnectionEditorForwardingTab,

    pub(crate) connect_after_save: bool,
    pub(crate) focused_field: ConnectionEditorField,
    pub(crate) error: Option<String>,
}

impl ConnectionEditorState {
    /// Compare raw settings, including invalid input, without counting navigation,
    /// validation feedback or per-draft consent as edits.
    pub(crate) fn default_settings_snapshot(&self) -> Self {
        let mut snapshot = self.clone();
        snapshot.id = None;
        snapshot.section = ConnectionEditorSection::Basic;
        snapshot.focused_field = ConnectionEditorField::Name;
        snapshot.error = None;
        snapshot.field_errors.clear();
        snapshot.agent_allow_all_confirmed = false;
        snapshot.agent_forwarding_endpoint_index = 0;
        snapshot.agent_preview = None;
        snapshot.agent_preview_loading = false;
        snapshot.ssh_algorithm_tab = ConnectionEditorSshAlgorithmTab::KeyExchange;
        snapshot.forwarding_tab = ConnectionEditorForwardingTab::Agent;
        snapshot
    }

    pub(crate) fn inherit_new_connection_identity(&mut self, source: &Self) {
        self.name = source.name.clone();
        self.description = source.description.clone();
        self.tags = source.tags.clone();
        self.new_tag = source.new_tag.clone();
        self.icon = source.icon.clone();
        self.icon_auto_detect = source.icon_auto_detect;
        self.group_id = source.group_id.clone();
        self.new_group_name = source.new_group_name.clone();
        self.pending_group_name = source.pending_group_name.clone();
        self.pending_group_parent_id = source.pending_group_parent_id.clone();
        self.host = source.host.clone();
        self.username = source.username.clone();
        let standard_port = match source.kind {
            ConnectionKindTab::Ssh => "22",
            ConnectionKindTab::Telnet => "23",
            ConnectionKindTab::Rdp => "3389",
            ConnectionKindTab::Vnc => "5900",
            ConnectionKindTab::Adb => "5555",
            _ => "",
        };
        if !standard_port.is_empty()
            && !source.port.trim().is_empty()
            && source.port != standard_port
        {
            self.port = source.port.clone();
        }
    }

    /// Tabs the editor navigation offers for this draft.
    pub(crate) fn sections(&self) -> &'static [ConnectionEditorSection] {
        ConnectionEditorSection::for_kind(self.kind, self.defaults_mode)
    }

    pub(crate) fn uses_mstsc_rdp(&self) -> bool {
        self.kind == ConnectionKindTab::Rdp && self.rdp_backend == "mstsc"
    }

    pub(crate) fn uses_windows_rdp(&self) -> bool {
        matches!(self.rdp_backend.as_str(), "windows" | "active-x")
    }

    pub(crate) fn normalize_rdp_backend_settings(&mut self) {
        if self.uses_windows_rdp() {
            self.rdp_security
                .windows_authentication
                .get_or_insert_with(|| "system".into());
        }
    }

    pub(crate) fn select_section(&mut self, section: ConnectionEditorSection) -> bool {
        if !self.sections().contains(&section) {
            return false;
        }
        self.section = section;
        true
    }

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

impl ConnectionEditorState {
    pub(crate) fn password_account_id(&self) -> Option<&str> {
        match self.kind {
            ConnectionKindTab::Ssh | ConnectionKindTab::Telnet => self.account_id.as_deref(),
            ConnectionKindTab::Rdp | ConnectionKindTab::Vnc => self.password_id.as_deref(),
            _ => None,
        }
        .filter(|id| !id.is_empty())
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
    pub(crate) target: ActionLinkTarget,
    pub(crate) mac_format: Option<MacAddressFormat>,
    pub(crate) is_default: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ActionLinkMenuState {
    /// Distinguishes reopened menus even at the same location in the same session.
    pub(crate) generation: u64,
    pub(crate) session_id: String,
    pub(crate) x: Pixels,
    pub(crate) y: Pixels,
    pub(crate) kind_label: String,
    pub(crate) value: String,
    pub(crate) actions: Vec<ActionLinkMenuAction>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ActionLinkTooltipState {
    pub(crate) session_id: String,
    pub(crate) x: Pixels,
    pub(crate) y: Pixels,
    pub(crate) default_action: ActionLinkAction,
    pub(crate) has_more_actions: bool,
    /// Identity key for hover stability (session|kind|value|start|end).
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
    pub(crate) fillable: bool,
    pub(crate) description: String,
    pub(crate) command: String,
    pub(crate) display: String,
    pub(crate) source: String,
    pub(crate) score: u32,
    pub(crate) indices: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CommandSuggestionState {
    /// Previous results may stay visible during typing, but cannot be accepted.
    pub(crate) refreshing: bool,
    pub(crate) mode: nyaterm_core::completion::types::CompletionMode,
    pub(crate) completion_revision: u64,
    pub(crate) scope: Option<nyaterm_core::completion::types::CompletionScope>,
    pub(crate) session_id: String,
    pub(crate) input_version: u64,
    pub(crate) sync_revision: u64,
    pub(crate) draft: String,
    pub(crate) items: Vec<CommandSuggestionItem>,
    pub(crate) selected_index: Option<usize>,
}

impl CommandSuggestionState {
    pub(crate) fn begin_input_refresh(&mut self, text: &str) -> bool {
        let ordinary_edit = (!text.is_empty() && text.chars().all(|ch| !ch.is_control()))
            || matches!(text, "\u{7f}" | "\u{8}" | "\u{1b}[3~");
        let changed =
            self.refreshing != ordinary_edit || (ordinary_edit && self.selected_index.is_some());
        self.refreshing = ordinary_edit;
        if ordinary_edit {
            self.selected_index = None;
        }
        changed
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CredentialSuggestionState {
    pub(crate) session_id: String,
    pub(crate) kind: CredentialPromptKind,
    pub(crate) matches: Vec<CredentialAutofillTarget>,
    pub(crate) prompt_text: String,
    /// Only a unique precise match is preselected. Ambiguous matches and the
    /// unfiltered fallback require an explicit choice before Enter can fill.
    pub(crate) selected_index: Option<usize>,
}

use nyaterm_core::credential_autofill::matching::CredentialAutofillMatcher;
pub(crate) use nyaterm_core::credential_autofill::matching::{
    ConnectionPasswordTarget, CredentialAutofillAction, CredentialAutofillMatchEvent,
    CredentialAutofillMatchRequest, CredentialAutofillMatchRequestKey, CredentialAutofillTarget,
    PendingCredentialAutofill, credential_autofill_action,
};

pub(crate) fn credential_autofill_target_display_name(target: &CredentialAutofillTarget) -> String {
    match target {
        CredentialAutofillTarget::Vault(credential) => credential.name.clone(),
        CredentialAutofillTarget::ConnectionPassword(candidate) => format!(
            "{} ({})",
            candidate.connection_name,
            t!("credentialAutofill.connectionPasswordLabel")
        ),
    }
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
    let mut matcher = CredentialAutofillMatcher::default();
    while let Ok(request) = command_rx.recv() {
        let event = CredentialAutofillMatchEvent {
            key: request.key.clone(),
            outcome: matcher.evaluate(request),
        };
        event_queue.push(event);
    }
}

const CREDENTIAL_AUTOFILL_MATCH_EVENT_CAP: usize = 128;

#[cfg(test)]
mod credential_autofill_match_tests {
    use super::{
        CredentialAutofillMatchEvent, CredentialAutofillMatchEventQueue,
        CredentialAutofillMatchRequestKey,
    };
    use crate::models::event_wake::EventWake;
    use nyaterm_core::credential_autofill::matching::CredentialAutofillMatchOutcome;
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
