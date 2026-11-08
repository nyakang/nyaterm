use super::TerminalWindowNode;
#[cfg(any(test, feature = "test-support"))]
use super::{SmartSplitMode, WorkspacePaneNode, WorkspaceSplitDirection};
use crate::workspace_layout::pane::uuid_v4_like;

impl TerminalWindowNode {
    pub fn leaf(tab_ids: Vec<String>, active_tab_id: Option<String>) -> Self {
        let tab_ids = Self::unique_tabs(tab_ids);
        let active_tab_id = active_tab_id
            .filter(|id| tab_ids.iter().any(|tab| tab == id))
            .or_else(|| tab_ids.first().cloned());
        Self::Leaf {
            id: format!("tw-leaf-{}", uuid_v4_like()),
            tab_ids,
            active_tab_id,
        }
    }

    /// Build a balanced multi-leaf tree tiling every tab into its own leaf (Tauri smartSplit).
    #[cfg(any(test, feature = "test-support"))]
    pub fn build_smart_split_layout(tab_ids: &[String], mode: SmartSplitMode) -> Option<Self> {
        let tab_ids = Self::unique_tabs(tab_ids.to_vec());
        if tab_ids.is_empty() {
            return None;
        }
        if tab_ids.len() == 1 {
            return Some(Self::leaf(tab_ids, None));
        }
        let (direction, alternate) = match mode {
            SmartSplitMode::Auto => (WorkspaceSplitDirection::Horizontal, true),
            SmartSplitMode::Horizontal => (WorkspaceSplitDirection::Horizontal, false),
            SmartSplitMode::Vertical => (WorkspaceSplitDirection::Vertical, false),
        };
        Some(Self::build_balanced_tree(&tab_ids, direction, alternate))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn build_balanced_tree(
        tab_ids: &[String],
        direction: WorkspaceSplitDirection,
        alternate: bool,
    ) -> Self {
        if tab_ids.len() == 1 {
            return Self::leaf(tab_ids.to_vec(), tab_ids.first().cloned());
        }
        let mid = tab_ids.len().div_ceil(2);
        let mid = mid.max(1).min(tab_ids.len() - 1);
        let next_direction = if alternate {
            match direction {
                WorkspaceSplitDirection::Horizontal => WorkspaceSplitDirection::Vertical,
                WorkspaceSplitDirection::Vertical => WorkspaceSplitDirection::Horizontal,
            }
        } else {
            direction
        };
        let ratio_f = mid as f64 / tab_ids.len() as f64;
        let ratio_percent = ((ratio_f * 100.0).round() as u8).clamp(
            WorkspacePaneNode::MIN_RATIO_PERCENT,
            WorkspacePaneNode::MAX_RATIO_PERCENT,
        );
        Self::Split {
            id: format!("tw-split-{}", uuid_v4_like()),
            direction,
            ratio_percent,
            first: Box::new(Self::build_balanced_tree(
                &tab_ids[..mid],
                next_direction,
                alternate,
            )),
            second: Box::new(Self::build_balanced_tree(
                &tab_ids[mid..],
                next_direction,
                alternate,
            )),
        }
    }

    pub fn collect_tab_ids(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_tab_ids_into(&mut out);
        out
    }

    pub(super) fn collect_tab_ids_into(&self, out: &mut Vec<String>) {
        match self {
            Self::Leaf { tab_ids, .. } => out.extend(tab_ids.iter().cloned()),
            Self::Split { first, second, .. } => {
                first.collect_tab_ids_into(out);
                second.collect_tab_ids_into(out);
            }
        }
    }

    pub fn contains_tab(&self, tab_id: &str) -> bool {
        match self {
            Self::Leaf { tab_ids, .. } => tab_ids.iter().any(|id| id == tab_id),
            Self::Split { first, second, .. } => {
                first.contains_tab(tab_id) || second.contains_tab(tab_id)
            }
        }
    }

    pub fn replace_tab_id(&mut self, old_id: &str, new_id: &str) -> bool {
        match self {
            Self::Leaf {
                tab_ids,
                active_tab_id,
                ..
            } => {
                let Some(index) = tab_ids.iter().position(|id| id == old_id) else {
                    return false;
                };
                if tab_ids.iter().all(|id| id != new_id) {
                    tab_ids[index] = new_id.to_string();
                } else {
                    tab_ids.remove(index);
                }
                if active_tab_id.as_deref() == Some(old_id) {
                    *active_tab_id = Some(new_id.to_string());
                }
                true
            }
            Self::Split { first, second, .. } => {
                first.replace_tab_id(old_id, new_id) || second.replace_tab_id(old_id, new_id)
            }
        }
    }

    #[cfg(test)]
    pub fn active_tabs(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_active_tabs(&mut out);
        out
    }

    #[cfg(test)]
    pub(super) fn collect_active_tabs(&self, out: &mut Vec<String>) {
        match self {
            Self::Leaf { active_tab_id, .. } => {
                if let Some(id) = active_tab_id {
                    out.push(id.clone());
                }
            }
            Self::Split { first, second, .. } => {
                first.collect_active_tabs(out);
                second.collect_active_tabs(out);
            }
        }
    }

    pub fn set_active_tab(&mut self, tab_id: &str) -> bool {
        match self {
            Self::Leaf {
                tab_ids,
                active_tab_id,
                ..
            } => {
                if tab_ids.iter().any(|id| id == tab_id) {
                    *active_tab_id = Some(tab_id.to_string());
                    true
                } else {
                    false
                }
            }
            Self::Split { first, second, .. } => {
                first.set_active_tab(tab_id) || second.set_active_tab(tab_id)
            }
        }
    }

    pub fn ensure_tab(&mut self, tab_id: &str, preferred_leaf: Option<&str>) {
        if self.contains_tab(tab_id) {
            return;
        }
        if let Some(leaf_id) = preferred_leaf
            && self.insert_tab_into_leaf(leaf_id, tab_id)
        {
            return;
        }
        // Default: first leaf.
        self.insert_tab_into_first_leaf(tab_id);
    }

    pub(super) fn insert_tab_into_first_leaf(&mut self, tab_id: &str) {
        match self {
            Self::Leaf {
                tab_ids,
                active_tab_id,
                ..
            } => {
                if !tab_ids.iter().any(|id| id == tab_id) {
                    tab_ids.push(tab_id.to_string());
                }
                if active_tab_id.is_none() {
                    *active_tab_id = Some(tab_id.to_string());
                }
            }
            Self::Split { first, .. } => first.insert_tab_into_first_leaf(tab_id),
        }
    }

    pub(super) fn insert_tab_into_leaf(&mut self, leaf_id: &str, tab_id: &str) -> bool {
        match self {
            Self::Leaf {
                id,
                tab_ids,
                active_tab_id,
            } => {
                if id != leaf_id {
                    return false;
                }
                if !tab_ids.iter().any(|id| id == tab_id) {
                    tab_ids.push(tab_id.to_string());
                }
                *active_tab_id = Some(tab_id.to_string());
                true
            }
            Self::Split { first, second, .. } => {
                first.insert_tab_into_leaf(leaf_id, tab_id)
                    || second.insert_tab_into_leaf(leaf_id, tab_id)
            }
        }
    }
}
