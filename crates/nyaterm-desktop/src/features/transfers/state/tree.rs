//! Session-local directory caches. No listing request is issued by presentation.

use gpui::{FocusHandle, ScrollStrategy, UniformListScrollHandle};
use nyaterm_files::file_browser::{
    FileBrowserBackendKind, file_browser_identity, file_browser_parent, file_browser_path_is_root,
    file_browser_root,
};
use nyaterm_files::sftp::{RemoteFilePath, SftpFileEntry, SftpFileType};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

mod navigation;
mod requests;

use navigation::TreeNavigationCheckpoint;
use requests::TreeListingRequest;

#[derive(Clone)]
pub(in crate::features) struct TransferTreeRow {
    pub key: String,
    pub path: RemoteFilePath,
    pub label: String,
    pub depth: usize,
    pub directory: bool,
    pub expandable: bool,
    pub root: bool,
    pub expanded: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub entry: Option<SftpFileEntry>,
}

#[derive(Clone)]
pub(in crate::features) struct TransferTreePresentation {
    pub rows: Arc<[TransferTreeRow]>,
    pub selected: Arc<HashSet<String>>,
    pub focused: Option<String>,
    pub scroll: UniformListScrollHandle,
}

struct DirectoryNode {
    path: RemoteFilePath,
    entries: Option<Arc<Vec<SftpFileEntry>>>,
    pending: Option<u64>,
    error: Option<String>,
}

impl DirectoryNode {
    fn new(path: RemoteFilePath) -> Self {
        Self {
            path,
            entries: None,
            pending: None,
            error: None,
        }
    }
}

#[derive(Default)]
struct TreeSession {
    backend: Option<FileBrowserBackendKind>,
    root: Option<RemoteFilePath>,
    nodes: HashMap<String, DirectoryNode>,
    expanded: HashSet<String>,
    selected: HashSet<String>,
    anchor: Option<String>,
    focused: Option<String>,
    scroll: UniformListScrollHandle,
    scroll_to_selection: bool,
    rows: Arc<[TransferTreeRow]>,
    dirty: bool,
    show_hidden: bool,
    interaction_version: u64,
    reveal_chain: Vec<String>,
    manual_requests: VecDeque<String>,
}

#[derive(Default)]
pub(super) struct TransferTreeState {
    sessions: HashMap<String, TreeSession>,
    next_generation: u64,
    navigations: HashMap<String, TreeNavigationCheckpoint>,
    // Keep the slot until its terminal event, even when cancellation has been requested.
    listing: Option<TreeListingRequest>,
}

// Valid UTF-8 tokens and display-only paths denote the same directory; invalid
// UTF-8 paths must retain their byte identity even when their labels collide.
fn key(backend: FileBrowserBackendKind, path: &RemoteFilePath) -> String {
    match (backend, path.raw_path()) {
        (FileBrowserBackendKind::Remote | FileBrowserBackendKind::Adb, Ok(Some(raw)))
            if std::str::from_utf8(&raw).ok() == Some(path.display_path.as_str()) =>
        {
            path.display_path.clone()
        }
        (FileBrowserBackendKind::Remote | FileBrowserBackendKind::Adb, _) => path.identity_key(),
        (FileBrowserBackendKind::Local, _) => file_browser_identity(backend, &path.display_path),
    }
}

impl TransferTreeState {
    pub(super) fn remove_session(&mut self, session: &str) {
        self.cancel_session_requests(session);
        self.navigations.remove(session);
        self.sessions.remove(session);
    }

    pub(super) fn replace_session(&mut self, old: &str, new: &str) {
        self.cancel_session_requests(old);
        self.navigations.remove(old);
        if let Some(mut state) = self.sessions.remove(old) {
            // Cached listings survive reconnect; requests belong to the old connection.
            for node in state.nodes.values_mut() {
                node.pending = None;
            }
            state.dirty = true;
            self.sessions.insert(new.to_string(), state);
        }
    }

