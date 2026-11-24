pub(crate) mod spice;

use futures::channel::mpsc::UnboundedReceiver;
use gpui::Pixels;
use nyaterm_core::CredentialPromptKind;
use nyaterm_core::mac_address::MacAddressFormat;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use crate::action_links::{ActionLinkAction, ActionLinkTarget};

use super::event_wake::{ANY_INTEREST, EventWake};
pub(crate) use nyaterm_connections_gpui::models::{ConnectionEditorSection, ConnectionKindTab};

/// Logical drive letters currently present on this machine (Windows only).
/// Shared by the drive picker and its toggle handler so an explicit selection
/// never contains drives the user cannot see in the editor.
pub(crate) fn local_drive_letters() -> Vec<char> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::GetLogicalDrives;
        let mask = unsafe { GetLogicalDrives() };
        ('A'..='Z')
            .filter(|letter| {
                let index = u32::from(*letter as u8 - b'A');
                mask & (1 << index) != 0
            })
            .collect()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
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
    FtpPassiveMode,
    FtpFilenameEncodingMode,
    FtpFilenameEncoding,
    FtpSecurityMode,
    FtpTlsVersion,
    SftpCwdFollowMode,
    SftpFilenameEncoding,
    SftpPipelineDepth,
    SshAlgorithmMode,
    SshProfile,
    SshTerminalType,
    RdpBackend,
    RdpCertificatePolicy,
    RdpDisplayMode,
    RdpAudio,
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
    RdpCustomDrivePath,
    RdpReconnectAttempts,
    VncReconnectAttempts,
    SpiceTcpPort,
    SpiceTlsPort,
    SpiceProxy,
    SpiceCaPem,
    SpiceCertificateSubject,
    SpiceReconnectAttempts,
    FtpInitialDirectory,
    FtpLocalInitialDirectory,
    FtpConnectTimeout,
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
            | Self::SerialPort
            | Self::BaudRate
            | Self::AgentEnvironmentVariable
            | Self::AgentUnixSocket => ConnectionEditorSection::Basic,
            Self::AgentForwardingEnvironmentVariable | Self::AgentForwardingSocketPath => {
                ConnectionEditorSection::Forwarding
            }
            Self::PostLoginCommand | Self::PostLoginDelay => ConnectionEditorSection::PostLogin,
            Self::SftpShellDetectionTimeout => ConnectionEditorSection::Sftp,
            Self::TelnetKeepAliveInterval => ConnectionEditorSection::Compatibility,
            Self::TelnetAutoLoginTimeout
            | Self::TelnetAutoLoginUsernamePrompt
            | Self::TelnetAutoLoginPasswordPrompt
            | Self::TelnetAutoLoginSuccessPrompt
            | Self::TelnetAutoLoginFailurePrompt
            | Self::TelnetAutoLoginMaxRetries => ConnectionEditorSection::AutoLogin,
            Self::SpiceTcpPort | Self::SpiceTlsPort => ConnectionEditorSection::Basic,
            Self::SpiceProxy => ConnectionEditorSection::Network,
            Self::SpiceCaPem | Self::SpiceCertificateSubject => ConnectionEditorSection::Security,
            Self::SpiceReconnectAttempts => ConnectionEditorSection::Session,
            Self::RdpDisplayWidth | Self::RdpDisplayHeight => ConnectionEditorSection::Display,
            Self::RdpCustomDrivePath | Self::RdpReconnectAttempts | Self::VncReconnectAttempts => {
                ConnectionEditorSection::Session
            }
            Self::FtpInitialDirectory
            | Self::FtpLocalInitialDirectory
            | Self::FtpConnectTimeout => ConnectionEditorSection::Advanced,
        }
    }
}

/// Dropdown value that stands for "ask for the device when connecting" rather
/// than a port name. It is never stored in `serial_port`, which keeps the last
/// used device as the connect-time dialog's default.
pub(crate) const SERIAL_PORT_ASK_AT_CONNECT: &str = "__ask_at_connect__";

/// Title a new serial connection takes from its device.
///
/// The value can come from the enumerated list (`COM7`, `/dev/ttyUSB0`) or be
/// typed by hand, where Windows also accepts a `\\.\COM10` prefix and Linux
/// users often paste a `/dev/serial/by-id/...` path. Only the part that names
/// the device is kept, so a pasted path does not turn into a title as long as
/// the path itself. `None` means there is no device to name the connection
/// after: nothing selected, or "ask when connecting" chosen.
pub(crate) fn serial_connection_title(port_name: &str) -> Option<String> {
    let port_name = port_name.trim();
    if port_name.is_empty() || port_name == SERIAL_PORT_ASK_AT_CONNECT {
        return None;
    }
    // Windows needs the prefix to address COM10 and above, but it is not part
    // of the device's name.
    let port_name = port_name.strip_prefix(r"\\.\").unwrap_or(port_name);
    let device = port_name
        .rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())?;
    Some(device.to_string())
}

