//! Batch editing of connection-level fields for the selected assets.
//!
//! The draft mirrors the current value of each field across the selection:
//! a uniform value is shown as-is, a mixed value is shown as the "keep
//! unchanged" placeholder, and an empty value is shown empty. A field only
//! becomes part of the write patch once the user edits it, which is what makes
//! "edited to empty" mean clear while "never touched" means keep. Applying the
//! draft is delegated to `nyaterm_core::apply_connection_batch_edit`, which
//! writes only the touched fields and leaves everything else, including unknown
//! persisted fields, untouched.

mod view;
mod window;

use std::collections::{HashMap, HashSet};

use gpui::{Context, ScrollHandle, SharedString, Window};
use nyaterm_core::{
    BatchField, ConnectionBatchEdit, ConnectionBatchKind, ConnectionType,
    MAX_SFTP_SHELL_DETECTION_TIMEOUT_MS, MIN_SFTP_SHELL_DETECTION_TIMEOUT_MS, RecordingMode,
    SavedConnection, SecretString, SerialFlowControl, SftpCwdFollowMode, SshAgentEndpoint,
    SshAgentForwardingPolicy, SshAlgorithmMode, SshProfile, SshTerminalType,
    apply_connection_batch_edit, validate_connection_batch_edit,
};
use nyaterm_store::StoreDomain;
use nyaterm_ui::ChildWindowSlot;
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::text_inputs::TextInputSetup;
use crate::models::{ConnectionEditorSection, ConnectionKindTab};

pub(in crate::features) const ASSET_BATCH_SELECT_PREFIX: &str = "asset-batch.select.";
pub(in crate::features) const ASSET_BATCH_TEXT_PREFIX: &str = "asset-batch.";
pub(in crate::features) const ASSET_BATCH_BOOL_PREFIX: &str = "asset-batch.bool.";

/// What the selected connections currently hold for one field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::features) enum FieldBasis {
    /// No selected connection supports this field (Local/Serial, non-SSH, ...).
    #[default]
    Inapplicable,
    /// Selected connections differ; untouched means keep, edited means write.
    Mixed,
    /// Selected connections agree; the draft holds that value.
    Uniform,
}

/// Every field the batch editor exposes. Keys double as widget id suffixes.
///
/// The first block is editable and participates in the write patch. The second
/// block mirrors fields that exist in the connection editor but must stay
/// disabled here: they are rendered in place, never patched, and never receive
/// a widget id that could be focused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::features) enum BatchFieldId {
    Group,
    Description,
    Icon,
    IconAutoDetect,
    Port,
    Username,
    Encoding,
    Backspace,
    SshProfile,
    TerminalType,
    DynamicTabTitle,
    SftpEnabled,
    SftpCompatibility,
    SftpCwdFollow,
    SftpShellTimeout,
    SftpFilenameEncoding,
    SftpPipelineDepth,
    X11,
    AgentForwarding,
    PostLoginEnabled,
    PostLoginCommand,
    PostLoginDelay,
    SshAlgorithmMode,
    SshKex,
    SshCiphers,
    SshMacs,
    SshHostKeys,
    RdpUseNla,
    RdpCertificatePolicy,
    RdpAdminSession,
    VncSecurityMode,

    Name,
    Host,
    Tags,
    Account,
    Password,
    AuthMode,
    Otp,
    OtpAutoFill,
    Key,
    SshAuthAgentEndpoint,
    SshAuthAgentEnvironment,
    SshAuthAgentSocket,
    Proxy,
    ProxyJump,
    ShellPath,
    ShellArgs,
    WorkingDir,
    SerialPort,
    BaudRate,
    DataBits,
    Parity,
    StopBits,
    FlowControl,
    TelnetRawTcp,
    TelnetKeepAlive,
    TelnetLocalEcho,
    TelnetLocalLineEdit,
    TelnetForceChar,
    TelnetSendNaws,
    TelnetSendSga,
    TelnetEnterMode,
    TelnetAutoLoginEnabled,
    TelnetAutoLoginWake,
    TelnetAutoLoginTimeout,
    TelnetAutoLoginRetries,
    TelnetAutoLoginUserPrompt,
    TelnetAutoLoginPasswordPrompt,
    TelnetAutoLoginSuccessPrompt,
    TelnetAutoLoginFailurePrompt,
    RdpDomain,
    RdpBackend,
    RdpDisplayMode,
    RdpWidth,
    RdpHeight,
    RdpWallpaper,
    RdpThemes,
    RdpFontSmoothing,
    RdpDesktopComposition,
    RdpDisableFullWindowDrag,
    RdpDisableMenuAnimations,
    RdpDisableCursorShadow,
    RdpDisableCursorBlinking,
    RdpAutoReconnect,
    RdpReconnectAttempts,
    VncScaleMode,
    VncClipboard,
    VncShared,
    VncViewOnly,
    VncReconnect,
    VncReconnectAttempts,
    RecordingUseGlobal,
    RecordingAutoStart,
    RecordingMode,
    AgentExternal,
    AgentStoredKeys,
    AgentPolicy,
    AgentEndpoints,
}

impl BatchFieldId {
    pub(in crate::features) fn key(self) -> &'static str {
        match self {
            Self::Group => "group",
            Self::Description => "description",
            Self::Icon => "icon",
            Self::IconAutoDetect => "icon-auto-detect",
            Self::Port => "port",
            Self::Username => "username",
            Self::Encoding => "encoding",
            Self::Backspace => "backspace",
            Self::SshProfile => "ssh-profile",
            Self::TerminalType => "terminal-type",
            Self::DynamicTabTitle => "dynamic-tab-title",
            Self::SftpEnabled => "sftp-enabled",
            Self::SftpCompatibility => "sftp-compatibility",
            Self::SftpCwdFollow => "sftp-cwd-follow",
            Self::SftpShellTimeout => "sftp-shell-timeout",
            Self::SftpFilenameEncoding => "sftp-filename-encoding",
            Self::SftpPipelineDepth => "sftp-pipeline-depth",
            Self::X11 => "x11",
            Self::AgentForwarding => "agent-forwarding",
            Self::PostLoginEnabled => "post-login-enabled",
            Self::PostLoginCommand => "post-login-command",
            Self::PostLoginDelay => "post-login-delay",
            Self::SshAlgorithmMode => "ssh-algorithm-mode",
            Self::SshKex => "ssh-kex",
            Self::SshCiphers => "ssh-ciphers",
            Self::SshMacs => "ssh-macs",
            Self::SshHostKeys => "ssh-host-keys",
            Self::RdpUseNla => "rdp-use-nla",
            Self::RdpCertificatePolicy => "rdp-certificate-policy",
            Self::RdpAdminSession => "rdp-admin-session",
            Self::VncSecurityMode => "vnc-security-mode",
            Self::Name => "display.name",
            Self::Host => "display.host",
            Self::Tags => "display.tags",
            Self::Account => "display.account",
            Self::Password => "display.password",
            Self::AuthMode => "display.auth-mode",
            Self::Otp => "display.otp",
            Self::OtpAutoFill => "display.otp-auto-fill",
            Self::Key => "display.key",
            Self::SshAuthAgentEndpoint => "display.ssh-auth-agent-endpoint",
            Self::SshAuthAgentEnvironment => "display.ssh-auth-agent-environment",
            Self::SshAuthAgentSocket => "display.ssh-auth-agent-socket",
            Self::Proxy => "display.proxy",
            Self::ProxyJump => "display.proxy-jump",
            Self::ShellPath => "display.shell-path",
            Self::ShellArgs => "display.shell-args",
            Self::WorkingDir => "display.working-dir",
            Self::SerialPort => "display.serial-port",
            Self::BaudRate => "display.baud-rate",
            Self::DataBits => "display.data-bits",
            Self::Parity => "display.parity",
            Self::StopBits => "display.stop-bits",
            Self::FlowControl => "display.flow-control",
            Self::TelnetRawTcp => "display.telnet-raw-tcp",
            Self::TelnetKeepAlive => "display.telnet-keep-alive",
            Self::TelnetLocalEcho => "display.telnet-local-echo",
            Self::TelnetLocalLineEdit => "display.telnet-local-line-edit",
            Self::TelnetForceChar => "display.telnet-force-char",
            Self::TelnetSendNaws => "display.telnet-send-naws",
            Self::TelnetSendSga => "display.telnet-send-sga",
            Self::TelnetEnterMode => "display.telnet-enter-mode",
            Self::TelnetAutoLoginEnabled => "display.telnet-auto-login-enabled",
            Self::TelnetAutoLoginWake => "display.telnet-auto-login-wake",
            Self::TelnetAutoLoginTimeout => "display.telnet-auto-login-timeout",
            Self::TelnetAutoLoginRetries => "display.telnet-auto-login-retries",
            Self::TelnetAutoLoginUserPrompt => "display.telnet-auto-login-user-prompt",
            Self::TelnetAutoLoginPasswordPrompt => "display.telnet-auto-login-password-prompt",
            Self::TelnetAutoLoginSuccessPrompt => "display.telnet-auto-login-success-prompt",
            Self::TelnetAutoLoginFailurePrompt => "display.telnet-auto-login-failure-prompt",
            Self::RdpDomain => "display.rdp-domain",
            Self::RdpBackend => "display.rdp-backend",
            Self::RdpDisplayMode => "display.rdp-display-mode",
            Self::RdpWidth => "display.rdp-width",
            Self::RdpHeight => "display.rdp-height",
            Self::RdpWallpaper => "display.rdp-wallpaper",
            Self::RdpThemes => "display.rdp-themes",
            Self::RdpFontSmoothing => "display.rdp-font-smoothing",
            Self::RdpDesktopComposition => "display.rdp-desktop-composition",
            Self::RdpDisableFullWindowDrag => "display.rdp-disable-full-window-drag",
            Self::RdpDisableMenuAnimations => "display.rdp-disable-menu-animations",
            Self::RdpDisableCursorShadow => "display.rdp-disable-cursor-shadow",
            Self::RdpDisableCursorBlinking => "display.rdp-disable-cursor-blinking",
            Self::RdpAutoReconnect => "display.rdp-auto-reconnect",
            Self::RdpReconnectAttempts => "display.rdp-reconnect-attempts",
            Self::VncScaleMode => "display.vnc-scale-mode",
            Self::VncClipboard => "display.vnc-clipboard",
            Self::VncShared => "display.vnc-shared",
            Self::VncViewOnly => "display.vnc-view-only",
            Self::VncReconnect => "display.vnc-reconnect",
            Self::VncReconnectAttempts => "display.vnc-reconnect-attempts",
            Self::RecordingUseGlobal => "display.recording-use-global",
            Self::RecordingAutoStart => "display.recording-auto-start",
            Self::RecordingMode => "display.recording-mode",
            Self::AgentExternal => "display.agent-external",
            Self::AgentStoredKeys => "display.agent-stored-keys",
            Self::AgentPolicy => "display.agent-policy",
            Self::AgentEndpoints => "display.agent-endpoints",
        }
    }

    pub(in crate::features) fn from_key(key: &str) -> Option<Self> {
        EDITABLE_FIELDS
            .iter()
            .copied()
            .find(|field| field.key() == key)
    }

    /// Fields the batch editor can write. Everything else in the enum is
    /// rendered for position parity with the connection editor and is always
    /// disabled, so it must never produce a widget id or a patch.
    pub(in crate::features) fn is_editable(self) -> bool {
        EDITABLE_FIELDS.contains(&self)
    }

    pub(in crate::features) fn label_key(self) -> &'static str {
        match self {
            Self::Group => "dialog.group",
            Self::Description => "dialog.description",
            Self::Icon => "dialog.icon",
            Self::IconAutoDetect => "dialog.iconAutoDetect",
            Self::Port => "dialog.port",
            Self::Username => "dialog.username",
            Self::Encoding => "dialog.encoding",
            Self::Backspace => "dialog.backspaceMode",
            Self::SshProfile => "dialog.sshProfile",
            Self::TerminalType => "dialog.sshTerminalType",
            Self::DynamicTabTitle => "dialog.dynamicTabTitle",
            Self::SftpEnabled => "dialog.enabled",
            Self::SftpCompatibility => "dialog.sftpCompatibilityMode",
            Self::SftpCwdFollow => "dialog.sftpCwdFollowMode",
            Self::SftpShellTimeout => "dialog.sftpShellDetectionTimeout",
            Self::SftpFilenameEncoding => "dialog.sftpFilenameEncoding",
            Self::SftpPipelineDepth => "dialog.sftpPipelineDepth",
            Self::X11 => "dialog.x11Forwarding",
            Self::AgentForwarding => "dialog.sshAgentForwarding",
            Self::PostLoginEnabled => "dialog.enabled",
            Self::PostLoginCommand => "dialog.postLoginCommandContent",
            Self::PostLoginDelay => "dialog.postLoginDelay",
            Self::SshAlgorithmMode => "dialog.algorithmMode",
            Self::SshKex => "dialog.algorithmKexTab",
            Self::SshCiphers => "dialog.algorithmCiphersTab",
            Self::SshMacs => "dialog.algorithmMacsTab",
            Self::SshHostKeys => "dialog.algorithmHostKeysTab",
            Self::RdpUseNla => "dialog.rdpUseNla",
            Self::RdpCertificatePolicy => "dialog.rdpCertificatePolicy",
            Self::RdpAdminSession => "dialog.rdpAdministrativeSession",
            Self::VncSecurityMode => "dialog.vncSecurityMode",
            Self::Name => "dialog.connectionName",
            Self::Host => "dialog.host",
            Self::Tags => "dialog.tags",
            Self::Account => "dialog.account",
            Self::Password => "dialog.password",
            Self::AuthMode => "dialog.authentication",
            Self::Otp => "dialog.selectOtp",
            Self::OtpAutoFill => "dialog.autoFillOtp",
            Self::Key => "dialog.privateKey",
            Self::SshAuthAgentEndpoint => "dialog.sshAgentEndpoint",
            Self::SshAuthAgentEnvironment => "dialog.sshAgentEnvironmentVariable",
            Self::SshAuthAgentSocket => "dialog.sshAgentSocketPath",
            Self::Proxy => "dialog.proxySelect",
            Self::ProxyJump => "dialog.selectProxyJump",
            Self::ShellPath => "dialog.shellPath",
            Self::ShellArgs => "dialog.shellArgs",
            Self::WorkingDir => "dialog.workingDir",
            Self::SerialPort => "dialog.serialPort",
            Self::BaudRate => "dialog.baudRate",
            Self::DataBits => "dialog.dataBits",
            Self::Parity => "dialog.parity",
            Self::StopBits => "dialog.stopBits",
            Self::FlowControl => "dialog.serialFlowControl",
            Self::TelnetRawTcp => "dialog.telnetRawTcpCli",
            Self::TelnetKeepAlive => "dialog.telnetKeepAliveInterval",
            Self::TelnetLocalEcho => "dialog.telnetLocalEcho",
            Self::TelnetLocalLineEdit => "dialog.telnetLocalLineEdit",
            Self::TelnetForceChar => "dialog.telnetForceCharAtATime",
            Self::TelnetSendNaws => "dialog.telnetSendNaws",
            Self::TelnetSendSga => "dialog.telnetSendSga",
            Self::TelnetEnterMode => "dialog.telnetEnterMode",
            Self::TelnetAutoLoginEnabled => "dialog.enabled",
            Self::TelnetAutoLoginWake => "dialog.telnetAutoLoginWakeEnter",
            Self::TelnetAutoLoginTimeout => "dialog.telnetAutoLoginTimeout",
            Self::TelnetAutoLoginRetries => "dialog.telnetAutoLoginMaxRetries",
            Self::TelnetAutoLoginUserPrompt => "dialog.telnetAutoLoginUsernamePrompt",
            Self::TelnetAutoLoginPasswordPrompt => "dialog.telnetAutoLoginPasswordPrompt",
            Self::TelnetAutoLoginSuccessPrompt => "dialog.telnetAutoLoginSuccessPrompt",
            Self::TelnetAutoLoginFailurePrompt => "dialog.telnetAutoLoginFailurePrompt",
            Self::RdpDomain => "dialog.rdpDomain",
            Self::RdpBackend => "dialog.rdpBackend",
            Self::RdpDisplayMode => "dialog.rdpDisplayMode",
            Self::RdpWidth => "dialog.rdpWidth",
            Self::RdpHeight => "dialog.rdpHeight",
            Self::RdpWallpaper => "dialog.rdpWallpaper",
            Self::RdpThemes => "dialog.rdpThemes",
            Self::RdpFontSmoothing => "dialog.rdpFontSmoothing",
            Self::RdpDesktopComposition => "dialog.rdpDesktopComposition",
            Self::RdpDisableFullWindowDrag => "dialog.rdpDisableFullWindowDrag",
            Self::RdpDisableMenuAnimations => "dialog.rdpDisableMenuAnimations",
            Self::RdpDisableCursorShadow => "dialog.rdpDisableCursorShadow",
            Self::RdpDisableCursorBlinking => "dialog.rdpDisableCursorBlinking",
            Self::RdpAutoReconnect => "dialog.rdpAutoReconnect",
            Self::RdpReconnectAttempts => "dialog.rdpReconnectAttempts",
            Self::VncScaleMode => "dialog.vncScaleMode",
            Self::VncClipboard => "dialog.vncClipboard",
            Self::VncShared => "dialog.vncSharedSession",
            Self::VncViewOnly => "dialog.vncViewOnly",
            Self::VncReconnect => "dialog.vncReconnect",
            Self::VncReconnectAttempts => "dialog.vncReconnectAttempts",
            Self::RecordingUseGlobal => "dialog.recordingUseGlobal",
            Self::RecordingAutoStart => "dialog.recordingAutoStart",
            Self::RecordingMode => "dialog.recordingMode",
            Self::AgentExternal => "dialog.sshAgentExternalSource",
            Self::AgentStoredKeys => "dialog.sshAgentStoredKeysSource",
            Self::AgentPolicy => "dialog.sshAgentForwardingPolicy",
            Self::AgentEndpoints => "dialog.sshAgentEndpointList",
        }
    }

    pub(in crate::features) fn is_bool(self) -> bool {
        matches!(
            self,
            Self::IconAutoDetect
                | Self::DynamicTabTitle
                | Self::SftpEnabled
                | Self::SftpCompatibility
                | Self::X11
                | Self::AgentForwarding
                | Self::PostLoginEnabled
                | Self::RdpUseNla
                | Self::RdpAdminSession
                | Self::OtpAutoFill
                | Self::TelnetRawTcp
                | Self::TelnetLocalEcho
                | Self::TelnetLocalLineEdit
                | Self::TelnetForceChar
                | Self::TelnetSendNaws
                | Self::TelnetSendSga
                | Self::TelnetAutoLoginEnabled
                | Self::TelnetAutoLoginWake
                | Self::RdpWallpaper
                | Self::RdpThemes
                | Self::RdpFontSmoothing
                | Self::RdpDesktopComposition
                | Self::RdpDisableFullWindowDrag
                | Self::RdpDisableMenuAnimations
                | Self::RdpDisableCursorShadow
                | Self::RdpDisableCursorBlinking
                | Self::RdpAutoReconnect
                | Self::VncClipboard
                | Self::VncShared
                | Self::VncViewOnly
                | Self::VncReconnect
                | Self::RecordingUseGlobal
                | Self::RecordingAutoStart
                | Self::AgentExternal
                | Self::AgentStoredKeys
        )
    }

    pub(in crate::features) fn text_input_id(self) -> SharedString {
        SharedString::from(format!("{ASSET_BATCH_TEXT_PREFIX}{}", self.key()))
    }

    pub(in crate::features) fn select_id(self) -> SharedString {
        SharedString::from(format!("{ASSET_BATCH_SELECT_PREFIX}{}", self.key()))
    }

    pub(in crate::features) fn bool_id(self) -> SharedString {
        SharedString::from(format!("{ASSET_BATCH_BOOL_PREFIX}{}", self.key()))
    }
}

