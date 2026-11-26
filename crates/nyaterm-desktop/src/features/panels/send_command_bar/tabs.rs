use gpui::{Context, IntoElement, SharedString, Window, div, prelude::*, px, rgb};
use nyaterm_ui::{
    NyaButton, NyaButtonVariant, NyaContextMenu, NyaDialogWindowExt as _, NyaDropdownMenu,
    NyaMenuItem, NyaScrollable,
};
use rust_i18n::t;

use super::state::{SendCommandBarViewState, SendCommandDensity};
use crate::features::{NyaTermApp, panels::SendCommandTabId, text_inputs::TextInputSetup};

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
        let mut items = Vec::new();
        let mut rows = Vec::new();
        for (id, name) in &labels {
            let id = *id;
            items.push(
                NyaMenuItem::action(name.clone())
                    .checked(id == active)
                    .on_click(
                        cx.listener(move |app, _, window, cx| app.select_send_tab(id, window, cx)),
                    ),
            );
            if !multiple || (collapsed && id != active) {
                continue;
            }
            let group: SharedString = id.input_id("tab").into();
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
                .h(px(26.))
                .flex_none()
                .flex()
                .items_center()
                .rounded(px(3.))
                .when(id == active, |row| row.bg(rgb(palette.surface_elevated)))
                .child(
                    div()
                        .max_w(px(if collapsed { 80. } else { 128. }))
                        .min_w_0()
                        .overflow_hidden()
                        .child(
                            NyaButton::new(
                                id.input_id("label"),
                                if self.send_command.tab_is_sending(id) {
                                    format!("● {name}")
                                } else {
                                    name.clone()
                                },
                            )
                            .variant(NyaButtonVariant::Ghost)
                            .small()
                            .compact()
                            .height(px(26.))
                            .text_color(rgb(if id == active {
                                palette.text
                            } else {
                                palette.text_muted
                            }))
                            .tooltip(name.clone())
                            .on_click(cx.listener(
                                move |app, event: &gpui::ClickEvent, window, cx| {
                                    if event.click_count() >= 2 {
                                        app.rename_send_tab(id, window, cx);
                                    } else {
                                        app.select_send_tab(id, window, cx);
                                    }
                                },
                            )),
                        ),
                )
                .child(
                    div()
                        .w(px(22.))
                        .h(px(26.))
                        .flex_none()
                        .when(id != active, |close| {
                            close
                                .opacity(0.)
                                .group_hover(group, |style| style.opacity(1.))
                        })
                        .child(
                            NyaButton::new(id.input_id("close"), "")
                                .icon("icons/close.svg")
                                .variant(NyaButtonVariant::Ghost)
                                .small()
                                .height(px(26.))
                                .disabled(self.send_command.tab_is_sending(id))
                                .tooltip(if self.send_command.tab_is_sending(id) {
                                    t!("serialSend.tabs.cannotClose")
                                } else {
                                    t!("serialSend.tabs.close")
                                })
                                .on_click(cx.listener(move |app, _, window, cx| {
                                    cx.stop_propagation();
                                    app.request_close_send_tabs(vec![id], window, cx);
                                })),
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
                    .min_w_0()
                    .overflow_x_scroll()
                    .track_scroll(&scroll)
                    .children(rows),
            )
            .horizontal_scrollbar(&scroll);
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
                    div()
                        .debug_selector(|| "send-command.all-tabs".into())
                        .w(px(28.))
                        .flex_none()
                        .child(
                            NyaDropdownMenu::new("send-command.all-tabs")
                                .icon("icons/chevron-down.svg")
                                .tooltip(t!("serialSend.tabs.all"))
                                .max_height(px(320.))
                                .scrollable(true)
                                .items(items),
                        ),
                )
            })
            .child(
                div()
                    .debug_selector(|| "send-command.add-tab".into())
                    .flex_none()
                    .child(
                        NyaButton::new("send-command.add-tab", "")
                            .icon("icons/conn/add.svg")
                            .variant(NyaButtonVariant::Ghost)
                            .small()
                            .height(px(26.))
                            .tooltip(t!("serialSend.tabs.add"))
                            .on_click(cx.listener(|app, _, window, cx| {
                                let prefix = app.send_command.input_id("");
                                app.dismiss_send_command_selects(&prefix, cx);
                                let id = app.send_command.add_tab(cx);
                                app.select_send_tab(id, window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }
}
