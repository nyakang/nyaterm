use super::{ASSET_BATCH_SELECT_PREFIX, ASSET_BATCH_TEXT_PREFIX};
use crate::models::{ConnectionEditorSection, ConnectionKindTab};
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
pub(in crate::features) enum BatchFieldKey {
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
    SshCompatibility,
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
    AllowShortRsaHostKeys,
    RdpUseNla,
    RdpCertificatePolicy,
    RdpAdminSession,
    VncSecurityMode,

    AdbDirectory,
    AdbScreenQuality,
    AdbScreenMaxSize,
    AdbScreenFps,
    AdbScreenBitRate,
    AdbScreenPowerOn,
    AdbScreenOrientation,
    FtpInitialDirectory,
    FtpLocalInitialDirectory,
    FtpPassiveMode,
    FtpFilenameEncodingMode,
    FtpFilenameEncoding,
    FtpAsciiMode,
    FtpSecurityMode,
    FtpTlsVersion,
    FtpConnectTimeout,

    Name,
    Host,
    AdbSerial,
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
    RdpRedirectClipboard,
    RdpRedirectDrives,
    RdpDriveLetters,
    RdpCustomDrivePath,
    RdpRedirectPrinters,
    RdpRedirectPorts,
    RdpRedirectSmartcards,
    RdpRedirectMicrophone,
    RdpRedirectWebauthn,
    RdpAudio,
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

impl BatchFieldKey {
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
            | Self::RdpReconnectAttempts
            | Self::RdpRedirectClipboard
            | Self::RdpRedirectDrives
            | Self::RdpDriveLetters
            | Self::RdpCustomDrivePath
            | Self::RdpRedirectPrinters
            | Self::RdpRedirectPorts
            | Self::RdpRedirectSmartcards
            | Self::RdpRedirectMicrophone
            | Self::RdpRedirectWebauthn
            | Self::RdpAudio => Some(RdpBatchFieldGroup::SecurityAndDisplay),
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
            Self::SshCompatibility => "ssh-compatibility",
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
            Self::AllowShortRsaHostKeys => "allow-short-rsa-host-keys",
            Self::RdpUseNla => "rdp-use-nla",
            Self::RdpCertificatePolicy => "rdp-certificate-policy",
            Self::RdpAdminSession => "rdp-admin-session",
            Self::VncSecurityMode => "vnc-security-mode",
            Self::AdbDirectory => "adb-directory",
            Self::AdbScreenQuality => "adb-screen-quality",
            Self::AdbScreenMaxSize => "adb-screen-max-size",
            Self::AdbScreenFps => "adb-screen-max-fps",
            Self::AdbScreenBitRate => "adb-screen-bit-rate",
            Self::AdbScreenPowerOn => "adb-screen-power-on",
            Self::AdbScreenOrientation => "adb-screen-orientation",
            Self::FtpInitialDirectory => "ftp-initial-directory",
            Self::FtpLocalInitialDirectory => "ftp-local-initial-directory",
            Self::FtpPassiveMode => "ftp-passive-mode",
            Self::FtpFilenameEncodingMode => "ftp-filename-encoding-mode",
            Self::FtpFilenameEncoding => "ftp-filename-encoding",
            Self::FtpAsciiMode => "ftp-ascii-mode",
            Self::FtpSecurityMode => "ftp-security-mode",
            Self::FtpTlsVersion => "ftp-tls-version",
            Self::FtpConnectTimeout => "ftp-connect-timeout",
            Self::Name => "display.name",
            Self::Host => "display.host",
            Self::AdbSerial => "display.adb-serial",
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
            Self::RdpRedirectClipboard => "display.rdp-redirect-clipboard",
            Self::RdpRedirectDrives => "display.rdp-redirect-drives",
            Self::RdpDriveLetters => "display.rdp-drive-letters",
            Self::RdpCustomDrivePath => "display.rdp-custom-drive-path",
            Self::RdpRedirectPrinters => "display.rdp-redirect-printers",
            Self::RdpRedirectPorts => "display.rdp-redirect-ports",
            Self::RdpRedirectSmartcards => "display.rdp-redirect-smartcards",
            Self::RdpRedirectMicrophone => "display.rdp-redirect-microphone",
            Self::RdpRedirectWebauthn => "display.rdp-redirect-webauthn",
            Self::RdpAudio => "display.rdp-audio",
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
            Self::SshCompatibility => "dialog.sshCompatibilityMode",
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
            Self::AllowShortRsaHostKeys => "dialog.allowShortRsaHostKeys",
            Self::RdpUseNla => "dialog.rdpUseNla",
            Self::RdpCertificatePolicy => "dialog.rdpCertificatePolicy",
            Self::RdpAdminSession => "dialog.rdpAdministrativeSession",
            Self::VncSecurityMode => "dialog.vncSecurityMode",
            Self::AdbDirectory => "android.directory",
            Self::AdbScreenQuality => "android.qualityPreset",
            Self::AdbScreenMaxSize => "android.maxSize",
            Self::AdbScreenFps => "android.maxFps",
            Self::AdbScreenBitRate => "android.bitRate",
            Self::AdbScreenPowerOn => "android.powerOn",
            Self::AdbScreenOrientation => "android.orientation",
            Self::FtpInitialDirectory => "dialog.ftpInitialDirectory",
            Self::FtpLocalInitialDirectory => "dialog.ftpLocalInitialDirectory",
            Self::FtpPassiveMode => "dialog.ftpPassiveMode",
            Self::FtpFilenameEncodingMode => "dialog.filenameEncodingTitle",
            Self::FtpFilenameEncoding => "dialog.filenameEncodingLabel",
            Self::FtpAsciiMode => "dialog.ftpAsciiMode",
            Self::FtpSecurityMode => "dialog.ftpSecurityMode",
            Self::FtpTlsVersion => "dialog.ftpTlsVersion",
            Self::FtpConnectTimeout => "dialog.ftpConnectTimeout",
            Self::Name => "dialog.connectionName",
            Self::Host => "dialog.host",
            Self::AdbSerial => "android.deviceSerial",
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
            Self::RdpRedirectClipboard => "dialog.rdpRedirectClipboard",
            Self::RdpRedirectDrives => "dialog.rdpRedirectDrives",
            Self::RdpDriveLetters => "dialog.rdpDriveLetters",
            Self::RdpCustomDrivePath => "dialog.rdpCustomDrive",
            Self::RdpRedirectPrinters => "dialog.rdpRedirectPrinters",
            Self::RdpRedirectPorts => "dialog.rdpRedirectPorts",
            Self::RdpRedirectSmartcards => "dialog.rdpRedirectSmartcards",
            Self::RdpRedirectMicrophone => "dialog.rdpRedirectMicrophone",
            Self::RdpRedirectWebauthn => "dialog.rdpRedirectWebauthn",
            Self::RdpAudio => "dialog.rdpAudio",
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
                | Self::SshCompatibility
                | Self::AllowShortRsaHostKeys
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
                | Self::RdpRedirectClipboard
                | Self::RdpRedirectDrives
                | Self::RdpRedirectPrinters
                | Self::RdpRedirectPorts
                | Self::RdpRedirectSmartcards
                | Self::RdpRedirectMicrophone
                | Self::RdpRedirectWebauthn
                | Self::VncClipboard
                | Self::VncShared
                | Self::VncViewOnly
                | Self::VncReconnect
                | Self::RecordingUseGlobal
                | Self::RecordingAutoStart
                | Self::AgentExternal
                | Self::AgentStoredKeys
                | Self::AdbScreenPowerOn
                | Self::FtpAsciiMode
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
pub(in crate::features) const EDITABLE_FIELDS: [BatchFieldKey; 123] = [
    BatchFieldKey::Group,
    BatchFieldKey::Description,
    BatchFieldKey::Icon,
    BatchFieldKey::IconAutoDetect,
    BatchFieldKey::Port,
    BatchFieldKey::Username,
    BatchFieldKey::Encoding,
    BatchFieldKey::Backspace,
    BatchFieldKey::SshProfile,
    BatchFieldKey::TerminalType,
    BatchFieldKey::SshCompatibility,
    BatchFieldKey::DynamicTabTitle,
    BatchFieldKey::SftpEnabled,
    BatchFieldKey::SftpCompatibility,
    BatchFieldKey::SftpCwdFollow,
    BatchFieldKey::SftpShellTimeout,
    BatchFieldKey::SftpFilenameEncoding,
    BatchFieldKey::SftpPipelineDepth,
    BatchFieldKey::X11,
    BatchFieldKey::AgentForwarding,
    BatchFieldKey::PostLoginEnabled,
    BatchFieldKey::PostLoginCommand,
    BatchFieldKey::PostLoginDelay,
    BatchFieldKey::SshAlgorithmMode,
    BatchFieldKey::SshKex,
    BatchFieldKey::SshCiphers,
    BatchFieldKey::SshMacs,
    BatchFieldKey::SshHostKeys,
    BatchFieldKey::AllowShortRsaHostKeys,
    BatchFieldKey::RdpUseNla,
    BatchFieldKey::RdpCertificatePolicy,
    BatchFieldKey::RdpAdminSession,
    BatchFieldKey::VncSecurityMode,
    BatchFieldKey::Tags,
    BatchFieldKey::Account,
    BatchFieldKey::Password,
    BatchFieldKey::AuthMode,
    BatchFieldKey::Otp,
    BatchFieldKey::OtpAutoFill,
    BatchFieldKey::Key,
    BatchFieldKey::SshAuthAgentEndpoint,
    BatchFieldKey::SshAuthAgentEnvironment,
    BatchFieldKey::SshAuthAgentSocket,
    BatchFieldKey::Proxy,
    BatchFieldKey::ProxyJump,
    BatchFieldKey::ShellPath,
    BatchFieldKey::ShellArgs,
    BatchFieldKey::WorkingDir,
    BatchFieldKey::SerialPort,
    BatchFieldKey::BaudRate,
    BatchFieldKey::DataBits,
    BatchFieldKey::Parity,
    BatchFieldKey::StopBits,
    BatchFieldKey::FlowControl,
    BatchFieldKey::TelnetRawTcp,
    BatchFieldKey::TelnetKeepAlive,
    BatchFieldKey::TelnetLocalEcho,
    BatchFieldKey::TelnetLocalLineEdit,
    BatchFieldKey::TelnetForceChar,
    BatchFieldKey::TelnetSendNaws,
    BatchFieldKey::TelnetSendSga,
    BatchFieldKey::TelnetEnterMode,
    BatchFieldKey::TelnetAutoLoginEnabled,
    BatchFieldKey::TelnetAutoLoginWake,
    BatchFieldKey::TelnetAutoLoginTimeout,
    BatchFieldKey::TelnetAutoLoginRetries,
    BatchFieldKey::TelnetAutoLoginUserPrompt,
    BatchFieldKey::TelnetAutoLoginPasswordPrompt,
    BatchFieldKey::TelnetAutoLoginSuccessPrompt,
    BatchFieldKey::TelnetAutoLoginFailurePrompt,
    BatchFieldKey::RdpDomain,
    BatchFieldKey::RdpBackend,
    BatchFieldKey::RdpDisplayMode,
    BatchFieldKey::RdpWidth,
    BatchFieldKey::RdpHeight,
    BatchFieldKey::RdpWallpaper,
    BatchFieldKey::RdpThemes,
    BatchFieldKey::RdpFontSmoothing,
    BatchFieldKey::RdpDesktopComposition,
    BatchFieldKey::RdpDisableFullWindowDrag,
    BatchFieldKey::RdpDisableMenuAnimations,
    BatchFieldKey::RdpDisableCursorShadow,
    BatchFieldKey::RdpDisableCursorBlinking,
    BatchFieldKey::RdpAutoReconnect,
    BatchFieldKey::RdpReconnectAttempts,
    BatchFieldKey::RdpRedirectClipboard,
    BatchFieldKey::RdpRedirectDrives,
    BatchFieldKey::RdpDriveLetters,
    BatchFieldKey::RdpCustomDrivePath,
    BatchFieldKey::RdpRedirectPrinters,
    BatchFieldKey::RdpRedirectPorts,
    BatchFieldKey::RdpRedirectSmartcards,
    BatchFieldKey::RdpRedirectMicrophone,
    BatchFieldKey::RdpRedirectWebauthn,
    BatchFieldKey::RdpAudio,
    BatchFieldKey::VncScaleMode,
    BatchFieldKey::VncClipboard,
    BatchFieldKey::VncShared,
    BatchFieldKey::VncViewOnly,
    BatchFieldKey::VncReconnect,
    BatchFieldKey::VncReconnectAttempts,
    BatchFieldKey::RecordingUseGlobal,
    BatchFieldKey::RecordingAutoStart,
    BatchFieldKey::RecordingMode,
    BatchFieldKey::AgentExternal,
    BatchFieldKey::AgentStoredKeys,
    BatchFieldKey::AgentPolicy,
    BatchFieldKey::AdbDirectory,
    BatchFieldKey::AdbScreenQuality,
    BatchFieldKey::AdbScreenMaxSize,
    BatchFieldKey::AdbScreenFps,
    BatchFieldKey::AdbScreenBitRate,
    BatchFieldKey::AdbScreenPowerOn,
    BatchFieldKey::AdbScreenOrientation,
    BatchFieldKey::FtpInitialDirectory,
    BatchFieldKey::FtpLocalInitialDirectory,
    BatchFieldKey::FtpPassiveMode,
    BatchFieldKey::FtpFilenameEncodingMode,
    BatchFieldKey::FtpFilenameEncoding,
    BatchFieldKey::FtpAsciiMode,
    BatchFieldKey::FtpSecurityMode,
    BatchFieldKey::FtpTlsVersion,
    BatchFieldKey::FtpConnectTimeout,
];

/// Fields that exist in the connection editor but cannot be batch-written.
/// They are rendered `disabled` in their original position and never enter a
/// patch, so their value is only ever read for display.
pub(in crate::features) const DISPLAY_FIELDS: [BatchFieldKey; 4] = [
    BatchFieldKey::Name,
    BatchFieldKey::Host,
    BatchFieldKey::AdbSerial,
    BatchFieldKey::AgentEndpoints,
];

/// The only row-level actions the batch editor offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum BatchFieldAction {
    /// Text and list fields: deleting the content clears the value.
    None,
    /// Booleans, enums, steppers and dropdowns: writes the documented default.
    ResetDefault,
    /// The inline password row: removes the stored credential.
    ClearPassword,
}

impl BatchFieldKey {
    /// The connection-editor section the row belongs to. Exhaustive on purpose:
    /// adding a batch field must place it in a section.
    pub(in crate::features) fn section(self) -> ConnectionEditorSection {
        match self {
            Self::Group
            | Self::Description
            | Self::Icon
            | Self::IconAutoDetect
            | Self::Name
            | Self::Host
            | Self::Port
            | Self::Username
            | Self::AdbSerial
            | Self::Tags
            | Self::Account
            | Self::Password
            | Self::AuthMode
            | Self::Otp
            | Self::OtpAutoFill
            | Self::Key
            | Self::SshAuthAgentEndpoint
            | Self::SshAuthAgentEnvironment
            | Self::SshAuthAgentSocket
            | Self::ShellPath
            | Self::ShellArgs
            | Self::WorkingDir
            | Self::SerialPort
            | Self::BaudRate
            | Self::DataBits
            | Self::Parity
            | Self::StopBits
            | Self::FlowControl
            | Self::RdpDomain
            | Self::AdbDirectory => ConnectionEditorSection::Basic,
            Self::Proxy | Self::ProxyJump => ConnectionEditorSection::Network,
            Self::Encoding
            | Self::Backspace
            | Self::SshProfile
            | Self::TerminalType
            | Self::SshCompatibility
            | Self::DynamicTabTitle
            | Self::RecordingUseGlobal
            | Self::RecordingAutoStart
            | Self::RecordingMode => ConnectionEditorSection::Terminal,
            Self::SftpEnabled
            | Self::SftpCompatibility
            | Self::SftpCwdFollow
            | Self::SftpShellTimeout
            | Self::SftpFilenameEncoding
            | Self::SftpPipelineDepth => ConnectionEditorSection::Sftp,
            Self::X11
            | Self::AgentForwarding
            | Self::AgentExternal
            | Self::AgentStoredKeys
            | Self::AgentPolicy
            | Self::AgentEndpoints => ConnectionEditorSection::Forwarding,
            Self::PostLoginEnabled | Self::PostLoginCommand | Self::PostLoginDelay => {
                ConnectionEditorSection::PostLogin
            }
            Self::SshAlgorithmMode
            | Self::SshKex
            | Self::SshCiphers
            | Self::SshMacs
            | Self::SshHostKeys
            | Self::AllowShortRsaHostKeys => ConnectionEditorSection::Algorithms,
            Self::TelnetAutoLoginEnabled
            | Self::TelnetAutoLoginWake
            | Self::TelnetAutoLoginTimeout
            | Self::TelnetAutoLoginRetries
            | Self::TelnetAutoLoginUserPrompt
            | Self::TelnetAutoLoginPasswordPrompt
            | Self::TelnetAutoLoginSuccessPrompt
            | Self::TelnetAutoLoginFailurePrompt => ConnectionEditorSection::AutoLogin,
            Self::TelnetRawTcp
            | Self::TelnetKeepAlive
            | Self::TelnetLocalEcho
            | Self::TelnetLocalLineEdit
            | Self::TelnetForceChar
            | Self::TelnetSendNaws
            | Self::TelnetSendSga
            | Self::TelnetEnterMode => ConnectionEditorSection::Compatibility,
            Self::RdpUseNla
            | Self::RdpCertificatePolicy
            | Self::RdpAdminSession
            | Self::VncSecurityMode => ConnectionEditorSection::Security,
            Self::RdpDisplayMode
            | Self::RdpWidth
            | Self::RdpHeight
            | Self::RdpWallpaper
            | Self::RdpThemes
            | Self::RdpFontSmoothing
            | Self::RdpDesktopComposition
            | Self::RdpDisableFullWindowDrag
            | Self::RdpDisableMenuAnimations
            | Self::RdpDisableCursorShadow
            | Self::RdpDisableCursorBlinking
            | Self::VncScaleMode => ConnectionEditorSection::Display,
            Self::RdpBackend
            | Self::RdpAutoReconnect
            | Self::RdpReconnectAttempts
            | Self::RdpRedirectClipboard
            | Self::RdpRedirectDrives
            | Self::RdpDriveLetters
            | Self::RdpCustomDrivePath
            | Self::RdpRedirectPrinters
            | Self::RdpRedirectPorts
            | Self::RdpRedirectSmartcards
            | Self::RdpRedirectMicrophone
            | Self::RdpRedirectWebauthn
            | Self::RdpAudio
            | Self::VncClipboard
            | Self::VncShared
            | Self::VncViewOnly
            | Self::VncReconnect
            | Self::VncReconnectAttempts => ConnectionEditorSection::Session,
            Self::AdbScreenQuality
            | Self::AdbScreenMaxSize
            | Self::AdbScreenFps
            | Self::AdbScreenBitRate
            | Self::AdbScreenPowerOn
            | Self::AdbScreenOrientation => ConnectionEditorSection::AndroidScreen,
            Self::FtpFilenameEncodingMode | Self::FtpFilenameEncoding => {
                ConnectionEditorSection::Charset
            }
            Self::FtpPassiveMode
            | Self::FtpAsciiMode
            | Self::FtpSecurityMode
            | Self::FtpTlsVersion
            | Self::FtpInitialDirectory
            | Self::FtpLocalInitialDirectory
            | Self::FtpConnectTimeout => ConnectionEditorSection::Advanced,
        }
    }

