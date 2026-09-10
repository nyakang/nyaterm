use rust_i18n::t;

use gpui::{Context, KeyDownEvent, PathPromptOptions, SharedString, Window};
use nyaterm_core::{
    Group, RdpClipboardSettings, RdpDisplaySettings, RdpReconnectSettings, RdpSecuritySettings,
    VncClipboardSettings, VncDisplaySettings, VncReconnectSettings, VncSecuritySettings, uuid,
};
use nyaterm_store::{StoreDomain, store_request};

use super::helpers::{
    ConnectionEditorToggle, ConnectionEditorValidationError, build_saved_connection_from_editor,
    connection_editor_from_saved,
};
use crate::features::NyaTermApp;
use crate::models::{
    ConnectionEditorAdvancedTab, ConnectionEditorField, ConnectionEditorPasswordSource,
    ConnectionEditorRdpTab, ConnectionEditorSelect, ConnectionEditorSshAlgorithmTab,
    ConnectionEditorState, ConnectionEditorTelnetTab, ConnectionKindTab,
};

impl NyaTermApp {
    pub(in crate::features) fn connection_editor_validation_error(
        &self,
        editor: &ConnectionEditorState,
    ) -> Option<String> {
        build_saved_connection_from_editor(editor)
            .err()
            .map(|error| self.connection_editor_validation_message(&error))
    }

    fn connection_editor_validation_message(
        &self,
        error: &ConnectionEditorValidationError,
    ) -> String {
        use nyaterm_transport::{SshAlgorithmListKind, SshAlgorithmValidationError};

        let algorithm_kind = |kind: SshAlgorithmListKind| match kind {
            SshAlgorithmListKind::KeyExchange => t!("dialog.algorithmKex"),
            SshAlgorithmListKind::Cipher => t!("dialog.algorithmCiphers"),
            SshAlgorithmListKind::Mac => t!("dialog.algorithmMacs"),
            SshAlgorithmListKind::HostKey => t!("dialog.algorithmHostKeys"),
        };
        match error {
            ConnectionEditorValidationError::HostRequired => t!("dialog.hostRequired").into(),
            ConnectionEditorValidationError::PortInvalid => t!("dialog.portInvalid").into(),
            ConnectionEditorValidationError::UsernameRequired => {
                t!("dialog.usernameRequired").into()
            }
            ConnectionEditorValidationError::ShellPathRequired => {
                t!("dialog.shellPathRequired").into()
            }
            ConnectionEditorValidationError::SerialPortRequired => {
                t!("dialog.serialPortRequired").into()
            }
            ConnectionEditorValidationError::BaudRateInvalid => {
                t!("dialog.baudRateInvalid", min = "1", max = "4000000").into()
            }
            ConnectionEditorValidationError::RdpDisplayWidthInvalid => {
                t!("dialog.rdpDisplayWidthInvalid").into()
            }
            ConnectionEditorValidationError::RdpDisplayHeightInvalid => {
                t!("dialog.rdpDisplayHeightInvalid").into()
            }
            ConnectionEditorValidationError::RdpReconnectAttemptsInvalid => {
                t!("dialog.rdpReconnectAttemptsInvalid").into()
            }
            ConnectionEditorValidationError::VncReconnectAttemptsInvalid => {
                "VNC reconnect attempts must be between 0 and 20".to_string()
            }
            ConnectionEditorValidationError::PostLoginCommandRequired => {
                t!("dialog.postLoginCommandRequired").into()
            }
            ConnectionEditorValidationError::PostLoginDelayInvalid => {
                t!("dialog.postLoginDelayInvalid", min = "0", max = "60000").into()
            }
            ConnectionEditorValidationError::SftpShellDetectionTimeoutInvalid => t!(
                "dialog.sftpShellDetectionTimeoutInvalid",
                min = "100",
                max = "60000"
            )
            .into(),
            ConnectionEditorValidationError::SshAgentEndpoint(error) => match error {
                nyaterm_core::SshAgentEndpointValidationError::Empty => {
                    t!("dialog.sshAgentEndpointEmpty").into()
                }
                nyaterm_core::SshAgentEndpointValidationError::Invalid => {
                    t!("dialog.sshAgentEndpointInvalid").into()
                }
                nyaterm_core::SshAgentEndpointValidationError::TooLong => {
                    t!("dialog.sshAgentEndpointTooLong").into()
                }
                nyaterm_core::SshAgentEndpointValidationError::DuplicateEndpoint => {
                    t!("dialog.sshAgentEndpointDuplicate").into()
                }
                nyaterm_core::SshAgentEndpointValidationError::TooManyEndpoints
                | nyaterm_core::SshAgentEndpointValidationError::TooManyIdentities
                | nyaterm_core::SshAgentEndpointValidationError::InvalidFingerprint
                | nyaterm_core::SshAgentEndpointValidationError::DuplicateFingerprint => {
                    t!("dialog.sshAgentEndpointInvalid").into()
                }
            },
            ConnectionEditorValidationError::SshAlgorithms(
                SshAlgorithmValidationError::EmptyList { kind },
            ) => t!(
                "dialog.algorithmListRequired",
                category = algorithm_kind(*kind)
            )
            .into(),
            ConnectionEditorValidationError::SshAlgorithms(
                SshAlgorithmValidationError::Unsupported { kind, algorithm },
            ) => t!(
                "dialog.algorithmUnsupportedError",
                algorithm = algorithm,
                category = algorithm_kind(*kind)
            )
            .into(),
        }
    }

