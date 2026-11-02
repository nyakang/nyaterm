use std::collections::HashMap;

use gpui::Entity;

use super::workspace::FileManagerWorkspace;
use crate::models::NavItem;

#[derive(Clone)]
pub(in crate::features) struct FileManagerSidebarRestore {
    pub left_open: bool,
    pub left_item: Option<NavItem>,
    pub left_collapsed: bool,
    pub right_open: bool,
    pub right_item: Option<NavItem>,
    pub right_collapsed: bool,
}

#[derive(Default)]
pub(in crate::features) struct FileManagerFeatureState {
    workspaces: HashMap<String, Entity<FileManagerWorkspace>>,
    pub(super) sftp_workspaces: HashMap<String, Entity<FileManagerWorkspace>>,
    next_id: u64,
    sidebar_restore: Option<FileManagerSidebarRestore>,
    sidebar_restore_suppressed: bool,
}

impl FileManagerFeatureState {
    pub(in crate::features) fn workspace(&self, id: &str) -> Option<Entity<FileManagerWorkspace>> {
        self.workspaces
            .get(id)
            .or_else(|| self.sftp_workspaces.get(id.strip_prefix("sftp-")?))
            .cloned()
    }

    pub(in crate::features) fn workspaces(
        &self,
    ) -> impl Iterator<Item = (&String, &Entity<FileManagerWorkspace>)> {
        self.workspaces.iter()
    }

    pub(in crate::features) fn all_workspaces(
        &self,
    ) -> impl Iterator<Item = Entity<FileManagerWorkspace>> + '_ {
        self.workspaces
            .values()
            .chain(self.sftp_workspaces.values())
            .cloned()
    }

    pub(in crate::features) fn remove_sftp_session(&mut self, id: &str) {
        self.sftp_workspaces.remove(id);
    }

    pub(in crate::features) fn active_id(&self) -> Option<&str> {
        self.workspaces.keys().next().map(String::as_str)
    }

    pub(in crate::features) fn insert(
        &mut self,
        id: String,
        workspace: Entity<FileManagerWorkspace>,
    ) {
        self.workspaces.insert(id, workspace);
    }

    pub(in crate::features) fn remove(&mut self, id: &str) -> Option<Entity<FileManagerWorkspace>> {
        self.workspaces.remove(id)
    }

    pub(in crate::features) fn next_id(&mut self) -> String {
        self.next_id = self.next_id.saturating_add(1);
        format!("fm-{}", self.next_id)
    }

    pub(in crate::features) fn references_session(
        &self,
        session_id: &str,
        cx: &AppContext,
    ) -> bool {
        self.workspaces
            .values()
            .any(|workspace| workspace.read(cx).references_session(session_id))
    }

    pub(in crate::features) fn replace_session_id(
        &mut self,
        old_id: &str,
        new_id: &str,
        cx: &mut AppContext,
    ) {
        if let Some(workspace) = self.sftp_workspaces.remove(old_id) {
            self.sftp_workspaces.insert(new_id.to_string(), workspace);
        }
        for workspace in self
            .workspaces
            .values()
            .chain(self.sftp_workspaces.values())
        {
            workspace.update(cx, |workspace, cx| {
                workspace.replace_session_id(old_id, new_id);
                if workspace.id == format!("sftp-{old_id}") {
                    workspace.id = format!("sftp-{new_id}");
                }
                cx.notify();
            });
        }
    }

    pub(in crate::features) fn set_sidebar_restore(&mut self, restore: FileManagerSidebarRestore) {
        if self.sidebar_restore.is_none() {
            self.sidebar_restore = Some(restore);
        }
        self.sidebar_restore_suppressed = false;
    }

    pub(in crate::features) fn take_sidebar_restore(
        &mut self,
    ) -> Option<FileManagerSidebarRestore> {
        if self.sidebar_restore_suppressed {
            self.sidebar_restore = None;
            return None;
        }
        self.sidebar_restore.take()
    }

    pub(in crate::features) fn suppress_sidebar_restore(&mut self) {
        self.sidebar_restore_suppressed = true;
        self.sidebar_restore = None;
    }

    pub(in crate::features) fn sidebar_restore_pending(&self) -> bool {
        self.sidebar_restore.is_some() && !self.sidebar_restore_suppressed
    }
}

type AppContext = gpui::App;
