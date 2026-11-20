use crate::features::commands::staged::{StagedCommandHistory, StagedQuickCommandTarget};
use crate::features::terminal::terminal_runtime::StagedCommandInput;
use futures::{FutureExt as _, StreamExt as _};
use std::sync::atomic::Ordering;
use std::time::Duration;

use gpui::{Context, KeyDownEvent};
use nyaterm_transport::SessionKind;

use crate::features::NyaTermApp;
use crate::send_command::{
    SendCommandControlFocus, SendCommandDataType, SendCommandLineEnding, SendCommandMode,
    SendCommandTarget,
};

impl NyaTermApp {
    pub(in crate::features) fn set_send_command_clear_after_send(
        &mut self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if self.send_command.is_sending() {
            return;
        }
        self.settings.set_send_command_clear_after_send(enabled);
        self.persist_ui_layout();
        cx.notify();
    }

    pub(in crate::features) fn apply_send_command_control_input(
        &mut self,
        control_id: &str,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let Some(control) = (match control_id {
            "count" => Some(SendCommandControlFocus::Count),
            "interval" => Some(SendCommandControlFocus::Interval),
            _ => None,
        }) else {
            return;
        };
        let filtered = normalize_send_command_control_input(control, &text);
        if !self
            .send_command
            .apply_control_input(control, filtered.clone())
        {
            let value = self.send_command.synced_control_input(control);
            self.reset_text_input(&format!("send-command.{control_id}"), &value, cx);
            return;
        }
        if filtered != text {
            self.reset_text_input(&format!("send-command.{control_id}"), &filtered, cx);
        }
        cx.notify();
    }

    pub(in crate::features) fn handle_send_command_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        self.mark_user_activity();
        let keystroke = &event.keystroke;
        if keystroke.modifiers.alt || keystroke.modifiers.function {
            return false;
        }
        let accel = keystroke.modifiers.platform || keystroke.modifiers.control;

