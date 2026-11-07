use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{AnyElement, Context, IntoElement, Subscription, div, prelude::*, px};
use nyaterm_core::AiMessageRole;
use nyaterm_ui::text::{
    NyaSelectableCode, NyaTextFormat, NyaTextScope, NyaTextState, NyaTextStyle,
};
use nyaterm_ui::{NyaContextMenu, NyaMarkdown, NyaMenuItem, NyaSelectableText};
use rust_i18n::t;

use super::components::ai_command_target_label;
use super::messages::{command_heading, localized_risk, text_preview};
use super::tool_call::tool_call_title;

mod order;
use super::transcript::AiTranscriptRow;
use super::{AiPanel, AiPanelSnapshot};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum TextOwner {
    Message(String),
    Step(u16),
    Command(String),
    Approval(String),
    Run(String),
}

type TextKey = (TextOwner, String);

pub(super) struct TextEntry {
    source: String,
    pub(super) state: NyaTextState,
    format: NyaTextFormat,
    visible: bool,
    _layout_subscription: Subscription,
}

#[derive(Default)]
pub(super) struct TranscriptTextCache {
    context: Option<(String, Option<String>, Option<String>)>,
    pub scope: NyaTextScope,
    pub(super) entries: HashMap<TextKey, TextEntry>,
    pub reset_selection: bool,
    rows: HashMap<TextOwner, Vec<usize>>,
}

