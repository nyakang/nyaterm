use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use gpui::TestAppContext;
use nyaterm_core::{
    AiAction, AiAgentKind, AiContext, AiMessage, AiMessageRole, AiMode, AiModelConfigItem,
    AiModelSource, AiProviderCredential, AiProviderKind, AiSession, AiSettings,
};

use crate::features::{
    runtime_jobs::AiAgentLoopState, runtime_jobs::AiAgentStepStatus, runtime_jobs::AiChatJobOutput,
    runtime_jobs::AiChatWorkerEvent, runtime_jobs::AiDiscoveryJobResult,
};
use crate::models::AiPreparedRequest;

use super::{AiFeatureFocus, AiFeatureInit, AiFeatureState, AiSettingsMutation};

#[test]
fn native_continuations_keep_task_language_until_a_new_user_task() {
    use nyaterm_core::ai::harness::{AgentToolRegistry, AgentToolResult};
    use serde_json::json;

    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let request = serde_json::from_value(json!({
        "mode":"agent", "action":"generate_command", "userInput":"inspect",
        "options":{"language":"zh-CN"}
    }))
    .unwrap();
    state.begin_native_run(request);
    let call =
        AgentToolRegistry::parse(&[], r#"{"action":"get_environment","arguments":{}}"#).unwrap();
    let call_id = call.id.clone();
    let (run_id, _, _) = state.begin_native_call(call).unwrap();
    state.complete_native_call(
        &run_id,
        AgentToolResult {
            call_id,
            value: json!({}),
            is_error: false,
        },
    );
    // A new UI selection is only consumed by the next user submission.
    let next_language = crate::i18n::normalize_locale("ja-JP").into_owned();
    state.switch_scope("terminal:other");
    state.switch_scope("unbound:");
    let (_, continuation) = state.begin_native_continuation().unwrap();
    assert_eq!(continuation.options.language, "zh-CN");
    assert_eq!(state.agent_task_language(), "zh-CN");
    state.begin_chat_request_with_language("next task".into(), AiMode::Agent, None, next_language);
    assert_eq!(state.agent_task_language(), "ja");
}

#[test]
fn native_batch_waits_for_each_result_and_preserves_scope_and_cancellation_guards() {
    use nyaterm_core::ai::harness::{AgentDecision, AgentToolRegistry, AgentToolResult};
    use nyaterm_core::ai::{AiChatCompletion, AiToolCall};
    use serde_json::json;

    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.begin_native_run(
        serde_json::from_value(
            json!({"mode":"agent","action":"generate_command","userInput":"inspect"}),
        )
        .unwrap(),
    );
    let completion = AiChatCompletion {
        metadata: Default::default(),
        text: "Inspecting sessions".into(),
        reasoning_content: None,
        tool_calls: ["a", "b", "c"]
            .into_iter()
            .map(|id| AiToolCall {
                id: Some(id.into()),
                name: "get_environment".into(),
                arguments: json!({}),
                thought_signature: None,
            })
            .collect(),
    };
    let AgentDecision::Tools(batch) = AgentToolRegistry::decision(&completion).unwrap() else {
        panic!("expected tools");
    };
    state.queue_native_batch(batch).unwrap();
    assert!(state.begin_native_continuation().is_none());
    let first = state.next_native_call().unwrap();
    let (run_id, _, _) = state.begin_native_call(first).unwrap();
    assert!(state.next_native_call().is_none());
    let scope = state.active_scope_key().to_string();
    state.switch_scope("terminal:other");
    assert!(state.next_native_call().is_none());
    assert!(!state.complete_native_call(
        &run_id,
        AgentToolResult {
            call_id: "a".into(),
            value: json!({}),
            is_error: false
        }
    ));
    state.switch_scope(&scope);
    assert!(state.complete_native_call(
        &run_id,
        AgentToolResult {
            call_id: "a".into(),
            value: json!({}),
            is_error: false
        }
    ));
    assert!(state.begin_native_continuation().is_none());
    let second = state.next_native_call().unwrap();
    assert_eq!(second.id, "b");
    state.begin_native_call(second).unwrap();
    assert!(state.complete_native_call(
        &run_id,
        AgentToolResult {
            call_id: "b".into(),
            value: json!({"code":"denied"}),
            is_error: true
        }
    ));
    assert!(state.next_native_call().is_none());
    let context = state.native_request_context().unwrap();
    assert_eq!(context.turns.len(), 1);
    assert_eq!(context.turns[0].results[2].value["code"], "not_executed");
    state.cancel_chat_and_agent();
    assert!(state.next_native_call().is_none());
    assert!(state.begin_native_continuation().is_none());
}

#[test]
fn native_text_answer_finishes_unverified_without_creating_a_tool_call() {
    use nyaterm_core::ai::harness::AgentRunStatus;
    use serde_json::json;
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.begin_native_run(
        serde_json::from_value(
            json!({"mode":"agent","action":"generate_command","userInput":"explain"}),
        )
        .unwrap(),
    );
    state.finish_native_text("Explanation".into()).unwrap();
    assert_eq!(
        state.native_run_view().unwrap().status,
        AgentRunStatus::Unverified
    );
    assert!(state.native_pending_call().is_none());
    assert!(state.native_request_context().unwrap().turns.is_empty());
    assert!(state.begin_native_continuation().is_none());
}

#[test]
fn legacy_observation_and_rejection_continuations_keep_task_language() {
    use nyaterm_core::ai::{build_agent_rejection_message, build_observation_message};

    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.begin_chat_request_with_language("inspect".into(), AiMode::Agent, None, "zh-CN".into());
    let execution = AiAgentLoopState {
        execution_id: "fixture-execution".into(),
        command_card_id: None,
        ai_session_id: state.chat_session_id().into(),
        terminal_session_id: "terminal-a".into(),
        available_targets: Vec::new(),
        default_target_session_id: None,
        command: "pwd".into(),
        step_index: 0,
        max_steps: 4,
    };
    for rejected in [false, true] {
        state.chat.pending = false;
        let language = state.agent_task_language();
        let message = if rejected {
            build_agent_rejection_message("pwd", language)
        } else {
            build_observation_message(
                &nyaterm_core::CommandObservation {
                    output: "fixture output".into(),
                    exit_code: Some(0),
                    duration_ms: 1,
                },
                "pwd",
                language,
            )
        };
        state
            .begin_agent_continuation(&execution, &message)
            .unwrap();
        assert_eq!(state.agent_task_language(), "zh-CN");
        assert!(
            state
                .agent_conversation_snapshot()
                .last()
                .unwrap()
                .content
                .contains(if rejected {
                    "用户拒绝"
                } else {
                    "执行完成"
                })
        );
    }
    state.begin_chat_request_with_language(
        "next task".into(),
        AiMode::Agent,
        None,
        crate::i18n::normalize_locale("ja_JP").into_owned(),
    );
    assert_eq!(state.agent_task_language(), "ja");
}

#[test]
fn history_load_keeps_the_current_chat_on_failure_or_after_a_new_request() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let launch = state.begin_chat_request("original".into(), AiMode::Ask, None);
    state.apply_chat_delta(launch.job_id, "answer", None);
    state.cancel_chat_and_agent();
    let source = state.chat_session_id().to_string();
    let messages = state.chat_messages().to_vec();
    let job = state.begin_history_operation("load").unwrap();
    assert!(state.history_load_is_current(job, "unbound:", &source));
    state.finish_history_message_load(
        job,
        &source,
        "other".into(),
        Err("fixture failure".into()),
        "loaded".into(),
    );
    assert_eq!(state.chat_session_id(), source);
    assert_eq!(state.chat_messages(), messages);
    assert_eq!(state.history_error(), Some("fixture failure"));

    let job = state.begin_history_operation("load").unwrap();
    let (epoch, captured) = state.visible_scope_guard();
    state.begin_chat_request("new question".into(), AiMode::Ask, None);
    assert!(!state.history_load_is_current(job, "unbound:", &source));
    assert_ne!(epoch.load(Ordering::Acquire), captured);
    let current = state.chat_messages().to_vec();
    state.finish_history_message_load(
        job,
        &source,
        "other".into(),
        Ok(Vec::new()),
        "loaded".into(),
    );
    assert_eq!(state.chat_session_id(), source);
    assert_eq!(state.chat_messages(), current);
}

#[test]
fn switching_terminals_or_starting_a_new_chat_invalidates_the_history_binding_guard() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.switch_visible_scope("terminal:a");
    let source = state.chat_session_id().to_string();
    let job = state.begin_history_operation("load").unwrap();
    let (epoch, captured) = state.visible_scope_guard();
    state.switch_visible_scope("terminal:b");
    assert!(!state.history_load_is_current(job, "terminal:a", &source));
    state.switch_visible_scope("terminal:a");
    assert_ne!(epoch.load(Ordering::Acquire), captured);
    state.start_new_chat();
    assert!(!state.history_load_is_current(job, "terminal:a", &source));
    let current = state.chat_session_id().to_string();
    state.finish_history_message_load(
        job,
        &source,
        "other".into(),
        Ok(Vec::new()),
        "loaded".into(),
    );
    assert_eq!(state.chat_session_id(), current);
}

#[test]
fn stream_refreshes_coalesce_and_immediate_flush_invalidates_old_timers() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let generation = state.request_stream_refresh().unwrap();
    assert!(state.request_stream_refresh().is_none());
    state.clear_panel_refresh_request();
    assert!(!state.take_stream_refresh(generation));
    let next = state.request_stream_refresh().unwrap();
    assert!(state.take_stream_refresh(next));
    assert!(!state.take_stream_refresh(next));
}

