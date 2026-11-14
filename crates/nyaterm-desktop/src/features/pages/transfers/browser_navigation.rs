use super::{normalized_transfer_browser_path, remote_file_name, remote_parent_path};
use crate::features::NyaTermApp;
use crate::models::{TransferBrowserNavigationSnapshot, TransferBrowserSessionCacheState};
use gpui::{Context, Window};
use nyaterm_files::file_browser::navigation::remote_ancestor_selection;
use nyaterm_files::sftp::{RemoteFilePath, SftpFileEntry};
use std::collections::VecDeque;

#[cfg(test)]
mod tests;

impl NyaTermApp {
    pub(in crate::features::pages::transfers) fn toggle_transfer_browser_view_mode(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let mode = self.settings.toggle_file_explorer_view_mode();
        self.persist_transfer_browser_ui_settings(cx);
        if mode == nyaterm_core::TransferBrowserViewMode::Tree {
            self.reveal_transfer_tree_current_path(cx);
        } else if let Some(session) = self.session.active_id_owned() {
            self.transfer.cancel_tree_session_requests(&session);
        }
        self.defer_transfer_panel_snapshot_flush(cx);
        cx.notify();
    }

    pub(in crate::features::pages::transfers) fn valid_transfer_browser_child_name(
        &self,
        name: &str,
    ) -> bool {
        let backend = self
            .session
            .active_file_browser_backend()
            .unwrap_or(nyaterm_files::file_browser::FileBrowserBackendKind::Remote);
        nyaterm_files::file_browser::valid_file_browser_child_name(backend, name)
    }

    pub(in crate::features) fn android_file_initial_directory(
        &self,
        session_id: &str,
    ) -> Option<String> {
        let config = match &self.session.metadata(session_id)?.launch_config {
            crate::models::SessionLaunchConfig::AdbShell(config)
            | crate::models::SessionLaunchConfig::AdbFiles(config)
            | crate::models::SessionLaunchConfig::AdbScreen(config) => config,
            _ => return None,
        };
        Some(if config.directory.is_empty() {
            nyaterm_core::models::android::default_android_directory()
        } else {
            config.directory.clone()
        })
    }

    pub(in crate::features) fn validate_android_browser_cache(&mut self, session_id: &str) {
        if self.android_file_initial_directory(session_id).is_none() {
            return;
        }
        let invalid = self
            .transfer
            .browser_session_cache(session_id)
            .is_some_and(|cache| {
                nyaterm_files::adb::raw_path(&RemoteFilePath {
                    display_path: cache.current_path.clone(),
                    raw_path_token: cache.current_raw_path_token.clone(),
                })
                .is_err()
                    || (!cache.home_dir.is_empty()
                        && nyaterm_files::adb::raw_path(&RemoteFilePath::new(&cache.home_dir))
                            .is_err())
                    || cache
                        .history
                        .iter()
                        .chain(cache.visited_history.iter())
                        .any(|path| {
                            nyaterm_files::adb::raw_path(&RemoteFilePath::new(path)).is_err()
                        })
            });
        if invalid {
            self.transfer.remove_browser_session_cache(session_id);
        }
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

        // A loading path still displays the previous listing. Cache the stable
        // snapshot so switching away cannot associate those entries with a new path.
        let browser = self.transfer.browser_view();
        let stable = browser
            .loading
            .then(|| self.transfer.browser_navigation_rollback(session_id))
            .flatten();
        let current_path =
            normalized_transfer_browser_path(stable.map_or(browser.path.as_str(), |snapshot| {
                snapshot.browser_path.as_str()
            }));
        if current_path.is_empty() {
            return;
        }

        let current_raw_path_token = stable.map_or_else(
            || self.transfer.browser_remote_file_path().raw_path_token,
            |snapshot| snapshot.browser_raw_path_token.clone(),
        );
        let mut history = stable
            .map_or(browser.history, |snapshot| &snapshot.history)
            .clone();
        if history.is_empty() {
            history.push_back(current_path.clone());
        }
        let history_index = stable
            .map_or(browser.history_index, |snapshot| snapshot.history_index)
            .min(history.len().saturating_sub(1));

        let home_dir = normalized_transfer_browser_path(browser.home_dir);
        let home_dir = if home_dir == "." {
            current_path.clone()
        } else {
            home_dir
        };

        let cache = TransferBrowserSessionCacheState {
            entries: stable
                .map_or(browser.entries, |snapshot| &snapshot.entries)
                .clone(),
            current_path,
            current_raw_path_token,
            home_dir,
            history,
            history_index,
            visited_history: stable
                .map_or(browser.visited_history, |snapshot| {
                    &snapshot.visited_history
                })
                .clone(),
        };
        self.transfer
            .store_browser_session_cache(session_id.to_string(), cache);
    }

