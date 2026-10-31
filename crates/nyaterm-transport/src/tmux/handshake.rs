//! Passive startup validation followed by a bounded, nonce-bearing query.
//! No workspace state or tmux configuration is changed until both succeed.
use super::control::{BlockId, ControlMessage, parse_line};
use russh::{ChannelMsg, ChannelReadHalf, ChannelWriteHalf, client};
use std::sync::Arc;
use std::time::Duration;

pub(crate) const CONFIRM_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const CONFIRM_MAX_BYTES: usize = 512 * 1024;
pub(crate) const CONTROL_PREFIX: &[u8] = b"\x1bP1000p";

#[derive(Default)]
pub(crate) struct Startup {
    block: Option<BlockId>,
    completed: bool,
    session: bool,
}

impl Startup {
    /// None rejects the candidate; true confirms the complete startup.
    pub(crate) fn line(&mut self, line: &[u8]) -> Option<bool> {
        let line = std::str::from_utf8(line).ok()?.trim_end_matches('\r');
        match parse_line(line) {
            Some(ControlMessage::Begin(id)) if self.block.is_none() && !self.completed => {
                self.block = Some(id);
            }
            Some(ControlMessage::End(id)) if self.block == Some(id) => {
                self.block = None;
                self.completed = true;
            }
            Some(
                ControlMessage::Begin(_)
                | ControlMessage::End(_)
                | ControlMessage::Error(_)
                | ControlMessage::Exit { .. },
            ) => return None,
            _ if self.block.is_some() => {}
            Some(ControlMessage::SessionChanged { session }) => {
                if session.strip_prefix('$')?.parse::<u64>().is_err() {
                    return None;
                }
                self.session = true;
            }
            Some(_) => {}
            None => return None,
        }
        Some(self.completed && self.session)
    }
}

#[derive(Debug, PartialEq)]
enum Progress {
    Pending,
    Confirmed(Vec<u8>),
    Rejected,
}

struct Probe {
    token: String,
    command: String,
    raw: Vec<u8>,
    consumed: usize,
    block: Option<(BlockId, bool, usize)>,
}

impl Probe {
    fn new(startup: &[u8]) -> Self {
        let token = format!("nyaterm_{}", uuid::Uuid::new_v4().simple());
        Self {
            command: format!("display-message -p '{token}'"),
            token,
            raw: startup.to_vec(),
            consumed: startup.len(),
            block: None,
        }
    }

    fn feed(&mut self, data: &[u8]) -> Progress {
        self.raw.extend_from_slice(data);
        if self.raw.len() > CONFIRM_MAX_BYTES {
            return Progress::Rejected;
        }
        while let Some(offset) = self.raw[self.consumed..].iter().position(|b| *b == b'\n') {
            let end = self.consumed + offset;
            let bytes = &self.raw[self.consumed..end];
            self.consumed = end + 1;
            let Ok(line) = std::str::from_utf8(bytes) else {
                return Progress::Rejected;
            };
            let line = line.trim_end_matches('\r');
            match parse_line(line) {
                Some(ControlMessage::Begin(id)) if self.block.is_none() => {
                    self.block = Some((id, false, 0));
                }
                Some(ControlMessage::End(id)) => {
                    if let Some((begin, matched, 1)) = self.block.take()
                        && begin == id
                        && matched
                    {
                        return Progress::Confirmed(self.raw[self.consumed..].to_vec());
                    }
                    return Progress::Rejected;
                }
                Some(
                    ControlMessage::Begin(_)
                    | ControlMessage::Error(_)
                    | ControlMessage::Exit { .. },
                ) => return Progress::Rejected,
                _ if self.block.is_some() => {
                    let (_, matched, count) = self.block.as_mut().unwrap();
                    *matched = line == self.token;
                    *count += 1;
                }
                Some(_) => {} // Notifications alone cannot verify the channel.
                None if line == self.command => {} // PTY echo in single -C mode.
                None => return Progress::Rejected,
            }
        }
        Progress::Pending
    }
}

