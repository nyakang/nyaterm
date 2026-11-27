use rust_i18n::t;

use super::state::SendCommandBarViewState;
use gpui::{Context, IntoElement, div, prelude::*, px, rgb};
use nyaterm_ui::{
    NyaButton, NyaNumberInput, NyaNumberInputOptions, NyaPopover, NyaPopoverAlign,
    NyaPopoverPlacement, NyaScrollable, NyaSelectOption, NyaTabItem, NyaTabs,
};

use super::super::{send_command_compact_tip, send_command_control_group};
use crate::features::NyaTermApp;
use crate::send_command::{SendCommandDataType, SendCommandLineEnding, SendCommandMode};

impl NyaTermApp {
    pub(super) fn send_command_bar_controls(
        &mut self,
        state: &SendCommandBarViewState,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tab_id = state.send.tab_id;
        let palette = state.palette;
        let is_sending = state.is_sending;
        let (mode_options, selected_mode) = if state.send.data_type == SendCommandDataType::Hex {
            (
                vec![
                    NyaSelectOption::new("byte", t!("serialSend.byteByByte")),
                    NyaSelectOption::new("packet", t!("serialSend.packet")),
                ],
                match state.send.mode {
                    SendCommandMode::Packet => "packet",
                    _ => "byte",
                },
            )
        } else {
            (
                vec![
                    NyaSelectOption::new("line", t!("serialSend.lineByLine")),
                    NyaSelectOption::new("character", t!("serialSend.characterByCharacter")),
                ],
                match state.send.mode {
                    SendCommandMode::Character => "character",
                    _ => "line",
                },
            )
        };
        let line_ending_options = vec![
            NyaSelectOption::new("none", t!("serialSend.noLineEnding")),
            NyaSelectOption::new("cr", "CR"),
            NyaSelectOption::new("lf", "LF"),
            NyaSelectOption::new("crlf", "CR+LF"),
        ];
        let selected_line_ending = match state.send.line_ending {
            SendCommandLineEnding::None => "none",
            SendCommandLineEnding::Cr => "cr",
            SendCommandLineEnding::Lf => "lf",
            SendCommandLineEnding::Crlf => "crlf",
        }
        .to_string();
        let layout = ToolbarLayout::for_width(state.send.viewport_width, state.show_line_ending);
        let count = self.number_input(
            self.send_command.input_id("count"),
            &state.send.count_input,
            NyaNumberInputOptions::default()
                .range(1.0, 9_999.0)
                .step(1.0)
                .allow_infinity(true)
                .disabled(is_sending),
            cx,
        );
        let interval = self.number_input(
            self.send_command.input_id("interval"),
            &state.send.interval_input,
            NyaNumberInputOptions::default()
                .range(0.0, 60.0)
                .step(0.001)
                .decimal_places(3)
                .suffix(t!("serialSend.seconds"))
                .disabled(is_sending),
            cx,
        );
        let count_label = t!("serialSend.countShort");
        let interval_label = t!("serialSend.intervalShort");
        let repeat = match layout {
            ToolbarLayout::Folded => {
                let summary = format!(
                    "×{} · {}{}",
                    state.send.count_input,
                    state.send.interval_input,
                    t!("serialSend.seconds")
                );
                let repeat_tip =
                    format!("{} · {}", t!("serialSend.count"), t!("serialSend.interval"));
                NyaPopover::new(
                    tab_id.input_id("repeat-popover"),
                    send_command_compact_tip(
                        "bottom-command-repeat-tip",
                        repeat_tip.clone(),
                        NyaButton::new("bottom-command-repeat", summary)
                            .height(px(32.))
                            .disabled(is_sending)
                            .accessibility_label(repeat_tip),
                    ),
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(labeled_number(
                            palette,
                            Some(t!("serialSend.count").into()),
                            144.,
                            NyaNumberInput::new(&count),
                        ))
                        .child(labeled_number(
                            palette,
                            Some(t!("serialSend.interval").into()),
                            144.,
                            NyaNumberInput::new(&interval),
                        )),
                )
                .placement(NyaPopoverPlacement::Top)
                .align(NyaPopoverAlign::Start)
                .offset(px(4.))
                .into_any_element()
            }
            ToolbarLayout::Full | ToolbarLayout::Unlabeled => {
                let labels = layout == ToolbarLayout::Full;
                let count_field = labeled_number(
                    palette,
                    labels.then(|| count_label.into()),
                    108.,
                    NyaNumberInput::new(&count),
                );
                let interval_field = labeled_number(
                    palette,
                    labels.then(|| interval_label.into()),
                    144.,
                    NyaNumberInput::new(&interval),
                );
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(if labels {
                        div()
                            .h(px(32.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .child(count_field)
                            .into_any_element()
                    } else {
                        send_command_control_group(palette, t!("serialSend.count"), count_field)
                            .into_any_element()
                    })
                    .child(if labels {
                        div()
                            .h(px(32.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .child(interval_field)
                            .into_any_element()
                    } else {
                        send_command_control_group(
                            palette,
                            t!("serialSend.interval"),
                            interval_field,
                        )
                        .into_any_element()
                    })
                    .into_any_element()
            }
        };

        div()
            .id("bottom-command-controls")
            .debug_selector(|| "bottom-command-controls".into())
            .h(px(36.))
            .px(px(2.))
            .py(px(2.))
            .min_w_0()
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .overflow_x_scrollbar()
            .child(
                div().w(px(112.)).h(px(32.)).flex_none().child(
                    NyaTabs::new("bottom-command-data-tabs")
                        .items([
                            NyaTabItem::new(t!("serialSend.text")).disabled(is_sending),
                            NyaTabItem::new("HEX").disabled(is_sending),
                        ])
                        .selected_index(usize::from(
                            state.send.data_type == SendCommandDataType::Hex,
                        ))
                        .on_select(cx.listener(move |this, index: &usize, window, cx| {
                            if this.send_command.active_id() != tab_id {
                                return;
                            }
                            this.set_send_command_data_type(
                                if *index == 0 {
                                    SendCommandDataType::Text
                                } else {
                                    SendCommandDataType::Hex
                                },
                                cx,
                            );
                            this.focus_send_command_composer(window, cx);
                        })),
                ),
            )
            .child(send_command_control_group(
                palette,
                t!("serialSend.sendMode"),
                self.form_select_control(
                    self.send_command.input_id("mode-select"),
                    mode_options,
                    Some(selected_mode.to_string()),
                    is_sending,
                    cx,
                ),
            ))
            .when(state.show_line_ending, |this| {
                this.child(send_command_control_group(
                    palette,
                    t!("serialSend.lineEnding"),
                    self.form_select_control(
                        self.send_command.input_id("eol-select"),
                        line_ending_options,
                        Some(selected_line_ending),
                        is_sending,
                        cx,
                    ),
                ))
            })
            .child(toolbar_divider(palette))
            .child(self.send_command_target_picker(state, cx))
            .child(toolbar_divider(palette))
            .child(repeat)
            .into_any_element()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolbarLayout {
    /// Count and interval carry a visible short label.
    Full,
    /// Labels move into the tooltips.
    Unlabeled,
    /// Count and interval fold into one summary button with a popover.
    Folded,
}

impl ToolbarLayout {
    fn for_width(width: f32, line_ending: bool) -> Self {
        // The line-ending select only appears for serial text and needs ~110px.
        let extra = if line_ending { 110. } else { 0. };
        if width <= 0. || width >= 804. + extra {
            Self::Full
        } else if width >= 684. + extra {
            Self::Unlabeled
        } else {
            Self::Folded
        }
    }
}

fn toolbar_divider(palette: crate::theme::ThemePalette) -> impl IntoElement {
    div()
        .h(px(18.))
        .w(px(1.))
        .mx_1()
        .flex_none()
        .bg(rgb(palette.border))
}

fn labeled_number(
    palette: crate::theme::ThemePalette,
    label: Option<gpui::SharedString>,
    width: f32,
    input: NyaNumberInput,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_1p5()
        .px_1()
        .when_some(label, |row, label| {
            row.child(
                div()
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child(label),
            )
        })
        .child(div().w(px(width)).h(px(32.)).flex_none().child(input))
}
