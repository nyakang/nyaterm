use gpui::{Context, Focusable as _, IntoElement, div, prelude::*, px, rgb};
use nyaterm_ui::{
    NyaButton, NyaButtonVariant, NyaCheckbox, NyaDropdownMenu, NyaMenuItem, NyaTooltip,
};
use rust_i18n::t;

use super::super::send_command_compact_tip;
use super::state::{SendCommandBarViewState, SendCommandDensity};
use crate::features::NyaTermApp;
use crate::send_command::SendCommandDataType;

impl NyaTermApp {
    pub(super) fn send_command_bar_footer(
        &mut self,
        state: &SendCommandBarViewState,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tab_id = state.send.tab_id;
        let palette = state.palette;
        let active_sending = state.is_sending;
        let displayed_run = self.send_command.run_id();
        let tabs = self.send_command_tabs(state, cx);
        let compact = matches!(
            state.density,
            SendCommandDensity::Compact | SendCommandDensity::Minimal
        );
        let text_mode = state.send.data_type == SendCommandDataType::Text;
        let hex = state.send.hex.clone();
        let valid_utf8 = hex.read(cx).document().pending_nibble().is_none()
            && std::str::from_utf8(hex.read(cx).document().bytes()).is_ok();
        let conversion = if text_mode {
            NyaMenuItem::action(t!("serialSend.convertToHex"))
                .disabled(state.is_sending)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.send_command.active_id() != tab_id {
                        return;
                    }
                    if this.send_command.convert_to_hex(cx) {
                        let state = this.send_command.presentation(cx);
                        this.reset_text_input(
                            &this.send_command.input_id("interval"),
                            &state.interval_input,
                            cx,
                        );
                        window.focus(&state.hex.read(cx).focus_handle(cx), cx);
                        cx.notify();
                    }
                }))
        } else {
            NyaMenuItem::action(t!("serialSend.decodeUtf8"))
                .disabled(state.is_sending || !valid_utf8)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.send_command.active_id() != tab_id {
                        return;
                    }
                    if this.send_command.decode_utf8(cx).is_ok() {
                        let state = this.send_command.presentation(cx);
                        this.reset_text_input(
                            &this.send_command.input_id("draft"),
                            &state.draft,
                            cx,
                        );
                        this.reset_text_input(
                            &this.send_command.input_id("interval"),
                            &state.interval_input,
                            cx,
                        );
                        this.focus_text_input_if_present(
                            &this.send_command.input_id("draft"),
                            window,
                            cx,
                        );
                        cx.notify();
                    }
                }))
        };
        let mut menu = vec![conversion];
        if !text_mode {
            menu.push(
                NyaMenuItem::action(t!("serialSend.pasteUtf8")).on_click(cx.listener(
                    move |this, _, window, cx| {
                        if this.send_command.active_id() != tab_id {
                            return;
                        }
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
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.send_command.active_id() != tab_id {
                        return;
                    }
                    this.send_command.clear_draft(cx);
                    if this.send_command.presentation(cx).data_type == SendCommandDataType::Text {
                        this.reset_text_input(&this.send_command.input_id("draft"), "", cx);
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
            .child(tabs)
            .when(
                !compact && self.send_command.tab_labels().len() > 1,
                |footer| {
                    footer.child(
                        div()
                            .w(px(1.))
                            .h(px(14.))
                            .flex_none()
                            .bg(rgb(palette.border)),
                    )
                },
            )
            .when(
                compact && self.send_command.tab_labels().len() == 1,
                |footer| footer.child(div().flex_1()),
            )
            .when(!compact, |footer| {
                footer.child(
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
            })
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
            .child(send_command_compact_tip(
                tab_id.input_id("tools-tip"),
                t!("serialSend.payloadActions"),
                NyaDropdownMenu::new(tab_id.input_id("tools"))
                    .icon("icons/session/more.svg")
                    .items(menu),
            ))
            .child(
                div()
                    .id("send-command.clear-after-send-help")
                    .flex_none()
                    .when(compact, |el| {
                        el.tooltip(|window, cx| {
                            NyaTooltip::new(t!("serialSend.clearAfterSend"))
                                .compact()
                                .build(window, cx)
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
                            .disabled(self.send_command.is_sending())
                            .on_click(cx.listener(move |this, enabled: &bool, _, cx| {
                                if this.send_command.active_id() != tab_id {
                                    return;
                                }
                                this.set_send_command_clear_after_send(*enabled, cx)
                            })),
                    ),
            )
            .child(
                send_command_compact_tip(
                    "bottom-command-send-tip",
                    state.action_hint.clone(),
                    NyaButton::new(
                        "bottom-command-send",
                        match (compact, active_sending) {
                            (true, _) => String::new(),
                            (false, true) => t!("serialSend.stop").to_string(),
                            (false, false) if state.is_broadcast && state.target_count > 1 => {
                                format!("{} ({})", t!("serialSend.send"), state.target_count)
                            }
                            (false, false) => t!("serialSend.send").to_string(),
                        },
                    )
                    .icon(if active_sending {
                        "icons/session/stop.svg"
                    } else {
                        "icons/send.svg"
                    })
                    .variant(if active_sending {
                        NyaButtonVariant::Danger
                    } else {
                        NyaButtonVariant::Primary
                    })
                    .small()
                    .height(px(26.))
                    .disabled(state.send_disabled)
                    .accessibility_label(state.action_hint.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.send_command.active_id() != tab_id {
                            return;
                        }
                        // A queued click on a Send button must not turn into
                        // a Stop action after a run starts on another event.
                        if let Some(run_id) = displayed_run {
                            if this.send_command.run_id() == Some(run_id) {
                                this.stop_send_command(cx);
                            }
                        } else if !this.send_command.active_is_sending() {
                            this.send_bottom_command(false, cx);
                        }
                    })),
                )
                .debug_selector(|| "bottom-command-send".into()),
            )
            .into_any_element()
    }
}
