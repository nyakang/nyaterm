//! Apply HostBytes ANSI (stock `Display::new_frame` output) into a Framebuffer.
//!
//! Tracks the VT/ECMA-48 state emitted by official Mosh, including cursor and
//! scrolling modes, tab stops, colors, OSC metadata, and split sequences.

use crate::framebuffer::{Attr, Cell, Color, ColorType, Framebuffer, display_cell_width};

#[derive(Debug, Clone)]
struct SavedCursor {
    x: usize,
    y: usize,
    attr: Attr,
    auto_wrap_mode: bool,
    origin_mode: bool,
}

impl Default for SavedCursor {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            attr: Attr::default(),
            auto_wrap_mode: true,
            origin_mode: false,
        }
    }
}

/// Sticky SGR pen + incomplete-sequence carry across HostBytes chunks.
#[derive(Debug, Clone, Default)]
pub struct AnsiPen {
    pub attr: Attr,
    /// Incomplete ESC/CSI/OSC/UTF-8 fragment carried to the next HostBytes chunk.
    pub carry: Vec<u8>,
    saved_cursor: SavedCursor,
}

/// Apply an ANSI/hoststring fragment into `fb` (mutates cells + cursor).
/// Resets pen to default (call [`apply_ansi_with_pen`] to keep sticky SGR).
pub fn apply_ansi(fb: &mut Framebuffer, data: &[u8]) {
    let mut pen = AnsiPen::default();
    apply_ansi_with_pen(fb, &mut pen, data);
}

/// Like [`apply_ansi`] but preserves `pen` (and incomplete carry) across calls.
pub fn apply_ansi_with_pen(fb: &mut Framebuffer, pen: &mut AnsiPen, data: &[u8]) {
    // Reassemble incomplete ESC/UTF-8 from the previous HostBytes chunk.
    let buf: Vec<u8> = if pen.carry.is_empty() {
        data.to_vec()
    } else {
        let mut v = std::mem::take(&mut pen.carry);
        v.extend_from_slice(data);
        v
    };
    let data = &buf[..];
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        if b == 0x1b {
            match consume_escape(fb, pen, data, i) {
                EscapeConsume::Done(next) => i = next,
                EscapeConsume::NeedMore(start) => {
                    pen.carry = data[start..].to_vec();
                    return;
                }
            }
            continue;
        }
        if b == b'\r' {
            fb.cur_x = 0;
            fb.next_print_will_wrap = false;
            fb.retarget_combining_to_cursor();
            i += 1;
            continue;
        }
        if matches!(b, b'\n' | 0x0b | 0x0c) {
            index_down(fb, pen);
            i += 1;
            continue;
        }
        if b == 0x08 {
            // BS
            // Stock keeps the draw cursor one column beyond the right border
            // while autowrap is pending, even though the visible framebuffer
            // reports the last column. A BS first lands that implicit cursor
            // on the border; only an ordinary BS moves one visible cell left.
            if !fb.next_print_will_wrap && fb.cur_x > 0 {
                fb.cur_x -= 1;
            }
            fb.next_print_will_wrap = false;
            fb.retarget_combining_to_cursor();
            i += 1;
            continue;
        }
        if b == b'\t' {
            let wrap = fb.next_print_will_wrap;
            fb.cur_x = fb.next_tab_stop(1);
            fb.next_print_will_wrap = wrap;
            fb.retarget_combining_to_cursor();
            i += 1;
            continue;
        }
        if b == 0x07 {
            fb.bell_count = fb.bell_count.wrapping_add(1);
            i += 1;
            continue;
        }
        if b < 0x20 {
            i += 1;
            continue;
        }
        // UTF-8 printable (may be incomplete at chunk end)
        match decode_utf8_at(data, i) {
            Utf8Decode::NeedMore => {
                pen.carry = data[i..].to_vec();
                return;
            }
            Utf8Decode::Char(ch, len) => {
                i += len;
                if ch == '\0' {
                    continue;
                }
                if fb.try_extend_active_grapheme(ch) {
                    continue;
                }
                let glyph_width = display_cell_width(ch).unwrap_or(0);
                // Stock next_print_will_wrap: wrap *before* placing the next glyph.
                if fb.auto_wrap_mode && fb.next_print_will_wrap {
                    fb.next_print_will_wrap = false;
                    if fb.cur_y == fb.scroll_bottom {
                        let blank = erased_cell(pen);
                        scroll_up_region(fb, fb.scroll_top, fb.scroll_bottom, 1, &blank);
                        fb.cur_y = fb.scroll_bottom;
                    } else {
                        fb.cur_y = (fb.cur_y + 1).min(fb.rows.saturating_sub(1));
                    }
                    fb.cur_x = 0;
                } else if fb.auto_wrap_mode
                    && terminal_standalone_width(ch) == 2
                    && fb.cur_x + 1 >= fb.cols
                {
                    // HostBytes are interpreted by xterm.js, whose Unicode
                    // grapheme provider treats supplementary-plane standalone
                    // emoji as one column but BMP East Asian wide characters
                    // as two. Only the latter wraps before painting at the
                    // right border. The server-side framebuffer width remains
                    // `glyph_width`, which is intentionally a separate value.
                    let x = fb.cur_x;
                    let y = fb.cur_y;
                    if let Some(cell) = fb.cell_at_mut(x, y) {
                        *cell = erased_cell(pen);
                    }
                    if fb.cur_y == fb.scroll_bottom {
                        let blank = erased_cell(pen);
                        scroll_up_region(fb, fb.scroll_top, fb.scroll_bottom, 1, &blank);
                        fb.cur_y = fb.scroll_bottom;
                    } else {
                        fb.cur_y = (fb.cur_y + 1).min(fb.rows.saturating_sub(1));
                    }
                    fb.cur_x = 0;
                }
                let x = fb.cur_x;
                let y = fb.cur_y;
                if x < fb.cols && y < fb.rows {
                    if fb.insert_mode {
                        insert_chars(fb, glyph_width, &erased_cell(pen));
                    }
                    let w = fb.put_rune_with_hyperlink(x, y, ch, pen.attr, fb.active_hyperlink);
                    if w == 0 {
                        // Combining / ZW: do not advance cursor.
                        continue;
                    }
                    // Stock DECAWM: stay on last col and set wrap flag (no immediate wrap).
                    if x + w >= fb.cols {
                        fb.cur_x = fb.cols.saturating_sub(1);
                        fb.next_print_will_wrap = fb.auto_wrap_mode;
                    } else {
                        fb.cur_x = x + w;
                        fb.next_print_will_wrap = false;
                    }
                }
            }
        }
    }
}

