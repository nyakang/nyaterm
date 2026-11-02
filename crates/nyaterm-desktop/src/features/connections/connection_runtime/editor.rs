use rust_i18n::t;

use gpui::{Context, KeyDownEvent, PathPromptOptions, SharedString, Window};
use nyaterm_core::{
    Group, RdpClipboardSettings, RdpDisplaySettings, RdpReconnectSettings, RdpSecuritySettings,
    SavedConnection, SessionsConfig, VncClipboardSettings, VncDisplaySettings,
    VncReconnectSettings, VncSecuritySettings, uuid,
};
use nyaterm_store::{ConnectionStore, StorageError, StoreDomain};

use super::helpers::{
    ConnectionEditorToggle, ConnectionEditorValidationError, build_saved_connection_from_editor,
    connection_editor_from_saved, validate_editor_account,
};
use crate::features::NyaTermApp;
use crate::models::{
    ConnectionEditorCredentialOverlay, ConnectionEditorField, ConnectionEditorForwardingTab,
    ConnectionEditorPasswordSource, ConnectionEditorSelect, ConnectionEditorSshAlgorithmTab,
    ConnectionEditorState, ConnectionKindTab,
};

fn save_editor_connection(
    store: &ConnectionStore,
    mut persisted: SavedConnection,
    pending_group: Option<&Group>,
    password_edit: Option<crate::models::ConnectionPasswordEdit>,
) -> Result<SessionsConfig, StorageError> {
    // Read metadata in the store job so edits cannot restore stale usage or
    // ordering values captured when the editor was opened.
    let previous = store.get_connection(&persisted.id)?;
    if password_edit == Some(crate::models::ConnectionPasswordEdit::Keep)
        && let Some(auth) = persisted.auth.as_mut()
        && auth.mode == "password"
    {
        // Locking discards plaintext in the UI. Keep the latest stored secret,
        // including its ciphertext marker, rather than serializing the empty input.
        let original = previous.as_ref().and_then(|entry| entry.auth.as_ref());
        auth.password = original.and_then(|auth| auth.password.clone());
        auth.has_password = original.is_some_and(|auth| auth.has_password);
    }
    persisted.sort_order = if let Some(previous) = previous.as_ref()
        && previous.group_id == persisted.group_id
    {
        previous.sort_order
    } else {
        store
            .list_connections()?
            .iter()
            .filter(|connection| connection.group_id == persisted.group_id)
            .map(|connection| connection.sort_order)
            .max()
            .map_or(0, |order| order.saturating_add(1))
    };
    if let Some(previous) = previous {
        persisted.last_used_at_ms = previous.last_used_at_ms;
        persisted.extensions = previous.extensions;
        persisted.asset = previous.asset;
    }
    if let Some(group) = pending_group {
        store.save_group_and_connection(group, &persisted)?;
    } else {
        store.save_connection(&persisted)?;
    }
    store.load_sessions()
}

#[cfg(test)]
mod tests;