    pub(super) fn presentation(
        &mut self,
        session: Option<&str>,
        show_hidden: bool,
    ) -> TransferTreePresentation {
        let state = self
            .sessions
            .entry(session.unwrap_or_default().to_string())
            .or_default();
        if state.dirty || state.show_hidden != show_hidden {
            state.show_hidden = show_hidden;
            let mut rows = Vec::new();
            if let Some(root) = &state.root {
                state.append_rows(root, None, 0, &mut HashSet::new(), &mut rows);
            }
            state.rows = rows.into();
            let visible: HashSet<&str> = state.rows.iter().map(|row| row.key.as_str()).collect();
            state.selected.retain(|id| visible.contains(id.as_str()));
            // Missing ancestor listings can hide the reveal target between replies.
            if !state.scroll_to_selection
                && state
                    .focused
                    .as_deref()
                    .is_some_and(|id| !visible.contains(id))
            {
                state.focused = None;
            }
            if !state.scroll_to_selection
                && state
                    .anchor
                    .as_deref()
                    .is_some_and(|id| !visible.contains(id))
            {
                state.anchor = None;
            }
            state.dirty = false;
            if state.scroll_to_selection
                && let Some(index) = state
                    .focused
                    .as_ref()
                    .and_then(|selected| state.rows.iter().position(|row| &row.key == selected))
            {
                state.scroll.scroll_to_item(index, ScrollStrategy::Center);
                state.scroll_to_selection = false;
            }
        }
        TransferTreePresentation {
            rows: state.rows.clone(),
            selected: Arc::new(state.selected.clone()),
            focused: state.focused.clone(),
            scroll: state.scroll.clone(),
        }
    }

    pub(super) fn seed(
        &mut self,
        session: &str,
        backend: FileBrowserBackendKind,
        path: RemoteFilePath,
        entries: Arc<Vec<SftpFileEntry>>,
    ) {
        let state = self.sessions.entry(session.to_string()).or_default();
        state.ensure_backend(backend);
        state.apply_listing(path, entries);
    }

    /// Reveal also returns the missing ancestor listings, for the runtime adapter.
    pub(super) fn reveal(
        &mut self,
        session: &str,
        backend: FileBrowserBackendKind,
        path: RemoteFilePath,
    ) -> Vec<RemoteFilePath> {
        let state = self.sessions.entry(session.to_string()).or_default();
        state.ensure_backend(backend);
        let root = RemoteFilePath::new(match backend {
            FileBrowserBackendKind::Remote | FileBrowserBackendKind::Adb => "/".to_string(),
            FileBrowserBackendKind::Local => file_browser_root(backend, &path.display_path),
        });
        let path =
            if backend == FileBrowserBackendKind::Remote && !path.display_path.starts_with('/') {
                root.clone()
            } else {
                path
            };
        if state
            .root
            .as_ref()
            .is_some_and(|current| key(backend, current) != key(backend, &root))
        {
            // Drive/UNC roots share a backend. Retain caches for navigation rollback.
            state.selected.clear();
            state.anchor = None;
        }
        let mut chain = Vec::new();
        let mut current = path.clone();
        let mut seen = HashSet::new();
        for _ in 0..64 {
            if !seen.insert(key(backend, &current)) {
                break;
            }
            chain.push(current.clone());
            if file_browser_path_is_root(backend, &current.display_path) {
                break;
            }
            let parent = match backend {
                FileBrowserBackendKind::Remote | FileBrowserBackendKind::Adb => {
                    match current.parent() {
                        Ok(parent) => parent,
                        Err(_) => break,
                    }
                }
                FileBrowserBackendKind::Local => {
                    RemoteFilePath::new(file_browser_parent(backend, &current.display_path))
                }
            };
            if key(backend, &parent) == key(backend, &current) {
                break;
            }
            current = parent;
        }
        state.root = Some(root);
        let selected = key(backend, &path);
        state.focused = Some(selected.clone());
        state.anchor = Some(selected);
        state.scroll_to_selection = true;
        state.reveal_chain.clear();
        let mut missing = Vec::new();
        for path in chain.into_iter().rev() {
            let id = key(backend, &path);
            state.expanded.insert(id.clone());
            state.reveal_chain.push(id.clone());
            let node = state
                .nodes
                .entry(id)
                .or_insert_with(|| DirectoryNode::new(path));
            if node.entries.is_none() && node.pending.is_none() {
                node.error = None;
                missing.push(node.path.clone());
            }
        }
        state.dirty = true;
        self.cancel_obsolete_listing();
        missing
    }