fn terminal_standalone_width(ch: char) -> usize {
    display_cell_width(ch).unwrap_or(0)
}

fn scroll_up_region(fb: &mut Framebuffer, top: usize, bottom: usize, lines: usize, blank: &Cell) {
    if top >= fb.rows || bottom >= fb.rows || top > bottom {
        return;
    }
    let height = bottom - top + 1;
    let lines = lines.min(height);
    if lines == 0 {
        return;
    }
    fb.scroll_generation = fb.scroll_generation.wrapping_add(1);
    fb.scroll_rows_up(top, bottom, lines, blank);
}

fn index_down(fb: &mut Framebuffer, pen: &AnsiPen) {
    if fb.cur_y == fb.scroll_bottom {
        let blank = erased_cell(pen);
        scroll_up_region(fb, fb.scroll_top, fb.scroll_bottom, 1, &blank);
        fb.cur_y = fb.scroll_bottom;
    } else {
        fb.cur_y = (fb.cur_y + 1).min(fb.rows.saturating_sub(1));
    }
    fb.next_print_will_wrap = false;
    fb.retarget_combining_to_cursor();
}

fn erased_cell(pen: &AnsiPen) -> Cell {
    Cell::erased(pen.attr.bg)
}

enum Utf8Decode {
    Char(char, usize),
    NeedMore,
}

enum EscapeConsume {
    Done(usize),
    NeedMore(usize),
}

fn decode_utf8_at(data: &[u8], i: usize) -> Utf8Decode {
    let b0 = data[i];
    if b0 < 0x80 {
        return Utf8Decode::Char(b0 as char, 1);
    }
    let width = if b0 & 0xE0 == 0xC0 {
        2
    } else if b0 & 0xF0 == 0xE0 {
        3
    } else if b0 & 0xF8 == 0xF0 {
        4
    } else {
        return Utf8Decode::Char('\u{FFFD}', 1);
    };
    if i + width > data.len() {
        return Utf8Decode::NeedMore;
    }
    match std::str::from_utf8(&data[i..i + width]) {
        Ok(s) => Utf8Decode::Char(s.chars().next().unwrap_or('\u{FFFD}'), width),
        Err(_) => Utf8Decode::Char('\u{FFFD}', 1),
    }
}

