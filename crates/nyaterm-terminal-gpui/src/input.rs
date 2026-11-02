use gpui::{KeyDownEvent, KeyUpEvent};
use nyaterm_terminal::TerminalScreen;
use nyaterm_terminal::navigation::{NavigationKey, navigation_key_bytes};

/// Terminal keyboard mode flags that affect encoding of plain navigation keys.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalKeyMode {
    /// DECCKM application cursor keys (DECSET 1).
    pub application_cursor: bool,
    /// DECKPAM application keypad (ESC =).
    pub application_keypad: bool,
    /// Kitty keyboard protocol: disambiguate Esc/Ctrl/Alt text keys with CSI-u.
    pub kitty_keyboard_disambiguate: bool,
    /// Kitty keyboard protocol: append press/repeat event type to CSI-u keys.
    pub kitty_keyboard_report_event_types: bool,
    /// Kitty keyboard protocol: include shifted/base key alternatives in CSI-u.
    pub kitty_keyboard_report_alternate_keys: bool,
    /// Kitty keyboard protocol: report printable keys as CSI-u, even without modifiers.
    pub kitty_keyboard_report_all_keys_as_esc: bool,
    /// Kitty keyboard protocol: include generated text in CSI-u.
    pub kitty_keyboard_report_associated_text: bool,
}

pub fn terminal_key_bytes(event: &KeyDownEvent) -> Option<Vec<u8>> {
    terminal_key_bytes_with_mode(event, TerminalKeyMode::default())
}

pub fn terminal_key_bytes_with_mode(
    event: &KeyDownEvent,
    mode: TerminalKeyMode,
) -> Option<Vec<u8>> {
    let keystroke = &event.keystroke;
    if keystroke.modifiers.function {
        return None;
    }
    // Super/Win key combos are reserved for the shell/OS.
    if keystroke.modifiers.platform && !keystroke.modifiers.control && !keystroke.modifiers.alt {
        return None;
    }

    let key = keystroke.key.as_str();
    let ctrl = keystroke.modifiers.control;
    let alt = keystroke.modifiers.alt;
    let shift = keystroke.modifiers.shift;

    if let Some(bytes) = modified_navigation_key_bytes(key, shift, alt, ctrl) {
        return Some(bytes);
    }

    if (mode.kitty_keyboard_disambiguate || mode.kitty_keyboard_report_all_keys_as_esc)
        && let Some(bytes) =
            kitty_key_bytes(&event.keystroke, mode, KittyKeyEvent::from_key_down(event))
    {
        return Some(bytes);
    }

    if mode.application_keypad
        && !ctrl
        && !alt
        && !shift
        && !keystroke.modifiers.platform
        && let Some(bytes) = application_keypad_bytes(key, keystroke.key_char.as_deref())
    {
        return Some(bytes);
    }

    // Alt+Backspace: ESC DEL for shell/readline delete-word-backward.
    if alt && !ctrl && !keystroke.modifiers.platform && key == "backspace" {
        return Some(vec![0x1b, 0x7f]);
    }

    if ctrl && !alt {
        return control_key_bytes(key);
    }

    // Plain navigation / editing keys (no modifiers other than shift where irrelevant).
    if !ctrl && !alt && !keystroke.modifiers.platform {
        if shift && key == "tab" {
            return Some(b"\x1b[Z".to_vec());
        }
        match key {
            "enter" => return Some(b"\r".to_vec()),
            "backspace" => return Some(vec![0x7f]),
            "tab" => return Some(b"\t".to_vec()),
            "space" => return Some(b" ".to_vec()),
            "escape" => return Some(vec![0x1b]),
            "up" => {
                return Some(
                    navigation_key_bytes(NavigationKey::Up, mode.application_cursor).to_vec(),
                );
            }
            "down" => {
                return Some(
                    navigation_key_bytes(NavigationKey::Down, mode.application_cursor).to_vec(),
                );
            }
            "right" => {
                return Some(
                    navigation_key_bytes(NavigationKey::Right, mode.application_cursor).to_vec(),
                );
            }
            "left" => {
                return Some(
                    navigation_key_bytes(NavigationKey::Left, mode.application_cursor).to_vec(),
                );
            }
            "home" => {
                return Some(
                    navigation_key_bytes(NavigationKey::Home, mode.application_cursor).to_vec(),
                );
            }
            "end" => {
                return Some(
                    navigation_key_bytes(NavigationKey::End, mode.application_cursor).to_vec(),
                );
            }
            "insert" => return Some(b"\x1b[2~".to_vec()),
            "delete" => return Some(b"\x1b[3~".to_vec()),
            "pageup" => return Some(b"\x1b[5~".to_vec()),
            "pagedown" => return Some(b"\x1b[6~".to_vec()),
            _ => {}
        }

        if let Some(bytes) = function_key_bytes(key) {
            return Some(bytes);
        }

        return keystroke
            .key_char
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(|value| value.as_bytes().to_vec());
    }

    None
}

