use std::collections::{BTreeMap, HashMap, HashSet};

use nyaterm_core::{
    DeleteNoteNodeResult, NoteDocument, NoteFolder, NoteNodeChange, NoteNodeKind, NoteSummary,
    NoteTreePayload, NoteUpdateResult, NotesSnapshot, NotesUiState,
};
use redb::{ReadableDatabase, ReadableTable};
use serde_json::Value;

use super::{
    ConnectionStore, META_NOTE_SUMMARY_INDEX_VERSION, META_TABLE, NOTE_DOCUMENT_PREFIX,
    NOTE_FOLDER_PREFIX, NOTE_FOLDERS_TABLE, NOTE_SNAPSHOT_EXTRA_KEY, NOTE_SUMMARIES_TABLE,
    NOTE_SUMMARY_PREFIX, NOTES_TABLE, PORTABLE_OPAQUE_ENTITIES_TABLE, StorageError,
    clear_prefix_in_txn, current_time_ms, deserialize_json, entity_key, set_nested_json_value,
    write_json_in_txn,
};

const DEFAULT_NOTE_TITLE: &str = "新建笔记";
const DEFAULT_FOLDER_NAME: &str = "新建文件夹";
const MAX_NOTE_NAME_CHARS: usize = 120;
const NOTE_SUMMARY_INDEX_VERSION: u32 = 1;

trait NoteListItem {
    fn id(&self) -> &str;
    fn parent_id(&self) -> Option<&str>;
    fn title(&self) -> &str;
    fn sort_order(&self) -> i64;
}

impl NoteListItem for NoteDocument {
    fn id(&self) -> &str {
        &self.id
    }

    fn parent_id(&self) -> Option<&str> {
        self.parent_id.as_deref()
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn sort_order(&self) -> i64 {
        self.sort_order
    }
}

impl NoteListItem for NoteSummary {
    fn id(&self) -> &str {
        &self.id
    }

    fn parent_id(&self) -> Option<&str> {
        self.parent_id.as_deref()
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn sort_order(&self) -> i64 {
        self.sort_order
    }
}

impl ConnectionStore {
    pub fn list_note_tree(&self) -> Result<NoteTreePayload, StorageError> {
        self.ensure_notes_ready()?;
        Ok(NoteTreePayload {
            folders: self.list_note_folders_ready()?,
            notes: self.list_note_summaries_ready()?,
            ui: self.load_notes_ui_state()?,
        })
    }

    pub fn list_note_folders(&self) -> Result<Vec<NoteFolder>, StorageError> {
        self.ensure_notes_ready()?;
        self.list_note_folders_ready()
    }

    pub fn list_notes(&self) -> Result<Vec<NoteDocument>, StorageError> {
        self.ensure_notes_ready()?;
        self.list_notes_ready()
    }

    pub fn list_note_summaries(&self) -> Result<Vec<NoteSummary>, StorageError> {
        self.ensure_notes_ready()?;
        self.list_note_summaries_ready()
    }

    pub fn get_note(&self, note_id: &str) -> Result<Option<NoteDocument>, StorageError> {
        self.ensure_notes_ready()?;
        self.read_json_table(NOTES_TABLE, &entity_key(NOTE_DOCUMENT_PREFIX, note_id))
    }

    pub fn create_note_folder(
        &self,
        parent_id: Option<String>,
        name: Option<String>,
    ) -> Result<NoteFolder, StorageError> {
        self.ensure_notes_ready()?;
        let txn = self.db.begin_write()?;
        let folders = read_note_folders_in_txn(&txn)?;
        let notes = read_note_summaries_in_txn(&txn)?;
        validate_parent_exists(&folders, parent_id.as_deref())?;
        let sibling_names = sibling_names(&folders, &notes, parent_id.as_deref(), None);
        let name = normalize_or_unique_name(name, DEFAULT_FOLDER_NAME, &sibling_names)?;
        let sort_order = next_sort_order_for_parent(&folders, &notes, parent_id.as_deref());
        let now = current_time_ms();
        let folder = NoteFolder {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id,
            name,
            sort_order,
            created_at_ms: now,
            updated_at_ms: now,
            extra: BTreeMap::new(),
        };
        write_note_folder_in_txn(&txn, &folder)?;
        txn.commit()?;
        Ok(folder)
    }

    pub fn create_note(
        &self,
        parent_id: Option<String>,
        title: Option<String>,
        markdown: Option<String>,
    ) -> Result<NoteDocument, StorageError> {
        self.ensure_notes_ready()?;
        let txn = self.db.begin_write()?;
        let folders = read_note_folders_in_txn(&txn)?;
        let notes = read_note_summaries_in_txn(&txn)?;
        validate_parent_exists(&folders, parent_id.as_deref())?;
        let sibling_names = sibling_names(&folders, &notes, parent_id.as_deref(), None);
        let title = normalize_or_unique_name(title, DEFAULT_NOTE_TITLE, &sibling_names)?;
        let sort_order = next_sort_order_for_parent(&folders, &notes, parent_id.as_deref());
        let now = current_time_ms();
        let note = NoteDocument {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id,
            title,
            markdown: markdown.unwrap_or_default(),
            sort_order,
            revision: 1,
            created_at_ms: now,
            updated_at_ms: now,
            extra: BTreeMap::new(),
        };
        write_note_in_txn(&txn, &note)?;
        write_note_summary_in_txn(&txn, &NoteSummary::from(&note))?;
        txn.commit()?;
        Ok(note)
    }