    pub(in crate::features) fn open_connection_editor(
        &mut self,
        connection_id: Option<String>,
        parent_group_id: Option<String>,
        connect_after_save: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.refresh_connection_auth_catalog(cx);
        let editor = if let Some(connection_id) = connection_id {
            let Some(connection) = self
                .connection_state
                .connections()
                .iter()
                .find(|connection| connection.id == connection_id)
                .cloned()
            else {
                self.shell
                    .set_status(t!("dialog.connectionNotFound").to_string());
                cx.notify();
                return;
            };
            connection_editor_from_saved(connection, connect_after_save)
        } else {
            ConnectionEditorState {
                id: None,
                kind: ConnectionKindTab::Ssh,
                name: String::new(),
                description: String::new(),
                icon: None,
                // A new connection has no icon yet, so let the first successful
                // SSH session fill one in.
                icon_auto_detect: true,
                group_id: parent_group_id.filter(|value| !value.trim().is_empty()),
                new_group_name: String::new(),
                pending_group_name: None,
                pending_group_parent_id: None,
                host: String::new(),
                port: "22".to_string(),
                username: "root".to_string(),
                domain: String::new(),
                auth_mode: "password".to_string(),
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
                password_source: ConnectionEditorPasswordSource::Ask,
                password_id: None,
                password: nyaterm_core::SecretString::default(),
                existing_password: None,
                key_id: None,
                otp_id: None,
                auto_fill_otp: false,
                proxy_id: None,
                proxy_jump_id: None,
                x11_forwarding: false,
                dynamic_tab_title: false,
                agent_endpoint: Default::default(),
                agent_forwarding_config: nyaterm_core::SshAgentForwardingConfig::default(),
                agent_allow_all_confirmed: false,
                agent_forwarding_endpoint_index: 0,
                agent_preview: None,
                agent_preview_loading: false,
                backspace_mode: "del".to_string(),
                encoding: "global".to_string(),
                ssh_profile: Default::default(),
                terminal_type: None,
                sftp_enabled: true,
                sftp_cwd_follow_mode: "shell_integration".to_string(),
                sftp_shell_detection_timeout_ms: "3000".to_string(),
                sftp_pipeline_depth: None,
                sftp_extra: Default::default(),
                sftp_filename_encoding: "terminal".to_string(),
                ssh_algorithm_mode: "compatible".to_string(),
                ssh_algorithm_kex: Vec::new(),
                ssh_algorithm_ciphers: Vec::new(),
                ssh_algorithm_macs: Vec::new(),
                ssh_algorithm_host_keys: Vec::new(),
                ssh_algorithm_tab: ConnectionEditorSshAlgorithmTab::KeyExchange,
                shell_path: String::new(),
                shell_args: String::new(),
                working_dir: String::new(),
                serial_port: self
                    .connection_state
                    .serial_ports()
                    .first()
                    .cloned()
                    .unwrap_or_default(),
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
                telnet_auto_login_enabled: true,
                telnet_auto_login_send_wake_enter: true,
                telnet_auto_login_timeout_ms: "60000".to_string(),
                telnet_auto_login_username_prompt_regex: String::new(),
                telnet_auto_login_password_prompt_regex: String::new(),
                telnet_auto_login_success_prompt_regex: String::new(),
                telnet_auto_login_failure_prompt_regex: String::new(),
                telnet_auto_login_max_retries: "0".to_string(),
                post_login_enabled: false,
                post_login_command: String::new(),
                post_login_delay_ms: "1000".to_string(),
                recording: None,
                advanced_open: false,
                advanced_network_tab: ConnectionEditorAdvancedTab::Proxy,
                advanced_behavior_tab: ConnectionEditorAdvancedTab::PostLogin,
                telnet_advanced_tab: ConnectionEditorTelnetTab::Input,
                connect_after_save,
                focused_field: ConnectionEditorField::Name,
                error: None,
            }
        };

        self.connection_state.begin_editor(editor);
        // Fields mirror the draft, so they are rebuilt with it.
        self.connection_state.build_editor_fields(cx);
        self.shell
            .set_status(t!("dialog.connectionEditorOpened").to_string());
        if !self.open_connection_editor_window(cx) {
            // Land on the name and select it, so an edit can start by typing.
            match self
                .connection_state
                .editor_fields()
                .get(&ConnectionEditorField::Name)
                .cloned()
            {
                Some(field) => {
                    window.focus(&field.read(cx).focus_handle(), cx);
                    field.update(cx, |field, cx| field.select_all(window, cx));
                }
                None => {
                    let editor_focus = self.connection_state.editor_focus_handle();
                    window.focus(&editor_focus, cx);
                }
            }
        }
        cx.notify();
    }

