use gpui::{Context, TestAppContext};
use nyaterm_core::ai::harness::{AgentDecision, AgentRunStatus, AgentToolRegistry};
use nyaterm_core::ai::{AiChatCompletion, AiMode, AiToolCall};
use serde_json::json;

use super::{compact_draw, compact_host};
use crate::features::NyaTermApp;
use crate::features::ai::presentation::AiAgentStepKind;
use crate::features::runtime_jobs::{AiChatJobOutput, AiChatJobResult, AiChatWorkerEvent};
use crate::test_support::{TestAppCleanup, TestConfigDir};

fn finish_decision(app: &mut NyaTermApp, decision: AgentDecision, cx: &mut Context<NyaTermApp>) {
    let launch = app
        .ai
        .begin_chat_request("Inspect".into(), AiMode::Agent, None);
    app.ai.begin_native_run(
        serde_json::from_value(json!({
            "mode":"agent", "action":"generate_command", "userInput":"Inspect"
        }))
        .unwrap(),
    );
    app.dispatch_ai_chat_event(
        AiChatWorkerEvent::Finished(Box::new(AiChatJobResult {
            job_id: launch.job_id,
            session_id: launch.session_id,
            result: Ok(AiChatJobOutput {
                metadata: None,
                native_decision: Some(decision),
                mode: AiMode::Agent,
                text: "Explanation".into(),
                reasoning: None,
                command_cards: vec![],
                auto_execute_first: false,
                approval_note: None,
            }),
        })),
        cx,
    );
    app.flush_ai_panel_snapshot(cx);
}

#[test]
fn completed_text_routes_to_an_unverified_final_step_and_survives_redraw() {
    let mut cx = TestAppContext::single();
    let root = TestConfigDir::new("nyaterm-native-text-decision");
    let _cleanup = TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            finish_decision(app, AgentDecision::FinalText("Explanation".into()), cx);
            assert_eq!(
                app.ai.native_run_view().unwrap().status,
                AgentRunStatus::Unverified
            );
            assert!(!app.ai.chat_is_pending());
            assert!(app.ai.native_pending_call().is_none());
            let step = app.ai.agent_steps().last().unwrap();
            assert_eq!(step.kind, AiAgentStepKind::FinalAnswer);
            assert!(step.detail.contains("Explanation"));
            assert!(step.source_message_id.is_some());
        })
    });
    compact_draw(&app, vcx);
    vcx.update(|_, cx| {
        let app = app.read(cx);
        assert!(
            app.ai
                .chat_messages()
                .last()
                .unwrap()
                .content
                .contains("Explanation")
        );
        assert!(app.ai.native_request_context().unwrap().turns.is_empty());
    });
}

#[test]
fn control_batch_stops_at_the_question_and_cancelled_answers_cannot_execute_its_final() {
    let mut cx = TestAppContext::single();
    let root = TestConfigDir::new("nyaterm-native-batch-question");
    let _cleanup = TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    let decision = AgentToolRegistry::decision(&AiChatCompletion {
        text: "Inspecting".into(),
        reasoning_content: None,
        tool_calls: [
            (
                "plan",
                "update_plan",
                json!({"tasks":[{"id":"inspect","description":"Inspect","status":"in_progress"}]}),
            ),
            (
                "ask",
                "request_user_input",
                json!({"questions":[{"id":"target","question":"Which target?"}]}),
            ),
            (
                "final",
                "final_answer",
                json!({"answer":"Premature answer"}),
            ),
        ]
        .into_iter()
        .map(|(id, name, arguments)| AiToolCall {
            id: Some(id.into()),
            name: name.into(),
            arguments,
            thought_signature: None,
        })
        .collect(),
        metadata: Default::default(),
    })
    .unwrap();
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            finish_decision(app, decision, cx);
            let view = app.ai.native_run_view().unwrap();
            assert_eq!(view.status, AgentRunStatus::WaitingForUser);
            assert_eq!(view.call_id.as_deref(), Some("ask"));
            assert_eq!(view.plan.tasks.len(), 1);
            assert!(app.ai.native_request_context().unwrap().turns.is_empty());
            app.cancel_ai_chat(cx);
            app.submit_native_agent_answers(&view.run_id, "ask", cx);
            assert_eq!(
                app.ai.native_run_view().unwrap().status,
                AgentRunStatus::Cancelled
            );
            let context = app.ai.native_request_context().unwrap();
            assert_eq!(context.turns.len(), 1);
            assert_eq!(context.turns[0].results[2].value["code"], "not_executed");
            assert!(app.ai.begin_native_continuation().is_none());
            app.flush_ai_panel_snapshot(cx);
        })
    });
    compact_draw(&app, vcx);
}

#[test]
fn target_disconnect_cancels_a_background_model_request_and_rejects_its_late_answer() {
    let mut cx = TestAppContext::single();
    let root = TestConfigDir::new("nyaterm-native-background-disconnect");
    let _cleanup = TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.ai.switch_scope("terminal:owner");
            let launch = app
                .ai
                .begin_chat_request("Inspect target".into(), AiMode::Agent, None);
            app.ai.begin_native_run(
                serde_json::from_value(json!({
                    "mode":"agent","action":"generate_command","userInput":"Inspect target",
                    "targets":[{"terminalSessionId":"target","label":"Target","sessionType":"ssh"}]
                }))
                .unwrap(),
            );
            app.ai.switch_scope("terminal:unrelated");
            app.ai
                .begin_chat_request("Unrelated question".into(), AiMode::Ask, None);
            app.mark_session_disconnected("target", cx);
            assert_eq!(app.ai.active_scope_key(), "terminal:unrelated");
            assert!(app.ai.chat_is_pending());
            app.ai.switch_scope("terminal:owner");
            assert_eq!(
                app.ai.native_run_view().unwrap().status,
                AgentRunStatus::Cancelled
            );
            assert!(!app.ai.chat_is_pending());
            assert!(launch.cancel.load(std::sync::atomic::Ordering::Relaxed));
            app.dispatch_ai_chat_event(
                AiChatWorkerEvent::Finished(Box::new(AiChatJobResult {
                    job_id: launch.job_id,
                    session_id: launch.session_id,
                    result: Ok(AiChatJobOutput {
                        metadata: None,
                        native_decision: Some(AgentDecision::FinalText("Stale answer".into())),
                        mode: AiMode::Agent,
                        text: "Stale answer".into(),
                        reasoning: None,
                        command_cards: vec![],
                        auto_execute_first: false,
                        approval_note: None,
                    }),
                })),
                cx,
            );
            assert_eq!(
                app.ai.native_run_view().unwrap().status,
                AgentRunStatus::Cancelled
            );
            assert!(
                !app.ai
                    .chat_messages()
                    .last()
                    .unwrap()
                    .content
                    .contains("Stale answer")
            );
            assert!(app.ai.begin_native_continuation().is_none());
            app.flush_ai_panel_snapshot(cx);
        })
    });
    compact_draw(&app, vcx);
}
