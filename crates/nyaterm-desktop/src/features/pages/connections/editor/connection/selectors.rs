//! Connection editor selectors presentation.

use crate::features::NyaTermApp;
use crate::models::ConnectionEditorSelect;
use rust_i18n::t;

impl NyaTermApp {
    pub(in crate::features) fn focus_connection_editor_select(
        &self,
        select: ConnectionEditorSelect,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> bool {
        self.focus_select(connection_editor_select_id(select), window, cx)
    }

    pub(in crate::features::pages::connections::editor) fn connection_editor_select_is_focused(
        &self,
        window: &gpui::Window,
        cx: &gpui::App,
    ) -> bool {
        self.select_is_focused("connection-editor-kind", window, cx)
            || connection_editor_select_keys().into_iter().any(|select| {
                self.select_is_focused(connection_editor_select_id(select), window, cx)
            })
            || self.select_with_prefix_is_focused(
                "connection-editor-ssh-agent-forwarding-endpoint-",
                window,
                cx,
            )
    }

    pub(in crate::features::pages::connections::editor) fn connection_editor_select_menu_is_focused(
        &self,
        window: &gpui::Window,
        cx: &gpui::App,
    ) -> bool {
        self.select_menu_is_focused("connection-editor-kind", window, cx)
            || connection_editor_select_keys().into_iter().any(|select| {
                self.select_menu_is_focused(connection_editor_select_id(select), window, cx)
            })
            || self.select_menu_with_prefix_is_focused(
                "connection-editor-ssh-agent-forwarding-endpoint-",
                window,
                cx,
            )
    }
}

pub(super) fn connection_editor_select_keys() -> [ConnectionEditorSelect; 31] {
    [
        ConnectionEditorSelect::Group,
        ConnectionEditorSelect::SavedPassword,
        ConnectionEditorSelect::Account,
        ConnectionEditorSelect::SshKey,
        ConnectionEditorSelect::Otp,
        ConnectionEditorSelect::Proxy,
        ConnectionEditorSelect::ProxyJump,
        ConnectionEditorSelect::Backspace,
        ConnectionEditorSelect::Encoding,
        ConnectionEditorSelect::SftpCwdFollowMode,
        ConnectionEditorSelect::SftpPipelineDepth,
        ConnectionEditorSelect::SftpFilenameEncoding,
        ConnectionEditorSelect::SshAlgorithmMode,
        ConnectionEditorSelect::SshAgentEndpoint,
        ConnectionEditorSelect::SshAgentForwardingPolicy,
        ConnectionEditorSelect::SshProfile,
        ConnectionEditorSelect::SshTerminalType,
        ConnectionEditorSelect::RdpBackend,
        ConnectionEditorSelect::RdpCertificatePolicy,
        ConnectionEditorSelect::RdpDisplayMode,
        ConnectionEditorSelect::VncSecurityMode,
        ConnectionEditorSelect::VncScaleMode,
        ConnectionEditorSelect::RecordingMode,
        ConnectionEditorSelect::TelnetEnterMode,
        ConnectionEditorSelect::Shell,
        ConnectionEditorSelect::SerialPort,
        ConnectionEditorSelect::BaudRate,
        ConnectionEditorSelect::DataBits,
        ConnectionEditorSelect::Parity,
        ConnectionEditorSelect::StopBits,
        ConnectionEditorSelect::FlowControl,
    ]
}

pub(super) fn connection_editor_select_id(select: ConnectionEditorSelect) -> &'static str {
    match select {
        ConnectionEditorSelect::Authentication => {
            unreachable!("authentication uses connection-specific segmented tabs")
        }
        ConnectionEditorSelect::SshAgentEndpoint => "connection-editor-ssh-agent-endpoint",
        ConnectionEditorSelect::SshAgentForwardingPolicy => "connection-editor-ssh-agent-policy",
        ConnectionEditorSelect::Group => "connection-editor-group-select",
        ConnectionEditorSelect::SavedPassword => "connection-editor-saved-password",
        ConnectionEditorSelect::Account => "connection-editor-account",
        ConnectionEditorSelect::SshKey => "connection-editor-ssh-key",
        ConnectionEditorSelect::Otp => "connection-editor-otp",
        ConnectionEditorSelect::Proxy => "connection-editor-proxy",
        ConnectionEditorSelect::ProxyJump => "connection-editor-proxy-jump",
        ConnectionEditorSelect::Backspace => "connection-editor-backspace",
        ConnectionEditorSelect::Encoding => "connection-editor-encoding",
        ConnectionEditorSelect::SftpCwdFollowMode => "connection-editor-sftp-cwd-follow",
        ConnectionEditorSelect::SftpPipelineDepth => "connection-editor-sftp-pipeline-depth",
        ConnectionEditorSelect::SftpFilenameEncoding => "connection-editor-sftp-filename-encoding",
        ConnectionEditorSelect::SshAlgorithmMode => "connection-editor-ssh-algorithm-mode",
        ConnectionEditorSelect::SshProfile => "connection-editor-ssh-profile",
        ConnectionEditorSelect::SshTerminalType => "connection-editor-ssh-terminal-type",
        ConnectionEditorSelect::RdpBackend => "connection-editor-rdp-backend",
        ConnectionEditorSelect::RdpCertificatePolicy => "connection-editor-rdp-certificate-policy",
        ConnectionEditorSelect::RdpDisplayMode => "connection-editor-rdp-display-mode",
        ConnectionEditorSelect::VncSecurityMode => "connection-editor-vnc-security-mode",
        ConnectionEditorSelect::VncScaleMode => "connection-editor-vnc-scale-mode",
        ConnectionEditorSelect::RecordingMode => "connection-editor-recording-mode",
        ConnectionEditorSelect::TelnetEnterMode => "connection-editor-telnet-enter-mode",
        ConnectionEditorSelect::Shell => "connection-editor-shell",
        ConnectionEditorSelect::SerialPort => "connection-editor-serial-port",
        ConnectionEditorSelect::BaudRate => "connection-editor-baud-rate",
        ConnectionEditorSelect::DataBits => "connection-editor-data-bits",
        ConnectionEditorSelect::Parity => "connection-editor-parity",
        ConnectionEditorSelect::StopBits => "connection-editor-stop-bits",
        ConnectionEditorSelect::FlowControl => "connection-editor-flow-control",
    }
}

pub(super) fn connection_editor_select_is_searchable(select: ConnectionEditorSelect) -> bool {
    matches!(
        select,
        ConnectionEditorSelect::SavedPassword
            | ConnectionEditorSelect::Account
            | ConnectionEditorSelect::SshKey
            | ConnectionEditorSelect::Otp
            | ConnectionEditorSelect::Proxy
            | ConnectionEditorSelect::ProxyJump
    )
}

pub(super) fn connection_editor_select_search_placeholder(
    select: ConnectionEditorSelect,
) -> Option<String> {
    match select {
        ConnectionEditorSelect::SavedPassword => Some(t!("dialog.selectPassword").to_string()),
        ConnectionEditorSelect::Account => Some(t!("dialog.account").to_string()),
        ConnectionEditorSelect::SshKey => Some(t!("dialog.privateKey").to_string()),
        ConnectionEditorSelect::Otp => Some(t!("dialog.searchOtpEntries").to_string()),
        ConnectionEditorSelect::Proxy => Some(t!("network.searchProxies").to_string()),
        ConnectionEditorSelect::ProxyJump => Some(t!("network.searchConnections").to_string()),
        ConnectionEditorSelect::SshAgentForwardingPolicy => None,
        _ => None,
    }
}
