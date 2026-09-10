use std::collections::HashSet;

use nyaterm_ui::ChildWindowSlot;

use super::super::connection_runtime::ConnectionEditorToggle;
use crate::models::{
    ConnectionEditorAdvancedTab, ConnectionEditorField, ConnectionEditorPasswordSource,
    ConnectionEditorRdpTab, ConnectionEditorSelect, ConnectionEditorSshAlgorithmTab,
    ConnectionEditorState, ConnectionEditorTelnetTab, ConnectionGroupEditorState,
    ConnectionKindTab,
};

pub(super) fn clear_connection_editor_runtime_state(
    draft: &mut Option<ConnectionEditorState>,
    icon_picker_open: &mut bool,
    group_select_open: &mut bool,
    window: &mut ChildWindowSlot,
) {
    *icon_picker_open = false;
    *group_select_open = false;
    *draft = None;
    window.clear();
}

pub(super) fn select_saved_connection_after_editor_save(
    selected_ids: &mut HashSet<String>,
    last_selected_id: &mut Option<String>,
    expanded_group_ids: &mut HashSet<String>,
    connection_id: String,
    group_id: Option<String>,
) {
    selected_ids.clear();
    selected_ids.insert(connection_id.clone());
    *last_selected_id = Some(connection_id);
    if let Some(group_id) = group_id {
        expanded_group_ids.insert(group_id);
    }
}

pub(super) fn connection_editor_inline_panel_draft(
    draft: &Option<ConnectionEditorState>,
    has_window: bool,
    window_open_pending: bool,
) -> Option<ConnectionEditorState> {
    if has_window || window_open_pending {
        return None;
    }
    draft.clone()
}

pub(super) fn set_connection_editor_icon(
    draft: &mut Option<ConnectionEditorState>,
    icon: Option<&str>,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    editor.icon = icon
        .map(str::trim)
        .filter(|icon| !icon.is_empty())
        .map(ToOwned::to_owned);
    // Choosing an icon by hand is an explicit decision, so stop letting the
    // remote system overwrite it. Clearing the icon hands control back.
    editor.icon_auto_detect = editor.icon.is_none();
    editor.error = None;
    true
}

pub(super) fn set_connection_editor_icon_auto_detect(
    draft: &mut Option<ConnectionEditorState>,
    enabled: bool,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    if editor.icon_auto_detect == enabled {
        return false;
    }
    editor.icon_auto_detect = enabled;
    true
}

