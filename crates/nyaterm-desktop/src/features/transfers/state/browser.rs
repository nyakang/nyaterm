//! SFTP browser navigation, selection, menu and column-layout transitions.

use gpui::{Pixels, ScrollHandle, ScrollStrategy};
use nyaterm_files::sftp::{RemoteFilePath, SftpFileEntry};
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use super::browser_logic::BrowserFilterKey;
use super::{TransferBrowserState, TransferBrowserView, TransferFeatureState};
use crate::features::transfers::browser_paths::{
    remote_ancestor_selection, transfer_browser_path_is_root,
};
#[cfg(test)]
use crate::models::TransferBrowserPendingRenameState;
use crate::models::{
    TransferBrowserColumnResizeState, TransferBrowserContextTarget,
    TransferBrowserDragSelectionState, TransferBrowserFavoritesMenuState,
    TransferBrowserNavigationSnapshot, TransferBrowserPathMenuState,
    TransferBrowserSessionCacheState, TransferBrowserSortColumn, TransferBrowserUploadMenuState,
};

pub(in crate::features) struct TransferSessionTransferBundle {
    caches: Vec<(String, TransferBrowserSessionCacheState)>,
}

impl TransferFeatureState {
    pub(in crate::features) fn detach_sessions_for_transfer(
        &mut self,
        session_ids: &[String],
    ) -> TransferSessionTransferBundle {
        let mut caches = Vec::new();
        for id in session_ids {
            if let Some(cache) = self.browser.session_cache.remove(id) {
                caches.push((id.clone(), cache));
            }
            self.invalidate_browser_navigation(id);
            self.tree.remove_session(id);
        }
        TransferSessionTransferBundle { caches }
    }

    pub(in crate::features) fn attach_sessions_from_transfer(
        &mut self,
        bundle: TransferSessionTransferBundle,
    ) {
        self.browser.session_cache.extend(bundle.caches);
    }

    pub(in crate::features) fn retains_transfer_session(&self, id: &str) -> bool {
        self.browser.session_cache.contains_key(id) || self.browser.navigation_jobs.contains_key(id)
    }

