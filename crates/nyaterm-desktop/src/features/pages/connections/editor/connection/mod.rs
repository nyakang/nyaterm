use nyaterm_ui::icons::{CONNECTION_ICON_OPTIONS, resolve_connection_icon};
mod credentials;
mod groups;
mod selectors;

use self::credentials::connection_editor_agent_identity_picker;
use self::groups::{
    ConnectionEditorGroupControlArgs, connection_editor_group_control,
    connection_editor_group_display_label, connection_editor_group_menu_options,
    connection_group_path_label,
};
use self::selectors::{
    connection_editor_select_id, connection_editor_select_is_searchable,
    connection_editor_select_keys, connection_editor_select_search_placeholder,
};
pub(in crate::features::pages::connections) use groups::ordered_connection_groups;

use crate::models::ConnectionEditorFocusTarget;
mod android;
mod authentication;
mod layout;
mod local;
pub(in crate::features) mod options;
mod rdp;
mod recording;
mod serial;
mod ssh;
mod tags;
mod telnet;
mod vnc;

use rust_i18n::t;

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use gpui::{
    AnyElement, App, Context, Entity, FontWeight, IntoElement, SharedString, div,
    prelude::{
        FluentBuilder, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled,
    },
    px, rgb, rgba,
};
use nyaterm_core::{SavedConnection, truncate_preview};
use nyaterm_ui::{
    NyaInput, NyaPopover, NyaScrollable, NyaSelectOption, NyaSelectState, NyaSwitch, NyaTooltip,
};

use self::local::connection_editor_local_section;
use self::rdp::connection_editor_rdp_section;
use self::serial::connection_editor_serial_section;
use self::ssh::{
    SshConnectionSectionLabels, SshConnectionSectionOptions, connection_editor_ssh_section,
};
use self::telnet::connection_editor_telnet_section;
use self::vnc::connection_editor_vnc_section;
use super::super::list::{ConnectionEditorChoice, ConnectionEditorFields, editor_field};
use crate::features::selects::NO_SELECTION_VALUE;
use crate::features::{
    NyaTermApp, text_inputs::ORDINARY_INPUT_SHELL_PADDING_X_PX,
    text_inputs::ordinary_input_focus_ring, text_inputs::ordinary_input_shell_border_color,
    view_widgets::modal_dialog_shell, view_widgets::themed_icon,
};
use crate::models::{
    ConnectionEditorField, ConnectionEditorSection, ConnectionEditorSelect, ConnectionEditorState,
    ConnectionKindTab,
};

#[derive(Clone, Copy)]
struct ConnectionEditorSectionContext<'a> {
    palette: crate::theme::ThemePalette,
    editor: &'a ConnectionEditorState,
    fields: &'a ConnectionEditorFields,
    baud_popover_open: bool,
    narrow: bool,
}

impl NyaTermApp {
    pub(in crate::features) fn connection_editor_panel(
        &mut self,
        editor: ConnectionEditorState,
        viewport: (f32, f32),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.connection_editor_surface(editor, false, viewport, cx)
    }

    pub(in crate::features) fn connection_editor_window_view(
        &mut self,
        editor: ConnectionEditorState,
        viewport: (f32, f32),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.connection_editor_surface(editor, true, viewport, cx)
    }

