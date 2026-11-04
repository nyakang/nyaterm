use std::collections::VecDeque;

/// Index wrap endings in stable grid coordinates. Snapshot construction only
/// needs binary searches, even with a large scrollback buffer.
#[derive(Debug, Default)]
pub(crate) struct LineNumbers {
    wraps: VecDeque<i64>,
    // Keep numbering after a local clear stable as old wraps leave history.
    discarded_wraps: i64,
    // Coordinate of logical line one after a local clear, including reflow.
    origin: Option<i64>,
    pub(crate) initialized: bool,
}

impl LineNumbers {
    pub(crate) fn update(&mut self, line: i64, wraps: bool) {
        match (self.wraps.binary_search(&line), wraps) {
            (Err(index), true) => self.wraps.insert(index, line),
            (Ok(index), false) => {
                self.wraps.remove(index);
            }
            _ => {}
        }
    }

    pub(crate) fn stable_coordinate(&self, line: i64) -> i64 {
        line - self.discarded_wraps - self.wraps.partition_point(|end| *end < line) as i64
    }

    pub(crate) fn number(&self, line: i64, topmost: i64) -> Option<usize> {
        let origin = self
            .origin
            .unwrap_or_else(|| self.stable_coordinate(topmost));
        usize::try_from(self.stable_coordinate(line) - origin + 1)
            .ok()
            .filter(|number| *number > 0)
    }

    pub(crate) fn start_at(&mut self, line: i64) {
        self.restore_number(line, 1);
    }

    pub(crate) fn restore_number(&mut self, line: i64, number: usize) {
        self.origin = Some(self.stable_coordinate(line) - number.saturating_sub(1) as i64);
    }

    pub(crate) fn retain_from(&mut self, first: i64) {
        let discarded = self.wraps.partition_point(|end| *end < first);
        self.discarded_wraps += discarded as i64;
        self.wraps.drain(..discarded);
    }
}
