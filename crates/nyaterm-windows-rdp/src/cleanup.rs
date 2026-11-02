use std::time::{Duration, Instant};

use crate::lifecycle::{GRACEFUL_CLOSE_TIMEOUT, RETIREMENT_INTERVAL, RETIREMENT_TIMEOUT};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupAction {
    RequestClose,
    Wait,
    Destroy,
    Retain,
    Complete,
}

/// Owner-thread cleanup schedule. Deadlines are checked between calls; they
/// cannot interrupt a synchronous COM method which has stopped returning.
pub struct Cleanup {
    started: Instant,
    requested: bool,
    confirmed: bool,
    forced: bool,
    retry_at: Option<Instant>,
    complete: bool,
}

impl Cleanup {
    pub fn new(now: Instant) -> Self {
        Self {
            started: now,
            requested: false,
            confirmed: false,
            forced: false,
            retry_at: None,
            complete: false,
        }
    }

    pub fn next(&self, now: Instant) -> CleanupAction {
        if self.complete {
            return CleanupAction::Complete;
        }
        if !self.requested {
            return CleanupAction::RequestClose;
        }
        if self.forced
            && now.saturating_duration_since(self.started)
                >= GRACEFUL_CLOSE_TIMEOUT + RETIREMENT_TIMEOUT
        {
            return CleanupAction::Retain;
        }
        if self.retry_at.is_some_and(|due| now < due) {
            return CleanupAction::Wait;
        }
        if self.confirmed || now.saturating_duration_since(self.started) >= GRACEFUL_CLOSE_TIMEOUT {
            CleanupAction::Destroy
        } else {
            CleanupAction::Wait
        }
    }

    pub fn requested(&mut self, wait: bool) {
        self.requested = true;
        self.confirmed |= !wait;
    }

    pub fn confirmed(&mut self) {
        self.confirmed = true;
    }

    pub fn destroyed(&mut self, success: bool, now: Instant) {
        if success {
            self.complete = true;
        } else {
            self.forced = true;
            self.retry_at = Some(now + RETIREMENT_INTERVAL);
        }
    }

    pub fn elapsed(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.started)
    }
}

#[cfg(test)]
mod tests {
    use crate::cleanup::{Cleanup, CleanupAction};
    use std::time::{Duration, Instant};

    #[test]
    fn waits_for_confirmation_or_three_seconds_and_retries_busy_callbacks_once_per_second() {
        let now = Instant::now();
        let mut cleanup = Cleanup::new(now);
        assert_eq!(cleanup.next(now), CleanupAction::RequestClose);
        cleanup.requested(true);
        assert_eq!(
            cleanup.next(now + Duration::from_secs(2)),
            CleanupAction::Wait
        );
        let forced = now + Duration::from_secs(3);
        assert_eq!(cleanup.next(forced), CleanupAction::Destroy);
        cleanup.destroyed(false, forced); // e.g. invoked from inside a callback
        assert_eq!(
            cleanup.next(forced + Duration::from_millis(999)),
            CleanupAction::Wait
        );
        assert_eq!(
            cleanup.next(forced + Duration::from_secs(1)),
            CleanupAction::Destroy
        );
        assert_eq!(
            cleanup.next(now + Duration::from_secs(63)),
            CleanupAction::Retain
        );
    }

    #[test]
    fn confirmation_shortens_close_and_success_is_idempotent() {
        let now = Instant::now();
        let mut cleanup = Cleanup::new(now);
        cleanup.requested(true);
        cleanup.confirmed();
        assert_eq!(cleanup.next(now), CleanupAction::Destroy);
        cleanup.destroyed(true, now);
        cleanup.requested(true);
        assert_eq!(
            cleanup.next(now + Duration::from_secs(100)),
            CleanupAction::Complete
        );
    }
}
