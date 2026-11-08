//! Shared option tables for the connection editor's selects and tabs.
//!
//! The batch editor seeds its selects from the same functions so the two
//! surfaces cannot drift apart. Every label key, entry order, disabled entry
//! and special entry (none / follow global / automatic) is owned here.

use nyaterm_core::character_encoding::CharacterEncoding;
use nyaterm_core::{
    ConnectionType, Group, OtpEntry, ProxyConfig, RecordingMode, SavedConnection, SavedPassword,
    SshAgentEndpoint, SshAgentForwardingPolicy, SshKey, SshProfile, SshTerminalType,
    resolve_ssh_terminal_type,
};
use rust_i18n::t;

use crate::features::pages::connections::list::ConnectionEditorChoice;

/// Password / no-auth tabs, in the connection editor's order.
pub(in crate::features) fn password_auth_choices(
    selected: &str,
    protocol_requires_no_auth: bool,
) -> Vec<ConnectionEditorChoice> {
    let none_selected = protocol_requires_no_auth || selected == "none";
    let mut password =
        ConnectionEditorChoice::new(Some("password".to_string()), t!("dialog.password"), false);
    password.disabled = protocol_requires_no_auth;
    vec![
        password,
        ConnectionEditorChoice::new(
            Some("none".to_string()),
            t!("dialog.noAuthentication"),
            none_selected,
        ),
    ]
}

pub(in crate::features) fn ssh_auth_choices(selected: &str) -> Vec<ConnectionEditorChoice> {
    [
        ("none", t!("dialog.noAuthentication")),
        ("password", t!("dialog.password")),
        ("key", t!("dialog.privateKey")),
        ("agent", t!("dialog.sshAgent")),
    ]
    .into_iter()
    .map(|(value, label)| {
        ConnectionEditorChoice::new(Some(value.to_string()), label, selected == value)
    })
    .collect()
}

