use nyaterm_ui::notification::{NyaNotificationKind, NyaNotificationWindowExt as _};
use std::collections::HashSet;

use gpui::{
    AnyElement, App, ClickEvent, ClipboardItem, Context, FontWeight, IntoElement, SharedString,
    Window, div, prelude::*, px, relative, rgb,
};
use nyaterm_core::ai::{AiAgentStepStatus, AiCommandCard, AiMessage, AiMessageRole, RiskLevel};
use nyaterm_ui::chat::{NyaDisclosure, NyaShimmerText, running_indicator};
use nyaterm_ui::{NyaButton, NyaButtonVariant};
use rust_i18n::t;

use crate::features::ai::is_agent_command_card;
use crate::features::ai::panel::components::{agent_kind_display_name, ai_command_target_label};
use crate::features::ai::panel::texts::TextOwner;
use crate::features::ai::panel::tool_call::{AiToolCallView, tool_call_title};
use crate::features::ai::panel::transcript::AiTranscriptRow;
use crate::features::ai::panel::{AiAgentStepPresentation, AiPanel, AiPanelSnapshot};
use crate::features::ai::presentation::{AiAgentStepKind, AiCommandPhase, AiResponsePhase};
use crate::features::runtime_jobs::AiAgentStepView;
use crate::features::shell::gpui_code_font_family;

pub(super) fn localized_risk(risk: Option<&RiskLevel>) -> String {
    t!(match risk {
        Some(RiskLevel::Low) => "ai.riskLow",
        Some(RiskLevel::Medium) => "ai.riskMedium",
        Some(RiskLevel::High) => "ai.riskHigh",
        Some(RiskLevel::Critical) => "ai.riskCritical",
        None => "ai.riskUnrated",
    })
    .to_string()
}

impl AiPanelSnapshot {
    pub(super) fn step_command_key(&self, step: &AiAgentStepView) -> String {
        format!(
            "step-command-{}-{}",
            step.source_message_id
                .as_deref()
                .unwrap_or(&self.current_ai_session_id),
            step.step_index,
        )
    }

    pub(super) fn step_is_active(&self, step_index: u16) -> bool {
        self.running
            && self.agent_steps.last().is_some_and(|last| {
                last.step.step_index == step_index
                    && matches!(
                        last.step.status,
                        AiAgentStepStatus::Planning
                            | AiAgentStepStatus::Tool
                            | AiAgentStepStatus::Running
                            | AiAgentStepStatus::NeedsApproval
                    )
            })
    }

    pub(super) fn command_step(&self, card_id: &str) -> Option<&AiAgentStepPresentation> {
        if self.index.matches(self) {
            return self
                .index
                .card_step(card_id)
                .map(|index| &self.agent_steps[index]);
        }
        self.agent_steps.iter().find(|presentation| {
            presentation.step.command_card_id.as_deref() == Some(card_id)
                && presentation
                    .step
                    .source_message_id
                    .as_ref()
                    .is_some_and(|id| {
                        self.messages.iter().any(|message| {
                            &message.id == id
                                && message.command_cards.iter().any(|card| card.id == card_id)
                        })
                    })
        })
    }

    pub(super) fn command_phase(&self, card: &AiCommandCard) -> AiCommandPhase {
        self.command_step(&card.id)
            .map(|presentation| AiCommandPhase::from_step(&presentation.step))
            .unwrap_or_else(|| {
                if is_agent_command_card(card) {
                    AiCommandPhase::HistoryUnknown
                } else {
                    AiCommandPhase::Suggested
                }
            })
    }

    pub(super) fn card_owner(&self, card_id: &str) -> Option<&str> {
        if self.index.matches(self) {
            return self
                .index
                .card_owner(card_id)
                .map(|index| self.messages[index].id.as_str());
        }
        self.messages
            .iter()
            .find(|message| message.command_cards.iter().any(|card| card.id == card_id))
            .map(|message| message.id.as_str())
    }

