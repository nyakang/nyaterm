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

    pub(super) fn get<'a>(
        &self,
        rules: &'a [KeywordHighlightRule],
        id: &str,
    ) -> Option<&'a KeywordHighlightRule> {
        self.positions.get(id).map(|&i| &rules[i])
    }

    /// The ids whose content differs from the baseline.
    pub(super) fn changed_ids(&self) -> &HashSet<String> {
        &self.changed
    }
    pub(super) fn dirty(&self) -> bool {
        !self.changed.is_empty()
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
        index.rebuild(&base, &rules);
        assert!(index.dirty());
        assert!(index.changed_ids().contains("a"));
        rules[0].name = "original".into();
        index.rebuild(&base, &rules);
        assert!(!index.dirty());
        index.rebuild(&base, &[]);
        assert!(index.dirty());
        index.rebuild(&base, &base);
        assert!(!index.dirty());
    }

    /// A rule that only exists in the draft is a change on its own, even
    /// though the rules that both sides share stayed identical.
    #[test]
    fn a_new_rule_marks_the_draft_dirty() {
        let base = vec![KeywordHighlightRule {
            id: "a".into(),
            name: "original".into(),
            ..Default::default()
        }];
        let mut rules = base.clone();
        rules.push(KeywordHighlightRule {
            id: "b".into(),
            name: "new".into(),
            ..Default::default()
        });
        let mut index = RuleDraftIndex::default();
        index.rebuild(&base, &rules);
        assert!(index.dirty());
        assert!(index.changed_ids().contains("b"));
        assert!(!index.changed_ids().contains("a"));
    }
}
