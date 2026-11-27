//! Client cell grid + Diff, shaped after mosh-go `framebuffer.go`.
//!
//! Stock mosh and mosh-go keep a local Framebuffer so predictions are an
//! *overlay on cells*, and a single `Diff(old, new)` stream is written to
//! the PTY — never dual-write predicted glyphs beside raw HostBytes.

use std::cmp::Ordering;
use std::sync::Arc;

use unicode_width::UnicodeWidthChar;

const MAX_GRAPHEME_BYTES: usize = 32;
const ALLOCATION_OVERHEAD: usize = 16;
const ARC_OVERHEAD: usize = 2 * std::mem::size_of::<usize>() + ALLOCATION_OVERHEAD;

/// Match NyaTerm's native terminal Unicode width policy.
pub(crate) fn display_cell_width(ch: char) -> Option<usize> {
    UnicodeWidthChar::width(ch).map(|width| width.min(2))
}

/// OSC 8 hyperlink attached to terminal cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hyperlink {
    pub params: Vec<u8>,
    pub uri: Vec<u8>,
}

/// SGR attributes for one cell (subset sufficient for mosh paint + underline).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Attr {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub under: bool,
    pub blink: bool,
    pub reverse: bool,
    pub conceal: bool,
    pub strike: bool,
    pub fg: Color,
    pub bg: Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorType {
    #[default]
    Default,
    Index,
    Rgb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Color {
    pub kind: ColorType,
    /// Index 0–255, or 0x00RRGGBB for RGB.
    pub value: u32,
}

impl Color {
    pub const fn default_color() -> Self {
        Self {
            kind: ColorType::Default,
            value: 0,
        }
    }

    pub const fn index(idx: u32) -> Self {
        Self {
            kind: ColorType::Index,
            value: idx,
        }
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self {
            kind: ColorType::Rgb,
            value: ((r as u32) << 16) | ((g as u32) << 8) | (b as u32),
        }
    }
}

/// One character cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    /// Display width: 1 for normal, 2 for wide CJK, 0 for wide-continuation.
    pub width: u8,
    pub attr: Attr,
    /// Index into `Framebuffer::hyperlinks`; zero means no hyperlink.
    pub hyperlink: u32,
    /// UTF-8 codepoints that extend `ch` into one terminal grapheme.
    /// Most cells stay allocation-free; only combined glyphs allocate.
    grapheme_suffix: Option<Arc<str>>,
    /// Stock mosh distinguishes an erased cell from a printed space because a
    /// leading combining character gets a no-break-space fallback only in an
    /// empty cell.
    contents_empty: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            width: 1,
            attr: Attr::default(),
            hyperlink: 0,
            grapheme_suffix: None,
            contents_empty: true,
        }
    }
}

impl Cell {
    pub fn text(&self) -> String {
        let mut text = self.ch.to_string();
        if let Some(suffix) = &self.grapheme_suffix {
            text.push_str(suffix);
        }
        text
    }

    pub(crate) fn erased(background: Color) -> Self {
        Self {
            attr: Attr {
                bg: background,
                ..Attr::default()
            },
            ..Self::default()
        }
    }

    fn wide_continuation(background: Color) -> Self {
        let mut cell = Self::erased(background);
        cell.width = 0;
        cell
    }

    pub(crate) fn replace_char(&mut self, ch: char) {
        self.ch = ch;
        self.grapheme_suffix = None;
        self.contents_empty = false;
    }

    fn start_fallback(&mut self, ch: char) {
        // Store the same bytes that are painted. Replaying a diff therefore
        // produces an equivalent cell instead of a second fallback encoding.
        self.ch = '\u{a0}';
        self.width = 1;
        self.grapheme_suffix = Some(Arc::<str>::from(ch.to_string()));
        self.contents_empty = false;
    }

    fn content_len(&self) -> usize {
        if self.contents_empty {
            return 0;
        }
        // A leading combining character is stored in its painted, canonical
        // form (NBSP + suffix), but upstream does not count the synthetic NBSP
        // toward the 32-byte grapheme limit.
        let base_len = if self.ch == '\u{a0}' && self.grapheme_suffix.is_some() {
            0
        } else {
            self.ch.len_utf8()
        };
        base_len + self.grapheme_suffix.as_deref().map(str::len).unwrap_or(0)
    }

    fn append_grapheme_char(&mut self, ch: char) {
        // Match upstream: `full()` is checked before appending a complete
        // codepoint, so the stored contents may finish a few bytes over 32.
        if self.content_len() >= MAX_GRAPHEME_BYTES {
            return;
        }
        let mut suffix = self
            .grapheme_suffix
            .as_deref()
            .unwrap_or_default()
            .to_string();
        suffix.push(ch);
        self.grapheme_suffix = Some(Arc::<str>::from(suffix));
    }

    fn append_grapheme_to(&self, buf: &mut Vec<u8>) {
        push_char(buf, self.ch);
        if let Some(suffix) = self.grapheme_suffix.as_deref() {
            buf.extend_from_slice(suffix.as_bytes());
        }
    }
}

/// Terminal screen state (mosh-go `Framebuffer`).
#[derive(Debug, Clone)]
pub struct Framebuffer {
    pub cols: usize,
    pub rows: usize,
    /// Copy-on-write rows keep parallel SSP states cheap when only a subset of
    /// the screen changes. Rows, rather than whole screens, are shared.
    rows_data: Vec<Arc<Vec<Cell>>>,
    pub cur_x: usize,
    pub cur_y: usize,
    pub cursor_visible: bool,
    /// ANSI mode 4: printable cells shift existing content to the right.
    pub insert_mode: bool,
    /// DEC mode 7: printing past the last column wraps onto the next row.
    pub auto_wrap_mode: bool,
    /// DEC mode 6: absolute row addressing is relative to scroll margins.
    pub origin_mode: bool,
    /// DEC private modes that are part of the remote terminal state.
    pub reverse_video: bool,
    pub bracketed_paste: bool,
    pub mouse_reporting_mode: u16,
    pub mouse_focus_event: bool,
    pub mouse_encoding_mode: u16,
    /// Active scrolling margins, inclusive and zero-indexed.
    pub scroll_top: usize,
    pub scroll_bottom: usize,
    /// DEC horizontal tab stops. Stock terminals start with one every eight
    /// columns and allow applications to replace or clear them.
    tab_stops: Vec<bool>,
    /// Stateful side effects emitted by stock mosh's Display::new_frame.
    pub bell_count: u64,
    pub icon_name: Option<Vec<u8>>,
    pub window_title: Option<Vec<u8>>,
    pub clipboard: Option<Vec<u8>>,
    pub hyperlinks: Vec<Hyperlink>,
    pub active_hyperlink: u32,
    /// Cell receiving combining/variation/ZWJ continuations.
    combining_x: usize,
    combining_y: usize,
    combining_valid: bool,
    /// Stock DrawState `next_print_will_wrap` — last-col print defers wrap to next char.
    pub next_print_will_wrap: bool,
    /// Incremented on scroll_up / full clear so prediction can invalidate coords.
    pub scroll_generation: u64,
}

