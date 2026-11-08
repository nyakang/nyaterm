//! session restore state and transitions.

#[derive(Default)]
pub(super) struct SessionRestoreState {
    pub(super) complete: bool,
}

impl SessionRestoreState {
    pub(in crate::features) fn is_complete(&self) -> bool {
        self.complete
    }

    pub(in crate::features) fn mark_complete(&mut self) -> bool {
        if self.complete {
            return false;
        }
        self.complete = true;
        true
    }
}