    pub fn update_note(
        &self,
        note_id: &str,
        title: String,
        markdown: String,
        expected_revision: u64,
        force: bool,
    ) -> Result<NoteUpdateResult, StorageError> {
        self.ensure_notes_ready()?;
        let txn = self.db.begin_write()?;
        let folders = read_note_folders_in_txn(&txn)?;
        let notes = read_note_summaries_in_txn(&txn)?;
        let mut note = read_note_in_txn(&txn, note_id)?
            .ok_or_else(|| invalid(format!("Note '{note_id}' does not exist")))?;
        if !force && note.revision != expected_revision {
            return Err(invalid(format!(
                "Revision conflict: expected {}, found {}",
                expected_revision, note.revision
            )));
        }

        let title = normalize_note_name(&title)?;
        validate_unique_sibling_name(
            &folders,
            &notes,
            note.parent_id.as_deref(),
            &title,
            Some((NoteNodeKind::Note, note_id)),
        )?;
        let changed = note.title != title || note.markdown != markdown;
        let tree_changed = note.title != title;
        if changed {
            note.title = title;
            note.markdown = markdown;
            note.revision = note.revision.saturating_add(1);
            note.updated_at_ms = current_time_ms();
            let summary =
                summary_from_document(&note, notes.iter().find(|item| item.id == note.id));
            write_note_in_txn(&txn, &note)?;
            write_note_summary_in_txn(&txn, &summary)?;
        }
        txn.commit()?;
        Ok(NoteUpdateResult {
            note,
            changed,
            tree_changed,
        })
    }

    pub fn rename_note_node(
        &self,
        node_kind: NoteNodeKind,
        node_id: &str,
        name: String,
    ) -> Result<NoteNodeChange, StorageError> {
        self.ensure_notes_ready()?;
        let txn = self.db.begin_write()?;
        let folders = read_note_folders_in_txn(&txn)?;
        let notes = read_note_summaries_in_txn(&txn)?;
        let name = normalize_note_name(&name)?;
        let change = match node_kind {
            NoteNodeKind::Folder => {
                let mut folder = folders
                    .iter()
                    .find(|item| item.id == node_id)
                    .cloned()
                    .ok_or_else(|| invalid(format!("Folder '{node_id}' does not exist")))?;
                validate_unique_sibling_name(
                    &folders,
                    &notes,
                    folder.parent_id.as_deref(),
                    &name,
                    Some((NoteNodeKind::Folder, node_id)),
                )?;
                let changed = folder.name != name;
                if changed {
                    folder.name = name;
                    folder.updated_at_ms = current_time_ms();
                    write_note_folder_in_txn(&txn, &folder)?;
                }
                NoteNodeChange {
                    changed,
                    tree_changed: changed,
                    folder: Some(folder),
                    note: None,
                }
            }
            NoteNodeKind::Note => {
                let existing_summary = notes
                    .iter()
                    .find(|item| item.id == node_id)
                    .ok_or_else(|| invalid(format!("Note '{node_id}' does not exist")))?;
                validate_unique_sibling_name(
                    &folders,
                    &notes,
                    existing_summary.parent_id.as_deref(),
                    &name,
                    Some((NoteNodeKind::Note, node_id)),
                )?;
                let mut note = read_note_in_txn(&txn, node_id)?
                    .ok_or_else(|| invalid(format!("Note '{node_id}' does not exist")))?;
                let changed = note.title != name;
                if changed {
                    note.title = name;
                    note.revision = note.revision.saturating_add(1);
                    note.updated_at_ms = current_time_ms();
                    write_note_in_txn(&txn, &note)?;
                    write_note_summary_in_txn(
                        &txn,
                        &summary_from_document(&note, Some(existing_summary)),
                    )?;
                }
                NoteNodeChange {
                    changed,
                    tree_changed: changed,
                    folder: None,
                    note: Some(summary_from_document(&note, Some(existing_summary))),
                }
            }
        };
        txn.commit()?;
        Ok(change)
    }

    pub fn move_note_node(
        &self,
        node_kind: NoteNodeKind,
        node_id: &str,
        parent_id: Option<String>,
        sort_order: i64,
    ) -> Result<NoteNodeChange, StorageError> {
        self.ensure_notes_ready()?;
        let txn = self.db.begin_write()?;
        let folders = read_note_folders_in_txn(&txn)?;
        let notes = read_note_summaries_in_txn(&txn)?;
        validate_parent_exists(&folders, parent_id.as_deref())?;

        let change = match node_kind {
            NoteNodeKind::Folder => {
                let mut folder = folders
                    .iter()
                    .find(|item| item.id == node_id)
                    .cloned()
                    .ok_or_else(|| invalid(format!("Folder '{node_id}' does not exist")))?;
                if parent_id.as_deref() == Some(node_id) {
                    return Err(invalid("A folder cannot be moved into itself"));
                }
                if let Some(parent_id) = parent_id.as_deref() {
                    validate_not_descendant_folder(&folders, node_id, parent_id)?;
                }
                validate_unique_sibling_name(
                    &folders,
                    &notes,
                    parent_id.as_deref(),
                    &folder.name,
                    Some((NoteNodeKind::Folder, node_id)),
                )?;
                let changed = folder.parent_id != parent_id || folder.sort_order != sort_order;
                if changed {
                    folder.parent_id = parent_id;
                    folder.sort_order = sort_order;
                    folder.updated_at_ms = current_time_ms();
                    write_note_folder_in_txn(&txn, &folder)?;
                }
                NoteNodeChange {
                    changed,
                    tree_changed: changed,
                    folder: Some(folder),
                    note: None,
                }
            }
            NoteNodeKind::Note => {
                let existing_summary = notes
                    .iter()
                    .find(|item| item.id == node_id)
                    .ok_or_else(|| invalid(format!("Note '{node_id}' does not exist")))?;
                validate_unique_sibling_name(
                    &folders,
                    &notes,
                    parent_id.as_deref(),
                    &existing_summary.title,
                    Some((NoteNodeKind::Note, node_id)),
                )?;
                let mut note = read_note_in_txn(&txn, node_id)?
                    .ok_or_else(|| invalid(format!("Note '{node_id}' does not exist")))?;
                let changed = note.parent_id != parent_id || note.sort_order != sort_order;
                if changed {
                    note.parent_id = parent_id;
                    note.sort_order = sort_order;
                    note.revision = note.revision.saturating_add(1);
                    note.updated_at_ms = current_time_ms();
                    write_note_in_txn(&txn, &note)?;
                    write_note_summary_in_txn(
                        &txn,
                        &summary_from_document(&note, Some(existing_summary)),
                    )?;
                }
                NoteNodeChange {
                    changed,
                    tree_changed: changed,
                    folder: None,
                    note: Some(summary_from_document(&note, Some(existing_summary))),
                }
            }
        };
        txn.commit()?;
        Ok(change)
    }

