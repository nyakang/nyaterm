//! Pre-parser graphics protocol ingress.
//!
//! Kitty APC (`ESC _ G … ST`), iTerm2 OSC 1337 inline files, and Sixel DCS
//! payloads are intercepted *before* Alacritty's ANSI processor so their
//! binary/base64 bodies never pollute the character grid.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Which protocol produced a graphics payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsProtocol {
    Kitty,
    ITerm2,
    Sixel,
}

/// Kitty `a=d` delete target (`d=` key). Uppercase letters free stored data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyDeleteMode {
    /// `d=a` / `d=A` — all placements.
    All,
    /// `d=i` / `d=I` — placements for an image id.
    Image,
    /// `d=n` / `d=N` — newest placement (optionally of an image id).
    Newest,
    /// `d=c` / `d=C` — placements intersecting a cell.
    Cell,
    /// `d=p` / `d=P` — one placement id of an image.
    Placement,
    /// `d=x` / `d=X` — placements intersecting a column.
    Column,
    /// `d=y` / `d=Y` — placements intersecting a row (absolute line).
    Row,
    /// `d=z` / `d=Z` — placements with a matching z-index.
    ZIndex,
}

/// Events emitted by [`GraphicsIngress`] as it splits a byte stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphicsEvent {
    /// An inline / transmitted image that should be placed on the grid.
    Image {
        protocol: GraphicsProtocol,
        id: Option<u32>,
        /// Kitty placement id (`p`), when the host assigns one.
        placement_id: Option<u32>,
        width_cells: Option<u16>,
        height_cells: Option<u16>,
        /// Encoded image bytes (PNG/JPEG/GIF/…) when the protocol carries them.
        data: Vec<u8>,
        name: Option<String>,
        /// Kitty `m=1` means more base64 chunks follow for this image id.
        more: bool,
        /// Whether to place on the grid now (`a=T` / `a=p`). `a=t` is store-only.
        place: bool,
        /// Kitty z-index (`z`); `z>0` paints above text.
        z_index: i32,
        /// Kitty `z>0` paints above text; otherwise under text.
        above_text: bool,
        /// Kitty `C=1`: after placing, move cursor past the image.
        cursor_motion: bool,
        /// Kitty `f=` pixel format (24 / 32 / 100). `None` leaves payload as-is.
        format: Option<u32>,
        /// Kitty `o=z` zlib compression on the transmit payload.
        compressed: bool,
        /// Kitty `s=` pixel width (raw RGB/RGBA formats).
        pixel_width: Option<u32>,
        /// Kitty `v=` pixel height (raw RGB/RGBA formats).
        pixel_height: Option<u32>,
        /// Kitty `q=` response mode: 0=never, 1=errors only, 2=always.
        quiet: u8,
    },
    /// Kitty `a=q` image/placement existence query.
    Query {
        image_id: Option<u32>,
        placement_id: Option<u32>,
    },
    /// Kitty `a=d` delete request.
    Delete {
        mode: KittyDeleteMode,
        /// Uppercase `d` variants also free image data from the store.
        free_data: bool,
        image_id: Option<u32>,
        placement_id: Option<u32>,
        /// 1-based cell column from protocol (`x`), when present.
        col: Option<u32>,
        /// 1-based cell row from protocol (`y`), when present.
        row: Option<u32>,
        /// z-index filter for [`KittyDeleteMode::ZIndex`].
        z: Option<i32>,
    },
    /// iTerm2 OSC 1337 `ClearScrollback`.
    ClearScrollback,
}

/// Ordered stream segments from the ingress scanner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphicsSegment {
    Terminal(Vec<u8>),
    Event(GraphicsEvent),
}

/// Borrowed ordered stream segments from the ingress scanner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphicsSegmentRef<'a> {
    Terminal(&'a [u8]),
    Event(GraphicsEvent),
}

/// Maximum incomplete graphics sequence retained across output chunks.
const GRAPHICS_PENDING_LIMIT: usize = 8 * 1024 * 1024;
const MAX_KITTY_PENDING_BYTES: usize = 8 * 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_KITTY_STORED_IMAGES: usize = 64;
const MAX_KITTY_STORE_BYTES: usize = 16 * 1024 * 1024;

/// Stateful splitter that may hold an incomplete escape sequence across chunks.
#[derive(Debug, Default)]
pub struct GraphicsIngress {
    /// Bytes that start an unfinished graphics sequence from a prior chunk.
    pending: Vec<u8>,
}

impl GraphicsIngress {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed raw session output and return ordered terminal / graphics segments.
    ///
    /// Only a real graphics introducer (`ESC _` / `ESC ]` / `ESC P`) splits the
    /// stream. Every other escape — every SGR colour code, every cursor move —
    /// stays inside the surrounding run, so a TUI frame carrying hundreds of
    /// CSI sequences still leaves here as a single [`GraphicsSegment::Terminal`]
    /// instead of hundreds of one-per-escape allocations.
    pub fn advance(&mut self, input: &[u8]) -> Vec<GraphicsSegment> {
        let mut out = Vec::new();
        self.advance_segments(input, |segment| match segment {
            GraphicsSegmentRef::Terminal(bytes) => {
                out.push(GraphicsSegment::Terminal(bytes.to_vec()));
            }
            GraphicsSegmentRef::Event(event) => out.push(GraphicsSegment::Event(event)),
        });
        out
    }

    /// Feed raw session output and visit ordered terminal / graphics segments.
    ///
    /// Ordinary terminal bytes are borrowed from the caller's input in the
    /// common case. Bytes are copied only when an incomplete graphics sequence
    /// from a prior chunk must be spliced with the new input.
    pub fn advance_with<T, G>(&mut self, input: &[u8], mut on_terminal: T, mut on_graphics: G)
    where
        T: FnMut(&[u8]),
        G: FnMut(GraphicsEvent),
    {
        self.advance_segments(input, |segment| match segment {
            GraphicsSegmentRef::Terminal(bytes) => on_terminal(bytes),
            GraphicsSegmentRef::Event(event) => on_graphics(event),
        });
    }

