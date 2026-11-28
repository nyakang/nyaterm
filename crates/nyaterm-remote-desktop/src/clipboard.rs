pub use nyaterm_native_protocol::clipboard::{
    ClipboardAccess, ClipboardCommand, ClipboardContent, ClipboardEvent,
};
use sha2::{Digest, Sha256};

pub const MAX_CLIPBOARD_TEXT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardOrigin {
    Local,
    Remote,
}

#[derive(Clone, Debug, Default)]
pub struct ClipboardTracker {
    generation: u64,
    last_hash: Option<[u8; 32]>,
    last_origin: Option<ClipboardOrigin>,
}

impl ClipboardTracker {
    pub fn accept(&mut self, origin: ClipboardOrigin, text: &str) -> anyhow::Result<Option<u64>> {
        if text.len() > MAX_CLIPBOARD_TEXT_BYTES {
            anyhow::bail!("clipboard text exceeds the 4 MiB limit");
        }
        let hash: [u8; 32] = Sha256::digest(text.as_bytes()).into();
        if self.last_hash == Some(hash) && self.last_origin != Some(origin) {
            self.last_origin = Some(origin);
            return Ok(None);
        }
        if self.last_hash == Some(hash) && self.last_origin == Some(origin) {
            return Ok(None);
        }
        self.generation = self.generation.wrapping_add(1);
        self.last_hash = Some(hash);
        self.last_origin = Some(origin);
        Ok(Some(self.generation))
    }
}

#[cfg(test)]
mod tests {
    use crate::{ClipboardOrigin, ClipboardTracker, MAX_CLIPBOARD_TEXT_BYTES};

    #[test]
    fn suppresses_clipboard_echo_and_rejects_oversize_text() {
        let mut tracker = ClipboardTracker::default();
        assert_eq!(
            tracker.accept(ClipboardOrigin::Local, "hello").unwrap(),
            Some(1)
        );
        assert_eq!(
            tracker.accept(ClipboardOrigin::Remote, "hello").unwrap(),
            None
        );
        assert!(
            tracker
                .accept(
                    ClipboardOrigin::Local,
                    &"x".repeat(MAX_CLIPBOARD_TEXT_BYTES + 1)
                )
                .is_err()
        );
    }
}
