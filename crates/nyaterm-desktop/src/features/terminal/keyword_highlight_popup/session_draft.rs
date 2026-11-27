//! Validation identities belong to the editor, not the applied terminal rules.
use std::collections::{HashMap, HashSet};

use nyaterm_core::KeywordHighlightRule;
use nyaterm_terminal_gpui::keywords::validation::{
    KeywordPatternDiagnostic, validate_keyword_highlight_patterns,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MatchInput {
    patterns: Vec<String>,
    case_sensitive: bool,
    whole_word: bool,
    use_regex: bool,
}

impl From<&KeywordHighlightRule> for MatchInput {
    fn from(rule: &KeywordHighlightRule) -> Self {
        Self {
            patterns: rule.patterns.clone(),
            case_sensitive: rule.case_sensitive,
            whole_word: rule.whole_word,
            use_regex: rule.use_regex,
        }
    }
}

impl MatchInput {
    fn matches(&self, rule: &KeywordHighlightRule) -> bool {
        self.patterns == rule.patterns
            && self.case_sensitive == rule.case_sensitive
            && self.whole_word == rule.whole_word
            && self.use_regex == rule.use_regex
    }

    pub(super) fn validate(&self) -> Option<KeywordPatternDiagnostic> {
        validate_keyword_highlight_patterns(
            &self.patterns,
            self.case_sensitive,
            self.whole_word,
            self.use_regex,
        )
        .into_iter()
        .next()
    }
}

struct Entry {
    input: MatchInput,
    generation: u64,
    result: Option<Option<KeywordPatternDiagnostic>>,
}

#[derive(Default)]
pub(super) struct SessionValidation {
    entries: HashMap<String, Entry>,
    generation: u64,
}

impl SessionValidation {
    /// Existing applied rules are a baseline, not new input to compile on open.
    pub(super) fn from_applied(rules: &[KeywordHighlightRule]) -> Self {
        let mut state = Self::default();
        for rule in rules {
            state.entries.insert(
                rule.id.clone(),
                Entry {
                    input: MatchInput::from(rule),
                    generation: 0,
                    result: Some(None),
                },
            );
        }
        state
    }

    /// Return only changed matching inputs. Cosmetic changes reuse validation.
    pub(super) fn reconcile(
        &mut self,
        rules: &[KeywordHighlightRule],
    ) -> Vec<(String, u64, MatchInput)> {
        let ids: HashSet<_> = rules.iter().map(|rule| rule.id.as_str()).collect();
        self.entries.retain(|id, _| ids.contains(id.as_str()));
        let mut jobs = Vec::new();
        for rule in rules {
            if self
                .entries
                .get(&rule.id)
                .is_some_and(|entry| entry.input.matches(rule))
            {
                continue;
            }
            let input = MatchInput::from(rule);
            self.generation = self.generation.wrapping_add(1);
            let empty = input
                .patterns
                .iter()
                .all(|pattern| pattern.trim().is_empty());
            self.entries.insert(
                rule.id.clone(),
                Entry {
                    input: input.clone(),
                    generation: self.generation,
                    result: empty.then_some(None),
                },
            );
            if !empty {
                jobs.push((rule.id.clone(), self.generation, input));
            }
        }
        jobs
    }

    pub(super) fn pending(&self) -> Vec<(String, u64, MatchInput)> {
        self.entries
            .iter()
            .filter(|(_, entry)| entry.result.is_none())
            .map(|(id, entry)| (id.clone(), entry.generation, entry.input.clone()))
            .collect()
    }

    pub(super) fn finish(
        &mut self,
        id: &str,
        generation: u64,
        error: Option<KeywordPatternDiagnostic>,
    ) -> bool {
        let Some(entry) = self
            .entries
            .get_mut(id)
            .filter(|entry| entry.generation == generation)
        else {
            return false;
        };
        entry.result = Some(error);
        true
    }

    pub(super) fn valid(&self, id: &str) -> bool {
        self.entries
            .get(id)
            .is_some_and(|entry| entry.result == Some(None))
    }

    pub(super) fn has_pending(&self) -> bool {
        self.entries.values().any(|entry| entry.result.is_none())
    }

    pub(super) fn has_unapplied(&self) -> bool {
        self.entries
            .values()
            .any(|entry| entry.result != Some(None))
    }

    pub(super) fn error<'a>(
        &'a self,
        rules: &'a [KeywordHighlightRule],
    ) -> Option<(&'a KeywordHighlightRule, &'a KeywordPatternDiagnostic)> {
        rules.iter().find_map(|rule| {
            self.entries
                .get(&rule.id)?
                .result
                .as_ref()?
                .as_ref()
                .map(|error| (rule, error))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::SessionValidation;
    use nyaterm_core::KeywordHighlightRule;

    fn rule() -> KeywordHighlightRule {
        KeywordHighlightRule {
            id: "rule".into(),
            patterns: vec!["ERROR".into()],
            ..Default::default()
        }
    }

    #[test]
    fn cosmetic_edits_do_not_schedule_validation_even_when_pattern_is_invalid() {
        let mut rules = vec![rule()];
        let mut state = SessionValidation::from_applied(&rules);
        rules[0].name = "renamed".into();
        rules[0].bg_color_dark = "#112233".into();
        assert!(state.reconcile(&rules).is_empty());
        rules[0].patterns = vec!["[".into()];
        let jobs = state.reconcile(&rules);
        assert_eq!(jobs.len(), 1);
        let (id, generation, input) = &jobs[0];
        assert!(state.finish(id, *generation, input.validate()));
        rules[0].name = "renamed again".into();
        assert!(state.reconcile(&rules).is_empty());
        assert!(state.error(&rules).is_some());
        assert!(state.has_unapplied());
    }

    #[test]
    fn late_results_cannot_replace_new_input_or_resurrect_deleted_rules() {
        let mut rules = vec![rule()];
        let mut state = SessionValidation::default();
        let old = state.reconcile(&rules).pop().unwrap();
        rules[0].patterns = vec!["warning".into()];
        let new = state.reconcile(&rules).pop().unwrap();
        assert!(!state.finish(&old.0, old.1, old.2.validate()));
        assert!(state.has_pending());
        assert!(state.finish(&new.0, new.1, new.2.validate()));
        assert!(state.valid(&rules[0].id));
        state.reconcile(&[]);
        assert!(!state.finish(&new.0, new.1, None));
    }

    #[test]
    fn thousand_rule_metadata_edit_and_reorder_schedule_no_compilations() {
        let mut rules: Vec<_> = (0..1000)
            .map(|i| KeywordHighlightRule {
                id: i.to_string(),
                ..rule()
            })
            .collect();
        let mut state = SessionValidation::from_applied(&rules);
        rules[500].name = "renamed".into();
        rules[500].color_dark = "#aabbcc".into();
        rules.reverse();
        assert!(state.reconcile(&rules).is_empty());
        rules[500].whole_word = true;
        assert_eq!(state.reconcile(&rules).len(), 1);
    }
    #[test]
    #[ignore = "manual metadata-edit timing probe"]
    fn metadata_edit_timing_probe() {
        for count in [10, 1000] {
            let mut rules: Vec<_> = (0..count)
                .map(|i| KeywordHighlightRule {
                    id: i.to_string(),
                    ..rule()
                })
                .collect();
            let mut state = SessionValidation::from_applied(&rules);
            let started = std::time::Instant::now();
            for i in 0..1000 {
                rules[0].name = i.to_string();
                assert!(state.reconcile(&rules).is_empty());
            }
            eprintln!(
                "{count} rules: 1000 metadata edits in {:?}, no regex compilations",
                started.elapsed()
            );
        }
    }
}
