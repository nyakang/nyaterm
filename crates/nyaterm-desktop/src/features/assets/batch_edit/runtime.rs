use super::basis::{
    batch_kind_value, batch_skip_reasons_label, compute_batch_basis, parse_batch_kind,
    select_initial_value,
};
use super::fields::{BatchFieldKey, FieldBasis};
use super::options;
use super::{ASSET_BATCH_SELECT_PREFIX, ASSET_BATCH_TEXT_PREFIX};
use crate::features::NyaTermApp;
use crate::features::text_inputs::TextInputSetup;
use gpui::{Context, PathPromptOptions, SharedString, Window};
use nyaterm_core::models::SshProfile;
use nyaterm_core::models::connection_batch::{
    ConnectionBatchKind, ConnectionBatchSkipCounts, apply_connection_batch_edit,
    validate_connection_batch_edit,
};
use nyaterm_store::StoreDomain;
use nyaterm_ui::notification::NyaNotificationKind;
use rust_i18n::t;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::features) struct AssetBatchSaveSummary {
    pub updated_connections: usize,
    pub unchanged_connections: usize,
    pub skipped_fields: usize,
    pub skipped_reasons: ConnectionBatchSkipCounts,
}

impl AssetBatchSaveSummary {
    fn message(&self) -> String {
        let summary = t!(
            "assets.batch.saved",
            updated = self.updated_connections,
            unchanged = self.unchanged_connections,
            skippedFields = self.skipped_fields
        )
        .to_string();
        let reasons = batch_skip_reasons_label(self.skipped_reasons);
        if reasons.is_empty() {
            summary
        } else {
            format!("{summary} {reasons}")
        }
    }
}

struct PreparedAssetBatchUpdates {
    connections: Vec<nyaterm_core::models::SavedConnection>,
    summary: AssetBatchSaveSummary,
}

impl NyaTermApp {
    /// Open the editor for the current asset selection. Creating the widget
    /// entities here, in an event handler, keeps the render path read-only.
    ///
    /// Protocol and common scopes retain their original targets.
    pub(in crate::features) fn open_asset_batch_edit(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ids = self.start_workspace.selected_asset_ids();
        if ids.is_empty() {
            return;
        }
        if self.start_workspace.asset_batch_is_open() && self.activate_asset_batch_edit_window(cx) {
            return;
        }
        self.start_asset_batch_edit_for_selection(cx);
    }

    fn start_asset_batch_edit_for_selection(&mut self, cx: &mut Context<Self>) {
        let ids = self.start_workspace.selected_asset_ids();
        if ids.is_empty() {
            return;
        }
        let (basis, values, bool_values) =
            compute_batch_basis(self.connection_state.connections(), &ids, None);
        let groups = self.connection_state.groups().to_vec();
        self.forget_text_inputs(ASSET_BATCH_TEXT_PREFIX);
        self.start_workspace.start_asset_batch_edit(
            ids,
            basis,
            values,
            bool_values,
            Some(cx.focus_handle()),
        );
        self.start_workspace
            .asset_batch_mut()
            .initialize_scopes(self.connection_state.connections());
        self.seed_asset_batch_inputs(&groups, cx);
        self.open_asset_batch_edit_window(cx);
        cx.notify();
    }

    pub(in crate::features) fn set_asset_batch_scope(
        &mut self,
        kind: Option<ConnectionBatchKind>,
        cx: &mut Context<Self>,
    ) {
        self.start_workspace
            .asset_batch_mut()
            .prepare_scope(kind, self.connection_state.connections());
        if self.start_workspace.asset_batch_mut().set_scope(kind) {
            self.reseed_asset_batch_inputs(cx);
        }
    }
    pub(in crate::features) fn select_asset_batch_section(
        &mut self,
        section: crate::models::ConnectionEditorSection,
        cx: &mut Context<Self>,
    ) {
        if self.start_workspace.asset_batch_mut().set_section(section) {
            cx.notify();
        }
    }

