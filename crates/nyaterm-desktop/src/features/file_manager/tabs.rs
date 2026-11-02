use gpui::{Context, div, prelude::*};
use nyaterm_core::{FileManagerPaneRestore, FileManagerPaneSide, FileManagerSourceRestore};
use nyaterm_transport::RemoteFilePath;
use rust_i18n::t;

use super::pane::{PaneEndpoint, PaneId};
use super::state::FileManagerSidebarRestore;
use super::workspace::{FileManagerSeed, FileManagerWorkspace};
use crate::features::NyaTermApp;
use crate::models::{NavItem, PanelSide, WorkspaceTabId};

impl NyaTermApp {
    pub(in crate::features) fn open_file_manager_tab(
        &mut self,
        seed: Option<FileManagerSeed>,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self.file_manager.active_id().map(str::to_string) {
            let key = WorkspaceTabId::FileManager(id).layout_key();
            if self.workspace_tab_ids().contains(&key) {
                self.activate_workspace_tab(&key, cx);
                return;
            }
        }
        let id = self.file_manager.next_id();
        let seed = seed.unwrap_or_else(|| self.default_file_manager_seed());
        let app = cx.entity();
        let workspace = cx.new(|cx| FileManagerWorkspace::new(app, id.clone(), seed, cx));
        workspace.update(cx, |workspace, _| {
            workspace.restore_settings(&self.settings.summary().ui_file_manager);
        });
        self.file_manager.insert(id.clone(), workspace);
        let key = WorkspaceTabId::FileManager(id.clone()).layout_key();
        let current = self.active_workspace_tab();
        self.ensure_terminal_windows_root();
        if let Some(mut tree) = self.shell.terminal_window_tree() {
            let group = current
                .as_deref()
                .and_then(|key| tree.leaf_for_tab(key))
                .map(str::to_string)
                .or_else(|| tree.first_leaf_id());
            tree.ensure_tab(&key, group.as_deref());
            self.shell.restore_terminal_window_tree(Some(tree));
        }
        self.activate_workspace_tab(&key, cx);
        self.refresh_file_manager_pane(id.clone(), PaneId::Left, cx);
        self.refresh_file_manager_pane(id, PaneId::Right, cx);
        self.persist_file_manager_settings(cx);
        cx.notify();
    }

    pub(in crate::features) fn default_file_manager_seed(&self) -> FileManagerSeed {
        let restore = &self.settings.summary().ui_file_manager;
        if !restore.remember_layout {
            let (right, right_path) = self
                .seed_from_browser()
                .unwrap_or((PaneEndpoint::Local, None));
            return FileManagerSeed {
                left: PaneEndpoint::Local,
                left_path: None,
                right,
                right_path,
            };
        }
        let (right, right_path) = self
            .seed_from_browser()
            .unwrap_or_else(|| self.resolve_pane_restore(&restore.right));
        let (left, left_path) = self.resolve_pane_restore(&restore.left);
        FileManagerSeed {
            left,
            left_path,
            right,
            right_path,
        }
    }

    fn seed_from_browser(&self) -> Option<(PaneEndpoint, Option<RemoteFilePath>)> {
        let session_id = self.session.active_id()?;
        self.session
            .file_browser_backend_support_for_session(session_id)?;
        let path = self.transfer.browser_view().path.clone();
        let path = (!path.is_empty()).then(|| RemoteFilePath {
            display_path: path,
            raw_path_token: self.transfer.browser_view().raw_path_token.clone(),
        });
        let endpoint = if self
            .session
            .file_browser_backend_support_for_session(session_id)
            == Some(nyaterm_transport::FileBrowserBackendKind::Local)
        {
            PaneEndpoint::Local
        } else {
            PaneEndpoint::Session(session_id.to_string())
        };
        Some((endpoint, path))
    }

    fn resolve_pane_restore(
        &self,
        restore: &FileManagerPaneRestore,
    ) -> (PaneEndpoint, Option<RemoteFilePath>) {
        let path = (!restore.path.is_empty()).then(|| RemoteFilePath {
            display_path: restore.path.clone(),
            raw_path_token: restore.raw_path_token.clone(),
        });
        match &restore.source {
            FileManagerSourceRestore::Local => (PaneEndpoint::Local, path),
            FileManagerSourceRestore::Connection { id } if !id.is_empty() => {
                if let Some(session_id) = self.session_id_for_connection(id) {
                    (PaneEndpoint::Session(session_id), path)
                } else {
                    (PaneEndpoint::OfflineConnection(id.clone()), path)
                }
            }
            FileManagerSourceRestore::Connection { .. } => (PaneEndpoint::Local, None),
        }
    }

