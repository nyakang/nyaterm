//! One authoritative schedule per Store worker, with no GPUI or timer tasks.
use std::time::{Duration, Instant};

const STARTUP_DELAY: Duration = Duration::from_secs(30);
const IDLE_DELAY: Duration = Duration::from_secs(5);

pub(super) struct MaintenanceSchedule {
    requested_at: Option<Instant>,
    last_request_completed: Instant,
    completed: bool,
}

impl MaintenanceSchedule {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            requested_at: None,
            last_request_completed: now,
            completed: false,
        }
    }

    pub(super) fn request(&mut self, now: Instant) {
        if !self.completed && self.requested_at.is_none() {
            self.requested_at = Some(now);
        }
    }

    pub(super) fn request_completed(&mut self, now: Instant) {
        self.last_request_completed = now;
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        self.requested_at.map(|requested| {
            (requested + STARTUP_DELAY).max(self.last_request_completed + IDLE_DELAY)
        })
    }

    pub(super) fn finish(&mut self) {
        self.requested_at = None;
        self.completed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::MaintenanceSchedule;
    use std::time::{Duration, Instant};

    #[test]
    fn maintenance_waits_for_bootstrap_scheduling_and_thirty_seconds() {
        let now = Instant::now();
        let mut schedule = MaintenanceSchedule::new(now);
        assert!(schedule.deadline().is_none());
        schedule.request_completed(now + Duration::from_secs(20));
        schedule.request(now + Duration::from_secs(21));
        assert_eq!(schedule.deadline(), Some(now + Duration::from_secs(51)));
    }

    #[test]
    fn ordinary_requests_delay_maintenance_until_five_seconds_of_idle() {
        let now = Instant::now();
        let mut schedule = MaintenanceSchedule::new(now);
        schedule.request(now);
        schedule.request_completed(now + Duration::from_secs(29));
        assert_eq!(schedule.deadline(), Some(now + Duration::from_secs(34)));
        schedule.request_completed(now + Duration::from_secs(40));
        assert_eq!(schedule.deadline(), Some(now + Duration::from_secs(45)));
    }

    #[test]
    fn duplicate_requests_do_not_postpone_or_repeat_completed_maintenance() {
        let now = Instant::now();
        let mut schedule = MaintenanceSchedule::new(now);
        schedule.request(now);
        schedule.request(now + Duration::from_secs(10));
        assert_eq!(schedule.deadline(), Some(now + Duration::from_secs(30)));
        schedule.finish();
        schedule.request(now + Duration::from_secs(40));
        assert!(schedule.deadline().is_none());
    }
}
