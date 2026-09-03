pub struct ZmodemDetector {
    /// Bytes withheld until they are known not to be a split ZMODEM header.
    pending: Vec<u8>,
    /// Whether `pending[0]` is at the beginning of the stream or directly after a newline.
    pending_starts_at_line_start: bool,
}

impl ZmodemDetector {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            pending_starts_at_line_start: true,
        }
    }

    /// Feed raw bytes and return whether a ZMODEM header was found.
    ///
    /// The direction is inferred from the frame type byte that follows the
    /// header prefix:
    /// - ZRQINIT (0x00) → remote wants to **send** → we **download**
    /// - ZRINIT  (0x01) → remote wants to **receive** → we **upload**
    ///
    /// `passthrough` contains bytes that can be shown in the terminal. Split
    /// header prefixes are retained internally until enough bytes arrive.
    /// When an upload is detected, the "rz -y" shell echo is stripped from
    /// passthrough so the user doesn't see the command.
    pub fn feed(&mut self, data: &[u8]) -> ZmodemDetectResult {
        self.pending.extend_from_slice(data);

        if let Some((direction, header_start)) = detect_zmodem_start(&self.pending) {
            let mut passthrough = self.pending[..header_start].to_vec();
            let initial_bytes = self.pending[header_start..].to_vec();
            self.reset();
            if direction == ZmodemDirection::Upload {
                strip_rz_echo(&mut passthrough);
            }
            return ZmodemDetectResult::Detected {
                direction,
                passthrough,
                initial_bytes,
            };
        }

        let keep_from = retained_prefix_start(&self.pending, self.pending_starts_at_line_start);
        let passthrough = self.pending[..keep_from].to_vec();
        if keep_from > 0 {
            self.pending.drain(..keep_from);
            self.pending_starts_at_line_start = ends_at_line_start(&passthrough);
        }

        ZmodemDetectResult::NoMatch { passthrough }
    }

    pub fn has_pending_prefix(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn reset(&mut self) {
        self.pending.clear();
        self.pending_starts_at_line_start = true;
    }
}

fn detect_zmodem_start(data: &[u8]) -> Option<(ZmodemDirection, usize)> {
    for start in 0..data.len() {
        if data.len().saturating_sub(start) < ZMODEM_HEADER_LEN {
            break;
        }

        let header = &data[start..start + ZMODEM_HEADER_LEN];
        if header[0] != ZPAD
            || header[1] != ZPAD
            || header[2] != ZDLE
            || !matches!(header[3], ZHEX | ZBIN | ZBIN32)
        {
            continue;
        }

        let remaining = &data[start + ZMODEM_HEADER_LEN..];
        let frame_type = if header[3] == ZHEX {
            parse_hex_frame_type(remaining)
        } else {
            remaining.first().copied()
        };

        let direction = match frame_type {
            Some(0x00) => Some(ZmodemDirection::Download),
            Some(0x01) => Some(ZmodemDirection::Upload),
            _ => None,
        };

        if let Some(direction) = direction {
            return Some((direction, start));
        }
    }

    None
}

fn retained_prefix_start(data: &[u8], data_starts_at_line_start: bool) -> usize {
    let max_suffix = data.len().min(ZMODEM_HEADER_LEN + 1);
    for len in (1..=max_suffix).rev() {
        let start = data.len() - len;
        let suffix = &data[start..];
        if !is_possible_zmodem_prefix(suffix) {
            continue;
        }
        if suffix_contains_zdle(suffix)
            || is_line_start(data, start, data_starts_at_line_start)
            || has_rz_receive_prompt_before(data, start)
        {
            return start;
        }
    }
    data.len()
}

fn is_line_start(data: &[u8], start: usize, data_starts_at_line_start: bool) -> bool {
    if start == 0 {
        return data_starts_at_line_start;
    }
    matches!(data.get(start - 1), Some(b'\n' | b'\r'))
}

fn ends_at_line_start(data: &[u8]) -> bool {
    matches!(data.last(), Some(b'\n' | b'\r'))
}

fn suffix_contains_zdle(data: &[u8]) -> bool {
    data.contains(&ZDLE)
}

fn has_rz_receive_prompt_before(data: &[u8], start: usize) -> bool {
    const RZ_RECEIVE_PROMPT: &[u8] = b"z waiting to receive.";
    data[..start]
        .windows(RZ_RECEIVE_PROMPT.len())
        .any(|window| window == RZ_RECEIVE_PROMPT)
}

/// Strip the "rz" shell echo from the end of the passthrough data
/// so that the user doesn't see the upload command in the terminal.
/// Handles common echo variants: "rz\r\n", "rz\r", "rz".
fn strip_rz_echo(data: &mut Vec<u8>) {
    // Try to strip from the end: \r\n, \r, or just the command text.
    // The command is always at the end of the passthrough because the
    // ZMODEM header follows immediately.
    let patterns: &[&[u8]] = &[b"rz\r\n", b"rz\r", b"rz"];
    for &pat in patterns {
        if data.ends_with(pat) {
            data.truncate(data.len() - pat.len());
            return;
        }
    }
}

fn is_possible_zmodem_prefix(data: &[u8]) -> bool {
    match data {
        [] => true,
        [ZPAD] => true,
        [ZPAD, ZPAD] => true,
        [ZPAD, ZPAD, ZDLE] => true,
        [ZPAD, ZPAD, ZDLE, kind] => matches!(*kind, ZHEX | ZBIN | ZBIN32),
        [ZPAD, ZPAD, ZDLE, ZHEX, first_hex] => hex_digit(*first_hex).is_some(),
        _ => false,
    }
}

/// Parse a hex-encoded frame type byte from two ASCII hex chars.
fn parse_hex_frame_type(data: &[u8]) -> Option<u8> {
    if data.len() < 2 {
        return None;
    }
    let hi = hex_digit(data[0])?;
    let lo = hex_digit(data[1])?;
    Some((hi << 4) | lo)
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Transfer state machine
// ---------------------------------------------------------------------------
