use crate::SecretString;

use super::{
    ConnectionAuth, ConnectionNetwork, ConnectionPasswordSource, ConnectionPostLogin,
    ConnectionRecordingSettings, ConnectionType, ConnectionUsernameSource, RecordingMode,
    SavedConnection, SerialFlowControl, SftpCwdFollowMode, SftpSettings, SshAgentEndpoint,
    SshAgentForwardingConfig, SshAgentForwardingPolicy, SshAlgorithmMode, SshAlgorithmPreferences,
    SshProfile, SshTerminalType, default_rdp_certificate_policy, default_ssh_port,
    default_ssh_user, default_vnc_security_mode,
};

/// One batch-editable field with an explicit three-way state.
///
/// `Unchanged` is the only state that leaves every selected connection's value
/// alone. `Clear` never deletes a whole object; it restores that field's
/// documented default (or `None` where the field is optional).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum BatchField<T> {
    #[default]
    Unchanged,
    Set(T),
    Clear,
}

impl<T> BatchField<T> {
    pub fn is_unchanged(&self) -> bool {
        matches!(self, Self::Unchanged)
    }

    pub fn is_set(&self) -> bool {
        matches!(self, Self::Set(_))
    }

    pub fn is_clear(&self) -> bool {
        matches!(self, Self::Clear)
    }
}

/// Original protocol used to classify batch editing scopes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConnectionBatchKind {
    Ssh,
    LocalTerminal,
    Telnet,
    Serial,
    Rdp,
    Vnc,
}

impl ConnectionBatchKind {
    pub fn from_config(config: &ConnectionType) -> Self {
        match config {
            ConnectionType::Ssh { .. } => Self::Ssh,
            ConnectionType::LocalTerminal { .. } => Self::LocalTerminal,
            ConnectionType::Telnet { .. } => Self::Telnet,
            ConnectionType::Serial { .. } => Self::Serial,
            ConnectionType::Rdp { .. } => Self::Rdp,
            ConnectionType::Vnc { .. } => Self::Vnc,
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Self::Ssh => default_ssh_port(),
            Self::Telnet => super::default_telnet_port(),
            Self::Rdp => super::default_rdp_port(),
            Self::Vnc => super::default_vnc_port(),
            Self::LocalTerminal | Self::Serial => 0,
        }
    }
}

/// Connection-level batch edit instructions.
///
/// This intentionally carries no whole-connection object: every leaf field is
/// applied independently so untouched fields keep the per-connection value and
/// unknown (`extensions`/`extra`) data is never rewritten.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectionBatchEdit {
    pub group_id: BatchField<String>,
    pub description: BatchField<String>,
    pub icon: BatchField<String>,
    pub icon_auto_detect: BatchField<bool>,
    pub port: BatchField<u16>,
    pub username: BatchField<String>,

    pub host_key_alias: BatchField<String>,

    pub encoding: BatchField<String>,
    pub backspace_mode: BatchField<String>,
    pub ssh_profile: BatchField<SshProfile>,
    pub terminal_type: BatchField<SshTerminalType>,
    pub dynamic_tab_title: BatchField<bool>,

    pub sftp_enabled: BatchField<bool>,
    pub sftp_compatibility_mode: BatchField<bool>,
    pub sftp_cwd_follow_mode: BatchField<SftpCwdFollowMode>,
    pub sftp_shell_detection_timeout_ms: BatchField<u64>,
    pub sftp_filename_encoding: BatchField<String>,
    pub sftp_pipeline_depth: BatchField<u32>,

    pub x11_forwarding: BatchField<bool>,
    pub agent_forwarding_enabled: BatchField<bool>,

    pub post_login_enabled: BatchField<bool>,
    pub post_login_command: BatchField<String>,
    pub post_login_delay_ms: BatchField<u64>,

    pub ssh_algorithm_mode: BatchField<SshAlgorithmMode>,
    pub ssh_kex: BatchField<Vec<String>>,
    pub ssh_ciphers: BatchField<Vec<String>>,
    pub ssh_macs: BatchField<Vec<String>>,
    pub ssh_host_keys: BatchField<Vec<String>>,

    pub rdp_use_nla: BatchField<bool>,
    pub rdp_certificate_policy: BatchField<String>,
    pub rdp_administrative_session: BatchField<bool>,
    pub vnc_security_mode: BatchField<String>,

    pub tags: BatchField<Vec<String>>,
    pub recording_use_global: BatchField<bool>,
    pub recording_auto_start: BatchField<bool>,
    pub recording_mode: BatchField<RecordingMode>,

    pub auth_mode: BatchField<String>,
    pub account_id: BatchField<String>,
    pub password_id: BatchField<String>,
    pub password: BatchField<SecretString>,
    pub key_id: BatchField<String>,
    pub otp_id: BatchField<String>,
    pub auto_fill_otp: BatchField<bool>,
    pub auth_agent_endpoint: BatchField<SshAgentEndpoint>,
    pub agent_forwarding_external: BatchField<bool>,
    pub agent_forwarding_stored_keys: BatchField<bool>,
    pub agent_forwarding_policy: BatchField<SshAgentForwardingPolicy>,

    pub proxy_id: BatchField<String>,
    pub proxy_jump_id: BatchField<String>,

    pub shell_path: BatchField<String>,
    pub shell_args: BatchField<String>,
    pub working_dir: BatchField<String>,

    pub telnet_raw_tcp_cli: BatchField<bool>,
    pub telnet_enter_mode: BatchField<String>,
    pub telnet_local_echo: BatchField<bool>,
    pub telnet_local_line_edit: BatchField<bool>,
    pub telnet_force_character_at_a_time: BatchField<bool>,
    pub telnet_send_naws: BatchField<bool>,
    pub telnet_send_sga: BatchField<bool>,
    pub telnet_keep_alive_interval_secs: BatchField<u32>,
    pub telnet_auto_login_enabled: BatchField<bool>,
    pub telnet_auto_login_send_wake_enter: BatchField<bool>,
    pub telnet_auto_login_timeout_ms: BatchField<u64>,
    pub telnet_auto_login_max_retries: BatchField<u8>,
    pub telnet_auto_login_username_prompt: BatchField<String>,
    pub telnet_auto_login_password_prompt: BatchField<String>,
    pub telnet_auto_login_success_prompt: BatchField<String>,
    pub telnet_auto_login_failure_prompt: BatchField<String>,

    pub serial_port_name: BatchField<String>,
    pub serial_baud_rate: BatchField<u32>,
    pub serial_data_bits: BatchField<u8>,
    pub serial_parity: BatchField<String>,
    pub serial_stop_bits: BatchField<String>,
    pub serial_flow_control: BatchField<SerialFlowControl>,

    pub rdp_domain: BatchField<String>,
    pub rdp_backend: BatchField<String>,
    pub rdp_display_mode: BatchField<String>,
    pub rdp_display_width: BatchField<u32>,
    pub rdp_display_height: BatchField<u32>,
    pub rdp_display_wallpaper: BatchField<bool>,
    pub rdp_display_themes: BatchField<bool>,
    pub rdp_font_smoothing: BatchField<bool>,
    pub rdp_desktop_composition: BatchField<bool>,
    pub rdp_disable_full_window_drag: BatchField<bool>,
    pub rdp_disable_menu_animations: BatchField<bool>,
    pub rdp_disable_cursor_shadow: BatchField<bool>,
    pub rdp_disable_cursor_blinking: BatchField<bool>,
    pub rdp_auto_reconnect: BatchField<bool>,
    pub rdp_reconnect_attempts: BatchField<u32>,

    pub vnc_scale_mode: BatchField<String>,
    pub vnc_clipboard: BatchField<bool>,
    pub vnc_shared: BatchField<bool>,
    pub vnc_view_only: BatchField<bool>,
    pub vnc_reconnect: BatchField<bool>,
    pub vnc_reconnect_attempts: BatchField<u32>,
}