/// Fields the batch editor writes. Each one keeps its connection-editor
/// position; the connection editor labels stay in place for the rest.
pub(in crate::features) const EDITABLE_FIELDS: [BatchFieldId; 95] = [
    BatchFieldId::Group,
    BatchFieldId::Description,
    BatchFieldId::Icon,
    BatchFieldId::IconAutoDetect,
    BatchFieldId::Port,
    BatchFieldId::Username,
    BatchFieldId::Encoding,
    BatchFieldId::Backspace,
    BatchFieldId::SshProfile,
    BatchFieldId::TerminalType,
    BatchFieldId::DynamicTabTitle,
    BatchFieldId::SftpEnabled,
    BatchFieldId::SftpCompatibility,
    BatchFieldId::SftpCwdFollow,
    BatchFieldId::SftpShellTimeout,
    BatchFieldId::SftpFilenameEncoding,
    BatchFieldId::SftpPipelineDepth,
    BatchFieldId::X11,
    BatchFieldId::AgentForwarding,
    BatchFieldId::PostLoginEnabled,
    BatchFieldId::PostLoginCommand,
    BatchFieldId::PostLoginDelay,
    BatchFieldId::SshAlgorithmMode,
    BatchFieldId::SshKex,
    BatchFieldId::SshCiphers,
    BatchFieldId::SshMacs,
    BatchFieldId::SshHostKeys,
    BatchFieldId::RdpUseNla,
    BatchFieldId::RdpCertificatePolicy,
    BatchFieldId::RdpAdminSession,
    BatchFieldId::VncSecurityMode,
    BatchFieldId::Tags,
    BatchFieldId::Account,
    BatchFieldId::Password,
    BatchFieldId::AuthMode,
    BatchFieldId::Otp,
    BatchFieldId::OtpAutoFill,
    BatchFieldId::Key,
    BatchFieldId::SshAuthAgentEndpoint,
    BatchFieldId::SshAuthAgentEnvironment,
    BatchFieldId::SshAuthAgentSocket,
    BatchFieldId::Proxy,
    BatchFieldId::ProxyJump,
    BatchFieldId::ShellPath,
    BatchFieldId::ShellArgs,
    BatchFieldId::WorkingDir,
    BatchFieldId::SerialPort,
    BatchFieldId::BaudRate,
    BatchFieldId::DataBits,
    BatchFieldId::Parity,
    BatchFieldId::StopBits,
    BatchFieldId::FlowControl,
    BatchFieldId::TelnetRawTcp,
    BatchFieldId::TelnetKeepAlive,
    BatchFieldId::TelnetLocalEcho,
    BatchFieldId::TelnetLocalLineEdit,
    BatchFieldId::TelnetForceChar,
    BatchFieldId::TelnetSendNaws,
    BatchFieldId::TelnetSendSga,
    BatchFieldId::TelnetEnterMode,
    BatchFieldId::TelnetAutoLoginEnabled,
    BatchFieldId::TelnetAutoLoginWake,
    BatchFieldId::TelnetAutoLoginTimeout,
    BatchFieldId::TelnetAutoLoginRetries,
    BatchFieldId::TelnetAutoLoginUserPrompt,
    BatchFieldId::TelnetAutoLoginPasswordPrompt,
    BatchFieldId::TelnetAutoLoginSuccessPrompt,
    BatchFieldId::TelnetAutoLoginFailurePrompt,
    BatchFieldId::RdpDomain,
    BatchFieldId::RdpBackend,
    BatchFieldId::RdpDisplayMode,
    BatchFieldId::RdpWidth,
    BatchFieldId::RdpHeight,
    BatchFieldId::RdpWallpaper,
    BatchFieldId::RdpThemes,
    BatchFieldId::RdpFontSmoothing,
    BatchFieldId::RdpDesktopComposition,
    BatchFieldId::RdpDisableFullWindowDrag,
    BatchFieldId::RdpDisableMenuAnimations,
    BatchFieldId::RdpDisableCursorShadow,
    BatchFieldId::RdpDisableCursorBlinking,
    BatchFieldId::RdpAutoReconnect,
    BatchFieldId::RdpReconnectAttempts,
    BatchFieldId::VncScaleMode,
    BatchFieldId::VncClipboard,
    BatchFieldId::VncShared,
    BatchFieldId::VncViewOnly,
    BatchFieldId::VncReconnect,
    BatchFieldId::VncReconnectAttempts,
    BatchFieldId::RecordingUseGlobal,
    BatchFieldId::RecordingAutoStart,
    BatchFieldId::RecordingMode,
    BatchFieldId::AgentExternal,
    BatchFieldId::AgentStoredKeys,
    BatchFieldId::AgentPolicy,
];

/// Fields that exist in the connection editor but cannot be batch-written.
/// They are rendered `disabled` in their original position and never enter a
/// patch, so their value is only ever read for display.
pub(in crate::features) const DISPLAY_FIELDS: [BatchFieldId; 3] = [
    BatchFieldId::Name,
    BatchFieldId::Host,
    BatchFieldId::AgentEndpoints,
];

/// Draft state for the batch editor.
#[derive(Clone, Default)]
pub(in crate::features) struct AssetBatchEditDraft {
    open: bool,
    ids: Vec<String>,
    saving: bool,
    error: Option<SharedString>,
    section: Option<ConnectionEditorSection>,
    /// What the selection's protocols agree on. `Mixed` means more than one
    /// protocol is selected and no tab is preselected.
    kind_basis: FieldBasis,
    /// The shared protocol when every selected connection has the same kind.
    uniform_kind: Option<ConnectionBatchKind>,
    /// The protocol the user explicitly chose. Only this turns into a patch.
    picked_kind: Option<ConnectionBatchKind>,
    basis: HashMap<BatchFieldId, FieldBasis>,
    values: HashMap<BatchFieldId, String>,
    bool_values: HashMap<BatchFieldId, bool>,
    edited: HashSet<BatchFieldId>,
    window: ChildWindowSlot,
    icon_picker_open: bool,
    forwarding_x11: bool,
    algorithm_tab: usize,
    scroll: ScrollHandle,
}

impl AssetBatchEditDraft {
    pub(in crate::features) fn is_open(&self) -> bool {
        self.open
    }

    pub(in crate::features) fn ids(&self) -> &[String] {
        &self.ids
    }

    pub(in crate::features) fn saving(&self) -> bool {
        self.saving
    }

    pub(in crate::features) fn set_saving(&mut self, saving: bool) {
        self.saving = saving;
    }

    pub(in crate::features) fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    pub(in crate::features) fn set_error(&mut self, error: Option<SharedString>) {
        self.error = error;
    }

    pub(in crate::features) fn section(&self) -> ConnectionEditorSection {
        self.section.unwrap_or(ConnectionEditorSection::Basic)
    }

    pub(in crate::features) fn set_section(&mut self, section: ConnectionEditorSection) -> bool {
        if self.section == Some(section) {
            return false;
        }
        self.section = Some(section);
        true
    }

    pub(in crate::features) fn replace_kind_basis(
        &mut self,
        kind_basis: FieldBasis,
        uniform_kind: Option<ConnectionBatchKind>,
    ) {
        self.kind_basis = kind_basis;
        self.uniform_kind = uniform_kind;
    }

    /// The protocol whose fields are on screen: the explicit pick wins over the
    /// selection's shared protocol. `None` means "mixed and not chosen yet".
    pub(in crate::features) fn display_kind(&self) -> Option<ConnectionBatchKind> {
        self.picked_kind.or(self.uniform_kind)
    }

    pub(in crate::features) fn picked_kind(&self) -> Option<ConnectionBatchKind> {
        self.picked_kind
    }

    /// Whether the selection spans several protocols and the user has not
    /// chosen one; the form then shows the mixed prompt instead of fields.
    pub(in crate::features) fn awaiting_kind_pick(&self) -> bool {
        self.kind_basis == FieldBasis::Mixed && self.picked_kind.is_none()
    }

    pub(in crate::features) fn set_kind(&mut self, kind: ConnectionBatchKind) -> bool {
        let changed = self.display_kind() != Some(kind);
        self.picked_kind = Some(kind);
        if changed {
            self.section = Some(ConnectionEditorSection::Basic);
        }
        changed
    }