#[test]
fn native_questions_keep_the_owner_run_across_scope_switches_and_reject_stale_answers() {
    use nyaterm_core::ai::harness::{AgentRunStatus, AgentToolRegistry, AgentToolResult};
    use serde_json::json;
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.switch_scope("terminal:a");
    let request = serde_json::from_value(
        json!({"mode":"agent","action":"generate_command","userInput":"diagnose",
        "targets":[{"terminalSessionId":"a","label":"A","sessionType":"local"}]}),
    )
    .unwrap();
    state.begin_native_run(request);
    let call = AgentToolRegistry::parse(&[],r#"{"action":"request_user_input","arguments":{"questions":[{"id":"choice","question":"Which service?","options":["web","db"]}]}}"#).unwrap();
    let call_id = call.id.clone();
    let questions = serde_json::from_value(call.arguments["questions"].clone()).unwrap();
    let (run_id, _, _) = state.begin_native_call(call).unwrap();
    state.wait_native_user(questions).unwrap();
    assert!(state.native_answers(&run_id, &call_id).is_none());
    state.switch_scope("terminal:b");
    assert!(!state.set_native_answer(&run_id, &call_id, "choice", "web".into()));
    state.switch_scope("terminal:a");
    assert_eq!(
        state.native_run_view().unwrap().status,
        AgentRunStatus::WaitingForUser
    );
    assert!(!state.set_native_answer(&run_id, "stale-call", "choice", "web".into()));
    assert!(state.set_native_answer(&run_id, &call_id, "choice", "web".into()));
    let value = state.native_answers(&run_id, &call_id).unwrap();
    let result = AgentToolResult {
        call_id: call_id.clone(),
        value,
        is_error: false,
    };
    assert!(state.complete_native_call(&run_id, result.clone()));
    assert!(!state.complete_native_call(&run_id, result));
    assert!(!state.native_question_matches(&run_id, &call_id));
    assert_eq!(state.native_request_context().unwrap().turns.len(), 1);
    let read = AgentToolRegistry::parse(
        &[],
        r#"{"action":"session_get","arguments":{"sessionId":"a"}}"#,
    )
    .unwrap();
    let read_id = read.id.clone();
    state.begin_native_call(read).unwrap();
    state.cancel_chat_and_agent();
    assert!(!state.complete_native_call(
        &run_id,
        AgentToolResult {
            call_id: read_id,
            value: json!({}),
            is_error: false
        }
    ));
    assert_eq!(
        state.native_run_view().unwrap().status,
        AgentRunStatus::Cancelled
    );
}

#[test]
fn native_provider_failure_preserves_the_previous_completed_tool_step() {
    use crate::features::ai::presentation::AiAgentStepKind;
    use nyaterm_core::ai::harness::{AgentRunStatus, AgentToolRegistry, AgentToolResult};
    use serde_json::json;
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.begin_native_run(
        serde_json::from_value(
            json!({"action":"generate_command","mode":"agent","userInput":"inspect"}),
        )
        .unwrap(),
    );
    let call =
        AgentToolRegistry::parse(&[], r#"{"action":"get_environment","arguments":{}}"#).unwrap();
    let call_id = call.id.clone();
    let (run_id, _, _) = state.begin_native_call(call).unwrap();
    state.upsert_agent_step(
        0,
        AiAgentStepStatus::Completed,
        AiAgentStepKind::ToolProgress,
        "Tool result",
        "read completed",
    );
    assert!(state.complete_native_call(
        &run_id,
        AgentToolResult {
            call_id,
            value: json!({}),
            is_error: false
        }
    ));
    let (launch, _) = state.begin_native_continuation().unwrap();
    state
        .finish_chat_job(
            launch.job_id,
            launch.session_id,
            Err("fixture provider unavailable".into()),
        )
        .unwrap();
    assert_eq!(state.agent_steps()[0].status, AiAgentStepStatus::Completed);
    assert_eq!(state.agent_steps()[1].status, AiAgentStepStatus::Failed);
    assert_eq!(
        state.native_run_view().unwrap().status,
        AgentRunStatus::Failed
    );
}

#[test]
fn protocol_repair_clears_only_the_active_response_and_keeps_completed_steps() {
    use crate::features::ai::presentation::AiAgentStepKind;
    use nyaterm_core::ai::harness::{AgentToolRegistry, AgentToolResult};
    use serde_json::json;

    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.begin_native_run(
        serde_json::from_value(
            json!({"action":"generate_command","mode":"agent","userInput":"inspect"}),
        )
        .unwrap(),
    );
    let call =
        AgentToolRegistry::parse(&[], r#"{"action":"get_environment","arguments":{}}"#).unwrap();
    let call_id = call.id.clone();
    let (run_id, _, _) = state.begin_native_call(call).unwrap();
    state.upsert_agent_step(
        0,
        AiAgentStepStatus::Completed,
        AiAgentStepKind::ToolProgress,
        "Tool result",
        "read completed",
    );
    assert!(state.complete_native_call(
        &run_id,
        AgentToolResult {
            call_id,
            value: json!({"previous":"result"}),
            is_error: false
        }
    ));
    let records = state.agent.native.as_ref().unwrap().run.turns.clone();
    let (launch, _) = state.begin_native_continuation().unwrap();
    state.apply_chat_delta(launch.job_id, "rejected text", Some("rejected reasoning"));
    let messages = state.chat.messages.clone();
    assert!(!state.apply_agent_protocol_repair(launch.job_id + 1));
    assert_eq!(state.chat.messages, messages);
    let json_protocol = state.agent.json_protocol;
    assert!(state.apply_agent_protocol_repair(launch.job_id));
    let current_id = state.chat.streaming_assistant_id.as_ref().unwrap();
    let current = state
        .chat
        .messages
        .iter()
        .find(|message| &message.id == current_id)
        .unwrap();
    assert!(current.content.is_empty());
    assert!(current.reasoning_content.is_none());
    assert!(state.chat.response_preview.is_empty());
    assert!(state.chat.pending);
    assert_eq!(state.agent.json_protocol, json_protocol);
    assert_eq!(state.agent.native.as_ref().unwrap().run.turns, records);
    assert_eq!(state.agent_steps()[0].status, AiAgentStepStatus::Completed);
    assert!(state.panel.status.contains("retry 1/1"));
    state.cancel_chat_and_agent();
    assert!(!state.apply_agent_protocol_repair(launch.job_id));
}

fn state(cx: &TestAppContext) -> AiFeatureState {
    let focus = cx.update(|cx| AiFeatureFocus {
        chat: cx.focus_handle(),
        manual_model: cx.focus_handle(),
    });
    AiFeatureState::new(
        AiFeatureInit {
            settings: AiSettings::default(),
            model_draft: "model-a".to_string(),
            base_url_draft: "https://example.invalid".to_string(),
            chat_session_id: "session-a".to_string(),
            session_count: 0,
            message_count: 0,
            audit_count: 0,
        },
        focus,
    )
}

#[test]
fn composer_resize_floors_at_auto_grow_and_caps_at_max() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let natural = gpui::px(136.);
    let maximum = gpui::px(480.);

    assert_eq!(state.composer_min_height(), 0.);
    assert_eq!(
        state.update_composer_resize(gpui::px(0.), natural, maximum),
        None
    );

    state.start_composer_resize(gpui::px(400.), natural);
    assert!(state.composer_is_resizing());

    // Upward drag adds height anchored at the pressed box height.
    assert_eq!(
        state.update_composer_resize(gpui::px(350.), natural, maximum),
        Some(186.)
    );
    // Past the cap the floor stops following the pointer.
    assert_eq!(
        state.update_composer_resize(gpui::px(0.), natural, maximum),
        Some(480.)
    );
    // At or below the natural height the override drops back to auto-grow.
    assert_eq!(
        state.update_composer_resize(gpui::px(400.), natural, maximum),
        Some(0.)
    );
    assert_eq!(
        state.update_composer_resize(gpui::px(800.), natural, maximum),
        Some(0.)
    );

    assert!(state.finish_composer_resize());
    assert!(!state.composer_is_resizing());
    assert_eq!(
        state.update_composer_resize(gpui::px(0.), natural, maximum),
        None
    );
    assert!(!state.finish_composer_resize());
}

#[test]
fn composer_resize_preserves_hidden_floor_until_the_user_drags_down() {
    let cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let natural = gpui::px(80.);
    state.start_composer_resize(gpui::px(400.), natural);
    assert_eq!(
        state.update_composer_resize(gpui::px(0.), natural, gpui::px(480.)),
        Some(480.)
    );
    state.finish_composer_resize();

    // A small panel displays 120px but preserves the original 480px floor.
    state.start_composer_resize(gpui::px(400.), gpui::px(120.));
    assert_eq!(
        state.update_composer_resize(gpui::px(400.), natural, gpui::px(120.)),
        Some(480.)
    );
    assert_eq!(
        state.update_composer_resize(gpui::px(300.), natural, gpui::px(120.)),
        Some(480.)
    );
    assert_eq!(
        state.update_composer_resize(gpui::px(401.), natural, gpui::px(120.)),
        Some(119.)
    );
    assert_eq!(
        state.update_composer_resize(gpui::px(440.), natural, gpui::px(120.)),
        Some(0.)
    );
    state.finish_composer_resize();

    // A ceiling below natural height is the reset threshold for a downward drag.
    state.start_composer_resize(gpui::px(400.), gpui::px(60.));
    assert_eq!(
        state.update_composer_resize(gpui::px(399.), natural, gpui::px(60.)),
        Some(0.)
    );
    assert_eq!(
        state.update_composer_resize(gpui::px(401.), natural, gpui::px(60.)),
        Some(0.)
    );
}

#[test]
fn composer_resize_remains_continuous_after_natural_height_outgrows_the_floor() {
    let cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.start_composer_resize(gpui::px(400.), gpui::px(80.));
    assert_eq!(
        state.update_composer_resize(gpui::px(370.), gpui::px(80.), gpui::px(480.)),
        Some(110.)
    );
    state.finish_composer_resize();
    state.start_composer_resize(gpui::px(400.), gpui::px(136.));
    assert_eq!(
        state.update_composer_resize(gpui::px(400.), gpui::px(136.), gpui::px(300.)),
        Some(110.)
    );
    assert_eq!(
        state.update_composer_resize(gpui::px(399.), gpui::px(136.), gpui::px(300.)),
        Some(137.)
    );
    assert_eq!(
        state.update_composer_resize(gpui::px(0.), gpui::px(136.), gpui::px(300.)),
        Some(300.)
    );
    assert_eq!(
        state.update_composer_resize(gpui::px(399.), gpui::px(200.), gpui::px(300.)),
        Some(137.),
        "content growing during an upward drag must not clear the floor"
    );
}

#[test]
fn conversation_title_uses_the_first_nonempty_user_message_without_cutting_unicode() {
    let cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.set_chat_prompt_draft("unsent draft".into());
    assert_eq!(state.conversation_title(), None);

    let question = format!(
        "  > 引用\n\n **MixedCase**\t中文 👩🏽‍💻 e\u{301} {}  ",
        "长问题".repeat(100)
    );
    let expected = format!(
        "> 引用 **MixedCase** 中文 👩🏽‍💻 e\u{301} {}",
        "长问题".repeat(100)
    );
    for (index, (role, content)) in [
        (AiMessageRole::System, "system context".into()),
        (AiMessageRole::Assistant, "assistant content".into()),
        (AiMessageRole::User, " \n\t ".into()),
        (AiMessageRole::User, question),
        (AiMessageRole::User, "later question".into()),
    ]
    .into_iter()
    .enumerate()
    {
        state.chat.messages.push(Arc::new(AiMessage {
            id: format!("message-{index}"),
            session_id: state.chat_session_id().into(),
            role,
            content,
            created_at: "2026-11-07T00:00:00Z".into(),
            reasoning_content: None,
            command_cards: Vec::new(),
        }));
    }
    assert_eq!(
        state.conversation_title().as_deref(),
        Some(expected.as_str())
    );
}

#[test]
fn conversation_title_follows_history_and_terminal_scopes_and_resets_for_new_chats() {
    let cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.switch_visible_scope("terminal:a");
    state.begin_chat_request("First question A".into(), AiMode::Ask, None);
    state.cancel_chat_and_agent();
    let saved_id = state.chat_session_id().to_string();
    let saved_messages = state
        .chat_messages()
        .iter()
        .map(|message| (**message).clone())
        .collect();
    state.begin_chat_request("Follow-up A".into(), AiMode::Ask, None);
    state.cancel_chat_and_agent();
    assert_eq!(
        state.conversation_title().as_deref(),
        Some("First question A")
    );

    state.switch_visible_scope("terminal:b");
    assert_eq!(state.conversation_title(), None);
    state.begin_chat_request("First question B".into(), AiMode::Agent, None);
    state.cancel_chat_and_agent();
    assert_eq!(
        state.conversation_title().as_deref(),
        Some("First question B")
    );
    state.switch_visible_scope("terminal:a");
    assert_eq!(
        state.conversation_title().as_deref(),
        Some("First question A")
    );
    state.start_new_chat();
    assert_eq!(state.conversation_title(), None);

    let source_id = state.chat_session_id().to_string();
    let job_id = state.begin_history_operation("load").unwrap();
    assert!(state.finish_history_message_load(
        job_id,
        &source_id,
        saved_id.clone(),
        Ok(saved_messages),
        "loaded".into(),
    ));
    assert_eq!(
        state.conversation_title().as_deref(),
        Some("First question A")
    );
    state.toggle_history();
    state.close_history();
    assert_eq!(
        state.conversation_title().as_deref(),
        Some("First question A")
    );

    let job_id = state.begin_history_operation("delete").unwrap();
    assert_eq!(
        state.finish_history_session_delete(job_id, &saved_id, Ok(())),
        Some(true)
    );
    assert_eq!(state.conversation_title(), None);
}

#[test]
fn settings_draft_restore_and_replacement_keep_related_values_together() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.apply_settings_input(crate::models::AiInputField::ApiKey, "secret".to_string());
    let snapshot = state.settings_draft_snapshot();

    state.apply_settings_input(crate::models::AiInputField::Model, "changed".to_string());
    assert!(!state.settings_draft_matches(&snapshot.0, &snapshot.1, &snapshot.2, &snapshot.3));

    state.restore_settings_draft(snapshot.0, snapshot.1, snapshot.2, snapshot.3);
    let restored = state.settings_draft_snapshot();
    assert_eq!(restored.1, "model-a");
    assert_eq!(restored.3.expose_secret(), "secret");

    state.replace_settings_config(AiSettings::default(), true);
    assert!(state.settings_draft_snapshot().3.is_empty());
}

#[test]
fn pending_settings_preserve_masked_secret_until_a_new_draft_exists() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.settings.config.provider_profiles[0].api_key = Some("__SET__".to_string().into());
    state.settings.config.provider_credentials[0].api_key = Some("__SET__".to_string().into());

    let pending = state.pending_settings();
    assert_eq!(
        pending.provider_profiles[0].api_key.as_deref(),
        Some("__SET__")
    );
    assert_eq!(
        pending.provider_credentials[0].api_key.as_deref(),
        Some("__SET__")
    );

    state.apply_settings_input(
        crate::models::AiInputField::ApiKey,
        "replacement".to_string(),
    );
    let pending = state.pending_settings();
    assert_eq!(
        pending.provider_profiles[0].api_key.as_deref(),
        Some("replacement")
    );
    assert_eq!(
        pending.provider_credentials[0].api_key.as_deref(),
        Some("replacement")
    );
}