impl Framebuffer {
    pub fn new(cols: usize, rows: usize) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let blank_row = Arc::new(vec![Cell::default(); cols]);
        Self {
            cols,
            rows,
            rows_data: vec![blank_row; rows],
            cur_x: 0,
            cur_y: 0,
            cursor_visible: true,
            insert_mode: false,
            auto_wrap_mode: true,
            origin_mode: false,
            reverse_video: false,
            bracketed_paste: false,
            mouse_reporting_mode: 0,
            mouse_focus_event: false,
            mouse_encoding_mode: 0,
            scroll_top: 0,
            scroll_bottom: rows - 1,
            tab_stops: (0..cols)
                .map(|column| column > 0 && column % 8 == 0)
                .collect(),
            bell_count: 0,
            icon_name: None,
            window_title: None,
            clipboard: None,
            hyperlinks: Vec::new(),
            active_hyperlink: 0,
            combining_x: 0,
            combining_y: 0,
            combining_valid: true,
            next_print_will_wrap: false,
            scroll_generation: 0,
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols == self.cols && rows == self.rows {
            return;
        }
        let generation = self.scroll_generation;
        let mut next = Self::new(cols, rows);
        let copy_cols = self.cols.min(cols);
        let copy_rows = self.rows.min(rows);
        for y in 0..copy_rows {
            for x in 0..copy_cols {
                if let (Some(dst), Some(src)) = (next.cell_at_mut(x, y), self.cell_at(x, y)) {
                    *dst = src.clone();
                }
            }
        }
        next.cur_x = self.cur_x.min(cols.saturating_sub(1));
        next.cur_y = self.cur_y.min(rows.saturating_sub(1));
        next.cursor_visible = self.cursor_visible;
        next.insert_mode = self.insert_mode;
        next.auto_wrap_mode = self.auto_wrap_mode;
        next.origin_mode = self.origin_mode;
        next.reverse_video = self.reverse_video;
        next.bracketed_paste = self.bracketed_paste;
        next.mouse_reporting_mode = self.mouse_reporting_mode;
        next.mouse_focus_event = self.mouse_focus_event;
        next.mouse_encoding_mode = self.mouse_encoding_mode;
        for column in 0..copy_cols {
            next.tab_stops[column] = self.tab_stops[column];
        }
        next.bell_count = self.bell_count;
        next.icon_name = self.icon_name.clone();
        next.window_title = self.window_title.clone();
        next.clipboard = self.clipboard.clone();
        next.hyperlinks = self.hyperlinks.clone();
        next.active_hyperlink = self.active_hyperlink;
        next.combining_valid =
            self.combining_valid && self.combining_x < cols && self.combining_y < rows;
        if next.combining_valid {
            next.combining_x = self.combining_x;
            next.combining_y = self.combining_y;
        }
        next.next_print_will_wrap = false;
        next.scroll_generation = generation.wrapping_add(1);
        *self = next;
    }

    pub fn cell_at(&self, x: usize, y: usize) -> Option<&Cell> {
        if x >= self.cols || y >= self.rows {
            return None;
        }
        self.rows_data.get(y).and_then(|row| row.get(x))
    }

    pub fn cell_at_mut(&mut self, x: usize, y: usize) -> Option<&mut Cell> {
        if x >= self.cols || y >= self.rows {
            return None;
        }
        self.rows_data
            .get_mut(y)
            .and_then(|row| Arc::make_mut(row).get_mut(x))
    }

    pub(crate) fn retarget_combining_to_cursor(&mut self) {
        self.combining_x = self.cur_x;
        self.combining_y = self.cur_y;
        self.combining_valid = self.cur_x < self.cols && self.cur_y < self.rows;
    }

    pub(crate) fn set_tab_stop(&mut self, column: usize) {
        if let Some(stop) = self.tab_stops.get_mut(column) {
            *stop = true;
        }
    }

    pub(crate) fn clear_tab_stop(&mut self, column: usize) {
        if let Some(stop) = self.tab_stops.get_mut(column) {
            *stop = false;
        }
    }

    pub(crate) fn clear_all_tab_stops(&mut self) {
        self.tab_stops.fill(false);
    }

    pub(crate) fn next_tab_stop(&self, count: usize) -> usize {
        let mut column = self.cur_x;
        for _ in 0..count.max(1) {
            column = ((column + 1)..self.cols)
                .find(|candidate| self.tab_stops[*candidate])
                .unwrap_or_else(|| self.cols.saturating_sub(1));
        }
        column
    }

    pub(crate) fn previous_tab_stop(&self, count: usize) -> usize {
        let mut column = self.cur_x;
        for _ in 0..count.max(1) {
            column = (0..column)
                .rev()
                .find(|candidate| self.tab_stops[*candidate])
                .unwrap_or(0);
        }
        column
    }

    pub(crate) fn fill_all(&mut self, blank: &Cell) {
        let blank_row = Arc::new(vec![blank.clone(); self.cols]);
        self.rows_data = vec![blank_row; self.rows];
    }

    pub(crate) fn erase_row_range(
        &mut self,
        y: usize,
        start: usize,
        end_exclusive: usize,
        blank: &Cell,
    ) {
        if y >= self.rows || start >= end_exclusive || start >= self.cols {
            return;
        }
        let mut adjusted_start = start;
        let mut adjusted_end = end_exclusive.min(self.cols);
        if adjusted_start > 0
            && self
                .cell_at(adjusted_start, y)
                .is_some_and(|cell| cell.width == 0)
            && self
                .cell_at(adjusted_start - 1, y)
                .is_some_and(|cell| cell.width == 2)
        {
            adjusted_start -= 1;
        }
        if adjusted_end < self.cols
            && self
                .cell_at(adjusted_end - 1, y)
                .is_some_and(|cell| cell.width == 2)
            && self
                .cell_at(adjusted_end, y)
                .is_some_and(|cell| cell.width == 0)
        {
            adjusted_end += 1;
        }
        for x in adjusted_start..adjusted_end {
            if let Some(cell) = self.cell_at_mut(x, y) {
                *cell = blank.clone();
            }
        }
    }