    pub(in crate::features) fn close_connection_editor(&mut self, cx: &mut Context<Self>) {
        self.connection_state.close_editor();
        self.connection_state.clear_editor_fields();
        self.shell
            .set_status(t!("dialog.connectionEditorClosed").to_string());
        cx.notify();
    }

    pub(in crate::features) fn set_connection_icon_picker_open(
        &mut self,
        open: bool,
        cx: &mut Context<Self>,
    ) {
        if self.connection_state.set_editor_icon_picker_open(open) {
            cx.notify();
        }
    }

    pub(in crate::features) fn set_connection_editor_agent_identity_picker_open(
        &mut self,
        open: bool,
        cx: &mut Context<Self>,
    ) {
        if self
            .connection_state
            .set_editor_agent_identity_picker_open(open)
        {
            cx.notify();
        }
    }

    pub(in crate::features) fn toggle_connection_group_select(&mut self, cx: &mut Context<Self>) {
        self.connection_state.toggle_editor_group_select();
        cx.notify();
    }

    pub(in crate::features) fn close_connection_group_select(&mut self, cx: &mut Context<Self>) {
        self.connection_state.close_editor_group_select();
        cx.notify();
    }

    pub(in crate::features) fn set_connection_group_select_trigger_bounds(
        &mut self,
        bounds: gpui::Bounds<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self
            .connection_state
            .set_editor_group_select_trigger_bounds(bounds)
        {
            cx.notify();
        }
    }

    pub(in crate::features) fn set_connection_editor_icon(
        &mut self,
        icon: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        self.connection_state.set_editor_icon(icon);
        cx.notify();
    }

    pub(in crate::features) fn set_connection_editor_icon_auto_detect(
        &mut self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if self.connection_state.set_editor_icon_auto_detect(enabled) {
            cx.notify();
        }
    }

    pub(in crate::features) fn set_connection_editor_select_value(
        &mut self,
        select: ConnectionEditorSelect,
        value: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        self.connection_state
            .set_editor_select_value(select, value.map(ToOwned::to_owned));
        cx.notify();
    }

