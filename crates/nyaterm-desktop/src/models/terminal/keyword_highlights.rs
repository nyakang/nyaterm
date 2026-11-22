//! Session-owned rules survive presentation replacement and window transfers.

use nyaterm_core::models::KeywordHighlightRule;
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct TerminalSessionHighlightState {
    rules: Arc<Vec<KeywordHighlightRule>>,
}

impl TerminalSessionHighlightState {
    pub(crate) fn rules(&self) -> &[KeywordHighlightRule] {
        &self.rules
    }

    pub(crate) fn snapshot(&self) -> Arc<Vec<KeywordHighlightRule>> {
        Arc::clone(&self.rules)
    }

    pub(crate) fn replace(&mut self, rules: Vec<KeywordHighlightRule>) -> bool {
        if self.rules.as_ref() == &rules {
            return false;
        }
        self.rules = Arc::new(rules);
        true
    }
}
