//! One owner for the current shell's file-access evidence.
use nyaterm_core::terminal::file_context::ShellFileContext;

#[derive(Default)]
pub(super) struct FileContextState {
    baseline: Option<ShellFileContext>,
    current: Option<ShellFileContext>,
    observed_sequence: u64,
    submitted_sequence: Option<u64>,
}
impl FileContextState {
    pub(super) fn note_input(&mut self, bytes: &[u8]) -> bool {
        let boundary = bytes
            .iter()
            .any(|byte| matches!(byte, b'\r' | b'\n' | 3 | 4));
        if boundary {
            self.invalidate();
        }
        boundary
    }
    pub(super) fn current(&self) -> Option<&ShellFileContext> {
        self.current.as_ref()
    }
    pub(super) fn invalidate(&mut self) {
        self.submitted_sequence = Some(self.observed_sequence);
        self.current = None;
    }
    pub(super) fn observe(&mut self, context: Option<&ShellFileContext>) {
        let Some(context) = context else {
            self.current = None;
            return;
        };
        if self.baseline.is_none() && self.submitted_sequence.is_some() {
            return;
        }
        let baseline = self.baseline.get_or_insert_with(|| context.clone());
        if !baseline.same_shell(context) || context.sequence < self.observed_sequence {
            self.current = None;
            return;
        }
        self.observed_sequence = context.sequence;
        if self
            .submitted_sequence
            .is_some_and(|submitted| context.sequence <= submitted)
        {
            self.current = None;
            return;
        }
        self.current = Some(context.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::FileContextState;
    use nyaterm_core::terminal::file_context::ShellFileContext;
    fn context() -> ShellFileContext {
        ShellFileContext {
            binding: "binding".into(),
            instance: 12,
            user: 1000,
            sequence: 1,
            home: "/home/user".into(),
            cwd: "/tmp".into(),
        }
    }
    #[test]
    fn only_a_new_prompt_from_the_original_identity_restores_access() {
        let mut state = FileContextState::default();
        let mut outer = context();
        state.observe(Some(&outer));
        assert!(state.current().is_some());
        state.invalidate();
        state.observe(Some(&outer));
        assert!(
            state.current().is_none(),
            "old frames cannot undo submission"
        );
        let mut inner = outer.clone();
        inner.instance += 1;
        inner.sequence = 100;
        state.observe(Some(&inner));
        assert!(state.current().is_none());
        outer.sequence += 1;
        state.observe(Some(&outer));
        assert!(state.current().is_some());
        outer.home = "/root".into();
        state.observe(Some(&outer));
        assert!(state.current().is_none());
    }
    #[test]
    fn missing_or_reconnected_evidence_does_not_reuse_the_old_identity() {
        let mut state = FileContextState::default();
        let mut context = context();
        state.observe(Some(&context));
        state.observe(None);
        assert!(state.current().is_none());
        context.binding = "other".into();
        state.observe(Some(&context));
        assert!(state.current().is_none());
        state = FileContextState::default();
        state.observe(Some(&context));
        assert!(state.current().is_some());
    }

    #[test]
    fn submissions_and_eof_require_a_fresh_prompt_even_for_unknown_commands() {
        for input in [
            b"sudo -i\r".as_slice(),
            b"su -\n",
            b"my-alias\r",
            b"ssh host\r",
            b"\x04",
            b"\x03",
            b"\xff\r",
        ] {
            let mut state = FileContextState::default();
            let mut outer = context();
            state.observe(Some(&outer));
            assert!(state.note_input(input));
            state.observe(Some(&outer));
            assert!(state.current().is_none());
            let mut switched = outer.clone();
            switched.user = 0;
            switched.sequence += 1;
            state.observe(Some(&switched));
            assert!(state.current().is_none());
            outer.sequence += 1;
            state.observe(Some(&outer));
            assert!(state.current().is_some());
        }
    }

    #[test]
    fn a_submission_before_initial_evidence_cannot_learn_a_foreign_shell() {
        let mut state = FileContextState::default();
        state.note_input(b"sudo -i\r");
        state.observe(Some(&context()));
        assert!(state.current().is_none());
    }
}
