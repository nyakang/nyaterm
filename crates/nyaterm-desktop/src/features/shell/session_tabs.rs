//! A projection of workspace tabs and visible connection attempts. Layout trees
//! contain only workspace tabs; connection attempts never enter persisted layouts.
use gpui::Context;

use crate::features::NyaTermApp;
use crate::models::WorkspaceTabId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::features) enum SessionStripTabId {
    Workspace(WorkspaceTabId),
    Start(u64),
}

impl SessionStripTabId {
    pub fn layout_key(&self) -> String {
        match self {
            Self::Workspace(id) => id.layout_key(),
            Self::Start(sequence) => format!("\0start:{sequence}"),
        }
    }

    pub fn is_start(&self) -> bool {
        matches!(self, Self::Start(_))
    }
}

#[derive(Clone)]
pub(in crate::features) struct SessionStripTab {
    pub id: SessionStripTabId,
    pub request_id: Option<String>,
    pub name: String,
    pub error: Option<String>,
}

impl SessionStripTab {
    pub fn snapshot_key(&self) -> String {
        // Retry and pending -> failed retain a visual identity, but invalidate a
        // gesture begun against the previous request or state.
        format!(
            "{}:{:?}:{}",
            self.id.layout_key(),
            self.request_id,
            self.error.is_some()
        )
    }
}

impl NyaTermApp {
    pub(in crate::features) fn selected_session_strip_tab(
        &self,
        group: &Option<String>,
    ) -> Option<SessionStripTabId> {
        let tabs = self.session_strip_tabs(group);
        if let Some(tab) = tabs.iter().find(|tab| {
            tab.request_id
                .as_deref()
                .is_some_and(|id| self.session.start_request_is_active(id))
        }) {
            return Some(tab.id.clone());
        }
        // Layouts retain the previous formal tab while a connection attempt is
        // visible. That fallback must not compete with the selected attempt.
        let active = group
            .as_deref()
            .and_then(|id| self.shell.terminal_group_tabs(id))
            .map(|(_, active)| active)
            .unwrap_or_else(|| self.active_workspace_tab());
        tabs.into_iter()
            .find(|tab| {
                !tab.id.is_start() && active.as_deref() == Some(tab.id.layout_key().as_str())
            })
            .map(|tab| tab.id)
    }

    pub(in crate::features) fn drop_on_session_start_tab(
        &mut self,
        payload: &super::WorkspaceTabDragPayload,
        group: &Option<String>,
        request_id: &str,
        cx: &mut Context<Self>,
    ) {
        if payload.tab_id.is_start()
            && (payload.source_workspace_id != self.workspace_id
                || &payload.source_group_id != group)
        {
            self.clear_session_tab_drag(cx);
            return;
        }
        self.accept_session_tab_drop(payload, cx);
        if self.commit_session_tab_preview(payload, group, cx) {
            return;
        }
        let tabs = self.session_strip_tabs(group);
        let Some(index) = tabs
            .iter()
            .position(|tab| tab.request_id.as_deref() == Some(request_id))
        else {
            return;
        };
        let before = tabs[index + 1..]
            .iter()
            .find(|tab| !tab.id.is_start())
            .map(|tab| tab.id.layout_key());
        if payload.source_workspace_id != self.workspace_id {
            let placement = before
                .map(nyaterm_core::MoveTabPlacement::BeforeTab)
                .or_else(|| {
                    group
                        .clone()
                        .map(|leaf_id| nyaterm_core::MoveTabPlacement::TerminalLeaf {
                            leaf_id,
                            edge: None,
                        })
                })
                .unwrap_or(nyaterm_core::MoveTabPlacement::Append);
            self.request_tab_tree_move(payload, placement, cx);
        } else if let Some(before) = before {
            self.reorder_session_relative(payload.tab_id.layout_key(), before, false, cx);
        } else if let Some(group) = group {
            self.dock_tab_on_terminal_window_leaf(
                payload.tab_id.layout_key(),
                group.clone(),
                crate::models::TabDockZone::Center,
                cx,
            );
        } else {
            self.reorder_session_to_end(payload.tab_id.layout_key(), cx);
        }
    }

