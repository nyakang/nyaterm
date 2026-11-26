use gpui::{
    Context, IntoElement, MouseButton, ScrollDelta, ScrollEvent, SharedString, Window, div, point,
    prelude::*, px, rgb, svg,
};
use nyaterm_core::truncate_preview;
use nyaterm_ui::{
    NyaButton, NyaButtonVariant, NyaContextMenu, NyaDialogWindowExt as _, NyaDropdownMenu,
    NyaMenuItem, NyaTooltip,
};
use rust_i18n::t;

use super::super::send_command_compact_tip;
use super::state::{SendCommandBarViewState, SendCommandDensity};
use crate::features::{NyaTermApp, panels::SendCommandTabId, text_inputs::TextInputSetup};

pub(super) fn display_send_tab_name(name: &str) -> String {
    if !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_digit()) {
        format!("#{name}")
    } else {
        name.to_owned()
    }
}

pub(super) fn send_tab_scroll_x(current_x: f32, max_x: f32, delta_x: f32, delta_y: f32) -> f32 {
    let dominant = if delta_x.abs() > delta_y.abs() {
        delta_x
    } else {
        delta_y
    };
    (current_x + dominant).clamp(-max_x.max(0.), 0.)
}

impl NyaTermApp {
    pub(in crate::features) fn select_send_tab(
        &mut self,
        id: SendCommandTabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prefix = self.send_command.input_id("");
        self.dismiss_send_command_selects(&prefix, cx);
        if self.send_command.switch_tab(id) {
            self.focus_send_command_composer(window, cx);
            cx.notify();
        }
    }