    pub fn delete_note_node(
        &self,
        node_kind: NoteNodeKind,
        node_id: &str,
    ) -> Result<DeleteNoteNodeResult, StorageError> {
        self.ensure_notes_ready()?;
        let txn = self.db.begin_write()?;
        let folders = read_note_folders_in_txn(&txn)?;
        let notes = read_note_summaries_in_txn(&txn)?;
        let mut folder_ids = HashSet::new();
        let mut note_ids = HashSet::new();

        match node_kind {
            NoteNodeKind::Folder => {
                if !folders.iter().any(|folder| folder.id == node_id) {
                    return Err(invalid(format!("Folder '{node_id}' does not exist")));
                }
                collect_descendant_folder_ids(&folders, node_id, &mut folder_ids);
                folder_ids.insert(node_id.to_string());
                for note in &notes {
                    if note
                        .parent_id
                        .as_ref()
                        .is_some_and(|parent| folder_ids.contains(parent))
                    {
                        note_ids.insert(note.id.clone());
                    }
                }
            }
            NoteNodeKind::Note => {
                if !notes.iter().any(|note| note.id == node_id) {
                    return Err(invalid(format!("Note '{node_id}' does not exist")));
                }
                note_ids.insert(node_id.to_string());
            }
        }

        {
            for id in &folder_ids {
                super::cloud_sync_v4::record_remove(
                    &txn,
                    NOTE_FOLDERS_TABLE,
                    &entity_key(NOTE_FOLDER_PREFIX, id),
                )?;
            }
            for id in &note_ids {
                super::cloud_sync_v4::record_remove(
                    &txn,
                    NOTES_TABLE,
                    &entity_key(NOTE_DOCUMENT_PREFIX, id),
                )?;
            }
            let mut table = txn.open_table(NOTE_FOLDERS_TABLE)?;
            for id in &folder_ids {
                table.remove(entity_key(NOTE_FOLDER_PREFIX, id).as_str())?;
            }
        }
        {
            let mut table = txn.open_table(NOTES_TABLE)?;
            for id in &note_ids {
                table.remove(entity_key(NOTE_DOCUMENT_PREFIX, id).as_str())?;
            }
        }
        {
            let mut table = txn.open_table(NOTE_SUMMARIES_TABLE)?;
            for id in &note_ids {
                table.remove(entity_key(NOTE_SUMMARY_PREFIX, id).as_str())?;
            }
        }
        txn.commit()?;
        let folder_count = folder_ids.len();
        let note_count = note_ids.len();
        let mut ids = folder_ids.into_iter().chain(note_ids).collect::<Vec<_>>();
        ids.sort();
        Ok(DeleteNoteNodeResult {
            folder_count,
            note_count,
            ids,
        })
    }

    pub fn load_notes_snapshot(&self) -> Result<NotesSnapshot, StorageError> {
        self.ensure_notes_ready()?;
        // One read transaction keeps folder references and document revisions consistent.
        let txn = self.db.begin_read()?;
        let folder_table = txn.open_table(NOTE_FOLDERS_TABLE)?;
        let mut folders = Vec::new();
        for item in folder_table.iter()? {
            let (key, value) = item?;
            if key.value().starts_with(NOTE_FOLDER_PREFIX) {
                folders.push(deserialize_json(value.value())?);
            }
        }
        let note_table = txn.open_table(NOTES_TABLE)?;
        let mut notes = Vec::new();
        for item in note_table.iter()? {
            let (key, value) = item?;
            if key.value().starts_with(NOTE_DOCUMENT_PREFIX) {
                notes.push(deserialize_json(value.value())?);
            }
        }
        let extra = note_table
            .get(NOTE_SNAPSHOT_EXTRA_KEY)?
            .map(|value| deserialize_json(value.value()))
            .transpose()?
            .unwrap_or_default();
        sort_note_folders(&mut folders);
        sort_notes(&mut notes);
        Ok(NotesSnapshot {
            folders,
            notes,
            extra,
        })
    }

    pub fn replace_notes_snapshot(&self, snapshot: &NotesSnapshot) -> Result<(), StorageError> {
        validate_notes_snapshot(snapshot)?;
        let txn = self.db.begin_write()?;
        replace_notes_snapshot_in_txn(&txn, snapshot)?;
        txn.open_table(PORTABLE_OPAQUE_ENTITIES_TABLE)?
            .remove("notes")?;
        txn.commit()?;
        Ok(())
    }