pub(super) fn set_connection_editor_select_value(
    draft: &mut Option<ConnectionEditorState>,
    select: ConnectionEditorSelect,
    value: Option<String>,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    match select {
        ConnectionEditorSelect::Authentication => {
            editor.auth_mode = value.unwrap_or_else(|| "password".to_string());
            if editor.auth_mode == "none" {
                clear_connection_editor_password_secret(editor);
                editor.key_id = None;
            }
        }
        ConnectionEditorSelect::SshAgentEndpoint => {
            editor.agent_endpoint = match value.as_deref() {
                Some("environment") => nyaterm_core::SshAgentEndpoint::Environment {
                    variable: "SSH_AUTH_SOCK".to_string(),
                },
                Some("pageant") => nyaterm_core::SshAgentEndpoint::Pageant,
                Some("windows_openssh") => nyaterm_core::SshAgentEndpoint::WindowsOpenSsh,
                Some("unix_socket") => nyaterm_core::SshAgentEndpoint::UnixSocket {
                    path: String::new(),
                },
                _ => nyaterm_core::SshAgentEndpoint::Auto,
            };
        }
        ConnectionEditorSelect::SshAgentForwardingPolicy => {
            editor.agent_forwarding_config.policy = match value.as_deref() {
                Some("all") => {
                    editor.agent_allow_all_confirmed = false;
                    nyaterm_core::SshAgentForwardingPolicy::All
                }
                _ => nyaterm_core::SshAgentForwardingPolicy::Allowlist {
                    fingerprints: match &editor.agent_forwarding_config.policy {
                        nyaterm_core::SshAgentForwardingPolicy::Allowlist { fingerprints } => {
                            fingerprints.clone()
                        }
                        nyaterm_core::SshAgentForwardingPolicy::All => Vec::new(),
                    },
                },
            };
        }
        ConnectionEditorSelect::Group => {
            editor.group_id = value;
            editor.new_group_name.clear();
            editor.pending_group_name = None;
            editor.pending_group_parent_id = None;
            editor.focused_field = ConnectionEditorField::Name;
        }
        ConnectionEditorSelect::SavedPassword => editor.password_id = value,
        ConnectionEditorSelect::SshKey => editor.key_id = value,
        ConnectionEditorSelect::Otp => {
            editor.otp_id = value;
            if editor.otp_id.is_none() {
                editor.auto_fill_otp = false;
            }
        }
        ConnectionEditorSelect::Proxy => editor.proxy_id = value,
        ConnectionEditorSelect::ProxyJump => editor.proxy_jump_id = value,
        ConnectionEditorSelect::Backspace => {
            editor.backspace_mode = value.unwrap_or_else(|| "del".to_string());
        }
        ConnectionEditorSelect::Encoding => {
            editor.encoding = value.unwrap_or_else(|| "global".to_string());
        }
        ConnectionEditorSelect::SftpCwdFollowMode => {
            editor.sftp_cwd_follow_mode = value.unwrap_or_else(|| "shell_integration".to_string());
        }
        ConnectionEditorSelect::SftpPipelineDepth => {
            editor.sftp_pipeline_depth = value
                .and_then(|value| value.parse::<u32>().ok())
                .map(|value| value.clamp(4, 64));
        }
        ConnectionEditorSelect::SftpFilenameEncoding => {
            editor.sftp_filename_encoding = value.unwrap_or_else(|| "terminal".to_string());
        }
        ConnectionEditorSelect::SshAlgorithmMode => {
            let mode = value.unwrap_or_else(|| "compatible".to_string());
            if mode == "custom" {
                let defaults = &nyaterm_transport::supported_ssh_algorithms().compatible;
                if editor.ssh_algorithm_kex.is_empty() {
                    editor.ssh_algorithm_kex.clone_from(&defaults.kex);
                }
                if editor.ssh_algorithm_ciphers.is_empty() {
                    editor.ssh_algorithm_ciphers.clone_from(&defaults.ciphers);
                }
                if editor.ssh_algorithm_macs.is_empty() {
                    editor.ssh_algorithm_macs.clone_from(&defaults.macs);
                }
                if editor.ssh_algorithm_host_keys.is_empty() {
                    editor
                        .ssh_algorithm_host_keys
                        .clone_from(&defaults.host_keys);
                }
            } else {
                editor.ssh_algorithm_kex.clear();
                editor.ssh_algorithm_ciphers.clear();
                editor.ssh_algorithm_macs.clear();
                editor.ssh_algorithm_host_keys.clear();
            }
            editor.ssh_algorithm_mode = mode;
        }
        ConnectionEditorSelect::SshProfile => {
            editor.ssh_profile = match value.as_deref() {
                Some("network_device") => nyaterm_core::SshProfile::NetworkDevice,
                _ => nyaterm_core::SshProfile::Standard,
            };
        }
        ConnectionEditorSelect::SshTerminalType => {
            editor.terminal_type = match value.as_deref() {
                Some("xterm-256color") => Some(nyaterm_core::SshTerminalType::Xterm256Color),
                Some("xterm") => Some(nyaterm_core::SshTerminalType::Xterm),
                Some("vt100") => Some(nyaterm_core::SshTerminalType::Vt100),
                Some("vt220") => Some(nyaterm_core::SshTerminalType::Vt220),
                Some("ansi") => Some(nyaterm_core::SshTerminalType::Ansi),
                Some("linux") => Some(nyaterm_core::SshTerminalType::Linux),
                _ => None,
            };
        }
        ConnectionEditorSelect::RdpCertificatePolicy => {
            editor.rdp_security.certificate_policy = value.unwrap_or_else(|| "prompt".to_string());
        }
        ConnectionEditorSelect::RdpDisplayMode => {
            editor.rdp_display.mode = value.unwrap_or_else(|| "fit-window".to_string());
        }
        ConnectionEditorSelect::RdpClipboardMode => {
            editor.rdp_clipboard.mode = value.unwrap_or_else(|| "text-only".to_string());
        }
        ConnectionEditorSelect::VncSecurityMode => {
            editor.vnc_security.mode = value.unwrap_or_else(|| "auto".to_string());
        }
        ConnectionEditorSelect::VncScaleMode => {
            editor.vnc_display.scale_mode = value.unwrap_or_else(|| "fit".to_string());
        }
        ConnectionEditorSelect::RecordingMode => {
            let recording = editor.recording.get_or_insert_with(Default::default);
            recording.mode = Some(if value.as_deref() == Some("raw") {
                nyaterm_core::RecordingMode::Raw
            } else {
                nyaterm_core::RecordingMode::Transcript
            });
        }
        ConnectionEditorSelect::TelnetEnterMode => {
            editor.telnet_enter_mode = value.unwrap_or_else(|| "cr".to_string());
        }
        ConnectionEditorSelect::Shell => {
            editor.shell_path = value.unwrap_or_else(|| "powershell.exe".to_string());
        }
        ConnectionEditorSelect::SerialPort => editor.serial_port = value.unwrap_or_default(),
        ConnectionEditorSelect::BaudRate => {
            editor.baud_rate = value.unwrap_or_else(|| "115200".to_string());
        }
        ConnectionEditorSelect::DataBits => {
            editor.data_bits = value.unwrap_or_else(|| "8".to_string());
        }
        ConnectionEditorSelect::Parity => {
            editor.parity = value.unwrap_or_else(|| "none".to_string());
        }
        ConnectionEditorSelect::StopBits => {
            editor.stop_bits = value.unwrap_or_else(|| "1".to_string());
        }
    }
    editor.error = None;
    true
}