    fn connection_editor_select_entity(
        &mut self,
        select_key: ConnectionEditorSelect,
        choices: &[ConnectionEditorChoice],
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> Entity<NyaSelectState> {
        let options = Self::connection_editor_select_options(choices);
        let selected_value = choices.iter().find(|choice| choice.selected).map(|choice| {
            choice
                .value
                .clone()
                .unwrap_or_else(|| NO_SELECTION_VALUE.to_string())
        });
        let select = self.select_entity(
            connection_editor_select_id(select_key),
            options,
            selected_value,
            false,
            cx,
        );
        let searchable = connection_editor_select_is_searchable(select_key);
        let search_placeholder = connection_editor_select_search_placeholder(select_key);
        select.update(cx, |select, cx| {
            select.set_placeholder(placeholder, cx);
            select.set_searchable(searchable, cx);
            select.set_search_placeholder(search_placeholder, cx);
        });
        select
    }

    fn connection_editor_select_options(
        choices: &[ConnectionEditorChoice],
    ) -> Vec<NyaSelectOption> {
        choices
            .iter()
            .map(|choice| {
                let mut option = NyaSelectOption::new(
                    choice
                        .value
                        .clone()
                        .unwrap_or_else(|| NO_SELECTION_VALUE.to_string()),
                    choice.label.clone(),
                )
                .disabled(choice.disabled);
                if let Some(search_text) = choice.search_text.as_ref() {
                    option = option.search_text(search_text.clone());
                }
                if let Some(subtitle) = choice.subtitle.as_ref() {
                    option = option.subtitle(subtitle.clone());
                }
                option
            })
            .collect()
    }

    fn connection_editor_surface(
        &mut self,
        editor: ConnectionEditorState,
        native_window: bool,
        viewport: (f32, f32),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if editor.defaults_mode && self.connection_state.editor_save_in_flight() {
            return div()
                .w_full()
                .h(px(viewport.1.min(680.)))
                .flex()
                .items_center()
                .justify_center()
                .child(t!("common.loading"))
                .into_any_element();
        }
        self.connection_state
            .ensure_editor_forwarding_endpoint_fields(cx);
        self.connection_state.sync_editor_password_input(cx);
        let password_disabled =
            editor.kind == ConnectionKindTab::Vnc && editor.vnc_security.mode == "none";
        if let Some(input) = self
            .connection_state
            .editor_fields()
            .get(&ConnectionEditorField::Password)
        {
            input.update(cx, |state, cx| state.set_disabled(password_disabled, cx));
        }
        for (field, disabled) in [
            (
                ConnectionEditorField::SftpShellDetectionTimeout,
                editor.kind == ConnectionKindTab::Ssh && !editor.sftp_enabled,
            ),
            (
                ConnectionEditorField::RdpReconnectAttempts,
                editor.kind == ConnectionKindTab::Rdp && !editor.rdp_reconnect.enabled,
            ),
            (
                ConnectionEditorField::VncReconnectAttempts,
                editor.kind == ConnectionKindTab::Vnc && !editor.vnc_reconnect.enabled,
            ),
        ] {
            if let Some(entity) = self
                .connection_state
                .editor_number_fields()
                .get(&field)
                .cloned()
            {
                entity.update(cx, |entity, cx| entity.set_disabled(disabled, cx));
            }
        }
        let palette = self.theme_palette();
        let title = if editor.defaults_mode {
            t!("menu.defaultSessionSettings")
        } else if editor.id.is_some() {
            t!("dialog.editConnection")
        } else {
            t!("dialog.newConnection")
        };
        let name_label = t!("dialog.connectionName");
        let description_label = t!("dialog.description");
        let group_title = t!("dialog.group");
        let none_label = t!("dialog.none");
        let icon_label = t!("dialog.icon");
        let icon_auto_detect_label = t!("dialog.iconAutoDetect");
        let icon_auto_detect_hint = t!("dialog.iconAutoDetectTooltip");
        let icon_auto_detect = editor.icon_auto_detect;
        let icon_auto_detect_enabled = editor.kind == ConnectionKindTab::Ssh
            && editor.ssh_profile == nyaterm_core::SshProfile::Standard;
        let icon_auto_detect_hint = if editor.kind != ConnectionKindTab::Ssh
            || editor.ssh_profile == nyaterm_core::SshProfile::NetworkDevice
        {
            t!("dialog.iconAutoDetectNetworkDeviceTooltip").to_string()
        } else {
            icon_auto_detect_hint.to_string()
        };
        let group_options = connection_editor_group_menu_options(
            self.connection_state.groups(),
            &editor,
            none_label.clone(),
            t!("dialog.newGroup"),
        );
        let group_parent_id = if editor.pending_group_name.is_some() {
            editor.pending_group_parent_id.as_deref()
        } else {
            editor.group_id.as_deref()
        };
        let group_parent_hint = group_parent_id
            .and_then(|id| connection_group_path_label(self.connection_state.groups(), id))
            .map(|path| t!("dialog.newGroupParentHint", group = path).to_string())
            .unwrap_or_else(|| t!("dialog.newGroupRootHint").to_string());
        let key_label = editor
            .key_id
            .as_deref()
            .and_then(|id| {
                self.security
                    .ssh_keys()
                    .iter()
                    .find(|key| key.id == id)
                    .map(|key| key.name.clone())
            })
            .unwrap_or_else(|| none_label.to_string());
        let password_label = editor
            .account_id
            .as_ref()
            .filter(|_| {
                matches!(
                    editor.kind,
                    ConnectionKindTab::Ssh | ConnectionKindTab::Telnet
                )
            })
            .or(editor.password_id.as_ref())
            .and_then(|id| {
                self.security
                    .passwords()
                    .iter()
                    .find(|password| password.id == id.as_str())
                    .map(|password| password.name.clone())
            })
            .unwrap_or_else(|| t!("dialog.selectPassword").to_string());
        let otp_label = editor
            .otp_id
            .as_deref()
            .and_then(|id| {
                self.security
                    .otp_entries()
                    .iter()
                    .find(|entry| entry.id == id)
                    .map(|entry| {
                        if entry.issuer.is_empty() {
                            entry.username.clone()
                        } else if entry.username.is_empty() {
                            entry.issuer.clone()
                        } else {
                            format!("{} ({})", entry.issuer, entry.username)
                        }
                    })
            })
            .unwrap_or_else(|| t!("dialog.noOtp").to_string());
        let proxy_label = editor
            .proxy_id
            .as_deref()
            .and_then(|id| {
                self.tunnel_state
                    .proxies()
                    .iter()
                    .find(|proxy| proxy.id == id)
                    .map(|proxy| {
                        if proxy.protocol == "proxycommand" {
                            let command = proxy.command.as_deref().unwrap_or("").trim();
                            if command.is_empty() {
                                format!("{} · {}", proxy.name, proxy.protocol.to_ascii_uppercase())
                            } else {
                                format!(
                                    "{} · {} {}",
                                    proxy.name,
                                    proxy.protocol.to_ascii_uppercase(),
                                    truncate_preview(command, 18)
                                )
                            }
                        } else {
                            format!(
                                "{} · {} {}:{}",
                                proxy.name,
                                proxy.protocol.to_ascii_uppercase(),
                                proxy.host,
                                proxy.port
                            )
                        }
                    })
            })
            .unwrap_or_else(|| t!("dialog.noProxy").to_string());
        let jump_label = editor
            .proxy_jump_id
            .as_deref()
            .and_then(|id| {
                self.connection_state
                    .connections()
                    .iter()
                    .find(|connection| connection.id == id)
                    .map(|connection| connection.name.clone())
            })
            .unwrap_or_else(|| t!("dialog.noProxyJump").to_string());
        let auth_options = options::ssh_auth_choices(&editor.auth_mode);
        let key_options =
            options::ssh_key_choices(self.security.ssh_keys(), editor.key_id.as_deref());
        let password_selected = if matches!(
            editor.kind,
            ConnectionKindTab::Ssh | ConnectionKindTab::Telnet
        ) {
            editor.account_id.as_deref()
        } else {
            editor.password_id.as_deref()
        };
        let password_reference = if matches!(
            editor.kind,
            ConnectionKindTab::Ssh | ConnectionKindTab::Telnet
        ) {
            editor.account_id.as_ref()
        } else {
            editor.password_id.as_ref()
        };
        let password_none_selected = editor.account_id.is_none() && editor.password_id.is_none();
        let password_options = options::password_choices(
            self.security.passwords(),
            password_selected,
            password_none_selected,
            password_reference.map(String::as_str),
        );
        let account_options = options::account_choices(
            self.security.passwords(),
            password_selected,
            password_none_selected,
            password_reference.map(String::as_str),
        );
        let account_placeholder = if editor.account_id.is_none() {
            t!("authForm.manualIdentity").to_string()
        } else {
            password_label.clone()
        };
        let otp_options =
            options::otp_choices(self.security.otp_entries(), editor.otp_id.as_deref());
        let proxy_options =
            options::proxy_choices(self.tunnel_state.proxies(), editor.proxy_id.as_deref());
        let jump_options = options::proxy_jump_choices(
            self.connection_state.connections(),
            self.connection_state.groups(),
            editor.id.as_deref(),
            editor.proxy_jump_id.as_deref(),
        );
        let backspace_options = options::backspace_choices(&editor.backspace_mode);
        let telnet_enter_options = options::telnet_enter_choices(&editor.telnet_enter_mode);
        let sftp_pipeline_options =
            options::sftp_pipeline_depth_choices(editor.sftp_pipeline_depth);
        let encoding_options = options::encoding_choices(&editor.encoding);
        let ssh_profile_options = options::ssh_profile_choices(editor.ssh_profile);
        let ssh_terminal_options =
            options::ssh_terminal_type_choices(editor.ssh_profile, editor.terminal_type);
        let rdp_backend_options =
            options::rdp_backend_choices(&editor.rdp_backend, editor.uses_mstsc_rdp());
        let rdp_certificate_options =
            options::rdp_certificate_policy_choices(&editor.rdp_security.certificate_policy);
        let rdp_display_options = options::rdp_display_mode_choices(&editor.rdp_display.mode);
        let vnc_security_options = options::vnc_security_choices(&editor.vnc_security.mode);
        let vnc_scale_options = options::vnc_scale_choices(&editor.vnc_display.scale_mode);
        let recording_mode = editor
            .recording
            .as_ref()
            .and_then(|settings| settings.mode)
            .unwrap_or(nyaterm_core::RecordingMode::Transcript);
        let recording_mode_options = options::recording_mode_choices(recording_mode);
        let sftp_cwd_options = options::sftp_cwd_follow_choices(&editor.sftp_cwd_follow_mode);
        let sftp_filename_encoding_options =
            options::sftp_filename_encoding_choices(&editor.sftp_filename_encoding);
        let ssh_algorithm_mode_options =
            options::ssh_algorithm_mode_choices(&editor.ssh_algorithm_mode);
        let serial_port_options =
            options::serial_port_choices(self.connection_state.serial_ports(), &editor.serial_port);
        let baud_options = options::baud_rate_choices(&editor.baud_rate);
        let data_bits_options = options::data_bits_choices(&editor.data_bits);
        let parity_options = options::parity_choices(&editor.parity);
        let flow_control_options = options::flow_control_choices(editor.flow_control.as_str());
        let stop_bits_options = options::stop_bits_choices(&editor.stop_bits);
        let policy_options =
            options::ssh_agent_forwarding_policy_choices(&editor.agent_forwarding_config.policy);
        let ssh_agent_endpoint_options =
            options::ssh_agent_endpoint_choices(&editor.agent_endpoint);
        let shell_label = match editor.shell_path.as_str() {
            "powershell.exe" => t!("dialog.shellPowerShell"),
            "cmd.exe" => t!("dialog.shellCmd"),
            "bash" => t!("dialog.shellBash"),
            "wsl.exe" => t!("dialog.shellWsl"),
            "wt.exe" => t!("dialog.shellWindowsTerminal"),
            _ => t!("dialog.shellCustom"),
        };
        let shell_options = options::shell_choices(&editor.shell_path);
        let serial_port_placeholder = if self.connection_state.serial_ports_loading() {
            t!("dialog.loadingSerialPorts").to_string()
        } else if let Some(error) = self.connection_state.serial_ports_error() {
            format!("{}: {error}", t!("dialog.serialPortsLoadFailed"))
        } else if self.connection_state.serial_ports().is_empty() {
            t!("dialog.noSerialPortsFound").to_string()
        } else {
            t!("dialog.selectSerialPort").to_string()
        };
        let mut selects = HashMap::new();
        for (select_key, choices, placeholder) in [
            (
                ConnectionEditorSelect::SavedPassword,
                password_options.as_slice(),
                password_label.clone(),
            ),
            (
                ConnectionEditorSelect::Account,
                account_options.as_slice(),
                account_placeholder,
            ),
            (
                ConnectionEditorSelect::SshKey,
                key_options.as_slice(),
                key_label.clone(),
            ),
            (
                ConnectionEditorSelect::Otp,
                otp_options.as_slice(),
                otp_label.clone(),
            ),
            (
                ConnectionEditorSelect::Proxy,
                proxy_options.as_slice(),
                proxy_label.clone(),
            ),
            (
                ConnectionEditorSelect::ProxyJump,
                jump_options.as_slice(),
                jump_label.clone(),
            ),
            (
                ConnectionEditorSelect::Backspace,
                backspace_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::Encoding,
                encoding_options.as_slice(),
                t!("connection.encodingFollowGlobal").to_string(),
            ),
            (
                ConnectionEditorSelect::SftpCwdFollowMode,
                sftp_cwd_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::SftpPipelineDepth,
                sftp_pipeline_options.as_slice(),
                t!("dialog.sftpPipelineAutomatic").to_string(),
            ),
            (
                ConnectionEditorSelect::SftpFilenameEncoding,
                sftp_filename_encoding_options.as_slice(),
                t!("dialog.sftpFilenameEncodingFollowTerminal").to_string(),
            ),
            (
                ConnectionEditorSelect::SshAlgorithmMode,
                ssh_algorithm_mode_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::SshAgentEndpoint,
                ssh_agent_endpoint_options.as_slice(),
                t!("dialog.sshAgentAuto").to_string(),
            ),
            (
                ConnectionEditorSelect::SshAgentForwardingPolicy,
                policy_options.as_slice(),
                t!("dialog.sshAgentPolicyAllowlist").to_string(),
            ),
            (
                ConnectionEditorSelect::SshProfile,
                ssh_profile_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::SshTerminalType,
                ssh_terminal_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::RdpBackend,
                rdp_backend_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::RdpCertificatePolicy,
                rdp_certificate_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::RdpDisplayMode,
                rdp_display_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::VncSecurityMode,
                vnc_security_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::VncScaleMode,
                vnc_scale_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::RecordingMode,
                recording_mode_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::TelnetEnterMode,
                telnet_enter_options.as_slice(),
                "CR (\\r)".to_string(),
            ),
            (
                ConnectionEditorSelect::Shell,
                shell_options.as_slice(),
                shell_label.to_string(),
            ),
            (
                ConnectionEditorSelect::SerialPort,
                serial_port_options.as_slice(),
                serial_port_placeholder,
            ),
            (
                ConnectionEditorSelect::BaudRate,
                baud_options.as_slice(),
                t!("dialog.customBaudRate").to_string(),
            ),
            (
                ConnectionEditorSelect::DataBits,
                data_bits_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::Parity,
                parity_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::FlowControl,
                flow_control_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::StopBits,
                stop_bits_options.as_slice(),
                String::new(),
            ),
        ] {
            let select = self.connection_editor_select_entity(select_key, choices, placeholder, cx);
            let disabled = matches!(
                select_key,
                ConnectionEditorSelect::SftpCwdFollowMode
                    | ConnectionEditorSelect::SftpFilenameEncoding
                    | ConnectionEditorSelect::SftpPipelineDepth
            ) && !editor.sftp_enabled
                || select_key == ConnectionEditorSelect::SavedPassword && password_disabled
                || select_key == ConnectionEditorSelect::SerialPort
                    && self.connection_state.serial_ports_loading();
            select.update(cx, |select, cx| select.set_disabled(disabled, cx));
            selects.insert(select_key, select);
        }
        let forwarding_endpoint_selects = editor
            .agent_forwarding_config
            .sources
            .external_agent_endpoints
            .iter()
            .enumerate()
            .map(|(index, endpoint)| {
                let choices = options::ssh_agent_endpoint_choices(endpoint);
                let options = Self::connection_editor_select_options(&choices);
                let selected_value = Some(options::ssh_agent_endpoint_value(endpoint).to_string());
                let select = self.select_entity(
                    format!("connection-editor-ssh-agent-forwarding-endpoint-{index}"),
                    options,
                    selected_value,
                    false,
                    cx,
                );
                (index, select)
            })
            .collect();
        let forwarding_endpoint_fields = self
            .connection_state
            .editor_forwarding_endpoint_fields()
            .clone();
        let mut fields = ConnectionEditorFields::new(
            self.connection_state.editor_fields().clone(),
            self.connection_state.editor_number_fields().clone(),
            selects,
            forwarding_endpoint_selects,
            forwarding_endpoint_fields,
        );
        let targets = self
            .connection_state
            .editor_fields()
            .keys()
            .chain(self.connection_state.editor_number_fields().keys())
            .copied()
            .map(ConnectionEditorFocusTarget::Field)
            .chain(
                connection_editor_select_keys()
                    .into_iter()
                    .map(ConnectionEditorFocusTarget::Select),
            )
            .chain(
                self.connection_state
                    .editor_forwarding_endpoint_fields()
                    .keys()
                    .map(
                        |(index, field)| ConnectionEditorFocusTarget::ForwardingEndpoint {
                            index: *index,
                            field: *field,
                        },
                    ),
            )
            .collect::<Vec<_>>();
        fields.anchors = targets
            .into_iter()
            .map(|target| (target, self.connection_state.editor_anchor(target)))
            .collect();
        let scroll = self.connection_state.editor_scroll();
        let heading_anchor = self
            .connection_state
            .editor_anchor(ConnectionEditorFocusTarget::Heading);
        let account_id = if matches!(
            editor.kind,
            ConnectionKindTab::Ssh | ConnectionKindTab::Telnet
        ) {
            editor.account_id.as_deref()
        } else {
            editor.password_id.as_deref()
        };
        fields.account = account_id
            .and_then(|id| {
                self.security
                    .passwords()
                    .iter()
                    .find(|entry| entry.id == id)
            })
            .cloned();
        fields.account_password_locked =
            self.settings.summary().has_master_password && !self.security.secrets_unlocked();
        fields.errors = editor.field_errors.clone();
        let icon_key = editor.icon.as_deref();
        let icon_def = resolve_connection_icon(icon_key, editor.kind.label());
        let icon_picker_open = self.connection_state.editor_icon_picker_is_open();
        let group_select_open = self.connection_state.editor_group_select_is_open();
        let agent_identity_picker_open =
            self.connection_state.editor_agent_identity_picker_is_open();
        let credential_overlay = self.connection_state.editor_credential_overlay();
        let baud_popover_open = self.connection_state.editor_baud_popover_is_open();
        let validation_error = self.connection_editor_validation_error(&editor);
        let save_enabled = !self.connection_state.editor_save_in_flight();
        let width = if native_window {
            viewport.0
        } else {
            (viewport.0 - 26.).min(900.)
        };
        let height = if native_window {
            viewport.1
        } else {
            (viewport.1 - 26.).min(680.)
        };
        let wide = width >= 760.;
        let navigation_width = if wide { 180. } else { 128. };
        let narrow = width - navigation_width < 500.;
        let protocol_select = self.select_entity(
            "connection-editor-kind",
            [
                ConnectionKindTab::Ssh,
                ConnectionKindTab::Local,
                ConnectionKindTab::Telnet,
                ConnectionKindTab::Serial,
                ConnectionKindTab::Rdp,
                ConnectionKindTab::Vnc,
                ConnectionKindTab::Adb,
            ]
            .into_iter()
            .map(|kind| {
                NyaSelectOption::new(
                    kind.label(),
                    match kind {
                        ConnectionKindTab::Local => t!("dialog.localTerminal"),
                        ConnectionKindTab::Serial => t!("dialog.serial"),
                        _ => Cow::Borrowed(kind.label()),
                    },
                )
            })
            .collect(),
            Some(editor.kind.label().to_string()),
            false,
            cx,
        );
        let section_focus = self.connection_state.editor_section_focus();
        let section_context = ConnectionEditorSectionContext {
            palette,
            editor: &editor,
            fields: &fields,
            baud_popover_open,
            narrow,
        };
        let custom_icons = self
            .connection_state
            .custom_icons
            .records
            .iter()
            .filter_map(|record| {
                self.connection_state
                    .custom_icons
                    .images
                    .get(&record.id)
                    .cloned()
                    .map(|image| (record.clone(), image))
            })
            .collect::<Vec<_>>();
        let custom_selected = editor
            .icon
            .as_ref()
            .filter(|id| {
                self.connection_state
                    .custom_icons
                    .records
                    .iter()
                    .any(|record| &record.id == *id)
            })
            .cloned();
        let icon_rows = (CONNECTION_ICON_OPTIONS.len() + custom_icons.len()).div_ceil(7);
        // Every icon cell is 28 px, with a 4 px gap between rows.
        let icon_grid_height = icon_rows * 28 + icon_rows.saturating_sub(1) * 4;
        let icon_viewport_height = icon_grid_height.min(280) as f32;
        let mut icon_grid = div()
            .grid()
            .grid_cols(7)
            .gap_1()
            .pr_4()
            .debug_selector(|| "connection-editor-icon-grid".to_string());
        for (index, icon_key) in CONNECTION_ICON_OPTIONS.iter().copied().enumerate() {
            let icon = resolve_connection_icon(Some(icon_key), editor.kind.label());
            // Match the trigger's resolved icon, including protocol fallbacks
            // and legacy aliases, without treating a custom icon as a default.
            let selected = custom_selected.is_none() && icon == icon_def;
            icon_grid = icon_grid.child(
                div()
                    .id(SharedString::from(format!("connection-icon-{icon_key}")))
                    .size(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .bg(if selected {
                        rgba((palette.primary << 8) | 0x26)
                    } else {
                        rgba(0x00000000)
                    })
                    .border_1()
                    .border_color(if selected {
                        rgb(palette.primary)
                    } else {
                        rgba(0x00000000)
                    })
                    .hover(|this| this.bg(rgb(palette.hover)))
                    .when(index == 6, |this| {
                        this.debug_selector(|| "connection-editor-icon-rightmost".to_string())
                    })
                    .when(selected, |this| {
                        this.debug_selector(move || {
                            format!("connection-editor-icon-selected-{icon_key}")
                        })
                    })
                    .child(themed_icon(palette, icon, false, 16.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_connection_editor_icon(Some(icon_key), cx);
                    })),
            );
        }
        for (index, (record, image)) in custom_icons.into_iter().enumerate() {
            let id = record.id.clone();
            let selected = editor.icon.as_deref() == Some(record.id.as_str());
            icon_grid = icon_grid.child(
                div()
                    .id(SharedString::from(format!("custom-icon-{}", record.id)))
                    .size(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .bg(if selected {
                        rgba((palette.primary << 8) | 0x26)
                    } else {
                        rgba(0x00000000)
                    })
                    .border_1()
                    .border_color(if selected {
                        rgb(palette.primary)
                    } else {
                        rgba(0x00000000)
                    })
                    .hover(|this| this.bg(rgb(palette.hover)))
                    .when(index == 0, |this| {
                        this.debug_selector(|| "connection-editor-custom-icon-first".to_string())
                    })
                    .tooltip(move |window, cx| {
                        NyaTooltip::new(record.name.clone()).build(window, cx)
                    })
                    .child(
                        div()
                            .size(px(16.))
                            .when(index == 0, |this| {
                                this.debug_selector(|| {
                                    "connection-editor-custom-image-first".to_string()
                                })
                            })
                            .child(gpui::img(image).size(px(16.))),
                    )
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.set_connection_editor_icon(Some(&id), cx)
                    })),
            );
        }
        let selected_image = editor
            .icon
            .as_ref()
            .and_then(|id| self.connection_state.custom_icons.images.get(id))
            .cloned();
        let icon_picker_trigger = div()
            .id("connection-editor-icon-trigger")
            .size(px(32.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .border_1()
            .border_color(if icon_picker_open {
                rgb(palette.primary)
            } else {
                rgb(palette.border)
            })
            .bg(rgb(palette.input_surface()))
            .cursor_pointer()
            .hover(|this| this.bg(rgb(palette.hover)))
            .child(if let Some(image) = selected_image {
                gpui::img(image).size(px(17.)).into_any_element()
            } else {
                themed_icon(palette, icon_def, false, 17.).into_any_element()
            });
        let icon_picker_content = div()
            .debug_selector(|| "connection-editor-icon-popover-content".to_string())
            .occlude()
            .flex()
            .flex_col()
            // Seven 28 px icons plus gaps and a separate 16 px scrollbar track.
            .w(px(256.))
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.surface))
            .shadow_lg()
            .child(
                div()
                    .h(px(icon_viewport_height))
                    .overflow_y_scrollbar()
                    .child(icon_grid),
            )
            .child(
                nyaterm_ui::NyaButton::new("import-custom-icon", t!("dialog.importCustomIcon"))
                    .on_click(cx.listener(|app, _, window, cx| {
                        app.import_connection_custom_icon(window, cx)
                    })),
            )
            .when_some(custom_selected, |this, id| {
                this.child(
                    nyaterm_ui::NyaButton::new("delete-custom-icon", t!("dialog.deleteCustomIcon"))
                        .on_click(cx.listener(move |app, _, _, cx| {
                            app.delete_connection_custom_icon(id.clone(), cx)
                        })),
                )
            })
            .when(editor.kind == ConnectionKindTab::Ssh, |this| {
                this.child(
                    div()
                        .id("connection-editor-icon-auto-detect")
                        .debug_selector(|| "connection-editor-icon-auto-detect".to_string())
                        .mt_2()
                        .pt_2()
                        .border_t_1()
                        .border_color(rgb(palette.border))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .opacity(if icon_auto_detect_enabled { 1.0 } else { 0.55 })
                        .when(icon_auto_detect_enabled, |this| {
                            this.cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_connection_editor_icon_auto_detect(
                                        !icon_auto_detect,
                                        cx,
                                    );
                                }))
                        })
                        .when(!icon_auto_detect_enabled, |this| {
                            let tooltip = icon_auto_detect_hint.clone();
                            this.tooltip(move |window, cx| {
                                NyaTooltip::new(tooltip.clone()).build(window, cx)
                            })
                        })
                        .child(
                            div()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(palette.text))
                                        .child(icon_auto_detect_label),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(rgb(palette.text_dimmed))
                                        .child(icon_auto_detect_hint),
                                ),
                        )
                        .child(
                            NyaSwitch::new("connection-editor-icon-auto-detect-switch")
                                .checked(icon_auto_detect)
                                .disabled(!icon_auto_detect_enabled),
                        ),
                )
            });
        let icon_picker = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(icon_label),
            )
            .child(
                NyaPopover::new(
                    "connection-editor-icon-popover",
                    icon_picker_trigger,
                    icon_picker_content,
                )
                .appearance(false)
                .open(icon_picker_open)
                .on_open_change(cx.listener(|this, open, _, cx| {
                    this.set_connection_icon_picker_open(*open, cx);
                })),
            );

        let content = div()
            .id(SharedString::from(format!(
                "connection-editor-scroll-{}",
                editor.section.id()
            )))
            .debug_selector(|| "connection-editor-scroll".to_string())
            .relative()
            .min_w_0()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .on_scroll(cx.listener(|this, _: &gpui::ScrollEvent, window, cx| {
                if this.connection_editor_select_menu_is_focused(window, cx) {
                    cx.stop_propagation();
                }
            }))
            .p(px(20.))
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .id("connection-editor-section-heading")
                    .anchor_scroll(Some(heading_anchor))
                    .track_focus(&section_focus)
                    .tab_index(-1)
                    .debug_selector(|| "connection-editor-section-title".to_string())
                    .text_size(px(15.))
                    .font_weight(FontWeight(600.))
                    .child(editor.section.label()),
            )
            .when(editor.section == ConnectionEditorSection::Basic, |this| {
                this.child(
                    div()
                        .flex()
                        .when(!narrow, |this| this.flex_wrap())
                        .when(narrow, |this| this.flex_col())
                        .items_end()
                        .when(narrow, |this| this.items_stretch())
                        .gap_3()
                        .child(icon_picker)
                        .child(div().min_w(px(192.)).flex_1().child(editor_field(
                            palette,
                            name_label,
                            ConnectionEditorField::Name,
                            &fields,
                            cx,
                        )))
                        .child(
                            div()
                                .min_w(px(192.))
                                .when(!narrow, |this| this.max_w(px(288.)))
                                .flex_1()
                                .child(connection_editor_group_control(
                                    ConnectionEditorGroupControlArgs {
                                        palette,
                                        label: group_title,
                                        display_label: connection_editor_group_display_label(
                                            self.connection_state.groups(),
                                            &editor,
                                            none_label,
                                            t!("dialog.newGroup"),
                                        ),
                                        select_open: group_select_open,
                                        trigger_bounds: self
                                            .connection_state
                                            .editor_group_select_trigger_bounds(),
                                        parent_hint: group_parent_hint.clone(),
                                        choices: &group_options,
                                        fields: &fields,
                                    },
                                    cx,
                                )),
                        ),
                )
            })
            .when(editor.kind == ConnectionKindTab::Ssh, |this| {
                this.child(connection_editor_ssh_section(
                    section_context,
                    SshConnectionSectionLabels {
                        otp: otp_label.clone(),
                        proxy: proxy_label.clone(),
                        jump: jump_label.clone(),
                    },
                    SshConnectionSectionOptions { auth: auth_options },
                    cx,
                ))
            })
            .when(editor.kind == ConnectionKindTab::Adb, |this| {
                this.child(android::section(
                    section_context,
                    self.android.tracker.snapshot(),
                    cx,
                ))
            })
            .when(editor.kind == ConnectionKindTab::Local, |this| {
                this.child(connection_editor_local_section(section_context, cx))
            })
            .when(editor.kind == ConnectionKindTab::Telnet, |this| {
                this.child(connection_editor_telnet_section(section_context, cx))
            })
            .when(editor.kind == ConnectionKindTab::Serial, |this| {
                this.child(connection_editor_serial_section(section_context, cx))
            })
            .when(editor.kind == ConnectionKindTab::Rdp, |this| {
                this.child(connection_editor_rdp_section(section_context, cx))
            })
            .when(editor.kind == ConnectionKindTab::Vnc, |this| {
                this.child(connection_editor_vnc_section(section_context, cx))
            })
            .when(editor.section == ConnectionEditorSection::Basic, |this| {
                this.child(tags::connection_tags_field(palette, &editor, &fields, cx))
            })
            .when(editor.section == ConnectionEditorSection::Basic, |this| {
                this.child(connection_description_field(
                    palette,
                    description_label,
                    &fields,
                    cx,
                ))
            });
        let card = layout::connection_editor_frame(
            layout::ConnectionEditorFrame {
                palette,
                editor: &editor,
                title,
                content: content.into_any_element(),
                navigation_focus: self.connection_state.editor_navigation_focus(),
                editor_focus: self.connection_state.editor_focus_handle(),
                scroll,
                native_window,
                height,
                wide,
                navigation_width,
                protocol_select,
                validation_error,
                save_enabled,
            },
            cx,
        );
        let surface = div()
            .relative()
            .w_full()
            .when(native_window, |this| this.size_full())
            .overflow_hidden()
            .bg(rgb(palette.bg))
            .child(card)
            .when(agent_identity_picker_open, |this| {
                this.child(connection_editor_agent_identity_picker(
                    palette, &editor, cx,
                ))
            })
            .when_some(credential_overlay, |this, overlay| {
                this.child(self.connection_editor_credential_manager(overlay, cx))
            });
        if native_window {
            surface.into_any_element()
        } else {
            modal_dialog_shell(
                palette,
                self.shell_surface_color(palette.bg),
                "connection-editor-modal",
                900.,
                surface,
            )
            .into_any_element()
        }
    }
}

