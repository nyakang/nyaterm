use std::io;
use std::sync::{Arc, Mutex};

use crate::session_config::TelnetSessionConfig;
use crate::telnet_codec::{DO, DONT, IAC, OPT_NAWS, TelnetNawsState, WONT};
use crate::transport_io::TransportWriterCommand;
use crate::writer_mailbox::WriterMailbox;

#[derive(Clone, Copy)]
enum NawsOperation {
    Offer,
    Negotiate(u8),
    Resize(u16, u16),
}

/// The reader and resize path share one state and one FIFO. Always lock NAWS
/// before the mailbox; the socket writer never takes the NAWS lock.
#[derive(Clone)]
pub(crate) struct TelnetNawsWriter {
    state: Arc<Mutex<TelnetNawsState>>,
    command_tx: WriterMailbox,
    raw_tcp: bool,
    send_naws: bool,
}

impl TelnetNawsWriter {
    pub(crate) fn new(command_tx: WriterMailbox, config: &TelnetSessionConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(TelnetNawsState::new(config.cols, config.rows))),
            command_tx,
            raw_tcp: config.raw_tcp,
            send_naws: config.send_naws,
        }
    }

    pub(crate) fn offer(&self) -> io::Result<()> {
        self.send(NawsOperation::Offer)
    }

    pub(crate) fn negotiate(&self, command: u8) -> io::Result<()> {
        self.send(NawsOperation::Negotiate(command))
    }

    pub(crate) fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        self.send(NawsOperation::Resize(cols, rows))
    }

    fn send(&self, operation: NawsOperation) -> io::Result<()> {
        self.apply(operation, |payload| {
            self.command_tx
                .send(TransportWriterCommand::TelnetControl(payload))
        })
    }

    fn apply(
        &self,
        operation: NawsOperation,
        enqueue: impl FnOnce(Vec<u8>) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("telnet NAWS state poisoned"))?;
        let mut next = *state;
        let payload = match operation {
            NawsOperation::Offer if !self.raw_tcp && self.send_naws => next.offer(),
            NawsOperation::Negotiate(DO) if !self.raw_tcp && self.send_naws => next.accept(),
            NawsOperation::Negotiate(DO) if !self.raw_tcp => vec![IAC, WONT, OPT_NAWS],
            NawsOperation::Negotiate(DONT) if !self.raw_tcp => next.reject(),
            NawsOperation::Resize(cols, rows) => next.resize(cols, rows),
            _ => Vec::new(),
        };
        // Only a non-waiting mailbox enqueue runs under this lock. Returning
        // an unlocked payload would allow an older size to overtake a resize.
        if !payload.is_empty()
            && let Err(error) = enqueue(payload)
        {
            if !matches!(operation, NawsOperation::Resize(..)) {
                // A failed stop acknowledgement must still prohibit NAWS.
                // The reader reports the error and shuts down the connection.
                let _ = state.reject();
            }
            return Err(error);
        }
        *state = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
