//! A bounded worker mailbox with one UI output event retained on saturation.
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};

pub struct TransferInbox<T> {
    sender: Option<SyncSender<T>>,
    deferred: VecDeque<T>,
}

impl<T> TransferInbox<T> {
    pub fn new(capacity: usize) -> (Self, Receiver<T>) {
        let (sender, receiver) = sync_channel(capacity);
        (
            Self {
                sender: Some(sender),
                deferred: VecDeque::new(),
            },
            receiver,
        )
    }

    pub fn send(&mut self, command: T) {
        let Some(sender) = &self.sender else {
            return;
        };
        if !self.deferred.is_empty() {
            self.deferred.push_back(command);
            return;
        }
        if let Err(TrySendError::Full(command)) = sender.try_send(command) {
            self.deferred.push_back(command);
        }
    }

    // Called before consuming the next source event. The event pump retains
    // that event on false, propagating pressure to the bounded transport queue.
    pub fn ready(&mut self) -> bool {
        let Some(sender) = &self.sender else {
            return true;
        };
        while let Some(command) = self.deferred.pop_front() {
            match sender.try_send(command) {
                Ok(()) => {}
                Err(TrySendError::Full(command)) => {
                    self.deferred.push_front(command);
                    return false;
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.deferred.clear();
                    break;
                }
            }
        }
        true
    }

    pub fn close(&mut self) {
        self.sender.take();
        self.deferred.clear();
    }

    pub fn take_deferred(&mut self) -> VecDeque<T> {
        std::mem::take(&mut self.deferred)
    }
}

#[cfg(test)]
mod tests {
    use super::TransferInbox;

    #[test]
    fn saturation_preserves_order_and_close_does_not_wait_for_capacity() {
        let (mut inbox, receiver) = TransferInbox::new(1);
        inbox.send(1);
        inbox.send(2);
        inbox.send(3);
        assert!(!inbox.ready());
        assert_eq!(receiver.recv().unwrap(), 1);
        assert!(!inbox.ready());
        assert_eq!(receiver.recv().unwrap(), 2);
        assert!(inbox.ready());
        assert_eq!(receiver.recv().unwrap(), 3);
        inbox.send(4);
        inbox.close();
        assert_eq!(receiver.recv().unwrap(), 4);
        assert!(receiver.recv().is_err());
    }
}
