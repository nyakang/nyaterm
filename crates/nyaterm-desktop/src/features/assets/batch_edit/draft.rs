use super::basis::{
    BatchFieldApplicability, asset_batch_kind_counts, compute_batch_basis, compute_rdp_field_basis,
};
use super::fields::{BatchFieldId, EDITABLE_FIELDS, FieldBasis};
use gpui::{ScrollHandle, SharedString};
use nyaterm_core::SecretString;
use nyaterm_core::models::connection_batch::{
    BatchField, ConnectionBatchEdit, ConnectionBatchKind, validate_connection_batch_edit,
};
use nyaterm_core::models::{
    MAX_SFTP_SHELL_DETECTION_TIMEOUT_MS, MIN_SFTP_SHELL_DETECTION_TIMEOUT_MS, RecordingMode,
    SavedConnection, SerialFlowControl, SftpCwdFollowMode, SshAgentEndpoint,
    SshAgentForwardingPolicy, SshAlgorithmMode, SshProfile, SshTerminalType,
};
use nyaterm_ui::ChildWindowSlot;
use rust_i18n::t;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::features) enum BatchEditMode {
    #[default]
    Keep,
    Set,
    Clear,
}

#[derive(Clone, Default)]
pub(in crate::features) struct BatchScopeDraft {
    kind: Option<ConnectionBatchKind>,
    basis: HashMap<BatchFieldId, FieldBasis>,
    values: HashMap<BatchFieldId, String>,
    bool_values: HashMap<BatchFieldId, bool>,
    modes: HashMap<BatchFieldId, BatchEditMode>,
    original_values: HashMap<BatchFieldId, String>,
    original_bools: HashMap<BatchFieldId, bool>,
    password: Option<SecretString>,
    rdp_applicability: HashMap<BatchFieldId, BatchFieldApplicability>,
}

impl BatchScopeDraft {
    pub(in crate::features) fn rdp_applicability(
        &self,
        field: BatchFieldId,
    ) -> Option<BatchFieldApplicability> {
        self.rdp_applicability.get(&field).copied()
    }

    pub(in crate::features) fn field_available(&self, field: BatchFieldId) -> bool {
        self.rdp_applicability(field)
            .is_none_or(|state| state.applicable > 0)
            && (self.basis(field) != FieldBasis::Inapplicable || self.is_edited(field))
    }

    fn rdp_dependency_edit(&self) -> ConnectionBatchEdit {
        let field = |id| {
            if self.mode(id) == BatchEditMode::Set && self.value(id).trim().is_empty() {
                // A selector awaiting a choice has not changed the backend yet.
                BatchField::Unchanged
            } else {
                self.text_field(id, true)
            }
        };
        ConnectionBatchEdit {
            rdp_backend: field(BatchFieldId::RdpBackend),
            rdp_display_mode: field(BatchFieldId::RdpDisplayMode),
            ..Default::default()
        }
    }

