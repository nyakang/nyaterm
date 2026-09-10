use rust_i18n::t;

use gpui::{Context, Window};
use nyaterm_core::truncate_preview;
use nyaterm_transport::{RemoteFilePath, SftpAttributeUpdate, SftpFileEntry, SftpFileType};

use crate::features::NyaTermApp;
use crate::models::{
    TransferJobEvent, TransferJobKind, TransferJobOutput, TransferJobResult, TransferJobState,
    TransferJobStatus, TransferPropertiesField,
};

use super::{
    format_permissions_octal, normalized_transfer_browser_path, parse_transfer_mode,
    remote_file_name, remote_parent_path, transfer_properties_state_from_entry,
};

impl NyaTermApp {
    pub(super) fn open_current_transfer_browser_properties(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = normalized_transfer_browser_path(self.transfer.browser_view().path);
        if path.trim().is_empty() {
            self.shell
                .set_status("open a remote directory first".to_string());
            cx.notify();
            return;
        }
        let name = remote_file_name(&path);
        let entry = SftpFileEntry {
            name: if name.is_empty() { path.clone() } else { name },
            path,
            file_type: SftpFileType::Directory,
            size: Some(0),
            permissions: None,
            owner: String::new(),
            group: String::new(),
            modified_at: None,
            raw_path_token: None,
            symlink_target_is_directory: false,
        };
        self.open_transfer_properties(entry, window, cx);
    }

    pub(super) fn open_selected_transfer_properties(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.selected_transfer_entry() else {
            self.shell
                .set_status("select a remote item first".to_string());
            cx.notify();
            return;
        };
        self.forget_text_inputs("transfer.properties.");
        self.transfer
            .open_properties_dialog(transfer_properties_state_from_entry(
                entry.clone(),
                self.session.active_id_owned(),
            ));
        self.shell
            .set_status("remote properties opened".to_string());
        self.open_transfer_properties_component_dialog(window, cx);
        self.start_sftp_properties_load_job(entry.remote_path(), window, cx);
        cx.notify();
    }

    pub(super) fn open_transfer_properties(
        &mut self,
        entry: SftpFileEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.transfer.select_browser_path(entry.path.clone());
        self.transfer.set_remote_path(entry.path.clone());
        self.forget_text_inputs("transfer.properties.");
        self.transfer
            .open_properties_dialog(transfer_properties_state_from_entry(
                entry.clone(),
                self.session.active_id_owned(),
            ));
        self.shell
            .set_status("remote properties opened".to_string());
        self.open_transfer_properties_component_dialog(window, cx);
        self.start_sftp_properties_load_job(entry.remote_path(), window, cx);
        cx.notify();
    }

    pub(super) fn close_transfer_properties(&mut self, cx: &mut Context<Self>) {
        self.transfer.close_properties_dialog();
        self.forget_text_inputs("transfer.properties.");
        self.shell
            .set_status("remote properties closed".to_string());
        cx.notify();
    }

