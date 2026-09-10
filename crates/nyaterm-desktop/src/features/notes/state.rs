use std::collections::{HashMap, HashSet};

use nyaterm_core::{
    DeleteNoteNodeResult, NoteFolder, NoteNodeChange, NoteNodeKind, NoteSummary, NoteTreePayload,
    NotesUiState,
};
use nyaterm_ui::NyaWindowHandle;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::features) struct NoteTreeRow {
    pub id: String,
    pub kind: NoteNodeKind,
    pub parent_id: Option<String>,
    pub name: String,
    pub depth: usize,
    pub expanded: bool,
    pub has_children: bool,
}

pub(in crate::features) struct NotesFeatureState {
    folders: Vec<NoteFolder>,
    notes: Vec<NoteSummary>,
    expanded_folder_ids: HashSet<String>,
    selected_node_id: Option<String>,
    loaded: bool,
    loading: bool,
    error: Option<String>,
    generation: u64,
    editor_windows: HashMap<String, NyaWindowHandle>,
    pending_editor_windows: HashSet<String>,
}

impl NotesFeatureState {
    pub fn new() -> Self {
        Self {
            folders: Vec::new(),
            notes: Vec::new(),
            expanded_folder_ids: HashSet::new(),
            selected_node_id: None,
            loaded: false,
            loading: false,
            error: None,
            generation: 0,
            editor_windows: HashMap::new(),
            pending_editor_windows: HashSet::new(),
        }
    }

    pub fn folders(&self) -> &[NoteFolder] {
        &self.folders
    }

    pub fn notes(&self) -> &[NoteSummary] {
        &self.notes
    }

    pub fn revisions(&self) -> HashMap<String, u64> {
        self.notes
            .iter()
            .map(|note| (note.id.clone(), note.revision))
            .collect()
    }

    pub fn selected_node_id(&self) -> Option<&str> {
        self.selected_node_id.as_deref()
    }

    pub fn loaded(&self) -> bool {
        self.loaded
    }

