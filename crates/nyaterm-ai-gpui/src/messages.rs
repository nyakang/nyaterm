use crate::presentation::{AiAgentStepKind, AiAgentStepView};
use gpui::{
    AnyElement, App, ClickEvent, ClipboardItem, FontWeight, IntoElement, SharedString, Window, div,
    prelude::*, px, rgb,
};
use nyaterm_core::ai::{AiAgentStepStatus, AiCommandCard};
use nyaterm_ui::chat::{NyaDisclosure, NyaShimmerText};
use nyaterm_ui::notification::{NyaNotificationKind, NyaNotificationWindowExt as _};
use nyaterm_ui::{NyaButton, NyaButtonVariant, NyaIconButton, ThemePalette};
use rust_i18n::t;
pub fn disclosure(
    id: String,
    label: impl Into<SharedString>,
    open: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let selector = id.clone();
    let content_selector = format!("{id}-content");
    div()
        .debug_selector(move || selector.clone())
        .flex()
        .justify_start()
        .items_center()
        .child(
            div()
                .flex_none()
                .debug_selector(move || content_selector.clone())
                .child(NyaDisclosure::new(id, label, open).on_click(on_click)),
        )
        .into_any_element()
}

pub fn secondary(
    id: String,
    label: impl Into<SharedString>,
    variant: NyaButtonVariant,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let selector = id.clone();
    div()
        .debug_selector(move || selector.clone())
        .child(
            NyaButton::new(id, label)
                .small()
                .compact()
                .height(px(24.))
                .variant(variant)
                .on_click(on_click),
        )
        .into_any_element()
}

pub fn message_footer(
    id: &str,
    text: &str,
    timestamp: Option<String>,
    palette: ThemePalette,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_end()
        .gap_2()
        .when_some(timestamp, |row, timestamp| {
            row.child(
                div()
                    .relative()
                    // A glyph's optical center sits above its line-box center,
                    // so nudge it down to meet the icon's middle.
                    .top(px(2.))
                    .text_size(px(10.))
                    // Match the 20px copy button height so the glyphs share
                    // its vertical center instead of hanging near the top.
                    .line_height(px(20.))
                    // Main text at reduced alpha: a middle step between the
                    // theme's muted and dimmed tiers that follows the theme.
                    .text_color(rgb(palette.text).opacity(0.8))
                    .child(timestamp),
            )
        })
        .child(copy_message_button(id, text, palette))
        .into_any_element()
}

pub fn copy_message_button(id: &str, text: &str, palette: ThemePalette) -> AnyElement {
    let content = text.to_string();
    let selector = format!("ai-copy-{id}");
    div()
        .debug_selector(move || selector.clone())
        .child(
            NyaIconButton::new(format!("ai-copy-{id}"), "icons/copy.svg")
                .icon_size(px(12.))
                .size(px(20.))
                .color(rgb(palette.text).opacity(0.8))
                .tooltip(t!("ai.copy"))
                .on_click(move |_, window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(content.clone()));
                    window.notify_operation(
                        "copy-ai-message",
                        NyaNotificationKind::Success,
                        t!("common.copied").to_string(),
                        cx,
                    );
                }),
        )
        .into_any_element()
}

pub fn thinking_indicator(id: &str, label: impl Into<SharedString>) -> AnyElement {
    let selector = format!("ai-thinking-{id}");
    div()
        .debug_selector({
            let selector = selector.clone();
            move || selector.clone()
        })
        .child(
            NyaShimmerText::new(label)
                .text_size(px(11.))
                .line_height(px(16.))
                .id(selector)
                .duration(std::time::Duration::from_secs(2)),
        )
        .into_any_element()
}

/// Bounded, Unicode-safe text for previews; the original payload remains expandable.
pub fn text_preview(text: &str, lines: usize, characters: usize) -> String {
    let joined = text.lines().take(lines).collect::<Vec<_>>().join("\n");
    let preview: String = joined.chars().take(characters).collect();
    if preview.len() < text.trim_end().len() {
        format!("{preview}…")
    } else {
        preview
    }
}

