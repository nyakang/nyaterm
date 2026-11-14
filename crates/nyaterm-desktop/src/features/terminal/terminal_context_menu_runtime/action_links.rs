use gpui::{Context, IntoElement, MouseButton, SharedString, div, prelude::*, px, rgb};
use nyaterm_core::mac_address::MacAddressFormat;
use nyaterm_core::truncate_preview;
use nyaterm_ui::{NyaButton, NyaButtonVariant, NyaScrollable, NyaTooltip};

use crate::action_links::ActionLinkTarget;
use crate::features::NyaTermApp;

use super::helpers::{clamp_menu_position, terminal_ctx_item_with_icon};

impl NyaTermApp {
    pub(in crate::features) fn action_link_menu_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let Some(menu) = self.terminal.menus.action_link_menu.clone() else {
            return div().into_any_element();
        };
        if !self.action_link_source_is_active(&menu.session_id) {
            return div().into_any_element();
        }
        let is_mac = menu
            .actions
            .iter()
            .any(|action| action.mac_format.is_some());
        let menu_width = if is_mac { 420. } else { 260. };
        let (viewport_w, viewport_h) = self.shell.viewport_size();
        let (menu_x, menu_y) = clamp_menu_position(
            f32::from(menu.x),
            f32::from(menu.y),
            menu_width,
            360.,
            viewport_w,
            viewport_h,
        );
        let mut items = div()
            .max_h(px(360.))
            .overflow_y_scrollbar()
            .py_1()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(palette.border))
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(palette.text_muted))
                            .child(menu.kind_label.clone()),
                    )
                    .child(
                        div()
                            .mt_1()
                            .font_family(crate::features::shell::gpui_code_font_family())
                            .text_size(px(11.))
                            .text_color(rgb(palette.text))
                            .child(truncate_preview(&menu.value, 42)),
                    ),
            );
        if is_mac {
            for (index, format) in MacAddressFormat::ALL.into_iter().enumerate() {
                let actions = menu
                    .actions
                    .iter()
                    .filter(|action| action.mac_format == Some(format))
                    .collect::<Vec<_>>();
                let Some(first) = actions.first() else {
                    continue;
                };
                let text = first.target.text().to_owned();
                let mut row = div().px_3().py_1().flex().items_center().gap_2().child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_family(crate::features::shell::gpui_code_font_family())
                        .text_size(px(12.))
                        .text_color(rgb(palette.text))
                        .child(text.clone()),
                );
                for action in actions {
                    let target = action.target.clone();
                    let origin = menu.clone();
                    let selector = format!("action-link-menu-{}", action.id);
                    let button = NyaButton::new(
                        SharedString::from(format!("action-link-menu-{}", action.id)),
                        action.label.clone(),
                    )
                    .small()
                    .compact()
                    .variant(NyaButtonVariant::Ghost)
                    .tooltip(format!("{} {text}", action.label))
                    .when(
                        index == 0 && matches!(action.target, ActionLinkTarget::CopyText(_)),
                        |button| button.autofocus(),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.execute_action_link_menu_action(&origin, target.clone(), window, cx);
                    }));
                    row = row.child(
                        div()
                            .flex_none()
                            .debug_selector(move || selector.clone())
                            .child(button),
                    );
                }
                items = items.child(row);
            }
        } else {
            for action in &menu.actions {
                let target = action.target.clone();
                let command_preview = match &target {
                    ActionLinkTarget::ExecuteCommand(command) => Some(command.clone()),
                    _ => None,
                };
                let origin = menu.clone();
                let label = if action.is_default {
                    format!("{} (default)", action.label)
                } else {
                    action.label.clone()
                };
                let row = terminal_ctx_item_with_icon(
                    palette,
                    format!("action-link-menu-{}", action.id),
                    label,
                    Some(nyaterm_ui::icons::IconDef::mono(
                        if matches!(target, ActionLinkTarget::OpenUrl(_)) {
                            "icons/conn/connect.svg"
                        } else {
                            "icons/fe/forward.svg"
                        },
                        palette.text_muted,
                    )),
                    None,
                    cx.listener(move |this, _, window, cx| {
                        this.execute_action_link_menu_action(&origin, target.clone(), window, cx);
                    }),
                )
                .when_some(command_preview, |row, command| {
                    row.tooltip(move |window, cx| {
                        let command = command.clone();
                        NyaTooltip::element(move |window, _| {
                            div()
                                .max_w(px(
                                    (f32::from(window.viewport_size().width) - 48.).clamp(0., 420.)
                                ))
                                .font_family(crate::features::shell::gpui_code_font_family())
                                .whitespace_normal()
                                .child(command.clone())
                        })
                        .build(window, cx)
                    })
                });
                items = items.child(row);
            }
        }
        div()
            .id(SharedString::from("action-link-menu-overlay"))
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.dismiss_action_link_menu(window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, window, cx| {
                    this.dismiss_action_link_menu(window, cx);
                }),
            )
            .child(
                div()
                    .id(SharedString::from("action-link-menu"))
                    .debug_selector(|| "action-link-menu".into())
                    .absolute()
                    .top(px(menu_y))
                    .left(px(menu_x))
                    .w(px(menu_width))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(self.shell_surface_color(palette.surface))
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(items),
            )
            .into_any_element()
    }
}