    fn refresh_rdp_basis(
        &mut self,
        connections: &[SavedConnection],
        targets: &[(String, ConnectionBatchKind)],
    ) {
        let edit = self.rdp_dependency_edit();
        for field in EDITABLE_FIELDS
            .into_iter()
            .filter(|field| field.rdp_group().is_some())
        {
            let (basis, value, applicability) =
                compute_rdp_field_basis(connections, targets, field, &edit);
            self.basis.insert(field, basis);
            self.rdp_applicability.insert(field, applicability);
            self.original_values.remove(&field);
            self.original_bools.remove(&field);
            if let Some(value) = value {
                if field.is_bool() {
                    self.original_bools.insert(field, value == "true");
                } else {
                    self.original_values.insert(field, value);
                }
            }
            // Recomputing the applicable subset changes the Keep preview, never
            // an explicit user patch, including one that is temporarily dormant.
            if !self.is_edited(field) {
                self.values.remove(&field);
                self.bool_values.remove(&field);
                if let Some(value) = self.original_values.get(&field) {
                    self.values.insert(field, value.clone());
                }
                if let Some(value) = self.original_bools.get(&field) {
                    self.bool_values.insert(field, *value);
                }
            }
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
    pub(in crate::features) fn mode(&self, field: BatchFieldId) -> BatchEditMode {
        self.modes.get(&field).copied().unwrap_or_default()
    }
    pub(in crate::features) fn is_edited(&self, field: BatchFieldId) -> bool {
        self.mode(field) != BatchEditMode::Keep
    }
    fn display_kind(&self) -> Option<ConnectionBatchKind> {
        self.kind
    }
    pub(in crate::features) fn current_bool_is_set(&self, field: BatchFieldId) -> bool {
        self.bool_values.contains_key(&field)
    }
    pub(in crate::features) fn disabled_text(&self, field: BatchFieldId) -> String {
        if self.basis(field) == FieldBasis::Inapplicable {
            t!("assets.batch.inapplicable").to_string()
        } else if self.basis(field) == FieldBasis::Mixed {
            t!("assets.batch.mixed").to_string()
        } else if field.is_bool() {
            if self.bool_value(field) {
                t!("assets.batch.enabled").to_string()
            } else {
                t!("assets.batch.disabled").to_string()
            }
        } else {
            self.value(field).to_string()
        }
    }
    fn bool_field(&self, field: BatchFieldId) -> BatchField<bool> {
        match self.mode(field) {
            BatchEditMode::Keep => BatchField::Unchanged,
            BatchEditMode::Clear => BatchField::Clear,
            BatchEditMode::Set => BatchField::Set(self.bool_value(field)),
        }
    }
    fn text_field(&self, field: BatchFieldId, trim: bool) -> BatchField<String> {
        match self.mode(field) {
            BatchEditMode::Keep => BatchField::Unchanged,
            BatchEditMode::Clear => BatchField::Clear,
            BatchEditMode::Set => BatchField::Set(if trim {
                self.value(field).trim().to_string()
            } else {
                self.value(field).to_string()
            }),
        }
    }
    fn empty_is_clear(&self, field: BatchFieldId) -> bool {
        self.mode(field) == BatchEditMode::Clear
    }
    fn invalid_option(&self, field: BatchFieldId) -> String {
        t!("assets.batch.errorValue", field = t!(field.label_key())).to_string()
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
        Ok(BatchField::Set(
            self.require_text(field)?
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect(),
        ))
    }
    pub(in crate::features) fn build_edit(&self) -> Result<ConnectionBatchEdit, String> {
        let mut edit = ConnectionBatchEdit::default();

        for field in EDITABLE_FIELDS {
            if !self.is_edited(field) {
                continue;
            }
            if self.mode(field) == BatchEditMode::Clear {
                match field {
                    BatchFieldId::Group => edit.group_id = BatchField::Clear,
                    BatchFieldId::Description => edit.description = BatchField::Clear,
                    BatchFieldId::Icon => edit.icon = BatchField::Clear,
                    BatchFieldId::IconAutoDetect => edit.icon_auto_detect = BatchField::Clear,
                    BatchFieldId::Port => edit.port = BatchField::Clear,
                    BatchFieldId::Username => edit.username = BatchField::Clear,
                    BatchFieldId::Encoding => edit.encoding = BatchField::Clear,
                    BatchFieldId::Backspace => edit.backspace_mode = BatchField::Clear,
                    BatchFieldId::SshProfile => edit.ssh_profile = BatchField::Clear,
                    BatchFieldId::TerminalType => edit.terminal_type = BatchField::Clear,
                    BatchFieldId::DynamicTabTitle => edit.dynamic_tab_title = BatchField::Clear,
                    BatchFieldId::SftpEnabled => edit.sftp_enabled = BatchField::Clear,
                    BatchFieldId::SftpCompatibility => {
                        edit.sftp_compatibility_mode = BatchField::Clear
                    }
                    BatchFieldId::SftpCwdFollow => edit.sftp_cwd_follow_mode = BatchField::Clear,
                    BatchFieldId::SftpShellTimeout => {
                        edit.sftp_shell_detection_timeout_ms = BatchField::Clear
                    }
                    BatchFieldId::SftpFilenameEncoding => {
                        edit.sftp_filename_encoding = BatchField::Clear
                    }
                    BatchFieldId::SftpPipelineDepth => edit.sftp_pipeline_depth = BatchField::Clear,
                    BatchFieldId::X11 => edit.x11_forwarding = BatchField::Clear,
                    BatchFieldId::AgentForwarding => {
                        edit.agent_forwarding_enabled = BatchField::Clear
                    }
                    BatchFieldId::PostLoginEnabled => edit.post_login_enabled = BatchField::Clear,
                    BatchFieldId::PostLoginCommand => edit.post_login_command = BatchField::Clear,
                    BatchFieldId::PostLoginDelay => edit.post_login_delay_ms = BatchField::Clear,
                    BatchFieldId::SshAlgorithmMode => edit.ssh_algorithm_mode = BatchField::Clear,
                    BatchFieldId::SshKex => edit.ssh_kex = BatchField::Clear,
                    BatchFieldId::SshCiphers => edit.ssh_ciphers = BatchField::Clear,
                    BatchFieldId::SshMacs => edit.ssh_macs = BatchField::Clear,
                    BatchFieldId::SshHostKeys => edit.ssh_host_keys = BatchField::Clear,
                    BatchFieldId::RdpUseNla => edit.rdp_use_nla = BatchField::Clear,
                    BatchFieldId::RdpCertificatePolicy => {
                        edit.rdp_certificate_policy = BatchField::Clear
                    }
                    BatchFieldId::RdpAdminSession => {
                        edit.rdp_administrative_session = BatchField::Clear
                    }
                    BatchFieldId::VncSecurityMode => edit.vnc_security_mode = BatchField::Clear,
                    BatchFieldId::Tags => edit.tags = BatchField::Clear,
                    BatchFieldId::Account => {
                        if matches!(
                            self.kind,
                            Some(ConnectionBatchKind::Rdp | ConnectionBatchKind::Vnc)
                        ) {
                            edit.password_id = BatchField::Clear;
                        } else {
                            edit.account_id = BatchField::Clear;
                        }
                    }
                    BatchFieldId::Password => edit.password = BatchField::Clear,
                    BatchFieldId::AuthMode => edit.auth_mode = BatchField::Clear,
                    BatchFieldId::Otp => edit.otp_id = BatchField::Clear,
                    BatchFieldId::OtpAutoFill => edit.auto_fill_otp = BatchField::Clear,
                    BatchFieldId::Key => edit.key_id = BatchField::Clear,
                    BatchFieldId::SshAuthAgentEndpoint => {
                        edit.auth_agent_endpoint = BatchField::Clear
                    }
                    BatchFieldId::SshAuthAgentEnvironment => {
                        edit.auth_agent_endpoint = BatchField::Clear
                    }
                    BatchFieldId::SshAuthAgentSocket => {
                        edit.auth_agent_endpoint = BatchField::Clear
                    }
                    BatchFieldId::Proxy => edit.proxy_id = BatchField::Clear,
                    BatchFieldId::ProxyJump => edit.proxy_jump_id = BatchField::Clear,
                    BatchFieldId::AgentExternal => {
                        edit.agent_forwarding_external = BatchField::Clear
                    }
                    BatchFieldId::AgentStoredKeys => {
                        edit.agent_forwarding_stored_keys = BatchField::Clear
                    }
                    BatchFieldId::AgentPolicy => edit.agent_forwarding_policy = BatchField::Clear,
                    BatchFieldId::ShellPath => edit.shell_path = BatchField::Clear,
                    BatchFieldId::ShellArgs => edit.shell_args = BatchField::Clear,
                    BatchFieldId::WorkingDir => edit.working_dir = BatchField::Clear,
                    BatchFieldId::SerialPort => edit.serial_port_name = BatchField::Clear,
                    BatchFieldId::BaudRate => edit.serial_baud_rate = BatchField::Clear,
                    BatchFieldId::DataBits => edit.serial_data_bits = BatchField::Clear,
                    BatchFieldId::Parity => edit.serial_parity = BatchField::Clear,
                    BatchFieldId::StopBits => edit.serial_stop_bits = BatchField::Clear,
                    BatchFieldId::FlowControl => edit.serial_flow_control = BatchField::Clear,
                    BatchFieldId::TelnetRawTcp => edit.telnet_raw_tcp_cli = BatchField::Clear,
                    BatchFieldId::TelnetLocalEcho => edit.telnet_local_echo = BatchField::Clear,
                    BatchFieldId::TelnetLocalLineEdit => {
                        edit.telnet_local_line_edit = BatchField::Clear
                    }
                    BatchFieldId::TelnetForceChar => {
                        edit.telnet_force_character_at_a_time = BatchField::Clear
                    }
                    BatchFieldId::TelnetSendNaws => edit.telnet_send_naws = BatchField::Clear,
                    BatchFieldId::TelnetSendSga => edit.telnet_send_sga = BatchField::Clear,
                    BatchFieldId::TelnetKeepAlive => {
                        edit.telnet_keep_alive_interval_secs = BatchField::Clear
                    }
                    BatchFieldId::TelnetEnterMode => edit.telnet_enter_mode = BatchField::Clear,
                    BatchFieldId::TelnetAutoLoginEnabled => {
                        edit.telnet_auto_login_enabled = BatchField::Clear
                    }
                    BatchFieldId::TelnetAutoLoginWake => {
                        edit.telnet_auto_login_send_wake_enter = BatchField::Clear
                    }
                    BatchFieldId::TelnetAutoLoginTimeout => {
                        edit.telnet_auto_login_timeout_ms = BatchField::Clear
                    }
                    BatchFieldId::TelnetAutoLoginRetries => {
                        edit.telnet_auto_login_max_retries = BatchField::Clear
                    }
                    BatchFieldId::TelnetAutoLoginUserPrompt => {
                        edit.telnet_auto_login_username_prompt = BatchField::Clear
                    }
                    BatchFieldId::TelnetAutoLoginPasswordPrompt => {
                        edit.telnet_auto_login_password_prompt = BatchField::Clear
                    }
                    BatchFieldId::TelnetAutoLoginSuccessPrompt => {
                        edit.telnet_auto_login_success_prompt = BatchField::Clear
                    }
                    BatchFieldId::TelnetAutoLoginFailurePrompt => {
                        edit.telnet_auto_login_failure_prompt = BatchField::Clear
                    }
                    BatchFieldId::RdpDomain => edit.rdp_domain = BatchField::Clear,
                    BatchFieldId::RdpBackend => edit.rdp_backend = BatchField::Clear,
                    BatchFieldId::RdpDisplayMode => edit.rdp_display_mode = BatchField::Clear,
                    BatchFieldId::RdpWidth => edit.rdp_display_width = BatchField::Clear,
                    BatchFieldId::RdpHeight => edit.rdp_display_height = BatchField::Clear,
                    BatchFieldId::RdpWallpaper => edit.rdp_display_wallpaper = BatchField::Clear,
                    BatchFieldId::RdpThemes => edit.rdp_display_themes = BatchField::Clear,
                    BatchFieldId::RdpFontSmoothing => edit.rdp_font_smoothing = BatchField::Clear,
                    BatchFieldId::RdpDesktopComposition => {
                        edit.rdp_desktop_composition = BatchField::Clear
                    }
                    BatchFieldId::RdpDisableFullWindowDrag => {
                        edit.rdp_disable_full_window_drag = BatchField::Clear
                    }
                    BatchFieldId::RdpDisableMenuAnimations => {
                        edit.rdp_disable_menu_animations = BatchField::Clear
                    }
                    BatchFieldId::RdpDisableCursorShadow => {
                        edit.rdp_disable_cursor_shadow = BatchField::Clear
                    }
                    BatchFieldId::RdpDisableCursorBlinking => {
                        edit.rdp_disable_cursor_blinking = BatchField::Clear
                    }
                    BatchFieldId::RdpAutoReconnect => edit.rdp_auto_reconnect = BatchField::Clear,
                    BatchFieldId::RdpReconnectAttempts => {
                        edit.rdp_reconnect_attempts = BatchField::Clear
                    }
                    BatchFieldId::VncScaleMode => edit.vnc_scale_mode = BatchField::Clear,
                    BatchFieldId::VncClipboard => edit.vnc_clipboard = BatchField::Clear,
                    BatchFieldId::VncShared => edit.vnc_shared = BatchField::Clear,
                    BatchFieldId::VncViewOnly => edit.vnc_view_only = BatchField::Clear,
                    BatchFieldId::VncReconnect => edit.vnc_reconnect = BatchField::Clear,
                    BatchFieldId::VncReconnectAttempts => {
                        edit.vnc_reconnect_attempts = BatchField::Clear
                    }
                    BatchFieldId::RecordingUseGlobal => {
                        edit.recording_use_global = BatchField::Clear
                    }
                    BatchFieldId::RecordingAutoStart => {
                        edit.recording_auto_start = BatchField::Clear
                    }
                    BatchFieldId::RecordingMode => edit.recording_mode = BatchField::Clear,
                    _ => {}
                }
                continue;
            }
            if field.is_bool() {
                if !self.bool_values.contains_key(&field) {
                    return Err(t!("assets.batch.chooseBoolean").to_string());
                }
            } else if field == BatchFieldId::Password {
                if self.password.as_ref().is_none_or(SecretString::is_empty) {
                    return Err(
                        t!("assets.batch.errorUseClear", field = t!(field.label_key())).to_string(),
                    );
                }
            } else {
                self.require_text(field)?;
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
                    edit.encoding = self.text_field(field, true);
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
                            "standard" => SshProfile::Standard,
                            _ => return Err(self.invalid_option(field)),
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
                            "xterm-256color" => SshTerminalType::Xterm256Color,
                            _ => return Err(self.invalid_option(field)),
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
                            "shell_integration" | "shell-integration" => {
                                SftpCwdFollowMode::ShellIntegration
                            }
                            _ => return Err(self.invalid_option(field)),
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
                    edit.sftp_filename_encoding = self.text_field(field, true);
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
                            "compatible" => SshAlgorithmMode::Compatible,
                            _ => return Err(self.invalid_option(field)),
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
                    edit.password =
                        BatchField::Set(self.password.clone().expect("validated replacement"));
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
                        BatchField::Set(self.require_number::<u32>(field)?)
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
                        BatchField::Set(self.require_number::<u32>(field)?)
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
                        BatchField::Set(self.require_number::<u32>(field)?)
                    };
                }
                BatchFieldId::RdpHeight => {
                    edit.rdp_display_height = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        BatchField::Set(self.require_number::<u32>(field)?)
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
                        BatchField::Set(self.require_number::<u32>(field)?)
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
                        BatchField::Set(self.require_number::<u32>(field)?)
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
                        _ => return Err(self.invalid_option(field)),
                    };
                }
                // The loop walks EDITABLE_FIELDS, so the disabled display
                // fields are never reached and never enter a patch.
                _ => {}
            }
        }