    /// Replace the per-field basis after a protocol pick while keeping every
    /// edit the user already made on fields that still apply.
    pub(in crate::features) fn refresh_for_kind(
        &mut self,
        basis: HashMap<BatchFieldId, FieldBasis>,
        values: HashMap<BatchFieldId, String>,
        bool_values: HashMap<BatchFieldId, bool>,
    ) {
        let previous_values = std::mem::take(&mut self.values);
        let previous_bools = std::mem::take(&mut self.bool_values);
        let previous_edited = std::mem::take(&mut self.edited);
        self.basis = basis;
        self.values = values;
        self.bool_values = bool_values;
        for field in previous_edited {
            if self.basis.get(&field).copied().unwrap_or_default() == FieldBasis::Inapplicable {
                continue;
            }
            if field.is_bool() {
                if let Some(value) = previous_bools.get(&field) {
                    self.bool_values.insert(field, *value);
                }
            } else if let Some(value) = previous_values.get(&field) {
                self.values.insert(field, value.clone());
            }
            self.edited.insert(field);
        }
    }

    pub(in crate::features) fn basis(&self, field: BatchFieldId) -> FieldBasis {
        self.basis.get(&field).copied().unwrap_or_default()
    }

    pub(in crate::features) fn value(&self, field: BatchFieldId) -> &str {
        self.values.get(&field).map(String::as_str).unwrap_or("")
    }

    pub(in crate::features) fn bool_value(&self, field: BatchFieldId) -> bool {
        self.bool_values.get(&field).copied().unwrap_or(false)
    }

    /// Text a disabled field shows. It follows the same three-state rule as the
    /// editable fields, but it is never written back.
    pub(in crate::features) fn disabled_text(&self, field: BatchFieldId) -> String {
        if self.basis(field) == FieldBasis::Mixed {
            t!("assets.batch.mixed").to_string()
        } else if field.is_bool() {
            bool_text(self.bool_value(field))
        } else {
            self.value(field).to_string()
        }
    }

    pub(in crate::features) fn is_edited(&self, field: BatchFieldId) -> bool {
        self.edited.contains(&field)
    }

    pub(in crate::features) fn has_edits(&self) -> bool {
        self.picked_kind.is_some() || !self.edited.is_empty()
    }

    pub(in crate::features) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Start a fresh draft over `ids`. `kind_basis`/`uniform_kind` describe the
    /// protocols of the selection; `basis`, `values` and `bool_values` mirror
    /// the fields of that shared protocol, computed by the caller before any
    /// widget is created.
    pub(in crate::features) fn start(
        &mut self,
        ids: Vec<String>,
        kind_basis: FieldBasis,
        uniform_kind: Option<ConnectionBatchKind>,
        basis: HashMap<BatchFieldId, FieldBasis>,
        values: HashMap<BatchFieldId, String>,
        bool_values: HashMap<BatchFieldId, bool>,
    ) {
        *self = Self {
            open: true,
            ids,
            kind_basis,
            uniform_kind,
            basis,
            values,
            bool_values,
            section: Some(ConnectionEditorSection::Basic),
            ..Self::default()
        };
    }

    /// Record a user edit. The edited flag is what turns an empty value into
    /// "clear" while an untouched empty field stays "keep current value".
    pub(in crate::features) fn set_value(&mut self, field: BatchFieldId, value: String) {
        self.values.insert(field, value);
        self.edited.insert(field);
    }

    pub(in crate::features) fn set_bool_value(&mut self, field: BatchFieldId, value: bool) {
        self.bool_values.insert(field, value);
        self.edited.insert(field);
    }

    fn bool_field(&self, field: BatchFieldId) -> BatchField<bool> {
        if !self.is_edited(field) {
            BatchField::Unchanged
        } else {
            BatchField::Set(self.bool_value(field))
        }
    }

    fn text_field(&self, field: BatchFieldId, trim: bool) -> BatchField<String> {
        if !self.is_edited(field) {
            return BatchField::Unchanged;
        }
        let value = self.value(field);
        if value.trim().is_empty() {
            return BatchField::Clear;
        }
        BatchField::Set(if trim {
            value.trim().to_string()
        } else {
            value.to_string()
        })
    }

    fn empty_is_clear(&self, field: BatchFieldId) -> bool {
        self.value(field).trim().is_empty()
    }

    fn require_text(&self, field: BatchFieldId) -> Result<String, String> {
        let value = self.value(field).trim();
        if value.is_empty() {
            return Err(
                t!("assets.batch.errorUseClear", field = t!(field.label_key())).to_string(),
            );
        }
        Ok(value.to_string())
    }

    fn require_number<T: std::str::FromStr>(&self, field: BatchFieldId) -> Result<T, String> {
        self.value(field)
            .trim()
            .parse::<T>()
            .map_err(|_| t!("assets.batch.errorNumber", field = t!(field.label_key())).to_string())
    }

    fn list_field(&self, field: BatchFieldId) -> Result<BatchField<Vec<String>>, String> {
        let value = self.require_text(field)?;
        let list = value
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        if list.is_empty() {
            return Err(
                t!("assets.batch.errorUseClear", field = t!(field.label_key())).to_string(),
            );
        }
        Ok(BatchField::Set(list))
    }

    pub(in crate::features) fn window(&self) -> &ChildWindowSlot {
        &self.window
    }

    pub(in crate::features) fn window_mut(&mut self) -> &mut ChildWindowSlot {
        &mut self.window
    }

    pub(in crate::features) fn scroll(&self) -> &ScrollHandle {
        &self.scroll
    }

    pub(in crate::features) fn icon_picker_open(&self) -> bool {
        self.icon_picker_open
    }

    pub(in crate::features) fn set_icon_picker_open(&mut self, open: bool) -> bool {
        if self.icon_picker_open == open {
            return false;
        }
        self.icon_picker_open = open;
        true
    }

    pub(in crate::features) fn forwarding_x11(&self) -> bool {
        self.forwarding_x11
    }

    pub(in crate::features) fn set_forwarding_x11(&mut self, x11: bool) -> bool {
        if self.forwarding_x11 == x11 {
            return false;
        }
        self.forwarding_x11 = x11;
        true
    }

    pub(in crate::features) fn algorithm_tab(&self) -> usize {
        self.algorithm_tab.min(3)
    }

    pub(in crate::features) fn set_algorithm_tab(&mut self, tab: usize) -> bool {
        let tab = tab.min(3);
        if self.algorithm_tab == tab {
            return false;
        }
        self.algorithm_tab = tab;
        true
    }

