//! Pure delivery planning. Protocol selection and input authorization stay with callers.
use std::sync::Arc;
use std::time::Duration;

use crate::quick_command_script::QuickCommandChunk;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendCommandDataType {
    Text,
    Hex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendCommandMode {
    Line,
    Character,
    Packet,
    Byte,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendCommandLineEnding {
    None,
    Cr,
    Lf,
    Crlf,
}

impl SendCommandLineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Cr => "\r",
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextInterpretation {
    Plain,
    Script,
    Literal,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SendCommandStatistics {
    pub units: usize,
    pub bytes: usize,
}

/// Protocol-neutral counters cached when the composer changes, not during rendering.
#[derive(Debug, Clone, Copy, Default)]
pub struct TextSendStatistics {
    content_bytes: usize,
    content_characters: usize,
    breaks: usize,
    lines: usize,
    nonempty_lines: usize,
}

impl TextSendStatistics {
    pub fn from_text(text: &str) -> Self {
        let mut result = Self::default();
        let mut chars = text.chars().peekable();
        let mut line_has_content = false;
        let mut ends_with_break = false;
        while let Some(ch) = chars.next() {
            if matches!(ch, '\r' | '\n') {
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                result.breaks += 1;
                result.nonempty_lines += usize::from(line_has_content);
                line_has_content = false;
                ends_with_break = true;
            } else {
                result.content_bytes += ch.len_utf8();
                result.content_characters += 1;
                line_has_content = true;
                ends_with_break = false;
            }
        }
        result.nonempty_lines += usize::from(line_has_content);
        result.lines = result.breaks + usize::from(!text.is_empty() && !ends_with_break);
        result
    }

    pub fn for_delivery(
        self,
        mode: SendCommandMode,
        ending: SendCommandLineEnding,
    ) -> SendCommandStatistics {
        let ending_bytes = ending.as_str().len();
        let bytes = self.content_bytes + self.breaks * ending_bytes;
        match mode {
            SendCommandMode::Line => SendCommandStatistics {
                units: if ending_bytes == 0 {
                    self.nonempty_lines
                } else {
                    self.lines
                },
                bytes: self.content_bytes + self.lines * ending_bytes,
            },
            SendCommandMode::Character => SendCommandStatistics {
                units: self.content_characters + if ending_bytes == 0 { 0 } else { self.breaks },
                bytes,
            },
            SendCommandMode::Byte => SendCommandStatistics {
                units: bytes,
                bytes,
            },
            SendCommandMode::Packet => SendCommandStatistics {
                units: usize::from(bytes > 0),
                bytes,
            },
        }
    }
}

pub fn parse_send_command_hex(value: &str) -> Result<Vec<u8>, String> {
    let cleaned: String = value.chars().filter(|ch| !ch.is_whitespace()).collect();
    if !cleaned.len().is_multiple_of(2) {
        return Err("invalid hex input: odd number of digits".into());
    }
    if !cleaned.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err("invalid hex input: use 0-9 and A-F".into());
    }
    (0..cleaned.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&cleaned[index..index + 2], 16).map_err(|error| error.to_string())
        })
        .collect()
}

/// A shared payload plus line/pause boundaries, never one allocation per character.
#[derive(Clone)]
pub struct SendCommandPlan {
    bytes: Arc<[u8]>,
    lines: Arc<[usize]>,
    newlines: Arc<[(usize, usize)]>,
    pauses: Arc<[(usize, Duration)]>,
    mode: SendCommandMode,
    interval: Duration,
    offset: usize,
    line: usize,
    pause: usize,
    newline: usize,
    first: bool,
    pending: Option<PendingUnit>,
}

#[derive(Clone)]
struct PendingUnit {
    start: usize,
    end: usize,
    delay: Duration,
}

pub struct SendCommandBatch {
    pub bytes: Vec<u8>,
    pub delay: Duration,
    pub units: u64,
}

