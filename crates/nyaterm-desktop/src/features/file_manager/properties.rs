use super::operations::FileManagerSelection;
use super::pane::{PaneEndpoint, PaneId};
use crate::features::NyaTermApp;
use crate::features::formatting::format_permissions_octal;
use crate::features::runtime_jobs::await_blocking_job;
use gpui::{Context, Window};
use nyaterm_files::sftp::{RemoteFilePath, SftpAttributeUpdate, SftpFileProperties};
use nyaterm_ui::{NyaDialogHandle, NyaDialogWindowExt};
use rust_i18n::t;

/// A property dialog keeps its source even when the active pane/session changes.
#[derive(Clone)]
pub(in crate::features) struct FileManagerPropertiesTarget {
    pub request_id: String,
    pub dialog: Option<NyaDialogHandle>,
    pub manager_id: String,
    pub pane: PaneId,
    pub endpoint: PaneEndpoint,
    pub directory: RemoteFilePath,
    pub path: RemoteFilePath,
    pub access_generation: Option<u64>,
}

enum PropertiesResult {
    Loaded(Box<SftpFileProperties>),
    Saved,
}

impl NyaTermApp {
    pub(in crate::features) fn open_file_manager_properties(
        &mut self,
        manager_id: String,
        pane: PaneId,
        selection: FileManagerSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let [entry] = selection.entries.as_slice() else {
            return;
        };
        let target = FileManagerPropertiesTarget {
            request_id: nyaterm_core::uuid(),
            dialog: None,
            access_generation: selection
                .endpoint
                .session_id()
                .and_then(|id| self.file_access_generation(id)),
            manager_id,
            pane,
            endpoint: selection.endpoint,
            directory: selection.directory,
            path: entry.remote_path(),
        };
        if !self.file_manager_properties_target_available(&target, cx) {
            return;
        }
        self.open_file_manager_properties_dialog(entry.clone(), target, window, cx);
    }

    pub(in crate::features) fn file_manager_properties_target_available(
        &self,
        target: &FileManagerPropertiesTarget,
        cx: &gpui::App,
    ) -> bool {
        target
            .dialog
            .as_ref()
            .is_none_or(NyaDialogHandle::is_active)
            && target.access_generation
                == target
                    .endpoint
                    .session_id()
                    .and_then(|id| self.file_access_generation(id))
            && self
                .file_manager
                .workspace(&target.manager_id)
                .is_some_and(|workspace| {
                    let pane = workspace.read(cx).pane(target.pane);
                    pane.endpoint == target.endpoint && !pane.disconnected
                })
    }

    pub(in crate::features) fn start_file_manager_properties_job(
        &mut self,
        target: FileManagerPropertiesTarget,
        update: Option<SftpAttributeUpdate>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.file_manager_properties_target_available(&target, cx) {
            self.transfer
                .set_properties_error(t!("fileManager.sourceUnavailable").to_string());
            cx.notify();
            return;
        }
        if update.is_some()
            && self.file_browser_backend_for_endpoint(&target.endpoint)
                != nyaterm_files::file_browser::FileBrowserBackendKind::Remote
        {
            return;
        }
        let service = self.file_browser_service_for_endpoint(&target.endpoint);
        let path = target.path.clone();
        let task = self
            .blocking_jobs
            .submit_task("file-manager-properties", move |_| {
                let service = service?;
                if let Some(update) = update {
                    service.update_remote_path_attributes(&path, update)?;
                    Ok(PropertiesResult::Saved)
                } else {
                    service
                        .remote_file_properties(&path)
                        .map(|properties| PropertiesResult::Loaded(Box::new(properties)))
                }
            });
        let window_handle = window.window_handle();
        cx.spawn(async move |app, cx| {
            let result = await_blocking_job(task).await.and_then(
                |result: anyhow::Result<PropertiesResult>| {
                    result.map_err(|error| {
                        crate::features::android::labels::file_error_message(&error)
                    })
                },
            );
            let _ = window_handle.update(cx, |_, window, cx| {
                let _ = app.update(cx, |app, cx| {
                    // A successful mutation must refresh its source even if the dialog closed.
                    if matches!(&result, Ok(PropertiesResult::Saved))
                        && target.access_generation
                            == target
                                .endpoint
                                .session_id()
                                .and_then(|id| app.file_access_generation(id))
                    {
                        app.refresh_file_manager_directory(&target.endpoint, &target.directory, cx);
                    }
                    if !app
                        .transfer
                        .properties_target()
                        .is_some_and(|current| current.request_id == target.request_id)
                        || target
                            .dialog
                            .as_ref()
                            .is_some_and(|dialog| !dialog.is_active())
                    {
                        return;
                    }
                    if !app.file_manager_properties_target_available(&target, cx) {
                        app.transfer
                            .set_properties_error(t!("fileManager.sourceUnavailable").to_string());
                        cx.notify();
                        return;
                    }
                    match result {
                        Ok(PropertiesResult::Loaded(properties)) => {
                            let properties = *properties;
                            let Some(mut state) = app.transfer.properties_dialog().cloned() else {
                                return;
                            };
                            state.symlink_target_value =
                                properties.symlink_target.clone().unwrap_or_default();
                            state.mode_value =
                                format_permissions_octal(properties.permissions.unwrap_or(0o644));
                            state.owner_value = if properties.owner.is_empty() {
                                properties
                                    .uid
                                    .map(|uid| uid.to_string())
                                    .unwrap_or_default()
                            } else {
                                properties.owner.clone()
                            };
                            state.group_value = if properties.group.is_empty() {
                                properties
                                    .gid
                                    .map(|gid| gid.to_string())
                                    .unwrap_or_default()
                            } else {
                                properties.group.clone()
                            };
                            state.properties = Some(properties);
                            state.error = None;
                            app.transfer.open_properties_dialog(state);
                            app.transfer.set_properties_target(target);
                            app.sync_transfer_properties_inputs(cx);
                        }
                        Ok(PropertiesResult::Saved) => {
                            app.transfer.close_properties_dialog();
                            app.forget_text_inputs("transfer.properties.");
                            if target
                                .dialog
                                .as_ref()
                                .is_some_and(|dialog| dialog.is_top(window, cx))
                            {
                                window.close_nya_dialog(cx);
                            }
                        }
                        Err(error) => {
                            app.transfer.set_properties_error(error);
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }
}