    pub(in crate::features) fn session_id_for_connection(
        &self,
        connection_id: &str,
    ) -> Option<String> {
        self.session
            .metadata_entries()
            .filter(|(session_id, _)| !self.session.is_disconnected(session_id))
            .find_map(|(session_id, metadata)| {
                (metadata.source_connection_id.as_deref() == Some(connection_id)
                    && self
                        .session
                        .file_browser_backend_support_for_session(session_id)
                        .is_some())
                .then(|| session_id.to_string())
            })
    }

    pub(in crate::features) fn connection_id_for_session(
        &self,
        session_id: &str,
    ) -> Option<String> {
        self.session
            .metadata(session_id)
            .and_then(|metadata| metadata.source_connection_id.clone())
            .filter(|id| !id.is_empty())
    }

    pub(in crate::features) fn activate_file_manager_tab(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self.file_manager.workspace(id) else {
            return;
        };
        self.session.clear_active_start_selection();
        let key = WorkspaceTabId::FileManager(id.to_string()).layout_key();
        self.shell.workspace.focused_group = self.shell.sync_terminal_windows_active_tab(&key);
        self.shell.set_pane_focus_mode(false);
        self.shell.navigation.selected_nav = NavItem::Workspace;
        self.shell.request_session_tab_scroll_into_view();
        self.close_action_link_menu(cx);
        self.collapse_sidebars_for_file_manager(cx);
        workspace.update(cx, |workspace, cx| workspace.request_focus(cx));
        cx.notify();
    }

    pub(in crate::features) fn remove_workspace_file_manager_tab(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        let key = WorkspaceTabId::FileManager(id.to_string()).layout_key();
        self.file_manager.remove(id);
        if let Some(mut tree) = self.shell.terminal_window_tree() {
            let next = tree.remove_tab(&key);
            self.shell.restore_terminal_window_tree(next);
        }
        self.restore_sidebars_from_file_manager(cx);
        self.release_unreferenced_hidden_sessions(cx);
        self.persist_file_manager_settings(cx);
    }

    pub(in crate::features) fn open_file_manager_for_session(
        &mut self,
        session_id: &str,
        pane: Option<PaneId>,
        cx: &mut Context<Self>,
    ) {
        if self
            .session
            .file_browser_backend_support_for_session(session_id)
            .is_none()
        {
            return;
        }
        let pane = pane.unwrap_or(PaneId::Right);
        let endpoint = if self
            .session
            .file_browser_backend_support_for_session(session_id)
            == Some(nyaterm_transport::FileBrowserBackendKind::Local)
        {
            PaneEndpoint::Local
        } else {
            PaneEndpoint::Session(session_id.into())
        };
        let browser = self.transfer.browser_view();
        let path = if self.session.active_id() == Some(session_id) && !browser.path.is_empty() {
            Some(RemoteFilePath {
                display_path: browser.path.clone(),
                raw_path_token: browser.raw_path_token.clone(),
            })
        } else {
            None
        };
        if let Some(id) = self.file_manager.active_id().map(str::to_string)
            && let Some(workspace) = self.file_manager.workspace(&id)
        {
            workspace.update(cx, |workspace, _| {
                let state = workspace.pane_mut(pane);
                state.set_endpoint(endpoint);
                if let Some(path) = path {
                    state.path = path;
                }
                state.clear_selection();
                workspace.set_active(pane);
            });
            self.activate_workspace_tab(&WorkspaceTabId::FileManager(id.clone()).layout_key(), cx);
            self.refresh_file_manager_pane(id, pane, cx);
        } else {
            let mut seed = self.default_file_manager_seed();
            match pane {
                PaneId::Left => {
                    seed.left = endpoint;
                    seed.left_path = path;
                }
                PaneId::Right => {
                    seed.right = endpoint;
                    seed.right_path = path;
                }
            }
            self.open_file_manager_tab(Some(seed), cx);
        }
    }