#[test]
fn external_agent_mcp_integration_toggles_fail_closed_setting() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);

    state.toggle_settings_codex_mcp_integration();
    state.toggle_settings_claude_mcp_integration();
    let disabled = state.pending_settings();
    assert_eq!(disabled.codex.tool_integration_mode, None);
    assert_eq!(disabled.claude_code.tool_integration_mode, None);

    state.toggle_settings_codex_mcp_integration();
    state.toggle_settings_claude_mcp_integration();
    let enabled = state.pending_settings();
    assert_eq!(
        enabled.codex.tool_integration_mode.as_deref(),
        Some("nyaterm_mcp")
    );
    assert_eq!(
        enabled.claude_code.tool_integration_mode.as_deref(),
        Some("nyaterm_mcp")
    );
}

#[test]
fn ai_settings_persistence_ignores_old_completion_and_retries_latest_snapshot() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let first_snapshot = state.settings_config_cloned();
    let (first_generation, _) = state
        .queue_settings_persistence(first_snapshot)
        .expect("first save should start");
    state.toggle_settings_enabled();
    let latest_snapshot = state.settings_config_cloned();
    assert!(
        state
            .queue_settings_persistence(latest_snapshot.clone())
            .is_none()
    );

    let first = state.finish_settings_persistence(first_generation, true);
    assert!(!first.apply_result);
    let (latest_generation, queued) = first.next.expect("latest snapshot should follow");
    assert_eq!(queued.enabled, latest_snapshot.enabled);

    let failed = state.finish_settings_persistence(latest_generation, false);
    assert!(failed.report_result);
    assert!(state.settings_persistence_is_dirty());

    let retry_snapshot = state.settings_config_cloned();
    let (retry_generation, _) = state
        .queue_settings_persistence(retry_snapshot)
        .expect("retry should submit");
    let retried = state.finish_settings_persistence(retry_generation, true);
    assert!(retried.apply_result);
    assert!(!state.settings_persistence_is_dirty());
}

#[test]
fn model_catalog_mutations_keep_default_model_valid() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let first = "openai:model-a".to_string();
    let fallback = "openai:model-b".to_string();
    state.settings.config.models = vec![
        AiModelConfigItem {
            supported_reasoning_efforts: None,
            backend: Default::default(),
            id: first.clone(),
            name: "model-a".to_string(),
            provider_kind: Some(AiProviderKind::Openai),
            credential_id: None,
            enabled: true,
            source: AiModelSource::RustGenai,
            last_seen_at: None,
        },
        AiModelConfigItem {
            supported_reasoning_efforts: None,
            backend: Default::default(),
            id: fallback.clone(),
            name: "model-b".to_string(),
            provider_kind: Some(AiProviderKind::Openai),
            credential_id: None,
            enabled: true,
            source: AiModelSource::RustGenai,
            last_seen_at: None,
        },
    ];
    state.settings.config.default_model_id = Some(first.clone());

    state.toggle_settings_model_enabled(&first);
    assert_eq!(
        state.settings.config.default_model_id.as_deref(),
        Some(fallback.as_str())
    );
    assert!(state.default_model_is_enabled());

    state
        .settings
        .config
        .provider_credentials
        .push(AiProviderCredential {
            icon_data_url: None,
            api_protocol: None,
            api_format: Default::default(),
            id: "custom".to_string(),
            name: "Custom".to_string(),
            provider_kind: AiProviderKind::OpenaiCompatible,
            base_url: Some("https://example.invalid".to_string()),
            api_key: None,
            extra_headers: Vec::new(),
            enabled: true,
        });
    state.settings.config.default_model_id = None;
    assert_eq!(
        state.add_settings_manual_model("custom", "model-x"),
        AiSettingsMutation::Persist
    );
    let manual_id = state.settings.config.default_model_id.clone().unwrap();
    assert_eq!(
        state.remove_settings_manual_model(&manual_id),
        AiSettingsMutation::Persist
    );
    assert!(state.default_model_is_enabled());
}

#[test]
fn credential_edits_move_secret_drafts_into_both_compatible_records() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    assert!(state.apply_settings_credential_input("openai.api-key", "new-key".to_string()));
    state.commit_settings_credential_edits("openai");

    assert_eq!(
        state.settings.config.provider_credentials[0]
            .api_key
            .as_deref(),
        Some("new-key")
    );
    assert_eq!(
        state.settings.config.provider_profiles[0]
            .api_key
            .as_deref(),
        Some("new-key")
    );
    assert!(
        !state
            .settings
            .credential_secret_drafts
            .contains_key("openai")
    );
}

#[test]
fn credential_header_edits_flow_through_drafts_into_config() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let header_id = state
        .add_settings_credential_header("openai")
        .expect("add header");
    assert!(state.apply_settings_credential_input(
        &format!("openai.header.{header_id}.name"),
        "X-Token".to_string(),
    ));
    assert!(state.apply_settings_credential_input(
        &format!("openai.header.{header_id}.value"),
        "secret".to_string(),
    ));

    let pending = state.pending_settings();
    let pending_header = pending.provider_credentials[0]
        .extra_headers
        .iter()
        .find(|header| header.id == header_id)
        .expect("pending header");
    assert_eq!(pending_header.name, "X-Token");
    assert_eq!(pending_header.value.expose_secret(), "secret");
    assert!(
        state.settings.config.provider_credentials[0]
            .extra_headers
            .iter()
            .all(|header| header.value.is_empty())
    );

    state.commit_settings_credential_edits("openai");
    let stored = state.settings.config.provider_credentials[0]
        .extra_headers
        .iter()
        .find(|header| header.id == header_id)
        .expect("stored header");
    assert_eq!(stored.name, "X-Token");
    assert_eq!(stored.value.expose_secret(), "secret");
    assert!(
        !state
            .settings
            .credential_header_secret_drafts
            .contains_key(&("openai".to_string(), header_id.clone()))
    );

    assert!(state.remove_settings_credential_header("openai", &header_id));
    assert!(
        state.settings.config.provider_credentials[0]
            .extra_headers
            .is_empty()
    );
}

#[test]
fn header_drafts_with_repeated_ids_stay_with_their_credential() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    for credential in &mut state.settings.config.provider_credentials[..2] {
        credential.extra_headers = vec![nyaterm_core::AiHttpHeader {
            id: "same-id".into(),
            name: "X-Token".into(),
            value: "stored-fixture".into(),
        }];
    }
    let other_id = state.settings.config.provider_credentials[1].id.clone();
    assert!(
        state.apply_settings_credential_input(
            "openai.header.same-id.value",
            "edited-fixture".into()
        )
    );
    state.select_settings_provider(Some(other_id.clone()));
    let pending = state.pending_settings();
    assert_eq!(
        pending.provider_credentials[0].extra_headers[0]
            .value
            .expose_secret(),
        "edited-fixture"
    );
    assert_eq!(
        pending.provider_credentials[1].extra_headers[0]
            .value
            .expose_secret(),
        "stored-fixture"
    );
    assert!(
        !state.apply_settings_credential_input("missing.header.same-id.value", "ignored".into())
    );
    assert!(
        !state.apply_settings_credential_input("openai.header.missing.value", "ignored".into())
    );
    assert!(state.remove_settings_credential_header(&other_id, "same-id"));
    assert_eq!(
        state.pending_settings().provider_credentials[0].extra_headers[0]
            .value
            .expose_secret(),
        "edited-fixture"
    );
    state.replace_settings_config(AiSettings::default(), true);
    assert!(state.settings.credential_header_secret_drafts.is_empty());
}

