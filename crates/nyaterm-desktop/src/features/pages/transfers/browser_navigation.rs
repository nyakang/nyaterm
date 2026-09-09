use gpui::{Context, Window};
use nyaterm_transport::{RemoteFilePath, SftpFileEntry};

use std::collections::VecDeque;

use crate::features::NyaTermApp;
use crate::models::{TransferBrowserNavigationSnapshot, TransferBrowserSessionCacheState};

use super::{normalized_transfer_browser_path, remote_file_name, remote_parent_path};

impl NyaTermApp {
    pub(in crate::features::pages::transfers) fn valid_transfer_browser_child_name(
        &self,
        name: &str,
    ) -> bool {
        let backend = self
            .session
            .active_file_browser_backend()
            .unwrap_or(nyaterm_transport::FileBrowserBackendKind::Remote);
        nyaterm_transport::valid_file_browser_child_name(backend, name)
    }

    pub(in crate::features) fn cache_transfer_browser_session(&mut self, session_id: &str) {
        if session_id.trim().is_empty()
            || !self
                .session
                .file_browser_backend_support_for_session(session_id)
                .is_some()
        {
            return;
        }

        let current_path = normalized_transfer_browser_path(self.transfer.browser_view().path);
        if current_path.is_empty() {
            return;
        }

        let current_raw_path_token = self.transfer.browser_remote_file_path().raw_path_token;
        let browser = self.transfer.browser_view();
        let mut history = browser.history.clone();
        if history.is_empty() {
            history.push_back(current_path.clone());
        }
        let history_index = browser.history_index.min(history.len().saturating_sub(1));

        let home_dir = normalized_transfer_browser_path(browser.home_dir);
        let home_dir = if home_dir == "." {
            current_path.clone()
        } else {
            home_dir
        };

        let cache = TransferBrowserSessionCacheState {
            entries: browser.entries.clone(),
            current_path,
            current_raw_path_token,
            home_dir,
            history,
            history_index,
            visited_history: browser.visited_history.clone(),
        };
        self.transfer
            .store_browser_session_cache(session_id.to_string(), cache);
    }

    pub(in crate::features) fn restore_transfer_browser_session_cache(
        &mut self,
        session_id: &str,
    ) -> bool {
        let Some(remote_path) = self.transfer.restore_browser_session_cache(session_id) else {
            return false;
        };
        self.transfer.set_remote_path(remote_path);
        true
    }

    pub(in crate::features) fn reset_transfer_browser_for_active_session(&mut self) {
        self.transfer.set_remote_path(".");
        self.transfer
            .reset_browser_for_session(self.session.active_file_browser_backend().is_some());
    }

    pub(in crate::features) fn load_transfer_browser_for_active_session_if_needed(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(session_id) = self.session.active_id_owned() else {
            return;
        };
        if self.session.active_file_browser_backend().is_none()
            || self.session.is_disconnected(&session_id)
            || self.transfer.has_browser_session_cache(&session_id)
            || self.transfer.browser_view().loading
            || self
                .transfer
                .browser_navigation_job_running_for_session(&session_id)
        {
            return;
        }

        let initial_path = match self.session.active_file_browser_backend() {
            Some(nyaterm_transport::FileBrowserBackendKind::Local) => dirs::home_dir()
                .or_else(|| std::env::current_dir().ok())
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".to_string()),
            _ => ".".to_string(),
        };
        let rollback = self.prepare_transfer_browser_navigation();
        self.transfer
            .begin_browser_directory_load(initial_path.clone());
        self.record_transfer_browser_history(initial_path);
        self.start_sftp_list_job(None, rollback, cx);
    }

    pub(in crate::features::pages::transfers) fn open_transfer_browser_directory(
        &mut self,
        path: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rollback = self.prepare_transfer_browser_navigation();
        self.open_transfer_browser_directory_with_history_and_rollback(
            RemoteFilePath::new(path),
            true,
            rollback,
            cx,
        );
    }

    pub(in crate::features::pages::transfers) fn open_transfer_browser_entry_directory(
        &mut self,
        entry: SftpFileEntry,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rollback = self.prepare_transfer_browser_navigation();
        self.open_transfer_browser_directory_with_history_and_rollback(
            entry.remote_path(),
            true,
            rollback,
            cx,
        );
    }