    /// Feed raw session output and visit borrowed ordered segments.
    pub fn advance_segments(
        &mut self,
        input: &[u8],
        mut on_segment: impl FnMut(GraphicsSegmentRef<'_>),
    ) {
        if input.is_empty() && self.pending.is_empty() {
            return;
        }
        // Splice only when a prior chunk left an unfinished sequence behind;
        // the common case borrows the caller's bytes and copies nothing.
        let buf = if self.pending.is_empty() {
            Cow::Borrowed(input)
        } else {
            let mut spliced = std::mem::take(&mut self.pending);
            spliced.extend_from_slice(input);
            Cow::Owned(spliced)
        };
        let buf: &[u8] = &buf;
        let mut i = 0usize;
        let mut terminal_start = 0usize;

        while i < buf.len() {
            let Some(offset) = memchr::memchr(0x1b, &buf[i..]) else {
                break;
            };
            i += offset;
            match classify_at(buf, i) {
                SequenceClass::Incomplete => {
                    if buf.len().saturating_sub(i) > GRAPHICS_PENDING_LIMIT {
                        // Oversized unfinished sequence: give up on it and hand
                        // everything still unflushed to the terminal parser.
                        if terminal_start < buf.len() {
                            on_segment(GraphicsSegmentRef::Terminal(&buf[terminal_start..]));
                        }
                        self.pending.clear();
                        return;
                    }
                    if i > terminal_start {
                        on_segment(GraphicsSegmentRef::Terminal(&buf[terminal_start..i]));
                    }
                    self.pending = buf[i..].to_vec();
                    return;
                }
                SequenceClass::NotGraphics => {
                    // Leave ESC to the terminal parser, inside the current run.
                    i += 1;
                }
                SequenceClass::Complete { end, event } => {
                    // A graphics sequence does split the stream: flush the plain
                    // terminal bytes ahead of it first.
                    if i > terminal_start {
                        on_segment(GraphicsSegmentRef::Terminal(&buf[terminal_start..i]));
                    }
                    if let Some(event) = event {
                        on_segment(GraphicsSegmentRef::Event(event));
                    }
                    i = end;
                    terminal_start = i;
                }
            }
        }

        if terminal_start < buf.len() {
            on_segment(GraphicsSegmentRef::Terminal(&buf[terminal_start..]));
        }
    }
}

enum SequenceClass {
    Incomplete,
    NotGraphics,
    Complete {
        end: usize,
        event: Option<GraphicsEvent>,
    },
}

fn classify_at(buf: &[u8], start: usize) -> SequenceClass {
    // Need at least ESC + one more byte to classify.
    if start + 1 >= buf.len() {
        return SequenceClass::Incomplete;
    }
    match buf[start + 1] {
        // Kitty APC: ESC _ G …
        b'_' => classify_kitty_apc(buf, start),
        // OSC: ESC ] …
        b']' => classify_osc(buf, start),
        // DCS / Sixel: ESC P …
        b'P' => classify_dcs(buf, start),
        _ => SequenceClass::NotGraphics,
    }
}

fn classify_kitty_apc(buf: &[u8], start: usize) -> SequenceClass {
    // ESC _ G key=value,… ; payload ST
    if start + 2 >= buf.len() {
        return SequenceClass::Incomplete;
    }
    if buf[start + 2] != b'G' {
        return SequenceClass::NotGraphics;
    }
    match find_st(buf, start + 3) {
        None => SequenceClass::Incomplete,
        Some(st_at) => {
            let body = &buf[start + 3..st_at];
            let event = parse_kitty_body(body);
            SequenceClass::Complete {
                end: st_end(buf, st_at),
                event,
            }
        }
    }
}

fn classify_osc(buf: &[u8], start: usize) -> SequenceClass {
    // ESC ] 1337 ; …
    let after = start + 2;
    if after >= buf.len() {
        return SequenceClass::Incomplete;
    }
    // Peek whether this is 1337 without consuming non-1337 OSCs.
    let rest = &buf[after..];
    if rest.starts_with(b"1337") {
        if rest.len() > 4 && rest[4] != b';' {
            return SequenceClass::NotGraphics;
        }
        // Need the full OSC terminator.
        match find_osc_terminator(buf, after + 4) {
            None => SequenceClass::Incomplete,
            Some(term_at) => {
                let body = &buf[after + 4..term_at];
                // body starts with optional ';' then File=… or other commands
                let event = parse_iterm2_body(body);
                SequenceClass::Complete {
                    end: osc_end(buf, term_at),
                    event,
                }
            }
        }
    } else if is_incomplete_osc_prefix(rest) {
        SequenceClass::Incomplete
    } else {
        SequenceClass::NotGraphics
    }
}

fn is_incomplete_osc_prefix(rest: &[u8]) -> bool {
    // If we only have a prefix of "1337", wait for more bytes.
    const TARGET: &[u8] = b"1337";
    if rest.is_empty() {
        return true;
    }
    if rest.len() < TARGET.len() && TARGET.starts_with(rest) {
        return true;
    }
    false
}

fn classify_dcs(buf: &[u8], start: usize) -> SequenceClass {
    // ESC P … ST — only intercept sixel (params… q …). Other DCS pass through.
    let body_start = start + 2;
    if body_start >= buf.len() {
        return SequenceClass::Incomplete;
    }
    // Look ahead for sixel introducer `q` before ST; if we cannot finish, hold
    // only while the prefix is still compatible with a Sixel DCS.
    match find_st(buf, body_start) {
        None => {
            let probe = &buf[body_start..];
            if looks_like_incomplete_sixel_dcs(probe) {
                SequenceClass::Incomplete
            } else {
                SequenceClass::NotGraphics
            }
        }
        Some(st_at) => {
            let body = &buf[body_start..st_at];
            if sixel_introducer_pos(body).is_none() {
                return SequenceClass::NotGraphics;
            }
            SequenceClass::Complete {
                end: st_end(buf, st_at),
                event: Some(GraphicsEvent::Image {
                    protocol: GraphicsProtocol::Sixel,
                    id: None,
                    placement_id: None,
                    width_cells: None,
                    height_cells: None,
                    // Full DCS body (`params q payload`) for the rasterizer.
                    data: body.to_vec(),
                    name: None,
                    more: false,
                    place: true,
                    z_index: 0,
                    above_text: false,
                    cursor_motion: false,
                    format: None,
                    compressed: false,
                    pixel_width: None,
                    pixel_height: None,
                    quiet: 0,
                }),
            }
        }
    }
}

fn looks_like_incomplete_sixel_dcs(body: &[u8]) -> bool {
    match sixel_introducer_pos(body) {
        Some(_) => true,
        None => body.iter().all(|&b| is_sixel_param_byte(b)),
    }
}

fn sixel_introducer_pos(body: &[u8]) -> Option<usize> {
    for (idx, &b) in body.iter().enumerate() {
        if b == b'q' {
            return Some(idx);
        }
        if !is_sixel_param_byte(b) {
            return None;
        }
    }
    None
}

fn is_sixel_param_byte(b: u8) -> bool {
    matches!(b, b'0'..=b'9' | b';' | b'?' | b' ')
}

fn find_st(buf: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 1 < buf.len() {
        if buf[i] == 0x1b && buf[i + 1] == b'\\' {
            return Some(i);
        }
        // BEL is not a standard ST for APC/DCS but some hosts use it.
        if buf[i] == 0x07 {
            return Some(i);
        }
        i += 1;
    }
    if from < buf.len() && buf[buf.len() - 1] == 0x07 {
        return Some(buf.len() - 1);
    }
    None
}

fn st_end(buf: &[u8], st_at: usize) -> usize {
    if buf.get(st_at) == Some(&0x07) {
        st_at + 1
    } else {
        st_at + 2 // ESC \
    }
}

fn find_osc_terminator(buf: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i < buf.len() {
        if buf[i] == 0x07 {
            return Some(i);
        }
        if buf[i] == 0x1b {
            if i + 1 >= buf.len() {
                return None;
            }
            if buf[i + 1] == b'\\' {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

fn osc_end(buf: &[u8], term_at: usize) -> usize {
    if buf.get(term_at) == Some(&0x07) {
        term_at + 1
    } else {
        term_at + 2
    }
}

fn parse_kitty_body(body: &[u8]) -> Option<GraphicsEvent> {
    // Split control (key=value,…) and optional payload after first ';'.
    let (control, payload) = match body.iter().position(|&b| b == b';') {
        Some(idx) => (&body[..idx], &body[idx + 1..]),
        None => (body, &body[body.len()..]),
    };
    let control = std::str::from_utf8(control).ok()?;
    let mut params = HashMap::new();
    for part in control.split(',') {
        if part.is_empty() {
            continue;
        }
        let mut kv = part.splitn(2, '=');
        let key = kv.next().unwrap_or("");
        let value = kv.next().unwrap_or("");
        params.insert(key, value);
    }
    let action = params.get("a").copied().unwrap_or("t");
    match action {
        "d" | "D" => Some(parse_kitty_delete(&params)),
        "q" | "Q" => {
            let image_id = params.get("i").and_then(|v| v.parse().ok());
            let placement_id = params.get("p").and_then(|v| v.parse().ok());
            Some(GraphicsEvent::Query {
                image_id,
                placement_id,
            })
        }
        // t=transmit store, T=transmit+place, p=place existing id
        "t" | "T" | "p" | "P" => {
            let id = params.get("i").and_then(|v| v.parse().ok());
            let placement_id = params.get("p").and_then(|v| v.parse().ok());
            let width_cells = params.get("c").and_then(|v| v.parse().ok());
            let height_cells = params.get("r").and_then(|v| v.parse().ok());
            let more = params.get("m").map(|v| *v == "1").unwrap_or(false);
            let z_index: i32 = params.get("z").and_then(|v| v.parse().ok()).unwrap_or(0);
            let above_text = z_index > 0;
            let cursor_motion = params.get("C").map(|v| *v == "1").unwrap_or(false);
            let format = params.get("f").and_then(|v| v.parse().ok());
            let compressed = params
                .get("o")
                .map(|v| *v == "z" || *v == "Z")
                .unwrap_or(false);
            let pixel_width = params.get("s").and_then(|v| v.parse().ok());
            let pixel_height = params.get("v").and_then(|v| v.parse().ok());
            let quiet = params
                .get("q")
                .and_then(|v| v.parse::<u8>().ok())
                .unwrap_or(0)
                .min(2);
            let place = matches!(action, "T" | "p" | "P");
            let data = if payload.is_empty() {
                Vec::new()
            } else {
                decode_base64_ignore_ws(payload)
            };
            // place-only with id and no payload is valid (reuse stored image).
            if data.is_empty()
                && !more
                && !place
                && width_cells.is_none()
                && height_cells.is_none()
                && id.is_none()
            {
                return None;
            }
            if place && data.is_empty() && !more && id.is_none() {
                return None;
            }
            Some(GraphicsEvent::Image {
                protocol: GraphicsProtocol::Kitty,
                id,
                placement_id,
                width_cells,
                height_cells,
                data,
                name: None,
                more,
                place,
                z_index,
                above_text,
                cursor_motion,
                format,
                compressed,
                pixel_width,
                pixel_height,
                quiet,
            })
        }
        _ => None,
    }
}

fn parse_kitty_delete(params: &HashMap<&str, &str>) -> GraphicsEvent {
    let image_id = params.get("i").and_then(|v| v.parse().ok());
    let placement_id = params.get("p").and_then(|v| v.parse().ok());
    let col = params.get("x").and_then(|v| v.parse().ok());
    let row = params.get("y").and_then(|v| v.parse().ok());
    let z = params.get("z").and_then(|v| v.parse().ok());
    let d_raw = params.get("d").copied().unwrap_or("");
    let (mode, free_data) = if d_raw.is_empty() {
        // Shorthand: a=d,i=N deletes that image (and frees data). Bare a=d clears all.
        if image_id.is_some() {
            (KittyDeleteMode::Image, true)
        } else {
            (KittyDeleteMode::All, true)
        }
    } else {
        let ch = d_raw.chars().next().unwrap_or('a');
        let free_data = ch.is_ascii_uppercase();
        let mode = match ch.to_ascii_lowercase() {
            'a' => KittyDeleteMode::All,
            'i' => KittyDeleteMode::Image,
            'n' => KittyDeleteMode::Newest,
            'c' => KittyDeleteMode::Cell,
            'p' => KittyDeleteMode::Placement,
            'x' => KittyDeleteMode::Column,
            'y' => KittyDeleteMode::Row,
            'z' => KittyDeleteMode::ZIndex,
            _ if image_id.is_some() => KittyDeleteMode::Image,
            _ => KittyDeleteMode::All,
        };
        (mode, free_data)
    };
    GraphicsEvent::Delete {
        mode,
        free_data,
        image_id,
        placement_id,
        col,
        row,
        z,
    }
}

fn parse_iterm2_body(body: &[u8]) -> Option<GraphicsEvent> {
    // Formats:
    // ;File=name=…;width=…;height=…;inline=1:BASE64
    // File=… (without leading ;)
    let text = std::str::from_utf8(body).ok()?;
    let text = text.strip_prefix(';').unwrap_or(text);
    if !text.starts_with("File=") {
        if text.eq_ignore_ascii_case("ClearScrollback") {
            return Some(GraphicsEvent::ClearScrollback);
        }
        return None;
    }
    let payload_sep = text.find(':')?;
    let header = &text[..payload_sep];
    let b64 = &text.as_bytes()[payload_sep + 1..];
    let mut width_cells = None;
    let mut height_cells = None;
    let mut name = None;
    let mut inline = false;
    for part in header.split(';') {
        let mut kv = part.splitn(2, '=');
        let key = kv.next().unwrap_or("");
        let value = kv.next().unwrap_or("");
        match key {
            "File" | "name" => {
                if !value.is_empty() {
                    name = Some(value.to_string());
                }
            }
            "width" => width_cells = parse_cell_extent(value),
            "height" => height_cells = parse_cell_extent(value),
            "inline" => inline = value == "1" || value.eq_ignore_ascii_case("true"),
            _ => {}
        }
    }
    if !inline {
        // Download-only File= without inline — still strip; no placement.
        return None;
    }
    let data = decode_base64_ignore_ws(b64);
    Some(GraphicsEvent::Image {
        protocol: GraphicsProtocol::ITerm2,
        id: None,
        placement_id: None,
        width_cells,
        height_cells,
        data,
        name,
        more: false,
        place: true,
        z_index: 0,
        above_text: false,
        cursor_motion: false,
        format: None,
        compressed: false,
        pixel_width: None,
        pixel_height: None,
        quiet: 0,
    })
}

fn parse_cell_extent(value: &str) -> Option<u16> {
    // N, Npx, N%, auto
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("auto") {
        return None;
    }
    let digits: String = value.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let n: u16 = digits.parse().ok()?;
    if value.ends_with("px") {
        // Approximate: assume ~8px per cell if only pixels given.
        Some((n / 8).max(1))
    } else if value.ends_with('%') {
        None
    } else {
        Some(n.max(1))
    }
}

fn decode_base64_ignore_ws(input: &[u8]) -> Vec<u8> {
    let filtered: Vec<u8> = input
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    decode_base64_std(&filtered)
}

fn decode_base64_std(input: &[u8]) -> Vec<u8> {
    // Minimal standard base64 decoder (no external dep).
    fn val(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            b'=' => Some(0x80), // padding marker
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut chunk = [0u8; 4];
    let mut n = 0usize;
    for &b in input {
        let Some(v) = val(b) else {
            continue;
        };
        chunk[n] = v;
        n += 1;
        if n == 4 {
            let pad = chunk.iter().filter(|&&x| x == 0x80).count();
            for c in &mut chunk {
                if *c == 0x80 {
                    *c = 0;
                }
            }
            out.push((chunk[0] << 2) | (chunk[1] >> 4));
            if pad < 2 {
                out.push((chunk[1] << 4) | (chunk[2] >> 2));
            }
            if pad < 1 {
                out.push((chunk[2] << 6) | chunk[3]);
            }
            n = 0;
        }
    }
    out
}

/// Stable image placement retained by the terminal model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphicsPlacement {
    pub id: u64,
    pub protocol: GraphicsProtocol,
    /// Stable logical line at the top of the image.
    pub line: i64,
    /// Screen buffer this placement belongs to.
    pub screen: GraphicsScreenKind,
    pub col: usize,
    pub width_cells: usize,
    pub height_cells: usize,
    pub data: Arc<[u8]>,
    pub content_id: u64,
    pub name: Option<String>,
    /// Kitty image number used for delete / place-only reuse.
    pub kitty_id: Option<u32>,
    /// Kitty placement id (`p`), when assigned.
    pub placement_id: Option<u32>,
    /// Kitty z-index (`z`).
    pub z_index: i32,
    /// When true, paint above terminal text (Kitty `z>0`).
    pub above_text: bool,
}

/// Stable terminal screen identity for graphics placements.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum GraphicsScreenKind {
    #[default]
    Primary,
    Alternate,
}

/// Snapshot form of a placement, mapped into the current viewport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphicsImageSnapshot {
    pub id: u64,
    pub protocol: GraphicsProtocol,
    /// Viewport row where the visible portion starts.
    pub row: usize,
    /// Viewport column where the visible portion starts.
    pub col: usize,
    /// Visible width after clipping to the viewport.
    pub width_cells: usize,
    /// Visible height after clipping to the viewport.
    pub height_cells: usize,
    /// Full image width before viewport clipping.
    pub image_width_cells: usize,
    /// Full image height before viewport clipping.
    pub image_height_cells: usize,
    /// Cell offset into the source image for the visible top row.
    pub source_row_cells: usize,
    /// Cell offset into the source image for the visible left column.
    pub source_col_cells: usize,
    pub data: Arc<[u8]>,
    pub content_id: u64,
    pub name: Option<String>,
    pub above_text: bool,
}

/// Result of applying a graphics event to terminal state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphicsHandleResult {
    /// Kitty `C=1`: move text cursor past the placed image.
    pub cursor_motion: Option<GraphicsCursorMotion>,
    /// Kitty graphics protocol replies (`ESC_G … ST`) for the PTY.
    pub pty_writes: Vec<Vec<u8>>,
}

/// Placement geometry used to build a cursor-motion escape sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphicsCursorMotion {
    pub start_col: usize,
    pub width_cells: usize,
    pub height_cells: usize,
}

impl GraphicsCursorMotion {
    /// CSI sequences that move from the placement top-left to just after
    /// the bottom-right cell (`CUD` + `CHA`).
    pub fn to_ansi(self) -> Vec<u8> {
        let mut out = Vec::new();
        if self.height_cells > 1 {
            out.extend(format!("[{}B", self.height_cells - 1).bytes());
        }
        // 1-based column after the image's right edge.
        let col = self
            .start_col
            .saturating_add(self.width_cells)
            .saturating_add(1);
        out.extend(format!("[{}G", col.max(1)).bytes());
        out
    }
}

/// Mutable graphics state owned by [`crate::TerminalCore`].
#[derive(Debug, Default)]
struct PendingKittyTransfer {
    data: Vec<u8>,
    width_cells: Option<u16>,
    height_cells: Option<u16>,
    name: Option<String>,
    place: bool,
    placement_id: Option<u32>,
    z_index: i32,
    above_text: bool,
    format: Option<u32>,
    compressed: bool,
    pixel_width: Option<u32>,
    pixel_height: Option<u32>,
    overflowed: bool,
}

#[derive(Debug, Clone, Default)]
struct StoredKittyImage {
    data: Arc<[u8]>,
    width_cells: Option<u16>,
    height_cells: Option<u16>,
    name: Option<String>,
    last_used: u64,
}

#[derive(Debug, Default)]
pub struct TerminalGraphicsState {
    next_id: u64,
    store_generation: u64,
    placements: Vec<GraphicsPlacement>,
    /// Kitty image number → last transmitted payload (for place-only / multi-place).
    kitty_store: HashMap<u32, StoredKittyImage>,
    /// In-flight Kitty multi-chunk transfers keyed by image id (0 when absent).
    pending_kitty: HashMap<u32, PendingKittyTransfer>,
}

impl TerminalGraphicsState {
    /// Handle an event on the primary screen using unshifted coordinates.
    ///
    /// Kept for protocol-focused users and tests; terminal integrations should
    /// use [`Self::handle_on_screen`] so placements survive ring-buffer rotation.
    pub fn handle(
        &mut self,
        event: GraphicsEvent,
        cursor_line: i32,
        cursor_col: usize,
        screen_cols: usize,
        cell_width_px: u16,
        cell_height_px: u16,
    ) -> GraphicsHandleResult {
        self.handle_on_screen(
            event,
            GraphicsScreenKind::Primary,
            0,
            cursor_line,
            cursor_col,
            screen_cols,
            cell_width_px,
            cell_height_px,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn handle_on_screen(
        &mut self,
        event: GraphicsEvent,
        screen: GraphicsScreenKind,
        logical_origin: i64,
        cursor_line: i32,
        cursor_col: usize,
        screen_cols: usize,
        cell_width_px: u16,
        cell_height_px: u16,
    ) -> GraphicsHandleResult {
        let cursor_line = i64::from(cursor_line).saturating_add(logical_origin);
        let mut result = GraphicsHandleResult::default();
        match event {
            GraphicsEvent::ClearScrollback => {}
            GraphicsEvent::Query {
                image_id,
                placement_id,
            } => {
                let ok = self.kitty_query_ok(screen, image_id, placement_id);
                result
                    .pty_writes
                    .push(kitty_graphics_reply(image_id, placement_id, ok));
            }
            GraphicsEvent::Delete {
                mode,
                free_data,
                image_id,
                placement_id,
                col,
                row,
                z,
            } => {
                self.handle_delete(
                    mode,
                    free_data,
                    image_id,
                    placement_id,
                    col,
                    row,
                    z,
                    logical_origin,
                    cursor_line,
                    cursor_col,
                    screen,
                );
            }
            GraphicsEvent::Image {
                protocol,
                id,
                placement_id,
                width_cells,
                height_cells,
                data,
                name,
                more,
                place,
                z_index,
                above_text,
                cursor_motion,
                format,
                compressed,
                pixel_width,
                pixel_height,
                quiet,
            } => {
                if protocol == GraphicsProtocol::Kitty {
                    let (motion, reply) = self.handle_kitty_image(
                        id,
                        placement_id,
                        width_cells,
                        height_cells,
                        data,
                        name,
                        more,
                        place,
                        z_index,
                        above_text,
                        cursor_motion,
                        format,
                        compressed,
                        pixel_width,
                        pixel_height,
                        quiet,
                        cursor_line,
                        cursor_col,
                        screen,
                        screen_cols,
                        cell_width_px,
                        cell_height_px,
                    );
                    result.cursor_motion = motion;
                    if let Some(reply) = reply {
                        result.pty_writes.push(reply);
                    }
                } else if place {
                    let data = if protocol == GraphicsProtocol::Sixel {
                        match crate::sixel::decode_sixel_rgba(&data) {
                            Some((w, h, rgba)) => crate::sixel::pack_nyar_rgba(w, h, &rgba),
                            None => data,
                        }
                    } else {
                        data
                    };
                    let motion = self.push_placement(
                        protocol,
                        None,
                        None,
                        width_cells,
                        height_cells,
                        data,
                        name,
                        cursor_line,
                        cursor_col,
                        screen,
                        screen_cols,
                        cell_width_px,
                        cell_height_px,
                        0,
                        above_text,
                    );
                    if cursor_motion {
                        result.cursor_motion = Some(motion);
                    }
                }
            }
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_delete(
        &mut self,
        mode: KittyDeleteMode,
        free_data: bool,
        image_id: Option<u32>,
        placement_id: Option<u32>,
        col: Option<u32>,
        row: Option<u32>,
        z: Option<i32>,
        logical_origin: i64,
        cursor_line: i64,
        cursor_col: usize,
        screen: GraphicsScreenKind,
    ) {
        let cell_col = col
            .map(|v| v.saturating_sub(1) as usize)
            .unwrap_or(cursor_col);
        let cell_line = row
            .map(|v| i64::from(v.saturating_sub(1)).saturating_add(logical_origin))
            .unwrap_or(cursor_line);

        let mut freed_ids: Vec<u32> = Vec::new();
        match mode {
            KittyDeleteMode::All => {
                if free_data {
                    self.placements
                        .retain(|placement| placement.screen != screen);
                    self.kitty_store.clear();
                    self.pending_kitty.clear();
                } else {
                    self.placements
                        .retain(|placement| placement.screen != screen);
                }
                return;
            }
            KittyDeleteMode::Image => {
                let Some(id) = image_id else {
                    return;
                };
                self.placements.retain(|p| {
                    if p.screen == screen && p.kitty_id == Some(id) {
                        freed_ids.push(id);
                        false
                    } else {
                        true
                    }
                });
                if free_data {
                    self.kitty_store.remove(&id);
                    self.pending_kitty.remove(&id);
                }
                return;
            }
            KittyDeleteMode::Newest => {
                let idx = self
                    .placements
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, p)| {
                        p.screen == screen && image_id.is_none_or(|id| p.kitty_id == Some(id))
                    })
                    .map(|(i, _)| i);
                if let Some(idx) = idx {
                    let removed = self.placements.remove(idx);
                    if let Some(id) = removed.kitty_id {
                        freed_ids.push(id);
                        if free_data {
                            // Free only when no remaining placements share the image.
                            if !self.placements.iter().any(|p| p.kitty_id == Some(id)) {
                                self.kitty_store.remove(&id);
                                self.pending_kitty.remove(&id);
                            }
                        }
                    }
                }
                return;
            }
            KittyDeleteMode::Placement => {
                let Some(pid) = placement_id else {
                    return;
                };
                self.placements.retain(|p| {
                    let image_ok = image_id.is_none_or(|id| p.kitty_id == Some(id));
                    if p.screen == screen && image_ok && p.placement_id == Some(pid) {
                        if let Some(id) = p.kitty_id {
                            freed_ids.push(id);
                        }
                        false
                    } else {
                        true
                    }
                });
            }
            KittyDeleteMode::Cell => {
                self.placements.retain(|p| {
                    if p.screen == screen && placement_intersects_cell(p, cell_line, cell_col) {
                        if let Some(id) = p.kitty_id {
                            freed_ids.push(id);
                        }
                        false
                    } else {
                        true
                    }
                });
            }
            KittyDeleteMode::Column => {
                self.placements.retain(|p| {
                    if placement_intersects_col(p, cell_col) {
                        if let Some(id) = p.kitty_id {
                            freed_ids.push(id);
                        }
                        false
                    } else {
                        true
                    }
                });
            }
            KittyDeleteMode::Row => {
                self.placements.retain(|p| {
                    if p.screen == screen && placement_intersects_line(p, cell_line) {
                        if let Some(id) = p.kitty_id {
                            freed_ids.push(id);
                        }
                        false
                    } else {
                        true
                    }
                });
            }
            KittyDeleteMode::ZIndex => {
                let Some(z) = z else {
                    return;
                };
                self.placements.retain(|p| {
                    if p.screen == screen && p.z_index == z {
                        if let Some(id) = p.kitty_id {
                            freed_ids.push(id);
                        }
                        false
                    } else {
                        true
                    }
                });
            }
        }

        if free_data {
            for id in freed_ids {
                if !self.placements.iter().any(|p| p.kitty_id == Some(id)) {
                    self.kitty_store.remove(&id);
                    self.pending_kitty.remove(&id);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_kitty_image(
        &mut self,
        id: Option<u32>,
        placement_id: Option<u32>,
        width_cells: Option<u16>,
        height_cells: Option<u16>,
        data: Vec<u8>,
        name: Option<String>,
        more: bool,
        place: bool,
        z_index: i32,
        above_text: bool,
        cursor_motion: bool,
        format: Option<u32>,
        compressed: bool,
        pixel_width: Option<u32>,
        pixel_height: Option<u32>,
        quiet: u8,
        cursor_line: i64,
        cursor_col: usize,
        screen: GraphicsScreenKind,
        screen_cols: usize,
        cell_width_px: u16,
        cell_height_px: u16,
    ) -> (Option<GraphicsCursorMotion>, Option<Vec<u8>>) {
        let kitty_key = id.unwrap_or(0);
        // Place-only with no new payload: reuse stored image bytes.
        if place && data.is_empty() && !more {
            let stored = id.and_then(|k| self.get_stored_kitty_image(k));
            let Some(stored) = stored else {
                return (
                    None,
                    kitty_reply_if_needed(quiet, true, id, placement_id, false),
                );
            };
            let motion = self.push_placement(
                GraphicsProtocol::Kitty,
                id,
                placement_id,
                width_cells.or(stored.width_cells),
                height_cells.or(stored.height_cells),
                stored.data.to_vec(),
                name.or(stored.name),
                cursor_line,
                cursor_col,
                screen,
                screen_cols,
                cell_width_px,
                cell_height_px,
                z_index,
                above_text,
            );
            return (
                cursor_motion.then_some(motion),
                kitty_reply_if_needed(quiet, false, id, placement_id, true),
            );
        }

        let entry = self.pending_kitty.entry(kitty_key).or_default();
        if !data.is_empty() {
            if entry.data.len().saturating_add(data.len()) <= MAX_KITTY_PENDING_BYTES {
                entry.data.extend_from_slice(&data);
            } else {
                entry.data.clear();
                entry.overflowed = true;
            }
        }
        if width_cells.is_some() {
            entry.width_cells = width_cells;
        }
        if height_cells.is_some() {
            entry.height_cells = height_cells;
        }
        if name.is_some() {
            entry.name = name;
        }
        entry.place |= place;
        if placement_id.is_some() {
            entry.placement_id = placement_id;
        }
        entry.z_index = z_index;
        entry.above_text |= above_text;
        if format.is_some() {
            entry.format = format;
        }
        entry.compressed |= compressed;
        if pixel_width.is_some() {
            entry.pixel_width = pixel_width;
        }
        if pixel_height.is_some() {
            entry.pixel_height = pixel_height;
        }
        if more {
            return (None, None);
        }
        let finished = self.pending_kitty.remove(&kitty_key).unwrap_or_default();
        if finished.overflowed {
            return (
                None,
                kitty_reply_if_needed(quiet, true, id, placement_id, false),
            );
        }
        let width_cells = finished.width_cells.or(width_cells);
        let height_cells = finished.height_cells.or(height_cells);
        let name = finished.name;
        let place = finished.place || place;
        let placement_id = finished.placement_id.or(placement_id);
        let z_index = if finished.z_index != 0 {
            finished.z_index
        } else {
            z_index
        };
        let above_text = finished.above_text || above_text || z_index > 0;
        let format = finished.format.or(format);
        let compressed = finished.compressed || compressed;
        let pixel_width = finished.pixel_width.or(pixel_width);
        let pixel_height = finished.pixel_height.or(pixel_height);
        let data = crate::kitty_payload::finalize_kitty_payload(
            finished.data,
            format,
            compressed,
            pixel_width,
            pixel_height,
        );

        if let Some(kitty_id) = id
            && !data.is_empty()
        {
            let data = clamp_image_data_arc(data.clone());
            let last_used = self.next_store_generation();
            self.kitty_store.insert(
                kitty_id,
                StoredKittyImage {
                    data,
                    width_cells,
                    height_cells,
                    name: name.clone(),
                    last_used,
                },
            );
        }

        if place {
            let data = if data.is_empty() {
                id.and_then(|k| self.get_stored_kitty_image(k).map(|s| s.data.to_vec()))
                    .unwrap_or_default()
            } else {
                data
            };
            if data.is_empty() && width_cells.is_none() && height_cells.is_none() {
                return (
                    None,
                    kitty_reply_if_needed(quiet, true, id, placement_id, false),
                );
            }
            let motion = self.push_placement(
                GraphicsProtocol::Kitty,
                id,
                placement_id,
                width_cells,
                height_cells,
                data,
                name,
                cursor_line,
                cursor_col,
                screen,
                screen_cols,
                cell_width_px,
                cell_height_px,
                z_index,
                above_text,
            );
            self.prune_kitty_store();
            return (
                cursor_motion.then_some(motion),
                kitty_reply_if_needed(quiet, false, id, placement_id, true),
            );
        }
        self.prune_kitty_store();
        (
            None,
            kitty_reply_if_needed(quiet, false, id, placement_id, true),
        )
    }

    fn kitty_query_ok(
        &self,
        screen: GraphicsScreenKind,
        image_id: Option<u32>,
        placement_id: Option<u32>,
    ) -> bool {
        match (image_id, placement_id) {
            (Some(id), Some(pid)) => self.placements.iter().any(|p| {
                p.screen == screen && p.kitty_id == Some(id) && p.placement_id == Some(pid)
            }),
            (Some(id), None) => {
                self.kitty_store.contains_key(&id)
                    || self
                        .placements
                        .iter()
                        .any(|p| p.screen == screen && p.kitty_id == Some(id))
            }
            (None, Some(pid)) => self
                .placements
                .iter()
                .any(|p| p.screen == screen && p.placement_id == Some(pid)),
            (None, None) => {
                self.placements.iter().any(|p| p.screen == screen) || !self.kitty_store.is_empty()
            }
        }
    }

    fn next_store_generation(&mut self) -> u64 {
        let generation = self.store_generation;
        self.store_generation = self.store_generation.saturating_add(1);
        generation
    }

    fn get_stored_kitty_image(&mut self, id: u32) -> Option<StoredKittyImage> {
        let last_used = self.next_store_generation();
        let stored = self.kitty_store.get_mut(&id)?;
        stored.last_used = last_used;
        Some(stored.clone())
    }

    fn prune_kitty_store(&mut self) {
        while self.kitty_store.len() > MAX_KITTY_STORED_IMAGES
            || self.kitty_store_total_bytes() > MAX_KITTY_STORE_BYTES
        {
            let referenced: HashSet<u32> = self
                .placements
                .iter()
                .filter_map(|placement| placement.kitty_id)
                .collect();
            let victim = self
                .kitty_store
                .iter()
                .filter(|(id, _)| !referenced.contains(id))
                .min_by_key(|(_, image)| image.last_used)
                .map(|(id, _)| *id)
                .or_else(|| {
                    self.kitty_store
                        .iter()
                        .min_by_key(|(_, image)| image.last_used)
                        .map(|(id, _)| *id)
                });
            let Some(victim) = victim else {
                break;
            };
            self.kitty_store.remove(&victim);
        }
    }

    fn kitty_store_total_bytes(&self) -> usize {
        self.kitty_store
            .values()
            .map(|image| image.data.len())
            .sum()
    }

    #[allow(clippy::too_many_arguments)]
    fn push_placement(
        &mut self,
        protocol: GraphicsProtocol,
        kitty_id: Option<u32>,
        placement_id: Option<u32>,
        width_cells: Option<u16>,
        height_cells: Option<u16>,
        data: Vec<u8>,
        name: Option<String>,
        cursor_line: i64,
        cursor_col: usize,
        screen: GraphicsScreenKind,
        screen_cols: usize,
        cell_width_px: u16,
        cell_height_px: u16,
        z_index: i32,
        above_text: bool,
    ) -> GraphicsCursorMotion {
        let width = width_cells
            .map(|v| v as usize)
            .unwrap_or_else(|| estimate_width_cells(&data, screen_cols, cell_width_px))
            .clamp(1, screen_cols.max(1));
        let height = height_cells
            .map(|v| v as usize)
            .unwrap_or_else(|| estimate_height_cells(&data, width, cell_width_px, cell_height_px))
            .clamp(1, 256);
        let internal = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let content_id = image_content_id(&data);
        let data = clamp_image_data_arc(data);
        let col = cursor_col.min(screen_cols.saturating_sub(1));
        self.placements.push(GraphicsPlacement {
            id: internal,
            protocol,
            line: cursor_line,
            screen,
            col,
            width_cells: width,
            height_cells: height,
            data,
            content_id,
            name,
            kitty_id,
            placement_id,
            z_index,
            above_text,
        });
        const MAX_PLACEMENTS: usize = 64;
        if self.placements.len() > MAX_PLACEMENTS {
            let drop_n = self.placements.len() - MAX_PLACEMENTS;
            let _ = self.placements.drain(..drop_n);
        }
        GraphicsCursorMotion {
            start_col: col,
            width_cells: width,
            height_cells: height,
        }
    }

    pub fn shift_lines(&mut self, delta: usize) {
        if delta == 0 {
            return;
        }
        let delta = i64::try_from(delta).unwrap_or(i64::MAX);
        for placement in &mut self.placements {
            placement.line = placement.line.saturating_sub(delta);
        }
    }

    pub fn retain_line_range(&mut self, topmost: i32, bottommost: i32) {
        self.retain_logical_line_range(
            GraphicsScreenKind::Primary,
            i64::from(topmost),
            i64::from(bottommost),
        );
    }

    pub fn retain_logical_line_range(
        &mut self,
        screen: GraphicsScreenKind,
        topmost: i64,
        bottommost: i64,
    ) {
        self.placements.retain(|p| {
            p.screen != screen
                || (p.line.saturating_add(p.height_cells as i64) > topmost && p.line <= bottommost)
        });
    }

    pub fn clear_screen(&mut self, screen: GraphicsScreenKind) {
        self.placements
            .retain(|placement| placement.screen != screen);
    }

    pub fn clear(&mut self) {
        self.placements.clear();
        self.kitty_store.clear();
        self.pending_kitty.clear();
    }

    pub(crate) fn has_placements_for_screen(&self, screen: GraphicsScreenKind) -> bool {
        self.placements
            .iter()
            .any(|placement| placement.screen == screen)
    }

    pub fn viewport_images(
        &self,
        display_offset: usize,
        rows: usize,
        cols: usize,
    ) -> Vec<GraphicsImageSnapshot> {
        self.viewport_images_for_screen(GraphicsScreenKind::Primary, 0, display_offset, rows, cols)
    }

    pub fn viewport_images_for_screen(
        &self,
        screen: GraphicsScreenKind,
        logical_origin: i64,
        display_offset: usize,
        rows: usize,
        cols: usize,
    ) -> Vec<GraphicsImageSnapshot> {
        // Viewport row 0 shows absolute line = -(display_offset) in Alacritty
        // coordinates when display_offset is history lines scrolled up.
        // Our placements store line as the cursor line at insert time (0-based
        // from top of primary screen, negative into scrollback).
        let mut images = Vec::new();
        for placement in &self.placements {
            if placement.screen != screen {
                continue;
            }
            // Map absolute line → viewport row:
            // viewport_row = placement.line + display_offset
            // (when display_offset=0, line 0 is top row; scrolled line -3 at offset 3 is row 0)
            let physical_line = placement.line.saturating_sub(logical_origin);
            let row_i = physical_line.saturating_add(display_offset as i64);
            if row_i >= rows as i64 || row_i + placement.height_cells as i64 <= 0 {
                continue;
            }
            if placement.col >= cols {
                continue;
            }
            let source_row_cells = if row_i < 0 { (-row_i) as usize } else { 0 };
            let row = row_i.max(0) as usize;
            let height = if source_row_cells > 0 {
                placement
                    .height_cells
                    .saturating_sub(source_row_cells)
                    .min(rows.saturating_sub(row))
            } else {
                placement.height_cells.min(rows.saturating_sub(row))
            };
            if height == 0 {
                continue;
            }
            let width = placement
                .width_cells
                .min(cols.saturating_sub(placement.col));
            if width == 0 {
                continue;
            }
            images.push(GraphicsImageSnapshot {
                id: placement.id,
                protocol: placement.protocol,
                row,
                col: placement.col,
                width_cells: width,
                height_cells: height,
                image_width_cells: placement.width_cells,
                image_height_cells: placement.height_cells,
                source_row_cells,
                source_col_cells: 0,
                data: Arc::clone(&placement.data),
                content_id: placement.content_id,
                name: placement.name.clone(),
                above_text: placement.above_text,
            });
        }
        images
    }
}

fn kitty_reply_if_needed(
    quiet: u8,
    is_error: bool,
    image_id: Option<u32>,
    placement_id: Option<u32>,
    ok: bool,
) -> Option<Vec<u8>> {
    // q=0 never; q=1 errors only; q=2 always.
    let send = match quiet {
        0 => false,
        1 => is_error || !ok,
        _ => true,
    };
    if !send {
        return None;
    }
    Some(kitty_graphics_reply(image_id, placement_id, ok))
}

fn kitty_graphics_reply(image_id: Option<u32>, placement_id: Option<u32>, ok: bool) -> Vec<u8> {
    let mut ctrl = String::new();
    if let Some(id) = image_id {
        ctrl.push_str(&format!("i={id}"));
    }
    if let Some(pid) = placement_id {
        if !ctrl.is_empty() {
            ctrl.push(',');
        }
        ctrl.push_str(&format!("p={pid}"));
    }
    let status = if ok { "OK" } else { "ENOENT" };
    let mut out = Vec::with_capacity(ctrl.len() + 16);
    out.extend_from_slice(b"\x1b_G");
    out.extend_from_slice(ctrl.as_bytes());
    out.push(b';');
    out.extend_from_slice(status.as_bytes());
    out.extend_from_slice(b"\x1b\\");
    out
}

fn clamp_image_data_arc(data: Vec<u8>) -> Arc<[u8]> {
    if data.len() > MAX_IMAGE_BYTES {
        Arc::from(&data[..MAX_IMAGE_BYTES])
    } else {
        Arc::from(data)
    }
}

fn image_content_id(data: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut hasher);
    hasher.finish()
}

fn placement_intersects_cell(p: &GraphicsPlacement, line: i64, col: usize) -> bool {
    placement_intersects_line(p, line) && placement_intersects_col(p, col)
}

fn placement_intersects_col(p: &GraphicsPlacement, col: usize) -> bool {
    col >= p.col && col < p.col.saturating_add(p.width_cells)
}

fn placement_intersects_line(p: &GraphicsPlacement, line: i64) -> bool {
    line >= p.line && line < p.line.saturating_add(p.height_cells as i64)
}

fn estimate_width_cells(data: &[u8], screen_cols: usize, cell_width_px: u16) -> usize {
    if let Some((w, _)) = peek_raster_size(data) {
        let cell_w = f32::from(cell_width_px.max(1));
        ((w as f32 / cell_w).ceil() as usize)
            .max(1)
            .min(screen_cols.max(1))
    } else {
        10.min(screen_cols.max(1))
    }
}

fn estimate_height_cells(
    data: &[u8],
    width_cells: usize,
    cell_width_px: u16,
    cell_height_px: u16,
) -> usize {
    if let Some((w, h)) = peek_raster_size(data) {
        if w == 0 {
            return 1;
        }
        // Prefer pixel-accurate height when cell metrics are known.
        let cell_h = f32::from(cell_height_px.max(1));
        let from_px = (h as f32 / cell_h).ceil() as usize;
        let aspect = h as f32 / w as f32;
        let cell_w = f32::from(cell_width_px.max(1));
        let from_aspect = ((width_cells as f32 * cell_w * aspect) / cell_h).ceil() as usize;
        from_px.max(from_aspect).clamp(1, 64)
    } else {
        5
    }
}

fn peek_raster_size(data: &[u8]) -> Option<(u32, u32)> {
    if let Some(dims) = crate::sixel::nyar_dimensions(data) {
        return Some(dims);
    }
    if data.len() >= 24 && data.starts_with(b"\x89PNG\r\n\x1a\n") {
        // IHDR width/height at bytes 16..24
        let w = u32::from_be_bytes(data[16..20].try_into().ok()?);
        let h = u32::from_be_bytes(data[20..24].try_into().ok()?);
        return Some((w, h));
    }
    if data.len() >= 10 && data[0] == 0xff && data[1] == 0xd8 {
        // Scan JPEG SOF0/2
        let mut i = 2usize;
        while i + 9 < data.len() {
            if data[i] != 0xff {
                i += 1;
                continue;
            }
            let marker = data[i + 1];
            if marker == 0xd8 || marker == 0xd9 {
                i += 2;
                continue;
            }
            if i + 4 >= data.len() {
                break;
            }
            let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
            if matches!(marker, 0xc0..=0xc2) && i + 8 < data.len() {
                let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                return Some((w, h));
            }
            i += 2 + len;
        }
    }
    if data.len() >= 10 && data.starts_with(b"GIF8") {
        let w = u16::from_le_bytes([data[6], data[7]]) as u32;
        let h = u16::from_le_bytes([data[8], data[9]]) as u32;
        return Some((w, h));
    }
    None
}

#[cfg(test)]
mod tests;
