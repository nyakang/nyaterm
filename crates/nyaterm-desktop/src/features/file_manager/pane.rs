//! Independent file-manager pane state. Not shared with the sidebar browser.

use crate::features::transfers::{BrowserFilterCache, BrowserFilterKey};
use crate::models::{
    TransferBrowserColumnWidths, TransferBrowserSortColumn, TransferBrowserSortDirection,
};
use gpui::{FocusHandle, ScrollHandle, UniformListScrollHandle};
use nyaterm_files::file_browser::FileBrowserBackendKind;
use nyaterm_files::sftp::{RemoteFilePath, SftpFileEntry};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::features) enum PaneId {
    Left,
    Right,
}

impl PaneId {
    pub(in crate::features) fn other(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }

    pub(in crate::features) fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

pub(in crate::features) use crate::features::transfers::file_operations::FileOperationEndpoint as PaneEndpoint;

#[derive(Clone)]
pub(in crate::features) struct PaneNavigationSnapshot {
    pub path: RemoteFilePath,
    pub home_dir: String,
    pub entries: Arc<Vec<SftpFileEntry>>,
    pub loading: bool,
    pub error: Option<String>,
    pub status: String,
    pub history: VecDeque<RemoteFilePath>,
    pub history_index: usize,
    pub selected_path: Option<String>,
    pub selected_paths: HashSet<String>,
}

pub(in crate::features) struct BrowserPaneState {
    pub endpoint: PaneEndpoint,
    pub path: RemoteFilePath,
    pub home_dir: String,
    pub home_dir_pending: bool,
    pub entries: Arc<Vec<SftpFileEntry>>,
    pub loading: bool,
    pub error: Option<String>,
    pub status: String,
    pub disconnected: bool,
    pub search: String,
    pub sort_column: TransferBrowserSortColumn,
    pub sort_direction: TransferBrowserSortDirection,
    pub column_widths: TransferBrowserColumnWidths,
    pub history: VecDeque<RemoteFilePath>,
    pub history_index: usize,
    pub selected_path: Option<String>,
    pub selected_paths: HashSet<String>,
    pub list_scroll: UniformListScrollHandle,
    pub horizontal_scroll: ScrollHandle,
    pub focus: FocusHandle,
    pub generation: u64,
    pub selection_anchor: Option<String>,
    pub path_editing: bool,
    pub row_height: u32,
    pub visible_columns: Vec<String>,
    pub last_successful_path: Option<RemoteFilePath>,
    pub source_history: HashMap<PaneEndpoint, PaneNavigationSnapshot>,
    pub remembered_directories: Vec<nyaterm_core::FileManagerPaneRestore>,
    pub history_target: Option<usize>,
    select_after_load: Option<String>,
    filter: BrowserFilterCache,
    visibility_filter: Option<(String, bool)>,
}

impl BrowserPaneState {
    pub(in crate::features) fn new(endpoint: PaneEndpoint, focus: FocusHandle) -> Self {
        Self {
            endpoint,
            path: RemoteFilePath::new(String::new()),
            home_dir: String::new(),
            home_dir_pending: true,
            entries: Arc::new(Vec::new()),
            loading: false,
            error: None,
            status: String::new(),
            disconnected: false,
            search: String::new(),
            sort_column: TransferBrowserSortColumn::Name,
            sort_direction: TransferBrowserSortDirection::Ascending,
            column_widths: TransferBrowserColumnWidths {
                name: gpui::px(140.),
                size: gpui::px(88.),
                modified: gpui::px(150.),
                permissions: gpui::px(92.),
                ..Default::default()
            },
            history: VecDeque::new(),
            history_index: 0,
            selected_path: None,
            selected_paths: HashSet::new(),
            list_scroll: UniformListScrollHandle::new(),
            horizontal_scroll: ScrollHandle::new(),
            focus,
            generation: 0,
            selection_anchor: None,
            path_editing: false,
            row_height: 30,
            visible_columns: ["name", "size", "modified", "permissions"]
                .map(str::to_string)
                .to_vec(),
            last_successful_path: None,
            source_history: HashMap::new(),
            remembered_directories: Vec::new(),
            history_target: None,
            select_after_load: None,
            filter: BrowserFilterCache::default(),
            visibility_filter: None,
        }
    }

