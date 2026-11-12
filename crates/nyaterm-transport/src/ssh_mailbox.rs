//! SSH input accounting follows bytes through deferred open and shell injection.
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use tokio::sync::{Notify, mpsc};

use crate::SshCommand;
use crate::writer_mailbox::{BYTE_LIMIT, ITEM_LIMIT};

#[derive(Default)]
struct State {
    bytes: usize,
    peak_bytes: usize,
    items: usize,
    stopped: bool,
    resize: Option<(u16, u16, u16, u16)>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    space: Condvar,
    control: Notify,
}

#[derive(Clone)]
pub(crate) struct Sender {
    data: mpsc::UnboundedSender<PendingWrite>,
    shared: Arc<Shared>,
}

pub(crate) struct Receiver {
    data: mpsc::UnboundedReceiver<PendingWrite>,
    shared: Arc<Shared>,
}

pub(crate) struct PendingWrite {
    pub(crate) bytes: Vec<u8>,
    _permit: Permit,
}

struct Permit {
    bytes: usize,
    shared: Arc<Shared>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        state.bytes -= self.bytes;
        state.items -= 1;
        drop(state);
        self.shared.space.notify_all();
    }
}

pub(crate) fn channel() -> (Sender, Receiver) {
    let (tx, rx) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared::default());
    (
        Sender {
            data: tx,
            shared: shared.clone(),
        },
        Receiver { data: rx, shared },
    )
}

impl Sender {
    pub(crate) fn write(&self, bytes: Vec<u8>) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| io::Error::other("SSH input poisoned"))?;
        check(&state, bytes.len())?;
        state.bytes += bytes.len();
        state.peak_bytes = state.peak_bytes.max(state.bytes);
        state.items += 1;
        let write = PendingWrite {
            _permit: Permit {
                bytes: bytes.len(),
                shared: self.shared.clone(),
            },
            bytes,
        };
        // Drop a rejected permit outside the accounting lock.
        let result = self.data.send(write);
        drop(state);
        result.map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "SSH worker stopped"))
    }

    pub(crate) fn peak_bytes(&self) -> usize {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .peak_bytes
    }

    pub(crate) fn wait_capacity(&self, bytes: usize, cancelled: &AtomicBool) -> io::Result<()> {
        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| io::Error::other("SSH input poisoned"))?;
        loop {
            if cancelled.load(Ordering::Acquire) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            match check(&state, bytes) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    state = self
                        .shared
                        .space
                        .wait_timeout(state, Duration::from_millis(25))
                        .map_err(|_| io::Error::other("SSH input poisoned"))?
                        .0;
                }
                result => return result,
            }
        }
    }

    pub(crate) fn resize(&self, cols: u16, rows: u16, width: u16, height: u16) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .resize = Some((cols, rows, width, height));
        self.shared.control.notify_one();
    }

    pub(crate) fn close(&self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .stopped = true;
        self.shared.space.notify_all();
        self.shared.control.notify_one();
    }
}

