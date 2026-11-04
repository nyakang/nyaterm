//! Observe DECSTBM without exposing the dependency's private grid state.
use alacritty_terminal::vte::ansi::{Handler, Processor};

pub(crate) struct ScrollRegionTracker {
    parser: Processor,
    region: Region,
}

struct Region {
    rows: usize,
    top: usize,
    bottom: usize,
}

impl Handler for Region {
    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        let bottom = bottom.unwrap_or(self.rows);
        if top >= bottom {
            return;
        }
        self.top = top.saturating_sub(1).min(self.rows);
        self.bottom = bottom.min(self.rows).saturating_sub(1);
    }
    fn reset_state(&mut self) {
        self.top = 0;
        self.bottom = self.rows.saturating_sub(1);
    }
}

impl ScrollRegionTracker {
    pub(crate) fn new(rows: usize) -> Self {
        Self {
            parser: Processor::new(),
            region: Region {
                rows,
                top: 0,
                bottom: rows.saturating_sub(1),
            },
        }
    }
    pub(crate) fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.region, bytes);
    }
    pub(crate) fn resize(&mut self, rows: usize) {
        self.region.rows = rows;
        self.region.reset_state();
    }
    pub(crate) fn reset_stream(&mut self) {
        self.parser = Processor::new();
    }
    pub(crate) fn bounds(&self) -> (usize, usize) {
        (self.region.top, self.region.bottom)
    }
}