    pub(in crate::features) fn restore_transfer_browser_session_cache(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        self.validate_android_browser_cache(session_id);
        let Some(remote_path) = self.transfer.restore_browser_session_cache(session_id) else {
            return false;
        };
        self.transfer.set_remote_path(remote_path);
        if self.transfer.take_delete_refresh_pending(session_id) {
            let rollback = self.prepare_transfer_browser_navigation();
            let path = self.transfer.browser_remote_file_path();
            self.open_transfer_browser_directory_with_history_and_rollback(
                path, false, rollback, cx,
            );
        }
        true
    }

    pub(in crate::features) fn reset_transfer_browser_for_active_session(&mut self) {
        let initial_directory = self
            .session
            .active_id()
            .and_then(|id| self.android_file_initial_directory(id));
        self.transfer.reset_browser_for_session(
            self.session.active_file_browser_backend().is_some(),
            initial_directory,
        );
    }

    pub(in crate::features) fn load_transfer_browser_for_active_session_if_needed(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(session_id) = self.session.active_id_owned() else {
            return;
        };
        self.validate_android_browser_cache(&session_id);
        if self.session.active_file_browser_backend().is_none()
            || self.session.is_disconnected(&session_id)
            || self.transfer.has_browser_session_cache(&session_id)
            || self.transfer.browser_view().loading
            || self.transfer.browser_view().error.is_some()
            || self
                .transfer
                .browser_navigation_job_running_for_session(&session_id)
        {
            return;
        }

        if self.session.active_file_browser_backend()
            == Some(nyaterm_files::file_browser::FileBrowserBackendKind::Remote)
        {
            self.start_transfer_initial_directory_job(cx);
            return;
        }
        let initial_path = self
            .android_file_initial_directory(&session_id)
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .or_else(|| std::env::current_dir().ok())
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_else(|| ".".to_string())
            });
        let rollback = self.prepare_transfer_browser_navigation();
        self.transfer
            .begin_browser_directory_load(initial_path.clone());
        self.record_transfer_browser_history(initial_path);
        self.start_sftp_list_job(None, rollback, cx);
    }

    pub(in crate::features) fn open_transfer_browser_directory(
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

    pub(in crate::features) fn open_transfer_browser_entry_directory(
        &mut self,
        entry: SftpFileEntry,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_transfer_browser_directory_path(entry.remote_path(), cx);
    }

    pub(in crate::features) fn open_transfer_browser_directory_path(
        &mut self,
        path: RemoteFilePath,
        cx: &mut Context<Self>,
    ) {
        let rollback = self.prepare_transfer_browser_navigation();
        self.open_transfer_browser_directory_with_history_and_rollback(path, true, rollback, cx);
    }

    fn open_transfer_browser_directory_with_history_and_rollback(
        &mut self,
        mut path: RemoteFilePath,
        record_history: bool,
        rollback: TransferBrowserNavigationSnapshot,
        cx: &mut Context<Self>,
    ) {
        let select_after = if self.session.active_file_browser_backend()
            == Some(nyaterm_files::file_browser::FileBrowserBackendKind::Remote)
        {
            remote_ancestor_selection(
                RemoteFilePath {
                    display_path: rollback.browser_path.clone(),
                    raw_path_token: rollback.browser_raw_path_token.clone(),
                },
                &mut path,
            )
        } else {
            None
        };
        self.forget_text_inputs("transfer.browser.path");
        let display_path = path.display_path.clone();
        self.transfer.set_remote_path(display_path.clone());
        self.transfer.begin_browser_directory_load_path(path);
        if record_history {
            self.record_transfer_browser_history(display_path);
        } else {
            self.record_transfer_browser_visited_history(display_path);
        }
        self.start_sftp_list_job(select_after, rollback, cx);
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

    pub(in crate::features) fn toggle_transfer_browser_auto_sync_cwd(
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
        self.persist_transfer_browser_ui_settings(cx);
        if enabled {
            self.sync_transfer_browser_to_cwd(crate::models::TransferCwdSyncReason::Automatic, cx);
        } else {
            if let Some(session_id) = self.session.active_id_owned() {
                self.transfer.cancel_browser_auto_cwd_sync(&session_id);
            }
            self.transfer.clear_pending_cwd_sync();
        }
        self.defer_transfer_panel_snapshot_flush(cx);
        cx.notify();
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
        if self.session.active_file_browser_backend()
            == Some(nyaterm_files::file_browser::FileBrowserBackendKind::Remote)
        {
            self.open_transfer_browser_directory_with_history_and_rollback(
                parent_identity,
                true,
                rollback,
                cx,
            );
            return;
        }
        self.transfer.set_remote_path(parent.clone());
        self.transfer
            .begin_browser_parent_load_path(parent_identity);
        self.record_transfer_browser_history(parent);
        self.start_sftp_list_job(Some(current_path), rollback, cx);
    }

    pub(in crate::features) fn refresh_transfer_browser(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reload_transfer_browser(cx);
    }

    pub(in crate::features) fn reload_transfer_browser(&mut self, cx: &mut Context<Self>) {
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
    pub(in crate::features) fn prepare_transfer_browser_navigation(
        &mut self,
    ) -> TransferBrowserNavigationSnapshot {
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