    pub(super) fn begin_request(
        &mut self,
        session: &str,
        backend: FileBrowserBackendKind,
        path: RemoteFilePath,
    ) -> Option<u64> {
        let state = self.sessions.entry(session.to_string()).or_default();
        state.ensure_backend(backend);
        let node = state
            .nodes
            .entry(key(backend, &path))
            .or_insert_with(|| DirectoryNode::new(path));
        if node.pending.is_some() || node.entries.is_some() {
            return None;
        }
        self.next_generation += 1;
        node.pending = Some(self.next_generation);
        node.error = None;
        state.dirty = true;
        Some(self.next_generation)
    }

    pub(super) fn complete(
        &mut self,
        session: &str,
        path: &RemoteFilePath,
        generation: u64,
        result: Result<Vec<SftpFileEntry>, String>,
    ) -> bool {
        let Some(state) = self.sessions.get_mut(session) else {
            return false;
        };
        let Some(backend) = state.backend else {
            return false;
        };
        let Some(node) = state.nodes.get_mut(&key(backend, path)) else {
            return false;
        };
        if node.pending != Some(generation) {
            return false;
        }
        node.pending = None;
        match result {
            Ok(entries) => state.apply_listing(path.clone(), Arc::new(entries)),
            Err(error) => {
                node.error = Some(error);
                if state.reveal_chain.contains(&key(backend, path)) {
                    state.scroll_to_selection = false;
                }
                state.dirty = true;
            }
        }
        true
    }

    pub(super) fn invalidate(&mut self, session: &str, path: &RemoteFilePath, subtree: bool) {
        let Some(state) = self.sessions.get_mut(session) else {
            return;
        };
        let Some(backend) = state.backend else {
            return;
        };
        let id = key(backend, path);
        if subtree {
            state.remove_subtree(&id, &mut HashSet::new());
        } else if let Some(node) = state.nodes.get_mut(&id) {
            node.entries = None;
            node.pending = None;
            node.error = None;
        }
        state.dirty = true;
        self.cancel_obsolete_listing();
    }

    pub(super) fn select(&mut self, session: &str, id: String, additive: bool, range: bool) {
        let Some(state) = self.sessions.get_mut(session) else {
            return;
        };
        let Some(target) = state.rows.iter().position(|row| row.key == id) else {
            return;
        };
        state.interaction_version = state.interaction_version.wrapping_add(1);
        state.scroll_to_selection = false;
        state.scroll.0.borrow_mut().deferred_scroll_to_item = None;
        state.focused = Some(id.clone());
        if state.rows[target].root {
            if !additive && !range {
                state.selected.clear();
            }
            return;
        }
        if range {
            let anchor = state
                .anchor
                .as_ref()
                .and_then(|anchor| state.rows.iter().position(|row| &row.key == anchor))
                .unwrap_or(target);
            if !additive {
                state.selected.clear();
            }
            for row in &state.rows[anchor.min(target)..=anchor.max(target)] {
                if !row.root {
                    state.selected.insert(row.key.clone());
                }
            }
        } else if additive {
            if !state.selected.remove(&id) {
                state.selected.insert(id.clone());
            }
            state.anchor = Some(id);
        } else {
            state.selected.clear();
            state.selected.insert(id.clone());
            state.anchor = Some(id);
        }
    }

    pub(super) fn select_all(&mut self, session: &str) {
        let Some(state) = self.sessions.get_mut(session) else {
            return;
        };
        state.interaction_version = state.interaction_version.wrapping_add(1);
        state.scroll_to_selection = false;
        state.scroll.0.borrow_mut().deferred_scroll_to_item = None;
        state.selected = state
            .rows
            .iter()
            .filter(|row| !row.root)
            .map(|row| row.key.clone())
            .collect();
        state.focused = state
            .rows
            .iter()
            .find(|row| !row.root)
            .map(|row| row.key.clone());
        state.anchor = state.focused.clone();
    }

