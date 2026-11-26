use gpui::{Context, IntoElement, KeyDownEvent, div, prelude::*, px};
use nyaterm_transport::SessionKind;
use nyaterm_ui::{
    NyaButton, NyaCheckbox, NyaPopover, NyaPopoverAlign, NyaPopoverPlacement, NyaScrollable,
};
use rust_i18n::t;

use super::state::SendCommandBarViewState;
use crate::features::NyaTermApp;
use crate::send_command::{SendCommandSelection, SendCommandSessionClass, SendCommandTarget};

impl NyaTermApp {
    pub(super) fn send_command_target_picker(
        &mut self,
        state: &SendCommandBarViewState,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let selection = match &state.send.target {
            SendCommandTarget::Sessions(selection) => Some(selection),
            _ => None,
        };
        let sessions = self
            .session
            .ordered_sessions()
            .into_iter()
            .filter(|session| self.send_command_session_class(&session.id).is_some())
            .collect::<Vec<_>>();
        let labels = sessions
            .iter()
            .map(|session| self.session.display_name_by_info(session))
            .collect::<Vec<_>>();
        let labels = sessions
            .iter()
            .enumerate()
            .map(|(index, session)| {
                let name = &labels[index];
                let duplicate = labels.iter().filter(|label| *label == name).count() > 1;
                format!(
                    "{name} · {}{}",
                    protocol_label(session.kind),
                    if duplicate {
                        format!(" · #{}", index + 1)
                    } else {
                        String::new()
                    }
                )
            })
            .collect::<Vec<_>>();
        let label = match &state.send.target {
            SendCommandTarget::Current => t!("serialSend.currentSession").to_string(),
            SendCommandTarget::AllCompatible => t!("serialSend.allSessions").to_string(),
            SendCommandTarget::Group(id) => state
                .group_targets
                .iter()
                .find(|(group_id, _, _)| group_id == id)
                .map(|(_, name, count)| {
                    t!("serialSend.groupSession", name = name, count = count).to_string()
                })
                .unwrap_or_else(|| t!("network.group").to_string()),
            SendCommandTarget::Sessions(selected) if selected.ids().len() == 1 => sessions
                .iter()
                .position(|session| selected.contains(&session.id))
                .map(|index| labels[index].clone())
                .unwrap_or_else(|| t!("serialSend.unavailableTargets", count = 1).to_string()),
            SendCommandTarget::Sessions(selected) if selected.ids().is_empty() => {
                t!("serialSend.chooseSessions").to_string()
            }
            SendCommandTarget::Sessions(selected) => {
                t!("serialSend.selectedTargets", count = selected.ids().len()).to_string()
            }
        };
        let mut modes = vec![(
            SendCommandTarget::Current,
            t!("serialSend.currentSession").to_string(),
        )];
        if self.active_session_kind() != Some(SessionKind::Serial) {
            modes.push((
                SendCommandTarget::AllCompatible,
                t!("serialSend.allSessions").to_string(),
            ));
        }
        modes.extend(state.group_targets.iter().map(|(id, name, count)| {
            (
                SendCommandTarget::Group(id.clone()),
                t!("serialSend.groupSession", name = name, count = count).to_string(),
            )
        }));
        let mut content = div()
            .id("send-command.target-content")
            .debug_selector(|| "send-command.target-content".into())
            .w(px(300.))
            .flex()
            .flex_col()
            .gap_1()
            .track_focus(self.send_command.target_picker_focus())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.send_command.set_target_picker_open(false);
                    window.focus(this.send_command.target_trigger_focus(), cx);
                    cx.stop_propagation();
                    cx.notify();
                }
            }));
        for (index, (target, name)) in modes.into_iter().enumerate() {
            content = content.child(
                NyaButton::new(format!("send-command.target-mode-{index}"), name)
                    .full_width()
                    .label_left()
                    .selected(state.send.target == target)
                    .disabled(state.is_sending)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_send_command_target(target.clone(), cx);
                        this.send_command.set_target_picker_open(false);
                        window.focus(this.send_command.target_trigger_focus(), cx);
                    })),
            );
        }
        content = content.child(
            NyaButton::new(
                "send-command.target-manual",
                t!("serialSend.chooseSessions"),
            )
            .full_width()
            .label_left()
            .selected(selection.is_some())
            .disabled(state.is_sending)
            .on_click(cx.listener(|this, _, _, cx| {
                if !matches!(this.send_command.target(), SendCommandTarget::Sessions(_)) {
                    this.set_send_command_target(
                        SendCommandTarget::Sessions(SendCommandSelection::default()),
                        cx,
                    );
                }
            })),
        );
        if let Some(selection) = selection {
            let class = selection.class();
            content = content.child(
                div()
                    .flex()
                    .gap_1()
                    .child(
                        NyaButton::new(
                            "send-command.target-all",
                            t!("serialSend.selectCompatible"),
                        )
                        .disabled(state.is_sending || class.is_none())
                        .on_click(cx.listener(|this, _, _, cx| {
                            let SendCommandTarget::Sessions(selected) = this.send_command.target()
                            else {
                                return;
                            };
                            let Some(class) = selected.class() else {
                                return;
                            };
                            let mut selected = SendCommandSelection::default();
                            for session in this.session.ordered_sessions() {
                                if this.send_command_session_available(&session.id)
                                    && SendCommandSessionClass::for_kind(session.kind)
                                        == Some(class)
                                {
                                    selected.toggle(session.id, session.kind);
                                }
                            }
                            this.set_send_command_target(SendCommandTarget::Sessions(selected), cx);
                        })),
                    )
                    .child(
                        NyaButton::new("send-command.target-clear", t!("serialSend.clearTargets"))
                            .disabled(state.is_sending)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_send_command_target(
                                    SendCommandTarget::Sessions(SendCommandSelection::default()),
                                    cx,
                                )
                            })),
                    ),
            );
            content = content.child(div().text_xs().child(t!("serialSend.targetClassHint")));
            let mut list = div()
                .id("send-command.target-list")
                .max_h(px(240.))
                .flex()
                .flex_col()
                .gap_2()
                .py_1();
            for (index, session) in sessions.iter().enumerate() {
                let checked = selection.contains(&session.id);
                let disconnected = !self.send_command_session_available(&session.id);
                let name = if disconnected {
                    format!("{} · {}", labels[index], t!("serialSend.targetUnavailable"))
                } else {
                    labels[index].clone()
                };
                let id = session.id.clone();
                let kind = session.kind;
                let row_id = format!("send-command.target-row-{id}");
                let checkbox = NyaCheckbox::new(format!("send-command.target-{id}"))
                    .label(name)
                    .checked(checked)
                    .disabled(state.is_sending || (!checked && (disconnected || !selection.accepts(kind))))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // Revalidate after rendering: a session may have closed or moved.
                        let selected = matches!(this.send_command.target(), SendCommandTarget::Sessions(s) if s.contains(&id));
                        if selected || this.send_command_session_available(&id) {
                            this.send_command.toggle_target_session(id.clone(), kind);
                            cx.notify();
                        }
                    }));
                list = list.child(
                    div()
                        .min_w_0()
                        .flex_none()
                        .debug_selector(move || row_id.clone())
                        .child(checkbox),
                );
            }
            for (index, id) in selection
                .ids()
                .iter()
                .filter(|id| !sessions.iter().any(|session| &session.id == *id))
                .enumerate()
            {
                let id = id.clone();
                list = list.child(
                    NyaCheckbox::new(format!("send-command.missing-target-{index}"))
                        .label(t!("serialSend.targetUnavailable"))
                        .checked(true)
                        .disabled(state.is_sending)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send_command
                                .toggle_target_session(id.clone(), SessionKind::Ssh);
                            cx.notify();
                        })),
                );
            }
            content = content.child(list.overflow_y_scrollbar());
        }
        NyaPopover::new(
            "send-command.target-popover",
            div().w(px(190.)).h(px(32.)).child(
                NyaButton::new("send-command.target-trigger", label.clone())
                    .full_width()
                    .label_left()
                    .icon("icons/chevron-down.svg")
                    .height(px(32.))
                    .tooltip(label)
                    .disabled(state.is_sending)
                    .track_focus(self.send_command.target_trigger_focus())
                    .on_click(cx.listener(|this, _, window, cx| {
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
                    })),
            ),
            content,
        )
        .placement(NyaPopoverPlacement::Top)
        .align(NyaPopoverAlign::Start)
        .offset(px(4.))
        .trigger_click(false)
        .open(self.send_command.target_picker_open())
        .on_open_change(cx.listener(|this, open, window, cx| {
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
