//! Built-in remote-editor workspace, tab lifecycle and window tracking.

use crate::models::{TransferEditorState, TransferEditorWorkspaceState};
use gpui::FocusHandle;
#[cfg(test)]
use nyaterm_files::remote_file::RemoteTextMetadata;
#[cfg(test)]
use nyaterm_files::remote_file::RemoteTextRevision;
use nyaterm_files::remote_file::{RemoteTextDocument, RemoteTextGeneration, RemoteTextWriteResult};
#[cfg(test)]
use nyaterm_files::sftp::SftpWriteTextResult;
use nyaterm_ui::document_snapshot::NyaDocumentSnapshot;
use nyaterm_ui::{ChildWindowSlot, NyaWindowHandle};

#[cfg(test)]
use super::{
    TransferEditorCloseAfterSave, TransferEditorCloseOutcome, TransferEditorDiscardOutcome,
};

use super::{TransferEditorFeatureState, TransferEditorSaveOutcome, TransferFeatureState};

impl TransferFeatureState {
    pub(in crate::features) fn window_editor_workspace(
        &self,
    ) -> Option<TransferEditorWorkspaceState> {
        let mut workspace = self.editor.workspace.clone()?;
        workspace
            .tabs
            .retain(|tab| tab.display == nyaterm_core::InternalEditorDisplay::Window);
        if workspace.tabs.is_empty() {
            return None;
        }
        if !workspace
            .tabs
            .iter()
            .any(|tab| tab.id == workspace.active_tab_id)
        {
            workspace.active_tab_id = workspace.tabs[0].id.clone();
        }
        Some(workspace)
    }

    pub(in crate::features) fn begin_editor_document_reload(&mut self, id: &str) {
        if let Some(tab) = self
            .editor
            .workspace
            .as_mut()
            .and_then(|workspace| workspace.tabs.iter_mut().find(|tab| tab.id == id))
        {
            tab.generation = nyaterm_files::remote_file::RemoteTextGeneration::next();
            tab.loading = true;
            tab.reload_confirm = false;
            tab.error = None;
        }
    }

    pub(in crate::features) fn editor_documents(&self) -> &[TransferEditorState] {
        self.editor
            .workspace
            .as_ref()
            .map(|workspace| workspace.tabs.as_slice())
            .unwrap_or_default()
    }

    pub(in crate::features) fn editor_references_session(&self, session: &str) -> bool {
        self.editor_documents()
            .iter()
            .any(|tab| tab.session_id.as_deref() == Some(session))
    }

    pub(in crate::features) fn has_window_editor_documents(&self) -> bool {
        self.editor_documents()
            .iter()
            .any(|tab| tab.display == nyaterm_core::InternalEditorDisplay::Window)
    }

    pub(in crate::features) fn remove_editor_document(&mut self, id: &str) {
        if let Some(workspace) = self.editor.workspace.as_mut() {
            // Window navigation only considers its own documents, regardless of
            // where workspace files are interleaved in the document collection.
            let next_window = if workspace.active_tab_id == id {
                let tabs = workspace
                    .tabs
                    .iter()
                    .filter(|doc| doc.display == nyaterm_core::InternalEditorDisplay::Window)
                    .collect::<Vec<_>>();
                tabs.iter().position(|doc| doc.id == id).and_then(|index| {
                    let source = &tabs[index].session_id;
                    tabs[..index]
                        .iter()
                        .rev()
                        .chain(tabs[index + 1..].iter())
                        .find(|doc| &doc.session_id == source)
                        .or_else(|| tabs.get(index.saturating_sub(1)).filter(|doc| doc.id != id))
                        .or_else(|| tabs.get(index + 1))
                        .map(|doc| doc.id.clone())
                })
            } else {
                None
            };
            workspace.remove_tab(id);
            if let Some(next) = next_window {
                workspace.active_tab_id = next;
            }
            Self::clear_editor_close_state(workspace);
            if workspace.tabs.is_empty() {
                self.editor.workspace = None;
            }
        }
        self.editor.views.remove(id);
    }

    pub(in crate::features) fn workspace_editor_view(
        &self,
        id: &str,
    ) -> Option<gpui::Entity<super::super::workspace_editor::WorkspaceEditorView>> {
        self.editor.views.get(id).cloned()
    }

