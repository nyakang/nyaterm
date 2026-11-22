use nyaterm_core::models::KeywordHighlightRule;
use std::collections::{HashMap, HashSet};

/// Derived indexes and changed IDs. The editor's vectors own the draft data.
#[derive(Default)]
pub(super) struct RuleDraftIndex {
    positions: HashMap<String, usize>,
    base_positions: HashMap<String, usize>,
    changed: HashSet<String>,
}

impl RuleDraftIndex {
    pub(super) fn rebuild(
        &mut self,
        base: &[KeywordHighlightRule],
        rules: &[KeywordHighlightRule],
    ) {
        self.positions = rules
            .iter()
            .enumerate()
            .map(|(i, r)| (r.id.clone(), i))
            .collect();
        self.base_positions = base
            .iter()
            .enumerate()
            .map(|(i, r)| (r.id.clone(), i))
            .collect();
        self.changed.clear();
        for id in self.positions.keys().chain(self.base_positions.keys()) {
            if self.get(rules, id) != self.base_positions.get(id).map(|&i| &base[i]) {
                self.changed.insert(id.clone());
            }
        }
    }

    pub(super) fn update(
        &mut self,
        base: &[KeywordHighlightRule],
        rules: &[KeywordHighlightRule],
        id: &str,
    ) {
        if self.get(rules, id) == self.base_positions.get(id).map(|&i| &base[i]) {
            self.changed.remove(id);
        } else {
            self.changed.insert(id.to_string());
        }
    }

    pub(super) fn get<'a>(
        &self,
        rules: &'a [KeywordHighlightRule],
        id: &str,
    ) -> Option<&'a KeywordHighlightRule> {
        self.positions.get(id).map(|&i| &rules[i])
    }

    pub(super) fn position(&self, id: &str) -> Option<usize> {
        self.positions.get(id).copied()
    }
    pub(super) fn contains(&self, id: &str) -> bool {
        self.positions.contains_key(id)
    }
    pub(super) fn dirty(&self) -> bool {
        !self.changed.is_empty()
    }
}

#[derive(Clone)]
pub(super) struct RuleSummary {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) enabled: bool,
    pub(super) source: &'static str,
    pub(super) search: String,
}

impl RuleSummary {
    pub(super) fn new(rule: &KeywordHighlightRule, source: &'static str) -> Self {
        Self {
            id: rule.id.clone(),
            name: rule.name.clone(),
            enabled: rule.enabled,
            source,
            search: rule.name.to_lowercase(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RuleDraftIndex;
    use nyaterm_core::models::KeywordHighlightRule;

    #[test]
    fn changed_ids_clear_after_reverting_edits_and_restoring_deleted_rules() {
        let base = vec![KeywordHighlightRule {
            id: "a".into(),
            name: "original".into(),
            ..Default::default()
        }];
        let mut rules = base.clone();
        let mut index = RuleDraftIndex::default();
        index.rebuild(&base, &rules);
        assert!(!index.dirty());
        rules[0].name = "edited".into();
        index.update(&base, &rules, "a");
        assert!(index.dirty());
        rules[0].name = "original".into();
        index.update(&base, &rules, "a");
        assert!(!index.dirty());
        index.rebuild(&base, &[]);
        assert!(index.dirty());
        index.rebuild(&base, &base);
        assert!(!index.dirty());
    }
}
