//! Detection of `tmux -CC` startup inside a normal terminal byte stream.
//!
//! The user launches tmux control mode by running `tmux -CC new -As0` (as a
//! startup command or by hand). tmux then starts emitting line-based `%`
//! notifications. This module incrementally scans the outgoing data for a
//! whitelisted notification keyword at the start of a line and reports the
//! switch point so the caller can hand the channel to the control session.

use super::control::is_control_notification;

/// Upper bound for a held-back candidate line; longer lines flush as normal
/// output so a stray `%` can never stall the terminal.
const MAX_CANDIDATE_LINE_BYTES: usize = 512;

pub(crate) enum TmuxFeed {
    /// Bytes to forward through the regular output pipeline.
    Passthrough(Vec<u8>),
    /// A control notification was found at a line start.
    ///
    /// `passthrough` are bytes before the notification that still belong to
    /// the normal terminal stream; `rest` are the bytes after the detected
    /// line and must be consumed by the control-mode parser.
    Detected { passthrough: Vec<u8>, rest: Vec<u8> },
}

#[derive(Default)]
pub(crate) struct TmuxControlDetector {
    /// The next byte begins a new line.
    line_start: bool,
    /// Bytes of a not-yet-decided line that started with `%`.
    candidate: Option<Vec<u8>>,
    /// Set once control mode was detected; a dead detector passes through.
    done: bool,
}

impl TmuxControlDetector {
    pub(crate) fn new() -> Self {
        Self {
            line_start: true,
            candidate: None,
            done: false,
        }
    }

    pub(crate) fn feed(&mut self, data: &[u8]) -> TmuxFeed {
        if self.done {
            return TmuxFeed::Passthrough(data.to_vec());
        }

        let mut out: Vec<u8> = Vec::with_capacity(data.len());
        let mut i = 0;

        while i < data.len() {
            if let Some(candidate) = self.candidate.as_mut() {
                let byte = data[i];
                i += 1;
                if byte == b'\n' {
                    let mut line = std::mem::take(candidate);
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                    self.candidate = None;
                    match std::str::from_utf8(&line) {
                        Ok(text) if is_control_notification(text) => {
                            self.done = true;
                            return TmuxFeed::Detected {
                                passthrough: out,
                                rest: data[i..].to_vec(),
                            };
                        }
                        _ => {
                            // Not a notification: the line was ordinary output.
                            out.extend_from_slice(&line);
                            out.push(b'\n');
                            self.line_start = true;
                        }
                    }
                } else {
                    // Grammar: `%<keyword:[a-z-]+> [' ' <args>] '\n'.
                    // Once a space ends the keyword, args may be arbitrary.
                    let in_args = candidate[1..].contains(&b' ');
                    let allowed = if in_args {
                        true
                    } else {
                        byte.is_ascii_lowercase() || byte == b'-' || byte == b' '
                    };
                    if !allowed || candidate.len() >= MAX_CANDIDATE_LINE_BYTES {
                        // Can never become a notification: flush as output.
                        out.extend_from_slice(candidate);
                        out.push(byte);
                        self.candidate = None;
                        self.line_start = false;
                    } else {
                        candidate.push(byte);
                    }
                }
                continue;
            }

            let byte = data[i];
            i += 1;
            if self.line_start && byte == b'%' {
                self.candidate = Some(vec![b'%']);
                self.line_start = false;
            } else {
                out.push(byte);
                self.line_start = byte == b'\n';
            }
        }

        TmuxFeed::Passthrough(out)
    }
}

#[cfg(test)]
mod tests {
    use super::{TmuxControlDetector, TmuxFeed};

    #[test]
    fn detects_control_line_and_splits_stream() {
        let mut detector = TmuxControlDetector::new();
        let chunk =
            b"user@host:~$ tmux -CC new -As0\r\n%session-changed $0 0\r\n%window-add @1\r\n";
        match detector.feed(chunk) {
            TmuxFeed::Detected { passthrough, rest } => {
                assert_eq!(passthrough, b"user@host:~$ tmux -CC new -As0\r\n");
                assert_eq!(rest, b"%window-add @1\r\n");
            }
            TmuxFeed::Passthrough(_) => panic!("expected detection"),
        }
    }

    #[test]
    fn ignores_percent_text_mid_line() {
        let mut detector = TmuxControlDetector::new();
        match detector.feed(b"value is %window-add @1\n") {
            TmuxFeed::Passthrough(bytes) => {
                assert_eq!(bytes, b"value is %window-add @1\n");
            }
            TmuxFeed::Detected { .. } => panic!("must not detect mid-line"),
        }
    }

    #[test]
    fn ignores_unknown_percent_lines_at_start() {
        let mut detector = TmuxControlDetector::new();
        match detector.feed(b"%percent-sign example\nnext") {
            TmuxFeed::Passthrough(bytes) => {
                assert_eq!(bytes, b"%percent-sign example\nnext");
            }
            TmuxFeed::Detected { .. } => panic!("must not detect unknown keyword"),
        }
    }

    #[test]
    fn candidate_line_flushed_when_keyword_breaks() {
        let mut detector = TmuxControlDetector::new();
        match detector.feed(b"\n%foo_bar more\n") {
            TmuxFeed::Passthrough(bytes) => assert_eq!(bytes, b"\n%foo_bar more\n"),
            TmuxFeed::Detected { .. } => panic!("unexpected detection"),
        }
    }

    #[test]
    fn detection_spans_chunk_boundaries() {
        let mut detector = TmuxControlDetector::new();
        assert!(matches!(
            detector.feed(b"prompt%window-ad"),
            TmuxFeed::Passthrough(_)
        ));
        // `%window-ad` was not at a line start, second chunk continues normal.
        match detector.feed(b"\n%wind") {
            TmuxFeed::Passthrough(bytes) => assert_eq!(bytes, b"\n"),
            TmuxFeed::Detected { .. } => panic!("line not complete yet"),
        }
        match detector.feed(b"ow-add @1\n%exit\n") {
            TmuxFeed::Detected { rest, .. } => assert_eq!(rest, b"%exit\n"),
            TmuxFeed::Passthrough(_) => panic!("expected detection"),
        }
    }
}
