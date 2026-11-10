//! Shell editing plans. Terminal cells provide evidence; this model never owns a grid.

use std::ops::Range;
use std::time::{Duration, Instant};

use unicode_segmentation::UnicodeSegmentation;
use zeroize::Zeroize;

use super::input_tracker::{
    MAX_TRACKED_INPUT_BYTES, TerminalInputState, apply_terminal_input_data_in_place,
};

pub const EDIT_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EditCapability {
    #[default]
    None,
    MarkedShell,
    EchoMatched,
}

impl EditCapability {
    /// Echo alone is not evidence that a foreground program has a line editor.
    pub fn allows_editing(self, echo_line_editor: bool) -> bool {
        match self {
            Self::None => false,
            Self::MarkedShell => true,
            Self::EchoMatched => echo_line_editor,
        }
    }
}

/// Which line editor owns the command line, and therefore which wire encoding
/// can safely move over or delete text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShellLineEditing {
    /// ANSI-capable line editors (readline, zle, PSReadLine): arrow keys move
    /// the cursor and `0x7f` erases one character.
    #[default]
    AnsiLineEditor,
    /// Windows conhost cooked input: ANSI cursor motion is ignored and `0x08`
    /// is the erase character, so replacements must stay at the end of the line.
    LegacyConsole,
    /// A shell that published an OSC 133 input anchor; echo evidence is exact.
    Marked,
}