    pub fn load_notes_ui_state(&self) -> Result<NotesUiState, StorageError> {
        let value = self.load_settings_value()?;
        let ui = value.get("ui").and_then(Value::as_object);
        let expanded_folder_ids = ui
            .and_then(|ui| ui.get("notes_expanded_folder_ids"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .take(4096)
            .map(str::to_string)
            .collect();
        let last_selected_node_id = ui
            .and_then(|ui| ui.get("notes_last_selected_node_id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        Ok(NotesUiState {
            expanded_folder_ids,
            last_selected_node_id,
        })
    }

    pub fn save_notes_ui_state(&self, state: &NotesUiState) -> Result<NotesUiState, StorageError> {
        let mut value = self.load_settings_value()?;
        let mut expanded = Vec::new();
        let mut seen = HashSet::new();
        for id in state.expanded_folder_ids.iter().take(4096) {
            if !id.trim().is_empty() && seen.insert(id.as_str()) {
                expanded.push(Value::String(id.clone()));
            }
        }
        set_nested_json_value(
            &mut value,
            &["ui", "notes_expanded_folder_ids"],
            Value::Array(expanded),
        );
        set_nested_json_value(
            &mut value,
            &["ui", "notes_last_selected_node_id"],
            state
                .last_selected_node_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| Value::String(value.to_string()))
                .unwrap_or(Value::Null),
        );
        self.save_settings_value(&value)?;
        self.load_notes_ui_state()
    }

    fn ensure_notes_ready(&self) -> Result<(), StorageError> {
        self.migrate_opaque_notes()?;
        self.ensure_note_summary_index()
    }

    fn migrate_opaque_notes(&self) -> Result<(), StorageError> {
        let raw = {
            let txn = self.db.begin_read()?;
            let table = txn.open_table(PORTABLE_OPAQUE_ENTITIES_TABLE)?;
            table.get("notes")?.map(|value| value.value().to_string())
        };
        let Some(raw) = raw else {
            return Ok(());
        };
        let snapshot: NotesSnapshot =
            serde_json::from_str(&raw).map_err(|error| StorageError::PortableSnapshotEntity {
                entity: "notes".to_string(),
                message: error.to_string(),
            })?;
        validate_notes_snapshot(&snapshot).map_err(|error| {
            StorageError::PortableSnapshotEntity {
                entity: "notes".to_string(),
                message: error.to_string(),
            }
        })?;
        let txn = self.db.begin_write()?;
        replace_notes_snapshot_in_txn(&txn, &snapshot)?;
        txn.open_table(PORTABLE_OPAQUE_ENTITIES_TABLE)?
            .remove("notes")?;
        txn.commit()?;
        Ok(())
    }

    fn ensure_note_summary_index(&self) -> Result<(), StorageError> {
        let (version, document_ids, summary_ids) = {
            let txn = self.db.begin_read()?;
            let meta = txn.open_table(META_TABLE)?;
            let version = meta
                .get(META_NOTE_SUMMARY_INDEX_VERSION)?
                .and_then(|raw| raw.value().parse::<u32>().ok())
                .unwrap_or(0);
            let documents = txn.open_table(NOTES_TABLE)?;
            let mut document_ids = HashSet::new();
            for entry in documents.iter()? {
                let (key, _) = entry?;
                if let Some(id) = key.value().strip_prefix(NOTE_DOCUMENT_PREFIX) {
                    document_ids.insert(id.to_string());
                }
            }
            let summaries = txn.open_table(NOTE_SUMMARIES_TABLE)?;
            let mut summary_ids = HashSet::new();
            for entry in summaries.iter()? {
                let (key, _) = entry?;
                if let Some(id) = key.value().strip_prefix(NOTE_SUMMARY_PREFIX) {
                    summary_ids.insert(id.to_string());
                }
            }
            (version, document_ids, summary_ids)
        };
        if version >= NOTE_SUMMARY_INDEX_VERSION && document_ids == summary_ids {
            return Ok(());
        }
        let txn = self.db.begin_write()?;
        let previous_summaries = read_note_summaries_in_txn(&txn).unwrap_or_default();
        clear_prefix_in_txn(&txn, NOTE_SUMMARIES_TABLE, NOTE_SUMMARY_PREFIX)?;
        for note in read_notes_in_txn(&txn)? {
            let previous = previous_summaries
                .iter()
                .find(|summary| summary.id == note.id);
            write_note_summary_in_txn(&txn, &summary_from_document(&note, previous))?;
        }
        let version = NOTE_SUMMARY_INDEX_VERSION.to_string();
        txn.open_table(META_TABLE)?
            .insert(META_NOTE_SUMMARY_INDEX_VERSION, version.as_str())?;
        txn.commit()?;
        Ok(())
    }

    fn list_note_folders_ready(&self) -> Result<Vec<NoteFolder>, StorageError> {
        let mut folders = self.list_json_by_prefix(NOTE_FOLDERS_TABLE, NOTE_FOLDER_PREFIX)?;
        sort_note_folders(&mut folders);
        Ok(folders)
    }

    fn list_notes_ready(&self) -> Result<Vec<NoteDocument>, StorageError> {
        let mut notes = self.list_json_by_prefix(NOTES_TABLE, NOTE_DOCUMENT_PREFIX)?;
        sort_notes(&mut notes);
        Ok(notes)
    }

    fn list_note_summaries_ready(&self) -> Result<Vec<NoteSummary>, StorageError> {
        let mut notes = self.list_json_by_prefix(NOTE_SUMMARIES_TABLE, NOTE_SUMMARY_PREFIX)?;
        sort_note_summaries(&mut notes);
        Ok(notes)
    }
}

pub(super) fn replace_notes_snapshot_in_txn(
    txn: &redb::WriteTransaction,
    snapshot: &NotesSnapshot,
) -> Result<(), StorageError> {
    validate_notes_snapshot(snapshot)?;
    clear_prefix_in_txn(txn, NOTE_FOLDERS_TABLE, NOTE_FOLDER_PREFIX)?;
    clear_prefix_in_txn(txn, NOTES_TABLE, NOTE_DOCUMENT_PREFIX)?;
    clear_prefix_in_txn(txn, NOTE_SUMMARIES_TABLE, NOTE_SUMMARY_PREFIX)?;
    for folder in &snapshot.folders {
        write_note_folder_in_txn(txn, folder)?;
    }
    for note in &snapshot.notes {
        write_note_in_txn(txn, note)?;
        write_note_summary_in_txn(txn, &NoteSummary::from(note))?;
    }
    if snapshot.extra.is_empty() {
        txn.open_table(NOTES_TABLE)?
            .remove(NOTE_SNAPSHOT_EXTRA_KEY)?;
    } else {
        write_json_in_txn(txn, NOTES_TABLE, NOTE_SNAPSHOT_EXTRA_KEY, &snapshot.extra)?;
    }
    let version = NOTE_SUMMARY_INDEX_VERSION.to_string();
    txn.open_table(META_TABLE)?
        .insert(META_NOTE_SUMMARY_INDEX_VERSION, version.as_str())?;
    Ok(())
}

pub(super) fn validate_notes_snapshot(snapshot: &NotesSnapshot) -> Result<(), StorageError> {
    let mut all_ids = HashSet::new();
    let mut folder_ids = HashSet::new();
    for folder in &snapshot.folders {
        normalize_note_name(&folder.name)?;
        if !folder_ids.insert(folder.id.as_str()) || !all_ids.insert(folder.id.as_str()) {
            return Err(invalid(format!("Duplicate note folder id '{}'", folder.id)));
        }
    }
    for note in &snapshot.notes {
        normalize_note_name(&note.title)?;
        if !all_ids.insert(note.id.as_str()) {
            return Err(invalid(format!("Duplicate note id '{}'", note.id)));
        }
    }
    for folder in &snapshot.folders {
        if let Some(parent_id) = folder.parent_id.as_deref() {
            if !folder_ids.contains(parent_id) {
                return Err(invalid(format!(
                    "Note folder '{}' has missing parent '{}'",
                    folder.id, parent_id
                )));
            }
            validate_not_descendant_folder(&snapshot.folders, &folder.id, parent_id)?;
        }
    }
    for note in &snapshot.notes {
        if let Some(parent_id) = note.parent_id.as_deref()
            && !folder_ids.contains(parent_id)
        {
            return Err(invalid(format!(
                "Note '{}' has missing parent '{}'",
                note.id, parent_id
            )));
        }
    }
    for folder in &snapshot.folders {
        validate_unique_sibling_name(
            &snapshot.folders,
            &snapshot.notes,
            folder.parent_id.as_deref(),
            &folder.name,
            Some((NoteNodeKind::Folder, &folder.id)),
        )?;
    }
    for note in &snapshot.notes {
        validate_unique_sibling_name(
            &snapshot.folders,
            &snapshot.notes,
            note.parent_id.as_deref(),
            &note.title,
            Some((NoteNodeKind::Note, &note.id)),
        )?;
    }
    Ok(())
}

fn read_note_folders_in_txn(txn: &redb::WriteTransaction) -> Result<Vec<NoteFolder>, StorageError> {
    let table = txn.open_table(NOTE_FOLDERS_TABLE)?;
    let mut folders = Vec::new();
    for entry in table.iter()? {
        let (key, value) = entry?;
        if key.value().starts_with(NOTE_FOLDER_PREFIX) {
            folders.push(deserialize_json(value.value())?);
        }
    }
    sort_note_folders(&mut folders);
    Ok(folders)
}

fn read_notes_in_txn(txn: &redb::WriteTransaction) -> Result<Vec<NoteDocument>, StorageError> {
    let table = txn.open_table(NOTES_TABLE)?;
    let mut notes = Vec::new();
    for entry in table.iter()? {
        let (key, value) = entry?;
        if key.value().starts_with(NOTE_DOCUMENT_PREFIX) {
            notes.push(deserialize_json(value.value())?);
        }
    }
    sort_notes(&mut notes);
    Ok(notes)
}

fn read_note_summaries_in_txn(
    txn: &redb::WriteTransaction,
) -> Result<Vec<NoteSummary>, StorageError> {
    let table = txn.open_table(NOTE_SUMMARIES_TABLE)?;
    let mut notes = Vec::new();
    for entry in table.iter()? {
        let (key, value) = entry?;
        if key.value().starts_with(NOTE_SUMMARY_PREFIX) {
            notes.push(deserialize_json(value.value())?);
        }
    }
    sort_note_summaries(&mut notes);
    Ok(notes)
}

fn read_note_in_txn(
    txn: &redb::WriteTransaction,
    note_id: &str,
) -> Result<Option<NoteDocument>, StorageError> {
    let table = txn.open_table(NOTES_TABLE)?;
    let key = entity_key(NOTE_DOCUMENT_PREFIX, note_id);
    table
        .get(key.as_str())?
        .map(|value| deserialize_json(value.value()))
        .transpose()
}

fn write_note_folder_in_txn(
    txn: &redb::WriteTransaction,
    folder: &NoteFolder,
) -> Result<(), StorageError> {
    write_json_in_txn(
        txn,
        NOTE_FOLDERS_TABLE,
        &entity_key(NOTE_FOLDER_PREFIX, &folder.id),
        folder,
    )
}

fn write_note_in_txn(
    txn: &redb::WriteTransaction,
    note: &NoteDocument,
) -> Result<(), StorageError> {
    write_json_in_txn(
        txn,
        NOTES_TABLE,
        &entity_key(NOTE_DOCUMENT_PREFIX, &note.id),
        note,
    )
}

fn write_note_summary_in_txn(
    txn: &redb::WriteTransaction,
    note: &NoteSummary,
) -> Result<(), StorageError> {
    write_json_in_txn(
        txn,
        NOTE_SUMMARIES_TABLE,
        &entity_key(NOTE_SUMMARY_PREFIX, &note.id),
        note,
    )
}

fn summary_from_document(note: &NoteDocument, previous: Option<&NoteSummary>) -> NoteSummary {
    let mut summary = NoteSummary::from(note);
    if let Some(previous) = previous {
        summary.extra = previous.extra.clone();
    }
    summary
}

fn normalize_note_name(raw: &str) -> Result<String, StorageError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(invalid("Note name cannot be empty"));
    }
    if value.chars().count() > MAX_NOTE_NAME_CHARS {
        return Err(invalid(format!(
            "Note name cannot exceed {MAX_NOTE_NAME_CHARS} characters"
        )));
    }
    if value.contains('/') || value.contains('\\') {
        return Err(invalid("Note name cannot contain '/' or '\\'"));
    }
    if value.chars().any(char::is_control) {
        return Err(invalid("Note name cannot contain control characters"));
    }
    Ok(value.to_string())
}

fn normalize_or_unique_name(
    raw: Option<String>,
    fallback: &str,
    sibling_names: &HashSet<String>,
) -> Result<String, StorageError> {
    if let Some(raw) = raw {
        let name = normalize_note_name(&raw)?;
        if sibling_names.contains(&name.to_lowercase()) {
            return Err(invalid(format!(
                "A note item named '{name}' already exists in this folder"
            )));
        }
        return Ok(name);
    }
    let base = normalize_note_name(fallback)?;
    if !sibling_names.contains(&base.to_lowercase()) {
        return Ok(base);
    }
    for index in 2..10_000 {
        let candidate = format!("{base} {index}");
        if !sibling_names.contains(&candidate.to_lowercase()) {
            return Ok(candidate);
        }
    }
    Err(invalid("Could not generate a unique note name"))
}

fn sibling_names(
    folders: &[NoteFolder],
    notes: &[impl NoteListItem],
    parent_id: Option<&str>,
    exclude: Option<(NoteNodeKind, &str)>,
) -> HashSet<String> {
    let mut names = HashSet::new();
    for folder in folders {
        if folder.parent_id.as_deref() != parent_id
            || exclude == Some((NoteNodeKind::Folder, folder.id.as_str()))
        {
            continue;
        }
        names.insert(folder.name.to_lowercase());
    }
    for note in notes {
        if note.parent_id() != parent_id || exclude == Some((NoteNodeKind::Note, note.id())) {
            continue;
        }
        names.insert(note.title().to_lowercase());
    }
    names
}

fn validate_unique_sibling_name(
    folders: &[NoteFolder],
    notes: &[impl NoteListItem],
    parent_id: Option<&str>,
    name: &str,
    exclude: Option<(NoteNodeKind, &str)>,
) -> Result<(), StorageError> {
    if sibling_names(folders, notes, parent_id, exclude).contains(&name.to_lowercase()) {
        return Err(invalid(format!(
            "A note item named '{name}' already exists in this folder"
        )));
    }
    Ok(())
}

fn validate_parent_exists(
    folders: &[NoteFolder],
    parent_id: Option<&str>,
) -> Result<(), StorageError> {
    if let Some(parent_id) = parent_id
        && !folders.iter().any(|folder| folder.id == parent_id)
    {
        return Err(invalid(format!("Folder '{parent_id}' does not exist")));
    }
    Ok(())
}

fn validate_not_descendant_folder(
    folders: &[NoteFolder],
    source_id: &str,
    target_parent_id: &str,
) -> Result<(), StorageError> {
    let by_id: HashMap<&str, &NoteFolder> = folders
        .iter()
        .map(|folder| (folder.id.as_str(), folder))
        .collect();
    let mut current = Some(target_parent_id);
    let mut visited = HashSet::new();
    while let Some(folder_id) = current {
        if folder_id == source_id {
            return Err(invalid("A folder cannot be moved into its descendant"));
        }
        if !visited.insert(folder_id) {
            return Err(invalid("Folder hierarchy contains a cycle"));
        }
        current = by_id
            .get(folder_id)
            .and_then(|folder| folder.parent_id.as_deref());
    }
    Ok(())
}

fn collect_descendant_folder_ids(
    folders: &[NoteFolder],
    parent_id: &str,
    collected: &mut HashSet<String>,
) {
    for folder in folders {
        if folder.parent_id.as_deref() == Some(parent_id) && collected.insert(folder.id.clone()) {
            collect_descendant_folder_ids(folders, &folder.id, collected);
        }
    }
}

fn next_sort_order_for_parent(
    folders: &[NoteFolder],
    notes: &[impl NoteListItem],
    parent_id: Option<&str>,
) -> i64 {
    folders
        .iter()
        .filter(|folder| folder.parent_id.as_deref() == parent_id)
        .map(|folder| folder.sort_order)
        .chain(
            notes
                .iter()
                .filter(|note| note.parent_id() == parent_id)
                .map(NoteListItem::sort_order),
        )
        .max()
        .unwrap_or(-1)
        .saturating_add(1)
}

fn sort_note_folders(folders: &mut [NoteFolder]) {
    folders.sort_by(|left, right| {
        left.parent_id
            .cmp(&right.parent_id)
            .then(left.sort_order.cmp(&right.sort_order))
            .then(left.name.cmp(&right.name))
            .then(left.id.cmp(&right.id))
    });
}

fn sort_notes(notes: &mut [NoteDocument]) {
    notes.sort_by(|left, right| {
        left.parent_id
            .cmp(&right.parent_id)
            .then(left.sort_order.cmp(&right.sort_order))
            .then(left.title.cmp(&right.title))
            .then(left.id.cmp(&right.id))
    });
}

fn sort_note_summaries(notes: &mut [NoteSummary]) {
    notes.sort_by(|left, right| {
        left.parent_id
            .cmp(&right.parent_id)
            .then(left.sort_order.cmp(&right.sort_order))
            .then(left.title.cmp(&right.title))
            .then(left.id.cmp(&right.id))
    });
}

fn invalid(message: impl Into<String>) -> StorageError {
    StorageError::InvalidData(message.into())
}

#[cfg(test)]
mod tests {
    use std::{fs, ops::Deref, path::Path};