impl ConnectionBatchEdit {
    /// Whether every field is still `Unchanged`.
    pub fn is_empty(&self) -> bool {
        self.group_id.is_unchanged()
            && self.description.is_unchanged()
            && self.icon.is_unchanged()
            && self.icon_auto_detect.is_unchanged()
            && self.port.is_unchanged()
            && self.username.is_unchanged()
            && self.host_key_alias.is_unchanged()
            && self.encoding.is_unchanged()
            && self.backspace_mode.is_unchanged()
            && self.ssh_profile.is_unchanged()
            && self.terminal_type.is_unchanged()
            && self.dynamic_tab_title.is_unchanged()
            && self.sftp_enabled.is_unchanged()
            && self.sftp_compatibility_mode.is_unchanged()
            && self.sftp_cwd_follow_mode.is_unchanged()
            && self.sftp_shell_detection_timeout_ms.is_unchanged()
            && self.sftp_filename_encoding.is_unchanged()
            && self.sftp_pipeline_depth.is_unchanged()
            && self.x11_forwarding.is_unchanged()
            && self.agent_forwarding_enabled.is_unchanged()
            && self.post_login_enabled.is_unchanged()
            && self.post_login_command.is_unchanged()
            && self.post_login_delay_ms.is_unchanged()
            && self.ssh_algorithm_mode.is_unchanged()
            && self.ssh_kex.is_unchanged()
            && self.ssh_ciphers.is_unchanged()
            && self.ssh_macs.is_unchanged()
            && self.ssh_host_keys.is_unchanged()
            && self.rdp_use_nla.is_unchanged()
            && self.rdp_certificate_policy.is_unchanged()
            && self.rdp_administrative_session.is_unchanged()
            && self.vnc_security_mode.is_unchanged()
            && self.tags.is_unchanged()
            && self.recording_use_global.is_unchanged()
            && self.recording_auto_start.is_unchanged()
            && self.recording_mode.is_unchanged()
            && self.auth_mode.is_unchanged()
            && self.account_id.is_unchanged()
            && self.password_id.is_unchanged()
            && self.password.is_unchanged()
            && self.key_id.is_unchanged()
            && self.otp_id.is_unchanged()
            && self.auto_fill_otp.is_unchanged()
            && self.auth_agent_endpoint.is_unchanged()
            && self.agent_forwarding_external.is_unchanged()
            && self.agent_forwarding_stored_keys.is_unchanged()
            && self.agent_forwarding_policy.is_unchanged()
            && self.proxy_id.is_unchanged()
            && self.proxy_jump_id.is_unchanged()
            && self.shell_path.is_unchanged()
            && self.shell_args.is_unchanged()
            && self.working_dir.is_unchanged()
            && self.telnet_raw_tcp_cli.is_unchanged()
            && self.telnet_enter_mode.is_unchanged()
            && self.telnet_local_echo.is_unchanged()
            && self.telnet_local_line_edit.is_unchanged()
            && self.telnet_force_character_at_a_time.is_unchanged()
            && self.telnet_send_naws.is_unchanged()
            && self.telnet_send_sga.is_unchanged()
            && self.telnet_keep_alive_interval_secs.is_unchanged()
            && self.telnet_auto_login_enabled.is_unchanged()
            && self.telnet_auto_login_send_wake_enter.is_unchanged()
            && self.telnet_auto_login_timeout_ms.is_unchanged()
            && self.telnet_auto_login_max_retries.is_unchanged()
            && self.telnet_auto_login_username_prompt.is_unchanged()
            && self.telnet_auto_login_password_prompt.is_unchanged()
            && self.telnet_auto_login_success_prompt.is_unchanged()
            && self.telnet_auto_login_failure_prompt.is_unchanged()
            && self.serial_port_name.is_unchanged()
            && self.serial_baud_rate.is_unchanged()
            && self.serial_data_bits.is_unchanged()
            && self.serial_parity.is_unchanged()
            && self.serial_stop_bits.is_unchanged()
            && self.serial_flow_control.is_unchanged()
            && self.rdp_domain.is_unchanged()
            && self.rdp_backend.is_unchanged()
            && self.rdp_display_mode.is_unchanged()
            && self.rdp_display_width.is_unchanged()
            && self.rdp_display_height.is_unchanged()
            && self.rdp_display_wallpaper.is_unchanged()
            && self.rdp_display_themes.is_unchanged()
            && self.rdp_font_smoothing.is_unchanged()
            && self.rdp_desktop_composition.is_unchanged()
            && self.rdp_disable_full_window_drag.is_unchanged()
            && self.rdp_disable_menu_animations.is_unchanged()
            && self.rdp_disable_cursor_shadow.is_unchanged()
            && self.rdp_disable_cursor_blinking.is_unchanged()
            && self.rdp_auto_reconnect.is_unchanged()
            && self.rdp_reconnect_attempts.is_unchanged()
            && self.vnc_scale_mode.is_unchanged()
            && self.vnc_clipboard.is_unchanged()
            && self.vnc_shared.is_unchanged()
            && self.vnc_view_only.is_unchanged()
            && self.vnc_reconnect.is_unchanged()
            && self.vnc_reconnect_attempts.is_unchanged()
    }

    /// Whether at least one field was explicitly selected.
    pub fn has_changes(&self) -> bool {
        !self.is_empty()
    }
}

/// Why an explicitly selected leaf field could not be applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionBatchSkipReason {
    UnsupportedProtocol,
    MstscBackend,
    NonFixedRdpDisplay,
    GlobalRecording,
}

impl ConnectionBatchSkipReason {
    pub const ALL: [Self; 4] = [
        Self::UnsupportedProtocol,
        Self::MstscBackend,
        Self::NonFixedRdpDisplay,
        Self::GlobalRecording,
    ];

    fn index(self) -> usize {
        match self {
            Self::UnsupportedProtocol => 0,
            Self::MstscBackend => 1,
            Self::NonFixedRdpDisplay => 2,
            Self::GlobalRecording => 3,
        }
    }
}

/// Counts field/connection pairs, rather than distinct fields or connections.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConnectionBatchSkipCounts {
    counts: [usize; 4],
}

impl ConnectionBatchSkipCounts {
    pub fn count(&self, reason: ConnectionBatchSkipReason) -> usize {
        self.counts[reason.index()]
    }

    pub fn add(&mut self, reason: ConnectionBatchSkipReason, count: usize) {
        self.counts[reason.index()] += count;
    }

    pub fn merge(&mut self, other: Self) {
        for reason in ConnectionBatchSkipReason::ALL {
            self.add(reason, other.count(reason));
        }
    }
}

/// RDP fields with the same applicability rules. General fields include the
/// backend selector itself, so an external connection can switch to a helper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RdpBatchFieldGroup {
    General,
    SecurityAndDisplay,
    Dimensions,
}

/// Evaluate the effective configuration without changing the original or
/// normalizing persisted aliases. Both UI previews and patch application use it.
pub fn rdp_batch_field_skip_reason(
    config: &ConnectionType,
    edit: &ConnectionBatchEdit,
    group: RdpBatchFieldGroup,
) -> Option<ConnectionBatchSkipReason> {
    let ConnectionType::Rdp {
        backend, display, ..
    } = config
    else {
        return Some(ConnectionBatchSkipReason::UnsupportedProtocol);
    };
    if group == RdpBatchFieldGroup::General {
        return None;
    }
    let default_backend;
    let backend = match &edit.rdp_backend {
        BatchField::Unchanged => backend.as_str(),
        BatchField::Set(value) => value.as_str(),
        BatchField::Clear => {
            default_backend = super::default_rdp_backend();
            &default_backend
        }
    };
    if backend == "mstsc" {
        return Some(ConnectionBatchSkipReason::MstscBackend);
    }
    let default_mode;
    let mode = match &edit.rdp_display_mode {
        BatchField::Unchanged => display.mode.as_str(),
        BatchField::Set(value) => value.as_str(),
        BatchField::Clear => {
            default_mode = super::default_rdp_display_mode();
            &default_mode
        }
    };
    if group == RdpBatchFieldGroup::Dimensions && !matches!(mode, "fixed" | "native") {
        return Some(ConnectionBatchSkipReason::NonFixedRdpDisplay);
    }
    None
}

/// Result of applying one [`ConnectionBatchEdit`] to one connection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConnectionBatchApplyReport {
    /// Leaf fields written on this connection.
    pub applied_fields: usize,
    /// Leaf fields skipped because their protocol or dependencies do not apply.
    pub skipped_fields: usize,
    pub skipped_reasons: ConnectionBatchSkipCounts,
}

