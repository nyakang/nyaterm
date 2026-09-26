//! Built-in remote-editor workspace, tab lifecycle and window tracking.

use gpui::FocusHandle;
use nyaterm_transport::{RemoteTextDocument, RemoteTextGeneration, RemoteTextWriteResult};
#[cfg(test)]
use nyaterm_transport::{RemoteTextMetadata, RemoteTextRevision, SftpWriteTextResult};
use nyaterm_ui::{ChildWindowSlot, NyaWindowHandle};

use crate::models::{TransferEditorState, TransferEditorWorkspaceState};

use super::{
    TransferEditorCloseAfterSave, TransferEditorCloseOutcome, TransferEditorDiscardOutcome,
    TransferEditorFeatureState, TransferEditorSaveOutcome, TransferFeatureState,
};

impl TransferFeatureState {
    pub(in crate::features) fn editor_focus(&self) -> &FocusHandle {
        &self.editor.focus
    }

    pub(in crate::features) fn editor_workspace(&self) -> Option<&TransferEditorWorkspaceState> {
        self.editor.workspace.as_ref()
    }

    pub(in crate::features) fn editor_workspace_snapshot(
        &self,
    ) -> Option<TransferEditorWorkspaceState> {
        self.editor.workspace.clone()
    }

    pub(in crate::features) fn editor_has_workspace(&self) -> bool {
        self.editor.workspace.is_some()
    }

    pub(in crate::features) fn editor_inline_overlay_is_open(&self) -> bool {
        self.editor.workspace.is_some() && !self.editor.window.is_open_or_pending()
    }

    pub(in crate::features) fn active_editor_tab(&self) -> Option<&TransferEditorState> {
        self.editor
            .workspace
            .as_ref()
            .and_then(TransferEditorWorkspaceState::active_tab)
    }

    pub(in crate::features) fn active_editor_tab_mut(
        &mut self,
    ) -> Option<&mut TransferEditorState> {
        self.editor
            .workspace
            .as_mut()
            .and_then(TransferEditorWorkspaceState::active_tab_mut)
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
            workspace.active_tab_id = tab_id;
            Self::clear_editor_close_state(workspace);
            self.editor.tabs_menu_open = false;
            already_open
        } else {
            self.editor.workspace = Some(TransferEditorWorkspaceState::new(tab));
            self.editor.tabs_menu_open = false;
            false
        }
    }

    pub(in crate::features) fn activate_editor_tab(&mut self, tab_id: &str) -> bool {
        self.editor.tabs_menu_open = false;
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

    pub(in crate::features) fn request_editor_tab_close(
        &mut self,
        tab_id: &str,
    ) -> TransferEditorCloseOutcome {
        self.editor.tabs_menu_open = false;
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
        self.editor.tabs_menu_open = false;
        self.editor.window.cancel_open();
        TransferEditorCloseOutcome::Closed
    }

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
            self.editor.tabs_menu_open = false;
            self.editor.window.cancel_open();
            TransferEditorDiscardOutcome::WorkspaceDiscarded
        }
    }

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

    pub(in crate::features) fn cancel_editor_reload(&mut self) -> bool {
        let Some(tab) = self.active_editor_tab_mut() else {
            return false;
        };
        tab.reload_confirm = false;
        if tab
            .error
            .as_deref()
            .is_some_and(|error| error.contains("Reload will discard"))
        {
            tab.error = None;
        }
        true
    }

    pub(in crate::features) fn cancel_editor_conflict(&mut self) -> bool {
        let Some(tab) = self.active_editor_tab_mut() else {
            return false;
        };
        tab.conflict = false;
        true
    }

    pub(in crate::features) fn clear_editor_close_request(&mut self) -> bool {
        let Some(workspace) = self.editor.workspace.as_mut() else {
            return false;
        };
        let changed = workspace.close_confirm
            || workspace.pending_close_tab_id.is_some()
            || workspace.close_after_save_all;
        Self::clear_editor_close_state(workspace);
        changed
    }

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
        tab.conflict = false;
        tab.reload_confirm = false;
        true
    }

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
        tab.content = file.content;
        tab.revision = Some(file.revision);
        tab.loading = false;
        tab.saving = false;
        tab.dirty = false;
        tab.conflict = false;
        tab.close_after_save = false;
        tab.reload_confirm = false;
        tab.error = None;
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
                TransferEditorSaveOutcome::Saved
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
            self.editor.tabs_menu_open = false;
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
            self.editor.tabs_menu_open = false;
            self.editor.window.cancel_open();
        } else if removed > 0 {
            Self::clear_editor_close_state(workspace);
        }
        removed
    }

    pub(in crate::features) fn sync_editor_content(
        &mut self,
        tab_id: &str,
        content: String,
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
        if tab.content == content {
            return false;
        }
        tab.content = content;
        tab.dirty = true;
        tab.conflict = false;
        tab.close_after_save = false;
        tab.reload_confirm = false;
        tab.error = None;
        true
    }

    pub(in crate::features) fn editor_tabs_menu_is_open(&self) -> bool {
        self.editor.tabs_menu_open
    }

    pub(in crate::features) fn toggle_editor_tabs_menu(&mut self) -> bool {
        self.editor.tabs_menu_open = !self.editor.tabs_menu_open;
        self.editor.tabs_menu_open
    }

    pub(in crate::features) fn close_editor_tabs_menu(&mut self) -> bool {
        std::mem::take(&mut self.editor.tabs_menu_open)
    }

    pub(in crate::features::transfers) fn editor_window(&self) -> Option<NyaWindowHandle> {
        self.editor.window.handle()
    }

    pub(in crate::features) fn editor_window_is_open(&self) -> bool {
        self.editor.window.is_open()
    }

    pub(in crate::features) fn editor_window_open_is_pending(&self) -> bool {
        self.editor.window.is_pending()
    }

    /// Claim the right to open the editor window; also refuses when there is no
    /// workspace to show, so the caller can fall back to the inline overlay.
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
    pub(super) fn new(focus: FocusHandle) -> Self {
        Self {
            workspace: None,
            tabs_menu_open: false,
            focus,
            window: ChildWindowSlot::default(),
        }
    }
}
