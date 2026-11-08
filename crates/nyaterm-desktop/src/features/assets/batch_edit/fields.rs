use super::{ASSET_BATCH_SELECT_PREFIX, ASSET_BATCH_TEXT_PREFIX};
use gpui::SharedString;
use nyaterm_core::models::connection_batch::RdpBatchFieldGroup;
/// What the selected connections currently hold for one field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::features) enum FieldBasis {
    /// No selected connection supports this field (Local/Serial, non-SSH, ...).
    #[default]
    Inapplicable,
    /// Selected connections differ; Keep preserves each original value.
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
    pub(super) fn rdp_group(self) -> Option<RdpBatchFieldGroup> {
        match self {
            Self::RdpDomain | Self::RdpBackend => Some(RdpBatchFieldGroup::General),
            Self::RdpWidth | Self::RdpHeight => Some(RdpBatchFieldGroup::Dimensions),
            Self::RdpUseNla
            | Self::RdpCertificatePolicy
            | Self::RdpAdminSession
            | Self::RdpDisplayMode
            | Self::RdpWallpaper
            | Self::RdpThemes
            | Self::RdpFontSmoothing
            | Self::RdpDesktopComposition
            | Self::RdpDisableFullWindowDrag
            | Self::RdpDisableMenuAnimations
            | Self::RdpDisableCursorShadow
            | Self::RdpDisableCursorBlinking
            | Self::RdpAutoReconnect
            | Self::RdpReconnectAttempts => Some(RdpBatchFieldGroup::SecurityAndDisplay),
            _ => None,
        }
    }

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
            Self::Encoding => "connection.encoding",
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