#[test]
fn header_editor_survives_save_completion_and_restores_cancel_snapshot() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let original = state.settings_draft_snapshot();
    let header_id = state.add_settings_credential_header("openai").unwrap();
    let blank = state.pending_settings();
    let mut saved = blank.clone();
    nyaterm_core::normalize_ai_settings(&mut saved);
    state.accept_saved_settings(saved, &blank);
    assert_eq!(
        state.settings.config.provider_credentials[0]
            .extra_headers
            .len(),
        1
    );
    let submitted = state.pending_settings();
    state.apply_settings_credential_input(
        &format!("openai.header.{header_id}.name"),
        "X-Token".into(),
    );
    state.apply_settings_credential_input(
        &format!("openai.header.{header_id}.value"),
        "new-fixture".into(),
    );
    state.accept_saved_settings(submitted.clone(), &submitted);
    assert_eq!(
        state.pending_settings().provider_credentials[0].extra_headers[0]
            .value
            .expose_secret(),
        "new-fixture"
    );
    assert!(state.settings_header_errors().is_empty());
    let submitted = state.pending_settings();
    state.accept_saved_settings(
        nyaterm_core::mask_ai_settings(submitted.clone()),
        &submitted,
    );
    assert!(state.settings.credential_header_secret_drafts.is_empty());
    state.restore_settings_draft(original.0, original.1, original.2, original.3);
    assert!(
        state.pending_settings().provider_credentials[0]
            .extra_headers
            .is_empty()
    );
}

#[test]
fn credential_catalog_changes_preserve_an_absent_default_model() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.settings.config.default_model_id = None;
    state.settings.config.models.push(AiModelConfigItem {
        supported_reasoning_efforts: None,
        backend: Default::default(),
        id: "openai:model-a".to_string(),
        name: "model-a".to_string(),
        provider_kind: Some(AiProviderKind::Openai),
        credential_id: None,
        enabled: true,
        source: AiModelSource::RustGenai,
        last_seen_at: None,
    });

    assert_eq!(
        state.toggle_settings_credential_enabled("openai"),
        AiSettingsMutation::Persist
    );
    assert!(state.settings.config.default_model_id.is_none());

    state
        .settings
        .config
        .provider_credentials
        .push(AiProviderCredential {
            icon_data_url: None,
            api_protocol: None,
            api_format: Default::default(),
            id: "custom".to_string(),
            name: "Custom".to_string(),
            provider_kind: AiProviderKind::OpenaiCompatible,
            base_url: Some("https://example.invalid".to_string()),
            api_key: None,
            extra_headers: Vec::new(),
            enabled: true,
        });
    assert_eq!(
        state.remove_settings_credential("custom"),
        AiSettingsMutation::Persist
    );
    assert!(state.settings.config.default_model_id.is_none());
}

#[test]
fn action_and_discovery_catalog_updates_stay_on_settings_owner() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.add_settings_action(
        crate::models::AiActionListKind::Terminal,
        "custom-action".to_string(),
    );
    assert!(state.apply_settings_action_input(
        crate::models::AiActionListKind::Terminal,
        "custom-action",
        crate::models::AiActionEditorField::Prompt,
        "explain".to_string(),
    ));
    assert_eq!(
        state.settings_action_value(
            crate::models::AiActionListKind::Terminal,
            "custom-action",
            crate::models::AiActionEditorField::Prompt,
        ),
        "explain"
    );

    let discovery = nyaterm_core::AiModelDiscovery {
        id: "custom:model-y".to_string(),
        name: "model-y".to_string(),
        provider_kind: Some(AiProviderKind::OpenaiCompatible),
        credential_id: Some("custom".to_string()),
        source: AiModelSource::Manual,
    };
    assert_eq!(state.apply_settings_model_discoveries(vec![discovery]), 1);
    assert!(
        state
            .settings
            .config
            .models
            .iter()
            .any(|model| model.id == "custom:model-y")
    );
}

#[test]
fn transient_ai_menus_are_mutually_exclusive() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);

    assert!(!state.transient_menus_are_open());
    assert!(state.toggle_execution_menu());
    assert!(state.transient_menus_are_open());
    assert!(state.toggle_discovery_menu(3));
    assert!(!state.panel_execution_menu_is_open());
    assert!(state.discovery_menu_is_open());

    assert!(state.toggle_history());
    assert!(!state.discovery_menu_is_open());
    state.close_transient_menus();
    assert!(!state.transient_menus_are_open());
}

#[test]
fn history_and_auto_execution_confirmations_transition_on_the_owner() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.history.sessions.push(AiSession {
        agent_kind: Default::default(),
        scope: Default::default(),
        external_session_id: None,
        backend_metadata: None,
        id: "history".to_string(),
        connection_id: None,
        title: "History".to_string(),
        created_at: String::new(),
        updated_at: String::new(),
    });

    assert!(state.request_history_clear_confirm());
    assert!(state.confirm_history_clear());

    state.request_agent_auto_confirm();
    assert!(state.confirm_agent_auto_execution());
    assert_eq!(state.panel_status(), "Agent execution mode: auto");
}

#[test]
fn history_jobs_reject_overlap_and_ignore_stale_completions() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);

    let first = state.begin_history_operation("first").unwrap();
    assert!(state.begin_history_operation("overlap").is_none());
    assert!(state.history_is_pending());
    assert_eq!(
        state.panel_status(),
        "AI history operation already in progress"
    );
    assert!(state.finish_history_session_list(first, Ok(Vec::new())));

    let second = state.begin_history_operation("second").unwrap();
    assert!(!state.finish_history_session_list(first, Ok(Vec::new())));
    assert!(state.history_is_pending());
    assert!(state.finish_history_session_list(second, Ok(Vec::new())));
    assert!(!state.history_is_pending());
}

#[test]
fn history_usage_counts_ignore_superseded_jobs() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);

    let first = state.begin_history_usage_count_job();
    let second = state.begin_history_usage_count_job();
    assert!(!state.finish_history_usage_counts(first, Ok((1, 2, 3))));
    assert_eq!(
        (
            state.history.session_count,
            state.history.message_count,
            state.history.audit_count,
        ),
        (0, 0, 0)
    );
    assert!(state.finish_history_usage_counts(second, Ok((4, 5, 6))));
    assert_eq!(
        (
            state.history.session_count,
            state.history.message_count,
            state.history.audit_count,
        ),
        (4, 5, 6)
    );
}

#[test]
fn history_completion_updates_history_and_chat_atomically() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.history.sessions = vec![AiSession {
        id: "session-a".to_string(),
        agent_kind: Default::default(),
        scope: Default::default(),
        connection_id: None,
        title: "Session A".to_string(),
        created_at: String::new(),
        updated_at: String::new(),
        external_session_id: None,
        backend_metadata: None,
    }];
    state.chat.messages.push(Arc::new(AiMessage {
        id: "assistant-a".to_string(),
        session_id: "session-a".to_string(),
        role: AiMessageRole::Assistant,
        content: "answer".to_string(),
        created_at: String::new(),
        reasoning_content: None,
        command_cards: Vec::new(),
    }));

    let delete_job = state.begin_history_operation("delete").unwrap();
    assert_eq!(
        state.finish_history_session_delete(delete_job, "session-a", Ok(())),
        Some(true)
    );
    assert!(state.history_sessions().is_empty());
    assert!(state.chat_messages().is_empty());
    assert_ne!(state.chat_session_id(), "session-a");

    state.set_chat_run_mode(AiMode::Agent, Default::default());
    state.history.sessions.push(AiSession {
        agent_kind: Default::default(),
        scope: Default::default(),
        external_session_id: None,
        backend_metadata: None,
        id: state.chat_session_id().to_string(),
        connection_id: None,
        title: "Current".to_string(),
        created_at: String::new(),
        updated_at: String::new(),
    });
    let source_session_id = state.chat_session_id().to_string();
    let clear_job = state.begin_history_operation("clear").unwrap();
    assert_eq!(
        state.finish_history_clear(clear_job, &source_session_id, Ok(())),
        Some(true)
    );
    assert!(state.history_sessions().is_empty());
    assert!(state.history_query().is_empty());
    assert_eq!(state.chat_response_preview(), "Agent mode ready");
    assert_eq!(state.panel_status(), "AI history cleared");
}

#[test]
fn discovery_job_and_picker_lifecycles_stay_on_the_owner() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);

    let mut rx = state
        .take_discovery_event_receiver()
        .expect("the state holds its receiver until the drain starts");
    let tx = state.begin_discovery_job().unwrap();
    assert!(state.begin_discovery_job().is_none());
    tx.unbounded_send(AiDiscoveryJobResult {
        profile_id: "profile".to_string(),
        result: Ok(Vec::new()),
    })
    .unwrap();
    assert!(rx.try_recv().is_ok());
    state.note_discovery_event_delivered();
    assert!(!state.discovery_is_pending());

    state.toggle_discovery_menu(2);
    state.set_discovery_query("server".to_string());
    state.move_discovery_index(3, 1);
    state.move_discovery_index(3, -1);
    assert_eq!(state.discovery_index(), 0);
    assert!(state.escape_discovery_search(2));
    assert_eq!(state.discovery_index(), 2);
    assert!(state.discovery_query().is_empty());
    assert!(!state.escape_discovery_search(1));
    assert!(!state.discovery_menu_is_open());
}