    pub(in crate::features) fn set_connection_editor_password_source(
        &mut self,
        source: ConnectionEditorPasswordSource,
        cx: &mut Context<Self>,
    ) {
        self.connection_state.set_editor_password_source(source);
        cx.notify();
    }

    pub(in crate::features) fn set_connection_editor_advanced_tab(
        &mut self,
        tab: ConnectionEditorAdvancedTab,
        cx: &mut Context<Self>,
    ) {
        self.connection_state.set_editor_advanced_tab(tab);
        cx.notify();
    }

    pub(in crate::features) fn set_connection_editor_ssh_algorithm_tab(
        &mut self,
        tab: ConnectionEditorSshAlgorithmTab,
        cx: &mut Context<Self>,
    ) {
        if self.connection_state.set_editor_ssh_algorithm_tab(tab) {
            cx.notify();
        }
    }

    pub(in crate::features) fn set_connection_editor_ssh_algorithm_enabled(
        &mut self,
        tab: ConnectionEditorSshAlgorithmTab,
        id: &str,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if self
            .connection_state
            .set_editor_ssh_algorithm_enabled(tab, id, enabled)
        {
            cx.notify();
        }
    }

    pub(in crate::features) fn move_connection_editor_ssh_algorithm(
        &mut self,
        tab: ConnectionEditorSshAlgorithmTab,
        id: &str,
        direction: i8,
        cx: &mut Context<Self>,
    ) {
        if self
            .connection_state
            .move_editor_ssh_algorithm(tab, id, direction)
        {
            cx.notify();
        }
    }

    pub(in crate::features) fn add_connection_editor_agent_endpoint(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.connection_state.add_editor_agent_endpoint() {
            self.connection_state
                .rebuild_editor_forwarding_endpoint_fields(cx);
            cx.notify();
        }
    }

    pub(in crate::features) fn remove_connection_editor_agent_endpoint(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        if self.connection_state.remove_editor_agent_endpoint(index) {
            self.connection_state
                .rebuild_editor_forwarding_endpoint_fields(cx);
            cx.notify();
        }
    }

    pub(in crate::features) fn select_connection_editor_agent_endpoint(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        if self.connection_state.select_editor_agent_endpoint(index) {
            self.connection_state.sync_editor_fields_from_draft(cx);
            cx.notify();
        }
    }

    pub(in crate::features) fn set_connection_editor_agent_endpoint_type(
        &mut self,
        index: usize,
        value: &str,
        cx: &mut Context<Self>,
    ) {
        if self
            .connection_state
            .set_editor_agent_endpoint_type(index, value)
        {
            self.connection_state
                .rebuild_editor_forwarding_endpoint_fields(cx);
            cx.notify();
        }
    }

    pub(in crate::features) fn set_connection_editor_agent_endpoint_field(
        &mut self,
        index: usize,
        field: ConnectionEditorField,
        text: String,
        cx: &mut Context<Self>,
    ) {
        if self
            .connection_state
            .set_editor_agent_endpoint_field(index, field, text)
        {
            cx.notify();
        }
    }

    pub(in crate::features) fn move_connection_editor_agent_endpoint(
        &mut self,
        index: usize,
        direction: i8,
        cx: &mut Context<Self>,
    ) {
        if self
            .connection_state
            .move_editor_agent_endpoint(index, direction)
        {
            cx.notify();
        }
    }

    pub(in crate::features) fn toggle_connection_editor_agent_allowlist_fingerprint(
        &mut self,
        fingerprint: &str,
        cx: &mut Context<Self>,
    ) {
        if self
            .connection_state
            .toggle_editor_agent_allowlist_fingerprint(fingerprint)
        {
            cx.notify();
        }
    }

    pub(in crate::features) fn set_connection_editor_telnet_tab(
        &mut self,
        tab: ConnectionEditorTelnetTab,
        cx: &mut Context<Self>,
    ) {
        self.connection_state.set_editor_telnet_tab(tab);
        cx.notify();
    }