fn connection_description_field(
    palette: crate::theme::ThemePalette,
    label: impl Into<SharedString>,
    fields: &ConnectionEditorFields,
    cx: &App,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let entity = fields.get(&ConnectionEditorField::Description);
    let handle = entity.map(|field| field.read(cx).focus_handle());
    let focused = entity.is_some_and(|field| field.read(cx).has_focus());
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(label),
        )
        .child(
            div()
                .id("connection-editor-description")
                .debug_selector(|| "connection-editor-description".to_string())
                // Fixed: in a flex column the box would otherwise shrink to
                // whatever space the form had left, cutting a row in half.
                .h(px(56.))
                .flex_none()
                .min_w_0()
                .overflow_hidden()
                .rounded_sm()
                .border_1()
                .border_color(ordinary_input_shell_border_color(palette, focused))
                .when(focused, |this| {
                    this.shadow(ordinary_input_focus_ring(palette))
                })
                .bg(rgb(palette.input_surface()))
                .px(px(ORDINARY_INPUT_SHELL_PADDING_X_PX))
                // Textarea supplies its own top padding; adding shell padding
                // here pushes the first line toward the center of this short box.
                .cursor_text()
                .when_some(handle, |this, handle| {
                    this.on_click(move |_, window, cx| {
                        window.focus(&handle, cx);
                    })
                })
                .children(entity.map(NyaInput::new)),
        )
}

