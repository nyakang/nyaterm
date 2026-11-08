use super::basis::{batch_kind_value, compute_batch_basis, parse_batch_kind, select_initial_value};
use super::fields::{BatchFieldId, EDITABLE_FIELDS, FieldBasis};
use super::options;
use super::{ASSET_BATCH_SELECT_PREFIX, ASSET_BATCH_TEXT_PREFIX};
use crate::features::NyaTermApp;
use crate::features::assets::BatchEditMode;
use crate::features::text_inputs::TextInputSetup;
use gpui::{Context, Window};
use nyaterm_core::models::SshProfile;
use nyaterm_core::models::connection_batch::{
    ConnectionBatchKind, apply_connection_batch_edit, validate_connection_batch_edit,
};
use nyaterm_store::StoreDomain;
use rust_i18n::t;
impl NyaTermApp {
    /// Open the editor for the current asset selection. Creating the widget
    /// entities here, in an event handler, keeps the render path read-only.
    ///
    /// Mixed selections start in the common scope; protocol tabs keep their original targets.
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
        self.start_workspace
            .start_asset_batch_edit(ids, basis, values, bool_values);
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
        if self.start_workspace.asset_batch_mut().set_scope(kind) {
            self.reseed_asset_batch_inputs(cx);
        }
    }
    fn reseed_asset_batch_inputs(&mut self, cx: &mut Context<Self>) {
        if let Some(input) = self.existing_text_input(BatchFieldId::Password.text_input_id()) {
            input.update(cx, |state, cx| state.discard_content(cx));
        }
        self.forget_text_inputs(ASSET_BATCH_TEXT_PREFIX);
        let groups = self.connection_state.groups().to_vec();
        self.seed_asset_batch_inputs(&groups, cx);
        cx.notify();
    }
    pub(in crate::features) fn set_asset_batch_mode(
        &mut self,
        field: BatchFieldId,
        mode: BatchEditMode,
        cx: &mut Context<Self>,
    ) {
        self.start_workspace.asset_batch_mut().set_mode(field, mode);
        self.reseed_asset_batch_inputs(cx);
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
        if let Some(input) = self.existing_text_input(BatchFieldId::Password.text_input_id()) {
            input.update(cx, |state, cx| state.discard_content(cx));
        }
        self.start_workspace.clear_asset_batch_edit();
        self.forget_text_inputs(ASSET_BATCH_TEXT_PREFIX);
        cx.notify();
    }

    fn seed_asset_batch_inputs(&mut self, groups: &[nyaterm_core::Group], cx: &mut Context<Self>) {
        for (field, hint) in [
            (
                BatchFieldId::Description,
                t!("assets.batch.placeholderDescription"),
            ),
            (BatchFieldId::Port, t!("assets.batch.placeholderPort")),
            (
                BatchFieldId::Username,
                t!("assets.batch.placeholderUsername"),
            ),
            (
                BatchFieldId::SftpShellTimeout,
                t!("assets.batch.placeholderShellTimeout"),
            ),
            (
                BatchFieldId::PostLoginCommand,
                t!("assets.batch.placeholderPostLoginCommand"),
            ),
            (
                BatchFieldId::PostLoginDelay,
                t!("assets.batch.placeholderPostLoginDelay"),
            ),
            (BatchFieldId::SshKex, t!("assets.batch.placeholderSshKex")),
            (
                BatchFieldId::SshCiphers,
                t!("assets.batch.placeholderSshCiphers"),
            ),
            (BatchFieldId::SshMacs, t!("assets.batch.placeholderSshMacs")),
            (
                BatchFieldId::SshHostKeys,
                t!("assets.batch.placeholderSshHostKeys"),
            ),
            (BatchFieldId::Tags, t!("assets.batch.placeholderValue")),
            (
                BatchFieldId::SshAuthAgentEnvironment,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldId::SshAuthAgentSocket,
                t!("assets.batch.placeholderValue"),
            ),
            (BatchFieldId::ShellPath, t!("assets.batch.placeholderValue")),
            (BatchFieldId::ShellArgs, t!("assets.batch.placeholderValue")),
            (
                BatchFieldId::WorkingDir,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldId::TelnetKeepAlive,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldId::TelnetAutoLoginTimeout,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldId::TelnetAutoLoginRetries,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldId::TelnetAutoLoginUserPrompt,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldId::TelnetAutoLoginPasswordPrompt,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldId::TelnetAutoLoginSuccessPrompt,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldId::TelnetAutoLoginFailurePrompt,
                t!("assets.batch.placeholderValue"),
            ),
            (BatchFieldId::RdpDomain, t!("assets.batch.placeholderValue")),
            (BatchFieldId::RdpWidth, t!("assets.batch.placeholderValue")),
            (BatchFieldId::RdpHeight, t!("assets.batch.placeholderValue")),
            (
                BatchFieldId::RdpReconnectAttempts,
                t!("assets.batch.placeholderValue"),
            ),
            (
                BatchFieldId::VncReconnectAttempts,
                t!("assets.batch.placeholderValue"),
            ),
        ] {
            let (seed, placeholder, disabled) = {
                let draft = self.start_workspace.asset_batch();
                match draft.basis(field) {
                    FieldBasis::Inapplicable => (
                        String::new(),
                        t!("assets.batch.inapplicable").to_string(),
                        true,
                    ),
                    FieldBasis::Mixed => (
                        draft.value(field).to_string(),
                        t!("assets.batch.mixed").to_string(),
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
                || self.start_workspace.asset_batch().mode(field) != BatchEditMode::Set
                || self.start_workspace.asset_batch_saving();
            input.update(cx, |state, cx| state.set_disabled(disabled, cx));
        }

        {
            let (placeholder, disabled) = {
                let draft = self.start_workspace.asset_batch();
                match draft.basis(BatchFieldId::Password) {
                    FieldBasis::Inapplicable => (t!("assets.batch.inapplicable").to_string(), true),
                    FieldBasis::Mixed => (t!("assets.batch.mixed").to_string(), false),
                    FieldBasis::Uniform => (draft.value(BatchFieldId::Password).to_string(), false),
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
            let input = self.text_input(BatchFieldId::Password.text_input_id(), &seed, setup, cx);
            let disabled = disabled
                || self
                    .start_workspace
                    .asset_batch()
                    .mode(BatchFieldId::Password)
                    != BatchEditMode::Set
                || self.start_workspace.asset_batch_saving();
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

        for field in EDITABLE_FIELDS.into_iter().filter(|field| field.is_bool()) {
            let draft = self.start_workspace.asset_batch();
            let selected = (draft.basis(field) == FieldBasis::Uniform || draft.is_edited(field))
                .then(|| draft.bool_value(field).to_string());
            let selected =
                if draft.basis(field) == FieldBasis::Mixed && !draft.current_bool_is_set(field) {
                    None
                } else {
                    selected
                };
            let disabled = draft.mode(field) != BatchEditMode::Set || draft.saving();
            self.select_entity(
                field.select_id(),
                vec![
                    nyaterm_ui::NyaSelectOption::new("true", t!("assets.batch.enabled")),
                    nyaterm_ui::NyaSelectOption::new("false", t!("assets.batch.disabled")),
                ],
                selected,
                disabled,
                cx,
            );
        }
        let selected_value = |field: BatchFieldId| {
            let draft = self.start_workspace.asset_batch();
            select_initial_value(draft, field).unwrap_or_default()
        };
        let selected_ids = self.start_workspace.asset_batch_ids().to_vec();
        let account_kind = self.start_workspace.asset_batch().display_kind();
        let terminal_profile = if selected_value(BatchFieldId::SshProfile) == "network_device" {
            SshProfile::NetworkDevice
        } else {
            SshProfile::Standard
        };
        for (field, options) in [
            (
                BatchFieldId::AuthMode,
                vec![
                    nyaterm_ui::NyaSelectOption::new("password", t!("dialog.password")),
                    nyaterm_ui::NyaSelectOption::new("key", t!("dialog.privateKey")),
                    nyaterm_ui::NyaSelectOption::new("agent", t!("dialog.sshAgent")),
                    nyaterm_ui::NyaSelectOption::new("none", t!("dialog.noAuthentication")),
                ],
            ),
            (BatchFieldId::Group, options::group_select_options(groups)),
            (BatchFieldId::Icon, {
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
            (BatchFieldId::Encoding, options::encoding_select_options()),
            (BatchFieldId::Backspace, options::backspace_select_options()),
            (
                BatchFieldId::SshProfile,
                options::ssh_profile_select_options(),
            ),
            (
                BatchFieldId::TerminalType,
                options::terminal_type_select_options(terminal_profile),
            ),
            (
                BatchFieldId::SftpCwdFollow,
                options::sftp_cwd_follow_select_options(),
            ),
            (
                BatchFieldId::SftpFilenameEncoding,
                options::sftp_filename_encoding_select_options(),
            ),
            (
                BatchFieldId::SftpPipelineDepth,
                options::sftp_pipeline_depth_select_options(&selected_value(
                    BatchFieldId::SftpPipelineDepth,
                )),
            ),
            (
                BatchFieldId::SshAlgorithmMode,
                options::ssh_algorithm_mode_select_options(),
            ),
            (
                BatchFieldId::RdpCertificatePolicy,
                options::rdp_certificate_policy_select_options(),
            ),
            (
                BatchFieldId::VncSecurityMode,
                options::vnc_security_select_options(),
            ),
            (
                BatchFieldId::Account,
                options::account_select_options(
                    self.security.passwords(),
                    account_kind,
                    &selected_value(BatchFieldId::Account),
                ),
            ),
            (
                BatchFieldId::Key,
                options::key_select_options(
                    self.security.ssh_keys(),
                    &selected_value(BatchFieldId::Key),
                ),
            ),
            (
                BatchFieldId::Otp,
                options::otp_select_options(
                    self.security.otp_entries(),
                    &selected_value(BatchFieldId::Otp),
                ),
            ),
            (
                BatchFieldId::Proxy,
                options::proxy_select_options(
                    self.tunnel_state.proxies(),
                    &selected_value(BatchFieldId::Proxy),
                ),
            ),
            (
                BatchFieldId::ProxyJump,
                options::proxy_jump_select_options(
                    self.connection_state.connections(),
                    groups,
                    &selected_value(BatchFieldId::ProxyJump),
                    &selected_ids,
                ),
            ),
            (
                BatchFieldId::SshAuthAgentEndpoint,
                options::ssh_agent_endpoint_select_options(&selected_value(
                    BatchFieldId::SshAuthAgentEndpoint,
                )),
            ),
            (
                BatchFieldId::AgentPolicy,
                options::ssh_agent_policy_select_options(&selected_value(
                    BatchFieldId::AgentPolicy,
                )),
            ),
            (
                BatchFieldId::RdpBackend,
                options::rdp_backend_select_options(&selected_value(BatchFieldId::RdpBackend)),
            ),
            (
                BatchFieldId::RdpDisplayMode,
                options::rdp_display_mode_select_options(&selected_value(
                    BatchFieldId::RdpDisplayMode,
                )),
            ),
            (
                BatchFieldId::VncScaleMode,
                options::vnc_scale_select_options(&selected_value(BatchFieldId::VncScaleMode)),
            ),
            (
                BatchFieldId::RecordingMode,
                options::recording_mode_select_options(&selected_value(
                    BatchFieldId::RecordingMode,
                )),
            ),
            (
                BatchFieldId::SerialPort,
                options::serial_port_select_options(
                    self.connection_state.serial_ports(),
                    &selected_value(BatchFieldId::SerialPort),
                ),
            ),
            (
                BatchFieldId::BaudRate,
                options::baud_rate_select_options(&selected_value(BatchFieldId::BaudRate)),
            ),
            (
                BatchFieldId::DataBits,
                options::data_bits_select_options(&selected_value(BatchFieldId::DataBits)),
            ),
            (
                BatchFieldId::Parity,
                options::parity_select_options(&selected_value(BatchFieldId::Parity)),
            ),
            (
                BatchFieldId::StopBits,
                options::stop_bits_select_options(&selected_value(BatchFieldId::StopBits)),
            ),
            (
                BatchFieldId::FlowControl,
                options::flow_control_select_options(&selected_value(BatchFieldId::FlowControl)),
            ),
            (
                BatchFieldId::TelnetEnterMode,
                options::telnet_enter_select_options(&selected_value(
                    BatchFieldId::TelnetEnterMode,
                )),
            ),
        ] {
            self.seed_asset_batch_select(field, options, cx);
        }
    }

    fn seed_asset_batch_select(
        &mut self,
        field: BatchFieldId,
        options: Vec<nyaterm_ui::NyaSelectOption>,
        cx: &mut Context<Self>,
    ) {
        let (selected, placeholder, disabled) = {
            let draft = self.start_workspace.asset_batch();
            match draft.basis(field) {
                FieldBasis::Inapplicable => {
                    (None, t!("assets.batch.inapplicable").to_string(), true)
                }
                FieldBasis::Mixed => (
                    select_initial_value(draft, field),
                    t!("assets.batch.mixed").to_string(),
                    false,
                ),
                FieldBasis::Uniform => (select_initial_value(draft, field), String::new(), false),
            }
        };
        let options = options
            .into_iter()
            .filter(|option| {
                !option.value().is_empty()
                    && !(field == BatchFieldId::Encoding && option.value() == "global")
                    && !(field == BatchFieldId::SftpFilenameEncoding
                        && option.value() == "terminal")
            })
            .collect();
        let disabled = disabled
            || self.start_workspace.asset_batch().mode(field) != BatchEditMode::Set
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
        if let Some(field) = BatchFieldId::from_key(key) {
            self.start_workspace.set_asset_batch_value(field, text);
            cx.notify();
        }
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
        if let Some(field) = BatchFieldId::from_key(key) {
            if field.is_bool() {
                self.start_workspace
                    .set_asset_batch_bool(field, value == "true");
            } else {
                self.start_workspace.set_asset_batch_value(field, value);
            }
            cx.notify();
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
            let changed = prepare_asset_batch_updates(store, &targets, &edits)?;
            store.save_connections_atomic(&changed)?;
            Ok((changed.len(), targets.len() - changed.len()))
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
        outcome: Result<(usize, usize), nyaterm_store::StoreOperationError>,
        cx: &mut Context<Self>,
    ) {
        let current = self
            .start_workspace
            .asset_batch()
            .matches_generation(generation);
        match outcome {
            Ok((updated, skipped)) => {
                if current {
                    self.close_asset_batch_edit(cx);
                }
                self.shell.set_status(
                    t!("assets.batch.saved", updated = updated, skipped = skipped).to_string(),
                );
                let reload = nyaterm_store::store_request(StoreDomain::Connections, |store| {
                    store.load_sessions()
                });
                let queued = self.submit_store_request(
                    0,
                    reload,
                    |app, event, cx| {
                        app.finish_asset_batch_refresh(event.outcome, cx);
                    },
                    cx,
                );
                if !queued {
                    self.shell
                        .set_status(t!("assets.batch.refreshFailed").to_string());
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
        outcome: Result<nyaterm_core::SessionsConfig, nyaterm_store::StoreOperationError>,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Ok(sessions) => self.apply_loaded_sessions(sessions, cx),
            Err(_) => self
                .shell
                .set_status(t!("assets.batch.refreshFailed").to_string()),
        }
        cx.notify();
    }
}

fn prepare_asset_batch_updates(
    store: &nyaterm_store::ConnectionStore,
    targets: &[(String, ConnectionBatchKind)],
    edits: &std::collections::HashMap<
        Option<ConnectionBatchKind>,
        nyaterm_core::models::connection_batch::ConnectionBatchEdit,
    >,
) -> Result<Vec<nyaterm_core::models::SavedConnection>, nyaterm_store::StorageError> {
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
    for (mut connection, kind) in latest {
        let original = connection.clone();
        if let Some(edit) = edits.get(&None) {
            apply_connection_batch_edit(&mut connection, edit);
        }
        if let Some(edit) = edits.get(&Some(kind)) {
            apply_connection_batch_edit(&mut connection, edit);
        }
        if connection != original {
            changed.push(connection);
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::prepare_asset_batch_updates;
    use nyaterm_core::models::SavedConnection;
    use nyaterm_core::models::connection_batch::{
        BatchField, ConnectionBatchEdit, ConnectionBatchKind,
    };
    use nyaterm_core::test_support::TestTempDir;
    use nyaterm_store::ConnectionStore;
    use std::collections::HashMap;

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
