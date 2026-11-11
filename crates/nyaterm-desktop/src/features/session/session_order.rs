use nyaterm_transport::SessionInfo;

use crate::features::NyaTermApp;
use crate::features::session::SessionStartTabPlacement;
use crate::models::SessionRuntimeMetadata;

impl NyaTermApp {
    pub(in crate::features) fn register_session(
        &mut self,
        session_id: &str,
        metadata: SessionRuntimeMetadata,
    ) {
        if let crate::models::SessionLaunchConfig::AdbShell(config) = &metadata.launch_config {
            self.android.views.insert(
                session_id.to_string(),
                crate::features::android::AndroidView {
                    device_key: config.device_key.clone(),
                    kind: nyaterm_core::models::android::AndroidViewKind::Shell,
                },
            );
        }
        let encoding = metadata.launch_config.encoding().map(ToOwned::to_owned);
        let is_terminal = encoding.is_some();
        self.session.register_session_metadata(session_id, metadata);
        self.finish_session_registration(session_id, encoding, is_terminal);
    }

    pub(in crate::features) fn register_session_for_start(
        &mut self,
        session_id: &str,
        metadata: SessionRuntimeMetadata,
        tab_placement: Option<SessionStartTabPlacement>,
        insert_index: Option<usize>,
    ) {
        let tab_placement = tab_placement.map(|p| self.session.resolve_start_tab_placement(p));
        let active_tab = self.active_workspace_tab();
        // Capture the successor before registration reconciles the layout and
        // appends the new tab. Ordinary workspace tabs also bound reserved slots.
        let before = tab_placement.and_then(|placement| {
            let group = self
                .shell
                .terminal_start_display_group(placement.request_sequence);
            let tabs = group
                .as_deref()
                .and_then(|group| self.shell.terminal_group_tabs(group))
                .map(|(tabs, _)| tabs)
                .unwrap_or_else(|| self.workspace_tab_ids());
            tabs.into_iter().enumerate().find_map(|(index, id)| {
                (id != session_id
                    && self.session.workspace_tab_position(&id, index)
                        > (placement.insert_index, placement.request_sequence))
                    .then_some(id)
            })
        });
        if let crate::models::SessionLaunchConfig::AdbShell(config) = &metadata.launch_config {
            self.android.views.insert(
                session_id.to_string(),
                crate::features::android::AndroidView {
                    device_key: config.device_key.clone(),
                    kind: nyaterm_core::models::android::AndroidViewKind::Shell,
                },
            );
        }
        let encoding = metadata.launch_config.encoding().map(ToOwned::to_owned);
        let is_terminal = encoding.is_some();
        self.session.prepare_session_metadata_for_start(
            session_id,
            metadata,
            tab_placement,
            insert_index,
        );
        self.queue_auto_recording(session_id);
        self.session.claim_session_events(session_id);
        self.finish_session_registration(session_id, encoding, is_terminal);
        if let Some(placement) = tab_placement {
            self.shell
                .place_started_terminal_tab(session_id, placement.request_sequence);
            if let Some(before) = before {
                self.shell
                    .place_tab_before_in_terminal_windows(session_id, &before);
            }
            // A completed background request can select its destination group,
            // but must never hide the session that still owns keyboard input.
            if let Some(active) = active_tab {
                self.sync_terminal_windows_active_tab(&active);
            }
            self.sync_terminal_frame_snapshot_priority();
        }
    }

    pub(in crate::features) fn register_session_for_reconnect(
        &mut self,
        session_id: &str,
        metadata: SessionRuntimeMetadata,
    ) {
        if let crate::models::SessionLaunchConfig::AdbShell(config) = &metadata.launch_config {
            self.android.views.insert(
                session_id.to_string(),
                crate::features::android::AndroidView {
                    device_key: config.device_key.clone(),
                    kind: nyaterm_core::models::android::AndroidViewKind::Shell,
                },
            );
        }
        let encoding = metadata.launch_config.encoding().map(ToOwned::to_owned);
        self.session
            .prepare_provisional_reconnect(session_id, metadata);
        self.queue_auto_recording(session_id);
        self.session.claim_session_events(session_id);
        self.sync_session_event_bridge_config();
        if let Some(encoding) = encoding {
            self.terminal.ensure_frame_session(
                session_id.to_string(),
                encoding,
                self.terminal_scrollback_line_limit(),
            );
        }
    }

