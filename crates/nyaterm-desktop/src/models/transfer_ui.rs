use gpui::Pixels;
use nyaterm_transport::{
    RemoteTextGeneration, RemoteTextRevision, SftpFileEntry, SftpFileProperties,
};
use std::collections::HashSet;
use std::path::PathBuf;

use super::TransferBrowserSortColumn;

#[derive(Debug, Clone, Copy)]
pub(crate) struct TransferBrowserColumnResizeState {
    pub(crate) column: TransferBrowserSortColumn,
    pub(crate) start_x: Pixels,
    pub(crate) start_width: Pixels,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransferBrowserDragSelectionState {
    pub(crate) anchor_path: String,
    pub(crate) base_selection: HashSet<String>,
    pub(crate) additive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TransferBrowserFavoritesMenuState {
    pub(crate) x: Pixels,
    pub(crate) y: Pixels,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransferBrowserBreadcrumbSegment {
    pub(crate) label: String,
    pub(crate) path: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TransferBrowserPathMenuState {
    pub(crate) session_id: Option<String>,
    pub(crate) x: Pixels,
    pub(crate) y: Pixels,
    pub(crate) kind: TransferBrowserPathMenuKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TransferBrowserPathMenuKind {
    Overflow {
        segments: Vec<TransferBrowserBreadcrumbSegment>,
    },
    Children {
        path: String,
        branch_child_path: Option<String>,
        request_id: Option<String>,
        status: TransferBrowserChildrenMenuStatus,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TransferBrowserChildrenMenuStatus {
    Loading,
    Ready(Vec<SftpFileEntry>),
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TransferBrowserUploadMenuState {
    pub(crate) x: Pixels,
    pub(crate) y: Pixels,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum TransferBrowserContextTarget {
    #[default]
    CurrentDirectory,
    ParentDirectory,
    Entry(String),
    Suppressed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransferBrowserPendingRenameState {
    pub(crate) path: String,
    pub(crate) token: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TransferUnknownFileState {
    pub(crate) entry: SftpFileEntry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransferRenameState {
    pub(crate) old_path: String,
    pub(crate) raw_path_token: Option<String>,
    pub(crate) initial_name: String,
    pub(crate) value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransferMoveState {
    pub(crate) old_path: String,
    pub(crate) raw_path_token: Option<String>,
    pub(crate) name: String,
    pub(crate) value: String,
    /// Additional selected entries when moving more than one item at once.
    ///
    /// Empty for a single-item move, where `value` is the full destination path
    /// (rename-style). When non-empty, `value` is a destination *directory* and
    /// every entry (including `old_path`) is moved into it, matching the Tauri
    /// `openMoveDialog(getContextMenuEntries)` batch behavior.
    pub(crate) additional_entries: Vec<TransferMoveEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransferMoveEntry {
    pub(crate) old_path: String,
    pub(crate) raw_path_token: Option<String>,
    pub(crate) name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TransferJobMenuState {
    pub(crate) job_id: String,
    pub(crate) x: Pixels,
    pub(crate) y: Pixels,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransferNewFolderState {
    pub(crate) parent_path: String,
    pub(crate) value: String,
    pub(crate) mode: u32,
    pub(crate) open_after_create: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransferNewFileState {
    pub(crate) parent_path: String,
    pub(crate) value: String,
    pub(crate) mode: u32,
    pub(crate) open_after_create: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferPermissionTarget {
    NewFolder,
    NewFile,
    Properties,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferSymlinkField {
    Name,
    Target,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransferNewSymlinkState {
    pub(crate) parent_path: String,
    pub(crate) name: String,
    pub(crate) target: String,
    pub(crate) focused_field: TransferSymlinkField,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferPropertiesField {
    SymlinkTarget,
    Mode,
    Owner,
    Group,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransferPropertiesState {
    pub(crate) symlink_target_value: String,
    pub(crate) session_id: Option<String>,
    pub(crate) entry: SftpFileEntry,
    pub(crate) properties: Option<SftpFileProperties>,
    pub(crate) mode_value: String,
    pub(crate) owner_value: String,
    pub(crate) group_value: String,
    pub(crate) recursive: bool,
    pub(crate) saving: bool,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransferEditorState {
    pub(crate) id: String,
    pub(crate) session_id: Option<String>,
    pub(crate) remote_path: String,
    pub(crate) raw_path_token: Option<String>,
    pub(crate) name: String,
    pub(crate) content: String,
    pub(crate) search_query: String,
    pub(crate) active_match: usize,
    pub(crate) revision: Option<RemoteTextRevision>,
    pub(crate) generation: RemoteTextGeneration,
    pub(crate) loading: bool,
    pub(crate) saving: bool,
    pub(crate) dirty: bool,
    pub(crate) conflict: bool,
    pub(crate) close_after_save: bool,
    pub(crate) reload_confirm: bool,
    pub(crate) error: Option<String>,
    pub(crate) focused_field: TransferEditorField,
}

impl TransferEditorState {
    #[cfg(test)]
    pub(crate) fn tab_id(session_id: Option<&str>, remote_path: &str) -> String {
        format!("{}\n{remote_path}", session_id.unwrap_or_default())
    }

    pub(crate) fn tab_id_for_remote_path(
        session_id: Option<&str>,
        remote_path: &nyaterm_transport::RemoteFilePath,
    ) -> String {
        format!(
            "{}\n{}",
            session_id.unwrap_or_default(),
            remote_path.identity_key()
        )
    }

    pub(crate) fn remote_file_path(&self) -> nyaterm_transport::RemoteFilePath {
        nyaterm_transport::RemoteFilePath {
            display_path: self.remote_path.clone(),
            raw_path_token: self.raw_path_token.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransferEditorWorkspaceState {
    pub(crate) tabs: Vec<TransferEditorState>,
    pub(crate) active_tab_id: String,
    pub(crate) close_confirm: bool,
    pub(crate) pending_close_tab_id: Option<String>,
    pub(crate) close_after_save_all: bool,
}

impl TransferEditorWorkspaceState {
    pub(crate) fn new(tab: TransferEditorState) -> Self {
        Self {
            active_tab_id: tab.id.clone(),
            tabs: vec![tab],
            close_confirm: false,
            pending_close_tab_id: None,
            close_after_save_all: false,
        }
    }

    pub(crate) fn active_tab(&self) -> Option<&TransferEditorState> {
        self.tabs
            .iter()
            .find(|tab| tab.id == self.active_tab_id)
            .or_else(|| self.tabs.first())
    }

    pub(crate) fn active_tab_mut(&mut self) -> Option<&mut TransferEditorState> {
        let active_index = self
            .tabs
            .iter()
            .position(|tab| tab.id == self.active_tab_id)
            .unwrap_or(0);
        self.tabs.get_mut(active_index)
    }

    #[cfg(test)]
    pub(crate) fn tab_mut(
        &mut self,
        session_id: Option<&str>,
        remote_path: &str,
    ) -> Option<&mut TransferEditorState> {
        self.tabs
            .iter_mut()
            .find(|tab| tab.session_id.as_deref() == session_id && tab.remote_path == remote_path)
    }

    pub(crate) fn remove_tab(&mut self, tab_id: &str) -> bool {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return false;
        };
        let removed_active = self.active_tab_id == tab_id;
        self.tabs.remove(index);
        if removed_active {
            self.active_tab_id = self
                .tabs
                .get(index.min(self.tabs.len().saturating_sub(1)))
                .map(|tab| tab.id.clone())
                .unwrap_or_default();
        }
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransferExternalSyncPromptState {
    pub(crate) session_id: Option<String>,
    pub(crate) job_id: String,
    pub(crate) remote_path: String,
    pub(crate) raw_path_token: Option<String>,
    pub(crate) local_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferEditorField {
    Content,
    Search,
}

#[cfg(test)]
mod tests {
    use nyaterm_transport::{RemoteFilePath, RemoteTextGeneration};

    use super::{TransferEditorField, TransferEditorState, TransferEditorWorkspaceState};

    fn editor_tab(session_id: &str, remote_path: &str) -> TransferEditorState {
        TransferEditorState {
            id: TransferEditorState::tab_id(Some(session_id), remote_path),
            session_id: Some(session_id.to_string()),
            remote_path: remote_path.to_string(),
            raw_path_token: None,
            name: remote_path
                .rsplit('/')
                .next()
                .unwrap_or(remote_path)
                .to_string(),
            content: String::new(),
            search_query: String::new(),
            active_match: 0,
            revision: None,
            generation: RemoteTextGeneration::next(),
            loading: false,
            saving: false,
            dirty: false,
            conflict: false,
            close_after_save: false,
            reload_confirm: false,
            error: None,
            focused_field: TransferEditorField::Content,
        }
    }

    #[test]
    fn editor_workspace_tracks_active_tab_by_session_and_path() {
        let first = editor_tab("session-a", "/etc/hosts");
        let second = editor_tab("session-b", "/etc/hosts");
        let second_id = second.id.clone();
        let mut workspace = TransferEditorWorkspaceState::new(first);
        workspace.tabs.push(second);
        workspace.active_tab_id = second_id;

        assert_eq!(
            workspace
                .active_tab()
                .and_then(|tab| tab.session_id.as_deref()),
            Some("session-b")
        );
        workspace
            .tab_mut(Some("session-a"), "/etc/hosts")
            .expect("first tab")
            .dirty = true;
        assert!(workspace.tabs[0].dirty);
        assert!(!workspace.tabs[1].dirty);
    }

    #[test]
    fn editor_tab_ids_include_raw_remote_path_identity() {
        let first = RemoteFilePath::from_raw("/srv/invalid-?", b"/srv/invalid-\xfe");
        let second = RemoteFilePath::from_raw("/srv/invalid-?", b"/srv/invalid-\xff");

        assert_ne!(
            TransferEditorState::tab_id_for_remote_path(Some("session"), &first),
            TransferEditorState::tab_id_for_remote_path(Some("session"), &second)
        );
    }

    #[test]
    fn removing_active_editor_tab_selects_nearest_remaining_tab() {
        let first = editor_tab("session", "/one");
        let second = editor_tab("session", "/two");
        let third = editor_tab("session", "/three");
        let second_id = second.id.clone();
        let third_id = third.id.clone();
        let mut workspace = TransferEditorWorkspaceState::new(first);
        workspace.tabs.extend([second, third]);
        workspace.active_tab_id = second_id.clone();

        assert!(workspace.remove_tab(&second_id));
        assert_eq!(workspace.active_tab_id, third_id);
        assert_eq!(
            workspace.active_tab().map(|tab| tab.remote_path.as_str()),
            Some("/three")
        );
    }
}