    fn collapse_sidebars_for_file_manager(&mut self, cx: &mut Context<Self>) {
        if !self
            .settings
            .summary()
            .ui_file_manager
            .auto_collapse_sidebar
        {
            return;
        }
        if self.file_manager.sidebar_restore_pending() {
            return;
        }
        let left_item = self.current_left_panel();
        let right_item = self.current_right_panel();
        let left_is_transfers = left_item == Some(NavItem::Transfers);
        let right_is_transfers = right_item == Some(NavItem::Transfers);
        if !left_is_transfers && !right_is_transfers {
            return;
        }
        self.file_manager
            .set_sidebar_restore(FileManagerSidebarRestore {
                left_open: left_is_transfers,
                left_item,
                left_collapsed: self.shell.panels.left_collapsed,
                right_open: right_is_transfers,
                right_item,
                right_collapsed: self.shell.panels.right_collapsed,
            });
        if left_is_transfers {
            self.shell.panels.left_collapsed = true;
        }
        if right_is_transfers {
            self.shell.panels.right_collapsed = true;
        }
        cx.notify();
    }

    pub(in crate::features) fn restore_sidebars_from_file_manager(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(restore) = self.file_manager.take_sidebar_restore() else {
            return;
        };
        if restore.left_open {
            self.shell.panels.left_collapsed = restore.left_collapsed;
            if let Some(item) = restore.left_item
                && !restore.left_collapsed
            {
                self.shell.panels.active_left = Some(item);
            }
        }
        if restore.right_open {
            self.shell.panels.right_collapsed = restore.right_collapsed;
            if let Some(item) = restore.right_item
                && !restore.right_collapsed
            {
                self.shell.panels.active_right = Some(item);
            }
        }
        let _ = PanelSide::Left;
        cx.notify();
    }

    pub(in crate::features) fn note_file_manager_manual_sidebar_open(&mut self) {
        if self.file_manager.sidebar_restore_pending() {
            self.file_manager.suppress_sidebar_restore();
        }
    }

    pub(in crate::features) fn persist_file_manager_settings(&mut self, cx: &mut Context<Self>) {
        let open = self.file_manager.workspaces().next().is_some();
        let mut settings = self.settings.summary().ui_file_manager.clone();
        settings.open = open;
        if let Some(workspace) = self
            .file_manager
            .workspaces()
            .next()
            .map(|(_, workspace)| workspace.clone())
        {
            let snapshot = workspace.read(cx);
            if settings.remember_layout {
                settings.split_percent = snapshot.split_percent;
                settings.active_pane = match snapshot.active {
                    PaneId::Left => FileManagerPaneSide::Left,
                    PaneId::Right => FileManagerPaneSide::Right,
                };
                settings.queue_expanded = snapshot.queue_expanded;
                settings.queue_height = snapshot.queue_height.max(80.) as u32;
                settings.left = self.pane_restore_from(snapshot, PaneId::Left);
                settings.right = self.pane_restore_from(snapshot, PaneId::Right);
                settings.left_view = self.file_manager_view_settings(snapshot, PaneId::Left);
                settings.right_view = self.file_manager_view_settings(snapshot, PaneId::Right);
            }
        }
        self.settings.set_file_manager_ui(settings);
        self.queue_settings_save(
            crate::features::settings::SettingsSaveKind::FileExplorer,
            cx,
        );
    }

    fn pane_restore_from(
        &self,
        workspace: &FileManagerWorkspace,
        id: PaneId,
    ) -> FileManagerPaneRestore {
        let pane = workspace.pane(id);
        FileManagerPaneRestore {
            source: match &pane.endpoint {
                PaneEndpoint::Local => FileManagerSourceRestore::Local,
                PaneEndpoint::OfflineConnection(id) => {
                    FileManagerSourceRestore::Connection { id: id.clone() }
                }
                PaneEndpoint::Session(session_id) => {
                    match self.connection_id_for_session(session_id) {
                        Some(id) => FileManagerSourceRestore::Connection { id },
                        None => FileManagerSourceRestore::Local,
                    }
                }
            },
            path: if matches!(&pane.endpoint, PaneEndpoint::Session(id) if self.connection_id_for_session(id).is_none())
            {
                String::new()
            } else {
                pane.last_successful_path
                    .as_ref()
                    .unwrap_or(&pane.path)
                    .display_path
                    .clone()
            },
            raw_path_token: if matches!(&pane.endpoint, PaneEndpoint::Session(id) if self.connection_id_for_session(id).is_none())
            {
                None
            } else {
                pane.last_successful_path
                    .as_ref()
                    .unwrap_or(&pane.path)
                    .raw_path_token
                    .clone()
            },
        }
    }

