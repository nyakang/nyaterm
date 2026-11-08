//! Wire encoding shared by confirmed shell edits and suggestion replacement.

use gpui::{KeyDownEvent, KeyUpEvent, Keystroke, Modifiers};
use nyaterm_core::terminal::editing::EditPlan;
use nyaterm_terminal::navigation::{NavigationKey, navigation_key_bytes};
use nyaterm_terminal_gpui::{
    TerminalKeyMode, terminal_key_bytes_with_mode, terminal_key_release_bytes_with_mode,
};

pub(super) fn shell_edit_payload(
    plan: &EditPlan,
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
    let mut wire = shell_edit_key_bytes(if plan.move_right { "right" } else { "left" }, mode)
        .repeat(plan.move_steps);
    wire.extend(shell_edit_key_bytes("backspace", mode).repeat(plan.delete_steps));
    wire.extend(insertion_wire);
    logical.extend_from_slice(plan.insert_text.as_bytes());
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