    pub(in crate::features) fn build_edit(&self) -> Result<ConnectionBatchEdit, String> {
        let mut edit = ConnectionBatchEdit::default();
        if let Some(kind) = self.picked_kind {
            edit.kind = BatchField::Set(kind);
        }
        for field in EDITABLE_FIELDS {
            if !self.is_edited(field) {
                continue;
            }
            match field {
                BatchFieldId::Group => {
                    let value = self.value(field).trim();
                    edit.group_id = if value.is_empty() {
                        BatchField::Clear
                    } else {
                        BatchField::Set(value.to_string())
                    };
                }
                BatchFieldId::Description => {
                    edit.description = self.text_field(field, true);
                }
                BatchFieldId::Icon => {
                    edit.icon = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        BatchField::Set(self.value(field).to_string())
                    };
                }
                BatchFieldId::IconAutoDetect => {
                    edit.icon_auto_detect = self.bool_field(field);
                }
                BatchFieldId::Port => {
                    edit.port = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        let port = self.require_number::<u16>(field)?;
                        if port == 0 {
                            return Err(t!("assets.batch.errorPort").to_string());
                        }
                        BatchField::Set(port)
                    };
                }
                BatchFieldId::Username => {
                    edit.username = self.text_field(field, false);
                }
                BatchFieldId::Encoding => {
                    edit.encoding = if self.value(field) == "global" {
                        BatchField::Clear
                    } else {
                        self.text_field(field, true)
                    };
                }
                BatchFieldId::Backspace => {
                    edit.backspace_mode = self.text_field(field, true);
                }
                BatchFieldId::SshProfile => {
                    edit.ssh_profile = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        let profile = match self.value(field) {
                            "network_device" | "network-device" => SshProfile::NetworkDevice,
                            _ => SshProfile::Standard,
                        };
                        BatchField::Set(profile)
                    };
                }
                BatchFieldId::TerminalType => {
                    edit.terminal_type = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        let terminal = match self.value(field) {
                            "xterm" => SshTerminalType::Xterm,
                            "vt100" => SshTerminalType::Vt100,
                            "vt220" => SshTerminalType::Vt220,
                            "ansi" => SshTerminalType::Ansi,
                            "linux" => SshTerminalType::Linux,
                            _ => SshTerminalType::Xterm256Color,
                        };
                        BatchField::Set(terminal)
                    };
                }
                BatchFieldId::DynamicTabTitle => {
                    edit.dynamic_tab_title = self.bool_field(field);
                }
                BatchFieldId::SftpEnabled => {
                    edit.sftp_enabled = self.bool_field(field);
                }
                BatchFieldId::SftpCompatibility => {
                    edit.sftp_compatibility_mode = self.bool_field(field);
                }
                BatchFieldId::SftpCwdFollow => {
                    edit.sftp_cwd_follow_mode = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        let mode = match self.value(field) {
                            "off" => SftpCwdFollowMode::Off,
                            "rc_file" | "rc-file" => SftpCwdFollowMode::RcFile,
                            _ => SftpCwdFollowMode::ShellIntegration,
                        };
                        BatchField::Set(mode)
                    };
                }
                BatchFieldId::SftpShellTimeout => {
                    edit.sftp_shell_detection_timeout_ms = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        let value = self.require_number::<u64>(field)?;
                        if !(MIN_SFTP_SHELL_DETECTION_TIMEOUT_MS
                            ..=MAX_SFTP_SHELL_DETECTION_TIMEOUT_MS)
                            .contains(&value)
                        {
                            return Err(t!(
                                "assets.batch.errorTimeoutRange",
                                min = MIN_SFTP_SHELL_DETECTION_TIMEOUT_MS,
                                max = MAX_SFTP_SHELL_DETECTION_TIMEOUT_MS
                            )
                            .to_string());
                        }
                        BatchField::Set(value)
                    };
                }
                BatchFieldId::SftpFilenameEncoding => {
                    edit.sftp_filename_encoding = if self.value(field) == "terminal" {
                        BatchField::Clear
                    } else {
                        self.text_field(field, true)
                    };
                }
                BatchFieldId::SftpPipelineDepth => {
                    edit.sftp_pipeline_depth = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        let value = self.require_number::<u32>(field)?;
                        if value == 0 {
                            return Err(t!("assets.batch.errorPipelineDepth").to_string());
                        }
                        BatchField::Set(value)
                    };
                }
                BatchFieldId::X11 => {
                    edit.x11_forwarding = self.bool_field(field);
                }
                BatchFieldId::AgentForwarding => {
                    edit.agent_forwarding_enabled = self.bool_field(field);
                }
                BatchFieldId::PostLoginEnabled => {
                    edit.post_login_enabled = self.bool_field(field);
                }
                BatchFieldId::PostLoginCommand => {
                    edit.post_login_command = self.text_field(field, true);
                }
                BatchFieldId::PostLoginDelay => {
                    edit.post_login_delay_ms = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        BatchField::Set(self.require_number::<u64>(field)?)
                    };
                }
                BatchFieldId::SshAlgorithmMode => {
                    edit.ssh_algorithm_mode = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        let mode = match self.value(field) {
                            "secure" => SshAlgorithmMode::Secure,
                            "custom" => SshAlgorithmMode::Custom,
                            _ => SshAlgorithmMode::Compatible,
                        };
                        BatchField::Set(mode)
                    };
                }
                BatchFieldId::SshKex => {
                    edit.ssh_kex = self.list_field(field)?;
                }
                BatchFieldId::SshCiphers => {
                    edit.ssh_ciphers = self.list_field(field)?;
                }
                BatchFieldId::SshMacs => {
                    edit.ssh_macs = self.list_field(field)?;
                }
                BatchFieldId::SshHostKeys => {
                    edit.ssh_host_keys = self.list_field(field)?;
                }
                BatchFieldId::RdpUseNla => {
                    edit.rdp_use_nla = self.bool_field(field);
                }
                BatchFieldId::RdpCertificatePolicy => {
                    edit.rdp_certificate_policy = self.text_field(field, true);
                }
                BatchFieldId::RdpAdminSession => {
                    edit.rdp_administrative_session = self.bool_field(field);
                }
                BatchFieldId::VncSecurityMode => {
                    edit.vnc_security_mode = self.text_field(field, true);
                }
                BatchFieldId::Tags => {
                    let tags = self
                        .value(field)
                        .split(',')
                        .map(str::trim)
                        .filter(|tag| !tag.is_empty())
                        .map(str::to_string)
                        .collect::<Vec<_>>();
                    edit.tags = if tags.is_empty() {
                        BatchField::Clear
                    } else {
                        BatchField::Set(tags)
                    };
                }
                BatchFieldId::Account => {
                    let value = self.value(field).trim();
                    let uses_password_catalog = matches!(
                        self.display_kind(),
                        Some(ConnectionBatchKind::Rdp | ConnectionBatchKind::Vnc)
                    );
                    let patch = if value.is_empty() {
                        BatchField::Clear
                    } else {
                        BatchField::Set(value.to_string())
                    };
                    if uses_password_catalog {
                        edit.password_id = patch;
                    } else {
                        edit.account_id = patch;
                    }
                }
                BatchFieldId::Password => {
                    let value = self.value(field);
                    edit.password = if value.is_empty() {
                        BatchField::Clear
                    } else {
                        BatchField::Set(SecretString::new(value))
                    };
                }
                BatchFieldId::AuthMode => {
                    edit.auth_mode = self.text_field(field, true);
                }
                BatchFieldId::Otp => {
                    edit.otp_id = self.text_field(field, true);
                }
                BatchFieldId::OtpAutoFill => {
                    edit.auto_fill_otp = self.bool_field(field);
                }
                BatchFieldId::Key => {
                    edit.key_id = self.text_field(field, true);
                }
                BatchFieldId::SshAuthAgentEndpoint
                | BatchFieldId::SshAuthAgentEnvironment
                | BatchFieldId::SshAuthAgentSocket => {
                    if self.is_edited(BatchFieldId::SshAuthAgentEndpoint) {
                        let endpoint = self.value(BatchFieldId::SshAuthAgentEndpoint).trim();
                        edit.auth_agent_endpoint = match endpoint {
                            "environment" => BatchField::Set(SshAgentEndpoint::Environment {
                                variable: self
                                    .value(BatchFieldId::SshAuthAgentEnvironment)
                                    .trim()
                                    .to_string(),
                            }),
                            "unix_socket" => BatchField::Set(SshAgentEndpoint::UnixSocket {
                                path: self
                                    .value(BatchFieldId::SshAuthAgentSocket)
                                    .trim()
                                    .to_string(),
                            }),
                            "auto" => BatchField::Set(SshAgentEndpoint::Auto),
                            "pageant" => BatchField::Set(SshAgentEndpoint::Pageant),
                            "windows_openssh" => BatchField::Set(SshAgentEndpoint::WindowsOpenSsh),
                            _ => BatchField::Clear,
                        };
                    } else if self.is_edited(BatchFieldId::SshAuthAgentEnvironment) {
                        let variable = self.value(BatchFieldId::SshAuthAgentEnvironment).trim();
                        edit.auth_agent_endpoint = if variable.is_empty() {
                            BatchField::Clear
                        } else {
                            BatchField::Set(SshAgentEndpoint::Environment {
                                variable: variable.to_string(),
                            })
                        };
                    } else if self.is_edited(BatchFieldId::SshAuthAgentSocket) {
                        let path = self.value(BatchFieldId::SshAuthAgentSocket).trim();
                        edit.auth_agent_endpoint = if path.is_empty() {
                            BatchField::Clear
                        } else {
                            BatchField::Set(SshAgentEndpoint::UnixSocket {
                                path: path.to_string(),
                            })
                        };
                    }
                }
                BatchFieldId::Proxy => {
                    edit.proxy_id = self.text_field(field, true);
                }
                BatchFieldId::ProxyJump => {
                    edit.proxy_jump_id = self.text_field(field, true);
                }
                BatchFieldId::AgentExternal => {
                    edit.agent_forwarding_external = self.bool_field(field);
                }
                BatchFieldId::AgentStoredKeys => {
                    edit.agent_forwarding_stored_keys = self.bool_field(field);
                }
                BatchFieldId::AgentPolicy => {
                    edit.agent_forwarding_policy = match self.value(field).trim() {
                        "all" => BatchField::Set(SshAgentForwardingPolicy::All),
                        "allowlist" => BatchField::Set(SshAgentForwardingPolicy::Allowlist {
                            fingerprints: Vec::new(),
                        }),
                        _ => BatchField::Clear,
                    };
                }
                BatchFieldId::ShellPath => {
                    edit.shell_path = self.text_field(field, true);
                }
                BatchFieldId::ShellArgs => {
                    edit.shell_args = self.text_field(field, true);
                }
                BatchFieldId::WorkingDir => {
                    edit.working_dir = self.text_field(field, true);
                }
                BatchFieldId::SerialPort => {
                    edit.serial_port_name = self.text_field(field, true);
                }
                BatchFieldId::BaudRate => {
                    edit.serial_baud_rate = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        BatchField::Set(self.value(field).trim().parse::<u32>().unwrap_or(0))
                    };
                }
                BatchFieldId::DataBits => {
                    edit.serial_data_bits = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        BatchField::Set(self.value(field).trim().parse::<u8>().unwrap_or(0))
                    };
                }
                BatchFieldId::Parity => {
                    edit.serial_parity = self.text_field(field, true);
                }
                BatchFieldId::StopBits => {
                    edit.serial_stop_bits = self.text_field(field, true);
                }
                BatchFieldId::FlowControl => {
                    edit.serial_flow_control = match self.value(field).trim() {
                        "hardware" => BatchField::Set(SerialFlowControl::Hardware),
                        "software" => BatchField::Set(SerialFlowControl::Software),
                        "none" if self.is_edited(field) => BatchField::Set(SerialFlowControl::None),
                        _ => BatchField::Clear,
                    };
                }
                BatchFieldId::TelnetRawTcp => {
                    edit.telnet_raw_tcp_cli = self.bool_field(field);
                }
                BatchFieldId::TelnetLocalEcho => {
                    edit.telnet_local_echo = self.bool_field(field);
                }
                BatchFieldId::TelnetLocalLineEdit => {
                    edit.telnet_local_line_edit = self.bool_field(field);
                }
                BatchFieldId::TelnetForceChar => {
                    edit.telnet_force_character_at_a_time = self.bool_field(field);
                }
                BatchFieldId::TelnetSendNaws => {
                    edit.telnet_send_naws = self.bool_field(field);
                }
                BatchFieldId::TelnetSendSga => {
                    edit.telnet_send_sga = self.bool_field(field);
                }
                BatchFieldId::TelnetKeepAlive => {
                    edit.telnet_keep_alive_interval_secs = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        BatchField::Set(self.value(field).trim().parse::<u32>().unwrap_or(0))
                    };
                }
                BatchFieldId::TelnetEnterMode => {
                    edit.telnet_enter_mode = self.text_field(field, true);
                }
                BatchFieldId::TelnetAutoLoginEnabled => {
                    edit.telnet_auto_login_enabled = self.bool_field(field);
                }
                BatchFieldId::TelnetAutoLoginWake => {
                    edit.telnet_auto_login_send_wake_enter = self.bool_field(field);
                }
                BatchFieldId::TelnetAutoLoginTimeout => {
                    edit.telnet_auto_login_timeout_ms = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        BatchField::Set(self.value(field).trim().parse::<u64>().unwrap_or(0))
                    };
                }
                BatchFieldId::TelnetAutoLoginRetries => {
                    edit.telnet_auto_login_max_retries = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        BatchField::Set(self.value(field).trim().parse::<u8>().unwrap_or(0))
                    };
                }
                BatchFieldId::TelnetAutoLoginUserPrompt => {
                    edit.telnet_auto_login_username_prompt = self.text_field(field, true);
                }
                BatchFieldId::TelnetAutoLoginPasswordPrompt => {
                    edit.telnet_auto_login_password_prompt = self.text_field(field, true);
                }
                BatchFieldId::TelnetAutoLoginSuccessPrompt => {
                    edit.telnet_auto_login_success_prompt = self.text_field(field, true);
                }
                BatchFieldId::TelnetAutoLoginFailurePrompt => {
                    edit.telnet_auto_login_failure_prompt = self.text_field(field, true);
                }
                BatchFieldId::RdpDomain => {
                    edit.rdp_domain = self.text_field(field, true);
                }
                BatchFieldId::RdpBackend => {
                    edit.rdp_backend = self.text_field(field, true);
                }
                BatchFieldId::RdpDisplayMode => {
                    edit.rdp_display_mode = self.text_field(field, true);
                }
                BatchFieldId::RdpWidth => {
                    edit.rdp_display_width = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        BatchField::Set(self.value(field).trim().parse::<u32>().unwrap_or(0))
                    };
                }
                BatchFieldId::RdpHeight => {
                    edit.rdp_display_height = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        BatchField::Set(self.value(field).trim().parse::<u32>().unwrap_or(0))
                    };
                }
                BatchFieldId::RdpWallpaper => {
                    edit.rdp_display_wallpaper = self.bool_field(field);
                }
                BatchFieldId::RdpThemes => {
                    edit.rdp_display_themes = self.bool_field(field);
                }
                BatchFieldId::RdpFontSmoothing => {
                    edit.rdp_font_smoothing = self.bool_field(field);
                }
                BatchFieldId::RdpDesktopComposition => {
                    edit.rdp_desktop_composition = self.bool_field(field);
                }
                BatchFieldId::RdpDisableFullWindowDrag => {
                    edit.rdp_disable_full_window_drag = self.bool_field(field);
                }
                BatchFieldId::RdpDisableMenuAnimations => {
                    edit.rdp_disable_menu_animations = self.bool_field(field);
                }
                BatchFieldId::RdpDisableCursorShadow => {
                    edit.rdp_disable_cursor_shadow = self.bool_field(field);
                }
                BatchFieldId::RdpDisableCursorBlinking => {
                    edit.rdp_disable_cursor_blinking = self.bool_field(field);
                }
                BatchFieldId::RdpAutoReconnect => {
                    edit.rdp_auto_reconnect = self.bool_field(field);
                }
                BatchFieldId::RdpReconnectAttempts => {
                    edit.rdp_reconnect_attempts = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        BatchField::Set(self.value(field).trim().parse::<u32>().unwrap_or(0))
                    };
                }
                BatchFieldId::VncScaleMode => {
                    edit.vnc_scale_mode = self.text_field(field, true);
                }
                BatchFieldId::VncClipboard => {
                    edit.vnc_clipboard = self.bool_field(field);
                }
                BatchFieldId::VncShared => {
                    edit.vnc_shared = self.bool_field(field);
                }
                BatchFieldId::VncViewOnly => {
                    edit.vnc_view_only = self.bool_field(field);
                }
                BatchFieldId::VncReconnect => {
                    edit.vnc_reconnect = self.bool_field(field);
                }
                BatchFieldId::VncReconnectAttempts => {
                    edit.vnc_reconnect_attempts = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        BatchField::Set(self.value(field).trim().parse::<u32>().unwrap_or(0))
                    };
                }
                BatchFieldId::RecordingUseGlobal => {
                    edit.recording_use_global = self.bool_field(field);
                }
                BatchFieldId::RecordingAutoStart => {
                    edit.recording_auto_start = self.bool_field(field);
                }
                BatchFieldId::RecordingMode => {
                    edit.recording_mode = match self.value(field).trim() {
                        "raw" => BatchField::Set(RecordingMode::Raw),
                        "transcript" => BatchField::Set(RecordingMode::Transcript),
                        _ => BatchField::Clear,
                    };
                }
                // The loop walks EDITABLE_FIELDS, so the disabled display
                // fields are never reached and never enter a patch.
                _ => {}
            }
        }

        if edit.is_empty() {
            return Err(t!("assets.batch.errorNoFields").to_string());
        }
        validate_connection_batch_edit(&edit)?;
        Ok(edit)
    }
}

/// Per-protocol counts for the mixed-protocol confirmation. Zero entries are
/// omitted, so the prompt only lists what is actually selected.
fn asset_batch_kind_counts(
    connections: &[SavedConnection],
    ids: &[String],
) -> Vec<(ConnectionBatchKind, usize)> {
    let by_id: HashMap<&str, &SavedConnection> = connections
        .iter()
        .map(|connection| (connection.id.as_str(), connection))
        .collect();
    let mut counts = [0usize; 6];
    for id in ids {
        if let Some(connection) = by_id.get(id.as_str()) {
            counts[asset_batch_kind_slot(ConnectionBatchKind::from_config(&connection.config))] +=
                1;
        }
    }
    [
        ConnectionBatchKind::Ssh,
        ConnectionBatchKind::LocalTerminal,
        ConnectionBatchKind::Telnet,
        ConnectionBatchKind::Serial,
        ConnectionBatchKind::Rdp,
        ConnectionBatchKind::Vnc,
    ]
    .into_iter()
    .zip(counts)
    .filter_map(|(kind, count)| (count > 0).then_some((kind, count)))
    .collect()
}

fn asset_batch_kind_slot(kind: ConnectionBatchKind) -> usize {
    match kind {
        ConnectionBatchKind::Ssh => 0,
        ConnectionBatchKind::LocalTerminal => 1,
        ConnectionBatchKind::Telnet => 2,
        ConnectionBatchKind::Serial => 3,
        ConnectionBatchKind::Rdp => 4,
        ConnectionBatchKind::Vnc => 5,
    }
}

/// Renders the counts with the editor's own protocol labels, never raw
/// identifiers, and joins them with the locale's list separator.
fn asset_batch_kind_counts_label(counts: &[(ConnectionBatchKind, usize)]) -> String {
    let separator = t!("assets.batch.mixedProtocolsSeparator").to_string();
    counts
        .iter()
        .map(|(kind, count)| {
            t!(
                "assets.batch.mixedProtocolsItem",
                kind = view::batch_kind_label(*kind),
                count = count
            )
            .to_string()
        })
        .collect::<Vec<_>>()
        .join(&separator)
}