pub fn command_heading(card: &AiCommandCard) -> String {
    if !matches!(card.title.trim(), "" | "Agent Command") {
        return card.title.clone();
    }
    let explanation = card.explanation.trim();
    if explanation.is_empty() {
        return t!("ai.terminalCommand").to_string();
    }
    let clause = explanation
        .split(['\n', '，', '。', ',', ';', '；'])
        .next()
        .unwrap_or(explanation);
    text_preview(clause, 1, 80)
}

pub fn step_heading(step: &AiAgentStepView) -> String {
    match step.kind {
        AiAgentStepKind::Planning => t!("ai.thinking").to_string(),
        AiAgentStepKind::ToolProgress => {
            let name = step
                .title
                .trim()
                .strip_prefix("Tool ")
                .unwrap_or(step.title.trim());
            let key = match name {
                "get_environment" => "ai.activityEnvironment",
                "session_get" => "ai.activitySession",
                "terminal_execute" | "execute_command" => "ai.terminalCommand",
                "terminal_recent_output" => "ai.activityTerminalOutput",
                "sftp_list" => "ai.activityListFiles",
                "sftp_stat" => "ai.activityFileInfo",
                "sftp_read_text" => "ai.activityReadFile",
                "tool_output_read" => "ai.activityToolOutput",
                "request_user_input" => "ai.harness.waitingForUser",
                "update_plan" => "ai.harness.plan",
                _ if step.status == AiAgentStepStatus::Tool => "ai.preparingNextCommand",
                _ => "ai.workingOnStep",
            };
            t!(key).to_string()
        }
        _ => step.title.clone(),
    }
}

pub fn agent_turn_header(
    palette: ThemePalette,
    agent_kind: &nyaterm_core::AiAgentKind,
    agent_mode: bool,
    live: bool,
) -> AnyElement {
    let name = crate::components::agent_kind_display_name(agent_kind);
    let title = if agent_mode {
        format!("{name} {}", t!("ai.modeAgent"))
    } else {
        name.to_string()
    };
    let status = if live {
        t!("ai.activity.running")
    } else {
        t!("ai.activity.completed")
    };
    div()
        .debug_selector(|| "ai-agent-turn".to_string())
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight(600.))
                .text_color(rgb(palette.text))
                .child(title),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(rgb(if live {
                    palette.link
                } else {
                    palette.text_muted
                }))
                .child(status),
        )
        .into_any_element()
}

/// Prepared content slots keep selectable-text entities and transcript caches with the caller.
pub struct MessageBubble {
    pub id: String,
    pub palette: ThemePalette,
    pub is_user: bool,
    pub activity: bool,
    pub show_thought: bool,
    pub thought_open: bool,
    pub thinking: Option<SharedString>,
    pub header: Option<AnyElement>,
    pub thought: Option<AnyElement>,
    pub answer: Option<AnyElement>,
    pub details: Vec<AnyElement>,
    pub copy_text: Option<String>,
    pub timestamp: Option<String>,
}

pub fn show_message_answer(
    message: &nyaterm_core::ai::AiMessage,
    display: &str,
    reasoning_only: bool,
) -> bool {
    // Suppress only exact native-call explanation duplicates; persisted text is untouched.
    let duplicate = message.role != nyaterm_core::ai::AiMessageRole::User
        && !message.command_cards.is_empty()
        && message.command_cards.iter().all(|card| {
            !card.explanation.trim().is_empty() && card.explanation.trim() == display.trim()
        });
    !reasoning_only && !display.is_empty() && !duplicate
}