pub(super) fn set_connection_editor_password_source(
    draft: &mut Option<ConnectionEditorState>,
    source: ConnectionEditorPasswordSource,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    editor.password_source = source;
    match source {
        ConnectionEditorPasswordSource::Ask => clear_connection_editor_password_secret(editor),
        ConnectionEditorPasswordSource::Direct => editor.password_id = None,
        ConnectionEditorPasswordSource::Saved => {
            editor.password.expose_secret_mut().clear();
            editor.existing_password = None;
        }
    }
    editor.error = None;
    true
}

pub(super) fn set_connection_editor_advanced_tab(
    draft: &mut Option<ConnectionEditorState>,
    tab: ConnectionEditorAdvancedTab,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    match tab {
        ConnectionEditorAdvancedTab::Proxy
        | ConnectionEditorAdvancedTab::JumpHost
        | ConnectionEditorAdvancedTab::TwoFactor
        | ConnectionEditorAdvancedTab::AgentForwarding => editor.advanced_network_tab = tab,
        ConnectionEditorAdvancedTab::PostLogin
        | ConnectionEditorAdvancedTab::Terminal
        | ConnectionEditorAdvancedTab::Sftp
        | ConnectionEditorAdvancedTab::X11
        | ConnectionEditorAdvancedTab::Backspace => editor.advanced_behavior_tab = tab,
    }
    if matches!(
        editor.focused_field,
        ConnectionEditorField::PostLoginCommand | ConnectionEditorField::PostLoginDelay
    ) && tab != ConnectionEditorAdvancedTab::PostLogin
    {
        editor.focused_field = ConnectionEditorField::Name;
    }
    true
}

fn ssh_algorithm_values_mut(
    editor: &mut ConnectionEditorState,
    tab: ConnectionEditorSshAlgorithmTab,
) -> &mut Vec<String> {
    match tab {
        ConnectionEditorSshAlgorithmTab::KeyExchange => &mut editor.ssh_algorithm_kex,
        ConnectionEditorSshAlgorithmTab::Ciphers => &mut editor.ssh_algorithm_ciphers,
        ConnectionEditorSshAlgorithmTab::Macs => &mut editor.ssh_algorithm_macs,
        ConnectionEditorSshAlgorithmTab::HostKeys => &mut editor.ssh_algorithm_host_keys,
    }
}

fn ssh_algorithm_is_supported(tab: ConnectionEditorSshAlgorithmTab, id: &str) -> bool {
    let supported = nyaterm_transport::supported_ssh_algorithms();
    let options = match tab {
        ConnectionEditorSshAlgorithmTab::KeyExchange => &supported.kex,
        ConnectionEditorSshAlgorithmTab::Ciphers => &supported.ciphers,
        ConnectionEditorSshAlgorithmTab::Macs => &supported.macs,
        ConnectionEditorSshAlgorithmTab::HostKeys => &supported.host_keys,
    };
    options.iter().any(|option| option.id == id)
}

pub(super) fn set_connection_editor_ssh_algorithm_tab(
    draft: &mut Option<ConnectionEditorState>,
    tab: ConnectionEditorSshAlgorithmTab,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    if editor.ssh_algorithm_tab == tab {
        return false;
    }
    editor.ssh_algorithm_tab = tab;
    editor.error = None;
    true
}

pub(super) fn set_connection_editor_ssh_algorithm_enabled(
    draft: &mut Option<ConnectionEditorState>,
    tab: ConnectionEditorSshAlgorithmTab,
    id: &str,
    enabled: bool,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    if editor.ssh_algorithm_mode != "custom" {
        return false;
    }
    let values = ssh_algorithm_values_mut(editor, tab);
    let position = values.iter().position(|value| value == id);
    if enabled {
        if position.is_some() || !ssh_algorithm_is_supported(tab, id) {
            return false;
        }
        values.push(id.to_string());
    } else {
        let Some(position) = position else {
            return false;
        };
        if values.len() <= 1 {
            return false;
        }
        values.remove(position);
    }
    editor.error = None;
    true
}

pub(super) fn move_connection_editor_ssh_algorithm(
    draft: &mut Option<ConnectionEditorState>,
    tab: ConnectionEditorSshAlgorithmTab,
    id: &str,
    direction: i8,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    if editor.ssh_algorithm_mode != "custom" {
        return false;
    }
    let values = ssh_algorithm_values_mut(editor, tab);
    let Some(index) = values.iter().position(|value| value == id) else {
        return false;
    };
    let target = if direction < 0 {
        index.checked_sub(1)
    } else if direction > 0 && index + 1 < values.len() {
        Some(index + 1)
    } else {
        None
    };
    let Some(target) = target else {
        return false;
    };
    values.swap(index, target);
    editor.error = None;
    true
}

pub(super) fn add_connection_editor_agent_endpoint(
    draft: &mut Option<ConnectionEditorState>,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    if editor
        .agent_forwarding_config
        .sources
        .external_agent_endpoints
        .len()
        >= nyaterm_core::MAX_SSH_AGENT_FORWARDING_ENDPOINTS
    {
        return false;
    }
    editor
        .agent_forwarding_config
        .sources
        .external_agent_endpoints
        .push(nyaterm_core::SshAgentEndpoint::Auto);
    editor.agent_forwarding_config.sources.external_agent = true;
    editor.error = None;
    true
}