    /// Move the section rail like the connection editor: up/down plus home/end.
    pub(in crate::features) fn asset_batch_navigation_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let (kind_tab, current) = {
            let draft = self.start_workspace.asset_batch();
            let kind_tab = draft
                .display_kind()
                .map(crate::models::ConnectionKindTab::from_batch_kind)
                .unwrap_or(crate::models::ConnectionKindTab::Ssh);
            let sections = crate::models::ConnectionEditorSection::for_kind(kind_tab, false);
            let current = draft
                .section()
                .filter(|section| sections.contains(section))
                .unwrap_or(sections[0]);
            (kind_tab, current)
        };
        let sections = if self.start_workspace.asset_batch().display_kind().is_none() {
            &[crate::models::ConnectionEditorSection::Basic][..]
        } else {
            crate::models::ConnectionEditorSection::for_kind(kind_tab, false)
        };
        let index = sections
            .iter()
            .position(|section| *section == current)
            .unwrap_or(0);
        let next = match event.keystroke.key.as_str() {
            "up" => Some(index.saturating_sub(1)),
            "down" => Some((index + 1).min(sections.len() - 1)),
            "home" => Some(0),
            "end" => Some(sections.len() - 1),
            _ => None,
        };
        let Some(next) = next else {
            return false;
        };
        self.select_asset_batch_section(sections[next], cx);
        true
    }

    /// Clicking the rail keeps keyboard navigation alive, as in the editor.
    pub(in crate::features) fn focus_asset_batch_navigation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(focus) = self
            .start_workspace
            .asset_batch()
            .navigation_focus()
            .cloned()
        {
            window.focus(&focus, cx);
        }
    }
    fn refresh_asset_batch_field_input(&mut self, field: BatchFieldKey, cx: &mut Context<Self>) {
        let draft = self.start_workspace.asset_batch();
        let value = if field == BatchFieldKey::Password {
            draft.replacement_password()
        } else {
            draft.value(field)
        }
        .to_string();
        let selected = select_initial_value(draft, field);
        if let Some(input) = self.existing_text_input(field.text_input_id()) {
            input.update(cx, |state, cx| {
                if state.value(cx) != value {
                    state.set_content_silent(&value, cx);
                }
            });
        }
        if let Some(select) = self.selects.field(&field.select_id()) {
            select.update(cx, |state, cx| state.set_selected_value(selected, cx));
        }
    }

    fn reseed_asset_batch_inputs(&mut self, cx: &mut Context<Self>) {
        for field in super::fields::EDITABLE_FIELDS {
            self.refresh_asset_batch_field_input(field, cx);
        }
        let groups = self.connection_state.groups().to_vec();
        self.seed_asset_batch_inputs(&groups, cx);
        cx.notify();
    }
    /// Undo: restore the row's original value without producing a patch.
    pub(in crate::features) fn undo_asset_batch_field(
        &mut self,
        field: BatchFieldKey,
        cx: &mut Context<Self>,
    ) {
        self.start_workspace.asset_batch_mut().undo_field(field);
        self.refresh_asset_batch_field_input(field, cx);
        if matches!(
            field,
            BatchFieldKey::RdpBackend | BatchFieldKey::RdpDisplayMode
        ) {
            self.start_workspace
                .asset_batch_mut()
                .refresh_rdp_basis(self.connection_state.connections());
            self.reseed_asset_batch_inputs(cx);
        }
        cx.notify();
    }
    /// Restore default: write the row's documented default as `Clear`.
    pub(in crate::features) fn reset_asset_batch_field(
        &mut self,
        field: BatchFieldKey,
        cx: &mut Context<Self>,
    ) {
        self.start_workspace.asset_batch_mut().reset_field(field);
        self.refresh_asset_batch_field_input(field, cx);
        if matches!(
            field,
            BatchFieldKey::RdpBackend | BatchFieldKey::RdpDisplayMode
        ) {
            self.start_workspace
                .asset_batch_mut()
                .refresh_rdp_basis(self.connection_state.connections());
            self.reseed_asset_batch_inputs(cx);
        }
        cx.notify();
    }
    pub(in crate::features) fn clear_asset_batch_password(&mut self, cx: &mut Context<Self>) {
        self.reset_asset_batch_field(BatchFieldKey::Password, cx);
    }
    pub(in crate::features) fn close_asset_batch_edit(&mut self, cx: &mut Context<Self>) {
        let handle = self.start_workspace.asset_batch_window_handle();
        self.dismiss_asset_batch_edit(cx);
        if let Some(handle) = handle {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
        cx.notify();
    }

    /// Close without touching the native window; used by the window's own
    /// close handler, which removes the window itself.
    pub(in crate::features) fn dismiss_asset_batch_edit(&mut self, cx: &mut Context<Self>) {
        if let Some(input) = self.existing_text_input(BatchFieldKey::Password.text_input_id()) {
            input.update(cx, |state, cx| state.discard_content(cx));
        }
        self.start_workspace.clear_asset_batch_edit();
        self.forget_text_inputs(ASSET_BATCH_TEXT_PREFIX);
        cx.notify();
    }

    fn seed_asset_batch_inputs(&mut self, groups: &[nyaterm_core::Group], cx: &mut Context<Self>) {
        for (field, hint) in [
            (BatchFieldKey::SpiceTcpPort, t!("spice.tcpPort")),
            (BatchFieldKey::SpiceTlsPort, t!("spice.tlsPort")),
            (BatchFieldKey::SpiceHttpProxy, t!("spice.httpProxy")),
            (BatchFieldKey::SpiceCaPem, t!("spice.caPem")),
            (
                BatchFieldKey::SpiceCertificateSubject,
                t!("spice.certificateSubject"),
            ),
            (
                BatchFieldKey::SpiceReconnectAttempts,
                t!("spice.reconnectAttempts"),
            ),
            (
                BatchFieldKey::Description,
                t!("assets.batch.placeholderDescription"),
            ),
            (BatchFieldKey::Port, t!("assets.batch.placeholderPort")),
            (
                BatchFieldKey::Username,
                t!("assets.batch.placeholderUsername"),
            ),
            (
                BatchFieldKey::SftpShellTimeout,
                t!("assets.batch.placeholderShellTimeout"),
            ),
            (
                BatchFieldKey::PostLoginCommand,
                t!("assets.batch.placeholderPostLoginCommand"),
            ),
            (
                BatchFieldKey::PostLoginDelay,
                t!("assets.batch.placeholderPostLoginDelay"),
            ),
            (BatchFieldKey::SshKex, t!("assets.batch.placeholderSshKex")),
            (
                BatchFieldKey::SshCiphers,
                t!("assets.batch.placeholderSshCiphers"),
            ),
            (
                BatchFieldKey::SshMacs,
                t!("assets.batch.placeholderSshMacs"),
            ),
            (
                BatchFieldKey::SshHostKeys,
                t!("assets.batch.placeholderSshHostKeys"),
            ),
            (BatchFieldKey::Tags, t!("assets.batch.placeholderValue")),
            (
                BatchFieldKey::SshAuthAgentEnvironment,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldKey::SshAuthAgentSocket,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldKey::ShellPath,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldKey::ShellArgs,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldKey::WorkingDir,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldKey::TelnetKeepAlive,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldKey::TelnetAutoLoginTimeout,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldKey::TelnetAutoLoginRetries,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldKey::TelnetAutoLoginUserPrompt,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldKey::TelnetAutoLoginPasswordPrompt,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldKey::TelnetAutoLoginSuccessPrompt,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldKey::TelnetAutoLoginFailurePrompt,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldKey::RdpDomain,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldKey::RdpDriveLetters,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldKey::RdpCustomDrivePath,
                t!("assets.batch.placeholderValue"),
            ),
            (BatchFieldKey::RdpWidth, t!("assets.batch.placeholderValue")),
            (
                BatchFieldKey::RdpHeight,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldKey::RdpReconnectAttempts,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldKey::VncReconnectAttempts,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldKey::AdbDirectory,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldKey::AdbScreenMaxSize,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldKey::AdbScreenFps,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldKey::AdbScreenBitRate,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldKey::FtpInitialDirectory,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldKey::FtpLocalInitialDirectory,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldKey::FtpConnectTimeout,
                t!("assets.batch.placeholderValue"),
            ),
        ] {
            let (seed, placeholder, disabled) = {
                let draft = self.start_workspace.asset_batch();
                match draft.basis(field) {
                    FieldBasis::Inapplicable => (
                        draft.value(field).to_string(),
                        t!("assets.batch.inapplicable").to_string(),
                        true,
                    ),
                    FieldBasis::Mixed => (
                        draft.value(field).to_string(),
                        t!("assets.batch.keepPlaceholder").to_string(),
                        false,
                    ),
                    FieldBasis::Uniform => {
                        let value = draft.value(field).to_string();
                        let placeholder = if value.is_empty() {
                            String::new()
                        } else {
                            hint.to_string()
                        };
                        (value, placeholder, false)
                    }
                }
            };
            let input = self.text_input(
                field.text_input_id(),
                &seed,
                TextInputSetup::placeholder(placeholder),
                cx,
            );

            let disabled = disabled
                || !self.start_workspace.asset_batch().field_available(field)
                || self.start_workspace.asset_batch_saving();
            input.update(cx, |state, cx| state.set_disabled(disabled, cx));
        }

        {
            let (placeholder, disabled) = {
                let draft = self.start_workspace.asset_batch();
                match draft.basis(BatchFieldKey::Password) {
                    FieldBasis::Inapplicable => (t!("assets.batch.inapplicable").to_string(), true),
                    FieldBasis::Mixed => (t!("assets.batch.mixed").to_string(), false),
                    FieldBasis::Uniform => {
                        (draft.value(BatchFieldKey::Password).to_string(), false)
                    }
                }
            };
            // The seed is always empty: the existing credential is never read
            // back into the input, only its safe set/unset status is shown.
            let mut setup = TextInputSetup::masked();
            setup.placeholder = placeholder.into();
            let seed = self
                .start_workspace
                .asset_batch()
                .replacement_password()
                .to_string();
            let input = self.text_input(BatchFieldKey::Password.text_input_id(), &seed, setup, cx);
            let disabled = disabled || self.start_workspace.asset_batch_saving();
            input.update(cx, |state, cx| state.set_disabled(disabled, cx));
        }

        {
            let (selected, placeholder) = {
                let draft = self.start_workspace.asset_batch();
                match draft.display_kind() {
                    Some(kind) => (Some(batch_kind_value(kind).to_string()), String::new()),
                    None => (Some("common".to_string()), String::new()),
                }
            };
            let select = self.select_entity(
                format!("{ASSET_BATCH_SELECT_PREFIX}kind"),
                options::scope_select_options(self.start_workspace.asset_batch()),
                selected,
                false,
                cx,
            );
            select.update(cx, |state, cx| {
                state.set_placeholder(gpui::SharedString::from(placeholder), cx);
            });
        }

        let selected_value = |field: BatchFieldKey| {
            let draft = self.start_workspace.asset_batch();
            select_initial_value(draft, field).unwrap_or_default()
        };
        let selected_ids = self.start_workspace.asset_batch_ids().to_vec();
        let account_kind = self.start_workspace.asset_batch().display_kind();
        let terminal_profile = if selected_value(BatchFieldKey::SshProfile) == "network_device" {
            SshProfile::NetworkDevice
        } else {
            SshProfile::Standard
        };
        // Each protocol offers exactly the modes its editor offers; broad
        // options would let the batch write a mode the connection cannot use.
        let auth_options = match account_kind {
            Some(ConnectionBatchKind::Ssh) => vec![
                nyaterm_ui::NyaSelectOption::new("password", t!("dialog.password")),
                nyaterm_ui::NyaSelectOption::new("key", t!("dialog.privateKey")),
                nyaterm_ui::NyaSelectOption::new("agent", t!("dialog.sshAgent")),
                nyaterm_ui::NyaSelectOption::new("none", t!("dialog.noAuthentication")),
            ],
            Some(ConnectionBatchKind::Ftp) => vec![
                nyaterm_ui::NyaSelectOption::new("password", t!("dialog.password")),
                nyaterm_ui::NyaSelectOption::new("none", t!("dialog.ftpAnonymousLogin")),
            ],
            _ => vec![
                nyaterm_ui::NyaSelectOption::new("password", t!("dialog.password")),
                nyaterm_ui::NyaSelectOption::new("none", t!("dialog.noAuthentication")),
            ],
        };
        for (field, options) in [
            (BatchFieldKey::AuthMode, auth_options),
            (BatchFieldKey::Group, options::group_select_options(groups)),
            (BatchFieldKey::Icon, {
                let mut icons = options::icon_select_options();
                icons.extend(
                    self.connection_state
                        .custom_icons
                        .records
                        .iter()
                        .map(|icon| {
                            nyaterm_ui::NyaSelectOption::new(icon.id.clone(), icon.name.clone())
                        }),
                );
                icons
            }),
            (BatchFieldKey::Encoding, options::encoding_select_options()),
            (
                BatchFieldKey::Backspace,
                options::backspace_select_options(),
            ),
            (
                BatchFieldKey::SshProfile,
                options::ssh_profile_select_options(),
            ),
            (
                BatchFieldKey::TerminalType,
                options::terminal_type_select_options(terminal_profile),
            ),
            (
                BatchFieldKey::SftpCwdFollow,
                options::sftp_cwd_follow_select_options(),
            ),
            (
                BatchFieldKey::SftpFilenameEncoding,
                options::sftp_filename_encoding_select_options(),
            ),
            (
                BatchFieldKey::SftpPipelineDepth,
                options::sftp_pipeline_depth_select_options(&selected_value(
                    BatchFieldKey::SftpPipelineDepth,
                )),
            ),
            (
                BatchFieldKey::SshAlgorithmMode,
                options::ssh_algorithm_mode_select_options(),
            ),
            (
                BatchFieldKey::RdpCertificatePolicy,
                options::rdp_certificate_policy_select_options(),
            ),
            (
                BatchFieldKey::VncSecurityMode,
                options::vnc_security_select_options(),
            ),
            (
                BatchFieldKey::Account,
                options::account_select_options(
                    self.security.passwords(),
                    account_kind,
                    &selected_value(BatchFieldKey::Account),
                ),
            ),
            (
                BatchFieldKey::Key,
                options::key_select_options(
                    self.security.ssh_keys(),
                    &selected_value(BatchFieldKey::Key),
                ),
            ),
            (
                BatchFieldKey::Otp,
                options::otp_select_options(
                    self.security.otp_entries(),
                    &selected_value(BatchFieldKey::Otp),
                ),
            ),
            (
                BatchFieldKey::Proxy,
                options::proxy_select_options(
                    self.tunnel_state.proxies(),
                    &selected_value(BatchFieldKey::Proxy),
                ),
            ),
            (
                BatchFieldKey::ProxyJump,
                options::proxy_jump_select_options(
                    self.connection_state.connections(),
                    groups,
                    &selected_value(BatchFieldKey::ProxyJump),
                    &selected_ids,
                ),
            ),
            (
                BatchFieldKey::SshAuthAgentEndpoint,
                options::ssh_agent_endpoint_select_options(&selected_value(
                    BatchFieldKey::SshAuthAgentEndpoint,
                )),
            ),
            (
                BatchFieldKey::AgentPolicy,
                options::ssh_agent_policy_select_options(&selected_value(
                    BatchFieldKey::AgentPolicy,
                )),
            ),
            (
                BatchFieldKey::RdpBackend,
                options::rdp_backend_select_options(&selected_value(BatchFieldKey::RdpBackend)),
            ),
            (
                BatchFieldKey::RdpDisplayMode,
                options::rdp_display_mode_select_options(&selected_value(
                    BatchFieldKey::RdpDisplayMode,
                )),
            ),
            (
                BatchFieldKey::RdpAudio,
                options::rdp_audio_select_options(&selected_value(BatchFieldKey::RdpAudio)),
            ),
            (
                BatchFieldKey::VncScaleMode,
                options::vnc_scale_select_options(&selected_value(BatchFieldKey::VncScaleMode)),
            ),
            (
                BatchFieldKey::RecordingMode,
                options::recording_mode_select_options(&selected_value(
                    BatchFieldKey::RecordingMode,
                )),
            ),
            (
                BatchFieldKey::SerialPort,
                options::serial_port_select_options(
                    self.connection_state.serial_ports(),
                    &self.serial_ports_in_use(),
                    &selected_value(BatchFieldKey::SerialPort),
                ),
            ),
            (
                BatchFieldKey::BaudRate,
                options::baud_rate_select_options(&selected_value(BatchFieldKey::BaudRate)),
            ),
            (
                BatchFieldKey::DataBits,
                options::data_bits_select_options(&selected_value(BatchFieldKey::DataBits)),
            ),
            (
                BatchFieldKey::Parity,
                options::parity_select_options(&selected_value(BatchFieldKey::Parity)),
            ),
            (
                BatchFieldKey::StopBits,
                options::stop_bits_select_options(&selected_value(BatchFieldKey::StopBits)),
            ),
            (
                BatchFieldKey::FlowControl,
                options::flow_control_select_options(&selected_value(BatchFieldKey::FlowControl)),
            ),
            (
                BatchFieldKey::TelnetEnterMode,
                options::telnet_enter_select_options(&selected_value(
                    BatchFieldKey::TelnetEnterMode,
                )),
            ),
            (
                BatchFieldKey::AdbScreenQuality,
                options::android_quality_select_options(),
            ),
            (
                BatchFieldKey::AdbScreenOrientation,
                options::android_orientation_select_options(),
            ),
            (
                BatchFieldKey::FtpPassiveMode,
                options::ftp_passive_select_options(&selected_value(BatchFieldKey::FtpPassiveMode)),
            ),
            (
                BatchFieldKey::FtpFilenameEncodingMode,
                options::ftp_filename_encoding_mode_select_options(),
            ),
            (
                BatchFieldKey::FtpFilenameEncoding,
                options::ftp_filename_encoding_select_options(&selected_value(
                    BatchFieldKey::FtpFilenameEncoding,
                )),
            ),
            (
                BatchFieldKey::FtpSecurityMode,
                options::ftp_security_select_options(&selected_value(
                    BatchFieldKey::FtpSecurityMode,
                )),
            ),
            (
                BatchFieldKey::FtpTlsVersion,
                options::ftp_tls_select_options(&selected_value(BatchFieldKey::FtpTlsVersion)),
            ),
        ] {
            self.seed_asset_batch_select(field, options, cx);
        }
    }

    fn seed_asset_batch_select(
        &mut self,
        field: BatchFieldKey,
        options: Vec<nyaterm_ui::NyaSelectOption>,
        cx: &mut Context<Self>,
    ) {
        let (selected, placeholder, disabled) = {
            let draft = self.start_workspace.asset_batch();
            match draft.basis(field) {
                FieldBasis::Inapplicable => (
                    select_initial_value(draft, field),
                    t!("assets.batch.inapplicable").to_string(),
                    true,
                ),
                FieldBasis::Mixed => (
                    select_initial_value(draft, field),
                    t!("assets.batch.keepPlaceholder").to_string(),
                    false,
                ),
                FieldBasis::Uniform => (select_initial_value(draft, field), String::new(), false),
            }
        };
        let options = options
            .into_iter()
            .filter(|option| {
                !option.value().is_empty()
                    && !(field == BatchFieldKey::Encoding && option.value() == "global")
                    && !(field == BatchFieldKey::SftpFilenameEncoding
                        && option.value() == "terminal")
            })
            .collect();
        let disabled = disabled
            || !self.start_workspace.asset_batch().field_available(field)
            || self.start_workspace.asset_batch_saving();
        let select = self.select_entity(field.select_id(), options, selected, disabled, cx);
        select.update(cx, |state, cx| {
            state.set_placeholder(gpui::SharedString::from(placeholder), cx);
        });
    }

    /// Notify the parent so validation and the affected-field summary follow each edit.
    pub(in crate::features) fn apply_asset_batch_input(
        &mut self,
        id: &str,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = id.strip_prefix(ASSET_BATCH_TEXT_PREFIX) else {
            return;
        };
        if key.starts_with("select.") || key.starts_with("bool.") {
            return;
        }
        if let Some(field) = BatchFieldKey::from_key(key) {
            self.start_workspace.set_asset_batch_value(field, text);
            cx.notify();
        }
    }

    /// A switch is the editor's control for booleans; toggling it is the edit,
    /// so the draft holds a concrete value from that point on.
    pub(in crate::features) fn set_asset_batch_bool_value(
        &mut self,
        field: BatchFieldKey,
        value: bool,
        cx: &mut Context<Self>,
    ) {
        self.start_workspace.set_asset_batch_bool(field, value);
        cx.notify();
    }

    pub(in crate::features) fn prompt_asset_batch_custom_drive_path(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let field = BatchFieldKey::RdpCustomDrivePath;
        let options = PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(SharedString::from(
                t!("dialog.rdpCustomDrivePickPrompt").to_string(),
            )),
        };
        let cancelled_status = t!("dialog.rdpCustomDriveSelectionCancelled").to_string();
        let input = self.existing_text_input(field.text_input_id());
        let receiver = cx.prompt_for_paths(options);
        self.shell
            .set_status(t!("dialog.rdpCustomDriveSelecting").to_string());
        cx.spawn(async move |this, cx| {
            let selected = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(path) = selected {
                    let path = path.display().to_string();
                    this.start_workspace.set_asset_batch_value(field, &path);
                    if let Some(input) = input {
                        input.update(cx, |state, cx| state.set_content(&path, cx));
                    }
                    this.shell
                        .set_status(t!("dialog.rdpCustomDriveSelected", path = path).to_string());
                } else {
                    this.shell.set_status(cancelled_status);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn apply_asset_batch_select(
        &mut self,
        id: &str,
        value: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = id.strip_prefix(ASSET_BATCH_SELECT_PREFIX) else {
            return;
        };
        if key == "kind" {
            self.set_asset_batch_scope(parse_batch_kind(value), cx);
            return;
        }
        if let Some(field) = BatchFieldKey::from_key(key) {
            if field.is_bool() {
                self.start_workspace
                    .set_asset_batch_bool(field, value == "true");
            } else {
                self.start_workspace.set_asset_batch_value(field, value);
            }
            if matches!(
                field,
                BatchFieldKey::RdpBackend | BatchFieldKey::RdpDisplayMode
            ) {
                self.start_workspace
                    .asset_batch_mut()
                    .refresh_rdp_basis(self.connection_state.connections());
                self.reseed_asset_batch_inputs(cx);
            } else {
                cx.notify();
            }
        }
    }

    pub(in crate::features) fn save_asset_batch_edit(&mut self, cx: &mut Context<Self>) {
        if self.start_workspace.asset_batch_saving() {
            return;
        }
        let edits = match self.start_workspace.asset_batch().build_edits() {
            Ok(edits) => edits,
            Err(error) => {
                self.start_workspace
                    .set_asset_batch_error(Some(error.into()));
                cx.notify();
                return;
            }
        };
        let targets = self.start_workspace.asset_batch().targets().to_vec();
        let generation = self.start_workspace.asset_batch().generation();
        self.start_workspace.set_asset_batch_saving(true);
        self.start_workspace.set_asset_batch_error(None);
        self.reseed_asset_batch_inputs(cx);
        let request = nyaterm_store::store_mutation(StoreDomain::Connections, move |store| {
            let prepared = prepare_asset_batch_updates(store, &targets, &edits)?;
            store.save_connections_atomic(&prepared.connections)?;
            Ok(prepared.summary)
        });
        let queued = self.submit_store_request(
            0,
            request,
            move |app, event, cx| app.finish_asset_batch_save(generation, event.outcome, cx),
            cx,
        );
        if !queued {
            self.start_workspace.set_asset_batch_saving(false);
            self.reseed_asset_batch_inputs(cx);
        }
    }

    pub(in crate::features) fn finish_asset_batch_save(
        &mut self,
        generation: u64,
        outcome: Result<AssetBatchSaveSummary, nyaterm_store::StoreOperationError>,
        cx: &mut Context<Self>,
    ) {
        let current = self
            .start_workspace
            .asset_batch()
            .matches_generation(generation);
        match outcome {
            Ok(summary) => {
                if current {
                    self.close_asset_batch_edit(cx);
                }
                self.report_asset_batch_result(summary, false, cx);
                let reload = nyaterm_store::store_request(StoreDomain::Connections, |store| {
                    store.load_sessions()
                });
                let queued = self.submit_store_request(
                    0,
                    reload,
                    move |app, event, cx| {
                        app.finish_asset_batch_refresh(summary, event.outcome, cx);
                    },
                    cx,
                );
                if !queued {
                    self.report_asset_batch_result(summary, true, cx);
                }
            }
            Err(error) => {
                let message = t!("assets.batch.saveFailed", error = error.to_string()).to_string();
                if current {
                    self.start_workspace.set_asset_batch_saving(false);
                    self.start_workspace
                        .set_asset_batch_error(Some(message.into()));
                    self.reseed_asset_batch_inputs(cx);
                } else {
                    self.shell.set_status(message);
                }
            }
        }
        cx.notify();
    }

    pub(in crate::features) fn finish_asset_batch_refresh(
        &mut self,
        summary: AssetBatchSaveSummary,
        outcome: Result<nyaterm_core::SessionsConfig, nyaterm_store::StoreOperationError>,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Ok(sessions) => {
                self.apply_loaded_sessions(sessions, cx);
                self.report_asset_batch_result(summary, false, cx);
            }
            Err(_) => self.report_asset_batch_result(summary, true, cx),
        }
        cx.notify();
    }

    fn report_asset_batch_result(
        &mut self,
        summary: AssetBatchSaveSummary,
        refresh_failed: bool,
        cx: &mut Context<Self>,
    ) {
        let message = if refresh_failed {
            format!("{} {}", summary.message(), t!("assets.batch.refreshFailed"))
        } else {
            summary.message()
        };
        self.shell.set_status(message.clone());
        // The asset list has no terminal status bar. Keep partial-result feedback
        // visible on the main window, including when a newer editor is open.
        self.notify_background_operation(
            "asset-batch-save",
            if refresh_failed || summary.skipped_fields > 0 {
                NyaNotificationKind::Warning
            } else {
                NyaNotificationKind::Success
            },
            message,
            cx,
        );
    }
}

fn prepare_asset_batch_updates(
    store: &nyaterm_store::ConnectionStore,
    targets: &[(String, ConnectionBatchKind)],
    edits: &std::collections::HashMap<
        Option<ConnectionBatchKind>,
        nyaterm_core::models::connection_batch::ConnectionBatchEdit,
    >,
) -> Result<PreparedAssetBatchUpdates, nyaterm_store::StorageError> {
    let mut latest = Vec::with_capacity(targets.len());
    for (id, kind) in targets {
        let connection = store.get_connection(id)?.ok_or_else(|| {
            nyaterm_store::StorageError::InvalidData(t!("assets.batch.conflict").to_string())
        })?;
        if ConnectionBatchKind::from_config(&connection.config) != *kind {
            return Err(nyaterm_store::StorageError::InvalidData(
                t!("assets.batch.conflict").to_string(),
            ));
        }
        latest.push((connection, *kind));
    }
    for edit in edits.values() {
        validate_connection_batch_edit(edit).map_err(nyaterm_store::StorageError::InvalidData)?;
        store.validate_connection_batch_references(edit)?;
    }
    let mut changed = Vec::new();
    let mut summary = AssetBatchSaveSummary::default();
    for (mut connection, kind) in latest {
        let original = connection.clone();
        for edit in [edits.get(&None), edits.get(&Some(kind))]
            .into_iter()
            .flatten()
        {
            let report = apply_connection_batch_edit(&mut connection, edit);
            summary.skipped_fields += report.skipped_fields;
            summary.skipped_reasons.merge(report.skipped_reasons);
        }
        if connection != original {
            if let nyaterm_core::models::ConnectionType::Spice {
                endpoint, settings, ..
            } = &connection.config
            {
                settings.security.validate(endpoint).map_err(|_| {
                    nyaterm_store::StorageError::InvalidData(
                        t!("spice.validation.security").to_string(),
                    )
                })?;
                endpoint.validate_persistent_identity().map_err(|_| {
                    nyaterm_store::StorageError::InvalidData(
                        t!("spice.validation.identity").to_string(),
                    )
                })?;
            }
            changed.push(connection);
            summary.updated_connections += 1;
        } else {
            summary.unchanged_connections += 1;
        }
    }
    Ok(PreparedAssetBatchUpdates {
        connections: changed,
        summary,
    })
}

#[cfg(test)]
mod tests {
    use super::prepare_asset_batch_updates;
    use nyaterm_core::models::SavedConnection;
    use nyaterm_core::models::connection_batch::{
        BatchField, ConnectionBatchEdit, ConnectionBatchKind, ConnectionBatchSkipReason,
    };
    use nyaterm_core::test_support::TestTempDir;
    use nyaterm_store::ConnectionStore;
    use std::collections::HashMap;

    #[test]
    fn spice_batch_open_seeds_editable_connection_fields_and_reseeds_after_undo() {
        use crate::features::assets::batch_edit::basis::{compute_batch_basis, parse_batch_kind};
        use crate::features::assets::batch_edit::fields::BatchFieldKey;
        use crate::features::test_support::app_with_visible_local_session;
        use crate::test_support::{TestAppCleanup, TestConfigDir};
        use gpui::TestAppContext;

        let dir = TestConfigDir::new("nyaterm-spice-batch-inputs");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "local");
        let connections: Vec<SavedConnection> = vec![serde_json::from_value(serde_json::json!({
            "id":"spice", "name":"SPICE", "type":"spice",
            "endpoint":{"host":"guest","port":5900,"tls_port":5901},
            "settings":{"security":{"ca_pem":"certificate\nsecond line","certificate_subject":"CN=guest"}}
        })).unwrap()];
        app.update(&mut cx, |app, cx| {
            let ids = vec!["spice".into()];
            let (basis, values, bools) = compute_batch_basis(&connections, &ids, None);
            app.start_workspace
                .start_asset_batch_edit(ids, basis, values, bools, None);
            app.start_workspace
                .asset_batch_mut()
                .initialize_scopes(&connections);
            app.seed_asset_batch_inputs(&[], cx);
            for field in [
                BatchFieldKey::SpiceTcpPort,
                BatchFieldKey::SpiceTlsPort,
                BatchFieldKey::SpiceHttpProxy,
                BatchFieldKey::SpiceCaPem,
                BatchFieldKey::SpiceCertificateSubject,
                BatchFieldKey::SpiceReconnectAttempts,
            ] {
                assert!(
                    app.existing_text_input(field.text_input_id()).is_some(),
                    "{field:?}"
                );
            }
            let field = BatchFieldKey::SpiceTlsPort;
            let input = app.existing_text_input(field.text_input_id()).unwrap();
            assert_eq!(input.read(cx).value(cx), "5901");
            input.update(cx, |state, cx| state.set_content_silent("5999", cx));
            app.start_workspace
                .asset_batch_mut()
                .set_value(field, "5999".into());
            app.undo_asset_batch_field(field, cx);
            assert_eq!(input.read(cx).value(cx), "5901");
            let ca = app
                .existing_text_input(BatchFieldKey::SpiceCaPem.text_input_id())
                .unwrap();
            assert_eq!(ca.read(cx).value(cx), "certificate\nsecond line");
        });
        assert_eq!(parse_batch_kind("spice"), Some(ConnectionBatchKind::Spice));
    }

    fn rdp(id: &str, backend: &str) -> SavedConnection {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "type": "rdp", "host": "example.test", "username": "user",
            "backend": backend, "description": "original", "future-option": { "enabled": true }
        }))
        .unwrap()
    }

    #[test]
    fn spice_batch_preparation_rejects_invalid_port_tls_combinations_before_any_write() {
        let dir = TestTempDir::new("nyaterm-batch-spice-tls");
        let store = ConnectionStore::open(dir.path()).unwrap();
        let original: SavedConnection = serde_json::from_value(serde_json::json!({
            "id":"spice", "name":"SPICE", "type":"spice",
            "endpoint":{"host":"guest","port":5900,"tls_port":5901}
        }))
        .unwrap();
        store.save_connections_atomic(&[original]).unwrap();
        let before = store.get_connection("spice").unwrap().unwrap();
        let targets = vec![("spice".into(), ConnectionBatchKind::Spice)];
        let edit = ConnectionBatchEdit {
            spice: nyaterm_core::models::connection_batch::spice::SpiceBatchEdit {
                tls_port: BatchField::Clear,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut edits = HashMap::from([(Some(ConnectionBatchKind::Spice), edit)]);
        assert!(prepare_asset_batch_updates(&store, &targets, &edits).is_err());
        assert_eq!(store.get_connection("spice").unwrap().unwrap(), before);
        edits
            .get_mut(&Some(ConnectionBatchKind::Spice))
            .unwrap()
            .spice
            .require_tls = BatchField::Set(false);
        let prepared = prepare_asset_batch_updates(&store, &targets, &edits).unwrap();
        assert_eq!(prepared.connections.len(), 1);
        assert_eq!(store.get_connection("spice").unwrap().unwrap(), before);
    }

    #[test]
    fn partial_field_skips_are_reported_when_every_connection_updates() {
        let dir = TestTempDir::new("nyaterm-batch-partial-fields");
        let store = ConnectionStore::open(dir.path()).unwrap();
        let originals = [rdp("a", "builtin"), rdp("b", "mstsc")];
        store.save_connections_atomic(&originals).unwrap();
        let originals =
            originals.map(|connection| store.get_connection(&connection.id).unwrap().unwrap());
        let targets = vec![
            ("a".into(), ConnectionBatchKind::Rdp),
            ("b".into(), ConnectionBatchKind::Rdp),
        ];
        let edits = HashMap::from([
            (
                None,
                ConnectionBatchEdit {
                    description: BatchField::Set("updated".into()),
                    ..Default::default()
                },
            ),
            (
                Some(ConnectionBatchKind::Rdp),
                ConnectionBatchEdit {
                    rdp_use_nla: BatchField::Set(false),
                    ..Default::default()
                },
            ),
        ]);
        let prepared = prepare_asset_batch_updates(&store, &targets, &edits).unwrap();
        assert_eq!(prepared.summary.updated_connections, 2);
        assert_eq!(prepared.summary.unchanged_connections, 0);
        assert_eq!(prepared.summary.skipped_fields, 1);
        assert_eq!(
            prepared
                .summary
                .skipped_reasons
                .count(ConnectionBatchSkipReason::MstscBackend),
            1
        );
        store
            .save_connections_atomic(&prepared.connections)
            .unwrap();
        for (original, expected_nla) in originals.iter().zip([false, true]) {
            let actual = store.get_connection(&original.id).unwrap().unwrap();
            let mut expected = original.clone();
            expected.description = Some("updated".into());
            assert_eq!(actual.created_at_ms, original.created_at_ms);
            assert!(actual.updated_at_ms >= original.updated_at_ms);
            expected.updated_at_ms = actual.updated_at_ms;
            if let nyaterm_core::models::ConnectionType::Rdp { security, .. } = &mut expected.config
            {
                security.use_nla = expected_nla;
            }
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn identical_values_and_all_skipped_fields_preserve_data_and_report_no_changes() {
        let dir = TestTempDir::new("nyaterm-batch-no-change");
        let store = ConnectionStore::open(dir.path()).unwrap();
        let originals = [rdp("a", "mstsc"), rdp("b", "mstsc")];
        store.save_connections_atomic(&originals).unwrap();
        let originals =
            originals.map(|connection| store.get_connection(&connection.id).unwrap().unwrap());
        let targets = vec![
            ("a".into(), ConnectionBatchKind::Rdp),
            ("b".into(), ConnectionBatchKind::Rdp),
        ];
        for (scope, edit, skipped) in [
            (
                None,
                ConnectionBatchEdit {
                    description: BatchField::Set("original".into()),
                    ..Default::default()
                },
                0,
            ),
            (
                Some(ConnectionBatchKind::Rdp),
                ConnectionBatchEdit {
                    rdp_use_nla: BatchField::Set(false),
                    ..Default::default()
                },
                2,
            ),
        ] {
            let prepared =
                prepare_asset_batch_updates(&store, &targets, &HashMap::from([(scope, edit)]))
                    .unwrap();
            assert!(prepared.connections.is_empty());
            assert_eq!(prepared.summary.updated_connections, 0);
            assert_eq!(prepared.summary.unchanged_connections, 2);
            assert_eq!(prepared.summary.skipped_fields, skipped);
            assert_eq!(
                prepared
                    .summary
                    .skipped_reasons
                    .count(ConnectionBatchSkipReason::MstscBackend),
                skipped
            );
            store
                .save_connections_atomic(&prepared.connections)
                .unwrap();
            for original in &originals {
                assert_eq!(
                    store.get_connection(&original.id).unwrap().unwrap(),
                    *original
                );
            }
        }
    }

    #[test]
    fn invalid_reference_rejects_all_updates_before_persistence() {
        let dir = TestTempDir::new("nyaterm-batch-reference-conflict");
        let store = ConnectionStore::open(dir.path()).unwrap();
        let original: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "a", "name": "a", "type": "ssh", "host": "example.test", "username": "user"
        }))
        .unwrap();
        store
            .save_connections_atomic(std::slice::from_ref(&original))
            .unwrap();
        let original = store.get_connection(&original.id).unwrap().unwrap();
        let edits = HashMap::from([
            (
                None,
                ConnectionBatchEdit {
                    description: BatchField::Set("updated".into()),
                    ..Default::default()
                },
            ),
            (
                Some(ConnectionBatchKind::Ssh),
                ConnectionBatchEdit {
                    account_id: BatchField::Set("missing-account".into()),
                    ..Default::default()
                },
            ),
        ]);
        assert!(
            prepare_asset_batch_updates(&store, &[("a".into(), ConnectionBatchKind::Ssh)], &edits)
                .is_err()
        );
        assert_eq!(store.get_connection("a").unwrap().unwrap(), original);
    }

    #[test]
    fn changed_protocol_or_deleted_target_rejects_the_whole_batch_without_writes() {
        let dir = TestTempDir::new("nyaterm-batch-conflict");
        let store = ConnectionStore::open(dir.path()).unwrap();
        let connection = |id, kind| {
            serde_json::from_value::<SavedConnection>(serde_json::json!({"id": id, "name": id, "type": kind, "host": "host", "username": "user"})).unwrap()
        };
        let a = connection("a", "ssh");
        let b = connection("b", "rdp");
        store.save_connections_atomic(&[a, b]).unwrap();
        let targets = vec![
            ("a".into(), ConnectionBatchKind::Ssh),
            ("b".into(), ConnectionBatchKind::Ssh),
        ];
        let edits = HashMap::from([(
            None,
            ConnectionBatchEdit {
                description: BatchField::Set("changed".into()),
                ..Default::default()
            },
        )]);
        assert!(prepare_asset_batch_updates(&store, &targets, &edits).is_err());
        assert!(
            store
                .get_connection("a")
                .unwrap()
                .unwrap()
                .description
                .is_none()
        );
        store.delete_connection("b").unwrap();
        assert!(prepare_asset_batch_updates(&store, &targets, &edits).is_err());
        assert!(
            store
                .get_connection("a")
                .unwrap()
                .unwrap()
                .description
                .is_none()
        );
    }
}