pub(in crate::features) fn ssh_key_choices(
    keys: &[SshKey],
    selected: Option<&str>,
) -> Vec<ConnectionEditorChoice> {
    let mut choices = vec![ConnectionEditorChoice::new(
        None,
        t!("dialog.none"),
        selected.is_none(),
    )];
    choices.extend(keys.iter().map(|key| {
        let search_text = [Some(key.name.as_str()), key.key_file_path.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        let subtitle = key
            .key_file_path
            .clone()
            .or_else(|| key.cert_file_path.clone());
        let mut choice = ConnectionEditorChoice::new(
            Some(key.id.clone()),
            key.name.clone(),
            selected == Some(key.id.as_str()),
        )
        .search_text(search_text);
        if let Some(subtitle) = subtitle {
            choice = choice.subtitle(subtitle);
        }
        choice
    }));
    choices
}

/// Saved-password select: a plain entry list plus a stale-reference entry for
/// an id that is no longer in the password catalog.
pub(in crate::features) fn password_choices(
    passwords: &[SavedPassword],
    selected: Option<&str>,
    none_selected: bool,
    missing_reference: Option<&str>,
) -> Vec<ConnectionEditorChoice> {
    let mut choices = vec![ConnectionEditorChoice::new(
        None,
        t!("dialog.none"),
        none_selected,
    )];
    choices.extend(passwords.iter().map(|password| {
        ConnectionEditorChoice::new(
            Some(password.id.clone()),
            password.name.clone(),
            selected == Some(password.id.as_str()),
        )
        .subtitle(password.username.clone())
    }));
    if let Some(id) = missing_reference.filter(|id| !passwords.iter().any(|entry| entry.id == *id))
    {
        choices.push(ConnectionEditorChoice::new(
            Some(id.to_string()),
            t!("passwordManager.missingAccount"),
            true,
        ));
    }
    choices
}

/// Account select: the same catalog, but the empty entry means a manually
/// entered identity and each entry reports the saved-password state.
pub(in crate::features) fn account_choices(
    passwords: &[SavedPassword],
    selected: Option<&str>,
    none_selected: bool,
    missing_reference: Option<&str>,
) -> Vec<ConnectionEditorChoice> {
    let mut choices = password_choices(passwords, selected, none_selected, missing_reference);
    if let Some(choice) = choices.first_mut().filter(|choice| choice.value.is_none()) {
        choice.label = t!("authForm.manualIdentity").to_string();
    }
    for choice in choices.iter_mut().filter(|choice| choice.value.is_some()) {
        let Some(id) = choice.value.as_deref() else {
            continue;
        };
        let Some(account) = passwords.iter().find(|entry| entry.id == id) else {
            continue;
        };
        let username = account.username.trim();
        let username = if username.is_empty() {
            t!("authForm.accountUsernameFallback").to_string()
        } else {
            username.to_string()
        };
        let password_status = if account.has_password {
            t!("authForm.passwordSaved")
        } else {
            t!("authForm.passwordNotSaved")
        };
        choice.subtitle = Some(format!("{username} · {password_status}"));
    }
    choices
}

pub(in crate::features) fn otp_choices(
    entries: &[OtpEntry],
    selected: Option<&str>,
) -> Vec<ConnectionEditorChoice> {
    let mut choices = vec![ConnectionEditorChoice::new(
        None,
        t!("dialog.noOtp"),
        selected.is_none(),
    )];
    choices.extend(entries.iter().map(|entry| {
        let label = if entry.issuer.is_empty() {
            entry.username.clone()
        } else if entry.username.is_empty() {
            entry.issuer.clone()
        } else {
            format!("{} ({})", entry.issuer, entry.username)
        };
        let subtitle = [
            entry.otp_type.to_ascii_uppercase(),
            entry.algorithm.clone(),
            entry.digits.to_string(),
        ]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
        let search_text = [
            entry.issuer.as_str(),
            entry.username.as_str(),
            entry.otp_type.as_str(),
            entry.algorithm.as_str(),
        ]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
        ConnectionEditorChoice::new(
            Some(entry.id.clone()),
            label,
            selected == Some(entry.id.as_str()),
        )
        .search_text(search_text)
        .subtitle(subtitle)
    }));
    choices
}

pub(in crate::features) fn proxy_choices(
    proxies: &[ProxyConfig],
    selected: Option<&str>,
) -> Vec<ConnectionEditorChoice> {
    let mut choices = vec![ConnectionEditorChoice::new(
        None,
        t!("dialog.noProxy"),
        selected.is_none(),
    )];
    choices.extend(proxies.iter().map(|proxy| {
        let protocol = proxy.protocol.to_ascii_uppercase();
        let subtitle = if proxy.protocol == "proxycommand" {
            [protocol.as_str(), proxy.command.as_deref().unwrap_or("")]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" · ")
        } else {
            [
                format!("{} {}:{}", protocol, proxy.host, proxy.port),
                proxy.username.clone().unwrap_or_default(),
            ]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
        };
        let search_text = [
            proxy.name.clone(),
            proxy.protocol.clone(),
            proxy.host.clone(),
            proxy.port.to_string(),
            proxy.username.clone().unwrap_or_default(),
            proxy.command.clone().unwrap_or_default(),
        ]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
        ConnectionEditorChoice::new(
            Some(proxy.id.clone()),
            proxy.name.clone(),
            selected == Some(proxy.id.as_str()),
        )
        .search_text(search_text)
        .subtitle(subtitle)
    }));
    choices
}

pub(in crate::features) fn proxy_jump_choices(
    connections: &[SavedConnection],
    groups: &[Group],
    current_id: Option<&str>,
    selected: Option<&str>,
) -> Vec<ConnectionEditorChoice> {
    let mut choices = vec![ConnectionEditorChoice::new(
        None,
        t!("dialog.noProxyJump"),
        selected.is_none(),
    )];
    choices.extend(
        connections
            .iter()
            .filter(|connection| matches!(connection.config, ConnectionType::Ssh { .. }))
            .filter(|connection| current_id != Some(connection.id.as_str()))
            .filter(|connection| {
                !super::connection_proxy_jump_would_cycle(connections, current_id, connection)
            })
            .map(|connection| {
                let (host, port, username) = match &connection.config {
                    ConnectionType::Ssh {
                        host,
                        port,
                        username,
                        ..
                    } => (host.as_str(), *port, username.as_str()),
                    _ => ("", 22, ""),
                };
                let group_path = connection
                    .group_id
                    .as_deref()
                    .and_then(|id| super::connection_group_path_label(groups, id))
                    .unwrap_or_default();
                let host_port = format!("{host}:{port}");
                let subtitle = [group_path.as_str(), host_port.as_str(), username]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ");
                let search_text = [
                    connection.name.as_str(),
                    host,
                    username,
                    group_path.as_str(),
                ]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
                ConnectionEditorChoice::new(
                    Some(connection.id.clone()),
                    connection.name.clone(),
                    selected == Some(connection.id.as_str()),
                )
                .search_text(search_text)
                .subtitle(subtitle)
            }),
    );
    choices
}

