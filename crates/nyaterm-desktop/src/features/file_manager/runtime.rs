use super::operations::FileManagerSelection;
use super::pane::{PaneEndpoint, PaneId};
use super::view::FileManagerNameKind;
use super::workspace::{FileManagerListResult, FileManagerWorkspace};
use crate::features::NyaTermApp;
use crate::features::runtime_jobs::await_blocking_job;
use crate::features::text_inputs::TextInputSetup;
use crate::features::transfers::file_operations::{
    FileMutation, FileMutationIntent, FileTransferOperation,
};
use gpui::{Context, Window, prelude::*};
use nyaterm_transport::{FileBrowserService, RemoteFilePath};
use rust_i18n::t;

impl NyaTermApp {
    pub(in crate::features) fn file_browser_service_for_endpoint(
        &mut self,
        endpoint: &PaneEndpoint,
    ) -> anyhow::Result<FileBrowserService> {
        match endpoint {
            PaneEndpoint::Local => Ok(FileBrowserService::local()),
            PaneEndpoint::Session(id) => self.file_browser_service_for_session(id),
            PaneEndpoint::OfflineConnection(_) => {
                anyhow::bail!("reconnect the source before browsing files")
            }
        }
    }

    pub(in crate::features) fn navigate_file_manager_pane(
        &mut self,
        manager_id: String,
        pane_id: PaneId,
        path: RemoteFilePath,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self.file_manager.workspace(&manager_id) else {
            return;
        };
        let (endpoint, generation, snapshot) = workspace.update(cx, |workspace, cx| {
            let pane = workspace.pane_mut(pane_id);
            let snapshot = pane.capture_navigation();
            let generation = pane.begin_load(path.clone());
            cx.notify();
            (pane.endpoint.clone(), generation, snapshot)
        });
        let service = match self.file_browser_service_for_endpoint(&endpoint) {
            Ok(service) => service,
            Err(error) => {
                workspace.update(cx, |workspace, cx| {
                    workspace.apply_list_result(FileManagerListResult {
                        pane: pane_id,
                        generation,
                        endpoint,
                        path,
                        snapshot,
                        result: Err(error.to_string()),
                    });
                    let pane = workspace.pane_mut(pane_id);
                    pane.disconnected = true;
                    cx.notify();
                });
                return;
            }
        };
        let task = self.blocking_jobs.submit_task("file-manager-list", {
            let path = path.clone();
            move |_| {
                service.list_dir_path(&path).map_err(|error| {
                    let missing = error.chain().any(|cause| {
                        cause
                            .downcast_ref::<std::io::Error>()
                            .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
                    }) || error.to_string().contains("No such file")
                        || error.to_string().contains("no such file");
                    (missing, error.to_string())
                })
            }
        });
        cx.spawn(async move |this, cx| {
            let result = match await_blocking_job(task).await {
                Ok(result) => result,
                Err(error) => Err((false, error)),
            };
            let missing = result.as_ref().err().is_some_and(|(missing, _)| *missing);
            let success = result.is_ok();
            let _ = this.update(cx, |app, cx| {
                let changed = workspace.update(cx, |workspace, cx| {
                    let changed = workspace.apply_list_result(FileManagerListResult {
                        pane: pane_id,
                        generation,
                        endpoint: endpoint.clone(),
                        path: path.clone(),
                        snapshot,
                        result: result.map_err(|(_, message)| message),
                    });
                    if changed {
                        cx.notify();
                    }
                    changed
                });
                if changed && success {
                    app.persist_file_manager_settings(cx);
                }
                if changed
                    && missing
                    && workspace.read(cx).pane(pane_id).home_dir != path.display_path
                {
                    app.shell
                        .set_status(t!("fileManager.directoryMissing").to_string());
                    app.navigate_file_manager_home(manager_id, pane_id, cx);
                }
            });
        })
        .detach();
    }