impl ConnectionBatchApplyReport {
    pub fn changed(&self) -> bool {
        self.applied_fields > 0
    }

    fn record(&mut self, touched: bool, reason: Option<ConnectionBatchSkipReason>) {
        if !touched {
            return;
        }
        if let Some(reason) = reason {
            self.skipped_fields += 1;
            self.skipped_reasons.add(reason, 1);
        } else {
            self.applied_fields += 1;
        }
    }
}

fn endpoint_user_default(config: &ConnectionType) -> String {
    match config {
        ConnectionType::Ssh { .. } => default_ssh_user(),
        _ => String::new(),
    }
}

fn endpoint_port_default(config: &ConnectionType) -> u16 {
    match config {
        ConnectionType::Ssh { .. } => default_ssh_port(),
        ConnectionType::Telnet { .. } => super::default_telnet_port(),
        ConnectionType::Rdp { .. } => super::default_rdp_port(),
        ConnectionType::Vnc { .. } => super::default_vnc_port(),
        ConnectionType::LocalTerminal { .. } | ConnectionType::Serial { .. } => 0,
    }
}

fn backspace_default(config: &ConnectionType) -> String {
    match config {
        ConnectionType::Ssh { .. } => super::default_backspace_mode_ssh(),
        ConnectionType::Telnet { .. } => super::default_backspace_mode_telnet(),
        ConnectionType::Serial { .. } => super::default_backspace_mode_serial(),
        _ => String::new(),
    }
}

fn supports_endpoint(config: &ConnectionType) -> bool {
    matches!(
        config,
        ConnectionType::Ssh { .. }
            | ConnectionType::Telnet { .. }
            | ConnectionType::Rdp { .. }
            | ConnectionType::Vnc { .. }
    )
}

fn supports_terminal_encoding(config: &ConnectionType) -> bool {
    matches!(
        config,
        ConnectionType::Ssh { .. }
            | ConnectionType::LocalTerminal { .. }
            | ConnectionType::Telnet { .. }
            | ConnectionType::Serial { .. }
    )
}

fn supports_backspace(config: &ConnectionType) -> bool {
    matches!(
        config,
        ConnectionType::Ssh { .. } | ConnectionType::Telnet { .. } | ConnectionType::Serial { .. }
    )
}