#[test]
fn chat_start_stream_and_finish_are_reduced_by_the_owner() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.set_chat_prompt_draft("deploy".to_string());

    let launch = state.begin_chat_request("deploy".to_string(), AiMode::Agent, None);

    assert!(state.chat_is_pending());
    assert_eq!(state.chat_messages().len(), 2);
    assert!(state.chat_prompt_draft().is_empty());
    assert_eq!(state.agent_steps().len(), 1);
    assert_eq!(state.agent_steps()[0].status, AiAgentStepStatus::Planning);
    let mut rx = state
        .take_chat_event_receiver()
        .expect("the state holds its receiver until the drain starts");
    launch
        .tx
        .unbounded_send(AiChatWorkerEvent::Delta {
            job_id: launch.job_id,
            session_id: launch.session_id.clone(),
            text_delta: "working".to_string(),
            reasoning_delta: Some("reason".to_string()),
        })
        .unwrap();
    assert!(state.chat_event_is_wanted());
    let event = rx.try_recv().expect("the delta should be queued");
    let AiChatWorkerEvent::Delta {
        job_id,
        text_delta,
        reasoning_delta,
        ..
    } = event
    else {
        panic!("expected stream delta");
    };
    assert!(state.apply_chat_delta(job_id, &text_delta, reasoning_delta.as_deref()));
    assert_eq!(
        state.chat_response_preview(),
        "Running AI Agent step...working"
    );
    assert_eq!(
        state.chat_messages()[1].reasoning_content.as_deref(),
        Some("reason")
    );

    let effect = state
        .finish_chat_job(
            launch.job_id,
            launch.session_id,
            Ok(AiChatJobOutput {
                metadata: None,
                native_decision: None,
                mode: AiMode::Agent,
                text: "done".to_string(),
                reasoning: Some("final reason".to_string()),
                command_cards: Vec::new(),
                auto_execute_first: false,
                approval_note: None,
            }),
        )
        .unwrap();
    assert!(effect.succeeded);
    assert!(effect.clear_prompt_input);
    assert!(!state.chat_is_pending());
    assert_eq!(state.chat_response_preview(), "done");
    assert_eq!(state.agent_steps()[0].title, "Final Answer");
    assert!(state.agent_loop_snapshot().is_none());
}

#[test]
fn streaming_snapshot_does_not_share_mutable_message_arc() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let launch = state.begin_chat_request("inspect".to_string(), AiMode::Ask, None);
    assert!(state.apply_chat_delta(launch.job_id, "hello", None));

    let snapshot_messages = state.chat_snapshot_messages();
    let state_messages = state.chat_messages();
    assert_eq!(state_messages.len(), 2);
    assert_eq!(snapshot_messages.len(), 2);
    assert!(
        Arc::ptr_eq(&state_messages[0], &snapshot_messages[0]),
        "completed user messages should stay shared with the snapshot"
    );
    assert!(
        !Arc::ptr_eq(&state_messages[1], &snapshot_messages[1]),
        "the active streaming assistant message should be copied for snapshots"
    );
    assert_eq!(snapshot_messages[1].content, "hello");

    assert!(state.apply_chat_delta(launch.job_id, " world", None));
    assert_eq!(state.chat_messages()[1].content, "hello world");
    assert_eq!(
        snapshot_messages[1].content, "hello",
        "old snapshots should remain immutable after later streaming deltas"
    );
}

#[test]
fn chat_cancel_invalidates_the_job_and_clears_agent_lifecycle() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let launch = state.begin_chat_request("inspect".to_string(), AiMode::Agent, None);

    state.cancel_chat_and_agent();

    assert!(launch.cancel.load(Ordering::Relaxed));
    assert!(!state.chat_is_pending());
    assert_eq!(state.chat_response_preview(), "AI request cancelled");
    assert_eq!(state.agent_steps()[0].status, AiAgentStepStatus::Cancelled);
    assert!(
        state
            .finish_chat_job(launch.job_id, launch.session_id, Err("late".to_string()),)
            .is_none()
    );
}

#[test]
fn awaiting_agent_approval_remains_an_active_cancellable_run() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let launch = state.begin_chat_request("inspect".to_string(), AiMode::Agent, None);
    let card = nyaterm_core::AiCommandCard {
        id: "agent-command".to_string(),
        title: "Inspect".to_string(),
        command: "pwd".to_string(),
        explanation: String::new(),
        risk_level: None,
        risk_reason: None,
        expected_effect: String::new(),
        rollback: None,
        category: Some("AI Agent".to_string()),
        references: Vec::new(),
        target_terminal_session_id: Some("terminal-a".to_string()),
        target: None,
    };
    assert!(
        state
            .finish_chat_job(
                launch.job_id,
                launch.session_id.clone(),
                Ok(AiChatJobOutput {
                    metadata: None,
                    native_decision: None,
                    mode: AiMode::Agent,
                    text: "Run pwd".to_string(),
                    reasoning: None,
                    command_cards: vec![card],
                    auto_execute_first: false,
                    approval_note: None,
                }),
            )
            .is_some()
    );
    assert!(!state.chat_is_pending());
    assert!(state.chat_or_agent_is_running());
    assert!(state.ai_session_is_running(&launch.session_id));
    assert!(state.current_agent_command_card("agent-command"));
    state.cancel_chat_and_agent();
    assert!(!state.chat_or_agent_is_running());
    assert!(!state.current_agent_command_card("agent-command"));
}

#[test]
fn a_matching_job_id_cannot_finish_another_ai_session() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let launch = state.begin_chat_request("inspect".to_string(), AiMode::Ask, None);
    assert!(
        state
            .finish_chat_job(
                launch.job_id,
                "another-session".to_string(),
                Err("late".to_string())
            )
            .is_none()
    );
    assert!(state.chat_is_pending());
    state.cancel_chat_and_agent();
}

#[test]
fn agent_step_limit_ends_the_run_and_removes_pending_approval() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.begin_chat_request("inspect".to_string(), AiMode::Agent, None);
    state.chat.pending = false;
    assert!(state.begin_agent_step(1).is_err());
    assert!(!state.agent_task_is_active());
    assert!(!state.chat_or_agent_is_running());
}

#[test]
fn parallel_terminal_chats_isolate_cancel_and_late_results() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.switch_scope("terminal:a");
    let first = state.begin_chat_request("first".to_string(), AiMode::Ask, None);
    state.switch_scope("terminal:b");
    let second = state.begin_chat_request("second".to_string(), AiMode::Ask, None);
    assert_ne!(first.session_id, second.session_id);
    assert!(state.apply_chat_delta(second.job_id, "B", None));
    state.switch_scope("terminal:a");
    assert!(state.apply_chat_delta(first.job_id, "A", None));
    assert_eq!(state.chat_messages()[1].content, "A");
    state.cancel_chat_and_agent();
    assert!(
        state
            .finish_chat_job(first.job_id, first.session_id, Err("late".to_string()))
            .is_none()
    );
    state.switch_scope("terminal:b");
    assert!(state.chat_is_pending());
    assert_eq!(state.chat_messages()[1].content, "B");
    assert!(
        state
            .finish_chat_job(
                second.job_id,
                second.session_id,
                Ok(AiChatJobOutput {
                    metadata: None,
                    native_decision: None,
                    mode: AiMode::Ask,
                    text: "finished B".to_string(),
                    reasoning: None,
                    command_cards: Vec::new(),
                    auto_execute_first: false,
                    approval_note: None,
                })
            )
            .is_some()
    );
    assert_eq!(state.chat_messages()[1].content, "finished B");
}

#[test]
fn terminal_scopes_keep_run_mode_and_agent_protocol_independent() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.settings.config.codex.enabled = true;
    // Existing terminal drafts retain their mode; fresh drafts inherit the saved default.
    state.switch_scope("terminal:b");
    state.switch_scope("terminal:a");
    state.set_chat_run_mode(AiMode::Agent, AiAgentKind::Nyaterm);
    let first = state.begin_chat_request("inspect".to_string(), AiMode::Agent, None);
    assert!(state.apply_agent_protocol_fallback(first.job_id));
    state.switch_scope("terminal:b");
    assert_eq!(state.chat_run_mode(), AiMode::Ask);
    assert!(!state.agent_uses_json_protocol());
    state.set_chat_run_mode(AiMode::Agent, AiAgentKind::Codex);
    state.switch_scope("terminal:a");
    assert_eq!(state.chat_agent_kind(), AiAgentKind::Nyaterm);
    assert!(state.agent_uses_json_protocol());
    state.cancel_chat_and_agent();
    assert!(!state.agent_uses_json_protocol());
    state.switch_scope("terminal:b");
    assert_eq!(state.chat_agent_kind(), AiAgentKind::Codex);
}

#[test]
fn switching_through_ask_remembers_the_agent_runtime_without_persisting_it() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.settings.config.codex.enabled = true;
    assert!(state.set_chat_run_mode(AiMode::Agent, AiAgentKind::Codex));
    assert!(state.set_chat_run_mode(AiMode::Ask, AiAgentKind::Codex));
    assert_eq!(state.chat_agent_kind(), AiAgentKind::Nyaterm);
    assert_eq!(
        state.settings.config.default_agent_kind,
        AiAgentKind::Nyaterm
    );
    assert_eq!(state.last_agent_kind(), AiAgentKind::Codex);

    state.settings.config.codex.enabled = false;
    assert_eq!(state.last_agent_kind(), AiAgentKind::Nyaterm);
    assert!(!state.set_chat_run_mode(AiMode::Agent, AiAgentKind::Codex));
    state.settings.config.codex.enabled = true;
    assert!(state.set_chat_run_mode(AiMode::Agent, state.last_agent_kind()));
    assert_eq!(state.chat_agent_kind(), AiAgentKind::Codex);
}

#[test]
fn new_chat_preserves_idle_agent_steps_for_history_reload() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.switch_scope("terminal:a");
    let session_id = state.chat_session_id().to_string();
    state.begin_chat_request("inspect".to_string(), AiMode::Agent, None);
    let job_id = state.chat.job_id;
    state
        .finish_chat_job(
            job_id,
            session_id.clone(),
            Ok(AiChatJobOutput {
                metadata: None,
                native_decision: None,
                mode: AiMode::Agent,
                text: "done".to_string(),
                reasoning: None,
                command_cards: Vec::new(),
                auto_execute_first: false,
                approval_note: None,
            }),
        )
        .unwrap();
    state.start_new_chat();
    assert_eq!(
        state.scope_for_ai_session(&session_id).as_deref(),
        Some("terminal:a")
    );
    let new_session_id = state.chat_session_id().to_string();
    let history_job_id = state.begin_history_operation("loading").unwrap();
    assert!(state.finish_history_message_load(
        history_job_id,
        &new_session_id,
        session_id,
        Ok(Vec::new()),
        "loaded".to_string(),
    ));
    assert_eq!(state.agent_steps()[0].title, "Final Answer");
}

#[test]
fn mention_selection_and_navigation_are_atomic_owner_transitions() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.set_chat_prompt_draft("run @server".to_string());
    assert!(state.chat_mention_is_open());
    assert_eq!(state.chat_mention_query(), "server");

    state.move_chat_mention_index(3, -1);
    assert_eq!(state.chat_mention_index(), 2);
    state.hide_chat_mention();
    assert_eq!(state.chat_mention_index(), 2);
    state.set_chat_prompt_draft("run @server".to_string());
    state.select_chat_mention("session-a".to_string(), "Server A".to_string());

    assert_eq!(state.chat_prompt_draft(), "run ");
    assert_eq!(state.chat_target_session_ids(), &["session-a".to_string()]);
    assert!(!state.chat_mention_is_open());
    assert_eq!(state.panel_status(), "AI target session selected: Server A");

    state.remove_chat_target_session("session-a");
    assert!(state.chat_target_session_ids().is_empty());
    assert_eq!(state.panel_status(), "AI target sessions cleared");
}