    pub(in crate::features) fn navigate_file_manager_home(
        &mut self,
        manager_id: String,
        pane_id: PaneId,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self.file_manager.workspace(&manager_id) else {
            return;
        };
        let endpoint = workspace.read(cx).pane(pane_id).endpoint.clone();
        let service = match self.file_browser_service_for_endpoint(&endpoint) {
            Ok(service) => service,
            Err(error) => {
                workspace.update(cx, |workspace, cx| {
                    let pane = workspace.pane_mut(pane_id);
                    pane.disconnected = true;
                    pane.loading = false;
                    pane.error = Some(error.to_string());
                    cx.notify();
                });
                return;
            }
        };
        let generation = workspace.update(cx, |workspace, cx| {
            let pane = workspace.pane_mut(pane_id);
            pane.generation = pane.generation.wrapping_add(1);
            pane.loading = true;
            pane.error = None;
            cx.notify();
            pane.generation
        });
        let task = self
            .blocking_jobs
            .submit_task("file-manager-home", move |_| {
                service.home_dir().map_err(|error| error.to_string())
            });
        cx.spawn(async move |this, cx| {
            let result = await_blocking_job(task).await.and_then(|result| result);
            let _ = this.update(cx, |app, cx| {
                if workspace.read(cx).pane(pane_id).generation != generation
                    || workspace.read(cx).pane(pane_id).endpoint != endpoint
                {
                    return;
                }
                match result {
                    Ok(home) => {
                        workspace.update(cx, |workspace, _| {
                            let pane = workspace.pane_mut(pane_id);
                            pane.home_dir = home.clone();
                            pane.home_dir_pending = false;
                        });
                        app.navigate_file_manager_pane(
                            manager_id,
                            pane_id,
                            RemoteFilePath::new(home),
                            cx,
                        );
                    }
                    Err(error) => workspace.update(cx, |workspace, cx| {
                        let pane = workspace.pane_mut(pane_id);
                        pane.loading = false;
                        pane.error = Some(error);
                        cx.notify();
                    }),
                }
            });
        })
        .detach();
    }

    pub(in crate::features) fn refresh_file_manager_pane(
        &mut self,
        manager_id: String,
        pane_id: PaneId,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self.file_manager.workspace(&manager_id) else {
            return;
        };
        let path = workspace.read(cx).pane(pane_id).path.clone();
        if path.display_path.is_empty() {
            self.navigate_file_manager_home(manager_id, pane_id, cx);
        } else {
            self.navigate_file_manager_pane(manager_id, pane_id, path, cx);
        }
    }