pub fn message_bubble(
    view: MessageBubble,
    on_toggle_thought: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let palette = view.palette;
    let selector_id = view.id.clone();
    let mut body = div()
        .id(SharedString::from(format!("ai-msg-{}", view.id)))
        .debug_selector(move || format!("ai-message-{selector_id}"))
        .min_w_0()
        .when(!view.is_user, |body| body.w_full())
        .flex()
        .flex_col()
        .gap_2()
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(rgb(palette.text))
        .when(view.is_user, |body| {
            body.max_w(gpui::relative(0.85))
                .rounded_md()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(rgb(palette.hover))
                .p_2()
        });
    if let Some(header) = view.header {
        body = body.child(header);
    }
    if view.show_thought {
        body = body.child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .text_color(rgb(palette.text_muted))
                .child(disclosure(
                    format!("ai-thought-toggle-{}", view.id),
                    t!("ai.thoughtProcess"),
                    view.thought_open,
                    on_toggle_thought,
                ))
                .when_some(view.thinking.clone(), |header, label| {
                    header.child(thinking_indicator(&view.id, label))
                }),
        );
        if let Some(thought) = view.thought {
            body = body.child(
                div()
                    .min_w_0()
                    .w_full()
                    .text_color(rgb(palette.text_muted))
                    .child(thought),
            );
        }
    } else if !view.activity
        && let Some(label) = view.thinking
    {
        body = body.child(
            div()
                .text_color(rgb(palette.text_muted))
                .child(thinking_indicator(&view.id, label)),
        );
    }
    if let Some(answer) = view.answer {
        body = body.child(if view.is_user {
            answer
        } else {
            let id = view.id.clone();
            div()
                .debug_selector(move || format!("ai-answer-{id}"))
                .min_w_0()
                .w_full()
                .child(answer)
                .into_any_element()
        });
    }
    body = body.children(view.details);
    if view.is_user {
        // User bubbles add spacing beyond the transcript's ordinary row gap.
        let mut row = div()
            .w_full()
            .min_w_0()
            .pt(px(6.))
            .pb(px(6.))
            .flex()
            .flex_col()
            .items_end()
            .gap_1()
            .child(body);
        if let Some(text) = view.copy_text {
            row = row.child(message_footer(&view.id, &text, view.timestamp, palette));
        }
        return row.into_any_element();
    }
    if let Some(text) = view.copy_text {
        body = body.child(message_footer(&view.id, &text, view.timestamp, palette));
    }
    body.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::show_message_answer;
    use nyaterm_core::ai::{AiCommandCard, AiMessage, AiMessageRole};

    fn message() -> AiMessage {
        AiMessage {
            id: "message".into(),
            session_id: "session".into(),
            role: AiMessageRole::Assistant,
            content: "Explanation".into(),
            created_at: String::new(),
            reasoning_content: None,
            command_cards: vec![AiCommandCard {
                id: "card".into(),
                title: "List".into(),
                command: "ls".into(),
                explanation: "Explanation".into(),
                risk_level: None,
                risk_reason: None,
                expected_effect: String::new(),
                rollback: None,
                category: None,
                references: vec![],
                target_terminal_session_id: None,
                target: None,
            }],
        }
    }

    #[test]
    fn exact_explanation_duplicates_are_hidden_without_changing_saved_content() {
        let message = message();
        let before = message.clone();
        assert!(!show_message_answer(&message, " Explanation ", false));
        assert!(show_message_answer(
            &message,
            "Explanation with additional details",
            false
        ));
        assert_eq!(message, before);
    }

    #[test]
    fn user_and_cardless_answers_stay_visible_but_reasoning_rows_never_repeat_them() {
        let mut message = message();
        message.role = AiMessageRole::User;
        assert!(show_message_answer(&message, "Explanation", false));
        message.role = AiMessageRole::Assistant;
        message.command_cards.clear();
        assert!(show_message_answer(&message, "Explanation", false));
        assert!(!show_message_answer(&message, "Explanation", true));
        assert!(!show_message_answer(&message, "", false));
    }
}
