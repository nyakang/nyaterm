//! Coalesced searches share FIFO ordering with accepted mutations.
use super::Request;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
const CAPACITY: usize = 64;
#[derive(Debug, PartialEq, Eq)]
pub(super) enum QueueError {
    Busy,
    Closed,
}
struct State {
    requests: VecDeque<Request>,
    clients: usize,
    closed: bool,
}
struct Queue {
    state: Mutex<State>,
    available: Condvar,
}
pub(super) struct Sender(Arc<Queue>);
pub(super) struct Receiver(Arc<Queue>);
pub(super) fn channel() -> (Sender, Receiver) {
    let queue = Arc::new(Queue {
        state: Mutex::new(State {
            requests: VecDeque::new(),
            clients: 1,
            closed: false,
        }),
        available: Condvar::new(),
    });
    (Sender(queue.clone()), Receiver(queue))
}
impl Clone for Sender {
    fn clone(&self) -> Self {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clients += 1;
        Self(self.0.clone())
    }
}
impl Drop for Sender {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.clients -= 1;
        if state.clients == 0 {
            self.0.available.notify_one();
        }
    }
}
impl Sender {
    pub(super) fn is_closed(&self) -> bool {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .closed
    }
    pub(super) fn try_send(&self, request: Request) -> Result<(), QueueError> {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed {
            return Err(QueueError::Closed);
        }
        if matches!(request, Request::Search(..)) {
            state
                .requests
                .retain(|old| !matches!(old, Request::Search(..)));
        }
        if state.requests.len() >= CAPACITY {
            return Err(QueueError::Busy);
        }
        state.requests.push_back(request);
        self.0.available.notify_one();
        Ok(())
    }
}
impl Receiver {
    pub(super) fn recv(&self) -> Option<Request> {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(request) = state.requests.pop_front() {
                return Some(request);
            }
            if state.clients == 0 {
                return None;
            }
            state = self
                .0
                .available
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
        }
    }
}
impl Drop for Receiver {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.closed = true;
        state.requests.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{CAPACITY, QueueError, channel};
    use crate::runtime::{CompletionQuery, Request};
    use futures::channel::oneshot;
    use nyaterm_core::completion::types::{
        CompletionMode, CompletionScope, ShellCommandContext, ShellDialect,
    };
    use std::sync::Arc;
    fn context() -> ShellCommandContext {
        ShellCommandContext {
            scope: CompletionScope::new(&["queue"]),
            dialect: ShellDialect::Bash,
            command_events: true,
        }
    }
    fn search() -> (
        Request,
        oneshot::Receiver<crate::runtime::CompletionSearchOutcome>,
    ) {
        let (tx, rx) = oneshot::channel();
        (
            Request::Search(
                CompletionQuery {
                    context: context(),
                    input: "fixture".into(),
                    mode: CompletionMode::Token,
                    quick: Arc::from([]),
                    min: 1,
                    max: 64,
                    fill_policy: None,
                },
                0,
                0,
                tx,
            ),
            rx,
        )
    }
    #[test]
    fn replacing_searches_preserves_the_order_of_accepted_mutations() {
        let (tx, rx) = channel();
        let (old, cancelled) = search();
        assert!(tx.try_send(old).is_ok());
        assert!(
            tx.try_send(Request::Observe(context(), "first".into(), true))
                .is_ok()
        );
        let (delete, _) = oneshot::channel();
        assert!(
            tx.try_send(Request::Delete("second".into(), delete))
                .is_ok()
        );
        assert!(tx.try_send(search().0).is_ok());
        assert!(futures::executor::block_on(cancelled).is_err());
        assert!(matches!(rx.recv(), Some(Request::Observe(..))));
        assert!(matches!(rx.recv(), Some(Request::Delete(..))));
        assert!(matches!(rx.recv(), Some(Request::Search(..))));
        drop(tx);
        assert!(rx.recv().is_none());
    }
    #[test]
    fn full_mutation_queue_reports_pressure_and_closed_receiver_rejects_requests() {
        let (tx, rx) = channel();
        for _ in 0..CAPACITY {
            assert!(
                tx.try_send(Request::Observe(context(), "fixture".into(), true))
                    .is_ok()
            );
        }
        assert_eq!(tx.try_send(search().0), Err(QueueError::Busy));
        drop(rx);
        assert_eq!(tx.try_send(search().0), Err(QueueError::Closed));
    }
}
