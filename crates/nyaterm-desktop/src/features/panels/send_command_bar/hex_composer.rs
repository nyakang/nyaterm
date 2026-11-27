use super::state::{SendCommandBarViewState, SendCommandDensity};
use crate::features::NyaTermApp;
use gpui::IntoElement;
use nyaterm_ui::hex_editor::NyaHexEditor;
use rust_i18n::t;

impl NyaTermApp {
    pub(super) fn send_command_hex_composer(
        &self,
        state: &SendCommandBarViewState,
    ) -> gpui::AnyElement {
        NyaHexEditor::new(&state.send.hex, state.palette)
            .id(state.send.tab_id.input_id("hex"))
            .compact(matches!(
                state.density,
                SendCommandDensity::Compact | SendCommandDensity::Minimal
            ))
            .placeholder(state.input_hint.clone())
            .menu_labels([
                t!("serialSend.copyHex").into(),
                t!("serialSend.copyCompact").into(),
                t!("serialSend.copyCArray").into(),
                t!("serialSend.copyEscaped").into(),
                t!("serialSend.copyAscii").into(),
                t!("menu.paste").into(),
                t!("menu.selectAll").into(),
            ])
            .into_any_element()
    }
}