pub(in crate::features) fn backspace_choices(selected: &str) -> Vec<ConnectionEditorChoice> {
    [
        ("del", t!("dialog.backspaceDel")),
        ("ctrl-h", t!("dialog.backspaceCtrlH")),
    ]
    .into_iter()
    .map(|(value, label)| {
        ConnectionEditorChoice::new(
            Some(value.to_string()),
            label,
            match value {
                "ctrl-h" => matches!(selected, "ctrl-h" | "bs" | "ctrl_h"),
                _ => !matches!(selected, "ctrl-h" | "bs" | "ctrl_h"),
            },
        )
    })
    .collect()
}

pub(in crate::features) fn telnet_enter_choices(selected: &str) -> Vec<ConnectionEditorChoice> {
    [
        ("crlf", "CRLF (\\r\\n)"),
        ("cr", "CR (\\r)"),
        ("lf", "LF (\\n)"),
    ]
    .into_iter()
    .map(|(value, label)| {
        ConnectionEditorChoice::new(Some(value.to_string()), label, selected == value)
    })
    .collect()
}

pub(in crate::features) fn sftp_pipeline_depth_choices(
    depth: Option<u32>,
) -> Vec<ConnectionEditorChoice> {
    std::iter::once(ConnectionEditorChoice::new(
        None,
        t!("dialog.sftpPipelineAutomatic"),
        depth.is_none(),
    ))
    .chain((4..=64).map(|value| {
        ConnectionEditorChoice::new(
            Some(value.to_string()),
            value.to_string(),
            depth == Some(value),
        )
    }))
    .collect()
}

pub(in crate::features) fn encoding_choices(selected: &str) -> Vec<ConnectionEditorChoice> {
    std::iter::once("global")
        .chain(
            CharacterEncoding::ALL
                .into_iter()
                .map(|encoding| encoding.label()),
        )
        .map(|value| {
            let label = if value == "global" {
                t!("connection.encodingFollowGlobal").to_string()
            } else {
                value.to_string()
            };
            ConnectionEditorChoice::new(
                Some(value.to_string()),
                label,
                (value == "global"
                    && (selected.trim().is_empty()
                        || selected.trim().eq_ignore_ascii_case("global")))
                    || selected == value
                    || CharacterEncoding::parse(selected)
                        .is_ok_and(|encoding| encoding.label() == value),
            )
        })
        .collect()
}

pub(in crate::features) fn ssh_profile_choices(profile: SshProfile) -> Vec<ConnectionEditorChoice> {
    [
        ("standard", t!("dialog.sshProfileStandard")),
        ("network_device", t!("dialog.sshProfileNetworkDevice")),
    ]
    .into_iter()
    .map(|(value, label)| {
        ConnectionEditorChoice::new(
            Some(value.to_string()),
            label,
            matches!(
                (profile, value),
                (SshProfile::Standard, "standard") | (SshProfile::NetworkDevice, "network_device")
            ),
        )
    })
    .collect()
}

pub(in crate::features) fn terminal_type_values() -> [&'static str; 6] {
    ["xterm-256color", "xterm", "vt100", "vt220", "ansi", "linux"]
}

pub(in crate::features) fn ssh_terminal_type_choices(
    profile: SshProfile,
    selected: Option<SshTerminalType>,
) -> Vec<ConnectionEditorChoice> {
    let effective_terminal = resolve_ssh_terminal_type(profile, selected);
    let mut choices = vec![ConnectionEditorChoice::new(
        None,
        t!(
            "dialog.sshTerminalTypeDefault",
            value = effective_terminal.as_str()
        ),
        selected.is_none(),
    )];
    choices.extend(terminal_type_values().into_iter().map(|value| {
        ConnectionEditorChoice::new(
            Some(value.to_string()),
            value,
            selected.is_some() && effective_terminal.as_str() == value,
        )
    }));
    choices
}

