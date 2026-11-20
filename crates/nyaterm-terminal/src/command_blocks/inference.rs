//! Bounded, revocable presentation inference over parsed terminal cells.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use nyaterm_core::command_starts_interactive_input;
use nyaterm_core::terminal::input_tracker::MAX_TRACKED_INPUT_BYTES;
use nyaterm_core::terminal::prompt::{
    match_prompt, prompts_match, single_line_command_is_complete,
};
use zeroize::{Zeroize, Zeroizing};

mod cache;

use cache::{CursorObservation, EchoObservation, InferenceCache};

use crate::command_blocks::{CommandBoundarySource, TrackedCommand};
use crate::command_navigation::CommandSubmissionEvidence;
use crate::{TerminalCore, cell_text_is_blank, render_cell_width};

#[cfg(test)]
mod tests;

pub(crate) struct InferredCommand {
    pub submission: CommandSubmissionEvidence,
    pub output: Option<i64>,
    pub return_prompt: Option<i64>,
    return_prompt_editing: bool,
    cache: InferenceCache,
    scratch: Zeroizing<String>,
}

impl std::fmt::Debug for InferredCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InferredCommand")
            .field("submission", &self.submission)
            .field("output", &self.output)
            .field("return_prompt", &self.return_prompt)
            .finish_non_exhaustive()
    }
}

impl InferredCommand {
    pub(crate) fn invalidate_line(&mut self, line: i64) {
        self.cache
            .invalidate_line(self.submission.anchor.logical_line, self.output, line);
    }

    pub(crate) fn invalidate_cache(&mut self) {
        self.cache = InferenceCache::default();
    }
}

impl TerminalCore {
    /// The desktop has already applied foreground-program and session policy.
    /// This records structure only, never shell-input or execution authorization.
    pub fn note_command_submission(&mut self, submission: CommandSubmissionEvidence) -> bool {
        let first = self.primary_lines.logical_line(self.term.topmost_line());
        let last = self.primary_lines.logical_line(self.term.bottommost_line());
        if !self.command_block_display_supported()
            || submission.anchor.epoch != self.primary_lines.epoch
            || !(first..=last).contains(&submission.anchor.logical_line)
        {
            return false;
        }
        if let Some(previous) = self.primary_lines.inference.take() {
            // A subsequent accepted submission also confirms a prompt that may
            // already contain typed text by the time the worker observes it.
            let matching = previous.output.is_some_and(|output| {
                submission.anchor.logical_line >= output
                    && submission.anchor.logical_line > previous.submission.anchor.logical_line
            }) && previous
                .submission
                .prompt
                .as_deref()
                .zip(submission.prompt.as_deref())
                .is_some_and(|(expected, actual)| prompts_match(expected, actual));
            if matching {
                self.close_inferred_command(
                    previous.submission.anchor.logical_line,
                    submission.anchor.logical_line,
                );
            } else {
                self.abandon_inferred_command(previous.submission.anchor.logical_line);
            }
        }
        self.record_fallback_command_at(submission.anchor);
        if submission.command.len() > MAX_TRACKED_INPUT_BYTES
            || submission.command.trim().is_empty()
            || submission.command.chars().any(char::is_control)
            || !single_line_command_is_complete(&submission.command)
            || command_starts_interactive_input(&submission.command)
            || submission
                .prompt
                .as_ref()
                .is_some_and(|prompt| prompt.len() > MAX_TRACKED_INPUT_BYTES)
            || submission
                .prompt
                .as_deref()
                .and_then(match_prompt)
                .is_none()
        {
            return true;
        }
        // Late submissions must not create a second block behind an OSC B.
        if self
            .primary_lines
            .command_blocks
            .entries
            .last()
            .is_none_or(|block| {
                block.start != submission.anchor.logical_line
                    || block.end_source == Some(CommandBoundarySource::ShellIntegration)
                    || block.output_source == Some(CommandBoundarySource::ShellIntegration)
            })
        {
            return true;
        }
        self.primary_lines.inference = Some(InferredCommand {
            submission,
            output: None,
            return_prompt: None,
            return_prompt_editing: false,
            cache: InferenceCache::default(),
            scratch: Zeroizing::new(String::new()),
        });
        self.update_command_inference();
        true
    }

    pub(crate) fn cancel_command_inference(&mut self) {
        if let Some(pending) = self.primary_lines.inference.take() {
            self.abandon_inferred_command(pending.submission.anchor.logical_line);
        }
    }

    /// OSC may keep the already confirmed echo boundary while supplying its own
    /// prompt/output/end boundary. Unsupported modes and contrary evidence may not.
    pub(crate) fn supersede_command_inference(&mut self) {
        if let Some(pending) = self.primary_lines.inference.take() {
            self.reopen_inferred_command(pending.submission.anchor.logical_line);
        }
    }

    /// Local keystrokes may extend a returned prompt without invalidating the
    /// preceding region. Program output alone cannot claim to be that editing.
    pub fn note_command_input_activity(&mut self) {
        if let Some(pending) = self.primary_lines.inference.as_mut()
            && pending.return_prompt.is_some()
        {
            pending.return_prompt_editing = true;
        }
    }