    pub(in crate::features) fn session_strip_tabs(
        &self,
        group: &Option<String>,
    ) -> Vec<SessionStripTab> {
        let workspace = group
            .as_deref()
            .and_then(|id| self.shell.terminal_group_tabs(id))
            .map(|(tabs, _)| tabs)
            .unwrap_or_else(|| self.workspace_tab_ids());
        let belongs = |placement: Option<crate::features::session::SessionStartTabPlacement>| {
            group.as_deref().is_none_or(|group| {
                placement
                    .and_then(|p| self.shell.terminal_start_display_group(p.request_sequence))
                    .or_else(|| self.shell.first_terminal_group())
                    .as_deref()
                    == Some(group)
            })
        };
        let mut starts = self
            .session
            .start_pending_entries()
            .filter(|(_, pending)| pending.reconnect_session_id.is_none())
            .map(|(id, pending)| (id, pending, None))
            .chain(
                self.session
                    .start_failed_entries()
                    .map(|(id, failed)| (id, &failed.pending, Some(failed.error.clone()))),
            )
            .filter(|(_, pending, _)| belongs(pending.tab_placement))
            .filter_map(|(id, pending, error)| {
                let placement = pending
                    .tab_placement
                    .map(|p| self.session.resolve_start_tab_placement(p))?;
                Some((
                    (placement.insert_index, placement.request_sequence),
                    SessionStripTab {
                        id: SessionStripTabId::Start(placement.request_sequence),
                        request_id: Some(id.clone()),
                        name: pending
                            .custom_name
                            .as_deref()
                            .map(str::trim)
                            .filter(|name| !name.is_empty())
                            .unwrap_or(&pending.connection_name)
                            .to_string(),
                        error,
                    },
                ))
            })
            .collect::<Vec<_>>();
        starts.sort_by_key(|(position, _)| *position);
        let mut starts = starts.into_iter().peekable();
        let mut tabs = Vec::new();
        for (index, key) in workspace.into_iter().enumerate() {
            let position = self.session.workspace_tab_position(&key, index);
            while starts.peek().is_some_and(|(next, _)| *next < position) {
                tabs.push(starts.next().unwrap().1);
            }
            tabs.push(SessionStripTab {
                id: SessionStripTabId::Workspace(WorkspaceTabId::from_layout_key(&key)),
                request_id: None,
                name: String::new(),
                error: None,
            });
        }
        tabs.extend(starts.map(|(_, tab)| tab));
        tabs
    }

    pub(in crate::features) fn activate_session_strip_tab(
        &mut self,
        tab: &SessionStripTab,
        cx: &mut Context<Self>,
    ) {
        if let Some(request_id) = &tab.request_id {
            if tab.error.is_some() {
                self.select_failed_session_start(request_id.clone(), cx);
            } else {
                self.select_pending_session_start(request_id.clone(), cx);
            }
        } else {
            self.activate_workspace_tab(&tab.id.layout_key(), cx);
        }
    }

    pub(in crate::features) fn commit_session_strip_order(
        &mut self,
        group: &Option<String>,
        order: &[String],
        source: &str,
        cx: &mut Context<Self>,
    ) {
        let tabs = self.session_strip_tabs(group);
        let formal = order
            .iter()
            .filter(|key| {
                tabs.iter()
                    .any(|tab| !tab.id.is_start() && tab.id.layout_key() == **key)
            })
            .cloned()
            .collect::<Vec<_>>();
        if let Some(mut tree) = self.shell.terminal_window_tree() {
            for pair in formal.windows(2).rev() {
                tree.place_tab_before(&pair[0], &pair[1]);
            }
            self.shell.restore_terminal_window_tree(Some(tree));
        } else {
            let mut index = 0;
            for key in &formal {
                let leaves = self.tab_tree_session_ids(key);
                for leaf in &leaves {
                    self.session.move_session_to_index(leaf, index);
                    index += 1;
                }
            }
        }
        for (index, key) in order.iter().enumerate() {
            let tab = tabs.iter().find(|tab| tab.id.layout_key() == *key).unwrap();
            self.set_session_strip_tab_position(tab, index);
        }
        if let Some(tab) = tabs.iter().find(|tab| tab.id.layout_key() == source) {
            self.activate_session_strip_tab(tab, cx);
        }
        self.clear_session_tab_drag(cx);
        self.shell.request_session_tab_scroll_into_view();
        if self.session.restore_is_complete() {
            self.persist_open_tabs();
        } else if self.terminal_windows_is_multi_leaf() {
            self.persist_terminal_window_layout();
        }
        self.settle_session_start_tab_placements_if_idle();
        cx.notify();
    }

    pub(in crate::features) fn set_session_strip_tab_position(
        &mut self,
        tab: &SessionStripTab,
        index: usize,
    ) {
        match tab.id {
            SessionStripTabId::Start(sequence) => {
                let placement = self
                    .session
                    .start_pending_entries()
                    .map(|(_, pending)| pending)
                    .chain(
                        self.session
                            .start_failed_entries()
                            .map(|(_, failed)| &failed.pending),
                    )
                    .filter_map(|pending| pending.tab_placement)
                    .find(|placement| placement.request_sequence == sequence);
                if let Some(placement) = placement {
                    self.session.set_start_tab_position(placement, index);
                }
            }
            SessionStripTabId::Workspace(_) => self
                .session
                .set_workspace_tab_position(&tab.id.layout_key(), index),
        }
    }
}