    pub(in crate::features) fn set_workspace_editor_view(
        &mut self,
        id: String,
        view: gpui::Entity<super::super::workspace_editor::WorkspaceEditorView>,
    ) {
        self.editor.views.insert(id, view);
    }

    pub(in crate::features) fn editor_workspace(&self) -> Option<&TransferEditorWorkspaceState> {
        self.editor.workspace.as_ref()
    }

    #[cfg(test)]
    pub(in crate::features) fn editor_has_workspace(&self) -> bool {
        self.editor.workspace.is_some()
    }

    pub(in crate::features) fn active_editor_tab(&self) -> Option<&TransferEditorState> {
        let workspace = self.editor.workspace.as_ref()?;
        workspace
            .tabs
            .iter()
            .find(|tab| {
                tab.id == workspace.active_tab_id
                    && tab.display == nyaterm_core::InternalEditorDisplay::Window
            })
            .or_else(|| {
                workspace
                    .tabs
                    .iter()
                    .find(|tab| tab.display == nyaterm_core::InternalEditorDisplay::Window)
            })
    }

    #[cfg(test)]
    pub(in crate::features) fn active_editor_tab_mut(
        &mut self,
    ) -> Option<&mut TransferEditorState> {
        let id = self.active_editor_tab()?.id.clone();
        self.editor
            .workspace
            .as_mut()?
            .tabs
            .iter_mut()
            .find(|tab| tab.id == id)
    }

    pub(in crate::features) fn editor_tab_snapshot(
        &self,
        tab_id: &str,
    ) -> Option<TransferEditorState> {
        self.editor
            .workspace
            .as_ref()?
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .cloned()
    }

    pub(in crate::features) fn open_editor_tab(&mut self, tab: TransferEditorState) -> bool {
        let tab_id = tab.id.clone();
        let window_document = tab.display == nyaterm_core::InternalEditorDisplay::Window;
        if let Some(workspace) = self.editor.workspace.as_mut() {
            let already_open = workspace.tabs.iter().any(|current| current.id == tab_id);
            if !already_open {
                let insert_after = workspace
                    .tabs
                    .iter()
                    .position(|current| {
                        current.id == workspace.active_tab_id
                            && current.session_id == tab.session_id
                    })
                    .or_else(|| {
                        workspace
                            .tabs
                            .iter()
                            .rposition(|current| current.session_id == tab.session_id)
                    });
                let index = insert_after.map_or(workspace.tabs.len(), |index| index + 1);
                workspace.tabs.insert(index, tab);
            }
            if window_document {
                workspace.active_tab_id = tab_id;
            }
            Self::clear_editor_close_state(workspace);
            already_open
        } else {
            self.editor.workspace = Some(TransferEditorWorkspaceState::new(tab));
            false
        }
    }

    pub(in crate::features) fn activate_editor_tab(&mut self, tab_id: &str) -> bool {
        let Some(workspace) = self.editor.workspace.as_mut() else {
            return false;
        };
        if !workspace.tabs.iter().any(|tab| tab.id == tab_id) {
            return false;
        }
        workspace.active_tab_id = tab_id.to_string();
        Self::clear_editor_close_state(workspace);
        true
    }

    #[cfg(test)]
    pub(in crate::features) fn request_editor_tab_close(
        &mut self,
        tab_id: &str,
    ) -> TransferEditorCloseOutcome {
        let Some(workspace) = self.editor.workspace.as_mut() else {
            return TransferEditorCloseOutcome::Missing;
        };
        let Some(tab) = workspace.tabs.iter().find(|tab| tab.id == tab_id) else {
            return TransferEditorCloseOutcome::Missing;
        };
        if tab.dirty || tab.saving {
            workspace.active_tab_id = tab_id.to_string();
            workspace.close_confirm = true;
            workspace.pending_close_tab_id = Some(tab_id.to_string());
            workspace.close_after_save_all = false;
            return TransferEditorCloseOutcome::ConfirmationRequired;
        }
        workspace.remove_tab(tab_id);
        if workspace.tabs.is_empty() {
            self.editor.workspace = None;
            self.editor.window.cancel_open();
        }
        TransferEditorCloseOutcome::Closed
    }