    pub(in crate::features) const fn action(self) -> BatchFieldAction {
        match self {
            Self::Password => BatchFieldAction::ClearPassword,
            Self::Group
            | Self::Description
            | Self::Icon
            | Self::Name
            | Self::Host
            | Self::AdbSerial
            | Self::AgentEndpoints
            | Self::Tags
            | Self::PostLoginCommand
            | Self::Account
            | Self::Otp
            | Self::Key
            | Self::SshAuthAgentEndpoint
            | Self::SshAuthAgentEnvironment
            | Self::SshAuthAgentSocket
            | Self::Proxy
            | Self::ProxyJump
            | Self::SshKex
            | Self::SshCiphers
            | Self::SshMacs
            | Self::SshHostKeys
            | Self::ShellPath
            | Self::ShellArgs
            | Self::WorkingDir
            | Self::TelnetAutoLoginUserPrompt
            | Self::TelnetAutoLoginPasswordPrompt
            | Self::TelnetAutoLoginSuccessPrompt
            | Self::TelnetAutoLoginFailurePrompt
            | Self::RdpDomain
            | Self::RdpDriveLetters
            | Self::RdpCustomDrivePath
            | Self::AdbDirectory
            | Self::FtpInitialDirectory
            | Self::FtpLocalInitialDirectory
            | Self::FtpFilenameEncoding => BatchFieldAction::None,
            _ => BatchFieldAction::ResetDefault,
        }
    }