impl NyaTermApp {
    /// Open the editor for the current asset selection. Creating the widget
    /// entities here, in an event handler, keeps the render path read-only.
    ///
    /// A selection that spans more than one protocol asks for confirmation
    /// first: the editable fields differ per protocol and protocol-specific
    /// settings cannot be shared. Cancel (or Esc, or clicking the mask, which
    /// the shared confirm dialog routes to cancel) returns to the list with the
    /// existing selection intact.
    pub(in crate::features) fn open_asset_batch_edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ids = self.start_workspace.selected_asset_ids();
        if ids.is_empty() {
            return;
        }
        if self.start_workspace.asset_batch_is_open() && self.activate_asset_batch_edit_window(cx) {
            return;
        }
        let counts = asset_batch_kind_counts(self.connection_state.connections(), &ids);
        if counts.len() > 1 {
            let message = t!(
                "assets.batch.mixedProtocolsMessage",
                counts = asset_batch_kind_counts_label(&counts)
            )
            .to_string();
            self.open_confirm_dialog(
                (
                    t!("assets.batch.mixedProtocolsTitle").to_string(),
                    message,
                    t!("assets.batch.mixedProtocolsContinue").to_string(),
                    false,
                    |app: &mut NyaTermApp, _: &mut Window, cx: &mut Context<NyaTermApp>| {
                        app.start_asset_batch_edit_for_selection(cx);
                        true
                    },
                ),
                window,
                cx,
            );
            return;
        }
        self.start_asset_batch_edit_for_selection(cx);
    }

    fn start_asset_batch_edit_for_selection(&mut self, cx: &mut Context<Self>) {
        let ids = self.start_workspace.selected_asset_ids();
        if ids.is_empty() {
            return;
        }
        let (kind_basis, uniform_kind, basis, values, bool_values) =
            compute_batch_basis(self.connection_state.connections(), &ids, None);
        let groups = self.connection_state.groups().to_vec();
        self.forget_text_inputs(ASSET_BATCH_TEXT_PREFIX);
        self.start_workspace.start_asset_batch_edit(
            ids,
            kind_basis,
            uniform_kind,
            basis,
            values,
            bool_values,
        );
        self.seed_asset_batch_inputs(&groups, cx);
        self.open_asset_batch_edit_window(cx);
        cx.notify();
    }

    /// Pick a protocol for every selected connection. The field form switches
    /// to the target protocol with its defaults; nothing is written until the
    /// user saves.
    pub(in crate::features) fn set_asset_batch_kind(
        &mut self,
        kind: ConnectionBatchKind,
        cx: &mut Context<Self>,
    ) {
        if !self.start_workspace.asset_batch_is_open() {
            return;
        }
        if !self.start_workspace.asset_batch_set_kind(kind) {
            cx.notify();
            return;
        }
        let ids = self.start_workspace.asset_batch_ids().to_vec();
        let (kind_basis, uniform_kind, basis, values, bool_values) =
            compute_batch_basis(self.connection_state.connections(), &ids, Some(kind));
        self.start_workspace.refresh_asset_batch_fields(
            kind_basis,
            uniform_kind,
            basis,
            values,
            bool_values,
        );
        let groups = self.connection_state.groups().to_vec();
        self.forget_text_inputs(ASSET_BATCH_TEXT_PREFIX);
        self.seed_asset_batch_inputs(&groups, cx);
        cx.notify();
    }

    pub(in crate::features) fn close_asset_batch_edit(&mut self, cx: &mut Context<Self>) {
        let handle = self.start_workspace.asset_batch_window_handle();
        self.dismiss_asset_batch_edit(cx);
        if let Some(handle) = handle {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
        cx.notify();
    }

    /// Close without touching the native window; used by the window's own
    /// close handler, which removes the window itself.
    pub(in crate::features) fn dismiss_asset_batch_edit(&mut self, cx: &mut Context<Self>) {
        self.start_workspace.clear_asset_batch_edit();
        self.forget_text_inputs(ASSET_BATCH_TEXT_PREFIX);
        cx.notify();
    }

    pub(in crate::features) fn set_asset_batch_section(
        &mut self,
        section: ConnectionEditorSection,
        cx: &mut Context<Self>,
    ) {
        if self.start_workspace.set_asset_batch_section(section) {
            cx.notify();
        }
    }

    fn seed_asset_batch_inputs(&mut self, groups: &[nyaterm_core::Group], cx: &mut Context<Self>) {
        for (field, hint) in [
            (
                BatchFieldId::Description,
                t!("assets.batch.placeholderDescription"),
            ),
            (BatchFieldId::Port, t!("assets.batch.placeholderPort")),
            (
                BatchFieldId::Username,
                t!("assets.batch.placeholderUsername"),
            ),
            (
                BatchFieldId::SftpShellTimeout,
                t!("assets.batch.placeholderShellTimeout"),
            ),
            (
                BatchFieldId::PostLoginCommand,
                t!("assets.batch.placeholderPostLoginCommand"),
            ),
            (
                BatchFieldId::PostLoginDelay,
                t!("assets.batch.placeholderPostLoginDelay"),
            ),
            (BatchFieldId::SshKex, t!("assets.batch.placeholderSshKex")),
            (
                BatchFieldId::SshCiphers,
                t!("assets.batch.placeholderSshCiphers"),
            ),
            (BatchFieldId::SshMacs, t!("assets.batch.placeholderSshMacs")),
            (
                BatchFieldId::SshHostKeys,
                t!("assets.batch.placeholderSshHostKeys"),
            ),
            (BatchFieldId::Tags, t!("assets.batch.placeholderValue")),
            (
                BatchFieldId::SshAuthAgentEnvironment,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldId::SshAuthAgentSocket,
                t!("assets.batch.placeholderValue"),
            ),
            (BatchFieldId::ShellPath, t!("assets.batch.placeholderValue")),
            (BatchFieldId::ShellArgs, t!("assets.batch.placeholderValue")),
            (
                BatchFieldId::WorkingDir,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldId::TelnetKeepAlive,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldId::TelnetAutoLoginTimeout,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldId::TelnetAutoLoginRetries,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldId::TelnetAutoLoginUserPrompt,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldId::TelnetAutoLoginPasswordPrompt,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldId::TelnetAutoLoginSuccessPrompt,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldId::TelnetAutoLoginFailurePrompt,
                t!("assets.batch.placeholderValue"),
            ),
            (BatchFieldId::RdpDomain, t!("assets.batch.placeholderValue")),
            (BatchFieldId::RdpWidth, t!("assets.batch.placeholderValue")),
            (BatchFieldId::RdpHeight, t!("assets.batch.placeholderValue")),
            (
                BatchFieldId::RdpReconnectAttempts,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldId::VncReconnectAttempts,
                t!("assets.batch.placeholderValue"),
            ),
        ] {
            let (seed, placeholder, disabled) = {
                let draft = self.start_workspace.asset_batch();
                match draft.basis(field) {
                    FieldBasis::Inapplicable => (
                        String::new(),
                        t!("assets.batch.inapplicable").to_string(),
                        true,
                    ),
                    FieldBasis::Mixed => {
                        (String::new(), t!("assets.batch.mixed").to_string(), false)
                    }
                    FieldBasis::Uniform => {
                        let value = draft.value(field).to_string();
                        let placeholder = if value.is_empty() {
                            String::new()
                        } else {
                            hint.to_string()
                        };
                        (value, placeholder, false)
                    }
                }
            };
            let input = self.text_input(
                field.text_input_id(),
                &seed,
                TextInputSetup::placeholder(placeholder),
                cx,
            );
            if disabled {
                input.update(cx, |state, cx| state.set_disabled(true, cx));
            }
        }

        {
            let (placeholder, disabled) = {
                let draft = self.start_workspace.asset_batch();
                match draft.basis(BatchFieldId::Password) {
                    FieldBasis::Inapplicable => (t!("assets.batch.inapplicable").to_string(), true),
                    FieldBasis::Mixed => (t!("assets.batch.mixed").to_string(), false),
                    FieldBasis::Uniform => (draft.value(BatchFieldId::Password).to_string(), false),
                }
            };
            // The seed is always empty: the existing credential is never read
            // back into the input, only its safe set/unset status is shown.
            let mut setup = TextInputSetup::masked();
            setup.placeholder = placeholder.into();
            let input = self.text_input(BatchFieldId::Password.text_input_id(), "", setup, cx);
            if disabled {
                input.update(cx, |state, cx| state.set_disabled(true, cx));
            }
        }

        {
            let (selected, placeholder) = {
                let draft = self.start_workspace.asset_batch();
                match draft.display_kind() {
                    Some(kind) => (Some(batch_kind_value(kind).to_string()), String::new()),
                    None => (None, t!("assets.batch.mixed").to_string()),
                }
            };
            let select = self.select_entity(
                format!("{ASSET_BATCH_SELECT_PREFIX}kind"),
                view::kind_select_options(),
                selected,
                false,
                cx,
            );
            select.update(cx, |state, cx| {
                state.set_placeholder(gpui::SharedString::from(placeholder), cx);
            });
        }

        let selected_value = |field: BatchFieldId| {
            let draft = self.start_workspace.asset_batch();
            select_initial_value(draft, field).unwrap_or_default()
        };
        let selected_ids = self.start_workspace.asset_batch_ids().to_vec();
        let account_kind = self.start_workspace.asset_batch().display_kind();
        let terminal_profile = if selected_value(BatchFieldId::SshProfile) == "network_device" {
            SshProfile::NetworkDevice
        } else {
            SshProfile::Standard
        };
        for (field, options) in [
            (BatchFieldId::Group, view::group_select_options(groups)),
            (BatchFieldId::Icon, view::icon_select_options()),
            (BatchFieldId::Encoding, view::encoding_select_options()),
            (BatchFieldId::Backspace, view::backspace_select_options()),
            (BatchFieldId::SshProfile, view::ssh_profile_select_options()),
            (
                BatchFieldId::TerminalType,
                view::terminal_type_select_options(terminal_profile),
            ),
            (
                BatchFieldId::SftpCwdFollow,
                view::sftp_cwd_follow_select_options(),
            ),
            (
                BatchFieldId::SftpFilenameEncoding,
                view::sftp_filename_encoding_select_options(),
            ),
            (
                BatchFieldId::SftpPipelineDepth,
                view::sftp_pipeline_depth_select_options(&selected_value(
                    BatchFieldId::SftpPipelineDepth,
                )),
            ),
            (
                BatchFieldId::SshAlgorithmMode,
                view::ssh_algorithm_mode_select_options(),
            ),
            (
                BatchFieldId::RdpCertificatePolicy,
                view::rdp_certificate_policy_select_options(),
            ),
            (
                BatchFieldId::VncSecurityMode,
                view::vnc_security_select_options(),
            ),
            (
                BatchFieldId::Account,
                view::account_select_options(
                    self.security.passwords(),
                    account_kind,
                    &selected_value(BatchFieldId::Account),
                ),
            ),
            (
                BatchFieldId::Key,
                view::key_select_options(
                    self.security.ssh_keys(),
                    &selected_value(BatchFieldId::Key),
                ),
            ),
            (
                BatchFieldId::Otp,
                view::otp_select_options(
                    self.security.otp_entries(),
                    &selected_value(BatchFieldId::Otp),
                ),
            ),
            (
                BatchFieldId::Proxy,
                view::proxy_select_options(
                    self.tunnel_state.proxies(),
                    &selected_value(BatchFieldId::Proxy),
                ),
            ),
            (
                BatchFieldId::ProxyJump,
                view::proxy_jump_select_options(
                    self.connection_state.connections(),
                    groups,
                    &selected_value(BatchFieldId::ProxyJump),
                    &selected_ids,
                ),
            ),
            (
                BatchFieldId::SshAuthAgentEndpoint,
                view::ssh_agent_endpoint_select_options(&selected_value(
                    BatchFieldId::SshAuthAgentEndpoint,
                )),
            ),
            (
                BatchFieldId::AgentPolicy,
                view::ssh_agent_policy_select_options(&selected_value(BatchFieldId::AgentPolicy)),
            ),
            (
                BatchFieldId::RdpBackend,
                view::rdp_backend_select_options(&selected_value(BatchFieldId::RdpBackend)),
            ),
            (
                BatchFieldId::RdpDisplayMode,
                view::rdp_display_mode_select_options(&selected_value(
                    BatchFieldId::RdpDisplayMode,
                )),
            ),
            (
                BatchFieldId::VncScaleMode,
                view::vnc_scale_select_options(&selected_value(BatchFieldId::VncScaleMode)),
            ),
            (
                BatchFieldId::RecordingMode,
                view::recording_mode_select_options(&selected_value(BatchFieldId::RecordingMode)),
            ),
            (
                BatchFieldId::SerialPort,
                view::serial_port_select_options(
                    self.connection_state.serial_ports(),
                    &selected_value(BatchFieldId::SerialPort),
                ),
            ),
            (
                BatchFieldId::BaudRate,
                view::baud_rate_select_options(&selected_value(BatchFieldId::BaudRate)),
            ),
            (
                BatchFieldId::DataBits,
                view::data_bits_select_options(&selected_value(BatchFieldId::DataBits)),
            ),
            (
                BatchFieldId::Parity,
                view::parity_select_options(&selected_value(BatchFieldId::Parity)),
            ),
            (
                BatchFieldId::StopBits,
                view::stop_bits_select_options(&selected_value(BatchFieldId::StopBits)),
            ),
            (
                BatchFieldId::FlowControl,
                view::flow_control_select_options(&selected_value(BatchFieldId::FlowControl)),
            ),
            (
                BatchFieldId::TelnetEnterMode,
                view::telnet_enter_select_options(&selected_value(BatchFieldId::TelnetEnterMode)),
            ),
        ] {
            self.seed_asset_batch_select(field, options, cx);
        }
    }

    fn seed_asset_batch_select(
        &mut self,
        field: BatchFieldId,
        options: Vec<nyaterm_ui::NyaSelectOption>,
        cx: &mut Context<Self>,
    ) {
        let (selected, placeholder, disabled) = {
            let draft = self.start_workspace.asset_batch();
            match draft.basis(field) {
                FieldBasis::Inapplicable => {
                    (None, t!("assets.batch.inapplicable").to_string(), true)
                }
                FieldBasis::Mixed => (None, t!("assets.batch.mixed").to_string(), false),
                FieldBasis::Uniform => (select_initial_value(draft, field), String::new(), false),
            }
        };
        let select = self.select_entity(field.select_id(), options, selected, disabled, cx);
        select.update(cx, |state, cx| {
            state.set_placeholder(gpui::SharedString::from(placeholder), cx);
        });
    }

    pub(in crate::features) fn set_asset_batch_bool(
        &mut self,
        field: BatchFieldId,
        value: bool,
        cx: &mut Context<Self>,
    ) {
        self.start_workspace.set_asset_batch_bool(field, value);
        cx.notify();
    }

    pub(in crate::features) fn set_asset_batch_icon(&mut self, icon: &str, cx: &mut Context<Self>) {
        if !self.start_workspace.asset_batch_is_open() {
            return;
        }
        self.start_workspace
            .set_asset_batch_value(BatchFieldId::Icon, icon);
        self.start_workspace.set_asset_batch_icon_picker_open(false);
        cx.notify();
    }

    pub(in crate::features) fn set_asset_batch_icon_picker_open(
        &mut self,
        open: bool,
        cx: &mut Context<Self>,
    ) {
        if self.start_workspace.set_asset_batch_icon_picker_open(open) {
            cx.notify();
        }
    }

    pub(in crate::features) fn set_asset_batch_forwarding_x11(
        &mut self,
        x11: bool,
        cx: &mut Context<Self>,
    ) {
        if self.start_workspace.set_asset_batch_forwarding_x11(x11) {
            cx.notify();
        }
    }

    pub(in crate::features) fn set_asset_batch_algorithm_tab(
        &mut self,
        tab: usize,
        cx: &mut Context<Self>,
    ) {
        if self.start_workspace.set_asset_batch_algorithm_tab(tab) {
            cx.notify();
        }
    }

    /// Registry text edits. Values are already authoritative in the draft, so
    /// no re-render is required for every keystroke.
    pub(in crate::features) fn apply_asset_batch_input(&mut self, id: &str, text: &str) {
        let Some(key) = id.strip_prefix(ASSET_BATCH_TEXT_PREFIX) else {
            return;
        };
        if key.starts_with("select.") || key.starts_with("bool.") {
            return;
        }
        if let Some(field) = BatchFieldId::from_key(key) {
            self.start_workspace.set_asset_batch_value(field, text);
        }
    }

    pub(in crate::features) fn apply_asset_batch_select(
        &mut self,
        id: &str,
        value: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = id.strip_prefix(ASSET_BATCH_SELECT_PREFIX) else {
            return;
        };
        if key == "kind" {
            if let Some(kind) = parse_batch_kind(value) {
                self.set_asset_batch_kind(kind, cx);
            }
            return;
        }
        if let Some(field) = BatchFieldId::from_key(key) {
            self.start_workspace.set_asset_batch_value(field, value);
            cx.notify();
        }
    }

    /// Read every connection under the store lock, patch only the touched
    /// fields, and save it back one at a time.
    pub(in crate::features) fn save_asset_batch_edit(&mut self, cx: &mut Context<Self>) {
        if self.start_workspace.asset_batch_saving() {
            return;
        }
        let edit = match self.start_workspace.build_asset_batch_edit() {
            Ok(edit) => edit,
            Err(error) => {
                self.start_workspace
                    .set_asset_batch_error(Some(error.into()));
                cx.notify();
                return;
            }
        };
        let ids = self.start_workspace.asset_batch_ids().to_vec();
        self.start_workspace.set_asset_batch_saving(true);
        self.start_workspace.set_asset_batch_error(None);

        let request = nyaterm_store::store_mutation(StoreDomain::Connections, move |store| {
            let mut updated = 0usize;
            let mut skipped = 0usize;
            for id in &ids {
                let Some(mut connection) = store.get_connection(id)? else {
                    skipped += 1;
                    continue;
                };
                let report = apply_connection_batch_edit(&mut connection, &edit);
                if report.changed() {
                    store.save_connection(&connection)?;
                    updated += 1;
                } else {
                    skipped += 1;
                }
            }
            let sessions = store.load_sessions()?;
            Ok((sessions, updated, skipped))
        });

        let queued = self.submit_store_request(
            0,
            request,
            move |app, event, cx| {
                app.start_workspace.set_asset_batch_saving(false);
                match event.outcome {
                    Ok((sessions, updated, skipped)) => {
                        app.apply_loaded_sessions(sessions, cx);
                        app.shell.set_status(
                            t!("assets.batch.saved", updated = updated, skipped = skipped)
                                .to_string(),
                        );
                        app.close_asset_batch_edit(cx);
                    }
                    Err(error) => {
                        app.start_workspace.set_asset_batch_error(Some(
                            t!("assets.batch.saveFailed", error = error.to_string())
                                .to_string()
                                .into(),
                        ));
                    }
                }
                cx.notify();
            },
            cx,
        );
        if !queued {
            self.start_workspace.set_asset_batch_saving(false);
        }
    }
}