    pub(in crate::features) fn refresh_file_manager_directory(
        &mut self,
        endpoint: &PaneEndpoint,
        path: &RemoteFilePath,
        cx: &mut Context<Self>,
    ) {
        let targets: Vec<_> = self
            .file_manager
            .all_workspaces()
            .flat_map(|workspace| {
                let id = workspace.read(cx).id.clone();
                [PaneId::Left, PaneId::Right]
                    .into_iter()
                    .filter_map(|pane_id| {
                        let pane = workspace.read(cx).pane(pane_id);
                        (pane.endpoint == *endpoint
                            && (pane.path.raw_path_token.is_none()
                                || path.raw_path_token.is_none()
                                || pane.path.raw_path_token == path.raw_path_token)
                            && nyaterm_transport::file_browser_path_eq(
                                endpoint.kind(),
                                &pane.path.display_path,
                                &path.display_path,
                            ))
                        .then(|| (id.clone(), pane_id))
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        for (id, pane) in targets {
            self.refresh_file_manager_pane(id, pane, cx);
        }
        let sessions: Vec<_> = self
            .session
            .session_ids()
            .filter(|session| match endpoint {
                PaneEndpoint::Local => {
                    self.session
                        .file_browser_backend_support_for_session(session)
                        == Some(nyaterm_transport::FileBrowserBackendKind::Local)
                }
                PaneEndpoint::Session(id) => *session == id,
                PaneEndpoint::OfflineConnection(_) => false,
            })
            .map(str::to_string)
            .collect();
        let active = self.session.active_id_owned();
        for session in sessions {
            self.transfer
                .invalidate_file_operation_directory(&session, path);
        }
        let browser = self.transfer.browser_remote_file_path();
        let matches_active = match endpoint {
            PaneEndpoint::Local => {
                self.session.active_file_browser_backend()
                    == Some(nyaterm_transport::FileBrowserBackendKind::Local)
            }
            PaneEndpoint::Session(id) => active.as_ref() == Some(id),
            PaneEndpoint::OfflineConnection(_) => false,
        };
        if matches_active
            && !self.transfer.browser_view().loading
            && browser.display_path == path.display_path
            && (browser.raw_path_token.is_none()
                || path.raw_path_token.is_none()
                || browser.raw_path_token == path.raw_path_token)
        {
            if let Some(active) = active {
                self.transfer.take_delete_refresh_pending(&active);
            }
            self.open_transfer_browser_directory_path(browser, cx);
        }
    }

    pub(in crate::features) fn mark_file_manager_session_disconnected(
        &mut self,
        session_id: &str,
        disconnected: bool,
        cx: &mut Context<Self>,
    ) {
        let workspaces: Vec<_> = self.file_manager.all_workspaces().collect();
        for workspace in workspaces {
            workspace.update(cx, |workspace, cx| {
                workspace.mark_disconnected(session_id, disconnected);
                cx.notify();
            });
        }
        if !disconnected {
            self.refresh_file_manager_session_panes(session_id, cx);
        }
    }

    pub(in crate::features) fn refresh_file_manager_session_panes(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        let workspaces: Vec<_> = self
            .file_manager
            .all_workspaces()
            .map(|entity| (entity.read(cx).id.clone(), entity.clone()))
            .collect();
        for (manager_id, workspace) in workspaces {
            let panes = workspace.read(cx).pane_ids_for_session(session_id);
            for pane in panes {
                self.refresh_file_manager_pane(manager_id.clone(), pane, cx);
            }
        }
    }

    pub(in crate::features) fn confirm_file_manager_delete_selection(
        &mut self,
        origin: String,
        selection: FileManagerSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if selection.entries.is_empty() {
            return;
        }
        let count = selection.entries.len();
        self.open_confirm_dialog(
            (
                t!("fileExplorer.delete").to_string(),
                t!("fileManager.deleteConfirm", count = count).to_string(),
                t!("fileExplorer.delete").to_string(),
                true,
                move |app, _, cx| {
                    for entry in &selection.entries {
                        app.submit_file_mutation(
                            origin.clone(),
                            FileMutationIntent {
                                endpoint: selection.endpoint.clone(),
                                directory: selection.directory.clone(),
                                mutation: FileMutation::Delete(entry.remote_path()),
                            },
                            cx,
                        );
                    }
                    true
                },
            ),
            window,
            cx,
        );
    }

    pub(in crate::features) fn open_file_manager_name_target(
        &mut self,
        origin: String,
        selection: FileManagerSelection,
        kind: FileManagerNameKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(kind, FileManagerNameKind::Rename) && selection.entries.len() != 1 {
            return;
        }
        self.forget_text_inputs("file-manager.name");
        let initial = if matches!(kind, FileManagerNameKind::Rename) {
            selection
                .entries
                .first()
                .map(|entry| entry.name.clone())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let title = match kind {
            FileManagerNameKind::Folder => t!("fileExplorer.newFolder"),
            FileManagerNameKind::File => t!("fileExplorer.newFile"),
            FileManagerNameKind::Rename => t!("fileExplorer.cmRename"),
        }
        .to_string();
        self.open_form_dialog(
            (
                title,
                420.,
                t!("common.confirm").to_string(),
                move |app, _, cx| {
                    app.text_input_box(
                        "file-manager.name",
                        &initial,
                        TextInputSetup::placeholder(t!("fileExplorer.name")),
                        cx,
                    )
                    .into_any_element()
                },
                move |app, _, cx| {
                    let name = app
                        .existing_text_input("file-manager.name")
                        .map(|field| field.read(cx).value(cx))
                        .unwrap_or_default();
                    if !nyaterm_transport::valid_file_browser_child_name(
                        selection.endpoint.kind(),
                        &name,
                    ) {
                        app.shell
                            .set_status(t!("fileManager.invalidName").to_string());
                        return false;
                    }
                    let mutation = match kind {
                        FileManagerNameKind::Folder => FileMutation::CreateDirectory(name),
                        FileManagerNameKind::File => FileMutation::CreateFile(name),
                        FileManagerNameKind::Rename => {
                            let Some(entry) = selection.entries.first() else {
                                return true;
                            };
                            FileMutation::Rename {
                                source: entry.remote_path(),
                                name,
                            }
                        }
                    };
                    app.submit_file_mutation(
                        origin.clone(),
                        FileMutationIntent {
                            endpoint: selection.endpoint.clone(),
                            directory: selection.directory.clone(),
                            mutation,
                        },
                        cx,
                    );
                    true
                },
                |app, cx| {
                    app.forget_text_inputs("file-manager.name");
                    cx.notify();
                },
            ),
            window,
            cx,
        );
    }

    pub(in crate::features) fn workspace_references_session(
        &self,
        session_id: &str,
        cx: &gpui::App,
    ) -> bool {
        self.transfer.editor_references_session(session_id)
            || self.file_manager.references_session(session_id, cx)
            || self.transfer.file_operation_references_session(session_id)
    }
}

impl FileManagerWorkspace {
    pub(in crate::features) fn transfer_intent(
        &self,
        operation: FileTransferOperation,
    ) -> crate::features::transfers::file_operations::FileTransferIntent {
        let source = self.active_pane();
        let target = self.pane(self.active.other());
        crate::features::transfers::file_operations::FileTransferIntent {
            source: source.endpoint.clone(),
            entries: source.selected_entries(),
            target: target.endpoint.clone(),
            target_directory: target.path.clone(),
            operation,
        }
    }

    pub(in crate::features) fn selection(&self, pane: PaneId) -> FileManagerSelection {
        let pane = self.pane(pane);
        FileManagerSelection {
            endpoint: pane.endpoint.clone(),
            directory: pane.path.clone(),
            entries: pane.selected_entries(),
        }
    }
}