    #[cfg(test)]
    pub(in crate::features) fn request_editor_close(&mut self) -> TransferEditorCloseOutcome {
        let Some(workspace) = self.editor.workspace.as_mut() else {
            return TransferEditorCloseOutcome::Missing;
        };
        if let Some(dirty_tab_id) = workspace
            .tabs
            .iter()
            .find(|tab| tab.dirty || tab.saving)
            .map(|tab| tab.id.clone())
        {
            workspace.active_tab_id = dirty_tab_id;
            workspace.close_confirm = true;
            workspace.pending_close_tab_id = None;
            workspace.close_after_save_all = false;
            return TransferEditorCloseOutcome::ConfirmationRequired;
        }
        self.editor.workspace = None;
        self.editor.window.cancel_open();
        TransferEditorCloseOutcome::Closed
    }

    #[cfg(test)]
    pub(in crate::features) fn discard_editor(&mut self) -> TransferEditorDiscardOutcome {
        let Some(workspace) = self.editor.workspace.as_mut() else {
            return TransferEditorDiscardOutcome::Missing;
        };
        if let Some(tab_id) = workspace.pending_close_tab_id.clone() {
            if workspace
                .tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .is_some_and(|tab| tab.saving)
            {
                return TransferEditorDiscardOutcome::Missing;
            }
            workspace.remove_tab(&tab_id);
            Self::clear_editor_close_state(workspace);
            if workspace.tabs.is_empty() {
                self.editor.workspace = None;
                self.editor.window.cancel_open();
            }
            TransferEditorDiscardOutcome::TabDiscarded
        } else {
            if workspace.tabs.iter().any(|tab| tab.saving) {
                return TransferEditorDiscardOutcome::Missing;
            }
            self.editor.workspace = None;
            self.editor.window.cancel_open();
            TransferEditorDiscardOutcome::WorkspaceDiscarded
        }
    }

    #[cfg(test)]
    pub(in crate::features) fn cancel_editor_close(&mut self) -> bool {
        let Some(workspace) = self.editor.workspace.as_mut() else {
            return false;
        };
        Self::clear_editor_close_state(workspace);
        for tab in &mut workspace.tabs {
            tab.close_after_save = false;
        }
        true
    }

    #[cfg(test)]
    pub(in crate::features) fn editor_close_confirmation_is_open(&self) -> bool {
        self.editor
            .workspace
            .as_ref()
            .is_some_and(|workspace| workspace.close_confirm)
    }