    pub(in crate::features) fn visible_entries(
        &mut self,
        show_hidden: bool,
    ) -> Arc<[SftpFileEntry]> {
        let visibility = (self.search.clone(), show_hidden);
        if self
            .visibility_filter
            .as_ref()
            .is_some_and(|previous| previous != &visibility)
        {
            self.clear_selection();
        }
        self.visibility_filter = Some(visibility);
        let key = BrowserFilterKey::new(
            &self.entries,
            &self.search,
            show_hidden,
            self.sort_column,
            self.sort_direction,
        );
        self.filter.entries(key, &self.entries)
    }

    pub(in crate::features) fn capture_navigation(&self) -> PaneNavigationSnapshot {
        PaneNavigationSnapshot {
            path: self.path.clone(),
            home_dir: self.home_dir.clone(),
            entries: self.entries.clone(),
            loading: self.loading,
            error: self.error.clone(),
            status: self.status.clone(),
            history: self.history.clone(),
            history_index: self.history_index,
            selected_path: self.selected_path.clone(),
            selected_paths: self.selected_paths.clone(),
        }
    }

    pub(in crate::features) fn restore_navigation(&mut self, snapshot: PaneNavigationSnapshot) {
        self.path = snapshot.path;
        self.home_dir = snapshot.home_dir;
        self.entries = snapshot.entries;
        self.loading = snapshot.loading;
        self.error = snapshot.error;
        self.status = snapshot.status;
        self.history = snapshot.history;
        self.history_index = snapshot
            .history_index
            .min(self.history.len().saturating_sub(1));
        self.selected_path = snapshot.selected_path;
        self.selected_paths = snapshot.selected_paths;
        self.select_after_load = None;
    }

    pub(in crate::features) fn begin_load(&mut self, path: RemoteFilePath) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        // All navigation entry points share this intent, including breadcrumbs and history.
        self.select_after_load = (self.endpoint.kind() == FileBrowserBackendKind::Remote
            && self.path != path
            && self.path.parent().is_ok_and(|parent| parent == path))
        .then(|| self.path.identity_key());
        if self.path != path {
            self.clear_selection();
        }
        self.path = path;
        self.loading = true;
        self.error = None;
        self.status = "Loading directory...".to_string();
        self.list_scroll = UniformListScrollHandle::new();
        self.horizontal_scroll = ScrollHandle::new();
        self.generation
    }

    pub(in crate::features) fn apply_listing(
        &mut self,
        generation: u64,
        endpoint: &PaneEndpoint,
        path: &RemoteFilePath,
        entries: Vec<SftpFileEntry>,
        show_hidden: bool,
    ) -> bool {
        if self.generation != generation || &self.endpoint != endpoint {
            return false;
        }
        if self.path.display_path != path.display_path
            || self.path.raw_path_token != path.raw_path_token
        {
            return false;
        }
        self.entries = Arc::new(entries);
        self.selected_paths
            .retain(|key| self.entries.iter().any(|entry| entry.matches_identity(key)));
        if !self
            .selected_path
            .as_ref()
            .is_some_and(|key| self.entries.iter().any(|entry| entry.matches_identity(key)))
        {
            self.selected_path = None;
        }
        if let Some(identity) = self.select_after_load.take() {
            let visible = self.visible_entries(show_hidden);
            if let Some(index) = visible
                .iter()
                .position(|entry| entry.matches_identity(&identity))
            {
                self.select_entry(visible[index].identity_key());
                // The first list row is the parent-directory entry (`..`).
                self.list_scroll
                    .scroll_to_item(index + 1, gpui::ScrollStrategy::Nearest);
            }
        }
        self.loading = false;
        self.error = None;
        self.disconnected = false;
        self.status = format!("{} item(s)", self.entries.len());
        self.last_successful_path = Some(self.path.clone());
        if let Some(index) = self.history_target.take() {
            self.history_index = index.min(self.history.len().saturating_sub(1));
        } else {
            self.record_history(self.path.clone());
        }
        true
    }

