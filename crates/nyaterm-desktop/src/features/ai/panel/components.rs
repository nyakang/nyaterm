use rust_i18n::t;

use gpui::{Context, FontWeight, IntoElement, SharedString, div, prelude::*, px, rgb};
use nyaterm_core::ai::AiCommandCard;
use nyaterm_core::{AgentCommandExecutionMode, AiAgentKind};
use nyaterm_ui::{NyaButton, NyaButtonVariant};

use crate::features::formatting::compact_id;
use crate::theme::ThemePalette;

use super::AiPanel;

/// Product names of the agent runtimes; they are not translated.
pub(super) fn agent_kind_display_name(kind: &AiAgentKind) -> &'static str {
    match kind {
        AiAgentKind::Nyaterm => "NyaTerm",
        AiAgentKind::Codex => "Codex",
        AiAgentKind::ClaudeCode => "Claude Code",
    }
}

pub(super) fn execution_mode_label(mode: &AgentCommandExecutionMode) -> String {
    t!(match mode {
        AgentCommandExecutionMode::ConfirmEach => "ai.executionModeConfirmEach",
        AgentCommandExecutionMode::Smart => "ai.executionModeSmart",
        AgentCommandExecutionMode::Auto => "ai.executionModeAuto",
    })
    .to_string()
}

pub(super) fn ai_command_target_label(card: &AiCommandCard) -> String {
    card.target
        .as_ref()
        .map(|target| target.label.trim())
        .filter(|label| !label.is_empty())
        .map(str::to_string)
        .or_else(|| card.target_terminal_session_id.as_deref().map(compact_id))
        .unwrap_or_default()
}

pub(super) fn ai_send_button(
    _palette: ThemePalette,
    running: bool,
    disabled: bool,
    cx: &mut Context<AiPanel>,
) -> impl IntoElement {
    div()
        .debug_selector(|| "ai-send-control".to_string())
        .size(px(28.))
        .flex_none()
        .child(
            NyaButton::new("ai-ask-run", "")
                .small()
                .full_width()
                .height(px(28.))
                .icon(if running {
                    "icons/ai/stop.svg"
                } else {
                    "icons/ai/send.svg"
                })
                .variant(if running {
                    NyaButtonVariant::Secondary
                } else {
                    NyaButtonVariant::Primary
                })
                .tooltip(if running {
                    t!("ai.cancelTask")
                } else {
                    t!("ai.sendMessage")
                })
                .disabled(disabled)
                .on_click(cx.listener(move |panel, _, _, cx| {
                    panel.with_app(cx, move |app, cx| {
                        if app.ai.chat_or_agent_is_running() {
                            app.cancel_ai_chat(cx);
                        } else if !disabled {
                            app.start_ai_ask(cx);
                        }
                    });
                })),
        )
}

pub(super) fn ai_setup_step(
    palette: ThemePalette,
    index: &'static str,
    label: impl Into<SharedString>,
) -> impl IntoElement {
    let label: SharedString = label.into();
    div()
        .w_full()
        .max_w(px(280.))
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.bg))
        .px_3()
        .py_2()
        .flex()
        .items_start()
        .gap_2()
        .child(
            div()
                .size(px(18.))
                .rounded_full()
                .bg(rgb(palette.hover))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(10.))
                .font_weight(FontWeight(800.))
                .text_color(rgb(palette.link))
                .child(index),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(rgb(palette.text))
                .child(label),
        )
}

#[cfg(test)]
mod tests {
    use nyaterm_core::ai::{AiCommandCard, AiTerminalTarget};

    use super::ai_command_target_label;

    #[test]
    fn legacy_command_target_is_compact_and_keeps_its_execution_id() {
        let mut card: AiCommandCard = serde_json::from_value(serde_json::json!({
            "id": "legacy-command",
            "title": "Inspect resources",
            "command": "pwd",
            "explanation": "",
            "expectedEffect": "",
            "targetTerminalSessionId": "af0280b6-e62b-4d17-8f3c-000000000001"
        }))
        .unwrap();
        assert_eq!(ai_command_target_label(&card), "af0280b6..0001");
        assert_eq!(
            card.target_terminal_session_id.as_deref(),
            Some("af0280b6-e62b-4d17-8f3c-000000000001")
        );

        card.target = Some(AiTerminalTarget {
            terminal_session_id: card.target_terminal_session_id.clone().unwrap(),
            connection_id: None,
            label: "Production SSH".into(),
            host: Some("example.test".into()),
            username: None,
            session_type: "SSH".into(),
        });
        assert_eq!(ai_command_target_label(&card), "Production SSH");
        card.target.as_mut().unwrap().label = " ".into();
        assert_eq!(ai_command_target_label(&card), "af0280b6..0001");
    }
}
