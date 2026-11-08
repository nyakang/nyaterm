use gpui::{Context, Focusable as _, IntoElement, KeyDownEvent, Window, div, prelude::*, px, rgb};

use super::state::SendCommandBarViewState;
use crate::features::NyaTermApp;
use crate::send_command::SendCommandDataType;

impl NyaTermApp {
    pub(in crate::features) fn focus_send_command_composer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let send = self.send_command.presentation(cx);
        match send.data_type {
            SendCommandDataType::Hex => window.focus(&send.hex.read(cx).focus_handle(cx), cx),
            SendCommandDataType::Text => {
                let input = self.text_input(
                    "send-command.draft",
                    &send.draft,
                    crate::features::text_inputs::TextInputSetup::multi_line(rust_i18n::t!(
                        "serialSend.textPlaceholder"
                    )),
                    cx,
                );
                window.focus(&input.read(cx).focus_handle(), cx);
            }
        }
    }
    pub(super) fn send_command_bar_composer(
        &mut self,
        state: &SendCommandBarViewState,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let composer = match state.send.data_type {
            SendCommandDataType::Text => self.send_command_text_composer(state, cx),
            SendCommandDataType::Hex => self.send_command_hex_composer(state),
        };
        let palette = state.palette;
        div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .rounded(px(6.))
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.input_surface()))
            .overflow_hidden()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    if this.send_command.presentation(cx).data_type == SendCommandDataType::Hex {
                        this.send_command
                            .presentation(cx)
                            .hex
                            .update(cx, |hex, cx| hex.cancel_selection(cx));
                    }
                    window.focus(this.terminal.input_focus(), cx);
                    cx.stop_propagation();
                } else if this.handle_send_command_key_down(event, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(composer)
            .into_any_element()
    }
}