/// Applies `edit` to `connection`, writing only the fields explicitly selected.
///
/// Fields that do not apply to the concrete connection type are skipped and
/// counted instead of being coerced or reported as failures.
pub fn apply_connection_batch_edit(
    connection: &mut SavedConnection,
    edit: &ConnectionBatchEdit,
) -> ConnectionBatchApplyReport {
    let mut report = ConnectionBatchApplyReport::default();

    let apply = |condition: bool, touched: bool, report: &mut ConnectionBatchApplyReport| {
        report.record(
            touched,
            (!condition).then_some(ConnectionBatchSkipReason::UnsupportedProtocol),
        );
    };

    let is_ssh = matches!(connection.config, ConnectionType::Ssh { .. });
    let is_rdp = matches!(connection.config, ConnectionType::Rdp { .. });
    let is_vnc = matches!(connection.config, ConnectionType::Vnc { .. });
    let endpoint_ok = supports_endpoint(&connection.config);
    let rdp_reason = rdp_batch_field_skip_reason(
        &connection.config,
        edit,
        RdpBatchFieldGroup::SecurityAndDisplay,
    );
    let rdp_dimensions_reason =
        rdp_batch_field_skip_reason(&connection.config, edit, RdpBatchFieldGroup::Dimensions);
    let rdp_security_ok = rdp_reason.is_none();

    if !edit.group_id.is_unchanged() {
        connection.group_id = match &edit.group_id {
            BatchField::Set(value) => Some(value.clone()),
            BatchField::Clear => None,
            BatchField::Unchanged => unreachable!(),
        };
        apply(true, true, &mut report);
    }
    if !edit.description.is_unchanged() {
        connection.description = match &edit.description {
            BatchField::Set(value) => Some(value.clone()),
            BatchField::Clear => None,
            BatchField::Unchanged => unreachable!(),
        };
        apply(true, true, &mut report);
    }
    if !edit.icon.is_unchanged() {
        connection.icon = match &edit.icon {
            BatchField::Set(value) => Some(value.clone()),
            BatchField::Clear => None,
            BatchField::Unchanged => unreachable!(),
        };
        apply(true, true, &mut report);
    }
    if !edit.icon_auto_detect.is_unchanged() {
        connection.icon_auto_detect = match edit.icon_auto_detect {
            BatchField::Set(value) => Some(value),
            BatchField::Clear => None,
            BatchField::Unchanged => unreachable!(),
        };
        apply(true, true, &mut report);
    }

    if !edit.port.is_unchanged() {
        apply(endpoint_ok, true, &mut report);
        if endpoint_ok {
            let port = match edit.port {
                BatchField::Set(value) => value,
                BatchField::Clear => endpoint_port_default(&connection.config),
                BatchField::Unchanged => unreachable!(),
            };
            match &mut connection.config {
                ConnectionType::Ssh { port: slot, .. }
                | ConnectionType::Telnet { port: slot, .. }
                | ConnectionType::Rdp { port: slot, .. }
                | ConnectionType::Vnc { port: slot, .. } => *slot = port,
                _ => {}
            }
        }
    }
    if !edit.username.is_unchanged() {
        apply(endpoint_ok, true, &mut report);
        if endpoint_ok {
            let username = match &edit.username {
                BatchField::Set(value) => value.clone(),
                BatchField::Clear => endpoint_user_default(&connection.config),
                BatchField::Unchanged => unreachable!(),
            };
            match &mut connection.config {
                ConnectionType::Ssh { username: slot, .. }
                | ConnectionType::Telnet { username: slot, .. }
                | ConnectionType::Rdp { username: slot, .. }
                | ConnectionType::Vnc { username: slot, .. } => *slot = username,
                _ => {}
            }
        }
    }

    if !edit.host_key_alias.is_unchanged() {
        apply(is_ssh, true, &mut report);
        if is_ssh {
            let alias = match &edit.host_key_alias {
                BatchField::Set(value) => Some(value.clone()),
                BatchField::Clear => None,
                BatchField::Unchanged => unreachable!(),
            };
            match &mut connection.network {
                Some(network) => network.host_key_alias = alias,
                None if alias.is_some() => {
                    connection.network = Some(ConnectionNetwork {
                        proxy_id: None,
                        proxy_jump_id: None,
                        host_key_alias: alias,
                    });
                }
                None => {}
            }
        }
    }

    let encoding_ok = supports_terminal_encoding(&connection.config);
    if !edit.encoding.is_unchanged() {
        apply(encoding_ok, true, &mut report);
        if encoding_ok {
            let value = match &edit.encoding {
                BatchField::Set(value) => value.clone(),
                BatchField::Clear => String::new(),
                BatchField::Unchanged => unreachable!(),
            };
            match &mut connection.config {
                ConnectionType::Ssh { encoding, .. }
                | ConnectionType::LocalTerminal { encoding, .. }
                | ConnectionType::Telnet { encoding, .. }
                | ConnectionType::Serial { encoding, .. } => *encoding = value,
                _ => {}
            }
        }
    }

    let backspace_ok = supports_backspace(&connection.config);
    if !edit.backspace_mode.is_unchanged() {
        apply(backspace_ok, true, &mut report);
        if backspace_ok {
            let value = match &edit.backspace_mode {
                BatchField::Set(value) => value.clone(),
                BatchField::Clear => backspace_default(&connection.config),
                BatchField::Unchanged => unreachable!(),
            };
            match &mut connection.config {
                ConnectionType::Ssh { backspace_mode, .. }
                | ConnectionType::Telnet { backspace_mode, .. }
                | ConnectionType::Serial { backspace_mode, .. } => *backspace_mode = value,
                _ => {}
            }
        }
    }

    if !edit.ssh_profile.is_unchanged() {
        apply(is_ssh, true, &mut report);
        if is_ssh {
            connection.ssh_profile = match &edit.ssh_profile {
                BatchField::Set(value) => *value,
                BatchField::Clear => SshProfile::default(),
                BatchField::Unchanged => unreachable!(),
            };
        }
    }
    if !edit.terminal_type.is_unchanged() {
        apply(is_ssh, true, &mut report);
        if is_ssh {
            connection.terminal_type = match &edit.terminal_type {
                BatchField::Set(value) => Some(*value),
                BatchField::Clear => None,
                BatchField::Unchanged => unreachable!(),
            };
        }
    }
    let dynamic_title_ok = matches!(
        connection.config,
        ConnectionType::Ssh { .. } | ConnectionType::LocalTerminal { .. }
    );
    if !edit.dynamic_tab_title.is_unchanged() {
        apply(dynamic_title_ok, true, &mut report);
        if dynamic_title_ok {
            let value = flag_value(&edit.dynamic_tab_title, false);
            match &mut connection.config {
                ConnectionType::Ssh {
                    dynamic_tab_title, ..
                }
                | ConnectionType::LocalTerminal {
                    dynamic_tab_title, ..
                } => *dynamic_tab_title = value,
                _ => {}
            }
        }
    }

    if !edit.sftp_enabled.is_unchanged()
        || !edit.sftp_compatibility_mode.is_unchanged()
        || !edit.sftp_cwd_follow_mode.is_unchanged()
        || !edit.sftp_shell_detection_timeout_ms.is_unchanged()
        || !edit.sftp_filename_encoding.is_unchanged()
        || !edit.sftp_pipeline_depth.is_unchanged()
    {
        let sftp_touched = is_ssh;
        for touched in [
            !edit.sftp_enabled.is_unchanged(),
            !edit.sftp_compatibility_mode.is_unchanged(),
            !edit.sftp_cwd_follow_mode.is_unchanged(),
            !edit.sftp_shell_detection_timeout_ms.is_unchanged(),
            !edit.sftp_filename_encoding.is_unchanged(),
            !edit.sftp_pipeline_depth.is_unchanged(),
        ] {
            apply(sftp_touched, touched, &mut report);
        }
        if sftp_touched {
            apply_sftp(&mut connection.sftp, edit);
        }
    }

    if !edit.x11_forwarding.is_unchanged() {
        apply(is_ssh, true, &mut report);
        if let ConnectionType::Ssh { x11_forwarding, .. } = &mut connection.config {
            *x11_forwarding = flag_value(&edit.x11_forwarding, false);
        }
    }
    if !edit.agent_forwarding_enabled.is_unchanged() {
        apply(is_ssh, true, &mut report);
        if let ConnectionType::Ssh {
            agent_forwarding_config,
            ..
        } = &mut connection.config
        {
            let enabled = flag_value(&edit.agent_forwarding_enabled, false);
            match agent_forwarding_config {
                Some(config) => config.enabled = enabled,
                None if enabled => {
                    *agent_forwarding_config = Some(super::SshAgentForwardingConfig {
                        enabled,
                        ..Default::default()
                    });
                }
                None => {}
            }
        }
    }

    if !edit.post_login_enabled.is_unchanged()
        || !edit.post_login_command.is_unchanged()
        || !edit.post_login_delay_ms.is_unchanged()
    {
        let touched = [
            !edit.post_login_enabled.is_unchanged(),
            !edit.post_login_command.is_unchanged(),
            !edit.post_login_delay_ms.is_unchanged(),
        ];
        for touched in touched {
            apply(is_ssh, touched, &mut report);
        }
        if is_ssh {
            apply_post_login(&mut connection.post_login, edit);
        }
    }

    if !edit.ssh_algorithm_mode.is_unchanged()
        || !edit.ssh_kex.is_unchanged()
        || !edit.ssh_ciphers.is_unchanged()
        || !edit.ssh_macs.is_unchanged()
        || !edit.ssh_host_keys.is_unchanged()
    {
        let touched = [
            !edit.ssh_algorithm_mode.is_unchanged(),
            !edit.ssh_kex.is_unchanged(),
            !edit.ssh_ciphers.is_unchanged(),
            !edit.ssh_macs.is_unchanged(),
            !edit.ssh_host_keys.is_unchanged(),
        ];
        for touched in touched {
            apply(is_ssh, touched, &mut report);
        }
        if is_ssh {
            apply_ssh_algorithms(&mut connection.ssh_algorithms, edit);
        }
    }

    if !edit.rdp_use_nla.is_unchanged()
        || !edit.rdp_certificate_policy.is_unchanged()
        || !edit.rdp_administrative_session.is_unchanged()
    {
        let touched = [
            !edit.rdp_use_nla.is_unchanged(),
            !edit.rdp_certificate_policy.is_unchanged(),
            !edit.rdp_administrative_session.is_unchanged(),
        ];
        for touched in touched {
            report.record(touched, rdp_reason);
        }
        if rdp_security_ok && let ConnectionType::Rdp { security, .. } = &mut connection.config {
            if !edit.rdp_use_nla.is_unchanged() {
                security.use_nla = match edit.rdp_use_nla {
                    BatchField::Set(value) => value,
                    BatchField::Clear => super::RdpSecuritySettings::default().use_nla,
                    BatchField::Unchanged => unreachable!(),
                };
            }
            if !edit.rdp_certificate_policy.is_unchanged() {
                security.certificate_policy = match &edit.rdp_certificate_policy {
                    BatchField::Set(value) => value.clone(),
                    BatchField::Clear => default_rdp_certificate_policy(),
                    BatchField::Unchanged => unreachable!(),
                };
            }
            if !edit.rdp_administrative_session.is_unchanged() {
                security.administrative_session =
                    flag_value(&edit.rdp_administrative_session, false);
            }
        }
    }

    if !edit.vnc_security_mode.is_unchanged() {
        apply(is_vnc, true, &mut report);
        if let ConnectionType::Vnc { security, .. } = &mut connection.config {
            security.mode = match &edit.vnc_security_mode {
                BatchField::Set(value) => value.clone(),
                BatchField::Clear => default_vnc_security_mode(),
                BatchField::Unchanged => unreachable!(),
            };
        }
    }

    let is_telnet = matches!(connection.config, ConnectionType::Telnet { .. });
    let is_local = matches!(connection.config, ConnectionType::LocalTerminal { .. });
    let is_serial = matches!(connection.config, ConnectionType::Serial { .. });

    if !edit.tags.is_unchanged() {
        connection.tags = list_value(&edit.tags);
        apply(true, true, &mut report);
    }

    let recording_use_global = !edit.recording_use_global.is_unchanged();
    let recording_fields = [
        !edit.recording_auto_start.is_unchanged(),
        !edit.recording_mode.is_unchanged(),
    ];
    if recording_use_global {
        apply(true, true, &mut report);
        match edit.recording_use_global {
            BatchField::Set(true) | BatchField::Clear => connection.recording = None,
            BatchField::Set(false) => {
                connection
                    .recording
                    .get_or_insert_with(ConnectionRecordingSettings::default);
            }
            BatchField::Unchanged => unreachable!(),
        }
    }
    let recording_blocked = matches!(
        edit.recording_use_global,
        BatchField::Set(true) | BatchField::Clear
    );
    for touched in recording_fields {
        report.record(
            touched,
            recording_blocked.then_some(ConnectionBatchSkipReason::GlobalRecording),
        );
    }
    if !recording_blocked && recording_fields.iter().any(|touched| *touched) {
        let recording = connection
            .recording
            .get_or_insert_with(ConnectionRecordingSettings::default);
        if !edit.recording_auto_start.is_unchanged() {
            recording.auto_start = match edit.recording_auto_start {
                BatchField::Set(value) => Some(value),
                BatchField::Clear => None,
                BatchField::Unchanged => unreachable!(),
            };
        }
        if !edit.recording_mode.is_unchanged() {
            recording.mode = match &edit.recording_mode {
                BatchField::Set(value) => Some(*value),
                BatchField::Clear => None,
                BatchField::Unchanged => unreachable!(),
            };
        }
    }

    let account_ok = is_ssh || is_telnet;
    let password_reference_ok = is_rdp || is_vnc;

    if !edit.auth_mode.is_unchanged() {
        apply(endpoint_ok, true, &mut report);
        if endpoint_ok {
            let auth = connection.auth.get_or_insert_with(ConnectionAuth::default);
            auth.mode = match &edit.auth_mode {
                BatchField::Set(value) => value.clone(),
                BatchField::Clear => super::default_auth_mode(),
                BatchField::Unchanged => unreachable!(),
            };
        }
    }
    if !edit.account_id.is_unchanged() {
        apply(account_ok, true, &mut report);
        if account_ok {
            let auth = connection.auth.get_or_insert_with(ConnectionAuth::default);
            match &edit.account_id {
                BatchField::Set(value) => {
                    auth.account_id = Some(value.clone());
                    auth.password_id = None;
                    auth.username_source = Some(ConnectionUsernameSource::Account);
                    auth.password_source = Some(ConnectionPasswordSource::Account);
                }
                BatchField::Clear => {
                    auth.account_id = None;
                    auth.password_id = None;
                    auth.username_source = Some(ConnectionUsernameSource::Connection);
                    auth.password_source = Some(ConnectionPasswordSource::Connection);
                }
                BatchField::Unchanged => unreachable!(),
            }
        }
    }
    // An explicit username wins over the account's default source in the same patch.
    if account_ok
        && !edit.username.is_unchanged()
        && let Some(auth) = connection.auth.as_mut()
        && (auth.account_id.is_some() || auth.password_id.is_some())
    {
        auth.username_source = Some(ConnectionUsernameSource::Connection);
    }
    if !edit.password_id.is_unchanged() {
        apply(password_reference_ok, true, &mut report);
        if password_reference_ok {
            let auth = connection.auth.get_or_insert_with(ConnectionAuth::default);
            match &edit.password_id {
                BatchField::Set(value) => {
                    auth.password_id = Some(value.clone());
                    auth.account_id = None;
                    auth.password_source = Some(ConnectionPasswordSource::Account);
                }
                BatchField::Clear => {
                    auth.password_id = None;
                    auth.account_id = None;
                    auth.password_source = Some(ConnectionPasswordSource::Connection);
                }
                BatchField::Unchanged => unreachable!(),
            }
        }
    }
    if !edit.password.is_unchanged() {
        apply(endpoint_ok, true, &mut report);
        if endpoint_ok {
            let auth = connection.auth.get_or_insert_with(ConnectionAuth::default);
            match &edit.password {
                BatchField::Set(value) => {
                    auth.password = Some(value.clone());
                    auth.has_password = false;
                    auth.password_source = Some(ConnectionPasswordSource::Connection);
                }
                BatchField::Clear => {
                    auth.password = None;
                    auth.has_password = false;
                    auth.password_source = Some(ConnectionPasswordSource::Connection);
                }
                BatchField::Unchanged => unreachable!(),
            }
        }
    }
    if !edit.key_id.is_unchanged() {
        apply(is_ssh, true, &mut report);
        if is_ssh {
            let auth = connection.auth.get_or_insert_with(ConnectionAuth::default);
            match &edit.key_id {
                BatchField::Set(value) => {
                    auth.key_id = Some(value.clone());
                }
                BatchField::Clear => auth.key_id = None,
                BatchField::Unchanged => unreachable!(),
            }
        }
    }
    if !edit.otp_id.is_unchanged() {
        apply(is_ssh, true, &mut report);
        if is_ssh {
            let auth = connection.auth.get_or_insert_with(ConnectionAuth::default);
            auth.otp_id = match &edit.otp_id {
                BatchField::Set(value) => Some(value.clone()),
                BatchField::Clear => None,
                BatchField::Unchanged => unreachable!(),
            };
        }
    }
    if !edit.auto_fill_otp.is_unchanged() {
        apply(is_ssh, true, &mut report);
        if is_ssh {
            let auth = connection.auth.get_or_insert_with(ConnectionAuth::default);
            auth.auto_fill_otp = flag_value(&edit.auto_fill_otp, false);
        }
    }
    if !edit.auth_agent_endpoint.is_unchanged() {
        apply(is_ssh, true, &mut report);
        if let ConnectionType::Ssh {
            auth_agent_endpoint,
            ..
        } = &mut connection.config
        {
            *auth_agent_endpoint = match &edit.auth_agent_endpoint {
                BatchField::Set(value) => Some(value.clone()),
                BatchField::Clear => None,
                BatchField::Unchanged => unreachable!(),
            };
        }
    }

    let agent_touched = [
        !edit.agent_forwarding_external.is_unchanged(),
        !edit.agent_forwarding_stored_keys.is_unchanged(),
        !edit.agent_forwarding_policy.is_unchanged(),
    ];
    for touched in agent_touched {
        apply(is_ssh, touched, &mut report);
    }
    if is_ssh && agent_touched.iter().any(|touched| *touched) {
        let agent = match &mut connection.config {
            ConnectionType::Ssh {
                agent_forwarding_config,
                ..
            } => agent_forwarding_config.get_or_insert_with(SshAgentForwardingConfig::default),
            _ => unreachable!(),
        };
        if !edit.agent_forwarding_external.is_unchanged() {
            agent.sources.external_agent = flag_value(&edit.agent_forwarding_external, false);
        }
        if !edit.agent_forwarding_stored_keys.is_unchanged() {
            agent.sources.stored_keys = flag_value(&edit.agent_forwarding_stored_keys, true);
        }
        if !edit.agent_forwarding_policy.is_unchanged() {
            let fingerprints = match &agent.policy {
                SshAgentForwardingPolicy::Allowlist { fingerprints } => fingerprints.clone(),
                SshAgentForwardingPolicy::All => Vec::new(),
            };
            agent.policy = match &edit.agent_forwarding_policy {
                BatchField::Set(SshAgentForwardingPolicy::All) => SshAgentForwardingPolicy::All,
                BatchField::Set(SshAgentForwardingPolicy::Allowlist { .. }) => {
                    SshAgentForwardingPolicy::Allowlist { fingerprints }
                }
                BatchField::Clear => SshAgentForwardingPolicy::default(),
                BatchField::Unchanged => unreachable!(),
            };
        }
    }

    let network_ok = endpoint_ok;
    if !edit.proxy_id.is_unchanged() {
        apply(network_ok, true, &mut report);
        if network_ok {
            let network = connection
                .network
                .get_or_insert_with(ConnectionNetwork::default);
            network.proxy_id = match &edit.proxy_id {
                BatchField::Set(value) => Some(value.clone()),
                BatchField::Clear => None,
                BatchField::Unchanged => unreachable!(),
            };
        }
    }
    if !edit.proxy_jump_id.is_unchanged() {
        apply(network_ok, true, &mut report);
        if network_ok {
            let network = connection
                .network
                .get_or_insert_with(ConnectionNetwork::default);
            network.proxy_jump_id = match &edit.proxy_jump_id {
                BatchField::Set(value) => Some(value.clone()),
                BatchField::Clear => None,
                BatchField::Unchanged => unreachable!(),
            };
        }
    }

    if !edit.shell_path.is_unchanged() {
        apply(is_local, true, &mut report);
        if let ConnectionType::LocalTerminal { shell_path, .. } = &mut connection.config {
            *shell_path = plain_string_value(&edit.shell_path);
        }
    }
    if !edit.shell_args.is_unchanged() {
        apply(is_local, true, &mut report);
        if let ConnectionType::LocalTerminal { shell_args, .. } = &mut connection.config {
            *shell_args = plain_string_value(&edit.shell_args);
        }
    }
    if !edit.working_dir.is_unchanged() {
        apply(is_local, true, &mut report);
        if let ConnectionType::LocalTerminal { working_dir, .. } = &mut connection.config {
            *working_dir = match &edit.working_dir {
                BatchField::Set(value) if !value.trim().is_empty() => Some(value.clone()),
                BatchField::Set(_) | BatchField::Clear => None,
                BatchField::Unchanged => unreachable!(),
            };
        }
    }

    let telnet_fields = [
        !edit.telnet_raw_tcp_cli.is_unchanged(),
        !edit.telnet_enter_mode.is_unchanged(),
        !edit.telnet_local_echo.is_unchanged(),
        !edit.telnet_local_line_edit.is_unchanged(),
        !edit.telnet_force_character_at_a_time.is_unchanged(),
        !edit.telnet_send_naws.is_unchanged(),
        !edit.telnet_send_sga.is_unchanged(),
        !edit.telnet_keep_alive_interval_secs.is_unchanged(),
        !edit.telnet_auto_login_enabled.is_unchanged(),
        !edit.telnet_auto_login_send_wake_enter.is_unchanged(),
        !edit.telnet_auto_login_timeout_ms.is_unchanged(),
        !edit.telnet_auto_login_max_retries.is_unchanged(),
        !edit.telnet_auto_login_username_prompt.is_unchanged(),
        !edit.telnet_auto_login_password_prompt.is_unchanged(),
        !edit.telnet_auto_login_success_prompt.is_unchanged(),
        !edit.telnet_auto_login_failure_prompt.is_unchanged(),
    ];
    for touched in telnet_fields {
        apply(is_telnet, touched, &mut report);
    }
    if is_telnet
        && telnet_fields.iter().any(|touched| *touched)
        && let ConnectionType::Telnet {
            raw_tcp_cli,
            enter_mode,
            local_echo,
            local_line_edit,
            force_character_at_a_time,
            send_naws,
            send_sga,
            keep_alive_interval_secs,
            auto_login,
            ..
        } = &mut connection.config
    {
        if !edit.telnet_raw_tcp_cli.is_unchanged() {
            *raw_tcp_cli = flag_value(&edit.telnet_raw_tcp_cli, false);
        }
        if !edit.telnet_enter_mode.is_unchanged() {
            *enter_mode = match &edit.telnet_enter_mode {
                BatchField::Set(value) => value.clone(),
                BatchField::Clear => super::default_telnet_enter_mode(),
                BatchField::Unchanged => unreachable!(),
            };
        }
        if !edit.telnet_local_echo.is_unchanged() {
            *local_echo = flag_value(&edit.telnet_local_echo, false);
        }
        if !edit.telnet_local_line_edit.is_unchanged() {
            *local_line_edit = flag_value(&edit.telnet_local_line_edit, false);
        }
        if !edit.telnet_force_character_at_a_time.is_unchanged() {
            *force_character_at_a_time = flag_value(&edit.telnet_force_character_at_a_time, false);
        }
        if !edit.telnet_send_naws.is_unchanged() {
            *send_naws = flag_value(&edit.telnet_send_naws, true);
        }
        if !edit.telnet_send_sga.is_unchanged() {
            *send_sga = flag_value(&edit.telnet_send_sga, true);
        }
        if !edit.telnet_keep_alive_interval_secs.is_unchanged() {
            *keep_alive_interval_secs = match edit.telnet_keep_alive_interval_secs {
                BatchField::Set(value) => value,
                BatchField::Clear => super::DEFAULT_TELNET_KEEP_ALIVE_INTERVAL_SECS,
                BatchField::Unchanged => unreachable!(),
            };
        }
        if !edit.telnet_auto_login_enabled.is_unchanged() {
            auto_login.enabled = flag_value(&edit.telnet_auto_login_enabled, true);
        }
        if !edit.telnet_auto_login_send_wake_enter.is_unchanged() {
            auto_login.send_wake_enter = flag_value(&edit.telnet_auto_login_send_wake_enter, true);
        }
        if !edit.telnet_auto_login_timeout_ms.is_unchanged() {
            auto_login.timeout_ms = match edit.telnet_auto_login_timeout_ms {
                BatchField::Set(value) => value,
                BatchField::Clear => super::default_telnet_auto_login_timeout_ms(),
                BatchField::Unchanged => unreachable!(),
            };
        }
        if !edit.telnet_auto_login_max_retries.is_unchanged() {
            auto_login.max_retries = match edit.telnet_auto_login_max_retries {
                BatchField::Set(value) => value,
                BatchField::Clear => 0,
                BatchField::Unchanged => unreachable!(),
            };
        }
        if !edit.telnet_auto_login_username_prompt.is_unchanged() {
            auto_login.username_prompt_regex = regex_value(&edit.telnet_auto_login_username_prompt);
        }
        if !edit.telnet_auto_login_password_prompt.is_unchanged() {
            auto_login.password_prompt_regex = regex_value(&edit.telnet_auto_login_password_prompt);
        }
        if !edit.telnet_auto_login_success_prompt.is_unchanged() {
            auto_login.success_prompt_regex = regex_value(&edit.telnet_auto_login_success_prompt);
        }
        if !edit.telnet_auto_login_failure_prompt.is_unchanged() {
            auto_login.failure_prompt_regex = regex_value(&edit.telnet_auto_login_failure_prompt);
        }
    }

    let serial_fields = [
        !edit.serial_port_name.is_unchanged(),
        !edit.serial_baud_rate.is_unchanged(),
        !edit.serial_data_bits.is_unchanged(),
        !edit.serial_parity.is_unchanged(),
        !edit.serial_stop_bits.is_unchanged(),
        !edit.serial_flow_control.is_unchanged(),
    ];
    for touched in serial_fields {
        apply(is_serial, touched, &mut report);
    }
    if is_serial
        && serial_fields.iter().any(|touched| *touched)
        && let ConnectionType::Serial {
            port_name,
            baud_rate,
            data_bits,
            parity,
            flow_control,
            stop_bits,
            ..
        } = &mut connection.config
    {
        if !edit.serial_port_name.is_unchanged() {
            *port_name = plain_string_value(&edit.serial_port_name);
        }
        if !edit.serial_baud_rate.is_unchanged() {
            *baud_rate = match edit.serial_baud_rate {
                BatchField::Set(value) => value,
                BatchField::Clear => super::default_baud_rate(),
                BatchField::Unchanged => unreachable!(),
            };
        }
        if !edit.serial_data_bits.is_unchanged() {
            *data_bits = match edit.serial_data_bits {
                BatchField::Set(value) => value,
                BatchField::Clear => super::default_data_bits(),
                BatchField::Unchanged => unreachable!(),
            };
        }
        if !edit.serial_parity.is_unchanged() {
            *parity = match &edit.serial_parity {
                BatchField::Set(value) => value.clone(),
                BatchField::Clear => super::default_parity(),
                BatchField::Unchanged => unreachable!(),
            };
        }
        if !edit.serial_stop_bits.is_unchanged() {
            *stop_bits = match &edit.serial_stop_bits {
                BatchField::Set(value) => value.clone(),
                BatchField::Clear => super::default_stop_bits(),
                BatchField::Unchanged => unreachable!(),
            };
        }
        if !edit.serial_flow_control.is_unchanged() {
            *flow_control = match edit.serial_flow_control {
                BatchField::Set(value) => value,
                BatchField::Clear => SerialFlowControl::default(),
                BatchField::Unchanged => unreachable!(),
            };
        }
    }

    if !edit.rdp_domain.is_unchanged() {
        apply(is_rdp, true, &mut report);
        if let ConnectionType::Rdp { domain, .. } = &mut connection.config {
            *domain = plain_string_value(&edit.rdp_domain);
        }
    }
    if !edit.rdp_backend.is_unchanged() {
        apply(is_rdp, true, &mut report);
        if let ConnectionType::Rdp { backend, .. } = &mut connection.config {
            *backend = match &edit.rdp_backend {
                BatchField::Set(value) => value.clone(),
                BatchField::Clear => super::default_rdp_backend(),
                BatchField::Unchanged => unreachable!(),
            };
        }
    }
    let rdp_display_ok = rdp_reason.is_none();

    if !edit.rdp_display_mode.is_unchanged() {
        report.record(true, rdp_reason);
        if let ConnectionType::Rdp { display, .. } = &mut connection.config
            && rdp_display_ok
        {
            display.mode = match &edit.rdp_display_mode {
                BatchField::Set(value) => value.clone(),
                BatchField::Clear => super::default_rdp_display_mode(),
                BatchField::Unchanged => unreachable!(),
            };
        }
    }
    for touched in [
        !edit.rdp_display_width.is_unchanged(),
        !edit.rdp_display_height.is_unchanged(),
    ] {
        report.record(touched, rdp_dimensions_reason);
    }
    if rdp_dimensions_reason.is_none()
        && let ConnectionType::Rdp { display, .. } = &mut connection.config
    {
        if !edit.rdp_display_width.is_unchanged() {
            display.width = match edit.rdp_display_width {
                BatchField::Set(value) => value,
                BatchField::Clear => super::default_rdp_width(),
                BatchField::Unchanged => unreachable!(),
            };
        }
        if !edit.rdp_display_height.is_unchanged() {
            display.height = match edit.rdp_display_height {
                BatchField::Set(value) => value,
                BatchField::Clear => super::default_rdp_height(),
                BatchField::Unchanged => unreachable!(),
            };
        }
    }

    let rdp_display_effect_fields = [
        !edit.rdp_display_wallpaper.is_unchanged(),
        !edit.rdp_display_themes.is_unchanged(),
        !edit.rdp_font_smoothing.is_unchanged(),
        !edit.rdp_desktop_composition.is_unchanged(),
        !edit.rdp_disable_full_window_drag.is_unchanged(),
        !edit.rdp_disable_menu_animations.is_unchanged(),
        !edit.rdp_disable_cursor_shadow.is_unchanged(),
        !edit.rdp_disable_cursor_blinking.is_unchanged(),
        !edit.rdp_auto_reconnect.is_unchanged(),
        !edit.rdp_reconnect_attempts.is_unchanged(),
    ];
    for touched in rdp_display_effect_fields {
        report.record(touched, rdp_reason);
    }
    if rdp_display_ok
        && rdp_display_effect_fields.iter().any(|touched| *touched)
        && let ConnectionType::Rdp {
            display, reconnect, ..
        } = &mut connection.config
    {
        if !edit.rdp_display_wallpaper.is_unchanged() {
            display.display_wallpaper = flag_value(&edit.rdp_display_wallpaper, true);
        }
        if !edit.rdp_display_themes.is_unchanged() {
            display.display_themes = flag_value(&edit.rdp_display_themes, true);
        }
        if !edit.rdp_font_smoothing.is_unchanged() {
            display.enable_font_smoothing = flag_value(&edit.rdp_font_smoothing, true);
        }
        if !edit.rdp_desktop_composition.is_unchanged() {
            display.enable_desktop_composition = flag_value(&edit.rdp_desktop_composition, false);
        }
        if !edit.rdp_disable_full_window_drag.is_unchanged() {
            display.disable_full_window_drag = flag_value(&edit.rdp_disable_full_window_drag, true);
        }
        if !edit.rdp_disable_menu_animations.is_unchanged() {
            display.disable_menu_animations = flag_value(&edit.rdp_disable_menu_animations, true);
        }
        if !edit.rdp_disable_cursor_shadow.is_unchanged() {
            display.disable_cursor_shadow = flag_value(&edit.rdp_disable_cursor_shadow, false);
        }
        if !edit.rdp_disable_cursor_blinking.is_unchanged() {
            display.disable_cursor_blinking = flag_value(&edit.rdp_disable_cursor_blinking, false);
        }
        if !edit.rdp_auto_reconnect.is_unchanged() {
            reconnect.enabled = flag_value(&edit.rdp_auto_reconnect, true);
        }
        if !edit.rdp_reconnect_attempts.is_unchanged() {
            reconnect.max_attempts = match edit.rdp_reconnect_attempts {
                BatchField::Set(value) => value,
                BatchField::Clear => super::default_rdp_reconnect_attempts(),
                BatchField::Unchanged => unreachable!(),
            };
        }
    }

    let vnc_fields = [
        !edit.vnc_scale_mode.is_unchanged(),
        !edit.vnc_clipboard.is_unchanged(),
        !edit.vnc_shared.is_unchanged(),
        !edit.vnc_view_only.is_unchanged(),
        !edit.vnc_reconnect.is_unchanged(),
        !edit.vnc_reconnect_attempts.is_unchanged(),
    ];
    for touched in vnc_fields {
        apply(is_vnc, touched, &mut report);
    }
    if is_vnc
        && vnc_fields.iter().any(|touched| *touched)
        && let ConnectionType::Vnc {
            display,
            clipboard,
            reconnect,
            shared,
            view_only,
            ..
        } = &mut connection.config
    {
        if !edit.vnc_scale_mode.is_unchanged() {
            display.scale_mode = match &edit.vnc_scale_mode {
                BatchField::Set(value) => value.clone(),
                BatchField::Clear => super::default_vnc_scale_mode(),
                BatchField::Unchanged => unreachable!(),
            };
        }
        if !edit.vnc_clipboard.is_unchanged() {
            clipboard.enabled = flag_value(&edit.vnc_clipboard, true);
        }
        if !edit.vnc_shared.is_unchanged() {
            *shared = flag_value(&edit.vnc_shared, true);
        }
        if !edit.vnc_view_only.is_unchanged() {
            *view_only = flag_value(&edit.vnc_view_only, false);
        }
        if !edit.vnc_reconnect.is_unchanged() {
            reconnect.enabled = flag_value(&edit.vnc_reconnect, true);
        }
        if !edit.vnc_reconnect_attempts.is_unchanged() {
            reconnect.max_attempts = match edit.vnc_reconnect_attempts {
                BatchField::Set(value) => value,
                BatchField::Clear => super::default_vnc_reconnect_attempts(),
                BatchField::Unchanged => unreachable!(),
            };
        }
    }

    report
}

