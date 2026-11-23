use super::basis::{
    BatchFieldApplicability, asset_batch_kind_counts, compute_batch_basis_indexed,
    compute_rdp_field_basis,
};
use super::fields::{BatchFieldKey, EDITABLE_FIELDS, FieldBasis};
use crate::models::ConnectionEditorSection;
use gpui::{FocusHandle, ScrollHandle, SharedString};
use nyaterm_core::SecretString;
use nyaterm_core::models::android::{CaptureOrientation, ScreenQualityPreset};
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
    basis: HashMap<BatchFieldKey, FieldBasis>,
    values: HashMap<BatchFieldKey, String>,
    bool_values: HashMap<BatchFieldKey, bool>,
    modes: HashMap<BatchFieldKey, BatchEditMode>,
    original_values: HashMap<BatchFieldKey, String>,
    original_bools: HashMap<BatchFieldKey, bool>,
    password: Option<SecretString>,
    rdp_applicability: HashMap<BatchFieldKey, BatchFieldApplicability>,
}

impl BatchScopeDraft {
    pub(in crate::features) fn rdp_applicability(
        &self,
        field: BatchFieldKey,
    ) -> Option<BatchFieldApplicability> {
        self.rdp_applicability.get(&field).copied()
    }

    pub(in crate::features) fn field_available(&self, field: BatchFieldKey) -> bool {
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
            rdp_backend: field(BatchFieldKey::RdpBackend),
            rdp_display_mode: field(BatchFieldKey::RdpDisplayMode),
            ..Default::default()
        }
    }

    fn refresh_rdp_basis(
        &mut self,
        connections: &[SavedConnection],
        targets: &[(String, ConnectionBatchKind)],
    ) {
        let edit = self.rdp_dependency_edit();
        let by_id = connections
            .iter()
            .map(|connection| (connection.id.as_str(), connection))
            .collect();
        for field in EDITABLE_FIELDS
            .into_iter()
            .filter(|field| field.rdp_group().is_some())
        {
            let (basis, value, applicability) =
                compute_rdp_field_basis(&by_id, targets, field, &edit);
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

    pub(in crate::features) fn basis(&self, field: BatchFieldKey) -> FieldBasis {
        self.basis.get(&field).copied().unwrap_or_default()
    }
    pub(in crate::features) fn value(&self, field: BatchFieldKey) -> &str {
        self.values.get(&field).map(String::as_str).unwrap_or("")
    }
    pub(in crate::features) fn bool_value(&self, field: BatchFieldKey) -> bool {
        self.bool_values.get(&field).copied().unwrap_or(false)
    }
    pub(in crate::features) fn mode(&self, field: BatchFieldKey) -> BatchEditMode {
        self.modes.get(&field).copied().unwrap_or_default()
    }
    pub(in crate::features) fn is_edited(&self, field: BatchFieldKey) -> bool {
        self.mode(field) != BatchEditMode::Keep
    }
    fn display_kind(&self) -> Option<ConnectionBatchKind> {
        self.kind
    }
    pub(in crate::features) fn current_bool_is_set(&self, field: BatchFieldKey) -> bool {
        self.bool_values.contains_key(&field)
    }
    pub(in crate::features) fn disabled_text(&self, field: BatchFieldKey) -> String {
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
    fn bool_field(&self, field: BatchFieldKey) -> BatchField<bool> {
        match self.mode(field) {
            BatchEditMode::Keep => BatchField::Unchanged,
            BatchEditMode::Clear => BatchField::Clear,
            BatchEditMode::Set => BatchField::Set(self.bool_value(field)),
        }
    }
    fn text_field(&self, field: BatchFieldKey, trim: bool) -> BatchField<String> {
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
    fn empty_is_clear(&self, field: BatchFieldKey) -> bool {
        self.mode(field) == BatchEditMode::Clear
    }
    fn invalid_option(&self, field: BatchFieldKey) -> String {
        t!("assets.batch.errorValue", field = t!(field.label_key())).to_string()
    }
    fn require_text(&self, field: BatchFieldKey) -> Result<String, String> {
        let value = self.value(field).trim();
        if value.is_empty() {
            return Err(
                t!("assets.batch.errorUseClear", field = t!(field.label_key())).to_string(),
            );
        }
        Ok(value.to_string())
    }
    fn require_number<T: std::str::FromStr>(&self, field: BatchFieldKey) -> Result<T, String> {
        self.value(field)
            .trim()
            .parse::<T>()
            .map_err(|_| t!("assets.batch.errorNumber", field = t!(field.label_key())).to_string())
    }
    fn list_field(&self, field: BatchFieldKey) -> Result<BatchField<Vec<String>>, String> {
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
                    BatchFieldKey::Group => edit.group_id = BatchField::Clear,
                    BatchFieldKey::Description => edit.description = BatchField::Clear,
                    BatchFieldKey::Icon => edit.icon = BatchField::Clear,
                    BatchFieldKey::IconAutoDetect => edit.icon_auto_detect = BatchField::Clear,
                    BatchFieldKey::Port => edit.port = BatchField::Clear,
                    BatchFieldKey::Username => edit.username = BatchField::Clear,
                    BatchFieldKey::Encoding => edit.encoding = BatchField::Clear,
                    BatchFieldKey::Backspace => edit.backspace_mode = BatchField::Clear,
                    BatchFieldKey::SshProfile => edit.ssh_profile = BatchField::Clear,
                    BatchFieldKey::TerminalType => edit.terminal_type = BatchField::Clear,
                    BatchFieldKey::SshCompatibility => {
                        edit.ssh_compatibility_mode = BatchField::Clear
                    }
                    BatchFieldKey::DynamicTabTitle => edit.dynamic_tab_title = BatchField::Clear,
                    BatchFieldKey::SftpEnabled => edit.sftp_enabled = BatchField::Clear,
                    BatchFieldKey::SftpCompatibility => {
                        edit.sftp_compatibility_mode = BatchField::Clear
                    }
                    BatchFieldKey::SftpCwdFollow => edit.sftp_cwd_follow_mode = BatchField::Clear,
                    BatchFieldKey::SftpShellTimeout => {
                        edit.sftp_shell_detection_timeout_ms = BatchField::Clear
                    }
                    BatchFieldKey::SftpFilenameEncoding => {
                        edit.sftp_filename_encoding = BatchField::Clear
                    }
                    BatchFieldKey::SftpPipelineDepth => {
                        edit.sftp_pipeline_depth = BatchField::Clear
                    }
                    BatchFieldKey::X11 => edit.x11_forwarding = BatchField::Clear,
                    BatchFieldKey::AgentForwarding => {
                        edit.agent_forwarding_enabled = BatchField::Clear
                    }
                    BatchFieldKey::PostLoginEnabled => edit.post_login_enabled = BatchField::Clear,
                    BatchFieldKey::PostLoginCommand => edit.post_login_command = BatchField::Clear,
                    BatchFieldKey::PostLoginDelay => edit.post_login_delay_ms = BatchField::Clear,
                    BatchFieldKey::SshAlgorithmMode => edit.ssh_algorithm_mode = BatchField::Clear,
                    BatchFieldKey::SshKex => edit.ssh_kex = BatchField::Clear,
                    BatchFieldKey::SshCiphers => edit.ssh_ciphers = BatchField::Clear,
                    BatchFieldKey::SshMacs => edit.ssh_macs = BatchField::Clear,
                    BatchFieldKey::SshHostKeys => edit.ssh_host_keys = BatchField::Clear,
                    BatchFieldKey::AllowShortRsaHostKeys => {
                        edit.allow_short_rsa_host_keys = BatchField::Clear
                    }
                    BatchFieldKey::RdpUseNla => edit.rdp_use_nla = BatchField::Clear,
                    BatchFieldKey::RdpCertificatePolicy => {
                        edit.rdp_certificate_policy = BatchField::Clear
                    }
                    BatchFieldKey::RdpAdminSession => {
                        edit.rdp_administrative_session = BatchField::Clear
                    }
                    BatchFieldKey::VncSecurityMode => edit.vnc_security_mode = BatchField::Clear,
                    BatchFieldKey::Tags => edit.tags = BatchField::Clear,
                    BatchFieldKey::Account => {
                        if matches!(
                            self.kind,
                            Some(ConnectionBatchKind::Rdp | ConnectionBatchKind::Vnc)
                        ) {
                            edit.password_id = BatchField::Clear;
                        } else {
                            edit.account_id = BatchField::Clear;
                        }
                    }
                    BatchFieldKey::Password => edit.password = BatchField::Clear,
                    BatchFieldKey::AuthMode => edit.auth_mode = BatchField::Clear,
                    BatchFieldKey::Otp => edit.otp_id = BatchField::Clear,
                    BatchFieldKey::OtpAutoFill => edit.auto_fill_otp = BatchField::Clear,
                    BatchFieldKey::Key => edit.key_id = BatchField::Clear,
                    BatchFieldKey::SshAuthAgentEndpoint => {
                        edit.auth_agent_endpoint = BatchField::Clear
                    }
                    BatchFieldKey::SshAuthAgentEnvironment => {
                        edit.auth_agent_endpoint = BatchField::Clear
                    }
                    BatchFieldKey::SshAuthAgentSocket => {
                        edit.auth_agent_endpoint = BatchField::Clear
                    }
                    BatchFieldKey::Proxy => edit.proxy_id = BatchField::Clear,
                    BatchFieldKey::ProxyJump => edit.proxy_jump_id = BatchField::Clear,
                    BatchFieldKey::AgentExternal => {
                        edit.agent_forwarding_external = BatchField::Clear
                    }
                    BatchFieldKey::AgentStoredKeys => {
                        edit.agent_forwarding_stored_keys = BatchField::Clear
                    }
                    BatchFieldKey::AgentPolicy => edit.agent_forwarding_policy = BatchField::Clear,
                    BatchFieldKey::ShellPath => edit.shell_path = BatchField::Clear,
                    BatchFieldKey::ShellArgs => edit.shell_args = BatchField::Clear,
                    BatchFieldKey::WorkingDir => edit.working_dir = BatchField::Clear,
                    BatchFieldKey::SerialPort => edit.serial_port_name = BatchField::Clear,
                    BatchFieldKey::BaudRate => edit.serial_baud_rate = BatchField::Clear,
                    BatchFieldKey::DataBits => edit.serial_data_bits = BatchField::Clear,
                    BatchFieldKey::Parity => edit.serial_parity = BatchField::Clear,
                    BatchFieldKey::StopBits => edit.serial_stop_bits = BatchField::Clear,
                    BatchFieldKey::FlowControl => edit.serial_flow_control = BatchField::Clear,
                    BatchFieldKey::TelnetRawTcp => edit.telnet_raw_tcp_cli = BatchField::Clear,
                    BatchFieldKey::TelnetLocalEcho => edit.telnet_local_echo = BatchField::Clear,
                    BatchFieldKey::TelnetLocalLineEdit => {
                        edit.telnet_local_line_edit = BatchField::Clear
                    }
                    BatchFieldKey::TelnetForceChar => {
                        edit.telnet_force_character_at_a_time = BatchField::Clear
                    }
                    BatchFieldKey::TelnetSendNaws => edit.telnet_send_naws = BatchField::Clear,
                    BatchFieldKey::TelnetSendSga => edit.telnet_send_sga = BatchField::Clear,
                    BatchFieldKey::TelnetKeepAlive => {
                        edit.telnet_keep_alive_interval_secs = BatchField::Clear
                    }
                    BatchFieldKey::TelnetEnterMode => edit.telnet_enter_mode = BatchField::Clear,
                    BatchFieldKey::TelnetAutoLoginEnabled => {
                        edit.telnet_auto_login_enabled = BatchField::Clear
                    }
                    BatchFieldKey::TelnetAutoLoginWake => {
                        edit.telnet_auto_login_send_wake_enter = BatchField::Clear
                    }
                    BatchFieldKey::TelnetAutoLoginTimeout => {
                        edit.telnet_auto_login_timeout_ms = BatchField::Clear
                    }
                    BatchFieldKey::TelnetAutoLoginRetries => {
                        edit.telnet_auto_login_max_retries = BatchField::Clear
                    }
                    BatchFieldKey::TelnetAutoLoginUserPrompt => {
                        edit.telnet_auto_login_username_prompt = BatchField::Clear
                    }
                    BatchFieldKey::TelnetAutoLoginPasswordPrompt => {
                        edit.telnet_auto_login_password_prompt = BatchField::Clear
                    }
                    BatchFieldKey::TelnetAutoLoginSuccessPrompt => {
                        edit.telnet_auto_login_success_prompt = BatchField::Clear
                    }
                    BatchFieldKey::TelnetAutoLoginFailurePrompt => {
                        edit.telnet_auto_login_failure_prompt = BatchField::Clear
                    }
                    BatchFieldKey::RdpDomain => edit.rdp_domain = BatchField::Clear,
                    BatchFieldKey::RdpBackend => edit.rdp_backend = BatchField::Clear,
                    BatchFieldKey::RdpDisplayMode => edit.rdp_display_mode = BatchField::Clear,
                    BatchFieldKey::RdpWidth => edit.rdp_display_width = BatchField::Clear,
                    BatchFieldKey::RdpHeight => edit.rdp_display_height = BatchField::Clear,
                    BatchFieldKey::RdpWallpaper => edit.rdp_display_wallpaper = BatchField::Clear,
                    BatchFieldKey::RdpThemes => edit.rdp_display_themes = BatchField::Clear,
                    BatchFieldKey::RdpFontSmoothing => edit.rdp_font_smoothing = BatchField::Clear,
                    BatchFieldKey::RdpDesktopComposition => {
                        edit.rdp_desktop_composition = BatchField::Clear
                    }
                    BatchFieldKey::RdpDisableFullWindowDrag => {
                        edit.rdp_disable_full_window_drag = BatchField::Clear
                    }
                    BatchFieldKey::RdpDisableMenuAnimations => {
                        edit.rdp_disable_menu_animations = BatchField::Clear
                    }
                    BatchFieldKey::RdpDisableCursorShadow => {
                        edit.rdp_disable_cursor_shadow = BatchField::Clear
                    }
                    BatchFieldKey::RdpDisableCursorBlinking => {
                        edit.rdp_disable_cursor_blinking = BatchField::Clear
                    }
                    BatchFieldKey::RdpAutoReconnect => edit.rdp_auto_reconnect = BatchField::Clear,
                    BatchFieldKey::RdpReconnectAttempts => {
                        edit.rdp_reconnect_attempts = BatchField::Clear
                    }
                    BatchFieldKey::RdpRedirectClipboard => {
                        edit.rdp_redirect_clipboard = BatchField::Clear
                    }
                    BatchFieldKey::RdpRedirectDrives => {
                        edit.rdp_redirect_drives = BatchField::Clear
                    }
                    BatchFieldKey::RdpDriveLetters => edit.rdp_drive_letters = BatchField::Clear,
                    BatchFieldKey::RdpCustomDrivePath => {
                        edit.rdp_custom_drive_path = BatchField::Clear
                    }
                    BatchFieldKey::RdpRedirectPrinters => {
                        edit.rdp_redirect_printers = BatchField::Clear
                    }
                    BatchFieldKey::RdpRedirectPorts => edit.rdp_redirect_ports = BatchField::Clear,
                    BatchFieldKey::RdpRedirectSmartcards => {
                        edit.rdp_redirect_smartcards = BatchField::Clear
                    }
                    BatchFieldKey::RdpRedirectMicrophone => {
                        edit.rdp_redirect_microphone = BatchField::Clear
                    }
                    BatchFieldKey::RdpRedirectWebauthn => {
                        edit.rdp_redirect_webauthn = BatchField::Clear
                    }
                    BatchFieldKey::RdpAudio => edit.rdp_audio = BatchField::Clear,
                    BatchFieldKey::VncScaleMode => edit.vnc_scale_mode = BatchField::Clear,
                    BatchFieldKey::VncClipboard => edit.vnc_clipboard = BatchField::Clear,
                    BatchFieldKey::VncShared => edit.vnc_shared = BatchField::Clear,
                    BatchFieldKey::VncViewOnly => edit.vnc_view_only = BatchField::Clear,
                    BatchFieldKey::VncReconnect => edit.vnc_reconnect = BatchField::Clear,
                    BatchFieldKey::VncReconnectAttempts => {
                        edit.vnc_reconnect_attempts = BatchField::Clear
                    }
                    BatchFieldKey::RecordingUseGlobal => {
                        edit.recording_use_global = BatchField::Clear
                    }
                    BatchFieldKey::RecordingAutoStart => {
                        edit.recording_auto_start = BatchField::Clear
                    }
                    BatchFieldKey::RecordingMode => edit.recording_mode = BatchField::Clear,
                    BatchFieldKey::AdbDirectory => edit.adb_directory = BatchField::Clear,
                    BatchFieldKey::AdbScreenQuality => edit.adb_screen_quality = BatchField::Clear,
                    BatchFieldKey::AdbScreenMaxSize => edit.adb_screen_max_size = BatchField::Clear,
                    BatchFieldKey::AdbScreenFps => edit.adb_screen_max_fps = BatchField::Clear,
                    BatchFieldKey::AdbScreenBitRate => edit.adb_screen_bit_rate = BatchField::Clear,
                    BatchFieldKey::AdbScreenPowerOn => edit.adb_screen_power_on = BatchField::Clear,
                    BatchFieldKey::AdbScreenOrientation => {
                        edit.adb_screen_orientation = BatchField::Clear
                    }
                    BatchFieldKey::FtpInitialDirectory => {
                        edit.ftp_initial_directory = BatchField::Clear
                    }
                    BatchFieldKey::FtpLocalInitialDirectory => {
                        edit.ftp_local_initial_directory = BatchField::Clear
                    }
                    BatchFieldKey::FtpPassiveMode => edit.ftp_passive_mode = BatchField::Clear,
                    BatchFieldKey::FtpFilenameEncodingMode => {
                        edit.ftp_filename_encoding_mode = BatchField::Clear
                    }
                    BatchFieldKey::FtpFilenameEncoding => {
                        edit.ftp_filename_encoding = BatchField::Clear
                    }
                    BatchFieldKey::FtpAsciiMode => edit.ftp_ascii_mode = BatchField::Clear,
                    BatchFieldKey::FtpSecurityMode => edit.ftp_security_mode = BatchField::Clear,
                    BatchFieldKey::FtpTlsVersion => edit.ftp_tls_version = BatchField::Clear,
                    BatchFieldKey::FtpConnectTimeout => {
                        edit.ftp_connect_timeout_ms = BatchField::Clear
                    }
                    _ => {}
                }
                continue;
            }
            if field.is_bool() {
                if !self.bool_values.contains_key(&field) {
                    return Err(t!("assets.batch.chooseBoolean").to_string());
                }
            } else if field == BatchFieldKey::Password {
                if self.password.as_ref().is_none_or(SecretString::is_empty) {
                    return Err(
                        t!("assets.batch.errorUseClear", field = t!(field.label_key())).to_string(),
                    );
                }
            } else {
                self.require_text(field)?;
            }
            match field {
                BatchFieldKey::Group => {
                    let value = self.value(field).trim();
                    edit.group_id = if value.is_empty() {
                        BatchField::Clear
                    } else {
                        BatchField::Set(value.to_string())
                    };
                }
                BatchFieldKey::Description => {
                    edit.description = self.text_field(field, true);
                }
                BatchFieldKey::Icon => {
                    edit.icon = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        BatchField::Set(self.value(field).to_string())
                    };
                }
                BatchFieldKey::IconAutoDetect => {
                    edit.icon_auto_detect = self.bool_field(field);
                }
                BatchFieldKey::Port => {
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
                BatchFieldKey::Username => {
                    edit.username = self.text_field(field, false);
                }
                BatchFieldKey::Encoding => {
                    edit.encoding = self.text_field(field, true);
                }
                BatchFieldKey::Backspace => {
                    edit.backspace_mode = self.text_field(field, true);
                }
                BatchFieldKey::SshProfile => {
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
                BatchFieldKey::TerminalType => {
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
                BatchFieldKey::SshCompatibility => {
                    edit.ssh_compatibility_mode = self.bool_field(field);
                }
                BatchFieldKey::DynamicTabTitle => {
                    edit.dynamic_tab_title = self.bool_field(field);
                }
                BatchFieldKey::SftpEnabled => {
                    edit.sftp_enabled = self.bool_field(field);
                }
                BatchFieldKey::SftpCompatibility => {
                    edit.sftp_compatibility_mode = self.bool_field(field);
                }
                BatchFieldKey::SftpCwdFollow => {
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
                BatchFieldKey::SftpShellTimeout => {
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
                BatchFieldKey::SftpFilenameEncoding => {
                    edit.sftp_filename_encoding = self.text_field(field, true);
                }
                BatchFieldKey::SftpPipelineDepth => {
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
                BatchFieldKey::X11 => {
                    edit.x11_forwarding = self.bool_field(field);
                }
                BatchFieldKey::AgentForwarding => {
                    edit.agent_forwarding_enabled = self.bool_field(field);
                }
                BatchFieldKey::PostLoginEnabled => {
                    edit.post_login_enabled = self.bool_field(field);
                }
                BatchFieldKey::PostLoginCommand => {
                    edit.post_login_command = self.text_field(field, true);
                }
                BatchFieldKey::PostLoginDelay => {
                    edit.post_login_delay_ms = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        BatchField::Set(self.require_number::<u64>(field)?)
                    };
                }
                BatchFieldKey::SshAlgorithmMode => {
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
                BatchFieldKey::SshKex => {
                    edit.ssh_kex = self.list_field(field)?;
                }
                BatchFieldKey::SshCiphers => {
                    edit.ssh_ciphers = self.list_field(field)?;
                }
                BatchFieldKey::SshMacs => {
                    edit.ssh_macs = self.list_field(field)?;
                }
                BatchFieldKey::SshHostKeys => {
                    edit.ssh_host_keys = self.list_field(field)?;
                }
                BatchFieldKey::AllowShortRsaHostKeys => {
                    edit.allow_short_rsa_host_keys = self.bool_field(field);
                }
                BatchFieldKey::RdpUseNla => {
                    edit.rdp_use_nla = self.bool_field(field);
                }
                BatchFieldKey::RdpCertificatePolicy => {
                    edit.rdp_certificate_policy = self.text_field(field, true);
                }
                BatchFieldKey::RdpAdminSession => {
                    edit.rdp_administrative_session = self.bool_field(field);
                }
                BatchFieldKey::VncSecurityMode => {
                    edit.vnc_security_mode = self.text_field(field, true);
                }
                BatchFieldKey::Tags => {
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
                BatchFieldKey::Account => {
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
                BatchFieldKey::Password => {
                    edit.password =
                        BatchField::Set(self.password.clone().expect("validated replacement"));
                }
                BatchFieldKey::AuthMode => {
                    edit.auth_mode = self.text_field(field, true);
                }
                BatchFieldKey::Otp => {
                    edit.otp_id = self.text_field(field, true);
                }
                BatchFieldKey::OtpAutoFill => {
                    edit.auto_fill_otp = self.bool_field(field);
                }
                BatchFieldKey::Key => {
                    edit.key_id = self.text_field(field, true);
                }
                BatchFieldKey::SshAuthAgentEndpoint
                | BatchFieldKey::SshAuthAgentEnvironment
                | BatchFieldKey::SshAuthAgentSocket => {
                    if self.is_edited(BatchFieldKey::SshAuthAgentEndpoint) {
                        let endpoint = self.value(BatchFieldKey::SshAuthAgentEndpoint).trim();
                        edit.auth_agent_endpoint = match endpoint {
                            "environment" => BatchField::Set(SshAgentEndpoint::Environment {
                                variable: self
                                    .value(BatchFieldKey::SshAuthAgentEnvironment)
                                    .trim()
                                    .to_string(),
                            }),
                            "unix_socket" => BatchField::Set(SshAgentEndpoint::UnixSocket {
                                path: self
                                    .value(BatchFieldKey::SshAuthAgentSocket)
                                    .trim()
                                    .to_string(),
                            }),
                            "auto" => BatchField::Set(SshAgentEndpoint::Auto),
                            "pageant" => BatchField::Set(SshAgentEndpoint::Pageant),
                            "windows_openssh" => BatchField::Set(SshAgentEndpoint::WindowsOpenSsh),
                            _ => BatchField::Clear,
                        };
                    } else if self.is_edited(BatchFieldKey::SshAuthAgentEnvironment) {
                        let variable = self.value(BatchFieldKey::SshAuthAgentEnvironment).trim();
                        edit.auth_agent_endpoint = if variable.is_empty() {
                            BatchField::Clear
                        } else {
                            BatchField::Set(SshAgentEndpoint::Environment {
                                variable: variable.to_string(),
                            })
                        };
                    } else if self.is_edited(BatchFieldKey::SshAuthAgentSocket) {
                        let path = self.value(BatchFieldKey::SshAuthAgentSocket).trim();
                        edit.auth_agent_endpoint = if path.is_empty() {
                            BatchField::Clear
                        } else {
                            BatchField::Set(SshAgentEndpoint::UnixSocket {
                                path: path.to_string(),
                            })
                        };
                    }
                }
                BatchFieldKey::Proxy => {
                    edit.proxy_id = self.text_field(field, true);
                }
                BatchFieldKey::ProxyJump => {
                    edit.proxy_jump_id = self.text_field(field, true);
                }
                BatchFieldKey::AgentExternal => {
                    edit.agent_forwarding_external = self.bool_field(field);
                }
                BatchFieldKey::AgentStoredKeys => {
                    edit.agent_forwarding_stored_keys = self.bool_field(field);
                }
                BatchFieldKey::AgentPolicy => {
                    edit.agent_forwarding_policy = match self.value(field).trim() {
                        "all" => BatchField::Set(SshAgentForwardingPolicy::All),
                        "allowlist" => BatchField::Set(SshAgentForwardingPolicy::Allowlist {
                            fingerprints: Vec::new(),
                        }),
                        _ => BatchField::Clear,
                    };
                }
                BatchFieldKey::ShellPath => {
                    edit.shell_path = self.text_field(field, true);
                }
                BatchFieldKey::ShellArgs => {
                    edit.shell_args = self.text_field(field, true);
                }
                BatchFieldKey::WorkingDir => {
                    edit.working_dir = self.text_field(field, true);
                }
                BatchFieldKey::SerialPort => {
                    edit.serial_port_name = self.text_field(field, true);
                }
                BatchFieldKey::BaudRate => {
                    edit.serial_baud_rate = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        BatchField::Set(self.require_number::<u32>(field)?)
                    };
                }
                BatchFieldKey::DataBits => {
                    edit.serial_data_bits = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        BatchField::Set(self.value(field).trim().parse::<u8>().unwrap_or(0))
                    };
                }
                BatchFieldKey::Parity => {
                    edit.serial_parity = self.text_field(field, true);
                }
                BatchFieldKey::StopBits => {
                    edit.serial_stop_bits = self.text_field(field, true);
                }
                BatchFieldKey::FlowControl => {
                    edit.serial_flow_control = match self.value(field).trim() {
                        "hardware" => BatchField::Set(SerialFlowControl::Hardware),
                        "software" => BatchField::Set(SerialFlowControl::Software),
                        "none" if self.is_edited(field) => BatchField::Set(SerialFlowControl::None),
                        _ => BatchField::Clear,
                    };
                }
                BatchFieldKey::TelnetRawTcp => {
                    edit.telnet_raw_tcp_cli = self.bool_field(field);
                }
                BatchFieldKey::TelnetLocalEcho => {
                    edit.telnet_local_echo = self.bool_field(field);
                }
                BatchFieldKey::TelnetLocalLineEdit => {
                    edit.telnet_local_line_edit = self.bool_field(field);
                }
                BatchFieldKey::TelnetForceChar => {
                    edit.telnet_force_character_at_a_time = self.bool_field(field);
                }
                BatchFieldKey::TelnetSendNaws => {
                    edit.telnet_send_naws = self.bool_field(field);
                }
                BatchFieldKey::TelnetSendSga => {
                    edit.telnet_send_sga = self.bool_field(field);
                }
                BatchFieldKey::TelnetKeepAlive => {
                    edit.telnet_keep_alive_interval_secs = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        BatchField::Set(self.require_number::<u32>(field)?)
                    };
                }
                BatchFieldKey::TelnetEnterMode => {
                    edit.telnet_enter_mode = self.text_field(field, true);
                }
                BatchFieldKey::TelnetAutoLoginEnabled => {
                    edit.telnet_auto_login_enabled = self.bool_field(field);
                }
                BatchFieldKey::TelnetAutoLoginWake => {
                    edit.telnet_auto_login_send_wake_enter = self.bool_field(field);
                }
                BatchFieldKey::TelnetAutoLoginTimeout => {
                    edit.telnet_auto_login_timeout_ms = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        BatchField::Set(self.value(field).trim().parse::<u64>().unwrap_or(0))
                    };
                }
                BatchFieldKey::TelnetAutoLoginRetries => {
                    edit.telnet_auto_login_max_retries = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        BatchField::Set(self.value(field).trim().parse::<u8>().unwrap_or(0))
                    };
                }
                BatchFieldKey::TelnetAutoLoginUserPrompt => {
                    edit.telnet_auto_login_username_prompt = self.text_field(field, true);
                }
                BatchFieldKey::TelnetAutoLoginPasswordPrompt => {
                    edit.telnet_auto_login_password_prompt = self.text_field(field, true);
                }
                BatchFieldKey::TelnetAutoLoginSuccessPrompt => {
                    edit.telnet_auto_login_success_prompt = self.text_field(field, true);
                }
                BatchFieldKey::TelnetAutoLoginFailurePrompt => {
                    edit.telnet_auto_login_failure_prompt = self.text_field(field, true);
                }
                BatchFieldKey::RdpDomain => {
                    edit.rdp_domain = self.text_field(field, true);
                }
                BatchFieldKey::RdpBackend => {
                    edit.rdp_backend = self.text_field(field, true);
                }
                BatchFieldKey::RdpDisplayMode => {
                    edit.rdp_display_mode = self.text_field(field, true);
                }
                BatchFieldKey::RdpWidth => {
                    edit.rdp_display_width = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        BatchField::Set(self.require_number::<u32>(field)?)
                    };
                }
                BatchFieldKey::RdpHeight => {
                    edit.rdp_display_height = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        BatchField::Set(self.require_number::<u32>(field)?)
                    };
                }
                BatchFieldKey::RdpWallpaper => {
                    edit.rdp_display_wallpaper = self.bool_field(field);
                }
                BatchFieldKey::RdpThemes => {
                    edit.rdp_display_themes = self.bool_field(field);
                }
                BatchFieldKey::RdpFontSmoothing => {
                    edit.rdp_font_smoothing = self.bool_field(field);
                }
                BatchFieldKey::RdpDesktopComposition => {
                    edit.rdp_desktop_composition = self.bool_field(field);
                }
                BatchFieldKey::RdpDisableFullWindowDrag => {
                    edit.rdp_disable_full_window_drag = self.bool_field(field);
                }
                BatchFieldKey::RdpDisableMenuAnimations => {
                    edit.rdp_disable_menu_animations = self.bool_field(field);
                }
                BatchFieldKey::RdpDisableCursorShadow => {
                    edit.rdp_disable_cursor_shadow = self.bool_field(field);
                }
                BatchFieldKey::RdpDisableCursorBlinking => {
                    edit.rdp_disable_cursor_blinking = self.bool_field(field);
                }
                BatchFieldKey::RdpAutoReconnect => {
                    edit.rdp_auto_reconnect = self.bool_field(field);
                }
                BatchFieldKey::RdpReconnectAttempts => {
                    edit.rdp_reconnect_attempts = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        BatchField::Set(self.require_number::<u32>(field)?)
                    };
                }
                BatchFieldKey::RdpRedirectClipboard => {
                    edit.rdp_redirect_clipboard = self.bool_field(field);
                }
                BatchFieldKey::RdpRedirectDrives => {
                    edit.rdp_redirect_drives = self.bool_field(field);
                }
                BatchFieldKey::RdpDriveLetters => {
                    edit.rdp_drive_letters = self.text_field(field, false);
                }
                BatchFieldKey::RdpCustomDrivePath => {
                    edit.rdp_custom_drive_path = self.text_field(field, false);
                }
                BatchFieldKey::RdpRedirectPrinters => {
                    edit.rdp_redirect_printers = self.bool_field(field);
                }
                BatchFieldKey::RdpRedirectPorts => {
                    edit.rdp_redirect_ports = self.bool_field(field);
                }
                BatchFieldKey::RdpRedirectSmartcards => {
                    edit.rdp_redirect_smartcards = self.bool_field(field);
                }
                BatchFieldKey::RdpRedirectMicrophone => {
                    edit.rdp_redirect_microphone = self.bool_field(field);
                }
                BatchFieldKey::RdpRedirectWebauthn => {
                    edit.rdp_redirect_webauthn = self.bool_field(field);
                }
                BatchFieldKey::RdpAudio => {
                    edit.rdp_audio = self.text_field(field, true);
                }
                BatchFieldKey::VncScaleMode => {
                    edit.vnc_scale_mode = self.text_field(field, true);
                }
                BatchFieldKey::VncClipboard => {
                    edit.vnc_clipboard = self.bool_field(field);
                }
                BatchFieldKey::VncShared => {
                    edit.vnc_shared = self.bool_field(field);
                }
                BatchFieldKey::VncViewOnly => {
                    edit.vnc_view_only = self.bool_field(field);
                }
                BatchFieldKey::VncReconnect => {
                    edit.vnc_reconnect = self.bool_field(field);
                }
                BatchFieldKey::VncReconnectAttempts => {
                    edit.vnc_reconnect_attempts = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        BatchField::Set(self.require_number::<u32>(field)?)
                    };
                }
                BatchFieldKey::AdbDirectory => {
                    edit.adb_directory = self.text_field(field, true);
                }
                BatchFieldKey::AdbScreenQuality => {
                    edit.adb_screen_quality = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        let preset = match self.value(field).trim() {
                            "smooth" => ScreenQualityPreset::Smooth,
                            "balanced" => ScreenQualityPreset::Balanced,
                            "sharp" => ScreenQualityPreset::Sharp,
                            _ => return Err(self.invalid_option(field)),
                        };
                        BatchField::Set(preset)
                    };
                }
                BatchFieldKey::AdbScreenMaxSize => {
                    edit.adb_screen_max_size = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        let value = self.require_number::<u32>(field)?;
                        if !(128..=2560).contains(&value) {
                            return Err(t!(
                                "assets.batch.errorAndroidMaxSize",
                                min = 128,
                                max = 2560
                            )
                            .to_string());
                        }
                        BatchField::Set(value)
                    };
                }
                BatchFieldKey::AdbScreenFps => {
                    edit.adb_screen_max_fps = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        let value = self.require_number::<u32>(field)?;
                        if !(1..=120).contains(&value) {
                            return Err(t!("assets.batch.errorAndroidMaxFps", min = 1, max = 120)
                                .to_string());
                        }
                        BatchField::Set(value)
                    };
                }
                BatchFieldKey::AdbScreenBitRate => {
                    edit.adb_screen_bit_rate = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        let value = self.require_number::<u32>(field)?;
                        if !(100_000..=100_000_000).contains(&value) {
                            return Err(t!(
                                "assets.batch.errorAndroidBitRate",
                                min = 100_000,
                                max = 100_000_000
                            )
                            .to_string());
                        }
                        BatchField::Set(value)
                    };
                }
                BatchFieldKey::AdbScreenPowerOn => {
                    edit.adb_screen_power_on = self.bool_field(field);
                }
                BatchFieldKey::AdbScreenOrientation => {
                    edit.adb_screen_orientation = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        let orientation = match self.value(field).trim() {
                            "0" => CaptureOrientation::FollowDevice,
                            "@" => CaptureOrientation::LockInitial,
                            "@0" => CaptureOrientation::Degrees0,
                            "@90" => CaptureOrientation::Degrees90,
                            "@180" => CaptureOrientation::Degrees180,
                            "@270" => CaptureOrientation::Degrees270,
                            _ => return Err(self.invalid_option(field)),
                        };
                        BatchField::Set(orientation)
                    };
                }
                BatchFieldKey::FtpInitialDirectory => {
                    edit.ftp_initial_directory = self.text_field(field, true);
                }
                BatchFieldKey::FtpLocalInitialDirectory => {
                    edit.ftp_local_initial_directory = self.text_field(field, true);
                }
                BatchFieldKey::FtpPassiveMode => {
                    edit.ftp_passive_mode = self.text_field(field, true);
                }
                BatchFieldKey::FtpFilenameEncodingMode => {
                    edit.ftp_filename_encoding_mode = self.text_field(field, true);
                }
                BatchFieldKey::FtpFilenameEncoding => {
                    edit.ftp_filename_encoding = self.text_field(field, true);
                }
                BatchFieldKey::FtpAsciiMode => {
                    edit.ftp_ascii_mode = self.bool_field(field);
                }
                BatchFieldKey::FtpSecurityMode => {
                    edit.ftp_security_mode = self.text_field(field, true);
                }
                BatchFieldKey::FtpTlsVersion => {
                    edit.ftp_tls_version = self.text_field(field, true);
                }
                BatchFieldKey::FtpConnectTimeout => {
                    edit.ftp_connect_timeout_ms = if self.empty_is_clear(field) {
                        BatchField::Clear
                    } else {
                        let value = self.require_number::<u64>(field)?;
                        if !(1_000..=120_000).contains(&value) {
                            return Err(t!(
                                "assets.batch.errorFtpTimeout",
                                min = 1_000,
                                max = 120_000
                            )
                            .to_string());
                        }
                        BatchField::Set(value)
                    };
                }
                BatchFieldKey::RecordingUseGlobal => {
                    edit.recording_use_global = self.bool_field(field);
                }
                BatchFieldKey::RecordingAutoStart => {
                    edit.recording_auto_start = self.bool_field(field);
                }
                BatchFieldKey::RecordingMode => {
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
    section: Option<ConnectionEditorSection>,
    navigation_focus: Option<FocusHandle>,
    validation_cache: std::cell::Cell<Option<bool>>,
    summary_cache: std::cell::Cell<Option<(usize, usize)>>,
    summary_expanded: bool,
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
    /// Focus handle for the vertical section rail; mirrors the connection editor.
    pub(in crate::features) fn navigation_focus(&self) -> Option<&FocusHandle> {
        self.navigation_focus.as_ref()
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
        let Some(next) = self.scopes.remove(&kind) else {
            return false;
        };
        let old = std::mem::replace(&mut self.current, next);
        self.scopes.insert(old.kind, old);
        self.section = None;
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
    pub(in crate::features) fn set_mode(&mut self, field: BatchFieldKey, mode: BatchEditMode) {
        self.validation_cache.set(None);
        self.summary_cache.set(None);
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
        if mode == BatchEditMode::Clear {
            let kind = self
                .current
                .kind
                .map(crate::models::ConnectionKindTab::from_batch_kind)
                .unwrap_or(crate::models::ConnectionKindTab::Ssh);
            let value = field.default_value(kind).unwrap_or_default();
            if field.is_bool() {
                self.current.bool_values.insert(field, value == "true");
            } else {
                self.current.values.insert(field, value);
            }
        }
        if field == BatchFieldKey::Password && mode != BatchEditMode::Set {
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
    pub(in crate::features) fn set_value(&mut self, field: BatchFieldKey, value: String) {
        self.validation_cache.set(None);
        self.summary_cache.set(None);
        if self.saving
            || self
                .current
                .rdp_applicability(field)
                .is_some_and(|state| state.applicable == 0)
        {
            return;
        }
        if field == BatchFieldKey::Password {
            self.current.modes.insert(field, BatchEditMode::Set);
            self.current.password = Some(SecretString::new(value));
            return;
        }
        // Deleting the content is the documented way to clear a text field;
        // anything else is an explicit replacement.
        let mode = if value.is_empty() {
            BatchEditMode::Clear
        } else {
            BatchEditMode::Set
        };
        self.current.modes.insert(field, mode);
        self.current.values.insert(field, value);
    }
    pub(in crate::features) fn set_bool_value(&mut self, field: BatchFieldKey, value: bool) {
        self.validation_cache.set(None);
        self.summary_cache.set(None);
        if self.saving
            || self
                .current
                .rdp_applicability(field)
                .is_some_and(|state| state.applicable == 0)
        {
            return;
        }
        self.current.modes.insert(field, BatchEditMode::Set);
        self.current.bool_values.insert(field, value);
    }
    /// Reverts one row to its original value without producing a patch.
    pub(in crate::features) fn undo_field(&mut self, field: BatchFieldKey) {
        self.set_mode(field, BatchEditMode::Keep);
    }
    /// Restores the documented default by writing `Clear`.
    pub(in crate::features) fn reset_field(&mut self, field: BatchFieldKey) {
        self.set_mode(field, BatchEditMode::Clear);
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
        basis: HashMap<BatchFieldKey, FieldBasis>,
        values: HashMap<BatchFieldKey, String>,
        bool_values: HashMap<BatchFieldKey, bool>,
        navigation_focus: Option<FocusHandle>,
    ) {
        let generation = self.generation.wrapping_add(1);
        *self = Self {
            open: true,
            ids,
            generation,
            navigation_focus,
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
        let selected: std::collections::HashSet<&str> =
            self.ids.iter().map(String::as_str).collect();
        self.targets = connections
            .iter()
            .filter(|connection| selected.contains(connection.id.as_str()))
            .map(|connection| {
                (
                    connection.id.clone(),
                    ConnectionBatchKind::from_config(&connection.config),
                )
            })
            .collect();
        self.counts = asset_batch_kind_counts(connections, &self.ids);
        // Only prepare the initially visible scope. Inactive protocols are
        // projected on first activation; subsequent visits reuse their drafts.
        if let Some(&(kind, _)) = self.counts.iter().find(|(_, count)| *count > 0) {
            self.prepare_scope(Some(kind), connections);
            self.set_scope(Some(kind));
        }
    }
    pub(in crate::features) fn prepare_scope(
        &mut self,
        kind: Option<ConnectionBatchKind>,
        connections: &[SavedConnection],
    ) {
        if self.saving || self.current.kind == kind || self.scopes.contains_key(&kind) {
            return;
        }
        let Some(kind) = kind.filter(|kind| self.counts.iter().any(|(k, _)| k == kind)) else {
            return;
        };
        let by_id = connections
            .iter()
            .map(|connection| (connection.id.as_str(), connection))
            .collect();
        let ids = self
            .targets
            .iter()
            .filter(|(_, k)| *k == kind)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let (basis, values, bool_values) = compute_batch_basis_indexed(&by_id, &ids, Some(kind));
        let mut scope = BatchScopeDraft {
            kind: Some(kind),
            original_values: values.clone(),
            original_bools: bool_values.clone(),
            basis,
            values,
            bool_values,
            ..Default::default()
        };
        if kind == ConnectionBatchKind::Rdp {
            scope.refresh_rdp_basis(connections, &self.targets);
        }
        self.scopes.insert(Some(kind), scope);
    }
    pub(in crate::features) fn refresh_rdp_basis(&mut self, connections: &[SavedConnection]) {
        self.validation_cache.set(None);
        self.summary_cache.set(None);
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
    pub(in crate::features) fn valid_for_save(&self) -> bool {
        if let Some(valid) = self.validation_cache.get() {
            return valid;
        }
        let valid = self.build_edits().is_ok();
        self.validation_cache.set(Some(valid));
        valid
    }

    pub(in crate::features) fn summary(&self) -> (usize, usize) {
        if let Some(summary) = self.summary_cache.get() {
            return summary;
        }
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
        self.summary_cache.set(Some((connections, fields)));
        (connections, fields)
    }
    pub(in crate::features) fn summary_expanded(&self) -> bool {
        self.summary_expanded
    }
    pub(in crate::features) fn toggle_summary(&mut self) {
        self.summary_expanded = !self.summary_expanded;
    }
    pub(in crate::features) fn scope_summaries(
        &self,
    ) -> Vec<(Option<ConnectionBatchKind>, usize, usize)> {
        std::iter::once(None)
            .chain(self.counts.iter().map(|(kind, _)| Some(*kind)))
            .filter_map(|kind| {
                let scope = if self.current.kind == kind {
                    Some(&self.current)
                } else {
                    self.scopes.get(&kind)
                }?;
                let fields = scope
                    .modes
                    .values()
                    .filter(|mode| **mode != BatchEditMode::Keep)
                    .count();
                let connections = if let Some(kind) = kind {
                    self.counts
                        .iter()
                        .find(|(k, _)| *k == kind)
                        .map_or(0, |(_, n)| *n)
                } else {
                    self.targets.len()
                };
                (fields > 0).then_some((kind, connections, fields))
            })
            .collect()
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
    pub(in crate::features) fn section(&self) -> Option<ConnectionEditorSection> {
        self.section
    }
    pub(in crate::features) fn set_section(&mut self, section: ConnectionEditorSection) -> bool {
        if self.section == Some(section) {
            return false;
        }
        self.section = Some(section);
        true
    }
}
