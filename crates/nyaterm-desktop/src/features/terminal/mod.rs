//! Terminal input, selection, search, painting surface and view runtime.

use gpui::{App, KeyBinding, actions};

pub(in crate::features) mod ai_execution;
mod assist_state;
mod command_suggestions;
mod credential_autofill;
mod editing_payload;
mod editing_runtime;
mod editing_state;
mod ime_state;
mod input_runtime;
mod navigation_input;
mod send_command_runtime;
mod state;
mod terminal_context_menu_runtime;
mod terminal_font;
pub(in crate::features) mod terminal_runtime;
mod terminal_search_runtime;
mod terminal_selection_runtime;
mod terminal_surface;
mod terminal_surface_entity;
mod view_state;

pub(in crate::features) fn encoding_error_text(
    error: nyaterm_core::character_encoding::EncodingError,
) -> String {
    use nyaterm_core::character_encoding::EncodingError;
    match error {
        EncodingError::UnsupportedEncoding => {
            rust_i18n::t!("terminal.encodingUnsupported").to_string()
        }
        EncodingError::InvalidUtf8Input => {
            rust_i18n::t!("terminal.encodingInvalidInput").to_string()
        }
        EncodingError::UnrepresentableText(encoding) => rust_i18n::t!(
            "terminal.encodingUnrepresentable",
            encoding = encoding.label()
        )
        .to_string(),
    }
}

pub(in crate::features) const TERMINAL_KEY_CONTEXT: &str = "Terminal";

actions!(terminal, [TerminalTab, TerminalShiftTab, TerminalControlC]);

pub(in crate::features) fn init_key_bindings(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", TerminalTab, Some(TERMINAL_KEY_CONTEXT)),
        KeyBinding::new("shift-tab", TerminalShiftTab, Some(TERMINAL_KEY_CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-c", TerminalControlC, Some(TERMINAL_KEY_CONTEXT)),
    ]);
}

pub(in crate::features) use state::{
    LostTerminalSelectionRecovery, TerminalFeatureFocus, TerminalFeatureState,
};
pub(in crate::features) use terminal_font::{
    ResolvedAppearanceFont, TerminalFontMeasurement, TerminalFontMeasurementFailure,
    is_generic_terminal_font_family,
};
pub(in crate::features) use terminal_selection_runtime::measure_terminal_font;
pub(in crate::features) use terminal_surface_entity::{
    FULL_SHELL_PAINT_COUNT, terminal_surface_paint_count,
};
pub(in crate::features) use view_state::TerminalSessionTransferBundle;

#[cfg(test)]
mod tests {
    use gpui::{KeyBinding, KeyContext, Keymap, actions};

    #[cfg(not(target_os = "macos"))]
    use super::TerminalControlC;
    use super::{TERMINAL_KEY_CONTEXT, TerminalShiftTab, TerminalTab};

    actions!(terminal_test, [RootTab, RootShiftTab, RootCopy, InputCopy]);

    #[test]
    fn terminal_tab_bindings_shadow_root_focus_navigation() {
        let mut keymap = Keymap::default();
        keymap.add_bindings([
            KeyBinding::new("tab", RootTab, Some("Root")),
            KeyBinding::new("shift-tab", RootShiftTab, Some("Root")),
            KeyBinding::new("tab", TerminalTab, Some(TERMINAL_KEY_CONTEXT)),
            KeyBinding::new("shift-tab", TerminalShiftTab, Some(TERMINAL_KEY_CONTEXT)),
        ]);
        let contexts = [
            KeyContext::parse("Root").unwrap(),
            KeyContext::parse(TERMINAL_KEY_CONTEXT).unwrap(),
        ];

        let (tab_bindings, tab_pending) =
            keymap.bindings_for_input(&[gpui::Keystroke::parse("tab").unwrap()], &contexts);
        let (shift_tab_bindings, shift_tab_pending) =
            keymap.bindings_for_input(&[gpui::Keystroke::parse("shift-tab").unwrap()], &contexts);

        assert!(!tab_pending);
        assert!(!shift_tab_pending);
        assert!(tab_bindings[0].action().partial_eq(&TerminalTab));
        assert!(shift_tab_bindings[0].action().partial_eq(&TerminalShiftTab));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn terminal_ctrl_c_binding_shadows_root_copy() {
        let mut keymap = Keymap::default();
        keymap.add_bindings([
            KeyBinding::new("ctrl-c", RootCopy, Some("Root")),
            KeyBinding::new("ctrl-c", TerminalControlC, Some(TERMINAL_KEY_CONTEXT)),
        ]);
        let contexts = [
            KeyContext::parse("Root").unwrap(),
            KeyContext::parse(TERMINAL_KEY_CONTEXT).unwrap(),
        ];

        let (bindings, pending) =
            keymap.bindings_for_input(&[gpui::Keystroke::parse("ctrl-c").unwrap()], &contexts);

        assert!(!pending);
        assert!(bindings[0].action().partial_eq(&TerminalControlC));
        assert!(bindings[1].action().partial_eq(&RootCopy));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn input_ctrl_c_binding_stays_local_without_terminal_context() {
        let mut keymap = Keymap::default();
        keymap.add_bindings([
            KeyBinding::new("ctrl-c", RootCopy, Some("Root")),
            KeyBinding::new("ctrl-c", InputCopy, Some("Input")),
            KeyBinding::new("ctrl-c", TerminalControlC, Some(TERMINAL_KEY_CONTEXT)),
        ]);
        let contexts = [
            KeyContext::parse("Root").unwrap(),
            KeyContext::parse("Input").unwrap(),
        ];

        let (bindings, pending) =
            keymap.bindings_for_input(&[gpui::Keystroke::parse("ctrl-c").unwrap()], &contexts);

        assert!(!pending);
        assert!(bindings[0].action().partial_eq(&InputCopy));
        assert!(bindings[1].action().partial_eq(&RootCopy));
    }
}