/// On failure return every received byte, including the passive startup.
/// The timeout covers both writing the probe and waiting for its response.
pub(crate) async fn verify(
    channel: &Arc<ChannelWriteHalf<client::Msg>>,
    channel_rx: &mut ChannelReadHalf,
    startup: &[u8],
    initial: &[u8],
) -> Result<Vec<u8>, Vec<u8>> {
    let mut probe = Probe::new(startup);
    if probe.feed(initial) == Progress::Rejected {
        return Err(probe.raw);
    }
    let result = tokio::time::timeout(CONFIRM_TIMEOUT, async {
        let command = format!("{}\n", probe.command);
        channel.data_bytes(command.into_bytes()).await.ok()?;
        loop {
            match channel_rx.wait().await {
                Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                    match probe.feed(&data) {
                        Progress::Pending => {}
                        Progress::Confirmed(rest) => return Some(rest),
                        Progress::Rejected => return None,
                    }
                }
                None | Some(ChannelMsg::Eof | ChannelMsg::Close) => return None,
                _ => {}
            }
        }
    })
    .await;
    match result {
        Ok(Some(rest)) => Ok(rest),
        _ => Err(probe.raw),
    }
}

#[cfg(test)]
mod tests {
    use super::{CONFIRM_MAX_BYTES, CONFIRM_TIMEOUT, Probe, Progress};
    use futures::{Stream, StreamExt};
    use std::time::Duration;

    /// Feeds stream chunks into the probe the same way `verify` feeds channel
    /// reads; kept stream-shaped so tests can drive silent and busy peers.
    async fn read_probe(
        probe: &mut Probe,
        incoming: impl Stream<Item = Vec<u8>>,
    ) -> Option<Vec<u8>> {
        futures::pin_mut!(incoming);
        while let Some(data) = incoming.next().await {
            match probe.feed(&data) {
                Progress::Pending => {}
                Progress::Confirmed(rest) => return Some(rest),
                Progress::Rejected => return None,
            }
        }
        None
    }

    #[test]
    fn nonce_requires_complete_matching_response() {
        let mut probe = Probe::new(b"startup\r\n");
        assert_eq!(probe.feed(b"%window-add @1\n"), Progress::Pending);
        let response = format!("%begin 1 2 1\r\n{}\r\n%end 1 2 1\r\n", probe.token);
        for byte in &response.as_bytes()[..response.len() - 1] {
            assert_eq!(probe.feed(&[*byte]), Progress::Pending);
        }
        assert_eq!(
            probe.feed(b"\n%output %0 x\n"),
            Progress::Confirmed(b"%output %0 x\n".to_vec())
        );
    }

    #[test]
    fn rejects_mismatched_block_wrong_nonce_and_shell_with_exact_replay() {
        for response in [
            b"%begin 1 2 1\nwrong\n%end 1 2 1\n".as_slice(),
            b"\xff\r\nuser@host$ ",
            b"sh: display-message: not found\r\nprompt> ",
        ] {
            let mut probe = Probe::new(b"%window-add @1\r\n");
            assert_eq!(probe.feed(response), Progress::Rejected);
            assert_eq!(
                probe.raw,
                [b"%window-add @1\r\n".as_slice(), response].concat()
            );
        }
        let mut probe = Probe::new(b"");
        let response = format!("%begin 1 2 1\n{}\n%end 1 3 1\n", probe.token);
        assert_eq!(probe.feed(response.as_bytes()), Progress::Rejected);
    }

    #[test]
    fn probe_echo_is_accepted_and_buffer_is_bounded() {
        let mut probe = Probe::new(b"");
        let echo = format!("{}\r\n", probe.command);
        assert_eq!(probe.feed(echo.as_bytes()), Progress::Pending);
        assert_eq!(
            probe.feed(&vec![b'x'; CONFIRM_MAX_BYTES]),
            Progress::Rejected
        );
    }

    #[tokio::test(start_paused = true)]
    async fn silent_peer_and_continuous_notifications_have_fixed_deadline() {
        let mut silent = Probe::new(b"startup");
        assert!(
            tokio::time::timeout(
                CONFIRM_TIMEOUT,
                read_probe(&mut silent, futures::stream::pending())
            )
            .await
            .is_err()
        );
        assert_eq!(silent.raw, b"startup");
        let mut busy = Probe::new(b"");
        let incoming = futures::stream::unfold((), |()| async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Some((b"%window-add @1\n".to_vec(), ()))
        });
        let started = tokio::time::Instant::now();
        assert!(
            tokio::time::timeout(CONFIRM_TIMEOUT, read_probe(&mut busy, incoming))
                .await
                .is_err()
        );
        assert_eq!(started.elapsed(), CONFIRM_TIMEOUT);
        assert!(!busy.raw.is_empty());
    }
}