    /// The value `Clear` writes for this row, rendered exactly like the draft
    /// stores it. `None` means the field has no single scalar default.
    pub(in crate::features) fn default_value(self, kind: ConnectionKindTab) -> Option<String> {
        let value = match self {
            Self::Port => kind.batch_kind().default_port().to_string(),
            Self::Username => {
                if matches!(kind, ConnectionKindTab::Ssh) {
                    "root".to_string()
                } else {
                    String::new()
                }
            }
            Self::Backspace => match kind {
                ConnectionKindTab::Ssh | ConnectionKindTab::Telnet => "del".to_string(),
                ConnectionKindTab::Serial => "ctrl_h".to_string(),
                _ => return None,
            },
            Self::Encoding | Self::SftpFilenameEncoding | Self::FtpFilenameEncoding => {
                String::new()
            }
            Self::SshProfile => "standard".to_string(),
            Self::AuthMode => "password".to_string(),
            Self::TerminalType => String::new(),
            Self::SftpCwdFollow => "rc_file".to_string(),
            Self::SftpPipelineDepth => return None,
            Self::SftpShellTimeout => {
                nyaterm_core::models::default_sftp_shell_detection_timeout_ms().to_string()
            }
            Self::PostLoginDelay => "1000".to_string(),
            Self::SshAlgorithmMode => "secure".to_string(),
            Self::RdpBackend => "builtin".to_string(),
            Self::RdpCertificatePolicy => nyaterm_core::models::default_rdp_certificate_policy(),
            Self::RdpDriveLetters => nyaterm_core::models::default_rdp_drive_letters(),
            Self::RdpAudio => nyaterm_core::models::default_rdp_audio_mode(),
            Self::RdpCustomDrivePath => String::new(),
            Self::RdpRedirectClipboard => "true".to_string(),
            Self::RdpDisplayMode => "fit-window".to_string(),
            Self::RdpWidth => nyaterm_core::models::default_rdp_width().to_string(),
            Self::RdpHeight => nyaterm_core::models::default_rdp_height().to_string(),
            Self::RdpReconnectAttempts | Self::VncReconnectAttempts => {
                nyaterm_core::models::default_rdp_reconnect_attempts().to_string()
            }
            Self::VncSecurityMode => nyaterm_core::models::default_vnc_security_mode(),
            Self::VncScaleMode => nyaterm_core::models::default_vnc_scale_mode(),
            Self::TelnetKeepAlive => {
                nyaterm_core::models::DEFAULT_TELNET_KEEP_ALIVE_INTERVAL_SECS.to_string()
            }
            Self::TelnetEnterMode => nyaterm_core::models::default_telnet_enter_mode(),
            Self::TelnetAutoLoginTimeout => {
                nyaterm_core::models::default_telnet_auto_login_timeout_ms().to_string()
            }
            Self::TelnetAutoLoginRetries => "0".to_string(),
            Self::BaudRate => "115200".to_string(),
            Self::DataBits => "8".to_string(),
            Self::Parity => "none".to_string(),
            Self::StopBits => "1".to_string(),
            Self::FlowControl => "none".to_string(),
            Self::AdbDirectory => nyaterm_core::models::android::default_android_directory(),
            Self::AdbScreenQuality => "balanced".to_string(),
            Self::AdbScreenMaxSize => "1280".to_string(),
            Self::AdbScreenFps => "30".to_string(),
            Self::AdbScreenBitRate => "8000000".to_string(),
            Self::AdbScreenOrientation => "0".to_string(),
            Self::FtpPassiveMode => "auto".to_string(),
            Self::FtpFilenameEncodingMode => "auto".to_string(),
            Self::FtpSecurityMode => "none".to_string(),
            Self::FtpTlsVersion => "auto".to_string(),
            Self::FtpConnectTimeout => "15000".to_string(),
            _ if self.is_bool() => "false".to_string(),
            _ => return None,
        };
        Some(value)
    }
}

/// One row of the batch editor.
///
/// Rows mirror the connection editor's own layout for the same protocol and
/// section: the same fields share a row, and the row keeps the editor's column
/// ratios. A parity test keeps this table in step with `EDITABLE_FIELDS`.
#[derive(Clone, Copy, Debug)]
pub(in crate::features) struct BatchRowSpec {
    pub layout: BatchRowLayout,
    pub fields: &'static [BatchFieldSpec],
}

/// How the fields of one row share the horizontal space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum BatchRowLayout {
    /// One field per row.
    Stack,
    /// Equal-width columns, like the editor's `grid_cols(2)`.
    Columns,
    /// The editor's three-column grid (serial data bits/parity/stop bits).
    ThreeColumns,
    /// `flex_1` field plus the 132 px port column used by the SSH editor.
    HostPort,
    /// `flex_1` field plus the 150 px port column used by every other editor.
    HostPortWide,
}

