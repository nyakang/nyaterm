use rust_i18n::t;

use gpui::{Context, IntoElement, KeyDownEvent, SharedString, div, prelude::*, px, rgb, rgba, svg};
use nyaterm_ui::NyaTooltip;

use super::quick_commands_panel::{
    QuickCommandCardContent, QuickCommandCardExecutionMode, quick_command_detail_card,
};
use crate::features::NyaTermApp;

impl NyaTermApp {
    pub(in crate::features) fn quick_command_details_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let details = self
            .commands
            .quick_details()
            .cloned()
            .expect("quick command details overlay requires open state");
        let anchor_x = details.x;
        let anchor_y = details.y;
        let command = details.command;
        let has_description = command
            .description
            .as_deref()
            .is_some_and(|description| !description.trim().is_empty());
        let estimated_h = if has_description { 224. } else { 182. };
        let (viewport_w, viewport_h) = self.shell.viewport_size();
        let (popover_x, popover_y) = quick_command_details_popover_position(
            f32::from(anchor_x) - 304.,
            f32::from(anchor_y) - estimated_h - 6.,
            320.,
            estimated_h,
            viewport_w,
            viewport_h,
        );
        let copy_label = t!("quickCommands.copyCommand");
        let copy_text = command.command.clone();
        let copy_button = div()
            .id(SharedString::from("quick-command-details-copy"))
            .absolute()
            .top(px(6.))
            .right(px(6.))
            .size(px(24.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .cursor_pointer()
            .hover(|this| this.bg(rgb(palette.hover)))
            .child(
                svg()
                    .size(px(13.))
                    .flex_none()
                    .path("icons/copy.svg")
                    .text_color(rgb(palette.text_muted)),
            )
            .tooltip(move |window, cx| NyaTooltip::new(copy_label.clone()).build(window, cx))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.copy_quick_command_text(copy_text.clone(), cx);
            }))
            .into_any_element();

        div()
            .id(SharedString::from("quick-command-details-overlay"))
            .occlude()
            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
            .on_mouse_up(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(gpui::MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(gpui::MouseButton::Middle, |_, _, cx| cx.stop_propagation())
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_quick_command_details(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                cx.stop_propagation();
                if event.keystroke.key == "escape" {
                    this.close_quick_command_details(cx);
                }
            }))
            .child(
                div()
                    .id(SharedString::from("quick-command-details-popover"))
                    .absolute()
                    .left(px(popover_x))
                    .top(px(popover_y))
                    .w(px(320.))
                    .overflow_hidden()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgba((palette.border << 8) | 0x99))
                    .bg(self.shell_surface_color(palette.surface))
                    .shadow_lg()
                    .track_focus(self.commands.quick_details_focus())
                    .on_click(cx.listener(|this, _, window, cx| {
                        window.focus(this.commands.quick_details_focus(), cx);
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .child(quick_command_detail_card(
                        palette,
                        QuickCommandCardContent {
                            command: &command,
                            execution_mode: Some(QuickCommandCardExecutionMode {
                                append: command.execution_mode.as_deref() == Some("append"),
                                label: if command.execution_mode.as_deref() == Some("append") {
                                    t!("quickCommands.appendOnlyBadge")
                                } else {
                                    t!("quickCommands.executeImmediately")
                                },
                            }),
                            copy_button: Some(copy_button),
                        },
                    )),
            )
    }
}

fn quick_command_details_popover_position(
    x: f32,
    y: f32,
    popover_w: f32,
    popover_h: f32,
    viewport_w: f32,
    viewport_h: f32,
) -> (f32, f32) {
    let margin = 8.0;
    let max_x = (viewport_w - popover_w - margin).max(margin);
    let max_y = (viewport_h - popover_h - margin).max(margin);
    (x.clamp(margin, max_x), y.clamp(margin, max_y))
}
