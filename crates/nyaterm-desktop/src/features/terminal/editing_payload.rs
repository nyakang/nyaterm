//! Wire encoding shared by confirmed shell edits and suggestion replacement.

use gpui::{KeyDownEvent, KeyUpEvent, Keystroke, Modifiers};
use nyaterm_core::terminal::editing::{EditPlan, ShellLineEditing};
use nyaterm_terminal::navigation::{NavigationKey, navigation_key_bytes};
use nyaterm_terminal_gpui::{
    TerminalKeyMode, terminal_key_bytes_with_mode, terminal_key_release_bytes_with_mode,
};

pub(super) fn shell_edit_payload(
    plan: &EditPlan,
    mode: TerminalKeyMode,
    insertion_wire: Vec<u8>,
) -> (Vec<u8>, Vec<u8>) {
    shell_edit_payload_with_backend(plan, ShellLineEditing::AnsiLineEditor, mode, insertion_wire)
}

/// Encode an edit for the line editor that actually owns the command line.
///
/// Legacy console hosts (conhost, used by CMD) run a cooked input reader that
/// ignores ANSI cursor motion and erases on `0x08`, not `0x7f`. Callers only
/// reach that branch for whole-line replacements with the cursor at the end, so
/// no cursor movement is needed.
pub(super) fn shell_edit_payload_with_backend(
    plan: &EditPlan,
    backend: ShellLineEditing,
    mode: TerminalKeyMode,
    insertion_wire: Vec<u8>,
) -> (Vec<u8>, Vec<u8>) {
    let key = if plan.move_right {
        NavigationKey::Right
    } else {
        NavigationKey::Left
    };
    // Logical input retains editing intents; the wire uses the same press and
    // release encoding as physical keys, including Kitty keyboard flags.
    let mut logical = navigation_key_bytes(key, mode.application_cursor).repeat(plan.move_steps);
    logical.extend(std::iter::repeat_n(0x7f, plan.delete_steps));
    logical.extend_from_slice(plan.insert_text.as_bytes());

    let mut wire = if backend.allows_cursor_motion() {
        let mut wire = shell_edit_key_bytes(if plan.move_right { "right" } else { "left" }, mode)
            .repeat(plan.move_steps);
        wire.extend(shell_edit_key_bytes("backspace", mode).repeat(plan.delete_steps));
        wire
    } else {
        debug_assert_eq!(
            plan.move_steps, 0,
            "legacy console edits must not need cursor motion"
        );
        std::iter::repeat_n(0x08, plan.delete_steps).collect()
    };
    wire.extend(insertion_wire);
    (wire, logical)
}

fn shell_edit_key_bytes(key: &str, mode: TerminalKeyMode) -> Vec<u8> {
    let keystroke = Keystroke {
        key: key.into(),
        key_char: None,
        modifiers: Modifiers::default(),
    };
    let mut bytes = terminal_key_bytes_with_mode(
        &KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        },
        mode,
    )
    .expect("shell editing keys have a terminal encoding");
    if let Some(release) = terminal_key_release_bytes_with_mode(&KeyUpEvent { keystroke }, mode) {
        bytes.extend(release);
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::shell_edit_payload_with_backend;
    use nyaterm_core::TerminalInputState;
    use nyaterm_core::terminal::editing::{EditIntent, EditPlan, ShellLineEditing, plan_edit};
    use nyaterm_terminal_gpui::TerminalKeyMode;

    fn whole_line_replacement() -> EditPlan {
        let mut input = TerminalInputState::new();
        input.value = "pi".to_string();
        input.cursor = 2;
        plan_edit(
            &input,
            1,
            EditIntent::Replace {
                range: 0..2,
                text: "ping 172.22.222.149",
            },
        )
        .expect("a same-line replacement plans")
    }

    #[test]
    fn legacy_console_replacement_erases_with_backspace_only() {
        let plan = whole_line_replacement();
        let (wire, _) = shell_edit_payload_with_backend(
            &plan,
            ShellLineEditing::LegacyConsole,
            TerminalKeyMode::default(),
            b"ping 172.22.222.149".to_vec(),
        );
        assert_eq!(
            wire,
            [
                0x08, 0x08, b'p', b'i', b'n', b'g', b' ', b'1', b'7', b'2', b'.', b'2', b'2', b'.',
                b'2', b'2', b'2', b'.', b'1', b'4', b'9'
            ]
        );
        assert!(!wire.contains(&0x1b));
        assert!(!wire.contains(&0x7f));
    }

    #[test]
    fn ansi_line_editors_keep_escape_and_delete_encoding() {
        let plan = whole_line_replacement();
        let (wire, _) = shell_edit_payload_with_backend(
            &plan,
            ShellLineEditing::AnsiLineEditor,
            TerminalKeyMode::default(),
            b"ping 172.22.222.149".to_vec(),
        );
        assert!(wire.windows(2).any(|pair| pair == [0x7f, 0x7f]));
        assert!(wire.ends_with(b"ping 172.22.222.149"));
    }
}
