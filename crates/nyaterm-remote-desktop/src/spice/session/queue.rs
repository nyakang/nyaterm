use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

use crate::QueueWaker;
use crate::heartbeat::Heartbeat;
use crate::spice::protocol::{SpiceControl, SpiceErrorKind, SpiceSessionState};
use crate::spice::session::SpiceEvent;

const EVENT_LIMIT: usize = 128;
const BYTE_LIMIT: usize = 32 * 1024 * 1024;

#[derive(Default)]
struct State {
    events: VecDeque<(SpiceEvent, usize)>,
    bytes: usize,
    closed: bool,
    ended: bool,
}

pub(super) struct EventQueue {
    state: Mutex<State>,
    available: Condvar,
    pub heartbeat: Arc<Heartbeat>,
    waker: Option<QueueWaker>,
}

impl EventQueue {
    pub fn new(waker: Option<QueueWaker>) -> Self {
        Self {
            state: Mutex::default(),
            available: Condvar::new(),
            heartbeat: Arc::default(),
            waker,
        }
    }

    pub fn push(&self, event: SpiceEvent, bytes: usize) -> bool {
        if bytes > BYTE_LIMIT {
            self.fail(SpiceErrorKind::ResourceLimit);
            return false;
        }
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        while !state.closed
            && !state.ended
            && (state.events.len() >= EVENT_LIMIT || state.bytes + bytes > BYTE_LIMIT)
        {
            self.heartbeat.blocked(true);
            state = self
                .available
                .wait(state)
                .unwrap_or_else(|p| p.into_inner());
            self.heartbeat.blocked(false);
        }
        if state.closed || state.ended {
            return false;
        }
        state.bytes += bytes;
        state.events.push_back((event, bytes));
        drop(state);
        self.wake();
        true
    }

    pub fn fail(&self, error: SpiceErrorKind) {
        self.end(SpiceControl::Error { error, fatal: true });
    }

    pub fn disconnected(&self) {
        self.end(SpiceControl::State {
            state: SpiceSessionState::Disconnected,
        });
    }

    fn end(&self, control: SpiceControl) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.closed || state.ended {
            return;
        }
        state.ended = true;
        // One terminal control has a reserved slot even behind a full queue.
        state.events.push_back((SpiceEvent::Control(control), 0));
        drop(state);
        self.heartbeat.fail_once();
        self.available.notify_all();
        self.wake();
    }

    pub fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.closed = true;
        state.events.clear();
        state.bytes = 0;
        drop(state);
        self.available.notify_all();
    }

    pub fn drain(&self, limit: usize) -> Vec<SpiceEvent> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut result = Vec::new();
        for _ in 0..limit.min(EVENT_LIMIT + 1) {
            let Some((event, bytes)) = state.events.pop_front() else {
                break;
            };
            state.bytes -= bytes;
            result.push(event);
        }
        drop(state);
        self.available.notify_all();
        result
    }

    fn wake(&self) {
        if let Some(waker) = &self.waker {
            // A consumer's notifier must not unwind through the pipe worker.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| waker()));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::spice::protocol::{SpiceControl, SpiceErrorKind};
    use crate::spice::session::SpiceEvent;
    use crate::spice::session::queue::EventQueue;
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    #[test]
    fn closing_unblocks_a_full_queue_and_terminal_error_is_emitted_once() {
        let queue = Arc::new(EventQueue::new(None));
        for sequence in 0..128 {
            assert!(queue.push(SpiceEvent::Control(SpiceControl::Pong { sequence }), 1));
        }
        let producer = queue.clone();
        let (send, recv) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            send.send(producer.push(SpiceEvent::Control(SpiceControl::Refresh), 1))
                .unwrap();
        });
        assert!(recv.recv_timeout(Duration::from_millis(20)).is_err());
        queue.fail(SpiceErrorKind::Timeout);
        queue.fail(SpiceErrorKind::HelperCrash);
        assert!(!recv.recv_timeout(Duration::from_secs(1)).unwrap());
        assert_eq!(queue.drain(1024).len(), 129);
        queue.close();
        assert!(!queue.push(SpiceEvent::Control(SpiceControl::Refresh), 1));
        worker.join().unwrap();
    }
}