    pub(crate) fn scroll_rows_up(&mut self, top: usize, bottom: usize, lines: usize, blank: &Cell) {
        if top >= self.rows || bottom >= self.rows || top > bottom {
            return;
        }
        let height = bottom - top + 1;
        let lines = lines.min(height);
        if lines == 0 {
            return;
        }
        let region = &mut self.rows_data[top..=bottom];
        region.rotate_left(lines);
        let blank_row = Arc::new(vec![blank.clone(); self.cols]);
        for row in &mut region[height - lines..] {
            *row = blank_row.clone();
        }
    }

    pub(crate) fn insert_blank_rows(
        &mut self,
        top: usize,
        bottom: usize,
        lines: usize,
        blank: &Cell,
    ) {
        if top >= self.rows || bottom >= self.rows || top > bottom {
            return;
        }
        let height = bottom - top + 1;
        let lines = lines.min(height);
        if lines == 0 {
            return;
        }
        let region = &mut self.rows_data[top..=bottom];
        region.rotate_right(lines);
        let blank_row = Arc::new(vec![blank.clone(); self.cols]);
        for row in &mut region[..lines] {
            *row = blank_row.clone();
        }
    }

    pub(crate) fn delete_rows(&mut self, top: usize, bottom: usize, lines: usize, blank: &Cell) {
        self.scroll_rows_up(top, bottom, lines, blank);
    }

    /// Stable row allocation identities and approximate owned bytes, used by
    /// the numbered-state memory budget without double-counting shared rows.
    pub(crate) fn row_storage(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.rows_data.iter().map(|row| {
            let suffix_bytes = row
                .iter()
                .filter_map(|cell| cell.grapheme_suffix.as_deref())
                .map(|suffix| suffix.len().saturating_add(ARC_OVERHEAD))
                .sum::<usize>();
            (
                Arc::as_ptr(row) as usize,
                row.capacity()
                    .saturating_mul(std::mem::size_of::<Cell>())
                    .saturating_add(std::mem::size_of::<Vec<Cell>>())
                    .saturating_add(ARC_OVERHEAD)
                    .saturating_add(suffix_bytes),
            )
        })
    }

    pub(crate) fn metadata_storage_bytes(&self) -> usize {
        self.rows_data.capacity() * std::mem::size_of::<Arc<Vec<Cell>>>()
            + ALLOCATION_OVERHEAD
            + self.tab_stops.capacity() * std::mem::size_of::<bool>()
            + if self.tab_stops.capacity() > 0 {
                ALLOCATION_OVERHEAD
            } else {
                0
            }
            + self.hyperlinks.capacity() * std::mem::size_of::<Hyperlink>()
            + if self.hyperlinks.capacity() > 0 {
                ALLOCATION_OVERHEAD
            } else {
                0
            }
            + self
                .hyperlinks
                .iter()
                .map(|link| {
                    link.params.capacity()
                        + link.uri.capacity()
                        + if link.params.capacity() > 0 {
                            ALLOCATION_OVERHEAD
                        } else {
                            0
                        }
                        + if link.uri.capacity() > 0 {
                            ALLOCATION_OVERHEAD
                        } else {
                            0
                        }
                })
                .sum::<usize>()
            + allocated_vec_bytes(self.icon_name.as_ref())
            + allocated_vec_bytes(self.window_title.as_ref())
            + allocated_vec_bytes(self.clipboard.as_ref())
    }

    /// Place a rune. Returns display columns used (1 or 2). Wide runes also
    /// mark the following cell as a width-0 continuation when room remains.
    pub fn put_rune(&mut self, x: usize, y: usize, ch: char, attr: Attr) -> usize {
        self.put_rune_with_hyperlink(x, y, ch, attr, 0)
    }

    /// Place a rune carrying the currently active OSC 8 hyperlink.
    pub fn put_rune_with_hyperlink(
        &mut self,
        x: usize,
        y: usize,
        ch: char,
        attr: Attr,
        hyperlink: u32,
    ) -> usize {
        let Some(w) = display_cell_width(ch) else {
            return 0;
        };
        if w == 0 {
            return 0;
        }
        // Xterm clears the whole existing wide glyph when a printable cell is
        // written into its continuation column. Stock Display::new_frame can
        // legally produce this pattern after a right-margin wide wrap (BS+EL,
        // followed by spaces in a later frame). Leaving the owner intact makes
        // the new cell invisible and creates a persistent ghost glyph.
        let overwrites_wide_continuation = self.cell_at(x, y).is_some_and(|cell| cell.width == 0);
        if overwrites_wide_continuation && x > 0 {
            let owns_continuation = self.cell_at(x - 1, y).is_some_and(|cell| cell.width == 2);
            if owns_continuation && let Some(owner) = self.cell_at_mut(x - 1, y) {
                *owner = Cell::erased(attr.bg);
            }
        }
        // A new wide glyph can also cover the owner column of a different
        // wide glyph immediately to its right. Xterm uncovers that old
        // glyph's trailing continuation as a blank with the old background.
        let overlaps_wide_owner_on_right = w == 2
            && x + 2 < self.cols
            && self.cell_at(x + 1, y).is_some_and(|cell| cell.width == 2)
            && self.cell_at(x + 2, y).is_some_and(|cell| cell.width == 0);
        if overlaps_wide_owner_on_right && let Some(trailing) = self.cell_at_mut(x + 2, y) {
            *trailing = Cell::erased(trailing.attr.bg);
        }
        let old_width = self.cell_at(x, y).map(|cell| cell.width).unwrap_or(1);
        if let Some(cell) = self.cell_at_mut(x, y) {
            cell.replace_char(ch);
            cell.width = w as u8;
            cell.attr = attr;
            cell.hyperlink = hyperlink;
        }
        if w == 2 && x + 1 < self.cols {
            if let Some(cont) = self.cell_at_mut(x + 1, y) {
                *cont = Cell::wide_continuation(attr.bg);
            }
        } else if old_width == 2
            && x + 1 < self.cols
            && let Some(cont) = self.cell_at_mut(x + 1, y)
            && cont.width == 0
        {
            *cont = Cell::erased(cont.attr.bg);
        }
        self.combining_x = x;
        self.combining_y = y;
        self.combining_valid = true;
        w
    }