    pub(in crate::features) fn apply_error(
        &mut self,
        generation: u64,
        endpoint: &PaneEndpoint,
        snapshot: PaneNavigationSnapshot,
        error: String,
    ) -> bool {
        if self.generation != generation || &self.endpoint != endpoint {
            return false;
        }
        self.history_target = None;
        self.restore_navigation(snapshot);
        self.loading = false;
        self.status = if matches!(endpoint, PaneEndpoint::Adb(_)) {
            rust_i18n::t!("android.directoryLoadFailed", error = error).into_owned()
        } else {
            format!("directory load failed: {error}")
        };
        self.error = Some(error);
        true
    }

    pub(in crate::features) fn set_endpoint(&mut self, endpoint: PaneEndpoint) {
        if self.endpoint == endpoint {
            return;
        }
        let mut snapshot = self.capture_navigation();
        snapshot.loading = false;
        snapshot.selected_path = None;
        snapshot.selected_paths.clear();
        if let Some(path) = self.last_successful_path.clone() {
            snapshot.path = path;
        }
        if !snapshot.path.display_path.is_empty() && self.last_successful_path.is_some() {
            self.source_history.insert(self.endpoint.clone(), snapshot);
        }
        self.endpoint = endpoint;
        self.last_successful_path = None;
        self.generation = self.generation.wrapping_add(1);
        self.home_dir.clear();
        self.home_dir_pending = true;
        self.path = RemoteFilePath::new(String::new());
        self.entries = Arc::new(Vec::new());
        self.history.clear();
        self.history_index = 0;
        self.selected_path = None;
        self.selected_paths.clear();
        self.select_after_load = None;
        self.error = None;
        self.disconnected = false;
        self.search.clear();
        self.path_editing = false;
        self.selection_anchor = None;
        self.history_target = None;
        self.loading = false;
        if let Some(snapshot) = self.source_history.remove(&self.endpoint) {
            self.restore_navigation(snapshot);
            self.last_successful_path = Some(self.path.clone());
            self.home_dir_pending = self.home_dir.is_empty();
        }
    }

    pub(in crate::features) fn replace_session_id(&mut self, old_id: &str, new_id: &str) -> bool {
        if let Some(snapshot) = self
            .source_history
            .remove(&PaneEndpoint::Session(old_id.into()))
        {
            self.source_history
                .insert(PaneEndpoint::Session(new_id.into()), snapshot);
        }
        if self.endpoint.session_id() != Some(old_id) {
            return false;
        }
        self.endpoint = PaneEndpoint::Session(new_id.to_string());
        self.generation = self.generation.wrapping_add(1);
        true
    }

    pub(in crate::features) fn select_entry(&mut self, path: String) {
        self.selection_anchor = Some(path.clone());
        self.selected_path = Some(path.clone());
        self.selected_paths.clear();
        self.selected_paths.insert(path);
    }

    pub(in crate::features) fn toggle_mark(&mut self, path: String) {
        if !self.selected_paths.remove(&path) {
            self.selected_paths.insert(path.clone());
        }
        self.selection_anchor = Some(path.clone());
        self.selected_path = Some(path);
    }

    pub(in crate::features) fn selected_entries(&self) -> Vec<SftpFileEntry> {
        self.entries
            .iter()
            .filter(|entry| self.selected_paths.contains(&entry.identity_key()))
            .cloned()
            .collect()
    }

    pub(in crate::features) fn clear_selection(&mut self) {
        self.selected_path = None;
        self.selection_anchor = None;
        self.selected_paths.clear();
    }

    pub(in crate::features) fn select_visible(
        &mut self,
        identity: String,
        additive: bool,
        range: bool,
        visible: &[SftpFileEntry],
    ) {
        if range {
            let anchor = self
                .selection_anchor
                .as_deref()
                .and_then(|key| visible.iter().position(|e| e.matches_identity(key)));
            let end = visible.iter().position(|e| e.matches_identity(&identity));
            if let (Some(start), Some(end)) = (anchor, end) {
                if !additive {
                    self.selected_paths.clear();
                }
                self.selected_paths.extend(
                    visible[start.min(end)..=start.max(end)]
                        .iter()
                        .map(SftpFileEntry::identity_key),
                );
                self.selected_path = Some(identity);
                return;
            }
        }
        if additive {
            self.toggle_mark(identity);
        } else {
            self.select_entry(identity);
        }
    }