#[derive(Clone, Copy, Debug)]
pub(in crate::features) struct BatchFieldSpec {
    pub key: BatchFieldKey,
    pub action: BatchFieldAction,
}

const fn spec(key: BatchFieldKey) -> BatchFieldSpec {
    BatchFieldSpec {
        key,
        action: key.action(),
    }
}

const fn row(layout: BatchRowLayout, fields: &'static [BatchFieldSpec]) -> BatchRowSpec {
    BatchRowSpec { layout, fields }
}

// Basic-section header: icon, name and group share a row, exactly like
// `connection_editor_surface`. Tags and description follow the protocol body.
const BASIC_HEADER: BatchRowSpec = row(
    BatchRowLayout::Columns,
    &[
        spec(BatchFieldKey::Icon),
        spec(BatchFieldKey::Name),
        spec(BatchFieldKey::Group),
    ],
);

const BASIC_TAGS: BatchRowSpec = row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Tags)]);

const BASIC_DESCRIPTION: BatchRowSpec =
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Description)]);

const SSH_BASIC: &[BatchRowSpec] = &[
    BASIC_HEADER,
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::IconAutoDetect)],
    ),
    row(
        BatchRowLayout::HostPort,
        &[spec(BatchFieldKey::Host), spec(BatchFieldKey::Port)],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Account)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Username)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::AuthMode)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Password)]),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::SshAuthAgentEndpoint)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::SshAuthAgentEnvironment)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::SshAuthAgentSocket)],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Key)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Otp)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::OtpAutoFill)]),
    BASIC_TAGS,
    BASIC_DESCRIPTION,
];