fn bool_text(value: bool) -> String {
    if value { "true".into() } else { "false".into() }
}

/// Human-readable label for the read-only endpoint list. Endpoint values are
/// configuration, never secrets.
fn match_endpoint_label(endpoint: &nyaterm_core::SshAgentEndpoint) -> String {
    match endpoint {
        nyaterm_core::SshAgentEndpoint::Auto => "Auto".to_string(),
        nyaterm_core::SshAgentEndpoint::Environment { variable } => {
            format!("Environment ({variable})")
        }
        nyaterm_core::SshAgentEndpoint::UnixSocket { path } => format!("Socket ({path})"),
        nyaterm_core::SshAgentEndpoint::WindowsOpenSsh => "Windows OpenSSH".to_string(),
        nyaterm_core::SshAgentEndpoint::Pageant => "Pageant".to_string(),
    }
}

/// Map the draft's uniform value onto the widget's option key. Mixed and
/// inapplicable fields have no selection at all.
fn select_initial_value(draft: &AssetBatchEditDraft, field: BatchFieldId) -> Option<String> {
    if draft.basis(field) != FieldBasis::Uniform {
        return None;
    }
    let value = draft.value(field);
    match field {
        BatchFieldId::Group => Some(value.to_string()),
        BatchFieldId::Encoding => Some(if value.is_empty() {
            "global".to_string()
        } else {
            value.to_string()
        }),
        BatchFieldId::SftpFilenameEncoding => Some(if value.is_empty() {
            "terminal".to_string()
        } else {
            value.to_string()
        }),
        BatchFieldId::RdpBackend => Some(
            match value {
                "internal" | "auto" => "builtin",
                "active-x" => "windows",
                other => other,
            }
            .to_string(),
        ),
        BatchFieldId::RdpDisplayMode => Some(
            match value {
                "fixed" | "native" => "fixed",
                _ => "fit-window",
            }
            .to_string(),
        ),
        // These selects carry an explicit empty-valued option (none / manual
        // identity / automatic / protocol default), so an empty uniform value
        // is a real selection rather than "no selection".
        BatchFieldId::Account
        | BatchFieldId::Key
        | BatchFieldId::Otp
        | BatchFieldId::Proxy
        | BatchFieldId::ProxyJump
        | BatchFieldId::TerminalType
        | BatchFieldId::SftpPipelineDepth => Some(value.to_string()),
        _ if value.is_empty() => None,
        _ => Some(value.to_string()),
    }
}

fn endpoint_values(config: &ConnectionType) -> Option<(&str, u16, &str)> {
    match config {
        ConnectionType::Ssh {
            host,
            port,
            username,
            ..
        }
        | ConnectionType::Telnet {
            host,
            port,
            username,
            ..
        }
        | ConnectionType::Rdp {
            host,
            port,
            username,
            ..
        }
        | ConnectionType::Vnc {
            host,
            port,
            username,
            ..
        } => Some((host.as_str(), *port, username.as_str())),
        _ => None,
    }
}

fn is_network_kind(kind: ConnectionBatchKind) -> bool {
    matches!(
        kind,
        ConnectionBatchKind::Ssh
            | ConnectionBatchKind::Telnet
            | ConnectionBatchKind::Rdp
            | ConnectionBatchKind::Vnc
    )
}

/// Fields whose meaning does not depend on the protocol.
pub(in crate::features) fn is_kind_independent(field: BatchFieldId) -> bool {
    matches!(
        field,
        BatchFieldId::Group
            | BatchFieldId::Description
            | BatchFieldId::Icon
            | BatchFieldId::IconAutoDetect
            | BatchFieldId::Name
            | BatchFieldId::Tags
            | BatchFieldId::RecordingUseGlobal
            | BatchFieldId::RecordingAutoStart
            | BatchFieldId::RecordingMode
    )
}

/// Whether the field exists in the connection editor for `kind`. Fields that
/// do not exist are skipped; they never add an extra row.
pub(in crate::features) fn batch_field_in_kind(
    kind: ConnectionBatchKind,
    field: BatchFieldId,
) -> bool {
    use ConnectionBatchKind::{LocalTerminal, Rdp, Serial, Ssh, Telnet, Vnc};
    if is_kind_independent(field) {
        return true;
    }
    match field {
        BatchFieldId::Host | BatchFieldId::Port | BatchFieldId::Username => is_network_kind(kind),
        BatchFieldId::Account | BatchFieldId::Password | BatchFieldId::AuthMode => {
            is_network_kind(kind)
        }
        BatchFieldId::Otp
        | BatchFieldId::OtpAutoFill
        | BatchFieldId::Key
        | BatchFieldId::SshAuthAgentEndpoint
        | BatchFieldId::SshAuthAgentEnvironment
        | BatchFieldId::SshAuthAgentSocket => kind == Ssh,
        BatchFieldId::Proxy | BatchFieldId::ProxyJump => is_network_kind(kind),
        BatchFieldId::ShellPath | BatchFieldId::ShellArgs | BatchFieldId::WorkingDir => {
            kind == LocalTerminal
        }
        BatchFieldId::SerialPort
        | BatchFieldId::BaudRate
        | BatchFieldId::DataBits
        | BatchFieldId::Parity
        | BatchFieldId::StopBits
        | BatchFieldId::FlowControl => kind == Serial,
        BatchFieldId::TelnetRawTcp
        | BatchFieldId::TelnetKeepAlive
        | BatchFieldId::TelnetLocalEcho
        | BatchFieldId::TelnetLocalLineEdit
        | BatchFieldId::TelnetForceChar
        | BatchFieldId::TelnetSendNaws
        | BatchFieldId::TelnetSendSga
        | BatchFieldId::TelnetEnterMode
        | BatchFieldId::TelnetAutoLoginEnabled
        | BatchFieldId::TelnetAutoLoginWake
        | BatchFieldId::TelnetAutoLoginTimeout
        | BatchFieldId::TelnetAutoLoginRetries
        | BatchFieldId::TelnetAutoLoginUserPrompt
        | BatchFieldId::TelnetAutoLoginPasswordPrompt
        | BatchFieldId::TelnetAutoLoginSuccessPrompt
        | BatchFieldId::TelnetAutoLoginFailurePrompt => kind == Telnet,
        BatchFieldId::RdpDomain
        | BatchFieldId::RdpBackend
        | BatchFieldId::RdpDisplayMode
        | BatchFieldId::RdpWidth
        | BatchFieldId::RdpHeight
        | BatchFieldId::RdpWallpaper
        | BatchFieldId::RdpThemes
        | BatchFieldId::RdpFontSmoothing
        | BatchFieldId::RdpDesktopComposition
        | BatchFieldId::RdpDisableFullWindowDrag
        | BatchFieldId::RdpDisableMenuAnimations
        | BatchFieldId::RdpDisableCursorShadow
        | BatchFieldId::RdpDisableCursorBlinking
        | BatchFieldId::RdpAutoReconnect
        | BatchFieldId::RdpReconnectAttempts => kind == Rdp,
        BatchFieldId::VncScaleMode
        | BatchFieldId::VncClipboard
        | BatchFieldId::VncShared
        | BatchFieldId::VncViewOnly
        | BatchFieldId::VncReconnect
        | BatchFieldId::VncReconnectAttempts => kind == Vnc,
        BatchFieldId::Encoding | BatchFieldId::Backspace | BatchFieldId::DynamicTabTitle => {
            matches!(kind, Ssh | LocalTerminal | Telnet | Serial)
        }
        BatchFieldId::SshProfile | BatchFieldId::TerminalType => kind == Ssh,
        BatchFieldId::SftpEnabled
        | BatchFieldId::SftpCompatibility
        | BatchFieldId::SftpCwdFollow
        | BatchFieldId::SftpShellTimeout
        | BatchFieldId::SftpFilenameEncoding
        | BatchFieldId::SftpPipelineDepth
        | BatchFieldId::X11
        | BatchFieldId::AgentForwarding
        | BatchFieldId::PostLoginEnabled
        | BatchFieldId::PostLoginCommand
        | BatchFieldId::PostLoginDelay
        | BatchFieldId::SshAlgorithmMode
        | BatchFieldId::SshKex
        | BatchFieldId::SshCiphers
        | BatchFieldId::SshMacs
        | BatchFieldId::SshHostKeys
        | BatchFieldId::AgentExternal
        | BatchFieldId::AgentStoredKeys
        | BatchFieldId::AgentPolicy
        | BatchFieldId::AgentEndpoints => kind == Ssh,
        BatchFieldId::RdpUseNla
        | BatchFieldId::RdpCertificatePolicy
        | BatchFieldId::RdpAdminSession => kind == Rdp,
        BatchFieldId::VncSecurityMode => kind == Vnc,
        BatchFieldId::Group
        | BatchFieldId::Description
        | BatchFieldId::Icon
        | BatchFieldId::IconAutoDetect
        | BatchFieldId::Name
        | BatchFieldId::Tags
        | BatchFieldId::RecordingUseGlobal
        | BatchFieldId::RecordingAutoStart
        | BatchFieldId::RecordingMode => true,
    }
}

/// The value a field starts at when a selected connection is switched to
/// `kind`: target-protocol defaults, never the old protocol's configuration.
fn kind_default_value(field: BatchFieldId, kind: ConnectionBatchKind) -> String {
    match field {
        BatchFieldId::Port => kind.default_port().to_string(),
        BatchFieldId::IconAutoDetect
        | BatchFieldId::SftpEnabled
        | BatchFieldId::RdpUseNla
        | BatchFieldId::RdpWallpaper
        | BatchFieldId::RdpThemes
        | BatchFieldId::RdpFontSmoothing
        | BatchFieldId::RdpDesktopComposition
        | BatchFieldId::RdpAutoReconnect
        | BatchFieldId::VncClipboard
        | BatchFieldId::VncShared
        | BatchFieldId::VncReconnect
        | BatchFieldId::RecordingUseGlobal
        | BatchFieldId::TelnetAutoLoginWake
        | BatchFieldId::TelnetSendNaws
        | BatchFieldId::TelnetSendSga => bool_text(true),
        BatchFieldId::SftpCwdFollow => "off".to_string(),
        BatchFieldId::SftpShellTimeout => nyaterm_core::SftpSettings::default()
            .shell_detection_timeout_ms
            .to_string(),
        BatchFieldId::SftpFilenameEncoding => "terminal".to_string(),
        BatchFieldId::SshProfile => "standard".to_string(),
        BatchFieldId::SshAlgorithmMode => "compatible".to_string(),
        BatchFieldId::RdpCertificatePolicy => "prompt".to_string(),
        BatchFieldId::VncSecurityMode => "auto".to_string(),
        BatchFieldId::RdpBackend => "builtin".to_string(),
        BatchFieldId::RdpDisplayMode => "fit-window".to_string(),
        BatchFieldId::RdpWidth => "1920".to_string(),
        BatchFieldId::RdpHeight => "1080".to_string(),
        BatchFieldId::VncScaleMode => "fit".to_string(),
        BatchFieldId::RdpReconnectAttempts | BatchFieldId::VncReconnectAttempts => "3".to_string(),
        BatchFieldId::AuthMode => "password".to_string(),
        BatchFieldId::RecordingMode => "transcript".to_string(),
        BatchFieldId::BaudRate => "115200".to_string(),
        BatchFieldId::DataBits => "8".to_string(),
        BatchFieldId::Parity => "none".to_string(),
        BatchFieldId::StopBits => "1".to_string(),
        BatchFieldId::FlowControl => "none".to_string(),
        BatchFieldId::TelnetEnterMode => "crlf".to_string(),
        BatchFieldId::TelnetKeepAlive => "60".to_string(),
        BatchFieldId::TelnetAutoLoginTimeout => "60000".to_string(),
        BatchFieldId::TelnetAutoLoginRetries => "0".to_string(),
        _ if field.is_bool() => bool_text(false),
        _ => String::new(),
    }
}