fn consume_escape(
    fb: &mut Framebuffer,
    pen: &mut AnsiPen,
    data: &[u8],
    start: usize,
) -> EscapeConsume {
    let mut i = start + 1;
    if i >= data.len() {
        return EscapeConsume::NeedMore(start);
    }
    match data[i] {
        b'[' => {
            i += 1;
            // ECMA-48 CSI grammar: parameter bytes 0x30-0x3f, optional
            // intermediate bytes 0x20-0x2f, then one final byte 0x40-0x7e.
            // Unsupported commands must still be consumed as one sequence;
            // otherwise tails such as `0c` from secondary device attributes
            // are accidentally painted as terminal text.
            let body_start = i;
            while i < data.len() {
                let c = data[i];
                if (0x20..=0x3f).contains(&c) || c < 0x20 {
                    i += 1;
                    continue;
                }
                if (0x40..=0x7e).contains(&c) {
                    let body = &data[body_start..i];
                    let param_end = body
                        .iter()
                        .position(|byte| (0x20..=0x2f).contains(byte))
                        .unwrap_or(body.len());
                    apply_csi(fb, pen, &body[..param_end], c);
                    return EscapeConsume::Done(i + 1);
                }
                // Invalid CSI byte: consume it with the malformed sequence so
                // parser recovery never leaks protocol bytes onto the screen.
                return EscapeConsume::Done(i + 1);
            }
            EscapeConsume::NeedMore(start)
        }
        b']' => {
            // OSC ... BEL or ST
            i += 1;
            while i < data.len() {
                if data[i] == 0x07 {
                    apply_osc(fb, &data[start + 2..i]);
                    return EscapeConsume::Done(i + 1);
                }
                if data[i] == 0x1b && i + 1 < data.len() && data[i + 1] == b'\\' {
                    apply_osc(fb, &data[start + 2..i]);
                    return EscapeConsume::Done(i + 2);
                }
                i += 1;
            }
            EscapeConsume::NeedMore(start)
        }
        b'M' => {
            // RI (reverse index): move up, scrolling the active region down
            // when the cursor is already on its top margin.
            if fb.cur_y == fb.scroll_top {
                let blank = erased_cell(pen);
                fb.insert_blank_rows(fb.scroll_top, fb.scroll_bottom, 1, &blank);
                fb.scroll_generation = fb.scroll_generation.wrapping_add(1);
            } else {
                fb.cur_y = fb.cur_y.saturating_sub(1);
            }
            fb.next_print_will_wrap = false;
            fb.retarget_combining_to_cursor();
            EscapeConsume::Done(i + 1)
        }
        b'D' => {
            // IND
            index_down(fb, pen);
            EscapeConsume::Done(i + 1)
        }
        b'E' => {
            // NEL
            fb.cur_x = 0;
            index_down(fb, pen);
            EscapeConsume::Done(i + 1)
        }
        b'H' => {
            // HTS
            fb.set_tab_stop(fb.cur_x);
            EscapeConsume::Done(i + 1)
        }
        b'7' => {
            pen.saved_cursor = SavedCursor {
                x: fb.cur_x,
                y: fb.cur_y,
                attr: pen.attr,
                auto_wrap_mode: fb.auto_wrap_mode,
                origin_mode: fb.origin_mode,
            };
            EscapeConsume::Done(i + 1)
        }
        b'8' => {
            fb.cur_x = pen.saved_cursor.x.min(fb.cols.saturating_sub(1));
            fb.cur_y = pen.saved_cursor.y.min(fb.rows.saturating_sub(1));
            pen.attr = pen.saved_cursor.attr;
            fb.auto_wrap_mode = pen.saved_cursor.auto_wrap_mode;
            fb.origin_mode = pen.saved_cursor.origin_mode;
            if fb.origin_mode {
                fb.cur_y = fb.cur_y.clamp(fb.scroll_top, fb.scroll_bottom);
            }
            fb.next_print_will_wrap = false;
            fb.retarget_combining_to_cursor();
            EscapeConsume::Done(i + 1)
        }
        _ => {
            // Other ESC X — need the following byte if missing
            if i >= data.len() {
                EscapeConsume::NeedMore(start)
            } else {
                EscapeConsume::Done(i + 1)
            }
        }
    }
}

fn parse_params(params: &[u8]) -> Vec<u32> {
    if params.is_empty() {
        return Vec::new();
    }
    // Strip leading '?' for private modes
    let raw = if params.first() == Some(&b'?') {
        &params[1..]
    } else {
        params
    };
    if raw.is_empty() {
        return Vec::new();
    }
    raw.split(|&b| b == b';')
        .map(|p| {
            if p.is_empty() {
                0
            } else {
                std::str::from_utf8(p)
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0)
            }
        })
        .collect()
}