    fn close_send_tabs(
        &mut self,
        ids: &[SendCommandTabId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.send_command.close_tabs(ids) {
            return false;
        }
        for id in ids {
            let prefix = id.input_id("");
            self.dismiss_send_command_selects(&prefix, cx);
            self.forget_text_inputs(&prefix);
            self.forget_send_command_selects(&prefix);
        }
        self.focus_send_command_composer(window, cx);
        cx.notify();
        true
    }

    pub(super) fn request_close_send_tabs(
        &mut self,
        ids: Vec<SendCommandTabId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.send_command.can_close_tabs(&ids) {
            return;
        }
        if ids
            .iter()
            .any(|id| self.send_command.tab_has_draft(*id, cx))
        {
            self.open_confirm_dialog(
                (
                    t!("serialSend.tabs.close").into(),
                    t!("serialSend.tabs.discard").into(),
                    t!("common.close").into(),
                    true,
                    move |app, window, cx| app.close_send_tabs(&ids, window, cx),
                ),
                window,
                cx,
            );
        } else {
            self.close_send_tabs(&ids, window, cx);
        }
    }

    pub(super) fn rename_send_tab(
        &mut self,
        id: SendCommandTabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self.send_command.tab_name(id).map(str::to_owned) else {
            return;
        };
        let key = id.input_id("rename");
        self.forget_text_inputs(&key);
        let input = self.text_input(key, &name, TextInputSetup::placeholder(""), cx);
        let render_input = input.clone();
        let submit_input = input.clone();
        self.open_form_dialog(
            (
                t!("serialSend.tabs.rename").into(),
                320.,
                t!("common.save").into(),
                move |_, _, cx| {
                    let app = cx.weak_entity();
                    let input = render_input.clone();
                    div()
                        .w_full()
                        .h(px(32.))
                        .child(nyaterm_ui::NyaInput::new(&render_input).on_enter(
                            move |_, window, cx| {
                                let _ = app.update(cx, |app, cx| {
                                    let name = input.read(cx).value(cx);
                                    app.send_command.rename_tab(id, &name);
                                    app.forget_text_inputs(&id.input_id("rename"));
                                    window.close_nya_dialog(cx);
                                    app.focus_send_command_composer(window, cx);
                                    cx.notify();
                                });
                            },
                        ))
                        .into_any_element()
                },
                move |app, window, cx| {
                    let name = submit_input.read(cx).value(cx).to_string();
                    app.send_command.rename_tab(id, &name);
                    app.forget_text_inputs(&id.input_id("rename"));
                    app.focus_send_command_composer(window, cx);
                    cx.notify();
                    true
                },
                move |app, _| app.forget_text_inputs(&id.input_id("rename")),
            ),
            window,
            cx,
        );
        window.focus(&input.read(cx).focus_handle(), cx);
    }

    fn send_tab_menu(&self, id: SendCommandTabId, cx: &mut Context<Self>) -> Vec<NyaMenuItem> {
        let others = self
            .send_command
            .tab_labels()
            .into_iter()
            .map(|(id, _)| id)
            .filter(|other| *other != id)
            .collect::<Vec<_>>();
        let close = self.send_command.can_close_tabs(&[id]);
        let close_others = self.send_command.can_close_tabs(&others);
        vec![
            NyaMenuItem::action(t!("serialSend.tabs.rename")).on_click(
                cx.listener(move |app, _, window, cx| app.rename_send_tab(id, window, cx)),
            ),
            NyaMenuItem::action(t!("serialSend.tabs.duplicate")).on_click(cx.listener(
                move |app, _, window, cx| {
                    if let Some(new_id) = app.send_command.duplicate_tab(id, cx) {
                        app.select_send_tab(new_id, window, cx);
                    }
                },
            )),
            NyaMenuItem::separator(),
            NyaMenuItem::action(t!("serialSend.tabs.close"))
                .disabled(!close)
                .on_click(cx.listener(move |app, _, window, cx| {
                    app.request_close_send_tabs(vec![id], window, cx)
                })),
            NyaMenuItem::action(t!("serialSend.tabs.closeOthers"))
                .disabled(!close_others)
                .on_click(cx.listener(move |app, _, window, cx| {
                    app.request_close_send_tabs(others.clone(), window, cx)
                })),
        ]
    }

    pub(super) fn send_command_tabs(
        &mut self,
        state: &SendCommandBarViewState,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let labels = self.send_command.tab_labels();
        let multiple = labels.len() > 1;
        let active = self.send_command.active_id();
        let collapsed = state.density == SendCommandDensity::Minimal;
        let overflow = self.send_command.tabs_overflow;
        let scroll = self.send_command.tab_scroll.clone();
        let palette = state.palette;
        let hover_bg = self.shell_surface_color(palette.hover);
        let mut items = Vec::new();
        let mut rows = Vec::new();
        for (id, name) in &labels {
            let id = *id;
            let display_name = display_send_tab_name(name);
            let is_tab_sending = self.send_command.tab_is_sending(id);
            items.push(
                NyaMenuItem::action(if is_tab_sending {
                    format!("● {display_name}")
                } else {
                    display_name.clone()
                })
                .checked(id == active)
                .on_click(
                    cx.listener(move |app, _, window, cx| app.select_send_tab(id, window, cx)),
                ),
            );
            if !multiple || (collapsed && id != active) {
                continue;
            }
            let tab_tip: SharedString = self
                .send_command
                .tab_text_draft(id)
                .and_then(|draft| draft.lines().map(str::trim).find(|line| !line.is_empty()))
                .map(|first_line| format!("{display_name} · {}", truncate_preview(first_line, 36)))
                .unwrap_or_else(|| display_name.clone())
                .into();
            let close_tip: SharedString = if is_tab_sending {
                t!("serialSend.tabs.cannotClose").into()
            } else {
                t!("serialSend.tabs.close").into()
            };
            let group: SharedString = id.input_id("tab").into();
            let close_group: SharedString = id.input_id("close-hover").into();
            let row = div()
                .id(group.clone())
                .debug_selector(move || {
                    if id == active {
                        "send-command.active-tab".into()
                    } else {
                        id.input_id("tab")
                    }
                })
                .group(group.clone())
                .h(px(24.))
                .pl(px(6.))
                .pr(px(3.))
                .gap(px(2.))
                .flex_none()
                .flex()
                .items_center()
                .rounded(px(5.))
                .border_1()
                .cursor_pointer()
                .when(id == active, |row| {
                    row.bg(rgb(palette.surface_elevated))
                        .border_color(rgb(palette.border))
                })
                .when(id != active, |row| {
                    row.border_color(gpui::transparent_black())
                        .hover(move |style| style.bg(hover_bg))
                })
                .tooltip(move |window, cx| {
                    NyaTooltip::new(tab_tip.clone()).compact().build(window, cx)
                })
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |app, _, window, cx| {
                        cx.stop_propagation();
                        app.request_close_send_tabs(vec![id], window, cx);
                    }),
                )
                .on_click(
                    cx.listener(move |app, event: &gpui::ClickEvent, window, cx| {
                        if event.click_count() >= 2 {
                            app.rename_send_tab(id, window, cx);
                        } else {
                            app.select_send_tab(id, window, cx);
                        }
                    }),
                )
                .when(is_tab_sending, |row| {
                    row.child(
                        div()
                            .size(px(6.))
                            .mr(px(1.))
                            .rounded_full()
                            .flex_none()
                            .bg(rgb(palette.accent)),
                    )
                })
                .child(
                    div()
                        .max_w(px(if collapsed { 72. } else { 116. }))
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .text_color(rgb(if id == active {
                            palette.text
                        } else {
                            palette.text_muted
                        }))
                        .child(display_name),
                )
                .child(
                    div()
                        .id(SharedString::from(id.input_id("close")))
                        .group(close_group.clone())
                        .size(px(16.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(3.))
                        .when(id != active, |close| {
                            close
                                .opacity(0.)
                                .group_hover(group, |style| style.opacity(1.))
                        })
                        .when(is_tab_sending, |close| close.opacity(0.4).cursor_default())
                        .when(!is_tab_sending, |close| {
                            close.hover(move |style| style.bg(hover_bg))
                        })
                        .tooltip(move |window, cx| {
                            NyaTooltip::new(close_tip.clone())
                                .compact()
                                .build(window, cx)
                        })
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |app, _, window, cx| {
                            cx.stop_propagation();
                            if !app.send_command.tab_is_sending(id) {
                                app.request_close_send_tabs(vec![id], window, cx);
                            }
                        }))
                        .child(
                            svg()
                                .size(px(11.))
                                .path("icons/window/close.svg")
                                .text_color(rgb(palette.text_muted))
                                .when(!is_tab_sending, |icon| {
                                    icon.group_hover(close_group, |style| {
                                        style.text_color(rgb(palette.danger))
                                    })
                                }),
                        ),
                );
            let app = cx.weak_entity();
            rows.push(
                NyaContextMenu::new_dynamic(row, move |_, cx| {
                    app.update(cx, |app, cx| app.send_tab_menu(id, cx))
                        .unwrap_or_default()
                })
                .into_any_element(),
            );
        }
        let weak = cx.weak_entity();
        let measure_scroll = scroll.clone();
        let scroll_for_wheel = scroll.clone();
        let active_index = labels.iter().position(|(id, _)| *id == active).unwrap_or(0);
        let strip = div()
            .relative()
            .min_w_0()
            .flex_1()
            .overflow_hidden()
            .on_children_prepainted(move |_, _, cx| {
                if collapsed {
                    return;
                }
                let width = f32::from(measure_scroll.bounds().size.width);
                if width <= 0. {
                    return;
                }
                // Subtract the menu's occupied slot when already visible to avoid a
                // feedback loop where showing the menu itself creates overflow.
                let overflows =
                    f32::from(measure_scroll.max_offset().x) > if overflow { 30. } else { 1. };
                let Some(app) = weak.upgrade() else {
                    return;
                };
                let state = &app.read(cx).send_command;
                let resized = (state.tabs_viewport_width - width).abs() > 1.;
                if !resized && state.tabs_overflow == overflows && !state.reveal_active_tab {
                    return;
                }
                let handle = measure_scroll.clone();
                cx.defer(move |cx| {
                    app.update(cx, |app, cx| {
                        let state = &mut app.send_command;
                        if state.active_id() != active {
                            return;
                        }
                        let reveal = resized || state.reveal_active_tab;
                        state.tabs_viewport_width = width;
                        state.tabs_overflow = overflows;
                        state.reveal_active_tab = false;
                        if reveal {
                            handle.scroll_to_item(active_index);
                        }
                        cx.notify();
                    })
                });
            })
            .child(
                div()
                    .id("send-command.tab-scroll")
                    .h(px(26.))
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .min_w_0()
                    // Keep overflow scrollable via wheel and the all-tabs menu
                    // without painting a scrollbar thumb over 24px capsules.
                    .overflow_x_scroll()
                    .overflow_y_hidden()
                    .track_scroll(&scroll)
                    .on_scroll(cx.listener(move |_, event: &ScrollEvent, _, cx| {
                        let (delta_x, delta_y) = match event.delta {
                            ScrollDelta::Lines(delta) => (delta.x * 32., delta.y * 32.),
                            ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y)),
                        };
                        let max_x = f32::from(scroll_for_wheel.max_offset().x).max(0.);
                        if max_x <= 0. || (delta_x == 0. && delta_y == 0.) {
                            return;
                        }
                        let current = scroll_for_wheel.offset();
                        let next_x =
                            send_tab_scroll_x(f32::from(current.x), max_x, delta_x, delta_y);
                        if next_x != f32::from(current.x) {
                            scroll_for_wheel.set_offset(point(px(next_x), px(0.)));
                            cx.stop_propagation();
                        }
                    }))
                    .children(rows),
            );
        div()
            .id("send-command.tabs")
            .debug_selector(|| "send-command.tabs".into())
            .h(px(28.))
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(2.))
            .when(multiple, |row| {
                row.flex_1().when(
                    state.density == SendCommandDensity::Normal
                        || state.density == SendCommandDensity::Expanded,
                    |row| row.max_w(gpui::relative(0.5)),
                )
            })
            .when(!multiple, |row| row.flex_none())
            .when(multiple, |row| row.child(strip))
            .when(multiple && (overflow || collapsed), |row| {
                row.child(
                    send_command_compact_tip(
                        "send-command.all-tabs-tip",
                        t!("serialSend.tabs.all"),
                        NyaDropdownMenu::new("send-command.all-tabs")
                            .icon("icons/chevron-down.svg")
                            .max_height(px(320.))
                            .scrollable(true)
                            .items(items),
                    )
                    .debug_selector(|| "send-command.all-tabs".into())
                    .w(px(28.)),
                )
            })
            .child(
                send_command_compact_tip(
                    "send-command.add-tab-tip",
                    t!("serialSend.tabs.add"),
                    NyaButton::new("send-command.add-tab", "")
                        .icon("icons/conn/add.svg")
                        .variant(NyaButtonVariant::Ghost)
                        .small()
                        .height(px(26.))
                        .accessibility_label(t!("serialSend.tabs.add"))
                        .on_click(cx.listener(|app, _, window, cx| {
                            let prefix = app.send_command.input_id("");
                            app.dismiss_send_command_selects(&prefix, cx);
                            let id = app.send_command.add_tab(cx);
                            app.select_send_tab(id, window, cx);
                        })),
                )
                .debug_selector(|| "send-command.add-tab".into()),
            )
            .into_any_element()
    }
}
