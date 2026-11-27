use rust_i18n::t;

use std::borrow::Cow;

use nyaterm_core::command_draft::DraftOrigin;

use crate::features::{NyaTermApp, panels::SendCommandPresentationState};
use crate::send_command::{SendCommandDataType, SendCommandMode, SendCommandTarget};

use super::tabs::display_send_tab_name;

pub(super) struct SendCommandBarViewState {
    pub(super) send: SendCommandPresentationState,
    pub(super) palette: crate::theme::ThemePalette,
    pub(super) group_targets: Vec<(String, String, usize)>,
    pub(super) target_count: usize,
    pub(super) is_broadcast: bool,
    pub(super) show_line_ending: bool,
    pub(super) validation_error: bool,
    pub(super) status: String,
    pub(super) send_disabled: bool,
    pub(super) action_hint: String,
    pub(super) density: SendCommandDensity,
    pub(super) input_hint: Cow<'static, str>,
    pub(super) is_sending: bool,
    pub(super) progress_ratio: f32,
    pub(super) progress_label: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SendCommandDensity {
    Minimal,
    Compact,
    Normal,
    Expanded,
}

impl SendCommandDensity {
    fn for_size(width: f32, height: f32) -> Self {
        if width > 0. && width < 360. {
            Self::Minimal
        } else if height < 160. || (width > 0. && width < 500.) {
            Self::Compact
        } else if height >= 300. && width >= 700. {
            Self::Expanded
        } else {
            Self::Normal
        }
    }
}

impl NyaTermApp {
    pub(super) fn send_command_bar_view_state(&self, cx: &gpui::App) -> SendCommandBarViewState {
        let send = self.send_command.view_presentation(cx);
        let palette = self.theme_palette();
        let group_targets = self.send_command_group_target_options();
        let show_line_ending = self.send_command_has_configurable_line_ending()
            && send.data_type == SendCommandDataType::Text
            && send.mode == SendCommandMode::Line;
        let unit_result = self.send_command_target_statistics(cx);
        let (validation_error, unit_count, byte_count) = match &unit_result {
            Ok(stats) => (false, stats.units, stats.bytes),
            Err(_) => (true, 0usize, 0usize),
        };
        let hex = send.hex.read(cx);
        let density =
            SendCommandDensity::for_size(send.viewport_width, self.shell.command_send_height());
        let has_payload = if send.data_type == SendCommandDataType::Hex {
            !hex.document().bytes().is_empty()
        } else {
            !send.draft.is_empty()
        };
        let hex_error = send.data_type == SendCommandDataType::Hex && hex.error().is_some();
        let interval_error =
            nyaterm_core::send_command::parse_interval_ms(&send.interval_input).is_none();
        let validation_error = validation_error || hex_error;
        let target_ids = self.send_command_target_session_ids();
        let target_count = target_ids.len();
        let target_available = !target_ids.is_empty();
        let is_broadcast = match &send.target {
            SendCommandTarget::AllSessions | SendCommandTarget::Group(_) => true,
            SendCommandTarget::Sessions(selected) => selected.ids().len() > 1,
            SendCommandTarget::Current => false,
        };
        let send_disabled =
            !send.sending && (validation_error || !has_payload || !target_available);
        let action_hint = if send.sending {
            let tab_label = self
                .send_command
                .tab_name(send.tab_id)
                .map(display_send_tab_name)
                .unwrap_or_default();
            t!("serialSend.tabs.stop", name = tab_label).to_string()
        } else if interval_error {
            t!("quickCommands.errorSegmentedIntervalInvalid").to_string()
        } else if validation_error {
            t!("serialSend.hexError").to_string()
        } else if !has_payload {
            format!("{} · {}", t!("serialSend.payloadEmpty"), send_shortcut())
        } else if !target_available {
            format!("{} · {}", t!("serialSend.noTarget"), send_shortcut())
        } else if is_broadcast && target_count > 1 {
            format!(
                "{} ({target_count}) · {}",
                t!("serialSend.send"),
                send_shortcut()
            )
        } else {
            format!("{} · {}", t!("serialSend.send"), send_shortcut())
        };
        let mut status = if interval_error {
            t!("quickCommands.errorSegmentedIntervalInvalid").to_string()
        } else if hex_error {
            t!("serialSend.invalidPaste").to_string()
        } else if validation_error {
            t!("serialSend.incompleteByte").to_string()
        } else if send.data_type == SendCommandDataType::Hex {
            let doc = hex.document();
            if let Some(selection) = doc.selection() {
                t!("serialSend.selectedBytes", count = selection.len()).to_string()
            } else {
                let count = t!("serialSend.hexByteCount", count = doc.bytes().len());
                let index = doc.cursor().byte_index;
                match doc.bytes().get(index) {
                    Some(byte) => format!("{count} · 0x{index:02X}: {byte:02X}"),
                    None => count.to_string(),
                }
            }
        } else {
            if send.mode == SendCommandMode::Character {
                t!(
                    "serialSend.payloadStatsChars",
                    units = unit_count,
                    bytes = byte_count
                )
            } else {
                t!(
                    "serialSend.payloadStatsLines",
                    units = unit_count,
                    bytes = byte_count
                )
            }
            .to_string()
        };
        if target_count > 1 && !validation_error && !interval_error {
            let summary = t!(
                "serialSend.targetPayloadStats",
                count = target_count,
                units = unit_count,
                bytes = byte_count
            );
            if send.data_type == SendCommandDataType::Hex {
                status.push_str(" · ");
                status.push_str(&summary);
            } else {
                status = summary.to_string();
            }
        }
        if let SendCommandTarget::Sessions(selection) = &send.target {
            let unavailable = selection.ids().len().saturating_sub(target_count);
            if unavailable > 0 {
                status.push_str(" · ");
                status.push_str(&t!("serialSend.unavailableTargets", count = unavailable));
            }
        }
        let sources = send
            .provenance
            .origins
            .iter()
            .filter_map(|origin| match origin {
                DraftOrigin::Plugin {
                    plugin_id,
                    action_id,
                    ..
                } => Some(format!("{plugin_id}:{action_id}")),
                DraftOrigin::User => None,
            })
            .collect::<Vec<_>>();
        if !sources.is_empty() {
            status.push_str(" · ");
            status.push_str(&t!("plugins.draftOrigin", source = sources.join(", ")));
            if send.provenance.edited {
                status.push_str(" · ");
                status.push_str(&t!("plugins.draftEdited"));
            }
        }
        let input_hint = if send.data_type == SendCommandDataType::Hex {
            Cow::Owned(send_hex_placeholder())
        } else {
            Cow::Owned(send_text_placeholder())
        };
        let is_sending = send.sending;
        let infinite_progress = is_sending && send.rounds == 0;
        let progress_total = send.total.max(1);
        let progress_completed = if infinite_progress {
            send.completed
        } else {
            send.completed.min(progress_total)
        };
        let progress_ratio = if infinite_progress {
            // Indeterminate-ish pulse from completed units.
            (((progress_completed % 20) as f32) / 20.0).clamp(0.08, 0.95)
        } else {
            progress_completed as f32 / progress_total as f32
        };
        let progress_label = if is_sending {
            if infinite_progress {
                let round = t!(
                    "serialSend.shellProgressInfinite",
                    current = send.round.max(1)
                );
                let units = t!(
                    "serialSend.shellProgressUnits",
                    completed = progress_completed,
                    total = "∞"
                );
                format!("{round} · {units}")
            } else {
                let units = t!(
                    "serialSend.shellProgressUnits",
                    completed = progress_completed,
                    total = send.total
                );
                let round = t!(
                    "serialSend.shellProgressRound",
                    current = send.round.max(1),
                    total = send.rounds.max(1)
                );
                format!("{units} · {round}")
            }
        } else {
            String::new()
        };

        SendCommandBarViewState {
            send,
            palette,
            group_targets,
            target_count,
            is_broadcast,
            show_line_ending,
            validation_error,
            status,
            density,
            send_disabled,
            action_hint,
            input_hint,
            is_sending,
            progress_ratio,
            progress_label,
        }
    }
}

pub(super) fn send_shortcut() -> &'static str {
    if cfg!(target_os = "macos") {
        "Cmd+Enter"
    } else {
        "Ctrl+Enter"
    }
}

pub(super) fn localized_send_shortcut_hint() -> String {
    let modifier = if cfg!(target_os = "macos") {
        "Cmd+"
    } else {
        "Ctrl+"
    };
    t!("serialSend.sendShortcut").replace("Ctrl/Cmd + ", modifier)
}

pub(super) fn send_text_placeholder() -> String {
    format!(
        "{}  ·  {}",
        t!("serialSend.textPlaceholder"),
        localized_send_shortcut_hint()
    )
}

pub(super) fn send_hex_placeholder() -> String {
    format!(
        "{}  ·  {}",
        t!("serialSend.hexPlaceholder"),
        localized_send_shortcut_hint()
    )
}