pub(super) fn select_connection_editor_agent_endpoint(
    draft: &mut Option<ConnectionEditorState>,
    index: usize,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    if index
        >= editor
            .agent_forwarding_config
            .sources
            .external_agent_endpoints
            .len()
    {
        return false;
    }
    editor.agent_forwarding_endpoint_index = index;
    editor.error = None;
    true
}

pub(super) fn set_connection_editor_agent_endpoint_type(
    draft: &mut Option<ConnectionEditorState>,
    index: usize,
    value: &str,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    let Some(endpoint) = editor
        .agent_forwarding_config
        .sources
        .external_agent_endpoints
        .get_mut(index)
    else {
        return false;
    };
    *endpoint = match value {
        "environment" => nyaterm_core::SshAgentEndpoint::Environment {
            variable: "SSH_AUTH_SOCK".to_string(),
        },
        "unix_socket" => nyaterm_core::SshAgentEndpoint::UnixSocket {
            path: String::new(),
        },
        "pageant" => nyaterm_core::SshAgentEndpoint::Pageant,
        "windows_openssh" => nyaterm_core::SshAgentEndpoint::WindowsOpenSsh,
        _ => nyaterm_core::SshAgentEndpoint::Auto,
    };
    editor.agent_forwarding_endpoint_index = index;
    editor.error = None;
    true
}

pub(super) fn remove_connection_editor_agent_endpoint(
    draft: &mut Option<ConnectionEditorState>,
    index: usize,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    let endpoints = &mut editor
        .agent_forwarding_config
        .sources
        .external_agent_endpoints;
    if index >= endpoints.len() {
        return false;
    }
    endpoints.remove(index);
    editor.agent_forwarding_endpoint_index = editor
        .agent_forwarding_endpoint_index
        .min(endpoints.len().saturating_sub(1));
    editor.error = None;
    true
}

pub(super) fn move_connection_editor_agent_endpoint(
    draft: &mut Option<ConnectionEditorState>,
    index: usize,
    direction: i8,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    let endpoints = &mut editor
        .agent_forwarding_config
        .sources
        .external_agent_endpoints;
    let target = if direction < 0 {
        index.checked_sub(1)
    } else if direction > 0 && index + 1 < endpoints.len() {
        Some(index + 1)
    } else {
        None
    };
    let Some(target) = target else {
        return false;
    };
    endpoints.swap(index, target);
    editor.agent_forwarding_endpoint_index = target;
    editor.error = None;
    true
}

pub(super) fn toggle_connection_editor_agent_allowlist_fingerprint(
    draft: &mut Option<ConnectionEditorState>,
    fingerprint: &str,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    let nyaterm_core::SshAgentForwardingPolicy::Allowlist { fingerprints } =
        &mut editor.agent_forwarding_config.policy
    else {
        return false;
    };
    let fingerprint = fingerprint.trim();
    if fingerprint.is_empty() {
        return false;
    }
    if let Some(index) = fingerprints.iter().position(|value| value == fingerprint) {
        fingerprints.remove(index);
    } else if fingerprints.len() < nyaterm_core::MAX_SSH_AGENT_FORWARDING_IDENTITIES {
        fingerprints.push(fingerprint.to_string());
    } else {
        return false;
    }
    editor.error = None;
    true
}

pub(super) fn set_connection_editor_telnet_tab(
    draft: &mut Option<ConnectionEditorState>,
    tab: ConnectionEditorTelnetTab,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    editor.telnet_advanced_tab = tab;
    editor.error = None;
    true
}

pub(super) fn set_connection_editor_rdp_tab(
    draft: &mut Option<ConnectionEditorState>,
    tab: ConnectionEditorRdpTab,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    editor.rdp_advanced_tab = tab;
    editor.error = None;
    true
}

pub(super) fn set_connection_editor_kind(
    draft: &mut Option<ConnectionEditorState>,
    kind: ConnectionKindTab,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    editor.kind = kind;
    editor.focused_field = ConnectionEditorField::Name;
    editor.port = match kind {
        ConnectionKindTab::Ssh => {
            if editor.port.trim().is_empty() || editor.port == "23" {
                "22".to_string()
            } else {
                editor.port.clone()
            }
        }
        ConnectionKindTab::Telnet => {
            if editor.port.trim().is_empty() || editor.port == "22" {
                "23".to_string()
            } else {
                editor.port.clone()
            }
        }
        ConnectionKindTab::Rdp => {
            if editor.port.trim().is_empty() || editor.port == "22" || editor.port == "23" {
                "3389".to_string()
            } else {
                editor.port.clone()
            }
        }
        ConnectionKindTab::Vnc => {
            if editor.port.trim().is_empty()
                || editor.port == "22"
                || editor.port == "23"
                || editor.port == "3389"
            {
                "5900".to_string()
            } else {
                editor.port.clone()
            }
        }
        _ => editor.port.clone(),
    };
    editor.error = None;
    true
}