fn flag_value(field: &BatchField<bool>, default: bool) -> bool {
    match field {
        BatchField::Set(value) => *value,
        BatchField::Clear => default,
        BatchField::Unchanged => unreachable!("only touched fields are applied"),
    }
}

fn plain_string_value(field: &BatchField<String>) -> String {
    match field {
        BatchField::Set(value) => value.clone(),
        BatchField::Clear => String::new(),
        BatchField::Unchanged => unreachable!(),
    }
}

fn regex_value(field: &BatchField<String>) -> Option<String> {
    match field {
        BatchField::Set(value) if !value.trim().is_empty() => Some(value.clone()),
        BatchField::Set(_) | BatchField::Clear => None,
        BatchField::Unchanged => unreachable!(),
    }
}

fn apply_sftp(sftp: &mut SftpSettings, edit: &ConnectionBatchEdit) {
    if !edit.sftp_enabled.is_unchanged() {
        sftp.enabled = match edit.sftp_enabled {
            BatchField::Set(value) => value,
            BatchField::Clear => SftpSettings::default().enabled,
            BatchField::Unchanged => unreachable!(),
        };
    }
    if !edit.sftp_compatibility_mode.is_unchanged() {
        sftp.compatibility_mode = match edit.sftp_compatibility_mode {
            BatchField::Set(value) => value,
            BatchField::Clear => SftpSettings::default().compatibility_mode,
            BatchField::Unchanged => unreachable!(),
        };
    }
    if !edit.sftp_cwd_follow_mode.is_unchanged() {
        sftp.cwd_follow_mode = match &edit.sftp_cwd_follow_mode {
            BatchField::Set(value) => value.clone(),
            BatchField::Clear => SftpCwdFollowMode::default(),
            BatchField::Unchanged => unreachable!(),
        };
    }
    if !edit.sftp_shell_detection_timeout_ms.is_unchanged() {
        sftp.shell_detection_timeout_ms = match edit.sftp_shell_detection_timeout_ms {
            BatchField::Set(value) => value,
            BatchField::Clear => super::default_sftp_shell_detection_timeout_ms(),
            BatchField::Unchanged => unreachable!(),
        };
    }
    if !edit.sftp_filename_encoding.is_unchanged() {
        sftp.filename_encoding = match &edit.sftp_filename_encoding {
            BatchField::Set(value) => value.clone(),
            BatchField::Clear => String::new(),
            BatchField::Unchanged => unreachable!(),
        };
    }
    if !edit.sftp_pipeline_depth.is_unchanged() {
        sftp.pipeline_depth = match edit.sftp_pipeline_depth {
            BatchField::Set(value) => Some(value),
            BatchField::Clear => None,
            BatchField::Unchanged => unreachable!(),
        };
    }
}