    pub(in crate::features) fn set_connection_editor_rdp_tab(
        &mut self,
        tab: ConnectionEditorRdpTab,
        cx: &mut Context<Self>,
    ) {
        self.connection_state.set_editor_rdp_tab(tab);
        cx.notify();
    }

    /// Take an edit from a field widget into the draft.
    pub(in crate::features) fn apply_connection_editor_field_text(
        &mut self,
        field: ConnectionEditorField,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.connection_state.set_editor_field_text(field, text);
        cx.notify();
    }

    pub(in crate::features) fn commit_connection_editor_new_group(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let required_message = t!("dialog.groupNameRequired").to_string();
        if self
            .connection_state
            .commit_editor_new_group(required_message)
        {
            // The draft's copy is cleared by the commit; the box the name was
            // typed into holds its own buffer and has to be told.
            self.connection_state
                .reset_editor_field(ConnectionEditorField::NewGroupName, "", cx);
            cx.notify();
        }
    }

    pub(in crate::features) fn set_connection_editor_kind(
        &mut self,
        kind: ConnectionKindTab,
        cx: &mut Context<Self>,
    ) {
        if self.connection_state.set_editor_kind(kind) {
            // Switching kind rewrites the default port on the draft; the box has
            // to be told, or it keeps showing the other protocol's.
            self.connection_state.sync_editor_fields_from_draft(cx);
            let kind_label = match kind {
                ConnectionKindTab::Ssh => "SSH",
                ConnectionKindTab::Local => &t!("dialog.localTerminal"),
                ConnectionKindTab::Telnet => "Telnet",
                ConnectionKindTab::Serial => &t!("dialog.serial"),
                ConnectionKindTab::Rdp => "RDP",
                ConnectionKindTab::Vnc => "VNC",
            };
            self.shell
                .set_status(t!("dialog.connectionTypeChanged", "type" => kind_label));
        }
        cx.notify();
    }