fn apply_csi(fb: &mut Framebuffer, pen: &mut AnsiPen, params: &[u8], final_byte: u8) {
    let private = params.first() == Some(&b'?');
    let nums = parse_params(params);

    match final_byte {
        b'H' | b'f' => {
            // CUP — 1-indexed
            let row = nums.first().copied().unwrap_or(1).max(1) as usize;
            let col = nums.get(1).copied().unwrap_or(1).max(1) as usize;
            let top = if fb.origin_mode { fb.scroll_top } else { 0 };
            let bottom = if fb.origin_mode {
                fb.scroll_bottom
            } else {
                fb.rows.saturating_sub(1)
            };
            fb.cur_y = top.saturating_add(row - 1).min(bottom);
            fb.cur_x = (col - 1).min(fb.cols.saturating_sub(1));
            fb.next_print_will_wrap = false;
            fb.retarget_combining_to_cursor();
        }
        b'A' => {
            let n = nums.first().copied().unwrap_or(1).max(1) as usize;
            let top = if fb.origin_mode { fb.scroll_top } else { 0 };
            fb.cur_y = fb.cur_y.saturating_sub(n).max(top);
            fb.next_print_will_wrap = false;
            fb.retarget_combining_to_cursor();
        }
        b'B' => {
            let n = nums.first().copied().unwrap_or(1).max(1) as usize;
            let bottom = if fb.origin_mode {
                fb.scroll_bottom
            } else {
                fb.rows.saturating_sub(1)
            };
            fb.cur_y = fb.cur_y.saturating_add(n).min(bottom);
            fb.next_print_will_wrap = false;
            fb.retarget_combining_to_cursor();
        }
        b'C' => {
            let n = nums.first().copied().unwrap_or(1).max(1) as usize;
            fb.cur_x = (fb.cur_x + n).min(fb.cols.saturating_sub(1));
            fb.next_print_will_wrap = false;
            fb.retarget_combining_to_cursor();
        }
        b'D' => {
            let n = nums.first().copied().unwrap_or(1).max(1) as usize;
            fb.cur_x = fb.cur_x.saturating_sub(n);
            fb.next_print_will_wrap = false;
            fb.retarget_combining_to_cursor();
        }
        b'G' => {
            // CHA — column absolute 1-indexed
            let col = nums.first().copied().unwrap_or(1).max(1) as usize;
            fb.cur_x = (col - 1).min(fb.cols.saturating_sub(1));
            fb.next_print_will_wrap = false;
            fb.retarget_combining_to_cursor();
        }
        b'I' => {
            // CHT
            let n = nums.first().copied().unwrap_or(1).max(1) as usize;
            fb.cur_x = fb.next_tab_stop(n);
            fb.next_print_will_wrap = false;
            fb.retarget_combining_to_cursor();
        }
        b'Z' => {
            // CBT
            let n = nums.first().copied().unwrap_or(1).max(1) as usize;
            fb.cur_x = fb.previous_tab_stop(n);
            fb.next_print_will_wrap = false;
            fb.retarget_combining_to_cursor();
        }
        b'g' => {
            // TBC
            match nums.first().copied().unwrap_or(0) {
                0 => fb.clear_tab_stop(fb.cur_x),
                3 => fb.clear_all_tab_stops(),
                _ => {}
            }
        }
        b'd' => {
            // VPA
            let row = nums.first().copied().unwrap_or(1).max(1) as usize;
            let top = if fb.origin_mode { fb.scroll_top } else { 0 };
            let bottom = if fb.origin_mode {
                fb.scroll_bottom
            } else {
                fb.rows.saturating_sub(1)
            };
            fb.cur_y = top.saturating_add(row - 1).min(bottom);
            fb.next_print_will_wrap = false;
            fb.retarget_combining_to_cursor();
        }
        b'J' => {
            // ED
            let mode = nums.first().copied().unwrap_or(0);
            match mode {
                2 | 3 => {
                    let blank = erased_cell(pen);
                    fb.fill_all(&blank);
                    fb.scroll_generation = fb.scroll_generation.wrapping_add(1);
                    // stock often homes separately
                }
                0 => {
                    // erase from cursor to end of screen
                    erase_from_cursor(fb, &erased_cell(pen));
                }
                1 => {
                    erase_to_cursor(fb, &erased_cell(pen));
                }
                _ => {}
            }
        }
        b'K' => {
            // EL
            let mode = nums.first().copied().unwrap_or(0);
            let y = fb.cur_y;
            let blank = erased_cell(pen);
            match mode {
                0 => {
                    fb.erase_row_range(y, fb.cur_x, fb.cols, &blank);
                }
                1 => {
                    fb.erase_row_range(y, 0, fb.cur_x.saturating_add(1), &blank);
                }
                2 => {
                    fb.erase_row_range(y, 0, fb.cols, &blank);
                }
                _ => {}
            }
        }
        b'X' => {
            // ECH
            let n = nums.first().copied().unwrap_or(1).max(1) as usize;
            let y = fb.cur_y;
            let blank = erased_cell(pen);
            fb.erase_row_range(y, fb.cur_x, fb.cur_x.saturating_add(n), &blank);
        }
        b'@' => {
            // ICH — insert n blank cells at cursor
            let n = nums.first().copied().unwrap_or(1).max(1) as usize;
            insert_chars(fb, n, &erased_cell(pen));
        }
        b'P' => {
            // DCH — delete n cells at cursor
            let n = nums.first().copied().unwrap_or(1).max(1) as usize;
            delete_chars(fb, n, &erased_cell(pen));
        }
        b'L' => {
            // IL — insert n blank lines
            let n = nums.first().copied().unwrap_or(1).max(1) as usize;
            insert_lines(fb, n, &erased_cell(pen));
            fb.cur_x = 0;
            fb.next_print_will_wrap = false;
            fb.retarget_combining_to_cursor();
        }
        b'M' => {
            // DL — delete n lines
            let n = nums.first().copied().unwrap_or(1).max(1) as usize;
            delete_lines(fb, n, &erased_cell(pen));
            fb.cur_x = 0;
            fb.next_print_will_wrap = false;
            fb.retarget_combining_to_cursor();
        }
        b'm' => apply_sgr(pen, &nums),
        b'r' if !private => {
            let top = nums.first().copied().unwrap_or(1).max(1) as usize - 1;
            let bottom = nums.get(1).copied().unwrap_or(fb.rows as u32).max(1) as usize - 1;
            if top < bottom && bottom < fb.rows {
                fb.scroll_top = top;
                fb.scroll_bottom = bottom;
                fb.cur_x = 0;
                fb.cur_y = if fb.origin_mode { fb.scroll_top } else { 0 };
                fb.next_print_will_wrap = false;
                fb.retarget_combining_to_cursor();
            } else if nums.is_empty() {
                fb.scroll_top = 0;
                fb.scroll_bottom = fb.rows.saturating_sub(1);
                fb.cur_x = 0;
                fb.cur_y = if fb.origin_mode { fb.scroll_top } else { 0 };
                fb.next_print_will_wrap = false;
                fb.retarget_combining_to_cursor();
            }
        }
        b'h' if private => {
            for mode in nums {
                set_private_mode(fb, mode, true);
            }
        }
        b'l' if private => {
            for mode in nums {
                set_private_mode(fb, mode, false);
            }
        }
        b'h' => {
            if nums.contains(&4) {
                fb.insert_mode = true;
            }
        }
        b'l' if nums.contains(&4) => {
            fb.insert_mode = false;
        }
        _ => {}
    }
}

fn insert_chars(fb: &mut Framebuffer, n: usize, blank: &Cell) {
    let y = fb.cur_y;
    let x = fb.cur_x;
    if y >= fb.rows || x >= fb.cols || n == 0 {
        return;
    }
    let n = n.min(fb.cols - x);
    // Shift right from end
    for col in (x..fb.cols - n).rev() {
        if let (Some(src), Some(dst)) = (fb.cell_at(col, y).cloned(), fb.cell_at_mut(col + n, y)) {
            *dst = src;
        }
    }
    for col in x..x + n {
        if let Some(c) = fb.cell_at_mut(col, y) {
            *c = blank.clone();
        }
    }
}