fn agent_forwarding_value<T>(
    connection: &SavedConnection,
    read: impl Fn(
        &nyaterm_core::SshAgentForwardingSources,
        &nyaterm_core::SshAgentForwardingConfig,
    ) -> T,
) -> Option<T> {
    match &connection.config {
        ConnectionType::Ssh {
            agent_forwarding_config,
            ..
        } => {
            let default = nyaterm_core::SshAgentForwardingConfig::default();
            let config = agent_forwarding_config.as_ref().unwrap_or(&default);
            Some(read(&config.sources, config))
        }
        _ => None,
    }
}

fn config_encoding(config: &ConnectionType) -> Option<&str> {
    match config {
        ConnectionType::Ssh { encoding, .. }
        | ConnectionType::LocalTerminal { encoding, .. }
        | ConnectionType::Telnet { encoding, .. }
        | ConnectionType::Serial { encoding, .. } => Some(encoding.as_str()),
        _ => None,
    }
}

fn config_backspace(config: &ConnectionType) -> Option<&str> {
    match config {
        ConnectionType::Ssh { backspace_mode, .. }
        | ConnectionType::Telnet { backspace_mode, .. }
        | ConnectionType::Serial { backspace_mode, .. } => Some(backspace_mode.as_str()),
        _ => None,
    }
}

fn config_dynamic_tab_title(config: &ConnectionType) -> Option<bool> {
    match config {
        ConnectionType::Ssh {
            dynamic_tab_title, ..
        }
        | ConnectionType::LocalTerminal {
            dynamic_tab_title, ..
        } => Some(*dynamic_tab_title),
        _ => None,
    }
}