impl SendCommandPlan {
    pub fn text(
        text: &str,
        mode: SendCommandMode,
        ending: SendCommandLineEnding,
        interpretation: TextInterpretation,
        interval: Duration,
    ) -> Self {
        let mut bytes = Vec::with_capacity(text.len());
        let mut lines = Vec::new();
        let mut newlines = Vec::new();
        let mut pauses: Vec<(usize, Duration)> = Vec::new();
        let mut chars = text.chars().peekable();
        let mut layout_after_pause = false;
        let mut script_ends_with_return = false;
        while let Some(ch) = chars.next() {
            if interpretation == TextInterpretation::Script && ch == '\\' {
                match chars.next() {
                    Some('p') => {
                        if let Some((offset, delay)) = pauses
                            .last_mut()
                            .filter(|(offset, _)| *offset == bytes.len())
                        {
                            let _ = offset;
                            *delay += Duration::from_secs(1);
                        } else {
                            pauses.push((bytes.len(), Duration::from_secs(1)));
                        }
                        layout_after_pause = script_ends_with_return;
                        continue;
                    }
                    Some(control @ ('r' | 'n' | 't')) => {
                        let control = match control {
                            'r' => b'\r',
                            'n' => b'\n',
                            _ => b'\t',
                        };
                        bytes.push(control);
                        script_ends_with_return = control == b'\r';
                        if matches!(control, b'\r' | b'\n') {
                            lines.push(bytes.len());
                        }
                    }
                    Some('\\') => {
                        bytes.push(b'\\');
                        script_ends_with_return = false;
                    }
                    Some(other) => {
                        bytes.push(b'\\');
                        append_char(&mut bytes, other);
                        script_ends_with_return = false;
                    }
                    None => bytes.push(b'\\'),
                }
                layout_after_pause = false;
                continue;
            }
            if matches!(ch, '\r' | '\n') {
                let crlf = ch == '\r' && chars.peek() == Some(&'\n');
                if crlf {
                    chars.next();
                }
                if layout_after_pause && interpretation == TextInterpretation::Script {
                    continue;
                }
                script_ends_with_return = ch == '\r' && !crlf;
                let start = bytes.len();
                if interpretation == TextInterpretation::Literal {
                    append_char(&mut bytes, ch);
                    if crlf {
                        bytes.push(b'\n');
                    }
                } else {
                    bytes.extend_from_slice(ending.as_str().as_bytes());
                }
                if bytes.len() > start && interpretation != TextInterpretation::Literal {
                    newlines.push((start, bytes.len()));
                }
                lines.push(bytes.len());
            } else {
                append_char(&mut bytes, ch);
                script_ends_with_return = false;
                layout_after_pause = false;
            }
        }
        // Match the legacy all-pauses case instead of silently discarding its text.
        if bytes.is_empty()
            && !text.is_empty()
            && interpretation == TextInterpretation::Script
            && !pauses.is_empty()
        {
            bytes.extend_from_slice(text.as_bytes());
            pauses.clear();
        }
        if mode == SendCommandMode::Line
            && interpretation != TextInterpretation::Literal
            && !bytes.is_empty()
            && !bytes.ends_with(b"\r")
            && !bytes.ends_with(b"\n")
        {
            bytes.extend_from_slice(ending.as_str().as_bytes());
        }
        if lines.last().copied() != Some(bytes.len()) {
            lines.push(bytes.len());
        }
        lines.sort_unstable();
        lines.dedup();
        Self {
            bytes: bytes.into(),
            lines: lines.into(),
            newlines: newlines.into(),
            pauses: pauses.into(),
            mode,
            interval,
            offset: 0,
            line: 0,
            pause: 0,
            newline: 0,
            first: true,
            pending: None,
        }
    }

    pub fn hex(text: &str, mode: SendCommandMode, interval: Duration) -> Result<Self, String> {
        let bytes: Arc<[u8]> = parse_send_command_hex(text)?.into();
        Ok(Self {
            lines: vec![bytes.len()].into(),
            bytes,
            newlines: Arc::from([]),
            pauses: Arc::from([]),
            mode,
            interval,
            offset: 0,
            line: 0,
            pause: 0,
            newline: 0,
            first: true,
            pending: None,
        })
    }

    pub fn from_chunks(chunks: Vec<QuickCommandChunk>) -> Self {
        let mut bytes = Vec::new();
        let mut lines = Vec::new();
        let mut pauses = Vec::new();
        for chunk in chunks {
            if !chunk.delay.is_zero() {
                pauses.push((bytes.len(), chunk.delay));
            }
            bytes.extend_from_slice(&chunk.bytes);
            lines.push(bytes.len());
        }
        Self {
            bytes: bytes.into(),
            lines: lines.into(),
            newlines: Arc::from([]),
            pauses: pauses.into(),
            mode: SendCommandMode::Line,
            interval: Duration::ZERO,
            offset: 0,
            line: 0,
            pause: 0,
            newline: 0,
            first: true,
            pending: None,
        }
    }

    pub fn unit_count(&self) -> u64 {
        let mut plan = self.clone();
        let mut count = 0;
        while plan.next_unit().is_some() {
            count += 1;
        }
        count
    }