fn delete_chars(fb: &mut Framebuffer, n: usize, blank: &Cell) {
    let y = fb.cur_y;
    let x = fb.cur_x;
    if y >= fb.rows || x >= fb.cols || n == 0 {
        return;
    }
    let n = n.min(fb.cols - x);
    for col in x..fb.cols - n {
        if let (Some(src), Some(dst)) = (fb.cell_at(col + n, y).cloned(), fb.cell_at_mut(col, y)) {
            *dst = src;
        }
    }
    for col in (fb.cols - n)..fb.cols {
        if let Some(c) = fb.cell_at_mut(col, y) {
            *c = blank.clone();
        }
    }
}

fn insert_lines(fb: &mut Framebuffer, n: usize, blank: &Cell) {
    let y = fb.cur_y;
    if y < fb.scroll_top || y > fb.scroll_bottom || n == 0 {
        return;
    }
    let n = n.min(fb.scroll_bottom - y + 1);
    fb.insert_blank_rows(y, fb.scroll_bottom, n, blank);
    fb.scroll_generation = fb.scroll_generation.wrapping_add(1);
}

fn delete_lines(fb: &mut Framebuffer, n: usize, blank: &Cell) {
    let y = fb.cur_y;
    if y < fb.scroll_top || y > fb.scroll_bottom || n == 0 {
        return;
    }
    let n = n.min(fb.scroll_bottom - y + 1);
    fb.delete_rows(y, fb.scroll_bottom, n, blank);
    fb.scroll_generation = fb.scroll_generation.wrapping_add(1);
}

fn set_private_mode(fb: &mut Framebuffer, mode: u32, enabled: bool) {
    match mode {
        5 => fb.reverse_video = enabled,
        6 => {
            fb.origin_mode = enabled;
            fb.cur_x = 0;
            fb.cur_y = if enabled { fb.scroll_top } else { 0 };
            fb.next_print_will_wrap = false;
            fb.retarget_combining_to_cursor();
        }
        7 => fb.auto_wrap_mode = enabled,
        25 => fb.cursor_visible = enabled,
        2004 => fb.bracketed_paste = enabled,
        9 | 1000..=1003 => {
            if enabled {
                fb.mouse_reporting_mode = mode as u16;
            } else if fb.mouse_reporting_mode == mode as u16 {
                fb.mouse_reporting_mode = 0;
            }
        }
        1004 => fb.mouse_focus_event = enabled,
        1005 | 1006 | 1015 => {
            if enabled {
                fb.mouse_encoding_mode = mode as u16;
            } else if fb.mouse_encoding_mode == mode as u16 {
                fb.mouse_encoding_mode = 0;
            }
        }
        _ => {}
    }
}

fn apply_osc(fb: &mut Framebuffer, payload: &[u8]) {
    if let Some(value) = payload.strip_prefix(b"0;") {
        fb.icon_name = Some(value.to_vec());
        fb.window_title = Some(value.to_vec());
        return;
    }
    if let Some(value) = payload.strip_prefix(b"1;") {
        fb.icon_name = Some(value.to_vec());
        return;
    }
    if let Some(value) = payload.strip_prefix(b"2;") {
        fb.window_title = Some(value.to_vec());
        return;
    }
    if let Some(value) = payload.strip_prefix(b"52;c;") {
        fb.clipboard = Some(value.to_vec());
        return;
    }
    if let Some(value) = payload.strip_prefix(b"8;") {
        let mut fields = value.splitn(2, |byte| *byte == b';');
        let params = fields.next().unwrap_or_default();
        let uri = fields.next().unwrap_or_default();
        fb.set_active_hyperlink(params, uri);
    }
}

fn erase_from_cursor(fb: &mut Framebuffer, blank: &Cell) {
    let (cx, cy) = (fb.cur_x, fb.cur_y);
    for y in cy..fb.rows {
        let start = if y == cy { cx } else { 0 };
        fb.erase_row_range(y, start, fb.cols, blank);
    }
}

fn erase_to_cursor(fb: &mut Framebuffer, blank: &Cell) {
    let (cx, cy) = (fb.cur_x, fb.cur_y);
    for y in 0..=cy {
        let end = if y == cy {
            cx.min(fb.cols.saturating_sub(1))
        } else {
            fb.cols.saturating_sub(1)
        };
        fb.erase_row_range(y, 0, end.saturating_add(1), blank);
    }
}

