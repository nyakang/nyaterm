use crate::models::ConnectionEditorSection;
use rust_i18n::t;

use gpui::{
    Context, FontWeight, IntoElement, SharedString, div,
    prelude::{
        FluentBuilder, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled,
    },
    px, rgb, rgba, svg,
};
use nyaterm_ui::{NyaPopover, NyaScrollable};

use crate::features::NyaTermApp;
use crate::models::{ConnectionEditorField, ConnectionEditorSelect};

use super::super::super::list::{
    ConnectionEditorRenderContext, FieldLabel, connection_editor_select,
    connection_editor_select_on_open, editor_stepper_field, required,
};
use super::{ConnectionEditorSectionContext, recording::connection_editor_recording_section};

const SERIAL_BAUD_PRESETS: [&str; 12] = [
    "1200", "2400", "4800", "9600", "19200", "38400", "57600", "115200", "230400", "460800",
    "921600", "1000000",
];

fn serial_baud_picker(
    section: ConnectionEditorSectionContext<'_>,
    open: bool,
    cx: &mut Context<NyaTermApp>,
) -> impl IntoElement {
    let ConnectionEditorSectionContext {
        palette,
        editor,
        fields,
        baud_popover_open: _,
        narrow: _,
    } = section;
    let is_preset = SERIAL_BAUD_PRESETS.contains(&editor.baud_rate.trim());
    let ask_at_connect = editor.serial_prompt_baud_rate;
    let valid = editor
        .baud_rate
        .trim()
        .parse::<u32>()
        .is_ok_and(|value| (1..=4_000_000).contains(&value));
    let trigger = div()
        .id("connection-editor-baud-trigger")
        .h(px(32.))
        .min_w_0()
        .px_3()
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .rounded_sm()
        .border_1()
        .border_color(if valid {
            rgb(palette.border)
        } else {
            rgb(palette.danger)
        })
        .bg(rgb(palette.input_surface()))
        .cursor_pointer()
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_xs()
                .text_color(rgb(palette.text))
                .child(if ask_at_connect {
                    t!("dialog.askAtConnect").to_string()
                } else if editor.baud_rate.trim().is_empty() {
                    t!("dialog.selectBaudRate").to_string()
                } else if is_preset {
                    editor.baud_rate.clone()
                } else {
                    format!("{} · {}", editor.baud_rate, t!("dialog.customBaudRate"))
                }),
        )
        .child(
            svg()
                .size(px(14.))
                .path("icons/chevron-down.svg")
                .text_color(rgb(palette.text_muted)),
        );

    // The popup has its own fixed width, independent of the form column.
    // Keeping its presets compact leaves the custom input visible on open.
    let mut preset_grid = div().grid().grid_cols(3).gap_1();
    for preset in SERIAL_BAUD_PRESETS {
        let value = preset.to_string();
        // Deferring wins the display: no preset is in effect until one is
        // chosen again.
        let selected = !ask_at_connect && editor.baud_rate == preset;
        preset_grid = preset_grid.child(
            div()
                .id(SharedString::from(format!(
                    "connection-editor-baud-{preset}"
                )))
                .h(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .border_1()
                .border_color(if selected {
                    rgb(palette.primary)
                } else {
                    rgb(palette.border)
                })
                .bg(if selected {
                    rgba((palette.primary << 8) | 0x20)
                } else {
                    rgb(palette.input)
                })
                .text_size(px(10.))
                .cursor_pointer()
                .hover(|this| this.bg(rgb(palette.hover)))
                .child(preset)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_connection_editor_baud_rate(value.clone(), cx);
                })),
        );
    }

    let content = div()
        .occlude()
        .w(px(300.))
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.surface_elevated))
        .shadow_lg()
        .child(
            div()
                .max_h(px(360.))
                .overflow_y_scrollbar()
                .p_3()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight(600.))
                        .child(t!("dialog.selectBaudRate")),
                )
                .child(
                    div()
                        .id("connection-editor-baud-ask")
                        .debug_selector(|| "connection-editor-baud-ask".to_string())
                        .h(px(30.))
                        .w_full()
                        .px_2()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .rounded_sm()
                        .border_1()
                        .border_color(if ask_at_connect {
                            rgb(palette.primary)
                        } else {
                            rgb(palette.border)
                        })
                        .bg(if ask_at_connect {
                            rgba((palette.primary << 8) | 0x20)
                        } else {
                            rgb(palette.input)
                        })
                        .text_xs()
                        .cursor_pointer()
                        .hover(|this| this.bg(rgb(palette.hover)))
                        .child(t!("dialog.askAtConnect").to_string())
                        .when(ask_at_connect, |this| {
                            this.child(
                                svg()
                                    .size(px(12.))
                                    .path("icons/check.svg")
                                    .text_color(rgb(palette.primary)),
                            )
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_connection_editor_baud_prompt(true, cx);
                        })),
                )
                .child(preset_grid)
                .child(
                    div()
                        .border_t_1()
                        .border_color(rgb(palette.border))
                        .pt_3()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(palette.text_muted))
                                .child(t!("dialog.customBaudRate")),
                        )
                        .child(
                            div()
                                .debug_selector(|| "connection-editor-custom-baud".to_string())
                                .child(editor_stepper_field(
                                    palette,
                                    "",
                                    ConnectionEditorField::BaudRate,
                                    fields,
                                    cx,
                                )),
                        )
                        .when(!valid, |this| {
                            this.child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(palette.danger))
                                    .child(t!(
                                        "dialog.baudRateInvalid",
                                        min = "1",
                                        max = "4000000"
                                    )),
                            )
                        })
                        .child(
                            div()
                                .debug_selector(|| {
                                    "connection-editor-apply-custom-baud".to_string()
                                })
                                .child(
                                    nyaterm_ui::NyaButton::new(
                                        "connection-editor-apply-custom-baud",
                                        t!("dialog.applyCustomBaudRate"),
                                    )
                                    .disabled(!valid)
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            let value = this
                                                .connection_state
                                                .active_editor_draft()
                                                .map(|editor| editor.baud_rate)
                                                .unwrap_or_default();
                                            this.set_connection_editor_baud_rate(value, cx);
                                        },
                                    )),
                                ),
                        ),
                ),
        );

    div()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(t!("dialog.baudRate")),
        )
        .child(
            NyaPopover::new("connection-editor-baud-popover", trigger, content)
                .appearance(false)
                .open(open)
                .on_open_change(cx.listener(|this, open, _, cx| {
                    this.set_connection_editor_baud_popover_open(*open, cx);
                })),
        )
}