    pub(super) fn select_context(&mut self, session: &str, id: String) {
        let Some(state) = self.sessions.get_mut(session) else {
            return;
        };
        let Some(row) = state.rows.iter().find(|row| row.key == id) else {
            return;
        };
        state.interaction_version = state.interaction_version.wrapping_add(1);
        state.scroll_to_selection = false;
        state.scroll.0.borrow_mut().deferred_scroll_to_item = None;
        state.focused = Some(id.clone());
        if row.root {
            state.selected.clear();
        } else if !state.selected.contains(&id) {
            state.selected.clear();
            state.selected.insert(id.clone());
            state.anchor = Some(id);
        }
    }

    pub(super) fn clear_selection(&mut self, session: &str) {
        if let Some(state) = self.sessions.get_mut(session) {
            state.interaction_version = state.interaction_version.wrapping_add(1);
            state.scroll_to_selection = false;
            state.scroll.0.borrow_mut().deferred_scroll_to_item = None;
            state.selected.clear();
            state.anchor = None;
        }
    }

    pub(super) fn toggle(
        &mut self,
        session: &str,
        id: &str,
        expand: Option<bool>,
    ) -> Option<RemoteFilePath> {
        let state = self.sessions.get_mut(session)?;
        let node = state.nodes.get_mut(id)?;
        state.interaction_version = state.interaction_version.wrapping_add(1);
        state.scroll_to_selection = false;
        state.scroll.0.borrow_mut().deferred_scroll_to_item = None;
        state.focused = Some(id.to_string());
        let expand = expand.unwrap_or(!state.expanded.contains(id));
        if expand {
            state.expanded.insert(id.to_string());
            node.error = None;
            if !state.manual_requests.iter().any(|pending| pending == id) {
                state.manual_requests.push_back(id.to_string());
            }
        } else {
            state.expanded.remove(id);
            state.manual_requests.retain(|pending| pending != id);
            if let Some(index) = state
                .reveal_chain
                .iter()
                .position(|ancestor| ancestor == id)
            {
                state.reveal_chain.truncate(index);
            }
        }
        // Expanding an errored directory explicitly retries it.
        state.dirty = true;
        let request =
            (expand && node.entries.is_none() && node.pending.is_none()).then(|| node.path.clone());
        self.cancel_obsolete_listing();
        request
    }

    pub(super) fn move_selection(&mut self, session: &str, delta: isize) {
        let Some(state) = self.sessions.get_mut(session) else {
            return;
        };
        if state.rows.is_empty() {
            return;
        }
        let index = state
            .focused
            .as_ref()
            .and_then(|id| state.rows.iter().position(|row| &row.key == id))
            .unwrap_or(0);
        let next = index.saturating_add_signed(delta).min(state.rows.len() - 1);
        state.interaction_version = state.interaction_version.wrapping_add(1);
        state.scroll_to_selection = false;
        state.scroll.0.borrow_mut().deferred_scroll_to_item = None;
        state.focused = Some(state.rows[next].key.clone());
        state.selected.clear();
        if !state.rows[next].root {
            state.selected.insert(state.rows[next].key.clone());
            state.anchor = Some(state.rows[next].key.clone());
        }
        state.scroll.scroll_to_item(next, ScrollStrategy::Top);
    }

    pub(super) fn selected_row(&self, session: &str) -> Option<TransferTreeRow> {
        let state = self.sessions.get(session)?;
        state
            .rows
            .iter()
            .find(|row| Some(&row.key) == state.focused.as_ref())
            .cloned()
    }

    pub(super) fn selected_entries(&self, session: &str) -> Vec<SftpFileEntry> {
        let Some(state) = self.sessions.get(session) else {
            return Vec::new();
        };
        state
            .rows
            .iter()
            .filter(|row| state.selected.contains(&row.key))
            .filter_map(|row| row.entry.clone())
            .collect()
    }

    pub(super) fn select_parent(&mut self, session: &str, id: &str) {
        let Some(state) = self.sessions.get_mut(session) else {
            return;
        };
        let Some(index) = state.rows.iter().position(|row| row.key == id) else {
            return;
        };
        state.interaction_version = state.interaction_version.wrapping_add(1);
        state.scroll_to_selection = false;
        state.scroll.0.borrow_mut().deferred_scroll_to_item = None;
        let depth = state.rows[index].depth;
        if let Some(row) = state.rows[..index]
            .iter()
            .rev()
            .find(|row| row.depth < depth)
        {
            state.focused = Some(row.key.clone());
            state.selected.clear();
            if !row.root {
                state.selected.insert(row.key.clone());
                state.anchor = Some(row.key.clone());
            }
        }
    }
}

