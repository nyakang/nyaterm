//! Parsed observations invalidated by the terminal's existing line revisions.

#[derive(Debug, Clone, Copy)]
pub(super) struct EchoObservation {
    pub end: i64,
    pub matched: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct CursorObservation {
    pub start: i64,
    pub end: i64,
    pub matching_prompt: bool,
    pub empty_suffix: bool,
}

#[derive(Debug, Default)]
pub(super) struct InferenceCache {
    pub(super) echo: Option<EchoObservation>,
    pub(super) cursor: Option<CursorObservation>,
    pub(super) continuation_checked: bool,
    pub(super) input_validated: bool,
}

impl InferenceCache {
    pub(super) fn invalidate_line(&mut self, start: i64, output: Option<i64>, line: i64) {
        if self
            .echo
            .is_some_and(|echo| (start..=echo.end).contains(&line))
        {
            self.echo = None;
        }
        if self.cursor.is_some_and(|cursor| {
            // A changed wrap on the preceding row may join the cursor's chain.
            (cursor.start.saturating_sub(1)..=cursor.end).contains(&line)
        }) {
            self.cursor = None;
        }
        if let Some(output) = output {
            if (start..output).contains(&line) {
                self.input_validated = false;
            }
            if line == output {
                self.continuation_checked = false;
            }
        }
    }
}
