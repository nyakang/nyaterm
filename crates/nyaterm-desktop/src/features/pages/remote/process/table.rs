use super::data::{ProcessDisplayMode, process_row_height_px};
use super::resources::{compact_remote_svg_button, process_usage_color as usage_color};
use crate::features::view_widgets::APP_OVERLAY_PRIORITY;
use crate::models::RemoteProcessSortDirection;
use crate::theme::ThemePalette;
use gpui::{
    App, ClickEvent, FontWeight, IntoElement, SharedString, Window, deferred, div, prelude::*, px,
    rgb,
};
use nyaterm_remote_ops::remote_process::RemoteProcess;
use std::borrow::Cow;

pub(in crate::features::pages::remote) fn process_sort_button(
    palette: ThemePalette,
    id: impl Into<String>,
    label: impl Into<SharedString>,
    active: bool,
    direction: RemoteProcessSortDirection,
    numeric: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    // Flat sortable header cell (Tauri table header).
    div()
        .id(gpui::SharedString::from(id.into()))
        .h_full()
        .min_w_0()
        .px_1()
        .flex()
        .items_center()
        .when(numeric, |this| this.justify_end())
        .rounded_sm()
        .text_size(px(10.))
        .font_weight(if active {
            FontWeight(700.)
        } else {
            FontWeight(600.)
        })
        .text_color(if active {
            rgb(palette.text)
        } else {
            rgb(palette.text_dimmed)
        })
        .cursor_pointer()
        .hover(|this| {
            this.bg(rgb(palette.surface_elevated))
                .text_color(rgb(palette.text))
        })
        .child(if active {
            format!("{label} {}", direction.marker())
        } else {
            label.to_string()
        })
        .on_click(on_click)
}

pub(in crate::features::pages::remote) fn process_table_row<
    Select,
    Menu,
    CopyPid,
    CopyCommand,
    Term,
    Hup,
    Stop,
    Cont,
    Kill,
>(
    presentation: ProcessTableRowPresentation,
    process: &RemoteProcess,
    actions: ProcessTableRowActions<
        Select,
        Menu,
        CopyPid,
        CopyCommand,
        Term,
        Hup,
        Stop,
        Cont,
        Kill,
    >,
) -> gpui::Div
where
    Select: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    Menu: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    CopyPid: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    CopyCommand: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    Term: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    Hup: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    Stop: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    Cont: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    Kill: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    let ProcessTableRowPresentation {
        palette,
        menu_bg,
        mode,
        labels,
        selected,
        menu_open,
    } = presentation;
    let ProcessTableRowActions {
        on_select,
        on_menu,
        on_copy_pid,
        on_copy_command,
        on_term,
        on_hup,
        on_stop,
        on_cont,
        on_kill,
    } = actions;
    let row_h = process_row_height_px(mode);

    let menu = div()
        .relative()
        .flex()
        .items_center()
        .justify_end()
        .child(compact_remote_svg_button(
            palette,
            format!("process-menu-{}", process.pid),
            "icons/conn/more.svg",
            labels.more.clone(),
            on_menu,
        ))
        .when(menu_open, |this| {
            this.child(
                deferred(
                    div()
                        .id(gpui::SharedString::from(format!(
                            "process-menu-pop-{}",
                            process.pid
                        )))
                        .absolute()
                        .top(px(26.))
                        .right_0()
                        .w(px(148.))
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(palette.border))
                        .bg(menu_bg)
                        .shadow_lg()
                        .py_1()
                        .flex()
                        .flex_col()
                        .occlude()
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(process_menu_item(
                            palette,
                            format!("process-copy-pid-{}", process.pid),
                            labels.copy_pid.clone(),
                            on_copy_pid,
                        ))
                        .child(process_menu_item(
                            palette,
                            format!("process-copy-cmd-{}", process.pid),
                            labels.copy_command.clone(),
                            on_copy_command,
                        ))
                        .child(process_menu_sep(palette))
                        .child(process_menu_item(
                            palette,
                            format!("process-term-{}", process.pid),
                            labels.signal_term.clone(),
                            on_term,
                        ))
                        .child(process_menu_item(
                            palette,
                            format!("process-hup-{}", process.pid),
                            labels.signal_hup.clone(),
                            on_hup,
                        ))
                        .child(process_menu_item(
                            palette,
                            format!("process-stop-{}", process.pid),
                            labels.signal_stop.clone(),
                            on_stop,
                        ))
                        .child(process_menu_item(
                            palette,
                            format!("process-cont-{}", process.pid),
                            labels.signal_cont.clone(),
                            on_cont,
                        ))
                        .child(process_menu_item(
                            palette,
                            format!("process-kill-{}", process.pid),
                            labels.signal_kill.clone(),
                            on_kill,
                        )),
                )
                .with_priority(APP_OVERLAY_PRIORITY),
            )
        });

    let full_command = process.command.clone();
    let name = div()
        .id(SharedString::from(format!("process-name-{}", process.pid)))
        .min_w_0()
        .truncate()
        .text_size(px(12.))
        .font_weight(FontWeight(500.))
        .text_color(rgb(palette.text))
        .tooltip(move |window, cx| {
            nyaterm_ui::NyaTooltip::new(full_command.clone()).build(window, cx)
        })
        .child(process.command.clone());
    let menu = process_column(Some(28.), menu);
    let body = div()
        .id(SharedString::from(format!("process-row-{}", process.pid)))
        .h(px(row_h))
        .w_full()
        .min_w_0()
        .px_2()
        .flex()
        .items_center()
        .gap_1()
        .cursor_pointer()
        .on_click(on_select);
    let body = if matches!(
        mode,
        ProcessDisplayMode::Compact | ProcessDisplayMode::Narrow
    ) {
        body.flex_col()
            .items_stretch()
            .justify_center()
            .child(
                div()
                    .min_w_0()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().flex_1().min_w_0().child(name))
                    .child(menu),
            )
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .when(mode == ProcessDisplayMode::Compact, |this| this.flex_col())
                    .justify_between()
                    .gap_1()
                    .text_size(px(10.))
                    .font_family(crate::features::shell::gpui_code_font_family())
                    .text_color(rgb(palette.text_muted))
                    .child(format!("PID {}", process.pid))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .gap_2()
                            .child(format!("CPU {:.1}%", process.cpu_percent))
                            .child(format!(
                                "{} {:.1}%",
                                rust_i18n::t!("processManager.sortMemory"),
                                process.memory_percent
                            )),
                    ),
            )
    } else {
        body.child(process_column(None, name))
            .child(process_column(
                Some(72.),
                process_table_cell(palette, process.pid.to_string(), None, true),
            ))
            .child(process_column(
                Some(64.),
                process_table_cell(
                    palette,
                    format!("{:.1}%", process.cpu_percent),
                    Some(usage_color(palette, process.cpu_percent / 100.)),
                    true,
                ),
            ))
            .child(process_column(
                Some(64.),
                process_table_cell(
                    palette,
                    format!("{:.1}%", process.memory_percent),
                    Some(usage_color(palette, process.memory_percent / 100.)),
                    true,
                ),
            ))
            .when(mode == ProcessDisplayMode::Wide, |this| {
                this.child(process_column(
                    Some(72.),
                    process_table_cell(palette, process.user.clone(), None, false),
                ))
            })
            .child(menu)
    };

    div()
        .relative()
        .border_b_1()
        .border_color(rgb(palette.surface_elevated))
        .bg(if selected {
            rgb(palette.hover)
        } else {
            rgb(palette.surface)
        })
        .hover(|this| this.bg(rgb(palette.hover)))
        .when(selected, |this| {
            this.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(2.))
                    .bg(rgb(palette.primary)),
            )
        })
        .child(body)
}