    fn open_transfer_browser_directory_with_history_and_rollback(
        &mut self,
        path: RemoteFilePath,
        record_history: bool,
        rollback: TransferBrowserNavigationSnapshot,
        cx: &mut Context<Self>,
    ) {
        self.forget_text_inputs("transfer.browser.path");
        let display_path = path.display_path.clone();
        self.transfer.set_remote_path(display_path.clone());
        self.transfer.begin_browser_directory_load_path(path);
        if record_history {
            self.record_transfer_browser_history(display_path);
        } else {
            self.record_transfer_browser_visited_history(display_path);
        }
        self.start_sftp_list_job(None, rollback, cx);
    }

    pub(in crate::features) fn open_transfer_browser_history(
        &mut self,
        delta: isize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rollback = self.prepare_transfer_browser_navigation();
        let path = match self.transfer.browser_history_destination(delta) {
            Ok(path) => path,
            Err(status) => {
                self.transfer.set_browser_status(status);
                // 浏览器历史由根层的鼠标导航触发时，没有经过 TransferPanel::with_app。
                self.defer_transfer_panel_snapshot_flush(cx);
                cx.notify();
                return;
            }
        };
        self.open_transfer_browser_directory_with_history_and_rollback(
            RemoteFilePath::new(path),
            false,
            rollback,
            cx,
        );
        // 根层后退/前进没有面板实体的回调兜底，先发布 loading/path 快照。
        self.defer_transfer_panel_snapshot_flush(cx);
    }

    pub(in crate::features::pages::transfers) fn record_transfer_browser_history(
        &mut self,
        path: String,
    ) {
        let path = normalized_transfer_browser_path(&path);
        if path.is_empty() {
            return;
        }
        self.transfer.record_browser_history(path);
    }

    pub(in crate::features::pages::transfers) fn record_transfer_browser_visited_history(
        &mut self,
        path: String,
    ) {
        let path = normalized_transfer_browser_path(&path);
        if path.is_empty() {
            return;
        }
        self.transfer.record_browser_visited_history(path);
    }

    pub(in crate::features::pages::transfers) fn add_current_transfer_browser_favorite(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let path = normalized_transfer_browser_path(self.transfer.browser_view().path);
        self.add_transfer_browser_favorite_path(path, cx);
    }

    pub(in crate::features::pages::transfers) fn add_transfer_browser_favorite_path(
        &mut self,
        path: String,
        cx: &mut Context<Self>,
    ) {
        let path = normalized_transfer_browser_path(&path);
        if path.is_empty() {
            self.transfer
                .set_browser_status("open or select a remote directory before adding a favorite");
            cx.notify();
            return;
        }
        let existed = self.transfer.add_browser_favorite(path.clone());
        let status = if existed {
            format!("favorite directory moved to front: {path}")
        } else {
            format!("favorite directory added: {path}")
        };
        self.transfer.set_browser_status(status);
        self.persist_transfer_browser_favorites(cx);
        cx.notify();
    }

    pub(in crate::features::pages::transfers) fn remove_transfer_browser_favorite_path(
        &mut self,
        path: String,
        cx: &mut Context<Self>,
    ) {
        let path = normalized_transfer_browser_path(&path);
        if path.is_empty() {
            self.transfer
                .set_browser_status("favorite directory path is empty");
            cx.notify();
            return;
        }
        let removed = self.transfer.remove_browser_favorite(&path);
        let status = if removed {
            format!("favorite directory removed: {path}")
        } else {
            format!("favorite directory not found: {path}")
        };
        self.transfer.set_browser_status(status);
        if removed {
            self.persist_transfer_browser_favorites(cx);
        }
        cx.notify();
    }

    pub(in crate::features::pages::transfers) fn toggle_transfer_browser_auto_sync_cwd(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(connection_id) = self.active_transfer_browser_connection_id() else {
            self.transfer
                .set_browser_status("Auto CWD requires a saved local or SSH connection");
            cx.notify();
            return;
        };
        let enabled = self
            .settings
            .toggle_file_explorer_auto_sync_cwd(connection_id);
        if enabled {
            self.transfer
                .set_browser_status("Auto CWD enabled for this connection");
        } else {
            self.transfer
                .set_browser_status("Auto CWD disabled for this connection");
        }
        self.transfer.reset_browser_auto_sync_cwd();
        self.persist_transfer_browser_ui_settings(cx);
        if enabled {
            self.start_transfer_sync_cwd_job(cx);
        } else {
            cx.notify();
        }
    }

    pub(in crate::features::pages::transfers) fn toggle_transfer_browser_hidden_files(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let show_hidden_files = self.settings.toggle_file_explorer_hidden_files();
        if !show_hidden_files {
            self.transfer
                .retain_browser_selection(|path| !remote_file_name(path).starts_with('.'));
        }
        self.transfer.scroll_browser_to_item(0);
        let status = if show_hidden_files {
            "hidden files shown".to_string()
        } else {
            "hidden files hidden".to_string()
        };
        self.transfer.set_browser_status(status);
        self.persist_transfer_browser_ui_settings(cx);
        cx.notify();
    }