pub(super) fn connection_editor_serial_section(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let content = match section.editor.section {
        ConnectionEditorSection::Basic | ConnectionEditorSection::Serial => {
            serial_settings(section, cx)
        }
        ConnectionEditorSection::Terminal => serial_terminal(section, cx),
        _ => div(),
    };
    div()
        .debug_selector(|| "connection-editor-serial-section".to_string())
        .flex()
        .flex_col()
        .gap_3()
        .child(content)
}

fn serial_settings(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let ConnectionEditorSectionContext {
        palette,
        fields,
        editor,
        baud_popover_open,
        narrow,
    } = section;
    let app = cx.weak_entity();
    // Deferring the device makes the dropdown a default rather than a
    // requirement, so the asterisk goes away with it.
    let port_label = if editor.serial_prompt_port {
        FieldLabel::from(t!("dialog.serialPort").to_string())
    } else {
        required(t!("dialog.serialPort"))
    };
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .when(!narrow, |this| this.flex_wrap())
                .when(narrow, |this| this.flex_col())
                .items_end()
                .when(narrow, |this| this.items_stretch())
                .gap_3()
                .child(
                    div()
                        .min_w(px(180.))
                        .flex_1()
                        .child(connection_editor_select_on_open(
                            ConnectionEditorRenderContext {
                                palette,
                                fields,
                                cx,
                            },
                            "connection-editor-serial-port",
                            port_label,
                            ConnectionEditorSelect::SerialPort,
                            move |_, cx| {
                                let _ = app.update(cx, |app, cx| {
                                    app.request_connection_serial_ports_refresh(cx);
                                });
                            },
                        )),
                )
                .child(div().min_w(px(180.)).flex_1().child(serial_baud_picker(
                    section,
                    baud_popover_open,
                    cx,
                ))),
        )
        .when(
            editor.serial_prompt_port || editor.serial_prompt_baud_rate,
            |this| {
                this.child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(palette.text_muted))
                        .child(t!("dialog.askAtConnectHint")),
                )
            },
        )
        .child(
            div()
                .grid()
                .grid_cols(if narrow { 1 } else { 3 })
                .gap_2()
                .child(connection_editor_select(
                    ConnectionEditorRenderContext {
                        palette,
                        fields,
                        cx,
                    },
                    "connection-editor-data-bits",
                    t!("dialog.dataBits"),
                    ConnectionEditorSelect::DataBits,
                ))
                .child(connection_editor_select(
                    ConnectionEditorRenderContext {
                        palette,
                        fields,
                        cx,
                    },
                    "connection-editor-parity",
                    t!("dialog.parity"),
                    ConnectionEditorSelect::Parity,
                ))
                .child(connection_editor_select(
                    ConnectionEditorRenderContext {
                        palette,
                        fields,
                        cx,
                    },
                    "connection-editor-stop-bits",
                    t!("dialog.stopBits"),
                    ConnectionEditorSelect::StopBits,
                )),
        )
        .child(connection_editor_select(
            ConnectionEditorRenderContext {
                palette,
                fields,
                cx,
            },
            "connection-editor-flow-control",
            t!("dialog.serialFlowControl"),
            ConnectionEditorSelect::FlowControl,
        ))
}

fn serial_terminal(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let ConnectionEditorSectionContext {
        palette,
        fields,
        narrow,
        ..
    } = section;
    div().flex().flex_col().gap_3().child(
        div()
            .id("connection-editor-serial-advanced")
            .debug_selector(|| "connection-editor-serial-advanced".to_string())
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .p_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .when(!narrow, |this| this.flex_wrap())
                    .when(narrow, |this| this.flex_col())
                    .gap_3()
                    .child(
                        div()
                            .min_w(px(180.))
                            .flex_1()
                            .child(connection_editor_select(
                                ConnectionEditorRenderContext {
                                    palette,
                                    fields,
                                    cx,
                                },
                                "connection-editor-serial-backspace",
                                t!("dialog.backspaceMode"),
                                ConnectionEditorSelect::Backspace,
                            )),
                    )
                    .child(
                        div()
                            .min_w(px(180.))
                            .flex_1()
                            .child(connection_editor_select(
                                ConnectionEditorRenderContext {
                                    palette,
                                    fields,
                                    cx,
                                },
                                "connection-editor-serial-encoding",
                                t!("connection.encoding"),
                                ConnectionEditorSelect::Encoding,
                            )),
                    ),
            )
            .child(connection_editor_recording_section(section, cx)),
    )
}
