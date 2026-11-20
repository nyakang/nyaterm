//! Line-ending conversion for the FTP ASCII transfer type (`TYPE A`).
//!
//! RFC 959 requires the sender to convert local text to NVT-ASCII (CRLF line
//! endings) and the receiver to convert it back to the local representation.
//! The converters keep their state across chunks so a CRLF pair split between
//! two reads is still recognized.

/// Local text line ending used when decoding a download.
const LOCAL_NEWLINE: &[u8] = if cfg!(windows) { b"\r\n" } else { b"\n" };

/// Converts local text to NVT-ASCII by inserting a CR before every LF that is
/// not already preceded by one.
#[derive(Debug, Default)]
pub(crate) struct NvtEncoder {
    last_was_cr: bool,
}

impl NvtEncoder {
    pub(crate) fn encode(&mut self, input: &[u8], output: &mut Vec<u8>) {
        for &byte in input {
            if byte == b'\n' && !self.last_was_cr {
                output.push(b'\r');
            }
            output.push(byte);
            self.last_was_cr = byte == b'\r';
        }
    }
}

/// Converts NVT-ASCII text to the local line ending by replacing CRLF with the
/// platform newline. A lone CR is preserved.
#[derive(Debug, Default)]
pub(crate) struct LocalDecoder {
    pending_cr: bool,
}

impl LocalDecoder {
    pub(crate) fn decode(&mut self, input: &[u8], output: &mut Vec<u8>) {
        for &byte in input {
            if self.pending_cr {
                self.pending_cr = false;
                if byte == b'\n' {
                    output.extend_from_slice(LOCAL_NEWLINE);
                    continue;
                }
                output.push(b'\r');
            }
            if byte == b'\r' {
                self.pending_cr = true;
            } else {
                output.push(byte);
            }
        }
    }

    /// Writes a trailing CR that was not followed by LF in the stream.
    pub(crate) fn finish(&mut self, output: &mut Vec<u8>) {
        if self.pending_cr {
            self.pending_cr = false;
            output.push(b'\r');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LocalDecoder, NvtEncoder};

    fn encode(input: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        NvtEncoder::default().encode(input, &mut output);
        output
    }

    fn decode(input: &[u8]) -> Vec<u8> {
        let mut decoder = LocalDecoder::default();
        let mut output = Vec::new();
        decoder.decode(input, &mut output);
        decoder.finish(&mut output);
        output
    }

    #[test]
    fn encodes_bare_line_feeds_and_keeps_existing_crlf() {
        assert_eq!(encode(b"a\nb"), b"a\r\nb");
        assert_eq!(encode(b"a\r\nb"), b"a\r\nb");
        assert_eq!(encode(b"a\rb"), b"a\rb");
    }

    #[test]
    fn keeps_state_when_crlf_is_split_across_chunks() {
        let mut encoder = NvtEncoder::default();
        let mut output = Vec::new();
        encoder.encode(b"a\r", &mut output);
        encoder.encode(b"\nb", &mut output);
        assert_eq!(output, b"a\r\nb");
    }

    #[test]
    fn decodes_crlf_and_preserves_lone_carriage_returns() {
        let expected = if cfg!(windows) {
            b"a\r\nb".to_vec()
        } else {
            b"a\nb".to_vec()
        };
        assert_eq!(decode(b"a\r\nb"), expected);
        assert_eq!(decode(b"a\rb"), b"a\rb");
    }

    #[test]
    fn keeps_state_when_download_crlf_is_split_across_chunks() {
        let expected = if cfg!(windows) {
            b"a\r\nb".to_vec()
        } else {
            b"a\nb".to_vec()
        };
        let mut decoder = LocalDecoder::default();
        let mut output = Vec::new();
        decoder.decode(b"a\r", &mut output);
        decoder.decode(b"\nb", &mut output);
        decoder.finish(&mut output);
        assert_eq!(output, expected);
    }
}