const SSH_NETWORK: &[BatchRowSpec] = &[row(
    BatchRowLayout::Columns,
    &[spec(BatchFieldKey::Proxy), spec(BatchFieldKey::ProxyJump)],
)];

const SSH_TERMINAL: &[BatchRowSpec] = &[
    row(
        BatchRowLayout::Columns,
        &[
            spec(BatchFieldKey::SshProfile),
            spec(BatchFieldKey::TerminalType),
        ],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Encoding)]),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::SshCompatibility)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::DynamicTabTitle)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RecordingUseGlobal)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RecordingAutoStart)],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::RecordingMode)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Backspace)]),
];

const SSH_SFTP: &[BatchRowSpec] = &[
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::SftpEnabled)]),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::SftpCompatibility)],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::SftpCwdFollow)]),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::SftpShellTimeout)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::SftpFilenameEncoding)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::SftpPipelineDepth)],
    ),
];

const SSH_FORWARDING: &[BatchRowSpec] = &[
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::AgentForwarding)],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::AgentExternal)]),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::AgentEndpoints)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::AgentStoredKeys)],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::AgentPolicy)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::X11)]),
];

const SSH_POST_LOGIN: &[BatchRowSpec] = &[
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::PostLoginEnabled)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::PostLoginCommand)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::PostLoginDelay)],
    ),
];