    /// Append one zero-width codepoint to the combining cell selected by the
    /// server terminal model. Width is fixed by the first printable codepoint.
    pub(crate) fn try_extend_active_grapheme(&mut self, ch: char) -> bool {
        match display_cell_width(ch) {
            Some(0) => {}
            Some(_) => return false,
            None => return true,
        }
        if !self.combining_valid {
            return true;
        }
        let x = self.combining_x;
        let y = self.combining_y;
        let Some(previous) = self.cell_at(x, y).cloned() else {
            self.combining_valid = false;
            return true;
        };

        if previous.contents_empty {
            if let Some(cell) = self.cell_at_mut(x, y) {
                cell.start_fallback(ch);
            }
            // Upstream advances once when a combining character starts an
            // empty cell, leaving the combining target on that fallback cell.
            if self.cur_x + 1 >= self.cols {
                self.cur_x = self.cols.saturating_sub(1);
                self.next_print_will_wrap = true;
            } else {
                self.cur_x += 1;
                self.next_print_will_wrap = false;
            }
        } else if let Some(cell) = self.cell_at_mut(x, y) {
            cell.append_grapheme_char(ch);
        }
        true
    }

    /// Select an OSC 8 hyperlink, interning it so cells remain cheap to clone.
    pub fn set_active_hyperlink(&mut self, params: &[u8], uri: &[u8]) {
        if uri.is_empty() {
            self.active_hyperlink = 0;
            return;
        }
        if let Some(index) = self
            .hyperlinks
            .iter()
            .position(|link| link.params == params && link.uri == uri)
        {
            self.active_hyperlink = index as u32 + 1;
            return;
        }
        self.hyperlinks.push(Hyperlink {
            params: params.to_vec(),
            uri: uri.to_vec(),
        });
        self.active_hyperlink = self.hyperlinks.len() as u32;
    }

    /// Drop hyperlink definitions no longer referenced by a cell or by the
    /// active pen. Long-running shells can otherwise retain every OSC 8 URL
    /// ever seen, and numbered remote snapshots multiply that history.
    pub(crate) fn compact_hyperlinks(&mut self) {
        if self.hyperlinks.is_empty() {
            return;
        }
        let mut referenced = vec![false; self.hyperlinks.len() + 1];
        let mut has_invalid_reference = false;
        let mut mark_referenced = |id: u32| {
            if id == 0 {
                return;
            }
            if let Some(slot) = referenced.get_mut(id as usize) {
                *slot = true;
            } else {
                has_invalid_reference = true;
            }
        };
        mark_referenced(self.active_hyperlink);
        for row in &self.rows_data {
            for cell in row.iter() {
                mark_referenced(cell.hyperlink);
            }
        }
        if !has_invalid_reference
            && referenced
                .iter()
                .skip(1)
                .all(|is_referenced| *is_referenced)
        {
            return;
        }

        let old_links = &self.hyperlinks;
        let mut compact = Vec::new();
        let mut remap = vec![0u32; old_links.len() + 1];
        for old_id in 1..=old_links.len() {
            if !referenced[old_id] {
                continue;
            }
            remap[old_id] = remap_hyperlink(old_id as u32, old_links, &mut compact);
        }
        let identity = compact.len() == old_links.len()
            && remap
                .iter()
                .enumerate()
                .all(|(old_id, new_id)| *new_id == old_id as u32);
        if identity {
            return;
        }

        for row in &mut self.rows_data {
            let needs_remap = row
                .iter()
                .any(|cell| remapped_id(cell.hyperlink, &remap) != cell.hyperlink);
            if !needs_remap {
                continue;
            }
            for cell in Arc::make_mut(row) {
                cell.hyperlink = remapped_id(cell.hyperlink, &remap);
            }
        }
        self.active_hyperlink = remapped_id(self.active_hyperlink, &remap);
        self.hyperlinks = compact;
    }

    fn hyperlink(&self, id: u32) -> Option<&Hyperlink> {
        id.checked_sub(1)
            .and_then(|index| self.hyperlinks.get(index as usize))
    }

    /// Diff this framebuffer against `old` (mosh-go `Diff`).
    /// When `old` is `None` or size differs, emit a full redraw.
    pub fn diff(&self, old: Option<&Framebuffer>) -> Vec<u8> {
        match old {
            Some(prev) if prev.cols == self.cols && prev.rows == self.rows => {
                self.diff_same_size(prev)
            }
            _ => self.full_redraw(old),
        }
    }