pub(super) fn commit_connection_editor_new_group(
    draft: &mut Option<ConnectionEditorState>,
    required_message: String,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    let name = editor.new_group_name.trim().to_string();
    if name.is_empty() {
        editor.error = Some(required_message);
        return true;
    }
    editor.pending_group_parent_id = if editor.pending_group_name.is_some() {
        editor.pending_group_parent_id.clone()
    } else {
        editor.group_id.clone()
    };
    editor.pending_group_name = Some(name);
    editor.group_id = None;
    editor.new_group_name.clear();
    editor.focused_field = ConnectionEditorField::Name;
    editor.error = None;
    true
}

pub(super) fn toggle_connection_editor_flag(
    draft: &mut Option<ConnectionEditorState>,
    flag: ConnectionEditorToggle,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    match flag {
        ConnectionEditorToggle::AutoFillOtp => {
            editor.auto_fill_otp = editor.otp_id.is_some() && !editor.auto_fill_otp;
        }
        ConnectionEditorToggle::DynamicTabTitle => {
            editor.dynamic_tab_title = !editor.dynamic_tab_title;
        }
        ConnectionEditorToggle::X11 => editor.x11_forwarding = !editor.x11_forwarding,
        ConnectionEditorToggle::AgentForwarding => {
            editor.agent_forwarding_config.enabled = !editor.agent_forwarding_config.enabled;
            if editor.agent_forwarding_config.enabled
                && matches!(
                    editor.agent_forwarding_config.policy,
                    nyaterm_core::SshAgentForwardingPolicy::All
                )
            {
                editor.agent_allow_all_confirmed = false;
            }
        }
        ConnectionEditorToggle::AgentExternal => {
            let enabled = !editor.agent_forwarding_config.sources.external_agent;
            editor.agent_forwarding_config.sources.external_agent = enabled;
            // Match the Tauri form by seeding the common platform endpoint the
            // first time external Agent forwarding is enabled.
            if enabled
                && editor
                    .agent_forwarding_config
                    .sources
                    .external_agent_endpoints
                    .is_empty()
            {
                editor
                    .agent_forwarding_config
                    .sources
                    .external_agent_endpoints
                    .push(default_ssh_agent_forwarding_endpoint());
                editor.agent_forwarding_endpoint_index = 0;
            }
        }
        ConnectionEditorToggle::AgentStoredKeys => {
            editor.agent_forwarding_config.sources.stored_keys =
                !editor.agent_forwarding_config.sources.stored_keys;
        }
        ConnectionEditorToggle::SftpEnabled => editor.sftp_enabled = !editor.sftp_enabled,
        ConnectionEditorToggle::RawTcp => {
            editor.raw_tcp_cli = !editor.raw_tcp_cli;
            if editor.raw_tcp_cli {
                editor.telnet_enter_mode = "cr".to_string();
            }
        }
        ConnectionEditorToggle::LocalEcho => editor.local_echo = !editor.local_echo,
        ConnectionEditorToggle::LocalLineEdit => {
            editor.local_line_edit = !editor.local_line_edit;
        }
        ConnectionEditorToggle::ForceCharacterAtATime => {
            editor.force_character_at_a_time = !editor.force_character_at_a_time;
        }
        ConnectionEditorToggle::SendNaws => {
            if !editor.raw_tcp_cli {
                editor.send_naws = !editor.send_naws;
            }
        }
        ConnectionEditorToggle::SendSga => {
            if !editor.raw_tcp_cli {
                editor.send_sga = !editor.send_sga;
            }
        }
        ConnectionEditorToggle::TelnetAutoLoginEnabled => {
            editor.telnet_auto_login_enabled = !editor.telnet_auto_login_enabled;
        }
        ConnectionEditorToggle::TelnetAutoLoginSendWakeEnter => {
            editor.telnet_auto_login_send_wake_enter = !editor.telnet_auto_login_send_wake_enter;
        }
        ConnectionEditorToggle::PostLogin => {
            editor.post_login_enabled = !editor.post_login_enabled;
        }
        ConnectionEditorToggle::RdpUseNla => {
            editor.rdp_security.use_nla = !editor.rdp_security.use_nla;
        }
        ConnectionEditorToggle::RdpReconnect => {
            editor.rdp_reconnect.enabled = !editor.rdp_reconnect.enabled;
        }
        ConnectionEditorToggle::VncClipboard => {
            editor.vnc_clipboard.enabled = !editor.vnc_clipboard.enabled;
        }
        ConnectionEditorToggle::VncReconnect => {
            editor.vnc_reconnect.enabled = !editor.vnc_reconnect.enabled;
        }
        ConnectionEditorToggle::VncShared => {
            editor.vnc_shared = !editor.vnc_shared;
        }
        ConnectionEditorToggle::VncViewOnly => {
            editor.vnc_view_only = !editor.vnc_view_only;
        }
        ConnectionEditorToggle::RecordingUseGlobal => {
            if editor.recording.is_some() {
                editor.recording = None;
            } else {
                editor.recording = Some(Default::default());
            }
        }
        ConnectionEditorToggle::RecordingAutoStart => {
            let recording = editor.recording.get_or_insert_with(Default::default);
            recording.auto_start = Some(!recording.auto_start.unwrap_or(false));
        }
        ConnectionEditorToggle::Advanced => {
            editor.advanced_open = !editor.advanced_open;
            if !editor.advanced_open
                && matches!(
                    editor.focused_field,
                    ConnectionEditorField::PostLoginCommand | ConnectionEditorField::PostLoginDelay
                )
            {
                editor.focused_field = ConnectionEditorField::Name;
            }
        }
    }
    editor.error = None;
    true
}

