use std::collections::HashMap;

use zeroize::Zeroizing;

/// Session ownership is independent of focus and invalidated by competing input.
#[derive(Default)]
pub(super) struct StagedQuickCommands {
    next_id: u64,
    owners: HashMap<String, (u64, bool)>,
}

impl StagedQuickCommands {
    pub(super) fn begin(&mut self, targets: &[String]) -> u64 {
        self.next_id = self.next_id.checked_add(1).expect("quick command run id");
        for target in targets {
            self.owners.insert(target.clone(), (self.next_id, false));
        }
        self.next_id
    }

    pub(super) fn owns(&self, session: &str, id: u64) -> bool {
        self.owners
            .get(session)
            .is_some_and(|(owner, _)| *owner == id)
    }

    pub(super) fn mark_receiver(&mut self, session: &str, id: u64) {
        if let Some((owner, receiver)) = self.owners.get_mut(session)
            && *owner == id
        {
            *receiver = true;
        }
    }

    pub(super) fn cancel_receiver(&mut self, session: &str) {
        if self
            .owners
            .get(session)
            .is_some_and(|(_, receiver)| *receiver)
        {
            self.cancel(session);
        }
    }

    pub(super) fn cancel(&mut self, session: &str) {
        self.owners.remove(session);
    }

    pub(super) fn finish(&mut self, session: &str, id: u64) {
        if self.owns(session, id) {
            self.cancel(session);
        }
    }
}

#[derive(Default)]
pub(super) struct StagedCommandHistory {
    pending: Zeroizing<Vec<u8>>,
    suppressed: bool,
}

impl StagedCommandHistory {
    /// Any sensitive chunk suppresses the entire logical line, including its
    /// earlier prefix. Never feed this through the active editor's pending Enter.
    pub(super) fn push(&mut self, bytes: &[u8], sensitive: bool) -> Vec<String> {
        let mut submitted = Vec::new();
        for byte in bytes {
            self.suppressed |= sensitive;
            if matches!(byte, b'\r' | b'\n') {
                if !self.suppressed
                    && let Ok(text) = std::str::from_utf8(&self.pending)
                {
                    let command = text.trim();
                    if !command.is_empty() {
                        submitted.push(command.to_string());
                    }
                }
                self.pending = Zeroizing::new(Vec::new());
                self.suppressed = false;
            } else if self.suppressed {
                self.pending = Zeroizing::new(Vec::new());
            } else {
                self.pending.push(*byte);
            }
        }
        submitted
    }
}