    pub(in crate::features) fn browser_view(&self) -> TransferBrowserView<'_> {
        TransferBrowserView {
            path: &self.browser.path,
            raw_path_token: &self.browser.raw_path_token,
            home_dir: &self.browser.home_dir,
            home_dir_pending: self.browser.home_dir_pending,
            path_draft: &self.browser.path_draft,
            path_editing: self.browser.path_editing,
            entries: &self.browser.entries,
            loading: self.browser.loading,
            error: &self.browser.error,
            search: &self.browser.search,
            list_scroll: &self.browser.list_scroll,
            horizontal_scroll: &self.browser.horizontal_scroll,
            search_expanded: self.browser.search_expanded,
            history: &self.browser.history,
            history_index: self.browser.history_index,
            visited_history: &self.browser.visited_history,
            favorites: &self.browser.favorites,
            sort_column: self.browser.sort_column,
            sort_direction: self.browser.sort_direction,
            column_widths: self.browser.column_widths,
            column_resize: &self.browser.column_resize,
            selected_remote_path: &self.browser.selected_remote_path,
            selected_remote_paths: &self.browser.selected_remote_paths,
            drag_selection: &self.browser.drag_selection,
            external_drop_hover: self.browser.external_drop_hover,
            context_target: &self.browser.context_target,
            favorites_menu: &self.browser.favorites_menu,
            path_menu: &self.browser.path_menu,
            path_menu_scroll: &self.browser.path_menu_scroll,
            upload_menu: &self.browser.upload_menu,
            focus: &self.browser.focus,
        }
    }

    pub(in crate::features) fn set_browser_status(&mut self, status: impl Into<String>) {
        self.browser.status = status.into();
    }

    pub(in crate::features) fn select_browser_path(&mut self, path: impl Into<String>) {
        self.browser.selected_remote_path = Some(path.into());
    }

    /// The filtered, sorted listing the browser draws.
    ///
    /// Recomputed only when one of its real inputs moved. `show_hidden` is passed in
    /// because it lives in settings while everything else here is transfer state.
    pub(in crate::features) fn visible_browser_entries(
        &mut self,
        show_hidden: bool,
    ) -> Arc<[SftpFileEntry]> {
        let key = BrowserFilterKey::new(
            &self.browser.entries,
            &self.browser.search,
            show_hidden,
            self.browser.sort_column,
            self.browser.sort_direction,
        );
        self.browser_filter.entries(key, &self.browser.entries)
    }

    /// Replace the listing the way a store reply does: whole.
    #[cfg(test)]
    pub(in crate::features) fn replace_browser_entries_for_test(
        &mut self,
        entries: Vec<SftpFileEntry>,
    ) {
        self.browser.entries = Arc::new(entries);
    }

    #[cfg(test)]
    pub(in crate::features) fn browser_directory_requests_for_test(
        &mut self,
    ) -> &mut Vec<(String, RemoteFilePath)> {
        self.browser.directory_requests.get_or_insert_with(Vec::new)
    }

    #[cfg(test)]
    pub(in crate::features) fn browser_filter_recomputes(&self) -> usize {
        self.browser_filter.recomputes()
    }

    pub(in crate::features) fn browser_entries_are_empty(&self) -> bool {
        self.browser.entries.is_empty()
    }

    pub(in crate::features) fn clear_pending_cwd_sync(&mut self) {
        self.browser.pending_cwd_sync = false;
    }

    pub(in crate::features) fn defer_browser_cwd_sync(&mut self) {
        self.browser.pending_cwd_sync = true;
    }

    pub(in crate::features) fn take_pending_cwd_sync(&mut self) -> bool {
        std::mem::take(&mut self.browser.pending_cwd_sync)
    }

    /// Retire only automatic navigation; explicit Sync and user navigation keep ownership.
    pub(in crate::features) fn cancel_browser_auto_cwd_sync(&mut self, session_id: &str) -> bool {
        let Some(id) = self.browser.navigation_jobs.get(session_id) else {
            return false;
        };
        if !self.queue.jobs.iter().any(|job| {
            &job.id == id
                && matches!(
                    job.kind,
                    crate::models::TransferJobKind::SyncCwd {
                        reason: crate::models::TransferCwdSyncReason::Automatic,
                    }
                )
        }) {
            return false;
        }
        if let Some(snapshot) = self.invalidate_browser_navigation(session_id) {
            let remote_path = snapshot.remote_path.clone();
            self.browser.restore_navigation(snapshot);
            self.set_remote_path(remote_path);
        }
        true
    }

    pub(in crate::features) fn invalidate_browser_navigation(
        &mut self,
        session_id: &str,
    ) -> Option<TransferBrowserNavigationSnapshot> {
        let id = self.browser.navigation_jobs.remove(session_id)?;
        if let Some(handle) = self.browser.navigation_handles.remove(&id) {
            handle.cancel();
        }
        self.settle_tree_navigation(session_id, &id, false);
        self.browser.pending_navigations.remove(&id)
    }

    pub(in crate::features) fn make_browser_cwd_sync_explicit(&mut self, session_id: &str) {
        let Some(id) = self.browser.navigation_jobs.get(session_id) else {
            return;
        };
        if let Some(job) = self.queue.jobs.iter_mut().find(|job| &job.id == id)
            && let crate::models::TransferJobKind::SyncCwd { reason } = &mut job.kind
        {
            *reason = crate::models::TransferCwdSyncReason::Explicit;
        }
        self.renew_tree_navigation(session_id);
    }

    pub(in crate::features) fn remove_browser_session_cache(&mut self, session_id: &str) {
        // Retiring the latest job rejects late replies from the closed session.
        self.invalidate_browser_navigation(session_id);
        self.tree.remove_session(session_id);
        self.browser.session_cache.remove(session_id);
        self.file_ops.delete_refresh_pending.remove(session_id);
    }

    pub(in crate::features) fn has_browser_session_cache(&self, session_id: &str) -> bool {
        self.browser.session_cache.contains_key(session_id)
    }

    pub(in crate::features) fn browser_session_cache(
        &self,
        session_id: &str,
    ) -> Option<&TransferBrowserSessionCacheState> {
        self.browser.session_cache.get(session_id)
    }

    /// Refresh a cached session's listing after a background write to it (e.g. a
    /// cross-session "Send to"). Only replaces the listing when the cache's
    /// current directory matches `parent_path`, so an unrelated cached directory
    /// is left untouched. Returns whether a cache entry was updated. The active
    /// session is never switched by this.
    pub(in crate::features) fn refresh_browser_session_cache_listing(
        &mut self,
        session_id: &str,
        parent_path: &str,
        entries: Vec<SftpFileEntry>,
    ) -> bool {
        let Some(cache) = self.browser.session_cache.get_mut(session_id) else {
            return false;
        };
        fn normalize(value: &str) -> &str {
            value.trim().trim_end_matches('/')
        }
        if normalize(&cache.current_path) != normalize(parent_path) {
            return false;
        }
        cache.entries = Arc::new(entries);
        true
    }

    pub(in crate::features) fn browser_navigation_job_running_for_session(
        &self,
        session_id: &str,
    ) -> bool {
        self.browser.navigation_jobs.contains_key(session_id)
    }

    pub(in crate::features) fn browser_navigation_rollback(
        &self,
        session_id: &str,
    ) -> Option<&TransferBrowserNavigationSnapshot> {
        let id = self.browser.navigation_jobs.get(session_id)?;
        self.browser.pending_navigations.get(id)
    }

    pub(in crate::features) fn scroll_browser_to_item(&mut self, offset: usize) {
        self.browser
            .list_scroll
            .scroll_to_item_strict(offset, ScrollStrategy::Top);
    }

    /// Resolve navigation intent against the same filtered, sorted rows the view draws.
    /// Return an operation path, never the token-based selection identity.
    pub(in crate::features) fn restore_browser_navigation_selection(
        &mut self,
        requested: &str,
        show_hidden: bool,
    ) -> Option<String> {
        self.clear_browser_selection();
        let visible = self.visible_browser_entries(show_hidden);
        let exact = visible
            .iter()
            .position(|entry| entry.identity_key() == requested);
        let index = exact.or_else(|| {
            // Legacy callers pass display paths. Do not guess when decoding made
            // two remote names indistinguishable.
            let mut matches = visible
                .iter()
                .enumerate()
                .filter(|(_, entry)| entry.path == requested);
            let (index, _) = matches.next()?;
            matches.next().is_none().then_some(index)
        })?;
        let entry = &visible[index];
        self.select_browser_entry(entry.identity_key());
        let parent_count = usize::from(!transfer_browser_path_is_root(&self.browser.path));
        self.browser
            .list_scroll
            .scroll_to_item(index + parent_count, ScrollStrategy::Nearest);
        Some(entry.path.clone())
    }

    /// An inactive session loads its stable cache before receiving a listing.
    /// Recover the ancestor's token instead of pairing the new path with that cache's old token.
    pub(in crate::features) fn restore_browser_ancestor_path_token(
        &mut self,
        job_id: &str,
        requested: &str,
    ) {
        let Some(snapshot) = self.browser.pending_navigations.get(job_id) else {
            return;
        };
        let source = RemoteFilePath {
            display_path: snapshot.browser_path.clone(),
            raw_path_token: snapshot.browser_raw_path_token.clone(),
        };
        let mut destination = RemoteFilePath::new(self.browser.path.clone());
        if remote_ancestor_selection(source, &mut destination).as_deref() == Some(requested) {
            self.browser.raw_path_token = destination.raw_path_token;
        }
    }

    pub(in crate::features) fn start_browser_column_resize(
        &mut self,
        column: TransferBrowserSortColumn,
        position_x: Pixels,
    ) {
        self.browser.start_column_resize(column, position_x);
    }

    pub(in crate::features) fn update_browser_column_resize(&mut self, position_x: Pixels) -> bool {
        self.browser.update_column_resize(position_x)
    }

    pub(in crate::features) fn finish_browser_column_resize(&mut self) -> bool {
        self.browser.finish_column_resize()
    }

    pub(in crate::features) fn cancel_browser_path_edit(&mut self) {
        self.browser.cancel_path_edit();
    }

    pub(in crate::features) fn set_browser_search(&mut self, search: String) {
        self.browser.search = search;
        self.browser
            .list_scroll
            .scroll_to_item_strict(0, ScrollStrategy::Top);
        self.browser
            .horizontal_scroll
            .set_offset(Default::default());
    }

    pub(in crate::features) fn expand_browser_search(&mut self) {
        self.browser.search_expanded = true;
    }

    pub(in crate::features) fn close_browser_search(&mut self) {
        self.browser.search_expanded = false;
    }

    pub(in crate::features) fn clear_browser_search(&mut self) {
        self.browser.search.clear();
        self.browser
            .list_scroll
            .scroll_to_item_strict(0, ScrollStrategy::Top);
        self.browser
            .horizontal_scroll
            .set_offset(Default::default());
    }

    pub(in crate::features) fn toggle_browser_sort(
        &mut self,
        column: TransferBrowserSortColumn,
    ) -> String {
        if self.browser.sort_column == column {
            self.browser.sort_direction = self.browser.sort_direction.toggled();
        } else {
            self.browser.sort_column = column;
            self.browser.sort_direction = column.default_direction();
        }
        self.browser
            .list_scroll
            .scroll_to_item_strict(0, ScrollStrategy::Top);
        self.browser
            .horizontal_scroll
            .set_offset(Default::default());
        let status = format!(
            "sorted by {} {}",
            self.browser.sort_column.label().to_lowercase(),
            self.browser.sort_direction.marker()
        );
        self.browser.status = status.clone();
        status
    }

    pub(in crate::features) fn begin_browser_path_edit(&mut self, path: String) {
        self.browser.path_menu = None;
        self.browser.favorites_menu = None;
        self.browser.upload_menu = None;
        self.browser.path_draft = path;
        self.browser.path_editing = true;
        self.browser.status = "editing remote directory path".to_string();
    }

    pub(in crate::features) fn update_browser_path_draft(&mut self, path: String) {
        self.browser.path_draft = path;
        self.browser.status = "editing remote directory path".to_string();
    }

    pub(in crate::features) fn finish_browser_path_edit(&mut self) {
        self.browser.path_draft.clear();
        self.browser.path_editing = false;
        self.browser.pending_cwd_sync = false;
    }

    pub(in crate::features) fn dismiss_browser_path_edit(&mut self) {
        self.browser.path_editing = false;
    }

    pub(in crate::features) fn open_browser_favorites_menu(
        &mut self,
        menu: TransferBrowserFavoritesMenuState,
        status: impl Into<String>,
    ) {
        self.browser.upload_menu = None;
        self.browser.path_menu = None;
        self.browser.favorites_menu = Some(menu);
        self.browser.status = status.into();
    }

    pub(in crate::features) fn close_browser_favorites_menu(&mut self) {
        self.browser.favorites_menu = None;
    }

    pub(in crate::features) fn open_browser_upload_menu(
        &mut self,
        menu: TransferBrowserUploadMenuState,
    ) {
        self.browser.favorites_menu = None;
        self.browser.path_menu = None;
        self.browser.upload_menu = Some(menu);
        self.browser.status = "upload menu opened".to_string();
    }

    pub(in crate::features) fn close_browser_upload_menu(&mut self) {
        self.browser.upload_menu = None;
    }

    pub(in crate::features) fn open_browser_path_menu(
        &mut self,
        menu: TransferBrowserPathMenuState,
    ) {
        self.browser.favorites_menu = None;
        self.browser.upload_menu = None;
        self.browser.path_menu_scroll.set_offset(Default::default());
        self.browser.path_menu = Some(menu);
    }

    pub(in crate::features) fn close_browser_path_menu(&mut self) {
        self.browser.path_menu = None;
    }

    pub(in crate::features) fn store_browser_session_cache(
        &mut self,
        session_id: String,
        cache: TransferBrowserSessionCacheState,
    ) {
        self.browser.session_cache.insert(session_id, cache);
    }

    pub(in crate::features) fn restore_browser_session_cache(
        &mut self,
        session_id: &str,
    ) -> Option<String> {
        let cache = self.browser.session_cache.get(session_id)?.clone();
        let remote_path = cache.current_path.clone();
        self.browser.path = cache.current_path;
        self.browser.raw_path_token = cache.current_raw_path_token;
        self.browser.home_dir = cache.home_dir;
        self.browser.home_dir_pending = false;
        self.browser.path_draft.clear();
        self.browser.path_editing = false;
        self.browser.entries = cache.entries;
        self.browser.loading = false;
        self.browser.error = None;
        self.browser.status = format!(
            "restored cached directory · {} item(s)",
            self.browser.entries.len()
        );
        self.browser.history = cache.history;
        self.browser.history_index = cache
            .history_index
            .min(self.browser.history.len().saturating_sub(1));
        self.browser.visited_history = cache.visited_history;
        self.browser.list_scroll = gpui::UniformListScrollHandle::new();
        self.browser.horizontal_scroll = ScrollHandle::new();
        self.browser.clear_interaction();
        Some(remote_path)
    }

    pub(in crate::features) fn reset_browser_for_session(
        &mut self,
        browser_available: bool,
        initial_directory: Option<String>,
    ) {
        let path = initial_directory.clone().unwrap_or_else(|| ".".into());
        self.set_remote_path(path.clone());
        self.browser.path = path;
        self.browser.raw_path_token = None;
        self.browser.home_dir = initial_directory.unwrap_or_default();
        self.browser.home_dir_pending = false;
        self.browser.path_draft.clear();
        self.browser.path_editing = false;
        self.browser.entries = Arc::new(Vec::new());
        self.browser.loading = false;
        self.browser.error = None;
        self.browser.status = if browser_available {
            "List a directory to browse files.".to_string()
        } else {
            "Start a local or SSH session to browse files.".to_string()
        };
        self.browser.history.clear();
        self.browser.history_index = 0;
        self.browser.visited_history.clear();
        self.browser.list_scroll = gpui::UniformListScrollHandle::new();
        self.browser.horizontal_scroll = ScrollHandle::new();
        self.browser.clear_interaction();
    }

    pub(in crate::features) fn begin_browser_directory_load(&mut self, path: String) {
        self.begin_browser_directory_load_path(RemoteFilePath::new(path));
    }

    pub(in crate::features) fn begin_browser_directory_load_path(&mut self, path: RemoteFilePath) {
        self.cancel_browser_selection_click();
        self.browser.list_scroll = gpui::UniformListScrollHandle::new();
        self.browser.horizontal_scroll = ScrollHandle::new();
        self.browser.path = path.display_path;
        self.browser.raw_path_token = path.raw_path_token;
        self.browser.path_draft.clear();
        self.browser.path_editing = false;
        self.browser.pending_cwd_sync = false;
        self.browser.path_menu = None;
        self.browser.selected_remote_path = None;
        self.browser.selected_remote_paths.clear();
        self.browser.context_target = TransferBrowserContextTarget::CurrentDirectory;
        self.browser.cancel_pending_rename();
        self.browser.status = "Loading remote directory...".to_string();
        self.browser.loading = true;
        self.browser.error = None;
    }

    pub(in crate::features) fn begin_browser_parent_load(&mut self, path: String) {
        self.cancel_browser_selection_click();
        self.browser.list_scroll = gpui::UniformListScrollHandle::new();
        self.browser.horizontal_scroll = ScrollHandle::new();
        self.browser.path = path;
        self.browser.raw_path_token = None;
        self.browser.selected_remote_path = None;
        self.browser.selected_remote_paths.clear();
        self.browser.context_target = TransferBrowserContextTarget::CurrentDirectory;
        self.browser.cancel_pending_rename();
        self.browser.status = "Loading parent directory...".to_string();
        self.browser.loading = true;
        self.browser.error = None;
    }

    pub(in crate::features) fn begin_browser_parent_load_path(&mut self, path: RemoteFilePath) {
        self.begin_browser_parent_load(path.display_path.clone());
        self.browser.raw_path_token = path.raw_path_token;
    }

    pub(in crate::features) fn browser_remote_file_path(&self) -> RemoteFilePath {
        RemoteFilePath {
            display_path: self.browser.path.clone(),
            raw_path_token: self.browser.raw_path_token.clone(),
        }
    }

    pub(in crate::features) fn browser_history_destination(
        &mut self,
        delta: isize,
    ) -> Result<String, &'static str> {
        if self.browser.history.is_empty() {
            return Err("directory history is empty");
        }
        let next = self.browser.history_index as isize + delta;
        if next < 0 || next as usize >= self.browser.history.len() {
            return Err(if delta > 0 {
                "no older directory history"
            } else {
                "no newer directory history"
            });
        }
        self.browser.history_index = next as usize;
        self.browser
            .history
            .get(self.browser.history_index)
            .cloned()
            .ok_or("directory history entry is unavailable")
    }

    pub(in crate::features) fn record_browser_history(&mut self, path: String) {
        self.browser.record_history(path);
    }

    pub(in crate::features) fn record_browser_visited_history(&mut self, path: String) {
        self.browser.record_visited_history(path);
    }

    pub(in crate::features) fn add_browser_favorite(&mut self, path: String) -> bool {
        let existed = self
            .browser
            .favorites
            .iter()
            .any(|existing| existing == &path);
        self.browser.favorites.retain(|existing| existing != &path);
        self.browser.favorites.push_front(path);
        self.browser.favorites.truncate(12);
        existed
    }

    pub(in crate::features) fn remove_browser_favorite(&mut self, path: &str) -> bool {
        let previous_len = self.browser.favorites.len();
        self.browser.favorites.retain(|existing| existing != path);
        self.browser.favorites.len() < previous_len
    }

    pub(in crate::features) fn replace_browser_favorites(&mut self, favorites: VecDeque<String>) {
        self.browser.favorites = favorites;
        self.browser.favorites.truncate(12);
    }

    pub(in crate::features) fn browser_favorites_owned(&self) -> Vec<String> {
        self.browser.favorites.iter().cloned().collect()
    }

    pub(in crate::features) fn clear_browser_favorites(&mut self) {
        self.browser.favorites.clear();
    }

    pub(in crate::features) fn retain_browser_selection(
        &mut self,
        mut retain: impl FnMut(&str) -> bool,
    ) {
        self.browser
            .selected_remote_paths
            .retain(|path| retain(path));
        if self
            .browser
            .selected_remote_path
            .as_deref()
            .is_some_and(|path| !retain(path))
        {
            self.browser.selected_remote_path = None;
        }
    }

    pub(in crate::features) fn select_browser_entry(&mut self, path: String) {
        self.cancel_browser_selection_click();
        self.browser.selected_remote_path = Some(path.clone());
        self.browser.selected_remote_paths.clear();
        self.browser.selected_remote_paths.insert(path);
    }

    pub(in crate::features) fn replace_browser_selection(
        &mut self,
        paths: HashSet<String>,
        active_path: Option<String>,
    ) -> usize {
        self.cancel_browser_selection_click();
        self.browser.selected_remote_paths = paths;
        self.browser.selected_remote_path = active_path;
        self.browser.selected_remote_paths.len()
    }

    pub(in crate::features) fn clear_browser_selection(&mut self) {
        self.cancel_browser_selection_click();
        self.browser.selected_remote_path = None;
        self.browser.selected_remote_paths.clear();
    }

    pub(in crate::features) fn set_browser_context_target(
        &mut self,
        target: TransferBrowserContextTarget,
    ) {
        self.cancel_browser_selection_click();
        self.browser.context_target = target;
        self.browser.cancel_pending_rename();
    }

    pub(in crate::features) fn arm_browser_rename_click(
        &mut self,
        path: &str,
        is_unmodified_single_click: bool,
    ) -> bool {
        let pending_cancelled = self.browser.cancel_pending_rename();
        let next_candidate = (is_unmodified_single_click
            && self.browser.selected_remote_path.as_deref() == Some(path)
            && self.browser.selected_remote_paths.len() == 1
            && self.browser.selected_remote_paths.contains(path))
        .then(|| path.to_string());
        let changed = self.browser.rename_click_candidate != next_candidate;
        self.browser.rename_click_candidate = next_candidate;
        pending_cancelled || changed
    }

    pub(in crate::features) fn consume_browser_rename_click(&mut self, path: &str) -> bool {
        self.browser.rename_click_candidate.take().as_deref() == Some(path)
    }

    pub(in crate::features) fn clear_browser_rename_click(&mut self) -> bool {
        self.browser.rename_click_candidate.take().is_some()
    }

    pub(in crate::features) fn activate_marked_browser_path(
        &mut self,
        path: &str,
    ) -> Option<usize> {
        self.browser.drag_selection = None;
        if !self.browser.selected_remote_paths.contains(path) {
            return None;
        }
        self.browser.selected_remote_path = Some(path.to_string());
        Some(self.browser.selected_remote_paths.len())
    }

    pub(in crate::features) fn toggle_browser_path_mark(&mut self, path: String) -> usize {
        if !self.browser.selected_remote_paths.remove(&path) {
            self.browser.selected_remote_paths.insert(path.clone());
            self.browser.selected_remote_path = Some(path);
        } else if !self
            .browser
            .selected_remote_path
            .as_ref()
            .is_some_and(|active| self.browser.selected_remote_paths.contains(active))
        {
            self.browser.selected_remote_path = self
                .browser
                .entries
                .iter()
                .map(SftpFileEntry::identity_key)
                .find(|identity| self.browser.selected_remote_paths.contains(identity));
        }
        self.browser.selected_remote_paths.len()
    }

    pub(in crate::features) fn defer_browser_selection_click(&mut self, path: String) {
        self.browser.pending_selection_click = Some(path);
    }

    pub(in crate::features) fn consume_browser_selection_click(&mut self, path: &str) -> bool {
        if self.browser.pending_selection_click.as_deref() != Some(path) {
            return false;
        }
        self.browser.pending_selection_click = None;
        true
    }

    pub(in crate::features) fn cancel_browser_selection_click(&mut self) {
        self.browser.pending_selection_click = None;
    }

    pub(in crate::features) fn set_browser_drag_selection(
        &mut self,
        selection: TransferBrowserDragSelectionState,
    ) {
        self.cancel_browser_selection_click();
        self.browser.drag_selection = Some(selection);
    }

    pub(in crate::features) fn clear_browser_drag_selection(&mut self) {
        self.browser.drag_selection = None;
    }

    pub(in crate::features) fn finish_browser_drag_selection(&mut self) -> bool {
        self.browser.drag_selection.take().is_some()
    }

    pub(in crate::features) fn set_browser_external_drop_hover(&mut self, hover: bool) -> bool {
        if self.browser.external_drop_hover == hover {
            return false;
        }
        self.browser.external_drop_hover = hover;
        true
    }

    pub(in crate::features) fn browser_external_drop_hover_is_pending(&self) -> bool {
        self.browser.external_drop_hover
    }

    #[cfg(test)]
    pub(in crate::features) fn schedule_browser_pending_rename(
        &mut self,
        path: &str,
    ) -> Option<u64> {
        if self.browser.selected_remote_path.as_deref() != Some(path)
            || self.browser.selected_remote_paths.len() != 1
            || !self.browser.selected_remote_paths.contains(path)
        {
            return None;
        }
        self.browser.pending_rename_token = self.browser.pending_rename_token.wrapping_add(1);
        let token = self.browser.pending_rename_token;
        self.browser.pending_rename = Some(TransferBrowserPendingRenameState {
            path: path.to_string(),
            token,
        });
        Some(token)
    }

    pub(in crate::features) fn cancel_browser_pending_rename(&mut self) -> bool {
        self.browser.cancel_pending_rename()
    }

    pub(in crate::features) fn prepare_browser_navigation(
        &mut self,
        session_key: &str,
        remote_path: String,
    ) -> TransferBrowserNavigationSnapshot {
        self.clear_pending_cwd_sync();
        if let Some(snapshot) = self.invalidate_browser_navigation(session_key) {
            self.browser.restore_navigation(snapshot.clone());
            return snapshot;
        }
        self.browser.capture_navigation(remote_path)
    }

    pub(in crate::features) fn restore_browser_navigation(
        &mut self,
        snapshot: TransferBrowserNavigationSnapshot,
    ) -> String {
        let remote_path = snapshot.remote_path.clone();
        self.browser.restore_navigation(snapshot);
        remote_path
    }
}