    use nyaterm_core::test_support::TestTempDir;
    use redb::ReadableDatabase;

    use super::{
        ConnectionStore, META_NOTE_SUMMARY_INDEX_VERSION, META_TABLE, NOTE_DOCUMENT_PREFIX,
        NOTE_FOLDERS_TABLE, NOTE_SUMMARIES_TABLE, NOTE_SUMMARY_PREFIX, NOTES_TABLE, NoteDocument,
        NoteNodeKind, NotesSnapshot, NotesUiState, PORTABLE_OPAQUE_ENTITIES_TABLE, entity_key,
    };

    struct TempStore {
        store: Option<ConnectionStore>,
        dir: TestTempDir,
    }

    impl TempStore {
        fn new() -> Self {
            let dir = TestTempDir::new("nyaterm-notes-test");
            fs::create_dir_all(dir.path()).expect("create temp directory");
            let store = ConnectionStore::open(&dir).expect("open store");
            Self {
                store: Some(store),
                dir,
            }
        }

        fn path(&self) -> &Path {
            self.dir.path()
        }
    }

    impl Deref for TempStore {
        type Target = ConnectionStore;

        fn deref(&self) -> &Self::Target {
            self.store.as_ref().expect("temporary store is open")
        }
    }

    impl Drop for TempStore {
        fn drop(&mut self) {
            drop(self.store.take());
        }
    }

