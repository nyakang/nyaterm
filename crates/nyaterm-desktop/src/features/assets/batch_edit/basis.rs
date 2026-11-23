use super::draft::{AssetBatchEditDraft, BatchEditMode};
use super::fields::{BatchFieldKey, DISPLAY_FIELDS, EDITABLE_FIELDS, FieldBasis};
pub(in crate::features) use nyaterm_core::models::connection_batch::basis::BatchFieldApplicability;
pub(super) use nyaterm_core::models::connection_batch::basis::connection_kind_counts as asset_batch_kind_counts;
use nyaterm_core::models::connection_batch::basis::{self as domain_basis, BatchValueBasis};
use nyaterm_core::models::connection_batch::{
    ConnectionBatchEdit, ConnectionBatchKind, ConnectionBatchSkipCounts, ConnectionBatchSkipReason,
};
use nyaterm_core::models::{
    ConnectionType, SavedConnection, SftpCwdFollowMode, SshAlgorithmMode, SshProfile,
    SshTerminalType,
};
use rust_i18n::t;
use std::collections::HashMap;

/// Renders the counts with the editor's own protocol labels, never raw
/// identifiers, and joins them with the locale's list separator.
pub(super) fn asset_batch_kind_counts_label(counts: &[(ConnectionBatchKind, usize)]) -> String {
    let separator = t!("assets.batch.mixedProtocolsSeparator").to_string();
    counts
        .iter()
        .map(|(kind, count)| {
            t!(
                "assets.batch.mixedProtocolsItem",
                kind = super::options::batch_kind_label(*kind),
                count = count
            )
            .to_string()
        })
        .collect::<Vec<_>>()
        .join(&separator)
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
pub(super) fn select_initial_value(
    draft: &AssetBatchEditDraft,
    field: BatchFieldKey,
) -> Option<String> {
    if draft.mode(field) == BatchEditMode::Keep && draft.basis(field) != FieldBasis::Uniform {
        return None;
    }
    let value = draft.value(field);
    match field {
        BatchFieldKey::Group => Some(value.to_string()),
        BatchFieldKey::Encoding => Some(if value.is_empty() {
            "global".to_string()
        } else {
            value.to_string()
        }),
        BatchFieldKey::SftpFilenameEncoding => Some(if value.is_empty() {
            "terminal".to_string()
        } else {
            value.to_string()
        }),
        BatchFieldKey::RdpBackend => Some(
            match value {
                "internal" | "auto" => "builtin",
                "active-x" => "windows",
                other => other,
            }
            .to_string(),
        ),
        BatchFieldKey::RdpDisplayMode => Some(
            match value {
                "fixed" | "native" => "fixed",
                _ => "fit-window",
            }
            .to_string(),
        ),
        BatchFieldKey::RdpAudio => Some(
            match value {
                "play_on_local" | "redirect" => "local",
                "play_on_remote" | "resource_only" => "remote",
                other => other,
            }
            .to_string(),
        ),
        // Keep the original reference/default sentinel for display lookup. Empty
        // options are filtered from Set mode; removing a value requires Clear.
        BatchFieldKey::Account
        | BatchFieldKey::Key
        | BatchFieldKey::Otp
        | BatchFieldKey::Proxy
        | BatchFieldKey::ProxyJump
        | BatchFieldKey::TerminalType
        | BatchFieldKey::SftpPipelineDepth => Some(value.to_string()),
        _ if value.is_empty() => None,
        _ => Some(value.to_string()),
    }
}

fn endpoint_values(config: &ConnectionType) -> Option<(&str, u16, &str)> {
    match config {
        ConnectionType::Spice {
            endpoint, username, ..
        } => Some((
            endpoint.host.expose_secret(),
            endpoint.tls_port.or(endpoint.port).unwrap_or(0),
            username,
        )),
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
        | ConnectionType::Ftp {
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
            | ConnectionBatchKind::Spice
    )
}

/// Fields whose meaning does not depend on the protocol.
pub(in crate::features) fn is_kind_independent(field: BatchFieldKey) -> bool {
    matches!(
        field,
        BatchFieldKey::Group
            | BatchFieldKey::Description
            | BatchFieldKey::Icon
            | BatchFieldKey::IconAutoDetect
            | BatchFieldKey::Name
            | BatchFieldKey::Tags
    )
}

/// Whether the field exists in the connection editor for `kind`. Fields that
/// do not exist are skipped; they never add an extra row.
pub(in crate::features) fn batch_field_in_kind(
    kind: ConnectionBatchKind,
    field: BatchFieldKey,
) -> bool {
    use ConnectionBatchKind::{LocalTerminal, Rdp, Serial, Ssh, Telnet, Vnc};
    if is_kind_independent(field) {
        return true;
    }
    match field {
        BatchFieldKey::Host | BatchFieldKey::Port => {
            (is_network_kind(kind)
                && !(field == BatchFieldKey::Port && kind == ConnectionBatchKind::Spice))
                || matches!(kind, ConnectionBatchKind::Ftp | ConnectionBatchKind::Adb)
        }
        BatchFieldKey::SpiceTcpPort
        | BatchFieldKey::SpiceTlsPort
        | BatchFieldKey::SpiceHttpProxy
        | BatchFieldKey::SpiceCaPem
        | BatchFieldKey::SpiceCertificateSubject
        | BatchFieldKey::SpiceRequireTls
        | BatchFieldKey::SpiceDynamicResolution
        | BatchFieldKey::SpiceFitWindow
        | BatchFieldKey::SpiceViewOnly
        | BatchFieldKey::SpiceClipboardToGuest
        | BatchFieldKey::SpiceClipboardFromGuest
        | BatchFieldKey::SpiceAudioPlayback
        | BatchFieldKey::SpiceFileTransfer
        | BatchFieldKey::SpiceReconnectAttempts => kind == ConnectionBatchKind::Spice,
        BatchFieldKey::Username => is_network_kind(kind) || kind == ConnectionBatchKind::Ftp,
        BatchFieldKey::Account | BatchFieldKey::Password | BatchFieldKey::AuthMode => {
            is_network_kind(kind) || kind == ConnectionBatchKind::Ftp
        }
        BatchFieldKey::Otp
        | BatchFieldKey::OtpAutoFill
        | BatchFieldKey::Key
        | BatchFieldKey::SshAuthAgentEndpoint
        | BatchFieldKey::SshAuthAgentEnvironment
        | BatchFieldKey::SshAuthAgentSocket => kind == Ssh,
        BatchFieldKey::Proxy | BatchFieldKey::ProxyJump => is_network_kind(kind),
        BatchFieldKey::ShellPath | BatchFieldKey::ShellArgs | BatchFieldKey::WorkingDir => {
            kind == LocalTerminal
        }
        BatchFieldKey::SerialPort
        | BatchFieldKey::BaudRate
        | BatchFieldKey::DataBits
        | BatchFieldKey::Parity
        | BatchFieldKey::StopBits
        | BatchFieldKey::FlowControl => kind == Serial,
        BatchFieldKey::TelnetRawTcp
        | BatchFieldKey::TelnetKeepAlive
        | BatchFieldKey::TelnetLocalEcho
        | BatchFieldKey::TelnetLocalLineEdit
        | BatchFieldKey::TelnetForceChar
        | BatchFieldKey::TelnetSendNaws
        | BatchFieldKey::TelnetSendSga
        | BatchFieldKey::TelnetEnterMode
        | BatchFieldKey::TelnetAutoLoginEnabled
        | BatchFieldKey::TelnetAutoLoginWake
        | BatchFieldKey::TelnetAutoLoginTimeout
        | BatchFieldKey::TelnetAutoLoginRetries
        | BatchFieldKey::TelnetAutoLoginUserPrompt
        | BatchFieldKey::TelnetAutoLoginPasswordPrompt
        | BatchFieldKey::TelnetAutoLoginSuccessPrompt
        | BatchFieldKey::TelnetAutoLoginFailurePrompt => kind == Telnet,
        BatchFieldKey::RdpDomain
        | BatchFieldKey::RdpBackend
        | BatchFieldKey::RdpDisplayMode
        | BatchFieldKey::RdpWidth
        | BatchFieldKey::RdpHeight
        | BatchFieldKey::RdpWallpaper
        | BatchFieldKey::RdpThemes
        | BatchFieldKey::RdpFontSmoothing
        | BatchFieldKey::RdpDesktopComposition
        | BatchFieldKey::RdpDisableFullWindowDrag
        | BatchFieldKey::RdpDisableMenuAnimations
        | BatchFieldKey::RdpDisableCursorShadow
        | BatchFieldKey::RdpDisableCursorBlinking
        | BatchFieldKey::RdpAutoReconnect
        | BatchFieldKey::RdpReconnectAttempts
        | BatchFieldKey::RdpRedirectClipboard
        | BatchFieldKey::RdpRedirectDrives
        | BatchFieldKey::RdpDriveLetters
        | BatchFieldKey::RdpCustomDrivePath
        | BatchFieldKey::RdpRedirectPrinters
        | BatchFieldKey::RdpRedirectPorts
        | BatchFieldKey::RdpRedirectSmartcards
        | BatchFieldKey::RdpRedirectMicrophone
        | BatchFieldKey::RdpRedirectWebauthn
        | BatchFieldKey::RdpAudio => kind == Rdp,
        BatchFieldKey::VncScaleMode
        | BatchFieldKey::VncClipboard
        | BatchFieldKey::VncShared
        | BatchFieldKey::VncViewOnly
        | BatchFieldKey::VncReconnect
        | BatchFieldKey::VncReconnectAttempts => kind == Vnc,
        BatchFieldKey::Encoding => matches!(kind, Ssh | LocalTerminal | Telnet | Serial),
        BatchFieldKey::Backspace => matches!(kind, Ssh | Telnet | Serial),
        BatchFieldKey::DynamicTabTitle => matches!(kind, Ssh | LocalTerminal),
        BatchFieldKey::SshProfile
        | BatchFieldKey::TerminalType
        | BatchFieldKey::SshCompatibility => kind == Ssh,
        BatchFieldKey::SftpEnabled
        | BatchFieldKey::SftpCompatibility
        | BatchFieldKey::SftpCwdFollow
        | BatchFieldKey::SftpShellTimeout
        | BatchFieldKey::SftpFilenameEncoding
        | BatchFieldKey::SftpPipelineDepth
        | BatchFieldKey::X11
        | BatchFieldKey::AgentForwarding
        | BatchFieldKey::PostLoginEnabled
        | BatchFieldKey::PostLoginCommand
        | BatchFieldKey::PostLoginDelay
        | BatchFieldKey::SshAlgorithmMode
        | BatchFieldKey::SshKex
        | BatchFieldKey::SshCiphers
        | BatchFieldKey::SshMacs
        | BatchFieldKey::SshHostKeys
        | BatchFieldKey::AllowShortRsaHostKeys
        | BatchFieldKey::AgentExternal
        | BatchFieldKey::AgentStoredKeys
        | BatchFieldKey::AgentPolicy
        | BatchFieldKey::AgentEndpoints => kind == Ssh,
        BatchFieldKey::RdpUseNla
        | BatchFieldKey::RdpCertificatePolicy
        | BatchFieldKey::RdpAdminSession => kind == Rdp,
        BatchFieldKey::VncSecurityMode => kind == Vnc,
        BatchFieldKey::AdbDirectory
        | BatchFieldKey::AdbSerial
        | BatchFieldKey::AdbScreenQuality
        | BatchFieldKey::AdbScreenMaxSize
        | BatchFieldKey::AdbScreenFps
        | BatchFieldKey::AdbScreenBitRate
        | BatchFieldKey::AdbScreenPowerOn
        | BatchFieldKey::AdbScreenOrientation => kind == ConnectionBatchKind::Adb,
        BatchFieldKey::FtpInitialDirectory
        | BatchFieldKey::FtpLocalInitialDirectory
        | BatchFieldKey::FtpPassiveMode
        | BatchFieldKey::FtpFilenameEncodingMode
        | BatchFieldKey::FtpFilenameEncoding
        | BatchFieldKey::FtpAsciiMode
        | BatchFieldKey::FtpSecurityMode
        | BatchFieldKey::FtpTlsVersion
        | BatchFieldKey::FtpConnectTimeout => kind == ConnectionBatchKind::Ftp,
        BatchFieldKey::Group
        | BatchFieldKey::Description
        | BatchFieldKey::Icon
        | BatchFieldKey::IconAutoDetect
        | BatchFieldKey::Name
        | BatchFieldKey::Tags => true,
        BatchFieldKey::RecordingUseGlobal
        | BatchFieldKey::RecordingAutoStart
        | BatchFieldKey::RecordingMode => matches!(kind, Ssh | LocalTerminal | Telnet | Serial),
    }
}

pub(in crate::features) fn field_display_value(
    connection: &SavedConnection,
    field: BatchFieldKey,
) -> Option<String> {
    let is_ssh = matches!(&connection.config, ConnectionType::Ssh { .. });
    match field {
        BatchFieldKey::Name => Some(connection.name.clone()),
        BatchFieldKey::SpiceTcpPort => match &connection.config {
            ConnectionType::Spice { endpoint, .. } => {
                Some(endpoint.port.map(|p| p.to_string()).unwrap_or_default())
            }
            _ => None,
        },
        BatchFieldKey::SpiceTlsPort => match &connection.config {
            ConnectionType::Spice { endpoint, .. } => {
                Some(endpoint.tls_port.map(|p| p.to_string()).unwrap_or_default())
            }
            _ => None,
        },
        BatchFieldKey::SpiceHttpProxy => match &connection.config {
            ConnectionType::Spice { endpoint, .. } => Some(
                endpoint
                    .proxy
                    .as_ref()
                    .map(|p| p.expose_secret().to_owned())
                    .unwrap_or_default(),
            ),
            _ => None,
        },
        BatchFieldKey::SpiceCaPem => match &connection.config {
            ConnectionType::Spice {
                endpoint: _,
                settings,
                ..
            } => Some(settings.security.ca_pem.clone().unwrap_or_default()),
            _ => None,
        },
        BatchFieldKey::SpiceCertificateSubject => match &connection.config {
            ConnectionType::Spice {
                endpoint: _,
                settings,
                ..
            } => Some(
                settings
                    .security
                    .certificate_subject
                    .clone()
                    .unwrap_or_default(),
            ),
            _ => None,
        },
        BatchFieldKey::SpiceRequireTls => match &connection.config {
            ConnectionType::Spice {
                endpoint: _,
                settings,
                ..
            } => Some(bool_text(settings.security.require_tls)),
            _ => None,
        },
        BatchFieldKey::SpiceDynamicResolution => match &connection.config {
            ConnectionType::Spice {
                endpoint: _,
                settings,
                ..
            } => Some(bool_text(settings.dynamic_resolution)),
            _ => None,
        },
        BatchFieldKey::SpiceFitWindow => match &connection.config {
            ConnectionType::Spice {
                endpoint: _,
                settings,
                ..
            } => Some(bool_text(settings.fit_window)),
            _ => None,
        },
        BatchFieldKey::SpiceViewOnly => match &connection.config {
            ConnectionType::Spice {
                endpoint: _,
                settings,
                ..
            } => Some(bool_text(settings.view_only)),
            _ => None,
        },
        BatchFieldKey::SpiceClipboardToGuest => match &connection.config {
            ConnectionType::Spice {
                endpoint: _,
                settings,
                ..
            } => Some(bool_text(settings.clipboard.can_send())),
            _ => None,
        },
        BatchFieldKey::SpiceClipboardFromGuest => match &connection.config {
            ConnectionType::Spice {
                endpoint: _,
                settings,
                ..
            } => Some(bool_text(settings.clipboard.can_receive())),
            _ => None,
        },
        BatchFieldKey::SpiceAudioPlayback => match &connection.config {
            ConnectionType::Spice {
                endpoint: _,
                settings,
                ..
            } => Some(bool_text(settings.audio_playback)),
            _ => None,
        },
        BatchFieldKey::SpiceFileTransfer => match &connection.config {
            ConnectionType::Spice {
                endpoint: _,
                settings,
                ..
            } => Some(bool_text(settings.file_transfer)),
            _ => None,
        },
        BatchFieldKey::SpiceReconnectAttempts => match &connection.config {
            ConnectionType::Spice {
                endpoint: _,
                settings,
                ..
            } => Some(settings.reconnect_attempts.to_string()),
            _ => None,
        },
        BatchFieldKey::Tags => Some(connection.tags.join(", ")),
        BatchFieldKey::Host => {
            endpoint_values(&connection.config).map(|(host, _, _)| host.to_string())
        }
        BatchFieldKey::Account => Some(
            connection
                .auth
                .as_ref()
                .and_then(|auth| auth.saved_account_id())
                .unwrap_or_default()
                .to_string(),
        ),
        BatchFieldKey::Password => Some(match connection.auth.as_ref() {
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
        BatchFieldKey::AuthMode => Some(
            connection
                .auth
                .as_ref()
                .map(|auth| auth.mode.clone())
                .filter(|mode| !mode.is_empty())
                .unwrap_or_else(|| "password".to_string()),
        ),
        BatchFieldKey::Otp => Some(
            connection
                .auth
                .as_ref()
                .and_then(|auth| auth.otp_id.as_deref())
                .unwrap_or_default()
                .to_string(),
        ),
        BatchFieldKey::OtpAutoFill => Some(bool_text(
            connection
                .auth
                .as_ref()
                .is_some_and(|auth| auth.auto_fill_otp),
        )),
        BatchFieldKey::Key => Some(
            connection
                .auth
                .as_ref()
                .and_then(|auth| auth.key_id.as_deref())
                .unwrap_or_default()
                .to_string(),
        ),
        BatchFieldKey::SshAuthAgentEndpoint => match &connection.config {
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
        BatchFieldKey::SshAuthAgentEnvironment => match &connection.config {
            ConnectionType::Ssh {
                auth_agent_endpoint,
                ..
            } => Some(match auth_agent_endpoint {
                Some(nyaterm_core::SshAgentEndpoint::Environment { variable }) => variable.clone(),
                _ => String::new(),
            }),
            _ => None,
        },
        BatchFieldKey::SshAuthAgentSocket => match &connection.config {
            ConnectionType::Ssh {
                auth_agent_endpoint,
                ..
            } => Some(match auth_agent_endpoint {
                Some(nyaterm_core::SshAgentEndpoint::UnixSocket { path }) => path.clone(),
                _ => String::new(),
            }),
            _ => None,
        },
        BatchFieldKey::Proxy => Some(
            connection
                .network
                .as_ref()
                .and_then(|network| network.proxy_id.as_deref())
                .unwrap_or_default()
                .to_string(),
        ),
        BatchFieldKey::ProxyJump => Some(
            connection
                .network
                .as_ref()
                .and_then(|network| network.proxy_jump_id.as_deref())
                .unwrap_or_default()
                .to_string(),
        ),
        BatchFieldKey::ShellPath => match &connection.config {
            ConnectionType::LocalTerminal { shell_path, .. } => Some(shell_path.clone()),
            _ => None,
        },
        BatchFieldKey::ShellArgs => match &connection.config {
            ConnectionType::LocalTerminal { shell_args, .. } => Some(shell_args.clone()),
            _ => None,
        },
        BatchFieldKey::WorkingDir => match &connection.config {
            ConnectionType::LocalTerminal { working_dir, .. } => {
                Some(working_dir.clone().unwrap_or_default())
            }
            _ => None,
        },
        BatchFieldKey::SerialPort => match &connection.config {
            ConnectionType::Serial { port_name, .. } => Some(port_name.clone()),
            _ => None,
        },
        BatchFieldKey::BaudRate => match &connection.config {
            ConnectionType::Serial { baud_rate, .. } => Some(baud_rate.to_string()),
            _ => None,
        },
        BatchFieldKey::DataBits => match &connection.config {
            ConnectionType::Serial { data_bits, .. } => Some(data_bits.to_string()),
            _ => None,
        },
        BatchFieldKey::Parity => match &connection.config {
            ConnectionType::Serial { parity, .. } => Some(parity.clone()),
            _ => None,
        },
        BatchFieldKey::StopBits => match &connection.config {
            ConnectionType::Serial { stop_bits, .. } => Some(stop_bits.clone()),
            _ => None,
        },
        BatchFieldKey::FlowControl => match &connection.config {
            ConnectionType::Serial { flow_control, .. } => Some(flow_control.as_str().to_string()),
            _ => None,
        },
        BatchFieldKey::TelnetRawTcp => match &connection.config {
            ConnectionType::Telnet { raw_tcp_cli, .. } => Some(bool_text(*raw_tcp_cli)),
            _ => None,
        },
        BatchFieldKey::TelnetKeepAlive => match &connection.config {
            ConnectionType::Telnet {
                keep_alive_interval_secs,
                ..
            } => Some(keep_alive_interval_secs.to_string()),
            _ => None,
        },
        BatchFieldKey::TelnetLocalEcho => match &connection.config {
            ConnectionType::Telnet { local_echo, .. } => Some(bool_text(*local_echo)),
            _ => None,
        },
        BatchFieldKey::TelnetLocalLineEdit => match &connection.config {
            ConnectionType::Telnet {
                local_line_edit, ..
            } => Some(bool_text(*local_line_edit)),
            _ => None,
        },
        BatchFieldKey::TelnetForceChar => match &connection.config {
            ConnectionType::Telnet {
                force_character_at_a_time,
                ..
            } => Some(bool_text(*force_character_at_a_time)),
            _ => None,
        },
        BatchFieldKey::TelnetSendNaws => match &connection.config {
            ConnectionType::Telnet { send_naws, .. } => Some(bool_text(*send_naws)),
            _ => None,
        },
        BatchFieldKey::TelnetSendSga => match &connection.config {
            ConnectionType::Telnet { send_sga, .. } => Some(bool_text(*send_sga)),
            _ => None,
        },
        BatchFieldKey::TelnetEnterMode => match &connection.config {
            ConnectionType::Telnet { enter_mode, .. } => Some(enter_mode.clone()),
            _ => None,
        },
        BatchFieldKey::TelnetAutoLoginEnabled => match &connection.config {
            ConnectionType::Telnet { auto_login, .. } => Some(bool_text(auto_login.enabled)),
            _ => None,
        },
        BatchFieldKey::TelnetAutoLoginWake => match &connection.config {
            ConnectionType::Telnet { auto_login, .. } => {
                Some(bool_text(auto_login.send_wake_enter))
            }
            _ => None,
        },
        BatchFieldKey::TelnetAutoLoginTimeout => match &connection.config {
            ConnectionType::Telnet { auto_login, .. } => Some(auto_login.timeout_ms.to_string()),
            _ => None,
        },
        BatchFieldKey::TelnetAutoLoginRetries => match &connection.config {
            ConnectionType::Telnet { auto_login, .. } => Some(auto_login.max_retries.to_string()),
            _ => None,
        },
        BatchFieldKey::TelnetAutoLoginUserPrompt => match &connection.config {
            ConnectionType::Telnet { auto_login, .. } => {
                Some(auto_login.username_prompt_regex.clone().unwrap_or_default())
            }
            _ => None,
        },
        BatchFieldKey::TelnetAutoLoginPasswordPrompt => match &connection.config {
            ConnectionType::Telnet { auto_login, .. } => {
                Some(auto_login.password_prompt_regex.clone().unwrap_or_default())
            }
            _ => None,
        },
        BatchFieldKey::TelnetAutoLoginSuccessPrompt => match &connection.config {
            ConnectionType::Telnet { auto_login, .. } => {
                Some(auto_login.success_prompt_regex.clone().unwrap_or_default())
            }
            _ => None,
        },
        BatchFieldKey::TelnetAutoLoginFailurePrompt => match &connection.config {
            ConnectionType::Telnet { auto_login, .. } => {
                Some(auto_login.failure_prompt_regex.clone().unwrap_or_default())
            }
            _ => None,
        },
        BatchFieldKey::RdpDomain => match &connection.config {
            ConnectionType::Rdp { domain, .. } => Some(domain.clone()),
            _ => None,
        },
        BatchFieldKey::RdpBackend => match &connection.config {
            ConnectionType::Rdp { backend, .. } => Some(backend.clone()),
            _ => None,
        },
        BatchFieldKey::RdpDisplayMode => match &connection.config {
            ConnectionType::Rdp { display, .. } => Some(display.mode.clone()),
            _ => None,
        },
        BatchFieldKey::RdpWidth => match &connection.config {
            ConnectionType::Rdp { display, .. } => Some(display.width.to_string()),
            _ => None,
        },
        BatchFieldKey::RdpHeight => match &connection.config {
            ConnectionType::Rdp { display, .. } => Some(display.height.to_string()),
            _ => None,
        },
        BatchFieldKey::RdpWallpaper => match &connection.config {
            ConnectionType::Rdp { display, .. } => Some(bool_text(display.display_wallpaper)),
            _ => None,
        },
        BatchFieldKey::RdpThemes => match &connection.config {
            ConnectionType::Rdp { display, .. } => Some(bool_text(display.display_themes)),
            _ => None,
        },
        BatchFieldKey::RdpFontSmoothing => match &connection.config {
            ConnectionType::Rdp { display, .. } => Some(bool_text(display.enable_font_smoothing)),
            _ => None,
        },
        BatchFieldKey::RdpDesktopComposition => match &connection.config {
            ConnectionType::Rdp { display, .. } => {
                Some(bool_text(display.enable_desktop_composition))
            }
            _ => None,
        },
        BatchFieldKey::RdpDisableFullWindowDrag => match &connection.config {
            ConnectionType::Rdp { display, .. } => {
                Some(bool_text(display.disable_full_window_drag))
            }
            _ => None,
        },
        BatchFieldKey::RdpDisableMenuAnimations => match &connection.config {
            ConnectionType::Rdp { display, .. } => Some(bool_text(display.disable_menu_animations)),
            _ => None,
        },
        BatchFieldKey::RdpDisableCursorShadow => match &connection.config {
            ConnectionType::Rdp { display, .. } => Some(bool_text(display.disable_cursor_shadow)),
            _ => None,
        },
        BatchFieldKey::RdpDisableCursorBlinking => match &connection.config {
            ConnectionType::Rdp { display, .. } => Some(bool_text(display.disable_cursor_blinking)),
            _ => None,
        },
        BatchFieldKey::RdpAutoReconnect => match &connection.config {
            ConnectionType::Rdp { reconnect, .. } => Some(bool_text(reconnect.enabled)),
            _ => None,
        },
        BatchFieldKey::RdpReconnectAttempts => match &connection.config {
            ConnectionType::Rdp { reconnect, .. } => Some(reconnect.max_attempts.to_string()),
            _ => None,
        },
        BatchFieldKey::RdpRedirectClipboard => match &connection.config {
            ConnectionType::Rdp { redirection, .. } => Some(bool_text(redirection.clipboard)),
            _ => None,
        },
        BatchFieldKey::RdpRedirectDrives => match &connection.config {
            ConnectionType::Rdp { redirection, .. } => Some(bool_text(redirection.drives)),
            _ => None,
        },
        BatchFieldKey::RdpDriveLetters => match &connection.config {
            ConnectionType::Rdp { redirection, .. } => Some(redirection.drive_letters.clone()),
            _ => None,
        },
        BatchFieldKey::RdpCustomDrivePath => match &connection.config {
            ConnectionType::Rdp { redirection, .. } => Some(redirection.custom_drive_path.clone()),
            _ => None,
        },
        BatchFieldKey::RdpRedirectPrinters => match &connection.config {
            ConnectionType::Rdp { redirection, .. } => Some(bool_text(redirection.printers)),
            _ => None,
        },
        BatchFieldKey::RdpRedirectPorts => match &connection.config {
            ConnectionType::Rdp { redirection, .. } => Some(bool_text(redirection.ports)),
            _ => None,
        },
        BatchFieldKey::RdpRedirectSmartcards => match &connection.config {
            ConnectionType::Rdp { redirection, .. } => Some(bool_text(redirection.smartcards)),
            _ => None,
        },
        BatchFieldKey::RdpRedirectMicrophone => match &connection.config {
            ConnectionType::Rdp { redirection, .. } => Some(bool_text(redirection.microphone)),
            _ => None,
        },
        BatchFieldKey::RdpRedirectWebauthn => match &connection.config {
            ConnectionType::Rdp { redirection, .. } => Some(bool_text(redirection.webauthn)),
            _ => None,
        },
        BatchFieldKey::RdpAudio => match &connection.config {
            ConnectionType::Rdp { redirection, .. } => Some(redirection.audio.clone()),
            _ => None,
        },
        BatchFieldKey::VncScaleMode => match &connection.config {
            ConnectionType::Vnc { display, .. } => Some(display.scale_mode.clone()),
            _ => None,
        },
        BatchFieldKey::VncClipboard => match &connection.config {
            ConnectionType::Vnc { clipboard, .. } => Some(bool_text(clipboard.enabled)),
            _ => None,
        },
        BatchFieldKey::VncShared => match &connection.config {
            ConnectionType::Vnc { shared, .. } => Some(bool_text(*shared)),
            _ => None,
        },
        BatchFieldKey::VncViewOnly => match &connection.config {
            ConnectionType::Vnc { view_only, .. } => Some(bool_text(*view_only)),
            _ => None,
        },
        BatchFieldKey::VncReconnect => match &connection.config {
            ConnectionType::Vnc { reconnect, .. } => Some(bool_text(reconnect.enabled)),
            _ => None,
        },
        BatchFieldKey::VncReconnectAttempts => match &connection.config {
            ConnectionType::Vnc { reconnect, .. } => Some(reconnect.max_attempts.to_string()),
            _ => None,
        },
        BatchFieldKey::AdbSerial => match &connection.config {
            ConnectionType::Adb { device, .. } => Some(device.serial()),
            _ => None,
        },
        BatchFieldKey::AdbDirectory => match &connection.config {
            ConnectionType::Adb { directory, .. } => Some(directory.clone()),
            _ => None,
        },
        BatchFieldKey::AdbScreenQuality => match &connection.config {
            ConnectionType::Adb { screen, .. } => Some(
                nyaterm_core::models::android::ScreenQualityPreset::ALL
                    .into_iter()
                    .find(|preset| preset.matches(*screen))
                    .map(|preset| match preset {
                        nyaterm_core::models::android::ScreenQualityPreset::Smooth => "smooth",
                        nyaterm_core::models::android::ScreenQualityPreset::Balanced => "balanced",
                        nyaterm_core::models::android::ScreenQualityPreset::Sharp => "sharp",
                    })
                    .unwrap_or("custom")
                    .to_string(),
            ),
            _ => None,
        },
        BatchFieldKey::AdbScreenMaxSize => match &connection.config {
            ConnectionType::Adb { screen, .. } => Some(screen.max_size.to_string()),
            _ => None,
        },
        BatchFieldKey::AdbScreenFps => match &connection.config {
            ConnectionType::Adb { screen, .. } => Some(screen.max_fps.to_string()),
            _ => None,
        },
        BatchFieldKey::AdbScreenBitRate => match &connection.config {
            ConnectionType::Adb { screen, .. } => Some(screen.bit_rate.to_string()),
            _ => None,
        },
        BatchFieldKey::AdbScreenPowerOn => match &connection.config {
            ConnectionType::Adb { screen, .. } => Some(bool_text(screen.power_on)),
            _ => None,
        },
        BatchFieldKey::AdbScreenOrientation => match &connection.config {
            ConnectionType::Adb { screen, .. } => {
                Some(screen.capture_orientation.server_value().to_string())
            }
            _ => None,
        },
        BatchFieldKey::FtpInitialDirectory => match &connection.config {
            ConnectionType::Ftp {
                initial_directory, ..
            } => Some(initial_directory.clone()),
            _ => None,
        },
        BatchFieldKey::FtpLocalInitialDirectory => match &connection.config {
            ConnectionType::Ftp {
                local_initial_directory,
                ..
            } => Some(local_initial_directory.clone()),
            _ => None,
        },
        BatchFieldKey::FtpPassiveMode => match &connection.config {
            ConnectionType::Ftp { passive_mode, .. } => Some(passive_mode.clone()),
            _ => None,
        },
        BatchFieldKey::FtpFilenameEncodingMode => match &connection.config {
            ConnectionType::Ftp {
                ftp_filename_encoding_mode,
                ..
            } => Some(ftp_filename_encoding_mode.clone()),
            _ => None,
        },
        BatchFieldKey::FtpFilenameEncoding => match &connection.config {
            ConnectionType::Ftp {
                ftp_filename_encoding,
                ..
            } => Some(ftp_filename_encoding.clone()),
            _ => None,
        },
        BatchFieldKey::FtpAsciiMode => match &connection.config {
            ConnectionType::Ftp { ftp_ascii_mode, .. } => Some(bool_text(*ftp_ascii_mode)),
            _ => None,
        },
        BatchFieldKey::FtpSecurityMode => match &connection.config {
            ConnectionType::Ftp {
                ftp_security_mode, ..
            } => Some(ftp_security_mode.clone()),
            _ => None,
        },
        BatchFieldKey::FtpTlsVersion => match &connection.config {
            ConnectionType::Ftp {
                ftp_tls_version, ..
            } => Some(ftp_tls_version.clone()),
            _ => None,
        },
        BatchFieldKey::FtpConnectTimeout => match &connection.config {
            ConnectionType::Ftp {
                connect_timeout_ms, ..
            } => Some(connect_timeout_ms.to_string()),
            _ => None,
        },
        BatchFieldKey::RecordingUseGlobal => Some(bool_text(connection.recording.is_none())),
        BatchFieldKey::RecordingAutoStart => Some(bool_text(
            connection
                .recording
                .as_ref()
                .and_then(|recording| recording.auto_start)
                .unwrap_or(false),
        )),
        BatchFieldKey::RecordingMode => Some(
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
        BatchFieldKey::AgentExternal => {
            agent_forwarding_value(connection, |sources, _| bool_text(sources.external_agent))
        }
        BatchFieldKey::AgentStoredKeys => {
            agent_forwarding_value(connection, |sources, _| bool_text(sources.stored_keys))
        }
        BatchFieldKey::AgentPolicy => agent_forwarding_value(connection, |_, config| {
            match config.policy {
                nyaterm_core::SshAgentForwardingPolicy::All => "all",
                nyaterm_core::SshAgentForwardingPolicy::Allowlist { .. } => "allowlist",
            }
            .to_string()
        }),
        // The endpoint list is a composite read-only surface, so it keeps a
        // descriptive summary instead of a single scalar control value.
        BatchFieldKey::AgentEndpoints => agent_forwarding_value(connection, |sources, _| {
            sources
                .external_agent_endpoints
                .iter()
                .map(match_endpoint_label)
                .collect::<Vec<_>>()
                .join(", ")
        }),
        BatchFieldKey::Group => Some(connection.group_id.clone().unwrap_or_default()),
        BatchFieldKey::Description => Some(connection.description.clone().unwrap_or_default()),
        BatchFieldKey::Icon => Some(
            connection
                .icon
                .clone()
                .unwrap_or_else(|| super::options::default_icon_value().unwrap_or_default()),
        ),
        BatchFieldKey::IconAutoDetect => Some(bool_text(connection.icon_auto_detect_enabled())),
        BatchFieldKey::Port => {
            endpoint_values(&connection.config).map(|(_, port, _)| port.to_string())
        }
        BatchFieldKey::Username => {
            endpoint_values(&connection.config).map(|(_, _, username)| username.to_string())
        }
        BatchFieldKey::Encoding => config_encoding(&connection.config).map(str::to_string),
        BatchFieldKey::Backspace => config_backspace(&connection.config).map(str::to_string),
        BatchFieldKey::SshProfile => {
            if !is_ssh {
                return None;
            }
            Some(match connection.ssh_profile {
                SshProfile::Standard => "standard".to_string(),
                SshProfile::NetworkDevice => "network_device".to_string(),
            })
        }
        BatchFieldKey::TerminalType => {
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
        BatchFieldKey::SshCompatibility => {
            if !is_ssh {
                return None;
            }
            Some(bool_text(connection.ssh_compatibility_mode))
        }
        BatchFieldKey::DynamicTabTitle => {
            config_dynamic_tab_title(&connection.config).map(bool_text)
        }
        BatchFieldKey::SftpEnabled => {
            if !is_ssh {
                return None;
            }
            Some(bool_text(connection.sftp.enabled))
        }
        BatchFieldKey::SftpCompatibility => {
            if !is_ssh {
                return None;
            }
            Some(bool_text(connection.sftp.compatibility_mode))
        }
        BatchFieldKey::SftpCwdFollow => {
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
        BatchFieldKey::SftpShellTimeout => {
            if !is_ssh {
                return None;
            }
            Some(connection.sftp.shell_detection_timeout_ms.to_string())
        }
        BatchFieldKey::SftpFilenameEncoding => {
            if !is_ssh {
                return None;
            }
            Some(connection.sftp.filename_encoding.clone())
        }
        BatchFieldKey::SftpPipelineDepth => {
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
        BatchFieldKey::X11 => match &connection.config {
            ConnectionType::Ssh { x11_forwarding, .. } => Some(bool_text(*x11_forwarding)),
            _ => None,
        },
        BatchFieldKey::AgentForwarding => match &connection.config {
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
        BatchFieldKey::PostLoginEnabled => {
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
        BatchFieldKey::PostLoginCommand => {
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
        BatchFieldKey::PostLoginDelay => {
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
        BatchFieldKey::SshAlgorithmMode => {
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
        BatchFieldKey::SshKex => ssh_algorithm_list(connection, |algorithms| &algorithms.kex),
        BatchFieldKey::SshCiphers => {
            ssh_algorithm_list(connection, |algorithms| &algorithms.ciphers)
        }
        BatchFieldKey::SshMacs => ssh_algorithm_list(connection, |algorithms| &algorithms.macs),
        BatchFieldKey::SshHostKeys => {
            ssh_algorithm_list(connection, |algorithms| &algorithms.host_keys)
        }
        BatchFieldKey::AllowShortRsaHostKeys => {
            if !is_ssh {
                return None;
            }
            Some(bool_text(connection.allow_short_rsa_host_keys))
        }
        BatchFieldKey::RdpUseNla => match &connection.config {
            ConnectionType::Rdp { security, .. } => Some(bool_text(security.use_nla)),
            _ => None,
        },
        BatchFieldKey::RdpCertificatePolicy => match &connection.config {
            ConnectionType::Rdp { security, .. } => Some(security.certificate_policy.clone()),
            _ => None,
        },
        BatchFieldKey::RdpAdminSession => match &connection.config {
            ConnectionType::Rdp { security, .. } => {
                Some(bool_text(security.administrative_session))
            }
            _ => None,
        },
        BatchFieldKey::VncSecurityMode => match &connection.config {
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

type BatchBasis = (
    HashMap<BatchFieldKey, FieldBasis>,
    HashMap<BatchFieldKey, String>,
    HashMap<BatchFieldKey, bool>,
);

pub(super) fn compute_rdp_field_basis(
    by_id: &HashMap<&str, &SavedConnection>,
    targets: &[(String, ConnectionBatchKind)],
    field: BatchFieldKey,
    edit: &ConnectionBatchEdit,
) -> (FieldBasis, Option<String>, BatchFieldApplicability) {
    let (basis, applicability) = domain_basis::rdp_field_basis_indexed(
        by_id,
        targets,
        field.rdp_group().expect("RDP field"),
        edit,
        |connection| field_display_value(connection, field).expect("RDP field value"),
    );
    let (basis, value) = match basis {
        BatchValueBasis::Inapplicable => (FieldBasis::Inapplicable, None),
        BatchValueBasis::Mixed => (FieldBasis::Mixed, None),
        BatchValueBasis::Uniform(value) => (FieldBasis::Uniform, Some(value)),
    };
    (basis, value, applicability)
}

pub(super) fn batch_skip_reasons_label(counts: ConnectionBatchSkipCounts) -> String {
    ConnectionBatchSkipReason::ALL
        .into_iter()
        .filter(|reason| counts.count(*reason) > 0)
        .map(|reason| {
            let count = counts.count(reason);
            t!(
                match reason {
                    ConnectionBatchSkipReason::UnsupportedProtocol => "assets.batch.skipProtocol",
                    ConnectionBatchSkipReason::MstscBackend => "assets.batch.skipMstsc",
                    ConnectionBatchSkipReason::NonFixedRdpDisplay => "assets.batch.skipNonFixed",
                    ConnectionBatchSkipReason::GlobalRecording =>
                        "assets.batch.skipGlobalRecording",
                },
                count = count
            )
            .to_string()
        })
        .collect::<Vec<_>>()
        .join(&t!("assets.batch.mixedProtocolsSeparator"))
}

/// Common fields compare all targets; a protocol scope compares only its original members.
pub(super) fn compute_batch_basis(
    connections: &[SavedConnection],
    ids: &[String],
    target: Option<ConnectionBatchKind>,
) -> BatchBasis {
    let by_id: HashMap<&str, &SavedConnection> = connections
        .iter()
        .map(|connection| (connection.id.as_str(), connection))
        .collect();

    compute_batch_basis_indexed(&by_id, ids, target)
}

pub(super) fn compute_batch_basis_indexed(
    by_id: &HashMap<&str, &SavedConnection>,
    ids: &[String],
    target: Option<ConnectionBatchKind>,
) -> BatchBasis {
    let mut basis = HashMap::new();
    let mut values = HashMap::new();
    let mut bool_values = HashMap::new();

    for field in EDITABLE_FIELDS.into_iter().chain(DISPLAY_FIELDS) {
        if target.map_or(!is_kind_independent(field), |kind| {
            is_kind_independent(field) || !batch_field_in_kind(kind, field)
        }) {
            basis.insert(field, FieldBasis::Inapplicable);
            continue;
        }
        let summary = domain_basis::common_value(ids.iter().filter_map(|id| {
            let connection = by_id.get(id.as_str())?;
            if target
                .is_some_and(|kind| ConnectionBatchKind::from_config(&connection.config) != kind)
            {
                return None;
            }
            Some(field_display_value(connection, field))
        }));

        match summary {
            BatchValueBasis::Inapplicable => {
                basis.insert(field, FieldBasis::Inapplicable);
            }
            BatchValueBasis::Mixed => {
                basis.insert(field, FieldBasis::Mixed);
            }
            BatchValueBasis::Uniform(value) => {
                basis.insert(field, FieldBasis::Uniform);
                if field.is_bool() {
                    bool_values.insert(field, value == "true");
                } else {
                    values.insert(field, value);
                }
            }
        }
    }

    (basis, values, bool_values)
}

pub(in crate::features) const BATCH_KINDS: [ConnectionBatchKind; 9] = [
    ConnectionBatchKind::Ssh,
    ConnectionBatchKind::Telnet,
    ConnectionBatchKind::Rdp,
    ConnectionBatchKind::Vnc,
    ConnectionBatchKind::Spice,
    ConnectionBatchKind::Adb,
    ConnectionBatchKind::Ftp,
    ConnectionBatchKind::LocalTerminal,
    ConnectionBatchKind::Serial,
];

pub(in crate::features) fn batch_kind_value(kind: ConnectionBatchKind) -> &'static str {
    match kind {
        ConnectionBatchKind::Ssh => "ssh",
        ConnectionBatchKind::Telnet => "telnet",
        ConnectionBatchKind::Rdp => "rdp",
        ConnectionBatchKind::Vnc => "vnc",
        ConnectionBatchKind::Spice => "spice",
        ConnectionBatchKind::Adb => "adb",
        ConnectionBatchKind::LocalTerminal => "local",
        ConnectionBatchKind::Serial => "serial",
        ConnectionBatchKind::Ftp => "ftp",
    }
}

pub(in crate::features) fn parse_batch_kind(value: &str) -> Option<ConnectionBatchKind> {
    BATCH_KINDS
        .into_iter()
        .find(|kind| batch_kind_value(*kind) == value)
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
