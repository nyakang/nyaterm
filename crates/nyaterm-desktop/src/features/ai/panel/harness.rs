use gpui::{Context, IntoElement, SharedString, div, prelude::*, px, rgb};
use nyaterm_core::ai::harness::{AgentRunStatus, AgentVerificationStatus};
use nyaterm_ui::{NyaButton, NyaInputShell};
use rust_i18n::t;

use super::texts::TextOwner;
use super::{AiPanel, AiPanelSnapshot};
use crate::features::ai::state::harness::NativeRunView;

pub(in crate::features) fn answer_input_id(run_id: &str, call_id: &str, index: usize) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "ai.agent-answer.{run_id}.{}.{}",
        hex::encode(Sha256::digest(call_id.as_bytes())),
        index
    )
}

impl AiPanel {
    pub(super) fn native_run_card(
        &self,
        snapshot: &AiPanelSnapshot,
        view: NativeRunView,
        history: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = snapshot.chrome.palette;
        let mut card = div()
            .id("ai-native-run")
            .debug_selector(|| "ai-native-run".into())
            .w_full()
            .p_2()
            .gap_2()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(rgb(palette.border))
            .text_size(px(12.));
        if history && let Some(verification) = &view.verification {
            let label = match verification.status {
                AgentVerificationStatus::Verified => t!("ai.harness.verified"),
                AgentVerificationStatus::Unverified => t!("ai.harness.unverified"),
                AgentVerificationStatus::Blocked => t!("ai.harness.blocked"),
            };
            card = card.child(div().child(label).child(self.selectable_text(
                TextOwner::Run(view.run_id.clone()),
                "verification",
                false,
                cx,
            )));
        }
        if view.status == AgentRunStatus::WaitingForUser {
            card = card.child(
                div()
                    .font_weight(gpui::FontWeight(600.))
                    .child(t!("ai.harness.waitingForUser")),
            );
            let call_id = view.call_id.clone().expect("pending question call");
            for (index, question) in view.questions.iter().enumerate() {
                let mut question_card =
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(self.selectable_text(
                            TextOwner::Run(view.run_id.clone()),
                            &format!("question-{index}"),
                            false,
                            cx,
                        ));
                if let Some(options) = &question.options {
                    let mut buttons = div().flex().flex_wrap().gap_2();
                    for (option_index, option) in options.iter().enumerate() {
                        let option = option.clone();
                        let selected = view.answers.get(&question.id) == Some(&option);
                        let run_id = view.run_id.clone();
                        let call_id = call_id.clone();
                        let question_id = question.id.clone();
                        let input_id = answer_input_id(&run_id, &call_id, index);
                        buttons = buttons.child(
                            NyaButton::new(
                                format!("agent-choice-{index}-{option_index}"),
                                option.clone(),
                            )
                            .small()
                            .selected(selected)
                            .on_click(cx.listener(
                                move |panel, _, window, cx| {
                                    panel.with_app(cx, |app, cx| {
                                        if !app.apply_native_agent_answer(
                                            &run_id,
                                            &call_id,
                                            &question_id,
                                            option.clone(),
                                            cx,
                                        ) {
                                            return;
                                        }
                                        app.reset_text_input(&input_id, &option, cx);
                                        app.focus_text_input_if_present(&input_id, window, cx);
                                    })
                                },
                            )),
                        );
                    }
                    question_card = question_card.child(buttons);
                }
                if let Some(input) = snapshot.native_answer_inputs.get(index) {
                    question_card = question_card.child(
                        div().w_full().h(px(32.)).child(
                            NyaInputShell::new(
                                SharedString::from(answer_input_id(&view.run_id, &call_id, index)),
                                input,
                            )
                            .height(px(32.)),
                        ),
                    );
                }
                card = card.child(question_card);
            }
            let ready = view.questions.iter().all(|question| {
                view.answers
                    .get(&question.id)
                    .is_some_and(|answer| !answer.trim().is_empty())
            });
            let run_id = view.run_id.clone();
            let submit_call = call_id.clone();
            let cancel_run = run_id.clone();
            let cancel_call = call_id.clone();
            card = card.child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        NyaButton::new("agent-answer-submit", t!("ai.harness.submitAnswers"))
                            .disabled(!ready)
                            .on_click(cx.listener(move |panel, _, window, cx| {
                                panel.with_app(cx, |app, cx| {
                                    app.submit_native_agent_answers(&run_id, &submit_call, cx);
                                    app.focus_text_input_if_present("ai.chat.prompt", window, cx);
                                })
                            })),
                    )
                    .child(
                        NyaButton::new("agent-answer-cancel", t!("ai.harness.cancelRun")).on_click(
                            cx.listener(move |panel, _, window, cx| {
                                panel.with_app(cx, |app, cx| {
                                    if !app.ai.native_question_matches(&cancel_run, &cancel_call) {
                                        return;
                                    }
                                    app.cancel_ai_chat(cx);
                                    app.focus_text_input_if_present("ai.chat.prompt", window, cx);
                                })
                            }),
                        ),
                    ),
            );
        }
        card
    }
}