    fn file_manager_view_settings(
        &self,
        workspace: &FileManagerWorkspace,
        id: PaneId,
    ) -> nyaterm_core::models::settings::FileManagerPaneUiSettings {
        let pane = workspace.pane(id);
        let mut settings = pane.view_settings();
        let mut remember = |endpoint: &PaneEndpoint, path: &RemoteFilePath| {
            let source = match endpoint {
                PaneEndpoint::Local => FileManagerSourceRestore::Local,
                PaneEndpoint::OfflineConnection(id) => {
                    FileManagerSourceRestore::Connection { id: id.clone() }
                }
                PaneEndpoint::Session(session) => {
                    let Some(id) = self.connection_id_for_session(session) else {
                        return;
                    };
                    FileManagerSourceRestore::Connection { id }
                }
            };
            if path.display_path.is_empty() {
                return;
            }
            let value = FileManagerPaneRestore {
                source: source.clone(),
                path: path.display_path.clone(),
                raw_path_token: path.raw_path_token.clone(),
            };
            if let Some(existing) = settings
                .directories
                .iter_mut()
                .find(|entry| entry.source == source)
            {
                *existing = value;
            } else {
                settings.directories.push(value);
            }
        };
        for (endpoint, snapshot) in &pane.source_history {
            remember(endpoint, &snapshot.path);
        }
        if let Some(path) = &pane.last_successful_path {
            remember(&pane.endpoint, path);
        }
        settings
    }

    pub(in crate::features) fn switch_file_manager_source(
        &mut self,
        id: String,
        pane: PaneId,
        endpoint: PaneEndpoint,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self.file_manager.workspace(&id) else {
            return;
        };
        let source = match &endpoint {
            PaneEndpoint::Local => Some(FileManagerSourceRestore::Local),
            PaneEndpoint::OfflineConnection(id) => {
                Some(FileManagerSourceRestore::Connection { id: id.clone() })
            }
            PaneEndpoint::Session(session) => self
                .connection_id_for_session(session)
                .map(|id| FileManagerSourceRestore::Connection { id }),
        };
        workspace.update(cx, |workspace, cx| {
            let state = workspace.pane_mut(pane);
            state.set_endpoint(endpoint);
            state.clear_selection();
            if state.path.display_path.is_empty()
                && let Some(source) = &source
                && let Some(restore) = state
                    .remembered_directories
                    .iter()
                    .find(|entry| &entry.source == source)
            {
                state.path = RemoteFilePath {
                    display_path: restore.path.clone(),
                    raw_path_token: restore.raw_path_token.clone(),
                };
            }
            workspace
                .inputs(pane)
                .search
                .update(cx, |input, cx| input.set_content_silent("", cx));
            cx.notify();
        });
        self.refresh_file_manager_pane(id, pane, cx);
    }

