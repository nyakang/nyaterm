use crate::config::{
    DeleteNoteNodeResult, NoteDocument, NoteFolder, NoteSummary, NoteTreePayload, NotesChangedEvent,
};
use crate::error::{AppError, AppResult};
use tauri::Emitter;

#[tauri::command]
pub async fn export_notes(
    destination: String,
) -> AppResult<crate::core::note_export::NoteExportResult> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::core::note_export::export_notes(std::path::Path::new(&destination))
    })
    .await
    .map_err(|error| AppError::Config(format!("Notes export task failed: {error}")))?
}

fn schedule_cloud_sync_notify(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        crate::core::cloud_sync::notify_config_changed(&app).await;
    });
}

fn emit_notes_changed(
    app: &tauri::AppHandle,
    kind: &str,
    node_kind: Option<&str>,
    ids: Vec<String>,
    folders: Vec<NoteFolder>,
    notes: Vec<NoteSummary>,
    tree_changed: Option<bool>,
) {
    let payload = NotesChangedEvent {
        kind: kind.to_string(),
        node_kind: node_kind.map(str::to_string),
        ids,
        folders,
        notes,
        tree_changed,
    };
    let _ = app.emit("notes-changed", payload);
}

#[tauri::command]
pub fn list_note_tree() -> AppResult<NoteTreePayload> {
    let folders = crate::storage::list_note_folders()?;
    let notes = crate::storage::list_note_summaries()?;
    Ok(NoteTreePayload { folders, notes })
}

#[tauri::command]
pub fn get_note(note_id: String) -> AppResult<NoteDocument> {
    crate::storage::get_note(&note_id)?
        .ok_or_else(|| AppError::Config(format!("Note '{note_id}' does not exist")))
}

#[tauri::command]
pub fn create_note_folder(
    app: tauri::AppHandle,
    parent_id: Option<String>,
    name: Option<String>,
) -> AppResult<crate::config::NoteFolder> {
    let folder = crate::storage::create_note_folder(parent_id, name)?;
    emit_notes_changed(
        &app,
        "created",
        Some("folder"),
        vec![folder.id.clone()],
        vec![folder.clone()],
        Vec::new(),
        Some(true),
    );
    schedule_cloud_sync_notify(app);
    Ok(folder)
}

#[tauri::command]
pub fn create_note(
    app: tauri::AppHandle,
    parent_id: Option<String>,
    title: Option<String>,
    markdown: Option<String>,
) -> AppResult<NoteDocument> {
    let note = crate::storage::create_note(parent_id, title, markdown)?;
    emit_notes_changed(
        &app,
        "created",
        Some("note"),
        vec![note.id.clone()],
        Vec::new(),
        vec![NoteSummary::from(note.clone())],
        Some(true),
    );
    schedule_cloud_sync_notify(app);
    Ok(note)
}

#[tauri::command]
pub fn update_note(
    app: tauri::AppHandle,
    note_id: String,
    title: String,
    markdown: String,
    expected_revision: u64,
    force: Option<bool>,
) -> AppResult<NoteDocument> {
    let result = crate::storage::update_note(
        &note_id,
        title,
        markdown,
        expected_revision,
        force.unwrap_or(false),
    )?;
    if result.changed {
        emit_notes_changed(
            &app,
            "updated",
            Some("note"),
            vec![result.note.id.clone()],
            Vec::new(),
            vec![NoteSummary::from(result.note.clone())],
            Some(result.tree_changed),
        );
        schedule_cloud_sync_notify(app);
    }
    Ok(result.note)
}

#[tauri::command]
pub fn rename_note_node(
    app: tauri::AppHandle,
    node_kind: String,
    node_id: String,
    name: String,
) -> AppResult<()> {
    let result = crate::storage::rename_note_node(&node_kind, &node_id, name)?;
    if result.changed {
        emit_notes_changed(
            &app,
            "renamed",
            Some(&node_kind),
            vec![node_id],
            result.folder.into_iter().collect(),
            result.note.into_iter().collect(),
            Some(result.tree_changed),
        );
        schedule_cloud_sync_notify(app);
    }
    Ok(())
}

#[tauri::command]
pub fn move_note_node(
    app: tauri::AppHandle,
    node_kind: String,
    node_id: String,
    parent_id: Option<String>,
    sort_order: i64,
) -> AppResult<()> {
    let result = crate::storage::move_note_node(&node_kind, &node_id, parent_id, sort_order)?;
    if result.changed {
        emit_notes_changed(
            &app,
            "moved",
            Some(&node_kind),
            vec![node_id],
            result.folder.into_iter().collect(),
            result.note.into_iter().collect(),
            Some(result.tree_changed),
        );
        schedule_cloud_sync_notify(app);
    }
    Ok(())
}

#[tauri::command]
pub fn delete_note_node(
    app: tauri::AppHandle,
    node_kind: String,
    node_id: String,
) -> AppResult<DeleteNoteNodeResult> {
    let result = crate::storage::delete_note_node(&node_kind, &node_id)?;
    if result.folder_count > 0 || result.note_count > 0 {
        emit_notes_changed(
            &app,
            "deleted",
            Some(&node_kind),
            result.ids.clone(),
            Vec::new(),
            Vec::new(),
            Some(true),
        );
        schedule_cloud_sync_notify(app);
    }
    Ok(result)
}