impl AiPanel {
    pub(super) fn refresh_texts(&mut self, snapshot: &AiPanelSnapshot, cx: &mut Context<Self>) {
        let context = (
            snapshot.current_ai_session_id.clone(),
            snapshot.owner_terminal_id.clone(),
            snapshot.owner_connection_id.clone(),
        );
        if self.texts.context.as_ref() != Some(&context) {
            for entry in self.texts.entries.values() {
                entry.state.clear_selection(cx);
            }
            self.texts.entries.clear();
            self.texts.scope = NyaTextScope::default();
            self.texts.context = Some(context);
            self.texts.reset_selection = true;
        }
        let mut sources = HashMap::new();
        let mut put = |owner: TextOwner, purpose: &str, format: NyaTextFormat, text: &str| {
            if !text.is_empty() {
                sources.insert((owner, purpose.to_string()), (format, text.to_string()));
            }
        };
        for message in snapshot.messages.iter() {
            let owner = TextOwner::Message(message.id.clone());
            if let Some(prepared) = self.prepared_message(&message.id) {
                put(
                    owner.clone(),
                    "answer",
                    if message.role == AiMessageRole::User {
                        NyaTextFormat::Plain
                    } else {
                        NyaTextFormat::Markdown
                    },
                    &prepared.display,
                );
                if let Some(thought) = &prepared.reasoning {
                    put(owner, "thought", NyaTextFormat::Markdown, thought);
                }
            }
        }
        for presentation in snapshot.agent_steps.iter() {
            let step = &presentation.step;
            let owner = TextOwner::Step(step.step_index);
            put(
                owner.clone(),
                "detail",
                NyaTextFormat::Markdown,
                &step.detail,
            );
            if let Some(thought) = &step.thought {
                put(owner.clone(), "thought", NyaTextFormat::Markdown, thought);
            }
            if let Some(command) = &step.command {
                put(owner.clone(), "script", NyaTextFormat::Code, command);
            }
            if let Some(output) = &step.observation {
                put(owner, "output", NyaTextFormat::Plain, output);
            }
        }
        for card in snapshot
            .messages
            .iter()
            .flat_map(|message| &message.command_cards)
            .chain(snapshot.command_cards.iter())
        {
            let owner = TextOwner::Command(card.id.clone());
            let heading = command_heading(card);
            let explanation = if card.explanation.trim().is_empty() {
                &heading
            } else {
                &card.explanation
            };
            put(
                owner.clone(),
                "explanation",
                NyaTextFormat::Markdown,
                explanation,
            );
            put(owner.clone(), "heading", NyaTextFormat::Plain, &heading);
            let remainder = card
                .explanation
                .trim()
                .strip_prefix(&heading)
                .map(|rest| rest.trim_start_matches(['，', '。', ',', ';', '；', ' ']))
                .unwrap_or(card.explanation.trim());
            put(owner.clone(), "remainder", NyaTextFormat::Plain, remainder);
            put(owner.clone(), "script", NyaTextFormat::Code, &card.command);
            put(
                owner.clone(),
                "target-label",
                NyaTextFormat::Plain,
                &tool_call_title(&ai_command_target_label(card)),
            );
            put(
                owner.clone(),
                "risk",
                NyaTextFormat::Plain,
                &format!(
                    "{}: {}",
                    t!("ai.commandRisk"),
                    localized_risk(card.risk_level.as_ref())
                ),
            );
            if !card.expected_effect.is_empty() {
                put(
                    owner.clone(),
                    "expected-effect",
                    NyaTextFormat::Plain,
                    &format!("{}: {}", t!("ai.expectedEffect"), card.expected_effect),
                );
            }
            if let Some(rollback) = &card.rollback {
                put(
                    owner.clone(),
                    "rollback",
                    NyaTextFormat::Plain,
                    &format!("{}: {rollback}", t!("ai.rollback")),
                );
            }
            if let Some(host) = card.target.as_ref().and_then(|target| target.host.as_ref()) {
                put(owner.clone(), "host", NyaTextFormat::Plain, host);
            }
            if let Some(session_id) = card.target_terminal_session_id.as_deref().or_else(|| {
                card.target
                    .as_ref()
                    .map(|target| target.terminal_session_id.as_str())
            }) {
                put(
                    owner.clone(),
                    "target-session",
                    NyaTextFormat::Plain,
                    &format!("{}: {session_id}", t!("ai.targetSession")),
                );
            }
            if let Some(reason) = &card.risk_reason {
                put(owner.clone(), "risk-reason", NyaTextFormat::Plain, reason);
            }
            if let Some(presentation) = snapshot.command_step(&card.id) {
                let step = &presentation.step;
                put(owner.clone(), "detail", NyaTextFormat::Plain, &step.detail);
                if let Some(thought) = &step.thought {
                    put(owner.clone(), "thought", NyaTextFormat::Markdown, thought);
                }
                if let Some(output) = &step.observation {
                    if let Some(code) = step.exit_code {
                        put(
                            owner.clone(),
                            "exit-code",
                            NyaTextFormat::Plain,
                            t!("ai.commandExitCode", code = code).as_ref(),
                        );
                    }
                    put(owner.clone(), "output", NyaTextFormat::Plain, output);
                    let preview = if output.trim().is_empty() {
                        t!("ai.commandOutputEmpty").to_string()
                    } else {
                        text_preview(output.trim(), 2, 180)
                    };
                    put(owner, "output-preview", NyaTextFormat::Plain, &preview);
                }
            }
        }
        for view in snapshot.native_approvals.iter() {
            let owner = TextOwner::Approval(view.approval.request_id.clone());
            if let Some(command) = &view.command {
                put(owner.clone(), "script", NyaTextFormat::Code, command);
            }
            put(
                owner.clone(),
                "parameters",
                NyaTextFormat::Plain,
                &format!(
                    "{}: {}",
                    t!("ai.mcp.parameters"),
                    view.approval.parameter_summary
                ),
            );
            if let Some(target) = &view.approval.target {
                put(
                    owner,
                    "target",
                    NyaTextFormat::Plain,
                    &format!("{}: {target}", t!("ai.mcp.target")),
                );
            }
        }
        if let Some(view) = &snapshot.native_run {
            let owner = TextOwner::Run(view.run_id.clone());
            for (index, task) in view.plan.tasks.iter().enumerate() {
                put(
                    owner.clone(),
                    &format!("task-{index}"),
                    NyaTextFormat::Plain,
                    &task.description,
                );
                if let Some(verification) = &task.verification {
                    put(
                        owner.clone(),
                        &format!("task-verification-{index}"),
                        NyaTextFormat::Plain,
                        verification,
                    );
                }
            }
            if let Some(verification) = &view.verification {
                put(
                    owner.clone(),
                    "verification",
                    NyaTextFormat::Plain,
                    &verification.summary,
                );
            }
            for (index, question) in view.questions.iter().enumerate() {
                put(
                    owner.clone(),
                    &format!("question-{index}"),
                    NyaTextFormat::Plain,
                    &question.question,
                );
            }
        }
        let keys: HashSet<_> = sources.keys().cloned().collect();
        self.texts.entries.retain(|key, entry| {
            if keys.contains(key) {
                true
            } else {
                entry.state.clear_selection(cx);
                false
            }
        });
        for (key, (format, source)) in sources {
            if let Some(entry) = self.texts.entries.get_mut(&key) {
                if entry.format != format {
                    entry.state.clear_selection(cx);
                }
                if entry.format != format {
                    self.texts.entries.remove(&key);
                } else {
                    if entry.source != source {
                        entry.state.set_text(&source, cx);
                        entry.source = source;
                    }
                    continue;
                }
            }
            let state = NyaTextState::new(format, &source, cx);
            let owner = key.0.clone();
            let subscription = state.observe_layout(cx, move |panel, cx| {
                let rows = panel.texts.rows.get(&owner).cloned().unwrap_or_default();
                panel.transcript_scroll.update(cx, |scroll, cx| {
                    for index in rows {
                        scroll.remeasure_items(index..index + 1, cx);
                    }
                });
                cx.notify();
            });
            self.texts.entries.insert(
                key,
                TextEntry {
                    source,
                    state,
                    format,
                    visible: false,
                    _layout_subscription: subscription,
                },
            );
        }
        let rows = AiTranscriptRow::project(snapshot);
        let sequence = self.transcript_text_order(snapshot, &rows);
        self.texts.rows.clear();
        let visible: HashSet<_> = sequence.iter().map(|(_, key)| key.clone()).collect();
        for (key, entry) in &mut self.texts.entries {
            if entry.visible && !visible.contains(key) {
                entry.state.clear_selection(cx);
            }
            entry.visible = visible.contains(key);
        }
        let mut included = HashSet::new();
        let mut members = Vec::new();
        for (index, key) in sequence {
            let owned_rows = self.texts.rows.entry(key.0.clone()).or_default();
            if !owned_rows.contains(&index) {
                owned_rows.push(index);
            }
            if included.insert(key.clone())
                && let Some(entry) = self.texts.entries.get(&key)
            {
                members.push(entry.state.clone());
            }
        }
        self.texts.scope.replace_members(members);
    }

