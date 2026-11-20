//! One authoritative task slot per lane; host epochs never change on refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct Ticket {
    pub epoch: u64,
    id: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Lane {
    Refresh,
    Action,
    Logs,
    Details,
}
impl Lane {
    fn index(self) -> usize {
        self as usize
    }
}
struct Running {
    ticket: Ticket,
    key: String,
}
#[derive(Default)]
pub(super) struct Jobs {
    pub epoch: u64,
    next_id: u64,
    slots: [Option<Running>; 4],
    pub refresh_requested: bool,
}
impl Jobs {
    pub fn pending(&self, lane: Lane) -> bool {
        self.slots[lane.index()].is_some()
    }
    pub fn pending_key(&self, lane: Lane, key: &str) -> bool {
        self.slots[lane.index()]
            .as_ref()
            .is_some_and(|job| job.key == key)
    }
    pub fn start(&mut self, lane: Lane, key: String) -> Ticket {
        self.next_id += 1;
        let ticket = Ticket {
            epoch: self.epoch,
            id: self.next_id,
        };
        self.slots[lane.index()] = Some(Running { ticket, key });
        ticket
    }
    pub fn ticket(&self, lane: Lane) -> Option<Ticket> {
        self.slots[lane.index()].as_ref().map(|job| job.ticket)
    }
    pub fn finish(&mut self, lane: Lane, ticket: Ticket, key: &str) -> bool {
        let slot = &mut self.slots[lane.index()];
        if slot
            .as_ref()
            .is_some_and(|job| job.ticket == ticket && job.key == key)
        {
            *slot = None;
            true
        } else {
            false
        }
    }
    pub fn reset(&mut self) {
        self.epoch += 1;
        self.slots = [None, None, None, None];
        self.refresh_requested = false;
    }
}

#[cfg(test)]
mod tests {
    use super::{Jobs, Lane};
    #[test]
    fn refresh_does_not_invalidate_action_or_change_host_epoch() {
        let mut jobs = Jobs::default();
        let action = jobs.start(Lane::Action, String::new());
        let refresh = jobs.start(Lane::Refresh, String::new());
        assert_eq!(action.epoch, refresh.epoch);
        assert!(jobs.finish(Lane::Action, action, ""));
        assert!(jobs.pending(Lane::Refresh));
    }
    #[test]
    fn a_superseded_same_host_same_entry_reply_cannot_release_its_replacement() {
        let mut jobs = Jobs::default();
        let old = jobs.start(Lane::Details, "TCP|22|42".into());
        let current = jobs.start(Lane::Details, "TCP|22|42".into());
        assert_eq!(old.epoch, current.epoch);
        assert!(!jobs.finish(Lane::Details, old, "TCP|22|42"));
        assert_eq!(jobs.ticket(Lane::Details), Some(current));
        assert!(jobs.finish(Lane::Details, current, "TCP|22|42"));
    }

    #[test]
    fn old_host_and_old_same_entry_replies_cannot_finish_a_new_request() {
        let mut jobs = Jobs::default();
        let old = jobs.start(Lane::Logs, "system|sshd".into());
        jobs.reset();
        let new = jobs.start(Lane::Logs, "system|sshd".into());
        assert!(!jobs.finish(Lane::Logs, old, "system|sshd"));
        assert!(!jobs.finish(Lane::Logs, new, "gui/501|sshd"));
        assert!(jobs.finish(Lane::Logs, new, "system|sshd"));
    }
}