impl NyaTermApp {
    pub(in crate::features) fn connection_editor_validation_error(
        &self,
        editor: &ConnectionEditorState,
    ) -> Option<String> {
        build_saved_connection_from_editor(editor)
            .and_then(|_| validate_editor_account(editor, self.security.passwords()))
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
            ConnectionEditorValidationError::MissingAccount => {
                t!("passwordManager.missingAccount").into()
            }
            ConnectionEditorValidationError::HostRequired => t!("dialog.hostRequired").into(),
            ConnectionEditorValidationError::PortInvalid => t!("dialog.portInvalid").into(),
            ConnectionEditorValidationError::TelnetKeepAliveIntervalInvalid => {
                t!("dialog.telnetKeepAliveIntervalInvalid").into()
            }
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
            ConnectionEditorValidationError::RdpWindowsAuthenticationRequired => {
                t!("dialog.rdpWindowsAuthenticationRequired").into()
            }
            ConnectionEditorValidationError::RdpRelayUnsupported => {
                t!("dialog.rdpActiveXRelayUnsupported").into()
            }
            ConnectionEditorValidationError::RdpClipboardUnsupported => {
                t!("dialog.rdpActiveXClipboardUnsupported").into()
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
            ConnectionEditorValidationError::VncUsernameTooLong => {
                t!("dialog.vncUsernameTooLong").into()
            }
            ConnectionEditorValidationError::VncPasswordTooLong => {
                t!("dialog.vncPasswordTooLong").into()
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
            ConnectionEditorValidationError::SshAgentEndpoint(error)
            | ConnectionEditorValidationError::SshAgentForwarding { error, .. } => match error {
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
                tags: Vec::new(),
                new_tag: String::new(),
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
                rdp_backend: "builtin".to_string(),
                rdp_security: RdpSecuritySettings::default(),
                rdp_display: RdpDisplaySettings::default(),
                rdp_clipboard: RdpClipboardSettings::default(),
                rdp_reconnect: RdpReconnectSettings::default(),

                vnc_security: VncSecuritySettings::default(),
                vnc_display: VncDisplaySettings::default(),
                vnc_clipboard: VncClipboardSettings::default(),
                vnc_reconnect: VncReconnectSettings::default(),
                vnc_shared: true,
                vnc_view_only: false,
                password_source: ConnectionEditorPasswordSource::Ask,
                password_id: None,
                account_id: None,
                username_source: nyaterm_core::ConnectionUsernameSource::Connection,
                password_source_touched: false,
                password_source_demoted: false,
                password_edit: crate::models::ConnectionPasswordEdit::Keep,
                field_errors: Default::default(),
                password: nyaterm_core::SecretString::default(),
                existing_password: None,
                existing_password_locked: false,
                key_id: None,
                otp_id: None,
                auto_fill_otp: false,
                proxy_id: None,
                proxy_jump_id: None,
                host_key_alias: None,
                x11_forwarding: false,
                dynamic_tab_title: false,
                agent_endpoint: Default::default(),
                agent_forwarding_config: nyaterm_core::SshAgentForwardingConfig::default(),
                agent_allow_all_confirmed: false,
                agent_forwarding_endpoint_index: 0,
                agent_preview: None,
                agent_preview_loading: false,
                backspace_mode: "del".to_string(),
                encoding: String::new(),
                ssh_profile: Default::default(),
                terminal_type: None,
                sftp_enabled: true,
                sftp_compatibility_mode: false,
                sftp_cwd_follow_mode: "shell_integration".to_string(),
                sftp_shell_detection_timeout_ms: "3000".to_string(),
                sftp_pipeline_depth: None,
                sftp_extra: Default::default(),
                sftp_filename_encoding: String::new(),
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
                flow_control: Default::default(),
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
                telnet_keep_alive_interval_secs:
                    nyaterm_core::models::connection::DEFAULT_TELNET_KEEP_ALIVE_INTERVAL_SECS
                        .to_string(),
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

                forwarding_tab: ConnectionEditorForwardingTab::Agent,

                connect_after_save,
                section: crate::models::ConnectionEditorSection::Basic,
                focused_field: ConnectionEditorField::Name,
                error: None,
            }
        };

        self.connection_state.begin_editor(editor);
        self.reconcile_open_editor_password_source();
        if self.security_secrets_locked() {
            self.connection_state.clear_editor_secrets(cx);
        }
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

    pub(in crate::features) fn set_connection_editor_credential_overlay(
        &mut self,
        overlay: Option<ConnectionEditorCredentialOverlay>,
        cx: &mut Context<Self>,
    ) {
        if self.connection_state.set_editor_credential_overlay(overlay) {
            self.refresh_connection_auth_catalog(cx);
            cx.notify();
        }
    }

    pub(in crate::features) fn set_connection_editor_baud_popover_open(
        &mut self,
        open: bool,
        cx: &mut Context<Self>,
    ) {
        if self.connection_state.set_editor_baud_popover_open(open) {
            cx.notify();
        }
    }

    pub(in crate::features) fn set_connection_editor_baud_rate(
        &mut self,
        value: String,
        cx: &mut Context<Self>,
    ) {
        self.connection_state
            .set_editor_field_text(ConnectionEditorField::BaudRate, value.clone());
        self.connection_state
            .reset_editor_field(ConnectionEditorField::BaudRate, &value, cx);
        self.connection_state.set_editor_baud_popover_open(false);
        cx.notify();
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

    pub(in crate::features) fn add_connection_editor_tag(&mut self, cx: &mut Context<Self>) {
        if self.connection_state.add_editor_tag(cx) {
            self.connection_state
                .reset_editor_field(ConnectionEditorField::NewTag, "", cx);
            cx.notify();
        }
    }

    pub(in crate::features) fn remove_connection_editor_tag(
        &mut self,
        tag: &str,
        cx: &mut Context<Self>,
    ) {
        if self.connection_state.remove_editor_tag(tag, cx) {
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
        if matches!(
            select,
            ConnectionEditorSelect::Authentication
                | ConnectionEditorSelect::SavedPassword
                | ConnectionEditorSelect::Account
                | ConnectionEditorSelect::SshKey
                | ConnectionEditorSelect::Otp
        ) {
            self.connection_state.hide_editor_secrets(cx);
        }
        let account_selection = select == ConnectionEditorSelect::Account
            || (select == ConnectionEditorSelect::SavedPassword
                && self
                    .connection_state
                    .active_editor_draft()
                    .is_some_and(|editor| {
                        matches!(
                            editor.kind,
                            ConnectionKindTab::Ssh | ConnectionKindTab::Telnet
                        )
                    }));
        let previous_account = account_selection.then(|| {
            let id = self
                .connection_state
                .active_editor_draft()
                .and_then(|editor| editor.account_id.clone());
            let username = id.as_ref().and_then(|id| {
                self.security
                    .passwords()
                    .iter()
                    .find(|entry| entry.id == *id)
                    .map(|entry| entry.username.clone())
            });
            (id, username)
        });
        self.connection_state
            .set_editor_select_value(select, value.map(ToOwned::to_owned));
        if let Some((previous_id, previous_username)) = previous_account {
            let selected = value.and_then(|id| {
                self.security
                    .passwords()
                    .iter()
                    .find(|entry| entry.id == id)
                    .cloned()
            });
            self.connection_state.apply_editor_account_selection(
                previous_id.as_deref(),
                previous_username.as_deref(),
                selected.as_ref(),
                cx,
            );
        }

        if matches!(
            select,
            ConnectionEditorSelect::Account | ConnectionEditorSelect::SavedPassword
        ) {
            let id = self
                .connection_state
                .active_editor_draft()
                .and_then(|draft| draft.password_account_id().map(str::to_owned));
            let source = if let Some(id) = id {
                match self
                    .security
                    .passwords()
                    .iter()
                    .find(|entry| entry.id == id)
                {
                    Some(entry) if !entry.has_password => ConnectionEditorPasswordSource::Ask,
                    _ => ConnectionEditorPasswordSource::Saved,
                }
            } else {
                ConnectionEditorPasswordSource::Direct
            };
            self.connection_state.set_editor_password_source(source);
            self.connection_state.sync_editor_password_input(cx);
        }

        cx.notify();
    }

    pub(in crate::features) fn use_connection_editor_username(&mut self, cx: &mut Context<Self>) {
        let username = self.editor_bound_account().and_then(|account| {
            let username = account.username.trim();
            (!username.is_empty()).then(|| username.to_string())
        });
        self.connection_state
            .use_connection_username(username.as_deref(), cx);
        cx.notify();
    }

    pub(in crate::features) fn follow_connection_editor_account_username(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.connection_state.follow_account_username();
        cx.notify();
    }

    pub(in crate::features) fn editor_account_username_if_following(
        &self,
        id: &str,
    ) -> Option<String> {
        let editor = self.connection_state.active_editor_draft()?;
        if !matches!(
            editor.kind,
            ConnectionKindTab::Ssh | ConnectionKindTab::Telnet
        ) {
            return None;
        }
        if editor.account_id.as_deref() != Some(id)
            || editor.username_source != nyaterm_core::ConnectionUsernameSource::Account
        {
            return None;
        }
        let username = self
            .security
            .passwords()
            .iter()
            .find(|entry| entry.id == id)?
            .username
            .trim();
        (!username.is_empty()).then(|| username.to_string())
    }

    pub(in crate::features) fn adopt_released_account_username(
        &mut self,
        username: &str,
        cx: &mut Context<Self>,
    ) {
        self.connection_state
            .use_connection_username(Some(username), cx);
        cx.notify();
    }

    pub(in crate::features) fn reconcile_open_editor_password_source(&mut self) {
        let Some(editor) = self.connection_state.active_editor_draft() else {
            return;
        };
        if editor.password_source != ConnectionEditorPasswordSource::Saved {
            return;
        }
        let id = match editor.kind {
            ConnectionKindTab::Ssh | ConnectionKindTab::Telnet => editor.account_id.clone(),
            _ => editor.password_id.clone(),
        };
        let account = id.as_ref().and_then(|id| {
            self.security
                .passwords()
                .iter()
                .find(|entry| entry.id == *id)
        });
        if let Some(account) = account {
            self.connection_state
                .demote_saved_password_if_unavailable(account.has_password);
        }
    }

    fn editor_bound_account(&self) -> Option<nyaterm_core::SavedPassword> {
        let id = self
            .connection_state
            .active_editor_draft()?
            .account_id
            .clone()?;
        self.security
            .passwords()
            .iter()
            .find(|entry| entry.id == id)
            .cloned()
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
            self.connection_state.hide_editor_secrets(cx);
            self.connection_state
                .select_editor_section(crate::models::ConnectionEditorSection::Basic);
            self.dismiss_connection_editor_menus(cx);
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
                    this.connection_state.reset_editor_field(
                        ConnectionEditorField::ShellPath,
                        &path,
                        cx,
                    );
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
                    this.connection_state.reset_editor_field(
                        ConnectionEditorField::WorkingDir,
                        &path,
                        cx,
                    );
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
        self.connection_state
            .sync_telnet_keep_alive_field_disabled(cx);
        cx.notify();
    }

    pub(in crate::features) fn set_connection_editor_section(
        &mut self,
        section: crate::models::ConnectionEditorSection,
        cx: &mut Context<Self>,
    ) {
        if self.connection_state.select_editor_section(section) {
            self.connection_state.hide_editor_secrets(cx);
            self.dismiss_connection_editor_menus(cx);
            cx.notify();
        }
    }

    pub(in crate::features) fn set_connection_editor_forwarding_tab(
        &mut self,
        x11: bool,
        cx: &mut Context<Self>,
    ) {
        self.connection_state.select_editor_forwarding_tab(x11);
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
                if self.connection_state.editor_agent_identity_picker_is_open() {
                    self.set_connection_editor_agent_identity_picker_open(false, cx);
                    return true;
                }
                if let Some(overlay) = self.connection_state.editor_credential_overlay() {
                    self.connection_state.set_editor_credential_overlay(None);
                    self.refresh_connection_auth_catalog(cx);
                    let select = match overlay {
                        ConnectionEditorCredentialOverlay::Passwords => {
                            ConnectionEditorSelect::SavedPassword
                        }
                        ConnectionEditorCredentialOverlay::Keys => ConnectionEditorSelect::SshKey,
                    };
                    self.focus_connection_editor_select(select, window, cx);
                    cx.notify();
                    return true;
                }
                if self.connection_state.editor_baud_popover_is_open() {
                    self.connection_state.set_editor_baud_popover_open(false);
                    cx.notify();
                    return true;
                }
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
                if self.connection_state.editor_credential_overlay().is_some()
                    || self.connection_state.editor_agent_identity_picker_is_open()
                {
                    return true;
                }
                if let Some(input) = self
                    .connection_state
                    .editor_fields()
                    .get(&ConnectionEditorField::PostLoginCommand)
                    .cloned()
                    .filter(|input| input.read(cx).component_focus_handle(cx).is_focused(window))
                    && (input.update(cx, |input, cx| input.is_composing(window, cx))
                        || (!keystroke.modifiers.platform && !keystroke.modifiers.control))
                {
                    // The script editor owns newlines and IME confirmation;
                    // only the explicit save shortcut may submit this field.
                    return false;
                }
                if !keystroke.modifiers.platform
                    && !keystroke.modifiers.control
                    && self.connection_state.editor_tag_field_is_focused(cx)
                {
                    self.add_connection_editor_tag(cx);
                    return true;
                }
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
                let input_focused = self
                    .connection_state
                    .editor_fields()
                    .values()
                    .any(|field| field.read(cx).component_focus_handle(cx).is_focused(window))
                    || self
                        .connection_state
                        .editor_number_fields()
                        .values()
                        .any(|field| field.read(cx).component_focus_handle(cx).is_focused(window));
                if input_focused
                    || self
                        .connection_state
                        .editor_focus_handle()
                        .is_focused(window)
                {
                    self.save_connection_editor(window, cx);
                    return true;
                }
                return false;
            }
            "tab" if !keystroke.modifiers.platform && !keystroke.modifiers.control => {
                if keystroke.modifiers.shift {
                    window.focus_prev(cx);
                } else {
                    window.focus_next(cx);
                }
                return true;
            }
            _ => {}
        }

        false
    }

    pub(super) fn reveal_connection_editor_validation(
        &mut self,
        location: super::validation::ConnectionEditorValidationLocation,
        message: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::models::ConnectionEditorFocusTarget;
        self.set_connection_editor_section(location.section, cx);
        self.connection_state
            .set_editor_validation_location(location, message);
        let token = self.connection_state.editor_token();
        let app = cx.weak_entity();
        window.on_next_frame(move |window, _| {
            window.on_next_frame(move |window, cx| {
                let _ = app.update(cx, |this, cx| {
                    if this.connection_state.editor_token() != token
                        || !this
                            .connection_state
                            .active_editor_draft()
                            .is_some_and(|editor| editor.section == location.section)
                    {
                        return;
                    }
                    let focus = match location.target {
                        ConnectionEditorFocusTarget::Field(field) => this
                            .connection_state
                            .editor_fields()
                            .get(&field)
                            .map(|input| input.read(cx).component_focus_handle(cx))
                            .or_else(|| {
                                this.connection_state
                                    .editor_number_fields()
                                    .get(&field)
                                    .map(|input| input.read(cx).component_focus_handle(cx))
                            }),
                        ConnectionEditorFocusTarget::Select(select) => {
                            this.focus_connection_editor_select(select, window, cx);
                            None
                        }
                        ConnectionEditorFocusTarget::ForwardingEndpoint { index, field } => this
                            .connection_state
                            .editor_forwarding_endpoint_fields()
                            .get(&(index, field))
                            .map(|input| input.read(cx).component_focus_handle(cx)),
                        ConnectionEditorFocusTarget::Heading => {
                            Some(this.connection_state.editor_section_focus())
                        }
                    };
                    if let Some(focus) = focus {
                        window.focus(&focus, cx);
                    }
                    this.connection_state
                        .editor_anchor(location.target)
                        .scroll_to(window, cx);
                    cx.notify();
                });
            });
        });
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

        let built = match build_saved_connection_from_editor(&editor).and_then(|built| {
            validate_editor_account(&editor, self.security.passwords()).map(|()| built)
        }) {
            Ok(connection) => connection,
            Err(error) => {
                let message = self.connection_editor_validation_message(&error);
                let location = error.location(&editor);
                self.reveal_connection_editor_validation(location, message, window, cx);
                cx.notify();
                return;
            }
        };

        let Some(save_generation) = self.connection_state.begin_editor_save() else {
            return;
        };
        let origin_window = window.window_handle();
        let saved_id = built.id.clone();
        let mut persisted = built.clone();
        let password_edit = (editor.password_source == ConnectionEditorPasswordSource::Direct)
            .then_some(editor.password_edit);
        let account_id = built
            .auth
            .as_ref()
            .and_then(|auth| auth.saved_account_id())
            .map(str::to_owned);
        let account_password = built
            .auth
            .as_ref()
            .is_some_and(|auth| auth.mode == "password" && auth.uses_account_password());
        let inspect_password =
            !self.settings.summary().has_master_password || self.security.secrets_unlocked();
        let follow_account_username =
            editor.username_source == nyaterm_core::ConnectionUsernameSource::Account;
        let ssh_username_required =
            editor.kind == ConnectionKindTab::Ssh && editor.username.trim().is_empty();
        let missing_account = t!("passwordManager.missingAccount").to_string();
        let missing_password = t!("authForm.accountPasswordMissing").to_string();
        let invalid_password = t!("authForm.accountPasswordUnreadable").to_string();
        let missing_username = t!("dialog.usernameRequired").to_string();
        let connect_after_save = editor.connect_after_save;
        self.shell.set_status("saving connection...".to_string());
        let submitted = self.submit_store_request(
            0,
            nyaterm_store::store_mutation(StoreDomain::Connections, move |store| {
                if let Some(id) = account_id.as_deref() {
                    let Some(account) = store.load_password_by_id(id)? else {
                        return Ok(Err((ConnectionEditorField::Username, missing_account)));
                    };
                    if ssh_username_required
                        && (!follow_account_username || account.username.trim().is_empty())
                    {
                        return Ok(Err((ConnectionEditorField::Username, missing_username)));
                    }
                    if account_password
                        && !account.has_password
                        && let Some(auth) = persisted.auth.as_mut()
                        && auth.account_id.is_some()
                    {
                        auth.password_source = Some(
                            nyaterm_core::models::credentials::ConnectionPasswordSource::Connection,
                        );
                    }
                    if account_password && account.has_password && inspect_password {
                        match store.load_decrypted_password_by_id(id) {
                            Ok(Some(entry))
                                if entry
                                    .password
                                    .as_deref()
                                    .is_some_and(|value| !value.is_empty()) => {}
                            Ok(_) => {
                                return Ok(Err((
                                    ConnectionEditorField::Password,
                                    missing_password,
                                )));
                            }
                            Err(_) => {
                                return Ok(Err((
                                    ConnectionEditorField::Password,
                                    invalid_password,
                                )));
                            }
                        }
                    }
                }
                save_editor_connection(store, persisted, pending_group.as_ref(), password_edit)
                    .map(Ok)
            }),
            move |this, event, cx| match event.outcome {
                Ok(Err((_field, message))) => {
                    if this
                        .connection_state
                        .finish_editor_save_request(save_generation)
                    {
                        let app = cx.weak_entity();
                        cx.defer(move |cx| {
                            let _ = origin_window.update(cx, |_, window, cx| {
                                let _ = app.update(cx, |this, cx| {
                                    if this.connection_state.editor_token() != Some(save_generation)
                                    {
                                        return;
                                    }
                                    let select = if this
                                        .connection_state
                                        .active_editor_draft()
                                        .is_some_and(|editor| {
                                            matches!(
                                                editor.kind,
                                                ConnectionKindTab::Ssh | ConnectionKindTab::Telnet
                                            )
                                        }) {
                                        ConnectionEditorSelect::Account
                                    } else {
                                        ConnectionEditorSelect::SavedPassword
                                    };
                                    this.reveal_connection_editor_validation(
                                        super::validation::ConnectionEditorValidationLocation {
                                            section: crate::models::ConnectionEditorSection::Basic,
                                            target:
                                                crate::models::ConnectionEditorFocusTarget::Select(
                                                    select,
                                                ),
                                            algorithm_tab: None,
                                        },
                                        message,
                                        window,
                                        cx,
                                    );
                                });
                            });
                        });
                        cx.notify();
                    }
                }
                Ok(Ok(sessions)) => {
                    let saved = sessions
                        .connections
                        .iter()
                        .find(|connection| connection.id == saved_id)
                        .cloned();
                    this.apply_loaded_sessions(sessions, cx);
                    if !this
                        .connection_state
                        .finish_editor_save_request(save_generation)
                    {
                        cx.notify();
                        return;
                    }
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
                    if !this
                        .connection_state
                        .finish_editor_save_request(save_generation)
                    {
                        return;
                    }
                    let message = t!("dialog.connectionSaveFailed", error = error).to_string();
                    this.set_connection_editor_error(message, cx);
                }
            },
            cx,
        );
        if !submitted {
            self.connection_state
                .finish_editor_save_request(save_generation);
        }
        cx.notify();
    }
}
