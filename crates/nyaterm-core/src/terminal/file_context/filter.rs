//! Strip private runtime evidence before generic terminal parsers or raw recording.
use std::borrow::Cow;

const PREFIX: &[u8] = b"\x1b]7777;NyaTermFileContext;";

/// Streaming, constant-space filter. It retains only a possible prefix; once
/// identified, even malformed or oversized private payloads are never forwarded.
#[derive(Default)]
pub struct FileContextFilter {
    matched: usize,
    flushed: usize,
    discarding: bool,
    escaped: bool,
}

impl FileContextFilter {
    pub fn filter<'a>(&mut self, data: &'a [u8]) -> Cow<'a, [u8]> {
        if self.matched == 0 && !self.discarding && !data.contains(&b'\x1b') {
            return Cow::Borrowed(data);
        }
        let mut output = Vec::with_capacity(data.len());
        for &byte in data {
            if self.discarding {
                if matches!(byte, b'\x07' | b'\x18' | b'\x1a') || (self.escaped && byte == b'\\') {
                    self.discarding = false;
                }
                self.escaped = byte == b'\x1b';
                continue;
            }
            if byte == PREFIX[self.matched] {
                self.matched += 1;
                if self.matched == PREFIX.len() {
                    self.matched = 0;
                    self.flushed = 0;
                    self.discarding = true;
                    self.escaped = false;
                }
            } else {
                output.extend_from_slice(&PREFIX[self.flushed..self.matched]);
                self.matched = 0;
                self.flushed = 0;
                if byte == PREFIX[0] {
                    self.matched = 1;
                } else {
                    output.push(byte);
                }
            }
        }
        Cow::Owned(output)
    }

    /// Preserve a final incomplete ordinary escape sequence at a recording
    /// boundary. A recognized private payload remains suppressed until its end.
    pub fn flush_prefix(&mut self) -> &'static [u8] {
        let prefix = &PREFIX[self.flushed..self.matched];
        self.flushed = self.matched;
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::{FileContextFilter, PREFIX};

    #[test]
    fn private_payloads_are_removed_at_every_chunk_boundary() {
        for terminator in [b"\x07".as_slice(), b"\x1b\\"] {
            let mut wire = b"\xff\x1b[31mbefore".to_vec();
            wire.extend_from_slice(PREFIX);
            wire.extend_from_slice(b"1;private-runtime-evidence");
            wire.extend_from_slice(terminator);
            wire.extend_from_slice(b"after\x1b[0m");
            for size in 1..=wire.len() {
                let mut filter = FileContextFilter::default();
                let mut output = Vec::new();
                for chunk in wire.chunks(size) {
                    output.extend_from_slice(&filter.filter(chunk));
                }
                output.extend_from_slice(filter.flush_prefix());
                assert_eq!(output, b"\xff\x1b[31mbeforeafter\x1b[0m");
            }
        }
    }

    #[test]
    fn unrelated_and_incomplete_escapes_remain_byte_exact() {
        let wire = b"\x1b\x1b]7777;Other;data\x07\x1b]133;A\x07\x1b[0m\x1b]7777;Nya";
        for size in 1..=wire.len() {
            let mut filter = FileContextFilter::default();
            let mut output = Vec::new();
            for chunk in wire.chunks(size) {
                output.extend_from_slice(&filter.filter(chunk));
            }
            output.extend_from_slice(filter.flush_prefix());
            assert_eq!(output, wire);
        }
    }

    #[test]
    fn oversized_unterminated_private_payload_retains_no_payload() {
        let mut filter = FileContextFilter::default();
        assert!(filter.filter(PREFIX).is_empty());
        for _ in 0..100 {
            assert!(filter.filter(&[b'x'; 8192]).is_empty());
        }
        assert!(filter.flush_prefix().is_empty());
        assert_eq!(filter.filter(b"\x07visible").as_ref(), b"visible");
    }
}