fn apply_post_login(post_login: &mut Option<ConnectionPostLogin>, edit: &ConnectionBatchEdit) {
    if edit.post_login_enabled.is_clear() {
        *post_login = None;
        return;
    }

    let needs_object = !edit.post_login_enabled.is_unchanged()
        || !edit.post_login_command.is_unchanged()
        || !edit.post_login_delay_ms.is_unchanged();
    if !needs_object {
        return;
    }

    let object = post_login.get_or_insert_with(ConnectionPostLogin::default);
    if !edit.post_login_enabled.is_unchanged() {
        object.enabled = flag_value(&edit.post_login_enabled, false);
    }
    if !edit.post_login_command.is_unchanged() {
        object.command = match &edit.post_login_command {
            BatchField::Set(value) => value.clone(),
            BatchField::Clear => String::new(),
            BatchField::Unchanged => unreachable!(),
        };
    }
    if !edit.post_login_delay_ms.is_unchanged() {
        object.delay_ms = match edit.post_login_delay_ms {
            BatchField::Set(value) => value,
            BatchField::Clear => super::default_post_login_delay_ms(),
            BatchField::Unchanged => unreachable!(),
        };
    }
}

fn apply_ssh_algorithms(
    algorithms: &mut Option<SshAlgorithmPreferences>,
    edit: &ConnectionBatchEdit,
) {
    let needs_object = !edit.ssh_algorithm_mode.is_unchanged()
        || !edit.ssh_kex.is_unchanged()
        || !edit.ssh_ciphers.is_unchanged()
        || !edit.ssh_macs.is_unchanged()
        || !edit.ssh_host_keys.is_unchanged();
    if !needs_object {
        return;
    }

    let object = algorithms.get_or_insert_with(SshAlgorithmPreferences::default);
    if !edit.ssh_algorithm_mode.is_unchanged() {
        object.mode = match &edit.ssh_algorithm_mode {
            BatchField::Set(value) => value.clone(),
            BatchField::Clear => SshAlgorithmMode::default(),
            BatchField::Unchanged => unreachable!(),
        };
    }
    if !edit.ssh_kex.is_unchanged() {
        object.kex = list_value(&edit.ssh_kex);
    }
    if !edit.ssh_ciphers.is_unchanged() {
        object.ciphers = list_value(&edit.ssh_ciphers);
    }
    if !edit.ssh_macs.is_unchanged() {
        object.macs = list_value(&edit.ssh_macs);
    }
    if !edit.ssh_host_keys.is_unchanged() {
        object.host_keys = list_value(&edit.ssh_host_keys);
    }
}