impl TreeSession {
    fn ensure_backend(&mut self, backend: FileBrowserBackendKind) {
        if self.backend.is_some_and(|current| current != backend) {
            *self = Self::default();
        }
        self.backend = Some(backend);
    }

    fn apply_listing(&mut self, path: RemoteFilePath, entries: Arc<Vec<SftpFileEntry>>) {
        let Some(backend) = self.backend else {
            return;
        };
        let id = key(backend, &path);
        if self.nodes.get(&id).is_some_and(|node| {
            node.pending.is_none() && node.entries.as_deref() == Some(entries.as_ref())
        }) {
            return;
        }
        // Invalidate removed directories, but keep unrelated siblings' caches.
        let retained: HashSet<String> = entries
            .iter()
            .filter(|e| e.file_type == SftpFileType::Directory)
            .map(|e| key(backend, &e.remote_path()))
            .collect();
        let removed: Vec<String> = self
            .nodes
            .get(&id)
            .and_then(|node| node.entries.as_ref())
            .into_iter()
            .flat_map(|entries| entries.iter())
            .filter(|entry| entry.file_type == SftpFileType::Directory)
            .map(|e| key(backend, &e.remote_path()))
            .filter(|id| !retained.contains(id))
            .collect();
        for id in removed {
            self.remove_subtree(&id, &mut HashSet::new());
        }
        for entry in entries
            .iter()
            .filter(|e| e.file_type == SftpFileType::Directory && e.name != "." && e.name != "..")
        {
            self.nodes
                .entry(key(backend, &entry.remote_path()))
                .or_insert_with(|| DirectoryNode::new(entry.remote_path()));
        }
        let node = self
            .nodes
            .entry(id)
            .or_insert_with(|| DirectoryNode::new(path));
        node.entries = Some(entries);
        node.pending = None;
        node.error = None;
        self.dirty = true;
    }

    fn remove_subtree(&mut self, id: &str, seen: &mut HashSet<String>) {
        if !seen.insert(id.to_string()) {
            return;
        }
        if let Some(node) = self.nodes.remove(id)
            && let Some(entries) = node.entries
            && let Some(backend) = self.backend
        {
            for entry in entries
                .iter()
                .filter(|entry| entry.file_type == SftpFileType::Directory)
            {
                self.remove_subtree(&key(backend, &entry.remote_path()), seen);
            }
        }
        self.expanded.remove(id);
        self.manual_requests.retain(|pending| pending != id);
        if let Some(index) = self.reveal_chain.iter().position(|ancestor| ancestor == id) {
            self.reveal_chain.truncate(index);
        }
        self.selected.remove(id);
        if self.focused.as_deref() == Some(id) {
            self.focused = None;
        }
        if self.anchor.as_deref() == Some(id) {
            self.anchor = None;
        }
    }

    fn append_rows(
        &self,
        path: &RemoteFilePath,
        entry: Option<&SftpFileEntry>,
        depth: usize,
        visited: &mut HashSet<String>,
        rows: &mut Vec<TransferTreeRow>,
    ) {
        let Some(backend) = self.backend else {
            return;
        };
        let id = key(backend, path);
        if depth >= 64 || !visited.insert(id.clone()) {
            return;
        }
        let node = self.nodes.get(&id);
        let root = entry.is_none();
        let directory = entry.is_none_or(SftpFileEntry::is_directory);
        let expandable =
            root || entry.is_some_and(|entry| entry.file_type == SftpFileType::Directory);
        let expanded = expandable && self.expanded.contains(&id);
        rows.push(TransferTreeRow {
            key: id,
            path: path.clone(),
            label: entry
                .map(|e| e.name.clone())
                .unwrap_or_else(|| path.display_path.clone()),
            depth,
            directory,
            expandable,
            root,
            expanded,
            loading: node.is_some_and(|n| n.pending.is_some()),
            error: node.and_then(|n| n.error.clone()),
            entry: entry.cloned(),
        });
        if expanded && let Some(entries) = node.and_then(|node| node.entries.as_ref()) {
            let mut sorted: Vec<&SftpFileEntry> = entries
                .iter()
                .filter(|e| {
                    e.name != "."
                        && e.name != ".."
                        && (self.show_hidden || !e.name.starts_with('.'))
                })
                .collect();
            sorted.sort_by(|a, b| {
                b.is_directory()
                    .cmp(&a.is_directory())
                    .then_with(|| super::browser_logic::natural_compare_ascii(&a.name, &b.name))
            });
            for entry in sorted {
                self.append_rows(&entry.remote_path(), Some(entry), depth + 1, visited, rows);
            }
        }
    }
}

