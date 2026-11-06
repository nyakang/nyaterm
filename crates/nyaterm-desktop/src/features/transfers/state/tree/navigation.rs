//! Navigation owns presentation only until the next manual tree interaction.

use super::super::TransferFeatureState;
use super::{TransferTreeState, TreeSession};
use gpui::{DeferredScrollToItem, Pixels, Point};
use nyaterm_files::file_browser::FileBrowserBackendKind;
use nyaterm_files::sftp::RemoteFilePath;
use std::collections::HashSet;

pub(super) struct TreeNavigationCheckpoint {
    job_id: String,
    interaction_version: u64,
    root: Option<RemoteFilePath>,
    expanded: HashSet<String>,
    selected: HashSet<String>,
    anchor: Option<String>,
    focused: Option<String>,
    scroll_offset: Point<Pixels>,
    deferred_scroll: Option<DeferredScrollToItem>,
    scroll_to_selection: bool,
    reveal_chain: Vec<String>,
}

impl TreeNavigationCheckpoint {
    fn capture(state: &TreeSession, job_id: String) -> Self {
        let scroll = state.scroll.0.borrow();
        Self {
            job_id,
            interaction_version: state.interaction_version,
            root: state.root.clone(),
            expanded: state.expanded.clone(),
            selected: state.selected.clone(),
            anchor: state.anchor.clone(),
            focused: state.focused.clone(),
            scroll_offset: scroll.base_handle.offset(),
            deferred_scroll: scroll.deferred_scroll_to_item,
            scroll_to_selection: state.scroll_to_selection,
            reveal_chain: state.reveal_chain.clone(),
        }
    }

    fn restore(self, state: &mut TreeSession) {
        state.root = self.root;
        state.expanded = self.expanded;
        state.selected = self.selected;
        state.anchor = self.anchor;
        state.focused = self.focused;
        state.scroll_to_selection = self.scroll_to_selection;
        state.reveal_chain = self.reveal_chain;
        let mut scroll = state.scroll.0.borrow_mut();
        scroll.base_handle.set_offset(self.scroll_offset);
        scroll.deferred_scroll_to_item = self.deferred_scroll;
        state.dirty = true;
    }
}

impl TransferTreeState {
    pub(super) fn begin_navigation(
        &mut self,
        session: &str,
        backend: FileBrowserBackendKind,
        job_id: String,
    ) {
        let state = self.sessions.entry(session.to_string()).or_default();
        state.ensure_backend(backend);
        self.navigations.insert(
            session.to_string(),
            TreeNavigationCheckpoint::capture(state, job_id),
        );
    }

    pub(super) fn renew_navigation(&mut self, session: &str) {
        if let Some(checkpoint) = self.navigations.get_mut(session)
            && let Some(state) = self.sessions.get(session)
            && checkpoint.interaction_version != state.interaction_version
        {
            // Clicking Sync again is a new intent, even when it reuses the request.
            *checkpoint = TreeNavigationCheckpoint::capture(state, checkpoint.job_id.clone());
        }
    }

    /// Returns whether the request still owns positioning. Never restore directory caches.
    pub(super) fn settle_navigation(&mut self, session: &str, job_id: &str, success: bool) -> bool {
        if !self
            .navigations
            .get(session)
            .is_some_and(|checkpoint| checkpoint.job_id == job_id)
        {
            return false;
        }
        let checkpoint = self
            .navigations
            .remove(session)
            .expect("checked checkpoint");
        let Some(state) = self.sessions.get_mut(session) else {
            return false;
        };
        let owned = checkpoint.interaction_version == state.interaction_version;
        if owned && !success {
            checkpoint.restore(state);
            self.cancel_obsolete_listing();
        } else if !success {
            // Preserve manual presentation, but stop prefetching the failed target.
            state.reveal_chain.clear();
            self.cancel_obsolete_listing();
        }
        owned
    }
}

impl TransferFeatureState {
    pub(in crate::features::transfers::state) fn renew_tree_navigation(&mut self, session: &str) {
        self.tree.renew_navigation(session);
    }

    pub(in crate::features) fn begin_tree_navigation(
        &mut self,
        session: &str,
        backend: FileBrowserBackendKind,
        job_id: String,
    ) {
        self.tree.begin_navigation(session, backend, job_id);
    }

    pub(in crate::features) fn settle_tree_navigation(
        &mut self,
        session: &str,
        job_id: &str,
        success: bool,
    ) -> bool {
        self.tree.settle_navigation(session, job_id, success)
    }
}