    fn temp_store() -> TempStore {
        TempStore::new()
    }

    #[test]
    fn temporary_store_removes_database_directory_on_drop() {
        let dir = {
            let store = temp_store();
            let dir = store.path().to_path_buf();
            assert!(dir.join(super::super::DATABASE_FILE).is_file());
            dir
        };

        assert!(!dir.exists());
    }

    #[test]
    fn creates_updates_moves_and_recursively_deletes_notes() {
        let store = temp_store();
        let root = store
            .create_note_folder(None, Some("Root".into()))
            .expect("root");
        let child = store
            .create_note_folder(Some(root.id.clone()), Some("Child".into()))
            .expect("child");
        let note = store
            .create_note(
                Some(child.id.clone()),
                Some("Runbook".into()),
                Some("one".into()),
            )
            .expect("note");
        let updated = store
            .update_note(&note.id, "Runbook".into(), "two".into(), 1, false)
            .expect("update");
        assert_eq!(updated.note.revision, 2);
        assert!(
            store
                .update_note(&note.id, "Runbook".into(), "stale".into(), 1, false)
                .expect_err("conflict")
                .to_string()
                .contains("Revision conflict")
        );
        let forced = store
            .update_note(&note.id, "Runbook".into(), "forced".into(), 1, true)
            .expect("force overwrite stale revision");
        assert_eq!(forced.note.revision, 3);
        assert_eq!(forced.note.markdown, "forced");
        assert!(
            store
                .move_note_node(NoteNodeKind::Folder, &root.id, Some(child.id.clone()), 9,)
                .expect_err("cycle")
                .to_string()
                .contains("descendant")
        );

        let deleted = store
            .delete_note_node(NoteNodeKind::Folder, &root.id)
            .expect("delete tree");
        assert_eq!((deleted.folder_count, deleted.note_count), (2, 1));
        assert!(store.list_notes().expect("notes").is_empty());
    }

