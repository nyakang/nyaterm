//! Shared command preview for hover cards and detail popovers.

use std::borrow::Cow;

use gpui::{
    AnyElement, FontWeight, IntoElement, SharedString, WeakEntity, div, prelude::*, px, rgb, rgba,
    svg,
};
use nyaterm_core::QuickCommand;
use nyaterm_ui::NyaScrollable;

use super::super::quick_command_icon_mark;
use crate::features::NyaTermApp;

pub(in crate::features) struct QuickCommandCardContent<'a> {
    pub command: &'a QuickCommand,
    /// Execution badge beside the title.
    pub execution_mode: Option<QuickCommandCardExecutionMode>,
    /// Built by the caller so this module stays free of listener generics.
    pub copy_button: Option<AnyElement>,
}

#[derive(Clone)]
pub(in crate::features) struct QuickCommandCardExecutionMode {
    pub append: bool,
    pub label: Cow<'static, str>,
}

pub(in crate::features) fn quick_command_detail_card(
    palette: crate::theme::ThemePalette,
    content: QuickCommandCardContent<'_>,
) -> impl IntoElement {
    let QuickCommandCardContent {
        command,
        execution_mode,
        copy_button,
    } = content;
    let description = command
        .description
        .as_deref()
        .map(str::trim)
        .filter(|description| !description.is_empty())
        .map(ToOwned::to_owned);

    div()
        .flex()
        .flex_col()
        .child(
            div()
                .border_b_1()
                .border_color(rgba((palette.border << 8) | 0x4d))
                .bg(rgba((palette.surface_elevated << 8) | 0x4d))
                .p_3()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .size(px(16.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(quick_command_icon_mark(
                                    palette,
                                    command.icon_tag.as_deref(),
                                    command.color_tag.as_deref(),
                                    14.,
                                )),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .text_sm()
                                .font_weight(FontWeight(700.))
                                .text_color(rgb(palette.text))
                                .truncate()
                                .child(command.label.clone()),
                        )
                        .when_some(execution_mode, |this, mode| {
                            this.child(
                                div()
                                    .flex_none()
                                    .rounded_full()
                                    .border_1()
                                    .border_color(rgba((palette.primary << 8) | 0x33))
                                    .bg(rgba((palette.primary << 8) | 0x1a))
                                    .px_2()
                                    .py(px(1.))
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .text_size(px(10.))
                                    .font_weight(FontWeight(600.))
                                    .text_color(rgb(palette.link))
                                    .child(
                                        svg()
                                            .size(px(12.))
                                            .path(if mode.append {
                                                "icons/keyboard-return.svg"
                                            } else {
                                                "icons/conn/flash.svg"
                                            })
                                            .text_color(rgb(palette.link)),
                                    )
                                    .child(mode.label),
                            )
                        }),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .p_3()
                .when_some(description, |this, description| {
                    this.child(
                        div()
                            .text_xs()
                            .line_height(px(18.))
                            .text_color(rgb(palette.text_muted))
                            .child(description),
                    )
                })
                .child(
                    // The copy button is a sibling of the scroll box, not a child:
                    // an absolutely positioned child would ride the scroll offset.
                    div()
                        .relative()
                        .child(
                            div()
                                .id(SharedString::from("quick-command-card-command"))
                                .max_h(px(120.))
                                .rounded_md()
                                .border_1()
                                .border_color(rgba((palette.border << 8) | 0x66))
                                .bg(rgba((palette.bg << 8) | 0x80))
                                .p(px(10.))
                                .when(copy_button.is_some(), |this| this.pr(px(36.)))
                                .font_family(crate::features::shell::gpui_code_font_family())
                                .text_size(px(11.))
                                .line_height(px(17.))
                                .text_color(rgb(palette.text))
                                .child(command.command.clone())
                                .overflow_y_scrollbar(),
                        )
                        .children(copy_button),
                ),
        )
}

/// The hover-card wrapper owns opening and dismissal; this content owns no state.
pub(in crate::features) fn quick_command_hover_card(
    palette: crate::theme::ThemePalette,
    surface: gpui::Rgba,
    command: &QuickCommand,
    execution_mode: QuickCommandCardExecutionMode,
    app: WeakEntity<NyaTermApp>,
) -> impl IntoElement {
    let copy_text = command.command.clone();
    let copy_button = div()
        .id(SharedString::from("quick-command-tile-copy"))
        .debug_selector(|| "quick-command-tile-copy".into())
        .absolute()
        .top(px(6.))
        .right(px(6.))
        .size(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .cursor_pointer()
        .hover(move |this| this.bg(rgb(palette.hover)))
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path("icons/copy.svg")
                .text_color(rgb(palette.text_muted)),
        )
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            let text = copy_text.clone();
            app.update(cx, |app, cx| app.copy_quick_command_text(text, cx))
                .ok();
        })
        .into_any_element();

    div()
        .w(px(320.))
        .id("quick-command-hover-surface")
        .debug_selector(|| "quick-command-hover-surface".into())
        // NyaHoverCard owns the blocking hitbox and hover tracking. A nested
        // occluder or stopped mouse move hides entry from its close timer.
        .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
        .on_mouse_up(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(gpui::MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(gpui::MouseButton::Middle, |_, _, cx| cx.stop_propagation())
        .on_scroll(|_, _, cx| cx.stop_propagation())
        .overflow_hidden()
        .rounded_lg()
        .border_1()
        .border_color(rgba((palette.border << 8) | 0x99))
        .bg(surface)
        .shadow_lg()
        .child(quick_command_detail_card(
            palette,
            QuickCommandCardContent {
                command,
                execution_mode: Some(execution_mode),
                copy_button: Some(copy_button),
            },
        ))
        .into_any_element()
}