impl ShellLineEditing {
    pub fn allows_cursor_motion(self) -> bool {
        self != Self::LegacyConsole
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EditPhase {
    Ready,
    Pending,
    #[default]
    Uncertain,
}

pub enum EditIntent<'a> {
    Move(usize),
    Delete(Range<usize>),
    Replace { range: Range<usize>, text: &'a str },
}

/// A prediction, not an acknowledgement from the remote line editor.
pub struct EditPlan {
    pub input_version: u64,
    pub move_right: bool,
    pub move_steps: usize,
    pub delete_range: Option<Range<usize>>,
    pub delete_steps: usize,
    pub insert_text: String,
    pub predicted: TerminalInputState,
}

/// Complex graphemes have no portable relationship to shell arrow/erase steps.
pub fn supports_scalar_editing(value: &str) -> bool {
    !value.chars().any(char::is_control)
        && value
            .graphemes(true)
            .all(|grapheme| grapheme.chars().count() == 1)
}

pub fn plan_edit(
    input: &TerminalInputState,
    version: u64,
    intent: EditIntent<'_>,
) -> Option<EditPlan> {
    if input.desynced
        || input.line_rewrite_required
        || input.multiline
        || input.paste_mode
        || !supports_scalar_editing(&input.value)
        || !input.value.is_char_boundary(input.cursor)
    {
        return None;
    }
    let (target, range, text) = match intent {
        EditIntent::Move(target) => (target, None, ""),
        EditIntent::Delete(range) => (range.end, Some(range), ""),
        EditIntent::Replace { range, text } => (range.end, Some(range), text),
    };
    if target > input.value.len() || !input.value.is_char_boundary(target) {
        return None;
    }
    let mut predicted = input.clone();
    let mut delete_steps = 0;
    if let Some(range) = &range {
        if range.start >= range.end
            || range.end > input.value.len()
            || !input.value.is_char_boundary(range.start)
            || !input.value.is_char_boundary(range.end)
            || input.value.len() - (range.end - range.start) + text.len() > MAX_TRACKED_INPUT_BYTES
            || text
                .chars()
                .any(|ch| ch.is_control() && !matches!(ch, '\r' | '\n'))
        {
            return None;
        }
        delete_steps = input.value[range.clone()].chars().count();
        predicted.value.replace_range(range.clone(), text);
        predicted.cursor = range.start + text.len();
        predicted.multiline = predicted.value.contains(['\r', '\n']);
    } else {
        predicted.cursor = target;
    }
    let current = input.value[..input.cursor].chars().count();
    let target = input.value[..target].chars().count();
    Some(EditPlan {
        input_version: version,
        move_right: target > current,
        move_steps: current.abs_diff(target),
        delete_range: range,
        delete_steps,
        insert_text: text.to_string(),
        predicted,
    })
}

/// One authoritative predicted input per session. Checkpoints are immutable plans,
/// and must never become an independently edited second command buffer.
#[derive(Default)]
pub struct ShellEditState {
    pub input: TerminalInputState,
    pub version: u64,
    pub phase: EditPhase,
    pub queued_cursor: Option<usize>,
    pub last_input_at: Option<Instant>,
}

impl ShellEditState {
    /// Suggestions can use an ordinary typing prediction, but never stale input
    /// after a timeout, session switch, or other loss of confidence.
    pub fn assist_input(&self) -> Option<&TerminalInputState> {
        (self.phase != EditPhase::Uncertain && !self.input.desynced).then_some(&self.input)
    }

    pub fn clear_input(&mut self) {
        self.input.value.zeroize();
        self.input = TerminalInputState::new();
        self.invalidate();
    }

    pub fn deactivate(&mut self, editable: bool) {
        if self.phase != EditPhase::Ready || !editable {
            self.clear_input();
        } else {
            self.invalidate();
        }
    }

    pub fn note_input(&mut self, text: &str, now: Instant) {
        apply_terminal_input_data_in_place(&mut self.input, text);
        self.version = self.version.wrapping_add(1);
        self.last_input_at = Some(now);
        self.queued_cursor = None;
        self.phase = if self.input.desynced
            || self.input.line_rewrite_required
            || self.input.multiline
            || self.input.paste_mode
            || self.input.value.is_empty()
        {
            EditPhase::Uncertain
        } else {
            EditPhase::Pending
        };
    }

    pub fn accept_plan(&mut self, plan: EditPlan, now: Instant) -> bool {
        if self.phase != EditPhase::Ready || plan.input_version != self.version {
            return false;
        }
        self.input.value.zeroize();
        self.input = plan.predicted;
        self.version = self.version.wrapping_add(1);
        self.phase = EditPhase::Pending;
        self.last_input_at = Some(now);
        self.queued_cursor = None;
        true
    }

    pub fn confirm(&mut self) -> Option<usize> {
        self.phase = EditPhase::Ready;
        self.last_input_at = None;
        self.queued_cursor.take()
    }

    pub fn invalidate(&mut self) {
        self.phase = EditPhase::Uncertain;
        self.version = self.version.wrapping_add(1);
        self.queued_cursor = None;
        self.last_input_at = None;
    }

    pub fn expire(&mut self, now: Instant) -> bool {
        if self.phase == EditPhase::Pending
            && self.last_input_at.is_some_and(|last| {
                now.saturating_duration_since(last) >= EDIT_CONFIRMATION_TIMEOUT
            })
        {
            self.invalidate();
            true
        } else {
            false
        }
    }
}

impl Drop for ShellEditState {
    fn drop(&mut self) {
        self.input.value.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EDIT_CONFIRMATION_TIMEOUT, EditCapability, EditIntent, EditPhase, ShellEditState,
        plan_edit, supports_scalar_editing,
    };
    use crate::terminal::input_tracker::TerminalInputState;
    use std::time::Instant;

    #[test]
    fn replacing_wide_text_counts_editor_steps_not_cells() {
        let input = TerminalInputState {
            value: "a界b ".into(),
            cursor: 1,
            ..TerminalInputState::default()
        };
        let plan = plan_edit(
            &input,
            7,
            EditIntent::Replace {
                range: 1..4,
                text: "猫",
            },
        )
        .unwrap();
        assert_eq!(plan.move_steps, 1);
        assert!(plan.move_right);
        assert_eq!(plan.delete_steps, 1);
        assert_eq!(plan.predicted.value, "a猫b ");
        assert_eq!(plan.predicted.cursor, 4);
    }

    #[test]
    fn complex_graphemes_and_invalid_byte_ranges_do_not_produce_edits() {
        assert!(supports_scalar_editing("echo 中文 "));
        for value in ["e\u{301}", "👩\u{200d}💻", "🇨🇳"] {
            assert!(!supports_scalar_editing(value));
        }
        let input = TerminalInputState {
            value: "界".into(),
            cursor: 3,
            ..TerminalInputState::default()
        };
        assert!(plan_edit(&input, 0, EditIntent::Delete(1..3)).is_none());
        assert!(plan_edit(&input, 0, EditIntent::Move(1)).is_none());
    }

    #[test]
    fn pending_edits_wait_for_echo_and_expire_without_replay() {
        let now = Instant::now();
        let mut state = ShellEditState::default();
        state.note_input("abc", now);
        assert_eq!(state.phase, EditPhase::Pending);
        state.confirm();
        let plan = plan_edit(&state.input, state.version, EditIntent::Move(1)).unwrap();
        assert!(state.accept_plan(plan, now));
        state.queued_cursor = Some(0);
        assert!(!state.expire(now));
        assert!(state.expire(now + EDIT_CONFIRMATION_TIMEOUT));
        assert_eq!(state.phase, EditPhase::Uncertain);
        assert_eq!(state.queued_cursor, None);
        assert_eq!(state.input.cursor, 1);
    }

    #[test]
    fn ordinary_typing_updates_pending_prediction_once() {
        let mut state = ShellEditState::default();
        state.note_input("ab", Instant::now());
        state.note_input("c", Instant::now());
        assert_eq!(state.input.value, "abc");
        assert_eq!(state.version, 2);
        state.queued_cursor = Some(1);
        assert_eq!(state.confirm(), Some(1));
    }

    #[test]
    fn echo_requires_line_editor_context_for_every_edit() {
        assert!(!EditCapability::None.allows_editing(true));
        assert!(EditCapability::MarkedShell.allows_editing(false));
        assert!(!EditCapability::EchoMatched.allows_editing(false));
        assert!(EditCapability::EchoMatched.allows_editing(true));
    }

    #[test]
    fn deactivation_discards_unconfirmed_and_noneditable_input() {
        for (phase, editable, retained) in [
            (EditPhase::Pending, true, false),
            (EditPhase::Uncertain, true, false),
            (EditPhase::Ready, false, false),
            (EditPhase::Ready, true, true),
        ] {
            let mut state = ShellEditState::default();
            state.note_input("synthetic input", Instant::now());
            state.phase = phase;
            state.queued_cursor = Some(1);
            state.deactivate(editable);
            assert_eq!(!state.input.value.is_empty(), retained);
            assert_eq!(state.phase, EditPhase::Uncertain);
            assert!(state.queued_cursor.is_none());
            assert!(state.assist_input().is_none());
        }
    }

    #[test]
    fn suggestions_can_use_typing_prediction_but_not_expired_input() {
        let mut state = ShellEditState::default();
        let now = Instant::now();
        assert!(state.assist_input().is_none());
        state.note_input("git", now);
        assert_eq!(state.assist_input().unwrap().value, "git");
        state.expire(now + EDIT_CONFIRMATION_TIMEOUT);
        assert!(state.assist_input().is_none());
        state.confirm();
        assert_eq!(state.assist_input().unwrap().value, "git");
    }

    #[test]
    fn tracked_input_debug_redacts_unrecognized_sensitive_input() {
        let input = TerminalInputState {
            value: "synthetic-secret-for-test".into(),
            ..TerminalInputState::default()
        };
        let debug = format!("{input:?}");
        assert!(!debug.contains(&input.value));
        assert!(debug.contains("[REDACTED]"));
    }
}