    #[test]
    fn rejects_duplicate_names_across_folders_and_notes() {
        let store = temp_store();
        store
            .create_note(None, Some("Readme".into()), None)
            .expect("note");
        let error = store
            .create_note_folder(None, Some("README".into()))
            .expect_err("duplicate");
        assert!(error.to_string().contains("already exists"));
    }

    #[test]
    fn rejects_invalid_names_and_missing_parents_without_writing() {
        let store = temp_store();
        for invalid_name in ["   ", "bad/name", "bad\\name", "bad\nname"] {
            assert!(
                store
                    .create_note(None, Some(invalid_name.into()), None)
                    .is_err(),
                "name should be rejected: {invalid_name:?}"
            );
        }
        assert!(
            store
                .create_note(None, Some("x".repeat(121)), None)
                .is_err()
        );
        assert!(
            store
                .create_note(Some("missing".into()), Some("Note".into()), None)
                .is_err()
        );
        assert!(
            store
                .list_notes()
                .expect("notes remain readable")
                .is_empty()
        );
    }

    #[test]
    fn snapshot_round_trip_preserves_unknown_fields() {
        let store = temp_store();
        let snapshot: NotesSnapshot = serde_json::from_value(serde_json::json!({
            "folders": [],
            "notes": [{
                "id": "note-1", "parent_id": null, "title": "Note",
                "markdown": "body", "sort_order": 0, "revision": 1,
                "created_at_ms": 1, "updated_at_ms": 2, "future": true
            }],
            "future_root": 7
        }))
        .expect("snapshot fixture");
        store
            .replace_notes_snapshot(&snapshot)
            .expect("replace snapshot");
        assert_eq!(
            store.load_notes_snapshot().expect("load snapshot"),
            snapshot
        );
    }

    #[test]
    fn opaque_notes_take_precedence_and_are_promoted_atomically() {
        let store = temp_store();
        store
            .create_note(None, Some("Old".into()), None)
            .expect("old typed note");
        let replacement = serde_json::json!({
            "folders": [],
            "notes": [{
                "id": "new", "parent_id": null, "title": "Imported",
                "markdown": "new", "sort_order": 0, "revision": 1,
                "created_at_ms": 1, "updated_at_ms": 1
            }]
        })
        .to_string();
        let txn = store.db.begin_write().expect("transaction");
        txn.open_table(PORTABLE_OPAQUE_ENTITIES_TABLE)
            .expect("opaque table")
            .insert("notes", replacement.as_str())
            .expect("insert opaque notes");
        txn.commit().expect("commit opaque notes");

        let notes = store.list_notes().expect("promote notes");
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "Imported");
        let txn = store.db.begin_read().expect("read transaction");
        assert!(
            txn.open_table(PORTABLE_OPAQUE_ENTITIES_TABLE)
                .expect("opaque table")
                .get("notes")
                .expect("read opaque")
                .is_none()
        );
    }

