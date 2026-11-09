use gpui::FocusHandle;
use nyaterm_core::{RestorableOpenTab, RestorableTerminalWindowNode, RestorableWorkspacePaneNode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StartupRestoreFocusSnapshot {
    pub(crate) workspace_tab: String,
    pub(crate) active_session: Option<String>,
    pub(crate) terminal_group: Option<String>,
    pub(crate) focus: Option<FocusHandle>,
}

#[derive(Debug, Default)]
pub(crate) enum StartupRestoreFocusHandoff {
    #[default]
    Unrequested,
    Waiting(Option<StartupRestoreFocusSnapshot>),
    Scheduled(StartupRestoreFocusSnapshot),
    Finished,
}

#[derive(Debug, Default)]
pub struct StartupRestoreStore {
    started_after_window_open: bool,
    open_tabs_restored: bool,
    queue: Vec<RestorableOpenTab>,
    pending_pane_layouts: Vec<RestorableWorkspacePaneNode>,
    pending_active_pane_indexes: Vec<usize>,
    loaded_open_tabs: Option<Vec<RestorableOpenTab>>,
    loaded_terminal_window_layout: Option<Option<RestorableTerminalWindowNode>>,
    loaded_workspace_pane_layout: Option<Option<RestorableWorkspacePaneNode>>,
    focus_handoff: StartupRestoreFocusHandoff,
}

impl StartupRestoreStore {
    pub(crate) fn request_focus_handoff(&mut self) {
        if !self.queue.is_empty()
            && matches!(self.focus_handoff, StartupRestoreFocusHandoff::Unrequested)
        {
            self.focus_handoff = StartupRestoreFocusHandoff::Waiting(None);
        }
    }

    pub(crate) fn focus_handoff(&self) -> &StartupRestoreFocusHandoff {
        &self.focus_handoff
    }

    pub(crate) fn prepare_focus_handoff(&mut self, snapshot: StartupRestoreFocusSnapshot) -> bool {
        let StartupRestoreFocusHandoff::Waiting(previous) = &mut self.focus_handoff else {
            return false;
        };
        if previous
            .as_ref()
            .is_some_and(|previous| previous != &snapshot)
        {
            self.finish_focus_handoff();
            return false;
        }
        *previous = Some(snapshot);
        true
    }

    pub(crate) fn schedule_focus_handoff(&mut self) -> Option<StartupRestoreFocusSnapshot> {
        let StartupRestoreFocusHandoff::Waiting(Some(snapshot)) = &self.focus_handoff else {
            return None;
        };
        let snapshot = snapshot.clone();
        self.focus_handoff = StartupRestoreFocusHandoff::Scheduled(snapshot.clone());
        Some(snapshot)
    }

    pub(crate) fn defer_focus_handoff(&mut self) {
        if let StartupRestoreFocusHandoff::Scheduled(snapshot) = &self.focus_handoff {
            // Preserve the target while waiting for activation, so a later user
            // selection cannot turn the old startup request into a new handoff.
            self.focus_handoff = StartupRestoreFocusHandoff::Waiting(Some(snapshot.clone()));
        }
    }

    pub(crate) fn finish_focus_handoff(&mut self) {
        self.focus_handoff = StartupRestoreFocusHandoff::Finished;
    }

    pub fn mark_started_after_window_open(&mut self) -> bool {
        if self.started_after_window_open {
            return false;
        }
        self.started_after_window_open = true;
        true
    }

    pub fn started_after_window_open(&self) -> bool {
        self.started_after_window_open
    }

    pub fn open_tabs_restored(&self) -> bool {
        self.open_tabs_restored
    }

    pub fn mark_open_tabs_restored(&mut self) -> bool {
        if self.open_tabs_restored {
            return false;
        }
        self.open_tabs_restored = true;
        true
    }

    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    pub fn set_queue(&mut self, queue: Vec<RestorableOpenTab>) {
        self.queue = queue;
    }

    pub fn set_loaded_open_tabs(&mut self, tabs: Vec<RestorableOpenTab>) {
        self.loaded_open_tabs = Some(tabs);
    }

    pub fn take_loaded_open_tabs(&mut self) -> Option<Vec<RestorableOpenTab>> {
        self.loaded_open_tabs.take()
    }

    pub fn set_loaded_window_layouts(
        &mut self,
        terminal: Option<RestorableTerminalWindowNode>,
        workspace: Option<RestorableWorkspacePaneNode>,
    ) {
        self.loaded_terminal_window_layout = Some(terminal);
        self.loaded_workspace_pane_layout = Some(workspace);
    }

    pub fn take_loaded_terminal_window_layout(
        &mut self,
    ) -> Option<Option<RestorableTerminalWindowNode>> {
        self.loaded_terminal_window_layout.take()
    }

    pub fn take_loaded_workspace_pane_layout(
        &mut self,
    ) -> Option<Option<RestorableWorkspacePaneNode>> {
        self.loaded_workspace_pane_layout.take()
    }

    pub fn queue_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn pop_next_tab(&mut self) -> Option<RestorableOpenTab> {
        if self.queue.is_empty() {
            None
        } else {
            Some(self.queue.remove(0))
        }
    }

    pub fn can_pump_queue(&self, pending_session_active: bool) -> bool {
        !pending_session_active && !self.queue.is_empty()
    }

    pub fn can_finish_restore(&self, session_start_busy: bool) -> bool {
        // The final asynchronous start settles after its tab has left the queue.
        // Credential preparation must also settle before the caller rebuilds
        // layouts; its completion flag prevents subsequent empty-queue passes.
        self.open_tabs_restored && self.queue.is_empty() && !session_start_busy
    }

    pub fn clear_pending_layouts(&mut self) {
        self.pending_pane_layouts.clear();
        self.pending_active_pane_indexes.clear();
    }

    pub fn push_pending_pane_layout(&mut self, layout: RestorableWorkspacePaneNode) {
        self.pending_pane_layouts.push(layout);
    }

    pub fn push_pending_active_pane_index(&mut self, index: usize) {
        self.pending_active_pane_indexes.push(index);
    }

    pub fn take_pending_pane_layouts(&mut self) -> Vec<RestorableWorkspacePaneNode> {
        std::mem::take(&mut self.pending_pane_layouts)
    }

    pub fn take_pending_active_pane_indexes(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.pending_active_pane_indexes)
    }
}