#[test]
fn agent_step_limit_and_command_association_stay_on_the_owner() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    assert!(state.begin_agent_step(1).is_err());

    state.set_agent_loop(AiAgentLoopState {
        execution_id: "fixture-execution".into(),
        command_card_id: None,
        available_targets: Vec::new(),
        default_target_session_id: None,
        ai_session_id: "session-a".to_string(),
        terminal_session_id: "terminal-a".to_string(),
        command: "pwd".to_string(),

        step_index: 0,
        max_steps: 3,
    });

    assert!(
        state
            .take_agent_loop_for_session("other-terminal")
            .is_none()
    );
    assert!(state.take_agent_loop_for_session("terminal-a").is_some());
    assert!(state.agent_loop_snapshot().is_none());
}

#[test]
fn external_request_preparation_sets_request_status_focus_and_closes_menus() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.toggle_history();
    let request = AiPreparedRequest {
        action: AiAction::CustomFileAction,
        context: AiContext::default(),
        source_label: "remote file".to_string(),
    };

    state.prepare_external_request(request.clone(), "ready", "loaded", true);

    assert_eq!(state.chat_prepared_request(), Some(&request));
    assert_eq!(state.chat_response_preview(), "ready");
    assert_eq!(state.panel_status(), "loaded");
    assert!(state.chat_focus_is_pending());
    assert!(!state.history_is_open());
}

#[test]
fn detected_error_throttle_and_picker_indices_are_owned_transitions() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let now = Instant::now();

    assert!(state.note_detected_error("session".to_string(), "first".to_string(), now,));
    assert!(!state.note_detected_error(
        "session".to_string(),
        "second".to_string(),
        now + Duration::from_secs(29),
    ));
    assert!(state.note_detected_error(
        "session".to_string(),
        "third".to_string(),
        now + Duration::from_secs(30),
    ));

    state.set_discovery_index(9);
    state.set_chat_mention_index(7);
    assert_eq!(state.clamp_discovery_index(3), 2);
    assert_eq!(state.clamp_chat_mention_index(2), 1);
    assert_eq!(state.clamp_discovery_index(0), 0);
    assert_eq!(state.clamp_chat_mention_index(0), 0);
}

#[test]
fn panel_status_and_error_banner_change_only_through_owner_operations() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let now = Instant::now();

    state.set_panel_status("completed");
    assert_eq!(state.panel_status(), "completed");
    state.set_panel_status("replacement");
    assert_eq!(state.panel_status(), "replacement");

    assert!(state.note_detected_error("session".to_string(), "failure".to_string(), now,));
    assert!(state.panel_detected_error().is_some());
    state.clear_detected_error();
    assert!(state.panel_detected_error().is_none());
    assert_eq!(state.panel_status(), "terminal error detected");
}

#[test]
fn provider_presets_keep_accounts_models_and_secret_drafts_isolated() {
    use nyaterm_core::ai::provider_settings::model_belongs_to_provider;
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.settings.config.provider_credentials.clear();
    state.settings.config.models.clear();
    let first = state.add_settings_provider_preset(AiProviderKind::Openai);
    let second = state.add_settings_provider_preset(AiProviderKind::Openai);
    assert_ne!(first, second);
    let credentials = &state.settings.config.provider_credentials;
    assert_ne!(credentials[0].name, credentials[1].name);
    assert!(
        state
            .settings
            .config
            .models
            .iter()
            .filter(|model| model_belongs_to_provider(model, &credentials[0]))
            .all(|model| !model_belongs_to_provider(model, &credentials[1]))
    );
    state.apply_settings_credential_input(&format!("{first}.api-key"), "fixture-first".into());
    state.apply_settings_credential_input(&format!("{second}.api-key"), "fixture-second".into());
    let pending = state.pending_settings();
    assert_eq!(
        pending
            .provider_credentials
            .iter()
            .find(|item| item.id == first)
            .unwrap()
            .api_key
            .as_deref(),
        Some("fixture-first")
    );
    assert_eq!(
        pending
            .provider_credentials
            .iter()
            .find(|item| item.id == second)
            .unwrap()
            .api_key
            .as_deref(),
        Some("fixture-second")
    );
    state.remove_settings_credential(&first);
    assert!(
        !state
            .settings
            .config
            .models
            .iter()
            .any(|model| model.credential_id.as_deref() == Some(&first))
    );
    assert!(
        state
            .settings
            .config
            .models
            .iter()
            .any(|model| model.credential_id.as_deref() == Some(&second))
    );
}

#[test]
fn manual_model_add_updates_visible_rows_and_preserves_a_valid_default() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.settings.config.provider_credentials.clear();
    state.settings.config.models.clear();
    let id = state.add_settings_provider_preset(AiProviderKind::OpenaiCompatible);
    state.add_settings_manual_model(&id, "first");
    let default = state.settings.config.default_model_id.clone();
    state.add_settings_manual_model(&id, "second");
    assert_eq!(state.provider_view().model_order.len(), 2);
    assert_eq!(state.settings.config.default_model_id, default);
    let second = state
        .settings
        .config
        .models
        .iter()
        .find(|model| model.name == "second")
        .unwrap()
        .id
        .clone();
    let order = state.provider_view().model_order.clone();
    state.toggle_settings_model_enabled(&second);
    state.add_settings_manual_model(&id, "second");
    assert_eq!(state.provider_view().model_order, order);
    assert_eq!(state.settings.config.default_model_id, default);
    assert_eq!(
        state.add_settings_manual_model(&id, "second"),
        AiSettingsMutation::Notify
    );
}

#[test]
fn provider_model_rows_keep_order_when_toggled_and_deleted_default_falls_back() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.settings.config.provider_credentials.clear();
    state.settings.config.models.clear();
    let id = state.add_settings_provider_preset(AiProviderKind::Openai);
    state.add_settings_manual_model(&id, "first");
    state.add_settings_manual_model(&id, "second");
    state.select_settings_provider(Some(id));
    let before = state.provider_view().model_order.clone();
    let first = before[0].clone();
    let second = before[1].clone();
    state.toggle_settings_model_enabled(&first);
    state.refresh_provider_model_order();
    assert_eq!(state.provider_view().model_order, before);
    state.toggle_settings_model_enabled(&first);
    state.set_settings_default_model(&first);
    state.remove_settings_model(&first);
    assert_eq!(
        state.settings.config.default_model_id.as_deref(),
        Some(second.as_str())
    );
}

#[test]
fn provider_changes_and_cancel_invalidate_background_results_and_secret_drafts() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let baseline = state.settings_draft_snapshot();
    assert!(state.settings_draft_matches(&baseline.0, &baseline.1, &baseline.2, &baseline.3));
    state.add_settings_provider_preset(AiProviderKind::Openai);
    state.prepare_provider_settings();
    let id = state.provider_view().selected_id.clone().unwrap();
    let generation = state.provider_view().generation;
    state.apply_settings_credential_input(
        &format!("{id}.base-url"),
        "http://example.invalid/".into(),
    );
    assert_ne!(state.provider_view().generation, generation);
    let generation = state.provider_view().generation;
    state.apply_settings_credential_input(&format!("{id}.api-key"), "fixture-draft".into());
    assert_ne!(state.provider_view().generation, generation);
    assert!(!state.settings_draft_matches(&baseline.0, &baseline.1, &baseline.2, &baseline.3));
    let generation = state.provider_view().generation;
    state.restore_settings_draft(baseline.0, baseline.1, baseline.2, baseline.3);
    assert_ne!(state.provider_view().generation, generation);
    assert!(state.settings_credential_secret_drafts().is_empty());
    let restored = state.settings_draft_snapshot();
    state.replace_settings_config(restored.0, true);
    assert!(state.settings_credential_secret_drafts().is_empty());
}

#[test]
fn provider_batch_keeps_successes_when_an_account_fails_and_ignores_stale_results() {
    use crate::features::ai::ConnectionStatus;
    use nyaterm_core::ai::AiModelDiscovery;
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let id = state.add_settings_provider_preset(AiProviderKind::OpenaiCompatible);
    let generation = state.provider_view().generation;
    let discovery = AiModelDiscovery {
        id: format!("{id}:fixture"),
        name: "fixture".into(),
        provider_kind: Some(AiProviderKind::OpenaiCompatible),
        credential_id: Some(id.clone()),
        source: AiModelSource::RustGenai,
    };
    assert!(state.complete_provider_discoveries(
        generation,
        vec![
            (id.clone(), Ok(vec![discovery.clone()])),
            ("failed-account".into(), Err("fixture failure".into()))
        ],
        true
    ));
    assert!(
        state
            .settings
            .config
            .models
            .iter()
            .any(|model| model.id == discovery.id)
    );
    assert!(state.provider_view().statuses[&id] == ConnectionStatus::Success);
    assert!(state.provider_view().statuses["failed-account"] == ConnectionStatus::Error);
    state.remove_settings_model(&discovery.id);
    assert!(!state.complete_provider_discoveries(
        generation,
        vec![(id, Ok(vec![discovery.clone()]))],
        true
    ));
    assert!(
        !state
            .settings
            .config
            .models
            .iter()
            .any(|model| model.id == discovery.id)
    );
}

#[test]
fn connection_checks_do_not_modify_settings_and_reasoning_edits_fall_back_to_auto() {
    use nyaterm_core::ai::{AiModelDiscovery, AiModelReasoningEffort, AiReasoningEffort};
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let id = state.add_settings_provider_preset(AiProviderKind::OpenaiCompatible);
    state.add_settings_manual_model(&id, "fixture");
    let before = state.settings.config.clone();
    assert!(state.complete_provider_discoveries(
        state.provider_view().generation,
        vec![(
            id.clone(),
            Ok(vec![AiModelDiscovery {
                id: format!("{id}:other"),
                name: "other".into(),
                provider_kind: Some(AiProviderKind::OpenaiCompatible),
                credential_id: Some(id.clone()),
                source: AiModelSource::RustGenai
            }])
        )],
        false
    ));
    assert_eq!(state.settings.config, before);
    let model_id = format!("{id}:fixture");
    state.set_settings_default_model(&model_id);
    state.set_settings_reasoning_effort(AiReasoningEffort::High);
    state.toggle_model_reasoning(&model_id, AiModelReasoningEffort::High);
    assert_eq!(
        state.settings.config.default_reasoning_effort,
        AiReasoningEffort::Auto
    );
}