impl super::TransferFeatureState {
    pub(in crate::features) fn tree_paths_equal(
        &self,
        backend: FileBrowserBackendKind,
        left: &RemoteFilePath,
        right: &RemoteFilePath,
    ) -> bool {
        key(backend, left) == key(backend, right)
    }

    pub(in crate::features) fn tree_is_initialized(&self, session: &str) -> bool {
        self.tree
            .sessions
            .get(session)
            .is_some_and(|state| state.root.is_some())
    }
    pub(in crate::features) fn tree_presentation(
        &mut self,
        session: Option<&str>,
        show_hidden: bool,
    ) -> TransferTreePresentation {
        self.tree.presentation(session, show_hidden)
    }
    pub(in crate::features) fn seed_tree_listing(
        &mut self,
        session: &str,
        backend: FileBrowserBackendKind,
        path: RemoteFilePath,
        entries: Arc<Vec<SftpFileEntry>>,
    ) {
        self.tree.seed(session, backend, path, entries);
    }
    pub(in crate::features) fn reveal_tree_path(
        &mut self,
        session: &str,
        backend: FileBrowserBackendKind,
        path: RemoteFilePath,
    ) -> Vec<RemoteFilePath> {
        self.tree.reveal(session, backend, path)
    }
    pub(in crate::features) fn begin_tree_request(
        &mut self,
        session: &str,
        backend: FileBrowserBackendKind,
        path: RemoteFilePath,
    ) -> Option<u64> {
        self.tree.begin_request(session, backend, path)
    }
    pub(in crate::features) fn complete_tree_request(
        &mut self,
        session: &str,
        path: &RemoteFilePath,
        generation: u64,
        result: Result<Vec<SftpFileEntry>, String>,
    ) -> bool {
        self.tree.complete(session, path, generation, result)
    }
    pub(in crate::features) fn invalidate_tree_path(
        &mut self,
        session: &str,
        path: &RemoteFilePath,
        subtree: bool,
    ) {
        self.tree.invalidate(session, path, subtree);
    }
    pub(in crate::features) fn select_tree_row(
        &mut self,
        session: &str,
        id: String,
        additive: bool,
        range: bool,
    ) {
        self.tree.select(session, id, additive, range);
    }
    pub(in crate::features) fn select_all_tree_rows(&mut self, session: &str) {
        self.tree.select_all(session);
    }
    pub(in crate::features) fn select_tree_context_row(&mut self, session: &str, id: String) {
        self.tree.select_context(session, id);
    }
    pub(in crate::features) fn clear_tree_selection(&mut self, session: &str) {
        self.tree.clear_selection(session);
    }
    pub(in crate::features) fn toggle_tree_node(
        &mut self,
        session: &str,
        id: &str,
        expand: Option<bool>,
    ) -> Option<RemoteFilePath> {
        self.tree.toggle(session, id, expand)
    }
    pub(in crate::features) fn move_tree_selection(&mut self, session: &str, delta: isize) {
        self.tree.move_selection(session, delta);
    }
    pub(in crate::features) fn selected_tree_row(&self, session: &str) -> Option<TransferTreeRow> {
        self.tree.selected_row(session)
    }
    pub(in crate::features) fn selected_tree_entries(&self, session: &str) -> Vec<SftpFileEntry> {
        self.tree.selected_entries(session)
    }
    pub(in crate::features) fn select_tree_parent(&mut self, session: &str, id: &str) {
        self.tree.select_parent(session, id);
    }
    pub(in crate::features) fn tree_focus(&self) -> &FocusHandle {
        &self.tree_focus
    }
}

#[cfg(test)]
mod tests;
