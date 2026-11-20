use rust_i18n::t;

use super::state::SendCommandBarViewState;
use gpui::{Context, IntoElement, div, prelude::*, px, rgb};
use nyaterm_transport::SessionKind;
use nyaterm_ui::{
    NyaButton, NyaNumberInput, NyaNumberInputOptions, NyaPopover, NyaPopoverAlign,
    NyaPopoverPlacement, NyaScrollable, NyaSelectOption, NyaTabItem, NyaTabs,
};

use super::super::send_command_control_group;
use crate::features::NyaTermApp;
use crate::send_command::{
    SendCommandDataType, SendCommandLineEnding, SendCommandMode, SendCommandTarget,
};

impl NyaTermApp {
    pub(super) fn send_command_bar_controls(
        &mut self,
        state: &SendCommandBarViewState,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let palette = state.palette;
        let is_sending = state.is_sending;
        let is_serial = matches!(self.active_session_kind(), Some(SessionKind::Serial));
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
        let mut target_options = vec![NyaSelectOption::new(
            "current",
            t!("serialSend.currentSession"),
        )];
        if !is_serial {
            target_options.push(NyaSelectOption::new("all", t!("serialSend.allSessions")));
        }
        target_options.extend(state.group_targets.iter().map(|(group_id, name, count)| {
            NyaSelectOption::new(
                format!("group:{group_id}"),
                t!("serialSend.groupSession", name = name, count = count),
            )
        }));
        let selected_target = match &state.send.target {
            SendCommandTarget::Current => "current".to_string(),
            SendCommandTarget::AllCompatible if !is_serial => "all".to_string(),
            SendCommandTarget::AllCompatible => "current".to_string(),
            SendCommandTarget::Group(group_id) => format!("group:{group_id}"),
        };
        if !target_options
            .iter()
            .any(|option| option.value() == selected_target)
        {
            target_options.push(NyaSelectOption::new(
                selected_target.clone(),
                t!("network.group"),
            ));
        }
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
        let layout = ToolbarLayout::for_width(state.send.viewport_width, state.is_serial_text_line);
        let count = self.number_input(
            "send-command.count",
            &state.send.count_input,
            NyaNumberInputOptions::default()
                .range(1.0, 9_999.0)
                .step(1.0)
                .allow_infinity(true)
                .disabled(is_sending),
            cx,
        );
        let interval = self.number_input(
            "send-command.interval",
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
                NyaPopover::new(
                    "bottom-command-repeat-popover",
                    div().flex_none().child(
                        NyaButton::new("bottom-command-repeat", summary)
                            .height(px(32.))
                            .disabled(is_sending)
                            .tooltip(format!(
                                "{} · {}",
                                t!("serialSend.count"),
                                t!("serialSend.interval")
                            )),
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
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(send_command_control_group(
                        palette,
                        t!("serialSend.count"),
                        labeled_number(
                            palette,
                            labels.then(|| count_label.into()),
                            108.,
                            NyaNumberInput::new(&count),
                        ),
                    ))
                    .child(send_command_control_group(
                        palette,
                        t!("serialSend.interval"),
                        labeled_number(
                            palette,
                            labels.then(|| interval_label.into()),
                            144.,
                            NyaNumberInput::new(&interval),
                        ),
                    ))
                    .into_any_element()
            }
        };

        div()
            .id("bottom-command-controls")
            .debug_selector(|| "bottom-command-controls".into())
            .h(px(32.))
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
                        .on_select(cx.listener(|this, index: &usize, window, cx| {
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
                    "bottom-command-mode-select",
                    mode_options,
                    Some(selected_mode.to_string()),
                    is_sending,
                    cx,
                ),
            ))
            .when(state.is_serial_text_line, |this| {
                this.child(send_command_control_group(
                    palette,
                    t!("serialSend.lineEnding"),
                    self.form_select_control(
                        "bottom-command-eol-select",
                        line_ending_options,
                        Some(selected_line_ending),
                        is_sending,
                        cx,
                    ),
                ))
            })
            .child(toolbar_divider(palette))
            .child(send_command_control_group(
                palette,
                t!("serialSend.target"),
                self.form_select_control(
                    "bottom-command-target-select",
                    target_options,
                    Some(selected_target),
                    is_sending,
                    cx,
                ),
            ))
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
        if width <= 0. || width >= 840. + extra {
            Self::Full
        } else if width >= 720. + extra {
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