const SSH_ALGORITHMS: &[BatchRowSpec] = &[
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::SshAlgorithmMode)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::AllowShortRsaHostKeys)],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::SshKex)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::SshCiphers)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::SshMacs)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::SshHostKeys)]),
];

const LOCAL_BASIC: &[BatchRowSpec] = &[
    BASIC_HEADER,
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::ShellPath)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::ShellArgs)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::WorkingDir)]),
    BASIC_TAGS,
    BASIC_DESCRIPTION,
];

const LOCAL_TERMINAL: &[BatchRowSpec] = &[
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Encoding)]),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::DynamicTabTitle)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RecordingUseGlobal)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RecordingAutoStart)],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::RecordingMode)]),
];

const TELNET_BASIC: &[BatchRowSpec] = &[
    BASIC_HEADER,
    row(
        BatchRowLayout::HostPortWide,
        &[spec(BatchFieldKey::Host), spec(BatchFieldKey::Port)],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Account)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Username)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::AuthMode)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Password)]),
    BASIC_TAGS,
    BASIC_DESCRIPTION,
];

const TELNET_NETWORK: &[BatchRowSpec] = &[
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Proxy)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::ProxyJump)]),
];

const TELNET_AUTO_LOGIN: &[BatchRowSpec] = &[
    row(
        BatchRowLayout::Columns,
        &[
            spec(BatchFieldKey::TelnetAutoLoginEnabled),
            spec(BatchFieldKey::TelnetAutoLoginWake),
        ],
    ),
    row(
        BatchRowLayout::Columns,
        &[
            spec(BatchFieldKey::TelnetAutoLoginTimeout),
            spec(BatchFieldKey::TelnetAutoLoginRetries),
        ],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::TelnetAutoLoginUserPrompt)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::TelnetAutoLoginPasswordPrompt)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::TelnetAutoLoginSuccessPrompt)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::TelnetAutoLoginFailurePrompt)],
    ),
];

const TELNET_TERMINAL: &[BatchRowSpec] = &[
    row(
        BatchRowLayout::Columns,
        &[
            spec(BatchFieldKey::Backspace),
            spec(BatchFieldKey::TelnetEnterMode),
        ],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Encoding)]),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RecordingUseGlobal)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RecordingAutoStart)],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::RecordingMode)]),
];

