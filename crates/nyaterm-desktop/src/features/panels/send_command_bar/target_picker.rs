use gpui::{Context, FontWeight, IntoElement, KeyDownEvent, div, prelude::*, px, rgb, rgba};
use nyaterm_transport::SessionKind;
use nyaterm_ui::{
    NyaButton, NyaButtonVariant, NyaCheckbox, NyaPopover, NyaPopoverAlign, NyaPopoverPlacement,
    NyaScrollable, NyaTooltip,
};
use rust_i18n::t;

use super::state::SendCommandBarViewState;
use crate::features::NyaTermApp;
use crate::send_command::{SendCommandSelection, SendCommandTarget};

impl NyaTermApp {
    pub(super) fn send_command_target_picker(
        &mut self,
        state: &SendCommandBarViewState,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tab_id = state.send.tab_id;
        let palette = state.palette;
        let selection = match &state.send.target {
            SendCommandTarget::Sessions(selection) => Some(selection),
            _ => None,
        };
        let sessions = self
            .session
            .ordered_sessions()
            .into_iter()
            .filter(|session| self.send_command_session_supported(&session.id))
            .collect::<Vec<_>>();
        let raw_names = sessions
            .iter()
            .map(|session| self.session.display_name_by_info(session))
            .collect::<Vec<_>>();
        let disambiguated_names = sessions
            .iter()
            .enumerate()
            .map(|(index, _)| {
                let name = &raw_names[index];
                let duplicate = raw_names.iter().filter(|label| *label == name).count() > 1;
                if duplicate {
                    format!("{name} · #{}", index + 1)
                } else {
                    name.clone()
                }
            })
            .collect::<Vec<_>>();
        let full_labels = sessions
            .iter()
            .enumerate()
            .map(|(index, session)| {
                let protocol = protocol_label(session.kind);
                if protocol.is_empty() {
                    disambiguated_names[index].clone()
                } else {
                    format!("{} · {protocol}", disambiguated_names[index])
                }
            })
            .collect::<Vec<_>>();

        let empty_manual_selection = selection.is_some_and(|selected| selected.ids().is_empty());
        let (label, tooltip) = match &state.send.target {
            SendCommandTarget::Current => (
                t!("serialSend.currentSession").to_string(),
                t!("serialSend.target").to_string(),
            ),
            SendCommandTarget::AllSessions => (
                t!("serialSend.allSessions").to_string(),
                t!("serialSend.target").to_string(),
            ),
            SendCommandTarget::Group(id) => {
                let text = state
                    .group_targets
                    .iter()
                    .find(|(group_id, _, _)| group_id == id)
                    .map(|(_, name, count)| {
                        t!("serialSend.groupSession", name = name, count = count).to_string()
                    })
                    .unwrap_or_else(|| t!("network.group").to_string());
                let tip = format!("{} · {text}", t!("serialSend.target"));
                (text, tip)
            }
            SendCommandTarget::Sessions(selected) if selected.ids().is_empty() => (
                t!("serialSend.chooseSessions").to_string(),
                format!(
                    "{} · {}",
                    t!("serialSend.target"),
                    t!("serialSend.noTarget")
                ),
            ),
            SendCommandTarget::Sessions(selected) if selected.ids().len() == 1 => {
                let text = sessions
                    .iter()
                    .position(|session| selected.contains(&session.id))
                    .map(|index| full_labels[index].clone())
                    .unwrap_or_else(|| t!("serialSend.unavailableTargets", count = 1).to_string());
                let tip = format!("{} · {text}", t!("serialSend.target"));
                (text, tip)
            }
            SendCommandTarget::Sessions(selected) => {
                let text =
                    t!("serialSend.selectedTargets", count = selected.ids().len()).to_string();
                let picked = sessions
                    .iter()
                    .enumerate()
                    .filter(|(_, session)| selected.contains(&session.id))
                    .map(|(index, _)| full_labels[index].as_str())
                    .collect::<Vec<_>>();
                let tip = if picked.is_empty() {
                    format!("{} · {text}", t!("serialSend.target"))
                } else {
                    format!("{text}：{}", picked.join("、"))
                };
                (text, tip)
            }
        };

        let mut modes = vec![
            (
                SendCommandTarget::Current,
                t!("serialSend.currentSession").to_string(),
            ),
            (
                SendCommandTarget::AllSessions,
                t!("serialSend.allSessions").to_string(),
            ),
        ];
        modes.extend(state.group_targets.iter().map(|(id, name, count)| {
            (
                SendCommandTarget::Group(id.clone()),
                t!("serialSend.groupSession", name = name, count = count).to_string(),
            )
        }));

        let mut content = div()
            .id("send-command.target-content")
            .debug_selector(|| "send-command.target-content".into())
            .w(px(268.))
            .p(px(6.))
            .bg(rgb(palette.surface_elevated))
            .border_1()
            .border_color(rgb(palette.border))
            .rounded_md()
            .shadow_md()
            .text_color(rgb(palette.text))
            .flex()
            .flex_col()
            .gap(px(2.))
            .track_focus(self.send_command.target_picker_focus())
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if this.send_command.active_id() != tab_id {
                    return;
                }
                if event.keystroke.key == "escape" {
                    this.send_command.set_target_picker_open(false);
                    window.focus(this.send_command.target_trigger_focus(), cx);
                    cx.stop_propagation();
                    cx.notify();
                }
            }));

        for (index, (target, name)) in modes.into_iter().enumerate() {
            let is_active = state.send.target == target;
            let mut button = NyaButton::new(format!("send-command.target-mode-{index}"), name)
                .variant(NyaButtonVariant::Ghost)
                .height(px(28.))
                .text_size(px(12.))
                .full_width()
                .label_left()
                .selected(is_active)
                .disabled(state.is_sending)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.send_command.active_id() != tab_id {
                        return;
                    }
                    this.set_send_command_target(target.clone(), cx);
                    this.send_command.set_target_picker_open(false);
                    window.focus(this.send_command.target_trigger_focus(), cx);
                }));
            if is_active {
                button = button.content(nyaterm_ui::icons::mono_icon(
                    "icons/check.svg",
                    rgb(palette.primary).into(),
                    14.,
                ));
            }
            content = content.child(
                div()
                    .id(format!("send-command.target-mode-row-{index}"))
                    .child(button),
            );
        }

        content = content.child(div().h(px(1.)).mx_1().my(px(4.)).bg(rgb(palette.border)));

        let empty_selection = SendCommandSelection::default();
        let selection_value = selection.unwrap_or(&empty_selection);
        let selected_count = selection_value.ids().len();
        let any_available = sessions
            .iter()
            .any(|session| self.send_command_session_available(&session.id));

        content = content.child(
            div()
                .id("send-command.target-actions")
                .debug_selector(|| "send-command.target-actions".into())
                .h(px(24.))
                .w_full()
                .min_w_0()
                .px_2()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(11.))
                                .font_weight(FontWeight(600.))
                                .text_color(rgb(palette.text_muted))
                                .child(t!("serialSend.chooseSessions")),
                        )
                        .when(selected_count > 0, |row| {
                            row.child(
                                div()
                                    .flex_none()
                                    .px_1p5()
                                    .rounded(px(99.))
                                    .bg(rgb(palette.surface))
                                    .text_size(px(10.))
                                    .font_weight(FontWeight(600.))
                                    .text_color(rgb(palette.primary))
                                    .child(selected_count.to_string()),
                            )
                        }),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_0p5()
                        .child(
                            div()
                                .debug_selector(|| "send-command.target-all-action".into())
                                .child(
                                    NyaButton::new(
                                        "send-command.target-all",
                                        t!("serialSend.selectAllTargets"),
                                    )
                                    .variant(NyaButtonVariant::Ghost)
                                    .small()
                                    .compact()
                                    .height(px(22.))
                                    .disabled(state.is_sending || !any_available)
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            if this.send_command.active_id() != tab_id {
                                                return;
                                            }
                                            let mut selected = SendCommandSelection::default();
                                            for session in this.session.ordered_sessions() {
                                                if this.send_command_session_available(&session.id)
                                                {
                                                    selected.toggle(session.id, session.kind);
                                                }
                                            }
                                            this.set_send_command_target(
                                                SendCommandTarget::Sessions(selected),
                                                cx,
                                            );
                                        },
                                    )),
                                ),
                        )
                        .child(
                            div()
                                .debug_selector(|| "send-command.target-clear-action".into())
                                .child(
                                    NyaButton::new(
                                        "send-command.target-clear",
                                        t!("serialSend.clearTargets"),
                                    )
                                    .variant(NyaButtonVariant::Ghost)
                                    .small()
                                    .compact()
                                    .height(px(22.))
                                    .disabled(state.is_sending || selection_value.ids().is_empty())
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            if this.send_command.active_id() != tab_id {
                                                return;
                                            }
                                            this.set_send_command_target(
                                                SendCommandTarget::Sessions(
                                                    SendCommandSelection::default(),
                                                ),
                                                cx,
                                            )
                                        },
                                    )),
                                ),
                        ),
                ),
        );

        let missing_ids = selection_value
            .ids()
            .iter()
            .filter(|id| !sessions.iter().any(|session| &session.id == *id))
            .cloned()
            .collect::<Vec<_>>();

        if sessions.is_empty() && missing_ids.is_empty() {
            content = content.child(
                div()
                    .id("send-command.target-list")
                    .debug_selector(|| "send-command.target-list".into())
                    .h(px(32.))
                    .px_2()
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!("serialSend.noTarget")),
            );
        } else {
            let mut list = div()
                .id("send-command.target-list")
                .debug_selector(|| "send-command.target-list".into())
                .max_h(px(216.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .pt(px(2.));
            for (index, session) in sessions.iter().enumerate() {
                let checked = selection_value.contains(&session.id);
                let disconnected = !self.send_command_session_available(&session.id);
                let protocol = protocol_label(session.kind);
                let detail = match (protocol.is_empty(), disconnected) {
                    (true, false) => String::new(),
                    (false, false) => protocol.to_string(),
                    (true, true) => t!("serialSend.targetUnavailable").to_string(),
                    (false, true) => format!("{protocol} · {}", t!("serialSend.targetUnavailable")),
                };
                let id = session.id.clone();
                let kind = session.kind;
                let row_id = format!("send-command.target-row-{id}");
                let mut checkbox = NyaCheckbox::new(format!("send-command.target-{id}"))
                    .menu_row()
                    .label(disambiguated_names[index].clone())
                    .checked(checked)
                    .disabled(state.is_sending || (!checked && disconnected))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.send_command.active_id() != tab_id {
                            return;
                        }
                        // Revalidate after rendering: a session may have closed or moved.
                        let selected = matches!(
                            this.send_command.target(),
                            SendCommandTarget::Sessions(s) if s.contains(&id)
                        );
                        if selected || this.send_command_session_available(&id) {
                            this.send_command.toggle_target_session(id.clone(), kind);
                            cx.notify();
                        }
                    }));
                if !detail.is_empty() {
                    checkbox = checkbox.detail(detail);
                }
                list = list.child(
                    div()
                        .min_w_0()
                        .flex_none()
                        .debug_selector(move || row_id.clone())
                        .child(checkbox),
                );
            }
            for (index, id) in missing_ids.into_iter().enumerate() {
                list = list.child(
                    div().min_w_0().flex_none().child(
                        NyaCheckbox::new(format!("send-command.missing-target-{index}"))
                            .menu_row()
                            .label(t!("serialSend.targetUnavailable"))
                            .checked(true)
                            .disabled(state.is_sending)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.send_command.active_id() != tab_id {
                                    return;
                                }
                                this.send_command
                                    .toggle_target_session(id.clone(), SessionKind::Ssh);
                                cx.notify();
                            })),
                    ),
                );
            }
            content = content.child(list.overflow_y_scrollbar());
        }

        let is_broadcast = state.is_broadcast;
        let broadcast_active = is_broadcast && state.target_count > 0;
        let trailing = div()
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .when(broadcast_active, |row| {
                row.child(
                    div()
                        .flex_none()
                        .px_1p5()
                        .rounded(px(99.))
                        .bg(rgba((palette.warning << 8) | 0x24))
                        .text_size(px(10.))
                        .line_height(px(14.))
                        .font_weight(FontWeight(600.))
                        .text_color(rgb(palette.warning))
                        .child(format!("×{}", state.target_count)),
                )
            })
            .child(nyaterm_ui::icons::mono_icon(
                "icons/chevron-down.svg",
                rgb(if broadcast_active {
                    palette.warning
                } else {
                    palette.text_muted
                })
                .into(),
                12.,
            ));
        let slot_width = if state.send.viewport_width > 0. && state.send.viewport_width < 520. {
            140.
        } else {
            match &state.send.target {
                SendCommandTarget::Current => 148.,
                SendCommandTarget::AllSessions => 168.,
                _ => 188.,
            }
        };
        let accessible_label = label.clone();
        let mut trigger_button = NyaButton::new("send-command.target-trigger", label)
            .icon(if is_broadcast {
                "icons/session/sync-input.svg"
            } else {
                "icons/settings/terminal.svg"
            })
            .input_surface()
            .full_width()
            .label_left()
            .content(trailing)
            .height(px(32.))
            .accessibility_label(accessible_label)
            .disabled(state.is_sending)
            .track_focus(self.send_command.target_trigger_focus())
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.send_command.active_id() != tab_id {
                    return;
                }
                let open = !this.send_command.target_picker_open();
                this.send_command.set_target_picker_open(open);
                window.focus(
                    if open {
                        this.send_command.target_picker_focus()
                    } else {
                        this.send_command.target_trigger_focus()
                    },
                    cx,
                );
                cx.notify();
            }));
        if empty_manual_selection {
            trigger_button = trigger_button.text_color(rgb(palette.text_muted));
        } else if broadcast_active {
            trigger_button = trigger_button.text_color(rgb(palette.warning));
        }

        let trigger_tip: gpui::SharedString = tooltip.into();
        NyaPopover::new(
            tab_id.input_id("target-popover"),
            div()
                .id("send-command.target-trigger-slot")
                .debug_selector(|| "send-command.target-trigger-slot".into())
                .w(px(slot_width))
                .h(px(32.))
                .flex_none()
                .rounded(px(6.))
                .when(broadcast_active, |slot| {
                    slot.border_1()
                        .border_color(rgba((palette.warning << 8) | 0x80))
                })
                .tooltip(move |window, cx| {
                    NyaTooltip::new(trigger_tip.clone())
                        .compact()
                        .build(window, cx)
                })
                .child(trigger_button),
            content,
        )
        .appearance(false)
        .placement(NyaPopoverPlacement::Top)
        .align(NyaPopoverAlign::Start)
        .offset(px(4.))
        .trigger_click(false)
        .open(self.send_command.target_picker_open())
        .on_open_change(cx.listener(move |this, open, window, cx| {
            if this.send_command.active_id() != tab_id {
                return;
            }
            this.send_command.set_target_picker_open(*open);
            if !open {
                window.focus(this.send_command.target_trigger_focus(), cx);
            }
            cx.notify();
        }))
        .into_any_element()
    }
}

fn protocol_label(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::Ssh => "SSH",
        SessionKind::LocalPty => "Local",
        SessionKind::Telnet => "Telnet",
        SessionKind::RawTcp => "TCP",
        SessionKind::Serial => "Serial",
        SessionKind::AdbShell => "ADB",
        _ => "",
    }
}