    #[test]
    fn invalid_opaque_notes_are_reported_without_replacing_typed_data() {
        let store = temp_store();
        let existing = store
            .create_note(None, Some("Existing".into()), Some("safe".into()))
            .expect("existing note");
        let invalid = r#"{"folders":[],"notes":[{"id":"bad","parent_id":"missing","title":"Bad","markdown":"bad","sort_order":0,"revision":1,"created_at_ms":1,"updated_at_ms":1}]}"#;
        let txn = store.db.begin_write().expect("transaction");
        txn.open_table(PORTABLE_OPAQUE_ENTITIES_TABLE)
            .expect("opaque table")
            .insert("notes", invalid)
            .expect("insert opaque notes");
        txn.commit().expect("commit opaque notes");

        assert!(
            store
                .list_notes()
                .expect_err("invalid opaque notes")
                .to_string()
                .contains("missing parent")
        );
        let txn = store.db.begin_read().expect("read transaction");
        assert!(
            txn.open_table(PORTABLE_OPAQUE_ENTITIES_TABLE)
                .expect("opaque table")
                .get("notes")
                .expect("read opaque")
                .is_some()
        );
        let raw = txn
            .open_table(NOTES_TABLE)
            .expect("notes table")
            .get(entity_key(NOTE_DOCUMENT_PREFIX, &existing.id).as_str())
            .expect("read typed note")
            .expect("typed note remains")
            .value()
            .to_vec();
        let persisted: NoteDocument = serde_json::from_slice(&raw).expect("typed note JSON");
        assert_eq!(persisted.markdown, "safe");
    }

    #[test]
    fn invalid_snapshot_does_not_replace_existing_notes() {
        let store = temp_store();
        let existing = store
            .create_note(None, Some("Existing".into()), Some("safe".into()))
            .expect("existing note");
        let invalid: NotesSnapshot = serde_json::from_value(serde_json::json!({
            "folders": [],
            "notes": [{
                "id": "bad", "parent_id": "missing", "title": "Bad",
                "markdown": "bad", "sort_order": 0, "revision": 1,
                "created_at_ms": 1, "updated_at_ms": 1
            }]
        }))
        .expect("invalid snapshot fixture");
        assert!(store.replace_notes_snapshot(&invalid).is_err());
        assert_eq!(
            store.get_note(&existing.id).expect("load existing"),
            Some(existing)
        );
    }

    #[test]
    fn reads_tauri_redb_records_and_rebuilds_missing_summary_index() {
        let store = temp_store();
        let folder = r#"{"id":"folder-1","parent_id":null,"name":"Tauri","sort_order":0,"created_at_ms":10,"updated_at_ms":11}"#;
        let note = r##"{"id":"note-1","parent_id":"folder-1","title":"Imported","markdown":"# Tauri","sort_order":1,"revision":4,"created_at_ms":12,"updated_at_ms":13}"##;
        let txn = store.db.begin_write().expect("transaction");
        txn.open_table(NOTE_FOLDERS_TABLE)
            .expect("folder table")
            .insert("note_folders/folder-1", folder.as_bytes())
            .expect("insert Tauri folder");
        txn.open_table(NOTES_TABLE)
            .expect("notes table")
            .insert("notes/note-1", note.as_bytes())
            .expect("insert Tauri note");
        txn.open_table(META_TABLE)
            .expect("meta table")
            .remove(META_NOTE_SUMMARY_INDEX_VERSION)
            .expect("remove index version");
        txn.commit().expect("commit Tauri fixture");

        let tree = store.list_note_tree().expect("read Tauri notes");
        assert_eq!(tree.folders[0].name, "Tauri");
        assert_eq!(tree.notes[0].title, "Imported");
        assert_eq!(tree.notes[0].revision, 4);
        let txn = store.db.begin_read().expect("read rebuilt index");
        assert!(
            txn.open_table(NOTE_SUMMARIES_TABLE)
                .expect("summary table")
                .get("note_summaries/note-1")
                .expect("read summary")
                .is_some()
        );
    }

    #[test]
    fn rebuilds_a_missing_summary_even_when_index_version_is_current() {
        let store = temp_store();
        let note = store
            .create_note(None, Some("Indexed".into()), Some("body".into()))
            .expect("create note");
        let txn = store.db.begin_write().expect("transaction");
        txn.open_table(NOTE_SUMMARIES_TABLE)
            .expect("summary table")
            .remove(entity_key(NOTE_SUMMARY_PREFIX, &note.id).as_str())
            .expect("remove one summary");
        txn.commit().expect("commit missing summary");

        let summaries = store
            .list_note_summaries()
            .expect("rebuild missing summary");
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, note.id);
    }

    #[test]
    fn notes_ui_state_round_trip_deduplicates_and_trims_values() {
        let store = temp_store();
        let saved = store
            .save_notes_ui_state(&NotesUiState {
                expanded_folder_ids: vec!["a".into(), "a".into(), "b".into()],
                last_selected_node_id: Some("  note-1  ".into()),
            })
            .expect("save notes UI state");
        assert_eq!(saved.expanded_folder_ids, ["a", "b"]);
        assert_eq!(saved.last_selected_node_id.as_deref(), Some("note-1"));
        assert_eq!(store.load_notes_ui_state().expect("load state"), saved);
    }
}