    pub fn loading(&self) -> bool {
        self.loading
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn begin_load(&mut self) -> Option<u64> {
        if self.loading {
            return None;
        }
        self.loading = true;
        self.error = None;
        self.generation = self.generation.wrapping_add(1);
        Some(self.generation)
    }

    pub fn begin_refresh(&mut self) -> u64 {
        self.loading = true;
        self.error = None;
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    pub fn apply_load(&mut self, generation: u64, payload: NoteTreePayload) -> bool {
        if generation != self.generation {
            return false;
        }
        self.loading = false;
        self.loaded = true;
        self.error = None;
        self.folders = payload.folders;
        self.notes = payload.notes;
        self.expanded_folder_ids = payload.ui.expanded_folder_ids.into_iter().collect();
        self.selected_node_id = payload.ui.last_selected_node_id;
        self.sanitize_ui_state();
        true
    }

    pub fn fail_load(&mut self, generation: u64, error: String) -> bool {
        if generation != self.generation {
            return false;
        }
        self.loading = false;
        self.loaded = true;
        self.error = Some(error);
        true
    }

    pub fn set_selected(&mut self, selected: Option<String>) -> bool {
        if self.selected_node_id == selected {
            return false;
        }
        self.selected_node_id = selected;
        true
    }

    pub fn toggle_folder(&mut self, folder_id: &str) -> bool {
        if !self.folders.iter().any(|folder| folder.id == folder_id) {
            return false;
        }
        if !self.expanded_folder_ids.remove(folder_id) {
            self.expanded_folder_ids.insert(folder_id.to_string());
        }
        true
    }

    pub fn set_all_expanded(&mut self, expanded: bool) {
        self.expanded_folder_ids = if expanded {
            self.folders
                .iter()
                .map(|folder| folder.id.clone())
                .collect()
        } else {
            HashSet::new()
        };
    }

    pub fn ui_state(&self) -> NotesUiState {
        let mut expanded_folder_ids = self.expanded_folder_ids.iter().cloned().collect::<Vec<_>>();
        expanded_folder_ids.sort();
        NotesUiState {
            expanded_folder_ids,
            last_selected_node_id: self.selected_node_id.clone(),
        }
    }

    pub fn upsert_folder(&mut self, folder: NoteFolder) {
        if let Some(existing) = self.folders.iter_mut().find(|item| item.id == folder.id) {
            *existing = folder;
        } else {
            self.folders.push(folder);
        }
    }

    pub fn upsert_note(&mut self, note: NoteSummary) {
        if let Some(existing) = self.notes.iter_mut().find(|item| item.id == note.id) {
            *existing = note;
        } else {
            self.notes.push(note);
        }
    }

    pub fn apply_node_change(&mut self, change: NoteNodeChange) {
        if let Some(folder) = change.folder {
            self.upsert_folder(folder);
        }
        if let Some(note) = change.note {
            self.upsert_note(note);
        }
    }

    pub fn apply_delete(&mut self, result: &DeleteNoteNodeResult) {
        let ids = result
            .ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        self.folders
            .retain(|folder| !ids.contains(folder.id.as_str()));
        self.notes.retain(|note| !ids.contains(note.id.as_str()));
        self.expanded_folder_ids
            .retain(|id| !ids.contains(id.as_str()));
        if self
            .selected_node_id
            .as_deref()
            .is_some_and(|id| ids.contains(id))
        {
            self.selected_node_id = None;
        }
    }

    pub fn node(&self, node_id: &str) -> Option<(NoteNodeKind, Option<String>, String)> {
        self.folders
            .iter()
            .find(|folder| folder.id == node_id)
            .map(|folder| {
                (
                    NoteNodeKind::Folder,
                    folder.parent_id.clone(),
                    folder.name.clone(),
                )
            })
            .or_else(|| {
                self.notes
                    .iter()
                    .find(|note| note.id == node_id)
                    .map(|note| {
                        (
                            NoteNodeKind::Note,
                            note.parent_id.clone(),
                            note.title.clone(),
                        )
                    })
            })
    }

    pub fn visible_rows(&self, search: &str) -> Vec<NoteTreeRow> {
        visible_note_rows(
            &self.folders,
            &self.notes,
            &self.expanded_folder_ids,
            search,
        )
    }

    pub fn next_sort_order(&self, parent_id: Option<&str>) -> i64 {
        self.folders
            .iter()
            .filter(|item| item.parent_id.as_deref() == parent_id)
            .map(|item| item.sort_order)
            .chain(
                self.notes
                    .iter()
                    .filter(|item| item.parent_id.as_deref() == parent_id)
                    .map(|item| item.sort_order),
            )
            .max()
            .unwrap_or(-1)
            .saturating_add(1)
    }

    pub fn unique_name_for_parent(&self, parent_id: Option<&str>, base: &str) -> String {
        let sibling_names = self
            .folders
            .iter()
            .filter(|item| item.parent_id.as_deref() == parent_id)
            .map(|item| item.name.to_lowercase())
            .chain(
                self.notes
                    .iter()
                    .filter(|item| item.parent_id.as_deref() == parent_id)
                    .map(|item| item.title.to_lowercase()),
            )
            .collect::<HashSet<_>>();
        if !sibling_names.contains(&base.to_lowercase()) {
            return base.to_string();
        }
        (2..10_000)
            .map(|index| format!("{base} {index}"))
            .find(|candidate| !sibling_names.contains(&candidate.to_lowercase()))
            .unwrap_or_else(|| base.to_string())
    }

    pub fn set_folder_expanded(&mut self, folder_id: &str, expanded: bool) -> bool {
        if !self.folders.iter().any(|folder| folder.id == folder_id) {
            return false;
        }
        if expanded {
            self.expanded_folder_ids.insert(folder_id.to_string())
        } else {
            self.expanded_folder_ids.remove(folder_id)
        }
    }

    pub fn delete_counts(&self, node_id: &str) -> Option<(usize, usize)> {
        if self.notes.iter().any(|note| note.id == node_id) {
            return Some((0, 1));
        }
        if !self.folders.iter().any(|folder| folder.id == node_id) {
            return None;
        }
        let mut folder_ids = HashSet::from([node_id]);
        loop {
            let before = folder_ids.len();
            for folder in &self.folders {
                if folder
                    .parent_id
                    .as_deref()
                    .is_some_and(|parent| folder_ids.contains(parent))
                {
                    folder_ids.insert(folder.id.as_str());
                }
            }
            if folder_ids.len() == before {
                break;
            }
        }
        let note_count = self
            .notes
            .iter()
            .filter(|note| {
                note.parent_id
                    .as_deref()
                    .is_some_and(|parent| folder_ids.contains(parent))
            })
            .count();
        Some((folder_ids.len(), note_count))
    }

    pub fn can_move_to(&self, node_id: &str, target_parent_id: Option<&str>) -> bool {
        let Some(target_id) = target_parent_id else {
            return self.node(node_id).is_some();
        };
        if !self.folders.iter().any(|folder| folder.id == target_id) {
            return false;
        }
        let Some((kind, _, _)) = self.node(node_id) else {
            return false;
        };
        if kind == NoteNodeKind::Note {
            return true;
        }
        let mut current = Some(target_id);
        let mut seen = HashSet::new();
        while let Some(id) = current {
            if id == node_id || !seen.insert(id) {
                return false;
            }
            current = self
                .folders
                .iter()
                .find(|folder| folder.id == id)
                .and_then(|folder| folder.parent_id.as_deref());
        }
        true
    }

    pub(in crate::features) fn has_open_editor_windows(&self) -> bool {
        !self.editor_windows.is_empty() || !self.pending_editor_windows.is_empty()
    }

    pub fn editor_window(&self, note_id: &str) -> Option<NyaWindowHandle> {
        self.editor_windows.get(note_id).copied()
    }

    pub fn begin_editor_window(&mut self, note_id: &str) -> bool {
        self.pending_editor_windows.insert(note_id.to_string())
    }

    pub fn finish_editor_window(&mut self, note_id: String, handle: Option<NyaWindowHandle>) {
        self.pending_editor_windows.remove(&note_id);
        if let Some(handle) = handle {
            self.editor_windows.insert(note_id, handle);
        }
    }

    pub fn remove_editor_window(&mut self, note_id: &str) {
        self.pending_editor_windows.remove(note_id);
        self.editor_windows.remove(note_id);
    }

    pub fn take_editor_window(&mut self, note_id: &str) -> Option<NyaWindowHandle> {
        self.pending_editor_windows.remove(note_id);
        self.editor_windows.remove(note_id)
    }

    pub fn rekey_editor_window(&mut self, old_note_id: &str, new_note_id: String) {
        self.pending_editor_windows.remove(old_note_id);
        if let Some(handle) = self.editor_windows.remove(old_note_id) {
            self.editor_windows.insert(new_note_id, handle);
        }
    }

    fn sanitize_ui_state(&mut self) {
        let folder_ids = self
            .folders
            .iter()
            .map(|folder| folder.id.as_str())
            .collect::<HashSet<_>>();
        self.expanded_folder_ids
            .retain(|id| folder_ids.contains(id.as_str()));
        let selected_exists = self.selected_node_id.as_deref().is_none_or(|selected| {
            folder_ids.contains(selected)
                || self.notes.iter().any(|note| note.id.as_str() == selected)
        });
        if !selected_exists {
            self.selected_node_id = None;
        }
    }
}

fn visible_note_rows(
    folders: &[NoteFolder],
    notes: &[NoteSummary],
    expanded: &HashSet<String>,
    search: &str,
) -> Vec<NoteTreeRow> {
    #[derive(Clone)]
    struct Item {
        id: String,
        kind: NoteNodeKind,
        parent_id: Option<String>,
        name: String,
        sort_order: i64,
    }

    let mut items = Vec::with_capacity(folders.len() + notes.len());
    items.extend(folders.iter().map(|folder| Item {
        id: folder.id.clone(),
        kind: NoteNodeKind::Folder,
        parent_id: folder.parent_id.clone(),
        name: folder.name.clone(),
        sort_order: folder.sort_order,
    }));
    items.extend(notes.iter().map(|note| Item {
        id: note.id.clone(),
        kind: NoteNodeKind::Note,
        parent_id: note.parent_id.clone(),
        name: note.title.clone(),
        sort_order: note.sort_order,
    }));
    let folder_parent = folders
        .iter()
        .map(|folder| (folder.id.as_str(), folder.parent_id.as_deref()))
        .collect::<HashMap<_, _>>();
    let keyword = search.trim().to_lowercase();
    let mut visible_ids = HashSet::new();
    if keyword.is_empty() {
        visible_ids.extend(items.iter().map(|item| item.id.clone()));
    } else {
        for item in &items {
            if !item.name.to_lowercase().contains(&keyword) {
                continue;
            }
            visible_ids.insert(item.id.clone());
            let mut current = item.parent_id.as_deref();
            let mut seen = HashSet::new();
            while let Some(parent) = current {
                if !seen.insert(parent) {
                    break;
                }
                visible_ids.insert(parent.to_string());
                current = folder_parent.get(parent).copied().flatten();
            }
        }
    }

    let mut children: HashMap<Option<String>, Vec<Item>> = HashMap::new();
    let folder_ids = folders
        .iter()
        .map(|folder| folder.id.as_str())
        .collect::<HashSet<_>>();
    for mut item in items {
        if !visible_ids.contains(&item.id) {
            continue;
        }
        if item
            .parent_id
            .as_deref()
            .is_some_and(|parent| !folder_ids.contains(parent))
        {
            item.parent_id = None;
        }
        children
            .entry(item.parent_id.clone())
            .or_default()
            .push(item);
    }
    for siblings in children.values_mut() {
        siblings.sort_by(|left, right| {
            left.sort_order
                .cmp(&right.sort_order)
                .then_with(|| match (left.kind, right.kind) {
                    (NoteNodeKind::Folder, NoteNodeKind::Note) => std::cmp::Ordering::Less,
                    (NoteNodeKind::Note, NoteNodeKind::Folder) => std::cmp::Ordering::Greater,
                    _ => std::cmp::Ordering::Equal,
                })
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                .then_with(|| left.id.cmp(&right.id))
        });
    }

    fn append_rows(
        parent: Option<String>,
        depth: usize,
        children: &HashMap<Option<String>, Vec<Item>>,
        expanded: &HashSet<String>,
        searching: bool,
        visited: &mut HashSet<String>,
        rows: &mut Vec<NoteTreeRow>,
    ) {
        let Some(items) = children.get(&parent) else {
            return;
        };
        for item in items {
            if !visited.insert(item.id.clone()) {
                continue;
            }
            let has_children = children.contains_key(&Some(item.id.clone()));
            let is_expanded =
                item.kind == NoteNodeKind::Folder && (searching || expanded.contains(&item.id));
            rows.push(NoteTreeRow {
                id: item.id.clone(),
                kind: item.kind,
                parent_id: item.parent_id.clone(),
                name: item.name.clone(),
                depth,
                expanded: is_expanded,
                has_children,
            });
            if is_expanded {
                append_rows(
                    Some(item.id.clone()),
                    depth + 1,
                    children,
                    expanded,
                    searching,
                    visited,
                    rows,
                );
            }
        }
    }

    let mut rows = Vec::new();
    append_rows(
        None,
        0,
        &children,
        expanded,
        !keyword.is_empty(),
        &mut HashSet::new(),
        &mut rows,
    );
    rows
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap, HashSet};

    use nyaterm_core::{NoteFolder, NoteSummary, NoteTreePayload, NotesUiState};

    use super::{NotesFeatureState, visible_note_rows};

    fn folder(id: &str, parent: Option<&str>, order: i64) -> NoteFolder {
        NoteFolder {
            id: id.into(),
            parent_id: parent.map(str::to_string),
            name: id.into(),
            sort_order: order,
            created_at_ms: 1,
            updated_at_ms: 1,
            extra: BTreeMap::new(),
        }
    }

    fn note(id: &str, parent: Option<&str>, order: i64) -> NoteSummary {
        NoteSummary {
            id: id.into(),
            parent_id: parent.map(str::to_string),
            title: id.into(),
            sort_order: order,
            revision: 1,
            created_at_ms: 1,
            updated_at_ms: 1,
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn rows_sort_expand_and_keep_search_ancestors() {
        let folders = vec![folder("root", None, 1), folder("child", Some("root"), 0)];
        let notes = vec![note("match", Some("child"), 0), note("first", None, 0)];
        let collapsed = visible_note_rows(&folders, &notes, &HashSet::new(), "");
        assert_eq!(
            collapsed
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "root"]
        );
        let searched = visible_note_rows(&folders, &notes, &HashSet::new(), "match");
        assert_eq!(
            searched
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            vec!["root", "child", "match"]
        );
    }

    #[test]
    fn stale_load_does_not_replace_newer_catalog() {
        let mut state = NotesFeatureState::new();
        let first = state.begin_load().expect("first load");
        state.loading = false;
        let second = state.begin_load().expect("second load");
        assert!(!state.apply_load(
            first,
            NoteTreePayload {
                folders: vec![folder("stale", None, 0)],
                notes: vec![],
                ui: NotesUiState::default(),
            },
        ));
        assert!(state.apply_load(
            second,
            NoteTreePayload {
                folders: vec![folder("current", None, 0)],
                notes: vec![],
                ui: NotesUiState::default(),
            },
        ));
        assert_eq!(state.folders()[0].id, "current");
    }

    #[test]
    fn delete_counts_include_all_folder_descendants_and_their_notes() {
        let mut state = NotesFeatureState::new();
        state.folders = vec![folder("root", None, 0), folder("child", Some("root"), 0)];
        state.notes = vec![note("nested", Some("child"), 0), note("outside", None, 0)];

        assert_eq!(state.delete_counts("root"), Some((2, 1)));
        assert_eq!(state.delete_counts("nested"), Some((0, 1)));
        assert_eq!(state.delete_counts("missing"), None);
        assert!(!state.can_move_to("root", Some("child")));
        assert!(!state.can_move_to("root", Some("root")));
        assert!(state.can_move_to("child", None));
        assert!(state.can_move_to("nested", Some("root")));
    }

    #[test]
    fn moved_nodes_append_and_target_folders_can_be_expanded() {
        let mut state = NotesFeatureState::new();
        state.folders = vec![folder("root", None, 2), folder("child", Some("root"), 4)];
        state.notes = vec![
            note("root-note", None, 7),
            note("child-note", Some("root"), 9),
        ];

        assert_eq!(state.next_sort_order(None), 8);
        assert_eq!(state.next_sort_order(Some("root")), 10);
        assert!(state.set_folder_expanded("root", true));
        assert!(
            state
                .ui_state()
                .expanded_folder_ids
                .contains(&"root".into())
        );
        assert!(!state.set_folder_expanded("root", true));
    }

    #[test]
    fn catalog_revisions_are_keyed_by_note_id() {
        let mut state = NotesFeatureState::new();
        let mut first = note("first", None, 0);
        first.revision = 3;
        let mut second = note("second", None, 1);
        second.revision = 7;
        state.notes = vec![first, second];

        assert_eq!(
            state.revisions(),
            HashMap::from([("first".to_string(), 3), ("second".to_string(), 7)])
        );
    }
}