fn check(state: &State, bytes: usize) -> io::Result<()> {
    if state.stopped {
        return Err(io::ErrorKind::BrokenPipe.into());
    }
    if bytes > BYTE_LIMIT {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    if bytes > BYTE_LIMIT.saturating_sub(state.bytes) || state.items >= ITEM_LIMIT {
        return Err(io::ErrorKind::WouldBlock.into());
    }
    Ok(())
}

impl Receiver {
    /// Discard input at a protocol boundary without consuming close or resize.
    /// Writers enqueue under the same lock; release permits outside it.
    pub(crate) fn discard_writes(&mut self) {
        let state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut discarded = Vec::new();
        while let Ok(write) = self.data.try_recv() {
            discarded.push(write);
        }
        drop(state);
        drop(discarded);
    }

    fn control(&self) -> Option<SshCommand> {
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.stopped {
            return Some(SshCommand::Close);
        }
        state.resize.take().map(
            |(cols, rows, pixel_width, pixel_height)| SshCommand::Resize {
                cols,
                rows,
                pixel_width,
                pixel_height,
            },
        )
    }

    pub(crate) async fn recv(&mut self) -> Option<SshCommand> {
        loop {
            let notified = self.shared.control.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(command) = self.control() {
                return Some(command);
            }
            tokio::select! {
                biased;
                _ = &mut notified => {},
                data = self.data.recv() => return data.map(SshCommand::Write),
            }
        }
    }

    pub(crate) fn try_recv(&mut self) -> Result<SshCommand, mpsc::error::TryRecvError> {
        if let Some(command) = self.control() {
            return Ok(command);
        }
        self.data.try_recv().map(SshCommand::Write)
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .stopped = true;
        self.shared.space.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::channel;
    use crate::SshCommand;
    use crate::writer_mailbox::BYTE_LIMIT;

    #[test]
    fn protocol_boundary_discards_input_and_preserves_control() {
        let (tx, mut rx) = channel();
        tx.write(vec![1; BYTE_LIMIT]).unwrap();
        tx.resize(80, 24, 0, 0);
        rx.discard_writes();
        assert!(matches!(rx.try_recv().unwrap(), SshCommand::Resize { .. }));
        assert!(rx.try_recv().is_err());
        tx.write(vec![2; BYTE_LIMIT]).unwrap();
        tx.close();
        rx.discard_writes();
        assert!(matches!(rx.try_recv().unwrap(), SshCommand::Close));
        assert_eq!(tx.shared.state.lock().unwrap().bytes, 0);
    }

    #[test]
    fn deferred_and_in_flight_writes_keep_their_permits_and_controls_bypass_a_full_queue() {
        let (sender, mut receiver) = channel();
        sender.write(vec![0; BYTE_LIMIT]).unwrap();
        let SshCommand::Write(pending) = receiver.try_recv().unwrap() else {
            panic!("write");
        };
        assert_eq!(
            sender.write(vec![1]).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        sender.resize(10, 20, 0, 0);
        sender.resize(30, 40, 0, 0);
        assert!(matches!(
            receiver.try_recv(),
            Ok(SshCommand::Resize { cols: 30, .. })
        ));
        drop(pending);
        sender.write(vec![2]).unwrap();
        sender.close();
        assert!(matches!(receiver.try_recv(), Ok(SshCommand::Close)));
        assert_eq!(
            sender.write(vec![3]).unwrap_err().kind(),
            std::io::ErrorKind::BrokenPipe
        );
    }
}

#[cfg(test)]
mod capacity_tests {
    use super::channel;
    use crate::writer_mailbox::ITEM_LIMIT;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    #[test]
    fn item_permits_wake_waiters_only_after_in_flight_work_is_released() {
        let (sender, mut receiver) = channel();
        for _ in 0..ITEM_LIMIT {
            sender.write(vec![0]).unwrap();
        }
        assert_eq!(
            sender.write(vec![1]).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        let in_flight = receiver.try_recv().unwrap();
        let waiter = sender.clone();
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            tx.send(waiter.wait_capacity(1, &AtomicBool::new(false)))
                .unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
        drop(in_flight);
        rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn capacity_waits_end_on_cancel_close_or_receiver_drop() {
        for mode in 0..3 {
            let (sender, receiver) = channel();
            for _ in 0..ITEM_LIMIT {
                sender.write(vec![0]).unwrap();
            }
            let waiter = sender.clone();
            let cancelled = Arc::new(AtomicBool::new(false));
            let worker_cancel = cancelled.clone();
            let worker = std::thread::spawn(move || waiter.wait_capacity(1, &worker_cancel));
            match mode {
                0 => cancelled.store(true, Ordering::Release),
                1 => sender.close(),
                _ => drop(receiver),
            }
            let error = worker.join().unwrap().unwrap_err();
            assert!(matches!(
                error.kind(),
                std::io::ErrorKind::Interrupted | std::io::ErrorKind::BrokenPipe
            ));
        }
    }
}