const TELNET_COMPATIBILITY: &[BatchRowSpec] = &[
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::TelnetRawTcp)]),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::TelnetKeepAlive)],
    ),
    row(
        BatchRowLayout::Columns,
        &[
            spec(BatchFieldKey::TelnetLocalEcho),
            spec(BatchFieldKey::TelnetLocalLineEdit),
        ],
    ),
    row(
        BatchRowLayout::Columns,
        &[
            spec(BatchFieldKey::TelnetForceChar),
            spec(BatchFieldKey::TelnetSendNaws),
        ],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::TelnetSendSga)]),
];

const SERIAL_BASIC: &[BatchRowSpec] = &[
    BASIC_HEADER,
    row(
        BatchRowLayout::Columns,
        &[
            spec(BatchFieldKey::SerialPort),
            spec(BatchFieldKey::BaudRate),
        ],
    ),
    row(
        BatchRowLayout::ThreeColumns,
        &[
            spec(BatchFieldKey::DataBits),
            spec(BatchFieldKey::Parity),
            spec(BatchFieldKey::StopBits),
        ],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::FlowControl)]),
    BASIC_TAGS,
    BASIC_DESCRIPTION,
];

const SERIAL_TERMINAL: &[BatchRowSpec] = &[
    row(
        BatchRowLayout::Columns,
        &[
            spec(BatchFieldKey::Backspace),
            spec(BatchFieldKey::Encoding),
        ],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RecordingUseGlobal)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RecordingAutoStart)],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::RecordingMode)]),
];

const RDP_BASIC: &[BatchRowSpec] = &[
    BASIC_HEADER,
    row(
        BatchRowLayout::HostPortWide,
        &[spec(BatchFieldKey::Host), spec(BatchFieldKey::Port)],
    ),
    row(
        BatchRowLayout::Columns,
        &[
            spec(BatchFieldKey::Username),
            spec(BatchFieldKey::RdpDomain),
        ],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::AuthMode)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Account)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Password)]),
    BASIC_TAGS,
    BASIC_DESCRIPTION,
];

const RDP_NETWORK: &[BatchRowSpec] = &[
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Proxy)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::ProxyJump)]),
];

const RDP_SECURITY: &[BatchRowSpec] = &[
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::RdpUseNla)]),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RdpAdminSession)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RdpCertificatePolicy)],
    ),
];

const RDP_DISPLAY: &[BatchRowSpec] = &[
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RdpDisplayMode)],
    ),
    row(
        BatchRowLayout::Columns,
        &[
            spec(BatchFieldKey::RdpWidth),
            spec(BatchFieldKey::RdpHeight),
        ],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::RdpWallpaper)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::RdpThemes)]),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RdpFontSmoothing)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RdpDesktopComposition)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RdpDisableFullWindowDrag)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RdpDisableMenuAnimations)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RdpDisableCursorShadow)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RdpDisableCursorBlinking)],
    ),
];

const RDP_SESSION: &[BatchRowSpec] = &[
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::RdpBackend)]),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RdpAutoReconnect)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RdpReconnectAttempts)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RdpRedirectClipboard)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RdpRedirectDrives)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RdpDriveLetters)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RdpCustomDrivePath)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RdpRedirectPrinters)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RdpRedirectPorts)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RdpRedirectSmartcards)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RdpRedirectMicrophone)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::RdpRedirectWebauthn)],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::RdpAudio)]),
];

const VNC_BASIC: &[BatchRowSpec] = &[
    BASIC_HEADER,
    row(
        BatchRowLayout::HostPortWide,
        &[spec(BatchFieldKey::Host), spec(BatchFieldKey::Port)],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::AuthMode)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Account)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Password)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Username)]),
    BASIC_TAGS,
    BASIC_DESCRIPTION,
];

const VNC_NETWORK: &[BatchRowSpec] = &[
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Proxy)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::ProxyJump)]),
];

const VNC_SECURITY: &[BatchRowSpec] = &[row(
    BatchRowLayout::Stack,
    &[spec(BatchFieldKey::VncSecurityMode)],
)];

const VNC_DISPLAY: &[BatchRowSpec] = &[row(
    BatchRowLayout::Stack,
    &[spec(BatchFieldKey::VncScaleMode)],
)];

const VNC_SESSION: &[BatchRowSpec] = &[
    row(
        BatchRowLayout::Columns,
        &[
            spec(BatchFieldKey::VncClipboard),
            spec(BatchFieldKey::VncShared),
        ],
    ),
    row(
        BatchRowLayout::Columns,
        &[
            spec(BatchFieldKey::VncViewOnly),
            spec(BatchFieldKey::VncReconnect),
        ],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::VncReconnectAttempts)],
    ),
];

const ADB_BASIC: &[BatchRowSpec] = &[
    BASIC_HEADER,
    row(
        BatchRowLayout::HostPortWide,
        &[spec(BatchFieldKey::Host), spec(BatchFieldKey::Port)],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::AdbSerial)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::AdbDirectory)]),
    BASIC_TAGS,
    BASIC_DESCRIPTION,
];

const ADB_ANDROID_SCREEN: &[BatchRowSpec] = &[
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::AdbScreenQuality)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::AdbScreenPowerOn)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::AdbScreenOrientation)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::AdbScreenMaxSize)],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::AdbScreenFps)]),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::AdbScreenBitRate)],
    ),
];

