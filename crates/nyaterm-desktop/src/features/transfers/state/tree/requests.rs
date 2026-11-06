//! A single submitted tree listing, with the remaining demand kept in session state.

use super::super::TransferFeatureState;
use super::{TransferTreeState, TreeSession, key};
use crate::blocking_jobs::JobHandle;
use nyaterm_files::file_browser::{FileBrowserBackendKind, file_browser_parent};
use nyaterm_files::sftp::RemoteFilePath;

pub(super) struct TreeListingRequest {
    session: String,
    backend: FileBrowserBackendKind,
    path: RemoteFilePath,
    generation: u64,
    job_id: String,
    handle: Option<JobHandle>,
}

impl TreeSession {
    fn is_expanded_visible(&self, id: &str) -> bool {
        let (Some(backend), Some(root), Some(node)) =
            (self.backend, &self.root, self.nodes.get(id))
        else {
            return false;
        };
        if !self.expanded.contains(id) {
            return false;
        }
        let root_key = key(backend, root);
        let mut path = node.path.clone();
        // Check only this path's ancestry; do not rebuild/sort the entire visible tree.
        for _ in 0..64 {
            let id = key(backend, &path);
            if id == root_key {
                return true;
            }
            let parent = match backend {
                FileBrowserBackendKind::Remote => match path.parent() {
                    Ok(parent) => parent,
                    Err(_) => return false,
                },
                FileBrowserBackendKind::Local => {
                    RemoteFilePath::new(file_browser_parent(backend, &path.display_path))
                }
            };
            let parent_key = key(backend, &parent);
            if id == parent_key
                || !self.expanded.contains(&parent_key)
                || !self
                    .nodes
                    .get(&parent_key)
                    .and_then(|node| node.entries.as_ref())
                    .is_some_and(|entries| {
                        entries
                            .iter()
                            .any(|entry| key(backend, &entry.remote_path()) == id)
                    })
            {
                return false;
            }
            path = parent;
        }
        false
    }

    fn wants_listing(&self, id: &str) -> bool {
        self.reveal_chain.iter().any(|ancestor| ancestor == id)
            || (self.manual_requests.iter().any(|pending| pending == id)
                && self.is_expanded_visible(id))
    }
}

impl TransferTreeState {
    pub(super) fn cancel_obsolete_listing(&mut self) {
        let Some(request) = &self.listing else {
            return;
        };
        let wanted = self.sessions.get(&request.session).is_some_and(|state| {
            state.backend == Some(request.backend)
                && state.wants_listing(&key(request.backend, &request.path))
        });
        if !wanted {
            self.cancel_listing();
        }
    }

    fn cancel_listing(&mut self) {
        let Some(request) = self.listing.as_mut() else {
            return;
        };
        if let Some(handle) = &request.handle {
            handle.cancel();
        }
        if let Some(state) = self.sessions.get_mut(&request.session)
            && let Some(node) = state.nodes.get_mut(&key(request.backend, &request.path))
            && node.pending == Some(request.generation)
        {
            node.pending = None;
            state.dirty = true;
        }
    }

    pub(super) fn cancel_session_requests(&mut self, session: &str) {
        if self
            .listing
            .as_ref()
            .is_some_and(|request| request.session == session)
        {
            self.cancel_listing();
        }
        if let Some(state) = self.sessions.get_mut(session) {
            // Keep demand for the displayed tree so reopening a cached session can
            // resume loading without another reveal taking over manual selection.
            state.scroll_to_selection = false;
        }
    }

    pub(super) fn next_listing_path(
        &mut self,
        session: &str,
        navigation_path: Option<&RemoteFilePath>,
    ) -> Option<RemoteFilePath> {
        self.cancel_obsolete_listing();
        if self.listing.is_some() {
            return None;
        }
        let state = self.sessions.get(session)?;
        let backend = state.backend?;
        let excluded = navigation_path.map(|path| key(backend, path));
        for id in &state.manual_requests {
            if excluded.as_ref() != Some(id)
                && let Some(node) = state.nodes.get(id)
                && node.entries.is_none()
                && node.pending.is_none()
                && node.error.is_none()
                && state.is_expanded_visible(id)
            {
                return Some(node.path.clone());
            }
        }
        for id in &state.reveal_chain {
            let node = state.nodes.get(id)?;
            // An ancestor error blocks descendants until another explicit reveal or retry.
            if node.error.is_some() || node.pending.is_some() {
                return None;
            }
            if node.entries.is_none() && excluded.as_ref() != Some(id) {
                return Some(node.path.clone());
            }
        }
        None
    }

    pub(super) fn track_listing(
        &mut self,
        session: String,
        backend: FileBrowserBackendKind,
        path: RemoteFilePath,
        generation: u64,
        job_id: String,
    ) {
        assert!(self.listing.is_none(), "only one submitted tree listing");
        self.listing = Some(TreeListingRequest {
            session,
            backend,
            path,
            generation,
            job_id,
            handle: None,
        });
    }

    pub(super) fn bind_listing_handle(&mut self, handle: Option<JobHandle>) {
        if let Some(request) = self.listing.as_mut() {
            request.handle = handle;
        }
    }

    pub(super) fn finish_listing(&mut self, job_id: &str) -> bool {
        if self
            .listing
            .as_ref()
            .is_some_and(|request| request.job_id == job_id)
        {
            self.listing = None;
            return true;
        }
        false
    }

    pub(super) fn discard_listing(&mut self, job_id: &str) -> bool {
        if self
            .listing
            .as_ref()
            .is_some_and(|request| request.job_id == job_id)
        {
            self.cancel_listing();
            return self.finish_listing(job_id);
        }
        false
    }
}

impl TransferFeatureState {
    pub(in crate::features) fn next_tree_listing_path(
        &mut self,
        session: &str,
        navigation_path: Option<&RemoteFilePath>,
    ) -> Option<RemoteFilePath> {
        self.tree.next_listing_path(session, navigation_path)
    }

    pub(in crate::features) fn track_tree_listing(
        &mut self,
        session: String,
        backend: FileBrowserBackendKind,
        path: RemoteFilePath,
        generation: u64,
        job_id: String,
    ) {
        self.tree
            .track_listing(session, backend, path, generation, job_id);
    }

    pub(in crate::features) fn bind_tree_listing_handle(&mut self, handle: Option<JobHandle>) {
        self.tree.bind_listing_handle(handle);
    }

    pub(in crate::features) fn finish_tree_listing(&mut self, job_id: &str) -> bool {
        self.tree.finish_listing(job_id)
    }

    pub(in crate::features) fn discard_tree_listing(&mut self, job_id: &str) -> bool {
        self.tree.discard_listing(job_id)
    }

    pub(in crate::features) fn cancel_tree_session_requests(&mut self, session: &str) {
        self.tree.cancel_session_requests(session);
    }
}