    fn full_redraw(&self, old: Option<&Framebuffer>) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.cols * self.rows + 64);
        buf.extend_from_slice(b"\x1b[?25l");
        buf.extend_from_slice(b"\x1b[r");
        buf.extend_from_slice(b"\x1b[H");
        buf.extend_from_slice(b"\x1b[2J");
        buf.extend_from_slice(b"\x1b[m");
        // SGR and screen clears do not end OSC 8. Start from a known hyperlink
        // state before redrawing cells from a differently sized frame.
        buf.extend_from_slice(b"\x1b]8;;\x1b\\");

        let mut cur_attr = Attr::default();
        let mut cur_link: Option<Hyperlink> = None;
        for y in 0..self.rows {
            if y > 0 {
                buf.extend_from_slice(b"\r\n");
            }
            let mut pen_x: isize = 0;
            let visible = visible_cell_starts(self, y);
            let mut last_non_space = None;
            for x in (0..self.cols).rev() {
                if !visible[x] {
                    continue;
                }
                let c = self.cell_at(x, y).expect("framebuffer bounds");
                if (c.ch != ' ' && c.ch != '\0')
                    || c.grapheme_suffix.is_some()
                    || c.attr != Attr::default()
                    || c.hyperlink != 0
                {
                    last_non_space = Some(x);
                    break;
                }
            }
            if let Some(last) = last_non_space {
                for (x, is_visible) in visible.iter().enumerate().take(last + 1) {
                    if !is_visible {
                        continue;
                    }
                    let c = self.cell_at(x, y).expect("framebuffer bounds");
                    if pen_x != x as isize {
                        append_cup(&mut buf, y, x);
                    }
                    append_hyperlink_diff(&mut buf, &mut cur_link, self.hyperlink(c.hyperlink));
                    append_attr_diff(&mut buf, &mut cur_attr, &c.attr);
                    push_cell(&mut buf, c);
                    // The remote terminal and xterm.js can disagree about
                    // emoji width. Re-address after any wide cell so later
                    // paint cannot shift left on the local terminal.
                    pen_x = next_local_pen_x(c, x);
                }
            }
        }

        if cur_attr != Attr::default() {
            buf.extend_from_slice(b"\x1b[m");
        }
        append_hyperlink_diff(
            &mut buf,
            &mut cur_link,
            self.hyperlink(self.active_hyperlink),
        );
        append_terminal_metadata(&mut buf, self, old, true);
        append_cup(&mut buf, self.cur_y, self.cur_x);
        if self.cursor_visible {
            buf.extend_from_slice(b"\x1b[?25h");
        }
        buf
    }

    fn diff_same_size(&self, old: &Framebuffer) -> Vec<u8> {
        let mut buf = Vec::new();
        append_terminal_metadata(&mut buf, self, Some(old), false);

        let shared_rows_have_same_links = self.hyperlinks == old.hyperlinks;
        let change_spans = (0..self.rows)
            .map(|y| {
                if shared_rows_have_same_links && Arc::ptr_eq(&self.rows_data[y], &old.rows_data[y])
                {
                    None
                } else {
                    row_display_change_span(self, old, y)
                }
            })
            .collect::<Vec<_>>();
        let cells_changed = change_spans.iter().any(Option::is_some);
        if cells_changed {
            buf.extend_from_slice(b"\x1b[?25l");
        }

        let mut cur_attr = Attr::default();
        let mut cur_link = old.hyperlink(old.active_hyperlink).cloned();
        let mut pen_x: isize = -1;
        let mut pen_y: isize = -1;

        for (y, span) in change_spans.into_iter().enumerate() {
            let Some((first, last)) = span else {
                continue;
            };
            let visible = visible_cell_starts(self, y);
            for (x, is_visible) in visible.iter().enumerate().take(last + 1).skip(first) {
                if !is_visible {
                    continue;
                }
                let c = self.cell_at(x, y).expect("framebuffer bounds");
                if pen_x != x as isize || pen_y != y as isize {
                    append_cup(&mut buf, y, x);
                    pen_y = y as isize;
                }
                append_hyperlink_diff(&mut buf, &mut cur_link, self.hyperlink(c.hyperlink));
                append_attr_diff(&mut buf, &mut cur_attr, &c.attr);
                push_cell(&mut buf, c);
                // Always re-address after a wide cell. The server-side libc
                // and xterm.js do not agree on every emoji width, so assuming
                // the local cursor advanced two columns can leave later
                // erases or glyphs one column short.
                pen_x = next_local_pen_x(c, x);
            }
        }

        if cur_attr != Attr::default() {
            buf.extend_from_slice(b"\x1b[m");
        }
        append_hyperlink_diff(
            &mut buf,
            &mut cur_link,
            self.hyperlink(self.active_hyperlink),
        );

        let margins_changed =
            self.scroll_top != old.scroll_top || self.scroll_bottom != old.scroll_bottom;
        if cells_changed || margins_changed || self.cur_x != old.cur_x || self.cur_y != old.cur_y {
            append_cup(&mut buf, self.cur_y, self.cur_x);
        }
        if cells_changed {
            if self.cursor_visible {
                buf.extend_from_slice(b"\x1b[?25h");
            }
        } else if self.cursor_visible != old.cursor_visible {
            if self.cursor_visible {
                buf.extend_from_slice(b"\x1b[?25h");
            } else {
                buf.extend_from_slice(b"\x1b[?25l");
            }
        }

        buf
    }
}

fn remap_hyperlink(id: u32, old: &[Hyperlink], compact: &mut Vec<Hyperlink>) -> u32 {
    let Some(link) = id.checked_sub(1).and_then(|index| old.get(index as usize)) else {
        return 0;
    };
    if let Some(index) = compact.iter().position(|candidate| candidate == link) {
        return index as u32 + 1;
    }
    compact.push(link.clone());
    compact.len() as u32
}

fn remapped_id(id: u32, remap: &[u32]) -> u32 {
    remap.get(id as usize).copied().unwrap_or(0)
}

fn allocated_vec_bytes(value: Option<&Vec<u8>>) -> usize {
    value
        .map(|bytes| {
            bytes.capacity()
                + if bytes.capacity() > 0 {
                    ALLOCATION_OVERHEAD
                } else {
                    0
                }
        })
        .unwrap_or(0)
}

/// Cell columns that can actually be painted on a terminal. A wide cell owns
/// the following display column even if the terminal model later stores an
/// independent cell there; that hidden cell becomes visible only after the
/// wide cell is replaced.
fn visible_cell_starts(framebuffer: &Framebuffer, y: usize) -> Vec<bool> {
    let mut visible = vec![false; framebuffer.cols];
    let mut x = 0;
    while x < framebuffer.cols {
        let cell = framebuffer.cell_at(x, y).expect("framebuffer bounds");
        if cell.width == 0 {
            x += 1;
            continue;
        }
        visible[x] = true;
        x += usize::from(cell.width.max(1));
    }
    visible
}

fn row_display_change_span(
    next: &Framebuffer,
    old: &Framebuffer,
    y: usize,
) -> Option<(usize, usize)> {
    let next_visible = visible_cell_starts(next, y);
    let old_visible = visible_cell_starts(old, y);
    let mut first = None;
    let mut last = None;

    for x in 0..next.cols {
        if !next_visible[x] {
            continue;
        }
        let changed = !old_visible[x]
            || !cells_equivalent(
                next,
                next.cell_at(x, y).expect("framebuffer bounds"),
                old,
                old.cell_at(x, y).expect("framebuffer bounds"),
            );
        if changed {
            first.get_or_insert(x);
            last = Some(x);
        }
    }

    first.zip(last)
}

fn cells_equivalent(
    left_fb: &Framebuffer,
    left: &Cell,
    right_fb: &Framebuffer,
    right: &Cell,
) -> bool {
    left.ch == right.ch
        && left.width == right.width
        && left.attr == right.attr
        && left.grapheme_suffix == right.grapheme_suffix
        && left_fb.hyperlink(left.hyperlink) == right_fb.hyperlink(right.hyperlink)
}