pub fn terminal_key_release_bytes_with_mode(
    event: &KeyUpEvent,
    mode: TerminalKeyMode,
) -> Option<Vec<u8>> {
    if !mode.kitty_keyboard_report_event_types {
        return None;
    }
    if !(mode.kitty_keyboard_disambiguate || mode.kitty_keyboard_report_all_keys_as_esc) {
        return None;
    }
    if event.keystroke.modifiers.function {
        return None;
    }
    if event.keystroke.modifiers.platform
        && !event.keystroke.modifiers.control
        && !event.keystroke.modifiers.alt
    {
        return None;
    }

    kitty_key_bytes(&event.keystroke, mode, KittyKeyEvent::Release)
}

fn modified_navigation_key_bytes(key: &str, shift: bool, alt: bool, ctrl: bool) -> Option<Vec<u8>> {
    let modifier = csi_modifier(shift, alt, ctrl)?;
    let final_byte = match key {
        "up" => 'A',
        "down" => 'B',
        "right" => 'C',
        "left" => 'D',
        "home" => 'H',
        "end" => 'F',
        "f1" => 'P',
        "f2" => 'Q',
        "f3" => 'R',
        "f4" => 'S',
        _ => return modified_tilde_key_bytes(key, modifier),
    };
    Some(format!("\x1b[1;{modifier}{final_byte}").into_bytes())
}

fn modified_tilde_key_bytes(key: &str, modifier: u8) -> Option<Vec<u8>> {
    let number = match key {
        "insert" => 2,
        "delete" => 3,
        "pageup" => 5,
        "pagedown" => 6,
        "f5" => 15,
        "f6" => 17,
        "f7" => 18,
        "f8" => 19,
        "f9" => 20,
        "f10" => 21,
        "f11" => 23,
        "f12" => 24,
        _ => return None,
    };
    Some(format!("\x1b[{number};{modifier}~").into_bytes())
}