fn list_value(field: &BatchField<Vec<String>>) -> Vec<String> {
    match field {
        BatchField::Set(value) => value.clone(),
        BatchField::Clear => Vec::new(),
        BatchField::Unchanged => unreachable!(),
    }
}

/// Validation shared by the desktop form and the store job.
pub fn validate_connection_batch_edit(edit: &ConnectionBatchEdit) -> Result<(), String> {
    if !edit.has_changes() {
        return Err("no fields selected".to_string());
    }
    for (label, field) in [
        ("group_id", &edit.group_id),
        ("description", &edit.description),
        ("icon", &edit.icon),
        ("username", &edit.username),
        ("host_key_alias", &edit.host_key_alias),
        ("encoding", &edit.encoding),
        ("backspace_mode", &edit.backspace_mode),
        ("sftp_filename_encoding", &edit.sftp_filename_encoding),
        ("post_login_command", &edit.post_login_command),
        ("rdp_certificate_policy", &edit.rdp_certificate_policy),
        ("vnc_security_mode", &edit.vnc_security_mode),
        ("auth_mode", &edit.auth_mode),
        ("account_id", &edit.account_id),
        ("password_id", &edit.password_id),
        ("key_id", &edit.key_id),
        ("otp_id", &edit.otp_id),
        ("proxy_id", &edit.proxy_id),
        ("proxy_jump_id", &edit.proxy_jump_id),
        ("shell_path", &edit.shell_path),
        ("shell_args", &edit.shell_args),
        ("working_dir", &edit.working_dir),
        ("telnet_enter_mode", &edit.telnet_enter_mode),
        (
            "telnet_auto_login_username_prompt",
            &edit.telnet_auto_login_username_prompt,
        ),
        (
            "telnet_auto_login_password_prompt",
            &edit.telnet_auto_login_password_prompt,
        ),
        (
            "telnet_auto_login_success_prompt",
            &edit.telnet_auto_login_success_prompt,
        ),
        (
            "telnet_auto_login_failure_prompt",
            &edit.telnet_auto_login_failure_prompt,
        ),
        ("serial_port_name", &edit.serial_port_name),
        ("serial_parity", &edit.serial_parity),
        ("serial_stop_bits", &edit.serial_stop_bits),
        ("rdp_domain", &edit.rdp_domain),
        ("rdp_backend", &edit.rdp_backend),
        ("rdp_display_mode", &edit.rdp_display_mode),
        ("vnc_scale_mode", &edit.vnc_scale_mode),
    ] {
        if let BatchField::Set(value) = field
            && value.trim().is_empty()
        {
            return Err(format!("{label} cannot be empty; use clear instead"));
        }
    }
    if let BatchField::Set(value) = &edit.password
        && value.is_empty()
    {
        return Err("password cannot be empty; use clear instead".to_string());
    }
    if let BatchField::Set(endpoint) = &edit.auth_agent_endpoint {
        super::validate_ssh_agent_endpoint(endpoint)
            .map_err(|_| "invalid SSH agent endpoint".to_string())?;
    }
    if let BatchField::Set(port) = edit.port
        && port == 0
    {
        return Err("port must be between 1 and 65535".to_string());
    }
    if let BatchField::Set(value) = &edit.description
        && value.trim().is_empty()
    {
        return Err("description cannot be empty; use clear instead".to_string());
    }
    if let BatchField::Set(value) = &edit.icon
        && value.trim().is_empty()
    {
        return Err("icon cannot be empty; use clear instead".to_string());
    }
    if let BatchField::Set(value) = &edit.host_key_alias
        && value.trim().is_empty()
    {
        return Err("host key alias cannot be empty; use clear instead".to_string());
    }
    if let BatchField::Set(value) = &edit.sftp_filename_encoding
        && value.trim().is_empty()
    {
        return Err("SFTP filename encoding cannot be empty; use clear instead".to_string());
    }
    if let BatchField::Set(value) = edit.sftp_shell_detection_timeout_ms
        && !(super::MIN_SFTP_SHELL_DETECTION_TIMEOUT_MS
            ..=super::MAX_SFTP_SHELL_DETECTION_TIMEOUT_MS)
            .contains(&value)
    {
        return Err(format!(
            "SFTP shell detection timeout must be between {} and {} ms",
            super::MIN_SFTP_SHELL_DETECTION_TIMEOUT_MS,
            super::MAX_SFTP_SHELL_DETECTION_TIMEOUT_MS
        ));
    }
    if let BatchField::Set(value) = edit.sftp_pipeline_depth
        && value == 0
    {
        return Err("SFTP pipeline depth must be greater than 0".to_string());
    }
    if let BatchField::Set(value) = &edit.post_login_command
        && value.trim().is_empty()
    {
        return Err("post-login command cannot be empty; use clear instead".to_string());
    }
    if let BatchField::Set(value) = &edit.rdp_certificate_policy
        && value.trim().is_empty()
    {
        return Err("RDP certificate policy cannot be empty; use clear instead".to_string());
    }
    if let BatchField::Set(value) = &edit.vnc_security_mode
        && value.trim().is_empty()
    {
        return Err("VNC security mode cannot be empty; use clear instead".to_string());
    }
    if let BatchField::Set(0) = edit.rdp_display_width {
        return Err("RDP display width must be greater than 0".to_string());
    }
    if let BatchField::Set(0) = edit.rdp_display_height {
        return Err("RDP display height must be greater than 0".to_string());
    }
    if let BatchField::Set(0) = edit.serial_baud_rate {
        return Err("serial baud rate must be greater than 0".to_string());
    }
    if let BatchField::Set(value) = edit.serial_data_bits
        && !matches!(value, 5..=8)
    {
        return Err("serial data bits must be between 5 and 8".to_string());
    }
    for (label, field) in [
        (
            "Telnet username prompt",
            &edit.telnet_auto_login_username_prompt,
        ),
        (
            "Telnet password prompt",
            &edit.telnet_auto_login_password_prompt,
        ),
        (
            "Telnet success prompt",
            &edit.telnet_auto_login_success_prompt,
        ),
        (
            "Telnet failure prompt",
            &edit.telnet_auto_login_failure_prompt,
        ),
    ] {
        if let BatchField::Set(value) = field
            && !value.trim().is_empty()
            && regex::Regex::new(value).is_err()
        {
            return Err(format!("{label} is not a valid regular expression"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