fn append_terminal_metadata(
    buf: &mut Vec<u8>,
    next: &Framebuffer,
    old: Option<&Framebuffer>,
    force_persistent: bool,
) {
    let bell_changed = old
        .map(|fb| next.bell_count != fb.bell_count)
        .unwrap_or(next.bell_count != 0);
    if bell_changed {
        buf.push(0x07);
    }

    let old_icon = old.and_then(|fb| fb.icon_name.as_deref());
    let old_title = old.and_then(|fb| fb.window_title.as_deref());
    let icon_changed = force_persistent || next.icon_name.as_deref() != old_icon;
    let title_changed = force_persistent || next.window_title.as_deref() != old_title;
    if (icon_changed || title_changed)
        && next.icon_name.is_some()
        && next.icon_name == next.window_title
    {
        append_osc(buf, b"0;", next.icon_name.as_deref().unwrap_or_default());
    } else {
        if icon_changed && let Some(icon) = next.icon_name.as_deref() {
            append_osc(buf, b"1;", icon);
        }
        if title_changed && let Some(title) = next.window_title.as_deref() {
            append_osc(buf, b"2;", title);
        }
    }
    if next.clipboard.as_deref() != old.and_then(|fb| fb.clipboard.as_deref()) {
        append_osc(buf, b"52;c;", next.clipboard.as_deref().unwrap_or_default());
    }

    append_private_mode_diff(
        buf,
        5,
        next.reverse_video,
        old.map(|fb| fb.reverse_video).unwrap_or(false),
        force_persistent,
    );
    append_private_mode_diff(
        buf,
        2004,
        next.bracketed_paste,
        old.map(|fb| fb.bracketed_paste).unwrap_or(false),
        force_persistent,
    );

    let old_reporting = old.map(|fb| fb.mouse_reporting_mode).unwrap_or(0);
    if force_persistent || next.mouse_reporting_mode != old_reporting {
        if force_persistent {
            for mode in [1003, 1002, 1001, 1000, 9] {
                append_private_mode(buf, mode, false);
            }
        } else if old_reporting != 0 {
            append_private_mode(buf, old_reporting, false);
        }
        if next.mouse_reporting_mode != 0 {
            append_private_mode(buf, next.mouse_reporting_mode, true);
        }
    }
    append_private_mode_diff(
        buf,
        1004,
        next.mouse_focus_event,
        old.map(|fb| fb.mouse_focus_event).unwrap_or(false),
        force_persistent,
    );
    let old_encoding = old.map(|fb| fb.mouse_encoding_mode).unwrap_or(0);
    if force_persistent || next.mouse_encoding_mode != old_encoding {
        if force_persistent {
            for mode in [1015, 1006, 1005] {
                append_private_mode(buf, mode, false);
            }
        } else if old_encoding != 0 {
            append_private_mode(buf, old_encoding, false);
        }
        if next.mouse_encoding_mode != 0 {
            append_private_mode(buf, next.mouse_encoding_mode, true);
        }
    }

    let margins_changed = if force_persistent {
        next.scroll_top != 0 || next.scroll_bottom + 1 != next.rows
    } else {
        old.map(|fb| fb.scroll_top != next.scroll_top || fb.scroll_bottom != next.scroll_bottom)
            .unwrap_or(false)
    };
    if margins_changed {
        if next.scroll_top == 0 && next.scroll_bottom + 1 == next.rows {
            buf.extend_from_slice(b"\x1b[r");
        } else {
            buf.extend_from_slice(b"\x1b[");
            buf.extend_from_slice((next.scroll_top + 1).to_string().as_bytes());
            buf.push(b';');
            buf.extend_from_slice((next.scroll_bottom + 1).to_string().as_bytes());
            buf.push(b'r');
        }
    }
}

fn append_osc(buf: &mut Vec<u8>, prefix: &[u8], value: &[u8]) {
    buf.extend_from_slice(b"\x1b]");
    buf.extend_from_slice(prefix);
    buf.extend_from_slice(value);
    buf.push(0x07);
}

fn append_private_mode_diff(buf: &mut Vec<u8>, mode: u16, next: bool, old: bool, force: bool) {
    if force || next != old {
        append_private_mode(buf, mode, next);
    }
}

fn append_private_mode(buf: &mut Vec<u8>, mode: u16, enabled: bool) {
    buf.extend_from_slice(b"\x1b[?");
    buf.extend_from_slice(mode.to_string().as_bytes());
    buf.push(if enabled { b'h' } else { b'l' });
}

fn append_hyperlink_diff(
    buf: &mut Vec<u8>,
    current: &mut Option<Hyperlink>,
    next: Option<&Hyperlink>,
) {
    if current.as_ref() == next {
        return;
    }
    buf.extend_from_slice(b"\x1b]8;");
    if let Some(link) = next {
        buf.extend_from_slice(&link.params);
        buf.push(b';');
        buf.extend_from_slice(&link.uri);
    } else {
        buf.push(b';');
    }
    buf.extend_from_slice(b"\x1b\\");
    *current = next.cloned();
}

fn push_cell(buf: &mut Vec<u8>, cell: &Cell) {
    if cell.ch == '\0' || (cell.ch == ' ' && cell.grapheme_suffix.is_none()) {
        buf.push(b' ');
    } else {
        cell.append_grapheme_to(buf);
    }
}

fn next_local_pen_x(cell: &Cell, x: usize) -> isize {
    let joins_next_grapheme = cell
        .grapheme_suffix
        .as_deref()
        .and_then(|suffix| suffix.chars().last())
        == Some('\u{200d}');
    if cell.width == 2 && !joins_next_grapheme {
        -1
    } else {
        x as isize + cell.width.max(1) as isize
    }
}

fn push_char(buf: &mut Vec<u8>, ch: char) {
    let mut tmp = [0u8; 4];
    let encoded = ch.encode_utf8(&mut tmp);
    buf.extend_from_slice(encoded.as_bytes());
}

fn append_cup(buf: &mut Vec<u8>, row: usize, col: usize) {
    // 1-indexed CUP
    buf.extend_from_slice(b"\x1b[");
    buf.extend_from_slice((row + 1).to_string().as_bytes());
    buf.push(b';');
    buf.extend_from_slice((col + 1).to_string().as_bytes());
    buf.push(b'H');
}

fn append_attr_diff(buf: &mut Vec<u8>, cur: &mut Attr, next: &Attr) {
    if cur == next {
        return;
    }

    let needs_reset = (cur.bold && !next.bold)
        || (cur.dim && !next.dim)
        || (cur.italic && !next.italic)
        || (cur.under && !next.under)
        || (cur.blink && !next.blink)
        || (cur.reverse && !next.reverse)
        || (cur.conceal && !next.conceal)
        || (cur.strike && !next.strike)
        || (cur.fg.kind != ColorType::Default && next.fg.kind == ColorType::Default)
        || (cur.bg.kind != ColorType::Default && next.bg.kind == ColorType::Default);

    if needs_reset {
        buf.extend_from_slice(b"\x1b[0");
        *cur = Attr::default();
    } else {
        buf.extend_from_slice(b"\x1b[");
    }

    let mut first = !needs_reset;
    let mut add = |code: &str| {
        if !first {
            buf.push(b';');
        }
        first = false;
        buf.extend_from_slice(code.as_bytes());
    };

    if next.bold && !cur.bold {
        add("1");
    }
    if next.dim && !cur.dim {
        add("2");
    }
    if next.italic && !cur.italic {
        add("3");
    }
    if next.under && !cur.under {
        add("4");
    }
    if next.blink && !cur.blink {
        add("5");
    }
    if next.reverse && !cur.reverse {
        add("7");
    }
    if next.conceal && !cur.conceal {
        add("8");
    }
    if next.strike && !cur.strike {
        add("9");
    }

    if next.fg != cur.fg {
        append_color_param(buf, &next.fg, true, &mut first);
    }
    if next.bg != cur.bg {
        append_color_param(buf, &next.bg, false, &mut first);
    }

    // If we only opened CSI without params (identical after reset path edge),
    // still close SGR.
    if first && !needs_reset {
        // Nothing added — drop the open CSI we wrote.
        // Safer to emit reset+rebuild:
        buf.truncate(buf.len().saturating_sub(2)); // remove "\x1b["
        *cur = *next;
        return;
    }

    buf.push(b'm');
    *cur = *next;
}