    pub(in crate::features) fn dirty_editor_tab_ids(&self) -> Vec<String> {
        self.editor
            .workspace
            .as_ref()
            .map(|workspace| {
                workspace
                    .tabs
                    .iter()
                    .filter(|tab| tab.dirty && !tab.loading && !tab.saving)
                    .map(|tab| tab.id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(in crate::features) fn fail_editor_load_tab(
        &mut self,
        tab_id: &str,
        generation: RemoteTextGeneration,
        error: String,
    ) -> bool {
        let Some(tab) = self
            .editor
            .workspace
            .as_mut()
            .and_then(|workspace| workspace.tabs.iter_mut().find(|tab| tab.id == tab_id))
        else {
            return false;
        };
        if tab.generation != generation {
            return false;
        }
        tab.loading = false;
        tab.error = Some(error);
        true
    }

    pub(in crate::features) fn begin_editor_tab_save(&mut self, tab_id: &str) -> bool {
        let Some(tab) = self
            .editor
            .workspace
            .as_mut()
            .and_then(|workspace| workspace.tabs.iter_mut().find(|tab| tab.id == tab_id))
        else {
            return false;
        };
        if tab.loading || tab.saving {
            return false;
        }
        tab.saving = true;
        tab.error = None;
        tab.backup_warning_path = None;
        tab.conflict = false;
        tab.reload_confirm = false;
        true
    }

    #[cfg(test)]
    pub(in crate::features) fn prepare_editor_close_after_save(
        &mut self,
    ) -> TransferEditorCloseAfterSave {
        let Some(workspace) = self.editor.workspace.as_mut() else {
            return TransferEditorCloseAfterSave::Missing;
        };
        if let Some(tab_id) = workspace.pending_close_tab_id.clone() {
            let Some(tab) = workspace.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
                return TransferEditorCloseAfterSave::Missing;
            };
            if tab.loading {
                return TransferEditorCloseAfterSave::Loading;
            }
            tab.close_after_save = true;
            if tab.saving {
                TransferEditorCloseAfterSave::Saving
            } else {
                TransferEditorCloseAfterSave::Ready(tab_id)
            }
        } else {
            workspace.close_after_save_all = true;
            workspace.close_confirm = false;
            TransferEditorCloseAfterSave::All
        }
    }

    pub(in crate::features) fn complete_editor_load_tab(
        &mut self,
        tab_id: &str,
        generation: RemoteTextGeneration,
        file: RemoteTextDocument,
    ) -> bool {
        let Some(tab) = self
            .editor
            .workspace
            .as_mut()
            .and_then(|workspace| workspace.tabs.iter_mut().find(|tab| tab.id == tab_id))
        else {
            return false;
        };
        if tab.generation != generation {
            return false;
        }
        tab.content = file.content.into();
        tab.revision = Some(file.revision);
        tab.loading = false;
        tab.saving = false;
        tab.dirty = false;
        tab.conflict = false;
        tab.close_after_save = false;
        tab.reload_confirm = false;
        tab.error = None;
        tab.backup_warning_path = None;
        true
    }

    #[cfg(test)]
    pub(in crate::features) fn complete_editor_save(
        &mut self,
        session_id: Option<&str>,
        remote_path: &str,
        result: SftpWriteTextResult,
    ) -> Option<TransferEditorSaveOutcome> {
        let tab = self.editor.workspace.as_ref()?.tabs.iter().find(|tab| {
            tab.session_id.as_deref() == session_id && tab.remote_path == remote_path
        })?;
        let tab_id = tab.id.clone();
        let generation = tab.generation;
        let content = tab.content.clone();
        let result = match result {
            SftpWriteTextResult::Saved { modified_at, size } => RemoteTextWriteResult::Saved {
                revision: RemoteTextRevision::from_bytes(
                    content.as_bytes(),
                    RemoteTextMetadata {
                        size,
                        modified_at: Some(modified_at),
                    },
                ),
            },
            SftpWriteTextResult::Conflict { .. } => RemoteTextWriteResult::Conflict,
        };
        self.complete_editor_save_tab(&tab_id, generation, result)
    }

    pub(in crate::features) fn complete_editor_save_tab(
        &mut self,
        tab_id: &str,
        generation: RemoteTextGeneration,
        result: RemoteTextWriteResult,
    ) -> Option<TransferEditorSaveOutcome> {
        let workspace = self.editor.workspace.as_mut()?;
        let tab = workspace.tabs.iter_mut().find(|tab| tab.id == tab_id)?;
        if tab.generation != generation {
            return None;
        }
        let mut remove_tab_id = None;
        let outcome = match result {
            RemoteTextWriteResult::Saved { revision } => {
                if tab.close_after_save {
                    remove_tab_id = Some(tab.id.clone());
                }
                tab.revision = Some(revision);
                tab.saving = false;
                tab.dirty = false;
                tab.conflict = false;
                tab.close_after_save = false;
                tab.reload_confirm = false;
                tab.error = None;
                tab.backup_warning_path = None;
                TransferEditorSaveOutcome::Saved
            }
            RemoteTextWriteResult::SavedWithBackup {
                revision,
                backup_path,
            } => {
                tab.revision = Some(revision);
                tab.saving = false;
                tab.dirty = false;
                tab.conflict = false;
                tab.close_after_save = false;
                tab.reload_confirm = false;
                tab.error = None;
                tab.backup_warning_path = Some(backup_path.clone());
                workspace.close_after_save_all = false;
                workspace.close_confirm = false;
                workspace.pending_close_tab_id = None;
                TransferEditorSaveOutcome::SavedWithWarning(backup_path)
            }
            RemoteTextWriteResult::Conflict => {
                tab.saving = false;
                tab.conflict = true;
                tab.close_after_save = false;
                tab.error = Some("Remote file changed before save.".to_string());
                workspace.close_after_save_all = false;
                workspace.close_confirm = true;
                TransferEditorSaveOutcome::Conflict
            }
        };
        if let Some(tab_id) = remove_tab_id.as_deref() {
            workspace.remove_tab(tab_id);
            workspace.pending_close_tab_id = None;
            workspace.close_confirm = false;
        }
        let close_workspace = workspace.tabs.is_empty()
            || (workspace.close_after_save_all
                && workspace.tabs.iter().all(|tab| !tab.dirty && !tab.saving));
        if close_workspace {
            self.editor.workspace = None;
            self.editor.window.cancel_open();
            Some(TransferEditorSaveOutcome::SavedAndClosed)
        } else {
            Some(outcome)
        }
    }

    pub(in crate::features) fn fail_editor_operation_tab(
        &mut self,
        tab_id: &str,
        generation: RemoteTextGeneration,
        error: String,
    ) -> bool {
        let Some(workspace) = self.editor.workspace.as_mut() else {
            return false;
        };
        let Some(tab) = workspace.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return false;
        };
        if tab.generation != generation {
            return false;
        }
        tab.loading = false;
        tab.saving = false;
        tab.close_after_save = false;
        tab.error = Some(error);
        tab.backup_warning_path = None;
        workspace.close_after_save_all = false;
        true
    }

    pub(in crate::features) fn remove_editor_tabs_for_session(
        &mut self,
        session_id: &str,
    ) -> usize {
        let Some(workspace) = self.editor.workspace.as_mut() else {
            return 0;
        };
        let before = workspace.tabs.len();
        let active_removed = workspace
            .active_tab()
            .is_some_and(|tab| tab.session_id.as_deref() == Some(session_id));
        workspace
            .tabs
            .retain(|tab| tab.session_id.as_deref() != Some(session_id));
        let removed = before.saturating_sub(workspace.tabs.len());
        if active_removed {
            workspace.active_tab_id = workspace
                .tabs
                .first()
                .map(|tab| tab.id.clone())
                .unwrap_or_default();
        }
        if workspace.tabs.is_empty() {
            self.editor.workspace = None;
            self.editor.window.cancel_open();
        } else if removed > 0 {
            Self::clear_editor_close_state(workspace);
        }
        removed
    }

    pub(in crate::features) fn sync_editor_content(
        &mut self,
        tab_id: &str,
        content: impl Into<NyaDocumentSnapshot>,
    ) -> bool {
        let Some(workspace) = self.editor.workspace.as_mut() else {
            return false;
        };
        let Some(index) = workspace.tabs.iter().position(|tab| tab.id == tab_id) else {
            return false;
        };
        if workspace.tabs[index].loading || workspace.tabs[index].saving {
            return false;
        }
        Self::clear_editor_close_state(workspace);
        let tab = &mut workspace.tabs[index];
        tab.focused_field = crate::models::TransferEditorField::Content;
        let content = content.into();
        // Length-changing typing is an O(1) comparison. Equal-length replacements
        // still compare text so replacing a selection with itself stays clean.
        if tab.content == content {
            return false;
        }
        tab.content = content;
        tab.dirty = true;
        tab.conflict = false;
        tab.close_after_save = false;
        tab.reload_confirm = false;
        tab.error = None;
        tab.backup_warning_path = None;
        true
    }

    pub(in crate::features) fn editor_window(&self) -> Option<NyaWindowHandle> {
        self.editor.window.handle()
    }

    pub(in crate::features) fn editor_window_open_is_pending(&self) -> bool {
        self.editor.window.is_pending()
    }

    /// Claim the right to open the editor window while documents are available.
    pub(in crate::features) fn begin_editor_window_open(&mut self) -> bool {
        if self.editor.workspace.is_none() {
            return false;
        }
        self.editor.window.begin_open()
    }

    pub(in crate::features::transfers) fn finish_editor_window_open(
        &mut self,
        handle: NyaWindowHandle,
    ) {
        self.editor.window.finish_open(handle);
    }

    pub(in crate::features::transfers) fn finish_editor_window_activation(
        &mut self,
        handle: NyaWindowHandle,
        activated: bool,
    ) -> bool {
        self.editor.window.cancel_open();
        if activated {
            return false;
        }
        self.editor.window.clear_if(handle)
    }

    pub(in crate::features) fn clear_editor_window_tracking(&mut self) -> bool {
        let changed = self.editor.window.is_open_or_pending();
        self.editor.window.clear();
        changed
    }

    pub(in crate::features::transfers) fn clear_editor_window_if(
        &mut self,
        handle: NyaWindowHandle,
    ) -> bool {
        self.editor.window.clear_if(handle)
    }

    fn clear_editor_close_state(workspace: &mut TransferEditorWorkspaceState) {
        workspace.close_confirm = false;
        workspace.pending_close_tab_id = None;
        workspace.close_after_save_all = false;
    }
}

impl TransferEditorFeatureState {
    pub(super) fn new(_focus: FocusHandle) -> Self {
        Self {
            views: Default::default(),
            workspace: None,
            window: ChildWindowSlot::default(),
        }
    }
}