    pub(super) fn selectable_text(
        &self,
        owner: TextOwner,
        purpose: &str,
        answer: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(entry) = self
            .texts
            .entries
            .get(&(owner.clone(), purpose.to_string()))
        else {
            return div().into_any_element();
        };
        let palette = self.palette();
        let style = NyaTextStyle::transcript(palette, answer);
        let content = match entry.format {
            NyaTextFormat::Code => NyaSelectableCode::from_state(
                &entry.state,
                style,
                self.command_syntax
                    .highlights
                    .get(&entry.source)
                    .cloned()
                    .unwrap_or_else(|| Arc::from([])),
            )
            .into_any_element(),
            NyaTextFormat::Markdown => NyaMarkdown::from_state(&entry.state)
                .style(style)
                .into_any_element(),
            NyaTextFormat::Plain => NyaSelectableText::from_state(&entry.state)
                .style(style)
                .into_any_element(),
        };
        let menu_scope = self.texts.scope.clone();
        let element = div()
            .id(format!(
                "ai-text-{:?}-{owner:?}-{purpose}",
                self.texts.context
            ))
            .debug_selector({
                let label = format!("ai-text-{owner:?}-{purpose}");
                move || label.clone()
            })
            .min_w_0()
            .w_full()
            .when(answer, |element| {
                element.text_size(px(14.)).line_height(px(22.))
            })
            .child(content);
        let panel = cx.weak_entity();
        let context = self.texts.context.clone();
        let menu = NyaContextMenu::new_dynamic(element, move |window, cx| {
            let selected = menu_scope.selected_text(window, cx);
            let quote = selected.clone();
            let copy = selected;
            let quote_panel = panel.clone();
            let quote_context = context.clone();
            vec![
                NyaMenuItem::action(t!("ai.quote"))
                    .icon("icons/quote.svg")
                    .disabled(quote.trim().is_empty())
                    .on_click(move |_, window, cx| {
                        let text = quote.clone();
                        let _ = quote_panel.update(cx, |panel, cx| {
                            if panel.texts.context != quote_context {
                                return;
                            }
                            panel.with_app(cx, move |app, cx| {
                                app.quote_ai_message_text(text, cx);
                                app.focus_text_input_if_present("ai.chat.prompt", window, cx);
                            })
                        });
                    }),
                NyaMenuItem::action(t!("ai.copy"))
                    .icon("icons/copy.svg")
                    .disabled(copy.is_empty())
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()));
                    }),
            ]
        })
        // Quote / Copy are short labels. The default 220px menu leaves a wide
        // empty track beside the icon; keep this one at the compact menu width.
        .min_width(px(144.));
        let state = entry.state.clone();
        div()
            .min_w_0()
            .w_full()
            .capture_any_mouse_down(move |event, window, cx| {
                if matches!(
                    event.button,
                    gpui::MouseButton::Left | gpui::MouseButton::Right
                ) {
                    state.focus(window, cx);
                }
            })
            .child(self.texts.scope.wrap(menu))
            .into_any_element()
    }

    #[cfg(test)]
    pub(super) fn text_state(&self, owner: TextOwner, purpose: &str) -> Option<NyaTextState> {
        self.texts
            .entries
            .get(&(owner, purpose.to_string()))
            .map(|entry| entry.state.clone())
    }
}