fn append_color_param(buf: &mut Vec<u8>, c: &Color, fg: bool, first: &mut bool) {
    let sep = |buf: &mut Vec<u8>, first: &mut bool| {
        if !*first {
            buf.push(b';');
        }
        *first = false;
    };
    match c.kind {
        ColorType::Default => {
            sep(buf, first);
            buf.extend_from_slice(if fg { b"39" } else { b"49" });
        }
        ColorType::Index if c.value < 8 => {
            sep(buf, first);
            let base = if fg { 30 } else { 40 };
            buf.extend_from_slice((base + c.value).to_string().as_bytes());
        }
        ColorType::Index if c.value < 16 => {
            sep(buf, first);
            let base = if fg { 90 } else { 100 };
            buf.extend_from_slice((base + c.value - 8).to_string().as_bytes());
        }
        ColorType::Index => {
            sep(buf, first);
            buf.extend_from_slice(if fg { b"38;5;" } else { b"48;5;" });
            buf.extend_from_slice(c.value.to_string().as_bytes());
        }
        ColorType::Rgb => {
            sep(buf, first);
            let r = (c.value >> 16) & 0xff;
            let g = (c.value >> 8) & 0xff;
            let b = c.value & 0xff;
            buf.extend_from_slice(if fg { b"38;2;" } else { b"48;2;" });
            buf.extend_from_slice(r.to_string().as_bytes());
            buf.push(b';');
            buf.extend_from_slice(g.to_string().as_bytes());
            buf.push(b';');
            buf.extend_from_slice(b.to_string().as_bytes());
        }
    }
}

/// Ordering helper for tests.
pub fn cmp_cells(a: &Cell, b: &Cell) -> Ordering {
    (a.ch, a.width, a.attr.under).cmp(&(b.ch, b.width, b.attr.under))
}

#[cfg(test)]
mod tests {
    use super::{Arc, Attr, Color, Framebuffer, Hyperlink, UnicodeWidthChar};
    use crate::ansi_apply::apply_ansi;

    #[test]
    fn put_and_cell_at() {
        let mut fb = Framebuffer::new(80, 24);
        fb.put_rune(3, 2, 'Z', Attr::default());
        assert_eq!(fb.cell_at(3, 2).unwrap().ch, 'Z');
    }

    #[test]
    fn diff_empty_when_identical() {
        let fb = Framebuffer::new(10, 5);
        let other = fb.clone();
        assert!(fb.diff(Some(&other)).is_empty());
    }

    #[test]
    fn diff_emits_changed_glyph() {
        let old = Framebuffer::new(10, 5);
        let mut new = old.clone();
        new.put_rune(
            0,
            0,
            'a',
            Attr {
                under: true,
                ..Attr::default()
            },
        );
        new.cur_x = 1;
        let paint = new.diff(Some(&old));
        assert!(paint.windows(1).any(|w| w == b"a"));
        // underline SGR
        assert!(
            paint
                .windows(3)
                .any(|w| w == b"\x1b[4" || w.starts_with(b"\x1b["))
        );
        assert!(String::from_utf8_lossy(&paint).contains('a'));
    }

    #[test]
    fn full_redraw_on_size_change() {
        let old = Framebuffer::new(10, 5);
        let new = Framebuffer::new(20, 10);
        let paint = new.diff(Some(&old));
        assert!(paint.windows(4).any(|w| w == b"\x1b[2J"));
    }

    #[test]
    fn resize_redraw_does_not_repeat_one_shot_side_effects() {
        let mut old = Framebuffer::new(10, 5);
        old.bell_count = 1;
        old.clipboard = Some(b"remote clipboard".to_vec());
        let mut new = old.clone();
        new.resize(20, 10);

        let paint = new.diff(Some(&old));
        assert!(
            !paint.contains(&0x07),
            "resize must not ring an old bell again"
        );
        assert!(
            !paint.windows(5).any(|window| window == b"\x1b]52;"),
            "resize must not rewrite an unchanged remote clipboard"
        );
    }

    #[test]
    fn diff_clears_a_remote_clipboard_value() {
        let mut old = Framebuffer::new(10, 5);
        old.clipboard = Some(b"YQ==".to_vec());
        let mut next = old.clone();
        next.clipboard = None;

        assert_eq!(next.diff(Some(&old)), b"\x1b]52;c;\x07");
    }

    #[test]
    fn cloned_frames_copy_only_the_rows_that_change() {
        let base = Framebuffer::new(80, 24);
        let mut branch = base.clone();
        assert!(Arc::ptr_eq(&base.rows_data[0], &branch.rows_data[0]));
        assert!(Arc::ptr_eq(&base.rows_data[1], &branch.rows_data[1]));

        branch.put_rune(0, 0, 'x', Attr::default());
        assert!(!Arc::ptr_eq(&base.rows_data[0], &branch.rows_data[0]));
        assert!(Arc::ptr_eq(&base.rows_data[1], &branch.rows_data[1]));
        assert_eq!(base.cell_at(0, 0).unwrap().ch, ' ');
        assert_eq!(branch.cell_at(0, 0).unwrap().ch, 'x');
    }

