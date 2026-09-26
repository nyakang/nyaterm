//! Session-local directory caches. No listing request is issued by presentation.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{FocusHandle, ScrollStrategy, UniformListScrollHandle};
use nyaterm_transport::{
    FileBrowserBackendKind, RemoteFilePath, SftpFileEntry, SftpFileType, file_browser_identity,
    file_browser_parent, file_browser_path_is_root, file_browser_root,
};

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
}

#[derive(Default)]
pub(super) struct TransferTreeState {
    sessions: HashMap<String, TreeSession>,
    next_generation: u64,
}

// Valid UTF-8 tokens and display-only paths denote the same directory; invalid
// UTF-8 paths must retain their byte identity even when their labels collide.
fn key(backend: FileBrowserBackendKind, path: &RemoteFilePath) -> String {
    match (backend, path.raw_path()) {
        (FileBrowserBackendKind::Remote, Ok(Some(raw)))
            if std::str::from_utf8(&raw).ok() == Some(path.display_path.as_str()) =>
        {
            path.display_path.clone()
        }
        (FileBrowserBackendKind::Remote, _) => path.identity_key(),
        (FileBrowserBackendKind::Local, _) => file_browser_identity(backend, &path.display_path),
    }
}

impl TransferTreeState {
    pub(super) fn remove_session(&mut self, session: &str) {
        self.sessions.remove(session);
    }