    pub(in crate::features) fn bind_file_manager_sources(&mut self, cx: &mut Context<Self>) {
        let targets: Vec<_> = self
            .file_manager
            .workspaces()
            .flat_map(|(id, workspace)| {
                [PaneId::Left, PaneId::Right]
                    .into_iter()
                    .filter_map(|pane| {
                        if let PaneEndpoint::OfflineConnection(connection) =
                            &workspace.read(cx).pane(pane).endpoint
                        {
                            self.session_id_for_connection(connection)
                                .filter(|session| !self.session.is_disconnected(session))
                                .map(|session| (id.clone(), workspace.clone(), pane, session))
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        for (id, workspace, pane, session) in targets {
            workspace.update(cx, |workspace, cx| {
                let state = workspace.pane_mut(pane);
                state.endpoint = PaneEndpoint::Session(session);
                state.generation = state.generation.wrapping_add(1);
                state.disconnected = false;
                cx.notify();
            });
            self.refresh_file_manager_pane(id, pane, cx);
        }
    }

    pub(in crate::features) fn reconnect_file_manager_source(
        &mut self,
        endpoint: PaneEndpoint,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        match endpoint {
            PaneEndpoint::Session(session) => self.reconnect_session(session, window, cx),
            PaneEndpoint::OfflineConnection(connection) => {
                if let Some(saved) = self.connection_state.connection_by_id(&connection).cloned() {
                    self.start_saved_connection(saved, window, cx);
                } else {
                    self.shell
                        .set_status(t!("fileManager.sourceUnavailable").to_string());
                }
            }
            PaneEndpoint::Local => {}
        }
    }

    pub(in crate::features) fn restore_file_manager_after_sessions(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if !self.settings.summary().ui_file_manager.open {
            return;
        }
        if self.file_manager.workspaces().next().is_some() {
            if let Some(id) = self.file_manager.active_id().map(str::to_string) {
                self.refresh_file_manager_pane(id.clone(), PaneId::Left, cx);
                self.refresh_file_manager_pane(id, PaneId::Right, cx);
            }
            return;
        }
        self.open_file_manager_tab(None, cx);
    }

    pub(in crate::features) fn file_manager_tab_title(&self, id: &str, cx: &gpui::App) -> String {
        let Some(workspace) = self.file_manager.workspace(id) else {
            return t!("fileManager.tabTitle").to_string();
        };
        let workspace = workspace.read(cx);
        let left = self.endpoint_label(&workspace.pane(PaneId::Left).endpoint);
        let right = self.endpoint_label(&workspace.pane(PaneId::Right).endpoint);
        format!("{} · {left} | {right}", t!("fileManager.tabTitle"))
    }

    pub(in crate::features) fn endpoint_label(&self, endpoint: &PaneEndpoint) -> String {
        match endpoint {
            PaneEndpoint::Local => t!("fileManager.local").to_string(),
            PaneEndpoint::OfflineConnection(id) => self
                .connection_state
                .connection_by_id(id)
                .map(|connection| {
                    format!("{} · {}", connection.name, t!("fileManager.disconnected"))
                })
                .unwrap_or_else(|| t!("fileManager.sourceUnavailable").to_string()),
            PaneEndpoint::Session(id) => self
                .session
                .display_name(id)
                .unwrap_or_else(|| t!("fileManager.session").to_string()),
        }
    }

    pub(in crate::features) fn file_manager_source_choices(&self) -> Vec<(PaneEndpoint, String)> {
        let mut choices = vec![(PaneEndpoint::Local, t!("fileManager.local").to_string())];
        for session_id in self.session.session_ids() {
            if self
                .session
                .file_browser_backend_support_for_session(session_id)
                .is_none()
            {
                continue;
            }
            if self
                .session
                .file_browser_backend_support_for_session(session_id)
                == Some(nyaterm_transport::FileBrowserBackendKind::Local)
            {
                continue;
            }
            let mut label = self.endpoint_label(&PaneEndpoint::Session(session_id.to_string()));
            label = format!(
                "{label} · {}",
                t!(if self.session.is_disconnected(session_id) {
                    "fileManager.disconnected"
                } else {
                    "fileManager.connected"
                })
            );
            choices.push((PaneEndpoint::Session(session_id.to_string()), label));
        }
        choices
    }
}

impl FileManagerWorkspace {
    pub(in crate::features) fn request_focus(&mut self, cx: &mut Context<Self>) {
        self.focus_pending = true;
        cx.notify();
    }
}

impl NyaTermApp {
    pub(in crate::features) fn workspace_file_manager_content(
        &mut self,
        id: &str,
        _cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.file_manager
            .workspace(id)
            .map(|view| view.into_any_element())
            .unwrap_or_else(|| div().into_any_element())
    }
}

impl NyaTermApp {
    pub(in crate::features) fn session_is_sftp_only(&self, id: &str) -> bool {
        self.session
            .metadata(id)
            .and_then(|metadata| metadata.ssh_config.as_ref())
            .is_some_and(|config| {
                config.effective_runtime_mode()
                    == nyaterm_core::models::ssh_runtime::SshRuntimeMode::Sftp
            })
    }

    pub(in crate::features) fn ensure_sftp_session_view(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        self.file_manager
            .sftp_workspaces
            .retain(|id, _| self.session.has_session(id));
        if !self.session_is_sftp_only(id) || self.file_manager.sftp_workspaces.contains_key(id) {
            return;
        }
        if let Some(metadata) = self.session.metadata_mut(id) {
            metadata.ai_execution_profile = nyaterm_core::AiExecutionProfile::Disabled;
        }
        let app = cx.entity();
        let seed = FileManagerSeed {
            left: PaneEndpoint::Session(id.to_string()),
            left_path: None,
            right: PaneEndpoint::Local,
            right_path: None,
        };
        let view = cx.new(|cx| {
            let mut workspace = FileManagerWorkspace::new(app, format!("sftp-{id}"), seed, cx);
            workspace.single_pane = true;
            workspace
        });
        self.file_manager
            .sftp_workspaces
            .insert(id.to_string(), view);
        self.refresh_file_manager_pane(format!("sftp-{id}"), PaneId::Left, cx);
    }

    pub(in crate::features) fn sftp_session_content(&self, id: &str) -> gpui::AnyElement {
        self.file_manager
            .sftp_workspaces
            .get(id)
            .map(|view| view.clone().into_any_element())
            .unwrap_or_else(|| div().into_any_element())
    }
}