pub(in crate::features) fn rdp_backend_choices(
    backend: &str,
    uses_mstsc: bool,
) -> Vec<ConnectionEditorChoice> {
    let mut choices = vec![ConnectionEditorChoice::new(
        Some("builtin".into()),
        t!("dialog.rdpBackendBuiltin"),
        matches!(backend, "builtin" | "internal" | "auto"),
    )];
    let mut windows_backend = ConnectionEditorChoice::new(
        Some("windows".into()),
        t!("dialog.rdpBackendWindows"),
        matches!(backend, "windows" | "active-x"),
    );
    windows_backend.disabled = !cfg!(windows);
    choices.push(windows_backend);
    let mut mstsc_backend = ConnectionEditorChoice::new(
        Some("mstsc".into()),
        t!("dialog.rdpBackendMstsc"),
        uses_mstsc,
    );
    mstsc_backend.disabled = !cfg!(windows);
    choices.push(mstsc_backend);
    if !matches!(
        backend,
        "auto" | "builtin" | "internal" | "windows" | "active-x" | "mstsc"
    ) {
        choices.push(ConnectionEditorChoice::new(
            Some(backend.to_string()),
            t!("dialog.rdpBackendUnsupported"),
            true,
        ));
    }
    choices
}

pub(in crate::features) fn rdp_certificate_policy_choices(
    policy: &str,
) -> Vec<ConnectionEditorChoice> {
    [
        ("prompt", t!("dialog.rdpCertificatePrompt")),
        ("strict", t!("dialog.rdpCertificateStrict")),
        ("accept-temporarily", t!("dialog.rdpCertificateTemporary")),
    ]
    .into_iter()
    .map(|(value, label)| {
        let selected = match policy {
            "strict" | "reject_changed" | "reject-changed" => value == "strict",
            "insecure" | "accept-temporarily" => value == "accept-temporarily",
            _ => value == "prompt",
        };
        ConnectionEditorChoice::new(Some(value.to_string()), label, selected)
    })
    .collect()
}

pub(in crate::features) fn rdp_display_mode_choices(mode: &str) -> Vec<ConnectionEditorChoice> {
    [
        ("fit-window", t!("dialog.rdpDisplayFitWindow")),
        ("fixed", t!("dialog.rdpDisplayFixed")),
    ]
    .into_iter()
    .map(|(value, label)| {
        let current = match mode {
            "fixed" | "native" => "fixed",
            _ => "fit-window",
        };
        ConnectionEditorChoice::new(Some(value.to_string()), label, current == value)
    })
    .collect()
}

pub(in crate::features) fn vnc_security_choices(mode: &str) -> Vec<ConnectionEditorChoice> {
    [
        ("auto", t!("dialog.vncSecurityAuto")),
        ("none", t!("dialog.vncSecurityNone")),
        ("vnc-auth", t!("dialog.vncSecurityPassword")),
    ]
    .into_iter()
    .map(|(value, label)| {
        ConnectionEditorChoice::new(Some(value.to_string()), label, mode == value)
    })
    .collect()
}

pub(in crate::features) fn vnc_scale_choices(scale_mode: &str) -> Vec<ConnectionEditorChoice> {
    [
        ("fit", t!("dialog.vncScaleFit")),
        ("stretch", t!("dialog.vncScaleStretch")),
        ("actual", t!("dialog.vncScaleActual")),
    ]
    .into_iter()
    .map(|(value, label)| {
        ConnectionEditorChoice::new(Some(value.to_string()), label, scale_mode == value)
    })
    .collect()
}

pub(in crate::features) fn recording_mode_choices(
    mode: RecordingMode,
) -> Vec<ConnectionEditorChoice> {
    [
        ("transcript", t!("dialog.recordingModeTranscript")),
        ("raw", t!("dialog.recordingModeRaw")),
    ]
    .into_iter()
    .map(|(value, label)| {
        ConnectionEditorChoice::new(
            Some(value.to_string()),
            label,
            matches!(
                (mode, value),
                (RecordingMode::Transcript, "transcript") | (RecordingMode::Raw, "raw")
            ),
        )
    })
    .collect()
}