    /// Merge only zero-delay units. Large units are split on UTF-8 boundaries;
    /// continuation writes carry no extra interval and complete the unit once.
    pub fn next_batch(&mut self, limit: usize) -> Option<SendCommandBatch> {
        assert!(limit >= 4);
        let first = self.pending.take().or_else(|| self.next_unit())?;
        let mut result = SendCommandBatch {
            delay: first.delay,
            bytes: Vec::with_capacity(limit),
            units: 0,
        };
        let mut current = first;
        loop {
            let available = limit - result.bytes.len();
            if current.end - current.start > available {
                let mut split = current.start + available;
                if !matches!(self.mode, SendCommandMode::Byte | SendCommandMode::Packet) {
                    while split > current.start
                        && self
                            .bytes
                            .get(split)
                            .is_some_and(|byte| byte & 0xc0 == 0x80)
                    {
                        split -= 1;
                    }
                }
                result
                    .bytes
                    .extend_from_slice(&self.bytes[current.start..split]);
                current.start = split;
                current.delay = Duration::ZERO;
                self.pending = Some(current);
                break;
            }
            result
                .bytes
                .extend_from_slice(&self.bytes[current.start..current.end]);
            result.units += 1;
            if result.bytes.len() == limit {
                break;
            }
            let Some(next) = self.next_unit() else {
                break;
            };
            if !next.delay.is_zero() {
                self.pending = Some(next);
                break;
            }
            current = next;
        }
        Some(result)
    }
}

impl SendCommandPlan {
    fn next_unit(&mut self) -> Option<PendingUnit> {
        if self.offset >= self.bytes.len() {
            return None;
        }
        while self.line < self.lines.len() && self.lines[self.line] <= self.offset {
            self.line += 1;
        }
        while self.newline < self.newlines.len() && self.newlines[self.newline].0 < self.offset {
            self.newline += 1;
        }
        let mut delay = if self.first {
            Duration::ZERO
        } else {
            self.interval
        };
        while self.pause < self.pauses.len() && self.pauses[self.pause].0 <= self.offset {
            delay += self.pauses[self.pause].1;
            self.pause += 1;
        }
        let mut end = match self.mode {
            SendCommandMode::Byte => self.offset + 1,
            SendCommandMode::Packet => self.bytes.len(),
            SendCommandMode::Line => self
                .lines
                .get(self.line)
                .copied()
                .unwrap_or(self.bytes.len()),
            SendCommandMode::Character => {
                if let Some((_, end)) = self
                    .newlines
                    .get(self.newline)
                    .filter(|(start, _)| *start == self.offset)
                {
                    *end
                } else {
                    self.offset
                        + match self.bytes[self.offset] {
                            0..=0x7f => 1,
                            0xc0..=0xdf => 2,
                            0xe0..=0xef => 3,
                            _ => 4,
                        }
                }
            }
        };
        if let Some((offset, _)) = self
            .pauses
            .get(self.pause)
            .filter(|(offset, _)| *offset > self.offset)
        {
            end = end.min(*offset);
        }
        let start = self.offset;
        self.offset = end;
        self.first = false;
        Some(PendingUnit { start, end, delay })
    }
}

impl Iterator for SendCommandPlan {
    type Item = QuickCommandChunk;
    fn next(&mut self) -> Option<Self::Item> {
        let unit = self.pending.take().or_else(|| self.next_unit())?;
        Some(QuickCommandChunk {
            bytes: self.bytes[unit.start..unit.end].to_vec(),
            delay: unit.delay,
        })
    }
}

fn append_char(bytes: &mut Vec<u8>, ch: char) {
    bytes.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
}

/// Parse decimal seconds without floating-point rounding or silent precision loss.
pub fn parse_interval_ms(input: &str) -> Option<u64> {
    let input = input.trim();
    let (whole, fraction) = input.split_once('.').unwrap_or((input, ""));
    if fraction.len() > 3
        || !whole.bytes().all(|ch| ch.is_ascii_digit())
        || !fraction.bytes().all(|ch| ch.is_ascii_digit())
        || (whole.is_empty() && fraction.is_empty())
    {
        return None;
    }
    let seconds = if whole.is_empty() {
        0
    } else {
        whole.parse::<u64>().ok()?
    };
    let fraction = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u64>().ok()? * 10u64.pow(3 - fraction.len() as u32)
    };
    seconds
        .checked_mul(1_000)?
        .checked_add(fraction)
        .filter(|value| *value <= 60_000)
}

pub fn format_interval_ms(interval: u64) -> String {
    let mut value = format!("{}.{:03}", interval / 1_000, interval % 1_000);
    while value.ends_with('0') && !value.ends_with(".00") {
        value.pop();
    }
    value
}

#[cfg(test)]
mod tests;
