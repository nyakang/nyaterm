use super::pane::{PaneEndpoint, PaneId, PanePathMenu};
use crate::features::NyaTermApp;
use crate::features::runtime_jobs::await_blocking_job;
use gpui::Context;
use nyaterm_files::sftp::RemoteFilePath;

pub(super) struct ChildrenRequest {
    pub endpoint: PaneEndpoint,
    pub generation: u64,
    pub path: RemoteFilePath,
    pub branch: Option<RemoteFilePath>,
}

impl NyaTermApp {
    pub(super) fn resolve_file_manager_path_home(
        &mut self,
        manager: String,
        pane_id: PaneId,
        endpoint: PaneEndpoint,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self.file_manager.workspace(&manager) else {
            return;
        };
        let pane = workspace.read(cx).pane(pane_id);
        if pane.endpoint != endpoint || pane.generation != generation || !pane.home_dir.is_empty() {
            return;
        }
        let access_generation = endpoint
            .session_id()
            .and_then(|id| self.file_access_generation(id));
        let service = self.file_browser_service_for_endpoint(&endpoint);
        let task = self
            .blocking_jobs
            .submit_task("file-manager-path-home", move |_| {
                service
                    .and_then(|service| service.home_dir())
                    .map_err(|error| crate::features::android::labels::file_error_message(&error))
            });
        let workspace = workspace.downgrade();
        cx.spawn(async move |app, cx| {
            let result = await_blocking_job(task).await.and_then(|result| result);
            let _ = app.update(cx, |app, cx| {
                if endpoint
                    .session_id()
                    .and_then(|id| app.file_access_generation(id))
                    != access_generation
                {
                    return;
                }
                let Some(workspace) = workspace.upgrade() else {
                    return;
                };
                workspace.update(cx, |workspace, cx| {
                    let pane = workspace.pane_mut(pane_id);
                    if pane.endpoint != endpoint || pane.generation != generation {
                        return;
                    }
                    pane.home_dir_pending = false;
                    if let Ok(home) = result {
                        pane.home_dir = home;
                    }
                    workspace.refresh_path_completion(pane_id, cx);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(super) fn file_manager_children(
        &mut self,
        manager: String,
        pane_id: PaneId,
        intent: ChildrenRequest,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self.file_manager.workspace(&manager) else {
            return;
        };
        let ChildrenRequest {
            endpoint,
            generation: expected_generation,
            path,
            branch,
        } = intent;
        let pane = workspace.read(cx).pane(pane_id);
        if pane.endpoint != endpoint || pane.generation != expected_generation {
            return;
        }
        let access_generation = endpoint
            .session_id()
            .and_then(|id| self.file_access_generation(id));
        let (generation, request) = workspace.update(cx, |workspace, cx| {
            let pane = workspace.pane_mut(pane_id);
            pane.menu_request = pane.menu_request.wrapping_add(1);
            pane.open_path_menu(PanePathMenu::Children {
                path: path.clone(),
                branch,
                request: pane.menu_request,
                access_generation,
                result: None,
            });
            cx.notify();
            (pane.generation, pane.menu_request)
        });
        let service = self.file_browser_service_for_endpoint(&endpoint);
        let task = self.blocking_jobs.submit_task("file-manager-children", {
            let path = path.clone();
            move |_| {
                service
                    .and_then(|service| service.list_dir_path(&path))
                    .map_err(|error| crate::features::android::labels::file_error_message(&error))
            }
        });
        let workspace = workspace.downgrade();
        cx.spawn(async move |app, cx| {
            let result = await_blocking_job(task).await.and_then(|result| result);
            let _ = app.update(cx, |app, cx| {
                if endpoint
                    .session_id()
                    .and_then(|id| app.file_access_generation(id))
                    != access_generation
                {
                    return;
                }
                let Some(workspace) = workspace.upgrade() else {
                    return;
                };
                workspace.update(cx, |workspace, cx| {
                    let pane = workspace.pane_mut(pane_id);
                    if pane.apply_children_result(generation, &endpoint, request, &path, result) {
                        cx.notify();
                    }
                });
            });
        })
        .detach();
    }

    fn file_manager_favorite_connection(&self, endpoint: &PaneEndpoint) -> Option<String> {
        match endpoint {
            PaneEndpoint::Session(id) => self.connection_id_for_session(id),
            PaneEndpoint::OfflineConnection(id) => Some(id.clone()),
            _ => None,
        }
    }

    pub(super) fn file_manager_favorites(&self, endpoint: &PaneEndpoint) -> Vec<String> {
        if let Some(id) = self.file_manager_favorite_connection(endpoint) {
            return self
                .settings
                .summary()
                .ui_file_explorer_favorite_dirs_by_connection_id
                .get(&id)
                .cloned()
                .unwrap_or_default();
        }
        self.transfer.temporary_browser_favorites(endpoint)
    }

    pub(super) fn set_file_manager_favorite(
        &mut self,
        endpoint: PaneEndpoint,
        path: String,
        favorite: bool,
        cx: &mut Context<Self>,
    ) {
        if path.is_empty() {
            return;
        }
        let mut favorites = self.file_manager_favorites(&endpoint);
        favorites.retain(|p| p != &path);
        if favorite {
            favorites.insert(0, path);
            favorites.truncate(12);
        }
        if let Some(id) = self.file_manager_favorite_connection(&endpoint) {
            self.settings.set_file_explorer_favorites(id, favorites);
            self.sync_transfer_browser_favorites_for_active_session();
            self.queue_settings_save(
                crate::features::settings::SettingsSaveKind::FileExplorer,
                cx,
            );
        } else {
            self.transfer
                .set_temporary_browser_favorites(endpoint.clone(), favorites.clone());
            if endpoint.session_id().is_some() && endpoint.session_id() == self.session.active_id()
            {
                self.transfer.replace_browser_favorites(favorites.into());
            }
        }
        self.defer_transfer_panel_snapshot_flush(cx);
        cx.notify();
    }
}
