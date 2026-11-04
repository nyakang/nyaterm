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

    /// Absolute number of a logical line, if numbering has an origin.
    ///
    /// Never falls back to the current topmost line: rebasing onto the retained
    /// window made the gutter restart at `scrollback_limit + rows` (10097) and,
    /// after a later coordinate rewrite, jump back to `1` while content kept
    /// incrementing. An unanchored screen reports `None` and the host falls back
    /// to its own absolute row counter.
    pub(crate) fn number(&self, line: i64) -> Option<usize> {
        let origin = self.origin?;
        usize::try_from(self.stable_coordinate(line) - origin + 1)
            .ok()
            .filter(|number| *number > 0)
    }

    /// Pinned origin coordinate, when numbering has been anchored.
    pub(crate) fn base(&self) -> Option<i64> {
        self.origin
    }

    pub(crate) fn start_at(&mut self, line: i64) {
        self.restore_number(line, 1);
    }

    /// Anchor numbering to the first hard line of the session.
    ///
    /// Logical line zero is the session's first row, so a coordinate origin of
    /// zero keeps the absolute line number of every later row; scrollback
    /// pruning does not move retained rows, so the origin must not follow the
    /// retained window. Coordinate rewrites (width reflow, row-count resize)
    /// re-anchor through [`Self::restore_number`] instead.
    pub(crate) fn ensure_origin(&mut self) {
        if self.origin.is_none() {
            self.origin = Some(0);
        }
    }

    /// Replace the wrap index with an ascending scan of the retained grid.
    ///
    /// Initialization visits rows in ascending logical order, so appending
    /// beats a binary search per row; the index always describes the current
    /// window, which starts with no discarded wraps.
    pub(crate) fn rebuild(&mut self, wraps: Vec<i64>) {
        self.wraps = wraps.into();
        self.discarded_wraps = 0;
    }

    pub(crate) fn restore_number(&mut self, line: i64, number: usize) {
        self.origin = Some(self.stable_coordinate(line) - number.saturating_sub(1) as i64);
    }

    pub(crate) fn retain_from(&mut self, first: i64) {
        let discarded = self.wraps.partition_point(|end| *end < first);
        self.discarded_wraps += discarded as i64;
        self.wraps.drain(..discarded);
    }

    /// Number of `line` using only the pinned origin.
    pub(crate) fn pinned_number(&self, line: i64) -> Option<usize> {
        self.number(line)
    }
}