fn default_ssh_agent_forwarding_endpoint() -> nyaterm_core::SshAgentEndpoint {
    #[cfg(windows)]
    {
        nyaterm_core::SshAgentEndpoint::WindowsOpenSsh
    }
    #[cfg(not(windows))]
    {
        nyaterm_core::SshAgentEndpoint::Environment {
            variable: "SSH_AUTH_SOCK".to_string(),
        }
    }
}

pub(super) fn insert_connection_editor_description_newline(
    draft: &mut Option<ConnectionEditorState>,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    if editor.focused_field != ConnectionEditorField::Description {
        return false;
    }
    editor.description.push('\n');
    editor.error = None;
    true
}

pub(super) fn advance_connection_editor_focus(draft: &mut Option<ConnectionEditorState>) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    let password_field_visible = editor.auth_mode == "password"
        && editor.password_source == ConnectionEditorPasswordSource::Direct;
    let post_login_fields_visible = editor.advanced_open
        && editor.post_login_enabled
        && editor.advanced_behavior_tab == ConnectionEditorAdvancedTab::PostLogin;
    editor.focused_field = editor.focused_field.next(
        editor.kind,
        editor.auth_mode.as_str(),
        password_field_visible,
        post_login_fields_visible,
    );
    editor.error = None;
    true
}

fn clear_connection_editor_password_secret(editor: &mut ConnectionEditorState) {
    editor.password_source = ConnectionEditorPasswordSource::Ask;
    editor.password_id = None;
    editor.password.expose_secret_mut().clear();
    editor.existing_password = None;
}

pub(super) fn set_connection_editor_error(
    draft: &mut Option<ConnectionEditorState>,
    error: String,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    editor.error = Some(error);
    true
}

pub(super) fn apply_connection_editor_shell_path(
    draft: &mut Option<ConnectionEditorState>,
    shell_path: String,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    editor.shell_path = shell_path;
    editor.error = None;
    true
}

pub(super) fn apply_connection_editor_working_dir(
    draft: &mut Option<ConnectionEditorState>,
    working_dir: String,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    editor.working_dir = working_dir;
    editor.error = None;
    true
}

pub(super) fn set_connection_group_editor_error(
    draft: &mut Option<ConnectionGroupEditorState>,
    error: String,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    editor.error = Some(error);
    true
}