    #[test]
    fn zero_width_suffixes_do_not_change_the_server_cell_width() {
        assert_eq!(UnicodeWidthChar::width('\u{1f3fd}'), Some(2));
        assert_eq!(UnicodeWidthChar::width('\u{1f1e8}'), Some(1));

        let mut heart = Framebuffer::new(8, 2);
        apply_ansi(&mut heart, "❤️X".as_bytes());
        assert_eq!(heart.cell_at(0, 0).unwrap().ch, '❤');
        assert_eq!(heart.cell_at(0, 0).unwrap().width, 1);
        assert_eq!(heart.cell_at(1, 0).unwrap().ch, 'X');
        assert_eq!(heart.cell_at(1, 0).unwrap().width, 1);
        assert_eq!(heart.cur_x, 2);

        let mut zwj = Framebuffer::new(8, 2);
        apply_ansi(&mut zwj, "👩‍💻X".as_bytes());
        assert_eq!(zwj.cell_at(0, 0).unwrap().ch, '👩');
        assert_eq!(zwj.cell_at(0, 0).unwrap().width, 2);
        assert_eq!(zwj.cell_at(2, 0).unwrap().ch, '💻');
        assert_eq!(zwj.cell_at(2, 0).unwrap().width, 2);
        assert_eq!(zwj.cell_at(4, 0).unwrap().ch, 'X');
        assert_eq!(zwj.cur_x, 5);
    }

    #[test]
    fn explicit_cursor_move_retargets_combining_characters() {
        let mut fb = Framebuffer::new(8, 2);
        apply_ansi(&mut fb, "a\x1b[2;1H\u{301}".as_bytes());

        assert!(fb.cell_at(0, 0).unwrap().grapheme_suffix.is_none());
        let fallback = fb.cell_at(0, 1).unwrap();
        assert_eq!(fallback.ch, '\u{a0}');
        assert_eq!(fallback.grapheme_suffix.as_deref(), Some("\u{301}"));
        assert_eq!(fb.cur_x, 1);
        assert_eq!(fb.cur_y, 1);
        let paint = fb.diff(None);
        assert!(
            paint
                .windows(4)
                .any(|window| window == "\u{a0}\u{301}".as_bytes())
        );
    }

    #[test]
    fn leading_combining_character_diff_round_trips() {
        let blank = Framebuffer::new(8, 2);
        let mut frame = blank.clone();
        apply_ansi(&mut frame, "\u{301}".as_bytes());

        let paint = frame.diff(Some(&blank));
        let mut replay = blank.clone();
        apply_ansi(&mut replay, &paint);

        assert!(
            frame.diff(Some(&replay)).is_empty(),
            "a rendered leading combining character must replay to the same frame"
        );
    }

    #[test]
    fn resize_does_not_clamp_an_offscreen_combining_target() {
        let mut fb = Framebuffer::new(5, 1);
        apply_ansi(&mut fb, b"\x1b[1;5HZ");
        fb.resize(2, 1);
        apply_ansi(&mut fb, "\u{301}".as_bytes());

        assert!(fb.cell_at(1, 0).unwrap().grapheme_suffix.is_none());
        assert_eq!(fb.cell_at(1, 0).unwrap().ch, ' ');
    }

    #[test]
    fn wide_then_narrow_restores_trailing_blank() {
        let mut fb = Framebuffer::new(5, 1);
        apply_ansi(
            &mut fb,
            "\x1b[31;48;5;4;4m\x1b]8;;https://example.test\x1b\\界\x1b[1;1HX".as_bytes(),
        );

        assert_eq!(fb.cell_at(0, 0).unwrap().ch, 'X');
        assert_eq!(fb.cell_at(0, 0).unwrap().width, 1);
        let restored = fb.cell_at(1, 0).unwrap();
        assert_eq!(restored.ch, ' ');
        assert_eq!(restored.width, 1);
        assert_eq!(restored.attr.bg, Color::index(4));
        assert_eq!(restored.attr.fg, Color::default_color());
        assert!(!restored.attr.under);
        assert_eq!(restored.hyperlink, 0);
    }

    #[test]
    fn narrowing_a_wide_cell_preserves_an_overwritten_continuation() {
        let mut fb = Framebuffer::new(5, 1);
        apply_ansi(
            &mut fb,
            "界\x1b[1;2H\x1b[32;48;5;4;4m\x1b]8;;https://example.test\x1b\\Y\x1b[1;1HX".as_bytes(),
        );

        assert_eq!(fb.cell_at(0, 0).unwrap().ch, 'X');
        assert_eq!(fb.cell_at(0, 0).unwrap().width, 1);
        let overwritten = fb.cell_at(1, 0).unwrap();
        assert_eq!(overwritten.ch, 'Y');
        assert_eq!(overwritten.width, 1);
        assert_eq!(overwritten.attr.fg, Color::index(2));
        assert_eq!(overwritten.attr.bg, Color::index(4));
        assert!(overwritten.attr.under);
        assert!(fb.hyperlink(overwritten.hyperlink).is_some());
    }

    #[test]
    fn overlapping_wide_glyph_clears_the_old_trailing_continuation() {
        let mut fb = Framebuffer::new(5, 1);
        apply_ansi(&mut fb, "\x1b[41m\x1b[1;2H界\x1b[44m\x1b[1;1H界".as_bytes());

        assert_eq!(fb.cell_at(0, 0).unwrap().ch, '界');
        assert_eq!(fb.cell_at(0, 0).unwrap().width, 2);
        assert_eq!(fb.cell_at(1, 0).unwrap().width, 0);
        assert_eq!(fb.cell_at(2, 0).unwrap().ch, ' ');
        assert_eq!(fb.cell_at(2, 0).unwrap().width, 1);
        assert_eq!(fb.cell_at(2, 0).unwrap().attr.bg, Color::index(1));
    }

    #[test]
    fn unprintable_unicode_is_ignored_instead_of_combined() {
        assert_eq!(UnicodeWidthChar::width('\u{85}'), None);

        let mut fb = Framebuffer::new(5, 1);
        apply_ansi(&mut fb, "A\u{85}B".as_bytes());

        assert_eq!(fb.cell_at(0, 0).unwrap().ch, 'A');
        assert!(fb.cell_at(0, 0).unwrap().grapheme_suffix.is_none());
        assert_eq!(fb.cell_at(1, 0).unwrap().ch, 'B');
        assert_eq!(fb.cur_x, 2);
    }

    #[test]
    fn metadata_storage_counts_hyperlink_entries() {
        let mut fb = Framebuffer::new(1, 1);
        let baseline = fb.metadata_storage_bytes();
        for index in 0..64 {
            fb.set_active_hyperlink(
                format!("id={index}").as_bytes(),
                format!("https://e/{index}").as_bytes(),
            );
        }
        let payload_bytes = fb
            .hyperlinks
            .iter()
            .map(|link| link.params.capacity() + link.uri.capacity())
            .sum::<usize>();
        let entry_bytes = fb.hyperlinks.capacity() * std::mem::size_of::<Hyperlink>();

        assert!(fb.metadata_storage_bytes() - baseline >= payload_bytes + entry_bytes);
    }
}