    pub(super) fn message_steps<'a>(
        &'a self,
        message_id: &'a str,
    ) -> impl Iterator<Item = &'a AiAgentStepPresentation> + 'a {
        let indices: std::borrow::Cow<'a, [usize]> = if self.index.matches(self) {
            std::borrow::Cow::Borrowed(self.index.steps(message_id))
        } else {
            std::borrow::Cow::Owned(
                self.agent_steps
                    .iter()
                    .enumerate()
                    .filter(|(_, presentation)| {
                        let step = &presentation.step;
                        if step.kind == AiAgentStepKind::FinalAnswer {
                            return false;
                        }
                        let owns_source = step.source_message_id.as_deref() == Some(message_id);
                        let fallback = self.step_is_active(step.step_index)
                            && self.streaming_assistant_id.as_deref() == Some(message_id)
                            && !step.source_message_id.as_deref().is_some_and(|id| {
                                self.messages.iter().any(|message| message.id == id)
                            });
                        if !owns_source && !fallback {
                            return false;
                        }
                        let has_card = step
                            .command_card_id
                            .as_deref()
                            .is_some_and(|id| self.command_step(id).is_some());
                        !has_card
                    })
                    .map(|(index, _)| index)
                    .collect(),
            )
        };
        (0..indices.len()).map(move |position| &self.agent_steps[indices[position]])
    }

    pub(super) fn step_in_message(&self, step_index: u16) -> bool {
        let step = if self.index.matches(self) {
            self.index
                .step_position(step_index)
                .map(|position| &self.agent_steps[position])
        } else {
            self.agent_steps
                .iter()
                .find(|step| step.step.step_index == step_index)
        };
        let Some(step) = step else {
            return false;
        };
        if step.step.kind == AiAgentStepKind::FinalAnswer {
            return false;
        }
        let source = step
            .step
            .source_message_id
            .as_deref()
            .filter(|id| self.index.message(self, id).is_some())
            .or_else(|| {
                if self.step_is_active(step_index) {
                    self.streaming_assistant_id.as_deref()
                } else {
                    None
                }
            });
        source.is_some_and(|id| self.index.message(self, id).is_some())
    }

    /// Reasoning inside Activity stays open while it streams, then folds unless
    /// the user opened it. Rendering and text order must agree on this.
    pub(super) fn activity_thought_open(&self, message_id: &str) -> bool {
        self.expanded_message_thoughts
            .iter()
            .any(|id| id == message_id)
            || (self.running
                && self.streaming_assistant_id.as_deref() == Some(message_id)
                && self.response_phase.shows_thinking())
    }
}