#[test]
fn selected_run_mode_survives_settings_round_trip_and_seeds_new_terminal_drafts() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.settings.config.codex.enabled = true;
    assert!(state.set_chat_run_mode(AiMode::Agent, AiAgentKind::Codex));
    let encoded = serde_json::to_string(state.settings_config()).unwrap();
    let restored: AiSettings = serde_json::from_str(&encoded).unwrap();
    assert_eq!(restored.default_mode, AiMode::Agent);
    assert_eq!(restored.default_agent_kind, AiAgentKind::Codex);
    state.switch_scope("terminal:new");
    assert_eq!(state.chat_run_mode(), AiMode::Agent);
    assert_eq!(state.chat_agent_kind(), AiAgentKind::Codex);
    assert!(state.set_chat_run_mode(AiMode::Ask, AiAgentKind::Codex));
    assert_eq!(state.settings_config().default_mode, AiMode::Ask);
    assert_eq!(
        state.settings_config().default_agent_kind,
        AiAgentKind::Nyaterm
    );
}

#[test]
fn disabled_external_agent_cannot_replace_the_current_mode_or_saved_default() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    assert!(state.set_chat_run_mode(AiMode::Agent, AiAgentKind::Nyaterm));
    for kind in [AiAgentKind::Codex, AiAgentKind::ClaudeCode] {
        assert!(!state.set_chat_run_mode(AiMode::Agent, kind));
        assert_eq!(state.chat_run_mode(), AiMode::Agent);
        assert_eq!(state.chat_agent_kind(), AiAgentKind::Nyaterm);
        assert_eq!(
            state.settings_config().default_agent_kind,
            AiAgentKind::Nyaterm
        );
    }
}

#[test]
fn disabling_an_external_agent_falls_back_to_ask_for_its_existing_terminal_draft() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.settings.config.codex.enabled = true;
    state.set_chat_run_mode(AiMode::Agent, AiAgentKind::Codex);
    state.toggle_settings_codex_enabled();
    assert_eq!(state.chat_run_mode(), AiMode::Ask);
    assert_eq!(state.chat_agent_kind(), AiAgentKind::Nyaterm);
}

#[test]
fn response_phases_stop_thinking_when_visible_text_or_tool_arguments_arrive() {
    use crate::features::ai::presentation::AiResponsePhase;
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let launch = state.begin_chat_request("inspect".into(), AiMode::Ask, None);
    assert_eq!(state.response_phase(), AiResponsePhase::Waiting);
    assert!(state.apply_chat_delta(launch.job_id, "<think>consider", None));
    assert_eq!(state.response_phase(), AiResponsePhase::Thinking);
    assert!(state.apply_chat_delta(launch.job_id, "</think>Answer", None));
    assert_eq!(state.response_phase(), AiResponsePhase::Responding);
    assert!(state.apply_chat_delta(launch.job_id, "", Some("late thought")));
    assert_eq!(state.response_phase(), AiResponsePhase::Responding);
    state.cancel_chat_and_agent();
    assert_eq!(state.response_phase(), AiResponsePhase::Ended);
    assert!(!state.apply_chat_delta(launch.job_id, "late output", None));
    assert!(!state.apply_agent_tool_delta(launch.job_id, Some("execute_command"), 4));

    let launch = state.begin_chat_request("inspect".into(), AiMode::Agent, None);
    assert!(state.apply_agent_tool_delta(launch.job_id, Some("execute_command"), 4));
    assert_eq!(state.response_phase(), AiResponsePhase::ToolArguments);
    assert!(state.apply_chat_delta(launch.job_id, "", Some("reason")));
    assert_eq!(state.response_phase(), AiResponsePhase::ToolArguments);
    assert!(state.apply_agent_protocol_fallback(launch.job_id));
    assert_eq!(state.response_phase(), AiResponsePhase::Waiting);
}

fn presentation_command(id: &str) -> nyaterm_core::AiCommandCard {
    nyaterm_core::AiCommandCard {
        id: id.into(),
        title: "Inspect".into(),
        command: "echo fixture".into(),
        explanation: "Read only".into(),
        risk_level: Some(nyaterm_core::RiskLevel::Low),
        risk_reason: Some("No changes".into()),
        expected_effect: String::new(),
        rollback: None,
        category: None,
        references: Vec::new(),
        target_terminal_session_id: Some("terminal-a".into()),
        target: None,
    }
}

#[test]
fn agent_proposals_link_the_exact_message_and_card_before_execution() {
    use crate::features::ai::presentation::{AiAgentStepKind, AiCommandPhase, AiResponsePhase};
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    for automatic in [false, true] {
        let mut state = state(&cx);
        let launch = state.begin_chat_request("inspect".into(), AiMode::Agent, None);
        let assistant_id = state.chat_streaming_assistant_id().unwrap().to_string();
        state.apply_chat_delta(launch.job_id, "streamed protocol", None);
        state
            .finish_chat_job(
                launch.job_id,
                launch.session_id,
                Ok(AiChatJobOutput {
                    metadata: None,
                    native_decision: None,
                    mode: AiMode::Agent,
                    text: String::new(),
                    reasoning: Some("Read only".into()),
                    command_cards: vec![presentation_command("agent-fixture")],
                    auto_execute_first: automatic,
                    approval_note: Some("Review this command".into()),
                }),
            )
            .unwrap();
        let step = &state.agent_steps()[0];
        assert_eq!(
            step.source_message_id.as_deref(),
            Some(assistant_id.as_str())
        );
        assert_eq!(step.command_card_id.as_deref(), Some("agent-fixture"));
        assert_eq!(step.command.as_deref(), Some("echo fixture"));
        assert_eq!(step.kind, AiAgentStepKind::ToolProgress);
        assert_eq!(
            AiCommandPhase::from_step(step),
            if automatic {
                AiCommandPhase::Preparing
            } else {
                AiCommandPhase::NeedsApproval
            }
        );
        assert!(
            state.chat_messages()[1].content.is_empty(),
            "protocol text must not duplicate the execution block"
        );
        assert_eq!(state.response_phase(), AiResponsePhase::Ended);
        assert!(!state.apply_chat_delta(launch.job_id, "late", None));
        assert!(!state.apply_agent_tool_delta(launch.job_id, None, 8));
    }
}

#[test]
fn final_answers_are_not_classified_as_thoughts_commands_or_output() {
    use crate::features::ai::presentation::AiAgentStepKind;
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let launch = state.begin_chat_request("inspect".into(), AiMode::Agent, None);
    let id = state.chat_streaming_assistant_id().unwrap().to_string();
    state
        .finish_chat_job(
            launch.job_id,
            launch.session_id,
            Ok(AiChatJobOutput {
                metadata: None,
                native_decision: None,
                mode: AiMode::Agent,
                text: "## Result\nAll good".into(),
                reasoning: None,
                command_cards: Vec::new(),
                auto_execute_first: false,
                approval_note: None,
            }),
        )
        .unwrap();
    let step = &state.agent_steps()[0];
    assert_eq!(step.kind, AiAgentStepKind::FinalAnswer);
    assert_eq!(step.source_message_id.as_deref(), Some(id.as_str()));
    assert!(step.thought.is_none() && step.command.is_none() && step.observation.is_none());
    assert_eq!(state.chat_messages()[1].content, "## Result\nAll good");
}

#[test]
fn consecutive_agent_commands_and_full_final_answer_own_distinct_assistant_messages() {
    use crate::features::ai::presentation::{AiAgentStepKind, AiResponsePhase};
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    for automatic in [false, true] {
        let mut state = state(&cx);
        let mut launch = state.begin_chat_request("inspect".into(), AiMode::Agent, None);
        let mut message_ids = Vec::new();
        for index in 0..2 {
            let id = state.chat_streaming_assistant_id().unwrap().to_string();
            message_ids.push(id.clone());
            let card_id = format!("agent-command-{index}");
            state
                .finish_chat_job(
                    launch.job_id,
                    launch.session_id.clone(),
                    Ok(AiChatJobOutput {
                        metadata: None,
                        native_decision: None,
                        mode: AiMode::Agent,
                        text: String::new(),
                        reasoning: Some("Inspect safely".into()),
                        command_cards: vec![presentation_command(&card_id)],
                        auto_execute_first: automatic,
                        approval_note: None,
                    }),
                )
                .unwrap();
            let (_, step_index) = state.begin_agent_step(4).unwrap();
            let execution = AiAgentLoopState {
                execution_id: "fixture-execution".into(),
                command_card_id: Some(card_id.clone()),
                ai_session_id: launch.session_id.clone(),
                terminal_session_id: "terminal-a".into(),
                available_targets: Vec::new(),
                default_target_session_id: None,
                command: "echo fixture".into(),

                step_index,
                max_steps: 4,
            };
            state.set_agent_loop(execution.clone());
            state.record_agent_observation(
                step_index,
                &nyaterm_core::CommandObservation {
                    output: "fixture output".into(),
                    exit_code: Some(0),
                    duration_ms: 1,
                },
                "Observation".into(),
            );
            let step = &state.agent_steps()[usize::from(step_index)];
            assert_eq!(step.command_card_id.as_deref(), Some(card_id.as_str()));
            assert_eq!(step.source_message_id.as_deref(), Some(id.as_str()));
            launch = state
                .begin_agent_continuation(&execution, "fixture output")
                .unwrap();
            assert_eq!(state.response_phase(), AiResponsePhase::Waiting);
            assert!(state.apply_chat_delta(launch.job_id, "", Some("Next step")));
            assert_eq!(state.response_phase(), AiResponsePhase::Thinking);
        }
        let final_id = state.chat_streaming_assistant_id().unwrap().to_string();
        assert!(!message_ids.contains(&final_id));
        let answer = format!(
            "## Result\n{}\nFull answer ending.",
            "Detailed finding. ".repeat(30)
        );
        assert!(state.apply_chat_delta(launch.job_id, "## Result", None));
        assert_eq!(state.response_phase(), AiResponsePhase::Responding);
        state
            .finish_chat_job(
                launch.job_id,
                launch.session_id,
                Ok(AiChatJobOutput {
                    metadata: None,
                    native_decision: None,
                    mode: AiMode::Agent,
                    text: answer.clone(),
                    reasoning: None,
                    command_cards: Vec::new(),
                    auto_execute_first: false,
                    approval_note: None,
                }),
            )
            .unwrap();
        assert_eq!(state.response_phase(), AiResponsePhase::Ended);
        assert_eq!(state.chat_messages().len(), 4);
        assert_eq!(state.chat_messages()[3].id, final_id);
        assert_eq!(state.chat_messages()[3].content, answer);
        let final_step = state.agent_steps().last().unwrap();
        assert_eq!(final_step.kind, AiAgentStepKind::FinalAnswer);
        assert_eq!(
            final_step.source_message_id.as_deref(),
            Some(final_id.as_str())
        );
        assert!(final_step.thought.is_none() && final_step.observation.is_none());
        assert!(!state.apply_chat_delta(launch.job_id, "late", None));
    }
}

