//! Connection-editor presentation only; hidden types remain usable connections.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionTypeLayout {
    #[serde(default)]
    pub order: Vec<String>,
    #[serde(default)]
    pub hidden: Vec<String>,
    /// Retain extensions written by versions with additional presentation options.
    #[serde(flatten)]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

impl ConnectionTypeLayout {
    /// The caller supplies its supported catalog in default order. Newly supported
    /// types append to the saved order; unavailable IDs survive in storage only.
    pub fn ordered<'a>(&self, catalog: &'a [&'a str]) -> Vec<&'a str> {
        let mut result = Vec::with_capacity(catalog.len());
        for id in self
            .order
            .iter()
            .map(String::as_str)
            .chain(catalog.iter().copied())
        {
            if let Some(known) = catalog.iter().copied().find(|known| *known == id)
                && !result.contains(&known)
            {
                result.push(known);
            }
        }
        result
    }

    /// Recover an empty visible selection without rewriting the saved document.
    pub fn visible<'a>(&self, catalog: &'a [&'a str]) -> Vec<&'a str> {
        let mut result: Vec<_> = self
            .ordered(catalog)
            .into_iter()
            .filter(|id| !self.hidden.iter().any(|hidden| hidden == id))
            .collect();
        if result.is_empty()
            && let Some(first) = catalog.first()
        {
            result.push(*first);
        }
        result
    }

    pub fn set_visible(&mut self, catalog: &[&str], id: &str, visible: bool) -> bool {
        if !catalog.contains(&id) {
            return false;
        }
        let current = self.visible(catalog);
        if !visible && current.len() == 1 && current.contains(&id) {
            return false;
        }
        self.hidden.retain(|hidden| hidden != id);
        if !visible {
            self.hidden.push(id.to_owned());
        }
        true
    }

    pub fn is_default_for(&self, catalog: &[&str]) -> bool {
        self.ordered(catalog) == catalog
            && !catalog
                .iter()
                .any(|id| self.hidden.iter().any(|hidden| hidden == id))
    }

    pub fn move_to(&mut self, catalog: &[&str], id: &str, target_index: usize) -> bool {
        let mut ordered = self.ordered(catalog);
        let Some(from_index) = ordered.iter().position(|known| *known == id) else {
            return false;
        };
        if ordered.is_empty() {
            return false;
        }
        let to_index = target_index.min(ordered.len() - 1);
        if from_index == to_index {
            return false;
        }
        let moved = ordered.remove(from_index);
        ordered.insert(to_index, moved);

        for known in catalog {
            if !self.order.iter().any(|item| item == known) {
                self.order.push((*known).into());
            }
        }
        let mut seen_known = Vec::with_capacity(catalog.len());
        self.order.retain(|item| {
            if catalog.contains(&item.as_str()) {
                if seen_known.contains(&item.as_str()) {
                    false
                } else if let Some(canonical) = catalog
                    .iter()
                    .copied()
                    .find(|known| *known == item.as_str())
                {
                    seen_known.push(canonical);
                    true
                } else {
                    true
                }
            } else {
                true
            }
        });
        let mut ordered_iter = ordered.into_iter();
        for item in &mut self.order {
            if catalog.contains(&item.as_str())
                && let Some(next_known) = ordered_iter.next()
            {
                *item = next_known.into();
            }
        }
        true
    }

    pub fn move_relative(&mut self, catalog: &[&str], id: &str, down: bool) -> bool {
        let ordered = self.ordered(catalog);
        let Some(index) = ordered.iter().position(|known| *known == id) else {
            return false;
        };
        let neighbor = if down {
            index.checked_add(1)
        } else {
            index.checked_sub(1)
        };
        let Some(other) = neighbor.and_then(|index| ordered.get(index)).copied() else {
            return false;
        };
        for known in catalog {
            if !self.order.iter().any(|item| item == known) {
                self.order.push((*known).into());
            }
        }
        // Swap all occurrences so duplicate IDs cannot undo the effective move.
        for item in &mut self.order {
            if item == id {
                *item = other.into();
            } else if item == other {
                *item = id.into();
            }
        }
        true
    }

    pub fn reset_known(&mut self, catalog: &[&str]) {
        self.order.retain(|id| !catalog.contains(&id.as_str()));
        self.hidden.retain(|id| !catalog.contains(&id.as_str()));
    }
}

#[cfg(test)]
mod tests {
    use super::ConnectionTypeLayout;
    use serde_json::json;

    #[test]
    fn ordering_preserves_extensions_and_appends_new_protocols() {
        let raw = json!({"order": ["vnc", "spice", "ssh", "vnc"],
            "hidden": ["spice"], "future": {"style": 2}});
        let layout: ConnectionTypeLayout = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(
            layout.visible(&["ssh", "vnc", "ftp"]),
            ["vnc", "ssh", "ftp"]
        );
        assert_eq!(
            layout.ordered(&["ssh", "vnc", "ftp", "spice"]),
            ["vnc", "spice", "ssh", "ftp"]
        );
        assert_eq!(serde_json::to_value(layout).unwrap(), raw);
    }

    #[test]
    fn last_visible_protocol_is_protected_and_empty_selection_recovers_without_mutation() {
        let mut layout = ConnectionTypeLayout::default();
        assert!(layout.set_visible(&["ssh", "vnc"], "ssh", false));
        assert!(!layout.set_visible(&["ssh", "vnc"], "vnc", false));
        layout.hidden.push("vnc".into());
        let before = layout.clone();
        assert_eq!(layout.visible(&["ssh", "vnc"]), ["ssh"]);
        assert_eq!(layout, before);
        assert!(!layout.set_visible(&["ssh", "vnc"], "ssh", false));
    }

    #[test]
    fn moving_and_resetting_leave_unknown_preferences_intact() {
        let mut layout: ConnectionTypeLayout = serde_json::from_value(json!({
            "order": ["vnc", "spice", "ssh", "vnc"], "hidden": ["ssh", "spice"], "future": true
        }))
        .unwrap();
        assert!(layout.move_relative(&["ssh", "vnc", "ftp"], "ftp", false));
        assert_eq!(
            layout.ordered(&["ssh", "vnc", "ftp"]),
            ["vnc", "ftp", "ssh"]
        );
        assert!(!layout.move_relative(&["ssh", "vnc", "ftp"], "vnc", false));
        layout.reset_known(&["ssh", "vnc", "ftp"]);
        assert_eq!(
            layout.visible(&["ssh", "vnc", "ftp"]),
            ["ssh", "vnc", "ftp"]
        );
        assert_eq!(layout.order, ["spice"]);
        assert_eq!(layout.hidden, ["spice"]);
        assert_eq!(layout.extensions["future"], true);
    }

    #[test]
    fn move_to_reorders_across_multiple_slots_and_preserves_unknown_ids() {
        let catalog = ["ssh", "local", "telnet", "ftp"];
        let mut layout: ConnectionTypeLayout = serde_json::from_value(json!({
            "order": ["ssh", "future_proto", "local", "ssh"],
            "hidden": []
        }))
        .unwrap();
        assert!(layout.is_default_for(&catalog));

        // Move "ftp" from index 3 directly to index 1.
        assert!(layout.move_to(&catalog, "ftp", 1));
        assert_eq!(layout.ordered(&catalog), ["ssh", "ftp", "local", "telnet"]);
        assert!(!layout.is_default_for(&catalog));
        // Same-index move is a no-op.
        assert!(!layout.move_to(&catalog, "ftp", 1));

        // Move "ftp" back to the end restores the default effective order.
        assert!(layout.move_to(&catalog, "ftp", 99));
        assert_eq!(layout.ordered(&catalog), catalog);
        assert!(layout.is_default_for(&catalog));

        // Hiding a known type marks layout non-default; unknown hidden types do not.
        layout.hidden.push("future_proto".into());
        assert!(layout.is_default_for(&catalog));
        assert!(layout.set_visible(&catalog, "telnet", false));
        assert!(!layout.is_default_for(&catalog));

        // Unknown entries remain intact after reset.
        layout.reset_known(&catalog);
        assert_eq!(layout.order, ["future_proto"]);
        assert_eq!(layout.hidden, ["future_proto"]);
        assert!(layout.is_default_for(&catalog));
    }
}
