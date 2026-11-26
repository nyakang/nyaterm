use gpui::{Context, Focusable as _, IntoElement, div, prelude::*, px, rgb};
use nyaterm_ui::{
    NyaButton, NyaButtonVariant, NyaCheckbox, NyaDropdownMenu, NyaMenuItem, NyaTooltip,
};
use rust_i18n::t;

use super::state::{SendCommandBarViewState, SendCommandDensity};
use crate::features::NyaTermApp;
use crate::send_command::SendCommandDataType;

impl NyaTermApp {
    pub(super) fn send_command_bar_footer(
        &mut self,
        state: &SendCommandBarViewState,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let palette = state.palette;
        let compact = state.density == SendCommandDensity::Compact;
        let text_mode = state.send.data_type == SendCommandDataType::Text;
        let hex = state.send.hex.clone();
        let valid_utf8 = hex.read(cx).document().pending_nibble().is_none()
            && std::str::from_utf8(hex.read(cx).document().bytes()).is_ok();
        let conversion = if text_mode {
            NyaMenuItem::action(t!("serialSend.convertToHex"))
                .disabled(state.is_sending)
                .on_click(cx.listener(|this, _, window, cx| {
                    if this.send_command.convert_to_hex(cx) {
                        let state = this.send_command.presentation(cx);
                        this.reset_text_input("send-command.interval", &state.interval_input, cx);
                        window.focus(&state.hex.read(cx).focus_handle(cx), cx);
                        cx.notify();
                    }
                }))
        } else {
            NyaMenuItem::action(t!("serialSend.decodeUtf8"))
                .disabled(state.is_sending || !valid_utf8)
                .on_click(cx.listener(|this, _, window, cx| {
                    if this.send_command.decode_utf8(cx).is_ok() {
                        let state = this.send_command.presentation(cx);
                        this.reset_text_input("send-command.draft", &state.draft, cx);
                        this.reset_text_input("send-command.interval", &state.interval_input, cx);
                        this.focus_text_input_if_present("send-command.draft", window, cx);
                        cx.notify();
                    }
                }))
        };
        let mut menu = vec![conversion];
        if !text_mode {
            menu.push(
                NyaMenuItem::action(t!("serialSend.pasteUtf8")).on_click(cx.listener(
                    move |_, _, window, cx| {
                        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                            hex.update(cx, |hex, cx| hex.insert_bytes(text.as_bytes(), cx));
                        }
                        window.focus(&hex.read(cx).focus_handle(cx), cx);
                    },
                )),
            );
        }
        menu.push(
            NyaMenuItem::action(t!("serialSend.clearPayload"))
                .shortcut(if cfg!(target_os = "macos") {
                    "Cmd+L"
                } else {
                    "Ctrl+L"
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.send_command.clear_draft(cx);
                    if this.send_command.presentation(cx).data_type == SendCommandDataType::Text {
                        this.reset_text_input("send-command.draft", "", cx);
                    }
                    cx.notify();
                })),
        );
        div()
            .id("send-command.footer")
            .debug_selector(|| "send-command.footer".into())
            .h(px(30.))
            .w_full()
            .min_w_0()
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .overflow_hidden()
                    .text_size(px(11.))
                    .text_color(rgb(if state.validation_error {
                        palette.danger
                    } else {
                        palette.text_muted
                    }))
                    .child(if state.is_sending {
                        state.progress_label.clone()
                    } else {
                        state.status.clone()
                    }),
            )
            .when(
                state.is_sending && !compact && state.send.rounds > 0,
                |footer| {
                    footer.child(
                        div()
                            .w(px(64.))
                            .h(px(3.))
                            .flex_none()
                            .bg(rgb(palette.surface_elevated))
                            .child(
                                div()
                                    .h_full()
                                    .w(gpui::relative(state.progress_ratio))
                                    .bg(rgb(palette.accent)),
                            ),
                    )
                },
            )
            .child(
                NyaDropdownMenu::new("send-command.tools")
                    .icon("icons/session/more.svg")
                    .tooltip(t!("serialSend.payloadActions"))
                    .items(menu),
            )
            .child(
                div()
                    .id("send-command.clear-after-send-help")
                    .flex_none()
                    .when(compact, |el| {
                        el.tooltip(|window, cx| {
                            NyaTooltip::new(t!("serialSend.clearAfterSend")).build(window, cx)
                        })
                    })
                    .child(
                        NyaCheckbox::new("send-command.clear-after-send")
                            .label(if compact {
                                "".into()
                            } else {
                                t!("serialSend.clearAfterSend")
                            })
                            .checked(self.settings.summary().ui_serial_send_clear_after_send)
                            .disabled(state.is_sending)
                            .on_click(cx.listener(|this, enabled: &bool, _, cx| {
                                this.set_send_command_clear_after_send(*enabled, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| "bottom-command-send".into())
                    .flex_none()
                    .child(
                        NyaButton::new(
                            "bottom-command-send",
                            match (compact, state.is_sending) {
                                (true, _) => String::new(),
                                (false, true) => t!("serialSend.stop").to_string(),
                                (false, false) => t!("serialSend.send").to_string(),
                            },
                        )
                        .icon(if state.is_sending {
                            "icons/session/stop.svg"
                        } else {
                            "icons/send.svg"
                        })
                        .variant(if state.is_sending {
                            NyaButtonVariant::Danger
                        } else {
                            NyaButtonVariant::Primary
                        })
                        .small()
                        .height(px(26.))
                        .disabled(state.send_disabled)
                        .tooltip(state.action_hint.clone())
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.send_command.is_sending() {
                                this.stop_send_command(cx);
                            } else {
                                this.send_bottom_command(false, cx);
                            }
                        })),
                    ),
            )
            .into_any_element()
    }
}