    fn open_transfer_properties_component_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.transfer.properties_dialog() else {
            return;
        };
        let title = t!(
            "fileExplorer.propertiesOf",
            name = truncate_preview(&state.entry.name, 42)
        )
        .to_string();
        let primary_label = if state
            .session_id
            .as_deref()
            .and_then(|session_id| self.session.file_browser_backend_for_session(session_id))
            == Some(nyaterm_transport::FileBrowserBackendKind::Local)
        {
            t!("common.close").to_string()
        } else {
            t!("common.save").to_string()
        };
        self.open_form_dialog(
            (
                title,
                460.,
                primary_label,
                |app, _, cx| app.transfer_properties_dialog_content(cx),
                |app, window, cx| app.submit_transfer_properties(window, cx),
                |app, cx| app.close_transfer_properties(cx),
            ),
            window,
            cx,
        );
    }

    pub(in crate::features) fn apply_transfer_properties_input(
        &mut self,
        field_id: &str,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = (match field_id {
            "symlink-target" => Some(TransferPropertiesField::SymlinkTarget),
            "mode" => Some(TransferPropertiesField::Mode),
            "owner" => Some(TransferPropertiesField::Owner),
            "group" => Some(TransferPropertiesField::Group),
            _ => None,
        }) else {
            return;
        };
        let filtered = normalize_transfer_properties_input(field, &text);
        if !self.transfer.set_properties_input(field, filtered.clone()) {
            return;
        }
        if filtered != text {
            self.reset_text_input(&format!("transfer.properties.{field_id}"), &filtered, cx);
        }
        cx.notify();
    }

    pub(in crate::features) fn sync_transfer_properties_inputs(&mut self, cx: &mut Context<Self>) {
        let Some((mode, owner, group)) = self.transfer.properties_input_values() else {
            return;
        };
        self.reset_text_input("transfer.properties.mode", &mode, cx);
        self.reset_text_input("transfer.properties.owner", &owner, cx);
        self.reset_text_input("transfer.properties.group", &group, cx);
        if let Some(state) = self.transfer.properties_dialog() {
            let value = state.symlink_target_value.clone();
            self.reset_text_input("transfer.properties.symlink-target", &value, cx);
        }
    }

    pub(super) fn start_sftp_properties_load_job(
        &mut self,
        remote_path: RemoteFilePath,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let service = match self.active_file_browser_service() {
            Ok(service) => service,
            Err(error) => {
                self.shell.set_status(error.to_string());
                cx.notify();
                return;
            }
        };
        let remote_display_path = remote_path.display_path.clone();
        let id = self.transfer.next_transfer_job_id("sftp-properties");
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id: self.session.active_id_owned(),
            kind: TransferJobKind::LoadProperties {
                remote_path: remote_display_path.clone(),
            },
            status: TransferJobStatus::Running,
            detail: format!("Loading properties for {remote_display_path}"),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
        });
        self.transfer
            .set_browser_status(format!("Loading properties for {remote_display_path}"));
        let transfer_tx = self.transfer.transfer_event_sender();
        self.submit_transfer_blocking_job(
            "sftp-load-properties",
            id.clone(),
            transfer_tx.clone(),
            move || {
                let result = service
                    .remote_file_properties(&remote_path)
                    .map(|properties| TransferJobOutput::PropertiesLoaded {
                        remote_path: remote_display_path,
                        properties,
                    })
                    .map_err(|error| error.to_string());
                let _ = transfer_tx.unbounded_send(TransferJobResult {
                    id,
                    event: TransferJobEvent::Finished(result),
                });
            },
        );
        cx.notify();
    }

    pub(super) fn submit_transfer_properties(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(state) = self.transfer.properties_dialog().cloned() else {
            self.shell
                .set_status("no remote properties dialog is active".to_string());
            cx.notify();
            return true;
        };
        if state.saving {
            return false;
        }
        if state
            .session_id
            .as_deref()
            .and_then(|session_id| self.session.file_browser_backend_for_session(session_id))
            == Some(nyaterm_transport::FileBrowserBackendKind::Local)
        {
            self.close_transfer_properties(cx);
            return true;
        }
        let Some(properties) = state.properties.clone() else {
            self.shell
                .set_status("remote properties are still loading".to_string());
            cx.notify();
            return false;
        };
        if properties.file_type == nyaterm_transport::SftpFileType::Symlink {
            if state.symlink_target_value.is_empty() || state.symlink_target_value.contains('\0') {
                self.transfer
                    .set_properties_error(t!("fileExplorer.symlinkTargetRequired").to_string());
                cx.notify();
                return false;
            }
            if properties.symlink_target.as_deref() == Some(state.symlink_target_value.as_str()) {
                self.close_transfer_properties(cx);
                return true;
            }
            self.transfer.begin_properties_save();
            self.start_sftp_properties_update_job(
                RemoteFilePath {
                    display_path: properties.path,
                    raw_path_token: state.entry.raw_path_token,
                },
                remote_parent_path(&state.entry.path),
                SftpAttributeUpdate {
                    symlink_target: Some(state.symlink_target_value),
                    ..Default::default()
                },
                window,
                cx,
            );
            return false;
        }
        let mode = match parse_transfer_mode(&state.mode_value) {
            Some(mode) => mode,
            None => {
                self.transfer
                    .set_properties_error("Mode must be a 3 or 4 digit octal value.".to_string());
                cx.notify();
                return false;
            }
        };
        let owner = state.owner_value.trim().to_string();
        let group = state.group_value.trim().to_string();
        if owner.is_empty() || group.is_empty() {
            self.transfer
                .set_properties_error("Owner and group are required.".to_string());
            cx.notify();
            return false;
        }
        let initial_mode = properties
            .permissions
            .map(format_permissions_octal)
            .unwrap_or_else(|| "0644".to_string());
        let owner_changed =
            owner != properties.owner && properties.uid.is_none_or(|uid| owner != uid.to_string());
        let group_changed =
            group != properties.group && properties.gid.is_none_or(|gid| group != gid.to_string());
        let update = SftpAttributeUpdate {
            symlink_target: None,
            mode: (state.mode_value != initial_mode).then_some(mode),
            owner: owner_changed.then_some(owner),
            group: group_changed.then_some(group),
            recursive: state.recursive && properties.is_directory(),
        };
        if update.mode.is_none() && update.owner.is_none() && update.group.is_none() {
            self.close_transfer_properties(cx);
            return true;
        }
        self.transfer.begin_properties_save();
        self.start_sftp_properties_update_job(
            RemoteFilePath {
                display_path: properties.path,
                raw_path_token: state.entry.raw_path_token,
            },
            remote_parent_path(&state.entry.path),
            update,
            window,
            cx,
        );
        false
    }

    pub(super) fn start_sftp_properties_update_job(
        &mut self,
        remote_path: RemoteFilePath,
        parent_path: String,
        update: SftpAttributeUpdate,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let service = match self.active_file_browser_service() {
            Ok(service) => service,
            Err(error) => {
                self.shell.set_status(error.to_string());
                cx.notify();
                return;
            }
        };
        let remote_display_path = remote_path.display_path.clone();
        let id = self.transfer.next_transfer_job_id("sftp-update-properties");
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id: self.session.active_id_owned(),
            kind: TransferJobKind::UpdateProperties {
                remote_path: remote_display_path.clone(),
                parent_path: parent_path.clone(),
            },
            status: TransferJobStatus::Running,
            detail: format!("Updating properties for {remote_display_path}"),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
        });
        self.transfer
            .set_browser_status(format!("Updating properties for {remote_display_path}"));
        let transfer_tx = self.transfer.transfer_event_sender();
        self.submit_transfer_blocking_job(
            "sftp-update-properties",
            id.clone(),
            transfer_tx.clone(),
            move || {
                let result = (|| {
                    service.update_remote_path_attributes(&remote_path, update)?;
                    let properties = service.remote_file_properties(&remote_path)?;
                    let entries = service.list_dir(&parent_path)?;
                    Ok(TransferJobOutput::PropertiesUpdated {
                        remote_path: remote_display_path,
                        parent_path,
                        properties,
                        entries,
                    })
                })()
                .map_err(|error: anyhow::Error| error.to_string());
                let _ = transfer_tx.unbounded_send(TransferJobResult {
                    id,
                    event: TransferJobEvent::Finished(result),
                });
            },
        );
        cx.notify();
    }
}

fn normalize_transfer_properties_input(field: TransferPropertiesField, text: &str) -> String {
    if field == TransferPropertiesField::Mode {
        text.chars()
            .filter(|value| ('0'..='7').contains(value))
            .take(4)
            .collect()
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use crate::models::TransferPropertiesField;

    use super::normalize_transfer_properties_input;

    #[test]
    fn properties_mode_input_keeps_four_octal_digits() {
        assert_eq!(
            normalize_transfer_properties_input(TransferPropertiesField::Mode, "09a7555"),
            "0755"
        );
    }

    #[test]
    fn properties_owner_and_group_input_are_not_filtered() {
        assert_eq!(
            normalize_transfer_properties_input(TransferPropertiesField::Owner, "dev team"),
            "dev team"
        );
        assert_eq!(
            normalize_transfer_properties_input(TransferPropertiesField::Group, "release-team"),
            "release-team"
        );
    }
}
