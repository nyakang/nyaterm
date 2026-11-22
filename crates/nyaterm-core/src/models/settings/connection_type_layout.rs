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
}