/// Title a new serial connection takes from its device and framing.
///
/// This is the same shape a quick serial link gets (`COM7 @ 115200`), so a saved
/// connection and a temporary one are named alike. The rate joins the title
/// because one board is often opened at several of them; without a rate only the
/// device is left.
pub(crate) fn serial_connection_name(port_name: &str, baud_rate: &str) -> Option<String> {
    let device = serial_connection_title(port_name)?;
    let baud_rate = baud_rate.trim();
    if baud_rate.is_empty() {
        return Some(device);
    }
    Some(format!("{device} @ {baud_rate}"))
}

/// Name a serial connection is saved under when it has no device to speak of.
///
/// The connect prompt can own the device, leaving the editor without one; the
/// connection list still needs something to show.
pub(crate) const SERIAL_FALLBACK_NAME: &str = "Serial";

/// Whether a name is the one a device and rate produce rather than one the user
/// typed.
///
/// The device alone counts: names written before the rate joined the title, and
/// connections imported with only a device, are the editor's default too. An
/// empty name is nobody's choice yet, and neither is the bare ["Serial"] of a
/// connection saved without a device, so both count as the device's as well.
pub(crate) fn serial_name_restates_device(name: &str, port_name: &str, baud_rate: u32) -> bool {
    let name = name.trim();
    if name.is_empty() || name.eq_ignore_ascii_case(SERIAL_FALLBACK_NAME) {
        return true;
    }
    serial_connection_name(port_name, &baud_rate.to_string()).as_deref() == Some(name)
        || serial_connection_title(port_name).as_deref() == Some(name)
}

