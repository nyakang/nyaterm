use gpui::Context;
use nyaterm_core::{
    AiExecutionProfile, ConnectionAuth, ConnectionNetwork, ConnectionPostLogin, ConnectionType,
    RdpClipboardSettings, RdpDisplaySettings, RdpReconnectSettings, RdpSecuritySettings,
    SavedConnection, SftpCwdFollowMode, SftpSettings, SshAgentEndpointValidationError,
    SshAgentForwardingConfig, SshAlgorithmMode, SshAlgorithmPreferences, TelnetAutoLoginConfig,
    VncClipboardSettings, VncDisplaySettings, VncReconnectSettings, VncSecuritySettings, uuid,
    validate_ssh_agent_endpoint, validate_ssh_agent_forwarding_config,
};
use nyaterm_store::{StoreDomain, store_request};

use crate::features::NyaTermApp;
use crate::models::{
    ConnectionEditorAdvancedTab, ConnectionEditorField, ConnectionEditorPasswordSource,
    ConnectionEditorRdpTab, ConnectionEditorSshAlgorithmTab, ConnectionEditorState,
    ConnectionEditorTelnetTab, ConnectionKindTab,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ConnectionEditorValidationError {
    HostRequired,
    PortInvalid,
    UsernameRequired,
    ShellPathRequired,
    SerialPortRequired,
    BaudRateInvalid,
    RdpDisplayWidthInvalid,
    RdpDisplayHeightInvalid,
    RdpReconnectAttemptsInvalid,
    VncReconnectAttemptsInvalid,
    PostLoginCommandRequired,
    PostLoginDelayInvalid,
    SftpShellDetectionTimeoutInvalid,
    SshAgentEndpoint(SshAgentEndpointValidationError),
    SshAlgorithms(nyaterm_transport::SshAlgorithmValidationError),
}

pub(super) fn connection_editor_from_saved(
    connection: SavedConnection,
    connect_after_save: bool,
) -> ConnectionEditorState {
    let auth = connection.auth.clone().unwrap_or_default();
    let network = connection.network.clone().unwrap_or(ConnectionNetwork {
        proxy_id: None,
        proxy_jump_id: None,
    });
    let post_login = connection
        .post_login
        .clone()
        .unwrap_or(ConnectionPostLogin {
            enabled: false,
            command: String::new(),
            delay_ms: 1000,
        });
    let sftp = connection.sftp.clone();
    let ssh_algorithms = connection.ssh_algorithms.clone().unwrap_or_default();
    let telnet_auto_login = match &connection.config {
        ConnectionType::Telnet { auto_login, .. } => auto_login.clone(),
        _ => TelnetAutoLoginConfig::default(),
    };
    let password_source = if auth.password_id.is_some() {
        ConnectionEditorPasswordSource::Saved
    } else if auth.password.is_some() || auth.has_password {
        ConnectionEditorPasswordSource::Direct
    } else {
        ConnectionEditorPasswordSource::Ask
    };
    let icon_auto_detect = connection.icon_auto_detect_enabled();
    let mut editor = ConnectionEditorState {
        id: Some(connection.id),
        kind: ConnectionKindTab::from_connection_type(&connection.config),
        name: connection.name,
        description: connection.description.unwrap_or_default(),
        icon: connection.icon,
        icon_auto_detect,
        group_id: connection.group_id,
        new_group_name: String::new(),
        pending_group_name: None,
        pending_group_parent_id: None,
        host: String::new(),
        port: String::new(),
        username: "root".to_string(),
        domain: String::new(),
        auth_mode: auth.mode,
        rdp_security: RdpSecuritySettings::default(),
        rdp_display: RdpDisplaySettings::default(),
        rdp_clipboard: RdpClipboardSettings::default(),
        rdp_reconnect: RdpReconnectSettings::default(),
        rdp_advanced_tab: ConnectionEditorRdpTab::Security,
        vnc_security: VncSecuritySettings::default(),
        vnc_display: VncDisplaySettings::default(),
        vnc_clipboard: VncClipboardSettings::default(),
        vnc_reconnect: VncReconnectSettings::default(),
        vnc_shared: true,
        vnc_view_only: false,
        password_source,
        password_id: auth.password_id,
        password: nyaterm_core::SecretString::default(),
        existing_password: auth.password.filter(|value| !value.is_empty()),
        key_id: auth.key_id,
        otp_id: auth.otp_id,
        auto_fill_otp: auth.auto_fill_otp,
        proxy_id: network.proxy_id,
        proxy_jump_id: network.proxy_jump_id,
        x11_forwarding: false,
        dynamic_tab_title: false,
        agent_endpoint: Default::default(),
        agent_forwarding_config: SshAgentForwardingConfig::default(),
        agent_allow_all_confirmed: false,
        agent_forwarding_endpoint_index: 0,
        agent_preview: None,
        agent_preview_loading: false,
        backspace_mode: "del".to_string(),
        encoding: "global".to_string(),
        ssh_profile: connection.ssh_profile,
        terminal_type: connection.terminal_type,
        sftp_enabled: sftp.enabled,
        sftp_cwd_follow_mode: sftp_cwd_follow_mode_value(sftp.cwd_follow_mode),
        sftp_shell_detection_timeout_ms: sftp.shell_detection_timeout_ms.to_string(),
        sftp_pipeline_depth: sftp.pipeline_depth,
        sftp_extra: sftp.extra.clone(),
        sftp_filename_encoding: if sftp.filename_encoding.is_empty() {
            "terminal".to_string()
        } else {
            sftp.filename_encoding
        },
        ssh_algorithm_mode: ssh_algorithm_mode_value(ssh_algorithms.mode),
        ssh_algorithm_kex: ssh_algorithms.kex,
        ssh_algorithm_ciphers: ssh_algorithms.ciphers,
        ssh_algorithm_macs: ssh_algorithms.macs,
        ssh_algorithm_host_keys: ssh_algorithms.host_keys,
        ssh_algorithm_tab: ConnectionEditorSshAlgorithmTab::KeyExchange,
        shell_path: String::new(),
        shell_args: String::new(),
        working_dir: String::new(),
        serial_port: String::new(),
        baud_rate: "115200".to_string(),
        data_bits: "8".to_string(),
        parity: "none".to_string(),
        stop_bits: "1".to_string(),
        raw_tcp_cli: false,
        telnet_enter_mode: "cr".to_string(),
        local_echo: false,
        local_line_edit: false,
        force_character_at_a_time: false,
        send_naws: true,
        send_sga: true,
        telnet_auto_login_enabled: telnet_auto_login.enabled,
        telnet_auto_login_send_wake_enter: telnet_auto_login.send_wake_enter,
        telnet_auto_login_timeout_ms: telnet_auto_login.timeout_ms.to_string(),
        telnet_auto_login_username_prompt_regex: telnet_auto_login
            .username_prompt_regex
            .unwrap_or_default(),
        telnet_auto_login_password_prompt_regex: telnet_auto_login
            .password_prompt_regex
            .unwrap_or_default(),
        telnet_auto_login_success_prompt_regex: telnet_auto_login
            .success_prompt_regex
            .unwrap_or_default(),
        telnet_auto_login_failure_prompt_regex: telnet_auto_login
            .failure_prompt_regex
            .unwrap_or_default(),
        telnet_auto_login_max_retries: telnet_auto_login.max_retries.to_string(),
        post_login_enabled: post_login.enabled,
        post_login_command: post_login.command,
        post_login_delay_ms: post_login.delay_ms.to_string(),
        recording: connection.recording.clone(),
        advanced_open: false,
        advanced_network_tab: ConnectionEditorAdvancedTab::Proxy,
        advanced_behavior_tab: ConnectionEditorAdvancedTab::PostLogin,
        telnet_advanced_tab: ConnectionEditorTelnetTab::Input,
        connect_after_save,
        focused_field: ConnectionEditorField::Name,
        error: None,
    };

    match connection.config {
        ConnectionType::Ssh {
            host,
            port,
            username,
            backspace_mode,
            x11_forwarding,
            dynamic_tab_title,
            auth_agent_endpoint,
            agent_forwarding_config,
            encoding,
            ..
        } => {
            editor.host = host;
            editor.port = port.to_string();
            editor.username = username;
            editor.backspace_mode = backspace_mode;
            editor.x11_forwarding = x11_forwarding;
            editor.dynamic_tab_title = dynamic_tab_title;
            // Preserve foreign-platform endpoints so editing unrelated fields
            // on another device does not erase the original Agent settings.
            editor.agent_endpoint = auth_agent_endpoint.unwrap_or_default();
            editor.agent_forwarding_config = agent_forwarding_config.unwrap_or_default();
            editor.agent_allow_all_confirmed = editor.agent_forwarding_config.enabled
                && matches!(
                    editor.agent_forwarding_config.policy,
                    nyaterm_core::SshAgentForwardingPolicy::All
                );
            editor.agent_forwarding_endpoint_index = 0;
            editor.encoding = encoding_to_editor_value(&encoding);
        }
        ConnectionType::LocalTerminal {
            shell_path,
            shell_args,
            working_dir,
            encoding,
            ..
        } => {
            editor.shell_path = shell_path;
            editor.shell_args = shell_args;
            editor.working_dir = working_dir.unwrap_or_default();
            editor.encoding = encoding_to_editor_value(&encoding);
        }
        ConnectionType::Telnet {
            host,
            port,
            username,
            raw_tcp_cli,
            enter_mode,
            local_echo,
            local_line_edit,
            force_character_at_a_time,
            send_naws,
            send_sga,
            backspace_mode,
            encoding,
            ..
        } => {
            editor.host = host;
            editor.port = port.to_string();
            editor.username = username;
            editor.raw_tcp_cli = raw_tcp_cli;
            editor.telnet_enter_mode = enter_mode;
            editor.local_echo = local_echo;
            editor.local_line_edit = local_line_edit;
            editor.force_character_at_a_time = force_character_at_a_time;
            editor.send_naws = send_naws;
            editor.send_sga = send_sga;
            editor.backspace_mode = backspace_mode;
            editor.encoding = encoding_to_editor_value(&encoding);
        }
        ConnectionType::Serial {
            port_name,
            baud_rate,
            data_bits,
            parity,
            stop_bits,
            backspace_mode,
            encoding,
            ..
        } => {
            editor.serial_port = port_name;
            editor.baud_rate = baud_rate.to_string();
            editor.data_bits = data_bits.to_string();
            editor.parity = parity;
            editor.stop_bits = stop_bits;
            editor.backspace_mode = backspace_mode;
            editor.encoding = encoding_to_editor_value(&encoding);
        }
        ConnectionType::Rdp {
            host,
            port,
            username,
            domain,
            security,
            display,
            clipboard,
            reconnect,
        } => {
            editor.host = host;
            editor.port = port.to_string();
            editor.username = username;
            editor.domain = domain;
            editor.rdp_security = security;
            editor.rdp_display = display;
            editor.rdp_clipboard = clipboard;
            editor.rdp_reconnect = reconnect;
        }
        ConnectionType::Vnc {
            host,
            port,
            security,
            display,
            clipboard,
            reconnect,
            shared,
            view_only,
        } => {
            editor.host = host;
            editor.port = port.to_string();
            editor.vnc_security = security;
            editor.vnc_display = display;
            editor.vnc_clipboard = clipboard;
            editor.vnc_reconnect = reconnect;
            editor.vnc_shared = shared;
            editor.vnc_view_only = view_only;
        }
    }
    editor
}

fn encoding_to_editor_value(value: &str) -> String {
    if value.trim().is_empty() {
        "global".to_string()
    } else {
        value.trim().to_string()
    }
}

fn editor_encoding_to_saved(value: &str) -> String {
    match value.trim() {
        "" | "global" => String::new(),
        value => value.to_string(),
    }
}

fn sftp_cwd_follow_mode_value(value: SftpCwdFollowMode) -> String {
    match value {
        SftpCwdFollowMode::Off => "off",
        SftpCwdFollowMode::ShellIntegration => "shell_integration",
        SftpCwdFollowMode::RcFile => "rc_file",
    }
    .to_string()
}

fn parse_sftp_cwd_follow_mode(value: &str) -> SftpCwdFollowMode {
    match value {
        "off" => SftpCwdFollowMode::Off,
        "rc_file" => SftpCwdFollowMode::RcFile,
        _ => SftpCwdFollowMode::ShellIntegration,
    }
}

fn ssh_algorithm_mode_value(value: SshAlgorithmMode) -> String {
    match value {
        SshAlgorithmMode::Compatible => "compatible",
        SshAlgorithmMode::Secure => "secure",
        SshAlgorithmMode::Custom => "custom",
    }
    .to_string()
}

fn parse_ssh_algorithm_mode(value: &str) -> SshAlgorithmMode {
    match value {
        "secure" => SshAlgorithmMode::Secure,
        "custom" => SshAlgorithmMode::Custom,
        _ => SshAlgorithmMode::Compatible,
    }
}

pub(super) fn build_saved_connection_from_editor(
    editor: &ConnectionEditorState,
) -> Result<SavedConnection, ConnectionEditorValidationError> {
    let config = match editor.kind {
        ConnectionKindTab::Ssh => {
            let host = editor.host.trim().to_string();
            if host.is_empty() {
                return Err(ConnectionEditorValidationError::HostRequired);
            }
            let port = parse_port(&editor.port)?;
            let username = editor.username.trim().to_string();
            if username.is_empty() {
                return Err(ConnectionEditorValidationError::UsernameRequired);
            }
            ConnectionType::Ssh {
                host,
                port,
                username,
                backspace_mode: nyaterm_core::terminal::connection_input::BackspaceMode::parse(
                    &editor.backspace_mode,
                )
                .persistence_id()
                .to_string(),
                ai_execution_profile: AiExecutionProfile::Auto,
                x11_forwarding: editor.x11_forwarding,
                auth_agent_endpoint: (editor.auth_mode == "agent")
                    .then(|| editor.agent_endpoint.clone()),
                agent_forwarding_config: (editor.agent_forwarding_config
                    != nyaterm_core::SshAgentForwardingConfig::default())
                .then(|| editor.agent_forwarding_config.clone()),
                legacy_agent_forwarding: None,
                encoding: editor_encoding_to_saved(&editor.encoding),
                dynamic_tab_title: editor.dynamic_tab_title,
            }
        }
        ConnectionKindTab::Local => {
            let shell_path = editor.shell_path.trim().to_string();
            if shell_path.is_empty() {
                return Err(ConnectionEditorValidationError::ShellPathRequired);
            }
            ConnectionType::LocalTerminal {
                shell_path,
                shell_args: editor.shell_args.trim().to_string(),
                working_dir: non_empty_optional(&editor.working_dir),
                ai_execution_profile: AiExecutionProfile::Posix,
                encoding: editor_encoding_to_saved(&editor.encoding),
                dynamic_tab_title: editor.dynamic_tab_title,
            }
        }
        ConnectionKindTab::Telnet => {
            let host = editor.host.trim().to_string();
            if host.is_empty() {
                return Err(ConnectionEditorValidationError::HostRequired);
            }
            let port = parse_port(&editor.port)?;
            ConnectionType::Telnet {
                host,
                port,
                username: editor.username.trim().to_string(),
                ai_execution_profile: AiExecutionProfile::Auto,
                backspace_mode: nyaterm_core::terminal::connection_input::BackspaceMode::parse(
                    &editor.backspace_mode,
                )
                .persistence_id()
                .to_string(),
                raw_tcp_cli: editor.raw_tcp_cli,
                enter_mode: non_empty_or(editor.telnet_enter_mode.clone(), "cr"),
                local_echo: editor.local_echo,
                local_line_edit: editor.local_line_edit,
                force_character_at_a_time: editor.force_character_at_a_time,
                send_naws: editor.send_naws,
                send_sga: editor.send_sga,
                auto_login: TelnetAutoLoginConfig {
                    enabled: editor.telnet_auto_login_enabled,
                    send_wake_enter: editor.telnet_auto_login_send_wake_enter,
                    timeout_ms: editor
                        .telnet_auto_login_timeout_ms
                        .trim()
                        .parse::<u64>()
                        .unwrap_or(60_000)
                        .clamp(100, 600_000),
                    username_prompt_regex: non_empty_optional(
                        &editor.telnet_auto_login_username_prompt_regex,
                    ),
                    password_prompt_regex: non_empty_optional(
                        &editor.telnet_auto_login_password_prompt_regex,
                    ),
                    success_prompt_regex: non_empty_optional(
                        &editor.telnet_auto_login_success_prompt_regex,
                    ),
                    failure_prompt_regex: non_empty_optional(
                        &editor.telnet_auto_login_failure_prompt_regex,
                    ),
                    max_retries: editor
                        .telnet_auto_login_max_retries
                        .trim()
                        .parse::<u8>()
                        .unwrap_or(0),
                },
                encoding: editor_encoding_to_saved(&editor.encoding),
            }
        }
        ConnectionKindTab::Serial => {
            let port_name = editor.serial_port.trim().to_string();
            if port_name.is_empty() {
                return Err(ConnectionEditorValidationError::SerialPortRequired);
            }
            let baud_rate = editor
                .baud_rate
                .trim()
                .parse::<u32>()
                .map_err(|_| ConnectionEditorValidationError::BaudRateInvalid)?;
            if !(1..=4_000_000).contains(&baud_rate) {
                return Err(ConnectionEditorValidationError::BaudRateInvalid);
            }
            let data_bits = editor
                .data_bits
                .trim()
                .parse::<u8>()
                .unwrap_or(8)
                .clamp(5, 8);
            ConnectionType::Serial {
                port_name,
                baud_rate,
                data_bits,
                parity: non_empty_or(editor.parity.clone(), "none"),
                stop_bits: non_empty_or(editor.stop_bits.clone(), "1"),
                ai_execution_profile: AiExecutionProfile::Auto,
                backspace_mode: nyaterm_core::terminal::connection_input::BackspaceMode::parse(
                    &editor.backspace_mode,
                )
                .persistence_id()
                .to_string(),
                encoding: editor_encoding_to_saved(&editor.encoding),
            }
        }
        ConnectionKindTab::Rdp => {
            let host = editor.host.trim().to_string();
            if host.is_empty() {
                return Err(ConnectionEditorValidationError::HostRequired);
            }
            let port = parse_port(&editor.port)?;
            if editor.username.trim().is_empty() {
                return Err(ConnectionEditorValidationError::UsernameRequired);
            }
            if !(640..=7680).contains(&editor.rdp_display.width) {
                return Err(ConnectionEditorValidationError::RdpDisplayWidthInvalid);
            }
            if !(480..=4320).contains(&editor.rdp_display.height) {
                return Err(ConnectionEditorValidationError::RdpDisplayHeightInvalid);
            }
            if editor.rdp_reconnect.max_attempts > 20 {
                return Err(ConnectionEditorValidationError::RdpReconnectAttemptsInvalid);
            }
            ConnectionType::Rdp {
                host,
                port,
                username: editor.username.trim().to_string(),
                domain: editor.domain.trim().to_string(),
                security: editor.rdp_security.clone(),
                display: editor.rdp_display.clone(),
                clipboard: editor.rdp_clipboard.clone(),
                reconnect: editor.rdp_reconnect.clone(),
            }
        }
        ConnectionKindTab::Vnc => {
            let host = editor.host.trim().to_string();
            if host.is_empty() {
                return Err(ConnectionEditorValidationError::HostRequired);
            }
            let port = parse_port(&editor.port)?;
            if editor.vnc_reconnect.max_attempts > 20 {
                return Err(ConnectionEditorValidationError::VncReconnectAttemptsInvalid);
            }
            ConnectionType::Vnc {
                host,
                port,
                security: editor.vnc_security.clone(),
                display: editor.vnc_display.clone(),
                clipboard: editor.vnc_clipboard.clone(),
                reconnect: editor.vnc_reconnect.clone(),
                shared: editor.vnc_shared,
                view_only: editor.vnc_view_only,
            }
        }
    };

    if editor.kind == ConnectionKindTab::Ssh {
        // Endpoint drafts are used only by Agent authentication or forwarding.
        // Hidden values left after changing auth mode must not block saving.
        if editor.auth_mode == "agent" || editor.agent_forwarding_config.enabled {
            validate_ssh_agent_endpoint(&editor.agent_endpoint)
                .map_err(ConnectionEditorValidationError::SshAgentEndpoint)?;
        }
        validate_ssh_agent_forwarding_config(&editor.agent_forwarding_config)
            .map_err(ConnectionEditorValidationError::SshAgentEndpoint)?;
        if editor.post_login_enabled && editor.post_login_command.trim().is_empty() {
            return Err(ConnectionEditorValidationError::PostLoginCommandRequired);
        }
        let delay = editor
            .post_login_delay_ms
            .trim()
            .parse::<u64>()
            .map_err(|_| ConnectionEditorValidationError::PostLoginDelayInvalid)?;
        if delay > 60_000 {
            return Err(ConnectionEditorValidationError::PostLoginDelayInvalid);
        }
        let sftp_timeout = editor
            .sftp_shell_detection_timeout_ms
            .trim()
            .parse::<u64>()
            .map_err(|_| ConnectionEditorValidationError::SftpShellDetectionTimeoutInvalid)?;
        if !(100..=60_000).contains(&sftp_timeout) {
            return Err(ConnectionEditorValidationError::SftpShellDetectionTimeoutInvalid);
        }
    }

    let name = if editor.name.trim().is_empty() {
        match &config {
            ConnectionType::Ssh { host, port, .. }
            | ConnectionType::Telnet { host, port, .. }
            | ConnectionType::Rdp { host, port, .. }
            | ConnectionType::Vnc { host, port, .. } => {
                format!("{host}:{port}")
            }
            ConnectionType::LocalTerminal { .. } => "Local Terminal".to_string(),
            ConnectionType::Serial { port_name, .. } => port_name.clone(),
        }
    } else {
        editor.name.trim().to_string()
    };

    let auth = match editor.kind {
        ConnectionKindTab::Ssh
        | ConnectionKindTab::Telnet
        | ConnectionKindTab::Rdp
        | ConnectionKindTab::Vnc => {
            let password = editor.password.trim().to_string();
            let existing = editor.existing_password.clone();
            let mode = match editor.auth_mode.as_str() {
                "password" => "password".to_string(),
                "agent" if editor.kind == ConnectionKindTab::Ssh => "agent".to_string(),
                "key"
                    if editor.kind == ConnectionKindTab::Ssh
                        && editor
                            .key_id
                            .as_deref()
                            .is_some_and(|value| !value.trim().is_empty()) =>
                {
                    "key".to_string()
                }
                _ => "none".to_string(),
            };
            Some(ConnectionAuth {
                password_id: (mode == "password"
                    && editor.password_source == ConnectionEditorPasswordSource::Saved)
                    .then(|| editor.password_id.clone())
                    .flatten(),
                password: if mode == "password"
                    && editor.password_source == ConnectionEditorPasswordSource::Direct
                {
                    if !password.is_empty() {
                        Some(password.into())
                    } else {
                        existing
                    }
                } else {
                    None
                },
                key_id: (mode == "key")
                    .then(|| {
                        editor
                            .key_id
                            .clone()
                            .filter(|value| !value.trim().is_empty())
                    })
                    .flatten(),
                otp_id: (editor.kind == ConnectionKindTab::Ssh)
                    .then(|| {
                        editor
                            .otp_id
                            .clone()
                            .filter(|value| !value.trim().is_empty())
                    })
                    .flatten(),
                auto_fill_otp: editor.kind == ConnectionKindTab::Ssh && editor.auto_fill_otp,
                has_password: false,
                mode,
            })
        }
        _ => None,
    };

    let network = match editor.kind {
        ConnectionKindTab::Ssh | ConnectionKindTab::Rdp | ConnectionKindTab::Vnc => {
            let proxy_id = editor
                .proxy_id
                .clone()
                .filter(|value| !value.trim().is_empty());
            let proxy_jump_id = editor
                .proxy_jump_id
                .clone()
                .filter(|value| !value.trim().is_empty());
            if proxy_id.is_some() || proxy_jump_id.is_some() {
                Some(ConnectionNetwork {
                    proxy_id,
                    proxy_jump_id,
                })
            } else {
                None
            }
        }
        _ => None,
    };

    let post_login = if editor.kind == ConnectionKindTab::Ssh
        && (editor.post_login_enabled || !editor.post_login_command.trim().is_empty())
    {
        let delay_ms = editor
            .post_login_delay_ms
            .trim()
            .parse::<u64>()
            .unwrap_or(1000)
            .min(60_000);
        Some(ConnectionPostLogin {
            enabled: editor.post_login_enabled,
            command: editor.post_login_command.clone(),
            delay_ms,
        })
    } else {
        None
    };
    let ssh_algorithms = if editor.kind == ConnectionKindTab::Ssh {
        let preferences = SshAlgorithmPreferences {
            mode: parse_ssh_algorithm_mode(&editor.ssh_algorithm_mode),
            kex: editor.ssh_algorithm_kex.clone(),
            ciphers: editor.ssh_algorithm_ciphers.clone(),
            macs: editor.ssh_algorithm_macs.clone(),
            host_keys: editor.ssh_algorithm_host_keys.clone(),
        };
        let preferences =
            (preferences != SshAlgorithmPreferences::default()).then_some(preferences);
        let transport_preferences =
            preferences
                .as_ref()
                .map(|preferences| nyaterm_transport::SshAlgorithmPreferences {
                    mode: match preferences.mode {
                        SshAlgorithmMode::Compatible => {
                            nyaterm_transport::SshAlgorithmMode::Compatible
                        }
                        SshAlgorithmMode::Secure => nyaterm_transport::SshAlgorithmMode::Secure,
                        SshAlgorithmMode::Custom => nyaterm_transport::SshAlgorithmMode::Custom,
                    },
                    kex: preferences.kex.clone(),
                    ciphers: preferences.ciphers.clone(),
                    macs: preferences.macs.clone(),
                    host_keys: preferences.host_keys.clone(),
                });
        nyaterm_transport::validate_ssh_algorithm_preferences(transport_preferences.as_ref())
            .map_err(ConnectionEditorValidationError::SshAlgorithms)?;
        preferences
    } else {
        None
    };
    let sftp = if editor.kind == ConnectionKindTab::Ssh {
        SftpSettings {
            pipeline_depth: editor.sftp_pipeline_depth,
            extra: editor.sftp_extra.clone(),
            enabled: editor.sftp_enabled,
            cwd_follow_mode: parse_sftp_cwd_follow_mode(&editor.sftp_cwd_follow_mode),
            shell_detection_timeout_ms: editor
                .sftp_shell_detection_timeout_ms
                .trim()
                .parse::<u64>()
                .unwrap_or(3000)
                .clamp(100, 60_000),
            filename_encoding: match editor.sftp_filename_encoding.trim() {
                "" | "terminal" | "global" => String::new(),
                value => value.to_string(),
            },
        }
    } else {
        SftpSettings::default()
    };

    Ok(SavedConnection {
        extensions: Default::default(),
        id: editor.id.clone().unwrap_or_else(uuid),
        name,
        config,
        group_id: editor
            .group_id
            .clone()
            .filter(|value| !value.trim().is_empty()),
        description: non_empty_optional(&editor.description),
        sort_order: 0,
        icon: editor.icon.clone().filter(|value| !value.trim().is_empty()),
        // Only SSH sessions report a remote system, so nothing else can be
        // auto-detected; persist the flag explicitly rather than relying on the
        // "unset means enabled while blank" default, which would flip once the
        // user chose an icon.
        icon_auto_detect: Some(editor.kind == ConnectionKindTab::Ssh && editor.icon_auto_detect),
        auth,
        network,
        post_login,
        // RDP does not expose recording controls, but an existing hidden value
        // remains part of the compatibility-sensitive saved connection.
        recording: editor.recording.clone(),
        ssh_algorithms,
        ssh_profile: if editor.kind == ConnectionKindTab::Ssh {
            editor.ssh_profile
        } else {
            Default::default()
        },
        terminal_type: (editor.kind == ConnectionKindTab::Ssh)
            .then_some(editor.terminal_type)
            .flatten(),
        sftp,
        // The connection editor does not surface asset facts; monitoring
        // populates them separately via the atomic merge storage API, so a
        // freshly built connection starts with no asset metadata.
        asset: None,
        created_at_ms: None,
        updated_at_ms: None,
        last_used_at_ms: None,
    })
}

pub(super) fn parse_port(value: &str) -> Result<u16, ConnectionEditorValidationError> {
    let port = value
        .trim()
        .parse::<u16>()
        .map_err(|_| ConnectionEditorValidationError::PortInvalid)?;
    if port == 0 {
        return Err(ConnectionEditorValidationError::PortInvalid);
    }
    Ok(port)
}

pub(super) fn non_empty_or(value: String, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use nyaterm_core::{
        AiExecutionProfile, ConnectionAuth, ConnectionRecordingSettings, ConnectionType,
        RdpClipboardSettings, RdpDisplaySettings, RdpReconnectSettings, RdpSecuritySettings,
        RecordingMode, RecordingRotationPolicy, SavedConnection, SftpCwdFollowMode, SftpSettings,
        SshAgentEndpoint, SshAlgorithmMode, SshAlgorithmPreferences, SshProfile, SshTerminalType,
        VncClipboardSettings, VncDisplaySettings, VncReconnectSettings, VncSecuritySettings,
    };

    use crate::models::{ConnectionEditorPasswordSource, ConnectionKindTab};

    use super::{
        ConnectionEditorValidationError, build_saved_connection_from_editor,
        connection_editor_from_saved,
    };

    #[test]
    fn connection_editor_round_trip_preserves_icon() {
        let connection = SavedConnection {
            extensions: Default::default(),
            id: "connection-1".to_string(),
            name: "Local".to_string(),
            config: ConnectionType::LocalTerminal {
                shell_path: "bash".to_string(),
                shell_args: String::new(),
                working_dir: None,
                ai_execution_profile: AiExecutionProfile::Posix,
                encoding: String::new(),
                dynamic_tab_title: false,
            },
            group_id: None,
            description: None,
            sort_order: 0,
            icon: Some("linux".to_string()),
            icon_auto_detect: None,
            auth: None,
            recording: None,
            ssh_algorithms: None,
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp: Default::default(),
            network: None,
            post_login: None,
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        };

        let editor = connection_editor_from_saved(connection, false);
        assert_eq!(editor.icon.as_deref(), Some("linux"));
        let saved = build_saved_connection_from_editor(&editor).expect("valid connection");
        assert_eq!(saved.icon.as_deref(), Some("linux"));
    }

    #[test]
    fn connection_editor_round_trip_preserves_saved_password_reference() {
        let connection = SavedConnection {
            extensions: Default::default(),
            id: "connection-ssh".to_string(),
            name: "SSH".to_string(),
            config: ConnectionType::Ssh {
                host: "example.com".to_string(),
                port: 22,
                username: "root".to_string(),
                backspace_mode: "del".to_string(),
                ai_execution_profile: AiExecutionProfile::Auto,
                x11_forwarding: false,
                auth_agent_endpoint: None,
                agent_forwarding_config: None,
                legacy_agent_forwarding: None,
                encoding: String::new(),
                dynamic_tab_title: false,
            },
            group_id: None,
            description: None,
            sort_order: 0,
            icon: None,
            icon_auto_detect: None,
            auth: Some(ConnectionAuth {
                mode: "password".to_string(),
                password_id: Some("password-1".to_string()),
                ..ConnectionAuth::default()
            }),
            recording: None,
            ssh_algorithms: None,
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp: Default::default(),
            network: None,
            post_login: None,
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        };

        let editor = connection_editor_from_saved(connection, false);
        assert_eq!(
            editor.password_source,
            ConnectionEditorPasswordSource::Saved
        );
        assert_eq!(editor.password_id.as_deref(), Some("password-1"));

        let saved = build_saved_connection_from_editor(&editor).expect("valid connection");
        assert_eq!(
            saved.auth.and_then(|auth| auth.password_id).as_deref(),
            Some("password-1")
        );
    }

    #[test]
    fn connection_editor_round_trip_preserves_ssh_agent_authentication() {
        let expected_endpoint = if cfg!(unix) {
            SshAgentEndpoint::WindowsOpenSsh
        } else {
            SshAgentEndpoint::Auto
        };
        let connection = SavedConnection {
            extensions: Default::default(),
            id: "connection-agent".to_string(),
            name: "Agent SSH".to_string(),
            config: ConnectionType::Ssh {
                host: "example.com".to_string(),
                port: 22,
                username: "root".to_string(),
                backspace_mode: "del".to_string(),
                ai_execution_profile: AiExecutionProfile::Auto,
                x11_forwarding: false,
                auth_agent_endpoint: Some(expected_endpoint.clone()),
                agent_forwarding_config: Some(nyaterm_core::SshAgentForwardingConfig {
                    enabled: true,
                    sources: nyaterm_core::SshAgentForwardingSources {
                        external_agent: true,
                        external_agent_endpoints: vec![expected_endpoint.clone()],
                        stored_keys: false,
                    },
                    policy: nyaterm_core::SshAgentForwardingPolicy::All,
                }),
                legacy_agent_forwarding: None,
                encoding: String::new(),
                dynamic_tab_title: false,
            },
            group_id: None,
            description: None,
            sort_order: 0,
            icon: None,
            icon_auto_detect: None,
            auth: Some(ConnectionAuth {
                mode: "agent".to_string(),
                ..ConnectionAuth::default()
            }),
            recording: None,
            ssh_algorithms: None,
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp: Default::default(),
            network: None,
            post_login: None,
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        };

        let editor = connection_editor_from_saved(connection, false);
        assert_eq!(editor.auth_mode, "agent");
        assert!(editor.agent_forwarding_config.enabled);
        assert_eq!(editor.agent_endpoint, expected_endpoint);
        let saved = build_saved_connection_from_editor(&editor).expect("valid Agent connection");
        assert_eq!(saved.auth.as_ref().expect("auth settings").mode, "agent");
        assert!(matches!(
            saved.config,
            ConnectionType::Ssh {
                auth_agent_endpoint: Some(agent_endpoint),
                ..
            } if agent_endpoint == expected_endpoint
        ));
    }

    #[test]
    fn connection_editor_rejects_invalid_ssh_agent_endpoint_before_save() {
        let connection = SavedConnection {
            extensions: Default::default(),
            id: "connection-agent-invalid".to_string(),
            name: "Agent SSH".to_string(),
            config: ConnectionType::Ssh {
                host: "example.com".to_string(),
                port: 22,
                username: "root".to_string(),
                backspace_mode: "del".to_string(),
                ai_execution_profile: AiExecutionProfile::Auto,
                x11_forwarding: false,
                auth_agent_endpoint: Some(SshAgentEndpoint::Auto),
                agent_forwarding_config: None,
                legacy_agent_forwarding: None,
                encoding: String::new(),
                dynamic_tab_title: false,
            },
            group_id: None,
            description: None,
            sort_order: 0,
            icon: None,
            icon_auto_detect: None,
            auth: Some(ConnectionAuth {
                mode: "agent".to_string(),
                ..ConnectionAuth::default()
            }),
            recording: None,
            ssh_algorithms: None,
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp: Default::default(),
            network: None,
            post_login: None,
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        };
        let mut editor = connection_editor_from_saved(connection, false);
        editor.agent_endpoint = SshAgentEndpoint::Environment {
            variable: "SSH=AUTH_SOCK".to_string(),
        };
        assert!(matches!(
            build_saved_connection_from_editor(&editor),
            Err(ConnectionEditorValidationError::SshAgentEndpoint(_))
        ));

        // A hidden endpoint draft must not block password authentication.
        editor.auth_mode = "password".to_string();
        assert!(build_saved_connection_from_editor(&editor).is_ok());
    }

    #[test]
    fn connection_editor_round_trip_preserves_ssh_encoding_sftp_and_algorithms() {
        let connection = SavedConnection {
            extensions: Default::default(),
            id: "connection-ssh".to_string(),
            name: "SSH".to_string(),
            config: ConnectionType::Ssh {
                host: "example.com".to_string(),
                port: 22,
                username: "root".to_string(),
                backspace_mode: "del".to_string(),
                ai_execution_profile: AiExecutionProfile::Auto,
                x11_forwarding: false,
                auth_agent_endpoint: None,
                agent_forwarding_config: None,
                legacy_agent_forwarding: None,
                encoding: "GBK".to_string(),
                dynamic_tab_title: false,
            },
            group_id: None,
            description: None,
            sort_order: 0,
            icon: None,
            icon_auto_detect: None,
            auth: None,
            recording: None,
            ssh_algorithms: Some(SshAlgorithmPreferences {
                mode: SshAlgorithmMode::Custom,
                kex: vec!["curve25519-sha256".to_string()],
                ciphers: vec!["aes128-ctr".to_string()],
                macs: vec!["hmac-sha2-256".to_string()],
                host_keys: vec!["ssh-ed25519".to_string()],
            }),
            ssh_profile: SshProfile::NetworkDevice,
            terminal_type: Some(SshTerminalType::Vt220),
            sftp: SftpSettings {
                pipeline_depth: None,
                extra: Default::default(),
                enabled: false,
                cwd_follow_mode: SftpCwdFollowMode::RcFile,
                shell_detection_timeout_ms: 5000,
                filename_encoding: "GB18030".to_string(),
            },
            network: None,
            post_login: None,
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        };

        let editor = connection_editor_from_saved(connection.clone(), false);
        let saved = build_saved_connection_from_editor(&editor).expect("valid connection");

        assert_eq!(saved.ssh_algorithms, connection.ssh_algorithms);
        assert_eq!(saved.sftp, connection.sftp);
        assert_eq!(saved.ssh_profile, SshProfile::NetworkDevice);
        assert_eq!(saved.terminal_type, Some(SshTerminalType::Vt220));
        let mut invalid = editor.clone();
        invalid.ssh_algorithm_kex = vec!["not-a-kex".to_string()];
        assert_eq!(
            build_saved_connection_from_editor(&invalid).unwrap_err(),
            ConnectionEditorValidationError::SshAlgorithms(
                nyaterm_transport::SshAlgorithmValidationError::Unsupported {
                    kind: nyaterm_transport::SshAlgorithmListKind::KeyExchange,
                    algorithm: "not-a-kex".to_string(),
                }
            )
        );
        match saved.config {
            ConnectionType::Ssh { encoding, .. } => assert_eq!(encoding, "GBK"),
            other => panic!("expected SSH connection, got {other:?}"),
        }

        let mut compatible = editor.clone();
        compatible.ssh_algorithm_mode = "compatible".to_string();
        compatible.ssh_algorithm_kex.clear();
        compatible.ssh_algorithm_ciphers.clear();
        compatible.ssh_algorithm_macs.clear();
        compatible.ssh_algorithm_host_keys.clear();
        assert_eq!(
            build_saved_connection_from_editor(&compatible)
                .expect("compatible connection")
                .ssh_algorithms,
            None
        );
    }

    #[test]
    fn connection_editor_round_trip_preserves_telnet_behavior() {
        let connection = SavedConnection {
            extensions: Default::default(),
            id: "connection-telnet".to_string(),
            name: "Telnet".to_string(),
            config: ConnectionType::Telnet {
                host: "device.local".to_string(),
                port: 2323,
                username: String::new(),
                ai_execution_profile: AiExecutionProfile::Auto,
                backspace_mode: "ctrl_h".to_string(),
                raw_tcp_cli: true,
                enter_mode: "lf".to_string(),
                local_echo: true,
                local_line_edit: true,
                force_character_at_a_time: true,
                send_naws: false,
                send_sga: false,
                auto_login: Default::default(),
                encoding: String::new(),
            },
            group_id: None,
            description: None,
            sort_order: 0,
            icon: None,
            icon_auto_detect: None,
            auth: None,
            recording: None,
            ssh_algorithms: None,
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp: Default::default(),
            network: None,
            post_login: None,
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        };

        let editor = connection_editor_from_saved(connection, false);
        let saved = build_saved_connection_from_editor(&editor).expect("valid connection");
        let ConnectionType::Telnet {
            enter_mode,
            local_line_edit,
            force_character_at_a_time,
            send_naws,
            send_sga,
            ..
        } = saved.config
        else {
            panic!("expected telnet connection");
        };
        assert_eq!(enter_mode, "lf");
        assert!(local_line_edit);
        assert!(force_character_at_a_time);
        assert!(!send_naws);
        assert!(!send_sga);
    }

    #[test]
    fn connection_editor_round_trip_preserves_rdp_settings_and_auth_mode() {
        let expected_security = RdpSecuritySettings {
            use_nla: false,
            certificate_policy: "strict".to_string(),
        };
        let expected_display = RdpDisplaySettings {
            mode: "fixed".to_string(),
            width: 2560,
            height: 1440,
            color_depth: 24,
        };
        let expected_clipboard = RdpClipboardSettings {
            mode: "disabled".to_string(),
        };
        let expected_reconnect = RdpReconnectSettings {
            enabled: false,
            max_attempts: 17,
        };
        let expected_recording = ConnectionRecordingSettings {
            auto_start: Some(true),
            mode: Some(RecordingMode::Raw),
            path_template: Some("rdp/{session}.bin".to_string()),
            include_timestamps: Some(false),
            rotation: Some(RecordingRotationPolicy::Size { max_bytes: 4096 }),
        };
        let connection = SavedConnection {
            extensions: Default::default(),
            id: "connection-rdp".to_string(),
            name: "RDP".to_string(),
            config: ConnectionType::Rdp {
                host: "desktop.example.com".to_string(),
                port: 3390,
                username: "operator".to_string(),
                domain: "ACME".to_string(),
                security: expected_security.clone(),
                display: expected_display.clone(),
                clipboard: expected_clipboard.clone(),
                reconnect: expected_reconnect.clone(),
            },
            group_id: None,
            description: None,
            sort_order: 0,
            icon: None,
            icon_auto_detect: None,
            auth: Some(ConnectionAuth {
                mode: "none".to_string(),
                ..ConnectionAuth::default()
            }),
            recording: Some(expected_recording.clone()),
            ssh_algorithms: None,
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp: Default::default(),
            network: None,
            post_login: None,
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        };

        let editor = connection_editor_from_saved(connection, false);
        assert_eq!(editor.kind, ConnectionKindTab::Rdp);
        assert_eq!(editor.domain, "ACME");
        assert_eq!(editor.auth_mode, "none");

        let mut invalid = editor.clone();
        invalid.username.clear();
        assert_eq!(
            build_saved_connection_from_editor(&invalid).unwrap_err(),
            ConnectionEditorValidationError::UsernameRequired
        );
        invalid.username = "operator".to_string();
        invalid.rdp_display.width = 639;
        assert_eq!(
            build_saved_connection_from_editor(&invalid).unwrap_err(),
            ConnectionEditorValidationError::RdpDisplayWidthInvalid
        );
        invalid.rdp_display.width = 2560;
        invalid.rdp_display.height = 4321;
        assert_eq!(
            build_saved_connection_from_editor(&invalid).unwrap_err(),
            ConnectionEditorValidationError::RdpDisplayHeightInvalid
        );
        invalid.rdp_display.height = 1440;
        invalid.rdp_reconnect.max_attempts = 21;
        assert_eq!(
            build_saved_connection_from_editor(&invalid).unwrap_err(),
            ConnectionEditorValidationError::RdpReconnectAttemptsInvalid
        );

        let saved = build_saved_connection_from_editor(&editor).expect("valid connection");
        let ConnectionType::Rdp {
            host,
            port,
            username,
            domain,
            security,
            display,
            clipboard,
            reconnect,
        } = saved.config
        else {
            panic!("expected RDP connection");
        };

        assert_eq!(host, "desktop.example.com");
        assert_eq!(port, 3390);
        assert_eq!(username, "operator");
        assert_eq!(domain, "ACME");
        assert_eq!(security, expected_security);
        assert_eq!(display, expected_display);
        assert_eq!(clipboard, expected_clipboard);
        assert_eq!(reconnect, expected_reconnect);
        assert_eq!(saved.auth.expect("auth settings").mode, "none");
        assert_eq!(saved.recording, Some(expected_recording));
    }

    #[test]
    fn connection_editor_round_trip_preserves_vnc_settings_and_auth_mode() {
        let expected_security = VncSecuritySettings {
            mode: "vnc-auth".to_string(),
        };
        let expected_display = VncDisplaySettings {
            scale_mode: "actual".to_string(),
        };
        let expected_clipboard = VncClipboardSettings { enabled: false };
        let expected_reconnect = VncReconnectSettings {
            enabled: false,
            max_attempts: 12,
        };
        let connection = SavedConnection {
            extensions: Default::default(),
            id: "connection-vnc".to_string(),
            name: "VNC".to_string(),
            config: ConnectionType::Vnc {
                host: "desktop.example.com".to_string(),
                port: 5901,
                security: expected_security.clone(),
                display: expected_display.clone(),
                clipboard: expected_clipboard.clone(),
                reconnect: expected_reconnect.clone(),
                shared: false,
                view_only: true,
            },
            group_id: None,
            description: None,
            sort_order: 0,
            icon: None,
            icon_auto_detect: None,
            auth: Some(ConnectionAuth {
                mode: "password".to_string(),
                has_password: true,
                ..ConnectionAuth::default()
            }),
            recording: None,
            ssh_algorithms: None,
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp: Default::default(),
            network: None,
            post_login: None,
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        };

        let editor = connection_editor_from_saved(connection, false);
        assert_eq!(editor.kind, ConnectionKindTab::Vnc);
        assert_eq!(editor.auth_mode, "password");
        assert_eq!(
            editor.password_source,
            ConnectionEditorPasswordSource::Direct
        );

        let mut invalid = editor.clone();
        invalid.vnc_reconnect.max_attempts = 21;
        assert_eq!(
            build_saved_connection_from_editor(&invalid).unwrap_err(),
            ConnectionEditorValidationError::VncReconnectAttemptsInvalid
        );

        let saved = build_saved_connection_from_editor(&editor).expect("valid connection");
        let ConnectionType::Vnc {
            host,
            port,
            security,
            display,
            clipboard,
            reconnect,
            shared,
            view_only,
        } = saved.config
        else {
            panic!("expected VNC connection");
        };

        assert_eq!(host, "desktop.example.com");
        assert_eq!(port, 5901);
        assert_eq!(security, expected_security);
        assert_eq!(display, expected_display);
        assert_eq!(clipboard, expected_clipboard);
        assert_eq!(reconnect, expected_reconnect);
        assert!(!shared);
        assert!(view_only);
        let saved_auth = saved.auth.expect("auth settings");
        assert_eq!(saved_auth.mode, "password");
        assert!(!saved_auth.has_password);
        assert!(saved_auth.password.is_none());
    }

    #[test]
    fn connection_editor_saves_telnet_username_password_and_encoding() {
        let connection = SavedConnection {
            extensions: Default::default(),
            id: "connection-telnet".to_string(),
            name: "Telnet".to_string(),
            config: ConnectionType::Telnet {
                host: "device.local".to_string(),
                port: 23,
                username: "operator".to_string(),
                ai_execution_profile: AiExecutionProfile::Auto,
                backspace_mode: "del".to_string(),
                raw_tcp_cli: false,
                enter_mode: "cr".to_string(),
                local_echo: true,
                local_line_edit: true,
                force_character_at_a_time: false,
                send_naws: true,
                send_sga: true,
                auto_login: Default::default(),
                encoding: "GB18030".to_string(),
            },
            group_id: None,
            description: None,
            sort_order: 0,
            icon: None,
            icon_auto_detect: None,
            auth: Some(ConnectionAuth {
                mode: "password".to_string(),
                password: Some("secret".to_string().into()),
                ..ConnectionAuth::default()
            }),
            recording: None,
            ssh_algorithms: None,
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp: Default::default(),
            network: None,
            post_login: None,
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        };

        let editor = connection_editor_from_saved(connection, false);
        let saved = build_saved_connection_from_editor(&editor).expect("valid connection");

        match saved.config {
            ConnectionType::Telnet {
                username,
                encoding,
                local_echo,
                local_line_edit,
                ..
            } => {
                assert_eq!(username, "operator");
                assert_eq!(encoding, "GB18030");
                assert!(local_echo);
                assert!(local_line_edit);
            }
            other => panic!("expected Telnet connection, got {other:?}"),
        }
        assert_eq!(
            saved
                .auth
                .as_ref()
                .and_then(|auth| auth.password.as_deref()),
            Some("secret")
        );
    }
}

pub(super) fn non_empty_optional(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

impl NyaTermApp {
    pub(in crate::features) fn set_connection_editor_error(
        &mut self,
        error: String,
        cx: &mut Context<Self>,
    ) {
        self.connection_state.set_editor_error(error.clone());
        self.shell.set_status(error);
        cx.notify();
    }

    pub(in crate::features) fn refresh_connection_auth_catalog(&mut self, cx: &mut Context<Self>) {
        self.refresh_connection_serial_ports();
        self.submit_store_request(
            0,
            store_request(StoreDomain::Security, |store| {
                Ok((
                    store.list_ssh_keys()?,
                    store.list_otp_entries()?,
                    store.list_passwords()?,
                    store.list_credentials()?,
                ))
            }),
            |this, event, cx| match event.outcome {
                Ok(catalog) => {
                    this.security
                        .replace_catalog(catalog.0, catalog.1, catalog.2, catalog.3);
                    cx.notify();
                }
                Err(error) => {
                    let message = format!("failed to refresh authentication catalog: {error}");
                    this.shell.set_status(message.clone());
                    this.settings.update_store_status(message, false);
                    cx.notify();
                }
            },
            cx,
        );
    }

    pub(in crate::features) fn refresh_connection_serial_ports(&mut self) {
        self.connection_state.replace_serial_ports(
            self.session
                .manager()
                .list_serial_ports()
                .unwrap_or_default(),
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::features) enum ConnectionEditorToggle {
    AutoFillOtp,
    DynamicTabTitle,
    X11,
    AgentForwarding,
    AgentExternal,
    AgentStoredKeys,
    SftpEnabled,
    RawTcp,
    LocalEcho,
    LocalLineEdit,
    ForceCharacterAtATime,
    SendNaws,
    SendSga,
    TelnetAutoLoginEnabled,
    TelnetAutoLoginSendWakeEnter,
    PostLogin,
    RdpUseNla,
    RdpReconnect,
    VncClipboard,
    VncReconnect,
    VncShared,
    VncViewOnly,
    RecordingUseGlobal,
    RecordingAutoStart,
    Advanced,
}