pub(super) fn disclosure(
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

fn secondary(
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

fn thinking_indicator(id: &str, label: impl Into<SharedString>) -> AnyElement {
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
pub(super) fn text_preview(text: &str, lines: usize, characters: usize) -> String {
    let joined = text.lines().take(lines).collect::<Vec<_>>().join("\n");
    let preview: String = joined.chars().take(characters).collect();
    if preview.len() < text.trim_end().len() {
        format!("{preview}…")
    } else {
        preview
    }
}

pub(super) fn command_heading(card: &AiCommandCard) -> String {
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

fn step_heading(step: &AiAgentStepView) -> String {
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

impl AiPanel {
    pub(super) fn ai_message_bubble(
        &self,
        snapshot: &AiPanelSnapshot,
        message: &AiMessage,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ai_message_bubble_inner(snapshot, message, false, false, false, cx)
    }

    pub(super) fn ai_activity_message(
        &self,
        snapshot: &AiPanelSnapshot,
        message: &AiMessage,
        reasoning_only: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .w_full()
            .min_w_0()
            .child(self.ai_message_bubble_inner(snapshot, message, true, reasoning_only, false, cx))
            .into_any_element()
    }

    pub(super) fn ai_agent_turn_header(
        &self,
        snapshot: &AiPanelSnapshot,
        live: bool,
    ) -> AnyElement {
        let palette = snapshot.chrome.palette;
        let name = agent_kind_display_name(&snapshot.agent_kind);
        let title = if snapshot.agent_mode {
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

    pub(super) fn ai_final_message(
        &self,
        snapshot: &AiPanelSnapshot,
        message: &AiMessage,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ai_message_bubble_inner(snapshot, message, false, false, true, cx)
    }

    fn ai_message_bubble_inner(
        &self,
        snapshot: &AiPanelSnapshot,
        message: &AiMessage,
        activity: bool,
        reasoning_only: bool,
        final_answer: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = snapshot.chrome.palette;
        let is_user = message.role == AiMessageRole::User;
        let streaming = snapshot.streaming_assistant_id.as_deref() == Some(message.id.as_str());
        let thinking = streaming
            && snapshot.message_steps(&message.id).next().is_none()
            && (snapshot.response_phase.shows_thinking()
                || snapshot.response_phase == AiResponsePhase::ToolArguments);
        let activity_label = if snapshot.response_phase == AiResponsePhase::ToolArguments {
            t!("ai.commandPreparing")
        } else {
            t!("ai.thinking")
        };
        let prepared = self.prepared_message(&message.id);
        let display = prepared
            .map(|content| content.display.as_str())
            .unwrap_or_default();
        let reasoning = prepared.and_then(|content| content.reasoning.as_deref());
        let mut body = div()
            .id(SharedString::from(format!("ai-msg-{}", message.id)))
            .debug_selector({
                let id = message.id.clone();
                move || format!("ai-message-{id}")
            })
            .min_w_0()
            .when(!is_user, |body| body.w_full())
            .flex()
            .flex_col()
            .gap_2()
            .text_size(px(12.))
            .line_height(px(18.))
            .text_color(rgb(palette.text))
            .when(is_user, |body| {
                body.max_w(relative(0.85))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.hover))
                    .p_2()
            });
        if !is_user && !activity && !final_answer {
            body = body.child(self.ai_agent_turn_header(snapshot, streaming && snapshot.running));
        }
        if activity {
            if let Some(_reasoning) = reasoning.filter(|text| {
                text.trim() != display.trim()
                    && !message
                        .command_cards
                        .iter()
                        .any(|card| card.explanation.trim() == text.trim())
            }) {
                let open = snapshot.activity_thought_open(&message.id);
                let id = message.id.clone();
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_color(rgb(palette.text_muted))
                        .child(disclosure(
                            format!("ai-thought-toggle-{id}"),
                            t!("ai.thoughtProcess"),
                            open,
                            cx.listener(move |panel, _, _, cx| {
                                let id = id.clone();
                                panel.with_app(cx, move |app, cx| {
                                    app.ai.toggle_message_thought(id);
                                    app.defer_ai_panel_snapshot_flush(cx);
                                });
                            }),
                        ))
                        .when(thinking, |header| {
                            header.child(thinking_indicator(&message.id, activity_label.clone()))
                        }),
                );
                if open {
                    body = body.child(
                        div()
                            .min_w_0()
                            .w_full()
                            .text_color(rgb(palette.text_muted))
                            .child(self.selectable_text(
                                TextOwner::Message(message.id.clone()),
                                "thought",
                                false,
                                cx,
                            )),
                    );
                }
            }
        } else if let Some(_reasoning) = reasoning.filter(|_| !final_answer) {
            let open = snapshot.expanded_message_thoughts.contains(&message.id);
            let id = message.id.clone();
            let header = disclosure(
                format!("ai-thought-toggle-{id}"),
                t!("ai.thoughtProcess"),
                open,
                cx.listener(move |panel, _, _, cx| {
                    let id = id.clone();
                    panel.with_app(cx, move |app, cx| {
                        app.ai.toggle_message_thought(id);
                        app.defer_ai_panel_snapshot_flush(cx);
                    });
                }),
            );
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_color(rgb(palette.text_muted))
                    .child(header)
                    .when(thinking, |header| {
                        header.child(thinking_indicator(&message.id, activity_label.clone()))
                    }),
            );
            if open {
                body = body.child(
                    div()
                        .min_w_0()
                        .w_full()
                        .text_color(rgb(palette.text_muted))
                        .child(self.selectable_text(
                            TextOwner::Message(message.id.clone()),
                            "thought",
                            false,
                            cx,
                        )),
                );
            }
        } else if thinking {
            body = body.child(
                div()
                    .text_color(rgb(palette.text_muted))
                    .child(thinking_indicator(&message.id, activity_label.clone())),
            );
        }
        // Native calls project the same explanation into both payloads. Suppress only
        // an exact duplicate in presentation, retaining saved text and legacy notes.
        let duplicate_explanation = !is_user
            && !message.command_cards.is_empty()
            && message.command_cards.iter().all(|card| {
                !card.explanation.trim().is_empty() && card.explanation.trim() == display.trim()
            });
        if !reasoning_only && !display.is_empty() && !duplicate_explanation {
            body = body.child(if is_user {
                self.selectable_text(TextOwner::Message(message.id.clone()), "answer", false, cx)
            } else {
                div()
                    .debug_selector({
                        let id = message.id.clone();
                        move || format!("ai-answer-{id}")
                    })
                    .min_w_0()
                    .w_full()
                    .child(self.selectable_text(
                        TextOwner::Message(message.id.clone()),
                        "answer",
                        true,
                        cx,
                    ))
                    .into_any_element()
            });
        }
        let mut shown = HashSet::new();
        for card in &message.command_cards {
            if shown.insert(&card.id) && snapshot.card_owner(&card.id) == Some(message.id.as_str())
            {
                body = body.child(if activity {
                    self.ai_execution_command_view(snapshot, card.clone(), cx)
                } else {
                    self.ai_command_card_view(snapshot, card.clone(), cx)
                });
            }
        }
        for step in snapshot.message_steps(&message.id) {
            body = body.child(self.ai_agent_step_card_inner(snapshot, step.clone(), activity, cx));
        }
        if is_user {
            // The transcript already leaves 8px under every row. A user bubble
            // needs a little more air so it separates from the turns around it.
            return div()
                .w_full()
                .min_w_0()
                .pt(px(6.))
                .pb(px(6.))
                .flex()
                .justify_end()
                .child(body)
                .into_any_element();
        }
        body.into_any_element()
    }

    pub(super) fn ai_agent_step_card(
        &self,
        snapshot: &AiPanelSnapshot,
        presentation: AiAgentStepPresentation,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ai_agent_step_card_inner(snapshot, presentation, false, cx)
    }

    fn ai_agent_step_card_inner(
        &self,
        snapshot: &AiPanelSnapshot,
        presentation: AiAgentStepPresentation,
        activity: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = snapshot.chrome.palette;
        let step = presentation.step;
        let index = step.step_index;
        let heading = step_heading(&step);
        // Only the current live step animates; historical progress remains still.
        let active = snapshot.step_is_active(index);
        let mut row = div()
            .id(format!("ai-agent-step-{index}"))
            .debug_selector(move || format!("ai-agent-step-{index}"))
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .text_size(px(12.))
            .line_height(px(18.));
        if step.command.is_some() || step.observation.is_some() {
            row = AiToolCallView::from_step(&step).apply_frame(row, palette);
        }
        // Unlinked progress stays compact; commands and answers are explicit payload kinds.
        let mut title = div()
            .flex()
            .items_center()
            .gap_1()
            .text_size(px(11.))
            .text_color(rgb(palette.text_muted));
        if active && step.status == AiAgentStepStatus::Planning {
            title = title
                .child(NyaShimmerText::new(heading.clone()).id(format!("ai-planning-{index}")));
        } else {
            if active
                && matches!(
                    step.status,
                    AiAgentStepStatus::Tool | AiAgentStepStatus::Running
                )
            {
                title = title.child(
                    div()
                        .debug_selector(move || format!("ai-step-running-{index}"))
                        .child(running_indicator(
                            format!("ai-step-running-{index}"),
                            rgb(palette.link),
                        )),
                );
            }
            title = title.child(heading);
        }
        row = row.child(title);
        if step.kind == AiAgentStepKind::ToolProgress && !active {
            let key = match step.status {
                AiAgentStepStatus::Completed => "ai.commandCompleted",
                AiAgentStepStatus::Failed => "ai.commandFailed",
                AiAgentStepStatus::Rejected => "ai.commandRejected",
                AiAgentStepStatus::Cancelled => "ai.commandCancelled",
                _ => "ai.commandHistoryUnknown",
            };
            row = row.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!(key)),
            );
        }
        if step.kind == AiAgentStepKind::FinalAnswer {
            return row
                .child(self.selectable_text(TextOwner::Step(index), "detail", true, cx))
                .into_any_element();
        }
        if let Some(thought) = step.thought.as_ref().filter(|text| !text.trim().is_empty()) {
            let duplicate = step.source_message_id.as_ref().is_some_and(|id| {
                snapshot.index.message(snapshot, id).is_some_and(|message| {
                    &message.id == id
                        && (self
                            .prepared_message(&message.id)
                            .is_some_and(|prepared| prepared.display.trim() == thought.trim())
                            || message
                                .reasoning_content
                                .as_deref()
                                .is_some_and(|text| text.trim() == thought.trim()))
                })
            });
            if activity {
                // Planning metadata is an internal progress hint. Provider reasoning
                // is already displayed as prose in the owning activity message.
                if step.kind != AiAgentStepKind::Planning && !duplicate {
                    row = row.child(self.selectable_text(
                        TextOwner::Step(index),
                        "thought",
                        false,
                        cx,
                    ));
                }
            } else {
                row = row.child(disclosure(
                    format!("ai-agent-thought-{index}"),
                    t!("ai.thoughtProcess"),
                    presentation.thought_open,
                    cx.listener(move |panel, _, _, cx| {
                        panel.with_app(cx, move |app, cx| {
                            app.toggle_ai_agent_thought_expanded(index, cx)
                        })
                    }),
                ));
                if presentation.thought_open {
                    row = row.child(self.selectable_text(
                        TextOwner::Step(index),
                        "thought",
                        false,
                        cx,
                    ));
                }
            }
        }
        if let Some(_command) = &step.command {
            let source = step.source_message_id.clone();
            let key = snapshot.step_command_key(&step);
            let open = step.status == AiAgentStepStatus::NeedsApproval
                || snapshot.expanded_command_scripts.contains(&key);
            let id = key.clone();
            row = row.child(disclosure(
                key,
                t!("ai.terminalCommand"),
                open,
                cx.listener(move |panel, _, _, cx| {
                    let anchor = panel.transcript_rows.iter().position(|row| match row {
                        AiTranscriptRow::AgentStep { step_index, .. } => *step_index == index,
                        AiTranscriptRow::ActivityMessage { id, .. } => source.as_ref() == Some(id),
                        _ => false,
                    });
                    if !open && let Some(index) = anchor {
                        panel.transcript_scroll.update(cx, |scroll, cx| {
                            scroll.scroll_to_item(index, cx);
                        });
                    }
                    let id = id.clone();
                    panel.with_app(cx, move |app, cx| {
                        app.ai.toggle_command_script(id);
                        app.defer_ai_panel_snapshot_flush(cx);
                    });
                }),
            ));
            if open {
                row = row.child(
                    div()
                        .debug_selector(move || format!("ai-step-command-body-{index}"))
                        .font_family(gpui_code_font_family())
                        .text_color(rgb(palette.text))
                        .child(self.selectable_text(TextOwner::Step(index), "script", false, cx)),
                );
            }
        }
        if let Some(_output) = step.observation {
            row = row.child(disclosure(
                format!("ai-agent-output-{index}"),
                t!("ai.executionOutput"),
                presentation.output_open,
                cx.listener(move |panel, _, _, cx| {
                    panel.with_app(cx, move |app, cx| {
                        app.toggle_ai_agent_output_expanded(index, cx)
                    })
                }),
            ));
            if presentation.output_open {
                row = row.child(
                    div()
                        .font_family(gpui_code_font_family())
                        .text_color(rgb(palette.text_muted))
                        .child(self.selectable_text(TextOwner::Step(index), "output", false, cx)),
                );
            }
        } else if step.kind == AiAgentStepKind::Diagnostic && !step.detail.is_empty() {
            row = row.child(
                div()
                    .text_color(rgb(palette.text_muted))
                    .child(self.selectable_text(TextOwner::Step(index), "detail", false, cx)),
            );
        }
        row.into_any_element()
    }

    pub(super) fn ai_command_card_view(
        &self,
        snapshot: &AiPanelSnapshot,
        card: AiCommandCard,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_command_card(snapshot, card, false, cx)
    }

    fn ai_execution_command_view(
        &self,
        snapshot: &AiPanelSnapshot,
        card: AiCommandCard,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = card.id.clone();
        let phase = snapshot.command_phase(&card);
        let open = phase.offers_approval() || snapshot.expanded_command_scripts.contains(&key);
        let mut row = AiToolCallView::from_phase(phase)
            .frame(snapshot.chrome.palette)
            .debug_selector({
                let key = key.clone();
                move || format!("ai-tool-call-{key}")
            })
            .flex()
            .flex_col()
            .gap_2()
            .child(self.selectable_text(TextOwner::Command(key.clone()), "explanation", false, cx));
        if phase.offers_approval() {
            return row
                .child(self.render_command_card(snapshot, card, true, cx))
                .into_any_element();
        }
        let id = key.clone();
        let mut command_header = div().flex().items_center().gap_1();
        if phase == AiCommandPhase::Running {
            command_header = command_header.child(running_indicator(
                format!("ai-command-live-{key}"),
                rgb(snapshot.chrome.palette.link),
            ));
        }
        row = row.child(command_header.child(disclosure(
            format!("ai-command-toggle-{key}"),
            format!(
                "{} · {}",
                tool_call_title(&ai_command_target_label(&card)),
                t!(phase.label_key())
            ),
            open,
            cx.listener(move |panel, _, _, cx| {
                let anchor = panel.transcript_rows.iter().position(|row| {
                    matches!(row, AiTranscriptRow::ActivityMessage { index, .. }
                        if panel.panel().is_some_and(|snapshot|
                            snapshot.messages[*index].command_cards.iter().any(|card| card.id == id)))
                });
                if !open && let Some(index) = anchor {
                    panel.transcript_scroll.update(cx, |scroll, cx| {
                        scroll.scroll_to_item(index, cx);
                    });
                }
                let id = id.clone();
                panel.with_app(cx, move |app, cx| {
                    app.ai.toggle_command_script(id);
                    app.defer_ai_panel_snapshot_flush(cx);
                });
            }),
        )));
        if open {
            row = row.child(self.render_command_card(snapshot, card, true, cx));
        }
        row.into_any_element()
    }

    fn render_command_card(
        &self,
        snapshot: &AiPanelSnapshot,
        card: AiCommandCard,
        in_timeline: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = snapshot.chrome.palette;
        let phase = snapshot.command_phase(&card);
        let linked = snapshot.command_step(&card.id);
        let key = card.id.clone();
        let tool = AiToolCallView::from_phase(phase);
        let color = tool.status_color(palette);
        let target = ai_command_target_label(&card);
        let heading = command_heading(&card);
        let script_open = in_timeline
            || phase.offers_approval()
            || snapshot.expanded_command_scripts.contains(&key);
        let long_command = card.command.lines().count() > 3 || card.command.chars().count() > 160;
        let mut block = div()
            .id(format!("ai-command-card-{key}"))
            .debug_selector({
                let key = key.clone();
                move || format!("ai-command-{key}")
            })
            .min_w_0()
            .w_full()
            .when(!in_timeline, |block| tool.apply_frame(block, palette))
            .flex()
            .flex_col()
            .gap(px(6.))
            .text_size(px(12.))
            .line_height(px(18.))
            .text_color(rgb(palette.text))
            .when(!in_timeline, |block| {
                block.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .text_size(px(13.))
                                .text_color(rgb(palette.text))
                                .font_weight(FontWeight(600.))
                                .child(self.selectable_text(
                                    TextOwner::Command(key.clone()),
                                    "heading",
                                    false,
                                    cx,
                                )),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .text_size(px(10.))
                                .line_height(px(16.))
                                .text_color(rgb(color))
                                .flex()
                                .items_center()
                                .gap_1()
                                .when(phase == AiCommandPhase::Running, |status| {
                                    status.child(
                                        div()
                                            .debug_selector({
                                                let key = key.clone();
                                                move || format!("ai-running-{key}")
                                            })
                                            .child(running_indicator(
                                                format!("ai-running-{key}"),
                                                rgb(color),
                                            )),
                                    )
                                })
                                .child(if phase == AiCommandPhase::Preparing && snapshot.running {
                                    NyaShimmerText::new(t!(phase.label_key()))
                                        .id(format!("ai-preparing-{key}"))
                                        .into_any_element()
                                } else {
                                    div().child(t!(phase.label_key())).into_any_element()
                                }),
                        ),
                )
            })
            .when(!target.is_empty(), |block| {
                block.child(
                    div()
                        .min_w_0()
                        .text_size(px(10.))
                        .line_height(px(14.))
                        .text_color(rgb(palette.text_muted))
                        .text_ellipsis()
                        .child(self.selectable_text(
                            TextOwner::Command(key.clone()),
                            "target-label",
                            false,
                            cx,
                        )),
                )
            });
        if !in_timeline && !card.explanation.trim().is_empty() && heading != card.explanation.trim()
        {
            let explanation = card.explanation.trim();
            let remainder = explanation
                .strip_prefix(&heading)
                .map(|rest| rest.trim_start_matches(['，', '。', ',', ';', '；', ' ']))
                .unwrap_or(explanation);
            if !remainder.is_empty() {
                block = block.child(div().text_color(rgb(palette.text_muted)).child(
                    self.selectable_text(TextOwner::Command(key.clone()), "remainder", false, cx),
                ));
            }
        }
        if phase == AiCommandPhase::NeedsApproval {
            block = block.child(
                div()
                    .text_color(rgb(palette.warning))
                    .child(self.selectable_text(
                        TextOwner::Command(key.clone()),
                        "risk",
                        false,
                        cx,
                    )),
            );
            if let Some(_reason) = &card.risk_reason {
                block = block.child(div().text_color(rgb(palette.warning)).child(
                    self.selectable_text(TextOwner::Command(key.clone()), "risk-reason", false, cx),
                ));
            }
        }
        if let Some(presentation) = linked {
            let step = &presentation.step;
            if matches!(
                phase,
                AiCommandPhase::Failed | AiCommandPhase::Rejected | AiCommandPhase::Cancelled
            ) && step.observation.is_none()
                && !step.detail.trim().is_empty()
            {
                block = block.child(div().text_color(rgb(color)).child(self.selectable_text(
                    TextOwner::Command(key.clone()),
                    "detail",
                    false,
                    cx,
                )));
            }
            let thought_in_message = step.source_message_id.as_ref().is_some_and(|id| {
                snapshot.index.message(snapshot, id).is_some_and(|message| {
                    &message.id == id
                        && (message.reasoning_content.is_some()
                            || self
                                .prepared_message(&message.id)
                                .is_some_and(|prepared| prepared.reasoning.is_some()))
                })
            });
            if let Some(_thought) = step.thought.as_ref().filter(|thought| {
                !in_timeline
                    && !thought_in_message
                    && !thought.trim().is_empty()
                    && thought.trim() != card.explanation.trim()
            }) {
                let index = step.step_index;
                block = block.child(disclosure(
                    format!("ai-command-thought-{key}"),
                    t!("ai.thoughtProcess"),
                    presentation.thought_open,
                    cx.listener(move |panel, _, _, cx| {
                        panel.with_app(cx, move |app, cx| {
                            app.toggle_ai_agent_thought_expanded(index, cx)
                        })
                    }),
                ));
                if presentation.thought_open {
                    block = block.child(self.selectable_text(
                        TextOwner::Command(key.clone()),
                        "thought",
                        false,
                        cx,
                    ));
                }
            }
            if let Some(_output) = &step.observation {
                block = block.child(
                    div()
                        .debug_selector({
                            let key = key.clone();
                            move || format!("ai-command-result-{key}")
                        })
                        .min_w_0()
                        .w_full()
                        .text_color(rgb(palette.text_muted))
                        .child(self.selectable_text(
                            TextOwner::Command(key.clone()),
                            "output-preview",
                            false,
                            cx,
                        )),
                );
                if step.exit_code.is_some() {
                    block = block.child(div().text_size(px(11.)).text_color(rgb(color)).child(
                        self.selectable_text(
                            TextOwner::Command(key.clone()),
                            "exit-code",
                            false,
                            cx,
                        ),
                    ));
                }
                let index = step.step_index;
                block = block.child(disclosure(
                    format!("ai-command-output-{key}"),
                    t!("ai.executionOutput"),
                    presentation.output_open,
                    cx.listener(move |panel, _, _, cx| {
                        panel.with_app(cx, move |app, cx| {
                            app.toggle_ai_agent_output_expanded(index, cx)
                        })
                    }),
                ));
                if presentation.output_open {
                    block = block.child(
                        div()
                            .min_w_0()
                            .w_full()
                            .text_color(rgb(palette.text_muted))
                            .font_family(gpui_code_font_family())
                            .child(self.selectable_text(
                                TextOwner::Command(key.clone()),
                                "output",
                                false,
                                cx,
                            )),
                    );
                }
            }
        }
        block = block.child(
            div()
                .debug_selector({
                    let key = key.clone();
                    move || format!("ai-command-body-{key}")
                })
                .min_w_0()
                .w_full()
                .py_1()
                .font_family(gpui_code_font_family())
                .when(long_command && !script_open, |body| {
                    body.max_h(px(62.)).overflow_hidden()
                })
                .child(self.selectable_text(TextOwner::Command(key.clone()), "script", false, cx)),
        );
        if long_command && !phase.offers_approval() && !in_timeline {
            let id = key.clone();
            block = block.child(disclosure(
                format!("ai-command-script-{key}"),
                if script_open {
                    t!("ai.collapseCommand")
                } else {
                    t!("ai.expandCommand")
                },
                script_open,
                cx.listener(move |panel, _, _, cx| {
                    let id = id.clone();
                    panel.with_app(cx, move |app, cx| {
                        app.ai.toggle_command_script(id);
                        app.defer_ai_panel_snapshot_flush(cx);
                    });
                }),
            ));
        }
        let details_open = snapshot.expanded_command_details.contains(&key);
        if card.target_terminal_session_id.is_some()
            || !card.expected_effect.is_empty()
            || card.rollback.is_some()
            || (phase != AiCommandPhase::NeedsApproval && card.risk_reason.is_some())
        {
            let id = key.clone();
            block = block.child(disclosure(
                format!("ai-command-details-{key}"),
                t!("ai.commandDetails"),
                details_open,
                cx.listener(move |panel, _, _, cx| {
                    let id = id.clone();
                    panel.with_app(cx, move |app, cx| {
                        app.ai.toggle_command_details(id);
                        app.defer_ai_panel_snapshot_flush(cx);
                    });
                }),
            ));
            if details_open {
                let mut details = div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .gap_1()
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(rgb(palette.text_muted));
                if let Some(_session_id) = &card.target_terminal_session_id {
                    details = details.child(div().min_w_0().child(self.selectable_text(
                        TextOwner::Command(key.clone()),
                        "target-session",
                        false,
                        cx,
                    )));
                }
                if let Some(host) = card.target.as_ref().and_then(|target| target.host.as_ref())
                    && !host.is_empty()
                {
                    details = details.child(self.selectable_text(
                        TextOwner::Command(key.clone()),
                        "host",
                        false,
                        cx,
                    ));
                }
                if phase != AiCommandPhase::NeedsApproval {
                    details = details.child(self.selectable_text(
                        TextOwner::Command(key.clone()),
                        "risk",
                        false,
                        cx,
                    ));
                    if let Some(_reason) = &card.risk_reason {
                        details = details.child(self.selectable_text(
                            TextOwner::Command(key.clone()),
                            "risk-reason",
                            false,
                            cx,
                        ));
                    }
                }
                if !card.expected_effect.is_empty() {
                    details = details.child(self.selectable_text(
                        TextOwner::Command(key.clone()),
                        "expected-effect",
                        false,
                        cx,
                    ));
                }
                if let Some(_rollback) = &card.rollback {
                    details = details.child(self.selectable_text(
                        TextOwner::Command(key.clone()),
                        "rollback",
                        false,
                        cx,
                    ));
                }
                block = block.child(details);
            }
        }
        let mut actions = div().flex().flex_wrap().items_center().gap_1();
        if phase.offers_approval() || phase.offers_run() {
            let id = key.clone();
            actions = actions.child(
                div()
                    .debug_selector({
                        let key = key.clone();
                        move || format!("ai-command-run-{key}")
                    })
                    .child(
                        NyaButton::new(
                            format!("ai-command-run-{key}"),
                            if phase.offers_approval() {
                                t!("ai.approveCommand")
                            } else {
                                t!("ai.runCommand")
                            },
                        )
                        .small()
                        .variant(NyaButtonVariant::Primary)
                        .on_click(cx.listener(move |panel, _, _, cx| {
                            let id = id.clone();
                            panel
                                .with_app(cx, move |app, cx| app.run_ai_command_card_by_id(id, cx));
                        })),
                    ),
            );
        }
        if phase.offers_approval() {
            let id = key.clone();
            actions = actions.child(secondary(
                format!("ai-command-reject-{key}"),
                t!("ai.rejectCommand"),
                NyaButtonVariant::Secondary,
                cx.listener(move |panel, _, _, cx| {
                    let id = id.clone();
                    panel.with_app(cx, move |app, cx| {
                        app.reject_ai_agent_command_card_by_id(id, cx)
                    });
                }),
            ));
        }
        if phase.offers_reuse() {
            let id = key.clone();
            actions = actions.child(secondary(
                format!("ai-command-insert-{key}"),
                t!("ai.insertCommand"),
                NyaButtonVariant::Ghost,
                cx.listener(move |panel, _, _, cx| {
                    let id = id.clone();
                    panel.with_app(cx, move |app, cx| app.insert_ai_command_card_by_id(id, cx));
                }),
            ));
        }
        let command = card.command.clone();
        actions = actions.child(secondary(
            format!("ai-command-copy-{key}"),
            t!("ai.copyCommand"),
            NyaButtonVariant::Ghost,
            cx.listener(move |_, _, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(command.clone()));
                window.notify_operation(
                    "copy-ai-command",
                    NyaNotificationKind::Success,
                    t!("common.copied").to_string(),
                    cx,
                );
            }),
        ));
        if phase.offers_reuse() {
            let id = key.clone();
            actions = actions.child(secondary(
                format!("ai-command-save-{key}"),
                t!("ai.saveCommand"),
                NyaButtonVariant::Ghost,
                cx.listener(move |panel, _, _, cx| {
                    let id = id.clone();
                    panel.with_app(cx, move |app, cx| app.save_ai_command_card_by_id(id, cx));
                }),
            ));
        }
        block.child(actions).into_any_element()
    }
}