    pub(in crate::features) fn prompt_connection_editor_shell_path(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let options = PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(SharedString::from(
                t!("dialog.selectShellExecutable").to_string(),
            )),
        };
        let cancelled_status = t!("dialog.shellPathSelectionCancelled").to_string();
        let receiver = cx.prompt_for_paths(options);
        self.shell
            .set_status(t!("dialog.selectingShellExecutable").to_string());
        cx.spawn(async move |this, cx| {
            let selected = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(path) = selected {
                    let path = path.display().to_string();
                    this.connection_state.apply_editor_shell_path(path.clone());
                    this.shell
                        .set_status(t!("dialog.shellPathSelected", path = path).to_string());
                } else {
                    this.shell.set_status(cancelled_status);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn prompt_connection_editor_working_dir(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let options = PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(SharedString::from(
                t!("dialog.selectWorkingDirectory").to_string(),
            )),
        };
        let cancelled_status = t!("dialog.workingDirectorySelectionCancelled").to_string();
        let receiver = cx.prompt_for_paths(options);
        self.shell
            .set_status(t!("dialog.selectingWorkingDirectory").to_string());
        cx.spawn(async move |this, cx| {
            let selected = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(path) = selected {
                    let path = path.display().to_string();
                    this.connection_state.apply_editor_working_dir(path.clone());
                    this.shell
                        .set_status(t!("dialog.workingDirectorySelected", path = path).to_string());
                } else {
                    this.shell.set_status(cancelled_status);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn toggle_connection_editor_flag(
        &mut self,
        flag: ConnectionEditorToggle,
        cx: &mut Context<Self>,
    ) {
        self.connection_state.toggle_editor_flag(flag);
        cx.notify();
    }

    pub(in crate::features) fn handle_connection_editor_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.mark_user_activity();
        let keystroke = &event.keystroke;
        if keystroke.modifiers.alt || keystroke.modifiers.function {
            return false;
        }

        match keystroke.key.as_str() {
            "escape" => {
                if self.connection_state.editor_icon_picker_is_open() {
                    self.connection_state.close_editor_icon_picker();
                    cx.notify();
                    return true;
                }
                if self.connection_state.editor_group_select_is_open() {
                    self.connection_state.close_editor_group_select();
                    cx.notify();
                    return true;
                }
                self.close_connection_editor(cx);
                return true;
            }
            "enter" => {
                if !keystroke.modifiers.platform
                    && !keystroke.modifiers.control
                    && self.connection_state.editor_description_is_focused()
                {
                    self.connection_state.insert_editor_description_newline();
                    cx.notify();
                    return true;
                }
                if self.connection_state.editor_new_group_field_is_focused(cx) {
                    self.commit_connection_editor_new_group(cx);
                    return true;
                }
                self.save_connection_editor(window, cx);
                return true;
            }
            "tab" if !keystroke.modifiers.platform && !keystroke.modifiers.control => {
                self.connection_state.advance_editor_focus();
                cx.notify();
                return true;
            }
            _ => {}
        }

        false
    }

    pub(in crate::features) fn save_connection_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut editor) = self.connection_state.active_editor_draft() else {
            return;
        };
        if editor.agent_forwarding_config.enabled
            && matches!(
                editor.agent_forwarding_config.policy,
                nyaterm_core::SshAgentForwardingPolicy::All
            )
            && !editor.agent_allow_all_confirmed
        {
            self.open_confirm_dialog(
                (
                    t!("dialog.sshAgentAllowAllConfirmTitle").to_string(),
                    t!("dialog.sshAgentAllowAllConfirmMessage").to_string(),
                    t!("dialog.sshAgentAllowAllConfirmAction").to_string(),
                    true,
                    |this: &mut NyaTermApp, window, cx| {
                        this.connection_state.confirm_editor_agent_allow_all();
                        this.save_connection_editor(window, cx);
                        true
                    },
                ),
                window,
                cx,
            );
            return;
        }

        let pending_group = editor.pending_group_name.as_ref().map(|name| Group {
            id: uuid(),
            name: name.clone(),
            parent_id: editor.pending_group_parent_id.clone(),
            sort_order: self.connection_state.groups().len() as i32,
            created_at_ms: None,
            updated_at_ms: None,
        });
        if let Some(group) = pending_group.as_ref() {
            editor.group_id = Some(group.id.clone());
        }

        let built = match build_saved_connection_from_editor(&editor) {
            Ok(connection) => connection,
            Err(error) => {
                let message = self.connection_editor_validation_message(&error);
                self.set_connection_editor_error(message, cx);
                return;
            }
        };

        let saved_id = built.id.clone();
        let persisted = built.clone();
        let connect_after_save = editor.connect_after_save;
        self.shell.set_status("saving connection...".to_string());
        self.submit_store_request(
            0,
            store_request(StoreDomain::Connections, move |store| {
                let mut persisted = persisted;
                if let Some(previous) = store.get_connection(&persisted.id)? {
                    persisted.extensions = previous.extensions;
                }
                if let Some(group) = &pending_group {
                    store.save_group_and_connection(group, &persisted)?;
                } else {
                    store.save_connection(&persisted)?;
                }
                store.load_sessions()
            }),
            move |this, event, cx| match event.outcome {
                Ok(sessions) => {
                    let saved = sessions
                        .connections
                        .iter()
                        .find(|connection| connection.id == saved_id)
                        .cloned();
                    this.apply_loaded_sessions(sessions, cx);
                    let Some(saved) = saved else {
                        this.set_connection_editor_error(
                            "saved connection was not returned by storage".to_string(),
                            cx,
                        );
                        return;
                    };
                    this.connection_state
                        .finish_editor_save(saved.id.clone(), saved.group_id.clone());
                    this.shell
                        .set_status(t!("dialog.connectionSaved").to_string());
                    if connect_after_save {
                        this.continue_saved_connection_start(saved, Default::default(), cx);
                    } else {
                        cx.notify();
                    }
                }
                Err(error) => {
                    let message = t!("dialog.connectionSaveFailed", error = error).to_string();
                    this.set_connection_editor_error(message, cx);
                }
            },
            cx,
        );
        cx.notify();
    }
}