/// One field's value as the batch editor displays it, or `None` when the
/// connection's protocol does not support the field.
fn field_display_value(connection: &SavedConnection, field: BatchFieldId) -> Option<String> {
    let is_ssh = matches!(&connection.config, ConnectionType::Ssh { .. });
    match field {
        BatchFieldId::Name => Some(connection.name.clone()),
        BatchFieldId::Tags => Some(connection.tags.join(", ")),
        BatchFieldId::Host => {
            endpoint_values(&connection.config).map(|(host, _, _)| host.to_string())
        }
        BatchFieldId::Account => Some(
            connection
                .auth
                .as_ref()
                .and_then(|auth| auth.saved_account_id())
                .unwrap_or_default()
                .to_string(),
        ),
        BatchFieldId::Password => Some(match connection.auth.as_ref() {
            Some(auth)
                if auth
                    .password_id
                    .as_deref()
                    .is_some_and(|id| !id.trim().is_empty())
                    || auth.has_password
                    || auth.password.is_some() =>
            {
                t!("assets.batch.passwordSet").to_string()
            }
            _ => t!("assets.batch.passwordUnset").to_string(),
        }),
        BatchFieldId::AuthMode => Some(
            connection
                .auth
                .as_ref()
                .map(|auth| auth.mode.clone())
                .filter(|mode| !mode.is_empty())
                .unwrap_or_else(|| "password".to_string()),
        ),
        BatchFieldId::Otp => Some(
            connection
                .auth
                .as_ref()
                .and_then(|auth| auth.otp_id.as_deref())
                .unwrap_or_default()
                .to_string(),
        ),
        BatchFieldId::OtpAutoFill => Some(bool_text(
            connection
                .auth
                .as_ref()
                .is_some_and(|auth| auth.auto_fill_otp),
        )),
        BatchFieldId::Key => Some(
            connection
                .auth
                .as_ref()
                .and_then(|auth| auth.key_id.as_deref())
                .unwrap_or_default()
                .to_string(),
        ),
        BatchFieldId::SshAuthAgentEndpoint => match &connection.config {
            ConnectionType::Ssh {
                auth_agent_endpoint,
                ..
            } => Some(match auth_agent_endpoint {
                None | Some(nyaterm_core::SshAgentEndpoint::Auto) => "auto".to_string(),
                Some(nyaterm_core::SshAgentEndpoint::Environment { .. }) => {
                    "environment".to_string()
                }
                Some(nyaterm_core::SshAgentEndpoint::UnixSocket { .. }) => {
                    "unix_socket".to_string()
                }
                Some(nyaterm_core::SshAgentEndpoint::Pageant) => "pageant".to_string(),
                Some(nyaterm_core::SshAgentEndpoint::WindowsOpenSsh) => {
                    "windows_openssh".to_string()
                }
            }),
            _ => None,
        },
        // Both text fields stay applicable for SSH even when the endpoint type
        // currently selects the other one: the user may switch the type and the
        // value has to be editable in the same draft.
        BatchFieldId::SshAuthAgentEnvironment => match &connection.config {
            ConnectionType::Ssh {
                auth_agent_endpoint,
                ..
            } => Some(match auth_agent_endpoint {
                Some(nyaterm_core::SshAgentEndpoint::Environment { variable }) => variable.clone(),
                _ => String::new(),
            }),
            _ => None,
        },
        BatchFieldId::SshAuthAgentSocket => match &connection.config {
            ConnectionType::Ssh {
                auth_agent_endpoint,
                ..
            } => Some(match auth_agent_endpoint {
                Some(nyaterm_core::SshAgentEndpoint::UnixSocket { path }) => path.clone(),
                _ => String::new(),
            }),
            _ => None,
        },
        BatchFieldId::Proxy => Some(
            connection
                .network
                .as_ref()
                .and_then(|network| network.proxy_id.as_deref())
                .unwrap_or_default()
                .to_string(),
        ),
        BatchFieldId::ProxyJump => Some(
            connection
                .network
                .as_ref()
                .and_then(|network| network.proxy_jump_id.as_deref())
                .unwrap_or_default()
                .to_string(),
        ),
        BatchFieldId::ShellPath => match &connection.config {
            ConnectionType::LocalTerminal { shell_path, .. } => Some(shell_path.clone()),
            _ => None,
        },
        BatchFieldId::ShellArgs => match &connection.config {
            ConnectionType::LocalTerminal { shell_args, .. } => Some(shell_args.clone()),
            _ => None,
        },
        BatchFieldId::WorkingDir => match &connection.config {
            ConnectionType::LocalTerminal { working_dir, .. } => {
                Some(working_dir.clone().unwrap_or_default())
            }
            _ => None,
        },
        BatchFieldId::SerialPort => match &connection.config {
            ConnectionType::Serial { port_name, .. } => Some(port_name.clone()),
            _ => None,
        },
        BatchFieldId::BaudRate => match &connection.config {
            ConnectionType::Serial { baud_rate, .. } => Some(baud_rate.to_string()),
            _ => None,
        },
        BatchFieldId::DataBits => match &connection.config {
            ConnectionType::Serial { data_bits, .. } => Some(data_bits.to_string()),
            _ => None,
        },
        BatchFieldId::Parity => match &connection.config {
            ConnectionType::Serial { parity, .. } => Some(parity.clone()),
            _ => None,
        },
        BatchFieldId::StopBits => match &connection.config {
            ConnectionType::Serial { stop_bits, .. } => Some(stop_bits.clone()),
            _ => None,
        },
        BatchFieldId::FlowControl => match &connection.config {
            ConnectionType::Serial { flow_control, .. } => Some(flow_control.as_str().to_string()),
            _ => None,
        },
        BatchFieldId::TelnetRawTcp => match &connection.config {
            ConnectionType::Telnet { raw_tcp_cli, .. } => Some(bool_text(*raw_tcp_cli)),
            _ => None,
        },
        BatchFieldId::TelnetKeepAlive => match &connection.config {
            ConnectionType::Telnet {
                keep_alive_interval_secs,
                ..
            } => Some(keep_alive_interval_secs.to_string()),
            _ => None,
        },
        BatchFieldId::TelnetLocalEcho => match &connection.config {
            ConnectionType::Telnet { local_echo, .. } => Some(bool_text(*local_echo)),
            _ => None,
        },
        BatchFieldId::TelnetLocalLineEdit => match &connection.config {
            ConnectionType::Telnet {
                local_line_edit, ..
            } => Some(bool_text(*local_line_edit)),
            _ => None,
        },
        BatchFieldId::TelnetForceChar => match &connection.config {
            ConnectionType::Telnet {
                force_character_at_a_time,
                ..
            } => Some(bool_text(*force_character_at_a_time)),
            _ => None,
        },
        BatchFieldId::TelnetSendNaws => match &connection.config {
            ConnectionType::Telnet { send_naws, .. } => Some(bool_text(*send_naws)),
            _ => None,
        },
        BatchFieldId::TelnetSendSga => match &connection.config {
            ConnectionType::Telnet { send_sga, .. } => Some(bool_text(*send_sga)),
            _ => None,
        },
        BatchFieldId::TelnetEnterMode => match &connection.config {
            ConnectionType::Telnet { enter_mode, .. } => Some(enter_mode.clone()),
            _ => None,
        },
        BatchFieldId::TelnetAutoLoginEnabled => match &connection.config {
            ConnectionType::Telnet { auto_login, .. } => Some(bool_text(auto_login.enabled)),
            _ => None,
        },
        BatchFieldId::TelnetAutoLoginWake => match &connection.config {
            ConnectionType::Telnet { auto_login, .. } => {
                Some(bool_text(auto_login.send_wake_enter))
            }
            _ => None,
        },
        BatchFieldId::TelnetAutoLoginTimeout => match &connection.config {
            ConnectionType::Telnet { auto_login, .. } => Some(auto_login.timeout_ms.to_string()),
            _ => None,
        },
        BatchFieldId::TelnetAutoLoginRetries => match &connection.config {
            ConnectionType::Telnet { auto_login, .. } => Some(auto_login.max_retries.to_string()),
            _ => None,
        },
        BatchFieldId::TelnetAutoLoginUserPrompt => match &connection.config {
            ConnectionType::Telnet { auto_login, .. } => {
                Some(auto_login.username_prompt_regex.clone().unwrap_or_default())
            }
            _ => None,
        },
        BatchFieldId::TelnetAutoLoginPasswordPrompt => match &connection.config {
            ConnectionType::Telnet { auto_login, .. } => {
                Some(auto_login.password_prompt_regex.clone().unwrap_or_default())
            }
            _ => None,
        },
        BatchFieldId::TelnetAutoLoginSuccessPrompt => match &connection.config {
            ConnectionType::Telnet { auto_login, .. } => {
                Some(auto_login.success_prompt_regex.clone().unwrap_or_default())
            }
            _ => None,
        },
        BatchFieldId::TelnetAutoLoginFailurePrompt => match &connection.config {
            ConnectionType::Telnet { auto_login, .. } => {
                Some(auto_login.failure_prompt_regex.clone().unwrap_or_default())
            }
            _ => None,
        },
        BatchFieldId::RdpDomain => match &connection.config {
            ConnectionType::Rdp { domain, .. } => Some(domain.clone()),
            _ => None,
        },
        BatchFieldId::RdpBackend => match &connection.config {
            ConnectionType::Rdp { backend, .. } => Some(backend.clone()),
            _ => None,
        },
        BatchFieldId::RdpDisplayMode => match &connection.config {
            ConnectionType::Rdp { display, .. } => Some(display.mode.clone()),
            _ => None,
        },
        BatchFieldId::RdpWidth => match &connection.config {
            ConnectionType::Rdp { display, .. } => Some(display.width.to_string()),
            _ => None,
        },
        BatchFieldId::RdpHeight => match &connection.config {
            ConnectionType::Rdp { display, .. } => Some(display.height.to_string()),
            _ => None,
        },
        BatchFieldId::RdpWallpaper => match &connection.config {
            ConnectionType::Rdp { display, .. } => Some(bool_text(display.display_wallpaper)),
            _ => None,
        },
        BatchFieldId::RdpThemes => match &connection.config {
            ConnectionType::Rdp { display, .. } => Some(bool_text(display.display_themes)),
            _ => None,
        },
        BatchFieldId::RdpFontSmoothing => match &connection.config {
            ConnectionType::Rdp { display, .. } => Some(bool_text(display.enable_font_smoothing)),
            _ => None,
        },
        BatchFieldId::RdpDesktopComposition => match &connection.config {
            ConnectionType::Rdp { display, .. } => {
                Some(bool_text(display.enable_desktop_composition))
            }
            _ => None,
        },
        BatchFieldId::RdpDisableFullWindowDrag => match &connection.config {
            ConnectionType::Rdp { display, .. } => {
                Some(bool_text(display.disable_full_window_drag))
            }
            _ => None,
        },
        BatchFieldId::RdpDisableMenuAnimations => match &connection.config {
            ConnectionType::Rdp { display, .. } => Some(bool_text(display.disable_menu_animations)),
            _ => None,
        },
        BatchFieldId::RdpDisableCursorShadow => match &connection.config {
            ConnectionType::Rdp { display, .. } => Some(bool_text(display.disable_cursor_shadow)),
            _ => None,
        },
        BatchFieldId::RdpDisableCursorBlinking => match &connection.config {
            ConnectionType::Rdp { display, .. } => Some(bool_text(display.disable_cursor_blinking)),
            _ => None,
        },
        BatchFieldId::RdpAutoReconnect => match &connection.config {
            ConnectionType::Rdp { reconnect, .. } => Some(bool_text(reconnect.enabled)),
            _ => None,
        },
        BatchFieldId::RdpReconnectAttempts => match &connection.config {
            ConnectionType::Rdp { reconnect, .. } => Some(reconnect.max_attempts.to_string()),
            _ => None,
        },
        BatchFieldId::VncScaleMode => match &connection.config {
            ConnectionType::Vnc { display, .. } => Some(display.scale_mode.clone()),
            _ => None,
        },
        BatchFieldId::VncClipboard => match &connection.config {
            ConnectionType::Vnc { clipboard, .. } => Some(bool_text(clipboard.enabled)),
            _ => None,
        },
        BatchFieldId::VncShared => match &connection.config {
            ConnectionType::Vnc { shared, .. } => Some(bool_text(*shared)),
            _ => None,
        },
        BatchFieldId::VncViewOnly => match &connection.config {
            ConnectionType::Vnc { view_only, .. } => Some(bool_text(*view_only)),
            _ => None,
        },
        BatchFieldId::VncReconnect => match &connection.config {
            ConnectionType::Vnc { reconnect, .. } => Some(bool_text(reconnect.enabled)),
            _ => None,
        },
        BatchFieldId::VncReconnectAttempts => match &connection.config {
            ConnectionType::Vnc { reconnect, .. } => Some(reconnect.max_attempts.to_string()),
            _ => None,
        },
        BatchFieldId::RecordingUseGlobal => Some(bool_text(connection.recording.is_none())),
        BatchFieldId::RecordingAutoStart => Some(bool_text(
            connection
                .recording
                .as_ref()
                .and_then(|recording| recording.auto_start)
                .unwrap_or(false),
        )),
        BatchFieldId::RecordingMode => Some(
            match connection
                .recording
                .as_ref()
                .and_then(|recording| recording.mode)
            {
                Some(nyaterm_core::RecordingMode::Raw) => "raw",
                _ => "transcript",
            }
            .to_string(),
        ),
        BatchFieldId::AgentExternal => {
            agent_forwarding_value(connection, |sources, _| bool_text(sources.external_agent))
        }
        BatchFieldId::AgentStoredKeys => {
            agent_forwarding_value(connection, |sources, _| bool_text(sources.stored_keys))
        }
        BatchFieldId::AgentPolicy => agent_forwarding_value(connection, |_, config| {
            match config.policy {
                nyaterm_core::SshAgentForwardingPolicy::All => "all",
                nyaterm_core::SshAgentForwardingPolicy::Allowlist { .. } => "allowlist",
            }
            .to_string()
        }),
        // The endpoint list is a composite read-only surface, so it keeps a
        // descriptive summary instead of a single scalar control value.
        BatchFieldId::AgentEndpoints => agent_forwarding_value(connection, |sources, _| {
            sources
                .external_agent_endpoints
                .iter()
                .map(match_endpoint_label)
                .collect::<Vec<_>>()
                .join(", ")
        }),
        BatchFieldId::Group => Some(connection.group_id.clone().unwrap_or_default()),
        BatchFieldId::Description => Some(connection.description.clone().unwrap_or_default()),
        BatchFieldId::Icon => Some(
            connection
                .icon
                .clone()
                .unwrap_or_else(|| view::default_icon_value().unwrap_or_default()),
        ),
        BatchFieldId::IconAutoDetect => Some(bool_text(connection.icon_auto_detect_enabled())),
        BatchFieldId::Port => {
            endpoint_values(&connection.config).map(|(_, port, _)| port.to_string())
        }
        BatchFieldId::Username => {
            endpoint_values(&connection.config).map(|(_, _, username)| username.to_string())
        }
        BatchFieldId::Encoding => config_encoding(&connection.config).map(str::to_string),
        BatchFieldId::Backspace => config_backspace(&connection.config).map(str::to_string),
        BatchFieldId::SshProfile => {
            if !is_ssh {
                return None;
            }
            Some(match connection.ssh_profile {
                SshProfile::Standard => "standard".to_string(),
                SshProfile::NetworkDevice => "network_device".to_string(),
            })
        }
        BatchFieldId::TerminalType => {
            if !is_ssh {
                return None;
            }
            Some(
                connection
                    .terminal_type
                    .map(SshTerminalType::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        }
        BatchFieldId::DynamicTabTitle => {
            config_dynamic_tab_title(&connection.config).map(bool_text)
        }
        BatchFieldId::SftpEnabled => {
            if !is_ssh {
                return None;
            }
            Some(bool_text(connection.sftp.enabled))
        }
        BatchFieldId::SftpCompatibility => {
            if !is_ssh {
                return None;
            }
            Some(bool_text(connection.sftp.compatibility_mode))
        }
        BatchFieldId::SftpCwdFollow => {
            if !is_ssh {
                return None;
            }
            Some(
                match connection.sftp.cwd_follow_mode {
                    SftpCwdFollowMode::Off => "off",
                    SftpCwdFollowMode::ShellIntegration => "shell_integration",
                    SftpCwdFollowMode::RcFile => "rc_file",
                }
                .to_string(),
            )
        }
        BatchFieldId::SftpShellTimeout => {
            if !is_ssh {
                return None;
            }
            Some(connection.sftp.shell_detection_timeout_ms.to_string())
        }
        BatchFieldId::SftpFilenameEncoding => {
            if !is_ssh {
                return None;
            }
            Some(connection.sftp.filename_encoding.clone())
        }
        BatchFieldId::SftpPipelineDepth => {
            if !is_ssh {
                return None;
            }
            Some(
                connection
                    .sftp
                    .pipeline_depth
                    .map(|depth| depth.to_string())
                    .unwrap_or_default(),
            )
        }
        BatchFieldId::X11 => match &connection.config {
            ConnectionType::Ssh { x11_forwarding, .. } => Some(bool_text(*x11_forwarding)),
            _ => None,
        },
        BatchFieldId::AgentForwarding => match &connection.config {
            ConnectionType::Ssh {
                agent_forwarding_config,
                legacy_agent_forwarding,
                ..
            } => Some(bool_text(
                agent_forwarding_config
                    .as_ref()
                    .map(|config| config.enabled)
                    .or(*legacy_agent_forwarding)
                    .unwrap_or(false),
            )),
            _ => None,
        },
        BatchFieldId::PostLoginEnabled => {
            if !is_ssh {
                return None;
            }
            Some(bool_text(
                connection
                    .post_login
                    .as_ref()
                    .is_some_and(|login| login.enabled),
            ))
        }
        BatchFieldId::PostLoginCommand => {
            if !is_ssh {
                return None;
            }
            Some(
                connection
                    .post_login
                    .as_ref()
                    .map(|login| login.command.clone())
                    .unwrap_or_default(),
            )
        }
        BatchFieldId::PostLoginDelay => {
            if !is_ssh {
                return None;
            }
            Some(
                connection
                    .post_login
                    .as_ref()
                    .map(|login| login.delay_ms)
                    .unwrap_or_default()
                    .to_string(),
            )
        }
        BatchFieldId::SshAlgorithmMode => {
            if !is_ssh {
                return None;
            }
            Some(
                match connection
                    .ssh_algorithms
                    .as_ref()
                    .map(|algorithms| algorithms.mode.clone())
                    .unwrap_or_default()
                {
                    SshAlgorithmMode::Compatible => "compatible",
                    SshAlgorithmMode::Secure => "secure",
                    SshAlgorithmMode::Custom => "custom",
                }
                .to_string(),
            )
        }
        BatchFieldId::SshKex => ssh_algorithm_list(connection, |algorithms| &algorithms.kex),
        BatchFieldId::SshCiphers => {
            ssh_algorithm_list(connection, |algorithms| &algorithms.ciphers)
        }
        BatchFieldId::SshMacs => ssh_algorithm_list(connection, |algorithms| &algorithms.macs),
        BatchFieldId::SshHostKeys => {
            ssh_algorithm_list(connection, |algorithms| &algorithms.host_keys)
        }
        BatchFieldId::RdpUseNla => match &connection.config {
            ConnectionType::Rdp { security, .. } => Some(bool_text(security.use_nla)),
            _ => None,
        },
        BatchFieldId::RdpCertificatePolicy => match &connection.config {
            ConnectionType::Rdp { security, .. } => Some(security.certificate_policy.clone()),
            _ => None,
        },
        BatchFieldId::RdpAdminSession => match &connection.config {
            ConnectionType::Rdp { security, .. } => {
                Some(bool_text(security.administrative_session))
            }
            _ => None,
        },
        BatchFieldId::VncSecurityMode => match &connection.config {
            ConnectionType::Vnc { security, .. } => Some(security.mode.clone()),
            _ => None,
        },
    }
}

fn ssh_algorithm_list(
    connection: &SavedConnection,
    pick: impl Fn(&nyaterm_core::SshAlgorithmPreferences) -> &[String],
) -> Option<String> {
    if !matches!(&connection.config, ConnectionType::Ssh { .. }) {
        return None;
    }
    let list = connection
        .ssh_algorithms
        .as_ref()
        .map(&pick)
        .unwrap_or_default();
    Some(list.join(", "))
}

fn is_shared_endpoint(field: BatchFieldId) -> bool {
    matches!(
        field,
        BatchFieldId::Host | BatchFieldId::Port | BatchFieldId::Username
    )
}

/// A selected connection's value as it would stand under `target`. Connections
/// that already use the target keep their value; connections of another
/// protocol contribute the target's default, except for the protocol
/// independent fields the batch switch preserves.
fn field_value_for_target(
    connection: &SavedConnection,
    field: BatchFieldId,
    target: ConnectionBatchKind,
) -> Option<String> {
    if !batch_field_in_kind(target, field) {
        return None;
    }
    let actual = ConnectionBatchKind::from_config(&connection.config);
    let carried = is_kind_independent(field)
        || (is_shared_endpoint(field) && is_network_kind(actual) && is_network_kind(target));
    if actual == target || carried {
        field_display_value(connection, field).or_else(|| Some(kind_default_value(field, target)))
    } else {
        Some(kind_default_value(field, target))
    }
}

/// Protocol agreement, the shared protocol, per-field basis, and the values
/// shown for `target`'s form.
type BatchBasis = (
    FieldBasis,
    Option<ConnectionBatchKind>,
    HashMap<BatchFieldId, FieldBasis>,
    HashMap<BatchFieldId, String>,
    HashMap<BatchFieldId, bool>,
);

/// Compare each field across the selection under `target`. When the selection
/// spans protocols and the caller has not chosen one, the maps are empty and
/// the view shows the mixed prompt instead of the fields.
fn compute_batch_basis(
    connections: &[SavedConnection],
    ids: &[String],
    target: Option<ConnectionBatchKind>,
) -> BatchBasis {
    let by_id: HashMap<&str, &SavedConnection> = connections
        .iter()
        .map(|connection| (connection.id.as_str(), connection))
        .collect();

    let mut selected_kinds: Vec<ConnectionBatchKind> = ids
        .iter()
        .filter_map(|id| by_id.get(id.as_str()))
        .map(|connection| ConnectionBatchKind::from_config(&connection.config))
        .collect();
    selected_kinds.dedup();

    let uniform_kind = selected_kinds.first().copied();
    let kind_basis = if selected_kinds.len() > 1 || selected_kinds.is_empty() {
        if selected_kinds.len() > 1 {
            FieldBasis::Mixed
        } else {
            FieldBasis::Inapplicable
        }
    } else {
        FieldBasis::Uniform
    };

    let mut basis = HashMap::new();
    let mut values = HashMap::new();
    let mut bool_values = HashMap::new();
    let Some(target) = target.or(uniform_kind) else {
        return (kind_basis, uniform_kind, basis, values, bool_values);
    };

    for field in EDITABLE_FIELDS.into_iter().chain(DISPLAY_FIELDS) {
        if !batch_field_in_kind(target, field) {
            basis.insert(field, FieldBasis::Inapplicable);
            continue;
        }
        let mut seen: Option<String> = None;
        let mut mixed = false;
        for id in ids {
            let Some(connection) = by_id.get(id.as_str()) else {
                continue;
            };
            let Some(value) = field_value_for_target(connection, field, target) else {
                continue;
            };
            match &seen {
                None => seen = Some(value),
                Some(existing) if *existing != value => {
                    mixed = true;
                    break;
                }
                _ => {}
            }
        }

        match (seen, mixed) {
            (None, _) => {
                basis.insert(field, FieldBasis::Inapplicable);
            }
            (Some(_), true) => {
                basis.insert(field, FieldBasis::Mixed);
            }
            (Some(value), false) => {
                basis.insert(field, FieldBasis::Uniform);
                if field.is_bool() {
                    bool_values.insert(field, value == "true");
                } else {
                    values.insert(field, value);
                }
            }
        }
    }

    (kind_basis, uniform_kind, basis, values, bool_values)
}

/// Map a batch protocol onto the editor's tab, which also supplies the section
/// list and labels the batch form mirrors.
pub(in crate::features) fn batch_kind_tab(kind: ConnectionBatchKind) -> ConnectionKindTab {
    match kind {
        ConnectionBatchKind::Ssh => ConnectionKindTab::Ssh,
        ConnectionBatchKind::LocalTerminal => ConnectionKindTab::Local,
        ConnectionBatchKind::Telnet => ConnectionKindTab::Telnet,
        ConnectionBatchKind::Serial => ConnectionKindTab::Serial,
        ConnectionBatchKind::Rdp => ConnectionKindTab::Rdp,
        ConnectionBatchKind::Vnc => ConnectionKindTab::Vnc,
    }
}

pub(in crate::features) const BATCH_KINDS: [ConnectionBatchKind; 6] = [
    ConnectionBatchKind::Ssh,
    ConnectionBatchKind::Telnet,
    ConnectionBatchKind::Rdp,
    ConnectionBatchKind::Vnc,
    ConnectionBatchKind::LocalTerminal,
    ConnectionBatchKind::Serial,
];

pub(in crate::features) fn batch_kind_value(kind: ConnectionBatchKind) -> &'static str {
    match kind {
        ConnectionBatchKind::Ssh => "ssh",
        ConnectionBatchKind::Telnet => "telnet",
        ConnectionBatchKind::Rdp => "rdp",
        ConnectionBatchKind::Vnc => "vnc",
        ConnectionBatchKind::LocalTerminal => "local",
        ConnectionBatchKind::Serial => "serial",
    }
}

pub(in crate::features) fn parse_batch_kind(value: &str) -> Option<ConnectionBatchKind> {
    BATCH_KINDS
        .into_iter()
        .find(|kind| batch_kind_value(*kind) == value)
}

#[cfg(test)]
mod tests;