    pub(super) fn replace_session(&mut self, old: &str, new: &str) {
        if let Some(state) = self.sessions.remove(old) {
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
            if state
                .focused
                .as_deref()
                .is_some_and(|id| !visible.contains(id))
            {
                state.focused = None;
            }
            if state
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
            FileBrowserBackendKind::Remote => "/".to_string(),
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
            *state = TreeSession::default();
            state.backend = Some(backend);
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
                FileBrowserBackendKind::Remote => match current.parent() {
                    Ok(parent) => parent,
                    Err(_) => break,
                },
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
        let mut missing = Vec::new();
        for path in chain.into_iter().rev() {
            let id = key(backend, &path);
            state.expanded.insert(id.clone());
            let node = state
                .nodes
                .entry(id)
                .or_insert_with(|| DirectoryNode::new(path));
            if node.entries.is_none() && node.pending.is_none() {
                missing.push(node.path.clone());
            }
        }
        state.dirty = true;
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
    }

    pub(super) fn select(&mut self, session: &str, id: String, additive: bool, range: bool) {
        let Some(state) = self.sessions.get_mut(session) else {
            return;
        };
        let Some(target) = state.rows.iter().position(|row| row.key == id) else {
            return;
        };
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
        state.focused = Some(id.to_string());
        let expand = expand.unwrap_or(!state.expanded.contains(id));
        if expand {
            state.expanded.insert(id.to_string());
        } else {
            state.expanded.remove(id);
        }
        // Expanding an errored directory explicitly retries it.
        state.dirty = true;
        (expand && node.entries.is_none() && node.pending.is_none()).then(|| node.path.clone())
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
    pub(in crate::features) fn tree_is_initialized(&self, session: &str) -> bool {
        self.tree
            .sessions
            .get(session)
            .is_some_and(|state| state.root.is_some())
    }
    pub(in crate::features) fn missing_expanded_tree_paths(
        &self,
        session: &str,
    ) -> Vec<RemoteFilePath> {
        self.tree
            .sessions
            .get(session)
            .into_iter()
            .flat_map(|state| {
                state
                    .expanded
                    .iter()
                    .filter_map(|id| state.nodes.get(id))
                    .filter(|node| {
                        node.entries.is_none() && node.pending.is_none() && node.error.is_none()
                    })
                    .map(|node| node.path.clone())
            })
            .collect()
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
mod tests {
    use super::TransferTreeState;
    use nyaterm_transport::{FileBrowserBackendKind, RemoteFilePath, SftpFileEntry, SftpFileType};
    use std::sync::Arc;

    fn directory(name: &str, path: &str) -> SftpFileEntry {
        SftpFileEntry {
            name: name.into(),
            path: path.into(),
            file_type: SftpFileType::Directory,
            size: None,
            permissions: None,
            owner: String::new(),
            group: String::new(),
            modified_at: None,
            raw_path_token: None,
            symlink_target_is_directory: false,
        }
    }

    fn file(name: &str, path: &str) -> SftpFileEntry {
        SftpFileEntry {
            name: name.into(),
            path: path.into(),
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
    fn invalidation_rejects_old_results_and_preserves_other_parents() {
        let mut tree = TransferTreeState::default();
        tree.seed(
            "a",
            FileBrowserBackendKind::Remote,
            RemoteFilePath::new("/"),
            Arc::new(vec![directory("one", "/one"), directory("two", "/two")]),
        );
        let old = tree
            .begin_request(
                "a",
                FileBrowserBackendKind::Remote,
                RemoteFilePath::new("/one"),
            )
            .unwrap();
        let two = tree
            .begin_request(
                "a",
                FileBrowserBackendKind::Remote,
                RemoteFilePath::new("/two"),
            )
            .unwrap();
        tree.invalidate("a", &RemoteFilePath::new("/one"), false);
        assert!(!tree.complete("a", &RemoteFilePath::new("/one"), old, Ok(vec![])));
        assert!(tree.complete("a", &RemoteFilePath::new("/two"), two, Ok(vec![])));
        let new = tree
            .begin_request(
                "a",
                FileBrowserBackendKind::Remote,
                RemoteFilePath::new("/one"),
            )
            .unwrap();
        assert_ne!(old, new);
    }

    #[test]
    fn session_scope_raw_identity_and_keyboard_selection_are_independent() {
        let mut tree = TransferTreeState::default();
        let mut first = directory("same", "/same");
        first.raw_path_token = RemoteFilePath::from_raw("/same", b"/\xff").raw_path_token;
        let mut second = first.clone();
        second.raw_path_token = RemoteFilePath::from_raw("/same", b"/\xfe").raw_path_token;
        tree.seed(
            "a",
            FileBrowserBackendKind::Remote,
            RemoteFilePath::new("/"),
            Arc::new(vec![first, second]),
        );
        tree.reveal(
            "a",
            FileBrowserBackendKind::Remote,
            RemoteFilePath::new("/"),
        );
        let view = tree.presentation(Some("a"), false);
        assert_eq!(view.rows.len(), 3);
        assert_ne!(view.rows[1].key, view.rows[2].key);
        tree.move_selection("a", 1);
        assert_eq!(tree.selected_row("a").unwrap().depth, 1);
        assert_eq!(tree.presentation(Some("b"), false).rows.len(), 0);
        tree.remove_session("a");
        assert!(tree.presentation(Some("a"), false).rows.is_empty());
    }

    #[test]
    fn reveal_missing_ancestors_and_seed_supersede_pending_listings() {
        let mut tree = TransferTreeState::default();
        let pending = tree
            .begin_request(
                "a",
                FileBrowserBackendKind::Remote,
                RemoteFilePath::new("/a/b"),
            )
            .unwrap();
        tree.seed(
            "a",
            FileBrowserBackendKind::Remote,
            RemoteFilePath::new("/a/b"),
            Arc::new(vec![]),
        );
        assert!(!tree.complete(
            "a",
            &RemoteFilePath::new("/a/b"),
            pending,
            Ok(vec![directory("old", "/a/b/old")])
        ));
        let missing = tree.reveal(
            "a",
            FileBrowserBackendKind::Remote,
            RemoteFilePath::new("/a/b"),
        );
        assert_eq!(
            missing
                .iter()
                .map(|p| p.display_path.as_str())
                .collect::<Vec<_>>(),
            vec!["/", "/a"]
        );
        assert!(tree.sessions["a"].scroll_to_selection);
        tree.seed(
            "a",
            FileBrowserBackendKind::Remote,
            RemoteFilePath::new("/"),
            Arc::new(vec![directory("a", "/a")]),
        );
        tree.seed(
            "a",
            FileBrowserBackendKind::Remote,
            RemoteFilePath::new("/a"),
            Arc::new(vec![directory("b", "/a/b")]),
        );
        tree.presentation(Some("a"), false);
        assert!(!tree.sessions["a"].scroll_to_selection);
    }

    #[test]
    fn initial_relative_remote_directory_uses_filesystem_root() {
        let mut tree = TransferTreeState::default();
        let missing = tree.reveal(
            "a",
            FileBrowserBackendKind::Remote,
            RemoteFilePath::new("."),
        );
        assert_eq!(missing, vec![RemoteFilePath::new("/")]);
        let view = tree.presentation(Some("a"), false);
        assert_eq!(view.rows.len(), 1);
        assert_eq!(view.rows[0].label, "/");
        assert_eq!(view.rows[0].path, RemoteFilePath::new("/"));
    }

    #[test]
    fn selection_excludes_root_supports_ranges_and_directory_symlinks_do_not_expand() {
        let mut symlink = file("linked-dir", "/linked-dir");
        symlink.file_type = SftpFileType::Symlink;
        symlink.symlink_target_is_directory = true;
        let mut tree = TransferTreeState::default();
        tree.seed(
            "a",
            FileBrowserBackendKind::Remote,
            RemoteFilePath::new("/"),
            Arc::new(vec![
                directory("folder", "/folder"),
                file("one.txt", "/one.txt"),
                symlink,
            ]),
        );
        tree.reveal(
            "a",
            FileBrowserBackendKind::Remote,
            RemoteFilePath::new("/"),
        );
        let view = tree.presentation(Some("a"), false);
        let root = view.rows.iter().find(|row| row.root).unwrap();
        let folder = view.rows.iter().find(|row| row.label == "folder").unwrap();
        let linked = view
            .rows
            .iter()
            .find(|row| row.label == "linked-dir")
            .unwrap();
        assert!(folder.expandable);
        assert!(linked.directory);
        assert!(!linked.expandable);
        assert!(tree.toggle("a", &linked.key, Some(true)).is_none());

        tree.select("a", root.key.clone(), false, false);
        assert!(tree.selected_entries("a").is_empty());
        tree.select("a", folder.key.clone(), false, false);
        tree.select("a", linked.key.clone(), false, true);
        assert_eq!(tree.selected_entries("a").len(), 2);
        tree.select_all("a");
        assert_eq!(tree.selected_entries("a").len(), 3);
    }
}