pub(in crate::features) fn sftp_cwd_follow_choices(mode: &str) -> Vec<ConnectionEditorChoice> {
    [
        ("off", t!("dialog.sftpCwdFollowOff")),
        (
            "shell_integration",
            t!("dialog.sftpCwdFollowShellIntegration"),
        ),
        ("rc_file", t!("dialog.sftpCwdFollowRcFile")),
    ]
    .into_iter()
    .map(|(value, label)| {
        ConnectionEditorChoice::new(Some(value.to_string()), label, mode == value)
    })
    .collect()
}

pub(in crate::features) fn sftp_filename_encoding_choices(
    selected: &str,
) -> Vec<ConnectionEditorChoice> {
    std::iter::once("terminal")
        .chain(
            CharacterEncoding::ALL
                .into_iter()
                .map(|encoding| encoding.label()),
        )
        .map(|value| {
            let label = if value == "terminal" {
                t!("dialog.sftpFilenameEncodingFollowTerminal").to_string()
            } else {
                value.to_string()
            };
            ConnectionEditorChoice::new(
                Some(value.to_string()),
                label,
                (value == "terminal"
                    && (selected.trim().is_empty()
                        || ["terminal", "global"]
                            .iter()
                            .any(|follow| selected.trim().eq_ignore_ascii_case(follow))))
                    || selected == value
                    || CharacterEncoding::parse(selected)
                        .is_ok_and(|encoding| encoding.label() == value),
            )
        })
        .collect()
}

pub(in crate::features) fn ssh_algorithm_mode_choices(
    selected: &str,
) -> Vec<ConnectionEditorChoice> {
    [
        ("compatible", t!("dialog.algorithmModeCompatible")),
        ("secure", t!("dialog.algorithmModeSecure")),
        ("custom", t!("dialog.algorithmModeCustom")),
    ]
    .into_iter()
    .map(|(value, label)| {
        ConnectionEditorChoice::new(Some(value.to_string()), label, selected == value)
    })
    .collect()
}

pub(in crate::features) fn serial_port_choices(
    ports: &[String],
    selected: &str,
) -> Vec<ConnectionEditorChoice> {
    let mut choices = Vec::new();
    if !selected.is_empty() && !ports.iter().any(|port| port == selected) {
        choices.push(ConnectionEditorChoice::new(
            Some(selected.to_string()),
            selected.to_string(),
            true,
        ));
    }
    choices.extend(ports.iter().map(|port| {
        ConnectionEditorChoice::new(Some(port.clone()), port.clone(), selected == port)
    }));
    choices
}

pub(in crate::features) fn baud_rate_choices(selected: &str) -> Vec<ConnectionEditorChoice> {
    [
        "9600", "19200", "38400", "57600", "115200", "230400", "460800", "921600",
    ]
    .into_iter()
    .map(|value| {
        ConnectionEditorChoice::new(
            Some(value.to_string()),
            value.to_string(),
            selected == value,
        )
    })
    .collect()
}

pub(in crate::features) fn data_bits_choices(selected: &str) -> Vec<ConnectionEditorChoice> {
    ["5", "6", "7", "8"]
        .into_iter()
        .map(|value| {
            ConnectionEditorChoice::new(
                Some(value.to_string()),
                value.to_string(),
                selected == value,
            )
        })
        .collect()
}

pub(in crate::features) fn parity_choices(selected: &str) -> Vec<ConnectionEditorChoice> {
    [
        ("none", t!("dialog.parityNone")),
        ("odd", t!("dialog.parityOdd")),
        ("even", t!("dialog.parityEven")),
        ("mark", t!("dialog.parityMark")),
        ("space", t!("dialog.paritySpace")),
    ]
    .into_iter()
    .map(|(value, label)| {
        ConnectionEditorChoice::new(Some(value.to_string()), label, selected == value)
    })
    .collect()
}

pub(in crate::features) fn flow_control_choices(selected: &str) -> Vec<ConnectionEditorChoice> {
    [
        ("none", t!("dialog.serialFlowControlNone")),
        ("software", t!("dialog.serialFlowControlSoftware")),
        ("hardware", t!("dialog.serialFlowControlHardware")),
    ]
    .into_iter()
    .map(|(value, label)| {
        ConnectionEditorChoice::new(Some(value.to_string()), label, selected == value)
    })
    .collect()
}