#[derive(Clone)]
pub(in crate::features::pages::remote) struct ProcessTableLabels {
    pub more: Cow<'static, str>,
    pub copy_pid: Cow<'static, str>,
    pub copy_command: Cow<'static, str>,
    pub signal_term: Cow<'static, str>,
    pub signal_hup: Cow<'static, str>,
    pub signal_stop: Cow<'static, str>,
    pub signal_cont: Cow<'static, str>,
    pub signal_kill: Cow<'static, str>,
}

#[derive(Clone)]
pub(in crate::features::pages::remote) struct ProcessTableRowPresentation {
    pub palette: ThemePalette,
    pub menu_bg: gpui::Rgba,
    pub mode: ProcessDisplayMode,
    pub labels: ProcessTableLabels,
    pub selected: bool,
    pub menu_open: bool,
}

pub(in crate::features::pages::remote) struct ProcessTableRowActions<
    Select,
    Menu,
    CopyPid,
    CopyCommand,
    Term,
    Hup,
    Stop,
    Cont,
    Kill,
> {
    pub on_select: Select,
    pub on_menu: Menu,
    pub on_copy_pid: CopyPid,
    pub on_copy_command: CopyCommand,
    pub on_term: Term,
    pub on_hup: Hup,
    pub on_stop: Stop,
    pub on_cont: Cont,
    pub on_kill: Kill,
}

fn process_menu_item(
    palette: ThemePalette,
    id: impl Into<String>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    div()
        .id(gpui::SharedString::from(id.into()))
        .h(px(24.))
        .px_3()
        .flex()
        .items_center()
        .text_size(px(12.))
        .text_color(rgb(palette.text))
        .cursor_pointer()
        .hover(|this| this.bg(rgb(palette.surface_elevated)))
        .on_click(on_click)
        .child(label)
}

fn process_menu_sep(palette: ThemePalette) -> impl IntoElement {
    div().h(px(1.)).mx_2().my_1().bg(rgb(palette.border))
}

pub(in crate::features::pages::remote) fn process_table_cell(
    palette: ThemePalette,
    value: String,
    color: Option<gpui::Hsla>,
    numeric: bool,
) -> impl IntoElement {
    // Tauri ProcessManager numeric columns are mono + right-aligned.
    div()
        .min_w_0()
        .font_family(crate::features::shell::gpui_code_font_family())
        .text_xs()
        .when(numeric, |this| this.text_right())
        .text_color(color.unwrap_or_else(|| rgb(palette.text).into()))
        .truncate()
        .child(value)
}

/// Shared by table headings and rows so the flexible name never competes with numbers.
pub(in crate::features::pages::remote) fn process_column(
    width: Option<f32>,
    content: impl IntoElement,
) -> gpui::Div {
    div()
        .min_w_0()
        .h_full()
        .flex()
        .flex_col()
        .justify_center()
        .when_some(width, |this, width| this.w(px(width)).flex_none())
        .when(width.is_none(), |this| this.flex_1())
        .child(content)
}