fn apply_sgr(pen: &mut AnsiPen, nums: &[u32]) {
    if nums.is_empty() {
        pen.attr = Attr::default();
        return;
    }
    let mut i = 0;
    while i < nums.len() {
        match nums[i] {
            0 => pen.attr = Attr::default(),
            1 => pen.attr.bold = true,
            2 => pen.attr.dim = true,
            3 => pen.attr.italic = true,
            4 => pen.attr.under = true,
            5 | 6 => pen.attr.blink = true,
            7 => pen.attr.reverse = true,
            8 => pen.attr.conceal = true,
            9 => pen.attr.strike = true,
            22 => {
                pen.attr.bold = false;
                pen.attr.dim = false;
            }
            23 => pen.attr.italic = false,
            24 => pen.attr.under = false,
            25 => pen.attr.blink = false,
            27 => pen.attr.reverse = false,
            28 => pen.attr.conceal = false,
            29 => pen.attr.strike = false,
            39 => pen.attr.fg = Color::default_color(),
            49 => pen.attr.bg = Color::default_color(),
            n @ 30..=37 => {
                pen.attr.fg = Color::index(n - 30);
            }
            n @ 40..=47 => {
                pen.attr.bg = Color::index(n - 40);
            }
            n @ 90..=97 => {
                pen.attr.fg = Color::index(n - 90 + 8);
            }
            n @ 100..=107 => {
                pen.attr.bg = Color::index(n - 100 + 8);
            }
            38 => {
                // 38;5;n or 38;2;r;g;b
                if i + 1 < nums.len() && nums[i + 1] == 5 && i + 2 < nums.len() {
                    pen.attr.fg = Color::index(nums[i + 2]);
                    i += 2;
                } else if i + 1 < nums.len() && nums[i + 1] == 2 && i + 4 < nums.len() {
                    pen.attr.fg =
                        Color::rgb(nums[i + 2] as u8, nums[i + 3] as u8, nums[i + 4] as u8);
                    i += 4;
                }
            }
            48 => {
                if i + 1 < nums.len() && nums[i + 1] == 5 && i + 2 < nums.len() {
                    pen.attr.bg = Color::index(nums[i + 2]);
                    i += 2;
                } else if i + 1 < nums.len() && nums[i + 1] == 2 && i + 4 < nums.len() {
                    pen.attr.bg =
                        Color::rgb(nums[i + 2] as u8, nums[i + 3] as u8, nums[i + 4] as u8);
                    i += 4;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let _ = ColorType::Default; // keep import used for docs/readability
}

#[cfg(test)]
mod tests {
    use super::{
        AnsiPen, Color, Framebuffer, apply_ansi, apply_ansi_with_pen, terminal_standalone_width,
    };

    #[test]
    fn cup_and_print() {
        let mut fb = Framebuffer::new(80, 24);
        apply_ansi(&mut fb, b"\x1b[1;1Hhi");
        assert_eq!(fb.cell_at(0, 0).unwrap().ch, 'h');
        assert_eq!(fb.cell_at(1, 0).unwrap().ch, 'i');
        assert_eq!(fb.cur_x, 2);
        assert_eq!(fb.cur_y, 0);
    }

    #[test]
    fn clear_and_prompt() {
        let mut fb = Framebuffer::new(80, 24);
        apply_ansi(&mut fb, b"\x1b[H\x1b[2J$ ");
        assert_eq!(fb.cell_at(0, 0).unwrap().ch, '$');
        assert_eq!(fb.cell_at(1, 0).unwrap().ch, ' ');
    }

    #[test]
    fn unsupported_csi_prefix_is_consumed_without_painting_its_tail() {
        let mut fb = Framebuffer::new(80, 24);
        apply_ansi(&mut fb, b"A\x1b[>0cB\x1b[!pC");

        assert_eq!(fb.cell_at(0, 0).unwrap().ch, 'A');
        assert_eq!(fb.cell_at(1, 0).unwrap().ch, 'B');
        assert_eq!(fb.cell_at(2, 0).unwrap().ch, 'C');
        assert_eq!(fb.cur_x, 3);
    }

    #[test]
    fn horizontal_tab_uses_stock_eight_column_stops() {
        let mut fb = Framebuffer::new(20, 2);
        apply_ansi(&mut fb, b"A\tB");

        assert_eq!(fb.cell_at(0, 0).unwrap().ch, 'A');
        assert_eq!(fb.cell_at(8, 0).unwrap().ch, 'B');
        assert_eq!(fb.cur_x, 9);
    }

    #[test]
    fn custom_tab_stops_and_tab_clear_match_stock_terminal_state() {
        let mut fb = Framebuffer::new(20, 2);
        apply_ansi(&mut fb, b"abc\x1bH\r\tX");
        assert_eq!(fb.cell_at(3, 0).unwrap().ch, 'X');

        apply_ansi(&mut fb, b"\x1b[3g\r\tY");
        assert_eq!(fb.cell_at(19, 0).unwrap().ch, 'Y');
    }

    #[test]
    fn forward_and_backward_tab_commands_use_active_stops() {
        let mut fb = Framebuffer::new(24, 2);
        apply_ansi(&mut fb, b"\x1b[2IY\x1b[3ZX");

        assert_eq!(fb.cell_at(16, 0).unwrap().ch, 'Y');
        assert_eq!(fb.cell_at(0, 0).unwrap().ch, 'X');
    }

    #[test]
    fn index_and_next_line_apply_stock_scroll_margin_behavior() {
        let mut fb = Framebuffer::new(4, 3);
        apply_ansi(&mut fb, b"A\x1b[2;1HB\x1b[3;1HC\x1bD");
        assert_eq!(fb.cell_at(0, 0).unwrap().ch, 'B');
        assert_eq!(fb.cell_at(0, 1).unwrap().ch, 'C');

        apply_ansi(&mut fb, b"\x1b[2;3H\x1bEX");
        assert_eq!(fb.cell_at(0, 2).unwrap().ch, 'X');
    }

    #[test]
    fn setting_scroll_margins_homes_the_cursor_like_stock() {
        let mut fb = Framebuffer::new(20, 12);
        apply_ansi(&mut fb, b"\x1b[5;5H\x1b[2;10rX");

        assert_eq!(fb.scroll_top, 1);
        assert_eq!(fb.scroll_bottom, 9);
        assert_eq!(fb.cell_at(0, 0).unwrap().ch, 'X');
        assert_eq!((fb.cur_x, fb.cur_y), (1, 0));
    }

    #[test]
    fn reverse_index_scrolls_down_inside_the_active_margins() {
        let mut fb = Framebuffer::new(4, 3);
        apply_ansi(
            &mut fb,
            b"\x1b[1;1HA\x1b[2;1HB\x1b[3;1HC\x1b[2;3r\x1b[2;1H\x1bM",
        );

        assert_eq!(fb.cell_at(0, 0).unwrap().ch, 'A');
        assert_eq!(fb.cell_at(0, 1).unwrap().ch, ' ');
        assert_eq!(fb.cell_at(0, 2).unwrap().ch, 'B');
        assert_eq!((fb.cur_x, fb.cur_y), (0, 1));
    }

    #[test]
    fn dec_save_restore_recovers_cursor_and_renditions() {
        let mut fb = Framebuffer::new(10, 4);
        apply_ansi(&mut fb, b"\x1b[3;4H\x1b[31m\x1b7\x1b[1;1H\x1b[mX\x1b8Y");

        assert_eq!(fb.cell_at(0, 0).unwrap().ch, 'X');
        let restored = fb.cell_at(3, 2).unwrap();
        assert_eq!(restored.ch, 'Y');
        assert_eq!(restored.attr.fg, Color::index(1));
        assert_eq!((fb.cur_x, fb.cur_y), (4, 2));
    }

    #[test]
    fn ansi_insert_mode_shifts_existing_cells_before_printing() {
        let mut fb = Framebuffer::new(6, 2);
        apply_ansi(&mut fb, b"abc\r\x1b[4hX");

        assert_eq!(fb.cell_at(0, 0).unwrap().ch, 'X');
        assert_eq!(fb.cell_at(1, 0).unwrap().ch, 'a');
        assert_eq!(fb.cell_at(2, 0).unwrap().ch, 'b');
        assert_eq!(fb.cell_at(3, 0).unwrap().ch, 'c');
    }

    #[test]
    fn disabling_auto_wrap_overwrites_the_last_column() {
        let mut fb = Framebuffer::new(3, 2);
        apply_ansi(&mut fb, b"abc\x1b[?7lD");

        assert_eq!(fb.cell_at(0, 0).unwrap().ch, 'a');
        assert_eq!(fb.cell_at(1, 0).unwrap().ch, 'b');
        assert_eq!(fb.cell_at(2, 0).unwrap().ch, 'D');
        assert_eq!(fb.cell_at(0, 1).unwrap().ch, ' ');
        assert_eq!((fb.cur_x, fb.cur_y), (2, 0));
        assert!(!fb.next_print_will_wrap);
    }

    #[test]
    fn native_wide_glyph_wraps_before_right_margin() {
        for glyph in ["💻", "界", "☰"] {
            let mut fb = Framebuffer::new(4, 2);
            apply_ansi(&mut fb, format!("abc{glyph}").as_bytes());
            assert_eq!(fb.cell_at(3, 0).unwrap().ch, ' ');
            assert_eq!(fb.cell_at(0, 1).unwrap().ch.to_string(), glyph);
            assert_eq!(fb.cell_at(0, 1).unwrap().width, 2);
            assert_eq!((fb.cur_x, fb.cur_y), (2, 1));
        }
    }

    #[test]
    fn native_terminal_width_includes_supplementary_cjk_and_emoji() {
        for (ch, expected) in [
            ('a', 1),
            ('\u{10000}', 1),
            ('\u{20000}', 2),
            ('💻', 2),
            ('界', 2),
            ('☰', 2),
        ] {
            assert_eq!(terminal_standalone_width(ch), expected);
        }
    }

    #[test]
    fn insert_and_delete_line_move_to_column_zero_like_stock() {
        let mut inserted = Framebuffer::new(8, 4);
        apply_ansi(&mut inserted, b"\x1b[2;4H\x1b[LX");
        assert_eq!(inserted.cell_at(0, 1).unwrap().ch, 'X');

        let mut deleted = Framebuffer::new(8, 4);
        apply_ansi(&mut deleted, b"\x1b[2;4H\x1b[MY");
        assert_eq!(deleted.cell_at(0, 1).unwrap().ch, 'Y');
    }

    #[test]
    fn origin_mode_makes_absolute_rows_relative_to_scroll_margins() {
        let mut fb = Framebuffer::new(10, 6);
        apply_ansi(&mut fb, b"\x1b[2;5r\x1b[?6h\x1b[1;1HX");

        assert_eq!(fb.cell_at(0, 1).unwrap().ch, 'X');
        assert_eq!((fb.cur_x, fb.cur_y), (1, 1));
    }

    #[test]
    fn enabling_origin_mode_homes_directly_to_the_top_margin() {
        let mut fb = Framebuffer::new(10, 6);
        apply_ansi(&mut fb, b"\x1b[2;5r\x1b[?6hX");

        assert_eq!(fb.cell_at(0, 1).unwrap().ch, 'X');
        assert_eq!((fb.cur_x, fb.cur_y), (1, 1));
    }

    #[test]
    fn underline_sgr() {
        let mut fb = Framebuffer::new(80, 24);
        apply_ansi(&mut fb, b"\x1b[4ma\x1b[24m");
        assert!(fb.cell_at(0, 0).unwrap().attr.under);
    }

    #[test]
    fn ich_inserts_blanks() {
        let mut fb = Framebuffer::new(10, 3);
        apply_ansi(&mut fb, b"\x1b[Habc");
        assert_eq!(fb.cell_at(0, 0).unwrap().ch, 'a');
        apply_ansi(&mut fb, b"\x1b[1;1H\x1b[2@");
        assert_eq!(fb.cell_at(0, 0).unwrap().ch, ' ');
        assert_eq!(fb.cell_at(1, 0).unwrap().ch, ' ');
        assert_eq!(fb.cell_at(2, 0).unwrap().ch, 'a');
        assert_eq!(fb.cell_at(3, 0).unwrap().ch, 'b');
        assert_eq!(fb.cell_at(4, 0).unwrap().ch, 'c');
    }

    #[test]
    fn dch_deletes_chars() {
        let mut fb = Framebuffer::new(10, 3);
        apply_ansi(&mut fb, b"\x1b[Habcde");
        apply_ansi(&mut fb, b"\x1b[1;2H\x1b[2P"); // delete 2 at col 1
        assert_eq!(fb.cell_at(0, 0).unwrap().ch, 'a');
        assert_eq!(fb.cell_at(1, 0).unwrap().ch, 'd');
        assert_eq!(fb.cell_at(2, 0).unwrap().ch, 'e');
    }

    #[test]
    fn split_csi_cup_reassembled_across_chunks() {
        let mut fb = Framebuffer::new(80, 24);
        let mut pen = AnsiPen::default();
        apply_ansi_with_pen(&mut fb, &mut pen, b"\x1b[1;");
        assert!(!pen.carry.is_empty(), "incomplete CSI must be carried");
        apply_ansi_with_pen(&mut fb, &mut pen, b"5H");
        assert!(pen.carry.is_empty());
        assert_eq!(fb.cur_y, 0);
        assert_eq!(fb.cur_x, 4, "CUP 1;5 → col 4");
    }

    #[test]
    fn split_utf8_reassembled_across_chunks() {
        let mut fb = Framebuffer::new(10, 3);
        let mut pen = AnsiPen::default();
        // '€' = E2 82 AC
        apply_ansi_with_pen(&mut fb, &mut pen, &[0xE2, 0x82]);
        assert!(!pen.carry.is_empty());
        apply_ansi_with_pen(&mut fb, &mut pen, &[0xAC]);
        assert!(pen.carry.is_empty());
        assert_eq!(fb.cell_at(0, 0).unwrap().ch, '€');
    }

    #[test]
    fn printable_defers_wrap_until_next_char() {
        let mut fb = Framebuffer::new(4, 3);
        apply_ansi(&mut fb, b"\x1b[Habcd"); // fills row 0 cols 0..3
        // Stock: after last col, stay at col 3 with wrap flag set.
        assert_eq!(fb.cur_x, 3);
        assert_eq!(fb.cur_y, 0);
        assert!(fb.next_print_will_wrap);
        assert_eq!(fb.cell_at(3, 0).unwrap().ch, 'd');
        apply_ansi(&mut fb, b"X");
        assert_eq!(fb.cur_y, 1);
        assert_eq!(fb.cur_x, 1);
        assert_eq!(fb.cell_at(0, 1).unwrap().ch, 'X');
        assert!(!fb.next_print_will_wrap);
    }

    #[test]
    fn cup_clears_wrap_flag() {
        let mut fb = Framebuffer::new(4, 3);
        apply_ansi(&mut fb, b"\x1b[Habcd");
        assert!(fb.next_print_will_wrap);
        apply_ansi(&mut fb, b"\x1b[1;1H");
        assert!(!fb.next_print_will_wrap);
        assert_eq!(fb.cur_x, 0);
        assert_eq!(fb.cur_y, 0);
    }

    #[test]
    fn erase_and_scroll_preserve_the_active_background() {
        let mut fb = Framebuffer::new(5, 2);
        let mut pen = AnsiPen::default();
        apply_ansi_with_pen(
            &mut fb,
            &mut pen,
            b"\x1b[31;48;5;4;4;7m\x1b]8;;https://example.test\x1b\\ABCDE\x1b[1;2H\x1b[3X",
        );
        for col in 1..4 {
            let cell = fb.cell_at(col, 0).unwrap();
            assert_eq!(cell.ch, ' ');
            assert_eq!(cell.attr.bg, Color::index(4));
            assert_eq!(cell.attr.fg, Color::default_color());
            assert!(!cell.attr.under);
            assert!(!cell.attr.reverse);
            assert_eq!(cell.hyperlink, 0);
        }

        apply_ansi_with_pen(&mut fb, &mut pen, b"\x1b[2;1H\n");
        for col in 0..fb.cols {
            let cell = fb.cell_at(col, 1).unwrap();
            assert_eq!(cell.attr.bg, Color::index(4));
            assert_eq!(cell.attr.fg, Color::default_color());
            assert!(!cell.attr.under);
            assert!(!cell.attr.reverse);
            assert_eq!(cell.hyperlink, 0);
        }
    }

    #[test]
    fn x10_mouse_reporting_mode_is_reconstructed() {
        let mut fb = Framebuffer::new(5, 2);
        apply_ansi(&mut fb, b"\x1b[?9h");
        assert_eq!(fb.mouse_reporting_mode, 9);
        apply_ansi(&mut fb, b"\x1b[?9l");
        assert_eq!(fb.mouse_reporting_mode, 0);
    }
}