fn connection_editor_footer_button(
    _palette: crate::theme::ThemePalette,
    id: &'static str,
    label: impl Into<SharedString>,
    primary: bool,
    enabled: bool,
    disabled_tooltip: Option<String>,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let mut button = nyaterm_ui::NyaButton::new(id, label)
        .height(px(36.))
        .variant(if primary {
            nyaterm_ui::NyaButtonVariant::Primary
        } else {
            nyaterm_ui::NyaButtonVariant::Secondary
        })
        .disabled(!enabled)
        .on_click(on_click);
    if !enabled && let Some(tooltip) = disabled_tooltip {
        button = button.tooltip(tooltip);
    }
    div().debug_selector(move || id.to_string()).child(button)
}

fn connection_proxy_jump_would_cycle(
    connections: &[SavedConnection],
    current_id: Option<&str>,
    candidate: &SavedConnection,
) -> bool {
    let Some(current_id) = current_id else {
        return false;
    };
    let mut seen = HashSet::new();
    let mut next_id = Some(candidate.id.clone());
    while let Some(id) = next_id {
        if id == current_id || !seen.insert(id.clone()) {
            return true;
        }
        next_id = connections
            .iter()
            .find(|connection| connection.id == id)
            .and_then(|connection| connection.network.as_ref())
            .and_then(|network| network.proxy_jump_id.clone());
    }
    false
}

#[cfg(test)]
mod tests;
