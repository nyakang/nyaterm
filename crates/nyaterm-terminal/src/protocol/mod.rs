//! Protocol modes and outbound input encoding derived from terminal state.
use crate::TerminalScreen;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalProtocolState {
    pub focus_reporting: bool,
    pub bracketed_paste: bool,
    pub mouse_reporting: bool,
    pub mouse_sgr: bool,
    pub mouse_utf8: bool,
    pub mouse_drag_reporting: bool,
    pub mouse_motion_reporting: bool,
    pub application_cursor_keys: bool,
    pub application_keypad: bool,
    pub kitty_keyboard_disambiguate: bool,
    pub kitty_keyboard_report_event_types: bool,
    pub kitty_keyboard_report_alternate_keys: bool,
    pub kitty_keyboard_report_all_keys_as_esc: bool,
    pub kitty_keyboard_report_associated_text: bool,
    pub alternate_scroll: bool,
    pub alternate_screen: bool,
}

impl TerminalProtocolState {
    pub fn from_screen(screen: &TerminalScreen) -> Self {
        Self {
            focus_reporting: screen.focus_reporting(),
            bracketed_paste: screen.bracketed_paste(),
            mouse_reporting: screen.mouse_reporting(),
            mouse_sgr: screen.mouse_sgr(),
            mouse_utf8: screen.mouse_utf8(),
            mouse_drag_reporting: screen.mouse_drag_reporting(),
            mouse_motion_reporting: screen.mouse_motion_reporting(),
            application_cursor_keys: screen.application_cursor_keys(),
            application_keypad: screen.application_keypad(),
            kitty_keyboard_disambiguate: screen.kitty_keyboard_disambiguate(),
            kitty_keyboard_report_event_types: screen.kitty_keyboard_report_event_types(),
            kitty_keyboard_report_alternate_keys: screen.kitty_keyboard_report_alternate_keys(),
            kitty_keyboard_report_all_keys_as_esc: screen.kitty_keyboard_report_all_keys_as_esc(),
            kitty_keyboard_report_associated_text: screen.kitty_keyboard_report_associated_text(),
            alternate_scroll: screen.alternate_scroll(),
            alternate_screen: screen.alternate_screen(),
        }
    }

    pub fn alternate_scroll_payload(self, delta_lines: i32) -> Option<Vec<u8>> {
        if delta_lines == 0
            || !self.alternate_screen
            || !self.alternate_scroll
            || self.mouse_reporting
        {
            return None;
        }
        let up = delta_lines > 0;
        let unit = crate::alternate_scroll_key_bytes(up, self.application_cursor_keys);
        let steps = delta_lines.unsigned_abs().min(8) as usize;
        let mut payload = Vec::with_capacity(unit.len() * steps);
        for _ in 0..steps {
            payload.extend_from_slice(&unit);
        }
        Some(payload)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn encode_mouse_report(
        self,
        button: u8,
        col: u16,
        row: u16,
        press: bool,
        motion: bool,
        shift: bool,
        alt: bool,
        ctrl: bool,
    ) -> Vec<u8> {
        if !self.mouse_reporting {
            return Vec::new();
        }
        let x = col.saturating_add(1);
        let y = row.saturating_add(1);
        let mut code = if press || self.mouse_sgr { button } else { 3 };
        if motion {
            code = code.saturating_add(32);
        }
        if shift {
            code = code.saturating_add(4);
        }
        if alt {
            code = code.saturating_add(8);
        }
        if ctrl {
            code = code.saturating_add(16);
        }
        if self.mouse_sgr {
            let suffix = if press { 'M' } else { 'm' };
            format!("\x1b[<{code};{x};{y}{suffix}").into_bytes()
        } else {
            crate::encode_legacy_mouse_report(code, col, row, self.mouse_utf8)
        }
    }
}

#[cfg(test)]
mod tests;