    pub(in crate::features) fn select_all(&mut self, visible: &[SftpFileEntry]) {
        self.selected_paths = visible.iter().map(SftpFileEntry::identity_key).collect();
        self.selected_path = visible.first().map(SftpFileEntry::identity_key);
        self.selection_anchor = self.selected_path.clone();
    }

    pub(in crate::features) fn drag_entries(&self, entry: &SftpFileEntry) -> Vec<SftpFileEntry> {
        if self.selected_paths.contains(&entry.identity_key()) {
            self.selected_entries()
        } else {
            vec![entry.clone()]
        }
    }

    pub(in crate::features) fn history_path(&mut self, delta: isize) -> Option<RemoteFilePath> {
        let index = self.history_index.checked_add_signed(delta)?;
        let path = self.history.get(index)?.clone();
        self.history_target = Some(index);
        Some(path)
    }

    fn record_history(&mut self, path: RemoteFilePath) {
        if self
            .history
            .get(self.history_index)
            .is_some_and(|entry| entry.identity_key() == path.identity_key())
        {
            return;
        }
        if !self.history.is_empty() {
            self.history.truncate(self.history_index + 1);
        }
        self.history.push_back(path);
        while self.history.len() > 128 {
            self.history.pop_front();
        }
        self.history_index = self.history.len().saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{BrowserPaneState, PaneEndpoint};
    use gpui::TestAppContext;
    use nyaterm_files::sftp::{RemoteFilePath, SftpFileEntry, SftpFileType};

    fn entry(name: &str, path: &str) -> SftpFileEntry {
        SftpFileEntry {
            name: name.to_string(),
            path: path.to_string(),
            file_type: SftpFileType::File,
            size: Some(1),
            permissions: None,
            owner: String::new(),
            group: String::new(),
            modified_at: None,
            raw_path_token: None,
            symlink_target_is_directory: false,
        }
    }

    #[test]
    fn panes_keep_independent_search_sort_and_selection() {
        let cx = TestAppContext::single();
        cx.update(|cx| {
            let mut left = BrowserPaneState::new(PaneEndpoint::Local, cx.focus_handle());
            let mut right =
                BrowserPaneState::new(PaneEndpoint::Session("ssh-a".into()), cx.focus_handle());
            left.entries = std::sync::Arc::new(vec![entry("alpha", "/a"), entry("beta", "/b")]);
            right.entries = std::sync::Arc::new(vec![entry("gamma", "/g"), entry("delta", "/d")]);
            left.select_entry("/a".into());
            left.search = "al".into();
            right.select_entry("/g".into());
            right.search = "de".into();
            let left_visible = left.visible_entries(true);
            let right_visible = right.visible_entries(true);
            assert_eq!(left_visible.len(), 1);
            assert_eq!(left_visible[0].name, "alpha");
            assert_eq!(right.selected_path.as_deref(), Some("/g"));
            assert_eq!(left.selected_path.as_deref(), Some("/a"));
            assert_eq!(right_visible.len(), 1);
            assert_eq!(right_visible[0].name, "delta");
        });
    }

    #[test]
    fn stale_generation_or_endpoint_replies_are_discarded() {
        let cx = TestAppContext::single();
        cx.update(|cx| {
            let mut pane = BrowserPaneState::new(PaneEndpoint::Local, cx.focus_handle());
            let generation = pane.begin_load(RemoteFilePath::new("/one"));
            pane.endpoint = PaneEndpoint::Session("ssh-b".into());
            assert!(!pane.apply_listing(
                generation,
                &PaneEndpoint::Local,
                &RemoteFilePath::new("/one"),
                vec![entry("stale", "/one/stale")],
                true,
            ));
            let generation = pane.begin_load(RemoteFilePath::new("/two"));
            assert!(!pane.apply_listing(
                generation.wrapping_sub(1),
                &PaneEndpoint::Session("ssh-b".into()),
                &RemoteFilePath::new("/two"),
                vec![entry("old", "/two/old")],
                true,
            ));
            assert!(pane.apply_listing(
                generation,
                &PaneEndpoint::Session("ssh-b".into()),
                &RemoteFilePath::new("/two"),
                vec![entry("fresh", "/two/fresh")],
                true,
            ));
            assert_eq!(pane.entries.len(), 1);
            assert_eq!(pane.entries[0].name, "fresh");
        });
    }

    #[test]
    fn parent_listing_selects_the_directory_that_was_opened() {
        let cx = TestAppContext::single();
        cx.update(|cx| {
            let endpoint = PaneEndpoint::Session("sftp".into());
            let mut pane = BrowserPaneState::new(endpoint.clone(), cx.focus_handle());
            let child = RemoteFilePath::new("/parent/child");
            let parent = RemoteFilePath::new("/parent");
            pane.path = child.clone();
            let generation = pane.begin_load(parent.clone());

            assert!(pane.apply_listing(
                generation,
                &endpoint,
                &parent,
                vec![SftpFileEntry {
                    name: "child".into(),
                    path: child.display_path.clone(),
                    file_type: SftpFileType::Directory,
                    size: None,
                    permissions: None,
                    owner: String::new(),
                    group: String::new(),
                    modified_at: None,
                    raw_path_token: None,
                    symlink_target_is_directory: false,
                }],
                true,
            ));

            assert_eq!(pane.selected_path.as_deref(), Some("/parent/child"));
            assert_eq!(pane.selected_paths.len(), 1);
            assert!(pane.selected_paths.contains("/parent/child"));
            assert!(pane.select_after_load.is_none());
            assert_eq!(
                pane.list_scroll
                    .0
                    .borrow()
                    .deferred_scroll_to_item
                    .unwrap()
                    .item_index,
                1
            );
        });
    }

    #[test]
    fn parent_listing_does_not_select_filtered_or_missing_children() {
        let cx = TestAppContext::single();
        cx.update(|cx| {
            for (name, search, show_hidden, present) in [
                ("child", "README", true, true),
                (".child", "", false, true),
                ("child", "", true, false),
            ] {
                let endpoint = PaneEndpoint::Session("sftp".into());
                let mut pane = BrowserPaneState::new(endpoint.clone(), cx.focus_handle());
                let child = format!("/parent/{name}");
                pane.path = RemoteFilePath::new(&child);
                // A filter change while loading must be applied before restoring selection.
                pane.visible_entries(true);
                let parent = RemoteFilePath::new("/parent");
                let generation = pane.begin_load(parent.clone());
                pane.search = search.into();
                let mut entries = vec![entry("README", "/parent/README")];
                if present {
                    let mut directory = entry(name, &child);
                    directory.file_type = SftpFileType::Directory;
                    entries.push(directory);
                }
                assert!(pane.apply_listing(generation, &endpoint, &parent, entries, show_hidden));
                assert!(pane.selected_path.is_none());
                assert!(pane.selected_paths.is_empty());
                assert!(pane.selection_anchor.is_none());
                assert!(pane.selected_entries().is_empty());
                assert!(pane.select_after_load.is_none());
                assert!(
                    pane.list_scroll
                        .0
                        .borrow()
                        .deferred_scroll_to_item
                        .is_none()
                );
            }
        });
    }

    #[test]
    fn parent_listing_scrolls_to_sorted_visible_row_and_preserves_raw_identity() {
        let cx = TestAppContext::single();
        cx.update(|cx| {
            for descending in [false, true] {
                let endpoint = PaneEndpoint::Session("sftp".into());
                let mut pane = BrowserPaneState::new(endpoint.clone(), cx.focus_handle());
                let child = RemoteFilePath::from_raw("/parent/item1500", b"/parent/item1500\xff");
                let parent = child.parent().unwrap();
                pane.path = child.clone();
                pane.visible_entries(true);
                let generation = pane.begin_load(parent.clone());
                pane.search = " ITEM ".into();
                pane.sort_direction = if descending {
                    crate::models::TransferBrowserSortDirection::Descending
                } else {
                    crate::models::TransferBrowserSortDirection::Ascending
                };
                let mut entries = (0..2000)
                    .rev()
                    .map(|index| {
                        let name = format!("item{index}");
                        let mut directory = entry(&name, &format!("/parent/{name}"));
                        directory.file_type = SftpFileType::Directory;
                        if index == 1500 {
                            directory.raw_path_token = child.raw_path_token.clone();
                        }
                        directory
                    })
                    .collect::<Vec<_>>();
                // These sort before the target but must not count toward its visible row.
                for name in [".item", "another"] {
                    let mut directory = entry(name, &format!("/parent/{name}"));
                    directory.file_type = SftpFileType::Directory;
                    entries.push(directory);
                }
                assert!(pane.apply_listing(generation, &endpoint, &parent, entries, false));
                assert_eq!(pane.selected_path, Some(child.identity_key()));
                assert_eq!(pane.selected_paths.len(), 1);
                assert_eq!(pane.selection_anchor, Some(child.identity_key()));
                let scroll = pane.list_scroll.0.borrow().deferred_scroll_to_item.unwrap();
                assert_eq!(scroll.item_index, if descending { 500 } else { 1501 });
                assert_eq!(scroll.strategy, gpui::ScrollStrategy::Nearest);
                let visible = pane.visible_entries(false);
                assert!(visible[scroll.item_index - 1].matches_identity(&child.identity_key()));
                assert_eq!(pane.selected_path, Some(child.identity_key()));
            }
        });
    }

    #[test]
    fn parent_selection_intent_is_replaced_by_new_navigation_and_cleared_on_failure() {
        let cx = TestAppContext::single();
        cx.update(|cx| {
            let endpoint = PaneEndpoint::Session("sftp".into());
            let mut pane = BrowserPaneState::new(endpoint.clone(), cx.focus_handle());
            pane.path = RemoteFilePath::new("/parent/child");
            pane.select_entry("/parent/child/file".into());
            let snapshot = pane.capture_navigation();
            let parent = RemoteFilePath::new("/parent");
            let stale = pane.begin_load(parent.clone());
            assert_eq!(pane.select_after_load.as_deref(), Some("/parent/child"));
            let other = RemoteFilePath::new("/other");
            let generation = pane.begin_load(other.clone());
            assert!(pane.select_after_load.is_none());
            assert!(!pane.apply_listing(stale, &endpoint, &parent, vec![], true));
            assert!(pane.apply_listing(generation, &endpoint, &other, vec![], true));
            assert!(pane.selected_paths.is_empty());
            assert!(
                pane.list_scroll
                    .0
                    .borrow()
                    .deferred_scroll_to_item
                    .is_none()
            );

            pane.restore_navigation(snapshot.clone());
            let generation = pane.begin_load(parent);
            assert!(pane.apply_error(generation, &endpoint, snapshot, "denied".into()));
            assert_eq!(pane.path.display_path, "/parent/child");
            assert_eq!(pane.selected_path.as_deref(), Some("/parent/child/file"));
            assert!(pane.select_after_load.is_none());
        });
    }

    #[test]
    fn refresh_root_and_local_navigation_do_not_request_parent_selection() {
        let cx = TestAppContext::single();
        cx.update(|cx| {
            for (endpoint, current, target) in [
                (PaneEndpoint::Local, "/parent/child", "/parent"),
                (PaneEndpoint::Session("sftp".into()), "/", "/"),
                (PaneEndpoint::Session("sftp".into()), "/parent", "/parent"),
                (PaneEndpoint::Session("sftp".into()), "/parent/child", "/"),
            ] {
                let mut pane = BrowserPaneState::new(endpoint, cx.focus_handle());
                pane.path = RemoteFilePath::new(current);
                pane.begin_load(RemoteFilePath::new(target));
                assert!(pane.select_after_load.is_none());
            }
        });
    }

    #[test]
    fn swapping_panes_exchanges_endpoint_search_and_selection() {
        let cx = TestAppContext::single();
        cx.update(|cx| {
            let mut left = BrowserPaneState::new(PaneEndpoint::Local, cx.focus_handle());
            let mut right =
                BrowserPaneState::new(PaneEndpoint::Session("ssh-a".into()), cx.focus_handle());
            left.select_entry("/a".into());
            left.search = "alpha".into();
            right.select_entry("/g".into());
            right.search = "gamma".into();
            std::mem::swap(&mut left, &mut right);
            assert_eq!(left.endpoint, PaneEndpoint::Session("ssh-a".into()));
            assert_eq!(right.endpoint, PaneEndpoint::Local);
            assert_eq!(left.selected_path.as_deref(), Some("/g"));
            assert_eq!(right.selected_path.as_deref(), Some("/a"));
            assert_eq!(left.search, "gamma");
            assert_eq!(right.search, "alpha");
        });
    }
    #[test]
    fn sorted_range_and_frozen_drag_keep_the_visible_selection() {
        let cx = TestAppContext::single();
        cx.update(|cx| {
            let mut pane = BrowserPaneState::new(PaneEndpoint::Local, cx.focus_handle());
            pane.entries = std::sync::Arc::new(vec![
                entry("item10", "/10"),
                entry("item2", "/2"),
                entry("item1", "/1"),
                entry("hidden", "/h"),
            ]);
            pane.search = "item".into();
            let visible = pane.visible_entries(true);
            assert_eq!(
                visible
                    .iter()
                    .map(|entry| entry.name.as_str())
                    .collect::<Vec<_>>(),
                ["item1", "item2", "item10"]
            );
            pane.select_visible("/1".into(), false, false, &visible);
            pane.select_visible("/10".into(), false, true, &visible);
            assert_eq!(pane.selected_paths.len(), 3);
            let frozen = pane.drag_entries(&visible[1]);
            pane.select_entry("/h".into());
            assert_eq!(frozen.len(), 3);
            assert!(frozen.iter().all(|entry| entry.name.starts_with("item")));
            assert_eq!(pane.drag_entries(&visible[0]).len(), 1);
        });
    }

    #[test]
    fn refresh_does_not_resurrect_cleared_selection_and_error_keeps_listing() {
        let cx = TestAppContext::single();
        cx.update(|cx| {
            let mut pane = BrowserPaneState::new(PaneEndpoint::Local, cx.focus_handle());
            let path = RemoteFilePath::new("/directory");
            let generation = pane.begin_load(path.clone());
            pane.apply_listing(
                generation,
                &PaneEndpoint::Local,
                &path,
                vec![entry("file", "/directory/file")],
                true,
            );
            pane.select_entry("/directory/file".into());
            let snapshot = pane.capture_navigation();
            let generation = pane.begin_load(path.clone());
            pane.clear_selection();
            pane.apply_listing(
                generation,
                &PaneEndpoint::Local,
                &path,
                vec![entry("file", "/directory/file")],
                true,
            );
            assert!(pane.selected_paths.is_empty());
            let generation = pane.begin_load(path);
            pane.apply_error(
                generation,
                &PaneEndpoint::Local,
                snapshot,
                "permission denied".into(),
            );
            assert_eq!(pane.entries.len(), 1);
            assert_eq!(pane.error.as_deref(), Some("permission denied"));
        });
    }

    #[test]
    fn each_source_restores_its_raw_path_and_history_without_selection() {
        let cx = TestAppContext::single();
        cx.update(|cx| {
            let endpoint = PaneEndpoint::Session("ssh".into());
            let mut pane = BrowserPaneState::new(endpoint.clone(), cx.focus_handle());
            let one = RemoteFilePath::from_raw("/lossy/one", b"/\xff/one");
            let two = RemoteFilePath::from_raw("/lossy/two", b"/\xff/two");
            for path in [&one, &two] {
                let generation = pane.begin_load(path.clone());
                pane.apply_listing(generation, &endpoint, path, vec![], true);
            }
            pane.select_entry("old".into());
            pane.set_endpoint(PaneEndpoint::Local);
            let stale = pane.generation;
            pane.set_endpoint(endpoint.clone());
            assert_eq!(pane.path, two);
            assert!(pane.selected_paths.is_empty());
            assert_eq!(pane.history_path(-1), Some(one.clone()));
            let generation = pane.begin_load(one.clone());
            pane.apply_listing(generation, &endpoint, &one, vec![], true);
            assert_eq!(pane.history.len(), 2);
            assert_eq!(pane.history_index, 0);
            assert!(!pane.apply_listing(stale, &endpoint, &one, vec![], true));
        });
    }
}
