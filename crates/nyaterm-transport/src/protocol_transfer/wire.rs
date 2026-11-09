//! Transfer workers wait for transport capacity without blocking GPUI.
use crate::SessionManager;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Default)]
pub struct TransferWire {
    target: Option<(Arc<SessionManager>, String)>,
    stopped: Arc<AtomicBool>,
}

impl TransferWire {
    pub fn connected(manager: Arc<SessionManager>, session_id: String) -> Self {
        Self {
            target: Some((manager, session_id)),
            ..Self::default()
        }
    }

    pub fn send(&self, bytes: &[u8]) -> Result<bool, String> {
        let Some((manager, session_id)) = &self.target else {
            return Ok(false);
        };
        manager
            .write_raw_cancellable(session_id, bytes, &self.stopped)
            .map_err(|error| error.to_string())?;
        Ok(true)
    }

    // Cancellation must interrupt a capacity wait, but its final protocol abort
    // still gets one nonblocking attempt through the same bounded mailbox.
    pub fn send_abort(&self, bytes: &[u8]) -> Result<bool, String> {
        let Some((manager, session_id)) = &self.target else {
            return Ok(false);
        };
        manager
            .write_raw(session_id, bytes)
            .map_err(|error| error.to_string())?;
        Ok(true)
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }
}