    fn inferred_block_mut(&mut self, start: i64) -> Option<&mut TrackedCommand> {
        let blocks = &mut self.primary_lines.command_blocks.entries;
        let index = blocks
            .binary_search_by_key(&start, |block| block.start)
            .ok()?;
        blocks.get_mut(index)
    }

    fn reopen_inferred_command(&mut self, start: i64) {
        if let Some(block) = self.inferred_block_mut(start)
            && block.end_source == Some(CommandBoundarySource::ConfirmedPrompt)
        {
            block.end = None;
            block.end_source = None;
            block.output_lines = 0;
            block.collapse_revision = block.collapse_revision.wrapping_add(1);
            let id = block.id;
            self.collapsed_commands.remove(&id);
        }
    }

    fn abandon_inferred_command(&mut self, start: i64) {
        self.reopen_inferred_command(start);
        if let Some(block) = self.inferred_block_mut(start)
            && block.output_source == Some(CommandBoundarySource::ConfirmedPrompt)
        {
            // A later navigation anchor alone must not turn rejected input or
            // interactive content into a foldable output range.
            block.output = None;
            block.output_source = None;
            block.output_lines = 0;
        }
    }

    fn close_inferred_command(&mut self, start: i64, end: i64) {
        let lines = &self.primary_lines.line_numbers;
        let count = self
            .primary_lines
            .command_blocks
            .entries
            .last()
            .filter(|block| block.start == start)
            .and_then(|block| block.output)
            .map_or(0, |output| {
                (lines.stable_coordinate(end) - lines.stable_coordinate(output)).max(0) as usize
            });
        if let Some(block) = self.inferred_block_mut(start)
            && block.end_source != Some(CommandBoundarySource::ShellIntegration)
        {
            block.end = Some(end);
            block.end_source = Some(CommandBoundarySource::ConfirmedPrompt);
            block.output_lines = count;
        }
    }

    /// Returns one bounded hard line, preserving soft-wrap spaces and skipping
    /// wide-character spacers. No history-wide snapshot or text scan is needed.
    fn inference_line(&mut self, start: i64, text: &mut String) -> Option<i64> {
        text.zeroize();
        let origin = self.primary_lines.logical_origin;
        let first = i64::from(self.term.topmost_line().0);
        let last = i64::from(self.term.bottommost_line().0);
        let mut row = start - origin;
        if row < first || row > last {
            return None;
        }
        let mut visited = 0;
        loop {
            for col in 0..self.cols {
                visited += 1;
                #[cfg(test)]
                {
                    self.inference_cells_visited += 1;
                }
                if visited > MAX_TRACKED_INPUT_BYTES {
                    return None;
                }
                let cell = &self.term.grid()[Line(row as i32)][Column(col)];
                if render_cell_width(cell) != 0 {
                    text.push(cell.c);
                    if let Some(marks) = cell.zerowidth() {
                        text.extend(marks.iter().copied());
                    }
                    if text.len() > MAX_TRACKED_INPUT_BYTES {
                        return None;
                    }
                }
            }
            if !self.term.grid()[Line(row as i32)][Column(self.cols - 1)]
                .flags
                .contains(Flags::WRAPLINE)
            {
                let length = text.trim_end_matches(' ').len();
                text.truncate(length);
                return Some(row + origin);
            }
            row += 1;
            if row > last {
                return None;
            }
        }
    }

    fn inference_cursor_line(&mut self, text: &mut String) -> Option<(i64, i64)> {
        let cursor = self.term.grid().cursor.point;
        let mut start = cursor.line;
        let first = self.term.topmost_line();
        let mut visited = 0;
        while start > first
            && self.term.grid()[start - 1usize][Column(self.cols - 1)]
                .flags
                .contains(Flags::WRAPLINE)
        {
            visited += 1;
            #[cfg(test)]
            {
                self.inference_cells_visited += 1;
            }
            if visited > MAX_TRACKED_INPUT_BYTES {
                return None;
            }
            start -= 1usize;
        }
        let start = self.primary_lines.logical_line(start);
        let end = self.inference_line(start, text)?;
        Some((start, end))
    }