#[test]
fn foreign_agent_continuation_cannot_claim_the_active_response() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let launch = state.begin_chat_request("inspect".into(), AiMode::Agent, None);
    let assistant_id = state.chat_streaming_assistant_id().unwrap().to_string();
    let execution = AiAgentLoopState {
        execution_id: "fixture-execution".into(),
        command_card_id: None,
        ai_session_id: "another-session".into(),
        terminal_session_id: "terminal-a".into(),
        available_targets: Vec::new(),
        default_target_session_id: None,
        command: "echo fixture".into(),

        step_index: 0,
        max_steps: 4,
    };
    assert!(
        state
            .begin_agent_continuation(&execution, "foreign output")
            .is_none()
    );
    assert_eq!(
        state.chat_streaming_assistant_id(),
        Some(assistant_id.as_str())
    );
    assert_eq!(state.chat_messages().len(), 2);
    assert!(state.apply_chat_delta(launch.job_id, "active answer", None));
}

#[test]
fn command_results_distinguish_success_failure_and_unknown_exit_codes() {
    use crate::features::ai::presentation::{AiAgentStepKind, AiCommandPhase};
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    for (index, code, phase) in [
        (0, Some(0), AiCommandPhase::Completed),
        (1, Some(7), AiCommandPhase::Failed),
        (2, None, AiCommandPhase::Observed),
    ] {
        state.upsert_agent_step(
            index,
            AiAgentStepStatus::Running,
            AiAgentStepKind::Command,
            "Running",
            "echo fixture",
        );
        state.record_agent_observation(
            index,
            &nyaterm_core::CommandObservation {
                output: "line one\nline two".into(),
                exit_code: code,
                duration_ms: 20,
            },
            "summary".into(),
        );
        let step = state
            .agent_steps()
            .iter()
            .find(|step| step.step_index == index)
            .unwrap();
        assert_eq!(AiCommandPhase::from_step(step), phase);
        assert_eq!(step.command.as_deref(), Some("echo fixture"));
        assert_eq!(step.observation.as_deref(), Some("line one\nline two"));
        assert!(!phase.offers_approval() && !phase.offers_run());
        assert!(phase.offers_reuse());
    }
}

#[test]
fn payload_categories_do_not_depend_on_english_titles() {
    use crate::features::ai::presentation::AiAgentStepKind;
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.upsert_agent_step(
        0,
        AiAgentStepStatus::Completed,
        AiAgentStepKind::Diagnostic,
        "Final Answer shell running",
        "diagnostic",
    );
    assert!(state.agent_steps()[0].thought.is_none());
    assert!(state.agent_steps()[0].command.is_none());
    assert_eq!(
        state.agent_steps()[0].observation.as_deref(),
        Some("diagnostic")
    );
}

#[test]
fn message_disclosure_choices_follow_their_conversation_scope() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    state.switch_scope("terminal:a");
    state.toggle_message_thought("assistant-a".into());
    state.toggle_command_details("agent-a".into());
    state.toggle_command_script("agent-a".into());
    state.toggle_agent_history();
    state.toggle_execution_group("user-a".into());
    state.switch_scope("terminal:b");
    assert!(state.expanded_message_thoughts().is_empty());
    assert!(state.expanded_command_details().is_empty());
    assert!(state.expanded_command_scripts().is_empty());
    assert!(!state.agent_history_expanded());
    assert!(state.expanded_execution_groups().is_empty());
    state.toggle_message_thought("assistant-b".into());
    state.switch_scope("terminal:a");
    assert!(state.expanded_message_thoughts().contains("assistant-a"));
    assert!(!state.expanded_message_thoughts().contains("assistant-b"));
    assert!(state.expanded_command_scripts().contains("agent-a"));
    assert!(state.agent_history_expanded());
    assert!(state.expanded_execution_groups().contains("user-a"));
    state.start_new_chat();
    assert!(state.expanded_message_thoughts().is_empty());
    assert!(state.expanded_command_details().is_empty());
    assert!(state.expanded_command_scripts().is_empty());
    assert!(!state.agent_history_expanded());
    assert!(state.expanded_execution_groups().is_empty());
}

#[test]
fn settled_agent_cards_cannot_be_approved_again_and_keep_their_identity() {
    use crate::features::ai::presentation::{AiAgentStepKind, AiCommandPhase};
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let launch = state.begin_chat_request("inspect".into(), AiMode::Agent, None);
    state
        .finish_chat_job(
            launch.job_id,
            launch.session_id,
            Ok(AiChatJobOutput {
                metadata: None,
                native_decision: None,
                mode: AiMode::Agent,
                text: String::new(),
                reasoning: None,
                command_cards: vec![presentation_command("agent-fixture")],
                auto_execute_first: false,
                approval_note: None,
            }),
        )
        .unwrap();
    assert!(state.current_agent_command_card("agent-fixture"));
    for status in [
        AiAgentStepStatus::Running,
        AiAgentStepStatus::Completed,
        AiAgentStepStatus::Failed,
        AiAgentStepStatus::Rejected,
        AiAgentStepStatus::Cancelled,
    ] {
        state.upsert_agent_step(
            0,
            status,
            AiAgentStepKind::Command,
            "Fixture",
            "command preview",
        );
        assert!(!state.current_agent_command_card("agent-fixture"));
        assert_eq!(
            state.agent_steps()[0].command.as_deref(),
            Some("echo fixture")
        );
        assert_eq!(
            state.agent_steps()[0].command_card_id.as_deref(),
            Some("agent-fixture")
        );
    }
    state.cancel_chat_and_agent();
    assert_eq!(
        AiCommandPhase::from_step(&state.agent_steps()[0]),
        AiCommandPhase::Cancelled
    );
}

#[test]
fn native_run_freezes_only_its_session_history_before_the_current_task() {
    use nyaterm_core::ai::{AiChatRequest, AiMessage, AiMessageRole};
    use std::sync::Arc;
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let session = state.chat.session_id.clone();
    let message = |owner: &str, role, content: &str| {
        Arc::new(AiMessage {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: owner.into(),
            role,
            content: content.into(),
            created_at: String::new(),
            reasoning_content: None,
            command_cards: vec![],
        })
    };
    state.chat.messages = vec![
        message(&session, AiMessageRole::User, "old"),
        message("other-session", AiMessageRole::User, "unrelated"),
        message(&session, AiMessageRole::Assistant, "previous answer"),
        message(&session, AiMessageRole::User, "follow up"),
        message(&session, AiMessageRole::Assistant, ""),
    ];
    let request: AiChatRequest = serde_json::from_value(serde_json::json!({
        "mode":"agent", "action":"generate_command", "sessionId":session,
        "userInput":"follow up", "options":{"historyTurns":1}
    }))
    .unwrap();
    state.begin_native_run(request);
    let initial = state.native_initial_history().unwrap();
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0].content, "previous answer");
    state.chat.messages.clear();
    assert_eq!(state.native_initial_history().unwrap(), initial);
    state.switch_scope("terminal:other");
    assert!(state.native_initial_history().is_none());
}

#[test]
fn native_terminal_execution_policy_is_snapshotted_per_scope_and_run() {
    let cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let request = || {
        serde_json::from_value(
            serde_json::json!({"mode":"agent","action":"generate_command","userInput":"fixture"}),
        )
        .unwrap()
    };
    let mut settings = state.settings_config_cloned();
    settings.agent_background_execution_enabled = false;
    settings.terminal_output_lines = 7;
    state.replace_settings_config(settings, true);
    state.switch_scope("terminal:first");
    state.begin_native_run(request());
    let first = state.native_terminal_policy().unwrap();
    let mut settings = state.settings_config_cloned();
    settings.agent_background_execution_enabled = true;
    settings.terminal_output_lines = 0;
    state.replace_settings_config(settings, true);
    state.switch_scope("terminal:second");
    state.begin_native_run(request());
    let second = state.native_terminal_policy().unwrap();
    assert!(second.background);
    assert_eq!(second.max_lines, 0);
    state.switch_scope("terminal:first");
    assert_eq!(state.native_terminal_policy(), Some(first));
    assert!(!first.background);
    assert_eq!(first.max_lines, 7);
}

#[test]
fn provider_edits_keep_running_snapshot_and_deletion_cancels_inactive_run() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = state(&cx);
    let id = state.add_settings_provider_preset(AiProviderKind::OpenaiCompatible);
    state
        .settings
        .config
        .provider_credentials
        .iter_mut()
        .find(|credential| credential.id == id)
        .unwrap()
        .base_url = Some("http://localhost:12345/v1".into());
    state.add_settings_manual_model(&id, "test-model");
    let mut request: nyaterm_core::ai::AiChatRequest = serde_json::from_value(
        serde_json::json!({"mode":"ask","action":"explain_output","userInput":"test"}),
    )
    .unwrap();
    request.model_id = state.settings.config.default_model_id.clone();
    state.begin_chat_request("test".into(), AiMode::Ask, None);
    let initial = state.provider_session(&request).unwrap();
    let mut config = state.settings_config_cloned();
    config.agent_command_execution_mode = nyaterm_core::AgentCommandExecutionMode::Auto;
    config.agent_step_timeout_ms = Some(4321);
    config
        .provider_credentials
        .iter_mut()
        .find(|credential| credential.id == id)
        .unwrap()
        .base_url = Some("http://localhost:54321/v1".into());
    state.replace_settings_config(config, false);
    let still_running = state.provider_session(&request).unwrap();
    assert!(Arc::ptr_eq(&initial, &still_running));
    assert_eq!(
        state.run_settings().agent_command_execution_mode,
        initial.settings.agent_command_execution_mode
    );
    assert_eq!(
        state.run_settings().agent_step_timeout_ms,
        initial.settings.agent_step_timeout_ms
    );
    assert!(state.chat.pending);
    state.begin_chat_request("new".into(), AiMode::Ask, None);
    let next = state.provider_session(&request).unwrap();
    assert!(!Arc::ptr_eq(&initial, &next));
    assert_eq!(
        state.run_settings().agent_command_execution_mode,
        nyaterm_core::AgentCommandExecutionMode::Auto
    );
    assert_eq!(state.run_settings().agent_step_timeout_ms, Some(4321));
    assert!(
        next.settings
            .provider_credentials
            .iter()
            .find(|credential| credential.id == id)
            .unwrap()
            .base_url
            .as_deref()
            .unwrap()
            .contains("54321")
    );
    state.switch_visible_scope("terminal:other");
    let mut config = state.settings_config_cloned();
    config
        .provider_credentials
        .retain(|credential| credential.id != id);
    state.replace_settings_config(config, false);
    state.switch_visible_scope("unbound:");
    assert!(!state.chat.pending);
    assert!(state.chat.provider_session.is_none());
}
