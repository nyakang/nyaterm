use super::draft::{AssetBatchEditDraft, BatchEditMode};
use super::fields::{BatchFieldId, DISPLAY_FIELDS, EDITABLE_FIELDS, FieldBasis};
use nyaterm_core::models::connection_batch::{
    ConnectionBatchEdit, ConnectionBatchKind, ConnectionBatchSkipCounts, ConnectionBatchSkipReason,
    rdp_batch_field_skip_reason,
};
use nyaterm_core::models::{
    ConnectionType, SavedConnection, SftpCwdFollowMode, SshAlgorithmMode, SshProfile,
    SshTerminalType,
};
use rust_i18n::t;
use std::collections::HashMap;
/// Counts for the protocol scopes; empty protocols never appear.
pub(super) fn asset_batch_kind_counts(
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
    field: BatchFieldId,
) -> Option<String> {
    if draft.mode(field) == BatchEditMode::Keep && draft.basis(field) != FieldBasis::Uniform {
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
        // Keep the original reference/default sentinel for display lookup. Empty
        // options are filtered from Set mode; removing a value requires Clear.
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
        BatchFieldId::Encoding => matches!(kind, Ssh | LocalTerminal | Telnet | Serial),
        BatchFieldId::Backspace => matches!(kind, Ssh | Telnet | Serial),
        BatchFieldId::DynamicTabTitle => matches!(kind, Ssh | LocalTerminal),
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
                .unwrap_or_else(|| super::options::default_icon_value().unwrap_or_default()),
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

type BatchBasis = (
    HashMap<BatchFieldId, FieldBasis>,
    HashMap<BatchFieldId, String>,
    HashMap<BatchFieldId, bool>,
);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::features) struct BatchFieldApplicability {
    pub total: usize,
    pub applicable: usize,
    pub skipped_reasons: ConnectionBatchSkipCounts,
}

pub(super) fn compute_rdp_field_basis(
    connections: &[SavedConnection],
    targets: &[(String, ConnectionBatchKind)],
    field: BatchFieldId,
    edit: &ConnectionBatchEdit,
) -> (FieldBasis, Option<String>, BatchFieldApplicability) {
    let group = field.rdp_group().expect("RDP field");
    let by_id: HashMap<&str, &SavedConnection> = connections
        .iter()
        .map(|connection| (connection.id.as_str(), connection))
        .collect();
    let mut applicability = BatchFieldApplicability::default();
    let mut first = None;
    let mut mixed = false;
    for (id, kind) in targets {
        if *kind != ConnectionBatchKind::Rdp {
            continue;
        }
        applicability.total += 1;
        let connection = by_id.get(id.as_str()).copied();
        let reason = connection.map_or(
            Some(ConnectionBatchSkipReason::UnsupportedProtocol),
            |connection| rdp_batch_field_skip_reason(&connection.config, edit, group),
        );
        if let Some(reason) = reason {
            applicability.skipped_reasons.add(reason, 1);
            continue;
        }
        applicability.applicable += 1;
        let value = field_display_value(connection.expect("applicable connection"), field)
            .expect("RDP field value");
        match &first {
            None => first = Some(value),
            Some(existing) if *existing != value => mixed = true,
            _ => {}
        }
    }
    let basis = if applicability.applicable == 0 {
        FieldBasis::Inapplicable
    } else if mixed {
        FieldBasis::Mixed
    } else {
        FieldBasis::Uniform
    };
    (basis, (!mixed).then_some(first).flatten(), applicability)
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
        let mut seen: Option<String> = None;
        let mut mixed = false;
        for id in ids {
            let Some(connection) = by_id.get(id.as_str()) else {
                continue;
            };
            if target
                .is_some_and(|kind| ConnectionBatchKind::from_config(&connection.config) != kind)
            {
                continue;
            }
            let Some(value) = field_display_value(connection, field) else {
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

    (basis, values, bool_values)
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