        if !edit.is_empty() {
            validate_connection_batch_edit(&edit)?;
        }
        Ok(edit)
    }
}

/// Inactive scopes live only in `scopes`; switching moves the current draft rather than mirroring it.
#[derive(Clone, Default)]
pub(in crate::features) struct AssetBatchEditDraft {
    open: bool,
    ids: Vec<String>,
    saving: bool,
    error: Option<SharedString>,
    generation: u64,
    targets: Vec<(String, ConnectionBatchKind)>,
    counts: Vec<(ConnectionBatchKind, usize)>,
    current: BatchScopeDraft,
    scopes: HashMap<Option<ConnectionBatchKind>, BatchScopeDraft>,
    window: ChildWindowSlot,
    scroll: ScrollHandle,
}
impl std::ops::Deref for AssetBatchEditDraft {
    type Target = BatchScopeDraft;
    fn deref(&self) -> &Self::Target {
        &self.current
    }
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
    pub(in crate::features) fn set_saving(&mut self, value: bool) {
        self.saving = value;
    }
    pub(in crate::features) fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }
    pub(in crate::features) fn set_error(&mut self, value: Option<SharedString>) {
        self.error = value;
    }
    pub(in crate::features) fn generation(&self) -> u64 {
        self.generation
    }
    pub(in crate::features) fn matches_generation(&self, generation: u64) -> bool {
        self.open && self.generation == generation
    }
    pub(in crate::features) fn counts(&self) -> &[(ConnectionBatchKind, usize)] {
        &self.counts
    }
    pub(in crate::features) fn targets(&self) -> &[(String, ConnectionBatchKind)] {
        &self.targets
    }
    pub(in crate::features) fn display_kind(&self) -> Option<ConnectionBatchKind> {
        self.current.kind
    }
    pub(in crate::features) fn set_scope(&mut self, kind: Option<ConnectionBatchKind>) -> bool {
        if self.current.kind == kind
            || (kind.is_some() && !self.counts.iter().any(|(k, _)| Some(*k) == kind))
        {
            return false;
        }
        let next = self
            .scopes
            .remove(&kind)
            .expect("all scopes initialized at open");
        let old = std::mem::replace(&mut self.current, next);
        self.scopes.insert(old.kind, old);
        true
    }
    #[cfg(test)]
    pub(in crate::features) fn set_kind(&mut self, kind: ConnectionBatchKind) -> bool {
        self.set_scope(Some(kind))
    }
    pub(in crate::features) fn has_edits(&self) -> bool {
        self.current
            .modes
            .values()
            .chain(self.scopes.values().flat_map(|scope| scope.modes.values()))
            .any(|mode| *mode != BatchEditMode::Keep)
    }
    pub(in crate::features) fn set_mode(&mut self, field: BatchFieldId, mode: BatchEditMode) {
        if self.saving
            || (mode != BatchEditMode::Keep
                && self
                    .current
                    .rdp_applicability(field)
                    .is_some_and(|state| state.applicable == 0))
        {
            return;
        }
        self.current.modes.insert(field, mode);
        if mode == BatchEditMode::Keep {
            if let Some(value) = self.current.original_values.get(&field) {
                self.current.values.insert(field, value.clone());
            } else {
                self.current.values.remove(&field);
            }
            if let Some(value) = self.current.original_bools.get(&field) {
                self.current.bool_values.insert(field, *value);
            } else {
                self.current.bool_values.remove(&field);
            }
        }
        if field == BatchFieldId::Password && mode != BatchEditMode::Set {
            self.current.password = None;
        }
    }
    pub(in crate::features) fn replacement_password(&self) -> &str {
        self.current
            .password
            .as_ref()
            .map(SecretString::expose_secret)
            .unwrap_or("")
    }
    pub(in crate::features) fn set_value(&mut self, field: BatchFieldId, value: String) {
        if self.saving
            || self.mode(field) != BatchEditMode::Set
            || self
                .current
                .rdp_applicability(field)
                .is_some_and(|state| state.applicable == 0)
        {
            return;
        }
        if field == BatchFieldId::Password {
            self.current.password = Some(SecretString::new(value));
        } else {
            self.current.values.insert(field, value);
        }
    }
    pub(in crate::features) fn set_bool_value(&mut self, field: BatchFieldId, value: bool) {
        if !self.saving
            && self.mode(field) == BatchEditMode::Set
            && self
                .current
                .rdp_applicability(field)
                .is_none_or(|state| state.applicable > 0)
        {
            self.current.bool_values.insert(field, value);
        }
    }
    pub(in crate::features) fn clear(&mut self) {
        let generation = self.generation.wrapping_add(1);
        *self = Self {
            generation,
            ..Default::default()
        };
    }
    pub(in crate::features) fn start(
        &mut self,
        ids: Vec<String>,
        basis: HashMap<BatchFieldId, FieldBasis>,
        values: HashMap<BatchFieldId, String>,
        bool_values: HashMap<BatchFieldId, bool>,
    ) {
        let generation = self.generation.wrapping_add(1);
        *self = Self {
            open: true,
            ids,
            generation,
            current: BatchScopeDraft {
                original_values: values.clone(),
                original_bools: bool_values.clone(),
                basis,
                values,
                bool_values,
                ..Default::default()
            },
            ..Default::default()
        };
    }
    pub(in crate::features) fn initialize_scopes(&mut self, connections: &[SavedConnection]) {
        self.targets = connections
            .iter()
            .filter(|connection| self.ids.contains(&connection.id))
            .map(|connection| {
                (
                    connection.id.clone(),
                    ConnectionBatchKind::from_config(&connection.config),
                )
            })
            .collect();
        self.counts = asset_batch_kind_counts(connections, &self.ids);
        for &(kind, _) in &self.counts {
            let (basis, values, bool_values) =
                compute_batch_basis(connections, &self.ids, Some(kind));
            self.scopes.insert(
                Some(kind),
                BatchScopeDraft {
                    kind: Some(kind),
                    original_values: values.clone(),
                    original_bools: bool_values.clone(),
                    basis,
                    values,
                    bool_values,
                    ..Default::default()
                },
            );
        }
        self.refresh_rdp_basis(connections);
    }
    pub(in crate::features) fn refresh_rdp_basis(&mut self, connections: &[SavedConnection]) {
        if self.current.kind == Some(ConnectionBatchKind::Rdp) {
            self.current.refresh_rdp_basis(connections, &self.targets);
        } else if let Some(scope) = self.scopes.get_mut(&Some(ConnectionBatchKind::Rdp)) {
            scope.refresh_rdp_basis(connections, &self.targets);
        }
    }
    pub(in crate::features) fn build_edits(
        &self,
    ) -> Result<HashMap<Option<ConnectionBatchKind>, ConnectionBatchEdit>, String> {
        let mut edits = HashMap::new();
        for scope in std::iter::once(&self.current).chain(self.scopes.values()) {
            let edit = scope.build_edit()?;
            if !edit.is_empty() {
                edits.insert(scope.kind, edit);
            }
        }
        if edits.is_empty() {
            return Err(t!("assets.batch.errorNoFields").to_string());
        }
        Ok(edits)
    }
    pub(in crate::features) fn summary(&self) -> (usize, usize) {
        let mut connections = 0;
        let mut fields = 0;
        for scope in std::iter::once(&self.current).chain(self.scopes.values()) {
            let count = scope
                .modes
                .values()
                .filter(|mode| **mode != BatchEditMode::Keep)
                .count();
            fields += count;
        }
        for (_, kind) in &self.targets {
            if std::iter::once(&self.current)
                .chain(self.scopes.values())
                .any(|scope| {
                    (scope.kind.is_none() || scope.kind == Some(*kind))
                        && scope
                            .modes
                            .values()
                            .any(|mode| *mode != BatchEditMode::Keep)
                })
            {
                connections += 1;
            }
        }
        (connections, fields)
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
}