const FTP_BASIC: &[BatchRowSpec] = &[
    BASIC_HEADER,
    row(
        BatchRowLayout::HostPortWide,
        &[spec(BatchFieldKey::Host), spec(BatchFieldKey::Port)],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Account)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Username)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::AuthMode)]),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::Password)]),
    BASIC_TAGS,
    BASIC_DESCRIPTION,
];

const FTP_CHARSET: &[BatchRowSpec] = &[
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::FtpFilenameEncodingMode)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::FtpFilenameEncoding)],
    ),
];

const FTP_ADVANCED: &[BatchRowSpec] = &[
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::FtpPassiveMode)],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::FtpAsciiMode)]),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::FtpSecurityMode)],
    ),
    row(BatchRowLayout::Stack, &[spec(BatchFieldKey::FtpTlsVersion)]),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::FtpInitialDirectory)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::FtpLocalInitialDirectory)],
    ),
    row(
        BatchRowLayout::Stack,
        &[spec(BatchFieldKey::FtpConnectTimeout)],
    ),
];

/// The rows the batch editor renders for one protocol tab and section.
///
/// Ordering, row grouping and column ratios follow the connection editor's
/// section body (`ssh.rs`, `local.rs`, `telnet.rs`, `serial.rs`, `rdp.rs`,
/// `vnc.rs`, `android.rs`, `ftp.rs`). Empty means the editor renders nothing
/// for that pair.
pub(in crate::features) fn batch_rows(
    kind: ConnectionKindTab,
    section: ConnectionEditorSection,
) -> &'static [BatchRowSpec] {
    match (kind, section) {
        (ConnectionKindTab::Ssh, ConnectionEditorSection::Basic) => SSH_BASIC,
        (ConnectionKindTab::Ssh, ConnectionEditorSection::Network) => SSH_NETWORK,
        (ConnectionKindTab::Ssh, ConnectionEditorSection::Terminal) => SSH_TERMINAL,
        (ConnectionKindTab::Ssh, ConnectionEditorSection::Sftp) => SSH_SFTP,
        (ConnectionKindTab::Ssh, ConnectionEditorSection::Forwarding) => SSH_FORWARDING,
        (ConnectionKindTab::Ssh, ConnectionEditorSection::PostLogin) => SSH_POST_LOGIN,
        (ConnectionKindTab::Ssh, ConnectionEditorSection::Algorithms) => SSH_ALGORITHMS,
        (ConnectionKindTab::Local, ConnectionEditorSection::Basic) => LOCAL_BASIC,
        (ConnectionKindTab::Local, ConnectionEditorSection::Terminal) => LOCAL_TERMINAL,
        (ConnectionKindTab::Telnet, ConnectionEditorSection::Basic) => TELNET_BASIC,
        (ConnectionKindTab::Telnet, ConnectionEditorSection::Network) => TELNET_NETWORK,
        (ConnectionKindTab::Telnet, ConnectionEditorSection::AutoLogin) => TELNET_AUTO_LOGIN,
        (ConnectionKindTab::Telnet, ConnectionEditorSection::Terminal) => TELNET_TERMINAL,
        (ConnectionKindTab::Telnet, ConnectionEditorSection::Compatibility) => TELNET_COMPATIBILITY,
        (ConnectionKindTab::Serial, ConnectionEditorSection::Basic)
        | (ConnectionKindTab::Serial, ConnectionEditorSection::Serial) => SERIAL_BASIC,
        (ConnectionKindTab::Serial, ConnectionEditorSection::Terminal) => SERIAL_TERMINAL,
        (ConnectionKindTab::Rdp, ConnectionEditorSection::Basic) => RDP_BASIC,
        (ConnectionKindTab::Rdp, ConnectionEditorSection::Network) => RDP_NETWORK,
        (ConnectionKindTab::Rdp, ConnectionEditorSection::Security) => RDP_SECURITY,
        (ConnectionKindTab::Rdp, ConnectionEditorSection::Display) => RDP_DISPLAY,
        (ConnectionKindTab::Rdp, ConnectionEditorSection::Session) => RDP_SESSION,
        (ConnectionKindTab::Vnc, ConnectionEditorSection::Basic) => VNC_BASIC,
        (ConnectionKindTab::Vnc, ConnectionEditorSection::Network) => VNC_NETWORK,
        (ConnectionKindTab::Vnc, ConnectionEditorSection::Security) => VNC_SECURITY,
        (ConnectionKindTab::Vnc, ConnectionEditorSection::Display) => VNC_DISPLAY,
        (ConnectionKindTab::Vnc, ConnectionEditorSection::Session) => VNC_SESSION,
        (ConnectionKindTab::Adb, ConnectionEditorSection::Basic) => ADB_BASIC,
        (ConnectionKindTab::Adb, ConnectionEditorSection::AndroidScreen) => ADB_ANDROID_SCREEN,
        (ConnectionKindTab::Ftp, ConnectionEditorSection::Basic) => FTP_BASIC,
        (ConnectionKindTab::Ftp, ConnectionEditorSection::Charset) => FTP_CHARSET,
        (ConnectionKindTab::Ftp, ConnectionEditorSection::Advanced) => FTP_ADVANCED,
        _ => &[],
    }
}
