use super::super::panels::RemoteMonitorPanel;
use super::data::ProcessDisplayMode;
use super::resources::process_usage_color as usage_color;
use crate::features::transfers::format_file_size;
use crate::theme::ThemePalette;
use crate::widgets::small_button;
use gpui::{
    App, ClickEvent, Context, FontWeight, IntoElement, KeyDownEvent, SharedString, Window, div,
    prelude::*, px, rgb,
};
use nyaterm_remote_ops::remote_process::RemoteProcess;
use std::borrow::Cow;

#[derive(Clone)]
pub(in crate::features::pages::remote) struct ProcessDetailLabels {
    pub cpu: Cow<'static, str>,
    pub memory: Cow<'static, str>,
    pub rss: Cow<'static, str>,
    pub elapsed: Cow<'static, str>,
    pub copy_command: Cow<'static, str>,
    pub apply_nice: Cow<'static, str>,
}

pub(in crate::features::pages::remote) fn process_details(
    palette: ThemePalette,
    process: &RemoteProcess,
    mode: ProcessDisplayMode,
    labels: ProcessDetailLabels,
    nice_input: Option<gpui::AnyElement>,
    on_copy_command: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &mut Context<RemoteMonitorPanel>,
) -> gpui::AnyElement {
    let command = if process.command_line.trim().is_empty() {
        process.command.clone()
    } else {
        process.command_line.clone()
    };
    let pid = process.pid;
    let narrow = matches!(
        mode,
        ProcessDisplayMode::Compact | ProcessDisplayMode::Narrow
    );
    let state_color = match process.state.trim().to_ascii_lowercase().as_str() {
        "stopped" | "t" => rgb(palette.warning),
        "zombie" | "z" => rgb(palette.danger),
        _ => rgb(palette.text_muted),
    };

    let metrics = div()
        .grid()
        .grid_cols(if mode == ProcessDisplayMode::Compact {
            1
        } else if narrow {
            2
        } else {
            4
        })
        .gap_1()
        .child(process_metric(
            palette,
            labels.cpu.clone(),
            format!("{:.1}%", process.cpu_percent),
            usage_color(palette, process.cpu_percent / 100.),
        ))
        .child(process_metric(
            palette,
            labels.memory.clone(),
            format!("{:.1}%", process.memory_percent),
            usage_color(palette, process.memory_percent / 100.),
        ))
        .child(process_metric(
            palette,
            labels.rss.clone(),
            format_file_size(Some(process.rss_kb.saturating_mul(1024))),
            rgb(palette.text).into(),
        ))
        .child(process_metric(
            palette,
            labels.elapsed.clone(),
            process.elapsed.clone(),
            rgb(palette.text_muted).into(),
        ));

    div()
        .w_full()
        .min_w_0()
        .flex_none()
        .whitespace_normal()
        .border_t_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.bg))
        .px_2()
        .py_2()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex_none()
                .flex()
                .items_start()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight(600.))
                                .text_color(rgb(palette.text))
                                .whitespace_normal()
                                .child(process.command.clone()),
                        )
                        .child(
                            div()
                                .font_family(crate::features::shell::gpui_code_font_family())
                                .text_size(px(10.))
                                .text_color(rgb(palette.text_dimmed))
                                .whitespace_normal()
                                .child(format!(
                                    "PID {} · PPID {} · {}",
                                    process.pid, process.ppid, process.user
                                )),
                        ),
                )
                .child(
                    div()
                        .h(px(22.))
                        .px_2()
                        .flex_none()
                        .flex()
                        .items_center()
                        .rounded_md()
                        .border_1()
                        .border_color(state_color)
                        .text_size(px(10.))
                        .font_weight(FontWeight(700.))
                        .text_color(state_color)
                        .child(process.state.clone()),
                ),
        )
        .child(metrics)
        .child(
            div()
                .flex_none()
                .min_w_0()
                .rounded_md()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(rgb(palette.surface))
                .px_2()
                .py_1()
                .flex()
                .flex_col()
                .items_start()
                .gap_2()
                .child(
                    div()
                        .min_w_0()
                        .w_full()
                        .whitespace_normal()
                        .font_family(crate::features::shell::gpui_code_font_family())
                        .text_size(px(10.))
                        .line_height(px(14.))
                        .text_color(rgb(palette.text_muted))
                        .child(command),
                )
                .child(small_button(
                    palette,
                    format!("process-copy-command-{pid}"),
                    labels.copy_command.clone(),
                    on_copy_command,
                )),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .min_w_0()
                        .w(px(80.))
                        .h(px(28.))
                        .flex_none()
                        .on_key_down(cx.listener(|panel, event: &KeyDownEvent, window, cx| {
                            panel.with_app(cx, |this, cx| {
                                if event.keystroke.key.as_str() == "enter" {
                                    cx.stop_propagation();
                                    this.apply_process_nice_draft(window, cx);
                                }
                            });
                        }))
                        .children(nice_input),
                )
                .child(small_button(
                    palette,
                    format!("process-nice-apply-{pid}"),
                    labels.apply_nice.clone(),
                    cx.listener(move |panel, _, window, cx| {
                        panel.with_app(cx, |this, cx| {
                            this.apply_process_nice_draft(window, cx);
                        });
                    }),
                )),
        )
        .into_any_element()
}

fn process_metric(
    palette: ThemePalette,
    label: impl Into<SharedString>,
    value: String,
    color: gpui::Hsla,
) -> impl IntoElement {
    let label: SharedString = label.into();
    div()
        .min_h(px(39.))
        .min_w_0()
        .px_2()
        .py_1()
        .flex()
        .flex_col()
        .child(
            div()
                .text_size(px(9.))
                .text_color(rgb(palette.text_muted))
                .whitespace_normal()
                .child(label),
        )
        .child(
            div()
                .font_family(crate::features::shell::gpui_code_font_family())
                .text_size(px(11.))
                .font_weight(FontWeight(700.))
                .text_color(color)
                .whitespace_normal()
                .child(value),
        )
}