/// Column resize is self-contained: it only reads and writes browser geometry.
///
/// Keeping it here rather than on `NyaTermApp` means a drag cannot reach any
/// other app state; the page-level handlers are forwarders that own the redraw.
impl TransferBrowserState {
    fn clear_interaction(&mut self) {
        self.pending_selection_click = None;
        self.selected_remote_path = None;
        self.selected_remote_paths.clear();
        self.drag_selection = None;
        self.external_drop_hover = false;
        self.context_target = TransferBrowserContextTarget::CurrentDirectory;
        self.cancel_pending_rename();
        self.favorites_menu = None;
        self.path_menu = None;
        self.upload_menu = None;
    }

    fn record_history(&mut self, path: String) {
        if super::browser_logic::record_browser_history(
            &mut self.history,
            &mut self.history_index,
            &path,
        ) {
            self.record_visited_history(path);
        }
    }

    fn record_visited_history(&mut self, path: String) {
        self.visited_history.retain(|existing| existing != &path);
        self.visited_history.push_front(path);
        self.visited_history.truncate(30);
    }

    fn capture_navigation(&self, remote_path: String) -> TransferBrowserNavigationSnapshot {
        TransferBrowserNavigationSnapshot {
            remote_path,
            browser_path: self.path.clone(),
            browser_raw_path_token: self.raw_path_token.clone(),
            entries: self.entries.clone(),
            loading: self.loading,
            error: self.error.clone(),
            status: self.status.clone(),
            history: self.history.clone(),
            history_index: self.history_index,
            visited_history: self.visited_history.clone(),
            selected_path: self.selected_remote_path.clone(),
            selected_paths: self.selected_remote_paths.clone(),
            list_scroll: self.list_scroll.clone(),
            horizontal_scroll: self.horizontal_scroll.clone(),
        }
    }