    pub(in crate::features) fn transfer_browser_auto_sync_cwd_enabled(&self) -> bool {
        let Some(connection_id) = self.active_transfer_browser_connection_id() else {
            return false;
        };
        self.settings
            .summary()
            .ui_file_explorer_auto_sync_cwd_connection_ids
            .iter()
            .any(|id| id == &connection_id)
    }

    pub(in crate::features) fn sync_transfer_browser_favorites_for_active_session(&mut self) {
        let Some(connection_id) = self.active_transfer_browser_connection_id() else {
            self.transfer.clear_browser_favorites();
            return;
        };
        let favorites = self
            .settings
            .summary()
            .ui_file_explorer_favorite_dirs_by_connection_id
            .get(&connection_id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|path| normalized_transfer_browser_path(&path))
            .filter(|path| !path.trim().is_empty())
            .fold(VecDeque::<String>::new(), |mut paths, path| {
                if !paths.iter().any(|existing| existing == &path) {
                    paths.push_back(path);
                }
                paths
            });
        self.transfer.replace_browser_favorites(favorites);
    }

    pub(in crate::features::pages::transfers) fn persist_transfer_browser_favorites(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(connection_id) = self.active_transfer_browser_connection_id() else {
            self.transfer
                .set_browser_status("favorite kept for this temporary session only");
            return;
        };
        let favorites = self.transfer.browser_favorites_owned();
        self.settings
            .set_file_explorer_favorites(connection_id, favorites);
        self.persist_transfer_browser_ui_settings(cx);
        cx.notify();
    }

    pub(in crate::features::pages::transfers) fn persist_transfer_browser_ui_settings(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.queue_settings_save(
            crate::features::settings::SettingsSaveKind::FileExplorer,
            cx,
        );
    }

    pub(in crate::features::pages::transfers) fn active_transfer_browser_connection_id(
        &self,
    ) -> Option<String> {
        let session_id = self.session.active_id()?;
        self.session
            .metadata(session_id)?
            .source_connection_id
            .clone()
            .filter(|connection_id| !connection_id.trim().is_empty())
    }

    pub(in crate::features::pages::transfers) fn open_transfer_parent_directory(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rollback = self.prepare_transfer_browser_navigation();
        let current_path = normalized_transfer_browser_path(self.transfer.browser_view().path);
        if current_path == "/" || current_path == "." {
            self.transfer
                .set_browser_status("already at the top remote directory");
            cx.notify();
            return;
        }
        let current_identity = self.transfer.browser_remote_file_path();
        let parent_identity = current_identity
            .parent()
            .unwrap_or_else(|_| RemoteFilePath::new(remote_parent_path(&current_path)));
        let parent = parent_identity.display_path.clone();
        if parent == current_path {
            self.transfer
                .set_browser_status("remote parent directory is unavailable");
            cx.notify();
            return;
        }
        self.transfer.set_remote_path(parent.clone());
        self.transfer
            .begin_browser_parent_load_path(parent_identity);
        self.record_transfer_browser_history(parent);
        self.start_sftp_list_job(Some(current_path), rollback, cx);
    }

    pub(in crate::features::pages::transfers) fn refresh_transfer_browser(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = if self.transfer.browser_view().path.trim().is_empty() {
            self.transfer.normalized_remote_path()
        } else {
            self.transfer.browser_view().path.clone()
        };
        let rollback = self.prepare_transfer_browser_navigation();
        let path = if path == self.transfer.browser_view().path.as_str() {
            self.transfer.browser_remote_file_path()
        } else {
            RemoteFilePath::new(path)
        };
        self.open_transfer_browser_directory_with_history_and_rollback(path, true, rollback, cx);
    }
}

impl NyaTermApp {
    fn prepare_transfer_browser_navigation(&mut self) -> TransferBrowserNavigationSnapshot {
        let session_key = self.session.active_id_owned().unwrap_or_default();
        let remote_path = self.transfer.remote_path().to_string();
        let snapshot = self
            .transfer
            .prepare_browser_navigation(&session_key, remote_path);
        self.transfer.set_remote_path(snapshot.remote_path.clone());
        snapshot
    }

    pub(in crate::features) fn restore_transfer_browser_navigation(
        &mut self,
        snapshot: TransferBrowserNavigationSnapshot,
    ) {
        let remote_path = self.transfer.restore_browser_navigation(snapshot);
        self.transfer.set_remote_path(remote_path);
    }
}