fn csi_modifier(shift: bool, alt: bool, ctrl: bool) -> Option<u8> {
    let mut modifier = 1u8;
    if shift {
        modifier += 1;
    }
    if alt {
        modifier += 2;
    }
    if ctrl {
        modifier += 4;
    }
    (modifier > 1).then_some(modifier)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KittyKeyEvent {
    Press,
    Repeat,
    Release,
}

impl KittyKeyEvent {
    fn from_key_down(event: &KeyDownEvent) -> Self {
        if event.is_held {
            Self::Repeat
        } else {
            Self::Press
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::Press => 1,
            Self::Repeat => 2,
            Self::Release => 3,
        }
    }
}

fn kitty_key_bytes(
    keystroke: &gpui::Keystroke,
    mode: TerminalKeyMode,
    event: KittyKeyEvent,
) -> Option<Vec<u8>> {
    let key = keystroke.key.as_str();
    let ctrl = keystroke.modifiers.control;
    let alt = keystroke.modifiers.alt;
    let shift = keystroke.modifiers.shift;
    let codepoint = match key {
        "escape" => Some(27),
        "enter" => Some(13),
        "tab" => Some(9),
        "backspace" => Some(127),
        _ => single_codepoint(key)
            .or_else(|| keystroke.key_char.as_deref().and_then(single_codepoint)),
    }?;

    let named_ambiguous = matches!(key, "escape" | "enter" | "tab" | "backspace");
    if event == KittyKeyEvent::Press
        && !mode.kitty_keyboard_report_all_keys_as_esc
        && !named_ambiguous
        && !alt
        && !ctrl
    {
        return None;
    }

    let modifier = csi_modifier(shift, alt, ctrl).unwrap_or(1);
    let event_type = mode
        .kitty_keyboard_report_event_types
        .then_some(event.code());
    let associated_text = if mode.kitty_keyboard_report_associated_text
        && mode.kitty_keyboard_report_all_keys_as_esc
        && event == KittyKeyEvent::Press
        && !ctrl
    {
        keystroke
            .key_char
            .as_deref()
            .and_then(kitty_text_codepoints)
    } else {
        None
    };

    let mut key_param = codepoint.to_string();
    if mode.kitty_keyboard_report_alternate_keys
        && let Some(alternate) = kitty_alternate_key_param(keystroke, codepoint, key, shift)
    {
        key_param.push_str(&alternate);
    }

    let mut modifier_param = modifier.to_string();
    if let Some(event_type) = event_type {
        modifier_param.push(':');
        modifier_param.push_str(&event_type.to_string());
    }

    if modifier == 1
        && event_type.is_none()
        && associated_text.is_none()
        && !key_param.contains(':')
    {
        Some(format!("\x1b[{key_param}u").into_bytes())
    } else if let Some(associated_text) = associated_text {
        Some(format!("\x1b[{key_param};{modifier_param};{associated_text}u").into_bytes())
    } else {
        Some(format!("\x1b[{key_param};{modifier_param}u").into_bytes())
    }
}

fn single_codepoint(value: &str) -> Option<u32> {
    let mut chars = value.chars();
    let ch = chars.next()?;
    chars.next().is_none().then_some(ch as u32)
}

fn kitty_alternate_key_param(
    keystroke: &gpui::Keystroke,
    codepoint: u32,
    key: &str,
    shift: bool,
) -> Option<String> {
    if matches!(key, "escape" | "enter" | "tab" | "backspace") {
        return None;
    }

    let shifted = shift
        .then(|| keystroke.key_char.as_deref().and_then(single_codepoint))
        .flatten()
        .filter(|value| *value != codepoint);
    let base = single_codepoint(key).filter(|value| *value != codepoint || shifted.is_some());

    if shifted.is_none() && base.is_none() {
        return None;
    }

    Some(format!(
        ":{}:{}",
        shifted.map(|value| value.to_string()).unwrap_or_default(),
        base.map(|value| value.to_string()).unwrap_or_default()
    ))
}

fn kitty_text_codepoints(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    let mut encoded = String::new();
    for ch in value.chars() {
        if ch.is_control() {
            return None;
        }
        if !encoded.is_empty() {
            encoded.push(':');
        }
        encoded.push_str(&(ch as u32).to_string());
    }
    Some(encoded)
}

fn function_key_bytes(key: &str) -> Option<Vec<u8>> {
    let bytes: &[u8] = match key {
        "f1" => b"\x1bOP",
        "f2" => b"\x1bOQ",
        "f3" => b"\x1bOR",
        "f4" => b"\x1bOS",
        "f5" => b"\x1b[15~",
        "f6" => b"\x1b[17~",
        "f7" => b"\x1b[18~",
        "f8" => b"\x1b[19~",
        "f9" => b"\x1b[20~",
        "f10" => b"\x1b[21~",
        "f11" => b"\x1b[23~",
        "f12" => b"\x1b[24~",
        _ => return None,
    };
    Some(bytes.to_vec())
}

/// Application keypad (DECKPAM) SS3 encodings for numeric keypad keys.
fn application_keypad_bytes(key: &str, key_char: Option<&str>) -> Option<Vec<u8>> {
    // Prefer explicit keypad key names when the platform provides them.
    let from_key = match key {
        "numpad0" | "kp_0" => Some(b"\x1bOp".as_slice()),
        "numpad1" | "kp_1" => Some(b"\x1bOq".as_slice()),
        "numpad2" | "kp_2" => Some(b"\x1bOr".as_slice()),
        "numpad3" | "kp_3" => Some(b"\x1bOs".as_slice()),
        "numpad4" | "kp_4" => Some(b"\x1bOt".as_slice()),
        "numpad5" | "kp_5" => Some(b"\x1bOu".as_slice()),
        "numpad6" | "kp_6" => Some(b"\x1bOv".as_slice()),
        "numpad7" | "kp_7" => Some(b"\x1bOw".as_slice()),
        "numpad8" | "kp_8" => Some(b"\x1bOx".as_slice()),
        "numpad9" | "kp_9" => Some(b"\x1bOy".as_slice()),
        "numpad_decimal" | "kp_decimal" | "numpad_dot" => Some(b"\x1bOn".as_slice()),
        "numpad_enter" | "kp_enter" => Some(b"\x1bOM".as_slice()),
        "numpad_add" | "kp_add" | "numpad_plus" => Some(b"\x1bOk".as_slice()),
        "numpad_subtract" | "kp_subtract" | "numpad_minus" => Some(b"\x1bOm".as_slice()),
        "numpad_multiply" | "kp_multiply" => Some(b"\x1bOj".as_slice()),
        "numpad_divide" | "kp_divide" => Some(b"\x1bOo".as_slice()),
        "numpad_comma" | "kp_comma" => Some(b"\x1bOl".as_slice()),
        _ => None,
    };
    if let Some(bytes) = from_key {
        return Some(bytes.to_vec());
    }
    // Fallback: when only key_char is available (some platforms alias numpad to
    // main-row keys), map the keypad-produced character set.
    match key_char {
        Some("0") if key == "0" => None, // ambiguous main-row digit; skip
        Some(ch) if ch.len() == 1 && key.starts_with("numpad") => match ch.chars().next()? {
            '0' => Some(b"\x1bOp".to_vec()),
            '1' => Some(b"\x1bOq".to_vec()),
            '2' => Some(b"\x1bOr".to_vec()),
            '3' => Some(b"\x1bOs".to_vec()),
            '4' => Some(b"\x1bOt".to_vec()),
            '5' => Some(b"\x1bOu".to_vec()),
            '6' => Some(b"\x1bOv".to_vec()),
            '7' => Some(b"\x1bOw".to_vec()),
            '8' => Some(b"\x1bOx".to_vec()),
            '9' => Some(b"\x1bOy".to_vec()),
            '.' => Some(b"\x1bOn".to_vec()),
            '+' => Some(b"\x1bOk".to_vec()),
            '-' => Some(b"\x1bOm".to_vec()),
            '*' => Some(b"\x1bOj".to_vec()),
            '/' => Some(b"\x1bOo".to_vec()),
            ',' => Some(b"\x1bOl".to_vec()),
            _ => None,
        },
        _ => None,
    }
}

pub(super) fn control_key_bytes(key: &str) -> Option<Vec<u8>> {
    // Ctrl+Arrow handled above.
    if matches!(key, "up" | "down" | "left" | "right") {
        return None;
    }
    let byte = match key {
        // Plain Backspace is DEL (0x7f); Ctrl+Backspace is BS (0x08), matching xterm.
        "backspace" => 0x08,
        "space" => 0x00,
        "left_bracket" | "[" => 0x1b,
        "backslash" | "\\" => 0x1c,
        "right_bracket" | "]" => 0x1d,
        "6" => 0x1e,
        // Ctrl+- and Ctrl+_ (Ctrl+Shift+-) also send US, as in xterm; nano uses ^_ for go-to-line.
        "slash" | "/" | "minus" | "-" | "_" => 0x1f,
        value if value.len() == 1 => {
            let byte = value.as_bytes()[0].to_ascii_lowercase();
            if byte.is_ascii_lowercase() {
                byte - b'a' + 1
            } else {
                return None;
            }
        }
        _ => return None,
    };
    Some(vec![byte])
}

pub fn trim_terminal_output(output: &mut String) {
    const MAX_BYTES: usize = 64 * 1024;
    if output.len() <= MAX_BYTES {
        return;
    }
    let drain_to = output
        .char_indices()
        .find_map(|(index, _)| (index >= output.len() - MAX_BYTES).then_some(index))
        .unwrap_or(0);
    output.drain(..drain_to);
}

pub fn initial_terminal_screen(banner: &str) -> TerminalScreen {
    terminal_screen_from_output(banner)
}

pub fn terminal_screen_from_output(output: &str) -> TerminalScreen {
    let mut screen = TerminalScreen::default();
    screen.advance(output.as_bytes());
    screen
}