/// Which draft strings become editable fields, and which are secrets.
///
/// Driven off the draft rather than a fixed list so a field that does not apply
/// to the current kind is simply never built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConnectionEditorPlaceholder {
    Empty,
    I18n(&'static str),
    Literal(&'static str),
}

pub(super) fn editor_field_seeds(
    draft: &ConnectionEditorState,
) -> Vec<(
    ConnectionEditorField,
    String,
    bool,
    ConnectionEditorPlaceholder,
)> {
    use ConnectionEditorPlaceholder::{Empty, I18n, Literal};

    vec![
        (
            ConnectionEditorField::Name,
            draft.name.clone(),
            false,
            Empty,
        ),
        (
            ConnectionEditorField::Description,
            draft.description.clone(),
            false,
            I18n("dialog.descriptionPlaceholder"),
        ),
        (
            ConnectionEditorField::NewGroupName,
            draft.new_group_name.clone(),
            false,
            I18n("dialog.newGroupPlaceholder"),
        ),
        (
            ConnectionEditorField::Host,
            draft.host.clone(),
            false,
            Literal("192.168.1.100"),
        ),
        (
            ConnectionEditorField::Port,
            draft.port.clone(),
            false,
            Empty,
        ),
        (
            ConnectionEditorField::Username,
            draft.username.clone(),
            false,
            Empty,
        ),
        (
            ConnectionEditorField::AgentEnvironmentVariable,
            match &draft.agent_endpoint {
                nyaterm_core::SshAgentEndpoint::Environment { variable } => variable.clone(),
                _ => "SSH_AUTH_SOCK".to_string(),
            },
            false,
            Literal("SSH_AUTH_SOCK"),
        ),
        (
            ConnectionEditorField::AgentUnixSocket,
            match &draft.agent_endpoint {
                nyaterm_core::SshAgentEndpoint::UnixSocket { path } => path.clone(),
                _ => String::new(),
            },
            false,
            Literal("/path/to/agent.sock"),
        ),
        (
            ConnectionEditorField::AgentForwardingEnvironmentVariable,
            forwarding_environment_variable(draft),
            false,
            Literal("SSH_AUTH_SOCK"),
        ),
        (
            ConnectionEditorField::AgentForwardingSocketPath,
            forwarding_socket_path(draft),
            false,
            Literal("/path/to/agent.sock"),
        ),
        (
            ConnectionEditorField::Domain,
            draft.domain.clone(),
            false,
            Empty,
        ),
        (
            ConnectionEditorField::Password,
            draft.password.expose_secret().to_owned(),
            true,
            I18n("dialog.passwordPlaceholder"),
        ),
        (
            ConnectionEditorField::ShellPath,
            draft.shell_path.clone(),
            false,
            I18n("dialog.shellPathPlaceholder"),
        ),
        (
            ConnectionEditorField::ShellArgs,
            draft.shell_args.clone(),
            false,
            I18n("dialog.shellArgsPlaceholder"),
        ),
        (
            ConnectionEditorField::WorkingDir,
            draft.working_dir.clone(),
            false,
            I18n("dialog.workingDirPlaceholder"),
        ),
        (
            ConnectionEditorField::SerialPort,
            draft.serial_port.clone(),
            false,
            I18n("dialog.serialPortPlaceholder"),
        ),
        (
            ConnectionEditorField::BaudRate,
            draft.baud_rate.clone(),
            false,
            I18n("dialog.customBaudRatePlaceholder"),
        ),
        (
            ConnectionEditorField::PostLoginCommand,
            draft.post_login_command.clone(),
            false,
            Literal("cd /opt/app"),
        ),
        (
            ConnectionEditorField::PostLoginDelay,
            draft.post_login_delay_ms.clone(),
            false,
            Empty,
        ),
        (
            ConnectionEditorField::SftpShellDetectionTimeout,
            draft.sftp_shell_detection_timeout_ms.clone(),
            false,
            Empty,
        ),
        (
            ConnectionEditorField::TelnetAutoLoginTimeout,
            draft.telnet_auto_login_timeout_ms.clone(),
            false,
            Empty,
        ),
        (
            ConnectionEditorField::TelnetAutoLoginUsernamePrompt,
            draft.telnet_auto_login_username_prompt_regex.clone(),
            false,
            I18n("dialog.telnetAutoLoginUsernamePromptPlaceholder"),
        ),
        (
            ConnectionEditorField::TelnetAutoLoginPasswordPrompt,
            draft.telnet_auto_login_password_prompt_regex.clone(),
            false,
            I18n("dialog.telnetAutoLoginPasswordPromptPlaceholder"),
        ),
        (
            ConnectionEditorField::TelnetAutoLoginSuccessPrompt,
            draft.telnet_auto_login_success_prompt_regex.clone(),
            false,
            I18n("dialog.telnetAutoLoginSuccessPromptPlaceholder"),
        ),
        (
            ConnectionEditorField::TelnetAutoLoginFailurePrompt,
            draft.telnet_auto_login_failure_prompt_regex.clone(),
            false,
            I18n("dialog.telnetAutoLoginFailurePromptPlaceholder"),
        ),
        (
            ConnectionEditorField::TelnetAutoLoginMaxRetries,
            draft.telnet_auto_login_max_retries.clone(),
            false,
            Empty,
        ),
        (
            ConnectionEditorField::RdpDisplayWidth,
            draft.rdp_display.width.to_string(),
            false,
            Empty,
        ),
        (
            ConnectionEditorField::RdpDisplayHeight,
            draft.rdp_display.height.to_string(),
            false,
            Empty,
        ),
        (
            ConnectionEditorField::RdpReconnectAttempts,
            draft.rdp_reconnect.max_attempts.to_string(),
            false,
            Empty,
        ),
        (
            ConnectionEditorField::VncReconnectAttempts,
            draft.vnc_reconnect.max_attempts.to_string(),
            false,
            Empty,
        ),
    ]
}

pub(super) fn forwarding_endpoint_field_seeds(
    draft: &ConnectionEditorState,
) -> Vec<(usize, ConnectionEditorField, String, &'static str)> {
    draft
        .agent_forwarding_config
        .sources
        .external_agent_endpoints
        .iter()
        .enumerate()
        .filter_map(|(index, endpoint)| match endpoint {
            nyaterm_core::SshAgentEndpoint::Environment { variable } => Some((
                index,
                ConnectionEditorField::AgentForwardingEnvironmentVariable,
                variable.clone(),
                "SSH_AUTH_SOCK",
            )),
            nyaterm_core::SshAgentEndpoint::UnixSocket { path } => Some((
                index,
                ConnectionEditorField::AgentForwardingSocketPath,
                path.clone(),
                "/path/to/agent.sock",
            )),
            _ => None,
        })
        .collect()
}

pub(super) fn set_connection_editor_forwarding_endpoint_field(
    draft: &mut Option<ConnectionEditorState>,
    index: usize,
    field: ConnectionEditorField,
    text: String,
) -> bool {
    let Some(editor) = draft.as_mut() else {
        return false;
    };
    let Some(endpoint) = editor
        .agent_forwarding_config
        .sources
        .external_agent_endpoints
        .get_mut(index)
    else {
        return false;
    };
    *endpoint = match field {
        ConnectionEditorField::AgentForwardingEnvironmentVariable => {
            nyaterm_core::SshAgentEndpoint::Environment { variable: text }
        }
        ConnectionEditorField::AgentForwardingSocketPath => {
            nyaterm_core::SshAgentEndpoint::UnixSocket { path: text }
        }
        _ => return false,
    };
    editor.error = None;
    true
}

fn selected_forwarding_endpoint(
    draft: &ConnectionEditorState,
) -> Option<&nyaterm_core::SshAgentEndpoint> {
    draft
        .agent_forwarding_config
        .sources
        .external_agent_endpoints
        .get(draft.agent_forwarding_endpoint_index)
}

fn forwarding_environment_variable(draft: &ConnectionEditorState) -> String {
    match selected_forwarding_endpoint(draft) {
        Some(nyaterm_core::SshAgentEndpoint::Environment { variable }) => variable.clone(),
        _ => "SSH_AUTH_SOCK".to_string(),
    }
}

fn forwarding_socket_path(draft: &ConnectionEditorState) -> String {
    match selected_forwarding_endpoint(draft) {
        Some(nyaterm_core::SshAgentEndpoint::UnixSocket { path }) => path.clone(),
        _ => String::new(),
    }
}

/// Write an edited field back into the draft, clearing any stale validation.
///
/// Previously the error was cleared when a field took focus; a field now takes
/// focus on its own, so the edit itself is what says the message is out of date.
pub(super) fn set_connection_editor_field_text(
    draft: &mut ConnectionEditorState,
    field: ConnectionEditorField,
    text: String,
) {
    draft.error = None;
    match field {
        ConnectionEditorField::Name => draft.name = text,
        ConnectionEditorField::Description => draft.description = text,
        ConnectionEditorField::NewGroupName => draft.new_group_name = text,
        ConnectionEditorField::Host => draft.host = text,
        ConnectionEditorField::Port => draft.port = text,
        ConnectionEditorField::Username => draft.username = text,
        ConnectionEditorField::AgentEnvironmentVariable => {
            draft.agent_endpoint = nyaterm_core::SshAgentEndpoint::Environment { variable: text };
        }
        ConnectionEditorField::AgentUnixSocket => {
            draft.agent_endpoint = nyaterm_core::SshAgentEndpoint::UnixSocket { path: text };
        }
        ConnectionEditorField::AgentForwardingEnvironmentVariable => {
            if let Some(endpoint) = draft
                .agent_forwarding_config
                .sources
                .external_agent_endpoints
                .get_mut(draft.agent_forwarding_endpoint_index)
            {
                *endpoint = nyaterm_core::SshAgentEndpoint::Environment { variable: text };
            }
        }
        ConnectionEditorField::AgentForwardingSocketPath => {
            if let Some(endpoint) = draft
                .agent_forwarding_config
                .sources
                .external_agent_endpoints
                .get_mut(draft.agent_forwarding_endpoint_index)
            {
                *endpoint = nyaterm_core::SshAgentEndpoint::UnixSocket { path: text };
            }
        }
        ConnectionEditorField::Domain => draft.domain = text,
        ConnectionEditorField::Password => draft.password = text.into(),
        ConnectionEditorField::ShellPath => draft.shell_path = text,
        ConnectionEditorField::ShellArgs => draft.shell_args = text,
        ConnectionEditorField::WorkingDir => draft.working_dir = text,
        ConnectionEditorField::SerialPort => draft.serial_port = text,
        ConnectionEditorField::BaudRate => draft.baud_rate = text,
        ConnectionEditorField::PostLoginCommand => draft.post_login_command = text,
        ConnectionEditorField::PostLoginDelay => draft.post_login_delay_ms = text,
        ConnectionEditorField::SftpShellDetectionTimeout => {
            draft.sftp_shell_detection_timeout_ms = text
        }
        ConnectionEditorField::TelnetAutoLoginTimeout => {
            draft.telnet_auto_login_timeout_ms = text;
        }
        ConnectionEditorField::TelnetAutoLoginUsernamePrompt => {
            draft.telnet_auto_login_username_prompt_regex = text;
        }
        ConnectionEditorField::TelnetAutoLoginPasswordPrompt => {
            draft.telnet_auto_login_password_prompt_regex = text;
        }
        ConnectionEditorField::TelnetAutoLoginSuccessPrompt => {
            draft.telnet_auto_login_success_prompt_regex = text;
        }
        ConnectionEditorField::TelnetAutoLoginFailurePrompt => {
            draft.telnet_auto_login_failure_prompt_regex = text;
        }
        ConnectionEditorField::TelnetAutoLoginMaxRetries => {
            draft.telnet_auto_login_max_retries = text;
        }
        ConnectionEditorField::RdpDisplayWidth => {
            if let Ok(value) = text.parse() {
                draft.rdp_display.width = value;
            }
        }
        ConnectionEditorField::RdpDisplayHeight => {
            if let Ok(value) = text.parse() {
                draft.rdp_display.height = value;
            }
        }
        ConnectionEditorField::RdpReconnectAttempts => {
            if let Ok(value) = text.parse() {
                draft.rdp_reconnect.max_attempts = value;
            }
        }
        ConnectionEditorField::VncReconnectAttempts => {
            if let Ok(value) = text.parse() {
                draft.vnc_reconnect.max_attempts = value;
            }
        }
    }
}
