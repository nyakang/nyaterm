use std::time::{Duration, Instant};

const DEBOUNCE: Duration = Duration::from_millis(400);
const MAX_FAILURES: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayRequest {
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    pub scale: u32,
}

pub struct DisplayUpdates {
    generation: u64,
    desired: Option<DisplayRequest>,
    applied: Option<DisplayRequest>,
    due: Option<Instant>,
    failures: u32,
    ready: bool,
    in_flight: Option<DisplayRequest>,
}

impl DisplayUpdates {
    pub fn new(generation: u64) -> Self {
        Self {
            generation,
            desired: None,
            applied: None,
            due: None,
            failures: 0,
            ready: false,
            in_flight: None,
        }
    }

    pub fn observe(&mut self, width: u32, height: u32, scale: u32, now: Instant) {
        let request = DisplayRequest {
            generation: self.generation,
            width: width.clamp(200, 8192) & !1,
            height: height.clamp(200, 8192) & !1,
            scale: scale.clamp(100, 500),
        };
        if self.desired == Some(request) {
            return;
        }
        self.desired = Some(request);
        self.failures = 0;
        self.due = (self.applied != Some(request)).then_some(now + DEBOUNCE);
    }

    pub fn login_complete(&mut self, now: Instant) {
        self.ready = true;
        self.applied = None;
        self.failures = 0;
        self.due = self.desired.map(|_| now);
    }

    pub fn reconnecting(&mut self) {
        self.ready = false;
        self.applied = None;
        self.due = None;
        self.in_flight = None;
    }

    pub fn take_due(&mut self, now: Instant) -> Option<DisplayRequest> {
        if !self.ready || self.in_flight.is_some() || self.due.is_none_or(|due| now < due) {
            return None;
        }
        self.due = None;
        self.in_flight = self.desired;
        self.in_flight
    }

    /// Returns true once when dynamic resize has exhausted its bounded retries.
    pub fn complete(&mut self, request: DisplayRequest, success: bool, now: Instant) -> bool {
        if self.in_flight != Some(request) {
            return false;
        }
        self.in_flight = None;
        if !self.ready || request.generation != self.generation || self.desired != Some(request) {
            return false;
        }
        if success {
            self.applied = Some(request);
            self.failures = 0;
            return false;
        }
        self.failures += 1;
        if self.failures >= MAX_FAILURES {
            return true;
        }
        self.due = Some(now + Duration::from_millis(500 * (1 << (self.failures - 1))));
        false
    }
}

#[cfg(test)]
mod tests {
    use crate::display::DisplayUpdates;
    use std::time::{Duration, Instant};

    #[test]
    fn debounces_and_stops_retrying_one_rejected_viewport() {
        let mut updates = DisplayUpdates::new(1);
        let now = Instant::now();
        updates.observe(1000, 800, 150, now);
        assert!(updates.take_due(now + Duration::from_secs(1)).is_none());
        updates.login_complete(now);
        for attempt in 0..4 {
            let at = now + Duration::from_secs(attempt * 5);
            let request = updates.take_due(at).unwrap();
            assert_eq!(updates.complete(request, false, at), attempt == 3);
        }
        assert!(updates.take_due(now + Duration::from_secs(100)).is_none());
        updates.observe(1200, 800, 150, now + Duration::from_secs(100));
        assert!(updates.take_due(now + Duration::from_secs(100)).is_none());
        assert!(updates.take_due(now + Duration::from_secs(101)).is_some());
    }

    #[test]
    fn repeated_bounds_do_not_postpone_resize_and_stale_results_do_not_replace_new_layout() {
        let mut updates = DisplayUpdates::new(1);
        let now = Instant::now();
        updates.observe(1000, 800, 150, now);
        updates.login_complete(now);
        let initial = updates.take_due(now).unwrap();
        updates.complete(initial, true, now);
        updates.observe(1200, 800, 150, now);
        updates.observe(1200, 800, 150, now + Duration::from_millis(300));
        let request = updates.take_due(now + Duration::from_millis(400)).unwrap();
        assert!(updates.take_due(now + Duration::from_secs(5)).is_none());
        updates.observe(1400, 800, 150, now + Duration::from_secs(1));
        assert!(!updates.complete(request, true, now + Duration::from_secs(1)));
        let latest = updates.take_due(now + Duration::from_secs(2)).unwrap();
        assert_eq!(latest.width, 1400);
        updates.reconnecting();
        assert!(!updates.complete(latest, true, now + Duration::from_secs(2)));
        assert!(updates.take_due(now + Duration::from_secs(100)).is_none());
    }
}
