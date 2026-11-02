//! Shared input accounting for every virtual pane of one control client.
//! Reservations follow input through batching and the SSH send, not just recv.
use std::collections::HashSet;
use std::io;
use std::sync::{Arc, Mutex};

use tokio::sync::{Notify, mpsc};

use crate::SshCommand;
use crate::writer_mailbox::{BYTE_LIMIT, ITEM_LIMIT};

#[derive(Default)]
struct State {
    bytes: usize,
    items: usize,
    peak_bytes: usize,
    stopped: bool,
    suspended: bool,
    generation: u64,
    detach: bool,
    close_panes: HashSet<String>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    control: Notify,
}

#[derive(Clone)]
pub(crate) struct Sender {
    tx: mpsc::UnboundedSender<PendingCommand>,
    shared: Arc<Shared>,
}

pub(crate) struct Receiver {
    rx: mpsc::UnboundedReceiver<PendingCommand>,
    shared: Arc<Shared>,
}

pub(crate) struct Control {
    shared: Arc<Shared>,
}

pub(crate) struct PendingCommand {
    pub(crate) command: SshCommand,
    pub(crate) permit: Permit,
}

pub(crate) struct Permit {
    shared: Arc<Shared>,
    bytes: usize,
    generation: u64,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        state.bytes -= self.bytes;
        state.items -= 1;
    }
}

pub(crate) fn channel() -> (Sender, Receiver) {
    let (tx, rx) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared::default());
    (
        Sender {
            tx,
            shared: shared.clone(),
        },
        Receiver { rx, shared },
    )
}