    fn restore_navigation(&mut self, snapshot: TransferBrowserNavigationSnapshot) {
        self.pending_selection_click = None;
        self.path = snapshot.browser_path;
        self.raw_path_token = snapshot.browser_raw_path_token;
        self.entries = snapshot.entries;
        self.loading = snapshot.loading;
        self.error = snapshot.error;
        self.status = snapshot.status;
        self.history = snapshot.history;
        self.history_index = snapshot
            .history_index
            .min(self.history.len().saturating_sub(1));
        self.visited_history = snapshot.visited_history;
        self.selected_remote_path = snapshot.selected_path;
        self.selected_remote_paths = snapshot.selected_paths;
        self.list_scroll = snapshot.list_scroll;
        self.horizontal_scroll = snapshot.horizontal_scroll;
        self.context_target = TransferBrowserContextTarget::CurrentDirectory;
        self.cancel_pending_rename();
    }

    pub(super) fn cancel_pending_rename(&mut self) -> bool {
        let pending_cancelled = self.pending_rename.take().is_some();
        let candidate_cancelled = self.rename_click_candidate.take().is_some();
        if pending_cancelled {
            self.pending_rename_token = self.pending_rename_token.wrapping_add(1);
        }
        pending_cancelled || candidate_cancelled
    }

    fn cancel_path_edit(&mut self) {
        self.path_draft.clear();
        self.path_editing = false;
        self.status = "remote directory path edit cancelled".to_string();
    }