        // The box owns the text and takes Enter as a newline, the way Tauri's
        // textarea does; Ctrl/Cmd+Enter sends the same payload as the button.
        match keystroke.key.as_str() {
            "enter" if accel => self.send_bottom_command(false, cx),
            "l" if accel => {
                self.send_command.clear_draft(cx);
                if self.send_command.presentation(cx).data_type == SendCommandDataType::Text {
                    self.reset_text_input("send-command.draft", "", cx);
                }
                cx.notify();
            }
            _ => return false,
        }
        true
    }

    /// Textarea edits belong only to the text draft; Hex edits its own bytes.
    pub(in crate::features) fn apply_send_command_draft(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.send_command.apply_draft(text);
        cx.notify();
    }

    pub(in crate::features) fn send_bottom_command(
        &mut self,
        append_enter: bool,
        cx: &mut Context<Self>,
    ) {
        if self.send_command.is_sending() {
            self.stop_send_command(cx);
            return;
        }

        let draft = self.send_command.draft_for_send(append_enter, cx);
        let ids = self.send_command_target_session_ids();
        if ids.is_empty() {
            self.shell
                .set_status("start a connected terminal session before sending");
            cx.notify();
            return;
        }
        let mut plans = Vec::new();
        for id in &ids {
            let kind = self.session.session_info(id).map(|session| session.kind);
            match self.send_command.build_plan(&draft, kind, cx) {
                Ok(plan) => plans.push((id.clone(), plan)),
                Err(error) => {
                    self.shell.set_status(error);
                    cx.notify();
                    return;
                }
            }
        }
        let units_per_round: u64 = plans.iter().map(|(_, plan)| plan.unit_count()).sum();
        if units_per_round == 0 {
            self.shell.set_status("command send is empty");
            cx.notify();
            return;
        }
        let sent_draft = self.send_command.draft_for_send(false, cx);
        let clear_after_send = self.settings.summary().ui_serial_send_clear_after_send;
        let run = self
            .send_command
            .begin_send(units_per_round.min(u32::MAX as u64) as u32);
        let run_id = self.commands.staged.begin(&ids);
        self.commands.staged.hide_run(run_id);
        self.send_command.set_run_id(run_id);
        let mut wake = self.commands.staged.take_wake_receiver(run_id);
        let input = if run.raw_units {
            StagedCommandInput::Bytes
        } else {
            StagedCommandInput::Text
        };
        let current = matches!(self.send_command.target(), SendCommandTarget::Current);
        let interval = Duration::from_secs_f64(run.interval_seconds);
        self.shell
            .set_status(format!("sending {units_per_round} unit(s)"));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let mut plans = plans;
            let mut round = 0u32;
            let mut interrupted_any = false;
            let mut aborted = false;
            let mut failed = 0usize;
            'rounds: loop {
                if run.cancel.load(Ordering::Acquire) {
                    aborted = true;
                    break;
                }
                if !run.infinite && round >= run.rounds {
                    break;
                }
                round = round.saturating_add(1);
                let now = cx.background_executor().now();
                let mut targets = plans
                    .iter()
                    .map(|(id, plan)| {
                        let mut plan = plan.clone();
                        let pending = plan.next_batch(16 * 1024);
                        StagedQuickCommandTarget {
                            source_session_id: current.then(|| id.clone()),
                            session_id: id.clone(),
                            history: StagedCommandHistory::default(),
                            plan,
                            pending,
                            ready_at: now + if round > 1 { interval } else { Duration::ZERO },
                            finished: false,
                        }
                    })
                    .collect::<Vec<_>>();
                let _ = this.update(cx, |this, cx| {
                    this.send_command.set_progress_round(round);
                    cx.notify();
                });
                loop {
                    if run.cancel.load(Ordering::Acquire) {
                        aborted = true;
                        break 'rounds;
                    }
                    let Ok((done, interrupted, wait, progress)) = this.update(cx, |this, cx| {
                        let result =
                            this.poll_quick_command_targets(&mut targets, run_id, input, false, cx);
                        this.send_command.complete_progress_units(result.3);
                        if result.3 > 0 {
                            cx.notify();
                        }
                        result
                    }) else {
                        return;
                    };
                    let _ = progress;
                    interrupted_any |= interrupted;
                    if done {
                        break;
                    }
                    let timer = cx
                        .background_executor()
                        .timer(wait.max(Duration::from_millis(1)))
                        .fuse();
                    let notification = wake.next().fuse();
                    futures::pin_mut!(timer, notification);
                    futures::select_biased! { _ = notification => {}, _ = timer => {} }
                }
                failed += targets
                    .iter()
                    .filter(|target| target.pending.is_some())
                    .count();
                plans.retain(|(id, _)| {
                    targets
                        .iter()
                        .any(|target| &target.session_id == id && target.pending.is_none())
                });
                if plans.is_empty() {
                    aborted |= interrupted_any;
                    break;
                }
                if !run.infinite && round >= run.rounds {
                    break;
                }
                if interval.is_zero() {
                    let timer = cx
                        .background_executor()
                        .timer(Duration::from_millis(1))
                        .fuse();
                    let notification = wake.next().fuse();
                    futures::pin_mut!(timer, notification);
                    futures::select_biased! { _ = notification => {}, _ = timer => {} }
                }
            }
            let _ = this.update(cx, |this, cx| {
                for id in &ids {
                    this.commands.staged.finish(id, run_id);
                }
                this.commands.staged.end_run(run_id);
                aborted |= interrupted_any;
                let progress = this.send_command.finish_send();
                if this.send_command.clear_sent_draft(
                    clear_after_send,
                    !aborted
                        && failed == 0
                        && !run.infinite
                        && !run.cancel.load(Ordering::Acquire)
                        && progress.completed == progress.total,
                    &sent_draft,
                    cx,
                ) && this.send_command.presentation(cx).data_type == SendCommandDataType::Text
                {
                    this.reset_text_input("send-command.draft", "", cx);
                }
                this.shell
                    .set_status(if aborted || run.cancel.load(Ordering::Acquire) {
                        format!("command send stopped at {} unit(s)", progress.completed)
                    } else if failed > 0 {
                        format!("command send failed on {failed} target(s)")
                    } else {
                        format!("command send completed: {round} round(s)")
                    });
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::features) fn stop_send_command(&mut self, cx: &mut Context<Self>) {
        if self.send_command.request_cancel() {
            if let Some(run_id) = self.send_command.run_id() {
                self.commands.staged.cancel_run(run_id);
            }
            self.shell.set_status("stopping command send…".to_string());
            cx.notify();
        }
    }

    pub(in crate::features) fn send_command_target_session_ids(&self) -> Vec<String> {
        // Live sessions only from local metadata (skip disconnected tabs).
        let sessions = self
            .session
            .ordered_sessions()
            .into_iter()
            .filter(|session| {
                !self.session.is_disconnected(&session.id)
                    && !matches!(session.kind, SessionKind::Rdp | SessionKind::Vnc)
            })
            .collect::<Vec<_>>();
        let live_session_ids = sessions
            .iter()
            .map(|session| session.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        let active_kind = self.active_session_kind();
        let is_compatible = |kind: SessionKind| -> bool {
            match active_kind {
                Some(SessionKind::Serial) => matches!(kind, SessionKind::Serial),
                Some(SessionKind::Rdp | SessionKind::Vnc) => false,
                Some(_) => !matches!(kind, SessionKind::Serial),
                None => true,
            }
        };
        match self.send_command.target() {
            SendCommandTarget::Current => self
                .session
                .active_id()
                .filter(|session_id| live_session_ids.contains(*session_id))
                .map(ToOwned::to_owned)
                .into_iter()
                .collect(),
            SendCommandTarget::AllCompatible => {
                if active_kind.is_none() {
                    return Vec::new();
                }
                sessions
                    .into_iter()
                    .filter(|session| is_compatible(session.kind))
                    .map(|session| session.id)
                    .collect()
            }
            SendCommandTarget::Group(group_id) => {
                let Some(group) = self
                    .sync_input
                    .groups()
                    .iter()
                    .find(|group| &group.id == group_id)
                else {
                    return Vec::new();
                };
                if !group.enabled {
                    return Vec::new();
                }
                let paused: std::collections::HashSet<&str> = group
                    .paused_session_ids
                    .iter()
                    .map(String::as_str)
                    .collect();
                let session_kind_by_id: std::collections::HashMap<&str, SessionKind> = sessions
                    .iter()
                    .map(|session| (session.id.as_str(), session.kind))
                    .collect();
                group
                    .session_ids
                    .iter()
                    .filter(|session_id| !paused.contains(session_id.as_str()))
                    .filter(|session_id| {
                        session_kind_by_id
                            .get(session_id.as_str())
                            .copied()
                            .is_some_and(is_compatible)
                    })
                    .cloned()
                    .collect()
            }
        }
    }

    pub(in crate::features) fn send_command_group_target_options(
        &self,
    ) -> Vec<(String, String, usize)> {
        let sessions = self
            .session
            .ordered_sessions()
            .into_iter()
            .filter(|session| {
                !self.session.is_disconnected(&session.id)
                    && !matches!(session.kind, SessionKind::Rdp | SessionKind::Vnc)
            })
            .collect::<Vec<_>>();
        let active_kind = self.active_session_kind();
        let is_compatible = |kind: SessionKind| -> bool {
            match active_kind {
                Some(SessionKind::Serial) => matches!(kind, SessionKind::Serial),
                Some(SessionKind::Rdp | SessionKind::Vnc) => false,
                Some(_) => !matches!(kind, SessionKind::Serial),
                None => true,
            }
        };
        let session_kind_by_id: std::collections::HashMap<&str, SessionKind> = sessions
            .iter()
            .map(|session| (session.id.as_str(), session.kind))
            .collect();
        self.sync_input
            .groups()
            .iter()
            .filter(|group| group.enabled)
            .filter_map(|group| {
                let paused: std::collections::HashSet<&str> = group
                    .paused_session_ids
                    .iter()
                    .map(String::as_str)
                    .collect();
                let count = group
                    .session_ids
                    .iter()
                    .filter(|session_id| !paused.contains(session_id.as_str()))
                    .filter(|session_id| {
                        session_kind_by_id
                            .get(session_id.as_str())
                            .copied()
                            .is_some_and(is_compatible)
                    })
                    .count();
                if count == 0 {
                    None
                } else {
                    Some((group.id.clone(), group.name.clone(), count))
                }
            })
            .collect()
    }

    pub(in crate::features) fn set_send_command_target(
        &mut self,
        target: SendCommandTarget,
        cx: &mut Context<Self>,
    ) {
        if !self.send_command.set_target(target) {
            return;
        }
        let label = match self.send_command.target() {
            SendCommandTarget::Current => "Current".to_string(),
            SendCommandTarget::AllCompatible => "All compatible".to_string(),
            SendCommandTarget::Group(id) => self
                .sync_input
                .groups()
                .iter()
                .find(|group| &group.id == id)
                .map(|group| format!("Group: {}", group.name))
                .unwrap_or_else(|| "Group".to_string()),
        };
        self.shell
            .set_status(format!("command send target: {label}"));
        cx.notify();
    }

    pub(in crate::features) fn active_session_kind(&self) -> Option<SessionKind> {
        let active_id = self.session.active_id()?;
        if self.session.is_disconnected(active_id) {
            return None;
        }
        self.session
            .session_info(active_id)
            .map(|session| session.kind)
    }

    pub(in crate::features) fn set_send_command_data_type(
        &mut self,
        data_type: SendCommandDataType,
        cx: &mut Context<Self>,
    ) {
        let Some(interval) = self.send_command.set_data_type(data_type) else {
            return;
        };
        self.reset_text_input("send-command.interval", &interval, cx);
        self.shell.set_status(format!(
            "command send data: {}",
            match data_type {
                SendCommandDataType::Text => "Text",
                SendCommandDataType::Hex => "Hex",
            }
        ));
        cx.notify();
    }

    pub(in crate::features) fn set_send_command_mode(
        &mut self,
        mode: SendCommandMode,
        cx: &mut Context<Self>,
    ) {
        let Some(interval) = self.send_command.set_mode(mode) else {
            return;
        };
        self.reset_text_input("send-command.interval", &interval, cx);
        cx.notify();
    }

    pub(in crate::features) fn set_send_command_line_ending(
        &mut self,
        line_ending: SendCommandLineEnding,
        cx: &mut Context<Self>,
    ) {
        if self.send_command.set_line_ending(line_ending) {
            cx.notify();
        }
    }
}

fn normalize_send_command_control_input(control: SendCommandControlFocus, text: &str) -> String {
    match control {
        SendCommandControlFocus::Count => text
            .chars()
            .filter(|ch| {
                ch.is_ascii_digit() || matches!(ch, 'i' | 'n' | 'f' | 'I' | 'N' | 'F' | '∞')
            })
            .collect(),
        SendCommandControlFocus::Interval => text
            .chars()
            .filter(|ch| ch.is_ascii_digit() || *ch == '.')
            .collect(),
    }
}

#[cfg(test)]
mod control_input_tests {
    use super::{SendCommandControlFocus, normalize_send_command_control_input};

    #[test]
    fn count_input_keeps_numbers_and_infinity_spellings() {
        assert_eq!(
            normalize_send_command_control_input(SendCommandControlFocus::Count, "12 x INF ∞"),
            "12INF∞"
        );
    }

    #[test]
    fn interval_input_keeps_decimal_characters() {
        assert_eq!(
            normalize_send_command_control_input(SendCommandControlFocus::Interval, "1s.25"),
            "1.25"
        );
    }
}
