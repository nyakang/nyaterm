use std::collections::HashSet;

use super::{TabDockEdge, TabDockZone, TerminalWindowNode, WorkspacePaneNode};
use crate::workspace_layout::pane::uuid_v4_like;

impl TerminalWindowNode {
    /// Place `tab_id` immediately before `before_tab_id` (same or other leaf).
    /// Mirrors Tauri TabBar reorder / move-tab-here within multi-leaf windows.
    pub fn place_tab_before(&mut self, tab_id: &str, before_tab_id: &str) -> bool {
        if tab_id.is_empty() || before_tab_id.is_empty() || tab_id == before_tab_id {
            return false;
        }
        if !self.contains_tab(tab_id) || !self.contains_tab(before_tab_id) {
            return false;
        }
        // Already immediately before target.
        if let Some(ids) = self.leaf_tab_ids_for_tab(before_tab_id)
            && let Some(pos) = ids.iter().position(|id| id == before_tab_id)
            && pos > 0
            && ids[pos - 1] == tab_id
        {
            return true;
        }
        let Some(next) = self.remove_tab(tab_id) else {
            return false;
        };
        *self = next;
        if !self.contains_tab(before_tab_id) {
            self.insert_tab_into_first_leaf(tab_id);
            return false;
        }
        self.insert_tab_before(before_tab_id, tab_id)
    }

    pub fn leaf_tab_ids_for_tab(&self, tab_id: &str) -> Option<Vec<String>> {
        match self {
            Self::Leaf { tab_ids, .. } => {
                if tab_ids.iter().any(|id| id == tab_id) {
                    Some(tab_ids.clone())
                } else {
                    None
                }
            }
            Self::Split { first, second, .. } => first
                .leaf_tab_ids_for_tab(tab_id)
                .or_else(|| second.leaf_tab_ids_for_tab(tab_id)),
        }
    }

    pub(super) fn insert_tab_before(&mut self, before_tab_id: &str, tab_id: &str) -> bool {
        match self {
            Self::Leaf {
                tab_ids,
                active_tab_id,
                ..
            } => {
                if let Some(pos) = tab_ids.iter().position(|id| id == before_tab_id) {
                    if !tab_ids.iter().any(|id| id == tab_id) {
                        tab_ids.insert(pos, tab_id.to_string());
                    }
                    *active_tab_id = Some(tab_id.to_string());
                    true
                } else {
                    false
                }
            }
            Self::Split { first, second, .. } => {
                first.insert_tab_before(before_tab_id, tab_id)
                    || second.insert_tab_before(before_tab_id, tab_id)
            }
        }
    }

    /// Dock `tab_id` onto `target_leaf_id` center (merge) or edge (split).
    pub fn dock_tab(&mut self, tab_id: &str, target_leaf_id: &str, zone: TabDockZone) -> bool {
        if tab_id.is_empty()
            || !self.contains_tab(tab_id)
            || self.leaf_tabs(target_leaf_id).is_none()
        {
            return false;
        }
        match zone {
            TabDockZone::Center => self.move_tab_to_leaf(tab_id, target_leaf_id),
            TabDockZone::Edge(edge) => self.dock_tab_to_edge(tab_id, target_leaf_id, edge),
        }
    }

    pub(super) fn dock_tab_to_edge(
        &mut self,
        tab_id: &str,
        target_leaf_id: &str,
        edge: TabDockEdge,
    ) -> bool {
        // If tab is already the sole occupant of the target leaf, nothing to do.
        if let Some((leaf_id, count)) = self.leaf_tab_count_for(tab_id)
            && leaf_id == target_leaf_id
            && count == 1
        {
            return false;
        }
        // Remove from current placement first.
        let Some(next) = self.remove_tab(tab_id) else {
            // Tree emptied — create target as detached only.
            *self = Self::leaf(vec![tab_id.to_string()], Some(tab_id.to_string()));
            return true;
        };
        *self = next;
        // If target disappeared (was emptied by remove), just insert on first leaf.
        if !self.leaf_ids().iter().any(|id| id == target_leaf_id) {
            self.insert_tab_into_first_leaf(tab_id);
            return true;
        }
        let detached = Self::leaf(vec![tab_id.to_string()], Some(tab_id.to_string()));
        self.replace_leaf_with_edge_split(target_leaf_id, edge, detached)
    }

    pub(super) fn leaf_tab_count_for(&self, tab_id: &str) -> Option<(String, usize)> {
        match self {
            Self::Leaf { id, tab_ids, .. } => {
                if tab_ids.iter().any(|id| id == tab_id) {
                    Some((id.clone(), tab_ids.len()))
                } else {
                    None
                }
            }
            Self::Split { first, second, .. } => first
                .leaf_tab_count_for(tab_id)
                .or_else(|| second.leaf_tab_count_for(tab_id)),
        }
    }

    pub(super) fn replace_leaf_with_edge_split(
        &mut self,
        target_leaf_id: &str,
        edge: TabDockEdge,
        detached: Self,
    ) -> bool {
        match self {
            Self::Leaf {
                id,
                tab_ids,
                active_tab_id,
            } => {
                if id != target_leaf_id {
                    return false;
                }
                let remaining = Self::Leaf {
                    id: id.clone(),
                    tab_ids: tab_ids.clone(),
                    active_tab_id: active_tab_id.clone(),
                };
                let (first, second) = if edge.first_is_dropped() {
                    (detached, remaining)
                } else {
                    (remaining, detached)
                };
                *self = Self::Split {
                    id: format!("tw-split-{}", uuid_v4_like()),
                    direction: edge.direction(),
                    ratio_percent: WorkspacePaneNode::DEFAULT_RATIO_PERCENT,
                    first: Box::new(first),
                    second: Box::new(second),
                };
                true
            }
            Self::Split { first, second, .. } => {
                first.replace_leaf_with_edge_split(target_leaf_id, edge, detached.clone())
                    || second.replace_leaf_with_edge_split(target_leaf_id, edge, detached)
            }
        }
    }

    pub(super) fn unique_tabs(tab_ids: Vec<String>) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for id in tab_ids {
            if seen.insert(id.clone()) {
                out.push(id);
            }
        }
        out
    }
}