impl Sender {
    pub(crate) fn same_channel(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    fn reserve(&self, bytes: usize) -> io::Result<Permit> {
        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| io::Error::other("tmux input poisoned"))?;
        if state.stopped || state.suspended || self.tx.is_closed() {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        if bytes > BYTE_LIMIT {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        if bytes > BYTE_LIMIT.saturating_sub(state.bytes) || state.items >= ITEM_LIMIT {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        state.bytes += bytes;
        state.peak_bytes = state.peak_bytes.max(state.bytes);
        state.items += 1;
        Ok(Permit {
            shared: self.shared.clone(),
            bytes,
            generation: state.generation,
        })
    }

    pub(crate) fn write(&self, pane: &str, data: &[u8]) -> io::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        // Reserve before copying a potentially large paste on the UI thread.
        let permit = self.reserve(data.len())?;
        self.enqueue(
            SshCommand::TmuxPaneInput {
                pane: pane.into(),
                data: data.to_vec(),
            },
            permit,
        )
    }

    fn enqueue(&self, command: SshCommand, permit: Permit) -> io::Result<()> {
        let state = self
            .shared
            .state
            .lock()
            .map_err(|_| io::Error::other("tmux input poisoned"))?;
        if state.stopped || state.suspended || permit.generation != state.generation {
            drop(state);
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let result = self.tx.send(PendingCommand { command, permit });
        // A failed send owns a permit whose Drop takes the accounting lock.
        drop(state);
        result.map_err(|_| io::ErrorKind::BrokenPipe.into())
    }

    pub(crate) fn send(&self, command: SshCommand) -> io::Result<()> {
        match command {
            SshCommand::TmuxDetach | SshCommand::TmuxPaneClose { .. } => {
                let mut state = self
                    .shared
                    .state
                    .lock()
                    .map_err(|_| io::Error::other("tmux input poisoned"))?;
                if state.stopped || state.suspended {
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
                match command {
                    SshCommand::TmuxDetach => state.detach = true,
                    SshCommand::TmuxPaneClose { pane } => {
                        state.close_panes.insert(pane);
                    }
                    _ => unreachable!(),
                }
                drop(state);
                self.shared.control.notify_one();
                Ok(())
            }
            command => {
                let bytes = match &command {
                    SshCommand::TmuxCommand { line } => line.len(),
                    SshCommand::TmuxPaneInput { data, .. } => data.len(),
                    _ => 0,
                };
                let permit = self.reserve(bytes)?;
                self.enqueue(command, permit)
            }
        }
    }

    pub(crate) fn peak_bytes(&self) -> usize {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .peak_bytes
    }
}

impl Receiver {
    pub(crate) fn resume(&mut self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .suspended = false;
    }

    pub(crate) fn control_receiver(&self) -> Control {
        Control {
            shared: self.shared.clone(),
        }
    }

    pub(crate) async fn recv(&mut self) -> Option<PendingCommand> {
        self.rx.recv().await
    }

    /// No command from a detached generation may run on a later attachment.
    pub(crate) fn discard_pending(&mut self) {
        {
            let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
            state.suspended = true;
            state.generation += 1;
            state.detach = false;
            state.close_panes.clear();
        }
        while self.rx.try_recv().is_ok() {}
    }
}

impl Control {
    pub(crate) async fn recv(&self) -> SshCommand {
        loop {
            let notified = self.shared.control.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut state = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
                if std::mem::take(&mut state.detach) {
                    return SshCommand::TmuxDetach;
                }
                if let Some(pane) = state.close_panes.iter().next().cloned() {
                    state.close_panes.remove(&pane);
                    return SshCommand::TmuxPaneClose { pane };
                }
            }
            notified.await;
        }
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .stopped = true;
    }
}

#[cfg(test)]
mod tests {
    use super::channel;
    use crate::SshCommand;
    use crate::writer_mailbox::{BYTE_LIMIT, ITEM_LIMIT};
    use std::io::ErrorKind;

    #[tokio::test]
    async fn panes_share_a_byte_budget_until_the_send_completes() {
        let (tx, mut rx) = channel();
        tx.write("%1", &vec![1; BYTE_LIMIT]).unwrap();
        let sending = rx.recv().await.unwrap();
        assert_eq!(
            tx.write("%2", b"x").unwrap_err().kind(),
            ErrorKind::WouldBlock
        );
        assert_eq!(
            tx.write("%2", &vec![0; BYTE_LIMIT + 1]).unwrap_err().kind(),
            ErrorKind::InvalidInput
        );
        drop(sending);
        tx.write("%2", b"x").unwrap();
        assert_eq!(rx.recv().await.unwrap().permit.bytes, 1);
        assert_eq!(tx.peak_bytes(), BYTE_LIMIT);
    }

    #[tokio::test]
    async fn item_saturation_does_not_block_detach_or_pane_close() {
        let (tx, mut rx) = channel();
        for _ in 0..ITEM_LIMIT {
            tx.write("%1", b"x").unwrap();
        }
        assert_eq!(
            tx.write("%2", b"x").unwrap_err().kind(),
            ErrorKind::WouldBlock
        );
        tx.send(SshCommand::TmuxPaneClose { pane: "%1".into() })
            .unwrap();
        tx.send(SshCommand::TmuxDetach).unwrap();
        let control = rx.control_receiver();
        assert!(matches!(control.recv().await, SshCommand::TmuxDetach));
        assert!(matches!(
            control.recv().await,
            SshCommand::TmuxPaneClose { .. }
        ));
        rx.discard_pending();
        assert_eq!(
            tx.write("%2", b"stale attachment").unwrap_err().kind(),
            ErrorKind::BrokenPipe
        );
        rx.resume();
        tx.write("%2", b"next attach").unwrap();
        drop(rx);
        assert_eq!(
            tx.write("%2", b"x").unwrap_err().kind(),
            ErrorKind::BrokenPipe
        );
    }

    #[tokio::test]
    async fn a_reservation_cannot_cross_a_detach_and_reattach_boundary() {
        let (tx, mut rx) = channel();
        let permit = tx.reserve(4).unwrap();
        rx.discard_pending();
        rx.resume();
        let error = tx
            .enqueue(
                SshCommand::TmuxPaneInput {
                    pane: "%0".into(),
                    data: b"late".to_vec(),
                },
                permit,
            )
            .err()
            .unwrap();
        assert_eq!(error.kind(), ErrorKind::BrokenPipe);
        assert_eq!(tx.shared.state.lock().unwrap().bytes, 0);
        assert!(rx.rx.try_recv().is_err());
    }
}