/// Title a serial tab carries for the device and rate it actually opens.
///
/// A name the user typed is kept verbatim. A name that only restated the device
/// saved with the connection is the editor's default, so the tab follows the
/// device this session really opened — which matters when the connect prompt
/// asked: the same saved connection serves the `COM7` it was saved from and the
/// `COM9` the board reappeared as, and the tab says which one is open.
pub(crate) fn serial_session_title(
    saved_name: &str,
    saved_port_name: &str,
    saved_baud_rate: u32,
    port_name: &str,
    baud_rate: u32,
) -> String {
    if !serial_name_restates_device(saved_name, saved_port_name, saved_baud_rate) {
        return saved_name.to_string();
    }
    serial_connection_name(port_name, &baud_rate.to_string())
        .unwrap_or_else(|| saved_name.to_string())
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
    pub(crate) ftp_initial_directory: String,
    pub(crate) ftp_local_initial_directory: String,
    pub(crate) ftp_passive_mode: String,
    pub(crate) ftp_filename_encoding_mode: String,
    pub(crate) ftp_filename_encoding: String,
    pub(crate) ftp_ascii_mode: bool,
    pub(crate) ftp_security_mode: String,
    pub(crate) ftp_tls_version: String,
    pub(crate) ftp_connect_timeout_ms: String,
    pub(crate) rdp_backend: String,
    pub(crate) rdp_security: nyaterm_core::RdpSecuritySettings,
    pub(crate) rdp_display: nyaterm_core::RdpDisplaySettings,
    pub(crate) rdp_clipboard: nyaterm_core::RdpClipboardSettings,
    pub(crate) rdp_redirection: nyaterm_core::RdpRedirectionSettings,
    pub(crate) rdp_reconnect: nyaterm_core::RdpReconnectSettings,

    pub(crate) spice: spice::SpiceEditorDraft,
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
    pub(crate) allow_short_rsa_host_keys: bool,
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
    /// Ask for the device when a serial session starts. The saved port becomes a
    /// hint and may stay empty.
    pub(crate) serial_prompt_port: bool,
    /// Ask for the baud rate when a serial session starts. The saved rate becomes
    /// the dialog default.
    pub(crate) serial_prompt_baud_rate: bool,
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
            ConnectionKindTab::Spice => "5901",
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
            ConnectionKindTab::Rdp | ConnectionKindTab::Vnc | ConnectionKindTab::Spice => {
                self.password_id.as_deref()
            }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CredentialSuggestionState {
    pub(crate) session_id: String,
    pub(crate) kind: CredentialPromptKind,
    pub(crate) matches: Vec<CredentialAutofillTarget>,
    pub(crate) prompt_text: String,
    /// Only a unique match is preselected; ambiguous matches require an
    /// explicit choice before Enter can fill.
    pub(crate) selected_index: Option<usize>,
}

use nyaterm_core::credential_autofill::matching::CredentialAutofillMatcher;
pub(crate) use nyaterm_core::credential_autofill::matching::{
    CredentialAutofillAction, CredentialAutofillMatchEvent, CredentialAutofillMatchRequest,
    CredentialAutofillMatchRequestKey, CredentialAutofillTarget, OtpPromptTarget,
    PendingCredentialAutofill, credential_autofill_action,
};

pub(crate) fn credential_autofill_target_display_name(target: &CredentialAutofillTarget) -> String {
    match target {
        CredentialAutofillTarget::Vault(credential) => credential.name.clone(),
        CredentialAutofillTarget::Otp(otp) => {
            if otp.issuer.trim().is_empty() {
                otp.username.clone()
            } else {
                otp.issuer.clone()
            }
        }
    }
}

/// Second line of a suggestion row: the account the entry belongs to.
pub(crate) fn credential_autofill_target_subtitle(target: &CredentialAutofillTarget) -> &str {
    match target {
        CredentialAutofillTarget::Vault(credential) => &credential.username,
        CredentialAutofillTarget::Otp(otp) => &otp.username,
    }
}

/// An OTP entry only joins the suggestion pipeline when it opts in with a
/// prompt pattern; without one it stays available through its connection bind.
pub(crate) fn otp_prompt_target(
    entry: &nyaterm_core::OtpEntry,
) -> Option<CredentialAutofillTarget> {
    let pattern = entry.prompt_regex.as_deref().unwrap_or_default().trim();
    if pattern.is_empty() {
        return None;
    }
    Some(CredentialAutofillTarget::Otp(OtpPromptTarget {
        id: entry.id.clone(),
        issuer: entry.issuer.clone(),
        username: entry.username.clone(),
        prompt_regex: pattern.to_string(),
    }))
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

#[cfg(test)]
mod tests {
    use super::{
        SERIAL_PORT_ASK_AT_CONNECT, serial_connection_name, serial_connection_title,
        serial_name_restates_device, serial_session_title,
    };

    #[test]
    fn serial_settings_stay_in_basic_information_except_in_defaults() {
        use super::{ConnectionEditorField, ConnectionEditorSection, ConnectionKindTab};
        assert_eq!(
            ConnectionEditorSection::for_kind(ConnectionKindTab::Serial, false),
            &[
                ConnectionEditorSection::Basic,
                ConnectionEditorSection::Terminal
            ]
        );
        assert_eq!(
            ConnectionEditorSection::for_kind(ConnectionKindTab::Serial, true),
            &[
                ConnectionEditorSection::Serial,
                ConnectionEditorSection::Terminal
            ]
        );
        assert_eq!(
            ConnectionEditorSection::initial_for_kind(ConnectionKindTab::Serial, false),
            ConnectionEditorSection::Basic
        );
        assert_eq!(
            ConnectionEditorSection::initial_for_kind(ConnectionKindTab::Serial, true),
            ConnectionEditorSection::Serial
        );
        assert_eq!(
            ConnectionEditorField::SerialPort.section(),
            ConnectionEditorSection::Basic
        );
        assert_eq!(
            ConnectionEditorField::BaudRate.section(),
            ConnectionEditorSection::Basic
        );
    }

    /// FTP exposes charset and transfer-mode controls without a network page.
    #[test]
    fn ftp_sections_omit_the_network_page() {
        use super::{ConnectionEditorSection, ConnectionKindTab};
        assert_eq!(
            ConnectionEditorSection::for_kind(ConnectionKindTab::Ftp, false),
            &[
                ConnectionEditorSection::Basic,
                ConnectionEditorSection::Charset,
                ConnectionEditorSection::Advanced,
            ]
        );
        assert_eq!(
            ConnectionEditorSection::for_kind(ConnectionKindTab::Ftp, true),
            &[
                ConnectionEditorSection::Charset,
                ConnectionEditorSection::Advanced,
            ]
        );
    }

    /// The title names the device, wherever the device name came from.
    #[test]
    fn a_device_names_the_connection_not_the_path_that_reaches_it() {
        for (port_name, expected) in [
            ("COM7", "COM7"),
            ("  COM7  ", "COM7"),
            // Windows needs the prefix to open COM10 and above.
            (r"\\.\COM10", "COM10"),
            ("/dev/ttyUSB0", "ttyUSB0"),
            ("/dev/ttyACM0", "ttyACM0"),
            (
                "/dev/serial/by-id/usb-FTDI_FT232R_USB_UART_A50285BI-if00-port0",
                "usb-FTDI_FT232R_USB_UART_A50285BI-if00-port0",
            ),
        ] {
            assert_eq!(
                serial_connection_title(port_name).as_deref(),
                Some(expected),
                "{port_name}"
            );
        }
    }

    /// Without a device there is nothing to name the connection after.
    #[test]
    fn an_unpicked_device_has_no_title() {
        assert_eq!(serial_connection_title(""), None);
        assert_eq!(serial_connection_title("   "), None);
        assert_eq!(serial_connection_title(SERIAL_PORT_ASK_AT_CONNECT), None);
    }

    /// The rate joins the device, which is how a quick serial link is titled.
    #[test]
    fn the_rate_joins_the_device_in_the_title() {
        assert_eq!(
            serial_connection_name("COM7", "115200").as_deref(),
            Some("COM7 @ 115200")
        );
        assert_eq!(
            serial_connection_name(r"\.\COM10", " 921600 ").as_deref(),
            Some("COM10 @ 921600")
        );
        assert_eq!(
            serial_connection_name("/dev/serial/by-id/usb-FTDI-if00-port0", "57600").as_deref(),
            Some("usb-FTDI-if00-port0 @ 57600")
        );
        // A missing rate leaves the device on its own rather than a bare " @ ".
        assert_eq!(
            serial_connection_name("COM7", "  ").as_deref(),
            Some("COM7")
        );
        // Without a device there is nothing to name, however precise the rate.
        assert_eq!(serial_connection_name("", "115200"), None);
        assert_eq!(
            serial_connection_name(SERIAL_PORT_ASK_AT_CONNECT, "115200"),
            None
        );
    }

    /// Only the device's own wording follows the device; anything the user typed
    /// is theirs to keep.
    #[test]
    fn a_name_tells_the_devices_wording_apart_from_the_users() {
        assert!(serial_name_restates_device("COM7 @ 115200", "COM7", 115200));
        // Named after the device alone, which older titles did.
        assert!(serial_name_restates_device("COM7", "COM7", 115200));
        // Nothing typed yet, and the bare "Serial" a device-less save leaves.
        assert!(serial_name_restates_device("   ", "COM7", 115200));
        assert!(serial_name_restates_device("Serial", "", 115200));
        assert!(serial_name_restates_device("serial", "", 115200));
        assert!(!serial_name_restates_device(
            "Stepper bench",
            "COM7",
            115200
        ));
        // Anything else written over the device's wording is a choice of words.
        assert!(!serial_name_restates_device("Serial bench", "", 115200));
        // A rate the connection does not open at is not the device's wording.
        assert!(!serial_name_restates_device("COM7 @ 9600", "COM7", 115200));
        assert!(!serial_name_restates_device(
            "COM7 @ 115200",
            "COM9",
            115200
        ));
    }

    /// The tab names the device it really opened, unless the user chose words.
    #[test]
    fn a_serial_tab_follows_the_device_it_opened() {
        assert_eq!(
            serial_session_title("COM7 @ 115200", "COM7", 115200, "COM9", 57600),
            "COM9 @ 57600"
        );
        assert_eq!(
            serial_session_title("COM7", "COM7", 115200, "/dev/ttyACM0", 115200),
            "ttyACM0 @ 115200"
        );
        assert_eq!(
            serial_session_title("", "COM7", 115200, "COM9", 115200),
            "COM9 @ 115200"
        );
        // The bare "Serial" of a connection saved without a device is the
        // device's to name too, which is the connect prompt's job then.
        assert_eq!(
            serial_session_title("Serial", "", 115200, "COM9", 115200),
            "COM9 @ 115200"
        );
        // A name of the user's own is not the tab's to rewrite.
        assert_eq!(
            serial_session_title("Stepper bench", "COM7", 115200, "COM9", 57600),
            "Stepper bench"
        );
        // Without a device to name it after, the saved name is all there is.
        assert_eq!(
            serial_session_title("COM7 @ 115200", "COM7", 115200, "", 115200),
            "COM7 @ 115200"
        );
    }

    #[test]
    fn otp_entries_join_the_suggestion_pipeline_only_with_a_pattern() {
        use super::{
            CredentialAutofillTarget, credential_autofill_target_display_name,
            credential_autofill_target_subtitle, otp_prompt_target,
        };
        use nyaterm_core::{OtpEntry, SecretString};

        let entry = |pattern: Option<&str>| OtpEntry {
            id: "otp-1".into(),
            otp_type: "totp".into(),
            issuer: "GitHub".into(),
            username: "octocat".into(),
            prompt_regex: pattern.map(str::to_string),
            secret: Some(SecretString::from("secret")),
            algorithm: "SHA1".into(),
            digits: 6,
            period: 30,
            counter: 0,
            has_secret: true,
        };

        assert!(otp_prompt_target(&entry(None)).is_none());
        assert!(otp_prompt_target(&entry(Some("   "))).is_none());

        let target = otp_prompt_target(&entry(Some(" Verification code: "))).expect("opted in");
        assert_eq!(target.id(), "otp-1");
        assert!(target.is_otp());
        assert!(matches!(target, CredentialAutofillTarget::Otp(_)));
        assert_eq!(credential_autofill_target_display_name(&target), "GitHub");
        assert_eq!(credential_autofill_target_subtitle(&target), "octocat");
    }
}
