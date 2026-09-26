use gpui::{Context, KeyDownEvent, Window};

use crate::features::NyaTermApp;

impl NyaTermApp {
    pub(in crate::features) fn handle_global_shortcut(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.security.screen_locked() {
            return false;
        }

        if event.keystroke.key.as_str() == "escape" && self.shell.pane_focus_mode() {
            self.shell.set_pane_focus_mode(false);
            cx.notify();
            return true;
        }

        if event.keystroke.key.as_str() == "escape" && self.translation.dialog_is_open() {
            self.close_translation_dialog(window, cx);
            return true;
        }

        // Dismiss strip menus with Escape (Tauri dropdown dismiss).
        if event.keystroke.key.as_str() == "escape"
            && (self.shell.open_tabs_menu_is_open() || self.shell.new_session_menu_is_open())
        {
            self.close_open_tabs_menu(cx);
            self.close_new_session_menu(cx);
            return true;
        }

        if event.keystroke.key.as_str() == "escape" && self.close_last_floating_panel(cx) {
            return true;
        }

        false
    }
}