    fn start_column_resize(&mut self, column: TransferBrowserSortColumn, position_x: Pixels) {
        self.column_resize = Some(TransferBrowserColumnResizeState {
            column,
            start_x: position_x,
            start_width: self.column_widths.get(column),
        });
        self.status = format!("resizing {} column", column.label().to_lowercase());
    }

    /// Returns false when no resize is in flight, so the caller can skip the redraw.
    fn update_column_resize(&mut self, position_x: Pixels) -> bool {
        let Some(state) = self.column_resize else {
            return false;
        };
        let next_width = state.start_width + (position_x - state.start_x);
        self.column_widths.set(state.column, next_width);
        let width = f32::from(self.column_widths.get(state.column)).round();
        self.status = format!("{} column: {width}px", state.column.label().to_lowercase());
        true
    }

    /// Returns false when no resize was in flight, so the caller can skip the redraw.
    fn finish_column_resize(&mut self) -> bool {
        if self.column_resize.take().is_none() {
            return false;
        }
        self.status = "file column width updated".to_string();
        true
    }
}

impl TransferFeatureState {
    pub(in crate::features) fn invalidate_file_operation_directory(
        &mut self,
        session: &str,
        path: &nyaterm_files::sftp::RemoteFilePath,
    ) {
        if let Some(cache) = self.browser.session_cache.get(session)
            && cache.current_path == path.display_path
            && (cache.current_raw_path_token.is_none()
                || path.raw_path_token.is_none()
                || cache.current_raw_path_token == path.raw_path_token)
        {
            self.file_ops
                .delete_refresh_pending
                .insert(session.to_string());
        }
        self.invalidate_tree_path(session, path, false);
    }
}