pub(in crate::features) fn stop_bits_choices(selected: &str) -> Vec<ConnectionEditorChoice> {
    ["1", "1.5", "2"]
        .into_iter()
        .map(|value| {
            ConnectionEditorChoice::new(
                Some(value.to_string()),
                value.to_string(),
                selected == value,
            )
        })
        .collect()
}

pub(in crate::features) fn ssh_agent_forwarding_policy_choices(
    policy: &SshAgentForwardingPolicy,
) -> Vec<ConnectionEditorChoice> {
    vec![
        ConnectionEditorChoice::new(
            Some("allowlist".to_string()),
            t!("dialog.sshAgentPolicyAllowlist"),
            matches!(policy, SshAgentForwardingPolicy::Allowlist { .. }),
        ),
        ConnectionEditorChoice::new(
            Some("all".to_string()),
            t!("dialog.sshAgentPolicyAll"),
            matches!(policy, SshAgentForwardingPolicy::All),
        ),
    ]
}

pub(in crate::features) fn ssh_agent_endpoint_value(endpoint: &SshAgentEndpoint) -> &'static str {
    match endpoint {
        SshAgentEndpoint::Auto => "auto",
        SshAgentEndpoint::Environment { .. } => "environment",
        SshAgentEndpoint::UnixSocket { .. } => "unix_socket",
        SshAgentEndpoint::WindowsOpenSsh => "windows_openssh",
        SshAgentEndpoint::Pageant => "pageant",
    }
}

pub(in crate::features) fn ssh_agent_endpoint_choices(
    endpoint: &SshAgentEndpoint,
) -> Vec<ConnectionEditorChoice> {
    let selected = ssh_agent_endpoint_value(endpoint);
    let mut choices = vec![ConnectionEditorChoice::new(
        Some("auto".to_string()),
        t!("dialog.sshAgentAuto"),
        selected == "auto",
    )];
    if cfg!(unix) {
        choices.extend([
            ConnectionEditorChoice::new(
                Some("environment".to_string()),
                t!("dialog.sshAgentEnvironment"),
                selected == "environment",
            ),
            ConnectionEditorChoice::new(
                Some("unix_socket".to_string()),
                t!("dialog.sshAgentUnixSocket"),
                selected == "unix_socket",
            ),
        ]);
    }
    if cfg!(windows) {
        choices.extend([
            ConnectionEditorChoice::new(
                Some("windows_openssh".to_string()),
                t!("dialog.sshAgentWindowsOpenSsh"),
                selected == "windows_openssh",
            ),
            ConnectionEditorChoice::new(
                Some("pageant".to_string()),
                t!("dialog.sshAgentPageant"),
                selected == "pageant",
            ),
        ]);
    }
    if !nyaterm_core::ssh_agent_endpoint_supported_on_current_platform(endpoint) {
        let label = match endpoint {
            SshAgentEndpoint::Environment { .. } => t!("dialog.sshAgentEnvironment"),
            SshAgentEndpoint::UnixSocket { .. } => t!("dialog.sshAgentUnixSocket"),
            SshAgentEndpoint::Pageant => t!("dialog.sshAgentPageant"),
            SshAgentEndpoint::WindowsOpenSsh => t!("dialog.sshAgentWindowsOpenSsh"),
            SshAgentEndpoint::Auto => t!("dialog.sshAgentAuto"),
        };
        choices.push(ConnectionEditorChoice::new(
            Some(selected.to_string()),
            format!("{} ({})", label, t!("dialog.sshAgentUnavailableOnPlatform")),
            true,
        ));
    }
    choices
}

pub(in crate::features) fn shell_choices(selected: &str) -> Vec<ConnectionEditorChoice> {
    [
        ("powershell.exe", t!("dialog.shellPowerShell")),
        ("cmd.exe", t!("dialog.shellCmd")),
        ("bash", t!("dialog.shellBash")),
        ("wsl.exe", t!("dialog.shellWsl")),
        ("wt.exe", t!("dialog.shellWindowsTerminal")),
    ]
    .into_iter()
    .map(|(value, label)| {
        ConnectionEditorChoice::new(Some(value.to_string()), label, selected == value)
    })
    .collect()
}