    pub(crate) fn update_command_inference(&mut self) {
        let Some(mut pending) = self.primary_lines.inference.take() else {
            return;
        };
        let start = pending.submission.anchor.logical_line;
        if !self.logical_line_display_supported()
            || pending.submission.anchor.epoch != self.primary_lines.epoch
            || start < self.primary_lines.logical_line(self.term.topmost_line())
        {
            self.abandon_inferred_command(start);
            return;
        }
        if self
            .primary_lines
            .command_blocks
            .entries
            .last()
            .is_none_or(|block| {
                block.start != start
                    || block.output_source == Some(CommandBoundarySource::ShellIntegration)
                    || block.end_source == Some(CommandBoundarySource::ShellIntegration)
            })
        {
            return;
        }
        if !pending.cache.input_validated
            && let Some(output) = pending.output
            && (start..output).any(|line| {
                !self
                    .primary_lines
                    .metadata
                    .get(&line)
                    .is_some_and(|metadata| metadata.inferred_input)
            })
        {
            self.abandon_inferred_command(start);
            return;
        }
        pending.cache.input_validated = pending.output.is_some();
        if pending.output.is_none() {
            if pending.cache.echo.is_none() {
                let Some(end) = self.inference_line(start, &mut pending.scratch) else {
                    return;
                };
                let matched = match_prompt(&pending.scratch).is_some_and(|found| {
                    pending
                        .submission
                        .prompt
                        .as_deref()
                        .and_then(match_prompt)
                        .is_some_and(|expected| expected.same_shell(&found))
                        && pending.scratch[found.prefix.len()..].trim_end()
                            == pending.submission.command.trim_end()
                });
                pending.scratch.zeroize();
                pending.cache.echo = Some(EchoObservation { end, matched });
                if matched {
                    for row in start..=end {
                        self.primary_lines
                            .metadata
                            .entry(row)
                            .or_default()
                            .inferred_input = true;
                    }
                }
            }
            if let Some(EchoObservation { end, matched: true }) = pending.cache.echo {
                let cursor = self
                    .primary_lines
                    .logical_line(self.term.grid().cursor.point.line);
                if cursor > end {
                    let output = end + 1;
                    if let Some(block) = self.inferred_block_mut(start) {
                        block.output = Some(output);
                        block.output_source = Some(CommandBoundarySource::ConfirmedPrompt);
                    }
                    pending.output = Some(output);
                    pending.cache.input_validated = true;
                }
            }
        }
        if let Some(output) = pending.output {
            // A continuation prompt is input, even when the original Enter
            // looked like a single-line submission (for example `for x in a b`).
            if !pending.cache.continuation_checked {
                let row = Line((output - self.primary_lines.logical_origin) as i32);
                for col in 0..self.cols.min(32) {
                    #[cfg(test)]
                    {
                        self.inference_cells_visited += 1;
                    }
                    let cell = &self.term.grid()[row][Column(col)];
                    if render_cell_width(cell) != 0 {
                        pending.scratch.push(cell.c);
                    }
                }
                let prefix = pending.scratch.trim_start();
                let continuation = prefix.starts_with('>')
                    || prefix.starts_with("... ")
                    || prefix.starts_with("quote>")
                    || prefix.starts_with("dquote>")
                    || prefix.starts_with("heredoc>");
                pending.scratch.zeroize();
                if continuation {
                    self.abandon_inferred_command(start);
                    return;
                }
                pending.cache.continuation_checked = true;
            }
            let cursor = self.term.grid().cursor.point;
            let cursor_line = self.primary_lines.logical_line(cursor.line);
            if pending
                .cache
                .cursor
                .is_none_or(|cached| !(cached.start..=cached.end).contains(&cursor_line))
            {
                pending.cache.cursor =
                    self.inference_cursor_line(&mut pending.scratch)
                        .map(|(start, end)| {
                            let found = match_prompt(&pending.scratch);
                            let matching_prompt = found.as_ref().is_some_and(|found| {
                                pending
                                    .submission
                                    .prompt
                                    .as_deref()
                                    .and_then(match_prompt)
                                    .is_some_and(|expected| expected.same_shell(found))
                            });
                            let empty_suffix = found.is_some_and(|found| {
                                pending.scratch[found.prefix.len()..].trim().is_empty()
                            });
                            CursorObservation {
                                start,
                                end,
                                matching_prompt,
                                empty_suffix,
                            }
                        });
                pending.scratch.zeroize();
            }
            if let Some(observed) = pending.cache.cursor {
                let line = observed.start;
                if pending
                    .return_prompt
                    .is_some_and(|previous| previous != line)
                {
                    self.reopen_inferred_command(start);
                    pending.return_prompt = None;
                    pending.return_prompt_editing = false;
                }
                // Cursor-only movement does not change line signatures, but
                // still has to revoke a provisional prompt boundary.
                let editing = pending.return_prompt == Some(line) && pending.return_prompt_editing;
                let at_text_end = observed.matching_prompt
                    && observed.empty_suffix
                    && !editing
                    && (cursor.column.0..self.cols).all(|col| {
                        #[cfg(test)]
                        {
                            self.inference_cells_visited += 1;
                        }
                        cell_text_is_blank(&self.term.grid()[cursor.line][Column(col)])
                    });
                let matching =
                    line >= output && observed.matching_prompt && (editing || at_text_end);
                if matching {
                    self.close_inferred_command(start, line);
                    pending.return_prompt = Some(line);
                } else {
                    self.reopen_inferred_command(start);
                    pending.return_prompt = None;
                    pending.return_prompt_editing = false;
                }
            } else {
                self.reopen_inferred_command(start);
                pending.return_prompt = None;
                pending.return_prompt_editing = false;
            }
        }
        self.primary_lines.inference = Some(pending);
    }
}