    fn finish_session_registration(
        &mut self,
        session_id: &str,
        encoding: Option<String>,
        is_terminal: bool,
    ) {
        if let Some(encoding) = encoding {
            self.terminal.ensure_frame_session(
                session_id.to_string(),
                encoding,
                self.terminal_scrollback_line_limit(),
            );
        }
        self.sync_session_event_bridge_config();
        self.reconcile_terminal_windows();
        if !is_terminal {
            self.terminal.remove_frame_session(session_id);
        }
        if self.session.restore_is_complete() {
            self.persist_open_tabs();
        }
    }

    pub(in crate::features) fn settle_session_start_tab_placements_if_idle(&mut self) {
        if self.session.start_visible_tab_reservation_count() == 0 {
            self.session.clear_start_tab_placements();
            self.shell.clear_terminal_start_groups();
        }
    }

    /// Tab-root count for chrome (status bar) without allocating SessionInfo.
    pub(in crate::features) fn ordered_tab_session_count(&self) -> usize {
        self.session
            .session_order()
            .iter()
            .filter(|session_id| {
                !self.is_secondary_pane_session(session_id)
                    && !self.shell.session_tab_is_hidden(session_id)
            })
            .count()
    }

    /// True when this session is a secondary leaf inside another tab's pane tree
    /// (Tauri: multiple SessionPanes under one Tab, only one strip entry).
    pub(in crate::features) fn is_secondary_pane_session(&self, session_id: &str) -> bool {
        self.shell
            .workspace_tab_owner(session_id)
            .is_some_and(|owner| owner != session_id)
    }

    /// Owning tab-root session id for a leaf (self when the session is a tab root).
    pub(in crate::features) fn tab_root_for_session(&self, session_id: &str) -> String {
        let mut current = session_id.to_string();
        // Flatten owner chains defensively.
        for _ in 0..8 {
            match self.shell.workspace_tab_owner(&current) {
                Some(owner) if owner != current => current = owner.to_string(),
                _ => break,
            }
        }
        current
    }

    pub(in crate::features) fn tab_tree_session_ids(&self, session_id: &str) -> Vec<String> {
        let tab_root = self.tab_root_for_session(session_id);
        self.shell
            .workspace_pane_root(&tab_root)
            .map(|root| root.session_ids())
            .filter(|ids| !ids.is_empty())
            .unwrap_or_else(|| vec![tab_root])
    }

    pub(in crate::features) fn tab_tree_is_locked(&self, session_id: &str) -> bool {
        let tab_root = self.tab_root_for_session(session_id);
        self.session.tab_is_locked(&tab_root)
            || self
                .tab_tree_session_ids(&tab_root)
                .iter()
                .any(|id| self.session.tab_is_locked(id))
    }

    /// Sessions shown in the global tab strip / multi-leaf tab lists (tab roots only).
    pub(in crate::features) fn ordered_tab_sessions(&self) -> Vec<SessionInfo> {
        let mut ordered = Vec::with_capacity(self.session.session_order_len());
        let mut seen = std::collections::HashSet::with_capacity(self.session.session_order_len());
        for session_id in self.session.session_order() {
            if !seen.insert(session_id.as_str()) {
                continue;
            }
            if self.is_secondary_pane_session(session_id)
                || self.shell.session_tab_is_hidden(session_id)
            {
                continue;
            }
            if let Some(session) = self.session.session_info(session_id) {
                ordered.push(session);
            }
        }
        for (session_id, _) in self.session.metadata_entries() {
            if seen.insert(session_id)
                && !self.shell.session_tab_is_hidden(session_id)
                && !self.is_secondary_pane_session(session_id)
                && let Some(session) = self.session.session_info(session_id)
            {
                ordered.push(session);
            }
        }
        if self.shell.terminal_windows_restore_is_complete()
            && self.shell.workspace_pane_roots().is_empty()
        {
            let ids = self.shell.terminal_window_tab_ids();
            let positions = ids
                .iter()
                .enumerate()
                .map(|(index, id)| (id.as_str(), index))
                .collect::<std::collections::HashMap<_, _>>();
            ordered.sort_by_key(|session| {
                positions
                    .get(session.id.as_str())
                    .copied()
                    .unwrap_or(usize::MAX)
            });
        }
        ordered
    }

    /// Prefer the currently focused leaf when it belongs to `tab_root`, else the tab root.
    pub(in crate::features) fn active_pane_for_tab_root(&self, tab_root: &str) -> String {
        if let Some(active) = self.session.active_id()
            && self.tab_root_for_session(active) == tab_root
        {
            return active.to_string();
        }
        if let Some(root) = self.shell.workspace_pane_root(tab_root)
            && let Some(first) = root.session_ids().into_iter().next()
        {
            return first;
        }
        tab_root.to_string()
    }
}
